# Test fonts

Deterministic `pyftsubset` subsets used only by `lune-termview` tests, so results do not depend
on installed fonts. **They are test data and are never bundled in the app**, so `licenses/` is
unaffected. Each licence sits beside the fonts: `RobotoMonoNerdFont.LICENSE.txt`,
`NotoSansSymbols2.OFL.txt`, `TwemojiLunegit.LICENSE.md`.

Tool: fontTools 4.58.0 (`pyftsubset`). Re-running a command reproduces the committed file
byte for byte (checked for the Regular subset). `$LF` is Lunegit's
`src/Lunegit.Desktop/Assets/Fonts`; `$SP` holds the full RobotoMono Nerd Font Mono files (the app's bundled copy, see
`THIRD_PARTY_NOTICES.md`).

```bash
RU="U+0020-007E,U+00A0-00FF,U+0300-036F,U+2500-259F,U+E0A0-E0A3,U+E0B0-E0B3,U+F000-F02F"
for w in Regular Bold; do
  pyftsubset $SP/RobotoMonoNerdFontMono-$w.ttf --unicodes="$RU" --layout-features='*' \
    --glyph-names --notdef-outline --name-IDs='*' --output-file=RobotoMonoNerdFontMono-$w.subset.ttf
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
| `RobotoMonoNerdFontMono-Regular.subset.ttf` | 45464 | 413: U+0020-007E, U+00A0-00FF, U+0300-0301, U+0303, U+0309, U+030F, U+0323, U+2500-259F, U+E0A0-E0A3, U+E0B0-E0B3, U+F000-F02F |
| `RobotoMonoNerdFontMono-Bold.subset.ttf` | 45504 | same 413 |
| `NotoSansSymbols2.subset.ttf` | 2576 | U+23FA, U+2605, U+26A0, U+2733, U+273B (all five asked for) |
| `TwemojiLunegit.subset.ttf` | 7940 | U+200D, U+26A0, U+2705, U+FE0F, U+1F3A8, U+1F3FD, U+1F44D, U+1F600 (keeps `sbix`; rasterizes as colour) |

Only the combining marks the source font has survive from U+0300-036F. The Twemoji subset has
no U+2605, U+2733, U+23FA or U+273B on purpose, so those fall back to NotoSansSymbols2. It does
have U+26A0 (as does NotoSansSymbols2); the shaper test for `⚠\u{FE0F}` without an
`emoji_family` observes which face fallback picks.
