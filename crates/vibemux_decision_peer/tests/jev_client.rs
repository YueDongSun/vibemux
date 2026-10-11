#![cfg(feature = "test_endpoint")]
//! Real-HTTP tests against a loopback axum server (ADR 034 §6).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use vibemux_decision::{
    AgentKind, DecisionQuestion, ExternalError, JevConfig, Label, PeerRequest, RouteFacts,
    Threshold,
};
use vibemux_decision_peer::jev_client::{ClientLimits, Credential, JevClient, read_credential_env};

fn request() -> PeerRequest {
    let eligible = [AgentKind::Claude, AgentKind::Codex];
    let question = DecisionQuestion::harness_route(
        &eligible,
        &BTreeMap::new(),
        Threshold::from_unit(0.9).unwrap(),
    )
    .unwrap();
    let facts = RouteFacts::new(
        vec![Label::new("rust").unwrap()],
        2048,
        Some(AgentKind::Codex),
        &eligible,
    );
    PeerRequest::for_route(&question, &facts, &JevConfig::default())
}

fn success_body() -> Value {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "harness_route": {
                "type": "choice",
                "choice": "codex",
                "probabilities": {"claude": 0.1, "codex": 0.8, "escalate": 0.1},
                "confidence": 0.81,
            }
        },
        "usage": {"tokens": 12},
    })
}

/// Shrunk limits so timeout/retry tests stay fast.
fn fast_limits() -> ClientLimits {
    ClientLimits {
        per_attempt_timeout: Duration::from_millis(400),
        total_budget: Duration::from_millis(900),
        retry_backoff: Duration::from_millis(50),
        max_attempts: 2,
        max_response_bytes: 4096,
    }
}

struct RunningServer {
    address: SocketAddr,
    stop: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl RunningServer {
    async fn shutdown(self) {
        let _ = self.stop.send(());
        self.handle.await.expect("server task joins");
    }

    fn endpoint(&self) -> String {
        format!("http://{}/v1/systemone", self.address)
    }

    fn client(&self, limits: ClientLimits) -> JevClient {
        JevClient::with_test_endpoint(limits, &self.endpoint()).expect("loopback client")
    }
}

async fn spawn(app: Router) -> RunningServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("local address");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .expect("server runs");
    });
    RunningServer {
        address,
        stop,
        handle,
    }
}

#[tokio::test]
async fn a_success_response_round_trips_the_request_shape_and_auth_header() {
    let app = Router::new().route(
        "/v1/systemone",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(
                headers.get("authorization").expect("authorization header"),
                "Bearer sk_test_value"
            );
            assert_eq!(body["model"], "jev-latest");
            assert!(body["state"].is_object(), "state must be an object");
            let questions = &body["questions"]["harness_route"];
            assert_eq!(questions["type"], "choice");
            let criteria = questions["criteria"].as_object().expect("criteria object");
            let mut keys: Vec<_> = criteria.keys().cloned().collect();
            keys.sort();
            assert_eq!(keys, ["claude", "codex", "escalate"]);
            let as_text = body.to_string();
            assert!(!as_text.contains("credential_env\""));
            assert!(!as_text.contains("\"prompt\""));
            Json(success_body())
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let answer = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await
        .expect("answered");
    assert_eq!(answer.model, "jev-1.13.0");
    assert_eq!(answer.choice, "codex");
    assert_eq!(answer.probabilities.len(), 3);
    server.shutdown().await;
}

#[tokio::test]
async fn a_redirect_is_refused_and_never_followed() {
    let app = Router::new().route(
        "/v1/systemone",
        post(|| async {
            (
                StatusCode::TEMPORARY_REDIRECT,
                [("location", "http://192.0.2.1/forbidden")],
            )
                .into_response()
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let result = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await;
    assert_eq!(result, Err(ExternalError::RedirectRefused));
    server.shutdown().await;
}

#[tokio::test]
async fn an_oversized_content_length_is_refused() {
    let app = Router::new().route("/v1/systemone", post(|| async { "x".repeat(5000) }));
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let result = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await;
    assert_eq!(result, Err(ExternalError::ResponseTooLarge));
    server.shutdown().await;
}

#[tokio::test]
async fn an_oversized_streamed_body_is_refused() {
    let app = Router::new().route(
        "/v1/systemone",
        post(|| async {
            axum::body::Body::from_stream(futures::stream::iter(
                (0..20).map(|_| Ok::<_, std::io::Error>(vec![b'x'; 1024])),
            ))
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let result = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await;
    assert_eq!(result, Err(ExternalError::ResponseTooLarge));
    server.shutdown().await;
}

#[tokio::test]
async fn rate_limited_once_then_success_retries_exactly_once() {
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    let app = Router::new().route(
        "/v1/systemone",
        post(move || {
            let counted = counted.clone();
            async move {
                let seen = counted.fetch_add(1, Ordering::SeqCst);
                if seen == 0 {
                    StatusCode::TOO_MANY_REQUESTS.into_response()
                } else {
                    Json(success_body()).into_response()
                }
            }
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let answer = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await
        .expect("answered after one retry");
    assert_eq!(answer.choice, "codex");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn rate_limited_twice_exhausts_the_single_retry() {
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    let app = Router::new().route(
        "/v1/systemone",
        post(move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                StatusCode::from_u16(529).expect("529 is a valid status")
            }
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let result = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await;
    assert_eq!(result, Err(ExternalError::RateLimited));
    assert_eq!(count.load(Ordering::SeqCst), 2, "never a third request");
    server.shutdown().await;
}

#[derive(Clone, Copy)]
enum MalformedCase {
    NotJson,
    MissingQuestionKey,
    ExtraAnswerKey,
    UnknownAnswerField,
    WrongType,
}

#[tokio::test]
async fn malformed_responses_are_rejected() {
    let selected = Arc::new(Mutex::new(MalformedCase::NotJson));
    let shared = selected.clone();
    let app = Router::new().route(
        "/v1/systemone",
        post(move || {
            let shared = shared.clone();
            async move {
                let case = *shared.lock().await;
                match case {
                    MalformedCase::NotJson => "not json at all".to_string(),
                    MalformedCase::MissingQuestionKey => json!({
                        "model": "jev-1.13.0",
                        "answers": {
                            "some_other_question": {
                                "type": "choice",
                                "choice": "codex",
                                "probabilities": {"codex": 1.0},
                                "confidence": 1.0,
                            }
                        }
                    })
                    .to_string(),
                    MalformedCase::ExtraAnswerKey => json!({
                        "model": "jev-1.13.0",
                        "answers": {
                            "harness_route": {
                                "type": "choice",
                                "choice": "codex",
                                "probabilities": {"codex": 0.9, "escalate": 0.1},
                                "confidence": 0.9,
                            },
                            "extra_question": {
                                "type": "choice",
                                "choice": "codex",
                                "probabilities": {"codex": 1.0},
                                "confidence": 1.0,
                            }
                        }
                    })
                    .to_string(),
                    MalformedCase::UnknownAnswerField => json!({
                        "model": "jev-1.13.0",
                        "answers": {
                            "harness_route": {
                                "type": "choice",
                                "choice": "codex",
                                "probabilities": {"codex": 0.9, "escalate": 0.1},
                                "confidence": 0.9,
                                "unexpected": true,
                            }
                        }
                    })
                    .to_string(),
                    MalformedCase::WrongType => json!({
                        "model": "jev-1.13.0",
                        "answers": {
                            "harness_route": {
                                "type": "not_choice",
                                "choice": "codex",
                                "probabilities": {"codex": 0.9, "escalate": 0.1},
                                "confidence": 0.9,
                            }
                        }
                    })
                    .to_string(),
                }
            }
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    for case in [
        MalformedCase::NotJson,
        MalformedCase::MissingQuestionKey,
        MalformedCase::ExtraAnswerKey,
        MalformedCase::UnknownAnswerField,
        MalformedCase::WrongType,
    ] {
        *selected.lock().await = case;
        let result = client
            .ask(&request(), &Credential::new("sk_test_value"))
            .await;
        assert_eq!(result, Err(ExternalError::ResponseMalformed));
    }
    server.shutdown().await;
}

#[tokio::test]
async fn unknown_top_level_fields_are_ignored() {
    let app = Router::new().route(
        "/v1/systemone",
        post(|| async {
            let mut body = success_body();
            body.as_object_mut()
                .expect("object")
                .insert("extra".to_string(), json!("ignored"));
            Json(body)
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let answer = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await
        .expect("unknown top-level fields do not block parsing");
    assert_eq!(answer.choice, "codex");
    server.shutdown().await;
}

#[tokio::test]
async fn a_slow_server_hits_the_per_attempt_timeout() {
    let app = Router::new().route(
        "/v1/systemone",
        post(|| async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Json(success_body())
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let result = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await;
    assert_eq!(result, Err(ExternalError::BackendTimeout));
    server.shutdown().await;
}

#[tokio::test]
async fn the_response_model_passes_through_unchanged() {
    let app = Router::new().route(
        "/v1/systemone",
        post(|| async {
            let mut body = success_body();
            body["model"] = json!("something-else");
            Json(body)
        }),
    );
    let server = spawn(app).await;
    let client = server.client(fast_limits());
    let answer = client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await
        .expect("answered");
    assert_eq!(answer.model, "something-else");
    server.shutdown().await;
}

#[tokio::test]
async fn http_status_mapping_for_unauthorized_invalid_and_other_errors() {
    for (status, expected) in [
        (StatusCode::UNAUTHORIZED, ExternalError::Unauthorized),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            ExternalError::RequestInvalid,
        ),
        (StatusCode::INTERNAL_SERVER_ERROR, ExternalError::HttpStatus),
    ] {
        let app = Router::new().route("/v1/systemone", post(move || async move { status }));
        let server = spawn(app).await;
        let client = server.client(fast_limits());
        let result = client
            .ask(&request(), &Credential::new("sk_test_value"))
            .await;
        assert_eq!(result, Err(expected), "status {status}");
        server.shutdown().await;
    }
}

#[tokio::test]
async fn missing_or_empty_credential_never_reaches_the_server() {
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    let app = Router::new().route(
        "/v1/systemone",
        post(move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                Json(success_body())
            }
        }),
    );
    let server = spawn(app).await;
    assert_eq!(
        read_credential_env("SOME_KEY", &|_| None).err(),
        Some(ExternalError::CredentialUnavailable)
    );
    assert_eq!(
        read_credential_env("SOME_KEY", &|_| Some(String::new())).err(),
        Some(ExternalError::CredentialUnavailable)
    );
    assert_eq!(count.load(Ordering::SeqCst), 0, "no request was ever sent");
    // Confirm the server still works so a wiring mistake would not hide as a pass.
    let client = server.client(fast_limits());
    client
        .ask(&request(), &Credential::new("sk_test_value"))
        .await
        .expect("server reachable");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.shutdown().await;
}
