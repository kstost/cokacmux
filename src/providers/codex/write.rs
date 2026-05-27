//! UniversalSession → Codex rollout JSONL.

use std::io::Write;
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::debug;
use crate::error::Result;
use crate::time::to_rfc3339_ms;
use crate::universal::{ContentBlock, ImageSource, Role, UMessage, UniversalSession};

use super::CodexWriteOpts;

pub fn to_jsonl_path(session: &UniversalSession, path: &Path, opts: &CodexWriteOpts) -> Result<()> {
    debug::log(
        "provider_codex_write_file_start",
        serde_json::json!({
            "path": path.display().to_string(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "replay_raw": opts.replay_raw,
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
        "provider_codex_write_file_ok",
        serde_json::json!({
            "path": path.display().to_string(),
            "session_id": &session.session_id,
            "bytes": s.len(),
            "lines": s.lines().count(),
        }),
    );
    Ok(())
}

pub fn to_jsonl_string(session: &UniversalSession, opts: &CodexWriteOpts) -> Result<String> {
    debug::log(
        "provider_codex_write_string_start",
        serde_json::json!({
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "replay_raw": opts.replay_raw,
        }),
    );
    let mut out = String::new();
    let mut replayed_raw_messages = 0usize;
    let mut synthesized_messages = 0usize;
    let replay_codex_raw = opts.replay_raw
        && session
            .messages
            .iter()
            .any(|m| m.provenance.source_event_type.starts_with("codex:"));
    let turn_id = uuid::Uuid::now_v7().to_string();
    let scaffold_ts = session
        .created_at
        .map(to_rfc3339_ms)
        .unwrap_or_else(|| to_rfc3339_ms(chrono::Utc::now()));

    // 1) If a session_meta is present in the source and the session originated
    //    from codex, replay it. Otherwise synthesize a minimal one.
    let mut wrote_session_meta = false;
    if opts.replay_raw {
        for m in &session.messages {
            if m.provenance.source_event_type == "codex:session_meta" {
                let s = serde_json::to_string(&m.provenance.raw)?;
                out.push_str(&s);
                out.push('\n');
                wrote_session_meta = true;
                replayed_raw_messages = replayed_raw_messages.saturating_add(1);
                break;
            }
        }
    }
    if !wrote_session_meta {
        out.push_str(&serde_json::to_string(&synth_session_meta(session))?);
        out.push('\n');
    }

    if !replay_codex_raw {
        out.push_str(&serde_json::to_string(&synth_task_started(
            &scaffold_ts,
            &turn_id,
            session,
        ))?);
        out.push('\n');
        out.push_str(&serde_json::to_string(&synth_turn_context(
            &scaffold_ts,
            &turn_id,
            session,
        ))?);
        out.push('\n');
    }

    // 2) Replay or synthesize each message.
    for m in &session.messages {
        // Skip session_meta — already emitted above.
        if m.provenance.source_event_type == "codex:session_meta" {
            continue;
        }

        if opts.replay_raw && m.provenance.source_event_type.starts_with("codex:") {
            let line = serde_json::to_string(&m.provenance.raw)?;
            out.push_str(&line);
            out.push('\n');
            replayed_raw_messages = replayed_raw_messages.saturating_add(1);
            continue;
        }

        if should_skip_foreign_runtime_context(m) {
            continue;
        }

        for value in synthesize_lines(session, m) {
            let line = serde_json::to_string(&value)?;
            out.push_str(&line);
            out.push('\n');
        }
        synthesized_messages = synthesized_messages.saturating_add(1);
    }

    if !replay_codex_raw {
        out.push_str(&serde_json::to_string(&synth_token_count(
            &scaffold_ts,
            session,
        ))?);
        out.push('\n');
        out.push_str(&serde_json::to_string(&synth_task_complete(
            &scaffold_ts,
            &turn_id,
            session,
        ))?);
        out.push('\n');
    }
    debug::log(
        "provider_codex_write_string_ok",
        serde_json::json!({
            "session_id": &session.session_id,
            "bytes": out.len(),
            "lines": out.lines().count(),
            "replayed_raw_messages": replayed_raw_messages,
            "synthesized_messages": synthesized_messages,
        }),
    );
    Ok(out)
}

fn synth_session_meta(session: &UniversalSession) -> Value {
    let ts = session
        .created_at
        .map(to_rfc3339_ms)
        .unwrap_or_else(|| to_rfc3339_ms(chrono::Utc::now()));
    let mut payload = Map::new();
    payload.insert("id".into(), Value::String(session.session_id.clone()));
    payload.insert("timestamp".into(), Value::String(ts.clone()));
    payload.insert("cwd".into(), Value::String(session.cwd.clone()));
    payload.insert("originator".into(), Value::String("codex_exec".into()));
    payload.insert(
        "cli_version".into(),
        Value::String(env!("CARGO_PKG_VERSION").into()),
    );
    payload.insert("source".into(), Value::String("exec".into()));
    payload.insert(
        "model_provider".into(),
        Value::String(
            session
                .model
                .as_ref()
                .and_then(|m| m.provider_id.clone())
                .unwrap_or_else(|| "openai".into()),
        ),
    );
    payload.insert(
        "base_instructions".into(),
        json!({"text": "You are Codex, a coding agent based on GPT-5."}),
    );
    if let Some(g) = &session.git {
        let mut gmap = Map::new();
        if let Some(b) = &g.branch {
            gmap.insert("branch".into(), Value::String(b.clone()));
        }
        if let Some(c) = &g.commit {
            gmap.insert("commit_hash".into(), Value::String(c.clone()));
        }
        if let Some(u) = &g.origin_url {
            gmap.insert("repository_url".into(), Value::String(u.clone()));
        }
        payload.insert("git".into(), Value::Object(gmap));
    }
    json!({
        "timestamp": ts,
        "type": "session_meta",
        "payload": Value::Object(payload),
    })
}

fn synth_task_started(ts: &str, turn_id: &str, _session: &UniversalSession) -> Value {
    json!({
        "timestamp": ts,
        "type": "event_msg",
        "payload": {
            "type": "task_started",
            "turn_id": turn_id,
            "started_at": chrono::Utc::now().timestamp(),
            "model_context_window": 258400,
            "collaboration_mode_kind": "default",
        },
    })
}

fn synth_turn_context(ts: &str, turn_id: &str, session: &UniversalSession) -> Value {
    let model = session
        .model
        .as_ref()
        .map(|m| m.model_id.as_str())
        .filter(|id| !id.trim().is_empty())
        .unwrap_or("gpt-5.5");
    let effort = session
        .model
        .as_ref()
        .and_then(|m| m.variant.as_deref())
        .filter(|v| !v.trim().is_empty())
        .filter(|v| *v != "default")
        .unwrap_or("medium");
    json!({
        "timestamp": ts,
        "type": "turn_context",
        "payload": {
            "turn_id": turn_id,
            "cwd": session.cwd.as_str(),
            "current_date": chrono::Utc::now().format("%Y-%m-%d").to_string(),
            "timezone": "Etc/UTC",
            "approval_policy": "never",
            "sandbox_policy": {"type": "read-only"},
            "permission_profile": {
                "type": "managed",
                "file_system": {
                    "type": "restricted",
                    "entries": [{"path": {"type": "special", "value": {"kind": "root"}}, "access": "read"}],
                },
                "network": "restricted",
            },
            "model": model,
            "personality": "pragmatic",
            "collaboration_mode": {
                "mode": "default",
                "settings": {
                    "model": model,
                    "reasoning_effort": effort,
                    "developer_instructions": null,
                },
            },
            "realtime_active": false,
            "effort": effort,
            "summary": "none",
            "truncation_policy": {"mode": "tokens", "limit": 10000},
        },
    })
}

fn synth_token_count(ts: &str, session: &UniversalSession) -> Value {
    let usage = session.usage_total.as_ref();
    let input = usage.and_then(|u| u.input_tokens).unwrap_or(0);
    let cached = usage.and_then(|u| u.cached_input_tokens).unwrap_or(0);
    let output = usage.and_then(|u| u.output_tokens).unwrap_or(0);
    let reasoning = usage.and_then(|u| u.reasoning_output_tokens).unwrap_or(0);
    let total = usage
        .and_then(|u| u.total_tokens)
        .unwrap_or(input + output + reasoning);
    let token_usage = json!({
        "input_tokens": input,
        "cached_input_tokens": cached,
        "output_tokens": output,
        "reasoning_output_tokens": reasoning,
        "total_tokens": total,
    });
    json!({
        "timestamp": ts,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": token_usage.clone(),
                "last_token_usage": token_usage,
                "model_context_window": 258400,
            },
            "rate_limits": {
                "limit_id": "codex",
                "limit_name": null,
                "primary": {"used_percent": 0.0, "window_minutes": 300, "resets_at": null},
                "secondary": {"used_percent": 0.0, "window_minutes": 10080, "resets_at": null},
                "credits": null,
                "plan_type": null,
                "rate_limit_reached_type": null,
            },
        },
    })
}

fn synth_task_complete(ts: &str, turn_id: &str, session: &UniversalSession) -> Value {
    json!({
        "timestamp": ts,
        "type": "event_msg",
        "payload": {
            "type": "task_complete",
            "turn_id": turn_id,
            "last_agent_message": last_assistant_text(session).unwrap_or_default(),
            "completed_at": chrono::Utc::now().timestamp(),
            "duration_ms": 0,
            "time_to_first_token_ms": 0,
        },
    })
}

fn synthesize_lines(session: &UniversalSession, m: &UMessage) -> Vec<Value> {
    let ts = synthesized_timestamp(session, m);

    // Decide which codex shape best fits this message.
    // - role User/Assistant/Developer with Text → response_item.message
    // - assistant message with ToolUse → response_item.function_call
    // - tool message with ToolResult → response_item.function_call_output
    // - assistant message with Thinking → response_item.reasoning
    // - unsupported provider meta is skipped; Codex resume rejects unknown
    //   event_msg payload types.

    if matches!(m.role, Role::User | Role::Assistant | Role::Developer) {
        let role_str = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Developer => "developer",
            _ => "user",
        };
        let block_type = if matches!(m.role, Role::Assistant) {
            "output_text"
        } else {
            "input_text"
        };
        let mut content: Vec<Value> = Vec::new();
        let mut text_parts: Vec<String> = Vec::new();
        let mut images: Vec<String> = Vec::new();
        let mut local_images: Vec<String> = Vec::new();
        let mut lines = Vec::new();
        if matches!(m.role, Role::Assistant)
            && !m
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Thinking { .. }))
            && m.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { .. }
                        | ContentBlock::ToolUse { .. }
                        | ContentBlock::Image { .. }
                )
            })
        {
            lines.push(json!({
                "timestamp": ts,
                "type": "response_item",
                "payload": {
                    "type": "reasoning",
                    "summary": [],
                    "content": null,
                },
            }));
        }
        for b in &m.content {
            match b {
                ContentBlock::Text { text, .. } => {
                    text_parts.push(text.clone());
                    content.push(json!({"type": block_type, "text": text}));
                }
                ContentBlock::Thinking {
                    text, encrypted, ..
                } => {
                    push_codex_text_lines(
                        &mut lines,
                        &ts,
                        m.role,
                        role_str,
                        &mut content,
                        &mut text_parts,
                        &mut images,
                        &mut local_images,
                    );
                    let mut p = json!({
                        "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": text}],
                        "content": null,
                    });
                    if let Some(enc) = encrypted {
                        p["encrypted_content"] = Value::String(enc.clone());
                    }
                    lines.push(json!({
                        "timestamp": ts,
                        "type": "response_item",
                        "payload": p,
                    }));
                }
                ContentBlock::ToolUse {
                    call_id,
                    name,
                    input,
                    extras,
                    ..
                } => {
                    push_codex_text_lines(
                        &mut lines,
                        &ts,
                        m.role,
                        role_str,
                        &mut content,
                        &mut text_parts,
                        &mut images,
                        &mut local_images,
                    );
                    let payload = codex_tool_use_payload(name, call_id, input, extras);
                    lines.push(json!({
                        "timestamp": ts,
                        "type": "response_item",
                        "payload": payload,
                    }));
                }
                ContentBlock::ToolResult {
                    call_id,
                    output,
                    is_error,
                    extras,
                    ..
                } => {
                    push_codex_text_lines(
                        &mut lines,
                        &ts,
                        m.role,
                        role_str,
                        &mut content,
                        &mut text_parts,
                        &mut images,
                        &mut local_images,
                    );
                    let payload = codex_tool_result_payload(call_id, output, *is_error, extras);
                    lines.push(json!({
                        "timestamp": ts,
                        "type": "response_item",
                        "payload": payload,
                    }));
                }
                ContentBlock::Image {
                    mime,
                    source,
                    extras,
                    ..
                } => {
                    if matches!(m.role, Role::User) {
                        let mut item = json!({
                            "type": "input_image",
                            "image_url": codex_image_url(mime, source),
                        });
                        if let Some(detail) = extras.get("codex_image_detail") {
                            item["detail"] = detail.clone();
                        }
                        content.push(item);
                        match source {
                            ImageSource::LocalPath { path } => local_images.push(path.clone()),
                            ImageSource::Url { url } => images.push(url.clone()),
                            ImageSource::Base64 { data } => {
                                images.push(format!("data:{};base64,{}", mime, data));
                            }
                        }
                    } else if matches!(m.role, Role::Assistant) {
                        push_codex_text_lines(
                            &mut lines,
                            &ts,
                            m.role,
                            role_str,
                            &mut content,
                            &mut text_parts,
                            &mut images,
                            &mut local_images,
                        );
                        if let Some(item) = synth_image_generation_call(&ts, source, extras) {
                            lines.push(item);
                        } else if let Some(event) = synth_image_generation_end(&ts, mime, source) {
                            lines.push(event);
                        }
                    }
                }
                _ => {}
            }
        }
        push_codex_text_lines(
            &mut lines,
            &ts,
            m.role,
            role_str,
            &mut content,
            &mut text_parts,
            &mut images,
            &mut local_images,
        );
        return lines;
    }

    if matches!(m.role, Role::Tool) {
        let mut lines = Vec::new();
        for b in &m.content {
            if let ContentBlock::ToolResult {
                call_id,
                output,
                is_error,
                extras,
                ..
            } = b
            {
                let payload = codex_tool_result_payload(call_id, output, *is_error, extras);
                lines.push(json!({
                    "timestamp": ts,
                    "type": "response_item",
                    "payload": payload,
                }));
            }
        }
        return lines;
    }

    Vec::new()
}

fn codex_tool_use_payload(
    name: &str,
    call_id: &str,
    input: &Value,
    extras: &std::collections::BTreeMap<String, Value>,
) -> Value {
    let source_type = extras
        .get("codex_response_item_type")
        .and_then(|value| value.as_str());
    if source_type == Some("local_shell_call") {
        let mut payload = json!({
            "type": "local_shell_call",
            "status": codex_extra_str(extras, "status").unwrap_or("completed"),
            "action": input,
        });
        if !call_id.trim().is_empty() {
            payload["call_id"] = Value::String(call_id.to_string());
        }
        return payload;
    }
    if source_type == Some("tool_search_call") {
        let mut payload = json!({
            "type": "tool_search_call",
            "execution": codex_extra_str(extras, "execution").unwrap_or(""),
            "arguments": input,
        });
        if !call_id.trim().is_empty() {
            payload["call_id"] = Value::String(call_id.to_string());
        }
        if let Some(status) = extras.get("status") {
            payload["status"] = status.clone();
        }
        return payload;
    }
    if source_type == Some("web_search_call") {
        let mut payload = json!({
            "type": "web_search_call",
            "action": input,
        });
        if let Some(status) = extras.get("status") {
            payload["status"] = status.clone();
        }
        return payload;
    }
    if source_type == Some("custom_tool_call") {
        let mut payload = json!({
            "type": "custom_tool_call",
            "call_id": call_id,
            "name": name,
            "input": input
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| input.to_string()),
        });
        if let Some(status) = extras.get("status") {
            payload["status"] = status.clone();
        }
        return payload;
    }

    let mut payload = json!({
        "type": "function_call",
        "name": name,
        "call_id": call_id,
        "arguments": serde_json::to_string(input).unwrap_or_default(),
    });
    if let Some(namespace) = extras.get("namespace") {
        payload["namespace"] = namespace.clone();
    }
    payload
}

fn codex_tool_result_payload(
    call_id: &str,
    output: &Value,
    is_error: bool,
    extras: &std::collections::BTreeMap<String, Value>,
) -> Value {
    if extras
        .get("codex_response_item_type")
        .and_then(|value| value.as_str())
        == Some("tool_search_output")
    {
        let mut payload = json!({
            "type": "tool_search_output",
            "status": codex_extra_str(extras, "status").unwrap_or("completed"),
            "execution": codex_extra_str(extras, "execution").unwrap_or(""),
            "tools": codex_tool_search_tools_value(output),
        });
        if !call_id.trim().is_empty() {
            payload["call_id"] = Value::String(call_id.to_string());
        }
        return payload;
    }

    let is_custom = extras
        .get("codex_response_item_type")
        .and_then(|value| value.as_str())
        == Some("custom_tool_call_output");
    let mut payload = json!({
        "type": if is_custom {
            "custom_tool_call_output"
        } else {
            "function_call_output"
        },
        "call_id": call_id,
        "output": codex_tool_output_value(output, extras),
    });
    if is_custom {
        if let Some(name) = extras.get("name") {
            payload["name"] = name.clone();
        }
    }
    if is_error {
        payload["is_error"] = Value::Bool(true);
    }
    payload
}

fn codex_extra_str<'a>(
    extras: &'a std::collections::BTreeMap<String, Value>,
    key: &str,
) -> Option<&'a str> {
    extras
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

fn codex_tool_search_tools_value(output: &Value) -> Value {
    if output.is_array() {
        output.clone()
    } else if let Some(tools) = output.get("tools") {
        tools.clone()
    } else {
        Value::Array(Vec::new())
    }
}

fn codex_tool_output_value(
    output: &Value,
    extras: &std::collections::BTreeMap<String, Value>,
) -> Value {
    if extras
        .get("codex_output_content_items")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        && output.is_array()
    {
        return output.clone();
    }
    Value::String(match output {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn should_skip_foreign_runtime_context(message: &UMessage) -> bool {
    if message.flags.is_meta {
        return true;
    }
    if matches!(message.role, Role::System | Role::Developer) {
        return message.provenance.source_event_type.starts_with("codex:")
            || message.provenance.source_event_type.starts_with("claude:");
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

fn last_assistant_text(session: &UniversalSession) -> Option<String> {
    session.messages.iter().rev().find_map(|message| {
        if message.role != Role::Assistant {
            return None;
        }
        let text = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

fn synth_image_generation_end(ts: &str, mime: &str, source: &ImageSource) -> Option<Value> {
    let call_id = format!("ig_{}", uuid::Uuid::now_v7().simple());
    match source {
        ImageSource::Base64 { data } => Some(json!({
            "timestamp": ts,
            "type": "event_msg",
            "payload": {
                "type": "image_generation_end",
                "call_id": call_id,
                "status": "completed",
                "result": data,
            },
        })),
        ImageSource::LocalPath { path } => Some(json!({
            "timestamp": ts,
            "type": "event_msg",
            "payload": {
                "type": "image_generation_end",
                "call_id": call_id,
                "status": "completed",
                "saved_path": path,
                "mime": mime,
            },
        })),
        ImageSource::Url { url } => Some(json!({
            "timestamp": ts,
            "type": "event_msg",
            "payload": {
                "type": "image_generation_end",
                "call_id": call_id,
                "status": "completed",
                "url": url,
                "mime": mime,
            },
        })),
    }
}

fn synth_image_generation_call(
    ts: &str,
    source: &ImageSource,
    extras: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    let ImageSource::Base64 { data } = source else {
        return None;
    };
    let id = extras
        .get("id")
        .and_then(|value| value.as_str())
        .or_else(|| extras.get("call_id").and_then(|value| value.as_str()))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("ig_{}", uuid::Uuid::now_v7().simple()));
    let status = extras
        .get("status")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("completed");
    let mut payload = json!({
        "type": "image_generation_call",
        "id": id,
        "status": status,
        "result": data,
    });
    if let Some(prompt) = extras.get("revised_prompt") {
        payload["revised_prompt"] = prompt.clone();
    }
    Some(json!({
        "timestamp": ts,
        "type": "response_item",
        "payload": payload,
    }))
}

fn codex_image_url(mime: &str, source: &ImageSource) -> String {
    match source {
        ImageSource::Base64 { data } => format!("data:{};base64,{}", mime, data),
        ImageSource::Url { url } => url.clone(),
        ImageSource::LocalPath { path } if path.starts_with("file://") => path.clone(),
        ImageSource::LocalPath { path } if path.starts_with('/') => format!("file://{}", path),
        ImageSource::LocalPath { path } => path.clone(),
    }
}

fn push_codex_text_lines(
    lines: &mut Vec<Value>,
    ts: &str,
    role: Role,
    role_str: &str,
    content: &mut Vec<Value>,
    text_parts: &mut Vec<String>,
    images: &mut Vec<String>,
    local_images: &mut Vec<String>,
) {
    if content.is_empty() && images.is_empty() && local_images.is_empty() {
        text_parts.clear();
        return;
    }

    let display_text = text_parts.join("\n");
    let response_item = if content.is_empty() {
        None
    } else {
        let mut payload = json!({
            "type": "message",
            "role": role_str,
            "content": std::mem::take(content),
        });
        if matches!(role, Role::Assistant) {
            payload["phase"] = Value::String("final_answer".into());
        }
        Some(json!({
            "timestamp": ts,
            "type": "response_item",
            "payload": payload,
        }))
    };

    match role {
        Role::User => {
            if let Some(response_item) = response_item {
                lines.push(response_item);
            }
            if !display_text.is_empty() || !images.is_empty() || !local_images.is_empty() {
                lines.push(json!({
                    "timestamp": ts,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": display_text,
                        "images": std::mem::take(images),
                        "local_images": std::mem::take(local_images),
                        "text_elements": [],
                    },
                }));
            }
        }
        Role::Assistant => {
            if !display_text.is_empty() {
                lines.push(json!({
                    "timestamp": ts,
                    "type": "event_msg",
                    "payload": {
                        "type": "agent_message",
                        "message": display_text,
                        "phase": "final_answer",
                        "memory_citation": null,
                    },
                }));
            }
            if let Some(response_item) = response_item {
                lines.push(response_item);
            }
        }
        _ => {
            if let Some(response_item) = response_item {
                lines.push(response_item);
            }
        }
    }
    text_parts.clear();
    images.clear();
    local_images.clear();
}

fn synthesized_timestamp(session: &UniversalSession, m: &UMessage) -> String {
    m.timestamp
        .or(session.updated_at)
        .or(session.created_at)
        .map(to_rfc3339_ms)
        .unwrap_or_else(|| to_rfc3339_ms(chrono::Utc::now()))
}
