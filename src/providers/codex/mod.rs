//! Codex adapter — JSONL "rollout" files.

pub mod from_universal;
pub mod read;
pub mod write;

#[cfg(feature = "discovery")]
pub mod install;

use std::path::Path;

use crate::error::Result;
use crate::universal::UniversalSession;

#[derive(Debug, Clone, Default)]
pub struct CodexReadCtx {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CodexWriteOpts {
    /// Preserve original Codex JSONL records when available. This keeps
    /// same-provider Codex round-trips byte-identical. Disable it when
    /// canonicalizing older converter output for Codex CLI resume.
    pub replay_raw: bool,
}

impl Default for CodexWriteOpts {
    fn default() -> Self {
        Self { replay_raw: true }
    }
}

pub fn from_file(path: &Path) -> Result<UniversalSession> {
    read::from_jsonl_path(path, &CodexReadCtx::default())
}

pub fn from_file_with(path: &Path, ctx: &CodexReadCtx) -> Result<UniversalSession> {
    read::from_jsonl_path(path, ctx)
}

pub fn to_file(session: &UniversalSession, path: &Path, opts: &CodexWriteOpts) -> Result<()> {
    write::to_jsonl_path(session, path, opts)
}

pub fn from_jsonl_str(jsonl: &str, ctx: &CodexReadCtx) -> Result<UniversalSession> {
    read::from_jsonl_str(jsonl, ctx)
}

pub fn to_jsonl_string(session: &UniversalSession, opts: &CodexWriteOpts) -> Result<String> {
    write::to_jsonl_string(session, opts)
}
