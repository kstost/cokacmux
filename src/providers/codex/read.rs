//! Codex rollout JSONL → UniversalSession.

use std::path::Path;

use crate::debug;
use crate::error::Result;
use crate::universal::UniversalSession;

use super::from_universal::parse_lines;
use super::CodexReadCtx;

pub fn from_jsonl_path(path: &Path, ctx: &CodexReadCtx) -> Result<UniversalSession> {
    debug::log(
        "provider_codex_read_file_start",
        serde_json::json!({
            "path": path.display().to_string(),
            "ctx_session_id": ctx.session_id.as_deref(),
            "ctx_cwd": ctx.cwd.as_deref(),
        }),
    );
    let content = match std::fs::read_to_string(path) {
        Ok(content) => {
            debug::log(
                "provider_codex_read_file_loaded",
                serde_json::json!({
                    "path": path.display().to_string(),
                    "bytes": content.len(),
                    "lines": content.lines().count(),
                }),
            );
            content
        }
        Err(error) => {
            debug::log(
                "provider_codex_read_file_error",
                serde_json::json!({
                    "path": path.display().to_string(),
                    "error": error.to_string(),
                }),
            );
            return Err(error.into());
        }
    };
    let mut session = match parse_lines(&content, ctx) {
        Ok(session) => session,
        Err(error) => {
            debug::log(
                "provider_codex_parse_error",
                serde_json::json!({
                    "path": path.display().to_string(),
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };

    // The session UUID is the last 36 chars of the file stem (before .jsonl).
    if ctx.session_id.is_none() {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if stem.len() >= 36 {
                let candidate = &stem[stem.len() - 36..];
                if is_uuid(candidate) {
                    session.session_id = candidate.to_string();
                }
            }
        }
    }
    session.origin.source_path = Some(path.display().to_string());

    if let Ok(meta) = path.metadata() {
        if let Ok(mtime) = meta.modified() {
            if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                session.updated_at = crate::time::from_epoch_s(d.as_secs() as i64);
            }
        }
    }
    debug::log(
        "provider_codex_read_file_ok",
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

pub fn from_jsonl_str(jsonl: &str, ctx: &CodexReadCtx) -> Result<UniversalSession> {
    debug::log(
        "provider_codex_read_str_start",
        serde_json::json!({
            "bytes": jsonl.len(),
            "lines": jsonl.lines().count(),
            "ctx_session_id": ctx.session_id.as_deref(),
            "ctx_cwd": ctx.cwd.as_deref(),
        }),
    );
    let result = parse_lines(jsonl, ctx);
    match &result {
        Ok(session) => debug::log(
            "provider_codex_read_str_ok",
            serde_json::json!({
                "session_id": &session.session_id,
                "messages": session.messages.len(),
                "cwd": &session.cwd,
            }),
        ),
        Err(error) => debug::log(
            "provider_codex_read_str_error",
            serde_json::json!({
                "error": error.to_string(),
            }),
        ),
    }
    result
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36 && uuid::Uuid::parse_str(s).is_ok()
}
