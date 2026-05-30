//! UniversalSession → opencode.db rows. Phase 2 finishes synthesis.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::debug;
use crate::error::Result;
use crate::universal::{ImageSource, Role, UMessage, UniversalSession};

use super::db;

#[derive(Debug, Clone, Copy)]
pub struct WriteOpts {
    pub overwrite: bool,
}

impl Default for WriteOpts {
    fn default() -> Self {
        Self { overwrite: true }
    }
}

pub fn to_db_path(session: &UniversalSession, db_path: &Path) -> Result<()> {
    to_db_path_with_opts(session, db_path, &WriteOpts::default())
}

pub fn to_db_path_with_opts(
    session: &UniversalSession,
    db_path: &Path,
    opts: &WriteOpts,
) -> Result<()> {
    debug::log(
        "provider_opencode_write_db_start",
        serde_json::json!({
            "db_path": db_path.display().to_string(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "overwrite": opts.overwrite,
        }),
    );
    let mut conn = match db::open_readwrite(db_path) {
        Ok(conn) => conn,
        Err(error) => {
            debug::log(
                "provider_opencode_write_db_error",
                serde_json::json!({
                    "db_path": db_path.display().to_string(),
                    "session_id": &session.session_id,
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };
    if let Err(error) = db::ensure_schema(&conn) {
        debug::log(
            "provider_opencode_write_db_error",
            serde_json::json!({
                "db_path": db_path.display().to_string(),
                "session_id": &session.session_id,
                "stage": "ensure_schema",
                "error": error.to_string(),
            }),
        );
        return Err(error);
    }
    let result = to_db_connection_with_opts(&mut conn, session, opts);
    match &result {
        Ok(()) => debug::log(
            "provider_opencode_write_db_ok",
            serde_json::json!({
                "db_path": db_path.display().to_string(),
                "session_id": &session.session_id,
            }),
        ),
        Err(error) => debug::log(
            "provider_opencode_write_db_error",
            serde_json::json!({
                "db_path": db_path.display().to_string(),
                "session_id": &session.session_id,
                "error": error.to_string(),
            }),
        ),
    }
    result
}

pub fn to_db_connection(conn: &mut rusqlite::Connection, session: &UniversalSession) -> Result<()> {
    to_db_connection_with_opts(conn, session, &WriteOpts::default())
}

pub fn to_db_connection_with_opts(
    conn: &mut rusqlite::Connection,
    session: &UniversalSession,
    opts: &WriteOpts,
) -> Result<()> {
    debug::log(
        "provider_opencode_write_connection_start",
        serde_json::json!({
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "overwrite": opts.overwrite,
        }),
    );
    let tx = conn.transaction()?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    if !opts.overwrite {
        let existing: i64 = tx.query_row(
            "SELECT COUNT(*) FROM session WHERE id = ?1",
            rusqlite::params![session.session_id],
            |row| row.get(0),
        )?;
        if existing > 0 {
            return Err(crate::error::ConvertError::Other(format!(
                "opencode session already exists: {} (set overwrite=true to replace)",
                session.session_id
            )));
        }
    }

    // OpenCode puts CLI-driven sessions under the special `global` project.
    // (Verified against a live opencode.db v1.15.5: every session listed by
    // `opencode session list` has project_id='global'.) Sessions tied to a
    // tracked worktree-project use a separate project_id but those rows are
    // managed by opencode's own startup; we don't try to replicate them.
    let project_id = "global";
    tx.execute(
        "INSERT OR IGNORE INTO project (id, worktree, time_created, time_updated, sandboxes)
         VALUES (?1, '/', ?2, ?2, '{}')",
        rusqlite::params![project_id, now_ms],
    )?;

    let time_created = session
        .created_at
        .map(|t| t.timestamp_millis())
        .unwrap_or(now_ms);
    let time_updated = session
        .updated_at
        .map(|t| t.timestamp_millis())
        .unwrap_or(time_created);
    // OpenCode's `session.model` column is stored as a JSON-stringified
    // object: `{"id": "...", "providerID": "...", "variant": "..."}`.
    // `opencode session list` expects these fields to be strings, so fill
    // conservative defaults when converting from providers that do not carry
    // all OpenCode-specific metadata.
    let session_model = opencode_model_ref(session.model.as_ref());
    let model_str = session_model_json(&session_model).to_string();
    let title = session.title.clone().unwrap_or_default();
    let usage = session.usage_total.clone().unwrap_or_default();
    let session_path = extra_string(session, "opencode_path")
        .unwrap_or_else(|| opencode_session_path(&session.cwd));
    let parent_id = extra_string(session, "opencode_parent_id");
    let share_url = extra_string(session, "opencode_share_url");
    let summary_additions = extra_i64(session, "opencode_summary_additions");
    let summary_deletions = extra_i64(session, "opencode_summary_deletions");
    let summary_files = extra_i64(session, "opencode_summary_files");
    let summary_diffs = extra_string(session, "opencode_summary_diffs");
    let revert = extra_string(session, "opencode_revert");
    let permission = extra_string(session, "opencode_permission");
    let time_compacting = extra_i64(session, "opencode_time_compacting");
    let time_archived = extra_i64(session, "opencode_time_archived");
    let workspace_id = extra_string(session, "opencode_workspace_id");

    // OpenCode session list expects non-empty `slug` and a `version`
    // (it uses these when rendering the picker). We synthesize a short
    // hex slug from the session id and use this crate's version as the
    // `version` column.
    let slug: String = extra_string(session, "opencode_slug").unwrap_or_else(|| {
        session
            .session_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(8)
            .collect::<String>()
            .to_lowercase()
    });
    let version = extra_string(session, "opencode_version")
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    // The `agent` column ("build", "plan", …) is preserved from the read
    // side via `session.extras.opencode_agent`. ModelInfo.variant is for
    // the model's variant (e.g. "medium") and is written into the model
    // JSON column, NOT the agent column.
    let agent_str: String = session
        .extras
        .get("opencode_agent")
        .and_then(|v| v.as_str())
        .filter(|agent| !agent.trim().is_empty())
        .unwrap_or("build")
        .to_string();

    tx.execute(
        "INSERT OR REPLACE INTO session
            (id, project_id, parent_id, directory, title, agent, model, cost,
             tokens_input, tokens_output, tokens_reasoning,
             tokens_cache_read, tokens_cache_write,
             time_created, time_updated, slug, version, path,
             share_url, summary_additions, summary_deletions, summary_files, summary_diffs,
             revert, permission, time_compacting, time_archived, workspace_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
                 ?27, ?28)",
        rusqlite::params![
            session.session_id,
            project_id,
            parent_id,
            session.cwd,
            title,
            agent_str,
            model_str,
            usage.cost_usd.unwrap_or(0.0),
            usage.input_tokens.unwrap_or(0) as i64,
            usage.output_tokens.unwrap_or(0) as i64,
            usage.reasoning_output_tokens.unwrap_or(0) as i64,
            usage.cached_input_tokens.unwrap_or(0) as i64,
            0i64,
            time_created,
            time_updated,
            slug,
            version,
            session_path,
            share_url,
            summary_additions,
            summary_deletions,
            summary_files,
            summary_diffs,
            revert,
            permission,
            time_compacting,
            time_archived,
            workspace_id,
        ],
    )?;

    // Drop existing message+part rows for this session, then INSERT.
    tx.execute(
        "DELETE FROM part WHERE session_id = ?1",
        rusqlite::params![session.session_id],
    )?;
    tx.execute(
        "DELETE FROM message WHERE session_id = ?1",
        rusqlite::params![session.session_id],
    )?;
    tx.execute(
        "DELETE FROM session_message WHERE session_id = ?1",
        rusqlite::params![session.session_id],
    )?;

    let mut session_message_rows_inserted = 0usize;
    for m in &session.messages {
        if let Some(row) = opencode_session_message_write_row(m, &session.session_id, time_created)
        {
            insert_session_message_row(&tx, &session.session_id, &row)?;
            session_message_rows_inserted = session_message_rows_inserted.saturating_add(1);
        }
    }
    if session_message_rows_inserted == 0 {
        let agent_row = SessionMessageWriteRow {
            id: opencode_event_id(),
            type_tag: "agent-switched".into(),
            time_created,
            time_updated: time_created,
            data: serde_json::json!({
                "agent": agent_str.as_str(),
                "time": {"created": time_created},
            })
            .to_string(),
        };
        insert_session_message_row(&tx, &session.session_id, &agent_row)?;
        session_message_rows_inserted = session_message_rows_inserted.saturating_add(1);

        let model_row = SessionMessageWriteRow {
            id: opencode_event_id(),
            type_tag: "model-switched".into(),
            time_created,
            time_updated: time_created,
            data: serde_json::json!({
                "model": session_model_json(&session_model),
                "time": {"created": time_created},
            })
            .to_string(),
        };
        insert_session_message_row(&tx, &session.session_id, &model_row)?;
        session_message_rows_inserted = session_message_rows_inserted.saturating_add(1);
    }

    let write_messages = opencode_write_messages(session);
    let tool_results = collect_tool_results(&write_messages);
    let tool_use_call_ids = collect_tool_use_call_ids(&write_messages);
    let tool_result_call_ids: BTreeSet<String> = tool_results.keys().cloned().collect();
    let fused_tool_result_calls = tool_use_call_ids
        .intersection(&tool_result_call_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    debug::log(
        "provider_opencode_write_tools_collected",
        serde_json::json!({
            "session_id": &session.session_id,
            "write_messages": write_messages.len(),
            "tool_results": tool_results.len(),
            "tool_use_call_ids": tool_use_call_ids.len(),
            "fused_tool_result_calls": fused_tool_result_calls.len(),
        }),
    );

    let mut last_message_time = i64::MIN;
    let mut last_message_id: Option<String> = None;
    let mut message_rows_inserted = 0usize;
    let mut part_rows_inserted = 0usize;
    let mut fused_tool_result_parts_skipped = 0usize;
    for m in &write_messages {
        if should_skip_message_row(m) {
            continue;
        }
        let raw_created = m
            .timestamp
            .map(|t| t.timestamp_millis())
            .unwrap_or(time_created);
        let t_created = if raw_created <= last_message_time {
            last_message_time + 1
        } else {
            raw_created
        };
        last_message_time = t_created;
        let data = message_data_json(session, m, &agent_str, &last_message_id, t_created);
        tx.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![m.id, session.session_id, t_created, data.to_string()],
        )?;
        message_rows_inserted = message_rows_inserted.saturating_add(1);

        let mut part_seq = 0usize;
        if matches!(m.role, Role::Assistant) && !has_opencode_control_part(m, "step-start") {
            let payload = serde_json::json!({"type": "step-start"});
            insert_part_row(
                &tx,
                &session.session_id,
                &m.id,
                part_seq,
                t_created,
                payload,
            )?;
            part_seq = part_seq.saturating_add(1);
            part_rows_inserted = part_rows_inserted.saturating_add(1);

            let has_reasoning = m
                .content
                .iter()
                .any(|block| matches!(block, crate::universal::ContentBlock::Thinking { .. }));
            if !has_reasoning {
                let payload = serde_json::json!({
                    "type": "reasoning",
                    "text": "",
                    "time": {"start": t_created, "end": t_created},
                });
                insert_part_row(
                    &tx,
                    &session.session_id,
                    &m.id,
                    part_seq,
                    t_created,
                    payload,
                )?;
                part_seq = part_seq.saturating_add(1);
                part_rows_inserted = part_rows_inserted.saturating_add(1);
            }
        }

        // Each ContentBlock → one part row, with assistant step parts around it.
        for block in &m.content {
            if should_skip_fused_tool_result(block, &fused_tool_result_calls) {
                fused_tool_result_parts_skipped = fused_tool_result_parts_skipped.saturating_add(1);
                continue;
            }
            let part_time = t_created + part_seq as i64;
            let mut payload = block_to_part_data(block, &tool_results, part_time);
            if matches!(m.role, Role::Assistant) {
                decorate_assistant_part(&mut payload, part_time);
            }
            insert_part_row(
                &tx,
                &session.session_id,
                &m.id,
                part_seq,
                part_time,
                payload,
            )?;
            part_seq = part_seq.saturating_add(1);
            part_rows_inserted = part_rows_inserted.saturating_add(1);
        }
        if matches!(m.role, Role::Assistant) && !has_opencode_control_part(m, "step-finish") {
            let payload = step_finish_part_data(m);
            let part_time = t_created + part_seq as i64;
            insert_part_row(
                &tx,
                &session.session_id,
                &m.id,
                part_seq,
                part_time,
                payload,
            )?;
            part_rows_inserted = part_rows_inserted.saturating_add(1);
        }
        last_message_id = Some(m.id.clone());
    }

    tx.commit()?;
    debug::log(
        "provider_opencode_write_connection_ok",
        serde_json::json!({
            "session_id": &session.session_id,
            "session_message_rows": session_message_rows_inserted,
            "message_rows": message_rows_inserted,
            "part_rows": part_rows_inserted,
            "fused_tool_result_parts_skipped": fused_tool_result_parts_skipped,
        }),
    );
    Ok(())
}

struct SessionMessageWriteRow {
    id: String,
    type_tag: String,
    time_created: i64,
    time_updated: i64,
    data: String,
}

#[derive(Debug, Clone)]
struct OpenCodeModelRef {
    provider_id: String,
    model_id: String,
    variant: String,
}

#[derive(Debug, Clone)]
struct ToolResultInfo {
    output: serde_json::Value,
    is_error: bool,
}

fn opencode_model_ref(model: Option<&crate::universal::ModelInfo>) -> OpenCodeModelRef {
    OpenCodeModelRef {
        provider_id: model
            .and_then(|m| m.provider_id.as_deref())
            .filter(|provider| !provider.trim().is_empty())
            .unwrap_or("openai")
            .to_string(),
        model_id: model
            .map(|m| m.model_id.as_str())
            .filter(|id| !id.trim().is_empty())
            .unwrap_or("gpt-5.5")
            .to_string(),
        variant: model
            .and_then(|m| m.variant.as_deref())
            .filter(|variant| !variant.trim().is_empty())
            .unwrap_or("default")
            .to_string(),
    }
}

fn insert_session_message_row(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    row: &SessionMessageWriteRow,
) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO session_message
            (id, session_id, type, time_created, time_updated, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            row.id.as_str(),
            session_id,
            row.type_tag.as_str(),
            row.time_created,
            row.time_updated,
            row.data.as_str()
        ],
    )?;
    Ok(())
}

fn insert_part_row(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    message_id: &str,
    _part_seq: usize,
    part_time: i64,
    payload: serde_json::Value,
) -> Result<()> {
    let part_id = crate::ids::opencode_part_id();
    tx.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        rusqlite::params![
            part_id,
            message_id,
            session_id,
            part_time,
            payload.to_string()
        ],
    )?;
    Ok(())
}

fn opencode_event_id() -> String {
    crate::ids::opencode_event_id()
}

fn extra_string(session: &UniversalSession, key: &str) -> Option<String> {
    session
        .extras
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(String::from)
}

fn extra_i64(session: &UniversalSession, key: &str) -> Option<i64> {
    session.extras.get(key).and_then(|value| value.as_i64())
}

fn message_model_ref(
    session: &UniversalSession,
    message: &crate::universal::UMessage,
) -> OpenCodeModelRef {
    if message
        .model
        .as_ref()
        .map(|m| !m.model_id.trim().is_empty())
        .unwrap_or(false)
    {
        opencode_model_ref(message.model.as_ref())
    } else {
        opencode_model_ref(session.model.as_ref())
    }
}

fn session_model_json(model: &OpenCodeModelRef) -> serde_json::Value {
    serde_json::json!({
        "id": model.model_id.as_str(),
        "providerID": model.provider_id.as_str(),
        "variant": model.variant.as_str(),
    })
}

fn message_model_json(model: &OpenCodeModelRef) -> serde_json::Value {
    let mut value = serde_json::json!({
        "providerID": model.provider_id.as_str(),
        "modelID": model.model_id.as_str(),
    });
    if model.variant != "default" {
        value["variant"] = serde_json::Value::String(model.variant.clone());
    }
    value
}

fn opencode_write_messages(session: &UniversalSession) -> Vec<UMessage> {
    let mut out: Vec<UMessage> = Vec::new();
    for message in &session.messages {
        if should_skip_message_row(message) {
            continue;
        }
        let mut message = message.clone();
        if let Some(prev) = out.last_mut() {
            if should_merge_assistant_messages(prev, &message) {
                prev.content.append(&mut message.content);
                if prev.usage.is_none() {
                    prev.usage = message.usage;
                }
                if prev.stop_reason.is_none() {
                    prev.stop_reason = message.stop_reason;
                }
                if prev.model.is_none() {
                    prev.model = message.model;
                }
                continue;
            }
        }
        out.push(message);
    }
    out
}

fn should_merge_assistant_messages(prev: &UMessage, next: &UMessage) -> bool {
    if prev.role != Role::Assistant || next.role != Role::Assistant {
        return false;
    }
    if prev.flags.is_meta || next.flags.is_meta {
        return false;
    }
    let prev_reasoning_only = prev.content.iter().all(|block| {
        matches!(
            block,
            crate::universal::ContentBlock::Thinking { .. }
                | crate::universal::ContentBlock::Other { .. }
        )
    });
    let next_reasoning_only = next.content.iter().all(|block| {
        matches!(
            block,
            crate::universal::ContentBlock::Thinking { .. }
                | crate::universal::ContentBlock::Other { .. }
        )
    });
    prev_reasoning_only || next_reasoning_only
}

fn opencode_session_path(cwd: &str) -> String {
    cwd.trim_start_matches('/').to_string()
}

fn message_data_json(
    session: &UniversalSession,
    message: &crate::universal::UMessage,
    agent: &str,
    previous_message_id: &Option<String>,
    time_created: i64,
) -> serde_json::Value {
    use crate::universal::Role;

    let model = message_model_ref(session, message);
    match message.role {
        Role::User => serde_json::json!({
            "role": "user",
            "time": {"created": time_created},
            "agent": agent,
            "model": message_model_json(&model),
            "summary": {"diffs": []},
        }),
        Role::Assistant => {
            let usage = message.usage.as_ref();
            let mut data = serde_json::json!({
                "parentID": message
                    .parent_id
                    .as_deref()
                    .or(previous_message_id.as_deref())
                    .unwrap_or(&message.id),
                "role": "assistant",
                "time": {
                    "created": time_created,
                    "completed": time_created,
                },
                "mode": agent,
                "agent": agent,
                "path": {
                    "cwd": session.cwd.as_str(),
                    "root": "/",
                },
                "cost": cost_json(usage.and_then(|u| u.cost_usd).unwrap_or(0.0)),
                "tokens": usage_json(usage),
                "modelID": model.model_id.as_str(),
                "providerID": model.provider_id.as_str(),
                "finish": message
                    .stop_reason
                    .as_deref()
                    .filter(|finish| !finish.trim().is_empty())
                    .unwrap_or_else(|| infer_finish(message)),
            });
            if model.variant != "default" {
                data["variant"] = serde_json::Value::String(model.variant.clone());
            }
            data
        }
        Role::Tool => serde_json::json!({
            "role": "tool",
            "time": {"created": time_created},
            "model": message_model_json(&model),
        }),
        Role::Developer => serde_json::json!({
            "role": "developer",
            "time": {"created": time_created},
            "model": message_model_json(&model),
        }),
        Role::System => serde_json::json!({
            "role": "system",
            "time": {"created": time_created},
            "model": message_model_json(&model),
        }),
    }
}

fn usage_json(usage: Option<&crate::universal::Usage>) -> serde_json::Value {
    let input = usage.and_then(|u| u.input_tokens).unwrap_or(0);
    let output = usage.and_then(|u| u.output_tokens).unwrap_or(0);
    let reasoning = usage.and_then(|u| u.reasoning_output_tokens).unwrap_or(0);
    let cache_read = usage.and_then(|u| u.cached_input_tokens).unwrap_or(0);
    let cache_write = 0u64;
    let total = usage
        .and_then(|u| u.total_tokens)
        .unwrap_or(input + output + reasoning + cache_read + cache_write);
    serde_json::json!({
        "total": total,
        "input": input,
        "output": output,
        "reasoning": reasoning,
        "cache": {
            "read": cache_read,
            "write": cache_write,
        },
    })
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn cost_json(value: f64) -> serde_json::Value {
    let value = finite_or_zero(value);
    if value == 0.0 {
        serde_json::json!(0)
    } else {
        serde_json::json!(value)
    }
}

fn infer_finish(message: &crate::universal::UMessage) -> &'static str {
    if message
        .content
        .iter()
        .any(|block| matches!(block, crate::universal::ContentBlock::ToolUse { .. }))
    {
        "tool-calls"
    } else {
        "stop"
    }
}

fn collect_tool_use_call_ids(messages: &[UMessage]) -> BTreeSet<String> {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            crate::universal::ContentBlock::ToolUse { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}

fn collect_tool_results(messages: &[UMessage]) -> BTreeMap<String, ToolResultInfo> {
    let mut out = BTreeMap::new();
    for block in messages.iter().flat_map(|message| message.content.iter()) {
        if let crate::universal::ContentBlock::ToolResult {
            call_id,
            output,
            is_error,
            ..
        } = block
        {
            out.insert(
                call_id.clone(),
                ToolResultInfo {
                    output: output.clone(),
                    is_error: *is_error,
                },
            );
        }
    }
    out
}

fn opencode_session_message_write_row(
    message: &crate::universal::UMessage,
    session_id: &str,
    default_time: i64,
) -> Option<SessionMessageWriteRow> {
    let source_type = message
        .provenance
        .source_event_type
        .strip_prefix("opencode:session_message.")?;
    let raw_row = message.provenance.raw.get("session_message");
    let time_created = raw_row
        .and_then(|r| r.get("time_created"))
        .and_then(|v| v.as_i64())
        .or_else(|| message.timestamp.map(|t| t.timestamp_millis()))
        .unwrap_or(default_time);
    let time_updated = raw_row
        .and_then(|r| r.get("time_updated"))
        .and_then(|v| v.as_i64())
        .unwrap_or(time_created);
    // Treat the Universal message id as the canonical row id. This matters for
    // clone: raw OpenCode provenance still contains the source `evt_...` id,
    // and reusing it would `INSERT OR REPLACE` the original session_message
    // row because `session_message.id` is globally primary-keyed.
    let id = if message.id.starts_with("evt_") {
        message.id.clone()
    } else {
        opencode_event_id()
    };
    let type_tag = raw_row
        .and_then(|r| r.get("type"))
        .and_then(|v| v.as_str())
        .filter(|kind| !kind.is_empty())
        .unwrap_or(source_type)
        .to_string();
    let mut data = raw_row
        .and_then(|r| r.get("data"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"time": {"created": time_created}}));
    if let Some(obj) = data.as_object_mut() {
        obj.entry("time".to_string())
            .or_insert_with(|| serde_json::json!({"created": time_created}));
        obj.remove("id");
        obj.remove("type");
        obj.remove("sessionID");
        obj.remove("session_id");
    }
    let _ = session_id;

    Some(SessionMessageWriteRow {
        id,
        type_tag,
        time_created,
        time_updated,
        data: data.to_string(),
    })
}

fn should_skip_message_row(message: &crate::universal::UMessage) -> bool {
    if message
        .provenance
        .source_event_type
        .starts_with("opencode:session_message.")
    {
        return true;
    }
    if message.flags.is_meta {
        return true;
    }
    if matches!(
        message.role,
        crate::universal::Role::System | crate::universal::Role::Developer
    ) {
        return true;
    }
    is_foreign_runtime_context(message)
}

fn is_foreign_runtime_context(message: &crate::universal::UMessage) -> bool {
    if message.role != crate::universal::Role::User {
        return false;
    }
    let mut text = String::new();
    for block in &message.content {
        if let crate::universal::ContentBlock::Text { text: part, .. } = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(part);
        } else {
            return false;
        }
    }
    let trimmed = text.trim_start();
    trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<permissions instructions>")
        || trimmed.starts_with("<skills_instructions>")
}

fn step_finish_part_data(message: &crate::universal::UMessage) -> serde_json::Value {
    let usage = message.usage.as_ref();
    serde_json::json!({
        "reason": message
            .stop_reason
            .as_deref()
            .filter(|reason| !reason.trim().is_empty())
            .unwrap_or_else(|| infer_finish(message)),
        "type": "step-finish",
        "tokens": usage_json(usage),
        "cost": cost_json(usage.and_then(|u| u.cost_usd).unwrap_or(0.0)),
    })
}

fn should_skip_fused_tool_result(
    block: &crate::universal::ContentBlock,
    fused_tool_result_calls: &BTreeSet<String>,
) -> bool {
    match block {
        crate::universal::ContentBlock::ToolResult { call_id, .. } => {
            fused_tool_result_calls.contains(call_id)
        }
        _ => false,
    }
}

fn has_opencode_control_part(message: &crate::universal::UMessage, kind: &str) -> bool {
    message.content.iter().any(|block| match block {
        crate::universal::ContentBlock::Other { type_tag, payload } => {
            type_tag == kind
                || payload
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|payload_kind| payload_kind == kind)
                    .unwrap_or(false)
        }
        _ => false,
    })
}

fn block_to_part_data(
    block: &crate::universal::ContentBlock,
    tool_results: &BTreeMap<String, ToolResultInfo>,
    part_time: i64,
) -> serde_json::Value {
    use crate::universal::ContentBlock as B;
    match block {
        B::Text { text, extras } => opencode_textual_part_payload(extras, "text", text),
        B::Thinking {
            text,
            encrypted,
            extras,
            ..
        } => opencode_reasoning_part_payload(extras, text, encrypted.as_deref()),
        B::ToolUse {
            call_id,
            name,
            input,
            ..
        } => tool_part_payload(name, call_id, input, tool_results.get(call_id), part_time),
        B::ToolResult {
            call_id,
            output,
            is_error,
            ..
        } => tool_part_payload(
            "tool",
            call_id,
            &serde_json::json!({}),
            Some(&ToolResultInfo {
                output: output.clone(),
                is_error: *is_error,
            }),
            part_time,
        ),
        B::Image {
            source,
            mime,
            extras,
            ..
        } => opencode_file_part_payload(extras, mime, source),
        B::Attachment {
            name, path, mime, ..
        } => serde_json::json!({
            "type": "file",
            "filename": name,
            "url": path,
            "mime": mime,
        }),
        B::Patch { unified_diff, .. } => serde_json::json!({"type": "patch", "diff": unified_diff}),
        B::Other { type_tag, payload } => {
            if payload
                .get("type")
                .and_then(|v| v.as_str())
                .map(|t| t == type_tag)
                .unwrap_or(false)
            {
                payload.clone()
            } else {
                serde_json::json!({"type": type_tag, "payload": payload})
            }
        }
    }
}

fn opencode_textual_part_payload(
    extras: &std::collections::BTreeMap<String, serde_json::Value>,
    expected_type: &str,
    text: &str,
) -> serde_json::Value {
    if let Some(mut payload) = extras
        .get("opencode_part_data")
        .and_then(|value| value.as_object())
        .cloned()
    {
        if payload.get("type").and_then(|value| value.as_str()) == Some(expected_type) {
            payload.insert("text".into(), serde_json::Value::String(text.to_string()));
            let mut value = serde_json::Value::Object(payload);
            if expected_type == "text" {
                opencode_apply_codex_phase(&mut value, extras);
                opencode_apply_codex_item_id(&mut value, extras, "msg_");
            }
            return value;
        }
    }
    let mut value = serde_json::json!({"type": expected_type, "text": text});
    if expected_type == "text" {
        opencode_apply_codex_phase(&mut value, extras);
        opencode_apply_codex_item_id(&mut value, extras, "msg_");
    }
    value
}

fn opencode_apply_codex_phase(
    payload: &mut serde_json::Value,
    extras: &std::collections::BTreeMap<String, serde_json::Value>,
) {
    let Some(phase) = extras
        .get("codex_phase")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };
    if payload
        .get("metadata")
        .and_then(|metadata| metadata.get("openai"))
        .and_then(|openai| openai.get("phase"))
        .is_some()
    {
        return;
    }
    if let Some(object) = payload.as_object_mut() {
        let metadata = object
            .entry("metadata")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(metadata_object) = metadata.as_object_mut() {
            let openai = metadata_object
                .entry("openai")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(openai_object) = openai.as_object_mut() {
                openai_object.insert("phase".into(), serde_json::Value::String(phase.to_string()));
            }
        }
    }
}

fn opencode_apply_codex_item_id(
    payload: &mut serde_json::Value,
    extras: &std::collections::BTreeMap<String, serde_json::Value>,
    expected_prefix: &str,
) {
    let Some(item_id) = extras
        .get("codex_item_id")
        .and_then(|value| value.as_str())
        .filter(|value| value.starts_with(expected_prefix))
    else {
        return;
    };
    if payload
        .get("metadata")
        .and_then(|metadata| metadata.get("openai"))
        .and_then(|openai| openai.get("itemId"))
        .is_some()
    {
        return;
    }
    if let Some(object) = payload.as_object_mut() {
        let metadata = object
            .entry("metadata")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(metadata_object) = metadata.as_object_mut() {
            let openai = metadata_object
                .entry("openai")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(openai_object) = openai.as_object_mut() {
                openai_object.insert(
                    "itemId".into(),
                    serde_json::Value::String(item_id.to_string()),
                );
            }
        }
    }
}

fn opencode_reasoning_part_payload(
    extras: &std::collections::BTreeMap<String, serde_json::Value>,
    text: &str,
    encrypted: Option<&str>,
) -> serde_json::Value {
    let mut payload = opencode_textual_part_payload(extras, "reasoning", text);
    opencode_apply_codex_item_id(&mut payload, extras, "rs_");
    if payload
        .get("metadata")
        .and_then(|metadata| metadata.get("openai"))
        .and_then(|openai| openai.get("reasoningEncryptedContent"))
        .is_some()
    {
        return payload;
    }
    let Some(encrypted) = encrypted.filter(|value| !value.trim().is_empty()) else {
        return payload;
    };
    if let Some(object) = payload.as_object_mut() {
        let metadata = object
            .entry("metadata")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(metadata_object) = metadata.as_object_mut() {
            let openai = metadata_object
                .entry("openai")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(openai_object) = openai.as_object_mut() {
                openai_object.insert(
                    "reasoningEncryptedContent".into(),
                    serde_json::Value::String(encrypted.to_string()),
                );
            }
        }
    }
    payload
}

fn opencode_file_part_payload(
    extras: &std::collections::BTreeMap<String, serde_json::Value>,
    mime: &str,
    source: &ImageSource,
) -> serde_json::Value {
    if let Some(mut payload) = extras
        .get("opencode_part_data")
        .and_then(|value| value.as_object())
        .cloned()
    {
        if payload.get("type").and_then(|value| value.as_str()) == Some("file") {
            payload.insert("mime".into(), serde_json::Value::String(mime.to_string()));
            payload
                .entry("url")
                .or_insert_with(|| serde_json::Value::String(opencode_image_url(mime, source)));
            return serde_json::Value::Object(payload);
        }
    }
    serde_json::json!({
        "type": "file",
        "mime": mime,
        "url": opencode_image_url(mime, source),
    })
}

fn opencode_image_url(mime: &str, source: &ImageSource) -> String {
    match source {
        ImageSource::Base64 { data } => format!("data:{};base64,{}", mime, data),
        ImageSource::Url { url } => url.clone(),
        ImageSource::LocalPath { path } if path.starts_with("file://") => path.clone(),
        ImageSource::LocalPath { path } if path.starts_with('/') => format!("file://{}", path),
        ImageSource::LocalPath { path } => path.clone(),
    }
}

fn decorate_assistant_part(payload: &mut serde_json::Value, part_time: i64) {
    let Some(kind) = payload.get("type").and_then(|v| v.as_str()) else {
        return;
    };
    if !matches!(kind, "text" | "reasoning") {
        return;
    }
    let Some(obj) = payload.as_object_mut() else {
        return;
    };
    obj.entry("time").or_insert_with(|| {
        serde_json::json!({
            "start": part_time,
            "end": part_time,
        })
    });
}

fn tool_part_payload(
    name: &str,
    call_id: &str,
    input: &serde_json::Value,
    result: Option<&ToolResultInfo>,
    part_time: i64,
) -> serde_json::Value {
    let tool_name = if name.trim().is_empty() { "tool" } else { name };
    let state = match result {
        Some(result) if result.is_error => serde_json::json!({
            "status": "error",
            "input": input,
            "error": tool_output_string(&result.output),
            "metadata": {},
            "time": {
                "start": part_time,
                "end": part_time,
            },
        }),
        Some(result) => serde_json::json!({
            "status": "completed",
            "input": input,
            "output": tool_output_string(&result.output),
            "title": tool_name,
            "metadata": {},
            "time": {
                "start": part_time,
                "end": part_time,
            },
        }),
        None => serde_json::json!({
            "status": "pending",
            "input": input,
            "raw": serde_json::to_string(input).unwrap_or_default(),
        }),
    };
    serde_json::json!({
        "type": "tool",
        "tool": tool_name,
        "callID": call_id,
        "state": state,
    })
}

fn tool_output_string(output: &serde_json::Value) -> String {
    output
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| output.to_string())
}
