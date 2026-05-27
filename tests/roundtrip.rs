//! Roundtrip tests: from_X → to_X should preserve text bodies, and
//! same-provider replay should be bit-identical via provenance.raw.

#[cfg(feature = "opencode")]
use cokacmux::Provider;
use cokacmux::{providers, universal::Role};
use serde_json::Value;

// -------- fixtures --------

fn jsonl_values(jsonl: &str) -> Vec<Value> {
    jsonl
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn claude_fixture() -> &'static str {
    r#"{"type":"permission-mode","permissionMode":"default","sessionId":"sess-claude-1"}
{"type":"user","sessionId":"sess-claude-1","cwd":"/tmp","timestamp":"2026-05-20T01:00:00.000Z","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"hello there"}}
{"type":"assistant","sessionId":"sess-claude-1","cwd":"/tmp","timestamp":"2026-05-20T01:00:01.000Z","uuid":"a1","parentUuid":"u1","message":{"role":"assistant","id":"msg_xxx","model":"claude-opus-4-7","content":[{"type":"text","text":"hi back"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}}
"#
}

fn codex_fixture() -> &'static str {
    r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"sess-codex-1","cwd":"/tmp","cli_version":"0.131.0","git":{"branch":"main","commit_hash":"abc123"}}}
{"timestamp":"2026-05-20T01:00:00.100Z","type":"turn_context","payload":{"model":"gpt-5.5","model_reasoning_effort":"medium"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello there"}],"id":"u1"}}
{"timestamp":"2026-05-20T01:00:01.500Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi back"}],"id":"a1"}}
{"timestamp":"2026-05-20T01:00:02.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}
"#
}

// -------- Claude roundtrip --------

#[test]
fn claude_same_provider_roundtrip_is_bit_identical() {
    let src = claude_fixture();
    let session = providers::claude::from_jsonl_str(src, &Default::default()).unwrap();
    assert!(!session.messages.is_empty(), "should parse messages");
    assert_eq!(session.cwd, "/tmp");

    let out = providers::claude::to_jsonl_string(&session, &Default::default()).unwrap();
    assert_eq!(out, src, "claude roundtrip should be bit-identical");
}

#[test]
fn claude_text_content_extracted() {
    let session = providers::claude::from_jsonl_str(claude_fixture(), &Default::default()).unwrap();
    let texts: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .flat_map(|m| {
            m.content.iter().filter_map(|b| {
                if let cokacmux::ContentBlock::Text { text, .. } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
        })
        .collect();
    assert_eq!(texts, vec!["hello there", "hi back"]);
}

#[test]
fn claude_tool_result_content_array_roundtrips_without_text_only_loss() {
    let src = r#"{"type":"user","sessionId":"claude-tool-array","cwd":"/tmp","timestamp":"2026-05-20T01:00:00.000Z","uuid":"tr1","parentUuid":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":[{"type":"text","text":"see image"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]}]}}
"#;
    let session = providers::claude::from_jsonl_str(src, &Default::default()).unwrap();
    let out = providers::claude::to_jsonl_string(&session, &Default::default()).unwrap();
    let values = jsonl_values(&out);
    let content = values[0]["message"]["content"][0]["content"].clone();

    assert!(content.is_array(), "Claude tool_result array was flattened");
    assert_eq!(content[1]["type"], "image");
}

// -------- Codex roundtrip --------

#[test]
fn codex_same_provider_roundtrip_is_bit_identical() {
    let src = codex_fixture();
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    assert_eq!(session.cwd, "/tmp");
    assert_eq!(session.session_id, "sess-codex-1");
    assert!(session.git.is_some());
    let m = session.model.as_ref().expect("model");
    assert_eq!(m.model_id, "gpt-5.5");

    let out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    assert_eq!(out, src, "codex roundtrip should be bit-identical");
}

#[test]
fn codex_token_count_extracted_to_usage_total() {
    let session = providers::codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let u = session.usage_total.expect("usage_total");
    assert_eq!(u.input_tokens, Some(10));
    assert_eq!(u.output_tokens, Some(5));
    assert_eq!(u.total_tokens, Some(15));
}

// -------- OpenCode roundtrip (via tempfile) --------

/// Regression test for the bug where Claude `attachment` lines lost their
/// type-specific payload (parser was reading non-existent `attachment.name`/
/// `path`/`mime` keys instead of preserving the actual payload).
#[test]
fn claude_attachment_preserves_typed_payload() {
    let src = r#"{"type":"attachment","sessionId":"s","cwd":"/tmp","uuid":"a1","timestamp":"2026-05-20T01:00:00.000Z","attachment":{"type":"deferred_tools_delta","addedNames":["X","Y"],"addedLines":["X","Y"],"removedNames":[]}}
"#;
    let session = providers::claude::from_jsonl_str(src, &Default::default()).unwrap();
    assert_eq!(session.messages.len(), 1);
    let m = &session.messages[0];
    assert!(m
        .provenance
        .source_event_type
        .starts_with("claude:attachment"));
    // The structured content block must carry the typed payload, not nulls.
    match &m.content[0] {
        cokacmux::ContentBlock::Other { type_tag, payload } => {
            assert!(
                type_tag.contains("deferred_tools_delta"),
                "type_tag should reflect the attachment subtype, got {:?}",
                type_tag
            );
            assert_eq!(
                payload.get("type").and_then(|v| v.as_str()),
                Some("deferred_tools_delta")
            );
            assert!(
                payload.get("addedNames").is_some(),
                "addedNames must survive"
            );
        }
        other => panic!("expected Other content block, got {:?}", other),
    }
}

/// Regression test for the bug where OpenCode session.agent ("build", "plan")
/// got overwritten by ModelInfo.variant ("medium") during round-trip.
#[cfg(feature = "opencode")]
#[test]
fn opencode_agent_column_survives_roundtrip() {
    use cokacmux::providers::opencode::db;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s1', 'global', '/tmp', 'X', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"medium\"}',
                     1000, 2000)",
            [],
        )
        .unwrap();
    }
    let session = providers::opencode::from_db_path(&src, "s1").unwrap();
    // Now write to a fresh DB and read back.
    let dst = dir.path().join("dst.db");
    providers::opencode::to_db_path(&session, &dst).unwrap();

    let conn = rusqlite::Connection::open(&dst).unwrap();
    let (agent, model): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT agent, model FROM session WHERE id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        agent.as_deref(),
        Some("build"),
        "agent column must round-trip unchanged"
    );
    let m: serde_json::Value = serde_json::from_str(&model.unwrap()).unwrap();
    assert_eq!(m.get("id").and_then(|v| v.as_str()), Some("gpt-5.5"));
    assert_eq!(
        m.get("variant").and_then(|v| v.as_str()),
        Some("medium"),
        "model.variant must round-trip unchanged (must not collide with agent)"
    );
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_writer_defaults_cli_required_session_fields() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dst.db");
    let mut session =
        cokacmux::UniversalSession::new("ses_default_fields", Provider::OpenCode, "/tmp");
    session.title = Some("Default Fields".into());

    providers::opencode::to_db_path(&session, &db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let (agent, model): (String, String) = conn
        .query_row(
            "SELECT agent, model FROM session WHERE id = 'ses_default_fields'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(agent, "build");

    let model: serde_json::Value = serde_json::from_str(&model).unwrap();
    assert_eq!(model.get("id").and_then(|v| v.as_str()), Some("gpt-5.5"));
    assert_eq!(
        model.get("providerID").and_then(|v| v.as_str()),
        Some("openai")
    );
    assert_eq!(
        model.get("variant").and_then(|v| v.as_str()),
        Some("default")
    );
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_writer_emits_cli_compatible_message_data() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dst.db");
    let mut session =
        cokacmux::UniversalSession::new("ses_cli_shape", Provider::OpenCode, "/home/kst/123");
    session.title = Some("CLI Shape".into());
    session.messages.push(cokacmux::UMessage {
        id: "msg_user".into(),
        parent_id: None,
        index: 0,
        timestamp: None,
        role: Role::User,
        model: None,
        usage: None,
        stop_reason: None,
        content: vec![cokacmux::ContentBlock::text("hi")],
        flags: Default::default(),
        provenance: cokacmux::Provenance {
            source_event_type: "test:user".into(),
            raw: serde_json::json!({}),
        },
        extras: Default::default(),
    });
    session.messages.push(cokacmux::UMessage {
        id: "msg_assistant".into(),
        parent_id: Some("msg_user".into()),
        index: 1,
        timestamp: None,
        role: Role::Assistant,
        model: Some(cokacmux::ModelInfo {
            provider_id: Some("openai".into()),
            model_id: "gpt-5.5".into(),
            variant: Some("medium".into()),
        }),
        usage: Some(cokacmux::Usage {
            input_tokens: Some(3),
            output_tokens: Some(2),
            total_tokens: Some(5),
            cost_usd: Some(0.001),
            ..Default::default()
        }),
        stop_reason: Some("stop".into()),
        content: vec![cokacmux::ContentBlock::text("hello")],
        flags: Default::default(),
        provenance: cokacmux::Provenance {
            source_event_type: "test:assistant".into(),
            raw: serde_json::json!({}),
        },
        extras: Default::default(),
    });

    providers::opencode::to_db_path(&session, &db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let path: String = conn
        .query_row(
            "SELECT path FROM session WHERE id = 'ses_cli_shape'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(path, "home/kst/123");

    let user: String = conn
        .query_row("SELECT data FROM message WHERE id = 'msg_user'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let user: serde_json::Value = serde_json::from_str(&user).unwrap();
    assert_eq!(user["agent"], "build");
    assert_eq!(user["model"]["providerID"], "openai");
    assert_eq!(user["model"]["modelID"], "gpt-5.5");
    assert!(user["summary"]["diffs"].as_array().unwrap().is_empty());

    let assistant: String = conn
        .query_row(
            "SELECT data FROM message WHERE id = 'msg_assistant'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let assistant: serde_json::Value = serde_json::from_str(&assistant).unwrap();
    assert_eq!(assistant["parentID"], "msg_user");
    assert_eq!(assistant["path"]["cwd"], "/home/kst/123");
    assert_eq!(assistant["path"]["root"], "/");
    assert_eq!(assistant["modelID"], "gpt-5.5");
    assert_eq!(assistant["providerID"], "openai");
    assert_eq!(assistant["variant"], "medium");
    assert_eq!(assistant["tokens"]["input"], 3);
    assert_eq!(assistant["tokens"]["output"], 2);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_writer_fuses_tool_results_into_replayable_tool_part() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dst.db");
    let mut session =
        cokacmux::UniversalSession::new("ses_tool_fusion", Provider::OpenCode, "/tmp");
    session.messages.push(cokacmux::UMessage {
        id: "msg_assistant".into(),
        parent_id: None,
        index: 0,
        timestamp: None,
        role: Role::Assistant,
        model: None,
        usage: None,
        stop_reason: Some("tool-calls".into()),
        content: vec![cokacmux::ContentBlock::tool_use(
            "call_shell",
            "shell",
            serde_json::json!({"command": "ls"}),
        )],
        flags: Default::default(),
        provenance: cokacmux::Provenance {
            source_event_type: "test:assistant".into(),
            raw: serde_json::json!({}),
        },
        extras: Default::default(),
    });
    session.messages.push(cokacmux::UMessage {
        id: "msg_tool".into(),
        parent_id: Some("msg_assistant".into()),
        index: 1,
        timestamp: None,
        role: Role::Tool,
        model: None,
        usage: None,
        stop_reason: None,
        content: vec![cokacmux::ContentBlock::tool_result(
            "call_shell",
            serde_json::json!("permission denied"),
            true,
        )],
        flags: Default::default(),
        provenance: cokacmux::Provenance {
            source_event_type: "test:tool".into(),
            raw: serde_json::json!({}),
        },
        extras: Default::default(),
    });

    providers::opencode::to_db_path(&session, &db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let tool_parts: Vec<serde_json::Value> = conn
        .prepare("SELECT data FROM part WHERE session_id = 'ses_tool_fusion' ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            let data: String = row.get(0)?;
            Ok(serde_json::from_str::<serde_json::Value>(&data).unwrap())
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    let tool_part = tool_parts
        .iter()
        .find(|part| part.get("type").and_then(|v| v.as_str()) == Some("tool"))
        .expect("assistant parts should include a replayable tool part");
    assert_eq!(tool_part["tool"], "shell");
    assert_eq!(tool_part["state"]["status"], "error");
    assert_eq!(tool_part["state"]["input"]["command"], "ls");
    assert_eq!(tool_part["state"]["error"], "permission denied");

    let back = providers::opencode::from_db_path(&db_path, "ses_tool_fusion").unwrap();
    let mut saw_use = false;
    let mut saw_error_result = false;
    for block in back.messages.iter().flat_map(|m| m.content.iter()) {
        match block {
            cokacmux::ContentBlock::ToolUse { call_id, name, .. } => {
                saw_use |= call_id == "call_shell" && name == "shell";
            }
            cokacmux::ContentBlock::ToolResult {
                call_id, is_error, ..
            } => {
                saw_error_result |= call_id == "call_shell" && *is_error;
            }
            _ => {}
        }
    }
    assert!(saw_use);
    assert!(saw_error_result);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_session_message_events_survive_roundtrip() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s1', 'global', '/tmp', 'Session Messages', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     1000, 2000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, time_created, time_updated, data)
             VALUES (?1, 's1', 'agent-switched', 1100, 1100, ?2)",
            rusqlite::params![
                "evt_agent",
                serde_json::json!({
                    "agent": "plan",
                    "time": {"created": 1100}
                })
                .to_string()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, time_created, time_updated, data)
             VALUES (?1, 's1', 'model-switched', 1200, 1200, ?2)",
            rusqlite::params![
                "evt_model",
                serde_json::json!({
                    "model": {
                        "id": "gpt-5.4-mini",
                        "providerID": "openai",
                        "variant": "default"
                    },
                    "time": {"created": 1200}
                })
                .to_string()
            ],
        )
        .unwrap();
    }

    let session = providers::opencode::from_db_path(&src, "s1").unwrap();
    assert_eq!(
        session
            .extras
            .get("opencode_agent")
            .and_then(|v| v.as_str()),
        Some("plan")
    );
    assert_eq!(
        session.model.as_ref().map(|m| m.model_id.as_str()),
        Some("gpt-5.4-mini")
    );
    assert!(session.messages.iter().any(|m| {
        m.flags.is_meta
            && m.provenance.source_event_type == "opencode:session_message.agent-switched"
    }));
    assert!(session.messages.iter().any(|m| {
        m.flags.is_meta
            && m.provenance.source_event_type == "opencode:session_message.model-switched"
    }));

    let dst = dir.path().join("dst.db");
    providers::opencode::to_db_path(&session, &dst).unwrap();
    let conn = rusqlite::Connection::open(&dst).unwrap();
    let rows: Vec<(String, String)> = conn
        .prepare(
            "SELECT type, data FROM session_message
             WHERE session_id = 's1' ORDER BY type",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|(kind, data)| { kind == "agent-switched" && data.contains(r#""agent":"plan""#) }));
    assert!(rows
        .iter()
        .any(|(kind, data)| { kind == "model-switched" && data.contains("gpt-5.4-mini") }));
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_session_row_extended_fields_survive_roundtrip() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src_path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session
                (id, project_id, parent_id, directory, title, agent, model,
                 time_created, time_updated, slug, version, path, share_url,
                 summary_additions, summary_deletions, summary_files, summary_diffs,
                 revert, permission, time_compacting, time_archived, workspace_id)
             VALUES
                ('s_extended', 'global', 'ses_parent', '/tmp', 'Extended', 'build',
                 '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                 1000, 2000, 'native-slug', '1.15.7', '/tmp/.opencode/session',
                 'https://share.example/s_extended', 1, 2, 3, '[{\"path\":\"a.txt\"}]',
                 '{\"messageID\":\"msg_1\",\"diff\":\"diff\"}',
                 '[{\"permission\":\"bash\",\"pattern\":\"*\",\"action\":\"ask\"}]',
                 1500, 1800, 'workspace_1')",
            [],
        )
        .unwrap();
    }

    let session = providers::opencode::from_db_path(&src_path, "s_extended").unwrap();
    let dst_path = dir.path().join("dst.db");
    providers::opencode::to_db_path(&session, &dst_path).unwrap();

    let conn = rusqlite::Connection::open(&dst_path).unwrap();
    let row: (
        Option<String>,
        String,
        String,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT parent_id, slug, version, share_url,
                    summary_additions, summary_deletions, summary_files, summary_diffs,
                    revert, permission, time_compacting, time_archived, workspace_id, path
             FROM session WHERE id = 's_extended'",
            [],
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
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(row.0.as_deref(), Some("ses_parent"));
    assert_eq!(row.1, "native-slug");
    assert_eq!(row.2, "1.15.7");
    assert_eq!(row.3.as_deref(), Some("https://share.example/s_extended"));
    assert_eq!((row.4, row.5, row.6), (Some(1), Some(2), Some(3)));
    assert_eq!(row.7.as_deref(), Some(r#"[{"path":"a.txt"}]"#));
    assert!(row.8.as_deref().unwrap().contains("msg_1"));
    assert!(row.9.as_deref().unwrap().contains("bash"));
    assert_eq!((row.10, row.11), (Some(1500), Some(1800)));
    assert_eq!(row.12.as_deref(), Some("workspace_1"));
    assert_eq!(row.13.as_deref(), Some("/tmp/.opencode/session"));
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_v2_session_messages_parse_without_legacy_rows() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s_v2', 'global', '/tmp', 'V2 Only', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     1000, 3000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, time_created, time_updated, data)
             VALUES (?1, 's_v2', 'user', 1100, 1100, ?2)",
            rusqlite::params![
                "evt_user",
                serde_json::json!({
                    "text": "hi from v2",
                    "files": [],
                    "agents": {},
                    "references": [],
                    "time": {"created": 1100}
                })
                .to_string()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, time_created, time_updated, data)
             VALUES (?1, 's_v2', 'assistant', 1200, 1300, ?2)",
            rusqlite::params![
                "evt_assistant",
                serde_json::json!({
                    "agent": "build",
                    "model": {
                        "id": "gpt-5.5",
                        "providerID": "openai",
                        "variant": "default"
                    },
                    "content": [
                        {"type": "reasoning", "id": "r1", "text": "thinking v2"},
                        {"type": "text", "text": "hello v2"},
                        {
                            "type": "tool",
                            "id": "call_v2",
                            "name": "bash",
                            "state": {
                                "status": "completed",
                                "input": {"command": "pwd"},
                                "structured": {"output": "/tmp"}
                            }
                        }
                    ],
                    "finish": "stop",
                    "tokens": {
                        "input": 1,
                        "output": 2,
                        "reasoning": 3,
                        "cache": {"read": 4, "write": 5}
                    },
                    "time": {"created": 1200, "completed": 1300}
                })
                .to_string()
            ],
        )
        .unwrap();
    }

    let session = providers::opencode::from_db_path(&src, "s_v2").unwrap();
    let fragments: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .flat_map(|m| {
            m.content.iter().filter_map(|b| match b {
                cokacmux::ContentBlock::Text { text, .. } => Some(text.as_str()),
                cokacmux::ContentBlock::Thinking { text, .. } => Some(text.as_str()),
                _ => None,
            })
        })
        .collect();
    assert_eq!(fragments, vec!["hi from v2", "thinking v2", "hello v2"]);

    let tool_results: Vec<(&str, bool)> = session
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            cokacmux::ContentBlock::ToolResult {
                call_id, is_error, ..
            } => Some((call_id.as_str(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(tool_results, vec![("call_v2", false)]);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_legacy_control_parts_keep_native_shape_on_write() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src_path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s_parts', 'global', '/tmp', 'Control Parts', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     1000, 2000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('m1', 's_parts', 1000, 1000,
                     '{\"role\":\"assistant\",\"time\":{\"created\":1000}}')",
            [],
        )
        .unwrap();
        for (id, data) in [
            (
                "p_patch",
                serde_json::json!({
                    "type": "patch",
                    "hash": "abc123",
                    "files": ["/tmp/changed.txt"]
                }),
            ),
            (
                "p_snapshot",
                serde_json::json!({
                    "type": "snapshot",
                    "snapshot": "snap-1"
                }),
            ),
            (
                "p_agent",
                serde_json::json!({
                    "type": "agent",
                    "name": "build",
                    "source": {"value": "@build", "start": 0, "end": 6}
                }),
            ),
            (
                "p_subtask",
                serde_json::json!({
                    "type": "subtask",
                    "prompt": "child work",
                    "description": "desc",
                    "agent": "build"
                }),
            ),
            (
                "p_retry",
                serde_json::json!({
                    "type": "retry",
                    "attempt": 2,
                    "error": {"name": "APIError", "message": "retry me"},
                    "time": {"created": 1001}
                }),
            ),
            (
                "p_compaction",
                serde_json::json!({
                    "type": "compaction",
                    "auto": true,
                    "overflow": false,
                    "tail_start_id": "m0"
                }),
            ),
        ] {
            conn.execute(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                 VALUES (?1, 'm1', 's_parts', 1000, 1000, ?2)",
                rusqlite::params![id, data.to_string()],
            )
            .unwrap();
        }
    }

    let session = providers::opencode::from_db_path(&src_path, "s_parts").unwrap();
    let dst_path = dir.path().join("dst.db");
    providers::opencode::to_db_path(&session, &dst_path).unwrap();

    let conn = rusqlite::Connection::open(&dst_path).unwrap();
    let rows: Vec<serde_json::Value> = conn
        .prepare(
            "SELECT data FROM part
             WHERE session_id = 's_parts'
             ORDER BY id",
        )
        .unwrap()
        .query_map([], |row| {
            let data: String = row.get(0)?;
            Ok(serde_json::from_str::<serde_json::Value>(&data).unwrap())
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();

    assert_eq!(rows.len(), 9);
    let mut original_control_parts = 0usize;
    for row in rows {
        assert!(
            row.get("payload").is_none(),
            "native OpenCode control part must not be wrapped: {row}"
        );
        match row.get("type").and_then(|v| v.as_str()) {
            Some("agent" | "compaction" | "patch" | "retry" | "snapshot" | "subtask") => {
                original_control_parts += 1;
            }
            Some("step-start" | "reasoning" | "step-finish") => {}
            other => panic!("unexpected OpenCode part type: {other:?}"),
        }
    }
    assert_eq!(original_control_parts, 6);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_native_step_parts_are_not_duplicated_on_write() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src_path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s_native_steps', 'global', '/tmp', 'Native Steps', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     1000, 2000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_user', 's_native_steps', 1000, 1000,
                     '{\"role\":\"user\",\"time\":{\"created\":1000},\"agent\":\"build\",\"model\":{\"providerID\":\"openai\",\"modelID\":\"gpt-5.5\"}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('prt_user', 'msg_user', 's_native_steps', 1000, 1000,
                     '{\"type\":\"text\",\"text\":\"hi\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_assist', 's_native_steps', 1001, 1001,
                     '{\"parentID\":\"msg_user\",\"role\":\"assistant\",\"time\":{\"created\":1001,\"completed\":1002},\"mode\":\"build\",\"agent\":\"build\",\"path\":{\"cwd\":\"/tmp\",\"root\":\"/\"},\"cost\":0,\"tokens\":{\"input\":1,\"output\":1,\"reasoning\":1,\"cache\":{\"read\":0,\"write\":0}},\"modelID\":\"gpt-5.5\",\"providerID\":\"openai\",\"finish\":\"stop\"}')",
            [],
        )
        .unwrap();
        for (idx, (id, data)) in [
            ("prt_start", serde_json::json!({"type": "step-start"})),
            (
                "prt_reason",
                serde_json::json!({
                    "type": "reasoning",
                    "text": "thought",
                    "time": {"start": 1001, "end": 1001},
                    "metadata": {
                        "openai": {
                            "itemId": "rs_1",
                            "reasoningEncryptedContent": "encrypted"
                        }
                    }
                }),
            ),
            (
                "prt_text",
                serde_json::json!({
                    "type": "text",
                    "text": "ok",
                    "time": {"start": 1001, "end": 1001},
                    "metadata": {
                        "openai": {
                            "itemId": "msg_1",
                            "phase": "final_answer"
                        }
                    }
                }),
            ),
            (
                "prt_finish",
                serde_json::json!({
                    "type": "step-finish",
                    "reason": "stop",
                    "tokens": {"input": 1, "output": 1, "reasoning": 1, "cache": {"read": 0, "write": 0}},
                    "cost": 0
                }),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            conn.execute(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                 VALUES (?1, 'msg_assist', 's_native_steps', ?2, ?2, ?3)",
                rusqlite::params![id, 1001 + idx as i64, data.to_string()],
            )
            .unwrap();
        }
    }

    let session = providers::opencode::from_db_path(&src_path, "s_native_steps").unwrap();
    let dst_path = dir.path().join("dst.db");
    providers::opencode::to_db_path(&session, &dst_path).unwrap();

    let conn = rusqlite::Connection::open(&dst_path).unwrap();
    let part_types: Vec<String> = conn
        .prepare(
            "SELECT data FROM part
             WHERE session_id = 's_native_steps'
             ORDER BY time_created, id",
        )
        .unwrap()
        .query_map([], |row| {
            let data: String = row.get(0)?;
            let value: serde_json::Value = serde_json::from_str(&data).unwrap();
            Ok(value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string())
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();

    assert_eq!(
        part_types,
        vec!["text", "step-start", "reasoning", "text", "step-finish"]
    );
    let assistant_parts: Vec<serde_json::Value> = conn
        .prepare(
            "SELECT data FROM part
             WHERE session_id = 's_native_steps'
               AND json_extract(data, '$.type') IN ('reasoning', 'text')
               AND json_extract(data, '$.metadata.openai.itemId') IS NOT NULL
             ORDER BY time_created, id",
        )
        .unwrap()
        .query_map([], |row| {
            let data: String = row.get(0)?;
            Ok(serde_json::from_str::<serde_json::Value>(&data).unwrap())
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(assistant_parts.len(), 2);
    assert_eq!(
        assistant_parts[0]["metadata"]["openai"]["reasoningEncryptedContent"],
        "encrypted"
    );
    assert_eq!(
        assistant_parts[1]["metadata"]["openai"]["phase"],
        "final_answer"
    );

    let rows: Vec<serde_json::Value> = conn
        .prepare(
            "SELECT data FROM message
             WHERE session_id = 's_native_steps'
             ORDER BY time_created, id",
        )
        .unwrap()
        .query_map([], |row| {
            let data: String = row.get(0)?;
            Ok(serde_json::from_str::<serde_json::Value>(&data).unwrap())
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(rows[0]["model"].get("variant").is_none());
    assert!(rows[1].get("variant").is_none());
    assert_eq!(rows[1]["cost"].as_i64(), Some(0));

    let step_finish: serde_json::Value = conn
        .query_row(
            "SELECT data FROM part
             WHERE session_id = 's_native_steps'
               AND json_extract(data, '$.type') = 'step-finish'",
            [],
            |row| {
                let data: String = row.get(0)?;
                Ok(serde_json::from_str(&data).unwrap())
            },
        )
        .unwrap();
    assert_eq!(step_finish["cost"].as_i64(), Some(0));
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_writer_preserves_encrypted_reasoning_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dst.db");
    let mut session =
        cokacmux::UniversalSession::new("ses_encrypted_reasoning", Provider::OpenCode, "/tmp");
    session.messages.push(cokacmux::UMessage {
        id: "msg_assistant".into(),
        parent_id: None,
        index: 0,
        timestamp: None,
        role: Role::Assistant,
        model: None,
        usage: None,
        stop_reason: Some("stop".into()),
        content: vec![cokacmux::ContentBlock::Thinking {
            text: "hidden thought summary".into(),
            encrypted: Some("encrypted-payload".into()),
            extras: Default::default(),
        }],
        flags: Default::default(),
        provenance: cokacmux::Provenance {
            source_event_type: "codex:response_item.reasoning".into(),
            raw: serde_json::json!({}),
        },
        extras: Default::default(),
    });

    providers::opencode::to_db_path(&session, &db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let reasoning: serde_json::Value = conn
        .query_row(
            "SELECT data FROM part
             WHERE session_id = 'ses_encrypted_reasoning'
               AND json_extract(data, '$.type') = 'reasoning'",
            [],
            |row| {
                let data: String = row.get(0)?;
                Ok(serde_json::from_str(&data).unwrap())
            },
        )
        .unwrap();
    assert_eq!(
        reasoning["metadata"]["openai"]["reasoningEncryptedContent"],
        "encrypted-payload"
    );
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_writer_omits_empty_assistant_text_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dst.db");
    let mut session =
        cokacmux::UniversalSession::new("ses_no_empty_metadata", Provider::OpenCode, "/tmp");
    session.messages.push(cokacmux::UMessage {
        id: "msg_assistant".into(),
        parent_id: None,
        index: 0,
        timestamp: None,
        role: Role::Assistant,
        model: None,
        usage: None,
        stop_reason: Some("stop".into()),
        content: vec![
            cokacmux::ContentBlock::Thinking {
                text: "visible reasoning".into(),
                encrypted: None,
                extras: Default::default(),
            },
            cokacmux::ContentBlock::Text {
                text: "answer".into(),
                extras: Default::default(),
            },
        ],
        flags: Default::default(),
        provenance: cokacmux::Provenance {
            source_event_type: "test:assistant".into(),
            raw: serde_json::json!({}),
        },
        extras: Default::default(),
    });

    providers::opencode::to_db_path(&session, &db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let rows: Vec<serde_json::Value> = conn
        .prepare(
            "SELECT data FROM part
             WHERE session_id = 'ses_no_empty_metadata'
               AND json_extract(data, '$.type') IN ('reasoning', 'text')
             ORDER BY time_created, id",
        )
        .unwrap()
        .query_map([], |row| {
            let data: String = row.get(0)?;
            Ok(serde_json::from_str::<serde_json::Value>(&data).unwrap())
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();

    assert_eq!(rows.len(), 2);
    for row in rows {
        assert!(
            row.get("time").is_some(),
            "assistant text-like part keeps native time: {row}"
        );
        assert!(
            row.get("metadata").is_none(),
            "writer must not synthesize empty metadata: {row}"
        );
    }
}

#[cfg(feature = "opencode")]
#[test]
fn codex_to_opencode_preserves_source_text_phase_metadata() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex_phase_src","cwd":"/tmp","cli_version":"0.131.0"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}
{"timestamp":"2026-05-20T01:00:01.500Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}],"phase":"final_answer"}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dst.db");

    providers::opencode::to_db_path(&session, &db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let text_part: serde_json::Value = conn
        .query_row(
            "SELECT data FROM part
             WHERE session_id = 'codex_phase_src'
               AND json_extract(data, '$.type') = 'text'
               AND json_extract(data, '$.text') = 'hi'",
            [],
            |row| {
                let data: String = row.get(0)?;
                Ok(serde_json::from_str(&data).unwrap())
            },
        )
        .unwrap();

    assert_eq!(text_part["metadata"]["openai"]["phase"], "final_answer");
    assert!(text_part["metadata"]["openai"].get("itemId").is_none());
}

#[cfg(feature = "opencode")]
#[test]
fn codex_to_opencode_preserves_native_shaped_openai_item_ids() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex_item_ids_src","cwd":"/tmp","cli_version":"0.131.0"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"reasoning","id":"rs_test_item","summary":[],"content":null,"encrypted_content":"encrypted"}}
{"timestamp":"2026-05-20T01:00:01.500Z","type":"response_item","payload":{"type":"message","role":"assistant","id":"msg_test_item","content":[{"type":"output_text","text":"hi"}],"phase":"final_answer"}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dst.db");

    providers::opencode::to_db_path(&session, &db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let reasoning: serde_json::Value = conn
        .query_row(
            "SELECT data FROM part
             WHERE session_id = 'codex_item_ids_src'
               AND json_extract(data, '$.type') = 'reasoning'
               AND json_extract(data, '$.metadata.openai.itemId') = 'rs_test_item'",
            [],
            |row| {
                let data: String = row.get(0)?;
                Ok(serde_json::from_str(&data).unwrap())
            },
        )
        .unwrap();
    assert_eq!(
        reasoning["metadata"]["openai"]["reasoningEncryptedContent"],
        "encrypted"
    );

    let text_part: serde_json::Value = conn
        .query_row(
            "SELECT data FROM part
             WHERE session_id = 'codex_item_ids_src'
               AND json_extract(data, '$.type') = 'text'
               AND json_extract(data, '$.text') = 'hi'",
            [],
            |row| {
                let data: String = row.get(0)?;
                Ok(serde_json::from_str(&data).unwrap())
            },
        )
        .unwrap();
    assert_eq!(text_part["metadata"]["openai"]["itemId"], "msg_test_item");
    assert_eq!(text_part["metadata"]["openai"]["phase"], "final_answer");
}

#[test]
fn codex_structured_tool_output_roundtrips_without_stringifying() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex_structured_output","cwd":"/tmp","cli_version":"0.131.0"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"function_call","name":"view_image","call_id":"call_img","arguments":"{\"path\":\"/tmp/a.png\"}"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_img","output":[{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo="},{"type":"input_text","text":"ok"}]}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    let values = jsonl_values(&out);
    let output = values
        .iter()
        .find_map(|value| {
            let payload = value.get("payload")?;
            if payload.get("type").and_then(|value| value.as_str()) == Some("function_call_output")
            {
                payload.get("output")
            } else {
                None
            }
        })
        .expect("missing function_call_output");

    assert!(
        output.is_array(),
        "structured output was stringified: {output}"
    );
    assert_eq!(output[0]["type"], "input_image");
}

#[test]
fn codex_custom_tool_call_shape_roundtrips_when_source_derived() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex_custom_tool","cwd":"/tmp","cli_version":"0.131.0"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"custom_tool_call","call_id":"call_custom","name":"freeform","input":"raw prompt","status":"completed"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_custom","name":"freeform","output":"raw answer"}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    let payload_types = jsonl_values(&out)
        .iter()
        .filter_map(|value| value.get("payload"))
        .filter_map(|payload| payload.get("type").and_then(|value| value.as_str()))
        .map(str::to_string)
        .collect::<Vec<_>>();

    assert!(payload_types.contains(&"custom_tool_call".to_string()));
    assert!(payload_types.contains(&"custom_tool_call_output".to_string()));
}

#[test]
fn codex_native_tool_search_web_search_and_local_shell_roundtrip_shapes() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex_native_tools","cwd":"/tmp","cli_version":"0.131.0"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"local_shell_call","call_id":"shell_1","status":"completed","action":{"type":"exec","command":["pwd"],"timeout_ms":1000,"working_directory":"/tmp","env":null,"user":null}}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"tool_search_call","call_id":"search_1","status":"completed","execution":"list","arguments":{"query":"fmt"}}}
{"timestamp":"2026-05-20T01:00:01.500Z","type":"response_item","payload":{"type":"tool_search_output","call_id":"search_1","status":"completed","execution":"list","tools":[{"name":"fmt","description":"format"}]}}
{"timestamp":"2026-05-20T01:00:02.000Z","type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{"type":"search","query":"rust serde"}}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    let values = jsonl_values(&out);
    let payloads = values
        .iter()
        .filter_map(|value| value.get("payload"))
        .collect::<Vec<_>>();

    let local_shell = payloads
        .iter()
        .find(|payload| {
            payload.get("type").and_then(|value| value.as_str()) == Some("local_shell_call")
        })
        .expect("missing local_shell_call");
    assert_eq!(local_shell["action"]["command"][0], "pwd");

    let tool_search_output = payloads
        .iter()
        .find(|payload| {
            payload.get("type").and_then(|value| value.as_str()) == Some("tool_search_output")
        })
        .expect("missing tool_search_output");
    assert_eq!(tool_search_output["tools"][0]["name"], "fmt");

    let web_search = payloads
        .iter()
        .find(|payload| {
            payload.get("type").and_then(|value| value.as_str()) == Some("web_search_call")
        })
        .expect("missing web_search_call");
    assert_eq!(web_search["action"]["query"], "rust serde");
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_file_parts_use_native_url_shape_for_images() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dst.db");
    let mut session =
        cokacmux::UniversalSession::new("ses_image_url_shape", Provider::OpenCode, "/tmp");
    session.messages.push(cokacmux::UMessage {
        id: "msg_user".into(),
        parent_id: None,
        index: 0,
        timestamp: None,
        role: Role::User,
        model: None,
        usage: None,
        stop_reason: None,
        content: vec![cokacmux::ContentBlock::Image {
            mime: "image/png".into(),
            source: cokacmux::ImageSource::Base64 {
                data: "iVBORw0KGgo=".into(),
            },
            extras: Default::default(),
        }],
        flags: Default::default(),
        provenance: cokacmux::Provenance {
            source_event_type: "test:user".into(),
            raw: serde_json::json!({}),
        },
        extras: Default::default(),
    });

    providers::opencode::to_db_path(&session, &db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let file_part: serde_json::Value = conn
        .query_row(
            "SELECT data FROM part
             WHERE session_id = 'ses_image_url_shape'
               AND json_extract(data, '$.type') = 'file'",
            [],
            |row| {
                let data: String = row.get(0)?;
                Ok(serde_json::from_str(&data).unwrap())
            },
        )
        .unwrap();
    assert_eq!(file_part["url"], "data:image/png;base64,iVBORw0KGgo=");
    assert!(file_part.get("source").is_none());

    let src_path = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src_path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s_native_file', 'global', '/tmp', 'Native File', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     1000, 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_native_file', 's_native_file', 1000, 1000,
                     '{\"role\":\"user\",\"time\":{\"created\":1000},\"agent\":\"build\",\"model\":{\"providerID\":\"openai\",\"modelID\":\"gpt-5.5\"}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('prt_native_file', 'msg_native_file', 's_native_file', 1000, 1000,
                     '{\"type\":\"file\",\"mime\":\"image/png\",\"url\":\"data:image/png;base64,iVBORw0KGgo=\"}')",
            [],
        )
        .unwrap();
    }

    let parsed = providers::opencode::from_db_path(&src_path, "s_native_file").unwrap();
    let images = parsed
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter(|block| matches!(block, cokacmux::ContentBlock::Image { .. }))
        .count();
    assert_eq!(images, 1);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_v2_user_files_are_semantic_file_blocks() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src_path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s_v2_user_files', 'global', '/tmp', 'V2 User Files', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     1000, 2000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, time_created, time_updated, data)
             VALUES ('evt_user_files', 's_v2_user_files', 'user', 1000, 1000, ?1)",
            [serde_json::json!({
                "text": "look",
                "files": [
                    {"uri": "data:image/png;base64,iVBORw0KGgo=", "mime": "image/png", "name": "shot.png"},
                    {"uri": "file:///tmp/readme.txt", "mime": "text/plain", "name": "readme.txt"}
                ],
                "agents": {},
                "references": [],
                "time": {"created": 1000}
            })
            .to_string()],
        )
        .unwrap();
    }

    let session = providers::opencode::from_db_path(&src_path, "s_v2_user_files").unwrap();
    let mut images = 0usize;
    let mut attachments = 0usize;
    for block in session
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
    {
        match block {
            cokacmux::ContentBlock::Image { mime, .. } => {
                images += 1;
                assert_eq!(mime, "image/png");
            }
            cokacmux::ContentBlock::Attachment {
                name, path, mime, ..
            } => {
                attachments += 1;
                assert_eq!(name.as_deref(), Some("readme.txt"));
                assert_eq!(path.as_deref(), Some("file:///tmp/readme.txt"));
                assert_eq!(mime.as_deref(), Some("text/plain"));
            }
            _ => {}
        }
    }
    assert_eq!(images, 1);
    assert_eq!(attachments, 1);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_all_v2_session_message_types_are_represented() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src_path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s_v2_all', 'global', '/tmp', 'V2 All', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     1000, 3000)",
            [],
        )
        .unwrap();
        for (idx, (id, kind, data)) in [
            (
                "evt_agent",
                "agent-switched",
                serde_json::json!({"agent": "plan", "time": {"created": 1000}}),
            ),
            (
                "evt_model",
                "model-switched",
                serde_json::json!({
                    "model": {"id": "gpt-5.4-mini", "providerID": "openai", "variant": "default"},
                    "time": {"created": 1001}
                }),
            ),
            (
                "evt_user",
                "user",
                serde_json::json!({
                    "text": "hello",
                    "files": [],
                    "agents": {},
                    "references": [],
                    "time": {"created": 1002}
                }),
            ),
            (
                "evt_synthetic",
                "synthetic",
                serde_json::json!({
                    "sessionID": "s_child",
                    "text": "synthetic text",
                    "time": {"created": 1003}
                }),
            ),
            (
                "evt_shell",
                "shell",
                serde_json::json!({
                    "callID": "shell_1",
                    "command": "pwd",
                    "output": "/tmp",
                    "time": {"created": 1004, "completed": 1005}
                }),
            ),
            (
                "evt_assistant",
                "assistant",
                serde_json::json!({
                    "agent": "build",
                    "model": {"id": "gpt-5.5", "providerID": "openai", "variant": "default"},
                    "content": [
                        {"type": "text", "text": "assistant text"},
                        {"type": "reasoning", "id": "r1", "text": "assistant thought"}
                    ],
                    "finish": "stop",
                    "time": {"created": 1006, "completed": 1007}
                }),
            ),
            (
                "evt_compaction",
                "compaction",
                serde_json::json!({
                    "reason": "manual",
                    "summary": "compact summary",
                    "include": "tail",
                    "time": {"created": 1008}
                }),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let time = 1000 + idx as i64;
            conn.execute(
                "INSERT INTO session_message
                    (id, session_id, type, time_created, time_updated, data)
                 VALUES (?1, 's_v2_all', ?2, ?3, ?3, ?4)",
                rusqlite::params![id, kind, time, data.to_string()],
            )
            .unwrap();
        }
    }

    let session = providers::opencode::from_db_path(&src_path, "s_v2_all").unwrap();
    let seen = session
        .messages
        .iter()
        .map(|m| m.provenance.source_event_type.as_str())
        .filter_map(|source| source.strip_prefix("opencode:session_message."))
        .collect::<Vec<_>>();
    assert_eq!(
        seen,
        vec![
            "agent-switched",
            "model-switched",
            "user",
            "synthetic",
            "shell",
            "assistant",
            "compaction"
        ]
    );
    assert!(session.messages.iter().any(|m| m.role == Role::User));
    assert!(session.messages.iter().any(|m| m.role == Role::Assistant));
    assert!(session.messages.iter().any(|m| m.flags.is_compaction));

    let dst_path = dir.path().join("dst.db");
    providers::opencode::to_db_path(&session, &dst_path).unwrap();
    let conn = rusqlite::Connection::open(&dst_path).unwrap();
    let legacy_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message WHERE session_id = 's_v2_all'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let session_message_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_message WHERE session_id = 's_v2_all'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_count, 0);
    assert_eq!(session_message_count, 7);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_v2_tool_result_prefers_content_over_empty_structured() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src_path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated)
             VALUES ('s_v2_tool_content', 'global', '/tmp', 'V2 Tool Content', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     1000, 2000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, time_created, time_updated, data)
             VALUES ('evt_assistant_tool', 's_v2_tool_content', 'assistant', 1000, 1000, ?1)",
            [serde_json::json!({
                "agent": "build",
                "model": {"id": "gpt-5.5", "providerID": "openai", "variant": "default"},
                "content": [{
                    "type": "tool",
                    "id": "call_1",
                    "name": "read",
                    "state": {
                        "status": "completed",
                        "input": {"path": "/tmp/a.txt"},
                        "structured": {},
                        "content": [
                            {"type": "text", "text": "file contents"},
                            {"type": "file", "uri": "data:image/png;base64,iVBORw0KGgo=", "mime": "image/png", "name": "shot.png"}
                        ]
                    },
                    "time": {"created": 1000, "completed": 1001}
                }],
                "time": {"created": 1000, "completed": 1001}
            })
            .to_string()],
        )
        .unwrap();
    }

    let session = providers::opencode::from_db_path(&src_path, "s_v2_tool_content").unwrap();
    let output = session
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .find_map(|block| match block {
            cokacmux::ContentBlock::ToolResult { output, .. } => Some(output),
            _ => None,
        })
        .expect("missing tool result");

    assert_eq!(output[0]["text"], "file contents");
    let image_count = session
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter(|block| matches!(block, cokacmux::ContentBlock::Image { .. }))
        .count();
    assert_eq!(image_count, 1);
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_roundtrip_preserves_message_text() {
    use cokacmux::providers::opencode::db;

    // 1) Build a source DB with two messages.
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.db");
    {
        let conn = rusqlite::Connection::open(&src_path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('proj1','/tmp', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, model, agent, time_created, time_updated)
             VALUES ('ses_test','proj1','/tmp','Hello','openai/gpt-5.5','build', 1000, 2000)",
            [],
        )
        .unwrap();
        // user message + text part
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('m1','ses_test', 1000, 1000, '{\"role\":\"user\",\"time\":{\"created\":1000}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('p1','m1','ses_test', 1000, 1000, '{\"type\":\"text\",\"text\":\"hi\"}')",
            [],
        )
        .unwrap();
        // assistant message + text part
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('m2','ses_test', 2000, 2000, '{\"role\":\"assistant\",\"time\":{\"created\":2000}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('p2','m2','ses_test', 2000, 2000, '{\"type\":\"text\",\"text\":\"hi back\"}')",
            [],
        )
        .unwrap();
    }

    // 2) Read into Universal.
    let session = providers::opencode::from_db_path(&src_path, "ses_test").unwrap();
    assert_eq!(session.cwd, "/tmp");
    assert_eq!(session.messages.len(), 2);

    // 3) Write to a fresh DB.
    let dst_path = dir.path().join("dst.db");
    providers::opencode::to_db_path(&session, &dst_path).unwrap();

    // 4) Read back and confirm text bodies.
    let rb = providers::opencode::from_db_path(&dst_path, "ses_test").unwrap();
    let texts: Vec<&str> = rb
        .messages
        .iter()
        .flat_map(|m| {
            m.content.iter().filter_map(|b| {
                if let cokacmux::ContentBlock::Text { text, .. } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
        })
        .collect();
    assert_eq!(texts, vec!["hi", "hi back"]);
    let _ = Provider::OpenCode; // touch enum
}

// -------- cli_version capture (strategy §10.5: provider version drift) --------

#[test]
fn claude_read_captures_cli_version_into_origin() {
    let jsonl = r#"{"type":"user","sessionId":"sess-v","cwd":"/tmp","version":"2.1.145","timestamp":"2026-05-20T01:00:00.000Z","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"hi"}}
"#;
    let session = providers::claude::from_jsonl_str(jsonl, &Default::default()).unwrap();
    assert_eq!(
        session.origin.cli_version.as_deref(),
        Some("2.1.145"),
        "claude per-line `version` must populate origin.cli_version"
    );
}

#[test]
fn codex_read_captures_cli_version_into_origin() {
    let jsonl = codex_fixture();
    let session = providers::codex::from_jsonl_str(jsonl, &Default::default()).unwrap();
    assert_eq!(
        session.origin.cli_version.as_deref(),
        Some("0.131.0"),
        "codex session_meta.payload.cli_version must populate origin.cli_version"
    );
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_read_captures_version_into_origin_cli_version() {
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oc.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        db::ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, title, agent, model,
                                  time_created, time_updated, version)
             VALUES ('ses_v', 'global', '/tmp', 't', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     0, 0, '1.15.5')",
            [],
        )
        .unwrap();
    }
    let session = providers::opencode::from_db_path(&path, "ses_v").unwrap();
    assert_eq!(
        session.origin.cli_version.as_deref(),
        Some("1.15.5"),
        "opencode session.version must populate origin.cli_version"
    );
    // Lossless extras still hold the original column for round-trip.
    assert_eq!(
        session
            .extras
            .get("opencode_version")
            .and_then(|v| v.as_str()),
        Some("1.15.5"),
    );
}
