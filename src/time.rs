//! Time helpers shared across providers.

use chrono::{DateTime, TimeZone, Utc};

/// Parse an RFC 3339 / ISO-8601 timestamp.
pub fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Convert epoch milliseconds to UTC time.
pub fn from_epoch_ms(ms: i64) -> Option<DateTime<Utc>> {
    let secs = ms.div_euclid(1000);
    let sub_ms = ms.rem_euclid(1000) as u32;
    Utc.timestamp_opt(secs, sub_ms * 1_000_000).single()
}

/// Convert epoch seconds to UTC time.
pub fn from_epoch_s(s: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(s, 0).single()
}

/// Format a UTC time as RFC 3339 with millisecond precision and trailing 'Z'.
pub fn to_rfc3339_ms(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
