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

use std::path::PathBuf;
use std::process::Command;

use cokacmux::providers::{claude, codex, opencode};
use cokacmux::Provider;

fn home() -> PathBuf {
    dirs::home_dir().expect("home dir")
}

/// Find a source session we can clone from. Prefer the small "Greeting"
/// session in /home/kst/123 because it's tiny.
fn pick_claude_source() -> Option<PathBuf> {
    let p = home()
        .join(".claude")
        .join("projects")
        .join("-home-kst-123");
    if !p.is_dir() {
        return None;
    }
    std::fs::read_dir(&p)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().map(|s| s == "jsonl").unwrap_or(false))
}

// =====================================================================
// Claude
// =====================================================================

#[test]
#[ignore]
fn live_claude_install_and_resume_path() {
    let src = match pick_claude_source() {
        Some(p) => p,
        None => {
            eprintln!("no source claude session — skip");
            return;
        }
    };
    eprintln!("source: {}", src.display());

    // Read source.
    let mut session = claude::from_file(&src, &Default::default()).expect("from_file");

    // Mint a fresh UUID v7 so we don't collide with any existing session.
    let new_uuid = uuid::Uuid::now_v7().to_string();
    let original_uuid = session.session_id.clone();
    session.session_id = new_uuid.clone();
    eprintln!("test uuid: {} (was {})", new_uuid, original_uuid);

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
    assert!(report.jsonl_path.exists());

    // The encoded path must match what `claude --resume <UUID>` would look for.
    let expected_encoding = claude::path::encode_cwd(&session.cwd);
    let expected_path = claude_home
        .join("projects")
        .join(&expected_encoding)
        .join(format!("{}.jsonl", new_uuid));
    assert_eq!(report.jsonl_path, expected_path);

    // Re-parse the installed file and check the session_id and cwd match.
    let reparsed = claude::from_file(&report.jsonl_path, &Default::default()).expect("re-parse");
    assert_eq!(reparsed.session_id, new_uuid);
    assert_eq!(reparsed.cwd, session.cwd);

    // Cleanup. Remove the JSONL — leave the projects/<encoded-cwd>/ alone
    // if it already existed (it almost certainly did).
    std::fs::remove_file(&report.jsonl_path).expect("cleanup");
    eprintln!("cleaned up: {}", report.jsonl_path.display());
}

// =====================================================================
// Codex
// =====================================================================

#[test]
#[ignore]
fn live_codex_install_with_threads_index() {
    let src = match pick_claude_source() {
        Some(p) => p,
        None => {
            eprintln!("no source claude session — skip");
            return;
        }
    };
    let session_claude = claude::from_file(&src, &Default::default()).expect("from_file");

    // Convert to a UniversalSession with a fresh UUID before installing.
    let mut session = session_claude.clone();
    session.origin.provider = Some(Provider::Codex);
    session.session_id = uuid::Uuid::now_v7().to_string();
    let test_id = session.session_id.clone();
    eprintln!("test uuid: {}", test_id);

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
    assert!(report.rollout_path.exists());
    assert!(report.indexed, "threads index update should succeed");

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
    assert_eq!(rid, test_id);
    assert_eq!(rpath, report.rollout_path.display().to_string());
    assert_eq!(source, "cli");
    assert!(!mp.is_empty());
    // sandbox_policy must be valid JSON with a "type" key.
    let p: serde_json::Value = serde_json::from_str(&sandbox).expect("sandbox JSON");
    assert!(p.get("type").is_some());
    assert!(["never", "on-request", "untrusted", "on-failure"].contains(&approval.as_str()));

    eprintln!(
        "threads row: id={}, source={}, sandbox_policy={}",
        rid, source, sandbox
    );

    // Cleanup: drop the row and the rollout file.
    conn.execute(
        "DELETE FROM threads WHERE id = ?1",
        rusqlite::params![test_id],
    )
    .expect("delete row");
    std::fs::remove_file(&report.rollout_path).expect("remove rollout");
    eprintln!("cleaned up.");
}

// =====================================================================
// OpenCode  (read-only verification with `opencode session list`)
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

    // Generate a fresh `ses_` id.
    let rand = uuid::Uuid::now_v7().simple().to_string();
    // OpenCode session ids look like `ses_<hex>`; we use a fresh `ses_test_<hex>`
    // namespace so cleanup can target it precisely.
    let test_sid = format!("ses_test_{}", &rand[..16]);
    eprintln!("test session_id: {}", test_sid);

    // Read a tiny existing session as the source, then rename its id.
    let src_sid = "ses_1bcbac7d3ffeQ6QyJn54Ri3O5E"; // the "Greeting" session
    let mut session = match opencode::from_db_path(&db, src_sid) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not read source session {}: {} — skip", src_sid, e);
            return;
        }
    };
    session.session_id = test_sid.clone();
    // Re-id messages so we don't collide with the original session's primary
    // keys (we cloned from a session that's still in the DB).
    for m in &mut session.messages {
        m.id = format!("{}_m{:04}", test_sid, m.index);
    }
    // Give it a distinctive title so we can spot it.
    session.title = Some("cokacmux-live-test".into());

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

    // Run `opencode session list` and verify our id appears.
    let out = Command::new("opencode")
        .args(["session", "list"])
        .output()
        .expect("opencode session list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let found = stdout.contains(&test_sid);
    eprintln!("opencode session list shows our id: {}", found);

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
        "DELETE FROM session WHERE id = ?1",
        rusqlite::params![test_sid],
    )
    .expect("rm session");
    eprintln!("cleaned up.");

    assert!(
        found,
        "opencode CLI did not show our installed session in `session list`"
    );
}
