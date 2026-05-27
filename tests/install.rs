//! Install helpers: write into each provider's on-disk layout using an
//! isolated temporary directory as the "home". Verifies that round-tripping
//! through an install produces the expected file/db layout.

#![cfg(feature = "discovery")]

#[cfg(feature = "opencode")]
use cokacmux::providers::opencode;
use cokacmux::providers::{claude, codex};
use cokacmux::session::{clone::ArtifactPath, native_validate};
use cokacmux::Provider;

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
            db_path: db,
            session_id: session.session_id.clone(),
        },
    )
    .unwrap();
    assert!(validation.ok, "{:?}", validation);
}
