#ifndef MINIMACS_NATIVE_H
#define MINIMACS_NATIVE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    uint8_t valid;
    uint8_t red;
    uint8_t green;
    uint8_t blue;
} MmColor;

typedef struct {
    const uint8_t *text;
    size_t text_len;
    MmColor foreground;
    MmColor background;
    uint8_t modifiers;
} MmCell;

typedef struct {
    const MmCell *cells;
    size_t cell_count;
    uint16_t width;
    uint16_t height;
    uint16_t cursor_x;
    uint16_t cursor_y;
    uint8_t cursor_visible;
} MmFrame;

enum MmModifier {
    MM_MOD_CONTROL = 1 << 0,
    MM_MOD_ALT = 1 << 1,
    MM_MOD_SHIFT = 1 << 2,
};

enum MmCellModifier {
    MM_CELL_BOLD = 1 << 0,
    MM_CELL_ITALIC = 1 << 1,
    MM_CELL_UNDERLINED = 1 << 2,
    MM_CELL_REVERSED = 1 << 3,
};

enum MmKeyCode {
    MM_KEY_CHAR = 0,
    MM_KEY_ENTER = 1,
    MM_KEY_TAB = 2,
    MM_KEY_BACKSPACE = 3,
    MM_KEY_DELETE = 4,
    MM_KEY_ESCAPE = 5,
    MM_KEY_LEFT = 6,
    MM_KEY_RIGHT = 7,
    MM_KEY_UP = 8,
    MM_KEY_DOWN = 9,
    MM_KEY_HOME = 10,
    MM_KEY_END = 11,
    MM_KEY_PAGE_UP = 12,
    MM_KEY_PAGE_DOWN = 13,
};

enum MmMouseKind {
    MM_MOUSE_CLICK = 0,
    MM_MOUSE_SCROLL_UP = 1,
    MM_MOUSE_SCROLL_DOWN = 2,
};

enum MmCommand {
    MM_COMMAND_SAVE = 0,
    MM_COMMAND_UNDO = 1,
    MM_COMMAND_REDO = 2,
    MM_COMMAND_CANCEL = 3,
};

void *minimacs_native_new(uint16_t width, uint16_t height);
void minimacs_native_free(void *handle);
MmFrame minimacs_native_frame(void *handle);
bool minimacs_native_resize(void *handle, uint16_t width, uint16_t height);
bool minimacs_native_key(void *handle, uint32_t code, uint32_t scalar, uint8_t modifiers);
bool minimacs_native_insert_utf8(void *handle, const char *text);
bool minimacs_native_mouse(void *handle, uint32_t kind, uint16_t column, uint16_t row);
bool minimacs_native_open_file(void *handle, const char *path);
bool minimacs_native_command(void *handle, uint32_t command);
bool minimacs_native_poll(void *handle);
bool minimacs_native_has_background_work(void *handle);
bool minimacs_native_should_quit(void *handle);

#ifdef __cplusplus
}
#endif

#endif
