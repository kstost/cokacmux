//! Claude JSONL line → UMessage mapping.
//!
//! Observed line types (from the live `~/.claude/projects/.../<uuid>.jsonl`):
//!   message, user, ai-title, permission-mode, last-prompt,
//!   file-history-snapshot, task_reminder, system, skill_listing,
//!   deferred_tools_delta, attachment, queue-operation, …
//!
//! Strategy:
//! - `user` / `assistant` / `message` (with role) → conversation messages
//!   with structured content blocks.
//! - everything else → `role: System`, `flags.is_meta: true`, payload kept
//!   verbatim in `provenance.raw`. No silent drops.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::debug;
use crate::error::Result;
use crate::ids;
use crate::time;
use crate::universal::{
    ContentBlock, ImageSource, MessageFlags, ModelInfo, Provenance, Provider, Role, UMessage,
    UniversalSession, Usage, SCHEMA_VERSION,
};

use super::sidecar;
use super::ClaudeReadCtx;

pub fn parse_lines(content: &str, ctx: &ClaudeReadCtx) -> Result<UniversalSession> {
    let total_lines = content.lines().count();
    debug::log(
        "provider_claude_parse_start",
        serde_json::json!({
            "bytes": content.len(),
            "lines": total_lines,
            "ctx_session_id": ctx.session_id.as_deref(),
            "ctx_cwd": ctx.cwd.as_deref(),
            "inline_tool_results": ctx.inline_tool_results,
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
    session.origin.provider = Some(Provider::Claude);

    let mut idx: u32 = 0;
    let mut session_cwd_seen = !session.cwd.is_empty();
    let mut session_id_seen = !session.session_id.is_empty();
    let mut empty_lines = 0usize;
    let mut invalid_json_lines = 0usize;
    let mut meta_messages = 0usize;
    let mut conversation_messages = 0usize;

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
            } // tolerate corrupt lines
        };

        // Extract cwd/sessionId opportunistically from any line that has them.
        if !session_cwd_seen {
            if let Some(c) = val.get("cwd").and_then(|v| v.as_str()) {
                if !c.is_empty() {
                    session.cwd = c.to_string();
                    session_cwd_seen = true;
                }
            }
        }
        if !session_id_seen {
            if let Some(s) = val.get("sessionId").and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    session.session_id = s.to_string();
                    session_id_seen = true;
                }
            }
        }
        // Claude per-line `version` is the Claude Code CLI version (e.g.
        // "2.1.145"). Capture the first non-empty occurrence as the
        // provider version-drift signal (see strategy §10.5).
        if session.origin.cli_version.is_none() {
            if let Some(v) = val.get("version").and_then(|v| v.as_str()) {
                if !v.is_empty() {
                    session.origin.cli_version = Some(v.to_string());
                }
            }
        }

        let line_type = val
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

        // Sidechain marker — preserved, not dropped.
        let is_sidechain = val
            .get("isSidechain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Current Claude Code stores generated titles in `ai-title`. Manual
        // session names from `claude -n/--name` are stored as `custom-title`
        // and also mirrored as `agent-name`.
        if matches!(
            line_type.as_str(),
            "ai-title" | "custom-title" | "agent-name"
        ) {
            if let Some(t) = claude_title(&val) {
                if session.title.is_none() {
                    session.title = Some(t.to_string());
                }
            }
        }

        // Compose the UMessage.
        let umessage = build_umessage(&val, &line_type, idx, ts, is_sidechain, ctx, lineno);
        if umessage.flags.is_meta {
            meta_messages = meta_messages.saturating_add(1);
        } else {
            conversation_messages = conversation_messages.saturating_add(1);
        }
        session.messages.push(umessage);
        idx += 1;
    }

    debug::log(
        "provider_claude_parse_ok",
        serde_json::json!({
            "lines": total_lines,
            "empty_lines": empty_lines,
            "invalid_json_lines": invalid_json_lines,
            "messages": session.messages.len(),
            "meta_messages": meta_messages,
            "conversation_messages": conversation_messages,
            "session_id_present": !session.session_id.is_empty(),
            "cwd_present": !session.cwd.is_empty(),
            "title_present": session.title.is_some(),
        }),
    );
    Ok(session)
}

fn claude_title(val: &Value) -> Option<&str> {
    val.get("aiTitle")
        .or_else(|| val.get("customTitle"))
        .or_else(|| val.get("agentName"))
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
}

fn build_umessage(
    val: &Value,
    line_type: &str,
    idx: u32,
    ts: Option<chrono::DateTime<chrono::Utc>>,
    is_sidechain: bool,
    ctx: &ClaudeReadCtx,
    lineno: usize,
) -> UMessage {
    // Derive a stable id. Claude JSONL lines have:
    //  - `uuid`        — unique per JSONL line
    //  - `message.id`  — Anthropic message id, may be shared across multiple
    //                    streamed chunks of one assistant reply
    // We want the line-unique id so downstream INSERTs into opencode.db
    // (PRIMARY KEY message.id) don't collide. `message.id` is preserved
    // in extras for round-trip fidelity.
    let id = val
        .get("uuid")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            val.get("message")
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| ids::synth_id(&format!("claude:{}:{}", line_type, lineno)));

    let parent_id = val
        .get("parentUuid")
        .and_then(|v| v.as_str())
        .map(String::from);

    let (role, content, model, usage, source_tag, mut flags) = match line_type {
        "user" => parse_user_line(val, ctx),
        "assistant" | "message" => parse_assistant_line(val),
        // Claude's `attachment` lines carry a typed payload under the
        // `attachment` key — e.g. `attachment.type = "deferred_tools_delta"`,
        // `attachment.type = "skill_listing"`, etc. — with type-specific
        // fields. Route to a meta system message with the full payload
        // preserved under ContentBlock::Other so nothing is dropped on a
        // cross-provider conversion.
        "attachment" => {
            let inner_type = val
                .get("attachment")
                .and_then(|a| a.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("attachment");
            let payload = val.get("attachment").cloned().unwrap_or(Value::Null);
            (
                Role::System,
                vec![ContentBlock::other(
                    format!("claude_attachment.{}", inner_type),
                    payload,
                )],
                None,
                None,
                format!("claude:attachment.{}", inner_type),
                MessageFlags {
                    is_meta: true,
                    ..Default::default()
                },
            )
        }
        // All meta-ish types collapse into a single "meta" system message
        // with content [Other{ type_tag: ..., payload: val }].
        other => (
            Role::System,
            vec![ContentBlock::other(other, val.clone())],
            None,
            None,
            format!("claude:{}", other),
            MessageFlags {
                is_meta: true,
                ..Default::default()
            },
        ),
    };

    flags.is_sidechain = is_sidechain;

    UMessage {
        id,
        parent_id,
        index: idx,
        timestamp: ts,
        role,
        model,
        usage,
        stop_reason: val
            .get("message")
            .and_then(|m| m.get("stop_reason"))
            .and_then(|v| v.as_str())
            .map(String::from),
        content,
        flags,
        provenance: Provenance {
            source_event_type: source_tag,
            raw: val.clone(),
        },
        extras: BTreeMap::new(),
    }
}

/// `{"type":"user", "message": {...}, "isSidechain": false, ...}` —
/// `message.content` is either a string (plain text) or an array of
/// `{type: "text"|"tool_result"|"image"}` blocks.
fn parse_user_line(
    val: &Value,
    ctx: &ClaudeReadCtx,
) -> (
    Role,
    Vec<ContentBlock>,
    Option<ModelInfo>,
    Option<Usage>,
    String,
    MessageFlags,
) {
    let msg = val.get("message");
    let content = msg.and_then(|m| m.get("content"));
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut role = Role::User;

    if let Some(s) = content.and_then(|v| v.as_str()) {
        blocks.push(ContentBlock::text(s));
    } else if let Some(arr) = content.and_then(|v| v.as_array()) {
        for item in arr {
            let it = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match it {
                "text" => {
                    let t = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    blocks.push(ContentBlock::text(t));
                }
                "tool_result" => {
                    role = Role::Tool;
                    let call_id = item
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let is_error = item
                        .get("is_error")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let (output, extras) = extract_tool_result_output(item, ctx);
                    blocks.push(ContentBlock::ToolResult {
                        call_id,
                        output,
                        is_error,
                        extras,
                    });
                }
                "image" => {
                    if let Some(block) = parse_claude_image(item) {
                        blocks.push(block);
                    } else {
                        blocks.push(ContentBlock::other("image", item.clone()));
                    }
                }
                "attachment" => {
                    blocks.push(ContentBlock::Attachment {
                        name: item.get("name").and_then(|v| v.as_str()).map(String::from),
                        path: item.get("path").and_then(|v| v.as_str()).map(String::from),
                        mime: item.get("mime").and_then(|v| v.as_str()).map(String::from),
                        extras: BTreeMap::new(),
                    });
                }
                other => blocks.push(ContentBlock::other(other, item.clone())),
            }
        }
    }

    (
        role,
        blocks,
        None,
        None,
        "claude:user".to_string(),
        MessageFlags::default(),
    )
}

fn parse_claude_image(item: &Value) -> Option<ContentBlock> {
    let source = item.get("source")?;

    if let Some(data) = source.get("data").and_then(|v| v.as_str()) {
        let mime = source
            .get("media_type")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("mime").and_then(|v| v.as_str()))
            .unwrap_or("application/octet-stream")
            .to_string();
        return Some(ContentBlock::Image {
            mime,
            source: ImageSource::Base64 {
                data: data.to_string(),
            },
            extras: BTreeMap::new(),
        });
    }

    let source = serde_json::from_value::<ImageSource>(source.clone()).ok()?;
    let mime = item
        .get("mime")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream")
        .to_string();
    Some(ContentBlock::Image {
        mime,
        source,
        extras: BTreeMap::new(),
    })
}

/// `{"type":"assistant"|"message", "message": {"role":..., "content":[...], "model":..., "usage":...}}`
fn parse_assistant_line(
    val: &Value,
) -> (
    Role,
    Vec<ContentBlock>,
    Option<ModelInfo>,
    Option<Usage>,
    String,
    MessageFlags,
) {
    let msg = val.get("message");
    let role = msg
        .and_then(|m| m.get("role"))
        .and_then(|v| v.as_str())
        .map(parse_role)
        .unwrap_or(Role::Assistant);

    let mut blocks: Vec<ContentBlock> = Vec::new();
    if let Some(arr) = msg
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_array())
    {
        for item in arr {
            let it = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match it {
                "text" => {
                    let t = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    blocks.push(ContentBlock::text(t));
                }
                "thinking" => {
                    let t = item.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                    let mut b = ContentBlock::thinking(t);
                    if let ContentBlock::Thinking { extras, .. } = &mut b {
                        if let Some(sig) = item.get("signature") {
                            extras.insert("signature".into(), sig.clone());
                        }
                    }
                    blocks.push(b);
                }
                "tool_use" => {
                    let call_id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = item.get("input").cloned().unwrap_or(Value::Null);
                    blocks.push(ContentBlock::tool_use(call_id, name, input));
                }
                "image" => {
                    if let Some(block) = parse_claude_image(item) {
                        blocks.push(block);
                    } else {
                        blocks.push(ContentBlock::other("image", item.clone()));
                    }
                }
                "attachment" => {
                    blocks.push(ContentBlock::Attachment {
                        name: item.get("name").and_then(|v| v.as_str()).map(String::from),
                        path: item.get("path").and_then(|v| v.as_str()).map(String::from),
                        mime: item.get("mime").and_then(|v| v.as_str()).map(String::from),
                        extras: BTreeMap::new(),
                    });
                }
                other => blocks.push(ContentBlock::other(other, item.clone())),
            }
        }
    } else if let Some(s) = msg.and_then(|m| m.get("content")).and_then(|v| v.as_str()) {
        blocks.push(ContentBlock::text(s));
    }

    let model = msg
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| ModelInfo {
            provider_id: Some("anthropic".into()),
            model_id: s.to_string(),
            variant: None,
        });

    let usage = msg.and_then(|m| m.get("usage")).map(|u| Usage {
        input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()),
        output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()),
        cached_input_tokens: u.get("cache_read_input_tokens").and_then(|v| v.as_u64()),
        reasoning_output_tokens: None,
        total_tokens: None,
        cost_usd: None,
    });

    (
        role,
        blocks,
        model,
        usage,
        "claude:assistant".to_string(),
        MessageFlags::default(),
    )
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

fn extract_tool_result_output(
    item: &Value,
    ctx: &ClaudeReadCtx,
) -> (Value, BTreeMap<String, Value>) {
    let mut extras = BTreeMap::new();
    // `content` may be a string OR an array of content blocks. Preserve a
    // source array verbatim so non-text blocks such as images are not dropped
    // when cloning back to Claude.
    let raw_text = if let Some(s) = item.get("content").and_then(|v| v.as_str()) {
        s.to_string()
    } else if let Some(arr) = item.get("content").and_then(|v| v.as_array()) {
        extras.insert("claude_tool_result_content_array".into(), Value::Bool(true));
        return (Value::Array(arr.clone()), extras);
    } else {
        return (item.get("content").cloned().unwrap_or(Value::Null), extras);
    };

    // Inline sidecar hydrate.
    if ctx.inline_tool_results {
        if let Some(side) = sidecar::extract_sidecar_ref(&raw_text) {
            if let Some(full) = sidecar::read_sidecar(&side) {
                return (Value::String(full), extras);
            }
        }
    }
    (Value::String(raw_text), extras)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ai_title_as_session_title() {
        let jsonl = r#"{"type":"ai-title","sessionId":"s1","aiTitle":"Generated Claude Title"}"#;

        let session = parse_lines(jsonl, &ClaudeReadCtx::default()).unwrap();

        assert_eq!(session.title.as_deref(), Some("Generated Claude Title"));
    }

    #[test]
    fn parses_custom_title_as_fallback() {
        let jsonl = r#"{"type":"ai-title","sessionId":"s1","customTitle":"Legacy Claude Title"}"#;

        let session = parse_lines(jsonl, &ClaudeReadCtx::default()).unwrap();

        assert_eq!(session.title.as_deref(), Some("Legacy Claude Title"));
    }

    #[test]
    fn parses_custom_title_record_as_session_title() {
        let jsonl =
            r#"{"type":"custom-title","sessionId":"s1","customTitle":"Manual Claude Title"}"#;

        let session = parse_lines(jsonl, &ClaudeReadCtx::default()).unwrap();

        assert_eq!(session.title.as_deref(), Some("Manual Claude Title"));
    }

    #[test]
    fn parses_agent_name_record_as_title_fallback() {
        let jsonl = r#"{"type":"agent-name","sessionId":"s1","agentName":"Named Agent"}"#;

        let session = parse_lines(jsonl, &ClaudeReadCtx::default()).unwrap();

        assert_eq!(session.title.as_deref(), Some("Named Agent"));
    }
}
