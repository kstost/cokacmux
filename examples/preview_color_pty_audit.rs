//! PTY-driven audit for session-preview summary colors.
//!
//! This creates an isolated HOME with a synthetic Claude session, launches the
//! real cokacmux TUI inside a real PTY, waits for the preview worker to render,
//! and verifies the visible terminal cells carry the expected 256-color indexes.
//!
//! Usage:
//!   cargo build --bin cokacmux
//!   cargo run --example preview_color_pty_audit --features tui
//!
//! Optional:
//!   COKACMUX_BIN=/path/to/cokacmux cargo run --example preview_color_pty_audit --features tui

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

const CTRL_Q: u8 = 0x11;
const COLS: u16 = 160;
const ROWS: u16 = 64;

type AuditResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() -> AuditResult<()> {
    let exe = cokacmux_path()?;
    let home = make_temp_home()?;
    write_claude_color_session(&home)?;

    let mut audit = PtyAudit::spawn(&exe, &home, COLS, ROWS)?;
    let (screen, captured_len) =
        audit.wait_for_screen("tool result [toolu_color] · error", Duration::from_secs(8))?;

    assert_cell_color(&screen, "provider:", "claude", vt100::Color::Idx(139))?;
    assert_cell_color(&screen, "title   :", "error", vt100::Color::Idx(255))?;
    assert_cell_color(
        &screen,
        "cwd     :",
        "/tmp/error.rs",
        vt100::Color::Idx(116),
    )?;
    assert_cell_color(&screen, "USER #", "USER", vt100::Color::Idx(117))?;
    assert_cell_color(
        &screen,
        "http://cokacmux.cokac.com/dist_beta/",
        "http",
        vt100::Color::Idx(252),
    )?;
    assert_cell_color(&screen, "ASSISTANT #", "ASSISTANT", vt100::Color::Idx(114))?;
    assert_cell_color(&screen, "thinking", "thinking", vt100::Color::Idx(183))?;
    assert_cell_color(&screen, "tool use:", "tool use", vt100::Color::Idx(215))?;
    assert_cell_color(&screen, "path:", "path:", vt100::Color::Idx(180))?;
    assert_cell_color(&screen, "path:", "/tmp/error.rs", vt100::Color::Idx(252))?;
    assert_cell_color(
        &screen,
        "      nested_path:",
        "nested_path:",
        vt100::Color::Idx(252),
    )?;
    assert_cell_color(
        &screen,
        "      nested_path:",
        "/tmp/error.rs",
        vt100::Color::Idx(252),
    )?;
    assert_cell_color(&screen, "image:", "image:", vt100::Color::Idx(180))?;
    assert_cell_color(
        &screen,
        "attachment:",
        "attachment:",
        vt100::Color::Idx(181),
    )?;
    assert_cell_color(&screen, "TOOL #", "TOOL", vt100::Color::Idx(215))?;
    assert_cell_color(
        &screen,
        "-rw-rw-rw- 1 501",
        "-rw-rw-rw-",
        vt100::Color::Idx(252),
    )?;
    assert_cell_color(
        &screen,
        "Chunk ID: 5fea74",
        "Chunk ID:",
        vt100::Color::Idx(252),
    )?;
    assert_cell_color(
        &screen,
        "use ratatui::buffer::Buffer;",
        "ratatui",
        vt100::Color::Idx(252),
    )?;
    assert_cell_color(
        &screen,
        "tool result [toolu_color] · error",
        "tool result",
        vt100::Color::Idx(203),
    )?;
    assert_cell_color(
        &screen,
        "tool result [toolu_color] · error",
        "[toolu_color]",
        vt100::Color::Idx(245),
    )?;
    assert_cell_color(
        &screen,
        "tool result [toolu_color] · error",
        "error",
        vt100::Color::Idx(203),
    )?;

    audit.quit_cleanly()?;
    println!(
        "PASS preview_color_pty_audit home={} captured={} bytes",
        home.display(),
        captured_len
    );
    Ok(())
}

struct PtyAudit {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    master: Arc<Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>>,
    captured: Arc<Mutex<Vec<u8>>>,
    reader_thread: Option<std::thread::JoinHandle<()>>,
}

impl PtyAudit {
    fn spawn(exe: &Path, home: &Path, cols: u16, rows: u16) -> AuditResult<Self> {
        let pty = NativePtySystem::default();
        let pair = pty.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(exe);
        // The audit must measure the app's intended color output.  Inherited
        // NO_COLOR would legitimately suppress ANSI colors and turn this into
        // an environment test instead.
        cmd.env_remove("NO_COLOR");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLUMNS", cols.to_string());
        cmd.env("LINES", rows.to_string());
        cmd.env("HOME", home.display().to_string());
        cmd.env("USERPROFILE", home.display().to_string());
        cmd.env("COKACMUX_DEBUG", "0");
        cmd.cwd(env::current_dir()?);

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer: Arc<Mutex<Option<Box<dyn Write + Send>>>> =
            Arc::new(Mutex::new(Some(pair.master.take_writer()?)));
        let writer_for_reader = Arc::clone(&writer);
        let master = Arc::new(Mutex::new(Some(pair.master)));
        let captured = Arc::new(Mutex::new(Vec::with_capacity(256 * 1024)));
        let captured_for_reader = Arc::clone(&captured);

        {
            let mut w = writer.lock().unwrap();
            if let Some(w) = w.as_mut() {
                let _ = w.write_all(b"\x1b[1;1R");
                let _ = w.flush();
            }
        }

        let reader_thread = std::thread::spawn(move || {
            let mut tmp = [0u8; 8192];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        let slice = &tmp[..n];
                        captured_for_reader.lock().unwrap().extend_from_slice(slice);
                        if slice.windows(4).any(|w| w == b"\x1b[6n") {
                            if let Ok(mut guard) = writer_for_reader.lock() {
                                let Some(w) = guard.as_mut() else {
                                    break;
                                };
                                let _ = w.write_all(b"\x1b[1;1R");
                                let _ = w.flush();
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child,
            writer,
            master,
            captured,
            reader_thread: Some(reader_thread),
        })
    }

    fn wait_for_screen(
        &mut self,
        needle: &str,
        timeout: Duration,
    ) -> AuditResult<(vt100::Screen, usize)> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(100));
            let bytes = self.captured.lock().unwrap().clone();
            let mut parser = vt100::Parser::new(ROWS, COLS, 0);
            parser.process(&bytes);
            let screen = parser.screen().clone();
            let text = screen.contents();
            if text.contains("panicked at")
                || text.contains("thread main panicked")
                || (text.contains("RUST_BACKTRACE") && text.contains("backtrace"))
            {
                return Err("PTY output contains panic/backtrace text".into());
            }
            if text.contains(needle) {
                return Ok((screen, bytes.len()));
            }
        }
        let screen = {
            let bytes = self.captured.lock().unwrap().clone();
            let mut parser = vt100::Parser::new(ROWS, COLS, 0);
            parser.process(&bytes);
            parser.screen().contents()
        };
        Err(format!("timed out waiting for {needle:?}\n--- screen ---\n{screen}").into())
    }

    fn quit_cleanly(&mut self) -> AuditResult<()> {
        {
            let mut guard = self.writer.lock().unwrap();
            let Some(w) = guard.as_mut() else {
                return Err("PTY writer is already closed".into());
            };
            w.write_all(&[CTRL_Q])?;
            w.flush()?;
        }

        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                if !status.success() {
                    return Err(format!("cokacmux exited with {status:?}").into());
                }
                break;
            }
            if started.elapsed() > Duration::from_secs(4) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err("cokacmux did not exit cleanly after Ctrl+Q".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        self.writer.lock().unwrap().take();
        self.master.lock().unwrap().take();
        if let Some(handle) = self.reader_thread.take() {
            let started = Instant::now();
            while !handle.is_finished() && started.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(50));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        Ok(())
    }
}

fn assert_cell_color(
    screen: &vt100::Screen,
    row_contains: &str,
    needle: &str,
    expected: vt100::Color,
) -> AuditResult<()> {
    let Some((row, col, row_text)) = find_cell_for_text(screen, row_contains, needle) else {
        return Err(format!(
            "missing text {needle:?} on row containing {row_contains:?}\n--- screen ---\n{}",
            screen.contents()
        )
        .into());
    };
    let cell = screen
        .cell(row, col)
        .ok_or_else(|| format!("missing cell at row={row} col={col}"))?;
    let actual = cell.fgcolor();
    if actual != expected {
        let context = cell_context(screen, row, col);
        return Err(format!(
            "wrong color for {needle:?} on row {row} col {col}: expected {expected:?}, got {actual:?}\ncell={:?}\ncontext={context}\nrow: {row_text:?}",
            cell.contents()
        )
        .into());
    }
    Ok(())
}

fn cell_context(screen: &vt100::Screen, row: u16, col: u16) -> String {
    let (_, cols) = screen.size();
    let start = col.saturating_sub(8);
    let end = col.saturating_add(24).min(cols);
    let mut parts = Vec::new();
    for c in start..end {
        if let Some(cell) = screen.cell(row, c) {
            parts.push(format!("{}:{:?}:{:?}", c, cell.contents(), cell.fgcolor()));
        }
    }
    parts.join(" ")
}

fn find_cell_for_text(
    screen: &vt100::Screen,
    row_contains: &str,
    needle: &str,
) -> Option<(u16, u16, String)> {
    let (rows, cols) = screen.size();
    for row in 0..rows {
        let (text, starts) = row_text_and_starts(screen, row, cols);
        let Some(anchor) = text.find(row_contains) else {
            continue;
        };
        let Some(start) = text[anchor..].find(needle).map(|offset| anchor + offset) else {
            continue;
        };
        let col = starts
            .iter()
            .find_map(|(byte, col)| (*byte == start).then_some(*col))?;
        return Some((row, col, text));
    }
    None
}

fn row_text_and_starts(screen: &vt100::Screen, row: u16, cols: u16) -> (String, Vec<(usize, u16)>) {
    let mut text = String::new();
    let mut starts = Vec::with_capacity(cols as usize);
    for col in 0..cols {
        starts.push((text.len(), col));
        let piece = screen
            .cell(row, col)
            .map(|cell| cell.contents())
            .unwrap_or("");
        if piece.is_empty() {
            text.push(' ');
        } else {
            text.push_str(piece);
        }
    }
    (text, starts)
}

fn write_claude_color_session(home: &Path) -> AuditResult<()> {
    let cwd = "/tmp/error.rs";
    let session_id = "preview-color-session";
    let session_dir = home
        .join(".claude")
        .join("projects")
        .join(encode_claude_cwd(cwd));
    fs::create_dir_all(&session_dir)?;

    let sid = serde_json::to_string(session_id)?;
    let cwd_json = serde_json::to_string(cwd)?;
    let user_text = serde_json::to_string("http://cokacmux.cokac.com/dist_beta/")?;
    let title = serde_json::to_string("error")?;
    let tool_path = serde_json::to_string("/tmp/error.rs")?;
    let listing_output = serde_json::to_string(
        "Chunk ID: 5fea74\n-rw-rw-rw- 1 501 dialout 14347 May 21 Cargo.lock\nuse ratatui::buffer::Buffer;",
    )?;
    let content = format!(
        "{{\"type\":\"ai-title\",\"sessionId\":{sid},\"cwd\":{cwd_json},\"timestamp\":\"2026-05-29T00:00:00.000Z\",\"aiTitle\":{title}}}\n\
         {{\"type\":\"user\",\"sessionId\":{sid},\"cwd\":{cwd_json},\"timestamp\":\"2026-05-29T00:00:01.000Z\",\"uuid\":\"u1\",\"parentUuid\":null,\"message\":{{\"role\":\"user\",\"content\":{user_text}}}}}\n\
         {{\"type\":\"assistant\",\"sessionId\":{sid},\"cwd\":{cwd_json},\"timestamp\":\"2026-05-29T00:00:02.000Z\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{{\"role\":\"assistant\",\"id\":\"msg_preview_color\",\"model\":\"claude-opus-4-7\",\"content\":[\
         {{\"type\":\"thinking\",\"thinking\":\"reasoning sample\"}},\
         {{\"type\":\"text\",\"text\":\"assistant color sample\"}},\
         {{\"type\":\"tool_use\",\"id\":\"toolu_listing\",\"name\":\"Bash\",\"input\":{{\"command\":\"ls -la\",\"description\":\"List repo root\"}}}},\
         {{\"type\":\"tool_use\",\"id\":\"toolu_color\",\"name\":\"Read\",\"input\":{{\"path\":{tool_path},\"config\":{{\"nested_path\":{tool_path},\"mode\":\"read\",\"limit\":1,\"flag\":true}},\"items\":[{{\"path\":{tool_path}}}],\"ok\":true}}}},\
         {{\"type\":\"image\",\"source\":{{\"type\":\"base64\",\"data\":\"aGVsbG8=\"}},\"mime\":\"image/png\"}},\
         {{\"type\":\"attachment\",\"name\":\"preview.txt\",\"path\":{tool_path},\"mime\":\"text/plain\"}}\
         ],\"stop_reason\":\"tool_use\",\"usage\":{{\"input_tokens\":3,\"output_tokens\":5}}}}}}\n\
         {{\"type\":\"user\",\"sessionId\":{sid},\"cwd\":{cwd_json},\"timestamp\":\"2026-05-29T00:00:03.000Z\",\"uuid\":\"t1\",\"parentUuid\":\"a1\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_listing\",\"is_error\":false,\"content\":{listing_output}}},{{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_color\",\"is_error\":true,\"content\":\"permission denied\"}}]}}}}\n"
    );
    fs::write(session_dir.join(format!("{session_id}.jsonl")), content)?;
    Ok(())
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

fn make_temp_home() -> AuditResult<PathBuf> {
    let root = env::temp_dir().join(format!("cokacmux-preview-color-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&root)?;
    Ok(root)
}

fn cokacmux_path() -> AuditResult<PathBuf> {
    if let Some(path) = env::var_os("COKACMUX_BIN") {
        return Ok(PathBuf::from(path));
    }
    let bin = format!("cokacmux{}", env::consts::EXE_SUFFIX);
    if let Ok(current_exe) = env::current_exe() {
        if let Some(debug_dir) = current_exe
            .parent()
            .and_then(|examples_dir| examples_dir.parent())
        {
            let path = debug_dir.join(&bin);
            if path.exists() {
                return Ok(path);
            }
        }
    }
    let path = env::current_dir()?.join("target").join("debug").join(bin);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "missing cokacmux binary at {}; run `cargo build --bin cokacmux` first",
            path.display()
        )
        .into())
    }
}
