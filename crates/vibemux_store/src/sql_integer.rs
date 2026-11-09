//! SQLite INTEGER conversions for the store's domain-facing `u64` values.
//!
//! rusqlite 0.41 removed its `u64` conversions because SQLite stores INTEGER
//! as a signed 64-bit value. Domain-facing rows keep `u64`, so every
//! conversion happens at this boundary and fails closed instead of wrapping.

use rusqlite::{Error, Result, types::Type};

#[derive(Debug, thiserror::Error)]
#[error("sqlite INTEGER out of range: {value}")]
struct IntegerOutOfRange {
    value: String,
}

/// Converts a domain `u64` into the signed value SQLite stores.
pub(crate) fn to_sql_u64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        Error::ToSqlConversionFailure(Box::new(IntegerOutOfRange {
            value: value.to_string(),
        }))
    })
}

/// Converts a stored SQLite INTEGER back into the domain's `u64`.
pub(crate) fn from_sql_u64(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        Error::FromSqlConversionFailure(
            0,
            Type::Integer,
            Box::new(IntegerOutOfRange {
                value: value.to_string(),
            }),
        )
    })
}
