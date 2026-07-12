//! Pi JSONL entries → UMessage mapping.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::debug;
use crate::error::Result;
use crate::time;
use crate::universal::{
    ContentBlock, ImageSource, MessageFlags, ModelInfo, Provenance, Provider, Role, UMessage,
    UniversalSession, Usage, SCHEMA_VERSION,
};

use super::PiReadCtx;

pub fn parse_lines(content: &str, ctx: &PiReadCtx) -> Result<UniversalSession> {
    let mut session = UniversalSession {
        schema_version: SCHEMA_VERSION.to_string(),
        session_id: ctx.session_id.clone().unwrap_or_default(),
        origin: Default::default(),
        cwd: ctx.cwd.clone().unwrap_or_default(),
        created_at: None,
        updated_at: None,
        title: None,
        model: None,
        git: None,
        usage_total: None,
        session_meta: None,
        messages: Vec::new(),
        extras: BTreeMap::new(),
    };
    session.origin.provider = Some(Provider::Pi);

    let mut idx = 0u32;
    let mut invalid_json_lines = 0usize;
    let mut non_empty_lines = 0usize;
    let mut header_seen = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        non_empty_lines = non_empty_lines.saturating_add(1);
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                invalid_json_lines = invalid_json_lines.saturating_add(1);
                continue;
            }
        };
        let entry_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        let ts = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(time::parse_rfc3339);

        if !header_seen {
            header_seen = true;
            if entry_type == "session" {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    if session.session_id.is_empty() {
                        session.session_id = id.to_string();
                    }
                }
                if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
                    if session.cwd.is_empty() {
                        session.cwd = cwd.to_string();
                    }
                }
                session.created_at = ts;
                session.session_meta = Some(value.clone());
                continue;
            }
        }

        if session.created_at.is_none() {
            session.created_at = ts;
        }
        session.updated_at = ts.or(session.updated_at);

        match entry_type {
            "message" => {
                if session.title.is_none() {
                    if let Some(title) = value.get("message").and_then(super::user_message_title) {
                        session.title = Some(title);
                    }
                }
                session
                    .messages
                    .push(message_entry_to_umessage(&value, idx, ts));
                idx = idx.saturating_add(1);
            }
            "custom_message" => {
                session
                    .messages
                    .push(custom_message_entry_to_umessage(&value, idx, ts));
                idx = idx.saturating_add(1);
            }
            "compaction" => {
                session.messages.push(summary_entry_to_umessage(
                    &value,
                    idx,
                    ts,
                    "pi:compaction",
                    true,
                ));
                idx = idx.saturating_add(1);
            }
            "branch_summary" => {
                session.messages.push(summary_entry_to_umessage(
                    &value,
                    idx,
                    ts,
                    "pi:branch_summary",
                    false,
                ));
                idx = idx.saturating_add(1);
            }
            "model_change" => {
                if let Some(model) = model_change_from_entry(&value) {
                    session.model = Some(model);
                }
                session
                    .messages
                    .push(meta_entry_to_umessage(&value, idx, ts, "pi:model_change"));
                idx = idx.saturating_add(1);
            }
            "thinking_level_change" | "label" | "custom" => {
                session.messages.push(meta_entry_to_umessage(
                    &value,
                    idx,
                    ts,
                    &format!("pi:{entry_type}"),
                ));
                idx = idx.saturating_add(1);
            }
            "session_info" => {
                if let Some(title) = value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                {
                    session.title = Some(title);
                }
                session
                    .messages
                    .push(meta_entry_to_umessage(&value, idx, ts, "pi:session_info"));
                idx = idx.saturating_add(1);
            }
            _ => {
                session.messages.push(meta_entry_to_umessage(
                    &value,
                    idx,
                    ts,
                    &format!("pi:{entry_type}"),
                ));
                idx = idx.saturating_add(1);
            }
        }
    }

    debug::log(
        "provider_pi_parse_ok",
        serde_json::json!({
            "lines": non_empty_lines,
            "invalid_json_lines": invalid_json_lines,
            "session_id_present": !session.session_id.is_empty(),
            "cwd_present": !session.cwd.is_empty(),
            "messages": session.messages.len(),
        }),
    );
    Ok(session)
}

fn message_entry_to_umessage(value: &Value, index: u32, ts: Option<DateTime<Utc>>) -> UMessage {
    let message = value.get("message").unwrap_or(&Value::Null);
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    let mut flags = MessageFlags::default();
    let mut extras = BTreeMap::new();
    extras.insert("pi_entry_type".into(), json!("message"));

    let (role, content) = match role {
        "user" => (Role::User, pi_content_to_blocks(message.get("content"))),
        "assistant" => (
            Role::Assistant,
            pi_content_to_blocks(message.get("content")),
        ),
        "toolResult" => {
            let mut block_extras = BTreeMap::new();
            if let Some(tool_name) = message.get("toolName") {
                block_extras.insert("toolName".into(), tool_name.clone());
            }
            if let Some(details) = message.get("details") {
                block_extras.insert("details".into(), details.clone());
            }
            (
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    call_id: message
                        .get("toolCallId")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    output: pi_tool_result_output(message),
                    is_error: message
                        .get("isError")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    extras: block_extras,
                }],
            )
        }
        "bashExecution" => {
            let mut block_extras = BTreeMap::new();
            if let Some(command) = message.get("command").and_then(Value::as_str) {
                block_extras.insert("command".into(), json!(command));
            }
            if let Some(exit_code) = message.get("exitCode") {
                block_extras.insert("exitCode".into(), exit_code.clone());
            }
            (
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    call_id: entry_id(value),
                    output: json!(message.get("output").and_then(Value::as_str).unwrap_or("")),
                    is_error: message
                        .get("exitCode")
                        .and_then(Value::as_i64)
                        .is_some_and(|code| code != 0),
                    extras: block_extras,
                }],
            )
        }
        "custom" => {
            flags.is_meta = true;
            (Role::System, pi_content_to_blocks(message.get("content")))
        }
        _ => {
            flags.is_meta = true;
            (
                Role::System,
                vec![ContentBlock::other("pi_message", message.clone())],
            )
        }
    };

    UMessage {
        id: entry_id(value),
        parent_id: parent_id(value),
        index,
        timestamp: ts,
        role,
        model: assistant_model(message),
        usage: assistant_usage(message),
        stop_reason: message
            .get("stopReason")
            .and_then(Value::as_str)
            .map(str::to_string),
        content,
        flags,
        provenance: Provenance {
            source_event_type: format!(
                "pi:message.{role}",
                role = message
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ),
            raw: value.clone(),
        },
        extras,
    }
}

fn custom_message_entry_to_umessage(
    value: &Value,
    index: u32,
    ts: Option<DateTime<Utc>>,
) -> UMessage {
    let mut extras = BTreeMap::new();
    extras.insert("pi_entry_type".into(), json!("custom_message"));
    if let Some(custom_type) = value.get("customType") {
        extras.insert("customType".into(), custom_type.clone());
    }
    if let Some(display) = value.get("display") {
        extras.insert("display".into(), display.clone());
    }
    UMessage {
        id: entry_id(value),
        parent_id: parent_id(value),
        index,
        timestamp: ts,
        role: Role::System,
        model: None,
        usage: None,
        stop_reason: None,
        content: pi_content_to_blocks(value.get("content")),
        flags: MessageFlags::default(),
        provenance: Provenance {
            source_event_type: "pi:custom_message".into(),
            raw: value.clone(),
        },
        extras,
    }
}

fn summary_entry_to_umessage(
    value: &Value,
    index: u32,
    ts: Option<DateTime<Utc>>,
    source_event_type: &str,
    is_compaction: bool,
) -> UMessage {
    let flags = MessageFlags {
        is_compaction,
        ..Default::default()
    };
    UMessage {
        id: entry_id(value),
        parent_id: parent_id(value),
        index,
        timestamp: ts,
        role: Role::System,
        model: None,
        usage: None,
        stop_reason: None,
        content: vec![ContentBlock::text(
            value
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )],
        flags,
        provenance: Provenance {
            source_event_type: source_event_type.into(),
            raw: value.clone(),
        },
        extras: BTreeMap::new(),
    }
}

fn meta_entry_to_umessage(
    value: &Value,
    index: u32,
    ts: Option<DateTime<Utc>>,
    source_event_type: &str,
) -> UMessage {
    let flags = MessageFlags {
        is_meta: true,
        ..Default::default()
    };
    UMessage {
        id: entry_id(value),
        parent_id: parent_id(value),
        index,
        timestamp: ts,
        role: Role::System,
        model: None,
        usage: None,
        stop_reason: None,
        content: vec![ContentBlock::other("pi_entry", value.clone())],
        flags,
        provenance: Provenance {
            source_event_type: source_event_type.into(),
            raw: value.clone(),
        },
        extras: BTreeMap::new(),
    }
}

fn entry_id(value: &Value) -> String {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(crate::ids::new_uuid_v7)
}

fn parent_id(value: &Value) -> Option<String> {
    value
        .get("parentId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn pi_content_to_blocks(value: Option<&Value>) -> Vec<ContentBlock> {
    match value {
        Some(Value::String(text)) => vec![ContentBlock::text(text.clone())],
        Some(Value::Array(items)) => items.iter().map(pi_content_block_to_block).collect(),
        Some(value) if !value.is_null() => vec![ContentBlock::other("pi_content", value.clone())],
        _ => Vec::new(),
    }
}

fn pi_content_block_to_block(value: &Value) -> ContentBlock {
    match value.get("type").and_then(Value::as_str) {
        Some("text") => ContentBlock::text(
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
        Some("thinking") => ContentBlock::thinking(
            value
                .get("thinking")
                .or_else(|| value.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
        Some("image") => ContentBlock::Image {
            mime: value
                .get("mimeType")
                .or_else(|| value.get("mime"))
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string(),
            source: ImageSource::Base64 {
                data: value
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
            extras: BTreeMap::new(),
        },
        Some("toolCall") => ContentBlock::ToolUse {
            call_id: value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            input: value.get("arguments").cloned().unwrap_or(Value::Null),
            extras: BTreeMap::new(),
        },
        Some(kind) => ContentBlock::other(kind, value.clone()),
        None => ContentBlock::other("pi_content", value.clone()),
    }
}

fn pi_tool_result_output(message: &Value) -> Value {
    match message.get("content") {
        Some(Value::String(text)) => json!(text),
        Some(value) => value.clone(),
        None => Value::Null,
    }
}

fn assistant_model(message: &Value) -> Option<ModelInfo> {
    let model_id = message.get("model").and_then(Value::as_str)?;
    Some(ModelInfo {
        provider_id: message
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
        model_id: model_id.to_string(),
        variant: None,
    })
}

fn model_change_from_entry(value: &Value) -> Option<ModelInfo> {
    let model_value = value
        .get("modelId")
        .or_else(|| value.get("model"))
        .and_then(Value::as_str)?;
    let explicit_provider = value.get("provider").and_then(Value::as_str);
    let (provider_id, model_id) = if explicit_provider.is_none() && model_value.contains('/') {
        let mut parts = model_value.splitn(2, '/');
        (
            parts.next().filter(|value| !value.is_empty()),
            parts.next().unwrap_or(model_value),
        )
    } else {
        (explicit_provider, model_value)
    };
    Some(ModelInfo {
        provider_id: provider_id.map(str::to_string),
        model_id: model_id.to_string(),
        variant: None,
    })
}

fn assistant_usage(message: &Value) -> Option<Usage> {
    let usage = message.get("usage")?;
    Some(Usage {
        input_tokens: usage.get("input").and_then(Value::as_u64),
        output_tokens: usage.get("output").and_then(Value::as_u64),
        cached_input_tokens: usage.get("cacheRead").and_then(Value::as_u64),
        reasoning_output_tokens: None,
        total_tokens: usage.get("totalTokens").and_then(Value::as_u64),
        cost_usd: usage
            .get("cost")
            .and_then(|cost| cost.get("total"))
            .and_then(Value::as_f64),
    })
}
