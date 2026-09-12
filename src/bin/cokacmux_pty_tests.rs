//! PTY correctness regressions. These are part of the explicit, isolated Rust
//! test gate; none require a provider CLI or a user's running session.

use super::*;

fn checkpoint_roundtrip(parser: &vt100::Parser) -> vt100::Parser {
    let wire = serde_json::to_vec(&parser.checkpoint(true)).unwrap();
    let checkpoint = serde_json::from_slice(&wire).unwrap();
    vt100::Parser::from_checkpoint(checkpoint).unwrap()
}

#[test]
fn checkpoint_preserves_continuation_at_every_byte_boundary() {
    let streams = [
        "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\n",
        "\x1b[31mNORMAL\x1b7\x1b[?1049hALT\x1b[2;4r\x1b[?6h\x1b[2;3Hnext\nline\x1b[?1049l\x1b8tail",
        "한글🙂e\u{301}\x1b[38;2;1;2;3m끝",
        "A\x1b[3\n1mRED\x1b[?2004h\x1b[?1h\x1b[?2004l",
        "\x1b]2;partial title\x07text\x1bP1;2qignored\x1b\\done",
    ];
    for stream in streams {
        let bytes = stream.as_bytes();
        for split in 0..=bytes.len() {
            let mut daemon = vt100::Parser::new(5, 20, AGENT_SCROLLBACK_LINES);
            daemon.process(&bytes[..split]);
            let mut client = checkpoint_roundtrip(&daemon);
            daemon.process(&bytes[split..]);
            client.process(&bytes[split..]);
            assert_eq!(
                serde_json::to_value(client.checkpoint(true)).unwrap(),
                serde_json::to_value(daemon.checkpoint(true)).unwrap(),
                "checkpoint continuation differs at byte {split} of {stream:?}",
            );
        }
    }
}

#[test]
fn checkpoint_preserves_hidden_grid_and_saved_cursor_after_resize() {
    let mut daemon = vt100::Parser::new(5, 20, 100);
    daemon.process(b"\x1b[3;7HNORMAL\x1b[?1049h\x1b[2;4rALT");
    daemon.screen_mut().set_size(6, 30);
    let mut client = checkpoint_roundtrip(&daemon);
    assert!(client.screen().alternate_screen());
    let continuation = b"\x1b[?1049lX\x1b8Y";
    daemon.process(continuation);
    client.process(continuation);
    assert_eq!(
        serde_json::to_value(client.checkpoint(true)).unwrap(),
        serde_json::to_value(daemon.checkpoint(true)).unwrap(),
    );
    assert!(client.screen().contents().contains("NORMAL"));
}

#[test]
fn checkpoint_rejects_invalid_internal_indices_before_use() {
    let parser = vt100::Parser::new(5, 20, 100);
    for path in ["intermediate_idx", "partial_utf8_len"] {
        let mut wire = serde_json::to_value(parser.checkpoint(true)).unwrap();
        wire["parser"][path] = serde_json::json!(999);
        let checkpoint = serde_json::from_value(wire).unwrap();
        assert!(vt100::Parser::from_checkpoint(checkpoint).is_err());
    }
    let mut wire = serde_json::to_value(parser.checkpoint(true)).unwrap();
    wire["parser"]["params"]["len"] = serde_json::json!(999);
    let checkpoint = serde_json::from_value(wire).unwrap();
    assert!(vt100::Parser::from_checkpoint(checkpoint).is_err());
}

#[test]
fn ansi_snapshot_preserves_every_scrollback_row_including_short_history() {
    for count in 6..=20 {
        let mut daemon = vt100::Parser::new(5, 20, 100);
        for line in 1..=count {
            daemon.process(format!("LINE{line:03}\r\n").as_bytes());
        }
        let expected_history = parser_scrollback_plain_lines(&mut daemon);
        let expected_screen = daemon.screen().contents_formatted();
        let snapshot = parser_snapshot_bytes(&mut daemon, true);
        let mut client = vt100::Parser::new(5, 20, 100);
        client.process(&snapshot);
        assert_eq!(parser_scrollback_plain_lines(&mut client), expected_history);
        assert_eq!(client.screen().contents_formatted(), expected_screen);
    }
}

#[test]
fn device_reports_preserve_query_count_order_and_parse_position() {
    let bytes = b"A\x1b[6nB\x1b[6n\x1b[5n\x1b[?6n";
    let expected = b"\x1b[1;2R\x1b[1;3R\x1b[0n\x1b[?1;3R";
    for chunk_size in 1..=bytes.len() {
        let mut parser = vt100::Parser::new(5, 20, 0);
        parser.set_collect_responses(true);
        let mut replies = Vec::new();
        for chunk in bytes.chunks(chunk_size) {
            parser.process(chunk);
            replies.extend(parser.take_responses());
        }
        assert_eq!(replies, expected, "read chunk size {chunk_size}");
    }
}

#[test]
fn cursor_reports_respect_origin_mode() {
    let mut parser = vt100::Parser::new(5, 20, 0);
    parser.set_collect_responses(true);
    parser.process(b"\x1b[2;4r\x1b[?6h\x1b[2;3H\x1b[6n");
    assert_eq!(parser.take_responses(), b"\x1b[2;3R");
}

#[test]
fn keys_preserve_backtab_modifiers_and_application_cursor_mode() {
    let cases: &[(KeyCode, KeyModifiers, bool, &[u8])] = &[
        (KeyCode::BackTab, KeyModifiers::SHIFT, false, b"\x1b[Z"),
        (KeyCode::Tab, KeyModifiers::SHIFT, false, b"\x1b[Z"),
        (KeyCode::Char(' '), KeyModifiers::CONTROL, false, b"\0"),
        (KeyCode::Char('@'), KeyModifiers::CONTROL, false, b"\0"),
        (KeyCode::Up, KeyModifiers::NONE, false, b"\x1b[A"),
        (KeyCode::Up, KeyModifiers::NONE, true, b"\x1bOA"),
        (KeyCode::Home, KeyModifiers::NONE, true, b"\x1bOH"),
        (KeyCode::Up, KeyModifiers::CONTROL, true, b"\x1b[1;5A"),
        (KeyCode::F(1), KeyModifiers::NONE, false, b"\x1bOP"),
        (KeyCode::F(1), KeyModifiers::SHIFT, false, b"\x1b[1;2P"),
        (KeyCode::F(5), KeyModifiers::CONTROL, false, b"\x1b[15;5~"),
        (KeyCode::F(12), KeyModifiers::ALT, false, b"\x1b[24;3~"),
    ];
    for &(code, modifiers, application, expected) in cases {
        assert_eq!(
            key_event_to_bytes_with_mode(KeyEvent::new(code, modifiers), application).unwrap(),
            expected,
        );
    }
}

#[test]
fn reader_retries_interrupted_but_reports_permanent_errors() {
    struct Reads(VecDeque<io::Result<Vec<u8>>>);
    impl Read for Reads {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let bytes = self.0.pop_front().unwrap()?;
            buf[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }
    let mut reader = Reads(VecDeque::from([
        Err(io::Error::from(ErrorKind::Interrupted)),
        Err(io::Error::from(ErrorKind::Interrupted)),
        Ok(b"ok".to_vec()),
        Err(io::Error::from(ErrorKind::BrokenPipe)),
        Ok(Vec::new()),
    ]));
    let mut buf = [0; 8];
    assert_eq!(
        read_pty_retry_interrupted(&mut reader, &mut buf).unwrap(),
        2
    );
    assert_eq!(&buf[..2], b"ok");
    assert_eq!(
        read_pty_retry_interrupted(&mut reader, &mut buf)
            .unwrap_err()
            .kind(),
        ErrorKind::BrokenPipe
    );
    assert_eq!(
        read_pty_retry_interrupted(&mut reader, &mut buf).unwrap(),
        0
    );
}

#[test]
fn resize_failure_preserves_applied_size_and_allows_same_size_retry() {
    let mut parser = vt100::Parser::new(24, 80, 100);
    parser.process(b"\x1b[24;60HKEEP");
    let expected = parser.screen().contents_formatted();
    let mut applied = agent_pty_size(80, 24);
    let requested = agent_pty_size(40, 6);
    let result = resize_pty_state(&mut parser, &mut applied, requested, |_| {
        Err(io::Error::other("injected resize failure"))
    });
    assert!(result.is_err());
    assert_eq!((applied.rows, applied.cols), (24, 80));
    assert_eq!(parser.screen().contents_formatted(), expected);
    resize_pty_state(&mut parser, &mut applied, requested, |_| Ok(())).unwrap();
    assert_eq!((applied.rows, applied.cols), (6, 40));
}

#[test]
fn replay_window_changes_after_eviction_and_late_completion_cannot_recreate_it() {
    let mut ledger = AcceptedAgentInputSequences::default();
    let original = ledger.prepare_client("original");
    ledger.record("original", 9);
    for client in 0..DAEMON_INPUT_DEDUPE_CLIENTS_MAX {
        let client = format!("other-{client}");
        ledger.prepare_client(&client);
        ledger.record(&client, 1);
    }
    assert_eq!(ledger.by_client.len(), DAEMON_INPUT_DEDUPE_CLIENTS_MAX);
    assert_eq!(ledger.epochs.len(), DAEMON_INPUT_DEDUPE_CLIENTS_MAX);
    ledger.record("original", 10);
    assert!(!ledger.epochs.contains_key("original"));
    assert_ne!(ledger.prepare_client("original"), original);
    assert!(!ledger.already_accepted("original", 9));
}

#[test]
fn output_drain_keeps_allocation_and_copies_only_consumed_prefixes() {
    let buffer = Arc::new(Mutex::new(AgentOutputBuffer::default()));
    let (tx, _rx) = mpsc::channel();
    let original = vec![b'x'; 2 * 1024 * 1024];
    queue_agent_output(&buffer, &tx, 1, AgentOutputKind::Output, original.clone()).unwrap();
    let allocation = lock_agent_output_buffer(&buffer)
        .segments
        .front()
        .unwrap()
        .data
        .as_ptr();
    let mut received = Vec::new();
    for index in 0..128 {
        let (chunk, more) = take_agent_output_chunk(&buffer, 16 * 1024);
        received.extend(chunk.unwrap().data);
        if more {
            let pending = lock_agent_output_buffer(&buffer);
            let front = pending.segments.front().unwrap();
            assert_eq!(front.data.as_ptr(), allocation);
            assert_eq!(front.offset, (index + 1) * 16 * 1024);
        }
    }
    assert_eq!(received, original);
    assert_eq!(lock_agent_output_buffer(&buffer).queued_bytes, 0);
}

#[cfg(unix)]
#[test]
fn exit_sends_final_snapshot_before_notice_after_output_discard() {
    let (left, mut right) = AgentStream::pair().unwrap();
    right.set_nonblocking(true).unwrap();
    let mut conn = DaemonConnection::new(left).unwrap();
    for _ in 0..200 {
        conn.send_event(&AgentDaemonEvent::Output {
            data: vec![b'x'; 16 * 1024],
        })
        .unwrap();
        if conn.needs_resync {
            break;
        }
    }
    assert!(conn.needs_resync);
    conn.send_exit_after_resync("done".into(), || AgentDaemonEvent::Snapshot {
        data: b"FINAL-MARKER".to_vec(),
        state: None,
    })
    .unwrap();
    let mut received = Vec::new();
    super::tests::drain_daemon_connection_pair(&mut conn, &mut right, &mut received);
    let events: Vec<AgentDaemonEvent> = received
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert!(matches!(
        events.last(),
        Some(AgentDaemonEvent::Exited { .. })
    ));
    assert!(
        matches!(events.get(events.len() - 2), Some(AgentDaemonEvent::Snapshot { data, .. }) if data == b"FINAL-MARKER")
    );
    assert!(!conn.needs_resync);
}

#[cfg(unix)]
#[test]
fn client_checkpoint_restores_paste_mode_history_and_ordered_output() {
    let (mut client, _requests) =
        super::tests::buffered_output_test_client_with_requests("checkpoint-order", 9910);
    let mut daemon = vt100::Parser::new(6, 40, 100);
    daemon.process(b"before\x1b[?2004h\x1b[?1h");
    let mut history = ScreenHistory::default();
    history.capture_lines(vec!["old frame".into()]);
    let snapshot = AgentTerminalSnapshot {
        parser: daemon.checkpoint(true),
        history,
    }
    .prepare()
    .unwrap();
    let (tx, _rx) = mpsc::channel();
    queue_agent_output_with_snapshot(
        &client.output_buffer,
        &tx,
        client.reader_id,
        AgentOutputKind::Snapshot,
        Vec::new(),
        Some(snapshot),
    )
    .unwrap();
    queue_agent_output(
        &client.output_buffer,
        &tx,
        client.reader_id,
        AgentOutputKind::Output,
        b"after".to_vec(),
    )
    .unwrap();
    assert!(client.drain_pending_output(1));
    assert_eq!(client.parser.screen().size(), (6, 40));
    assert!(client.bracketed_paste_mode);
    assert!(client.parser.screen().application_cursor());
    assert_eq!(client.screen_history.all_lines(), ["old frame"]);
    while client.drain_pending_output(1) {}
    assert!(client.parser.screen().contents().contains("beforeafter"));
    client.exited = Some("test cleanup".into());
}

#[cfg(unix)]
#[test]
fn legacy_snapshot_keeps_advertised_paste_mode_but_honors_resets() {
    let (mut client, _requests) =
        super::tests::buffered_output_test_client_with_requests("legacy-paste", 9913);
    client.bracketed_paste_mode = true;
    client.process_agent_snapshot(b"\x1b[2J\x1b[Hlegacy");
    client.process_agent_output(b" output", true);
    assert!(client.bracketed_paste_mode);
    assert!(client.parser.screen().bracketed_paste());

    client.process_agent_snapshot(b"\x1b[2J\x1b[Hnew\x1b[?2004l");
    assert!(!client.bracketed_paste_mode);
    client.process_agent_output(b"\x1b[?2004h", true);
    assert!(client.bracketed_paste_mode);
    client.process_agent_output(b"\x1bc", true);
    assert!(!client.bracketed_paste_mode);
    client.exited = Some("test cleanup".into());
}

#[cfg(unix)]
#[test]
fn client_resize_waits_for_ack_and_does_not_commit_failed_send() {
    let (mut client, requests) =
        super::tests::buffered_output_test_client_with_requests("resize-ack", 9911);
    let before = client.parser.screen().size();
    client.resize(100, 30);
    assert_eq!(client.parser.screen().size(), before);
    assert_eq!(client.pending_resize, Some((30, 100)));
    client.resize(100, 30);
    assert_eq!(requests.try_iter().count(), 1);
    client.apply_acknowledged_pty_size((30, 100));
    assert_eq!(client.parser.screen().size(), (30, 100));
    assert!(client.pending_resize.is_none());
    drop(requests);
    client.resize(40, 6);
    assert_eq!(client.parser.screen().size(), (30, 100));
    assert!(client.pending_resize.is_none());
    assert!(client.connection_ended.is_some());
    client.exited = Some("test cleanup".into());
}

#[cfg(unix)]
#[test]
fn expired_replay_window_keeps_input_without_replaying_or_accepting_new_input() {
    let (mut client, requests) =
        super::tests::buffered_output_test_client_with_requests("expired-input", 9912);
    client.input_acknowledgements = true;
    client.input_replay_epoch = Some("old-window".into());
    client.daemon_pid = 123;
    client.daemon_pid_start_ticks = Some(456);
    client
        .send_input_data(b"do not duplicate\r".to_vec())
        .unwrap();
    requests.try_recv().unwrap();
    client.input_replay_epoch = Some("new-window".into());
    client.unacknowledged_input_replay_queued = false;
    assert!(client.replay_unacknowledged_input().is_err());
    assert_eq!(client.unacknowledged_input.len(), 1);
    assert!(requests.try_recv().is_err());
    assert_eq!(
        client.send_input_data(b"new".to_vec()).unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    client.exited = Some("test cleanup".into());
}
