//! Lightweight cokacmux-style debug logging shared by library code.
//!
//! This is intentionally dependency-light and best-effort: logging must never
//! affect conversion/session behavior.

use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

const APP_DIR_NAME: &str = ".cokacmux";
const DEBUG_LOG_FILE: &str = "cokacmux.log";
const DEBUG_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const DEBUG_UNKNOWN: u8 = 0;
const DEBUG_OFF: u8 = 1;
const DEBUG_ON: u8 = 2;

static DEBUG_STATE: AtomicU8 = AtomicU8::new(DEBUG_UNKNOWN);

pub(crate) fn set_enabled(enabled: bool) {
    DEBUG_STATE.store(
        if enabled { DEBUG_ON } else { DEBUG_OFF },
        Ordering::Relaxed,
    );
    if enabled {
        write_log_to(DEBUG_LOG_FILE, "library_debug_enabled {\"source\":\"cli\"}");
    }
}

pub(crate) fn log(event: &str, details: serde_json::Value) {
    if !enabled() {
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
    write_log_to(debug_log_file_for(event), &msg);
}

fn enabled() -> bool {
    match DEBUG_STATE.load(Ordering::Relaxed) {
        DEBUG_ON => true,
        DEBUG_OFF => false,
        _ => init_enabled(),
    }
}

fn init_enabled() -> bool {
    let convert_env_enabled = std::env::var("COKACCONVERT_DEBUG")
        .map(|value| value == "1")
        .unwrap_or(false);
    let enabled = convert_env_enabled;
    DEBUG_STATE.store(
        if enabled { DEBUG_ON } else { DEBUG_OFF },
        Ordering::Relaxed,
    );
    if enabled {
        write_log_to(
            DEBUG_LOG_FILE,
            "library_debug_enabled {\"source\":\"COKACCONVERT_DEBUG\"}",
        );
    }
    enabled
}

fn debug_log_file_for(_event: &str) -> &'static str {
    DEBUG_LOG_FILE
}

fn write_log_to(filename: &str, msg: &str) {
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
    let thread = std::thread::current();
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

fn app_config_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(APP_DIR_NAME))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|home| !home.is_empty())
                .map(PathBuf::from)
        })
}
