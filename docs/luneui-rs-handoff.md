# luneui-rs hand-off

What Luneterm found while being luneui-rs's first real consumer (roadmap D23, M9). Luneterm never
patches the toolkit: no `[patch]`, no fork, no vendored copy (`scripts/check-deps.sh` enforces it).
A gap is recorded here with its evidence and worked around app-side through public API; fixing it
is a luneui-rs task, and Luneterm then bumps the pin (`Cargo.toml` and `scripts/check-deps.sh`, one
reviewed change). Flipping a Class B entry to `done` is a luneui-rs commit.

- Toolkit read: `~/Projects/luneui-rs` at `5216931d8ae06f01cbf38e3408bc43b1096dceed` (`main`, "Merge
  branch 'orch/library-pass'"), the rev Luneterm pins. Paths marked `L:` are in that repo at that rev.
- Luneterm at `b535456` (M9 step 8) for the CI runs below. Written 2026-11-08; the Windows section was extended on 2026-11-09 (M10 step 8).
- Not yet run: the hands-on macOS run (the user's stop after M9's last step) and any Windows or Linux
  window. Every item that waits on them says so; nothing below is rounded up to a pass.

## Class B entries

### `platform.material` (L: `docs/components/platform/material.md`, owner "Luneterm M9 to M11 runs")

Luneterm's numbering after the 2026-11-08 roadmap revision: the runs are M9 (macOS Vibrancy) and M10
(Windows acrylic and Mica, the inactive window, Liquid Glass on macOS, blur and noise against
Lunegit). The owner line in luneui-rs still says "M9 to M11"; M11 no longer exists.

| What the done standard needs | Evidence so far | Status |
|---|---|---|
| macOS `Vibrancy` (NSVisualEffect, light, kept active) on a real window | The app applies it through `material::apply(window, Material::Vibrancy)` for the default Lunegit glass style (`crates/luneterm/src/app.rs`, `apply_material`). A debug launch on this Mac (Apple M1 Pro, 2026-11-08, M9 step 6) returned `Ok`: stderr held only the adapter line and no `glass: ... continuing opaque` line. Nobody looked at the window. | **Run by the user at M10 Stop A (2026-11-09)**: reported ok ("ok i think") with no defect named; the comparison against Lunegit and the dark glass are judged again at step 14 (see "Stop A" below) |
| Windows acrylic and Mica | `material::apply` returned `Ok` for Default (acrylic), Mica, Mica Alt and Off, switched live from the settings page on claude-vm-win with no error and no crash (2026-11-09). **Not judged:** the VM has no 3D acceleration (WARP), so no OS material shows. The glass look needs a real GPU. | **Skipped at Stop A** (item 10, 2026-11-09: "skipped for now"); open until the step 14 glass check on a real GPU |
| The inactive window | Not run. The toolkit no longer draws an inactive solid, so `VibrancyFollowsWindow` changes nothing in the fills; the app maps the Lunegit style to `Vibrancy` (M9 step 4 note). | M10 |
| Liquid Glass on macOS versions without it | Not run. | M10 |
| Blur and noise against Avalonia's defaults | Not run. | M10 |

### `render.surface` (L: `docs/components/render/surface.md`, owner "consumer's window run")

| What the done standard needs | Evidence so far | Status |
|---|---|---|
| Acquire, present, resize, skip while hidden in a consumer's window | The app's frame loop handles all three `Acquired` results (`app.rs`, `redraw`): `Frame` draws, `Skip` sets `ControlFlow::Wait` and shows the window, `Redraw` asks for another redraw; `Occluded` stops redraws and the session keeps pumping without drawing; a lost device rebuilds the stack. The macOS debug launch (2026-11-08) opened the window, ran a login shell, idled at 0.0% CPU and exited 15 ms after the shell's SIGHUP. | **Run by the user at M10 Stop A (2026-11-09)** on the release build: tabs, selection, settings, shell integration and the title were used and reported ok; no defect in the first frame, resize or the window was named ("Stop A" below). Occlusion and minimise were not singled out |
| A pixel gate on a presented frame | Offscreen only. The e2e case E8 draws a real session through the toolkit's `dump` path and compares it to a reference replay: byte-identical on macOS and on Windows CI (1040 x 660), so the scene is stable; presented frames cannot be captured on this Mac (no screen capture). | M10: tried on claude-vm-win (P19), otherwise recorded as not closed |
| Windows window path | **Run on claude-vm-win, 2026-11-09 (M10 step 8)**: the window opened on DX12 with the adapter `Microsoft Basic Render Driver` (WARP), drew, presented, took live resize (maximise, restore, edge drag; the ConPTY followed: 208x46, then 111x28) and the device was never lost. See the Windows section. | Seen on WARP only; a real GPU is the user's stop |

## Toolkit gaps

Each was worked around app-side. Dates are when the evidence was read or produced.

| # | Gap | Evidence | Luneterm's workaround | Suggested upstream change |
|---|---|---|---|---|
| 1 | `gpu::instance` offers no Dx12 shader-compiler choice, and wgpu's default `Auto` loads `dxcompiler.dll` by bare name | `L: crates/lune-ui-wgpu/src/gpu.rs:201-204` builds the descriptor from `new_with_display_handle` or `new_without_display_handle`; `L: crates/lune-ui-wgpu/src/platform/windows.rs:2-4` sets only `dx12.presentation_system`. wgpu 30.0.1: `Dx12Compiler` defaults to `Auto` (`wgpu-types backend.rs`), `Auto` loads the DLL through `libloading::Library::new` (`wgpu-hal dx12/instance.rs`), and `Instance::new` initializes each backend eagerly. A DLL planted in the working directory was loaded by an unhardened process in `crates/luneterm/tests/dll_search_gpu.rs` (CI run 42474728954, 2026-11-08: control loaded the plant, hardened children loaded NULL, GPU initialized) | `lune_pty::restrict_dll_search()` before any winit or wgpu object (`main`, D17) | An option on `gpu::instance` for the compiler (FXC, or DXC from an absolute path), or hardening the search inside the toolkit before the instance is created |
| 2 | `TextCache` loads no system fonts, so emoji and CJK fallback vanish unless the app supplies them | `L: crates/lune-ui-wgpu/src/text_cache.rs:81-99`: "No system font is loaded and the system locale is not read"; this is by design (determinism). 2026-11-08 | `fonts::text_cache` once at start (`fonts.db_mut().load_system_fonts()`, 59 to 64 ms on the Apple M1 Pro, debug) into `TextCache.fonts` | Nothing required. A documented helper (`TextCache::load_system_fonts()`) would save each terminal-like consumer the same code |
| 3 | The scrollbar drawing function is `pub(crate)`; only `thumb_geometry` is public | `L: crates/lune-ui-core/src/widgets/scrollbar.rs:64` (`pub fn thumb_geometry`) against `:125` (`pub(crate) fn scrollbar`). 2026-11-08 | The terminal body is a `Painter::custom` slot, not a scroll area, so the app computes the geometry with `thumb_geometry(Axis::Vertical, ...)`, draws the thumb in the accent colour at alpha 0.55 above the grid, and implements the drag itself (`shell::scroll_thumb`, `ThumbDrag`) | A public bar for custom content (track, thumb, drag delta), so a custom surface gets the same look and behaviour as a scroll area |
| 4 | No public way to say "a raw key consumer owns the keyboard": `Ui::begin` moves focus on a plain Tab and dismisses an open overlay on Escape, `FrameOutput.focused` is the only keyboard-ownership signal, and the one facility that keeps Tab for a widget is not public | `L: crates/lune-ui-core/src/ui.rs:406` (Escape, only with a top overlay that has `dismiss_on_escape`), `:416-419` (Tab steps focus unless the focused widget is the tab taker), `:1114` (`pub(crate) fn take_tab_next_frame`, used by `code_editor.rs:1178`); `L: docs/components/infra/shortcut-routing.md:24,62,112`. 2026-11-08 | Terminal-owned keys never reach the Translator; the app matches its own shortcuts at event time with `Chord::matches` (D4). A terminal slot made with `Painter::custom` has no widget to register as the tab taker anyway | A public tab-taking call for custom widgets, or a per-frame "keyboard owned" flag that skips the Tab and Escape handling |
| 5 | The toolkit's `Key` is too coarse for terminal input | The terminal needs the unmodified key, the text with all modifiers, the keypad flag and the US-QWERTY position of the key (`input/keys.rs`: Lunegit's `TerminalKeyRouter` rules); the toolkit `Key` carries none of them. 2026-11-08 | The app maps winit's `KeyEvent` itself (`KeyEventExtModifierSupplement`) and feeds only non-key events to the Translator | An optional raw payload on `Event::Key` (the winit event's unmodified key, text and location), so a consumer need not tap winit twice |
| 6 | `glass_attributes` and the window helpers give no Option-as-Alt control | The macOS terminal needs `OptionAsAlt::Both` or winit feeds Option+key to the dead-key IME before the router sees it. 2026-11-08 | `window.set_option_as_alt(...)` called by the app after the window exists (`app.rs`, `input::option_as_alt`) | Documented as the consumer's job, or a field on the window attributes |
| 7 | The paste size limit is a silent refusal | `L: crates/lune-ui-winit/src/clipboard.rs:5-19` and `docs/components/platform/clipboard.md:107`: `MAX_PASTE_BYTES` 33,554,432; a longer read gives `ClipboardUnavailable`, with no truncation and no size. 2026-11-08 | The app treats it as "nothing to paste" | Optionally report the size, so a consumer can say why |
| 8 | The tab strip's empty space and the `+` button: the strip puts `+` at its right end (not after the last tab), and its scroll area covers the empty space between the last tab and `+` as the topmost region, which swallows a press although it captures none, so a region the app registers under it never sees the press | `L: crates/lune-ui-core/src/widgets/tab_strip.rs:400` (`new_rect` at `rect.right() - NEW_MARGIN - NEW_SIZE`), `crates/lune-ui-core/src/ui/pointer.rs:37-46` (`topmost` takes the highest region whatever its sense; `senses_capture` is checked on that one only). 2026-11-08 | `shell::draw`: a primary press in the card header with `Painter.hover` `None` (the scroll area senses no hover) or on the app's own header region is an empty press and asks for a window drag (`shell::tests::the_empty_strip_drags_the_window_and_a_tab_does_not`). A double press there does not maximize | `TabStripResponse` reporting a press on empty strip space, or the scroll area registering no region when it has no bars |

## Windows section

### The run on claude-vm-win (M10 step 8, 2026-11-09)

`luneterm.exe` from CI run 42504143753 (the artifact `luneterm-windows-x64`, 14,582,784 bytes, built
from `3fa8cdb`), started in the interactive session through `Win+R` with `LUNETERM_CONFIG_DIR` set
to a scratch folder and stderr to a file, driven over SSH and VNC (vncdotool, 1920x1080). Windows 11
Pro 10.0.26200, no 3D GPU, no `pwsh`: the shell was Windows PowerShell 5.1 (`powershell.exe`). The
files this run made on the VM (`lt-run`, `%APPDATA%\Luneterm`) were removed afterwards.

What was seen, item by item:

| Item | Result |
|---|---|
| Starts on WARP | **Yes.** stderr: `gpu Microsoft Basic Render Driver (Dx12); shell powershell.exe; grid 125x33`. First frame drawn, window shown, no `glass:` error. |
| Typing | **Yes.** ASCII, an e-acute typed by keysym, Backspace, Enter, Up (history), Esc (clears the line), Ctrl+C (interrupted `Start-Sleep 100` within 3 s). PSReadLine colours drew. |
| Shell | Windows PowerShell 5.1 under the integration: the prompt wrapper kept `PS C:\Users\claude>`; the profile and PSReadLine ran. |
| Shell integration (PowerShell) | **Yes.** `shell-integration\pwsh` was written under the config dir; the live directory reached `workspace.json` (`C:\Windows` after `cd`); while `Start-Sleep 40` ran the outline animated (pixels changed along the card's top, bottom and left edges between two captures 3 s apart, none in the body); the close-confirm dialog appeared for a running command (D7: only the 133 marks say it runs) and did not for an idle one. |
| Tabs | **Yes.** `Ctrl+Shift+T`; the `+` button (new tab in the active tab's live directory, `C:\Windows`); `Alt+1`; the close glyph on an idle tab (no dialog); the tab context menu (Close, Close Others, Close All, Rename, Duplicate Tab, Copy Working Directory); Rename (typed in the toolkit's field, saved to `workspace.json`, shown in the title band). `Alt+2` to `Alt+9` were not each tried. |
| Close-confirm | **Yes.** `Ctrl+Shift+W` and the caption close button both asked while a command ran (Cancel kept the tab); with idle tabs the caption close exits and leaves no process. |
| Selection and copy | **Yes.** A drag selected a line (highlight drawn); `Ctrl+Shift+C` then `Ctrl+Shift+V` pasted it at the prompt: the OS clipboard works through the toolkit. |
| Link click | **Yes.** With Shift held: underline and hand cursor over a printed URL; Shift+click opened it in Edge through `ShellExecuteW` (the page loaded, the address bar showed the exact URL). Edge was not running before and was closed after. |
| Settings | **Yes.** `Ctrl+,` opens the page. Color theme to Dracula and Font size +2 reached every open tab and were saved (`palette` and `font_size` in `settings.json`, read by key). Glass style cycled Default, Mica, Mica Alt, Off, Default live with no error. |
| Resize | **Yes.** Maximise (the ConPTY reported 208x46), restore (111x28), a drag of the right edge (the window grew). |
| Workspace restore | **Yes.** Closed and started again: two tabs, the first in `C:\Windows`, the second in `C:\Users\claude`, the active tab and the renamed title back. |
| Default config directory | **Yes.** Without `LUNETERM_CONFIG_DIR` the app used `%APPDATA%\Luneterm`. Assumption A3: `icacls` shows the folder inherits SYSTEM, Administrators and the user only (no Users or Everyone entry). That is one VM account, not a general proof. |
| Caption buttons | **Yes.** Maximise, restore and close worked (close went through close-confirm); minimise is drawn, not clicked. |
| ConPTY input (ledger 10) | The keys above arrived and PSReadLine read them. Not re-measured: the byte table of `docs/conpty-input.md` stands (CI). Not tried: 4-byte UTF-8 (VNC cannot type U+1F600), Ctrl+Z, Ctrl+Space. |
| IME | **Not run.** No input method is installed on the VM. |
| Idle CPU, outline cost (ledger 16) | **Not measured** here (the step 9 hands-on item, `scripts/cpu-sample.sh`). |
| Presented frame against the offscreen dump (P19) | **Not closed.** `WindowSurface` has no `COPY_SRC`, so the only source is a VNC capture of the client area. The live layout matches `shell::Layout` (card 10 px in, strip at 34, caption buttons 46 px each), but the content differs from every dump scene (a live shell, another tab title, no OS material on WARP), so no pixel comparison with a tolerance was made. An opt-in `COPY_SRC` would be a toolkit change (D23). |

Instrument notes for the next run: `key super-r` did not open Run, while `keydown super`, `key r`,
`keyup super` did. TightVNC drops Shift for a lowercase keysym: `key ctrl-shift-t` reached the shell
as Ctrl+T (the terminal printed `^T`), while `keydown ctrl keydown shift key T` opened a tab. That
was the instrument, not the app. A `click 1` must follow a `move` in the same call. The first
capture of a connection can be black; a second one after a move is real.

Findings:

- **CI on windows-latest failed on 13 tests of this milestone's code** (the first CI run of M10's
  steps 1 to 7, `3fa8cdb`): selection tests expecting `\n` (a copy breaks lines with CR LF on
  Windows, by design), workspace and shell-integration tests using `/a`-style paths (not absolute on
  Windows), the integration scripts checked out with CRLF (autocrlf), and `bash -n` over a Windows
  path. All test portability, fixed in `0a39c61` and `e35e540` (the tests, and `.gitattributes`
  pinning LF on the scripts); run 42509338634 at `e35e540` is green on all four jobs. No app
  behaviour changed, so the exe run above (`3fa8cdb`) stands for `e35e540`.
- No toolkit gap was found in this run: the window, the surface, the clipboard, the drawn caption,
  the edge resize and `material::apply` worked on Windows. The two limits are the VM's (no 3D
  hardware, so no glass to look at) and the missing readback (P19).

## Stop A: the user's hands-on run (M10 step 9, macOS, 2026-11-09)

The release build of `782cced` on the user's Mac with their real config directory. Reported
result per item (the checklist is the task's `hands-on-mid.md`):

| Item | Result |
|---|---|
| 1 Tabs, 2 Selection, 4 Settings and restore, 5 Shell integration, 8 Kitty reset, 9 Title | Reported ok ("ok i think"); no defect named. |
| 3 Links | First **not ok**: no underline and no hand with Cmd or Shift held. On the retest it worked with Cmd held; the earlier report expected a hover effect without a modifier. The user then changed the behaviour (`b46c9fc`): underline on hover, a tooltip with the URL and the open hint, a hand only while the modifier is held. |
| 6 Outline CPU (ledger 16) | **Skipped** by the user: no figure. |
| 7 Bell (F3, the dock bounce) | Reported ok with the rest. |
| 10 Windows glass | **Skipped for now**; open until step 14. |
| 11 macOS Liquid Glass, blur and noise against Lunegit | Not stated separately; judged again at step 14. |

Toolkit asks that came out of the run (for a luneui-rs task, not patched here: D23): the strip's
`+` right after the last tab instead of at the strip's right end (gap 8 above), and a tab-switch
animation (the active highlight sliding).

### Earlier: CI evidence (M9)

- **Run 42474728954** (22b0e71, M9 step 7), all four jobs green: the app and the toolkit build on
  `x86_64-pc-windows-msvc` with the git pin and `--locked`. `dll_search_gpu` on windows-latest: the
  control child loaded the planted `dxcompiler.dll`; the hardened children (in the plant's
  directory, and with it on `PATH`) loaded NULL, with `gpu::instance(None)` and `Gpu::new`
  completing, so the runner has an adapter and the hardening does not break GPU initialisation.
  `luneterm.exe` = 13,858,304 bytes, `glass-ui.exe` = 12,567,552 bytes (the spike, retired in
  M10 step 12; artifact `luneterm-windows-x64`).
- **Run 42477613074** (b535456, M9 step 8), all four jobs green: the e2e harness ran 10 cases, 0
  failed, in 46.1 s. E2 to E5 and E9 skipped (they need a Unix tty, `stty`). E1 (round trip through
  ConPTY and the engine) and E7 (exit code 3, the child dies within 2 s of the session dropping)
  passed; E6k key latency median 20 ms; E8 drew the real session to a PNG and it matched the
  reference replay at 1040 x 660. The Windows `luneterm.exe` size is the same 13,858,304 bytes.
- At the time not seen: any window, the adapter name, IME. The window and the adapter were seen on 2026-11-09 (above); the app also started from `Win+R` in the interactive session, so the DLL hardening did not break the GPU start there.

## What worked (P20, so far)

- The toolkit resolved and built from a git rev on all three CI runners, with `Cargo.lock`
  recording both `lune-ui` 0.1.0 (the in-tree spike toolkit, removed in M10 step 12) and 0.2.0
  (luneui-rs) without duplicating wgpu, winit, glyphon or cosmic-text.
- The gallery's composition (event loop with a user event, `glass_attributes`, hidden-then-shown,
  `material::apply`, `gpu::instance`, `Gpu::new`, `WindowSurface`, `FrameRenderer`, `Clipboard::service`,
  `OutputState::apply`) was enough to build the app shell without touching the toolkit.
- The `dump` path (offscreen target and readback) is deterministic: two dumps of the shell are
  byte-identical, and the e2e render matched its reference on macOS and Windows.
- The clipboard (`ClipboardRequest::Read` answered on a later frame, `ui.clipboard_write`) and IME
  (`set_ime_area`, `Event::Ime`) APIs worked as documented in the code paths the unit tests reach;
  real typing and the OS clipboard are for the hands-on run.

## Gaps seen in M10 step 6 (settings page and menus)

- **Tab context menu rows.** The strip's own menu takes `menu_extra` rows as plain text: no glyph
  and no separator before them, unlike `Ui::menu_with`'s rows. The app's `⋯` menu and the
  terminal's menu have both; the tab's menu looks plainer next to them.
- **Tab tooltip under an open context menu.** The strip's hover tooltip for the tab under the
  pointer can stay visible beside the tab's context menu while it is open (`menu-tab` dump). The app
  has no lever on it.
- **Settings page scroll bar and banner.** The page's scroll thumb sits at the viewport's right edge
  (the app keeps its groups 12 px clear of it); the restart banner is a full-width bar over the
  content's last rows rather than docked outside the scroll area.

## Seen in M10 step 4 (link tooltip)

- `Ui::tooltip` fits a link tooltip without a patch: the app registers a `Sense::HOVER` region over
  the link's cells and hangs the tooltip from it (`render.rs`). One small gap: the text is one
  style, so the hint line ("Cmd+click to open") reads as loud as the URL; a dimmer second line
  would need `tooltip_with` drawing its own two runs, which the app has not done.
