//! Wheel-only mouse support. Hit testing uses the last rendered content
//! rectangles, never a guessed terminal size or the keyboard focus.

use super::*;
use crossterm::event::{MouseEvent, MouseEventKind};

const WHEEL_LINES: i32 = 3;

#[derive(Debug, Clone)]
pub(super) enum MouseWheelTarget {
    Sessions,
    Preview(PreviewKey),
    AgentSidebar {
        workspace: AgentKey,
        offset: usize,
        total_rows: usize,
    },
    Agent {
        reader_id: u64,
    },
}

#[derive(Debug, Clone)]
pub(super) struct MouseWheelRegion {
    pub(super) area: Rect,
    pub(super) target: MouseWheelTarget,
    valid_since_epoch_ms: u64,
}

#[derive(Debug, Clone)]
pub(super) struct AgentSidebarScroll {
    pub(super) workspace: AgentKey,
    pub(super) offset: usize,
}

impl App {
    pub(super) fn begin_mouse_wheel_frame(&mut self) {
        self.previous_mouse_wheel_regions = std::mem::take(&mut self.mouse_wheel_regions);
    }

    pub(super) fn record_mouse_wheel_region(&mut self, area: Rect, target: MouseWheelTarget) {
        if area.width > 0 && area.height > 0 {
            let valid_since_epoch_ms = self
                .previous_mouse_wheel_regions
                .iter()
                .find(|region| region.area == area && same_target(&region.target, &target))
                .map_or_else(current_epoch_ms, |region| region.valid_since_epoch_ms);
            self.mouse_wheel_regions.push(MouseWheelRegion {
                area,
                target,
                valid_since_epoch_ms,
            });
        }
    }
}

fn same_target(left: &MouseWheelTarget, right: &MouseWheelTarget) -> bool {
    match (left, right) {
        (MouseWheelTarget::Sessions, MouseWheelTarget::Sessions) => true,
        (MouseWheelTarget::Preview(left), MouseWheelTarget::Preview(right)) => left == right,
        (
            MouseWheelTarget::Agent { reader_id: left },
            MouseWheelTarget::Agent { reader_id: right },
        ) => left == right,
        (
            MouseWheelTarget::AgentSidebar {
                workspace: left, ..
            },
            MouseWheelTarget::AgentSidebar {
                workspace: right, ..
            },
        ) => left == right,
        _ => false,
    }
}

fn wheel_direction(kind: MouseEventKind) -> Option<i32> {
    match kind {
        MouseEventKind::ScrollUp => Some(1),
        MouseEventKind::ScrollDown => Some(-1),
        _ => None,
    }
}

pub(super) fn should_forward_terminal_input(event: &Event) -> bool {
    match event {
        Event::Mouse(mouse) => wheel_direction(mouse.kind).is_some(),
        _ => true,
    }
}

pub(super) fn input_blocked(app: &App) -> bool {
    !matches!(app.input_mode, InputMode::Normal)
        || app.notice_overlay.is_some()
        || app.data_task.is_some()
        || app.ai_search_pending.is_some()
        || app.new_session_launch.is_some()
        || app.attach_in_flight.is_some()
        || app.queued_attach.is_some()
        || app.attach_terminal_input_target.is_some()
        || app.pending_runtime_action.is_some()
        || app.agent_kill_pending.is_some()
        || app.killall_pending.is_some()
        || (app.session_refresh_pending && app.session_refresh_show_overlay)
        || agent_exit_overlay_context(app).is_some()
}

fn contains_mouse(area: Rect, mouse: MouseEvent) -> bool {
    mouse.column >= area.x
        && mouse.column < area.right()
        && mouse.row >= area.y
        && mouse.row < area.bottom()
}

pub(super) fn handle_mouse_input_event(app: &mut App, mouse: MouseEvent, queued_at_epoch_ms: u64) {
    let Some(direction) = wheel_direction(mouse.kind) else {
        return;
    };
    if input_blocked(app) {
        return;
    }
    let Some(region) = app
        .mouse_wheel_regions
        .iter()
        .find(|region| contains_mouse(region.area, mouse))
        .cloned()
    else {
        return;
    };
    // A stalled input queue must not replay an old wheel gesture into a pane
    // that was attached, resized or revealed after the gesture was queued.
    // Unchanged targets retain their timestamp across ordinary redraws.
    // Equal millisecond timestamps cannot establish ordering, so fail closed
    // at the transition boundary as well.
    if queued_at_epoch_ms <= region.valid_since_epoch_ms {
        return;
    }

    let agent_view = app.is_agent_view();
    match region.target {
        MouseWheelTarget::Sessions if !agent_view => {
            app.move_selection(-direction * WHEEL_LINES);
        }
        MouseWheelTarget::Preview(key) if !agent_view => {
            // A selection change may have been processed before the next draw.
            let current_key = app
                .current()
                .map(|info| PreviewKey::new(info, app.preview_mode));
            if current_key.as_ref() == Some(&key) {
                app.scroll_preview(-direction * WHEEL_LINES);
            }
        }
        MouseWheelTarget::AgentSidebar {
            workspace,
            offset,
            total_rows,
        } if agent_view => {
            if app.agent_workspace_info().map(AgentKey::new).as_ref() != Some(&workspace) {
                return;
            }
            let current = app
                .agent_sidebar_scroll
                .as_ref()
                .filter(|scroll| scroll.workspace == workspace)
                .map_or(offset, |scroll| scroll.offset);
            let max_offset = total_rows.saturating_sub(region.area.height as usize);
            let offset = apply_scrollback_delta(current, -direction * WHEEL_LINES).min(max_offset);
            app.agent_sidebar_scroll = Some(AgentSidebarScroll { workspace, offset });
        }
        MouseWheelTarget::Agent { reader_id } if agent_view => {
            // Never forward to a replacement connection or an invisible pane.
            // No wheel action starts an attach or enters deferred input replay.
            let agent = if app
                .active_agent
                .as_ref()
                .is_some_and(|agent| agent.reader_id == reader_id)
            {
                app.active_agent.as_mut()
            } else {
                app.agent_aux
                    .as_mut()
                    .filter(|aux| aux.agent.reader_id == reader_id)
                    .map(|aux| &mut aux.agent)
            };
            if let Some(agent) = agent {
                if let Err(error) = scroll_agent_with_wheel(agent, mouse, region.area) {
                    app.status = format!("wheel scroll not sent: {error}");
                }
            }
        }
        _ => {}
    }
}

/// XTerm wheel buttons are presses 64/65, with no matching release. Coordinates
/// are pane-relative and one-based, not the enclosing cokacmux coordinates.
/// https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking
fn encode_mouse_wheel(screen: &vt100::Screen, mouse: MouseEvent, area: Rect) -> Option<Vec<u8>> {
    let direction = wheel_direction(mouse.kind)?;
    if screen.mouse_protocol_mode() == vt100::MouseProtocolMode::None
        || !contains_mouse(area, mouse)
    {
        return None;
    }
    let col = mouse.column.checked_sub(area.x)?;
    let row = mouse.row.checked_sub(area.y)?;
    let (rows, cols) = screen.size();
    if col >= cols || row >= rows {
        return None;
    }
    let x = u32::from(col) + 1;
    let y = u32::from(row) + 1;
    let mut button = if direction > 0 { 64u32 } else { 65u32 };
    // X10 press-only tracking does not encode modifier keys.
    if screen.mouse_protocol_mode() != vt100::MouseProtocolMode::Press {
        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
            button |= 4;
        }
        if mouse.modifiers.contains(KeyModifiers::ALT) {
            button |= 8;
        }
        if mouse.modifiers.contains(KeyModifiers::CONTROL) {
            button |= 16;
        }
    }
    match screen.mouse_protocol_encoding() {
        vt100::MouseProtocolEncoding::Sgr => Some(format!("\x1b[<{button};{x};{y}M").into_bytes()),
        vt100::MouseProtocolEncoding::Default => {
            if x > 223 || y > 223 {
                return None;
            }
            Some(vec![
                0x1b,
                b'[',
                b'M',
                (button + 32) as u8,
                (x + 32) as u8,
                (y + 32) as u8,
            ])
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            if x > 2015 || y > 2015 {
                return None;
            }
            let mut bytes = b"\x1b[M".to_vec();
            for codepoint in [button + 32, x + 32, y + 32] {
                let mut utf8 = [0u8; 4];
                bytes.extend_from_slice(
                    char::from_u32(codepoint)?.encode_utf8(&mut utf8).as_bytes(),
                );
            }
            Some(bytes)
        }
    }
}

fn wheel_fallback_keys(
    provider: Provider,
    direction: i32,
    overlay_open: bool,
) -> Option<Vec<KeyEvent>> {
    let action = AgentScrollAction::Pages(direction);
    match provider {
        Provider::Codex => {
            // These fixed bindings are installed in the child's keymap at
            // launch. Do not forward plain arrows into its prompt/history.
            let key = KeyEvent::new(
                if direction > 0 {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                },
                KeyModifiers::SHIFT,
            );
            let mut keys = codex_child_scroll_delegated_keys(
                AgentScrollAction::Lines(direction),
                key,
                overlay_open,
            )?;
            keys.extend(std::iter::repeat_n(key, WHEEL_LINES as usize - 1));
            Some(keys)
        }
        Provider::Claude => claude_child_scroll_key(action).map(|key| vec![key]),
        Provider::OpenCode => opencode_child_scroll_key(action).map(|key| vec![key]),
        _ => None,
    }
}

fn scroll_agent_with_wheel(
    agent: &mut AgentClient,
    mouse: MouseEvent,
    area: Rect,
) -> io::Result<()> {
    let Some(direction) = wheel_direction(mouse.kind) else {
        return Ok(());
    };
    if agent.pending_snapshot_output
        || agent.snapshot_parse_in_progress
        || agent.pending_resize.is_some()
    {
        // Wait for authoritative geometry/modes instead of sending coordinates
        // into a partially installed snapshot or an unacknowledged resize.
        return Ok(());
    }
    let connected = agent.exited.is_none() && agent.connection_ended.is_none();
    if connected && agent.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None {
        let data = encode_mouse_wheel(agent.parser.screen(), mouse, area).ok_or_else(|| {
            io::Error::new(
                ErrorKind::Unsupported,
                "wheel position is outside the child's mouse protocol range",
            )
        })?;
        return send_wheel_input(agent, data);
    }

    if connected && !is_plain_pty_tool_session_info(&agent.info) {
        if let Some(keys) = wheel_fallback_keys(
            agent.info.provider,
            direction,
            agent.codex_transcript_overlay_assumed_open,
        ) {
            let mut data = Vec::new();
            for key in keys {
                let encoded =
                    key_event_to_bytes_with_mode(key, agent.parser.screen().application_cursor())
                        .ok_or_else(|| {
                        io::Error::new(ErrorKind::Unsupported, "wheel scroll key is not encodable")
                    })?;
                data.extend_from_slice(&encoded);
            }
            // One admission check for the whole wheel action: no partial
            // sequence of open-transcript and scroll requests on queue failure.
            send_wheel_input(agent, data)?;
            if agent.info.provider == Provider::Codex {
                agent.codex_transcript_overlay_assumed_open = true;
            }
            return Ok(());
        }
    }
    // Plain shells and children without a known scroll binding retain their
    // input unchanged; the parent only moves its output-history viewport.
    agent.scroll_screen(
        AgentScrollAction::Lines(direction * WHEEL_LINES),
        area.height.max(1) as usize,
    );
    Ok(())
}

fn send_wheel_input(agent: &mut AgentClient, data: Vec<u8>) -> io::Result<()> {
    agent.send_input_data(data)?;
    if agent.scrollback_offset() > 0 {
        agent.set_scrollback_offset(0);
    }
    agent.last_input_epoch_ms = current_epoch_ms();
    agent.pending_input_since_epoch_ms = Some(agent.last_input_epoch_ms);
    agent.pending_input_key = Some("mouse wheel".into());
    agent.pending_input_count = agent.pending_input_count.saturating_add(1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{
        app_for_key_tests, buffered_output_test_client_with_requests, session_info,
    };
    use super::*;
    use crossterm::event::MouseButton;

    fn wheel(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn dispatch(app: &mut App, event: MouseEvent) {
        // Model an event read after the rendered frame, without sleeping.
        handle_mouse_input_event(app, event, current_epoch_ms().saturating_add(1));
    }

    fn request_bytes(requests: &Receiver<AgentWriterRequest>) -> Vec<Vec<u8>> {
        requests
            .try_iter()
            .map(|request| match request.request {
                AgentDaemonRequest::Input { data, .. } => data,
                _ => panic!("wheel must not attach, resize or detach a session"),
            })
            .collect()
    }

    #[test]
    fn mouse_motion_clicks_and_horizontal_wheels_do_not_enter_ui_queue() {
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            assert!(!should_forward_terminal_input(&Event::Mouse(wheel(
                kind, 1, 1
            ))));
        }
        for kind in [MouseEventKind::ScrollUp, MouseEventKind::ScrollDown] {
            assert!(should_forward_terminal_input(&Event::Mouse(wheel(
                kind, 1, 1
            ))));
        }
        assert!(should_forward_terminal_input(&Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        ))));
        assert!(should_forward_terminal_input(&Event::Paste("kept".into())));
    }

    #[test]
    fn wheel_hit_test_excludes_borders_and_empty_rectangles() {
        let area = Rect::new(10, 4, 20, 8);
        for (col, row, expected) in [
            (10, 4, true),
            (29, 11, true),
            (9, 4, false),
            (30, 4, false),
            (10, 3, false),
            (10, 12, false),
        ] {
            assert_eq!(
                contains_mouse(area, wheel(MouseEventKind::ScrollUp, col, row)),
                expected
            );
        }
        assert!(!contains_mouse(
            Rect::new(10, 4, 0, 0),
            wheel(MouseEventKind::ScrollUp, 10, 4)
        ));
    }

    #[test]
    fn sgr_wheel_uses_pane_relative_coordinates_and_modifier_bits() {
        let mut parser = vt100::Parser::new(8, 80, 0);
        let area = Rect::new(20, 7, 80, 8);
        let mut event = wheel(MouseEventKind::ScrollDown, 23, 9);
        assert!(encode_mouse_wheel(parser.screen(), event, area).is_none());
        parser.process(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(
            encode_mouse_wheel(parser.screen(), event, area).unwrap(),
            b"\x1b[<65;4;3M"
        );
        event.modifiers = KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL;
        assert_eq!(
            encode_mouse_wheel(parser.screen(), event, area).unwrap(),
            b"\x1b[<93;4;3M"
        );
        event.kind = MouseEventKind::ScrollUp;
        event.modifiers = KeyModifiers::NONE;
        assert_eq!(
            encode_mouse_wheel(parser.screen(), event, area).unwrap(),
            b"\x1b[<64;4;3M"
        );
        event.column = 19;
        assert!(encode_mouse_wheel(parser.screen(), event, area).is_none());
    }

    #[test]
    fn legacy_wheel_encodings_reject_unrepresentable_coordinates() {
        let mut parser = vt100::Parser::new(8, 2020, 0);
        let area = Rect::new(0, 0, 2020, 8);
        parser.process(b"\x1b[?1000h");
        assert_eq!(
            encode_mouse_wheel(
                parser.screen(),
                wheel(MouseEventKind::ScrollUp, 222, 0),
                area
            )
            .unwrap(),
            [0x1b, b'[', b'M', 96, 255, 33]
        );
        assert!(encode_mouse_wheel(
            parser.screen(),
            wheel(MouseEventKind::ScrollUp, 223, 0),
            area
        )
        .is_none());
        parser.process(b"\x1b[?1005h");
        assert_eq!(
            encode_mouse_wheel(
                parser.screen(),
                wheel(MouseEventKind::ScrollUp, 2014, 0),
                area
            )
            .unwrap(),
            [0x1b, b'[', b'M', 96, 0xdf, 0xbf, 33]
        );
        assert!(encode_mouse_wheel(
            parser.screen(),
            wheel(MouseEventKind::ScrollUp, 2015, 0),
            area
        )
        .is_none());
        parser.process(b"\x1b[?1006h");
        assert_eq!(
            encode_mouse_wheel(
                parser.screen(),
                wheel(MouseEventKind::ScrollUp, 2015, 0),
                area
            )
            .unwrap(),
            b"\x1b[<64;2016;1M"
        );
        parser.process(b"\x1b[?9h");
        let mut event = wheel(MouseEventKind::ScrollUp, 0, 0);
        event.modifiers = KeyModifiers::CONTROL;
        assert_eq!(
            encode_mouse_wheel(parser.screen(), event, area).unwrap(),
            b"\x1b[<64;1;1M"
        );
    }

    #[test]
    fn session_list_and_preview_follow_pointer_without_changing_focus() {
        let mut app = app_for_key_tests();
        for index in 0..10 {
            app.sessions.push(session_info(
                Provider::Claude,
                &format!("session-{index}"),
                "/repo",
            ));
        }
        app.list_state.select(Some(0));
        app.focus = FocusPane::Preview;
        let backend = ratatui::backend::TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_list(frame, &mut app, Rect::new(0, 0, 30, 10)))
            .unwrap();
        dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 3, 3));
        assert_eq!(app.list_state.selected(), Some(3));
        assert_eq!(app.focus, FocusPane::Preview);
        dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 3, 1)); // column header
        assert_eq!(app.list_state.selected(), Some(3));

        let key = PreviewKey::new(app.current().unwrap(), app.preview_mode);
        app.record_mouse_wheel_region(Rect::new(31, 1, 28, 8), MouseWheelTarget::Preview(key));
        app.focus = FocusPane::Sessions;
        dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 35, 3));
        assert_eq!(app.preview_scroll, 3);
        assert_eq!(app.list_state.selected(), Some(3));
        assert_eq!(app.focus, FocusPane::Sessions);
        dispatch(&mut app, wheel(MouseEventKind::ScrollUp, 35, 3));
        dispatch(&mut app, wheel(MouseEventKind::ScrollUp, 35, 3));
        assert_eq!(app.preview_scroll, 0);
        app.move_selection(1);
        dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 35, 3));
        assert_eq!(
            app.preview_scroll, 0,
            "old preview hit target must not scroll a new selection"
        );
    }

    #[test]
    fn modal_and_refresh_overlay_block_wheels() {
        let mut app = app_for_key_tests();
        let info = session_info(Provider::Claude, "preview", "/repo");
        app.sessions.push(info.clone());
        app.list_state.select(Some(0));
        app.record_mouse_wheel_region(
            Rect::new(0, 0, 20, 8),
            MouseWheelTarget::Preview(PreviewKey::new(&info, app.preview_mode)),
        );
        app.input_mode = InputMode::Notice {
            title: "notice".into(),
            message: "blocked".into(),
        };
        dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 1, 1));
        assert_eq!(app.preview_scroll, 0);
        app.input_mode = InputMode::Normal;
        app.session_refresh_pending = true;
        app.session_refresh_show_overlay = true;
        dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 1, 1));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn redraw_preserves_wheel_age_only_for_unchanged_targets() {
        let mut app = app_for_key_tests();
        let area = Rect::new(0, 0, 20, 8);
        app.record_mouse_wheel_region(area, MouseWheelTarget::Agent { reader_id: 1 });
        app.mouse_wheel_regions[0].valid_since_epoch_ms = 1;
        app.begin_mouse_wheel_frame();
        app.record_mouse_wheel_region(area, MouseWheelTarget::Agent { reader_id: 1 });
        assert_eq!(app.mouse_wheel_regions[0].valid_since_epoch_ms, 1);
        app.begin_mouse_wheel_frame();
        app.record_mouse_wheel_region(area, MouseWheelTarget::Agent { reader_id: 2 });
        assert!(app.mouse_wheel_regions[0].valid_since_epoch_ms > 1);
    }

    #[test]
    fn plain_terminal_wheel_changes_history_without_writing_to_pty() {
        let (mut agent, requests) =
            buffered_output_test_client_with_requests("wheel-history", 9920);
        agent.info.source = PathBuf::from(SHELL_SESSION_SOURCE_MARKER);
        for index in 0..30 {
            agent.parser.process(format!("line {index}\r\n").as_bytes());
        }
        scroll_agent_with_wheel(
            &mut agent,
            wheel(MouseEventKind::ScrollUp, 1, 1),
            Rect::new(0, 0, 80, 8),
        )
        .unwrap();
        assert_eq!(agent.scrollback_offset(), WHEEL_LINES as usize);
        assert!(requests.try_recv().is_err());
        assert_eq!(
            agent.last_input_epoch_ms, 0,
            "local scrolling is not child input activity"
        );
        scroll_agent_with_wheel(
            &mut agent,
            wheel(MouseEventKind::ScrollDown, 1, 1),
            Rect::new(0, 0, 80, 8),
        )
        .unwrap();
        assert_eq!(agent.scrollback_offset(), 0);
        agent.exited = Some("test cleanup".into());
    }

    #[test]
    fn cli_wheel_prefers_native_mouse_then_known_scroll_keys() {
        let (mut agent, requests) = buffered_output_test_client_with_requests("wheel-cli", 9921);
        let area = Rect::new(10, 5, 80, 8);
        agent.info.provider = Provider::Codex;
        agent.parser.process(b"\x1b[?1000h\x1b[?1006h");
        scroll_agent_with_wheel(&mut agent, wheel(MouseEventKind::ScrollUp, 12, 6), area).unwrap();
        assert_eq!(request_bytes(&requests), [b"\x1b[<64;3;2M".to_vec()]);
        assert!(!agent.codex_transcript_overlay_assumed_open);
        agent.parser.process(b"\x1b[?1000l");
        scroll_agent_with_wheel(&mut agent, wheel(MouseEventKind::ScrollUp, 12, 6), area).unwrap();
        assert_eq!(request_bytes(&requests), [b"\x1b[1;2A".repeat(4)]);
        assert!(agent.codex_transcript_overlay_assumed_open);
        scroll_agent_with_wheel(&mut agent, wheel(MouseEventKind::ScrollDown, 12, 6), area)
            .unwrap();
        assert_eq!(request_bytes(&requests), [b"\x1b[1;2B".repeat(3)]);
        for provider in [Provider::Claude, Provider::OpenCode] {
            agent.info.provider = provider;
            scroll_agent_with_wheel(&mut agent, wheel(MouseEventKind::ScrollUp, 12, 6), area)
                .unwrap();
            assert_eq!(request_bytes(&requests), [b"\x1b[5~".to_vec()]);
        }
        agent.exited = Some("test cleanup".into());
    }

    #[test]
    fn wheel_targets_visible_reader_not_focus_or_replacement_connection() {
        let mut app = app_for_key_tests();
        let (mut main, main_requests) =
            buffered_output_test_client_with_requests("wheel-main", 9922);
        let (mut auxiliary, aux_requests) =
            buffered_output_test_client_with_requests("wheel-aux", 9923);
        main.parser.process(b"\x1b[?1000h\x1b[?1006h");
        auxiliary.parser.process(b"\x1b[?1000h\x1b[?1006h");
        let parent = AgentKey::new(&main.info);
        app.active_agent = Some(main);
        app.agent_aux = Some(AgentAuxPane {
            kind: AgentAuxKind::Terminal,
            parent,
            agent: auxiliary,
        });
        app.show_sessions_view = false;
        app.agent_focus = AgentFocusPane::Main;
        app.record_mouse_wheel_region(
            Rect::new(40, 1, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9923 },
        );
        dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 42, 2));
        assert!(main_requests.try_recv().is_err());
        assert_eq!(request_bytes(&aux_requests), [b"\x1b[<65;3;2M".to_vec()]);
        assert_eq!(app.agent_focus, AgentFocusPane::Main);
        app.agent_aux.as_mut().unwrap().agent.reader_id = 9924;
        dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 42, 2));
        assert!(aux_requests.try_recv().is_err());
        app.begin_mouse_wheel_frame();
        app.record_mouse_wheel_region(
            Rect::new(40, 1, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9924 },
        );
        handle_mouse_input_event(&mut app, wheel(MouseEventKind::ScrollDown, 42, 2), 1);
        assert!(
            aux_requests.try_recv().is_err(),
            "queued old gesture must not reach a newly rendered client"
        );
        let transition_time = app.mouse_wheel_regions[0].valid_since_epoch_ms;
        handle_mouse_input_event(
            &mut app,
            wheel(MouseEventKind::ScrollDown, 42, 2),
            transition_time,
        );
        assert!(aux_requests.try_recv().is_err());
        app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
        app.agent_aux.as_mut().unwrap().agent.exited = Some("test cleanup".into());
    }

    #[test]
    fn sidebar_wheel_scrolls_without_an_attach_or_focus_change() {
        let mut app = app_for_key_tests();
        let (main, requests) = buffered_output_test_client_with_requests("sidebar-first", 9925);
        let key = AgentKey::new(&main.info);
        let mut candidates = vec![main.info.clone()];
        for index in 1..15 {
            candidates.push(session_info(
                Provider::Claude,
                &format!("sidebar-{index}"),
                "/repo",
            ));
        }
        app.active_agent = Some(main);
        app.show_sessions_view = false;
        app.agent_focus = AgentFocusPane::Main;
        let backend = ratatui::backend::TestBackend::new(32, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_agent_sidebar(
                    frame,
                    &mut app,
                    Rect::new(0, 0, 32, 6),
                    &candidates,
                    &key,
                    false,
                )
            })
            .unwrap();
        for _ in 0..2 {
            dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 4, 2));
        }
        assert_eq!(app.agent_sidebar_scroll.as_ref().unwrap().offset, 6);
        terminal
            .draw(|frame| {
                app.begin_mouse_wheel_frame();
                draw_agent_sidebar(
                    frame,
                    &mut app,
                    Rect::new(0, 0, 32, 6),
                    &candidates,
                    &key,
                    false,
                );
            })
            .unwrap();
        assert!(matches!(
            &app.mouse_wheel_regions[0].target,
            MouseWheelTarget::AgentSidebar { offset: 6, .. }
        ));
        assert_eq!(app.current_active_agent_key(), Some(key));
        assert_eq!(app.agent_focus, AgentFocusPane::Main);
        assert!(app.attach_in_flight.is_none());
        assert!(app.queued_attach.is_none());
        assert!(requests.try_recv().is_err());
        for _ in 0..20 {
            dispatch(&mut app, wheel(MouseEventKind::ScrollDown, 4, 2));
        }
        assert_eq!(app.agent_sidebar_scroll.as_ref().unwrap().offset, 11);
        app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
    }

    #[test]
    fn wheel_waits_for_snapshot_and_resize_and_keeps_failed_input_state() {
        let (mut agent, requests) =
            buffered_output_test_client_with_requests("wheel-pending", 9926);
        agent.info.provider = Provider::Codex;
        let event = wheel(MouseEventKind::ScrollUp, 1, 1);
        let area = Rect::new(0, 0, 80, 8);
        agent.snapshot_parse_in_progress = true;
        scroll_agent_with_wheel(&mut agent, event, area).unwrap();
        agent.snapshot_parse_in_progress = false;
        agent.pending_resize = Some((8, 80));
        scroll_agent_with_wheel(&mut agent, event, area).unwrap();
        assert!(requests.try_recv().is_err());
        agent.pending_resize = None;
        drop(requests);
        assert!(scroll_agent_with_wheel(&mut agent, event, area).is_err());
        assert!(!agent.codex_transcript_overlay_assumed_open);
        assert_eq!(agent.last_input_epoch_ms, 0);
        agent.exited = Some("test cleanup".into());
    }
}
