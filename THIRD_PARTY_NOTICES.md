# Third-party notices

| Component | Use | Licence |
|---|---|---|
| xterm.js 6.0.0 (`f447274`) | `lune-vt` is a port of its `src/common` | MIT — `licenses/xterm.js-LICENSE` |
| XTerm.NET 1.0.14 (Tom Laird-McConnell), via Lunegit | `crates/lune-vt/src/input/legacy_key_encoder.rs` is a port of Lunegit's `XtermVtInputEncoder.GenerateKeyInput` / `GenerateCharInput`, which Lunegit's own header calls a faithful port of XTerm.NET 1.0.14's `KeyboardInputGenerator` | MIT — `licenses/XTerm.NET-LICENSE` |
| Avalonia 12.1.2 acrylic noise texture and colour formulas | inside the luneui-rs toolkit the app links (the `lune-ui-wgpu` noise texture and the `lune-ui-core` acrylic code; luneui-rs's own notice is `licenses/luneui-rs-THIRD_PARTY_NOTICES.md`) | MIT — `licenses/avalonia-LICENSE` |
| RznMono 0.007 | bundled terminal font (app): Regular, Bold, Italic, Bold Italic | OFL 1.1 — `licenses/RznMono.OFL.txt` (a copy of `crates/luneterm/assets/RznMono.OFL.txt`) |
| RobotoMono Nerd Font Mono | bundled in the app as the fallback for Nerd icons RznMono does not have, and as a terminal font option | Apache 2.0 — `licenses/RobotoMonoNerdFont.LICENSE.txt` (a copy of `crates/luneterm/assets/RobotoMonoNerdFont.LICENSE.txt`) |
| RznSans | bundled UI font (app) | OFL 1.1 — `licenses/RznSans.OFL.txt` (a copy of `crates/luneterm/assets/RznSans.OFL.txt`) |
| luneui-rs (git pin `5216931`) | the UI toolkit: windowing glue, widgets, glass, text, the frame renderer | MIT OR Apache-2.0 — `licenses/luneui-rs-LICENSE-MIT`, `licenses/luneui-rs-LICENSE-APACHE`; its own third-party notices: `licenses/luneui-rs-THIRD_PARTY_NOTICES.md` |
| Every Rust crate the shipped binaries are built from | the dependencies of `luneterm` and `luneterm-ffi` | each its own licence — `licenses/THIRD_PARTY_CRATES.txt`, generated from `cargo metadata --locked` by `scripts/gen-licenses.py` (`--check` runs in CI; it fails on a package with no text, an SPDX id with no text in `licenses/spdx/`, or a stale file) |

Luneterm itself is MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; copies in `licenses/` as
`luneterm-LICENSE-MIT` and `luneterm-LICENSE-APACHE`, which travel with the Windows exe in CI's
`luneterm-windows-x64` artifact).

Lunegit itself has no entry: the other Lunegit-derived code in `lune-vt` (the
protocol-aware key encoder, the kitty keyboard / modifyOtherKeys / XTVERSION / VS16 / OSC 52 /
OSC 7, 9;9, 9;4 and 133 extensions, the paste sanitizer; `docs/porting-xterm.md`, "Ported from Lunegit") is
ported from Lunegit, which this project treats as the owner's own code (the M7 plan's assumption;
no Lunegit licence file was checked). The `Lunegit.*.dll` files that `tools/lunegit-oracle` loads to
generate the input goldens are read from the owner's local Lunegit build, never copied into this
repository. The XTerm.NET row is there because Lunegit's `XtermVtInputEncoder.cs` header says that
file is a port of XTerm.NET's code; the licence text is the file at
`https://github.com/tomlm/XTerm.NET` (`LICENSE`, fetched 2026-11-02 for this entry), the licence
expression in the 1.0.14 package's `.nuspec` is MIT. Whether that port copies expression or only
behaviour was not examined.
