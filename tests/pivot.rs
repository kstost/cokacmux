//! Adapter pivot tests plus the public cross-provider `convert()` contract.
//! Provider adapters still cover semantic preservation; `convert()` now writes
//! a two-message continuation wrapper rather than a lossless target transcript.

#[cfg(feature = "opencode")]
use cokacmux::session::{
    clone::ArtifactPath,
    install::{install_universal_session, InstallSessionOpts},
    native_validate,
};
use cokacmux::{
    convert, providers, read_session, universal::Role, wrap_session_for_context_convert,
    ContentBlock, ImageSource, Provider, SessionSource, SessionTarget, UniversalSession,
    CONTEXT_ACK, CONTEXT_CONTINUATION_PROMPT,
};
use serde_json::Value;

fn claude_fixture() -> &'static str {
    r#"{"type":"user","sessionId":"sess-claude-1","cwd":"/tmp","timestamp":"2026-05-20T01:00:00.000Z","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"hello there"}}
{"type":"assistant","sessionId":"sess-claude-1","cwd":"/tmp","timestamp":"2026-05-20T01:00:01.000Z","uuid":"a1","parentUuid":"u1","message":{"role":"assistant","id":"msg_xxx","model":"claude-opus-4-7","content":[{"type":"text","text":"hi back"}]}}
"#
}

fn codex_fixture() -> &'static str {
    r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"sess-codex-1","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:00.100Z","type":"turn_context","payload":{"model":"gpt-5.5"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello there"}]}}
{"timestamp":"2026-05-20T01:00:01.500Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi back"}]}}
"#
}

fn pi_fixture() -> &'static str {
    r#"{"type":"session","version":3,"id":"sess-pi-1","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp"}
{"type":"message","id":"u1","parentId":null,"timestamp":"2026-05-20T01:00:00.500Z","message":{"role":"user","content":"hello there","timestamp":1779240000500}}
{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-05-20T01:00:01.500Z","message":{"role":"assistant","content":[{"type":"text","text":"hi back"}],"provider":"openai","model":"gpt-5.5","usage":{"input":3,"output":2,"totalTokens":5,"cost":{"total":0}},"stopReason":"stop","timestamp":1779240001500}}
"#
}

#[cfg(feature = "gjc")]
fn gjc_fixture() -> &'static str {
    r#"{"type":"session","version":3,"id":"sess-gjc-1","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp","title":"GJC title","titleSource":"user"}
{"type":"message","id":"u1","parentId":null,"timestamp":"2026-05-20T01:00:00.500Z","message":{"role":"user","content":"hello there","timestamp":1779240000500}}
{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-05-20T01:00:01.500Z","message":{"role":"assistant","content":[{"type":"text","text":"hi back"}],"provider":"openai","model":"gpt-5.5","usage":{"input":3,"output":2,"totalTokens":5,"cost":{"total":0}},"stopReason":"stop","timestamp":1779240001500}}
"#
}

fn user_assistant_texts(session: &UniversalSession) -> Vec<&str> {
    session
        .messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .flat_map(|m| {
            m.content.iter().filter_map(|b| match b {
                ContentBlock::Text { text, .. } if !text.is_empty() => Some(text.as_str()),
                _ => None,
            })
        })
        .collect()
}

fn texts_for_role(session: &UniversalSession, role: Role) -> Vec<&str> {
    session
        .messages
        .iter()
        .filter(|m| m.role == role && !m.flags.is_meta)
        .flat_map(|m| {
            m.content.iter().filter_map(|b| match b {
                ContentBlock::Text { text, .. } if !text.is_empty() => Some(text.as_str()),
                _ => None,
            })
        })
        .collect()
}

fn jsonl_values(jsonl: &str) -> Vec<Value> {
    jsonl
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn claude_conversation_lines(values: &[Value]) -> Vec<&Value> {
    values
        .iter()
        .filter(|v| {
            matches!(
                v.get("type").and_then(|t| t.as_str()),
                Some("user" | "assistant")
            )
        })
        .collect()
}

#[derive(Debug, PartialEq)]
struct SemanticProfile {
    cwd: String,
    fragments: Vec<String>,
    tool_uses: Vec<(String, String, Value)>,
    tool_results: Vec<(String, String, bool)>,
    images: Vec<(String, String)>,
}

fn semantic_profile(session: &UniversalSession) -> SemanticProfile {
    let mut fragments = Vec::new();
    let mut tool_uses = Vec::new();
    let mut tool_results = Vec::new();
    let mut images = Vec::new();

    for msg in session.messages.iter().filter(|m| !m.flags.is_meta) {
        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } if !text.is_empty() => {
                    fragments.push(text.clone());
                }
                ContentBlock::Thinking { text, .. } if !text.is_empty() => {
                    fragments.push(text.clone());
                }
                ContentBlock::ToolUse {
                    call_id,
                    name,
                    input,
                    ..
                } => {
                    tool_uses.push((call_id.clone(), name.clone(), input.clone()));
                }
                ContentBlock::ToolResult {
                    call_id,
                    output,
                    is_error,
                    ..
                } => {
                    tool_results.push((call_id.clone(), normalized_output(output), *is_error));
                }
                ContentBlock::Image { mime, source, .. } => {
                    images.push((mime.clone(), image_source_key(source)));
                }
                _ => {}
            }
        }
    }

    SemanticProfile {
        cwd: session.cwd.clone(),
        fragments,
        tool_uses,
        tool_results,
        images,
    }
}

fn normalized_output(output: &Value) -> String {
    output
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| output.to_string())
}

fn image_source_key(source: &ImageSource) -> String {
    match source {
        ImageSource::LocalPath { path } => format!("local:{path}"),
        ImageSource::Base64 { data } => format!("base64:{data}"),
        ImageSource::Url { url } => format!("url:{url}"),
    }
}

fn rich_claude_fixture() -> &'static str {
    r#"{"type":"permission-mode","permissionMode":"default","sessionId":"rich-claude-1"}
{"type":"ai-title","sessionId":"rich-claude-1","aiTitle":"Rich Claude"}
{"type":"user","sessionId":"rich-claude-1","cwd":"/tmp/rich","timestamp":"2026-05-20T01:00:00.000Z","uuid":"u-rich-claude","parentUuid":null,"message":{"role":"user","content":[{"type":"text","text":"please inspect repo"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]}}
{"type":"assistant","sessionId":"rich-claude-1","cwd":"/tmp/rich","timestamp":"2026-05-20T01:00:01.000Z","uuid":"a-rich-claude-1","parentUuid":"u-rich-claude","message":{"role":"assistant","id":"msg_rich_claude_1","model":"claude-sonnet-4-5","content":[{"type":"thinking","thinking":"Need repo context","signature":"sig-rich"},{"type":"text","text":"I'll inspect the project."},{"type":"tool_use","id":"call_rich_claude","name":"shell","input":{"command":"ls"}}],"stop_reason":"tool_use","usage":{"input_tokens":12,"output_tokens":6}}}
{"type":"user","sessionId":"rich-claude-1","cwd":"/tmp/rich","timestamp":"2026-05-20T01:00:02.000Z","uuid":"tr-rich-claude","parentUuid":"a-rich-claude-1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_rich_claude","content":"permission denied","is_error":true}]}}
{"type":"assistant","sessionId":"rich-claude-1","cwd":"/tmp/rich","timestamp":"2026-05-20T01:00:03.000Z","uuid":"a-rich-claude-2","parentUuid":"tr-rich-claude","message":{"role":"assistant","id":"msg_rich_claude_2","model":"claude-sonnet-4-5","content":[{"type":"text","text":"Reported the access failure."}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":4}}}
"#
}

fn rich_codex_fixture() -> &'static str {
    r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"rich-codex-1","cwd":"/tmp/rich","cli_version":"0.131.0","git":{"branch":"main","commit_hash":"abc123"}}}
{"timestamp":"2026-05-20T01:00:00.100Z","type":"turn_context","payload":{"model":"gpt-5.5","model_provider":"openai","model_reasoning_effort":"medium"}}
{"timestamp":"2026-05-20T01:00:00.400Z","type":"event_msg","payload":{"type":"user_message","message":"please inspect repo","images":["https://example.test/screenshot.png"],"local_images":[],"text_elements":[]}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"please inspect repo"}],"id":"u-rich-codex"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"reasoning","id":"reason-rich-codex","summary":[{"type":"summary_text","text":"Need repo context"}],"content":null}}
{"timestamp":"2026-05-20T01:00:01.200Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I'll inspect the project."}],"id":"a-rich-codex-1"}}
{"timestamp":"2026-05-20T01:00:01.400Z","type":"response_item","payload":{"type":"function_call","id":"fc-rich-codex","name":"shell","call_id":"call_rich_codex","arguments":"{\"command\":\"ls\"}"}}
{"timestamp":"2026-05-20T01:00:02.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_rich_codex","output":"permission denied","is_error":true}}
{"timestamp":"2026-05-20T01:00:03.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Reported the access failure."}],"id":"a-rich-codex-2"}}
{"timestamp":"2026-05-20T01:00:04.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":17,"output_tokens":10,"total_tokens":27}}}}
"#
}

fn rich_pi_fixture() -> &'static str {
    r#"{"type":"session","version":3,"id":"rich-pi-1","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp/rich"}
{"type":"session_info","id":"info-rich-pi","parentId":null,"timestamp":"2026-05-20T01:00:00.100Z","name":"Rich Pi"}
{"type":"message","id":"u-rich-pi","parentId":"info-rich-pi","timestamp":"2026-05-20T01:00:00.500Z","message":{"role":"user","content":[{"type":"text","text":"please inspect repo"},{"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="}],"timestamp":1779240000500}}
{"type":"message","id":"a-rich-pi-1","parentId":"u-rich-pi","timestamp":"2026-05-20T01:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need repo context"},{"type":"text","text":"I'll inspect the project."},{"type":"toolCall","id":"call_rich_pi","name":"shell","arguments":{"command":"ls"}}],"provider":"openai","model":"gpt-5.5","usage":{"input":12,"output":6,"totalTokens":18,"cost":{"total":0.42}},"stopReason":"tool_use","timestamp":1779240001000}}
{"type":"message","id":"tr-rich-pi","parentId":"a-rich-pi-1","timestamp":"2026-05-20T01:00:02.000Z","message":{"role":"toolResult","toolCallId":"call_rich_pi","toolName":"shell","content":"permission denied","isError":true,"timestamp":1779240002000}}
{"type":"message","id":"a-rich-pi-2","parentId":"tr-rich-pi","timestamp":"2026-05-20T01:00:03.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Reported the access failure."}],"provider":"openai","model":"gpt-5.5","usage":{"input":5,"output":4,"totalTokens":9,"cost":{"total":0}},"stopReason":"stop","timestamp":1779240003000}}
"#
}

#[cfg(feature = "gjc")]
fn rich_gjc_fixture() -> &'static str {
    r#"{"type":"session","version":3,"id":"rich-gjc-1","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp/rich","title":"Rich GJC","titleSource":"user"}
{"type":"message","id":"u-rich-gjc","parentId":null,"timestamp":"2026-05-20T01:00:00.500Z","message":{"role":"user","content":[{"type":"text","text":"please inspect repo"},{"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="}],"timestamp":1779240000500}}
{"type":"message","id":"a-rich-gjc-1","parentId":"u-rich-gjc","timestamp":"2026-05-20T01:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need repo context"},{"type":"text","text":"I'll inspect the project."},{"type":"toolCall","id":"call_rich_gjc","name":"shell","arguments":{"command":"ls"}}],"provider":"openai","model":"gpt-5.5","usage":{"input":12,"output":6,"totalTokens":18,"cost":{"total":0.42}},"stopReason":"tool_use","timestamp":1779240001000}}
{"type":"message","id":"tr-rich-gjc","parentId":"a-rich-gjc-1","timestamp":"2026-05-20T01:00:02.000Z","message":{"role":"toolResult","toolCallId":"call_rich_gjc","toolName":"shell","content":"permission denied","isError":true,"timestamp":1779240002000}}
{"type":"message","id":"a-rich-gjc-2","parentId":"tr-rich-gjc","timestamp":"2026-05-20T01:00:03.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Reported the access failure."}],"provider":"openai","model":"gpt-5.5","usage":{"input":5,"output":4,"totalTokens":9,"cost":{"total":0}},"stopReason":"stop","timestamp":1779240003000}}
"#
}

fn write_rich_opencode_fixture(db_path: &std::path::Path) {
    use cokacmux::providers::opencode::db;

    let conn = rusqlite::Connection::open(db_path).unwrap();
    db::ensure_schema(&conn).unwrap();
    conn.execute(
        "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
         VALUES ('global', '/', 1779240000000, 1779240004000, '{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session
            (id, project_id, directory, title, agent, model, cost,
             tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write,
             time_created, time_updated)
         VALUES
            ('rich-opencode-1', 'global', '/tmp/rich', 'Rich OpenCode',
             'build', '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"medium\"}', 0.42,
             17, 10, 3, 2, 0, 1779240000000, 1779240004000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES
            ('u-rich-opencode', 'rich-opencode-1', 1779240000500, 1779240000500,
             '{\"role\":\"user\",\"time\":{\"created\":1779240000500}}'),
            ('a-rich-opencode-1', 'rich-opencode-1', 1779240001000, 1779240001400,
             '{\"role\":\"assistant\",\"time\":{\"created\":1779240001000},\"model\":{\"providerID\":\"openai\",\"modelID\":\"gpt-5.5\",\"variant\":\"medium\"},\"finish\":\"tool_use\"}'),
            ('tr-rich-opencode', 'rich-opencode-1', 1779240002000, 1779240002000,
             '{\"role\":\"tool\",\"time\":{\"created\":1779240002000}}'),
            ('a-rich-opencode-2', 'rich-opencode-1', 1779240003000, 1779240003000,
             '{\"role\":\"assistant\",\"time\":{\"created\":1779240003000},\"model\":{\"providerID\":\"openai\",\"modelID\":\"gpt-5.5\",\"variant\":\"medium\"},\"finish\":\"stop\"}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES
            ('p-u-rich-opencode', 'u-rich-opencode', 'rich-opencode-1', 1779240000500, 1779240000500,
             '{\"type\":\"text\",\"text\":\"please inspect repo\"}'),
            ('p-u-rich-opencode-image', 'u-rich-opencode', 'rich-opencode-1', 1779240000510, 1779240000510,
             '{\"type\":\"file\",\"mime\":\"image/png\",\"source\":{\"kind\":\"base64\",\"data\":\"iVBORw0KGgo=\"}}'),
            ('p-a-rich-opencode-thinking', 'a-rich-opencode-1', 'rich-opencode-1', 1779240001000, 1779240001000,
             '{\"type\":\"reasoning\",\"text\":\"Need repo context\"}'),
            ('p-a-rich-opencode-text', 'a-rich-opencode-1', 'rich-opencode-1', 1779240001200, 1779240001200,
             '{\"type\":\"text\",\"text\":\"I''ll inspect the project.\"}'),
            ('p-a-rich-opencode-tool', 'a-rich-opencode-1', 'rich-opencode-1', 1779240001400, 1779240001400,
             '{\"type\":\"tool\",\"tool\":\"shell\",\"callID\":\"call_rich_opencode\",\"state\":{\"status\":\"pending\",\"input\":{\"command\":\"ls\"}}}'),
            ('p-tr-rich-opencode', 'tr-rich-opencode', 'rich-opencode-1', 1779240002000, 1779240002000,
             '{\"type\":\"tool\",\"callID\":\"call_rich_opencode\",\"state\":{\"status\":\"error\",\"output\":\"permission denied\"}}'),
            ('p-a-rich-opencode-final', 'a-rich-opencode-2', 'rich-opencode-1', 1779240003000, 1779240003000,
             '{\"type\":\"text\",\"text\":\"Reported the access failure.\"}')",
        [],
    )
    .unwrap();
}

#[cfg(feature = "opencode")]
fn create_codex_threads_table(path: &std::path::Path) {
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

fn write_native_source(provider: Provider, dir: &std::path::Path) -> SessionSource {
    match provider {
        Provider::Claude => {
            let path = dir.join("rich-claude.jsonl");
            std::fs::write(&path, rich_claude_fixture()).unwrap();
            SessionSource::Path(path)
        }
        Provider::Codex => {
            let path = dir.join("rich-codex.jsonl");
            std::fs::write(&path, rich_codex_fixture()).unwrap();
            SessionSource::Path(path)
        }
        Provider::Pi => {
            let path = dir.join("rich-pi.jsonl");
            std::fs::write(&path, rich_pi_fixture()).unwrap();
            SessionSource::Path(path)
        }
        #[cfg(feature = "gjc")]
        Provider::Gjc => {
            let path = dir.join("rich-gjc.jsonl");
            std::fs::write(&path, rich_gjc_fixture()).unwrap();
            SessionSource::Path(path)
        }
        Provider::OpenCode => {
            let db_path = dir.join("rich-opencode.db");
            write_rich_opencode_fixture(&db_path);
            SessionSource::OpenCodeDb {
                db_path,
                session_id: "rich-opencode-1".to_string(),
            }
        }
    }
}

fn target_for(provider: Provider, session_id: &str, dir: &std::path::Path) -> SessionTarget {
    match provider {
        Provider::Claude => SessionTarget::Path(dir.join("target-claude.jsonl")),
        Provider::Codex => SessionTarget::Path(dir.join("target-codex.jsonl")),
        Provider::Pi => SessionTarget::Path(dir.join("target-pi.jsonl")),
        Provider::Gjc => SessionTarget::Path(dir.join("target-gjc.jsonl")),
        Provider::OpenCode => SessionTarget::OpenCodeDb {
            db_path: dir.join(format!("target-opencode-{session_id}.db")),
        },
    }
}

fn source_from_target(
    provider: Provider,
    target: &SessionTarget,
    session_id: &str,
) -> SessionSource {
    match (provider, target) {
        (
            Provider::Claude | Provider::Codex | Provider::Pi | Provider::Gjc,
            SessionTarget::Path(path),
        ) => SessionSource::Path(path.clone()),
        (Provider::OpenCode, SessionTarget::OpenCodeDb { db_path }) => SessionSource::OpenCodeDb {
            db_path: db_path.clone(),
            session_id: session_id.to_string(),
        },
        _ => panic!("target/provider mismatch"),
    }
}

#[cfg(feature = "opencode")]
fn pivot_providers() -> Vec<Provider> {
    let mut providers = vec![Provider::Claude, Provider::Codex, Provider::OpenCode];
    #[cfg(feature = "pi")]
    providers.push(Provider::Pi);
    #[cfg(feature = "gjc")]
    providers.push(Provider::Gjc);
    providers
}

#[test]
fn claude_to_codex_preserves_text() {
    let session = providers::claude::from_jsonl_str(claude_fixture(), &Default::default()).unwrap();
    let codex_out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    let back = providers::codex::from_jsonl_str(&codex_out, &Default::default()).unwrap();
    assert_eq!(user_assistant_texts(&back), vec!["hello there", "hi back"]);
}

#[cfg(feature = "pi")]
#[test]
fn pi_to_codex_preserves_text() {
    let session = providers::pi::from_jsonl_str(pi_fixture(), &Default::default()).unwrap();
    let codex_out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    let back = providers::codex::from_jsonl_str(&codex_out, &Default::default()).unwrap();
    assert_eq!(user_assistant_texts(&back), vec!["hello there", "hi back"]);
}

#[cfg(feature = "gjc")]
#[test]
fn gjc_to_codex_preserves_text() {
    let session = providers::gjc::from_jsonl_str(gjc_fixture(), &Default::default()).unwrap();
    let codex_out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    let back = providers::codex::from_jsonl_str(&codex_out, &Default::default()).unwrap();
    assert_eq!(user_assistant_texts(&back), vec!["hello there", "hi back"]);
}

#[test]
fn claude_to_codex_emits_resume_compatible_jsonl() {
    let src = r#"{"type":"permission-mode","permissionMode":"default","sessionId":"sess-claude-1"}
{"type":"user","sessionId":"sess-claude-1","cwd":"/tmp","timestamp":"2026-05-20T01:00:00.000Z","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"hello there"}}
{"type":"assistant","sessionId":"sess-claude-1","cwd":"/tmp","timestamp":"2026-05-20T01:00:01.000Z","uuid":"a1","parentUuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"hi back"}]}}
"#;
    let session = providers::claude::from_jsonl_str(src, &Default::default()).unwrap();
    let codex_out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();

    assert!(!codex_out.contains("\"timestamp\":null"));
    assert!(!codex_out.contains("synthesized."));
    let first_line: serde_json::Value =
        serde_json::from_str(codex_out.lines().next().unwrap()).unwrap();
    let meta_payload = first_line.get("payload").unwrap();
    for key in ["timestamp", "source", "model_provider", "base_instructions"] {
        assert!(
            meta_payload.get(key).is_some(),
            "session_meta payload should include {key}"
        );
    }
    for line in codex_out.lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        if value.get("type").and_then(|v| v.as_str()) == Some("response_item") {
            assert!(value
                .get("payload")
                .and_then(|payload| payload.get("id"))
                .is_none());
        }
    }
    assert!(
        codex_out.contains(r#""type":"user_message""#),
        "Codex TUI transcript display needs event_msg.user_message"
    );
    assert!(
        codex_out.contains(r#""type":"agent_message""#),
        "Codex TUI transcript display needs event_msg.agent_message"
    );
}

#[test]
fn codex_to_claude_preserves_text() {
    let session = providers::codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let claude_out = providers::claude::to_jsonl_string(&session, &Default::default()).unwrap();
    let back = providers::claude::from_jsonl_str(&claude_out, &Default::default()).unwrap();
    assert_eq!(user_assistant_texts(&back), vec!["hello there", "hi back"]);
}

#[test]
fn codex_failed_tool_outputs_are_marked_as_errors() {
    let src = r#"{"timestamp":"2026-05-21T15:21:52.000Z","type":"session_meta","payload":{"id":"failed-codex-tools","cwd":"/tmp/cokacmux-agent-corpus"}}
{"timestamp":"2026-05-21T15:21:53.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"call_exec","arguments":"{\"cmd\":\"wc -l sample.txt\"}"}}
{"timestamp":"2026-05-21T15:21:54.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_exec","output":"Chunk ID: abc\nWall time: 0.0000 seconds\nProcess exited with code 1\nOutput:\nbwrap failed\n"}}
{"timestamp":"2026-05-21T15:21:55.000Z","type":"response_item","payload":{"type":"custom_tool_call","status":"completed","call_id":"call_patch","name":"apply_patch","input":"*** Begin Patch\n*** Add File: agent-output/codex.txt\n+x\n*** End Patch\n"}}
{"timestamp":"2026-05-21T15:21:56.000Z","type":"event_msg","payload":{"type":"patch_apply_end","call_id":"call_patch","stdout":"","stderr":"Failed to write file\n","success":false,"status":"failed"}}
{"timestamp":"2026-05-21T15:21:57.000Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_patch","output":"Exit code: 1\nWall time: 0.1 seconds\nOutput:\nFailed to write file\n"}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let results = session
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                call_id, is_error, ..
            } => Some((call_id.as_str(), *is_error)),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(results, vec![("call_exec", true), ("call_patch", true)]);
}

#[test]
fn codex_to_claude_emits_resume_compatible_jsonl() {
    let session = providers::codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let claude_out = providers::claude::to_jsonl_string(&session, &Default::default()).unwrap();
    let values = jsonl_values(&claude_out);

    assert_eq!(
        values[0].get("type").and_then(|v| v.as_str()),
        Some("custom-title")
    );
    assert!(
        values
            .iter()
            .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("agent-name")),
        "synthetic Claude sessions should include a native agent-name record"
    );
    assert!(
        values
            .iter()
            .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("queue-operation")),
        "synthetic Claude sessions should include queue-operation records"
    );
    assert!(
        values
            .iter()
            .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("last-prompt")),
        "synthetic Claude sessions should include a native last-prompt record"
    );
    assert!(
        !claude_out.contains("synthesized from"),
        "provider meta should not be emitted as visible Claude transcript rows"
    );
    assert!(
        !claude_out.contains(r#""type":"system""#),
        "Codex meta/system records should be skipped for Claude resume"
    );

    let conversation = claude_conversation_lines(&values);
    assert_eq!(conversation.len(), 2);

    let user = conversation[0];
    assert_eq!(user.get("type").and_then(|v| v.as_str()), Some("user"));
    assert!(user.get("parentUuid").unwrap().is_null());
    assert_eq!(
        user.get("isSidechain").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        user.get("userType").and_then(|v| v.as_str()),
        Some("external")
    );
    assert_eq!(
        user.get("entrypoint").and_then(|v| v.as_str()),
        Some("sdk-cli")
    );
    assert!(user.get("version").and_then(|v| v.as_str()).is_some());
    assert!(uuid::Uuid::parse_str(user.get("uuid").unwrap().as_str().unwrap()).is_ok());
    assert_eq!(
        user.pointer("/message/content").and_then(|v| v.as_str()),
        Some("hello there")
    );

    let assistant = conversation[1];
    assert_eq!(
        assistant.get("type").and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert_eq!(
        assistant.get("parentUuid").and_then(|v| v.as_str()),
        user.get("uuid").and_then(|v| v.as_str())
    );
    assert_eq!(
        assistant.pointer("/message/type").and_then(|v| v.as_str()),
        Some("message")
    );
    assert!(assistant
        .pointer("/message/id")
        .and_then(|v| v.as_str())
        .unwrap()
        .starts_with("msg_"));
    assert_eq!(
        assistant
            .pointer("/message/content/0/text")
            .and_then(|v| v.as_str()),
        Some("hi back")
    );
}

#[cfg(feature = "opencode")]
#[test]
fn all_configured_cross_provider_conversions_wrap_source_context() {
    let providers = pivot_providers();
    for from in providers.iter().copied() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = write_native_source(from, source_dir.path());
        let source_session = read_session(from, &source).unwrap();
        let source_profile = semantic_profile(&source_session);

        for to in providers.iter().copied().filter(|to| *to != from) {
            let target_dir = tempfile::tempdir().unwrap();
            let target = target_for(to, &source_session.session_id, target_dir.path());
            let converted_source = convert(from, to, &source, &target).unwrap();
            assert_eq!(
                converted_source.messages.len(),
                2,
                "{from:?} -> {to:?} convert() should return a two-message wrapper"
            );
            assert_context_wrapper(
                from,
                to,
                &source_session,
                &source_profile,
                &converted_source,
            );

            if let (Provider::OpenCode, SessionTarget::OpenCodeDb { db_path }) = (to, &target) {
                let report = native_validate::validate_clone_artifact(
                    Provider::OpenCode,
                    &converted_source.session_id,
                    &ArtifactPath::OpenCodeDb {
                        db_path: db_path.clone(),
                        session_id: converted_source.session_id.clone(),
                    },
                )
                .unwrap();
                assert!(
                    report.ok,
                    "{from:?} -> {to:?} OpenCode wrapper should pass native validation: {}",
                    report.failure_summary()
                );
            }
            if let (Provider::Pi, SessionTarget::Path(path)) = (to, &target) {
                let report = native_validate::validate_clone_artifact(
                    Provider::Pi,
                    &converted_source.session_id,
                    &ArtifactPath::File(path.clone()),
                )
                .unwrap();
                assert!(
                    report.ok,
                    "{from:?} -> {to:?} Pi wrapper should pass native validation: {}",
                    report.failure_summary()
                );
            }
            if let (Provider::Gjc, SessionTarget::Path(path)) = (to, &target) {
                let report = native_validate::validate_clone_artifact(
                    Provider::Gjc,
                    &converted_source.session_id,
                    &ArtifactPath::File(path.clone()),
                )
                .unwrap();
                assert!(
                    report.ok,
                    "{from:?} -> {to:?} GJC wrapper should pass native validation: {}",
                    report.failure_summary()
                );
            }

            let target_source = source_from_target(to, &target, &converted_source.session_id);
            let converted = read_session(to, &target_source).unwrap();
            assert_context_wrapper(from, to, &source_session, &source_profile, &converted);
        }
    }
}

#[cfg(feature = "opencode")]
#[test]
fn rich_cross_provider_wrappers_install_into_native_live_layouts() {
    let providers = pivot_providers();
    for from in providers.iter().copied() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = write_native_source(from, source_dir.path());
        let source_session = read_session(from, &source).unwrap();
        let source_profile = semantic_profile(&source_session);

        for to in providers.iter().copied().filter(|to| *to != from) {
            let converted = wrap_session_for_context_convert(&source_session, to);
            assert_eq!(
                converted.messages.len(),
                2,
                "{from:?} -> {to:?} wrapper should synthesize exactly two visible messages"
            );

            let target_dir = tempfile::tempdir().unwrap();
            let mut opts = InstallSessionOpts::default();
            match to {
                Provider::Claude => {
                    opts.claude_home = Some(target_dir.path().join(".claude"));
                }
                Provider::Codex => {
                    let state_path = target_dir.path().join("state_5.sqlite");
                    create_codex_threads_table(&state_path);
                    opts.codex_home = Some(target_dir.path().join(".codex"));
                    opts.codex_state_5_path = Some(state_path);
                }
                Provider::OpenCode => {
                    opts.opencode_db_path = Some(target_dir.path().join("opencode.db"));
                }
                Provider::Pi => {
                    opts.pi_agent_dir = Some(target_dir.path().join(".pi").join("agent"));
                }
                Provider::Gjc => {
                    opts.gjc_agent_dir = Some(target_dir.path().join(".gjc").join("agent"));
                }
            }

            let codex_state_5_path = opts.codex_state_5_path.clone();
            let report = install_universal_session(to, &converted, &opts).unwrap();
            assert!(
                report.validation.ok,
                "{from:?} -> {to:?} installed wrapper should pass native validation: {}",
                report.validation.failure_summary()
            );
            assert_installed_native_wrapper_shape(
                to,
                &report.artifact,
                &report.session_id,
                codex_state_5_path.as_deref(),
            );
            let back = read_installed_session(to, &report.artifact, &report.session_id);
            assert_context_wrapper(from, to, &source_session, &source_profile, &back);
        }
    }
}

#[cfg(feature = "opencode")]
fn read_installed_session(
    provider: Provider,
    artifact: &ArtifactPath,
    session_id: &str,
) -> UniversalSession {
    match (provider, artifact) {
        (Provider::Claude, ArtifactPath::File(path)) => {
            providers::claude::from_file(path, &Default::default()).unwrap()
        }
        (Provider::Codex, ArtifactPath::File(path)) => providers::codex::from_file(path).unwrap(),
        #[cfg(feature = "pi")]
        (Provider::Pi, ArtifactPath::File(path)) => providers::pi::from_file(path).unwrap(),
        #[cfg(feature = "gjc")]
        (Provider::Gjc, ArtifactPath::File(path)) => providers::gjc::from_file(path).unwrap(),
        (Provider::OpenCode, ArtifactPath::OpenCodeDb { db_path, .. }) => {
            providers::opencode::from_db_path(db_path, session_id).unwrap()
        }
        _ => panic!("provider/artifact mismatch: {provider:?} {artifact:?}"),
    }
}

#[cfg(feature = "opencode")]
fn assert_installed_native_wrapper_shape(
    provider: Provider,
    artifact: &ArtifactPath,
    session_id: &str,
    codex_state_5_path: Option<&std::path::Path>,
) {
    match (provider, artifact) {
        (Provider::Claude, ArtifactPath::File(path)) => {
            assert_claude_native_wrapper_shape(path, session_id);
        }
        (Provider::Codex, ArtifactPath::File(path)) => {
            assert_codex_native_wrapper_shape(path, session_id, codex_state_5_path);
        }
        (Provider::Pi, ArtifactPath::File(path)) => {
            assert_pi_native_wrapper_shape(path, session_id);
        }
        (Provider::Gjc, ArtifactPath::File(path)) => {
            assert_gjc_native_wrapper_shape(path, session_id);
        }
        (Provider::OpenCode, ArtifactPath::OpenCodeDb { db_path, .. }) => {
            assert_opencode_native_wrapper_shape(db_path, session_id);
        }
        _ => panic!("provider/artifact mismatch: {provider:?} {artifact:?}"),
    }
}

#[cfg(feature = "opencode")]
fn jsonl_file_values(path: &std::path::Path) -> Vec<Value> {
    jsonl_values(&std::fs::read_to_string(path).unwrap())
}

#[cfg(feature = "opencode")]
fn assert_claude_native_wrapper_shape(path: &std::path::Path, session_id: &str) {
    let values = jsonl_file_values(path);
    let conversation = claude_conversation_lines(&values);
    assert_eq!(
        conversation.len(),
        2,
        "Claude wrapper artifact should have exactly two conversation rows"
    );
    assert!(
        values
            .iter()
            .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("queue-operation")),
        "Claude wrapper should include native queue-operation rows"
    );
    assert!(
        values
            .iter()
            .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("last-prompt")),
        "Claude wrapper should include a native last-prompt row"
    );

    let user = conversation[0];
    let assistant = conversation[1];
    assert_eq!(user.get("type").and_then(|v| v.as_str()), Some("user"));
    assert_eq!(
        assistant.get("type").and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert_eq!(
        user.get("sessionId").and_then(|v| v.as_str()),
        Some(session_id)
    );
    assert_eq!(
        assistant.get("sessionId").and_then(|v| v.as_str()),
        Some(session_id)
    );
    assert!(uuid::Uuid::parse_str(user.get("uuid").unwrap().as_str().unwrap()).is_ok());
    assert!(uuid::Uuid::parse_str(assistant.get("uuid").unwrap().as_str().unwrap()).is_ok());
    assert!(user.get("parentUuid").unwrap().is_null());
    assert_eq!(
        assistant.get("parentUuid").and_then(|v| v.as_str()),
        user.get("uuid").and_then(|v| v.as_str())
    );
    assert!(user.get("timestamp").and_then(|v| v.as_str()).is_some());
    assert!(assistant
        .get("timestamp")
        .and_then(|v| v.as_str())
        .is_some());
    assert_eq!(
        user.pointer("/message/role").and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        assistant.pointer("/message/role").and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert!(
        user.pointer("/message/content")
            .and_then(|v| v.as_str())
            .unwrap()
            .ends_with(CONTEXT_CONTINUATION_PROMPT),
        "Claude wrapper user content should end with the continuation prompt"
    );
    assert!(assistant
        .pointer("/message/id")
        .and_then(|v| v.as_str())
        .unwrap()
        .starts_with("msg_"));
    assert_eq!(
        assistant
            .pointer("/message/content/0/type")
            .and_then(|v| v.as_str()),
        Some("text")
    );
    assert_eq!(
        assistant
            .pointer("/message/content/0/text")
            .and_then(|v| v.as_str()),
        Some(CONTEXT_ACK)
    );
}

#[cfg(feature = "opencode")]
fn assert_codex_native_wrapper_shape(
    path: &std::path::Path,
    session_id: &str,
    state_5_path: Option<&std::path::Path>,
) {
    let values = jsonl_file_values(path);
    let session_meta = values
        .iter()
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("session_meta"))
        .expect("Codex wrapper should include session_meta");
    assert_eq!(
        session_meta.pointer("/payload/id").and_then(|v| v.as_str()),
        Some(session_id)
    );
    assert!(session_meta
        .pointer("/payload/cwd")
        .and_then(|v| v.as_str())
        .map(|cwd| !cwd.is_empty())
        .unwrap_or(false));
    assert!(
        values
            .iter()
            .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("turn_context")),
        "Codex wrapper should include a native turn_context row"
    );
    assert!(
        !values.iter().any(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some("response_item")
                && v.pointer("/payload/type").and_then(|t| t.as_str()) == Some("reasoning")
                && v.pointer("/payload/summary")
                    .and_then(|summary| summary.as_array())
                    .map(|summary| summary.is_empty())
                    .unwrap_or(false)
                && v.pointer("/payload/encrypted_content").is_none()
        }),
        "Codex wrapper should not synthesize empty reasoning items"
    );

    let response_messages = values
        .iter()
        .filter(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some("response_item")
                && v.pointer("/payload/type").and_then(|t| t.as_str()) == Some("message")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        response_messages.len(),
        2,
        "Codex wrapper artifact should have exactly two response_item.message rows"
    );
    assert_eq!(
        response_messages[0]
            .pointer("/payload/role")
            .and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        response_messages[0]
            .pointer("/payload/content/0/type")
            .and_then(|v| v.as_str()),
        Some("input_text")
    );
    assert!(response_messages[0]
        .pointer("/payload/content/0/text")
        .and_then(|v| v.as_str())
        .unwrap()
        .ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert_eq!(
        response_messages[1]
            .pointer("/payload/role")
            .and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert_eq!(
        response_messages[1]
            .pointer("/payload/phase")
            .and_then(|v| v.as_str()),
        Some("final_answer")
    );
    assert_eq!(
        response_messages[1]
            .pointer("/payload/content/0/type")
            .and_then(|v| v.as_str()),
        Some("output_text")
    );
    assert_eq!(
        response_messages[1]
            .pointer("/payload/content/0/text")
            .and_then(|v| v.as_str()),
        Some(CONTEXT_ACK)
    );

    for required in [
        "task_started",
        "user_message",
        "agent_message",
        "token_count",
        "task_complete",
    ] {
        assert!(
            values.iter().any(|v| {
                v.get("type").and_then(|t| t.as_str()) == Some("event_msg")
                    && v.pointer("/payload/type").and_then(|t| t.as_str()) == Some(required)
            }),
            "Codex wrapper should include event_msg.{required}"
        );
    }

    if let Some(state_5_path) = state_5_path {
        let conn = rusqlite::Connection::open(state_5_path).unwrap();
        let (
            rollout_path,
            source,
            model_provider,
            cwd,
            title,
            sandbox_policy,
            approval_mode,
            has_user_event,
            first_user_message,
            preview,
            memory_mode,
            model,
            reasoning_effort,
        ): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT rollout_path, source, model_provider, cwd, title,
                        sandbox_policy, approval_mode, has_user_event,
                        first_user_message, preview, memory_mode, model, reasoning_effort
                 FROM threads WHERE id = ?1",
                rusqlite::params![session_id],
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
                    ))
                },
            )
            .unwrap();
        assert_eq!(rollout_path, path.display().to_string());
        assert!(!source.is_empty());
        assert!(!model_provider.is_empty());
        assert!(!cwd.is_empty());
        assert!(!title.is_empty());
        assert!(serde_json::from_str::<Value>(&sandbox_policy)
            .unwrap()
            .get("type")
            .is_some());
        assert!(!approval_mode.is_empty());
        assert_eq!(has_user_event, 1);
        assert!(first_user_message.ends_with(CONTEXT_CONTINUATION_PROMPT));
        assert!(preview.contains("==="));
        assert_eq!(memory_mode, "enabled");
        assert!(model.map(|m| !m.is_empty()).unwrap_or(false));
        assert!(reasoning_effort.map(|e| !e.is_empty()).unwrap_or(false));
    }
}

#[cfg(feature = "opencode")]
fn assert_pi_native_wrapper_shape(path: &std::path::Path, session_id: &str) {
    let values = jsonl_file_values(path);
    let header = values
        .first()
        .expect("Pi wrapper artifact should have a session header");
    assert_eq!(header.get("type").and_then(|v| v.as_str()), Some("session"));
    assert_eq!(header.get("id").and_then(|v| v.as_str()), Some(session_id));
    assert_eq!(header.get("version").and_then(|v| v.as_u64()), Some(3));
    assert!(header
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|cwd| !cwd.is_empty())
        .unwrap_or(false));

    let messages = values
        .iter()
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("message"))
        .collect::<Vec<_>>();
    assert_eq!(
        messages.len(),
        2,
        "Pi wrapper artifact should have exactly two message rows"
    );

    let user = messages[0];
    let assistant = messages[1];
    assert_eq!(
        user.pointer("/message/role").and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        assistant.pointer("/message/role").and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert_eq!(
        assistant.get("parentId").and_then(|v| v.as_str()),
        user.get("id").and_then(|v| v.as_str())
    );
    assert!(user
        .pointer("/message/content")
        .and_then(|v| v.as_str())
        .unwrap()
        .ends_with(CONTEXT_CONTINUATION_PROMPT));
    assert_eq!(
        assistant
            .pointer("/message/content/0/type")
            .and_then(|v| v.as_str()),
        Some("text")
    );
    assert_eq!(
        assistant
            .pointer("/message/content/0/text")
            .and_then(|v| v.as_str()),
        Some(CONTEXT_ACK)
    );
}

#[cfg(feature = "opencode")]
fn assert_gjc_native_wrapper_shape(path: &std::path::Path, session_id: &str) {
    assert_pi_native_wrapper_shape(path, session_id);
    let values = jsonl_file_values(path);
    let header = values
        .first()
        .expect("GJC wrapper artifact should have a session header");
    assert!(header.get("title").and_then(|v| v.as_str()).is_some());
}

#[cfg(feature = "opencode")]
fn assert_opencode_native_wrapper_shape(db_path: &std::path::Path, session_id: &str) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let (directory, slug, version, model): (String, String, String, String) = conn
        .query_row(
            "SELECT directory, slug, version, model FROM session WHERE id = ?1",
            rusqlite::params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert!(!directory.is_empty());
    assert!(!slug.is_empty());
    assert!(!version.is_empty());
    let model: Value = serde_json::from_str(&model).unwrap();
    assert!(model.get("id").and_then(|v| v.as_str()).is_some());
    assert!(model.get("providerID").and_then(|v| v.as_str()).is_some());

    let mut stmt = conn
        .prepare(
            "SELECT id, data FROM message
             WHERE session_id = ?1
             ORDER BY time_created, id",
        )
        .unwrap();
    let messages = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        messages.len(),
        2,
        "OpenCode wrapper artifact should have exactly two message rows"
    );
    assert!(messages[0].0.starts_with("msg_"));
    assert!(messages[1].0.starts_with("msg_"));
    let user_data: Value = serde_json::from_str(&messages[0].1).unwrap();
    let assistant_data: Value = serde_json::from_str(&messages[1].1).unwrap();
    assert_eq!(user_data.get("role").and_then(|v| v.as_str()), Some("user"));
    assert_eq!(
        assistant_data.get("role").and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert_eq!(
        assistant_data.get("parentID").and_then(|v| v.as_str()),
        Some(messages[0].0.as_str())
    );
    assert_eq!(
        assistant_data.get("finish").and_then(|v| v.as_str()),
        Some("stop")
    );
    assert!(assistant_data
        .get("tokens")
        .and_then(|v| v.as_object())
        .is_some());

    let user_parts = opencode_part_data(&conn, session_id, &messages[0].0);
    assert_eq!(user_parts.len(), 1);
    assert_eq!(
        user_parts[0].get("type").and_then(|v| v.as_str()),
        Some("text")
    );
    assert!(user_parts[0]
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap()
        .ends_with(CONTEXT_CONTINUATION_PROMPT));

    let assistant_parts = opencode_part_data(&conn, session_id, &messages[1].0);
    let assistant_part_types = assistant_parts
        .iter()
        .filter_map(|part| part.get("type").and_then(|v| v.as_str()))
        .collect::<Vec<_>>();
    assert!(assistant_part_types.contains(&"step-start"));
    assert!(assistant_part_types.contains(&"reasoning"));
    assert!(assistant_part_types.contains(&"text"));
    assert!(assistant_part_types.contains(&"step-finish"));
    assert!(
        assistant_parts.iter().any(|part| {
            part.get("type").and_then(|v| v.as_str()) == Some("text")
                && part.get("text").and_then(|v| v.as_str()) == Some(CONTEXT_ACK)
        }),
        "OpenCode assistant text part should contain ok"
    );

    let session_message_types = conn
        .prepare(
            "SELECT id, type FROM session_message
             WHERE session_id = ?1
             ORDER BY time_created, id",
        )
        .unwrap()
        .query_map(rusqlite::params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    if !session_message_types.is_empty() {
        assert_eq!(session_message_types.len(), 2);
        assert!(session_message_types
            .iter()
            .all(|(id, _)| id.starts_with("evt_")));
        assert_eq!(
            session_message_types
                .iter()
                .map(|(_, kind)| kind.as_str())
                .collect::<Vec<_>>(),
            vec!["agent-switched", "model-switched"]
        );
    }
}

#[cfg(feature = "opencode")]
fn opencode_part_data(
    conn: &rusqlite::Connection,
    session_id: &str,
    message_id: &str,
) -> Vec<Value> {
    let mut stmt = conn
        .prepare(
            "SELECT data FROM part
             WHERE session_id = ?1 AND message_id = ?2
             ORDER BY time_created, id",
        )
        .unwrap();
    stmt.query_map(rusqlite::params![session_id, message_id], |row| {
        let data: String = row.get(0)?;
        Ok(serde_json::from_str(&data).unwrap())
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

#[cfg(feature = "opencode")]
fn assert_context_wrapper(
    from: Provider,
    to: Provider,
    source: &UniversalSession,
    source_profile: &SemanticProfile,
    converted: &UniversalSession,
) {
    assert_eq!(
        converted.cwd, source.cwd,
        "{from:?} -> {to:?} should preserve cwd on the wrapper session"
    );
    let user_texts = texts_for_role(converted, Role::User);
    assert_eq!(
        user_texts.len(),
        1,
        "{from:?} -> {to:?} should expose one user context message"
    );
    let context = user_texts[0];
    assert!(
        context.ends_with(CONTEXT_CONTINUATION_PROMPT),
        "{from:?} -> {to:?} context should end with the continuation prompt"
    );
    assert!(
        context.contains(&format!(
            "=== {} ({}) ===",
            source.session_id,
            from.as_str()
        )),
        "{from:?} -> {to:?} context should include source session header"
    );
    for fragment in &source_profile.fragments {
        assert!(
            context.contains(fragment),
            "{from:?} -> {to:?} context should contain source fragment {fragment:?}"
        );
    }
    for (call_id, name, input) in &source_profile.tool_uses {
        assert!(
            context.contains(&format!("tool_use[{call_id}] {name}:")),
            "{from:?} -> {to:?} context should contain source tool call {call_id:?}"
        );
        let input = serde_json::to_string(input).unwrap();
        assert!(
            context.contains(&input),
            "{from:?} -> {to:?} context should contain source tool input {input:?}"
        );
    }
    for (call_id, output, is_error) in &source_profile.tool_results {
        let label = if *is_error {
            format!("tool_result[{call_id}] error:")
        } else {
            format!("tool_result[{call_id}]:")
        };
        assert!(
            context.contains(&label),
            "{from:?} -> {to:?} context should contain source tool result {call_id:?}"
        );
        assert!(
            context.contains(output),
            "{from:?} -> {to:?} context should contain source tool output {output:?}"
        );
    }
    for (mime, _) in &source_profile.images {
        assert!(
            context.contains(&format!("image: {mime} ")),
            "{from:?} -> {to:?} context should contain source image marker for {mime:?}"
        );
    }

    let assistant_texts = texts_for_role(converted, Role::Assistant);
    assert_eq!(
        assistant_texts,
        vec![CONTEXT_ACK],
        "{from:?} -> {to:?} should expose one assistant acknowledgement"
    );
}

#[cfg(feature = "opencode")]
#[test]
fn three_provider_pivot_preserves_text() {
    use cokacmux::providers::opencode;
    // codex → claude → opencode → codex
    let session = providers::codex::from_jsonl_str(codex_fixture(), &Default::default()).unwrap();
    let claude_str = providers::claude::to_jsonl_string(&session, &Default::default()).unwrap();
    let session2 = providers::claude::from_jsonl_str(&claude_str, &Default::default()).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("p.db");
    opencode::to_db_path(&session2, &db).unwrap();

    let session3 = opencode::from_db_path(&db, &session2.session_id).unwrap();
    let codex_str = providers::codex::to_jsonl_string(&session3, &Default::default()).unwrap();
    let session4 = providers::codex::from_jsonl_str(&codex_str, &Default::default()).unwrap();
    assert_eq!(
        user_assistant_texts(&session4),
        vec!["hello there", "hi back"]
    );
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_live_step_parts_to_codex_preserves_text() {
    use cokacmux::providers::opencode;
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("opencode.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    db::ensure_schema(&conn).unwrap();
    conn.execute(
        "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
         VALUES ('global', '/', 0, 0, '{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session
            (id, project_id, directory, title, agent, model,
             time_created, time_updated)
         VALUES
            ('ses_live_greeting', 'global', '/home/kst/123', 'Greeting',
             'build', '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"medium\"}',
             1779245070381, 1779245072486)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES
            ('msg_user', 'ses_live_greeting', 1779245070420, 1779245072527,
             '{\"role\":\"user\",\"time\":{\"created\":1779245070420},\"agent\":\"build\",\"model\":{\"providerID\":\"openai\",\"modelID\":\"gpt-5.5\",\"variant\":\"medium\"}}'),
            ('msg_assistant', 'ses_live_greeting', 1779245070438, 1779245072451,
             '{\"parentID\":\"msg_user\",\"role\":\"assistant\",\"mode\":\"build\",\"agent\":\"build\",\"variant\":\"medium\",\"path\":{\"cwd\":\"/home/kst/123\",\"root\":\"/\"},\"modelID\":\"gpt-5.5\",\"providerID\":\"openai\",\"time\":{\"created\":1779245070438,\"completed\":1779245072449},\"finish\":\"stop\"}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES
            ('prt_user_text', 'msg_user', 'ses_live_greeting', 1779245070425, 1779245070426,
             '{\"type\":\"text\",\"text\":\"hi\"}'),
            ('prt_step_start', 'msg_assistant', 'ses_live_greeting', 1779245071912, 1779245071914,
             '{\"type\":\"step-start\"}'),
            ('prt_assistant_text', 'msg_assistant', 'ses_live_greeting', 1779245072173, 1779245072412,
             '{\"type\":\"text\",\"text\":\"Hi. How can I help?\",\"metadata\":{\"openai\":{\"phase\":\"final_answer\"}}}'),
            ('prt_step_finish', 'msg_assistant', 'ses_live_greeting', 1779245072441, 1779245072442,
             '{\"reason\":\"stop\",\"type\":\"step-finish\"}')",
        [],
    )
    .unwrap();

    let session = opencode::from_db_path(&db_path, "ses_live_greeting").unwrap();
    assert_eq!(
        user_assistant_texts(&session),
        vec!["hi", "Hi. How can I help?"]
    );

    let codex_out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    assert!(codex_out.contains("\"text\":\"hi\""));
    assert!(codex_out.contains("\"text\":\"Hi. How can I help?\""));
    assert!(codex_out.contains(r#""type":"user_message""#));
    assert!(codex_out.contains(r#""type":"agent_message""#));

    let back = providers::codex::from_jsonl_str(&codex_out, &Default::default()).unwrap();
    assert_eq!(
        user_assistant_texts(&back),
        vec!["hi", "Hi. How can I help?"]
    );

    let claude_out = providers::claude::to_jsonl_string(&session, &Default::default()).unwrap();
    assert!(claude_out.contains(r#""content":"hi""#));
    assert!(claude_out.contains(r#""text":"Hi. How can I help?""#));
    assert!(
        !claude_out.contains("step-start") && !claude_out.contains("step-finish"),
        "OpenCode control parts are not valid visible Claude content"
    );
    let values = jsonl_values(&claude_out);
    let conversation = claude_conversation_lines(&values);
    assert_eq!(conversation.len(), 2);
    assert_eq!(
        conversation[1].get("parentUuid").and_then(|v| v.as_str()),
        conversation[0].get("uuid").and_then(|v| v.as_str())
    );
    let claude_back = providers::claude::from_jsonl_str(&claude_out, &Default::default()).unwrap();
    assert_eq!(
        user_assistant_texts(&claude_back),
        vec!["hi", "Hi. How can I help?"]
    );
}

#[cfg(feature = "opencode")]
#[test]
fn opencode_completed_tool_parts_to_claude_preserve_tool_results() {
    use cokacmux::providers::opencode;
    use cokacmux::providers::opencode::db;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("opencode.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    db::ensure_schema(&conn).unwrap();
    conn.execute(
        "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
         VALUES ('global', '/', 0, 0, '{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session
            (id, project_id, directory, title, agent, model, time_created, time_updated)
         VALUES
            ('ses_completed_tool', 'global', '/tmp', 'Completed Tool',
             'build', '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
             1000, 3000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES
            ('msg_user', 'ses_completed_tool', 1000, 1000,
             '{\"role\":\"user\",\"time\":{\"created\":1000}}'),
            ('msg_assistant', 'ses_completed_tool', 2000, 3000,
             '{\"role\":\"assistant\",\"time\":{\"created\":2000},\"finish\":\"tool_use\"}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES
            ('p_user', 'msg_user', 'ses_completed_tool', 1000, 1000,
             '{\"type\":\"text\",\"text\":\"run pwd\"}'),
            ('p_text', 'msg_assistant', 'ses_completed_tool', 2000, 2000,
             '{\"type\":\"text\",\"text\":\"I will run it.\"}'),
            ('p_tool', 'msg_assistant', 'ses_completed_tool', 2100, 2200,
             '{\"type\":\"tool\",\"tool\":\"shell\",\"callID\":\"call_done\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"pwd\"},\"output\":\"/tmp\"}}')",
        [],
    )
    .unwrap();

    let session = opencode::from_db_path(&db_path, "ses_completed_tool").unwrap();
    let claude_out = providers::claude::to_jsonl_string(&session, &Default::default()).unwrap();
    assert!(claude_out.contains(r#""type":"tool_result""#));
    let back = providers::claude::from_jsonl_str(&claude_out, &Default::default()).unwrap();
    let tool_results = back
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                call_id, output, ..
            } => Some((call_id.as_str(), normalized_output(output))),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_results, vec![("call_done", "/tmp".to_string())]);
}

#[cfg(feature = "opencode")]
#[test]
fn codex_to_opencode_preserves_equal_timestamp_tool_order() {
    use cokacmux::providers::opencode;

    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"same-time-tools","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"function_call","id":"fc_b","name":"shell","call_id":"call_b","arguments":"{\"command\":\"b\"}"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"function_call","id":"fc_a","name":"shell","call_id":"call_a","arguments":"{\"command\":\"a\"}"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_b","output":"b"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_a","output":"a"}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("opencode.db");
    opencode::to_db_path(&session, &db_path).unwrap();
    let back = opencode::from_db_path(&db_path, "same-time-tools").unwrap();
    let tool_uses = back
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let tool_results = back
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(tool_uses, vec!["call_b", "call_a"]);
    assert_eq!(tool_results, vec!["call_b", "call_a"]);
}

#[test]
fn codex_image_generation_end_is_portable_image_content() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-image","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"event_msg","payload":{"type":"image_generation_end","call_id":"ig_1","status":"completed","revised_prompt":"draw square","result":"iVBORw0KGgo=","saved_path":"/tmp/generated.png"}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let images = semantic_profile(&session).images;
    assert_eq!(
        images,
        vec![("image/png".to_string(), "base64:iVBORw0KGgo=".to_string())]
    );

    let claude_out = providers::claude::to_jsonl_string(&session, &Default::default()).unwrap();
    let claude_back = providers::claude::from_jsonl_str(&claude_out, &Default::default()).unwrap();
    assert_eq!(semantic_profile(&claude_back).images, images);

    #[cfg(feature = "opencode")]
    {
        use cokacmux::providers::opencode;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("image.db");
        opencode::to_db_path(&session, &db_path).unwrap();
        let opencode_back = opencode::from_db_path(&db_path, "codex-image").unwrap();
        assert_eq!(semantic_profile(&opencode_back).images, images);
    }
}

#[test]
fn codex_response_item_input_image_is_preserved_without_event_msg() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-input-image","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"look"},{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo="}]}}
"#;

    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();

    assert_eq!(
        semantic_profile(&session).images,
        vec![("image/png".to_string(), "base64:iVBORw0KGgo=".to_string())]
    );
}

#[test]
fn codex_response_item_image_generation_call_is_portable_image_content() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-image-generation-call","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"image_generation_call","id":"ig_1","status":"completed","revised_prompt":"draw square","result":"iVBORw0KGgo="}}
"#;

    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let images = semantic_profile(&session).images;

    assert_eq!(
        images,
        vec![("image/png".to_string(), "base64:iVBORw0KGgo=".to_string())]
    );

    let out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    assert!(jsonl_values(&out).iter().any(|value| {
        value.get("type").and_then(|value| value.as_str()) == Some("response_item")
            && value
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(|value| value.as_str())
                == Some("image_generation_call")
    }));
}

#[test]
fn codex_input_image_detail_roundtrips_to_codex() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-input-image-detail","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo=","detail":"original"}]}}
"#;

    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    let values = jsonl_values(&out);
    let input_image = values
        .iter()
        .filter_map(|value| value.get("payload"))
        .filter_map(|payload| payload.get("content"))
        .filter_map(|content| content.as_array())
        .flat_map(|content| content.iter())
        .find(|item| item.get("type").and_then(|value| value.as_str()) == Some("input_image"))
        .expect("missing Codex input_image");

    assert_eq!(input_image["detail"], "original");
}

#[test]
fn codex_user_image_write_emits_input_image_content() {
    let src = r#"{"type":"user","sessionId":"claude-image-src","cwd":"/tmp","timestamp":"2026-05-20T01:00:00.000Z","uuid":"u1","parentUuid":null,"message":{"role":"user","content":[{"type":"text","text":"look"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]}}
"#;
    let session = providers::claude::from_jsonl_str(src, &Default::default()).unwrap();

    let out = providers::codex::to_jsonl_string(&session, &Default::default()).unwrap();
    let values = jsonl_values(&out);
    let user_message = values
        .iter()
        .find(|value| {
            value.get("type").and_then(|value| value.as_str()) == Some("response_item")
                && value
                    .get("payload")
                    .and_then(|payload| payload.get("type"))
                    .and_then(|value| value.as_str())
                    == Some("message")
                && value
                    .get("payload")
                    .and_then(|payload| payload.get("role"))
                    .and_then(|value| value.as_str())
                    == Some("user")
        })
        .expect("missing Codex user response_item");
    let content = user_message["payload"]["content"].as_array().unwrap();

    assert!(content.iter().any(|item| {
        item.get("type").and_then(|value| value.as_str()) == Some("input_image")
            && item.get("image_url").and_then(|value| value.as_str())
                == Some("data:image/png;base64,iVBORw0KGgo=")
    }));
}

#[test]
fn codex_event_and_response_item_duplicate_images_are_deduped() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-duplicate-image","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:00.400Z","type":"event_msg","payload":{"type":"user_message","message":"","images":["data:image/png;base64,iVBORw0KGgo="],"local_images":[],"text_elements":[]}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo="}]}}
"#;

    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();

    assert_eq!(
        semantic_profile(&session).images,
        vec![("image/png".to_string(), "base64:iVBORw0KGgo=".to_string())]
    );
}

#[test]
fn codex_response_item_then_event_duplicate_images_are_deduped() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-duplicate-image-reverse","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:00.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo="}]}}
{"timestamp":"2026-05-20T01:00:00.600Z","type":"event_msg","payload":{"type":"user_message","message":"","images":["data:image/png;base64,iVBORw0KGgo="],"local_images":[],"text_elements":[]}}
"#;

    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();

    assert_eq!(
        semantic_profile(&session).images,
        vec![("image/png".to_string(), "base64:iVBORw0KGgo=".to_string())]
    );
}

#[test]
fn codex_event_image_url_keeps_image_mime_for_opencode_roundtrip() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-url-image","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:00.600Z","type":"event_msg","payload":{"type":"user_message","message":"","images":["https://example.test/screenshot.png"],"local_images":[],"text_elements":[]}}
"#;

    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    assert_eq!(
        semantic_profile(&session).images,
        vec![(
            "image/png".to_string(),
            "url:https://example.test/screenshot.png".to_string()
        )]
    );

    #[cfg(feature = "opencode")]
    {
        use cokacmux::providers::opencode;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("url-image.db");
        opencode::to_db_path(&session, &db_path).unwrap();
        let opencode_back = opencode::from_db_path(&db_path, "codex-url-image").unwrap();
        assert_eq!(
            semantic_profile(&opencode_back).images,
            semantic_profile(&session).images
        );
    }
}

#[test]
fn codex_raw_response_item_event_unwraps_semantic_item() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-raw-response-item","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:00.600Z","type":"event_msg","payload":{"type":"raw_response_item","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"raw item text"}],"phase":"final_answer"}}}
"#;

    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();

    assert!(
        semantic_profile(&session)
            .fragments
            .contains(&"raw item text".to_string()),
        "raw_response_item message was not exposed semantically"
    );
}

#[test]
fn codex_response_item_compaction_variants_are_flagged() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-response-compaction","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:00.600Z","type":"response_item","payload":{"type":"compaction","encrypted_content":"enc"}}
{"timestamp":"2026-05-20T01:00:00.700Z","type":"response_item","payload":{"type":"context_compaction","encrypted_content":"enc2"}}
{"timestamp":"2026-05-20T01:00:00.800Z","type":"response_item","payload":{"type":"compaction_trigger"}}
"#;

    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let flagged = session
        .messages
        .iter()
        .filter(|message| message.flags.is_compaction)
        .map(|message| message.provenance.source_event_type.as_str())
        .collect::<Vec<_>>();

    assert!(flagged.contains(&"codex:response_item.compaction"));
    assert!(flagged.contains(&"codex:response_item.context_compaction"));
    assert!(flagged.contains(&"codex:response_item.compaction_trigger"));
}

#[test]
fn codex_observed_rollout_events_are_represented() {
    let src = r#"{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{"id":"codex-events","cwd":"/tmp"}}
{"timestamp":"2026-05-20T01:00:00.001Z","type":"compacted","payload":{"message":"compact","replacement_history":[]}}
{"timestamp":"2026-05-20T01:00:00.002Z","type":"event_msg","payload":{"type":"user_message","message":"hello","images":[],"local_images":[],"text_elements":[]}}
{"timestamp":"2026-05-20T01:00:00.003Z","type":"event_msg","payload":{"type":"agent_message","message":"hi","phase":"final_answer","memory_citation":null}}
{"timestamp":"2026-05-20T01:00:00.004Z","type":"event_msg","payload":{"type":"token_count","info":null}}
{"timestamp":"2026-05-20T01:00:00.005Z","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}
{"timestamp":"2026-05-20T01:00:00.006Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}
{"timestamp":"2026-05-20T01:00:00.007Z","type":"event_msg","payload":{"type":"exec_command_end","call_id":"call_exec","status":"failed","exit_code":1}}
{"timestamp":"2026-05-20T01:00:00.008Z","type":"event_msg","payload":{"type":"patch_apply_end","call_id":"call_patch","success":false,"status":"failed"}}
{"timestamp":"2026-05-20T01:00:00.009Z","type":"event_msg","payload":{"type":"web_search_end","call_id":"ws1","query":"q"}}
{"timestamp":"2026-05-20T01:00:00.010Z","type":"event_msg","payload":{"type":"error","message":"boom","codex_error_info":"other"}}
{"timestamp":"2026-05-20T01:00:00.011Z","type":"event_msg","payload":{"type":"context_compacted"}}
{"timestamp":"2026-05-20T01:00:00.012Z","type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}
{"timestamp":"2026-05-20T01:00:00.013Z","type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"mcp1","result":{"Ok":{"content":[],"isError":false}}}}
{"timestamp":"2026-05-20T01:00:00.014Z","type":"event_msg","payload":{"type":"thread_rolled_back","num_turns":1}}
{"timestamp":"2026-05-20T01:00:00.015Z","type":"event_msg","payload":{"type":"image_generation_call","id":"ig1","status":"generating","result":"abc"}}
{"timestamp":"2026-05-20T01:00:00.016Z","type":"event_msg","payload":{"type":"image_generation_end","call_id":"ig1","status":"completed","result":"abc"}}
{"timestamp":"2026-05-20T01:00:00.017Z","type":"event_msg","payload":{"type":"entered_review_mode","target":{"type":"custom","instructions":"review"}}}
{"timestamp":"2026-05-20T01:00:00.018Z","type":"event_msg","payload":{"type":"exited_review_mode","review_output":{"findings":[]}}}
{"timestamp":"2026-05-20T01:00:00.019Z","type":"event_msg","payload":{"type":"view_image_tool_call","call_id":"view1","path":"/tmp/a.png"}}
{"timestamp":"2026-05-20T01:00:00.020Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"Plan","text":"plan"}}}
{"timestamp":"2026-05-20T01:00:00.021Z","type":"event_msg","payload":{"type":"thread_goal_updated","goal":{"status":"active"}}}
"#;
    let session = providers::codex::from_jsonl_str(src, &Default::default()).unwrap();
    let sources = session
        .messages
        .iter()
        .map(|m| m.provenance.source_event_type.as_str())
        .collect::<Vec<_>>();

    for expected in [
        "codex:compacted",
        "codex:event_msg.user_message",
        "codex:event_msg.agent_message",
        "codex:event_msg.token_count",
        "codex:event_msg.task_started",
        "codex:event_msg.task_complete",
        "codex:event_msg.exec_command_end",
        "codex:event_msg.patch_apply_end",
        "codex:event_msg.web_search_end",
        "codex:event_msg.error",
        "codex:event_msg.context_compacted",
        "codex:event_msg.turn_aborted",
        "codex:event_msg.mcp_tool_call_end",
        "codex:event_msg.thread_rolled_back",
        "codex:event_msg.image_generation_call",
        "codex:event_msg.image_generation_end",
        "codex:event_msg.entered_review_mode",
        "codex:event_msg.exited_review_mode",
        "codex:event_msg.view_image_tool_call",
        "codex:event_msg.item_completed",
        "codex:event_msg.thread_goal_updated",
    ] {
        assert!(
            sources.contains(&expected),
            "missing observed Codex event source {expected}"
        );
    }
    assert_eq!(
        semantic_profile(&session).images,
        vec![("image/png".to_string(), "base64:abc".to_string())]
    );
    assert!(
        session.messages.iter().any(|message| {
            message.flags.is_compaction
                && matches!(
                    message.provenance.source_event_type.as_str(),
                    "codex:compacted" | "codex:event_msg.context_compacted"
                )
        }),
        "Codex compaction-shaped records should carry compaction flags"
    );
}

#[test]
fn codex_source_declared_event_msg_variants_are_not_dropped() {
    let event_types = [
        "warning",
        "guardian_warning",
        "realtime_conversation_started",
        "realtime_conversation_realtime",
        "realtime_conversation_closed",
        "realtime_conversation_sdp",
        "model_reroute",
        "model_verification",
        "turn_started",
        "turn_complete",
        "agent_reasoning",
        "agent_reasoning_raw_content",
        "agent_reasoning_section_break",
        "session_configured",
        "mcp_startup_update",
        "mcp_startup_complete",
        "mcp_tool_call_begin",
        "web_search_begin",
        "image_generation_begin",
        "exec_command_begin",
        "exec_command_output_delta",
        "terminal_interaction",
        "exec_approval_request",
        "request_permissions",
        "request_user_input",
        "dynamic_tool_call_request",
        "dynamic_tool_call_response",
        "elicitation_request",
        "apply_patch_approval_request",
        "guardian_assessment",
        "deprecation_notice",
        "stream_error",
        "patch_apply_begin",
        "patch_apply_updated",
        "turn_diff",
        "realtime_conversation_list_voices_response",
        "plan_update",
        "shutdown_complete",
        "item_started",
        "hook_started",
        "hook_completed",
        "agent_message_content_delta",
        "plan_delta",
        "reasoning_content_delta",
        "reasoning_raw_content_delta",
        "collab_agent_spawn_begin",
        "collab_agent_spawn_end",
        "collab_agent_interaction_begin",
        "collab_agent_interaction_end",
        "collab_waiting_begin",
        "collab_waiting_end",
        "collab_close_begin",
        "collab_close_end",
        "collab_resume_begin",
        "collab_resume_end",
    ];
    let mut lines = vec![serde_json::json!({
        "timestamp": "2026-05-20T01:00:00.000Z",
        "type": "session_meta",
        "payload": {"id": "codex-source-events", "cwd": "/tmp"}
    })
    .to_string()];
    for (idx, event_type) in event_types.iter().enumerate() {
        lines.push(
            serde_json::json!({
                "timestamp": format!("2026-05-20T01:00:{:02}.000Z", idx + 1),
                "type": "event_msg",
                "payload": {"type": event_type, "marker": idx}
            })
            .to_string(),
        );
    }
    let src = lines.join("\n") + "\n";
    let session = providers::codex::from_jsonl_str(&src, &Default::default()).unwrap();
    let sources = session
        .messages
        .iter()
        .map(|message| message.provenance.source_event_type.as_str())
        .collect::<Vec<_>>();

    for event_type in event_types {
        let expected = format!("codex:event_msg.{event_type}");
        assert!(
            sources.contains(&expected.as_str()),
            "missing source-declared Codex EventMsg variant {expected}"
        );
    }
}
