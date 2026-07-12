//! Install helpers: write into each provider's on-disk layout using an
//! isolated temporary directory as the "home". Verifies that round-tripping
//! through an install produces the expected file/db layout.

#![cfg(feature = "discovery")]

use std::path::{Path, PathBuf};

#[cfg(feature = "gjc")]
use cokacmux::providers::gjc;
#[cfg(feature = "opencode")]
use cokacmux::providers::opencode;
#[cfg(feature = "pi")]
use cokacmux::providers::pi;
use cokacmux::providers::{claude, codex};
use cokacmux::session::{
    clone::ArtifactPath,
    install::{install_universal_session, InstallSessionOpts},
    native_validate,
};
use cokacmux::{
    wrap_session_for_context_convert, ContentBlock, Provider, Role, CONTEXT_ACK,
    CONTEXT_CONTINUATION_PROMPT,
};

fn claude_fixture() -> &'static str {
    r#"{"type":"user","sessionId":"installtest-1","cwd":"/tmp/abc","timestamp":"2026-05-20T01:00:00.000Z","uuid":"019e0000-0000-7000-8000-000000000001","parentUuid":null,"message":{"role":"user","content":"hi"}}
{"type":"assistant","sessionId":"installtest-1","cwd":"/tmp/abc","timestamp":"2026-05-20T01:00:01.000Z","uuid":"019e0000-0000-7000-8000-000000000002","parentUuid":"019e0000-0000-7000-8000-000000000001","message":{"role":"assistant","content":[{"type":"text","text":"yo"}]}}
"#
}

fn codex_fixture() -> &'static str {
    r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"installtest-codex","cwd":"/tmp/abc"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}
"#
}

#[cfg(feature = "pi")]
fn pi_fixture() -> &'static str {
    r#"{"type":"session","version":3,"id":"019e0000-0000-7000-8000-0000000000aa","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp/abc"}
{"type":"message","id":"019e0000-0000-7000-8000-0000000000ab","parentId":null,"timestamp":"2026-05-20T01:00:00.500Z","message":{"role":"user","content":"hi","timestamp":1779240000500}}
{"type":"message","id":"019e0000-0000-7000-8000-0000000000ac","parentId":"019e0000-0000-7000-8000-0000000000ab","timestamp":"2026-05-20T01:00:01.500Z","message":{"role":"assistant","content":[{"type":"text","text":"yo"}],"provider":"openai","model":"gpt-5.5","usage":{"input":1,"output":1,"totalTokens":2,"cost":{"total":0}},"stopReason":"stop","timestamp":1779240001500}}
"#
}

#[cfg(feature = "gjc")]
fn gjc_fixture() -> &'static str {
    r#"{"type":"session","version":3,"id":"019e0000-0000-7000-8000-0000000000ba","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp/abc","title":"GJC fixture","titleSource":"user"}
{"type":"message","id":"019e0000-0000-7000-8000-0000000000bb","parentId":null,"timestamp":"2026-05-20T01:00:00.500Z","message":{"role":"user","content":"hi","timestamp":1779240000500}}
{"type":"message","id":"019e0000-0000-7000-8000-0000000000bc","parentId":"019e0000-0000-7000-8000-0000000000bb","timestamp":"2026-05-20T01:00:01.500Z","message":{"role":"assistant","content":[{"type":"text","text":"yo"}],"provider":"openai","model":"gpt-5.5","usage":{"input":1,"output":1,"totalTokens":2,"cost":{"total":0}},"stopReason":"stop","timestamp":1779240001500}}
"#
}

#[cfg(feature = "opencode")]
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

fn text_messages_for_role(session: &cokacmux::UniversalSession, role: Role) -> Vec<&str> {
    session
        .messages
        .iter()
        .filter(|message| message.role == role)
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn jsonl_files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files);
    files
}

fn collect_jsonl_files(root: &Path, files: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

#[test]
fn claude_install_into_tempdir() {
    let tmp = tempfile::tempdir().unwrap();
    let session = claude::from_jsonl_str(claude_fixture(), &Default::default()).unwrap();
    let report = claude::install::install_to_user_dir(
        &session,
        &claude::install::InstallOpts {
            claude_home: Some(tmp.path().to_path_buf()),
            overwrite: false,
        },
    )
    .unwrap();
    // Expected layout
    let expected_dir = tmp.path().join("projects").join("-tmp-abc");
    let expected_file = expected_dir.join("installtest-1.jsonl");
    assert_eq!(report.project_dir, expected_dir);
    assert_eq!(report.jsonl_path, expected_file);
    assert!(expected_file.exists());
    assert!(report.bytes_written > 0);
    let validation = native_validate::validate_clone_artifact(
        Provider::Claude,
        &session.session_id,
        &ArtifactPath::File(report.jsonl_path.clone()),
    )
    .unwrap();
    assert!(validation.ok, "{:?}", validation);

    // Re-install without overwrite must fail.
    let err = claude::install::install_to_user_dir(
        &session,
        &claude::install::InstallOpts {
            claude_home: Some(tmp.path().to_path_buf()),
            overwrite: false,
        },
    );
    assert!(err.is_err());

    // With overwrite=true it succeeds.
    claude::install::install_to_user_dir(
        &session,
        &claude::install::InstallOpts {
            claude_home: Some(tmp.path().to_path_buf()),
            overwrite: true,
        },
    )
    .unwrap();
}

#[test]
fn context_wrapper_install_into_claude_tempdir_validates_native_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let source = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::Claude);

    let report = install_universal_session(
        Provider::Claude,
        &converted,
        &InstallSessionOpts {
            claude_home: Some(tmp.path().to_path_buf()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.provider, Provider::Claude);
    assert_eq!(report.session_id, converted.session_id);
    assert!(report.validation.ok, "{:?}", report.validation);
    let ArtifactPath::File(path) = &report.artifact else {
        panic!("expected Claude file artifact: {:?}", report.artifact);
    };
    assert_eq!(
        path.file_stem().and_then(|stem| stem.to_str()),
        Some(converted.session_id.as_str())
    );

    let back = claude::from_file(path, &Default::default()).unwrap();
    let user_texts = text_messages_for_role(&back, Role::User);
    assert_eq!(user_texts.len(), 1);
    assert!(user_texts[0].ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert!(user_texts[0].contains("installtest-codex"));
    let assistant_texts = text_messages_for_role(&back, Role::Assistant);
    assert_eq!(assistant_texts, vec![CONTEXT_ACK]);
}

#[test]
fn codex_install_into_tempdir() {
    let tmp = tempfile::tempdir().unwrap();
    let session = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let report = codex::install::install_to_user_dir(
        &session,
        &codex::install::InstallOpts {
            codex_home: Some(tmp.path().to_path_buf()),
            overwrite: false,
            update_index: false, // no state_5.sqlite in tempdir
            state_5_path: None,
        },
    )
    .unwrap();
    let p = &report.rollout_path;
    assert!(p.exists());
    let name = p.file_name().and_then(|n| n.to_str()).unwrap();
    assert!(name.starts_with("rollout-"));
    assert!(name.ends_with("installtest-codex.jsonl"));
    // Date folders
    let parent = p.parent().unwrap();
    assert!(parent.starts_with(tmp.path().join("sessions")));
    assert!(!report.indexed); // we asked it not to
}

#[test]
fn codex_install_validation_failure_removes_rollout_when_required_index_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let source = claude::from_jsonl_str(claude_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::Codex);

    let err = install_universal_session(
        Provider::Codex,
        &converted,
        &InstallSessionOpts {
            codex_home: Some(tmp.path().to_path_buf()),
            ..Default::default()
        },
    )
    .expect_err("missing required state_5.sqlite index should fail validation");

    let message = err.to_string();
    assert!(message.contains("state_5_index_present"), "{message}");
    let jsonls = jsonl_files_under(&tmp.path().join("sessions"));
    assert!(
        jsonls.is_empty(),
        "failed install should remove rollout files: {jsonls:?}"
    );
}

#[cfg(feature = "pi")]
#[test]
fn pi_install_into_tempdir() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().join(".pi").join("agent");
    let session = pi::from_jsonl_str(pi_fixture(), &Default::default()).unwrap();
    let report = pi::install::install_to_user_dir(
        &session,
        &pi::install::InstallOpts {
            pi_agent_dir: Some(agent_dir.clone()),
            pi_session_dir: None,
            overwrite: false,
        },
    )
    .unwrap();

    let expected_dir = agent_dir
        .join("sessions")
        .join(pi::encoded_cwd_dir("/tmp/abc"));
    assert_eq!(report.jsonl_path.parent(), Some(expected_dir.as_path()));
    assert!(report.jsonl_path.exists());
    assert!(report
        .jsonl_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(&format!("_{}.jsonl", session.session_id))));
    let validation = native_validate::validate_clone_artifact(
        Provider::Pi,
        &session.session_id,
        &ArtifactPath::File(report.jsonl_path.clone()),
    )
    .unwrap();
    assert!(validation.ok, "{:?}", validation);

    let err = pi::install::install_to_user_dir(
        &session,
        &pi::install::InstallOpts {
            pi_agent_dir: Some(agent_dir.clone()),
            pi_session_dir: None,
            overwrite: false,
        },
    )
    .expect_err("re-install without overwrite should fail");
    assert!(err.to_string().contains("already exists"), "{err}");

    pi::install::install_to_user_dir(
        &session,
        &pi::install::InstallOpts {
            pi_agent_dir: Some(agent_dir),
            pi_session_dir: None,
            overwrite: true,
        },
    )
    .unwrap();
}

#[cfg(feature = "pi")]
#[test]
fn pi_install_rejects_session_ids_that_can_escape_the_session_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().join(".pi").join("agent");
    let mut session = pi::from_jsonl_str(pi_fixture(), &Default::default()).unwrap();
    session.session_id = "../../../outside".into();

    let error = pi::install::install_to_user_dir(
        &session,
        &pi::install::InstallOpts {
            pi_agent_dir: Some(agent_dir.clone()),
            ..Default::default()
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("safe filename component"));
    assert!(
        !agent_dir.exists(),
        "validation must happen before creating directories"
    );
    assert!(!tmp.path().join("outside.jsonl").exists());
}

#[cfg(feature = "pi")]
#[test]
fn context_wrapper_install_into_pi_tempdir_validates_native_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().join(".pi").join("agent");
    let source = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::Pi);

    let report = install_universal_session(
        Provider::Pi,
        &converted,
        &InstallSessionOpts {
            pi_agent_dir: Some(agent_dir.clone()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.provider, Provider::Pi);
    assert_eq!(report.session_id, converted.session_id);
    assert!(report.validation.ok, "{:?}", report.validation);
    let ArtifactPath::File(path) = &report.artifact else {
        panic!("expected Pi file artifact: {:?}", report.artifact);
    };
    assert!(path.starts_with(agent_dir.join("sessions")));
    assert!(path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(&format!("_{}.jsonl", converted.session_id))));

    let back = pi::from_file(path).unwrap();
    let user_texts = text_messages_for_role(&back, Role::User);
    assert_eq!(user_texts.len(), 1);
    assert!(user_texts[0].ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert!(user_texts[0].contains("installtest-codex"));
    let assistant_texts = text_messages_for_role(&back, Role::Assistant);
    assert_eq!(assistant_texts, vec![CONTEXT_ACK]);
}

#[cfg(feature = "gjc")]
#[test]
fn gjc_install_into_tempdir() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().join(".gjc").join("agent");
    let session = gjc::from_jsonl_str(gjc_fixture(), &Default::default()).unwrap();
    let report = gjc::install::install_to_user_dir(
        &session,
        &gjc::install::InstallOpts {
            gjc_agent_dir: Some(agent_dir.clone()),
            gjc_session_dir: None,
            overwrite: false,
        },
    )
    .unwrap();

    let expected_dir = agent_dir
        .join("sessions")
        .join(gjc::encoded_cwd_dir("/tmp/abc"));
    assert_eq!(report.jsonl_path.parent(), Some(expected_dir.as_path()));
    assert!(report.jsonl_path.exists());
    assert!(report
        .jsonl_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(&format!("_{}.jsonl", session.session_id))));
    let validation = native_validate::validate_clone_artifact(
        Provider::Gjc,
        &session.session_id,
        &ArtifactPath::File(report.jsonl_path.clone()),
    )
    .unwrap();
    assert!(validation.ok, "{:?}", validation);

    let err = gjc::install::install_to_user_dir(
        &session,
        &gjc::install::InstallOpts {
            gjc_agent_dir: Some(agent_dir.clone()),
            gjc_session_dir: None,
            overwrite: false,
        },
    )
    .expect_err("re-install without overwrite should fail");
    assert!(err.to_string().contains("already exists"), "{err}");

    gjc::install::install_to_user_dir(
        &session,
        &gjc::install::InstallOpts {
            gjc_agent_dir: Some(agent_dir),
            gjc_session_dir: None,
            overwrite: true,
        },
    )
    .unwrap();
}

#[cfg(feature = "gjc")]
#[test]
fn gjc_install_rejects_session_ids_that_can_escape_the_session_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().join(".gjc").join("agent");
    let mut session = gjc::from_jsonl_str(gjc_fixture(), &Default::default()).unwrap();
    session.session_id = "../../../outside".into();

    let error = gjc::install::install_to_user_dir(
        &session,
        &gjc::install::InstallOpts {
            gjc_agent_dir: Some(agent_dir.clone()),
            ..Default::default()
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("safe filename component"));
    assert!(
        !agent_dir.exists(),
        "validation must happen before creating directories"
    );
    assert!(!tmp.path().join("outside.jsonl").exists());
}

#[cfg(feature = "gjc")]
#[test]
fn context_wrapper_install_into_gjc_tempdir_validates_native_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().join(".gjc").join("agent");
    let source = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::Gjc);

    let report = install_universal_session(
        Provider::Gjc,
        &converted,
        &InstallSessionOpts {
            gjc_agent_dir: Some(agent_dir.clone()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.provider, Provider::Gjc);
    assert_eq!(report.session_id, converted.session_id);
    assert!(report.validation.ok, "{:?}", report.validation);
    let ArtifactPath::File(path) = &report.artifact else {
        panic!("expected GJC file artifact: {:?}", report.artifact);
    };
    assert!(path.starts_with(agent_dir.join("sessions")));
    assert!(path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(&format!("_{}.jsonl", converted.session_id))));

    let back = gjc::from_file(path).unwrap();
    let user_texts = text_messages_for_role(&back, Role::User);
    assert_eq!(user_texts.len(), 1);
    assert!(user_texts[0].ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert!(user_texts[0].contains("installtest-codex"));
    let assistant_texts = text_messages_for_role(&back, Role::Assistant);
    assert_eq!(assistant_texts, vec![CONTEXT_ACK]);
}

#[cfg(feature = "opencode")]
#[test]
fn context_wrapper_install_into_codex_tempdir_indexes_and_validates_native_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let state_path = tmp.path().join("state_5.sqlite");
    create_codex_threads_table(&state_path);

    let source = claude::from_jsonl_str(claude_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::Codex);
    let report = install_universal_session(
        Provider::Codex,
        &converted,
        &InstallSessionOpts {
            codex_home: Some(tmp.path().to_path_buf()),
            codex_state_5_path: Some(state_path.clone()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.provider, Provider::Codex);
    assert_eq!(report.session_id, converted.session_id);
    assert!(report.validation.ok, "{:?}", report.validation);
    let ArtifactPath::File(path) = &report.artifact else {
        panic!("expected Codex file artifact: {:?}", report.artifact);
    };
    assert!(path.starts_with(tmp.path().join("sessions")));
    assert!(path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(&format!("{}.jsonl", converted.session_id))));
    assert!(report
        .validation
        .checks
        .iter()
        .any(|check| { check.name == "state_5_rollout_path_matches" && check.ok }));

    let conn = rusqlite::Connection::open(&state_path).unwrap();
    let indexed_path: String = conn
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = ?1",
            rusqlite::params![converted.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed_path, path.display().to_string());

    let back = codex::from_file(path).unwrap();
    let user_texts = text_messages_for_role(&back, Role::User);
    assert_eq!(user_texts.len(), 1);
    assert!(user_texts[0].ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert!(user_texts[0].contains("installtest-1"));
    let assistant_texts = text_messages_for_role(&back, Role::Assistant);
    assert_eq!(assistant_texts, vec![CONTEXT_ACK]);
}

#[cfg(feature = "opencode")]
#[test]
fn codex_install_validates_explicit_state_5_path_outside_codex_home() {
    let tmp = tempfile::tempdir().unwrap();
    let codex_home = tmp.path().join("codex-home");
    let state_dir = tmp.path().join("state-clone");
    std::fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state_5.sqlite");
    create_codex_threads_table(&state_path);

    let source = claude::from_jsonl_str(claude_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::Codex);
    let report = install_universal_session(
        Provider::Codex,
        &converted,
        &InstallSessionOpts {
            codex_home: Some(codex_home.clone()),
            codex_state_5_path: Some(state_path.clone()),
            ..Default::default()
        },
    )
    .unwrap();

    assert!(report.validation.ok, "{:?}", report.validation);
    assert!(!codex_home.join("state_5.sqlite").exists());
    assert!(report
        .validation
        .checks
        .iter()
        .any(|check| check.name == "state_5_rollout_path_matches" && check.ok));

    let ArtifactPath::File(path) = &report.artifact else {
        panic!("expected Codex file artifact: {:?}", report.artifact);
    };
    let conn = rusqlite::Connection::open(&state_path).unwrap();
    let indexed_path: String = conn
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = ?1",
            rusqlite::params![converted.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed_path, path.display().to_string());
}

#[cfg(feature = "opencode")]
#[test]
fn codex_failed_install_removes_explicit_state_5_override_row() {
    let tmp = tempfile::tempdir().unwrap();
    let codex_home = tmp.path().join("codex-home");
    let state_dir = tmp.path().join("state-clone");
    std::fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state_5.sqlite");
    create_codex_threads_table(&state_path);

    let source = claude::from_jsonl_str(claude_fixture(), &Default::default()).unwrap();
    let mut converted = wrap_session_for_context_convert(&source, Provider::Codex);
    converted.cwd.clear();

    let err = install_universal_session(
        Provider::Codex,
        &converted,
        &InstallSessionOpts {
            codex_home: Some(codex_home.clone()),
            codex_state_5_path: Some(state_path.clone()),
            ..Default::default()
        },
    )
    .expect_err("empty cwd should fail native validation after writing");

    let message = err.to_string();
    assert!(message.contains("session_meta_cwd_present"), "{message}");
    let jsonls = jsonl_files_under(&codex_home.join("sessions"));
    assert!(
        jsonls.is_empty(),
        "failed install should remove rollout files: {jsonls:?}"
    );

    let conn = rusqlite::Connection::open(&state_path).unwrap();
    let indexed_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM threads WHERE id = ?1",
            rusqlite::params![converted.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed_rows, 0);
}

#[cfg(feature = "opencode")]
#[test]
fn codex_context_wrapper_install_can_skip_index_validation_when_index_update_is_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let state_path = tmp.path().join("state_5.sqlite");
    create_codex_threads_table(&state_path);

    let source = claude::from_jsonl_str(claude_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::Codex);
    let report = install_universal_session(
        Provider::Codex,
        &converted,
        &InstallSessionOpts {
            codex_home: Some(tmp.path().to_path_buf()),
            codex_state_5_path: Some(state_path.clone()),
            codex_update_index: false,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(report.validation.ok, "{:?}", report.validation);
    assert!(report.validation.checks.iter().any(|check| {
        check.name == "state_5_index_required"
            && check.ok
            && check.detail.contains("disabled codex index update")
    }));

    let conn = rusqlite::Connection::open(&state_path).unwrap();
    let indexed_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM threads WHERE id = ?1",
            rusqlite::params![converted.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed_rows, 0);
}

/// Build a state_5.sqlite with the same schema columns codex uses, then
/// run install with `update_index=true` and verify the threads row is
/// well-formed (all NOT NULL columns populated, foreign-keyable to the
/// rollout file).
#[cfg(feature = "opencode")]
#[test]
fn codex_install_updates_threads_table() {
    use rusqlite::Connection;

    let tmp = tempfile::tempdir().unwrap();
    let state_path = tmp.path().join("state_5.sqlite");

    // Create the same schema codex itself uses.
    {
        let conn = Connection::open(&state_path).unwrap();
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

    let session = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let report = codex::install::install_to_user_dir(
        &session,
        &codex::install::InstallOpts {
            codex_home: Some(tmp.path().to_path_buf()),
            overwrite: false,
            update_index: true,
            state_5_path: Some(state_path.clone()),
        },
    )
    .unwrap();
    assert!(
        report.indexed,
        "should have indexed: indexed={}",
        report.indexed
    );

    // Read back the threads row and verify shape.
    let conn = Connection::open(&state_path).unwrap();
    let row: (
        String,
        String,
        i64,
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT id, rollout_path, created_at, updated_at,
                    source, model_provider, cwd, title,
                    sandbox_policy, approval_mode
             FROM threads WHERE id = ?1",
            rusqlite::params![session.session_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0, "installtest-codex");
    assert_eq!(row.1, report.rollout_path.display().to_string());
    assert!(row.2 > 0);
    assert!(row.3 > 0);
    assert_eq!(row.4, "exec"); // source
    assert!(!row.5.is_empty()); // model_provider
    assert_eq!(row.6, "/tmp/abc"); // cwd
    assert!(!row.7.is_empty()); // title
                                // sandbox_policy must be valid JSON
    let policy: serde_json::Value = serde_json::from_str(&row.8).expect("sandbox_policy JSON");
    assert!(
        policy.get("type").is_some(),
        "sandbox_policy needs type: {}",
        row.8
    );
    assert_eq!(row.9, "never"); // approval_mode, matching codex exec rollouts
    let validation = native_validate::validate_clone_artifact(
        Provider::Codex,
        &session.session_id,
        &ArtifactPath::File(report.rollout_path.clone()),
    )
    .unwrap();
    assert!(validation.ok, "{:?}", validation);
}

/// Install into a CLONE of the user's live `state_5.sqlite` to prove
/// real-schema compatibility. We never write to the user's actual file.
#[cfg(feature = "opencode")]
#[test]
#[ignore = "reads the user's live Codex state; run only in the explicit live-read gate"]
fn codex_install_against_live_state_5_clone() {
    let live = match dirs::home_dir() {
        Some(h) => h.join(".codex").join("state_5.sqlite"),
        None => return, // no home dir → skip
    };
    if !live.exists() {
        return; // no live state — skip silently
    }
    let tmp = tempfile::tempdir().unwrap();
    let cloned = tmp.path().join("state_5.sqlite");
    std::fs::copy(&live, &cloned).unwrap();

    let session = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let report = codex::install::install_to_user_dir(
        &session,
        &codex::install::InstallOpts {
            codex_home: Some(tmp.path().to_path_buf()),
            overwrite: false,
            update_index: true,
            state_5_path: Some(cloned.clone()),
        },
    )
    .unwrap();
    assert!(report.indexed);

    let conn = rusqlite::Connection::open(&cloned).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM threads WHERE id = ?1",
            rusqlite::params!["installtest-codex"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let validation = native_validate::validate_clone_artifact(
        Provider::Codex,
        &session.session_id,
        &ArtifactPath::File(report.rollout_path.clone()),
    )
    .unwrap();
    assert!(validation.ok, "{:?}", validation);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_install_into_tempdir() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("opencode.db");
    // Build a small session.
    let cf = claude_fixture();
    let mut session = claude::from_jsonl_str(cf, &Default::default()).unwrap();
    session.session_id = cokacmux::ids::opencode_session_id();
    let user_id = cokacmux::ids::opencode_message_id();
    let assistant_id = cokacmux::ids::opencode_message_id();
    if let Some(user) = session.messages.get_mut(0) {
        user.id = user_id.clone();
        user.parent_id = None;
    }
    if let Some(assistant) = session.messages.get_mut(1) {
        assistant.id = assistant_id;
        assistant.parent_id = Some(user_id);
    }
    let report = opencode::install::install_to_default_db(
        &session,
        &opencode::install::InstallOpts {
            db_path: Some(db.clone()),
            overwrite: false,
        },
    )
    .unwrap();
    assert_eq!(report.db_path, db);
    assert_eq!(report.messages, session.messages.len());

    // Read back.
    let back = opencode::from_db_path(&db, &session.session_id).unwrap();
    assert_eq!(back.cwd, "/tmp/abc");
    let validation = native_validate::validate_clone_artifact(
        Provider::OpenCode,
        &session.session_id,
        &ArtifactPath::OpenCodeDb {
            db_path: db.clone(),
            session_id: session.session_id.clone(),
        },
    )
    .unwrap();
    assert!(validation.ok, "{:?}", validation);

    let err = opencode::install::install_to_default_db(
        &session,
        &opencode::install::InstallOpts {
            db_path: Some(db.clone()),
            overwrite: false,
        },
    )
    .expect_err("re-install without overwrite should fail");
    assert!(err.to_string().contains("already exists"), "{err}");

    opencode::install::install_to_default_db(
        &session,
        &opencode::install::InstallOpts {
            db_path: Some(db.clone()),
            overwrite: true,
        },
    )
    .unwrap();
}

#[cfg(feature = "opencode")]
#[test]
fn context_wrapper_install_into_opencode_tempdir_uses_native_ids_and_validates() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("opencode.db");
    let source = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::OpenCode);

    let report = install_universal_session(
        Provider::OpenCode,
        &converted,
        &InstallSessionOpts {
            opencode_db_path: Some(db.clone()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.provider, Provider::OpenCode);
    assert_eq!(report.session_id, converted.session_id);
    assert!(report.session_id.starts_with("ses_"));
    assert!(report.validation.ok, "{:?}", report.validation);
    assert!(report
        .validation
        .checks
        .iter()
        .any(|check| { check.name == "session_id_shape_native" && check.ok }));
    assert!(report
        .validation
        .checks
        .iter()
        .any(|check| { check.name == "message_id_shape_native" && check.ok }));

    let back = opencode::from_db_path(&db, &report.session_id).unwrap();
    let user_texts = text_messages_for_role(&back, Role::User);
    assert_eq!(user_texts.len(), 1);
    assert!(user_texts[0].ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert!(user_texts[0].contains("installtest-codex"));
    let assistant_texts = text_messages_for_role(&back, Role::Assistant);
    assert_eq!(assistant_texts, vec![CONTEXT_ACK]);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_install_uses_existing_project_for_session_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("opencode.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        opencode::db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, vcs, time_created, time_updated, sandboxes)
             VALUES ('project-current', '/tmp/abc', 'git', 1, 1, '[]')",
            [],
        )
        .unwrap();
    }

    let source = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::OpenCode);
    opencode::install::install_to_default_db(
        &converted,
        &opencode::install::InstallOpts {
            db_path: Some(db.clone()),
            overwrite: false,
        },
    )
    .unwrap();

    let conn = rusqlite::Connection::open(&db).unwrap();
    let project_id: String = conn
        .query_row(
            "SELECT project_id FROM session WHERE id = ?1",
            rusqlite::params![converted.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(project_id, "project-current");
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_install_global_fallback_tracks_session_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("opencode.db");
    let source = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::OpenCode);

    opencode::install::install_to_default_db(
        &converted,
        &opencode::install::InstallOpts {
            db_path: Some(db.clone()),
            overwrite: false,
        },
    )
    .unwrap();

    let conn = rusqlite::Connection::open(&db).unwrap();
    let row: (String, String, String) = conn
        .query_row(
            "SELECT s.project_id, p.worktree, p.sandboxes
             FROM session s
             JOIN project p ON p.id = s.project_id
             WHERE s.id = ?1",
            rusqlite::params![converted.session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row.0, "global");
    assert_eq!(row.1, "/tmp/abc");
    assert_eq!(row.2, "[]");
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_install_does_not_rewrite_existing_global_project_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("opencode.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        opencode::db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 1, 1, '[]')",
            [],
        )
        .unwrap();
    }

    let source = codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let converted = wrap_session_for_context_convert(&source, Provider::OpenCode);

    opencode::install::install_to_default_db(
        &converted,
        &opencode::install::InstallOpts {
            db_path: Some(db.clone()),
            overwrite: false,
        },
    )
    .unwrap();

    let conn = rusqlite::Connection::open(&db).unwrap();
    let row: (String, String) = conn
        .query_row(
            "SELECT s.project_id, p.worktree
             FROM session s
             JOIN project p ON p.id = s.project_id
             WHERE s.id = ?1",
            rusqlite::params![converted.session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(row.0, "global");
    assert_eq!(row.1, "/");
}
