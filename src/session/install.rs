//! Install synthesized UniversalSession data into provider-native live stores.
//!
//! This is stricter than writing a standalone JSONL/DB export: Claude and
//! Codex need provider-specific paths, and Codex may also need its
//! `state_5.sqlite::threads` index updated for list/resume behavior.

use std::path::{Path, PathBuf};

use crate::error::{ConvertError, Result};
use crate::providers;
use crate::session::clone::{
    capture_codex_state_thread_row, remove_clone_path_entry, ArtifactPath, ClonePathRollback,
    CodexStateThreadRollback,
};
use crate::session::native_validate::{self, NativeValidationOpts, NativeValidationReport};
use crate::universal::{Provider, UniversalSession};

#[derive(Debug, Clone)]
pub struct InstallSessionOpts {
    /// If false and the target provider already has this session id, error out.
    pub overwrite: bool,
    /// Override `~/.claude` root. Tests use this to install into a tempdir.
    pub claude_home: Option<PathBuf>,
    /// Override `~/.codex` root. Tests use this to install into a tempdir.
    pub codex_home: Option<PathBuf>,
    /// Override `~/.codex/state_5.sqlite`. Tests use this with a cloned DB.
    pub codex_state_5_path: Option<PathBuf>,
    /// Update Codex's `state_5.sqlite::threads` row when possible.
    pub codex_update_index: bool,
    /// Override OpenCode's default DB path. Tests use this to install into a tempdir.
    pub opencode_db_path: Option<PathBuf>,
    /// Override Pi's `~/.pi/agent` root. Tests use this to install into a tempdir.
    pub pi_agent_dir: Option<PathBuf>,
    /// Override Pi's `--session-dir` flat session directory.
    pub pi_session_dir: Option<PathBuf>,
    /// Override GJC's `~/.gjc/agent` root. Tests use this to install into a tempdir.
    pub gjc_agent_dir: Option<PathBuf>,
    /// Override GJC's `--session-dir` session directory.
    pub gjc_session_dir: Option<PathBuf>,
}

impl Default for InstallSessionOpts {
    fn default() -> Self {
        Self {
            overwrite: false,
            claude_home: None,
            codex_home: None,
            codex_state_5_path: None,
            codex_update_index: true,
            opencode_db_path: None,
            pi_agent_dir: None,
            pi_session_dir: None,
            gjc_agent_dir: None,
            gjc_session_dir: None,
        }
    }
}

#[derive(Debug)]
pub struct InstallSessionReport {
    pub provider: Provider,
    pub session_id: String,
    pub artifact: ArtifactPath,
    pub validation: NativeValidationReport,
}

pub fn install_universal_session(
    provider: Provider,
    session: &UniversalSession,
    opts: &InstallSessionOpts,
) -> Result<InstallSessionReport> {
    crate::debug::log(
        "session_install_start",
        serde_json::json!({
            "provider": provider.as_str(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "overwrite": opts.overwrite,
        }),
    );

    #[cfg(feature = "opencode")]
    if provider == Provider::OpenCode {
        return install_opencode_transactional(session, opts);
    }

    let (artifact, rollback) = match provider {
        #[cfg(feature = "claude")]
        Provider::Claude => {
            let provider_opts = providers::claude::install::InstallOpts {
                claude_home: opts.claude_home.clone(),
                overwrite: opts.overwrite,
            };
            let jsonl = providers::claude::install::planned_jsonl_path(session, &provider_opts)?;
            let sidecar = jsonl.with_extension("");
            let paths = vec![jsonl.clone(), sidecar.clone()];
            ensure_install_paths_available(&paths, opts.overwrite)?;
            let rollback = ClonePathRollback::capture(&paths, opts.overwrite)?;
            if opts.overwrite {
                if let Err(error) = remove_clone_path_entry(&sidecar) {
                    return Err(error_after_install_rollback(error, rollback));
                }
            }
            let report = match providers::claude::install::install_to_planned_path(
                session,
                &provider_opts,
                &jsonl,
            ) {
                Ok(report) => report,
                Err(error) => return Err(error_after_install_rollback(error, rollback)),
            };
            (
                ArtifactPath::File(report.jsonl_path),
                InstallRollback::Paths(rollback),
            )
        }
        #[cfg(feature = "codex")]
        Provider::Codex => {
            let provider_opts = providers::codex::install::InstallOpts {
                codex_home: opts.codex_home.clone(),
                overwrite: opts.overwrite,
                update_index: opts.codex_update_index,
                state_5_path: opts.codex_state_5_path.clone(),
            };
            let plan = providers::codex::install::planned_install(session, &provider_opts)?;
            ensure_install_paths_available(
                std::slice::from_ref(&plan.rollout_path),
                opts.overwrite,
            )?;
            let path_rollback = ClonePathRollback::capture(
                std::slice::from_ref(&plan.rollout_path),
                opts.overwrite,
            )?;
            let state_rollback = if opts.codex_update_index {
                match capture_codex_state_thread_row(
                    &plan.index_path,
                    &session.session_id,
                    opts.overwrite,
                ) {
                    Ok(rollback) => rollback,
                    Err(error) => {
                        path_rollback.commit();
                        return Err(error);
                    }
                }
            } else {
                CodexStateThreadRollback::inactive()
            };
            let rollback = InstallRollback::Codex {
                path: path_rollback,
                state: state_rollback,
            };
            let report =
                match providers::codex::install::install_planned(session, &provider_opts, &plan) {
                    Ok(report) => report,
                    Err(error) => return Err(error_after_install_rollback(error, rollback)),
                };
            (ArtifactPath::File(report.rollout_path), rollback)
        }
        #[cfg(feature = "pi")]
        Provider::Pi => {
            let provider_opts = providers::pi::install::InstallOpts {
                pi_agent_dir: opts.pi_agent_dir.clone(),
                pi_session_dir: opts.pi_session_dir.clone(),
                overwrite: opts.overwrite,
            };
            let path = providers::pi::install::planned_jsonl_path(session, &provider_opts)?;
            ensure_install_paths_available(std::slice::from_ref(&path), opts.overwrite)?;
            let rollback = ClonePathRollback::capture(std::slice::from_ref(&path), opts.overwrite)?;
            let report = match providers::pi::install::install_to_planned_path(
                session,
                &provider_opts,
                &path,
            ) {
                Ok(report) => report,
                Err(error) => return Err(error_after_install_rollback(error, rollback)),
            };
            (
                ArtifactPath::File(report.jsonl_path),
                InstallRollback::Paths(rollback),
            )
        }
        #[cfg(feature = "gjc")]
        Provider::Gjc => {
            let provider_opts = providers::gjc::install::InstallOpts {
                gjc_agent_dir: opts.gjc_agent_dir.clone(),
                gjc_session_dir: opts.gjc_session_dir.clone(),
                overwrite: opts.overwrite,
            };
            let path = providers::gjc::install::planned_jsonl_path(session, &provider_opts)?;
            ensure_install_paths_available(std::slice::from_ref(&path), opts.overwrite)?;
            let rollback = ClonePathRollback::capture(std::slice::from_ref(&path), opts.overwrite)?;
            let report = match providers::gjc::install::install_to_planned_path(
                session,
                &provider_opts,
                &path,
            ) {
                Ok(report) => report,
                Err(error) => return Err(error_after_install_rollback(error, rollback)),
            };
            (
                ArtifactPath::File(report.jsonl_path),
                InstallRollback::Paths(rollback),
            )
        }
        #[allow(unreachable_patterns)]
        disabled => {
            return Err(ConvertError::Unsupported(format!(
                "{} provider feature not enabled",
                disabled.as_str()
            )));
        }
    };

    let validation_opts = NativeValidationOpts {
        // Keep validation aligned with the side effects the caller requested.
        // A Codex install with `codex_update_index=false` is intentionally a
        // rollout-file-only install; requiring a threads row afterward would
        // turn a supported option into a post-write validation failure.
        require_codex_state_index: provider != Provider::Codex || opts.codex_update_index,
        codex_state_5_path: opts.codex_state_5_path.clone(),
    };
    let validation = match native_validate::validate_clone_artifact_with_opts(
        provider,
        &session.session_id,
        &artifact,
        validation_opts,
    ) {
        Ok(validation) => validation,
        Err(error) => return Err(error_after_install_rollback(error, rollback)),
    };
    crate::debug::log(
        "session_install_validation",
        serde_json::json!({
            "provider": provider.as_str(),
            "session_id": &session.session_id,
            "ok": validation.ok,
            "checks": validation.checks.len(),
            "failures": validation.checks.iter().filter(|check| !check.ok).count(),
        }),
    );
    if !validation.ok {
        let error = ConvertError::Other(format!(
            "{} session {} failed native install validation: {}",
            provider.as_str(),
            session.session_id,
            validation.failure_summary(),
        ));
        return Err(error_after_install_rollback(error, rollback));
    }
    rollback.commit();

    crate::debug::log(
        "session_install_ok",
        serde_json::json!({
            "provider": provider.as_str(),
            "session_id": &session.session_id,
            "artifact": format!("{:?}", &artifact),
            "native_validation_checks": validation.checks.len(),
        }),
    );

    Ok(InstallSessionReport {
        provider,
        session_id: session.session_id.clone(),
        artifact,
        validation,
    })
}

enum InstallRollback {
    Paths(ClonePathRollback),
    Codex {
        path: ClonePathRollback,
        state: CodexStateThreadRollback,
    },
}

impl InstallRollback {
    fn rollback(self) -> Result<()> {
        match self {
            Self::Paths(paths) => paths.rollback(),
            Self::Codex { path, state } => {
                let mut failures = Vec::new();
                if let Err(error) = state.rollback() {
                    failures.push(error.to_string());
                }
                if let Err(error) = path.rollback() {
                    failures.push(error.to_string());
                }
                if failures.is_empty() {
                    Ok(())
                } else {
                    Err(ConvertError::Other(failures.join("; ")))
                }
            }
        }
    }

    fn commit(self) {
        match self {
            Self::Paths(paths) => paths.commit(),
            Self::Codex { path, state } => {
                state.commit();
                path.commit();
            }
        }
    }
}

fn error_after_install_rollback(
    error: ConvertError,
    rollback: impl Into<InstallRollback>,
) -> ConvertError {
    match rollback.into().rollback() {
        Ok(()) => error,
        Err(rollback_error) => ConvertError::Other(format!(
            "{error}; install rollback failed: {rollback_error}"
        )),
    }
}

impl From<ClonePathRollback> for InstallRollback {
    fn from(value: ClonePathRollback) -> Self {
        Self::Paths(value)
    }
}

fn ensure_install_paths_available(paths: &[PathBuf], overwrite: bool) -> Result<()> {
    if overwrite {
        return Ok(());
    }
    for path in paths {
        if path_entry_exists(path)? {
            return Err(ConvertError::Other(format!(
                "install target already exists at {} (set overwrite=true to replace)",
                path.display()
            )));
        }
    }
    Ok(())
}

fn path_entry_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(feature = "opencode")]
fn install_opencode_transactional(
    session: &UniversalSession,
    opts: &InstallSessionOpts,
) -> Result<InstallSessionReport> {
    let provider_opts = providers::opencode::install::InstallOpts {
        db_path: opts.opencode_db_path.clone(),
        overwrite: opts.overwrite,
    };
    let db_path = providers::opencode::install::planned_db_path(&provider_opts)?;
    let db_existed = path_entry_exists(&db_path)?;
    let result = (|| -> Result<InstallSessionReport> {
        if let Some(parent) = db_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = providers::opencode::db::open_readwrite(&db_path)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        providers::opencode::db::ensure_schema(&tx)?;
        providers::opencode::write::to_db_transaction_with_opts(
            &tx,
            session,
            &providers::opencode::write::WriteOpts {
                overwrite: opts.overwrite,
            },
        )?;

        let artifact = ArtifactPath::OpenCodeDb {
            db_path: db_path.clone(),
            session_id: session.session_id.clone(),
        };
        let validation =
            native_validate::validate_opencode_connection(&db_path, &session.session_id, &tx);
        crate::debug::log(
            "session_install_validation",
            serde_json::json!({
                "provider": Provider::OpenCode.as_str(),
                "session_id": &session.session_id,
                "ok": validation.ok,
                "checks": validation.checks.len(),
                "failures": validation.checks.iter().filter(|check| !check.ok).count(),
            }),
        );
        if !validation.ok {
            return Err(ConvertError::Other(format!(
                "{} session {} failed native install validation: {}",
                Provider::OpenCode.as_str(),
                session.session_id,
                validation.failure_summary(),
            )));
        }
        tx.commit()?;
        crate::debug::log(
            "session_install_ok",
            serde_json::json!({
                "provider": Provider::OpenCode.as_str(),
                "session_id": &session.session_id,
                "artifact": format!("{:?}", &artifact),
                "native_validation_checks": validation.checks.len(),
            }),
        );
        Ok(InstallSessionReport {
            provider: Provider::OpenCode,
            session_id: session.session_id.clone(),
            artifact,
            validation,
        })
    })();

    match result {
        Ok(report) => Ok(report),
        Err(error) => {
            if !db_existed {
                if let Err(cleanup_error) = remove_new_opencode_database(&db_path) {
                    return Err(ConvertError::Other(format!(
                        "{error}; failed to remove newly-created OpenCode database after rollback: {cleanup_error}"
                    )));
                }
            }
            Err(error)
        }
    }
}

#[cfg(feature = "opencode")]
fn remove_new_opencode_database(db_path: &Path) -> Result<()> {
    let mut failures = Vec::new();
    let file_name = db_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "opencode.db".to_string());
    for path in [
        db_path.to_path_buf(),
        db_path.with_file_name(format!("{file_name}-wal")),
        db_path.with_file_name(format!("{file_name}-shm")),
        db_path.with_file_name(format!("{file_name}-journal")),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ConvertError::Other(failures.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn assert_no_rollback_backup(dir: &Path) {
        assert!(
            fs::read_dir(dir).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".rollback-")),
            "rollback must consume every private backup"
        );
    }

    #[cfg(feature = "claude")]
    #[test]
    fn failed_claude_overwrite_restores_jsonl_and_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join(".claude");
        let session = UniversalSession::new("rollback-session", Provider::Claude, "/repo");
        let target = providers::claude::install::planned_jsonl_path(
            &session,
            &providers::claude::install::InstallOpts {
                claude_home: Some(home.clone()),
                overwrite: true,
            },
        )
        .unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"old claude transcript\n").unwrap();
        let sidecar = target.with_extension("");
        fs::create_dir(&sidecar).unwrap();
        fs::write(sidecar.join("old-result.txt"), b"old sidecar").unwrap();

        let error = install_universal_session(
            Provider::Claude,
            &session,
            &InstallSessionOpts {
                overwrite: true,
                claude_home: Some(home),
                ..Default::default()
            },
        )
        .expect_err("an empty Claude transcript must fail native validation");

        assert!(error
            .to_string()
            .contains("failed native install validation"));
        assert_eq!(fs::read(&target).unwrap(), b"old claude transcript\n");
        assert_eq!(
            fs::read(sidecar.join("old-result.txt")).unwrap(),
            b"old sidecar"
        );
        assert_no_rollback_backup(target.parent().unwrap());
    }

    #[cfg(feature = "pi")]
    #[test]
    fn failed_pi_overwrite_restores_previous_jsonl() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("pi-sessions");
        fs::create_dir(&sessions).unwrap();
        let session_id = "019e0000-0000-7000-8000-0000000000aa";
        let old = format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{session_id}\",\"timestamp\":\"2026-05-20T01:00:00.000Z\",\"cwd\":\"/repo\"}}\nold\n"
        );
        let target = sessions.join("old.jsonl");
        fs::write(&target, &old).unwrap();
        let session = UniversalSession::new(session_id, Provider::Pi, "/repo");

        install_universal_session(
            Provider::Pi,
            &session,
            &InstallSessionOpts {
                overwrite: true,
                pi_session_dir: Some(sessions.clone()),
                ..Default::default()
            },
        )
        .expect_err("a header-only Pi transcript must fail native validation");

        assert_eq!(fs::read_to_string(&target).unwrap(), old);
        assert_no_rollback_backup(&sessions);
    }

    #[cfg(feature = "gjc")]
    #[test]
    fn failed_gjc_overwrite_restores_previous_jsonl() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("gjc-sessions");
        fs::create_dir(&sessions).unwrap();
        let session_id = "019e0000-0000-7000-8000-0000000000ba";
        let old = format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{session_id}\",\"timestamp\":\"2026-05-20T01:00:00.000Z\",\"cwd\":\"/repo\",\"title\":\"old\"}}\nold\n"
        );
        let target = sessions.join("old.jsonl");
        fs::write(&target, &old).unwrap();
        let session = UniversalSession::new(session_id, Provider::Gjc, "/repo");

        install_universal_session(
            Provider::Gjc,
            &session,
            &InstallSessionOpts {
                overwrite: true,
                gjc_session_dir: Some(sessions.clone()),
                ..Default::default()
            },
        )
        .expect_err("a header-only GJC transcript must fail native validation");

        assert_eq!(fs::read_to_string(&target).unwrap(), old);
        assert_no_rollback_backup(&sessions);
    }

    #[cfg(all(feature = "codex", feature = "opencode"))]
    #[test]
    fn failed_codex_overwrite_restores_rollout_and_threads_row() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join(".codex");
        let session_id = "11111111-1111-7111-8111-111111111111";
        let rollout_dir = home.join("sessions/2026/05/20");
        fs::create_dir_all(&rollout_dir).unwrap();
        let rollout = rollout_dir.join(format!("rollout-2026-05-20T01-00-00-{session_id}.jsonl"));
        fs::write(&rollout, b"old codex transcript\n").unwrap();
        let state_5 = home.join("state_5.sqlite");
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                source TEXT NOT NULL,
                model_provider TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT NOT NULL,
                sandbox_policy TEXT NOT NULL,
                approval_mode TEXT NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                has_user_event INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0,
                archived_at INTEGER,
                git_sha TEXT,
                git_branch TEXT,
                git_origin_url TEXT,
                cli_version TEXT NOT NULL DEFAULT '',
                first_user_message TEXT NOT NULL DEFAULT '',
                agent_nickname TEXT,
                agent_role TEXT,
                memory_mode TEXT NOT NULL DEFAULT 'enabled',
                model TEXT,
                reasoning_effort TEXT,
                agent_path TEXT,
                created_at_ms INTEGER,
                updated_at_ms INTEGER,
                thread_source TEXT,
                preview TEXT NOT NULL DEFAULT ''
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads
                (id, rollout_path, created_at, updated_at, source, model_provider,
                 cwd, title, sandbox_policy, approval_mode)
             VALUES (?1, ?2, 1, 2, 'exec', 'openai', '/old', 'old title',
                     '{\"type\":\"read-only\"}', 'never')",
            rusqlite::params![session_id, rollout.display().to_string()],
        )
        .unwrap();
        drop(conn);

        // Empty cwd survives serialization but must fail the native cwd check
        // after both the rollout and state row have been replaced.
        let session = UniversalSession::new(session_id, Provider::Codex, "");
        install_universal_session(
            Provider::Codex,
            &session,
            &InstallSessionOpts {
                overwrite: true,
                codex_home: Some(home),
                codex_state_5_path: Some(state_5.clone()),
                codex_update_index: true,
                ..Default::default()
            },
        )
        .expect_err("empty Codex cwd must fail native validation");

        assert_eq!(fs::read(&rollout).unwrap(), b"old codex transcript\n");
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        let restored: (String, String, String) = conn
            .query_row(
                "SELECT rollout_path, cwd, title FROM threads WHERE id = ?1",
                rusqlite::params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            restored,
            (
                rollout.display().to_string(),
                "/old".to_string(),
                "old title".to_string()
            )
        );
        assert_no_rollback_backup(&rollout_dir);
    }

    #[cfg(feature = "opencode")]
    fn seed_valid_opencode_session(db_path: &Path, session_id: &str) {
        let conn = rusqlite::Connection::open(db_path).unwrap();
        providers::opencode::db::ensure_schema(&conn).unwrap();
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
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/repo', 1, 1, '[]')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session
                (id, project_id, directory, title, slug, version, path,
                 time_created, time_updated)
             VALUES (?1, 'global', '/repo', 'old title', 'old-slug', '1.0',
                     '-repo', 1, 2)",
            rusqlite::params![session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_0123456789abABCDEFGHIJKLMN', ?1, 1, 1,
                     '{\"role\":\"user\",\"text\":\"old\"}')",
            rusqlite::params![session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO todo
                (session_id, content, status, priority, position,
                 time_created, time_updated)
             VALUES (?1, 'old todo', 'in_progress', 'high', 1, 1, 2)",
            rusqlite::params![session_id],
        )
        .unwrap();
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn failed_opencode_overwrite_rolls_back_same_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("opencode.db");
        let session_id = "ses_0123456789abABCDEFGHIJKLMN";
        seed_valid_opencode_session(&db_path, session_id);
        let session = UniversalSession::new(session_id, Provider::OpenCode, "/repo");

        install_universal_session(
            Provider::OpenCode,
            &session,
            &InstallSessionOpts {
                overwrite: true,
                opencode_db_path: Some(db_path.clone()),
                ..Default::default()
            },
        )
        .expect_err("an OpenCode session without messages must fail validation");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let title: String = conn
            .query_row(
                "SELECT title FROM session WHERE id = ?1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        let message_data: String = conn
            .query_row(
                "SELECT data FROM message WHERE session_id = ?1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "old title");
        assert!(message_data.contains("old"));
        let todo: String = conn
            .query_row(
                "SELECT content FROM todo WHERE session_id = ?1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(todo, "old todo");
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn failed_new_opencode_install_removes_new_database() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("new-opencode.db");
        let session = UniversalSession::new(
            "ses_0123456789abABCDEFGHIJKLMN",
            Provider::OpenCode,
            "/repo",
        );

        install_universal_session(
            Provider::OpenCode,
            &session,
            &InstallSessionOpts {
                opencode_db_path: Some(db_path.clone()),
                ..Default::default()
            },
        )
        .expect_err("an invalid new install must roll back");

        assert!(!db_path.exists());
        assert!(!db_path.with_file_name("new-opencode.db-wal").exists());
        assert!(!db_path.with_file_name("new-opencode.db-shm").exists());
    }
}
