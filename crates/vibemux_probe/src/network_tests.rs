//! Owned loopback peers test the public probe without any installed launcher.
use super::*;
use tokio::{io::AsyncWriteExt, net::TcpListener, task::JoinHandle};

const PEER_DEADLINE: Duration = Duration::from_secs(3);
const MAX_REQUEST_BYTES: usize = 4096;

async fn health_peer(response: String) -> (u16, JoinHandle<std::io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("owned health listener");
    let port = listener.local_addr().expect("listener address").port();
    let worker = tokio::spawn(async move {
        timeout(PEER_DEADLINE, async {
            // The first connection is the TCP reachability observation. It
            // closes without HTTP; the second carries the actual health read.
            loop {
                let (mut stream, _) = listener.accept().await?;
                let mut request = Vec::new();
                while request.len() < MAX_REQUEST_BYTES {
                    let mut buffer = [0; 512];
                    let count = stream.read(&mut buffer).await?;
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        break;
                    }
                }
                if request.is_empty() {
                    continue;
                }
                if !request.starts_with(b"GET /health HTTP/1.1\r\n") {
                    return Err(std::io::Error::other("unexpected owned health request"));
                }
                stream.write_all(response.as_bytes()).await?;
                return Ok(());
            }
        })
        .await
        .map_err(|_| std::io::Error::other("owned health peer deadline"))?
    });
    (port, worker)
}

fn isolated_config(home: &Path, port: u16) -> ProbeConfig {
    ProbeConfig {
        home_dir: home.to_owned(),
        path_entries: Vec::new(),
        cc_switch_port: port,
        command_timeout: Duration::from_secs(1),
        network_timeout: Duration::from_secs(1),
        telemetry_window_seconds: 60,
        run_a2a_self_test: false,
    }
}

#[tokio::test]
async fn loopback_health_redirect_never_contacts_its_target_or_verifies_gateway() {
    let home = tempfile::tempdir().expect("isolated home");
    let target = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("owned redirect target");
    let address = target.local_addr().expect("target address");
    let target_worker = tokio::spawn(async move {
        match timeout(PEER_DEADLINE, target.accept()).await {
            Ok(Ok((mut stream, _))) => {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await
                    .expect("target reply");
                true
            }
            Ok(Err(_)) => panic!("owned target accept failed"),
            Err(_) => false,
        }
    });
    let (port, health) = health_peer(format!(
        "HTTP/1.1 302 Found\r\nLocation: http://{address}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )).await;
    let report = run_probe(&isolated_config(home.path(), port)).await;
    health.await.expect("health joined").expect("health served");
    let followed = target_worker.await.expect("target joined");
    assert!(
        !followed,
        "loopback health cannot authorize redirect traffic"
    );
    assert!(report.gateway.tcp_reachable);
    assert_eq!(report.gateway.health_status, Some(302));
    assert_eq!(report.gateway.state, ProbeState::Failed);
    assert_eq!(report.gateway.code, "gateway_health_failed");
    assert_eq!(report.a2a.state, ProbeState::NotRun);
    assert!(
        report
            .agents
            .iter()
            .all(|agent| agent.launcher_state == ProbeState::Unavailable
                && agent.authentication_state == ProbeState::NotRun
                && agent.inference_state == ProbeState::NotRun)
    );
}

#[tokio::test]
async fn loopback_health_reports_success_and_failure_from_the_original_response() {
    let home = tempfile::tempdir().expect("isolated home");
    for (status, expected) in [
        ("200 OK", ProbeState::Verified),
        ("204 No Content", ProbeState::Verified),
        ("403 Forbidden", ProbeState::Failed),
    ] {
        let (port, worker) = health_peer(format!(
            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ))
        .await;
        let report = run_probe(&isolated_config(home.path(), port)).await;
        worker.await.expect("health joined").expect("health served");
        assert!(report.gateway.tcp_reachable);
        assert_eq!(report.gateway.state, expected);
        assert_eq!(
            report.gateway.health_status,
            Some(status[..3].parse().expect("literal status"))
        );
    }
}
