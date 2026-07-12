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
        Provider::Pi => remove_pi(info),
        Provider::Gjc => remove_gjc(info),
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
    remove_claude_with_sidecar_remover(info, |path| std::fs::remove_dir_all(path))
}

fn remove_claude_with_sidecar_remover<F>(
    info: &SessionInfo,
    remove_sidecar: F,
) -> Result<RemoveReport>
where
    F: FnOnce(&std::path::Path) -> std::io::Result<()>,
{
    // info.source is the JSONL file path.
    let p = &info.source;
    if !p.exists() {
        return Err(ConvertError::Other(format!(
            "claude session file not found: {}",
            p.display()
        )));
    }
    // Remove the optional sidecar before unlinking the transcript. A sidecar
    // failure must not be reported as success after the primary JSONL has
    // already been destroyed.
    let sidecar = p.with_extension("");
    if sidecar.is_dir() {
        remove_sidecar(&sidecar)?;
    }
    std::fs::remove_file(p)?;
    Ok(RemoveReport {
        provider: Provider::Claude,
        deleted_file: Some(p.clone()),
        deleted_rows: 0,
    })
}

fn remove_pi(info: &SessionInfo) -> Result<RemoveReport> {
    let p = &info.source;
    if !p.exists() {
        return Err(ConvertError::Other(format!(
            "pi session file not found: {}",
            p.display()
        )));
    }
    std::fs::remove_file(p)?;
    Ok(RemoveReport {
        provider: Provider::Pi,
        deleted_file: Some(p.clone()),
        deleted_rows: 0,
    })
}

fn remove_gjc(info: &SessionInfo) -> Result<RemoveReport> {
    let p = &info.source;
    if !p.exists() {
        return Err(ConvertError::Other(format!(
            "gjc session file not found: {}",
            p.display()
        )));
    }
    std::fs::remove_file(p)?;
    Ok(RemoveReport {
        provider: Provider::Gjc,
        deleted_file: Some(p.clone()),
        deleted_rows: 0,
    })
}

#[cfg(feature = "opencode")]
fn remove_codex(info: &SessionInfo) -> Result<RemoveReport> {
    let p = &info.source;
    let mut deleted_rows: u64 = 0;
    // DELETE FROM threads row if state_5.sqlite is reachable. Prefer the
    // rollout's own home so temp/test homes and non-default installs roll
    // back correctly. Delete the index row before unlinking the rollout: if
    // SQLite rejects the operation, preserving the transcript is safer than
    // returning an error after its data has already been destroyed.
    let state_5 = infer_codex_home_from_rollout(p)
        .or_else(|| {
            crate::providers::discovery::configured_home_dir().map(|home| home.join(".codex"))
        })
        .map(|home| home.join("state_5.sqlite"));
    if let Some(state_5) = state_5.filter(|path| path.exists()) {
        let conn = rusqlite::Connection::open_with_flags(
            &state_5,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        deleted_rows += conn.execute(
            "DELETE FROM threads WHERE id = ?1",
            rusqlite::params![info.session_id],
        )? as u64;
    }
    let deleted_file = if p.exists() {
        std::fs::remove_file(p)?;
        Some(p.clone())
    } else {
        None
    };
    Ok(RemoveReport {
        provider: Provider::Codex,
        deleted_file,
        deleted_rows,
    })
}

#[cfg(not(feature = "opencode"))]
fn remove_codex(info: &SessionInfo) -> Result<RemoveReport> {
    let p = &info.source;
    let deleted_file = if p.exists() {
        std::fs::remove_file(p)?;
        Some(p.clone())
    } else {
        None
    };
    Ok(RemoveReport {
        provider: Provider::Codex,
        deleted_file,
        deleted_rows: 0,
    })
}

#[cfg(feature = "opencode")]
fn remove_opencode(info: &SessionInfo) -> Result<RemoveReport> {
    // info.source is the opencode.db path.
    if !info.source.is_file() {
        return Err(ConvertError::Other(format!(
            "opencode database not found: {}",
            info.source.display()
        )));
    }
    let mut conn = rusqlite::Connection::open_with_flags(
        &info.source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    let tx = conn.transaction()?;
    let mut deleted_rows: u64 = 0;
    // `todo` is an official child table but is absent from older schemas and
    // our minimal compatibility schema. Foreign keys are connection-local in
    // SQLite, so delete it explicitly when present.
    if crate::providers::opencode::db::table_exists(&tx, "todo")? {
        deleted_rows += tx.execute(
            "DELETE FROM todo WHERE session_id = ?1",
            rusqlite::params![info.session_id],
        )? as u64;
    }
    deleted_rows += tx.execute(
        "DELETE FROM part WHERE session_id = ?1",
        rusqlite::params![info.session_id],
    )? as u64;
    deleted_rows += tx.execute(
        "DELETE FROM message WHERE session_id = ?1",
        rusqlite::params![info.session_id],
    )? as u64;
    let has_session_message: bool = tx.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'session_message'
        )",
        [],
        |row| row.get(0),
    )?;
    if has_session_message {
        deleted_rows += tx.execute(
            "DELETE FROM session_message WHERE session_id = ?1",
            rusqlite::params![info.session_id],
        )? as u64;
    }
    deleted_rows += tx.execute(
        "DELETE FROM session WHERE id = ?1",
        rusqlite::params![info.session_id],
    )? as u64;
    tx.commit()?;
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

    #[test]
    fn remove_claude_preserves_jsonl_when_sidecar_removal_fails() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("session.jsonl");
        let sidecar = jsonl.with_extension("");
        std::fs::write(&jsonl, "important transcript\n").unwrap();
        std::fs::create_dir(&sidecar).unwrap();
        let info = SessionInfo {
            provider: Provider::Claude,
            session_id: "session".into(),
            cwd: "/tmp".into(),
            source: jsonl.clone(),
            updated_at_epoch_s: 0,
            title: None,
            relation: None,
        };

        let error = remove_claude_with_sidecar_remover(&info, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "sidecar is busy",
            ))
        })
        .expect_err("sidecar failure must be returned");

        assert!(error.to_string().contains("sidecar is busy"));
        assert_eq!(
            std::fs::read_to_string(&jsonl).unwrap(),
            "important transcript\n"
        );
        assert!(sidecar.is_dir());
    }

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
            relation: None,
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
    fn remove_codex_preserves_rollout_when_state_delete_fails() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join(".codex");
        let rollout_dir = codex_home.join("sessions/2026/05/30");
        std::fs::create_dir_all(&rollout_dir).unwrap();
        let session_id = "11111111-1111-7111-8111-111111111111";
        let rollout = rollout_dir.join(format!("rollout-2026-05-30T00-00-00-{session_id}.jsonl"));
        std::fs::write(&rollout, "important transcript\n").unwrap();
        let state_5 = codex_home.join("state_5.sqlite");
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        conn.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
            rusqlite::params![session_id, rollout.display().to_string()],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_thread_delete
             BEFORE DELETE ON threads
             BEGIN SELECT RAISE(ABORT, 'delete rejected'); END;",
        )
        .unwrap();
        drop(conn);
        let info = SessionInfo {
            provider: Provider::Codex,
            session_id: session_id.into(),
            cwd: "/tmp".into(),
            source: rollout.clone(),
            updated_at_epoch_s: 0,
            title: None,
            relation: None,
        };

        let error = remove(&info).expect_err("state deletion should fail");

        assert!(error.to_string().contains("delete rejected"));
        assert_eq!(
            std::fs::read_to_string(&rollout).unwrap(),
            "important transcript\n"
        );
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
        conn.execute_batch(
            "CREATE TABLE todo (
                session_id TEXT NOT NULL,
                content TEXT NOT NULL,
                status TEXT NOT NULL,
                priority TEXT NOT NULL,
                position INTEGER NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                PRIMARY KEY (session_id, position)
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO todo
                (session_id, content, status, priority, position, time_created, time_updated)
             VALUES ('ses_remove', 'stale', 'pending', 'high', 0, 1, 1)",
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
            relation: None,
        };
        let report = remove(&info).unwrap();
        assert_eq!(report.deleted_rows, 4);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM session WHERE id = 'ses_remove') +
                    (SELECT COUNT(*) FROM message WHERE session_id = 'ses_remove') +
                    (SELECT COUNT(*) FROM session_message WHERE session_id = 'ses_remove') +
                    (SELECT COUNT(*) FROM todo WHERE session_id = 'ses_remove')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn remove_opencode_rolls_back_all_rows_when_a_delete_fails() {
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
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('prt_remove', 'msg_remove', 'ses_remove', 1, 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_session_delete
             BEFORE DELETE ON session
             BEGIN SELECT RAISE(ABORT, 'delete rejected'); END;",
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
            relation: None,
        };

        remove(&info).expect_err("trigger should reject the transaction");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM session WHERE id = 'ses_remove') +
                    (SELECT COUNT(*) FROM message WHERE session_id = 'ses_remove') +
                    (SELECT COUNT(*) FROM part WHERE session_id = 'ses_remove')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 3,
            "failed removal must not partially delete rows"
        );
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn remove_opencode_does_not_create_a_missing_database() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("missing.db");
        let info = SessionInfo {
            provider: Provider::OpenCode,
            session_id: "ses_missing".into(),
            cwd: "/tmp".into(),
            source: db_path.clone(),
            updated_at_epoch_s: 0,
            title: None,
            relation: None,
        };

        remove(&info).expect_err("a missing DB is not a removable session");

        assert!(!db_path.exists());
    }
}
