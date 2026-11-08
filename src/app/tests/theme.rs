use super::*;

use ratatui::style::Color;

/// True for colors the terminal resolves from its own palette: the default
/// foreground/background (`Reset`) and the 16 named ANSI colors. Omarchy
/// (and every terminal theme) rewrites exactly these on a theme switch, so a
/// UI drawn only from them follows the theme without minimacs knowing it.
fn is_terminal_palette(color: Color) -> bool {
    !matches!(color, Color::Rgb(..) | Color::Indexed(_))
}

/// Every cell must use palette colors only, and nothing may paint ANSI
/// black/white text on the terminal's default background: Omarchy maps ANSI
/// black to the theme background (invisible text) and white to the
/// foreground, and on a plain light/dark terminal one of them is invisible.
fn assert_screen_follows_terminal_theme(terminal: &Terminal<TestBackend>, scenario: &str) {
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            assert!(
                is_terminal_palette(cell.fg) && is_terminal_palette(cell.bg),
                "{scenario}: cell ({x},{y}) {:?} uses a hardcoded color fg={:?} bg={:?}",
                cell.symbol(),
                cell.fg,
                cell.bg
            );
            if cell.bg == Color::Reset {
                assert!(
                    !matches!(cell.fg, Color::Black | Color::White),
                    "{scenario}: cell ({x},{y}) {:?} paints {:?} text on the default background",
                    cell.symbol(),
                    cell.fg
                );
            }
        }
    }
}

#[test]
fn split_panes_with_region_and_syntax_use_only_the_terminal_palette() {
    let text = "// comment\nfn main() {\n    let s = \"string\";\n    let n = 42;\n}\n";
    let events = vec![
        ctrl('x'),
        char_key('3'), // side-by-side split: separator + unfocused mode line
        ctrl(' '),     // set mark
        ctrl('n'),     // extend region over the first line
    ];
    let (mut app, mut events) = test_app_with_text(60, 12, text, events);
    app.editor
        .enable_buffer_syntax_for_test(0, crate::syntax::Language::Rust);
    app.run_until_idle(&mut events).unwrap();
    assert_screen_follows_terminal_theme(&app.terminal, "split + region + syntax");
}

#[test]
fn isearch_highlights_use_only_the_terminal_palette() {
    let text = "hello world hello again hello";
    let mut events = vec![ctrl('s')];
    events.extend(key_events("hello"));
    let (mut app, mut events) = test_app_with_text(60, 8, text, events);
    app.run_until_idle(&mut events).unwrap();
    assert!(app.editor.isearch().is_some());
    assert_screen_follows_terminal_theme(&app.terminal, "isearch");
}

#[test]
fn completion_list_uses_only_the_terminal_palette() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["alpha.txt", "beta.txt", "gamma.txt", "delta.txt"] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }
    std::fs::create_dir(dir.path().join("src")).unwrap();

    let mut events = vec![ctrl('x'), ctrl('f')];
    events.extend((0..200).map(|_| key(KeyCode::Backspace)));
    events.extend(key_events(&format!("{}/", dir.path().display())));
    events.push(key(KeyCode::Tab));
    let (mut app, mut events) = test_app(60, 12, events);
    app.run_until_idle(&mut events).unwrap();
    assert!(app.editor.minibuffer().completions().is_some());
    assert_screen_follows_terminal_theme(&app.terminal, "completions");
}
