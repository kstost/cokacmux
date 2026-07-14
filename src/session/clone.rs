//! Clone a session by copying provider-native storage and patching only the
//! identifiers that must be unique for the clone.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use chrono::Utc;
use serde_json::Value;

use crate::context_convert::{
    context_file_contents, wrap_session_for_context_convert,
    wrap_session_for_context_file_reference,
};
use crate::error::{ConvertError, Result};
use crate::providers;
use crate::providers::discovery::SessionInfo;
use crate::session::install::{install_universal_session, InstallSessionOpts};
use crate::universal::{Provider, UniversalSession};

#[derive(Debug)]
pub struct CloneOpts {
    /// Override the target provider. Defaults to the source provider. Same-provider
    /// clone copies provider-native storage; cross-provider clone installs a
    /// two-message context wrapper in the target provider's native live store.
    pub to: Option<Provider>,
    /// Override cwd on the new session. Defaults to the source cwd.
    pub cwd: Option<String>,
    /// If true and target already has a session with the new id, replace it.
    pub overwrite: bool,
    /// Override the new id (otherwise a fresh provider-native id is minted).
    pub new_id: Option<String>,
    /// How cross-provider clones hand off the source context.
    pub context_mode: CloneContextMode,
    /// Override `~/.cokacmux/context` for file-reference context clones.
    pub context_dir: Option<PathBuf>,
}

impl Default for CloneOpts {
    fn default() -> Self {
        Self {
            to: None,
            cwd: None,
            overwrite: false,
            new_id: None,
            context_mode: CloneContextMode::Inline,
            context_dir: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneContextMode {
    Inline,
    FileReference,
}

impl CloneContextMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::FileReference => "file_reference",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Inline => Self::FileReference,
            Self::FileReference => Self::Inline,
        }
    }
}

#[derive(Debug)]
pub struct CloneReport {
    pub source_provider: Provider,
    pub source_session_id: String,
    pub new_session_id: String,
    pub target_provider: Provider,
    pub new_cwd: String,
    pub artifact: ArtifactPath,
}

#[derive(Debug)]
pub enum ArtifactPath {
    File(PathBuf),
    OpenCodeDb {
        db_path: PathBuf,
        session_id: String,
    },
}

enum JsonLine {
    Blank,
    Json(Value),
}

/// Clone the session described by `src` into live storage with a new session id.
/// Same-provider targets preserve native storage; cross-provider targets are
/// prepared as a two-message context handoff in the target provider's format.
pub fn clone_to_live(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    crate::debug::log(
        "clone_to_live_start",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": opts.to.map(|p| p.as_str()),
            "cwd_override": opts.cwd.as_deref(),
            "overwrite": opts.overwrite,
            "new_id_provided": opts.new_id.is_some(),
            "context_mode": opts.context_mode.as_str(),
        }),
    );

    let target_provider = opts.to.unwrap_or(src.provider);
    if target_provider != src.provider {
        return clone_cross_provider_context_wrapper(src, target_provider, opts);
    }

    match src.provider {
        Provider::Claude => clone_claude_same_provider(src, opts),
        Provider::Codex => clone_codex_same_provider(src, opts),
        Provider::OpenCode => {
            #[cfg(feature = "opencode")]
            {
                clone_opencode_same_provider(src, opts)
            }
            #[cfg(not(feature = "opencode"))]
            {
                Err(ConvertError::Unsupported(
                    "opencode feature not enabled".into(),
                ))
            }
        }
        Provider::Pi => clone_pi_same_provider(src, opts),
        Provider::Gjc => clone_gjc_same_provider(src, opts),
    }
}

pub fn mint_session_id_for(target: Provider) -> String {
    mint_id_for(target)
}

fn clone_cross_provider_context_wrapper(
    src: &SessionInfo,
    target_provider: Provider,
    opts: &CloneOpts,
) -> Result<CloneReport> {
    clone_cross_provider_context_wrapper_with_install_opts(
        src,
        target_provider,
        opts,
        &InstallSessionOpts {
            overwrite: opts.overwrite,
            ..Default::default()
        },
    )
}

fn clone_cross_provider_context_wrapper_with_install_opts(
    src: &SessionInfo,
    target_provider: Provider,
    opts: &CloneOpts,
    install_opts: &InstallSessionOpts,
) -> Result<CloneReport> {
    if let Some(new_id) = opts.new_id.as_deref() {
        // Validate before file-reference mode creates a persistent context
        // file. An invalid id must be a side-effect-free error.
        ensure_native_session_id_for(target_provider, new_id)?;
    }
    let source_session = super::load(src)?;
    let context_file_path = match opts.context_mode {
        CloneContextMode::Inline => None,
        CloneContextMode::FileReference => Some(write_context_reference_file(
            src,
            target_provider,
            &source_session,
            opts.context_dir.as_deref(),
        )?),
    };
    let mut wrapped = if let Some(path) = context_file_path.as_ref() {
        wrap_session_for_context_file_reference(&source_session, target_provider, path)
    } else {
        wrap_session_for_context_convert(&source_session, target_provider)
    };
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| src.cwd.clone());
    if !new_cwd.is_empty() {
        wrapped.cwd = new_cwd.clone();
    }
    if let Some(new_id) = opts.new_id.clone() {
        wrapped.session_id = new_id.clone();
        if let Some(context) = wrapped
            .extras
            .get_mut("context_convert")
            .and_then(Value::as_object_mut)
        {
            context.insert("target_session_id".into(), Value::String(new_id));
        }
    }

    let install = match install_universal_session(target_provider, &wrapped, install_opts) {
        Ok(install) => install,
        Err(error) => {
            if let Some(path) = context_file_path.as_ref() {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
    };
    crate::debug::log(
        "clone_cross_provider_context_wrapper_ok",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": target_provider.as_str(),
            "new_session_id": &install.session_id,
            "artifact": format!("{:?}", &install.artifact),
            "messages": wrapped.messages.len(),
            "context_mode": opts.context_mode.as_str(),
            "context_file_path": context_file_path.as_ref().map(|path| path.display().to_string()),
            "native_validation_checks": install.validation.checks.len(),
        }),
    );
    Ok(CloneReport {
        source_provider: src.provider,
        source_session_id: src.session_id.clone(),
        new_session_id: install.session_id,
        target_provider,
        new_cwd,
        artifact: install.artifact,
    })
}

fn write_context_reference_file(
    src: &SessionInfo,
    target_provider: Provider,
    source_session: &crate::universal::UniversalSession,
    context_dir: Option<&Path>,
) -> Result<PathBuf> {
    let dir = match context_dir {
        Some(path) => path.to_path_buf(),
        None => default_context_dir()?,
    };
    fs::create_dir_all(&dir)?;
    if context_dir.is_none() {
        set_private_context_dir_permissions(&dir)?;
    }
    let filename = format!(
        "{}-{}-to-{}-{}.md",
        src.provider.as_str(),
        safe_path_component(&src.session_id),
        target_provider.as_str(),
        safe_path_component(&crate::ids::new_uuid_v7()),
    );
    let path = dir.join(filename);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(context_file_contents(source_session).as_bytes())?;
    file.sync_all()?;
    crate::debug::log(
        "clone_context_file_written",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": target_provider.as_str(),
            "path": path.display().to_string(),
        }),
    );
    Ok(path)
}

fn default_context_dir() -> Result<PathBuf> {
    Ok(crate::providers::discovery::home_dir()?
        .join(".cokacmux")
        .join("context"))
}

#[cfg(unix)]
fn set_private_context_dir_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_context_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn clone_claude_same_provider(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    let home = infer_claude_home_from_jsonl(&src.source)
        .or_else(default_claude_home)
        .ok_or_else(|| ConvertError::Other("could not determine claude home".into()))?;
    clone_claude_same_provider_at_home(src, opts, &home)
}

fn clone_claude_same_provider_at_home(
    src: &SessionInfo,
    opts: &CloneOpts,
    claude_home: &Path,
) -> Result<CloneReport> {
    if !src.source.is_file() {
        return Err(ConvertError::Other(format!(
            "claude source JSONL not found: {}",
            src.source.display()
        )));
    }
    let new_id = opts
        .new_id
        .clone()
        .unwrap_or_else(|| mint_id_for(Provider::Claude));
    ensure_distinct_native_clone_id(Provider::Claude, &src.session_id, &new_id)?;
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| src.cwd.clone());
    if new_cwd.is_empty() {
        return Err(ConvertError::MissingField("session.cwd"));
    }
    let target = claude_jsonl_path(claude_home, &new_cwd, &new_id);
    ensure_claude_sidecar_target_available(&src.source, &target, opts.overwrite)?;
    let mut lines = read_jsonl_lines(&src.source)?;
    let id_map = claude_line_uuid_map(&lines);
    patch_claude_jsonl_lines(
        &mut lines,
        &src.session_id,
        &new_id,
        &src.cwd,
        &new_cwd,
        &id_map,
    );
    let rewritten_sidecar_refs =
        rewrite_claude_sidecar_references(&mut lines, &src.source, &target);
    let repaired = repair_claude_parent_chain(&mut lines);
    let sanitized = sanitize_claude_content_blocks(&mut lines);
    let rollback =
        ClonePathRollback::capture(&[target.clone(), target.with_extension("")], opts.overwrite)?;
    let bytes_written = match write_jsonl_lines_atomic(&target, opts.overwrite, &lines) {
        Ok(bytes_written) => bytes_written,
        Err(error) => return Err(error_after_clone_rollback(error, rollback)),
    };
    let artifact = ArtifactPath::File(target.clone());
    if let Err(error) = clone_claude_sidecar(&src.source, &target, opts.overwrite) {
        return Err(error_after_clone_rollback(error, rollback));
    }
    let validation = match ensure_clone_artifact_native_or_cleanup(
        Provider::Claude,
        &new_id,
        &new_cwd,
        &artifact,
        opts,
    ) {
        Ok(validation) => validation,
        Err(error) => return Err(error_after_clone_rollback(error, rollback)),
    };
    rollback.commit();
    crate::debug::log(
        "clone_to_live_ok",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": Provider::Claude.as_str(),
            "new_session_id": &new_id,
            "artifact": format!("{:?}", &artifact),
            "path": "claude_native_jsonl_copy",
            "bytes_written": bytes_written,
            "uuid_refs": id_map.len(),
            "parent_chain_rows": repaired,
            "sanitized_content_rows": sanitized,
            "rewritten_sidecar_refs": rewritten_sidecar_refs,
            "native_validation_checks": validation.checks.len(),
        }),
    );
    Ok(CloneReport {
        source_provider: Provider::Claude,
        source_session_id: src.session_id.clone(),
        new_session_id: new_id,
        target_provider: Provider::Claude,
        new_cwd,
        artifact,
    })
}

fn clone_codex_same_provider(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    let home = infer_codex_home_from_rollout(&src.source)
        .or_else(default_codex_home)
        .ok_or_else(|| ConvertError::Other("could not determine codex home".into()))?;
    clone_codex_same_provider_at_home(src, opts, &home)
}

fn clone_codex_same_provider_at_home(
    src: &SessionInfo,
    opts: &CloneOpts,
    codex_home: &Path,
) -> Result<CloneReport> {
    if !src.source.is_file() {
        return Err(ConvertError::Other(format!(
            "codex source rollout not found: {}",
            src.source.display()
        )));
    }
    let new_id = opts
        .new_id
        .clone()
        .unwrap_or_else(|| mint_id_for(Provider::Codex));
    ensure_distinct_native_clone_id(Provider::Codex, &src.session_id, &new_id)?;
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| src.cwd.clone());
    if new_cwd.is_empty() {
        return Err(ConvertError::MissingField("session.cwd"));
    }
    let mut target_identity = UniversalSession::new(&new_id, Provider::Codex, &new_cwd);
    target_identity.created_at = Some(Utc::now());
    let target = providers::codex::install::planned_install(
        &target_identity,
        &providers::codex::install::InstallOpts {
            codex_home: Some(codex_home.to_path_buf()),
            overwrite: opts.overwrite,
            update_index: false,
            state_5_path: None,
        },
    )?
    .rollout_path;
    let mut lines = read_jsonl_lines(&src.source)?;
    patch_codex_jsonl_lines(&mut lines, &src.session_id, &new_id, &src.cwd, &new_cwd);
    let rollback = ClonePathRollback::capture(std::slice::from_ref(&target), opts.overwrite)?;
    let bytes_written = match write_jsonl_lines_atomic(&target, opts.overwrite, &lines) {
        Ok(bytes_written) => bytes_written,
        Err(error) => return Err(error_after_clone_rollback(error, rollback)),
    };
    let artifact = ArtifactPath::File(target.clone());

    let state_rollback = match copy_codex_state_thread_row(
        codex_home,
        &src.session_id,
        &new_id,
        &target,
        &new_cwd,
        opts.overwrite,
    ) {
        Ok(state_rollback) => state_rollback,
        Err(error) => return Err(error_after_clone_rollback(error, rollback)),
    };

    let validation = match ensure_clone_artifact_native_or_cleanup(
        Provider::Codex,
        &new_id,
        &new_cwd,
        &artifact,
        opts,
    ) {
        Ok(validation) => validation,
        Err(error) => {
            let state_error = state_rollback.rollback().err();
            let file_error = rollback.rollback().err();
            return Err(error_with_rollback_failures(
                error,
                state_error.into_iter().chain(file_error),
            ));
        }
    };
    state_rollback.commit();
    rollback.commit();
    crate::debug::log(
        "clone_to_live_ok",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": Provider::Codex.as_str(),
            "new_session_id": &new_id,
            "artifact": format!("{:?}", &artifact),
            "path": "codex_native_rollout_copy",
            "bytes_written": bytes_written,
            "native_validation_checks": validation.checks.len(),
        }),
    );
    Ok(CloneReport {
        source_provider: Provider::Codex,
        source_session_id: src.session_id.clone(),
        new_session_id: new_id,
        target_provider: Provider::Codex,
        new_cwd,
        artifact,
    })
}

/// Same-provider OpenCode clone: SQL row-level copy. Preserves every column
/// of every row in the origin session, including provider-specific columns
/// and every internal JSON field. Only database identifiers and id references
/// that must become unique are remapped.
#[cfg(feature = "opencode")]
fn clone_opencode_same_provider(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    if let Some(new_id) = opts.new_id.as_deref() {
        ensure_distinct_native_clone_id(Provider::OpenCode, &src.session_id, new_id)?;
    }
    let (report, validation) = providers::opencode::clone::clone_session_rows_with_validation(
        &src.source,
        &src.session_id,
        &providers::opencode::clone::OpenCodeRowCloneOpts {
            new_session_id: opts.new_id.clone(),
            cwd: opts.cwd.clone(),
            overwrite: opts.overwrite,
        },
        |conn, new_session_id| {
            let validation = super::native_validate::validate_opencode_connection(
                &src.source,
                new_session_id,
                conn,
            );
            if validation.ok {
                Ok(validation)
            } else {
                Err(ConvertError::Other(format!(
                    "{} clone artifact failed native validation: {}",
                    Provider::OpenCode.as_str(),
                    validation.failure_summary()
                )))
            }
        },
    )?;
    let new_id = report.new_session_id.clone();
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| src.cwd.clone());
    let artifact = ArtifactPath::OpenCodeDb {
        db_path: report.db_path,
        session_id: new_id.clone(),
    };
    crate::debug::log(
        "clone_to_live_ok",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": Provider::OpenCode.as_str(),
            "new_session_id": &new_id,
            "artifact": format!("{:?}", &artifact),
            "native_validation_checks": validation.checks.len(),
            "path": "opencode_row_copy",
            "messages_copied": report.messages_copied,
            "parts_copied": report.parts_copied,
            "session_messages_copied": report.session_messages_copied,
        }),
    );
    Ok(CloneReport {
        source_provider: Provider::OpenCode,
        source_session_id: src.session_id.clone(),
        new_session_id: new_id,
        target_provider: Provider::OpenCode,
        new_cwd,
        artifact,
    })
}

#[cfg(feature = "pi")]
fn clone_pi_same_provider(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    if !src.source.is_file() {
        return Err(ConvertError::Other(format!(
            "pi source JSONL not found: {}",
            src.source.display()
        )));
    }
    let new_id = opts
        .new_id
        .clone()
        .unwrap_or_else(|| mint_id_for(Provider::Pi));
    ensure_distinct_native_clone_id(Provider::Pi, &src.session_id, &new_id)?;
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| src.cwd.clone());
    if new_cwd.is_empty() {
        return Err(ConvertError::MissingField("session.cwd"));
    }

    let mut session = super::load(src)?;
    session.session_id = new_id.clone();
    session.cwd = new_cwd.clone();
    session.created_at = Some(Utc::now());
    session.updated_at = session.created_at;
    session.extras.insert(
        "pi_parent_session".into(),
        serde_json::Value::String(
            src.source
                .canonicalize()
                .unwrap_or_else(|_| src.source.clone())
                .display()
                .to_string(),
        ),
    );

    let native_opts = pi_install_opts_from_source(&src.source, opts.overwrite);
    let install = install_universal_session(
        Provider::Pi,
        &session,
        &InstallSessionOpts {
            overwrite: opts.overwrite,
            pi_agent_dir: native_opts.pi_agent_dir,
            pi_session_dir: native_opts.pi_session_dir,
            ..Default::default()
        },
    )?;
    let artifact = install.artifact;
    let validation = install.validation;
    crate::debug::log(
        "clone_to_live_ok",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": Provider::Pi.as_str(),
            "new_session_id": &new_id,
            "artifact": format!("{:?}", &artifact),
            "native_validation_checks": validation.checks.len(),
            "path": "pi_native_jsonl_replay",
        }),
    );
    Ok(CloneReport {
        source_provider: Provider::Pi,
        source_session_id: src.session_id.clone(),
        new_session_id: new_id,
        target_provider: Provider::Pi,
        new_cwd,
        artifact,
    })
}

#[cfg(not(feature = "pi"))]
fn clone_pi_same_provider(_src: &SessionInfo, _opts: &CloneOpts) -> Result<CloneReport> {
    Err(ConvertError::Unsupported("pi feature not enabled".into()))
}

#[cfg(feature = "gjc")]
fn clone_gjc_same_provider(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    if !src.source.is_file() {
        return Err(ConvertError::Other(format!(
            "gjc source JSONL not found: {}",
            src.source.display()
        )));
    }
    let new_id = opts
        .new_id
        .clone()
        .unwrap_or_else(|| mint_id_for(Provider::Gjc));
    ensure_distinct_native_clone_id(Provider::Gjc, &src.session_id, &new_id)?;
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| src.cwd.clone());
    if new_cwd.is_empty() {
        return Err(ConvertError::MissingField("session.cwd"));
    }

    let mut session = super::load(src)?;
    session.session_id = new_id.clone();
    session.cwd = new_cwd.clone();
    session.created_at = Some(Utc::now());
    session.updated_at = session.created_at;
    session.extras.insert(
        "gjc_parent_session".into(),
        serde_json::Value::String(
            src.source
                .canonicalize()
                .unwrap_or_else(|_| src.source.clone())
                .display()
                .to_string(),
        ),
    );

    let native_opts = gjc_install_opts_from_source(&src.source, opts.overwrite);
    let install = install_universal_session(
        Provider::Gjc,
        &session,
        &InstallSessionOpts {
            overwrite: opts.overwrite,
            gjc_agent_dir: native_opts.gjc_agent_dir,
            gjc_session_dir: native_opts.gjc_session_dir,
            ..Default::default()
        },
    )?;
    let artifact = install.artifact;
    let validation = install.validation;
    crate::debug::log(
        "clone_to_live_ok",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": Provider::Gjc.as_str(),
            "new_session_id": &new_id,
            "artifact": format!("{:?}", &artifact),
            "native_validation_checks": validation.checks.len(),
            "path": "gjc_native_jsonl_replay",
        }),
    );
    Ok(CloneReport {
        source_provider: Provider::Gjc,
        source_session_id: src.session_id.clone(),
        new_session_id: new_id,
        target_provider: Provider::Gjc,
        new_cwd,
        artifact,
    })
}

#[cfg(not(feature = "gjc"))]
fn clone_gjc_same_provider(_src: &SessionInfo, _opts: &CloneOpts) -> Result<CloneReport> {
    Err(ConvertError::Unsupported("gjc feature not enabled".into()))
}

#[cfg(feature = "pi")]
fn pi_install_opts_from_source(
    source: &Path,
    overwrite: bool,
) -> providers::pi::install::InstallOpts {
    let Some(parent) = source.parent() else {
        return providers::pi::install::InstallOpts {
            overwrite,
            ..Default::default()
        };
    };
    let parent_name = parent
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if parent_name.starts_with("--") && parent_name.ends_with("--") {
        if let Some(sessions_dir) = parent.parent() {
            if sessions_dir.file_name().and_then(|name| name.to_str()) == Some("sessions") {
                if let Some(agent_dir) = sessions_dir.parent() {
                    return providers::pi::install::InstallOpts {
                        pi_agent_dir: Some(agent_dir.to_path_buf()),
                        overwrite,
                        ..Default::default()
                    };
                }
            }
        }
    }
    providers::pi::install::InstallOpts {
        pi_session_dir: Some(parent.to_path_buf()),
        overwrite,
        ..Default::default()
    }
}

#[cfg(feature = "gjc")]
fn gjc_install_opts_from_source(
    source: &Path,
    overwrite: bool,
) -> providers::gjc::install::InstallOpts {
    let Some(parent) = source.parent() else {
        return providers::gjc::install::InstallOpts {
            overwrite,
            ..Default::default()
        };
    };
    if let Some(sessions_dir) = parent.parent() {
        if sessions_dir.file_name().and_then(|name| name.to_str()) == Some("sessions") {
            if let Some(agent_dir) = sessions_dir.parent() {
                return providers::gjc::install::InstallOpts {
                    gjc_agent_dir: Some(agent_dir.to_path_buf()),
                    overwrite,
                    ..Default::default()
                };
            }
        }
    }
    providers::gjc::install::InstallOpts {
        gjc_session_dir: Some(parent.to_path_buf()),
        overwrite,
        ..Default::default()
    }
}

fn read_jsonl_lines(path: &Path) -> Result<Vec<JsonLine>> {
    let file = File::open(path)?;
    let mut lines = Vec::new();
    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            lines.push(JsonLine::Blank);
            continue;
        }
        let value: Value = serde_json::from_str(&line).map_err(|error| {
            ConvertError::Other(format!(
                "failed to parse JSONL line {} in {}: {}",
                idx + 1,
                path.display(),
                error
            ))
        })?;
        lines.push(JsonLine::Json(value));
    }
    Ok(lines)
}

#[derive(Debug)]
enum ClonePathPreviousState {
    Absent,
    Backup(PathBuf),
    Untouched,
}

#[derive(Debug)]
struct ClonePathRollbackEntry {
    path: PathBuf,
    previous: ClonePathPreviousState,
}

/// Keeps a private, complete copy of every overwritten native artifact until
/// all provider-specific follow-up work and validation succeeds. This is a
/// small local transaction across files/directories; an error restores the
/// previous user data instead of deleting the newly written path and losing
/// both generations.
#[derive(Debug)]
pub(super) struct ClonePathRollback {
    entries: Vec<ClonePathRollbackEntry>,
}

impl ClonePathRollback {
    pub(super) fn capture(paths: &[PathBuf], overwrite: bool) -> Result<Self> {
        let mut entries = Vec::with_capacity(paths.len());
        for path in paths {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => Some(metadata),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    cleanup_clone_rollback_backups(&entries);
                    return Err(error.into());
                }
            };
            let previous = if metadata.is_none() {
                ClonePathPreviousState::Absent
            } else if !overwrite {
                ClonePathPreviousState::Untouched
            } else {
                let backup = clone_rollback_backup_path(path);
                if let Err(error) = copy_path_for_clone_rollback(path, &backup) {
                    let _ = remove_clone_path_entry(&backup);
                    cleanup_clone_rollback_backups(&entries);
                    return Err(error);
                }
                ClonePathPreviousState::Backup(backup)
            };
            entries.push(ClonePathRollbackEntry {
                path: path.clone(),
                previous,
            });
        }
        Ok(Self { entries })
    }

    pub(super) fn rollback(self) -> Result<()> {
        let mut failures = Vec::new();
        for entry in self.entries.into_iter().rev() {
            match entry.previous {
                ClonePathPreviousState::Absent => {
                    if let Err(error) = remove_clone_path_entry(&entry.path) {
                        failures.push(format!("remove {} failed: {error}", entry.path.display()));
                    }
                }
                ClonePathPreviousState::Backup(backup) => {
                    if let Err(error) = remove_clone_path_entry(&entry.path) {
                        failures.push(format!("remove {} failed: {error}", entry.path.display()));
                        continue;
                    }
                    if let Err(error) = fs::rename(&backup, &entry.path) {
                        failures.push(format!(
                            "restore {} from {} failed: {error}",
                            entry.path.display(),
                            backup.display()
                        ));
                    }
                }
                ClonePathPreviousState::Untouched => {}
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ConvertError::Other(failures.join("; ")))
        }
    }

    pub(super) fn commit(self) {
        cleanup_clone_rollback_backups(&self.entries);
    }
}

fn clone_rollback_backup_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("session");
    path.with_file_name(format!(
        ".{name}.rollback-{}",
        uuid::Uuid::now_v7().simple()
    ))
}

fn copy_path_for_clone_rollback(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_dir() && !file_type.is_symlink() {
        copy_dir_private(source, target)?;
    } else if file_type.is_symlink() {
        copy_symlink(source, target)?;
    } else if file_type.is_file() {
        copy_file_private(source, target)?;
    } else {
        return Err(ConvertError::Other(format!(
            "unsupported clone rollback artifact: {}",
            source.display()
        )));
    }
    Ok(())
}

fn copy_dir_private(source: &Path, target: &Path) -> Result<()> {
    create_private_dir(target)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() && !file_type.is_symlink() {
            copy_dir_private(&source_path, &target_path)?;
        } else if file_type.is_symlink() {
            copy_symlink(&source_path, &target_path)?;
        } else if file_type.is_file() {
            copy_file_private(&source_path, &target_path)?;
        } else {
            return Err(ConvertError::Other(format!(
                "unsupported filesystem entry while cloning sidecar or rollback data: {}",
                source_path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    Ok(())
}

fn copy_file_private(source: &Path, target: &Path) -> Result<()> {
    let mut source_file = File::open(source)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut target_file = options.open(target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        target_file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    std::io::copy(&mut source_file, &mut target_file)?;
    target_file.sync_all()?;
    Ok(())
}

pub(super) fn remove_clone_path_entry(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn clone_path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn cleanup_clone_rollback_backups(entries: &[ClonePathRollbackEntry]) {
    for entry in entries {
        if let ClonePathPreviousState::Backup(backup) = &entry.previous {
            if let Err(error) = remove_clone_path_entry(backup) {
                crate::debug::log(
                    "clone_rollback_backup_cleanup_failed",
                    serde_json::json!({
                        "path": entry.path.display().to_string(),
                        "backup": backup.display().to_string(),
                        "error": error.to_string(),
                    }),
                );
            }
        }
    }
}

fn error_after_clone_rollback(error: ConvertError, rollback: ClonePathRollback) -> ConvertError {
    let rollback_error = rollback.rollback().err();
    error_with_rollback_failures(error, rollback_error)
}

fn error_with_rollback_failures<I>(error: ConvertError, rollback_errors: I) -> ConvertError
where
    I: IntoIterator<Item = ConvertError>,
{
    let failures = rollback_errors
        .into_iter()
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if failures.is_empty() {
        error
    } else {
        ConvertError::Other(format!(
            "{error}; clone rollback failed: {}",
            failures.join("; ")
        ))
    }
}

fn write_jsonl_lines_atomic(path: &Path, overwrite: bool, lines: &[JsonLine]) -> Result<u64> {
    if clone_path_exists(path)? && !overwrite {
        return Err(ConvertError::Other(format!(
            "clone target already exists at {} (set overwrite=true to replace)",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("clone.jsonl");
    let tmp_path = path.with_file_name(format!(".{}.tmp-{}", file_name, uuid::Uuid::now_v7()));
    let result = (|| -> Result<u64> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp_path)?;
        for line in lines {
            match line {
                JsonLine::Blank => {
                    writeln!(file)?;
                }
                JsonLine::Json(value) => {
                    serde_json::to_writer(&mut file, value)?;
                    writeln!(file)?;
                }
            }
        }
        file.sync_all()?;
        publish_clone_temp_file(&tmp_path, path, overwrite)?;
        if let Some(parent) = path.parent() {
            let _ = File::open(parent).and_then(|dir| dir.sync_all());
        }
        Ok(fs::metadata(path).map(|m| m.len()).unwrap_or(0))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// Publish a complete temporary file without ever deleting the previous
/// destination first. Unix normally replaces in one rename. Platforms that
/// reject replacement get a same-directory displacement/restore fallback.
fn publish_clone_temp_file(tmp_path: &Path, path: &Path, overwrite: bool) -> Result<()> {
    if !overwrite {
        // A hard-link publication is an atomic create-if-absent operation.
        // Unlike a preflight exists check followed by rename, it cannot
        // clobber a destination created concurrently (including a symlink).
        fs::hard_link(tmp_path, path)?;
        fs::remove_file(tmp_path)?;
        return Ok(());
    }

    let first_error = match fs::rename(tmp_path, path) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if !clone_path_exists(path)? {
        return Err(first_error.into());
    }

    let displaced = clone_rollback_backup_path(path);
    fs::rename(path, &displaced)?;
    match fs::rename(tmp_path, path) {
        Ok(()) => {
            if let Err(error) = remove_clone_path_entry(&displaced) {
                crate::debug::log(
                    "clone_displaced_file_cleanup_failed",
                    serde_json::json!({
                        "path": path.display().to_string(),
                        "displaced": displaced.display().to_string(),
                        "error": error.to_string(),
                    }),
                );
            }
            Ok(())
        }
        Err(publish_error) => match fs::rename(&displaced, path) {
            Ok(()) => Err(publish_error.into()),
            Err(restore_error) => Err(ConvertError::Other(format!(
                "failed to publish {}: {}; failed to restore previous destination from {}: {}",
                path.display(),
                publish_error,
                displaced.display(),
                restore_error
            ))),
        },
    }
}

fn claude_line_uuid_map(lines: &[JsonLine]) -> HashMap<String, String> {
    let mut id_map = HashMap::new();
    for line in lines {
        let JsonLine::Json(Value::Object(map)) = line else {
            continue;
        };
        if let Some(uuid) = map.get("uuid").and_then(Value::as_str) {
            id_map
                .entry(uuid.to_string())
                .or_insert_with(|| uuid::Uuid::now_v7().to_string());
        }
    }
    id_map
}

fn patch_claude_jsonl_lines(
    lines: &mut [JsonLine],
    old_sid: &str,
    new_sid: &str,
    old_cwd: &str,
    new_cwd: &str,
    id_map: &HashMap<String, String>,
) {
    for line in lines {
        let JsonLine::Json(Value::Object(map)) = line else {
            continue;
        };
        rewrite_string_if_equal(map, "sessionId", old_sid, new_sid);
        rewrite_string_if_equal(map, "cwd", old_cwd, new_cwd);
        rewrite_mapped_string(map, "uuid", id_map);
        rewrite_mapped_string(map, "parentUuid", id_map);
        rewrite_mapped_string(map, "messageId", id_map);
        rewrite_mapped_string(map, "sourceToolAssistantUUID", id_map);
        rewrite_mapped_string(map, "leafUuid", id_map);
        if let Some(Value::Object(snapshot)) = map.get_mut("snapshot") {
            rewrite_mapped_string(snapshot, "messageId", id_map);
        }
    }
}

fn rewrite_claude_sidecar_references(
    lines: &mut [JsonLine],
    source_jsonl: &Path,
    target_jsonl: &Path,
) -> usize {
    let source_root = source_jsonl.with_extension("").join("tool-results");
    let target_root = target_jsonl.with_extension("").join("tool-results");
    let mut rewritten = 0usize;
    for line in lines {
        if let JsonLine::Json(value) = line {
            rewrite_claude_sidecar_references_in_value(
                value,
                &source_root,
                &target_root,
                &mut rewritten,
            );
        }
    }
    rewritten
}

fn rewrite_claude_sidecar_references_in_value(
    value: &mut Value,
    source_root: &Path,
    target_root: &Path,
    rewritten: &mut usize,
) {
    match value {
        Value::String(text) => {
            let (replacement, count) =
                rewrite_claude_sidecar_reference_text(text, source_root, target_root);
            if count > 0 {
                *text = replacement;
                *rewritten = rewritten.saturating_add(count);
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite_claude_sidecar_references_in_value(
                    value,
                    source_root,
                    target_root,
                    rewritten,
                );
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                rewrite_claude_sidecar_references_in_value(
                    value,
                    source_root,
                    target_root,
                    rewritten,
                );
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn rewrite_claude_sidecar_reference_text(
    text: &str,
    source_root: &Path,
    target_root: &Path,
) -> (String, usize) {
    const NEEDLE: &str = "Full output saved to: ";

    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut rewritten = 0usize;
    while let Some(offset) = rest.find(NEEDLE) {
        let path_start = offset + NEEDLE.len();
        out.push_str(&rest[..path_start]);
        let after = &rest[path_start..];
        let path_end = after.find(['\r', '\n']).unwrap_or(after.len());
        let candidate_text = after[..path_end].trim_end();
        let candidate = Path::new(candidate_text);
        let relative = candidate.strip_prefix(source_root).ok().filter(|relative| {
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
        });
        if let Some(relative) = relative {
            out.push_str(&target_root.join(relative).display().to_string());
            out.push_str(&after[candidate_text.len()..path_end]);
            rewritten = rewritten.saturating_add(1);
        } else {
            out.push_str(&after[..path_end]);
        }
        rest = &after[path_end..];
    }
    out.push_str(rest);
    (out, rewritten)
}

fn repair_claude_parent_chain(lines: &mut [JsonLine]) -> usize {
    let mut leaf_uuid: Option<String> = None;
    let mut updated = 0usize;
    let mut conversation_uuids = std::collections::HashSet::new();

    for line in lines.iter() {
        let JsonLine::Json(Value::Object(map)) = line else {
            continue;
        };
        let Some(kind) = map.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(kind, "user" | "assistant") {
            continue;
        }
        if map
            .get("isSidechain")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let Some(uuid) = map.get("uuid").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        conversation_uuids.insert(uuid.clone());
        leaf_uuid = Some(uuid);
    }

    let Some(fallback_leaf_uuid) = leaf_uuid else {
        return updated;
    };
    for line in lines {
        let JsonLine::Json(Value::Object(map)) = line else {
            continue;
        };
        if map.get("type").and_then(Value::as_str) == Some("last-prompt") {
            let current = map.get("leafUuid").and_then(Value::as_str);
            let needs_repair = current
                .map(|uuid| !conversation_uuids.contains(uuid))
                .unwrap_or(true);
            if needs_repair {
                map.insert("leafUuid".into(), Value::String(fallback_leaf_uuid.clone()));
                updated = updated.saturating_add(1);
            }
        }
    }
    updated
}

fn sanitize_claude_content_blocks(lines: &mut [JsonLine]) -> usize {
    let mut sanitized = 0usize;
    for line in lines {
        let JsonLine::Json(Value::Object(top)) = line else {
            continue;
        };
        if !matches!(
            top.get("type").and_then(Value::as_str),
            Some("user" | "assistant")
        ) {
            continue;
        }
        let Some(Value::Object(inner)) = top.get_mut("message") else {
            continue;
        };
        let Some(Value::Array(content)) = inner.get_mut("content") else {
            continue;
        };
        let before = content.len();
        content.retain(|block| {
            block
                .get("type")
                .and_then(Value::as_str)
                .map(is_claude_api_content_type)
                .unwrap_or(true)
        });
        if content.len() != before {
            sanitized = sanitized.saturating_add(1);
        }
    }
    sanitized
}

fn is_claude_api_content_type(kind: &str) -> bool {
    matches!(
        kind,
        "advisor_tool_result"
            | "bash_code_execution_tool_result"
            | "code_execution_tool_result"
            | "container_upload"
            | "document"
            | "image"
            | "redacted_thinking"
            | "search_result"
            | "server_tool_use"
            | "text"
            | "text_editor_code_execution_tool_result"
            | "thinking"
            | "tool_result"
            | "tool_search_tool_result"
            | "tool_use"
            | "web_fetch_tool_result"
            | "web_search_tool_result"
    )
}

fn patch_codex_jsonl_lines(
    lines: &mut [JsonLine],
    old_sid: &str,
    new_sid: &str,
    old_cwd: &str,
    new_cwd: &str,
) {
    for line in lines {
        let JsonLine::Json(Value::Object(map)) = line else {
            continue;
        };
        let Some(Value::Object(payload)) = map.get_mut("payload") else {
            continue;
        };
        rewrite_string_if_equal(payload, "id", old_sid, new_sid);
        rewrite_string_if_equal(payload, "cwd", old_cwd, new_cwd);
    }
}

fn rewrite_string_if_equal(
    map: &mut serde_json::Map<String, Value>,
    key: &str,
    old: &str,
    new: &str,
) {
    if let Some(Value::String(value)) = map.get_mut(key) {
        if value == old {
            *value = new.to_string();
        }
    }
}

fn rewrite_mapped_string(
    map: &mut serde_json::Map<String, Value>,
    key: &str,
    id_map: &HashMap<String, String>,
) {
    if let Some(Value::String(value)) = map.get_mut(key) {
        if let Some(new_id) = id_map.get(value.as_str()) {
            *value = new_id.clone();
        }
    }
}

fn claude_jsonl_path(claude_home: &Path, cwd: &str, session_id: &str) -> PathBuf {
    claude_home
        .join("projects")
        .join(providers::claude::path::encode_cwd(cwd))
        .join(format!("{session_id}.jsonl"))
}

fn clone_claude_sidecar(source_jsonl: &Path, target_jsonl: &Path, overwrite: bool) -> Result<()> {
    let source_sidecar = source_jsonl.with_extension("");
    let target_sidecar = target_jsonl.with_extension("");
    if !source_sidecar.is_dir() {
        if clone_path_exists(&target_sidecar)? {
            if !overwrite {
                return Err(ConvertError::Other(format!(
                    "claude sidecar target already exists at {} (set overwrite=true to replace)",
                    target_sidecar.display()
                )));
            }
            remove_clone_path_entry(&target_sidecar)?;
        }
        return Ok(());
    }
    if clone_path_exists(&target_sidecar)? {
        if !overwrite {
            return Err(ConvertError::Other(format!(
                "claude sidecar target already exists at {} (set overwrite=true to replace)",
                target_sidecar.display()
            )));
        }
    }
    let sidecar_name = target_sidecar
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sidecar");
    let tmp_sidecar =
        target_sidecar.with_file_name(format!(".{}.tmp-{}", sidecar_name, uuid::Uuid::now_v7()));
    let copy_result = copy_dir_private(&source_sidecar, &tmp_sidecar);
    if let Err(error) = copy_result {
        let _ = fs::remove_dir_all(&tmp_sidecar);
        return Err(error);
    }
    if clone_path_exists(&target_sidecar)? && overwrite {
        remove_clone_path_entry(&target_sidecar)?;
    }
    if let Err(error) = fs::rename(&tmp_sidecar, &target_sidecar) {
        let _ = fs::remove_dir_all(&tmp_sidecar);
        return Err(error.into());
    }
    Ok(())
}

fn ensure_claude_sidecar_target_available(
    _source_jsonl: &Path,
    target_jsonl: &Path,
    overwrite: bool,
) -> Result<()> {
    let target_sidecar = target_jsonl.with_extension("");
    if clone_path_exists(&target_sidecar)? && !overwrite {
        return Err(ConvertError::Other(format!(
            "claude sidecar target already exists at {} (set overwrite=true to replace)",
            target_sidecar.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source_path: &Path, target_path: &Path) -> Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source_path)?, target_path)?;
    Ok(())
}

#[cfg(windows)]
fn copy_symlink(source_path: &Path, target_path: &Path) -> Result<()> {
    let link_target = fs::read_link(source_path)?;
    if source_path.is_dir() {
        std::os::windows::fs::symlink_dir(link_target, target_path)?;
    } else {
        std::os::windows::fs::symlink_file(link_target, target_path)?;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn copy_symlink(source_path: &Path, target_path: &Path) -> Result<()> {
    fs::copy(source_path, target_path)?;
    Ok(())
}

fn infer_claude_home_from_jsonl(path: &Path) -> Option<PathBuf> {
    let project_dir = path.parent()?;
    let projects_dir = project_dir.parent()?;
    if projects_dir.file_name().and_then(|name| name.to_str()) != Some("projects") {
        return None;
    }
    projects_dir.parent().map(Path::to_path_buf)
}

fn infer_codex_home_from_rollout(path: &Path) -> Option<PathBuf> {
    let day = path.parent()?;
    let month = day.parent()?;
    let year = month.parent()?;
    let sessions = year.parent()?;
    if sessions.file_name().and_then(|name| name.to_str()) != Some("sessions") {
        return None;
    }
    sessions.parent().map(Path::to_path_buf)
}

#[cfg(feature = "discovery")]
fn default_claude_home() -> Option<PathBuf> {
    crate::providers::discovery::configured_home_dir().map(|home| home.join(".claude"))
}

#[cfg(not(feature = "discovery"))]
fn default_claude_home() -> Option<PathBuf> {
    None
}

#[cfg(feature = "discovery")]
fn default_codex_home() -> Option<PathBuf> {
    crate::providers::discovery::configured_home_dir().map(|home| home.join(".codex"))
}

#[cfg(not(feature = "discovery"))]
fn default_codex_home() -> Option<PathBuf> {
    None
}

fn safe_path_component(value: &str) -> String {
    if value.is_empty() {
        return "%EMPTY".to_string();
    }

    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_') {
            out.push(char::from(*byte));
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }
    out
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'A' + (nibble - 10)),
        _ => unreachable!("hex nibble is always <= 15"),
    }
}

#[cfg(feature = "opencode")]
#[derive(Debug)]
pub(super) struct CodexStateThreadRollback {
    state_5: Option<PathBuf>,
    columns: Vec<String>,
    new_session_id: String,
    previous_values: Option<Vec<rusqlite::types::Value>>,
}

#[cfg(feature = "opencode")]
impl CodexStateThreadRollback {
    pub(super) fn inactive() -> Self {
        Self {
            state_5: None,
            columns: Vec::new(),
            new_session_id: String::new(),
            previous_values: None,
        }
    }

    pub(super) fn rollback(self) -> Result<()> {
        let Some(state_5) = self.state_5 else {
            return Ok(());
        };
        let mut conn = rusqlite::Connection::open_with_flags(
            &state_5,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM threads WHERE id = ?1",
            rusqlite::params![self.new_session_id],
        )?;
        if let Some(previous_values) = self.previous_values {
            insert_codex_thread_values(&tx, &self.columns, &previous_values)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn commit(self) {}
}

#[cfg(not(feature = "opencode"))]
#[derive(Debug)]
pub(super) struct CodexStateThreadRollback;

#[cfg(not(feature = "opencode"))]
impl CodexStateThreadRollback {
    pub(super) fn inactive() -> Self {
        Self
    }

    pub(super) fn rollback(self) -> Result<()> {
        Ok(())
    }

    pub(super) fn commit(self) {}
}

/// Capture the exact pre-install Codex index row before a file overwrite is
/// allowed. The returned guard restores that row (or removes a newly-created
/// row) if publication or native validation later fails.
#[cfg(feature = "opencode")]
pub(super) fn capture_codex_state_thread_row(
    state_5: &Path,
    session_id: &str,
    overwrite: bool,
) -> Result<CodexStateThreadRollback> {
    if !state_5.exists() {
        return Ok(CodexStateThreadRollback::inactive());
    }
    let conn = rusqlite::Connection::open_with_flags(
        state_5,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    let columns = ordered_table_columns(&conn, "threads")?;
    if !columns.iter().any(|column| column == "id") {
        return Err(ConvertError::Other(
            "threads table missing expected column `id` (state_5.sqlite schema drift?)".into(),
        ));
    }
    let select_sql = format!(
        "SELECT {} FROM threads WHERE id = ?1",
        columns
            .iter()
            .map(|column| quote_sql_ident(column))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let previous_values = {
        let mut stmt = conn.prepare(&select_sql)?;
        match stmt.query_row(rusqlite::params![session_id], |row| {
            let mut values = Vec::with_capacity(columns.len());
            for idx in 0..columns.len() {
                values.push(row.get::<_, rusqlite::types::Value>(idx)?);
            }
            Ok(values)
        }) {
            Ok(values) => Some(values),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(error) => return Err(error.into()),
        }
    };
    if previous_values.is_some() && !overwrite {
        return Err(ConvertError::Other(format!(
            "codex state row already exists for {session_id} (set overwrite=true to replace)"
        )));
    }
    Ok(CodexStateThreadRollback {
        state_5: Some(state_5.to_path_buf()),
        columns,
        new_session_id: session_id.to_string(),
        previous_values,
    })
}

#[cfg(not(feature = "opencode"))]
pub(super) fn capture_codex_state_thread_row(
    _state_5: &Path,
    _session_id: &str,
    _overwrite: bool,
) -> Result<CodexStateThreadRollback> {
    Ok(CodexStateThreadRollback)
}

#[cfg(feature = "opencode")]
fn copy_codex_state_thread_row(
    codex_home: &Path,
    source_session_id: &str,
    new_session_id: &str,
    rollout_path: &Path,
    cwd: &str,
    overwrite: bool,
) -> Result<CodexStateThreadRollback> {
    use rusqlite::types::Value as SqlValue;

    let state_5 = codex_home.join("state_5.sqlite");
    if !state_5.is_file() {
        crate::debug::log(
            "codex_clone_state_index_skipped",
            serde_json::json!({
                "state_5": state_5.display().to_string(),
                "reason": "missing",
            }),
        );
        return Ok(CodexStateThreadRollback::inactive());
    }
    let mut conn = rusqlite::Connection::open_with_flags(
        &state_5,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    let columns = ordered_table_columns(&conn, "threads")?;
    for required in ["id", "rollout_path", "cwd"] {
        if !columns.iter().any(|column| column == required) {
            return Err(ConvertError::Other(format!(
                "threads table missing expected column `{required}` (state_5.sqlite schema drift?)"
            )));
        }
    }
    let tx = conn.transaction()?;
    let select_sql = format!(
        "SELECT {} FROM threads WHERE id = ?1",
        columns
            .iter()
            .map(|column| quote_sql_ident(column))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let previous_values =
        select_codex_thread_values(&tx, &select_sql, new_session_id, columns.len())?;
    if previous_values.is_some() && !overwrite {
        return Err(ConvertError::Other(format!(
            "codex state row already exists for {} (set overwrite=true to replace)",
            new_session_id
        )));
    }
    let mut values =
        select_codex_thread_values(&tx, &select_sql, source_session_id, columns.len())?
            .ok_or_else(|| {
                ConvertError::Other(format!(
                    "source Codex state row not found for {}; native clone cannot rebuild it",
                    source_session_id
                ))
            })?;

    for (column, value) in columns.iter().zip(values.iter_mut()) {
        match column.as_str() {
            "id" => *value = SqlValue::Text(new_session_id.to_string()),
            "rollout_path" => *value = SqlValue::Text(rollout_path.display().to_string()),
            "cwd" => *value = SqlValue::Text(cwd.to_string()),
            _ => {}
        }
    }

    if previous_values.is_some() {
        tx.execute(
            "DELETE FROM threads WHERE id = ?1",
            rusqlite::params![new_session_id],
        )?;
    }
    insert_codex_thread_values(&tx, &columns, &values)?;
    tx.commit()?;
    crate::debug::log(
        "codex_clone_state_index_ok",
        serde_json::json!({
            "state_5": state_5.display().to_string(),
            "source_session_id": source_session_id,
            "new_session_id": new_session_id,
            "rollout_path": rollout_path.display().to_string(),
        }),
    );
    Ok(CodexStateThreadRollback {
        state_5: Some(state_5),
        columns,
        new_session_id: new_session_id.to_string(),
        previous_values,
    })
}

#[cfg(not(feature = "opencode"))]
fn copy_codex_state_thread_row(
    _codex_home: &Path,
    _source_session_id: &str,
    _new_session_id: &str,
    _rollout_path: &Path,
    _cwd: &str,
    _overwrite: bool,
) -> Result<CodexStateThreadRollback> {
    Ok(CodexStateThreadRollback)
}

#[cfg(feature = "opencode")]
fn select_codex_thread_values(
    tx: &rusqlite::Transaction<'_>,
    select_sql: &str,
    session_id: &str,
    column_count: usize,
) -> Result<Option<Vec<rusqlite::types::Value>>> {
    use rusqlite::types::Value as SqlValue;

    let mut stmt = tx.prepare(select_sql)?;
    match stmt.query_row(rusqlite::params![session_id], |row| {
        let mut values = Vec::with_capacity(column_count);
        for idx in 0..column_count {
            values.push(row.get::<_, SqlValue>(idx)?);
        }
        Ok(values)
    }) {
        Ok(values) => Ok(Some(values)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(feature = "opencode")]
fn insert_codex_thread_values(
    tx: &rusqlite::Transaction<'_>,
    columns: &[String],
    values: &[rusqlite::types::Value],
) -> Result<()> {
    let placeholders = (1..=columns.len())
        .map(|idx| format!("?{idx}"))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO threads ({}) VALUES ({})",
        columns
            .iter()
            .map(|column| quote_sql_ident(column))
            .collect::<Vec<_>>()
            .join(", "),
        placeholders
    );
    tx.execute(&insert_sql, rusqlite::params_from_iter(values.iter()))?;
    Ok(())
}

#[cfg(feature = "opencode")]
fn ordered_table_columns(conn: &rusqlite::Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", quote_sql_ident(table)))?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if names.is_empty() {
        return Err(ConvertError::Other(format!(
            "table `{table}` not found or has no columns"
        )));
    }
    Ok(names)
}

#[cfg(feature = "opencode")]
fn quote_sql_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn ensure_clone_artifact_native_or_cleanup(
    target_provider: Provider,
    session_id: &str,
    cwd: &str,
    artifact: &ArtifactPath,
    opts: &CloneOpts,
) -> Result<super::native_validate::NativeValidationReport> {
    match super::native_validate::ensure_clone_artifact_native(
        target_provider,
        session_id,
        artifact,
    ) {
        Ok(validation) => Ok(validation),
        Err(validation_error) => {
            let cleanup_error = if opts.overwrite {
                crate::debug::log(
                    "clone_validation_failed_outer_rollback_required",
                    serde_json::json!({
                        "target_provider": target_provider.as_str(),
                        "session_id": session_id,
                        "artifact": format!("{:?}", artifact),
                        "reason": "previous artifact is owned by the caller's overwrite rollback guard",
                        "error": validation_error.to_string(),
                    }),
                );
                None
            } else {
                match remove_installed_clone_artifact(target_provider, session_id, cwd, artifact) {
                    Ok(report) => {
                        crate::debug::log(
                            "clone_validation_failed_cleanup_ok",
                            serde_json::json!({
                                "target_provider": target_provider.as_str(),
                                "session_id": session_id,
                                "artifact": format!("{:?}", artifact),
                                "deleted_file": report
                                    .deleted_file
                                    .map(|path| path.display().to_string()),
                                "deleted_rows": report.deleted_rows,
                                "error": validation_error.to_string(),
                            }),
                        );
                        None
                    }
                    Err(cleanup_error) => {
                        let cleanup_error = cleanup_error.to_string();
                        crate::debug::log(
                            "clone_validation_failed_cleanup_error",
                            serde_json::json!({
                                "target_provider": target_provider.as_str(),
                                "session_id": session_id,
                                "artifact": format!("{:?}", artifact),
                                "validation_error": validation_error.to_string(),
                                "cleanup_error": cleanup_error,
                            }),
                        );
                        Some(cleanup_error)
                    }
                }
            };

            if let Some(cleanup_error) = cleanup_error {
                Err(ConvertError::Other(format!(
                    "{}; cleanup failed: {}",
                    validation_error, cleanup_error
                )))
            } else {
                Err(validation_error)
            }
        }
    }
}

fn remove_installed_clone_artifact(
    provider: Provider,
    session_id: &str,
    cwd: &str,
    artifact: &ArtifactPath,
) -> Result<super::remove::RemoveReport> {
    let source = match artifact {
        ArtifactPath::File(path) => path.clone(),
        ArtifactPath::OpenCodeDb { db_path, .. } => db_path.clone(),
    };
    let info = SessionInfo {
        provider,
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
        source,
        updated_at_epoch_s: 0,
        title: None,
        relation: None,
    };
    super::remove::remove(&info)
}

fn mint_id_for(target: Provider) -> String {
    match target {
        Provider::Claude | Provider::Codex | Provider::Pi | Provider::Gjc => {
            uuid::Uuid::now_v7().to_string()
        }
        Provider::OpenCode => crate::ids::opencode_session_id(),
    }
}

fn ensure_native_session_id_for(target: Provider, session_id: &str) -> Result<()> {
    let ok = match target {
        Provider::Claude | Provider::Codex | Provider::Pi | Provider::Gjc => {
            uuid::Uuid::parse_str(session_id).is_ok()
        }
        Provider::OpenCode => is_opencode_session_id(session_id),
    };
    if ok {
        Ok(())
    } else {
        Err(ConvertError::Other(format!(
            "new session id `{session_id}` is not a native {} session id",
            target.as_str()
        )))
    }
}

fn ensure_distinct_native_clone_id(
    target: Provider,
    source_session_id: &str,
    new_session_id: &str,
) -> Result<()> {
    ensure_native_session_id_for(target, new_session_id)?;
    if new_session_id == source_session_id {
        return Err(ConvertError::Other(format!(
            "new {} session id must differ from the source session id",
            target.as_str()
        )));
    }
    Ok(())
}

fn is_opencode_session_id(session_id: &str) -> bool {
    let Some(body) = session_id.strip_prefix("ses_") else {
        return false;
    };
    body.len() == 26
        && body.is_ascii()
        && body[..12].chars().all(|c| c.is_ascii_hexdigit())
        && body[12..].chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use serde_json::{json, Value};

    use super::*;

    fn session_info(
        provider: Provider,
        session_id: &str,
        cwd: &str,
        source: PathBuf,
    ) -> SessionInfo {
        SessionInfo {
            provider,
            session_id: session_id.into(),
            cwd: cwd.into(),
            source,
            updated_at_epoch_s: 0,
            title: None,
            relation: None,
        }
    }

    fn parse_jsonl(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn clone_path_rollback_restores_previous_file_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target_file = dir.path().join("session.jsonl");
        let target_dir = dir.path().join("session");
        fs::write(&target_file, "old-file").unwrap();
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("old-sidecar.txt"), "old-sidecar").unwrap();

        let rollback =
            ClonePathRollback::capture(&[target_file.clone(), target_dir.clone()], true).unwrap();
        fs::write(&target_file, "new-file").unwrap();
        fs::remove_dir_all(&target_dir).unwrap();
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("new-sidecar.txt"), "new-sidecar").unwrap();

        rollback.rollback().unwrap();

        assert_eq!(fs::read_to_string(&target_file).unwrap(), "old-file");
        assert_eq!(
            fs::read_to_string(target_dir.join("old-sidecar.txt")).unwrap(),
            "old-sidecar"
        );
        assert!(!target_dir.join("new-sidecar.txt").exists());
        assert!(
            fs::read_dir(dir.path()).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".rollback-")),
            "successful rollback must consume its private backups"
        );
    }

    #[test]
    fn claude_sidecar_overwrite_removes_stale_target_when_source_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.jsonl");
        let target = dir.path().join("target.jsonl");
        fs::write(&source, "source").unwrap();
        fs::write(&target, "target").unwrap();
        let stale_sidecar = target.with_extension("");
        fs::create_dir(&stale_sidecar).unwrap();
        fs::write(stale_sidecar.join("stale.txt"), "stale").unwrap();

        clone_claude_sidecar(&source, &target, true).unwrap();

        assert!(!stale_sidecar.exists());
    }

    #[cfg(unix)]
    #[test]
    fn clone_path_rollback_refuses_incomplete_backup_of_special_entry() {
        let dir = tempfile::tempdir().unwrap();
        let target_dir = dir.path().join("session");
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("old-sidecar.txt"), "old-sidecar").unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(target_dir.join("live.sock"))
            .expect("create unsupported sidecar entry");

        let error = ClonePathRollback::capture(std::slice::from_ref(&target_dir), true)
            .expect_err("an incomplete rollback copy must never authorize overwrite");

        assert!(error.to_string().contains("unsupported filesystem entry"));
        assert_eq!(
            fs::read_to_string(target_dir.join("old-sidecar.txt")).unwrap(),
            "old-sidecar"
        );
        assert!(target_dir.join("live.sock").exists());
        assert!(
            fs::read_dir(dir.path()).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".rollback-")),
            "failed capture must remove its partial private backup"
        );
    }

    #[test]
    fn cross_provider_clone_installs_two_message_context_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.jsonl");
        fs::write(
            &source_path,
            r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"source-codex","cwd":"/repo"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"continue this work"}]}}
"#,
        )
        .unwrap();
        let src = session_info(Provider::Codex, "source-codex", "/repo", source_path);
        let report = clone_cross_provider_context_wrapper_with_install_opts(
            &src,
            Provider::Claude,
            &CloneOpts::default(),
            &InstallSessionOpts {
                claude_home: Some(dir.path().join(".claude")),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(report.source_provider, Provider::Codex);
        assert_eq!(report.source_session_id, "source-codex");
        assert_eq!(report.target_provider, Provider::Claude);
        let ArtifactPath::File(path) = &report.artifact else {
            panic!("expected Claude file artifact, got {:?}", report.artifact);
        };
        assert!(path.is_file());

        let back = providers::claude::from_file(path, &Default::default()).unwrap();
        let user_texts = text_messages_for_role(&back, crate::universal::Role::User);
        let assistant_texts = text_messages_for_role(&back, crate::universal::Role::Assistant);
        assert_eq!(user_texts.len(), 1);
        assert_eq!(assistant_texts.len(), 1);
        let user_text = user_texts[0];
        assert!(user_text.contains("=== source-codex (codex) ==="));
        assert!(user_text.contains("continue this work"));
        assert!(user_text.ends_with(crate::CONTEXT_CONTINUATION_PROMPT));
        assert_eq!(assistant_texts[0], crate::CONTEXT_ACK);
    }

    #[test]
    fn cross_provider_clone_file_reference_writes_context_file_and_installs_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.jsonl");
        fs::write(
            &source_path,
            r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"source-codex","cwd":"/repo"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"continue this work"}]}}
"#,
        )
        .unwrap();
        let context_dir = dir.path().join(".cokacmux").join("context");
        let src = session_info(Provider::Codex, "source-codex", "/repo", source_path);
        let report = clone_cross_provider_context_wrapper_with_install_opts(
            &src,
            Provider::Claude,
            &CloneOpts {
                context_mode: CloneContextMode::FileReference,
                context_dir: Some(context_dir.clone()),
                ..Default::default()
            },
            &InstallSessionOpts {
                claude_home: Some(dir.path().join(".claude")),
                ..Default::default()
            },
        )
        .unwrap();

        let mut context_paths = fs::read_dir(&context_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        context_paths.sort();
        assert_eq!(context_paths.len(), 1);
        let context_path = &context_paths[0];
        let context_text = fs::read_to_string(context_path).unwrap();
        assert!(context_text.contains("=== source-codex (codex) ==="));
        assert!(context_text.contains("continue this work"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(context_path).unwrap().permissions().mode() & 0o777,
                0o600,
                "persisted transcript context must not be readable by other users"
            );
        }

        let ArtifactPath::File(path) = &report.artifact else {
            panic!("expected Claude file artifact, got {:?}", report.artifact);
        };
        let back = providers::claude::from_file(path, &Default::default()).unwrap();
        let user_texts = text_messages_for_role(&back, crate::universal::Role::User);
        let assistant_texts = text_messages_for_role(&back, crate::universal::Role::Assistant);
        assert_eq!(user_texts.len(), 1);
        assert_eq!(assistant_texts.len(), 1);
        let user_text = user_texts[0];
        assert!(user_text.contains(&context_path.display().to_string()));
        assert!(user_text.contains("read this file first"));
        assert!(!user_text.contains("continue this work"));
        assert_eq!(assistant_texts[0], crate::CONTEXT_ACK);
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn cross_provider_clone_rejects_non_native_target_session_id_before_install() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.jsonl");
        fs::write(
            &source_path,
            r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"source-codex","cwd":"/repo"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"continue this work"}]}}
"#,
        )
        .unwrap();
        let db_path = dir.path().join("opencode.db");
        let context_dir = dir.path().join("context");
        let src = session_info(Provider::Codex, "source-codex", "/repo", source_path);
        let err = clone_cross_provider_context_wrapper_with_install_opts(
            &src,
            Provider::OpenCode,
            &CloneOpts {
                new_id: Some("not-native".into()),
                context_mode: CloneContextMode::FileReference,
                context_dir: Some(context_dir.clone()),
                ..Default::default()
            },
            &InstallSessionOpts {
                opencode_db_path: Some(db_path.clone()),
                ..Default::default()
            },
        )
        .expect_err("invalid target-native session id should be rejected before install");

        assert!(err.to_string().contains("not a native opencode session id"));
        assert!(
            !db_path.exists(),
            "invalid id should fail before creating the OpenCode DB"
        );
        assert!(
            !context_dir.exists(),
            "invalid id should fail before persisting a context transcript"
        );
    }

    fn text_messages_for_role(
        session: &crate::universal::UniversalSession,
        role: crate::universal::Role,
    ) -> Vec<&str> {
        session
            .messages
            .iter()
            .filter(|message| message.role == role && !message.flags.is_meta)
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                crate::universal::ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn claude_clone_copies_native_jsonl_and_patches_identity_only() {
        let dir = tempfile::tempdir().unwrap();
        let claude_home = dir.path().join(".claude");
        let source_dir = claude_home
            .join("projects")
            .join(providers::claude::path::encode_cwd("/old/cwd"));
        fs::create_dir_all(&source_dir).unwrap();
        let source_path = source_dir.join("old-session.jsonl");
        let source_sidecar = source_path.with_extension("");
        let source_overflow = source_sidecar.join("tool-results").join("a.txt");
        fs::write(
            &source_path,
            [
                json!({
                    "type": "user",
                    "sessionId": "old-session",
                    "cwd": "/old/cwd",
                    "uuid": "11111111-1111-7111-8111-111111111111",
                    "parentUuid": null,
                    "message": {"role": "user", "content": "hi"},
                    "providerSpecific": {"keep": true}
                })
                .to_string(),
                json!({
                    "type": "assistant",
                    "sessionId": "old-session",
                    "cwd": "/old/cwd",
                    "uuid": "22222222-2222-7222-8222-222222222222",
                    "parentUuid": "11111111-1111-7111-8111-111111111111",
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "step-start"},
                            {"type": "text", "text": "valid"},
                            {
                                "type": "tool_result",
                                "tool_use_id": "call-1",
                                "content": format!(
                                    "Output too large. Full output saved to: {}\n\nPreview",
                                    source_overflow.display()
                                )
                            }
                        ]
                    },
                    "extraNativeField": "preserved"
                })
                .to_string(),
                json!({
                    "type": "last-prompt",
                    "sessionId": "old-session",
                    "leafUuid": "22222222-2222-7222-8222-222222222222"
                })
                .to_string(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        fs::create_dir_all(source_sidecar.join("tool-results")).unwrap();
        fs::write(&source_overflow, "sidecar").unwrap();
        let src = session_info(Provider::Claude, "old-session", "/old/cwd", source_path);

        let report = clone_claude_same_provider_at_home(
            &src,
            &CloneOpts {
                cwd: Some("/new/cwd".into()),
                new_id: Some("33333333-3333-7333-8333-333333333333".into()),
                ..Default::default()
            },
            &claude_home,
        )
        .unwrap();

        let ArtifactPath::File(path) = report.artifact else {
            panic!("expected file artifact");
        };
        let values = parse_jsonl(&path);
        assert_eq!(values.len(), 3);
        assert_eq!(
            values[0]["sessionId"],
            "33333333-3333-7333-8333-333333333333"
        );
        assert_eq!(values[0]["cwd"], "/new/cwd");
        assert_eq!(values[0]["providerSpecific"]["keep"], true);
        assert_ne!(
            values[0]["uuid"].as_str().unwrap(),
            "11111111-1111-7111-8111-111111111111"
        );
        assert_eq!(values[1]["parentUuid"].as_str(), values[0]["uuid"].as_str());
        assert_eq!(values[1]["extraNativeField"], "preserved");
        let content = values[1]["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        let cloned_sidecar = path.with_extension("").join("tool-results").join("a.txt");
        let cloned_ref = content[1]["content"].as_str().unwrap();
        assert!(cloned_ref.contains(&cloned_sidecar.display().to_string()));
        assert!(!cloned_ref.contains(&source_overflow.display().to_string()));
        assert_eq!(values[2]["leafUuid"].as_str(), values[1]["uuid"].as_str());
        assert!(path
            .with_extension("")
            .join("tool-results")
            .join("a.txt")
            .is_file());
    }

    #[test]
    fn same_provider_clone_rejects_source_session_id_without_touching_source() {
        let dir = tempfile::tempdir().unwrap();
        let claude_home = dir.path().join(".claude");
        let source_id = "11111111-1111-7111-8111-111111111111";
        let source_dir = claude_home
            .join("projects")
            .join(providers::claude::path::encode_cwd("/repo"));
        fs::create_dir_all(&source_dir).unwrap();
        let source_path = source_dir.join(format!("{source_id}.jsonl"));
        let source_text = json!({
            "type": "user",
            "sessionId": source_id,
            "cwd": "/repo",
            "uuid": "22222222-2222-7222-8222-222222222222",
            "message": {"role": "user", "content": "keep me"}
        })
        .to_string()
            + "\n";
        fs::write(&source_path, &source_text).unwrap();
        let src = session_info(Provider::Claude, source_id, "/repo", source_path.clone());

        let error = clone_claude_same_provider_at_home(
            &src,
            &CloneOpts {
                overwrite: true,
                new_id: Some(source_id.into()),
                ..Default::default()
            },
            &claude_home,
        )
        .expect_err("a clone must never overwrite its own source identity");

        assert!(error.to_string().contains("must differ"));
        assert_eq!(fs::read_to_string(source_path).unwrap(), source_text);
    }

    #[test]
    fn claude_clone_preflights_sidecar_conflict_before_writing_file() {
        let dir = tempfile::tempdir().unwrap();
        let claude_home = dir.path().join(".claude");
        let source_dir = claude_home
            .join("projects")
            .join(providers::claude::path::encode_cwd("/old/cwd"));
        fs::create_dir_all(&source_dir).unwrap();
        let source_path = source_dir.join("old-session.jsonl");
        fs::write(
            &source_path,
            json!({
                "type": "user",
                "sessionId": "old-session",
                "cwd": "/old/cwd",
                "uuid": "11111111-1111-7111-8111-111111111111",
                "message": {"role": "user", "content": "hi"}
            })
            .to_string()
                + "\n",
        )
        .unwrap();
        fs::create_dir_all(source_path.with_extension("").join("tool-results")).unwrap();
        let new_id = "33333333-3333-7333-8333-333333333333";
        let target_path = claude_jsonl_path(&claude_home, "/old/cwd", new_id);
        let target_sidecar = target_path.with_extension("");
        fs::create_dir_all(&target_sidecar).unwrap();
        fs::write(target_sidecar.join("existing.txt"), "keep").unwrap();
        let src = session_info(Provider::Claude, "old-session", "/old/cwd", source_path);

        let error = clone_claude_same_provider_at_home(
            &src,
            &CloneOpts {
                new_id: Some(new_id.into()),
                ..Default::default()
            },
            &claude_home,
        )
        .expect_err("sidecar conflict should fail before writing clone JSONL");

        assert!(error.to_string().contains("sidecar target already exists"));
        assert!(!target_path.exists());
        assert_eq!(
            fs::read_to_string(target_sidecar.join("existing.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn claude_clone_then_remove_deletes_only_clone_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let claude_home = dir.path().join(".claude");
        let source_dir = claude_home
            .join("projects")
            .join(providers::claude::path::encode_cwd("/old/cwd"));
        fs::create_dir_all(&source_dir).unwrap();
        let source_path = source_dir.join("old-session.jsonl");
        let source_content = json!({
            "type": "user",
            "sessionId": "old-session",
            "cwd": "/old/cwd",
            "uuid": "11111111-1111-7111-8111-111111111111",
            "parentUuid": null,
            "message": {"role": "user", "content": "hi"},
            "nativeSourceField": "must-stay"
        })
        .to_string()
            + "\n";
        fs::write(&source_path, &source_content).unwrap();
        let source_sidecar = source_path.with_extension("");
        fs::create_dir_all(source_sidecar.join("tool-results")).unwrap();
        fs::write(
            source_sidecar.join("tool-results").join("a.txt"),
            "source-sidecar",
        )
        .unwrap();
        let source_mtime_before = fs::metadata(&source_path).unwrap().modified().unwrap();
        let src = session_info(
            Provider::Claude,
            "old-session",
            "/old/cwd",
            source_path.clone(),
        );

        let report = clone_claude_same_provider_at_home(
            &src,
            &CloneOpts {
                new_id: Some("33333333-3333-7333-8333-333333333333".into()),
                ..Default::default()
            },
            &claude_home,
        )
        .unwrap();
        let clone_path = match &report.artifact {
            ArtifactPath::File(path) => path.clone(),
            other => panic!("expected file artifact, got {other:?}"),
        };
        assert!(clone_path.is_file());
        assert!(clone_path
            .with_extension("")
            .join("tool-results")
            .join("a.txt")
            .is_file());
        assert_eq!(fs::read_to_string(&source_path).unwrap(), source_content);
        assert_eq!(
            fs::metadata(&source_path).unwrap().modified().unwrap(),
            source_mtime_before,
            "cloning must not change the source Claude JSONL mtime"
        );

        let clone_info = session_info(
            Provider::Claude,
            &report.new_session_id,
            "/old/cwd",
            clone_path.clone(),
        );
        let remove_report = crate::session::remove::remove(&clone_info).unwrap();

        assert_eq!(remove_report.provider, Provider::Claude);
        assert!(!clone_path.exists());
        assert!(!clone_path.with_extension("").exists());
        assert_eq!(fs::read_to_string(&source_path).unwrap(), source_content);
        assert_eq!(
            fs::metadata(&source_path).unwrap().modified().unwrap(),
            source_mtime_before,
            "removing the clone must not change the source Claude JSONL mtime"
        );
        assert_eq!(
            fs::read_to_string(source_sidecar.join("tool-results").join("a.txt")).unwrap(),
            "source-sidecar"
        );
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn codex_clone_copies_rollout_and_state_row_without_synthesizing_events() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join(".codex");
        let source_dir = codex_home.join("sessions/2026/05/30");
        fs::create_dir_all(&source_dir).unwrap();
        let source_path = source_dir
            .join("rollout-2026-05-30T00-00-00-11111111-1111-7111-8111-111111111111.jsonl");
        fs::write(
            &source_path,
            [
                json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "11111111-1111-7111-8111-111111111111",
                        "cwd": "/old/cwd",
                        "source": "native-test"
                    }
                })
                .to_string(),
                json!({
                    "type": "event_msg",
                    "payload": {
                        "message": "kept",
                        "unknown_codex_field": {"nested": true}
                    }
                })
                .to_string(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let state_5 = codex_home.join("state_5.sqlite");
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        conn.execute(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT,
                custom TEXT
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, cwd, title, custom)
             VALUES (?1, ?2, ?3, 'old title', 'native-value')",
            rusqlite::params![
                "11111111-1111-7111-8111-111111111111",
                source_path.display().to_string(),
                "/old/cwd"
            ],
        )
        .unwrap();
        drop(conn);
        let src = session_info(
            Provider::Codex,
            "11111111-1111-7111-8111-111111111111",
            "/old/cwd",
            source_path,
        );

        let report = clone_codex_same_provider_at_home(
            &src,
            &CloneOpts {
                cwd: Some("/new/cwd".into()),
                new_id: Some("22222222-2222-7222-8222-222222222222".into()),
                ..Default::default()
            },
            &codex_home,
        )
        .unwrap();

        let ArtifactPath::File(path) = report.artifact else {
            panic!("expected file artifact");
        };
        let values = parse_jsonl(&path);
        assert_eq!(values.len(), 2);
        assert_eq!(
            values[0]["payload"]["id"],
            "22222222-2222-7222-8222-222222222222"
        );
        assert_eq!(values[0]["payload"]["cwd"], "/new/cwd");
        assert_eq!(values[0]["payload"]["source"], "native-test");
        assert_eq!(values[1]["payload"]["unknown_codex_field"]["nested"], true);
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        let row = conn
            .query_row(
                "SELECT rollout_path, cwd, title, custom FROM threads WHERE id = ?1",
                rusqlite::params!["22222222-2222-7222-8222-222222222222"],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, path.display().to_string());
        assert_eq!(row.1, "/new/cwd");
        assert_eq!(row.2, "old title");
        assert_eq!(row.3, "native-value");
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn codex_state_clone_rollback_restores_previous_target_row() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join(".codex");
        fs::create_dir(&codex_home).unwrap();
        let state_5 = codex_home.join("state_5.sqlite");
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        conn.execute(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, cwd, title)
             VALUES ('source', '/source.jsonl', '/source', 'source title')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, cwd, title)
             VALUES ('target', '/old.jsonl', '/old', 'old target title')",
            [],
        )
        .unwrap();
        drop(conn);

        let rollback = copy_codex_state_thread_row(
            &codex_home,
            "source",
            "target",
            Path::new("/new.jsonl"),
            "/new",
            true,
        )
        .unwrap();
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        let replaced = conn
            .query_row(
                "SELECT rollout_path, cwd, title FROM threads WHERE id = 'target'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            replaced,
            (
                "/new.jsonl".to_string(),
                "/new".to_string(),
                "source title".to_string()
            )
        );
        drop(conn);

        rollback.rollback().unwrap();

        let conn = rusqlite::Connection::open(&state_5).unwrap();
        let restored = conn
            .query_row(
                "SELECT rollout_path, cwd, title FROM threads WHERE id = 'target'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            restored,
            (
                "/old.jsonl".to_string(),
                "/old".to_string(),
                "old target title".to_string()
            )
        );
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn codex_clone_then_remove_deletes_only_clone_artifacts_and_state_row() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join(".codex");
        let source_dir = codex_home.join("sessions/2026/05/30");
        fs::create_dir_all(&source_dir).unwrap();
        let source_path = source_dir
            .join("rollout-2026-05-30T00-00-00-11111111-1111-7111-8111-111111111111.jsonl");
        let source_content = [
            json!({
                "type": "session_meta",
                "payload": {
                    "id": "11111111-1111-7111-8111-111111111111",
                    "cwd": "/old/cwd",
                    "source": "native-test"
                }
            })
            .to_string(),
            json!({
                "type": "event_msg",
                "payload": {"message": "source-kept", "custom": 7}
            })
            .to_string(),
        ]
        .join("\n")
            + "\n";
        fs::write(&source_path, &source_content).unwrap();
        let state_5 = codex_home.join("state_5.sqlite");
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        conn.execute(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT,
                custom TEXT
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, cwd, title, custom)
             VALUES (?1, ?2, ?3, 'source title', 'source-custom')",
            rusqlite::params![
                "11111111-1111-7111-8111-111111111111",
                source_path.display().to_string(),
                "/old/cwd"
            ],
        )
        .unwrap();
        drop(conn);
        let source_mtime_before = fs::metadata(&source_path).unwrap().modified().unwrap();
        let src = session_info(
            Provider::Codex,
            "11111111-1111-7111-8111-111111111111",
            "/old/cwd",
            source_path.clone(),
        );

        let report = clone_codex_same_provider_at_home(
            &src,
            &CloneOpts {
                new_id: Some("22222222-2222-7222-8222-222222222222".into()),
                ..Default::default()
            },
            &codex_home,
        )
        .unwrap();
        let clone_path = match &report.artifact {
            ArtifactPath::File(path) => path.clone(),
            other => panic!("expected file artifact, got {other:?}"),
        };
        assert!(clone_path.is_file());
        let clone_info = session_info(
            Provider::Codex,
            &report.new_session_id,
            "/old/cwd",
            clone_path.clone(),
        );
        let remove_report = crate::session::remove::remove(&clone_info).unwrap();

        assert_eq!(remove_report.provider, Provider::Codex);
        assert!(!clone_path.exists());
        assert_eq!(fs::read_to_string(&source_path).unwrap(), source_content);
        assert_eq!(
            fs::metadata(&source_path).unwrap().modified().unwrap(),
            source_mtime_before,
            "cloning and removing a clone must not change the source Codex rollout mtime"
        );
        let conn = rusqlite::Connection::open(&state_5).unwrap();
        let clone_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE id = ?1",
                rusqlite::params!["22222222-2222-7222-8222-222222222222"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(clone_rows, 0);
        let source_row = conn
            .query_row(
                "SELECT rollout_path, cwd, title, custom FROM threads WHERE id = ?1",
                rusqlite::params!["11111111-1111-7111-8111-111111111111"],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(source_row.0, source_path.display().to_string());
        assert_eq!(source_row.1, "/old/cwd");
        assert_eq!(source_row.2, "source title");
        assert_eq!(source_row.3, "source-custom");
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn opencode_clone_then_remove_deletes_only_clone_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        providers::opencode::db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 1, 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session
                (id, project_id, slug, directory, title, version, time_created, time_updated, path)
             VALUES ('ses_source', 'global', 'slug-source', '/old/cwd', 'source title', 'v1', 1, 1, '-old-cwd')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_source', 'ses_source', 1, 1, ?1)",
            rusqlite::params![json!({
                "role": "user",
                "path": {"cwd": "/old/cwd"},
                "custom": "source-message"
            })
            .to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('prt_source', 'msg_source', 'ses_source', 1, 1, ?1)",
            rusqlite::params![json!({"type": "text", "text": "hello"}).to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, time_created, time_updated, data)
             VALUES ('evt_source', 'ses_source', 'agent-switched', 1, 1, ?1)",
            rusqlite::params![json!({"agent": "build"}).to_string()],
        )
        .unwrap();
        drop(conn);
        let src = session_info(
            Provider::OpenCode,
            "ses_source",
            "/old/cwd",
            db_path.clone(),
        );

        let report = clone_opencode_same_provider(&src, &CloneOpts::default()).unwrap();
        let cloned_info = session_info(
            Provider::OpenCode,
            &report.new_session_id,
            "/old/cwd",
            db_path.clone(),
        );
        let remove_report = crate::session::remove::remove(&cloned_info).unwrap();

        assert_eq!(remove_report.provider, Provider::OpenCode);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let clone_rows: i64 = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM session WHERE id = ?1) +
                    (SELECT COUNT(*) FROM message WHERE session_id = ?1) +
                    (SELECT COUNT(*) FROM part WHERE session_id = ?1) +
                    (SELECT COUNT(*) FROM session_message WHERE session_id = ?1)",
                rusqlite::params![report.new_session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(clone_rows, 0);
        let source_rows: i64 = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM session WHERE id = 'ses_source') +
                    (SELECT COUNT(*) FROM message WHERE session_id = 'ses_source') +
                    (SELECT COUNT(*) FROM part WHERE session_id = 'ses_source') +
                    (SELECT COUNT(*) FROM session_message WHERE session_id = 'ses_source')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_rows, 4);
        let source_session = conn
            .query_row(
                "SELECT directory, title, path FROM session WHERE id = 'ses_source'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(source_session.0, "/old/cwd");
        assert_eq!(source_session.1, "source title");
        assert_eq!(source_session.2, "-old-cwd");
        let source_message_data: String = conn
            .query_row(
                "SELECT data FROM message WHERE id = 'msg_source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(source_message_data.contains("source-message"));
    }

    #[test]
    fn validation_failure_removes_installed_file_artifact_when_not_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clone-id.jsonl");
        fs::write(&path, "").unwrap();
        let artifact = ArtifactPath::File(path.clone());

        let error = ensure_clone_artifact_native_or_cleanup(
            Provider::Claude,
            "clone-id",
            "/tmp",
            &artifact,
            &CloneOpts::default(),
        )
        .expect_err("empty clone artifact should fail native validation");

        assert!(
            error.to_string().contains("failed native validation"),
            "{error}"
        );
        assert!(!path.exists(), "failed clone artifact should be removed");
    }
}
