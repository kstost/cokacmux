//! Same-provider OpenCode → OpenCode clone via SQL row-level copy.
//!
//! The Universal-pivot path rebuilds `message.data` / `part.data` /
//! `session_message.data` from typed `ContentBlock`s, which can lose
//! provider-specific fields that we don't model. For same-provider clone
//! we want **exact preservation** of every column and every internal JSON
//! field except the identifier fields that MUST change (primary keys and
//! the one `message.data.parentID` reference that points back at a
//! message id).
//!
//! This module reads the origin rows directly via SQL and INSERTs new
//! rows with freshly minted `ses_…` / `msg_…` / `prt_…` / `evt_…` ids.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use crate::debug;
use crate::error::{ConvertError, Result};
use crate::ids;

#[derive(Debug, Clone, Default)]
pub struct OpenCodeRowCloneOpts {
    /// Override the destination session id (otherwise `ids::opencode_session_id()`).
    pub new_session_id: Option<String>,
    /// Override the new session's cwd. When set, the `session.directory`
    /// column is updated; `path` is also rebuilt from cwd. JSON fields
    /// inside `message.data` like `path.cwd` are also rewritten when the
    /// origin string matches the old cwd exactly.
    pub cwd: Option<String>,
    /// If true and a row with the new session id already exists, replace it
    /// (deletes existing row sets before INSERT).
    pub overwrite: bool,
}

#[derive(Debug)]
pub struct OpenCodeRowCloneReport {
    pub db_path: PathBuf,
    pub new_session_id: String,
    pub messages_copied: usize,
    pub parts_copied: usize,
    pub session_messages_copied: usize,
}

/// Clone an OpenCode session by SQL row-level copy with id remapping.
/// Preserves every column and every internal JSON field except identifier
/// fields and (optionally) cwd-derived paths.
pub fn clone_session_rows(
    db_path: &Path,
    src_session_id: &str,
    opts: &OpenCodeRowCloneOpts,
) -> Result<OpenCodeRowCloneReport> {
    debug::log(
        "opencode_row_clone_start",
        serde_json::json!({
            "db_path": db_path.display().to_string(),
            "src_session_id": src_session_id,
            "overwrite": opts.overwrite,
            "cwd_override": opts.cwd.as_deref(),
            "new_session_id_provided": opts.new_session_id.is_some(),
        }),
    );

    let mut conn = super::db::open_readwrite(db_path)?;
    // Lock probe — bail with a clear message if opencode holds an
    // exclusive lock. Use a separate scope so the probe txn is dropped
    // before we open our real one.
    {
        let probe = super::db::open_readwrite(db_path)?;
        if let Err(e) = probe.execute("BEGIN IMMEDIATE; ROLLBACK;", []) {
            return Err(ConvertError::Other(format!(
                "could not acquire write lock on {} (is opencode running?): {}",
                db_path.display(),
                e
            )));
        }
    }
    super::db::ensure_schema(&conn)?;

    let new_session_id = opts
        .new_session_id
        .clone()
        .unwrap_or_else(ids::opencode_session_id);

    let tx = conn.transaction()?;

    // 1. Read origin session row (28 columns)
    let session_row = read_session_row(&tx, src_session_id)?;

    // 2. Read message rows + build id_map(msg_old → msg_new)
    let message_rows = read_message_rows(&tx, src_session_id)?;
    let msg_id_map: HashMap<String, String> = message_rows
        .iter()
        .map(|r| (r.id.clone(), ids::opencode_message_id()))
        .collect();

    // 3. Read part rows + build id_map(prt_old → prt_new)
    let part_rows = read_part_rows(&tx, src_session_id)?;
    let prt_id_map: HashMap<String, String> = part_rows
        .iter()
        .map(|r| (r.id.clone(), ids::opencode_part_id()))
        .collect();

    // 4. Read session_message rows + build id_map(evt_old → evt_new). CRITICAL:
    // `session_message.id` is globally primary-keyed; reusing origin's id would
    // overwrite the origin row on INSERT OR REPLACE. Always re-mint.
    let session_message_rows = read_session_message_rows(&tx, src_session_id)?;
    let evt_id_map: HashMap<String, String> = session_message_rows
        .iter()
        .map(|r| (r.id.clone(), ids::opencode_event_id()))
        .collect();

    if opts.overwrite {
        tx.execute(
            "DELETE FROM part WHERE session_id = ?1",
            rusqlite::params![new_session_id],
        )?;
        tx.execute(
            "DELETE FROM message WHERE session_id = ?1",
            rusqlite::params![new_session_id],
        )?;
        tx.execute(
            "DELETE FROM session_message WHERE session_id = ?1",
            rusqlite::params![new_session_id],
        )?;
        tx.execute(
            "DELETE FROM session WHERE id = ?1",
            rusqlite::params![new_session_id],
        )?;
    }

    // 5. INSERT new session row — every column copied verbatim except `id`
    // and optionally `directory`/`path` when cwd is overridden.
    let new_directory = opts
        .cwd
        .clone()
        .unwrap_or_else(|| session_row.directory.clone());
    let new_path = opts.cwd.as_deref().map(opencode_session_path_for_cwd);
    let final_path = new_path.or(session_row.path.clone());
    tx.execute(
        "INSERT INTO session
            (id, project_id, parent_id, slug, directory, title, version, share_url,
             summary_additions, summary_deletions, summary_files, summary_diffs,
             revert, permission, time_created, time_updated, time_compacting,
             time_archived, workspace_id, path, agent, model, cost,
             tokens_input, tokens_output, tokens_reasoning,
             tokens_cache_read, tokens_cache_write)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
                 ?27, ?28)",
        rusqlite::params![
            new_session_id,
            session_row.project_id,
            session_row.parent_id,
            session_row.slug,
            new_directory,
            session_row.title,
            session_row.version,
            session_row.share_url,
            session_row.summary_additions,
            session_row.summary_deletions,
            session_row.summary_files,
            session_row.summary_diffs,
            session_row.revert,
            session_row.permission,
            session_row.time_created,
            session_row.time_updated,
            session_row.time_compacting,
            session_row.time_archived,
            session_row.workspace_id,
            final_path,
            session_row.agent,
            session_row.model,
            session_row.cost,
            session_row.tokens_input,
            session_row.tokens_output,
            session_row.tokens_reasoning,
            session_row.tokens_cache_read,
            session_row.tokens_cache_write,
        ],
    )?;

    // 6. INSERT message rows — data JSON verbatim except internal `parentID`
    // (which references another message id and must be remapped) and any
    // cwd-derived `path.cwd` field when overridden.
    let mut messages_copied = 0usize;
    for row in &message_rows {
        let new_id = msg_id_map.get(&row.id).expect("msg id_map complete");
        let rewritten_data = rewrite_message_data_json(
            &row.data,
            &msg_id_map,
            &session_row.directory,
            opts.cwd.as_deref(),
        );
        tx.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                new_id,
                new_session_id,
                row.time_created,
                row.time_updated,
                rewritten_data,
            ],
        )?;
        messages_copied = messages_copied.saturating_add(1);
    }

    // 7. INSERT part rows — data verbatim. `callID` (tool call id) is NOT a
    // db identifier so it stays stable. Only the part's `id` and its
    // `message_id` reference are remapped.
    let mut parts_copied = 0usize;
    for row in &part_rows {
        let new_id = prt_id_map.get(&row.id).expect("prt id_map complete");
        let new_msg_id = msg_id_map.get(&row.message_id).ok_or_else(|| {
            ConvertError::Other(format!(
                "part {} references unknown message_id {}",
                row.id, row.message_id
            ))
        })?;
        tx.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                new_id,
                new_msg_id,
                new_session_id,
                row.time_created,
                row.time_updated,
                row.data,
            ],
        )?;
        parts_copied = parts_copied.saturating_add(1);
    }

    // 8. INSERT session_message rows — data verbatim.
    let mut session_messages_copied = 0usize;
    for row in &session_message_rows {
        let new_id = evt_id_map.get(&row.id).expect("evt id_map complete");
        tx.execute(
            "INSERT INTO session_message (id, session_id, type, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                new_id,
                new_session_id,
                row.type_tag,
                row.time_created,
                row.time_updated,
                row.data,
            ],
        )?;
        session_messages_copied = session_messages_copied.saturating_add(1);
    }

    tx.commit()?;

    debug::log(
        "opencode_row_clone_ok",
        serde_json::json!({
            "db_path": db_path.display().to_string(),
            "src_session_id": src_session_id,
            "new_session_id": &new_session_id,
            "messages": messages_copied,
            "parts": parts_copied,
            "session_messages": session_messages_copied,
        }),
    );

    Ok(OpenCodeRowCloneReport {
        db_path: db_path.to_path_buf(),
        new_session_id,
        messages_copied,
        parts_copied,
        session_messages_copied,
    })
}

struct SessionRow {
    project_id: String,
    parent_id: Option<String>,
    slug: String,
    directory: String,
    title: String,
    version: String,
    share_url: Option<String>,
    summary_additions: Option<i64>,
    summary_deletions: Option<i64>,
    summary_files: Option<i64>,
    summary_diffs: Option<String>,
    revert: Option<String>,
    permission: Option<String>,
    time_created: i64,
    time_updated: i64,
    time_compacting: Option<i64>,
    time_archived: Option<i64>,
    workspace_id: Option<String>,
    path: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    cost: f64,
    tokens_input: i64,
    tokens_output: i64,
    tokens_reasoning: i64,
    tokens_cache_read: i64,
    tokens_cache_write: i64,
}

fn read_session_row(conn: &Connection, session_id: &str) -> Result<SessionRow> {
    conn.query_row(
        "SELECT project_id, parent_id, slug, directory, title, version, share_url,
                summary_additions, summary_deletions, summary_files, summary_diffs,
                revert, permission, time_created, time_updated, time_compacting,
                time_archived, workspace_id, path, agent, model, cost,
                tokens_input, tokens_output, tokens_reasoning,
                tokens_cache_read, tokens_cache_write
         FROM session WHERE id = ?1",
        rusqlite::params![session_id],
        |row| {
            Ok(SessionRow {
                project_id: row.get(0)?,
                parent_id: row.get(1)?,
                slug: row.get(2)?,
                directory: row.get(3)?,
                title: row.get(4)?,
                version: row.get(5)?,
                share_url: row.get(6)?,
                summary_additions: row.get(7)?,
                summary_deletions: row.get(8)?,
                summary_files: row.get(9)?,
                summary_diffs: row.get(10)?,
                revert: row.get(11)?,
                permission: row.get(12)?,
                time_created: row.get(13)?,
                time_updated: row.get(14)?,
                time_compacting: row.get(15)?,
                time_archived: row.get(16)?,
                workspace_id: row.get(17)?,
                path: row.get(18)?,
                agent: row.get(19)?,
                model: row.get(20)?,
                cost: row.get(21)?,
                tokens_input: row.get(22)?,
                tokens_output: row.get(23)?,
                tokens_reasoning: row.get(24)?,
                tokens_cache_read: row.get(25)?,
                tokens_cache_write: row.get(26)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            ConvertError::Parse(format!("no session row for id {}", session_id))
        }
        other => other.into(),
    })
}

struct MessageRow {
    id: String,
    time_created: i64,
    time_updated: i64,
    data: String,
}

fn read_message_rows(conn: &Connection, session_id: &str) -> Result<Vec<MessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, time_created, time_updated, data
         FROM message WHERE session_id = ?1
         ORDER BY time_created, id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                time_created: row.get(1)?,
                time_updated: row.get(2)?,
                data: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

struct PartRow {
    id: String,
    message_id: String,
    time_created: i64,
    time_updated: i64,
    data: String,
}

fn read_part_rows(conn: &Connection, session_id: &str) -> Result<Vec<PartRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, message_id, time_created, time_updated, data
         FROM part WHERE session_id = ?1
         ORDER BY time_created, id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok(PartRow {
                id: row.get(0)?,
                message_id: row.get(1)?,
                time_created: row.get(2)?,
                time_updated: row.get(3)?,
                data: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

struct SessionMessageRow {
    id: String,
    type_tag: String,
    time_created: i64,
    time_updated: i64,
    data: String,
}

fn read_session_message_rows(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<SessionMessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, type, time_created, time_updated, data
         FROM session_message WHERE session_id = ?1
         ORDER BY time_created, id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok(SessionMessageRow {
                id: row.get(0)?,
                type_tag: row.get(1)?,
                time_created: row.get(2)?,
                time_updated: row.get(3)?,
                data: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Rewrite `parentID` (message id reference) and optionally `path.cwd`
/// inside a `message.data` JSON. Everything else is preserved verbatim.
fn rewrite_message_data_json(
    data_json: &str,
    msg_id_map: &HashMap<String, String>,
    old_cwd: &str,
    new_cwd: Option<&str>,
) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(data_json) else {
        return data_json.to_string();
    };
    if let Value::Object(map) = &mut value {
        if let Some(Value::String(s)) = map.get_mut("parentID") {
            if let Some(new_id) = msg_id_map.get(s) {
                *s = new_id.clone();
            }
        }
        if let Some(new) = new_cwd {
            if let Some(Value::Object(path_obj)) = map.get_mut("path") {
                if let Some(Value::String(cwd_field)) = path_obj.get_mut("cwd") {
                    if cwd_field == old_cwd {
                        *cwd_field = new.to_string();
                    }
                }
            }
        }
    }
    serde_json::to_string(&value).unwrap_or_else(|_| data_json.to_string())
}

fn opencode_session_path_for_cwd(cwd: &str) -> String {
    // Mirror the synthesis used by writer's `opencode_session_path`.
    let safe: String = cwd
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    safe
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn seed_origin(conn: &Connection) {
        super::super::db::ensure_schema(conn).unwrap();
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
             VALUES ('global', '/', 0, 0, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session
                (id, project_id, directory, title, agent, model, slug, version, path,
                 share_url, summary_additions, summary_files, revert, permission,
                 time_created, time_updated, workspace_id, parent_id,
                 cost, tokens_input, tokens_output, tokens_reasoning,
                 tokens_cache_read, tokens_cache_write)
             VALUES ('ses_origin', 'global', '/tmp/x', 'My Title', 'build',
                     '{\"id\":\"gpt-5.5\",\"providerID\":\"openai\",\"variant\":\"default\"}',
                     'user-set-slug', '1.15.7', '-tmp-x', 'https://share/abc',
                     5, 2, 'soft', 'allow',
                     1000, 2000, 'ws-1', 'ses_parent',
                     1.23, 100, 50, 10, 5, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_user', 'ses_origin', 1000, 1000,
                     '{\"role\":\"user\",\"time\":{\"created\":1000},\"agent\":\"build\",\"opencode_internal_x\":\"keep-me\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('msg_asst', 'ses_origin', 1500, 1500,
                     '{\"role\":\"assistant\",\"parentID\":\"msg_user\",\"path\":{\"cwd\":\"/tmp/x\",\"root\":\"/\"},\"opencode_internal_y\":42}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES ('prt_text', 'msg_asst', 'ses_origin', 1500, 1500,
                     '{\"type\":\"text\",\"text\":\"hello\",\"unknown_part_field\":true}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, time_created, time_updated, data)
             VALUES ('evt_agent', 'ses_origin', 'agent-switched', 1000, 1000,
                     '{\"agent\":\"build\",\"time\":{\"created\":1000}}')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn row_clone_preserves_all_columns_and_remaps_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oc.db");
        {
            let conn = Connection::open(&path).unwrap();
            seed_origin(&conn);
        }
        let report =
            clone_session_rows(&path, "ses_origin", &OpenCodeRowCloneOpts::default()).unwrap();
        let new_sid = report.new_session_id.clone();
        assert!(new_sid.starts_with("ses_"));
        assert_ne!(new_sid, "ses_origin");
        assert_eq!(report.messages_copied, 2);
        assert_eq!(report.parts_copied, 1);
        assert_eq!(report.session_messages_copied, 1);

        let conn = Connection::open(&path).unwrap();

        // Origin row untouched (no INSERT OR REPLACE damage).
        let (origin_title, origin_slug, origin_share): (String, String, Option<String>) = conn
            .query_row(
                "SELECT title, slug, share_url FROM session WHERE id='ses_origin'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(origin_title, "My Title");
        assert_eq!(origin_slug, "user-set-slug");
        assert_eq!(origin_share.as_deref(), Some("https://share/abc"));

        // New row preserves every column.
        let (
            new_title,
            new_slug,
            new_share,
            new_perm,
            new_workspace,
            new_parent,
            new_dir,
            new_path,
            new_version,
            new_summary_add,
            new_revert,
        ): (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            String,
            Option<i64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT title, slug, share_url, permission, workspace_id, parent_id,
                        directory, path, version, summary_additions, revert
                 FROM session WHERE id = ?1",
                rusqlite::params![&new_sid],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(new_title, "My Title");
        assert_eq!(new_slug, "user-set-slug", "slug must be preserved");
        assert_eq!(
            new_share.as_deref(),
            Some("https://share/abc"),
            "share_url must be preserved"
        );
        assert_eq!(new_perm.as_deref(), Some("allow"));
        assert_eq!(new_workspace.as_deref(), Some("ws-1"));
        assert_eq!(new_parent.as_deref(), Some("ses_parent"));
        assert_eq!(new_dir, "/tmp/x");
        assert_eq!(new_path.as_deref(), Some("-tmp-x"));
        assert_eq!(new_version, "1.15.7");
        assert_eq!(new_summary_add, Some(5));
        assert_eq!(new_revert.as_deref(), Some("soft"));

        // Message row: parentID got remapped, internal unknown field kept.
        let asst_data: String = conn
            .query_row(
                "SELECT data FROM message WHERE session_id = ?1 AND data LIKE '%opencode_internal_y%'",
                rusqlite::params![&new_sid],
                |r| r.get(0),
            )
            .unwrap();
        let asst_json: serde_json::Value = serde_json::from_str(&asst_data).unwrap();
        let new_parent_id = asst_json.get("parentID").and_then(|v| v.as_str()).unwrap();
        assert_ne!(new_parent_id, "msg_user", "parentID must be remapped");
        assert!(new_parent_id.starts_with("msg_"));
        assert_eq!(
            asst_json
                .get("opencode_internal_y")
                .and_then(|v| v.as_i64()),
            Some(42),
            "unknown internal field must be preserved"
        );

        // Part row: unknown field preserved verbatim.
        let part_data: String = conn
            .query_row(
                "SELECT data FROM part WHERE session_id = ?1",
                rusqlite::params![&new_sid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(part_data.contains("\"unknown_part_field\":true"));
        assert!(part_data.contains("\"text\":\"hello\""));

        // session_message: new evt_ id (not reused).
        let evt_ids: Vec<String> = conn
            .prepare("SELECT id FROM session_message WHERE session_id = ?1")
            .unwrap()
            .query_map(rusqlite::params![&new_sid], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(evt_ids.len(), 1);
        assert!(evt_ids[0].starts_with("evt_"));
        assert_ne!(evt_ids[0], "evt_agent");
    }

    #[test]
    fn row_clone_with_cwd_override_rewrites_directory_and_path_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oc.db");
        {
            let conn = Connection::open(&path).unwrap();
            seed_origin(&conn);
        }
        let report = clone_session_rows(
            &path,
            "ses_origin",
            &OpenCodeRowCloneOpts {
                cwd: Some("/new/cwd".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let conn = Connection::open(&path).unwrap();
        let (new_dir, new_path): (String, Option<String>) = conn
            .query_row(
                "SELECT directory, path FROM session WHERE id = ?1",
                rusqlite::params![&report.new_session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(new_dir, "/new/cwd");
        assert_eq!(new_path.as_deref(), Some("-new-cwd"));

        // path.cwd inside message.data also rewritten.
        let asst_data: String = conn
            .query_row(
                "SELECT data FROM message WHERE session_id = ?1 AND data LIKE '%parentID%'",
                rusqlite::params![&report.new_session_id],
                |r| r.get(0),
            )
            .unwrap();
        let asst_json: serde_json::Value = serde_json::from_str(&asst_data).unwrap();
        assert_eq!(
            asst_json.pointer("/path/cwd").and_then(|v| v.as_str()),
            Some("/new/cwd")
        );
    }
}
