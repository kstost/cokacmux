//! Clone a session by copying provider-native storage and patching only the
//! identifiers that must be unique for the clone.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::error::{ConvertError, Result};
use crate::providers;
use crate::providers::discovery::SessionInfo;
use crate::universal::Provider;

#[derive(Debug)]
pub struct CloneOpts {
    /// Override the target provider. Defaults to the source provider. Clone
    /// only supports same-provider copies; cross-provider data creation must
    /// use conversion instead.
    pub to: Option<Provider>,
    /// Override cwd on the new session. Defaults to the source cwd.
    pub cwd: Option<String>,
    /// If true and target already has a session with the new id, replace it.
    pub overwrite: bool,
    /// Override the new id (otherwise a fresh provider-native id is minted).
    pub new_id: Option<String>,
}

impl Default for CloneOpts {
    fn default() -> Self {
        Self {
            to: None,
            cwd: None,
            overwrite: false,
            new_id: None,
        }
    }
}

#[derive(Debug)]
pub struct CloneReport {
    pub source_provider: Provider,
    pub source_session_id: String,
    pub new_session_id: String,
    pub target_provider: Provider,
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

/// Clone the session described by `src` into the same provider's live
/// storage with a new session id. Returns the path/id of the new artifact.
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
        }),
    );

    let target_provider = opts.to.unwrap_or(src.provider);
    if target_provider != src.provider {
        crate::debug::log(
            "clone_cross_provider_rejected",
            serde_json::json!({
                "source_provider": src.provider.as_str(),
                "source_session_id": &src.session_id,
                "target_provider": target_provider.as_str(),
            }),
        );
        return Err(ConvertError::Unsupported(format!(
            "cross-provider clone is not supported because clone must preserve native provider data; use convert to create a {} session from {}",
            target_provider.as_str(),
            src.provider.as_str()
        )));
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
    }
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
    let repaired = repair_claude_parent_chain(&mut lines);
    let sanitized = sanitize_claude_content_blocks(&mut lines);
    let bytes_written = write_jsonl_lines_atomic(&target, opts.overwrite, &lines)?;
    let artifact = ArtifactPath::File(target.clone());
    if let Err(error) = clone_claude_sidecar(&src.source, &target, opts.overwrite) {
        let _ = fs::remove_file(&target);
        return Err(error);
    }
    let validation = ensure_clone_artifact_native_or_cleanup(
        Provider::Claude,
        &new_id,
        &new_cwd,
        &artifact,
        opts,
    )?;
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
            "native_validation_checks": validation.checks.len(),
        }),
    );
    Ok(CloneReport {
        source_provider: Provider::Claude,
        source_session_id: src.session_id.clone(),
        new_session_id: new_id,
        target_provider: Provider::Claude,
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
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| src.cwd.clone());
    if new_cwd.is_empty() {
        return Err(ConvertError::MissingField("session.cwd"));
    }
    let target = codex_rollout_path(codex_home, Utc::now(), &new_id);
    let mut lines = read_jsonl_lines(&src.source)?;
    patch_codex_jsonl_lines(&mut lines, &src.session_id, &new_id, &src.cwd, &new_cwd);
    let bytes_written = write_jsonl_lines_atomic(&target, opts.overwrite, &lines)?;
    let artifact = ArtifactPath::File(target.clone());

    if let Err(error) = copy_codex_state_thread_row(
        codex_home,
        &src.session_id,
        &new_id,
        &target,
        &new_cwd,
        opts.overwrite,
    ) {
        let _ = remove_installed_clone_artifact(Provider::Codex, &new_id, &new_cwd, &artifact);
        return Err(error);
    }

    let validation = ensure_clone_artifact_native_or_cleanup(
        Provider::Codex,
        &new_id,
        &new_cwd,
        &artifact,
        opts,
    )?;
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
        artifact,
    })
}

/// Same-provider OpenCode clone: SQL row-level copy. Preserves every column
/// of every row in the origin session, including provider-specific columns
/// and every internal JSON field. Only database identifiers and id references
/// that must become unique are remapped.
#[cfg(feature = "opencode")]
fn clone_opencode_same_provider(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    let report = providers::opencode::clone::clone_session_rows(
        &src.source,
        &src.session_id,
        &providers::opencode::clone::OpenCodeRowCloneOpts {
            new_session_id: opts.new_id.clone(),
            cwd: opts.cwd.clone(),
            overwrite: opts.overwrite,
        },
    )?;
    let new_id = report.new_session_id.clone();
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| src.cwd.clone());
    let artifact = ArtifactPath::OpenCodeDb {
        db_path: report.db_path,
        session_id: new_id.clone(),
    };
    let validation = ensure_clone_artifact_native_or_cleanup(
        Provider::OpenCode,
        &new_id,
        &new_cwd,
        &artifact,
        opts,
    )?;
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
        artifact,
    })
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

fn write_jsonl_lines_atomic(path: &Path, overwrite: bool, lines: &[JsonLine]) -> Result<u64> {
    if path.exists() && !overwrite {
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
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
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
        if path.exists() && overwrite {
            fs::remove_file(path)?;
        }
        fs::rename(&tmp_path, path)?;
        Ok(fs::metadata(path).map(|m| m.len()).unwrap_or(0))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
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

fn repair_claude_parent_chain(lines: &mut [JsonLine]) -> usize {
    let mut previous_uuid: Option<String> = None;
    let mut leaf_uuid: Option<String> = None;
    let mut updated = 0usize;

    for line in lines.iter_mut() {
        let JsonLine::Json(Value::Object(map)) = line else {
            continue;
        };
        let Some(kind) = map.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(kind, "user" | "assistant") {
            continue;
        }
        let Some(uuid) = map.get("uuid").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        map.insert(
            "parentUuid".into(),
            previous_uuid
                .as_ref()
                .map(|parent| Value::String(parent.clone()))
                .unwrap_or(Value::Null),
        );
        updated = updated.saturating_add(1);
        previous_uuid = Some(uuid.clone());
        leaf_uuid = Some(uuid);
    }

    let Some(leaf_uuid) = leaf_uuid else {
        return updated;
    };
    for line in lines {
        let JsonLine::Json(Value::Object(map)) = line else {
            continue;
        };
        if map.get("type").and_then(Value::as_str) == Some("last-prompt") {
            map.insert("leafUuid".into(), Value::String(leaf_uuid.clone()));
            updated = updated.saturating_add(1);
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

fn codex_rollout_path(codex_home: &Path, ts: DateTime<Utc>, session_id: &str) -> PathBuf {
    codex_home
        .join("sessions")
        .join(format!("{:04}", ts.format("%Y")))
        .join(format!("{:02}", ts.format("%m")))
        .join(format!("{:02}", ts.format("%d")))
        .join(format!(
            "rollout-{}-{session_id}.jsonl",
            ts.format("%Y-%m-%dT%H-%M-%S")
        ))
}

fn clone_claude_sidecar(source_jsonl: &Path, target_jsonl: &Path, overwrite: bool) -> Result<()> {
    let source_sidecar = source_jsonl.with_extension("");
    if !source_sidecar.is_dir() {
        return Ok(());
    }
    let target_sidecar = target_jsonl.with_extension("");
    if target_sidecar.exists() {
        if !overwrite {
            return Err(ConvertError::Other(format!(
                "claude sidecar target already exists at {} (set overwrite=true to replace)",
                target_sidecar.display()
            )));
        }
        if target_sidecar.is_dir() {
            fs::remove_dir_all(&target_sidecar)?;
        } else {
            fs::remove_file(&target_sidecar)?;
        }
    }
    let sidecar_name = target_sidecar
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sidecar");
    let tmp_sidecar =
        target_sidecar.with_file_name(format!(".{}.tmp-{}", sidecar_name, uuid::Uuid::now_v7()));
    let copy_result = copy_dir_recursive(&source_sidecar, &tmp_sidecar);
    if let Err(error) = copy_result {
        let _ = fs::remove_dir_all(&tmp_sidecar);
        return Err(error);
    }
    if target_sidecar.exists() && overwrite {
        if target_sidecar.is_dir() {
            fs::remove_dir_all(&target_sidecar)?;
        } else {
            fs::remove_file(&target_sidecar)?;
        }
    }
    if let Err(error) = fs::rename(&tmp_sidecar, &target_sidecar) {
        let _ = fs::remove_dir_all(&tmp_sidecar);
        return Err(error.into());
    }
    Ok(())
}

fn ensure_claude_sidecar_target_available(
    source_jsonl: &Path,
    target_jsonl: &Path,
    overwrite: bool,
) -> Result<()> {
    let source_sidecar = source_jsonl.with_extension("");
    if !source_sidecar.is_dir() {
        return Ok(());
    }
    let target_sidecar = target_jsonl.with_extension("");
    if target_sidecar.exists() && !overwrite {
        return Err(ConvertError::Other(format!(
            "claude sidecar target already exists at {} (set overwrite=true to replace)",
            target_sidecar.display()
        )));
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_symlink() {
            copy_symlink(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path)?;
        }
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
    dirs::home_dir().map(|home| home.join(".claude"))
}

#[cfg(not(feature = "discovery"))]
fn default_claude_home() -> Option<PathBuf> {
    None
}

#[cfg(feature = "discovery")]
fn default_codex_home() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".codex"))
}

#[cfg(not(feature = "discovery"))]
fn default_codex_home() -> Option<PathBuf> {
    None
}

#[cfg(feature = "opencode")]
fn copy_codex_state_thread_row(
    codex_home: &Path,
    source_session_id: &str,
    new_session_id: &str,
    rollout_path: &Path,
    cwd: &str,
    overwrite: bool,
) -> Result<bool> {
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
        return Ok(false);
    }
    let mut conn = rusqlite::Connection::open(&state_5)?;
    let columns = ordered_table_columns(&conn, "threads")?;
    for required in ["id", "rollout_path", "cwd"] {
        if !columns.iter().any(|column| column == required) {
            return Err(ConvertError::Other(format!(
                "threads table missing expected column `{required}` (state_5.sqlite schema drift?)"
            )));
        }
    }
    let tx = conn.transaction()?;
    let existing = tx.query_row(
        "SELECT COUNT(*) FROM threads WHERE id = ?1",
        rusqlite::params![new_session_id],
        |row| row.get::<_, i64>(0),
    )?;
    if existing > 0 && !overwrite {
        return Err(ConvertError::Other(format!(
            "codex state row already exists for {} (set overwrite=true to replace)",
            new_session_id
        )));
    }

    let select_sql = format!(
        "SELECT {} FROM threads WHERE id = ?1",
        columns
            .iter()
            .map(|column| quote_sql_ident(column))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut stmt = tx.prepare(&select_sql)?;
    let mut values = match stmt.query_row(rusqlite::params![source_session_id], |row| {
        let mut values = Vec::with_capacity(columns.len());
        for idx in 0..columns.len() {
            values.push(row.get::<_, SqlValue>(idx)?);
        }
        Ok(values)
    }) {
        Ok(values) => values,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err(ConvertError::Other(format!(
                "source Codex state row not found for {}; native clone cannot rebuild it",
                source_session_id
            )));
        }
        Err(error) => return Err(error.into()),
    };
    drop(stmt);

    for (column, value) in columns.iter().zip(values.iter_mut()) {
        match column.as_str() {
            "id" => *value = SqlValue::Text(new_session_id.to_string()),
            "rollout_path" => *value = SqlValue::Text(rollout_path.display().to_string()),
            "cwd" => *value = SqlValue::Text(cwd.to_string()),
            _ => {}
        }
    }

    if overwrite {
        tx.execute(
            "DELETE FROM threads WHERE id = ?1",
            rusqlite::params![new_session_id],
        )?;
    }
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
    Ok(true)
}

#[cfg(not(feature = "opencode"))]
fn copy_codex_state_thread_row(
    _codex_home: &Path,
    _source_session_id: &str,
    _new_session_id: &str,
    _rollout_path: &Path,
    _cwd: &str,
    _overwrite: bool,
) -> Result<bool> {
    Ok(false)
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
                    "clone_validation_failed_cleanup_skipped",
                    serde_json::json!({
                        "target_provider": target_provider.as_str(),
                        "session_id": session_id,
                        "artifact": format!("{:?}", artifact),
                        "reason": "overwrite_enabled",
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
    };
    super::remove::remove(&info)
}

fn mint_id_for(target: Provider) -> String {
    match target {
        Provider::Claude | Provider::Codex => uuid::Uuid::now_v7().to_string(),
        Provider::OpenCode => crate::ids::opencode_session_id(),
    }
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
    fn rejects_cross_provider_clone_before_reading_source() {
        let missing = PathBuf::from("/tmp/cokacmux-missing-source.jsonl");
        let src = session_info(Provider::Claude, "source-id", "/repo", missing);

        let error = clone_to_live(
            &src,
            &CloneOpts {
                to: Some(Provider::Codex),
                ..Default::default()
            },
        )
        .expect_err("cross-provider clone must be rejected");

        assert!(
            error.to_string().contains("cross-provider clone"),
            "{error}"
        );
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
                            {"type": "text", "text": "valid"}
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
        let source_sidecar = source_path.with_extension("");
        fs::create_dir_all(source_sidecar.join("tool-results")).unwrap();
        fs::write(source_sidecar.join("tool-results").join("a.txt"), "sidecar").unwrap();
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
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(values[2]["leafUuid"].as_str(), values[1]["uuid"].as_str());
        assert!(path
            .with_extension("")
            .join("tool-results")
            .join("a.txt")
            .is_file());
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
