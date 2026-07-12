//! Pi adapter — JSONL session files under `~/.pi/agent/sessions`.

pub mod from_universal;
pub mod read;
pub mod write;

#[cfg(feature = "discovery")]
pub mod install;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::error::Result;
use crate::universal::UniversalSession;

#[derive(Debug, Clone, Default)]
pub struct PiReadCtx {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PiWriteOpts {
    /// Preserve original Pi JSONL entries when available. The session header is
    /// regenerated from `UniversalSession` so cloned sessions get the requested
    /// id/cwd while the rest of the tree stays native.
    pub replay_raw: bool,
}

impl Default for PiWriteOpts {
    fn default() -> Self {
        Self { replay_raw: true }
    }
}

pub fn from_file(path: &Path) -> Result<UniversalSession> {
    read::from_jsonl_path(path, &PiReadCtx::default())
}

pub fn from_file_with(path: &Path, ctx: &PiReadCtx) -> Result<UniversalSession> {
    read::from_jsonl_path(path, ctx)
}

pub fn from_jsonl_str(jsonl: &str, ctx: &PiReadCtx) -> Result<UniversalSession> {
    read::from_jsonl_str(jsonl, ctx)
}

pub fn to_file(session: &UniversalSession, path: &Path, opts: &PiWriteOpts) -> Result<()> {
    write::to_jsonl_path(session, path, opts)
}

pub fn to_jsonl_string(session: &UniversalSession, opts: &PiWriteOpts) -> Result<String> {
    write::to_jsonl_string(session, opts)
}

pub const CURRENT_SESSION_VERSION: u64 = 3;
pub const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";
pub const ENV_SESSION_DIR: &str = "PI_CODING_AGENT_SESSION_DIR";

pub fn default_agent_dir() -> Option<PathBuf> {
    std::env::var_os(ENV_AGENT_DIR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            crate::providers::discovery::configured_home_dir()
                .map(|home| home.join(".pi").join("agent"))
        })
}

pub fn default_sessions_root() -> Option<PathBuf> {
    std::env::var_os(ENV_SESSION_DIR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| default_agent_dir().map(|dir| dir.join("sessions")))
}

pub fn default_project_session_dir(cwd: &str) -> Option<PathBuf> {
    let root = default_sessions_root()?;
    if std::env::var_os(ENV_SESSION_DIR)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        Some(root)
    } else {
        Some(root.join(encoded_cwd_dir(cwd)))
    }
}

pub fn encoded_cwd_dir(cwd: &str) -> String {
    let trimmed = cwd.trim_start_matches(['/', '\\']);
    let safe = trimmed
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':') {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>();
    format!("--{}--", safe)
}

pub fn session_file_name(session_id: &str, created_at: Option<DateTime<Utc>>) -> String {
    let timestamp = created_at
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let file_timestamp = timestamp.replace([':', '.'], "-");
    format!("{file_timestamp}_{session_id}.jsonl")
}

pub(crate) fn session_id_is_safe_path_component(session_id: &str) -> bool {
    let mut chars = session_id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let last = session_id.chars().next_back().unwrap_or(first);
    first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
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
            && crate::providers::pi::read::read_header(&path)
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
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let content = message.get("content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.as_str()),
                Value::Object(object)
                    if object.get("type").and_then(Value::as_str) == Some("text") =>
                {
                    object.get("text").and_then(Value::as_str)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    normalize_title_text(&text)
}

fn normalize_title_text(text: &str) -> Option<String> {
    let cleaned = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    let title = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}
