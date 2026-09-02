//! Read-only CC Switch configuration adapter and bounded model HTTP requests.

use futures::StreamExt;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    path::Path,
    time::{Duration, Instant},
};
use thiserror::Error;

pub const MAX_PROVIDER_CONFIGURATION_BYTES: usize = 256 * 1024;
pub const MAX_MODEL_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_PROMPT_BYTES: usize = 24 * 1024;
pub const MAX_OUTPUT_TOKENS: u32 = 4096;
pub const MODEL_DEADLINE: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSelection {
    pub app_type: String,
    pub provider_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderInventory {
    pub selection: ProviderSelection,
    pub name: String,
    pub configured_model: Option<String>,
    pub usable_configuration: bool,
}

#[derive(Clone, Copy, Debug)]
enum ApiKind {
    Anthropic,
    ChatCompletions,
}

#[derive(Clone)]
pub struct ModelProvider {
    pub label: String,
    pub configured_model: String,
    pub wire_model: String,
    endpoint: url::Url,
    api_key: String,
    auth_header: &'static str,
    api_kind: ApiKind,
}

impl fmt::Debug for ModelProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelProvider")
            .field("label", &self.label)
            .field("model", &self.wire_model)
            .field("endpoint", &"[redacted]")
            .field("api_key", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider configuration is unavailable")]
    Configuration,
    #[error("provider interface is not supported")]
    Unsupported,
    #[error("provider request is invalid")]
    InvalidRequest,
    #[error("provider transport failed")]
    Transport,
    #[error("provider request deadline exceeded")]
    Deadline,
    #[error("provider rejected request with status {0}")]
    Http(u16),
    #[error("provider response exceeds its size limit")]
    ResponseTooLarge,
    #[error("provider response is invalid")]
    InvalidResponse,
}
impl ProviderError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Configuration => "provider_configuration_unavailable",
            Self::Unsupported => "provider_interface_unsupported",
            Self::InvalidRequest => "provider_invalid_request",
            Self::Transport => "provider_transport_failed",
            Self::Deadline => "provider_deadline",
            Self::Http(_) => "provider_http_error",
            Self::ResponseTooLarge => "provider_response_too_large",
            Self::InvalidResponse => "provider_invalid_response",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelCompletion {
    pub value: Value,
    pub model: String,
    pub elapsed_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub response_sha256: String,
}

fn open_configuration(path: &Path) -> Result<Connection, ProviderError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ProviderError::Configuration)?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|_| ProviderError::Configuration)?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|_| ProviderError::Configuration)?;
    Ok(connection)
}

pub fn inventory(path: &Path) -> Result<Vec<ProviderInventory>, ProviderError> {
    let connection = open_configuration(path)?;
    let mut query=connection.prepare("SELECT id,app_type,name,settings_config FROM providers ORDER BY app_type,sort_index,name LIMIT 256")
        .map_err(|_|ProviderError::Configuration)?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|_| ProviderError::Configuration)?;
    let mut inventory = Vec::new();
    for row in rows {
        let (id, app_type, name, encoded) = row.map_err(|_| ProviderError::Configuration)?;
        let provider = parse_provider(&app_type, &name, &encoded);
        inventory.push(ProviderInventory {
            selection: ProviderSelection {
                app_type,
                provider_id: id,
            },
            name,
            configured_model: provider.as_ref().ok().map(|p| p.configured_model.clone()),
            usable_configuration: provider.is_ok(),
        });
    }
    Ok(inventory)
}

pub fn load_provider(
    path: &Path,
    selection: &ProviderSelection,
) -> Result<ModelProvider, ProviderError> {
    if selection.app_type.len() > 64 || selection.provider_id.len() > 256 {
        return Err(ProviderError::InvalidRequest);
    }
    let connection = open_configuration(path)?;
    let (name, encoded): (String, String) = connection
        .query_row(
            "SELECT name,settings_config FROM providers WHERE app_type=? AND id=?",
            [&selection.app_type, &selection.provider_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| ProviderError::Configuration)?;
    parse_provider(&selection.app_type, &name, &encoded)
}

fn parse_provider(
    app_type: &str,
    label: &str,
    encoded: &str,
) -> Result<ModelProvider, ProviderError> {
    if encoded.len() > MAX_PROVIDER_CONFIGURATION_BYTES {
        return Err(ProviderError::Configuration);
    }
    let config: Value = serde_json::from_str(encoded).map_err(|_| ProviderError::Configuration)?;
    let (base, key, model, auth_header, api_kind) = match app_type {
        "claude" | "claude-desktop" => {
            let env = config.get("env").ok_or(ProviderError::Configuration)?;
            let (key, header) = if let Some(key) = env
                .get("ANTHROPIC_API_KEY")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                (key, "x-api-key")
            } else {
                (field(env, "ANTHROPIC_AUTH_TOKEN")?, "authorization")
            };
            (
                field(env, "ANTHROPIC_BASE_URL")?,
                key,
                env.get("ANTHROPIC_MODEL")
                    .or_else(|| env.get("ANTHROPIC_DEFAULT_SONNET_MODEL"))
                    .and_then(Value::as_str)
                    .ok_or(ProviderError::Configuration)?,
                header,
                ApiKind::Anthropic,
            )
        }
        "opencode" => {
            let options = config.get("options").ok_or(ProviderError::Configuration)?;
            let model = config
                .get("models")
                .and_then(Value::as_object)
                .and_then(|m| m.keys().next())
                .ok_or(ProviderError::Configuration)?;
            let api_kind = match config.get("npm").and_then(Value::as_str) {
                Some("@ai-sdk/openai-compatible") => ApiKind::ChatCompletions,
                Some("@ai-sdk/anthropic") => ApiKind::Anthropic,
                _ => return Err(ProviderError::Unsupported),
            };
            (
                field(options, "baseURL")?,
                field(options, "apiKey")?,
                model.as_str(),
                "authorization",
                api_kind,
            )
        }
        _ => return Err(ProviderError::Unsupported),
    };
    if key.is_empty()
        || key.len() > 8192
        || key.contains(['\r', '\n'])
        || model.is_empty()
        || model.len() > 256
    {
        return Err(ProviderError::Configuration);
    }
    let mut endpoint = url::Url::parse(base).map_err(|_| ProviderError::Configuration)?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ProviderError::Configuration);
    }
    let path = endpoint.path().trim_end_matches('/');
    let suffix = match api_kind {
        ApiKind::Anthropic => "messages",
        ApiKind::ChatCompletions => "chat/completions",
    };
    let path = if path.ends_with(&format!("/{suffix}")) {
        path.to_string()
    } else if matches!(api_kind, ApiKind::ChatCompletions) || path.ends_with("/v1") {
        // OpenAI-compatible baseURL already includes any provider-specific API version.
        format!("{path}/{suffix}")
    } else {
        format!("{path}/v1/{suffix}")
    };
    endpoint.set_path(&path);
    // CC Switch's Claude context-window suffix is a harness annotation, not a model substitution.
    let wire_model = model
        .strip_suffix("[1M]")
        .or_else(|| model.strip_suffix("[1m]"))
        .unwrap_or(model)
        .to_string();
    Ok(ModelProvider {
        label: label.to_string(),
        configured_model: model.to_string(),
        wire_model,
        endpoint,
        api_key: key.to_string(),
        auth_header,
        api_kind,
    })
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(ProviderError::Configuration)
}

impl ModelProvider {
    pub async fn complete_json(
        &self,
        system: &str,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<ModelCompletion, ProviderError> {
        if system.len() + prompt.len() > MAX_PROMPT_BYTES
            || max_tokens == 0
            || max_tokens > MAX_OUTPUT_TOKENS
        {
            return Err(ProviderError::InvalidRequest);
        }
        let started = Instant::now();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(MODEL_DEADLINE)
            .build()
            .map_err(|_| ProviderError::Transport)?;
        let payload = match self.api_kind {
            ApiKind::Anthropic => {
                json!({"model":self.wire_model,"max_tokens":max_tokens,"system":system,"messages":[{"role":"user","content":prompt}],"stream":false})
            }
            ApiKind::ChatCompletions => {
                json!({"model":self.wire_model,"max_tokens":max_tokens,"messages":[{"role":"system","content":system},{"role":"user","content":prompt}],"stream":false})
            }
        };
        let mut request = client.post(self.endpoint.clone()).json(&payload);
        request = if self.auth_header == "authorization" {
            request.bearer_auth(&self.api_key)
        } else {
            request.header(self.auth_header, &self.api_key)
        };
        if matches!(self.api_kind, ApiKind::Anthropic) {
            request = request.header("anthropic-version", "2023-06-01");
        }
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                ProviderError::Deadline
            } else {
                ProviderError::Transport
            }
        })?;
        if !response.status().is_success() {
            return Err(ProviderError::Http(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_MODEL_RESPONSE_BYTES as u64)
        {
            return Err(ProviderError::ResponseTooLarge);
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ProviderError::Transport)?;
            if body.len().saturating_add(chunk.len()) > MAX_MODEL_RESPONSE_BYTES {
                return Err(ProviderError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let response: Value =
            serde_json::from_slice(&body).map_err(|_| ProviderError::InvalidResponse)?;
        let text = match self.api_kind {
            ApiKind::Anthropic => response
                .get("content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .ok_or(ProviderError::InvalidResponse)?,
            ApiKind::ChatCompletions => response
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .ok_or(ProviderError::InvalidResponse)?
                .to_string(),
        };
        let value = parse_model_json(&text)?;
        let usage = response.get("usage");
        Ok(ModelCompletion {
            value,
            model: self.wire_model.clone(),
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            input_tokens: usage
                .and_then(|u| u.get("input_tokens").or_else(|| u.get("prompt_tokens")))
                .and_then(Value::as_u64),
            output_tokens: usage
                .and_then(|u| {
                    u.get("output_tokens")
                        .or_else(|| u.get("completion_tokens"))
                })
                .and_then(Value::as_u64),
            response_sha256: Sha256::digest(text.as_bytes())
                .iter()
                .flat_map(|byte| {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    [
                        char::from(HEX[usize::from(*byte >> 4)]),
                        char::from(HEX[usize::from(*byte & 15)]),
                    ]
                })
                .collect(),
        })
    }
}

pub fn parse_model_json(text: &str) -> Result<Value, ProviderError> {
    let text = text.trim();
    let body = if let Some(rest) = text
        .strip_prefix("```json\n")
        .or_else(|| text.strip_prefix("```\n"))
    {
        rest.strip_suffix("```")
            .ok_or(ProviderError::InvalidResponse)?
            .trim()
    } else {
        text
    };
    serde_json::from_str(body).map_err(|_| ProviderError::InvalidResponse)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_json_and_configuration_boundaries() {
        assert!(parse_model_json("preface {\"ok\":true}").is_err());
        assert_eq!(
            parse_model_json("```json\n{\"ok\":true}\n```").expect("json"),
            json!({"ok":true})
        );
        let config=json!({"env":{"ANTHROPIC_BASE_URL":"https://example.invalid/anthropic","ANTHROPIC_AUTH_TOKEN":"private_test_key","ANTHROPIC_MODEL":"test-model[1M]"}}).to_string();
        let provider = parse_provider("claude", "test", &config).expect("provider");
        assert_eq!(provider.wire_model, "test-model");
        assert_eq!(provider.endpoint.path(), "/anthropic/v1/messages");
        assert!(!format!("{provider:?}").contains("private_test_key"));
        assert!(!format!("{provider:?}").contains("example.invalid"));
        assert!(parse_provider("codex", "test", &config).is_err());
    }
    #[test]
    fn cc_switch_configuration_connection_is_read_only() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join("providers.sqlite3");
        let writable = Connection::open(&path).expect("fixture database");
        writable
            .execute("CREATE TABLE sentinel(value TEXT)", [])
            .expect("schema");
        drop(writable);
        let connection = open_configuration(&path).expect("read-only adapter");
        assert!(
            connection
                .execute("INSERT INTO sentinel VALUES ('changed')", [])
                .is_err()
        );
    }

    #[tokio::test]
    async fn bounded_real_http_checks_auth_json_usage_and_refuses_redirects() {
        use axum::{
            Json, Router,
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
            routing::post,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let app=Router::new().route("/v1/messages",post(|headers:HeaderMap,Json(body):Json<Value>|async move{
            assert_eq!(headers.get("authorization").expect("authorization"),"Bearer private_test_key");
            assert_eq!(body["model"],"fixture-model");assert_eq!(body["max_tokens"],64);
            Json(json!({"content":[{"type":"thinking","thinking":"not returned"},{"type":"text","text":"{\"ok\":true}"}],"usage":{"input_tokens":3,"output_tokens":4}}))
        })).route("/redirect/v1/messages",post(||async{(StatusCode::TEMPORARY_REDIRECT,[("location","http://192.0.2.1/forbidden")]).into_response()}))
          .route("/oversized/v1/messages",post(||async{axum::body::Body::from_stream(futures::stream::iter((0..300).map(|_|Ok::<_,std::io::Error>(vec![b'x';1024]))))}));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .expect("server");
        });
        let configuration = |prefix: &str| {
            json!({"env":{"ANTHROPIC_BASE_URL":format!("http://{address}{prefix}"),"ANTHROPIC_AUTH_TOKEN":"private_test_key","ANTHROPIC_MODEL":"fixture-model"}}).to_string()
        };
        let provider = parse_provider("claude", "fixture", &configuration("")).expect("provider");
        let result = provider
            .complete_json("JSON only", "fixture", 64)
            .await
            .expect("real HTTP result");
        assert_eq!(result.value, json!({"ok":true}));
        assert_eq!(result.input_tokens, Some(3));
        assert_eq!(result.output_tokens, Some(4));
        let redirect =
            parse_provider("claude", "fixture", &configuration("/redirect")).expect("redirect");
        assert!(matches!(
            redirect.complete_json("JSON", "fixture", 64).await,
            Err(ProviderError::Http(307))
        ));
        let oversized =
            parse_provider("claude", "fixture", &configuration("/oversized")).expect("oversized");
        assert!(matches!(
            oversized.complete_json("JSON", "fixture", 64).await,
            Err(ProviderError::ResponseTooLarge)
        ));
        let _ = stop.send(());
        server.await.expect("join");
    }
    #[test]
    fn openai_compatible_base_url_preserves_declared_api_version() {
        for prefix in ["/v1", "/api/coding/v3", "/api/paas/v4", ""] {
            let config=json!({"npm":"@ai-sdk/openai-compatible","options":{"baseURL":format!("https://example.invalid{prefix}"),"apiKey":"fixture_key"},"models":{"fixture-model":{}}}).to_string();
            let provider = parse_provider("opencode", "fixture", &config).expect("provider");
            assert_eq!(
                provider.endpoint.path(),
                format!("{prefix}/chat/completions")
            );
        }
    }
}
