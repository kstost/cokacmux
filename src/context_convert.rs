//! Context-wrapper conversion.
//!
//! This is intentionally not a faithful provider-to-provider transcript
//! conversion. It packages the source session into one user message and pairs
//! it with a single assistant acknowledgement, so provider writers only need
//! to synthesize a minimal valid two-message session.
//!
//! Keep this module separate from the provider adapters: adapter round-trips can
//! still test native preservation, while cross-provider continuation remains a
//! simple context handoff instead of a hidden lossless-conversion promise.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use chrono::{Duration, Utc};
use serde_json::{json, Value};

use crate::universal::{
    ContentBlock, GitInfo, ImageSource, MessageFlags, ModelInfo, Provenance, Provider,
    ProviderOrigin, Role, UMessage, UniversalSession, Usage, SCHEMA_VERSION,
};

pub const CONTEXT_CONTINUATION_PROMPT: &str =
    "You should keep working from this context. If you got it, then say ok";
pub const CONTEXT_ACK: &str = "ok";

/// Build the target session used by `convert`.
///
/// The visible conversation is exactly two messages:
/// 1. user: the full rendered source session plus the continuation prompt
/// 2. assistant: `ok`
pub fn wrap_session_for_context_convert(
    source: &UniversalSession,
    target_provider: Provider,
) -> UniversalSession {
    let now = Utc::now();
    let created_at = now;
    let assistant_at = created_at + Duration::seconds(1);
    let session_id = target_native_session_id(target_provider);
    let user_id = target_native_message_id(target_provider);
    let assistant_id = target_native_message_id(target_provider);
    let context = context_user_message_text(source);

    let mut extras = BTreeMap::new();
    extras.insert(
        "context_convert".into(),
        json!({
            "source_provider": source.origin.provider.map(|p| p.as_str()),
            "source_session_id": &source.session_id,
            "target_session_id": &session_id,
            "target_provider": target_provider.as_str(),
            "strategy": "two_message_context_wrapper",
        }),
    );

    let mut wrapped = UniversalSession {
        schema_version: SCHEMA_VERSION.to_string(),
        session_id,
        origin: ProviderOrigin {
            provider: Some(target_provider),
            ..Default::default()
        },
        cwd: source.cwd.clone(),
        created_at: Some(created_at),
        updated_at: Some(assistant_at),
        title: context_convert_title(source),
        model: None,
        git: source.git.clone(),
        usage_total: None,
        session_meta: None,
        messages: Vec::new(),
        extras,
    };

    wrapped.messages.push(UMessage {
        id: user_id.clone(),
        parent_id: None,
        index: 0,
        timestamp: Some(created_at),
        role: Role::User,
        model: None,
        usage: None,
        stop_reason: None,
        content: vec![ContentBlock::text(context)],
        flags: MessageFlags::default(),
        provenance: Provenance {
            source_event_type: "cokacmux:context_convert.user".into(),
            raw: json!({
                "source_provider": source.origin.provider.map(|p| p.as_str()),
                "source_session_id": &source.session_id,
            }),
        },
        extras: BTreeMap::new(),
    });
    wrapped.messages.push(UMessage {
        id: assistant_id,
        parent_id: Some(user_id),
        index: 1,
        timestamp: Some(assistant_at),
        role: Role::Assistant,
        model: None,
        usage: None,
        stop_reason: Some("stop".into()),
        content: vec![ContentBlock::text(CONTEXT_ACK)],
        flags: MessageFlags::default(),
        provenance: Provenance {
            source_event_type: "cokacmux:context_convert.assistant".into(),
            raw: json!({
                "content": CONTEXT_ACK,
            }),
        },
        extras: BTreeMap::new(),
    });

    wrapped
}

fn target_native_session_id(provider: Provider) -> String {
    match provider {
        Provider::Claude | Provider::Codex | Provider::Pi | Provider::Gjc => {
            crate::ids::new_uuid_v7()
        }
        Provider::OpenCode => crate::ids::opencode_session_id(),
    }
}

fn target_native_message_id(provider: Provider) -> String {
    match provider {
        Provider::Claude | Provider::Codex | Provider::Pi | Provider::Gjc => {
            crate::ids::new_uuid_v7()
        }
        Provider::OpenCode => crate::ids::opencode_message_id(),
    }
}

pub fn context_user_message_text(source: &UniversalSession) -> String {
    let mut context = render_source_session(source);
    while context.ends_with('\n') {
        context.pop();
    }
    if !context.is_empty() {
        context.push_str("\n\n");
    }
    context.push_str(CONTEXT_CONTINUATION_PROMPT);
    context
}

fn context_convert_title(source: &UniversalSession) -> Option<String> {
    source
        .title
        .as_ref()
        .filter(|title| !title.trim().is_empty())
        .cloned()
        .or_else(|| Some(format!("Context from {}", source.session_id)))
}

fn render_source_session(session: &UniversalSession) -> String {
    let mut out = String::new();
    let provider = session.origin.provider.map(|p| p.as_str()).unwrap_or("?");
    let _ = writeln!(out, "=== {} ({}) ===", session.session_id, provider);
    if let Some(title) = session.title.as_deref().filter(|value| !value.is_empty()) {
        let _ = writeln!(out, "title  : {}", title);
    }
    let _ = writeln!(out, "cwd    : {}", session.cwd);
    if let Some(model) = &session.model {
        let _ = writeln!(out, "model  : {}", model_label(model));
    }
    if let Some(git) = &session.git {
        let _ = writeln!(out, "git    : {}", git_label(git));
    }
    if let Some(usage) = &session.usage_total {
        let _ = writeln!(out, "tokens : {}", usage_label(usage));
    }
    if let Some(created_at) = session.created_at {
        let _ = writeln!(out, "created: {}", created_at.to_rfc3339());
    }
    if let Some(updated_at) = session.updated_at {
        let _ = writeln!(out, "updated: {}", updated_at.to_rfc3339());
    }
    let _ = writeln!(out, "messages: {}", session.messages.len());
    out.push('\n');

    for message in &session.messages {
        render_message(&mut out, message);
    }
    out
}

fn render_message(out: &mut String, message: &UMessage) {
    let _ = write!(out, "[{} #{}", role_label(message.role), message.index);
    if let Some(timestamp) = message.timestamp {
        let _ = write!(out, " @ {}", timestamp.to_rfc3339());
    }
    if message.flags.is_sidechain {
        out.push_str(" sidechain");
    }
    if message.flags.is_meta {
        out.push_str(" meta");
    }
    if message.flags.is_compaction {
        out.push_str(" compaction");
    }
    if message.flags.skipped {
        out.push_str(" skipped");
    }
    out.push_str("]\n");

    if message.content.is_empty() {
        out.push_str("(empty message)\n\n");
        return;
    }
    for block in &message.content {
        render_block(out, block);
    }
    out.push('\n');
}

fn render_block(out: &mut String, block: &ContentBlock) {
    match block {
        ContentBlock::Text { text, .. } => {
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
        ContentBlock::Thinking {
            text, encrypted, ..
        } => {
            out.push_str("(thinking");
            if encrypted.is_some() {
                out.push_str(", encrypted");
            }
            out.push_str(") ");
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
        ContentBlock::ToolUse {
            call_id,
            name,
            input,
            ..
        } => {
            let input = serde_json::to_string(input).unwrap_or_default();
            let _ = writeln!(out, "tool_use[{}] {}: {}", call_id, name, input);
        }
        ContentBlock::ToolResult {
            call_id,
            output,
            is_error,
            ..
        } => {
            let output = value_as_text(output);
            let _ = writeln!(
                out,
                "tool_result[{}]{}: {}",
                call_id,
                if *is_error { " error" } else { "" },
                output
            );
        }
        ContentBlock::Image { mime, source, .. } => {
            let _ = writeln!(out, "image: {} {}", mime, image_source_label(mime, source));
        }
        ContentBlock::Attachment {
            name, path, mime, ..
        } => {
            let _ = writeln!(
                out,
                "attachment: name={} path={} mime={}",
                name.as_deref().unwrap_or(""),
                path.as_deref().unwrap_or(""),
                mime.as_deref().unwrap_or("")
            );
        }
        ContentBlock::Patch { unified_diff, .. } => {
            out.push_str("patch:\n");
            out.push_str(unified_diff);
            if !unified_diff.ends_with('\n') {
                out.push('\n');
            }
        }
        ContentBlock::Other { type_tag, payload } => {
            let payload = serde_json::to_string(payload).unwrap_or_default();
            let _ = writeln!(out, "other[{}]: {}", type_tag, payload);
        }
    }
}

fn value_as_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default())
}

fn image_source_label(mime: &str, source: &ImageSource) -> String {
    match source {
        ImageSource::LocalPath { path } => format!("path={}", path),
        ImageSource::Base64 { data } => format!("data:{};base64,{}", mime, data),
        ImageSource::Url { url } => format!("url={}", url),
    }
}

fn role_label(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::System => "system",
        Role::Developer => "developer",
    }
}

fn model_label(model: &ModelInfo) -> String {
    let provider = model.provider_id.as_deref().unwrap_or("?");
    if let Some(variant) = model.variant.as_deref().filter(|value| !value.is_empty()) {
        format!("{}/{} ({})", provider, model.model_id, variant)
    } else {
        format!("{}/{}", provider, model.model_id)
    }
}

fn git_label(git: &GitInfo) -> String {
    let branch = git.branch.as_deref().unwrap_or("?");
    let commit = git.commit.as_deref().unwrap_or("");
    format!("{} {}", branch, commit).trim().to_string()
}

fn usage_label(usage: &Usage) -> String {
    let mut parts = Vec::new();
    if let Some(value) = usage.input_tokens {
        parts.push(format!("in={}", value));
    }
    if let Some(value) = usage.output_tokens {
        parts.push(format!("out={}", value));
    }
    if let Some(value) = usage.cached_input_tokens {
        parts.push(format!("cached={}", value));
    }
    if let Some(value) = usage.reasoning_output_tokens {
        parts.push(format!("reasoning={}", value));
    }
    if let Some(value) = usage.total_tokens {
        parts.push(format!("total={}", value));
    }
    if let Some(value) = usage.cost_usd {
        parts.push(format!("cost=${:.4}", value));
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn wrapper_contains_exact_two_messages_and_ack_prompt() {
        let mut source = UniversalSession::new("source-id", Provider::Codex, "/repo");
        source.title = Some("Original title".into());
        source.created_at = Some(chrono::Utc.with_ymd_and_hms(2020, 1, 2, 3, 4, 5).unwrap());
        source.updated_at = Some(chrono::Utc.with_ymd_and_hms(2020, 1, 2, 4, 4, 5).unwrap());
        source.messages.push(UMessage {
            id: "m1".into(),
            parent_id: None,
            index: 0,
            timestamp: None,
            role: Role::User,
            model: None,
            usage: None,
            stop_reason: None,
            content: vec![ContentBlock::text("hello")],
            flags: MessageFlags::default(),
            provenance: Provenance {
                source_event_type: "test".into(),
                raw: json!({}),
            },
            extras: BTreeMap::new(),
        });
        source.messages.push(UMessage {
            id: "m2".into(),
            parent_id: Some("m1".into()),
            index: 1,
            timestamp: None,
            role: Role::User,
            model: None,
            usage: None,
            stop_reason: None,
            content: vec![ContentBlock::Image {
                mime: "image/png".into(),
                source: ImageSource::Base64 {
                    data: "iVBORw0KGgo=".into(),
                },
                extras: BTreeMap::new(),
            }],
            flags: MessageFlags::default(),
            provenance: Provenance {
                source_event_type: "test".into(),
                raw: json!({}),
            },
            extras: BTreeMap::new(),
        });

        let wrapped = wrap_session_for_context_convert(&source, Provider::Claude);
        assert_eq!(wrapped.origin.provider, Some(Provider::Claude));
        assert!(uuid::Uuid::parse_str(&wrapped.session_id).is_ok());
        assert_eq!(wrapped.messages.len(), 2);
        assert!(
            wrapped.created_at.unwrap() > source.updated_at.unwrap(),
            "wrapper session should be a fresh target session, not an old source timestamp"
        );
        assert_eq!(
            wrapped.updated_at.unwrap(),
            wrapped.created_at.unwrap() + Duration::seconds(1)
        );
        assert_eq!(wrapped.messages[0].role, Role::User);
        assert_eq!(wrapped.messages[1].role, Role::Assistant);
        assert_eq!(
            wrapped.messages[1].parent_id,
            Some(wrapped.messages[0].id.clone())
        );
        assert_eq!(wrapped.title.as_deref(), Some("Original title"));

        let ContentBlock::Text { text, .. } = &wrapped.messages[0].content[0] else {
            panic!("expected text context");
        };
        assert!(text.contains("=== source-id (codex) ==="));
        assert!(text.contains("[user #0]"));
        assert!(text.contains("hello"));
        assert!(text.contains("image: image/png data:image/png;base64,iVBORw0KGgo="));
        assert!(text.ends_with(CONTEXT_CONTINUATION_PROMPT));

        let ContentBlock::Text { text, .. } = &wrapped.messages[1].content[0] else {
            panic!("expected ack text");
        };
        assert_eq!(text, CONTEXT_ACK);
    }

    #[test]
    fn opencode_wrapper_uses_native_ids() {
        let source = UniversalSession::new("source-id", Provider::Codex, "/repo");
        let wrapped = wrap_session_for_context_convert(&source, Provider::OpenCode);

        assert!(wrapped.session_id.starts_with("ses_"));
        assert!(wrapped.messages[0].id.starts_with("msg_"));
        assert!(wrapped.messages[1].id.starts_with("msg_"));
        assert_eq!(
            wrapped.messages[1].parent_id,
            Some(wrapped.messages[0].id.clone())
        );
    }
}
