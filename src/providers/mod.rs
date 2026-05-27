//! Provider adapters.

#[cfg(feature = "claude")]
pub mod claude;

#[cfg(feature = "codex")]
pub mod codex;

#[cfg(feature = "opencode")]
pub mod opencode;

#[cfg(feature = "discovery")]
pub mod discovery;
