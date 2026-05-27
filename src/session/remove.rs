//! Remove a session from the agent's live storage.

use crate::error::{ConvertError, Result};
use crate::providers::discovery::SessionInfo;
use crate::universal::Provider;

#[derive(Debug)]
pub struct RemoveReport {
    pub provider: Provider,
    pub deleted_file: Option<std::path::PathBuf>,
    pub deleted_rows: u64,
}

/// Delete the session described by `info`. For Claude this is a file
/// unlink. For Codex it's a file unlink + DELETE FROM state_5.sqlite::threads.
/// For OpenCode it's DELETE FROM session/message/part rows on opencode.db.
pub fn remove(info: &SessionInfo) -> Result<RemoveReport> {
    crate::debug::log(
        "delete_library_start",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &info.session_id,
            "source": info.source.display().to_string(),
        }),
    );
    let result = match info.provider {
        Provider::Claude => remove_claude(info),
        Provider::Codex => remove_codex(info),
        Provider::OpenCode => remove_opencode(info),
    };
    match &result {
        Ok(report) => crate::debug::log(
            "delete_library_ok",
            serde_json::json!({
                "provider": report.provider.as_str(),
                "session_id": &info.session_id,
                "deleted_file": format!("{:?}", &report.deleted_file),
                "deleted_rows": report.deleted_rows,
            }),
        ),
        Err(error) => crate::debug::log(
            "delete_library_error",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "error": error.to_string(),
            }),
        ),
    }
    result
}

fn remove_claude(info: &SessionInfo) -> Result<RemoveReport> {
    // info.source is the JSONL file path.
    let p = &info.source;
    if !p.exists() {
        return Err(ConvertError::Other(format!(
            "claude session file not found: {}",
            p.display()
        )));
    }
    std::fs::remove_file(p)?;
    // Also remove the optional sidecar directory <basename>/ if present.
    let sidecar = p.with_extension("");
    if sidecar.is_dir() {
        let _ = std::fs::remove_dir_all(&sidecar);
    }
    Ok(RemoveReport {
        provider: Provider::Claude,
        deleted_file: Some(p.clone()),
        deleted_rows: 0,
    })
}

#[cfg(feature = "opencode")]
fn remove_codex(info: &SessionInfo) -> Result<RemoveReport> {
    let p = &info.source;
    let mut deleted_rows: u64 = 0;
    // Delete the rollout JSONL.
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    // DELETE FROM threads row if state_5.sqlite is reachable.
    if let Some(home) = dirs::home_dir() {
        let state_5 = home.join(".codex").join("state_5.sqlite");
        if state_5.exists() {
            let conn = rusqlite::Connection::open(&state_5)?;
            deleted_rows += conn.execute(
                "DELETE FROM threads WHERE id = ?1",
                rusqlite::params![info.session_id],
            )? as u64;
        }
    }
    Ok(RemoveReport {
        provider: Provider::Codex,
        deleted_file: Some(p.clone()),
        deleted_rows,
    })
}

#[cfg(not(feature = "opencode"))]
fn remove_codex(info: &SessionInfo) -> Result<RemoveReport> {
    let p = &info.source;
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(RemoveReport {
        provider: Provider::Codex,
        deleted_file: Some(p.clone()),
        deleted_rows: 0,
    })
}

#[cfg(feature = "opencode")]
fn remove_opencode(info: &SessionInfo) -> Result<RemoveReport> {
    // info.source is the opencode.db path.
    let conn = rusqlite::Connection::open(&info.source)?;
    let mut deleted_rows: u64 = 0;
    deleted_rows += conn.execute(
        "DELETE FROM part WHERE session_id = ?1",
        rusqlite::params![info.session_id],
    )? as u64;
    deleted_rows += conn.execute(
        "DELETE FROM message WHERE session_id = ?1",
        rusqlite::params![info.session_id],
    )? as u64;
    deleted_rows += conn.execute(
        "DELETE FROM session WHERE id = ?1",
        rusqlite::params![info.session_id],
    )? as u64;
    Ok(RemoveReport {
        provider: Provider::OpenCode,
        deleted_file: None,
        deleted_rows,
    })
}

#[cfg(not(feature = "opencode"))]
fn remove_opencode(_info: &SessionInfo) -> Result<RemoveReport> {
    Err(ConvertError::Unsupported(
        "opencode remove requires the `opencode` feature".into(),
    ))
}
