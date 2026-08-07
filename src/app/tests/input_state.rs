use super::*;

fn release(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new_with_kind(
        code,
        modifiers,
        KeyEventKind::Release,
    ))
}

#[test]
fn key_release_events_are_ignored() {
    // Kitty-protocol terminals (and Windows) report release events;
    // acting on them would execute every keystroke twice.
    let events = vec![
        char_key('a'),
        release(KeyCode::Char('a'), KeyModifiers::NONE),
        char_key('b'),
        release(KeyCode::Char('b'), KeyModifiers::NONE),
    ];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.buffer_text(), "ab");
}

#[test]
fn key_release_does_not_start_isearch() {
    let events = vec![release(KeyCode::Char('s'), KeyModifiers::CONTROL)];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    assert!(app.editor.isearch.is_none());
}

#[test]
fn key_release_does_not_advance_pending_chord() {
    // C-x pressed, then a released C-c: the chord must neither complete
    // (no quit) nor be consumed — a following C-c press still quits.
    let events = vec![
        ctrl('x'),
        release(KeyCode::Char('c'), KeyModifiers::CONTROL),
    ];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    assert!(!app.editor.should_quit);
    assert_eq!(app.editor.pending_keys, "C-x ");

    let mut events = TestEventSource::new(vec![ctrl('c')]);
    app.run_until_idle(&mut events).unwrap();
    assert!(app.editor.should_quit);
}

/// Event source scripted with an explicit sequence of poll outcomes,
/// including timeouts and closure — unlike `TestEventSource`, which only
/// ever yields events until it closes.
struct ScriptedEventSource {
    polls: std::collections::VecDeque<Poll>,
}

impl EventSource for ScriptedEventSource {
    fn next_event(&mut self) -> Poll {
        self.polls.pop_front().unwrap_or(Poll::Closed)
    }
}

#[test]
fn run_returns_error_when_event_source_closes() {
    // A dead terminal (poll/read error) must terminate the main loop
    // with an error, not be treated as a timeout — the old code spun
    // forever at 10 polls/s on a hung-up tty.
    let (mut app, _) = test_app(40, 10, vec![]);
    let mut source = ScriptedEventSource {
        polls: std::collections::VecDeque::new(),
    };
    let err = app.run(&mut source).unwrap_err();
    assert!(
        err.to_string().contains("event source closed"),
        "unexpected error: {err}"
    );
}

#[test]
fn run_continues_after_timeout_and_errors_on_close() {
    // A timeout is idle, not death: the loop must keep going and
    // process later events; only Closed ends it.
    let (mut app, _) = test_app(40, 10, vec![]);
    let mut source = ScriptedEventSource {
        polls: [Poll::Timeout, Poll::Event(char_key('a')), Poll::Closed].into(),
    };
    let result = app.run(&mut source);
    assert!(result.is_err());
    assert_eq!(app.editor.buffer_text(), "a");
}

#[test]
fn run_skips_render_for_discarded_events() {
    // The production loop must not redraw after events that changed
    // nothing: any-motion mouse tracking (mode 1003) floods Moved
    // events on bare movement, each of which used to be a full render.
    let (mut app, _) = test_app(40, 10, vec![]);
    let mut source = ScriptedEventSource {
        polls: [
            Poll::Event(mouse_moved(1, 1)),
            Poll::Event(mouse_moved(2, 1)),
            Poll::Event(char_key('a')),
            Poll::Event(mouse_moved(3, 1)),
            Poll::Closed,
        ]
        .into(),
    };
    let _ = app.run(&mut source);
    assert_eq!(app.editor.buffer_text(), "a");
    assert_eq!(
        app.renders, 2,
        "initial render plus one for the keystroke; none for motion"
    );
}

#[test]
fn run_exits_cleanly_on_quit_before_source_closes() {
    // A normal quit must still return Ok — the queue behind it never
    // gets a chance to close the source.
    let (mut app, mut events) = test_app(40, 10, vec![ctrl('x'), ctrl('c')]);
    app.run(&mut events).unwrap();
    assert!(app.editor.should_quit);
}

#[test]
fn cg_cancels_pending_keys() {
    let events = vec![ctrl('x'), ctrl('g')];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    assert!(!app.editor.should_quit);
    assert_eq!(app.editor.pending_keys, "");
}

#[test]
fn unbound_key_after_prefix_does_not_self_insert() {
    let mut events = vec![ctrl('x'), char_key('j')];
    events.extend(key_events("")); // no-op, keeps style consistent
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    // The dead-end chord must not insert a literal 'j' or modify the buffer.
    assert_eq!(app.editor.buffer_text(), "");
    assert!(!app.editor.current_buffer().modified);
    // The user gets feedback instead.
    let screen = capture_screen(&app.terminal);
    assert!(screen.contains("C-x j is undefined"), "screen: {screen}");
}

#[test]
fn pending_keys_show_in_mode_line() {
    let events = vec![ctrl('x')];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    assert!(!app.editor.pending_keys.is_empty());
    let screen = capture_screen(&app.terminal);
    // Mode line should contain pending keys
    assert!(screen.contains("C-x"));
}

fn mouse_moved(x: u16, y: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    })
}

// === Esc-as-Meta prefix tests ===

#[test]
fn esc_less_than_moves_to_buffer_beginning() {
    let text = "hello\nworld\nfoo";
    let events = vec![
        ctrl('e'),         // go to end of first line
        key(KeyCode::Esc), // Esc prefix
        char_key('<'),     // < — should become M-<
    ];
    let (mut app, mut events) = test_app_with_text(40, 10, text, events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.point(), 0);
}

#[test]
fn esc_greater_than_moves_to_buffer_end() {
    let text = "hello\nworld\nfoo";
    let events = vec![
        key(KeyCode::Esc), // Esc prefix
        char_key('>'),     // > — should become M->
    ];
    let (mut app, mut events) = test_app_with_text(40, 10, text, events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.point(), 15); // "hello\nworld\nfoo" = 15 chars
}

#[test]
fn esc_f_moves_forward_word() {
    let text = "hello world";
    let events = vec![
        key(KeyCode::Esc), // Esc prefix
        char_key('f'),     // f — should become M-f
    ];
    let (mut app, mut events) = test_app_with_text(40, 10, text, events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.point(), 5); // end of "hello"
}

#[test]
fn esc_shows_pending_indicator() {
    let events = vec![key(KeyCode::Esc)];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    assert!(app.editor.pending_keys.contains("ESC"));
}

// === Input-state tests ===
//
// These pin down how the pending-chord (`C-x ...`) and pending-ESC state
// interact with the other event kinds: paste and mouse events cancel any
// pending input (like C-g does), while a resize leaves it alone.

#[test]
fn cg_cancelled_chord_key_self_inserts() {
    // After C-g cancels a pending C-x chord, the next key must go through
    // the keymap from the root: 's' self-inserts instead of completing
    // C-x C-s.
    let events = vec![ctrl('x'), ctrl('g'), char_key('s')];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.buffer_text(), "s");
    assert_eq!(app.editor.pending_keys, "");
}

#[test]
fn cg_cancels_pending_esc() {
    // C-g clears a pending ESC prefix: the following 'f' self-inserts
    // instead of running M-f.
    let events = vec![key(KeyCode::Esc), ctrl('g'), char_key('f')];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.buffer_text(), "fhello world");
    assert_eq!(app.editor.pending_keys, "");
}

#[test]
fn pending_chord_survives_resize() {
    // A resize must not cancel a chord in progress: C-x <resize> 2 still
    // splits the window.
    let events = vec![ctrl('x'), Event::Resize(40, 10), char_key('2')];
    let (mut app, mut events) = test_app_with_text(40, 12, "hello", events);
    app.run_until_idle(&mut events).unwrap();
    let screen = capture_screen(&app.terminal);
    let mode_line_count = screen.lines().filter(|l| l.contains("--")).count();
    assert!(
        mode_line_count >= 2,
        "C-x 2 across a resize should split, screen:\n{screen}"
    );
}

#[test]
fn paste_cancels_pending_chord() {
    // A paste event cancels a pending chord: C-x <paste> C-s must NOT
    // complete C-x C-s (save-file); the paste is inserted and the C-s
    // starts an incremental search from the keymap root.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();

    let events = vec![ctrl('x'), Event::Paste("Y".to_string()), ctrl('s')];
    let backend = TestBackend::new(40, 10);
    let terminal = Terminal::new(backend).unwrap();
    let mut app = App::new(terminal, editor);
    let mut event_source = TestEventSource::new(events);
    app.run_until_idle(&mut event_source).unwrap();

    assert_eq!(app.editor.buffer_text(), "Yoriginal", "paste was inserted");
    assert!(
        app.editor.isearch.is_some(),
        "C-s after the paste starts isearch instead of completing C-x C-s"
    );
    assert_eq!(app.editor.pending_keys, "", "no pending prefix remains");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "original",
        "file must not have been saved"
    );
}

#[test]
fn mouse_click_cancels_pending_chord() {
    // A mouse click mid-chord cancels the chord AND performs the click
    // (cancel-then-handle): after C-x <click on other pane>, '2' goes
    // through the keymap from the root and self-inserts into the clicked
    // pane instead of completing C-x 2.
    let mut events = vec![ctrl('x'), char_key('2')]; // split first
    events.push(ctrl('x')); // start a chord
    events.push(mouse_click(3, 6)); // click the bottom pane mid-chord
    events.push(char_key('2'));
    let (mut app, mut events) = test_app_with_text(40, 10, "hello\nworld", events);
    app.run_until_idle(&mut events).unwrap();

    assert_eq!(app.editor.pending_keys, "", "the click cancelled the chord");
    assert_eq!(
        app.editor.pane_tree.focus_path(),
        &[1],
        "the click still switched focus to the second pane"
    );
    assert!(
        app.editor.buffer_text().contains('2'),
        "'2' self-inserted instead of completing C-x 2, text: {:?}",
        app.editor.buffer_text()
    );
}

#[test]
fn mouse_click_cancels_pending_esc() {
    // A mouse click cancels a pending ESC: ESC <click> 'f' self-inserts
    // 'f' at the clicked position instead of running M-f (forward-word).
    let events = vec![key(KeyCode::Esc), mouse_click(0, 0), char_key('f')];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.buffer_text(), "fhello world");
    assert_eq!(app.editor.point(), 1, "point after the self-inserted 'f'");
}

#[test]
fn paste_cancels_pending_esc() {
    // A paste cancels a pending ESC: ESC <paste> 'f' self-inserts 'f'
    // after the pasted text instead of running M-f.
    let events = vec![
        key(KeyCode::Esc),
        Event::Paste("xy".to_string()),
        char_key('f'),
    ];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.buffer_text(), "xyfhello world");
    assert_eq!(app.editor.point(), 3, "point after the self-inserted 'f'");
}

// === Render gating tests ===
//
// `EnableMouseCapture` turns on any-motion tracking (mode 1003), so bare
// mouse movement floods `Moved` events. Discarded events must not
// trigger a render (or moving the mouse over the terminal becomes a
// render storm). The run loop renders once up front; only events that
// may have changed state render again.

#[test]
fn mouse_motion_does_not_render() {
    let events = vec![mouse_moved(1, 1), mouse_moved(2, 1), mouse_moved(3, 2)];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.renders, 1, "only the initial render, none per motion");
}

#[test]
fn discarded_mouse_kinds_do_not_render() {
    // Drag, button release, and non-left buttons are discarded by
    // handle_mouse; none of them may trigger a render.
    let discarded = [
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
    ];
    let events = discarded
        .iter()
        .map(|&kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 1,
                row: 1,
                modifiers: KeyModifiers::NONE,
            })
        })
        .collect();
    let (mut app, mut events) = test_app_with_text(40, 10, "hello", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.renders, 1, "only the initial render");
}

#[test]
fn mouse_click_and_scroll_render() {
    let events = vec![mouse_click(2, 1), mouse_scroll_down(2, 1)];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello\nworld", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.renders, 3, "initial render plus one per acted-on event");
}

#[test]
fn key_release_does_not_render() {
    let events = vec![release(KeyCode::Char('a'), KeyModifiers::NONE)];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.renders, 1, "only the initial render");
}

#[test]
fn focus_events_do_not_render() {
    let events = vec![Event::FocusGained, Event::FocusLost];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.renders, 1, "only the initial render");
}

#[test]
fn key_press_and_resize_render() {
    let events = vec![char_key('a'), Event::Resize(50, 12)];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello", events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.renders, 3, "initial render plus one per event");
}

#[test]
fn mouse_motion_does_not_cancel_pending_chord() {
    // Bare mouse motion is discarded, so it must NOT cancel a pending
    // chord: C-x <motion> 2 still completes C-x 2 (split). Only mouse
    // events that are acted on (left click, scroll) cancel the chord —
    // otherwise merely moving the mouse over the terminal would kill
    // every chord mid-way under any-motion tracking.
    let events = vec![ctrl('x'), mouse_moved(5, 3), char_key('2')];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello\nworld", events);
    app.run_until_idle(&mut events).unwrap();

    let screen = capture_screen(&app.terminal);
    let mode_line_count = screen.lines().filter(|l| l.contains("--")).count();
    assert!(
        mode_line_count >= 2,
        "C-x 2 across mouse motion should split, screen:\n{screen}"
    );
    assert_eq!(app.editor.pending_keys, "", "chord completed");
}
