//! ID synthesis helpers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// Generate a fresh UUID v7 (time-ordered).
pub fn new_uuid_v7() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Derive a deterministic stable id from a seed string. Used when an upstream
/// record has no id of its own — same input always produces same output.
pub fn synth_id(seed: &str) -> String {
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    let bytes = h.finalize();
    // 16-hex prefix is plenty for in-session uniqueness.
    let hex: String = bytes.iter().take(8).map(|b| format!("{:02x}", b)).collect();
    format!("ut_{}", hex)
}

static OPENCODE_LAST_MS: AtomicU64 = AtomicU64::new(0);
static OPENCODE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate an OpenCode-style monotonic identifier body.
///
/// OpenCode's JS `Identifier.create(descending)` uses the low 48 bits of
/// `(unix_ms * 0x1000 + counter)` as a 12-hex timestamp/counter prefix, then
/// appends 14 random base62 characters. `SessionID.descending()` in OpenCode
/// uses the bitwise-inverted prefix; message/part/event ids use ascending.
pub fn opencode_identifier(descending: bool) -> String {
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    let last = OPENCODE_LAST_MS.load(Ordering::SeqCst);
    let counter = if last == unix_ms {
        OPENCODE_COUNTER
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1)
    } else {
        OPENCODE_LAST_MS.store(unix_ms, Ordering::SeqCst);
        OPENCODE_COUNTER.store(1, Ordering::SeqCst);
        1
    };
    let mask = 0xffff_ffff_ffffu64;
    let mut prefix = unix_ms.wrapping_mul(0x1000).wrapping_add(counter) & mask;
    if descending {
        prefix = (!prefix) & mask;
    }
    format!("{prefix:012x}{}", random_base62_from_uuid(14))
}

pub fn opencode_session_id() -> String {
    format!("ses_{}", opencode_identifier(true))
}

pub fn opencode_message_id() -> String {
    format!("msg_{}", opencode_identifier(false))
}

pub fn opencode_part_id() -> String {
    format!("prt_{}", opencode_identifier(false))
}

pub fn opencode_event_id() -> String {
    format!("evt_{}", opencode_identifier(false))
}

fn random_base62_from_uuid(len: usize) -> String {
    const CHARS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let uuid = uuid::Uuid::now_v7();
        for byte in uuid.as_bytes() {
            if out.len() >= len {
                break;
            }
            out.push(CHARS[*byte as usize % CHARS.len()] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_stable() {
        assert_eq!(synth_id("hello"), synth_id("hello"));
        assert_ne!(synth_id("hello"), synth_id("world"));
    }

    #[test]
    fn uuid_unique() {
        let a = new_uuid_v7();
        let b = new_uuid_v7();
        assert_ne!(a, b);
    }

    #[test]
    fn opencode_native_ids_have_expected_prefixes_and_lengths() {
        let session = opencode_session_id();
        let message = opencode_message_id();
        let part = opencode_part_id();
        let event = opencode_event_id();
        assert!(session.starts_with("ses_"));
        assert!(message.starts_with("msg_"));
        assert!(part.starts_with("prt_"));
        assert!(event.starts_with("evt_"));
        assert_eq!(session.len(), 30);
        assert_eq!(message.len(), 30);
        assert_eq!(part.len(), 30);
        assert_eq!(event.len(), 30);
    }
}
