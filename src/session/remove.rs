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
    // DELETE FROM threads row if state_5.sqlite is reachable. Prefer the
    // rollout's own home so temp/test homes and non-default installs roll
    // back correctly.
    let state_5 = infer_codex_home_from_rollout(p)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .map(|home| home.join("state_5.sqlite"));
    if let Some(state_5) = state_5.filter(|path| path.exists()) {
        let conn = rusqlite::Connection::open(&state_5)?;
        deleted_rows += conn.execute(
            "DELETE FROM threads WHERE id = ?1",
            rusqlite::params![info.session_id],
        )? as u64;
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
        "DELETE FROM session_message WHERE session_id = ?1",
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

#[cfg(feature = "opencode")]
fn infer_codex_home_from_rollout(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let day = path.parent()?;
    let month = day.parent()?;
    let year = month.parent()?;
    let sessions = year.parent()?;
    if sessions.file_name().and_then(|name| name.to_str()) != Some("sessions") {
        return None;
    }
    sessions.parent().map(std::path::Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "opencode")]
    #[test]
    fn remove_codex_deletes_state_row_from_rollout_home() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join(".codex");
        let rollout_dir = codex_home.join("sessions/2026/05/30");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        let rollout = rollout_dir
            .join("rollout-2026-05-30T00-00-00-11111111-1111-7111-8111-111111111111.jsonl");
        std::fs::write(&rollout, "{}\n").unwrap();
        let state_5 = codex_home.join("state_5.sqlite");
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        conn.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
            rusqlite::params![
                "11111111-1111-7111-8111-111111111111",
                rollout.display().to_string()
            ],
        )
        .unwrap();
        drop(conn);
        let info = SessionInfo {
            provider: Provider::Codex,
            session_id: "11111111-1111-7111-8111-111111111111".into(),
            cwd: "/tmp".into(),
            source: rollout.clone(),
            updated_at_epoch_s: 0,
            title: None,
        };

        let report = remove(&info).unwrap();

        assert_eq!(report.deleted_rows, 1);
        assert!(!rollout.exists());
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn remove_opencode_deletes_session_message_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::providers::opencode::db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 1, 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session
                (id, project_id, slug, directory, title, version, time_created, time_updated, path)
             VALUES ('ses_remove', 'global', 'slug', '/tmp', 'title', 'v', 1, 1, '-tmp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_remove', 'ses_remove', 1, 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, time_created, time_updated, data)
             VALUES ('evt_remove', 'ses_remove', 'agent-switched', 1, 1, '{}')",
            [],
        )
        .unwrap();
        drop(conn);

        let info = SessionInfo {
            provider: Provider::OpenCode,
            session_id: "ses_remove".into(),
            cwd: "/tmp".into(),
            source: db_path.clone(),
            updated_at_epoch_s: 0,
            title: None,
        };
        let report = remove(&info).unwrap();
        assert_eq!(report.deleted_rows, 3);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM session WHERE id = 'ses_remove') +
                    (SELECT COUNT(*) FROM message WHERE session_id = 'ses_remove') +
                    (SELECT COUNT(*) FROM session_message WHERE session_id = 'ses_remove')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
