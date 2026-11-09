# Test fonts

Deterministic `pyftsubset` subsets used only by `lune-termview` tests, so results do not depend on
installed fonts. **They are test data and are never bundled in the app** (the app bundles the full
faces from the app's `assets` directory), so `licenses/` is unaffected. Each licence sits beside the
fonts: `RznMono.OFL.txt`, `RobotoMonoNerdFont.LICENSE.txt`, `NotoSansSymbols2.OFL.txt`,
`TwemojiLunegit.LICENSE.md`.

Tool: fontTools 4.58.0 (`pyftsubset`). Re-running a command reproduces the committed file
byte for byte (checked for the RznMono Regular subset). `$LF` is Lunegit's
`src/Lunegit.Desktop/Assets/Fonts`; `$A` is the app crate's `assets` directory, with its bundled copies of
RznMono 0.007 (`RznMono-{Regular,Bold,Italic,BoldItalic}.ttf`, from `fonts/rznmono/desktop` in the
RznSans repository) and RobotoMono Nerd Font Mono (see `THIRD_PARTY_NOTICES.md`).

The RznMono subsets are the terminal font. The RobotoMono subsets hold **private-use icons only**:
they are the app's fallback face for the Nerd icons RznMono does not have (M10 D9a), so a Latin
character missing from a RznMono subset shows as tofu in a golden and cannot pass quietly through
the fallback.

```bash
RU="U+0020-007E,U+00A0-00FF,U+0300-036F,U+2500-259F,U+E0A0-E0A3,U+E0B0-E0B3"
for w in Regular Bold Italic BoldItalic; do
  pyftsubset $A/RznMono-$w.ttf --unicodes="$RU" --layout-features='*' \
    --glyph-names --notdef-outline --name-IDs='*' --output-file=RznMono-$w.subset.ttf
done
for w in Regular Bold; do
  pyftsubset $A/RobotoMonoNerdFontMono-$w.ttf --unicodes="U+F000-F02F,U+F07B" --layout-features='*' \
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
| `RznMono-Regular.subset.ttf` | 69632 | 365: U+0020-007E, U+00A0-00FF, U+0300-0301, U+0303, U+0309, U+030F, U+0323, U+2500-259F, U+E0A0-E0A3, U+E0B0-E0B3 |
| `RznMono-Bold.subset.ttf` | 71668 | same 365 |
| `RznMono-Italic.subset.ttf` | 71628 | same 365 |
| `RznMono-BoldItalic.subset.ttf` | 74676 | same 365 |
| `RobotoMonoNerdFontMono-Regular.pua.subset.ttf` | 11280 | 49: U+F000-F02F, U+F07B |
| `RobotoMonoNerdFontMono-Bold.pua.subset.ttf` | 11336 | same 49 |
| `NotoSansSymbols2.subset.ttf` | 2576 | U+23FA, U+2605, U+26A0, U+2733, U+273B (all five asked for) |
| `TwemojiLunegit.subset.ttf` | 7940 | U+200D, U+26A0, U+2705, U+FE0F, U+1F3A8, U+1F3FD, U+1F44D, U+1F600 (keeps `sbix`; rasterizes as colour) |

Only the combining marks the source font has survive from U+0300-036F (RznMono has six). The Twemoji subset has
no U+2605, U+2733, U+23FA or U+273B on purpose, so those fall back to NotoSansSymbols2. It does
have U+26A0 (as does NotoSansSymbols2); the shaper test for `⚠\u{FE0F}` without an
`emoji_family` observes which face fallback picks.
