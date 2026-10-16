//! Owned PTY process lifecycle for the first-party host binary.
use crate::{
    model::TerminalModel,
    protocol::{self, QUEUE_CAPACITY, Result, TerminalError, VERSION},
    wire::{Command, Notice, Open, Status, command, notice},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command as OsCommand, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use vibemux_platform::ProcessTree;

const READY_MARKER: &[u8] = b"VIBEMUX_PTY_READY";
const ENVIRONMENT_NAMES: &[&str] = &[
    "PATH",
    "SystemRoot",
    "WINDIR",
    "USERPROFILE",
    "HOME",
    "APPDATA",
    "LOCALAPPDATA",
    "TEMP",
    "TMP",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CONFIG_DIR",
];

fn process_exited(child: &mut dyn portable_pty::Child, tree: &ProcessTree) -> Result<bool> {
    #[cfg(windows)]
    {
        let _ = tree;
        child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|_| TerminalError("terminal_wait_failed"))
    }
    #[cfg(unix)]
    {
        let _ = child;
        // Preserve the group identity until cleanup has signalled its members.
        tree.leader_exited()
            .map_err(|_| TerminalError("terminal_wait_failed"))
    }
}

pub fn status(phase: &str, code: &str, worktree: &Path, branch: &str, process_id: u32) -> Notice {
    Notice {
        version: VERSION,
        payload: Some(notice::Payload::Status(Status {
            phase: phase.into(),
            code: code.into(),
            worktree: worktree.to_string_lossy().into_owned(),
            branch: branch.into(),
            process_id,
        })),
    }
}

fn vendor_path(open: &Open) -> Result<PathBuf> {
    let path = std::fs::canonicalize(&open.executable)
        .map_err(|_| TerminalError("terminal_executable_missing"))?;
    if !Path::new(&open.executable).is_absolute() || !path.is_file() {
        return Err(TerminalError("terminal_executable_invalid"));
    }
    if open.fixture {
        let own =
            std::env::current_exe().map_err(|_| TerminalError("terminal_host_unavailable"))?;
        let expected = own.with_file_name(if cfg!(windows) {
            "vibemux_terminal_fixture.exe"
        } else {
            "vibemux_terminal_fixture"
        });
        if std::fs::canonicalize(expected).ok().as_ref() != Some(&path) {
            return Err(TerminalError("terminal_fixture_invalid"));
        }
    } else {
        if Path::new(&open.executable)
            .file_stem()
            .and_then(|value| value.to_str())
            != Some("claude")
        {
            return Err(TerminalError("terminal_executable_invalid"));
        }
        let root = std::fs::canonicalize(&open.project_root)
            .map_err(|_| TerminalError("terminal_directory_unavailable"))?;
        if crate::workspace::is_within(&root, &path) {
            return Err(TerminalError("terminal_executable_invalid"));
        }
        #[cfg(windows)]
        if path.extension().and_then(|value| value.to_str()) != Some("exe") {
            return Err(TerminalError("terminal_executable_invalid"));
        }
    }
    Ok(path)
}

pub fn run_host() -> Result<()> {
    let mut input = std::io::stdin();
    let first = protocol::read_message::<Command>(&mut input)?
        .ok_or(TerminalError("terminal_open_required"))?;
    protocol::validate_command(&first)?;
    let Some(command::Operation::Open(open)) = first.operation else {
        return Err(TerminalError("terminal_open_required"));
    };
    let vendor = vendor_path(&open)?;
    let root = crate::workspace::checked_directory(Path::new(&open.project_root))?;
    let mut output = std::io::stdout();
    protocol::write_message(&mut output, &status("starting", "", Path::new(""), "", 0))?;
    let receipt = if open.fixture {
        None
    } else {
        Some(crate::workspace::create(
            &root,
            Path::new(&open.git_executable),
        )?)
    };
    if receipt
        .as_ref()
        .is_some_and(|receipt| crate::workspace::is_within(&receipt.project_root, &vendor))
    {
        return Err(TerminalError("terminal_executable_invalid"));
    }
    let cwd = receipt
        .as_ref()
        .map_or(root.as_path(), |receipt| receipt.worktree.as_path());
    let branch = receipt
        .as_ref()
        .map_or("", |receipt| receipt.branch.as_str());
    let size = open
        .size
        .ok_or(TerminalError("terminal_dimensions_invalid"))?;
    let pty_size = |size: crate::wire::Size| PtySize {
        rows: size.rows as u16,
        cols: size.columns as u16,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = native_pty_system()
        .openpty(pty_size(size))
        .map_err(|_| TerminalError("terminal_pty_open_failed"))?;
    let own_executable =
        std::env::current_exe().map_err(|_| TerminalError("terminal_host_unavailable"))?;
    let mut launch = CommandBuilder::new(own_executable);
    launch.arg("--pty_child");
    launch.arg(&vendor);
    launch.cwd(cwd);
    launch.env_clear();
    for name in ENVIRONMENT_NAMES {
        if let Some(value) = std::env::var_os(name) {
            launch.env(name, value);
        }
    }
    launch.env("TERM", "xterm-256color");
    launch.env("COLORTERM", "truecolor");
    let mut child = pair
        .slave
        .spawn_command(launch)
        .map_err(|_| TerminalError("terminal_pty_spawn_failed"))?;
    drop(pair.slave);
    let process_id = child
        .process_id()
        .ok_or(TerminalError("terminal_process_identity_missing"))?;
    let mut tree = match ProcessTree::contain(process_id) {
        Ok(tree) => tree,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TerminalError("terminal_containment_failed"));
        }
    };
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|_| TerminalError("terminal_pty_write_failed"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| TerminalError("terminal_pty_read_failed"))?;
    let (data_tx, data_rx) = mpsc::sync_channel::<Vec<u8>>(QUEUE_CAPACITY);
    let overflow = Arc::new(AtomicBool::new(false));
    let reader_overflow = Arc::clone(&overflow);
    let read_worker = std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        while let Ok(length) = reader.read(&mut buffer) {
            if length == 0 {
                break;
            }
            if data_tx.try_send(buffer[..length].to_vec()).is_err() {
                reader_overflow.store(true, Ordering::Release);
            }
        }
    });
    // The trampoline enables raw input before advertising readiness, so the go
    // byte cannot be swallowed by cooked console line editing.
    let ready_deadline = Instant::now() + Duration::from_secs(8);
    let mut greeting = Vec::new();
    let mut startup_model = TerminalModel::new(size)?;
    let ready = loop {
        if Instant::now() >= ready_deadline {
            break false;
        }
        if let Ok(bytes) = data_rx.recv_timeout(Duration::from_millis(20)) {
            startup_model.process(&bytes)?;
            // ConPTY's INHERIT_CURSOR handshake precedes child console I/O.
            // Answer its cursor report request before waiting for the gate.
            for response in startup_model.responses() {
                writer
                    .write_all(&response)
                    .and_then(|_| writer.flush())
                    .map_err(|_| TerminalError("terminal_startup_reply_failed"))?;
            }
            greeting.extend_from_slice(&bytes);
            if greeting
                .windows(READY_MARKER.len())
                .any(|window| window == READY_MARKER)
            {
                break true;
            }
            if greeting.len() > 8192 {
                break false;
            }
        }
        if process_exited(child.as_mut(), &tree)? {
            break false;
        }
    };
    let result = if !ready {
        Err(TerminalError("terminal_trampoline_timeout"))
    } else {
        writer
            .write_all(&[1])
            .and_then(|_| writer.flush())
            .map_err(|_| TerminalError("terminal_release_failed"))?;
        protocol::write_message(
            &mut output,
            &status(
                if open.fixture { "fixture" } else { "running" },
                "",
                cwd,
                branch,
                process_id,
            ),
        )?;
        let (command_tx, command_rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        // This pipe reader belongs to the helper process. GUI shutdown closes
        // the pipe; a malicious client retaining it is bounded by host exit.
        let pipe_worker = std::thread::spawn(move || {
            loop {
                let result = protocol::read_message::<Command>(&mut input);
                let terminal = result.as_ref().map_or(true, |value| value.is_none());
                if command_tx.send(result).is_err() || terminal {
                    break;
                }
            }
        });
        let mut model = TerminalModel::new(size)?;
        let mut dirty = true;
        let mut last_frame = Instant::now() - Duration::from_secs(1);
        let loop_result = (|| -> Result<()> {
            loop {
                if overflow.load(Ordering::Acquire) {
                    return Err(TerminalError("terminal_output_queue_full"));
                }
                for bytes in data_rx.try_iter() {
                    model.process(&bytes)?;
                    dirty = true;
                }
                for response in model.responses() {
                    writer
                        .write_all(&response)
                        .map_err(|_| TerminalError("terminal_pty_write_failed"))?;
                }
                match command_rx.try_recv() {
                    Ok(Ok(Some(message))) => {
                        protocol::validate_command(&message)?;
                        match message.operation {
                            Some(command::Operation::Input(bytes)) => {
                                writer
                                    .write_all(&bytes)
                                    .and_then(|_| writer.flush())
                                    .map_err(|_| TerminalError("terminal_pty_write_failed"))?;
                            }
                            Some(command::Operation::Resize(size)) => {
                                pair.master
                                    .resize(pty_size(size))
                                    .map_err(|_| TerminalError("terminal_resize_failed"))?;
                                model.resize(size)?;
                                dirty = true;
                            }
                            Some(command::Operation::Scroll(lines)) => {
                                model.scroll(lines);
                                dirty = true;
                            }
                            Some(command::Operation::Close(true)) => break,
                            _ => return Err(TerminalError("terminal_command_invalid")),
                        }
                    }
                    Ok(Ok(None)) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Ok(Err(error)) => return Err(error),
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                if dirty && last_frame.elapsed() >= Duration::from_millis(33) {
                    protocol::write_message(
                        &mut output,
                        &Notice {
                            version: VERSION,
                            payload: Some(notice::Payload::Screen(model.screen())),
                        },
                    )?;
                    last_frame = Instant::now();
                    dirty = false;
                }
                if process_exited(child.as_mut(), &tree)? {
                    break;
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            Ok(())
        })();
        if pipe_worker.is_finished() {
            let _ = pipe_worker.join();
        }
        loop_result
    };
    let _ = tree.terminate();
    let _ = child.wait();
    drop(writer);
    drop(pair.master);
    let _ = read_worker.join();
    let (phase, code) = result
        .as_ref()
        .map_or(("failed", "terminal_session_failed"), |_| ("exited", ""));
    let code = result.as_ref().err().map_or(code, |error| error.0);
    let _ = protocol::write_message(&mut output, &status(phase, code, cwd, branch, process_id));
    result
}

pub fn run_pty_child(executable: &Path) -> i32 {
    if !executable.is_absolute() || crossterm::terminal::enable_raw_mode().is_err() {
        return 121;
    }
    let mut output = std::io::stdout();
    if output
        .write_all(READY_MARKER)
        .and_then(|_| output.flush())
        .is_err()
    {
        return 122;
    }
    #[cfg(windows)]
    let input = {
        use std::os::windows::io::AsHandle;
        std::io::stdin()
            .as_handle()
            .try_clone_to_owned()
            .map(std::fs::File::from)
    };
    #[cfg(unix)]
    let input = {
        use std::os::fd::AsFd;
        std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map(std::fs::File::from)
    };
    let Ok(mut input) = input else {
        return 122;
    };
    let mut go = [0_u8; 1];
    if input.read_exact(&mut go).is_err() || go != [1] {
        return 122;
    }
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = output.write_all(b"\x1b[2J\x1b[H");
    let _ = output.flush();
    let Ok(mut child) = OsCommand::new(executable)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    else {
        return 123;
    };
    child
        .wait()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(124)
}
