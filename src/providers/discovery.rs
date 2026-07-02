//! Discovery — locate sessions in the agents' default user directories.

use std::path::{Path, PathBuf};

use crate::error::{ConvertError, Result};
use crate::universal::{Provider, UniversalSession};

/// Information about a discovered session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub provider: Provider,
    pub session_id: String,
    pub cwd: String,
    pub source: PathBuf, // file path; opencode uses the db file
    pub updated_at_epoch_s: u64,
    pub title: Option<String>,
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
            });
        }
    }
    out.sort_by(|a, b| b.updated_at_epoch_s.cmp(&a.updated_at_epoch_s));
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
        .flatten()
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
    out.sort_by(|a, b| b.updated_at_epoch_s.cmp(&a.updated_at_epoch_s));
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
            let cwd = extract_cwd_from_codex(&p).unwrap_or_default();
            out.push(SessionInfo {
                provider: Provider::Codex,
                session_id,
                cwd,
                source: p,
                updated_at_epoch_s: mtime,
                title,
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

fn extract_cwd_from_codex(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    for line in BufReader::new(f).lines().flatten().take(8) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
                if let Some(c) = v
                    .get("payload")
                    .and_then(|p| p.get("cwd"))
                    .and_then(|v| v.as_str())
                {
                    return Some(c.to_string());
                }
            }
        }
    }
    None
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
    out.sort_by(|a, b| b.updated_at_epoch_s.cmp(&a.updated_at_epoch_s));
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
    for line in BufReader::new(file).lines().flatten() {
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
    out.sort_by(|a, b| b.updated_at_epoch_s.cmp(&a.updated_at_epoch_s));
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
    for line in BufReader::new(file).lines().flatten() {
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
    if let Ok(home) = home_dir() {
        paths.push(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
        );
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
