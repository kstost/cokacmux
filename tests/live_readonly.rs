//! Read-only live storage checks.
//!
//! These tests inspect real local Claude/Codex/OpenCode session artifacts,
//! but write only to tempdirs. They are ignored by default because they depend
//! on the developer machine having live agent data.

#![cfg(all(
    feature = "claude",
    feature = "codex",
    feature = "opencode",
    feature = "discovery"
))]

use std::path::{Path, PathBuf};

use cokacmux::session::{
    clone::ArtifactPath,
    install::{install_universal_session, InstallSessionOpts},
};
use cokacmux::{
    providers, read_session, wrap_session_for_context_convert, ContentBlock, Provider, Role,
    SessionSource, UniversalSession, CONTEXT_ACK, CONTEXT_CONTINUATION_PROMPT,
};

#[test]
#[ignore]
fn live_recent_sessions_install_as_context_wrappers_in_all_target_native_layouts() {
    let limit = std::env::var("COKACMUX_LIVE_READONLY_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(3);

    let samples = live_samples(limit);
    if samples.is_empty() {
        eprintln!("no live session samples found; skip");
        return;
    }

    for sample in samples {
        let source = read_session(sample.provider, &sample.source).unwrap_or_else(|error| {
            panic!(
                "failed to parse live {} sample {}: {error}",
                sample.provider.as_str(),
                sample.label
            )
        });
        assert_live_session_shape(sample.provider, &source);

        for target_provider in [Provider::Claude, Provider::Codex, Provider::OpenCode] {
            if target_provider == sample.provider {
                continue;
            }
            let wrapped = wrap_session_for_context_convert(&source, target_provider);
            assert_wrapper_shape(sample.provider, target_provider, &source, &wrapped);

            let temp = tempfile::tempdir().unwrap();
            let opts = install_opts_for(target_provider, temp.path());
            let report = install_universal_session(target_provider, &wrapped, &opts)
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to install {} -> {} wrapper for {}: {error}",
                        sample.provider.as_str(),
                        target_provider.as_str(),
                        sample.label
                    )
                });
            assert!(
                report.validation.ok,
                "{} -> {} wrapper failed native validation for {}: {}",
                sample.provider.as_str(),
                target_provider.as_str(),
                sample.label,
                report.validation.failure_summary()
            );
            assert_installed_wrapper_is_two_visible_messages(
                target_provider,
                &report.artifact,
                &report.session_id,
            );
        }
    }
}

#[derive(Debug)]
struct LiveSample {
    provider: Provider,
    label: String,
    source: SessionSource,
}

fn live_samples(limit: usize) -> Vec<LiveSample> {
    let mut out = Vec::new();
    out.extend(recent_claude_samples(limit));
    out.extend(recent_codex_samples(limit));
    out.extend(recent_opencode_samples(limit));
    out
}

fn recent_claude_samples(limit: usize) -> Vec<LiveSample> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let root = home.join(".claude").join("projects");
    if !root.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_jsonl_files(&root, &mut files);
    files.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .map(std::cmp::Reverse)
    });
    files
        .into_iter()
        .take(limit)
        .map(|path| LiveSample {
            provider: Provider::Claude,
            label: path.display().to_string(),
            source: SessionSource::Path(path),
        })
        .collect()
}

fn recent_codex_samples(limit: usize) -> Vec<LiveSample> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let root = home.join(".codex").join("sessions");
    if !root.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_jsonl_files(&root, &mut files);
    files.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .map(std::cmp::Reverse)
    });
    files
        .into_iter()
        .take(limit)
        .map(|path| LiveSample {
            provider: Provider::Codex,
            label: path.display().to_string(),
            source: SessionSource::Path(path),
        })
        .collect()
}

fn recent_opencode_samples(limit: usize) -> Vec<LiveSample> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let db_path = home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db");
    if !db_path.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT id FROM session ORDER BY time_updated DESC LIMIT ?1")
        .unwrap();
    stmt.query_map(rusqlite::params![limit as i64], |row| {
        row.get::<_, String>(0)
    })
    .unwrap()
    .map(|row| {
        let session_id = row.unwrap();
        LiveSample {
            provider: Provider::OpenCode,
            label: format!("{}#{}", db_path.display(), session_id),
            source: SessionSource::OpenCodeDb {
                db_path: db_path.clone(),
                session_id,
            },
        }
    })
    .collect()
}

fn collect_jsonl_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn assert_live_session_shape(provider: Provider, session: &UniversalSession) {
    assert_eq!(session.origin.provider, Some(provider));
    assert!(
        !session.session_id.trim().is_empty(),
        "{} live sample should have a session id",
        provider.as_str()
    );
    assert!(
        !session.cwd.trim().is_empty(),
        "{} live sample should have cwd",
        provider.as_str()
    );
    assert!(
        session
            .messages
            .iter()
            .any(|message| !message.flags.is_meta),
        "{} live sample should contain visible conversation messages",
        provider.as_str()
    );
}

fn install_opts_for(provider: Provider, root: &Path) -> InstallSessionOpts {
    let mut opts = InstallSessionOpts::default();
    match provider {
        Provider::Claude => {
            opts.claude_home = Some(root.join(".claude"));
        }
        Provider::Codex => {
            let state_path = root.join("state_5.sqlite");
            create_codex_threads_table(&state_path);
            opts.codex_home = Some(root.join(".codex"));
            opts.codex_state_5_path = Some(state_path);
        }
        Provider::OpenCode => {
            opts.opencode_db_path = Some(root.join("opencode.db"));
        }
    }
    opts
}

fn create_codex_threads_table(path: &Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE threads (
            id TEXT PRIMARY KEY,
            rollout_path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            source TEXT NOT NULL,
            model_provider TEXT NOT NULL,
            cwd TEXT NOT NULL,
            title TEXT NOT NULL,
            sandbox_policy TEXT NOT NULL,
            approval_mode TEXT NOT NULL,
            tokens_used INTEGER NOT NULL DEFAULT 0,
            has_user_event INTEGER NOT NULL DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0,
            archived_at INTEGER,
            git_sha TEXT,
            git_branch TEXT,
            git_origin_url TEXT,
            cli_version TEXT NOT NULL DEFAULT '',
            first_user_message TEXT NOT NULL DEFAULT '',
            agent_nickname TEXT,
            agent_role TEXT,
            memory_mode TEXT NOT NULL DEFAULT 'enabled',
            model TEXT,
            reasoning_effort TEXT,
            agent_path TEXT,
            created_at_ms INTEGER,
            updated_at_ms INTEGER,
            thread_source TEXT,
            preview TEXT NOT NULL DEFAULT ''
        );
    "#,
    )
    .unwrap();
}

fn assert_wrapper_shape(
    from: Provider,
    to: Provider,
    source: &UniversalSession,
    wrapped: &UniversalSession,
) {
    assert_eq!(wrapped.origin.provider, Some(to));
    assert_eq!(wrapped.messages.len(), 2);
    assert_ne!(wrapped.session_id, source.session_id);
    assert_eq!(
        wrapped.updated_at.unwrap(),
        wrapped.created_at.unwrap() + chrono::Duration::seconds(1)
    );
    let user = &wrapped.messages[0];
    let assistant = &wrapped.messages[1];
    assert_eq!(user.role, Role::User);
    assert_eq!(assistant.role, Role::Assistant);
    assert_eq!(assistant.parent_id.as_deref(), Some(user.id.as_str()));
    let user_text = only_text(user);
    assert!(user_text.contains(&format!(
        "=== {} ({}) ===",
        source.session_id,
        from.as_str()
    )));
    assert!(user_text.ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert_eq!(only_text(assistant), CONTEXT_ACK);
}

fn assert_installed_wrapper_is_two_visible_messages(
    provider: Provider,
    artifact: &ArtifactPath,
    session_id: &str,
) {
    let session = match (provider, artifact) {
        (Provider::Claude, ArtifactPath::File(path)) => {
            providers::claude::from_file(path, &Default::default()).unwrap()
        }
        (Provider::Codex, ArtifactPath::File(path)) => providers::codex::from_file(path).unwrap(),
        (Provider::OpenCode, ArtifactPath::OpenCodeDb { db_path, .. }) => {
            providers::opencode::from_db_path(db_path, session_id).unwrap()
        }
        _ => panic!("provider/artifact mismatch: {provider:?} {artifact:?}"),
    };
    assert_eq!(session.session_id, session_id);
    let visible = session
        .messages
        .iter()
        .filter(|message| !message.flags.is_meta)
        .collect::<Vec<_>>();
    assert_eq!(
        visible.len(),
        2,
        "{} installed wrapper should expose exactly two visible messages",
        provider.as_str()
    );
    assert_eq!(visible[0].role, Role::User);
    assert_eq!(visible[1].role, Role::Assistant);
    assert!(only_text(visible[0]).ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert_eq!(only_text(visible[1]), CONTEXT_ACK);
}

fn only_text(message: &cokacmux::UMessage) -> &str {
    let texts = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(texts.len(), 1);
    texts[0]
}
