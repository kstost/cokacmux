//! Clone parent/child metadata stored by cokacmux.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::providers::discovery::{home_dir, SessionInfo};
use crate::universal::Provider;

const APP_DIR_NAME: &str = ".cokacmux";
const CLONE_TREE_STORE_FILE: &str = "clone_tree.json";
const CLONE_TREE_STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey {
    pub provider: Provider,
    pub session_id: String,
}

impl SessionKey {
    pub fn new(provider: Provider, session_id: impl Into<String>) -> Self {
        Self {
            provider,
            session_id: session_id.into(),
        }
    }

    pub fn from_info(info: &SessionInfo) -> Self {
        Self::new(info.provider, info.session_id.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneLink {
    pub parent: SessionKey,
    pub child: SessionKey,
    pub cloned_at_epoch_s: u64,
}

#[derive(Debug, Clone)]
pub struct CloneTreeRow<'a> {
    pub info: &'a SessionInfo,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CloneTreeStore {
    #[serde(default = "clone_tree_store_version")]
    version: u32,
    #[serde(default)]
    links: Vec<CloneLink>,
}

impl Default for CloneTreeStore {
    fn default() -> Self {
        Self {
            version: CLONE_TREE_STORE_VERSION,
            links: Vec::new(),
        }
    }
}

fn clone_tree_store_version() -> u32 {
    CLONE_TREE_STORE_VERSION
}

pub fn load_links() -> Result<Vec<CloneLink>> {
    let path = clone_tree_store_path()?;
    crate::debug::log(
        "clone_tree_load_start",
        serde_json::json!({
            "path": path.display().to_string(),
        }),
    );
    let result = read_store(&path).map(|store| store.links);
    match &result {
        Ok(links) => crate::debug::log(
            "clone_tree_load_ok",
            serde_json::json!({
                "path": path.display().to_string(),
                "links": links.len(),
            }),
        ),
        Err(error) => crate::debug::log(
            "clone_tree_load_error",
            serde_json::json!({
                "path": path.display().to_string(),
                "error": error.to_string(),
            }),
        ),
    }
    result
}

pub fn record_clone(parent: SessionKey, child: SessionKey) -> Result<()> {
    record_clone_at_path(&clone_tree_store_path()?, parent, child, current_epoch_s())
}

pub fn record_clone_report(report: &super::clone::CloneReport) -> Result<()> {
    record_clone(
        SessionKey::new(report.source_provider, report.source_session_id.clone()),
        SessionKey::new(report.target_provider, report.new_session_id.clone()),
    )
}

pub fn visible_tree_rows<'a, F>(
    sessions: &'a [SessionInfo],
    links: &[CloneLink],
    mut matches_filter: F,
) -> Vec<CloneTreeRow<'a>>
where
    F: FnMut(&SessionInfo) -> bool,
{
    let infos_by_key: HashMap<SessionKey, &'a SessionInfo> = sessions
        .iter()
        .map(|info| (SessionKey::from_info(info), info))
        .collect();

    let mut children_by_parent: HashMap<SessionKey, Vec<SessionKey>> = HashMap::new();
    let mut parent_by_child: HashMap<SessionKey, SessionKey> = HashMap::new();
    for link in links {
        if !infos_by_key.contains_key(&link.parent) || !infos_by_key.contains_key(&link.child) {
            continue;
        }
        children_by_parent
            .entry(link.parent.clone())
            .or_default()
            .push(link.child.clone());
        parent_by_child.insert(link.child.clone(), link.parent.clone());
    }

    for children in children_by_parent.values_mut() {
        children.sort_by(|a, b| compare_keys(a, b, &infos_by_key));
        children.dedup();
    }

    let mut included = HashSet::new();
    for info in sessions {
        if !matches_filter(info) {
            continue;
        }
        let mut key = SessionKey::from_info(info);
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(key.clone()) {
                break;
            }
            included.insert(key.clone());
            let Some(parent) = parent_by_child.get(&key) else {
                break;
            };
            key = parent.clone();
        }
    }

    if included.is_empty() {
        if !sessions.is_empty() {
            crate::debug::log(
                "clone_tree_rows_empty",
                serde_json::json!({
                    "sessions": sessions.len(),
                    "links": links.len(),
                }),
            );
        }
        return Vec::new();
    }

    let mut roots: Vec<SessionKey> = sessions
        .iter()
        .map(SessionKey::from_info)
        .filter(|key| !parent_by_child.contains_key(key))
        .collect();
    roots.sort_by(|a, b| compare_keys(a, b, &infos_by_key));

    let mut rows = Vec::new();
    let mut visited = HashSet::new();
    for root in roots {
        push_tree_rows(
            &root,
            0,
            &infos_by_key,
            &children_by_parent,
            &included,
            &mut visited,
            &mut rows,
        );
    }

    let mut remaining: Vec<SessionKey> = sessions
        .iter()
        .map(SessionKey::from_info)
        .filter(|key| !visited.contains(key))
        .collect();
    remaining.sort_by(|a, b| compare_keys(a, b, &infos_by_key));
    for key in remaining {
        push_tree_rows(
            &key,
            0,
            &infos_by_key,
            &children_by_parent,
            &included,
            &mut visited,
            &mut rows,
        );
    }

    if rows.len() != sessions.len() {
        crate::debug::log(
            "clone_tree_rows_result",
            serde_json::json!({
                "sessions": sessions.len(),
                "links": links.len(),
                "rows": rows.len(),
                "included": included.len(),
            }),
        );
    }
    rows
}

fn push_tree_rows<'a>(
    key: &SessionKey,
    depth: usize,
    infos_by_key: &HashMap<SessionKey, &'a SessionInfo>,
    children_by_parent: &HashMap<SessionKey, Vec<SessionKey>>,
    included: &HashSet<SessionKey>,
    visited: &mut HashSet<SessionKey>,
    rows: &mut Vec<CloneTreeRow<'a>>,
) {
    if !visited.insert(key.clone()) {
        return;
    }
    if included.contains(key) {
        if let Some(info) = infos_by_key.get(key) {
            rows.push(CloneTreeRow { info, depth });
        }
    }
    if let Some(children) = children_by_parent.get(key) {
        for child in children {
            push_tree_rows(
                child,
                depth.saturating_add(1),
                infos_by_key,
                children_by_parent,
                included,
                visited,
                rows,
            );
        }
    }
}

fn compare_keys(
    a: &SessionKey,
    b: &SessionKey,
    infos_by_key: &HashMap<SessionKey, &SessionInfo>,
) -> Ordering {
    let a_info = infos_by_key.get(a);
    let b_info = infos_by_key.get(b);
    b_info
        .map(|info| info.updated_at_epoch_s)
        .cmp(&a_info.map(|info| info.updated_at_epoch_s))
        .then_with(|| a.provider.as_str().cmp(b.provider.as_str()))
        .then_with(|| a.session_id.cmp(&b.session_id))
}

fn clone_tree_store_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(APP_DIR_NAME).join(CLONE_TREE_STORE_FILE))
}

fn record_clone_at_path(
    path: &Path,
    parent: SessionKey,
    child: SessionKey,
    cloned_at_epoch_s: u64,
) -> Result<()> {
    crate::debug::log(
        "clone_tree_record_start",
        serde_json::json!({
            "path": path.display().to_string(),
            "parent_provider": parent.provider.as_str(),
            "parent_session_id": &parent.session_id,
            "child_provider": child.provider.as_str(),
            "child_session_id": &child.session_id,
        }),
    );
    let mut store = read_store(path)?;
    store.links.retain(|link| link.child != child);
    store.links.push(CloneLink {
        parent,
        child,
        cloned_at_epoch_s,
    });
    let result = write_store(path, &store);
    match &result {
        Ok(()) => crate::debug::log(
            "clone_tree_record_ok",
            serde_json::json!({
                "path": path.display().to_string(),
                "links": store.links.len(),
            }),
        ),
        Err(error) => crate::debug::log(
            "clone_tree_record_error",
            serde_json::json!({
                "path": path.display().to_string(),
                "error": error.to_string(),
            }),
        ),
    }
    result
}

fn read_store(path: &Path) -> Result<CloneTreeStore> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(CloneTreeStore::default()),
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(CloneTreeStore::default()),
        Err(e) => Err(e.into()),
    }
}

fn write_store(path: &Path, store: &CloneTreeStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(store)? + "\n")?;
    Ok(())
}

fn current_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn info(provider: Provider, session_id: &str, updated_at_epoch_s: u64) -> SessionInfo {
        SessionInfo {
            provider,
            session_id: session_id.to_string(),
            cwd: "/repo".to_string(),
            source: PathBuf::from("/tmp/source"),
            updated_at_epoch_s,
            title: None,
        }
    }

    fn key(provider: Provider, session_id: &str) -> SessionKey {
        SessionKey::new(provider, session_id)
    }

    fn link(parent: SessionKey, child: SessionKey) -> CloneLink {
        CloneLink {
            parent,
            child,
            cloned_at_epoch_s: 1,
        }
    }

    #[test]
    fn record_clone_is_idempotent_per_child() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(APP_DIR_NAME).join(CLONE_TREE_STORE_FILE);
        let parent = key(Provider::Codex, "p1");
        let child = key(Provider::Claude, "c1");

        record_clone_at_path(&path, parent.clone(), child.clone(), 10).unwrap();
        record_clone_at_path(&path, parent.clone(), child.clone(), 20).unwrap();

        let store = read_store(&path).unwrap();
        assert_eq!(store.links.len(), 1);
        assert_eq!(store.links[0].parent, parent);
        assert_eq!(store.links[0].child, child);
        assert_eq!(store.links[0].cloned_at_epoch_s, 20);
    }

    #[test]
    fn missing_store_reads_as_empty_tree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(APP_DIR_NAME).join(CLONE_TREE_STORE_FILE);

        let store = read_store(&path).unwrap();

        assert!(store.links.is_empty());
    }

    #[test]
    fn tree_rows_include_matching_child_ancestors() {
        let parent = info(Provider::Codex, "parent", 30);
        let child = info(Provider::Claude, "child", 20);
        let grandchild = info(Provider::OpenCode, "grandchild", 10);
        let sessions = vec![parent, child, grandchild];
        let links = vec![
            link(
                key(Provider::Codex, "parent"),
                key(Provider::Claude, "child"),
            ),
            link(
                key(Provider::Claude, "child"),
                key(Provider::OpenCode, "grandchild"),
            ),
        ];

        let rows = visible_tree_rows(&sessions, &links, |info| info.session_id == "grandchild");

        let ids: Vec<(&str, usize)> = rows
            .iter()
            .map(|row| (row.info.session_id.as_str(), row.depth))
            .collect();
        assert_eq!(ids, vec![("parent", 0), ("child", 1), ("grandchild", 2)]);
    }

    #[test]
    fn missing_link_endpoint_is_ignored() {
        let child = info(Provider::Claude, "child", 20);
        let sessions = vec![child];
        let links = vec![link(
            key(Provider::Codex, "missing-parent"),
            key(Provider::Claude, "child"),
        )];

        let rows = visible_tree_rows(&sessions, &links, |_| true);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].info.session_id, "child");
        assert_eq!(rows[0].depth, 0);
    }
}
