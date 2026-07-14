//! Install a UniversalSession into GJC's live JSONL session store.

use std::path::PathBuf;

use crate::error::{ConvertError, Result};
use crate::universal::UniversalSession;

#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
    pub gjc_agent_dir: Option<PathBuf>,
    pub gjc_session_dir: Option<PathBuf>,
    pub overwrite: bool,
}

#[derive(Debug)]
pub struct InstallReport {
    pub jsonl_path: PathBuf,
}

pub fn install_to_user_dir(
    session: &UniversalSession,
    opts: &InstallOpts,
) -> Result<InstallReport> {
    let path = planned_jsonl_path(session, opts)?;
    install_to_planned_path(session, opts, &path)
}

/// Resolve the exact file that an install will replace, without mutating the
/// session store. This keeps overwrite backup and publication on one path.
pub(crate) fn planned_jsonl_path(
    session: &UniversalSession,
    opts: &InstallOpts,
) -> Result<PathBuf> {
    if !crate::providers::pi::session_id_is_safe_path_component(&session.session_id) {
        return Err(ConvertError::Other(format!(
            "gjc session id is not a safe filename component: {}",
            session.session_id
        )));
    }
    let dir = target_session_dir(session, opts)?;
    Ok(
        super::find_session_file_by_id(&dir, &session.session_id).unwrap_or_else(|| {
            dir.join(super::session_file_name(
                &session.session_id,
                session.created_at,
            ))
        }),
    )
}

pub(crate) fn install_to_planned_path(
    session: &UniversalSession,
    opts: &InstallOpts,
    path: &std::path::Path,
) -> Result<InstallReport> {
    crate::debug::log(
        "provider_gjc_install_start",
        serde_json::json!({
            "session_id": &session.session_id,
            "cwd": &session.cwd,
            "overwrite": opts.overwrite,
        }),
    );
    let dir = path.parent().ok_or_else(|| {
        ConvertError::Other(format!(
            "gjc install path has no parent: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(&dir)?;
    if path.exists() && !opts.overwrite {
        return Err(ConvertError::Other(format!(
            "gjc session already exists: {}",
            path.display()
        )));
    }
    super::to_file(session, &path, &Default::default())?;
    crate::debug::log(
        "provider_gjc_install_ok",
        serde_json::json!({
            "session_id": &session.session_id,
            "path": path.display().to_string(),
        }),
    );
    Ok(InstallReport {
        jsonl_path: path.to_path_buf(),
    })
}

fn target_session_dir(session: &UniversalSession, opts: &InstallOpts) -> Result<PathBuf> {
    if let Some(dir) = opts.gjc_session_dir.clone() {
        return Ok(dir);
    }
    if let Some(agent_dir) = opts.gjc_agent_dir.clone() {
        return Ok(agent_dir
            .join("sessions")
            .join(super::encoded_cwd_dir(&session.cwd)));
    }
    super::default_project_session_dir(&session.cwd)
        .ok_or_else(|| ConvertError::Other("cannot resolve gjc session dir".into()))
}
