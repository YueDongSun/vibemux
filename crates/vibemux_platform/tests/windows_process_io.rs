#![cfg(windows)]
use std::{
    io::Write,
    process::Stdio,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command, runtime::Builder};
use vibemux_platform::process_stdio::spawn_with_async_output;

const WRITER: &str = "process.stdout.write('ready');process.stderr.write('ready');process.stdin.once('data',()=>{let count=0;for(const stream of [process.stdout,process.stderr]){stream.on('error',()=>{});stream.write('probe',error=>{if(!error)process.exit(7);if(++count===2)process.exit(0)});}});setInterval(()=>{},1000);";

fn cancellation_case(queue_blocking_work: bool) {
    let node = std::env::var_os("VIBEMUX_DEV_TEST_NODE")
        .expect("select installed Node for the fixed pipe fixture");
    let runtime = Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let mut spawned = runtime.block_on(async {
        let mut command = Command::new(node);
        command
            .args(["-e", WRITER])
            .stdin(Stdio::piped())
            .creation_flags(0x0800_0000)
            .env_clear();
        for name in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        spawn_with_async_output(&mut command).await.unwrap()
    });
    let mut output = spawned.output;
    let mut input = std::fs::File::from(
        spawned
            .child
            .stdin
            .take()
            .unwrap()
            .into_owned_handle()
            .unwrap(),
    );
    let ready = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut a = [0; 5];
            let mut b = [0; 5];
            output.stdout.read_exact(&mut a).await?;
            output.stderr.read_exact(&mut b).await?;
            Ok::<_, std::io::Error>(a == *b"ready" && b == *b"ready")
        })
        .await
    });
    let mut release = None;
    let mut blocker = None;
    if queue_blocking_work {
        let (sender, receiver) = mpsc::channel();
        let (entered, observed) = mpsc::channel();
        blocker = Some(runtime.spawn_blocking(move || {
            entered.send(()).unwrap();
            let _ = receiver.recv();
        }));
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        release = Some(sender);
    }
    let pending = runtime.block_on(async {
        let mut a = [0; 1];
        let mut b = [0; 1];
        tokio::time::timeout(Duration::from_millis(100), async {
            tokio::join!(output.stdout.read(&mut a), output.stderr.read(&mut b))
        })
        .await
        .is_err()
    });
    drop(output);
    if let Some(release) = release {
        release.send(()).unwrap();
    }
    if let Some(blocker) = blocker {
        runtime.block_on(blocker).unwrap();
    }
    let (finished, observed) = mpsc::channel();
    let owner = thread::spawn(move || {
        drop(runtime);
        let _ = finished.send(());
    });
    let settled_while_writer_open = observed.recv_timeout(Duration::from_secs(1)).is_ok();
    let writer_was_alive = spawned.child.try_wait().unwrap().is_none();
    let mut disconnected = false;
    if settled_while_writer_open && writer_was_alive {
        input.write_all(b"check").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(status) = spawned.child.try_wait().unwrap() {
                disconnected = status.success();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    // The independent owner always stops/reaps the fixed writer before any
    // assertion, including the deliberately failing old blocking-pipe case.
    let _ = spawned.child.start_kill();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut reaped = false;
    while Instant::now() < deadline {
        if spawned.child.try_wait().unwrap().is_some() {
            reaped = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !settled_while_writer_open {
        observed.recv_timeout(Duration::from_secs(3)).unwrap();
    }
    owner.join().unwrap();
    assert!(matches!(ready, Ok(Ok(true))));
    assert!(pending && writer_was_alive && reaped);
    assert!(
        settled_while_writer_open,
        "capture cancellation left runtime shutdown waiting for the live writer"
    );
    assert!(
        disconnected,
        "both read endpoints must close while the fixed writer remains owned"
    );
}

#[test]
#[ignore = "requires explicitly selected Node; owns a fixed pipe-writer fixture"]
fn pending_output_reads_do_not_hold_runtime_shutdown() {
    cancellation_case(false);
}

#[test]
#[ignore = "requires explicitly selected Node; owns a fixed pipe-writer fixture"]
fn queued_read_cancellation_does_not_start_a_late_blocking_read() {
    cancellation_case(true);
}

#[test]
#[ignore = "requires explicitly selected Node; owns a fixed output fixture"]
fn normal_output_and_eof_are_preserved() {
    let node = std::env::var_os("VIBEMUX_DEV_TEST_NODE").expect("select fixed Node fixture");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let mut command = Command::new(node);
        command
            .args([
                "-e",
                "process.stdout.write('out');process.stderr.write('err');",
            ])
            .stdin(Stdio::null())
            .creation_flags(0x0800_0000);
        let mut spawned = spawn_with_async_output(&mut command).await.unwrap();
        let mut output = spawned.output;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let captured = tokio::time::timeout(Duration::from_secs(3), async {
            let (a, b) = tokio::join!(
                output.stdout.read_to_end(&mut stdout),
                output.stderr.read_to_end(&mut stderr)
            );
            a?;
            b?;
            Ok::<_, std::io::Error>(())
        })
        .await;
        if !matches!(captured, Ok(Ok(()))) {
            let _ = spawned.child.start_kill();
        }
        let reaped = tokio::time::timeout(Duration::from_secs(3), spawned.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(captured, Ok(Ok(()))) && reaped.success());
        assert_eq!(stdout, b"out");
        assert_eq!(stderr, b"err");
    });
}
