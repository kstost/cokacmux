//! Post-conversion validation helpers.

use super::schema::{ContentBlock, UniversalSession};
use crate::error::{ConvertError, Result};

/// Check that every `ToolUse.call_id` has a matching `ToolResult.call_id`
/// later in the same order. Returns problematic call ids for unmatched,
/// orphaned, or out-of-order tool results.
pub fn unmatched_tool_calls(session: &UniversalSession) -> Vec<String> {
    let mut pending: Vec<String> = Vec::new();
    let mut problems = Vec::new();
    for m in &session.messages {
        for b in &m.content {
            match b {
                ContentBlock::ToolUse { call_id, .. } => {
                    pending.push(call_id.clone());
                }
                ContentBlock::ToolResult { call_id, .. } => {
                    match pending.iter().position(|pending_id| pending_id == call_id) {
                        Some(0) => {
                            pending.remove(0);
                        }
                        Some(pos) => {
                            problems.push(call_id.clone());
                            pending.remove(pos);
                        }
                        None => problems.push(call_id.clone()),
                    }
                }
                _ => {}
            }
        }
    }
    problems.extend(pending);
    problems
}

pub fn check_strict(session: &UniversalSession) -> Result<()> {
    let unmatched = unmatched_tool_calls(session);
    if !unmatched.is_empty() {
        return Err(ConvertError::Validation(format!(
            "{} unmatched or out-of-order tool call/result id(s): {:?}",
            unmatched.len(),
            unmatched
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::universal::{Provenance, Provider, Role, UMessage};

    fn session_with(messages: Vec<UMessage>) -> UniversalSession {
        let mut session = UniversalSession::new("s1", Provider::Codex, "/tmp");
        session.messages = messages;
        session
    }

    fn msg(index: u32, content: Vec<ContentBlock>) -> UMessage {
        UMessage {
            id: format!("m{index}"),
            parent_id: None,
            index,
            timestamp: None,
            role: Role::Assistant,
            model: None,
            usage: None,
            stop_reason: None,
            content,
            flags: Default::default(),
            provenance: Provenance {
                source_event_type: "test".into(),
                raw: json!({}),
            },
            extras: Default::default(),
        }
    }

    #[test]
    fn accepts_ordered_tool_results() {
        let session = session_with(vec![
            msg(
                0,
                vec![
                    ContentBlock::tool_use("a", "shell", json!({})),
                    ContentBlock::tool_use("b", "shell", json!({})),
                ],
            ),
            msg(
                1,
                vec![
                    ContentBlock::tool_result("a", json!("ok"), false),
                    ContentBlock::tool_result("b", json!("ok"), false),
                ],
            ),
        ]);

        assert!(unmatched_tool_calls(&session).is_empty());
    }

    #[test]
    fn flags_orphan_and_out_of_order_tool_results() {
        let session = session_with(vec![
            msg(
                0,
                vec![
                    ContentBlock::tool_use("a", "shell", json!({})),
                    ContentBlock::tool_use("b", "shell", json!({})),
                ],
            ),
            msg(
                1,
                vec![
                    ContentBlock::tool_result("b", json!("early"), false),
                    ContentBlock::tool_result("c", json!("orphan"), false),
                    ContentBlock::tool_result("a", json!("ok"), false),
                ],
            ),
        ]);

        assert_eq!(
            unmatched_tool_calls(&session),
            vec!["b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn duplicate_tool_uses_need_duplicate_results() {
        let session = session_with(vec![
            msg(
                0,
                vec![
                    ContentBlock::tool_use("a", "shell", json!({})),
                    ContentBlock::tool_use("a", "shell", json!({})),
                ],
            ),
            msg(1, vec![ContentBlock::tool_result("a", json!("ok"), false)]),
        ]);

        assert_eq!(unmatched_tool_calls(&session), vec!["a".to_string()]);
    }
}
