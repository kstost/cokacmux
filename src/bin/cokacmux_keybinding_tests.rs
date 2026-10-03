//! Regression coverage for user-owned bindings, routing, and live reload.

use super::tests::app_for_key_tests;
use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn loading_fills_defaults_in_memory_without_rewriting_user_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    let original = serde_json::json!({
        "sessions": { "filter": ["/"], "toggle_focus": ["tab", "esc"], "move_next": ["down", "j"] },
        "agent": { "focus_auxiliary": [], "scroll_page_up": ["shift+pageup", "alt+pageup"] },
        "agent.focus_prev": null,
        "cokacdir.passthrough_shift": false,
        "custom_notes": "keep me"
    });
    fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let (bindings, _) = KeyBindings::read_from_path(Some(&path)).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    assert!(bindings.matches(KeyAction::NewSessionComplete, key(KeyCode::Tab)));
    assert!(!bindings.cokacdir_passthrough_shift);
    assert!(bindings.bindings[&KeyAction::AgentFocusAuxiliary].is_empty());
    assert!(bindings.bindings[&KeyAction::AgentFocusPrev].is_empty());
    assert!(bindings.matches(KeyAction::SessionFilter, key(KeyCode::Char('/'))));
    KeyBindings::read_from_path(Some(&path)).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "second load must not rewrite the file"
    );
}

#[test]
fn partial_config_reload_does_not_overwrite_edits_or_trigger_another_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    for content in [
        "{\n  \"agent.focus_prev\": [\"f8\"]\n}\n",
        "{\n  \"agent.focus_prev\": [], \"notes\": \"new edit\"\n}\n",
    ] {
        fs::write(&path, content).unwrap();
        let mut observed = None;
        let reload = check_keybindings_reload(Some(&path), &mut observed).unwrap();
        let bindings = reload.keybindings.unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
        assert_eq!(
            bindings.matches(KeyAction::AgentFocusPrev, key(KeyCode::F(8))),
            content.contains("f8")
        );
        assert!(check_keybindings_reload(Some(&path), &mut observed).is_none());
    }
}

#[test]
fn control_aliases_preserve_alt_and_reject_extra_modifiers() {
    for letter in 'a'..='z' {
        let plain = KeyBinding::parse(&format!("ctrl+{letter}")).unwrap();
        let alt = KeyBinding::parse(&format!("ctrl+alt+{letter}")).unwrap();
        let control = char::from(letter as u8 - b'a' + 1);
        for event in [
            KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char(control), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char(control), KeyModifiers::CONTROL),
        ] {
            assert!(plain.matches(event), "{letter}: {event:?}");
            assert!(!alt.matches(event), "{letter}: {event:?}");
            let alt_event = KeyEvent {
                modifiers: event.modifiers | KeyModifiers::ALT,
                ..event
            };
            assert!(alt.matches(alt_event), "{letter}: {alt_event:?}");
            assert!(!plain.matches(alt_event), "{letter}: {alt_event:?}");
            for extra in [
                KeyModifiers::SHIFT,
                KeyModifiers::SUPER,
                KeyModifiers::META,
                KeyModifiers::HYPER,
            ] {
                assert!(!plain.matches(KeyEvent {
                    modifiers: event.modifiers | extra,
                    ..event
                }));
                assert!(!alt.matches(KeyEvent {
                    modifiers: alt_event.modifiers | extra,
                    ..alt_event
                }));
            }
        }
    }
    // Ambiguous terminal aliases must still not steal Escape / Ctrl+3.
    assert!(!KeyBinding::parse("ctrl+[")
        .unwrap()
        .matches(key(KeyCode::Esc)));
    assert!(!KeyBinding::parse("ctrl+]")
        .unwrap()
        .matches(key(KeyCode::Char('\u{1d}'))));
}

#[test]
fn shift_tab_spellings_match_real_terminal_events() {
    for spelling in ["backtab", "shift+tab", "shift+backtab"] {
        let binding = KeyBinding::parse(spelling).unwrap();
        for event in [
            key(KeyCode::BackTab),
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
        ] {
            assert!(binding.matches(event), "{spelling}: {event:?}");
        }
        assert!(!binding.matches(key(KeyCode::Tab)));
        assert!(!binding.matches(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::SHIFT | KeyModifiers::ALT
        )));
        assert_eq!(binding.label(), "Shift+Tab");
    }
    let cokacdir = cokacdir_session_info_for_cwd("/repo".into());
    assert!(agent_shift_shortcuts_disabled_for_active_info(
        Some(&cokacdir),
        key(KeyCode::BackTab)
    ));
}

#[test]
fn sessions_escape_and_notice_dismiss_can_be_moved_or_disabled() {
    let mut app = app_for_key_tests();
    app.keybindings.apply_json(&serde_json::json!({
        "sessions.escape": ["f8"], "notice.dismiss": ["f9"]
    }));
    app.input_mode = InputMode::Notice {
        title: "Notice".into(),
        message: "message".into(),
    };
    for code in [KeyCode::Enter, KeyCode::Esc] {
        handle_key(&mut app, key(code), 100, 80, 20);
        assert!(matches!(app.input_mode, InputMode::Notice { .. }));
    }
    handle_key(&mut app, key(KeyCode::F(9)), 100, 80, 20);
    assert!(matches!(app.input_mode, InputMode::Normal));
    handle_key(&mut app, key(KeyCode::Esc), 100, 80, 20);
    assert!(!app.should_quit);
    app.keybindings
        .apply_json(&serde_json::json!({ "sessions.escape": [] }));
    handle_key(&mut app, key(KeyCode::F(8)), 100, 80, 20);
    assert!(!app.should_quit);
    assert_eq!(
        app.keybindings.help(KeyAction::SessionEscape, "Esc"),
        "unbound"
    );
    app.keybindings
        .apply_json(&serde_json::json!({ "sessions.escape": ["f8"] }));
    handle_key(&mut app, key(KeyCode::F(8)), 100, 80, 20);
    assert!(app.should_quit);
}

#[test]
fn background_cancellation_uses_configured_keys() {
    let mut app = app_for_key_tests();
    let cancel = Arc::new(AtomicBool::new(false));
    app.keybindings
        .apply_json(&serde_json::json!({ "data_task.cancel": ["f8"], "ai_search.cancel": ["f9"] }));
    app.data_task = Some(
        DataTaskPending::new(1, DataTaskKind::Clone, "clone".into())
            .with_cancel_token(cancel.clone()),
    );
    assert!(app.handle_data_task_key(key(KeyCode::Esc)));
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.status.contains("F8"));
    assert!(app.handle_data_task_key(key(KeyCode::F(8))));
    assert!(cancel.load(Ordering::Relaxed));
    app.data_task = None;
    let cancel = Arc::new(AtomicBool::new(false));
    app.ai_search_pending = Some(AiSearchPending {
        seq: 1,
        query: "test".into(),
        provider: Provider::Codex,
        phase: AiSearchPhase::Indexing,
        indexed: 0,
        total: 1,
        reused: 0,
        written: 0,
        cancel_requested: false,
        cancel: cancel.clone(),
        started_at: Instant::now(),
    });
    assert!(handle_ai_search_locked_key(&mut app, key(KeyCode::Esc)));
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.status.contains("F9"));
    assert!(handle_ai_search_locked_key(&mut app, key(KeyCode::F(9))));
    assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn settings_section_and_text_edit_keys_are_configurable() {
    let mut app = app_for_key_tests();
    app.keybindings
        .apply_json(&serde_json::json!({ "ai_title_settings": {
            "section_next": ["f8"], "move_left": ["f9"], "backspace": ["f10"]
        }}));
    let mut state = SettingsState::new(&app.settings.cokacmux);
    state.section = SettingsSection::General;
    let section = state.section;
    app.input_mode = InputMode::Settings {
        state,
        return_to: AiTitleSettingsReturn::Normal,
    };
    handle_key(&mut app, key(KeyCode::Right), 100, 80, 20);
    let InputMode::Settings { state, .. } = &app.input_mode else {
        panic!("settings")
    };
    assert_eq!(state.section, section);
    handle_key(&mut app, key(KeyCode::F(8)), 100, 80, 20);
    let InputMode::Settings { state, .. } = &mut app.input_mode else {
        panic!("settings")
    };
    assert_ne!(state.section, section);
    state.section = SettingsSection::Agents;
    state.selected = SETTINGS_AGENTS_CODEX;
    state.draft.agent_programs.codex = Some("abc".into());
    state.begin_editing_selected_text();
    handle_key(&mut app, key(KeyCode::Left), 100, 80, 20);
    handle_key(&mut app, key(KeyCode::Backspace), 100, 80, 20);
    let InputMode::Settings { state, .. } = &app.input_mode else {
        panic!("settings")
    };
    assert_eq!(state.draft.agent_programs.codex.as_deref(), Some("abc"));
    handle_key(&mut app, key(KeyCode::F(9)), 100, 80, 20);
    handle_key(&mut app, key(KeyCode::F(10)), 100, 80, 20);
    let InputMode::Settings { state, .. } = &app.input_mode else {
        panic!("settings")
    };
    assert_eq!(state.draft.agent_programs.codex.as_deref(), Some("ac"));
}

#[test]
fn settings_activation_and_focused_sidebar_navigation_can_be_rebound() {
    let mut app = app_for_key_tests();
    app.keybindings.apply_json(&serde_json::json!({
        "ai_title_settings.activate": ["f8"],
        "agent.focused_sidebar_prev": [],
        "agent.focused_sidebar_next": ["f9"]
    }));
    let mut state = SettingsState::new(&app.settings.cokacmux);
    state.section = SettingsSection::Ai;
    state.selected = SETTINGS_AI_CODEX;
    state.draft.ai_provider = None;
    app.input_mode = InputMode::Settings {
        state,
        return_to: AiTitleSettingsReturn::Normal,
    };
    handle_key(&mut app, key(KeyCode::Char(' ')), 100, 80, 20);
    let InputMode::Settings { state, .. } = &app.input_mode else {
        panic!("settings")
    };
    assert_eq!(state.draft.ai_provider, None);
    handle_key(&mut app, key(KeyCode::F(8)), 100, 80, 20);
    let InputMode::Settings { state, .. } = &app.input_mode else {
        panic!("settings")
    };
    assert_eq!(state.draft.ai_provider, Some(Provider::Codex));
    assert_eq!(
        focused_agent_sidebar_select_key(&app.keybindings, key(KeyCode::Up)),
        None
    );
    assert_eq!(
        focused_agent_sidebar_select_key(&app.keybindings, key(KeyCode::Down)),
        None
    );
    assert_eq!(
        focused_agent_sidebar_select_key(&app.keybindings, key(KeyCode::F(9))),
        Some(1)
    );
    let help = help_text_from_items(&settings_help_items(state, &app.keybindings));
    assert!(help.contains("F8 select"));
    assert!(!help.contains("Space"));
}

#[test]
fn path_completion_key_can_be_moved_without_consuming_tab() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("alpha")).unwrap();
    fs::create_dir(dir.path().join("beta")).unwrap();
    let mut cwd = format!("{}{}", dir.path().display(), std::path::MAIN_SEPARATOR);
    let mut cursor = cwd.len();
    let mut completion = NewSessionPathCompletion::default();
    let mut bindings = KeyBindings::default();
    bindings.apply_json(&serde_json::json!({ "new_session.complete": ["f8"] }));
    assert!(!handle_new_session_cwd_completion_key(
        &mut cwd,
        &mut cursor,
        &mut completion,
        &bindings,
        key(KeyCode::Tab)
    ));
    assert!(handle_new_session_cwd_completion_key(
        &mut cwd,
        &mut cursor,
        &mut completion,
        &bindings,
        key(KeyCode::F(8))
    ));
    assert!(new_session_completion_is_visible(&completion));
}

#[test]
fn printable_completion_key_with_no_candidates_does_not_change_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = format!("{}{}", dir.path().display(), std::path::MAIN_SEPARATOR);
    for (binding, ch) in [("x", 'x'), ("space", ' ')] {
        let mut app = app_for_key_tests();
        app.keybindings
            .apply_json(&serde_json::json!({ "new_session.complete": [binding] }));
        app.input_mode = InputMode::NewSession {
            selected: NEW_SESSION_FIELD_CWD,
            kind: NewSessionKind::CodingAgent,
            cwd: cwd.clone(),
            cwd_cursor: cwd.len(),
            cwd_completion: NewSessionPathCompletion::default(),
            provider: Provider::Codex,
            provider_options: vec![Provider::Codex],
            launch_mode: AgentLaunchMode::Normal,
        };
        handle_key(&mut app, key(KeyCode::Char(ch)), 100, 80, 20);
        let InputMode::NewSession {
            selected,
            cwd: actual,
            cwd_cursor,
            ..
        } = &app.input_mode
        else {
            panic!("new session modal must stay open");
        };
        assert_eq!(actual, &cwd);
        assert_eq!(*cwd_cursor, cwd.len());
        assert_eq!(*selected, NEW_SESSION_FIELD_CWD);
        // Moving completion away from Tab must preserve Tab navigation.
        handle_key(&mut app, key(KeyCode::Tab), 100, 80, 20);
        assert!(matches!(
            app.input_mode,
            InputMode::NewSession {
                selected: NEW_SESSION_FIELD_PROVIDER,
                ..
            }
        ));
    }
}

#[cfg(unix)]
#[test]
fn rebound_scroll_actions_send_native_claude_and_opencode_input() {
    use super::tests::buffered_output_test_client_with_requests;
    for (provider, expected) in [
        (
            Provider::Claude,
            vec![
                b"\x1b[<64;41;5M".to_vec(),
                b"\x1b[<65;41;5M".to_vec(),
                b"\x1b[5~".to_vec(),
                b"\x1b[6~".to_vec(),
                b"\x1b[1;5H".to_vec(),
                b"\x1b[1;5F".to_vec(),
            ],
        ),
        (
            Provider::OpenCode,
            vec![
                b"\x1b\x19".to_vec(),
                b"\x1b\x05".to_vec(),
                b"\x1b[5~".to_vec(),
                b"\x1b[6~".to_vec(),
                vec![7],
                b"\x1b\x07".to_vec(),
            ],
        ),
    ] {
        let mut app = app_for_key_tests();
        let (mut client, requests) =
            buffered_output_test_client_with_requests("rebound-scroll", 922);
        client.info.provider = provider;
        client.parser.process(b"\x1b[?1000h\x1b[?1006h");
        app.set_active_agent(client);
        app.agent_focus = AgentFocusPane::Main;
        app.show_sessions_view = false;
        // Match the fixture's 80x8 PTY plus title/status rows so this test
        // exercises scrolling, without starting an unacknowledged resize.
        app.settings.cokacmux.agent_sidebar_visible = false;
        let mut bindings = KeyBindings::default();
        bindings.apply_json(&serde_json::json!({ "agent": {
            "scroll_line_up": ["f5"], "scroll_line_down": ["f6"],
            "scroll_page_up": ["f7"], "scroll_page_down": ["f8"],
            "scroll_top": ["f9"], "scroll_bottom": ["f10"]
        }}));
        app.on_keybindings_reloaded(KeybindingsReload {
            keybindings: Some(bindings),
            status: "reloaded".into(),
        });
        for function_key in 5..=10 {
            handle_agent_key(&mut app, key(KeyCode::F(function_key)), 80, 10);
            assert!(
                app.status.contains("delegated"),
                "{}: {}",
                provider.as_str(),
                app.status
            );
        }
        let inputs: Vec<Vec<u8>> = requests
            .try_iter()
            .filter_map(|request| match request.request {
                AgentDaemonRequest::Input { data, .. } => Some(data),
                _ => None,
            })
            .collect();
        assert_eq!(inputs, expected, "{}", provider.as_str());
        app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
    }
}

#[cfg(unix)]
#[test]
fn claude_line_scroll_requires_negotiated_mouse_mode_and_stable_geometry() {
    use super::tests::buffered_output_test_client_with_requests;
    let (mut client, requests) =
        buffered_output_test_client_with_requests("claude-line-scroll", 923);
    let scroll_key = key(KeyCode::F(8));
    let status =
        delegate_agent_scroll_to_child(&mut client, AgentScrollAction::Lines(1), scroll_key, 8);
    assert!(status.contains("fullscreen mouse reporting"), "{status}");
    assert!(
        requests.try_recv().is_err(),
        "never forward F8 into the prompt"
    );
    client.parser.process(b"\x1b[?1000h");
    client.pending_snapshot_output = true;
    let status =
        delegate_agent_scroll_to_child(&mut client, AgentScrollAction::Lines(1), scroll_key, 8);
    assert!(status.contains("screen geometry"), "{status}");
    assert!(requests.try_recv().is_err());
    client.pending_snapshot_output = false;
    let status =
        delegate_agent_scroll_to_child(&mut client, AgentScrollAction::Lines(1), scroll_key, 8);
    assert!(status.contains("delegated"), "{status}");
    let request = requests.try_recv().unwrap();
    assert!(
        matches!(request.request, AgentDaemonRequest::Input { data, .. }
        if data == vec![0x1b, b'[', b'M', 96, 73, 37])
    );
    assert!(requests.try_recv().is_err());
    client.exited = Some("test cleanup".into());
}

#[test]
fn custom_codex_scroll_keys_are_translated_after_live_reload() {
    let mut bindings = KeyBindings::default();
    bindings.apply_json(&serde_json::json!({ "agent.scroll_page_up": ["f8"] }));
    let original = key(KeyCode::F(8));
    let action = agent_scrollback_key(&bindings, original).unwrap();
    let delegated = KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT | KeyModifiers::ALT);
    assert_eq!(
        codex_child_scroll_delegated_keys(action, original, false),
        Some(vec![delegated, delegated])
    );
    assert_eq!(
        codex_child_scroll_delegated_keys(action, original, true),
        Some(vec![delegated])
    );
}

#[cfg(unix)]
#[test]
fn reloaded_bindings_release_old_keys_to_the_focused_child() {
    use super::tests::buffered_output_test_client_with_requests;
    let mut app = app_for_key_tests();
    let (client, requests) = buffered_output_test_client_with_requests("keybinding-reload", 920);
    app.set_active_agent(client);
    app.show_sessions_view = false;
    app.agent_focus = AgentFocusPane::Main;
    let mut bindings = KeyBindings::default();
    bindings.apply_json(&serde_json::json!({ "agent": {
        "focus_prev": ["f8"], "focus_next": [],
        "toggle_cokacdir_panel": ["f9"], "toggle_terminal_panel": []
    }}));
    app.on_keybindings_reloaded(KeybindingsReload {
        keybindings: Some(bindings),
        status: "reloaded".into(),
    });
    for event in [
        KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    ] {
        handle_agent_key(&mut app, event, 120, 30);
        assert_eq!(app.agent_focus, AgentFocusPane::Main);
    }
    let inputs: Vec<Vec<u8>> = requests
        .try_iter()
        .filter_map(|request| match request.request {
            AgentDaemonRequest::Input { data, .. } => Some(data),
            _ => None,
        })
        .collect();
    assert_eq!(
        inputs,
        vec![
            b"\x1b[1;2D".to_vec(),
            b"\x1b[1;2C".to_vec(),
            vec![6],
            vec![20]
        ]
    );
    app.settings.cokacmux.agent_sidebar_visible = true;
    handle_agent_key(&mut app, key(KeyCode::F(8)), 120, 30);
    assert_eq!(app.agent_focus, AgentFocusPane::Sidebar);
    app.keybindings
        .apply_json(&serde_json::json!({ "agent.sidebar_sessions": ["f11"] }));
    handle_agent_key(&mut app, key(KeyCode::Esc), 120, 30);
    assert!(!app.show_sessions_view);
    handle_agent_key(&mut app, key(KeyCode::F(11)), 120, 30);
    assert!(app.show_sessions_view);
    app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
}

#[cfg(unix)]
#[test]
fn cokacdir_shift_passthrough_can_be_kept_or_overridden() {
    use super::tests::buffered_output_test_client_with_requests;
    let mut app = app_for_key_tests();
    let (mut client, requests) = buffered_output_test_client_with_requests("cokacdir-binding", 921);
    client.info = cokacdir_session_info_for_cwd("/repo".into());
    app.set_active_agent(client);
    app.agent_focus = AgentFocusPane::Main;
    app.settings.cokacmux.agent_sidebar_visible = true;
    let event = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
    handle_agent_key(&mut app, event, 120, 30);
    assert_eq!(app.agent_focus, AgentFocusPane::Main);
    assert!(requests.try_iter().any(|request| matches!(request.request, AgentDaemonRequest::Input { data, .. } if data.as_slice() == b"\x1b[1;2D")));
    app.keybindings
        .apply_json(&serde_json::json!({ "cokacdir": { "passthrough_shift": false } }));
    handle_agent_key(&mut app, event, 120, 30);
    assert_eq!(app.agent_focus, AgentFocusPane::Sidebar);
    app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
}
