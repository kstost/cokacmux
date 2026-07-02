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
    crate::debug::log(
        "provider_gjc_install_start",
        serde_json::json!({
            "session_id": &session.session_id,
            "cwd": &session.cwd,
            "overwrite": opts.overwrite,
        }),
    );
    let dir = target_session_dir(session, opts)?;
    std::fs::create_dir_all(&dir)?;
    let existing = super::find_session_file_by_id(&dir, &session.session_id);
    let path = existing.unwrap_or_else(|| {
        dir.join(super::session_file_name(
            &session.session_id,
            session.created_at,
        ))
    });
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
    Ok(InstallReport { jsonl_path: path })
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
