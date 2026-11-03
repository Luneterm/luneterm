# Third-party notices

| Component | Use | Licence |
|---|---|---|
| xterm.js 6.0.0 (`f447274`) | `lune-vt` is a port of its `src/common` | MIT — `licenses/xterm.js-LICENSE` |
| XTerm.NET 1.0.14 (Tom Laird-McConnell), via Lunegit | `crates/lune-vt/src/input/legacy_key_encoder.rs` is a port of Lunegit's `XtermVtInputEncoder.GenerateKeyInput` / `GenerateCharInput`, which Lunegit's own header calls a faithful port of XTerm.NET 1.0.14's `KeyboardInputGenerator` | MIT — `licenses/XTerm.NET-LICENSE` |
| Avalonia 12.1.2 acrylic noise texture and colour formulas | `crates/lune-ui/assets/acrylic-noise.png`, `crates/lune-ui/src/acrylic.rs` | MIT — `licenses/avalonia-LICENSE` |
| RobotoMono Nerd Font Mono | bundled terminal font (spike) | Apache 2.0 — `spikes/glass-ui/assets/RobotoMonoNerdFont.LICENSE.txt` |
| RznSans | bundled UI font (spike) | OFL 1.1 — `spikes/glass-ui/assets/RznSans.OFL.txt` |

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
