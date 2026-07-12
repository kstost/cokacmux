//! GJC adapter — JSONL session files under `~/.gjc/agent/sessions`.
//!
//! GJC's transcript format is a fork of Pi's session JSONL. Keep the heavy
//! entry parser/writer shared with the Pi adapter, then patch the provider
//! identity and GJC-only header fields at this boundary.

pub mod install;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::error::Result;
use crate::universal::{Provider, UniversalSession};

#[derive(Debug, Clone, Default)]
pub struct GjcReadCtx {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GjcWriteOpts {
    /// Preserve original GJC JSONL entries when available. The session header is
    /// regenerated so cloned sessions get the requested id/cwd/title.
    pub replay_raw: bool,
}

impl Default for GjcWriteOpts {
    fn default() -> Self {
        Self { replay_raw: true }
    }
}

pub const CURRENT_SESSION_VERSION: u64 = crate::providers::pi::CURRENT_SESSION_VERSION;
pub const ENV_AGENT_DIR: &str = "GJC_CODING_AGENT_DIR";

pub fn from_file(path: &Path) -> Result<UniversalSession> {
    from_file_with(path, &GjcReadCtx::default())
}

pub fn from_file_with(path: &Path, ctx: &GjcReadCtx) -> Result<UniversalSession> {
    let pi_ctx = crate::providers::pi::PiReadCtx {
        session_id: ctx.session_id.clone(),
        cwd: ctx.cwd.clone(),
    };
    let mut session = crate::providers::pi::from_file_with(path, &pi_ctx)?;
    session.origin.provider = Some(Provider::Gjc);
    promote_pi_provenance_to_gjc(&mut session);
    apply_gjc_header_metadata(&mut session);
    Ok(session)
}

pub fn from_jsonl_str(jsonl: &str, ctx: &GjcReadCtx) -> Result<UniversalSession> {
    let pi_ctx = crate::providers::pi::PiReadCtx {
        session_id: ctx.session_id.clone(),
        cwd: ctx.cwd.clone(),
    };
    let mut session = crate::providers::pi::from_jsonl_str(jsonl, &pi_ctx)?;
    session.origin.provider = Some(Provider::Gjc);
    promote_pi_provenance_to_gjc(&mut session);
    apply_gjc_header_metadata(&mut session);
    Ok(session)
}

pub fn to_file(session: &UniversalSession, path: &Path, opts: &GjcWriteOpts) -> Result<()> {
    let text = to_jsonl_string(session, opts)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    crate::jsonl::write_text_atomic(path, &text)
}

pub fn to_jsonl_string(session: &UniversalSession, opts: &GjcWriteOpts) -> Result<String> {
    let mut pi_session = session.clone();
    demote_gjc_provenance_to_pi(&mut pi_session);
    let pi_opts = crate::providers::pi::PiWriteOpts {
        replay_raw: opts.replay_raw,
    };
    let text = crate::providers::pi::to_jsonl_string(&pi_session, &pi_opts)?;
    patch_gjc_header(text, session)
}

pub fn read_header(path: &Path) -> Result<Option<Value>> {
    crate::providers::pi::read::read_header(path)
}

pub fn default_agent_dir() -> Option<PathBuf> {
    std::env::var_os(ENV_AGENT_DIR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            crate::providers::discovery::configured_home_dir()
                .map(|home| home.join(".gjc").join("agent"))
        })
}

pub fn default_sessions_root() -> Option<PathBuf> {
    if let Some(agent_dir) = std::env::var_os(ENV_AGENT_DIR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Some(agent_dir.join("sessions"));
    }
    if let Some(root) = xdg_data_agent_root(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::consts::OS,
        Path::exists,
    ) {
        return Some(root.join("sessions"));
    }
    default_agent_dir().map(|dir| dir.join("sessions"))
}

pub fn default_project_session_dir(cwd: &str) -> Option<PathBuf> {
    default_sessions_root().map(|root| root.join(encoded_cwd_dir(cwd)))
}

pub fn encoded_cwd_dir(cwd: &str) -> String {
    let resolved_cwd = normalize_path(cwd);
    let home =
        crate::providers::discovery::configured_home_dir().map(|home| normalize_pathbuf(&home));
    let temp = Some(normalize_pathbuf(&std::env::temp_dir()));

    if let Some(home) = home {
        if let Some(relative) = relative_within(&home, &resolved_cwd) {
            return encode_relative("-", &relative);
        }
    }
    if let Some(temp) = temp {
        if let Some(relative) = relative_within(&temp, &resolved_cwd) {
            return encode_relative("-tmp", &relative);
        }
    }
    encode_legacy_absolute(&resolved_cwd)
}

pub fn session_file_name(session_id: &str, created_at: Option<DateTime<Utc>>) -> String {
    crate::providers::pi::session_file_name(session_id, created_at)
}

pub fn find_session_file_by_id(root_or_project_dir: &Path, session_id: &str) -> Option<PathBuf> {
    if !root_or_project_dir.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(root_or_project_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if let Some(found) = find_session_file_by_id(&path, session_id) {
                return Some(found);
            }
        } else if file_type.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            && read_header(&path)
                .ok()
                .flatten()
                .and_then(|header| {
                    header
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .as_deref()
                == Some(session_id)
        {
            return Some(path);
        }
    }
    None
}

pub(crate) fn user_message_title(message: &Value) -> Option<String> {
    crate::providers::pi::user_message_title(message)
}

fn apply_gjc_header_metadata(session: &mut UniversalSession) {
    let Some(header) = session.session_meta.as_ref() else {
        return;
    };
    if let Some(title) = header
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        session.title = Some(title.to_string());
    }
}

fn promote_pi_provenance_to_gjc(session: &mut UniversalSession) {
    for message in &mut session.messages {
        if let Some(rest) = message.provenance.source_event_type.strip_prefix("pi:") {
            message.provenance.source_event_type = format!("gjc:{rest}");
        }
    }
}

fn demote_gjc_provenance_to_pi(session: &mut UniversalSession) {
    for message in &mut session.messages {
        if let Some(rest) = message.provenance.source_event_type.strip_prefix("gjc:") {
            message.provenance.source_event_type = format!("pi:{rest}");
        }
    }
}

fn patch_gjc_header(text: String, session: &UniversalSession) -> Result<String> {
    let Some((first, rest)) = text.split_once('\n') else {
        return Ok(text);
    };
    let mut header: Value = serde_json::from_str(first)?;
    if let Some(object) = header.as_object_mut() {
        if let Some(title) = session
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            object.insert("title".into(), json!(title));
            let source = session
                .session_meta
                .as_ref()
                .and_then(|header| header.get("titleSource"))
                .and_then(Value::as_str)
                .filter(|source| matches!(*source, "auto" | "user"))
                .unwrap_or("user");
            object.insert("titleSource".into(), json!(source));
        }
        if let Some(parent) = session
            .extras
            .get("gjc_parent_session")
            .or_else(|| session.extras.get("pi_parent_session"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            object.insert("parentSession".into(), json!(parent));
        }
    }
    Ok(format!("{}\n{}", serde_json::to_string(&header)?, rest))
}

fn normalize_path(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    normalize_pathbuf(&path)
}

fn normalize_pathbuf(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    absolute.canonicalize().unwrap_or(absolute)
}

fn relative_within(root: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}

fn encode_relative(prefix: &str, relative: &Path) -> String {
    let relative = relative.to_string_lossy().replace(['/', '\\', ':'], "-");
    if relative.is_empty() {
        prefix.to_string()
    } else if prefix.ends_with('-') {
        format!("{prefix}{relative}")
    } else {
        format!("{prefix}-{relative}")
    }
}

fn encode_legacy_absolute(path: &Path) -> String {
    let trimmed = path.to_string_lossy();
    let trimmed = trimmed.trim_start_matches(['/', '\\']);
    format!("--{}--", trimmed.replace(['/', '\\', ':'], "-"))
}

fn xdg_data_agent_root<F>(xdg_data_home: Option<&OsStr>, os: &str, exists: F) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    if !matches!(os, "linux" | "macos") {
        return None;
    }
    let home = xdg_data_home?;
    if home.is_empty() {
        return None;
    }
    let root = PathBuf::from(home).join("gjc");
    exists(&root).then_some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_data_agent_root_matches_gjc_dir_resolver_rule() {
        let xdg_home = PathBuf::from("/tmp/cokacmux-xdg-data");
        let expected = xdg_home.join("gjc");

        assert_eq!(
            xdg_data_agent_root(Some(xdg_home.as_os_str()), "linux", |path| path == expected),
            Some(expected.clone())
        );
        assert_eq!(
            xdg_data_agent_root(Some(xdg_home.as_os_str()), "macos", |path| path == expected),
            Some(expected)
        );
        assert!(xdg_data_agent_root(Some(xdg_home.as_os_str()), "linux", |_| false).is_none());
        assert!(xdg_data_agent_root(Some(xdg_home.as_os_str()), "windows", |_| true).is_none());
    }
}
