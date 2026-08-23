use std::{collections::BTreeMap, time::Duration};

use a2a::TRANSPORT_PROTOCOL_HTTP_JSON;
use tokio::{net::TcpStream, time::sleep};
use vibemux_a2a::{
    InformationShare, LocalInformationServer, MAX_HTTP_BODY_BYTES, discover_agent_card,
    share_information,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_loopback_peers_share_information() {
    let server = LocalInformationServer::start("receiver_peer")
        .await
        .expect("start owned A2A server");
    let address = server.address();
    let card = discover_agent_card(server.base_url())
        .await
        .expect("discover Agent Card over HTTP");
    assert_eq!(card.supported_interfaces.len(), 1);
    assert_eq!(
        card.supported_interfaces[0].protocol_binding,
        TRANSPORT_PROTOCOL_HTTP_JSON
    );

    let share = InformationShare {
        correlation_id: format!("correlation_{}", uuid::Uuid::new_v4().simple()),
        sender_peer_id: "sender_peer".to_string(),
        information_kind: "status".to_string(),
        text: "VIBEMUX_A2A_INFO_SHARE_OK".to_string(),
        metadata: BTreeMap::from([("scope".to_string(), "basic".to_string())]),
    };
    let acknowledgement = share_information(server.base_url(), &share)
        .await
        .expect("share information through official A2A client/server");
    assert!(acknowledgement.accepted);
    assert_eq!(acknowledgement.correlation_id, share.correlation_id);
    assert_eq!(acknowledgement.receiver_peer_id, "receiver_peer");
    assert_eq!(server.received().expect("received shares"), vec![share]);

    server.shutdown().await.expect("graceful server shutdown");
    sleep(Duration::from_millis(25)).await;
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_share_is_rejected_without_killing_server() {
    let server = LocalInformationServer::start("receiver_peer")
        .await
        .expect("start owned A2A server");
    let invalid = serde_json::json!({
        "message": {
            "messageId": "invalid_message",
            "contextId": "invalid_context",
            "role": "ROLE_USER",
            "parts": [{"data": {"contract": "unknown.contract"}}]
        }
    });
    let response = reqwest::Client::new()
        .post(format!("{}/message:send", server.base_url()))
        .header("A2A-Version", "1.0")
        .json(&invalid)
        .send()
        .await
        .expect("send malformed request");
    assert!(response.status().is_client_error());

    let oversized = "x".repeat(MAX_HTTP_BODY_BYTES + 1);
    let response = reqwest::Client::new()
        .post(format!("{}/message:send", server.base_url()))
        .header("A2A-Version", "1.0")
        .header("Content-Type", "application/json")
        .body(oversized)
        .send()
        .await
        .expect("send oversized request");
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

    let valid = InformationShare {
        correlation_id: "correlation_after_error".to_string(),
        sender_peer_id: "sender_peer".to_string(),
        information_kind: "status".to_string(),
        text: "VIBEMUX_A2A_INFO_SHARE_OK".to_string(),
        metadata: BTreeMap::new(),
    };
    assert!(share_information(server.base_url(), &valid).await.is_ok());
    server.shutdown().await.expect("graceful server shutdown");
}
