//! cokacmux — converter and session manager for coding-agent session data.
//!
//! Supported coding-agent providers write their session data to disk in
//! very different shapes. The currently implemented adapters include:
//!
//! - **Claude**: JSONL per session under `~/.claude/projects/<encoded-cwd>/`
//!   with sidecar `tool-results/` directory for overflow.
//! - **Codex**: JSONL "rollout" files under `~/.codex/sessions/YYYY/MM/DD/`,
//!   indexed by a SQLite `state_5.sqlite::threads` table.
//! - **OpenCode**: SQLite database at `~/.local/share/opencode/opencode.db`
//!   with `session`, `message`, and `part` tables.
//! - **Pi**: JSONL session files under `~/.pi/agent/sessions/--<cwd>--/`
//!   with in-file `id`/`parentId` tree structure.
//! - **GJC**: JSONL session files under `~/.gjc/agent/sessions/<encoded-cwd>/`
//!   with the same in-file tree structure as Pi.
//!
//! This crate defines [`UniversalSession`] — a provider-agnostic data model
//! that can hold supported provider data without loss — and provides
//! `from_X` / `to_X` adapter pairs for each provider.
//!
//! Same-provider adapter round-trips may preserve native data, but `convert()`
//! is deliberately a different contract: it does not try to reproduce a full
//! native session history in another provider. Instead it wraps the source
//! session into a two-message continuation context: one user message containing
//! the rendered source context, followed by one assistant message containing
//! `ok`.
//!
//! ## Quick start
//!
//! ```no_run
//! use cokacmux::{Provider, SessionSource, read_session};
//! # fn main() -> cokacmux::Result<()> {
//! let session = read_session(
//!     Provider::Claude,
//!     &SessionSource::Path("/path/to/session.jsonl".into()),
//! )?;
//! println!("{} messages", session.messages.len());
//! # Ok(()) }
//! ```

#![forbid(unsafe_code)]

pub mod context_convert;
#[doc(hidden)]
pub mod debug;
pub mod error;
pub mod ids;
pub mod jsonl;
pub mod pivot;
pub mod providers;
pub mod time;
pub mod universal;

#[cfg(feature = "discovery")]
pub mod session;

// Re-exports — these form the public surface area.
pub use context_convert::{
    context_user_message_text, wrap_session_for_context_convert, CONTEXT_ACK,
    CONTEXT_CONTINUATION_PROMPT,
};
pub use error::{ConvertError, Result};
pub use universal::{
    ContentBlock, GitInfo, ImageSource, MessageFlags, ModelInfo, Provenance, Provider,
    ProviderOrigin, Role, UMessage, UniversalSession, Usage,
};

/// Enable or disable best-effort debug logging for this process.
///
/// The interactive `cokacmux` binary calls this when launched with
/// `--debug`. Library users normally do not need to call it.
pub fn set_debug_logging(enabled: bool) {
    debug::set_enabled(enabled);
}

use std::path::PathBuf;

/// Where to read a session from.
#[derive(Debug, Clone)]
pub enum SessionSource {
    /// A JSONL file path (Claude, Codex, Pi, or GJC).
    Path(PathBuf),
    /// An OpenCode database + session id.
    OpenCodeDb {
        db_path: PathBuf,
        session_id: String,
    },
    /// Auto-pick the most recently updated session for `provider` whose
    /// working directory equals `cwd`.
    LatestInCwd { provider: Provider, cwd: PathBuf },
}

/// Where to write a session to.
#[derive(Debug, Clone)]
pub enum SessionTarget {
    /// A file path. JSONL for Claude/Codex/Pi/GJC; not valid for OpenCode.
    Path(PathBuf),
    /// An OpenCode database path (will INSERT into `session`/`message`/`part`).
    OpenCodeDb { db_path: PathBuf },
}

/// Read a session from `src` and parse it into a [`UniversalSession`].
pub fn read_session(provider: Provider, src: &SessionSource) -> Result<UniversalSession> {
    debug::log(
        "read_session_start",
        serde_json::json!({
            "provider": provider.as_str(),
            "source": session_source_label(src),
        }),
    );
    let result: Result<UniversalSession> = match (provider, src) {
        #[cfg(feature = "claude")]
        (Provider::Claude, SessionSource::Path(p)) => {
            providers::claude::from_file(p, &Default::default())
        }
        #[cfg(feature = "codex")]
        (Provider::Codex, SessionSource::Path(p)) => providers::codex::from_file(p),
        #[cfg(feature = "pi")]
        (Provider::Pi, SessionSource::Path(p)) => providers::pi::from_file(p),
        #[cfg(feature = "gjc")]
        (Provider::Gjc, SessionSource::Path(p)) => providers::gjc::from_file(p),
        #[cfg(feature = "opencode")]
        (
            Provider::OpenCode,
            SessionSource::OpenCodeDb {
                db_path,
                session_id,
            },
        ) => providers::opencode::from_db_path(db_path, session_id),
        #[cfg(feature = "discovery")]
        (prov, SessionSource::LatestInCwd { provider, cwd }) if *provider == prov => {
            providers::discovery::latest_for_cwd(prov, cwd)
        }
        (p, _) => Err(ConvertError::Unsupported(format!(
            "read_session: provider={:?} source mismatch",
            p
        ))),
    };
    match &result {
        Ok(session) => debug::log(
            "read_session_ok",
            serde_json::json!({
                "provider": provider.as_str(),
                "session_id": &session.session_id,
                "messages": session.messages.len(),
                "cwd": &session.cwd,
            }),
        ),
        Err(error) => debug::log(
            "read_session_error",
            serde_json::json!({
                "provider": provider.as_str(),
                "error": error.to_string(),
            }),
        ),
    }
    result
}

/// Write a session to `dst` in the given `provider`'s native format.
pub fn write_session(
    provider: Provider,
    #[allow(unused_variables)] session: &UniversalSession,
    dst: &SessionTarget,
) -> Result<()> {
    debug::log(
        "write_session_start",
        serde_json::json!({
            "provider": provider.as_str(),
            "target": session_target_label(dst),
            "session_id": &session.session_id,
            "messages": session.messages.len(),
        }),
    );
    let result: Result<()> = match (provider, dst) {
        #[cfg(feature = "claude")]
        (Provider::Claude, SessionTarget::Path(p)) => {
            providers::claude::to_file(session, p, &Default::default())
        }
        #[cfg(feature = "codex")]
        (Provider::Codex, SessionTarget::Path(p)) => {
            providers::codex::to_file(session, p, &Default::default())
        }
        #[cfg(feature = "pi")]
        (Provider::Pi, SessionTarget::Path(p)) => {
            providers::pi::to_file(session, p, &Default::default())
        }
        #[cfg(feature = "gjc")]
        (Provider::Gjc, SessionTarget::Path(p)) => {
            providers::gjc::to_file(session, p, &Default::default())
        }
        #[cfg(feature = "opencode")]
        (Provider::OpenCode, SessionTarget::OpenCodeDb { db_path }) => {
            providers::opencode::to_db_path(session, db_path)
        }
        (p, _) => Err(ConvertError::Unsupported(format!(
            "write_session: provider={:?} target mismatch",
            p
        ))),
    };
    match &result {
        Ok(()) => debug::log(
            "write_session_ok",
            serde_json::json!({
                "provider": provider.as_str(),
                "session_id": &session.session_id,
            }),
        ),
        Err(error) => debug::log(
            "write_session_error",
            serde_json::json!({
                "provider": provider.as_str(),
                "session_id": &session.session_id,
                "error": error.to_string(),
            }),
        ),
    }
    result
}

/// Context-wrapper conversion in one call.
///
/// This is intentionally not a lossless cross-provider transcript converter.
/// Keep adapter-level preservation tests separate from this public helper so
/// future changes do not accidentally reintroduce a hidden provider-to-provider
/// synthesis contract.
///
/// The written target session contains two visible messages: a user message
/// with the rendered source context and continuation prompt, then an assistant
/// `ok`.
pub fn convert(
    from: Provider,
    to: Provider,
    src: &SessionSource,
    dst: &SessionTarget,
) -> Result<UniversalSession> {
    debug::log(
        "convert_start",
        serde_json::json!({
            "from": from.as_str(),
            "to": to.as_str(),
            "source": session_source_label(src),
            "target": session_target_label(dst),
        }),
    );
    let source_session = match read_session(from, src) {
        Ok(session) => session,
        Err(error) => {
            debug::log(
                "convert_error",
                serde_json::json!({
                    "stage": "read",
                    "from": from.as_str(),
                    "to": to.as_str(),
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };
    let converted = wrap_session_for_context_convert(&source_session, to);
    if let Err(error) = write_session(to, &converted, dst) {
        debug::log(
            "convert_error",
            serde_json::json!({
                "stage": "write",
                "from": from.as_str(),
                "to": to.as_str(),
                "source_session_id": &source_session.session_id,
                "target_session_id": &converted.session_id,
                "error": error.to_string(),
            }),
        );
        return Err(error);
    }
    debug::log(
        "convert_ok",
        serde_json::json!({
            "from": from.as_str(),
            "to": to.as_str(),
            "source_session_id": &source_session.session_id,
            "target_session_id": &converted.session_id,
            "source_messages": source_session.messages.len(),
            "target_messages": converted.messages.len(),
        }),
    );
    Ok(converted)
}

fn session_source_label(src: &SessionSource) -> String {
    match src {
        SessionSource::Path(path) => format!("path:{}", path.display()),
        SessionSource::OpenCodeDb {
            db_path,
            session_id,
        } => format!("opencode_db:{}#{}", db_path.display(), session_id),
        SessionSource::LatestInCwd { provider, cwd } => {
            format!("latest_in_cwd:{}:{}", provider.as_str(), cwd.display())
        }
    }
}

fn session_target_label(dst: &SessionTarget) -> String {
    match dst {
        SessionTarget::Path(path) => format!("path:{}", path.display()),
        SessionTarget::OpenCodeDb { db_path } => format!("opencode_db:{}", db_path.display()),
    }
}
