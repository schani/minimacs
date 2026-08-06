//! C ABI used by the macOS AppKit frontend.
//!
//! The bridge deliberately exchanges a viewport-sized cell grid rather than
//! the whole Rope. It reuses the terminal renderer with a `TestBackend`, so
//! both frontends share wrapping, panes, prompts, mode lines, and syntax styles
//! while AppKit remains responsible for native windowing and Core Text drawing.

use std::ffi::{c_char, c_void, CStr};
use std::path::Path;
use std::ptr;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::{Backend, TestBackend};
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use crate::app::App;
use crate::command::Command;
use crate::editor::Editor;

const MOD_CONTROL: u8 = 1 << 0;
const MOD_ALT: u8 = 1 << 1;
const MOD_SHIFT: u8 = 1 << 2;

const CELL_BOLD: u8 = 1 << 0;
const CELL_ITALIC: u8 = 1 << 1;
const CELL_UNDERLINED: u8 = 1 << 2;
const CELL_REVERSED: u8 = 1 << 3;

const KEY_CHAR: u32 = 0;
const KEY_ENTER: u32 = 1;
const KEY_TAB: u32 = 2;
const KEY_BACKSPACE: u32 = 3;
const KEY_DELETE: u32 = 4;
const KEY_ESCAPE: u32 = 5;
const KEY_LEFT: u32 = 6;
const KEY_RIGHT: u32 = 7;
const KEY_UP: u32 = 8;
const KEY_DOWN: u32 = 9;
const KEY_HOME: u32 = 10;
const KEY_END: u32 = 11;
const KEY_PAGE_UP: u32 = 12;
const KEY_PAGE_DOWN: u32 = 13;

const MOUSE_CLICK: u32 = 0;
const MOUSE_SCROLL_UP: u32 = 1;
const MOUSE_SCROLL_DOWN: u32 = 2;

const COMMAND_SAVE: u32 = 0;
const COMMAND_UNDO: u32 = 1;
const COMMAND_REDO: u32 = 2;
const COMMAND_CANCEL: u32 = 3;

/// RGB color passed over the C ABI. `valid == 0` asks AppKit to use its
/// default foreground/background color.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MmColor {
    pub valid: u8,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

/// One viewport cell. The UTF-8 pointer is borrowed from the native handle and
/// remains valid until the next mutating bridge call.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MmCell {
    pub text: *const u8,
    pub text_len: usize,
    pub foreground: MmColor,
    pub background: MmColor,
    pub modifiers: u8,
}

/// A borrowed, viewport-bounded frame.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MmFrame {
    pub cells: *const MmCell,
    pub cell_count: usize,
    pub width: u16,
    pub height: u16,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub cursor_visible: u8,
}

impl Default for MmFrame {
    fn default() -> Self {
        Self {
            cells: ptr::null(),
            cell_count: 0,
            width: 0,
            height: 0,
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: 0,
        }
    }
}

struct NativeApp {
    app: App<TestBackend>,
    cell_text: Vec<Box<[u8]>>,
    cells: Vec<MmCell>,
}

impl NativeApp {
    fn new(width: u16, height: u16) -> Option<Self> {
        let backend = TestBackend::new(width.max(2), height.max(2));
        let terminal = Terminal::new(backend).ok()?;
        let mut native = Self {
            app: App::new(terminal, Editor::new()),
            cell_text: Vec::new(),
            cells: Vec::new(),
        };
        native.redraw()?;
        Some(native)
    }

    fn redraw(&mut self) -> Option<()> {
        self.app.update_viewport();
        self.app.render().ok()?;
        self.refresh_snapshot();
        Some(())
    }

    fn process_event(&mut self, event: Event) -> bool {
        let state_changed = self.app.dispatch_event(event);
        let syntax_changed = self.app.apply_syntax_completions();
        if state_changed || syntax_changed {
            self.redraw().is_some()
        } else {
            false
        }
    }

    fn resize(&mut self, width: u16, height: u16) -> bool {
        let width = width.max(2);
        let height = height.max(2);
        self.app.terminal.backend_mut().resize(width, height);
        self.redraw().is_some()
    }

    fn poll(&mut self) -> bool {
        if self.app.apply_syntax_completions() {
            self.redraw().is_some()
        } else {
            false
        }
    }

    fn open_file(&mut self, path: &Path) -> bool {
        if self.app.editor.open_file(path).is_err() {
            return false;
        }
        self.redraw().is_some()
    }

    fn execute(&mut self, command: Command) -> bool {
        self.app.editor.execute(command);
        self.redraw().is_some()
    }

    fn refresh_snapshot(&mut self) {
        let backend = self.app.terminal.backend();
        let source = &backend.buffer().content;
        self.cell_text = source
            .iter()
            .map(|cell| cell.symbol().as_bytes().to_vec().into_boxed_slice())
            .collect();
        self.cells = source
            .iter()
            .zip(&self.cell_text)
            .map(|(cell, text)| MmCell {
                text: text.as_ptr(),
                text_len: text.len(),
                foreground: color_to_rgb(cell.fg),
                background: color_to_rgb(cell.bg),
                modifiers: modifier_bits(cell.modifier),
            })
            .collect();
    }

    fn frame(&mut self) -> MmFrame {
        let area = self.app.terminal.backend().buffer().area;
        let cursor = self
            .app
            .terminal
            .backend_mut()
            .get_cursor_position()
            .unwrap_or_default();
        MmFrame {
            cells: self.cells.as_ptr(),
            cell_count: self.cells.len(),
            width: area.width,
            height: area.height,
            cursor_x: cursor.x,
            cursor_y: cursor.y,
            cursor_visible: 1,
        }
    }
}

fn modifier_bits(modifier: Modifier) -> u8 {
    let mut bits = 0;
    if modifier.contains(Modifier::BOLD) {
        bits |= CELL_BOLD;
    }
    if modifier.contains(Modifier::ITALIC) {
        bits |= CELL_ITALIC;
    }
    if modifier.contains(Modifier::UNDERLINED) {
        bits |= CELL_UNDERLINED;
    }
    if modifier.contains(Modifier::REVERSED) {
        bits |= CELL_REVERSED;
    }
    bits
}

fn color_to_rgb(color: Color) -> MmColor {
    let rgb = match color {
        Color::Reset => return MmColor::default(),
        Color::Black => (0, 0, 0),
        Color::Red => (128, 0, 0),
        Color::Green => (0, 128, 0),
        Color::Yellow => (128, 128, 0),
        Color::Blue => (0, 0, 128),
        Color::Magenta => (128, 0, 128),
        Color::Cyan => (0, 128, 128),
        Color::Gray => (192, 192, 192),
        Color::DarkGray => (128, 128, 128),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (0, 0, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        Color::Rgb(red, green, blue) => (red, green, blue),
        Color::Indexed(index) => indexed_color(index),
    };
    MmColor {
        valid: 1,
        red: rgb.0,
        green: rgb.1,
        blue: rgb.2,
    }
}

fn indexed_color(index: u8) -> (u8, u8, u8) {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let value = index - 16;
            let component = |part: u8| if part == 0 { 0 } else { 55 + 40 * part };
            (
                component(value / 36),
                component((value % 36) / 6),
                component(value % 6),
            )
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn key_code(code: u32, scalar: u32) -> Option<KeyCode> {
    match code {
        KEY_CHAR => char::from_u32(scalar).map(KeyCode::Char),
        KEY_ENTER => Some(KeyCode::Enter),
        KEY_TAB => Some(KeyCode::Tab),
        KEY_BACKSPACE => Some(KeyCode::Backspace),
        KEY_DELETE => Some(KeyCode::Delete),
        KEY_ESCAPE => Some(KeyCode::Esc),
        KEY_LEFT => Some(KeyCode::Left),
        KEY_RIGHT => Some(KeyCode::Right),
        KEY_UP => Some(KeyCode::Up),
        KEY_DOWN => Some(KeyCode::Down),
        KEY_HOME => Some(KeyCode::Home),
        KEY_END => Some(KeyCode::End),
        KEY_PAGE_UP => Some(KeyCode::PageUp),
        KEY_PAGE_DOWN => Some(KeyCode::PageDown),
        _ => None,
    }
}

fn key_modifiers(bits: u8) -> KeyModifiers {
    let mut modifiers = KeyModifiers::NONE;
    if bits & MOD_CONTROL != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    if bits & MOD_ALT != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if bits & MOD_SHIFT != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    modifiers
}

unsafe fn handle_mut<'a>(handle: *mut c_void) -> Option<&'a mut NativeApp> {
    (handle as *mut NativeApp).as_mut()
}

/// Create a native editor handle. The initial render is synchronous and does
/// not start the syntax worker for the scratch buffer.
#[no_mangle]
pub extern "C" fn minimacs_native_new(width: u16, height: u16) -> *mut c_void {
    NativeApp::new(width, height)
        .map(|app| Box::into_raw(Box::new(app)).cast())
        .unwrap_or(ptr::null_mut())
}

/// Destroy a handle returned by [`minimacs_native_new`].
#[no_mangle]
pub unsafe extern "C" fn minimacs_native_free(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle as *mut NativeApp));
    }
}

/// Return the current borrowed frame. It remains valid until the next bridge
/// call that can redraw the editor.
#[no_mangle]
pub unsafe extern "C" fn minimacs_native_frame(handle: *mut c_void) -> MmFrame {
    handle_mut(handle).map_or_else(MmFrame::default, |app| app.frame())
}

#[no_mangle]
pub unsafe extern "C" fn minimacs_native_resize(
    handle: *mut c_void,
    width: u16,
    height: u16,
) -> bool {
    handle_mut(handle).is_some_and(|app| app.resize(width, height))
}

#[no_mangle]
pub unsafe extern "C" fn minimacs_native_key(
    handle: *mut c_void,
    code: u32,
    scalar: u32,
    modifiers: u8,
) -> bool {
    let Some(app) = handle_mut(handle) else {
        return false;
    };
    let Some(code) = key_code(code, scalar) else {
        return false;
    };
    app.process_event(Event::Key(KeyEvent::new(code, key_modifiers(modifiers))))
}

/// Insert UTF-8 text as one paste/undo group. This is also the IME commit path.
#[no_mangle]
pub unsafe extern "C" fn minimacs_native_insert_utf8(
    handle: *mut c_void,
    text: *const c_char,
) -> bool {
    let Some(app) = handle_mut(handle) else {
        return false;
    };
    if text.is_null() {
        return false;
    }
    let Ok(text) = CStr::from_ptr(text).to_str() else {
        return false;
    };
    app.process_event(Event::Paste(text.to_string()))
}

#[no_mangle]
pub unsafe extern "C" fn minimacs_native_mouse(
    handle: *mut c_void,
    kind: u32,
    column: u16,
    row: u16,
) -> bool {
    let Some(app) = handle_mut(handle) else {
        return false;
    };
    let kind = match kind {
        MOUSE_CLICK => MouseEventKind::Down(MouseButton::Left),
        MOUSE_SCROLL_UP => MouseEventKind::ScrollUp,
        MOUSE_SCROLL_DOWN => MouseEventKind::ScrollDown,
        _ => return false,
    };
    app.process_event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn minimacs_native_open_file(
    handle: *mut c_void,
    path: *const c_char,
) -> bool {
    let Some(app) = handle_mut(handle) else {
        return false;
    };
    if path.is_null() {
        return false;
    }
    let Ok(path) = CStr::from_ptr(path).to_str() else {
        return false;
    };
    app.open_file(Path::new(path))
}

#[no_mangle]
pub unsafe extern "C" fn minimacs_native_command(handle: *mut c_void, command: u32) -> bool {
    let Some(app) = handle_mut(handle) else {
        return false;
    };
    let command = match command {
        COMMAND_SAVE => Command::Save,
        COMMAND_UNDO => Command::Undo,
        COMMAND_REDO => Command::Redo,
        COMMAND_CANCEL => Command::Cancel,
        _ => return false,
    };
    app.execute(command)
}

/// Poll for a background syntax completion. This never redraws unless a new
/// completion was accepted.
#[no_mangle]
pub unsafe extern "C" fn minimacs_native_poll(handle: *mut c_void) -> bool {
    handle_mut(handle).is_some_and(NativeApp::poll)
}

#[no_mangle]
pub unsafe extern "C" fn minimacs_native_should_quit(handle: *mut c_void) -> bool {
    handle_mut(handle).is_some_and(|app| app.app.editor.should_quit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn screen_text(frame: MmFrame) -> String {
        if frame.cells.is_null() {
            return String::new();
        }
        let cells = unsafe { std::slice::from_raw_parts(frame.cells, frame.cell_count) };
        cells
            .iter()
            .map(|cell| {
                let bytes = unsafe { std::slice::from_raw_parts(cell.text, cell.text_len) };
                std::str::from_utf8(bytes).unwrap()
            })
            .collect()
    }

    #[test]
    fn bridge_renders_and_edits_through_the_shared_keymap() {
        let handle = minimacs_native_new(30, 6);
        assert!(!handle.is_null());
        let initial = unsafe { minimacs_native_frame(handle) };
        assert_eq!((initial.width, initial.height), (30, 6));
        assert_eq!(initial.cell_count, 180);
        assert!(screen_text(initial).contains("*scratch*"));

        assert!(unsafe { minimacs_native_key(handle, KEY_CHAR, 'h' as u32, 0) });
        assert!(unsafe { minimacs_native_key(handle, KEY_CHAR, 'i' as u32, 0) });
        assert!(screen_text(unsafe { minimacs_native_frame(handle) }).starts_with("hi"));

        // C-b uses the same keymap/input state as the terminal frontend.
        assert!(unsafe { minimacs_native_key(handle, KEY_CHAR, 'b' as u32, MOD_CONTROL) });
        let moved = unsafe { minimacs_native_frame(handle) };
        assert_eq!((moved.cursor_x, moved.cursor_y), (1, 0));
        unsafe { minimacs_native_free(handle) };
    }

    #[test]
    fn bridge_resizes_inserts_utf8_and_runs_commands() {
        let handle = minimacs_native_new(10, 3);
        let text = CString::new("hé\n").unwrap();
        assert!(unsafe { minimacs_native_insert_utf8(handle, text.as_ptr()) });
        assert!(screen_text(unsafe { minimacs_native_frame(handle) }).contains("hé"));
        assert!(unsafe { minimacs_native_command(handle, COMMAND_UNDO) });
        assert!(!screen_text(unsafe { minimacs_native_frame(handle) }).contains("hé"));
        assert!(unsafe { minimacs_native_command(handle, COMMAND_REDO) });
        assert!(unsafe { minimacs_native_command(handle, COMMAND_CANCEL) });
        assert!(unsafe { minimacs_native_resize(handle, 24, 8) });
        let frame = unsafe { minimacs_native_frame(handle) };
        assert_eq!((frame.width, frame.height), (24, 8));
        assert!(!unsafe { minimacs_native_poll(handle) });
        assert!(!unsafe { minimacs_native_should_quit(handle) });
        unsafe { minimacs_native_free(handle) };
    }

    #[test]
    fn bridge_opens_clicks_scrolls_and_saves_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let path = CString::new(path.to_str().unwrap()).unwrap();
        let handle = minimacs_native_new(20, 5);
        assert!(unsafe { minimacs_native_open_file(handle, path.as_ptr()) });
        assert!(screen_text(unsafe { minimacs_native_frame(handle) }).contains("one"));
        assert!(unsafe { minimacs_native_mouse(handle, MOUSE_CLICK, 1, 1) });
        assert!(unsafe { minimacs_native_mouse(handle, MOUSE_SCROLL_DOWN, 1, 1) });
        assert!(unsafe { minimacs_native_mouse(handle, MOUSE_SCROLL_UP, 1, 1) });
        assert!(unsafe { minimacs_native_command(handle, COMMAND_SAVE) });
        unsafe { minimacs_native_free(handle) };
    }

    #[test]
    fn bridge_rejects_null_invalid_and_non_utf8_inputs() {
        let invalid = [0xff_u8, 0];
        assert!(unsafe { minimacs_native_frame(ptr::null_mut()) }
            .cells
            .is_null());
        assert!(!unsafe { minimacs_native_resize(ptr::null_mut(), 1, 1) });
        assert!(!unsafe { minimacs_native_key(ptr::null_mut(), KEY_CHAR, 0, 0) });
        assert!(!unsafe { minimacs_native_insert_utf8(ptr::null_mut(), ptr::null()) });
        assert!(!unsafe { minimacs_native_mouse(ptr::null_mut(), MOUSE_CLICK, 0, 0) });
        assert!(!unsafe { minimacs_native_open_file(ptr::null_mut(), ptr::null()) });
        assert!(!unsafe { minimacs_native_command(ptr::null_mut(), COMMAND_SAVE) });
        assert!(!unsafe { minimacs_native_poll(ptr::null_mut()) });
        assert!(!unsafe { minimacs_native_should_quit(ptr::null_mut()) });
        unsafe { minimacs_native_free(ptr::null_mut()) };

        let handle = minimacs_native_new(4, 2);
        assert!(!unsafe { minimacs_native_key(handle, 999, 0, 0) });
        assert!(!unsafe { minimacs_native_key(handle, KEY_CHAR, u32::MAX, 0) });
        assert!(!unsafe { minimacs_native_mouse(handle, 999, 0, 0) });
        assert!(!unsafe { minimacs_native_command(handle, 999) });
        assert!(!unsafe { minimacs_native_insert_utf8(handle, ptr::null()) });
        assert!(!unsafe { minimacs_native_open_file(handle, ptr::null()) });
        assert!(!unsafe { minimacs_native_insert_utf8(handle, invalid.as_ptr().cast()) });
        assert!(!unsafe { minimacs_native_open_file(handle, invalid.as_ptr().cast()) });
        unsafe { minimacs_native_free(handle) };
    }

    #[test]
    fn color_and_modifier_conversion_covers_terminal_palette() {
        assert_eq!(color_to_rgb(Color::Reset), MmColor::default());
        assert_eq!(color_to_rgb(Color::Rgb(1, 2, 3)).blue, 3);
        assert_eq!(color_to_rgb(Color::Indexed(16)).red, 0);
        assert_eq!(color_to_rgb(Color::Indexed(231)).red, 255);
        assert_eq!(color_to_rgb(Color::Indexed(232)).red, 8);
        assert_eq!(color_to_rgb(Color::Indexed(255)).red, 238);
        for color in [
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ] {
            assert_eq!(color_to_rgb(color).valid, 1);
        }
        let all = Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED | Modifier::REVERSED;
        assert_eq!(
            modifier_bits(all),
            CELL_BOLD | CELL_ITALIC | CELL_UNDERLINED | CELL_REVERSED
        );
    }

    #[test]
    fn every_native_key_code_maps() {
        for code in KEY_ENTER..=KEY_PAGE_DOWN {
            assert!(key_code(code, 0).is_some(), "code {code}");
        }
        assert_eq!(key_code(KEY_CHAR, 'x' as u32), Some(KeyCode::Char('x')));
        let modifiers = key_modifiers(MOD_CONTROL | MOD_ALT | MOD_SHIFT);
        assert!(modifiers.contains(KeyModifiers::CONTROL));
        assert!(modifiers.contains(KeyModifiers::ALT));
        assert!(modifiers.contains(KeyModifiers::SHIFT));
    }
}
