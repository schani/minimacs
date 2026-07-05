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

    // Misc
    Cancel,
    Quit,
    GotoLine,
}
