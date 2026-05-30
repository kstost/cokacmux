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
    let db = opts
        .db_path
        .clone()
        .or_else(default_db_path)
        .ok_or_else(|| ConvertError::Other("could not determine opencode db path".into()))?;
    if !db.exists() {
        // Create a fresh DB with our minimal schema. opencode will migrate
        // it further on next launch.
        if let Some(parent) = db.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }
    // Probe for write access — bail with a clear message if opencode holds an
    // exclusive lock.
    {
        let conn = super::db::open_readwrite(&db)?;
        if let Err(e) = conn.execute("BEGIN IMMEDIATE; ROLLBACK;", []) {
            crate::debug::log(
                "provider_opencode_install_lock_error",
                serde_json::json!({
                    "session_id": &session.session_id,
                    "db_path": db.display().to_string(),
                    "error": e.to_string(),
                }),
            );
            return Err(ConvertError::Other(format!(
                "could not acquire write lock on {} (is opencode running?): {}",
                db.display(),
                e
            )));
        }
    }
    super::write::to_db_path_with_opts(
        session,
        &db,
        &super::write::WriteOpts {
            overwrite: opts.overwrite,
        },
    )?;
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

#[cfg(feature = "discovery")]
fn default_db_path() -> Option<PathBuf> {
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        return Some(
            PathBuf::from(local_app_data)
                .join("opencode")
                .join("opencode.db"),
        );
    }
    dirs::home_dir().map(|h| {
        h.join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db")
    })
}
#[cfg(not(feature = "discovery"))]
fn default_db_path() -> Option<PathBuf> {
    None
}
