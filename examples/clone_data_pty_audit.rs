//! PTY-driven audit for cloned session folder snapshots.
//!
//! This launches the real cokacmux TUI in a real PTY with an isolated HOME,
//! drives `c`, confirm, `e`, launch-confirm, restore-confirm, and verifies the
//! resulting files on disk.

use std::env;
use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

const CTRL_Q: u8 = 0x11;
const ENTER: u8 = b'\r';
const COLS: u16 = 120;
const ROWS: u16 = 34;

type AuditResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

fn main() -> AuditResult<()> {
    let exe = cokacmux_path();
    if !exe.exists() {
        return Err(format!(
            "missing cokacmux binary at {}; set COKACMUX_BIN or run cargo build --bin cokacmux",
            exe.display()
        )
        .into());
    }

    let total_started = Instant::now();
    let restore = audit_clone_restore_success(&exe)?;
    let delete = audit_clone_delete_snapshot(&exe)?;
    #[cfg(unix)]
    let rollback = audit_clone_data_failure_rolls_back(&exe)?;

    println!("[ok] real PTY clone-data audit passed");
    println!("  binary: {}", exe.display());
    println!("  restore success: {}", restore);
    println!("  delete cleanup: {}", delete);
    #[cfg(unix)]
    println!("  snapshot failure rollback: {}", rollback);
    println!("  total={}ms", total_started.elapsed().as_millis());
    Ok(())
}

fn audit_clone_restore_success(exe: &Path) -> AuditResult<String> {
    let audit_started = Instant::now();
    let sandbox = tempfile::tempdir()?;
    let home = sandbox.path().join("home");
    let project = sandbox.path().join("project");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&project)?;
    fs::write(project.join("marker.txt"), "snapshot-original\n")?;
    fs::write(project.join("src.txt"), "source-file\n")?;
    write_agent_stub_settings(&home)?;
    write_claude_session(&home, &project, "pty-source")?;

    let mut audit = PtyAudit::spawn(exe, &home)?;

    let boot_ms = audit.wait_screen_contains("pty-source", Duration::from_secs(6))?;
    audit.send(b"c")?;
    let clone_prompt_ms =
        audit.wait_screen_contains("Copy working folder data", Duration::from_secs(4))?;
    audit.send(b"y")?;
    let (snapshot_json, snapshot_dir, clone_session_id, snapshot_wait_ms) =
        wait_for_snapshot(&home, "pty-source", Duration::from_secs(6))?;
    assert_file_eq(&snapshot_dir.join("marker.txt"), "snapshot-original\n")?;
    assert_file_eq(&snapshot_dir.join("src.txt"), "source-file\n")?;

    fs::write(project.join("marker.txt"), "mutated-before-restore\n")?;
    fs::write(project.join("current-only.txt"), "backup-only\n")?;

    thread::sleep(Duration::from_millis(250));
    audit.send(b"e")?;
    let launch_prompt_ms =
        audit.wait_screen_contains("choose launch mode", Duration::from_secs(4))?;
    audit.send(&[ENTER])?;
    let restore_prompt_ms =
        audit.wait_screen_contains("Saved folder data exists", Duration::from_secs(4))?;
    audit.send(b"y")?;
    let (backup_path, restore_wait_ms) = wait_for_restore(&project, Duration::from_secs(6))?;
    if snapshot_json.exists() || snapshot_dir.exists() {
        return Err(format!(
            "restore left consumed snapshot behind: json={} dir={}",
            snapshot_json.display(),
            snapshot_dir.display()
        )
        .into());
    }

    audit.quit_cleanly()?;
    let bytes = audit.captured_len();

    Ok(format!(
        "home={} source=pty-source clone={} snapshot_json={} snapshot_dir={} backup={} boot={}ms clone_prompt={}ms snapshot={}ms launch_prompt={}ms restore_prompt={}ms restore={}ms total={}ms bytes={}",
        home.display(),
        clone_session_id,
        snapshot_json.display(),
        snapshot_dir.display(),
        backup_path.display(),
        boot_ms,
        clone_prompt_ms,
        snapshot_wait_ms,
        launch_prompt_ms,
        restore_prompt_ms,
        restore_wait_ms,
        audit_started.elapsed().as_millis(),
        bytes
    ))
}

fn audit_clone_delete_snapshot(exe: &Path) -> AuditResult<String> {
    let audit_started = Instant::now();
    let sandbox = tempfile::tempdir()?;
    let home = sandbox.path().join("home");
    let project = sandbox.path().join("project");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&project)?;
    fs::write(project.join("delete-marker.txt"), "delete-snapshot\n")?;
    write_agent_stub_settings(&home)?;
    write_claude_session(&home, &project, "pty-delete-source")?;

    let mut audit = PtyAudit::spawn(exe, &home)?;
    let boot_ms = audit.wait_screen_contains("pty-delete-source", Duration::from_secs(6))?;
    audit.send(b"c")?;
    let clone_prompt_ms =
        audit.wait_screen_contains("Copy working folder data", Duration::from_secs(4))?;
    audit.send(b"y")?;
    let (snapshot_json, snapshot_dir, clone_session_id, snapshot_wait_ms) =
        wait_for_snapshot(&home, "pty-delete-source", Duration::from_secs(6))?;
    let clone_file = claude_session_file(&home, &project, &clone_session_id);
    if !clone_file.is_file() {
        return Err(format!("expected cloned session file {}", clone_file.display()).into());
    }

    audit.send(b"d")?;
    let delete_prompt_ms = audit.wait_screen_contains(&clone_session_id, Duration::from_secs(4))?;
    audit.send(b"y")?;
    let delete_wait_ms = wait_until(Duration::from_secs(6), || {
        !clone_file.exists() && !snapshot_json.exists() && !snapshot_dir.exists()
    })?;
    let source_file = claude_session_file(&home, &project, "pty-delete-source");
    if !source_file.is_file() {
        return Err("delete cleanup removed the source session".into());
    }

    audit.quit_cleanly()?;
    let bytes = audit.captured_len();

    Ok(format!(
        "home={} source=pty-delete-source clone={} boot={}ms clone_prompt={}ms snapshot={}ms delete_prompt={}ms delete={}ms total={}ms bytes={}",
        home.display(),
        clone_session_id,
        boot_ms,
        clone_prompt_ms,
        snapshot_wait_ms,
        delete_prompt_ms,
        delete_wait_ms,
        audit_started.elapsed().as_millis(),
        bytes
    ))
}

#[cfg(unix)]
fn audit_clone_data_failure_rolls_back(exe: &Path) -> AuditResult<String> {
    let audit_started = Instant::now();
    let sandbox = tempfile::tempdir()?;
    let home = sandbox.path().join("home");
    let project = sandbox.path().join("project");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&project)?;
    fs::write(project.join("rollback-marker.txt"), "rollback\n")?;
    let _unsupported_socket = std::os::unix::net::UnixListener::bind(project.join("block.sock"))?;
    write_agent_stub_settings(&home)?;
    write_claude_session(&home, &project, "pty-rollback-source")?;

    let mut audit = PtyAudit::spawn(exe, &home)?;
    let boot_ms = audit.wait_screen_contains("pty-rollback-source", Duration::from_secs(6))?;
    audit.send(b"c")?;
    let clone_prompt_ms =
        audit.wait_screen_contains("Copy working folder data", Duration::from_secs(4))?;
    audit.send(b"y")?;
    let rollback_wait_ms =
        audit.wait_screen_contains("aborted: folder data copy failed", Duration::from_secs(6))?;

    let files = claude_session_files(&home, &project)?;
    if files.len() != 1 || files[0] != claude_session_file(&home, &project, "pty-rollback-source") {
        return Err(format!("rollback left unexpected session files: {files:?}").into());
    }
    let data_root = home.join(".cokacmux").join("data");
    if data_root.exists() {
        let leftovers: Vec<_> = fs::read_dir(&data_root)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<_, _>>()?;
        if !leftovers.is_empty() {
            return Err(format!("rollback left snapshot data behind: {leftovers:?}").into());
        }
    }

    audit.quit_cleanly()?;
    let bytes = audit.captured_len();

    Ok(format!(
        "home={} source=pty-rollback-source boot={}ms clone_prompt={}ms rollback={}ms total={}ms bytes={}",
        home.display(),
        boot_ms,
        clone_prompt_ms,
        rollback_wait_ms,
        audit_started.elapsed().as_millis(),
        bytes
    ))
}

struct PtyAudit {
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>>,
    reader_thread: Option<thread::JoinHandle<()>>,
    captured: Arc<Mutex<Vec<u8>>>,
}

impl PtyAudit {
    fn spawn(exe: &Path, home: &Path) -> AuditResult<Self> {
        let pty = NativePtySystem::default();
        let pair = pty.openpty(PtySize {
            cols: COLS,
            rows: ROWS,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(exe);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLUMNS", COLS.to_string());
        cmd.env("LINES", ROWS.to_string());
        cmd.env("HOME", home.display().to_string());
        cmd.env("COKACMUX_DEBUG", "1");
        cmd.cwd(env::current_dir()?);

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer()?));
        let writer_for_reader = Arc::clone(&writer);
        let master = Arc::new(Mutex::new(Some(pair.master)));
        let captured = Arc::new(Mutex::new(Vec::with_capacity(256 * 1024)));
        let captured_for_reader = Arc::clone(&captured);

        {
            let mut writer = writer.lock().unwrap();
            writer.write_all(b"\x1b[1;1R")?;
            writer.flush()?;
        }

        let reader_thread = thread::spawn(move || {
            let mut tmp = [0u8; 8192];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        let slice = &tmp[..n];
                        if let Ok(mut captured) = captured_for_reader.lock() {
                            captured.extend_from_slice(slice);
                        }
                        if slice.windows(4).any(|window| window == b"\x1b[6n") {
                            if let Ok(mut writer) = writer_for_reader.lock() {
                                let _ = writer.write_all(b"\x1b[1;1R");
                                let _ = writer.flush();
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child: Some(child),
            writer,
            master,
            reader_thread: Some(reader_thread),
            captured,
        })
    }

    fn send(&self, bytes: &[u8]) -> AuditResult<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    fn wait_screen_contains(&self, needle: &str, timeout: Duration) -> AuditResult<u128> {
        let start = Instant::now();
        loop {
            let screen = self.screen_text();
            if screen.contains(needle) {
                return Ok(start.elapsed().as_millis());
            }
            if start.elapsed() >= timeout {
                return Err(format!(
                    "timed out waiting for screen text {needle:?}\n--- screen ---\n{screen}"
                )
                .into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn screen_text(&self) -> String {
        let bytes = self.captured.lock().unwrap().clone();
        let mut parser = vt100::Parser::new(ROWS, COLS, 2000);
        parser.process(&bytes);
        let lines: Vec<String> = parser
            .screen()
            .rows(0, COLS)
            .take(ROWS as usize)
            .map(|line| line.trim_end_matches(' ').to_string())
            .collect();
        lines.join("\n")
    }

    fn captured_len(&self) -> usize {
        self.captured.lock().unwrap().len()
    }

    fn quit_cleanly(&mut self) -> AuditResult<()> {
        self.send(&[CTRL_Q])?;
        let mut exited_ok = false;
        if let Some(child) = self.child.as_mut() {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(4) {
                if let Some(status) = child.try_wait()? {
                    exited_ok = status.success();
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            if !exited_ok {
                let _ = child.kill();
                let status = child.wait()?;
                exited_ok = status.success();
            }
        }
        self.child = None;
        self.close_reader();
        if !exited_ok {
            return Err("cokacmux did not exit cleanly after Ctrl+Q".into());
        }
        let captured_guard = self.captured.lock().unwrap();
        let captured = String::from_utf8_lossy(&captured_guard);
        if captured.contains("panicked at")
            || captured.contains("thread main panicked")
            || (captured.contains("RUST_BACKTRACE") && captured.contains("backtrace"))
        {
            return Err("captured panic/backtrace text in PTY output".into());
        }
        Ok(())
    }

    fn close_reader(&mut self) {
        if let Ok(mut master) = self.master.lock() {
            master.take();
        }
        let start = Instant::now();
        while self
            .reader_thread
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
            && start.elapsed() < Duration::from_secs(2)
        {
            thread::sleep(Duration::from_millis(50));
        }
        if self
            .reader_thread
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            if let Some(handle) = self.reader_thread.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for PtyAudit {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
        self.close_reader();
    }
}

fn cokacmux_path() -> PathBuf {
    if let Some(path) = env::var_os("COKACMUX_BIN") {
        return PathBuf::from(path);
    }
    let bin = format!("cokacmux{}", env::consts::EXE_SUFFIX);
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("target")
        .join("debug")
        .join(bin)
}

fn write_agent_stub_settings(home: &Path) -> AuditResult<()> {
    let app_dir = home.join(".cokacmux");
    fs::create_dir_all(&app_dir)?;
    let stub = agent_stub_program(home)?;
    let settings = serde_json::json!({
        "cokacmux": {
            "agent_programs": {
                "claude": stub.display().to_string()
            }
        }
    });
    fs::write(
        app_dir.join("settings.json"),
        format!("{}\n", serde_json::to_string_pretty(&settings)?),
    )?;
    Ok(())
}

#[cfg(unix)]
fn agent_stub_program(home: &Path) -> AuditResult<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let path = home.join("agent-stub.sh");
    fs::write(
        &path,
        "#!/bin/sh\nprintf 'cokacmux-agent-stub\\n'\nexit 0\n",
    )?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

#[cfg(windows)]
fn agent_stub_program(home: &Path) -> AuditResult<PathBuf> {
    let path = home.join("agent-stub.cmd");
    fs::write(
        &path,
        "@echo off\r\necho cokacmux-agent-stub\r\nexit /b 0\r\n",
    )?;
    Ok(path)
}

fn write_claude_session(home: &Path, project: &Path, session_id: &str) -> AuditResult<()> {
    let encoded_cwd = encode_claude_cwd(&project.display().to_string());
    let session_dir = home.join(".claude").join("projects").join(encoded_cwd);
    fs::create_dir_all(&session_dir)?;
    let cwd = json_string(&project.display().to_string())?;
    let sid = json_string(session_id)?;
    let content = format!(
        "{{\"type\":\"permission-mode\",\"permissionMode\":\"default\",\"sessionId\":{sid}}}\n\
         {{\"type\":\"user\",\"sessionId\":{sid},\"cwd\":{cwd},\"timestamp\":\"2026-05-28T00:00:00.000Z\",\"uuid\":\"u1\",\"parentUuid\":null,\"message\":{{\"role\":\"user\",\"content\":\"pty clone audit\"}}}}\n\
         {{\"type\":\"assistant\",\"sessionId\":{sid},\"cwd\":{cwd},\"timestamp\":\"2026-05-28T00:00:01.000Z\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{{\"role\":\"assistant\",\"id\":\"msg_pty_audit\",\"model\":\"claude-opus-4-7\",\"content\":[{{\"type\":\"text\",\"text\":\"ready\"}}],\"stop_reason\":\"end_turn\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}}}\n"
    );
    fs::write(session_dir.join(format!("{session_id}.jsonl")), content)?;
    Ok(())
}

fn json_string(value: &str) -> AuditResult<String> {
    Ok(serde_json::to_string(value)?)
}

fn encode_claude_cwd(abs_path: &str) -> String {
    abs_path
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '.' | '_' | ':' => '-',
            other => other,
        })
        .collect()
}

fn claude_session_file(home: &Path, project: &Path, session_id: &str) -> PathBuf {
    home.join(".claude")
        .join("projects")
        .join(encode_claude_cwd(&project.display().to_string()))
        .join(format!("{session_id}.jsonl"))
}

fn claude_session_files(home: &Path, project: &Path) -> AuditResult<Vec<PathBuf>> {
    let dir = home
        .join(".claude")
        .join("projects")
        .join(encode_claude_cwd(&project.display().to_string()));
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn wait_until<F>(timeout: Duration, mut predicate: F) -> AuditResult<u128>
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    loop {
        if predicate() {
            return Ok(start.elapsed().as_millis());
        }
        if start.elapsed() >= timeout {
            return Err("timed out waiting for condition".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_snapshot(
    home: &Path,
    source_session_id: &str,
    timeout: Duration,
) -> AuditResult<(PathBuf, PathBuf, String, u128)> {
    let data_root = home.join(".cokacmux").join("data");
    let start = Instant::now();
    loop {
        if data_root.is_dir() {
            let mut json_files = Vec::new();
            for entry in fs::read_dir(&data_root)? {
                let path = entry?.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                    json_files.push(path);
                }
            }
            if let Some(json_path) = json_files.into_iter().next() {
                let value: serde_json::Value = serde_json::from_slice(&fs::read(&json_path)?)?;
                let session_id = value
                    .get("session_id")
                    .and_then(|value| value.as_str())
                    .ok_or("snapshot metadata missing session_id")?
                    .to_string();
                if session_id == source_session_id {
                    return Err("snapshot was written for source session instead of clone".into());
                }
                let snapshot_path = value
                    .get("snapshot_path")
                    .and_then(|value| value.as_str())
                    .map(PathBuf::from)
                    .ok_or("snapshot metadata missing snapshot_path")?;
                if snapshot_path.is_dir() {
                    return Ok((
                        json_path,
                        snapshot_path,
                        session_id,
                        start.elapsed().as_millis(),
                    ));
                }
            }
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "timed out waiting for clone snapshot under {}",
                data_root.display()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_restore(project: &Path, timeout: Duration) -> AuditResult<(PathBuf, u128)> {
    let start = Instant::now();
    loop {
        if fs::read_to_string(project.join("marker.txt"))
            .ok()
            .as_deref()
            == Some("snapshot-original\n")
            && !project.join("current-only.txt").exists()
        {
            let backup = find_backup(project)?;
            assert_file_eq(&backup.join("marker.txt"), "mutated-before-restore\n")?;
            assert_file_eq(&backup.join("current-only.txt"), "backup-only\n")?;
            return Ok((backup, start.elapsed().as_millis()));
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "timed out waiting for restored project at {}",
                project.display()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn find_backup(project: &Path) -> AuditResult<PathBuf> {
    let parent = project.parent().ok_or("project path has no parent")?;
    let prefix = format!(
        "{}.cokacmux-backup-",
        project
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("project path has no file name")?
    );
    for entry in fs::read_dir(parent)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&prefix))
        {
            return Ok(path);
        }
    }
    Err(format!("backup directory with prefix {prefix:?} not found").into())
}

fn assert_file_eq(path: &Path, expected: &str) -> AuditResult<()> {
    let actual = fs::read_to_string(path)?;
    if actual != expected {
        return Err(format!(
            "unexpected content in {}: expected {:?}, got {:?}",
            path.display(),
            expected,
            actual
        )
        .into());
    }
    Ok(())
}
