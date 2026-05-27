//! Briefly drive the release cokacmux.exe under a PTY and capture stderr.
//!
//! The bug we want to make sure is gone: vt100 panics flooding stderr while
//! cokacmux is running. This launches cokacmux in interactive TUI mode in a
//! PTY at a moderate size, idles for a few seconds without sending any
//! attach-to-agent key, then sends Ctrl+Q to quit and reports anything that
//! reached stderr.
//!
//! IMPORTANT: this example only opens the TUI session browser -- it never
//! presses `e` so cokacmux does not spawn a child claude.exe (which would
//! collide with the surrounding Claude Code session on Windows).
//!
//! Usage:
//!   cargo run --example drive_cokacmux --features tui -- [seconds] [cols] [rows] [out_path]

use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

fn main() -> std::io::Result<()> {
    let mut args = env::args().skip(1);
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);
    let cols: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(120);
    let rows: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let out_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("drive_cokacmux.bin"));

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| std::io::Error::other(format!("openpty: {e}")))?;

    let release_bin = std::env::current_dir()?
        .join("target")
        .join("release")
        .join("cokacmux.exe");
    if !release_bin.exists() {
        eprintln!("missing {}", release_bin.display());
        std::process::exit(2);
    }
    let mut cmd = CommandBuilder::new(release_bin);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLUMNS", cols.to_string());
    cmd.env("LINES", rows.to_string());
    cmd.env("COKACMUX_DEBUG", "1");
    cmd.cwd(std::env::current_dir()?);

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| std::io::Error::other(format!("spawn: {e}")))?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer: std::sync::Arc<std::sync::Mutex<Box<dyn Write + Send>>> =
        std::sync::Arc::new(std::sync::Mutex::new(pair.master.take_writer().unwrap()));
    let writer_for_reader = writer.clone();
    let _master = pair.master;

    // pre-answer DSR
    {
        let mut w = writer.lock().unwrap();
        let _ = w.write_all(b"\x1b[1;1R");
        let _ = w.flush();
    }

    let out_path_for_thread = out_path.clone();
    let reader_thread = std::thread::spawn(move || -> std::io::Result<(usize, usize)> {
        let mut file = File::create(&out_path_for_thread)?;
        let mut buf = [0u8; 8192];
        let mut total = 0usize;
        let mut panic_hits = 0usize;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    file.write_all(&buf[..n])?;
                    file.flush()?;
                    // count any "panicked at" string sightings inside output
                    if buf[..n]
                        .windows("panicked at".len())
                        .any(|w| w == b"panicked at")
                    {
                        panic_hits += 1;
                    }
                    if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
                        if let Ok(mut w) = writer_for_reader.lock() {
                            let _ = w.write_all(b"\x1b[1;1R");
                            let _ = w.flush();
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    eprintln!("[reader] err {e}");
                    break;
                }
            }
        }
        Ok((total, panic_hits))
    });

    std::thread::sleep(Duration::from_secs(secs.saturating_sub(1)));

    // Quit with Ctrl+Q (cokacmux quit shortcut)
    if let Ok(mut w) = writer.lock() {
        let _ = w.write_all(b"\x11");
        let _ = w.flush();
    }
    std::thread::sleep(Duration::from_millis(800));
    let _ = child.kill();
    let _ = child.wait();

    let (bytes, panic_hits) = reader_thread.join().unwrap()?;
    println!(
        "captured {} bytes to {}; panic-string hits = {}",
        bytes,
        out_path.display(),
        panic_hits
    );
    std::process::exit(if panic_hits == 0 { 0 } else { 1 });
}
