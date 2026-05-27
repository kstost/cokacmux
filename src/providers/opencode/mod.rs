//! OpenCode adapter — SQLite (`opencode.db`).

pub mod clone;
pub mod db;
pub mod from_universal;
pub mod read;
pub mod write;

#[cfg(feature = "discovery")]
pub mod install;

use std::path::Path;

use crate::error::Result;
use crate::universal::UniversalSession;

pub fn from_db_path(db_path: &Path, session_id: &str) -> Result<UniversalSession> {
    read::from_db_path(db_path, session_id)
}

pub fn from_db_connection(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<UniversalSession> {
    read::from_db_connection(conn, session_id)
}

pub fn to_db_path(session: &UniversalSession, db_path: &Path) -> Result<()> {
    write::to_db_path(session, db_path)
}
