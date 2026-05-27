//! Replay a captured PTY byte stream through vt100::Parser and find the
//! exact panic location.
//!
//! Usage:
//!   cargo run --example vt100_replay --features tui -- <capture.bin> [cols] [rows]
//! Defaults: cols=33, rows=10

use std::env;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

static HOOK_VT100: AtomicUsize = AtomicUsize::new(0);
static HOOK_OTHER: AtomicUsize = AtomicUsize::new(0);

fn install_filter() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let file = info.location().map(|l| l.file()).unwrap_or("");
        if file.contains("vt100-") {
            HOOK_VT100.fetch_add(1, Ordering::SeqCst);
            return; // suppress
        }
        HOOK_OTHER.fetch_add(1, Ordering::SeqCst);
        default_hook(info);
    }));
}

fn try_chunk(parser: &mut vt100::Parser, bytes: &[u8]) -> bool {
    matches!(
        catch_unwind(AssertUnwindSafe(|| parser.process(bytes))),
        Ok(())
    )
}

fn replay_whole(label: &str, cols: u16, rows: u16, bytes: &[u8]) {
    let before = HOOK_VT100.load(Ordering::SeqCst);
    let mut parser = vt100::Parser::new(rows, cols, 0);
    let ok = try_chunk(&mut parser, bytes);
    let after = HOOK_VT100.load(Ordering::SeqCst);
    println!(
        "[{}] cols={} rows={} bytes={} -> ok={} hook_vt100_delta={}",
        label,
        cols,
        rows,
        bytes.len(),
        ok,
        after - before
    );
}

fn bisect_panic(cols: u16, rows: u16, bytes: &[u8]) {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    let mut offset = 0usize;
    let chunk_size = 1usize;
    while offset < bytes.len() {
        let end = (offset + chunk_size).min(bytes.len());
        let chunk = &bytes[offset..end];
        let panicked_before = HOOK_VT100.load(Ordering::SeqCst);
        let ok = try_chunk(&mut parser, chunk);
        let panicked_after = HOOK_VT100.load(Ordering::SeqCst);
        if !ok || panicked_after > panicked_before {
            // Provide context: previous 32 bytes plus the panic byte
            let ctx_start = offset.saturating_sub(32);
            println!(
                "[bisect cols={}] PANIC at byte offset {} (byte 0x{:02x}). Last 32 bytes: {:?}",
                cols,
                offset,
                bytes[offset],
                &bytes[ctx_start..end]
            );
            println!(
                "  -> printable: {:?}",
                String::from_utf8_lossy(&bytes[ctx_start..end])
            );
            // Try to keep going so we find subsequent panics, but the parser
            // state may be corrupted.
            return;
        }
        offset = end;
    }
    println!(
        "[bisect cols={}] no panic across {} bytes",
        cols,
        bytes.len()
    );
}

fn replay_with_resizes(label: &str, bytes: &[u8], schedule: &[(u16, u16)]) {
    // schedule: list of (cols, rows) checkpoints. We split the input into
    // equal chunks across the schedule -- between each chunk we set_size.
    let before = HOOK_VT100.load(Ordering::SeqCst);
    let first = schedule[0];
    let mut parser = vt100::Parser::new(first.1, first.0, 0);
    let chunk = bytes.len() / schedule.len().max(1);
    let mut offset = 0;
    let mut panicked_at: Option<(usize, (u16, u16))> = None;
    for (i, (c, r)) in schedule.iter().copied().enumerate() {
        parser.screen_mut().set_size(r, c);
        let end = if i + 1 == schedule.len() {
            bytes.len()
        } else {
            (offset + chunk).min(bytes.len())
        };
        let slice = &bytes[offset..end];
        let _ = try_chunk(&mut parser, slice);
        let after = HOOK_VT100.load(Ordering::SeqCst);
        if after > before {
            panicked_at = Some((end, (c, r)));
            break;
        }
        offset = end;
    }
    let after = HOOK_VT100.load(Ordering::SeqCst);
    println!(
        "[{}] schedule={:?} -> panic={:?} hook_vt100_delta={}",
        label,
        schedule,
        panicked_at,
        after - before
    );
}

fn main() {
    install_filter();
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .expect("usage: vt100_replay <capture.bin> [cols] [rows]");
    let cols: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);
    let rows: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);

    let bytes = std::fs::read(&path).expect("read capture");
    println!("loaded {} bytes from {}", bytes.len(), path);

    // 1) whole-stream replay across a range of widths
    for w in [
        16u16, 17, 18, 19, 20, 21, 22, 24, 26, 28, 30, 32, 33, 34, 40, 50, 60, 80,
    ] {
        replay_whole(&format!("whole_w{}", w), w, rows, &bytes);
    }

    println!("---");

    // 2) Replay with resize schedule -- this mimics the user's actual usage:
    //    sidebar is being adjusted, so PTY size changes repeatedly.
    let schedules: Vec<Vec<(u16, u16)>> = vec![
        // start narrow, widen
        vec![(20, 10), (22, 10), (24, 10), (28, 10), (32, 10), (40, 10)],
        // start wide, narrow
        vec![(60, 10), (40, 10), (32, 10), (24, 10), (20, 10)],
        // oscillate
        vec![(20, 10), (40, 10), (20, 10), (40, 10), (20, 10)],
        // increment by 1 across the box-drawing boundary
        vec![(18, 10), (19, 10), (20, 10), (21, 10), (22, 10), (23, 10)],
        // rapid jumps of 2 (matches Alt+arrow resize step in cokacmux)
        vec![
            (20, 5),
            (22, 5),
            (24, 5),
            (26, 5),
            (28, 5),
            (30, 5),
            (32, 5),
            (34, 5),
            (36, 5),
        ],
        // small height extremes
        vec![(20, 1), (20, 2), (20, 3), (20, 5)],
    ];
    for (i, sched) in schedules.iter().enumerate() {
        replay_with_resizes(&format!("resize_{}", i), &bytes, sched);
    }

    println!("---");

    // 3) byte-by-byte bisection at the captured width to pinpoint the
    //    panicking sequence
    for w in [cols, 22, 24, 32, 80] {
        bisect_panic(w, rows, &bytes);
    }

    println!("---");
    println!(
        "totals: hook_vt100={} hook_other={}",
        HOOK_VT100.load(Ordering::SeqCst),
        HOOK_OTHER.load(Ordering::SeqCst)
    );
}
