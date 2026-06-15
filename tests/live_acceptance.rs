//! Live agent-acceptance tests.
//!
//! These tests write to the user's REAL agent storage directories
//! (`~/.claude/projects/`, `~/.codex/sessions/`, `~/.local/share/opencode/`)
//! using freshly-generated session IDs that do not collide with existing
//! sessions, then clean up after themselves.
//!
//! Marked `#[ignore]` so they don't run with `cargo test`. Run explicitly:
//!
//!     cargo test --all-features -- --ignored live
//!
//! Each test refuses to run if the corresponding agent is currently
//! holding the database (lock probe) or if the live home isn't present.

#![cfg(feature = "discovery")]
#![cfg(feature = "opencode")]

use std::path::{Path, PathBuf};
use std::process::Command;

use cokacmux::providers::{claude, codex, opencode};
use cokacmux::session;
use cokacmux::{
    context_user_message_text, wrap_session_for_context_convert, ContentBlock, Provider, Role,
    UniversalSession, CONTEXT_ACK, CONTEXT_CONTINUATION_PROMPT,
};

const MAX_ACCEPTANCE_CONTEXT_BYTES: usize = 128 * 1024;

fn home() -> PathBuf {
    dirs::home_dir().expect("home dir")
}

fn pick_cross_provider_source(target: Provider) -> Option<UniversalSession> {
    pick_cross_provider_source_where(target, |_| true)
}

fn pick_cross_provider_source_where(
    target: Provider,
    mut accept: impl FnMut(&UniversalSession) -> bool,
) -> Option<UniversalSession> {
    let sessions = session::list_all().ok()?;
    for info in sessions {
        if info.provider == target {
            continue;
        }
        match session::load(&info) {
            Ok(session) if is_good_acceptance_source(&session) && accept(&session) => {
                return Some(session);
            }
            _ => continue,
        }
    }
    None
}

fn is_good_acceptance_source(session: &UniversalSession) -> bool {
    if session.messages.is_empty() || is_context_wrapper_session(session) {
        return false;
    }
    context_user_message_text(session).len() <= MAX_ACCEPTANCE_CONTEXT_BYTES
}

fn is_context_wrapper_session(session: &UniversalSession) -> bool {
    if session.extras.contains_key("context_convert") {
        return true;
    }
    let user_texts = visible_texts_for_role(session, Role::User);
    let assistant_texts = visible_texts_for_role(session, Role::Assistant);
    user_texts.len() == 1
        && assistant_texts == vec![CONTEXT_ACK]
        && user_texts[0].ends_with(CONTEXT_CONTINUATION_PROMPT)
}

fn visible_texts_for_role(session: &UniversalSession, role: Role) -> Vec<&str> {
    session
        .messages
        .iter()
        .filter(|message| message.role == role && !message.flags.is_meta)
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn assert_two_message_wrapper(session: &UniversalSession) {
    let user_texts = visible_texts_for_role(session, Role::User);
    let assistant_texts = visible_texts_for_role(session, Role::Assistant);
    assert_eq!(user_texts.len(), 1);
    assert!(user_texts[0].ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert_eq!(assistant_texts, vec![CONTEXT_ACK]);
}

// =====================================================================
// Claude
// =====================================================================

#[test]
#[ignore]
fn live_claude_install_and_resume_path() {
    let source = match pick_cross_provider_source(Provider::Claude) {
        Some(session) => session,
        None => {
            eprintln!("no non-Claude source session — skip");
            return;
        }
    };

    let session = wrap_session_for_context_convert(&source, Provider::Claude);
    eprintln!(
        "source: {:?} {} -> Claude test uuid: {}",
        source.origin.provider, source.session_id, session.session_id
    );

    // Install to live ~/.claude.
    let claude_home = home().join(".claude");
    let report = claude::install::install_to_user_dir(
        &session,
        &claude::install::InstallOpts {
            claude_home: Some(claude_home.clone()),
            overwrite: false,
        },
    )
    .expect("install");

    eprintln!("installed: {}", report.jsonl_path.display());
    let file_exists = report.jsonl_path.exists();

    // The encoded path must match what `claude --resume <UUID>` would look for.
    let expected_encoding = claude::path::encode_cwd(&session.cwd);
    let expected_path = claude_home
        .join("projects")
        .join(&expected_encoding)
        .join(format!("{}.jsonl", session.session_id));
    let path_matches = report.jsonl_path == expected_path;

    // Re-parse the installed file and check the session_id and cwd match.
    let reparsed = claude::from_file(&report.jsonl_path, &Default::default());

    // Cleanup. Remove the JSONL — leave the projects/<encoded-cwd>/ alone
    // if it already existed (it almost certainly did).
    std::fs::remove_file(&report.jsonl_path).expect("cleanup");
    eprintln!("cleaned up: {}", report.jsonl_path.display());

    assert!(file_exists);
    assert!(path_matches);
    let reparsed = reparsed.expect("re-parse");
    assert_eq!(reparsed.session_id, session.session_id);
    assert_eq!(reparsed.cwd, session.cwd);
    assert_two_message_wrapper(&reparsed);
}

// =====================================================================
// Codex
// =====================================================================

#[test]
#[ignore]
fn live_codex_install_with_threads_index() {
    let source = match pick_cross_provider_source(Provider::Codex) {
        Some(session) => session,
        None => {
            eprintln!("no non-Codex source session — skip");
            return;
        }
    };
    let session = wrap_session_for_context_convert(&source, Provider::Codex);
    let test_id = session.session_id.clone();
    eprintln!(
        "source: {:?} {} -> Codex test uuid: {}",
        source.origin.provider, source.session_id, test_id
    );

    let codex_home = home().join(".codex");
    if !codex_home.is_dir() {
        eprintln!("no ~/.codex — skip");
        return;
    }

    // Install — including state_5.sqlite::threads INSERT.
    let report = codex::install::install_to_user_dir(
        &session,
        &codex::install::InstallOpts {
            codex_home: Some(codex_home.clone()),
            overwrite: false,
            update_index: true,
            state_5_path: None, // use the live one
        },
    )
    .expect("install");

    eprintln!("installed rollout: {}", report.rollout_path.display());
    let rollout_exists = report.rollout_path.exists();
    let indexed = report.indexed;
    let reparsed = codex::from_file(&report.rollout_path);

    // Verify the threads row exists with our id and matches expected fields.
    let state_5 = report.index_path.as_ref().unwrap().clone();
    let conn = rusqlite::Connection::open(&state_5).expect("open state_5");
    let (rid, rpath, source, mp, sandbox, approval): (
        String,
        String,
        String,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT id, rollout_path, source, model_provider, sandbox_policy, approval_mode
             FROM threads WHERE id = ?1",
            rusqlite::params![test_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .expect("threads row");
    let rollout_path_matches = rpath == report.rollout_path.display().to_string();
    let sandbox_json = serde_json::from_str::<serde_json::Value>(&sandbox);
    let approval_valid =
        ["never", "on-request", "untrusted", "on-failure"].contains(&approval.as_str());

    // Cleanup: drop the row and the rollout file before asserting parsed shape.
    conn.execute(
        "DELETE FROM threads WHERE id = ?1",
        rusqlite::params![test_id],
    )
    .expect("delete row");
    std::fs::remove_file(&report.rollout_path).expect("remove rollout");
    eprintln!("cleaned up.");

    assert!(rollout_exists);
    assert!(indexed, "threads index update should succeed");
    let reparsed = reparsed.expect("re-parse");
    assert_eq!(reparsed.session_id, test_id);
    assert_eq!(reparsed.cwd, session.cwd);
    assert_two_message_wrapper(&reparsed);
    assert_eq!(rid, test_id);
    assert!(rollout_path_matches);
    assert!(
        ["exec", "cli"].contains(&source.as_str()),
        "unexpected Codex thread source: {source}"
    );
    assert!(!mp.is_empty());
    // sandbox_policy must be valid JSON with a "type" key.
    let p = sandbox_json.expect("sandbox JSON");
    assert!(p.get("type").is_some());
    assert!(approval_valid);

    eprintln!(
        "threads row: id={}, source={}, sandbox_policy={}",
        rid, source, sandbox
    );
}

// =====================================================================
// OpenCode
// =====================================================================

#[test]
#[ignore]
fn live_opencode_install_and_list() {
    let db = home()
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db");
    if !db.is_file() {
        eprintln!("no opencode.db — skip");
        return;
    }

    let source = match pick_cross_provider_source_where(Provider::OpenCode, |session| {
        Path::new(&session.cwd).is_dir()
    }) {
        Some(session) => session,
        None => {
            eprintln!("no non-OpenCode source session with existing cwd — skip");
            return;
        }
    };
    let mut session = wrap_session_for_context_convert(&source, Provider::OpenCode);
    let test_sid = session.session_id.clone();
    session.title = Some("cokacmux-live-test".into());
    eprintln!(
        "source: {:?} {} -> OpenCode test session_id: {}",
        source.origin.provider, source.session_id, test_sid
    );

    // Install — must refuse if opencode is running (lock probe).
    let report = match opencode::install::install_to_default_db(
        &session,
        &opencode::install::InstallOpts {
            db_path: Some(db.clone()),
            overwrite: false,
        },
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("install rejected (opencode running?): {} — skip", e);
            return;
        }
    };
    eprintln!(
        "installed to {} ({} messages)",
        report.db_path.display(),
        report.messages
    );

    // Run `opencode session list` from the installed session cwd. OpenCode
    // chooses the visible project from the current worktree.
    let out = Command::new("opencode")
        .current_dir(&session.cwd)
        .args(["session", "list", "--format", "json"])
        .output();
    let found = out
        .as_ref()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains(&test_sid))
        .unwrap_or(false);
    eprintln!(
        "opencode session list from {} shows our id: {}",
        session.cwd, found
    );
    let reparsed = opencode::from_db_path(&db, &test_sid);
    let project_row = {
        let conn = rusqlite::Connection::open(&db).expect("open opencode.db");
        conn.query_row(
            "SELECT s.project_id, p.worktree
             FROM session s
             JOIN project p ON p.id = s.project_id
             WHERE s.id = ?1",
            rusqlite::params![test_sid],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("project row")
    };

    // Cleanup.
    let conn = rusqlite::Connection::open(&db).expect("open opencode.db");
    conn.execute(
        "DELETE FROM part WHERE session_id = ?1",
        rusqlite::params![test_sid],
    )
    .expect("rm part");
    conn.execute(
        "DELETE FROM message WHERE session_id = ?1",
        rusqlite::params![test_sid],
    )
    .expect("rm message");
    conn.execute(
        "DELETE FROM session_message WHERE session_id = ?1",
        rusqlite::params![test_sid],
    )
    .expect("rm session_message");
    conn.execute(
        "DELETE FROM session WHERE id = ?1",
        rusqlite::params![test_sid],
    )
    .expect("rm session");
    eprintln!("cleaned up.");

    let reparsed = reparsed.expect("re-parse");
    assert_eq!(reparsed.session_id, test_sid);
    assert_two_message_wrapper(&reparsed);
    out.expect("opencode session list");
    if project_row.0 != "global" {
        assert_eq!(project_row.1, session.cwd);
    }
    assert!(
        found,
        "opencode CLI did not show our installed session in `session list`"
    );
}
