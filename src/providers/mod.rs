//! Provider adapters.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(feature = "claude")]
pub mod claude;

#[cfg(feature = "codex")]
pub mod codex;

#[cfg(feature = "opencode")]
pub mod opencode;

#[cfg(feature = "pi")]
pub mod pi;

#[cfg(feature = "gjc")]
pub mod gjc;

#[cfg(feature = "discovery")]
pub mod discovery;

/// A provider CLI that cokacmux runs so the provider itself performs an
/// operation on its own storage (building an index, importing a session).
#[derive(Debug, Clone)]
pub struct ProviderCommand {
    pub program: PathBuf,
    /// PATH for the child when the program was found on a PATH other than
    /// this process's (a login shell's).
    pub path_env: Option<OsString>,
}

impl ProviderCommand {
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        if let Some(path) = &self.path_env {
            command.env("PATH", path);
        }
        command
    }
}

/// Runs `command` with its output captured in temporary files, so a process
/// it leaves behind holding the output open cannot keep the wait from ending.
/// `Ok(None)` when it did not exit within `timeout`; it is then killed.
pub(crate) fn run_command_bounded(
    mut command: Command,
    timeout: Duration,
) -> std::io::Result<Option<Output>> {
    let stdout_path = bounded_command_capture_path("stdout");
    let stderr_path = bounded_command_capture_path("stderr");
    let result = (|| {
        let stdout = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stdout_path)?;
        let stderr = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stderr_path)?;
        let mut child = command
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(Some(Output {
                    status,
                    stdout: fs::read(&stdout_path)?,
                    stderr: fs::read(&stderr_path)?,
                }));
            }
            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    result
}

fn bounded_command_capture_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cokacmux-provider-cli-{}-{}-{label}.out",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ))
}

/// The last `max_chars` characters of a command's output, for error messages.
pub(crate) fn output_tail(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    let skip = trimmed.chars().count().saturating_sub(max_chars);
    trimmed.chars().skip(skip).collect()
}
