//! SHA-256 digests used as content-free identities: prompt, operator config,
//! request fingerprint, reservation key, and transcript.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

const HEX: &[u8; 16] = b"0123456789abcdef";

/// A SHA-256 value that serializes as 64 lowercase hexadecimal characters.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self::from_hasher(Sha256::new_with_prefix(bytes))
    }

    pub(crate) fn from_hasher(hasher: Sha256) -> Self {
        let mut value = [0_u8; 32];
        value.copy_from_slice(&hasher.finalize());
        Self(value)
    }

    /// Parses exactly 64 lowercase hexadecimal characters.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let mut value = [0_u8; 32];
        for (index, pair) in bytes.chunks_exact(2).enumerate() {
            value[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Some(Self(value))
    }

    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut text = String::with_capacity(64);
        for byte in self.0 {
            text.push(char::from(HEX[usize::from(byte >> 4)]));
            text.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        text
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Domain-separated hash over length-prefixed fields, so no concatenation of
/// different field values can collide with another.
pub(crate) fn hash_fields(domain: &str, fields: &[&[u8]]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    for field in fields {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    Sha256Digest::from_hasher(hasher)
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Sha256Digest({})", self.to_hex())
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_hex(&text).ok_or_else(|| de::Error::custom("invalid sha256 digest"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_matches_known_vector_and_round_trips() {
        let digest = Sha256Digest::of(b"abc");
        assert_eq!(
            digest.to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(Sha256Digest::from_hex(&digest.to_hex()), Some(digest));
        let encoded = serde_json::to_string(&digest).expect("encode digest");
        assert_eq!(
            serde_json::from_str::<Sha256Digest>(&encoded).expect("decode digest"),
            digest
        );
    }

    #[test]
    fn hex_parsing_rejects_uppercase_short_and_non_hex_text() {
        let upper = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
        assert_eq!(Sha256Digest::from_hex(upper), None);
        assert_eq!(Sha256Digest::from_hex("ab"), None);
        assert_eq!(Sha256Digest::from_hex(&"g".repeat(64)), None);
    }

    #[test]
    fn length_prefixed_fields_do_not_collide_across_boundaries() {
        assert_ne!(
            hash_fields("domain", &[b"ab", b"c"]),
            hash_fields("domain", &[b"a", b"bc"])
        );
        assert_ne!(
            hash_fields("first", &[b"value"]),
            hash_fields("second", &[b"value"])
        );
    }
}
