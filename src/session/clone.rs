//! Clone a session — duplicate it with a fresh session id, optionally to a
//! different provider and/or a different cwd. Installs into the target
//! provider's real storage.

use std::{collections::HashMap, path::PathBuf};

#[cfg(not(feature = "opencode"))]
use crate::error::ConvertError;
use crate::error::Result;
use crate::providers;
use crate::providers::discovery::SessionInfo;
use crate::universal::{Provider, UniversalSession};

#[derive(Debug)]
pub struct CloneOpts {
    /// Override the target provider. Defaults to the source provider.
    pub to: Option<Provider>,
    /// Override cwd on the new session. Defaults to the source cwd.
    pub cwd: Option<String>,
    /// If true and target already has a session with the new id, replace it.
    pub overwrite: bool,
    /// Override the new id (otherwise a fresh UUID v7 is minted).
    pub new_id: Option<String>,
}

impl Default for CloneOpts {
    fn default() -> Self {
        Self {
            to: None,
            cwd: None,
            overwrite: false,
            new_id: None,
        }
    }
}

#[derive(Debug)]
pub struct CloneReport {
    pub source_provider: Provider,
    pub source_session_id: String,
    pub new_session_id: String,
    pub target_provider: Provider,
    pub artifact: ArtifactPath,
}

#[derive(Debug)]
pub enum ArtifactPath {
    File(PathBuf),
    OpenCodeDb {
        db_path: PathBuf,
        session_id: String,
    },
}

/// Clone the session described by `src` into the target provider's live
/// storage with a new session id. Returns the path/id of the new artifact.
pub fn clone_to_live(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    crate::debug::log(
        "clone_to_live_start",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": opts.to.map(|p| p.as_str()),
            "cwd_override": opts.cwd.as_deref(),
            "overwrite": opts.overwrite,
            "new_id_provided": opts.new_id.is_some(),
        }),
    );

    // Same-provider OpenCode clone uses SQL row-level copy so every column
    // and every internal JSON field is preserved verbatim — see
    // `providers::opencode::clone`. The Universal-pivot path rebuilds row
    // contents and can lose OpenCode-specific fields we don't model.
    #[cfg(feature = "opencode")]
    if src.provider == Provider::OpenCode
        && opts.to.map(|t| t == Provider::OpenCode).unwrap_or(true)
    {
        return clone_opencode_same_provider(src, opts);
    }

    let mut session = match super::load(src) {
        Ok(session) => {
            crate::debug::log(
                "clone_load_ok",
                serde_json::json!({
                    "source_provider": src.provider.as_str(),
                    "source_session_id": &src.session_id,
                    "messages": session.messages.len(),
                }),
            );
            session
        }
        Err(error) => {
            crate::debug::log(
                "clone_load_error",
                serde_json::json!({
                    "source_provider": src.provider.as_str(),
                    "source_session_id": &src.session_id,
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };
    apply_mutations(&mut session, src, opts);
    let target_provider = opts.to.unwrap_or(src.provider);
    let new_id = session.session_id.clone();
    let artifact = match install(&session, target_provider, opts) {
        Ok(artifact) => artifact,
        Err(error) => {
            crate::debug::log(
                "clone_install_error",
                serde_json::json!({
                    "source_provider": src.provider.as_str(),
                    "source_session_id": &src.session_id,
                    "target_provider": target_provider.as_str(),
                    "new_session_id": &new_id,
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };
    let validation =
        super::native_validate::ensure_clone_artifact_native(target_provider, &new_id, &artifact)?;
    crate::debug::log(
        "clone_to_live_ok",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": target_provider.as_str(),
            "new_session_id": &new_id,
            "artifact": format!("{:?}", &artifact),
            "native_validation_checks": validation.checks.len(),
        }),
    );
    Ok(CloneReport {
        source_provider: src.provider,
        source_session_id: src.session_id.clone(),
        new_session_id: new_id,
        target_provider,
        artifact,
    })
}

/// Same-provider OpenCode clone: SQL row-level copy. Preserves every column
/// of every row in the origin session, including `slug`, `share_url`,
/// `summary_*`, `permission`, `workspace_id`, and every internal field
/// inside `message.data`/`part.data`/`session_message.data`. Only db
/// identifier columns (`session.id`, `message.id`, `part.id`,
/// `session_message.id`) and `message.data.parentID` are remapped.
#[cfg(feature = "opencode")]
fn clone_opencode_same_provider(src: &SessionInfo, opts: &CloneOpts) -> Result<CloneReport> {
    let report = providers::opencode::clone::clone_session_rows(
        &src.source,
        &src.session_id,
        &providers::opencode::clone::OpenCodeRowCloneOpts {
            new_session_id: opts.new_id.clone(),
            cwd: opts.cwd.clone(),
            overwrite: opts.overwrite,
        },
    )?;
    let new_id = report.new_session_id.clone();
    let artifact = ArtifactPath::OpenCodeDb {
        db_path: report.db_path,
        session_id: new_id.clone(),
    };
    let validation = super::native_validate::ensure_clone_artifact_native(
        Provider::OpenCode,
        &new_id,
        &artifact,
    )?;
    crate::debug::log(
        "clone_to_live_ok",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": Provider::OpenCode.as_str(),
            "new_session_id": &new_id,
            "artifact": format!("{:?}", &artifact),
            "native_validation_checks": validation.checks.len(),
            "path": "opencode_row_copy",
            "messages_copied": report.messages_copied,
            "parts_copied": report.parts_copied,
            "session_messages_copied": report.session_messages_copied,
        }),
    );
    Ok(CloneReport {
        source_provider: Provider::OpenCode,
        source_session_id: src.session_id.clone(),
        new_session_id: new_id,
        target_provider: Provider::OpenCode,
        artifact,
    })
}

fn apply_mutations(session: &mut UniversalSession, src: &SessionInfo, opts: &CloneOpts) {
    let target_provider = opts.to.unwrap_or(src.provider);
    crate::debug::log(
        "clone_apply_mutations_start",
        serde_json::json!({
            "source_provider": src.provider.as_str(),
            "source_session_id": &src.session_id,
            "target_provider": target_provider.as_str(),
            "messages": session.messages.len(),
        }),
    );
    let new_id = opts
        .new_id
        .clone()
        .unwrap_or_else(|| mint_id_for(target_provider));
    let old_id = session.session_id.clone();
    session.session_id = new_id.clone();
    let old_cwd = session.cwd.clone();
    let new_cwd = opts.cwd.clone().unwrap_or_else(|| old_cwd.clone());
    session.cwd = new_cwd.clone();
    // Origin marker — preserve original source for traceability.
    session.origin.source_path = Some(format!("clone-of:{}", old_id));
    if target_provider == Provider::OpenCode {
        clear_opencode_source_session_row_runtime_fields(session);
    }

    let id_map: HashMap<String, String> = session
        .messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            (
                m.id.clone(),
                mint_message_id_for_message(target_provider, &new_id, i, m),
            )
        })
        .collect();

    // Rekey message ids so target storage doesn't collide with the source
    // session's primary keys. Claude also expects line ids to be UUID-like.
    for (i, m) in session.messages.iter_mut().enumerate() {
        let old_msg_id = m.id.clone();
        let new_msg_id = id_map
            .get(&old_msg_id)
            .cloned()
            .unwrap_or_else(|| mint_message_id_for(target_provider, &new_id, i));
        m.id = new_msg_id.clone();
        if let Some(parent_id) = m.parent_id.clone() {
            if let Some(new_parent_id) = id_map.get(&parent_id) {
                m.parent_id = Some(new_parent_id.clone());
            }
        }

        // Rewrite top-level identity fields in provenance.raw so the
        // re-emitted file (claude/codex use raw-replay for same-provider
        // round-trip) is self-consistent with the new session_id / cwd.
        rewrite_raw_identity(
            &mut m.provenance.raw,
            &old_id,
            &new_id,
            &old_cwd,
            &new_cwd,
            &old_msg_id,
            &new_msg_id,
            &id_map,
        );
    }

    if target_provider == Provider::Claude {
        let repaired = repair_claude_raw_parent_chain(session);
        let sanitized = sanitize_claude_raw_content_for_resume(session);
        crate::debug::log(
            "clone_claude_resume_repair",
            serde_json::json!({
                "session_id": &session.session_id,
                "parent_chain_rows": repaired,
                "sanitized_content_rows": sanitized,
            }),
        );
    }
    crate::debug::log(
        "clone_apply_mutations_ok",
        serde_json::json!({
            "old_session_id": old_id,
            "new_session_id": &session.session_id,
            "old_cwd": old_cwd,
            "new_cwd": &session.cwd,
            "messages": session.messages.len(),
            "id_map_entries": id_map.len(),
        }),
    );
}

fn clear_opencode_source_session_row_runtime_fields(session: &mut UniversalSession) {
    for key in [
        "opencode_parent_id",
        "opencode_slug",
        "opencode_share_url",
        "opencode_summary_additions",
        "opencode_summary_deletions",
        "opencode_summary_files",
        "opencode_summary_diffs",
        "opencode_revert",
        "opencode_permission",
        "opencode_time_compacting",
        "opencode_time_archived",
        "opencode_workspace_id",
        "opencode_path",
    ] {
        session.extras.remove(key);
    }
}

/// Rewrite session_id / cwd / per-line uuid in a raw record so a re-emitted
/// JSONL line is self-consistent. Provider-specific knowledge of which fields
/// carry these identifiers is hard-coded — when a provider adds new ones we
/// add them here.
fn rewrite_raw_identity(
    raw: &mut serde_json::Value,
    old_sid: &str,
    new_sid: &str,
    old_cwd: &str,
    new_cwd: &str,
    old_msg_id: &str,
    new_msg_id: &str,
    id_map: &HashMap<String, String>,
) {
    use serde_json::Value;
    let Value::Object(map) = raw else { return };

    // Claude: top-level `sessionId`, `cwd`, `uuid` (per-line id),
    // `parentUuid`, and snapshot fields that reference line UUIDs.
    if let Some(Value::String(s)) = map.get_mut("sessionId") {
        if s == old_sid {
            *s = new_sid.into();
        }
    }
    if let Some(Value::String(s)) = map.get_mut("cwd") {
        if s == old_cwd {
            *s = new_cwd.into();
        }
    }
    if let Some(Value::String(s)) = map.get_mut("uuid") {
        if s == old_msg_id {
            *s = new_msg_id.into();
        }
    }
    rewrite_id_field(map, "parentUuid", id_map);
    rewrite_id_field(map, "messageId", id_map);
    rewrite_id_field(map, "sourceToolAssistantUUID", id_map);
    rewrite_id_field(map, "leafUuid", id_map);
    if let Some(Value::Object(snapshot)) = map.get_mut("snapshot") {
        rewrite_id_field(snapshot, "messageId", id_map);
    }

    // Codex: payload.id == session id (in session_meta); payload.cwd == cwd
    // (in session_meta). Other lines don't carry these.
    if let Some(payload) = map.get_mut("payload") {
        if let Value::Object(pmap) = payload {
            if let Some(Value::String(s)) = pmap.get_mut("id") {
                if s == old_sid {
                    *s = new_sid.into();
                }
            }
            if let Some(Value::String(s)) = pmap.get_mut("cwd") {
                if s == old_cwd {
                    *s = new_cwd.into();
                }
            }
        }
    }

    // OpenCode provenance stores the message row under `message`; the actual
    // writer is synthetic today, but keeping parentID consistent avoids stale
    // raw data when adding future replay paths.
    if let Some(Value::Object(message)) = map.get_mut("message") {
        rewrite_id_field(message, "parentID", id_map);
    }

    if let Some(Value::Object(session_message)) = map.get_mut("session_message") {
        rewrite_id_field(session_message, "id", id_map);
        if let Some(Value::String(s)) = session_message.get_mut("session_id") {
            if s == old_sid {
                *s = new_sid.into();
            }
        }
    }
}

fn repair_claude_raw_parent_chain(session: &mut UniversalSession) -> usize {
    let mut previous_uuid: Option<String> = None;
    let mut leaf_uuid: Option<String> = None;
    let mut updated = 0usize;

    for message in &mut session.messages {
        let serde_json::Value::Object(map) = &mut message.provenance.raw else {
            continue;
        };
        let Some(kind) = map.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if !matches!(kind, "user" | "assistant") {
            continue;
        }
        let Some(uuid) = map.get("uuid").and_then(|v| v.as_str()).map(str::to_string) else {
            continue;
        };
        map.insert(
            "parentUuid".into(),
            previous_uuid
                .as_ref()
                .map(|parent| serde_json::Value::String(parent.clone()))
                .unwrap_or(serde_json::Value::Null),
        );
        updated = updated.saturating_add(1);
        previous_uuid = Some(uuid.clone());
        leaf_uuid = Some(uuid);
    }

    let Some(leaf_uuid) = leaf_uuid else {
        return updated;
    };
    for message in &mut session.messages {
        let serde_json::Value::Object(map) = &mut message.provenance.raw else {
            continue;
        };
        if map.get("type").and_then(|v| v.as_str()) == Some("last-prompt") {
            map.insert(
                "leafUuid".into(),
                serde_json::Value::String(leaf_uuid.clone()),
            );
            updated = updated.saturating_add(1);
        }
    }
    updated
}

fn sanitize_claude_raw_content_for_resume(session: &mut UniversalSession) -> usize {
    let mut sanitized = 0usize;
    for message in &mut session.messages {
        let serde_json::Value::Object(top) = &mut message.provenance.raw else {
            continue;
        };
        if !matches!(
            top.get("type").and_then(|v| v.as_str()),
            Some("user" | "assistant")
        ) {
            continue;
        }
        let Some(serde_json::Value::Object(inner)) = top.get_mut("message") else {
            continue;
        };
        let Some(serde_json::Value::Array(content)) = inner.get_mut("content") else {
            continue;
        };
        let before = content.len();
        content.retain(|block| {
            block
                .get("type")
                .and_then(|v| v.as_str())
                .map(is_claude_api_content_type)
                .unwrap_or(true)
        });
        if content.len() != before {
            sanitized = sanitized.saturating_add(1);
        }
    }
    sanitized
}

fn is_claude_api_content_type(kind: &str) -> bool {
    matches!(
        kind,
        "advisor_tool_result"
            | "bash_code_execution_tool_result"
            | "code_execution_tool_result"
            | "container_upload"
            | "document"
            | "image"
            | "redacted_thinking"
            | "search_result"
            | "server_tool_use"
            | "text"
            | "text_editor_code_execution_tool_result"
            | "thinking"
            | "tool_result"
            | "tool_search_tool_result"
            | "tool_use"
            | "web_fetch_tool_result"
            | "web_search_tool_result"
    )
}

fn rewrite_id_field(
    map: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    id_map: &HashMap<String, String>,
) {
    if let Some(serde_json::Value::String(s)) = map.get_mut(key) {
        if let Some(new_id) = id_map.get(s.as_str()) {
            *s = new_id.clone();
        }
    }
}

fn mint_id_for(target: Provider) -> String {
    match target {
        // Claude and Codex use UUID. Codex prefers v7 (time-ordered). Claude
        // accepts any UUID. Use v7 across the board.
        Provider::Claude | Provider::Codex => uuid::Uuid::now_v7().to_string(),
        // OpenCode session ids are `ses_` + native descending Identifier.
        Provider::OpenCode => crate::ids::opencode_session_id(),
    }
}

fn mint_message_id_for(target: Provider, session_id: &str, index: usize) -> String {
    match target {
        Provider::Claude => uuid::Uuid::now_v7().to_string(),
        Provider::Codex => format!("{}_m{:04}", session_id, index),
        Provider::OpenCode => crate::ids::opencode_message_id(),
    }
}

fn mint_message_id_for_message(
    target: Provider,
    session_id: &str,
    index: usize,
    message: &crate::universal::UMessage,
) -> String {
    if target == Provider::OpenCode
        && message
            .provenance
            .source_event_type
            .starts_with("opencode:session_message.")
    {
        crate::ids::opencode_event_id()
    } else {
        mint_message_id_for(target, session_id, index)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{json, Value};

    use super::*;
    use crate::universal::{
        ContentBlock, MessageFlags, Provenance, Role, UMessage, UniversalSession,
    };

    fn message(id: &str, parent_id: Option<&str>, role: Role, raw: Value) -> UMessage {
        UMessage {
            id: id.into(),
            parent_id: parent_id.map(String::from),
            index: 0,
            timestamp: None,
            role,
            model: None,
            usage: None,
            stop_reason: None,
            content: vec![ContentBlock::text("text")],
            flags: MessageFlags::default(),
            provenance: Provenance {
                source_event_type: format!("claude:{}", id),
                raw,
            },
            extras: Default::default(),
        }
    }

    #[test]
    fn claude_clone_rekeys_line_uuids_and_parent_refs() {
        let mut session = UniversalSession::new("old-session", Provider::Claude, "/old/cwd");
        session.messages.push(message(
            "snap",
            None,
            Role::System,
            json!({
                "type": "file-history-snapshot",
                "messageId": "u1",
                "snapshot": {"messageId": "u1"}
            }),
        ));
        session.messages.push(message(
            "u1",
            None,
            Role::User,
            json!({
                "type": "user",
                "sessionId": "old-session",
                "cwd": "/old/cwd",
                "uuid": "u1",
                "parentUuid": null,
                "message": {"role": "user", "content": "hi"}
            }),
        ));
        session.messages.push(message(
            "a1",
            Some("u1"),
            Role::Assistant,
            json!({
                "type": "assistant",
                "sessionId": "old-session",
                "cwd": "/old/cwd",
                "uuid": "a1",
                "parentUuid": "u1",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "yo"}]}
            }),
        ));
        let src = SessionInfo {
            provider: Provider::Claude,
            session_id: "old-session".into(),
            cwd: "/old/cwd".into(),
            source: PathBuf::new(),
            updated_at_epoch_s: 0,
            title: None,
        };

        apply_mutations(
            &mut session,
            &src,
            &CloneOpts {
                to: Some(Provider::Claude),
                cwd: Some("/new/cwd".into()),
                new_id: Some("019e463f-a792-7641-a9ab-a32c8bf6b8ff".into()),
                overwrite: false,
            },
        );

        let user_id = session.messages[1].id.clone();
        let assistant_id = session.messages[2].id.clone();
        assert!(uuid::Uuid::parse_str(&user_id).is_ok());
        assert!(uuid::Uuid::parse_str(&assistant_id).is_ok());
        assert_eq!(
            session.messages[2].parent_id.as_deref(),
            Some(user_id.as_str())
        );

        let snapshot = &session.messages[0].provenance.raw;
        assert_eq!(
            snapshot.get("messageId").and_then(|v| v.as_str()),
            Some(user_id.as_str())
        );
        assert_eq!(
            snapshot
                .pointer("/snapshot/messageId")
                .and_then(|v| v.as_str()),
            Some(user_id.as_str())
        );
        let user_raw = &session.messages[1].provenance.raw;
        assert_eq!(
            user_raw.get("uuid").and_then(|v| v.as_str()),
            Some(user_id.as_str())
        );
        assert_eq!(
            user_raw.get("sessionId").and_then(|v| v.as_str()),
            Some("019e463f-a792-7641-a9ab-a32c8bf6b8ff")
        );
        assert_eq!(
            user_raw.get("cwd").and_then(|v| v.as_str()),
            Some("/new/cwd")
        );
        assert_eq!(
            session.messages[2]
                .provenance
                .raw
                .get("parentUuid")
                .and_then(|v| v.as_str()),
            Some(user_id.as_str())
        );
    }

    #[test]
    fn claude_clone_repairs_stale_raw_parent_and_leaf_refs() {
        let mut session = UniversalSession::new("old-session", Provider::Claude, "/old/cwd");
        session.messages.push(message(
            "u1",
            None,
            Role::User,
            json!({
                "type": "user",
                "sessionId": "old-session",
                "cwd": "/old/cwd",
                "uuid": "u1",
                "parentUuid": null,
                "message": {"role": "user", "content": "hi"}
            }),
        ));
        session.messages.push(message(
            "a1",
            None,
            Role::Assistant,
            json!({
                "type": "assistant",
                "sessionId": "old-session",
                "cwd": "/old/cwd",
                "uuid": "a1",
                "parentUuid": "msg_stale_not_a_line_uuid",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "yo"}]}
            }),
        ));
        session.messages.push(message(
            "last",
            None,
            Role::System,
            json!({
                "type": "last-prompt",
                "leafUuid": "stale_leaf",
                "sessionId": "old-session"
            }),
        ));
        let src = SessionInfo {
            provider: Provider::Claude,
            session_id: "old-session".into(),
            cwd: "/old/cwd".into(),
            source: PathBuf::new(),
            updated_at_epoch_s: 0,
            title: None,
        };

        apply_mutations(
            &mut session,
            &src,
            &CloneOpts {
                to: Some(Provider::Claude),
                cwd: None,
                new_id: Some("019e463f-a792-7641-a9ab-a32c8bf6b8ff".into()),
                overwrite: false,
            },
        );

        let user_id = session.messages[0].id.clone();
        let assistant_id = session.messages[1].id.clone();
        assert_eq!(
            session.messages[1]
                .provenance
                .raw
                .get("parentUuid")
                .and_then(|v| v.as_str()),
            Some(user_id.as_str())
        );
        assert_eq!(
            session.messages[2]
                .provenance
                .raw
                .get("leafUuid")
                .and_then(|v| v.as_str()),
            Some(assistant_id.as_str())
        );
    }

    #[test]
    fn claude_clone_removes_non_api_content_blocks_from_raw_resume() {
        let mut session = UniversalSession::new("old-session", Provider::Claude, "/old/cwd");
        session.messages.push(message(
            "a1",
            None,
            Role::Assistant,
            json!({
                "type": "assistant",
                "sessionId": "old-session",
                "cwd": "/old/cwd",
                "uuid": "a1",
                "parentUuid": null,
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "step-start"},
                        {"type": "text", "text": "valid"},
                        {"type": "step-finish", "reason": "stop"}
                    ]
                }
            }),
        ));
        let src = SessionInfo {
            provider: Provider::Claude,
            session_id: "old-session".into(),
            cwd: "/old/cwd".into(),
            source: PathBuf::new(),
            updated_at_epoch_s: 0,
            title: None,
        };

        apply_mutations(
            &mut session,
            &src,
            &CloneOpts {
                to: Some(Provider::Claude),
                cwd: None,
                new_id: Some("019e463f-a792-7641-a9ab-a32c8bf6b8ff".into()),
                overwrite: false,
            },
        );

        let content = session.messages[0]
            .provenance
            .raw
            .pointer("/message/content")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(
            content[0].get("type").and_then(|v| v.as_str()),
            Some("text")
        );
    }

    #[test]
    fn opencode_clone_uses_native_message_ids_that_sort_before_new_runtime_ids() {
        let id = mint_message_id_for(Provider::OpenCode, "ses_test_abc", 0);
        assert!(id.starts_with("msg_"));
        assert_eq!(id.len(), 30);
        assert!(
            id[4..16].chars().all(|c| c.is_ascii_hexdigit()),
            "OpenCode native ids use a 12-hex timestamp/counter prefix"
        );
    }

    #[test]
    fn opencode_clone_rekeys_session_message_event_ids_without_reusing_source_rows() {
        let mut session = UniversalSession::new("ses_old", Provider::OpenCode, "/tmp");
        session.messages.push(UMessage {
            id: "msg_old".into(),
            parent_id: None,
            index: 0,
            timestamp: None,
            role: Role::User,
            model: None,
            usage: None,
            stop_reason: None,
            content: vec![ContentBlock::text("hi")],
            flags: MessageFlags::default(),
            provenance: Provenance {
                source_event_type: "opencode:message.user".into(),
                raw: json!({"message": {"role": "user"}}),
            },
            extras: Default::default(),
        });
        session.messages.push(UMessage {
            id: "evt_old".into(),
            parent_id: None,
            index: 1,
            timestamp: None,
            role: Role::System,
            model: None,
            usage: None,
            stop_reason: None,
            content: vec![ContentBlock::other(
                "opencode_session_message.agent-switched",
                json!({"agent": "build"}),
            )],
            flags: MessageFlags {
                is_meta: true,
                ..Default::default()
            },
            provenance: Provenance {
                source_event_type: "opencode:session_message.agent-switched".into(),
                raw: json!({
                    "session_message": {
                        "id": "evt_old",
                        "session_id": "ses_old",
                        "type": "agent-switched",
                        "time_created": 1,
                        "time_updated": 1,
                        "data": {"agent": "build", "time": {"created": 1}}
                    }
                }),
            },
            extras: Default::default(),
        });
        let src = SessionInfo {
            provider: Provider::OpenCode,
            session_id: "ses_old".into(),
            cwd: "/tmp".into(),
            source: PathBuf::new(),
            updated_at_epoch_s: 0,
            title: None,
        };

        apply_mutations(
            &mut session,
            &src,
            &CloneOpts {
                to: Some(Provider::OpenCode),
                cwd: None,
                new_id: Some("ses_new".into()),
                overwrite: false,
            },
        );

        assert!(session.messages[0].id.starts_with("msg_"));
        let event = &session.messages[1];
        assert!(event.id.starts_with("evt_"));
        assert_ne!(event.id, "evt_old");
        assert_eq!(
            event
                .provenance
                .raw
                .pointer("/session_message/id")
                .and_then(|v| v.as_str()),
            Some(event.id.as_str())
        );
        assert_eq!(
            event
                .provenance
                .raw
                .pointer("/session_message/session_id")
                .and_then(|v| v.as_str()),
            Some("ses_new")
        );
    }
}

fn install(session: &UniversalSession, target: Provider, opts: &CloneOpts) -> Result<ArtifactPath> {
    crate::debug::log(
        "clone_install_start",
        serde_json::json!({
            "target_provider": target.as_str(),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
            "overwrite": opts.overwrite,
        }),
    );
    match target {
        Provider::Claude => {
            let report = providers::claude::install::install_to_user_dir(
                session,
                &providers::claude::install::InstallOpts {
                    claude_home: None,
                    overwrite: opts.overwrite,
                },
            )?;
            let artifact = ArtifactPath::File(report.jsonl_path);
            crate::debug::log(
                "clone_install_ok",
                serde_json::json!({
                    "target_provider": target.as_str(),
                    "session_id": &session.session_id,
                    "artifact": format!("{:?}", &artifact),
                }),
            );
            Ok(artifact)
        }
        Provider::Codex => {
            let report = providers::codex::install::install_to_user_dir(
                session,
                &providers::codex::install::InstallOpts {
                    codex_home: None,
                    overwrite: opts.overwrite,
                    update_index: true,
                    state_5_path: None,
                },
            )?;
            let artifact = ArtifactPath::File(report.rollout_path);
            crate::debug::log(
                "clone_install_ok",
                serde_json::json!({
                    "target_provider": target.as_str(),
                    "session_id": &session.session_id,
                    "artifact": format!("{:?}", &artifact),
                }),
            );
            Ok(artifact)
        }
        Provider::OpenCode => {
            #[cfg(feature = "opencode")]
            {
                let report = providers::opencode::install::install_to_default_db(
                    session,
                    &providers::opencode::install::InstallOpts { db_path: None },
                )?;
                let artifact = ArtifactPath::OpenCodeDb {
                    db_path: report.db_path,
                    session_id: session.session_id.clone(),
                };
                crate::debug::log(
                    "clone_install_ok",
                    serde_json::json!({
                        "target_provider": target.as_str(),
                        "session_id": &session.session_id,
                        "artifact": format!("{:?}", &artifact),
                    }),
                );
                Ok(artifact)
            }
            #[cfg(not(feature = "opencode"))]
            {
                Err(ConvertError::Unsupported(
                    "opencode feature not enabled".into(),
                ))
            }
        }
    }
}
