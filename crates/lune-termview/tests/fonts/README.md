# Test fonts

Deterministic `pyftsubset` subsets used only by `lune-termview` tests, so results do not depend on
installed fonts. **They are test data and are never bundled in the app** (the app bundles the full
faces from the app's `assets` directory), so `licenses/` is unaffected. Each licence sits beside the
fonts: `RznMono.OFL.txt`, `RobotoMonoNerdFont.LICENSE.txt`, `NotoSansSymbols2.OFL.txt`,
`TwemojiLunegit.LICENSE.md`.

Tool: fontTools 4.58.0 (`pyftsubset`); the RznMono subsets of 1.001 were cut with 4.65.0, which
reproduces the 1.000 Regular subset of 4.58.0 byte for byte (2026-11-15); those of 1.003 with 4.58.0
(2026-11-19). Re-running a command
reproduces the committed file byte for byte (checked for the RznMono Regular subset, 2026-11-14). `$LF` is Lunegit's
`src/Lunegit.Desktop/Assets/Fonts`; `$A` is the app crate's `assets` directory, with its bundled copies of
RznMono 1.003 (`RznMono-{Regular,Bold,Italic,BoldItalic}.ttf`, from `fonts/rznmono/desktop` in the
RznSans repository at tag `rznmono-v1.003`).

The RznMono subsets are the terminal font. `--layout-features='*'` keeps the `calt` feature, and
closure pulls in the ligature glyphs (the spacer `LIG` and the 128 `.liga` glyphs, every one of
them built from the characters kept), so the ligature tests and goldens run on them. Every
outline, advance and hint of the earlier 0.007 subsets is unchanged in 1.000, so the goldens
drawn before the swap stayed byte-identical. 1.001 changes only the outlines of 35 accented
letters (mark direction: À, Ž, Ě and others; no other table); every golden but `ligatures@2x`
(re-blessed for the luneui-rs v0.6.1 rasterizer fix, handoff gap 9) stayed byte-identical. 1.002
reverses the contour direction of Nerd icons only (none in these subsets): every subset outline is
unchanged, only the version tables differ, and no golden changed. 1.003 adds 130 code points (Geometric Shapes
U+25A0-25FF and 36 TUI symbols), none in these subsets: against the 1.001 subsets only `OS/2`, `head`
and `name` differ (glyph order, `glyf`, `hmtx` and cmap equal; checked 2026-11-19).

The RobotoMono subsets hold **private-use icons only** and are **test fixtures**, never bundled:
the app dropped RobotoMono Nerd Font (rznmono-1000 D1; RznMono 1.000 has the Nerd icons), but the
RznMono subsets carry none of U+F000-F02F, so these keep proving the generic `icon_family`
mechanism (a second family for the icons the primary lacks; RznMono cannot be both, an
`icon_family` equal to the family is skipped). A Latin character missing from a RznMono subset
shows as tofu in a golden and cannot pass quietly through them. They were cut from RobotoMono Nerd
Font Mono as bundled before rznmono-1000 (`$R` below: the app crate's `assets` directory at
commit `e5a8e01`; its licence and notice are in that commit's `THIRD_PARTY_NOTICES.md`).

```bash
RU="U+0020-007E,U+00A0-00FF,U+0300-036F,U+2500-259F,U+E0A0-E0A3,U+E0B0-E0B3"
for w in Regular Bold Italic BoldItalic; do
  pyftsubset $A/RznMono-$w.ttf --unicodes="$RU" --layout-features='*' \
    --glyph-names --notdef-outline --name-IDs='*' --output-file=RznMono-$w.subset.ttf
done
for w in Regular Bold; do
  pyftsubset $R/RobotoMonoNerdFontMono-$w.ttf --unicodes="U+F000-F02F,U+F07B" --layout-features='*' \
    --glyph-names --notdef-outline --name-IDs='*' --output-file=RobotoMonoNerdFontMono-$w.pua.subset.ttf
done
pyftsubset "$LF/NotoSansSymbols2-Regular.ttf" \
  --unicodes="U+2733,U+2605,U+23FA,U+26A0,U+273B" --layout-features='*' --notdef-outline \
  --name-IDs='*' --output-file=NotoSansSymbols2.subset.ttf
pyftsubset "$LF/TwemojiLunegit.ttf" \
  --unicodes="U+1F3A8,U+2705,U+1F44D,U+1F3FD,U+1F600,U+26A0,U+FE0F,U+200D" --layout-features='*' \
  --notdef-outline --name-IDs='*' --output-file=TwemojiLunegit.subset.ttf
```

| file | bytes | codepoints present (cmap) |
|---|---|---|
| `RznMono-Regular.subset.ttf` | 131184 | 365: U+0020-007E, U+00A0-00FF, U+0300-0301, U+0303, U+0309, U+030F, U+0323, U+2500-259F, U+E0A0-E0A3, U+E0B0-E0B3; GSUB `calt`, `cv01`; 128 `.liga` glyphs |
| `RznMono-Bold.subset.ttf` | 135800 | same |
| `RznMono-Italic.subset.ttf` | 133048 | same |
| `RznMono-BoldItalic.subset.ttf` | 138756 | same |
| `RobotoMonoNerdFontMono-Regular.pua.subset.ttf` | 11280 | 49: U+F000-F02F, U+F07B |
| `RobotoMonoNerdFontMono-Bold.pua.subset.ttf` | 11336 | same 49 |
| `NotoSansSymbols2.subset.ttf` | 2576 | U+23FA, U+2605, U+26A0, U+2733, U+273B (all five asked for) |
| `TwemojiLunegit.subset.ttf` | 7940 | U+200D, U+26A0, U+2705, U+FE0F, U+1F3A8, U+1F3FD, U+1F44D, U+1F600 (keeps `sbix`; rasterizes as colour) |

Only the combining marks the source font has survive from U+0300-036F (RznMono has six). The Twemoji subset has
no U+2605, U+2733, U+23FA or U+273B on purpose, so those fall back to NotoSansSymbols2. It does
have U+26A0 (as does NotoSansSymbols2); the shaper test for `⚠\u{FE0F}` without an
`emoji_family` observes which face fallback picks.
