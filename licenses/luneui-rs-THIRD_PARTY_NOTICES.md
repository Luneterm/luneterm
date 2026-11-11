# Third-party notices

Every bundled third-party asset or code has a row here and its licence text under `licenses/`.
Paths are repo paths. Luneterm's own notices are in Luneterm, not here.

| Component | Use | Licence |
|---|---|---|
| Avalonia 12.1.2 acrylic noise texture and colour formulas | `crates/lune-ui-wgpu/assets/acrylic-noise.png`, `crates/lune-ui-core/src/acrylic.rs`, comments in `crates/lune-ui-platform/src/material/windows.rs` | MIT — `licenses/avalonia-LICENSE` |
| glyphon 0.12.0 text renderer | `crates/lune-ui-wgpu/src/glyph.rs`, `crates/lune-ui-wgpu/src/glyph/` | MIT — `licenses/glyphon-LICENSE-MIT` (glyphon is MIT OR Apache-2.0 OR Zlib) |
| Pathfinder stem-darkening constants (servo/pathfinder content/src/effects.rs, © 2020 The Pathfinder Project Developers) | `crates/lune-ui-core/src/text/rendering.rs` (`StemDarkening::MACOS`) | Apache-2.0 — `licenses/Apache-2.0.txt` (Pathfinder is Apache-2.0 OR MIT) |
| RznSans | UI font, `apps/gallery/assets/RznSans-Regular.ttf`, `apps/gallery/assets/RznSans-Medium.ttf` | OFL 1.1 — `licenses/RznSans-OFL-1.1.txt` |
| RobotoMono Nerd Font Mono | terminal font, `apps/gallery/assets/RobotoMonoNerdFontMono-Regular.ttf` | Apache 2.0 — `licenses/Apache-2.0.txt`; Nerd Fonts patcher MIT — `licenses/NerdFonts-MIT.txt`; glyph sources below |
| Lunegit sample themes and locale files (commit 33a1dd41) | test fixtures only, `apps/gallery/assets/lunegit/` (copied byte for byte; the gallery's `lunegit_files` test) | MIT OR Apache-2.0, granted by Lunegit's owner (D284) — `licenses/lunegit-fixtures.txt` |

`crates/lune-ui-core/src/icon.rs` holds Codicons codepoints only. The glyphs come from the app's font
(the Nerd Font above in the gallery), so no Codicons font file is bundled.

## Nerd Font glyph sources

The Nerd Font glyphs come from the upstream sources below. The licence list is taken from the
upstream audit, `https://github.com/ryanoasis/nerd-fonts/blob/master/license-audit.md`, fetched
2026-11-04. The audit gives no per-set licence texts, so none are bundled; the licence named here
is the audit's statement.

| Source | Licence |
|---|---|
| Codicons | CC BY 4.0 |
| Font Awesome | CC BY 4.0 |
| Material Design Icons | Apache 2.0 |
| Devicons | MIT |
| Font Awesome Extension | MIT |
| IEC Power Symbols | MIT |
| Seti-UI | MIT |
| Octicons | MIT |
| Powerline Extra Symbols | MIT |
| Pomicons | OFL 1.1 |
| Weather Icons | OFL 1.1 |
| Font Logos | "Unlicensed" (as stated in the audit) |
| Powerline Symbols | "Free License" (as stated in the audit) |
