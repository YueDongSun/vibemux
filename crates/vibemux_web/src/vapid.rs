//! VAPID (RFC 8292) application-server identity for self-hosted Web Push.
//!
//! The stored private key is a raw 32-byte P-256 scalar encoded as base64url
//! without padding. Tokens are compact ES256 JWTs whose signatures use
//! RFC 6979 deterministic nonces.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use serde::Serialize;

const PRIVATE_KEY_BYTES: usize = 32;
const MAX_KEY_GENERATION_ATTEMPTS: usize = 64;
/// The JWS header exactly as RFC 8292 §2.4 shows it.
const JWT_HEADER: &str = r#"{"typ":"JWT","alg":"ES256"}"#;

#[derive(Debug, thiserror::Error)]
#[error("vapid_failed")]
pub(crate) struct VapidError;

/// A parsed VAPID signing key and its encoded public key.
pub(crate) struct VapidKey {
    signing_key: SigningKey,
    public_key: String,
}

#[derive(Serialize)]
struct VapidClaims<'a> {
    aud: &'a str,
    exp: u64,
    sub: &'a str,
}

impl VapidKey {
    /// Parse a stored private key; rejects any encoding but 32 canonical bytes.
    pub(crate) fn from_base64(encoded: &str) -> Result<Self, VapidError> {
        let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| VapidError)?;
        if bytes.len() != PRIVATE_KEY_BYTES {
            return Err(VapidError);
        }
        let signing_key = SigningKey::from_slice(&bytes).map_err(|_| VapidError)?;
        let public_key =
            URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_sec1_point(false).as_bytes());
        Ok(Self {
            signing_key,
            public_key,
        })
    }

    /// Generate a new private key in the stored encoding.
    pub(crate) fn generate_base64() -> Result<String, VapidError> {
        for _ in 0..MAX_KEY_GENERATION_ATTEMPTS {
            let mut bytes = [0u8; PRIVATE_KEY_BYTES];
            getrandom::fill(&mut bytes).map_err(|_| VapidError)?;
            // Rejects the zero scalar and values at or above the curve order.
            if SigningKey::from_slice(&bytes).is_ok() {
                return Ok(URL_SAFE_NO_PAD.encode(bytes));
            }
        }
        Err(VapidError)
    }

    /// Uncompressed SEC1 public key, base64url: the browser's
    /// `applicationServerKey` and the RFC 8292 `k` parameter.
    pub(crate) fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Sign a token for one push-service origin (`aud`) until `expires_at`.
    pub(crate) fn sign_token(
        &self,
        audience: &str,
        expires_at: u64,
        subject: &str,
    ) -> Result<String, VapidError> {
        let signing_input = signing_input(audience, expires_at, subject)?;
        let signature: Signature = self
            .signing_key
            .try_sign(signing_input.as_bytes())
            .map_err(|_| VapidError)?;
        // JWS ES256 carries the fixed-size r || s form, not DER.
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }

    /// The RFC 8292 §3 `Authorization` header value for a signed token.
    pub(crate) fn authorization(&self, token: &str) -> String {
        format!("vapid t={token}, k={}", self.public_key)
    }
}

fn signing_input(audience: &str, expires_at: u64, subject: &str) -> Result<String, VapidError> {
    let claims = serde_json::to_vec(&VapidClaims {
        aud: audience,
        exp: expires_at,
        sub: subject,
    })
    .map_err(|_| VapidError)?;
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(JWT_HEADER),
        URL_SAFE_NO_PAD.encode(claims)
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use p256::ecdsa::{VerifyingKey, signature::Verifier};

    // RFC 8292 §2.4, Figures 1 and 2 (line wrapping removed).
    pub(crate) const RFC8292_AUDIENCE: &str = "https://push.example.net";
    pub(crate) const RFC8292_EXPIRES_AT: u64 = 1_453_523_768;
    pub(crate) const RFC8292_SUBJECT: &str = "mailto:push@example.com";
    const RFC8292_TOKEN: &str = "eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9.eyJhdWQiOiJodHRwczovL3B1c2guZXhhbXBsZS5uZXQiLCJleHAiOjE0NTM1MjM3NjgsInN1YiI6Im1haWx0bzpwdXNoQGV4YW1wbGUuY29tIn0.i3CYb7t4xfxCDquptFOepC9GAu_HLGkMlMuCGSK2rpiUfnK9ojFwDXb1JrErtmysazNjjvW2L9OkSSHzvoD1oA";
    const RFC8292_PUBLIC_KEY: &str =
        "BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dRuU_RCPCfA5aq9ojSwk5Y2EmClBPs";

    // Fixed test key: base64url(SHA-256("vibemux_web vapid test vector")). The
    // expected token was computed independently with OpenSSL 3.5.4
    // (`pkeyutl -sign -rawin -digest sha256 -pkeyopt nonce-type:1`, RFC 6979)
    // over the RFC 8292 example claims.
    pub(crate) const TEST_PRIVATE_KEY: &str = "4IGcqhOV90PWkjo2FJt40kU6Vu-ZRvfrHyCXV5OAzrs";
    pub(crate) const TEST_PUBLIC_KEY: &str =
        "BP7BuO3V_rMQcZ8i2_VrC81Gg6pDUiS8Tl7j9snTNq6PJeZQKeN3LvKwaXAMPM7B41CEoNqbKrQhsTxkQi2arg0";
    pub(crate) const TEST_TOKEN: &str = "eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9.eyJhdWQiOiJodHRwczovL3B1c2guZXhhbXBsZS5uZXQiLCJleHAiOjE0NTM1MjM3NjgsInN1YiI6Im1haWx0bzpwdXNoQGV4YW1wbGUuY29tIn0.aqAVFZru5Ut1_9UVOa8iwIUNbLG7QNGh6E2mLjvNF7oU__ABh70ctrkKrvj_Uzo-hGgzHbemLampyjHttBlyow";

    #[test]
    fn token_layout_and_signature_encoding_match_the_rfc8292_example() {
        let (rfc_signing_input, rfc_signature) = RFC8292_TOKEN
            .rsplit_once('.')
            .expect("RFC token has three segments");
        assert_eq!(
            signing_input(RFC8292_AUDIENCE, RFC8292_EXPIRES_AT, RFC8292_SUBJECT)
                .expect("signing input"),
            rfc_signing_input
        );
        let verifying_key = VerifyingKey::from_sec1_bytes(
            &URL_SAFE_NO_PAD
                .decode(RFC8292_PUBLIC_KEY)
                .expect("RFC key is base64url"),
        )
        .expect("RFC key is a P-256 point");
        let signature = Signature::from_slice(
            &URL_SAFE_NO_PAD
                .decode(rfc_signature)
                .expect("RFC signature is base64url"),
        )
        .expect("RFC signature is raw r || s");
        verifying_key
            .verify(rfc_signing_input.as_bytes(), &signature)
            .expect("RFC 8292 example signature verifies");
    }

    #[test]
    fn deterministic_token_matches_an_independent_rfc6979_signature() {
        let key = VapidKey::from_base64(TEST_PRIVATE_KEY).expect("test key");
        assert_eq!(key.public_key(), TEST_PUBLIC_KEY);
        let token = key
            .sign_token(RFC8292_AUDIENCE, RFC8292_EXPIRES_AT, RFC8292_SUBJECT)
            .expect("token");
        assert_eq!(token, TEST_TOKEN);
        assert_eq!(
            key.authorization(&token),
            format!("vapid t={TEST_TOKEN}, k={TEST_PUBLIC_KEY}")
        );
    }

    #[test]
    fn stored_private_keys_must_be_canonical_nonzero_scalars() {
        for invalid in [
            String::new(),
            "not base64url!".to_owned(),
            format!("{TEST_PRIVATE_KEY}="),
            URL_SAFE_NO_PAD.encode([7u8; 31]),
            URL_SAFE_NO_PAD.encode([7u8; 33]),
            URL_SAFE_NO_PAD.encode([0u8; 32]),
            URL_SAFE_NO_PAD.encode([0xffu8; 32]),
        ] {
            assert!(
                VapidKey::from_base64(&invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        let generated = VapidKey::generate_base64().expect("generate key");
        assert_eq!(generated.len(), 43);
        let key = VapidKey::from_base64(&generated).expect("generated key parses");
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(key.public_key())
                .expect("public key is base64url")
                .first(),
            Some(&4)
        );
        assert_ne!(generated, VapidKey::generate_base64().expect("second key"));
    }
}
