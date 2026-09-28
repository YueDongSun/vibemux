//! Screenshot-only synthetic UI runner. Never contacts a daemon or harness.
use vibemux_frontend::{
    gui, preview,
    theme::{
        ThemeId,
        serialize::{UserConfig, WindowSize},
    },
};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario = args.first().map(String::as_str).unwrap_or("chat");
    let theme = args
        .get(1)
        .and_then(|name| {
            ThemeId::ALL
                .into_iter()
                .find(|theme| theme.as_str() == name)
        })
        .unwrap_or_default();
    let width = args
        .get(2)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1280)
        .clamp(720, 3840);
    let height = args
        .get(3)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(800)
        .clamp(520, 2160);
    let scale = args
        .get(4)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(1.0)
        .clamp(1.0, 2.0);
    let snapshot = preview::supervisor_snapshot(scenario == "disconnected");
    let detached = scenario == "task" || scenario == "task_terminal";
    let terminal_tab = scenario == "task_terminal";
    let drawer = scenario == "drawer";
    let screenshot_path = args.get(5).cloned();
    let config = UserConfig {
        theme,
        window_size: WindowSize { width, height },
        ..Default::default()
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([width as f32, height as f32])
            .with_title("VibeMux · DEMO preview"),
        ..Default::default()
    };
    eframe::run_native(
        "VibeMux · DEMO preview",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_pixels_per_point(scale);
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    width as f32,
                    height as f32,
                )));
            let mut app = gui::VibeMuxApp::new(cc, preview::probe_view(), config);
            app.set_supervisor_snapshot(snapshot);
            match scenario {
                "welcome" => app.request_new_task(),
                "collapsed" => app.set_sidebar_collapsed(true),
                _ => {}
            }
            if drawer {
                app.select_task("task_ui");
            }
            if detached {
                app.open_task_window("task_ui");
                if terminal_tab {
                    app.set_task_window_tab("task_ui", gui::TaskDetailTab::Terminal);
                }
            }
            if let Some(path) = screenshot_path {
                let target = if detached {
                    egui::ViewportId::from_hash_of(("vibemux_task", "task_ui"))
                } else {
                    egui::ViewportId::ROOT
                };
                install_capture(&cc.egui_ctx, target, path);
            }
            Ok(Box::new(app))
        }),
    )
}

fn install_capture(context: &egui::Context, target: egui::ViewportId, path: String) {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };
    let started = Instant::now();
    let requested = Arc::new(AtomicBool::new(false));
    context.on_end_pass(
        "preview_capture",
        Arc::new(move |context| {
            let captured = context.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Screenshot {
                        viewport_id, image, ..
                    } if *viewport_id == target => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(captured) = captured {
                let bytes: Vec<u8> = captured
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_srgba_unmultiplied())
                    .collect();
                let result = image::save_buffer(
                    &path,
                    &bytes,
                    captured.width() as u32,
                    captured.height() as u32,
                    image::ColorType::Rgba8,
                );
                if result.is_err() {
                    eprintln!("preview_screenshot_failed");
                    std::process::exit(4);
                }
                std::process::exit(0);
            }
            if started.elapsed() > Duration::from_secs(10) {
                eprintln!("preview_screenshot_timeout");
                std::process::exit(4);
            }
            if context.viewport_id() == target
                && started.elapsed() > Duration::from_millis(600)
                && !requested.swap(true, Ordering::SeqCst)
            {
                context.send_viewport_cmd_to(
                    target,
                    egui::ViewportCommand::Screenshot(Default::default()),
                );
            }
            context.request_repaint_after(Duration::from_millis(50));
        }),
    );
}
