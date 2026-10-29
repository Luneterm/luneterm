# TwemojiLunegit.ttf

Bundled colour-emoji font. The artwork is **Twemoji**, taken unmodified from the
[`jdecked/twemoji`](https://github.com/jdecked/twemoji) fork (the actively maintained continuation
of Twitter's Twemoji), release **v17.0.3** (2026-06-01, Unicode Emoji 17).

Lunegit does not ship any upstream Twemoji font binary — no such build exists in the sbix format
macOS requires. This file is generated from the project's SVG sources by
`scripts/build-emoji-font.sh`; see that script for the exact pipeline and
`.claude/docs/terminal-emoji-rendering.md` for why
sbix is the only workable format.

## Attribution

Emoji artwork © 2022–present Jason Sofonia & Justine De Caires, © 2014–2021 Twitter, Inc and other
contributors. Licensed under **CC-BY 4.0**: <https://creativecommons.org/licenses/by/4.0/>

The artwork is what this font contains, so **CC-BY 4.0 is the licence that governs the shipped
file**, and attribution is required. Upstream accepts a mention in an About/Legal section; Lunegit
satisfies this through the in-app licence catalog (`Assets/Licenses/licenses.json`).

Twemoji's own tooling is separately licensed under the MIT License, and `nanoemoji` (the Google
Fonts build tool used to assemble the font) under Apache-2.0. Neither is redistributed here —
they are build-time only and contribute no code to the shipped artefact.

## CC-BY 4.0 summary of obligations

Retain the copyright notice above, the licence link, and an indication of whether changes were
made. **Changes were made:** the SVG artwork was rasterised to 128px PNG bitmaps and packaged into
an OpenType `sbix` table; the artwork itself is unaltered.
