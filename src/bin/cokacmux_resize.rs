//! Drag only rendered internal borders. A gesture owns its original layout;
//! queued events cannot resize a replacement workspace or reach a child PTY.
use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Sessions,
    Sidebar,
    Auxiliary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LayoutKey {
    area: Rect,
    agent_view: bool,
    main_reader: Option<u64>,
    aux_reader: Option<u64>,
    sessions_width: Option<u16>,
    sessions_percent: u16,
    sidebar_width: u16,
    sidebar_visible: bool,
    aux_width: Option<u16>,
}

impl LayoutKey {
    fn new(app: &App, area: Rect) -> Self {
        Self {
            area,
            agent_view: app.is_agent_view(),
            main_reader: app.active_agent.as_ref().map(|a| a.reader_id),
            aux_reader: app.agent_aux.as_ref().map(|a| a.agent.reader_id),
            sessions_width: app.settings.cokacmux.sessions_pane_width,
            sessions_percent: app.settings.cokacmux.sessions_pane_percent,
            sidebar_width: app.settings.cokacmux.agent_sidebar_width,
            sidebar_visible: app.settings.cokacmux.agent_sidebar_visible,
            aux_width: app.agent_aux_width,
        }
    }
}

#[derive(Debug, Clone)]
struct Border {
    area: Rect,
    target: Target,
    width: u16,
}

#[derive(Debug, Clone)]
struct Frame {
    key: LayoutKey,
    borders: Vec<Border>,
    valid_since: u64,
}

#[derive(Debug)]
struct Capture {
    key: LayoutKey,
    target: Target,
    pressed_at: u64,
    column: u16,
    width: u16,
    changed: bool,
}

#[derive(Default)]
pub(super) struct PaneResize {
    frame: Option<Frame>,
    previous: Option<Frame>,
    capture: Option<Capture>,
}

pub(super) fn begin_frame(app: &mut App) {
    app.pane_resize.previous = app.pane_resize.frame.take();
}

fn record(app: &mut App, area: Rect, borders: Vec<Border>) {
    let key = LayoutKey::new(app, area);
    let valid_since = app
        .pane_resize
        .previous
        .as_ref()
        .filter(|frame| frame.key == key)
        .map_or_else(current_epoch_ms, |frame| frame.valid_since);
    app.pane_resize.frame = Some(Frame {
        key,
        borders,
        valid_since,
    });
}

pub(super) fn record_sessions(app: &mut App, area: Rect, left: Rect, right: Rect) {
    let mut borders = Vec::new();
    if left.width > 0 && right.width > 0 && left.height > 0 {
        // Both neighboring panes draw a border column at this divider.
        borders.push(Border {
            area: Rect::new(left.right() - 1, left.y, 2, left.height),
            target: Target::Sessions,
            width: left.width,
        });
    }
    record(app, area, borders);
}

pub(super) fn record_agents(app: &mut App, area: Rect, layout: AgentPaneLayout) {
    let mut borders = Vec::new();
    if let Some(sidebar) = layout.sidebar {
        if sidebar.width > 0 && layout.main.width > 0 && sidebar.height > 0 {
            borders.push(Border {
                area: Rect::new(sidebar.right() - 1, sidebar.y, 1, sidebar.height),
                target: Target::Sidebar,
                width: sidebar.width,
            });
        }
    }
    if let Some(aux) = layout.auxiliary {
        if aux.width > 0 && layout.main.width > 0 && aux.height > 0 {
            borders.push(Border {
                area: Rect::new(aux.x, aux.y, 1, aux.height),
                target: Target::Auxiliary,
                width: aux.width,
            });
        }
    }
    record(app, area, borders);
}

fn current(app: &App, key: &LayoutKey) -> bool {
    !mouse::input_blocked(app) && LayoutKey::new(app, key.area) == *key
}

/// Commit the width already shown, including when focus/geometry interrupts a
/// drag. Invalidate its hit targets until a fresh frame has been rendered.
pub(super) fn cancel(app: &mut App) {
    if let Some(capture) = app.pane_resize.capture.take() {
        if capture.changed {
            match app.save_settings() {
                Ok(()) => app.status = format!("layout {}.", app.settings_save_word()),
                Err(error) => app.status = format!("layout changed, save failed: {error}"),
            }
        }
    }
    app.pane_resize.frame = None;
    app.pane_resize.previous = None;
}

pub(super) fn finish_frame(app: &mut App) {
    if app.pane_resize.capture.as_ref().is_some_and(|capture| {
        !current(app, &capture.key)
            || app
                .pane_resize
                .frame
                .as_ref()
                .is_none_or(|frame| frame.key != capture.key)
    }) {
        cancel(app);
    }
    if mouse::input_blocked(app) {
        app.pane_resize.frame = None;
    }
}

fn apply(app: &mut App, capture: &mut Capture, column: u16) {
    let delta = i32::from(column) - i32::from(capture.column);
    let total = capture.key.area.width;
    let (next, old) = match capture.target {
        Target::Sessions => {
            let next = (i32::from(capture.width) + delta)
                .clamp(0, i32::from(max_sessions_pane_width(total))) as u16;
            (next, app.sessions_pane_width(total))
        }
        Target::Sidebar => {
            let next = (i32::from(capture.width) + delta).clamp(0, i32::from(total)) as u16;
            (
                next,
                agent_sidebar_width(total, app.agent_sidebar_config_width()),
            )
        }
        Target::Auxiliary => {
            let content =
                total.saturating_sub(agent_sidebar_width(total, app.agent_sidebar_config_width()));
            let (min, max) = agent_auxiliary_width_bounds(content);
            let next =
                (i32::from(capture.width) - delta).clamp(i32::from(min), i32::from(max)) as u16;
            (next, agent_auxiliary_width(content, app.agent_aux_width))
        }
    };
    if next == old {
        return;
    }
    match capture.target {
        Target::Sessions => {
            app.settings.cokacmux.sessions_pane_width = Some(next);
            app.clear_preview_cache();
        }
        Target::Sidebar => app.settings.cokacmux.agent_sidebar_width = next,
        Target::Auxiliary => {
            app.agent_aux_width = Some(next);
            app.settings.cokacmux.agent_aux_width = Some(next);
        }
    }
    // Rendering resizes each visible PTY once per frame, coalescing mouse bursts.
    app.status = format!(
        "layout: {} {} cols",
        match capture.target {
            Target::Sessions => "sessions",
            Target::Sidebar => "agent sidebar",
            Target::Auxiliary => "right panel",
        },
        next
    );
    capture.changed = true;
    capture.key = LayoutKey::new(app, capture.key.area);
}

pub(super) fn handle_mouse(app: &mut App, event: MouseEvent, queued_at: u64) -> bool {
    if let Some(mut capture) = app.pane_resize.capture.take() {
        if queued_at < capture.pressed_at {
            app.pane_resize.capture = Some(capture);
            return true;
        }
        if !current(app, &capture.key) {
            app.pane_resize.capture = Some(capture);
            cancel(app);
            return true;
        }
        let is_release = event.kind == MouseEventKind::Up(MouseButton::Left);
        if matches!(
            event.kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) && queued_at >= capture.pressed_at
        {
            apply(app, &mut capture, event.column);
            app.pane_resize.capture = Some(capture);
            if is_release {
                cancel(app);
            }
            return true;
        }
        app.pane_resize.capture = Some(capture);
        // A fresh press ends a lost-release gesture; other buttons/wheel events
        // remain captured and never leak into the child application.
        if !matches!(event.kind, MouseEventKind::Down(_)) {
            return true;
        }
        let frame = app.pane_resize.frame.clone();
        cancel(app);
        app.pane_resize.frame = frame.filter(|frame| current(app, &frame.key));
    }
    if event.kind != MouseEventKind::Down(MouseButton::Left) || !event.modifiers.is_empty() {
        return false;
    }
    let Some(frame) = app.pane_resize.frame.as_ref() else {
        return false;
    };
    if queued_at <= frame.valid_since || !current(app, &frame.key) {
        return false;
    }
    let Some(border) = frame.borders.iter().find(|border| {
        event.column >= border.area.x
            && event.column < border.area.right()
            && event.row >= border.area.y
            && event.row < border.area.bottom()
    }) else {
        return false;
    };
    let capture = Capture {
        key: frame.key.clone(),
        target: border.target,
        pressed_at: queued_at,
        column: event.column,
        width: border.width,
        changed: false,
    };
    selection::clear(app);
    mouse::cancel_mouse_capture(app);
    app.pane_resize.capture = Some(capture);
    true
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app_for_key_tests, buffered_output_test_client_with_requests};
    use super::*;
    use ratatui::backend::TestBackend;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 200,
        height: 24,
    };

    fn draw(app: &mut App) {
        let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
        terminal
            .draw(|frame| {
                if app.is_agent_view() {
                    ui_agent(frame, app);
                } else {
                    ui(frame, app);
                }
            })
            .unwrap();
    }

    fn event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn dispatch(app: &mut App, kind: MouseEventKind, column: u16) {
        mouse::handle_mouse_input_event(app, event(kind, column, 4), u64::MAX - 1);
    }

    fn border(app: &App, target: Target) -> Border {
        app.pane_resize
            .frame
            .as_ref()
            .unwrap()
            .borders
            .iter()
            .find(|border| border.target == target)
            .unwrap()
            .clone()
    }

    struct Agents {
        app: App,
        main_requests: Receiver<AgentWriterRequest>,
        aux_requests: Receiver<AgentWriterRequest>,
    }

    impl Drop for Agents {
        fn drop(&mut self) {
            if let Some(agent) = self.app.active_agent.as_mut() {
                agent.exited = Some("test cleanup".into());
            }
            if let Some(aux) = self.app.agent_aux.as_mut() {
                aux.agent.exited = Some("test cleanup".into());
            }
        }
    }

    fn agents() -> Agents {
        let mut app = app_for_key_tests();
        let (mut main, main_requests) =
            buffered_output_test_client_with_requests("resize-main", 9970);
        let (mut auxiliary, aux_requests) =
            buffered_output_test_client_with_requests("resize-aux", 9971);
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
        app.settings.cokacmux.agent_sidebar_width = 35;
        app.agent_aux_width = Some(60);
        app.settings.cokacmux.agent_aux_width = Some(60);
        app.agent_focus = AgentFocusPane::Main;
        Agents {
            app,
            main_requests,
            aux_requests,
        }
    }

    #[test]
    fn either_sessions_border_drags_without_a_jump_and_survives_redraw() {
        for offset in [0, 1] {
            let mut app = app_for_key_tests();
            draw(&mut app);
            let border = border(&app, Target::Sessions);
            let column = border.area.x + offset;
            let focus = app.focus;
            dispatch(&mut app, MouseEventKind::Down(MouseButton::Left), column);
            assert!(app.pane_resize.capture.is_some());
            dispatch(
                &mut app,
                MouseEventKind::Drag(MouseButton::Left),
                column + 11,
            );
            assert_eq!(app.sessions_pane_width(AREA.width), border.width + 11);
            assert!(
                app.settings_writer.is_none(),
                "no filesystem work during a drag"
            );
            draw(&mut app);
            assert!(app.pane_resize.capture.is_some());
            dispatch(&mut app, MouseEventKind::Up(MouseButton::Left), column + 15);
            assert_eq!(
                app.settings.cokacmux.sessions_pane_width,
                Some(border.width + 15)
            );
            assert!(app.pane_resize.capture.is_none());
            assert_eq!(app.focus, focus);
            assert_eq!(app.status, "layout saved.");
        }
    }

    #[test]
    fn sessions_drag_clamps_and_can_restore_a_collapsed_pane_before_release() {
        let mut app = app_for_key_tests();
        draw(&mut app);
        let border = border(&app, Target::Sessions);
        let column = border.area.x + 1;
        dispatch(&mut app, MouseEventKind::Down(MouseButton::Left), column);
        dispatch(&mut app, MouseEventKind::Drag(MouseButton::Left), u16::MAX);
        assert_eq!(
            app.sessions_pane_width(AREA.width),
            max_sessions_pane_width(AREA.width)
        );
        dispatch(&mut app, MouseEventKind::Drag(MouseButton::Left), 0);
        assert_eq!(app.sessions_pane_width(AREA.width), 0);
        draw(&mut app);
        assert!(app.pane_resize.capture.is_some());
        dispatch(&mut app, MouseEventKind::Up(MouseButton::Left), column);
        assert_eq!(app.sessions_pane_width(AREA.width), border.width);
    }

    #[test]
    fn click_without_motion_preserves_proportional_width_and_does_not_save() {
        let mut app = app_for_key_tests();
        app.settings.cokacmux.sessions_pane_width = None;
        draw(&mut app);
        let x = border(&app, Target::Sessions).area.x;
        dispatch(&mut app, MouseEventKind::Down(MouseButton::Left), x);
        dispatch(&mut app, MouseEventKind::Up(MouseButton::Left), x);
        assert_eq!(app.settings.cokacmux.sessions_pane_width, None);
        assert!(app.settings_writer.is_none());
    }

    #[test]
    fn both_agent_borders_resize_independently_without_focus_or_child_input() {
        let mut fixture = agents();
        let app = &mut fixture.app;
        draw(app);
        let sidebar = border(app, Target::Sidebar);
        dispatch(app, MouseEventKind::Down(MouseButton::Left), sidebar.area.x);
        dispatch(
            app,
            MouseEventKind::Drag(MouseButton::Left),
            sidebar.area.x + 7,
        );
        draw(app);
        dispatch(
            app,
            MouseEventKind::Up(MouseButton::Left),
            sidebar.area.x + 7,
        );
        assert_eq!(app.settings.cokacmux.agent_sidebar_width, 42);
        assert_eq!(app.agent_aux_width, Some(60));
        draw(app);
        let auxiliary = border(app, Target::Auxiliary);
        dispatch(
            app,
            MouseEventKind::Down(MouseButton::Left),
            auxiliary.area.x,
        );
        dispatch(
            app,
            MouseEventKind::Drag(MouseButton::Left),
            auxiliary.area.x - 9,
        );
        draw(app);
        dispatch(
            app,
            MouseEventKind::Up(MouseButton::Left),
            auxiliary.area.x - 9,
        );
        assert_eq!(app.agent_aux_width, Some(69));
        assert_eq!(app.settings.cokacmux.agent_aux_width, Some(69));
        assert_eq!(app.settings.cokacmux.agent_sidebar_width, 42);
        assert_eq!(app.agent_focus, AgentFocusPane::Main);
        assert!(app.mouse_button_capture.is_none());
        let main: Vec<_> = fixture
            .main_requests
            .try_iter()
            .map(|request| request.request)
            .collect();
        let aux: Vec<_> = fixture
            .aux_requests
            .try_iter()
            .map(|request| request.request)
            .collect();
        assert!(main
            .iter()
            .any(|request| matches!(request, AgentDaemonRequest::Resize { .. })));
        assert!(aux
            .iter()
            .any(|request| matches!(request, AgentDaemonRequest::Resize { .. })));
        assert!(main
            .iter()
            .chain(&aux)
            .all(|request| !matches!(request, AgentDaemonRequest::Input { .. })));
    }

    #[test]
    fn sidebar_drag_can_return_from_full_width_and_auxiliary_respects_bounds() {
        let mut fixture = agents();
        let app = &mut fixture.app;
        draw(app);
        let x = border(app, Target::Sidebar).area.x;
        dispatch(app, MouseEventKind::Down(MouseButton::Left), x);
        dispatch(app, MouseEventKind::Drag(MouseButton::Left), AREA.width - 1);
        assert_eq!(app.agent_sidebar_config_width(), AREA.width);
        draw(app);
        assert!(app.pane_resize.capture.is_some());
        dispatch(app, MouseEventKind::Up(MouseButton::Left), x);
        assert_eq!(app.agent_sidebar_config_width(), 35);
        draw(app);
        let x = border(app, Target::Auxiliary).area.x;
        let (min, max) = agent_auxiliary_width_bounds(AREA.width - 35);
        dispatch(app, MouseEventKind::Down(MouseButton::Left), x);
        dispatch(app, MouseEventKind::Drag(MouseButton::Left), 0);
        assert_eq!(app.agent_aux_width, Some(max));
        dispatch(app, MouseEventKind::Up(MouseButton::Left), u16::MAX);
        assert_eq!(app.agent_aux_width, Some(min));
    }

    #[test]
    fn stale_or_modified_presses_and_non_border_cells_never_begin_resize() {
        let mut app = app_for_key_tests();
        draw(&mut app);
        let frame = app.pane_resize.frame.as_ref().unwrap();
        let x = border(&app, Target::Sessions).area.x;
        let stale = frame.valid_since;
        assert!(!handle_mouse(
            &mut app,
            event(MouseEventKind::Down(MouseButton::Left), x, 4),
            stale
        ));
        for (button, modifiers, column, row) in [
            (MouseButton::Right, KeyModifiers::NONE, x, 4),
            (MouseButton::Left, KeyModifiers::SHIFT, x, 4),
            (MouseButton::Left, KeyModifiers::NONE, x - 1, 4),
            (MouseButton::Left, KeyModifiers::NONE, x, AREA.height - 1),
            (MouseButton::Left, KeyModifiers::NONE, 0, 4),
        ] {
            let mouse = MouseEvent {
                kind: MouseEventKind::Down(button),
                modifiers,
                column,
                row,
            };
            assert!(!handle_mouse(&mut app, mouse, u64::MAX - 1));
            assert!(app.pane_resize.capture.is_none());
        }
        app.input_mode = InputMode::Notice {
            title: "test".into(),
            message: "test".into(),
        };
        assert!(!handle_mouse(
            &mut app,
            event(MouseEventKind::Down(MouseButton::Left), x, 4),
            u64::MAX - 1
        ));
    }

    #[test]
    fn layout_changes_reader_replacement_and_focus_loss_cancel_a_drag() {
        for change in 0..6 {
            let mut fixture = agents();
            let app = &mut fixture.app;
            draw(app);
            let x = border(app, Target::Sidebar).area.x;
            dispatch(app, MouseEventKind::Down(MouseButton::Left), x);
            match change {
                0 => app.active_agent.as_mut().unwrap().reader_id += 1,
                1 => app.settings.cokacmux.agent_sidebar_visible = false,
                2 => app.show_sessions_view = true,
                3 | 4 => {
                    let mut previous = true;
                    handle_main_event(
                        app,
                        MainEvent::Input {
                            event: if change == 3 {
                                Event::FocusLost
                            } else {
                                Event::Resize(150, 30)
                            },
                            queued_at_epoch_ms: u64::MAX - 1,
                        },
                        &mut previous,
                        "resize_test",
                    );
                }
                _ => {
                    app.input_mode = InputMode::Notice {
                        title: "test".into(),
                        message: "test".into(),
                    }
                }
            }
            dispatch(app, MouseEventKind::Drag(MouseButton::Left), x + 20);
            dispatch(app, MouseEventKind::Up(MouseButton::Left), x + 20);
            assert_eq!(app.settings.cokacmux.agent_sidebar_width, 35);
            assert!(app.pane_resize.capture.is_none());
        }
    }

    #[test]
    fn stale_release_is_ignored_and_a_new_press_recovers_a_lost_release() {
        let mut app = app_for_key_tests();
        draw(&mut app);
        let x = border(&app, Target::Sessions).area.x;
        dispatch(&mut app, MouseEventKind::Down(MouseButton::Left), x);
        dispatch(&mut app, MouseEventKind::Drag(MouseButton::Left), x + 10);
        let width = app.sessions_pane_width(AREA.width);
        assert!(handle_mouse(
            &mut app,
            event(MouseEventKind::Up(MouseButton::Left), x, 4),
            1
        ));
        assert!(app.pane_resize.capture.is_some());
        assert_eq!(app.sessions_pane_width(AREA.width), width);
        draw(&mut app);
        let x = border(&app, Target::Sessions).area.x;
        dispatch(&mut app, MouseEventKind::Down(MouseButton::Left), x);
        assert!(app.pane_resize.capture.is_some());
        dispatch(&mut app, MouseEventKind::Up(MouseButton::Left), x + 5);
        assert_eq!(app.sessions_pane_width(AREA.width), width + 5);
    }
}
