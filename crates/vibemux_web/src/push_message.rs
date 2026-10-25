//! One RFC 8030 push-message request: an RFC 8291 `aes128gcm` body encrypted
//! for the subscription, and an RFC 8292 VAPID `Authorization` header bound
//! to the push service's origin. Transport stays with the caller.

use crate::vapid::VapidKey;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use url::Url;

/// Seconds a push service may hold an undelivered message.
const PUSH_TTL_SECONDS: u32 = 300;
/// RFC 8030 §5.3 urgency: state changes are not time-critical.
const PUSH_URGENCY: &str = "low";
/// VAPID token lifetime; RFC 8292 §2 allows at most 24 hours.
const VAPID_TOKEN_LIFETIME_SECONDS: u64 = 12 * 60 * 60;
const _: () = assert!(VAPID_TOKEN_LIFETIME_SECONDS <= 24 * 60 * 60);
const CONTENT_ENCODING: &str = "aes128gcm";
const CONTENT_TYPE: &str = "application/octet-stream";

#[derive(Debug, thiserror::Error)]
#[error("push_message_failed")]
pub(crate) struct PushMessageError;

/// Headers and encrypted body for one POST to a push resource.
pub(crate) struct PushRequest {
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: Vec<u8>,
}

/// Encrypt `payload` for the subscription keys and sign for `endpoint`'s origin.
///
/// `p256dh` and `auth` are the subscription's base64url keys. Encryption
/// draws a fresh salt and ephemeral key pair on every call.
pub(crate) fn build_push_request(
    endpoint: &Url,
    p256dh: &str,
    auth: &str,
    payload: &[u8],
    vapid_key: &VapidKey,
    subject: &str,
    now_seconds: u64,
) -> Result<PushRequest, PushMessageError> {
    let user_agent_public_key = URL_SAFE_NO_PAD
        .decode(p256dh)
        .map_err(|_| PushMessageError)?;
    let auth_secret = URL_SAFE_NO_PAD.decode(auth).map_err(|_| PushMessageError)?;
    // ece pads the record to a multiple of 128 bytes before encrypting.
    let body = ece::encrypt(&user_agent_public_key, &auth_secret, payload)
        .map_err(|_| PushMessageError)?;
    let audience = push_service_origin(endpoint)?;
    let expires_at = now_seconds
        .checked_add(VAPID_TOKEN_LIFETIME_SECONDS)
        .ok_or(PushMessageError)?;
    let token = vapid_key
        .sign_token(&audience, expires_at, subject)
        .map_err(|_| PushMessageError)?;
    Ok(PushRequest {
        headers: vec![
            ("TTL", PUSH_TTL_SECONDS.to_string()),
            ("Urgency", PUSH_URGENCY.to_owned()),
            ("Content-Encoding", CONTENT_ENCODING.to_owned()),
            ("Content-Length", body.len().to_string()),
            ("Content-Type", CONTENT_TYPE.to_owned()),
            ("Authorization", vapid_key.authorization(&token)),
        ],
        body,
    })
}

/// RFC 8292 §2 `aud`: the serialized origin of the push resource URL.
fn push_service_origin(endpoint: &Url) -> Result<String, PushMessageError> {
    let origin = endpoint.origin();
    if !origin.is_tuple() {
        return Err(PushMessageError);
    }
    Ok(origin.ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vapid::tests::{
        RFC8292_AUDIENCE, RFC8292_EXPIRES_AT, RFC8292_SUBJECT, TEST_PRIVATE_KEY, TEST_PUBLIC_KEY,
        TEST_TOKEN,
    };
    use ece::EcKeyComponents;

    // RFC 8291 §5 and Appendix A (line wrapping removed).
    const RFC8291_PLAINTEXT: &[u8] = b"When I grow up, I want to be a watermelon";
    const RFC8291_AUTH_SECRET: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const RFC8291_UA_PRIVATE_KEY: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const RFC8291_UA_PUBLIC_KEY: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const RFC8291_BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";
    const RFC8291_SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const RFC8291_CIPHERTEXT: &str =
        "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ";
    const RFC8291_AS_PUBLIC_KEY: &str =
        "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const RFC8291_RECORD_SIZE: u32 = 4096;
    /// salt (16) || rs (4) || idlen (1) || keyid (65): RFC 8291 §4.
    const HEADER_BYTES: usize = 86;
    const AEAD_TAG_BYTES: usize = 16;
    const PAYLOAD: &[u8] = br#"{"kind":"task","id":"task_1","state":"in_progress"}"#;

    fn decode(value: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD
            .decode(value)
            .expect("test vector is base64url")
    }

    fn user_agent_keys() -> EcKeyComponents {
        EcKeyComponents::new(
            decode(RFC8291_UA_PRIVATE_KEY),
            decode(RFC8291_UA_PUBLIC_KEY),
        )
    }

    fn rfc8292_request(payload: &[u8]) -> PushRequest {
        let endpoint = Url::parse("https://push.example.net/push/JzLQ3raZJfFBR0aqvOMsLrt54w4rJUsV")
            .expect("endpoint");
        build_push_request(
            &endpoint,
            RFC8291_UA_PUBLIC_KEY,
            RFC8291_AUTH_SECRET,
            payload,
            &VapidKey::from_base64(TEST_PRIVATE_KEY).expect("test key"),
            RFC8292_SUBJECT,
            // 12 hours before the RFC 8292 example expiry, pinning the lifetime.
            1_453_480_568,
        )
        .expect("push request")
    }

    #[test]
    fn linked_ece_backend_decrypts_the_rfc8291_example_message() {
        let body = decode(RFC8291_BODY);
        // 86-byte header + 41-byte plaintext + delimiter + tag. (The §5 sample
        // request says `Content-Length: 145`, which its own body contradicts.)
        assert_eq!(body.len(), 144);
        assert_eq!(&body[..16], decode(RFC8291_SALT).as_slice());
        assert_eq!(
            &body[21..HEADER_BYTES],
            decode(RFC8291_AS_PUBLIC_KEY).as_slice()
        );
        assert_eq!(&body[HEADER_BYTES..], decode(RFC8291_CIPHERTEXT).as_slice());
        let plaintext = ece::decrypt(&user_agent_keys(), &decode(RFC8291_AUTH_SECRET), &body)
            .expect("RFC 8291 example decrypts");
        assert_eq!(plaintext, RFC8291_PLAINTEXT);
    }

    #[test]
    fn push_request_headers_and_vapid_token_are_pinned() {
        let request = rfc8292_request(PAYLOAD);
        assert_eq!(
            request.headers,
            vec![
                ("TTL", "300".to_owned()),
                ("Urgency", "low".to_owned()),
                ("Content-Encoding", "aes128gcm".to_owned()),
                ("Content-Length", "230".to_owned()),
                ("Content-Type", "application/octet-stream".to_owned()),
                (
                    "Authorization",
                    format!("vapid t={TEST_TOKEN}, k={TEST_PUBLIC_KEY}")
                ),
            ]
        );
    }

    #[test]
    fn push_request_body_is_one_padded_rfc8291_record_for_the_subscription() {
        for (payload, padded_record_bytes) in [(PAYLOAD.to_vec(), 128), (vec![b'x'; 1024], 1152)] {
            let request = rfc8292_request(&payload);
            assert!(
                request
                    .headers
                    .contains(&("Content-Length", request.body.len().to_string()))
            );
            let body = request.body;
            assert_eq!(
                body.len(),
                HEADER_BYTES + padded_record_bytes + AEAD_TAG_BYTES
            );
            assert_eq!(
                u32::from_be_bytes(body[16..20].try_into().expect("record size")),
                RFC8291_RECORD_SIZE
            );
            assert_eq!(body[20], 65);
            let sender_key = &body[21..HEADER_BYTES];
            p256::ecdsa::VerifyingKey::from_sec1_bytes(sender_key)
                .expect("keyid is an uncompressed P-256 point");
            // RFC 8292 §3.2: the key-exchange key differs from the signing key.
            assert_ne!(sender_key, decode(TEST_PUBLIC_KEY).as_slice());
            let plaintext = ece::decrypt(&user_agent_keys(), &decode(RFC8291_AUTH_SECRET), &body)
                .expect("the subscription's private key decrypts the body");
            assert_eq!(plaintext, payload);
        }
        let first = rfc8292_request(PAYLOAD).body;
        let second = rfc8292_request(PAYLOAD).body;
        assert_ne!(first[..16], second[..16], "salt must be fresh");
        assert_ne!(
            first[21..HEADER_BYTES],
            second[21..HEADER_BYTES],
            "ephemeral key must be fresh"
        );
    }

    #[test]
    fn audience_is_the_endpoint_origin() {
        for (endpoint, audience) in [
            ("https://push.example.net/push/opaque", RFC8292_AUDIENCE),
            ("https://PUSH.example.net:443/push/opaque", RFC8292_AUDIENCE),
            // Subscription validation accepts a trailing-dot host; the origin keeps it.
            (
                "https://push.example.net./other",
                "https://push.example.net.",
            ),
        ] {
            assert_eq!(
                push_service_origin(&Url::parse(endpoint).expect("endpoint"))
                    .expect("tuple origin"),
                audience,
                "{endpoint}"
            );
        }
        assert!(push_service_origin(&Url::parse("data:text/plain,x").expect("url")).is_err());
    }

    #[test]
    fn malformed_subscription_keys_are_rejected() {
        let endpoint = Url::parse("https://push.example.net/push/opaque").expect("endpoint");
        let key = VapidKey::from_base64(TEST_PRIVATE_KEY).expect("test key");
        let mut not_a_point = vec![4u8];
        not_a_point.extend([0u8; 64]);
        for (p256dh, auth) in [
            (
                URL_SAFE_NO_PAD.encode(not_a_point),
                RFC8291_AUTH_SECRET.to_owned(),
            ),
            ("not base64url!".to_owned(), RFC8291_AUTH_SECRET.to_owned()),
            (
                RFC8291_UA_PUBLIC_KEY.to_owned(),
                "not base64url!".to_owned(),
            ),
        ] {
            assert!(
                build_push_request(
                    &endpoint,
                    &p256dh,
                    &auth,
                    PAYLOAD,
                    &key,
                    RFC8292_SUBJECT,
                    RFC8292_EXPIRES_AT,
                )
                .is_err(),
                "accepted {p256dh} / {auth}"
            );
        }
    }
}
