//! Session title overrides stored in cokacmux's own metadata directory.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
#[cfg(feature = "discovery")]
use crate::providers::discovery::{home_dir, SessionInfo};
#[cfg(test)]
use crate::universal::Provider;

const APP_DIR_NAME: &str = ".cokacmux";
const TITLE_STORE_FILE: &str = "titles.json";
const TITLE_STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TitleStore {
    #[serde(default = "title_store_version")]
    version: u32,
    #[serde(default)]
    titles: BTreeMap<String, String>,
}

impl Default for TitleStore {
    fn default() -> Self {
        Self {
            version: TITLE_STORE_VERSION,
            titles: BTreeMap::new(),
        }
    }
}

fn title_store_version() -> u32 {
    TITLE_STORE_VERSION
}

#[cfg(feature = "discovery")]
pub fn set_title(info: &SessionInfo, title: &str) -> Result<()> {
    crate::debug::log(
        "title_set_start",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &info.session_id,
            "title_len": title.chars().count(),
        }),
    );
    let result = set_title_in_store(&title_store_path()?, info, title);
    match &result {
        Ok(()) => crate::debug::log(
            "title_set_ok",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
            }),
        ),
        Err(error) => crate::debug::log(
            "title_set_error",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "error": error.to_string(),
            }),
        ),
    }
    result
}

#[cfg(feature = "discovery")]
pub fn apply_overrides(sessions: &mut [SessionInfo]) {
    let Ok(path) = title_store_path() else {
        crate::debug::log("title_apply_no_path", serde_json::json!({}));
        return;
    };
    if let Ok(store) = read_title_store(&path) {
        let before = sessions.iter().filter(|info| info.title.is_some()).count();
        apply_overrides_from_store(&store, sessions);
        let after = sessions.iter().filter(|info| info.title.is_some()).count();
        crate::debug::log(
            "title_apply_ok",
            serde_json::json!({
                "path": path.display().to_string(),
                "stored_titles": store.titles.len(),
                "sessions": sessions.len(),
                "titles_before": before,
                "titles_after": after,
            }),
        );
    } else {
        crate::debug::log(
            "title_apply_read_error",
            serde_json::json!({
                "path": path.display().to_string(),
            }),
        );
    }
}

#[cfg(feature = "discovery")]
pub fn title_override(info: &SessionInfo) -> Option<Option<String>> {
    let store = read_title_store(&title_store_path().ok()?).ok()?;
    title_override_from_store(&store, info)
}

#[cfg(feature = "discovery")]
fn title_store_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(APP_DIR_NAME).join(TITLE_STORE_FILE))
}

#[cfg(feature = "discovery")]
fn set_title_in_store(path: &Path, info: &SessionInfo, title: &str) -> Result<()> {
    let mut store = read_title_store(path)?;
    store
        .titles
        .insert(title_key(info), title.trim().to_string());
    write_title_store(path, &store)
}

#[cfg(feature = "discovery")]
fn apply_overrides_from_store(store: &TitleStore, sessions: &mut [SessionInfo]) {
    for info in sessions {
        if let Some(title) = title_override_from_store(store, info) {
            info.title = title;
        }
    }
}

#[cfg(feature = "discovery")]
fn title_override_from_store(store: &TitleStore, info: &SessionInfo) -> Option<Option<String>> {
    store
        .titles
        .get(&title_key(info))
        .map(|title| normalize_stored_title(title))
}

#[cfg(feature = "discovery")]
fn title_key(info: &SessionInfo) -> String {
    format!("{}:{}", info.provider.as_str(), info.session_id)
}

fn normalize_stored_title(title: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn read_title_store(path: &Path) -> Result<TitleStore> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(TitleStore::default()),
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(TitleStore::default()),
        Err(e) => Err(e.into()),
    }
}

fn write_title_store(path: &Path, store: &TitleStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(store)? + "\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_info(provider: Provider, session_id: &str, title: Option<&str>) -> SessionInfo {
        SessionInfo {
            provider,
            session_id: session_id.to_string(),
            cwd: "/tmp/project".to_string(),
            source: PathBuf::from("/tmp/source"),
            updated_at_epoch_s: 0,
            title: title.map(str::to_string),
        }
    }

    #[test]
    fn stores_title_in_cokacmux_titles_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(APP_DIR_NAME).join(TITLE_STORE_FILE);
        let info = session_info(Provider::Claude, "s1", Some("Provider Title"));

        set_title_in_store(&path, &info, " Custom Title ").unwrap();

        let store = read_title_store(&path).unwrap();
        assert_eq!(
            store.titles.get("claude:s1").map(String::as_str),
            Some("Custom Title")
        );
        assert!(path.exists());
    }

    #[test]
    fn overrides_session_titles_from_store() {
        let mut store = TitleStore::default();
        store
            .titles
            .insert("codex:s1".to_string(), "Custom Title".to_string());
        let mut sessions = vec![session_info(Provider::Codex, "s1", Some("Provider Title"))];

        apply_overrides_from_store(&store, &mut sessions);

        assert_eq!(sessions[0].title.as_deref(), Some("Custom Title"));
    }

    #[test]
    fn empty_title_hides_provider_title() {
        let mut store = TitleStore::default();
        store
            .titles
            .insert("opencode:s1".to_string(), String::new());
        let mut sessions = vec![session_info(
            Provider::OpenCode,
            "s1",
            Some("Provider Title"),
        )];

        apply_overrides_from_store(&store, &mut sessions);

        assert_eq!(sessions[0].title, None);
    }
}
