# Lunegit terminal bug ledger

Bugs and gaps found in Lunegit's terminal (as of 2026-10-29), and what Luneterm must do about each.
Paths refer to the Lunegit repo. Every fixed bug gets a regression test here too.

| # | Issue (Lunegit) | Status there | Luneterm |
|---|---|---|---|
| 1 | Wide glyphs spill onto the next cell | Fixed (glyph containment) | Fixed in `lune-termview` (M2): Lunegit's rule ported with its cases (`contain` unit tests, `fit_scale_lunegit_cases`, `allowance_cases`), plus a per-glyph clip rect in the shader and the font-independent `containment_no_spill` test (CJK, emoji, combining flood, box drawing) |
| 2 | Glyphs overflow vertically into adjacent rows | Open, by design | Fixed in `lune-termview` (M2): every glyph instance carries a clip rect whose vertical extent is its row, enforced in the fragment shader (decision D5); covered by `containment_no_spill` |
| 3 | Split multibyte UTF-8 decoded as U+FFFD | Fixed (`PtyTerminalSessionDecodeRegressionTests`) | Streaming decoder test from day one |
| 4 | Shrink reflow destroyed wrapped tails (old XTerm.NET engine) | Fixed by moving to xterm.js | Covered by porting `BufferReflow` tests + resize vectors |
| 5 | VS16 emoji (e.g. ⚠ + U+FE0F) must be 2 cells | Fixed (custom unicode provider) | Port as width-provider extension + test |
| 6 | Output flood: 534–908 projections/s vs 60 fps | Fixed (16 ms throttle) | Render on vsync from dirty rows; honour mode 2026 |
| 7 | Heavy-output retained memory (~40 MB unattributed; V8 suspected) | Partly fixed | No V8; measure scrollback memory |
| 8 | COLR/CBDT emoji render blank | Fixed via bundled sbix Twemoji; Win/Linux unverified | Verified in M2: the `emoji_sbix` golden (sbix test font) and the macOS-only `system_fallback` golden (Apple Color Emoji); CI probe P4 shows colour content from the system emoji font on all three OSes (Apple Color Emoji 26x26, Segoe UI Emoji 26x25 on Windows, Noto Color Emoji 32x30 on Linux) |
| 9 | Master fd via reflection; termios offsets hard-coded (macOS 24) | Workaround + guard tests | Use the fd from the PTY crate and `tcgetattr` |
| 10 | Windows ConPTY key encoding never measured | Open | Measure early on the Windows VM |
| 11 | Kitty flags stuck after a crashed app (Ctrl+C sends `ESC[99;5u`) | Accepted risk | Decide: reset flags when the foreground process changes |
| 12 | Persisted workspace overwritten when tabs fail to spawn (`todo.md:263`) | Open | Never persist a state built from failed spawns |
| 13 | OSC 52 query never answered | By design (security) | Keep |
| 14 | Synchronized output parsed but not used for painting | Gap | Implement paint deferral |
| 15 | Ligatures effectively unsupported | Gap | Atlas is keyed by cosmic-text `CacheKey` (font, glyph id, size, subpixel bin), not by character, so whole-row shaping can feed it later (M2 shapes per cluster; ligatures are not drawn yet) |
| 16 | Running-outline CPU budget (≤3 pp) never measured | Open | Measure; GPU-drawn animation |
| 17 | No OSC 133 full semantic prompts | Parked | Extension later |
