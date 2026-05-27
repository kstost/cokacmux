//! Full-text search across all known sessions.

use crate::error::Result;
use crate::providers::discovery::SessionInfo;

use super::render::{render, Mode};

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub info: SessionInfo,
    /// Number of non-overlapping matches in the rendered full session.
    pub matches: usize,
    /// A short snippet (the first match, capped).
    pub snippet: String,
}

/// Search across all enabled providers for sessions whose rendered full
/// session data contains `query`.
pub fn search_all(query: &str, case_insensitive: bool) -> Result<Vec<SearchHit>> {
    crate::debug::log(
        "search_library_start",
        serde_json::json!({
            "query_len": query.chars().count(),
            "case_insensitive": case_insensitive,
        }),
    );
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let q = if case_insensitive {
        query.to_lowercase()
    } else {
        query.to_string()
    };
    let infos = super::list_all()?;
    crate::debug::log(
        "search_library_sessions_loaded",
        serde_json::json!({
            "sessions": infos.len(),
        }),
    );
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut load_errors = 0usize;
    for info in infos {
        let session = match super::load(&info) {
            Ok(session) => session,
            Err(error) => {
                load_errors = load_errors.saturating_add(1);
                crate::debug::log(
                    "search_library_load_error",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "error": error.to_string(),
                    }),
                );
                continue;
            }
        };
        let text = render(&session, Mode::Full);
        let haystack = if case_insensitive {
            text.to_lowercase()
        } else {
            text.clone()
        };
        let count = count_matches(&haystack, &q);
        if count > 0 {
            hits.push(SearchHit {
                info,
                matches: count,
                snippet: snippet_for(&text, query, case_insensitive),
            });
        }
    }
    // Newest sessions first.
    hits.sort_by(|a, b| b.info.updated_at_epoch_s.cmp(&a.info.updated_at_epoch_s));
    crate::debug::log(
        "search_library_ok",
        serde_json::json!({
            "hits": hits.len(),
            "load_errors": load_errors,
        }),
    );
    Ok(hits)
}

fn count_matches(haystack: &str, query: &str) -> usize {
    if query.is_empty() {
        return 0;
    }
    haystack.match_indices(query).count()
}

fn snippet_for(text: &str, query: &str, ci: bool) -> String {
    const CONTEXT: usize = 40;
    let haystack = if ci {
        text.to_lowercase()
    } else {
        text.to_string()
    };
    let q = if ci {
        query.to_lowercase()
    } else {
        query.to_string()
    };
    let i = match haystack.find(&q) {
        Some(p) => p,
        None => return text.chars().take(80).collect(),
    };
    let start = i.saturating_sub(CONTEXT);
    let mut start = start;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let end = (i + query.len() + CONTEXT).min(text.len());
    let mut end = end;
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < text.len() { "…" } else { "" };
    format!(
        "{}{}{}",
        prefix,
        &text[start..end].replace('\n', " "),
        suffix
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_matches_counts_non_overlapping_hits() {
        assert_eq!(count_matches("aaaa", "aa"), 2);
        assert_eq!(count_matches("abc", "z"), 0);
        assert_eq!(count_matches("abc", ""), 0);
    }

    #[test]
    fn snippet_uses_original_text_case() {
        let snippet = snippet_for("Hello Tool Result", "tool", true);
        assert!(snippet.contains("Tool"));
    }
}
