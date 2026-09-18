//! Local selection of the last displayed PTY viewport. Only the selected
//! viewport is frozen; the agent and its output parser continue running.

use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

#[derive(Default)]
pub(super) struct TextSelection {
    frames: Vec<Arc<SelectionFrame>>,
    active: Option<SelectedText>,
    clipboard_request: Option<Vec<u8>>,
    copy_status: Option<String>,
}

struct SelectionFrame {
    reader_id: u64,
    buffer: Buffer,
    wrapped: Vec<bool>,
}

struct SelectedText {
    frame: Arc<SelectionFrame>,
    // Row before column so tuple ordering follows reading order.
    anchor: (u16, u16),
    end: (u16, u16),
    started_at: u64,
    dragging: bool,
    moved: bool,
}

impl SelectedText {
    fn columns(&self, row: u16) -> Option<(u16, u16)> {
        let start = self.anchor.min(self.end);
        let end = self.anchor.max(self.end);
        if !self.moved || row < start.0 || row > end.0 {
            return None;
        }
        let area = self.frame.buffer.area;
        Some((
            if row == start.0 { start.1 } else { area.x },
            if row == end.0 {
                end.1
            } else {
                area.right() - 1
            },
        ))
    }

    fn text(&self) -> String {
        let area = self.frame.buffer.area;
        let mut text = String::new();
        for row in area.y..area.bottom() {
            let Some((start, end)) = self.columns(row) else {
                continue;
            };
            let mut line = String::new();
            visit_glyphs(&self.frame.buffer, row, |col, width, symbol| {
                if col <= end && col + width > start {
                    line.push_str(symbol);
                }
            });
            let wrapped = self.frame.wrapped[usize::from(row - area.y)];
            text.push_str(if wrapped {
                &line
            } else {
                line.trim_end_matches(' ')
            });
            if row < self.anchor.max(self.end).0 && !wrapped {
                text.push('\n');
            }
        }
        text
    }
}

fn agent(app: &App, reader_id: u64) -> Option<&AgentClient> {
    app.active_agent
        .iter()
        .chain(app.agent_aux.iter().map(|aux| &aux.agent))
        .find(|agent| agent.reader_id == reader_id)
}

fn valid(app: &App, selected: &SelectedText) -> bool {
    app.is_agent_view()
        && !mouse::input_blocked(app)
        && agent(app, selected.frame.reader_id).is_some_and(|agent| {
            agent.pending_resize.is_none()
                && !agent.pending_snapshot_output
                && !agent.snapshot_parse_in_progress
        })
        && app.mouse_wheel_regions.iter().any(|region| {
            region.area == selected.frame.buffer.area
                && selected.started_at > region.valid_since_epoch_ms
                && matches!(region.target, mouse::MouseWheelTarget::Agent { reader_id }
                    if reader_id == selected.frame.reader_id)
        })
}

pub(super) fn clear(app: &mut App) {
    app.text_selection.active = None;
    app.text_selection.copy_status = None;
}

pub(super) fn is_active(app: &App) -> bool {
    app.text_selection.active.is_some()
}

pub(super) fn begin_frame(app: &mut App) {
    app.text_selection.frames.clear();
}

pub(super) fn record_frame(app: &mut App, buffer: &Buffer, reader_id: u64, area: Rect) {
    if area.width == 0 || area.height == 0 || mouse::input_blocked(app) {
        return;
    }
    if let Some(selected) = app.text_selection.active.as_ref().filter(|selected| {
        selected.frame.reader_id == reader_id
            && selected.frame.buffer.area == area
            && valid(app, selected)
    }) {
        app.text_selection.frames.push(Arc::clone(&selected.frame));
        return;
    }
    let Some(agent) = agent(app, reader_id) else {
        return;
    };
    let screen = agent.parser.screen();
    let history = screen.scrollback() == 0 && agent.history_scroll_offset > 0;
    let wrapped = (0..area.height)
        .map(|row| !history && screen.row_wrapped(row))
        .collect();
    let mut snapshot = Buffer::empty(area);
    for row in area.y..area.bottom() {
        for col in area.x..area.right() {
            snapshot[(col, row)].clone_from(&buffer[(col, row)]);
        }
    }
    app.text_selection.frames.push(Arc::new(SelectionFrame {
        reader_id,
        buffer: snapshot,
        wrapped,
    }));
}

fn position(area: Rect, event: MouseEvent) -> (u16, u16) {
    (
        event.row.clamp(area.y, area.bottom() - 1),
        event.column.clamp(area.x, area.right() - 1),
    )
}

pub(super) fn handle_mouse(app: &mut App, event: MouseEvent, queued_at: u64) -> bool {
    if matches!(event.kind, MouseEventKind::Down(_)) {
        // A fresh click starts a new gesture, including clicks in other panes.
        clear(app);
        if event.kind != MouseEventKind::Down(MouseButton::Left)
            || mouse::input_blocked(app)
            || !app.is_agent_view()
        {
            return false;
        }
        let Some(frame) = app
            .text_selection
            .frames
            .iter()
            .find(|frame| {
                let area = frame.buffer.area;
                event.column >= area.x
                    && event.column < area.right()
                    && event.row >= area.y
                    && event.row < area.bottom()
            })
            .cloned()
        else {
            return false;
        };
        let Some(agent) = agent(app, frame.reader_id) else {
            return false;
        };
        // Alt reserves a local gesture even when the child requests mouse input.
        // A scrolled-back view also belongs to the parent, not the live child.
        if !event.modifiers.contains(KeyModifiers::ALT)
            && agent.scrollback_offset() == 0
            && agent.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
        {
            return false;
        }
        let anchor = position(frame.buffer.area, event);
        let selected = SelectedText {
            frame,
            anchor,
            end: anchor,
            started_at: queued_at,
            dragging: true,
            moved: false,
        };
        if !valid(app, &selected) {
            return true;
        }
        mouse::cancel_mouse_capture(app);
        app.agent_focus = if app
            .active_agent
            .as_ref()
            .is_some_and(|agent| agent.reader_id == selected.frame.reader_id)
        {
            AgentFocusPane::Main
        } else {
            AgentFocusPane::Auxiliary
        };
        app.text_selection.active = Some(selected);
        return true;
    }
    if matches!(
        event.kind,
        MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
    ) {
        clear(app);
        return false;
    }
    if !matches!(
        event.kind,
        MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
    ) {
        return false;
    }
    let Some(mut selected) = app.text_selection.active.take() else {
        return false;
    };
    if !valid(app, &selected) {
        clear(app);
        return true;
    }
    if selected.dragging && queued_at >= selected.started_at {
        selected.end = position(selected.frame.buffer.area, event);
        selected.moved |= selected.end != selected.anchor;
        selected.dragging = event.kind != MouseEventKind::Up(MouseButton::Left);
    }
    // A click without a drag only changes focus; it does not select a character.
    if selected.dragging || selected.moved {
        app.text_selection.active = Some(selected);
    }
    true
}

// Walk displayed glyphs, not UTF-8 bytes. Include a whole wide glyph if either
// half lies in the selection, and skip its continuation cell when copying.
fn visit_glyphs(buffer: &Buffer, row: u16, mut visit: impl FnMut(u16, u16, &str)) {
    let mut col = buffer.area.x;
    while col < buffer.area.right() {
        let symbol = buffer[(col, row)].symbol();
        let width = UnicodeWidthStr::width(symbol)
            .max(1)
            .min(usize::from(buffer.area.right() - col)) as u16;
        visit(col, width, symbol);
        col += width;
    }
}

pub(super) fn finish_frame(app: &mut App, buffer: &mut Buffer, footer: Option<Rect>) {
    if app
        .text_selection
        .active
        .as_ref()
        .is_some_and(|selected| !valid(app, selected))
    {
        clear(app);
    }
    if mouse::input_blocked(app) || !app.is_agent_view() {
        app.text_selection.frames.clear();
    }
    let Some(selected) = app.text_selection.active.as_ref() else {
        return;
    };
    let snapshot = &selected.frame.buffer;
    let area = snapshot.area;
    for row in area.y..area.bottom() {
        for col in area.x..area.right() {
            buffer[(col, row)].clone_from(&snapshot[(col, row)]);
        }
        if let Some((start, end)) = selected.columns(row) {
            visit_glyphs(snapshot, row, |col, width, _| {
                if col <= end && col + width > start {
                    for x in col..col + width {
                        buffer[(x, row)].set_style(
                            Style::default()
                                .fg(THEME_BG)
                                .bg(THEME_FG)
                                .remove_modifier(Modifier::REVERSED),
                        );
                    }
                }
            });
        }
    }
    if let Some(footer) = footer {
        let status = app
            .text_selection
            .copy_status
            .as_deref()
            .unwrap_or("Text selection · view paused · Enter copy · Esc clear");
        fill_area(buffer, footer, theme_status_style());
        buffer.set_stringn(
            footer.x,
            footer.y,
            status,
            footer.width as usize,
            theme_status_style(),
        );
    }
}

pub(super) fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(selected) = app.text_selection.active.as_ref() else {
        return false;
    };
    if !valid(app, selected) {
        clear(app);
        return false;
    }
    let copy = (key.code == KeyCode::Enter && key.modifiers.is_empty())
        || (matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers == (KeyModifiers::CONTROL | KeyModifiers::SHIFT));
    if copy && selected.moved {
        let text = selected.text();
        if text.is_empty() {
            app.text_selection.copy_status = Some("Selection is empty · Esc clear".into());
        } else {
            app.text_selection.clipboard_request = Some(clipboard_sequence(text.as_bytes()));
            app.text_selection.copy_status =
                Some("Copy requested · view paused · Esc clear".into());
        }
        return true;
    }
    clear(app);
    key.code == KeyCode::Esc && key.modifiers.is_empty()
}

// OSC 52 requests a copy on the user's terminal, including over SSH. Do not
// query/read the existing clipboard, spawn helpers, or log selected text.
// https://invisible-island.net/xterm/ctlseqs/ctlseqs.html (OSC 52)
fn clipboard_sequence(text: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::with_capacity(9 + text.len().div_ceil(3) * 4);
    output.extend_from_slice(b"\x1b]52;c;");
    for chunk in text.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[usize::from(a >> 2)]);
        output.push(ALPHABET[usize::from(((a & 3) << 4) | (b >> 4))]);
        output.push(if chunk.len() > 1 {
            ALPHABET[usize::from(((b & 15) << 2) | (c >> 6))]
        } else {
            b'='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[usize::from(c & 63)]
        } else {
            b'='
        });
    }
    output.extend_from_slice(b"\x1b\\");
    output
}

pub(super) fn flush_clipboard(app: &mut App, output: &mut impl Write) {
    if let Some(data) = app.text_selection.clipboard_request.take() {
        if let Err(error) = output.write_all(&data).and_then(|_| output.flush()) {
            app.text_selection.copy_status = Some(format!("Copy failed: {error} · Esc clear"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app_for_key_tests, buffered_output_test_client_with_requests};
    use super::*;

    const AREA: Rect = Rect {
        x: 10,
        y: 2,
        width: 20,
        height: 3,
    };

    struct Fixture {
        app: App,
        requests: Receiver<AgentWriterRequest>,
    }

    impl Fixture {
        fn new(native_mouse: bool) -> Self {
            let (mut client, requests) =
                buffered_output_test_client_with_requests("text-selection", 9950);
            client.parser = vt100::Parser::new(AREA.height, AREA.width, 100);
            client.parser.process(b"hello world");
            if native_mouse {
                client.parser.process(b"\x1b[?1002h\x1b[?1006h");
            }
            let mut app = app_for_key_tests();
            app.active_agent = Some(client);
            app.show_sessions_view = false;
            let mut fixture = Self { app, requests };
            fixture.draw();
            fixture
        }

        fn draw(&mut self) -> Buffer {
            self.app.begin_mouse_wheel_frame();
            let client = self.app.active_agent.as_ref().unwrap();
            let reader_id = client.reader_id;
            let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 8));
            if client.history_scroll_offset > 0 && client.parser.screen().scrollback() == 0 {
                let lines = client
                    .screen_history
                    .visible_lines(client.history_scroll_offset, 3);
                render_plain_agent_history(&mut buffer, AREA, &lines, true);
            } else {
                render_vt100_screen(&mut buffer, client.parser.screen(), AREA, true);
            }
            self.app
                .record_mouse_wheel_region(AREA, mouse::MouseWheelTarget::Agent { reader_id });
            record_frame(&mut self.app, &buffer, reader_id, AREA);
            finish_frame(&mut self.app, &mut buffer, Some(Rect::new(0, 7, 40, 1)));
            buffer
        }

        fn event(&mut self, kind: MouseEventKind, col: u16, row: u16, modifiers: KeyModifiers) {
            mouse::handle_mouse_input_event(
                &mut self.app,
                MouseEvent {
                    kind,
                    column: col,
                    row,
                    modifiers,
                },
                current_epoch_ms().saturating_add(1),
            );
        }

        fn select_hello(&mut self, modifiers: KeyModifiers) {
            self.event(
                MouseEventKind::Down(MouseButton::Left),
                AREA.x,
                AREA.y,
                modifiers,
            );
            // Releasing Alt during a gesture must not redirect it to the child.
            self.event(
                MouseEventKind::Drag(MouseButton::Left),
                AREA.x + 4,
                AREA.y,
                KeyModifiers::NONE,
            );
            self.event(
                MouseEventKind::Up(MouseButton::Left),
                AREA.x + 4,
                AREA.y,
                KeyModifiers::NONE,
            );
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for client in self
                .app
                .active_agent
                .iter_mut()
                .chain(self.app.agent_aux.iter_mut().map(|aux| &mut aux.agent))
            {
                client.exited = Some("test cleanup".into());
            }
        }
    }

    #[test]
    fn alt_selection_copies_displayed_text_without_sending_input_to_the_child() {
        let mut fixture = Fixture::new(true);
        fixture.select_hello(KeyModifiers::ALT);
        assert_eq!(
            fixture.app.text_selection.active.as_ref().unwrap().text(),
            "hello"
        );
        assert!(handle_key(
            &mut fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ));
        let mut output = Vec::new();
        flush_clipboard(&mut fixture.app, &mut output);
        assert_eq!(output, b"\x1b]52;c;aGVsbG8=\x1b\\");
        assert!(fixture.requests.try_recv().is_err());
        assert_eq!(
            fixture
                .app
                .active_agent
                .as_ref()
                .unwrap()
                .last_input_epoch_ms,
            0
        );
    }

    #[test]
    fn ordinary_native_drag_is_still_forwarded_and_plain_apps_select_locally() {
        let mut native = Fixture::new(true);
        native.select_hello(KeyModifiers::NONE);
        assert!(!is_active(&native.app));
        let bytes: Vec<_> = native
            .requests
            .try_iter()
            .map(|request| match request.request {
                AgentDaemonRequest::Input { data, .. } => data,
                _ => panic!("selection must not change the connection"),
            })
            .collect();
        assert_eq!(
            bytes,
            [
                b"\x1b[<0;1;1M".to_vec(),
                b"\x1b[<32;5;1M".to_vec(),
                b"\x1b[<0;5;1m".to_vec()
            ]
        );

        let mut plain = Fixture::new(false);
        plain.select_hello(KeyModifiers::NONE);
        assert_eq!(
            plain.app.text_selection.active.as_ref().unwrap().text(),
            "hello"
        );
        assert!(plain.requests.try_recv().is_err());
    }

    #[test]
    fn output_keeps_arriving_while_the_selected_view_is_frozen() {
        let mut fixture = Fixture::new(false);
        fixture.select_hello(KeyModifiers::NONE);
        fixture
            .app
            .active_agent
            .as_mut()
            .unwrap()
            .parser
            .process(b"\x1b[Hworld");
        let buffer = fixture.draw();
        assert_eq!(buffer[(AREA.x, AREA.y)].symbol(), "h");
        assert_eq!(buffer[(AREA.x, AREA.y)].bg, THEME_FG);
        assert_eq!(
            fixture.app.text_selection.active.as_ref().unwrap().text(),
            "hello"
        );
        assert!(handle_key(
            &mut fixture.app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        ));
        assert_eq!(fixture.draw()[(AREA.x, AREA.y)].symbol(), "w");
        assert!(fixture.requests.try_recv().is_err());
    }

    #[test]
    fn reverse_selection_includes_whole_wide_glyphs_and_preserves_line_breaks() {
        let mut buffer = Buffer::empty(Rect::new(5, 3, 8, 2));
        buffer.set_string(5, 3, "가나A", Style::default());
        buffer.set_string(5, 4, "e\u{301}nd", Style::default());
        let mut selected = SelectedText {
            frame: Arc::new(SelectionFrame {
                reader_id: 1,
                buffer,
                wrapped: vec![false, false],
            }),
            anchor: (4, 7),
            end: (3, 6),
            started_at: 1,
            dragging: false,
            moved: true,
        };
        assert_eq!(selected.text(), "가나A\ne\u{301}nd");
        selected.anchor = (3, 8);
        assert_eq!(selected.text(), "가나");
    }

    #[test]
    fn soft_wrapped_rows_are_joined_when_copying() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 2));
        buffer.set_string(0, 0, "hello", Style::default());
        buffer.set_string(0, 1, "world", Style::default());
        let selected = SelectedText {
            frame: Arc::new(SelectionFrame {
                reader_id: 1,
                buffer,
                wrapped: vec![true, false],
            }),
            anchor: (0, 0),
            end: (1, 4),
            started_at: 1,
            dragging: false,
            moved: true,
        };
        assert_eq!(selected.text(), "helloworld");
    }

    #[test]
    fn selection_uses_the_displayed_history_and_stays_inside_its_pane() {
        let mut fixture = Fixture::new(true);
        let client = fixture.app.active_agent.as_mut().unwrap();
        client.screen_history.capture_lines(vec![
            "old line".into(),
            "second".into(),
            "third".into(),
            "live".into(),
        ]);
        client.history_scroll_offset = 1;
        fixture.draw();
        fixture.event(
            MouseEventKind::Down(MouseButton::Left),
            AREA.x,
            AREA.y,
            KeyModifiers::NONE,
        );
        fixture.event(
            MouseEventKind::Up(MouseButton::Left),
            AREA.right() + 20,
            AREA.y,
            KeyModifiers::NONE,
        );
        let selected = fixture.app.text_selection.active.as_ref().unwrap();
        assert_eq!(selected.end, (AREA.y, AREA.right() - 1));
        assert_eq!(selected.text(), "old line");
        assert!(fixture.requests.try_recv().is_err());
    }

    #[test]
    fn stale_gestures_and_replaced_connections_do_not_select_or_receive_input() {
        let mut fixture = Fixture::new(true);
        mouse::handle_mouse_input_event(
            &mut fixture.app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: AREA.x,
                row: AREA.y,
                modifiers: KeyModifiers::ALT,
            },
            0,
        );
        assert!(!is_active(&fixture.app));
        fixture.select_hello(KeyModifiers::ALT);
        fixture.app.active_agent.as_mut().unwrap().reader_id += 1;
        fixture.event(
            MouseEventKind::Drag(MouseButton::Left),
            AREA.x + 10,
            AREA.y,
            KeyModifiers::ALT,
        );
        assert!(!is_active(&fixture.app));
        fixture.event(
            MouseEventKind::Up(MouseButton::Left),
            AREA.x + 10,
            AREA.y,
            KeyModifiers::ALT,
        );
        assert!(fixture.requests.try_recv().is_err());
    }

    #[test]
    fn auxiliary_selection_follows_the_pointer_and_never_reaches_the_main_agent() {
        let mut fixture = Fixture::new(true);
        let (mut auxiliary, requests) =
            buffered_output_test_client_with_requests("aux-selection", 9951);
        auxiliary.parser = vt100::Parser::new(3, 20, 100);
        auxiliary.parser.process(b"auxiliary\x1b[?1002h\x1b[?1006h");
        fixture.app.agent_aux = Some(AgentAuxPane {
            kind: AgentAuxKind::Terminal,
            parent: AgentKey::new(&fixture.app.active_agent.as_ref().unwrap().info),
            agent: auxiliary,
        });
        let area = Rect::new(31, 2, 20, 3);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 8));
        render_vt100_screen(
            &mut buffer,
            fixture
                .app
                .agent_aux
                .as_ref()
                .unwrap()
                .agent
                .parser
                .screen(),
            area,
            false,
        );
        fixture
            .app
            .record_mouse_wheel_region(area, mouse::MouseWheelTarget::Agent { reader_id: 9951 });
        record_frame(&mut fixture.app, &buffer, 9951, area);
        fixture.app.agent_focus = AgentFocusPane::Main;
        fixture.event(
            MouseEventKind::Down(MouseButton::Left),
            39,
            2,
            KeyModifiers::ALT,
        );
        // Drag left across the divider into the main pane.
        fixture.event(
            MouseEventKind::Up(MouseButton::Left),
            10,
            2,
            KeyModifiers::NONE,
        );
        assert_eq!(fixture.app.agent_focus, AgentFocusPane::Auxiliary);
        assert_eq!(
            fixture.app.text_selection.active.as_ref().unwrap().text(),
            "auxiliary"
        );
        assert!(fixture.requests.try_recv().is_err());
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn modal_resize_and_session_view_cancel_selection() {
        for change in 0..3 {
            let mut fixture = Fixture::new(false);
            fixture.select_hello(KeyModifiers::NONE);
            match change {
                0 => {
                    fixture.app.input_mode = InputMode::Notice {
                        title: "notice".into(),
                        message: "modal".into(),
                    }
                }
                1 => fixture.app.active_agent.as_mut().unwrap().pending_resize = Some((10, 40)),
                _ => fixture.app.show_sessions_view = true,
            }
            fixture.draw();
            assert!(!is_active(&fixture.app));
            assert!(fixture.requests.try_recv().is_err());
        }
    }

    #[test]
    fn ordinary_typing_and_wheels_clear_selection_without_consuming_the_action() {
        let mut fixture = Fixture::new(false);
        fixture.select_hello(KeyModifiers::NONE);
        assert!(!handle_key(
            &mut fixture.app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)
        ));
        assert!(!is_active(&fixture.app));
        fixture.select_hello(KeyModifiers::NONE);
        assert!(!handle_mouse(
            &mut fixture.app,
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: AREA.x,
                row: AREA.y,
                modifiers: KeyModifiers::NONE,
            },
            current_epoch_ms().saturating_add(1)
        ));
        assert!(!is_active(&fixture.app));
    }

    #[test]
    fn clipboard_encoding_handles_utf8_and_each_padding_length() {
        for (text, encoded) in [
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("가", "6rCA"),
        ] {
            assert_eq!(
                clipboard_sequence(text.as_bytes()),
                format!("\x1b]52;c;{encoded}\x1b\\").into_bytes()
            );
        }
    }
}
