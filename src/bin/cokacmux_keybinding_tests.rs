//! Regression coverage for user-owned bindings, routing, and live reload.

use super::tests::app_for_key_tests;
use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn enabled_keys_json(keys: &[&str]) -> serde_json::Value {
    serde_json::Value::Array(
        keys.iter()
            .map(|key| serde_json::json!({ "key": key, "enabled": true }))
            .collect(),
    )
}

fn assert_complete_config(value: &serde_json::Value) {
    for (path, _, _) in DEFAULT_KEYBINDINGS {
        assert!(
            keybinding_json_value(value, path).is_some(),
            "missing {path}"
        );
    }
    for path in ["cokacdir.passthrough_shift", "cokacdir.passthrough_kill"] {
        assert!(
            keybinding_json_value(value, path).is_some(),
            "missing {path}"
        );
    }
}

#[test]
fn every_default_key_has_a_boolean_and_can_be_disabled() {
    let mut config = KeyBindings::default_config_json();
    let defaults = KeyBindings::default();
    let mut loaded = KeyBindings::default();
    loaded.apply_json(&config);
    for (path, action, keys) in DEFAULT_KEYBINDINGS {
        let entries = keybinding_json_value_mut(&mut config, path)
            .unwrap()
            .as_array_mut()
            .unwrap();
        assert_eq!(entries.len(), keys.len(), "{path}");
        assert_eq!(
            loaded.labels(*action, usize::MAX),
            defaults.labels(*action, usize::MAX),
            "{path}"
        );
        for (entry, key) in entries.iter_mut().zip(keys.iter()) {
            assert_eq!(entry["key"], *key, "{path}");
            assert_eq!(entry["enabled"], true, "{path}");
            entry["enabled"] = false.into();
        }
    }
    loaded.apply_json(&config);
    for (_, action, _) in DEFAULT_KEYBINDINGS {
        assert!(loaded.bindings[action].is_empty(), "{action:?}");
        assert_eq!(loaded.help(*action, "default"), "unbound");
    }
    assert!(!complete_keybinding_config(&mut config).unwrap());
}

#[test]
fn per_key_flags_preserve_other_keys_and_hide_disabled_help() {
    let mut bindings = KeyBindings::default();
    bindings.apply_json(&serde_json::json!({ "agent.focus_prev": [
        { "key": "shift+left", "enabled": false },
        { "key": "ctrl+left", "enabled": true },
        { "key": "ctrl+dot" }
    ] }));
    assert!(!bindings.matches(
        KeyAction::AgentFocusPrev,
        KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)
    ));
    assert!(bindings.matches(
        KeyAction::AgentFocusPrev,
        KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)
    ));
    assert!(bindings.matches(
        KeyAction::AgentFocusPrev,
        KeyEvent::new(KeyCode::Char('.'), KeyModifiers::CONTROL)
    ));
    assert_eq!(
        bindings.help(KeyAction::AgentFocusPrev, "default"),
        "Ctrl+←/Ctrl+."
    );
}

#[test]
fn legacy_and_boolean_entries_roundtrip_without_losing_user_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    let original = serde_json::json!({
        "sessions.quit": "f8",
        "agent": {
            "focus_prev": [
                "ctrl+left",
                { "key": "ctrl+dot", "note": "preserve me" },
                { "key": "shift+left", "enabled": false }
            ],
            "focus_next": { "key": "f9", "enabled": false, "note": "later" },
            "focus_main": null,
            "focus_auxiliary": []
        }
    });
    fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    let (bindings, observed) = KeyBindings::read_from_path(Some(&path)).unwrap();
    let bytes = fs::read(&path).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_complete_config(&saved);
    assert_eq!(saved["sessions.quit"], enabled_keys_json(&["f8"]));
    assert_eq!(
        saved["agent"]["focus_prev"],
        serde_json::json!([
            { "key": "ctrl+left", "enabled": true },
            { "key": "ctrl+dot", "enabled": true, "note": "preserve me" },
            { "key": "shift+left", "enabled": false }
        ])
    );
    assert_eq!(
        saved["agent"]["focus_next"],
        serde_json::json!([
            { "key": "f9", "enabled": false, "note": "later" }
        ])
    );
    assert!(saved["agent"]["focus_main"].is_null());
    assert_eq!(saved["agent"]["focus_auxiliary"], serde_json::json!([]));
    assert!(bindings.matches(KeyAction::SessionQuit, key(KeyCode::F(8))));
    assert!(bindings.bindings[&KeyAction::AgentFocusNext].is_empty());
    KeyBindings::read_from_path(Some(&path)).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(KeyBindings::file_mtime(Some(&path)).unwrap(), observed);
}

#[test]
fn malformed_enabled_flags_do_not_replace_active_bindings_or_rewrite_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    let mut app = app_for_key_tests();
    app.keybindings
        .apply_json(&serde_json::json!({ "agent.focus_prev": ["f8"] }));
    for invalid in [
        serde_json::json!("false"),
        serde_json::json!(0),
        serde_json::Value::Null,
        serde_json::json!([]),
    ] {
        let content = serde_json::to_vec(&serde_json::json!({ "agent.focus_prev": [
            { "key": "f9", "enabled": invalid }
        ] }))
        .unwrap();
        fs::write(&path, &content).unwrap();
        let mut observed = None;
        let reload = check_keybindings_reload(Some(&path), &mut observed).unwrap();
        assert!(reload.keybindings.is_none());
        assert!(
            reload.status.contains("enabled must be a JSON boolean"),
            "{}",
            reload.status
        );
        app.on_keybindings_reloaded(reload);
        assert!(app
            .keybindings
            .matches(KeyAction::AgentFocusPrev, key(KeyCode::F(8))));
        assert!(!app
            .keybindings
            .matches(KeyAction::AgentFocusPrev, key(KeyCode::F(9))));
        assert_eq!(fs::read(&path).unwrap(), content);
    }
    assert!(parse_keybinding_json_list(&serde_json::json!([
        { "key": "not-a-valid-key", "enabled": false }
    ]))
    .unwrap()
    .is_empty());
    assert!(parse_keybinding_json_list(&serde_json::json!([
        { "key": "not-a-valid-key", "enabled": true }
    ]))
    .is_err());
    for entry in [
        serde_json::json!({ "enabled": false }),
        serde_json::json!({ "key": 12, "enabled": false }),
    ] {
        assert!(parse_keybinding_json_list(&entry).is_err());
    }
}

#[cfg(unix)]
#[test]
fn boolean_reload_releases_shift_left_to_child_and_can_restore_it() {
    use super::tests::buffered_output_test_client_with_requests;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    let mut app = app_for_key_tests();
    let (client, requests) = buffered_output_test_client_with_requests("keybinding-enabled", 922);
    app.set_active_agent(client);
    app.show_sessions_view = false;
    app.settings.cokacmux.agent_sidebar_visible = true;
    for enabled in [false, true, false] {
        let config = serde_json::json!({ "agent.focus_prev": [
            { "key": "ctrl+left", "enabled": true },
            { "key": "shift+left", "enabled": enabled }
        ] });
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut observed = None;
        app.on_keybindings_reloaded(check_keybindings_reload(Some(&path), &mut observed).unwrap());
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["agent.focus_prev"], config["agent.focus_prev"]);
        assert!(check_keybindings_reload(Some(&path), &mut observed).is_none());
        app.agent_focus = AgentFocusPane::Main;
        handle_agent_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT),
            120,
            30,
        );
        assert_eq!(
            app.agent_focus,
            if enabled {
                AgentFocusPane::Sidebar
            } else {
                AgentFocusPane::Main
            }
        );
        let inputs: Vec<Vec<u8>> = requests
            .try_iter()
            .filter_map(|request| match request.request {
                AgentDaemonRequest::Input { data, .. } => Some(data),
                _ => None,
            })
            .collect();
        assert_eq!(
            inputs,
            if enabled {
                vec![]
            } else {
                vec![b"\x1b[1;2D".to_vec()]
            }
        );
        app.agent_focus = AgentFocusPane::Main;
        handle_agent_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            120,
            30,
        );
        assert_eq!(app.agent_focus, AgentFocusPane::Sidebar);
        assert!(!requests
            .try_iter()
            .any(|request| matches!(request.request, AgentDaemonRequest::Input { .. })));
    }
    app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
}

#[test]
fn loading_completes_file_without_changing_user_values() {
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
    let (bindings, observed) = KeyBindings::read_from_path(Some(&path)).unwrap();
    let bytes = fs::read(&path).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_complete_config(&saved);
    for (action, expected) in [
        ("sessions.filter", enabled_keys_json(&["/"])),
        ("sessions.toggle_focus", enabled_keys_json(&["tab", "esc"])),
        ("sessions.move_next", enabled_keys_json(&["down", "j"])),
        ("agent.focus_auxiliary", serde_json::json!([])),
        (
            "agent.scroll_page_up",
            enabled_keys_json(&["shift+pageup", "alt+pageup"]),
        ),
        ("agent.focus_prev", serde_json::Value::Null),
        ("cokacdir.passthrough_shift", serde_json::json!(false)),
    ] {
        assert_eq!(keybinding_json_value(&saved, action), Some(&expected));
    }
    assert_eq!(saved["custom_notes"], "keep me");
    assert!(
        saved["agent"].get("focus_prev").is_none(),
        "flat override must not be duplicated"
    );
    assert!(bindings.matches(KeyAction::NewSessionComplete, key(KeyCode::Tab)));
    assert!(!bindings.cokacdir_passthrough_shift);
    assert!(bindings.bindings[&KeyAction::AgentFocusAuxiliary].is_empty());
    assert!(bindings.bindings[&KeyAction::AgentFocusPrev].is_empty());
    assert!(bindings.matches(KeyAction::SessionFilter, key(KeyCode::Char('/'))));
    assert_eq!(observed, KeyBindings::file_mtime(Some(&path)).unwrap());
    KeyBindings::read_from_path(Some(&path)).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        bytes,
        "complete file must not be rewritten"
    );
    assert_eq!(observed, KeyBindings::file_mtime(Some(&path)).unwrap());
}

#[test]
fn partial_config_reload_completes_file_without_another_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    for content in [
        r#"{"agent.focus_prev": ["f8"]}"#,
        r#"{"agent.focus_prev": [], "notes": "new edit"}"#,
    ] {
        fs::write(&path, content).unwrap();
        let mut observed = None;
        let reload = check_keybindings_reload(Some(&path), &mut observed).unwrap();
        let bindings = reload.keybindings.unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_complete_config(&saved);
        let original: serde_json::Value = serde_json::from_str(content).unwrap();
        for (name, value) in original.as_object().unwrap() {
            if name == "agent.focus_prev" && content.contains("f8") {
                assert_eq!(saved[name], enabled_keys_json(&["f8"]));
            } else {
                assert_eq!(&saved[name], value);
            }
        }
        assert_eq!(
            bindings.matches(KeyAction::AgentFocusPrev, key(KeyCode::F(8))),
            content.contains("f8")
        );
        assert!(check_keybindings_reload(Some(&path), &mut observed).is_none());
    }
}

#[test]
fn complete_config_keeps_its_existing_format_and_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    let original = serde_json::to_string(&KeyBindings::default_config_json()).unwrap() + "\n\n";
    fs::write(&path, &original).unwrap();
    let before = KeyBindings::file_mtime(Some(&path)).unwrap();
    KeyBindings::read_from_path(Some(&path)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert_eq!(KeyBindings::file_mtime(Some(&path)).unwrap(), before);
}

#[test]
fn completion_rejects_stale_snapshot_and_reloads_latest_user_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    let original = r#"{"agent.focus_prev":["f8"]}"#;
    let newest = r#"{"agent.focus_prev":["f9"],"notes":"newest"}"#;
    let mut completed: serde_json::Value = serde_json::from_str(original).unwrap();
    complete_keybinding_config(&mut completed).unwrap();
    fs::write(&path, newest).unwrap();
    assert_eq!(
        persist_completed_keybindings(&path, original, &completed).unwrap(),
        None
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), newest);
    let (bindings, _) = KeyBindings::read_from_path(Some(&path)).unwrap();
    assert!(bindings.matches(KeyAction::AgentFocusPrev, key(KeyCode::F(9))));
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_complete_config(&saved);
    assert_eq!(saved["notes"], "newest");
}

#[test]
fn completion_retries_after_another_instance_releases_its_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    let original = r#"{"agent.focus_prev":["f8"]}"#;
    fs::write(&path, original).unwrap();
    let guard = runtime_path_mutation_guard(&fs::canonicalize(&path).unwrap()).unwrap();
    let mut observed = None;
    let reload = check_keybindings_reload(Some(&path), &mut observed).unwrap();
    assert!(reload
        .keybindings
        .unwrap()
        .matches(KeyAction::AgentFocusPrev, key(KeyCode::F(8))));
    assert_eq!(
        observed, None,
        "busy completion must be retried even without an edit"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    drop(guard);
    assert!(check_keybindings_reload(Some(&path), &mut observed).is_some());
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_complete_config(&saved);
    assert!(check_keybindings_reload(Some(&path), &mut observed).is_none());
}

#[test]
fn completion_preserves_unknown_group_values_and_rejects_invalid_roots() {
    let mut value = serde_json::json!({"agent": "keep this", "agent.focus_prev": []});
    assert!(complete_keybinding_config(&mut value).unwrap());
    assert_complete_config(&value);
    assert_eq!(value["agent"], "keep this");
    assert_eq!(value["agent.focus_prev"], serde_json::json!([]));
    assert!(!complete_keybinding_config(&mut value).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybinding.json");
    for content in ["[]", "null", "{broken"] {
        fs::write(&path, content).unwrap();
        assert!(KeyBindings::read_from_path(Some(&path)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
    }
}

#[cfg(unix)]
#[test]
fn completion_preserves_symlinks_permissions_and_readonly_bindings() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("actual.json");
    let path = dir.path().join("keybinding.json");
    fs::write(&target, r#"{"agent.focus_prev":["f8"]}"#).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&target, &path).unwrap();
    KeyBindings::read_from_path(Some(&path)).unwrap();
    assert!(fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_complete_config(&serde_json::from_slice(&fs::read(&target).unwrap()).unwrap());
    let original = r#"{"agent.focus_prev":["f9"]}"#;
    fs::write(&target, original).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o400)).unwrap();
    let (bindings, _) = KeyBindings::read_from_path(Some(&path)).unwrap();
    assert!(bindings.matches(KeyAction::AgentFocusPrev, key(KeyCode::F(9))));
    assert_eq!(fs::read_to_string(&target).unwrap(), original);
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
