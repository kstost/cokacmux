//! Post-conversion validation helpers.

use std::collections::HashSet;

use super::schema::{ContentBlock, UniversalSession};
use crate::error::{ConvertError, Result};

/// Check that every `ToolUse.call_id` has a matching `ToolResult.call_id`
/// somewhere later in the message stream. Returns Err in strict mode if
/// unmatched, otherwise returns the list of unmatched call ids.
pub fn unmatched_tool_calls(session: &UniversalSession) -> Vec<String> {
    let mut uses: HashSet<String> = HashSet::new();
    let mut results: HashSet<String> = HashSet::new();
    for m in &session.messages {
        for b in &m.content {
            match b {
                ContentBlock::ToolUse { call_id, .. } => {
                    uses.insert(call_id.clone());
                }
                ContentBlock::ToolResult { call_id, .. } => {
                    results.insert(call_id.clone());
                }
                _ => {}
            }
        }
    }
    uses.difference(&results).cloned().collect()
}

pub fn check_strict(session: &UniversalSession) -> Result<()> {
    let unmatched = unmatched_tool_calls(session);
    if !unmatched.is_empty() {
        return Err(ConvertError::Validation(format!(
            "{} tool_use(s) without matching tool_result: {:?}",
            unmatched.len(),
            unmatched
        )));
    }
    Ok(())
}
