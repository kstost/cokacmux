//! Lightweight cokacmux-style debug logging shared by library code.
//!
//! This is intentionally dependency-light and best-effort: logging must never
//! affect conversion/session behavior, and must never block the calling
//! thread on disk I/O. Lines are formatted at the call site (so timestamps
//! and thread identity stay accurate) and handed to a dedicated writer
//! thread through a bounded queue. When the queue is full — e.g. the disk
//! has stalled — lines are dropped and accounted for instead of stalling
//! the caller.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const APP_DIR_NAME: &str = ".cokacmux";
const DEBUG_LOG_FILE: &str = "cokacmux.log";
const DEBUG_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const DEBUG_UNKNOWN: u8 = 0;
const DEBUG_OFF: u8 = 1;
const DEBUG_ON: u8 = 2;
const LOG_QUEUE_CAPACITY: usize = 4096;
// Several cokacmux processes (TUI + agent daemons) append to the same log
// file; a cached handle goes stale once another process rotates the file,
// so handles are re-opened and rotation is re-checked on this cadence.
const LOG_REOPEN_INTERVAL: Duration = Duration::from_secs(2);

static DEBUG_STATE: AtomicU8 = AtomicU8::new(DEBUG_UNKNOWN);
static DROPPED_LINES: AtomicU64 = AtomicU64::new(0);
static LOG_SENDER: OnceLock<Option<SyncSender<LogMessage>>> = OnceLock::new();

enum LogMessage {
    Line {
        filename: &'static str,
        max_bytes: u64,
        line: String,
    },
    Flush(SyncSender<()>),
}

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
    let env_enabled = std::env::var("COKACMUX_DEBUG")
        .map(|value| value == "1")
        .unwrap_or(false);
    let enabled = env_enabled;
    DEBUG_STATE.store(
        if enabled { DEBUG_ON } else { DEBUG_OFF },
        Ordering::Relaxed,
    );
    if enabled {
        write_log_to(
            DEBUG_LOG_FILE,
            "library_debug_enabled {\"source\":\"COKACMUX_DEBUG\"}",
        );
    }
    enabled
}

fn debug_log_file_for(_event: &str) -> &'static str {
    DEBUG_LOG_FILE
}

fn write_log_to(filename: &'static str, msg: &str) {
    write_line(filename, DEBUG_LOG_MAX_BYTES, msg);
}

/// Format `msg` with the standard log prefix and queue it for the shared
/// writer thread. Never blocks: if the queue is full or the writer thread
/// could not be spawned, the line is dropped and counted.
#[doc(hidden)]
pub fn write_line(filename: &'static str, max_bytes: u64, msg: &str) {
    let Some(sender) = log_sender() else {
        DROPPED_LINES.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let line = format_line(msg);
    if sender
        .try_send(LogMessage::Line {
            filename,
            max_bytes,
            line,
        })
        .is_err()
    {
        DROPPED_LINES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Wait until every line queued so far has been written (best effort).
/// Returns false on timeout or when the writer thread is unavailable.
#[doc(hidden)]
pub fn flush(timeout: Duration) -> bool {
    let Some(sender) = log_sender() else {
        return false;
    };
    let (ack_tx, ack_rx) = sync_channel(1);
    if sender.try_send(LogMessage::Flush(ack_tx)).is_err() {
        return false;
    }
    ack_rx.recv_timeout(timeout).is_ok()
}

fn log_sender() -> Option<&'static SyncSender<LogMessage>> {
    LOG_SENDER
        .get_or_init(|| {
            let (tx, rx) = sync_channel(LOG_QUEUE_CAPACITY);
            std::thread::Builder::new()
                .name("cokacmux-log-writer".into())
                .spawn(move || writer_loop(rx))
                .ok()
                .map(|_| tx)
        })
        .as_ref()
}

fn format_line(msg: &str) -> String {
    let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    let thread_id = format!("{:?}", thread.id());
    format!(
        "[{} pid={} thread={} {}] {}",
        timestamp,
        std::process::id(),
        thread_name,
        thread_id,
        msg
    )
}

struct OpenLog {
    file: File,
    opened_at: Instant,
}

fn writer_loop(rx: Receiver<LogMessage>) {
    let mut files: HashMap<&'static str, OpenLog> = HashMap::new();
    while let Ok(message) = rx.recv() {
        match message {
            LogMessage::Line {
                filename,
                max_bytes,
                line,
            } => {
                let dropped = DROPPED_LINES.swap(0, Ordering::Relaxed);
                if dropped > 0 {
                    let notice = format_line(&format!(
                        "debug_log_lines_dropped {{\"count\":{}}}",
                        dropped
                    ));
                    write_one(&mut files, filename, max_bytes, &notice);
                }
                write_one(&mut files, filename, max_bytes, &line);
            }
            LogMessage::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

fn write_one(
    files: &mut HashMap<&'static str, OpenLog>,
    filename: &'static str,
    max_bytes: u64,
    line: &str,
) {
    let expired = files
        .get(filename)
        .is_some_and(|log| log.opened_at.elapsed() >= LOG_REOPEN_INTERVAL);
    if expired {
        files.remove(filename);
    }
    if !files.contains_key(filename) {
        rotate_if_needed(filename, max_bytes);
        let Some(file) = open_log_file(filename) else {
            return;
        };
        files.insert(
            filename,
            OpenLog {
                file,
                opened_at: Instant::now(),
            },
        );
    }
    let Some(log) = files.get_mut(filename) else {
        return;
    };
    if writeln!(log.file, "{}", line).is_err() {
        // Drop the handle so the next line retries a fresh open.
        files.remove(filename);
    }
}

fn rotate_if_needed(filename: &str, max_bytes: u64) {
    let Some(dir) = log_dir() else {
        return;
    };
    let path = dir.join(filename);
    if path
        .metadata()
        .map(|meta| meta.len() > max_bytes)
        .unwrap_or(false)
    {
        let rotated = dir.join(format!("{}.1", filename));
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, rotated);
    }
}

fn open_log_file(filename: &str) -> Option<File> {
    let dir = log_dir()?;
    fs::create_dir_all(&dir).ok()?;
    #[cfg(unix)]
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    let path = dir.join(filename);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    #[cfg(unix)]
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    Some(file)
}

fn log_dir() -> Option<PathBuf> {
    app_config_dir().map(|dir| dir.join("debug"))
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
