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

#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
    /// Override `~/.claude` root. Tests use this to install into a tempdir.
    pub claude_home: Option<PathBuf>,
    /// If `false` and the target session JSONL already exists, error out.
    pub overwrite: bool,
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
    let jsonl = planned_jsonl_path(session, opts)?;
    install_to_planned_path(session, opts, &jsonl)
}

/// Resolve and validate the native destination without creating or replacing
/// anything. Session-level installers use this to capture the previous
/// artifact before authorizing an overwrite.
pub(crate) fn planned_jsonl_path(
    session: &UniversalSession,
    opts: &InstallOpts,
) -> Result<PathBuf> {
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
    validate_session_id(&session.session_id)?;
    Ok(home
        .join("projects")
        .join(encode_cwd(&session.cwd))
        .join(format!("{}.jsonl", session.session_id)))
}

pub(crate) fn install_to_planned_path(
    session: &UniversalSession,
    opts: &InstallOpts,
    jsonl: &Path,
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
    let project = jsonl.parent().ok_or_else(|| {
        ConvertError::Other(format!(
            "claude install path has no parent: {}",
            jsonl.display()
        ))
    })?;
    std::fs::create_dir_all(&project)?;
    if jsonl.exists() && !opts.overwrite {
        return Err(ConvertError::Other(format!(
            "session JSONL already exists at {} (set overwrite=true to replace)",
            jsonl.display()
        )));
    }
    super::write::to_install_jsonl_path(session, &jsonl, &ClaudeWriteOpts::default())?;
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
        project_dir: project.to_path_buf(),
        jsonl_path: jsonl.to_path_buf(),
        bytes_written,
    })
}

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Ok(());
    }
    Err(ConvertError::Validation(
        "session.session_id must contain only ASCII letters, digits, '-' or '_'".into(),
    ))
}

#[cfg(feature = "discovery")]
fn default_claude_home() -> Option<PathBuf> {
    crate::providers::discovery::configured_home_dir().map(|h| h.join(".claude"))
}
#[cfg(not(feature = "discovery"))]
fn default_claude_home() -> Option<PathBuf> {
    None
}

#[allow(dead_code)]
fn _suppress(_p: &Path) {} // keep `Path` use even if discovery feature is off

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universal::{Provider, UniversalSession};

    #[test]
    fn rejects_session_id_that_can_escape_project_directory() {
        let temp = tempfile::tempdir().unwrap();
        let mut session = UniversalSession::new("../escape", Provider::Claude, "/tmp");
        session.session_id = "../../escape".into();

        let error = install_to_user_dir(
            &session,
            &InstallOpts {
                claude_home: Some(temp.path().join("claude")),
                overwrite: false,
            },
        )
        .expect_err("path separators in a session id must be rejected");

        assert!(error.to_string().contains("session.session_id"));
        assert!(!temp.path().join("escape.jsonl").exists());
    }

    #[test]
    fn install_rewrites_identity_and_inlines_hydrated_tool_result() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source").join("old-session.jsonl");
        let sidecar = source
            .with_extension("")
            .join("tool-results")
            .join("result.txt");
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar, "complete tool output").unwrap();
        let line = serde_json::json!({
            "type": "user",
            "sessionId": "old-session",
            "cwd": "/old",
            "uuid": "019e0000-0000-7000-8000-000000000001",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "call_1",
                    "content": format!(
                        "Output too large. Full output saved to: {}\n\npreview",
                        sidecar.display()
                    )
                }]
            }
        });
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, format!("{}\n", line)).unwrap();

        let mut session =
            crate::providers::claude::from_file(&source, &Default::default()).unwrap();
        session.session_id = "new-session".into();
        session.cwd = "/new".into();
        let report = install_to_user_dir(
            &session,
            &InstallOpts {
                claude_home: Some(temp.path().join("target")),
                overwrite: false,
            },
        )
        .unwrap();

        let installed = std::fs::read_to_string(&report.jsonl_path).unwrap();
        assert!(installed.contains("complete tool output"));
        assert!(!installed.contains(&sidecar.display().to_string()));
        let installed_line: serde_json::Value =
            serde_json::from_str(installed.lines().next().unwrap()).unwrap();
        assert_eq!(installed_line["sessionId"], "new-session");
        assert_eq!(installed_line["cwd"], "/new");
    }
}
