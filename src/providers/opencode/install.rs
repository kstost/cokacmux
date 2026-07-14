//! Install a UniversalSession into OpenCode's `~/.local/share/opencode/opencode.db`.
//!
//! Safety: this writes to a SQLite DB that may be in active use by the
//! `opencode` process. We open with default flags (which uses SQLite's
//! own locking) and refuse to proceed if the lock probe fails. Callers
//! who want to install into a custom DB path can use `opencode::to_db_path`
//! directly without going through this function.

use std::path::PathBuf;

use crate::error::{ConvertError, Result};
use crate::universal::UniversalSession;

#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
    /// Override the default DB path (for tests).
    pub db_path: Option<PathBuf>,
    /// If false and target session id already exists, error out.
    pub overwrite: bool,
}

#[derive(Debug)]
pub struct InstallReport {
    pub db_path: PathBuf,
    pub messages: usize,
}

pub fn install_to_default_db(
    session: &UniversalSession,
    opts: &InstallOpts,
) -> Result<InstallReport> {
    crate::debug::log(
        "provider_opencode_install_start",
        serde_json::json!({
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "db_override": opts.db_path.as_ref().map(|p| p.display().to_string()),
            "overwrite": opts.overwrite,
        }),
    );
    let db = planned_db_path(opts)?;
    if !db.exists() {
        // Create a fresh DB with our minimal schema. opencode will migrate
        // it further on next launch.
        if let Some(parent) = db.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut conn = super::db::open_readwrite(&db)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| {
            crate::debug::log(
                "provider_opencode_install_lock_error",
                serde_json::json!({
                    "session_id": &session.session_id,
                    "db_path": db.display().to_string(),
                    "error": error.to_string(),
                }),
            );
            ConvertError::Other(format!(
                "could not acquire write lock on {} (is opencode running?): {}",
                db.display(),
                error
            ))
        })?;
    super::db::ensure_schema(&tx)?;
    super::write::to_db_transaction_with_opts(
        &tx,
        session,
        &super::write::WriteOpts {
            overwrite: opts.overwrite,
        },
    )?;
    tx.commit()?;
    crate::debug::log(
        "provider_opencode_install_ok",
        serde_json::json!({
            "session_id": &session.session_id,
            "db_path": db.display().to_string(),
            "messages": session.messages.len(),
        }),
    );
    Ok(InstallReport {
        db_path: db,
        messages: session.messages.len(),
    })
}

/// Resolve the live database path without creating it. Session-level install
/// uses this to start one immediate transaction spanning write and validation.
pub(crate) fn planned_db_path(opts: &InstallOpts) -> Result<PathBuf> {
    opts.db_path
        .clone()
        .or_else(default_db_path)
        .ok_or_else(|| ConvertError::Other("could not determine opencode db path".into()))
}

#[cfg(feature = "discovery")]
fn default_db_path() -> Option<PathBuf> {
    if let Some(home) = crate::providers::discovery::configured_home_dir() {
        return Some(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
        );
    }
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        return Some(
            PathBuf::from(local_app_data)
                .join("opencode")
                .join("opencode.db"),
        );
    }
    std::env::var("APPDATA")
        .ok()
        .map(|app_data| PathBuf::from(app_data).join("opencode").join("opencode.db"))
}
#[cfg(not(feature = "discovery"))]
fn default_db_path() -> Option<PathBuf> {
    None
}
