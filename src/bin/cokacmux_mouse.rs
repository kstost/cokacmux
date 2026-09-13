//! Mouse support. Hit testing uses the last rendered content
//! rectangles, never a guessed terminal size or the keyboard focus.

use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

const WHEEL_LINES: i32 = 3;

#[derive(Debug, Clone)]
pub(super) struct MouseButtonCapture {
    reader_id: u64,
    area: Rect,
    button: MouseButton,
    pressed_at: u64,
    press_position: (u16, u16),
    last_position: (u16, u16),
    modifiers: KeyModifiers,
    mode: vt100::MouseProtocolMode,
    encoding: vt100::MouseProtocolEncoding,
    release_pending: bool,
}

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
        Event::Mouse(mouse) => mouse.kind != MouseEventKind::Moved,
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
    if matches!(
        mouse.kind,
        MouseEventKind::Down(_) | MouseEventKind::Drag(_) | MouseEventKind::Up(_)
    ) {
        handle_button_input(app, mouse, queued_at_epoch_ms);
        return;
    }
    if matches!(
        mouse.kind,
        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
    ) {
        if input_blocked(app) || !app.is_agent_view() {
            return;
        }
        let region = app
            .mouse_wheel_regions
            .iter()
            .find(|region| {
                contains_mouse(region.area, mouse)
                    && queued_at_epoch_ms > region.valid_since_epoch_ms
            })
            .cloned();
        if let Some(MouseWheelRegion {
            area,
            target: MouseWheelTarget::Agent { reader_id },
            ..
        }) = region
        {
            if let Some(agent) = mouse_agent(app, reader_id) {
                if agent_accepts_mouse(agent) {
                    if let Some(data) = encode_mouse_event(agent.parser.screen(), mouse, area) {
                        if let Err(error) = send_wheel_input(agent, data) {
                            app.status = format!("mouse input not sent: {error}");
                        }
                    }
                }
            }
        }
        return;
    }
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
    wheel_direction(mouse.kind)?;
    encode_mouse_event(screen, mouse, area)
}

fn encode_mouse_event(screen: &vt100::Screen, mouse: MouseEvent, area: Rect) -> Option<Vec<u8>> {
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
    let button_code = |button| match button {
        MouseButton::Left => 0u32,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let mode = screen.mouse_protocol_mode();
    let release = matches!(mouse.kind, MouseEventKind::Up(_));
    let mut button = match mouse.kind {
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        MouseEventKind::ScrollLeft => 66,
        MouseEventKind::ScrollRight => 67,
        MouseEventKind::Down(button) => button_code(button),
        MouseEventKind::Up(button) if mode != vt100::MouseProtocolMode::Press => {
            if screen.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr {
                button_code(button)
            } else {
                3
            }
        }
        MouseEventKind::Drag(button)
            if matches!(
                mode,
                vt100::MouseProtocolMode::ButtonMotion | vt100::MouseProtocolMode::AnyMotion
            ) =>
        {
            button_code(button) | 32
        }
        _ => return None,
    };
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
        vt100::MouseProtocolEncoding::Sgr => {
            let suffix = if release { 'm' } else { 'M' };
            Some(format!("\x1b[<{button};{x};{y}{suffix}").into_bytes())
        }
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

fn mouse_agent(app: &mut App, reader_id: u64) -> Option<&mut AgentClient> {
    if app
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
    }
}

fn agent_accepts_mouse(agent: &AgentClient) -> bool {
    agent.exited.is_none()
        && agent.connection_ended.is_none()
        && !agent.pending_snapshot_output
        && !agent.snapshot_parse_in_progress
        && agent.pending_resize.is_none()
        && agent.scrollback_offset() == 0
}

fn captured_event(
    capture: &MouseButtonCapture,
    kind: MouseEventKind,
    modifiers: KeyModifiers,
    screen: &vt100::Screen,
) -> MouseEvent {
    let protocol_limit = match capture.encoding {
        vt100::MouseProtocolEncoding::Default => 223,
        vt100::MouseProtocolEncoding::Utf8 => 2015,
        vt100::MouseProtocolEncoding::Sgr => u16::MAX,
    };
    let (rows, columns) = screen.size();
    let width = capture.area.width.min(columns).min(protocol_limit).max(1);
    let height = capture.area.height.min(rows).min(protocol_limit).max(1);
    MouseEvent {
        kind,
        modifiers,
        column: capture
            .last_position
            .0
            .clamp(capture.area.x, capture.area.x.saturating_add(width - 1)),
        row: capture
            .last_position
            .1
            .clamp(capture.area.y, capture.area.y.saturating_add(height - 1)),
    }
}

/// Keep rejected releases on their original connection, ahead of later input.
/// The writer's acknowledgement queue owns already admitted bytes, including
/// bytes retained after a writer failure; those must never be submitted twice.
pub(super) fn cancel_mouse_capture(app: &mut App) {
    let Some(mut capture) = app.mouse_button_capture.take() else {
        return;
    };
    // End a cancelled gesture at its origin so losing focus or resizing does
    // not accidentally commit a cross-panel drop in the child.
    capture.last_position = capture.press_position;
    release_mouse_capture(app, capture);
}

fn release_mouse_capture(app: &mut App, mut capture: MouseButtonCapture) {
    capture.release_pending = true;
    let Some(agent) = mouse_agent(app, capture.reader_id) else {
        return;
    };
    agent.pending_mouse_release = Some(capture);
    if let Err(error) = flush_pending_release(agent) {
        app.status = format!("mouse release waiting: {error}");
    }
}

pub(super) fn flush_pending_release(agent: &mut AgentClient) -> io::Result<()> {
    if agent
        .pending_mouse_release
        .as_ref()
        .is_some_and(|capture| !capture.release_pending)
    {
        return Ok(());
    }
    let Some(capture) = agent.pending_mouse_release.take() else {
        return Ok(());
    };
    if capture.reader_id != agent.reader_id
        || agent.exited.is_some()
        || agent.connection_ended.is_some()
    {
        return Ok(());
    }
    if agent.pending_snapshot_output
        || agent.snapshot_parse_in_progress
        || agent.pending_resize.is_some()
    {
        agent.pending_mouse_release = Some(capture);
        return Err(io::Error::new(
            ErrorKind::WouldBlock,
            "mouse release is waiting for the child's current screen",
        ));
    }
    if agent.parser.screen().mouse_protocol_mode() != capture.mode
        || agent.parser.screen().mouse_protocol_encoding() != capture.encoding
    {
        return Ok(());
    }
    let event = captured_event(
        &capture,
        MouseEventKind::Up(capture.button),
        capture.modifiers,
        agent.parser.screen(),
    );
    if let Some(data) = encode_mouse_event(agent.parser.screen(), event, capture.area) {
        if let Err(error) = send_admitted_mouse_input(agent, data) {
            agent.pending_mouse_release = Some(capture);
            return Err(error);
        }
    }
    Ok(())
}

/// AgentClient::drop still owns the original writer, so release before detach.
pub(super) fn release_before_detach(agent: &mut AgentClient) {
    if let Some(capture) = agent.pending_mouse_release.as_mut() {
        if !capture.release_pending {
            capture.last_position = capture.press_position;
            capture.release_pending = true;
        }
    }
    let _ = flush_pending_release(agent);
}

fn send_admitted_mouse_input(agent: &mut AgentClient, data: Vec<u8>) -> io::Result<()> {
    let retained_before = agent.unacknowledged_input.len();
    let result = agent.send_input_data_inner(data);
    if result.is_err() && agent.unacknowledged_input.len() == retained_before {
        return result;
    }
    record_mouse_input(agent);
    Ok(())
}

fn send_button_input(agent: &mut AgentClient, data: Vec<u8>) -> io::Result<()> {
    flush_pending_release(agent)?;
    send_admitted_mouse_input(agent, data)
}

pub(super) fn finish_mouse_frame(app: &mut App) {
    let cancel = app.mouse_button_capture.as_ref().is_some_and(|capture|
        input_blocked(app) || !app.is_agent_view() || !app.mouse_wheel_regions.iter().any(|region|
            region.area == capture.area && matches!(region.target, MouseWheelTarget::Agent { reader_id } if reader_id == capture.reader_id)));
    if cancel {
        cancel_mouse_capture(app);
    }
    for agent in app
        .active_agent
        .iter_mut()
        .chain(app.agent_aux.iter_mut().map(|aux| &mut aux.agent))
    {
        if let Err(error) = flush_pending_release(agent) {
            app.status = format!("mouse release waiting: {error}");
        }
    }
}

fn handle_button_input(app: &mut App, mouse: MouseEvent, queued_at: u64) {
    if input_blocked(app) || !app.is_agent_view() {
        cancel_mouse_capture(app);
        return;
    }
    if let MouseEventKind::Down(button) = mouse.kind {
        cancel_mouse_capture(app);
        let region = app
            .mouse_wheel_regions
            .iter()
            .find(|region| {
                contains_mouse(region.area, mouse) && queued_at > region.valid_since_epoch_ms
            })
            .cloned();
        let Some(MouseWheelRegion {
            area,
            target: MouseWheelTarget::Agent { reader_id },
            ..
        }) = region
        else {
            return;
        };
        let Some(agent) = mouse_agent(app, reader_id) else {
            return;
        };
        if !agent_accepts_mouse(agent) {
            return;
        }
        let Some(data) = encode_mouse_event(agent.parser.screen(), mouse, area) else {
            return;
        };
        let mode = agent.parser.screen().mouse_protocol_mode();
        let encoding = agent.parser.screen().mouse_protocol_encoding();
        if let Err(error) = send_button_input(agent, data) {
            app.status = format!("mouse press not sent: {error}");
            return;
        }
        let capture = (mode != vt100::MouseProtocolMode::Press).then_some(MouseButtonCapture {
            reader_id,
            area,
            button,
            pressed_at: queued_at,
            press_position: (mouse.column, mouse.row),
            last_position: (mouse.column, mouse.row),
            mode,
            encoding,
            modifiers: mouse.modifiers,
            release_pending: false,
        });
        agent.pending_mouse_release = capture.clone();
        app.agent_focus = if app
            .active_agent
            .as_ref()
            .is_some_and(|agent| agent.reader_id == reader_id)
        {
            AgentFocusPane::Main
        } else {
            AgentFocusPane::Auxiliary
        };
        app.mouse_button_capture = capture;
        return;
    }
    let Some(mut capture) = app.mouse_button_capture.take() else {
        return;
    };
    let button = match mouse.kind {
        MouseEventKind::Drag(button) | MouseEventKind::Up(button) => button,
        _ => return,
    };
    if button != capture.button || queued_at < capture.pressed_at {
        app.mouse_button_capture = Some(capture);
        return;
    }
    let current_region = app.mouse_wheel_regions.iter().any(|region|
        region.area == capture.area && matches!(region.target, MouseWheelTarget::Agent { reader_id } if reader_id == capture.reader_id));
    if !current_region {
        app.mouse_button_capture = Some(capture);
        cancel_mouse_capture(app);
        return;
    }
    capture.last_position = (mouse.column, mouse.row);
    capture.modifiers = mouse.modifiers;
    if matches!(mouse.kind, MouseEventKind::Up(_)) {
        release_mouse_capture(app, capture);
        return;
    }
    let Some(agent) = mouse_agent(app, capture.reader_id) else {
        return;
    };
    if !agent_accepts_mouse(agent) {
        app.mouse_button_capture = Some(capture);
        cancel_mouse_capture(app);
        return;
    }
    if agent.parser.screen().mouse_protocol_mode() != capture.mode
        || agent.parser.screen().mouse_protocol_encoding() != capture.encoding
    {
        return;
    }
    let event = captured_event(&capture, mouse.kind, mouse.modifiers, agent.parser.screen());
    if let Some(data) = encode_mouse_event(agent.parser.screen(), event, capture.area) {
        if let Err(error) = send_button_input(agent, data) {
            app.status = format!("mouse input not sent: {error}");
            app.mouse_button_capture = Some(capture);
            return;
        }
    }
    app.mouse_button_capture = Some(capture);
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
    record_mouse_input(agent);
    Ok(())
}

fn record_mouse_input(agent: &mut AgentClient) {
    agent.last_input_epoch_ms = current_epoch_ms();
    agent.pending_input_since_epoch_ms = Some(agent.last_input_epoch_ms);
    agent.pending_input_key = Some("mouse".into());
    agent.pending_input_count = agent.pending_input_count.saturating_add(1);
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
    fn hover_is_filtered_but_buttons_drags_releases_and_wheels_are_preserved() {
        assert!(!should_forward_terminal_input(&Event::Mouse(wheel(
            MouseEventKind::Moved,
            1,
            1
        ))));
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            assert!(should_forward_terminal_input(&Event::Mouse(wheel(
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
    fn native_buttons_use_child_modes_and_sgr_release_suffix() {
        let mut parser = vt100::Parser::new(8, 80, 0);
        let area = Rect::new(20, 7, 80, 8);
        let down = wheel(MouseEventKind::Down(MouseButton::Left), 23, 9);
        let drag = wheel(MouseEventKind::Drag(MouseButton::Left), 24, 10);
        let up = wheel(MouseEventKind::Up(MouseButton::Left), 24, 10);
        assert!(encode_mouse_event(parser.screen(), down, area).is_none());
        parser.process(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(
            encode_mouse_event(parser.screen(), down, area).unwrap(),
            b"\x1b[<0;4;3M"
        );
        assert!(encode_mouse_event(parser.screen(), drag, area).is_none());
        assert_eq!(
            encode_mouse_event(parser.screen(), up, area).unwrap(),
            b"\x1b[<0;5;4m"
        );
        parser.process(b"\x1b[?1002h");
        assert_eq!(
            encode_mouse_event(parser.screen(), drag, area).unwrap(),
            b"\x1b[<32;5;4M"
        );
        parser.process(b"\x1b[?1006l");
        assert_eq!(
            encode_mouse_event(parser.screen(), up, area).unwrap(),
            vec![27, b'[', b'M', 35, 37, 36]
        );
        parser.process(b"\x1b[?9h");
        assert!(encode_mouse_event(parser.screen(), up, area).is_none());
        assert!(encode_mouse_event(parser.screen(), drag, area).is_none());
    }

    #[test]
    fn drag_and_release_stay_with_pressed_reader_when_pointer_crosses_panes() {
        let mut app = app_for_key_tests();
        let (mut main, main_requests) =
            buffered_output_test_client_with_requests("mouse-main", 9941);
        let (mut auxiliary, aux_requests) =
            buffered_output_test_client_with_requests("mouse-aux", 9942);
        main.parser.process(b"\x1b[?1002h\x1b[?1006h");
        auxiliary.parser.process(b"\x1b[?1002h\x1b[?1006h");
        let parent = AgentKey::new(&main.info);
        app.active_agent = Some(main);
        app.agent_aux = Some(AgentAuxPane {
            kind: AgentAuxKind::Terminal,
            parent,
            agent: auxiliary,
        });
        app.show_sessions_view = false;
        app.record_mouse_wheel_region(
            Rect::new(0, 0, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9941 },
        );
        app.record_mouse_wheel_region(
            Rect::new(40, 0, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9942 },
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Down(MouseButton::Left), 2, 2),
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Drag(MouseButton::Left), 42, 3),
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Up(MouseButton::Left), 42, 3),
        );
        assert_eq!(
            request_bytes(&main_requests),
            [
                b"\x1b[<0;3;3M".to_vec(),
                b"\x1b[<32;40;4M".to_vec(),
                b"\x1b[<0;40;4m".to_vec(),
            ]
        );
        assert!(aux_requests.try_recv().is_err());
        assert!(app.mouse_button_capture.is_none());
        assert_eq!(app.agent_focus, AgentFocusPane::Main);
        app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
        app.agent_aux.as_mut().unwrap().agent.exited = Some("test cleanup".into());
    }

    #[test]
    fn replaced_reader_never_receives_an_old_drag_or_release() {
        let mut app = app_for_key_tests();
        let (mut main, requests) =
            buffered_output_test_client_with_requests("mouse-replaced", 9943);
        main.parser.process(b"\x1b[?1002h\x1b[?1006h");
        app.active_agent = Some(main);
        app.show_sessions_view = false;
        app.record_mouse_wheel_region(
            Rect::new(0, 0, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9943 },
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Down(MouseButton::Left), 2, 2),
        );
        assert_eq!(request_bytes(&requests).len(), 1);
        app.active_agent.as_mut().unwrap().reader_id = 9944;
        dispatch(
            &mut app,
            wheel(MouseEventKind::Drag(MouseButton::Left), 4, 2),
        );
        dispatch(&mut app, wheel(MouseEventKind::Up(MouseButton::Left), 4, 2));
        assert!(requests.try_recv().is_err());
        assert!(app.mouse_button_capture.is_none());
        app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
    }

    #[test]
    fn release_waits_for_screen_and_precedes_the_next_key_with_its_modifiers() {
        let mut app = app_for_key_tests();
        let (mut main, requests) = buffered_output_test_client_with_requests("mouse-release", 9945);
        main.parser.process(b"\x1b[?1002h\x1b[?1006h");
        app.active_agent = Some(main);
        app.show_sessions_view = false;
        app.record_mouse_wheel_region(
            Rect::new(0, 0, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9945 },
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Down(MouseButton::Left), 2, 2),
        );
        assert_eq!(request_bytes(&requests).len(), 1);
        app.active_agent.as_mut().unwrap().pending_snapshot_output = true;
        let mut release = wheel(MouseEventKind::Up(MouseButton::Left), 12, 3);
        release.modifiers = KeyModifiers::SHIFT;
        dispatch(&mut app, release);
        let main = app.active_agent.as_mut().unwrap();
        assert!(main.pending_mouse_release.is_some());
        assert!(main.send_input_data(b"x".to_vec()).is_err());
        assert!(requests.try_recv().is_err());
        main.pending_snapshot_output = false;
        main.send_input_data(b"x".to_vec()).unwrap();
        assert_eq!(
            request_bytes(&requests),
            [b"\x1b[<4;13;4m".to_vec(), b"x".to_vec()]
        );
        assert!(main.pending_mouse_release.is_none());
        finish_mouse_frame(&mut app);
        assert!(requests.try_recv().is_err());
        app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
    }

    #[test]
    fn release_retained_after_writer_failure_is_not_submitted_twice() {
        let mut app = app_for_key_tests();
        let (mut main, requests) =
            buffered_output_test_client_with_requests("mouse-retained", 9946);
        main.parser.process(b"\x1b[?1002h\x1b[?1006h");
        app.active_agent = Some(main);
        app.show_sessions_view = false;
        app.record_mouse_wheel_region(
            Rect::new(0, 0, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9946 },
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Down(MouseButton::Left), 2, 2),
        );
        assert_eq!(request_bytes(&requests).len(), 1);
        app.active_agent.as_mut().unwrap().input_acknowledgements = true;
        drop(requests);
        dispatch(&mut app, wheel(MouseEventKind::Up(MouseButton::Left), 4, 3));
        let main = app.active_agent.as_mut().unwrap();
        assert!(main.pending_mouse_release.is_none());
        assert_eq!(main.unacknowledged_input.len(), 1);
        assert_eq!(
            main.unacknowledged_input.front().unwrap().data,
            b"\x1b[<0;5;4m"
        );
        flush_pending_release(main).unwrap();
        assert_eq!(main.unacknowledged_input.len(), 1);
        assert!(main.send_input_data(b"x".to_vec()).is_err());
        main.exited = Some("test cleanup".into());
    }

    #[test]
    fn cancellation_releases_at_origin_and_ignores_a_late_physical_release() {
        let mut app = app_for_key_tests();
        let (mut main, requests) = buffered_output_test_client_with_requests("mouse-cancel", 9947);
        main.parser.process(b"\x1b[?1002h\x1b[?1006h");
        app.active_agent = Some(main);
        app.show_sessions_view = false;
        app.record_mouse_wheel_region(
            Rect::new(0, 0, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9947 },
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Down(MouseButton::Left), 2, 2),
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Drag(MouseButton::Left), 30, 3),
        );
        assert_eq!(request_bytes(&requests).len(), 2);
        cancel_mouse_capture(&mut app);
        dispatch(
            &mut app,
            wheel(MouseEventKind::Up(MouseButton::Left), 30, 3),
        );
        assert_eq!(request_bytes(&requests), [b"\x1b[<0;3;3m".to_vec()]);
        app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
    }

    #[test]
    fn detaching_a_pressed_reader_sends_its_release_before_detach() {
        let mut app = app_for_key_tests();
        let (mut main, requests) = buffered_output_test_client_with_requests("mouse-detach", 9948);
        main.parser.process(b"\x1b[?1002h\x1b[?1006h");
        app.active_agent = Some(main);
        app.show_sessions_view = false;
        app.record_mouse_wheel_region(
            Rect::new(0, 0, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9948 },
        );
        dispatch(
            &mut app,
            wheel(MouseEventKind::Down(MouseButton::Left), 2, 2),
        );
        assert_eq!(request_bytes(&requests).len(), 1);
        drop(app.active_agent.take());
        let release = requests.try_recv().unwrap();
        assert!(
            matches!(release.request, AgentDaemonRequest::Input { data, .. } if data == b"\x1b[<0;3;3m")
        );
        dispatch(&mut app, wheel(MouseEventKind::Up(MouseButton::Left), 4, 3));
        assert!(app.mouse_button_capture.is_none());
    }

    #[test]
    fn switching_to_sessions_blocks_buttons_before_the_next_redraw() {
        let mut app = app_for_key_tests();
        let (mut main, requests) = buffered_output_test_client_with_requests("mouse-hidden", 9949);
        main.parser.process(b"\x1b[?1002h\x1b[?1006h");
        app.active_agent = Some(main);
        app.show_sessions_view = false;
        app.record_mouse_wheel_region(
            Rect::new(0, 0, 40, 8),
            MouseWheelTarget::Agent { reader_id: 9949 },
        );
        app.show_sessions_view = true;
        dispatch(
            &mut app,
            wheel(MouseEventKind::Down(MouseButton::Left), 2, 2),
        );
        dispatch(&mut app, wheel(MouseEventKind::ScrollRight, 2, 2));
        assert!(requests.try_recv().is_err());
        assert!(app.mouse_button_capture.is_none());
        app.active_agent.as_mut().unwrap().exited = Some("test cleanup".into());
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
