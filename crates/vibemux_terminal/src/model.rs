//! Generic VT engine; no vendor UI reconstruction or task-state inference.
use crate::{
    protocol::{QUEUE_CAPACITY, Result, TerminalError, validate_size},
    wire::{Cell, Screen, Size},
};
use alacritty_terminal::{
    Term,
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::Column,
    term::{Config, Osc52, TermMode, cell::Flags},
    vte::ansi::{Color, Processor, Rgb},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{self, Receiver, SyncSender},
};

pub const DEFAULT_FOREGROUND: u32 = 0xedece5;
pub const DEFAULT_BACKGROUND: u32 = 0x1f1e1d;
const HISTORY_LINES: usize = 1000;
const MAX_OSC_BYTES: usize = 1024;
const MAX_COMBINING_CHARACTERS: usize = 8;
const ANSI_COLORS: [u32; 16] = [
    0x1f1e1d, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xdcdfe4, 0x5c6370,
    0xff7a85, 0xb3df8f, 0xffd68a, 0x85c5ff, 0xdf9aff, 0x76dce6, 0xffffff,
];

pub fn palette_color(index: usize) -> u32 {
    match index {
        0..=15 => ANSI_COLORS[index],
        16..=231 => {
            let value = index - 16;
            let channel = |part: usize| if part == 0 { 0 } else { 55 + part as u32 * 40 };
            (channel(value / 36) << 16) | (channel(value / 6 % 6) << 8) | channel(value % 6)
        }
        232..=255 => {
            let gray = 8 + (index as u32 - 232) * 10;
            gray * 0x010101
        }
        257 => DEFAULT_BACKGROUND,
        _ => DEFAULT_FOREGROUND,
    }
}

#[derive(Clone)]
struct Listener {
    responses: SyncSender<Vec<u8>>,
    dimensions: Arc<AtomicU32>,
    overflow: Arc<AtomicBool>,
}
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let response = match event {
            Event::PtyWrite(value) => Some(value),
            Event::ColorRequest(index, format) => {
                let color = palette_color(index);
                Some(format(Rgb {
                    r: (color >> 16) as u8,
                    g: (color >> 8) as u8,
                    b: color as u8,
                }))
            }
            Event::TextAreaSizeRequest(format) => {
                let size = self.dimensions.load(Ordering::Relaxed);
                Some(format(WindowSize {
                    num_cols: size as u16,
                    num_lines: (size >> 16) as u16,
                    cell_width: 8,
                    cell_height: 16,
                }))
            }
            // Terminal escape sequences never open URLs or access the clipboard.
            _ => None,
        };
        if let Some(response) = response {
            if response.len() > 4096 || self.responses.try_send(response.into_bytes()).is_err() {
                self.overflow.store(true, Ordering::Release);
            }
        }
    }
}

struct Geometry(Size);
impl Dimensions for Geometry {
    fn columns(&self) -> usize {
        self.0.columns as usize
    }
    fn screen_lines(&self) -> usize {
        self.0.rows as usize
    }
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
}

pub struct TerminalModel {
    terminal: Term<Listener>,
    parser: Processor,
    size: Size,
    responses: Receiver<Vec<u8>>,
    dimensions: Arc<AtomicU32>,
    overflow: Arc<AtomicBool>,
    osc_size: Option<usize>,
    escape_pending: bool,
}
impl TerminalModel {
    pub fn new(size: Size) -> Result<Self> {
        validate_size(&size)?;
        let (tx, responses) = mpsc::sync_channel(QUEUE_CAPACITY);
        let dimensions = Arc::new(AtomicU32::new((size.rows << 16) | size.columns));
        let overflow = Arc::new(AtomicBool::new(false));
        let listener = Listener {
            responses: tx,
            dimensions: Arc::clone(&dimensions),
            overflow: Arc::clone(&overflow),
        };
        let config = Config {
            scrolling_history: HISTORY_LINES,
            osc52: Osc52::Disabled,
            kitty_keyboard: false,
            ..Config::default()
        };
        Ok(Self {
            terminal: Term::new(config, &Geometry(size), listener),
            parser: Processor::default(),
            size,
            responses,
            dimensions,
            overflow,
            osc_size: None,
            escape_pending: false,
        })
    }
    pub fn process(&mut self, bytes: &[u8]) -> Result<()> {
        for byte in bytes {
            match self.osc_size {
                Some(_) if matches!(*byte, 0x07 | 0x18 | 0x1a | 0x1b) => {
                    self.osc_size = None;
                    self.escape_pending = *byte == 0x1b;
                }
                Some(length) => {
                    if length >= MAX_OSC_BYTES {
                        return Err(TerminalError("terminal_escape_limit"));
                    }
                    self.osc_size = Some(length + 1);
                }
                None if *byte == 0x1b => self.escape_pending = true,
                None if self.escape_pending && *byte == b']' => {
                    self.osc_size = Some(0);
                    self.escape_pending = false;
                }
                None if self.escape_pending
                    && matches!(*byte, 0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f) => {}
                None => self.escape_pending = false,
            }
            self.parser.advance(&mut self.terminal, *byte);
            // VT combining marks attach to the cursor cell or either preceding
            // column (including wide characters), even across input chunks.
            let grid = self.terminal.grid();
            let point = grid.cursor.point;
            for offset in 0..=2 {
                if let Some(column) = point.column.0.checked_sub(offset) {
                    if grid[point.line][Column(column)]
                        .zerowidth()
                        .is_some_and(|extra| extra.len() > MAX_COMBINING_CHARACTERS)
                    {
                        return Err(TerminalError("terminal_combining_limit"));
                    }
                }
            }
        }
        if self.overflow.load(Ordering::Acquire) {
            return Err(TerminalError("terminal_query_queue_full"));
        }
        Ok(())
    }
    pub fn resize(&mut self, size: Size) -> Result<()> {
        validate_size(&size)?;
        self.terminal.resize(Geometry(size));
        self.size = size;
        self.dimensions
            .store((size.rows << 16) | size.columns, Ordering::Relaxed);
        Ok(())
    }
    pub fn scroll(&mut self, lines: i32) {
        self.terminal.scroll_display(Scroll::Delta(lines));
    }
    pub fn responses(&self) -> Vec<Vec<u8>> {
        self.responses.try_iter().collect()
    }
    pub fn screen(&self) -> Screen {
        let content = self.terminal.renderable_content();
        let mut cells = vec![
            Cell {
                text: " ".into(),
                foreground: DEFAULT_FOREGROUND,
                background: DEFAULT_BACKGROUND,
                flags: 0
            };
            (self.size.rows * self.size.columns) as usize
        ];
        let resolve = |color: Color| match color {
            Color::Spec(rgb) => ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32,
            Color::Indexed(index) => palette_color(index as usize),
            Color::Named(name) => palette_color(name as usize),
        };
        let offset = content.display_offset;
        let mode = content.mode;
        for item in content.display_iter {
            let row = item.point.line.0 + offset as i32;
            let Ok(row) = usize::try_from(row) else {
                continue;
            };
            let column = item.point.column.0;
            let Some(target) = cells.get_mut(row * self.size.columns as usize + column) else {
                continue;
            };
            let mut fg = resolve(item.cell.fg);
            let mut bg = resolve(item.cell.bg);
            if item.cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut text = item.cell.c.to_string();
            if let Some(extra) = item.cell.zerowidth() {
                text.extend(extra.iter().take(MAX_COMBINING_CHARACTERS));
            }
            if item.cell.flags.contains(Flags::HIDDEN) {
                text = " ".into();
            }
            *target = Cell {
                text,
                foreground: fg,
                background: bg,
                flags: u32::from(
                    item.cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
                ) | (u32::from(item.cell.flags.contains(Flags::BOLD)) << 1)
                    | (u32::from(item.cell.flags.intersects(Flags::ALL_UNDERLINES)) << 2)
                    | (u32::from(item.cell.flags.contains(Flags::WIDE_CHAR)) << 3),
            };
        }
        let cursor = self.terminal.grid().cursor.point;
        Screen {
            size: Some(self.size),
            cells,
            cursor_row: (cursor.line.0.max(0) as u32).min(self.size.rows - 1),
            cursor_column: (cursor.column.0 as u32).min(self.size.columns - 1),
            cursor_visible: mode.contains(TermMode::SHOW_CURSOR) && offset == 0,
            application_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_vt_engine_handles_color_cursor_unicode_and_queries() {
        let mut model = TerminalModel::new(Size {
            columns: 40,
            rows: 8,
        })
        .unwrap();
        model
            .process("\x1b[2J\x1b[H\x1b[31mClaude\x1b[0m 東京\x1b[6n".as_bytes())
            .unwrap();
        let screen = model.screen();
        assert_eq!(screen.cells[0].text, "C");
        assert_eq!(screen.cells[0].foreground, ANSI_COLORS[1]);
        assert!(!model.responses().is_empty());
        crate::protocol::validate_screen(&screen).unwrap();
        model.process(b"\x1b[?1049hALT\x1b[?1049l").unwrap();
        assert_eq!(model.screen().cells[0].text, "C");
    }
    #[test]
    fn clipboard_requests_are_disabled_and_resizing_is_bounded() {
        let mut model = TerminalModel::new(Size {
            columns: 20,
            rows: 5,
        })
        .unwrap();
        model.process(b"\x1b]52;c;c2VjcmV0\x07").unwrap();
        assert!(model.responses().is_empty());
        model
            .resize(Size {
                columns: 30,
                rows: 10,
            })
            .unwrap();
        assert_eq!(model.screen().cells.len(), 300);
    }

    #[test]
    fn terminal_query_pressure_fails_without_unbounded_responses() {
        let mut model = TerminalModel::new(Size {
            columns: 20,
            rows: 5,
        })
        .unwrap();
        assert_eq!(
            model.process(&b"\x1b[6n".repeat(QUEUE_CAPACITY + 1)),
            Err(TerminalError("terminal_query_queue_full"))
        );
        assert_eq!(model.responses().len(), QUEUE_CAPACITY);
    }

    #[test]
    fn unfinished_escape_and_combining_growth_are_bounded_across_chunks() {
        let size = Size {
            columns: 20,
            rows: 5,
        };
        let mut model = TerminalModel::new(size).unwrap();
        model.process(b"\x1b").unwrap();
        model.process(b"]0;").unwrap();
        model.process(&vec![b'x'; MAX_OSC_BYTES - 2]).unwrap();
        assert_eq!(
            model.process(b"x"),
            Err(TerminalError("terminal_escape_limit"))
        );
        let mut model = TerminalModel::new(size).unwrap();
        model.process("東".as_bytes()).unwrap();
        for _ in 0..MAX_COMBINING_CHARACTERS {
            model.process("\u{301}".as_bytes()).unwrap();
        }
        assert_eq!(
            model.process("\u{301}".as_bytes()),
            Err(TerminalError("terminal_combining_limit"))
        );
    }
}
