//! Pi session JSONL → UniversalSession.

use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::debug;
use crate::error::Result;
use crate::universal::UniversalSession;

use super::from_universal::parse_lines;
use super::PiReadCtx;

pub fn from_jsonl_path(path: &Path, ctx: &PiReadCtx) -> Result<UniversalSession> {
    debug::log(
        "provider_pi_read_file_start",
        serde_json::json!({
            "path": path.display().to_string(),
            "ctx_session_id": ctx.session_id.as_deref(),
            "ctx_cwd": ctx.cwd.as_deref(),
        }),
    );
    let content = std::fs::read_to_string(path)?;
    let mut session = parse_lines(&content, ctx)?;
    session.origin.source_path = Some(path.display().to_string());
    if let Ok(meta) = path.metadata() {
        if let Ok(mtime) = meta.modified() {
            if let Ok(duration) = mtime.duration_since(std::time::UNIX_EPOCH) {
                session.updated_at = crate::time::from_epoch_s(duration.as_secs() as i64);
            }
        }
    }
    debug::log(
        "provider_pi_read_file_ok",
        serde_json::json!({
            "path": path.display().to_string(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "cwd": &session.cwd,
            "title_present": session.title.is_some(),
        }),
    );
    Ok(session)
}

pub fn from_jsonl_str(jsonl: &str, ctx: &PiReadCtx) -> Result<UniversalSession> {
    debug::log(
        "provider_pi_read_str_start",
        serde_json::json!({
            "bytes": jsonl.len(),
            "lines": jsonl.lines().count(),
            "ctx_session_id": ctx.session_id.as_deref(),
            "ctx_cwd": ctx.cwd.as_deref(),
        }),
    );
    parse_lines(jsonl, ctx)
}

pub fn read_header(path: &Path) -> Result<Option<Value>> {
    let file = std::fs::File::open(path)?;
    let mut line = String::new();
    let mut reader = BufReader::new(file);
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let value: Value = match serde_json::from_str(line.trim()) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if value.get("type").and_then(Value::as_str) == Some("session") {
        Ok(Some(value))
    } else {
        Ok(None)
    }
}
