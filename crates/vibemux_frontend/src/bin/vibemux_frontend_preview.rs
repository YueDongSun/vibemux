//! Screenshot-only synthetic UI runner. Never contacts a daemon or harness.
//!
//! Arguments, all positional and optional:
//! `[scenario [theme [width [height [scale [screenshot_path]]]]]]`. An
//! unknown scene or theme, a value that is not a number, or an extra
//! argument exits with code 4, so a typo cannot save a screenshot of a
//! different scene under the requested name.
use std::{ffi::OsString, path::PathBuf, str::FromStr};

use vibemux_frontend::{
    gui, preview,
    theme::{
        ThemeId,
        serialize::{UserConfig, WindowSize},
    },
};

/// The scenes this runner can draw.
const SCENARIOS: [&str; 7] = [
    "chat",
    "drawer",
    "task",
    "task_terminal",
    "disconnected",
    "welcome",
    "collapsed",
];

/// Exit code for a usage error, matching the other frontend binaries.
const USAGE_EXIT: i32 = 4;

#[derive(Debug, PartialEq)]
struct PreviewArguments {
    scenario: &'static str,
    theme: ThemeId,
    width: u32,
    height: u32,
    scale: f32,
    screenshot_path: Option<PathBuf>,
}

/// Parse the positional arguments after the program name. Missing trailing
/// arguments take their defaults, and sizes and the scale are clamped into
/// the range the runner supports. The screenshot path may be any operating
/// system string; every other argument must be valid Unicode.
fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<PreviewArguments, String> {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    if arguments.len() > 6 {
        return Err(format!(
            "expected at most 6 arguments: \
             [scenario [theme [width [height [scale [screenshot_path]]]]]], got {}",
            arguments.len()
        ));
    }
    let text = |index: usize, name: &str| -> Result<Option<&str>, String> {
        arguments
            .get(index)
            .map(|value| {
                value
                    .to_str()
                    .ok_or_else(|| format!("the {name} is not valid Unicode"))
            })
            .transpose()
    };
    let scenario = match text(0, "scenario")? {
        None => SCENARIOS[0],
        Some(name) => SCENARIOS
            .into_iter()
            .find(|scenario| *scenario == name)
            .ok_or_else(|| {
                format!(
                    "unknown scenario {name} (expected one of: {})",
                    SCENARIOS.join(", ")
                )
            })?,
    };
    let theme = match text(1, "theme")? {
        None => ThemeId::default(),
        Some(name) => ThemeId::ALL
            .into_iter()
            .find(|theme| theme.as_str() == name)
            .ok_or_else(|| {
                let names: Vec<&str> = ThemeId::ALL.iter().map(|theme| theme.as_str()).collect();
                format!(
                    "unknown theme {name} (expected one of: {})",
                    names.join(", ")
                )
            })?,
    };
    let width = number(text(2, "width")?, "width", 1280_u32)?.clamp(720, 3840);
    let height = number(text(3, "height")?, "height", 800_u32)?.clamp(520, 2160);
    let scale = number(text(4, "scale")?, "scale", 1.0_f32)?;
    if !scale.is_finite() {
        return Err(format!("the scale must be finite, got {scale}"));
    }
    Ok(PreviewArguments {
        scenario,
        theme,
        width,
        height,
        scale: scale.clamp(1.0, 2.0),
        screenshot_path: arguments.get(5).map(PathBuf::from),
    })
}

/// `default` when the argument is absent; otherwise the parsed number.
fn number<T: FromStr>(value: Option<&str>, name: &str, default: T) -> Result<T, String> {
    value.map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| format!("the {name} must be a number, got {value}"))
    })
}

fn main() -> eframe::Result<()> {
    let PreviewArguments {
        scenario,
        theme,
        width,
        height,
        scale,
        screenshot_path,
    } = match parse_arguments(std::env::args_os().skip(1)) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(USAGE_EXIT);
        }
    };
    let snapshot = preview::supervisor_snapshot(scenario == "disconnected");
    let detached = scenario == "task" || scenario == "task_terminal";
    let terminal_tab = scenario == "task_terminal";
    let drawer = scenario == "drawer";
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

fn install_capture(context: &egui::Context, target: egui::ViewportId, path: PathBuf) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn non_unicode() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0x61, 0xD800])
    }

    #[cfg(unix)]
    fn non_unicode() -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![b'a', 0xff])
    }

    fn parse(arguments: &[&str]) -> Result<PreviewArguments, String> {
        parse_arguments(arguments.iter().map(OsString::from))
    }

    #[test]
    fn missing_arguments_take_the_defaults() {
        assert_eq!(
            parse(&[]),
            Ok(PreviewArguments {
                scenario: "chat",
                theme: ThemeId::ClaudeLight,
                width: 1280,
                height: 800,
                scale: 1.0,
                screenshot_path: None,
            })
        );
    }

    #[test]
    fn every_scene_and_theme_parses() {
        // The README example.
        let parsed = parse(&["chat", "claude_light", "1440", "900", "1"]).expect("example");
        assert_eq!((parsed.width, parsed.height), (1440, 900));
        for scenario in SCENARIOS {
            assert_eq!(parse(&[scenario]).expect(scenario).scenario, scenario);
        }
        for theme in ThemeId::ALL {
            assert_eq!(
                parse(&["chat", theme.as_str()]).expect("theme").theme,
                theme
            );
        }
    }

    #[test]
    fn sizes_and_the_scale_are_clamped() {
        let parsed = parse(&["chat", "nord", "100", "99999", "3"]).expect("clamped");
        assert_eq!((parsed.width, parsed.height), (720, 2160));
        assert_eq!(parsed.scale, 2.0);
        assert_eq!(
            parse(&["chat", "nord", "1280", "800", "0.5"])
                .expect("low")
                .scale,
            1.0
        );
    }

    #[test]
    fn an_unknown_scene_or_theme_is_refused() {
        // `switcher` is planned, not implemented; it must not fall back to `chat`.
        let scene = parse(&["switcher"]).expect_err("scene refused");
        assert!(scene.starts_with("unknown scenario switcher"), "{scene}");
        let theme = parse(&["chat", "solarized"]).expect_err("theme refused");
        assert!(theme.starts_with("unknown theme solarized"), "{theme}");
    }

    #[test]
    fn a_malformed_number_or_extra_argument_is_refused() {
        for arguments in [
            &["chat", "nord", "wide"][..],
            &["chat", "nord", "1280", "tall"],
            &["chat", "nord", "1280", "800", "big"],
            &["chat", "nord", "1280", "800", "NaN"],
            &["chat", "nord", "1280", "800", "inf"],
            &["chat", "nord", "1280", "800", "1", "out.png", "extra"],
        ] {
            assert!(parse(arguments).is_err(), "{arguments:?}");
        }
    }

    #[test]
    fn the_screenshot_path_may_be_any_operating_system_string() {
        let mut arguments: Vec<OsString> = ["chat", "nord", "1280", "800", "1"]
            .into_iter()
            .map(OsString::from)
            .collect();
        arguments.push(non_unicode());
        let parsed = parse_arguments(arguments).expect("path");
        assert_eq!(
            parsed
                .screenshot_path
                .as_deref()
                .map(|path| path.as_os_str()),
            Some(non_unicode().as_os_str())
        );
        let scene = parse_arguments([non_unicode()]).expect_err("scene refused");
        assert_eq!(scene, "the scenario is not valid Unicode");
    }
}
