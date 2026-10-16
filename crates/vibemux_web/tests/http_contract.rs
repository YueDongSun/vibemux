#![forbid(unsafe_code)]
use std::{sync::Arc, time::Duration};
use tokio::sync::{RwLock, oneshot};
use vibemux_web::{
    control_bridge::{BridgePoller, BridgeSnapshot},
    private_state::InstanceLease,
    push::PushService,
    server::{API_ROUTES, CSP, OPENAPI, build_router, mime_type},
};
const TEST_TOKEN: &str = "test_token_abcdefghijklmnopqrstuvwxyz_0123456789";

async fn fixture(
    snapshot: BridgeSnapshot,
) -> (
    tempfile::TempDir,
    InstanceLease,
    String,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let temp = tempfile::tempdir().unwrap();
    let lease = InstanceLease::acquire(temp.path()).unwrap();
    let push = Arc::new(
        PushService::startup(
            &lease.state_dir,
            vec!["fcm.googleapis.com".into()],
            "mailto:operator@example.com".into(),
            TEST_TOKEN,
        )
        .unwrap(),
    );
    let router = build_router(
        Arc::new(RwLock::new(snapshot)),
        push,
        TEST_TOKEN.to_owned(),
        "https://operator.example/".into(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, receive) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = receive.await;
            })
            .await
    });
    (temp, lease, url, stop, task)
}

#[tokio::test]
async fn every_route_and_fallback_rejects_anonymous_requests_with_security_headers() {
    let (_temp, _lease, url, stop, task) = fixture(BridgeSnapshot::default()).await;
    let client = reqwest::Client::new();
    let mut routes = API_ROUTES.to_vec();
    routes.extend([
        ("get", "/"),
        ("get", "/sw.js"),
        ("get", "/manifest.webmanifest"),
        ("get", "/openapi.json"),
        ("get", "/unknown_asset.js"),
        ("delete", "/api/v1/tasks"),
    ]);
    for (method, path) in routes {
        let path = path.replace("{task_id}", "00000000-0000-0000-0000-000000000001");
        let response = client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
                format!("{url}{path}"),
            )
            .send()
            .await
            .unwrap();
        assert!(
            matches!(response.status().as_u16(), 401 | 429),
            "{path}: {}",
            response.status()
        );
        assert_eq!(response.headers()["content-security-policy"], CSP);
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn wrong_and_duplicate_bearers_fail_and_auth_attempts_are_bounded() {
    let (_temp, _lease, url, stop, task) = fixture(BridgeSnapshot::default()).await;
    let client = reqwest::Client::new();
    for value in ["Bearer wrong", "Basic ignored", "bearer wrong"] {
        assert_eq!(
            client
                .get(format!("{url}/api/v1/health"))
                .header("authorization", value)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let duplicate = client
        .get(format!("{url}/api/v1/health"))
        .header("authorization", format!("Bearer {TEST_TOKEN}"))
        .header("authorization", format!("Bearer {TEST_TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 401);
    for _ in 0..4 {
        assert_eq!(
            client
                .get(format!("{url}/api/v1/health"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let limited = client
        .get(format!("{url}/api/v1/health"))
        .bearer_auth(TEST_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), 429);
    assert_eq!(limited.headers()["content-security-policy"], CSP);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn static_assets_openapi_and_spa_obey_etags_mime_and_bearer_auth() {
    let (_temp, _lease, url, stop, task) = fixture(BridgeSnapshot::default()).await;
    let client = reqwest::Client::new();
    for (path, expected_mime) in [
        ("/", "text/html; charset=utf-8"),
        ("/sw.js", "text/javascript; charset=utf-8"),
        ("/manifest.webmanifest", "application/manifest+json"),
        ("/openapi.json", "application/json"),
    ] {
        let response = client
            .get(format!("{url}{path}"))
            .bearer_auth(TEST_TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], expected_mime);
        assert_eq!(response.headers()["cache-control"], "private, no-cache");
        let etag = response.headers()["etag"].clone();
        let bytes = response.bytes().await.unwrap();
        assert!(!bytes.is_empty());
        if path == "/" {
            let html = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(!html.contains("<script>"));
            assert!(!html.contains("onclick="));
        }
        let cached = client
            .get(format!("{url}{path}"))
            .bearer_auth(TEST_TOKEN)
            .header("if-none-match", etag)
            .send()
            .await
            .unwrap();
        assert_eq!(cached.status(), 304);
        assert_eq!(cached.headers()["content-security-policy"], CSP);
        assert!(cached.bytes().await.unwrap().is_empty());
    }
    assert_eq!(mime_type("icon.svg"), "image/svg+xml");
    assert_eq!(mime_type("font.woff2"), "font/woff2");
    let spa = client
        .get(format!("{url}/board"))
        .bearer_auth(TEST_TOKEN)
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(spa.status(), 200);
    assert_eq!(
        client
            .get(format!("{url}/missing.js"))
            .bearer_auth(TEST_TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn enrollment_requires_bearer_post_body_and_single_use_token() {
    let (_temp, _lease, url, stop, task) = fixture(BridgeSnapshot::default()).await;
    let client = reqwest::Client::new();
    let invitation: serde_json::Value = client
        .post(format!("{url}/api/v1/enrollment/create"))
        .bearer_auth(TEST_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = invitation["token"].as_str().unwrap();
    assert_eq!(token.len(), 43);
    let query = client
        .get(format!("{url}/api/v1/enrollment?token={token}"))
        .bearer_auth(TEST_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(query.status(), 400);
    let response = client
        .post(format!("{url}/api/v1/enrollment"))
        .bearer_auth(TEST_TOKEN)
        .json(&serde_json::json!({"token":token}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["instance_token"],
        TEST_TOKEN
    );
    assert_eq!(
        client
            .post(format!("{url}/api/v1/enrollment"))
            .bearer_auth(TEST_TOKEN)
            .json(&serde_json::json!({"token":token}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let oversized = client
        .post(format!("{url}/api/v1/enrollment"))
        .bearer_auth(TEST_TOKEN)
        .header("content-type", "application/json")
        .body(vec![b'a'; 9000])
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), 413);
    assert_eq!(oversized.headers()["referrer-policy"], "no-referrer");
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[test]
fn openapi_covers_all_registered_json_operations_and_no_canonical_writes() {
    let document: serde_json::Value = serde_json::from_str(OPENAPI).unwrap();
    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(
        document["security"],
        serde_json::json!([{"instance_bearer":[]}])
    );
    let mut documented = Vec::new();
    for (path, operations) in document["paths"].as_object().unwrap() {
        for (method, _) in operations.as_object().unwrap() {
            documented.push((method.as_str(), path.as_str()));
        }
    }
    let mut registered = API_ROUTES.to_vec();
    registered.sort();
    documented.sort();
    assert_eq!(registered, documented);
    assert!(registered.iter().all(|(method, path)| *method == "get"
        || path.starts_with("/api/v1/enrollment")
        || *path == "/api/v1/push/subscriptions"));
    assert!(
        document["paths"]
            .as_object()
            .unwrap()
            .keys()
            .all(|path| !path.contains("/cancel") && !path.contains("/pause"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_reads_real_daemon_over_control_ipc_and_never_exposes_prompt_or_paths() {
    use vibemux_types::{
        Run, RunSpec, Task, TaskSpec,
        a2a::{A2aRunStart, RunWorkspace},
    };
    use vibemuxd::{WriterWorker, control::DaemonControlServer, process::DaemonPaths};
    let project = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let paths =
        DaemonPaths::from_project_root_with_local_app_data(project.path(), project.path()).unwrap();
    #[cfg(not(windows))]
    let paths = DaemonPaths::from_project_root(project.path()).unwrap();
    paths.ensure_runtime_dir().unwrap();
    let writer = WriterWorker::start(paths.database_path()).unwrap();
    let handle = writer.handle().unwrap();
    let project_id = handle.project_id().unwrap().unwrap();
    let task_record = Task::new(TaskSpec {
        project_id,
        title: "Fresh board title".into(),
        description: String::new(),
    })
    .unwrap();
    let task_id = task_record.task_id().to_string();
    let run = Run::new(RunSpec {
        project_id,
        task_id: task_record.task_id(),
        harness: "fixture".into(),
        role: "worker".into(),
        protocol: "a2a".into(),
        base_commit: "a".repeat(40),
    })
    .unwrap();
    handle
        .start_a2a_run(A2aRunStart {
            task: task_record,
            run,
            peer_id: "fixture_peer".into(),
            external_task_id: "fixture_external".into(),
            transport: "http_json".into(),
            protocol_version: "1.0".into(),
            workspace: RunWorkspace {
                path: project.path().to_string_lossy().into_owned(),
                branch: "DO_NOT_LEAK_BRANCH".into(),
                base_commit: "a".repeat(40),
                ownership_token: "DO_NOT_LEAK_SECRET".into(),
            },
            timestamp: time::OffsetDateTime::now_utc(),
            idempotency_key: "http_fixture".into(),
        })
        .unwrap();
    drop(handle);
    writer.shutdown().unwrap();
    let daemon = DaemonControlServer::start_for_paths(&paths).await.unwrap();
    let snapshot = tokio::time::timeout(
        Duration::from_secs(30),
        BridgePoller::new(paths, Duration::from_secs(5)).refresh(),
    )
    .await
    .unwrap()
    .unwrap();
    let (_temp, _lease, url, stop, http_task) = fixture(snapshot).await;
    let response = reqwest::Client::new()
        .get(format!("{url}/api/v1/tasks/{task_id}"))
        .bearer_auth(TEST_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(body.contains("Fresh board title"));
    assert!(body.contains("\"stale\":false"));
    for forbidden in ["DO_NOT_LEAK_BRANCH", "DO_NOT_LEAK_SECRET"] {
        assert!(!body.contains(forbidden));
    }
    stop.send(()).unwrap();
    http_task.await.unwrap().unwrap();
    daemon.shutdown().await.unwrap();
}
