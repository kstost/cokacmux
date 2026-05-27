//! Claude tool-results sidecar: large tool outputs get written to
//! `<session-uuid>/tool-results/<random>.txt` instead of being inlined.
//!
//! In the JSONL line a sidecar reference looks like this (observed):
//!   "Full output saved to: /home/.../tool-results/<random>.txt"
//! When `ClaudeReadCtx::inline_tool_results` is enabled we detect that
//! pattern and replace the truncated preview with the full file contents.

use std::path::{Path, PathBuf};

/// If `text` mentions a "Full output saved to:" sidecar path, return it.
pub fn extract_sidecar_ref(text: &str) -> Option<PathBuf> {
    let needle = "Full output saved to: ";
    let i = text.find(needle)?;
    let after = &text[i + needle.len()..];
    let end = after
        .find(|c: char| c == '\n' || c == '\r')
        .unwrap_or(after.len());
    let path = after[..end].trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

pub fn read_sidecar(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_path() {
        let s = "Output too large (32KB). Full output saved to: /tmp/x/y.txt\n\nPreview...";
        assert_eq!(extract_sidecar_ref(s), Some(PathBuf::from("/tmp/x/y.txt")));
    }

    #[test]
    fn no_match() {
        assert_eq!(extract_sidecar_ref("just text"), None);
    }
}
