//! UniversalSession → Pi session JSONL.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::debug;
use crate::error::Result;
use crate::time::to_rfc3339_ms;
use crate::universal::{ContentBlock, ImageSource, Role, UMessage, UniversalSession};

use super::{PiWriteOpts, CURRENT_SESSION_VERSION};

pub fn to_jsonl_path(session: &UniversalSession, path: &Path, opts: &PiWriteOpts) -> Result<()> {
    debug::log(
        "provider_pi_write_file_start",
        serde_json::json!({
            "path": path.display().to_string(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "replay_raw": opts.replay_raw,
        }),
    );
    let text = to_jsonl_string(session, opts)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    crate::jsonl::write_text_atomic(path, &text)?;
    debug::log(
        "provider_pi_write_file_ok",
        serde_json::json!({
            "path": path.display().to_string(),
            "session_id": &session.session_id,
            "bytes": text.len(),
            "lines": text.lines().count(),
        }),
    );
    Ok(())
}

pub fn to_jsonl_string(session: &UniversalSession, opts: &PiWriteOpts) -> Result<String> {
    let mut out = String::new();
    out.push_str(&serde_json::to_string(&session_header(session))?);
    out.push('\n');

    let replay_raw = opts.replay_raw
        && session
            .messages
            .iter()
            .any(|message| message.provenance.source_event_type.starts_with("pi:"));

    let mut previous_entry_id: Option<String> = None;
    for message in &session.messages {
        if replay_raw && message.provenance.source_event_type.starts_with("pi:") {
            let line = serde_json::to_string(&message.provenance.raw)?;
            out.push_str(&line);
            out.push('\n');
            previous_entry_id = Some(message.id.clone());
            continue;
        }
        let entry = synth_entry(session, message, previous_entry_id.as_deref());
        previous_entry_id = entry
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(message.id.clone()));
        out.push_str(&serde_json::to_string(&entry)?);
        out.push('\n');
    }
    Ok(out)
}

fn session_header(session: &UniversalSession) -> Value {
    let timestamp = session.created_at.unwrap_or_else(Utc::now);
    let mut header = serde_json::Map::new();
    header.insert("type".into(), json!("session"));
    header.insert("version".into(), json!(CURRENT_SESSION_VERSION));
    header.insert("id".into(), json!(session.session_id));
    header.insert("timestamp".into(), json!(to_rfc3339_ms(timestamp)));
    header.insert("cwd".into(), json!(session.cwd));
    if let Some(parent_session) = session
        .extras
        .get("pi_parent_session")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        header.insert("parentSession".into(), json!(parent_session));
    }
    Value::Object(header)
}

fn synth_entry(
    session: &UniversalSession,
    message: &UMessage,
    previous_entry_id: Option<&str>,
) -> Value {
    let entry_id = pi_entry_id(&message.id);
    let parent_id = message
        .parent_id
        .as_deref()
        .map(pi_entry_id)
        .or_else(|| previous_entry_id.map(str::to_string));
    let timestamp = message.timestamp.unwrap_or_else(Utc::now);
    match message.role {
        Role::User => json!({
            "type": "message",
            "id": entry_id,
            "parentId": parent_id,
            "timestamp": to_rfc3339_ms(timestamp),
            "message": {
                "role": "user",
                "content": blocks_to_pi_user_content(&message.content),
                "timestamp": timestamp.timestamp_millis(),
            },
        }),
        Role::Assistant => json!({
            "type": "message",
            "id": entry_id,
            "parentId": parent_id,
            "timestamp": to_rfc3339_ms(timestamp),
            "message": {
                "role": "assistant",
                "content": blocks_to_pi_assistant_content(&message.content),
                "api": "cokacmux",
                "provider": message.model.as_ref().and_then(|m| m.provider_id.as_deref()).unwrap_or("cokacmux"),
                "model": message.model.as_ref().map(|m| m.model_id.as_str()).or_else(|| session.model.as_ref().map(|m| m.model_id.as_str())).unwrap_or("context"),
                "usage": pi_usage(message),
                "stopReason": message.stop_reason.as_deref().unwrap_or("stop"),
                "timestamp": timestamp.timestamp_millis(),
            },
        }),
        Role::Tool => synth_tool_entry(message, &entry_id, parent_id, timestamp),
        Role::System | Role::Developer => json!({
            "type": "custom_message",
            "id": entry_id,
            "parentId": parent_id,
            "timestamp": to_rfc3339_ms(timestamp),
            "customType": "cokacmux",
            "content": blocks_to_plain_text(&message.content),
            "display": true,
        }),
    }
}

fn synth_tool_entry(
    message: &UMessage,
    entry_id: &str,
    parent_id: Option<String>,
    timestamp: DateTime<Utc>,
) -> Value {
    if let Some((call_id, output, is_error, tool_name)) = first_tool_result(&message.content) {
        return json!({
            "type": "message",
            "id": entry_id,
            "parentId": parent_id,
            "timestamp": to_rfc3339_ms(timestamp),
            "message": {
                "role": "toolResult",
                "toolCallId": call_id,
                "toolName": tool_name,
                "content": output,
                "isError": is_error,
                "timestamp": timestamp.timestamp_millis(),
            },
        });
    }
    json!({
        "type": "custom_message",
        "id": entry_id,
        "parentId": parent_id,
        "timestamp": to_rfc3339_ms(timestamp),
        "customType": "cokacmux-tool",
        "content": blocks_to_plain_text(&message.content),
        "display": true,
    })
}

fn first_tool_result(content: &[ContentBlock]) -> Option<(String, Value, bool, String)> {
    for block in content {
        if let ContentBlock::ToolResult {
            call_id,
            output,
            is_error,
            extras,
        } = block
        {
            let tool_name = extras
                .get("toolName")
                .or_else(|| extras.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            return Some((call_id.clone(), output.clone(), *is_error, tool_name));
        }
    }
    None
}

fn blocks_to_pi_user_content(blocks: &[ContentBlock]) -> Value {
    if blocks.len() == 1 {
        if let ContentBlock::Text { text, .. } = &blocks[0] {
            return json!(text);
        }
    }
    Value::Array(blocks.iter().filter_map(block_to_pi_content).collect())
}

fn blocks_to_pi_assistant_content(blocks: &[ContentBlock]) -> Value {
    Value::Array(blocks.iter().filter_map(block_to_pi_content).collect())
}

fn block_to_pi_content(block: &ContentBlock) -> Option<Value> {
    match block {
        ContentBlock::Text { text, .. } => Some(json!({"type": "text", "text": text})),
        ContentBlock::Thinking { text, .. } => Some(json!({"type": "thinking", "thinking": text})),
        ContentBlock::ToolUse {
            call_id,
            name,
            input,
            ..
        } => Some(json!({
            "type": "toolCall",
            "id": call_id,
            "name": name,
            "arguments": input,
        })),
        ContentBlock::Image { mime, source, .. } => match source {
            ImageSource::Base64 { data } => Some(json!({
                "type": "image",
                "mimeType": mime,
                "data": data,
            })),
            _ => Some(json!({
                "type": "text",
                "text": format!("[image: {} {}]", mime, image_source_label(source)),
            })),
        },
        ContentBlock::ToolResult { .. } => None,
        ContentBlock::Attachment { path, name, .. } => Some(json!({
            "type": "text",
            "text": format!("[attachment: {}{}]", name.as_deref().unwrap_or(""), path.as_deref().map(|p| format!(" {p}")).unwrap_or_default()),
        })),
        ContentBlock::Patch { unified_diff, .. } => Some(json!({
            "type": "text",
            "text": unified_diff,
        })),
        ContentBlock::Other { type_tag, payload } => Some(json!({
            "type": "text",
            "text": format!("[{}] {}", type_tag, payload),
        })),
    }
}

fn blocks_to_plain_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => out.push_str(text),
            ContentBlock::Thinking { text, .. } => out.push_str(text),
            ContentBlock::ToolUse {
                call_id,
                name,
                input,
                ..
            } => out.push_str(&format!("tool_use[{call_id}] {name}: {input}\n")),
            ContentBlock::ToolResult {
                call_id,
                output,
                is_error,
                ..
            } => out.push_str(&format!(
                "tool_result[{call_id}]{}: {output}\n",
                if *is_error { " error" } else { "" }
            )),
            ContentBlock::Image { mime, source, .. } => out.push_str(&format!(
                "[image: {} {}]\n",
                mime,
                image_source_label(source)
            )),
            ContentBlock::Attachment { name, path, .. } => out.push_str(&format!(
                "[attachment: {}{}]\n",
                name.as_deref().unwrap_or(""),
                path.as_deref().map(|p| format!(" {p}")).unwrap_or_default()
            )),
            ContentBlock::Patch { unified_diff, .. } => out.push_str(unified_diff),
            ContentBlock::Other { type_tag, payload } => {
                out.push_str(&format!("[{}] {}\n", type_tag, payload))
            }
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn image_source_label(source: &ImageSource) -> String {
    match source {
        ImageSource::LocalPath { path } => path.clone(),
        ImageSource::Base64 { data } => format!("base64:{} bytes", data.len()),
        ImageSource::Url { url } => url.clone(),
    }
}

fn pi_usage(message: &UMessage) -> Value {
    let usage = message.usage.as_ref();
    let input = usage.and_then(|usage| usage.input_tokens).unwrap_or(0);
    let output = usage.and_then(|usage| usage.output_tokens).unwrap_or(0);
    let cache_read = usage
        .and_then(|usage| usage.cached_input_tokens)
        .unwrap_or(0);
    let total = usage
        .and_then(|usage| usage.total_tokens)
        .unwrap_or(input + output);
    let cost_total = usage.and_then(|usage| usage.cost_usd).unwrap_or(0.0);
    json!({
        "input": input,
        "output": output,
        "cacheRead": cache_read,
        "cacheWrite": 0,
        "totalTokens": total,
        "cost": {
            "input": 0,
            "output": 0,
            "cacheRead": 0,
            "cacheWrite": 0,
            "total": cost_total,
        },
    })
}

fn pi_entry_id(id: &str) -> String {
    let trimmed = id.trim();
    if !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return trimmed.to_string();
    }
    crate::ids::new_uuid_v7()
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .take(8)
        .collect()
}
