use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::Command;

/// A normalized key representation for the keymap trie.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
}

impl Key {
    pub fn from_event(event: KeyEvent) -> Self {
        Self {
            code: event.code,
            ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
        }
    }

    /// Human-readable display of the key for the mode line.
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("C-");
        }
        if self.alt {
            s.push_str("M-");
        }
        match self.code {
            KeyCode::Char(c) => s.push(c),
            KeyCode::Enter => s.push_str("RET"),
            KeyCode::Tab => s.push_str("TAB"),
            KeyCode::BackTab => s.push_str("S-TAB"),
            KeyCode::Backspace => s.push_str("DEL"),
            KeyCode::Delete => s.push_str("Delete"),
            KeyCode::Left => s.push_str("Left"),
            KeyCode::Right => s.push_str("Right"),
            KeyCode::Up => s.push_str("Up"),
            KeyCode::Down => s.push_str("Down"),
            KeyCode::Home => s.push_str("Home"),
            KeyCode::End => s.push_str("End"),
            KeyCode::PageUp => s.push_str("PgUp"),
            KeyCode::PageDown => s.push_str("PgDn"),
            KeyCode::Esc => s.push_str("ESC"),
            _ => s.push('?'),
        }
        s
    }
}

/// A node in the keymap trie.
#[derive(Debug, Clone)]
pub struct KeymapNode {
    pub children: HashMap<Key, KeymapNode>,
    pub command: Option<Command>,
}

impl KeymapNode {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
            command: None,
        }
    }

    /// Bind a key sequence to a command.
    pub fn bind(&mut self, keys: &[Key], command: Command) {
        if keys.is_empty() {
            self.command = Some(command);
            return;
        }
        let child = self.children.entry(keys[0].clone()).or_insert_with(KeymapNode::new);
        child.bind(&keys[1..], command);
    }

    /// Look up a key in this node's children.
    pub fn get(&self, key: &Key) -> Option<&KeymapNode> {
        self.children.get(key)
    }
}

/// Result of processing a key through the keymap.
#[derive(Debug)]
pub enum KeymapResult {
    /// A command was matched.
    Matched(Command),
    /// Partial match — waiting for more keys.
    Pending,
    /// No match found.
    NotFound,
}

/// State machine for multi-key chord processing.
#[allow(dead_code)]
pub struct KeymapState {
    pending_keys: Vec<Key>,
    keymap: KeymapNode, // root of trie
}

#[allow(dead_code)]
impl KeymapState {
    pub fn new(keymap: KeymapNode) -> Self {
        Self {
            pending_keys: Vec::new(),
            keymap,
        }
    }

    /// Process a key event and return the result.
    pub fn process_key(&mut self, event: KeyEvent) -> KeymapResult {
        let key = Key::from_event(event);
        self.pending_keys.push(key.clone());

        // Walk the trie from root using all pending keys
        let mut node = &self.keymap;
        for k in &self.pending_keys {
            match node.get(k) {
                Some(child) => node = child,
                None => {
                    self.pending_keys.clear();
                    return KeymapResult::NotFound;
                }
            }
        }

        // If we reached a node with a command, return it
        if let Some(cmd) = &node.command {
            let result = cmd.clone();
            self.pending_keys.clear();
            return KeymapResult::Matched(result);
        }

        // If the node has children, we're pending
        if !node.children.is_empty() {
            return KeymapResult::Pending;
        }

        // Dead end
        self.pending_keys.clear();
        KeymapResult::NotFound
    }

    /// Get the display string for currently pending keys.
    pub fn pending_display(&self) -> String {
        if self.pending_keys.is_empty() {
            return String::new();
        }
        self.pending_keys
            .iter()
            .map(|k| k.display())
            .collect::<Vec<_>>()
            .join(" ")
            + " "
    }

    /// Clear pending keys (used by C-g cancel).
    pub fn clear(&mut self) {
        self.pending_keys.clear();
    }

    /// Whether we have pending keys.
    pub fn has_pending(&self) -> bool {
        !self.pending_keys.is_empty()
    }

    /// Replace the keymap (used for vim mode switching).
    pub fn set_keymap(&mut self, keymap: KeymapNode) {
        self.keymap = keymap;
        self.pending_keys.clear();
    }
}

// Helper constructors for keys
fn ctrl(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        ctrl: true,
        alt: false,
    }
}

fn alt(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        ctrl: false,
        alt: true,
    }
}

fn alt_key(code: KeyCode) -> Key {
    Key {
        code,
        ctrl: false,
        alt: true,
    }
}

fn plain(code: KeyCode) -> Key {
    Key {
        code,
        ctrl: false,
        alt: false,
    }
}

/// Build the default emacs-like keymap.
pub fn default_keymap() -> KeymapNode {
    let mut root = KeymapNode::new();

    // Movement
    root.bind(&[ctrl('f')], Command::ForwardChar);
    root.bind(&[ctrl('b')], Command::BackwardChar);
    root.bind(&[ctrl('n')], Command::NextLine);
    root.bind(&[ctrl('p')], Command::PreviousLine);
    root.bind(&[ctrl('a')], Command::BeginningOfLine);
    root.bind(&[ctrl('e')], Command::EndOfLine);
    root.bind(&[ctrl('v')], Command::PageDown);
    root.bind(&[alt('v')], Command::PageUp);
    root.bind(&[alt('f')], Command::ForwardWord);
    root.bind(&[alt('b')], Command::BackwardWord);
    root.bind(&[alt_key(KeyCode::Right)], Command::ForwardWord);
    root.bind(&[alt_key(KeyCode::Left)], Command::BackwardWord);
    root.bind(&[alt('<')], Command::BufferBeginning);
    root.bind(&[alt('>')], Command::BufferEnd);

    // Arrow keys
    root.bind(&[plain(KeyCode::Right)], Command::ForwardChar);
    root.bind(&[plain(KeyCode::Left)], Command::BackwardChar);
    root.bind(&[plain(KeyCode::Down)], Command::NextLine);
    root.bind(&[plain(KeyCode::Up)], Command::PreviousLine);
    root.bind(&[plain(KeyCode::Home)], Command::BeginningOfLine);
    root.bind(&[plain(KeyCode::End)], Command::EndOfLine);
    root.bind(&[plain(KeyCode::PageDown)], Command::PageDown);
    root.bind(&[plain(KeyCode::PageUp)], Command::PageUp);

    // Editing
    root.bind(&[plain(KeyCode::Enter)], Command::InsertNewline);
    root.bind(&[plain(KeyCode::Backspace)], Command::DeleteBackward);
    root.bind(&[ctrl('d')], Command::DeleteForward);
    root.bind(&[plain(KeyCode::Delete)], Command::DeleteForward);
    root.bind(&[ctrl('k')], Command::KillLine);
    root.bind(&[plain(KeyCode::Tab)], Command::IndentLine);
    root.bind(&[plain(KeyCode::BackTab)], Command::DedentLine);
    root.bind(
        &[alt_key(KeyCode::Backspace)],
        Command::DeleteWordBackward,
    );

    // Undo/Redo
    root.bind(&[ctrl('/')], Command::Undo);
    // C-_ is also undo in emacs, crossterm sends it as ctrl('_')
    root.bind(&[ctrl('_')], Command::Undo);
    // Many terminals send C-/ as Ctrl-7 (byte 0x1F, shared key on US keyboards)
    root.bind(&[ctrl('7')], Command::Undo);
    // Kitty keyboard protocol sends C-_ as base key '-' with Ctrl+Shift;
    // Key::from_event strips Shift, so we need ctrl('-')
    root.bind(&[ctrl('-')], Command::Undo);

    // Mark/Region
    root.bind(&[ctrl(' ')], Command::SetMark);
    root.bind(&[ctrl('w')], Command::Cut);
    root.bind(&[alt('w')], Command::Copy);
    root.bind(&[ctrl('y')], Command::Paste);

    // C-x prefixed commands
    root.bind(&[ctrl('x'), ctrl('c')], Command::Quit);
    root.bind(&[ctrl('x'), ctrl('s')], Command::Save);
    root.bind(&[ctrl('x'), ctrl('w')], Command::WriteFile);
    root.bind(&[ctrl('x'), ctrl('f')], Command::FindFile);
    root.bind(
        &[ctrl('x'), plain(KeyCode::Char('b'))],
        Command::SwitchBuffer,
    );
    root.bind(
        &[ctrl('x'), plain(KeyCode::Char('k'))],
        Command::KillBuffer,
    );
    root.bind(
        &[ctrl('x'), plain(KeyCode::Char('2'))],
        Command::SplitVertical,
    );
    root.bind(
        &[ctrl('x'), plain(KeyCode::Char('3'))],
        Command::SplitHorizontal,
    );
    root.bind(
        &[ctrl('x'), plain(KeyCode::Char('0'))],
        Command::DeletePane,
    );
    root.bind(
        &[ctrl('x'), plain(KeyCode::Char('1'))],
        Command::DeleteOtherPanes,
    );
    root.bind(
        &[ctrl('x'), plain(KeyCode::Char('o'))],
        Command::CycleFocus,
    );
    root.bind(&[ctrl('x'), ctrl('x')], Command::SwapPointAndMark);

    // Display
    root.bind(&[ctrl('l')], Command::RecenterTopBottom);

    // Search
    root.bind(&[ctrl('s')], Command::ISearchForward);
    root.bind(&[ctrl('r')], Command::ISearchBackward);

    // Cancel
    root.bind(&[ctrl('g')], Command::Cancel);

    // M-g g → goto-line
    root.bind(
        &[alt('g'), plain(KeyCode::Char('g'))],
        Command::GotoLine,
    );

    root
}

/// Build the vim normal mode keymap.
///
/// In normal mode, bare keys are commands (no self-insert).
/// Mode-switching keys (i, a, o, etc.) are NOT in this keymap;
/// they are handled directly in app.rs so it can switch the mode.
pub fn vim_normal_keymap() -> KeymapNode {
    let mut root = KeymapNode::new();

    // Movement
    root.bind(&[plain(KeyCode::Char('h'))], Command::BackwardChar);
    root.bind(&[plain(KeyCode::Char('l'))], Command::ForwardChar);
    root.bind(&[plain(KeyCode::Char('j'))], Command::NextLine);
    root.bind(&[plain(KeyCode::Char('k'))], Command::PreviousLine);
    root.bind(&[plain(KeyCode::Char('w'))], Command::ForwardWord);
    root.bind(&[plain(KeyCode::Char('b'))], Command::BackwardWord);
    root.bind(&[plain(KeyCode::Char('0'))], Command::BeginningOfLine);
    root.bind(
        &[plain(KeyCode::Char('$'))],
        Command::EndOfLine,
    );
    root.bind(
        &[plain(KeyCode::Char('G'))],
        Command::BufferEnd,
    );
    root.bind(
        &[plain(KeyCode::Char('g')), plain(KeyCode::Char('g'))],
        Command::BufferBeginning,
    );

    // Arrow keys
    root.bind(&[plain(KeyCode::Right)], Command::ForwardChar);
    root.bind(&[plain(KeyCode::Left)], Command::BackwardChar);
    root.bind(&[plain(KeyCode::Down)], Command::NextLine);
    root.bind(&[plain(KeyCode::Up)], Command::PreviousLine);
    root.bind(&[plain(KeyCode::Home)], Command::BeginningOfLine);
    root.bind(&[plain(KeyCode::End)], Command::EndOfLine);
    root.bind(&[plain(KeyCode::PageDown)], Command::PageDown);
    root.bind(&[plain(KeyCode::PageUp)], Command::PageUp);

    // Scroll (Ctrl-d / Ctrl-u)
    root.bind(&[ctrl('d')], Command::PageDown);
    root.bind(&[ctrl('u')], Command::PageUp);

    // Editing (stay in normal mode)
    root.bind(&[plain(KeyCode::Char('x'))], Command::DeleteForward);
    root.bind(
        &[plain(KeyCode::Char('d')), plain(KeyCode::Char('d'))],
        Command::DeleteLine,
    );
    root.bind(
        &[plain(KeyCode::Char('D'))],
        Command::KillLine,
    );
    root.bind(
        &[plain(KeyCode::Char('J'))],
        Command::JoinLines,
    );

    // Undo/Redo
    root.bind(&[plain(KeyCode::Char('u'))], Command::Undo);
    root.bind(&[ctrl('r')], Command::Redo);

    // Clipboard
    root.bind(&[plain(KeyCode::Char('p'))], Command::Paste);
    root.bind(&[plain(KeyCode::Char('v'))], Command::SetMark);
    // Note: standalone 'y' (copy visual selection) is handled in app.rs
    // before keymap lookup, so only 'yy' is bound here.
    root.bind(
        &[plain(KeyCode::Char('y')), plain(KeyCode::Char('y'))],
        Command::YankLine,
    );

    // Search
    root.bind(
        &[plain(KeyCode::Char('/'))],
        Command::ISearchForward,
    );
    root.bind(
        &[plain(KeyCode::Char('?'))],
        Command::ISearchBackward,
    );

    // Vim command line
    root.bind(
        &[plain(KeyCode::Char(':'))],
        Command::VimCommandPrompt,
    );

    // Display
    root.bind(&[ctrl('l')], Command::RecenterTopBottom);

    // Pane management (Ctrl-w prefix, like vim)
    root.bind(&[ctrl('w'), plain(KeyCode::Char('s'))], Command::SplitVertical);
    root.bind(&[ctrl('w'), plain(KeyCode::Char('v'))], Command::SplitHorizontal);
    root.bind(&[ctrl('w'), plain(KeyCode::Char('w'))], Command::CycleFocus);
    root.bind(&[ctrl('w'), plain(KeyCode::Char('q'))], Command::DeletePane);
    root.bind(&[ctrl('w'), plain(KeyCode::Char('o'))], Command::DeleteOtherPanes);

    root
}

/// Build the vim insert mode keymap.
///
/// In insert mode, most keys self-insert. This keymap only binds
/// editing helpers; Esc (to exit insert) is handled in app.rs.
pub fn vim_insert_keymap() -> KeymapNode {
    let mut root = KeymapNode::new();

    // Basic editing
    root.bind(&[plain(KeyCode::Enter)], Command::InsertNewline);
    root.bind(&[plain(KeyCode::Backspace)], Command::DeleteBackward);
    root.bind(&[plain(KeyCode::Delete)], Command::DeleteForward);
    root.bind(&[plain(KeyCode::Tab)], Command::IndentLine);
    root.bind(&[plain(KeyCode::BackTab)], Command::DedentLine);
    root.bind(&[ctrl('w')], Command::DeleteWordBackward);
    root.bind(&[ctrl('u')], Command::KillLine);

    // Arrow-key movement
    root.bind(&[plain(KeyCode::Right)], Command::ForwardChar);
    root.bind(&[plain(KeyCode::Left)], Command::BackwardChar);
    root.bind(&[plain(KeyCode::Down)], Command::NextLine);
    root.bind(&[plain(KeyCode::Up)], Command::PreviousLine);
    root.bind(&[plain(KeyCode::Home)], Command::BeginningOfLine);
    root.bind(&[plain(KeyCode::End)], Command::EndOfLine);

    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_key_lookup() {
        let keymap = default_keymap();
        let state = KeymapState::new(keymap);
        // Verify C-f is bound
        let node = state.keymap.get(&ctrl('f'));
        assert!(node.is_some());
        assert_eq!(node.unwrap().command, Some(Command::ForwardChar));
    }

    #[test]
    fn multi_key_chord_lookup() {
        let keymap = default_keymap();
        // C-x should have children
        let cx_node = keymap.get(&ctrl('x'));
        assert!(cx_node.is_some());
        let cx = cx_node.unwrap();
        // C-x C-s should map to Save
        let cxcs = cx.get(&ctrl('s'));
        assert!(cxcs.is_some());
        assert_eq!(cxcs.unwrap().command, Some(Command::Save));
    }

    #[test]
    fn keymap_state_single_key() {
        let keymap = default_keymap();
        let mut state = KeymapState::new(keymap);
        let event = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::ForwardChar),
            _ => panic!("Expected Matched"),
        }
    }

    #[test]
    fn keymap_state_multi_key() {
        let keymap = default_keymap();
        let mut state = KeymapState::new(keymap);

        // C-x should be pending
        let cx = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        match state.process_key(cx) {
            KeymapResult::Pending => {}
            other => panic!("Expected Pending, got {:?}", other),
        }
        assert!(state.has_pending());
        assert!(!state.pending_display().is_empty());

        // C-s should complete to Save
        let cs = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        match state.process_key(cs) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::Save),
            other => panic!("Expected Matched(Save), got {:?}", other),
        }
        assert!(!state.has_pending());
    }

    #[test]
    fn keymap_state_not_found() {
        let keymap = default_keymap();
        let mut state = KeymapState::new(keymap);
        // F12 is not bound
        let event = KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE);
        match state.process_key(event) {
            KeymapResult::NotFound => {}
            other => panic!("Expected NotFound, got {:?}", other),
        }
    }

    #[test]
    fn keymap_state_clear() {
        let keymap = default_keymap();
        let mut state = KeymapState::new(keymap);
        let cx = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        state.process_key(cx);
        assert!(state.has_pending());
        state.clear();
        assert!(!state.has_pending());
    }

    #[test]
    fn pending_display_format() {
        let keymap = default_keymap();
        let mut state = KeymapState::new(keymap);
        let cx = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        state.process_key(cx);
        assert_eq!(state.pending_display(), "C-x ");
    }

    #[test]
    fn alt_key_lookup() {
        let keymap = default_keymap();
        let mut state = KeymapState::new(keymap);
        let event = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::PageUp),
            other => panic!("Expected Matched(PageUp), got {:?}", other),
        }
    }

    #[test]
    fn alt_arrow_keys() {
        let keymap = default_keymap();
        let mut state = KeymapState::new(keymap);
        let event = KeyEvent::new(KeyCode::Left, KeyModifiers::ALT);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::BackwardWord),
            other => panic!("Expected Matched(BackwardWord), got {:?}", other),
        }

        let mut state = KeymapState::new(default_keymap());
        let event = KeyEvent::new(KeyCode::Right, KeyModifiers::ALT);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::ForwardWord),
            other => panic!("Expected Matched(ForwardWord), got {:?}", other),
        }
    }

    #[test]
    fn ctrl_underscore_undo_all_variants() {
        // Ctrl-_ should be Undo regardless of how the terminal reports it.
        // Legacy terminals: ctrl('_') or ctrl('7')
        // Kitty keyboard protocol: Ctrl+Shift+- reports as ctrl('-') with SHIFT stripped
        let keymap = default_keymap();

        let mut state = KeymapState::new(keymap.clone());
        let event = KeyEvent::new(KeyCode::Char('_'), KeyModifiers::CONTROL);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::Undo),
            other => panic!("Expected Matched(Undo) for ctrl('_'), got {:?}", other),
        }

        let mut state = KeymapState::new(keymap.clone());
        let event = KeyEvent::new(KeyCode::Char('7'), KeyModifiers::CONTROL);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::Undo),
            other => panic!("Expected Matched(Undo) for ctrl('7'), got {:?}", other),
        }

        // Kitty protocol: terminal sends base key '-' with Ctrl+Shift,
        // Key::from_event strips Shift, leaving ctrl('-')
        let mut state = KeymapState::new(keymap.clone());
        let event = KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::Undo),
            other => panic!("Expected Matched(Undo) for ctrl('-') (Kitty protocol), got {:?}", other),
        }
    }

    #[test]
    fn m_g_g_goto_line() {
        let keymap = default_keymap();
        let mut state = KeymapState::new(keymap);

        let mg = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT);
        match state.process_key(mg) {
            KeymapResult::Pending => {}
            other => panic!("Expected Pending, got {:?}", other),
        }

        let g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        match state.process_key(g) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::GotoLine),
            other => panic!("Expected Matched(GotoLine), got {:?}", other),
        }
    }

    // === Vim keymap tests ===

    #[test]
    fn vim_normal_hjkl() {
        let keymap = vim_normal_keymap();
        let cases = vec![
            ('h', Command::BackwardChar),
            ('j', Command::NextLine),
            ('k', Command::PreviousLine),
            ('l', Command::ForwardChar),
        ];
        for (c, expected) in cases {
            let mut state = KeymapState::new(keymap.clone());
            let event = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            match state.process_key(event) {
                KeymapResult::Matched(cmd) => assert_eq!(cmd, expected, "key '{}'", c),
                other => panic!("Expected Matched for '{}', got {:?}", c, other),
            }
        }
    }

    #[test]
    fn vim_normal_word_motion() {
        let keymap = vim_normal_keymap();

        let mut state = KeymapState::new(keymap.clone());
        let event = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::ForwardWord),
            other => panic!("Expected ForwardWord, got {:?}", other),
        }

        let mut state = KeymapState::new(keymap.clone());
        let event = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::BackwardWord),
            other => panic!("Expected BackwardWord, got {:?}", other),
        }
    }

    #[test]
    fn vim_normal_dd_delete_line() {
        let keymap = vim_normal_keymap();
        let mut state = KeymapState::new(keymap);

        let d1 = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        match state.process_key(d1) {
            KeymapResult::Pending => {}
            other => panic!("Expected Pending for first 'd', got {:?}", other),
        }

        let d2 = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        match state.process_key(d2) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::DeleteLine),
            other => panic!("Expected Matched(DeleteLine), got {:?}", other),
        }
    }

    #[test]
    fn vim_normal_gg_buffer_beginning() {
        let keymap = vim_normal_keymap();
        let mut state = KeymapState::new(keymap);

        let g1 = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        match state.process_key(g1) {
            KeymapResult::Pending => {}
            other => panic!("Expected Pending for first 'g', got {:?}", other),
        }

        let g2 = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        match state.process_key(g2) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::BufferBeginning),
            other => panic!("Expected Matched(BufferBeginning), got {:?}", other),
        }
    }

    #[test]
    fn vim_normal_yy_yank_line() {
        let keymap = vim_normal_keymap();
        let mut state = KeymapState::new(keymap);

        // 'y' alone is not bound (visual copy handled in app.rs), so first 'y' is Pending
        let y1 = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        match state.process_key(y1) {
            KeymapResult::Pending => {}
            other => panic!("Expected Pending for first 'y', got {:?}", other),
        }

        let y2 = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        match state.process_key(y2) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::YankLine),
            other => panic!("Expected Matched(YankLine), got {:?}", other),
        }
    }

    #[test]
    fn vim_normal_colon_command() {
        let keymap = vim_normal_keymap();
        let mut state = KeymapState::new(keymap);
        let event = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::VimCommandPrompt),
            other => panic!("Expected Matched(VimCommandPrompt), got {:?}", other),
        }
    }

    #[test]
    fn vim_normal_pane_split() {
        let keymap = vim_normal_keymap();
        let mut state = KeymapState::new(keymap);

        let cw = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        match state.process_key(cw) {
            KeymapResult::Pending => {}
            other => panic!("Expected Pending for Ctrl-w, got {:?}", other),
        }

        let s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        match state.process_key(s) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::SplitVertical),
            other => panic!("Expected Matched(SplitVertical), got {:?}", other),
        }
    }

    #[test]
    fn vim_insert_self_insert_fallthrough() {
        let keymap = vim_insert_keymap();
        let mut state = KeymapState::new(keymap);
        // Printable chars should NOT be in the keymap (handled as self-insert fallback)
        let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        match state.process_key(event) {
            KeymapResult::NotFound => {}
            other => panic!("Expected NotFound for 'a' in insert mode, got {:?}", other),
        }
    }

    #[test]
    fn vim_insert_enter() {
        let keymap = vim_insert_keymap();
        let mut state = KeymapState::new(keymap);
        let event = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::InsertNewline),
            other => panic!("Expected Matched(InsertNewline), got {:?}", other),
        }
    }

    #[test]
    fn vim_insert_backspace() {
        let keymap = vim_insert_keymap();
        let mut state = KeymapState::new(keymap);
        let event = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        match state.process_key(event) {
            KeymapResult::Matched(cmd) => assert_eq!(cmd, Command::DeleteBackward),
            other => panic!("Expected Matched(DeleteBackward), got {:?}", other),
        }
    }

    #[test]
    fn set_keymap_clears_pending() {
        let keymap = vim_normal_keymap();
        let mut state = KeymapState::new(keymap);

        // Start a dd chord
        let d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        state.process_key(d);
        assert!(state.has_pending());

        // Switching keymap clears pending
        state.set_keymap(vim_insert_keymap());
        assert!(!state.has_pending());
    }
}
