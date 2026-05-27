//! Standalone reproducer for the vt100-0.16 panic.
//!
//! Goal: prove (a) which inputs panic vt100::Parser::process(), and (b) that
//! a panic-hook + catch_unwind wrapper keeps stderr quiet.
//!
//! Run with: cargo run --example vt100_repro --features tui

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

static HOOK_FIRED: AtomicUsize = AtomicUsize::new(0);
static HOOK_VT100: AtomicUsize = AtomicUsize::new(0);

fn install_filter() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        HOOK_FIRED.fetch_add(1, Ordering::SeqCst);
        let file = info.location().map(|l| l.file()).unwrap_or("");
        if file.contains("vt100-") {
            HOOK_VT100.fetch_add(1, Ordering::SeqCst);
            return; // suppress
        }
        default_hook(info);
    }));
}

fn safe_process(parser: &mut vt100::Parser, bytes: &[u8]) -> bool {
    match catch_unwind(AssertUnwindSafe(|| parser.process(bytes))) {
        Ok(()) => true,
        Err(_) => false,
    }
}

fn try_case(label: &str, cols: u16, rows: u16, bytes: &[u8]) {
    let before_fired = HOOK_FIRED.load(Ordering::SeqCst);
    let before_vt = HOOK_VT100.load(Ordering::SeqCst);
    let mut parser = vt100::Parser::new(rows, cols, 0);
    let ok = safe_process(&mut parser, bytes);
    let fired = HOOK_FIRED.load(Ordering::SeqCst) - before_fired;
    let vt = HOOK_VT100.load(Ordering::SeqCst) - before_vt;
    println!(
        "[{}] cols={} rows={} len={} -> ok={} hook_fired={} hook_vt100={}",
        label,
        cols,
        rows,
        bytes.len(),
        ok,
        fired,
        vt
    );
}

fn main() {
    install_filter();

    // Case A: Just an ASCII string -- should never panic.
    try_case("ascii", 80, 24, b"hello world\r\n");

    // Case B: Claude Code banner snippet at exact terminal width (32 cols).
    // The corner glyph + horizontal rules fit exactly.
    let banner_32 = "\x1b[H\x1b[38;2;215;119;87m\
        \u{256d}\u{2500} Claude Code \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256e}\r\n\
        \u{2502}                              \u{2502}\r\n";
    try_case("banner_fit_32", 32, 10, banner_32.as_bytes());

    // Case C: Banner narrower than terminal -- no edge issue expected.
    try_case("banner_narrow_in_wide", 80, 10, banner_32.as_bytes());

    // Case D: Force a wide CJK char at the very last column.
    //   Move cursor to col 80 (1-based) on row 1, then emit a CJK wide char.
    let cjk_edge = b"\x1b[1;80H\xe6\xbc\xa2"; // U+6F22 "漢"
    try_case("cjk_at_last_col_80", 80, 24, cjk_edge);

    // Case E: CJK char at col cols, where parser cols=80.
    //   Move to col 80 then write a wide char; col+1 == 81 -> out of bounds.
    try_case("cjk_at_col80_w80", 80, 24, cjk_edge);

    // Case F: CJK char at last column on a tiny grid (cols=4).
    let cjk_small = b"\x1b[1;4H\xe6\xbc\xa2";
    try_case("cjk_last_col_4", 4, 3, cjk_small);

    // Case G: Real Claude Code banner sample
    let real_banner = b"\x1b[2J\x1b[H\x1b[?25l\x1b[m\x1b[H\x1b[J\x1b[38;2;215;119;87m\xe2\x95\xad\xe2\x94\x80 Claude Code \xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x95\xae\r\n\xe2\x94\x82                  \xe2\x94\x82\r\n";
    try_case("real_banner_w20", 20, 5, real_banner);
    try_case("real_banner_w18", 18, 5, real_banner);
    try_case("real_banner_w17", 17, 5, real_banner);
    try_case("real_banner_w16", 16, 5, real_banner);

    println!("---");
    println!("--- resize/state-corruption scenarios ---");

    // Case H: write a wide char filling last 2 columns, then RESIZE to
    // shrink cols by 1 -- this leaves a "wide" cell at the new last column
    // with no room for its continuation. Subsequent writes that hit that
    // cell should panic.
    let before = HOOK_FIRED.load(Ordering::SeqCst);
    let mut parser = vt100::Parser::new(5, 20, 0);
    // Move cursor to col 18 (1-based 19), then write 漢 (width 2). The
    // wide char occupies cols 18, 19 (0-based).
    parser.process(b"\x1b[1;19H\xe6\xbc\xa2");
    println!(
        "after wide-at-edge write: pos={:?} cell18_wide={} cell19_continuation={}",
        parser.screen().cursor_position(),
        parser
            .screen()
            .cell(0, 18)
            .map(|c| c.is_wide())
            .unwrap_or(false),
        parser
            .screen()
            .cell(0, 19)
            .map(|c| c.is_wide_continuation())
            .unwrap_or(false)
    );
    // Shrink to 19 cols -- now col 18 is the LAST col, but still is_wide.
    parser.screen_mut().set_size(5, 19);
    println!(
        "after shrink to 19: cell18_wide={}",
        parser
            .screen()
            .cell(0, 18)
            .map(|c| c.is_wide())
            .unwrap_or(false)
    );
    // Now write a NEW wide char at col 18 -- this triggers the "cell at pos is_wide, need to clear pos+1" path.
    parser.process(b"\x1b[1;19H\xe6\xbc\xa2");
    let after = HOOK_FIRED.load(Ordering::SeqCst);
    println!("wide_then_shrink panic? hook_delta={}", after - before);

    // Case I: same idea but multiple shrinks
    let before = HOOK_FIRED.load(Ordering::SeqCst);
    let mut parser = vt100::Parser::new(5, 30, 0);
    parser.process(b"\x1b[1;29H\xe6\xbc\xa2"); // wide at col 28-29
    parser.screen_mut().set_size(5, 29);
    parser.screen_mut().set_size(5, 28);
    parser.process(b"\x1b[1;28HX");
    let after = HOOK_FIRED.load(Ordering::SeqCst);
    println!(
        "wide_then_multi_shrink panic? hook_delta={}",
        after - before
    );

    // Case J: char that's wide in some tables but not in unicode-width.
    //   Test some box drawing chars that might be flagged as wide on some
    //   systems. Try writing each at the last col.
    for (name, ch) in &[
        ("box_corner_DL", '\u{256E}'), // ╮
        ("box_corner_UL", '\u{256D}'), // ╭
        ("box_vert", '\u{2502}'),      // │
        ("block_full", '\u{2588}'),    // █
        ("eight_spoked", '\u{2733}'),  // ✳
        ("middle_dot", '\u{00B7}'),    // ·
        ("kanji_kan", '\u{6F22}'),     // 漢 (definitely wide)
        ("emoji_grin", '\u{1F600}'),   // 😀
    ] {
        let before = HOOK_FIRED.load(Ordering::SeqCst);
        let mut parser = vt100::Parser::new(5, 20, 0);
        // place cursor at last col
        parser.process(b"\x1b[1;20H");
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        parser.process(s.as_bytes());
        let after = HOOK_FIRED.load(Ordering::SeqCst);
        println!(
            "char_at_last_col[{}] U+{:04X} hook_delta={}",
            name,
            *ch as u32,
            after - before
        );
    }

    println!("---");
    println!("--- erase/clear_wide after shrink ---");

    // Case K: write a wide char near the right edge, shrink cols by 1 so
    // the wide-flagged cell lands at the new last column, then issue an
    // escape sequence that calls row.erase() at that column.
    //   ECH ("\x1b[NX") erases N characters starting at cursor without
    //   moving the cursor. It calls row.erase under the hood, which calls
    //   clear_wide which indexes cells[col+1] -> panic.
    let cases: &[(&str, u16, u16, &[u8], u16, u16, &[u8])] = &[
        // (label, start_cols, rows, init bytes, new_cols, new_rows, post bytes)
        (
            "ECH_after_shrink_w20to19",
            20,
            5,
            b"\x1b[1;19H\xe6\xbc\xa2", // 漢 at col 18-19 (0-based)
            19,
            5,
            b"\x1b[1;19H\x1b[1X", // cursor col19 (=18 0-based, last), erase 1 char
        ),
        (
            "CHA_then_overwrite_w20to19",
            20,
            5,
            b"\x1b[1;19H\xe6\xbc\xa2",
            19,
            5,
            b"\x1b[1;19HA",
        ),
        (
            "EL_after_shrink_w20to19",
            20,
            5,
            b"\x1b[1;19H\xe6\xbc\xa2",
            19,
            5,
            b"\x1b[1;19H\x1b[K", // EL erase line from cursor
        ),
        (
            "DCH_after_shrink_w20to19",
            20,
            5,
            b"\x1b[1;19H\xe6\xbc\xa2",
            19,
            5,
            b"\x1b[1;19H\x1b[1P", // DCH delete 1 char
        ),
        // width 76 (matches the agent.log "len 76 index 76" panic)
        (
            "ECH_after_shrink_w77to76",
            77,
            5,
            b"\x1b[1;76H\xe6\xbc\xa2", // wide at col 75-76
            76,
            5,
            b"\x1b[1;76H\x1b[1X",
        ),
        // emoji - should be width 2
        (
            "emoji_then_overwrite",
            20,
            5,
            b"\x1b[1;19H\xf0\x9f\x98\x80", // 😀 at col 18-19
            19,
            5,
            b"\x1b[1;19HA",
        ),
    ];
    for (label, c0, r0, init, c1, r1, post) in cases {
        let before = HOOK_FIRED.load(Ordering::SeqCst);
        let mut parser = vt100::Parser::new(*r0, *c0, 0);
        let ok_init = matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parser.process(init))),
            Ok(())
        );
        parser.screen_mut().set_size(*r1, *c1);
        let ok_post = matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parser.process(post))),
            Ok(())
        );
        let after = HOOK_FIRED.load(Ordering::SeqCst);
        println!(
            "[{}] ok_init={} ok_post={} hook_delta={}",
            label,
            ok_init,
            ok_post,
            after - before
        );
    }

    println!("---");
    println!(
        "totals: hook_fired={} hook_vt100={}",
        HOOK_FIRED.load(Ordering::SeqCst),
        HOOK_VT100.load(Ordering::SeqCst)
    );
}
