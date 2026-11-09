# ConPTY input: what a Windows program receives

Ledger row 10 ("Windows ConPTY key encoding never measured"). This page records what a child of
the pseudoconsole actually reads when the terminal writes a VT key sequence into ConPTY. The
golden table is `crates/lune-pty/tests/data/conpty-input.tsv`. The test that measures it and
checks it is `crates/lune-pty/tests/conpty_input.rs`.

## Method

The test binary re-executes itself as the PTY child through `PortablePtyBackend`, so the
measurement covers the real stack, portable-pty's ConPTY flags included. The child logs what it
reads to a side file. It never reports through the PTY output, because conhost re-renders output
and does not pass the bytes through. The parent writes each case in one write. After a 50 ms gap
it writes a mark byte (0x07, Ctrl+G), and it waits for that mark in the log before the next case.
Everything between two marks belongs to one case. Every run checks the instrument: `a` must read as
`61` in vt mode and as `vk=41 ch=0061` in keys mode, or the run fails with "harness broken".

The child reads in one of two modes. Each mode gets its own child and its own pseudoconsole.

| mode | console setup | read call | stands for |
|---|---|---|---|
| `vt` | `CONIN$` mode = `ENABLE_VIRTUAL_TERMINAL_INPUT` only, `SetConsoleCP(65001)` | `ReadFile` | VT-aware programs: ssh, vim, WSL |
| `keys` | `CONIN$` mode = `ENABLE_EXTENDED_FLAGS` only | `ReadConsoleInputW` | cmd, PSReadLine, most console programs |

In keys mode the table lists key-down records, plus one kind of key-up: the Alt key-up that
carries a character typed by Alt+numpad (see UTF-8 below). It also lists focus and mouse records.

### Conditions of the committed measurement

- OS: `Microsoft Windows [Version 10.0.26100.33650]` and `10.0.26100.33674`. The windows-latest
  image changed between the two runs, and both gave the same table. Build `10.0.26200.9711` (the
  self-hosted runner, CI run 42565765001) differs in one case: vt mode `ctrl-z` reads back `1a`
  where 26100 gives a zero-byte read (`[read0]`), so the golden lists both.
- `%SystemRoot%\System32\conpty.dll`: absent. portable-pty therefore falls back to kernel32's
  inbox `CreatePseudoConsole` (the inbox conhost).
- portable-pty 0.9.0 flags: `PSEUDOCONSOLE_INHERIT_CURSOR | PSEUDOCONSOLE_RESIZE_QUIRK |
  PSEUDOCONSOLE_WIN32_INPUT_MODE`.
- conhost requested win32-input-mode (`ESC[?9001h`) in both modes. The harness **ignores** the
  request and sends plain VT, which is what the app sends today.
- conhost's cursor request `ESC[6n` (from INHERIT_CURSOR) is answered with `ESC[1;1R`.
- The child ignores console control events (`SetConsoleCtrlHandler(NULL, TRUE)`). Processed input
  is off in both modes, so Ctrl+C arrives as data.
- Size: 120×30. The child enables no DEC modes: no mouse tracking, no focus reporting, no
  bracketed paste.

## Results per group

"Unchanged" means the child read exactly the bytes that were sent.

| group | vt mode | keys mode |
|---|---|---|
| printable ASCII | unchanged | key records from the US layout; Shift is synthesized for shifted characters (`A`, `~`) |
| UTF-8, 2 and 3 bytes (é, €) | unchanged | é: one record with `vk=00`. €: Alt down, numpad 6, numpad 3, then the character (`20ac`) on the Alt **key-up**. |
| UTF-8, 4 bytes (😀) | **lost**: `ef bf bd ef bf bd` (two U+FFFD) | two Alt+numpad groups. Each Alt key-up carries one surrogate half (`d83d`, then `de00`). |
| Enter, Tab, lone Esc | unchanged | `vk=0d`, `vk=09`, `vk=1b`. Lone Esc survives, followed by a 50 ms gap. |
| Shift+Tab `ESC[Z` | unchanged | Shift + `vk=09` |
| Backspace `7f` / Ctrl+H `08` | unchanged, and distinct | translated, and **swapped**: Backspace → `vk=08 ch=0008`; Ctrl+H → Ctrl + `vk=08 ch=007f` |
| Ctrl+C, D, Z | C and D unchanged. Ctrl+Z is **lost**: `ReadFile` returns 0 bytes, which a `ReadFile` caller takes as EOF. | Ctrl+C: `vk=43 sc=00`, with no Ctrl press record. D and Z: Ctrl press + key. |
| Ctrl+Space `00` | unchanged (`00`) | translated to **Ctrl+Shift+2** (Ctrl+@, `vk=32 ch=0000`), not Ctrl+Space |
| Ctrl+\ ] ^ _ | unchanged | Ctrl+`vk=dc`/`dd`. ^ and _ become Ctrl+Shift+6 and Ctrl+Shift+minus, with the control character set. |
| Alt+letter, Alt+Shift+letter, Alt+Backspace | unchanged | Alt (and Shift) presses + key with `LEFT_ALT` |
| arrows, normal (`CSI A`) and application (`SS3 A`) | unchanged | the same VK. CSI sets `ENHANCED_KEY`; SS3 does not. |
| arrows with Shift / Alt / Ctrl / Ctrl+Shift | unchanged | modifier presses + arrow, with the right control state |
| Home/End (CSI, SS3, tilde forms), Ins, Del, Ctrl+Del, PgUp/PgDn | unchanged | the right VKs. Every form except SS3 sets `ENHANCED_KEY`. |
| F1–F12, Shift+F5, Ctrl+F1 | unchanged | `vk=70`–`7b`, with modifiers |
| focus in/out | **lost** (nothing arrives) | `FOCUS_EVENT` records |
| bracketed paste | unchanged, markers included | the markers are stripped and the text arrives as key records |
| SGR mouse | unchanged | `MOUSE_EVENT` records with 0-based cells (`10;5` → x=9 y=4), even though `ENABLE_MOUSE_INPUT` is off |
| X10 mouse `ESC[M␠*%` | unchanged | `CSI M` is swallowed, and the three coordinate bytes leak out as keys (`␠ * %`) |
| kitty CSI-u, modifyOtherKeys | unchanged | **lost** (no records) |
| CPR reply `ESC[12;34R` | unchanged | translated **wrongly**, to Shift+F3 (the `CSI 1;2 R` F3 form) |
| DA1 reply | unchanged | **lost** |
| win32-input-mode `ESC[65;30;97;1;0;1_` | translated to its character (`61`) | exactly the encoded record (`vk=41 sc=1e ch=0061`) |

In short, vt-mode readers get the VT bytes as sent, except for 4-byte UTF-8, focus reports and
Ctrl+Z. keys-mode readers get conhost's interpretation, and everything that has no key record is
lost.

## What this means for the key encoder (M6/M9)

- **Keep xterm's legacy encoding as the default on Windows.** Every legacy key the encoder sends
  (cursor and keypad keys in both modes, F-keys, modifiers in the `1;N` form, Alt as an ESC
  prefix, Backspace as `7f`) reaches both kinds of program correctly.
- **Lone Esc survives** in both modes when a gap follows it. Back-to-back Esc + key timing was not
  measured.
- **Ctrl+Space cannot be delivered faithfully in plain VT.** vt readers get NUL, which is correct.
  keys readers get Ctrl+Shift+2, so a PSReadLine binding on Ctrl+Space does not fire.
- **Backspace and Ctrl+H stay distinct** in both modes, although keys readers see their characters
  swapped. Send `7f` for Backspace, as xterm does.
- **Do not send kitty CSI-u or modifyOtherKeys to Windows children** unless the program asked for
  them. keys readers lose those sequences entirely.
- **Never send an unsolicited CPR.** A stray `ESC[row;colR` reaches keys readers as Shift+F3. The
  engine answers only real `ESC[6n` requests, ConPTY's own included (D11).
- **Focus reports:** send them only after DECSET 1004, as on every OS. In this setup vt readers
  received none. Whether conhost forwards them after the child sets `?1004h` was not measured.
- **Mouse:** SGR works in both modes, so prefer it. X10 garbles keys readers.
- **4-byte UTF-8 (emoji) is lost to vt readers** whatever the terminal sends as plain VT. keys
  readers get it as surrogate pairs on Alt key-ups. This is a conhost limitation, not an encoder
  bug.
- **win32-input-mode is worth adopting on Windows (recommendation, M9).** conhost asks for it on
  every session (`ESC[?9001h`). A win32-input-mode sequence arrives as the exact key record for
  keys readers, and as the right character for vt readers. With it the encoder could deliver
  Ctrl+Space, Ctrl+H and modifier-exact keys that plain VT cannot express. The cost is that the
  encoder must emit VK, scan code and control state from the platform key event. Only one
  sequence (a plain `a`) was measured here. Measure the modified keys before relying on it.

### Status after M7: which of these the encoder meets

The encoders are in `lune-vt` (`Terminal::key_event`, `key_input`, `char_input`, `mouse_event`,
`focus_event`, `paste`; `docs/architecture.md`, "lune-vt engine"). They take no platform input:
the same bytes are produced on every OS, so each line below is about the bytes, not about a
measurement on Windows. Nothing here was re-measured through ConPTY; `conpty_input.tsv` is
unchanged.

| Recommendation | Status |
|---|---|
| Legacy encoding as the default | **Met.** The kitty and modifyOtherKeys branches of `encode_key` run only when the application asked for them (`kitty_flags & 1`, modifyOtherKeys level >= 1); with neither the bytes are `legacy_key_input` / `legacy_char_input`, which `goldens/lunegit-input/keys.golden` pins row by row against Lunegit's own encoder (its legacy generator is itself a port of XTerm.NET's, which sent these bytes before the engine swap). Backspace is `7f` (`legacy_key_encoder.rs`). |
| No kitty / modifyOtherKeys unless the program asked | **Met in the encoder.** The state starts at 0, only the application's own `CSI > Ps u` / `CSI = Ps ; Pm u` / `CSI > 4 ; Pv m` set it, and RIS, DECSTR and `Terminal::reset_keyboard_protocol()` clear it (ledger row 11). Whether ConPTY forwards such a request to the terminal was not measured. |
| No unsolicited CPR | **Met.** The M7 replies (XTVERSION, the kitty flags query, the modifyOtherKeys query) are answers to queries only, and the engine's CPR logic is unchanged (M6 D11). |
| Focus reports only after DECSET 1004 | **Met in the encoder.** `focus_event` returns `None` unless `CSI ? 1004 h` is on (`focus_*` tests in `headless/terminal_tests.rs`). Whether conhost forwards the child's `?1004h` stays unmeasured. |
| Prefer SGR mouse | **Not the encoder's choice.** `mouse_event` encodes in the protocol and encoding the application selected with DECSET (`?9`, `?1000`, `?1002`, `?1003`; default, `?1006`, `?1016`). A program that selects the default encoding gets the X10-style bytes that garble keys-mode readers. |
| Lone Esc, Esc + key timing | **The host's.** The encoder returns `ESC` for Escape; when it is written is the host's (M9). |
| Ctrl+Space, 4-byte UTF-8, Ctrl+Z | **Not fixable in plain VT.** Ctrl+Space encodes as NUL (a comment in `legacy_key_encoder.rs` notes XTerm.NET does too); the others are conhost limitations. |
| win32-input-mode | **Not implemented.** Out of scope for M7 (plan risks); M9 decides after measuring modified keys. |

## How to re-measure

On Windows (or on CI, where the lune-pty soak runs the test six times per run):

```
cargo test -p lune-pty --test conpty_input
```

The test prints the measured table between `BEGIN conpty-input` and `END conpty-input`, with the
conditions as `#` comments. If the table no longer matches the golden, the test fails with one
line per difference. The same happens when the golden file is missing.

To update:

1. Check that the difference is real and not a harness fault. The instrument check on `a` must
   still pass.
2. Replace the rows in `crates/lune-pty/tests/data/conpty-input.tsv` with the printed rows, and
   keep the header comments.
3. If repeated runs disagree, list each observation for that case, separated by ` | `.
4. Update this page.

The comment lines are not compared, so an OS build change alone does not fail the test. To add a
case, append it to `CASES` in the test. A case must not contain 0x07; the test asserts this.
