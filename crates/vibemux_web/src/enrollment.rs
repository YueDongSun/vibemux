//! Bounded one-time secrets, with monotonic expiration and no secret Debug output.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

pub const ENROLLMENT_TTL: Duration = Duration::from_secs(120);
pub const MAX_ENROLLMENTS: usize = 8;
pub const FAILED_ATTEMPT_LIMIT: u32 = 8;
pub const ATTEMPT_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
#[error("web_enrollment_rejected")]
pub struct EnrollmentError;

pub fn random_token() -> Result<String, EnrollmentError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| EnrollmentError)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[derive(Serialize)]
pub struct EnrollmentInvitation {
    pub token: String,
    pub expires_in_seconds: u64,
    pub qr_svg: String,
    pub enrollment_url: String,
}
#[derive(Default)]
pub struct EnrollmentStore {
    entries: BTreeMap<String, Instant>,
    limiter: AttemptLimiter,
}
impl EnrollmentStore {
    pub fn issue(
        &mut self,
        enrollment_url: &str,
        now: Instant,
    ) -> Result<EnrollmentInvitation, EnrollmentError> {
        self.entries.retain(|_, expiry| *expiry > now);
        if self.entries.len() >= MAX_ENROLLMENTS {
            return Err(EnrollmentError);
        }
        let token = random_token()?;
        let payload = serde_json::json!({"url":enrollment_url,"token":token});
        let qr = qrcode::QrCode::new(payload.to_string()).map_err(|_| EnrollmentError)?;
        let qr_svg = qr
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(256, 256)
            .build();
        self.entries
            .insert(token_digest(&token), now + ENROLLMENT_TTL);
        Ok(EnrollmentInvitation {
            token,
            expires_in_seconds: ENROLLMENT_TTL.as_secs(),
            qr_svg,
            enrollment_url: enrollment_url.to_owned(),
        })
    }
    pub fn redeem(&mut self, token: &str, now: Instant) -> Result<(), EnrollmentError> {
        if !self.limiter.permitted(now) {
            return Err(EnrollmentError);
        }
        let digest = token_digest(token);
        let found = self
            .entries
            .keys()
            .find(|key| bool::from(key.as_bytes().ct_eq(digest.as_bytes())))
            .cloned();
        let valid = found
            .and_then(|key| self.entries.remove(&key))
            .is_some_and(|expiry| expiry > now);
        self.entries.retain(|_, expiry| *expiry > now);
        if valid {
            Ok(())
        } else {
            self.limiter.failed(now);
            Err(EnrollmentError)
        }
    }
}
fn token_digest(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub struct AttemptLimiter {
    window: Instant,
    failures: u32,
}
impl Default for AttemptLimiter {
    fn default() -> Self {
        Self {
            window: Instant::now(),
            failures: 0,
        }
    }
}
impl AttemptLimiter {
    fn reset_if_elapsed(&mut self, now: Instant) {
        if now.saturating_duration_since(self.window) >= ATTEMPT_WINDOW {
            self.window = now;
            self.failures = 0;
        }
    }
    pub fn permitted(&mut self, now: Instant) -> bool {
        self.reset_if_elapsed(now);
        self.failures < FAILED_ATTEMPT_LIMIT
    }
    pub fn failed(&mut self, now: Instant) {
        self.reset_if_elapsed(now);
        self.failures = self.failures.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokens_are_random_short_lived_and_single_use() {
        let now = Instant::now();
        let mut store = EnrollmentStore::default();
        let first = store.issue("https://operator.example/", now).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(&first.token).unwrap().len(), 32);
        assert!(!first.qr_svg.contains("?token="));
        store.redeem(&first.token, now).unwrap();
        assert!(store.redeem(&first.token, now).is_err());
        let expired = store.issue("https://operator.example/", now).unwrap();
        assert!(store.redeem(&expired.token, now + ENROLLMENT_TTL).is_err());
        assert!(
            store
                .redeem(
                    &expired.token,
                    now + ENROLLMENT_TTL + Duration::from_secs(1)
                )
                .is_err()
        );
        assert_ne!(first.token, expired.token);
    }
    #[test]
    fn failed_attempts_and_capacity_are_bounded() {
        let mut store = EnrollmentStore::default();
        let now = Instant::now();
        let token = store.issue("https://operator.example/", now).unwrap().token;
        for _ in 0..FAILED_ATTEMPT_LIMIT {
            assert!(store.redeem("invalid", now).is_err());
        }
        assert!(store.redeem(&token, now).is_err());
        assert!(store.redeem(&token, now + ATTEMPT_WINDOW).is_ok());
        for _ in 0..MAX_ENROLLMENTS {
            store.issue("https://operator.example/", now).unwrap();
        }
        assert!(store.issue("https://operator.example/", now).is_err());
    }
}
