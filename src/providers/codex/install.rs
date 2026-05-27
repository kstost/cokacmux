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

use std::path::PathBuf;

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
    /// Requires the `codex-index` feature (which pulls in `rusqlite` via
    /// the `opencode` feature). If the feature is off this field is
    /// ignored.
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

pub fn install_to_user_dir(
    session: &UniversalSession,
    opts: &InstallOpts,
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
    let home = opts
        .codex_home
        .clone()
        .or_else(default_codex_home)
        .ok_or_else(|| ConvertError::Other("could not determine codex home".into()))?;
    if session.session_id.is_empty() {
        return Err(ConvertError::MissingField("session.session_id"));
    }

    let ts = session.created_at.unwrap_or_else(Utc::now);
    let dir = home
        .join("sessions")
        .join(format!("{:04}", ts.format("%Y")))
        .join(format!("{:02}", ts.format("%m")))
        .join(format!("{:02}", ts.format("%d")));
    std::fs::create_dir_all(&dir)?;

    let fname = rollout_filename(ts, &session.session_id);
    let path = dir.join(fname);
    if path.exists() && !opts.overwrite {
        return Err(ConvertError::Other(format!(
            "rollout already exists at {} (set overwrite=true to replace)",
            path.display()
        )));
    }
    super::write::to_jsonl_path(session, &path, &CodexWriteOpts::default())?;
    let bytes_written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    // Try to update the index.
    let index_path = opts
        .state_5_path
        .clone()
        .unwrap_or_else(|| home.join("state_5.sqlite"));
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
        rollout_path: path,
        bytes_written,
        index_path: if opts.update_index {
            Some(index_path)
        } else {
            None
        },
        indexed,
    })
}

fn rollout_filename(ts: DateTime<Utc>, sid: &str) -> String {
    format!("rollout-{}-{}.jsonl", ts.format("%Y-%m-%dT%H-%M-%S"), sid)
}

#[cfg(feature = "discovery")]
fn default_codex_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex"))
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
    for c in &must_have {
        if !cols.contains(&c.to_string()) {
            return Err(ConvertError::Other(format!(
                "threads table missing expected column `{}` (state_5.sqlite schema drift?)",
                c
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
                    (Some(i), Some(o)) => Some(i + o),
                    _ => None,
                }
            })
        })
        .unwrap_or(0) as i64;

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
                    ?10, 0, 0,
                    ?11, ?12,
                    'enabled', ?13, ?14,
                    ?15, ?16, ?17,
                    ?18, ?19, ?20, ?21
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
                    ?10, 0, 0,
                    ?11, ?12,
                    'enabled', ?13, ?14,
                    ?15, ?16, ?17,
                    ?18, ?19, ?20
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
                    ?10, 0, 0,
                    ?11, ?12,
                    'enabled', ?13, ?14,
                    ?15, ?16, ?17,
                    ?18, ?19, ?20
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
                    ?10, 0, 0,
                    ?11, ?12,
                    'enabled', ?13, ?14,
                    ?15, ?16, ?17,
                    ?18, ?19
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
