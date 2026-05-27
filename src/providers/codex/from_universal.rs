//! Codex JSONL line → UMessage mapping.
//!
//! Outer types seen: `session_meta`, `turn_context`, `event_msg`, `response_item`.
//! `event_msg.payload.type` inner types: `user_message`, `agent_message`,
//! `token_count`, `task_started`, `task_complete`, `exec_command_end`,
//! `patch_apply_end`, etc.
//! `response_item.payload.type` inner types: `message`, `function_call`,
//! `function_call_output`, `custom_tool_call`, `custom_tool_call_output`,
//! `reasoning`, `image_generation_call`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::debug;
use crate::error::Result;
use crate::ids;
use crate::time;
use crate::universal::{
    ContentBlock, GitInfo, ImageSource, MessageFlags, ModelInfo, Provenance, Provider, Role,
    UMessage, UniversalSession, Usage, SCHEMA_VERSION,
};

use super::CodexReadCtx;

pub fn parse_lines(content: &str, ctx: &CodexReadCtx) -> Result<UniversalSession> {
    let total_lines = content.lines().count();
    debug::log(
        "provider_codex_parse_start",
        serde_json::json!({
            "bytes": content.len(),
            "lines": total_lines,
            "ctx_session_id": ctx.session_id.as_deref(),
            "ctx_cwd": ctx.cwd.as_deref(),
        }),
    );
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
    session.origin.provider = Some(Provider::Codex);

    let mut current_model: Option<ModelInfo> = None;
    let mut idx: u32 = 0;
    let mut tool_error_hints: BTreeMap<String, bool> = BTreeMap::new();
    let mut empty_lines = 0usize;
    let mut invalid_json_lines = 0usize;
    let mut session_meta_lines = 0usize;
    let mut turn_context_lines = 0usize;
    let mut event_msg_lines = 0usize;
    let mut response_item_lines = 0usize;
    let mut unknown_outer_lines = 0usize;

    for (lineno, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            empty_lines = empty_lines.saturating_add(1);
            continue;
        }
        let val: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                invalid_json_lines = invalid_json_lines.saturating_add(1);
                continue;
            }
        };

        let outer_type = val
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let ts = val
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(time::parse_rfc3339);
        if session.created_at.is_none() {
            session.created_at = ts;
        }

        let payload = val.get("payload").cloned().unwrap_or(Value::Null);

        match outer_type.as_str() {
            "session_meta" => {
                session_meta_lines = session_meta_lines.saturating_add(1);
                if let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) {
                    if session.cwd.is_empty() {
                        session.cwd = cwd.to_string();
                    }
                }
                if let Some(g) = payload.get("git") {
                    session.git = Some(GitInfo {
                        branch: g.get("branch").and_then(|v| v.as_str()).map(String::from),
                        commit: g
                            .get("commit_hash")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        origin_url: g
                            .get("repository_url")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    });
                }
                if let Some(v) = payload.get("cli_version").and_then(|v| v.as_str()) {
                    session.origin.cli_version = Some(v.to_string());
                }
                if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                    if session.session_id.is_empty() {
                        session.session_id = id.to_string();
                    }
                }
                session.session_meta = Some(payload.clone());
                // Also emit as a meta system message so the timeline is complete.
                session.messages.push(make_meta_msg(
                    &val,
                    "codex:session_meta",
                    idx,
                    ts,
                    payload.clone(),
                ));
                idx += 1;
            }
            "turn_context" => {
                turn_context_lines = turn_context_lines.saturating_add(1);
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    let variant = payload
                        .get("model_provider")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    current_model = Some(ModelInfo {
                        provider_id: variant,
                        model_id: m.to_string(),
                        variant: payload
                            .get("model_reasoning_effort")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    });
                    if session.model.is_none() {
                        session.model = current_model.clone();
                    }
                }
                session.messages.push(make_meta_msg(
                    &val,
                    "codex:turn_context",
                    idx,
                    ts,
                    payload.clone(),
                ));
                idx += 1;
            }
            "event_msg" => {
                event_msg_lines = event_msg_lines.saturating_add(1);
                idx = handle_event_msg(
                    &mut session,
                    &val,
                    &payload,
                    ts,
                    idx,
                    &current_model,
                    &mut tool_error_hints,
                );
            }
            "response_item" => {
                response_item_lines = response_item_lines.saturating_add(1);
                idx = handle_response_item(
                    &mut session,
                    &val,
                    &payload,
                    ts,
                    idx,
                    &current_model,
                    &mut tool_error_hints,
                );
            }
            _ => {
                unknown_outer_lines = unknown_outer_lines.saturating_add(1);
                // Unknown outer type — preserved as system meta.
                session.messages.push(make_meta_msg(
                    &val,
                    &format!("codex:{}", outer_type),
                    idx,
                    ts,
                    payload.clone(),
                ));
                idx += 1;
            }
        }

        let _ = lineno; // currently unused but useful for error reporting later
    }

    debug::log(
        "provider_codex_parse_ok",
        serde_json::json!({
            "lines": total_lines,
            "empty_lines": empty_lines,
            "invalid_json_lines": invalid_json_lines,
            "session_meta_lines": session_meta_lines,
            "turn_context_lines": turn_context_lines,
            "event_msg_lines": event_msg_lines,
            "response_item_lines": response_item_lines,
            "unknown_outer_lines": unknown_outer_lines,
            "messages": session.messages.len(),
            "session_id_present": !session.session_id.is_empty(),
            "cwd_present": !session.cwd.is_empty(),
            "model_present": session.model.is_some(),
        }),
    );
    Ok(session)
}

fn handle_event_msg(
    session: &mut UniversalSession,
    val: &Value,
    payload: &Value,
    ts: Option<chrono::DateTime<chrono::Utc>>,
    mut idx: u32,
    current_model: &Option<ModelInfo>,
    tool_error_hints: &mut BTreeMap<String, bool>,
) -> u32 {
    let inner = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source = format!("codex:event_msg.{}", inner);
    if let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) {
        if let Some(is_error) = codex_event_tool_is_error(payload) {
            tool_error_hints.insert(call_id.to_string(), is_error);
        }
    }

    match inner.as_str() {
        "user_message" => {
            // Captures images that response_item.message would miss.
            let mut blocks: Vec<ContentBlock> = Vec::new();
            for key in ["images", "local_images"] {
                if let Some(arr) = payload.get(key).and_then(|v| v.as_array()) {
                    for img in arr {
                        if let Some(p) = img.as_str() {
                            let (mime, source) = parse_codex_image_ref(p);
                            blocks.push(ContentBlock::Image {
                                mime,
                                source,
                                extras: BTreeMap::new(),
                            });
                        }
                    }
                }
            }
            // The user text itself is mirrored on response_item.message.
            // Images only appear on event_msg.user_message, so keep image
            // events as visible user content and text-only events as meta.
            let has_image_blocks = !blocks.is_empty();
            let duplicate_response_item_images =
                has_image_blocks && previous_codex_user_response_contains_images(session, &blocks);
            session.messages.push(UMessage {
                id: ids::synth_id(&format!("codex:um:{}", idx)),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role: if has_image_blocks && !duplicate_response_item_images {
                    Role::User
                } else {
                    Role::System
                },
                model: None,
                usage: None,
                stop_reason: None,
                content: if duplicate_response_item_images {
                    Vec::new()
                } else {
                    blocks
                },
                flags: MessageFlags {
                    is_meta: !has_image_blocks || duplicate_response_item_images,
                    ..Default::default()
                },
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        "agent_message" => {
            session
                .messages
                .push(make_meta_msg(val, &source, idx, ts, payload.clone()));
            idx += 1;
        }
        "token_count" => {
            let usage = extract_usage_from_token_count(payload);
            session.usage_total = usage.clone().or(session.usage_total.take());
            let mut m = make_meta_msg(val, &source, idx, ts, payload.clone());
            m.usage = usage;
            session.messages.push(m);
            idx += 1;
        }
        "image_generation_end" => {
            if let Some(image) = codex_generated_image_block(payload) {
                session.messages.push(UMessage {
                    id: payload
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| ids::synth_id(&format!("codex:image:{}", idx))),
                    parent_id: None,
                    index: idx,
                    timestamp: ts,
                    role: Role::Assistant,
                    model: None,
                    usage: None,
                    stop_reason: None,
                    content: vec![image],
                    flags: MessageFlags::default(),
                    provenance: Provenance {
                        source_event_type: source,
                        raw: val.clone(),
                    },
                    extras: BTreeMap::new(),
                });
                idx += 1;
            } else {
                session
                    .messages
                    .push(make_meta_msg(val, &source, idx, ts, payload.clone()));
                idx += 1;
            }
        }
        "raw_response_item" => {
            if let Some(item) = payload.get("item").filter(|value| value.is_object()) {
                idx = handle_response_item(
                    session,
                    val,
                    item,
                    ts,
                    idx,
                    current_model,
                    tool_error_hints,
                );
            } else {
                session
                    .messages
                    .push(make_meta_msg(val, &source, idx, ts, payload.clone()));
                idx += 1;
            }
        }
        _ => {
            session
                .messages
                .push(make_meta_msg(val, &source, idx, ts, payload.clone()));
            idx += 1;
        }
    }
    idx
}

fn codex_generated_image_block(payload: &Value) -> Option<ContentBlock> {
    let source = if let Some(data) = payload
        .get("result")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        ImageSource::Base64 {
            data: data.to_string(),
        }
    } else if let Some(path) = payload
        .get("saved_path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        ImageSource::LocalPath {
            path: path.to_string(),
        }
    } else if let Some(url) = payload
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        ImageSource::Url {
            url: url.to_string(),
        }
    } else {
        return None;
    };
    let mut extras: BTreeMap<String, Value> = BTreeMap::new();
    for key in [
        "id",
        "call_id",
        "status",
        "revised_prompt",
        "saved_path",
        "url",
    ] {
        if let Some(value) = payload.get(key) {
            extras.insert(key.to_string(), value.clone());
        }
    }
    let mime = payload
        .get("mime")
        .and_then(|v| v.as_str())
        .unwrap_or("image/png")
        .to_string();
    Some(ContentBlock::Image {
        mime,
        source,
        extras,
    })
}

fn handle_response_item(
    session: &mut UniversalSession,
    val: &Value,
    payload: &Value,
    ts: Option<chrono::DateTime<chrono::Utc>>,
    mut idx: u32,
    current_model: &Option<ModelInfo>,
    tool_error_hints: &mut BTreeMap<String, bool>,
) -> u32 {
    let inner = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source = format!("codex:response_item.{}", inner);

    match inner.as_str() {
        "message" => {
            let role_str = payload
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("assistant")
                .to_string();
            let role = parse_role(&role_str);
            let blocks = message_content(
                payload.get("content"),
                payload.get("phase").and_then(|v| v.as_str()),
                payload.get("id").and_then(|v| v.as_str()),
            );
            if role == Role::User {
                dedupe_previous_codex_user_image_event(session, &blocks);
            }
            session.messages.push(UMessage {
                id: payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| ids::synth_id(&format!("codex:rim:{}", idx))),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role,
                model: current_model.clone(),
                usage: None,
                stop_reason: None,
                content: blocks,
                flags: MessageFlags::default(),
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        "local_shell_call" => {
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("id").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let input = payload.get("action").cloned().unwrap_or(Value::Null);
            let extras = codex_response_item_extras(inner.as_str(), payload, &["status"]);
            session.messages.push(UMessage {
                id: payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| ids::synth_id(&format!("codex:tu:{}", idx))),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role: Role::Assistant,
                model: current_model.clone(),
                usage: None,
                stop_reason: None,
                content: vec![ContentBlock::ToolUse {
                    call_id,
                    name: "local_shell".into(),
                    input,
                    extras,
                }],
                flags: MessageFlags::default(),
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        "tool_search_call" => {
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("id").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let input = payload.get("arguments").cloned().unwrap_or(Value::Null);
            let extras =
                codex_response_item_extras(inner.as_str(), payload, &["status", "execution"]);
            session.messages.push(UMessage {
                id: payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| ids::synth_id(&format!("codex:tu:{}", idx))),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role: Role::Assistant,
                model: current_model.clone(),
                usage: None,
                stop_reason: None,
                content: vec![ContentBlock::ToolUse {
                    call_id,
                    name: "tool_search".into(),
                    input,
                    extras,
                }],
                flags: MessageFlags::default(),
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        "web_search_call" => {
            let call_id = payload
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input = payload.get("action").cloned().unwrap_or(Value::Null);
            let extras = codex_response_item_extras(inner.as_str(), payload, &["status"]);
            session.messages.push(UMessage {
                id: payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| ids::synth_id(&format!("codex:tu:{}", idx))),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role: Role::Assistant,
                model: current_model.clone(),
                usage: None,
                stop_reason: None,
                content: vec![ContentBlock::ToolUse {
                    call_id,
                    name: "web_search".into(),
                    input,
                    extras,
                }],
                flags: MessageFlags::default(),
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        "function_call" | "custom_tool_call" => {
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // function_call.arguments is a JSON-encoded STRING (per OpenAI Responses API).
            // custom_tool_call.input is a plain string OR object.
            let input = if inner == "function_call" {
                match payload.get("arguments").and_then(|v| v.as_str()) {
                    Some(s) => serde_json::from_str::<Value>(s).unwrap_or(Value::String(s.into())),
                    None => payload.get("arguments").cloned().unwrap_or(Value::Null),
                }
            } else {
                payload.get("input").cloned().unwrap_or(Value::Null)
            };
            let extras =
                codex_response_item_extras(inner.as_str(), payload, &["namespace", "status"]);
            session.messages.push(UMessage {
                id: payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| ids::synth_id(&format!("codex:tu:{}", idx))),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role: Role::Assistant,
                model: current_model.clone(),
                usage: None,
                stop_reason: None,
                content: vec![ContentBlock::ToolUse {
                    call_id,
                    name,
                    input,
                    extras,
                }],
                flags: MessageFlags::default(),
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let output = payload.get("output").cloned().unwrap_or(Value::Null);
            let is_error = payload
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| {
                    tool_error_hints
                        .remove(&call_id)
                        .unwrap_or_else(|| codex_output_looks_failed(&output))
                });
            let mut extras: BTreeMap<String, Value> = BTreeMap::new();
            extras.insert("codex_response_item_type".into(), json!(inner.as_str()));
            if output.is_array() {
                extras.insert("codex_output_content_items".into(), Value::Bool(true));
            }
            if let Some(name) = payload.get("name") {
                extras.insert("name".into(), name.clone());
            }
            session.messages.push(UMessage {
                id: ids::synth_id(&format!("codex:tr:{}", idx)),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role: Role::Tool,
                model: None,
                usage: None,
                stop_reason: None,
                content: vec![ContentBlock::ToolResult {
                    call_id,
                    output,
                    is_error,
                    extras,
                }],
                flags: MessageFlags::default(),
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        "tool_search_output" => {
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let output = payload.get("tools").cloned().unwrap_or(Value::Null);
            let extras =
                codex_response_item_extras(inner.as_str(), payload, &["status", "execution"]);
            session.messages.push(UMessage {
                id: ids::synth_id(&format!("codex:tr:{}", idx)),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role: Role::Tool,
                model: None,
                usage: None,
                stop_reason: None,
                content: vec![ContentBlock::ToolResult {
                    call_id,
                    output,
                    is_error: false,
                    extras,
                }],
                flags: MessageFlags::default(),
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        "image_generation_call" => {
            if let Some(image) = codex_generated_image_block(payload) {
                session.messages.push(UMessage {
                    id: payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .or_else(|| payload.get("call_id").and_then(|v| v.as_str()))
                        .map(String::from)
                        .unwrap_or_else(|| ids::synth_id(&format!("codex:image:{}", idx))),
                    parent_id: None,
                    index: idx,
                    timestamp: ts,
                    role: Role::Assistant,
                    model: current_model.clone(),
                    usage: None,
                    stop_reason: None,
                    content: vec![image],
                    flags: MessageFlags::default(),
                    provenance: Provenance {
                        source_event_type: source,
                        raw: val.clone(),
                    },
                    extras: BTreeMap::new(),
                });
                idx += 1;
            } else {
                session
                    .messages
                    .push(make_meta_msg(val, &source, idx, ts, payload.clone()));
                idx += 1;
            }
        }
        "reasoning" => {
            let summary = payload
                .get("summary")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let encrypted = payload
                .get("encrypted_content")
                .and_then(|v| v.as_str())
                .map(String::from);
            let mut extras: BTreeMap<String, Value> = BTreeMap::new();
            if let Some(c) = payload.get("content") {
                extras.insert("content".into(), c.clone());
            }
            if let Some(item_id) = payload
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|value| value.starts_with("rs_"))
            {
                extras.insert("codex_item_id".into(), Value::String(item_id.to_string()));
            }
            session.messages.push(UMessage {
                id: payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| ids::synth_id(&format!("codex:re:{}", idx))),
                parent_id: None,
                index: idx,
                timestamp: ts,
                role: Role::Assistant,
                model: current_model.clone(),
                usage: None,
                stop_reason: None,
                content: vec![ContentBlock::Thinking {
                    text: summary,
                    encrypted,
                    extras,
                }],
                flags: MessageFlags::default(),
                provenance: Provenance {
                    source_event_type: source,
                    raw: val.clone(),
                },
                extras: BTreeMap::new(),
            });
            idx += 1;
        }
        _ => {
            session
                .messages
                .push(make_meta_msg(val, &source, idx, ts, payload.clone()));
            idx += 1;
        }
    }
    idx
}

fn codex_response_item_extras(
    inner: &str,
    payload: &Value,
    keys: &[&str],
) -> BTreeMap<String, Value> {
    let mut extras: BTreeMap<String, Value> = BTreeMap::new();
    extras.insert("codex_response_item_type".into(), json!(inner));
    for key in keys {
        if let Some(value) = payload.get(*key) {
            extras.insert((*key).to_string(), value.clone());
        }
    }
    extras
}

fn codex_event_tool_is_error(payload: &Value) -> Option<bool> {
    if let Some(success) = payload.get("success").and_then(|v| v.as_bool()) {
        return Some(!success);
    }
    if let Some(status) = payload.get("status").and_then(|v| v.as_str()) {
        return match status {
            "failed" | "error" | "cancelled" | "canceled" => Some(true),
            "completed" | "success" | "succeeded" => Some(false),
            _ => None,
        };
    }
    if let Some(ok) = payload.get("result").and_then(|v| v.get("Ok")) {
        return ok.get("isError").and_then(|v| v.as_bool());
    }
    if payload.get("result").and_then(|v| v.get("Err")).is_some() {
        return Some(true);
    }
    None
}

fn codex_output_looks_failed(output: &Value) -> bool {
    let text = match output {
        Value::String(s) => s.as_str(),
        _ => return false,
    };
    for line in text.lines() {
        if line
            .strip_prefix("Exit code: ")
            .and_then(parse_nonzero_exit_code)
            .unwrap_or(false)
        {
            return true;
        }
        if let Some((_, rest)) = line.split_once("Process exited with code ") {
            if parse_nonzero_exit_code(rest).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

fn parse_nonzero_exit_code(s: &str) -> Option<bool> {
    let digits: String = s
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<i32>().ok().map(|code| code != 0)
}

fn parse_codex_image_ref(reference: &str) -> (String, ImageSource) {
    if let Some((mime, data)) = parse_data_image_url(reference) {
        return (mime, ImageSource::Base64 { data });
    }
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return (
            infer_image_mime_from_ref(reference).into(),
            ImageSource::Url {
                url: reference.into(),
            },
        );
    }
    (
        infer_image_mime_from_ref(reference).into(),
        ImageSource::LocalPath {
            path: reference.into(),
        },
    )
}

fn infer_image_mime_from_ref(reference: &str) -> &'static str {
    let clean = reference
        .split(['?', '#'])
        .next()
        .unwrap_or(reference)
        .to_ascii_lowercase();
    if clean.ends_with(".jpg") || clean.ends_with(".jpeg") {
        "image/jpeg"
    } else if clean.ends_with(".gif") {
        "image/gif"
    } else if clean.ends_with(".webp") {
        "image/webp"
    } else if clean.ends_with(".bmp") {
        "image/bmp"
    } else if clean.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "image/png"
    }
}

fn parse_data_image_url(reference: &str) -> Option<(String, String)> {
    let rest = reference.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    Some((mime.to_string(), data.to_string()))
}

fn message_content(
    content: Option<&Value>,
    phase: Option<&str>,
    item_id: Option<&str>,
) -> Vec<ContentBlock> {
    let Some(arr) = content.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let t = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match t {
            "input_text" | "output_text" => {
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut extras: BTreeMap<String, Value> = BTreeMap::new();
                extras.insert("codex_block".into(), json!(t));
                if t == "output_text" {
                    if let Some(phase) = phase.filter(|value| !value.trim().is_empty()) {
                        extras.insert("codex_phase".into(), Value::String(phase.to_string()));
                    }
                    if let Some(item_id) = item_id.filter(|value| value.starts_with("msg_")) {
                        extras.insert("codex_item_id".into(), Value::String(item_id.to_string()));
                    }
                }
                out.push(ContentBlock::Text {
                    text: text.into(),
                    extras,
                });
            }
            "input_image" => {
                if let Some(url) = item.get("image_url").and_then(|v| v.as_str()) {
                    let (mime, source) = parse_codex_image_ref(url);
                    let mut extras: BTreeMap<String, Value> = BTreeMap::new();
                    extras.insert("codex_block".into(), json!(t));
                    if let Some(detail) = item.get("detail") {
                        extras.insert("codex_image_detail".into(), detail.clone());
                    }
                    out.push(ContentBlock::Image {
                        mime,
                        source,
                        extras,
                    });
                }
            }
            other => out.push(ContentBlock::other(other, item.clone())),
        }
    }
    out
}

fn dedupe_previous_codex_user_image_event(session: &mut UniversalSession, blocks: &[ContentBlock]) {
    let image_keys = blocks
        .iter()
        .filter_map(content_block_image_key)
        .collect::<std::collections::BTreeSet<_>>();
    if image_keys.is_empty() {
        return;
    }
    let Some(previous) = session.messages.last() else {
        return;
    };
    if previous.provenance.source_event_type != "codex:event_msg.user_message" {
        return;
    }
    if previous
        .content
        .iter()
        .any(|block| !matches!(block, ContentBlock::Image { .. }))
    {
        return;
    }
    let previous_keys = previous
        .content
        .iter()
        .filter_map(content_block_image_key)
        .collect::<std::collections::BTreeSet<_>>();
    if !previous_keys.is_empty() && previous_keys.is_subset(&image_keys) {
        session.messages.pop();
    }
}

fn previous_codex_user_response_contains_images(
    session: &UniversalSession,
    blocks: &[ContentBlock],
) -> bool {
    let image_keys = blocks
        .iter()
        .filter_map(content_block_image_key)
        .collect::<std::collections::BTreeSet<_>>();
    if image_keys.is_empty() {
        return false;
    }
    let Some(previous) = session.messages.last() else {
        return false;
    };
    if previous.provenance.source_event_type != "codex:response_item.message" {
        return false;
    }
    if previous.role != Role::User {
        return false;
    }
    let previous_keys = previous
        .content
        .iter()
        .filter_map(content_block_image_key)
        .collect::<std::collections::BTreeSet<_>>();
    !previous_keys.is_empty() && image_keys.is_subset(&previous_keys)
}

fn content_block_image_key(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Image { source, .. } => Some(match source {
            ImageSource::Base64 { data } => format!("base64:{data}"),
            ImageSource::Url { url } => format!("url:{url}"),
            ImageSource::LocalPath { path } => format!("path:{path}"),
        }),
        _ => None,
    }
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

fn make_meta_msg(
    raw: &Value,
    source: &str,
    idx: u32,
    ts: Option<chrono::DateTime<chrono::Utc>>,
    payload: Value,
) -> UMessage {
    UMessage {
        id: ids::synth_id(&format!("{}:{}", source, idx)),
        parent_id: None,
        index: idx,
        timestamp: ts,
        role: Role::System,
        model: None,
        usage: None,
        stop_reason: None,
        content: vec![ContentBlock::other(source, payload)],
        flags: MessageFlags {
            is_meta: true,
            is_compaction: codex_source_is_compaction(source),
            ..Default::default()
        },
        provenance: Provenance {
            source_event_type: source.to_string(),
            raw: raw.clone(),
        },
        extras: BTreeMap::new(),
    }
}

fn codex_source_is_compaction(source: &str) -> bool {
    source.contains("compaction") || source.contains("compacted")
}

fn extract_usage_from_token_count(payload: &Value) -> Option<Usage> {
    let info = payload.get("info")?;
    let last = info
        .get("last_token_usage")
        .or_else(|| info.get("total_token_usage"))?;
    Some(Usage {
        input_tokens: last.get("input_tokens").and_then(|v| v.as_u64()),
        output_tokens: last.get("output_tokens").and_then(|v| v.as_u64()),
        cached_input_tokens: last.get("cached_input_tokens").and_then(|v| v.as_u64()),
        reasoning_output_tokens: last.get("reasoning_output_tokens").and_then(|v| v.as_u64()),
        total_tokens: last.get("total_tokens").and_then(|v| v.as_u64()),
        cost_usd: None,
    })
}
