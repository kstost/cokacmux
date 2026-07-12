//! Discovery — locate sessions in the agents' default user directories.

use std::path::{Path, PathBuf};

use crate::error::{ConvertError, Result};
use crate::universal::{Provider, UniversalSession};

/// Provider metadata that identifies a session as non-root agent work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRelation {
    SpawnedAgent {
        parent_session_id: String,
        depth: Option<u32>,
    },
    AuxiliaryAgent {
        kind: AuxiliaryAgentKind,
    },
    Internal,
}

/// Codex-managed auxiliary session kinds that are not collaboration spawns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxiliaryAgentKind {
    Review,
    Compact,
    MemoryConsolidation,
    Other,
}

/// Information about a discovered session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub provider: Provider,
    pub session_id: String,
    pub cwd: String,
    pub source: PathBuf, // file path; opencode uses the db file
    pub updated_at_epoch_s: u64,
    pub title: Option<String>,
    /// `None` means top-level or unclassified; callers must keep it visible.
    pub relation: Option<SessionRelation>,
}

/// Pick the most-recently-updated session for `provider` matching `cwd`.
pub fn latest_for_cwd(provider: Provider, cwd: &Path) -> Result<UniversalSession> {
    let target_cwd = cwd.display().to_string();
    crate::debug::log(
        "discovery_latest_for_cwd_start",
        serde_json::json!({
            "provider": provider.as_str(),
            "cwd": &target_cwd,
        }),
    );
    let info = match provider {
        Provider::Claude => latest_claude_by_cwd(&target_cwd)?,
        Provider::Codex => latest_codex_by_cwd(&target_cwd)?,
        Provider::OpenCode => latest_opencode_by_cwd(&target_cwd)?,
        Provider::Pi => latest_pi_by_cwd(&target_cwd)?,
        Provider::Gjc => latest_gjc_by_cwd(&target_cwd)?,
    };
    crate::debug::log(
        "discovery_latest_for_cwd_match",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &info.session_id,
            "source": info.source.display().to_string(),
        }),
    );
    let mut session = match info.provider {
        #[cfg(feature = "claude")]
        Provider::Claude => crate::providers::claude::from_file(&info.source, &Default::default()),
        #[cfg(feature = "codex")]
        Provider::Codex => crate::providers::codex::from_file(&info.source),
        #[cfg(feature = "pi")]
        Provider::Pi => crate::providers::pi::from_file(&info.source),
        #[cfg(feature = "gjc")]
        Provider::Gjc => crate::providers::gjc::from_file(&info.source),
        #[cfg(feature = "opencode")]
        Provider::OpenCode => {
            crate::providers::opencode::from_db_path(&info.source, &info.session_id)
        }
        #[allow(unreachable_patterns)]
        _ => Err(ConvertError::Unsupported(
            "provider feature disabled".into(),
        )),
    }?;
    if session.title.is_none() {
        session.title = info.title;
    }
    crate::debug::log(
        "discovery_latest_for_cwd_ok",
        serde_json::json!({
            "provider": provider.as_str(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
        }),
    );
    Ok(session)
}

pub fn home_dir() -> Result<PathBuf> {
    configured_home_dir().ok_or_else(|| ConvertError::Other("cannot resolve home dir".into()))
}

pub fn configured_home_dir() -> Option<PathBuf> {
    std::env::var_os("COKACMUX_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

pub fn list_all(provider: Provider) -> Result<Vec<SessionInfo>> {
    crate::debug::log(
        "discovery_list_all_start",
        serde_json::json!({
            "provider": provider.as_str(),
        }),
    );
    let result = match provider {
        Provider::Claude => list_claude(),
        Provider::Codex => list_codex(),
        Provider::OpenCode => list_opencode(),
        Provider::Pi => list_pi(),
        Provider::Gjc => list_gjc(),
    };
    match &result {
        Ok(items) => crate::debug::log(
            "discovery_list_all_ok",
            serde_json::json!({
                "provider": provider.as_str(),
                "count": items.len(),
            }),
        ),
        Err(error) => crate::debug::log(
            "discovery_list_all_error",
            serde_json::json!({
                "provider": provider.as_str(),
                "error": error.to_string(),
            }),
        ),
    }
    result
}

// ---------- Claude ----------
fn list_claude() -> Result<Vec<SessionInfo>> {
    let projects = home_dir()?.join(".claude").join("projects");
    if !projects.is_dir() {
        crate::debug::log(
            "discovery_claude_missing_projects",
            serde_json::json!({
                "path": projects.display().to_string(),
            }),
        );
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for proj in std::fs::read_dir(&projects)?.flatten() {
        if !proj.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        for f in std::fs::read_dir(proj.path())?.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = match p.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let mtime = p
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let meta = extract_claude_meta_from_jsonl(&p);
            out.push(SessionInfo {
                provider: Provider::Claude,
                session_id: stem,
                cwd: meta.cwd.unwrap_or_default(),
                source: p,
                updated_at_epoch_s: mtime,
                title: meta.title,
                relation: None,
            });
        }
    }
    out.sort_by_key(|info| std::cmp::Reverse(info.updated_at_epoch_s));
    crate::debug::log(
        "discovery_claude_scan_ok",
        serde_json::json!({
            "projects_path": projects.display().to_string(),
            "count": out.len(),
        }),
    );
    Ok(out)
}

fn latest_claude_by_cwd(cwd: &str) -> Result<SessionInfo> {
    list_claude()?
        .into_iter()
        .find(|i| i.cwd == cwd)
        .ok_or_else(|| ConvertError::Parse(format!("no claude session matching cwd {}", cwd)))
}

const CLAUDE_DISCOVERY_SCAN_LINES: usize = 256;

#[derive(Default)]
struct ClaudeJsonlMeta {
    cwd: Option<String>,
    title: Option<String>,
}

fn extract_claude_meta_from_jsonl(path: &Path) -> ClaudeJsonlMeta {
    use std::io::{BufRead, BufReader};
    let mut meta = ClaudeJsonlMeta::default();
    let Ok(f) = std::fs::File::open(path) else {
        return meta;
    };
    for line in BufReader::new(f)
        .lines()
        .map_while(std::result::Result::ok)
        .take(CLAUDE_DISCOVERY_SCAN_LINES)
    {
        if meta.cwd.is_some() && meta.title.is_some() {
            break;
        }
        if !line.contains("\"cwd\"")
            && !line.contains("\"aiTitle\"")
            && !line.contains("\"customTitle\"")
            && !line.contains("\"agentName\"")
        {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if meta.cwd.is_none() {
                if let Some(c) = v
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .filter(|c| !c.is_empty())
                {
                    meta.cwd = Some(c.to_string());
                }
            }
            let line_type = v.get("type").and_then(|v| v.as_str());
            if meta.title.is_none()
                && matches!(line_type, Some("ai-title" | "custom-title" | "agent-name"))
            {
                if let Some(t) = v
                    .get("aiTitle")
                    .or_else(|| v.get("customTitle"))
                    .or_else(|| v.get("agentName"))
                    .and_then(|v| v.as_str())
                    .filter(|t| !t.is_empty())
                {
                    meta.title = Some(t.to_string());
                }
            }
        }
    }
    meta
}

// ---------- Codex ----------
fn list_codex() -> Result<Vec<SessionInfo>> {
    let sessions = home_dir()?.join(".codex").join("sessions");
    if !sessions.is_dir() {
        crate::debug::log(
            "discovery_codex_missing_sessions",
            serde_json::json!({
                "path": sessions.display().to_string(),
            }),
        );
        return Ok(Vec::new());
    }
    let titles = codex_thread_titles().unwrap_or_default();
    let mut out: Vec<SessionInfo> = Vec::new();
    walk_codex(&sessions, &titles, &mut out);
    out.sort_by_key(|info| std::cmp::Reverse(info.updated_at_epoch_s));
    crate::debug::log(
        "discovery_codex_scan_ok",
        serde_json::json!({
            "sessions_path": sessions.display().to_string(),
            "count": out.len(),
            "titles": titles.len(),
        }),
    );
    Ok(out)
}

fn walk_codex(
    dir: &Path,
    titles: &std::collections::HashMap<String, String>,
    out: &mut Vec<SessionInfo>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Ok(file_type) = e.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk_codex(&p, titles, out);
        } else if file_type.is_file() && p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let Some(sid_candidate) = trailing_uuid_candidate(stem) else {
                crate::debug::log(
                    "discovery_codex_skip_non_uuid_rollout",
                    serde_json::json!({
                        "path": p.display().to_string(),
                        "reason": "stem shorter than UUID length",
                    }),
                );
                continue;
            };
            if uuid::Uuid::parse_str(&sid_candidate).is_err() {
                crate::debug::log(
                    "discovery_codex_skip_non_uuid_rollout",
                    serde_json::json!({
                        "path": p.display().to_string(),
                        "reason": "filename does not end with UUID",
                        "candidate": sid_candidate,
                    }),
                );
                continue;
            }
            let session_id = sid_candidate;
            let title = titles.get(&session_id).cloned().filter(|t| !t.is_empty());
            let mtime = p
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let meta = extract_codex_meta_from_jsonl(&p, &session_id);
            out.push(SessionInfo {
                provider: Provider::Codex,
                session_id,
                cwd: meta.cwd.unwrap_or_default(),
                source: p,
                updated_at_epoch_s: mtime,
                title,
                relation: meta.relation,
            });
        }
    }
}

fn trailing_uuid_candidate(stem: &str) -> Option<String> {
    let chars = stem.chars().collect::<Vec<_>>();
    if chars.len() < 36 {
        return None;
    }
    Some(chars[chars.len() - 36..].iter().collect())
}

#[cfg(feature = "opencode")]
fn codex_thread_titles() -> Result<std::collections::HashMap<String, String>> {
    let db = home_dir()?.join(".codex").join("state_5.sqlite");
    codex_thread_titles_from_db(&db)
}

#[cfg(not(feature = "opencode"))]
fn codex_thread_titles() -> Result<std::collections::HashMap<String, String>> {
    Ok(std::collections::HashMap::new())
}

#[cfg(feature = "opencode")]
fn codex_thread_titles_from_db(path: &Path) -> Result<std::collections::HashMap<String, String>> {
    if !path.is_file() {
        return Ok(std::collections::HashMap::new());
    }
    let conn = crate::providers::opencode::db::open_readonly(path)?;
    let mut stmt = conn.prepare("SELECT id, title FROM threads WHERE title <> ''")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

const CODEX_DISCOVERY_SCAN_LINES: usize = 8;

#[derive(Debug, Default, PartialEq, Eq)]
struct CodexJsonlMeta {
    cwd: Option<String>,
    relation: Option<SessionRelation>,
}

fn extract_codex_meta_from_jsonl(path: &Path, expected_session_id: &str) -> CodexJsonlMeta {
    use std::io::{BufRead, BufReader};

    let Ok(file) = std::fs::File::open(path) else {
        return CodexJsonlMeta::default();
    };
    for line in BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
        .take(CODEX_DISCOVERY_SCAN_LINES)
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = value.get("payload").and_then(serde_json::Value::as_object) else {
            return CodexJsonlMeta::default();
        };
        let cwd = payload
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let id_matches =
            payload.get("id").and_then(serde_json::Value::as_str) == Some(expected_session_id);
        return CodexJsonlMeta {
            cwd,
            relation: if id_matches {
                codex_session_relation(payload.get("source"))
            } else {
                None
            },
        };
    }
    CodexJsonlMeta::default()
}

fn codex_session_relation(source: Option<&serde_json::Value>) -> Option<SessionRelation> {
    let source = source?.as_object()?;
    if source.len() != 1 {
        return None;
    }
    if let Some(subagent) = source.get("subagent") {
        return codex_subagent_relation(subagent);
    }
    source
        .get("internal")
        .and_then(serde_json::Value::as_str)
        .map(|_| SessionRelation::Internal)
}

fn codex_subagent_relation(subagent: &serde_json::Value) -> Option<SessionRelation> {
    match subagent {
        serde_json::Value::String(kind) => Some(SessionRelation::AuxiliaryAgent {
            kind: match kind.as_str() {
                "review" => AuxiliaryAgentKind::Review,
                "compact" => AuxiliaryAgentKind::Compact,
                "memory" | "memory_consolidation" => AuxiliaryAgentKind::MemoryConsolidation,
                _ => AuxiliaryAgentKind::Other,
            },
        }),
        serde_json::Value::Object(tagged) => {
            if tagged.len() == 1 {
                if let Some(spawn) = tagged
                    .get("thread_spawn")
                    .and_then(serde_json::Value::as_object)
                {
                    let parent_session_id = spawn
                        .get("parent_thread_id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|id| !id.is_empty());
                    let depth = spawn
                        .get("depth")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|depth| u32::try_from(depth).ok());
                    if let Some(parent_session_id) = parent_session_id {
                        return Some(SessionRelation::SpawnedAgent {
                            parent_session_id: parent_session_id.to_string(),
                            depth,
                        });
                    }
                }
            }
            Some(SessionRelation::AuxiliaryAgent {
                kind: AuxiliaryAgentKind::Other,
            })
        }
        _ => None,
    }
}

fn latest_codex_by_cwd(cwd: &str) -> Result<SessionInfo> {
    list_codex()?
        .into_iter()
        .find(|i| i.cwd == cwd)
        .ok_or_else(|| ConvertError::Parse(format!("no codex session matching cwd {}", cwd)))
}

// ---------- Pi ----------
#[cfg(feature = "pi")]
fn list_pi() -> Result<Vec<SessionInfo>> {
    let Some(root) = crate::providers::pi::default_sessions_root() else {
        return Ok(Vec::new());
    };
    if !root.is_dir() {
        crate::debug::log(
            "discovery_pi_missing_sessions",
            serde_json::json!({
                "path": root.display().to_string(),
            }),
        );
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    if std::env::var_os(crate::providers::pi::ENV_SESSION_DIR)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        scan_pi_session_dir(&root, &mut out);
    } else {
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(Vec::new());
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false) {
                scan_pi_session_dir(&entry.path(), &mut out);
            }
        }
    }
    out.sort_by_key(|info| std::cmp::Reverse(info.updated_at_epoch_s));
    crate::debug::log(
        "discovery_pi_scan_ok",
        serde_json::json!({
            "sessions_path": root.display().to_string(),
            "count": out.len(),
        }),
    );
    Ok(out)
}

#[cfg(not(feature = "pi"))]
fn list_pi() -> Result<Vec<SessionInfo>> {
    Err(ConvertError::Unsupported("pi feature not enabled".into()))
}

#[cfg(feature = "pi")]
fn scan_pi_session_dir(dir: &Path, out: &mut Vec<SessionInfo>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|ty| ty.is_file()).unwrap_or(false)
            || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
        {
            continue;
        }
        let Some(meta) = extract_pi_meta_from_jsonl(&path) else {
            continue;
        };
        let mtime = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(SessionInfo {
            provider: Provider::Pi,
            session_id: meta.session_id,
            cwd: meta.cwd,
            source: path,
            updated_at_epoch_s: meta.updated_at_epoch_s.unwrap_or(mtime),
            title: meta.title,
            relation: None,
        });
    }
}

#[cfg(feature = "pi")]
#[derive(Debug, Default)]
struct PiJsonlMeta {
    session_id: String,
    cwd: String,
    title: Option<String>,
    updated_at_epoch_s: Option<u64>,
}

#[cfg(feature = "pi")]
fn extract_pi_meta_from_jsonl(path: &Path) -> Option<PiJsonlMeta> {
    use std::io::{BufRead, BufReader};

    let file = std::fs::File::open(path).ok()?;
    let mut meta = PiJsonlMeta::default();
    let mut header_seen = false;
    for line in BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
    {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if !header_seen {
            header_seen = true;
            if value.get("type").and_then(|ty| ty.as_str()) != Some("session") {
                return None;
            }
            meta.session_id = value.get("id").and_then(|id| id.as_str())?.to_string();
            meta.cwd = value
                .get("cwd")
                .and_then(|cwd| cwd.as_str())
                .unwrap_or_default()
                .to_string();
            meta.updated_at_epoch_s = value
                .get("timestamp")
                .and_then(|ts| ts.as_str())
                .and_then(crate::time::parse_rfc3339)
                .map(|dt| dt.timestamp().max(0) as u64);
            continue;
        }
        match value.get("type").and_then(|ty| ty.as_str()) {
            Some("session_info") => {
                if let Some(title) = value
                    .get("name")
                    .and_then(|name| name.as_str())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                {
                    meta.title = Some(title);
                }
            }
            Some("message") => {
                let message = value.get("message").unwrap_or(&serde_json::Value::Null);
                let role = message.get("role").and_then(|role| role.as_str());
                if meta.title.is_none() {
                    if let Some(title) = crate::providers::pi::user_message_title(message) {
                        meta.title = Some(title);
                    }
                }
                if matches!(role, Some("user" | "assistant")) {
                    if let Some(epoch_s) = value
                        .get("message")
                        .and_then(|message| message.get("timestamp"))
                        .and_then(|ts| ts.as_i64())
                        .map(|ms| (ms / 1000).max(0) as u64)
                        .or_else(|| {
                            value
                                .get("timestamp")
                                .and_then(|ts| ts.as_str())
                                .and_then(crate::time::parse_rfc3339)
                                .map(|dt| dt.timestamp().max(0) as u64)
                        })
                    {
                        meta.updated_at_epoch_s =
                            Some(meta.updated_at_epoch_s.unwrap_or(0).max(epoch_s));
                    }
                }
            }
            _ => {}
        }
    }
    (!meta.session_id.is_empty()).then_some(meta)
}

fn latest_pi_by_cwd(cwd: &str) -> Result<SessionInfo> {
    list_pi()?
        .into_iter()
        .find(|i| i.cwd == cwd)
        .ok_or_else(|| ConvertError::Parse(format!("no pi session matching cwd {}", cwd)))
}

// ---------- GJC ----------
#[cfg(feature = "gjc")]
fn list_gjc() -> Result<Vec<SessionInfo>> {
    let Some(root) = crate::providers::gjc::default_sessions_root() else {
        return Ok(Vec::new());
    };
    if !root.is_dir() {
        crate::debug::log(
            "discovery_gjc_missing_sessions",
            serde_json::json!({
                "path": root.display().to_string(),
            }),
        );
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false) {
            scan_gjc_session_dir(&entry.path(), &mut out);
        }
    }
    out.sort_by_key(|info| std::cmp::Reverse(info.updated_at_epoch_s));
    crate::debug::log(
        "discovery_gjc_scan_ok",
        serde_json::json!({
            "sessions_path": root.display().to_string(),
            "count": out.len(),
        }),
    );
    Ok(out)
}

#[cfg(not(feature = "gjc"))]
fn list_gjc() -> Result<Vec<SessionInfo>> {
    Err(ConvertError::Unsupported("gjc feature not enabled".into()))
}

#[cfg(feature = "gjc")]
fn scan_gjc_session_dir(dir: &Path, out: &mut Vec<SessionInfo>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|ty| ty.is_file()).unwrap_or(false)
            || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
        {
            continue;
        }
        let Some(meta) = extract_gjc_meta_from_jsonl(&path) else {
            continue;
        };
        let mtime = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(SessionInfo {
            provider: Provider::Gjc,
            session_id: meta.session_id,
            cwd: meta.cwd,
            source: path,
            updated_at_epoch_s: meta.updated_at_epoch_s.unwrap_or(mtime),
            title: meta.title,
            relation: None,
        });
    }
}

#[cfg(feature = "gjc")]
#[derive(Debug, Default)]
struct GjcJsonlMeta {
    session_id: String,
    cwd: String,
    title: Option<String>,
    updated_at_epoch_s: Option<u64>,
}

#[cfg(feature = "gjc")]
fn extract_gjc_meta_from_jsonl(path: &Path) -> Option<GjcJsonlMeta> {
    use std::io::{BufRead, BufReader};

    let file = std::fs::File::open(path).ok()?;
    let mut meta = GjcJsonlMeta::default();
    let mut header_seen = false;
    for line in BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
    {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if !header_seen {
            header_seen = true;
            if value.get("type").and_then(|ty| ty.as_str()) != Some("session") {
                return None;
            }
            meta.session_id = value.get("id").and_then(|id| id.as_str())?.to_string();
            meta.cwd = value
                .get("cwd")
                .and_then(|cwd| cwd.as_str())
                .unwrap_or_default()
                .to_string();
            meta.title = value
                .get("title")
                .and_then(|title| title.as_str())
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_string);
            meta.updated_at_epoch_s = value
                .get("timestamp")
                .and_then(|ts| ts.as_str())
                .and_then(crate::time::parse_rfc3339)
                .map(|dt| dt.timestamp().max(0) as u64);
            continue;
        }
        match value.get("type").and_then(|ty| ty.as_str()) {
            Some("session_info") => {
                if meta.title.is_none() {
                    if let Some(title) = value
                        .get("name")
                        .and_then(|name| name.as_str())
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                    {
                        meta.title = Some(title);
                    }
                }
            }
            Some("compaction") => {
                if meta.title.is_none() {
                    if let Some(title) = value
                        .get("shortSummary")
                        .and_then(|name| name.as_str())
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                    {
                        meta.title = Some(title);
                    }
                }
            }
            Some("message") => {
                let message = value.get("message").unwrap_or(&serde_json::Value::Null);
                let role = message.get("role").and_then(|role| role.as_str());
                if meta.title.is_none() {
                    if let Some(title) = crate::providers::gjc::user_message_title(message) {
                        meta.title = Some(title);
                    }
                }
                if matches!(role, Some("user" | "assistant")) {
                    if let Some(epoch_s) = value
                        .get("message")
                        .and_then(|message| message.get("timestamp"))
                        .and_then(|ts| ts.as_i64())
                        .map(|ms| (ms / 1000).max(0) as u64)
                        .or_else(|| {
                            value
                                .get("timestamp")
                                .and_then(|ts| ts.as_str())
                                .and_then(crate::time::parse_rfc3339)
                                .map(|dt| dt.timestamp().max(0) as u64)
                        })
                    {
                        meta.updated_at_epoch_s =
                            Some(meta.updated_at_epoch_s.unwrap_or(0).max(epoch_s));
                    }
                }
            }
            _ => {}
        }
    }
    (!meta.session_id.is_empty()).then_some(meta)
}

fn latest_gjc_by_cwd(cwd: &str) -> Result<SessionInfo> {
    list_gjc()?
        .into_iter()
        .find(|i| i.cwd == cwd)
        .ok_or_else(|| ConvertError::Parse(format!("no gjc session matching cwd {}", cwd)))
}

// ---------- OpenCode ----------
#[cfg(feature = "opencode")]
fn list_opencode() -> Result<Vec<SessionInfo>> {
    let Some(db) = default_opencode_db_candidates()
        .into_iter()
        .find(|p| p.is_file())
    else {
        crate::debug::log("discovery_opencode_missing_db", serde_json::json!({}));
        return Ok(Vec::new());
    };
    if !db.is_file() {
        crate::debug::log(
            "discovery_opencode_missing_db",
            serde_json::json!({
                "db_path": db.display().to_string(),
            }),
        );
        return Ok(Vec::new());
    }
    let conn = crate::providers::opencode::db::open_readonly(&db)?;
    let mut stmt = conn.prepare(
        "SELECT id, directory, title, time_updated FROM session ORDER BY time_updated DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SessionInfo {
            provider: Provider::OpenCode,
            session_id: row.get::<_, String>(0)?,
            cwd: row.get::<_, String>(1)?,
            source: db.clone(),
            updated_at_epoch_s: (row.get::<_, i64>(3)? / 1000).max(0) as u64,
            title: {
                let t: String = row.get(2)?;
                if t.is_empty() {
                    None
                } else {
                    Some(t)
                }
            },
            relation: None,
        })
    })?;
    let out: Vec<SessionInfo> = rows.filter_map(|r| r.ok()).collect();
    crate::debug::log(
        "discovery_opencode_scan_ok",
        serde_json::json!({
            "db_path": db.display().to_string(),
            "count": out.len(),
        }),
    );
    Ok(out)
}

#[cfg(not(feature = "opencode"))]
fn list_opencode() -> Result<Vec<SessionInfo>> {
    Err(ConvertError::Unsupported(
        "opencode feature not enabled".into(),
    ))
}

fn latest_opencode_by_cwd(cwd: &str) -> Result<SessionInfo> {
    list_opencode()?
        .into_iter()
        .find(|i| i.cwd == cwd)
        .ok_or_else(|| ConvertError::Parse(format!("no opencode session matching cwd {}", cwd)))
}

/// Find the single top-level OpenCode session in `cwd` created at or after
/// `created_after_epoch_ms`. Returns `(session_id, db_path)` only when exactly
/// one row qualifies — zero or several candidates yield None so callers never
/// link the wrong session.
#[cfg(feature = "opencode")]
pub fn unique_opencode_session_created_after(
    cwd: &str,
    created_after_epoch_ms: i64,
) -> Option<(String, std::path::PathBuf)> {
    let db = default_opencode_db_candidates()
        .into_iter()
        .find(|p| p.is_file())?;
    match unique_opencode_session_created_after_in_db(&db, cwd, created_after_epoch_ms) {
        Ok(Some(session_id)) => Some((session_id, db)),
        Ok(None) => None,
        Err(error) => {
            crate::debug::log(
                "discovery_opencode_created_after_error",
                serde_json::json!({
                    "db_path": db.display().to_string(),
                    "cwd": cwd,
                    "created_after_epoch_ms": created_after_epoch_ms,
                    "error": error.to_string(),
                }),
            );
            None
        }
    }
}

#[cfg(not(feature = "opencode"))]
pub fn unique_opencode_session_created_after(
    _cwd: &str,
    _created_after_epoch_ms: i64,
) -> Option<(String, std::path::PathBuf)> {
    None
}

#[cfg(feature = "opencode")]
fn unique_opencode_session_created_after_in_db(
    db: &Path,
    cwd: &str,
    created_after_epoch_ms: i64,
) -> Result<Option<String>> {
    let conn = crate::providers::opencode::db::open_readonly(db)?;
    let mut stmt = conn.prepare(
        "SELECT id FROM session \
         WHERE directory = ?1 AND time_created >= ?2 AND parent_id IS NULL",
    )?;
    let ids: Vec<String> = stmt
        .query_map(rusqlite::params![cwd, created_after_epoch_ms], |row| {
            row.get::<_, String>(0)
        })?
        .filter_map(|r| r.ok())
        .collect();
    match ids.as_slice() {
        [id] => Ok(Some(id.clone())),
        [] => Ok(None),
        many => {
            crate::debug::log(
                "discovery_opencode_created_after_ambiguous",
                serde_json::json!({
                    "db_path": db.display().to_string(),
                    "cwd": cwd,
                    "created_after_epoch_ms": created_after_epoch_ms,
                    "matches": many,
                }),
            );
            Ok(None)
        }
    }
}

#[cfg(feature = "opencode")]
fn default_opencode_db_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(home) = home_dir() {
        paths.push(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
        );
    }
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local_app_data)
                .join("opencode")
                .join("opencode.db"),
        );
    }
    if let Ok(app_data) = std::env::var("APPDATA") {
        paths.push(PathBuf::from(app_data).join("opencode").join("opencode.db"));
    }
    paths
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn extracts_claude_cwd_and_ai_title_for_discovery() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"permission-mode","sessionId":"s1","permissionMode":"default"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"user","sessionId":"s1","cwd":"/tmp/project","message":{{"role":"user","content":"hello"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"ai-title","sessionId":"s1","aiTitle":"Generated Claude Title"}}"#
        )
        .unwrap();

        let meta = extract_claude_meta_from_jsonl(file.path());

        assert_eq!(meta.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(meta.title.as_deref(), Some("Generated Claude Title"));
    }

    #[test]
    fn extracts_custom_title_for_discovery_as_fallback() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"ai-title","sessionId":"s1","customTitle":"Legacy Claude Title"}}"#
        )
        .unwrap();

        let meta = extract_claude_meta_from_jsonl(file.path());

        assert_eq!(meta.title.as_deref(), Some("Legacy Claude Title"));
    }

    #[test]
    fn extracts_custom_title_record_for_discovery() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"custom-title","sessionId":"s1","customTitle":"Manual Claude Title"}}"#
        )
        .unwrap();

        let meta = extract_claude_meta_from_jsonl(file.path());

        assert_eq!(meta.title.as_deref(), Some("Manual Claude Title"));
    }

    #[test]
    fn extracts_agent_name_record_for_discovery_as_fallback() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"agent-name","sessionId":"s1","agentName":"Named Agent"}}"#
        )
        .unwrap();

        let meta = extract_claude_meta_from_jsonl(file.path());

        assert_eq!(meta.title.as_deref(), Some("Named Agent"));
    }

    fn codex_meta_file(lines: &[serde_json::Value]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file
    }

    #[test]
    fn codex_root_session_has_no_relation_even_with_legacy_subagent_hints() {
        let session_id = "019f5150-9e93-70f2-b01b-9099a3126b32";
        let file = codex_meta_file(&[serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "cwd": "/repo/root",
                "source": "cli",
                "thread_source": "subagent",
                "forked_from_id": "019f5150-bf9f-7d53-a832-69bf0d220320"
            }
        })]);

        let meta = extract_codex_meta_from_jsonl(file.path(), session_id);

        assert_eq!(meta.cwd.as_deref(), Some("/repo/root"));
        assert_eq!(meta.relation, None);
    }

    #[test]
    fn codex_thread_spawn_relation_keeps_parent_and_optional_depth() {
        let session_id = "019f5150-9e93-70f2-b01b-9099a3126b32";
        let parent_id = "019f5150-bf9f-7d53-a832-69bf0d220320";
        let file = codex_meta_file(&[serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "cwd": "/repo/child",
                "source": {
                    "subagent": {
                        "thread_spawn": {
                            "parent_thread_id": parent_id,
                            "depth": 2,
                            "agent_nickname": "redacted"
                        }
                    }
                }
            }
        })]);

        let meta = extract_codex_meta_from_jsonl(file.path(), session_id);

        assert_eq!(meta.cwd.as_deref(), Some("/repo/child"));
        assert_eq!(
            meta.relation,
            Some(SessionRelation::SpawnedAgent {
                parent_session_id: parent_id.to_string(),
                depth: Some(2),
            })
        );

        let file_without_depth = codex_meta_file(&[serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "cwd": "/repo/child",
                "source": {
                    "subagent": {
                        "thread_spawn": {"parent_thread_id": parent_id}
                    }
                }
            }
        })]);
        let meta_without_depth =
            extract_codex_meta_from_jsonl(file_without_depth.path(), session_id);
        assert_eq!(
            meta_without_depth.relation,
            Some(SessionRelation::SpawnedAgent {
                parent_session_id: parent_id.to_string(),
                depth: None,
            })
        );

        let file_with_invalid_depth = codex_meta_file(&[serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "cwd": "/repo/child",
                "source": {
                    "subagent": {
                        "thread_spawn": {
                            "parent_thread_id": parent_id,
                            "depth": -1
                        }
                    }
                }
            }
        })]);
        let meta_with_invalid_depth =
            extract_codex_meta_from_jsonl(file_with_invalid_depth.path(), session_id);
        assert_eq!(
            meta_with_invalid_depth.relation,
            Some(SessionRelation::SpawnedAgent {
                parent_session_id: parent_id.to_string(),
                depth: None,
            })
        );
    }

    #[test]
    fn codex_review_session_is_an_auxiliary_agent() {
        let session_id = "019f5150-9e93-70f2-b01b-9099a3126b32";
        let file = codex_meta_file(&[serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "cwd": "/repo/review",
                "source": {"subagent": "review"}
            }
        })]);

        let meta = extract_codex_meta_from_jsonl(file.path(), session_id);

        assert_eq!(
            meta.relation,
            Some(SessionRelation::AuxiliaryAgent {
                kind: AuxiliaryAgentKind::Review,
            })
        );
    }

    #[test]
    fn codex_classifies_other_auxiliary_and_internal_sources() {
        let session_id = "019f5150-9e93-70f2-b01b-9099a3126b32";
        let cases = [
            (
                serde_json::json!({"subagent": "compact"}),
                SessionRelation::AuxiliaryAgent {
                    kind: AuxiliaryAgentKind::Compact,
                },
            ),
            (
                serde_json::json!({"subagent": "memory_consolidation"}),
                SessionRelation::AuxiliaryAgent {
                    kind: AuxiliaryAgentKind::MemoryConsolidation,
                },
            ),
            (
                serde_json::json!({"subagent": {"other": "guardian"}}),
                SessionRelation::AuxiliaryAgent {
                    kind: AuxiliaryAgentKind::Other,
                },
            ),
            (
                serde_json::json!({"internal": "memory_consolidation"}),
                SessionRelation::Internal,
            ),
        ];

        for (source, expected) in cases {
            let file = codex_meta_file(&[serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "cwd": "/repo/auxiliary",
                    "source": source
                }
            })]);

            let meta = extract_codex_meta_from_jsonl(file.path(), session_id);

            assert_eq!(meta.relation, Some(expected));
        }
    }

    #[test]
    fn codex_discovery_uses_only_the_first_canonical_session_meta() {
        let session_id = "019f5150-9e93-70f2-b01b-9099a3126b32";
        let parent_id = "019f5150-bf9f-7d53-a832-69bf0d220320";
        let file = codex_meta_file(&[
            serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "id": session_id,
                    "cwd": "/repo/child",
                    "source": {
                        "subagent": {
                            "thread_spawn": {
                                "parent_thread_id": parent_id,
                                "depth": 1
                            }
                        }
                    }
                }
            }),
            serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "id": parent_id,
                    "cwd": "/repo/copied-parent",
                    "source": "cli"
                }
            }),
        ]);

        let meta = extract_codex_meta_from_jsonl(file.path(), session_id);

        assert_eq!(meta.cwd.as_deref(), Some("/repo/child"));
        assert_eq!(
            meta.relation,
            Some(SessionRelation::SpawnedAgent {
                parent_session_id: parent_id.to_string(),
                depth: Some(1),
            })
        );
    }

    #[test]
    fn codex_malformed_source_has_no_relation() {
        let session_id = "019f5150-9e93-70f2-b01b-9099a3126b32";
        let file = codex_meta_file(&[serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "cwd": "/repo/root",
                "source": {"subagent": 42}
            }
        })]);

        let meta = extract_codex_meta_from_jsonl(file.path(), session_id);

        assert_eq!(meta.cwd.as_deref(), Some("/repo/root"));
        assert_eq!(meta.relation, None);
    }

    #[test]
    fn codex_mismatched_payload_id_keeps_session_and_cwd_but_not_relation() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = "019f5150-9e93-70f2-b01b-9099a3126b32";
        let path = dir
            .path()
            .join(format!("rollout-2026-07-11T00-00-00-{session_id}.jsonl"));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "id": "019f5150-bf9f-7d53-a832-69bf0d220320",
                    "cwd": "/repo/untrusted",
                    "source": {"subagent": "review"}
                }
            })
        )
        .unwrap();

        let mut discovered = Vec::new();
        walk_codex(
            dir.path(),
            &std::collections::HashMap::new(),
            &mut discovered,
        );

        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].session_id, session_id);
        assert_eq!(discovered[0].cwd, "/repo/untrusted");
        assert_eq!(discovered[0].relation, None);
    }

    #[cfg(feature = "pi")]
    #[test]
    fn extracts_pi_first_user_message_as_title_for_discovery() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"s1","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp/project"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"u1","parentId":null,"timestamp":"2026-05-20T01:00:01.000Z","message":{{"role":"user","content":"  First\n\nuser   prompt  ","timestamp":1779240001000}}}}"#
        )
        .unwrap();

        let meta = extract_pi_meta_from_jsonl(file.path()).unwrap();

        assert_eq!(meta.title.as_deref(), Some("First user prompt"));
    }

    #[cfg(feature = "pi")]
    #[test]
    fn extracts_pi_session_info_title_over_first_user_fallback() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"s1","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp/project"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"u1","parentId":null,"timestamp":"2026-05-20T01:00:01.000Z","message":{{"role":"user","content":[{{"type":"text","text":"first user"}},{{"type":"image","data":"abc"}}],"timestamp":1779240001000}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"session_info","id":"info","parentId":"u1","timestamp":"2026-05-20T01:00:02.000Z","name":"Named Pi Session"}}"#
        )
        .unwrap();

        let meta = extract_pi_meta_from_jsonl(file.path()).unwrap();

        assert_eq!(meta.title.as_deref(), Some("Named Pi Session"));
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn extracts_codex_titles_from_state_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state_5.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT NOT NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, title) VALUES (?1, ?2)",
            rusqlite::params!["s1", "Codex Thread Title"],
        )
        .unwrap();

        let titles = codex_thread_titles_from_db(&db).unwrap();

        assert_eq!(
            titles.get("s1").map(String::as_str),
            Some("Codex Thread Title")
        );
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn unique_opencode_session_created_after_requires_single_match() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let conn = crate::providers::opencode::db::open_readwrite(&db).unwrap();
        conn.execute_batch(crate::providers::opencode::db::SCHEMA_MIN)
            .unwrap();
        let insert = "INSERT INTO session \
             (id, project_id, parent_id, directory, time_created, time_updated) \
             VALUES (?1, 'proj', ?2, ?3, ?4, ?4)";
        conn.execute(
            insert,
            rusqlite::params!["ses_old", None::<String>, "/repo", 1_000i64],
        )
        .unwrap();

        // Sessions created before the cutoff never match.
        assert_eq!(
            unique_opencode_session_created_after_in_db(&db, "/repo", 2_000).unwrap(),
            None
        );

        conn.execute(
            insert,
            rusqlite::params!["ses_fresh", None::<String>, "/repo", 3_000i64],
        )
        .unwrap();
        conn.execute(
            insert,
            rusqlite::params!["ses_child", "ses_fresh", "/repo", 3_500i64],
        )
        .unwrap();
        conn.execute(
            insert,
            rusqlite::params!["ses_other", None::<String>, "/elsewhere", 3_000i64],
        )
        .unwrap();

        // Child sessions and other directories are ignored.
        assert_eq!(
            unique_opencode_session_created_after_in_db(&db, "/repo", 2_000).unwrap(),
            Some("ses_fresh".to_string())
        );

        conn.execute(
            insert,
            rusqlite::params!["ses_second", None::<String>, "/repo", 4_000i64],
        )
        .unwrap();

        // Two qualifying sessions are ambiguous: link nothing.
        assert_eq!(
            unique_opencode_session_created_after_in_db(&db, "/repo", 2_000).unwrap(),
            None
        );
    }
}
