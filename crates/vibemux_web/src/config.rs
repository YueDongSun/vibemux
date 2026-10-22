//! Explicit fail-closed configuration. Secrets are references, never config values.
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

pub const MAX_CONFIG_BYTES: usize = 32 * 1024;
pub const MAX_SECRET_BYTES: usize = 128;
pub const DEFAULT_POLL_SECONDS: u64 = 5;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    pub schema_version: u32,
    #[serde(default = "default_bind")]
    pub bind_address: SocketAddr,
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
    pub instance_token: SecretReference,
    pub enrollment_url: String,
    pub push_allowed_hosts: Vec<String>,
    pub vapid_subject: String,
    #[serde(default)]
    pub task_links: Vec<crate::control_bridge::TrackedTaskLinks>,
    /// Serve the page and its static assets without the token to requests
    /// that name the loopback listener literally; the API keeps requiring
    /// it. Off unless set; see `local_page`.
    #[serde(default)]
    pub local_page_without_token: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretReference {
    pub env_var: Option<String>,
    pub secret_file: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
#[error("web_configuration_invalid")]
pub struct ConfigError;

fn default_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8420))
}
fn default_poll() -> u64 {
    DEFAULT_POLL_SECONDS
}

impl WebConfig {
    pub fn load(project_root: &Path) -> Result<Option<Self>, ConfigError> {
        let path = project_root.join(".vibemux/web_config.json");
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ConfigError),
            Ok(_) => {}
        }
        let bytes =
            crate::private_state::read_bounded(&path, MAX_CONFIG_BYTES).map_err(|_| ConfigError)?;
        let config: Self = serde_json::from_slice(&bytes).map_err(|_| ConfigError)?;
        config.validate()?;
        Ok(Some(config))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != 1
            || !self.bind_address.ip().is_loopback()
            || !(1..=300).contains(&self.poll_seconds)
            || self.push_allowed_hosts.len() > 16
            || self.push_allowed_hosts.is_empty()
        {
            return Err(ConfigError);
        }
        crate::control_bridge::normalize_task_links(self.task_links.clone())
            .map_err(|_| ConfigError)?;
        let enrollment_url = url::Url::parse(&self.enrollment_url).map_err(|_| ConfigError)?;
        if enrollment_url.scheme() != "https"
            || enrollment_url.host_str().is_none()
            || !enrollment_url.username().is_empty()
            || enrollment_url.password().is_some()
            || enrollment_url.query().is_some()
            || enrollment_url.fragment().is_some()
            || enrollment_url.path() != "/"
            || enrollment_url.port().is_some_and(|p| p != 443)
            || self.enrollment_url.len() > 2048
        {
            return Err(ConfigError);
        }
        for host in &self.push_allowed_hosts {
            if host.len() > 253
                || host.contains('*')
                || host.to_ascii_lowercase() != *host
                || !host.contains('.')
                || host.split('.').any(|label| {
                    label.is_empty()
                        || label.starts_with('-')
                        || label.ends_with('-')
                        || !label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
                || host.parse::<std::net::IpAddr>().is_ok()
            {
                return Err(ConfigError);
            }
        }
        if !self.vapid_subject.starts_with("mailto:")
            || self.vapid_subject.len() > 254
            || !self.vapid_subject.contains('@')
            || self.vapid_subject.chars().any(char::is_whitespace)
            || self.instance_token.env_var.is_some() == self.instance_token.secret_file.is_some()
            || self.instance_token.env_var.as_ref().is_some_and(|n| {
                n.is_empty()
                    || n.len() > 128
                    || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            || self
                .instance_token
                .secret_file
                .as_ref()
                .is_some_and(|p| !p.is_absolute())
        {
            return Err(ConfigError);
        }
        Ok(())
    }
}

impl SecretReference {
    pub fn resolve(&self) -> Result<String, ConfigError> {
        let token = match (&self.env_var, &self.secret_file) {
            (Some(name), None) => std::env::var(name).map_err(|_| ConfigError)?,
            (None, Some(path)) => {
                crate::private_state::verify_private_file(path).map_err(|_| ConfigError)?;
                String::from_utf8(
                    crate::private_state::read_bounded(path, MAX_SECRET_BYTES)
                        .map_err(|_| ConfigError)?,
                )
                .map_err(|_| ConfigError)?
            }
            _ => return Err(ConfigError),
        };
        let token = token.trim().to_owned();
        // A 256-bit random token encoded as base64url is 43 characters.
        if !(43..=MAX_SECRET_BYTES).contains(&token.len())
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ConfigError);
        }
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_is_explicit_bounded_and_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        assert!(WebConfig::load(temp.path()).unwrap().is_none());
        std::fs::create_dir(temp.path().join(".vibemux")).unwrap();
        let config_path = temp.path().join(".vibemux/web_config.json");
        let valid = serde_json::json!({"schema_version":1,"instance_token":{"env_var":"WEB_TOKEN"},"enrollment_url":"https://operator.example/","push_allowed_hosts":["fcm.googleapis.com"],"vapid_subject":"mailto:operator@example.com"});
        std::fs::write(&config_path, serde_json::to_vec(&valid).unwrap()).unwrap();
        let loaded = WebConfig::load(temp.path()).unwrap().unwrap();
        assert!(!loaded.local_page_without_token);
        let mut local_page = valid.clone();
        local_page["local_page_without_token"] = serde_json::json!(true);
        std::fs::write(&config_path, serde_json::to_vec(&local_page).unwrap()).unwrap();
        assert!(
            WebConfig::load(temp.path())
                .unwrap()
                .unwrap()
                .local_page_without_token
        );
        for (key, value) in [
            ("schema_version", serde_json::json!(2)),
            ("bind_address", serde_json::json!("0.0.0.0:8420")),
            ("poll_seconds", serde_json::json!(0)),
            (
                "enrollment_url",
                serde_json::json!("https://operator.example/?token=bad"),
            ),
            ("push_allowed_hosts", serde_json::json!(["*.example.com"])),
            ("local_page_without_token", serde_json::json!("yes")),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            std::fs::write(&config_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(WebConfig::load(temp.path()).is_err(), "{key}");
        }
        std::fs::write(&config_path, vec![b' '; MAX_CONFIG_BYTES + 1]).unwrap();
        assert!(WebConfig::load(temp.path()).is_err());
    }
}
