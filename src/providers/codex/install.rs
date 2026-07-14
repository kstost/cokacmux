//! Install a UniversalSession back into Codex's `~/.codex/sessions` layout
//! AND register it in `~/.codex/state_5.sqlite::threads` so that
//! `codex resume <sid>` (or the picker) sees it.
//!
//! All NOT NULL columns of the `threads` table are populated with values
//! drawn from the same enum domain as codex's own writes:
//! ```text
//!   source           = (from session_meta.source if any, else 'exec')
//!   model_provider   = (from session.model.provider_id if any, else 'openai')
//!   approval_mode    = 'never'        -- matches `codex exec` rollouts
//!   sandbox_policy   = {"type":"read-only"}  -- safest valid value
//!   memory_mode      = 'enabled'      -- column default
//! ```
//! These are real values observed in a live `state_5.sqlite` v3 schema. We
//! use `INSERT OR REPLACE` so re-installing the same UUID overwrites the
//! prior row cleanly.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::error::{ConvertError, Result};
use crate::universal::UniversalSession;

use super::CodexWriteOpts;

#[derive(Debug, Clone)]
pub struct InstallOpts {
    /// Override `~/.codex` root.
    pub codex_home: Option<PathBuf>,
    /// If false and target file exists, error out.
    pub overwrite: bool,
    /// Also register the session in `state_5.sqlite::threads` so that
    /// `codex resume <sid>` and the picker see it.
    ///
    /// Requires the `opencode` feature, which provides the `rusqlite`
    /// dependency used for the Codex index. If that feature is off, update
    /// attempts are reported as not indexed.
    pub update_index: bool,
    /// Override the state_5.sqlite path (for tests).
    pub state_5_path: Option<PathBuf>,
}

impl Default for InstallOpts {
    fn default() -> Self {
        Self {
            codex_home: None,
            overwrite: false,
            update_index: true,
            state_5_path: None,
        }
    }
}

#[derive(Debug)]
pub struct InstallReport {
    pub rollout_path: PathBuf,
    pub bytes_written: u64,
    pub index_path: Option<PathBuf>,
    pub indexed: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct InstallPlan {
    pub rollout_path: PathBuf,
    pub index_path: PathBuf,
}

pub fn install_to_user_dir(
    session: &UniversalSession,
    opts: &InstallOpts,
) -> Result<InstallReport> {
    let plan = planned_install(session, opts)?;
    install_planned(session, opts, &plan)
}

/// Resolve the rollout and index destinations without changing either. An
/// existing rollout is selected by session identity rather than by the new
/// session timestamp, so overwrite cannot create duplicate files for one id.
pub(crate) fn planned_install(
    session: &UniversalSession,
    opts: &InstallOpts,
) -> Result<InstallPlan> {
    let home = opts
        .codex_home
        .clone()
        .or_else(default_codex_home)
        .ok_or_else(|| ConvertError::Other("could not determine codex home".into()))?;
    if session.session_id.is_empty() {
        return Err(ConvertError::MissingField("session.session_id"));
    }
    validate_session_id(&session.session_id)?;

    let existing = find_rollout_by_session_id(&home.join("sessions"), &session.session_id)?;
    let rollout_path = if let Some(existing) = existing {
        existing
    } else {
        let ts = session.created_at.unwrap_or_else(Utc::now);
        home.join("sessions")
            .join(format!("{:04}", ts.format("%Y")))
            .join(format!("{:02}", ts.format("%m")))
            .join(format!("{:02}", ts.format("%d")))
            .join(rollout_filename(ts, &session.session_id))
    };
    let index_path = opts
        .state_5_path
        .clone()
        .unwrap_or_else(|| home.join("state_5.sqlite"));
    Ok(InstallPlan {
        rollout_path,
        index_path,
    })
}

pub(crate) fn install_planned(
    session: &UniversalSession,
    opts: &InstallOpts,
    plan: &InstallPlan,
) -> Result<InstallReport> {
    crate::debug::log(
        "provider_codex_install_start",
        serde_json::json!({
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "overwrite": opts.overwrite,
            "update_index": opts.update_index,
            "home_override": opts.codex_home.as_ref().map(|p| p.display().to_string()),
            "state_5_override": opts.state_5_path.as_ref().map(|p| p.display().to_string()),
        }),
    );
    let path = &plan.rollout_path;
    let dir = path.parent().ok_or_else(|| {
        ConvertError::Other(format!(
            "codex install path has no parent: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(&dir)?;
    if path.exists() && !opts.overwrite {
        return Err(ConvertError::Other(format!(
            "rollout already exists at {} (set overwrite=true to replace)",
            path.display()
        )));
    }
    if opts.update_index
        && !opts.overwrite
        && codex_thread_row_exists(&plan.index_path, &session.session_id)?
    {
        return Err(ConvertError::Other(format!(
            "codex state row already exists for {} (set overwrite=true to replace)",
            session.session_id
        )));
    }
    super::write::to_install_jsonl_path(session, &path, &CodexWriteOpts::default())?;
    let bytes_written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    // Try to update the index.
    let index_path = plan.index_path.clone();
    let indexed = if opts.update_index {
        match index_threads_row(session, &path, &index_path) {
            Ok(()) => {
                crate::debug::log(
                    "provider_codex_install_index_ok",
                    serde_json::json!({
                        "session_id": &session.session_id,
                        "index_path": index_path.display().to_string(),
                    }),
                );
                true
            }
            Err(error) => {
                crate::debug::log(
                    "provider_codex_install_index_error",
                    serde_json::json!({
                        "session_id": &session.session_id,
                        "index_path": index_path.display().to_string(),
                        "error": error.to_string(),
                    }),
                );
                false
            }
        }
    } else {
        false
    };

    crate::debug::log(
        "provider_codex_install_ok",
        serde_json::json!({
            "session_id": &session.session_id,
            "rollout_path": path.display().to_string(),
            "bytes_written": bytes_written,
            "index_path": if opts.update_index { Some(index_path.display().to_string()) } else { None },
            "indexed": indexed,
        }),
    );
    Ok(InstallReport {
        rollout_path: path.to_path_buf(),
        bytes_written,
        index_path: if opts.update_index {
            Some(index_path)
        } else {
            None
        },
        indexed,
    })
}

fn find_rollout_by_session_id(sessions: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    if !sessions.exists() {
        return Ok(None);
    }
    if !sessions.is_dir() {
        return Err(ConvertError::Other(format!(
            "codex sessions path is not a directory: {}",
            sessions.display()
        )));
    }
    let mut matches = Vec::new();
    collect_rollouts_by_session_id(sessions, session_id, &mut matches)?;
    matches.sort();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => Err(ConvertError::Other(format!(
            "found {count} Codex rollouts for session {}; refusing ambiguous overwrite: {}",
            session_id,
            matches
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn collect_rollouts_by_session_id(
    dir: &Path,
    session_id: &str,
    matches: &mut Vec<PathBuf>,
) -> Result<()> {
    let suffix = format!("-{session_id}.jsonl");
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_rollouts_by_session_id(&path, session_id, matches)?;
        } else if file_type.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        {
            matches.push(path);
        }
    }
    Ok(())
}

#[cfg(feature = "opencode")]
fn codex_thread_row_exists(state_5_path: &Path, session_id: &str) -> Result<bool> {
    if !state_5_path.exists() {
        return Ok(false);
    }
    let conn = rusqlite::Connection::open_with_flags(
        state_5_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM threads WHERE id = ?1)",
        rusqlite::params![session_id],
        |row| row.get(0),
    )?;
    Ok(exists)
}

#[cfg(not(feature = "opencode"))]
fn codex_thread_row_exists(_state_5_path: &Path, _session_id: &str) -> Result<bool> {
    Ok(false)
}

fn rollout_filename(ts: DateTime<Utc>, sid: &str) -> String {
    format!("rollout-{}-{}.jsonl", ts.format("%Y-%m-%dT%H-%M-%S"), sid)
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
fn default_codex_home() -> Option<PathBuf> {
    crate::providers::discovery::configured_home_dir().map(|h| h.join(".codex"))
}
#[cfg(not(feature = "discovery"))]
fn default_codex_home() -> Option<PathBuf> {
    None
}

// ---------- threads index update ----------

/// INSERT (or REPLACE) a row into `state_5.sqlite::threads`. If the file
/// doesn't exist, returns Ok without doing anything (codex will rebuild
/// the index from JSONL files on next launch). If the file exists but the
/// `threads` table is missing or has an incompatible schema, returns an
/// error — the caller's `InstallReport.indexed` will be false.
#[cfg(feature = "opencode")]
fn index_threads_row(
    session: &UniversalSession,
    rollout_path: &std::path::Path,
    state_5_path: &std::path::Path,
) -> Result<()> {
    if !state_5_path.exists() {
        return Err(ConvertError::Other(format!(
            "state_5.sqlite not found at {} — skipping index update",
            state_5_path.display()
        )));
    }
    let conn = rusqlite::Connection::open(state_5_path)?;

    // Sanity: make sure the threads table is what we expect.
    let cols = collect_table_columns(&conn, "threads")?;
    let must_have = [
        "id",
        "rollout_path",
        "created_at",
        "updated_at",
        "source",
        "model_provider",
        "cwd",
        "title",
        "sandbox_policy",
        "approval_mode",
    ];
    for column in must_have {
        if !cols.contains(column) {
            return Err(ConvertError::Other(format!(
                "threads table missing expected column `{}` (state_5.sqlite schema drift?)",
                column
            )));
        }
    }

    // Derive values that match what codex itself writes.
    let now = chrono::Utc::now();
    let created = session.created_at.unwrap_or(now);
    let updated = session.updated_at.unwrap_or(created).max(created);
    let created_s = created.timestamp();
    let updated_s = updated.timestamp();
    let created_ms = created.timestamp_millis();
    let updated_ms = updated.timestamp_millis();

    let title = session
        .title
        .clone()
        .or_else(|| first_user_text(session).map(|t| truncate(t, 80)))
        .unwrap_or_default();
    let first_user = first_user_text(session).unwrap_or_default();
    let preview = truncate(first_user.clone(), 200);

    let model_provider = session
        .model
        .as_ref()
        .and_then(|m| m.provider_id.clone())
        .unwrap_or_else(|| "openai".to_string());
    let source = session_meta_string(session, "source").unwrap_or_else(|| "exec".to_string());
    let thread_source = session_meta_string(session, "thread_source");
    let approval_mode = if source == "exec" {
        "never"
    } else {
        "on-request"
    };
    let cli_version = session
        .origin
        .cli_version
        .clone()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| session_meta_string(session, "cli_version"))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let model_id = session
        .model
        .as_ref()
        .map(|m| m.model_id.clone())
        .filter(|id| !id.trim().is_empty())
        .or_else(|| Some("gpt-5.5".to_string()));
    let reasoning_effort = session
        .model
        .as_ref()
        .and_then(|m| m.variant.clone())
        .filter(|effort| !effort.trim().is_empty() && effort != "default")
        .or_else(|| Some("medium".to_string()));
    let git_sha = session.git.as_ref().and_then(|g| g.commit.clone());
    let git_branch = session.git.as_ref().and_then(|g| g.branch.clone());
    let git_origin = session.git.as_ref().and_then(|g| g.origin_url.clone());
    let tokens_used: i64 = session
        .usage_total
        .as_ref()
        .and_then(|u| {
            u.total_tokens.or_else(|| {
                // fall back to input+output if total is missing
                match (u.input_tokens, u.output_tokens) {
                    (Some(i), Some(o)) => Some(i.saturating_add(o)),
                    _ => None,
                }
            })
        })
        .unwrap_or(0)
        .min(i64::MAX as u64) as i64;
    let has_user_event = if session
        .messages
        .iter()
        .any(|m| m.role == crate::universal::Role::User && !m.flags.is_meta)
    {
        1i64
    } else {
        0i64
    };

    let has_thread_source = cols.contains("thread_source");
    let has_preview = cols.contains("preview");
    match (has_thread_source, has_preview) {
        (true, true) => {
            conn.execute(
                "INSERT OR REPLACE INTO threads (
                    id, rollout_path, created_at, updated_at,
                    source, model_provider, cwd, title,
                    sandbox_policy, approval_mode,
                    tokens_used, has_user_event, archived,
                    cli_version, first_user_message,
                    memory_mode, model, reasoning_effort,
                    git_sha, git_branch, git_origin_url,
                    created_at_ms, updated_at_ms, thread_source, preview
                ) VALUES (
                    ?1, ?2, ?3, ?4,
                    ?5, ?6, ?7, ?8,
                    '{\"type\":\"read-only\"}', ?9,
                    ?10, ?11, 0,
                    ?12, ?13,
                    'enabled', ?14, ?15,
                    ?16, ?17, ?18,
                    ?19, ?20, ?21, ?22
                )",
                rusqlite::params![
                    session.session_id,
                    rollout_path.display().to_string(),
                    created_s,
                    updated_s,
                    source,
                    model_provider,
                    session.cwd,
                    title,
                    approval_mode,
                    tokens_used,
                    has_user_event,
                    cli_version,
                    first_user,
                    model_id,
                    reasoning_effort,
                    git_sha,
                    git_branch,
                    git_origin,
                    created_ms,
                    updated_ms,
                    thread_source,
                    preview,
                ],
            )?;
        }
        (true, false) => {
            conn.execute(
                "INSERT OR REPLACE INTO threads (
                    id, rollout_path, created_at, updated_at,
                    source, model_provider, cwd, title,
                    sandbox_policy, approval_mode,
                    tokens_used, has_user_event, archived,
                    cli_version, first_user_message,
                    memory_mode, model, reasoning_effort,
                    git_sha, git_branch, git_origin_url,
                    created_at_ms, updated_at_ms, thread_source
                ) VALUES (
                    ?1, ?2, ?3, ?4,
                    ?5, ?6, ?7, ?8,
                    '{\"type\":\"read-only\"}', ?9,
                    ?10, ?11, 0,
                    ?12, ?13,
                    'enabled', ?14, ?15,
                    ?16, ?17, ?18,
                    ?19, ?20, ?21
                )",
                rusqlite::params![
                    session.session_id,
                    rollout_path.display().to_string(),
                    created_s,
                    updated_s,
                    source,
                    model_provider,
                    session.cwd,
                    title,
                    approval_mode,
                    tokens_used,
                    has_user_event,
                    cli_version,
                    first_user,
                    model_id,
                    reasoning_effort,
                    git_sha,
                    git_branch,
                    git_origin,
                    created_ms,
                    updated_ms,
                    thread_source,
                ],
            )?;
        }
        (false, true) => {
            conn.execute(
                "INSERT OR REPLACE INTO threads (
                    id, rollout_path, created_at, updated_at,
                    source, model_provider, cwd, title,
                    sandbox_policy, approval_mode,
                    tokens_used, has_user_event, archived,
                    cli_version, first_user_message,
                    memory_mode, model, reasoning_effort,
                    git_sha, git_branch, git_origin_url,
                    created_at_ms, updated_at_ms, preview
                ) VALUES (
                    ?1, ?2, ?3, ?4,
                    ?5, ?6, ?7, ?8,
                    '{\"type\":\"read-only\"}', ?9,
                    ?10, ?11, 0,
                    ?12, ?13,
                    'enabled', ?14, ?15,
                    ?16, ?17, ?18,
                    ?19, ?20, ?21
                )",
                rusqlite::params![
                    session.session_id,
                    rollout_path.display().to_string(),
                    created_s,
                    updated_s,
                    source,
                    model_provider,
                    session.cwd,
                    title,
                    approval_mode,
                    tokens_used,
                    has_user_event,
                    cli_version,
                    first_user,
                    model_id,
                    reasoning_effort,
                    git_sha,
                    git_branch,
                    git_origin,
                    created_ms,
                    updated_ms,
                    preview,
                ],
            )?;
        }
        (false, false) => {
            conn.execute(
                "INSERT OR REPLACE INTO threads (
                    id, rollout_path, created_at, updated_at,
                    source, model_provider, cwd, title,
                    sandbox_policy, approval_mode,
                    tokens_used, has_user_event, archived,
                    cli_version, first_user_message,
                    memory_mode, model, reasoning_effort,
                    git_sha, git_branch, git_origin_url,
                    created_at_ms, updated_at_ms
                ) VALUES (
                    ?1, ?2, ?3, ?4,
                    ?5, ?6, ?7, ?8,
                    '{\"type\":\"read-only\"}', ?9,
                    ?10, ?11, 0,
                    ?12, ?13,
                    'enabled', ?14, ?15,
                    ?16, ?17, ?18,
                    ?19, ?20
                )",
                rusqlite::params![
                    session.session_id,
                    rollout_path.display().to_string(),
                    created_s,
                    updated_s,
                    source,
                    model_provider,
                    session.cwd,
                    title,
                    approval_mode,
                    tokens_used,
                    has_user_event,
                    cli_version,
                    first_user,
                    model_id,
                    reasoning_effort,
                    git_sha,
                    git_branch,
                    git_origin,
                    created_ms,
                    updated_ms,
                ],
            )?;
        }
    }
    Ok(())
}

#[cfg(feature = "opencode")]
fn session_meta_string(session: &UniversalSession, key: &str) -> Option<String> {
    session
        .session_meta
        .as_ref()
        .and_then(|meta| meta.get(key))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
}

#[cfg(not(feature = "opencode"))]
fn index_threads_row(
    _session: &UniversalSession,
    _rollout_path: &std::path::Path,
    _state_5_path: &std::path::Path,
) -> Result<()> {
    Err(ConvertError::Unsupported(
        "codex threads index update requires the `opencode` feature (rusqlite)".into(),
    ))
}

#[cfg(feature = "opencode")]
fn collect_table_columns(
    conn: &rusqlite::Connection,
    table: &str,
) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect::<std::collections::HashSet<_>>();
    Ok(names)
}

#[cfg(feature = "opencode")]
fn first_user_text(session: &UniversalSession) -> Option<String> {
    for m in &session.messages {
        if matches!(m.role, crate::universal::Role::User) {
            for b in &m.content {
                if let crate::universal::ContentBlock::Text { text, .. } = b {
                    if !text.is_empty() {
                        return Some(text.clone());
                    }
                }
            }
        }
    }
    None
}

#[cfg(feature = "opencode")]
fn truncate(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    // truncate on char boundary
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universal::{Provider, UniversalSession};

    #[test]
    fn rejects_session_id_that_can_escape_sessions_directory() {
        let temp = tempfile::tempdir().unwrap();
        let session = UniversalSession::new("../../escape", Provider::Codex, "/tmp");

        let error = install_to_user_dir(
            &session,
            &InstallOpts {
                codex_home: Some(temp.path().join("codex")),
                overwrite: false,
                update_index: false,
                state_5_path: None,
            },
        )
        .expect_err("path separators in a session id must be rejected");

        assert!(error.to_string().contains("session.session_id"));
        assert!(!temp.path().join("escape.jsonl").exists());
    }

    #[test]
    fn planned_install_reuses_existing_rollout_for_same_session_id() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex");
        let session_id = "11111111-1111-7111-8111-111111111111";
        let existing = home
            .join("sessions/2025/01/02")
            .join(format!("rollout-2025-01-02T03-04-05-{session_id}.jsonl"));
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, "old").unwrap();
        let session = UniversalSession::new(session_id, Provider::Codex, "/repo");

        let plan = planned_install(
            &session,
            &InstallOpts {
                codex_home: Some(home),
                overwrite: true,
                update_index: false,
                state_5_path: None,
            },
        )
        .unwrap();

        assert_eq!(plan.rollout_path, existing);
    }

    #[test]
    fn planned_install_refuses_duplicate_rollouts_for_one_session_id() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex");
        let session_id = "11111111-1111-7111-8111-111111111111";
        for day in ["01", "02"] {
            let path = home
                .join(format!("sessions/2025/01/{day}"))
                .join(format!("rollout-2025-01-{day}T03-04-05-{session_id}.jsonl"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "old").unwrap();
        }
        let session = UniversalSession::new(session_id, Provider::Codex, "/repo");

        let error = planned_install(
            &session,
            &InstallOpts {
                codex_home: Some(home),
                overwrite: true,
                update_index: false,
                state_5_path: None,
            },
        )
        .expect_err("duplicate identity must not select an arbitrary rollout");

        assert!(error.to_string().contains("2 Codex rollouts"));
    }

    #[test]
    fn install_rewrites_replayed_session_identity() {
        let source = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"old-session","session_id":"old-session","cwd":"/old","model_provider":"openai"}}
{"timestamp":"2026-05-20T01:00:00.100Z","type":"turn_context","payload":{"cwd":"/old","model":"gpt-5.5","effort":"high"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#;
        let mut session =
            crate::providers::codex::from_jsonl_str(source, &Default::default()).unwrap();
        session.session_id = "new-session".into();
        session.cwd = "/new".into();
        let temp = tempfile::tempdir().unwrap();

        let report = install_to_user_dir(
            &session,
            &InstallOpts {
                codex_home: Some(temp.path().to_path_buf()),
                overwrite: false,
                update_index: false,
                state_5_path: None,
            },
        )
        .unwrap();

        let values = std::fs::read_to_string(report.rollout_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let meta = values
            .iter()
            .find(|value| value["type"] == "session_meta")
            .unwrap();
        assert_eq!(meta["payload"]["id"], "new-session");
        assert_eq!(meta["payload"]["session_id"], "new-session");
        assert_eq!(meta["payload"]["cwd"], "/new");
        let turn = values
            .iter()
            .find(|value| value["type"] == "turn_context")
            .unwrap();
        assert_eq!(turn["payload"]["cwd"], "/new");
    }
}
