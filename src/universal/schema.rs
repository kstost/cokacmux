//! UniversalSession — the schema everything converts through.
//!
//! Design principles:
//! - **Provider-agnostic** first-class fields (`role: User|Assistant|…`), with
//!   provider-specific extras under `extras: Map<String, Value>` slots.
//! - **Lossless**: each `UMessage` keeps its original raw record under
//!   `provenance.raw` so `to_X` can recover provider-native fields when
//!   round-tripping back to the same provider.
//! - **Stable tool pairing**: every `ToolUse` has a `call_id`; the
//!   corresponding `ToolResult` references the same `call_id`. Providers
//!   that don't supply a call id get a synthetic one.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current schema version. Bumped on breaking changes.
pub const SCHEMA_VERSION: &str = "ut/1.0";

/// Which agent the session originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
    OpenCode,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
            Provider::OpenCode => "opencode",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "codex" => Some(Provider::Codex),
            "claude" => Some(Provider::Claude),
            "opencode" => Some(Provider::OpenCode),
            _ => None,
        }
    }
}

/// Logical role of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
    Developer,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderOrigin {
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cli_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_mtime: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelInfo {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provider_id: Option<String>,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitInfo {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageFlags {
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_sidechain: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_meta: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_compaction: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub skipped: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    /// Provider-namespaced event tag, e.g. "codex:response_item.function_call",
    /// "claude:assistant", "opencode:part.text".
    pub source_event_type: String,
    /// Verbatim original record (one JSONL line or one DB row encoded as JSON).
    /// Preserved so to_X back to the same provider is fully lossless.
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extras: BTreeMap<String, Value>,
    },
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extras: BTreeMap<String, Value>,
    },
    ToolUse {
        call_id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extras: BTreeMap<String, Value>,
    },
    ToolResult {
        call_id: String,
        output: Value,
        #[serde(default, skip_serializing_if = "is_false_ref")]
        is_error: bool,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extras: BTreeMap<String, Value>,
    },
    Image {
        mime: String,
        source: ImageSource,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extras: BTreeMap<String, Value>,
    },
    Attachment {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extras: BTreeMap<String, Value>,
    },
    Patch {
        unified_diff: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extras: BTreeMap<String, Value>,
    },
    /// Catch-all for content kinds we don't have first-class support for.
    /// Preserves the payload verbatim under `payload`.
    Other { type_tag: String, payload: Value },
}

fn is_false_ref(b: &bool) -> bool {
    !*b
}

impl ContentBlock {
    pub fn text<S: Into<String>>(s: S) -> Self {
        ContentBlock::Text {
            text: s.into(),
            extras: BTreeMap::new(),
        }
    }
    pub fn thinking<S: Into<String>>(s: S) -> Self {
        ContentBlock::Thinking {
            text: s.into(),
            encrypted: None,
            extras: BTreeMap::new(),
        }
    }
    pub fn tool_use(call_id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        ContentBlock::ToolUse {
            call_id: call_id.into(),
            name: name.into(),
            input,
            extras: BTreeMap::new(),
        }
    }
    pub fn tool_result(call_id: impl Into<String>, output: Value, is_error: bool) -> Self {
        ContentBlock::ToolResult {
            call_id: call_id.into(),
            output,
            is_error,
            extras: BTreeMap::new(),
        }
    }
    pub fn other(tag: impl Into<String>, payload: Value) -> Self {
        ContentBlock::Other {
            type_tag: tag.into(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSource {
    LocalPath { path: String },
    Base64 { data: String },
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub flags: MessageFlags,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalSession {
    pub schema_version: String,
    pub session_id: String,
    pub origin: ProviderOrigin,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_total: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_meta: Option<Value>,
    pub messages: Vec<UMessage>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, Value>,
}

impl UniversalSession {
    pub fn new(session_id: impl Into<String>, provider: Provider, cwd: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            session_id: session_id.into(),
            origin: ProviderOrigin {
                provider: Some(provider),
                ..Default::default()
            },
            cwd: cwd.into(),
            created_at: None,
            updated_at: None,
            title: None,
            model: None,
            git: None,
            usage_total: None,
            session_meta: None,
            messages: Vec::new(),
            extras: BTreeMap::new(),
        }
    }
}
