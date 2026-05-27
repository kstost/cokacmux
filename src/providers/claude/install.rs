//! Install a UniversalSession back into Claude Code's `~/.claude/projects`
//! layout — useful for "I want `claude --resume <sid>` to pick this up".
//!
//! **Known limitation:** if the session contains tool results that were
//! externalized to a `tool-results/<random>.txt` sidecar file in the
//! original layout, the install path will inline them into the JSONL
//! instead of regenerating the sidecar. Claude Code itself accepts both
//! shapes, so this is a fidelity quirk rather than a correctness issue —
//! the resumed session will work but the resulting JSONL may be larger
//! than the original.

use std::path::{Path, PathBuf};

use crate::error::{ConvertError, Result};
use crate::universal::UniversalSession;

use super::{path::encode_cwd, ClaudeWriteOpts};

#[derive(Debug, Clone)]
pub struct InstallOpts {
    /// Override `~/.claude` root. Tests use this to install into a tempdir.
    pub claude_home: Option<PathBuf>,
    /// If `false` and the target session JSONL already exists, error out.
    pub overwrite: bool,
}

impl Default for InstallOpts {
    fn default() -> Self {
        Self {
            claude_home: None,
            overwrite: false,
        }
    }
}

#[derive(Debug)]
pub struct InstallReport {
    pub project_dir: PathBuf,
    pub jsonl_path: PathBuf,
    pub bytes_written: u64,
}

pub fn install_to_user_dir(
    session: &UniversalSession,
    opts: &InstallOpts,
) -> Result<InstallReport> {
    crate::debug::log(
        "provider_claude_install_start",
        serde_json::json!({
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "cwd": &session.cwd,
            "overwrite": opts.overwrite,
            "home_override": opts.claude_home.as_ref().map(|p| p.display().to_string()),
        }),
    );
    let home = opts
        .claude_home
        .clone()
        .or_else(default_claude_home)
        .ok_or_else(|| ConvertError::Other("could not determine claude home".into()))?;
    if session.cwd.is_empty() {
        return Err(ConvertError::MissingField("session.cwd"));
    }
    if session.session_id.is_empty() {
        return Err(ConvertError::MissingField("session.session_id"));
    }
    let project = home.join("projects").join(encode_cwd(&session.cwd));
    std::fs::create_dir_all(&project)?;
    let jsonl = project.join(format!("{}.jsonl", session.session_id));
    if jsonl.exists() && !opts.overwrite {
        return Err(ConvertError::Other(format!(
            "session JSONL already exists at {} (set overwrite=true to replace)",
            jsonl.display()
        )));
    }
    super::write::to_jsonl_path(session, &jsonl, &ClaudeWriteOpts::default())?;
    let bytes_written = std::fs::metadata(&jsonl).map(|m| m.len()).unwrap_or(0);
    crate::debug::log(
        "provider_claude_install_ok",
        serde_json::json!({
            "session_id": &session.session_id,
            "project_dir": project.display().to_string(),
            "jsonl_path": jsonl.display().to_string(),
            "bytes_written": bytes_written,
        }),
    );
    Ok(InstallReport {
        project_dir: project,
        jsonl_path: jsonl,
        bytes_written,
    })
}

#[cfg(feature = "discovery")]
fn default_claude_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude"))
}
#[cfg(not(feature = "discovery"))]
fn default_claude_home() -> Option<PathBuf> {
    None
}

#[allow(dead_code)]
fn _suppress(_p: &Path) {} // keep `Path` use even if discovery feature is off
