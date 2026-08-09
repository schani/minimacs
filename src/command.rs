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
            Self::Cancel => "keyboard-quit",
            Self::Quit => "save-buffers-kill-terminal",
            Self::GotoLine => "goto-line",
        }
    }
}
