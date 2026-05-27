//! Capture raw Claude Code PTY output bytes to a file so we can replay them
//! against vt100::Parser and pinpoint the exact bytes that panic.
//!
//! Usage:
//!   cargo run --example capture_claude --features tui -- [cols] [rows] [seconds] [out_path]
//! Defaults: cols=33, rows=10, seconds=5, out_path=capture.bin

use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

fn main() -> std::io::Result<()> {
    let mut args = env::args().skip(1);
    let cols: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(33);
    let rows: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let out_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("capture.bin"));

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| std::io::Error::other(format!("openpty: {e}")))?;

    // Allow overriding the program via $CAPTURE_PROG / $CAPTURE_ARGS for testing
    let prog = std::env::var("CAPTURE_PROG").unwrap_or_else(|_| {
        if cfg!(windows) {
            "claude.exe".into()
        } else {
            "claude".into()
        }
    });
    let extra_args = std::env::var("CAPTURE_ARGS").unwrap_or_default();

    let mut cmd = CommandBuilder::new(&prog);
    for a in extra_args.split_whitespace() {
        cmd.arg(a);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLUMNS", cols.to_string());
    cmd.env("LINES", rows.to_string());
    // cwd: pick a writable dir
    cmd.cwd(std::env::current_dir()?);

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| std::io::Error::other(format!("spawn: {e}")))?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let _master = pair.master;

    // Some applications (claude.exe included) emit ESC[6n cursor-position
    // queries on startup and block waiting for the response. Pre-empt that
    // by writing a plausible answer immediately. We then keep watching the
    // stream and replying to any further queries.
    let _ = writer.write_all(b"\x1b[1;1R");
    let _ = writer.flush();

    let out_path_for_thread = out_path.clone();
    let writer_clone: std::sync::Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>> =
        std::sync::Arc::new(std::sync::Mutex::new(writer));
    let writer_for_reader = writer_clone.clone();
    let reader_thread = std::thread::spawn(move || -> std::io::Result<usize> {
        let mut file = File::create(&out_path_for_thread)?;
        let mut buf = [0u8; 8192];
        let mut total = 0;
        let mut reads = 0;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            match reader.read(&mut buf) {
                Ok(0) => {
                    eprintln!("[reader] EOF after {} reads, {} bytes", reads, total);
                    break;
                }
                Ok(n) => {
                    reads += 1;
                    total += n;
                    eprintln!("[reader] read#{} n={} total={}", reads, n, total);
                    file.write_all(&buf[..n])?;
                    file.flush()?;
                    // Respond to ESC[6n (DSR) queries
                    if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
                        if let Ok(mut w) = writer_for_reader.lock() {
                            let _ = w.write_all(b"\x1b[1;1R");
                            let _ = w.flush();
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    eprintln!("[reader] read error: {e}");
                    break;
                }
            }
        }
        eprintln!("[reader] exiting; total reads={} bytes={}", reads, total);
        Ok(total)
    });

    // Give the agent time to draw its banner.
    std::thread::sleep(Duration::from_secs(secs.saturating_sub(1)));

    // Try to gracefully quit by sending Ctrl-C twice (Claude Code's exit shortcut).
    if let Ok(mut w) = writer_clone.lock() {
        let _ = w.write_all(&[0x03]);
        let _ = w.flush();
        std::thread::sleep(Duration::from_millis(200));
        let _ = w.write_all(&[0x03]);
        let _ = w.flush();
    }

    std::thread::sleep(Duration::from_millis(800));
    let _ = child.kill();
    let _ = child.wait();

    let total = reader_thread.join().unwrap()?;
    println!("captured {} bytes to {}", total, out_path.display());
    Ok(())
}
