//! Full-text search across all known sessions.

use std::sync::atomic::{AtomicBool, Ordering};

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchProgress {
    pub processed: usize,
    pub total: usize,
    pub hits: usize,
    pub load_errors: usize,
}

/// Search across all enabled providers for sessions whose rendered full
/// session data contains `query`.
pub fn search_all(query: &str, case_insensitive: bool) -> Result<Vec<SearchHit>> {
    search_all_with_cancel(query, case_insensitive, None)
}

pub fn search_all_with_cancel(
    query: &str,
    case_insensitive: bool,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<SearchHit>> {
    search_impl(None, query, case_insensitive, cancel, None)
}

pub fn search_all_with_cancel_and_progress(
    query: &str,
    case_insensitive: bool,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(SearchProgress),
) -> Result<Vec<SearchHit>> {
    search_impl(None, query, case_insensitive, cancel, Some(&mut progress))
}

/// Search only the supplied sessions, without running provider discovery.
pub fn search_infos_with_cancel(
    infos: Vec<SessionInfo>,
    query: &str,
    case_insensitive: bool,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<SearchHit>> {
    search_impl(Some(infos), query, case_insensitive, cancel, None)
}

/// Search only the supplied sessions and report progress to the caller.
pub fn search_infos_with_cancel_and_progress(
    infos: Vec<SessionInfo>,
    query: &str,
    case_insensitive: bool,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(SearchProgress),
) -> Result<Vec<SearchHit>> {
    search_impl(
        Some(infos),
        query,
        case_insensitive,
        cancel,
        Some(&mut progress),
    )
}

fn search_impl(
    infos: Option<Vec<SessionInfo>>,
    query: &str,
    case_insensitive: bool,
    cancel: Option<&AtomicBool>,
    mut progress: Option<&mut dyn FnMut(SearchProgress)>,
) -> Result<Vec<SearchHit>> {
    let scope = if infos.is_some() { "provided" } else { "all" };
    crate::debug::log(
        "search_library_start",
        serde_json::json!({
            "query_len": query.chars().count(),
            "case_insensitive": case_insensitive,
            "scope": scope,
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
    let infos = match infos {
        Some(infos) => infos,
        None => super::list_all()?,
    };
    crate::debug::log(
        "search_library_sessions_loaded",
        serde_json::json!({
            "sessions": infos.len(),
            "scope": scope,
        }),
    );
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut processed = 0usize;
    let mut load_errors = 0usize;
    let total = infos.len();
    if let Some(progress) = progress.as_deref_mut() {
        progress(SearchProgress {
            processed: 0,
            total,
            hits: 0,
            load_errors,
        });
    }
    for info in infos {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            crate::debug::log(
                "search_library_cancelled",
                serde_json::json!({
                    "hits": hits.len(),
                    "load_errors": load_errors,
                }),
            );
            return Ok(hits);
        }
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
                processed = processed.saturating_add(1);
                if let Some(progress) = progress.as_deref_mut() {
                    progress(SearchProgress {
                        processed,
                        total,
                        hits: hits.len(),
                        load_errors,
                    });
                }
                continue;
            }
        };
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            crate::debug::log(
                "search_library_cancelled",
                serde_json::json!({
                    "hits": hits.len(),
                    "load_errors": load_errors,
                }),
            );
            return Ok(hits);
        }
        let text = render(&session, Mode::Full);
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            crate::debug::log(
                "search_library_cancelled",
                serde_json::json!({
                    "hits": hits.len(),
                    "load_errors": load_errors,
                }),
            );
            return Ok(hits);
        }
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
        processed = processed.saturating_add(1);
        if let Some(progress) = progress.as_deref_mut() {
            progress(SearchProgress {
                processed,
                total,
                hits: hits.len(),
                load_errors,
            });
        }
    }
    // Newest sessions first.
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.info.updated_at_epoch_s));
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
    let q = if ci {
        query.to_lowercase()
    } else {
        query.to_string()
    };
    let (match_start, match_end) = if ci {
        let (haystack, ranges) = lowercase_with_original_ranges(text);
        match haystack.find(&q) {
            Some(p) if !q.is_empty() => {
                let start = ranges.get(p).map(|range| range.0).unwrap_or(0);
                let end = ranges
                    .get(p + q.len().saturating_sub(1))
                    .map(|range| range.1)
                    .unwrap_or(text.len());
                (start, end)
            }
            _ => return text.chars().take(80).collect(),
        }
    } else {
        match text.find(query) {
            Some(p) => (p, p + query.len()),
            None => return text.chars().take(80).collect(),
        }
    };
    let start = match_start.saturating_sub(CONTEXT);
    let mut start = start;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let end = match_end.saturating_add(CONTEXT).min(text.len());
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

fn lowercase_with_original_ranges(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut folded = String::new();
    let mut ranges = Vec::new();
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        for lower in ch.to_lowercase() {
            let mut buf = [0u8; 4];
            let lower = lower.encode_utf8(&mut buf);
            folded.push_str(lower);
            for _ in 0..lower.len() {
                ranges.push((start, end));
            }
        }
    }
    (folded, ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universal::Provider;

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

    #[test]
    fn supplied_infos_do_not_trigger_global_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let info = SessionInfo {
            provider: Provider::Codex,
            session_id: "missing-session".into(),
            cwd: "/missing".into(),
            source: dir.path().join("missing.jsonl"),
            updated_at_epoch_s: 0,
            title: None,
            relation: None,
        };
        let mut updates = Vec::new();

        let hits =
            search_infos_with_cancel_and_progress(vec![info], "needle", true, None, |progress| {
                updates.push(progress)
            })
            .unwrap();

        assert!(hits.is_empty());
        assert_eq!(
            updates,
            vec![
                SearchProgress {
                    processed: 0,
                    total: 1,
                    hits: 0,
                    load_errors: 0,
                },
                SearchProgress {
                    processed: 1,
                    total: 1,
                    hits: 0,
                    load_errors: 1,
                },
            ]
        );
    }
}
