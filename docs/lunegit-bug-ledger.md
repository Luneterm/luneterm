# Lunegit terminal bug ledger

Bugs and gaps found in Lunegit's terminal (as of 2026-10-29), and what Luneterm must do about each.
Paths refer to the Lunegit repo. Every fixed bug gets a regression test here too.

| # | Issue (Lunegit) | Status there | Luneterm |
|---|---|---|---|
| 1 | Wide glyphs spill onto the next cell | Fixed (glyph containment) | Port containment + its tests (`TerminalView*` glyph overflow tests) |
| 2 | Glyphs overflow vertically into adjacent rows | Open, by design | Clip glyphs to their row |
| 3 | Split multibyte UTF-8 decoded as U+FFFD | Fixed (`PtyTerminalSessionDecodeRegressionTests`) | Streaming decoder test from day one |
| 4 | Shrink reflow destroyed wrapped tails (old XTerm.NET engine) | Fixed by moving to xterm.js | Covered by porting `BufferReflow` tests + resize vectors |
| 5 | VS16 emoji (e.g. ⚠ + U+FE0F) must be 2 cells | Fixed (custom unicode provider) | Port as width-provider extension + test |
| 6 | Output flood: 534–908 projections/s vs 60 fps | Fixed (16 ms throttle) | Render on vsync from dirty rows; honour mode 2026 |
| 7 | Heavy-output retained memory (~40 MB unattributed; V8 suspected) | Partly fixed | No V8; measure scrollback memory |
| 8 | COLR/CBDT emoji render blank | Fixed via bundled sbix Twemoji; Win/Linux unverified | Verify rasterizer colour-font support on all OSes |
| 9 | Master fd via reflection; termios offsets hard-coded (macOS 24) | Workaround + guard tests | Use the fd from the PTY crate and `tcgetattr` |
| 10 | Windows ConPTY key encoding never measured | Open | Measure early on the Windows VM |
| 11 | Kitty flags stuck after a crashed app (Ctrl+C sends `ESC[99;5u`) | Accepted risk | Decide: reset flags when the foreground process changes |
| 12 | Persisted workspace overwritten when tabs fail to spawn (`todo.md:263`) | Open | Never persist a state built from failed spawns |
| 13 | OSC 52 query never answered | By design (security) | Keep |
| 14 | Synchronized output parsed but not used for painting | Gap | Implement paint deferral |
| 15 | Ligatures effectively unsupported | Gap | Optional; design atlas to allow it |
| 16 | Running-outline CPU budget (≤3 pp) never measured | Open | Measure; GPU-drawn animation |
| 17 | No OSC 133 full semantic prompts | Parked | Extension later |
