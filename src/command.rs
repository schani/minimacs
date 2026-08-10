#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    // Movement
    ForwardChar,
    BackwardChar,
    ForwardWord,
    BackwardWord,
    NextLine,
    PreviousLine,
    BeginningOfLine,
    EndOfLine,
    BufferBeginning,
    BufferEnd,
    PageDown,
    PageUp,

    // Editing
    InsertChar(char),
    InsertNewline,
    IndentLine,
    DedentLine,
    DeleteBackward,
    DeleteForward,
    DeleteWordBackward,
    KillLine,

    // Undo/Redo
    Undo,
    Redo,

    // Mark/Region
    SetMark,
    SwapPointAndMark,
    Cut,
    Copy,
    Paste,

    // Files & Buffers
    Save,
    WriteFile,
    FindFile,
    SwitchBuffer,
    KillBuffer,

    // Panes
    SplitVertical,
    SplitHorizontal,
    DeletePane,
    DeleteOtherPanes,
    CycleFocus,

    // Search
    ISearchForward,
    ISearchBackward,

    // Display
    RecenterTopBottom,

    // Help
    DescribeBindings,

    // Command dispatch
    ExecuteExtended,

    // Misc
    Cancel,
    Quit,
    GotoLine,
}

impl Command {
    /// Stable, Emacs-style name shown by generated keybinding help.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ForwardChar => "forward-char",
            Self::BackwardChar => "backward-char",
            Self::ForwardWord => "forward-word",
            Self::BackwardWord => "backward-word",
            Self::NextLine => "next-line",
            Self::PreviousLine => "previous-line",
            Self::BeginningOfLine => "beginning-of-line",
            Self::EndOfLine => "end-of-line",
            Self::BufferBeginning => "beginning-of-buffer",
            Self::BufferEnd => "end-of-buffer",
            Self::PageDown => "scroll-up-command",
            Self::PageUp => "scroll-down-command",
            Self::InsertChar(_) => "self-insert-command",
            Self::InsertNewline => "newline",
            Self::IndentLine => "indent-for-tab-command",
            Self::DedentLine => "dedent-line",
            Self::DeleteBackward => "delete-backward-char",
            Self::DeleteForward => "delete-forward-char",
            Self::DeleteWordBackward => "backward-kill-word",
            Self::KillLine => "kill-line",
            Self::Undo => "undo",
            Self::Redo => "undo-redo",
            Self::SetMark => "set-mark-command",
            Self::SwapPointAndMark => "exchange-point-and-mark",
            Self::Cut => "kill-region",
            Self::Copy => "kill-ring-save",
            Self::Paste => "yank",
            Self::Save => "save-buffer",
            Self::WriteFile => "write-file",
            Self::FindFile => "find-file",
            Self::SwitchBuffer => "switch-to-buffer",
            Self::KillBuffer => "kill-buffer",
            Self::SplitVertical => "split-window-below",
            Self::SplitHorizontal => "split-window-right",
            Self::DeletePane => "delete-window",
            Self::DeleteOtherPanes => "delete-other-windows",
            Self::CycleFocus => "other-window",
            Self::ISearchForward => "isearch-forward",
            Self::ISearchBackward => "isearch-backward",
            Self::RecenterTopBottom => "recenter-top-bottom",
            Self::DescribeBindings => "describe-bindings",
            Self::ExecuteExtended => "execute-extended-command",
            Self::Cancel => "keyboard-quit",
            Self::Quit => "save-buffers-kill-terminal",
            Self::GotoLine => "goto-line",
        }
    }

    /// Every command that can be invoked by name. Parameterized internal
    /// commands such as `InsertChar(char)` are deliberately absent.
    pub fn interactive_commands() -> &'static [Command] {
        static COMMANDS: &[Command] = &[
            Command::ForwardChar,
            Command::BackwardChar,
            Command::ForwardWord,
            Command::BackwardWord,
            Command::NextLine,
            Command::PreviousLine,
            Command::BeginningOfLine,
            Command::EndOfLine,
            Command::BufferBeginning,
            Command::BufferEnd,
            Command::PageDown,
            Command::PageUp,
            Command::InsertNewline,
            Command::IndentLine,
            Command::DedentLine,
            Command::DeleteBackward,
            Command::DeleteForward,
            Command::DeleteWordBackward,
            Command::KillLine,
            Command::Undo,
            Command::Redo,
            Command::SetMark,
            Command::SwapPointAndMark,
            Command::Cut,
            Command::Copy,
            Command::Paste,
            Command::Save,
            Command::WriteFile,
            Command::FindFile,
            Command::SwitchBuffer,
            Command::KillBuffer,
            Command::SplitVertical,
            Command::SplitHorizontal,
            Command::DeletePane,
            Command::DeleteOtherPanes,
            Command::CycleFocus,
            Command::ISearchForward,
            Command::ISearchBackward,
            Command::RecenterTopBottom,
            Command::DescribeBindings,
            Command::ExecuteExtended,
            Command::Cancel,
            Command::Quit,
            Command::GotoLine,
        ];
        COMMANDS
    }

    /// Resolve an exact interactive command name.
    pub fn from_name(name: &str) -> Option<Command> {
        Self::interactive_commands()
            .iter()
            .find(|command| command.name() == name)
            .cloned()
    }

    /// Resolve a command when the input is either exact or a unique prefix.
    pub fn from_name_or_unique_prefix(input: &str) -> Option<Command> {
        if let Some(command) = Self::from_name(input) {
            return Some(command);
        }
        let mut matches = Self::interactive_commands()
            .iter()
            .filter(|command| command.name().starts_with(input));
        let command = matches.next()?.clone();
        matches.next().is_none().then_some(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_command_names_are_unique() {
        let mut names: Vec<_> = Command::interactive_commands()
            .iter()
            .map(Command::name)
            .collect();
        let original_len = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), original_len);
    }

    #[test]
    fn command_lookup_uses_the_interactive_registry() {
        assert_eq!(Command::from_name("find-file"), Some(Command::FindFile));
        assert_eq!(Command::from_name("not-a-command"), None);
        assert!(!Command::interactive_commands()
            .iter()
            .any(|command| matches!(command, Command::InsertChar(_))));
    }
}
