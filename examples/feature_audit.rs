//! Autonomous PTY-based audit of cokacmux's user-facing features.
//!
//! For each test:
//!  - launch a fresh cokacmux under a PTY at a chosen size
//!  - send a scripted sequence of keystrokes
//!  - capture the rendered output until quit
//!  - assert: no "panicked at" / no "thread panicked"; expected markers present
//!  - process exited with status 0
//!
//! Safety:
//!  - settings.json is backed up before the run and restored after.
//!  - destructive / process-spawning confirmations are NOT accepted.
//!  - 'e' and Ctrl+N are opened then canceled when exercised.
//!  - 'c' (clone), 't' (edit title), 'd' (delete), Ctrl+K (kill), and Ctrl+]
//!    (detach) are not exercised.
//!
//! Usage:
//!   cargo run --release --example feature_audit --features tui
//!
//! Optional:
//!   COKACMUX_BIN=/path/to/cokacmux cargo run --release --example feature_audit --features tui
//!
//! Exits with non-zero status if any test fails.

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

// ASCII control byte helpers
const ESC: u8 = 0x1B;
const TAB: u8 = 0x09;
const ENTER: u8 = b'\r';
const CTRL_N: u8 = 0x0E;
const CTRL_Q: u8 = 0x11;

// CSI modifier encodings used by common terminals:
//   3 = Alt, 6 = Ctrl+Shift.
const ALT_UP: &[u8] = b"\x1b[1;3A";
const ALT_DOWN: &[u8] = b"\x1b[1;3B";
const ALT_RIGHT: &[u8] = b"\x1b[1;3C";
const ALT_LEFT: &[u8] = b"\x1b[1;3D";
const CTRL_SHIFT_UP: &[u8] = b"\x1b[1;6A";
const CTRL_SHIFT_DOWN: &[u8] = b"\x1b[1;6B";
const CTRL_SHIFT_RIGHT: &[u8] = b"\x1b[1;6C";
const CTRL_SHIFT_LEFT: &[u8] = b"\x1b[1;6D";

/// One step of input to send.
enum Step {
    /// Raw bytes (ASCII or CSI escape).
    Send(&'static [u8]),
    /// Pause for N ms to let cokacmux render.
    Wait(u64),
}

struct TestCase {
    name: &'static str,
    cols: u16,
    rows: u16,
    /// Initial settle time after spawn before sending input.
    boot_ms: u64,
    /// Scripted keystrokes.
    steps: &'static [Step],
    /// Substrings that MUST appear in the captured output.
    expect_present: &'static [&'static str],
    /// Substrings that MUST NOT appear.
    expect_absent: &'static [&'static str],
    /// CLI args; empty means TUI mode (we send Ctrl-Q to quit).
    cli_args: &'static [&'static str],
    /// If true, we expect the process to exit on its own without us sending quit.
    expects_self_exit: bool,
}

struct TestOutcome {
    name: &'static str,
    passed: bool,
    detail: String,
}

fn cokacmux_path() -> PathBuf {
    if let Some(path) = env::var_os("COKACMUX_BIN") {
        return PathBuf::from(path);
    }

    let bin_name = format!("cokacmux{}", env::consts::EXE_SUFFIX);
    if let Ok(current_exe) = env::current_exe() {
        if let Some(release_dir) = current_exe
            .parent()
            .and_then(|examples_dir| examples_dir.parent())
        {
            return release_dir.join(&bin_name);
        }
    }

    env::current_dir()
        .unwrap()
        .join("target")
        .join("release")
        .join(bin_name)
}

fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".cokacmux").join("settings.json"))
}

fn run_test(tc: &TestCase) -> TestOutcome {
    let exe = cokacmux_path();
    if !exe.exists() {
        return TestOutcome {
            name: tc.name,
            passed: false,
            detail: format!("missing binary at {}", exe.display()),
        };
    }

    let pty = NativePtySystem::default();
    let pair = match pty.openpty(PtySize {
        cols: tc.cols,
        rows: tc.rows,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            return TestOutcome {
                name: tc.name,
                passed: false,
                detail: format!("openpty: {e}"),
            };
        }
    };

    let mut cmd = CommandBuilder::new(&exe);
    for a in tc.cli_args {
        cmd.arg(a);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLUMNS", tc.cols.to_string());
    cmd.env("LINES", tc.rows.to_string());
    cmd.env("COKACMUX_DEBUG", "0");
    cmd.cwd(env::current_dir().unwrap());

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            return TestOutcome {
                name: tc.name,
                passed: false,
                detail: format!("spawn: {e}"),
            };
        }
    };
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
    let writer_clone = writer.clone();
    let master_holder: Arc<Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>> =
        Arc::new(Mutex::new(Some(pair.master)));

    // pre-answer DSR
    {
        let mut w = writer.lock().unwrap();
        let _ = w.write_all(b"\x1b[1;1R");
        let _ = w.flush();
    }

    let buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::with_capacity(64 * 1024)));
    let buf_clone = buffer.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut tmp = [0u8; 8192];
        loop {
            match reader.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    let slice = &tmp[..n];
                    if let Ok(mut buf) = buf_clone.lock() {
                        buf.extend_from_slice(slice);
                    }
                    // auto-respond to DSR queries
                    if slice.windows(4).any(|w| w == b"\x1b[6n") {
                        if let Ok(mut w) = writer_clone.lock() {
                            let _ = w.write_all(b"\x1b[1;1R");
                            let _ = w.flush();
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    std::thread::sleep(Duration::from_millis(tc.boot_ms));

    for step in tc.steps {
        match step {
            Step::Send(bytes) => {
                if let Ok(mut w) = writer.lock() {
                    let _ = w.write_all(bytes);
                    let _ = w.flush();
                }
            }
            Step::Wait(ms) => std::thread::sleep(Duration::from_millis(*ms)),
        }
    }

    let exit_status;
    if tc.expects_self_exit {
        // Wait up to 5s for self-exit
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(s)) => {
                    exit_status = Some(s);
                    break;
                }
                Ok(None) => {
                    if start.elapsed() > Duration::from_secs(5) {
                        let _ = child.kill();
                        exit_status = child.wait().ok();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => {
                    exit_status = None;
                    break;
                }
            }
        }
    } else {
        // Send Ctrl+Q to quit
        if let Ok(mut w) = writer.lock() {
            let _ = w.write_all(&[CTRL_Q]);
            let _ = w.flush();
        }
        // Wait up to 3s for clean exit
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(s)) => {
                    exit_status = Some(s);
                    break;
                }
                Ok(None) => {
                    if start.elapsed() > Duration::from_secs(3) {
                        let _ = child.kill();
                        exit_status = child.wait().ok();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => {
                    exit_status = None;
                    break;
                }
            }
        }
    }

    // Give the reader a moment to drain, then drop master/writer so the
    // ConPTY closes and the reader sees EOF.
    std::thread::sleep(Duration::from_millis(200));
    drop(writer);
    if let Ok(mut guard) = master_holder.lock() {
        guard.take();
    }
    // Poll for thread completion with a hard timeout. We do not call
    // join() blindly because portable-pty on Windows ConPTY sometimes
    // keeps the read pipe open after the slave exits, which would hang
    // the runner. Letting the thread leak is harmless -- it'll exit when
    // the process does.
    let join_start = Instant::now();
    while !reader_thread.is_finished() && join_start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if reader_thread.is_finished() {
        let _ = reader_thread.join();
    }

    let captured = buffer.lock().unwrap().clone();
    let captured_str = String::from_utf8_lossy(&captured);

    // Universal failure checks
    if captured_str.contains("panicked at") {
        return TestOutcome {
            name: tc.name,
            passed: false,
            detail: format!(
                "captured 'panicked at' substring. exit={:?} bytes={}",
                exit_status,
                captured.len()
            ),
        };
    }
    if captured_str.contains("thread main panicked") {
        return TestOutcome {
            name: tc.name,
            passed: false,
            detail: "captured 'thread main panicked'".to_string(),
        };
    }
    if captured_str.contains("RUST_BACKTRACE") && captured_str.contains("backtrace") {
        return TestOutcome {
            name: tc.name,
            passed: false,
            detail: "captured backtrace".to_string(),
        };
    }

    for needle in tc.expect_present {
        if !captured_str.contains(needle) {
            return TestOutcome {
                name: tc.name,
                passed: false,
                detail: format!(
                    "expected substring not found: {:?}. exit={:?} bytes={}",
                    needle,
                    exit_status,
                    captured.len()
                ),
            };
        }
    }
    for needle in tc.expect_absent {
        if captured_str.contains(needle) {
            return TestOutcome {
                name: tc.name,
                passed: false,
                detail: format!("forbidden substring found: {:?}", needle),
            };
        }
    }

    let exited_ok = exit_status.as_ref().map(|s| s.success()).unwrap_or(false);
    if !exited_ok {
        return TestOutcome {
            name: tc.name,
            passed: false,
            detail: format!("non-zero exit: {:?}", exit_status),
        };
    }

    TestOutcome {
        name: tc.name,
        passed: true,
        detail: format!("ok ({} bytes)", captured.len()),
    }
}

fn main() {
    // ------------------------------------------------------------------ backup
    let mut backup: Option<(PathBuf, Vec<u8>)> = None;
    if let Some(p) = settings_path() {
        if p.exists() {
            if let Ok(data) = fs::read(&p) {
                backup = Some((p.clone(), data));
                eprintln!("[setup] backed up {}", p.display());
            }
        }
    }

    // ------------------------------------------------------------------ tests
    let tests: Vec<TestCase> = vec![
        // --- CLI mode tests (process exits on its own) ----------------
        TestCase {
            name: "cli_version",
            cols: 80,
            rows: 24,
            boot_ms: 200,
            steps: &[],
            expect_present: &["cokacmux "],
            expect_absent: &[],
            cli_args: &["--version"],
            expects_self_exit: true,
        },
        TestCase {
            name: "cli_help",
            cols: 80,
            rows: 24,
            boot_ms: 200,
            steps: &[],
            expect_present: &["USAGE", "INTERACTIVE KEYS", "cokacmux killall"],
            expect_absent: &[],
            cli_args: &["--help"],
            expects_self_exit: true,
        },
        TestCase {
            name: "cli_check",
            cols: 80,
            rows: 24,
            boot_ms: 500,
            steps: &[],
            expect_present: &["sessions discovered"],
            expect_absent: &[],
            cli_args: &["--check"],
            expects_self_exit: true,
        },
        // --- TUI: basic boot ------------------------------------------
        TestCase {
            name: "tui_boot_120x30",
            cols: 120,
            rows: 30,
            boot_ms: 1500,
            steps: &[],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "tui_boot_40x10",
            cols: 40,
            rows: 10,
            boot_ms: 1500,
            steps: &[],
            expect_present: &[],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "tui_boot_200x60",
            cols: 200,
            rows: 60,
            boot_ms: 1500,
            steps: &[],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "tui_boot_20x5_tiny",
            cols: 20,
            rows: 5,
            boot_ms: 1500,
            steps: &[],
            expect_present: &[],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- navigation ----------------------------------------------
        TestCase {
            name: "nav_down_x5",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"\x1b[B"),
                Step::Wait(80),
                Step::Send(b"\x1b[B"),
                Step::Wait(80),
                Step::Send(b"\x1b[B"),
                Step::Wait(80),
                Step::Send(b"\x1b[B"),
                Step::Wait(80),
                Step::Send(b"\x1b[B"),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "nav_up_past_top",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"\x1b[A"),
                Step::Wait(60),
                Step::Send(b"\x1b[A"),
                Step::Wait(60),
                Step::Send(b"\x1b[A"),
                Step::Wait(60),
                Step::Send(b"\x1b[A"),
                Step::Wait(60),
                Step::Send(b"\x1b[A"),
                Step::Wait(60),
                Step::Send(b"\x1b[A"),
                Step::Wait(60),
                Step::Send(b"\x1b[A"),
                Step::Wait(60),
                Step::Send(b"\x1b[A"),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "nav_jk",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"jjjjj"),
                Step::Wait(200),
                Step::Send(b"kkk"),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "nav_pgdown_pgup",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"\x1b[6~"),
                Step::Wait(150), // PgDn
                Step::Send(b"\x1b[6~"),
                Step::Wait(150),
                Step::Send(b"\x1b[5~"),
                Step::Wait(200), // PgUp
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "nav_home_end",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"G"),
                Step::Wait(200), // End
                Step::Send(b"g"),
                Step::Wait(200), // Home (g lowercase)
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "shortcut_alt_sidebar_select",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(ALT_DOWN),
                Step::Wait(120),
                Step::Send(ALT_DOWN),
                Step::Wait(120),
                Step::Send(ALT_UP),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "shortcut_ctrl_shift_sidebar_select",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(CTRL_SHIFT_DOWN),
                Step::Wait(120),
                Step::Send(CTRL_SHIFT_DOWN),
                Step::Wait(120),
                Step::Send(CTRL_SHIFT_UP),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "shortcut_alt_resize_panes",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(ALT_RIGHT),
                Step::Wait(120),
                Step::Send(ALT_RIGHT),
                Step::Wait(120),
                Step::Send(ALT_LEFT),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "shortcut_ctrl_shift_resize_panes",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(CTRL_SHIFT_RIGHT),
                Step::Wait(120),
                Step::Send(CTRL_SHIFT_RIGHT),
                Step::Wait(120),
                Step::Send(CTRL_SHIFT_LEFT),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "shortcut_ctrl_shift_select_from_preview",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(&[TAB]),
                Step::Wait(120),
                Step::Send(CTRL_SHIFT_DOWN),
                Step::Wait(120),
                Step::Send(CTRL_SHIFT_UP),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- focus toggling ------------------------------------------
        TestCase {
            name: "focus_tab_esc",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(&[TAB]),
                Step::Wait(200),
                Step::Send(&[TAB]),
                Step::Wait(200),
                Step::Send(&[ESC]),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- preview summary toggle (Enter) ---------------------------
        TestCase {
            name: "preview_enter_toggle",
            cols: 120,
            rows: 30,
            boot_ms: 1500,
            steps: &[
                Step::Send(&[ENTER]),
                Step::Wait(400),
                Step::Send(&[ENTER]),
                Step::Wait(300),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- view toggle ---------------------------------------------
        TestCase {
            name: "view_toggle_v",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"v"),
                Step::Wait(300),
                Step::Send(b"v"),
                Step::Wait(300),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- process start dialogs opened and canceled ----------------
        TestCase {
            name: "new_session_dialog_open_cancel",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(&[CTRL_N]),
                Step::Wait(400),
                Step::Send(&[ESC]),
                Step::Wait(200),
            ],
            expect_present: &["New session", "Choose what to start", "Terminal", "Folder"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "agent_launch_dialog_open_cancel",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"e"),
                Step::Wait(400),
                Step::Send(&[ESC]),
                Step::Wait(200),
            ],
            expect_present: &["Agent launch", "Start/attach", "Normal", "Skip permissions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- filter ---------------------------------------------------
        TestCase {
            name: "filter_open_and_type",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"/"),
                Step::Wait(200),
                Step::Send(b"claude"),
                Step::Wait(400),
                Step::Send(&[ESC]),
                Step::Wait(200), // close filter
            ],
            expect_present: &["sessions", "Search sessions", "Search", "Cancel"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "filter_no_match",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"/"),
                Step::Wait(200),
                Step::Send(b"zzzz_no_match_string_xxxx"),
                Step::Wait(400),
                Step::Send(&[ESC]),
                Step::Wait(200),
            ],
            expect_present: &[],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- refresh --------------------------------------------------
        TestCase {
            name: "refresh_r",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[Step::Send(b"r"), Step::Wait(800)],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- random key smash ---------------------------------------
        TestCase {
            name: "random_key_smash",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"abfghilmnop123!@#$%^&*()"),
                Step::Wait(200),
                Step::Send(b"\x1b[Z"),
                Step::Wait(100), // Shift+Tab
                Step::Send(b"\x7f"),
                Step::Wait(100), // Backspace
                Step::Send(&[ENTER]),
                Step::Wait(200),
                Step::Send(&[ESC]),
                Step::Wait(200),
            ],
            expect_present: &[],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- combined workflow ---------------------------------------
        TestCase {
            name: "workflow_filter_then_nav",
            cols: 120,
            rows: 30,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"/"),
                Step::Wait(150),
                Step::Send(b"c"),
                Step::Wait(150),
                Step::Send(&[ENTER]),
                Step::Wait(5000), // confirm async full-session search
                Step::Send(b"jjj"),
                Step::Wait(150),
                Step::Send(b"kk"),
                Step::Wait(150),
                Step::Send(&[TAB]),
                Step::Wait(200),
                Step::Send(&[TAB]),
                Step::Wait(200),
            ],
            expect_present: &["Search sessions", "search=c"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        // --- mid-run resize ------------------------------------------
        // We can't actually resize a portable-pty under crossterm easily,
        // but we can re-launch at the boundary sizes that the bug fix targets.
        TestCase {
            name: "tiny_with_nav",
            cols: 30,
            rows: 8,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"jjjj"),
                Step::Wait(200),
                Step::Send(b"v"),
                Step::Wait(200),
                Step::Send(b"v"),
                Step::Wait(200),
            ],
            expect_present: &[],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "boundary_77_cols",
            cols: 77,
            rows: 24,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"jjjj"),
                Step::Wait(150),
                Step::Send(&[ENTER]),
                Step::Wait(200),
                Step::Send(&[ENTER]),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
        TestCase {
            name: "boundary_76_cols",
            cols: 76,
            rows: 24,
            boot_ms: 1200,
            steps: &[
                Step::Send(b"jjjj"),
                Step::Wait(150),
                Step::Send(b"v"),
                Step::Wait(200),
            ],
            expect_present: &["sessions"],
            expect_absent: &[],
            cli_args: &[],
            expects_self_exit: false,
        },
    ];

    // ------------------------------------------------------------------ run
    let mut outcomes: Vec<TestOutcome> = Vec::new();
    let total = tests.len();
    let mut passed_count = 0usize;
    for (i, tc) in tests.iter().enumerate() {
        eprintln!("[{}/{}] {} ...", i + 1, total, tc.name);
        let _ = std::io::Write::flush(&mut std::io::stderr());
        let o = run_test(tc);
        if o.passed {
            passed_count += 1;
            eprintln!("    PASS - {}", o.detail);
        } else {
            eprintln!("    FAIL - {}", o.detail);
        }
        let _ = std::io::Write::flush(&mut std::io::stderr());
        outcomes.push(o);
    }

    // ------------------------------------------------------------------ restore
    if let Some((path, data)) = backup {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, &data);
        eprintln!("[teardown] restored {}", path.display());
    }

    // ------------------------------------------------------------------ report
    println!();
    println!("==============================");
    println!("  FEATURE AUDIT  {}/{} passed", passed_count, total);
    println!("==============================");
    for o in &outcomes {
        let tag = if o.passed { "PASS" } else { "FAIL" };
        println!("  {} {} - {}", tag, o.name, o.detail);
    }
    if passed_count != total {
        std::process::exit(1);
    }
}

// dirs crate access
#[allow(unused_imports)]
use std::path::Path as _Path;
mod dirs {
    use std::path::PathBuf;
    pub fn home_dir() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    }
}
