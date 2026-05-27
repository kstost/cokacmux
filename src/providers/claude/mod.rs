//! Claude Code adapter — JSONL session files.

pub mod from_universal;
pub mod path;
pub mod read;
pub mod sidecar;
pub mod write;

#[cfg(feature = "discovery")]
pub mod install;

use std::path::Path;

use crate::error::Result;
use crate::universal::UniversalSession;

#[derive(Debug, Clone)]
pub struct ClaudeReadCtx {
    /// If true, when a tool_use_id references a sidecar file at
    /// `<session-uuid>/tool-results/<hash>.txt`, inline the file contents
    /// into the corresponding `ToolResult.output`. Default: true.
    pub inline_tool_results: bool,
    /// Override session id. By default the file stem is used.
    pub session_id: Option<String>,
    /// Override cwd. By default the first `cwd`-bearing line is used.
    pub cwd: Option<String>,
}

impl Default for ClaudeReadCtx {
    fn default() -> Self {
        Self {
            inline_tool_results: true,
            session_id: None,
            cwd: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeWriteOpts {
    /// Emit a `<basename>/tool-results/` sidecar directory for tool results
    /// larger than this threshold (in bytes). 0 disables sidecar emission.
    pub sidecar_threshold_bytes: usize,
}

/// Parse a Claude JSONL session file into a UniversalSession.
pub fn from_file(path: &Path, ctx: &ClaudeReadCtx) -> Result<UniversalSession> {
    read::from_jsonl_path(path, ctx)
}

/// Write a UniversalSession to a Claude JSONL session file.
pub fn to_file(session: &UniversalSession, path: &Path, opts: &ClaudeWriteOpts) -> Result<()> {
    write::to_jsonl_path(session, path, opts)
}

/// Convenience: parse from a JSONL string.
pub fn from_jsonl_str(jsonl: &str, ctx: &ClaudeReadCtx) -> Result<UniversalSession> {
    read::from_jsonl_str(jsonl, ctx)
}

/// Convenience: serialize to a JSONL string.
pub fn to_jsonl_string(session: &UniversalSession, opts: &ClaudeWriteOpts) -> Result<String> {
    write::to_jsonl_string(session, opts)
}
