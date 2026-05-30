//! Render a UniversalSession as a human-readable transcript.
//!
//! Two modes:
//!  - `Full`: every message, every block, no truncation.
//!  - `Summary`: skip meta, cap each text block at PREVIEW_BLOCK_CAP chars.

use std::fmt::Write as _;

use crate::universal::{ContentBlock, ImageSource, Role, UMessage, UniversalSession, Usage};
use serde_json::Value;

pub const PREVIEW_BLOCK_CAP: usize = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Full,
    Summary,
}

pub fn render(session: &UniversalSession, mode: Mode) -> String {
    if mode == Mode::Summary {
        return render_summary(session);
    }

    let mut out = String::new();
    out.push_str(&format!(
        "=== {} ({}) ===\n",
        session.session_id,
        session.origin.provider.map(|p| p.as_str()).unwrap_or("?"),
    ));
    if let Some(t) = &session.title {
        out.push_str(&format!("title  : {}\n", t));
    }
    out.push_str(&format!("cwd    : {}\n", session.cwd));
    if let Some(m) = &session.model {
        let prov = m.provider_id.as_deref().unwrap_or("?");
        let var = m.variant.as_deref().unwrap_or("");
        let suffix = if var.is_empty() {
            String::new()
        } else {
            format!(" ({})", var)
        };
        out.push_str(&format!("model  : {}/{}{}\n", prov, m.model_id, suffix));
    }
    if let Some(g) = &session.git {
        let b = g.branch.as_deref().unwrap_or("?");
        let c = g.commit.as_deref().unwrap_or("");
        out.push_str(&format!("git    : {} {}\n", b, c));
    }
    if let Some(u) = &session.usage_total {
        out.push_str(&format!(
            "tokens : in={} out={} cached={} reasoning={} cost=${:.4}\n",
            u.input_tokens.unwrap_or(0),
            u.output_tokens.unwrap_or(0),
            u.cached_input_tokens.unwrap_or(0),
            u.reasoning_output_tokens.unwrap_or(0),
            u.cost_usd.unwrap_or(0.0),
        ));
    }
    if let Some(t) = session.created_at {
        out.push_str(&format!("created: {}\n", t.to_rfc3339()));
    }
    if let Some(t) = session.updated_at {
        out.push_str(&format!("updated: {}\n", t.to_rfc3339()));
    }
    out.push_str(&format!("messages: {}\n", session.messages.len()));
    out.push('\n');

    for m in &session.messages {
        if mode == Mode::Summary && (m.flags.is_meta || m.flags.skipped) {
            continue;
        }
        if mode == Mode::Summary && m.content.is_empty() {
            continue;
        }
        let role_str = role_label(m.role);
        let ts = m
            .timestamp
            .map(|t| t.format("%H:%M:%S").to_string())
            .unwrap_or_default();
        let ts_suffix = if ts.is_empty() {
            String::new()
        } else {
            format!(" @ {}", ts)
        };
        out.push_str(&format!("[{}{}]", role_str, ts_suffix));
        if m.flags.is_sidechain {
            out.push_str(" (sidechain)");
        }
        if m.flags.is_meta && mode == Mode::Full {
            out.push_str(" (meta)");
        }
        out.push('\n');

        for b in &m.content {
            render_block(&mut out, b, mode);
        }
        out.push('\n');
    }
    out
}

fn render_summary(session: &UniversalSession) -> String {
    let mut out = String::new();
    let provider = session.origin.provider.map(|p| p.as_str()).unwrap_or("?");
    let visible_messages = session
        .messages
        .iter()
        .filter(|m| summary_message_visible(m))
        .count();

    out.push_str("Session\n");
    summary_field(&mut out, "provider", provider);
    summary_field(&mut out, "id", &session.session_id);
    if let Some(title) = session.title.as_deref().filter(|value| !value.is_empty()) {
        summary_field(&mut out, "title", title);
    }
    summary_field(&mut out, "cwd", &session.cwd);
    if let Some(model) = &session.model {
        summary_field(&mut out, "model", &model_label(model));
    }
    if let Some(git) = &session.git {
        let branch = git.branch.as_deref().unwrap_or("?");
        let commit = git.commit.as_deref().unwrap_or("");
        summary_field(
            &mut out,
            "git",
            &format!("{} {}", branch, commit).trim().to_string(),
        );
    }
    if let Some(usage) = &session.usage_total {
        summary_field(&mut out, "tokens", &usage_label(usage));
    }
    if let Some(created) = session.created_at {
        summary_field(&mut out, "created", &created.to_rfc3339());
    }
    if let Some(updated) = session.updated_at {
        summary_field(&mut out, "updated", &updated.to_rfc3339());
    }
    summary_field(
        &mut out,
        "messages",
        &format!("{}/{} visible", visible_messages, session.messages.len()),
    );
    out.push('\n');

    out.push_str("Messages\n");
    if visible_messages == 0 {
        out.push_str("  (no visible messages)\n");
        return out;
    }

    for message in session.messages.iter().filter(|m| summary_message_visible(m)) {
        render_summary_message(&mut out, message);
    }
    out
}

fn summary_message_visible(message: &UMessage) -> bool {
    !message.flags.is_meta && !message.flags.skipped && !message.content.is_empty()
}

fn summary_field(out: &mut String, label: &str, value: &str) {
    let value = sanitize_for_terminal(value);
    let _ = writeln!(out, "  {:<8}: {}", label, value);
}

fn render_summary_message(out: &mut String, message: &UMessage) {
    let mut meta = Vec::new();
    if let Some(timestamp) = message.timestamp {
        meta.push(timestamp.format("%H:%M:%S").to_string());
    }
    if message.flags.is_sidechain {
        meta.push("sidechain".into());
    }
    if message.flags.is_compaction {
        meta.push("compaction".into());
    }
    if let Some(model) = &message.model {
        meta.push(model_label(model));
    }
    if let Some(stop_reason) = message
        .stop_reason
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        meta.push(format!("stop={}", stop_reason));
    }
    if let Some(usage) = &message.usage {
        meta.push(usage_label(usage));
    }

    let role = role_label(message.role).to_ascii_uppercase();
    if meta.is_empty() {
        let _ = writeln!(out, "{} #{}", role, message.index);
    } else {
        let _ = writeln!(out, "{} #{} · {}", role, message.index, meta.join(" · "));
    }

    for block in &message.content {
        render_summary_block(out, block);
    }
    out.push('\n');
}

fn render_summary_block(out: &mut String, block: &ContentBlock) {
    match block {
        ContentBlock::Text { text, extras } => {
            if let Some(label) = text_block_summary_label(extras) {
                let _ = writeln!(out, "  {}:", label);
                push_indented_text(out, "    ", &truncate_for_mode(text, Mode::Summary));
            } else {
                push_indented_text(out, "  ", &truncate_for_mode(text, Mode::Summary));
            }
        }
        ContentBlock::Thinking {
            text, encrypted, ..
        } => {
            out.push_str("  thinking");
            if encrypted.is_some() {
                out.push_str(" · encrypted");
            }
            out.push_str(":\n");
            if text.trim().is_empty() {
                out.push_str("    (no visible reasoning text)\n");
            } else {
                push_indented_text(out, "    ", &truncate_for_mode(text, Mode::Summary));
            }
        }
        ContentBlock::ToolUse {
            call_id,
            name,
            input,
            extras,
        } => {
            let mut label = format!("  tool use: {}", display_tool_name(name));
            if !call_id.is_empty() {
                label.push_str(&format!(" [{}]", short_id(call_id)));
            }
            if let Some(status) = extras.get("status").and_then(Value::as_str) {
                label.push_str(&format!(" · {}", status));
            }
            out.push_str(&label);
            out.push('\n');
            render_summary_value(out, "    ", input);
        }
        ContentBlock::ToolResult {
            call_id,
            output,
            is_error,
            extras,
        } => {
            let mut label = String::from("  tool result");
            if let Some(name) = extras.get("name").and_then(Value::as_str) {
                label.push_str(&format!(": {}", display_tool_name(name)));
            }
            if !call_id.is_empty() {
                label.push_str(&format!(" [{}]", short_id(call_id)));
            }
            label.push_str(if *is_error { " · error" } else { " · ok" });
            out.push_str(&label);
            out.push('\n');
            render_summary_value(out, "    ", output);
        }
        ContentBlock::Image { mime, source, .. } => {
            let _ = writeln!(out, "  image: {} ({})", mime, image_source_label(source));
        }
        ContentBlock::Attachment {
            name, path, mime, ..
        } => {
            let mut pieces = Vec::new();
            if let Some(name) = name.as_deref().filter(|value| !value.is_empty()) {
                pieces.push(format!("name={}", sanitize_for_terminal(name)));
            }
            if let Some(path) = path.as_deref().filter(|value| !value.is_empty()) {
                pieces.push(format!("path={}", sanitize_for_terminal(path)));
            }
            if let Some(mime) = mime.as_deref().filter(|value| !value.is_empty()) {
                pieces.push(format!("mime={}", sanitize_for_terminal(mime)));
            }
            if pieces.is_empty() {
                out.push_str("  attachment\n");
            } else {
                let _ = writeln!(out, "  attachment: {}", pieces.join(" · "));
            }
        }
        ContentBlock::Patch { unified_diff, .. } => {
            let (added, removed) = diff_stats(unified_diff);
            let _ = writeln!(out, "  patch: +{} -{}", added, removed);
            push_indented_text(
                out,
                "    ",
                &truncate_for_mode(unified_diff, Mode::Summary),
            );
        }
        ContentBlock::Other { type_tag, payload } => {
            let _ = writeln!(out, "  other: {}", other_summary_label(type_tag));
            if !value_is_empty(payload) {
                render_summary_value(out, "    ", payload);
            }
        }
    }
}

fn text_block_summary_label(extras: &std::collections::BTreeMap<String, Value>) -> Option<String> {
    let codex_block = extras.get("codex_block").and_then(Value::as_str)?;
    let mut label = match codex_block {
        "input_text" => "input text".to_string(),
        "output_text" => "output text".to_string(),
        other => other.replace('_', " "),
    };
    if let Some(phase) = extras
        .get("codex_phase")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        label.push_str(&format!(" · {}", phase));
    }
    Some(label)
}

fn render_summary_value(out: &mut String, indent: &str, value: &Value) {
    match value {
        Value::Null => {
            let _ = writeln!(out, "{}(none)", indent);
        }
        Value::String(text) => {
            let text = truncate_for_mode(text, Mode::Summary);
            if text.trim().is_empty() {
                let _ = writeln!(out, "{}(empty)", indent);
            } else {
                push_indented_text(out, indent, &text);
            }
        }
        Value::Bool(_) | Value::Number(_) => {
            let _ = writeln!(out, "{}{}", indent, value);
        }
        Value::Array(items) => {
            if items.is_empty() {
                let _ = writeln!(out, "{}[]", indent);
                return;
            }
            if let Some(inline) = inline_value(value, 120) {
                let _ = writeln!(out, "{}{}", indent, inline);
                return;
            }
            for (idx, item) in items.iter().take(6).enumerate() {
                match item {
                    Value::Object(_) | Value::Array(_) => {
                        let _ = writeln!(out, "{}- item {}", indent, idx + 1);
                        render_summary_value(out, &format!("{}  ", indent), item);
                    }
                    _ => {
                        let _ = writeln!(
                            out,
                            "{}- {}",
                            indent,
                            inline_value(item, 120)
                                .unwrap_or_else(|| compact_json(item, Mode::Summary))
                        );
                    }
                }
            }
            if items.len() > 6 {
                let _ = writeln!(out, "{}... +{} items", indent, items.len() - 6);
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                let _ = writeln!(out, "{}{{}}", indent);
                return;
            }
            for (idx, (key, item)) in map.iter().enumerate() {
                if idx >= 8 {
                    let _ = writeln!(out, "{}... +{} fields", indent, map.len() - idx);
                    break;
                }
                match item {
                    Value::String(text) if text.contains('\n') => {
                        let _ = writeln!(out, "{}{}:", indent, key);
                        push_indented_text(
                            out,
                            &format!("{}  ", indent),
                            &truncate_for_mode(text, Mode::Summary),
                        );
                    }
                    Value::Object(_) | Value::Array(_) => {
                        if let Some(inline) = inline_value(item, 100) {
                            let _ = writeln!(out, "{}{}: {}", indent, key, inline);
                        } else {
                            let _ = writeln!(out, "{}{}:", indent, key);
                            render_summary_value(out, &format!("{}  ", indent), item);
                        }
                    }
                    _ => {
                        let _ = writeln!(
                            out,
                            "{}{}: {}",
                            indent,
                            key,
                            inline_value(item, 120)
                                .unwrap_or_else(|| compact_json(item, Mode::Summary))
                        );
                    }
                }
            }
        }
    }
}

fn push_indented_text(out: &mut String, indent: &str, text: &str) {
    let text = sanitize_for_terminal(text);
    if text.is_empty() {
        let _ = writeln!(out, "{}(empty)", indent);
        return;
    }
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{}{}", indent, line);
        }
    }
    if text.ends_with('\n') {
        return;
    }
}

fn inline_value(value: &Value, max_len: usize) -> Option<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Some(value.to_string()),
        Value::String(text) => {
            let text = sanitize_for_terminal(text);
            if text.contains('\n') {
                None
            } else {
                Some(truncate_inline_text(&text, max_len))
            }
        }
        Value::Array(items) => {
            if items.len() > 4 {
                return None;
            }
            let mut parts = Vec::new();
            for item in items {
                parts.push(inline_value(item, max_len)?);
            }
            let joined = format!("[{}]", parts.join(", "));
            (joined.len() <= max_len).then_some(joined)
        }
        Value::Object(map) => {
            if map.len() > 3 {
                return None;
            }
            let mut parts = Vec::new();
            for (key, item) in map {
                parts.push(format!("{}: {}", key, inline_value(item, max_len)?));
            }
            let joined = format!("{{{}}}", parts.join(", "));
            (joined.len() <= max_len).then_some(joined)
        }
    }
}

fn truncate_inline_text(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    if max_len == 0 {
        return String::new();
    }
    let keep = max_len.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

fn compact_json(value: &Value, mode: Mode) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| String::new());
    truncate_for_mode(&text, mode)
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

fn display_tool_name(name: &str) -> String {
    if name.trim().is_empty() {
        "(unnamed)".into()
    } else {
        sanitize_for_terminal(name)
    }
}

fn image_source_label(source: &ImageSource) -> String {
    match source {
        ImageSource::LocalPath { path } => format!("path={}", sanitize_for_terminal(path)),
        ImageSource::Url { url } => format!("url={}", sanitize_for_terminal(url)),
        ImageSource::Base64 { data } => format!("base64, {} bytes", data.len()),
    }
}

fn other_summary_label(type_tag: &str) -> String {
    match type_tag {
        "opencode_user_files" => "opencode files".into(),
        "opencode_user_agents" => "opencode agents".into(),
        "opencode_user_references" => "opencode references".into(),
        tag if tag.starts_with("claude_attachment.") => {
            format!("claude {}", tag.trim_start_matches("claude_attachment."))
        }
        tag if tag.starts_with("opencode_session_message.") => {
            format!(
                "opencode {}",
                tag.trim_start_matches("opencode_session_message.")
            )
        }
        tag => tag.replace('_', " "),
    }
}

fn model_label(model: &crate::universal::ModelInfo) -> String {
    let provider = model.provider_id.as_deref().unwrap_or("?");
    let suffix = model
        .variant
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|variant| format!(" ({})", variant))
        .unwrap_or_default();
    format!("{}/{}{}", provider, model.model_id, suffix)
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

fn diff_stats(diff: &str) -> (usize, usize) {
    let added = diff
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    let removed = diff
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();
    (added, removed)
}

fn render_block(out: &mut String, b: &ContentBlock, mode: Mode) {
    match b {
        ContentBlock::Text { text, .. } => {
            push_text(out, text, mode);
            out.push('\n');
        }
        ContentBlock::Thinking { text, .. } => {
            out.push_str("(thinking) ");
            push_text(out, text, mode);
            out.push('\n');
        }
        ContentBlock::ToolUse {
            call_id,
            name,
            input,
            ..
        } => {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            let truncated = truncate_for_mode(&input_str, mode);
            out.push_str(&format!(
                "→ tool_use[{}] {}: {}\n",
                short_id(call_id),
                name,
                truncated
            ));
        }
        ContentBlock::ToolResult {
            call_id,
            output,
            is_error,
            ..
        } => {
            let err = if *is_error { " ERROR" } else { "" };
            let output_str = match output {
                serde_json::Value::String(s) => s.clone(),
                v => serde_json::to_string(v).unwrap_or_default(),
            };
            let truncated = truncate_for_mode(&output_str, mode);
            out.push_str(&format!(
                "← tool_result[{}]{}: {}\n",
                short_id(call_id),
                err,
                truncated
            ));
        }
        ContentBlock::Image { mime, .. } => {
            out.push_str(&format!("(image, mime={})\n", mime));
        }
        ContentBlock::Attachment { name, mime, .. } => {
            out.push_str(&format!(
                "(attachment{}{})\n",
                name.as_ref()
                    .map(|n| format!(" name={}", n))
                    .unwrap_or_default(),
                mime.as_ref()
                    .map(|m| format!(" mime={}", m))
                    .unwrap_or_default(),
            ));
        }
        ContentBlock::Patch { unified_diff, .. } => {
            let truncated = truncate_for_mode(unified_diff, mode);
            out.push_str(&format!("(patch)\n{}\n", truncated));
        }
        ContentBlock::Other { type_tag, payload } => {
            if mode == Mode::Summary {
                out.push_str(&format!("({})\n", type_tag));
            } else {
                let pretty = serde_json::to_string(payload).unwrap_or_default();
                out.push_str(&format!(
                    "({}) {}\n",
                    type_tag,
                    truncate_for_mode(&pretty, mode)
                ));
            }
        }
    }
}

fn role_label(r: Role) -> &'static str {
    match r {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::System => "system",
        Role::Developer => "developer",
    }
}

fn push_text(out: &mut String, text: &str, mode: Mode) {
    // truncate_for_mode also sanitizes; this keeps text and tool-result/
    // patch/other content on the same code path.
    out.push_str(&truncate_for_mode(text, mode));
}

#[cfg(test)]
#[allow(dead_code)]
fn _ensure_sanitize_used() {
    let _ = sanitize_for_terminal("");
}

fn truncate_for_mode(s: &str, mode: Mode) -> String {
    // Strip ANSI escape sequences and other control characters first so the
    // rendered preview doesn't corrupt the terminal (real session content —
    // especially tool_result outputs like `ls --color=always` or grep
    // matches — frequently embeds CSI sequences that would otherwise be
    // re-interpreted by the user's terminal and leak across cell boundaries).
    let s = sanitize_for_terminal(s);
    if mode == Mode::Full || s.len() <= PREVIEW_BLOCK_CAP {
        return s;
    }
    let mut end = PREVIEW_BLOCK_CAP;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let kept = s[..end].to_string();
    format!("{}… [+{} chars]", kept, s.len() - end)
}

/// Remove ANSI escape sequences and other terminal-control bytes from text
/// destined for direct stdout / Paragraph display. The original text stays
/// untouched in the UniversalSession on disk; this only affects rendered
/// output.
pub fn sanitize_for_terminal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // Drop the escape sequence. Recognize CSI (`ESC [ … final`),
                // OSC (`ESC ] … BEL` or `ESC ] … ESC \`), and single-char
                // intermediate escapes (`ESC X`).
                match chars.next() {
                    Some('[') => {
                        // CSI — read until a "final byte" in 0x40..=0x7E.
                        for c2 in chars.by_ref() {
                            let v = c2 as u32;
                            if (0x40..=0x7e).contains(&v) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        // OSC — read until BEL (0x07) or ESC '\'.
                        while let Some(c2) = chars.next() {
                            if c2 == '\x07' {
                                break;
                            }
                            if c2 == '\x1b' {
                                let _ = chars.next(); // consume the '\'
                                break;
                            }
                        }
                    }
                    Some(_) => { /* single-char escape; already consumed */ }
                    None => break,
                }
            }
            '\n' | '\t' => out.push(c),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                out.push('·');
            }
            other => out.push(other),
        }
    }
    out
}

fn short_id(s: &str) -> String {
    if s.len() <= 12 {
        s.to_string()
    } else {
        format!("{}…", &s[..12])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universal::{
        ContentBlock, ImageSource, MessageFlags, Provenance, Role, UMessage, UniversalSession,
    };

    #[test]
    fn sanitize_strips_ansi_csi() {
        // bash output via `ls --color`: "\x1b[01;34mdir\x1b[0m"
        let input = "before \x1b[01;34mdir\x1b[0m after";
        assert_eq!(sanitize_for_terminal(input), "before dir after");
    }

    #[test]
    fn sanitize_strips_osc_terminal_title() {
        // OSC 0;… BEL (terminal-title setter)
        let input = "x\x1b]0;my title\x07y";
        assert_eq!(sanitize_for_terminal(input), "xy");
    }

    #[test]
    fn sanitize_drops_other_control_chars_to_dot() {
        let input = "a\x01b\x07c\x7fd";
        assert_eq!(sanitize_for_terminal(input), "a·b·c·d");
    }

    #[test]
    fn sanitize_keeps_tabs_and_newlines() {
        let input = "line1\nline2\twith tab";
        assert_eq!(sanitize_for_terminal(input), "line1\nline2\twith tab");
    }

    #[test]
    fn render_minimal() {
        let mut s = UniversalSession::new("test-sid", crate::universal::Provider::Claude, "/tmp");
        s.messages.push(UMessage {
            id: "m1".into(),
            parent_id: None,
            index: 0,
            timestamp: None,
            role: Role::User,
            model: None,
            usage: None,
            stop_reason: None,
            content: vec![ContentBlock::text("hi")],
            flags: MessageFlags::default(),
            provenance: Provenance {
                source_event_type: "test".into(),
                raw: serde_json::Value::Null,
            },
            extras: Default::default(),
        });
        let out = render(&s, Mode::Summary);
        assert!(out.contains("test-sid"));
        assert!(out.contains("USER #0"));
        assert!(out.contains("hi"));

        let full = render(&s, Mode::Full);
        assert!(full.contains("[user"));
    }

    #[test]
    fn summary_formats_common_content_blocks() {
        let mut s = UniversalSession::new("summary-sid", crate::universal::Provider::Codex, "/repo");
        s.messages.push(UMessage {
            id: "m1".into(),
            parent_id: None,
            index: 0,
            timestamp: None,
            role: Role::Assistant,
            model: None,
            usage: None,
            stop_reason: None,
            content: vec![
                ContentBlock::thinking("checked the plan"),
                ContentBlock::tool_use(
                    "call-1234567890",
                    "Bash",
                    serde_json::json!({"cmd": "echo hi", "cwd": "/repo"}),
                ),
                ContentBlock::tool_result("call-1234567890", serde_json::json!("hi"), false),
                ContentBlock::Image {
                    mime: "image/png".into(),
                    source: ImageSource::LocalPath {
                        path: "/tmp/a.png".into(),
                    },
                    extras: Default::default(),
                },
                ContentBlock::Attachment {
                    name: Some("log.txt".into()),
                    path: Some("/tmp/log.txt".into()),
                    mime: Some("text/plain".into()),
                    extras: Default::default(),
                },
                ContentBlock::Patch {
                    unified_diff: "--- a\n+++ b\n-old\n+new\n".into(),
                    extras: Default::default(),
                },
                ContentBlock::other("opencode_user_references", serde_json::json!([{"path": "src"}])),
            ],
            flags: MessageFlags::default(),
            provenance: Provenance {
                source_event_type: "test".into(),
                raw: serde_json::Value::Null,
            },
            extras: Default::default(),
        });

        let out = render(&s, Mode::Summary);
        assert!(out.contains("ASSISTANT #0"));
        assert!(out.contains("thinking:"));
        assert!(out.contains("tool use: Bash [call-1234567…]"));
        assert!(out.contains("cmd: echo hi"));
        assert!(out.contains("tool result [call-1234567…] · ok"));
        assert!(out.contains("image: image/png (path=/tmp/a.png)"));
        assert!(out.contains("attachment: name=log.txt · path=/tmp/log.txt · mime=text/plain"));
        assert!(out.contains("patch: +1 -1"));
        assert!(out.contains("other: opencode references"));
    }
}
