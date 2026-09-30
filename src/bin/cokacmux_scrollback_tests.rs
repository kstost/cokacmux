//! Scrollback settings and retention regressions for the explicit Rust test gate.
//! Settings saves are disabled in UI fixtures; clients have no running agents.

use super::*;

fn scrollback_settings_app() -> App {
    let mut app = super::tests::app_for_key_tests();
    let mut state = SettingsState::new(&app.settings.cokacmux);
    state.section = SettingsSection::General;
    state.selected = SETTINGS_GENERAL_SCROLLBACK_LINES;
    app.input_mode = InputMode::Settings {
        state,
        return_to: AiTitleSettingsReturn::Normal,
    };
    app
}

fn press(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 100, 80, 24);
}

fn edit_scrollback_value(app: &mut App, value: &str) {
    let InputMode::Settings { state, .. } = &app.input_mode else {
        panic!("expected settings");
    };
    let previous_len = state.draft.scrollback_lines.chars().count();
    press(app, KeyCode::Enter);
    press(app, KeyCode::Home);
    for _ in 0..previous_len {
        press(app, KeyCode::Delete);
    }
    for ch in value.chars() {
        press(app, KeyCode::Char(ch));
    }
    press(app, KeyCode::Enter);
}

#[test]
fn scrollback_settings_defaults_and_roundtrips() {
    let legacy: Settings = serde_json::from_str(r#"{"cokacmux":{}}"#).unwrap();
    assert_eq!(legacy.cokacmux.scrollback_lines, None);
    assert_eq!(CokacmuxSettings::default().scrollback_lines, None);
    for lines in [
        None,
        Some(0),
        Some(7),
        Some(10_000),
        Some(100_001),
        Some(1_000_000),
        Some(usize::MAX),
    ] {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "cokacmux": { "scrollback_lines": lines }
        }))
        .unwrap();
        let wire = serde_json::to_vec(&settings.normalized()).unwrap();
        let restored: Settings = serde_json::from_slice(&wire).unwrap();
        assert_eq!(restored.cokacmux.scrollback_lines, lines);
    }
}

#[test]
fn invalid_scrollback_json_preserves_other_settings() {
    for value in [
        serde_json::json!(-1),
        serde_json::json!("50000"),
        serde_json::json!(1.5),
        serde_json::json!(true),
        serde_json::json!({}),
    ] {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "cokacmux": {
                "scrollback_lines": value,
                "agent_sidebar_visible": false,
                "future_setting": "keep"
            }
        }))
        .unwrap();
        assert_eq!(settings.cokacmux.scrollback_lines, None);
        assert!(!settings.cokacmux.agent_sidebar_visible);
        assert_eq!(settings.cokacmux.extra["future_setting"], "keep");
    }
}

#[test]
fn scrollback_ui_edits_and_saves_valid_values() {
    for (value, expected) in [
        ("0", Some(0)),
        ("25000", Some(25_000)),
        ("100001", Some(100_001)),
        ("1000000", Some(1_000_000)),
        ("", None),
        ("unlimited", None),
        (" UNLIMITED ", None),
    ] {
        let mut app = scrollback_settings_app();
        edit_scrollback_value(&mut app, value);
        assert_eq!(app.settings.cokacmux.scrollback_lines, None);
        assert!(
            matches!(&app.input_mode, InputMode::Settings { state, .. } if state.edit_finished)
        );
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(app.settings.cokacmux.scrollback_lines, expected);
        assert_eq!(app.settings.cokacmux.ai.provider, None);
        let saved = serde_json::to_value(&app.settings).unwrap();
        assert_eq!(
            saved["cokacmux"]["scrollback_lines"],
            serde_json::json!(expected)
        );
    }
}

#[test]
fn clearing_a_saved_numeric_limit_restores_unlimited() {
    let mut app = scrollback_settings_app();
    app.settings.cokacmux.scrollback_lines = Some(1_000_000);
    if let InputMode::Settings { state, .. } = &mut app.input_mode {
        state.draft.scrollback_lines = "1000000".into();
        state.original = state.draft.clone();
    }
    edit_scrollback_value(&mut app, "");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.input_mode, InputMode::Normal));
    assert_eq!(app.settings.cokacmux.scrollback_lines, None);
    let draft = SettingsDraft::from_settings(&app.settings.cokacmux);
    assert_eq!(
        settings_text_display_value(&draft, SettingsTextField::ScrollbackLines),
        "unlimited"
    );
}

#[test]
fn invalid_scrollback_ui_save_keeps_draft_open_and_settings_unchanged() {
    for value in ["-1", "1.5", "no", "999999999999999999999999999999"] {
        let mut app = scrollback_settings_app();
        edit_scrollback_value(&mut app, value);
        // Saving from another section must still validate the number first.
        if let InputMode::Settings { state, .. } = &mut app.input_mode {
            state.draft.agent_sidebar_visible = false;
            state.section = SettingsSection::Ai;
            state.selected = SETTINGS_AI_NONE;
        }
        press(&mut app, KeyCode::Enter);
        let InputMode::Settings { state, .. } = &app.input_mode else {
            panic!("invalid value closed settings");
        };
        assert_eq!(state.section, SettingsSection::General);
        assert_eq!(state.selected, SETTINGS_GENERAL_SCROLLBACK_LINES);
        assert_eq!(state.draft.scrollback_lines, value);
        assert_eq!(
            state.text_status(SettingsTextField::ScrollbackLines).level,
            SettingsStatusLevel::Error
        );
        assert_eq!(app.settings.cokacmux.scrollback_lines, None);
        assert!(app.settings.cokacmux.agent_sidebar_visible);
        assert!(app.status.contains("whole number"));
    }
}

#[test]
fn scrollback_digit_shortcuts_edit_and_escape_restores_the_value() {
    for ch in '1'..='5' {
        let mut app = scrollback_settings_app();
        press(&mut app, KeyCode::Char(ch));
        let InputMode::Settings { state, .. } = &app.input_mode else {
            panic!("expected settings");
        };
        assert_eq!(state.section, SettingsSection::General);
        assert_eq!(state.draft.ai_provider, None);
        assert_eq!(state.draft.scrollback_lines, ch.to_string());
        assert!(state.editing.is_some());
        let (lines, cursor) = settings_modal_lines(state, 78);
        let (row, col) = cursor.expect("numeric editor needs a cursor");
        assert!(lines[row]
            .spans
            .iter()
            .any(|span| span.content.contains("Scrollback lines")));
        assert!(col < 78);
        press(&mut app, KeyCode::Esc);
        let InputMode::Settings { state, .. } = &app.input_mode else {
            panic!("escape should cancel only the edit");
        };
        assert!(state.editing.is_none());
        assert!(!state.is_dirty());
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.input_mode, InputMode::Normal));
    }
}

#[test]
fn history_custom_limit_discards_oldest_lines_after_serialized_restore() {
    let mut history = ScreenHistory::new(3);
    for line in 1..=7 {
        history.capture_lines(vec![line.to_string()]);
    }
    assert_eq!(history.all_lines(), ["5", "6", "7"]);
    let wire = serde_json::to_vec(&history).unwrap();
    let mut restored: ScreenHistory = serde_json::from_slice(&wire).unwrap();
    restored.capture_lines(vec!["8".into()]);
    assert_eq!(restored.all_lines(), ["6", "7", "8"]);
    assert_eq!(restored.max_lines, 3);

    let old: ScreenHistory =
        serde_json::from_str(r#"{"lines":["old"],"last_snapshot":["old"]}"#).unwrap();
    assert_eq!(old.max_lines, 10_000);
    assert_eq!(old.all_lines(), ["old"]);
}

#[test]
fn zero_capacity_disables_both_history_buffers() {
    let mut parser = vt100::Parser::new(5, 20, 0);
    let mut history = ScreenHistory::new(0);
    for line in 0..20 {
        parser.process(format!("LINE{line}\r\n").as_bytes());
        history.capture(&mut parser);
        history.capture_lines(vec![format!("FRAME{line}")]);
    }
    assert_eq!(parser_max_scrollback(&mut parser), 0);
    assert!(history.lines.is_empty());
    assert!(history.last_snapshot.is_empty());
    assert!(parser.screen().contents().contains("LINE19"));
}

#[test]
fn unlimited_buffers_keep_history_past_former_limit_and_restore() {
    let mut parser = vt100::Parser::new(1, 1, AGENT_SCROLLBACK_LINES);
    let mut history = ScreenHistory::default();
    let count = 100_005;
    for line in 0..count {
        parser.process(b"x\r\n");
        history.capture_lines(vec![line.to_string()]);
    }
    assert_eq!(parser_max_scrollback(&mut parser), count);
    assert_eq!(history.len(), count);
    let prepared = AgentTerminalSnapshot {
        parser: parser.checkpoint(true),
        history,
    }
    .prepare()
    .unwrap();
    assert_eq!(prepared.history.len(), count);
    assert_eq!(
        prepared.history.lines.front().map(String::as_str),
        Some("0")
    );
    assert_eq!(prepared.history.max_lines, usize::MAX);
    assert_eq!(prepared.parser.screen().scrollback_capacity(), usize::MAX);
}

#[test]
fn checkpoints_accept_any_capacity_and_reject_history_exceeding_its_own_limit() {
    for capacity in [100_001, 1_000_000, usize::MAX] {
        let parser = vt100::Parser::new(5, 20, capacity);
        let prepared = AgentTerminalSnapshot {
            parser: parser.checkpoint(true),
            history: ScreenHistory::new(capacity),
        }
        .prepare()
        .unwrap();
        assert_eq!(prepared.parser.screen().scrollback_capacity(), capacity);
        assert_eq!(prepared.history.max_lines, capacity);
    }
    let parser = vt100::Parser::new(5, 20, 2);
    let mut too_many_lines = ScreenHistory::new(2);
    too_many_lines.lines = ["a".into(), "b".into(), "c".into()].into();
    assert!(AgentTerminalSnapshot {
        parser: parser.checkpoint(true),
        history: too_many_lines,
    }
    .prepare()
    .is_err());
}

#[test]
fn snapshot_compaction_does_not_truncate_a_larger_configured_history() {
    let scrollback = (0..12_000)
        .map(|line| format!("LINE{line}"))
        .collect::<Vec<_>>();
    let visible = (12_000..12_004)
        .map(|line| format!("LINE{line}"))
        .collect::<Vec<_>>();
    assert!(compact_frame_scrollback_lines(&scrollback, &visible, 4).is_none());
}

#[test]
fn snapshot_sanitizing_preserves_custom_parser_capacity() {
    for capacity in [7, 100_001, usize::MAX] {
        let mut parser = vt100::Parser::new(5, 20, capacity);
        parser.process(b"OLD\r\nA\r\nB\r\nC\r\nD\r\nE\r\nA\r\nB\r\nC\r\nD\r\nE");
        let visible = parser.screen().contents();
        assert!(sanitize_snapshot_visible_screen_duplicates(&mut parser).is_some());
        assert_eq!(parser.screen().scrollback_capacity(), capacity);
        assert_eq!(parser.screen().contents(), visible);
        assert_eq!(parser_scrollback_plain_lines(&mut parser), ["OLD"]);
    }
}

#[test]
fn parser_capacity_survives_alternate_screen_checkpoint_and_reset() {
    for capacity in [0, 7, 100_001, usize::MAX] {
        let mut parser = vt100::Parser::new(5, 20, capacity);
        parser.process(b"normal\x1b[?1049hALT");
        assert_eq!(parser.screen().scrollback_capacity(), capacity);
        let wire = serde_json::to_vec(&parser.checkpoint(true)).unwrap();
        let mut restored =
            vt100::Parser::from_checkpoint(serde_json::from_slice(&wire).unwrap()).unwrap();
        assert!(restored.screen().alternate_screen());
        assert_eq!(restored.screen().scrollback_capacity(), capacity);
        restored.process(b"\x1bc");
        assert_eq!(restored.screen().scrollback_capacity(), capacity);
    }
}

#[cfg(unix)]
#[test]
fn client_reconnect_and_legacy_snapshot_keep_daemon_capacity() {
    for capacity in [0, 3, 100_001, usize::MAX] {
        let (mut client, _requests) =
            super::tests::buffered_output_test_client_with_requests("scrollback-capacity", 9920);
        // Prevent drop from touching runtime storage, including on assertion failure.
        client.exited = Some("test cleanup".into());
        let parser = vt100::Parser::new(5, 40, capacity);
        let snapshot = AgentTerminalSnapshot {
            parser: parser.checkpoint(true),
            history: ScreenHistory::new(capacity),
        };
        let wire = serde_json::to_vec(&snapshot).unwrap();
        let snapshot: AgentTerminalSnapshot = serde_json::from_slice(&wire).unwrap();
        client.install_agent_snapshot(snapshot.prepare().unwrap());
        assert_eq!(client.parser.screen().scrollback_capacity(), capacity);
        assert_eq!(client.screen_history.max_lines, capacity);
        client.process_agent_snapshot(b"\x1b[2J\x1b[H\x1b[?1049hALT");
        assert_eq!(client.parser.screen().scrollback_capacity(), capacity);
        assert_eq!(client.screen_history.max_lines, capacity);
        assert!(client.parser.screen().contents().contains("ALT"));
    }
}

#[test]
fn only_terminals_keep_a_cokacmux_scrollback() {
    let limited = CokacmuxSettings {
        scrollback_lines: Some(5_000),
        ..Default::default()
    };
    let unlimited = CokacmuxSettings::default();
    let cwd = "/tmp/project".to_string();
    let terminals = [
        shell_session_info_for_cwd(cwd.clone()),
        cli_command_session_info("web".into(), cwd.clone(), vec!["npm".into(), "run".into()])
            .unwrap(),
    ];
    for info in &terminals {
        assert_eq!(agent_scrollback_lines_for(info, &limited), 5_000);
        assert_eq!(
            agent_scrollback_lines_for(info, &unlimited),
            AGENT_SCROLLBACK_LINES
        );
    }
    let stored_session = SessionInfo {
        provider: Provider::Codex,
        session_id: "stored".into(),
        cwd: cwd.clone(),
        source: PathBuf::from("/tmp/project/rollout.jsonl"),
        updated_at_epoch_s: 0,
        title: None,
        relation: None,
    };
    let others = [
        cokacdir_session_info_for_cwd(cwd.clone()),
        new_agent_session_info(Provider::Claude, cwd.clone()),
        stored_session,
    ];
    for info in &others {
        assert_eq!(agent_scrollback_lines_for(info, &limited), 0);
        assert_eq!(agent_scrollback_lines_for(info, &unlimited), 0);
    }
}

#[test]
fn alternate_screen_frames_stay_out_of_history_until_the_app_exits() {
    fn feed(parser: &mut vt100::Parser, hash: &mut u64, history: &mut ScreenHistory, bytes: &[u8]) {
        process_parser_output(parser, bytes, hash, Some(history));
    }
    let mut parser = vt100::Parser::new(5, 30, AGENT_SCROLLBACK_LINES);
    let mut history = ScreenHistory::new(AGENT_SCROLLBACK_LINES);
    let mut hash = screen_activity_hash(parser.screen());
    feed(&mut parser, &mut hash, &mut history, b"$ ls\r\nfile\r\n$ cokacdir");
    let before = history.all_lines();
    assert!(before.iter().any(|line| line == "file"));

    // A full-screen editor redraws its status line on every key.
    feed(&mut parser, &mut hash, &mut history, b"\x1b[?1049h");
    for col in 1..=50 {
        let frame = format!("\x1b[H\x1b[2Jf1.txt Ln 1, Col {col}\r\nbody\r\nmore");
        feed(&mut parser, &mut hash, &mut history, frame.as_bytes());
    }
    assert_eq!(history.all_lines(), before);

    feed(&mut parser, &mut hash, &mut history, b"\x1b[?1049l\r\n$ done");
    let after = history.all_lines();
    assert!(after.iter().any(|line| line == "$ done"));
    assert!(!after.iter().any(|line| line.contains("Col")));
}
