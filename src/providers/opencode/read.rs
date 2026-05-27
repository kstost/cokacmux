//! OpenCode SQLite → UniversalSession.
//!
//! Schema (live from `opencode.db` v1.15.5):
//!   session(id, project_id, directory, title, agent, model, cost,
//!           tokens_input, tokens_output, tokens_reasoning,
//!           tokens_cache_read, tokens_cache_write,
//!           time_created, time_updated, ...)
//!   message(id, session_id, time_created, time_updated, data JSON)
//!   part   (id, message_id, session_id, time_created, time_updated, data JSON)
//!   session_message(id, session_id, type, time_created, time_updated, data JSON)
//!
//! `data` is JSON-as-TEXT. For `message.data` we see e.g.
//!   {"role":"user", "time": {"created": ms}, "agent":..., "model":..., "summary":{...}}
//!   {"parentID":..., "role":"assistant", "mode":"build", "path":{...},
//!    "cost":..., "tokens":{...}, "model":{...}, ...}
//! For `part.data` we see {type:"text", text:...} | {type:"tool", ...} | etc.

use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;

use crate::debug;
use crate::error::{ConvertError, Result};
use crate::universal::UniversalSession;

use super::db;
use super::from_universal::build_session;

pub fn from_db_path(path: &Path, session_id: &str) -> Result<UniversalSession> {
    debug::log(
        "provider_opencode_read_db_start",
        serde_json::json!({
            "db_path": path.display().to_string(),
            "session_id": session_id,
        }),
    );
    let conn = match db::open_readonly(path) {
        Ok(conn) => {
            debug::log(
                "provider_opencode_read_db_open_ok",
                serde_json::json!({
                    "db_path": path.display().to_string(),
                }),
            );
            conn
        }
        Err(error) => {
            debug::log(
                "provider_opencode_read_db_error",
                serde_json::json!({
                    "db_path": path.display().to_string(),
                    "session_id": session_id,
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };
    let mut s = match from_db_connection(&conn, session_id) {
        Ok(session) => session,
        Err(error) => {
            debug::log(
                "provider_opencode_read_db_error",
                serde_json::json!({
                    "db_path": path.display().to_string(),
                    "session_id": session_id,
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };
    s.origin.source_path = Some(format!("{}#{}", path.display(), session_id));
    if let Ok(meta) = path.metadata() {
        if let Ok(mtime) = meta.modified() {
            if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                s.updated_at = crate::time::from_epoch_s(d.as_secs() as i64);
            }
        }
    }
    debug::log(
        "provider_opencode_read_db_ok",
        serde_json::json!({
            "db_path": path.display().to_string(),
            "session_id": &s.session_id,
            "messages": s.messages.len(),
            "cwd": &s.cwd,
            "title_present": s.title.is_some(),
        }),
    );
    Ok(s)
}

pub fn from_db_connection(conn: &Connection, session_id: &str) -> Result<UniversalSession> {
    debug::log(
        "provider_opencode_read_connection_start",
        serde_json::json!({
            "session_id": session_id,
        }),
    );
    // session row
    let mut stmt = conn.prepare(
        "SELECT id, project_id, directory, title, agent, model, cost,
                tokens_input, tokens_output, tokens_reasoning,
                tokens_cache_read, tokens_cache_write,
                time_created, time_updated,
                parent_id, slug, version, share_url,
                summary_additions, summary_deletions, summary_files, summary_diffs,
                revert, permission, time_compacting, time_archived, workspace_id, path
         FROM session WHERE id = ?1",
    )?;
    let session_row = stmt
        .query_row(rusqlite::params![session_id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                directory: row.get(2)?,
                title: row.get(3)?,
                agent: row.get(4)?,
                model: row.get(5)?,
                cost: row.get(6)?,
                tokens_input: row.get(7)?,
                tokens_output: row.get(8)?,
                tokens_reasoning: row.get(9)?,
                tokens_cache_read: row.get(10)?,
                tokens_cache_write: row.get(11)?,
                time_created: row.get(12)?,
                time_updated: row.get(13)?,
                parent_id: row.get(14)?,
                slug: row.get(15)?,
                version: row.get(16)?,
                share_url: row.get(17)?,
                summary_additions: row.get(18)?,
                summary_deletions: row.get(19)?,
                summary_files: row.get(20)?,
                summary_diffs: row.get(21)?,
                revert: row.get(22)?,
                permission: row.get(23)?,
                time_compacting: row.get(24)?,
                time_archived: row.get(25)?,
                workspace_id: row.get(26)?,
                path: row.get(27)?,
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                ConvertError::Parse(format!("no session row for id {}", session_id))
            }
            other => ConvertError::Sqlite(other),
        })?;

    // messages
    let mut stmt = conn.prepare(
        "SELECT id, session_id, time_created, time_updated, data
         FROM message WHERE session_id = ?1 ORDER BY time_created ASC, id ASC",
    )?;
    let message_rows: Vec<MessageRow> = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                time_created: row.get(2)?,
                time_updated: row.get(3)?,
                data: row.get(4)?,
            })
        })?
        .filter_map(Result_alias)
        .collect();

    // parts (we'll group by message_id)
    let mut stmt = conn.prepare(
        "SELECT id, message_id, session_id, time_created, time_updated, data
         FROM part WHERE session_id = ?1 ORDER BY time_created ASC, id ASC",
    )?;
    let part_rows: Vec<PartRow> = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok(PartRow {
                id: row.get(0)?,
                message_id: row.get(1)?,
                time_created: row.get(3)?,
                time_updated: row.get(4)?,
                data: row.get(5)?,
            })
        })?
        .filter_map(Result_alias)
        .collect();

    let session_message_rows = if table_exists(conn, "session_message")? {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, type, time_created, time_updated, data
             FROM session_message
             WHERE session_id = ?1
             ORDER BY time_created ASC, id ASC",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![session_id], |row| {
                Ok(SessionMessageRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    type_tag: row.get(2)?,
                    time_created: row.get(3)?,
                    time_updated: row.get(4)?,
                    data: row.get(5)?,
                })
            })?
            .filter_map(Result_alias)
            .collect();
        rows
    } else {
        Vec::new()
    };

    debug::log(
        "provider_opencode_read_rows",
        serde_json::json!({
            "session_id": session_id,
            "messages": message_rows.len(),
            "parts": part_rows.len(),
            "session_messages": session_message_rows.len(),
        }),
    );

    let result = build_session(
        &session_row,
        &message_rows,
        &part_rows,
        &session_message_rows,
    );
    match &result {
        Ok(session) => debug::log(
            "provider_opencode_read_connection_ok",
            serde_json::json!({
                "session_id": &session.session_id,
                "messages": session.messages.len(),
            }),
        ),
        Err(error) => debug::log(
            "provider_opencode_read_connection_error",
            serde_json::json!({
                "session_id": session_id,
                "error": error.to_string(),
            }),
        ),
    }
    result
}

#[allow(non_snake_case)]
fn Result_alias<T>(r: std::result::Result<T, rusqlite::Error>) -> Option<T> {
    r.ok()
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        rusqlite::params![table],
        |row| row.get(0),
    )?;
    Ok(exists > 0)
}

pub struct SessionRow {
    pub id: String,
    pub project_id: String,
    pub directory: String,
    pub title: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub cost: f64,
    pub tokens_input: i64,
    pub tokens_output: i64,
    pub tokens_reasoning: i64,
    pub tokens_cache_read: i64,
    pub tokens_cache_write: i64,
    pub time_created: i64,
    pub time_updated: i64,
    pub parent_id: Option<String>,
    pub slug: String,
    pub version: String,
    pub share_url: Option<String>,
    pub summary_additions: Option<i64>,
    pub summary_deletions: Option<i64>,
    pub summary_files: Option<i64>,
    pub summary_diffs: Option<String>,
    pub revert: Option<String>,
    pub permission: Option<String>,
    pub time_compacting: Option<i64>,
    pub time_archived: Option<i64>,
    pub workspace_id: Option<String>,
    pub path: Option<String>,
}

pub struct MessageRow {
    pub id: String,
    pub time_created: i64,
    pub time_updated: i64,
    pub data: String, // JSON
}

pub struct PartRow {
    pub id: String,
    pub message_id: String,
    pub time_created: i64,
    pub time_updated: i64,
    pub data: String, // JSON
}

pub struct SessionMessageRow {
    pub id: String,
    pub session_id: String,
    pub type_tag: String,
    pub time_created: i64,
    pub time_updated: i64,
    pub data: String, // JSON
}

impl MessageRow {
    pub fn parse_data(&self) -> Value {
        serde_json::from_str(&self.data).unwrap_or(Value::Null)
    }
}

impl PartRow {
    pub fn parse_data(&self) -> Value {
        serde_json::from_str(&self.data).unwrap_or(Value::Null)
    }
}

impl SessionMessageRow {
    pub fn parse_data(&self) -> Value {
        serde_json::from_str(&self.data).unwrap_or(Value::Null)
    }
}
