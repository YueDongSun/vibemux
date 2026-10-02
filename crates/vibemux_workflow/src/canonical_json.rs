//! VibeMux canonical JSON, version 1.
//!
//! Contract identities hash these bytes, so the encoding is fixed here
//! rather than delegated to a serializer's formatting choices:
//!
//! - no insignificant whitespace;
//! - object members sorted by the UTF-8 bytes of their keys;
//! - numbers must be integers representable as `i64` or `u64`; fractions
//!   and exponents are rejected rather than rounded;
//! - strings are UTF-8 with exactly these escapes: `\"`, `\\`, `\b`, `\f`,
//!   `\n`, `\r`, `\t`, and `\u00xx` (lowercase hex) for other control
//!   characters below U+0020; every other character is emitted as is, so
//!   Unicode literals are preserved byte for byte;
//! - nesting deeper than [`MAX_CANONICAL_DEPTH`] is rejected.

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::Sha256Digest;

pub const MAX_CANONICAL_DEPTH: usize = 32;
pub const CANONICAL_JSON_VERSION: &str = "vibemux_canonical_json_v1";

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CanonicalError {
    #[error("canonical JSON allows integer numbers only")]
    NonIntegerNumber,
    #[error("canonical JSON nesting is too deep")]
    TooDeep,
    #[error("value could not be converted to JSON")]
    Unserializable,
}

impl CanonicalError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NonIntegerNumber => "workflow_canonical_non_integer",
            Self::TooDeep => "workflow_canonical_too_deep",
            Self::Unserializable => "workflow_canonical_unserializable",
        }
    }
}

/// Canonical bytes of any serializable value.
pub fn to_canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, CanonicalError> {
    let value = serde_json::to_value(value).map_err(|_| CanonicalError::Unserializable)?;
    canonical_bytes(&value)
}

/// Canonical bytes of a JSON value.
pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut output = Vec::new();
    write_value(value, 0, &mut output)?;
    Ok(output)
}

/// Domain-separated digest of the canonical bytes.
pub fn canonical_digest(
    domain: &str,
    value: &impl Serialize,
) -> Result<Sha256Digest, CanonicalError> {
    let bytes = to_canonical_bytes(value)?;
    Ok(Sha256Digest::of_fields(
        domain,
        &[CANONICAL_JSON_VERSION.as_bytes(), &bytes],
    ))
}

fn write_value(value: &Value, depth: usize, output: &mut Vec<u8>) -> Result<(), CanonicalError> {
    if depth > MAX_CANONICAL_DEPTH {
        return Err(CanonicalError::TooDeep);
    }
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                output.extend_from_slice(integer.to_string().as_bytes());
            } else if let Some(integer) = number.as_u64() {
                output.extend_from_slice(integer.to_string().as_bytes());
            } else {
                return Err(CanonicalError::NonIntegerNumber);
            }
        }
        Value::String(text) => write_string(text, output),
        Value::Array(items) => {
            output.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_value(item, depth + 1, output)?;
            }
            output.push(b']');
        }
        Value::Object(members) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            output.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_string(key, output);
                output.push(b':');
                if let Some(member) = members.get(key) {
                    write_value(member, depth + 1, output)?;
                }
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn write_string(text: &str, output: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push(b'"');
    for character in text.chars() {
        match character {
            '"' => output.extend_from_slice(b"\\\""),
            '\\' => output.extend_from_slice(b"\\\\"),
            '\u{8}' => output.extend_from_slice(b"\\b"),
            '\u{c}' => output.extend_from_slice(b"\\f"),
            '\n' => output.extend_from_slice(b"\\n"),
            '\r' => output.extend_from_slice(b"\\r"),
            '\t' => output.extend_from_slice(b"\\t"),
            control if u32::from(control) < 0x20 => {
                let code = u32::from(control) as usize;
                output.extend_from_slice(b"\\u00");
                output.push(HEX[code >> 4]);
                output.push(HEX[code & 0x0f]);
            }
            other => {
                let mut buffer = [0_u8; 4];
                output.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    output.push(b'"');
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    #[test]
    fn keys_are_sorted_and_whitespace_removed() {
        let value = json!({"b": [1, {"z": true, "a": null}], "a": "x"});
        assert_eq!(
            canonical_bytes(&value).expect("canonical"),
            br#"{"a":"x","b":[1,{"a":null,"z":true}]}"#.to_vec()
        );
    }

    #[test]
    fn unicode_and_markup_are_preserved_and_controls_escaped() {
        let value = json!({"title": "任务 ✓ <img src=x onerror=1> @path /cmd", "c": "\u{1}\n\"\\"});
        let bytes = canonical_bytes(&value).expect("canonical");
        let text = String::from_utf8(bytes).expect("utf8");
        assert_eq!(
            text,
            "{\"c\":\"\\u0001\\n\\\"\\\\\",\"title\":\"任务 ✓ <img src=x onerror=1> @path /cmd\"}"
        );
    }

    #[test]
    fn fractions_and_excess_depth_are_rejected() {
        assert_eq!(
            canonical_bytes(&json!({"n": 1.5})),
            Err(CanonicalError::NonIntegerNumber)
        );
        let mut deep = json!(0);
        for _ in 0..=MAX_CANONICAL_DEPTH + 1 {
            deep = json!([deep]);
        }
        assert_eq!(canonical_bytes(&deep), Err(CanonicalError::TooDeep));
        assert!(canonical_bytes(&json!({"big": u64::MAX, "neg": i64::MIN})).is_ok());
    }

    #[test]
    fn digest_is_domain_separated() {
        let value = json!({"a": 1});
        assert_ne!(
            canonical_digest("first", &value).expect("digest"),
            canonical_digest("second", &value).expect("digest")
        );
    }

    proptest! {
        #[test]
        fn canonical_bytes_round_trip_and_are_stable(
            entries in proptest::collection::btree_map(".{0,12}", ".{0,24}", 0..8)
        ) {
            let value = serde_json::to_value(&entries).expect("value");
            let first = canonical_bytes(&value).expect("canonical");
            let reparsed: Value = serde_json::from_slice(&first).expect("parse canonical");
            prop_assert_eq!(&reparsed, &value);
            prop_assert_eq!(canonical_bytes(&reparsed).expect("canonical again"), first);
        }
    }
}
