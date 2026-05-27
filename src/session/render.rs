//! Render a UniversalSession as a human-readable transcript.
//!
//! Two modes:
//!  - `Full`: every message, every block, no truncation.
//!  - `Summary`: skip meta, cap each text block at PREVIEW_BLOCK_CAP chars.

use crate::universal::{ContentBlock, Role, UniversalSession};

pub const PREVIEW_BLOCK_CAP: usize = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Full,
    Summary,
}

pub fn render(session: &UniversalSession, mode: Mode) -> String {
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
        ContentBlock, MessageFlags, Provenance, Role, UMessage, UniversalSession,
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
        assert!(out.contains("[user"));
        assert!(out.contains("hi"));
    }
}
