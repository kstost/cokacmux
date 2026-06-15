//! Install synthesized UniversalSession data into provider-native live stores.
//!
//! This is stricter than writing a standalone JSONL/DB export: Claude and
//! Codex need provider-specific paths, and Codex may also need its
//! `state_5.sqlite::threads` index updated for list/resume behavior.

use std::path::PathBuf;

use crate::error::{ConvertError, Result};
use crate::providers;
use crate::providers::discovery::SessionInfo;
use crate::session::clone::ArtifactPath;
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

    let artifact = match provider {
        #[cfg(feature = "claude")]
        Provider::Claude => {
            let report = providers::claude::install::install_to_user_dir(
                session,
                &providers::claude::install::InstallOpts {
                    claude_home: opts.claude_home.clone(),
                    overwrite: opts.overwrite,
                },
            )?;
            ArtifactPath::File(report.jsonl_path)
        }
        #[cfg(feature = "codex")]
        Provider::Codex => {
            let report = providers::codex::install::install_to_user_dir(
                session,
                &providers::codex::install::InstallOpts {
                    codex_home: opts.codex_home.clone(),
                    overwrite: opts.overwrite,
                    update_index: opts.codex_update_index,
                    state_5_path: opts.codex_state_5_path.clone(),
                },
            )?;
            ArtifactPath::File(report.rollout_path)
        }
        #[cfg(feature = "opencode")]
        Provider::OpenCode => {
            let report = providers::opencode::install::install_to_default_db(
                session,
                &providers::opencode::install::InstallOpts {
                    db_path: opts.opencode_db_path.clone(),
                    overwrite: opts.overwrite,
                },
            )?;
            ArtifactPath::OpenCodeDb {
                db_path: report.db_path,
                session_id: session.session_id.clone(),
            }
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
    let validation = native_validate::validate_clone_artifact_with_opts(
        provider,
        &session.session_id,
        &artifact,
        validation_opts,
    )?;
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
        let cleanup_error = cleanup_failed_install(provider, session, &artifact, opts)
            .map(|error| format!("; cleanup failed: {error}"))
            .unwrap_or_default();
        return Err(ConvertError::Other(
            format!(
                "{} session {} failed native install validation: {}",
                provider.as_str(),
                session.session_id,
                validation.failure_summary(),
            ) + &cleanup_error,
        ));
    }

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

fn cleanup_failed_install(
    provider: Provider,
    session: &UniversalSession,
    artifact: &ArtifactPath,
    opts: &InstallSessionOpts,
) -> Option<String> {
    if opts.overwrite {
        crate::debug::log(
            "session_install_validation_failed_cleanup_skipped",
            serde_json::json!({
                "provider": provider.as_str(),
                "session_id": &session.session_id,
                "artifact": format!("{:?}", artifact),
                "reason": "overwrite_enabled",
            }),
        );
        return None;
    }

    let source = match artifact {
        ArtifactPath::File(path) => path.clone(),
        ArtifactPath::OpenCodeDb { db_path, .. } => db_path.clone(),
    };
    let info = SessionInfo {
        provider,
        session_id: session.session_id.clone(),
        cwd: session.cwd.clone(),
        source,
        updated_at_epoch_s: 0,
        title: session.title.clone(),
    };

    let override_cleanup = cleanup_codex_state_override(provider, session, opts);
    match (crate::session::remove::remove(&info), override_cleanup) {
        (Ok(report), Ok(extra_deleted_rows)) => {
            let deleted_rows = report.deleted_rows.saturating_add(extra_deleted_rows);
            let deleted_file = report.deleted_file;
            crate::debug::log(
                "session_install_validation_failed_cleanup_ok",
                serde_json::json!({
                    "provider": provider.as_str(),
                    "session_id": &session.session_id,
                    "artifact": format!("{:?}", artifact),
                    "deleted_file": deleted_file.as_ref().map(|path| path.display().to_string()),
                    "deleted_rows": deleted_rows,
                }),
            );
            None
        }
        (Ok(report), Err(error)) => {
            let error = error.to_string();
            crate::debug::log(
                "session_install_validation_failed_cleanup_error",
                serde_json::json!({
                    "provider": provider.as_str(),
                    "session_id": &session.session_id,
                    "artifact": format!("{:?}", artifact),
                    "deleted_file": report.deleted_file.map(|path| path.display().to_string()),
                    "deleted_rows": report.deleted_rows,
                    "error": &error,
                }),
            );
            Some(error)
        }
        (Err(error), Ok(_)) | (Err(error), Err(_)) => {
            let error = error.to_string();
            crate::debug::log(
                "session_install_validation_failed_cleanup_error",
                serde_json::json!({
                    "provider": provider.as_str(),
                    "session_id": &session.session_id,
                    "artifact": format!("{:?}", artifact),
                    "error": &error,
                }),
            );
            Some(error)
        }
    }
}

fn cleanup_codex_state_override(
    provider: Provider,
    session: &UniversalSession,
    opts: &InstallSessionOpts,
) -> Result<u64> {
    if provider != Provider::Codex {
        return Ok(0);
    }
    if opts.codex_state_5_path.is_none() {
        return Ok(0);
    }

    #[cfg(feature = "opencode")]
    {
        let state_5_path = opts
            .codex_state_5_path
            .as_ref()
            .expect("checked codex_state_5_path above");
        if !state_5_path.exists() {
            return Ok(0);
        }
        let conn = rusqlite::Connection::open(state_5_path)?;
        let deleted = conn.execute(
            "DELETE FROM threads WHERE id = ?1",
            rusqlite::params![session.session_id],
        )? as u64;
        return Ok(deleted);
    }

    #[cfg(not(feature = "opencode"))]
    {
        let _ = session;
        Ok(0)
    }
}
