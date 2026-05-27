//! Session manager — provider-agnostic operations layered on top of the
//! provider adapters: list across all providers, resolve by id prefix,
//! render as human-readable transcript, clone with new id, remove from
//! the agent's live storage, and search by content.

pub mod clone;
pub mod clone_tree;
pub mod native_validate;
pub mod remove;
pub mod render;
pub mod search;
pub mod title;

use crate::error::{ConvertError, Result};
#[cfg(feature = "discovery")]
use crate::providers::discovery::{self, SessionInfo};
use crate::universal::{Provider, UniversalSession};

pub use search::{search_all, SearchHit};

/// List every session known to every enabled provider, most-recent first.
#[cfg(feature = "discovery")]
pub fn list_all() -> Result<Vec<SessionInfo>> {
    crate::debug::log("session_list_all_start", serde_json::json!({}));
    let mut out: Vec<SessionInfo> = Vec::new();
    for p in [Provider::Claude, Provider::Codex, Provider::OpenCode] {
        // Each provider list error is non-fatal — e.g. missing ~/.codex shouldn't
        // hide ~/.claude sessions.
        match discovery::list_all(p) {
            Ok(mut xs) => {
                crate::debug::log(
                    "session_list_provider_ok",
                    serde_json::json!({
                        "provider": p.as_str(),
                        "count": xs.len(),
                    }),
                );
                out.append(&mut xs);
            }
            Err(error) => {
                crate::debug::log(
                    "session_list_provider_error",
                    serde_json::json!({
                        "provider": p.as_str(),
                        "error": error.to_string(),
                    }),
                );
            }
        }
    }
    title::apply_overrides(&mut out);
    out.sort_by(|a, b| b.updated_at_epoch_s.cmp(&a.updated_at_epoch_s));
    crate::debug::log(
        "session_list_all_ok",
        serde_json::json!({
            "count": out.len(),
        }),
    );
    Ok(out)
}

/// Resolve a session id (or unique prefix) to exactly one SessionInfo across
/// all providers. Errors if no match, or if multiple sessions share the
/// prefix (caller must supply more chars).
#[cfg(feature = "discovery")]
pub fn resolve(id_or_prefix: &str) -> Result<SessionInfo> {
    crate::debug::log(
        "session_resolve_start",
        serde_json::json!({
            "id_or_prefix": id_or_prefix,
        }),
    );
    let all = list_all()?;
    let matches: Vec<_> = all
        .into_iter()
        .filter(|s| s.session_id.starts_with(id_or_prefix) || s.session_id == id_or_prefix)
        .collect();
    crate::debug::log(
        "session_resolve_matches",
        serde_json::json!({
            "id_or_prefix": id_or_prefix,
            "matches": matches.len(),
        }),
    );
    match matches.len() {
        0 => {
            crate::debug::log(
                "session_resolve_error",
                serde_json::json!({
                    "id_or_prefix": id_or_prefix,
                    "error": "no match",
                }),
            );
            Err(ConvertError::Other(format!(
                "no session matching {:?}",
                id_or_prefix
            )))
        }
        1 => {
            let info = matches.into_iter().next().unwrap();
            crate::debug::log(
                "session_resolve_ok",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "cwd": &info.cwd,
                }),
            );
            Ok(info)
        }
        n => {
            let ids: Vec<String> = matches.iter().map(|s| s.session_id.clone()).collect();
            crate::debug::log(
                "session_resolve_error",
                serde_json::json!({
                    "id_or_prefix": id_or_prefix,
                    "error": "ambiguous",
                    "matches": n,
                    "ids": &ids,
                }),
            );
            Err(ConvertError::Other(format!(
                "{} sessions match prefix {:?}: {}",
                n,
                id_or_prefix,
                ids.join(", ")
            )))
        }
    }
}

/// Load a SessionInfo's content as a UniversalSession.
#[cfg(feature = "discovery")]
pub fn load(info: &SessionInfo) -> Result<UniversalSession> {
    crate::debug::log(
        "session_load_start",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &info.session_id,
            "source": info.source.display().to_string(),
        }),
    );
    let result = match info.provider {
        #[cfg(feature = "claude")]
        Provider::Claude => crate::providers::claude::from_file(&info.source, &Default::default()),
        #[cfg(feature = "codex")]
        Provider::Codex => crate::providers::codex::from_file(&info.source),
        #[cfg(feature = "opencode")]
        Provider::OpenCode => {
            crate::providers::opencode::from_db_path(&info.source, &info.session_id)
        }
        #[allow(unreachable_patterns)]
        _ => Err(ConvertError::Unsupported(
            "provider feature disabled".into(),
        )),
    };
    let mut session = match result {
        Ok(session) => session,
        Err(error) => {
            crate::debug::log(
                "session_load_error",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };
    if let Some(title) = title::title_override(info) {
        session.title = title;
    } else if session.title.is_none() {
        session.title = info.title.clone();
    }
    crate::debug::log(
        "session_load_ok",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "title_present": session.title.is_some(),
        }),
    );
    Ok(session)
}
