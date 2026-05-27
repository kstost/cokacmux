//! Native artifact validation for cloned sessions.
//!
//! These checks are deliberately conservative: they verify the target agent's
//! storage invariants that we know are required for list/resume, without
//! pretending to be a full reimplementation of each agent.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ConvertError, Result};
use crate::providers::discovery::SessionInfo;
use crate::session::clone::ArtifactPath;
use crate::universal::Provider;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeValidationReport {
    pub provider: Provider,
    pub session_id: String,
    pub artifact: String,
    pub ok: bool,
    pub checks: Vec<NativeValidationCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeValidationCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl NativeValidationReport {
    fn new(provider: Provider, session_id: impl Into<String>, artifact: impl Into<String>) -> Self {
        Self {
            provider,
            session_id: session_id.into(),
            artifact: artifact.into(),
            ok: true,
            checks: Vec::new(),
        }
    }

    fn check(&mut self, name: impl Into<String>, ok: bool, detail: impl Into<String>) {
        if !ok {
            self.ok = false;
        }
        self.checks.push(NativeValidationCheck {
            name: name.into(),
            ok,
            detail: detail.into(),
        });
    }

    pub fn failure_summary(&self) -> String {
        self.checks
            .iter()
            .filter(|check| !check.ok)
            .map(|check| format!("{}: {}", check.name, check.detail))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

pub fn validate_info(info: &SessionInfo) -> Result<NativeValidationReport> {
    match info.provider {
        Provider::Claude => validate_claude_file(&info.source, &info.session_id),
        Provider::Codex => validate_codex_rollout(&info.source, &info.session_id),
        Provider::OpenCode => validate_opencode_db(&info.source, &info.session_id),
    }
}

pub fn validate_clone_artifact(
    provider: Provider,
    session_id: &str,
    artifact: &ArtifactPath,
) -> Result<NativeValidationReport> {
    match (provider, artifact) {
        (Provider::Claude, ArtifactPath::File(path)) => validate_claude_file(path, session_id),
        (Provider::Codex, ArtifactPath::File(path)) => validate_codex_rollout(path, session_id),
        (
            Provider::OpenCode,
            ArtifactPath::OpenCodeDb {
                db_path,
                session_id,
            },
        ) => validate_opencode_db(db_path, session_id),
        (provider, artifact) => {
            let mut report =
                NativeValidationReport::new(provider, session_id, format!("{:?}", artifact));
            report.check(
                "artifact_kind",
                false,
                format!("provider/artifact mismatch: {:?}", artifact),
            );
            Ok(report)
        }
    }
}

pub fn ensure_clone_artifact_native(
    provider: Provider,
    session_id: &str,
    artifact: &ArtifactPath,
) -> Result<NativeValidationReport> {
    let report = validate_clone_artifact(provider, session_id, artifact)?;
    crate::debug::log(
        "native_validate_clone_report",
        serde_json::json!({
            "provider": provider.as_str(),
            "session_id": session_id,
            "artifact": format!("{:?}", artifact),
            "ok": report.ok,
            "checks": report.checks.len(),
            "failures": report.checks.iter().filter(|check| !check.ok).count(),
        }),
    );
    if report.ok {
        Ok(report)
    } else {
        Err(ConvertError::Other(format!(
            "{} clone artifact failed native validation: {}",
            provider.as_str(),
            report.failure_summary()
        )))
    }
}

fn validate_claude_file(path: &Path, session_id: &str) -> Result<NativeValidationReport> {
    let mut report =
        NativeValidationReport::new(Provider::Claude, session_id, path.display().to_string());
    report.check("file_exists", path.is_file(), path.display().to_string());
    report.check(
        "extension_jsonl",
        path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"),
        path.display().to_string(),
    );
    report.check(
        "file_stem_matches_session_id",
        path.file_stem().and_then(|stem| stem.to_str()) == Some(session_id),
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("")
            .to_string(),
    );
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            report.check("read_file", false, error.to_string());
            return Ok(report);
        }
    };
    validate_claude_jsonl_text(&mut report, &content, session_id);
    Ok(report)
}

fn validate_claude_jsonl_text(
    report: &mut NativeValidationReport,
    content: &str,
    session_id: &str,
) {
    let mut line_count = 0usize;
    let mut json_errors = 0usize;
    let mut session_id_mismatches = 0usize;
    let mut conversation_rows = 0usize;
    let mut missing_uuid_rows = 0usize;
    let mut invalid_uuid_rows = 0usize;
    let mut unsafe_content_blocks = 0usize;

    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        line_count = line_count.saturating_add(1);
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                json_errors = json_errors.saturating_add(1);
                continue;
            }
        };
        let Some(object) = value.as_object() else {
            json_errors = json_errors.saturating_add(1);
            continue;
        };
        if let Some(actual) = object.get("sessionId").and_then(Value::as_str) {
            if actual != session_id {
                session_id_mismatches = session_id_mismatches.saturating_add(1);
            }
        }
        let line_type = object.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(line_type, "user" | "assistant" | "message") {
            conversation_rows = conversation_rows.saturating_add(1);
            match object.get("uuid").and_then(Value::as_str) {
                Some(uuid) if uuid::Uuid::parse_str(uuid).is_ok() => {}
                Some(_) => invalid_uuid_rows = invalid_uuid_rows.saturating_add(1),
                None => missing_uuid_rows = missing_uuid_rows.saturating_add(1),
            }
        }
        if let Some(content) = object
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        {
            for block in content {
                if let Some(kind) = block.get("type").and_then(Value::as_str) {
                    if !claude_api_content_type_is_known_safe(kind) {
                        unsafe_content_blocks = unsafe_content_blocks.saturating_add(1);
                    }
                }
            }
        }
    }

    report.check(
        "non_empty_jsonl",
        line_count > 0,
        format!("lines={line_count}"),
    );
    report.check(
        "jsonl_parse",
        json_errors == 0,
        format!("errors={json_errors}"),
    );
    report.check(
        "session_id_consistency",
        session_id_mismatches == 0,
        format!("mismatches={session_id_mismatches}"),
    );
    report.check(
        "conversation_rows_present",
        conversation_rows > 0,
        format!("conversation_rows={conversation_rows}"),
    );
    report.check(
        "conversation_uuid_present",
        missing_uuid_rows == 0,
        format!("missing={missing_uuid_rows}"),
    );
    report.check(
        "conversation_uuid_valid",
        invalid_uuid_rows == 0,
        format!("invalid={invalid_uuid_rows}"),
    );
    report.check(
        "claude_content_block_types_safe",
        unsafe_content_blocks == 0,
        format!("unsafe_blocks={unsafe_content_blocks}"),
    );
}

fn claude_api_content_type_is_known_safe(kind: &str) -> bool {
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

fn validate_codex_rollout(path: &Path, session_id: &str) -> Result<NativeValidationReport> {
    let mut report =
        NativeValidationReport::new(Provider::Codex, session_id, path.display().to_string());
    report.check("file_exists", path.is_file(), path.display().to_string());
    report.check(
        "extension_jsonl",
        path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"),
        path.display().to_string(),
    );
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            report.check("read_file", false, error.to_string());
            return Ok(report);
        }
    };
    validate_codex_jsonl_text(&mut report, &content, session_id);
    validate_codex_state_index(&mut report, path, session_id);
    Ok(report)
}

fn validate_codex_jsonl_text(report: &mut NativeValidationReport, content: &str, session_id: &str) {
    let mut line_count = 0usize;
    let mut json_errors = 0usize;
    let mut missing_type = 0usize;
    let mut session_meta_count = 0usize;
    let mut session_meta_id_ok = false;
    let mut session_meta_cwd_present = false;
    let mut event_rows = 0usize;

    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        line_count = line_count.saturating_add(1);
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                json_errors = json_errors.saturating_add(1);
                continue;
            }
        };
        let line_type = value.get("type").and_then(Value::as_str);
        if line_type.is_none() {
            missing_type = missing_type.saturating_add(1);
        }
        if line_type == Some("session_meta") {
            session_meta_count = session_meta_count.saturating_add(1);
            let payload = value.get("payload").unwrap_or(&Value::Null);
            session_meta_id_ok |= payload.get("id").and_then(Value::as_str) == Some(session_id);
            session_meta_cwd_present |= payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(|cwd| !cwd.is_empty())
                .unwrap_or(false);
        } else if matches!(
            line_type,
            Some("event_msg" | "response_item" | "turn_context")
        ) {
            event_rows = event_rows.saturating_add(1);
        }
    }

    report.check(
        "non_empty_jsonl",
        line_count > 0,
        format!("lines={line_count}"),
    );
    report.check(
        "jsonl_parse",
        json_errors == 0,
        format!("errors={json_errors}"),
    );
    report.check(
        "line_type_present",
        missing_type == 0,
        format!("missing={missing_type}"),
    );
    report.check(
        "session_meta_present",
        session_meta_count > 0,
        format!("session_meta={session_meta_count}"),
    );
    report.check(
        "session_meta_id_matches",
        session_meta_id_ok,
        format!("session_id={session_id}"),
    );
    report.check(
        "session_meta_cwd_present",
        session_meta_cwd_present,
        "payload.cwd".to_string(),
    );
    report.check(
        "event_rows_present",
        event_rows > 0,
        format!("event_rows={event_rows}"),
    );
}

fn validate_codex_state_index(
    report: &mut NativeValidationReport,
    rollout_path: &Path,
    session_id: &str,
) {
    let Some(codex_home) = infer_codex_home_from_rollout(rollout_path) else {
        report.check(
            "state_5_home_inferred",
            false,
            "rollout path is not under sessions/YYYY/MM/DD".to_string(),
        );
        return;
    };
    let state_5 = codex_home.join("state_5.sqlite");
    if !state_5.is_file() {
        report.check(
            "state_5_index_present",
            true,
            format!("skipped: {} not found", state_5.display()),
        );
        return;
    }
    #[cfg(feature = "opencode")]
    {
        match rusqlite::Connection::open(&state_5).and_then(|conn| {
            conn.query_row(
                "SELECT rollout_path FROM threads WHERE id = ?1",
                rusqlite::params![session_id],
                |row| row.get::<_, String>(0),
            )
        }) {
            Ok(indexed_path) => {
                let rollout_display = rollout_path.display().to_string();
                report.check(
                    "state_5_thread_row",
                    true,
                    format!("rollout_path={indexed_path}"),
                );
                report.check(
                    "state_5_rollout_path_matches",
                    indexed_path == rollout_display,
                    format!("indexed={indexed_path} actual={rollout_display}"),
                );
            }
            Err(error) => report.check("state_5_thread_row", false, error.to_string()),
        }
    }
    #[cfg(not(feature = "opencode"))]
    {
        report.check(
            "state_5_thread_row",
            true,
            "skipped: opencode feature disabled".to_string(),
        );
    }
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

fn validate_opencode_db(db_path: &Path, session_id: &str) -> Result<NativeValidationReport> {
    let mut report = NativeValidationReport::new(
        Provider::OpenCode,
        session_id,
        format!("{}#{}", db_path.display(), session_id),
    );
    report.check(
        "db_exists",
        db_path.is_file(),
        db_path.display().to_string(),
    );
    if !db_path.is_file() {
        return Ok(report);
    }
    #[cfg(feature = "opencode")]
    {
        let conn = match rusqlite::Connection::open(db_path) {
            Ok(conn) => conn,
            Err(error) => {
                report.check("open_db", false, error.to_string());
                return Ok(report);
            }
        };
        validate_opencode_db_connection(&mut report, &conn, session_id);
    }
    #[cfg(not(feature = "opencode"))]
    {
        report.check("open_db", false, "opencode feature disabled".to_string());
    }
    Ok(report)
}

#[cfg(feature = "opencode")]
fn validate_opencode_db_connection(
    report: &mut NativeValidationReport,
    conn: &rusqlite::Connection,
    session_id: &str,
) {
    let session_rows = count_query(
        conn,
        "SELECT COUNT(*) FROM session WHERE id = ?1",
        session_id,
    );
    report.check(
        "session_row_present",
        session_rows == Some(1),
        format!("rows={session_rows:?}"),
    );
    let metadata_ok = conn
        .query_row(
            "SELECT directory, slug, version, path FROM session WHERE id = ?1",
            rusqlite::params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map(|(directory, slug, version, path)| {
            !directory.is_empty() && !slug.is_empty() && !version.is_empty() && !path.is_empty()
        })
        .unwrap_or(false);
    report.check(
        "session_metadata_non_empty",
        metadata_ok,
        "directory/slug/version/path".to_string(),
    );
    let message_rows = count_query(
        conn,
        "SELECT COUNT(*) FROM message WHERE session_id = ?1",
        session_id,
    );
    report.check(
        "message_rows_present",
        message_rows.unwrap_or(0) > 0,
        format!("rows={message_rows:?}"),
    );
    report.check(
        "session_id_shape_native",
        is_opencode_native_id(session_id, "ses_"),
        session_id.to_string(),
    );
    let non_native_message_ids = count_non_native_ids(
        conn,
        "SELECT id FROM message WHERE session_id = ?1",
        session_id,
        "msg_",
    );
    report.check(
        "message_id_shape_native",
        non_native_message_ids == Some(0),
        format!("non_native={non_native_message_ids:?}"),
    );
    let non_native_part_ids = count_non_native_ids(
        conn,
        "SELECT id FROM part WHERE session_id = ?1",
        session_id,
        "prt_",
    );
    report.check(
        "part_id_shape_native",
        non_native_part_ids == Some(0),
        format!("non_native={non_native_part_ids:?}"),
    );
    let orphan_parts = count_query(
        conn,
        "SELECT COUNT(*)
         FROM part p
         LEFT JOIN message m ON m.id = p.message_id
         WHERE p.session_id = ?1 AND m.id IS NULL",
        session_id,
    );
    report.check(
        "part_rows_have_messages",
        orphan_parts == Some(0),
        format!("orphans={orphan_parts:?}"),
    );
    if table_exists(conn, "session_message").unwrap_or(false) {
        let session_message_rows = count_query(
            conn,
            "SELECT COUNT(*) FROM session_message WHERE session_id = ?1",
            session_id,
        );
        report.check(
            "session_message_table_readable",
            session_message_rows.is_some(),
            format!("rows={session_message_rows:?}"),
        );
        let non_native_event_ids = count_non_native_ids(
            conn,
            "SELECT id FROM session_message WHERE session_id = ?1",
            session_id,
            "evt_",
        );
        report.check(
            "session_message_id_shape_native",
            non_native_event_ids == Some(0),
            format!("non_native={non_native_event_ids:?}"),
        );
    }
}

#[cfg(feature = "opencode")]
fn count_query(conn: &rusqlite::Connection, sql: &str, session_id: &str) -> Option<i64> {
    conn.query_row(sql, rusqlite::params![session_id], |row| row.get(0))
        .ok()
}

#[cfg(feature = "opencode")]
fn count_non_native_ids(
    conn: &rusqlite::Connection,
    sql: &str,
    session_id: &str,
    prefix: &str,
) -> Option<i64> {
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map(rusqlite::params![session_id], |row| row.get::<_, String>(0))
        .ok()?;
    let mut count = 0i64;
    for row in rows {
        let id = row.ok()?;
        if !is_opencode_native_id(&id, prefix) {
            count = count.saturating_add(1);
        }
    }
    Some(count)
}

fn is_opencode_native_id(id: &str, prefix: &str) -> bool {
    let Some(body) = id.strip_prefix(prefix) else {
        return false;
    };
    if body.len() != 26 {
        return false;
    }
    let (time_hex, random) = body.split_at(12);
    time_hex.chars().all(|c| c.is_ascii_hexdigit())
        && random
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase() || c.is_ascii_lowercase())
}

#[cfg(feature = "opencode")]
fn table_exists(conn: &rusqlite::Connection, table: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        rusqlite::params![table],
        |row| row.get(0),
    )?;
    Ok(exists > 0)
}
