//! Authenticated loopback HTTP surface. Security middleware includes fallback/errors.
use crate::{
    control_bridge::SnapshotState,
    enrollment::{AttemptLimiter, EnrollmentStore},
    push::{PushService, PushSubscription},
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

pub const MAX_HTTP_BODY: usize = 8 * 1024;
pub const MAX_HTTP_CONCURRENCY: usize = 32;
pub const CSP: &str = "default-src 'self'; connect-src 'self'; img-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";
static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");
pub const OPENAPI: &str = include_str!("../openapi.json");
pub const API_ROUTES: &[(&str, &str)] = &[
    ("get", "/api/v1/health"),
    ("get", "/api/v1/tasks"),
    ("get", "/api/v1/tasks/{task_id}"),
    ("get", "/api/v1/push/public_key"),
    ("post", "/api/v1/push/subscriptions"),
    ("post", "/api/v1/enrollment"),
    ("post", "/api/v1/enrollment/create"),
];

struct HttpState {
    instance_token: String,
    token_digest: [u8; 32],
    auth_limiter: Mutex<AttemptLimiter>,
    capacity: Arc<Semaphore>,
    enrollment: Mutex<EnrollmentStore>,
    enrollment_url: String,
    push: Arc<PushService>,
}

pub fn build_router(
    snapshot_state: SnapshotState,
    push: Arc<PushService>,
    instance_token: String,
    enrollment_url: String,
) -> Router {
    let state = Arc::new(HttpState {
        token_digest: Sha256::digest(instance_token.as_bytes()).into(),
        instance_token,
        auth_limiter: Mutex::new(AttemptLimiter::default()),
        capacity: Arc::new(Semaphore::new(MAX_HTTP_CONCURRENCY)),
        enrollment: Mutex::new(EnrollmentStore::default()),
        enrollment_url,
        push,
    });
    let service_routes = Router::new()
        .route("/api/v1/push/public_key", get(public_key))
        .route("/api/v1/push/subscriptions", post(subscribe))
        .route("/api/v1/enrollment", post(redeem))
        .route("/api/v1/enrollment/create", post(invite))
        .route("/openapi.json", get(openapi))
        .fallback(static_asset)
        .with_state(state.clone());
    Router::new()
        .merge(crate::api::api_router(snapshot_state))
        .merge(service_routes)
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY))
        .layer(middleware::from_fn_with_state(state, authenticate))
        .layer(middleware::from_fn(security_headers))
}

fn error(status: StatusCode, code: &'static str) -> Response {
    (status, Json(serde_json::json!({"error_code":code}))).into_response()
}

async fn authenticate(
    State(state): State<Arc<HttpState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut headers = request.headers().get_all(header::AUTHORIZATION).iter();
    let token = headers
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let duplicate = headers.next().is_some();
    let now = Instant::now();
    {
        let Ok(mut limiter) = state.auth_limiter.lock() else {
            return error(StatusCode::SERVICE_UNAVAILABLE, "web_auth_unavailable");
        };
        if !limiter.permitted(now) {
            return error(StatusCode::TOO_MANY_REQUESTS, "web_auth_rate_limited");
        }
        let valid = !duplicate
            && token.is_some_and(|token| {
                token.len() <= 128
                    && bool::from(
                        state
                            .token_digest
                            .ct_eq(&Sha256::digest(token.as_bytes())[..]),
                    )
            });
        if !valid {
            limiter.failed(now);
            return error(StatusCode::UNAUTHORIZED, "web_unauthorized");
        }
    }
    if request.uri().path() == "/api/v1/enrollment" && request.uri().query().is_some() {
        return error(StatusCode::BAD_REQUEST, "web_enrollment_body_only");
    }
    let Ok(_permit) = state.capacity.clone().try_acquire_owned() else {
        return error(StatusCode::TOO_MANY_REQUESTS, "web_busy");
    };
    next.run(request).await
}

async fn security_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Authorization"));
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

async fn public_key(State(state): State<Arc<HttpState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"public_key":state.push.public_key()}))
}
async fn subscribe(
    State(state): State<Arc<HttpState>>,
    payload: Result<Json<PushSubscription>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let subscription = match payload {
        Ok(Json(subscription)) => subscription,
        Err(rejection) => return error(rejection.status(), "web_subscription_rejected"),
    };
    let push = state.push.clone();
    match tokio::task::spawn_blocking(move || push.subscribe(subscription)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        _ => error(StatusCode::BAD_REQUEST, "web_subscription_rejected"),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentRequest {
    token: String,
}
async fn redeem(
    State(state): State<Arc<HttpState>>,
    payload: Result<Json<EnrollmentRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let request = match payload {
        Ok(Json(request)) => request,
        Err(rejection) => {
            record_enrollment("rejected");
            return error(rejection.status(), "web_enrollment_rejected");
        }
    };
    let result = state
        .enrollment
        .lock()
        .map_err(|_| ())
        .and_then(|mut store| store.redeem(&request.token, Instant::now()).map_err(|_| ()));
    match result {
        Ok(()) => {
            record_enrollment("accepted");
            Json(serde_json::json!({"instance_token":state.instance_token})).into_response()
        }
        Err(()) => {
            record_enrollment("rejected");
            error(StatusCode::BAD_REQUEST, "web_enrollment_rejected")
        }
    }
}
fn record_enrollment(outcome: &str) {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |t| t.as_secs());
    eprintln!("time={time} outcome={outcome}");
}
async fn invite(State(state): State<Arc<HttpState>>) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        state
            .enrollment
            .lock()
            .map_err(|_| ())
            .and_then(|mut store| {
                store
                    .issue(&state.enrollment_url, Instant::now())
                    .map_err(|_| ())
            })
    })
    .await;
    match result {
        Ok(Ok(invitation)) => Json(invitation).into_response(),
        _ => error(StatusCode::TOO_MANY_REQUESTS, "web_enrollment_capacity"),
    }
}

pub fn mime_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}
fn asset_response(bytes: &'static [u8], mime: &'static str, request: &Request<Body>) -> Response {
    let etag = format!("\"{}\"", URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)));
    let matches = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let value = candidate
                    .trim()
                    .strip_prefix("W/")
                    .unwrap_or(candidate.trim());
                value == etag || value == "*"
            })
        });
    let mut response = if matches {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, mime)], bytes).into_response()
    };
    if let Ok(etag) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    response
}
async fn openapi(request: Request<Body>) -> Response {
    asset_response(OPENAPI.as_bytes(), "application/json", &request)
}
async fn static_asset(request: Request<Body>) -> Response {
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        return error(StatusCode::METHOD_NOT_ALLOWED, "web_method_not_allowed");
    }
    let path = request.uri().path().trim_start_matches('/');
    if path.starts_with("api/") {
        return error(StatusCode::NOT_FOUND, "web_route_not_found");
    }
    let name = if path.is_empty() { "index.html" } else { path };
    let file = ASSETS.get_file(name).or_else(|| {
        // Only navigation gets the SPA shell, never a missing JS/manifest request.
        if !path.contains('.')
            && request
                .headers()
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("text/html"))
        {
            ASSETS.get_file("index.html")
        } else {
            None
        }
    });
    match file {
        Some(file) => asset_response(
            file.contents(),
            mime_type(file.path().to_str().unwrap_or("")),
            &request,
        ),
        None => error(StatusCode::NOT_FOUND, "web_asset_not_found"),
    }
}
