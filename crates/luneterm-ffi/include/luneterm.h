/*
 * luneterm.h - C ABI of the Luneterm VT engine (crate luneterm-ffi), ABI version 1.
 *
 * It mirrors Lunegit's IVtEngine over lune_vt::Terminal: lifecycle, write, resize, scroll, key /
 * char / mouse / focus / paste encoders, viewport read, event drain, damage and modes.
 *
 * Building the library
 *   The shipping library is built ONLY with
 *       cargo build -p luneterm-ffi --profile ffi-release
 *   (release, panic = "unwind"). Every export catches panics; a build with panic = "abort"
 *   (plain `cargo build --release`) is a compile error. The library never installs a panic hook.
 *   64-bit targets only.
 *
 * ABI version policy
 *   LUNETERM_ABI_VERSION is returned by luneterm_abi_version(); a host refuses a library whose
 *   value differs from the one it was written against. It is bumped on any change to an existing
 *   export's signature or meaning, to a struct layout, to a constant's value, or on a removal.
 *   Adding an export does not bump it: the host resolves symbols at load time.
 *
 * Status codes
 *   Every function returns an int32_t status. LUNETERM_OK (0) is success; errors are negative.
 *   Out-parameters are checked (NULL -> LUNETERM_ERR_NULL_POINTER, misaligned ->
 *   LUNETERM_ERR_INVALID_ARGUMENT) and set to zero/NULL before anything else, and they are filled
 *   only when the function returns LUNETERM_OK. A rejected argument never changes the engine and
 *   never poisons the handle.
 *
 * Threading
 *   A handle may be used from any thread, but one call at a time. A call made while another is in
 *   progress on the same handle returns LUNETERM_ERR_BUSY without waiting. Do not call
 *   luneterm_engine_free while another thread may start a call on the same handle.
 *
 * Panics
 *   A panic inside a call is caught: the call returns LUNETERM_ERR_PANIC and the handle is
 *   poisoned. From then on every call except luneterm_engine_free returns LUNETERM_ERR_POISONED.
 *
 * Ownership of returned memory
 *   The library owns every pointer it returns (the viewport, event and input-bytes "slots"). A slot
 *   stays valid and unchanged until the next call that fills the same slot, or until
 *   luneterm_engine_free:
 *       viewport slot - luneterm_engine_viewport
 *       events slot   - luneterm_engine_drain_events
 *       input slot    - luneterm_engine_key_event / _key_input / _char_input / _mouse_event /
 *                       _focus_event / _paste
 *   Every other call (write, resize, scroll, the getters of other kinds) leaves the slots alone.
 *   A call that reaches a slot and fails with LUNETERM_ERR_TOO_LARGE leaves it unspecified (its
 *   out-parameter is zeroed). Strings are pointer + length in bytes, never NUL-terminated, and may
 *   contain NUL. A pointer to an empty array or string is NULL. Everything that came from the
 *   terminal's child process (titles, uris, text, payloads) is untrusted data.
 *
 * Inputs
 *   (NULL, 0) is a valid empty byte range; (NULL, n > 0) is LUNETERM_ERR_NULL_POINTER; a length
 *   above PTRDIFF_MAX is LUNETERM_ERR_INVALID_ARGUMENT. Enums are plain integers, range-checked.
 *   Booleans are uint8_t: nonzero is true.
 */
#ifndef LUNETERM_H
#define LUNETERM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifdef __cplusplus
#define LUNETERM_STATIC_ASSERT(cond, msg) static_assert(cond, msg)
#define LUNETERM_ALIGNOF(type) alignof(type)
#else
#define LUNETERM_STATIC_ASSERT(cond, msg) _Static_assert(cond, msg)
#define LUNETERM_ALIGNOF(type) _Alignof(type)
#endif

/* ---- Version ------------------------------------------------------------------------------ */

#define LUNETERM_ABI_VERSION 1u

/* The ABI version the loaded library was built with. */
uint32_t luneterm_abi_version(void);

/* ---- Status codes ------------------------------------------------------------------------- */

#define LUNETERM_OK 0
/* A required pointer (the handle, an out-parameter, input data with a nonzero length) is NULL. */
#define LUNETERM_ERR_NULL_POINTER (-1)
/* An enum out of range, a size over a cap, a misaligned pointer, a length above PTRDIFF_MAX, or an
 * XTVERSION name the engine refuses. */
#define LUNETERM_ERR_INVALID_ARGUMENT (-2)
/* Text input that must be UTF-8 is not. */
#define LUNETERM_ERR_INVALID_UTF8 (-3)
/* A result does not fit the ABI's integer type (a viewport text pool beyond 4 GiB). */
#define LUNETERM_ERR_TOO_LARGE (-4)
/* A panic was caught inside the call; the handle is now poisoned. */
#define LUNETERM_ERR_PANIC (-5)
/* An earlier call panicked; only luneterm_engine_free still works on this handle. */
#define LUNETERM_ERR_POISONED (-6)
/* Another call is in progress on this handle. */
#define LUNETERM_ERR_BUSY (-7)

/* ---- Caps --------------------------------------------------------------------------------- */

/* Host-supplied sizes over a cap are rejected (LUNETERM_ERR_INVALID_ARGUMENT), never clamped.
 * Below 2 columns or 1 row they are clamped to 2 x 1, as the engine does. */
#define LUNETERM_MAX_COLS 4096u
#define LUNETERM_MAX_ROWS 2048u
#define LUNETERM_MAX_SCROLLBACK 100000u

/* ---- Engine handle and configuration ------------------------------------------------------ */

/* Opaque engine handle: luneterm_engine_new creates it, luneterm_engine_free releases it. */
typedef struct LunetermEngine LunetermEngine;

/* LunetermConfig.unicode */
#define LUNETERM_UNICODE_6 0u       /* Unicode 6 widths (the default) */
#define LUNETERM_UNICODE_11 1u      /* Unicode 11 widths */
#define LUNETERM_UNICODE_11_VS16 2u /* Unicode 11, U+FE0F after a width-1 character makes 2 cells */

/* Construction parameters. All zero is the safe default: 2 x 1, no scrollback, Unicode 6, XTVERSION
 * unanswered, clipboard writes off. */
typedef struct LunetermConfig {
    uint32_t cols;       /* below 2: 2; over LUNETERM_MAX_COLS: rejected */
    uint32_t rows;       /* below 1: 1; over LUNETERM_MAX_ROWS: rejected */
    uint32_t scrollback; /* lines; over LUNETERM_MAX_SCROLLBACK: rejected */
    uint32_t unicode;    /* LUNETERM_UNICODE_*; anything else: rejected */
    /* The name XTVERSION (CSI > q) answers with, UTF-8, not NUL-terminated, given to the engine as
     * is: 1 to 64 bytes, each 0x20..0x7E (anything else: LUNETERM_ERR_INVALID_ARGUMENT, or
     * LUNETERM_ERR_INVALID_UTF8 when not UTF-8). xtversion_len 0 (whatever the pointer, so (NULL, 0)
     * and (p, 0) alike) means the query is not answered. */
    const uint8_t* xtversion;
    uint64_t xtversion_len;
    uint8_t clipboard_write; /* initial OSC 52 clipboard-write gate; nonzero = on */
    uint8_t reserved[7];     /* ignored */
} LunetermConfig;

/* Creates an engine and stores the handle in *out (NULL on every error). The standard extensions
 * (Lunegit's terminal additions) are always installed; no logger is installed. */
int32_t luneterm_engine_new(const LunetermConfig* config, LunetermEngine** out);

/* Frees an engine. NULL is OK. A handle with a call in progress is LUNETERM_ERR_BUSY and is not
 * freed; a poisoned handle is freed. After LUNETERM_OK (or LUNETERM_ERR_PANIC) the handle and every
 * pointer from its slots are dead. */
int32_t luneterm_engine_free(LunetermEngine* engine);

/* ---- Control ------------------------------------------------------------------------------ */

/* Feeds PTY output to the engine: any bytes; split UTF-8 and split escape sequences are fine.
 * Events queue until drained: drain after every write. */
int32_t luneterm_engine_write(LunetermEngine* engine, const uint8_t* data, size_t len);

/* Resizes the grid (caps as above; over a cap changes nothing). */
int32_t luneterm_engine_resize(LunetermEngine* engine, uint32_t cols, uint32_t rows);

/* The current grid size. */
int32_t luneterm_engine_size(LunetermEngine* engine, uint32_t* cols, uint32_t* rows);

/* Scrolls the viewport by delta lines (negative = up), or to the top / the bottom. */
int32_t luneterm_engine_scroll_lines(LunetermEngine* engine, int32_t delta);
int32_t luneterm_engine_scroll_to_top(LunetermEngine* engine);
int32_t luneterm_engine_scroll_to_bottom(LunetermEngine* engine);

/* Runs one batch of deferred memory cleanup; *more is 1 while work remains (call again when the
 * host is idle). */
int32_t luneterm_engine_idle(LunetermEngine* engine, uint8_t* more);

/* Turns the OSC 52 clipboard-write gate on (nonzero) or off. */
int32_t luneterm_engine_set_clipboard_write(LunetermEngine* engine, uint8_t enabled);

/* Clears the kitty keyboard stacks and the modifyOtherKeys level (Lunegit ledger row 11). */
int32_t luneterm_engine_reset_keyboard_protocol(LunetermEngine* engine);

/* ---- Viewport ----------------------------------------------------------------------------- */

/* LunetermCell.fg / bg / underline_color are packed: bits 24-31 hold the kind, the rest depend on
 * it. DEFAULT: the theme's default (for an underline: follow the foreground), low bits 0. PALETTE:
 * the index is in bits 0-7. RGB: 0xRRGGBB in bits 0-23. */
#define LUNETERM_COLOR_DEFAULT 0u
#define LUNETERM_COLOR_PALETTE 1u
#define LUNETERM_COLOR_RGB 2u

/* LunetermCell.flags bits */
#define LUNETERM_CELL_BOLD 0x0001u
#define LUNETERM_CELL_DIM 0x0002u
#define LUNETERM_CELL_ITALIC 0x0004u
#define LUNETERM_CELL_BLINK 0x0008u
#define LUNETERM_CELL_INVERSE 0x0010u
#define LUNETERM_CELL_INVISIBLE 0x0020u
#define LUNETERM_CELL_STRIKETHROUGH 0x0040u
#define LUNETERM_CELL_OVERLINE 0x0080u

/* LunetermCell.underline */
#define LUNETERM_UNDERLINE_NONE 0u
#define LUNETERM_UNDERLINE_SINGLE 1u
#define LUNETERM_UNDERLINE_DOUBLE 2u
#define LUNETERM_UNDERLINE_CURLY 3u
#define LUNETERM_UNDERLINE_DOTTED 4u
#define LUNETERM_UNDERLINE_DASHED 5u

/* LunetermViewport.cursor_style */
#define LUNETERM_CURSOR_BLOCK 0u
#define LUNETERM_CURSOR_UNDERLINE 1u
#define LUNETERM_CURSOR_BAR 2u

typedef struct LunetermCell {
    uint32_t text_offset; /* into LunetermViewport.text; 0 for an empty text */
    uint32_t text_len;    /* bytes; 0 for an empty cell and for the second half of a wide character */
    uint32_t fg;          /* packed colour */
    uint32_t bg;          /* packed colour */
    uint32_t underline_color; /* packed colour */
    uint32_t link_id;     /* OSC 8 link id, 0 = none; the uri is in LunetermViewport.links */
    uint16_t flags;       /* LUNETERM_CELL_* */
    uint8_t width;        /* 1 normal, 2 first half of a wide character, 0 its second half */
    uint8_t underline;    /* LUNETERM_UNDERLINE_* */
} LunetermCell;

typedef struct LunetermLink {
    uint32_t id;
    uint32_t reserved; /* zero */
    uint64_t offset;   /* of the uri, into LunetermViewport.link_data */
    uint64_t len;      /* bytes */
} LunetermLink;

/* A snapshot of the screen. Every pointer points into the viewport slot. */
typedef struct LunetermViewport {
    uint32_t cols;
    uint32_t rows;
    const LunetermCell* cells; /* cell_count cells, row-major */
    uint64_t cell_count;       /* cols * rows */
    const uint8_t* text;       /* cell text pool, UTF-8 */
    uint64_t text_len;
    const LunetermLink* links; /* the links the cells use, ascending by id */
    uint64_t link_count;
    const uint8_t* link_data;  /* uri pool, UTF-8 */
    uint64_t link_data_len;
    const uint8_t* title;      /* window title, UTF-8, may contain NUL */
    uint64_t title_len;
    uint64_t viewport_top; /* first buffer line shown */
    uint64_t base_y;       /* first line of the bottom page */
    uint64_t total_lines;  /* lines in the active buffer, scrollback included */
    uint32_t cursor_x;     /* 0..=cols+1: cols while a wrap is pending, cols+1 after a zero-width
                              character printed there (as @xterm/headless 6.0.0); not clamped, so
                              clamp before indexing cells */
    uint32_t cursor_y;     /* viewport row */
    uint8_t cursor_visible;
    uint8_t cursor_style; /* LUNETERM_CURSOR_* */
    uint8_t cursor_blink;
    uint8_t is_at_bottom;
    uint8_t alt_screen;
    uint8_t reserved[3]; /* zero */
} LunetermViewport;

/* Snapshots the screen into the viewport slot. LUNETERM_ERR_TOO_LARGE: the cell text pool does not
 * fit uint32_t offsets. */
int32_t luneterm_engine_viewport(LunetermEngine* engine, LunetermViewport* out);

/* ---- Events ------------------------------------------------------------------------------- */

/* LunetermEvent.kind. Events that carry bytes point into LunetermEvents.data with offset/len. */
#define LUNETERM_EVENT_RESPONSE 1u /* data: bytes to send back to the application */
#define LUNETERM_EVENT_BELL 2u
#define LUNETERM_EVENT_TITLE_CHANGED 3u /* no data: read the title from the viewport */
#define LUNETERM_EVENT_COLOR 4u /* offset/len index LunetermEvents.colors */
#define LUNETERM_EVENT_FOCUS_REPORT_REQUESTED 5u
/* Reserved and never emitted through this ABI: the engine's window operations stay off. */
/* value: 0 window size in pixels, 1 cell size in pixels. The engine's window options are off and
 * this ABI has no way to turn them on, so ABI 1 never emits this event. */
#define LUNETERM_EVENT_WINDOW_REPORT 6u
#define LUNETERM_EVENT_A11Y_TEXT 7u /* data (only with screen-reader mode, which this ABI never enables) */
#define LUNETERM_EVENT_A11Y_TAB 8u  /* value: cells, saturated to INT32_MAX (same) */
#define LUNETERM_EVENT_UNKNOWN_OSC 9u /* value: ident; len: payload length; no data */
#define LUNETERM_EVENT_CLIPBOARD_WRITE 10u /* data: UTF-8 text (only while the gate is on) */
#define LUNETERM_EVENT_CWD_CHANGED 11u     /* data: UTF-8 */
#define LUNETERM_EVENT_PROGRESS 12u /* value: 0 none, 1 normal, 2 error, 3 indeterminate, 4 warning;
                                       value2: percent */
#define LUNETERM_EVENT_COMMAND_MARK 13u /* value: 0 A, 1 B, 2 C, 3 D; value2: exit code when flags has
                                           LUNETERM_EVENT_HAS_EXIT_CODE */
#define LUNETERM_EVENT_SYNC_UPDATE_IN_COMMAND 14u

/* LunetermEvent.flags */
#define LUNETERM_EVENT_HAS_EXIT_CODE 1u

typedef struct LunetermEvent {
    uint32_t kind; /* LUNETERM_EVENT_* */
    int32_t value;
    int32_t value2;
    uint32_t flags;
    uint64_t offset; /* into data (or the first request in colors) */
    uint64_t len;    /* bytes (or the number of requests) */
} LunetermEvent;

/* LunetermColorRequest.kind */
#define LUNETERM_COLOR_REQUEST_REPORT 0u      /* report the current colour */
#define LUNETERM_COLOR_REQUEST_SET 1u         /* set it to r, g, b */
#define LUNETERM_COLOR_REQUEST_RESTORE 2u     /* restore the themed colour */
#define LUNETERM_COLOR_REQUEST_RESTORE_ALL 3u /* restore the whole palette; index is 0 */

/* LunetermColorRequest.index: 0-255 is a palette entry */
#define LUNETERM_COLOR_INDEX_FOREGROUND 256u
#define LUNETERM_COLOR_INDEX_BACKGROUND 257u
#define LUNETERM_COLOR_INDEX_CURSOR 258u

typedef struct LunetermColorRequest {
    uint32_t kind;
    uint32_t index;
    uint8_t r, g, b;
    uint8_t reserved; /* zero */
} LunetermColorRequest;

/* The drained events, in order. Every pointer points into the events slot. */
typedef struct LunetermEvents {
    const LunetermEvent* events;
    uint64_t count;
    const uint8_t* data; /* byte payload pool */
    uint64_t data_len;
    const LunetermColorRequest* colors; /* request pool of the COLOR events */
    uint64_t color_count;
} LunetermEvents;

/* Drains the event queue into the events slot. An empty queue is count 0 with NULL pointers. */
int32_t luneterm_engine_drain_events(LunetermEngine* engine, LunetermEvents* out);

/* ---- Damage and modes --------------------------------------------------------------------- */

typedef struct LunetermDamage {
    uint8_t has_rows; /* nonzero when first_row..last_row is set */
    uint8_t full;     /* everything must be redrawn */
    uint8_t cursor_moved;
    uint8_t scrolled;
    uint32_t first_row; /* viewport rows to redraw, inclusive; 0 when has_rows is 0 */
    uint32_t last_row;
} LunetermDamage;

/* Takes the damage accumulated since the last call. */
int32_t luneterm_engine_take_damage(LunetermEngine* engine, LunetermDamage* out);

/* LunetermModes.mouse_tracking */
#define LUNETERM_MOUSE_TRACKING_NONE 0u
#define LUNETERM_MOUSE_TRACKING_X10 1u
#define LUNETERM_MOUSE_TRACKING_VT200 2u
#define LUNETERM_MOUSE_TRACKING_DRAG 3u
#define LUNETERM_MOUSE_TRACKING_ANY 4u

/* LunetermModes.mouse_encoding */
#define LUNETERM_MOUSE_ENCODING_DEFAULT 0u
#define LUNETERM_MOUSE_ENCODING_SGR 1u
#define LUNETERM_MOUSE_ENCODING_SGR_PIXELS 2u

/* Terminal modes the host needs for input encoding. Booleans are 0 or 1. */
typedef struct LunetermModes {
    uint8_t application_cursor_keys;
    uint8_t application_keypad;
    uint8_t bracketed_paste;
    uint8_t insert;
    uint8_t origin;
    uint8_t reverse_wraparound;
    uint8_t send_focus;
    uint8_t synchronized_output;
    uint8_t wraparound;
    uint8_t cursor_hidden;
    uint8_t mouse_tracking; /* LUNETERM_MOUSE_TRACKING_* */
    uint8_t mouse_encoding; /* LUNETERM_MOUSE_ENCODING_* */
    uint8_t kitty_keyboard_flags;
    uint8_t modify_other_keys; /* 0..2 */
} LunetermModes;

int32_t luneterm_engine_modes(LunetermEngine* engine, LunetermModes* out);

/* ---- Input encoders ----------------------------------------------------------------------- */

/* Bytes to write to the PTY, in the input slot. len 0 (and ptr NULL) means nothing to send. */
typedef struct LunetermBytes {
    const uint8_t* ptr;
    uint64_t len;
} LunetermBytes;

/* Modifier bits (LunetermKeyEvent.modifiers, LunetermMouseEvent.modifiers and the modifiers
 * arguments). Other bits are accepted and ignored. */
#define LUNETERM_MOD_SHIFT 1u
#define LUNETERM_MOD_ALT 2u
#define LUNETERM_MOD_CONTROL 4u

/* LunetermKeyEvent.key and the key argument of luneterm_engine_key_input (Lunegit's TerminalKey
 * ordinals); anything outside 0..27 is LUNETERM_ERR_INVALID_ARGUMENT. */
#define LUNETERM_KEY_ENTER 0u
#define LUNETERM_KEY_TAB 1u
#define LUNETERM_KEY_BACKSPACE 2u
#define LUNETERM_KEY_ESCAPE 3u
#define LUNETERM_KEY_SPACE 4u
#define LUNETERM_KEY_UP 5u
#define LUNETERM_KEY_DOWN 6u
#define LUNETERM_KEY_RIGHT 7u
#define LUNETERM_KEY_LEFT 8u
#define LUNETERM_KEY_HOME 9u
#define LUNETERM_KEY_END 10u
#define LUNETERM_KEY_PAGE_UP 11u
#define LUNETERM_KEY_PAGE_DOWN 12u
#define LUNETERM_KEY_INSERT 13u
#define LUNETERM_KEY_DELETE 14u
#define LUNETERM_KEY_F1 15u /* F2..F12 follow: F(n) = 14 + n */
#define LUNETERM_KEY_F12 26u
#define LUNETERM_KEY_CHARACTER 27u /* a text-producing key, described by codepoint */

typedef struct LunetermKeyEvent {
    uint32_t key; /* LUNETERM_KEY_* */
    /* For CHARACTER: the unshifted base character (a Unicode scalar value, else nothing is sent), or
     * the keypad character when is_keypad. */
    uint32_t codepoint;
    uint32_t shifted_codepoint;     /* what the key produces with Shift; 0 = unknown */
    uint32_t base_layout_codepoint; /* the key on the US-QWERTY layout; 0 = unknown */
    uint8_t modifiers;              /* LUNETERM_MOD_* */
    uint8_t is_keypad;
    uint8_t alt_is_meta; /* Alt acts as Meta (the legacy ESC prefix) */
    uint8_t reserved;    /* ignored */
} LunetermKeyEvent;

/* LunetermMouseEvent.button (Lunegit's TerminalMouseButton); outside 0..5 is rejected. */
#define LUNETERM_MOUSE_BUTTON_NONE 0u
#define LUNETERM_MOUSE_BUTTON_LEFT 1u
#define LUNETERM_MOUSE_BUTTON_MIDDLE 2u
#define LUNETERM_MOUSE_BUTTON_RIGHT 3u
#define LUNETERM_MOUSE_BUTTON_WHEEL_UP 4u
#define LUNETERM_MOUSE_BUTTON_WHEEL_DOWN 5u

/* LunetermMouseEvent.kind (Lunegit's TerminalMouseEventType); outside 0..5 is rejected. */
#define LUNETERM_MOUSE_KIND_DOWN 0u
#define LUNETERM_MOUSE_KIND_UP 1u
#define LUNETERM_MOUSE_KIND_MOVE 2u
#define LUNETERM_MOUSE_KIND_DRAG 3u /* motion with button held */
#define LUNETERM_MOUSE_KIND_WHEEL_UP 4u
#define LUNETERM_MOUSE_KIND_WHEEL_DOWN 5u

typedef struct LunetermMouseEvent {
    uint32_t button; /* LUNETERM_MOUSE_BUTTON_* */
    uint32_t kind;   /* LUNETERM_MOUSE_KIND_* */
    int32_t col;     /* zero-based cell column */
    int32_t row;     /* zero-based cell row */
    int32_t x;       /* pixel position (SGR pixels only) */
    int32_t y;
    uint8_t modifiers; /* LUNETERM_MOD_* */
    uint8_t reserved[3]; /* ignored */
} LunetermMouseEvent;

/* A keystroke under the live keyboard protocol state (kitty, modifyOtherKeys, DECCKM). */
int32_t luneterm_engine_key_event(LunetermEngine* engine, const LunetermKeyEvent* event, LunetermBytes* out);

/* The legacy bytes of a functional key with the live DECCKM state, whatever the protocol. */
int32_t luneterm_engine_key_input(LunetermEngine* engine, uint32_t key, uint8_t modifiers, LunetermBytes* out);

/* The legacy bytes of a character (Control folds to C0, Alt prefixes ESC). A codepoint that is not
 * a Unicode scalar value (a surrogate, above 0x10FFFF) is OK with len 0. */
int32_t luneterm_engine_char_input(LunetermEngine* engine, uint32_t codepoint, uint8_t modifiers, LunetermBytes* out);

/* A pointer event, encoded from the engine's own mouse protocol and encoding state. len 0 when no
 * tracking is on, the protocol does not report the event, or the position is outside the terminal.
 * The default encoding's bytes are raw (a coordinate above 95 is one byte above 0x7F). */
int32_t luneterm_engine_mouse_event(LunetermEngine* engine, const LunetermMouseEvent* event, LunetermBytes* out);

/* The focus report (ESC [ I / ESC [ O), only while the application has focus reporting on
 * (CSI ? 1004 h); focused nonzero = gained. */
int32_t luneterm_engine_focus_event(LunetermEngine* engine, uint8_t focused, LunetermBytes* out);

/* The bytes to write for pasting text (UTF-8, not NUL-terminated): line breaks become CR, and with
 * bracketed paste on the sanitized text is wrapped in ESC [ 200 ~ ... ESC [ 201 ~. The engine does
 * the sanitizing; the host cannot bypass it. Invalid UTF-8 is LUNETERM_ERR_INVALID_UTF8 and nothing
 * is produced. Empty text is OK with len 0. */
int32_t luneterm_engine_paste(LunetermEngine* engine, const uint8_t* text, size_t len, LunetermBytes* out);

#ifdef LUNETERM_PANIC_PROBE
/* Test only (a library built with --features panic-probe, never the shipping one): panics inside the
 * call guard, so the host can check that the panic is caught (LUNETERM_ERR_PANIC) and the handle
 * poisoned. */
int32_t luneterm_test_panic(LunetermEngine* engine);
#endif

/* ---- Layout (the same numbers are asserted in Rust) --------------------------------------- */

#define LUNETERM_LAYOUT(type, size, align)                                                         \
    LUNETERM_STATIC_ASSERT(sizeof(type) == (size), #type " size");                                 \
    LUNETERM_STATIC_ASSERT(LUNETERM_ALIGNOF(type) == (align), #type " alignment")
#define LUNETERM_OFFSET(type, field, offset)                                                       \
    LUNETERM_STATIC_ASSERT(offsetof(type, field) == (offset), #type "." #field " offset")

LUNETERM_LAYOUT(LunetermConfig, 40, 8);
LUNETERM_OFFSET(LunetermConfig, cols, 0);
LUNETERM_OFFSET(LunetermConfig, rows, 4);
LUNETERM_OFFSET(LunetermConfig, scrollback, 8);
LUNETERM_OFFSET(LunetermConfig, unicode, 12);
LUNETERM_OFFSET(LunetermConfig, xtversion, 16);
LUNETERM_OFFSET(LunetermConfig, xtversion_len, 24);
LUNETERM_OFFSET(LunetermConfig, clipboard_write, 32);
LUNETERM_OFFSET(LunetermConfig, reserved, 33);

LUNETERM_LAYOUT(LunetermBytes, 16, 8);
LUNETERM_OFFSET(LunetermBytes, ptr, 0);
LUNETERM_OFFSET(LunetermBytes, len, 8);

LUNETERM_LAYOUT(LunetermCell, 28, 4);
LUNETERM_OFFSET(LunetermCell, text_offset, 0);
LUNETERM_OFFSET(LunetermCell, text_len, 4);
LUNETERM_OFFSET(LunetermCell, fg, 8);
LUNETERM_OFFSET(LunetermCell, bg, 12);
LUNETERM_OFFSET(LunetermCell, underline_color, 16);
LUNETERM_OFFSET(LunetermCell, link_id, 20);
LUNETERM_OFFSET(LunetermCell, flags, 24);
LUNETERM_OFFSET(LunetermCell, width, 26);
LUNETERM_OFFSET(LunetermCell, underline, 27);

LUNETERM_LAYOUT(LunetermLink, 24, 8);
LUNETERM_OFFSET(LunetermLink, id, 0);
LUNETERM_OFFSET(LunetermLink, reserved, 4);
LUNETERM_OFFSET(LunetermLink, offset, 8);
LUNETERM_OFFSET(LunetermLink, len, 16);

LUNETERM_LAYOUT(LunetermViewport, 128, 8);
LUNETERM_OFFSET(LunetermViewport, cols, 0);
LUNETERM_OFFSET(LunetermViewport, rows, 4);
LUNETERM_OFFSET(LunetermViewport, cells, 8);
LUNETERM_OFFSET(LunetermViewport, cell_count, 16);
LUNETERM_OFFSET(LunetermViewport, text, 24);
LUNETERM_OFFSET(LunetermViewport, text_len, 32);
LUNETERM_OFFSET(LunetermViewport, links, 40);
LUNETERM_OFFSET(LunetermViewport, link_count, 48);
LUNETERM_OFFSET(LunetermViewport, link_data, 56);
LUNETERM_OFFSET(LunetermViewport, link_data_len, 64);
LUNETERM_OFFSET(LunetermViewport, title, 72);
LUNETERM_OFFSET(LunetermViewport, title_len, 80);
LUNETERM_OFFSET(LunetermViewport, viewport_top, 88);
LUNETERM_OFFSET(LunetermViewport, base_y, 96);
LUNETERM_OFFSET(LunetermViewport, total_lines, 104);
LUNETERM_OFFSET(LunetermViewport, cursor_x, 112);
LUNETERM_OFFSET(LunetermViewport, cursor_y, 116);
LUNETERM_OFFSET(LunetermViewport, cursor_visible, 120);
LUNETERM_OFFSET(LunetermViewport, cursor_style, 121);
LUNETERM_OFFSET(LunetermViewport, cursor_blink, 122);
LUNETERM_OFFSET(LunetermViewport, is_at_bottom, 123);
LUNETERM_OFFSET(LunetermViewport, alt_screen, 124);
LUNETERM_OFFSET(LunetermViewport, reserved, 125);

LUNETERM_LAYOUT(LunetermEvent, 32, 8);
LUNETERM_OFFSET(LunetermEvent, kind, 0);
LUNETERM_OFFSET(LunetermEvent, value, 4);
LUNETERM_OFFSET(LunetermEvent, value2, 8);
LUNETERM_OFFSET(LunetermEvent, flags, 12);
LUNETERM_OFFSET(LunetermEvent, offset, 16);
LUNETERM_OFFSET(LunetermEvent, len, 24);

LUNETERM_LAYOUT(LunetermColorRequest, 12, 4);
LUNETERM_OFFSET(LunetermColorRequest, kind, 0);
LUNETERM_OFFSET(LunetermColorRequest, index, 4);
LUNETERM_OFFSET(LunetermColorRequest, r, 8);
LUNETERM_OFFSET(LunetermColorRequest, g, 9);
LUNETERM_OFFSET(LunetermColorRequest, b, 10);
LUNETERM_OFFSET(LunetermColorRequest, reserved, 11);

LUNETERM_LAYOUT(LunetermEvents, 48, 8);
LUNETERM_OFFSET(LunetermEvents, events, 0);
LUNETERM_OFFSET(LunetermEvents, count, 8);
LUNETERM_OFFSET(LunetermEvents, data, 16);
LUNETERM_OFFSET(LunetermEvents, data_len, 24);
LUNETERM_OFFSET(LunetermEvents, colors, 32);
LUNETERM_OFFSET(LunetermEvents, color_count, 40);

LUNETERM_LAYOUT(LunetermDamage, 12, 4);
LUNETERM_OFFSET(LunetermDamage, has_rows, 0);
LUNETERM_OFFSET(LunetermDamage, full, 1);
LUNETERM_OFFSET(LunetermDamage, cursor_moved, 2);
LUNETERM_OFFSET(LunetermDamage, scrolled, 3);
LUNETERM_OFFSET(LunetermDamage, first_row, 4);
LUNETERM_OFFSET(LunetermDamage, last_row, 8);

LUNETERM_LAYOUT(LunetermModes, 14, 1);
LUNETERM_OFFSET(LunetermModes, application_cursor_keys, 0);
LUNETERM_OFFSET(LunetermModes, application_keypad, 1);
LUNETERM_OFFSET(LunetermModes, bracketed_paste, 2);
LUNETERM_OFFSET(LunetermModes, insert, 3);
LUNETERM_OFFSET(LunetermModes, origin, 4);
LUNETERM_OFFSET(LunetermModes, reverse_wraparound, 5);
LUNETERM_OFFSET(LunetermModes, send_focus, 6);
LUNETERM_OFFSET(LunetermModes, synchronized_output, 7);
LUNETERM_OFFSET(LunetermModes, wraparound, 8);
LUNETERM_OFFSET(LunetermModes, cursor_hidden, 9);
LUNETERM_OFFSET(LunetermModes, mouse_tracking, 10);
LUNETERM_OFFSET(LunetermModes, mouse_encoding, 11);
LUNETERM_OFFSET(LunetermModes, kitty_keyboard_flags, 12);
LUNETERM_OFFSET(LunetermModes, modify_other_keys, 13);

LUNETERM_LAYOUT(LunetermKeyEvent, 20, 4);
LUNETERM_OFFSET(LunetermKeyEvent, key, 0);
LUNETERM_OFFSET(LunetermKeyEvent, codepoint, 4);
LUNETERM_OFFSET(LunetermKeyEvent, shifted_codepoint, 8);
LUNETERM_OFFSET(LunetermKeyEvent, base_layout_codepoint, 12);
LUNETERM_OFFSET(LunetermKeyEvent, modifiers, 16);
LUNETERM_OFFSET(LunetermKeyEvent, is_keypad, 17);
LUNETERM_OFFSET(LunetermKeyEvent, alt_is_meta, 18);
LUNETERM_OFFSET(LunetermKeyEvent, reserved, 19);

LUNETERM_LAYOUT(LunetermMouseEvent, 28, 4);
LUNETERM_OFFSET(LunetermMouseEvent, button, 0);
LUNETERM_OFFSET(LunetermMouseEvent, kind, 4);
LUNETERM_OFFSET(LunetermMouseEvent, col, 8);
LUNETERM_OFFSET(LunetermMouseEvent, row, 12);
LUNETERM_OFFSET(LunetermMouseEvent, x, 16);
LUNETERM_OFFSET(LunetermMouseEvent, y, 20);
LUNETERM_OFFSET(LunetermMouseEvent, modifiers, 24);
LUNETERM_OFFSET(LunetermMouseEvent, reserved, 25);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* LUNETERM_H */
