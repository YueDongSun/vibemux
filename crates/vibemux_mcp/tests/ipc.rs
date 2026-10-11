#[path = "support/mcp_peer.rs"]
mod mcp_peer;

use mcp_peer::McpPeer;
use serde_json::json;
use uuid::Uuid;
use vibemux_types::{
    Run, RunSpec, Task, TaskSpec,
    a2a::{A2aRunStart, RunWorkspace},
};
use vibemuxd::{
    WriterWorker,
    control::{ControlClient, DaemonControlServer},
};

#[tokio::test]
async fn session_restart_budget_unloads_without_implicit_read_replay() {
    let temp = tempfile::tempdir().unwrap();
    let mut peer = McpPeer::start(&temp.path().join("missing_descriptor")).await;
    let mut observations = Vec::new();
    for index in 0..5 {
        let status = peer.call(json!({"jsonrpc":"2.0","id":10+index*2,"method":"tools/call","params":{"name":"host_status","arguments":{}}})).await;
        let failed = peer.call(json!({"jsonrpc":"2.0","id":11+index*2,"method":"tools/call","params":{"name":"health","arguments":{}}})).await;
        observations.push((status, failed));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    let status = peer.call(json!({"jsonrpc":"2.0","id":40,"method":"tools/call","params":{"name":"host_status","arguments":{}}})).await;
    let tools = peer
        .call(json!({"jsonrpc":"2.0","id":41,"method":"tools/list"}))
        .await;
    let notifications = peer.notifications;
    peer.finish().await;
    for (index, (before, failed)) in observations.iter().enumerate() {
        assert_eq!(
            before["result"]["structuredContent"]["worker_launches"],
            index.min(4)
        );
        assert_eq!(failed["result"]["isError"], true);
    }
    assert_eq!(status["result"]["structuredContent"]["worker_launches"], 4);
    assert_eq!(status["result"]["structuredContent"]["restarts_used"], 3);
    assert_eq!(
        status["result"]["structuredContent"]["backend_state"],
        "exhausted"
    );
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(
        notifications, 7,
        "four unloads and three availability reloads"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contained_reader_observes_disposable_daemon_without_changing_its_state() {
    let temp = tempfile::tempdir().expect("fixture directory");
    let database = temp.path().join("state.sqlite3");
    let runtime = temp.path().join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    let writer = WriterWorker::start(&database).expect("fixture writer");
    let handle = writer.handle().unwrap();
    let project_id = handle.project_id().unwrap().unwrap();
    let private_marker = Uuid::new_v4().to_string();
    let task = Task::new(TaskSpec {
        project_id,
        title: "Observe a fixture task".into(),
        description: String::new(),
    })
    .unwrap();
    let task_id = task.task_id();
    let run = Run::new(RunSpec {
        project_id,
        task_id,
        harness: "fixture".into(),
        role: "worker".into(),
        protocol: "a2a".into(),
        base_commit: "a".repeat(40),
    })
    .unwrap();
    let run_id = run.run_id();
    handle
        .start_a2a_run(A2aRunStart {
            task,
            run,
            peer_id: "fixture_peer".into(),
            external_task_id: "fixture_task".into(),
            transport: "http_json".into(),
            protocol_version: "1.0".into(),
            workspace: RunWorkspace {
                path: temp.path().to_string_lossy().into_owned(),
                branch: private_marker.clone(),
                base_commit: "a".repeat(40),
                ownership_token: Uuid::new_v4().to_string(),
            },
            timestamp: time::OffsetDateTime::now_utc(),
            idempotency_key: "fixture_start".into(),
        })
        .expect("fixture state");
    let before = handle.a2a_run(run_id).unwrap();
    drop(handle);
    writer.shutdown().unwrap();
    let server = DaemonControlServer::start(&database, &runtime)
        .await
        .expect("fixture control server");
    let descriptor_before = std::fs::read(server.descriptor_path()).unwrap();
    let mut peer = McpPeer::start(server.descriptor_path()).await;
    let health = peer.call(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"health","arguments":{}}})).await;
    let tasks = peer.call(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"frontend_tasks","arguments":{}}})).await;
    let detail = peer.call(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"frontend_task","arguments":{"task_id":task_id}}})).await;
    let status = peer.call(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"host_status","arguments":{}}})).await;
    peer.finish().await;
    let descriptor_unchanged =
        std::fs::read(server.descriptor_path()).unwrap() == descriptor_before;
    let daemon_alive = ControlClient::from_descriptor(server.descriptor_path())
        .unwrap()
        .health()
        .await
        .unwrap()
        .healthy;
    server.shutdown().await.unwrap();
    let writer = WriterWorker::start(&database).unwrap();
    let state_unchanged = writer.handle().unwrap().a2a_run(run_id).unwrap() == before;
    writer.shutdown().unwrap();
    assert_eq!(health["result"]["isError"], false);
    assert_eq!(health["result"]["structuredContent"]["healthy"], true);
    assert!(
        health["result"]["structuredContent"]
            .get("process_id")
            .is_none()
    );
    assert_eq!(
        tasks["result"]["structuredContent"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        detail["result"]["structuredContent"]["runs"][0]["run_id"],
        run_id.to_string()
    );
    assert_eq!(status["result"]["structuredContent"]["worker_launches"], 1);
    let encoded = detail.to_string();
    assert!(!encoded.contains(&private_marker));
    assert!(!encoded.contains("worktree_path") && !encoded.contains("fixture_branch"));
    assert!(descriptor_unchanged && state_unchanged && daemon_alive);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_versions_keep_old_health_and_reject_unsupported_reads_explicitly() {
    let temp = tempfile::tempdir().unwrap();
    let server = DaemonControlServer::start(&temp.path().join("state.sqlite3"), temp.path())
        .await
        .unwrap();
    let original = std::fs::read(server.descriptor_path()).unwrap();
    let mut descriptor: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let legacy = temp.path().join("legacy.json");
    std::fs::copy(server.descriptor_path(), &legacy).unwrap();
    descriptor["version"] = json!(1);
    std::fs::write(&legacy, serde_json::to_vec(&descriptor).unwrap()).unwrap();
    let mut peer = McpPeer::start(&legacy).await;
    let health = peer.call(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"health","arguments":{}}})).await;
    let refused = peer.call(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"frontend_tasks","arguments":{}}})).await;
    peer.finish().await;
    descriptor["version"] = json!(8);
    std::fs::write(&legacy, serde_json::to_vec(&descriptor).unwrap()).unwrap();
    let mut peer = McpPeer::start(&legacy).await;
    let unknown = peer.call(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"health","arguments":{}}})).await;
    peer.finish().await;
    let unchanged = std::fs::read(server.descriptor_path()).unwrap() == original;
    server.shutdown().await.unwrap();
    assert_eq!(health["result"]["structuredContent"]["healthy"], true);
    for response in [refused, unknown] {
        assert_eq!(response["result"]["isError"], true);
        let error: serde_json::Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(error["error"]["code"], "control_version_unsupported");
    }
    assert!(unchanged);
}
