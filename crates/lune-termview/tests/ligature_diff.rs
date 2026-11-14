//! The segment-equivalence premise behind ligature runs (plan D4): shaping each maximal run of
//! ASCII punctuation alone (`Shaper::ligate`) gives the glyphs that harfrust gives for the whole
//! line. Seeded random lines over letters, digits, space and the 26 punctuation chars, on the
//! four RznMono test subsets.
//!
//! `LUNE_TERMVIEW_LIG_DIFF_SEEDS=<n>` runs seeds 1..=n (lines per style, default 150),
//! `LUNE_TERMVIEW_LIG_DIFF_SEED=<n>` replays one seed (all four styles). The env knobs live here
//! and not in `src` because `scripts/check-lib-boundaries.sh` rule 3 keeps `src` free of
//! `env::var`. The builder-level twin with fixed lines is `build::lig_tests`.

use cosmic_text::{FontSystem, fontdb};
use lune_termview::{Shaper, TermFonts};
use std::sync::Arc;

const FACES: [&[u8]; 4] = [
    include_bytes!("fonts/RznMono-Regular.subset.ttf"),
    include_bytes!("fonts/RznMono-Bold.subset.ttf"),
    include_bytes!("fonts/RznMono-Italic.subset.ttf"),
    include_bytes!("fonts/RznMono-BoldItalic.subset.ttf"),
];

/// The ASCII punctuation RznMono's `calt` rules look at.
const PUNCT: &str = "!#$%&(*+-./:;<=>?@[]^_{|}~";

/// One xorshift64 stream per (seed, style): the same seed always rebuilds the same line.
fn rng(seed: u64, style: u64) -> impl FnMut() -> u64 {
    // splitmix64 finaliser: a seed's low bits spread over the whole state; never zero
    let mut z = seed
        .wrapping_mul(4)
        .wrapping_add(style)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let mut state = (z ^ (z >> 31)) | 1;
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    }
}

/// `text` shaped as one string with harfrust, default features: (glyph id, cluster) per glyph.
fn whole_line(font: &cosmic_text::Font, text: &str) -> Vec<(u32, u32)> {
    let mut ub = harfrust::UnicodeBuffer::new();
    ub.push_str(text);
    ub.guess_segment_properties();
    ub.set_direction(harfrust::Direction::LeftToRight);
    let out = font.shaper().shape(ub, &[]);
    out.glyph_infos()
        .iter()
        .map(|i| (i.glyph_id, i.cluster))
        .collect()
}

#[test]
fn substitution_from_runs_equals_shaping_the_whole_line() {
    let alphabet: Vec<char> = ('a'..='z')
        .chain('A'..='Z')
        .chain('0'..='9')
        .chain(" ".chars())
        .chain(PUNCT.chars())
        .chain(PUNCT.chars())
        .chain(PUNCT.chars())
        .collect();
    let replay = std::env::var("LUNE_TERMVIEW_LIG_DIFF_SEED").is_ok();
    let seeds: Vec<u64> = match std::env::var("LUNE_TERMVIEW_LIG_DIFF_SEED") {
        Ok(seed) => vec![
            seed.parse()
                .expect("LUNE_TERMVIEW_LIG_DIFF_SEED is a number"),
        ],
        Err(_) => {
            let n = std::env::var("LUNE_TERMVIEW_LIG_DIFF_SEEDS")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(150);
            (1..=n).collect()
        }
    };

    let mut db = fontdb::Database::new();
    let mut family = String::new();
    for face in FACES {
        let ids = db.load_font_source(fontdb::Source::Binary(Arc::new(face)));
        family = db.face(ids[0]).expect("face").families[0].0.clone();
    }
    let mut fs = FontSystem::new_with_locale_and_db("en-US".to_owned(), db);
    let fonts = TermFonts {
        family,
        size: 13.0,
        line_height: 17.0,
        emoji_family: None,
        icon_family: None,
        ligatures: true,
    };
    let mut shaper = Shaper::new();

    let (mut lines, mut ligated) = (0usize, 0usize);
    for (style, (bold, italic)) in [(false, false), (true, false), (false, true), (true, true)]
        .into_iter()
        .enumerate()
    {
        let g = shaper.shape(&mut fs, &fonts, "a", bold, italic).glyphs[0];
        let (font_id, weight) = (g.font_id, g.weight);
        let font = fs.get_font(font_id, weight).expect("font");
        for &seed in &seeds {
            let mut next = rng(seed, style as u64);
            let len = 1 + (next() % 60) as usize;
            let line: Vec<char> = (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            let text: String = line.iter().collect();
            let label = format!("seed {seed} bold={bold} italic={italic} {text:?}");

            // each cell on its own, then each maximal punctuation run (2+) from one shaping
            let mut drawn: Vec<u16> = line
                .iter()
                .map(|ch| {
                    let s = shaper.shape(&mut fs, &fonts, &ch.to_string(), bold, italic);
                    s.glyphs.first().map_or(0, |g| g.glyph_id)
                })
                .collect();
            let own = drawn.clone();
            let mut k = 0;
            while k < line.len() {
                if !line[k].is_ascii_punctuation() {
                    k += 1;
                    continue;
                }
                let a = k;
                while k < line.len() && line[k].is_ascii_punctuation() {
                    k += 1;
                }
                if k - a < 2 {
                    continue;
                }
                let run: String = line[a..k].iter().collect();
                if let Some(ids) =
                    shaper.ligate(&mut fs, &fonts, font_id, weight, &run, bold, italic)
                {
                    assert_eq!(ids.len(), k - a, "{label}");
                    drawn[a..k].copy_from_slice(ids);
                }
            }

            let whole = whole_line(&font, &text);
            assert_eq!(whole.len(), line.len(), "{label}");
            for (k, ch) in line.iter().enumerate() {
                assert_eq!(whole[k].1 as usize, k, "{label}");
                if *ch != ' ' {
                    assert_eq!(u32::from(drawn[k]), whole[k].0, "{label} cell {k}");
                }
                ligated += usize::from(drawn[k] != own[k]);
            }
            lines += 1;
        }
    }
    if !replay {
        assert!(lines >= 600, "{lines} lines");
        assert!(ligated > 100, "the lines hold ligatures: {ligated}");
    }
}
