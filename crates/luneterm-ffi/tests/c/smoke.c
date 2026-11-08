/*
 * C smoke test of the luneterm ABI (M8 step 5): one run through every export, as a C host calls
 * them. Build: cc -std=c11 -Wall -Wextra -Werror -pedantic -I include smoke.c -lluneterm_ffi
 * (-DLUNETERM_PANIC_PROBE against a library built with --features panic-probe).
 * Exit 0 only if every check passed; each failed check is printed by name. Only the fixed test
 * strings below are ever compared or printed.
 */
#include <stdio.h>
#include <string.h>

#include "luneterm.h"

static int failures = 0;

#define CHECK(cond)                                                                                \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            failures++;                                                                            \
            fprintf(stderr, "FAILED line %d: %s\n", __LINE__, #cond);                              \
        }                                                                                          \
    } while (0)

static void write_str(LunetermEngine* e, const char* s) {
    CHECK(luneterm_engine_write(e, (const uint8_t*)s, strlen(s)) == LUNETERM_OK);
}

static int bytes_equal(const LunetermBytes* b, const char* expected) {
    size_t n = strlen(expected);
    return b->len == (uint64_t)n && (n == 0 ? b->ptr == NULL : memcmp(b->ptr, expected, n) == 0);
}

static int mem_equal(const uint8_t* p, uint64_t len, const char* expected) {
    size_t n = strlen(expected);
    return len == (uint64_t)n && (n == 0 || memcmp(p, expected, n) == 0);
}

static void check_viewport(LunetermEngine* e) {
    LunetermViewport vp;
    memset(&vp, 0xAB, sizeof vp);
    CHECK(luneterm_engine_viewport(e, &vp) == LUNETERM_OK);
    CHECK(vp.cols == 80 && vp.rows == 24 && vp.cell_count == 80u * 24u);
    if (vp.cells == NULL || vp.cell_count < 4) {
        CHECK(!"viewport cells present");
        return;
    }
    /* Cell 0: a bold red 'R'. */
    const LunetermCell* c = &vp.cells[0];
    CHECK(c->text_len == 1 && vp.text[c->text_offset] == 'R');
    CHECK(c->fg == ((LUNETERM_COLOR_PALETTE << 24) | 1u));
    CHECK(c->bg == LUNETERM_COLOR_DEFAULT);
    CHECK(c->flags == LUNETERM_CELL_BOLD);
    CHECK(c->width == 1 && c->underline == LUNETERM_UNDERLINE_NONE && c->link_id == 0);
    /* Cells 1 and 2: a wide character (U+3042) and its empty second half. */
    CHECK(vp.cells[1].width == 2 && vp.cells[1].text_len == 3);
    CHECK(vp.cells[1].text_len == 3 &&
          memcmp(vp.text + vp.cells[1].text_offset, "\xe3\x81\x82", 3) == 0);
    CHECK(vp.cells[2].width == 0 && vp.cells[2].text_len == 0);
    /* Cell 3: a link. */
    CHECK(vp.cells[3].text_len == 1 && vp.text[vp.cells[3].text_offset] == 'L');
    CHECK(vp.cells[3].link_id != 0);
    CHECK(vp.link_count == 1 && vp.links != NULL);
    if (vp.link_count == 1 && vp.links != NULL) {
        CHECK(vp.links[0].id == vp.cells[3].link_id);
        CHECK(vp.links[0].reserved == 0);
        CHECK(vp.links[0].offset + vp.links[0].len <= vp.link_data_len);
        CHECK(mem_equal(vp.link_data + vp.links[0].offset, vp.links[0].len, "https://example.com/x"));
    }
    CHECK(mem_equal(vp.title, vp.title_len, "smoke title"));
    /* 'R', wide (2 cells), 'L' fill columns 0-3; the cursor sits at column 4 of row 0. */
    CHECK(vp.cursor_x == 4 && vp.cursor_y == 0 && vp.cursor_visible == 1);
    CHECK(vp.cursor_style == LUNETERM_CURSOR_BLOCK && vp.is_at_bottom == 1 && vp.alt_screen == 0);
    CHECK(vp.viewport_top == vp.base_y && vp.total_lines >= 24);
    CHECK(vp.reserved[0] == 0 && vp.reserved[1] == 0 && vp.reserved[2] == 0);
    /* An empty cell. */
    CHECK(vp.cells[10].text_len == 0 && vp.cells[10].width == 1);
}

static void check_events(LunetermEngine* e) {
    LunetermEvents ev;
    memset(&ev, 0xAB, sizeof ev);
    CHECK(luneterm_engine_drain_events(e, &ev) == LUNETERM_OK);
    /* Title, DA1 response, clipboard write, command mark D;3, bell: in write order. */
    CHECK(ev.count == 5);
    if (ev.count != 5) {
        return;
    }
    CHECK(ev.events[0].kind == LUNETERM_EVENT_TITLE_CHANGED);
    CHECK(ev.events[1].kind == LUNETERM_EVENT_RESPONSE);
    CHECK(ev.events[1].len > 2 && ev.events[1].offset + ev.events[1].len <= ev.data_len);
    CHECK(ev.events[1].len > 2 && ev.data[ev.events[1].offset] == 0x1b &&
          ev.data[ev.events[1].offset + 1] == '[');
    CHECK(ev.events[2].kind == LUNETERM_EVENT_CLIPBOARD_WRITE);
    CHECK(ev.events[2].offset + ev.events[2].len <= ev.data_len &&
          mem_equal(ev.data + ev.events[2].offset, ev.events[2].len, "hi"));
    CHECK(ev.events[3].kind == LUNETERM_EVENT_COMMAND_MARK);
    CHECK(ev.events[3].value == 3 && ev.events[3].value2 == 3);
    CHECK(ev.events[3].flags == LUNETERM_EVENT_HAS_EXIT_CODE);
    CHECK(ev.events[4].kind == LUNETERM_EVENT_BELL);
    /* A second drain is empty, with NULL pointers. */
    LunetermEvents again;
    memset(&again, 0xAB, sizeof again);
    CHECK(luneterm_engine_drain_events(e, &again) == LUNETERM_OK);
    CHECK(again.count == 0 && again.events == NULL && again.data == NULL && again.colors == NULL);
    /* The first drain's slot is untouched by write, resize-free calls and the other getters. */
    CHECK(ev.events[4].kind == LUNETERM_EVENT_BELL);
}

static void check_damage_and_modes(LunetermEngine* e) {
    LunetermDamage d;
    memset(&d, 0xAB, sizeof d);
    CHECK(luneterm_engine_take_damage(e, &d) == LUNETERM_OK);
    CHECK(d.has_rows == 1 && d.first_row == 0 && d.cursor_moved == 1);
    LunetermDamage empty;
    CHECK(luneterm_engine_take_damage(e, &empty) == LUNETERM_OK);
    CHECK(empty.has_rows == 0 && empty.full == 0 && empty.cursor_moved == 0 && empty.scrolled == 0);
    CHECK(empty.first_row == 0 && empty.last_row == 0);

    LunetermModes m;
    memset(&m, 0xAB, sizeof m);
    CHECK(luneterm_engine_modes(e, &m) == LUNETERM_OK);
    CHECK(m.bracketed_paste == 0 && m.mouse_tracking == LUNETERM_MOUSE_TRACKING_NONE);
    CHECK(m.wraparound == 1 && m.application_cursor_keys == 0);
    write_str(e, "\x1b[?2004h\x1b[?1000h\x1b[?1006h");
    CHECK(luneterm_engine_modes(e, &m) == LUNETERM_OK);
    CHECK(m.bracketed_paste == 1);
    CHECK(m.mouse_tracking == LUNETERM_MOUSE_TRACKING_VT200);
    CHECK(m.mouse_encoding == LUNETERM_MOUSE_ENCODING_SGR);
}

static void check_encoders(LunetermEngine* e) {
    LunetermBytes out;
    /* Up with DECCKM off and on. */
    CHECK(luneterm_engine_key_input(e, LUNETERM_KEY_UP, 0, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, "\x1b[A"));
    write_str(e, "\x1b[?1h");
    CHECK(luneterm_engine_key_input(e, LUNETERM_KEY_UP, 0, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, "\x1bOA"));
    /* key_event: Enter. */
    LunetermKeyEvent key;
    memset(&key, 0, sizeof key);
    key.key = LUNETERM_KEY_ENTER;
    key.alt_is_meta = 1;
    CHECK(luneterm_engine_key_event(e, &key, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, "\r"));
    /* Ctrl-C. */
    CHECK(luneterm_engine_char_input(e, 'c', LUNETERM_MOD_CONTROL, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, "\x03"));
    /* A non-scalar code point sends nothing. */
    CHECK(luneterm_engine_char_input(e, 0xD800, 0, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, ""));
    /* An SGR left press at column 5, row 3. */
    LunetermMouseEvent mouse;
    memset(&mouse, 0, sizeof mouse);
    mouse.button = LUNETERM_MOUSE_BUTTON_LEFT;
    mouse.kind = LUNETERM_MOUSE_KIND_DOWN;
    mouse.col = 5;
    mouse.row = 3;
    CHECK(luneterm_engine_mouse_event(e, &mouse, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, "\x1b[<0;6;4M"));
    /* Focus reporting off, then on. */
    CHECK(luneterm_engine_focus_event(e, 1, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, ""));
    write_str(e, "\x1b[?1004h");
    CHECK(luneterm_engine_focus_event(e, 1, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, "\x1b[I"));
    /* Bracketed paste (turned on in check_damage_and_modes). */
    const uint8_t text[] = {'h', 'i'};
    CHECK(luneterm_engine_paste(e, text, sizeof text, &out) == LUNETERM_OK);
    CHECK(bytes_equal(&out, "\x1b[200~hi\x1b[201~"));
    /* The input slot survives a viewport read. */
    LunetermViewport vp;
    CHECK(luneterm_engine_viewport(e, &vp) == LUNETERM_OK);
    CHECK(bytes_equal(&out, "\x1b[200~hi\x1b[201~"));
}

static void check_control(LunetermEngine* e) {
    uint32_t cols = 0, rows = 0;
    CHECK(luneterm_engine_size(e, &cols, &rows) == LUNETERM_OK && cols == 80 && rows == 24);
    CHECK(luneterm_engine_resize(e, 100, 30) == LUNETERM_OK);
    CHECK(luneterm_engine_size(e, &cols, &rows) == LUNETERM_OK && cols == 100 && rows == 30);
    CHECK(luneterm_engine_resize(e, 80, 24) == LUNETERM_OK);
    CHECK(luneterm_engine_scroll_lines(e, -1) == LUNETERM_OK);
    CHECK(luneterm_engine_scroll_to_top(e) == LUNETERM_OK);
    CHECK(luneterm_engine_scroll_to_bottom(e) == LUNETERM_OK);
    uint8_t more = 9;
    CHECK(luneterm_engine_idle(e, &more) == LUNETERM_OK && more <= 1);
    CHECK(luneterm_engine_set_clipboard_write(e, 0) == LUNETERM_OK);
    CHECK(luneterm_engine_reset_keyboard_protocol(e) == LUNETERM_OK);
    CHECK(luneterm_engine_write(e, NULL, 0) == LUNETERM_OK);
}

static void check_errors(LunetermEngine* e) {
    const uint8_t x = 'x';
    uint32_t cols = 7, rows = 7;
    LunetermBytes out;
    memset(&out, 0xAB, sizeof out);
    CHECK(luneterm_engine_write(NULL, &x, 1) == LUNETERM_ERR_NULL_POINTER);
    CHECK(luneterm_engine_size(NULL, &cols, &rows) == LUNETERM_ERR_NULL_POINTER);
    CHECK(cols == 0 && rows == 0);
    CHECK(luneterm_engine_write(e, NULL, 1) == LUNETERM_ERR_NULL_POINTER);
    CHECK(luneterm_engine_resize(e, LUNETERM_MAX_COLS + 1, 24) == LUNETERM_ERR_INVALID_ARGUMENT);
    CHECK(luneterm_engine_resize(e, 80, LUNETERM_MAX_ROWS + 1) == LUNETERM_ERR_INVALID_ARGUMENT);
    CHECK(luneterm_engine_size(e, &cols, &rows) == LUNETERM_OK && cols == 80 && rows == 24);
    LunetermKeyEvent key;
    memset(&key, 0, sizeof key);
    key.key = 28;
    CHECK(luneterm_engine_key_event(e, &key, &out) == LUNETERM_ERR_INVALID_ARGUMENT);
    CHECK(out.len == 0 && out.ptr == NULL);
    CHECK(luneterm_engine_key_input(e, 28, 0, &out) == LUNETERM_ERR_INVALID_ARGUMENT);
    CHECK(luneterm_engine_key_event(e, NULL, &out) == LUNETERM_ERR_NULL_POINTER);
    CHECK(luneterm_engine_key_event(e, &key, NULL) == LUNETERM_ERR_NULL_POINTER);
    LunetermMouseEvent mouse;
    memset(&mouse, 0, sizeof mouse);
    mouse.button = 6;
    CHECK(luneterm_engine_mouse_event(e, &mouse, &out) == LUNETERM_ERR_INVALID_ARGUMENT);
    const uint8_t bad[] = {0xff, 'a'};
    CHECK(luneterm_engine_paste(e, bad, sizeof bad, &out) == LUNETERM_ERR_INVALID_UTF8);
    CHECK(out.len == 0 && out.ptr == NULL);
    CHECK(luneterm_engine_paste(e, NULL, 1, &out) == LUNETERM_ERR_NULL_POINTER);
    LunetermViewport vp;
    CHECK(luneterm_engine_viewport(e, NULL) == LUNETERM_ERR_NULL_POINTER);
    CHECK(luneterm_engine_viewport(NULL, &vp) == LUNETERM_ERR_NULL_POINTER);
    CHECK(vp.cell_count == 0 && vp.cells == NULL);
    /* The engine is still usable. */
    CHECK(luneterm_engine_size(e, &cols, &rows) == LUNETERM_OK);
    /* Construction errors leave *out NULL. */
    LunetermConfig cfg;
    memset(&cfg, 0, sizeof cfg);
    cfg.cols = LUNETERM_MAX_COLS + 1;
    LunetermEngine* none = (LunetermEngine*)&cfg;
    CHECK(luneterm_engine_new(&cfg, &none) == LUNETERM_ERR_INVALID_ARGUMENT && none == NULL);
    CHECK(luneterm_engine_new(NULL, &none) == LUNETERM_ERR_NULL_POINTER && none == NULL);
    cfg.cols = 80;
    cfg.unicode = 3;
    CHECK(luneterm_engine_new(&cfg, &none) == LUNETERM_ERR_INVALID_ARGUMENT && none == NULL);
    CHECK(luneterm_engine_free(NULL) == LUNETERM_OK);
}

#ifdef LUNETERM_PANIC_PROBE
static void check_panic_probe(void) {
    LunetermConfig cfg;
    memset(&cfg, 0, sizeof cfg);
    LunetermEngine* e = NULL;
    CHECK(luneterm_engine_new(&cfg, &e) == LUNETERM_OK && e != NULL);
    if (e == NULL) {
        return;
    }
    CHECK(luneterm_test_panic(e) == LUNETERM_ERR_PANIC);
    const uint8_t x = 'x';
    CHECK(luneterm_engine_write(e, &x, 1) == LUNETERM_ERR_POISONED);
    LunetermViewport vp;
    CHECK(luneterm_engine_viewport(e, &vp) == LUNETERM_ERR_POISONED);
    CHECK(luneterm_test_panic(e) == LUNETERM_ERR_POISONED);
    CHECK(luneterm_engine_free(e) == LUNETERM_OK);
}
#endif

int main(void) {
    CHECK(luneterm_abi_version() == LUNETERM_ABI_VERSION);

    static const uint8_t xtversion[] = "luneterm-smoke";
    LunetermConfig cfg;
    memset(&cfg, 0, sizeof cfg);
    cfg.cols = 80;
    cfg.rows = 24;
    cfg.scrollback = 1000;
    cfg.unicode = LUNETERM_UNICODE_11_VS16;
    cfg.xtversion = xtversion;
    cfg.xtversion_len = sizeof xtversion - 1;
    cfg.clipboard_write = 1;
    LunetermEngine* e = NULL;
    CHECK(luneterm_engine_new(&cfg, &e) == LUNETERM_OK && e != NULL);
    if (e == NULL) {
        return 1;
    }

    /* A bold red 'R', a wide character, an OSC 8 link, a title, DA1, OSC 52, OSC 133 D;3, BEL. */
    write_str(e, "\x1b[1;31mR\x1b[0m");
    write_str(e, "\xe3\x81\x82");
    write_str(e, "\x1b]8;id=l;https://example.com/x\x1b\\L\x1b]8;;\x1b\\");
    write_str(e, "\x1b]2;smoke title\x07");
    write_str(e, "\x1b[c");
    write_str(e, "\x1b]52;c;aGk=\x07");
    write_str(e, "\x1b]133;D;3\x07");
    write_str(e, "\x07");

    check_viewport(e);
    check_events(e);
    check_damage_and_modes(e);
    check_encoders(e);
    check_control(e);
    check_errors(e);
    /* XTVERSION answers with the configured name (after draining the focus-report request). */
    LunetermEvents ev;
    CHECK(luneterm_engine_drain_events(e, &ev) == LUNETERM_OK);
    write_str(e, "\x1b[>q");
    CHECK(luneterm_engine_drain_events(e, &ev) == LUNETERM_OK && ev.count == 1);
    if (ev.count == 1) {
        CHECK(ev.events[0].kind == LUNETERM_EVENT_RESPONSE);
        CHECK(ev.events[0].offset + ev.events[0].len <= ev.data_len &&
              mem_equal(ev.data + ev.events[0].offset, ev.events[0].len,
                        "\x1bP>|luneterm-smoke\x1b\\"));
    }
    CHECK(luneterm_engine_free(e) == LUNETERM_OK);

#ifdef LUNETERM_PANIC_PROBE
    check_panic_probe();
#endif

    if (failures != 0) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    puts("luneterm c smoke: ok");
    return 0;
}
