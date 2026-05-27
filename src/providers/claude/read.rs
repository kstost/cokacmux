//! Claude Code JSONL → UniversalSession.

use std::path::Path;

use crate::debug;
use crate::error::Result;
use crate::universal::UniversalSession;

use super::from_universal::parse_lines;
use super::ClaudeReadCtx;

pub fn from_jsonl_path(path: &Path, ctx: &ClaudeReadCtx) -> Result<UniversalSession> {
    debug::log(
        "provider_claude_read_file_start",
        serde_json::json!({
            "path": path.display().to_string(),
            "ctx_session_id": ctx.session_id.as_deref(),
            "ctx_cwd": ctx.cwd.as_deref(),
            "inline_tool_results": ctx.inline_tool_results,
        }),
    );
    let content = match std::fs::read_to_string(path) {
        Ok(content) => {
            debug::log(
                "provider_claude_read_file_loaded",
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
                "provider_claude_read_file_error",
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
                "provider_claude_parse_error",
                serde_json::json!({
                    "path": path.display().to_string(),
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };

    // If the caller didn't override session_id and we can recover it from
    // the file stem (which is always the session UUID in Claude's layout),
    // do that.
    if ctx.session_id.is_none() {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            session.session_id = stem.to_string();
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
        "provider_claude_read_file_ok",
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

pub fn from_jsonl_str(jsonl: &str, ctx: &ClaudeReadCtx) -> Result<UniversalSession> {
    debug::log(
        "provider_claude_read_str_start",
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
            "provider_claude_read_str_ok",
            serde_json::json!({
                "session_id": &session.session_id,
                "messages": session.messages.len(),
                "cwd": &session.cwd,
            }),
        ),
        Err(error) => debug::log(
            "provider_claude_read_str_error",
            serde_json::json!({
                "error": error.to_string(),
            }),
        ),
    }
    result
}
