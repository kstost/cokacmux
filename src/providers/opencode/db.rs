//! Helpers for opening opencode.db.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::error::Result;

pub fn open_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(conn)
}

pub fn open_readwrite(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    Ok(conn)
}

/// Minimal schema for a target opencode.db we create from scratch. Mirrors
/// the columns we use; opencode itself adds more migrations.
pub const SCHEMA_MIN: &str = r#"
CREATE TABLE IF NOT EXISTS project (
    id TEXT PRIMARY KEY,
    worktree TEXT NOT NULL,
    vcs TEXT,
    name TEXT,
    icon_url TEXT,
    icon_color TEXT,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL,
    time_initialized INTEGER,
    sandboxes TEXT NOT NULL DEFAULT '{}',
    commands TEXT,
    icon_url_override TEXT
);
CREATE TABLE IF NOT EXISTS session (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    parent_id TEXT,
    slug TEXT NOT NULL DEFAULT '',
    directory TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    version TEXT NOT NULL DEFAULT '',
    share_url TEXT,
    summary_additions INTEGER,
    summary_deletions INTEGER,
    summary_files INTEGER,
    summary_diffs TEXT,
    revert TEXT,
    permission TEXT,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL,
    time_compacting INTEGER,
    time_archived INTEGER,
    workspace_id TEXT,
    path TEXT,
    agent TEXT,
    model TEXT,
    cost REAL NOT NULL DEFAULT 0,
    tokens_input INTEGER NOT NULL DEFAULT 0,
    tokens_output INTEGER NOT NULL DEFAULT 0,
    tokens_reasoning INTEGER NOT NULL DEFAULT 0,
    tokens_cache_read INTEGER NOT NULL DEFAULT 0,
    tokens_cache_write INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS message (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS part (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS session_message (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    type TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS session_message_session_idx
    ON session_message (session_id);
CREATE INDEX IF NOT EXISTS session_message_session_type_idx
    ON session_message (session_id, type);
CREATE INDEX IF NOT EXISTS session_message_time_created_idx
    ON session_message (time_created);
"#;

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_MIN)?;
    Ok(())
}
