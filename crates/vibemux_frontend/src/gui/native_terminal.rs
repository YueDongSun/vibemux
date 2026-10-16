//! Interactive terminal presentation over an owned out-of-process host.
use super::C;
use egui::{self, Key, RichText};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
};
use vibemux_terminal::{
    client::TerminalClient,
    protocol::{MAX_COLUMNS, MAX_INPUT_BYTES, MAX_ROWS, Result},
    wire::{Screen, Size, command},
};

pub const TERMINAL_WIDGET_ID: &str = "claude_terminal_surface";
const FONT_SIZE: f32 = 12.0;
const CELL_HEIGHT: f32 = 17.0;

#[derive(Default)]
pub struct EmbeddedTerminal {
    client: Option<TerminalClient>,
    pending: Option<Receiver<Result<TerminalClient>>>,
    launch_worker: Option<JoinHandle<()>>,
    error: Option<&'static str>,
    last_size: Option<Size>,
    focus_requested: bool,
    composing: bool,
}
impl EmbeddedTerminal {
    pub fn start(&mut self, ctx: &egui::Context, root: PathBuf, fixture: bool) {
        if self.launch_worker.is_some()
            || self.client.as_ref().is_some_and(TerminalClient::is_active)
        {
            return;
        }
        self.client.take();
        self.error = None;
        self.last_size = None;
        let context = ctx.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.pending = Some(rx);
        self.launch_worker = Some(std::thread::spawn(move || {
            let result = (|| {
                let (host, open) = vibemux_terminal::client::open_request(
                    &root,
                    fixture,
                    Size {
                        columns: 100,
                        rows: 30,
                    },
                )?;
                let repaint = context.clone();
                TerminalClient::start(&host, open, Arc::new(move || repaint.request_repaint()))
            })();
            let _ = tx.send(result);
            context.request_repaint();
        }));
    }
    pub fn poll(&mut self) {
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(client) => {
                            self.client = Some(client);
                            self.focus_requested = true;
                        }
                        Err(error) => self.error = Some(error.0),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some("terminal_launch_failed");
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self
            .launch_worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            if let Some(worker) = self.launch_worker.take() {
                if worker.join().is_err() {
                    self.error = Some("terminal_launch_failed");
                }
            }
        }
        if let Some(client) = &mut self.client {
            client.poll();
        }
    }
    pub fn render(&mut self, ui: &mut egui::Ui, c: &C, demo: bool) {
        self.poll();
        let ready = self
            .client
            .as_ref()
            .and_then(|client| client.screen.as_ref())
            .is_some_and(|screen| screen.cells.iter().any(|cell| !cell.text.trim().is_empty()));
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("native_terminal_capture_ready"), ready)
        });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(c.tr("Claude terminal"))
                    .size(16.0)
                    .strong()
                    .color(c.txt),
            );
            let active = self.launch_worker.is_some()
                || self.client.as_ref().is_some_and(TerminalClient::is_active);
            if ui
                .add_enabled(!active && !demo, egui::Button::new(c.tr("Start Claude")))
                .clicked()
            {
                match std::env::current_dir() {
                    Ok(root) => self.start(ui.ctx(), root, false),
                    Err(_) => self.error = Some("terminal_directory_unavailable"),
                }
            }
            if ui
                .add_enabled(active, egui::Button::new(c.tr("Stop session")))
                .clicked()
            {
                self.pending.take();
                if let Some(client) = &mut self.client {
                    client.stop();
                }
            }
            ui.label(
                RichText::new(c.tr(self.phase_label()))
                    .size(11.0)
                    .color(c.muted),
            );
        });
        ui.label(RichText::new(c.tr("Claude's native TUI runs here. Click the terminal to type; Claude handles its own permission prompts.")).size(12.0).color(c.muted));
        if let Some(client) = &self.client {
            if !client.status.branch.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        c.tr("Isolated branch"),
                        client.status.branch
                    ))
                    .monospace()
                    .size(11.0)
                    .color(c.muted),
                );
            }
        }
        if let Some(error) = self.error.or_else(|| {
            self.client
                .as_ref()
                .map(|client| client.status.code.as_str())
                .filter(|code| !code.is_empty())
        }) {
            ui.label(
                RichText::new(format!("{}: {error}", c.tr("Terminal error")))
                    .size(12.0)
                    .color(c.danger),
            );
        }
        ui.add_space(6.0);
        let available = ui.available_size();
        let font = egui::FontId::monospace(FONT_SIZE);
        let cell_width = ui.fonts(|fonts| fonts.glyph_width(&font, 'M')).max(5.0);
        let size = Size {
            columns: ((available.x - 16.0) / cell_width)
                .floor()
                .clamp(2.0, MAX_COLUMNS as f32) as u32,
            rows: ((available.y - 16.0) / CELL_HEIGHT)
                .floor()
                .clamp(1.0, MAX_ROWS as f32) as u32,
        };
        let (_, rect) = ui.allocate_space(available);
        let response = ui.interact(
            rect,
            egui::Id::new(TERMINAL_WIDGET_ID),
            egui::Sense::click_and_drag(),
        );
        ui.painter()
            .rect_filled(rect, 8, egui::Color32::from_rgb(31, 30, 29));
        if response.clicked() || self.focus_requested {
            response.request_focus();
            self.focus_requested = false;
        }
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
            ui.output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    rect,
                    cursor_rect: rect,
                })
            });
        }
        let Some(client) = &mut self.client else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                c.tr("Start Claude to open an interactive native terminal."),
                egui::FontId::proportional(13.0),
                c.muted,
            );
            return;
        };
        if self.last_size != Some(size) && client.is_active() {
            if let Err(error) = client.send(command::Operation::Resize(size)) {
                self.error = Some(error.0);
            } else {
                self.last_size = Some(size);
            }
        }
        let origin = rect.min + egui::vec2(8.0, 8.0);
        if let Some(screen) = &client.screen {
            paint_screen(ui, rect, origin, cell_width, &font, screen);
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 1.0 {
                let _ = client.send(command::Operation::Scroll(
                    (scroll / CELL_HEIGHT).round() as i32
                ));
            }
        }
        if response.has_focus() && client.is_active() {
            let application = client
                .screen
                .as_ref()
                .is_some_and(|screen| screen.application_cursor);
            let bracketed = client
                .screen
                .as_ref()
                .is_some_and(|screen| screen.bracketed_paste);
            let events = ui.input(|input| input.events.clone());
            let ime_event = events
                .iter()
                .any(|event| matches!(event, egui::Event::Ime(_)));
            for event in &events {
                match event {
                    egui::Event::Ime(egui::ImeEvent::Preedit(text)) => {
                        self.composing = !text.is_empty()
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled) => {
                        self.composing = false
                    }
                    _ => {}
                }
            }
            let composing = self.composing || ime_event;
            for event in events {
                if matches!(&event,egui::Event::Paste(text)|egui::Event::Text(text)|egui::Event::Ime(egui::ImeEvent::Commit(text)) if text.len()>MAX_INPUT_BYTES-12)
                {
                    self.error = Some("terminal_input_limit");
                    break;
                }
                if let Some(bytes) = encode_event(&event, application, bracketed, composing) {
                    if let Err(error) = client.send(command::Operation::Input(bytes)) {
                        self.error = Some(error.0);
                        break;
                    }
                }
            }
        }
    }
    pub fn phase(&self) -> &str {
        self.client.as_ref().map_or(
            if self.pending.is_some() {
                "starting"
            } else {
                "not_started"
            },
            |client| client.status.phase.as_str(),
        )
    }

    pub fn phase_label(&self) -> &'static str {
        match self.phase() {
            "running" => "Running",
            "starting" => "Starting terminal…",
            "fixture" => "OFFLINE PTY FIXTURE",
            "stopping" => "Stopping…",
            "failed" => "Failed",
            "exited" => "Session exited",
            _ => "Not started",
        }
    }
}
impl Drop for EmbeddedTerminal {
    fn drop(&mut self) {
        self.pending.take();
        if let Some(worker) = self.launch_worker.take() {
            let _ = worker.join();
        }
        self.client.take();
    }
}

fn color(value: u32) -> egui::Color32 {
    egui::Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}
fn paint_screen(
    ui: &egui::Ui,
    clip: egui::Rect,
    origin: egui::Pos2,
    width: f32,
    font: &egui::FontId,
    screen: &Screen,
) {
    let Some(size) = screen.size else {
        return;
    };
    let painter = ui.painter().with_clip_rect(clip);
    for (index, cell) in screen.cells.iter().enumerate() {
        let column = index % size.columns as usize;
        let row = index / size.columns as usize;
        let rect = egui::Rect::from_min_size(
            origin + egui::vec2(column as f32 * width, row as f32 * CELL_HEIGHT),
            egui::vec2(width, CELL_HEIGHT),
        );
        painter.rect_filled(rect, 0, color(cell.background));
    }
    for (index, cell) in screen.cells.iter().enumerate() {
        if cell.flags & 1 != 0 || cell.text.trim().is_empty() {
            continue;
        }
        let column = index % size.columns as usize;
        let row = index / size.columns as usize;
        let position = origin + egui::vec2(column as f32 * width, row as f32 * CELL_HEIGHT);
        painter.text(
            position,
            egui::Align2::LEFT_TOP,
            &cell.text,
            font.clone(),
            color(cell.foreground),
        );
        if cell.flags & 4 != 0 {
            painter.line_segment(
                [
                    position + egui::vec2(0.0, CELL_HEIGHT - 2.0),
                    position + egui::vec2(width, CELL_HEIGHT - 2.0),
                ],
                egui::Stroke::new(1.0, color(cell.foreground)),
            );
        }
    }
    if screen.cursor_visible {
        let cursor = egui::Rect::from_min_size(
            origin
                + egui::vec2(
                    screen.cursor_column as f32 * width,
                    screen.cursor_row as f32 * CELL_HEIGHT,
                ),
            egui::vec2(width, CELL_HEIGHT),
        );
        painter.rect_stroke(
            cursor,
            0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(217, 119, 87)),
            egui::StrokeKind::Inside,
        );
    }
}

pub fn encode_event(
    event: &egui::Event,
    application: bool,
    bracketed: bool,
    composing: bool,
) -> Option<Vec<u8>> {
    let bytes = match event {
        egui::Event::Text(text) if !composing => text.as_bytes().to_vec(),
        egui::Event::Ime(egui::ImeEvent::Commit(text)) => text.as_bytes().to_vec(),
        egui::Event::Paste(text) => {
            let text: String = text
                .chars()
                .filter(|character| {
                    !character.is_control() || matches!(character, '\r' | '\n' | '\t')
                })
                .collect();
            if bracketed {
                format!("\x1b[200~{text}\x1b[201~").into_bytes()
            } else {
                text.into_bytes()
            }
        }
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } if !composing => {
            if modifiers.ctrl && !modifiers.alt {
                let letter = match key {
                    Key::A => Some(1),
                    Key::B => Some(2),
                    Key::C => Some(3),
                    Key::D => Some(4),
                    Key::E => Some(5),
                    Key::F => Some(6),
                    Key::G => Some(7),
                    Key::H => Some(8),
                    Key::I => Some(9),
                    Key::J => Some(10),
                    Key::K => Some(11),
                    Key::L => Some(12),
                    Key::M => Some(13),
                    Key::N => Some(14),
                    Key::O => Some(15),
                    Key::P => Some(16),
                    Key::Q => Some(17),
                    Key::R => Some(18),
                    Key::S => Some(19),
                    Key::T => Some(20),
                    Key::U => Some(21),
                    Key::V => None,
                    Key::W => Some(23),
                    Key::X => Some(24),
                    Key::Y => Some(25),
                    Key::Z => Some(26),
                    _ => None,
                };
                if let Some(letter) = letter {
                    return Some(vec![letter]);
                }
            }
            let sequence = match key {
                Key::Enter if modifiers.shift => "\x1b[13;2u",
                Key::Enter => "\r",
                Key::Backspace => "\x7f",
                Key::Escape => "\x1b",
                Key::Tab if modifiers.shift => "\x1b[Z",
                Key::Tab => "\t",
                Key::ArrowUp if application => "\x1bOA",
                Key::ArrowDown if application => "\x1bOB",
                Key::ArrowRight if application => "\x1bOC",
                Key::ArrowLeft if application => "\x1bOD",
                Key::ArrowUp => "\x1b[A",
                Key::ArrowDown => "\x1b[B",
                Key::ArrowRight => "\x1b[C",
                Key::ArrowLeft => "\x1b[D",
                Key::Home => "\x1b[H",
                Key::End => "\x1b[F",
                Key::Delete => "\x1b[3~",
                Key::Insert => "\x1b[2~",
                Key::PageUp => "\x1b[5~",
                Key::PageDown => "\x1b[6~",
                _ => return None,
            };
            sequence.as_bytes().to_vec()
        }
        _ => return None,
    };
    if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
        return None;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paste_is_literal_bounded_and_terminal_keys_stay_terminal_keys() {
        let paste = encode_event(
            &egui::Event::Paste("$(echo x)\n東京\x1b[201~".into()),
            false,
            true,
            false,
        )
        .unwrap();
        assert!(paste.starts_with(b"\x1b[200~"));
        assert!(paste.ends_with(b"\x1b[201~"));
        assert_eq!(
            paste
                .windows(6)
                .filter(|part| *part == b"\x1b[201~")
                .count(),
            1
        );
        let key = egui::Event::Key {
            key: Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        };
        assert_eq!(encode_event(&key, false, false, false), Some(vec![3]));
        assert_eq!(
            encode_event(&egui::Event::Text("a".into()), false, false, true),
            None
        );
    }
}
