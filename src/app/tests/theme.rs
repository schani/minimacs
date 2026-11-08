use super::*;

use ratatui::style::{Color, Modifier};

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Screen position of the first occurrence of `text` on any row.
fn find_text(terminal: &Terminal<TestBackend>, text: &str) -> (u16, u16) {
    let screen = capture_screen(terminal);
    for (y, line) in screen.lines().enumerate() {
        if let Some(byte) = line.find(text) {
            return (line[..byte].chars().count() as u16, y as u16);
        }
    }
    panic!("{text:?} not on screen:\n{screen}");
}

fn cell_at(terminal: &Terminal<TestBackend>, x: u16, y: u16) -> ratatui::buffer::Cell {
    terminal.backend().buffer()[(x, y)].clone()
}

// === Without a desktop theme the default stays VS Code Light+ ===

#[test]
fn default_theme_draws_split_panes_in_light_plus() {
    let events = vec![
        ctrl('x'),
        char_key('3'), // side-by-side split: separator + unfocused mode line
        ctrl(' '),     // set mark
        ctrl('n'),     // region over the first line
    ];
    let (mut app, mut events) = test_app_with_text(60, 12, "first\nsecond\n", events);
    app.run_until_idle(&mut events).unwrap();
    let terminal = &app.terminal;

    let region = cell_at(terminal, 0, 0);
    assert_eq!((region.fg, region.bg), (Color::Reset, rgb(173, 214, 255)));

    let tilde = find_text(terminal, "~");
    assert_eq!(
        cell_at(terminal, tilde.0, tilde.1).fg,
        rgb(35, 120, 147),
        "fringe"
    );

    let bar = find_text(terminal, "│");
    let separator = cell_at(terminal, bar.0, bar.1);
    assert_eq!(
        (separator.fg, separator.bg),
        (rgb(200, 200, 200), rgb(255, 255, 255))
    );

    let mode_line_row = 10;
    let focused = cell_at(terminal, 1, mode_line_row);
    assert_eq!((focused.fg, focused.bg), (Color::White, rgb(0, 122, 204)));
    assert!(focused.modifier.contains(Modifier::BOLD));
    let unfocused = cell_at(terminal, bar.0 + 2, mode_line_row);
    assert_eq!(
        (unfocused.fg, unfocused.bg),
        (rgb(51, 51, 51), rgb(223, 223, 223))
    );
    assert!(!unfocused.modifier.contains(Modifier::BOLD));
}

#[test]
fn default_theme_draws_search_matches_in_light_plus() {
    let mut events = vec![ctrl('s')];
    events.extend(key_events("hello"));
    let (mut app, mut events) = test_app_with_text(60, 8, "hello world hello", events);
    app.run_until_idle(&mut events).unwrap();

    let current = cell_at(&app.terminal, 0, 0);
    assert_eq!((current.fg, current.bg), (Color::Black, rgb(168, 172, 148)));
    let other = cell_at(&app.terminal, 12, 0);
    assert_eq!((other.fg, other.bg), (Color::Black, rgb(248, 201, 171)));
}

#[test]
fn default_theme_draws_completions_in_light_plus() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["alpha.txt", "beta.txt"] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }

    let mut events = vec![ctrl('x'), ctrl('f')];
    events.extend((0..200).map(|_| key(KeyCode::Backspace)));
    events.extend(key_events(&format!("{}/", dir.path().display())));
    events.push(key(KeyCode::Tab));
    let (mut app, mut events) = test_app(60, 12, events);
    app.run_until_idle(&mut events).unwrap();

    let (x, y) = find_text(&app.terminal, "alpha.txt");
    let cell = cell_at(&app.terminal, x, y);
    assert_eq!((cell.fg, cell.bg), (Color::Black, rgb(243, 243, 243)));
}
