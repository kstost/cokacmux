//! UniversalSession → Claude JSONL.
//!
//! Two-track strategy:
//!  - If every message came from Claude, re-emit `provenance.raw` verbatim
//!    (claude→universal→claude is lossless).
//!  - Otherwise, synthesize a Claude Code-style transcript. Claude Code is
//!    stricter than our parser: line UUIDs must look like UUIDs, transcript
//!    records need the usual CLI fields, and provider meta/system records
//!    should not be emitted as visible conversation rows.

use std::io::Write;
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::debug;
use crate::error::Result;
use crate::time::to_rfc3339_ms;
use crate::universal::{ContentBlock, Role, UMessage, UniversalSession};

use super::ClaudeWriteOpts;

pub fn to_jsonl_path(
    session: &UniversalSession,
    path: &Path,
    opts: &ClaudeWriteOpts,
) -> Result<()> {
    debug::log(
        "provider_claude_write_file_start",
        serde_json::json!({
            "path": path.display().to_string(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "sidecar_threshold_bytes": opts.sidecar_threshold_bytes,
        }),
    );
    let s = to_jsonl_string(session, opts)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(s.as_bytes())?;
    debug::log(
        "provider_claude_write_file_ok",
        serde_json::json!({
            "path": path.display().to_string(),
            "session_id": &session.session_id,
            "bytes": s.len(),
            "lines": s.lines().count(),
        }),
    );
    Ok(())
}

const SYNTHETIC_CLAUDE_VERSION: &str = "2.1.147";

pub fn to_jsonl_string(session: &UniversalSession, _opts: &ClaudeWriteOpts) -> Result<String> {
    let replay_raw = should_replay_claude_raw(session);
    debug::log(
        "provider_claude_write_string_start",
        serde_json::json!({
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "strategy": if replay_raw { "replay_raw" } else { "synthesize" },
        }),
    );
    let result = if replay_raw {
        replay_claude_raw(session)
    } else {
        synthesize_session(session)
    };
    match &result {
        Ok(output) => debug::log(
            "provider_claude_write_string_ok",
            serde_json::json!({
                "session_id": &session.session_id,
                "bytes": output.len(),
                "lines": output.lines().count(),
                "strategy": if replay_raw { "replay_raw" } else { "synthesize" },
            }),
        ),
        Err(error) => debug::log(
            "provider_claude_write_string_error",
            serde_json::json!({
                "session_id": &session.session_id,
                "strategy": if replay_raw { "replay_raw" } else { "synthesize" },
                "error": error.to_string(),
            }),
        ),
    }
    result
}

fn should_replay_claude_raw(session: &UniversalSession) -> bool {
    !session.messages.is_empty()
        && session
            .messages
            .iter()
            .all(|m| m.provenance.source_event_type.starts_with("claude:"))
}

fn replay_claude_raw(session: &UniversalSession) -> Result<String> {
    let mut out = String::new();
    for m in &session.messages {
        let line = serde_json::to_string(&m.provenance.raw)?;
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

fn synthesize_session(session: &UniversalSession) -> Result<String> {
    let prepared = prepare_conversation(session);
    let mut values = Vec::new();

    let title = synthesize_title(session);
    let first_prompt = prepared
        .iter()
        .find(|m| m.role == Role::User)
        .and_then(prepared_text)
        .unwrap_or_default();
    let first_ts = prepared
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| m.timestamp.clone())
        .or_else(|| session.created_at.or(session.updated_at).map(to_rfc3339_ms))
        .unwrap_or_else(|| to_rfc3339_ms(chrono::Utc::now()));

    if let Some(title) = &title {
        values.push(json!({
            "type": "custom-title",
            "customTitle": title,
            "sessionId": session.session_id,
        }));
        values.push(json!({
            "type": "agent-name",
            "agentName": title,
            "sessionId": session.session_id,
        }));
    }

    if !first_prompt.is_empty() {
        values.push(json!({
            "type": "queue-operation",
            "operation": "enqueue",
            "timestamp": first_ts.clone(),
            "sessionId": session.session_id,
            "content": first_prompt.clone(),
        }));
        values.push(json!({
            "type": "queue-operation",
            "operation": "dequeue",
            "timestamp": first_ts.clone(),
            "sessionId": session.session_id,
        }));
    }

    let mut parent_uuid: Option<String> = None;
    let mut leaf_uuid: Option<String> = None;
    for prepared in prepared {
        let top = synthesize_conversation_line(session, &prepared, parent_uuid.as_deref());
        parent_uuid = Some(prepared.uuid);
        leaf_uuid = parent_uuid.clone();
        values.push(top);
    }

    if !first_prompt.is_empty() {
        let leaf = leaf_uuid
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        values.push(json!({
            "type": "last-prompt",
            "lastPrompt": first_prompt.clone(),
            "leafUuid": leaf,
            "sessionId": session.session_id,
        }));
    }

    if let Some(title) = &title {
        values.push(json!({
            "type": "custom-title",
            "customTitle": title,
            "sessionId": session.session_id,
        }));
        values.push(json!({
            "type": "agent-name",
            "agentName": title,
            "sessionId": session.session_id,
        }));
    }

    let mut out = String::new();
    for v in values {
        out.push_str(&serde_json::to_string(&v)?);
        out.push('\n');
    }
    Ok(out)
}

struct PreparedMessage<'a> {
    msg: &'a UMessage,
    role: Role,
    uuid: String,
    timestamp: String,
    message: Value,
}

fn prepare_conversation(session: &UniversalSession) -> Vec<PreparedMessage<'_>> {
    let mut out = Vec::new();
    for m in &session.messages {
        prepare_message(session, m, &mut out);
    }
    out
}

fn prepare_message<'a>(
    session: &UniversalSession,
    m: &'a UMessage,
    out: &mut Vec<PreparedMessage<'a>>,
) {
    if should_skip_foreign_runtime_context(m) {
        return;
    }
    match m.role {
        Role::User => {
            if let Some(content) = user_content_value(&m.content) {
                out.push(PreparedMessage {
                    msg: m,
                    role: Role::User,
                    uuid: claude_line_uuid(&m.id),
                    timestamp: synthesized_timestamp(session, m),
                    message: json!({
                        "role": "user",
                        "content": content,
                    }),
                });
            }
        }
        Role::Tool => {
            let content = Value::Array(content_blocks_to_claude(&m.content, Role::Tool));
            if content.as_array().map(|arr| arr.is_empty()).unwrap_or(true) {
                return;
            }
            out.push(PreparedMessage {
                msg: m,
                role: Role::Tool,
                uuid: claude_line_uuid(&m.id),
                timestamp: synthesized_timestamp(session, m),
                message: json!({
                    "role": "user",
                    "content": content,
                }),
            })
        }
        Role::Assistant => prepare_assistant_message(session, m, out),
        Role::System | Role::Developer => {}
    }
}

fn prepare_assistant_message<'a>(
    session: &UniversalSession,
    m: &'a UMessage,
    out: &mut Vec<PreparedMessage<'a>>,
) {
    let mut assistant_blocks = Vec::new();
    let mut tool_result_blocks = Vec::new();
    let mut split_idx = 0usize;

    for block in &m.content {
        if matches!(block, ContentBlock::ToolResult { .. }) {
            tool_result_blocks.push(block.clone());
            continue;
        }

        if !tool_result_blocks.is_empty() {
            push_prepared_assistant(session, m, &assistant_blocks, split_idx, out);
            split_idx += 1;
            assistant_blocks.clear();
            push_prepared_tool_results(session, m, &tool_result_blocks, split_idx, out);
            split_idx += 1;
            tool_result_blocks.clear();
        }
        assistant_blocks.push(block.clone());
    }

    if !tool_result_blocks.is_empty() {
        push_prepared_assistant(session, m, &assistant_blocks, split_idx, out);
        split_idx += 1;
        push_prepared_tool_results(session, m, &tool_result_blocks, split_idx, out);
    } else {
        push_prepared_assistant(session, m, &assistant_blocks, split_idx, out);
    }
}

fn push_prepared_assistant<'a>(
    session: &UniversalSession,
    m: &'a UMessage,
    blocks: &[ContentBlock],
    split_idx: usize,
    out: &mut Vec<PreparedMessage<'a>>,
) {
    let content = content_blocks_to_claude(blocks, Role::Assistant);
    if content.is_empty() {
        return;
    }
    let line_id = split_message_id(&m.id, split_idx);
    let mut inner = Map::new();
    inner.insert("id".into(), Value::String(assistant_message_id(&line_id)));
    inner.insert("type".into(), Value::String("message".into()));
    inner.insert("role".into(), Value::String("assistant".into()));
    inner.insert("model".into(), Value::String(claude_model_id(session, m)));
    inner.insert("content".into(), Value::Array(content.clone()));
    inner.insert(
        "stop_reason".into(),
        Value::String(
            m.stop_reason
                .clone()
                .unwrap_or_else(|| default_stop_reason(&content).into()),
        ),
    );
    if let Some(usage) = &m.usage {
        let mut u = Map::new();
        if let Some(v) = usage.input_tokens {
            u.insert("input_tokens".into(), json!(v));
        }
        if let Some(v) = usage.output_tokens {
            u.insert("output_tokens".into(), json!(v));
        }
        if let Some(v) = usage.cached_input_tokens {
            u.insert("cache_read_input_tokens".into(), json!(v));
        }
        if !u.is_empty() {
            inner.insert("usage".into(), Value::Object(u));
        }
    }
    out.push(PreparedMessage {
        msg: m,
        role: Role::Assistant,
        uuid: claude_line_uuid(&line_id),
        timestamp: synthesized_timestamp(session, m),
        message: Value::Object(inner),
    });
}

fn push_prepared_tool_results<'a>(
    session: &UniversalSession,
    m: &'a UMessage,
    blocks: &[ContentBlock],
    split_idx: usize,
    out: &mut Vec<PreparedMessage<'a>>,
) {
    let content = Value::Array(content_blocks_to_claude(blocks, Role::Tool));
    if content.as_array().map(|arr| arr.is_empty()).unwrap_or(true) {
        return;
    }
    out.push(PreparedMessage {
        msg: m,
        role: Role::Tool,
        uuid: claude_line_uuid(&split_message_id(&m.id, split_idx)),
        timestamp: synthesized_timestamp(session, m),
        message: json!({
            "role": "user",
            "content": content,
        }),
    });
}

fn split_message_id(id: &str, split_idx: usize) -> String {
    if split_idx == 0 {
        id.to_string()
    } else {
        format!("{id}:split:{split_idx}")
    }
}

fn synthesize_conversation_line(
    session: &UniversalSession,
    prepared: &PreparedMessage<'_>,
    parent_uuid: Option<&str>,
) -> Value {
    let role_type = match prepared.role {
        Role::Assistant => "assistant",
        _ => "user",
    };
    let mut top = Map::new();
    top.insert(
        "parentUuid".into(),
        parent_uuid
            .map(|p| Value::String(p.to_string()))
            .unwrap_or(Value::Null),
    );
    top.insert(
        "isSidechain".into(),
        Value::Bool(prepared.msg.flags.is_sidechain),
    );
    if role_type == "user" {
        top.insert(
            "promptId".into(),
            Value::String(uuid::Uuid::now_v7().to_string()),
        );
        top.insert("permissionMode".into(), Value::String("default".into()));
    } else if role_type == "assistant" {
        top.insert(
            "requestId".into(),
            Value::String(synthetic_claude_request_id()),
        );
    }
    top.insert("type".into(), Value::String(role_type.into()));
    top.insert("message".into(), prepared.message.clone());
    top.insert("uuid".into(), Value::String(prepared.uuid.clone()));
    top.insert(
        "timestamp".into(),
        Value::String(prepared.timestamp.clone()),
    );
    top.insert("userType".into(), Value::String("external".into()));
    top.insert("entrypoint".into(), Value::String("sdk-cli".into()));
    top.insert("cwd".into(), Value::String(session.cwd.clone()));
    top.insert(
        "sessionId".into(),
        Value::String(session.session_id.clone()),
    );
    top.insert(
        "version".into(),
        Value::String(SYNTHETIC_CLAUDE_VERSION.into()),
    );
    top.insert(
        "gitBranch".into(),
        Value::String(
            session
                .git
                .as_ref()
                .and_then(|g| g.branch.as_ref())
                .filter(|branch| !branch.is_empty())
                .cloned()
                .unwrap_or_else(|| "HEAD".into()),
        ),
    );
    Value::Object(top)
}

fn prepared_text(prepared: &PreparedMessage<'_>) -> Option<String> {
    prepared
        .message
        .get("content")
        .and_then(|content| {
            if let Some(text) = content.as_str() {
                Some(text.to_string())
            } else if let Some(arr) = content.as_array() {
                let text = arr
                    .iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                            block.get("text").and_then(|v| v.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            } else {
                None
            }
        })
        .filter(|text| !text.trim().is_empty())
}

fn should_skip_foreign_runtime_context(message: &UMessage) -> bool {
    if message.flags.is_meta {
        return true;
    }
    if matches!(message.role, Role::System | Role::Developer) {
        return true;
    }
    if message.role != Role::User {
        return false;
    }
    let mut text = String::new();
    for block in &message.content {
        if let ContentBlock::Text { text: part, .. } = block {
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

fn synthetic_claude_request_id() -> String {
    format!("req_{}", &uuid::Uuid::now_v7().simple().to_string()[..24])
}

fn user_content_value(blocks: &[ContentBlock]) -> Option<Value> {
    let values = content_blocks_to_claude(blocks, Role::User);
    if values.len() == 1 {
        if values[0].get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(text) = values[0].get("text").and_then(|v| v.as_str()) {
                return Some(Value::String(text.to_string()));
            }
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(Value::Array(values))
    }
}

fn content_blocks_to_claude(blocks: &[ContentBlock], role: Role) -> Vec<Value> {
    let mut out = Vec::new();
    for b in blocks {
        match b {
            ContentBlock::Text { text, .. } => {
                if !text.is_empty() {
                    out.push(json!({"type": "text", "text": text}));
                }
            }
            ContentBlock::Thinking { text, extras, .. } => {
                if text.is_empty() {
                    continue;
                }
                if let Some(sig) = extras.get("signature") {
                    let mut o = json!({"type": "thinking", "thinking": text});
                    o["signature"] = sig.clone();
                    out.push(o);
                } else {
                    out.push(json!({"type": "text", "text": text}));
                }
            }
            ContentBlock::ToolUse {
                call_id,
                name,
                input,
                ..
            } => {
                if matches!(role, Role::Assistant) && !call_id.is_empty() && !name.is_empty() {
                    out.push(json!({
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": input,
                    }));
                }
            }
            ContentBlock::ToolResult {
                call_id,
                output,
                is_error,
                extras,
                ..
            } => {
                if matches!(role, Role::User | Role::Tool) && !call_id.is_empty() {
                    // Tool results live inside a user-role line in Claude's
                    // format. Preserve source Claude content arrays verbatim
                    // when they were observed; otherwise normalize output to
                    // Claude's common string shape.
                    let content = if extras
                        .get("claude_tool_result_content_array")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                        && output.is_array()
                    {
                        output.clone()
                    } else {
                        Value::String(match output {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                    };
                    let mut o = json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                    });
                    if *is_error {
                        o["is_error"] = Value::Bool(true);
                    }
                    out.push(o);
                }
            }
            ContentBlock::Image { mime, source, .. } => {
                if let crate::universal::ImageSource::Base64 { data } = source {
                    out.push(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": mime,
                            "data": data,
                        }
                    }));
                } else {
                    // path / url — pass through as best we can
                    out.push(json!({"type": "image", "mime": mime, "source": source}));
                }
            }
            ContentBlock::Attachment {
                name, path, mime, ..
            } => out.push(json!({
                "type": "attachment", "name": name, "path": path, "mime": mime
            })),
            ContentBlock::Patch { unified_diff, .. } => {
                if !unified_diff.is_empty() {
                    out.push(json!({"type": "text", "text": unified_diff}));
                }
            }
            // Provider control records such as OpenCode step-start/step-finish
            // are not valid visible Claude transcript blocks.
            ContentBlock::Other { .. } => {}
        }
    }
    out
}

fn synthesized_timestamp(session: &UniversalSession, m: &UMessage) -> String {
    m.timestamp
        .or(session.updated_at)
        .or(session.created_at)
        .map(to_rfc3339_ms)
        .unwrap_or_else(|| to_rfc3339_ms(chrono::Utc::now()))
}

fn synthesize_title(session: &UniversalSession) -> Option<String> {
    session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(trim_title)
        .or_else(|| {
            session.messages.iter().find_map(|m| {
                if m.role != Role::User {
                    return None;
                }
                m.content.iter().find_map(|b| {
                    if let ContentBlock::Text { text, .. } = b {
                        let title = trim_title(text.trim());
                        if title.is_empty() {
                            None
                        } else {
                            Some(title)
                        }
                    } else {
                        None
                    }
                })
            })
        })
}

fn trim_title(title: &str) -> String {
    let first_line = title.lines().next().unwrap_or("").trim();
    let mut out: String = first_line.chars().take(80).collect();
    if first_line.chars().count() > 80 {
        out.push_str("...");
    }
    out
}

fn claude_line_uuid(id: &str) -> String {
    if uuid::Uuid::parse_str(id).is_ok() {
        id.to_string()
    } else {
        uuid::Uuid::now_v7().to_string()
    }
}

fn assistant_message_id(id: &str) -> String {
    if id.starts_with("msg_") {
        return id.to_string();
    }
    let suffix: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(32)
        .collect();
    if suffix.is_empty() {
        format!("msg_{}", uuid::Uuid::now_v7().simple())
    } else {
        format!("msg_{}", suffix)
    }
}

fn claude_model_id(session: &UniversalSession, m: &UMessage) -> String {
    m.model
        .as_ref()
        .or(session.model.as_ref())
        .map(|model| model.model_id.as_str())
        .filter(|id| id.starts_with("claude"))
        .unwrap_or("claude-sonnet-4-5")
        .to_string()
}

fn default_stop_reason(content: &[Value]) -> &'static str {
    if content
        .iter()
        .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
    {
        "tool_use"
    } else {
        "end_turn"
    }
}
