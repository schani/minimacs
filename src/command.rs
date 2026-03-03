#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
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
    InsertTab,
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

    // Misc
    Cancel,
    Quit,
    GotoLine,
}
