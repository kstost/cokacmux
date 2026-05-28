//! cokacmux — TUI session browser for Claude Code / Codex / OpenCode.
//!
//! Layout:
//!   ┌────────────────────────────┬─────────────────────────────┐
//!   │ session list (filterable)  │ preview (summary render)    │
//!   │                            │                             │
//!   └────────────────────────────┴─────────────────────────────┘
//!   q/Ctrl+Q quit · ↑↓ nav · / filter · v tree · t title · c clone · Delete/d del · r refresh

use std::collections::{hash_map::DefaultHasher, HashMap, HashSet, VecDeque};
#[cfg(windows)]
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, ErrorKind, Read, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

#[cfg(windows)]
use std::net::{TcpListener as AgentListener, TcpStream as AgentStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener as AgentListener, UnixStream as AgentStream};
#[cfg(unix)]
use std::os::unix::process::CommandExt as UnixCommandExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt as WindowsCommandExt;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn FreeConsole() -> i32;
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
}

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use cokacmux::providers;
use cokacmux::providers::discovery::SessionInfo;
use cokacmux::session;
use cokacmux::session::render::Mode;
use cokacmux::universal::Provider;

type Tui = Terminal<CrosstermBackend<Stdout>>;
const PREVIEW_CACHE_LIMIT: usize = 16;
const DEFAULT_SESSIONS_PANE_PERCENT: u16 = 45;
const PANE_RESIZE_STEP_COLUMNS: u16 = 2;
const AGENT_SCROLLBACK_LINES: usize = 10_000;
const AGENT_STATUS_HEIGHT: u16 = 1;
const AGENT_SIDEBAR_WIDTH: u16 = 30;
const DEFAULT_AGENT_SIDEBAR_WIDTH: u16 = AGENT_SIDEBAR_WIDTH;
const AGENT_SIDEBAR_RESIZE_STEP: u16 = PANE_RESIZE_STEP_COLUMNS;
const AGENT_MIN_PTY_COLS: u16 = 20;
const AGENT_MIN_PTY_ROWS: u16 = 5;
const AGENT_OUTPUT_POLL_LIMIT: usize = 256;
const TERMINAL_RESPONSE_SCAN_TAIL_BYTES: usize = 4;
const AGENT_DAEMON_ARG: &str = "--agent-daemon";
const AGENT_DAEMON_START_TIMEOUT_MS: u64 = 3_000;
const AGENT_STATE_POLL_INTERVAL_MS: u64 = 500;
const AGENT_BUSY_GRACE_MS: u64 = 3_000;
const AGENT_ACTIVITY_META_WRITE_INTERVAL_MS: u64 = 750;
const DEBUG_LOG_FILE: &str = "cokacmux.log";
const DEBUG_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const APP_DIR_NAME: &str = ".cokacmux";
const AGENT_SCROLL_PAGE_UP_DEFAULTS: &[&str] = &["shift+alt+up", "shift+alt+pageup"];
const AGENT_SCROLL_PAGE_DOWN_DEFAULTS: &[&str] = &["shift+alt+down", "shift+alt+pagedown"];
const PREVIOUS_AGENT_SCROLL_PAGE_UP_DEFAULTS: &[&str] = &["shift+alt+pageup"];
const PREVIOUS_AGENT_SCROLL_PAGE_DOWN_DEFAULTS: &[&str] = &["shift+alt+pagedown"];
const LEGACY_AGENT_SCROLL_PAGE_UP_DEFAULTS: &[&str] = &["shift+pageup", "alt+pageup"];
const LEGACY_AGENT_SCROLL_PAGE_DOWN_DEFAULTS: &[&str] = &["shift+pagedown", "alt+pagedown"];
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static WINDOWS_DAEMON_CTRL_EVENT_COUNT: AtomicU32 = AtomicU32::new(0);
#[cfg(windows)]
static WINDOWS_DAEMON_CTRL_EVENT_LAST: AtomicU32 = AtomicU32::new(0);

// Comfort-focused 256-color palette for dark terminals.
const THEME_BG: Color = Color::Indexed(234);
const THEME_BG_ALT: Color = Color::Indexed(235);
const THEME_STATUS_BG: Color = Color::Indexed(237);
const THEME_FG: Color = Color::Indexed(252);
const THEME_FG_DIM: Color = Color::Indexed(245);
const THEME_FG_STRONG: Color = Color::Indexed(255);
const THEME_SELECTED_BG: Color = Color::Indexed(66);
const THEME_SELECTED_TEXT: Color = Color::Indexed(255);
const THEME_ACCENT: Color = Color::Indexed(109);
const THEME_SHORTCUT: Color = Color::Indexed(109);
const THEME_POSITIVE: Color = Color::Indexed(108);
const THEME_BORDER: Color = Color::Indexed(240);
const THEME_BORDER_ACTIVE: Color = Color::Indexed(110);
const THEME_PROVIDER_CLAUDE: Color = Color::Indexed(139);
const THEME_PROVIDER_CODEX: Color = Color::Indexed(110);
const THEME_PROVIDER_OPENCODE: Color = Color::Indexed(107);
const AGENT_DEFAULT_BG: Color = THEME_BG;
const STARTUP_SPINNER_FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
const STARTUP_SPINNER_TICK_MS: u128 = 180;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    cokacmux: CokacmuxSettings,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
    #[cfg(test)]
    #[serde(skip)]
    skip_save: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cokacmux: CokacmuxSettings::default(),
            extra: serde_json::Map::new(),
            #[cfg(test)]
            skip_save: false,
        }
    }
}

impl Settings {
    fn load() -> Self {
        if let Some(path) = settings_path() {
            match fs::read_to_string(&path) {
                Ok(content) => {
                    if let Ok(settings) = serde_json::from_str::<Self>(&content) {
                        return settings.normalized();
                    }
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    let settings = Self::default();
                    let _ = settings.save_to_path(&path);
                    return settings;
                }
                Err(_) => {}
            }
        }
        Self::default()
    }

    fn normalized(mut self) -> Self {
        self.cokacmux.sessions_pane_percent = self.cokacmux.sessions_pane_percent.min(100);
        self.cokacmux.agent_programs.normalize_placeholders();
        self
    }

    fn save(&self) -> Result<()> {
        #[cfg(test)]
        if self.skip_save {
            return Ok(());
        }
        let Some(path) = settings_path() else {
            anyhow::bail!("cannot resolve home directory");
        };
        self.save_to_path(&path)
    }

    fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, format!("{}\n", content))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CokacmuxSettings {
    #[serde(default = "default_sessions_pane_percent")]
    sessions_pane_percent: u16,
    #[serde(default)]
    sessions_pane_width: Option<u16>,
    #[serde(default = "default_agent_sidebar_width")]
    agent_sidebar_width: u16,
    /// Whether the left "agents [N]" sidebar is currently visible in the
    /// agents view. Toggled by Ctrl+B. The configured width is preserved
    /// regardless; hiding/showing just collapses the column to 0.
    #[serde(default = "default_agent_sidebar_visible")]
    agent_sidebar_visible: bool,
    #[serde(default = "default_session_view")]
    session_view: SessionViewMode,
    #[serde(default)]
    agent_programs: AgentProgramSettings,
    #[serde(default, rename = "debug", skip_serializing)]
    _debug: bool,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for CokacmuxSettings {
    fn default() -> Self {
        Self {
            sessions_pane_percent: DEFAULT_SESSIONS_PANE_PERCENT,
            sessions_pane_width: None,
            agent_sidebar_width: DEFAULT_AGENT_SIDEBAR_WIDTH,
            agent_sidebar_visible: default_agent_sidebar_visible(),
            session_view: default_session_view(),
            agent_programs: AgentProgramSettings::default(),
            _debug: false,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct AgentProgramSettings {
    #[serde(default)]
    codex: Option<String>,
    #[serde(default)]
    claude: Option<String>,
    #[serde(default)]
    opencode: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for AgentProgramSettings {
    fn default() -> Self {
        Self {
            codex: Some(String::new()),
            claude: Some(String::new()),
            opencode: Some(String::new()),
            extra: serde_json::Map::new(),
        }
    }
}

impl AgentProgramSettings {
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        option_string_is_blank_or_none(&self.codex)
            && option_string_is_blank_or_none(&self.claude)
            && option_string_is_blank_or_none(&self.opencode)
            && self.extra.is_empty()
    }

    fn normalize_placeholders(&mut self) {
        normalize_program_placeholder(&mut self.codex);
        normalize_program_placeholder(&mut self.claude);
        normalize_program_placeholder(&mut self.opencode);
    }

    fn program_for(&self, provider: Provider) -> String {
        let configured = match provider {
            Provider::Codex => self.codex.as_deref(),
            Provider::Claude => self.claude.as_deref(),
            Provider::OpenCode => self.opencode.as_deref(),
        }
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(expand_configured_program_path);

        configured.unwrap_or_else(|| default_agent_program(provider).to_string())
    }
}

fn normalize_program_placeholder(value: &mut Option<String>) {
    if option_string_is_blank_or_none(value) {
        *value = Some(String::new());
    }
}

fn option_string_is_blank_or_none(value: &Option<String>) -> bool {
    match value.as_deref() {
        Some(value) => value.trim().is_empty(),
        None => true,
    }
}

fn expand_configured_program_path(value: &str) -> String {
    if value == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.display().to_string();
        }
    } else if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    value.to_string()
}

fn default_agent_sidebar_visible() -> bool {
    true
}

fn default_sessions_pane_percent() -> u16 {
    DEFAULT_SESSIONS_PANE_PERCENT
}

fn default_agent_sidebar_width() -> u16 {
    DEFAULT_AGENT_SIDEBAR_WIDTH
}

fn default_session_view() -> SessionViewMode {
    SessionViewMode::Tree
}

fn init_debug_from_cli(debug_enabled: bool, trace_enabled: bool) {
    let env_enabled = std::env::var("COKACMUX_DEBUG")
        .map(|value| value == "1")
        .unwrap_or(false);
    let trace_env_enabled = std::env::var("COKACMUX_TRACE")
        .map(|value| value == "1")
        .unwrap_or(false);
    let trace_enabled = trace_enabled || trace_env_enabled;
    let enabled = debug_enabled || env_enabled || trace_enabled;
    DEBUG_ENABLED.store(enabled, Ordering::Relaxed);
    TRACE_ENABLED.store(trace_enabled, Ordering::Relaxed);
    if enabled {
        let mut sources = Vec::new();
        if debug_enabled {
            sources.push("cli");
        }
        if env_enabled {
            sources.push("COKACMUX_DEBUG");
        }
        if trace_enabled {
            sources.push("trace");
        }
        debug_log_to(
            DEBUG_LOG_FILE,
            &format!(
                "debug enabled source={} trace={}",
                sources.join(","),
                trace_enabled
            ),
        );
    }
    cokacmux::set_debug_logging(enabled);
}

fn settings_path() -> Option<PathBuf> {
    app_config_dir().map(|dir| dir.join("settings.json"))
}

fn keybinding_path() -> Option<PathBuf> {
    app_config_dir().map(|dir| dir.join("keybinding.json"))
}

fn app_config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(APP_DIR_NAME))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum KeyAction {
    GlobalQuit,
    SessionQuit,
    SessionForceQuit,
    SessionToggleAgent,
    SessionKillAgent,
    SessionNewShell,
    SessionToggleFocus,
    SessionTogglePreview,
    SessionMoveNext,
    SessionMovePrev,
    SessionPageNext,
    SessionPagePrev,
    SessionTop,
    SessionBottom,
    SessionFilter,
    SessionToggleView,
    SessionRefresh,
    SessionDelete,
    SessionClone,
    SessionEditTitle,
    SessionLaunchAgent,
    SessionRefreshPreview,
    SessionsPaneResizeLeft,
    SessionsPaneResizeRight,
    SessionsSidebarPrev,
    SessionsSidebarNext,
    AgentToggleSessions,
    AgentKill,
    AgentNewShell,
    AgentToggleSidebar,
    AgentScrollLineUp,
    AgentScrollLineDown,
    AgentScrollPageUp,
    AgentScrollPageDown,
    AgentScrollTop,
    AgentScrollBottom,
    AgentPaneResizeLeft,
    AgentPaneResizeRight,
    AgentSidebarPrev,
    AgentSidebarNext,
    AgentSwitchPrev,
    AgentSwitchNext,
    ConfirmYes,
    ConfirmNo,
    FilterCancel,
    FilterApply,
    FilterMoveLeft,
    FilterMoveRight,
    FilterHome,
    FilterEnd,
    FilterBackspace,
    FilterDelete,
    TitleCancel,
    TitleSave,
    TitleMoveLeft,
    TitleMoveRight,
    TitleHome,
    TitleEnd,
    TitleBackspace,
    TitleDelete,
    AgentLaunchCancel,
    AgentLaunchConfirm,
    AgentLaunchNext,
    AgentLaunchPrev,
    AgentLaunchNormal,
    AgentLaunchSkipPermissions,
    NewSessionCancel,
    NewSessionConfirm,
    NewSessionNext,
    NewSessionPrev,
    NewSessionChoiceNext,
    NewSessionChoicePrev,
    NewSessionBackspace,
    NewSessionDelete,
    NewSessionHome,
    NewSessionEnd,
    CloneTargetCancel,
    CloneTargetConfirm,
    CloneTargetNext,
    CloneTargetPrev,
}

const DEFAULT_KEYBINDINGS: &[(&str, KeyAction, &[&str])] = &[
    ("global.quit", KeyAction::GlobalQuit, &["ctrl+q"]),
    ("sessions.quit", KeyAction::SessionQuit, &["q"]),
    (
        "sessions.force_quit",
        KeyAction::SessionForceQuit,
        &["ctrl+c"],
    ),
    (
        "sessions.toggle_agent",
        KeyAction::SessionToggleAgent,
        &["ctrl+]", "ctrl+[", "ctrl+3", "ctrl+5"],
    ),
    (
        "sessions.kill_agent",
        KeyAction::SessionKillAgent,
        &["ctrl+k"],
    ),
    (
        "sessions.new_shell",
        KeyAction::SessionNewShell,
        &["ctrl+n"],
    ),
    (
        "sessions.toggle_focus",
        KeyAction::SessionToggleFocus,
        &["tab", "esc"],
    ),
    (
        "sessions.toggle_preview",
        KeyAction::SessionTogglePreview,
        &["enter"],
    ),
    (
        "sessions.move_next",
        KeyAction::SessionMoveNext,
        &["down", "j"],
    ),
    (
        "sessions.move_prev",
        KeyAction::SessionMovePrev,
        &["up", "k"],
    ),
    (
        "sessions.page_next",
        KeyAction::SessionPageNext,
        &["pagedown"],
    ),
    (
        "sessions.page_prev",
        KeyAction::SessionPagePrev,
        &["pageup"],
    ),
    ("sessions.top", KeyAction::SessionTop, &["home", "g"]),
    ("sessions.bottom", KeyAction::SessionBottom, &["end", "G"]),
    ("sessions.filter", KeyAction::SessionFilter, &["/"]),
    ("sessions.toggle_view", KeyAction::SessionToggleView, &["v"]),
    ("sessions.refresh", KeyAction::SessionRefresh, &["r"]),
    (
        "sessions.delete",
        KeyAction::SessionDelete,
        &["delete", "d"],
    ),
    ("sessions.clone", KeyAction::SessionClone, &["c"]),
    ("sessions.edit_title", KeyAction::SessionEditTitle, &["t"]),
    (
        "sessions.launch_agent",
        KeyAction::SessionLaunchAgent,
        &["e"],
    ),
    (
        "sessions.refresh_preview",
        KeyAction::SessionRefreshPreview,
        &["space"],
    ),
    (
        "sessions.resize_left",
        KeyAction::SessionsPaneResizeLeft,
        &["alt+left", "ctrl+shift+left"],
    ),
    (
        "sessions.resize_right",
        KeyAction::SessionsPaneResizeRight,
        &["alt+right", "ctrl+shift+right"],
    ),
    (
        "sessions.sidebar_prev",
        KeyAction::SessionsSidebarPrev,
        &["alt+up", "ctrl+shift+up"],
    ),
    (
        "sessions.sidebar_next",
        KeyAction::SessionsSidebarNext,
        &["alt+down", "ctrl+shift+down"],
    ),
    (
        "agent.toggle_sessions",
        KeyAction::AgentToggleSessions,
        &["ctrl+]", "ctrl+[", "ctrl+3", "ctrl+5"],
    ),
    ("agent.kill", KeyAction::AgentKill, &["ctrl+k"]),
    ("agent.new_shell", KeyAction::AgentNewShell, &["ctrl+n"]),
    (
        "agent.toggle_sidebar",
        KeyAction::AgentToggleSidebar,
        &["ctrl+b"],
    ),
    (
        "agent.scroll_line_up",
        KeyAction::AgentScrollLineUp,
        &["shift+up"],
    ),
    (
        "agent.scroll_line_down",
        KeyAction::AgentScrollLineDown,
        &["shift+down"],
    ),
    (
        "agent.scroll_page_up",
        KeyAction::AgentScrollPageUp,
        AGENT_SCROLL_PAGE_UP_DEFAULTS,
    ),
    (
        "agent.scroll_page_down",
        KeyAction::AgentScrollPageDown,
        AGENT_SCROLL_PAGE_DOWN_DEFAULTS,
    ),
    (
        "agent.scroll_top",
        KeyAction::AgentScrollTop,
        &["shift+home", "alt+home"],
    ),
    (
        "agent.scroll_bottom",
        KeyAction::AgentScrollBottom,
        &["shift+end", "alt+end"],
    ),
    (
        "agent.resize_left",
        KeyAction::AgentPaneResizeLeft,
        &["alt+left", "ctrl+shift+left"],
    ),
    (
        "agent.resize_right",
        KeyAction::AgentPaneResizeRight,
        &["alt+right", "ctrl+shift+right"],
    ),
    (
        "agent.sidebar_prev",
        KeyAction::AgentSidebarPrev,
        &["alt+up", "ctrl+shift+up"],
    ),
    (
        "agent.sidebar_next",
        KeyAction::AgentSidebarNext,
        &["alt+down", "ctrl+shift+down"],
    ),
    (
        "agent.switch_prev",
        KeyAction::AgentSwitchPrev,
        &["ctrl+pageup"],
    ),
    (
        "agent.switch_next",
        KeyAction::AgentSwitchNext,
        &["ctrl+pagedown"],
    ),
    ("confirm.yes", KeyAction::ConfirmYes, &["y", "Y"]),
    ("confirm.no", KeyAction::ConfirmNo, &["esc", "n", "N"]),
    ("filter.cancel", KeyAction::FilterCancel, &["esc"]),
    ("filter.apply", KeyAction::FilterApply, &["enter"]),
    ("filter.move_left", KeyAction::FilterMoveLeft, &["left"]),
    ("filter.move_right", KeyAction::FilterMoveRight, &["right"]),
    ("filter.home", KeyAction::FilterHome, &["home"]),
    ("filter.end", KeyAction::FilterEnd, &["end"]),
    (
        "filter.backspace",
        KeyAction::FilterBackspace,
        &["backspace"],
    ),
    ("filter.delete", KeyAction::FilterDelete, &["delete"]),
    ("title.cancel", KeyAction::TitleCancel, &["esc"]),
    ("title.save", KeyAction::TitleSave, &["enter"]),
    ("title.move_left", KeyAction::TitleMoveLeft, &["left"]),
    ("title.move_right", KeyAction::TitleMoveRight, &["right"]),
    ("title.home", KeyAction::TitleHome, &["home"]),
    ("title.end", KeyAction::TitleEnd, &["end"]),
    ("title.backspace", KeyAction::TitleBackspace, &["backspace"]),
    ("title.delete", KeyAction::TitleDelete, &["delete"]),
    (
        "agent_launch.cancel",
        KeyAction::AgentLaunchCancel,
        &["esc"],
    ),
    (
        "agent_launch.confirm",
        KeyAction::AgentLaunchConfirm,
        &["enter"],
    ),
    (
        "agent_launch.next",
        KeyAction::AgentLaunchNext,
        &["down", "j"],
    ),
    (
        "agent_launch.prev",
        KeyAction::AgentLaunchPrev,
        &["up", "k"],
    ),
    ("agent_launch.normal", KeyAction::AgentLaunchNormal, &["1"]),
    (
        "agent_launch.skip_permissions",
        KeyAction::AgentLaunchSkipPermissions,
        &["2"],
    ),
    ("new_session.cancel", KeyAction::NewSessionCancel, &["esc"]),
    (
        "new_session.confirm",
        KeyAction::NewSessionConfirm,
        &["enter"],
    ),
    (
        "new_session.next",
        KeyAction::NewSessionNext,
        &["down", "j", "tab"],
    ),
    (
        "new_session.prev",
        KeyAction::NewSessionPrev,
        &["up", "k", "backtab"],
    ),
    (
        "new_session.choice_next",
        KeyAction::NewSessionChoiceNext,
        &["right", "l", "space"],
    ),
    (
        "new_session.choice_prev",
        KeyAction::NewSessionChoicePrev,
        &["left", "h"],
    ),
    (
        "new_session.backspace",
        KeyAction::NewSessionBackspace,
        &["backspace"],
    ),
    (
        "new_session.delete",
        KeyAction::NewSessionDelete,
        &["delete"],
    ),
    ("new_session.home", KeyAction::NewSessionHome, &["home"]),
    ("new_session.end", KeyAction::NewSessionEnd, &["end"]),
    (
        "clone_target.cancel",
        KeyAction::CloneTargetCancel,
        &["esc"],
    ),
    (
        "clone_target.confirm",
        KeyAction::CloneTargetConfirm,
        &["enter"],
    ),
    (
        "clone_target.next",
        KeyAction::CloneTargetNext,
        &["down", "j"],
    ),
    (
        "clone_target.prev",
        KeyAction::CloneTargetPrev,
        &["up", "k"],
    ),
];

#[derive(Debug, Clone)]
struct KeyBinding {
    code: KeyCode,
    modifiers: KeyModifiers,
    encoded: Option<Vec<u8>>,
}

impl KeyBinding {
    fn parse(input: &str) -> std::result::Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("empty key binding".into());
        }
        let mut parts: Vec<&str> = trimmed
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        let Some(key_part) = parts.pop() else {
            return Err("missing key".into());
        };
        let mut modifiers = KeyModifiers::NONE;
        for modifier in parts {
            match normalize_key_token(modifier).as_str() {
                "ctrl" | "control" => modifiers.insert(KeyModifiers::CONTROL),
                "alt" | "option" => modifiers.insert(KeyModifiers::ALT),
                "shift" => modifiers.insert(KeyModifiers::SHIFT),
                "super" | "cmd" | "command" => modifiers.insert(KeyModifiers::SUPER),
                "meta" => modifiers.insert(KeyModifiers::META),
                "hyper" => modifiers.insert(KeyModifiers::HYPER),
                other => return Err(format!("unknown modifier `{}`", other)),
            }
        }
        let code = parse_key_code(key_part)?;
        let encoded = key_binding_control_bytes(code, modifiers);
        Ok(Self {
            code,
            modifiers,
            encoded,
        })
    }

    fn matches(&self, key: KeyEvent) -> bool {
        if self.code == key.code && self.modifiers == key.modifiers {
            return true;
        }
        if let (KeyCode::Char(expected), KeyCode::Char(actual)) = (self.code, key.code) {
            if expected == actual
                && self.modifiers == KeyModifiers::NONE
                && key.modifiers == KeyModifiers::SHIFT
            {
                return true;
            }
        }
        self.encoded
            .as_deref()
            .zip(key_event_control_bytes(key))
            .is_some_and(|(expected, actual)| expected == actual.as_slice())
    }

    fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("Alt".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("Shift".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SUPER) {
            parts.push("Super".to_string());
        }
        if self.modifiers.contains(KeyModifiers::META) {
            parts.push("Meta".to_string());
        }
        if self.modifiers.contains(KeyModifiers::HYPER) {
            parts.push("Hyper".to_string());
        }
        parts.push(key_code_display(self.code));
        parts.join("+")
    }
}

fn key_binding_control_bytes(code: KeyCode, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    if !modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let KeyCode::Char(c) = code else {
        return None;
    };
    let bytes = key_event_to_bytes(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))?;
    if bytes == [0x1b] {
        return None;
    }
    Some(bytes)
}

fn key_event_control_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let Some(bytes) = key_event_to_bytes(key) {
            if bytes != [0x1b] {
                return Some(bytes);
            }
        }
    }
    if let KeyCode::Char(c) = key.code {
        if c.is_control() && c != '\u{1b}' {
            let mut buf = [0; 4];
            return Some(c.encode_utf8(&mut buf).as_bytes().to_vec());
        }
    }
    None
}

#[derive(Debug, Clone)]
struct KeyBindings {
    bindings: HashMap<KeyAction, Vec<KeyBinding>>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        for (_, action, defaults) in DEFAULT_KEYBINDINGS {
            bindings.insert(
                *action,
                defaults
                    .iter()
                    .filter_map(|binding| KeyBinding::parse(binding).ok())
                    .collect(),
            );
        }
        Self { bindings }
    }
}

impl KeyBindings {
    fn load_with_mtime(path: Option<&Path>) -> (Self, Option<SystemTime>) {
        match Self::read_from_path(path) {
            Ok(keybindings) => keybindings,
            Err(e) => {
                debug_log(
                    "keybindings_load_failed",
                    serde_json::json!({
                        "error": e,
                    }),
                );
                (Self::default(), Self::file_mtime(path).ok().flatten())
            }
        }
    }

    fn read_from_path(
        path: Option<&Path>,
    ) -> std::result::Result<(Self, Option<SystemTime>), String> {
        let modified = Self::ensure_file_or_mtime(path)?;
        let keybindings = Self::read_for_observed_mtime(path, modified)?;
        Ok((keybindings, modified))
    }

    fn read_for_observed_mtime(
        path: Option<&Path>,
        modified: Option<SystemTime>,
    ) -> std::result::Result<Self, String> {
        let mut keybindings = Self::default();
        let Some(path) = path else {
            return Ok(keybindings);
        };
        if modified.is_none() {
            return Ok(keybindings);
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(keybindings),
            Err(e) => {
                return Err(format!("read {} failed: {}", path.display(), e));
            }
        };
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(mut value) => {
                let migrated = migrate_legacy_keybinding_defaults(&mut value);
                keybindings.apply_json(&value);
                if migrated {
                    persist_migrated_keybinding_file(path, &value);
                }
            }
            Err(e) => return Err(format!("parse {} failed: {}", path.display(), e)),
        }
        Ok(keybindings)
    }

    fn file_mtime(path: Option<&Path>) -> std::result::Result<Option<SystemTime>, String> {
        let Some(path) = path else {
            return Ok(None);
        };
        match fs::metadata(path) {
            Ok(metadata) => metadata
                .modified()
                .map(Some)
                .map_err(|e| format!("stat {} modified time failed: {}", path.display(), e)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("stat {} failed: {}", path.display(), e)),
        }
    }

    fn ensure_file_or_mtime(
        path: Option<&Path>,
    ) -> std::result::Result<Option<SystemTime>, String> {
        Self::ensure_file_or_mtime_with_created(path).map(|(modified, _)| modified)
    }

    fn ensure_file_or_mtime_with_created(
        path: Option<&Path>,
    ) -> std::result::Result<(Option<SystemTime>, bool), String> {
        let Some(path) = path else {
            return Ok((None, false));
        };
        if let Some(modified) = Self::file_mtime(Some(path))? {
            return Ok((Some(modified), false));
        }
        Self::write_default_file(path)?;
        Self::file_mtime(Some(path)).map(|modified| (modified, true))
    }

    fn write_default_file(path: &Path) -> std::result::Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create {} failed: {}", parent.display(), e))?;
        }
        let content = serde_json::to_string_pretty(&Self::default_config_json())
            .map_err(|e| format!("serialize default keybindings failed: {}", e))?
            + "\n";
        let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(()),
            Err(e) => return Err(format!("create {} failed: {}", path.display(), e)),
        };
        file.write_all(content.as_bytes())
            .map_err(|e| format!("write {} failed: {}", path.display(), e))
    }

    fn default_config_json() -> serde_json::Value {
        let mut root = serde_json::Map::new();
        for (path, _, defaults) in DEFAULT_KEYBINDINGS {
            let value = serde_json::Value::Array(
                defaults
                    .iter()
                    .map(|binding| serde_json::Value::String((*binding).to_string()))
                    .collect(),
            );
            let Some((group, action)) = path.split_once('.') else {
                root.insert((*path).to_string(), value);
                continue;
            };
            let group_value = root
                .entry(group.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let serde_json::Value::Object(group_map) = group_value {
                group_map.insert(action.to_string(), value);
            }
        }
        serde_json::Value::Object(root)
    }

    fn apply_json(&mut self, value: &serde_json::Value) {
        for (path, action, _) in DEFAULT_KEYBINDINGS {
            let Some(raw_bindings) = keybinding_json_value(value, path) else {
                continue;
            };
            match parse_keybinding_json_list(raw_bindings) {
                Ok(bindings) => {
                    self.bindings.insert(*action, bindings);
                }
                Err(e) => debug_log(
                    "keybinding_action_parse_failed",
                    serde_json::json!({
                        "action": path,
                        "error": e,
                    }),
                ),
            }
        }
    }

    fn matches(&self, action: KeyAction, key: KeyEvent) -> bool {
        self.bindings
            .get(&action)
            .is_some_and(|bindings| bindings.iter().any(|binding| binding.matches(key)))
    }

    fn labels(&self, action: KeyAction, limit: usize) -> Vec<String> {
        self.bindings
            .get(&action)
            .into_iter()
            .flat_map(|bindings| bindings.iter())
            .map(KeyBinding::label)
            .take(limit)
            .collect()
    }

    fn help(&self, action: KeyAction, fallback: &str) -> String {
        let labels = self.labels(action, 2);
        if labels.is_empty() {
            fallback.to_string()
        } else {
            labels.join("/")
        }
    }

    fn help_pair(
        &self,
        previous: KeyAction,
        next: KeyAction,
        fallback_previous: &str,
        fallback_next: &str,
    ) -> String {
        format!(
            "{}/{}",
            self.help(previous, fallback_previous),
            self.help(next, fallback_next)
        )
    }
}

fn keybinding_json_value<'a>(
    root: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    if let Some(value) = root.get(path) {
        return Some(value);
    }
    let mut current = root;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn parse_keybinding_json_list(
    value: &serde_json::Value,
) -> std::result::Result<Vec<KeyBinding>, String> {
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::String(binding) => Ok(vec![KeyBinding::parse(binding)?]),
        serde_json::Value::Array(bindings) => bindings
            .iter()
            .map(|binding| match binding {
                serde_json::Value::String(value) => KeyBinding::parse(value),
                other => Err(format!("expected string key binding, got {}", other)),
            })
            .collect(),
        other => Err(format!("expected string, array, or null, got {}", other)),
    }
}

fn migrate_legacy_keybinding_defaults(root: &mut serde_json::Value) -> bool {
    let mut migrated = false;
    migrated |= migrate_generated_keybinding_value(
        flat_keybinding_json_value_mut(root, "agent.scroll_page_up"),
        &[
            LEGACY_AGENT_SCROLL_PAGE_UP_DEFAULTS,
            PREVIOUS_AGENT_SCROLL_PAGE_UP_DEFAULTS,
        ],
        AGENT_SCROLL_PAGE_UP_DEFAULTS,
    );
    migrated |= migrate_generated_keybinding_value(
        nested_keybinding_json_value_mut(root, &["agent", "scroll_page_up"]),
        &[
            LEGACY_AGENT_SCROLL_PAGE_UP_DEFAULTS,
            PREVIOUS_AGENT_SCROLL_PAGE_UP_DEFAULTS,
        ],
        AGENT_SCROLL_PAGE_UP_DEFAULTS,
    );
    migrated |= migrate_generated_keybinding_value(
        flat_keybinding_json_value_mut(root, "agent.scroll_page_down"),
        &[
            LEGACY_AGENT_SCROLL_PAGE_DOWN_DEFAULTS,
            PREVIOUS_AGENT_SCROLL_PAGE_DOWN_DEFAULTS,
        ],
        AGENT_SCROLL_PAGE_DOWN_DEFAULTS,
    );
    migrated |= migrate_generated_keybinding_value(
        nested_keybinding_json_value_mut(root, &["agent", "scroll_page_down"]),
        &[
            LEGACY_AGENT_SCROLL_PAGE_DOWN_DEFAULTS,
            PREVIOUS_AGENT_SCROLL_PAGE_DOWN_DEFAULTS,
        ],
        AGENT_SCROLL_PAGE_DOWN_DEFAULTS,
    );
    migrated
}

fn flat_keybinding_json_value_mut<'a>(
    root: &'a mut serde_json::Value,
    path: &str,
) -> Option<&'a mut serde_json::Value> {
    root.as_object_mut()?.get_mut(path)
}

fn nested_keybinding_json_value_mut<'a>(
    root: &'a mut serde_json::Value,
    path: &[&str],
) -> Option<&'a mut serde_json::Value> {
    let mut current = root;
    for part in path {
        current = current.as_object_mut()?.get_mut(*part)?;
    }
    Some(current)
}

fn migrate_generated_keybinding_value(
    value: Option<&mut serde_json::Value>,
    generated_values: &[&[&str]],
    current: &[&str],
) -> bool {
    let Some(value) = value else {
        return false;
    };
    if !generated_values
        .iter()
        .any(|generated| keybinding_json_list_equals(value, generated))
    {
        return false;
    }
    *value = keybinding_string_array_value(current);
    true
}

fn keybinding_json_list_equals(value: &serde_json::Value, expected: &[&str]) -> bool {
    match value {
        serde_json::Value::String(binding) => expected.len() == 1 && binding == expected[0],
        serde_json::Value::Array(bindings) => {
            bindings.len() == expected.len()
                && bindings
                    .iter()
                    .zip(expected.iter())
                    .all(|(binding, expected)| binding.as_str() == Some(*expected))
        }
        _ => false,
    }
}

fn keybinding_string_array_value(bindings: &[&str]) -> serde_json::Value {
    serde_json::Value::Array(
        bindings
            .iter()
            .map(|binding| serde_json::Value::String((*binding).to_string()))
            .collect(),
    )
}

fn persist_migrated_keybinding_file(path: &Path, value: &serde_json::Value) {
    let content = match serde_json::to_string_pretty(value) {
        Ok(content) => content + "\n",
        Err(e) => {
            debug_log(
                "keybindings_migration_serialize_failed",
                serde_json::json!({
                    "path": path.display().to_string(),
                    "error": e.to_string(),
                }),
            );
            return;
        }
    };
    if let Err(e) = fs::write(path, content) {
        debug_log(
            "keybindings_migration_write_failed",
            serde_json::json!({
                "path": path.display().to_string(),
                "error": e.to_string(),
            }),
        );
    }
}

fn parse_key_code(input: &str) -> std::result::Result<KeyCode, String> {
    let normalized = normalize_key_token(input);
    let code = match normalized.as_str() {
        "backspace" | "bksp" => KeyCode::Backspace,
        "enter" | "return" => KeyCode::Enter,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "slash" => KeyCode::Char('/'),
        "backslash" => KeyCode::Char('\\'),
        "comma" => KeyCode::Char(','),
        "dot" | "period" => KeyCode::Char('.'),
        "plus" => KeyCode::Char('+'),
        "minus" | "dash" => KeyCode::Char('-'),
        "semicolon" => KeyCode::Char(';'),
        "colon" => KeyCode::Char(':'),
        "quote" => KeyCode::Char('\''),
        "doublequote" => KeyCode::Char('"'),
        "backtick" | "grave" => KeyCode::Char('`'),
        "openbracket" | "lbracket" => KeyCode::Char('['),
        "closebracket" | "rbracket" => KeyCode::Char(']'),
        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            let number = normalized[1..]
                .parse::<u8>()
                .map_err(|_| format!("invalid function key `{}`", input))?;
            KeyCode::F(number)
        }
        _ => {
            let mut chars = input.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return Err(format!("unknown key `{}`", input)),
            }
        }
    };
    Ok(code)
}

fn normalize_key_token(input: &str) -> String {
    input
        .trim()
        .chars()
        .filter(|c| *c != '_' && *c != '-' && !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn key_code_display(code: KeyCode) -> String {
    match code {
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::F(n) => format!("F{}", n),
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Null => "Null".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::CapsLock => "CapsLock".to_string(),
        KeyCode::ScrollLock => "ScrollLock".to_string(),
        KeyCode::NumLock => "NumLock".to_string(),
        KeyCode::PrintScreen => "PrintScreen".to_string(),
        KeyCode::Pause => "Pause".to_string(),
        KeyCode::Menu => "Menu".to_string(),
        KeyCode::KeypadBegin => "KeypadBegin".to_string(),
        KeyCode::Media(media) => format!("{:?}", media),
        KeyCode::Modifier(modifier) => format!("{:?}", modifier),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ProviderFilter {
    All,
    One(Provider),
}

impl ProviderFilter {
    fn cycle(self) -> Self {
        match self {
            ProviderFilter::All => ProviderFilter::One(Provider::Claude),
            ProviderFilter::One(Provider::Claude) => ProviderFilter::One(Provider::Codex),
            ProviderFilter::One(Provider::Codex) => ProviderFilter::One(Provider::OpenCode),
            ProviderFilter::One(Provider::OpenCode) => ProviderFilter::All,
        }
    }
    fn label(self) -> &'static str {
        match self {
            ProviderFilter::All => "ALL",
            ProviderFilter::One(Provider::Claude) => "claude",
            ProviderFilter::One(Provider::Codex) => "codex",
            ProviderFilter::One(Provider::OpenCode) => "opencode",
        }
    }
    fn matches(self, p: Provider) -> bool {
        match self {
            ProviderFilter::All => true,
            ProviderFilter::One(q) => p == q,
        }
    }
}

const CLONE_PROVIDER_OPTIONS: [Provider; 3] =
    [Provider::Claude, Provider::Codex, Provider::OpenCode];

fn clone_provider_at(index: usize) -> Provider {
    CLONE_PROVIDER_OPTIONS[index % CLONE_PROVIDER_OPTIONS.len()]
}

#[cfg(test)]
fn clone_provider_default_index(source: Provider) -> usize {
    CLONE_PROVIDER_OPTIONS
        .iter()
        .position(|provider| *provider == source)
        .unwrap_or(0)
}

fn move_clone_provider_index(index: usize, delta: i32) -> usize {
    (index as i32 + delta).rem_euclid(CLONE_PROVIDER_OPTIONS.len() as i32) as usize
}

fn agent_launch_mode_at(index: usize) -> AgentLaunchMode {
    AGENT_LAUNCH_MODE_OPTIONS[index % AGENT_LAUNCH_MODE_OPTIONS.len()]
}

fn move_agent_launch_mode_index(index: usize, delta: i32) -> usize {
    (index as i32 + delta).rem_euclid(AGENT_LAUNCH_MODE_OPTIONS.len() as i32) as usize
}

fn new_session_field_count(kind: NewSessionKind) -> usize {
    match kind {
        NewSessionKind::Terminal => NEW_SESSION_FIELD_PROVIDER,
        NewSessionKind::CodingAgent => NEW_SESSION_FIELD_COUNT,
    }
}

fn clamp_new_session_field(index: usize, kind: NewSessionKind) -> usize {
    index.min(new_session_field_count(kind).saturating_sub(1))
}

fn move_new_session_field(index: usize, kind: NewSessionKind, delta: i32) -> usize {
    let count = new_session_field_count(kind);
    (index as i32 + delta).rem_euclid(count as i32) as usize
}

fn move_new_session_kind(kind: NewSessionKind, delta: i32) -> NewSessionKind {
    let index = match kind {
        NewSessionKind::Terminal => 0,
        NewSessionKind::CodingAgent => 1,
    };
    match (index + delta).rem_euclid(2) {
        0 => NewSessionKind::Terminal,
        _ => NewSessionKind::CodingAgent,
    }
}

fn move_provider_in_options(provider: Provider, delta: i32, options: &[Provider]) -> Provider {
    if options.is_empty() {
        return provider;
    }
    let Some(index) = options.iter().position(|candidate| *candidate == provider) else {
        return options[0];
    };
    let next = (index as i32 + delta).rem_euclid(options.len() as i32) as usize;
    options[next]
}

fn available_agent_provider_options(agent_programs: &AgentProgramSettings) -> Vec<Provider> {
    CLONE_PROVIDER_OPTIONS
        .iter()
        .copied()
        .filter(|provider| agent_provider_available(*provider, agent_programs))
        .collect()
}

fn normalize_agent_provider_selection(
    provider: Provider,
    options: &[Provider],
) -> Option<Provider> {
    if options.contains(&provider) {
        Some(provider)
    } else {
        options.first().copied()
    }
}

fn agent_provider_available(provider: Provider, agent_programs: &AgentProgramSettings) -> bool {
    resolve_agent_program_for_provider(provider, agent_programs).is_some()
}

fn move_launch_mode(launch_mode: AgentLaunchMode, delta: i32) -> AgentLaunchMode {
    let index = AGENT_LAUNCH_MODE_OPTIONS
        .iter()
        .position(|mode| *mode == launch_mode)
        .unwrap_or(0);
    agent_launch_mode_at(move_agent_launch_mode_index(index, delta))
}

fn clamped_selection_index(len: usize, selected: Option<usize>, delta: i32) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let max = len.saturating_sub(1) as i32;
    let current = selected.unwrap_or(0).min(len.saturating_sub(1)) as i32;
    Some((current + delta).clamp(0, max) as usize)
}

fn selection_index_after_removed_row(
    len_after: usize,
    removed_index: Option<usize>,
) -> Option<usize> {
    if len_after == 0 {
        return None;
    }
    Some(removed_index.unwrap_or(0).min(len_after.saturating_sub(1)))
}

fn next_agent_candidate_index(len: usize, current: usize, delta: i32, wrap: bool) -> usize {
    if len == 0 {
        return 0;
    }
    let current = current.min(len.saturating_sub(1)) as i32;
    if wrap {
        (current + delta).rem_euclid(len as i32) as usize
    } else {
        (current + delta).clamp(0, len.saturating_sub(1) as i32) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    Sessions,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SessionViewMode {
    List,
    Tree,
}

impl SessionViewMode {
    fn toggle(self) -> Self {
        match self {
            SessionViewMode::List => SessionViewMode::Tree,
            SessionViewMode::Tree => SessionViewMode::List,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SessionViewMode::List => "list",
            SessionViewMode::Tree => "tree",
        }
    }
}

#[derive(Debug, Clone)]
enum InputMode {
    Normal,
    Filter {
        draft: String,
        cursor: usize,
    },
    Confirm {
        prompt: String,
        action: PendingAction,
    },
    AgentLaunch {
        source: SessionInfo,
        selected: usize,
    },
    NewSession {
        selected: usize,
        kind: NewSessionKind,
        cwd: String,
        cwd_cursor: usize,
        provider: Provider,
        provider_options: Vec<Provider>,
        launch_mode: AgentLaunchMode,
    },
    CloneTarget {
        source: SessionInfo,
        selected: usize,
    },
    TitleEdit {
        source: SessionInfo,
        draft: String,
        cursor: usize,
    },
}

#[derive(Debug, Clone)]
enum PendingAction {
    Delete {
        info: SessionInfo,
        removed_index: Option<usize>,
    },
    CreateMissingLaunchCwd {
        info: SessionInfo,
        path: PathBuf,
        cols: u16,
        rows: u16,
        launch_mode: AgentLaunchMode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentLaunchMode {
    Normal,
    SkipPermissions,
}

impl AgentLaunchMode {
    fn as_str(self) -> &'static str {
        match self {
            AgentLaunchMode::Normal => "normal",
            AgentLaunchMode::SkipPermissions => "skip_permissions",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(AgentLaunchMode::Normal),
            "skip_permissions" => Some(AgentLaunchMode::SkipPermissions),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            AgentLaunchMode::Normal => "Normal",
            AgentLaunchMode::SkipPermissions => "Skip permissions (danger)",
        }
    }
}

const AGENT_LAUNCH_MODE_OPTIONS: [AgentLaunchMode; 2] =
    [AgentLaunchMode::Normal, AgentLaunchMode::SkipPermissions];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewSessionKind {
    Terminal,
    CodingAgent,
}

impl NewSessionKind {
    fn label(self) -> &'static str {
        match self {
            NewSessionKind::Terminal => "Terminal",
            NewSessionKind::CodingAgent => "Coding agent",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            NewSessionKind::Terminal => "terminal",
            NewSessionKind::CodingAgent => "coding_agent",
        }
    }
}

const NEW_SESSION_FIELD_COUNT: usize = 4;
const NEW_SESSION_FIELD_KIND: usize = 0;
const NEW_SESSION_FIELD_CWD: usize = 1;
const NEW_SESSION_FIELD_PROVIDER: usize = 2;
const NEW_SESSION_FIELD_PERMISSIONS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PreviewKey {
    provider: Provider,
    session_id: String,
    mode: Mode,
}

impl PreviewKey {
    fn new(info: &SessionInfo, mode: Mode) -> Self {
        Self {
            provider: info.provider,
            session_id: info.session_id.clone(),
            mode,
        }
    }
}

#[derive(Debug)]
struct PreviewEntry {
    text: String,
    wrap_width: usize,
    lines: Vec<String>,
}

impl PreviewEntry {
    fn new(text: String, wrap_width: usize, lines: Vec<String>) -> Self {
        Self {
            text,
            wrap_width,
            lines,
        }
    }
}

struct PreviewRequest {
    seq: u64,
    key: PreviewKey,
    info: SessionInfo,
    width: usize,
}

struct PreviewResult {
    seq: u64,
    key: PreviewKey,
    text: String,
    wrap_width: usize,
    lines: Vec<String>,
}

struct SearchPending {
    seq: u64,
    query: String,
    started_at: Instant,
}

struct SearchWorkerResult {
    seq: u64,
    query: String,
    hits: std::result::Result<Vec<session::SearchHit>, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AgentKey {
    provider: Provider,
    session_id: String,
}

impl AgentKey {
    fn new(info: &SessionInfo) -> Self {
        Self {
            provider: info.provider,
            session_id: info.session_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentListState {
    Idle,
    Live { activity: AgentActivity },
    Attached { mine: bool, activity: AgentActivity },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentActivity {
    Busy,
    Quiet,
}

impl AgentActivity {
    fn label(self) -> &'static str {
        match self {
            AgentActivity::Busy => "busy",
            AgentActivity::Quiet => "quiet",
        }
    }

    fn style(self) -> Style {
        match self {
            AgentActivity::Busy => Style::default()
                .fg(THEME_POSITIVE)
                .add_modifier(Modifier::BOLD),
            AgentActivity::Quiet => Style::default().fg(THEME_ACCENT),
        }
    }

    fn combine(self, other: Self) -> Self {
        if matches!(self, AgentActivity::Busy) || matches!(other, AgentActivity::Busy) {
            AgentActivity::Busy
        } else {
            AgentActivity::Quiet
        }
    }
}

impl AgentListState {
    fn label(self) -> &'static str {
        match self {
            AgentListState::Idle => "idle",
            AgentListState::Live { activity } | AgentListState::Attached { activity, .. } => {
                activity.label()
            }
        }
    }

    fn style(self) -> Style {
        match self {
            AgentListState::Idle => Style::default().fg(THEME_FG_DIM),
            AgentListState::Live { activity } | AgentListState::Attached { activity, .. } => {
                activity.style()
            }
        }
    }

    fn activity(self) -> AgentActivity {
        match self {
            AgentListState::Idle => AgentActivity::Quiet,
            AgentListState::Live { activity } | AgentListState::Attached { activity, .. } => {
                activity
            }
        }
    }

    fn attached_mine(self) -> Self {
        AgentListState::Attached {
            mine: true,
            activity: self.activity(),
        }
    }
}

fn is_switchable_agent_state(state: AgentListState) -> bool {
    matches!(
        state,
        AgentListState::Live { .. } | AgentListState::Attached { mine: true, .. }
    )
}

fn agent_list_state_debug_value(state: AgentListState) -> serde_json::Value {
    match state {
        AgentListState::Idle => serde_json::json!({
            "state": "idle",
            "activity": AgentActivity::Quiet.label(),
            "mine": serde_json::Value::Null,
            "label": "idle",
        }),
        AgentListState::Live { activity } => serde_json::json!({
            "state": "live",
            "activity": activity.label(),
            "mine": serde_json::Value::Null,
            "label": activity.label(),
        }),
        AgentListState::Attached { mine, activity } => serde_json::json!({
            "state": "attached",
            "activity": activity.label(),
            "mine": mine,
            "label": activity.label(),
        }),
    }
}

fn optional_agent_list_state_debug_value(state: Option<AgentListState>) -> serde_json::Value {
    state
        .map(agent_list_state_debug_value)
        .unwrap_or(serde_json::Value::Null)
}

fn agent_key_debug_value(key: &AgentKey) -> serde_json::Value {
    serde_json::json!({
        "provider": key.provider.as_str(),
        "session_id": &key.session_id,
    })
}

fn agent_info_kind(info: &SessionInfo) -> &'static str {
    if is_shell_session_info(info) {
        "shell"
    } else if is_new_agent_session_info(info) {
        "new_agent"
    } else {
        "stored_session"
    }
}

fn session_info_debug_value(info: &SessionInfo) -> serde_json::Value {
    serde_json::json!({
        "provider": info.provider.as_str(),
        "session_id": &info.session_id,
        "cwd": &info.cwd,
        "source": info.source.display().to_string(),
        "kind": agent_info_kind(info),
        "updated_at_epoch_s": info.updated_at_epoch_s,
        "title": info.title.as_deref(),
    })
}

fn agent_state_entries_debug_value(
    states: &HashMap<AgentKey, AgentListState>,
) -> Vec<serde_json::Value> {
    let mut entries: Vec<_> = states
        .iter()
        .map(|(key, state)| {
            serde_json::json!({
                "key": agent_key_debug_value(key),
                "state": agent_list_state_debug_value(*state),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.to_string().cmp(&b.to_string()));
    entries
}

fn agent_alias_entries_debug_value(
    aliases: &HashMap<AgentKey, AgentKey>,
) -> Vec<serde_json::Value> {
    let mut entries: Vec<_> = aliases
        .iter()
        .map(|(synthetic, backing)| {
            serde_json::json!({
                "synthetic": agent_key_debug_value(synthetic),
                "backing": agent_key_debug_value(backing),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.to_string().cmp(&b.to_string()));
    entries
}

struct NewAgentBackingSession {
    key: AgentKey,
    rollout_path: PathBuf,
}

struct AgentSession {
    info: SessionInfo,
    spec: AgentLaunchSpec,
    launch_mode: AgentLaunchMode,
    parser: vt100::Parser,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send>,
    writer: Box<dyn Write + Send>,
    output_rx: Receiver<Vec<u8>>,
    pty_log: Option<fs::File>,
    screen_history: ScreenHistory,
    pty_size: PtySize,
    screen_hash: u64,
    last_screen_change_epoch_ms: u64,
    last_output_epoch_ms: u64,
    last_input_epoch_ms: u64,
    last_meta_activity_write_epoch_ms: u64,
    debug_drain_logs: u32,
    terminal_response_scan_tail: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
struct ScreenHistory {
    lines: VecDeque<String>,
    last_snapshot: Vec<String>,
}

impl ScreenHistory {
    fn capture(&mut self, parser: &mut vt100::Parser) {
        let lines = parser_visible_plain_lines(parser, 0)
            .into_iter()
            .map(|line| line.trim_end_matches(' ').to_string())
            .collect::<Vec<_>>();
        if lines.is_empty() || lines.iter().all(|line| line.trim().is_empty()) {
            return;
        }
        if self.last_snapshot == lines {
            return;
        }
        self.last_snapshot = lines.clone();
        for line in lines {
            self.lines.push_back(line);
        }
        while self.lines.len() > AGENT_SCROLLBACK_LINES {
            self.lines.pop_front();
        }
    }

    fn len(&self) -> usize {
        self.lines.len()
    }

    fn all_lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }

    fn lines_before_visible(&self, visible_lines: &[String]) -> Vec<String> {
        let mut lines = self.all_lines();
        self.trim_trailing_current_snapshot(&mut lines, visible_lines);
        lines
    }

    fn trim_trailing_current_snapshot(&self, lines: &mut Vec<String>, visible_lines: &[String]) {
        if self.last_snapshot.is_empty() && self.lines.is_empty() {
            return;
        }
        if !self.last_snapshot.is_empty()
            && lines.len() >= self.last_snapshot.len()
            && lines[lines.len() - self.last_snapshot.len()..] == self.last_snapshot[..]
        {
            lines.truncate(lines.len() - self.last_snapshot.len());
            return;
        }
        let visible_lines = visible_lines
            .iter()
            .map(|line| line.trim_end_matches(' ').to_string())
            .collect::<Vec<_>>();
        if !visible_lines.is_empty()
            && lines.len() >= visible_lines.len()
            && lines[lines.len() - visible_lines.len()..] == visible_lines[..]
        {
            lines.truncate(lines.len() - visible_lines.len());
        }
    }

    fn max_scroll_offset(&self, visible_rows: usize) -> usize {
        self.lines.len().saturating_sub(visible_rows.max(1))
    }

    fn visible_lines(&self, offset: usize, visible_rows: usize) -> Vec<String> {
        let visible_rows = visible_rows.max(1);
        let end = self.lines.len().saturating_sub(offset);
        let start = end.saturating_sub(visible_rows);
        self.lines
            .iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .cloned()
            .collect()
    }
}

impl AgentSession {
    fn spawn(
        info: SessionInfo,
        cols: u16,
        rows: u16,
        launch_mode: AgentLaunchMode,
        agent_programs: &AgentProgramSettings,
    ) -> Result<Self> {
        let spec = agent_launch_spec_with_programs(&info, launch_mode, agent_programs);
        Self::spawn_with_spec(info, spec, cols, rows, launch_mode)
    }

    fn spawn_with_spec(
        info: SessionInfo,
        spec: AgentLaunchSpec,
        cols: u16,
        rows: u16,
        launch_mode: AgentLaunchMode,
    ) -> Result<Self> {
        if let Some(cwd) = &spec.cwd {
            validate_agent_launch_cwd(cwd)?;
        }
        let pty_size = agent_pty_size(cols, rows);
        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(pty_size)?;
        let mut command = agent_command_builder(&spec);
        if let Some(cwd) = &spec.cwd {
            command.cwd(cwd.as_os_str());
        }
        let debug_argv = debug_command_argv(&command);
        debug_log(
            "agent_spawn_command_prepared",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "command": spec.command_line(),
                "launch_mode": launch_mode.as_str(),
                "argv": debug_argv,
                "env": &spec.env,
                "cwd": spec.cwd.as_ref().map(|path| path.display().to_string()),
                "pty_cols": pty_size.cols,
                "pty_rows": pty_size.rows,
            }),
        );
        let child = pair.slave.spawn_command(command)?;
        let child_pid = child.process_id();
        debug_log(
            "agent_spawn_command_started",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "command": spec.command_line(),
                "child_pid": child_pid,
            }),
        );
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>();
        let reader_provider = info.provider;
        let reader_session_id = info.session_id.clone();
        let _ = thread::Builder::new()
            .name(format!(
                "cokacmux-agent-{}-{}",
                info.provider.as_str(),
                truncate_width(&info.session_id, 8)
            ))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                let mut read_count = 0u32;
                let mut total_bytes = 0usize;
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            debug_log(
                                "agent_pty_eof",
                                serde_json::json!({
                                    "provider": reader_provider.as_str(),
                                    "session_id": &reader_session_id,
                                    "reads": read_count,
                                    "total_bytes": total_bytes,
                                }),
                            );
                            break;
                        }
                        Ok(n) => {
                            read_count = read_count.saturating_add(1);
                            total_bytes = total_bytes.saturating_add(n);
                            if read_count <= 20 {
                                debug_log(
                                    "agent_pty_read",
                                    serde_json::json!({
                                        "provider": reader_provider.as_str(),
                                        "session_id": &reader_session_id,
                                        "read": read_count,
                                        "len": n,
                                        "total_bytes": total_bytes,
                                        "sample": debug_bytes_sample(&buf[..n], 512),
                                    }),
                                );
                            }
                            if output_tx.send(buf[..n].to_vec()).is_err() {
                                debug_log(
                                    "agent_pty_receiver_closed",
                                    serde_json::json!({
                                        "provider": reader_provider.as_str(),
                                        "session_id": &reader_session_id,
                                        "reads": read_count,
                                        "total_bytes": total_bytes,
                                    }),
                                );
                                break;
                            }
                        }
                        Err(e) => {
                            debug_log(
                                "agent_pty_read_error",
                                serde_json::json!({
                                    "provider": reader_provider.as_str(),
                                    "session_id": &reader_session_id,
                                    "error": e.to_string(),
                                    "reads": read_count,
                                    "total_bytes": total_bytes,
                                }),
                            );
                            break;
                        }
                    }
                }
            });

        let pty_log_path = agent_pty_log_path(&AgentKey::new(&info)).ok();
        let parser = vt100::Parser::new(pty_size.rows, pty_size.cols, AGENT_SCROLLBACK_LINES);
        let pty_log = pty_log_path
            .as_ref()
            .and_then(|path| open_agent_pty_log_for_new_run(path, &info));
        let screen_history = ScreenHistory::default();
        let screen_hash = screen_activity_hash(parser.screen());
        let now_ms = current_epoch_ms();

        Ok(Self {
            info,
            spec,
            launch_mode,
            parser,
            master: pair.master,
            child,
            writer,
            output_rx,
            pty_log,
            screen_history,
            pty_size,
            screen_hash,
            last_screen_change_epoch_ms: 0,
            last_output_epoch_ms: 0,
            last_input_epoch_ms: 0,
            last_meta_activity_write_epoch_ms: now_ms,
            debug_drain_logs: 0,
            terminal_response_scan_tail: Vec::new(),
        })
    }

    fn drain_output_chunks(&mut self) -> (Vec<Vec<u8>>, bool) {
        let mut chunks = Vec::new();
        let mut activity_changed = false;
        for _ in 0..AGENT_OUTPUT_POLL_LIMIT {
            match self.output_rx.try_recv() {
                Ok(bytes) => {
                    self.last_output_epoch_ms = current_epoch_ms();
                    activity_changed = true;
                    self.append_pty_log(&bytes);
                    if process_parser_output(
                        &mut self.parser,
                        &bytes,
                        &mut self.screen_hash,
                        Some(&mut self.screen_history),
                    ) {
                        self.last_screen_change_epoch_ms = current_epoch_ms();
                        activity_changed = true;
                    }
                    let mut terminal_response_scan = self.terminal_response_scan_tail.clone();
                    let previous_scan_len = terminal_response_scan.len();
                    terminal_response_scan.extend_from_slice(&bytes);
                    let terminal_response = terminal_response_for_combined_output(
                        self.parser.screen(),
                        &terminal_response_scan,
                        previous_scan_len,
                    );
                    let keep_start = terminal_response_scan
                        .len()
                        .saturating_sub(TERMINAL_RESPONSE_SCAN_TAIL_BYTES);
                    self.terminal_response_scan_tail =
                        terminal_response_scan[keep_start..].to_vec();

                    if let Some(response) = terminal_response {
                        match self.write_to_agent(&response) {
                            Ok(()) => {
                                debug_log(
                                    "agent_terminal_response_sent",
                                    serde_json::json!({
                                        "provider": self.info.provider.as_str(),
                                        "session_id": &self.info.session_id,
                                        "len": response.len(),
                                        "sample": debug_bytes_sample(&response, 128),
                                    }),
                                );
                            }
                            Err(e) => {
                                debug_log(
                                    "agent_terminal_response_failed",
                                    serde_json::json!({
                                        "provider": self.info.provider.as_str(),
                                        "session_id": &self.info.session_id,
                                        "error": e.to_string(),
                                    }),
                                );
                            }
                        }
                    }
                    chunks.push(bytes);
                }
                Err(_) => break,
            }
        }
        if !chunks.is_empty() && self.debug_drain_logs < 20 {
            let total_bytes: usize = chunks.iter().map(Vec::len).sum();
            self.debug_drain_logs = self.debug_drain_logs.saturating_add(1);
            debug_log(
                "daemon_agent_output_drained",
                serde_json::json!({
                    "provider": self.info.provider.as_str(),
                    "session_id": &self.info.session_id,
                    "event": self.debug_drain_logs,
                    "chunk_count": chunks.len(),
                    "total_bytes": total_bytes,
                    "screen_changed": activity_changed,
                    "screen_history_lines": self.screen_history.len(),
                    "visible": screen_has_visible_content(self.parser.screen()),
                    "last_screen_change_epoch_ms": self.last_screen_change_epoch_ms,
                    "last_output_epoch_ms": self.last_output_epoch_ms,
                    "preview": debug_screen_preview(self.parser.screen(), 5),
                }),
            );
        }
        (chunks, activity_changed)
    }

    fn append_pty_log(&mut self, bytes: &[u8]) {
        let Some(file) = self.pty_log.as_mut() else {
            return;
        };
        if let Err(e) = file.write_all(bytes) {
            debug_log(
                "agent_pty_log_write_failed",
                serde_json::json!({
                    "provider": self.info.provider.as_str(),
                    "session_id": &self.info.session_id,
                    "error": e.to_string(),
                }),
            );
            self.pty_log = None;
        }
    }

    fn rehydrate_parser_from_pty_log(&mut self) {
        let live_scrollback = parser_max_scrollback(&mut self.parser);
        if screen_has_visible_content(self.parser.screen())
            || live_scrollback > 0
            || self.screen_history.len() > 0
        {
            debug_log(
                "agent_pty_log_rehydrate_skipped_live_parser",
                serde_json::json!({
                    "provider": self.info.provider.as_str(),
                    "session_id": &self.info.session_id,
                    "visible": screen_has_visible_content(self.parser.screen()),
                    "scrollback_max": live_scrollback,
                    "screen_history_lines": self.screen_history.len(),
                }),
            );
            return;
        }
        let Ok(path) = agent_pty_log_path(&AgentKey::new(&self.info)) else {
            return;
        };
        if !path.exists() {
            return;
        }
        let mut parser = vt100::Parser::new(
            self.pty_size.rows,
            self.pty_size.cols,
            AGENT_SCROLLBACK_LINES,
        );
        let mut screen_history = ScreenHistory::default();
        replay_agent_pty_log_with_history(
            &mut parser,
            &path,
            &self.info,
            Some(&mut screen_history),
        );
        self.parser = parser;
        self.screen_history = screen_history;
        self.screen_hash = screen_activity_hash(self.parser.screen());
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let next = agent_pty_size(cols, rows);
        if self.pty_size.rows == next.rows && self.pty_size.cols == next.cols {
            return;
        }
        let _ = self.master.resize(next);
        self.parser.screen_mut().set_size(next.rows, next.cols);
        self.screen_hash = screen_activity_hash(self.parser.screen());
        self.pty_size = next;
    }

    fn send_bytes(&mut self, bytes: &[u8]) {
        self.last_input_epoch_ms = current_epoch_ms();
        let _ = self.write_to_agent(bytes);
    }

    fn write_to_agent(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    fn should_write_activity_meta(&mut self) -> bool {
        let now_ms = current_epoch_ms();
        if now_ms.saturating_sub(self.last_meta_activity_write_epoch_ms)
            < AGENT_ACTIVITY_META_WRITE_INTERVAL_MS
        {
            return false;
        }
        self.last_meta_activity_write_epoch_ms = now_ms;
        true
    }

    /// For shell sessions, update `info.cwd` to reflect the kernel's
    /// current view of the child process's cwd (e.g. after the user typed
    /// `cd ..` inside the shell). No-op for real agent sessions and on
    /// platforms without `/proc/<pid>/cwd`.
    /// Returns true when the shell's cwd changed (e.g. user typed `cd`).
    fn refresh_shell_cwd_from_kernel(&mut self) -> bool {
        if !is_shell_session_info(&self.info) {
            return false;
        }
        let Some(pid) = self.child.process_id() else {
            return false;
        };
        #[cfg(target_os = "linux")]
        {
            let proc_link = format!("/proc/{}/cwd", pid);
            if let Ok(target) = std::fs::read_link(&proc_link) {
                let cwd = target.display().to_string();
                if !cwd.is_empty() && cwd != self.info.cwd {
                    debug_log(
                        "agent_shell_cwd_changed",
                        serde_json::json!({
                            "session_id": &self.info.session_id,
                            "pid": pid,
                            "old": &self.info.cwd,
                            "new": &cwd,
                        }),
                    );
                    self.info.cwd = cwd;
                    return true;
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
        }
        false
    }

    fn screen_snapshot_bytes(&mut self, include_scrollback: bool) -> Vec<u8> {
        parser_snapshot_bytes_with_history(
            &mut self.parser,
            include_scrollback,
            &self.screen_history,
        )
    }
}

#[cfg(test)]
fn parser_snapshot_bytes(parser: &mut vt100::Parser, include_scrollback: bool) -> Vec<u8> {
    parser_snapshot_bytes_with_history(parser, include_scrollback, &ScreenHistory::default())
}

fn parser_snapshot_bytes_with_history(
    parser: &mut vt100::Parser,
    include_scrollback: bool,
    screen_history: &ScreenHistory,
) -> Vec<u8> {
    let original_scrollback = parser.screen().scrollback();
    let mut bytes = Vec::new();

    if include_scrollback {
        let mut replay_lines = parser_scrollback_plain_lines(parser);
        let visible_lines = parser_visible_plain_lines(parser, 0);
        let using_history_fallback = replay_lines.is_empty();
        if replay_lines.is_empty() {
            replay_lines = screen_history.lines_before_visible(&visible_lines);
        } else {
            screen_history.trim_trailing_current_snapshot(&mut replay_lines, &visible_lines);
        }
        if !replay_lines.is_empty() {
            if !using_history_fallback {
                replay_lines.extend(visible_lines);
            }
            append_plain_terminal_lines(&mut bytes, &replay_lines);
        }
    }

    parser.screen_mut().set_scrollback(0);
    let screen = parser.screen();
    bytes.extend_from_slice(b"\x1b[2J\x1b[H");
    bytes.extend_from_slice(&screen.contents_formatted());
    bytes.extend_from_slice(&screen.cursor_state_formatted());
    parser.screen_mut().set_scrollback(original_scrollback);
    bytes
}

fn parser_max_scrollback(parser: &mut vt100::Parser) -> usize {
    let original_scrollback = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(usize::MAX);
    let max_scrollback = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(original_scrollback);
    max_scrollback
}

fn parser_scrollback_plain_lines(parser: &mut vt100::Parser) -> Vec<String> {
    let original_scrollback = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(usize::MAX);
    let max_scrollback = parser.screen().scrollback();
    if max_scrollback == 0 {
        parser.screen_mut().set_scrollback(original_scrollback);
        return Vec::new();
    }

    let (_, cols) = parser.screen().size();
    let mut lines = Vec::with_capacity(max_scrollback);
    for offset in (1..=max_scrollback).rev() {
        parser.screen_mut().set_scrollback(offset);
        let line = parser.screen().rows(0, cols).next().unwrap_or_default();
        lines.push(line);
    }
    parser.screen_mut().set_scrollback(original_scrollback);
    lines
}

fn parser_visible_plain_lines(parser: &mut vt100::Parser, scrollback: usize) -> Vec<String> {
    let original_scrollback = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(scrollback);
    let (rows, cols) = parser.screen().size();
    let lines = parser.screen().rows(0, cols).take(rows as usize).collect();
    parser.screen_mut().set_scrollback(original_scrollback);
    lines
}

fn append_plain_terminal_lines(bytes: &mut Vec<u8>, lines: &[String]) {
    for (index, line) in lines.iter().enumerate() {
        bytes.extend_from_slice(line.trim_end_matches(' ').as_bytes());
        if index + 1 < lines.len() {
            bytes.extend_from_slice(b"\r\n");
        }
    }
}

fn process_parser_output(
    parser: &mut vt100::Parser,
    data: &[u8],
    screen_hash: &mut u64,
    mut screen_history: Option<&mut ScreenHistory>,
) -> bool {
    let mut screen_changed = false;
    let mut segment_start = 0usize;
    let boundaries = terminal_redraw_boundary_positions(data);

    for boundary in boundaries.into_iter().filter(|boundary| *boundary > 0) {
        if boundary > segment_start {
            if process_parser_segment(
                parser,
                &data[segment_start..boundary],
                screen_hash,
                screen_history_option_mut(&mut screen_history),
            ) {
                screen_changed = true;
            }
            segment_start = boundary;
        }
    }

    if segment_start < data.len()
        && process_parser_segment(
            parser,
            &data[segment_start..],
            screen_hash,
            screen_history_option_mut(&mut screen_history),
        )
    {
        screen_changed = true;
    }

    screen_changed
}

fn screen_history_option_mut<'a>(
    screen_history: &'a mut Option<&mut ScreenHistory>,
) -> Option<&'a mut ScreenHistory> {
    match screen_history {
        Some(history) => Some(&mut **history),
        None => None,
    }
}

fn process_parser_segment(
    parser: &mut vt100::Parser,
    data: &[u8],
    screen_hash: &mut u64,
    screen_history: Option<&mut ScreenHistory>,
) -> bool {
    if data.is_empty() {
        return false;
    }
    safe_parser_process(parser, data);
    let next_hash = screen_activity_hash(parser.screen());
    if next_hash == *screen_hash {
        return false;
    }
    *screen_hash = next_hash;
    if let Some(history) = screen_history {
        history.capture(parser);
    }
    true
}

fn terminal_redraw_boundary_positions(data: &[u8]) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut index = 0usize;
    while index + 2 < data.len() {
        if data[index] != 0x1b || data[index + 1] != b'[' {
            index += 1;
            continue;
        }
        let sequence_start = index;
        index += 2;
        while index < data.len() {
            let byte = data[index];
            if (0x40..=0x7e).contains(&byte) {
                let params = &data[sequence_start + 2..index];
                if is_terminal_redraw_boundary_csi(params, byte) {
                    positions.push(sequence_start);
                }
                index += 1;
                break;
            }
            index += 1;
        }
    }
    positions
}

fn is_terminal_redraw_boundary_csi(params: &[u8], final_byte: u8) -> bool {
    match final_byte {
        b'H' | b'f' => params.is_empty() || params == b"1;1" || params == b";",
        b'J' => params.contains(&b'2') || params.contains(&b'3'),
        b'h' | b'l' => params == b"?1049",
        _ => false,
    }
}

#[cfg(test)]
fn replay_agent_pty_log(parser: &mut vt100::Parser, path: &Path, info: &SessionInfo) {
    replay_agent_pty_log_with_history(parser, path, info, None);
}

fn replay_agent_pty_log_with_history(
    parser: &mut vt100::Parser,
    path: &Path,
    info: &SessionInfo,
    mut screen_history: Option<&mut ScreenHistory>,
) {
    let Ok(mut file) = fs::File::open(path) else {
        return;
    };
    let mut buf = [0u8; 8192];
    let mut total_bytes = 0usize;
    let mut screen_hash = screen_activity_hash(parser.screen());
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total_bytes = total_bytes.saturating_add(n);
                let _ = process_parser_output(
                    parser,
                    &buf[..n],
                    &mut screen_hash,
                    screen_history_option_mut(&mut screen_history),
                );
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                debug_log(
                    "agent_pty_log_replay_failed",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "path": path.display().to_string(),
                        "error": e.to_string(),
                    }),
                );
                return;
            }
        }
    }
    debug_log(
        "agent_pty_log_replayed",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &info.session_id,
            "path": path.display().to_string(),
            "bytes": total_bytes,
            "scrollback": parser.screen().scrollback(),
            "screen_history_lines": screen_history.as_ref().map(|history| history.len()),
        }),
    );
}

fn open_agent_pty_log_for_new_run(path: &Path, info: &SessionInfo) -> Option<fs::File> {
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            debug_log(
                "agent_pty_log_dir_failed",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "path": parent.display().to_string(),
                    "error": e.to_string(),
                }),
            );
            return None;
        }
        #[cfg(unix)]
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }

    match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
    {
        Ok(file) => {
            #[cfg(unix)]
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
            Some(file)
        }
        Err(e) => {
            debug_log(
                "agent_pty_log_open_failed",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "path": path.display().to_string(),
                    "error": e.to_string(),
                }),
            );
            None
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AgentDaemonRequest {
    Attach {
        cols: u16,
        rows: u16,
        client_pid: u32,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Input {
        data: Vec<u8>,
    },
    Detach,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AgentDaemonEvent {
    Attached {
        provider: Provider,
        session_id: String,
        command: String,
        #[serde(default)]
        daemon_pid: u32,
        #[serde(default)]
        snapshot_event: bool,
        #[serde(default)]
        last_screen_change_epoch_ms: u64,
        #[serde(default)]
        last_output_epoch_ms: u64,
        #[serde(default)]
        last_input_epoch_ms: u64,
    },
    Output {
        data: Vec<u8>,
    },
    Snapshot {
        data: Vec<u8>,
    },
    Exited {
        status: String,
    },
    Error {
        message: String,
    },
}

struct AgentClient {
    info: SessionInfo,
    command_line: String,
    parser: vt100::Parser,
    screen_history: ScreenHistory,
    history_scroll_offset: usize,
    stream: AgentStream,
    pty_size: PtySize,
    exited: Option<String>,
    screen_hash: u64,
    last_screen_change_epoch_ms: u64,
    last_output_epoch_ms: u64,
    last_input_epoch_ms: u64,
    pending_snapshot_output: bool,
    startup_spinner_started_at: Option<Instant>,
    debug_output_events: u32,
    /// Monotonic id identifying the reader thread that owns the read side
    /// of `stream`. Forwarded inside every `MainEvent::AgentEvent` so the
    /// main loop can ignore events that belong to a previous attach.
    reader_id: u64,
    /// Handle to the agent-socket reader thread. Joined on drop after the
    /// stream is shut down to wake the thread out of its blocking read.
    reader_thread: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentScrollAction {
    Lines(i32),
    Pages(i32),
    Top,
    Bottom,
}

impl Drop for AgentClient {
    fn drop(&mut self) {
        let mut detach_sent = false;
        debug_log(
            "agent_client_drop_start",
            serde_json::json!({
                "provider": self.info.provider.as_str(),
                "session_id": &self.info.session_id,
                "reader_id": self.reader_id,
                "exited": self.exited.as_deref(),
            }),
        );
        if self.exited.is_none() {
            match self.send_request(&AgentDaemonRequest::Detach) {
                Ok(()) => {
                    detach_sent = true;
                    debug_log(
                        "agent_client_drop_detach_sent",
                        serde_json::json!({
                            "provider": self.info.provider.as_str(),
                            "session_id": &self.info.session_id,
                            "reader_id": self.reader_id,
                        }),
                    );
                }
                Err(e) => debug_log(
                    "agent_client_drop_detach_failed",
                    serde_json::json!({
                        "provider": self.info.provider.as_str(),
                        "session_id": &self.info.session_id,
                        "reader_id": self.reader_id,
                        "error": e.to_string(),
                    }),
                ),
            }
        }
        let shutdown_mode = if detach_sent {
            std::net::Shutdown::Write
        } else {
            std::net::Shutdown::Both
        };
        let shutdown_result = self.stream.shutdown(shutdown_mode);
        debug_log(
            "agent_client_drop_shutdown",
            serde_json::json!({
                "provider": self.info.provider.as_str(),
                "session_id": &self.info.session_id,
                "reader_id": self.reader_id,
                "shutdown": shutdown_mode_debug_label(shutdown_mode),
                "ok": shutdown_result.is_ok(),
                "error": shutdown_result.err().map(|e| e.to_string()),
            }),
        );
        if let Some(handle) = self.reader_thread.take() {
            debug_log(
                "agent_client_drop_reader_join_start",
                serde_json::json!({
                    "provider": self.info.provider.as_str(),
                    "session_id": &self.info.session_id,
                    "reader_id": self.reader_id,
                }),
            );
            let _ = handle.join();
            debug_log(
                "agent_client_drop_reader_join_done",
                serde_json::json!({
                    "provider": self.info.provider.as_str(),
                    "session_id": &self.info.session_id,
                    "reader_id": self.reader_id,
                }),
            );
        }
    }
}

fn shutdown_mode_debug_label(mode: std::net::Shutdown) -> &'static str {
    match mode {
        std::net::Shutdown::Read => "read",
        std::net::Shutdown::Write => "write",
        std::net::Shutdown::Both => "both",
    }
}

impl AgentClient {
    fn attach_existing(
        info: SessionInfo,
        cols: u16,
        rows: u16,
        main_tx: Sender<MainEvent>,
        reader_id: u64,
    ) -> Result<Self> {
        let key = AgentKey::new(&info);
        debug_log(
            "agent_client_attach_existing_start",
            serde_json::json!({
                "key": agent_key_debug_value(&key),
                "cols": cols,
                "rows": rows,
                "reader_id": reader_id,
            }),
        );
        let stream = connect_agent_daemon(&key)?;
        Self::attach_stream(
            info,
            stream,
            cols,
            rows,
            main_tx,
            reader_id,
            AgentLaunchMode::Normal,
        )
    }

    fn attach_or_start(
        info: SessionInfo,
        cols: u16,
        rows: u16,
        main_tx: Sender<MainEvent>,
        reader_id: u64,
        launch_mode: AgentLaunchMode,
    ) -> Result<(Self, bool)> {
        let key = AgentKey::new(&info);
        let mut started = false;
        debug_log(
            "agent_client_attach_or_start_start",
            serde_json::json!({
                "info": session_info_debug_value(&info),
                "cols": cols,
                "rows": rows,
                "reader_id": reader_id,
                "launch_mode": launch_mode.as_str(),
            }),
        );
        let stream = match connect_agent_daemon(&key) {
            Ok(stream) => {
                debug_log(
                    "agent_client_attach_or_start_reuse",
                    serde_json::json!({
                        "key": agent_key_debug_value(&key),
                        "launch_mode": launch_mode.as_str(),
                    }),
                );
                stream
            }
            Err(e) => {
                debug_log(
                    "agent_client_attach_or_start_starting_daemon",
                    serde_json::json!({
                        "key": agent_key_debug_value(&key),
                        "connect_error_kind": format!("{:?}", e.kind()),
                        "connect_error": e.to_string(),
                        "launch_mode": launch_mode.as_str(),
                    }),
                );
                validate_session_launch_cwd(&info)?;
                start_agent_daemon(&info, launch_mode)?;
                started = true;
                wait_for_agent_daemon(&key)?
            }
        };
        let mut client =
            Self::attach_stream(info, stream, cols, rows, main_tx, reader_id, launch_mode)?;
        if started && !screen_has_visible_content(client.parser.screen()) {
            client.startup_spinner_started_at = Some(Instant::now());
        }
        debug_log(
            "agent_client_attach_or_start_ready",
            serde_json::json!({
                "key": agent_key_debug_value(&key),
                "started": started,
                "visible": screen_has_visible_content(client.parser.screen()),
                "startup_spinner": client.startup_spinner_started_at.is_some(),
                "reader_id": client.reader_id,
                "launch_mode": launch_mode.as_str(),
            }),
        );
        Ok((client, started))
    }

    fn attach_stream(
        info: SessionInfo,
        stream: AgentStream,
        cols: u16,
        rows: u16,
        main_tx: Sender<MainEvent>,
        reader_id: u64,
        launch_mode: AgentLaunchMode,
    ) -> Result<Self> {
        let mut client = Self::new(info, stream, cols, rows, main_tx, reader_id, launch_mode)?;
        debug_log(
            "agent_client_attach_request_send",
            serde_json::json!({
                "provider": client.info.provider.as_str(),
                "session_id": &client.info.session_id,
                "cols": client.pty_size.cols,
                "rows": client.pty_size.rows,
                "client_pid": std::process::id(),
                "reader_id": reader_id,
            }),
        );
        client.send_request(&AgentDaemonRequest::Attach {
            cols: client.pty_size.cols,
            rows: client.pty_size.rows,
            client_pid: std::process::id(),
        })?;
        Ok(client)
    }

    fn new(
        info: SessionInfo,
        stream: AgentStream,
        cols: u16,
        rows: u16,
        main_tx: Sender<MainEvent>,
        reader_id: u64,
        launch_mode: AgentLaunchMode,
    ) -> Result<Self> {
        // The reader thread uses a blocking clone of the stream; the main
        // thread keeps `self.stream` for writes only.
        let read_stream = stream.try_clone()?;
        read_stream.set_nonblocking(false)?;
        // Main thread's stream stays blocking too — write_json_line writes
        // are small and we want full-write semantics. We never `read` from
        // it on the main thread anymore.
        let pty_size = agent_pty_size(cols, rows);
        let parser = vt100::Parser::new(pty_size.rows, pty_size.cols, AGENT_SCROLLBACK_LINES);
        let screen_hash = screen_activity_hash(parser.screen());

        let provider = info.provider;
        let session_id = info.session_id.clone();
        let thread_tx = main_tx.clone();
        let thread_session_id = session_id.clone();
        let reader_thread = thread::Builder::new()
            .name(format!(
                "cokacmux-agent-reader-{}-{}",
                provider.as_str(),
                truncate_width(&session_id, 8)
            ))
            .spawn(move || {
                debug_log(
                    "agent_reader_thread_start",
                    serde_json::json!({
                        "provider": provider.as_str(),
                        "session_id": &thread_session_id,
                        "reader_id": reader_id,
                    }),
                );
                let reason = run_agent_reader_thread(read_stream, &thread_tx, reader_id);
                let _ = thread_tx.send(MainEvent::AgentReaderEnded { reader_id, reason });
                debug_log(
                    "agent_reader_thread_exit",
                    serde_json::json!({
                        "provider": provider.as_str(),
                        "session_id": thread_session_id,
                        "reader_id": reader_id,
                    }),
                );
            })?;

        let settings = Settings::load();
        let command_line =
            agent_launch_spec_with_settings(&info, launch_mode, &settings).command_line();

        Ok(Self {
            command_line,
            info,
            parser,
            screen_history: ScreenHistory::default(),
            history_scroll_offset: 0,
            stream,
            pty_size,
            exited: None,
            screen_hash,
            last_screen_change_epoch_ms: 0,
            last_output_epoch_ms: 0,
            last_input_epoch_ms: 0,
            pending_snapshot_output: false,
            startup_spinner_started_at: None,
            debug_output_events: 0,
            reader_id,
            reader_thread: Some(reader_thread),
        })
    }

    /// Apply a single `AgentDaemonEvent` that arrived via the main-loop
    /// channel from the reader thread. Replaces the old `drain_events`
    /// which performed its own non-blocking socket read.
    fn process_agent_event(&mut self, event: AgentDaemonEvent) {
        match event {
            AgentDaemonEvent::Attached {
                command,
                snapshot_event,
                last_screen_change_epoch_ms,
                last_output_epoch_ms,
                last_input_epoch_ms,
                ..
            } => {
                self.command_line = command;
                debug_log(
                    "agent_client_event_attached",
                    serde_json::json!({
                        "provider": self.info.provider.as_str(),
                        "session_id": &self.info.session_id,
                        "command": &self.command_line,
                        "snapshot_event": snapshot_event,
                        "last_screen_change_epoch_ms": last_screen_change_epoch_ms,
                        "last_output_epoch_ms": last_output_epoch_ms,
                        "last_input_epoch_ms": last_input_epoch_ms,
                    }),
                );
                self.pending_snapshot_output = !snapshot_event;
                self.last_screen_change_epoch_ms = self
                    .last_screen_change_epoch_ms
                    .max(last_screen_change_epoch_ms);
                self.last_output_epoch_ms = self.last_output_epoch_ms.max(last_output_epoch_ms);
                self.last_input_epoch_ms = self.last_input_epoch_ms.max(last_input_epoch_ms);
            }
            AgentDaemonEvent::Output { data } => {
                trace_log(
                    "agent_client_event_output",
                    serde_json::json!({
                        "provider": self.info.provider.as_str(),
                        "session_id": &self.info.session_id,
                        "len": data.len(),
                        "sample": debug_bytes_sample(&data, 512),
                    }),
                );
                let is_snapshot = std::mem::take(&mut self.pending_snapshot_output);
                self.process_agent_output(&data, !is_snapshot);
            }
            AgentDaemonEvent::Snapshot { data } => {
                debug_log(
                    "agent_client_event_snapshot",
                    serde_json::json!({
                        "provider": self.info.provider.as_str(),
                        "session_id": &self.info.session_id,
                        "len": data.len(),
                        "sample": debug_bytes_sample(&data, 512),
                    }),
                );
                self.pending_snapshot_output = false;
                self.process_agent_output(&data, false);
            }
            AgentDaemonEvent::Exited { status } => {
                debug_log(
                    "agent_client_event_exited",
                    serde_json::json!({
                        "provider": self.info.provider.as_str(),
                        "session_id": &self.info.session_id,
                        "status": &status,
                    }),
                );
                self.exited = Some(status);
            }
            AgentDaemonEvent::Error { message } => {
                debug_log(
                    "agent_client_event_error",
                    serde_json::json!({
                        "provider": self.info.provider.as_str(),
                        "session_id": &self.info.session_id,
                        "message": &message,
                    }),
                );
                self.exited = Some(message);
            }
        }
    }

    fn process_agent_output(&mut self, data: &[u8], counts_as_activity: bool) {
        let now_ms = current_epoch_ms();
        if counts_as_activity {
            self.last_output_epoch_ms = now_ms;
        }
        let screen_changed = process_parser_output(
            &mut self.parser,
            data,
            &mut self.screen_hash,
            Some(&mut self.screen_history),
        );
        if screen_changed {
            self.history_scroll_offset = self.history_scroll_offset.min(
                self.screen_history
                    .max_scroll_offset(self.pty_size.rows as usize),
            );
            if counts_as_activity {
                self.last_screen_change_epoch_ms = now_ms;
            }
        }
        if screen_has_visible_content(self.parser.screen()) {
            self.startup_spinner_started_at = None;
        }
        self.debug_output_events = self.debug_output_events.saturating_add(1);
        if self.debug_output_events <= 20 || TRACE_ENABLED.load(Ordering::Relaxed) {
            debug_log(
                "agent_client_output_processed",
                serde_json::json!({
                    "provider": self.info.provider.as_str(),
                    "session_id": &self.info.session_id,
                    "event": self.debug_output_events,
                    "len": data.len(),
                    "counts_as_activity": counts_as_activity,
                    "visible": screen_has_visible_content(self.parser.screen()),
                    "screen_changed": screen_changed,
                    "screen_history_lines": self.screen_history.len(),
                    "last_screen_change_epoch_ms": self.last_screen_change_epoch_ms,
                    "last_output_epoch_ms": self.last_output_epoch_ms,
                    "preview": debug_screen_preview(self.parser.screen(), 5),
                }),
            );
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let next = agent_pty_size(cols, rows);
        if self.pty_size.rows == next.rows && self.pty_size.cols == next.cols {
            return;
        }
        self.parser.screen_mut().set_size(next.rows, next.cols);
        self.screen_hash = screen_activity_hash(self.parser.screen());
        self.pty_size = next;
        self.history_scroll_offset = self
            .history_scroll_offset
            .min(self.screen_history.max_scroll_offset(next.rows as usize));
        let _ = self.send_request(&AgentDaemonRequest::Resize {
            cols: next.cols,
            rows: next.rows,
        });
    }

    fn scrollback_offset(&self) -> usize {
        let parser_offset = self.parser.screen().scrollback();
        if parser_offset > 0 {
            parser_offset
        } else {
            self.history_scroll_offset
        }
    }

    fn set_scrollback_offset(&mut self, rows: usize) -> usize {
        self.history_scroll_offset = 0;
        self.parser.screen_mut().set_scrollback(rows);
        self.screen_hash = screen_activity_hash(self.parser.screen());
        self.parser.screen().scrollback()
    }

    fn scroll_screen(&mut self, action: AgentScrollAction, page_rows: usize) -> usize {
        let page_rows = page_rows.max(1);
        if parser_max_scrollback(&mut self.parser) == 0 {
            return self.scroll_history_screen(action, page_rows);
        }

        let current = self.parser.screen().scrollback();
        let next = match action {
            AgentScrollAction::Lines(lines) => apply_scrollback_delta(current, lines),
            AgentScrollAction::Pages(pages) => {
                if pages >= 0 {
                    current.saturating_add(page_rows.saturating_mul(pages as usize))
                } else {
                    current.saturating_sub(page_rows.saturating_mul((-pages) as usize))
                }
            }
            AgentScrollAction::Top => usize::MAX,
            AgentScrollAction::Bottom => 0,
        };
        self.set_scrollback_offset(next)
    }

    fn scroll_history_screen(&mut self, action: AgentScrollAction, page_rows: usize) -> usize {
        self.parser.screen_mut().set_scrollback(0);
        self.screen_hash = screen_activity_hash(self.parser.screen());
        let max_offset = self.screen_history.max_scroll_offset(page_rows);
        let current = self.history_scroll_offset.min(max_offset);
        let next = match action {
            AgentScrollAction::Lines(lines) => apply_scrollback_delta(current, lines),
            AgentScrollAction::Pages(pages) => {
                if pages >= 0 {
                    current.saturating_add(page_rows.saturating_mul(pages as usize))
                } else {
                    current.saturating_sub(page_rows.saturating_mul((-pages) as usize))
                }
            }
            AgentScrollAction::Top => max_offset,
            AgentScrollAction::Bottom => 0,
        }
        .min(max_offset);
        self.history_scroll_offset = next;
        next
    }

    fn send_key(&mut self, key: KeyEvent) {
        if let Some(data) = key_event_to_bytes(key) {
            if self.scrollback_offset() > 0 {
                self.set_scrollback_offset(0);
                self.history_scroll_offset = 0;
            }
            self.last_input_epoch_ms = current_epoch_ms();
            let _ = self.send_request(&AgentDaemonRequest::Input { data });
        }
    }

    fn activity(&self) -> AgentActivity {
        agent_activity_from_timestamps(
            current_epoch_ms(),
            self.last_screen_change_epoch_ms,
            self.last_output_epoch_ms,
            self.last_input_epoch_ms,
        )
    }

    fn startup_spinner_started_at(&self) -> Option<Instant> {
        if self.startup_spinner_started_at.is_some()
            && self.exited.is_none()
            && !screen_has_visible_content(self.parser.screen())
        {
            self.startup_spinner_started_at
        } else {
            None
        }
    }

    fn send_request(&mut self, request: &AgentDaemonRequest) -> io::Result<()> {
        let request_kind = match request {
            AgentDaemonRequest::Attach { .. } => "attach",
            AgentDaemonRequest::Resize { .. } => "resize",
            AgentDaemonRequest::Input { .. } => "input",
            AgentDaemonRequest::Detach => "detach",
        };
        if DEBUG_ENABLED.load(Ordering::Relaxed) {
            let details = serde_json::json!({
                "provider": self.info.provider.as_str(),
                "session_id": &self.info.session_id,
                "reader_id": self.reader_id,
                "request": request_kind,
                "detail": request,
            });
            if matches!(request, AgentDaemonRequest::Input { .. }) {
                trace_log("agent_client_request_send", details);
            } else {
                debug_log("agent_client_request_send", details);
            }
        }
        let result = write_json_line(&mut self.stream, request);
        if let Err(e) = result.as_ref() {
            debug_log(
                "agent_client_request_send_failed",
                serde_json::json!({
                    "provider": self.info.provider.as_str(),
                    "session_id": &self.info.session_id,
                    "reader_id": self.reader_id,
                    "request": request_kind,
                    "error_kind": format!("{:?}", e.kind()),
                    "error": e.to_string(),
                }),
            );
        }
        result
    }
}

struct DaemonConnection {
    stream: AgentStream,
    read_buf: Vec<u8>,
}

impl DaemonConnection {
    fn new(stream: AgentStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            read_buf: Vec::new(),
        })
    }

    fn send_event(&mut self, event: &AgentDaemonEvent) -> io::Result<()> {
        write_json_line(&mut self.stream, event)
    }

    fn read_requests(&mut self) -> io::Result<Vec<AgentDaemonRequest>> {
        read_agent_daemon_requests(&mut self.stream, &mut self.read_buf)
    }
}

/// Events flowing into the main loop from worker threads. The single
/// `Receiver<MainEvent>` is the only thing the main loop blocks on, so it
/// wakes the instant any producer fires — terminal input, agent socket
/// output, or the housekeeping tick. There is no fallback latency timer.
enum MainEvent {
    /// A crossterm input event read off stdin in a dedicated thread.
    Input(Event),
    /// One decoded `AgentDaemonEvent` forwarded from the agent reader
    /// thread. The event id helps the main loop ignore events from a
    /// reader thread that belongs to a now-detached/old agent.
    AgentEvent {
        reader_id: u64,
        event: AgentDaemonEvent,
    },
    /// The agent reader thread terminated (socket EOF or error). Main loop
    /// treats this like a daemon-exit notification.
    AgentReaderEnded { reader_id: u64, reason: String },
    /// Housekeeping tick — fires every `AGENT_STATE_POLL_INTERVAL_MS` so
    /// preview results / agent runtime state polls get a chance to run
    /// even when the user is idle. Not a latency knob.
    Tick,
    /// Full-session search completed in the background.
    SearchResult(SearchWorkerResult),
}

struct App {
    settings: Settings,
    keybindings: KeyBindings,
    keybindings_path: Option<PathBuf>,
    keybindings_mtime: Option<SystemTime>,
    sessions: Vec<SessionInfo>,
    /// Live shell daemons discovered via `~/.cokacmux/agents/` meta scan.
    /// Shells are not in `sessions` (they have no on-disk storage), but they
    /// participate in the agents sidebar so the user can switch between
    /// shells and real agents with Alt+↑/↓ or Ctrl+Shift+↑/↓.
    live_shells: Vec<SessionInfo>,
    clone_links: Vec<session::clone_tree::CloneLink>,
    agent_states: HashMap<AgentKey, AgentListState>,
    new_agent_backing_aliases: HashMap<AgentKey, AgentKey>,
    last_agent_state_poll: Instant,
    list_state: ListState,
    session_view: SessionViewMode,
    provider_filter: ProviderFilter,
    text_filter: String,
    text_filter_matches: HashSet<AgentKey>,
    search_seq: u64,
    search_pending: Option<SearchPending>,
    input_mode: InputMode,
    preview_cache: HashMap<PreviewKey, PreviewEntry>,
    preview_cache_order: VecDeque<PreviewKey>,
    preview_requested: Option<(PreviewKey, u64)>,
    preview_seq: u64,
    preview_tx: Sender<PreviewRequest>,
    preview_rx: Receiver<PreviewResult>,
    preview_mode: Mode,
    preview_scroll: u16,
    preview_page_height: u16,
    focus: FocusPane,
    status: String,
    active_agent: Option<AgentClient>,
    should_quit: bool,
    /// Sender installed by `run()` after the channel exists. Cloned and
    /// handed to each agent reader thread on attach so they can forward
    /// `AgentDaemonEvent`s back to the main loop.
    main_tx: Option<Sender<MainEvent>>,
    /// Monotonic id assigned to each agent reader thread. Used to ignore
    /// late events from a thread whose AgentClient has already been
    /// dropped/replaced.
    next_reader_id: u64,
    /// When true the sessions list is rendered even if `active_agent` is
    /// Some — i.e. the user toggled to the sessions view via Ctrl+] / Ctrl+[ but
    /// the agent's daemon socket and reader thread stay alive in the
    /// background. The next Ctrl+] / Ctrl+[ flips this back to the agent view
    /// without re-attaching anything.
    show_sessions_view: bool,
}

#[derive(Debug, Clone, Copy)]
struct VisibleSessionRow<'a> {
    info: &'a SessionInfo,
    depth: usize,
}

impl App {
    fn new() -> Self {
        let (preview_tx, preview_rx) = spawn_preview_worker();
        let settings = Settings::load();
        let keybindings_path = keybinding_path();
        let (keybindings, keybindings_mtime) =
            KeyBindings::load_with_mtime(keybindings_path.as_deref());
        let session_view = settings.cokacmux.session_view;
        let orphan_pty_logs_removed = cleanup_orphan_agent_pty_logs();
        let mut app = Self {
            settings,
            keybindings,
            keybindings_path,
            keybindings_mtime,
            sessions: Vec::new(),
            live_shells: Vec::new(),
            clone_links: Vec::new(),
            agent_states: HashMap::new(),
            new_agent_backing_aliases: HashMap::new(),
            last_agent_state_poll: Instant::now(),
            list_state: ListState::default(),
            session_view,
            provider_filter: ProviderFilter::All,
            text_filter: String::new(),
            text_filter_matches: HashSet::new(),
            search_seq: 0,
            search_pending: None,
            input_mode: InputMode::Normal,
            preview_cache: HashMap::new(),
            preview_cache_order: VecDeque::new(),
            preview_requested: None,
            preview_seq: 0,
            preview_tx,
            preview_rx,
            preview_mode: Mode::Summary,
            preview_scroll: 0,
            preview_page_height: 10,
            focus: FocusPane::Sessions,
            status: String::from("loading…"),
            active_agent: None,
            should_quit: false,
            main_tx: None,
            next_reader_id: 0,
            show_sessions_view: true,
        };
        app.refresh();
        app.refresh_agent_runtime_states();
        debug_log(
            "app_new",
            serde_json::json!({
                "sessions": app.sessions.len(),
                "visible": app.visible().len(),
                "session_view": app.session_view.label(),
                "provider_filter": app.provider_filter.label(),
                "text_filter": &app.text_filter,
                "orphan_pty_logs_removed": orphan_pty_logs_removed,
            }),
        );
        app
    }

    fn sessions_pane_width(&self, total_width: u16) -> u16 {
        sessions_pane_width(
            total_width,
            self.settings.cokacmux.sessions_pane_width,
            self.settings.cokacmux.sessions_pane_percent,
        )
    }

    fn agent_sidebar_config_width(&self) -> u16 {
        if !self.settings.cokacmux.agent_sidebar_visible {
            return 0;
        }
        self.settings.cokacmux.agent_sidebar_width
    }

    fn agent_sidebar_width(&self, total_width: u16) -> u16 {
        agent_sidebar_width(total_width, self.agent_sidebar_config_width())
    }

    fn maybe_reload_keybindings(&mut self) {
        let (current_mtime, created) = match KeyBindings::ensure_file_or_mtime_with_created(
            self.keybindings_path.as_deref(),
        ) {
            Ok(state) => state,
            Err(e) => {
                debug_log(
                    "keybindings_mtime_failed",
                    serde_json::json!({
                        "error": e,
                    }),
                );
                self.status = format!("keybinding reload check failed: {}", truncate_width(&e, 80));
                return;
            }
        };
        if current_mtime == self.keybindings_mtime && !created {
            return;
        }

        match KeyBindings::read_for_observed_mtime(self.keybindings_path.as_deref(), current_mtime)
        {
            Ok(keybindings) => {
                self.keybindings = keybindings;
                self.keybindings_mtime = current_mtime;
                self.status = if created {
                    "keybinding file created with defaults.".into()
                } else if current_mtime.is_some() {
                    "keybindings reloaded.".into()
                } else {
                    "keybindings reset to defaults.".into()
                };
                debug_log(
                    "keybindings_reloaded",
                    serde_json::json!({
                        "path": self
                            .keybindings_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "has_file": current_mtime.is_some(),
                        "created": created,
                    }),
                );
            }
            Err(e) => {
                self.keybindings_mtime = current_mtime;
                self.status = format!("keybinding reload failed: {}", truncate_width(&e, 80));
                debug_log(
                    "keybindings_reload_failed",
                    serde_json::json!({
                        "path": self
                            .keybindings_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "error": e,
                    }),
                );
            }
        }
    }

    /// Flip the agents sidebar's visibility and persist the change so
    /// the choice survives restarts. The configured width is left alone
    /// so the user's manual resize comes back when they toggle on again.
    fn toggle_agent_sidebar_visible(&mut self) {
        let next = !self.settings.cokacmux.agent_sidebar_visible;
        self.settings.cokacmux.agent_sidebar_visible = next;
        self.status = if next {
            "agents sidebar: shown".into()
        } else {
            "agents sidebar: hidden".into()
        };
        if let Err(e) = self.settings.save() {
            debug_log(
                "settings_save_failed",
                serde_json::json!({
                    "field": "agent_sidebar_visible",
                    "error": e.to_string(),
                }),
            );
        }
        debug_log(
            "agent_sidebar_visible_toggle",
            serde_json::json!({ "visible": next }),
        );
    }

    fn adjust_sessions_pane_width(&mut self, delta: i16, total_width: u16) {
        let (next, limited) = adjusted_sessions_pane_width(
            self.settings.cokacmux.sessions_pane_width,
            total_width,
            self.settings.cokacmux.sessions_pane_percent,
            delta,
        );
        if limited {
            let preview = total_width.saturating_sub(next);
            self.status = format!(
                "layout limit: sessions {} cols, preview {} cols",
                next, preview
            );
            debug_log(
                "sessions_pane_resize_limited",
                serde_json::json!({
                    "next": next,
                    "preview": preview,
                    "total_width": total_width,
                    "delta": delta,
                }),
            );
            return;
        }

        self.settings.cokacmux.sessions_pane_width = Some(next);
        match self.settings.save() {
            Ok(()) => {
                let preview = total_width.saturating_sub(next);
                self.status = format!(
                    "layout saved: sessions {} cols, preview {} cols",
                    next, preview
                );
                debug_log(
                    "sessions_pane_resize_saved",
                    serde_json::json!({
                        "sessions": next,
                        "preview": preview,
                        "total_width": total_width,
                        "delta": delta,
                    }),
                );
            }
            Err(e) => {
                self.status = format!("layout changed, save failed: {}", e);
                debug_log(
                    "sessions_pane_resize_save_failed",
                    serde_json::json!({
                        "sessions": next,
                        "total_width": total_width,
                        "delta": delta,
                        "error": e.to_string(),
                    }),
                );
            }
        }
        self.clear_preview_cache();
    }

    fn adjust_agent_sidebar_width(&mut self, delta: i16, total_width: u16) {
        let (next, limited) =
            adjusted_agent_sidebar_width(self.agent_sidebar_config_width(), total_width, delta);
        if limited {
            self.status = format!("layout limit: agent sidebar {} cols", next);
            debug_log(
                "agent_sidebar_resize_limited",
                serde_json::json!({
                    "next": next,
                    "total_width": total_width,
                    "delta": delta,
                }),
            );
            return;
        }

        self.settings.cokacmux.agent_sidebar_width = next;
        match self.settings.save() {
            Ok(()) => {
                self.status = format!("layout saved: agent sidebar {} cols", next);
                debug_log(
                    "agent_sidebar_resize_saved",
                    serde_json::json!({
                        "next": next,
                        "total_width": total_width,
                        "delta": delta,
                    }),
                );
            }
            Err(e) => {
                self.status = format!("layout changed, save failed: {}", e);
                debug_log(
                    "agent_sidebar_resize_save_failed",
                    serde_json::json!({
                        "next": next,
                        "total_width": total_width,
                        "delta": delta,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn refresh(&mut self) {
        debug_log(
            "sessions_refresh_start",
            serde_json::json!({
                "session_view": self.session_view.label(),
                "provider_filter": self.provider_filter.label(),
                "text_filter": &self.text_filter,
            }),
        );
        match session::list_all() {
            Ok(all) => {
                self.sessions = all;
                let clone_tree_error = match session::clone_tree::load_links() {
                    Ok(links) => {
                        self.clone_links = links;
                        None
                    }
                    Err(e) => {
                        self.clone_links.clear();
                        Some(e.to_string())
                    }
                };
                self.status = if let Some(error) = clone_tree_error {
                    format!(
                        "{} sessions (clone tree error: {})",
                        self.visible().len(),
                        error
                    )
                } else {
                    format!("{} sessions", self.visible().len())
                };
                if !self.visible_rows().is_empty() {
                    self.list_state.select(Some(0));
                } else {
                    self.list_state.select(None);
                }
                self.clear_preview_cache();
                self.preview_scroll = 0;
                self.refresh_agent_runtime_states();
                debug_log(
                    "sessions_refresh_ok",
                    serde_json::json!({
                        "sessions": self.sessions.len(),
                        "visible": self.visible().len(),
                        "clone_links": self.clone_links.len(),
                        "status": &self.status,
                    }),
                );
            }
            Err(e) => {
                self.status = format!("list error: {}", e);
                debug_log(
                    "sessions_refresh_failed",
                    serde_json::json!({
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn prepare_sessions_view(&mut self, reason: &str) {
        let selected_key = self.current().map(AgentKey::new);
        let previous_status = self.status.clone();
        let active_agent = self
            .active_agent
            .as_ref()
            .map(|agent| session_info_debug_value(&agent.info));
        debug_log(
            "sessions_view_prepare_start",
            serde_json::json!({
                "reason": reason,
                "selected": selected_key.as_ref().map(|key| serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                })),
                "active_agent": active_agent,
                "show_sessions_view": self.show_sessions_view,
                "agent_states_before": agent_state_entries_debug_value(&self.agent_states),
                "live_shells_before": self.live_shells.iter().map(session_info_debug_value).collect::<Vec<_>>(),
            }),
        );

        self.refresh();

        let refresh_failed = self.status.starts_with("list error:");
        let active_backing_key = self.active_agent_backing_key();
        let restored_active_backing = active_backing_key
            .as_ref()
            .is_some_and(|key| self.restore_visible_selection(key));
        if !restored_active_backing {
            if let Some(key) = selected_key.as_ref() {
                self.restore_visible_selection(key);
            }
        }
        if let Some(key) = active_backing_key.as_ref() {
            debug_log(
                "sessions_view_active_backing_selection",
                serde_json::json!({
                    "backing": agent_key_debug_value(key),
                    "restored": restored_active_backing,
                }),
            );
        }
        if !refresh_failed {
            self.status = previous_status;
        }

        debug_log(
            "sessions_view_prepare_ok",
            serde_json::json!({
                "reason": reason,
                "sessions": self.sessions.len(),
                "visible": self.visible().len(),
                "selected": self.current().map(|info| serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                })),
                "status": &self.status,
                "agent_states_after": agent_state_entries_debug_value(&self.agent_states),
                "live_shells_after": self.live_shells.iter().map(session_info_debug_value).collect::<Vec<_>>(),
                "show_sessions_view": self.show_sessions_view,
            }),
        );
    }

    fn active_agent_backing_key(&self) -> Option<AgentKey> {
        let agent = self.active_agent.as_ref()?;
        if !is_new_agent_session_info(&agent.info) {
            return None;
        }
        let synthetic_key = AgentKey::new(&agent.info);
        self.new_agent_backing_aliases.get(&synthetic_key).cloned()
    }

    fn visible(&self) -> Vec<&SessionInfo> {
        self.visible_rows()
            .into_iter()
            .map(|row| row.info)
            .collect()
    }

    fn visible_rows(&self) -> Vec<VisibleSessionRow<'_>> {
        match self.session_view {
            SessionViewMode::List => self
                .sessions
                .iter()
                .filter(|s| self.matches_session_filters(s))
                .map(|info| VisibleSessionRow { info, depth: 0 })
                .collect(),
            SessionViewMode::Tree => {
                session::clone_tree::visible_tree_rows(&self.sessions, &self.clone_links, |info| {
                    self.matches_session_filters(info)
                })
                .into_iter()
                .map(|row| VisibleSessionRow {
                    info: row.info,
                    depth: row.depth,
                })
                .collect()
            }
        }
    }

    fn matches_session_filters(&self, s: &SessionInfo) -> bool {
        if !self.provider_filter.matches(s.provider) {
            return false;
        }
        let q = self.text_filter.to_lowercase();
        q.is_empty()
            || s.session_id.to_lowercase().contains(&q)
            || s.cwd.to_lowercase().contains(&q)
            || s.title.as_deref().unwrap_or("").to_lowercase().contains(&q)
            || self.text_filter_matches.contains(&AgentKey::new(s))
    }

    fn begin_filter(&mut self) {
        let draft = self.text_filter.clone();
        let cursor = draft.len();
        self.input_mode = InputMode::Filter { draft, cursor };
        self.focus = FocusPane::Sessions;
        self.status = "search sessions".into();
        debug_log(
            "filter_begin",
            serde_json::json!({
                "existing_query_len": self.text_filter.chars().count(),
            }),
        );
    }

    fn start_text_search(&mut self, query: String) {
        let query = query.trim().to_string();
        if query.is_empty() {
            self.text_filter.clear();
            self.text_filter_matches.clear();
            self.search_pending = None;
            self.input_mode = InputMode::Normal;
            self.focus = FocusPane::Sessions;
            self.select_first();
            self.status = format!("search cleared: {} sessions", self.visible().len());
            debug_log("filter_clear", serde_json::json!({}));
            return;
        }

        self.search_seq = self.search_seq.saturating_add(1);
        let seq = self.search_seq;
        self.search_pending = Some(SearchPending {
            seq,
            query: query.clone(),
            started_at: Instant::now(),
        });
        self.focus = FocusPane::Sessions;
        self.status = format!("searching \"{}\"...", truncate_width(&query, 24));
        debug_log(
            "filter_search_start",
            serde_json::json!({
                "seq": seq,
                "query_len": query.chars().count(),
            }),
        );

        let Some(tx) = self.main_tx.clone() else {
            let hits = session::search_all(&query, true).map_err(|e| e.to_string());
            self.on_search_result(SearchWorkerResult { seq, query, hits });
            return;
        };
        let worker_query = query.clone();
        match thread::Builder::new()
            .name("cokacmux-search".to_string())
            .spawn(move || {
                let hits = session::search_all(&worker_query, true).map_err(|e| e.to_string());
                let _ = tx.send(MainEvent::SearchResult(SearchWorkerResult {
                    seq,
                    query: worker_query,
                    hits,
                }));
            }) {
            Ok(_) => {}
            Err(e) => {
                self.search_pending = None;
                self.status = format!(
                    "search worker failed: {}",
                    truncate_width(&e.to_string(), 80)
                );
                debug_log(
                    "filter_search_spawn_failed",
                    serde_json::json!({
                        "seq": seq,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn on_search_result(&mut self, result: SearchWorkerResult) {
        let Some(pending) = self.search_pending.as_ref() else {
            debug_log(
                "filter_search_result_ignored",
                serde_json::json!({
                    "seq": result.seq,
                    "reason": "none_pending",
                }),
            );
            return;
        };
        if pending.seq != result.seq || pending.query != result.query {
            debug_log(
                "filter_search_result_ignored",
                serde_json::json!({
                    "seq": result.seq,
                    "pending_seq": pending.seq,
                    "reason": "stale",
                }),
            );
            return;
        }

        self.search_pending = None;
        match result.hits {
            Ok(hits) => self.finish_text_search(result.query, hits),
            Err(e) => {
                self.status = format!("search failed: {}", truncate_width(&e, 80));
                debug_log(
                    "filter_apply_failed",
                    serde_json::json!({
                        "seq": result.seq,
                        "query_len": result.query.chars().count(),
                        "error": e,
                    }),
                );
            }
        }
    }

    fn finish_text_search(&mut self, query: String, hits: Vec<session::SearchHit>) {
        let content_match_count = hits.len();
        self.text_filter = query;
        self.text_filter_matches = hits
            .into_iter()
            .map(|hit| AgentKey::new(&hit.info))
            .collect();
        self.input_mode = InputMode::Normal;
        self.focus = FocusPane::Sessions;
        self.select_first();
        let visible = self.visible().len();
        self.status = format!(
            "search \"{}\": {} sessions",
            truncate_width(&self.text_filter, 24),
            visible
        );
        debug_log(
            "filter_apply",
            serde_json::json!({
                "query_len": self.text_filter.chars().count(),
                "content_matches": content_match_count,
                "visible": visible,
            }),
        );
    }

    fn agent_state_for(&self, info: &SessionInfo) -> AgentListState {
        let key = AgentKey::new(info);
        if let Some(agent) = self.active_agent.as_ref() {
            if AgentKey::new(&agent.info) == key {
                let cached_state = self.agent_states.get(&key).copied();
                let client_activity = agent.activity();
                let activity = self
                    .agent_states
                    .get(&key)
                    .map(|state| state.activity())
                    .unwrap_or(AgentActivity::Quiet)
                    .combine(client_activity);
                let result = AgentListState::Attached {
                    mine: true,
                    activity,
                };
                trace_log(
                    "agent_state_for_active",
                    serde_json::json!({
                        "key": agent_key_debug_value(&key),
                        "info": session_info_debug_value(info),
                        "cached_state": optional_agent_list_state_debug_value(cached_state),
                        "client_activity": client_activity.label(),
                        "result": agent_list_state_debug_value(result),
                        "show_sessions_view": self.show_sessions_view,
                    }),
                );
                return result;
            }
        }
        let cached_state = self.agent_states.get(&key).copied();
        let result = cached_state.unwrap_or(AgentListState::Idle);
        trace_log(
            "agent_state_for",
            serde_json::json!({
                "key": agent_key_debug_value(&key),
                "info": session_info_debug_value(info),
                "cached_state": optional_agent_list_state_debug_value(cached_state),
                "result": agent_list_state_debug_value(result),
                "active_agent": self.active_agent.as_ref().map(|agent| session_info_debug_value(&agent.info)),
                "show_sessions_view": self.show_sessions_view,
                "agent_states_len": self.agent_states.len(),
                "live_shells_len": self.live_shells.len(),
            }),
        );
        result
    }

    fn poll_agent_runtime_states(&mut self) {
        if self.last_agent_state_poll.elapsed()
            < Duration::from_millis(AGENT_STATE_POLL_INTERVAL_MS)
        {
            return;
        }
        self.refresh_agent_runtime_states();
    }

    fn refresh_agent_runtime_states(&mut self) {
        self.last_agent_state_poll = Instant::now();
        let current_pid = std::process::id();
        let previous_states_len = self.agent_states.len();
        trace_log(
            "agent_runtime_refresh_start",
            serde_json::json!({
                "current_pid": current_pid,
                "sessions_len": self.sessions.len(),
                "previous_live_shells_len": self.live_shells.len(),
                "previous_agent_states_len": previous_states_len,
                "active_agent": self.active_agent.as_ref().map(|agent| session_info_debug_value(&agent.info)),
                "show_sessions_view": self.show_sessions_view,
            }),
        );

        // Refresh the live-shells list each tick — daemons can come and go
        // independently of our own clone/attach actions.
        self.live_shells = discover_live_shell_infos();

        let mut states = HashMap::new();
        let session_keys: Vec<AgentKey> = self.sessions.iter().map(AgentKey::new).collect();
        let shell_keys: Vec<AgentKey> = self.live_shells.iter().map(AgentKey::new).collect();
        trace_log(
            "agent_runtime_refresh_keys",
            serde_json::json!({
                "session_key_count": session_keys.len(),
                "shell_key_count": shell_keys.len(),
                "shells": self.live_shells.iter().map(session_info_debug_value).collect::<Vec<_>>(),
            }),
        );
        for key in session_keys.iter().chain(shell_keys.iter()) {
            let state = read_agent_runtime_state(key, current_pid);
            trace_log(
                "agent_runtime_refresh_key",
                serde_json::json!({
                    "key": agent_key_debug_value(key),
                    "state": agent_list_state_debug_value(state),
                }),
            );
            if state != AgentListState::Idle {
                states.insert(key.clone(), state);
            }
        }
        if let Some(agent) = self.active_agent.as_ref() {
            let key = AgentKey::new(&agent.info);
            let previous_state = states.get(&key).copied();
            let client_activity = agent.activity();
            let activity = states
                .get(&key)
                .map(|state| state.activity())
                .unwrap_or(AgentActivity::Quiet)
                .combine(client_activity);
            states.insert(
                key.clone(),
                AgentListState::Attached {
                    mine: true,
                    activity,
                },
            );
            trace_log(
                "agent_runtime_refresh_active_overlay",
                serde_json::json!({
                    "key": agent_key_debug_value(&key),
                    "previous_state": optional_agent_list_state_debug_value(previous_state),
                    "client_activity": client_activity.label(),
                    "overlay_state": agent_list_state_debug_value(AgentListState::Attached {
                        mine: true,
                        activity,
                    }),
                }),
            );
        }
        self.apply_new_agent_backing_states(&mut states);
        self.agent_states = states;
        trace_log(
            "agent_runtime_refresh_done",
            serde_json::json!({
                "current_pid": current_pid,
                "agent_states_len": self.agent_states.len(),
                "states": agent_state_entries_debug_value(&self.agent_states),
                "new_agent_backing_aliases": agent_alias_entries_debug_value(&self.new_agent_backing_aliases),
                "live_shells_len": self.live_shells.len(),
                "show_sessions_view": self.show_sessions_view,
            }),
        );
    }

    fn apply_new_agent_backing_states(&mut self, states: &mut HashMap<AgentKey, AgentListState>) {
        for info in &self.live_shells {
            if !is_new_agent_session_info(info) || info.provider != Provider::Codex {
                continue;
            }
            let synthetic_key = AgentKey::new(info);
            let Some(state) = states.get(&synthetic_key).copied() else {
                continue;
            };
            if state == AgentListState::Idle {
                continue;
            }
            if let Some(backing_key) = self.new_agent_backing_aliases.get(&synthetic_key).cloned() {
                if backing_key != synthetic_key {
                    let previous_state = states.insert(backing_key.clone(), state);
                    trace_log(
                        "new_agent_backing_cached_state_applied",
                        serde_json::json!({
                            "synthetic": agent_key_debug_value(&synthetic_key),
                            "backing": agent_key_debug_value(&backing_key),
                            "state": agent_list_state_debug_value(state),
                            "previous_state": optional_agent_list_state_debug_value(previous_state),
                        }),
                    );
                }
                continue;
            }
            let Some(backing) = new_agent_backing_session_for_key(&synthetic_key) else {
                continue;
            };
            if backing.key == synthetic_key {
                continue;
            }
            let previous_state = states.insert(backing.key.clone(), state);
            let previous_alias = self
                .new_agent_backing_aliases
                .insert(synthetic_key.clone(), backing.key.clone());
            if previous_alias.as_ref() != Some(&backing.key) {
                debug_log(
                    "new_agent_backing_session_linked",
                    serde_json::json!({
                        "synthetic": agent_key_debug_value(&synthetic_key),
                        "backing": agent_key_debug_value(&backing.key),
                        "rollout_path": backing.rollout_path.display().to_string(),
                        "state": agent_list_state_debug_value(state),
                        "previous_alias": previous_alias.as_ref().map(agent_key_debug_value),
                        "previous_state": optional_agent_list_state_debug_value(previous_state),
                    }),
                );
            } else {
                trace_log(
                    "new_agent_backing_state_applied",
                    serde_json::json!({
                        "synthetic": agent_key_debug_value(&synthetic_key),
                        "backing": agent_key_debug_value(&backing.key),
                        "rollout_path": backing.rollout_path.display().to_string(),
                        "state": agent_list_state_debug_value(state),
                        "previous_state": optional_agent_list_state_debug_value(previous_state),
                    }),
                );
            }
        }
    }

    fn mark_agent_attached_locally(&mut self, key: AgentKey) {
        let debug_key = key.clone();
        self.agent_states.insert(
            key,
            AgentListState::Attached {
                mine: true,
                activity: AgentActivity::Quiet,
            },
        );
        if DEBUG_ENABLED.load(Ordering::Relaxed) {
            debug_log(
                "agent_state_mark_attached_locally",
                serde_json::json!({
                    "key": agent_key_debug_value(&debug_key),
                    "agent_states_len": self.agent_states.len(),
                }),
            );
        }
    }

    fn live_agent_restore_candidates(&self) -> Vec<SessionInfo> {
        let mut candidates: Vec<SessionInfo> = self
            .sessions
            .iter()
            .chain(self.live_shells.iter())
            .filter(|info| {
                let key = AgentKey::new(info);
                if self.is_backing_key_for_new_agent(&key) {
                    return false;
                }
                self.agent_states
                    .get(&key)
                    .copied()
                    .is_some_and(is_switchable_agent_state)
            })
            .cloned()
            .collect();
        let mut seen: HashSet<AgentKey> = HashSet::new();
        candidates.retain(|info| seen.insert(AgentKey::new(info)));
        candidates
    }

    fn live_agent_restore_candidate(&self) -> Option<SessionInfo> {
        let selected_key = self.current().map(AgentKey::new);
        if let Some(selected_key) = selected_key.as_ref() {
            if let Some(info) = self.synthetic_new_agent_for_backing_key(selected_key) {
                return Some(info.clone());
            }
        }
        let candidates = self.live_agent_restore_candidates();
        if candidates.is_empty() {
            return None;
        }
        if let Some(selected_key) = selected_key {
            if let Some(info) = candidates
                .iter()
                .find(|info| AgentKey::new(info) == selected_key)
            {
                return Some(info.clone());
            }
        }
        candidates.into_iter().next()
    }

    fn synthetic_new_agent_for_backing_key(&self, backing_key: &AgentKey) -> Option<&SessionInfo> {
        self.live_shells.iter().find(|info| {
            if !is_new_agent_session_info(info) {
                return false;
            }
            let synthetic_key = AgentKey::new(info);
            self.new_agent_backing_aliases
                .get(&synthetic_key)
                .is_some_and(|candidate| candidate == backing_key)
        })
    }

    fn is_backing_key_for_new_agent(&self, key: &AgentKey) -> bool {
        self.synthetic_new_agent_for_backing_key(key).is_some()
    }

    fn runtime_info_for_selected_agent(&self, info: &SessionInfo) -> SessionInfo {
        let key = AgentKey::new(info);
        self.synthetic_new_agent_for_backing_key(&key)
            .cloned()
            .unwrap_or_else(|| info.clone())
    }

    fn live_agent_switch_candidates(&self) -> Vec<SessionInfo> {
        let Some(active_agent) = self.active_agent.as_ref() else {
            return Vec::new();
        };
        let current_key = AgentKey::new(&active_agent.info);
        // Pool = discovered agent sessions ∪ live shell daemons. From this we
        // keep entries that are either the current pane or have a live state
        // in `agent_states`. Shells participate so the sidebar shows both.
        let mut candidates: Vec<SessionInfo> = self
            .sessions
            .iter()
            .chain(self.live_shells.iter())
            .filter(|info| {
                let key = AgentKey::new(info);
                if self.is_backing_key_for_new_agent(&key) {
                    return false;
                }
                key == current_key
                    || self
                        .agent_states
                        .get(&key)
                        .copied()
                        .is_some_and(is_switchable_agent_state)
            })
            .cloned()
            .collect();
        // Defensive dedupe — a key should appear at most once.
        let mut seen: HashSet<AgentKey> = HashSet::new();
        candidates.retain(|info| seen.insert(AgentKey::new(info)));
        if !candidates
            .iter()
            .any(|info| AgentKey::new(info) == current_key)
        {
            candidates.insert(0, active_agent.info.clone());
        }
        candidates
    }

    fn select_visible_session(&mut self, key: &AgentKey) {
        let selected = self
            .visible()
            .iter()
            .position(|info| AgentKey::new(info) == *key);
        if let Some(index) = selected {
            self.list_state.select(Some(index));
            self.preview_scroll = 0;
            debug_log(
                "session_select_visible",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "index": index,
                    "visible": self.visible().len(),
                }),
            );
        } else {
            self.list_state.select(None);
            debug_log(
                "session_select_visible_missing",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "visible": self.visible().len(),
                }),
            );
        }
    }

    fn current(&self) -> Option<&SessionInfo> {
        let vis = self.visible();
        let i = self.list_state.selected()?;
        vis.get(i).copied()
    }

    fn restore_visible_selection(&mut self, key: &AgentKey) -> bool {
        let selected = self
            .visible_rows()
            .iter()
            .position(|row| AgentKey::new(row.info) == *key);
        let Some(index) = selected else {
            return false;
        };
        self.list_state.select(Some(index));
        self.preview_scroll = 0;
        debug_log(
            "session_restore_visible_selection",
            serde_json::json!({
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
                "index": index,
                "visible": self.visible().len(),
            }),
        );
        true
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.visible().len();
        let before = self.list_state.selected();
        self.list_state.select(clamped_selection_index(
            len,
            self.list_state.selected(),
            delta,
        ));
        self.preview_scroll = 0;
        debug_log(
            "session_selection_move",
            serde_json::json!({
                "delta": delta,
                "before": before,
                "after": self.list_state.selected(),
                "visible": len,
            }),
        );
    }

    fn select_first(&mut self) {
        let before = self.list_state.selected();
        if !self.visible().is_empty() {
            self.list_state.select(Some(0));
        } else {
            self.list_state.select(None);
        }
        self.preview_scroll = 0;
        debug_log(
            "session_select_first",
            serde_json::json!({
                "before": before,
                "after": self.list_state.selected(),
                "visible": self.visible().len(),
            }),
        );
    }

    fn select_last(&mut self) {
        let n = self.visible().len();
        let before = self.list_state.selected();
        if n > 0 {
            self.list_state.select(Some(n - 1));
        } else {
            self.list_state.select(None);
        }
        self.preview_scroll = 0;
        debug_log(
            "session_select_last",
            serde_json::json!({
                "before": before,
                "after": self.list_state.selected(),
                "visible": n,
            }),
        );
    }

    fn toggle_focus(&mut self) {
        let before = self.focus;
        self.focus = match self.focus {
            FocusPane::Sessions => FocusPane::Preview,
            FocusPane::Preview => FocusPane::Sessions,
        };
        debug_log(
            "focus_toggle",
            serde_json::json!({
                "before": format!("{:?}", before),
                "after": format!("{:?}", self.focus),
            }),
        );
    }

    fn toggle_session_view(&mut self) {
        let selected_key = self.current().map(AgentKey::new);
        let before = self.session_view;
        self.session_view = self.session_view.toggle();
        if let Some(key) = selected_key.as_ref() {
            self.select_visible_session(key);
            if self.list_state.selected().is_none() {
                self.select_first();
            }
        } else {
            self.select_first();
        }
        self.preview_scroll = 0;
        self.focus = FocusPane::Sessions;
        self.settings.cokacmux.session_view = self.session_view;
        self.status = match self.settings.save() {
            Ok(()) => format!("session view saved: {}", self.session_view.label()),
            Err(e) => format!(
                "session view changed: {} (save failed: {})",
                self.session_view.label(),
                e
            ),
        };
        debug_log(
            "session_view_toggle",
            serde_json::json!({
                "before": before.label(),
                "after": self.session_view.label(),
                "selected": selected_key.as_ref().map(|key| serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                })),
                "selected_index": self.list_state.selected(),
                "status": &self.status,
            }),
        );
    }

    fn toggle_preview_mode(&mut self) {
        let before = self.preview_mode;
        self.preview_mode = match self.preview_mode {
            Mode::Summary => Mode::Full,
            Mode::Full => Mode::Summary,
        };
        self.preview_scroll = 0;
        self.focus = FocusPane::Preview;
        self.status = format!("preview mode: {}", preview_mode_label(self.preview_mode));
        debug_log(
            "preview_mode_toggle",
            serde_json::json!({
                "before": preview_mode_label(before),
                "after": preview_mode_label(self.preview_mode),
            }),
        );
    }

    fn scroll_preview(&mut self, delta: i32) {
        let before = self.preview_scroll;
        let next = (self.preview_scroll as i32 + delta).max(0);
        self.preview_scroll = next.min(u16::MAX as i32) as u16;
        debug_log(
            "preview_scroll",
            serde_json::json!({
                "delta": delta,
                "before": before,
                "after": self.preview_scroll,
            }),
        );
    }

    fn scroll_preview_page(&mut self, pages: i32) {
        let page = self.preview_page_height.max(1) as i32;
        self.scroll_preview(page * pages);
    }

    fn preview_top(&mut self) {
        self.preview_scroll = 0;
        debug_log("preview_top", serde_json::json!({}));
    }

    fn preview_bottom(&mut self) {
        self.preview_scroll = u16::MAX;
        debug_log("preview_bottom", serde_json::json!({}));
    }

    fn poll_preview_results(&mut self) {
        while let Ok(result) = self.preview_rx.try_recv() {
            let stale_same_key = self
                .preview_requested
                .as_ref()
                .map(|(key, seq)| key == &result.key && result.seq < *seq)
                .unwrap_or(false);
            if stale_same_key {
                debug_log(
                    "preview_result_stale",
                    serde_json::json!({
                        "seq": result.seq,
                        "provider": result.key.provider.as_str(),
                        "session_id": &result.key.session_id,
                        "mode": preview_mode_label(result.key.mode),
                    }),
                );
                continue;
            }

            let completes_request = self
                .preview_requested
                .as_ref()
                .map(|(key, seq)| key == &result.key && *seq == result.seq)
                .unwrap_or(false);
            self.cache_preview(
                result.key.clone(),
                result.text,
                result.wrap_width,
                result.lines,
            );
            if completes_request {
                self.preview_requested = None;
            }
            debug_log(
                "preview_result_cached",
                serde_json::json!({
                    "seq": result.seq,
                    "provider": result.key.provider.as_str(),
                    "session_id": &result.key.session_id,
                    "mode": preview_mode_label(result.key.mode),
                    "wrap_width": result.wrap_width,
                    "lines": self.preview_cache.get(&result.key).map(|entry| entry.lines.len()),
                    "completes_request": completes_request,
                }),
            );
        }
    }

    fn request_preview(&mut self, info: SessionInfo, key: PreviewKey, width: usize) {
        let provider = info.provider;
        let session_id = info.session_id.clone();
        let mode = key.mode;
        if self.preview_cache.contains_key(&key) {
            trace_log(
                "preview_request_cache_hit",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "mode": preview_mode_label(key.mode),
                    "width": width,
                }),
            );
            return;
        }
        if self
            .preview_requested
            .as_ref()
            .map(|(requested_key, _)| requested_key == &key)
            .unwrap_or(false)
        {
            trace_log(
                "preview_request_already_pending",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "mode": preview_mode_label(key.mode),
                    "width": width,
                }),
            );
            return;
        }

        self.preview_seq = self.preview_seq.wrapping_add(1);
        let seq = self.preview_seq;
        let request = PreviewRequest {
            seq,
            key: key.clone(),
            info,
            width: width.max(1),
        };
        if self.preview_tx.send(request).is_ok() {
            self.preview_requested = Some((key, seq));
            debug_log(
                "preview_request_sent",
                serde_json::json!({
                    "seq": seq,
                    "provider": provider.as_str(),
                    "session_id": &session_id,
                    "mode": preview_mode_label(mode),
                    "width": width.max(1),
                }),
            );
        } else {
            let text = "preview worker stopped".to_string();
            let lines = wrap_preview_lines(&text, width);
            self.cache_preview(key, text, width.max(1), lines);
            self.preview_requested = None;
            debug_log(
                "preview_request_worker_stopped",
                serde_json::json!({
                    "seq": seq,
                    "provider": provider.as_str(),
                    "session_id": &session_id,
                    "width": width.max(1),
                }),
            );
        }
    }

    fn cache_preview(
        &mut self,
        key: PreviewKey,
        text: String,
        wrap_width: usize,
        lines: Vec<String>,
    ) {
        let line_count = lines.len();
        self.preview_cache_order.retain(|existing| existing != &key);
        self.preview_cache_order.push_back(key.clone());
        self.preview_cache
            .insert(key, PreviewEntry::new(text, wrap_width, lines));
        while self.preview_cache_order.len() > PREVIEW_CACHE_LIMIT {
            if let Some(old) = self.preview_cache_order.pop_front() {
                self.preview_cache.remove(&old);
                debug_log(
                    "preview_cache_evict",
                    serde_json::json!({
                        "provider": old.provider.as_str(),
                        "session_id": &old.session_id,
                        "mode": preview_mode_label(old.mode),
                    }),
                );
            }
        }
        debug_log(
            "preview_cache_store",
            serde_json::json!({
                "provider": self.preview_cache_order.back().map(|key| key.provider.as_str()),
                "session_id": self.preview_cache_order.back().map(|key| key.session_id.as_str()),
                "wrap_width": wrap_width,
                "lines": line_count,
                "entries": self.preview_cache_order.len(),
            }),
        );
    }

    fn clear_preview_cache(&mut self) {
        let entries = self.preview_cache.len();
        self.preview_cache.clear();
        self.preview_cache_order.clear();
        self.preview_requested = None;
        debug_log(
            "preview_cache_clear",
            serde_json::json!({
                "entries": entries,
            }),
        );
    }

    fn drop_cached_preview(&mut self, key: &PreviewKey) {
        let removed = self.preview_cache.remove(key).is_some();
        self.preview_cache_order.retain(|existing| existing != key);
        debug_log(
            "preview_cache_drop",
            serde_json::json!({
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
                "mode": preview_mode_label(key.mode),
                "removed": removed,
            }),
        );
    }

    fn ensure_wrapped_preview(&mut self, key: &PreviewKey, width: usize) -> Option<usize> {
        let entry = self.preview_cache.get_mut(key)?;
        let width = width.max(1);
        if entry.wrap_width != width {
            let before = entry.wrap_width;
            entry.lines = wrap_preview_lines(&entry.text, width);
            entry.wrap_width = width;
            debug_log(
                "preview_rewrap",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "mode": preview_mode_label(key.mode),
                    "before_width": before,
                    "after_width": width,
                    "lines": entry.lines.len(),
                }),
            );
        }
        Some(entry.lines.len())
    }

    fn preview_visible_lines(&self, key: &PreviewKey, start: usize, height: usize) -> Vec<String> {
        self.preview_cache
            .get(key)
            .map(|entry| {
                entry
                    .lines
                    .iter()
                    .skip(start)
                    .take(height)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Reap the active agent if its reader thread (or daemon) signaled
    /// exit. Events themselves arrive via the main-loop channel — this
    /// method no longer pulls from the socket; it only reacts to flags
    /// the channel-driven `process_agent_event` already set.
    fn poll_agent_sessions(&mut self) {
        let Some(agent) = self.active_agent.as_mut() else {
            return;
        };
        if let Some(exit_status) = agent.exited.clone() {
            let provider = agent.info.provider;
            let session_id = agent.info.session_id.clone();
            self.active_agent = None;
            self.status = format!(
                "{} agent {} exited ({})",
                provider.as_str(),
                truncate_width(&session_id, 14),
                exit_status
            );
            self.refresh();
            self.refresh_agent_runtime_states();
        }
    }

    /// Allocate the next reader id and return a clone of the main-loop
    /// sender so we can build a new `AgentClient`. Must be called from a
    /// context where `main_tx` has been installed (i.e. inside `run`).
    fn agent_attach_handles(&mut self) -> (Sender<MainEvent>, u64) {
        let tx = self
            .main_tx
            .clone()
            .expect("main_tx not installed before agent attach");
        let id = self.next_reader_id;
        self.next_reader_id = self.next_reader_id.saturating_add(1);
        (tx, id)
    }

    /// Apply a `MainEvent::AgentEvent` to the active agent if it still
    /// matches `reader_id`. Late events from a replaced agent are silently
    /// dropped — that AgentClient's Drop already joined its reader.
    fn on_agent_event(&mut self, reader_id: u64, event: AgentDaemonEvent) {
        let Some(agent) = self.active_agent.as_mut() else {
            return;
        };
        if agent.reader_id != reader_id {
            return;
        }
        agent.process_agent_event(event);
    }

    /// Apply a reader-thread exit signal. Treated like a daemon-disconnect.
    fn on_agent_reader_ended(&mut self, reader_id: u64, reason: String) {
        let Some(agent) = self.active_agent.as_mut() else {
            return;
        };
        if agent.reader_id != reader_id {
            return;
        }
        if agent.exited.is_none() {
            agent.exited = Some(format!("daemon connection ended: {}", reason));
        }
    }

    /// Open a shell pane at the cwd of the currently focused session. The
    /// shell runs through the same AgentClient/daemon infrastructure as the
    /// real agents (PTY + vt100 + Unix socket), keyed by a unique synthetic
    /// session id. Ctrl+] / Ctrl+[ switches back to the sessions screen, Ctrl+K kills.
    /// Returns the cwd that should be used when the user spawns a new
    /// shell pane while an agent is in the active view. For real agents
    /// it's the session's cwd; for shell panes it's the *live* cwd as
    /// recorded by the daemon's most recent meta write (so `cd ..` inside
    /// the shell is reflected when Ctrl+N is pressed again from there).
    fn active_agent_current_cwd(&self) -> Option<String> {
        let agent = self.active_agent.as_ref()?;
        if is_shell_session_info(&agent.info) {
            let key = AgentKey::new(&agent.info);
            for shell in &self.live_shells {
                if AgentKey::new(shell) == key && !shell.cwd.is_empty() {
                    return Some(shell.cwd.clone());
                }
            }
        }
        if agent.info.cwd.is_empty() {
            None
        } else {
            Some(agent.info.cwd.clone())
        }
    }

    /// Common shell-spawn path: build a new shell `SessionInfo` at `cwd`,
    /// install it as the active agent, and switch to the agents view.
    fn open_shell_at_cwd(&mut self, cwd: String, cols: u16, rows: u16, origin_tag: &str) {
        if cwd.is_empty() {
            self.status = "no cwd to open a shell at.".into();
            return;
        }
        let info = shell_session_info_for_cwd(cwd.clone());
        let key = AgentKey::new(&info);
        debug_log(
            "shell_open_start",
            serde_json::json!({
                "session_id": &info.session_id,
                "cwd": &cwd,
                "origin": origin_tag,
            }),
        );
        let (main_tx, reader_id) = self.agent_attach_handles();
        match AgentClient::attach_or_start(
            info,
            cols,
            rows,
            main_tx,
            reader_id,
            AgentLaunchMode::Normal,
        ) {
            Ok((agent, started)) => {
                self.status = format!(
                    "{} shell at {}",
                    if started { "started" } else { "attached" },
                    truncate_width(&cwd, 40),
                );
                self.active_agent = Some(agent);
                self.show_sessions_view = false;
                self.mark_agent_attached_locally(key.clone());
                self.refresh_agent_runtime_states();
                debug_log(
                    "shell_open_ok",
                    serde_json::json!({
                        "session_id": &key.session_id,
                        "started": started,
                    }),
                );
            }
            Err(e) => {
                self.status = format!("shell open failed: {}", e);
                debug_log(
                    "shell_open_failed",
                    serde_json::json!({
                        "session_id": &key.session_id,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn begin_new_session_from_focused(&mut self) {
        let cwd = self
            .current()
            .map(|info| info.cwd.clone())
            .filter(|cwd| !cwd.is_empty())
            .or_else(process_cwd_string)
            .unwrap_or_default();
        let provider = self
            .current()
            .map(|info| info.provider)
            .unwrap_or(Provider::Codex);
        self.begin_new_session(cwd, provider, "sessions");
    }

    fn begin_new_session_from_active_agent(&mut self) {
        let cwd = self
            .active_agent_current_cwd()
            .or_else(process_cwd_string)
            .unwrap_or_default();
        let provider = self
            .active_agent
            .as_ref()
            .map(|agent| agent.info.provider)
            .filter(|_| {
                self.active_agent
                    .as_ref()
                    .is_some_and(|agent| !is_shell_session_info(&agent.info))
            })
            .unwrap_or(Provider::Codex);
        self.begin_new_session(cwd, provider, "agent");
    }

    fn begin_new_session(&mut self, cwd: String, provider: Provider, origin: &str) {
        debug_log(
            "new_session_open",
            serde_json::json!({
                "origin": origin,
                "cwd": &cwd,
                "provider": provider.as_str(),
            }),
        );
        let cwd_cursor = cwd.len();
        self.status = "choose what to start.".into();
        let provider_options =
            available_agent_provider_options(&self.settings.cokacmux.agent_programs);
        let provider =
            normalize_agent_provider_selection(provider, &provider_options).unwrap_or(provider);
        self.input_mode = InputMode::NewSession {
            selected: NEW_SESSION_FIELD_KIND,
            kind: NewSessionKind::Terminal,
            cwd,
            cwd_cursor,
            provider,
            provider_options,
            launch_mode: AgentLaunchMode::Normal,
        };
    }

    fn start_new_session_from_modal(
        &mut self,
        kind: NewSessionKind,
        cwd: String,
        provider: Provider,
        launch_mode: AgentLaunchMode,
        cols: u16,
        rows: u16,
    ) -> bool {
        if kind == NewSessionKind::CodingAgent
            && !agent_provider_available(provider, &self.settings.cokacmux.agent_programs)
        {
            self.status = format!("{} agent is not installed.", provider.as_str());
            debug_log(
                "new_session_provider_unavailable",
                serde_json::json!({
                    "provider": provider.as_str(),
                }),
            );
            return false;
        }
        let cwd = match normalize_launch_cwd(&cwd) {
            Ok(cwd) => cwd,
            Err(message) => {
                self.status = message;
                debug_log(
                    "new_session_normalize_cwd_failed",
                    serde_json::json!({
                        "kind": kind.as_str(),
                        "provider": provider.as_str(),
                        "launch_mode": launch_mode.as_str(),
                        "status": &self.status,
                    }),
                );
                return false;
            }
        };
        debug_log(
            "new_session_start",
            serde_json::json!({
                "kind": kind.as_str(),
                "cwd": &cwd,
                "provider": provider.as_str(),
                "launch_mode": launch_mode.as_str(),
                "cols": cols,
                "rows": rows,
            }),
        );
        match kind {
            NewSessionKind::Terminal => {
                self.open_shell_at_cwd(cwd, cols, rows, "new_session:terminal");
            }
            NewSessionKind::CodingAgent => {
                let info = new_agent_session_info(provider, cwd.clone());
                debug_log(
                    "new_agent_open_start",
                    serde_json::json!({
                        "provider": provider.as_str(),
                        "session_id": &info.session_id,
                        "cwd": &cwd,
                        "launch_mode": launch_mode.as_str(),
                    }),
                );
                self.attach_agent(info, cols, rows, launch_mode);
                debug_log(
                    "new_agent_open_after_attach",
                    serde_json::json!({
                        "provider": provider.as_str(),
                        "cwd": &cwd,
                        "launch_mode": launch_mode.as_str(),
                        "active_agent": self.active_agent.as_ref().map(|agent| session_info_debug_value(&agent.info)),
                        "show_sessions_view": self.show_sessions_view,
                        "agent_states": agent_state_entries_debug_value(&self.agent_states),
                    }),
                );
            }
        }
        true
    }

    fn begin_agent_launch(&mut self, cols: u16, rows: u16) {
        let Some(info) = self.current().cloned() else {
            self.status = "no session selected.".into();
            return;
        };
        if self.main_tx.is_some() {
            self.refresh_agent_runtime_states();
        }
        let runtime_info = self.runtime_info_for_selected_agent(&info);
        let key = AgentKey::new(&runtime_info);
        if self
            .active_agent
            .as_ref()
            .is_some_and(|agent| AgentKey::new(&agent.info) == key)
        {
            self.show_sessions_view = false;
            self.status = format!(
                "switched to active {}",
                live_agent_status_label(&runtime_info)
            );
            debug_log(
                "agent_launch_switch_active",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                }),
            );
            return;
        }
        match self
            .agent_states
            .get(&key)
            .copied()
            .unwrap_or(AgentListState::Idle)
        {
            AgentListState::Live { .. } | AgentListState::Attached { mine: true, .. } => {
                self.attach_existing_live_agent(runtime_info, cols, rows, "agent_launch_live");
                return;
            }
            AgentListState::Attached { mine: false, .. } => {
                self.status = format!(
                    "{} is already attached in another cokacmux process.",
                    live_agent_status_label(&runtime_info)
                );
                debug_log(
                    "agent_launch_blocked_foreign_attached",
                    serde_json::json!({
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                    }),
                );
                return;
            }
            AgentListState::Idle => {}
        }
        let key = AgentKey::new(&info);
        debug_log(
            "agent_launch_mode_open",
            serde_json::json!({
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
            }),
        );
        self.focus = FocusPane::Sessions;
        self.status = "choose launch mode.".into();
        self.input_mode = InputMode::AgentLaunch {
            source: info,
            selected: 0,
        };
    }

    fn attach_existing_live_agent(
        &mut self,
        info: SessionInfo,
        cols: u16,
        rows: u16,
        origin: &str,
    ) {
        let key = AgentKey::new(&info);
        let label = live_agent_status_label(&info);
        let should_select_visible =
            !is_shell_session_info(&info) && !is_new_agent_session_info(&info);
        debug_log(
            "attach_existing_live_agent_start",
            serde_json::json!({
                "origin": origin,
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
                "cols": cols,
                "rows": rows,
            }),
        );
        if self.main_tx.is_none() {
            self.status = format!("cannot switch to live {}; event loop is not ready", label);
            debug_log(
                "attach_existing_live_agent_unavailable",
                serde_json::json!({
                    "origin": origin,
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                }),
            );
            return;
        }
        let (main_tx, reader_id) = self.agent_attach_handles();
        match AgentClient::attach_existing(info, cols, rows, main_tx, reader_id) {
            Ok(agent) => {
                self.status = format!("switched to live {}", label);
                self.show_sessions_view = false;
                self.active_agent = Some(agent);
                self.mark_agent_attached_locally(key.clone());
                if should_select_visible {
                    self.select_visible_session(&key);
                }
                self.refresh_agent_runtime_states();
                debug_log(
                    "attach_existing_live_agent_ready",
                    serde_json::json!({
                        "origin": origin,
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                    }),
                );
            }
            Err(e) => {
                self.status = format!("switch to live {} failed: {}", label, e);
                self.refresh_agent_runtime_states();
                debug_log(
                    "attach_existing_live_agent_failed",
                    serde_json::json!({
                        "origin": origin,
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn prompt_create_missing_launch_cwd(
        &mut self,
        info: SessionInfo,
        path: PathBuf,
        cols: u16,
        rows: u16,
        launch_mode: AgentLaunchMode,
    ) {
        let key = AgentKey::new(&info);
        let yes_key = self.keybindings.help(KeyAction::ConfirmYes, "y");
        let no_key = self.keybindings.help(KeyAction::ConfirmNo, "N");
        self.status = format!(
            "launch folder missing: {}",
            truncate_width(&path.display().to_string(), 48)
        );
        self.input_mode = InputMode::Confirm {
            prompt: create_missing_cwd_confirm_prompt(&info, &path, &yes_key, &no_key),
            action: PendingAction::CreateMissingLaunchCwd {
                info,
                path: path.clone(),
                cols,
                rows,
                launch_mode,
            },
        };
        debug_log(
            "attach_launch_cwd_create_confirm_open",
            serde_json::json!({
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
                "cwd": path.display().to_string(),
                "launch_mode": launch_mode.as_str(),
            }),
        );
    }

    fn create_missing_launch_cwd_and_attach(
        &mut self,
        info: SessionInfo,
        path: PathBuf,
        cols: u16,
        rows: u16,
        launch_mode: AgentLaunchMode,
    ) {
        let key = AgentKey::new(&info);
        match create_agent_launch_cwd(&path) {
            Ok(()) => {
                debug_log(
                    "attach_launch_cwd_created",
                    serde_json::json!({
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                        "cwd": path.display().to_string(),
                        "launch_mode": launch_mode.as_str(),
                    }),
                );
                self.attach_agent(info, cols, rows, launch_mode);
            }
            Err(e) => {
                self.status = format!(
                    "create launch folder failed: {}",
                    truncate_width(&e.to_string(), 72)
                );
                debug_log(
                    "attach_launch_cwd_create_failed",
                    serde_json::json!({
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                        "cwd": path.display().to_string(),
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn attach_agent(
        &mut self,
        info: SessionInfo,
        cols: u16,
        rows: u16,
        launch_mode: AgentLaunchMode,
    ) {
        let key = AgentKey::new(&info);
        debug_log(
            "attach_selected",
            serde_json::json!({
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
                "launch_mode": launch_mode.as_str(),
            }),
        );
        if live_agent_meta_snapshot(&key).is_none() {
            match missing_session_launch_cwd(&info) {
                Ok(Some(path)) => {
                    self.prompt_create_missing_launch_cwd(info, path, cols, rows, launch_mode);
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    self.status = format!(
                        "attach {} agent {} failed: {}",
                        key.provider.as_str(),
                        truncate_width(&key.session_id, 14),
                        e
                    );
                    debug_log(
                        "attach_launch_cwd_invalid",
                        serde_json::json!({
                            "provider": key.provider.as_str(),
                            "session_id": &key.session_id,
                            "cwd": &info.cwd,
                            "error": e.to_string(),
                        }),
                    );
                    return;
                }
            }
        }
        if let Err(e) = prepare_agent_session(&info) {
            self.status = format!(
                "prepare {} agent {} failed: {}",
                key.provider.as_str(),
                truncate_width(&key.session_id, 14),
                e
            );
            debug_log(
                "attach_prepare_failed",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "error": e.to_string(),
                }),
            );
            return;
        }
        let (main_tx, reader_id) = self.agent_attach_handles();
        match AgentClient::attach_or_start(info, cols, rows, main_tx, reader_id, launch_mode) {
            Ok((agent, started)) => {
                self.status = format!(
                    "{} {} agent {}{}",
                    if started { "started" } else { "attached" },
                    key.provider.as_str(),
                    truncate_width(&key.session_id, 14),
                    if started && launch_mode == AgentLaunchMode::SkipPermissions {
                        " with skipped permissions"
                    } else {
                        ""
                    }
                );
                self.show_sessions_view = false;
                self.active_agent = Some(agent);
                self.mark_agent_attached_locally(key.clone());
                self.refresh_agent_runtime_states();
                debug_log(
                    "attach_ready",
                    serde_json::json!({
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                        "started": started,
                        "launch_mode": launch_mode.as_str(),
                        "show_sessions_view": self.show_sessions_view,
                        "agent_states": agent_state_entries_debug_value(&self.agent_states),
                        "live_shells": self.live_shells.iter().map(session_info_debug_value).collect::<Vec<_>>(),
                    }),
                );
            }
            Err(e) => {
                self.status = format!(
                    "attach {} agent {} failed: {}",
                    key.provider.as_str(),
                    truncate_width(&key.session_id, 14),
                    e
                );
                self.refresh_agent_runtime_states();
                debug_log(
                    "attach_failed",
                    serde_json::json!({
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                        "error": e.to_string(),
                        "agent_states": agent_state_entries_debug_value(&self.agent_states),
                    }),
                );
            }
        }
    }

    /// True when the agent pane is the visible view (active agent
    /// exists AND user hasn't toggled to the sessions list with Ctrl+] / Ctrl+[).
    fn is_agent_view(&self) -> bool {
        self.active_agent.is_some() && !self.show_sessions_view
    }

    fn prepare_sessions_view_after_transition(
        &mut self,
        previous_is_agent_view: &mut bool,
        reason: &str,
    ) {
        let is_agent_view = self.is_agent_view();
        if !is_agent_view && *previous_is_agent_view {
            self.prepare_sessions_view(reason);
        }
        *previous_is_agent_view = is_agent_view;
    }

    /// Window toggle. Flips between sessions list and the active
    /// agent pane *without* changing connection state: the daemon, socket
    /// and reader thread all keep running while the sessions list is
    /// shown, so toggling back is instant and the screen is up to date.
    /// If no agent is currently active, reconnect to an already-live
    /// daemon when one is known. Ctrl+] / Ctrl+[ still does not start a new daemon.
    fn toggle_screens(&mut self, cols: u16, rows: u16) {
        if self.active_agent.is_some() {
            let before = self.show_sessions_view;
            self.show_sessions_view = !self.show_sessions_view;
            debug_log(
                "toggle_screens_active_agent",
                serde_json::json!({
                    "before_show_sessions_view": before,
                    "after_show_sessions_view": self.show_sessions_view,
                    "active_agent": self.active_agent.as_ref().map(|agent| session_info_debug_value(&agent.info)),
                    "agent_states": agent_state_entries_debug_value(&self.agent_states),
                }),
            );
            return;
        }
        debug_log(
            "toggle_screens_restore_attempt",
            serde_json::json!({
                "cols": cols,
                "rows": rows,
                "agent_states": agent_state_entries_debug_value(&self.agent_states),
            }),
        );
        self.restore_live_agent(cols, rows);
    }

    fn restore_live_agent(&mut self, cols: u16, rows: u16) {
        if self.main_tx.is_none() {
            self.status = "no active agent to switch to; press e to start selected agent".into();
            return;
        }
        self.refresh_agent_runtime_states();
        let Some(info) = self.live_agent_restore_candidate() else {
            self.status = "no active agent to switch to; press e to start selected agent".into();
            return;
        };
        let key = AgentKey::new(&info);
        let label = live_agent_status_label(&info);
        let should_select_visible =
            !is_shell_session_info(&info) && !is_new_agent_session_info(&info);
        debug_log(
            "restore_live_agent_selected",
            serde_json::json!({
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
                "shell": is_shell_session_info(&info),
            }),
        );
        let (main_tx, reader_id) = self.agent_attach_handles();
        match AgentClient::attach_existing(info, cols, rows, main_tx, reader_id) {
            Ok(agent) => {
                self.status = format!("switched to live {}", label);
                self.show_sessions_view = false;
                self.active_agent = Some(agent);
                self.mark_agent_attached_locally(key.clone());
                if should_select_visible {
                    self.select_visible_session(&key);
                }
                self.refresh_agent_runtime_states();
                debug_log(
                    "restore_live_agent_ready",
                    serde_json::json!({
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                    }),
                );
            }
            Err(e) => {
                self.status = format!("switch to live {} failed: {}", label, e);
                self.refresh_agent_runtime_states();
                debug_log(
                    "restore_live_agent_failed",
                    serde_json::json!({
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn detach_agent(&mut self) {
        if let Some(agent) = self.active_agent.take() {
            let provider = agent.info.provider;
            let session_id = agent.info.session_id.clone();
            drop(agent);
            self.status = format!(
                "detached {} agent {}",
                provider.as_str(),
                truncate_width(&session_id, 14)
            );
            self.refresh_agent_runtime_states();
            debug_log(
                "detach_agent",
                serde_json::json!({
                    "provider": provider.as_str(),
                    "session_id": session_id,
                }),
            );
        }
    }

    fn kill_active_agent(&mut self) {
        let Some(active_agent) = self.active_agent.as_ref() else {
            return;
        };
        let key = AgentKey::new(&active_agent.info);
        let provider = active_agent.info.provider;
        let session_id = active_agent.info.session_id.clone();
        let cols = active_agent.pty_size.cols;
        let rows = active_agent.pty_size.rows;
        let next_info = self.next_agent_after_current();

        let Some(agent) = self.active_agent.take() else {
            return;
        };
        drop(agent);

        let result = terminate_agent_daemon(&key);
        self.agent_states.remove(&key);
        self.refresh_agent_runtime_states();

        match result {
            Ok(outcome) => {
                let history_suffix = agent_history_deleted_suffix(outcome.pty_log_deleted);
                debug_log(
                    "kill_agent",
                    serde_json::json!({
                        "provider": provider.as_str(),
                        "session_id": &session_id,
                        "daemon_pid": outcome.pid,
                        "pty_log_deleted": outcome.pty_log_deleted,
                        "has_next": next_info.is_some(),
                    }),
                );
                match outcome.pid {
                    Some(pid) => {
                        if let Some(next_info) = next_info {
                            self.attach_after_agent_kill(
                                next_info,
                                cols,
                                rows,
                                provider,
                                &session_id,
                                pid,
                            );
                            if outcome.pty_log_deleted {
                                self.status.push_str("; history deleted");
                            }
                        } else {
                            self.status = format!(
                                "killed {} agent {} (pid {}){}",
                                provider.as_str(),
                                truncate_width(&session_id, 14),
                                pid,
                                history_suffix
                            );
                        }
                    }
                    None => {
                        if let Some(next_info) = next_info {
                            self.attach_after_agent_kill(
                                next_info,
                                cols,
                                rows,
                                provider,
                                &session_id,
                                0,
                            );
                            if outcome.pty_log_deleted {
                                self.status.push_str("; history deleted");
                            }
                        } else {
                            self.status = format!(
                                "cleared {} agent {} runtime{}",
                                provider.as_str(),
                                truncate_width(&session_id, 14),
                                history_suffix
                            );
                        }
                    }
                }
            }
            Err(e) => {
                self.status = format!(
                    "kill {} agent {} failed: {}",
                    provider.as_str(),
                    truncate_width(&session_id, 14),
                    e
                );
                debug_log(
                    "kill_agent_failed",
                    serde_json::json!({
                        "provider": provider.as_str(),
                        "session_id": &session_id,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn next_agent_after_current(&mut self) -> Option<SessionInfo> {
        let active_agent = self.active_agent.as_ref()?;
        let current_key = AgentKey::new(&active_agent.info);
        self.refresh_agent_runtime_states();
        let candidates = self.live_agent_switch_candidates();
        if candidates.len() <= 1 {
            return None;
        }
        let current_index = candidates
            .iter()
            .position(|info| AgentKey::new(info) == current_key)?;
        for offset in 1..candidates.len() {
            let next_index = (current_index + offset) % candidates.len();
            let next_info = candidates[next_index].clone();
            if AgentKey::new(&next_info) != current_key {
                return Some(next_info);
            }
        }
        None
    }

    fn attach_after_agent_kill(
        &mut self,
        next_info: SessionInfo,
        cols: u16,
        rows: u16,
        killed_provider: Provider,
        killed_session_id: &str,
        killed_pid: u32,
    ) {
        let next_key = AgentKey::new(&next_info);
        debug_log(
            "kill_agent_switch_selected",
            serde_json::json!({
                "from_provider": killed_provider.as_str(),
                "from_session_id": killed_session_id,
                "to_provider": next_key.provider.as_str(),
                "to_session_id": &next_key.session_id,
                "daemon_pid": killed_pid,
            }),
        );
        if let Err(e) = prepare_agent_session(&next_info) {
            self.status = format!(
                "killed {} agent {}; prepare next {} failed: {}",
                killed_provider.as_str(),
                truncate_width(killed_session_id, 14),
                truncate_width(&next_key.session_id, 14),
                e
            );
            debug_log(
                "kill_agent_switch_prepare_failed",
                serde_json::json!({
                    "provider": next_key.provider.as_str(),
                    "session_id": &next_key.session_id,
                    "error": e.to_string(),
                }),
            );
            return;
        }

        let should_select_visible = !is_new_agent_session_info(&next_info);
        let (main_tx, reader_id) = self.agent_attach_handles();
        match AgentClient::attach_or_start(
            next_info,
            cols,
            rows,
            main_tx,
            reader_id,
            AgentLaunchMode::Normal,
        ) {
            Ok((next_agent, started)) => {
                self.status = format!(
                    "killed {} {}; switched to {} {} agent {}",
                    killed_provider.as_str(),
                    truncate_width(killed_session_id, 14),
                    if started { "started" } else { "live" },
                    next_key.provider.as_str(),
                    truncate_width(&next_key.session_id, 14)
                );
                self.show_sessions_view = false;
                self.active_agent = Some(next_agent);
                self.mark_agent_attached_locally(next_key.clone());
                if should_select_visible {
                    self.select_visible_session(&next_key);
                }
                self.refresh_agent_runtime_states();
                debug_log(
                    "kill_agent_switch_ready",
                    serde_json::json!({
                        "provider": next_key.provider.as_str(),
                        "session_id": &next_key.session_id,
                        "started": started,
                    }),
                );
            }
            Err(e) => {
                self.status = format!(
                    "killed {} {}; switch to {} failed: {}",
                    killed_provider.as_str(),
                    truncate_width(killed_session_id, 14),
                    truncate_width(&next_key.session_id, 14),
                    e
                );
                self.refresh_agent_runtime_states();
                debug_log(
                    "kill_agent_switch_failed",
                    serde_json::json!({
                        "provider": next_key.provider.as_str(),
                        "session_id": &next_key.session_id,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn kill_selected_agent(&mut self) {
        let Some(info) = self.current().cloned() else {
            self.status = "no session selected.".into();
            return;
        };
        let selected_key = AgentKey::new(&info);
        let runtime_info = self.runtime_info_for_selected_agent(&info);
        let runtime_key = AgentKey::new(&runtime_info);
        let result = terminate_agent_daemon(&runtime_key);
        self.agent_states.remove(&selected_key);
        self.agent_states.remove(&runtime_key);
        self.refresh_agent_runtime_states();

        match result {
            Ok(outcome) => {
                let history_suffix = agent_history_deleted_suffix(outcome.pty_log_deleted);
                match outcome.pid {
                    Some(pid) => {
                        self.status = format!(
                            "killed {} agent {} (pid {}){}",
                            info.provider.as_str(),
                            truncate_width(&info.session_id, 14),
                            pid,
                            history_suffix
                        );
                    }
                    None => {
                        self.status = format!(
                            "no live {} agent for {}{}",
                            info.provider.as_str(),
                            truncate_width(&info.session_id, 14),
                            history_suffix
                        );
                    }
                }
                debug_log(
                    "kill_selected_agent",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "runtime_provider": runtime_key.provider.as_str(),
                        "runtime_session_id": &runtime_key.session_id,
                        "daemon_pid": outcome.pid,
                        "pty_log_deleted": outcome.pty_log_deleted,
                    }),
                );
            }
            Err(e) => {
                self.status = format!(
                    "kill {} agent {} failed: {}",
                    info.provider.as_str(),
                    truncate_width(&info.session_id, 14),
                    e
                );
                debug_log(
                    "kill_selected_agent_failed",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "runtime_provider": runtime_key.provider.as_str(),
                        "runtime_session_id": &runtime_key.session_id,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn switch_active_agent(&mut self, delta: i32, wrap: bool) {
        let Some(active_agent) = self.active_agent.as_ref() else {
            return;
        };
        let current_key = AgentKey::new(&active_agent.info);
        let cols = active_agent.pty_size.cols;
        let rows = active_agent.pty_size.rows;

        self.refresh_agent_runtime_states();
        let candidates = self.live_agent_switch_candidates();
        if candidates.len() <= 1 {
            self.status = "no other live agent.".into();
            debug_log(
                "agent_switch_no_target",
                serde_json::json!({
                    "provider": current_key.provider.as_str(),
                    "session_id": &current_key.session_id,
                }),
            );
            return;
        }

        let Some(current_index) = candidates
            .iter()
            .position(|info| AgentKey::new(info) == current_key)
        else {
            self.status = "active agent is not in session list.".into();
            return;
        };
        let next_index = next_agent_candidate_index(candidates.len(), current_index, delta, wrap);
        let next_info = candidates[next_index].clone();
        let next_key = AgentKey::new(&next_info);
        if next_key == current_key {
            self.status = "agent selection limit.".into();
            return;
        }

        debug_log(
            "agent_switch_selected",
            serde_json::json!({
                "from_provider": current_key.provider.as_str(),
                "from_session_id": &current_key.session_id,
                "to_provider": next_key.provider.as_str(),
                "to_session_id": &next_key.session_id,
                "delta": delta,
            }),
        );
        if let Err(e) = prepare_agent_session(&next_info) {
            self.status = format!(
                "prepare {} agent {} failed: {}",
                next_key.provider.as_str(),
                truncate_width(&next_key.session_id, 14),
                e
            );
            debug_log(
                "agent_switch_prepare_failed",
                serde_json::json!({
                    "provider": next_key.provider.as_str(),
                    "session_id": &next_key.session_id,
                    "error": e.to_string(),
                }),
            );
            return;
        }

        let should_select_visible = !is_new_agent_session_info(&next_info);
        let (main_tx, reader_id) = self.agent_attach_handles();
        match AgentClient::attach_or_start(
            next_info,
            cols,
            rows,
            main_tx,
            reader_id,
            AgentLaunchMode::Normal,
        ) {
            Ok((next_agent, started)) => {
                if let Some(old_agent) = self.active_agent.take() {
                    drop(old_agent);
                }
                self.status = format!(
                    "switched to {} {} agent {}",
                    if started { "started" } else { "live" },
                    next_key.provider.as_str(),
                    truncate_width(&next_key.session_id, 14)
                );
                self.show_sessions_view = false;
                self.active_agent = Some(next_agent);
                self.mark_agent_attached_locally(next_key.clone());
                if should_select_visible {
                    self.select_visible_session(&next_key);
                }
                self.refresh_agent_runtime_states();
                debug_log(
                    "agent_switch_ready",
                    serde_json::json!({
                        "provider": next_key.provider.as_str(),
                        "session_id": &next_key.session_id,
                        "started": started,
                    }),
                );
            }
            Err(e) => {
                self.status = format!(
                    "switch to {} agent {} failed: {}",
                    next_key.provider.as_str(),
                    truncate_width(&next_key.session_id, 14),
                    e
                );
                self.refresh_agent_runtime_states();
                debug_log(
                    "agent_switch_failed",
                    serde_json::json!({
                        "provider": next_key.provider.as_str(),
                        "session_id": &next_key.session_id,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn send_key_to_active_agent(&mut self, key: KeyEvent) {
        if let Some(agent) = self.active_agent.as_mut() {
            agent.send_key(key);
        }
    }

    fn scroll_active_agent_screen(&mut self, action: AgentScrollAction, page_rows: usize) {
        let Some(agent) = self.active_agent.as_mut() else {
            return;
        };
        let before = agent.scrollback_offset();
        let after = agent.scroll_screen(action, page_rows);
        let parser_scrollback = agent.parser.screen().scrollback();
        let parser_scrollback_max = parser_max_scrollback(&mut agent.parser);
        let history_scroll_offset = agent.history_scroll_offset;
        let screen_history_lines = agent.screen_history.len();
        self.status = if before == 0 && after == 0 && agent_scroll_action_moves_up(action) {
            "PTY scrollback: no saved lines.".into()
        } else if after == 0 {
            "PTY scrollback: bottom.".into()
        } else if before == after && agent_scroll_action_moves_up(action) {
            format!("PTY scrollback: top, {} lines up.", after)
        } else {
            format!("PTY scrollback: {} lines up.", after)
        };
        debug_log(
            "agent_scrollback",
            serde_json::json!({
                "provider": agent.info.provider.as_str(),
                "session_id": &agent.info.session_id,
                "action": format!("{:?}", action),
                "before": before,
                "after": after,
                "parser_scrollback": parser_scrollback,
                "parser_scrollback_max": parser_scrollback_max,
                "history_scroll_offset": history_scroll_offset,
                "screen_history_lines": screen_history_lines,
            }),
        );
    }

    fn begin_title_edit(&mut self) {
        let Some(info) = self.current().cloned() else {
            self.status = "no session selected.".into();
            debug_log("title_edit_no_selection", serde_json::json!({}));
            return;
        };
        self.focus = FocusPane::Sessions;
        self.status = "editing title.".into();
        let draft = info.title.clone().unwrap_or_default();
        let cursor = draft.len();
        debug_log(
            "title_edit_begin",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "title_len": draft.len(),
            }),
        );
        self.input_mode = InputMode::TitleEdit {
            source: info,
            draft,
            cursor,
        };
    }

    fn set_session_title(&mut self, info: SessionInfo, title: String) {
        let title = title.trim().to_string();
        match session::title::set_title(&info, &title) {
            Ok(()) => {
                let key = AgentKey::new(&info);
                let status = if title.is_empty() {
                    format!("cleared title for {}", truncate_width(&info.session_id, 14))
                } else {
                    format!(
                        "title saved for {}: {}",
                        truncate_width(&info.session_id, 14),
                        truncate_width(&title, 40)
                    )
                };
                self.refresh();
                self.select_visible_session(&key);
                self.status = status;
                debug_log(
                    "title_save_ok",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "title_len": title.len(),
                        "cleared": title.is_empty(),
                    }),
                );
            }
            Err(e) => {
                self.status = format!("title save failed: {}", e);
                debug_log(
                    "title_save_failed",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "title_len": title.len(),
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn delete_session(&mut self, info: SessionInfo, removed_index: Option<usize>) {
        debug_log(
            "delete_start",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "cwd": &info.cwd,
            }),
        );
        match session::remove::remove(&info) {
            Ok(rep) => {
                let status = format!(
                    "deleted {} (rows={}, file={:?})",
                    info.session_id, rep.deleted_rows, rep.deleted_file
                );
                self.refresh();
                let next_selection =
                    selection_index_after_removed_row(self.visible().len(), removed_index);
                self.list_state.select(next_selection);
                self.preview_scroll = 0;
                self.status = status;
                debug_log(
                    "delete_ok",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "deleted_rows": rep.deleted_rows,
                        "deleted_file": rep.deleted_file,
                        "removed_index": removed_index,
                        "selected_index": self.list_state.selected(),
                    }),
                );
            }
            Err(e) => {
                self.status = format!("delete failed: {}", e);
                debug_log(
                    "delete_failed",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    fn clone_session_to(&mut self, info: SessionInfo, target: Provider) {
        debug_log(
            "clone_start",
            serde_json::json!({
                "source_provider": info.provider.as_str(),
                "source_session_id": &info.session_id,
                "target_provider": target.as_str(),
                "cwd": &info.cwd,
            }),
        );
        match session::clone::clone_to_live(
            &info,
            &session::clone::CloneOpts {
                to: Some(target),
                ..Default::default()
            },
        ) {
            Ok(rep) => {
                let mut status = format!(
                    "cloned {} -> {}: {}",
                    info.provider.as_str(),
                    rep.target_provider.as_str(),
                    rep.new_session_id,
                );
                let clone_tree_error = session::clone_tree::record_clone_report(&rep).err();
                if let Some(e) = &clone_tree_error {
                    status.push_str(&format!(" (clone tree save failed: {})", e));
                }
                self.refresh();
                let new_key = AgentKey {
                    provider: rep.target_provider,
                    session_id: rep.new_session_id.clone(),
                };
                let focused_index = self
                    .visible()
                    .iter()
                    .position(|i| AgentKey::new(i) == new_key);
                if let Some(idx) = focused_index {
                    self.list_state.select(Some(idx));
                    self.preview_scroll = 0;
                }
                self.status = status;
                debug_log(
                    "clone_ok",
                    serde_json::json!({
                        "source_provider": info.provider.as_str(),
                        "source_session_id": &info.session_id,
                        "target_provider": rep.target_provider.as_str(),
                        "new_session_id": &rep.new_session_id,
                        "artifact": format!("{:?}", rep.artifact),
                        "clone_tree_error": clone_tree_error.map(|e| e.to_string()),
                        "focused_index": focused_index,
                    }),
                );
            }
            Err(e) => {
                self.status = format!(
                    "clone {} -> {} failed: {}",
                    info.provider.as_str(),
                    target.as_str(),
                    e
                );
                debug_log(
                    "clone_failed",
                    serde_json::json!({
                        "source_provider": info.provider.as_str(),
                        "source_session_id": &info.session_id,
                        "target_provider": target.as_str(),
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }
}

fn spawn_preview_worker() -> (Sender<PreviewRequest>, Receiver<PreviewResult>) {
    let (request_tx, request_rx) = mpsc::channel::<PreviewRequest>();
    let (result_tx, result_rx) = mpsc::channel::<PreviewResult>();
    match thread::Builder::new()
        .name("cokacmux-preview".to_string())
        .spawn(move || preview_worker(request_rx, result_tx))
    {
        Ok(_) => debug_log("preview_worker_spawned", serde_json::json!({})),
        Err(e) => debug_log(
            "preview_worker_spawn_failed",
            serde_json::json!({
                "error": e.to_string(),
            }),
        ),
    }
    (request_tx, result_rx)
}

fn preview_worker(request_rx: Receiver<PreviewRequest>, result_tx: Sender<PreviewResult>) {
    while let Ok(mut request) = request_rx.recv() {
        let first_seq = request.seq;
        let mut drained = 0usize;
        while let Ok(newer) = request_rx.try_recv() {
            request = newer;
            drained += 1;
        }
        debug_log(
            "preview_worker_request",
            serde_json::json!({
                "first_seq": first_seq,
                "seq": request.seq,
                "drained": drained,
                "provider": request.info.provider.as_str(),
                "session_id": &request.info.session_id,
                "mode": preview_mode_label(request.key.mode),
                "width": request.width,
            }),
        );

        let started = Instant::now();
        let text = match session::load(&request.info) {
            Ok(session) => {
                debug_log(
                    "preview_worker_load_ok",
                    serde_json::json!({
                        "seq": request.seq,
                        "messages": session.messages.len(),
                        "title": session.title.as_deref(),
                    }),
                );
                session::render::render(&session, request.key.mode)
            }
            Err(e) => {
                debug_log(
                    "preview_worker_load_failed",
                    serde_json::json!({
                        "seq": request.seq,
                        "error": e.to_string(),
                    }),
                );
                format!("error loading session: {}", e)
            }
        };
        let wrap_width = request.width.max(1);
        let lines = wrap_preview_lines(&text, wrap_width);
        debug_log(
            "preview_worker_rendered",
            serde_json::json!({
                "seq": request.seq,
                "text_len": text.len(),
                "lines": lines.len(),
                "wrap_width": wrap_width,
                "elapsed_ms": started.elapsed().as_millis(),
            }),
        );

        if result_tx
            .send(PreviewResult {
                seq: request.seq,
                key: request.key,
                text,
                wrap_width,
                lines,
            })
            .is_err()
        {
            debug_log(
                "preview_worker_result_send_failed",
                serde_json::json!({
                    "seq": request.seq,
                }),
            );
            break;
        }
    }
    debug_log("preview_worker_stop", serde_json::json!({}));
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let debug_enabled = args.iter().any(|a| a == "--debug");
    let trace_enabled = args.iter().any(|a| a == "--trace");
    let command_args: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--debug" && *arg != "--trace")
        .collect();
    init_debug_from_cli(debug_enabled, trace_enabled);
    install_vt100_panic_filter();
    // Headless smoke-test mode — doesn't enter raw mode / alternate screen.
    // Useful for CI-style sanity checks of the library + discovery layer.
    debug_log(
        "main_start",
        serde_json::json!({
            "args": &args,
            "pid": std::process::id(),
            "debug_log_file": DEBUG_LOG_FILE,
            "trace": TRACE_ENABLED.load(Ordering::Relaxed),
        }),
    );
    if args.first().map(String::as_str) == Some(AGENT_DAEMON_ARG) {
        debug_log("main_dispatch_agent_daemon", serde_json::json!({}));
        let result = run_agent_daemon_args(&args[1..]);
        debug_log(
            "main_agent_daemon_exit",
            serde_json::json!({
                "ok": result.is_ok(),
                "error": result.as_ref().err().map(|e| e.to_string()),
            }),
        );
        return result;
    }
    if matches!(command_args.as_slice(), ["killall"] | ["agents", "killall"]) {
        debug_log("main_dispatch_killall", serde_json::json!({}));
        let report = kill_all_agent_daemons()?;
        debug_log(
            "main_killall_done",
            serde_json::json!({
                "scanned": report.scanned,
                "killed": report.killed,
                "stale": report.stale,
                "skipped_self": report.skipped_self,
                "errors": report.errors,
                "pty_logs_deleted": report.pty_logs_deleted,
            }),
        );
        println!(
            "killed {} agent daemon(s); stale={} skipped_self={} errors={}{}",
            report.killed,
            report.stale,
            report.skipped_self,
            report.errors,
            if report.pty_logs_deleted > 0 {
                format!(" pty_logs_deleted={}", report.pty_logs_deleted)
            } else {
                String::new()
            }
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        debug_log("main_dispatch_version", serde_json::json!({}));
        println!("cokacmux {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        debug_log("main_dispatch_help", serde_json::json!({}));
        print_help();
        return Ok(());
    }
    if args.iter().any(|a| a == "--check") {
        debug_log("main_dispatch_check", serde_json::json!({}));
        let app = App::new();
        debug_log(
            "main_check_done",
            serde_json::json!({
                "sessions": app.sessions.len(),
                "status": &app.status,
            }),
        );
        println!(
            "cokacmux --check ok: {} sessions discovered (status: {})",
            app.sessions.len(),
            app.status
        );
        return Ok(());
    }

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal);
    restore_terminal(&mut terminal)?;
    debug_log(
        "main_tui_exit",
        serde_json::json!({
            "ok": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.to_string()),
        }),
    );
    result
}

fn print_help() {
    println!(
        "cokacmux — TUI session browser for Claude Code / Codex / OpenCode\n\n\
         USAGE:\n  cokacmux              launch interactive TUI\n  \
         cokacmux --debug      launch with debug logs enabled\n  \
         cokacmux --trace      launch with high-volume trace logs enabled\n  \
         cokacmux --check      headless sanity check (no TTY needed)\n  \
         cokacmux killall      terminate all cokacmux agent daemons\n  \
         cokacmux --version    print version\n\n\
         CONFIG:\n  ~/.cokacmux/settings.json\n  ~/.cokacmux/keybinding.json\n\n\
         INTERACTIVE KEYS:\n  \
         q / Ctrl+Q    quit\n  \
         ↑↓ / j k     navigate\n  \
         Alt+↑/↓ or Ctrl+Shift+↑/↓ select from sidebar/list\n  \
         Alt+←/→ or Ctrl+Shift+←/→ resize panes (saved)\n  \
         Agent: Shift+↑/↓ scroll PTY one line, Shift+Alt+↑/↓ page, Shift/Alt+Home/End top/bottom\n  \
         PgUp / PgDn   jump 10\n  \
         g/Home / G/End top / bottom\n  \
         Tab / Esc     switch focus between session list and preview\n  \
         /             open session-data search dialog\n  \
         v             toggle session list/tree view\n  \
         t             edit selected session title\n  \
         r             refresh from disk\n  \
         c             clone/convert selected session\n  \
         e             switch to live selected agent, or choose launch mode to start it\n  \
         Ctrl+] / Ctrl+[ switch between sessions and active agent\n  \
         Ctrl+K        kill selected/current agent\n  \
         Ctrl+PgUp/PgDn switch live agent from agent screen\n  \
         Delete / d    delete selected session (confirm)\n  \
         Enter         toggle preview summary/full"
    );
}

fn setup_terminal() -> Result<Tui> {
    debug_log("terminal_setup_start", serde_json::json!({}));
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    debug_log("terminal_setup_ok", serde_json::json!({}));
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    debug_log("terminal_restore_start", serde_json::json!({}));
    terminal.show_cursor()?;
    disable_raw_mode()?;
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::cursor::Show,
        crossterm::cursor::MoveToColumn(0),
        crossterm::style::Print("\r\n"),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine),
        crossterm::cursor::MoveToColumn(0)
    )?;
    debug_log("terminal_restore_ok", serde_json::json!({}));
    Ok(())
}

fn run(terminal: &mut Tui) -> Result<()> {
    let mut app = App::new();
    let (main_tx, main_rx) = mpsc::channel::<MainEvent>();
    app.main_tx = Some(main_tx.clone());

    // Input forwarder: blocks on crossterm event::poll/read in its own
    // thread and forwards events to the main loop. The long timeout is
    // only to allow the thread to wake periodically and notice that the
    // receiver is gone (so we can exit cleanly); it is not a latency
    // budget — the next read returns the instant stdin has data.
    let input_tx = main_tx.clone();
    let _input_thread = thread::Builder::new()
        .name("cokacmux-input".into())
        .spawn(move || loop {
            match event::poll(Duration::from_secs(60)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if input_tx.send(MainEvent::Input(ev)).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                },
                Ok(false) => {} // timer expired, just loop
                Err(_) => return,
            }
        })?;

    // Housekeeping ticker: fires at the existing agent-state poll
    // cadence. Not a latency knob — agent output/input are channel-driven.
    let tick_tx = main_tx.clone();
    let _tick_thread = thread::Builder::new()
        .name("cokacmux-tick".into())
        .spawn(move || loop {
            thread::sleep(Duration::from_millis(AGENT_STATE_POLL_INTERVAL_MS));
            if tick_tx.send(MainEvent::Tick).is_err() {
                return;
            }
        })?;

    debug_log("tui_start", serde_json::json!({}));

    // First render so the user sees something before any event arrives.
    app.poll_preview_results();
    app.poll_agent_sessions();
    app.poll_agent_runtime_states();
    terminal.draw(|f| {
        if app.is_agent_view() {
            ui_agent(f, &mut app);
        } else {
            ui(f, &mut app);
        }
    })?;
    let mut previous_is_agent_view = app.is_agent_view();

    while !app.should_quit {
        // Block until ANY producer fires. Output bytes, input keys, and
        // housekeeping ticks all wake us here — no fallback timer.
        let event = match main_rx.recv() {
            Ok(ev) => ev,
            Err(_) => break, // all senders dropped
        };
        match event {
            MainEvent::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if app.is_agent_view() {
                    let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
                    handle_agent_key(&mut app, key, cols);
                } else {
                    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                    let agent_cols = agent_terminal_width(cols, app.agent_sidebar_config_width());
                    handle_key(
                        &mut app,
                        key,
                        cols,
                        agent_cols,
                        rows.saturating_sub(AGENT_STATUS_HEIGHT),
                    );
                }
            }
            MainEvent::Input(_) => {}
            MainEvent::AgentEvent { reader_id, event } => {
                app.on_agent_event(reader_id, event);
            }
            MainEvent::AgentReaderEnded { reader_id, reason } => {
                app.on_agent_reader_ended(reader_id, reason);
            }
            MainEvent::SearchResult(result) => {
                app.on_search_result(result);
            }
            MainEvent::Tick => {
                app.poll_preview_results();
                app.poll_agent_sessions();
                app.poll_agent_runtime_states();
            }
        }
        app.prepare_sessions_view_after_transition(&mut previous_is_agent_view, "main_event");
        // Drain any further events that are already queued so we don't
        // render twice in a row for a tight burst of agent output.
        loop {
            match main_rx.try_recv() {
                Ok(MainEvent::Input(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    if app.is_agent_view() {
                        let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
                        handle_agent_key(&mut app, key, cols);
                    } else {
                        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                        let agent_cols =
                            agent_terminal_width(cols, app.agent_sidebar_config_width());
                        handle_key(
                            &mut app,
                            key,
                            cols,
                            agent_cols,
                            rows.saturating_sub(AGENT_STATUS_HEIGHT),
                        );
                    }
                }
                Ok(MainEvent::Input(_)) => {}
                Ok(MainEvent::AgentEvent { reader_id, event }) => {
                    app.on_agent_event(reader_id, event);
                }
                Ok(MainEvent::AgentReaderEnded { reader_id, reason }) => {
                    app.on_agent_reader_ended(reader_id, reason);
                }
                Ok(MainEvent::SearchResult(result)) => {
                    app.on_search_result(result);
                }
                Ok(MainEvent::Tick) => {
                    app.poll_preview_results();
                    app.poll_agent_sessions();
                    app.poll_agent_runtime_states();
                }
                Err(_) => break,
            }
            app.prepare_sessions_view_after_transition(&mut previous_is_agent_view, "queued_event");
        }
        if app.should_quit {
            break;
        }
        // Reap exited agent (channel-driven path no longer routes through
        // poll_agent_sessions automatically when no Tick is pending).
        app.poll_agent_sessions();
        app.prepare_sessions_view_after_transition(&mut previous_is_agent_view, "pre_draw");
        terminal.draw(|f| {
            if app.is_agent_view() {
                ui_agent(f, &mut app);
            } else {
                ui(f, &mut app);
            }
        })?;
    }
    debug_log("tui_stop", serde_json::json!({}));
    Ok(())
}

fn run_agent_daemon_args(args: &[String]) -> Result<()> {
    if !(args.len() == 4 || args.len() == 5) {
        anyhow::bail!(
            "{} requires <provider> <session-id> <cwd> <source> [launch-mode]",
            AGENT_DAEMON_ARG
        );
    }
    let provider = Provider::parse(&args[0])
        .ok_or_else(|| anyhow::anyhow!("unknown provider `{}`", args[0]))?;
    let info = SessionInfo {
        provider,
        session_id: args[1].clone(),
        cwd: args[2].clone(),
        source: PathBuf::from(&args[3]),
        updated_at_epoch_s: 0,
        title: None,
    };
    let launch_mode = match args.get(4) {
        Some(value) => AgentLaunchMode::parse(value)
            .ok_or_else(|| anyhow::anyhow!("unknown launch mode `{}`", value))?,
        None => AgentLaunchMode::Normal,
    };
    run_agent_daemon(info, launch_mode)
}

fn run_agent_daemon(info: SessionInfo, launch_mode: AgentLaunchMode) -> Result<()> {
    prepare_agent_daemon_process();
    let key = AgentKey::new(&info);
    let socket_path = agent_socket_path(&key)?;
    let meta_path = agent_meta_path(&key)?;
    let settings = Settings::load();
    let startup_spec = agent_launch_spec_with_settings(&info, launch_mode, &settings);
    validate_session_launch_cwd(&info)?;
    debug_log(
        "daemon_start",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &info.session_id,
            "launch_mode": launch_mode.as_str(),
            "command": startup_spec.command_line(),
        }),
    );
    let listener = bind_agent_listener(&key, &socket_path)?;
    listener.set_nonblocking(true)?;
    debug_log(
        "daemon_socket_bound",
        serde_json::json!({
            "provider": key.provider.as_str(),
            "session_id": &key.session_id,
            "socket": socket_path.display().to_string(),
        }),
    );
    let startup_epoch_ms = current_epoch_ms();
    write_agent_meta_parts(
        &meta_path,
        &info,
        &startup_spec,
        launch_mode,
        false,
        None,
        "starting",
        startup_epoch_ms,
        startup_epoch_ms,
        startup_epoch_ms,
        AgentActivity::Busy,
        None,
    )?;

    let mut agent =
        match AgentSession::spawn(info, 80, 24, launch_mode, &settings.cokacmux.agent_programs) {
            Ok(agent) => agent,
            Err(e) => {
                debug_log(
                    "daemon_agent_spawn_failed",
                    serde_json::json!({
                        "provider": key.provider.as_str(),
                        "session_id": &key.session_id,
                        "error": e.to_string(),
                    }),
                );
                let _ = fs::remove_file(&socket_path);
                let _ = fs::remove_file(&meta_path);
                let _ = remove_agent_pty_log(&key);
                return Err(e);
            }
        };
    if let Err(e) = write_agent_meta(&meta_path, &mut agent, false, None) {
        debug_log(
            "daemon_meta_write_failed",
            serde_json::json!({
                "provider": agent.info.provider.as_str(),
                "session_id": &agent.info.session_id,
                "phase": "spawned",
                "error": e.to_string(),
            }),
        );
    }
    debug_log(
        "daemon_agent_spawned",
        serde_json::json!({
            "provider": agent.info.provider.as_str(),
            "session_id": &agent.info.session_id,
            "launch_mode": launch_mode.as_str(),
            "command": agent.spec.command_line(),
            "child_pid": agent.child.process_id(),
        }),
    );
    let mut client: Option<DaemonConnection> = None;
    let mut attached_client_pid: Option<u32> = None;
    let daemon_started_at = Instant::now();
    let mut last_no_output_log_at = Instant::now();
    #[cfg(windows)]
    let mut last_windows_ctrl_event_count = windows_console_ctrl_event_snapshot().0;

    let exit_status = loop {
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let mut conn = match DaemonConnection::new(stream) {
                        Ok(conn) => conn,
                        Err(e) => {
                            debug_log(
                                "daemon_client_stream_setup_failed",
                                serde_json::json!({
                                    "provider": agent.info.provider.as_str(),
                                    "session_id": &agent.info.session_id,
                                    "error": e.to_string(),
                                }),
                            );
                            continue;
                        }
                    };
                    if send_daemon_attached(&mut conn, &mut agent, false).is_ok() {
                        client = Some(conn);
                        debug_log(
                            "daemon_client_connected",
                            serde_json::json!({
                                "provider": agent.info.provider.as_str(),
                                "session_id": &agent.info.session_id,
                            }),
                        );
                    } else {
                        debug_log(
                            "daemon_client_attach_snapshot_failed",
                            serde_json::json!({
                                "provider": agent.info.provider.as_str(),
                                "session_id": &agent.info.session_id,
                            }),
                        );
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    debug_log(
                        "daemon_accept_failed",
                        serde_json::json!({
                            "provider": agent.info.provider.as_str(),
                                "session_id": &agent.info.session_id,
                            "error": e.to_string(),
                        }),
                    );
                    break;
                }
            }
        }

        let (output_chunks, screen_changed) = agent.drain_output_chunks();
        // Track shell cwd changes on every tick (cheap /proc readlink) and
        // force an immediate meta write on change so the sidebar reflects
        // `cd` without waiting for the activity-meta rate limit.
        let cwd_changed = agent.refresh_shell_cwd_from_kernel();
        if cwd_changed || (screen_changed && agent.should_write_activity_meta()) {
            let _ = write_agent_meta(
                &meta_path,
                &mut agent,
                attached_client_pid.is_some(),
                attached_client_pid,
            );
        }
        for bytes in output_chunks {
            if let Some(conn) = client.as_mut() {
                if conn
                    .send_event(&AgentDaemonEvent::Output { data: bytes })
                    .is_err()
                {
                    client = None;
                    attached_client_pid = None;
                    let _ = write_agent_meta(&meta_path, &mut agent, false, None);
                    debug_log(
                        "daemon_client_output_failed",
                        serde_json::json!({
                            "provider": agent.info.provider.as_str(),
                            "session_id": &agent.info.session_id,
                        }),
                    );
                }
            }
        }

        if let Some(conn) = client.as_mut() {
            match conn.read_requests() {
                Ok(requests) => {
                    if !requests.is_empty() {
                        let details = serde_json::json!({
                            "provider": agent.info.provider.as_str(),
                            "session_id": &agent.info.session_id,
                            "count": requests.len(),
                            "attached_client_pid": attached_client_pid,
                            "child_pid": agent.child.process_id(),
                            "requests": requests.iter().map(agent_daemon_request_debug_value).collect::<Vec<_>>(),
                        });
                        if agent_daemon_requests_are_input_only(&requests) {
                            trace_log("daemon_requests_received", details);
                        } else {
                            debug_log("daemon_requests_received", details);
                        }
                    }
                    for request in requests {
                        match request {
                            AgentDaemonRequest::Attach {
                                cols,
                                rows,
                                client_pid,
                            } => {
                                agent.resize(cols, rows);
                                agent.rehydrate_parser_from_pty_log();
                                attached_client_pid = Some(client_pid);
                                let _ = write_agent_meta(
                                    &meta_path,
                                    &mut agent,
                                    true,
                                    Some(client_pid),
                                );
                                debug_log(
                                    "daemon_attach_request",
                                    serde_json::json!({
                                        "provider": agent.info.provider.as_str(),
                                        "session_id": &agent.info.session_id,
                                        "client_pid": client_pid,
                                        "cols": cols,
                                        "rows": rows,
                                    }),
                                );
                                let _ = send_daemon_attached(conn, &mut agent, true);
                            }
                            AgentDaemonRequest::Resize { cols, rows } => {
                                debug_log(
                                    "daemon_resize_request",
                                    serde_json::json!({
                                        "provider": agent.info.provider.as_str(),
                                        "session_id": &agent.info.session_id,
                                        "client_pid": attached_client_pid,
                                        "cols": cols,
                                        "rows": rows,
                                    }),
                                );
                                agent.resize(cols, rows);
                                let _ = send_daemon_attached(conn, &mut agent, false);
                            }
                            AgentDaemonRequest::Input { data } => {
                                trace_log(
                                    "daemon_input_request",
                                    serde_json::json!({
                                        "provider": agent.info.provider.as_str(),
                                        "session_id": &agent.info.session_id,
                                        "client_pid": attached_client_pid,
                                        "len": data.len(),
                                        "sample": debug_bytes_sample(&data, 128),
                                    }),
                                );
                                agent.send_bytes(&data);
                                let _ = write_agent_meta(
                                    &meta_path,
                                    &mut agent,
                                    true,
                                    attached_client_pid,
                                );
                            }
                            AgentDaemonRequest::Detach => {
                                let detached_client_pid = attached_client_pid;
                                attached_client_pid = None;
                                let meta_write =
                                    write_agent_meta(&meta_path, &mut agent, false, None);
                                debug_log(
                                    "daemon_detach_request",
                                    serde_json::json!({
                                        "provider": agent.info.provider.as_str(),
                                        "session_id": &agent.info.session_id,
                                        "detached_client_pid": detached_client_pid,
                                        "child_pid": agent.child.process_id(),
                                        "meta_write_ok": meta_write.is_ok(),
                                        "meta_write_error": meta_write.err().map(|e| e.to_string()),
                                    }),
                                );
                                client = None;
                                break;
                            }
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => {
                    let disconnected_client_pid = attached_client_pid;
                    attached_client_pid = None;
                    let _ = write_agent_meta(&meta_path, &mut agent, false, None);
                    debug_log(
                        "daemon_client_disconnected",
                        serde_json::json!({
                            "provider": agent.info.provider.as_str(),
                            "session_id": &agent.info.session_id,
                            "attached_client_pid": disconnected_client_pid,
                            "child_pid": agent.child.process_id(),
                            "error_kind": format!("{:?}", e.kind()),
                            "error": e.to_string(),
                        }),
                    );
                    client = None;
                }
            }
        }

        match agent.child.try_wait() {
            Ok(Some(status)) => {
                debug_log(
                    "daemon_child_try_wait_exit",
                    serde_json::json!({
                        "provider": agent.info.provider.as_str(),
                        "session_id": &agent.info.session_id,
                        "child_pid": agent.child.process_id(),
                        "status": status.to_string(),
                        "attached_client_pid": attached_client_pid,
                    }),
                );
                break status.to_string();
            }
            Ok(None) => {
                if agent.last_output_epoch_ms == 0
                    && last_no_output_log_at.elapsed() >= Duration::from_secs(5)
                {
                    last_no_output_log_at = Instant::now();
                    debug_log(
                        "daemon_agent_no_output_yet",
                        serde_json::json!({
                            "provider": agent.info.provider.as_str(),
                            "session_id": &agent.info.session_id,
                            "elapsed_ms": daemon_started_at.elapsed().as_millis(),
                            "attached": attached_client_pid.is_some(),
                            "pty_cols": agent.pty_size.cols,
                            "pty_rows": agent.pty_size.rows,
                        }),
                    );
                }
            }
            Err(e) => {
                debug_log(
                    "daemon_child_try_wait_failed",
                    serde_json::json!({
                        "provider": agent.info.provider.as_str(),
                        "session_id": &agent.info.session_id,
                        "child_pid": agent.child.process_id(),
                        "error": e.to_string(),
                        "attached_client_pid": attached_client_pid,
                    }),
                );
                break e.to_string();
            }
        }

        #[cfg(windows)]
        {
            let (ctrl_event_count, last_ctrl_event) = windows_console_ctrl_event_snapshot();
            if ctrl_event_count != last_windows_ctrl_event_count {
                last_windows_ctrl_event_count = ctrl_event_count;
                debug_log(
                    "daemon_windows_console_ctrl_event_seen",
                    serde_json::json!({
                        "provider": agent.info.provider.as_str(),
                        "session_id": &agent.info.session_id,
                        "event_count": ctrl_event_count,
                        "last_event": last_ctrl_event,
                        "attached_client_pid": attached_client_pid,
                        "child_pid": agent.child.process_id(),
                    }),
                );
            }
        }

        thread::sleep(Duration::from_millis(30));
    };
    debug_log(
        "daemon_child_exit",
        serde_json::json!({
            "provider": agent.info.provider.as_str(),
            "session_id": &agent.info.session_id,
            "child_pid": agent.child.process_id(),
            "status": &exit_status,
        }),
    );

    if let Some(conn) = client.as_mut() {
        let _ = conn.send_event(&AgentDaemonEvent::Exited {
            status: exit_status.clone(),
        });
    }
    drop(agent.pty_log.take());
    let pty_log_deleted = remove_agent_pty_log(&key);
    debug_log(
        "daemon_pty_log_cleanup",
        serde_json::json!({
            "provider": key.provider.as_str(),
            "session_id": &key.session_id,
            "deleted": pty_log_deleted,
        }),
    );
    let _ = fs::remove_file(socket_path);
    let _ = fs::remove_file(meta_path);
    Ok(())
}

fn send_daemon_attached(
    conn: &mut DaemonConnection,
    agent: &mut AgentSession,
    include_scrollback: bool,
) -> io::Result<()> {
    let snapshot = agent.screen_snapshot_bytes(include_scrollback);
    debug_log(
        "daemon_send_attached_snapshot",
        serde_json::json!({
            "provider": agent.info.provider.as_str(),
            "session_id": &agent.info.session_id,
            "snapshot_len": snapshot.len(),
            "include_scrollback": include_scrollback,
            "scrollback": agent.parser.screen().scrollback(),
            "screen_history_lines": agent.screen_history.len(),
            "visible": screen_has_visible_content(agent.parser.screen()),
            "last_screen_change_epoch_ms": agent.last_screen_change_epoch_ms,
            "last_output_epoch_ms": agent.last_output_epoch_ms,
            "preview": debug_screen_preview(agent.parser.screen(), 5),
        }),
    );
    conn.send_event(&AgentDaemonEvent::Attached {
        provider: agent.info.provider,
        session_id: agent.info.session_id.clone(),
        command: agent.spec.command_line(),
        daemon_pid: std::process::id(),
        snapshot_event: true,
        last_screen_change_epoch_ms: agent.last_screen_change_epoch_ms,
        last_output_epoch_ms: agent.last_output_epoch_ms,
        last_input_epoch_ms: agent.last_input_epoch_ms,
    })?;
    conn.send_event(&AgentDaemonEvent::Snapshot { data: snapshot })
}

fn write_agent_meta(
    meta_path: &Path,
    agent: &mut AgentSession,
    attached: bool,
    attached_client_pid: Option<u32>,
) -> io::Result<()> {
    let now_ms = current_epoch_ms();
    write_agent_meta_parts(
        meta_path,
        &agent.info,
        &agent.spec,
        agent.launch_mode,
        attached,
        attached_client_pid,
        if attached { "attached" } else { "live" },
        agent.last_screen_change_epoch_ms,
        agent.last_output_epoch_ms,
        agent.last_input_epoch_ms,
        agent_activity_from_timestamps(
            now_ms,
            agent.last_screen_change_epoch_ms,
            agent.last_output_epoch_ms,
            agent.last_input_epoch_ms,
        ),
        agent.child.process_id(),
    )
}

fn write_agent_meta_parts(
    meta_path: &Path,
    info: &SessionInfo,
    spec: &AgentLaunchSpec,
    launch_mode: AgentLaunchMode,
    attached: bool,
    attached_client_pid: Option<u32>,
    phase: &str,
    last_screen_change_epoch_ms: u64,
    last_output_epoch_ms: u64,
    last_input_epoch_ms: u64,
    activity: AgentActivity,
    child_pid: Option<u32>,
) -> io::Result<()> {
    if let Some(parent) = meta_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            debug_log(
                "agent_meta_parent_create_failed",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "meta_path": meta_path.display().to_string(),
                    "parent": parent.display().to_string(),
                    "error": e.to_string(),
                }),
            );
            return Err(e);
        }
    }
    let value = serde_json::json!({
        "pid": std::process::id(),
        "child_pid": child_pid,
        "provider": info.provider.as_str(),
        "session_id": &info.session_id,
        "cwd": &info.cwd,
        "source": info.source.display().to_string(),
        "command": spec.command_line(),
        "launch_mode": launch_mode.as_str(),
        "phase": phase,
        "activity": activity.label(),
        "attached": attached,
        "attached_client_pid": attached_client_pid,
        "last_screen_change_epoch_ms": last_screen_change_epoch_ms,
        "last_output_epoch_ms": last_output_epoch_ms,
        "last_input_epoch_ms": last_input_epoch_ms,
        "updated_at_epoch_s": current_epoch_s(),
    });
    let bytes = format!(
        "{}\n",
        serde_json::to_string(&value).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?
    );
    let tmp_path = meta_path.with_extension("json.tmp");
    if let Err(e) = fs::write(&tmp_path, bytes) {
        debug_log(
            "agent_meta_tmp_write_failed",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "meta_path": meta_path.display().to_string(),
                "tmp_path": tmp_path.display().to_string(),
                "error": e.to_string(),
            }),
        );
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp_path, meta_path) {
        debug_log(
            "agent_meta_rename_failed",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "meta_path": meta_path.display().to_string(),
                "tmp_path": tmp_path.display().to_string(),
                "error": e.to_string(),
            }),
        );
        return Err(e);
    }
    trace_log(
        "agent_meta_write",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &info.session_id,
            "meta_path": meta_path.display().to_string(),
            "daemon_pid": std::process::id(),
            "child_pid": child_pid,
            "cwd": &info.cwd,
            "source": info.source.display().to_string(),
            "launch_mode": launch_mode.as_str(),
            "phase": phase,
            "activity": activity.label(),
            "attached": attached,
            "attached_client_pid": attached_client_pid,
            "last_screen_change_epoch_ms": last_screen_change_epoch_ms,
            "last_output_epoch_ms": last_output_epoch_ms,
            "last_input_epoch_ms": last_input_epoch_ms,
        }),
    );
    Ok(())
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AgentMetaSnapshot {
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    child_pid: Option<u32>,
    provider: Option<String>,
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    attached: bool,
    attached_client_pid: Option<u32>,
    #[serde(default)]
    last_screen_change_epoch_ms: u64,
    #[serde(default)]
    last_output_epoch_ms: u64,
    #[serde(default)]
    last_input_epoch_ms: u64,
}

fn read_agent_meta_snapshot_at(meta_path: &Path) -> Option<AgentMetaSnapshot> {
    fs::read_to_string(meta_path)
        .ok()
        .and_then(|content| serde_json::from_str::<AgentMetaSnapshot>(&content).ok())
}

fn read_agent_meta_snapshot(key: &AgentKey) -> Option<AgentMetaSnapshot> {
    let meta_path = agent_meta_path(key).ok()?;
    read_agent_meta_snapshot_at(&meta_path)
}

fn live_agent_meta_snapshot(key: &AgentKey) -> Option<AgentMetaSnapshot> {
    read_agent_meta_snapshot(key).filter(|meta| meta.pid > 0 && process_is_alive(meta.pid))
}

fn new_agent_backing_session_for_key(key: &AgentKey) -> Option<NewAgentBackingSession> {
    let meta = live_agent_meta_snapshot(key)?;
    new_agent_backing_session_from_meta(&meta)
}

fn new_agent_backing_session_from_meta(meta: &AgentMetaSnapshot) -> Option<NewAgentBackingSession> {
    if meta.source.as_deref() != Some(NEW_AGENT_SESSION_SOURCE_MARKER) {
        return None;
    }
    if meta.provider.as_deref().and_then(Provider::parse) != Some(Provider::Codex) {
        return None;
    }
    let child_pid = meta.child_pid?;
    let expected_cwd = meta.cwd.as_deref().filter(|cwd| !cwd.is_empty());
    let rollout_path = codex_rollout_path_for_process_tree(child_pid, expected_cwd)?;
    let session_id = codex_session_id_from_rollout_path(&rollout_path)?;
    Some(NewAgentBackingSession {
        key: AgentKey {
            provider: Provider::Codex,
            session_id,
        },
        rollout_path,
    })
}

impl AgentMetaSnapshot {
    fn list_state(&self, current_pid: u32, now_epoch_ms: u64) -> AgentListState {
        let activity = agent_activity_from_timestamps(
            now_epoch_ms,
            self.last_screen_change_epoch_ms,
            self.last_output_epoch_ms,
            self.last_input_epoch_ms,
        );
        if self.attached {
            let Some(attached_client_pid) = self.attached_client_pid else {
                return AgentListState::Live { activity };
            };
            if attached_client_pid != current_pid && !process_is_alive(attached_client_pid) {
                return AgentListState::Live { activity };
            }
            AgentListState::Attached {
                mine: attached_client_pid == current_pid,
                activity,
            }
        } else {
            AgentListState::Live { activity }
        }
    }
}

fn agent_key_from_meta(meta: &AgentMetaSnapshot) -> Option<AgentKey> {
    let provider = Provider::parse(meta.provider.as_deref()?)?;
    let session_id = meta.session_id.as_deref()?.to_string();
    Some(AgentKey {
        provider,
        session_id,
    })
}

fn read_agent_runtime_state(key: &AgentKey, current_pid: u32) -> AgentListState {
    let meta_path = match agent_meta_path(key) {
        Ok(path) => path,
        Err(e) => {
            trace_log(
                "agent_runtime_state_path_failed",
                serde_json::json!({
                    "key": agent_key_debug_value(key),
                    "path_kind": "meta",
                    "current_pid": current_pid,
                    "error": e.to_string(),
                }),
            );
            return AgentListState::Idle;
        }
    };
    let socket_path = match agent_socket_path(key) {
        Ok(path) => path,
        Err(e) => {
            trace_log(
                "agent_runtime_state_path_failed",
                serde_json::json!({
                    "key": agent_key_debug_value(key),
                    "path_kind": "socket",
                    "current_pid": current_pid,
                    "error": e.to_string(),
                }),
            );
            return AgentListState::Idle;
        }
    };
    let state = read_agent_runtime_state_at(&meta_path, &socket_path, current_pid);
    trace_log(
        "agent_runtime_state_read",
        serde_json::json!({
            "key": agent_key_debug_value(key),
            "current_pid": current_pid,
            "meta_path": meta_path.display().to_string(),
            "socket_path": socket_path.display().to_string(),
            "meta_exists": meta_path.exists(),
            "socket_exists": socket_path.exists(),
            "result": agent_list_state_debug_value(state),
        }),
    );
    state
}

fn read_agent_runtime_state_at(
    meta_path: &Path,
    socket_path: &Path,
    current_pid: u32,
) -> AgentListState {
    let debug_enabled = DEBUG_ENABLED.load(Ordering::Relaxed);
    let content = match fs::read_to_string(meta_path) {
        Ok(content) => content,
        Err(e) => {
            trace_log(
                "agent_runtime_state_idle",
                serde_json::json!({
                    "reason": "meta_read_failed",
                    "current_pid": current_pid,
                    "meta_path": meta_path.display().to_string(),
                    "socket_path": socket_path.display().to_string(),
                    "meta_exists": meta_path.exists(),
                    "socket_exists": socket_path.exists(),
                    "error_kind": format!("{:?}", e.kind()),
                    "error": e.to_string(),
                }),
            );
            return AgentListState::Idle;
        }
    };
    let meta = match serde_json::from_str::<AgentMetaSnapshot>(&content) {
        Ok(meta) => meta,
        Err(e) => {
            trace_log(
                "agent_runtime_state_idle",
                serde_json::json!({
                    "reason": "meta_parse_failed",
                    "current_pid": current_pid,
                    "meta_path": meta_path.display().to_string(),
                    "socket_path": socket_path.display().to_string(),
                    "meta_exists": meta_path.exists(),
                    "socket_exists": socket_path.exists(),
                    "error": e.to_string(),
                    "content_sample": truncate_width(&content, 512),
                }),
            );
            return AgentListState::Idle;
        }
    };
    let daemon_alive = process_is_alive(meta.pid);
    if !daemon_alive {
        let meta_json = serde_json::from_str::<serde_json::Value>(&content).ok();
        let meta_updated_at_epoch_s = meta_json
            .as_ref()
            .and_then(|value| value.get("updated_at_epoch_s"))
            .and_then(serde_json::Value::as_u64);
        let stale_age_s =
            meta_updated_at_epoch_s.map(|updated_at| current_epoch_s().saturating_sub(updated_at));
        let meta_removed = fs::remove_file(meta_path).is_ok();
        let socket_removed = fs::remove_file(socket_path).is_ok();
        if debug_enabled {
            debug_log(
                "agent_meta_stale_removed",
                serde_json::json!({
                    "provider": meta.provider.as_deref(),
                    "session_id": meta.session_id.as_deref(),
                    "daemon_pid": meta.pid,
                    "current_pid": current_pid,
                    "meta_path": meta_path.display().to_string(),
                    "socket_path": socket_path.display().to_string(),
                    "meta_removed": meta_removed,
                    "socket_removed": socket_removed,
                    "meta_child_pid": meta.child_pid,
                    "meta_child_alive": meta.child_pid.map(process_is_alive),
                    "meta_phase": meta_json.as_ref().and_then(|value| value.get("phase")).and_then(serde_json::Value::as_str),
                    "meta_activity": meta_json.as_ref().and_then(|value| value.get("activity")).and_then(serde_json::Value::as_str),
                    "meta_attached": meta.attached,
                    "meta_attached_client_pid": meta.attached_client_pid,
                    "meta_attached_client_alive": meta.attached_client_pid.map(process_is_alive),
                    "meta_updated_at_epoch_s": meta_updated_at_epoch_s,
                    "meta_stale_age_s": stale_age_s,
                    "meta_last_screen_change_epoch_ms": meta.last_screen_change_epoch_ms,
                    "meta_last_output_epoch_ms": meta.last_output_epoch_ms,
                    "meta_last_input_epoch_ms": meta.last_input_epoch_ms,
                    "meta_command": meta_json.as_ref().and_then(|value| value.get("command")).and_then(serde_json::Value::as_str),
                    "meta_launch_mode": meta_json.as_ref().and_then(|value| value.get("launch_mode")).and_then(serde_json::Value::as_str),
                }),
            );
        }
        return AgentListState::Idle;
    }
    let state = meta.list_state(current_pid, current_epoch_ms());
    trace_log(
        "agent_runtime_state_meta_ok",
        serde_json::json!({
            "current_pid": current_pid,
            "meta_path": meta_path.display().to_string(),
            "socket_path": socket_path.display().to_string(),
            "socket_exists": socket_path.exists(),
            "daemon_pid": meta.pid,
            "daemon_alive": daemon_alive,
            "child_pid": meta.child_pid,
            "child_alive": meta.child_pid.map(process_is_alive),
            "provider": meta.provider.as_deref(),
            "session_id": meta.session_id.as_deref(),
            "cwd": meta.cwd.as_deref(),
            "source": meta.source.as_deref(),
            "attached": meta.attached,
            "attached_client_pid": meta.attached_client_pid,
            "attached_client_alive": meta.attached_client_pid.map(process_is_alive),
            "last_screen_change_epoch_ms": meta.last_screen_change_epoch_ms,
            "last_output_epoch_ms": meta.last_output_epoch_ms,
            "last_input_epoch_ms": meta.last_input_epoch_ms,
            "result": agent_list_state_debug_value(state),
        }),
    );
    state
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> *mut c_void;
        fn GetExitCodeProcess(hProcess: *mut c_void, lpExitCode: *mut u32) -> i32;
        fn CloseHandle(hObject: *mut c_void) -> i32;
    }

    if pid == 0 {
        return false;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            let error = io::Error::last_os_error();
            debug_log(
                "process_is_alive_windows_open_failed",
                serde_json::json!({
                    "pid": pid,
                    "current_pid": std::process::id(),
                    "raw_os_error": error.raw_os_error(),
                    "error": error.to_string(),
                }),
            );
            return false;
        }
        let mut exit_code = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        if ok == 0 {
            let error = io::Error::last_os_error();
            debug_log(
                "process_is_alive_windows_exit_code_failed",
                serde_json::json!({
                    "pid": pid,
                    "current_pid": std::process::id(),
                    "raw_os_error": error.raw_os_error(),
                    "error": error.to_string(),
                }),
            );
        } else if exit_code != STILL_ACTIVE {
            debug_log(
                "process_is_alive_windows_not_active",
                serde_json::json!({
                    "pid": pid,
                    "current_pid": std::process::id(),
                    "exit_code": exit_code,
                }),
            );
        }
        let _ = CloseHandle(handle);
        ok != 0 && exit_code == STILL_ACTIVE
    }
}

#[cfg(target_os = "linux")]
fn codex_rollout_path_for_process_tree(
    root_pid: u32,
    expected_cwd: Option<&str>,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for pid in linux_process_tree_pids(root_pid) {
        let fd_dir = PathBuf::from(format!("/proc/{pid}/fd"));
        let Ok(entries) = fs::read_dir(&fd_dir) else {
            trace_log(
                "new_agent_backing_fd_scan_skip",
                serde_json::json!({
                    "pid": pid,
                    "fd_dir": fd_dir.display().to_string(),
                    "reason": "fd_dir_unreadable",
                }),
            );
            continue;
        };
        for entry in entries.flatten() {
            let Ok(target) = fs::read_link(entry.path()) else {
                continue;
            };
            candidates.push((pid, target));
        }
    }
    select_unique_codex_rollout_path(root_pid, expected_cwd, candidates)
}

fn select_unique_codex_rollout_path(
    root_pid: u32,
    expected_cwd: Option<&str>,
    candidates: Vec<(u32, PathBuf)>,
) -> Option<PathBuf> {
    let candidate_count = candidates.len();
    let mut rollout_name_count = 0usize;
    let mut meta_mismatch_count = 0usize;
    let mut matches: Vec<(u32, PathBuf, String)> = Vec::new();
    for (pid, path) in candidates {
        let Some(session_id) = codex_session_id_from_rollout_path(&path) else {
            continue;
        };
        rollout_name_count += 1;
        if !codex_rollout_session_meta_matches(&path, &session_id, expected_cwd) {
            meta_mismatch_count += 1;
            continue;
        }
        matches.push((pid, path, session_id));
    }
    matches.sort_by(|a, b| a.1.cmp(&b.1));
    matches.dedup_by(|a, b| a.1 == b.1);
    match matches.as_slice() {
        [(_, path, _)] => Some(path.clone()),
        [] => {
            trace_log(
                "new_agent_backing_rollout_not_found",
                serde_json::json!({
                    "root_pid": root_pid,
                    "expected_cwd": expected_cwd,
                    "candidate_count": candidate_count,
                    "rollout_name_count": rollout_name_count,
                    "meta_mismatch_count": meta_mismatch_count,
                }),
            );
            None
        }
        many => {
            debug_log(
                "new_agent_backing_rollout_ambiguous",
                serde_json::json!({
                    "root_pid": root_pid,
                    "expected_cwd": expected_cwd,
                    "candidate_count": candidate_count,
                    "rollout_name_count": rollout_name_count,
                    "meta_mismatch_count": meta_mismatch_count,
                    "matches": many.iter().map(|(pid, path, session_id)| serde_json::json!({
                        "pid": pid,
                        "path": path.display().to_string(),
                        "session_id": session_id,
                    })).collect::<Vec<_>>(),
                }),
            );
            None
        }
    }
}

#[cfg(windows)]
fn codex_rollout_path_for_process_tree(
    root_pid: u32,
    expected_cwd: Option<&str>,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for pid in windows_process_tree_pids(root_pid) {
        for path in windows_open_rollout_paths_for_pid(pid) {
            candidates.push((pid, path));
        }
    }
    select_unique_codex_rollout_path(root_pid, expected_cwd, candidates)
}

#[cfg(target_os = "macos")]
fn codex_rollout_path_for_process_tree(
    root_pid: u32,
    expected_cwd: Option<&str>,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for pid in macos_process_tree_pids(root_pid) {
        for path in macos_open_rollout_paths_for_pid(pid) {
            candidates.push((pid, path));
        }
    }
    select_unique_codex_rollout_path(root_pid, expected_cwd, candidates)
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn codex_rollout_path_for_process_tree(
    root_pid: u32,
    expected_cwd: Option<&str>,
) -> Option<PathBuf> {
    trace_log(
        "new_agent_backing_rollout_scan_unsupported_os",
        serde_json::json!({
            "root_pid": root_pid,
            "expected_cwd": expected_cwd,
        }),
    );
    None
}

#[cfg(target_os = "linux")]
fn linux_process_tree_pids(root_pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([root_pid]);
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        let children_path = format!("/proc/{pid}/task/{pid}/children");
        let Ok(children) = fs::read_to_string(&children_path) else {
            trace_log(
                "new_agent_backing_children_scan_skip",
                serde_json::json!({
                    "pid": pid,
                    "children_path": children_path,
                }),
            );
            continue;
        };
        for child in children.split_whitespace() {
            if let Ok(child_pid) = child.parse::<u32>() {
                queue.push_back(child_pid);
            }
        }
    }
    out
}

#[cfg(windows)]
type WinHandle = *mut std::ffi::c_void;

#[cfg(windows)]
fn windows_process_tree_pids(root_pid: u32) -> Vec<u32> {
    #[repr(C)]
    struct ProcessEntry32W {
        size: u32,
        usage: u32,
        process_id: u32,
        default_heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_process_id: u32,
        pri_class_base: i32,
        flags: u32,
        exe_file: [u16; 260],
    }

    const TH32CS_SNAPPROCESS: u32 = 0x00000002;
    const INVALID_HANDLE_VALUE: WinHandle = (-1isize) as WinHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(dwFlags: u32, th32ProcessID: u32) -> WinHandle;
        fn Process32FirstW(hSnapshot: WinHandle, lppe: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(hSnapshot: WinHandle, lppe: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(hObject: WinHandle) -> i32;
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        let error = io::Error::last_os_error();
        trace_log(
            "new_agent_backing_windows_process_snapshot_failed",
            serde_json::json!({
                "root_pid": root_pid,
                "raw_os_error": error.raw_os_error(),
                "error": error.to_string(),
            }),
        );
        return vec![root_pid];
    }

    let mut parents: HashMap<u32, u32> = HashMap::new();
    let mut entry = ProcessEntry32W {
        size: std::mem::size_of::<ProcessEntry32W>() as u32,
        usage: 0,
        process_id: 0,
        default_heap_id: 0,
        module_id: 0,
        threads: 0,
        parent_process_id: 0,
        pri_class_base: 0,
        flags: 0,
        exe_file: [0; 260],
    };

    unsafe {
        let mut ok = Process32FirstW(snapshot, &mut entry);
        while ok != 0 {
            parents.insert(entry.process_id, entry.parent_process_id);
            entry.size = std::mem::size_of::<ProcessEntry32W>() as u32;
            ok = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
    }

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, parent_pid) in parents {
        children.entry(parent_pid).or_default().push(pid);
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([root_pid]);
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        if let Some(child_pids) = children.get(&pid) {
            queue.extend(child_pids.iter().copied());
        }
    }
    out
}

#[cfg(windows)]
fn windows_open_rollout_paths_for_pid(pid: u32) -> Vec<PathBuf> {
    let Some(entries) = windows_system_handle_entries_for_pid(pid) else {
        return Vec::new();
    };

    const PROCESS_DUP_HANDLE: u32 = 0x0040;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const DUPLICATE_SAME_ACCESS: u32 = 0x00000002;
    const FILE_TYPE_DISK: u32 = 0x0001;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> WinHandle;
        fn GetCurrentProcess() -> WinHandle;
        fn DuplicateHandle(
            hSourceProcessHandle: WinHandle,
            hSourceHandle: WinHandle,
            hTargetProcessHandle: WinHandle,
            lpTargetHandle: *mut WinHandle,
            dwDesiredAccess: u32,
            bInheritHandle: i32,
            dwOptions: u32,
        ) -> i32;
        fn GetFileType(hFile: WinHandle) -> u32;
        fn CloseHandle(hObject: WinHandle) -> i32;
    }

    let process_handle = unsafe {
        OpenProcess(
            PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if process_handle.is_null() {
        let error = io::Error::last_os_error();
        trace_log(
            "new_agent_backing_windows_process_open_failed",
            serde_json::json!({
                "pid": pid,
                "raw_os_error": error.raw_os_error(),
                "error": error.to_string(),
            }),
        );
        return Vec::new();
    }

    let current_process = unsafe { GetCurrentProcess() };
    let mut paths = Vec::new();
    for entry in entries {
        let mut duplicated: WinHandle = std::ptr::null_mut();
        let duplicated_ok = unsafe {
            DuplicateHandle(
                process_handle,
                entry.handle_value as WinHandle,
                current_process,
                &mut duplicated,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } != 0;
        if !duplicated_ok || duplicated.is_null() {
            continue;
        }

        let file_type = unsafe { GetFileType(duplicated) };
        if file_type == FILE_TYPE_DISK {
            if let Some(path) = windows_path_for_file_handle(duplicated) {
                if codex_session_id_from_rollout_path(&path).is_some() {
                    paths.push(path);
                }
            }
        }
        unsafe {
            let _ = CloseHandle(duplicated);
        }
    }
    unsafe {
        let _ = CloseHandle(process_handle);
    }
    paths
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct WindowsSystemHandleEntry {
    object: WinHandle,
    unique_process_id: usize,
    handle_value: usize,
    granted_access: u32,
    creator_back_trace_index: u16,
    object_type_index: u16,
    handle_attributes: u32,
    reserved: u32,
}

#[cfg(windows)]
fn windows_system_handle_entries_for_pid(pid: u32) -> Option<Vec<WindowsSystemHandleEntry>> {
    const SYSTEM_EXTENDED_HANDLE_INFORMATION: u32 = 64;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC0000004u32 as i32;
    const STATUS_BUFFER_TOO_SMALL: i32 = 0xC0000023u32 as i32;
    const MAX_HANDLE_INFO_BYTES: usize = 64 * 1024 * 1024;

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQuerySystemInformation(
            SystemInformationClass: u32,
            SystemInformation: *mut std::ffi::c_void,
            SystemInformationLength: u32,
            ReturnLength: *mut u32,
        ) -> i32;
    }

    let mut buffer_len = 1024 * 1024usize;
    let buffer = loop {
        let mut buffer = vec![0u8; buffer_len];
        let mut return_len = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut return_len,
            )
        };
        if status >= 0 {
            break buffer;
        }
        if status != STATUS_INFO_LENGTH_MISMATCH && status != STATUS_BUFFER_TOO_SMALL {
            trace_log(
                "new_agent_backing_windows_handle_query_failed",
                serde_json::json!({
                    "pid": pid,
                    "status": status,
                    "buffer_len": buffer_len,
                    "return_len": return_len,
                }),
            );
            return None;
        }
        let requested = usize::try_from(return_len).unwrap_or(0);
        buffer_len = buffer_len
            .saturating_mul(2)
            .max(requested.saturating_add(4096));
        if buffer_len > MAX_HANDLE_INFO_BYTES {
            trace_log(
                "new_agent_backing_windows_handle_query_too_large",
                serde_json::json!({
                    "pid": pid,
                    "buffer_len": buffer_len,
                    "return_len": return_len,
                }),
            );
            return None;
        }
    };

    let count_size = std::mem::size_of::<usize>();
    let header_size = count_size * 2;
    if buffer.len() < header_size {
        return None;
    }
    let handle_count = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<usize>()) };
    let entry_size = std::mem::size_of::<WindowsSystemHandleEntry>();
    if handle_count > (buffer.len().saturating_sub(header_size) / entry_size) {
        trace_log(
            "new_agent_backing_windows_handle_query_truncated",
            serde_json::json!({
                "pid": pid,
                "handle_count": handle_count,
                "buffer_len": buffer.len(),
                "entry_size": entry_size,
            }),
        );
        return None;
    }

    let mut entries = Vec::new();
    for index in 0..handle_count {
        let offset = header_size + (index * entry_size);
        let entry = unsafe {
            std::ptr::read_unaligned(
                buffer
                    .as_ptr()
                    .add(offset)
                    .cast::<WindowsSystemHandleEntry>(),
            )
        };
        if entry.unique_process_id == pid as usize {
            entries.push(entry);
        }
    }
    Some(entries)
}

#[cfg(windows)]
fn windows_path_for_file_handle(handle: WinHandle) -> Option<PathBuf> {
    const FILE_NAME_NORMALIZED: u32 = 0x0;
    const MAX_PATH_CHARS: u32 = 32_768;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(
            hFile: WinHandle,
            lpszFilePath: *mut u16,
            cchFilePath: u32,
            dwFlags: u32,
        ) -> u32;
    }

    let mut len = 1024u32;
    loop {
        let mut buffer = vec![0u16; len as usize];
        let written = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                FILE_NAME_NORMALIZED,
            )
        };
        if written == 0 {
            return None;
        }
        if written < buffer.len() as u32 {
            buffer.truncate(written as usize);
            let raw = String::from_utf16_lossy(&buffer);
            return Some(PathBuf::from(raw));
        }
        len = written.saturating_add(1);
        if len > MAX_PATH_CHARS {
            return None;
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_process_tree_pids(root_pid: u32) -> Vec<u32> {
    let output = match Command::new("ps").args(["-axo", "pid=,ppid="]).output() {
        Ok(output) => output,
        Err(e) => {
            trace_log(
                "new_agent_backing_macos_process_snapshot_failed",
                serde_json::json!({
                    "root_pid": root_pid,
                    "error": e.to_string(),
                }),
            );
            return vec![root_pid];
        }
    };
    if !output.status.success() {
        trace_log(
            "new_agent_backing_macos_process_snapshot_failed",
            serde_json::json!({
                "root_pid": root_pid,
                "status": output.status.code(),
                "stderr": String::from_utf8_lossy(&output.stderr).trim(),
            }),
        );
        return vec![root_pid];
    }

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(parent_pid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        children.entry(parent_pid).or_default().push(pid);
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([root_pid]);
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        if let Some(child_pids) = children.get(&pid) {
            queue.extend(child_pids.iter().copied());
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn macos_open_rollout_paths_for_pid(pid: u32) -> Vec<PathBuf> {
    let output = match Command::new("lsof")
        .args(["-nP", "-F", "n", "-p", &pid.to_string()])
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            trace_log(
                "new_agent_backing_macos_lsof_failed",
                serde_json::json!({
                    "pid": pid,
                    "error": e.to_string(),
                }),
            );
            return Vec::new();
        }
    };
    if !output.status.success() {
        trace_log(
            "new_agent_backing_macos_lsof_failed",
            serde_json::json!({
                "pid": pid,
                "status": output.status.code(),
                "stderr": String::from_utf8_lossy(&output.stderr).trim(),
            }),
        );
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
        .filter(|path| codex_session_id_from_rollout_path(path).is_some())
        .collect()
}

fn codex_session_id_from_rollout_path(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    if !file_name.starts_with("rollout-") || !file_name.ends_with(".jsonl") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if stem.len() < 36 {
        return None;
    }
    let session_id = &stem[stem.len() - 36..];
    uuid::Uuid::parse_str(session_id).ok()?;
    Some(session_id.to_string())
}

fn codex_rollout_session_meta_matches(
    path: &Path,
    session_id: &str,
    expected_cwd: Option<&str>,
) -> bool {
    use std::io::{BufRead, BufReader};

    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    for line in BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
        .take(16)
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            return false;
        };
        if payload.get("id").and_then(serde_json::Value::as_str) != Some(session_id) {
            return false;
        }
        if let Some(expected_cwd) = expected_cwd {
            let Some(actual_cwd) = payload.get("cwd").and_then(serde_json::Value::as_str) else {
                return false;
            };
            if !session_cwd_eq(actual_cwd, expected_cwd) {
                return false;
            }
        }
        return true;
    }
    false
}

fn current_epoch_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn agent_activity_from_timestamps(
    now_epoch_ms: u64,
    last_screen_change_epoch_ms: u64,
    last_output_epoch_ms: u64,
    last_input_epoch_ms: u64,
) -> AgentActivity {
    let last_activity_epoch_ms = last_screen_change_epoch_ms
        .max(last_output_epoch_ms)
        .max(last_input_epoch_ms);
    if last_activity_epoch_ms > 0
        && now_epoch_ms.saturating_sub(last_activity_epoch_ms) <= AGENT_BUSY_GRACE_MS
    {
        AgentActivity::Busy
    } else {
        AgentActivity::Quiet
    }
}

fn debug_command_argv(command: &CommandBuilder) -> Vec<String> {
    command
        .get_argv()
        .iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect()
}

fn debug_bytes_sample(bytes: &[u8], max_bytes: usize) -> String {
    let end = bytes.len().min(max_bytes);
    let mut sample = String::new();
    for ch in String::from_utf8_lossy(&bytes[..end]).chars() {
        match ch {
            '\r' => sample.push_str("\\r"),
            '\n' => sample.push_str("\\n"),
            '\t' => sample.push_str("\\t"),
            ch if ch.is_control() => {
                sample.push_str(&format!("\\u{{{:04x}}}", ch as u32));
            }
            ch => sample.push(ch),
        }
    }
    if bytes.len() > max_bytes {
        sample.push_str("...");
    }
    sample
}

fn debug_screen_preview(screen: &vt100::Screen, max_rows: usize) -> Vec<String> {
    let (rows, cols) = screen.size();
    let mut lines = Vec::new();
    for row in 0..rows {
        let mut line = String::new();
        for col in 0..cols {
            let contents = screen
                .cell(row, col)
                .filter(|cell| !cell.is_wide_continuation() && cell.has_contents())
                .map(|cell| cell.contents())
                .unwrap_or(" ");
            line.push_str(contents);
        }
        let trimmed = line.trim_end().to_string();
        if !trimmed.trim().is_empty() {
            lines.push(trimmed);
            if lines.len() >= max_rows {
                break;
            }
        }
    }
    lines
}

#[cfg(test)]
fn terminal_response_for_output(screen: &vt100::Screen, bytes: &[u8]) -> Option<Vec<u8>> {
    terminal_response_for_combined_output(screen, bytes, 0)
}

fn terminal_response_for_combined_output(
    screen: &vt100::Screen,
    bytes: &[u8],
    previous_len: usize,
) -> Option<Vec<u8>> {
    let mut response = Vec::new();

    if contains_new_sequence(bytes, previous_len, b"\x1b[5n") {
        response.extend_from_slice(b"\x1b[0n");
    }

    if contains_new_sequence(bytes, previous_len, b"\x1b[6n") {
        let (row, col) = screen.cursor_position();
        response.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
    }

    if contains_new_sequence(bytes, previous_len, b"\x1b[?6n") {
        let (row, col) = screen.cursor_position();
        response.extend_from_slice(format!("\x1b[?{};{}R", row + 1, col + 1).as_bytes());
    }

    (!response.is_empty()).then_some(response)
}

fn contains_new_sequence(bytes: &[u8], previous_len: usize, sequence: &[u8]) -> bool {
    if sequence.is_empty() || bytes.len() < sequence.len() {
        return false;
    }

    let start = previous_len.saturating_sub(sequence.len().saturating_sub(1));
    let end = bytes.len() - sequence.len();
    for index in start..=end {
        if index + sequence.len() <= previous_len {
            continue;
        }
        if &bytes[index..index + sequence.len()] == sequence {
            return true;
        }
    }
    false
}

fn debug_log(event: &str, details: serde_json::Value) {
    if !DEBUG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let msg = if details.as_object().is_some_and(|object| object.is_empty()) {
        event.to_string()
    } else {
        match serde_json::to_string(&details) {
            Ok(details) => format!("{} {}", event, details),
            Err(_) => event.to_string(),
        }
    };
    debug_log_to(debug_log_file_for(event), &msg);
}

fn trace_log(event: &str, details: serde_json::Value) {
    if TRACE_ENABLED.load(Ordering::Relaxed) {
        debug_log(event, details);
    }
}

#[cfg(not(windows))]
fn prepare_agent_daemon_process() {}

#[cfg(windows)]
fn prepare_agent_daemon_process() {
    let handler_ok =
        unsafe { SetConsoleCtrlHandler(Some(agent_daemon_console_ctrl_handler), 1) } != 0;
    let handler_error = if handler_ok {
        None
    } else {
        Some(io::Error::last_os_error().to_string())
    };
    let free_console_ok = unsafe { FreeConsole() } != 0;
    let free_console_error = if free_console_ok {
        None
    } else {
        Some(io::Error::last_os_error().to_string())
    };
    debug_log(
        "daemon_windows_process_detached",
        serde_json::json!({
            "console_ctrl_handler": handler_ok,
            "console_ctrl_handler_error": handler_error,
            "free_console": free_console_ok,
            "free_console_error": free_console_error,
        }),
    );
}

#[cfg(windows)]
unsafe extern "system" fn agent_daemon_console_ctrl_handler(ctrl_type: u32) -> i32 {
    WINDOWS_DAEMON_CTRL_EVENT_LAST.store(ctrl_type, Ordering::Relaxed);
    WINDOWS_DAEMON_CTRL_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);

    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const CTRL_CLOSE_EVENT: u32 = 2;
    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => 1,
        _ => 0,
    }
}

#[cfg(windows)]
fn windows_console_ctrl_event_snapshot() -> (u32, Option<u32>) {
    let count = WINDOWS_DAEMON_CTRL_EVENT_COUNT.load(Ordering::Relaxed);
    let last_event = if count == 0 {
        None
    } else {
        Some(WINDOWS_DAEMON_CTRL_EVENT_LAST.load(Ordering::Relaxed))
    };
    (count, last_event)
}

fn debug_log_file_for(_event: &str) -> &'static str {
    DEBUG_LOG_FILE
}

fn debug_log_to(filename: &str, msg: &str) {
    if !DEBUG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(dir) = app_config_dir().map(|dir| dir.join("debug")) else {
        return;
    };
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    #[cfg(unix)]
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));

    let path = dir.join(filename);
    if path
        .metadata()
        .map(|meta| meta.len() > DEBUG_LOG_MAX_BYTES)
        .unwrap_or(false)
    {
        let rotated = dir.join(format!("{}.1", filename));
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, rotated);
    }

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    #[cfg(unix)]
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
    let thread = thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    let thread_id = format!("{:?}", thread.id());
    let _ = writeln!(
        file,
        "[{} pid={} thread={} {}] {}",
        timestamp,
        std::process::id(),
        thread_name,
        thread_id,
        msg
    );
}

fn write_json_line<T: Serialize>(writer: &mut AgentStream, value: &T) -> io::Result<()> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
    bytes.push(b'\n');
    write_all_retry(writer, &bytes)?;
    writer.flush()
}

fn write_all_retry(writer: &mut AgentStream, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::WriteZero,
                    "socket write returned 0",
                ))
            }
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Body of the per-AgentClient reader thread. Blocks on `stream.read`,
/// parses newline-delimited `AgentDaemonEvent` JSON, and forwards each
/// parsed event to the main loop via `tx`. Returns a short reason string
/// when the stream is shut down (by main thread on Drop) or fails.
fn run_agent_reader_thread(
    mut stream: AgentStream,
    tx: &Sender<MainEvent>,
    reader_id: u64,
) -> String {
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => return "eof".into(),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return format!("read error: {}", e),
        }
        // Drain whatever complete lines accumulated.
        while let Some(pos) = buf.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = &line[..line.len().saturating_sub(1)];
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<AgentDaemonEvent>(line) {
                Ok(event) => {
                    if tx.send(MainEvent::AgentEvent { reader_id, event }).is_err() {
                        return "main receiver dropped".into();
                    }
                }
                Err(e) => {
                    return format!("invalid event json: {}", e);
                }
            }
        }
    }
}

fn read_agent_daemon_requests(
    stream: &mut AgentStream,
    read_buf: &mut Vec<u8>,
) -> io::Result<Vec<AgentDaemonRequest>> {
    let mut tmp = [0u8; 8192];
    let mut saw_eof = false;
    let mut total_read = 0usize;
    let initial_buffer_len = read_buf.len();
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => {
                saw_eof = true;
                break;
            }
            Ok(n) => {
                total_read = total_read.saturating_add(n);
                read_buf.extend_from_slice(&tmp[..n]);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                debug_log(
                    "daemon_request_read_error",
                    serde_json::json!({
                        "initial_buffer_len": initial_buffer_len,
                        "buffer_len": read_buf.len(),
                        "bytes_read": total_read,
                        "error_kind": format!("{:?}", e.kind()),
                        "error": e.to_string(),
                    }),
                );
                return Err(e);
            }
        }
    }

    let mut messages = Vec::new();
    while let Some(pos) = read_buf.iter().position(|byte| *byte == b'\n') {
        let line = read_buf.drain(..=pos).collect::<Vec<_>>();
        let line = &line[..line.len().saturating_sub(1)];
        if line.is_empty() {
            continue;
        }
        let msg = serde_json::from_slice::<AgentDaemonRequest>(line).map_err(|e| {
            debug_log(
                "daemon_request_parse_failed",
                serde_json::json!({
                    "line_len": line.len(),
                    "sample": debug_bytes_sample(line, 512),
                    "buffer_len_after_drain": read_buf.len(),
                    "error": e.to_string(),
                }),
            );
            io::Error::new(ErrorKind::InvalidData, e)
        })?;
        messages.push(msg);
    }
    if total_read > 0 || saw_eof || !messages.is_empty() {
        let details = serde_json::json!({
            "initial_buffer_len": initial_buffer_len,
            "bytes_read": total_read,
            "saw_eof": saw_eof,
            "messages": messages.iter().map(agent_daemon_request_debug_value).collect::<Vec<_>>(),
            "remaining_buffer_len": read_buf.len(),
        });
        if agent_daemon_requests_are_input_only(&messages) && !saw_eof {
            trace_log("daemon_request_read", details);
        } else {
            debug_log("daemon_request_read", details);
        }
    }
    if saw_eof && messages.is_empty() {
        return Err(io::Error::new(ErrorKind::BrokenPipe, "socket closed"));
    }
    Ok(messages)
}

fn agent_daemon_request_debug_value(request: &AgentDaemonRequest) -> serde_json::Value {
    match request {
        AgentDaemonRequest::Attach {
            cols,
            rows,
            client_pid,
        } => serde_json::json!({
            "type": "attach",
            "cols": cols,
            "rows": rows,
            "client_pid": client_pid,
        }),
        AgentDaemonRequest::Resize { cols, rows } => serde_json::json!({
            "type": "resize",
            "cols": cols,
            "rows": rows,
        }),
        AgentDaemonRequest::Input { data } => serde_json::json!({
            "type": "input",
            "len": data.len(),
            "sample": debug_bytes_sample(data, 128),
        }),
        AgentDaemonRequest::Detach => serde_json::json!({
            "type": "detach",
        }),
    }
}

fn agent_daemon_requests_are_input_only(requests: &[AgentDaemonRequest]) -> bool {
    !requests.is_empty()
        && requests
            .iter()
            .all(|request| matches!(request, AgentDaemonRequest::Input { .. }))
}

#[cfg(unix)]
fn bind_agent_listener(_key: &AgentKey, socket_path: &Path) -> io::Result<AgentListener> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = AgentListener::bind(socket_path)?;
    debug_log(
        "daemon_listener_bound_unix",
        serde_json::json!({
            "socket_path": socket_path.display().to_string(),
        }),
    );
    Ok(listener)
}

#[cfg(windows)]
fn bind_agent_listener(_key: &AgentKey, socket_path: &Path) -> io::Result<AgentListener> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = AgentListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    if let Err(e) = fs::write(socket_path, format!("tcp {addr}\n")) {
        debug_log(
            "daemon_listener_marker_write_failed",
            serde_json::json!({
                "socket_path": socket_path.display().to_string(),
                "addr": addr.to_string(),
                "error": e.to_string(),
            }),
        );
        return Err(e);
    }
    debug_log(
        "daemon_listener_bound_windows",
        serde_json::json!({
            "socket_path": socket_path.display().to_string(),
            "addr": addr.to_string(),
        }),
    );
    Ok(listener)
}

fn connect_agent_daemon(key: &AgentKey) -> io::Result<AgentStream> {
    #[cfg(unix)]
    {
        let socket_path = agent_socket_path(key).map_err(io::Error::other)?;
        match AgentStream::connect(&socket_path) {
            Ok(stream) => {
                debug_log(
                    "agent_daemon_connect_ok",
                    serde_json::json!({
                        "key": agent_key_debug_value(key),
                        "socket_path": socket_path.display().to_string(),
                    }),
                );
                Ok(stream)
            }
            Err(e) => {
                debug_log(
                    "agent_daemon_connect_failed",
                    serde_json::json!({
                        "key": agent_key_debug_value(key),
                        "socket_path": socket_path.display().to_string(),
                        "socket_exists": socket_path.exists(),
                        "error_kind": format!("{:?}", e.kind()),
                        "error": e.to_string(),
                    }),
                );
                Err(e)
            }
        }
    }
    #[cfg(windows)]
    {
        let socket_path = agent_socket_path(key).map_err(io::Error::other)?;
        let addr = match read_agent_tcp_addr(&socket_path) {
            Ok(addr) => addr,
            Err(e) => {
                debug_log(
                    "agent_daemon_connect_marker_failed",
                    serde_json::json!({
                        "key": agent_key_debug_value(key),
                        "socket_path": socket_path.display().to_string(),
                        "socket_exists": socket_path.exists(),
                        "error_kind": format!("{:?}", e.kind()),
                        "error": e.to_string(),
                    }),
                );
                return Err(e);
            }
        };
        match AgentStream::connect(&addr) {
            Ok(stream) => {
                debug_log(
                    "agent_daemon_connect_ok",
                    serde_json::json!({
                        "key": agent_key_debug_value(key),
                        "socket_path": socket_path.display().to_string(),
                        "addr": &addr,
                    }),
                );
                Ok(stream)
            }
            Err(e) => {
                debug_log(
                    "agent_daemon_connect_failed",
                    serde_json::json!({
                        "key": agent_key_debug_value(key),
                        "socket_path": socket_path.display().to_string(),
                        "addr": &addr,
                        "error_kind": format!("{:?}", e.kind()),
                        "error": e.to_string(),
                    }),
                );
                Err(e)
            }
        }
    }
}

#[cfg(windows)]
fn read_agent_tcp_addr(socket_path: &Path) -> io::Result<String> {
    let marker = fs::read_to_string(socket_path)?;
    let addr = marker.trim().strip_prefix("tcp ").unwrap_or(marker.trim());
    if !addr.starts_with("127.0.0.1:") {
        debug_log(
            "agent_tcp_marker_invalid",
            serde_json::json!({
                "socket_path": socket_path.display().to_string(),
                "marker": truncate_width(&marker, 256),
            }),
        );
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid agent tcp marker",
        ));
    }
    debug_log(
        "agent_tcp_marker_read",
        serde_json::json!({
            "socket_path": socket_path.display().to_string(),
            "addr": addr,
        }),
    );
    Ok(addr.to_string())
}

fn wait_for_agent_daemon(key: &AgentKey) -> Result<AgentStream> {
    let started = Instant::now();
    let timeout = Duration::from_millis(AGENT_DAEMON_START_TIMEOUT_MS);
    let mut last_error: Option<io::Error> = None;
    let mut attempt = 0u32;
    debug_log(
        "daemon_wait_start",
        serde_json::json!({
            "key": agent_key_debug_value(key),
            "timeout_ms": AGENT_DAEMON_START_TIMEOUT_MS,
        }),
    );
    while started.elapsed() < timeout {
        attempt = attempt.saturating_add(1);
        match connect_agent_daemon(key) {
            Ok(stream) => {
                debug_log(
                    "daemon_wait_connected",
                    serde_json::json!({
                        "key": agent_key_debug_value(key),
                        "attempt": attempt,
                        "elapsed_ms": started.elapsed().as_millis(),
                    }),
                );
                return Ok(stream);
            }
            Err(e) => {
                if attempt <= 10 || attempt % 10 == 0 {
                    debug_log(
                        "daemon_wait_retry",
                        serde_json::json!({
                            "key": agent_key_debug_value(key),
                            "attempt": attempt,
                            "elapsed_ms": started.elapsed().as_millis(),
                            "error_kind": format!("{:?}", e.kind()),
                            "error": e.to_string(),
                        }),
                    );
                }
                last_error = Some(e);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let error =
        last_error.unwrap_or_else(|| io::Error::new(ErrorKind::TimedOut, "daemon did not start"));
    debug_log(
        "daemon_wait_failed",
        serde_json::json!({
            "key": agent_key_debug_value(key),
            "attempts": attempt,
            "elapsed_ms": started.elapsed().as_millis(),
            "error_kind": format!("{:?}", error.kind()),
            "error": error.to_string(),
        }),
    );
    Err(error.into())
}

fn start_agent_daemon(info: &SessionInfo, launch_mode: AgentLaunchMode) -> Result<()> {
    let key = AgentKey::new(info);
    debug_log(
        "daemon_start_check",
        serde_json::json!({
            "info": session_info_debug_value(info),
            "launch_mode": launch_mode.as_str(),
        }),
    );
    match connect_agent_daemon(&key) {
        Ok(_) => {
            debug_log(
                "daemon_reuse_existing",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "launch_mode": launch_mode.as_str(),
                }),
            );
            return Ok(());
        }
        Err(e) => {
            debug_log(
                "daemon_existing_connect_unavailable",
                serde_json::json!({
                    "key": agent_key_debug_value(&key),
                    "error_kind": format!("{:?}", e.kind()),
                    "error": e.to_string(),
                }),
            );
        }
    }
    if let Some(meta) = live_agent_meta_snapshot(&key) {
        debug_log(
            "daemon_start_blocked_unreachable_live",
            serde_json::json!({
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
                "daemon_pid": meta.pid,
            }),
        );
        return Err(anyhow::anyhow!(
            "live {} agent {} is already running as pid {}, but its socket is unreachable; refusing to replace a background process",
            key.provider.as_str(),
            truncate_width(&key.session_id, 14),
            meta.pid
        ));
    }
    validate_session_launch_cwd(info)?;
    let socket_path = agent_socket_path(&key)?;
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let current_exe = std::env::current_exe()?;
    let command = build_agent_daemon_command(&current_exe, info, launch_mode);
    debug_log(
        "daemon_spawn_command_prepared",
        serde_json::json!({
            "key": agent_key_debug_value(&key),
            "launch_mode": launch_mode.as_str(),
            "current_exe": current_exe.display().to_string(),
            "debug_env_forwarded": DEBUG_ENABLED.load(Ordering::Relaxed),
            "socket_path": socket_path.display().to_string(),
            "source": info.source.display().to_string(),
            "cwd": &info.cwd,
            "windows_breakaway_requested": daemon_uses_windows_breakaway(),
        }),
    );
    let (child, windows_breakaway_used) =
        spawn_agent_daemon_process(command, &current_exe, info, launch_mode, &key)?;
    debug_log(
        "daemon_spawned",
        serde_json::json!({
            "provider": key.provider.as_str(),
            "session_id": &key.session_id,
            "launch_mode": launch_mode.as_str(),
            "daemon_pid": child.id(),
            "windows_breakaway_requested": daemon_uses_windows_breakaway(),
            "windows_breakaway_used": windows_breakaway_used,
        }),
    );
    Ok(())
}

fn build_agent_daemon_command(
    current_exe: &Path,
    info: &SessionInfo,
    launch_mode: AgentLaunchMode,
) -> Command {
    let mut command = Command::new(current_exe);
    command
        .arg(AGENT_DAEMON_ARG)
        .arg(info.provider.as_str())
        .arg(&info.session_id)
        .arg(&info.cwd)
        .arg(info.source.as_os_str())
        .arg(launch_mode.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if DEBUG_ENABLED.load(Ordering::Relaxed) {
        command.env("COKACMUX_DEBUG", "1");
    }
    if TRACE_ENABLED.load(Ordering::Relaxed) {
        command.env("COKACMUX_TRACE", "1");
    }
    command
}

#[cfg(unix)]
fn spawn_agent_daemon_process(
    mut command: Command,
    _current_exe: &Path,
    _info: &SessionInfo,
    _launch_mode: AgentLaunchMode,
    _key: &AgentKey,
) -> io::Result<(std::process::Child, bool)> {
    configure_daemon_command(&mut command);
    command.spawn().map(|child| (child, false))
}

#[cfg(windows)]
fn spawn_agent_daemon_process(
    mut command: Command,
    _current_exe: &Path,
    _info: &SessionInfo,
    launch_mode: AgentLaunchMode,
    key: &AgentKey,
) -> io::Result<(std::process::Child, bool)> {
    configure_daemon_command(&mut command, true);
    match command.spawn() {
        Ok(child) => Ok((child, true)),
        Err(e) => {
            debug_log(
                "daemon_spawn_breakaway_failed",
                serde_json::json!({
                    "key": agent_key_debug_value(key),
                    "launch_mode": launch_mode.as_str(),
                    "error_kind": format!("{:?}", e.kind()),
                    "error": e.to_string(),
                }),
            );
            Err(e)
        }
    }
}

#[cfg(not(windows))]
fn daemon_uses_windows_breakaway() -> bool {
    false
}

#[cfg(windows)]
fn daemon_uses_windows_breakaway() -> bool {
    true
}

#[cfg(unix)]
fn configure_daemon_command(command: &mut Command) {
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
fn configure_daemon_command(command: &mut Command, breakaway: bool) {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let mut flags = CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW;
    if breakaway {
        flags |= CREATE_BREAKAWAY_FROM_JOB;
    }
    command.creation_flags(flags);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AgentTermination {
    pid: Option<u32>,
    pty_log_deleted: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct KillAllAgentsReport {
    scanned: usize,
    killed: usize,
    stale: usize,
    skipped_self: usize,
    errors: usize,
    pty_logs_deleted: usize,
}

fn kill_all_agent_daemons() -> Result<KillAllAgentsReport> {
    let runtime_dir = agent_runtime_dir()?;
    Ok(kill_all_agent_daemons_at(&runtime_dir, std::process::id()))
}

fn kill_all_agent_daemons_at(runtime_dir: &Path, current_pid: u32) -> KillAllAgentsReport {
    let Ok(read_dir) = fs::read_dir(runtime_dir) else {
        return KillAllAgentsReport::default();
    };
    let mut report = KillAllAgentsReport::default();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        report.scanned = report.scanned.saturating_add(1);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                report.errors = report.errors.saturating_add(1);
                debug_log(
                    "killall_meta_read_failed",
                    serde_json::json!({
                        "path": path.display().to_string(),
                        "error": e.to_string(),
                    }),
                );
                continue;
            }
        };
        let meta = match serde_json::from_str::<AgentMetaSnapshot>(&content) {
            Ok(meta) => meta,
            Err(e) => {
                report.errors = report.errors.saturating_add(1);
                debug_log(
                    "killall_meta_parse_failed",
                    serde_json::json!({
                        "path": path.display().to_string(),
                        "error": e.to_string(),
                    }),
                );
                continue;
            }
        };
        if meta.pid == current_pid {
            report.skipped_self = report.skipped_self.saturating_add(1);
            continue;
        }
        if !process_is_alive(meta.pid) {
            report.stale = report.stale.saturating_add(1);
            if remove_agent_runtime_files_by_stem(runtime_dir, stem) {
                report.pty_logs_deleted = report.pty_logs_deleted.saturating_add(1);
            }
            continue;
        }
        let Some(key) = agent_key_from_meta(&meta) else {
            report.stale = report.stale.saturating_add(1);
            let _ = remove_agent_runtime_files_by_stem(runtime_dir, stem);
            debug_log(
                "killall_agent_unverified",
                serde_json::json!({
                    "path": path.display().to_string(),
                    "daemon_pid": meta.pid,
                    "reason": "missing_provider_or_session_id",
                }),
            );
            continue;
        };
        if agent_file_stem(&key) != stem || !verify_agent_daemon_identity(&key, meta.pid) {
            report.stale = report.stale.saturating_add(1);
            let _ = remove_agent_runtime_files_by_stem(runtime_dir, stem);
            debug_log(
                "killall_agent_unverified",
                serde_json::json!({
                    "path": path.display().to_string(),
                    "daemon_pid": meta.pid,
                    "provider": meta.provider.as_deref(),
                    "session_id": meta.session_id.as_deref(),
                    "reason": "identity_check_failed",
                }),
            );
            continue;
        }

        terminate_process_group(meta.pid);
        if process_is_alive(meta.pid) {
            report.errors = report.errors.saturating_add(1);
            debug_log(
                "killall_agent_still_alive",
                serde_json::json!({
                    "path": path.display().to_string(),
                    "daemon_pid": meta.pid,
                }),
            );
            continue;
        }
        report.killed = report.killed.saturating_add(1);
        if remove_agent_runtime_files_by_stem(runtime_dir, stem) {
            report.pty_logs_deleted = report.pty_logs_deleted.saturating_add(1);
        }
        debug_log(
            "killall_agent_killed",
            serde_json::json!({
                "path": path.display().to_string(),
                "daemon_pid": meta.pid,
                "provider": meta.provider.as_deref(),
                "session_id": meta.session_id.as_deref(),
            }),
        );
    }
    report
}

fn remove_agent_runtime_files_by_stem(runtime_dir: &Path, stem: &str) -> bool {
    let _ = fs::remove_file(runtime_dir.join(format!("{}.json", stem)));
    let _ = fs::remove_file(runtime_dir.join(format!("{}.sock", stem)));
    let _ = fs::remove_file(runtime_dir.join(format!("{}.tcp", stem)));
    fs::remove_file(
        runtime_dir
            .join("scrollback")
            .join(format!("{}.ptylog", stem)),
    )
    .is_ok()
}

fn terminate_agent_daemon(key: &AgentKey) -> Result<AgentTermination> {
    let meta_path = agent_meta_path(key)?;
    let socket_path = agent_socket_path(key)?;
    let meta = fs::read_to_string(&meta_path)
        .ok()
        .and_then(|content| serde_json::from_str::<AgentMetaSnapshot>(&content).ok());

    let mut terminated_pid = None;
    if let Some(meta) = meta.as_ref() {
        if process_is_alive(meta.pid) && verify_agent_daemon_identity(key, meta.pid) {
            let pid = meta.pid;
            terminate_process_group(pid);
            terminated_pid = Some(pid);
        } else if meta.pid > 0 {
            debug_log(
                "kill_agent_unverified",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "daemon_pid": meta.pid,
                }),
            );
        }
    }
    let _ = fs::remove_file(&meta_path);
    let _ = fs::remove_file(&socket_path);
    let pty_log_deleted = remove_agent_pty_log(key);
    Ok(AgentTermination {
        pid: terminated_pid,
        pty_log_deleted,
    })
}

fn verify_agent_daemon_identity(key: &AgentKey, expected_pid: u32) -> bool {
    if expected_pid == 0 || !process_is_alive(expected_pid) {
        return false;
    }
    let verified = process_cmdline_matches_agent_daemon(expected_pid, key);
    if !verified {
        debug_log(
            "agent_daemon_identity_cmdline_failed",
            serde_json::json!({
                "provider": key.provider.as_str(),
                "session_id": &key.session_id,
                "daemon_pid": expected_pid,
            }),
        );
    }
    verified
}

#[cfg(target_os = "linux")]
fn process_cmdline_matches_agent_daemon(pid: u32, key: &AgentKey) -> bool {
    fs::read(format!("/proc/{}/cmdline", pid))
        .ok()
        .map(|bytes| {
            let args = bytes
                .split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect::<Vec<_>>();
            agent_daemon_args_match(&args, key)
        })
        .unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_cmdline_matches_agent_daemon(pid: u32, key: &AgentKey) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            process_command_string_matches_agent_daemon(
                String::from_utf8_lossy(&output.stdout).trim(),
                key,
            )
        })
        .unwrap_or(false)
}

#[cfg(windows)]
fn process_cmdline_matches_agent_daemon(pid: u32, key: &AgentKey) -> bool {
    let command = format!(
        "$p = Get-CimInstance Win32_Process -Filter \"ProcessId = {}\"; if ($p) {{ $p.CommandLine }}",
        pid
    );
    let output = match Command::new("powershell")
        .args(["-NoProfile", "-Command", &command])
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            debug_log(
                "agent_daemon_identity_cmdline_probe_failed",
                serde_json::json!({
                    "pid": pid,
                    "key": agent_key_debug_value(key),
                    "error": e.to_string(),
                }),
            );
            return false;
        }
    };
    let command_line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let matched =
        output.status.success() && process_command_string_matches_agent_daemon(&command_line, key);
    debug_log(
        "agent_daemon_identity_cmdline_probe",
        serde_json::json!({
            "pid": pid,
            "key": agent_key_debug_value(key),
            "status": output.status.to_string(),
            "stdout_len": output.stdout.len(),
            "stderr": truncate_width(&stderr, 512),
            "command_line": truncate_width(&command_line, 512),
            "matched": matched,
        }),
    );
    matched
}

fn agent_daemon_args_match(args: &[String], key: &AgentKey) -> bool {
    let Some(program) = args.first() else {
        return false;
    };
    if !program_looks_like_current_app(program) {
        return false;
    }
    let Some(pos) = args.iter().position(|arg| arg == AGENT_DAEMON_ARG) else {
        return false;
    };
    args.get(pos + 1).map(String::as_str) == Some(key.provider.as_str())
        && args.get(pos + 2).map(String::as_str) == Some(key.session_id.as_str())
}

#[cfg(windows)]
fn process_command_string_matches_agent_daemon(command: &str, key: &AgentKey) -> bool {
    windows_command_line_to_args(command).is_some_and(|args| agent_daemon_args_match(&args, key))
}

#[cfg(windows)]
fn windows_command_line_to_args(command: &str) -> Option<Vec<String>> {
    use std::ffi::c_void;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn CommandLineToArgvW(lpCmdLine: *const u16, pNumArgs: *mut i32) -> *mut *mut u16;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(hMem: *mut c_void) -> *mut c_void;
    }

    let wide = command
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut argc = 0i32;
    let argv = unsafe { CommandLineToArgvW(wide.as_ptr(), &mut argc) };
    if argv.is_null() || argc < 0 {
        return None;
    }

    let args = unsafe {
        std::slice::from_raw_parts(argv, argc as usize)
            .iter()
            .map(|arg| {
                let ptr = *arg;
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len = len.saturating_add(1);
                }
                OsString::from_wide(std::slice::from_raw_parts(ptr, len))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>()
    };
    let _ = unsafe { LocalFree(argv.cast::<c_void>()) };
    Some(args)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_command_string_matches_agent_daemon(command: &str, key: &AgentKey) -> bool {
    command_string_to_args(command).is_some_and(|args| agent_daemon_args_match(&args, key))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn command_string_to_args(command: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, '\'') | (None, '"') => quote = Some(ch),
            (Some(q), ch) if ch == q => quote = None,
            (None, ch) if ch.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            _ => current.push(ch),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        args.push(current);
    }
    Some(args)
}

fn program_looks_like_current_app(program: &str) -> bool {
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    current_app_file_name()
        .as_deref()
        .is_some_and(|current| current_app_file_name_matches(name, current))
}

#[cfg(windows)]
fn current_app_file_name_matches(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[cfg(not(windows))]
fn current_app_file_name_matches(left: &str, right: &str) -> bool {
    left == right
}

fn current_app_file_name() -> Option<String> {
    std::env::current_exe().ok().and_then(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    })
}

fn remove_agent_pty_log(key: &AgentKey) -> bool {
    let Ok(path) = agent_pty_log_path(key) else {
        return false;
    };
    remove_agent_pty_log_file(key, &path)
}

fn remove_agent_pty_log_file(key: &AgentKey, path: &Path) -> bool {
    match fs::remove_file(&path) {
        Ok(()) => {
            debug_log(
                "agent_pty_log_removed",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "path": path.display().to_string(),
                }),
            );
            true
        }
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => {
            debug_log(
                "agent_pty_log_remove_failed",
                serde_json::json!({
                    "provider": key.provider.as_str(),
                    "session_id": &key.session_id,
                    "path": path.display().to_string(),
                    "error": e.to_string(),
                }),
            );
            false
        }
    }
}

fn cleanup_orphan_agent_pty_logs() -> usize {
    let Ok(runtime_dir) = agent_runtime_dir() else {
        return 0;
    };
    cleanup_orphan_agent_pty_logs_at(&runtime_dir)
}

fn cleanup_orphan_agent_pty_logs_at(runtime_dir: &Path) -> usize {
    let scrollback_dir = runtime_dir.join("scrollback");
    let Ok(read_dir) = fs::read_dir(&scrollback_dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("ptylog") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let meta_path = runtime_dir.join(format!("{}.json", stem));
        let has_live_meta = fs::read_to_string(&meta_path)
            .ok()
            .and_then(|content| serde_json::from_str::<AgentMetaSnapshot>(&content).ok())
            .map(|meta| process_is_alive(meta.pid))
            .unwrap_or(false);
        if has_live_meta {
            continue;
        }

        match fs::remove_file(&path) {
            Ok(()) => {
                removed = removed.saturating_add(1);
                debug_log(
                    "agent_orphan_pty_log_removed",
                    serde_json::json!({
                        "path": path.display().to_string(),
                        "meta_path": meta_path.display().to_string(),
                    }),
                );
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                debug_log(
                    "agent_orphan_pty_log_remove_failed",
                    serde_json::json!({
                        "path": path.display().to_string(),
                        "meta_path": meta_path.display().to_string(),
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }
    removed
}

fn agent_history_deleted_suffix(deleted: bool) -> &'static str {
    if deleted {
        "; history deleted"
    } else {
        ""
    }
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    if pid == 0 || pid > i32::MAX as u32 {
        return;
    }
    let pgid = -(pid as i32);
    unsafe {
        let _ = libc::kill(pgid, libc::SIGTERM);
    }
    thread::sleep(Duration::from_millis(150));
    if process_is_alive(pid) {
        unsafe {
            let _ = libc::kill(pgid, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
fn terminate_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn agent_socket_path(key: &AgentKey) -> Result<PathBuf> {
    #[cfg(unix)]
    let ext = "sock";
    #[cfg(windows)]
    let ext = "tcp";
    Ok(agent_runtime_dir()?.join(format!("{}.{}", agent_file_stem(key), ext)))
}

fn agent_meta_path(key: &AgentKey) -> Result<PathBuf> {
    Ok(agent_runtime_dir()?.join(format!("{}.json", agent_file_stem(key))))
}

fn agent_pty_log_path(key: &AgentKey) -> Result<PathBuf> {
    Ok(agent_runtime_dir()?
        .join("scrollback")
        .join(format!("{}.ptylog", agent_file_stem(key))))
}

fn agent_runtime_dir() -> Result<PathBuf> {
    let Some(dir) = app_config_dir().map(|dir| dir.join("agents")) else {
        anyhow::bail!("cannot resolve home directory");
    };
    Ok(dir)
}

/// Scan `~/.cokacmux/agents/` for live daemons that are not in the provider
/// session list: shell panes and fresh coding-agent panes. They are marked by
/// a synthetic `source` value in the meta JSON, so the agents sidebar can list
/// and switch to them alongside stored Claude/Codex/OpenCode sessions.
fn discover_live_shell_infos() -> Vec<SessionInfo> {
    let Ok(dir) = agent_runtime_dir() else {
        trace_log(
            "live_shell_discover_failed",
            serde_json::json!({
                "reason": "runtime_dir",
            }),
        );
        return Vec::new();
    };
    let Ok(read_dir) = fs::read_dir(&dir) else {
        trace_log(
            "live_shell_discover_failed",
            serde_json::json!({
                "reason": "read_dir",
                "dir": dir.display().to_string(),
            }),
        );
        return Vec::new();
    };
    trace_log(
        "live_shell_discover_start",
        serde_json::json!({
            "dir": dir.display().to_string(),
        }),
    );
    let mut out: Vec<SessionInfo> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                trace_log(
                    "live_shell_discover_skip",
                    serde_json::json!({
                        "reason": "meta_read_failed",
                        "path": path.display().to_string(),
                        "error": e.to_string(),
                    }),
                );
                continue;
            }
        };
        let meta = match serde_json::from_str::<AgentMetaSnapshot>(&content) {
            Ok(meta) => meta,
            Err(e) => {
                trace_log(
                    "live_shell_discover_skip",
                    serde_json::json!({
                        "reason": "meta_parse_failed",
                        "path": path.display().to_string(),
                        "error": e.to_string(),
                        "content_sample": truncate_width(&content, 512),
                    }),
                );
                continue;
            }
        };
        let Some(source) = meta.source.as_deref() else {
            trace_log(
                "live_shell_discover_skip",
                serde_json::json!({
                    "reason": "missing_source",
                    "path": path.display().to_string(),
                    "daemon_pid": meta.pid,
                    "provider": meta.provider.as_deref(),
                    "session_id": meta.session_id.as_deref(),
                }),
            );
            continue;
        };
        if source != SHELL_SESSION_SOURCE_MARKER && source != NEW_AGENT_SESSION_SOURCE_MARKER {
            trace_log(
                "live_shell_discover_skip",
                serde_json::json!({
                    "reason": "non_synthetic_source",
                    "path": path.display().to_string(),
                    "source": source,
                    "daemon_pid": meta.pid,
                    "provider": meta.provider.as_deref(),
                    "session_id": meta.session_id.as_deref(),
                }),
            );
            continue;
        }
        let Some(provider) = meta.provider.as_deref().and_then(Provider::parse) else {
            trace_log(
                "live_shell_discover_skip",
                serde_json::json!({
                    "reason": "invalid_provider",
                    "path": path.display().to_string(),
                    "source": source,
                    "daemon_pid": meta.pid,
                    "provider": meta.provider.as_deref(),
                    "session_id": meta.session_id.as_deref(),
                }),
            );
            continue;
        };
        let Some(session_id) = meta.session_id.clone() else {
            trace_log(
                "live_shell_discover_skip",
                serde_json::json!({
                    "reason": "missing_session_id",
                    "path": path.display().to_string(),
                    "source": source,
                    "daemon_pid": meta.pid,
                    "provider": provider.as_str(),
                }),
            );
            continue;
        };
        let cwd = meta.cwd.clone().unwrap_or_default();
        let title = if source == SHELL_SESSION_SOURCE_MARKER {
            shell_pane_title(&cwd)
        } else {
            new_agent_pane_title(provider, &cwd)
        };
        let debug_session_id = session_id.clone();
        let debug_cwd = cwd.clone();
        out.push(SessionInfo {
            provider,
            session_id,
            cwd,
            source: PathBuf::from(source),
            updated_at_epoch_s: 0,
            title: Some(title),
        });
        trace_log(
            "live_shell_discover_add",
            serde_json::json!({
                "path": path.display().to_string(),
                "source": source,
                "daemon_pid": meta.pid,
                "daemon_alive": process_is_alive(meta.pid),
                "provider": provider.as_str(),
                "session_id": &debug_session_id,
                "cwd": &debug_cwd,
                "attached": meta.attached,
                "attached_client_pid": meta.attached_client_pid,
            }),
        );
    }
    trace_log(
        "live_shell_discover_done",
        serde_json::json!({
            "count": out.len(),
            "items": out.iter().map(session_info_debug_value).collect::<Vec<_>>(),
        }),
    );
    out
}

fn agent_file_stem(key: &AgentKey) -> String {
    format!(
        "{}-{}",
        key.provider.as_str(),
        key.session_id
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>()
    )
}

fn prepare_agent_session(info: &SessionInfo) -> Result<()> {
    if is_shell_session_info(info) || is_new_agent_session_info(info) {
        return Ok(());
    }
    if info.provider == Provider::Codex {
        repair_cokacmux_codex_rollout(info)?;
    }
    Ok(())
}

fn repair_cokacmux_codex_rollout(info: &SessionInfo) -> Result<()> {
    let key = AgentKey::new(info);
    let live_meta = live_agent_meta_snapshot(&key);
    repair_cokacmux_codex_rollout_guarded(info, live_meta.as_ref())
}

fn repair_cokacmux_codex_rollout_guarded(
    info: &SessionInfo,
    live_meta: Option<&AgentMetaSnapshot>,
) -> Result<()> {
    if !codex_rollout_needs_repair(&info.source)? {
        return Ok(());
    }
    if let Some(meta) = live_meta {
        debug_log(
            "codex_repair_skipped_live_daemon",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "daemon_pid": meta.pid,
            }),
        );
        return Ok(());
    }
    let session = session::load(info)?;
    let content = providers::codex::to_jsonl_string(
        &session,
        &providers::codex::CodexWriteOpts { replay_raw: false },
    )?;
    fs::write(&info.source, content)?;
    Ok(())
}

fn codex_rollout_needs_repair(path: &Path) -> Result<bool> {
    let content = fs::read_to_string(path)?;
    let mut converter_owned = false;
    let mut incompatible = false;
    let mut has_user_response_text = false;
    let mut has_agent_response_text = false;
    let mut has_user_display_event = false;
    let mut has_agent_display_event = false;

    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let record_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if value
            .get("timestamp")
            .is_some_and(|timestamp| timestamp.is_null())
        {
            incompatible = true;
        }

        let payload = value.get("payload").and_then(|payload| payload.as_object());
        if record_type == "session_meta" {
            if let Some(payload) = payload {
                for key in [
                    "timestamp",
                    "source",
                    "thread_source",
                    "model_provider",
                    "base_instructions",
                ] {
                    if !payload.contains_key(key) {
                        incompatible = true;
                    }
                }
            }
            if payload
                .and_then(|payload| payload.get("originator"))
                .and_then(|originator| originator.as_str())
                .is_some_and(is_cokacmux_owned_originator)
            {
                converter_owned = true;
            }
        } else if record_type == "response_item" {
            if let Some(payload) = payload {
                if payload.get("type").and_then(|value| value.as_str()) == Some("message") {
                    let has_text = payload
                        .get("content")
                        .and_then(|value| value.as_array())
                        .is_some_and(|items| {
                            items.iter().any(|item| {
                                item.get("text")
                                    .and_then(|value| value.as_str())
                                    .is_some_and(|text| !text.is_empty())
                            })
                        });
                    if has_text {
                        match payload.get("role").and_then(|value| value.as_str()) {
                            Some("user") => has_user_response_text = true,
                            Some("assistant") => has_agent_response_text = true,
                            _ => {}
                        }
                    }
                }
            }
            if payload.and_then(|payload| payload.get("id")).is_some() {
                incompatible = true;
            }
        } else if record_type == "event_msg" {
            if let Some(payload_type) = payload
                .and_then(|payload| payload.get("type"))
                .and_then(|payload_type| payload_type.as_str())
            {
                if payload_type.starts_with("synthesized.") {
                    incompatible = true;
                }
                if payload_type == "user_message" {
                    has_user_display_event = true;
                } else if payload_type == "agent_message" {
                    has_agent_display_event = true;
                }
            }
        }
    }

    if has_user_response_text && !has_user_display_event {
        incompatible = true;
    }
    if has_agent_response_text && !has_agent_display_event {
        incompatible = true;
    }

    Ok(converter_owned && incompatible)
}

fn is_cokacmux_owned_originator(originator: &str) -> bool {
    originator == "cokacmux"
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentLaunchSpec {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<PathBuf>,
}

impl AgentLaunchSpec {
    fn command_line(&self) -> String {
        self.env
            .iter()
            .map(|(key, value)| format!("{}={}", key, shell_display_word(value)))
            .chain(std::iter::once(shell_display_word(&self.program)))
            .chain(self.args.iter().map(|arg| shell_display_word(arg)))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn shell_display_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(not(windows))]
fn agent_command_builder(spec: &AgentLaunchSpec) -> CommandBuilder {
    let program = resolve_unix_agent_program(&spec.program)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| spec.program.clone());
    let mut command = CommandBuilder::new(program);
    command.args(&spec.args);
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    command
}

#[cfg(windows)]
fn agent_command_builder(spec: &AgentLaunchSpec) -> CommandBuilder {
    let program = resolve_windows_agent_program(&spec.program);
    let mut command = CommandBuilder::from_argv(windows_agent_command_argv(program, &spec.args));
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    command
}

#[cfg(windows)]
fn windows_agent_command_argv(program: PathBuf, args: &[String]) -> Vec<OsString> {
    if let Some(mut argv) = windows_npm_cmd_shim_argv(&program) {
        argv.extend(args.iter().map(OsString::from));
        return argv;
    }

    let mut argv = Vec::new();

    if is_windows_batch_script(&program) {
        argv.push(windows_comspec());
        argv.push(OsString::from("/D"));
        argv.push(OsString::from("/C"));
        argv.push(program.into_os_string());
    } else if is_windows_powershell_script(&program) {
        argv.push(OsString::from("powershell.exe"));
        argv.push(OsString::from("-NoLogo"));
        argv.push(OsString::from("-NoProfile"));
        argv.push(OsString::from("-ExecutionPolicy"));
        argv.push(OsString::from("Bypass"));
        argv.push(OsString::from("-File"));
        argv.push(program.into_os_string());
    } else {
        argv.push(program.into_os_string());
    }

    argv.extend(args.iter().map(OsString::from));
    argv
}

#[cfg(windows)]
fn windows_npm_cmd_shim_argv(program: &Path) -> Option<Vec<OsString>> {
    if !is_windows_batch_script(program) {
        return None;
    }

    let script = windows_npm_cmd_shim_script(program)?;
    let node = windows_node_for_npm_shim(program);
    Some(vec![node.into_os_string(), script.into_os_string()])
}

#[cfg(windows)]
fn windows_npm_cmd_shim_script(program: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(program).ok()?;
    let lower = text.to_ascii_lowercase();
    if !lower.contains("node_modules") || !lower.contains(".js") || !lower.contains("%*") {
        return None;
    }

    let start = lower.find("node_modules")?;
    let end = lower[start..]
        .find(".js")
        .map(|offset| start + offset + 3)?;
    let relative = &text[start..end];
    let script = program.parent()?.join(relative);
    script.exists().then_some(script)
}

#[cfg(windows)]
fn windows_node_for_npm_shim(program: &Path) -> PathBuf {
    if let Some(local_node) = program
        .parent()
        .map(|parent| parent.join("node.exe"))
        .filter(|path| path.exists())
    {
        return local_node;
    }

    resolve_windows_agent_program_with_env(
        "node",
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
    )
    .unwrap_or_else(|| PathBuf::from("node.exe"))
}

#[cfg(windows)]
fn resolve_windows_agent_program(program: &str) -> PathBuf {
    if let Some(provider) = provider_for_default_agent_program(program) {
        if let Some(path) = resolve_windows_agent_program_for_provider(provider, program) {
            if provider == Provider::OpenCode {
                if let Some(native) = opencode_native_exe_for_windows_wrapper(&path) {
                    return native;
                }
            }
            return path;
        }
    }
    resolve_windows_agent_program_with_env(
        program,
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
    )
    .unwrap_or_else(|| PathBuf::from(program))
}

#[cfg(windows)]
fn provider_for_default_agent_program(program: &str) -> Option<Provider> {
    let trimmed = program.trim();
    CLONE_PROVIDER_OPTIONS
        .iter()
        .copied()
        .find(|provider| trimmed.eq_ignore_ascii_case(default_agent_program(*provider)))
}

#[cfg(windows)]
fn resolve_windows_agent_program_with_env(
    program: &str,
    path_env: Option<OsString>,
    pathext_env: Option<OsString>,
) -> Option<PathBuf> {
    let requested = Path::new(program);
    if requested.components().count() > 1 || requested.is_absolute() {
        return resolve_windows_agent_candidate(requested.to_path_buf(), &pathext_env);
    }

    path_env
        .as_deref()
        .into_iter()
        .flat_map(std::env::split_paths)
        .find_map(|dir| resolve_windows_agent_candidate(dir.join(program), &pathext_env))
}

#[cfg(windows)]
fn resolve_windows_agent_candidate(
    base: PathBuf,
    pathext_env: &Option<OsString>,
) -> Option<PathBuf> {
    if base.extension().is_some() && base.exists() {
        return Some(base);
    }

    for ext in windows_agent_extensions(pathext_env) {
        let candidate = base.with_extension(ext.trim_start_matches('.'));
        if candidate.exists() {
            return Some(candidate);
        }
    }

    if base.exists() {
        return Some(base);
    }
    None
}

#[cfg(windows)]
fn windows_agent_extensions(pathext_env: &Option<OsString>) -> Vec<String> {
    let mut extensions = Vec::new();
    for ext in [".exe", ".com", ".bat", ".cmd", ".ps1"] {
        extensions.push(ext.to_string());
    }

    if let Some(pathext) = pathext_env {
        for ext in pathext.to_string_lossy().split(';') {
            let ext = ext.trim();
            if ext.is_empty() {
                continue;
            }
            let normalized = if ext.starts_with('.') {
                ext.to_ascii_lowercase()
            } else {
                format!(".{}", ext.to_ascii_lowercase())
            };
            if !extensions
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&normalized))
            {
                extensions.push(normalized);
            }
        }
    }

    extensions
}

#[cfg(windows)]
fn is_windows_batch_script(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
}

#[cfg(windows)]
fn is_windows_powershell_script(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ps1"))
}

#[cfg(windows)]
fn windows_comspec() -> OsString {
    std::env::var_os("ComSpec").unwrap_or_else(|| OsString::from("cmd.exe"))
}

/// Marker stored in `SessionInfo.source` to indicate this is not a real
/// provider-backed session but an ephemeral shell pane spawned via the new
/// session dialog. Reuses the existing AgentClient/daemon infrastructure
/// (PTY + vt100 + socket) — `agent_launch_spec` branches on this to spawn
/// `$SHELL` instead of the provider's resume command.
const SHELL_SESSION_SOURCE_MARKER: &str = "@cokacmux-shell";

fn is_shell_session_info(info: &SessionInfo) -> bool {
    info.source.as_os_str() == SHELL_SESSION_SOURCE_MARKER
}

fn shell_session_info_for_cwd(cwd: String) -> SessionInfo {
    let title = shell_pane_title(&cwd);
    SessionInfo {
        // Provider is arbitrary for shell panes — AgentKey uses it only as
        // a namespacing factor for socket/meta paths. Pick Claude.
        provider: Provider::Claude,
        session_id: format!("shell-{}", uuid::Uuid::now_v7()),
        cwd,
        source: PathBuf::from(SHELL_SESSION_SOURCE_MARKER),
        updated_at_epoch_s: chrono::Utc::now().timestamp().max(0) as u64,
        title: Some(title),
    }
}

/// Marker for a coding agent launched from scratch through the new session
/// dialog. These panes have no stored provider history yet, so launch uses the
/// provider's fresh-start command instead of a resume/session command.
const NEW_AGENT_SESSION_SOURCE_MARKER: &str = "@cokacmux-new-agent";

fn is_new_agent_session_info(info: &SessionInfo) -> bool {
    info.source.as_os_str() == NEW_AGENT_SESSION_SOURCE_MARKER
}

fn new_agent_session_info(provider: Provider, cwd: String) -> SessionInfo {
    SessionInfo {
        provider,
        session_id: format!("new-{}", uuid::Uuid::now_v7()),
        cwd: cwd.clone(),
        source: PathBuf::from(NEW_AGENT_SESSION_SOURCE_MARKER),
        updated_at_epoch_s: chrono::Utc::now().timestamp().max(0) as u64,
        title: Some(new_agent_pane_title(provider, &cwd)),
    }
}

fn new_agent_pane_title(provider: Provider, cwd: &str) -> String {
    if cwd.is_empty() {
        return format!("new {}", provider.as_str());
    }
    let basename = Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(cwd);
    format!("new {} @ {}", provider.as_str(), basename)
}

fn session_cwd_eq(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let Some(left_key) = windows_session_cwd_key(left) else {
        return false;
    };
    let Some(right_key) = windows_session_cwd_key(right) else {
        return false;
    };
    left_key == right_key
}

fn windows_session_cwd_key(cwd: &str) -> Option<String> {
    if cwd.is_empty() || !is_windows_cwd_syntax(cwd) {
        return None;
    }

    let mut path = strip_windows_namespace_prefix(cwd.replace('/', "\\"));
    if !is_windows_absolute_cwd(&path) {
        return None;
    }

    strip_redundant_windows_trailing_separators(&mut path);
    Some(path.to_lowercase())
}

fn strip_windows_namespace_prefix(path: String) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("\\\\?\\unc\\") {
        format!("\\\\{}", &path[8..])
    } else if lower.starts_with("\\\\?\\") || lower.starts_with("\\\\.\\") {
        path[4..].to_string()
    } else {
        path
    }
}

fn is_windows_cwd_syntax(path: &str) -> bool {
    is_windows_drive_absolute_cwd(path)
        || path.starts_with("\\\\")
        || path.starts_with("//?/")
        || path.starts_with("//./")
}

fn is_windows_absolute_cwd(path: &str) -> bool {
    is_windows_drive_absolute_cwd(path) || path.starts_with("\\\\")
}

fn is_windows_drive_absolute_cwd(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn strip_redundant_windows_trailing_separators(path: &mut String) {
    while path.len() > 3 && path.ends_with('\\') {
        path.pop();
    }
}

fn process_cwd_string() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .filter(|cwd| !cwd.is_empty())
}

fn normalize_launch_cwd(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("folder path is required.".into());
    }
    let path = expand_home_path(trimmed);
    if path.exists() {
        if !path.is_dir() {
            return Err(format!("not a folder: {}", truncate_width(trimmed, 48)));
        }
    } else {
        fs::create_dir_all(&path).map_err(|e| {
            format!(
                "create folder failed: {}: {}",
                truncate_width(trimmed, 48),
                e
            )
        })?;
    }
    if !path.is_dir() {
        return Err(format!("not a folder: {}", truncate_width(trimmed, 48)));
    }
    path.canonicalize()
        .map(|path| path.display().to_string())
        .map_err(|e| format!("resolve folder failed: {}", e))
}

fn validate_session_launch_cwd(info: &SessionInfo) -> Result<()> {
    match missing_session_launch_cwd(info)? {
        Some(path) => Err(anyhow::anyhow!(
            "launch folder does not exist: {}",
            path.display()
        )),
        None => Ok(()),
    }
}

fn validate_agent_launch_cwd(path: &Path) -> Result<()> {
    match missing_agent_launch_cwd(path)? {
        Some(path) => Err(anyhow::anyhow!(
            "launch folder does not exist: {}",
            path.display()
        )),
        None => Ok(()),
    }
}

fn missing_session_launch_cwd(info: &SessionInfo) -> Result<Option<PathBuf>> {
    if info.cwd.is_empty() {
        return Ok(None);
    }
    missing_agent_launch_cwd(Path::new(&info.cwd))
}

fn missing_agent_launch_cwd(path: &Path) -> Result<Option<PathBuf>> {
    match path.metadata() {
        Ok(meta) if meta.is_dir() => Ok(None),
        Ok(_) => Err(anyhow::anyhow!(
            "launch folder is not a directory: {}",
            path.display()
        )),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Some(path.to_path_buf())),
        Err(e) => Err(anyhow::anyhow!(
            "cannot access launch folder {}: {}",
            path.display(),
            e
        )),
    }
}

fn create_agent_launch_cwd(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .map_err(|e| anyhow::anyhow!("create {} failed: {}", path.display(), e))?;
    validate_agent_launch_cwd(path)
}

fn expand_home_path(raw: &str) -> PathBuf {
    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(raw)
}

/// Label for a shell pane in the agents sidebar — distinguishes multiple
/// concurrent shells by their cwd. Renders as `shell @ <basename>` when a
/// cwd is present, falling back to just `shell` otherwise.
fn shell_pane_title(cwd: &str) -> String {
    if cwd.is_empty() {
        return "shell".into();
    }
    let basename = std::path::Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(cwd);
    format!("shell @ {}", basename)
}

fn live_agent_status_label(info: &SessionInfo) -> String {
    if is_shell_session_info(info) {
        if info.cwd.is_empty() {
            "shell".into()
        } else {
            format!("shell at {}", truncate_width(&info.cwd, 40))
        }
    } else if is_new_agent_session_info(info) {
        if info.cwd.is_empty() {
            format!("new {} agent", info.provider.as_str())
        } else {
            format!(
                "new {} agent at {}",
                info.provider.as_str(),
                truncate_width(&info.cwd, 40)
            )
        }
    } else {
        format!(
            "{} agent {}",
            info.provider.as_str(),
            truncate_width(&info.session_id, 14)
        )
    }
}

fn shell_launch_spec(info: &SessionInfo) -> AgentLaunchSpec {
    let cwd = if info.cwd.is_empty() {
        None
    } else {
        Some(PathBuf::from(&info.cwd))
    };
    let program = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(windows) {
            "cmd.exe".to_string()
        } else {
            "/bin/bash".to_string()
        }
    });
    AgentLaunchSpec {
        program,
        args: Vec::new(),
        env: Vec::new(),
        cwd,
    }
}

fn resolve_agent_program_for_provider(
    provider: Provider,
    agent_programs: &AgentProgramSettings,
) -> Option<PathBuf> {
    let program = agent_programs.program_for(provider);
    resolve_agent_program_candidate(provider, &program)
}

#[cfg(unix)]
fn resolve_agent_program_candidate(_provider: Provider, program: &str) -> Option<PathBuf> {
    resolve_unix_agent_program(program)
}

#[cfg(windows)]
fn resolve_agent_program_candidate(provider: Provider, program: &str) -> Option<PathBuf> {
    let resolved = resolve_windows_agent_program_for_provider(provider, program)?;
    if provider == Provider::OpenCode {
        opencode_native_exe_for_windows_wrapper(&resolved).or(Some(resolved))
    } else {
        Some(resolved)
    }
}

#[cfg(not(any(unix, windows)))]
fn resolve_agent_program_candidate(_provider: Provider, program: &str) -> Option<PathBuf> {
    let path = PathBuf::from(program);
    path.is_file().then_some(path)
}

#[cfg(unix)]
fn resolve_unix_agent_program(program: &str) -> Option<PathBuf> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded = expand_configured_program_path(trimmed);
    let path = Path::new(&expanded);
    if path.components().count() > 1 || path.is_absolute() {
        return unix_runnable_file(path).then(|| path.to_path_buf());
    }

    resolve_unix_program_with_which(&expanded)
        .or_else(|| resolve_unix_program_with_login_shell(&expanded))
}

#[cfg(unix)]
fn resolve_unix_program_with_which(program: &str) -> Option<PathBuf> {
    let output = Command::new("which").arg(program).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty())
        .then_some(PathBuf::from(path))
        .filter(|path| unix_runnable_file(path))
}

#[cfg(unix)]
fn resolve_unix_program_with_login_shell(program: &str) -> Option<PathBuf> {
    let command = format!("which {}", shell_single_quote(program));
    let output = Command::new("bash").args(["-lc", &command]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty())
        .then_some(PathBuf::from(path))
        .filter(|path| unix_runnable_file(path))
}

#[cfg(unix)]
fn unix_runnable_file(path: &Path) -> bool {
    path.metadata()
        .map(|meta| meta.is_file() && (meta.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(unix)]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn resolve_windows_agent_program_for_provider(
    provider: Provider,
    program: &str,
) -> Option<PathBuf> {
    let requested = Path::new(program);
    let extensions = windows_agent_provider_extensions(provider);
    if requested.components().count() > 1 || requested.is_absolute() {
        return resolve_windows_agent_candidate_with_extensions(
            requested.to_path_buf(),
            &extensions,
        );
    }

    std::env::var_os("PATH")
        .as_deref()
        .into_iter()
        .flat_map(std::env::split_paths)
        .find_map(|dir| {
            resolve_windows_agent_candidate_with_extensions(dir.join(program), &extensions)
        })
}

#[cfg(windows)]
fn resolve_windows_agent_candidate_with_extensions(
    base: PathBuf,
    extensions: &[&str],
) -> Option<PathBuf> {
    if base.extension().is_some() {
        return windows_agent_path_is_runnable(&base).then_some(base);
    }

    for ext in extensions {
        let candidate = base.with_extension(ext.trim_start_matches('.'));
        if windows_agent_path_is_runnable(&candidate) {
            return Some(candidate);
        }
    }

    None
}

#[cfg(windows)]
fn windows_agent_provider_extensions(provider: Provider) -> &'static [&'static str] {
    match provider {
        Provider::Codex => &[".cmd", ".exe", ".bat", ".com"],
        Provider::Claude => &[".exe", ".cmd", ".bat", ".com"],
        Provider::OpenCode => &[".exe", ".cmd", ".bat", ".com"],
    }
}

#[cfg(windows)]
fn windows_agent_path_is_runnable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(extension.as_str(), "exe" | "cmd" | "bat" | "com" | "ps1")
}

#[cfg(windows)]
fn opencode_native_exe_for_windows_wrapper(path: &Path) -> Option<PathBuf> {
    if !is_windows_batch_script(path) {
        return None;
    }
    let native = path
        .parent()?
        .join("node_modules")
        .join("opencode-ai")
        .join("bin")
        .join("opencode.exe");
    native.is_file().then_some(native)
}

fn default_agent_program(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "codex",
        Provider::Claude => "claude",
        Provider::OpenCode => "opencode",
    }
}

fn new_agent_launch_spec_with_programs(
    info: &SessionInfo,
    launch_mode: AgentLaunchMode,
    agent_programs: &AgentProgramSettings,
) -> AgentLaunchSpec {
    let cwd = if info.cwd.is_empty() {
        None
    } else {
        Some(PathBuf::from(&info.cwd))
    };
    match info.provider {
        Provider::Codex => {
            let mut args = Vec::new();
            if launch_mode == AgentLaunchMode::SkipPermissions {
                args.push("--yolo".to_string());
            }
            if let Some(path) = &cwd {
                args.push("-C".to_string());
                args.push(path.display().to_string());
            }
            AgentLaunchSpec {
                program: agent_programs.program_for(Provider::Codex),
                args,
                env: Vec::new(),
                cwd,
            }
        }
        Provider::Claude => {
            let mut args = Vec::new();
            if launch_mode == AgentLaunchMode::SkipPermissions {
                args.push("--dangerously-skip-permissions".to_string());
            }
            AgentLaunchSpec {
                program: agent_programs.program_for(Provider::Claude),
                args,
                env: Vec::new(),
                cwd,
            }
        }
        Provider::OpenCode => {
            let mut args = Vec::new();
            let mut env = Vec::new();
            if launch_mode == AgentLaunchMode::SkipPermissions {
                env.push((
                    "OPENCODE_PERMISSION".to_string(),
                    r#"{"*":"allow"}"#.to_string(),
                ));
            }
            if let Some(path) = &cwd {
                args.push(path.display().to_string());
            }
            AgentLaunchSpec {
                program: agent_programs.program_for(Provider::OpenCode),
                args,
                env,
                cwd,
            }
        }
    }
}

fn agent_launch_spec_with_settings(
    info: &SessionInfo,
    launch_mode: AgentLaunchMode,
    settings: &Settings,
) -> AgentLaunchSpec {
    agent_launch_spec_with_programs(info, launch_mode, &settings.cokacmux.agent_programs)
}

fn agent_launch_spec_with_programs(
    info: &SessionInfo,
    launch_mode: AgentLaunchMode,
    agent_programs: &AgentProgramSettings,
) -> AgentLaunchSpec {
    if is_shell_session_info(info) {
        return shell_launch_spec(info);
    }
    if is_new_agent_session_info(info) {
        return new_agent_launch_spec_with_programs(info, launch_mode, agent_programs);
    }
    let cwd = if info.cwd.is_empty() {
        None
    } else {
        Some(PathBuf::from(&info.cwd))
    };
    match info.provider {
        Provider::Codex => {
            let mut args = Vec::new();
            if launch_mode == AgentLaunchMode::SkipPermissions {
                args.push("--yolo".to_string());
            }
            args.push("resume".to_string());
            if let Some(path) = &cwd {
                args.push("-C".to_string());
                args.push(path.display().to_string());
            }
            args.push(info.session_id.clone());
            AgentLaunchSpec {
                program: agent_programs.program_for(Provider::Codex),
                args,
                env: Vec::new(),
                cwd,
            }
        }
        Provider::Claude => {
            let mut args = Vec::new();
            if launch_mode == AgentLaunchMode::SkipPermissions {
                args.push("--dangerously-skip-permissions".to_string());
            }
            args.push("--resume".to_string());
            args.push(info.session_id.clone());
            AgentLaunchSpec {
                program: agent_programs.program_for(Provider::Claude),
                args,
                env: Vec::new(),
                cwd,
            }
        }
        Provider::OpenCode => {
            let mut args = Vec::new();
            let mut env = Vec::new();
            if launch_mode == AgentLaunchMode::SkipPermissions {
                env.push((
                    "OPENCODE_PERMISSION".to_string(),
                    r#"{"*":"allow"}"#.to_string(),
                ));
            }
            if let Some(path) = &cwd {
                args.push(path.display().to_string());
            }
            args.push("--session".to_string());
            args.push(info.session_id.clone());
            AgentLaunchSpec {
                program: agent_programs.program_for(Provider::OpenCode),
                args,
                env,
                cwd,
            }
        }
    }
}

fn handle_new_session_key(
    app: &mut App,
    key: KeyEvent,
    cols: u16,
    rows: u16,
    keybindings: &KeyBindings,
) -> bool {
    let mut start_action: Option<(NewSessionKind, String, Provider, AgentLaunchMode)> = None;
    let handled = if let InputMode::NewSession {
        selected,
        kind,
        cwd,
        cwd_cursor,
        provider,
        provider_options,
        launch_mode,
    } = &mut app.input_mode
    {
        *selected = clamp_new_session_field(*selected, *kind);
        if *kind == NewSessionKind::CodingAgent {
            if let Some(normalized) =
                normalize_agent_provider_selection(*provider, provider_options)
            {
                *provider = normalized;
            }
        }
        if keybindings.matches(KeyAction::NewSessionCancel, key) {
            app.input_mode = InputMode::Normal;
            app.status = "cancelled.".into();
            debug_log_key_event(key, "new_session_cancel");
        } else if keybindings.matches(KeyAction::NewSessionConfirm, key) {
            start_action = Some((*kind, cwd.clone(), *provider, *launch_mode));
            debug_log(
                "new_session_confirm",
                serde_json::json!({
                    "kind": kind.as_str(),
                    "cwd": cwd,
                    "provider": provider.as_str(),
                    "launch_mode": launch_mode.as_str(),
                }),
            );
        } else if keybindings.matches(KeyAction::NewSessionNext, key)
            && !new_session_cwd_text_key(*selected, key)
        {
            *selected = move_new_session_field(*selected, *kind, 1);
            debug_log(
                "new_session_move_field",
                serde_json::json!({
                    "selected": *selected,
                    "kind": kind.as_str(),
                }),
            );
        } else if keybindings.matches(KeyAction::NewSessionPrev, key)
            && !new_session_cwd_text_key(*selected, key)
        {
            *selected = move_new_session_field(*selected, *kind, -1);
            debug_log(
                "new_session_move_field",
                serde_json::json!({
                    "selected": *selected,
                    "kind": kind.as_str(),
                }),
            );
        } else if *selected == NEW_SESSION_FIELD_CWD {
            if keybindings.matches(KeyAction::NewSessionBackspace, key) {
                delete_before_cursor(cwd, cwd_cursor);
                debug_log_key_event(key, "new_session_cwd_backspace");
            } else if keybindings.matches(KeyAction::NewSessionDelete, key) {
                delete_at_cursor(cwd, cwd_cursor);
                debug_log_key_event(key, "new_session_cwd_delete");
            } else if keybindings.matches(KeyAction::NewSessionHome, key) {
                *cwd_cursor = 0;
                debug_log_key_event(key, "new_session_cwd_home");
            } else if keybindings.matches(KeyAction::NewSessionEnd, key) {
                *cwd_cursor = cwd.len();
                debug_log_key_event(key, "new_session_cwd_end");
            } else if key.code == KeyCode::Left {
                *cwd_cursor = prev_char_boundary(cwd, *cwd_cursor);
                debug_log_key_event(key, "new_session_cwd_left");
            } else if key.code == KeyCode::Right {
                *cwd_cursor = next_char_boundary(cwd, *cwd_cursor);
                debug_log_key_event(key, "new_session_cwd_right");
            } else if let KeyCode::Char(c) = key.code {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    insert_at_cursor(cwd, cwd_cursor, c);
                    debug_log_key_event(key, "new_session_cwd_insert");
                } else {
                    debug_log_key_event(key, "new_session_ignored");
                }
            } else {
                debug_log_key_event(key, "new_session_ignored");
            }
        } else if keybindings.matches(KeyAction::NewSessionChoiceNext, key) {
            match *selected {
                NEW_SESSION_FIELD_KIND => {
                    *kind = move_new_session_kind(*kind, 1);
                    *selected = clamp_new_session_field(*selected, *kind);
                    if *kind == NewSessionKind::CodingAgent {
                        if let Some(normalized) =
                            normalize_agent_provider_selection(*provider, provider_options)
                        {
                            *provider = normalized;
                        }
                    }
                }
                NEW_SESSION_FIELD_PROVIDER => {
                    *provider = move_provider_in_options(*provider, 1, provider_options)
                }
                NEW_SESSION_FIELD_PERMISSIONS => *launch_mode = move_launch_mode(*launch_mode, 1),
                _ => {}
            }
            debug_log(
                "new_session_choice",
                serde_json::json!({
                    "selected": *selected,
                    "kind": kind.as_str(),
                    "provider": provider.as_str(),
                    "launch_mode": launch_mode.as_str(),
                }),
            );
        } else if keybindings.matches(KeyAction::NewSessionChoicePrev, key) {
            match *selected {
                NEW_SESSION_FIELD_KIND => {
                    *kind = move_new_session_kind(*kind, -1);
                    *selected = clamp_new_session_field(*selected, *kind);
                    if *kind == NewSessionKind::CodingAgent {
                        if let Some(normalized) =
                            normalize_agent_provider_selection(*provider, provider_options)
                        {
                            *provider = normalized;
                        }
                    }
                }
                NEW_SESSION_FIELD_PROVIDER => {
                    *provider = move_provider_in_options(*provider, -1, provider_options)
                }
                NEW_SESSION_FIELD_PERMISSIONS => *launch_mode = move_launch_mode(*launch_mode, -1),
                _ => {}
            }
            debug_log(
                "new_session_choice",
                serde_json::json!({
                    "selected": *selected,
                    "kind": kind.as_str(),
                    "provider": provider.as_str(),
                    "launch_mode": launch_mode.as_str(),
                }),
            );
        } else {
            debug_log_key_event(key, "new_session_ignored");
        }
        true
    } else {
        false
    };

    if let Some((kind, cwd, provider, launch_mode)) = start_action {
        if app.start_new_session_from_modal(kind, cwd, provider, launch_mode, cols, rows) {
            app.input_mode = InputMode::Normal;
        }
    }
    handled
}

fn new_session_cwd_text_key(selected: usize, key: KeyEvent) -> bool {
    selected == NEW_SESSION_FIELD_CWD
        && matches!(key.code, KeyCode::Char(_))
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

fn handle_agent_key(app: &mut App, key: KeyEvent, total_width: u16) {
    app.maybe_reload_keybindings();
    let keybindings = app.keybindings.clone();
    if keybindings.matches(KeyAction::GlobalQuit, key) {
        debug_log_agent_key(key, "quit");
        app.should_quit = true;
        return;
    }
    if matches!(app.input_mode, InputMode::NewSession { .. }) {
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let agent_cols = agent_terminal_width(cols, app.agent_sidebar_config_width());
        handle_new_session_key(
            app,
            key,
            agent_cols.max(1),
            rows.saturating_sub(AGENT_STATUS_HEIGHT).max(1),
            &keybindings,
        );
        return;
    }
    if keybindings.matches(KeyAction::AgentToggleSessions, key) {
        debug_log_agent_key(key, "toggle_to_sessions");
        // Ctrl+] / Ctrl+[ is a pure window toggle — show sessions list but keep
        // the agent connection alive in the background.
        debug_log(
            "agent_toggle_to_sessions",
            serde_json::json!({
                "active_agent": app.active_agent.as_ref().map(|agent| session_info_debug_value(&agent.info)),
                "show_sessions_view_before": app.show_sessions_view,
                "agent_states_before": agent_state_entries_debug_value(&app.agent_states),
            }),
        );
        app.show_sessions_view = true;
        return;
    }
    if keybindings.matches(KeyAction::AgentKill, key) {
        debug_log_agent_key(key, "kill");
        app.kill_active_agent();
        return;
    }
    if keybindings.matches(KeyAction::AgentNewShell, key) {
        debug_log_agent_key(key, "new_session_from_agent");
        app.begin_new_session_from_active_agent();
        return;
    }
    if keybindings.matches(KeyAction::AgentToggleSidebar, key) {
        debug_log_agent_key(key, "toggle_agent_sidebar");
        app.toggle_agent_sidebar_visible();
        return;
    }
    if let Some(action) = agent_scrollback_key(&keybindings, key) {
        debug_log_agent_key(key, "scrollback");
        let (_, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let page_rows = rows
            .saturating_sub(AGENT_STATUS_HEIGHT)
            .saturating_sub(1)
            .max(1) as usize;
        app.scroll_active_agent_screen(action, page_rows);
        return;
    }
    if let Some(delta) = agent_pane_resize_key(&keybindings, key) {
        debug_log_agent_key(key, "resize");
        app.adjust_agent_sidebar_width(delta, total_width);
        return;
    }
    if let Some(delta) = agent_sidebar_select_key(&keybindings, key) {
        debug_log_agent_key(key, "select");
        app.switch_active_agent(delta, false);
        return;
    }
    if let Some(delta) = agent_switch_key(&keybindings, key) {
        debug_log_agent_key(key, "switch");
        app.switch_active_agent(delta, true);
        return;
    }
    if should_log_agent_key(key) {
        debug_log_agent_key(key, "forward");
    }
    app.send_key_to_active_agent(key);
}

#[cfg(test)]
fn is_screen_toggle_key(key: KeyEvent) -> bool {
    let keybindings = KeyBindings::default();
    keybindings.matches(KeyAction::SessionToggleAgent, key)
        || keybindings.matches(KeyAction::AgentToggleSessions, key)
}

#[cfg(test)]
fn is_session_toggle_key(key: KeyEvent) -> bool {
    KeyBindings::default().matches(KeyAction::SessionToggleAgent, key)
}

#[cfg(test)]
fn is_session_kill_key(key: KeyEvent) -> bool {
    KeyBindings::default().matches(KeyAction::SessionKillAgent, key)
}

#[cfg(test)]
fn is_agent_kill_key(key: KeyEvent) -> bool {
    KeyBindings::default().matches(KeyAction::AgentKill, key)
}

#[cfg(test)]
fn is_global_quit_key(key: KeyEvent) -> bool {
    KeyBindings::default().matches(KeyAction::GlobalQuit, key)
}

fn apply_scrollback_delta(current: usize, delta: i32) -> usize {
    if delta >= 0 {
        current.saturating_add(delta as usize)
    } else {
        current.saturating_sub((-delta) as usize)
    }
}

fn agent_scroll_action_moves_up(action: AgentScrollAction) -> bool {
    matches!(
        action,
        AgentScrollAction::Lines(delta) | AgentScrollAction::Pages(delta) if delta > 0
    ) || matches!(action, AgentScrollAction::Top)
}

#[cfg(test)]
fn is_agent_scrollback_key(key: KeyEvent) -> Option<AgentScrollAction> {
    agent_scrollback_key(&KeyBindings::default(), key)
}

fn agent_scrollback_key(bindings: &KeyBindings, key: KeyEvent) -> Option<AgentScrollAction> {
    if bindings.matches(KeyAction::AgentScrollLineUp, key) {
        Some(AgentScrollAction::Lines(1))
    } else if bindings.matches(KeyAction::AgentScrollLineDown, key) {
        Some(AgentScrollAction::Lines(-1))
    } else if bindings.matches(KeyAction::AgentScrollPageUp, key) {
        Some(AgentScrollAction::Pages(1))
    } else if bindings.matches(KeyAction::AgentScrollPageDown, key) {
        Some(AgentScrollAction::Pages(-1))
    } else if bindings.matches(KeyAction::AgentScrollTop, key) {
        Some(AgentScrollAction::Top)
    } else if bindings.matches(KeyAction::AgentScrollBottom, key) {
        Some(AgentScrollAction::Bottom)
    } else {
        None
    }
}

#[cfg(test)]
fn is_agent_sidebar_select_key(key: KeyEvent) -> Option<i32> {
    agent_sidebar_select_key(&KeyBindings::default(), key)
}

#[cfg(test)]
fn is_sessions_sidebar_select_key(key: KeyEvent) -> Option<i32> {
    sessions_sidebar_select_key(&KeyBindings::default(), key)
}

fn agent_sidebar_select_key(bindings: &KeyBindings, key: KeyEvent) -> Option<i32> {
    if bindings.matches(KeyAction::AgentSidebarPrev, key) {
        Some(-1)
    } else if bindings.matches(KeyAction::AgentSidebarNext, key) {
        Some(1)
    } else {
        None
    }
}

fn sessions_sidebar_select_key(bindings: &KeyBindings, key: KeyEvent) -> Option<i32> {
    if bindings.matches(KeyAction::SessionsSidebarPrev, key) {
        Some(-1)
    } else if bindings.matches(KeyAction::SessionsSidebarNext, key) {
        Some(1)
    } else {
        None
    }
}

#[cfg(test)]
fn is_sessions_pane_resize_key(key: KeyEvent) -> Option<i16> {
    sessions_pane_resize_key(&KeyBindings::default(), key)
}

#[cfg(test)]
fn is_agent_pane_resize_key(key: KeyEvent) -> Option<i16> {
    agent_pane_resize_key(&KeyBindings::default(), key)
}

fn sessions_pane_resize_key(bindings: &KeyBindings, key: KeyEvent) -> Option<i16> {
    if bindings.matches(KeyAction::SessionsPaneResizeLeft, key) {
        Some(-(PANE_RESIZE_STEP_COLUMNS as i16))
    } else if bindings.matches(KeyAction::SessionsPaneResizeRight, key) {
        Some(PANE_RESIZE_STEP_COLUMNS as i16)
    } else {
        None
    }
}

fn agent_pane_resize_key(bindings: &KeyBindings, key: KeyEvent) -> Option<i16> {
    if bindings.matches(KeyAction::AgentPaneResizeLeft, key) {
        Some(-(AGENT_SIDEBAR_RESIZE_STEP as i16))
    } else if bindings.matches(KeyAction::AgentPaneResizeRight, key) {
        Some(AGENT_SIDEBAR_RESIZE_STEP as i16)
    } else {
        None
    }
}

#[cfg(test)]
fn is_agent_switch_key(key: KeyEvent) -> Option<i32> {
    agent_switch_key(&KeyBindings::default(), key)
}

fn agent_switch_key(bindings: &KeyBindings, key: KeyEvent) -> Option<i32> {
    if bindings.matches(KeyAction::AgentSwitchPrev, key) {
        Some(-1)
    } else if bindings.matches(KeyAction::AgentSwitchNext, key) {
        Some(1)
    } else {
        None
    }
}

fn should_log_agent_key(key: KeyEvent) -> bool {
    key.modifiers.intersects(
        KeyModifiers::CONTROL
            | KeyModifiers::ALT
            | KeyModifiers::SHIFT
            | KeyModifiers::SUPER
            | KeyModifiers::HYPER
            | KeyModifiers::META,
    ) || matches!(
        key.code,
        KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::F(_)
    )
}

fn debug_log_agent_key(key: KeyEvent, action: &str) {
    debug_log(
        "agent_key",
        serde_json::json!({
            "action": action,
            "code": key_code_label(key),
            "modifiers": format!("{:?}", key.modifiers),
            "kind": format!("{:?}", key.kind),
            "state": format!("{:?}", key.state),
        }),
    );
}

fn debug_log_session_key(app: &App, key: KeyEvent, action: &str) {
    debug_log(
        "session_key",
        serde_json::json!({
            "action": action,
            "code": key_code_label(key),
            "modifiers": format!("{:?}", key.modifiers),
            "kind": format!("{:?}", key.kind),
            "state": format!("{:?}", key.state),
            "focus": format!("{:?}", app.focus),
            "input_mode": input_mode_label(&app.input_mode),
            "selected": app.current().map(|info| serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
            })),
            "visible": app.visible().len(),
            "status": &app.status,
        }),
    );
}

fn debug_log_key_event(key: KeyEvent, action: &str) {
    debug_log(
        "session_key",
        serde_json::json!({
            "action": action,
            "code": key_code_label(key),
            "modifiers": format!("{:?}", key.modifiers),
            "kind": format!("{:?}", key.kind),
            "state": format!("{:?}", key.state),
        }),
    );
}

fn debug_log_title_edit_cursor(source: &SessionInfo, draft: &str, cursor: usize, action: &str) {
    debug_log(
        "title_edit_cursor",
        serde_json::json!({
            "action": action,
            "provider": source.provider.as_str(),
            "session_id": &source.session_id,
            "draft_len": draft.len(),
            "cursor": cursor,
        }),
    );
}

fn input_mode_label(mode: &InputMode) -> &'static str {
    match mode {
        InputMode::Normal => "normal",
        InputMode::Filter { .. } => "filter",
        InputMode::Confirm { .. } => "confirm",
        InputMode::AgentLaunch { .. } => "agent_launch",
        InputMode::NewSession { .. } => "new_session",
        InputMode::CloneTarget { .. } => "clone_target",
        InputMode::TitleEdit { .. } => "title_edit",
    }
}

fn key_code_label(key: KeyEvent) -> String {
    match key.code {
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "page_up".to_string(),
        KeyCode::PageDown => "page_down".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "back_tab".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::F(n) => format!("f{}", n),
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            format!("control_char_u+{:04x}", c as u32)
        }
        KeyCode::Char(_) => "char".to_string(),
        KeyCode::Null => "null".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::CapsLock => "caps_lock".to_string(),
        KeyCode::ScrollLock => "scroll_lock".to_string(),
        KeyCode::NumLock => "num_lock".to_string(),
        KeyCode::PrintScreen => "print_screen".to_string(),
        KeyCode::Pause => "pause".to_string(),
        KeyCode::Menu => "menu".to_string(),
        KeyCode::KeypadBegin => "keypad_begin".to_string(),
        KeyCode::Media(media) => format!("media_{:?}", media),
        KeyCode::Modifier(modifier) => format!("modifier_{:?}", modifier),
    }
}

fn key_event_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let mut bytes = Vec::new();
    if alt {
        bytes.push(0x1b);
    }

    match key.code {
        KeyCode::Char(c) if ctrl => {
            let lower = c.to_ascii_lowercase();
            let code = match lower {
                'a'..='z' => (lower as u8) - b'a' + 1,
                '[' => 0x1b,
                '\\' => 0x1c,
                ']' => 0x1d,
                '^' => 0x1e,
                '_' => 0x1f,
                '?' => 0x7f,
                _ => return None,
            };
            bytes.push(code);
        }
        KeyCode::Char(c) => {
            let mut buf = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        KeyCode::Enter => bytes.push(b'\r'),
        KeyCode::Tab => bytes.push(b'\t'),
        KeyCode::Backspace => bytes.push(0x7f),
        KeyCode::Esc => bytes.push(0x1b),
        KeyCode::Up => bytes.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => bytes.extend_from_slice(b"\x1b[B"),
        KeyCode::Right => bytes.extend_from_slice(b"\x1b[C"),
        KeyCode::Left => bytes.extend_from_slice(b"\x1b[D"),
        KeyCode::Home => bytes.extend_from_slice(b"\x1b[H"),
        KeyCode::End => bytes.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => bytes.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => bytes.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => bytes.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => bytes.extend_from_slice(b"\x1b[2~"),
        KeyCode::F(n) => match n {
            1 => bytes.extend_from_slice(b"\x1bOP"),
            2 => bytes.extend_from_slice(b"\x1bOQ"),
            3 => bytes.extend_from_slice(b"\x1bOR"),
            4 => bytes.extend_from_slice(b"\x1bOS"),
            5 => bytes.extend_from_slice(b"\x1b[15~"),
            6 => bytes.extend_from_slice(b"\x1b[17~"),
            7 => bytes.extend_from_slice(b"\x1b[18~"),
            8 => bytes.extend_from_slice(b"\x1b[19~"),
            9 => bytes.extend_from_slice(b"\x1b[20~"),
            10 => bytes.extend_from_slice(b"\x1b[21~"),
            11 => bytes.extend_from_slice(b"\x1b[23~"),
            12 => bytes.extend_from_slice(b"\x1b[24~"),
            _ => return None,
        },
        _ => return None,
    }

    Some(bytes)
}

fn theme_base_style() -> Style {
    Style::default().fg(THEME_FG).bg(THEME_BG)
}

fn theme_alt_style() -> Style {
    Style::default().fg(THEME_FG).bg(THEME_BG_ALT)
}

fn theme_status_style() -> Style {
    Style::default().fg(THEME_FG).bg(THEME_STATUS_BG)
}

fn theme_selected_style() -> Style {
    Style::default()
        .fg(THEME_SELECTED_TEXT)
        .bg(THEME_SELECTED_BG)
        .add_modifier(Modifier::BOLD)
}

fn startup_spinner_frame(elapsed: Duration) -> &'static str {
    let index =
        ((elapsed.as_millis() / STARTUP_SPINNER_TICK_MS) as usize) % STARTUP_SPINNER_FRAMES.len();
    STARTUP_SPINNER_FRAMES[index]
}

fn render_agent_startup_spinner(
    buf: &mut Buffer,
    area: Rect,
    info: &SessionInfo,
    started_at: Instant,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let label = format!(
        "{} starting {} agent {}",
        startup_spinner_frame(started_at.elapsed()),
        info.provider.as_str(),
        truncate_width(&info.session_id, 18)
    );
    let label = truncate_width(&label, area.width as usize);
    let label_width = UnicodeWidthStr::width(label.as_str()).min(area.width as usize) as u16;
    let x = area
        .x
        .saturating_add(area.width.saturating_sub(label_width) / 2);
    let y = area.y.saturating_add(area.height / 2);
    let available = area.right().saturating_sub(x).min(area.width).max(1) as usize;
    buf.set_stringn(
        x,
        y,
        label,
        available,
        Style::default()
            .fg(THEME_SHORTCUT)
            .bg(AGENT_DEFAULT_BG)
            .add_modifier(Modifier::BOLD),
    );
}

fn ui_agent(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();
    let main_area = Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.saturating_sub(AGENT_STATUS_HEIGHT),
    );
    let status_area = Rect::new(
        area.x,
        area.y + main_area.height,
        area.width,
        AGENT_STATUS_HEIGHT.min(area.height),
    );

    let app_status = app.status.clone();
    let active_key = app
        .active_agent
        .as_ref()
        .map(|agent| AgentKey::new(&agent.info));
    let sidebar_width = app.agent_sidebar_width(main_area.width);
    let (sidebar_area, agent_area) = if sidebar_width > 0 {
        let output_width = main_area.width.saturating_sub(sidebar_width);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(sidebar_width),
                Constraint::Length(output_width),
            ])
            .split(main_area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, main_area)
    };

    let Some(agent) = app.active_agent.as_mut() else {
        f.render_widget(Clear, area);
        return;
    };

    let (agent_screen_rows, agent_screen_cols) = agent_screen_size_for_area(agent_area);
    agent.resize(agent_screen_cols, agent_screen_rows);
    // Output is already in the parser — the reader thread pushed it via
    // process_agent_event before this draw was scheduled. No socket read
    // happens on the render path.
    let screen_size = agent.parser.screen().size();
    if screen_size != (agent_screen_rows, agent_screen_cols) {
        debug_log(
            "agent_parser_size_corrected",
            serde_json::json!({
                "provider": agent.info.provider.as_str(),
                "session_id": &agent.info.session_id,
                "screen_rows": screen_size.0,
                "screen_cols": screen_size.1,
                "area_rows": agent_area.height,
                "area_cols": agent_area.width,
                "target_rows": agent_screen_rows,
                "target_cols": agent_screen_cols,
            }),
        );
        agent
            .parser
            .screen_mut()
            .set_size(agent_screen_rows, agent_screen_cols);
    }

    let (agent_cursor, scrollback_offset) = {
        let startup_spinner_started_at = agent.startup_spinner_started_at();
        let screen = agent.parser.screen();
        let history_scroll_offset = agent.history_scroll_offset;
        let scrollback_offset = if screen.scrollback() > 0 {
            screen.scrollback()
        } else {
            history_scroll_offset
        };
        let cursor = if startup_spinner_started_at.is_some()
            || screen.hide_cursor()
            || scrollback_offset > 0
        {
            None
        } else {
            let (row, col) = screen.cursor_position();
            if row < agent_area.height && col < agent_area.width {
                Some((agent_area.x + col, agent_area.y + row))
            } else {
                None
            }
        };
        let buf = f.buffer_mut();
        fill_area(buf, agent_area, Style::default().bg(AGENT_DEFAULT_BG));
        if screen.scrollback() == 0 && history_scroll_offset > 0 {
            let lines = agent
                .screen_history
                .visible_lines(history_scroll_offset, agent_area.height as usize);
            render_plain_agent_history(buf, agent_area, &lines);
        } else {
            render_vt100_screen(buf, screen, agent_area);
        }
        if let Some(started_at) = startup_spinner_started_at {
            render_agent_startup_spinner(buf, agent_area, &agent.info, started_at);
        }
        (cursor, scrollback_offset)
    };

    let buf = f.buffer_mut();
    fill_area(buf, status_area, theme_status_style());
    let status_hint = if app_status.is_empty() {
        String::new()
    } else {
        format!(" · {}", app_status)
    };
    let scrollback_hint = if scrollback_offset > 0 {
        format!(" · scrollback {} up", scrollback_offset)
    } else {
        String::new()
    };
    let agent_help = agent_help_text(&app.keybindings);
    let status = format!(
        " {} {}{}{} · {} · {}",
        agent.info.provider.as_str(),
        truncate_width(&agent.info.session_id, 18),
        status_hint,
        scrollback_hint,
        agent_help,
        &agent.command_line
    );
    buf.set_stringn(
        status_area.x,
        status_area.y,
        truncate_width(&status, status_area.width as usize),
        status_area.width as usize,
        theme_status_style(),
    );

    app.refresh_agent_runtime_states();
    if let (Some(sidebar_area), Some(active_key)) = (sidebar_area, active_key.as_ref()) {
        let candidates = app.live_agent_switch_candidates();
        f.render_widget(Clear, sidebar_area);
        draw_agent_sidebar(f, app, sidebar_area, &candidates, active_key);
    }

    let modal_drawn = draw_input_modal(f, area, app);
    if !modal_drawn {
        if let Some(cursor) = agent_cursor {
            f.set_cursor_position(cursor);
        }
    }
}

fn draw_input_modal(f: &mut ratatui::Frame, area: Rect, app: &App) -> bool {
    if let InputMode::Confirm { prompt, action } = &app.input_mode {
        draw_confirm_modal(f, area, prompt, action);
    } else if let InputMode::Filter { draft, cursor } = &app.input_mode {
        draw_filter_modal(
            f,
            area,
            draft,
            *cursor,
            app.search_pending.as_ref(),
            &app.keybindings,
        );
    } else if let InputMode::AgentLaunch { source, selected } = &app.input_mode {
        draw_agent_launch_modal(
            f,
            area,
            source,
            *selected,
            &app.settings.cokacmux.agent_programs,
            &app.keybindings,
        );
    } else if let InputMode::NewSession {
        selected,
        kind,
        cwd,
        cwd_cursor,
        provider,
        provider_options,
        launch_mode,
    } = &app.input_mode
    {
        draw_new_session_modal(
            f,
            area,
            *selected,
            *kind,
            cwd,
            *cwd_cursor,
            *provider,
            provider_options,
            *launch_mode,
            &app.settings.cokacmux.agent_programs,
            &app.keybindings,
        );
    } else if let InputMode::CloneTarget { source, selected } = &app.input_mode {
        draw_clone_target_modal(f, area, source, *selected, &app.keybindings);
    } else if let InputMode::TitleEdit {
        source,
        draft,
        cursor,
    } = &app.input_mode
    {
        draw_title_edit_modal(f, area, source, draft, *cursor, &app.keybindings);
    } else {
        return false;
    }
    true
}

fn agent_sidebar_width(total_width: u16, configured_width: u16) -> u16 {
    configured_width.min(total_width)
}

fn agent_terminal_width(total_width: u16, configured_width: u16) -> u16 {
    total_width.saturating_sub(agent_sidebar_width(total_width, configured_width))
}

fn agent_pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(AGENT_MIN_PTY_ROWS),
        cols: cols.max(AGENT_MIN_PTY_COLS),
        pixel_width: 0,
        pixel_height: 0,
    }
}

// vt100 0.16.2 has an unwrap() in Screen::text() that panics when a wide
// character is drawn at the last column (col+1 out of bounds). catch_unwind
// keeps the process alive, but the default panic hook still floods stderr —
// which destroys the TUI. Replace the hook so vt100 panics go to debug_log
// only; other panics keep the original behavior.
fn install_vt100_panic_filter() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let file = info.location().map(|l| l.file()).unwrap_or("");
        // Match any source file under the vt100 crate (screen.rs, grid.rs, ...)
        let is_vt100 = file.contains("vt100-");
        if is_vt100 {
            let payload = info.payload();
            let msg = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<unknown>");
            debug_log(
                "vt100_panic_suppressed",
                serde_json::json!({
                    "file": file,
                    "line": info.location().map(|l| l.line()).unwrap_or(0),
                    "col": info.location().map(|l| l.column()).unwrap_or(0),
                    "message": msg,
                }),
            );
            return;
        }
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<unknown>");
        debug_log(
            "panic_unhandled",
            serde_json::json!({
                "file": file,
                "line": info.location().map(|l| l.line()).unwrap_or(0),
                "col": info.location().map(|l| l.column()).unwrap_or(0),
                "message": msg,
            }),
        );
        default_hook(info);
    }));
}

fn safe_parser_process(parser: &mut vt100::Parser, bytes: &[u8]) -> bool {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let (cols, rows) = parser.screen().size();
    match catch_unwind(AssertUnwindSafe(|| parser.process(bytes))) {
        Ok(()) => true,
        Err(_) => {
            let dump_path = dump_vt100_panic_input(bytes, cols, rows);
            debug_log(
                "vt100_parser_panic",
                serde_json::json!({
                    "len": bytes.len(),
                    "cols": cols,
                    "rows": rows,
                    "sample": debug_bytes_sample(bytes, 128),
                    "dump": dump_path.as_ref().map(|p| p.display().to_string()),
                }),
            );
            false
        }
    }
}

/// Dump raw bytes that triggered a vt100 panic, plus the parser size, so we
/// can replay the exact input in a standalone reproducer.
fn dump_vt100_panic_input(bytes: &[u8], cols: u16, rows: u16) -> Option<PathBuf> {
    if !DEBUG_ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let dir = app_config_dir()?.join("debug").join("vt100_panics");
    fs::create_dir_all(&dir).ok()?;
    let seq = VT100_PANIC_DUMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let now_ms = current_epoch_ms();
    let filename = format!("panic_{:013}_{:04}_c{}r{}.bin", now_ms, seq, cols, rows);
    let path = dir.join(filename);
    if fs::write(&path, bytes).is_err() {
        return None;
    }
    Some(path)
}

static VT100_PANIC_DUMP_SEQ: AtomicUsize = AtomicUsize::new(0);

fn screen_activity_hash(screen: &vt100::Screen) -> u64 {
    let mut hasher = DefaultHasher::new();
    let (rows, cols) = screen.size();
    rows.hash(&mut hasher);
    cols.hash(&mut hasher);
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                "".hash(&mut hasher);
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            if cell.has_contents() {
                cell.contents().hash(&mut hasher);
            } else {
                "".hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

fn screen_has_visible_content(screen: &vt100::Screen) -> bool {
    let (rows, cols) = screen.size();
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() || !cell.has_contents() {
                continue;
            }
            if !cell.contents().trim().is_empty() {
                return true;
            }
        }
    }
    false
}

fn sessions_pane_width(
    total_width: u16,
    configured_width: Option<u16>,
    fallback_percent: u16,
) -> u16 {
    let configured_width = configured_width.unwrap_or_else(|| {
        ((u32::from(total_width) * u32::from(fallback_percent.min(100))) / 100) as u16
    });
    configured_width.min(total_width)
}

fn agent_screen_size_for_area(area: Rect) -> (u16, u16) {
    (
        area.height.max(AGENT_MIN_PTY_ROWS),
        area.width.max(AGENT_MIN_PTY_COLS),
    )
}

fn adjusted_visible_pane_width(current: u16, total_width: u16, delta: i16) -> (u16, bool) {
    let next = (current as i32 + delta as i32).clamp(0, total_width as i32) as u16;
    (next, next == current)
}

fn adjusted_sessions_pane_width(
    configured_width: Option<u16>,
    total_width: u16,
    fallback_percent: u16,
    delta: i16,
) -> (u16, bool) {
    adjusted_visible_pane_width(
        sessions_pane_width(total_width, configured_width, fallback_percent),
        total_width,
        delta,
    )
}

fn adjusted_agent_sidebar_width(
    configured_width: u16,
    total_width: u16,
    delta: i16,
) -> (u16, bool) {
    adjusted_visible_pane_width(
        agent_sidebar_width(total_width, configured_width),
        total_width,
        delta,
    )
}

fn draw_agent_sidebar(
    f: &mut ratatui::Frame,
    app: &App,
    area: Rect,
    candidates: &[SessionInfo],
    active_key: &AgentKey,
) {
    let title = format!(" agents [{}] ", candidates.len());
    fill_area(f.buffer_mut(), area, theme_base_style());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME_BORDER))
        .style(theme_base_style())
        .title(title);
    f.render_widget(block, area);

    let inner = pane_inner(area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let id_width = inner.width.saturating_sub(19) as usize;
    let items: Vec<ListItem> = candidates
        .iter()
        .map(|info| {
            let key = AgentKey::new(info);
            let state = if key == *active_key {
                app.agent_state_for(info).attached_mine()
            } else {
                app.agent_state_for(info)
            };
            // Shells: the agent-provider tag is meaningless (we picked one
            // arbitrarily to namespace the daemon socket). What matters is
            // the cwd the shell is operating in — render that as the label.
            let (badge_span, label) = if is_shell_session_info(info) {
                let cwd_label = if info.cwd.is_empty() {
                    "(no cwd)".to_string()
                } else {
                    truncate_width(&info.cwd, id_width)
                };
                (shell_badge_span(), cwd_label)
            } else {
                let title = info.title.as_deref().unwrap_or("");
                let label = if title.is_empty() {
                    truncate_width(&info.session_id, id_width)
                } else {
                    truncate_width(title, id_width)
                };
                (prov_span(info.provider), label)
            };
            ListItem::new(Line::from(vec![
                Span::styled(fit_width(state.label(), 8, Align::Left), state.style()),
                Span::raw(" "),
                badge_span,
                Span::raw(" "),
                Span::styled(label, Style::default().fg(THEME_FG_STRONG)),
            ]))
        })
        .collect();

    let mut state = ListState::default();
    state.select(
        candidates
            .iter()
            .position(|info| AgentKey::new(info) == *active_key),
    );
    let list = List::new(items)
        .style(theme_base_style())
        .highlight_style(theme_selected_style())
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, inner, &mut state);
}

fn render_vt100_screen(buf: &mut Buffer, screen: &vt100::Screen, area: Rect) {
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = screen.cell(row, col) else {
                buf[(area.x + col, area.y + row)]
                    .set_symbol(" ")
                    .set_style(Style::default().bg(AGENT_DEFAULT_BG));
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let symbol = if cell.has_contents() {
                cell.contents()
            } else {
                " "
            };
            buf[(area.x + col, area.y + row)]
                .set_symbol(symbol)
                .set_style(vt100_cell_style(cell));
        }
    }
}

fn render_plain_agent_history(buf: &mut Buffer, area: Rect, lines: &[String]) {
    let style = Style::default().fg(THEME_FG).bg(AGENT_DEFAULT_BG);
    for (row, line) in lines.iter().take(area.height as usize).enumerate() {
        let y = area.y + row as u16;
        buf.set_stringn(
            area.x,
            y,
            truncate_width(line, area.width as usize),
            area.width as usize,
            style,
        );
    }
}

fn vt100_cell_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default()
        .fg(vt100_color(cell.fgcolor()))
        .bg(vt100_bg_color(cell.bgcolor()));
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.dim() {
        style = style.add_modifier(Modifier::DIM);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

fn vt100_bg_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => AGENT_DEFAULT_BG,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(r, g, b) => Color::Indexed(rgb_to_ansi256(r, g, b)),
    }
}

fn vt100_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => THEME_FG,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(r, g, b) => Color::Indexed(rgb_to_ansi256(r, g, b)),
    }
}

fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let (r, g, b) = (u16::from(r), u16::from(g), u16::from(b));
    let mut best_index = 16u8;
    let mut best_distance = u32::MAX;
    for index in 16u8..=255 {
        let (cr, cg, cb) = ansi256_rgb(index);
        let distance = color_distance(r, g, b, cr, cg, cb);
        if distance < best_distance {
            best_distance = distance;
            best_index = index;
        }
    }
    best_index
}

fn ansi256_rgb(index: u8) -> (u16, u16, u16) {
    if index >= 232 {
        let shade = 8 + u16::from(index - 232) * 10;
        return (shade, shade, shade);
    }

    let cube_index = index - 16;
    let r = cube_index / 36;
    let g = (cube_index % 36) / 6;
    let b = cube_index % 6;
    (
        ansi256_cube_component(r),
        ansi256_cube_component(g),
        ansi256_cube_component(b),
    )
}

fn ansi256_cube_component(value: u8) -> u16 {
    if value == 0 {
        0
    } else {
        55 + u16::from(value) * 40
    }
}

fn color_distance(r: u16, g: u16, b: u16, cr: u16, cg: u16, cb: u16) -> u32 {
    let dr = i32::from(r) - i32::from(cr);
    let dg = i32::from(g) - i32::from(cg);
    let db = i32::from(b) - i32::from(cb);
    (dr * dr + dg * dg + db * db) as u32
}

fn handle_key(app: &mut App, key: KeyEvent, total_width: u16, agent_cols: u16, agent_rows: u16) {
    app.maybe_reload_keybindings();
    let keybindings = app.keybindings.clone();
    debug_log_session_key(app, key, "received");
    if keybindings.matches(KeyAction::GlobalQuit, key) {
        app.should_quit = true;
        debug_log_session_key(app, key, "global_quit");
        return;
    }

    // Confirm modal
    if let InputMode::Confirm { action, .. } = app.input_mode.clone() {
        if keybindings.matches(KeyAction::ConfirmYes, key) {
            app.input_mode = InputMode::Normal;
            debug_log_session_key(app, key, "confirm_yes");
            match action {
                PendingAction::Delete {
                    info,
                    removed_index,
                } => app.delete_session(info, removed_index),
                PendingAction::CreateMissingLaunchCwd {
                    info,
                    path,
                    cols,
                    rows,
                    launch_mode,
                } => app.create_missing_launch_cwd_and_attach(info, path, cols, rows, launch_mode),
            }
        } else if keybindings.matches(KeyAction::ConfirmNo, key) {
            app.input_mode = InputMode::Normal;
            app.status = "cancelled.".into();
            debug_log_session_key(app, key, "confirm_cancel");
        } else {
            debug_log_session_key(app, key, "confirm_ignored");
        }
        return;
    }
    // New session dialog
    if matches!(app.input_mode, InputMode::NewSession { .. }) {
        handle_new_session_key(app, key, agent_cols.max(1), agent_rows.max(1), &keybindings);
        return;
    }
    // Agent launch mode selection
    if let InputMode::AgentLaunch { source, selected } = &mut app.input_mode {
        let mut next_mode: Option<InputMode> = None;
        let mut attach_action: Option<(SessionInfo, AgentLaunchMode)> = None;
        if keybindings.matches(KeyAction::AgentLaunchCancel, key) {
            next_mode = Some(InputMode::Normal);
            app.status = "cancelled.".into();
            debug_log_session_key(app, key, "agent_launch_cancel");
        } else if keybindings.matches(KeyAction::AgentLaunchConfirm, key) {
            let launch_mode = agent_launch_mode_at(*selected);
            next_mode = Some(InputMode::Normal);
            attach_action = Some((source.clone(), launch_mode));
            debug_log(
                "agent_launch_confirm",
                serde_json::json!({
                    "provider": source.provider.as_str(),
                    "session_id": &source.session_id,
                    "launch_mode": launch_mode.as_str(),
                }),
            );
        } else if keybindings.matches(KeyAction::AgentLaunchNext, key) {
            *selected = move_agent_launch_mode_index(*selected, 1);
            debug_log(
                "agent_launch_move",
                serde_json::json!({
                    "selected": *selected,
                    "launch_mode": agent_launch_mode_at(*selected).as_str(),
                }),
            );
        } else if keybindings.matches(KeyAction::AgentLaunchPrev, key) {
            *selected = move_agent_launch_mode_index(*selected, -1);
            debug_log(
                "agent_launch_move",
                serde_json::json!({
                    "selected": *selected,
                    "launch_mode": agent_launch_mode_at(*selected).as_str(),
                }),
            );
        } else if keybindings.matches(KeyAction::AgentLaunchNormal, key) {
            *selected = 0;
            debug_log_key_event(key, "agent_launch_select_normal");
        } else if keybindings.matches(KeyAction::AgentLaunchSkipPermissions, key) {
            *selected = 1;
            debug_log_key_event(key, "agent_launch_select_skip_permissions");
        } else {
            debug_log_key_event(key, "agent_launch_ignored");
        }
        if let Some(mode) = next_mode {
            app.input_mode = mode;
        }
        if let Some((source, launch_mode)) = attach_action {
            app.attach_agent(source, agent_cols.max(1), agent_rows.max(1), launch_mode);
        }
        return;
    }
    // Clone target selection mode
    if let InputMode::CloneTarget { source, selected } = &mut app.input_mode {
        let mut next_mode: Option<InputMode> = None;
        let mut clone_action: Option<(SessionInfo, Provider)> = None;
        if keybindings.matches(KeyAction::CloneTargetCancel, key) {
            next_mode = Some(InputMode::Normal);
            app.status = "cancelled.".into();
            debug_log_session_key(app, key, "clone_target_cancel");
        } else if keybindings.matches(KeyAction::CloneTargetConfirm, key) {
            let target = clone_provider_at(*selected);
            next_mode = Some(InputMode::Normal);
            clone_action = Some((source.clone(), target));
            debug_log(
                "clone_target_confirm",
                serde_json::json!({
                    "source_provider": source.provider.as_str(),
                    "source_session_id": &source.session_id,
                    "target_provider": target.as_str(),
                }),
            );
        } else if keybindings.matches(KeyAction::CloneTargetNext, key) {
            *selected = move_clone_provider_index(*selected, 1);
            debug_log(
                "clone_target_move",
                serde_json::json!({
                    "selected": *selected,
                    "provider": clone_provider_at(*selected).as_str(),
                }),
            );
        } else if keybindings.matches(KeyAction::CloneTargetPrev, key) {
            *selected = move_clone_provider_index(*selected, -1);
            debug_log(
                "clone_target_move",
                serde_json::json!({
                    "selected": *selected,
                    "provider": clone_provider_at(*selected).as_str(),
                }),
            );
        } else {
            debug_log_key_event(key, "clone_target_ignored");
        }
        if let Some(mode) = next_mode {
            app.input_mode = mode;
        }
        if let Some((source, target)) = clone_action {
            app.clone_session_to(source, target);
        }
        return;
    }
    // Title edit mode
    if let InputMode::TitleEdit {
        source,
        draft,
        cursor,
    } = &mut app.input_mode
    {
        let mut next_mode: Option<InputMode> = None;
        let mut save_action: Option<(SessionInfo, String)> = None;
        if keybindings.matches(KeyAction::TitleCancel, key) {
            next_mode = Some(InputMode::Normal);
            app.status = "cancelled.".into();
            debug_log(
                "title_edit_cancel",
                serde_json::json!({
                    "provider": source.provider.as_str(),
                    "session_id": &source.session_id,
                    "draft_len": draft.len(),
                    "cursor": *cursor,
                }),
            );
        } else if keybindings.matches(KeyAction::TitleSave, key) {
            save_action = Some((source.clone(), draft.clone()));
            next_mode = Some(InputMode::Normal);
            debug_log(
                "title_edit_commit",
                serde_json::json!({
                    "provider": source.provider.as_str(),
                    "session_id": &source.session_id,
                    "draft_len": draft.len(),
                }),
            );
        } else if keybindings.matches(KeyAction::TitleMoveLeft, key) {
            *cursor = prev_char_boundary(draft, *cursor);
            debug_log_title_edit_cursor(source, draft, *cursor, "left");
        } else if keybindings.matches(KeyAction::TitleMoveRight, key) {
            *cursor = next_char_boundary(draft, *cursor);
            debug_log_title_edit_cursor(source, draft, *cursor, "right");
        } else if keybindings.matches(KeyAction::TitleHome, key) {
            *cursor = 0;
            debug_log_title_edit_cursor(source, draft, *cursor, "home");
        } else if keybindings.matches(KeyAction::TitleEnd, key) {
            *cursor = draft.len();
            debug_log_title_edit_cursor(source, draft, *cursor, "end");
        } else if keybindings.matches(KeyAction::TitleBackspace, key) {
            delete_before_cursor(draft, cursor);
            debug_log_title_edit_cursor(source, draft, *cursor, "backspace");
        } else if keybindings.matches(KeyAction::TitleDelete, key) {
            delete_at_cursor(draft, cursor);
            debug_log_title_edit_cursor(source, draft, *cursor, "delete");
        } else if let KeyCode::Char(c) = key.code {
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                insert_at_cursor(draft, cursor, c);
                debug_log_title_edit_cursor(source, draft, *cursor, "insert");
            } else {
                debug_log_key_event(key, "title_edit_ignored");
            }
        } else {
            debug_log_key_event(key, "title_edit_ignored");
        }
        if let Some(mode) = next_mode {
            app.input_mode = mode;
        }
        if let Some((source, title)) = save_action {
            app.set_session_title(source, title);
        }
        return;
    }
    // Filter input mode
    let filter_is_searching = app.search_pending.is_some();
    if let InputMode::Filter { draft, cursor } = &mut app.input_mode {
        let mut next_mode: Option<InputMode> = None;
        let mut apply_query: Option<String> = None;
        if keybindings.matches(KeyAction::FilterCancel, key) {
            next_mode = Some(InputMode::Normal);
            app.search_pending = None;
            app.status = "cancelled.".into();
            debug_log(
                "filter_cancel",
                serde_json::json!({
                    "draft_len": draft.chars().count(),
                    "searching": filter_is_searching,
                }),
            );
        } else if filter_is_searching {
            debug_log_key_event(key, "filter_searching_ignored");
        } else if keybindings.matches(KeyAction::FilterApply, key) {
            apply_query = Some(draft.clone());
        } else if keybindings.matches(KeyAction::FilterMoveLeft, key) {
            *cursor = prev_char_boundary(draft, *cursor);
            debug_log(
                "filter_cursor",
                serde_json::json!({
                    "action": "left",
                    "draft_len": draft.chars().count(),
                    "cursor": *cursor,
                }),
            );
        } else if keybindings.matches(KeyAction::FilterMoveRight, key) {
            *cursor = next_char_boundary(draft, *cursor);
            debug_log(
                "filter_cursor",
                serde_json::json!({
                    "action": "right",
                    "draft_len": draft.chars().count(),
                    "cursor": *cursor,
                }),
            );
        } else if keybindings.matches(KeyAction::FilterHome, key) {
            *cursor = 0;
            debug_log(
                "filter_cursor",
                serde_json::json!({
                    "action": "home",
                    "draft_len": draft.chars().count(),
                    "cursor": *cursor,
                }),
            );
        } else if keybindings.matches(KeyAction::FilterEnd, key) {
            *cursor = draft.len();
            debug_log(
                "filter_cursor",
                serde_json::json!({
                    "action": "end",
                    "draft_len": draft.chars().count(),
                    "cursor": *cursor,
                }),
            );
        } else if keybindings.matches(KeyAction::FilterBackspace, key) {
            delete_before_cursor(draft, cursor);
            debug_log(
                "filter_backspace",
                serde_json::json!({
                    "draft_len": draft.chars().count(),
                    "cursor": *cursor,
                }),
            );
        } else if keybindings.matches(KeyAction::FilterDelete, key) {
            delete_at_cursor(draft, cursor);
            debug_log(
                "filter_delete",
                serde_json::json!({
                    "draft_len": draft.chars().count(),
                    "cursor": *cursor,
                }),
            );
        } else if let KeyCode::Char(c) = key.code {
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                insert_at_cursor(draft, cursor, c);
                debug_log(
                    "filter_insert",
                    serde_json::json!({
                        "draft_len": draft.chars().count(),
                        "cursor": *cursor,
                    }),
                );
            } else {
                debug_log_key_event(key, "filter_ignored");
            }
        } else {
            debug_log_key_event(key, "filter_ignored");
        }
        if let Some(mode) = next_mode {
            app.input_mode = mode;
        }
        if let Some(query) = apply_query {
            app.start_text_search(query);
        }
        return;
    }
    // Normal mode
    if let Some(delta) = sessions_pane_resize_key(&keybindings, key) {
        app.adjust_sessions_pane_width(delta, total_width);
        return;
    }
    if let Some(delta) = sessions_sidebar_select_key(&keybindings, key) {
        app.focus = FocusPane::Sessions;
        app.move_selection(delta);
        return;
    }
    if keybindings.matches(KeyAction::SessionToggleAgent, key) {
        // Window toggle. If an agent is already active in this process,
        // flip to its view. Otherwise reconnect to a known live daemon
        // without starting a new one.
        debug_log(
            "sessions_toggle_agent_requested",
            serde_json::json!({
                "active_agent": app.active_agent.as_ref().map(|agent| session_info_debug_value(&agent.info)),
                "show_sessions_view_before": app.show_sessions_view,
                "agent_states_before": agent_state_entries_debug_value(&app.agent_states),
                "selected": app.current().map(session_info_debug_value),
            }),
        );
        app.toggle_screens(agent_cols.max(1), agent_rows.max(1));
        debug_log(
            "sessions_toggle_agent_done",
            serde_json::json!({
                "active_agent": app.active_agent.as_ref().map(|agent| session_info_debug_value(&agent.info)),
                "show_sessions_view_after": app.show_sessions_view,
                "agent_states_after": agent_state_entries_debug_value(&app.agent_states),
                "status": &app.status,
            }),
        );
        return;
    }
    if keybindings.matches(KeyAction::SessionKillAgent, key) {
        app.kill_selected_agent();
        return;
    }
    if keybindings.matches(KeyAction::SessionQuit, key) {
        app.should_quit = true;
        debug_log_session_key(app, key, "quit");
    } else if keybindings.matches(KeyAction::SessionForceQuit, key) {
        app.should_quit = true;
        debug_log_session_key(app, key, "ctrl_c_quit");
    } else if keybindings.matches(KeyAction::SessionNewShell, key) {
        app.begin_new_session_from_focused();
    } else if keybindings.matches(KeyAction::SessionToggleFocus, key) {
        app.toggle_focus();
    } else if keybindings.matches(KeyAction::SessionTogglePreview, key) {
        app.toggle_preview_mode();
    } else if keybindings.matches(KeyAction::SessionMoveNext, key) {
        if app.focus == FocusPane::Preview {
            app.scroll_preview(1);
        } else {
            app.move_selection(1);
        }
    } else if keybindings.matches(KeyAction::SessionMovePrev, key) {
        if app.focus == FocusPane::Preview {
            app.scroll_preview(-1);
        } else {
            app.move_selection(-1);
        }
    } else if keybindings.matches(KeyAction::SessionPageNext, key) {
        if app.focus == FocusPane::Preview {
            app.scroll_preview_page(1);
        } else {
            app.move_selection(10);
        }
    } else if keybindings.matches(KeyAction::SessionPagePrev, key) {
        if app.focus == FocusPane::Preview {
            app.scroll_preview_page(-1);
        } else {
            app.move_selection(-10);
        }
    } else if keybindings.matches(KeyAction::SessionTop, key) {
        if app.focus == FocusPane::Preview {
            app.preview_top();
        } else {
            app.select_first();
        }
    } else if keybindings.matches(KeyAction::SessionBottom, key) {
        if app.focus == FocusPane::Preview {
            app.preview_bottom();
        } else {
            app.select_last();
        }
    } else if keybindings.matches(KeyAction::SessionFilter, key) {
        app.begin_filter();
    } else if keybindings.matches(KeyAction::SessionToggleView, key) {
        app.toggle_session_view();
    } else if keybindings.matches(KeyAction::SessionRefresh, key) {
        app.refresh();
    } else if keybindings.matches(KeyAction::SessionDelete, key) {
        if let Some(info) = app.current().cloned() {
            let removed_index = app.list_state.selected();
            debug_log(
                "delete_confirm_open",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                }),
            );
            let yes_key = keybindings.help(KeyAction::ConfirmYes, "y");
            let no_key = keybindings.help(KeyAction::ConfirmNo, "N");
            app.input_mode = InputMode::Confirm {
                prompt: delete_confirm_prompt(&info, &yes_key, &no_key),
                action: PendingAction::Delete {
                    info,
                    removed_index,
                },
            };
        }
    } else if keybindings.matches(KeyAction::SessionClone, key) {
        if let Some(info) = app.current().cloned() {
            debug_log(
                "clone_direct_start",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                }),
            );
            let target = info.provider;
            app.clone_session_to(info, target);
        }
    } else if keybindings.matches(KeyAction::SessionEditTitle, key)
        && app.focus == FocusPane::Sessions
    {
        app.begin_title_edit();
    } else if keybindings.matches(KeyAction::SessionLaunchAgent, key) {
        app.begin_agent_launch(agent_cols.max(1), agent_rows.max(1));
    } else if keybindings.matches(KeyAction::SessionRefreshPreview, key) {
        // Force a re-render of the preview (and refresh cache).
        if let Some(info) = app.current().cloned() {
            let key = PreviewKey::new(&info, app.preview_mode);
            app.drop_cached_preview(&key);
            app.preview_requested = None;
            debug_log(
                "preview_force_refresh",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": info.session_id,
                    "mode": preview_mode_label(app.preview_mode),
                }),
            );
        } else {
            app.clear_preview_cache();
            debug_log("preview_force_refresh_all", serde_json::json!({}));
        }
    } else {
        debug_log_session_key(app, key, "normal_ignored");
    }
}

fn ui(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(area);

    let sessions_width = app.sessions_pane_width(outer[0].width);
    let preview_width = outer[0].width.saturating_sub(sessions_width);
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(sessions_width),
            Constraint::Length(preview_width),
        ])
        .split(outer[0]);

    f.render_widget(Clear, main[0]);
    draw_list(f, app, main[0]);
    f.render_widget(Clear, main[1]);
    draw_preview(f, app, main[1]);
    f.render_widget(Clear, outer[1]);
    draw_status(f, app, outer[1]);

    draw_input_modal(f, area, app);
}

fn draw_confirm_modal(f: &mut ratatui::Frame, area: Rect, prompt: &str, action: &PendingAction) {
    let modal_area = confirm_modal_area(prompt, area);
    fill_area(f.buffer_mut(), modal_area, theme_alt_style());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME_BORDER_ACTIVE))
        .style(theme_alt_style())
        .title(confirm_modal_title(action));
    let p = Paragraph::new(confirm_modal_lines(prompt))
        .block(block)
        .style(theme_alt_style())
        .wrap(Wrap { trim: false });
    f.render_widget(ratatui::widgets::Clear, modal_area);
    f.render_widget(p, modal_area);
}

fn delete_confirm_prompt(info: &SessionInfo, yes_key: &str, no_key: &str) -> String {
    let session_id = confirm_prompt_line(&info.session_id, 64);
    format!(
        "Delete {} session?\n{}\nThis removes stored history.\n{} delete   {} cancel",
        info.provider.as_str(),
        session_id,
        yes_key,
        no_key
    )
}

fn create_missing_cwd_confirm_prompt(
    info: &SessionInfo,
    path: &Path,
    yes_key: &str,
    no_key: &str,
) -> String {
    let path = confirm_prompt_line(&path.display().to_string(), 72);
    format!(
        "{} launch folder does not exist.\n{}\nCreate it and continue?\n{} create/start   {} cancel",
        info.provider.as_str(),
        path,
        yes_key,
        no_key
    )
}

fn confirm_prompt_line(value: &str, width: usize) -> String {
    truncate_width(sanitize_for_single_line(value).trim(), width)
}

fn confirm_modal_title(action: &PendingAction) -> &'static str {
    match action {
        PendingAction::Delete { .. } => "Delete session",
        PendingAction::CreateMissingLaunchCwd { .. } => "Create folder",
    }
}

fn confirm_modal_lines(prompt: &str) -> Vec<Line<'static>> {
    prompt
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let style = match index {
                0 => Style::default().fg(THEME_FG_STRONG).bg(THEME_BG_ALT),
                1 => Style::default().fg(THEME_ACCENT).bg(THEME_BG_ALT),
                2 => Style::default().fg(THEME_FG_DIM).bg(THEME_BG_ALT),
                _ => Style::default().fg(THEME_SHORTCUT).bg(THEME_BG_ALT),
            };
            Line::from(Span::styled(format!("  {}", line), style))
        })
        .collect()
}

fn confirm_modal_area(prompt: &str, area: Rect) -> Rect {
    let prompt_lines: Vec<&str> = prompt.lines().collect();
    let widest_line = prompt_lines
        .iter()
        .map(|line| UnicodeWidthStr::width(*line))
        .max()
        .unwrap_or(0);
    let target_width = widest_line.saturating_add(4).clamp(38, 72) as u16;
    let width = target_width.min(area.width);
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let prompt_line_count: usize = prompt_lines
        .iter()
        .map(|line| UnicodeWidthStr::width(*line).div_ceil(inner_width).max(1))
        .sum();
    let height = (prompt_line_count as u16)
        .saturating_add(2)
        .clamp(4, 8)
        .min(area.height);
    centered_rect_fixed(width, height, area)
}

fn draw_filter_modal(
    f: &mut ratatui::Frame,
    area: Rect,
    draft: &str,
    cursor: usize,
    pending: Option<&SearchPending>,
    keybindings: &KeyBindings,
) {
    let modal_area = centered_rect_fixed(72, 7, area);
    fill_area(f.buffer_mut(), modal_area, theme_alt_style());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME_BORDER_ACTIVE))
        .style(theme_alt_style())
        .title("Search sessions");
    let input_width = modal_area.width.saturating_sub(4) as usize;
    let input = title_edit_display(draft, cursor, input_width);
    let search_button = if let Some(pending) = pending {
        format!(
            " {} Searching ",
            startup_spinner_frame(pending.started_at.elapsed())
        )
    } else {
        format!(
            " {} Search ",
            keybindings.help(KeyAction::FilterApply, "Enter")
        )
    };
    let cancel_button = format!(
        " {} Cancel ",
        keybindings.help(KeyAction::FilterCancel, "Esc")
    );
    let help = if pending.is_some() {
        format!(
            "{} cancel",
            keybindings.help(KeyAction::FilterCancel, "Esc")
        )
    } else {
        filter_help_text(keybindings)
    };
    let lines = vec![
        Line::from(Span::styled(
            "Query",
            Style::default().fg(THEME_FG_STRONG).bg(THEME_BG_ALT),
        )),
        Line::from(Span::styled(
            input,
            Style::default().fg(THEME_FG_STRONG).bg(THEME_BG_ALT),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(search_button, theme_selected_style()),
            Span::raw("  "),
            Span::styled(
                cancel_button,
                Style::default().fg(THEME_FG_DIM).bg(THEME_BG_ALT),
            ),
        ]),
        Line::from(Span::styled(
            help,
            Style::default().fg(THEME_FG_DIM).bg(THEME_BG_ALT),
        )),
    ];
    let p = Paragraph::new(lines)
        .block(block)
        .style(theme_alt_style())
        .wrap(Wrap { trim: false });
    f.render_widget(ratatui::widgets::Clear, modal_area);
    f.render_widget(p, modal_area);
}

fn draw_title_edit_modal(
    f: &mut ratatui::Frame,
    area: Rect,
    source: &SessionInfo,
    draft: &str,
    cursor: usize,
    keybindings: &KeyBindings,
) {
    let modal_area = centered_rect_fixed(72, 5, area);
    fill_area(f.buffer_mut(), modal_area, theme_alt_style());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME_BORDER_ACTIVE))
        .style(theme_alt_style())
        .title("Edit title");
    let input_width = modal_area.width.saturating_sub(4) as usize;
    let input = title_edit_display(draft, cursor, input_width);
    let lines = vec![
        Line::from(format!(
            "{} session {}",
            source.provider.as_str(),
            truncate_width(&source.session_id, 24)
        )),
        Line::from(Span::styled(
            input,
            Style::default()
                .fg(THEME_SELECTED_TEXT)
                .bg(THEME_SELECTED_BG),
        )),
        Line::from(Span::styled(
            title_edit_help_text(keybindings),
            Style::default().fg(THEME_FG_DIM).bg(THEME_BG_ALT),
        )),
    ];
    let p = Paragraph::new(lines)
        .block(block)
        .style(theme_alt_style())
        .wrap(Wrap { trim: false });
    f.render_widget(ratatui::widgets::Clear, modal_area);
    f.render_widget(p, modal_area);
}

fn draw_agent_launch_modal(
    f: &mut ratatui::Frame,
    area: Rect,
    source: &SessionInfo,
    selected: usize,
    agent_programs: &AgentProgramSettings,
    keybindings: &KeyBindings,
) {
    let modal_area = centered_rect_fixed(78, 8, area);
    fill_area(f.buffer_mut(), modal_area, theme_alt_style());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME_BORDER_ACTIVE))
        .style(theme_alt_style())
        .title("Agent launch");
    let inner_width = modal_area.width.saturating_sub(2) as usize;
    let label_width = 25.min(inner_width.saturating_sub(12));
    let command_width = inner_width.saturating_sub(7 + label_width);
    let mut lines = vec![
        Line::from(format!(
            "Start/attach {} session {}",
            source.provider.as_str(),
            truncate_width(&source.session_id, 24)
        )),
        Line::from(""),
    ];

    for (idx, launch_mode) in AGENT_LAUNCH_MODE_OPTIONS.iter().copied().enumerate() {
        let is_selected = idx == selected;
        let style = if is_selected {
            theme_selected_style()
        } else {
            theme_alt_style()
        };
        let label = launch_mode.label();
        let command =
            agent_launch_spec_with_programs(source, launch_mode, agent_programs).command_line();
        lines.push(Line::from(Span::styled(
            format!(
                "{} {}. {}  {}",
                if is_selected { ">" } else { " " },
                idx + 1,
                fit_width(label, label_width, Align::Left),
                truncate_width(&command, command_width)
            ),
            style,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        agent_launch_help_text(keybindings),
        Style::default().fg(THEME_FG_DIM).bg(THEME_BG_ALT),
    )));

    let p = Paragraph::new(lines)
        .block(block)
        .style(theme_alt_style())
        .wrap(Wrap { trim: false });
    f.render_widget(ratatui::widgets::Clear, modal_area);
    f.render_widget(p, modal_area);
}

fn draw_new_session_modal(
    f: &mut ratatui::Frame,
    area: Rect,
    selected: usize,
    kind: NewSessionKind,
    cwd: &str,
    cwd_cursor: usize,
    provider: Provider,
    provider_options: &[Provider],
    launch_mode: AgentLaunchMode,
    agent_programs: &AgentProgramSettings,
    keybindings: &KeyBindings,
) {
    let height = if kind == NewSessionKind::CodingAgent {
        10
    } else {
        8
    };
    let modal_area = centered_rect_fixed(84, height, area);
    fill_area(f.buffer_mut(), modal_area, theme_alt_style());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME_BORDER_ACTIVE))
        .style(theme_alt_style())
        .title("New session");
    let inner_width = modal_area.width.saturating_sub(2) as usize;
    let label_width = 12usize.min(inner_width.saturating_sub(8));
    let value_width = inner_width.saturating_sub(label_width + 5);
    let selected = clamp_new_session_field(selected, kind);
    let mut lines = vec![Line::from(Span::styled(
        "Choose what to start",
        Style::default().fg(THEME_FG_STRONG).bg(THEME_BG_ALT),
    ))];

    lines.push(new_session_field_line(
        selected == NEW_SESSION_FIELD_KIND,
        "Type",
        kind.label().to_string(),
        label_width,
        value_width,
    ));
    let cwd_value = if selected == NEW_SESSION_FIELD_CWD {
        editable_value_display(cwd, cwd_cursor, value_width)
    } else {
        fit_width(cwd, value_width, Align::Left)
    };
    lines.push(new_session_field_line(
        selected == NEW_SESSION_FIELD_CWD,
        "Folder",
        cwd_value,
        label_width,
        value_width,
    ));

    if kind == NewSessionKind::CodingAgent {
        let provider_label = if provider_options.is_empty() {
            "none installed".to_string()
        } else {
            provider.as_str().to_string()
        };
        lines.push(new_session_field_line(
            selected == NEW_SESSION_FIELD_PROVIDER,
            "Agent",
            provider_label,
            label_width,
            value_width,
        ));
        lines.push(new_session_field_line(
            selected == NEW_SESSION_FIELD_PERMISSIONS,
            "Permissions",
            launch_mode.label().to_string(),
            label_width,
            value_width,
        ));
    }

    lines.push(Line::from(Span::styled(
        format!(
            "  {}",
            truncate_width(
                &new_session_preview_command(
                    kind,
                    cwd,
                    provider,
                    provider_options,
                    launch_mode,
                    agent_programs,
                ),
                inner_width.saturating_sub(2)
            )
        ),
        Style::default().fg(THEME_FG_DIM).bg(THEME_BG_ALT),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        new_session_help_text(keybindings),
        Style::default().fg(THEME_FG_DIM).bg(THEME_BG_ALT),
    )));

    let p = Paragraph::new(lines)
        .block(block)
        .style(theme_alt_style())
        .wrap(Wrap { trim: false });
    f.render_widget(ratatui::widgets::Clear, modal_area);
    f.render_widget(p, modal_area);
}

fn new_session_field_line(
    selected: bool,
    label: &str,
    value: String,
    label_width: usize,
    value_width: usize,
) -> Line<'static> {
    let style = if selected {
        theme_selected_style()
    } else {
        theme_alt_style()
    };
    Line::from(Span::styled(
        format!(
            "{} {} {}",
            if selected { ">" } else { " " },
            fit_width(label, label_width, Align::Left),
            fit_width(&value, value_width, Align::Left)
        ),
        style,
    ))
}

fn editable_value_display(value: &str, cursor: usize, width: usize) -> String {
    let display = sanitize_for_single_line(value);
    title_edit_display(&display, cursor.min(display.len()), width)
}

fn new_session_preview_command(
    kind: NewSessionKind,
    cwd: &str,
    provider: Provider,
    provider_options: &[Provider],
    launch_mode: AgentLaunchMode,
    agent_programs: &AgentProgramSettings,
) -> String {
    match kind {
        NewSessionKind::Terminal => {
            let info = SessionInfo {
                provider: Provider::Claude,
                session_id: "preview".into(),
                cwd: cwd.to_string(),
                source: PathBuf::from(SHELL_SESSION_SOURCE_MARKER),
                updated_at_epoch_s: 0,
                title: None,
            };
            shell_launch_spec(&info).command_line()
        }
        NewSessionKind::CodingAgent => {
            if provider_options.is_empty() {
                return "no installed coding agents".to_string();
            }
            if !provider_options.contains(&provider) {
                return format!("{} agent is not installed", provider.as_str());
            }
            let info = SessionInfo {
                provider,
                session_id: "preview".into(),
                cwd: cwd.to_string(),
                source: PathBuf::from(NEW_AGENT_SESSION_SOURCE_MARKER),
                updated_at_epoch_s: 0,
                title: None,
            };
            new_agent_launch_spec_with_programs(&info, launch_mode, agent_programs).command_line()
        }
    }
}

fn draw_clone_target_modal(
    f: &mut ratatui::Frame,
    area: Rect,
    source: &SessionInfo,
    selected: usize,
    keybindings: &KeyBindings,
) {
    let modal_area = centered_rect(62, 30, area);
    fill_area(f.buffer_mut(), modal_area, theme_alt_style());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME_BORDER_ACTIVE))
        .style(theme_alt_style())
        .title("Clone target");
    let mut lines = vec![
        Line::from(format!(
            "Clone {} session {}",
            source.provider.as_str(),
            truncate_width(&source.session_id, 24)
        )),
        Line::from(""),
    ];

    for (idx, provider) in CLONE_PROVIDER_OPTIONS.iter().copied().enumerate() {
        let is_selected = idx == selected;
        let is_same = provider == source.provider;
        let style = if is_selected {
            theme_selected_style()
        } else {
            theme_alt_style()
        };
        let label = if is_same {
            format!("{} (same provider)", provider.as_str())
        } else {
            provider.as_str().to_string()
        };
        lines.push(Line::from(Span::styled(
            format!("{} {}", if is_selected { "▶" } else { " " }, label),
            style,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        clone_target_help_text(keybindings),
        Style::default().fg(THEME_FG_DIM).bg(THEME_BG_ALT),
    )));

    let p = Paragraph::new(lines)
        .block(block)
        .style(theme_alt_style())
        .wrap(Wrap { trim: false });
    f.render_widget(ratatui::widgets::Clear, modal_area);
    f.render_widget(p, modal_area);
}

fn title_edit_help_text(keybindings: &KeyBindings) -> String {
    format!(
        "{} move · {} · {} · {} save · {} cancel",
        keybindings.help_pair(
            KeyAction::TitleMoveLeft,
            KeyAction::TitleMoveRight,
            "Left",
            "Right",
        ),
        keybindings.help_pair(KeyAction::TitleHome, KeyAction::TitleEnd, "Home", "End"),
        keybindings.help_pair(
            KeyAction::TitleDelete,
            KeyAction::TitleBackspace,
            "Del",
            "Bksp",
        ),
        keybindings.help(KeyAction::TitleSave, "Enter"),
        keybindings.help(KeyAction::TitleCancel, "Esc"),
    )
}

fn filter_help_text(keybindings: &KeyBindings) -> String {
    format!(
        "{} move · {} · {} · {} search · {} cancel",
        keybindings.help_pair(
            KeyAction::FilterMoveLeft,
            KeyAction::FilterMoveRight,
            "Left",
            "Right",
        ),
        keybindings.help_pair(KeyAction::FilterHome, KeyAction::FilterEnd, "Home", "End"),
        keybindings.help_pair(
            KeyAction::FilterDelete,
            KeyAction::FilterBackspace,
            "Del",
            "Bksp",
        ),
        keybindings.help(KeyAction::FilterApply, "Enter"),
        keybindings.help(KeyAction::FilterCancel, "Esc"),
    )
}

fn agent_launch_help_text(keybindings: &KeyBindings) -> String {
    format!(
        "{} start/attach · {} choose · {}/{} select · {} cancel",
        keybindings.help(KeyAction::AgentLaunchConfirm, "Enter"),
        keybindings.help_pair(
            KeyAction::AgentLaunchPrev,
            KeyAction::AgentLaunchNext,
            "Up",
            "Down",
        ),
        keybindings.help(KeyAction::AgentLaunchNormal, "1"),
        keybindings.help(KeyAction::AgentLaunchSkipPermissions, "2"),
        keybindings.help(KeyAction::AgentLaunchCancel, "Esc"),
    )
}

fn new_session_help_text(keybindings: &KeyBindings) -> String {
    format!(
        "{} start · {} field · {} change · {} cancel",
        keybindings.help(KeyAction::NewSessionConfirm, "Enter"),
        keybindings.help_pair(
            KeyAction::NewSessionPrev,
            KeyAction::NewSessionNext,
            "Up",
            "Down",
        ),
        keybindings.help_pair(
            KeyAction::NewSessionChoicePrev,
            KeyAction::NewSessionChoiceNext,
            "Left",
            "Right",
        ),
        keybindings.help(KeyAction::NewSessionCancel, "Esc"),
    )
}

fn clone_target_help_text(keybindings: &KeyBindings) -> String {
    format!(
        "{} clone · {} choose · {} cancel",
        keybindings.help(KeyAction::CloneTargetConfirm, "Enter"),
        keybindings.help_pair(
            KeyAction::CloneTargetPrev,
            KeyAction::CloneTargetNext,
            "Up",
            "Down",
        ),
        keybindings.help(KeyAction::CloneTargetCancel, "Esc"),
    )
}

fn draw_list(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let rows = app.visible_rows();
    let inner = pane_inner(area);
    fill_area(f.buffer_mut(), area, theme_base_style());
    let cols = list_columns(inner.width);
    debug_assert!(inner.width < 20 || cols.row_width() == inner.width as usize);
    let selected = app.list_state.selected();
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            let s = row.info;
            let agent_state = app.agent_state_for(s);
            let prov = prov_span(s.provider);
            let title = s.title.as_deref().unwrap_or("");
            let age = age_label(s.updated_at_epoch_s);
            let marker = if selected == Some(idx) { "▶ " } else { "  " };
            let session_label = if app.session_view == SessionViewMode::Tree {
                tree_session_label(row.depth, &s.session_id)
            } else {
                s.session_id.clone()
            };
            let mut spans = vec![
                Span::raw(marker),
                Span::styled(
                    fit_width(agent_state.label(), cols.state, Align::Left),
                    agent_state.style(),
                ),
                Span::raw(" "),
                prov,
                Span::raw(" "),
                Span::styled(
                    fit_width(&session_label, cols.id, Align::Left),
                    Style::default().fg(THEME_FG_DIM),
                ),
            ];
            if cols.age > 0 {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    fit_width(&age, cols.age, Align::Right),
                    Style::default().fg(THEME_FG_DIM),
                ));
            }
            if cols.title > 0 {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    fit_width(title, cols.title, Align::Left),
                    Style::default().fg(THEME_FG_STRONG),
                ));
            }
            if cols.cwd > 0 {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    fit_width(&s.cwd, cols.cwd, Align::Left),
                    Style::default().fg(THEME_FG_DIM),
                ));
            }
            let line = Line::from(spans);
            ListItem::new(line)
        })
        .collect();

    let title = format!(
        " sessions [{}] {} {} {} ",
        rows.len(),
        app.provider_filter.label(),
        app.session_view.label(),
        if app.text_filter.is_empty() {
            String::new()
        } else {
            format!("search={}", app.text_filter)
        },
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(focus_style(app.focus == FocusPane::Sessions))
        .style(theme_base_style())
        .title(title);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let header = fit_width(&list_header(&cols), inner.width as usize, Align::Left);
    f.buffer_mut().set_stringn(
        inner.x,
        inner.y,
        header,
        inner.width as usize,
        Style::default()
            .fg(THEME_FG_DIM)
            .bg(THEME_BG)
            .add_modifier(Modifier::BOLD),
    );

    if inner.height <= 1 {
        return;
    }

    let list_area = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );
    let list = List::new(items)
        .style(theme_base_style())
        .highlight_style(theme_selected_style());
    f.render_stateful_widget(list, list_area, &mut app.list_state);
}

fn draw_preview(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let inner = pane_inner(area);
    fill_area(f.buffer_mut(), area, theme_base_style());
    app.preview_page_height = inner.height.max(1);

    let current = app.current().cloned();
    let mut lines = Vec::new();
    let mut max_scroll = 0u16;
    let title = if let Some(info) = current {
        let key = PreviewKey::new(&info, app.preview_mode);
        app.request_preview(info.clone(), key.clone(), inner.width as usize);
        if let Some(line_count) = app.ensure_wrapped_preview(&key, inner.width as usize) {
            max_scroll = line_count
                .saturating_sub(inner.height as usize)
                .min(u16::MAX as usize) as u16;
            if app.preview_scroll > max_scroll {
                app.preview_scroll = max_scroll;
            }
            lines =
                app.preview_visible_lines(&key, app.preview_scroll as usize, inner.height as usize);
        } else {
            app.preview_scroll = 0;
            lines.push(format!(
                "loading {} session {}",
                info.provider.as_str(),
                truncate_width(&info.session_id, 14)
            ));
        }
        let loading = app
            .preview_requested
            .as_ref()
            .map(|(requested_key, _)| requested_key == &key)
            .unwrap_or(false);
        format!(
            " preview {} {} {} {}/{}{} ",
            info.provider.as_str(),
            truncate_width(&info.session_id, 14),
            preview_mode_short(app.preview_mode),
            app.preview_scroll,
            max_scroll,
            if loading { " *" } else { "" }
        )
    } else {
        app.preview_scroll = 0;
        " preview ".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(focus_style(app.focus == FocusPane::Preview))
        .style(theme_base_style())
        .title(title);
    f.render_widget(block, area);

    let buf = f.buffer_mut();
    fill_area(buf, inner, theme_base_style());
    for (row, line) in lines.iter().take(inner.height as usize).enumerate() {
        buf.set_stringn(
            inner.x,
            inner.y + row as u16,
            line,
            inner.width as usize,
            theme_base_style(),
        );
    }
}

fn pane_inner(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

fn fill_area(buf: &mut Buffer, area: Rect, style: Style) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].reset();
            buf[(x, y)].set_style(style);
        }
    }
}

fn draw_status(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    fill_area(f.buffer_mut(), area, theme_status_style());
    let width = area.width as usize;
    let help = truncate_width(&help_text(app.focus, width, &app.keybindings), width);
    let mode = format!(
        "{} focus · {} preview",
        match app.focus {
            FocusPane::Sessions => "sessions",
            FocusPane::Preview => "preview",
        },
        preview_mode_label(app.preview_mode)
    );
    let mode = truncate_width(&mode, width);
    let mode_width = UnicodeWidthStr::width(mode.as_str());
    let status_width = width.saturating_sub(mode_width + 2);
    let status = truncate_width(&app.status, status_width);
    let mut status_spans = vec![Span::styled(
        mode,
        Style::default().fg(THEME_SHORTCUT).bg(THEME_STATUS_BG),
    )];
    if !status.is_empty() {
        status_spans.push(Span::raw("  "));
        status_spans.push(Span::styled(
            status,
            Style::default().fg(THEME_ACCENT).bg(THEME_STATUS_BG),
        ));
    }
    let lines = vec![
        Line::from(status_spans),
        Line::from(Span::styled(
            help,
            Style::default().fg(THEME_FG_DIM).bg(THEME_STATUS_BG),
        )),
    ];
    f.render_widget(Paragraph::new(lines).style(theme_status_style()), area);
}

fn help_text(focus: FocusPane, width: usize, keybindings: &KeyBindings) -> String {
    match focus {
        FocusPane::Sessions if width >= 92 => format!(
            "{} preview · {} select · {} new · {} tree · {} title · {} delete · {} launch · {}/{} quit",
            keybindings.help(KeyAction::SessionToggleFocus, "Tab/Esc"),
            keybindings.help_pair(
                KeyAction::SessionMovePrev,
                KeyAction::SessionMoveNext,
                "Up",
                "Down",
            ),
            keybindings.help(KeyAction::SessionNewShell, "Ctrl+N"),
            keybindings.help(KeyAction::SessionToggleView, "v"),
            keybindings.help(KeyAction::SessionEditTitle, "t"),
            keybindings.help(KeyAction::SessionDelete, "Delete/d"),
            keybindings.help(KeyAction::SessionLaunchAgent, "e"),
            keybindings.help(KeyAction::SessionQuit, "q"),
            keybindings.help(KeyAction::GlobalQuit, "Ctrl+Q"),
        ),
        FocusPane::Sessions if width >= 68 => format!(
            "{} preview · {} select · {} new · {} title · {} launch · {}/{} quit",
            keybindings.help(KeyAction::SessionToggleFocus, "Tab/Esc"),
            keybindings.help_pair(
                KeyAction::SessionMovePrev,
                KeyAction::SessionMoveNext,
                "Up",
                "Down",
            ),
            keybindings.help(KeyAction::SessionNewShell, "Ctrl+N"),
            keybindings.help(KeyAction::SessionEditTitle, "t"),
            keybindings.help(KeyAction::SessionLaunchAgent, "e"),
            keybindings.help(KeyAction::SessionQuit, "q"),
            keybindings.help(KeyAction::GlobalQuit, "Ctrl+Q"),
        ),
        FocusPane::Sessions => format!(
            "{} preview · {} select · {} title · {} launch · {}/{} quit",
            keybindings.help(KeyAction::SessionToggleFocus, "Tab/Esc"),
            keybindings.help_pair(
                KeyAction::SessionMovePrev,
                KeyAction::SessionMoveNext,
                "Up",
                "Down",
            ),
            keybindings.help(KeyAction::SessionEditTitle, "t"),
            keybindings.help(KeyAction::SessionLaunchAgent, "e"),
            keybindings.help(KeyAction::SessionQuit, "q"),
            keybindings.help(KeyAction::GlobalQuit, "Ctrl+Q"),
        ),
        FocusPane::Preview if width >= 92 => format!(
            "{} sessions · {} scroll · {} select · {} resize · {} page · {} top/bottom · {} summary/full · {}/{} quit",
            keybindings.help(KeyAction::SessionToggleFocus, "Tab/Esc"),
            keybindings.help_pair(
                KeyAction::SessionMovePrev,
                KeyAction::SessionMoveNext,
                "Up",
                "Down",
            ),
            keybindings.help_pair(
                KeyAction::SessionsSidebarPrev,
                KeyAction::SessionsSidebarNext,
                "Alt+Up",
                "Alt+Down",
            ),
            keybindings.help_pair(
                KeyAction::SessionsPaneResizeLeft,
                KeyAction::SessionsPaneResizeRight,
                "Alt+Left",
                "Alt+Right",
            ),
            keybindings.help_pair(
                KeyAction::SessionPagePrev,
                KeyAction::SessionPageNext,
                "PgUp",
                "PgDn",
            ),
            keybindings.help_pair(
                KeyAction::SessionTop,
                KeyAction::SessionBottom,
                "Home",
                "End",
            ),
            keybindings.help(KeyAction::SessionTogglePreview, "Enter"),
            keybindings.help(KeyAction::SessionQuit, "q"),
            keybindings.help(KeyAction::GlobalQuit, "Ctrl+Q"),
        ),
        FocusPane::Preview if width >= 68 => format!(
            "{} sessions · {} scroll · {} resize · {} page · {}/{} quit",
            keybindings.help(KeyAction::SessionToggleFocus, "Tab/Esc"),
            keybindings.help_pair(
                KeyAction::SessionMovePrev,
                KeyAction::SessionMoveNext,
                "Up",
                "Down",
            ),
            keybindings.help_pair(
                KeyAction::SessionsPaneResizeLeft,
                KeyAction::SessionsPaneResizeRight,
                "Alt+Left",
                "Alt+Right",
            ),
            keybindings.help_pair(
                KeyAction::SessionPagePrev,
                KeyAction::SessionPageNext,
                "PgUp",
                "PgDn",
            ),
            keybindings.help(KeyAction::SessionQuit, "q"),
            keybindings.help(KeyAction::GlobalQuit, "Ctrl+Q"),
        ),
        FocusPane::Preview => format!(
            "{} sessions · {} scroll · {} resize · {}/{} quit",
            keybindings.help(KeyAction::SessionToggleFocus, "Tab/Esc"),
            keybindings.help_pair(
                KeyAction::SessionMovePrev,
                KeyAction::SessionMoveNext,
                "Up",
                "Down",
            ),
            keybindings.help_pair(
                KeyAction::SessionsPaneResizeLeft,
                KeyAction::SessionsPaneResizeRight,
                "Alt+Left",
                "Alt+Right",
            ),
            keybindings.help(KeyAction::SessionQuit, "q"),
            keybindings.help(KeyAction::GlobalQuit, "Ctrl+Q"),
        ),
    }
}

fn agent_help_text(keybindings: &KeyBindings) -> String {
    format!(
        "{} sessions · {} new · {} kill · {} quit · {} scroll · {} switch · {} select · {} resize",
        keybindings.help(KeyAction::AgentToggleSessions, "Ctrl+]"),
        keybindings.help(KeyAction::AgentNewShell, "Ctrl+N"),
        keybindings.help(KeyAction::AgentKill, "Ctrl+K"),
        keybindings.help(KeyAction::GlobalQuit, "Ctrl+Q"),
        keybindings.help_pair(
            KeyAction::AgentScrollPageUp,
            KeyAction::AgentScrollPageDown,
            "Shift+Alt+Up",
            "Shift+Alt+Down",
        ),
        keybindings.help_pair(
            KeyAction::AgentSwitchPrev,
            KeyAction::AgentSwitchNext,
            "Ctrl+PgUp",
            "Ctrl+PgDn",
        ),
        keybindings.help_pair(
            KeyAction::AgentSidebarPrev,
            KeyAction::AgentSidebarNext,
            "Alt+Up",
            "Alt+Down",
        ),
        keybindings.help_pair(
            KeyAction::AgentPaneResizeLeft,
            KeyAction::AgentPaneResizeRight,
            "Alt+Left",
            "Alt+Right",
        ),
    )
}

fn prov_span(p: Provider) -> Span<'static> {
    let (text, color) = match p {
        Provider::Claude => ("claude  ", THEME_PROVIDER_CLAUDE),
        Provider::Codex => ("codex   ", THEME_PROVIDER_CODEX),
        Provider::OpenCode => ("opencode", THEME_PROVIDER_OPENCODE),
    };
    Span::styled(
        text,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// Badge for shell panes in the agents sidebar. Width matches `prov_span`
/// (8 cols) so columns align across shell and agent rows.
fn shell_badge_span() -> Span<'static> {
    Span::styled(
        "terminal",
        Style::default()
            .fg(THEME_FG_DIM)
            .add_modifier(Modifier::BOLD),
    )
}

fn age_label(epoch_s: u64) -> String {
    if epoch_s == 0 {
        return "?".into();
    }
    let now = chrono::Utc::now().timestamp() as u64;
    let secs = now.saturating_sub(epoch_s);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 86_400 * 30 {
        format!("{}d", secs / 86_400)
    } else {
        format!("{}w", secs / 604_800)
    }
}

fn tree_session_label(depth: usize, session_id: &str) -> String {
    if depth == 0 {
        session_id.to_string()
    } else {
        format!("{}└ {}", "  ".repeat(depth.saturating_sub(1)), session_id)
    }
}

fn title_edit_display(draft: &str, cursor: usize, width: usize) -> String {
    let cursor = clamp_to_char_boundary(draft, cursor);
    let before = &draft[..cursor];
    let after = &draft[cursor..];
    let full = format!("{}|{}", before, after);
    if UnicodeWidthStr::width(full.as_str()) <= width {
        return fit_width(&full, width, Align::Left);
    }

    if width <= 1 {
        return fit_width("|", width, Align::Left);
    }

    let before_target = (width - 1) / 2;
    let before_visible = suffix_width(before, before_target);
    let before_width = UnicodeWidthStr::width(before_visible.as_str());
    let after_visible = prefix_width(after, width.saturating_sub(1 + before_width));
    fit_width(
        &format!("{}|{}", before_visible, after_visible),
        width,
        Align::Left,
    )
}

fn insert_at_cursor(draft: &mut String, cursor: &mut usize, ch: char) {
    *cursor = clamp_to_char_boundary(draft, *cursor);
    draft.insert(*cursor, ch);
    *cursor += ch.len_utf8();
}

fn delete_before_cursor(draft: &mut String, cursor: &mut usize) {
    *cursor = clamp_to_char_boundary(draft, *cursor);
    if *cursor == 0 {
        return;
    }
    let previous = prev_char_boundary(draft, *cursor);
    draft.drain(previous..*cursor);
    *cursor = previous;
}

fn delete_at_cursor(draft: &mut String, cursor: &mut usize) {
    *cursor = clamp_to_char_boundary(draft, *cursor);
    if *cursor >= draft.len() {
        return;
    }
    let next = next_char_boundary(draft, *cursor);
    draft.drain(*cursor..next);
}

fn prev_char_boundary(s: &str, cursor: usize) -> usize {
    let cursor = clamp_to_char_boundary(s, cursor);
    if cursor == 0 {
        0
    } else {
        s[..cursor]
            .char_indices()
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }
}

fn next_char_boundary(s: &str, cursor: usize) -> usize {
    let cursor = clamp_to_char_boundary(s, cursor);
    if cursor >= s.len() {
        s.len()
    } else {
        cursor + s[cursor..].chars().next().map(char::len_utf8).unwrap_or(0)
    }
}

fn clamp_to_char_boundary(s: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(s.len());
    while cursor > 0 && !s.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn prefix_width(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out
}

fn suffix_width(s: &str, width: usize) -> String {
    let mut chars = Vec::new();
    let mut used = 0usize;
    for ch in s.chars().rev() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        chars.push(ch);
        used += ch_width;
    }
    chars.into_iter().rev().collect()
}

#[derive(Debug, Clone, Copy)]
enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
struct ListColumns {
    state: usize,
    id: usize,
    age: usize,
    title: usize,
    cwd: usize,
}

impl ListColumns {
    fn row_width(self) -> usize {
        let mut width = 2 + self.state + 1 + PROVIDER_COLUMN_WIDTH + 1 + self.id;
        if self.age > 0 {
            width += 1 + self.age;
        }
        if self.title > 0 {
            width += 1 + self.title;
        }
        if self.cwd > 0 {
            width += 1 + self.cwd;
        }
        width
    }
}

const LIST_MARKER_WIDTH: usize = 2;
const STATE_COLUMN_WIDTH: usize = 8;
const PROVIDER_COLUMN_WIDTH: usize = 8;
const AGE_COLUMN_WIDTH: usize = 4;

fn list_columns(row_width: u16) -> ListColumns {
    let content = (row_width as usize).saturating_sub(LIST_MARKER_WIDTH);
    let fixed = STATE_COLUMN_WIDTH + 1 + PROVIDER_COLUMN_WIDTH + 1;
    let rest = content.saturating_sub(fixed);
    if rest == 0 {
        return ListColumns {
            state: STATE_COLUMN_WIDTH,
            id: 0,
            age: 0,
            title: 0,
            cwd: 0,
        };
    }

    if rest < 13 {
        return ListColumns {
            state: STATE_COLUMN_WIDTH,
            id: rest,
            age: 0,
            title: 0,
            cwd: 0,
        };
    }

    let rest_without_age = rest.saturating_sub(1 + AGE_COLUMN_WIDTH);
    if rest_without_age < 19 {
        return ListColumns {
            state: STATE_COLUMN_WIDTH,
            id: rest_without_age,
            age: AGE_COLUMN_WIDTH,
            title: 0,
            cwd: 0,
        };
    }

    let rest_without_title = rest_without_age.saturating_sub(1);
    if rest_without_title < 31 {
        let (id, title, cwd) = allocate_fluid_columns(rest_without_title, false);
        return ListColumns {
            state: STATE_COLUMN_WIDTH,
            id,
            age: AGE_COLUMN_WIDTH,
            title,
            cwd,
        };
    }

    let rest_without_cwd = rest_without_title.saturating_sub(1);
    let (id, title, cwd) = allocate_fluid_columns(rest_without_cwd, true);
    ListColumns {
        state: STATE_COLUMN_WIDTH,
        id,
        age: AGE_COLUMN_WIDTH,
        title,
        cwd,
    }
}

fn allocate_fluid_columns(total: usize, include_cwd: bool) -> (usize, usize, usize) {
    let id_min = 8;
    let title_min = 10;
    let cwd_min = if include_cwd { 12 } else { 0 };
    let min_total = id_min + title_min + cwd_min;
    let mut id = id_min;
    let mut title = title_min;
    let mut cwd = cwd_min;
    let mut slack = total.saturating_sub(min_total);

    let id_extra = slack.min(28);
    id += id_extra;
    slack -= id_extra;

    let title_extra = slack.min(if include_cwd { 38 } else { usize::MAX });
    title += title_extra;
    slack -= title_extra;

    if include_cwd {
        cwd += slack;
    } else {
        title += slack;
    }

    (id, title, cwd)
}

fn list_header(cols: &ListColumns) -> String {
    let mut out = String::from("  ");
    out.push_str(&fit_width("state", cols.state, Align::Left));
    out.push(' ');
    out.push_str(&fit_width("provider", PROVIDER_COLUMN_WIDTH, Align::Left));
    out.push(' ');
    out.push_str(&fit_width("session", cols.id, Align::Left));
    if cols.age > 0 {
        out.push(' ');
        out.push_str(&fit_width("age", cols.age, Align::Right));
    }
    if cols.title > 0 {
        out.push(' ');
        out.push_str(&fit_width("title", cols.title, Align::Left));
    }
    if cols.cwd > 0 {
        out.push(' ');
        out.push_str(&fit_width("cwd", cols.cwd, Align::Left));
    }
    out
}

fn focus_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(THEME_SHORTCUT).bg(THEME_BG)
    } else {
        Style::default().fg(THEME_BORDER).bg(THEME_BG)
    }
}

fn preview_mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Summary => "summary",
        Mode::Full => "full",
    }
}

fn preview_mode_short(mode: Mode) -> &'static str {
    match mode {
        Mode::Summary => "S",
        Mode::Full => "F",
    }
}

#[cfg(test)]
fn max_preview_scroll(text: &str, area: Rect) -> u16 {
    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    wrap_preview_lines(text, inner_width)
        .len()
        .saturating_sub(inner_height)
        .min(u16::MAX as usize) as u16
}

fn wrap_preview_lines(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for source in text.split('\n') {
        let mut row = String::new();
        let mut used = 0usize;
        let mut saw_char = false;
        for ch in source.chars() {
            if ch == '\r' {
                continue;
            }
            saw_char = true;
            if ch == '\t' {
                for _ in 0..4 {
                    push_preview_piece(&mut rows, &mut row, &mut used, " ", 1, width);
                }
                continue;
            }

            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if char_width == 0 {
                continue;
            }
            let mut buf = [0; 4];
            let piece = ch.encode_utf8(&mut buf);
            push_preview_piece(&mut rows, &mut row, &mut used, piece, char_width, width);
        }
        if saw_char || !row.is_empty() {
            rows.push(row);
        } else {
            rows.push(String::new());
        }
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

fn push_preview_piece(
    rows: &mut Vec<String>,
    row: &mut String,
    used: &mut usize,
    piece: &str,
    piece_width: usize,
    width: usize,
) {
    let (piece, piece_width) = if piece_width > width {
        ("…", 1)
    } else {
        (piece, piece_width)
    };
    if *used + piece_width > width {
        rows.push(std::mem::take(row));
        *used = 0;
    }
    row.push_str(piece);
    *used += piece_width;
}

fn fit_width(s: &str, width: usize, align: Align) -> String {
    let clipped = truncate_width(s, width);
    let pad = width.saturating_sub(UnicodeWidthStr::width(clipped.as_str()));
    match align {
        Align::Left => format!("{}{}", clipped, " ".repeat(pad)),
        Align::Right => format!("{}{}", " ".repeat(pad), clipped),
    }
}

fn sanitize_for_single_line(s: &str) -> String {
    session::render::sanitize_for_terminal(s)
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect()
}

fn truncate_width(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let sanitized = sanitize_for_single_line(s);
    let s = sanitized.as_str();
    if UnicodeWidthStr::width(s) <= width {
        return s.to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    let reserve = width.saturating_sub(1);
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > reserve {
            break;
        }
        used += w;
        out.push(ch);
    }
    out.push('…');
    out
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(v[1])[1]
}

fn centered_rect_fixed(width: u16, height: u16, r: Rect) -> Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);
    Rect::new(
        r.x + r.width.saturating_sub(width) / 2,
        r.y + r.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression tests for the vt100 wide-char-at-edge panic. The bytes
    // below trigger Screen::text() / Row::clear_wide() OOB indexing in
    // upstream vt100 0.16.2. We rely on the local fork in
    // vendor/vt100-cokac to bound-check the access. Without the fork these
    // tests would panic inside vt100 -- catch_unwind would surface that as
    // safe_parser_process returning false.
    #[test]
    fn safe_parser_process_handles_wide_char_after_shrink_resize() {
        let cases: &[(u16, u16, &[u8], u16, u16, &[u8])] = &[
            // (init_cols, init_rows, init bytes, new_cols, new_rows, panic bytes)
            (
                20,
                5,
                b"\x1b[1;19H\xe6\xbc\xa2",
                19,
                5,
                b"\x1b[1;19H\x1b[1X",
            ),
            (20, 5, b"\x1b[1;19H\xe6\xbc\xa2", 19, 5, b"\x1b[1;19HA"),
            (20, 5, b"\x1b[1;19H\xe6\xbc\xa2", 19, 5, b"\x1b[1;19H\x1b[K"),
            (
                20,
                5,
                b"\x1b[1;19H\xe6\xbc\xa2",
                19,
                5,
                b"\x1b[1;19H\x1b[1P",
            ),
            // matches the production "len 76 index 76" panic exactly
            (
                77,
                5,
                b"\x1b[1;76H\xe6\xbc\xa2",
                76,
                5,
                b"\x1b[1;76H\x1b[1X",
            ),
            (20, 5, b"\x1b[1;19H\xf0\x9f\x98\x80", 19, 5, b"\x1b[1;19HA"),
        ];
        for (c0, r0, init, c1, r1, post) in cases {
            let mut parser = vt100::Parser::new(*r0, *c0, 0);
            assert!(
                safe_parser_process(&mut parser, init),
                "init chunk panicked for case c0={} r0={}",
                c0,
                r0
            );
            parser.screen_mut().set_size(*r1, *c1);
            assert!(
                safe_parser_process(&mut parser, post),
                "post-resize chunk panicked for case c0={}->c1={}",
                c0,
                c1
            );
        }
    }

    #[test]
    fn debug_log_file_for_uses_single_runtime_log() {
        for event in [
            "main_start",
            "agent_client_output_processed",
            "daemon_spawned",
            "preview_worker_request",
            "session_refresh_ok",
            "sessions_pane_resize",
            "clone_ok",
            "delete_ok",
            "title_edit_save",
            "filter_search_start",
            "provider_codex_read_start",
        ] {
            assert_eq!(debug_log_file_for(event), DEBUG_LOG_FILE);
        }
    }

    fn session_info(provider: Provider, session_id: &str, cwd: &str) -> SessionInfo {
        SessionInfo {
            provider,
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            source: PathBuf::from("/tmp/source"),
            updated_at_epoch_s: 0,
            title: None,
        }
    }

    fn new_agent_info(provider: Provider, cwd: &str) -> SessionInfo {
        let mut info = session_info(provider, "new-agent", cwd);
        info.source = PathBuf::from(NEW_AGENT_SESSION_SOURCE_MARKER);
        info.title = Some(new_agent_pane_title(provider, cwd));
        info
    }

    fn default_agent_launch_spec(
        info: &SessionInfo,
        launch_mode: AgentLaunchMode,
    ) -> AgentLaunchSpec {
        agent_launch_spec_with_programs(info, launch_mode, &AgentProgramSettings::default())
    }

    fn app_for_key_tests() -> App {
        let (preview_tx, _preview_request_rx) = mpsc::channel::<PreviewRequest>();
        let (_preview_result_tx, preview_rx) = mpsc::channel::<PreviewResult>();
        let mut settings = Settings::default();
        settings.skip_save = true;
        App {
            settings,
            keybindings: KeyBindings::default(),
            keybindings_path: None,
            keybindings_mtime: None,
            sessions: Vec::new(),
            live_shells: Vec::new(),
            clone_links: Vec::new(),
            agent_states: HashMap::new(),
            new_agent_backing_aliases: HashMap::new(),
            last_agent_state_poll: Instant::now(),
            list_state: ListState::default(),
            session_view: SessionViewMode::Tree,
            provider_filter: ProviderFilter::All,
            text_filter: String::new(),
            text_filter_matches: HashSet::new(),
            search_seq: 0,
            search_pending: None,
            input_mode: InputMode::Normal,
            preview_cache: HashMap::new(),
            preview_cache_order: VecDeque::new(),
            preview_requested: None,
            preview_seq: 0,
            preview_tx,
            preview_rx,
            preview_mode: Mode::Summary,
            preview_scroll: 0,
            preview_page_height: 10,
            focus: FocusPane::Sessions,
            status: String::new(),
            active_agent: None,
            should_quit: false,
            main_tx: None,
            next_reader_id: 0,
            show_sessions_view: true,
        }
    }

    #[test]
    fn keybinding_json_overrides_defaults_and_can_disable_actions() {
        let mut keybindings = KeyBindings::default();
        keybindings.apply_json(&serde_json::json!({
            "sessions": {
                "launch_agent": ["x"],
                "quit": []
            },
            "agent": {
                "scroll_page_down": ["ctrl+d"]
            }
        }));

        assert!(keybindings.matches(
            KeyAction::SessionLaunchAgent,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)
        ));
        assert!(!keybindings.matches(
            KeyAction::SessionLaunchAgent,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)
        ));
        assert!(!keybindings.matches(
            KeyAction::SessionQuit,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
        ));
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)
            ),
            Some(AgentScrollAction::Pages(-1))
        );
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::ALT)
            ),
            None
        );
    }

    #[test]
    fn missing_keybinding_file_is_created_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybinding.json");

        let (keybindings, mtime) = KeyBindings::load_with_mtime(Some(&path));

        assert!(path.exists());
        assert!(mtime.is_some());
        assert!(keybindings.matches(
            KeyAction::SessionLaunchAgent,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)
        ));
        assert!(keybindings.matches(
            KeyAction::SessionDelete,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)
        ));
        assert!(keybindings.matches(
            KeyAction::SessionDelete,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)
        ));
        let content = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["sessions"]["launch_agent"], serde_json::json!(["e"]));
        assert_eq!(
            value["sessions"]["delete"],
            serde_json::json!(["delete", "d"])
        );
        assert_eq!(
            value["agent"]["scroll_page_up"],
            serde_json::json!(["shift+alt+up", "shift+alt+pageup"])
        );
        assert_eq!(
            value["agent"]["scroll_page_down"],
            serde_json::json!(["shift+alt+down", "shift+alt+pagedown"])
        );
    }

    #[test]
    fn legacy_generated_agent_scroll_page_bindings_are_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybinding.json");
        fs::write(
            &path,
            r#"{
  "agent": {
    "scroll_page_up": ["shift+pageup", "alt+pageup"],
    "scroll_page_down": ["shift+pagedown", "alt+pagedown"]
  }
}
"#,
        )
        .unwrap();

        let (keybindings, _) = KeyBindings::load_with_mtime(Some(&path));

        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT | KeyModifiers::ALT)
            ),
            Some(AgentScrollAction::Pages(1))
        );
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT | KeyModifiers::ALT)
            ),
            Some(AgentScrollAction::Pages(-1))
        );
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT | KeyModifiers::ALT)
            ),
            Some(AgentScrollAction::Pages(1))
        );
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::SHIFT | KeyModifiers::ALT)
            ),
            Some(AgentScrollAction::Pages(-1))
        );
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT)
            ),
            None
        );

        let content = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            value["agent"]["scroll_page_up"],
            serde_json::json!(["shift+alt+up", "shift+alt+pageup"])
        );
        assert_eq!(
            value["agent"]["scroll_page_down"],
            serde_json::json!(["shift+alt+down", "shift+alt+pagedown"])
        );
    }

    #[test]
    fn previous_generated_agent_scroll_page_bindings_are_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybinding.json");
        fs::write(
            &path,
            r#"{
  "agent": {
    "scroll_page_up": ["shift+alt+pageup"],
    "scroll_page_down": ["shift+alt+pagedown"]
  }
}
"#,
        )
        .unwrap();

        let (keybindings, _) = KeyBindings::load_with_mtime(Some(&path));

        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT | KeyModifiers::ALT)
            ),
            Some(AgentScrollAction::Pages(1))
        );
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT | KeyModifiers::ALT)
            ),
            Some(AgentScrollAction::Pages(-1))
        );

        let content = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            value["agent"]["scroll_page_up"],
            serde_json::json!(["shift+alt+up", "shift+alt+pageup"])
        );
        assert_eq!(
            value["agent"]["scroll_page_down"],
            serde_json::json!(["shift+alt+down", "shift+alt+pagedown"])
        );
    }

    #[test]
    fn custom_agent_scroll_page_bindings_are_not_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybinding.json");
        fs::write(
            &path,
            r#"{
  "agent": {
    "scroll_page_up": ["shift+pageup", "alt+k"]
  }
}
"#,
        )
        .unwrap();

        let (keybindings, _) = KeyBindings::load_with_mtime(Some(&path));

        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT)
            ),
            Some(AgentScrollAction::Pages(1))
        );
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT | KeyModifiers::ALT)
            ),
            None
        );
        assert_eq!(
            agent_scrollback_key(
                &keybindings,
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT | KeyModifiers::ALT)
            ),
            None
        );
        let content = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            value["agent"]["scroll_page_up"],
            serde_json::json!(["shift+pageup", "alt+k"])
        );
    }

    #[test]
    fn custom_session_launch_key_is_used_by_handler() {
        let mut app = app_for_key_tests();
        app.keybindings.apply_json(&serde_json::json!({
            "sessions": {
                "launch_agent": ["x"]
            }
        }));
        app.sessions
            .push(session_info(Provider::Codex, "codex-id", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            100,
            80,
            20,
        );
        assert!(matches!(app.input_mode, InputMode::Normal));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            100,
            80,
            20,
        );
        assert!(matches!(app.input_mode, InputMode::AgentLaunch { .. }));
    }

    #[test]
    fn delete_key_opens_session_delete_confirm() {
        let mut app = app_for_key_tests();
        app.sessions
            .push(session_info(Provider::Codex, "codex-id", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
            100,
            80,
            20,
        );

        match app.input_mode {
            InputMode::Confirm {
                action:
                    PendingAction::Delete {
                        info,
                        removed_index,
                    },
                ..
            } => {
                assert_eq!(info.session_id, "codex-id");
                assert_eq!(removed_index, Some(0));
            }
            other => panic!("expected delete confirm, got {:?}", other),
        }
    }

    #[test]
    fn confirm_delete_uses_stored_target_even_if_selection_changes() {
        let dir = tempfile::tempdir().unwrap();
        let victim_path = dir.path().join("victim.jsonl");
        let other_path = dir.path().join("other.jsonl");
        fs::write(&victim_path, "{}\n").unwrap();
        fs::write(&other_path, "{}\n").unwrap();

        let mut victim = session_info(Provider::Claude, "victim", "/repo");
        victim.source = victim_path.clone();
        let mut other = session_info(Provider::Claude, "other", "/repo");
        other.source = other_path.clone();

        let mut app = app_for_key_tests();
        app.sessions.push(other);
        app.list_state.select(Some(0));
        app.input_mode = InputMode::Confirm {
            prompt: "Delete?".to_string(),
            action: PendingAction::Delete {
                info: victim,
                removed_index: Some(0),
            },
        };

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(!victim_path.exists());
        assert!(other_path.exists());
    }

    #[test]
    fn delete_confirm_prompt_sanitizes_session_id_controls() {
        let info = session_info(
            Provider::Claude,
            "sid\x1b]0;owned\x07next\nrow\tend",
            "/repo",
        );
        let prompt = delete_confirm_prompt(&info, "y/Y", "Esc/n");
        let lines: Vec<&str> = prompt.lines().collect();

        assert_eq!(lines.len(), 4);
        assert!(!prompt.contains('\x1b'));
        assert_eq!(lines[1], "sidnext row end");
    }

    #[test]
    fn confirm_modal_uses_compact_prompt_sized_area() {
        let area = Rect::new(0, 0, 120, 40);
        let prompt = delete_confirm_prompt(
            &session_info(Provider::Codex, "codex-id", "/repo"),
            "y/Y",
            "Esc/n",
        );
        let modal = confirm_modal_area(&prompt, area);

        assert_eq!(prompt.lines().count(), 4);
        assert_eq!(modal.width, 38);
        assert_eq!(modal.height, 6);
        assert_eq!(modal.x, 41);
        assert_eq!(modal.y, 17);
    }

    #[test]
    fn confirm_modal_wraps_without_using_screen_percent_height() {
        let area = Rect::new(0, 0, 50, 20);
        let prompt = delete_confirm_prompt(
            &session_info(
                Provider::OpenCode,
                "019e61ef-ec81-7bc0-a8a0-9e64619fa037-extra-long-id",
                "/repo",
            ),
            "y/Y",
            "Esc/n",
        );
        let modal = confirm_modal_area(&prompt, area);

        assert_eq!(modal.width, 50);
        assert!(modal.height <= 8);
    }

    #[test]
    fn keybinding_file_reload_applies_on_next_keypress() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybinding.json");
        fs::write(
            &path,
            r#"{"sessions":{"launch_agent":["x"],"toggle_view":["z"]}}"#,
        )
        .unwrap();

        let mut app = app_for_key_tests();
        app.keybindings_path = Some(path);
        app.keybindings_mtime = None;
        app.sessions
            .push(session_info(Provider::Codex, "codex-id", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            100,
            80,
            20,
        );
        assert!(matches!(app.input_mode, InputMode::Normal));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            100,
            80,
            20,
        );
        assert!(matches!(app.input_mode, InputMode::AgentLaunch { .. }));
        assert!(app.keybindings_mtime.is_some());
    }

    #[test]
    fn deleted_keybinding_file_is_recreated_on_next_keypress() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybinding.json");
        fs::write(&path, r#"{"sessions":{"launch_agent":["x"]}}"#).unwrap();
        let (keybindings, keybindings_mtime) = KeyBindings::load_with_mtime(Some(&path));
        fs::remove_file(&path).unwrap();

        let mut app = app_for_key_tests();
        app.keybindings = keybindings;
        app.keybindings_path = Some(path.clone());
        app.keybindings_mtime = keybindings_mtime;
        app.sessions
            .push(session_info(Provider::Codex, "codex-id", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(path.exists());
        assert!(matches!(app.input_mode, InputMode::AgentLaunch { .. }));
    }

    #[test]
    fn keybinding_reload_failure_keeps_previous_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybinding.json");
        fs::write(&path, r#"{"sessions":{"launch_agent":["x"]}}"#).unwrap();
        let (keybindings, _) = KeyBindings::load_with_mtime(Some(&path));

        fs::write(&path, "{ invalid json").unwrap();

        let mut app = app_for_key_tests();
        app.keybindings = keybindings;
        app.keybindings_path = Some(path);
        app.keybindings_mtime = None;
        app.sessions
            .push(session_info(Provider::Codex, "codex-id", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(matches!(app.input_mode, InputMode::AgentLaunch { .. }));
    }

    #[test]
    fn title_edit_key_uses_selected_session_title() {
        let mut app = app_for_key_tests();
        let mut info = session_info(Provider::Claude, "s1", "/tmp/project");
        info.title = Some("Existing Title".to_string());
        app.sessions.push(info);
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        match app.input_mode {
            InputMode::TitleEdit {
                source,
                draft,
                cursor,
            } => {
                assert_eq!(source.session_id, "s1");
                assert_eq!(draft, "Existing Title");
                assert_eq!(cursor, "Existing Title".len());
            }
            other => panic!("expected title edit mode, got {:?}", other),
        }
    }

    #[test]
    fn title_edit_mode_accepts_text_and_cancel() {
        let mut app = app_for_key_tests();
        app.input_mode = InputMode::TitleEdit {
            source: session_info(Provider::Claude, "s1", "/tmp/project"),
            draft: "Hi".to_string(),
            cursor: 2,
        };

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            100,
            80,
            20,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        match &app.input_mode {
            InputMode::TitleEdit { draft, .. } => assert_eq!(draft, "Ho"),
            other => panic!("expected title edit mode, got {:?}", other),
        }

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(app.status, "cancelled.");
    }

    #[test]
    fn title_edit_mode_moves_cursor_and_edits_at_cursor() {
        let mut app = app_for_key_tests();
        app.input_mode = InputMode::TitleEdit {
            source: session_info(Provider::Claude, "s1", "/tmp/project"),
            draft: "가나".to_string(),
            cursor: "가나".len(),
        };

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            100,
            80,
            20,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
            100,
            80,
            20,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
            100,
            80,
            20,
        );

        match &app.input_mode {
            InputMode::TitleEdit { draft, cursor, .. } => {
                assert_eq!(draft, "가X");
                assert_eq!(*cursor, "가X".len());
            }
            other => panic!("expected title edit mode, got {:?}", other),
        }
    }

    #[test]
    fn slash_opens_search_dialog_without_live_filtering() {
        let mut app = app_for_key_tests();
        app.text_filter = "old".to_string();
        app.sessions
            .push(session_info(Provider::Codex, "old-session", "/repo"));
        app.sessions
            .push(session_info(Provider::Codex, "new-session", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        match &app.input_mode {
            InputMode::Filter { draft, cursor } => {
                assert_eq!(draft, "old");
                assert_eq!(*cursor, "old".len());
            }
            other => panic!("expected filter mode, got {:?}", other),
        }

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert_eq!(app.text_filter, "old");
        match &app.input_mode {
            InputMode::Filter { draft, .. } => assert_eq!(draft, "oldx"),
            other => panic!("expected filter mode, got {:?}", other),
        }
    }

    #[test]
    fn search_cancel_preserves_existing_applied_filter() {
        let mut app = app_for_key_tests();
        app.text_filter = "old".to_string();
        app.input_mode = InputMode::Filter {
            draft: "new".to_string(),
            cursor: "new".len(),
        };

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(app.text_filter, "old");
        assert_eq!(app.status, "cancelled.");
    }

    #[test]
    fn search_filter_can_match_full_session_result_keys() {
        let mut app = app_for_key_tests();
        let info = session_info(Provider::Codex, "sid", "/repo");

        app.text_filter = "needle".to_string();
        assert!(!app.matches_session_filters(&info));

        app.text_filter_matches.insert(AgentKey::new(&info));
        assert!(app.matches_session_filters(&info));
    }

    #[test]
    fn search_result_applies_pending_query_and_closes_dialog() {
        let mut app = app_for_key_tests();
        let info = session_info(Provider::Codex, "sid", "/repo");
        app.sessions.push(info.clone());
        app.input_mode = InputMode::Filter {
            draft: "needle".to_string(),
            cursor: "needle".len(),
        };
        app.search_pending = Some(SearchPending {
            seq: 7,
            query: "needle".to_string(),
            started_at: Instant::now(),
        });

        app.on_search_result(SearchWorkerResult {
            seq: 7,
            query: "needle".to_string(),
            hits: Ok(vec![session::SearchHit {
                info: info.clone(),
                matches: 1,
                snippet: "needle".to_string(),
            }]),
        });

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(app.search_pending.is_none());
        assert_eq!(app.text_filter, "needle");
        assert!(app.text_filter_matches.contains(&AgentKey::new(&info)));
    }

    #[test]
    fn cancelled_search_result_is_ignored() {
        let mut app = app_for_key_tests();
        let info = session_info(Provider::Codex, "sid", "/repo");
        app.text_filter = "old".to_string();
        app.search_pending = Some(SearchPending {
            seq: 7,
            query: "needle".to_string(),
            started_at: Instant::now(),
        });
        app.input_mode = InputMode::Filter {
            draft: "needle".to_string(),
            cursor: "needle".len(),
        };

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            100,
            80,
            20,
        );
        app.on_search_result(SearchWorkerResult {
            seq: 7,
            query: "needle".to_string(),
            hits: Ok(vec![session::SearchHit {
                info,
                matches: 1,
                snippet: "needle".to_string(),
            }]),
        });

        assert_eq!(app.text_filter, "old");
        assert!(app.text_filter_matches.is_empty());
        assert!(app.search_pending.is_none());
    }

    #[test]
    fn f_no_longer_cycles_provider_filter_and_enter_toggles_preview() {
        // `f` was the provider-filter cycle key; that feature is disabled.
        // Pressing it should not change the filter or the selection.
        let mut app = app_for_key_tests();
        app.sessions
            .push(session_info(Provider::Claude, "claude-id", "/repo"));
        app.sessions
            .push(session_info(Provider::Codex, "codex-id", "/repo"));
        app.list_state.select(Some(1));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            100,
            80,
            20,
        );
        assert!(matches!(app.provider_filter, ProviderFilter::All));
        assert_eq!(app.list_state.selected(), Some(1));
        assert_eq!(app.preview_mode, Mode::Summary);

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            100,
            80,
            20,
        );
        assert_eq!(app.preview_mode, Mode::Full);
    }

    #[test]
    fn ctrl_bracket_without_active_agent_does_not_attach_selected_session() {
        let mut app = app_for_key_tests();
        app.sessions
            .push(session_info(Provider::Codex, "codex-id", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL),
            100,
            80,
            20,
        );

        assert!(app.active_agent.is_none());
        assert!(app.show_sessions_view);
        assert_eq!(
            app.status,
            "no active agent to switch to; press e to start selected agent"
        );
    }

    #[test]
    fn e_opens_agent_launch_mode_selector() {
        let mut app = app_for_key_tests();
        app.sessions
            .push(session_info(Provider::Codex, "codex-id", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        match &app.input_mode {
            InputMode::AgentLaunch { source, selected } => {
                assert_eq!(source.session_id, "codex-id");
                assert_eq!(*selected, 0);
            }
            other => panic!("expected agent launch mode, got {:?}", other),
        }

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            100,
            80,
            20,
        );
        match &app.input_mode {
            InputMode::AgentLaunch { selected, .. } => assert_eq!(*selected, 1),
            other => panic!("expected agent launch mode, got {:?}", other),
        }

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            100,
            80,
            20,
        );
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(app.status, "cancelled.");
    }

    #[test]
    fn e_on_live_selected_session_switches_without_launch_selector() {
        let mut app = app_for_key_tests();
        let info = session_info(Provider::Codex, "codex-id", "/repo");
        app.agent_states.insert(
            AgentKey::new(&info),
            AgentListState::Live {
                activity: AgentActivity::Quiet,
            },
        );
        app.sessions.push(info);
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(app.active_agent.is_none());
        assert!(app.status.contains("cannot switch to live"));
    }

    #[test]
    fn e_on_foreign_attached_selected_session_does_not_open_launch_selector() {
        let mut app = app_for_key_tests();
        let info = session_info(Provider::Claude, "claude-id", "/repo");
        app.agent_states.insert(
            AgentKey::new(&info),
            AgentListState::Attached {
                mine: false,
                activity: AgentActivity::Busy,
            },
        );
        app.sessions.push(info);
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(app.active_agent.is_none());
        assert!(app.status.contains("already attached in another"));
    }

    #[test]
    fn e_on_live_selected_missing_cwd_does_not_prompt_create_folder() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("deleted-project");
        let mut app = app_for_key_tests();
        let info = session_info(
            Provider::Codex,
            "live-missing-cwd",
            &missing.display().to_string(),
        );
        app.agent_states.insert(
            AgentKey::new(&info),
            AgentListState::Live {
                activity: AgentActivity::Quiet,
            },
        );
        app.sessions.push(info);
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(!missing.exists());
        assert!(app.status.contains("cannot switch to live"));
    }

    #[test]
    fn ctrl_n_opens_new_session_dialog_from_selected_session() {
        let mut app = app_for_key_tests();
        app.sessions
            .push(session_info(Provider::OpenCode, "opencode-id", "/repo"));
        app.list_state.select(Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            100,
            80,
            20,
        );

        match &app.input_mode {
            InputMode::NewSession {
                selected,
                kind,
                cwd,
                cwd_cursor,
                provider,
                provider_options,
                launch_mode,
            } => {
                assert_eq!(*selected, NEW_SESSION_FIELD_KIND);
                assert_eq!(*kind, NewSessionKind::Terminal);
                assert_eq!(cwd, "/repo");
                assert_eq!(*cwd_cursor, "/repo".len());
                let expected_provider = if provider_options.is_empty()
                    || provider_options.contains(&Provider::OpenCode)
                {
                    Provider::OpenCode
                } else {
                    provider_options[0]
                };
                assert_eq!(*provider, expected_provider);
                assert_eq!(*launch_mode, AgentLaunchMode::Normal);
            }
            other => panic!("expected new session mode, got {:?}", other),
        }
    }

    #[test]
    fn new_session_cwd_field_keeps_plain_jklh_as_text() {
        let mut app = app_for_key_tests();
        app.input_mode = InputMode::NewSession {
            selected: NEW_SESSION_FIELD_CWD,
            kind: NewSessionKind::Terminal,
            cwd: "/repo".into(),
            cwd_cursor: "/repo".len(),
            provider: Provider::Codex,
            provider_options: Vec::new(),
            launch_mode: AgentLaunchMode::Normal,
        };

        for ch in ['j', 'k', 'h', 'l', ' '] {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                100,
                80,
                20,
            );
        }

        match &app.input_mode {
            InputMode::NewSession {
                selected,
                cwd,
                cwd_cursor,
                ..
            } => {
                assert_eq!(*selected, NEW_SESSION_FIELD_CWD);
                assert_eq!(cwd, "/repojkhl ");
                assert_eq!(*cwd_cursor, "/repojkhl ".len());
            }
            other => panic!("expected new session mode, got {:?}", other),
        }
    }

    #[test]
    fn new_session_empty_folder_keeps_dialog_open() {
        let mut app = app_for_key_tests();
        app.input_mode = InputMode::NewSession {
            selected: NEW_SESSION_FIELD_KIND,
            kind: NewSessionKind::Terminal,
            cwd: "".into(),
            cwd_cursor: 0,
            provider: Provider::Codex,
            provider_options: Vec::new(),
            launch_mode: AgentLaunchMode::Normal,
        };

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(matches!(app.input_mode, InputMode::NewSession { .. }));
        assert!(app.status.starts_with("folder path is required."));
    }

    #[test]
    fn normalize_launch_cwd_creates_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let normalized = normalize_launch_cwd(&dir.path().display().to_string()).unwrap();
        assert_eq!(
            normalized,
            dir.path().canonicalize().unwrap().display().to_string()
        );

        let missing = dir.path().join("new").join("project");
        let normalized = normalize_launch_cwd(&missing.display().to_string()).unwrap();
        assert!(missing.is_dir());
        assert_eq!(
            normalized,
            missing.canonicalize().unwrap().display().to_string()
        );

        let file = dir.path().join("file.txt");
        fs::write(&file, "not a directory").unwrap();
        assert!(normalize_launch_cwd(&file.display().to_string())
            .unwrap_err()
            .starts_with("not a folder:"));
        assert!(normalize_launch_cwd("")
            .unwrap_err()
            .starts_with("folder path is required."));
    }

    #[test]
    fn existing_agent_launch_cwd_validation_rejects_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("deleted-project");
        let info = session_info(
            Provider::Claude,
            "missing-cwd",
            &missing.display().to_string(),
        );

        let error = validate_session_launch_cwd(&info).unwrap_err().to_string();

        assert!(error.starts_with("launch folder does not exist:"));
        assert!(error.contains("deleted-project"));
    }

    #[test]
    fn existing_agent_launch_cwd_validation_rejects_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let info = session_info(
            Provider::Claude,
            "file-cwd",
            &file.path().display().to_string(),
        );

        let error = validate_session_launch_cwd(&info).unwrap_err().to_string();

        assert!(error.starts_with("launch folder is not a directory:"));
    }

    #[test]
    fn create_agent_launch_cwd_creates_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("new").join("project");

        create_agent_launch_cwd(&missing).unwrap();

        assert!(missing.is_dir());
        validate_agent_launch_cwd(&missing).unwrap();
    }

    #[test]
    fn agent_launch_confirm_prompts_to_create_missing_cwd_before_starting_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("deleted-project");
        let mut app = app_for_key_tests();
        app.input_mode = InputMode::AgentLaunch {
            selected: 0,
            source: session_info(
                Provider::Claude,
                &format!("missing-cwd-{}", uuid::Uuid::now_v7()),
                &missing.display().to_string(),
            ),
        };

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            100,
            80,
            20,
        );

        match &app.input_mode {
            InputMode::Confirm {
                prompt,
                action:
                    PendingAction::CreateMissingLaunchCwd {
                        info,
                        path,
                        cols,
                        rows,
                        launch_mode,
                    },
            } => {
                assert!(prompt.contains("launch folder does not exist"));
                assert!(prompt.contains("Create it and continue?"));
                assert!(info.session_id.starts_with("missing-cwd-"));
                assert_eq!(path, &missing);
                assert_eq!(*cols, 80);
                assert_eq!(*rows, 20);
                assert_eq!(*launch_mode, AgentLaunchMode::Normal);
            }
            other => panic!("expected create-folder confirm, got {:?}", other),
        }
        assert!(app.active_agent.is_none());
        assert!(app.show_sessions_view);
        assert!(!missing.exists());
        assert!(app.status.contains("launch folder missing:"));
        assert!(app.status.contains("deleted-project"));
    }

    #[test]
    fn missing_cwd_create_confirm_cancel_does_not_create_folder() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("deleted-project");
        let mut app = app_for_key_tests();
        app.input_mode = InputMode::Confirm {
            prompt: "Create?".to_string(),
            action: PendingAction::CreateMissingLaunchCwd {
                info: session_info(
                    Provider::Claude,
                    "missing-cwd",
                    &missing.display().to_string(),
                ),
                path: missing.clone(),
                cols: 80,
                rows: 20,
                launch_mode: AgentLaunchMode::Normal,
            },
        };

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(app.status, "cancelled.");
        assert!(!missing.exists());
    }

    #[test]
    fn p_no_longer_cycles_provider_filter() {
        let mut app = app_for_key_tests();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            100,
            80,
            20,
        );
        assert!(matches!(app.provider_filter, ProviderFilter::All));
    }

    #[test]
    fn v_toggles_tree_view_and_preserves_selected_session() {
        let mut app = app_for_key_tests();
        app.session_view = SessionViewMode::List;
        app.sessions
            .push(session_info(Provider::Codex, "parent", "/repo"));
        app.sessions
            .push(session_info(Provider::Claude, "child", "/repo"));
        app.clone_links.push(session::clone_tree::CloneLink {
            parent: session::clone_tree::SessionKey::new(Provider::Codex, "parent"),
            child: session::clone_tree::SessionKey::new(Provider::Claude, "child"),
            cloned_at_epoch_s: 1,
        });
        app.list_state.select(Some(1));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
            100,
            80,
            20,
        );

        assert_eq!(app.session_view, SessionViewMode::Tree);
        assert_eq!(
            app.current().map(|info| info.session_id.as_str()),
            Some("child")
        );
        assert_eq!(app.settings.cokacmux.session_view, SessionViewMode::Tree);
        let rows = app.visible_rows();
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn tree_view_filter_keeps_parent_context_for_matching_child() {
        let mut app = app_for_key_tests();
        app.session_view = SessionViewMode::Tree;
        app.sessions
            .push(session_info(Provider::Codex, "parent", "/repo"));
        app.sessions
            .push(session_info(Provider::Claude, "child", "/repo"));
        app.clone_links.push(session::clone_tree::CloneLink {
            parent: session::clone_tree::SessionKey::new(Provider::Codex, "parent"),
            child: session::clone_tree::SessionKey::new(Provider::Claude, "child"),
            cloned_at_epoch_s: 1,
        });
        app.text_filter = "child".to_string();

        let rows = app.visible_rows();

        let ids: Vec<(&str, usize)> = rows
            .iter()
            .map(|row| (row.info.session_id.as_str(), row.depth))
            .collect();
        assert_eq!(ids, vec![("parent", 0), ("child", 1)]);
    }

    #[test]
    fn truncate_width_respects_display_width() {
        let clipped = truncate_width("abcdef", 4);
        assert_eq!(clipped, "abc…");
        assert!(UnicodeWidthStr::width(clipped.as_str()) <= 4);

        let wide = truncate_width("가나다라", 5);
        assert!(wide.ends_with('…'));
        assert!(UnicodeWidthStr::width(wide.as_str()) <= 5);
    }

    #[test]
    fn truncate_width_sanitizes_terminal_controls() {
        let cleaned = truncate_width("ab\x1b]0;owned\x07cd\nnext\tfield", 40);
        assert_eq!(cleaned, "abcd next field");
        assert!(!cleaned.contains('\x1b'));
        assert!(!cleaned.contains('\n'));
        assert!(!cleaned.contains('\t'));

        let clipped = truncate_width("가\x1b[31m나다라", 5);
        assert!(!clipped.contains('\x1b'));
        assert!(UnicodeWidthStr::width(clipped.as_str()) <= 5);
    }

    #[test]
    fn preview_scroll_accounts_for_wrapped_lines() {
        let area = Rect::new(0, 0, 12, 5); // inner width 10, inner height 3
        assert_eq!(max_preview_scroll("0123456789012345678901234", area), 0);
        assert_eq!(
            max_preview_scroll("01234567890123456789012345678901234", area),
            1
        );
    }

    #[test]
    fn wrap_preview_lines_respects_display_width() {
        let lines = wrap_preview_lines("abcd가나다라\txyz", 5);
        assert!(lines
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= 5));
        assert_eq!(lines[0], "abcd");
        assert_eq!(lines[1], "가나");
    }

    #[test]
    fn settings_normalize_pane_width_and_preserve_unknown_fields() {
        let settings = serde_json::from_str::<Settings>(
            r#"{
              "cokacmux": {
                "sessions_pane_percent": 99,
                "sessions_pane_width": 88,
                "agent_sidebar_width": 999,
                "session_view": "list",
                "agent_programs": {
                  "codex": "/opt/codex/bin/codex",
                  "claude": "/opt/claude/bin/claude",
                  "future_agent": "/opt/future/bin/agent"
                },
                "debug": true,
                "future": true
              },
              "other_tool": {
                "enabled": true
              }
            }"#,
        )
        .unwrap()
        .normalized();

        assert_eq!(settings.cokacmux.sessions_pane_percent, 99);
        assert_eq!(settings.cokacmux.sessions_pane_width, Some(88));
        assert_eq!(settings.cokacmux.agent_sidebar_width, 999);
        assert_eq!(settings.cokacmux.session_view, SessionViewMode::List);
        assert_eq!(
            settings
                .cokacmux
                .agent_programs
                .program_for(Provider::Codex),
            "/opt/codex/bin/codex"
        );
        assert_eq!(
            settings
                .cokacmux
                .agent_programs
                .program_for(Provider::Claude),
            "/opt/claude/bin/claude"
        );
        assert_eq!(
            settings
                .cokacmux
                .agent_programs
                .extra
                .get("future_agent")
                .and_then(|v| v.as_str()),
            Some("/opt/future/bin/agent")
        );
        assert!(settings.cokacmux._debug);
        assert_eq!(
            settings
                .cokacmux
                .extra
                .get("future")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(settings.extra.contains_key("other_tool"));

        let serialized = serde_json::to_value(&settings).unwrap();
        assert!(serialized.pointer("/cokacmux/debug").is_none());
        assert_eq!(
            serialized
                .pointer("/cokacmux/agent_programs/codex")
                .and_then(|v| v.as_str()),
            Some("/opt/codex/bin/codex")
        );
    }

    #[test]
    fn settings_default_session_view_is_tree() {
        let settings = serde_json::from_str::<Settings>(r#"{"cokacmux": {}}"#)
            .unwrap()
            .normalized();

        assert_eq!(settings.cokacmux.session_view, SessionViewMode::Tree);
    }

    #[test]
    fn settings_default_serializes_agent_program_placeholders() {
        let settings = Settings::default();
        let serialized = serde_json::to_value(&settings).unwrap();

        assert!(serialized
            .pointer("/cokacmux/sessions_pane_width")
            .is_some_and(serde_json::Value::is_null));
        assert_eq!(
            serialized
                .pointer("/cokacmux/agent_programs/codex")
                .and_then(|v| v.as_str()),
            Some("")
        );
        assert_eq!(
            serialized
                .pointer("/cokacmux/agent_programs/claude")
                .and_then(|v| v.as_str()),
            Some("")
        );
        assert_eq!(
            serialized
                .pointer("/cokacmux/agent_programs/opencode")
                .and_then(|v| v.as_str()),
            Some("")
        );
        assert_eq!(
            settings
                .cokacmux
                .agent_programs
                .program_for(Provider::Codex),
            "codex"
        );
    }

    #[test]
    fn agent_provider_choices_use_available_programs() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir
            .path()
            .join(if cfg!(windows) { "codex.cmd" } else { "codex" });
        fs::write(
            &codex,
            if cfg!(windows) {
                "@echo off\r\n"
            } else {
                "#!/bin/sh\n"
            },
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&codex).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&codex, perms).unwrap();
        }

        let agent_programs = AgentProgramSettings {
            codex: Some(codex.display().to_string()),
            claude: Some(dir.path().join("missing-claude").display().to_string()),
            opencode: Some(dir.path().join("missing-opencode").display().to_string()),
            extra: serde_json::Map::new(),
        };

        assert_eq!(
            available_agent_provider_options(&agent_programs),
            vec![Provider::Codex]
        );
        assert_eq!(
            normalize_agent_provider_selection(Provider::Claude, &[Provider::Codex]),
            Some(Provider::Codex)
        );
    }

    #[test]
    fn agent_provider_choice_movement_skips_unavailable_options() {
        let options = [Provider::Codex, Provider::OpenCode];

        assert_eq!(
            move_provider_in_options(Provider::Claude, 1, &options),
            Provider::Codex
        );
        assert_eq!(
            move_provider_in_options(Provider::Codex, 1, &options),
            Provider::OpenCode
        );
        assert_eq!(
            move_provider_in_options(Provider::Codex, -1, &options),
            Provider::OpenCode
        );
        assert_eq!(
            move_provider_in_options(Provider::Codex, 1, &[]),
            Provider::Codex
        );
    }

    #[test]
    fn clone_provider_default_is_source_provider() {
        assert_eq!(
            clone_provider_at(clone_provider_default_index(Provider::Claude)),
            Provider::Claude
        );
        assert_eq!(
            clone_provider_at(clone_provider_default_index(Provider::Codex)),
            Provider::Codex
        );
        assert_eq!(
            clone_provider_at(clone_provider_default_index(Provider::OpenCode)),
            Provider::OpenCode
        );
    }

    #[test]
    fn clone_provider_selection_wraps() {
        let first = 0;
        let last = CLONE_PROVIDER_OPTIONS.len() - 1;

        assert_eq!(move_clone_provider_index(first, -1), last);
        assert_eq!(move_clone_provider_index(last, 1), first);
    }

    #[test]
    fn cokacmux_dark_theme_colors_are_indexed() {
        let colors = [
            THEME_BG,
            THEME_BG_ALT,
            THEME_STATUS_BG,
            THEME_FG,
            THEME_FG_DIM,
            THEME_FG_STRONG,
            THEME_SELECTED_BG,
            THEME_SELECTED_TEXT,
            THEME_ACCENT,
            THEME_SHORTCUT,
            THEME_POSITIVE,
            THEME_BORDER,
            THEME_BORDER_ACTIVE,
            THEME_PROVIDER_CLAUDE,
            THEME_PROVIDER_CODEX,
            THEME_PROVIDER_OPENCODE,
            AGENT_DEFAULT_BG,
        ];
        assert!(colors
            .iter()
            .all(|color| matches!(color, Color::Indexed(_))));
        assert_eq!(THEME_BG, Color::Indexed(234));
        assert_eq!(THEME_SELECTED_BG, Color::Indexed(66));
        assert_eq!(THEME_POSITIVE, Color::Indexed(108));
        assert_eq!(THEME_ACCENT, Color::Indexed(109));
    }

    #[test]
    fn vt100_rgb_colors_are_quantized_to_indexed_colors() {
        assert_eq!(vt100_color(vt100::Color::Default), THEME_FG);
        assert_eq!(
            vt100_color(vt100::Color::Rgb(255, 0, 0)),
            Color::Indexed(196)
        );
        assert_eq!(
            vt100_color(vt100::Color::Rgb(255, 255, 255)),
            Color::Indexed(231)
        );
        assert_eq!(
            vt100_bg_color(vt100::Color::Rgb(0, 0, 0)),
            Color::Indexed(16)
        );
    }

    #[test]
    fn blank_vt100_screen_has_no_visible_content() {
        let mut parser = vt100::Parser::new(5, 20, AGENT_SCROLLBACK_LINES);
        assert!(!screen_has_visible_content(parser.screen()));

        parser.process(b"\x1b[2J\x1b[H   ");
        assert!(!screen_has_visible_content(parser.screen()));

        parser.process(b"ready");
        assert!(screen_has_visible_content(parser.screen()));
    }

    #[test]
    fn terminal_response_answers_cursor_position_report() {
        let mut parser = vt100::Parser::new(24, 80, AGENT_SCROLLBACK_LINES);
        parser.process(b"\x1b[10;20H");

        assert_eq!(
            terminal_response_for_output(parser.screen(), b"\x1b[6n").unwrap(),
            b"\x1b[10;20R"
        );
    }

    #[test]
    fn terminal_response_answers_device_status_report() {
        let parser = vt100::Parser::new(24, 80, AGENT_SCROLLBACK_LINES);

        assert_eq!(
            terminal_response_for_output(parser.screen(), b"\x1b[5n").unwrap(),
            b"\x1b[0n"
        );
    }

    #[test]
    fn terminal_response_detects_split_cursor_position_report() {
        let parser = vt100::Parser::new(24, 80, AGENT_SCROLLBACK_LINES);

        assert_eq!(
            terminal_response_for_combined_output(parser.screen(), b"\x1b[6n", 3).unwrap(),
            b"\x1b[1;1R"
        );
    }

    #[test]
    fn startup_spinner_frame_rotates_by_tick() {
        assert_eq!(startup_spinner_frame(Duration::from_millis(0)), "|");
        assert_eq!(startup_spinner_frame(Duration::from_millis(180)), "/");
        assert_eq!(startup_spinner_frame(Duration::from_millis(360)), "-");
        assert_eq!(startup_spinner_frame(Duration::from_millis(540)), "\\");
        assert_eq!(startup_spinner_frame(Duration::from_millis(720)), "|");
    }

    #[cfg(windows)]
    #[test]
    fn windows_agent_tcp_marker_accepts_loopback_addr_only() {
        let marker_path = std::env::temp_dir().join(format!(
            "cokacmux-agent-marker-test-{}.tcp",
            std::process::id()
        ));
        fs::write(&marker_path, "tcp 127.0.0.1:49321\n").unwrap();
        assert_eq!(
            read_agent_tcp_addr(&marker_path).unwrap(),
            "127.0.0.1:49321"
        );

        fs::write(&marker_path, "tcp 0.0.0.0:49321\n").unwrap();
        assert_eq!(
            read_agent_tcp_addr(&marker_path).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        let _ = fs::remove_file(marker_path);
    }

    #[test]
    fn list_selection_movement_clamps_at_edges() {
        assert_eq!(clamped_selection_index(0, None, 1), None);
        assert_eq!(clamped_selection_index(5, Some(0), -1), Some(0));
        assert_eq!(clamped_selection_index(5, Some(4), 1), Some(4));
        assert_eq!(clamped_selection_index(5, Some(4), 10), Some(4));
        assert_eq!(clamped_selection_index(5, Some(0), -10), Some(0));
        assert_eq!(clamped_selection_index(5, Some(2), 1), Some(3));
        assert_eq!(clamped_selection_index(5, Some(2), -1), Some(1));
    }

    #[test]
    fn restore_visible_selection_keeps_matching_session_after_refresh() {
        let mut app = app_for_key_tests();
        app.session_view = SessionViewMode::List;
        app.sessions
            .push(session_info(Provider::Codex, "one", "/repo"));
        app.sessions
            .push(session_info(Provider::Claude, "two", "/repo"));
        app.list_state.select(Some(1));
        let selected_key = AgentKey::new(app.current().unwrap());

        app.sessions.reverse();
        app.list_state.select(Some(0));

        assert!(app.restore_visible_selection(&selected_key));
        assert_eq!(app.current().unwrap().session_id, "two");
    }

    #[test]
    fn delete_selection_stays_on_row_below_removed_item() {
        assert_eq!(selection_index_after_removed_row(0, Some(0)), None);
        assert_eq!(selection_index_after_removed_row(4, Some(1)), Some(1));
        assert_eq!(selection_index_after_removed_row(4, Some(3)), Some(3));
        assert_eq!(selection_index_after_removed_row(4, Some(4)), Some(3));
        assert_eq!(selection_index_after_removed_row(4, None), Some(0));
    }

    #[test]
    fn agent_candidate_index_wraps_or_clamps() {
        assert_eq!(next_agent_candidate_index(3, 0, -1, true), 2);
        assert_eq!(next_agent_candidate_index(3, 2, 1, true), 0);
        assert_eq!(next_agent_candidate_index(3, 0, -1, false), 0);
        assert_eq!(next_agent_candidate_index(3, 2, 1, false), 2);
        assert_eq!(next_agent_candidate_index(3, 1, 1, false), 2);
    }

    #[test]
    fn agent_terminal_width_accounts_for_sidebar() {
        assert_eq!(agent_sidebar_width(50, 0), 0);
        assert_eq!(agent_terminal_width(50, 0), 50);
        assert_eq!(
            agent_sidebar_width(50, DEFAULT_AGENT_SIDEBAR_WIDTH),
            DEFAULT_AGENT_SIDEBAR_WIDTH
        );
        assert_eq!(
            agent_terminal_width(50, DEFAULT_AGENT_SIDEBAR_WIDTH),
            50 - DEFAULT_AGENT_SIDEBAR_WIDTH
        );
        assert_eq!(
            agent_sidebar_width(120, DEFAULT_AGENT_SIDEBAR_WIDTH),
            DEFAULT_AGENT_SIDEBAR_WIDTH
        );
        assert_eq!(
            agent_terminal_width(120, DEFAULT_AGENT_SIDEBAR_WIDTH),
            120 - DEFAULT_AGENT_SIDEBAR_WIDTH
        );
        assert_eq!(agent_sidebar_width(70, u16::MAX), 70);
        assert_eq!(agent_terminal_width(70, u16::MAX), 0);
    }

    #[test]
    fn sessions_pane_width_uses_saved_columns_or_fallback_percent() {
        assert_eq!(
            sessions_pane_width(100, None, DEFAULT_SESSIONS_PANE_PERCENT),
            45
        );
        assert_eq!(sessions_pane_width(100, Some(40), 45), 40);
        assert_eq!(sessions_pane_width(70, Some(u16::MAX), 45), 70);
        assert_eq!(sessions_pane_width(100, None, 150), 100);
    }

    #[test]
    fn sessions_pane_resize_clamps_to_visible_width() {
        assert_eq!(
            adjusted_sessions_pane_width(Some(30), 100, 45, 2),
            (32, false)
        );
        assert_eq!(
            adjusted_sessions_pane_width(Some(0), 100, 45, -2),
            (0, true)
        );
        assert_eq!(
            adjusted_sessions_pane_width(Some(100), 100, 45, 2),
            (100, true)
        );
        assert_eq!(
            adjusted_sessions_pane_width(Some(u16::MAX), 100, 45, -2),
            (98, false)
        );
        assert_eq!(adjusted_sessions_pane_width(None, 100, 45, 2), (47, false));
    }

    #[test]
    fn agent_sidebar_resize_clamps_to_visible_width() {
        assert_eq!(adjusted_agent_sidebar_width(30, 100, 2), (32, false));
        assert_eq!(adjusted_agent_sidebar_width(0, 100, -2), (0, true));
        assert_eq!(adjusted_agent_sidebar_width(100, 100, 2), (100, true));
        assert_eq!(adjusted_agent_sidebar_width(u16::MAX, 100, 2), (100, true));
        assert_eq!(adjusted_agent_sidebar_width(u16::MAX, 100, -2), (98, false));
    }

    #[test]
    fn agent_screen_size_keeps_internal_pty_safe_for_narrow_panes() {
        assert_eq!(
            agent_screen_size_for_area(Rect::new(0, 0, 0, 0)),
            (AGENT_MIN_PTY_ROWS, AGENT_MIN_PTY_COLS)
        );
        assert_eq!(
            agent_screen_size_for_area(Rect::new(0, 0, 0, 20)),
            (20, AGENT_MIN_PTY_COLS)
        );
        assert_eq!(
            agent_screen_size_for_area(Rect::new(0, 0, 80, 20)),
            (20, 80)
        );
        let pty_size = agent_pty_size(1, 1);
        assert_eq!(pty_size.cols, AGENT_MIN_PTY_COLS);
        assert_eq!(pty_size.rows, AGENT_MIN_PTY_ROWS);
    }

    #[test]
    fn list_header_includes_agent_state_column() {
        let cols = list_columns(120);
        let header = list_header(&cols);
        assert!(header.contains("state"));
        assert!(header.contains("provider"));
        assert!(header.contains("session"));
    }

    #[test]
    fn list_columns_fill_visible_table_width() {
        for width in 20..180 {
            let cols = list_columns(width);
            assert_eq!(cols.row_width(), width as usize, "width={}", width);
        }
    }

    #[test]
    fn list_columns_expand_variable_fields_with_available_width() {
        let compact = list_columns(70);
        let wide = list_columns(140);

        assert!(compact.title > 0);
        assert!(compact.cwd > 0);
        assert!(wide.id >= compact.id);
        assert!(wide.title >= compact.title);
        assert!(wide.cwd >= compact.cwd);
        assert_eq!(wide.row_width(), 140);
    }

    #[test]
    fn agent_meta_state_distinguishes_live_attached_and_mine() {
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join("agent.json");
        let socket = dir.path().join("agent.sock");
        let current_pid = std::process::id();

        fs::write(&meta, format!(r#"{{"pid":{}}}"#, current_pid)).unwrap();
        assert_eq!(
            read_agent_runtime_state_at(&meta, &socket, current_pid),
            AgentListState::Live {
                activity: AgentActivity::Quiet
            }
        );

        #[cfg(unix)]
        let mut foreign_client = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let mut foreign_client = Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .unwrap();
        fs::write(
            &meta,
            format!(
                r#"{{"pid":{},"attached":true,"attached_client_pid":{}}}"#,
                current_pid,
                foreign_client.id()
            ),
        )
        .unwrap();
        let foreign_state = read_agent_runtime_state_at(&meta, &socket, current_pid);
        let _ = foreign_client.kill();
        let _ = foreign_client.wait();
        assert_eq!(
            foreign_state,
            AgentListState::Attached {
                mine: false,
                activity: AgentActivity::Quiet
            }
        );

        fs::write(
            &meta,
            format!(
                r#"{{"pid":{},"attached":true,"attached_client_pid":{}}}"#,
                current_pid, current_pid
            ),
        )
        .unwrap();
        assert_eq!(
            read_agent_runtime_state_at(&meta, &socket, current_pid),
            AgentListState::Attached {
                mine: true,
                activity: AgentActivity::Quiet
            }
        );
    }

    #[test]
    fn agent_switchable_state_excludes_foreign_attached_sessions() {
        assert!(!is_switchable_agent_state(AgentListState::Idle));
        assert!(is_switchable_agent_state(AgentListState::Live {
            activity: AgentActivity::Busy
        }));
        assert!(is_switchable_agent_state(AgentListState::Attached {
            mine: true,
            activity: AgentActivity::Quiet
        }));
        assert!(!is_switchable_agent_state(AgentListState::Attached {
            mine: false,
            activity: AgentActivity::Busy
        }));
    }

    #[test]
    fn stale_attached_client_meta_is_listed_live() {
        let current_pid = std::process::id();
        let meta = AgentMetaSnapshot {
            pid: current_pid,
            child_pid: None,
            provider: Some("claude".into()),
            session_id: Some("s1".into()),
            cwd: Some("/repo".into()),
            source: None,
            attached: true,
            attached_client_pid: Some(u32::MAX),
            last_screen_change_epoch_ms: 0,
            last_output_epoch_ms: 0,
            last_input_epoch_ms: 0,
        };

        assert_eq!(
            meta.list_state(current_pid, current_epoch_ms()),
            AgentListState::Live {
                activity: AgentActivity::Quiet
            }
        );
    }

    #[test]
    fn restore_candidate_prefers_selected_live_session() {
        let mut app = app_for_key_tests();
        let first = session_info(Provider::Claude, "s1", "/repo1");
        let selected = session_info(Provider::Codex, "s2", "/repo2");
        app.sessions.push(first.clone());
        app.sessions.push(selected.clone());
        app.agent_states.insert(
            AgentKey::new(&first),
            AgentListState::Live {
                activity: AgentActivity::Quiet,
            },
        );
        app.agent_states.insert(
            AgentKey::new(&selected),
            AgentListState::Live {
                activity: AgentActivity::Busy,
            },
        );
        app.list_state.select(Some(1));

        let candidate = app.live_agent_restore_candidate().unwrap();
        assert_eq!(candidate.provider, Provider::Codex);
        assert_eq!(candidate.session_id, "s2");
    }

    #[test]
    fn restore_candidate_can_use_live_shell_without_visible_session() {
        let mut app = app_for_key_tests();
        let shell = SessionInfo {
            provider: Provider::Claude,
            session_id: "shell-test".into(),
            cwd: "/repo".into(),
            source: PathBuf::from(SHELL_SESSION_SOURCE_MARKER),
            updated_at_epoch_s: 0,
            title: Some("shell @ repo".into()),
        };
        app.live_shells.push(shell.clone());
        app.agent_states.insert(
            AgentKey::new(&shell),
            AgentListState::Live {
                activity: AgentActivity::Quiet,
            },
        );

        let candidate = app.live_agent_restore_candidate().unwrap();
        assert!(is_shell_session_info(&candidate));
        assert_eq!(candidate.session_id, "shell-test");
    }

    #[test]
    fn new_agent_state_is_not_inferred_for_stored_session_by_cwd() {
        let mut app = app_for_key_tests();
        let latest = session_info(Provider::Codex, "latest-real", "/repo");
        let fresh = new_agent_info(Provider::Codex, "/repo");
        app.sessions.push(latest.clone());
        app.live_shells.push(fresh.clone());
        app.agent_states.insert(
            AgentKey::new(&fresh),
            AgentListState::Attached {
                mine: true,
                activity: AgentActivity::Busy,
            },
        );

        assert_eq!(app.agent_state_for(&latest), AgentListState::Idle);
        assert_eq!(
            app.agent_state_for(&fresh),
            AgentListState::Attached {
                mine: true,
                activity: AgentActivity::Busy
            }
        );
    }

    #[test]
    fn selected_backing_session_restores_synthetic_new_agent() {
        let mut app = app_for_key_tests();
        let backing = session_info(Provider::Codex, "real-session", "/repo");
        let fresh = new_agent_info(Provider::Codex, "/repo");
        let backing_key = AgentKey::new(&backing);
        let fresh_key = AgentKey::new(&fresh);
        app.sessions.push(backing.clone());
        app.live_shells.push(fresh.clone());
        app.agent_states.insert(
            backing_key.clone(),
            AgentListState::Live {
                activity: AgentActivity::Quiet,
            },
        );
        app.agent_states.insert(
            fresh_key.clone(),
            AgentListState::Live {
                activity: AgentActivity::Quiet,
            },
        );
        app.new_agent_backing_aliases
            .insert(fresh_key.clone(), backing_key);
        app.list_state.select(Some(0));

        let candidate = app.live_agent_restore_candidate().unwrap();
        assert!(is_new_agent_session_info(&candidate));
        assert_eq!(candidate.session_id, fresh.session_id);
        assert_eq!(
            AgentKey::new(&app.runtime_info_for_selected_agent(&backing)),
            fresh_key
        );
    }

    #[test]
    fn codex_rollout_path_requires_matching_session_meta() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = "019e6917-024a-7282-b37f-6ff8317bf8ca";
        let rollout = dir
            .path()
            .join(format!("rollout-2026-05-27T10-59-36-{session_id}.jsonl"));
        fs::write(
            &rollout,
            format!(
                r#"{{"timestamp":"2026-05-27T10:59:36.139Z","type":"session_meta","payload":{{"id":"{session_id}","cwd":"/repo","originator":"codex-tui"}}}}"#
            ),
        )
        .unwrap();

        assert_eq!(
            codex_session_id_from_rollout_path(&rollout).as_deref(),
            Some(session_id)
        );
        assert!(codex_rollout_session_meta_matches(
            &rollout,
            session_id,
            Some("/repo")
        ));
        assert!(!codex_rollout_session_meta_matches(
            &rollout,
            session_id,
            Some("/other")
        ));

        let windows_rollout = dir
            .path()
            .join(format!("rollout-2026-05-27T11-00-00-{session_id}.jsonl"));
        fs::write(
            &windows_rollout,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{session_id}","cwd":"\\\\?\\C:\\Projects\\repo","originator":"codex-tui"}}}}"#
            ),
        )
        .unwrap();
        assert!(codex_rollout_session_meta_matches(
            &windows_rollout,
            session_id,
            Some("c:/projects/repo")
        ));
    }

    #[test]
    fn session_cwd_eq_normalizes_windows_extended_prefix() {
        assert!(session_cwd_eq(
            "\\\\?\\C:\\Projects\\repo",
            "C:\\PROJECTS\\repo"
        ));
        assert!(session_cwd_eq(
            "\\\\?\\C:\\Projects\\repo\\",
            "C:/Projects/repo"
        ));
        assert!(session_cwd_eq(
            "\\\\?\\UNC\\SERVER\\Share\\project",
            "\\\\server\\share\\project"
        ));
        assert!(!session_cwd_eq("/Repo", "/repo"));
        assert!(!session_cwd_eq("//Repo", "//repo"));
        assert!(!session_cwd_eq("C:\\Projects\\repo ", "C:\\Projects\\repo"));
    }

    #[test]
    fn agent_activity_uses_recent_screen_output_or_input_timestamps() {
        let now = 10_000;
        assert_eq!(
            agent_activity_from_timestamps(now, now - 1_000, 0, 0),
            AgentActivity::Busy
        );
        assert_eq!(
            agent_activity_from_timestamps(now, 0, now - 1_000, 0),
            AgentActivity::Busy
        );
        assert_eq!(
            agent_activity_from_timestamps(now, 0, 0, now - 1_000),
            AgentActivity::Busy
        );
        assert_eq!(
            agent_activity_from_timestamps(now, now - AGENT_BUSY_GRACE_MS - 1, 0, 0),
            AgentActivity::Quiet
        );
        assert_eq!(
            agent_activity_from_timestamps(now, 0, 0, 0),
            AgentActivity::Quiet
        );
    }

    #[test]
    fn stale_agent_meta_is_cleaned_and_treated_as_idle() {
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join("agent.json");
        let socket = dir.path().join("agent.sock");

        fs::write(&meta, r#"{"pid":3000000000,"attached":true}"#).unwrap();
        fs::write(&socket, "").unwrap();

        assert_eq!(
            read_agent_runtime_state_at(&meta, &socket, std::process::id()),
            AgentListState::Idle
        );
        assert!(!meta.exists());
        assert!(!socket.exists());
    }

    #[test]
    fn agent_launch_specs_match_provider_resume_commands() {
        let codex = default_agent_launch_spec(
            &session_info(Provider::Codex, "codex-id", "/repo"),
            AgentLaunchMode::Normal,
        );
        assert_eq!(codex.program, "codex");
        assert_eq!(codex.args, vec!["resume", "-C", "/repo", "codex-id"]);
        assert!(codex.env.is_empty());

        let claude = default_agent_launch_spec(
            &session_info(Provider::Claude, "claude-id", "/repo"),
            AgentLaunchMode::Normal,
        );
        assert_eq!(claude.program, "claude");
        assert_eq!(claude.args, vec!["--resume", "claude-id"]);
        assert!(claude.env.is_empty());
        assert_eq!(claude.cwd, Some(PathBuf::from("/repo")));

        let opencode = default_agent_launch_spec(
            &session_info(Provider::OpenCode, "opencode-id", "/repo"),
            AgentLaunchMode::Normal,
        );
        assert_eq!(opencode.program, "opencode");
        assert_eq!(opencode.args, vec!["/repo", "--session", "opencode-id"]);
        assert!(opencode.env.is_empty());
    }

    #[test]
    fn agent_launch_specs_use_configured_program_paths_when_present() {
        let agent_programs = AgentProgramSettings {
            codex: Some("/custom/bin/codex".into()),
            claude: Some("/custom/bin/claude".into()),
            opencode: Some("/custom/bin/opencode".into()),
            extra: serde_json::Map::new(),
        };

        let codex = agent_launch_spec_with_programs(
            &session_info(Provider::Codex, "codex-id", "/repo"),
            AgentLaunchMode::Normal,
            &agent_programs,
        );
        assert_eq!(codex.program, "/custom/bin/codex");
        assert_eq!(codex.args, vec!["resume", "-C", "/repo", "codex-id"]);

        let claude = agent_launch_spec_with_programs(
            &session_info(Provider::Claude, "claude-id", "/repo"),
            AgentLaunchMode::Normal,
            &agent_programs,
        );
        assert_eq!(claude.program, "/custom/bin/claude");
        assert_eq!(claude.args, vec!["--resume", "claude-id"]);

        let opencode = agent_launch_spec_with_programs(
            &session_info(Provider::OpenCode, "opencode-id", "/repo"),
            AgentLaunchMode::Normal,
            &agent_programs,
        );
        assert_eq!(opencode.program, "/custom/bin/opencode");
        assert_eq!(opencode.args, vec!["/repo", "--session", "opencode-id"]);

        let fresh_codex = new_agent_launch_spec_with_programs(
            &new_agent_info(Provider::Codex, "/repo"),
            AgentLaunchMode::SkipPermissions,
            &agent_programs,
        );
        assert_eq!(fresh_codex.program, "/custom/bin/codex");
        assert_eq!(fresh_codex.args, vec!["--yolo", "-C", "/repo"]);
    }

    #[test]
    fn blank_agent_program_settings_fall_back_to_path_lookup_names() {
        let agent_programs = AgentProgramSettings {
            codex: Some("   ".into()),
            claude: None,
            opencode: None,
            extra: serde_json::Map::new(),
        };

        let codex = agent_launch_spec_with_programs(
            &session_info(Provider::Codex, "codex-id", "/repo"),
            AgentLaunchMode::Normal,
            &agent_programs,
        );

        assert_eq!(codex.program, "codex");
        assert!(agent_programs.is_empty());
    }

    #[test]
    fn configured_agent_program_paths_expand_home_prefix() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let agent_programs = AgentProgramSettings {
            codex: None,
            claude: Some("~/bin/claude".into()),
            opencode: None,
            extra: serde_json::Map::new(),
        };

        assert_eq!(
            agent_programs.program_for(Provider::Claude),
            home.join("bin/claude").display().to_string()
        );
    }

    #[test]
    fn skip_permissions_launch_specs_add_provider_specific_bypass() {
        let codex = default_agent_launch_spec(
            &session_info(Provider::Codex, "codex-id", "/repo"),
            AgentLaunchMode::SkipPermissions,
        );
        assert_eq!(
            codex.args,
            vec!["--yolo", "resume", "-C", "/repo", "codex-id"]
        );
        assert!(codex.env.is_empty());

        let claude = default_agent_launch_spec(
            &session_info(Provider::Claude, "claude-id", "/repo"),
            AgentLaunchMode::SkipPermissions,
        );
        assert_eq!(
            claude.args,
            vec!["--dangerously-skip-permissions", "--resume", "claude-id"]
        );
        assert!(claude.env.is_empty());

        let opencode = default_agent_launch_spec(
            &session_info(Provider::OpenCode, "opencode-id", "/repo"),
            AgentLaunchMode::SkipPermissions,
        );
        assert_eq!(opencode.args, vec!["/repo", "--session", "opencode-id"]);
        assert_eq!(
            opencode.env,
            vec![("OPENCODE_PERMISSION".into(), r#"{"*":"allow"}"#.into())]
        );
    }

    #[test]
    fn fresh_agent_launch_specs_start_without_resume_session_args() {
        let codex = default_agent_launch_spec(
            &new_agent_info(Provider::Codex, "/repo"),
            AgentLaunchMode::Normal,
        );
        assert_eq!(codex.program, "codex");
        assert_eq!(codex.args, vec!["-C", "/repo"]);
        assert!(codex.env.is_empty());
        assert_eq!(codex.cwd, Some(PathBuf::from("/repo")));

        let codex_skip = default_agent_launch_spec(
            &new_agent_info(Provider::Codex, "/repo"),
            AgentLaunchMode::SkipPermissions,
        );
        assert_eq!(codex_skip.args, vec!["--yolo", "-C", "/repo"]);

        let claude = default_agent_launch_spec(
            &new_agent_info(Provider::Claude, "/repo"),
            AgentLaunchMode::Normal,
        );
        assert_eq!(claude.program, "claude");
        assert!(claude.args.is_empty());
        assert_eq!(claude.cwd, Some(PathBuf::from("/repo")));

        let claude_skip = default_agent_launch_spec(
            &new_agent_info(Provider::Claude, "/repo"),
            AgentLaunchMode::SkipPermissions,
        );
        assert_eq!(claude_skip.args, vec!["--dangerously-skip-permissions"]);

        let opencode = default_agent_launch_spec(
            &new_agent_info(Provider::OpenCode, "/repo"),
            AgentLaunchMode::Normal,
        );
        assert_eq!(opencode.program, "opencode");
        assert_eq!(opencode.args, vec!["/repo"]);
        assert!(opencode.env.is_empty());

        let opencode_skip = default_agent_launch_spec(
            &new_agent_info(Provider::OpenCode, "/repo"),
            AgentLaunchMode::SkipPermissions,
        );
        assert_eq!(opencode_skip.args, vec!["/repo"]);
        assert_eq!(
            opencode_skip.env,
            vec![("OPENCODE_PERMISSION".into(), r#"{"*":"allow"}"#.into())]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_agent_program_resolution_prefers_cmd_over_extensionless_shim() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("codex"), "#!/bin/sh\n").unwrap();
        fs::write(dir.path().join("codex.cmd"), "@echo off\r\n").unwrap();

        let resolved = resolve_windows_agent_program_with_env(
            "codex",
            Some(dir.path().as_os_str().to_os_string()),
            Some(OsString::from(".COM;.EXE;.BAT;.CMD")),
        )
        .unwrap();

        assert_eq!(resolved, dir.path().join("codex.cmd"));
        assert!(is_windows_batch_script(&resolved));
    }

    #[cfg(windows)]
    #[test]
    fn windows_batch_agent_command_runs_through_cmd_call() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("codex.cmd");
        fs::write(&shim, "@echo off\r\n").unwrap();

        let argv = windows_agent_command_argv(shim.clone(), &["resume".into(), "sid".into()]);

        assert_eq!(argv[0], windows_comspec());
        assert_eq!(argv[1], OsString::from("/D"));
        assert_eq!(argv[2], OsString::from("/C"));
        assert_eq!(argv[3], shim.into_os_string());
        assert_eq!(argv[4], OsString::from("resume"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_npm_cmd_shim_runs_node_script_directly() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir
            .path()
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "console.log('codex')\n").unwrap();
        let node = dir.path().join("node.exe");
        fs::write(&node, "").unwrap();
        let shim = dir.path().join("codex.cmd");
        fs::write(
            &shim,
            r#"@ECHO off
IF EXIST "%~dp0\node.exe" (
  SET "_prog=%~dp0\node.exe"
) ELSE (
  SET "_prog=node"
)
"%_prog%" "%~dp0\node_modules\@openai\codex\bin\codex.js" %*
"#,
        )
        .unwrap();

        let argv = windows_agent_command_argv(shim, &["resume".into(), "sid".into()]);

        assert_eq!(argv[0], node.into_os_string());
        assert_eq!(argv[1], script.into_os_string());
        assert_eq!(argv[2], OsString::from("resume"));
    }

    #[test]
    fn screen_toggle_key_accepts_encoded_control_forms() {
        assert!(is_screen_toggle_key(KeyEvent::new(
            KeyCode::Char(']'),
            KeyModifiers::CONTROL
        )));
        assert!(is_screen_toggle_key(KeyEvent::new(
            KeyCode::Char('['),
            KeyModifiers::CONTROL
        )));
        assert!(is_screen_toggle_key(KeyEvent::new(
            KeyCode::Char('3'),
            KeyModifiers::CONTROL
        )));
        assert!(is_screen_toggle_key(KeyEvent::new(
            KeyCode::Char('\u{1d}'),
            KeyModifiers::NONE
        )));
        assert!(is_screen_toggle_key(KeyEvent::new(
            KeyCode::Char('5'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_screen_toggle_key(KeyEvent::new(
            KeyCode::Char(']'),
            KeyModifiers::NONE
        )));
        assert!(!is_screen_toggle_key(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn session_toggle_key_matches_screen_toggle_key() {
        assert!(is_session_toggle_key(KeyEvent::new(
            KeyCode::Char(']'),
            KeyModifiers::CONTROL
        )));
        assert!(is_session_toggle_key(KeyEvent::new(
            KeyCode::Char('['),
            KeyModifiers::CONTROL
        )));
        assert!(is_session_toggle_key(KeyEvent::new(
            KeyCode::Char('3'),
            KeyModifiers::CONTROL
        )));
        assert!(is_session_toggle_key(KeyEvent::new(
            KeyCode::Char('\u{1d}'),
            KeyModifiers::NONE
        )));
        assert!(is_session_toggle_key(KeyEvent::new(
            KeyCode::Char('5'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_session_toggle_key(KeyEvent::new(
            KeyCode::Char('e'),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn agent_kill_key_accepts_ctrl_k_only() {
        assert!(is_agent_kill_key(KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::CONTROL
        )));
        assert!(is_agent_kill_key(KeyEvent::new(
            KeyCode::Char('K'),
            KeyModifiers::CONTROL
        )));
        assert!(is_agent_kill_key(KeyEvent::new(
            KeyCode::Char('\u{b}'),
            KeyModifiers::NONE
        )));
        assert!(!is_agent_kill_key(KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn global_quit_key_accepts_ctrl_q_only() {
        assert!(is_global_quit_key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL
        )));
        assert!(is_global_quit_key(KeyEvent::new(
            KeyCode::Char('Q'),
            KeyModifiers::CONTROL
        )));
        assert!(is_global_quit_key(KeyEvent::new(
            KeyCode::Char('\u{11}'),
            KeyModifiers::NONE
        )));
        assert!(is_global_quit_key(KeyEvent::new(
            KeyCode::Char('\u{11}'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_global_quit_key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn ctrl_q_quits_from_session_input_modes() {
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);

        let mut app = app_for_key_tests();
        app.input_mode = InputMode::Filter {
            draft: "query".to_string(),
            cursor: "query".len(),
        };
        handle_key(&mut app, ctrl_q, 80, 50, 20);
        assert!(app.should_quit);

        let mut app = app_for_key_tests();
        app.input_mode = InputMode::Confirm {
            prompt: "Delete?".to_string(),
            action: PendingAction::Delete {
                info: session_info(Provider::Codex, "codex-id", "/repo"),
                removed_index: Some(0),
            },
        };
        handle_key(&mut app, ctrl_q, 80, 50, 20);
        assert!(app.should_quit);

        let mut app = app_for_key_tests();
        app.input_mode = InputMode::AgentLaunch {
            selected: 0,
            source: session_info(Provider::Codex, "codex-id", "/repo"),
        };
        handle_key(&mut app, ctrl_q, 80, 50, 20);
        assert!(app.should_quit);

        let mut app = app_for_key_tests();
        app.input_mode = InputMode::CloneTarget {
            selected: 0,
            source: session_info(Provider::Codex, "codex-id", "/repo"),
        };
        handle_key(&mut app, ctrl_q, 80, 50, 20);
        assert!(app.should_quit);

        let mut app = app_for_key_tests();
        app.input_mode = InputMode::TitleEdit {
            source: session_info(Provider::Codex, "codex-id", "/repo"),
            draft: "Title".to_string(),
            cursor: "Title".len(),
        };
        handle_key(&mut app, ctrl_q, 80, 50, 20);
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_q_quits_from_agent_key_handler() {
        let mut app = app_for_key_tests();
        handle_agent_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            80,
        );
        assert!(app.should_quit);
    }

    #[test]
    fn agent_scrollback_keys_accept_dedicated_navigation() {
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)),
            Some(AgentScrollAction::Lines(1))
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)),
            Some(AgentScrollAction::Lines(-1))
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(
                KeyCode::Up,
                KeyModifiers::SHIFT | KeyModifiers::ALT
            )),
            Some(AgentScrollAction::Pages(1))
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(
                KeyCode::Down,
                KeyModifiers::SHIFT | KeyModifiers::ALT
            )),
            Some(AgentScrollAction::Pages(-1))
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(
                KeyCode::PageUp,
                KeyModifiers::SHIFT | KeyModifiers::ALT
            )),
            Some(AgentScrollAction::Pages(1))
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(
                KeyCode::PageDown,
                KeyModifiers::SHIFT | KeyModifiers::ALT
            )),
            Some(AgentScrollAction::Pages(-1))
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(KeyCode::Home, KeyModifiers::ALT)),
            Some(AgentScrollAction::Top)
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT)),
            Some(AgentScrollAction::Bottom)
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::ALT)),
            None
        );
        assert_eq!(
            is_agent_scrollback_key(KeyEvent::new(
                KeyCode::Up,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            None
        );
    }

    #[test]
    fn parser_snapshot_rehydrates_scrollback_lines() {
        let mut source = vt100::Parser::new(5, 20, AGENT_SCROLLBACK_LINES);
        for line in 1..=12 {
            let bytes = format!("COKSCROLL{:03}\r\n", line);
            safe_parser_process(&mut source, bytes.as_bytes());
        }
        source.screen_mut().set_scrollback(usize::MAX);
        assert!(
            source.screen().scrollback() > 0,
            "source parser should have scrollback"
        );
        source.screen_mut().set_scrollback(0);

        let snapshot = parser_snapshot_bytes(&mut source, true);
        assert_eq!(
            source.screen().scrollback(),
            0,
            "snapshot generation must restore the daemon parser view"
        );

        let mut restored = vt100::Parser::new(5, 20, AGENT_SCROLLBACK_LINES);
        safe_parser_process(&mut restored, &snapshot);
        restored.screen_mut().set_scrollback(usize::MAX);
        let restored_scrollback = restored.screen().scrollback();
        let restored_lines = parser_visible_plain_lines(&mut restored, restored_scrollback);
        assert!(
            restored_lines
                .iter()
                .any(|line| line.contains("COKSCROLL001")),
            "oldest scrollback line should be visible after restoring snapshot: {:?}",
            restored_lines
        );
    }

    #[test]
    fn persistent_pty_log_replays_into_scrollback() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        fs::write(
            &path,
            (1..=12)
                .map(|line| format!("PERSIST{:03}\r\n", line))
                .collect::<String>(),
        )
        .unwrap();

        let info = session_info(Provider::Claude, "persistent-scroll", "/repo");
        let mut parser = vt100::Parser::new(5, 20, AGENT_SCROLLBACK_LINES);
        replay_agent_pty_log(&mut parser, &path, &info);
        parser.screen_mut().set_scrollback(usize::MAX);
        let restored_scrollback = parser.screen().scrollback();
        let restored_lines = parser_visible_plain_lines(&mut parser, restored_scrollback);
        assert!(
            restored_lines
                .iter()
                .any(|line| line.contains("PERSIST001")),
            "persistent PTY log should replay old lines into scrollback: {:?}",
            restored_lines
        );
    }

    #[test]
    fn parser_snapshot_uses_screen_history_when_vt100_scrollback_is_empty() {
        let mut source = vt100::Parser::new(5, 30, AGENT_SCROLLBACK_LINES);
        let mut history = ScreenHistory::default();
        for frame in 1..=8 {
            let bytes = format!("\x1b[?1049h\x1b[H\x1b[2JFRAME{:03}\r\n", frame);
            safe_parser_process(&mut source, bytes.as_bytes());
            history.capture(&mut source);
        }
        assert_eq!(
            parser_max_scrollback(&mut source),
            0,
            "alternate-screen redraws should not create vt100 scrollback"
        );
        assert!(
            history.max_scroll_offset(5) > 0,
            "screen history should preserve redraw snapshots"
        );

        let snapshot = parser_snapshot_bytes_with_history(&mut source, true, &history);
        let mut restored = vt100::Parser::new(5, 30, AGENT_SCROLLBACK_LINES);
        safe_parser_process(&mut restored, &snapshot);
        let restored_text = parser_snapshot_text(&mut restored);
        assert!(
            restored_text.contains("FRAME001") && restored_text.contains("FRAME008"),
            "snapshot should transfer screen history fallback: {:?}",
            restored_text
        );
    }

    #[test]
    fn screen_history_captures_multiple_fullscreen_frames_in_one_chunk() {
        let mut source = vt100::Parser::new(5, 30, AGENT_SCROLLBACK_LINES);
        let mut history = ScreenHistory::default();
        let mut screen_hash = screen_activity_hash(source.screen());
        let mut bytes = Vec::new();
        for frame in 1..=8 {
            bytes.extend_from_slice(
                format!("\x1b[?1049h\x1b[H\x1b[2JFRAME{frame:03}\r\n").as_bytes(),
            );
        }

        assert!(process_parser_output(
            &mut source,
            &bytes,
            &mut screen_hash,
            Some(&mut history),
        ));

        let history_text = history.all_lines().join("\n");
        assert!(
            history_text.contains("FRAME001") && history_text.contains("FRAME008"),
            "history should retain intermediate redraw frames: {:?}",
            history_text
        );
        assert_eq!(
            parser_max_scrollback(&mut source),
            0,
            "test setup must exercise screen-history fallback"
        );
    }

    #[test]
    fn parser_snapshot_history_fallback_does_not_duplicate_current_frame() {
        let mut source = vt100::Parser::new(5, 30, AGENT_SCROLLBACK_LINES);
        let mut history = ScreenHistory::default();
        let mut screen_hash = screen_activity_hash(source.screen());
        for frame in 1..=3 {
            let bytes = format!("\x1b[?1049h\x1b[H\x1b[2JFRAME{frame:03}\r\n");
            let _ = process_parser_output(
                &mut source,
                bytes.as_bytes(),
                &mut screen_hash,
                Some(&mut history),
            );
        }

        let snapshot = parser_snapshot_bytes_with_history(&mut source, true, &history);
        let snapshot_text = String::from_utf8_lossy(&snapshot);
        assert_eq!(
            snapshot_text.matches("FRAME003").count(),
            1,
            "current fullscreen frame should not be duplicated in fallback snapshot: {:?}",
            snapshot_text
        );
    }

    #[cfg(unix)]
    #[test]
    fn agent_client_scrolls_screen_history_when_parser_scrollback_is_empty() {
        let (stream, _peer) = AgentStream::pair().unwrap();
        let pty_size = agent_pty_size(40, 6);
        let parser = vt100::Parser::new(pty_size.rows, pty_size.cols, AGENT_SCROLLBACK_LINES);
        let mut client = AgentClient {
            info: session_info(Provider::Claude, "history-scroll", "/repo"),
            command_line: "test".into(),
            parser,
            screen_history: ScreenHistory::default(),
            history_scroll_offset: 0,
            stream,
            pty_size,
            exited: Some("test".into()),
            screen_hash: 0,
            last_screen_change_epoch_ms: 0,
            last_output_epoch_ms: 0,
            last_input_epoch_ms: 0,
            pending_snapshot_output: false,
            startup_spinner_started_at: None,
            debug_output_events: 0,
            reader_id: 0,
            reader_thread: None,
        };
        client.screen_hash = screen_activity_hash(client.parser.screen());

        for frame in 1..=8 {
            let bytes = format!("\x1b[?1049h\x1b[H\x1b[2JFRAME{:03}\r\n", frame);
            client.process_agent_output(bytes.as_bytes(), true);
        }

        assert_eq!(
            parser_max_scrollback(&mut client.parser),
            0,
            "test setup must exercise the screen-history path"
        );
        let after = client.scroll_screen(AgentScrollAction::Pages(1), 5);
        assert!(
            after > 0,
            "page-up should use screen history when vt100 scrollback is empty"
        );
        assert_eq!(client.scrollback_offset(), after);

        let lines = client.screen_history.visible_lines(after, 5);
        assert!(
            lines.iter().any(|line| line.contains("FRAME")),
            "history render source should contain captured fullscreen frames: {:?}",
            lines
        );
    }

    #[cfg(unix)]
    #[test]
    fn live_pty_output_round_trips_through_scrollback_log_and_snapshot() {
        let shell = PathBuf::from("/bin/sh");
        if !shell.is_file() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let session_id = format!("pty-e2e-{}", uuid::Uuid::now_v7());
        let mut info = session_info(
            Provider::Claude,
            &session_id,
            &dir.path().display().to_string(),
        );
        info.source = PathBuf::from(SHELL_SESSION_SOURCE_MARKER);

        let key = AgentKey::new(&info);
        let pty_log_path = agent_pty_log_path(&key).unwrap();
        let _ = remove_agent_pty_log(&key);
        let spec = AgentLaunchSpec {
            program: shell.display().to_string(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: Some(dir.path().to_path_buf()),
        };

        let mut agent =
            AgentSession::spawn_with_spec(info.clone(), spec, 40, 6, AgentLaunchMode::Normal)
                .unwrap();
        agent.send_bytes(
            b"i=1; while [ \"$i\" -le 40 ]; do printf 'COKPTY%03d\\n' \"$i\"; i=$((i + 1)); done\n",
        );
        assert!(
            drain_agent_until(&mut agent, Duration::from_secs(5), |agent| {
                parser_snapshot_text(&mut agent.parser).contains("COKPTY040")
            }),
            "PTY child output did not reach the parser"
        );

        if let Some(file) = agent.pty_log.as_mut() {
            file.flush().unwrap();
        }
        let log_bytes = fs::read(&pty_log_path).unwrap();
        let log_text = String::from_utf8_lossy(&log_bytes);
        assert!(
            log_text.contains("COKPTY001"),
            "ptylog lost earliest output"
        );
        assert!(log_text.contains("COKPTY040"), "ptylog lost latest output");

        let snapshot_text = parser_snapshot_text(&mut agent.parser);
        assert!(
            snapshot_text.contains("COKPTY001") && snapshot_text.contains("COKPTY040"),
            "live parser snapshot did not preserve full scrollback: {:?}",
            snapshot_text
        );

        agent.parser.screen_mut().set_scrollback(usize::MAX);
        let top_offset = agent.parser.screen().scrollback();
        assert!(top_offset > 0, "PTY output should create scrollback");
        let top_lines = parser_visible_plain_lines(&mut agent.parser, top_offset);
        assert!(
            top_lines.iter().any(|line| line.contains("COKPTY001")),
            "scrolling to top did not reveal earliest output: {:?}",
            top_lines
        );
        agent.parser.screen_mut().set_scrollback(0);
        assert_eq!(agent.parser.screen().scrollback(), 0);

        agent.parser = vt100::Parser::new(
            agent.pty_size.rows,
            agent.pty_size.cols,
            AGENT_SCROLLBACK_LINES,
        );
        agent.screen_history = ScreenHistory::default();
        agent.screen_hash = screen_activity_hash(agent.parser.screen());
        agent.rehydrate_parser_from_pty_log();
        let replay_text = parser_snapshot_text(&mut agent.parser);
        assert!(
            replay_text.contains("COKPTY001") && replay_text.contains("COKPTY040"),
            "ptylog replay did not restore full scrollback: {:?}",
            replay_text
        );

        let snapshot = agent.screen_snapshot_bytes(true);
        let mut restored = vt100::Parser::new(
            agent.pty_size.rows,
            agent.pty_size.cols,
            AGENT_SCROLLBACK_LINES,
        );
        safe_parser_process(&mut restored, &snapshot);
        let restored_text = parser_snapshot_text(&mut restored);
        assert!(
            restored_text.contains("COKPTY001") && restored_text.contains("COKPTY040"),
            "snapshot event did not transfer full scrollback: {:?}",
            restored_text
        );

        agent.send_bytes(b"exit\n");
        let _ = drain_agent_until(&mut agent, Duration::from_millis(500), |_| false);
        let _ = agent.child.kill();
        let _ = agent.child.wait();
        drop(agent.pty_log.take());
        let _ = remove_agent_pty_log(&key);
    }

    #[cfg(unix)]
    #[test]
    fn live_pty_fullscreen_redraws_scroll_through_screen_history() {
        let shell = PathBuf::from("/bin/sh");
        if !shell.is_file() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let session_id = format!("pty-fullscreen-{}", uuid::Uuid::now_v7());
        let mut info = session_info(
            Provider::Claude,
            &session_id,
            &dir.path().display().to_string(),
        );
        info.source = PathBuf::from(SHELL_SESSION_SOURCE_MARKER);

        let key = AgentKey::new(&info);
        let _ = remove_agent_pty_log(&key);
        let spec = AgentLaunchSpec {
            program: shell.display().to_string(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: Some(dir.path().to_path_buf()),
        };

        let mut agent =
            AgentSession::spawn_with_spec(info.clone(), spec, 40, 6, AgentLaunchMode::Normal)
                .unwrap();
        agent.send_bytes(b"printf '\\033[?1049h'\n");
        let _ = drain_agent_until(&mut agent, Duration::from_secs(2), |_| false);

        for frame in 1..=12 {
            let command = format!("printf '\\033[H\\033[2JFRAME{frame:03}\\nROW{frame:03}\\n'\n");
            agent.send_bytes(command.as_bytes());
            assert!(
                drain_agent_until(&mut agent, Duration::from_secs(2), |agent| {
                    parser_snapshot_text(&mut agent.parser).contains(&format!("FRAME{frame:03}"))
                }),
                "fullscreen frame {frame:03} did not reach the parser"
            );
        }

        assert_eq!(
            parser_max_scrollback(&mut agent.parser),
            0,
            "fullscreen redraws should leave vt100 scrollback empty"
        );
        assert!(
            agent
                .screen_history
                .max_scroll_offset(agent.pty_size.rows as usize)
                > 0,
            "screen history should provide a fallback scroll range"
        );

        let snapshot = agent.screen_snapshot_bytes(true);
        let mut restored = vt100::Parser::new(
            agent.pty_size.rows,
            agent.pty_size.cols,
            AGENT_SCROLLBACK_LINES,
        );
        safe_parser_process(&mut restored, &snapshot);
        let restored_text = parser_snapshot_text(&mut restored);
        assert!(
            restored_text.contains("FRAME001") && restored_text.contains("FRAME012"),
            "snapshot event did not transfer fullscreen screen history: {:?}",
            restored_text
        );

        agent.send_bytes(b"printf '\\033[?1049l'; exit\n");
        let _ = drain_agent_until(&mut agent, Duration::from_millis(500), |_| false);
        let _ = agent.child.kill();
        let _ = agent.child.wait();
        drop(agent.pty_log.take());
        let _ = remove_agent_pty_log(&key);
    }

    #[cfg(unix)]
    fn drain_agent_until<F>(agent: &mut AgentSession, timeout: Duration, mut predicate: F) -> bool
    where
        F: FnMut(&mut AgentSession) -> bool,
    {
        let start = Instant::now();
        while start.elapsed() < timeout {
            let _ = agent.drain_output_chunks();
            if predicate(agent) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = agent.drain_output_chunks();
        predicate(agent)
    }

    fn parser_snapshot_text(parser: &mut vt100::Parser) -> String {
        String::from_utf8_lossy(&parser_snapshot_bytes(parser, true)).into_owned()
    }

    #[test]
    fn new_run_pty_log_open_truncates_existing_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.ptylog");
        fs::write(&path, b"OLD HISTORY\n").unwrap();

        let info = session_info(Provider::Claude, "fresh-run", "/repo");
        let mut file = open_agent_pty_log_for_new_run(&path, &info).unwrap();
        file.write_all(b"NEW HISTORY\n").unwrap();
        drop(file);

        assert_eq!(fs::read(&path).unwrap(), b"NEW HISTORY\n");
    }

    #[test]
    fn cleanup_orphan_agent_pty_logs_removes_only_non_live_logs() {
        let dir = tempfile::tempdir().unwrap();
        let scrollback_dir = dir.path().join("scrollback");
        fs::create_dir_all(&scrollback_dir).unwrap();
        let orphan = scrollback_dir.join("orphan.ptylog");
        let live = scrollback_dir.join("live.ptylog");
        fs::write(&orphan, b"ORPHAN\n").unwrap();
        fs::write(&live, b"LIVE\n").unwrap();
        fs::write(
            dir.path().join("live.json"),
            format!(r#"{{"pid":{}}}"#, std::process::id()),
        )
        .unwrap();

        assert_eq!(cleanup_orphan_agent_pty_logs_at(dir.path()), 1);
        assert!(!orphan.exists());
        assert!(live.exists());
    }

    #[test]
    fn kill_policy_removes_persistent_pty_log() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        fs::write(&path, b"PTY HISTORY\n").unwrap();

        let info = session_info(Provider::Claude, "kill-history", "/repo");
        let key = AgentKey::new(&info);
        assert!(remove_agent_pty_log_file(&key, &path));
        assert!(!path.exists());
        assert!(!remove_agent_pty_log_file(&key, &path));
    }

    #[test]
    fn killall_removes_stale_agent_runtime_files() {
        let dir = tempfile::tempdir().unwrap();
        let scrollback = dir.path().join("scrollback");
        fs::create_dir_all(&scrollback).unwrap();
        let stem = "codex-stale";
        fs::write(
            dir.path().join(format!("{}.json", stem)),
            r#"{"pid":3000000000,"provider":"codex","session_id":"stale"}"#,
        )
        .unwrap();
        fs::write(dir.path().join(format!("{}.sock", stem)), "").unwrap();
        fs::write(scrollback.join(format!("{}.ptylog", stem)), b"OLD\n").unwrap();

        let report = kill_all_agent_daemons_at(dir.path(), std::process::id());

        assert_eq!(report.scanned, 1);
        assert_eq!(report.stale, 1);
        assert_eq!(report.killed, 0);
        assert_eq!(report.pty_logs_deleted, 1);
        assert!(!dir.path().join(format!("{}.json", stem)).exists());
        assert!(!dir.path().join(format!("{}.sock", stem)).exists());
        assert!(!scrollback.join(format!("{}.ptylog", stem)).exists());
    }

    #[test]
    fn unconnected_current_process_is_not_verified_agent_daemon() {
        let info = session_info(Provider::Codex, "verify-miss", "/repo");
        let key = AgentKey::new(&info);

        assert!(!verify_agent_daemon_identity(&key, std::process::id()));
    }

    #[test]
    fn agent_daemon_identity_requires_current_app_and_matching_session() {
        let info = session_info(Provider::Codex, "verify-match", "/repo");
        let key = AgentKey::new(&info);
        let current = std::env::current_exe().unwrap().display().to_string();

        assert!(agent_daemon_args_match(
            &[
                current,
                AGENT_DAEMON_ARG.into(),
                "codex".into(),
                "verify-match".into(),
                "/repo".into(),
            ],
            &key,
        ));
        assert!(!agent_daemon_args_match(
            &[
                "sleep".into(),
                AGENT_DAEMON_ARG.into(),
                "codex".into(),
                "verify-match".into(),
            ],
            &key,
        ));
        assert!(!agent_daemon_args_match(
            &[
                "cokacmux".into(),
                AGENT_DAEMON_ARG.into(),
                "codex".into(),
                "other-session".into(),
            ],
            &key,
        ));
    }

    #[cfg(unix)]
    #[test]
    fn killall_does_not_terminate_unverified_live_pid() {
        let mut command = Command::new("sleep");
        command.arg("30");
        unsafe {
            command.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => return,
        };

        let dir = tempfile::tempdir().unwrap();
        let info = session_info(Provider::Codex, "pid-reuse", "/repo");
        let key = AgentKey::new(&info);
        let stem = agent_file_stem(&key);
        fs::write(
            dir.path().join(format!("{}.json", stem)),
            format!(
                r#"{{"pid":{},"provider":"codex","session_id":"pid-reuse"}}"#,
                child.id()
            ),
        )
        .unwrap();

        let report = kill_all_agent_daemons_at(dir.path(), std::process::id());

        assert_eq!(report.scanned, 1);
        assert_eq!(report.killed, 0);
        assert_eq!(report.stale, 1);
        assert!(
            child.try_wait().unwrap().is_none(),
            "unverified live pid was terminated"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn killall_skips_current_process_meta() {
        let dir = tempfile::tempdir().unwrap();
        let stem = "codex-self";
        let meta = dir.path().join(format!("{}.json", stem));
        fs::write(
            &meta,
            format!(
                r#"{{"pid":{},"provider":"codex","session_id":"self"}}"#,
                std::process::id()
            ),
        )
        .unwrap();

        let report = kill_all_agent_daemons_at(dir.path(), std::process::id());

        assert_eq!(report.scanned, 1);
        assert_eq!(report.skipped_self, 1);
        assert_eq!(report.killed, 0);
        assert!(meta.exists());
    }

    #[test]
    fn session_kill_key_matches_agent_kill_key() {
        assert!(is_session_kill_key(KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::CONTROL
        )));
        assert!(is_session_kill_key(KeyEvent::new(
            KeyCode::Char('\u{b}'),
            KeyModifiers::NONE
        )));
        assert!(!is_session_kill_key(KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn agent_switch_key_accepts_direct_switch_shortcuts() {
        assert_eq!(
            is_agent_switch_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            is_agent_switch_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            is_agent_switch_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            None
        );
        assert_eq!(
            is_agent_switch_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)),
            None
        );
        assert_eq!(
            is_agent_switch_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::CONTROL)),
            Some(-1)
        );
        assert_eq!(
            is_agent_switch_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::CONTROL)),
            Some(1)
        );
        assert_eq!(
            is_agent_switch_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            is_agent_switch_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn pane_resize_keys_accept_alt_or_ctrl_shift_left_right() {
        assert_eq!(
            is_sessions_pane_resize_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            is_sessions_pane_resize_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)),
            Some(PANE_RESIZE_STEP_COLUMNS as i16)
        );
        assert_eq!(
            is_sessions_pane_resize_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            Some(-(PANE_RESIZE_STEP_COLUMNS as i16))
        );
        assert_eq!(
            is_sessions_pane_resize_key(KeyEvent::new(
                KeyCode::Right,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(PANE_RESIZE_STEP_COLUMNS as i16)
        );
        assert_eq!(
            is_sessions_pane_resize_key(KeyEvent::new(
                KeyCode::Left,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(-(PANE_RESIZE_STEP_COLUMNS as i16))
        );
        assert_eq!(
            is_sessions_pane_resize_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            is_sessions_pane_resize_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(
            is_agent_pane_resize_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            Some(-(AGENT_SIDEBAR_RESIZE_STEP as i16))
        );
        assert_eq!(
            is_agent_pane_resize_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)),
            Some(AGENT_SIDEBAR_RESIZE_STEP as i16)
        );
        assert_eq!(
            is_agent_pane_resize_key(KeyEvent::new(
                KeyCode::Left,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(-(AGENT_SIDEBAR_RESIZE_STEP as i16))
        );
        assert_eq!(
            is_agent_pane_resize_key(KeyEvent::new(
                KeyCode::Right,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(AGENT_SIDEBAR_RESIZE_STEP as i16)
        );
        assert_eq!(
            is_agent_pane_resize_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            is_agent_pane_resize_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn agent_sidebar_select_key_accepts_alt_or_ctrl_shift_up_down() {
        assert_eq!(
            is_agent_sidebar_select_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)),
            Some(-1)
        );
        assert_eq!(
            is_agent_sidebar_select_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)),
            Some(1)
        );
        assert_eq!(
            is_agent_sidebar_select_key(KeyEvent::new(
                KeyCode::Up,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(-1)
        );
        assert_eq!(
            is_agent_sidebar_select_key(KeyEvent::new(
                KeyCode::Down,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(1)
        );
        assert_eq!(
            is_agent_sidebar_select_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            None
        );
        assert_eq!(
            is_agent_sidebar_select_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            is_agent_sidebar_select_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(
            is_agent_sidebar_select_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn sessions_sidebar_select_key_accepts_alt_or_ctrl_shift_up_down() {
        assert_eq!(
            is_sessions_sidebar_select_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)),
            Some(-1)
        );
        assert_eq!(
            is_sessions_sidebar_select_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)),
            Some(1)
        );
        assert_eq!(
            is_sessions_sidebar_select_key(KeyEvent::new(
                KeyCode::Up,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(-1)
        );
        assert_eq!(
            is_sessions_sidebar_select_key(KeyEvent::new(
                KeyCode::Down,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(1)
        );
        assert_eq!(
            is_sessions_sidebar_select_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            None
        );
        assert_eq!(
            is_sessions_sidebar_select_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            is_sessions_sidebar_select_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(
            is_sessions_sidebar_select_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn codex_repair_detection_is_limited_to_cokacmux_owned_rollouts() {
        use std::io::Write;

        let mut mux_owned = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            mux_owned,
            r#"{{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{{"id":"019e4660-0000-7000-8000-000000000006","cwd":"/tmp","originator":"cokacmux"}}}}"#
        )
        .unwrap();
        writeln!(
            mux_owned,
            r#"{{"timestamp":"2026-05-20T01:00:00.100Z","type":"response_item","payload":{{"type":"message","role":"user","content":[],"id":"u1"}}}}"#
        )
        .unwrap();
        assert!(codex_rollout_needs_repair(mux_owned.path()).unwrap());

        let mut missing_modern_meta = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            missing_modern_meta,
            r#"{{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{{"id":"019e4660-0000-7000-8000-000000000004","cwd":"/tmp","originator":"cokacmux","cli_version":"0.1.9"}}}}"#
        )
        .unwrap();
        writeln!(
            missing_modern_meta,
            r#"{{"timestamp":"2026-05-20T01:00:00.100Z","type":"response_item","payload":{{"type":"message","role":"user","content":[]}}}}"#
        )
        .unwrap();
        assert!(codex_rollout_needs_repair(missing_modern_meta.path()).unwrap());

        let mut missing_display_events = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            missing_display_events,
            r#"{{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{{"id":"019e4660-0000-7000-8000-000000000005","timestamp":"2026-05-20T01:00:00.000Z","cwd":"/tmp","originator":"cokacmux","source":"cli","thread_source":"user","model_provider":"openai","base_instructions":{{"text":"You are Codex."}}}}}}"#
        )
        .unwrap();
        writeln!(
            missing_display_events,
            r#"{{"timestamp":"2026-05-20T01:00:00.100Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"hello"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            missing_display_events,
            r#"{{"timestamp":"2026-05-20T01:00:01.100Z","type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"hi back"}}]}}}}"#
        )
        .unwrap();
        assert!(codex_rollout_needs_repair(missing_display_events.path()).unwrap());

        let mut native = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            native,
            r#"{{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{{"id":"019e4660-0000-7000-8000-000000000002","cwd":"/tmp","originator":"codex-tui"}}}}"#
        )
        .unwrap();
        writeln!(
            native,
            r#"{{"timestamp":"2026-05-20T01:00:00.100Z","type":"response_item","payload":{{"type":"message","role":"user","content":[],"id":"u1"}}}}"#
        )
        .unwrap();
        assert!(!codex_rollout_needs_repair(native.path()).unwrap());
    }

    #[test]
    fn codex_repair_rewrites_cokacmux_owned_rollout_for_resume() {
        use std::io::Write;

        let session_id = "019e4660-0000-7000-8000-000000000003";
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{{"id":"{}","cwd":"/tmp","originator":"cokacmux"}}}}"#,
            session_id
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":null,"type":"event_msg","payload":{{"type":"synthesized.claude:system","raw":{{}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-20T01:00:00.100Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"hello"}}],"id":"u1"}}}}"#
        )
        .unwrap();

        let info = SessionInfo {
            provider: Provider::Codex,
            session_id: session_id.to_string(),
            cwd: "/tmp".to_string(),
            source: file.path().to_path_buf(),
            updated_at_epoch_s: 0,
            title: None,
        };

        repair_cokacmux_codex_rollout(&info).unwrap();

        let repaired = fs::read_to_string(file.path()).unwrap();
        assert!(!repaired.contains("\"timestamp\":null"));
        assert!(!repaired.contains("synthesized."));
        assert!(!repaired.contains("\"id\":\"u1\""));
        assert!(repaired.contains("\"text\":\"hello\""));
        assert!(repaired.contains("\"type\":\"user_message\""));
    }

    #[test]
    fn codex_repair_does_not_rewrite_while_live_daemon_exists() {
        use std::io::Write;

        let session_id = "019e4660-0000-7000-8000-000000000007";
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-20T01:00:00.000Z","type":"session_meta","payload":{{"id":"{}","cwd":"/tmp","originator":"cokacmux"}}}}"#,
            session_id
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":null,"type":"event_msg","payload":{{"type":"synthesized.claude:system","raw":{{}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-20T01:00:00.100Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"hello"}}],"id":"u1"}}}}"#
        )
        .unwrap();

        let info = SessionInfo {
            provider: Provider::Codex,
            session_id: session_id.to_string(),
            cwd: "/tmp".to_string(),
            source: file.path().to_path_buf(),
            updated_at_epoch_s: 0,
            title: None,
        };
        let live_meta = AgentMetaSnapshot {
            pid: std::process::id(),
            provider: Some("codex".into()),
            session_id: Some(session_id.into()),
            ..Default::default()
        };

        repair_cokacmux_codex_rollout_guarded(&info, Some(&live_meta)).unwrap();

        let content = fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("\"timestamp\":null"));
        assert!(content.contains("synthesized."));
        assert!(content.contains("\"id\":\"u1\""));
    }
}
