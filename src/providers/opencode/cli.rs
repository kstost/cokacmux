//! OpenCode 2 session writes through the OpenCode CLI.
//!
//! OpenCode 2 keeps sessions in an event-sourced store and derives the
//! `session_v2` and `session_message` rows from its events, so another program
//! must not write those rows itself. Every write goes through
//! `opencode session import|export|delete`, run with a private server
//! (`--standalone`) on the data directory of the database cokacmux resolved,
//! so the operation lands in exactly that database.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::error::{ConvertError, Result};
use crate::providers::{output_tail, run_command_bounded, ProviderCommand};
use crate::universal::{ContentBlock, Role, UniversalSession};

/// How long one OpenCode CLI call may take. It starts a private server and
/// may migrate the database first, which is slow on a stalled disk.
const OPENCODE_CLI_TIMEOUT: Duration = Duration::from_secs(120);

/// Provider and model recorded on assistant messages cokacmux writes itself
/// (the acknowledgement of a context handoff). No model produced them, and
/// OpenCode keeps a message's model as a record only: a resumed session uses
/// the model the user selects.
const SYNTHETIC_MODEL_PROVIDER: &str = "cokacmux";
const SYNTHETIC_MODEL_ID: &str = "context-handoff";
const DEFAULT_AGENT: &str = "build";

/// `opencode` with its data directory pinned to the one holding `db_path`
/// (`<data home>/opencode/opencode.db`).
fn opencode_command(command: &ProviderCommand, db_path: &Path) -> Result<Command> {
    let data_home = db_path
        .parent()
        .filter(|dir| dir.file_name().is_some_and(|name| name == "opencode"))
        .and_then(Path::parent)
        .ok_or_else(|| {
            ConvertError::Other(format!(
                "cannot run opencode for {}: the database is not in an `opencode` data folder",
                db_path.display()
            ))
        })?;
    let mut process = command.command();
    process.env("XDG_DATA_HOME", data_home);
    Ok(process)
}

fn run_opencode(mut process: Command, action: &str) -> Result<std::process::Output> {
    // A private server starts for each call; keep its logs out of the output.
    process.args(["--standalone", "--log-level", "error"]);
    let output = run_command_bounded(process, OPENCODE_CLI_TIMEOUT)
        .map_err(|error| {
            ConvertError::Other(format!("could not run opencode to {action}: {error}"))
        })?
        .ok_or_else(|| {
            ConvertError::Other(format!(
                "opencode did not finish ({action}) within {}s",
                OPENCODE_CLI_TIMEOUT.as_secs()
            ))
        })?;
    if !output.status.success() {
        let detail = [
            output_tail(&output.stderr, 300),
            output_tail(&output.stdout, 300),
        ]
        .into_iter()
        .find(|text| !text.is_empty())
        .unwrap_or_default();
        return Err(ConvertError::Other(format!(
            "opencode failed to {action} ({}){}",
            output.status,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )));
    }
    Ok(output)
}

/// Imports `document` (the `opencode session export` format) as a new
/// session in `directory`. Fails rather than replace an existing session.
pub fn import_session(
    command: &ProviderCommand,
    db_path: &Path,
    document: &Value,
    directory: &str,
) -> Result<()> {
    let session_id = document
        .pointer("/info/id")
        .and_then(Value::as_str)
        .ok_or(ConvertError::MissingField("info.id"))?
        .to_string();
    let file = import_document_path();
    std::fs::write(&file, serde_json::to_vec(document)?)?;
    let result = (|| {
        let mut process = opencode_command(command, db_path)?;
        process
            .args(["session", "import"])
            .arg(&file)
            .args(["--directory", directory]);
        if Path::new(directory).is_dir() {
            process.current_dir(directory);
        }
        let output = run_opencode(process, "import the session")?;
        // The CLI reports an existing id on stdout and still exits 0.
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains(&format!("Imported session: {session_id}")) {
            Ok(())
        } else {
            Err(ConvertError::Other(format!(
                "opencode did not import session {session_id}: {}",
                output_tail(&output.stdout, 300)
            )))
        }
    })();
    let _ = std::fs::remove_file(&file);
    crate::debug::log(
        "provider_opencode_cli_import",
        json!({
            "db_path": db_path.display().to_string(),
            "session_id": &session_id,
            "directory": directory,
            "ok": result.is_ok(),
            "error": result.as_ref().err().map(|error| error.to_string()),
        }),
    );
    result
}

/// The session in the `opencode session import` format.
pub fn export_session(
    command: &ProviderCommand,
    db_path: &Path,
    session_id: &str,
) -> Result<Value> {
    let mut process = opencode_command(command, db_path)?;
    process.args(["session", "export", session_id]);
    let output = run_opencode(process, "export the session")?;
    serde_json::from_slice(&output.stdout).map_err(|error| {
        ConvertError::Parse(format!(
            "opencode export of {session_id} is not JSON: {error}"
        ))
    })
}

/// Deletes a session (OpenCode also deletes its child sessions).
pub fn delete_session(command: &ProviderCommand, db_path: &Path, session_id: &str) -> Result<()> {
    let mut process = opencode_command(command, db_path)?;
    process.args(["session", "delete", session_id]);
    run_opencode(process, "delete the session").map(|_| ())
}

fn import_document_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "cokacmux-opencode-import-{}-{}.json",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ))
}

/// Rewrites an exported session for import as a copy: a new session id and
/// new message ids (message ids are unique across sessions), with every
/// reference to an old message id inside the messages updated too.
pub fn retarget_export(export: &Value, new_session_id: &str) -> Result<Value> {
    let mut document = export.clone();
    let messages = document
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .ok_or(ConvertError::MissingField("messages"))?;
    let mut id_map = std::collections::HashMap::new();
    for message in messages.iter() {
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            id_map.insert(id.to_string(), crate::ids::opencode_message_id());
        }
    }
    for message in messages.iter_mut() {
        replace_strings(message, &id_map);
    }
    let info = document
        .get_mut("info")
        .and_then(Value::as_object_mut)
        .ok_or(ConvertError::MissingField("info"))?;
    info.insert("id".into(), Value::String(new_session_id.to_string()));
    // A copy is not the source's fork or child.
    info.remove("parentID");
    info.remove("share");
    Ok(document)
}

fn replace_strings(value: &mut Value, map: &std::collections::HashMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(new) = map.get(text.as_str()) {
                *text = new.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| replace_strings(item, map)),
        Value::Object(object) => object
            .values_mut()
            .for_each(|item| replace_strings(item, map)),
        _ => {}
    }
}

/// An import document for a session cokacmux composed itself (the two-message
/// context handoff of a cross-provider clone). Only text turns can be
/// written this way; anything else is refused rather than dropped.
pub fn import_document_from_session(session: &UniversalSession) -> Result<Value> {
    let created = session
        .created_at
        .map(|time| time.timestamp_millis())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let updated = session
        .updated_at
        .map(|time| time.timestamp_millis())
        .unwrap_or(created)
        .max(created);
    let agent = session
        .extras
        .get("opencode_agent")
        .and_then(Value::as_str)
        .filter(|agent| !agent.trim().is_empty())
        .unwrap_or(DEFAULT_AGENT);
    let mut messages = Vec::new();
    for (index, message) in session.messages.iter().enumerate() {
        if message.flags.is_meta {
            continue;
        }
        let text = message
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text, .. } => Ok(text.as_str()),
                _ => Err(ConvertError::Unsupported(
                    "only text messages can be imported into OpenCode 2".into(),
                )),
            })
            .collect::<Result<Vec<_>>>()?
            .join("\n\n");
        let time = message
            .timestamp
            .map(|time| time.timestamp_millis())
            .unwrap_or(created + index as i64);
        let id = crate::ids::opencode_message_id();
        let entry = match message.role {
            Role::User => json!({
                "id": id,
                "type": "user",
                "time": { "created": time },
                "text": text,
                "files": [],
            }),
            Role::Assistant => json!({
                "id": id,
                "type": "assistant",
                "time": { "created": time, "completed": time },
                "agent": agent,
                "model": {
                    "id": SYNTHETIC_MODEL_ID,
                    "providerID": SYNTHETIC_MODEL_PROVIDER,
                },
                "content": [{ "type": "text", "text": text }],
            }),
            other => {
                return Err(ConvertError::Unsupported(format!(
                    "{other:?} messages cannot be imported into OpenCode 2"
                )))
            }
        };
        messages.push(entry);
    }
    let mut info = Map::new();
    info.insert("id".into(), json!(session.session_id));
    // Required by the import format; OpenCode assigns the project of
    // `--directory` on import.
    info.insert("projectID".into(), json!("global"));
    info.insert(
        "title".into(),
        json!(session.title.clone().unwrap_or_default()),
    );
    info.insert("cost".into(), json!(0));
    info.insert(
        "tokens".into(),
        json!({ "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } }),
    );
    info.insert(
        "time".into(),
        json!({ "created": created, "updated": updated }),
    );
    info.insert("location".into(), json!({ "directory": session.cwd }));
    Ok(json!({ "info": Value::Object(info), "messages": messages }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::universal::{MessageFlags, Provenance, Provider, UMessage};

    fn message(role: Role, block: ContentBlock) -> UMessage {
        UMessage {
            id: "m".into(),
            parent_id: None,
            index: 0,
            timestamp: None,
            role,
            model: None,
            usage: None,
            stop_reason: None,
            content: vec![block],
            flags: MessageFlags::default(),
            provenance: Provenance {
                source_event_type: "test".into(),
                raw: Value::Null,
            },
            extras: Default::default(),
        }
    }

    /// The parts of an OpenCode 2 database a reader needs: `session_v2` and
    /// the `session_message` transcript, without the legacy tables.
    pub(crate) fn create_event_sourced_db(path: &Path) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session_v2 (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL, workspace_id TEXT,
                parent_id TEXT, slug TEXT NOT NULL, directory TEXT NOT NULL, path TEXT,
                title TEXT, version TEXT NOT NULL, share_url TEXT,
                summary_additions INTEGER, summary_deletions INTEGER,
                summary_files INTEGER, summary_diffs TEXT,
                cost REAL NOT NULL DEFAULT 0, tokens_input INTEGER NOT NULL DEFAULT 0,
                tokens_output INTEGER NOT NULL DEFAULT 0,
                tokens_reasoning INTEGER NOT NULL DEFAULT 0,
                tokens_cache_read INTEGER NOT NULL DEFAULT 0,
                tokens_cache_write INTEGER NOT NULL DEFAULT 0,
                revert TEXT, permission TEXT, agent TEXT, model TEXT,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                time_compacting INTEGER, time_archived INTEGER
             );
             CREATE TABLE session_message (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                seq INTEGER NOT NULL, time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn event_sourced_database_reads_from_session_v2_and_session_message() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");
        let conn = create_event_sourced_db(&db_path);
        conn.execute(
            "INSERT INTO session_v2 (id, project_id, slug, directory, title, version,
                                     time_created, time_updated)
             VALUES ('ses_v2', 'proj', 'quiet-star', '/repo', NULL, '2.0.24', 1000, 2000)",
            [],
        )
        .unwrap();
        for (seq, kind, data) in [
            (
                1,
                "user",
                r#"{"time":{"created":1001},"text":"hello","files":[]}"#,
            ),
            (
                2,
                "assistant",
                r#"{"time":{"created":1002},"agent":"build","model":{"id":"m","providerID":"p"},"content":[{"type":"text","text":"hi"}]}"#,
            ),
            (
                3,
                "idle",
                r#"{"time":{"created":1003},"outcome":"succeeded"}"#,
            ),
        ] {
            conn.execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
                 VALUES (?1, 'ses_v2', ?2, ?3, 1000, 1000, ?4)",
                rusqlite::params![format!("msg_{seq}"), kind, seq, data],
            )
            .unwrap();
        }
        assert!(crate::providers::opencode::db::is_event_sourced(&conn).unwrap());

        let session = crate::providers::opencode::from_db_connection(&conn, "ses_v2").unwrap();

        assert_eq!(session.cwd, "/repo");
        assert_eq!(
            session.title, None,
            "a NULL title must not become a made-up one"
        );
        let visible: Vec<_> = session
            .messages
            .iter()
            .filter(|message| !message.flags.is_meta)
            .map(|message| message.role)
            .collect();
        assert!(
            visible.starts_with(&[Role::User, Role::Assistant]),
            "{visible:?}"
        );
    }

    #[test]
    fn retargeted_export_gets_new_ids_and_keeps_internal_references() {
        let export = json!({
            "info": {"id": "ses_old", "parentID": "ses_parent", "title": "t"},
            "messages": [
                {"id": "msg_a", "type": "user", "text": "hi", "files": []},
                {"id": "msg_b", "type": "compaction", "recent": ["msg_a"]}
            ]
        });

        let copy = retarget_export(&export, "ses_new").unwrap();

        assert_eq!(copy["info"]["id"], "ses_new");
        assert!(copy["info"].get("parentID").is_none());
        let first = copy["messages"][0]["id"].as_str().unwrap();
        let second = copy["messages"][1]["id"].as_str().unwrap();
        assert!(first.starts_with("msg_") && first != "msg_a");
        assert!(second.starts_with("msg_") && second != "msg_b");
        assert_eq!(copy["messages"][1]["recent"][0], first);
        assert_eq!(copy["messages"][0]["text"], "hi");
        assert_eq!(
            export["info"]["id"], "ses_old",
            "the source export is unchanged"
        );
    }

    #[test]
    fn import_document_carries_the_fields_opencode_requires() {
        let mut session = UniversalSession::new("ses_wrapper", Provider::OpenCode, "/repo");
        session.title = Some("Context".into());
        for (role, text) in [(Role::User, "context"), (Role::Assistant, "ok")] {
            session
                .messages
                .push(message(role, ContentBlock::text(text)));
        }

        let document = import_document_from_session(&session).unwrap();

        for pointer in [
            "/info/id",
            "/info/projectID",
            "/info/title",
            "/info/cost",
            "/info/tokens/cache/read",
            "/info/time/created",
            "/info/time/updated",
            "/info/location/directory",
        ] {
            assert!(document.pointer(pointer).is_some(), "{pointer}");
        }
        let messages = document["messages"].as_array().unwrap();
        assert_eq!(messages[0]["type"], "user");
        assert_eq!(messages[0]["text"], "context");
        assert_eq!(messages[0]["files"], json!([]));
        assert_eq!(messages[1]["type"], "assistant");
        assert_eq!(messages[1]["content"][0]["text"], "ok");
        assert_eq!(messages[1]["model"]["providerID"], SYNTHETIC_MODEL_PROVIDER);
        assert!(messages[1]["agent"].is_string());
    }

    #[test]
    fn import_document_refuses_content_it_cannot_write() {
        let mut session = UniversalSession::new("ses_wrapper", Provider::OpenCode, "/repo");
        session.messages.push(message(
            Role::Assistant,
            ContentBlock::other("tool", json!({})),
        ));

        assert!(import_document_from_session(&session).is_err());
    }
}
