//! Assemble a UniversalSession from opencode session/message/part rows.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::debug;
use crate::error::Result;
use crate::time;
use crate::universal::{
    ContentBlock, ImageSource, MessageFlags, ModelInfo, Provenance, Provider, Role, UMessage,
    UniversalSession, Usage, SCHEMA_VERSION,
};

use super::read::{MessageRow, PartRow, SessionMessageRow, SessionRow};

pub fn build_session(
    session: &SessionRow,
    messages: &[MessageRow],
    parts: &[PartRow],
    session_messages: &[SessionMessageRow],
) -> Result<UniversalSession> {
    debug::log(
        "provider_opencode_build_session_start",
        serde_json::json!({
            "session_id": &session.id,
            "message_rows": messages.len(),
            "part_rows": parts.len(),
            "session_message_rows": session_messages.len(),
        }),
    );
    let mut s = UniversalSession {
        schema_version: SCHEMA_VERSION.to_string(),
        session_id: session.id.clone(),
        origin: crate::universal::ProviderOrigin {
            provider: Some(Provider::OpenCode),
            cli_version: if session.version.is_empty() {
                None
            } else {
                Some(session.version.clone())
            },
            ..Default::default()
        },
        cwd: session.directory.clone(),
        created_at: time::from_epoch_ms(session.time_created),
        updated_at: time::from_epoch_ms(session.time_updated),
        title: if session.title.is_empty() {
            None
        } else {
            Some(session.title.clone())
        },
        // OpenCode's `session.model` is a JSON-stringified object with
        // {id, providerID, variant}. The `agent` column ("build", "plan", …)
        // is SEPARATE from model.variant and we must not conflate them — if
        // we drop "agent" into `ModelInfo.variant` we lose it on round-trip
        // (the original model.variant gets overwritten). Keep model parsing
        // strict and stash `session.agent` in session.extras for write-back.
        model: session.model.as_deref().and_then(|s| {
            let v = serde_json::Value::String(s.to_string());
            extract_model(&v)
        }),
        git: None,
        usage_total: Some(Usage {
            input_tokens: Some(session.tokens_input as u64),
            output_tokens: Some(session.tokens_output as u64),
            cached_input_tokens: Some(session.tokens_cache_read as u64),
            reasoning_output_tokens: Some(session.tokens_reasoning as u64),
            total_tokens: None,
            cost_usd: Some(session.cost),
        }),
        session_meta: None,
        messages: Vec::new(),
        extras: BTreeMap::new(),
    };

    // Preserve OpenCode-specific row-level fields that don't belong on
    // the UniversalSession surface but matter for round-tripping back.
    if let Some(a) = &session.agent {
        s.extras.insert("opencode_agent".into(), json!(a.clone()));
    }
    if !session.project_id.is_empty() {
        s.extras.insert(
            "opencode_project_id".into(),
            json!(session.project_id.clone()),
        );
    }
    insert_nonempty_string(&mut s.extras, "opencode_slug", &session.slug);
    insert_nonempty_string(&mut s.extras, "opencode_version", &session.version);
    insert_opt_string(&mut s.extras, "opencode_parent_id", &session.parent_id);
    insert_opt_string(&mut s.extras, "opencode_share_url", &session.share_url);
    insert_opt_i64(
        &mut s.extras,
        "opencode_summary_additions",
        session.summary_additions,
    );
    insert_opt_i64(
        &mut s.extras,
        "opencode_summary_deletions",
        session.summary_deletions,
    );
    insert_opt_i64(
        &mut s.extras,
        "opencode_summary_files",
        session.summary_files,
    );
    insert_opt_string(
        &mut s.extras,
        "opencode_summary_diffs",
        &session.summary_diffs,
    );
    insert_opt_string(&mut s.extras, "opencode_revert", &session.revert);
    insert_opt_string(&mut s.extras, "opencode_permission", &session.permission);
    insert_opt_i64(
        &mut s.extras,
        "opencode_time_compacting",
        session.time_compacting,
    );
    insert_opt_i64(
        &mut s.extras,
        "opencode_time_archived",
        session.time_archived,
    );
    insert_opt_string(
        &mut s.extras,
        "opencode_workspace_id",
        &session.workspace_id,
    );
    insert_opt_string(&mut s.extras, "opencode_path", &session.path);
    for row in session_messages {
        let data = row.parse_data();
        match row.type_tag.as_str() {
            "agent-switched" => {
                if let Some(agent) = data.get("agent").and_then(|v| v.as_str()) {
                    if !agent.trim().is_empty() {
                        s.extras.insert("opencode_agent".into(), json!(agent));
                    }
                }
            }
            "model-switched" => {
                if let Some(model) = data.get("model").and_then(extract_model) {
                    s.model = Some(model);
                }
            }
            _ => {}
        }
    }

    let mut idx: u32 = 0;
    for m in messages {
        let data = m.parse_data();
        let role = data
            .get("role")
            .and_then(|v| v.as_str())
            .map(parse_role)
            .unwrap_or(Role::System);
        let model = data
            .get("model")
            .and_then(extract_model)
            .or_else(|| extract_model(&data));
        let usage = data.get("tokens").and_then(extract_usage);

        // Collect part rows for this message in order.
        let my_parts: Vec<&PartRow> = parts.iter().filter(|p| p.message_id == m.id).collect();
        let blocks = parts_to_blocks(&my_parts);

        let umsg = UMessage {
            id: m.id.clone(),
            parent_id: data
                .get("parentID")
                .and_then(|v| v.as_str())
                .map(String::from),
            index: idx,
            timestamp: time::from_epoch_ms(m.time_created),
            role,
            model,
            usage,
            stop_reason: data
                .get("finish")
                .and_then(|v| v.as_str())
                .map(String::from),
            content: blocks,
            flags: MessageFlags::default(),
            provenance: Provenance {
                source_event_type: format!("opencode:message.{}", role_str(role)),
                raw: json!({ "message": data, "parts": my_parts.iter().map(|p| p.parse_data()).collect::<Vec<_>>() }),
            },
            extras: BTreeMap::new(),
        };
        s.messages.push(umsg);
        idx += 1;
    }

    let use_session_messages_as_primary = messages.is_empty();
    for row in session_messages {
        let umsg = if use_session_messages_as_primary {
            session_message_to_primary_umessage(row, idx)
        } else {
            session_message_to_meta_umessage(row, idx)
        };
        s.messages.push(umsg);
        idx += 1;
    }

    s.messages
        .sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
    for (i, msg) in s.messages.iter_mut().enumerate() {
        msg.index = i as u32;
    }

    debug::log(
        "provider_opencode_build_session_ok",
        serde_json::json!({
            "session_id": &s.session_id,
            "messages": s.messages.len(),
            "cwd": &s.cwd,
            "title_present": s.title.is_some(),
            "model_present": s.model.is_some(),
        }),
    );
    Ok(s)
}

fn insert_nonempty_string(extras: &mut BTreeMap<String, Value>, key: &str, value: &str) {
    if !value.is_empty() {
        extras.insert(key.into(), json!(value));
    }
}

fn insert_opt_string(extras: &mut BTreeMap<String, Value>, key: &str, value: &Option<String>) {
    if let Some(value) = value.as_deref().filter(|value| !value.is_empty()) {
        extras.insert(key.into(), json!(value));
    }
}

fn insert_opt_i64(extras: &mut BTreeMap<String, Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        extras.insert(key.into(), json!(value));
    }
}

fn session_message_to_meta_umessage(row: &SessionMessageRow, idx: u32) -> UMessage {
    let data = row.parse_data();
    let mut flags = MessageFlags {
        is_meta: true,
        ..Default::default()
    };
    if row.type_tag == "compaction" {
        flags.is_compaction = true;
    }
    UMessage {
        id: row.id.clone(),
        parent_id: None,
        index: idx,
        timestamp: time::from_epoch_ms(row.time_created),
        role: Role::System,
        model: data.get("model").and_then(extract_model),
        usage: None,
        stop_reason: None,
        content: vec![ContentBlock::other(
            format!("opencode_session_message.{}", row.type_tag),
            data.clone(),
        )],
        flags,
        provenance: Provenance {
            source_event_type: format!("opencode:session_message.{}", row.type_tag),
            raw: session_message_raw(row, data),
        },
        extras: BTreeMap::new(),
    }
}

fn session_message_to_primary_umessage(row: &SessionMessageRow, idx: u32) -> UMessage {
    let data = row.parse_data();
    let mut flags = MessageFlags::default();
    let role = match row.type_tag.as_str() {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "shell" => Role::Tool,
        "compaction" => {
            flags.is_meta = true;
            flags.is_compaction = true;
            Role::System
        }
        "agent-switched" | "model-switched" => {
            flags.is_meta = true;
            Role::System
        }
        _ => Role::System,
    };
    if matches!(
        row.type_tag.as_str(),
        "synthetic" | "agent-switched" | "model-switched"
    ) {
        flags.is_meta = true;
    }
    let model = match row.type_tag.as_str() {
        "assistant" => data.get("model").and_then(extract_model),
        "model-switched" => data.get("model").and_then(extract_model),
        _ => None,
    };
    let usage = data.get("tokens").and_then(extract_usage);
    let stop_reason = data
        .get("finish")
        .and_then(|v| v.as_str())
        .map(String::from);
    let content = session_message_content(&row.type_tag, &data);

    UMessage {
        id: row.id.clone(),
        parent_id: None,
        index: idx,
        timestamp: time::from_epoch_ms(row.time_created),
        role,
        model,
        usage,
        stop_reason,
        content,
        flags,
        provenance: Provenance {
            source_event_type: format!("opencode:session_message.{}", row.type_tag),
            raw: session_message_raw(row, data),
        },
        extras: BTreeMap::new(),
    }
}

fn session_message_content(type_tag: &str, data: &Value) -> Vec<ContentBlock> {
    match type_tag {
        "user" => {
            let mut blocks = Vec::new();
            if let Some(text) = data.get("text").and_then(|v| v.as_str()) {
                blocks.push(ContentBlock::text(text));
            }
            if let Some(files) = data.get("files").and_then(|value| value.as_array()) {
                for file in files {
                    blocks.push(opencode_fileish_value_to_block(file));
                }
            } else if let Some(value) = data.get("files") {
                if !value_is_empty(value) {
                    blocks.push(ContentBlock::other("opencode_user_files", value.clone()));
                }
            }
            for key in ["agents", "references"] {
                if let Some(value) = data.get(key) {
                    if !value_is_empty(value) {
                        blocks.push(ContentBlock::other(
                            format!("opencode_user_{}", key),
                            value.clone(),
                        ));
                    }
                }
            }
            blocks
        }
        "assistant" => data
            .get("content")
            .and_then(|v| v.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .flat_map(session_message_assistant_part_to_blocks)
                    .collect()
            })
            .unwrap_or_else(|| vec![ContentBlock::other("assistant", data.clone())]),
        "shell" => {
            let call_id = data
                .get("callID")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let output = data.get("output").cloned().unwrap_or(Value::Null);
            vec![ContentBlock::tool_result(call_id, output, false)]
        }
        "synthetic" => data
            .get("text")
            .and_then(|v| v.as_str())
            .map(|text| vec![ContentBlock::text(text)])
            .unwrap_or_else(|| vec![ContentBlock::other(type_tag, data.clone())]),
        "compaction" => data
            .get("summary")
            .and_then(|v| v.as_str())
            .map(|summary| vec![ContentBlock::text(summary)])
            .unwrap_or_else(|| vec![ContentBlock::other(type_tag, data.clone())]),
        other => vec![ContentBlock::other(
            format!("opencode_session_message.{}", other),
            data.clone(),
        )],
    }
}

fn session_message_assistant_part_to_blocks(part: &Value) -> Vec<ContentBlock> {
    match part.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "text" => part
            .get("text")
            .and_then(|v| v.as_str())
            .map(|text| vec![ContentBlock::text(text)])
            .unwrap_or_default(),
        "reasoning" => part
            .get("text")
            .and_then(|v| v.as_str())
            .map(|text| vec![ContentBlock::thinking(text)])
            .unwrap_or_default(),
        "tool" => {
            let call_id = part
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = part
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let state = part.get("state").cloned().unwrap_or(Value::Null);
            let status = state.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let mut blocks = Vec::new();
            if let Some(input) = state.get("input") {
                blocks.push(ContentBlock::tool_use(call_id.clone(), name, input.clone()));
            }
            if matches!(status, "completed" | "error") {
                let output = opencode_v2_tool_state_output(&state);
                blocks.push(ContentBlock::tool_result(
                    call_id,
                    output,
                    status == "error",
                ));
            }
            append_opencode_tool_state_files(&mut blocks, &state);
            blocks
        }
        other => vec![ContentBlock::other(other, part.clone())],
    }
}

fn opencode_v2_tool_state_output(state: &Value) -> Value {
    if let Some(output) = state.get("output").or_else(|| state.get("result")) {
        return output.clone();
    }
    let content = state.get("content").filter(|value| !value_is_empty(value));
    let structured = state
        .get("structured")
        .filter(|value| !value_is_empty(value));
    match (content, structured) {
        (Some(content), Some(structured)) => {
            json!({"content": content.clone(), "structured": structured.clone()})
        }
        (Some(content), None) => content.clone(),
        (None, Some(structured)) => structured.clone(),
        (None, None) => state.get("error").cloned().unwrap_or(Value::Null),
    }
}

fn append_opencode_tool_state_files(blocks: &mut Vec<ContentBlock>, state: &Value) {
    if let Some(attachments) = state.get("attachments").and_then(|value| value.as_array()) {
        for attachment in attachments {
            blocks.push(opencode_fileish_value_to_block(attachment));
        }
    }
    if let Some(content) = state.get("content").and_then(|value| value.as_array()) {
        for item in content {
            if item.get("type").and_then(|value| value.as_str()) == Some("file") {
                blocks.push(opencode_fileish_value_to_block(item));
            }
        }
    }
}

fn opencode_fileish_value_to_block(data: &Value) -> ContentBlock {
    let mut normalized = data.clone();
    if let Some(object) = normalized.as_object_mut() {
        object.entry("type").or_insert_with(|| json!("file"));
        if !object.contains_key("url") {
            if let Some(uri) = data.get("uri").cloned() {
                object.insert("url".into(), uri);
            }
        }
        if !object.contains_key("filename") {
            if let Some(name) = data.get("name").cloned() {
                object.insert("filename".into(), name);
            }
        }
    }
    if let Some(image) = opencode_file_part_to_image(&normalized) {
        return image;
    }
    ContentBlock::Attachment {
        name: normalized
            .get("filename")
            .and_then(|value| value.as_str())
            .map(String::from),
        path: normalized
            .get("url")
            .and_then(|value| value.as_str())
            .map(String::from),
        mime: normalized
            .get("mime")
            .and_then(|value| value.as_str())
            .map(String::from),
        extras: BTreeMap::new(),
    }
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn session_message_raw(row: &SessionMessageRow, data: Value) -> Value {
    json!({
        "session_message": {
            "id": row.id.clone(),
            "session_id": row.session_id.clone(),
            "type": row.type_tag.clone(),
            "time_created": row.time_created,
            "time_updated": row.time_updated,
            "data": data,
        }
    })
}

fn parts_to_blocks(parts: &[&PartRow]) -> Vec<ContentBlock> {
    let mut out: Vec<ContentBlock> = Vec::new();
    for p in parts {
        let data = p.parse_data();
        let kind: String = data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        match kind.as_str() {
            "text" => {
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut extras = BTreeMap::new();
                extras.insert("opencode_part_data".into(), data.clone());
                out.push(ContentBlock::Text {
                    text: text.into(),
                    extras,
                });
            }
            "reasoning" => {
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut extras = BTreeMap::new();
                extras.insert("opencode_part_data".into(), data.clone());
                out.push(ContentBlock::Thinking {
                    text: text.into(),
                    encrypted: None,
                    extras,
                });
            }
            "tool" => {
                let call_id = data
                    .get("callID")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("id").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                let name = data
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let state = data.get("state").cloned().unwrap_or(Value::Null);
                let has_input = state.get("input").is_some()
                    || (state.is_null() && data.get("input").is_some());
                let status_str = state.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let has_output = state.get("output").is_some()
                    || state.get("result").is_some()
                    || (status_str == "error" && state.get("error").is_some());

                // Emit ToolUse if the part carries an input (or a name and no
                // explicit output — i.e. a pending call).
                if has_input || !has_output {
                    let input = state
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| data.get("input").cloned().unwrap_or(Value::Null));
                    out.push(ContentBlock::tool_use(call_id.clone(), name.clone(), input));
                }
                // Emit ToolResult only when this part actually carries output.
                if has_output {
                    let output = state
                        .get("output")
                        .cloned()
                        .or_else(|| state.get("result").cloned())
                        .or_else(|| state.get("error").cloned())
                        .unwrap_or(Value::Null);
                    let is_error = status_str == "error";
                    out.push(ContentBlock::tool_result(call_id, output, is_error));
                }
                append_opencode_tool_state_files(&mut out, &state);
            }
            "file" => {
                if let Some(image) = opencode_file_part_to_image(&data) {
                    out.push(image);
                    continue;
                }
                let name = data
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let path = data.get("url").and_then(|v| v.as_str()).map(String::from);
                let mime = data.get("mime").and_then(|v| v.as_str()).map(String::from);
                out.push(ContentBlock::Attachment {
                    name,
                    path,
                    mime,
                    extras: BTreeMap::new(),
                });
            }
            other => out.push(ContentBlock::other(other, data)),
        }
    }
    out
}

fn opencode_file_part_to_image(data: &Value) -> Option<ContentBlock> {
    let mime = data
        .get("mime")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream");
    if !mime.starts_with("image/") {
        return None;
    }
    if let Some(source) = data
        .get("source")
        .and_then(|v| serde_json::from_value::<ImageSource>(v.clone()).ok())
    {
        let mut extras = BTreeMap::new();
        extras.insert("opencode_part_data".into(), data.clone());
        return Some(ContentBlock::Image {
            mime: mime.to_string(),
            source,
            extras,
        });
    }
    let url = data.get("url").and_then(|v| v.as_str())?;
    let source = if let Some((_, encoded)) = url.split_once(";base64,") {
        if url.starts_with("data:") {
            ImageSource::Base64 {
                data: encoded.to_string(),
            }
        } else {
            return None;
        }
    } else if let Some(path) = url.strip_prefix("file://") {
        ImageSource::LocalPath {
            path: path.to_string(),
        }
    } else if url.starts_with("http://") || url.starts_with("https://") {
        ImageSource::Url {
            url: url.to_string(),
        }
    } else {
        return None;
    };
    let mut extras = BTreeMap::new();
    extras.insert("opencode_part_data".into(), data.clone());
    Some(ContentBlock::Image {
        mime: mime.to_string(),
        source,
        extras,
    })
}

fn extract_model(v: &Value) -> Option<ModelInfo> {
    // model can be:
    //   - object {providerID, modelID, variant}     (message.data.model form)
    //   - object {providerID, id, variant}          (session.model parsed form)
    //   - JSON-stringified object (session.model raw form)
    //   - "openai/gpt-5.5" style string
    if let Some(s) = v.as_str() {
        // Try parsing as a JSON object first (session.model is stored this way).
        if let Ok(inner) = serde_json::from_str::<Value>(s) {
            if inner.is_object() {
                return extract_model(&inner);
            }
        }
        let (prov, model) = s.split_once('/').unwrap_or(("", s));
        return Some(ModelInfo {
            provider_id: if prov.is_empty() {
                None
            } else {
                Some(prov.to_string())
            },
            model_id: model.to_string(),
            variant: None,
        });
    }
    if v.is_object() {
        let model_id = v
            .get("modelID")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("id").and_then(|x| x.as_str()))
            .unwrap_or("")
            .to_string();
        return Some(ModelInfo {
            provider_id: v
                .get("providerID")
                .and_then(|x| x.as_str())
                .map(String::from),
            model_id,
            variant: v.get("variant").and_then(|x| x.as_str()).map(String::from),
        });
    }
    None
}

fn extract_usage(v: &Value) -> Option<Usage> {
    Some(Usage {
        input_tokens: v.get("input").and_then(|x| x.as_u64()),
        output_tokens: v.get("output").and_then(|x| x.as_u64()),
        cached_input_tokens: v
            .get("cache")
            .and_then(|c| c.get("read"))
            .and_then(|x| x.as_u64()),
        reasoning_output_tokens: v.get("reasoning").and_then(|x| x.as_u64()),
        total_tokens: v.get("total").and_then(|x| x.as_u64()),
        cost_usd: None,
    })
}

fn parse_role(s: &str) -> Role {
    match s {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        "developer" => Role::Developer,
        _ => Role::System,
    }
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::System => "system",
        Role::Developer => "developer",
    }
}
