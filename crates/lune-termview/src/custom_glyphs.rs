//! Port of xterm.js addons/addon-webgl/src/CustomGlyphs.ts @ f447274.
//!
//! Box drawing, block elements, shades and powerline glyphs drawn procedurally at the device cell
//! size, so they join across cells regardless of the font. Upstream draws them into its texture
//! atlas; here [`crate::atlas::SwashRaster`] calls [`try_draw_custom_char`] on a
//! [`GlyphCanvas`] and stores the A8 result in the mask page.
//!
//! The tables, constants and comments follow upstream order. Deviations from upstream are in
//! `// LUNETERM:` blocks:
//!
//! 1. D17: the stroke snap in [`translate_args`] ([`snap`]).
//! 2. A pattern fill on a colourless mask: coverage = pattern x 255, no `CanvasPattern` cache
//!    (the atlas caches the result).
//! 3. [`has_custom_char`].
//! 4. A coordinate that parses to NaN (or is missing) skips its instruction, where upstream
//!    would draw nothing for it; the `throw` paths become a skip too.
//!
//! Upstream has no test file for this module; the tests below are original.

use crate::glyph_canvas::GlyphCanvas;

/// `IBlockVector`: x, y, w, h in eighths of the cell.
pub type BlockVector = (u8, u8, u8, u8);

/// `DrawFunctionDefinition`: `xp` is the percentage of 15% of the x axis, `yp` the same on the y
/// axis (so the double lines are as far apart in both directions).
pub type DrawFunctionDefinition = fn(f64, f64) -> String;

/// A box drawing definition: an SVG path string, or a function producing one.
#[derive(Clone, Copy)]
pub enum Def {
    Static(&'static str),
    Func(DrawFunctionDefinition),
}

impl std::fmt::Debug for Def {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Def::Static(s) => write!(f, "Static({s:?})"),
            Def::Func(_) => f.write_str("Func(..)"),
        }
    }
}

/// `Style`.
const NORMAL: u32 = 1;
const BOLD: u32 = 3;

macro_rules! nrm {
    ($s:expr) => {
        &[(NORMAL, Def::Static($s))]
    };
}
macro_rules! bld {
    ($s:expr) => {
        &[(BOLD, Def::Static($s))]
    };
}
macro_rules! mix {
    ($n:expr, $b:expr) => {
        &[(NORMAL, Def::Static($n)), (BOLD, Def::Static($b))]
    };
}
macro_rules! func {
    ($f:expr) => {
        &[(NORMAL, Def::Func($f))]
    };
}
/// The four curved corners: a line to `0.5 +- cw/ch * 0.5`, then a curve to the side midpoint.
macro_rules! curve {
    ($sign:tt, $start:literal, $end:literal) => {
        &[(
            NORMAL,
            Def::Func(|_xp, yp| {
                let k = 0.5 $sign (yp / 0.15 * 0.5);
                format!(concat!($start, " L.5,{} C.5,{},.5,.5,", $end, ",.5"), k, k)
            }),
        )]
    };
}

/// `.5 -/+ xp` and `.5 -/+ yp`: the two lines of a double border.
fn quad(xp: f64, yp: f64) -> [f64; 4] {
    [0.5 - xp, 0.5 + xp, 0.5 - yp, 0.5 + yp]
}

/// `blockElementDefinitions`.
pub const BLOCK_ELEMENTS: &[(char, &[BlockVector])] = &[
    // Block elements (0x2580-0x2590)
    ('▀', &[(0, 0, 8, 4)]), // UPPER HALF BLOCK
    ('▁', &[(0, 7, 8, 1)]), // LOWER ONE EIGHTH BLOCK
    ('▂', &[(0, 6, 8, 2)]), // LOWER ONE QUARTER BLOCK
    ('▃', &[(0, 5, 8, 3)]), // LOWER THREE EIGHTHS BLOCK
    ('▄', &[(0, 4, 8, 4)]), // LOWER HALF BLOCK
    ('▅', &[(0, 3, 8, 5)]), // LOWER FIVE EIGHTHS BLOCK
    ('▆', &[(0, 2, 8, 6)]), // LOWER THREE QUARTERS BLOCK
    ('▇', &[(0, 1, 8, 7)]), // LOWER SEVEN EIGHTHS BLOCK
    ('█', &[(0, 0, 8, 8)]), // FULL BLOCK
    ('▉', &[(0, 0, 7, 8)]), // LEFT SEVEN EIGHTHS BLOCK
    ('▊', &[(0, 0, 6, 8)]), // LEFT THREE QUARTERS BLOCK
    ('▋', &[(0, 0, 5, 8)]), // LEFT FIVE EIGHTHS BLOCK
    ('▌', &[(0, 0, 4, 8)]), // LEFT HALF BLOCK
    ('▍', &[(0, 0, 3, 8)]), // LEFT THREE EIGHTHS BLOCK
    ('▎', &[(0, 0, 2, 8)]), // LEFT ONE QUARTER BLOCK
    ('▏', &[(0, 0, 1, 8)]), // LEFT ONE EIGHTH BLOCK
    ('▐', &[(4, 0, 4, 8)]), // RIGHT HALF BLOCK
    // Block elements (0x2594-0x2595)
    ('▔', &[(0, 0, 8, 1)]), // UPPER ONE EIGHTH BLOCK
    ('▕', &[(7, 0, 1, 8)]), // RIGHT ONE EIGHTH BLOCK
    // Terminal graphic characters (0x2596-0x259F)
    ('▖', &[(0, 4, 4, 4)]),               // QUADRANT LOWER LEFT
    ('▗', &[(4, 4, 4, 4)]),               // QUADRANT LOWER RIGHT
    ('▘', &[(0, 0, 4, 4)]),               // QUADRANT UPPER LEFT
    ('▙', &[(0, 0, 4, 8), (0, 4, 8, 4)]), // QUADRANT UPPER LEFT AND LOWER LEFT AND LOWER RIGHT
    ('▚', &[(0, 0, 4, 4), (4, 4, 4, 4)]), // QUADRANT UPPER LEFT AND LOWER RIGHT
    ('▛', &[(0, 0, 4, 8), (4, 0, 4, 4)]), // QUADRANT UPPER LEFT AND UPPER RIGHT AND LOWER LEFT
    ('▜', &[(0, 0, 8, 4), (4, 0, 4, 8)]), // QUADRANT UPPER LEFT AND UPPER RIGHT AND LOWER RIGHT
    ('▝', &[(4, 0, 4, 4)]),               // QUADRANT UPPER RIGHT
    ('▞', &[(4, 0, 4, 4), (0, 4, 4, 4)]), // QUADRANT UPPER RIGHT AND LOWER LEFT
    ('▟', &[(4, 0, 4, 8), (0, 4, 8, 4)]), // QUADRANT UPPER RIGHT AND LOWER LEFT AND LOWER RIGHT
    // VERTICAL ONE EIGHTH BLOCK-2 through VERTICAL ONE EIGHTH BLOCK-7
    ('\u{1FB70}', &[(1, 0, 1, 8)]),
    ('\u{1FB71}', &[(2, 0, 1, 8)]),
    ('\u{1FB72}', &[(3, 0, 1, 8)]),
    ('\u{1FB73}', &[(4, 0, 1, 8)]),
    ('\u{1FB74}', &[(5, 0, 1, 8)]),
    ('\u{1FB75}', &[(6, 0, 1, 8)]),
    // HORIZONTAL ONE EIGHTH BLOCK-2 through HORIZONTAL ONE EIGHTH BLOCK-7
    ('\u{1FB76}', &[(0, 1, 8, 1)]),
    ('\u{1FB77}', &[(0, 2, 8, 1)]),
    ('\u{1FB78}', &[(0, 3, 8, 1)]),
    ('\u{1FB79}', &[(0, 4, 8, 1)]),
    ('\u{1FB7A}', &[(0, 5, 8, 1)]),
    ('\u{1FB7B}', &[(0, 6, 8, 1)]),
    // LEFT AND LOWER ONE EIGHTH BLOCK
    ('\u{1FB7C}', &[(0, 0, 1, 8), (0, 7, 8, 1)]),
    // LEFT AND UPPER ONE EIGHTH BLOCK
    ('\u{1FB7D}', &[(0, 0, 1, 8), (0, 0, 8, 1)]),
    // RIGHT AND UPPER ONE EIGHTH BLOCK
    ('\u{1FB7E}', &[(7, 0, 1, 8), (0, 0, 8, 1)]),
    // RIGHT AND LOWER ONE EIGHTH BLOCK
    ('\u{1FB7F}', &[(7, 0, 1, 8), (0, 7, 8, 1)]),
    // UPPER AND LOWER ONE EIGHTH BLOCK
    ('\u{1FB80}', &[(0, 0, 8, 1), (0, 7, 8, 1)]),
    // HORIZONTAL ONE EIGHTH BLOCK-1358
    (
        '\u{1FB81}',
        &[(0, 0, 8, 1), (0, 2, 8, 1), (0, 4, 8, 1), (0, 7, 8, 1)],
    ),
    // UPPER ONE QUARTER BLOCK
    ('\u{1FB82}', &[(0, 0, 8, 2)]),
    // UPPER THREE EIGHTHS BLOCK
    ('\u{1FB83}', &[(0, 0, 8, 3)]),
    // UPPER FIVE EIGHTHS BLOCK
    ('\u{1FB84}', &[(0, 0, 8, 5)]),
    // UPPER THREE QUARTERS BLOCK
    ('\u{1FB85}', &[(0, 0, 8, 6)]),
    // UPPER SEVEN EIGHTHS BLOCK
    ('\u{1FB86}', &[(0, 0, 8, 7)]),
    // RIGHT ONE QUARTER BLOCK
    ('\u{1FB87}', &[(6, 0, 2, 8)]),
    // RIGHT THREE EIGHTHS B0OCK
    ('\u{1FB88}', &[(5, 0, 3, 8)]),
    // RIGHT FIVE EIGHTHS BL0CK
    ('\u{1FB89}', &[(3, 0, 5, 8)]),
    // RIGHT THREE QUARTERS 0LOCK
    ('\u{1FB8A}', &[(2, 0, 6, 8)]),
    // RIGHT SEVEN EIGHTHS B0OCK
    ('\u{1FB8B}', &[(1, 0, 7, 8)]),
    // CHECKER BOARD FILL
    (
        '\u{1FB95}',
        &[
            (0, 0, 2, 2),
            (4, 0, 2, 2),
            (2, 2, 2, 2),
            (6, 2, 2, 2),
            (0, 4, 2, 2),
            (4, 4, 2, 2),
            (2, 6, 2, 2),
            (6, 6, 2, 2),
        ],
    ),
    // INVERSE CHECKER BOARD FILL
    (
        '\u{1FB96}',
        &[
            (2, 0, 2, 2),
            (6, 0, 2, 2),
            (0, 2, 2, 2),
            (4, 2, 2, 2),
            (2, 4, 2, 2),
            (6, 4, 2, 2),
            (0, 6, 2, 2),
            (4, 6, 2, 2),
        ],
    ),
    // HEAVY HORIZONTAL FILL (upper middle and lower one quarter block)
    ('\u{1FB97}', &[(0, 2, 8, 2), (0, 6, 8, 2)]),
];

/// `patternCharacterDefinitions`: the repeating pixel pattern of the shade characters.
pub const PATTERN_CHARACTERS: &[(char, &[&[u8]])] = &[
    // LIGHT SHADE (25%)
    (
        '░',
        &[&[1, 0, 0, 0], &[0, 0, 0, 0], &[0, 0, 1, 0], &[0, 0, 0, 0]],
    ),
    // MEDIUM SHADE (50%)
    ('▒', &[&[1, 0], &[0, 0], &[0, 1], &[0, 0]]),
    // DARK SHADE (75%)
    ('▓', &[&[0, 1], &[1, 1], &[1, 0], &[1, 1]]),
];

macro_rules! top_to_bottom {
    () => {
        "M.5,0 L.5,1"
    };
}
/// │
#[allow(dead_code)]
const TOP_TO_BOTTOM: &str = top_to_bottom!();
macro_rules! left_to_right {
    () => {
        "M0,.5 L1,.5"
    };
}
/// ─
#[allow(dead_code)]
const LEFT_TO_RIGHT: &str = left_to_right!();
macro_rules! top_to_right {
    () => {
        "M.5,0 L.5,.5 L1,.5"
    };
}
/// └
#[allow(dead_code)]
const TOP_TO_RIGHT: &str = top_to_right!();
macro_rules! top_to_left {
    () => {
        "M.5,0 L.5,.5 L0,.5"
    };
}
/// ┘
#[allow(dead_code)]
const TOP_TO_LEFT: &str = top_to_left!();
macro_rules! left_to_bottom {
    () => {
        "M0,.5 L.5,.5 L.5,1"
    };
}
/// ┐
#[allow(dead_code)]
const LEFT_TO_BOTTOM: &str = left_to_bottom!();
macro_rules! right_to_bottom {
    () => {
        "M0.5,1 L.5,.5 L1,.5"
    };
}
/// ┌
#[allow(dead_code)]
const RIGHT_TO_BOTTOM: &str = right_to_bottom!();
macro_rules! middle_to_top {
    () => {
        "M.5,.5 L.5,0"
    };
}
/// ╵
#[allow(dead_code)]
const MIDDLE_TO_TOP: &str = middle_to_top!();
macro_rules! middle_to_left {
    () => {
        "M.5,.5 L0,.5"
    };
}
/// ╴
#[allow(dead_code)]
const MIDDLE_TO_LEFT: &str = middle_to_left!();
macro_rules! middle_to_right {
    () => {
        "M.5,.5 L1,.5"
    };
}
/// ╶
#[allow(dead_code)]
const MIDDLE_TO_RIGHT: &str = middle_to_right!();
macro_rules! middle_to_bottom {
    () => {
        "M.5,.5 L.5,1"
    };
}
/// ╷
#[allow(dead_code)]
const MIDDLE_TO_BOTTOM: &str = middle_to_bottom!();
macro_rules! t_top {
    () => {
        "M0,.5 L1,.5 M.5,.5 L.5,0"
    };
}
/// ┴
#[allow(dead_code)]
const T_TOP: &str = t_top!();
macro_rules! t_left {
    () => {
        "M.5,0 L.5,1 M.5,.5 L0,.5"
    };
}
/// ┤
#[allow(dead_code)]
const T_LEFT: &str = t_left!();
macro_rules! t_right {
    () => {
        "M.5,0 L.5,1 M.5,.5 L1,.5"
    };
}
/// ├
#[allow(dead_code)]
const T_RIGHT: &str = t_right!();
macro_rules! t_bottom {
    () => {
        "M0,.5 L1,.5 M.5,.5 L.5,1"
    };
}
/// ┬
#[allow(dead_code)]
const T_BOTTOM: &str = t_bottom!();
macro_rules! cross {
    () => {
        "M0,.5 L1,.5 M.5,0 L.5,1"
    };
}
/// ┼
#[allow(dead_code)]
const CROSS: &str = cross!();
macro_rules! two_dashes_horizontal {
    () => {
        "M.1,.5 L.4,.5 M.6,.5 L.9,.5"
    };
}
/// ╌ // .2 empty, .3 filled
#[allow(dead_code)]
const TWO_DASHES_HORIZONTAL: &str = two_dashes_horizontal!();
macro_rules! three_dashes_horizontal {
    () => {
        "M.0667,.5 L.2667,.5 M.4,.5 L.6,.5 M.7333,.5 L.9333,.5"
    };
}
/// ┄ // .1333 empty, .2 filled
#[allow(dead_code)]
const THREE_DASHES_HORIZONTAL: &str = three_dashes_horizontal!();
macro_rules! four_dashes_horizontal {
    () => {
        "M.05,.5 L.2,.5 M.3,.5 L.45,.5 M.55,.5 L.7,.5 M.8,.5 L.95,.5"
    };
}
/// ┉ // .1 empty, .15 filled
#[allow(dead_code)]
const FOUR_DASHES_HORIZONTAL: &str = four_dashes_horizontal!();
macro_rules! two_dashes_vertical {
    () => {
        "M.5,.1 L.5,.4 M.5,.6 L.5,.9"
    };
}
/// ╎
#[allow(dead_code)]
const TWO_DASHES_VERTICAL: &str = two_dashes_vertical!();
macro_rules! three_dashes_vertical {
    () => {
        "M.5,.0667 L.5,.2667 M.5,.4 L.5,.6 M.5,.7333 L.5,.9333"
    };
}
/// ┆
#[allow(dead_code)]
const THREE_DASHES_VERTICAL: &str = three_dashes_vertical!();
macro_rules! four_dashes_vertical {
    () => {
        "M.5,.05 L.5,.2 M.5,.3 L.5,.45 L.5,.55 M.5,.7 L.5,.95"
    };
}
/// ┊
#[allow(dead_code)]
const FOUR_DASHES_VERTICAL: &str = four_dashes_vertical!();

/// `boxDrawingDefinitions`: SVG-path-like definitions per weight (1 normal, 3 bold).
#[allow(unused_variables)]
pub const BOX_DRAWING: &[(char, &[(u32, Def)])] = &[
    // Uniform normal and bold
    ('─', nrm!(left_to_right!())),
    ('━', bld!(left_to_right!())),
    ('│', nrm!(top_to_bottom!())),
    ('┃', bld!(top_to_bottom!())),
    ('┌', nrm!(right_to_bottom!())),
    ('┏', bld!(right_to_bottom!())),
    ('┐', nrm!(left_to_bottom!())),
    ('┓', bld!(left_to_bottom!())),
    ('└', nrm!(top_to_right!())),
    ('┗', bld!(top_to_right!())),
    ('┘', nrm!(top_to_left!())),
    ('┛', bld!(top_to_left!())),
    ('├', nrm!(t_right!())),
    ('┣', bld!(t_right!())),
    ('┤', nrm!(t_left!())),
    ('┫', bld!(t_left!())),
    ('┬', nrm!(t_bottom!())),
    ('┳', bld!(t_bottom!())),
    ('┴', nrm!(t_top!())),
    ('┻', bld!(t_top!())),
    ('┼', nrm!(cross!())),
    ('╋', bld!(cross!())),
    ('╴', nrm!(middle_to_left!())),
    ('╸', bld!(middle_to_left!())),
    ('╵', nrm!(middle_to_top!())),
    ('╹', bld!(middle_to_top!())),
    ('╶', nrm!(middle_to_right!())),
    ('╺', bld!(middle_to_right!())),
    ('╷', nrm!(middle_to_bottom!())),
    ('╻', bld!(middle_to_bottom!())),
    // Double border
    (
        '═',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,{ym} L1,{ym} M0,{yq} L1,{yq}")
        }),
    ),
    (
        '║',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M{xm},0 L{xm},1 M{xq},0 L{xq},1")
        }),
    ),
    (
        '╒',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M.5,1 L.5,{ym} L1,{ym} M.5,{yq} L1,{yq}")
        }),
    ),
    (
        '╓',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M{xm},1 L{xm},.5 L1,.5 M{xq},.5 L{xq},1")
        }),
    ),
    (
        '╔',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M1,{ym} L{xm},{ym} L{xm},1 M1,{yq} L{xq},{yq} L{xq},1")
        }),
    ),
    (
        '╕',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,{ym} L.5,{ym} L.5,1 M0,{yq} L.5,{yq}")
        }),
    ),
    (
        '╖',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M{xq},1 L{xq},.5 L0,.5 M{xm},.5 L{xm},1")
        }),
    ),
    (
        '╗',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,{yq} L{xm},{yq} L{xm},1 M0,{ym} L{xq},{ym} L{xq},1")
        }),
    ),
    (
        '╘',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M.5,0 L.5,{yq} L1,{yq} M.5,{ym} L1,{ym}")
        }),
    ),
    (
        '╙',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M1,.5 L{xm},.5 L{xm},0 M{xq},.5 L{xq},0")
        }),
    ),
    (
        '╚',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M1,{ym} L{xq},{ym} L{xq},0 M1,{yq} L{xm},{yq} L{xm},0")
        }),
    ),
    (
        '╛',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,{yq} L.5,{yq} L.5,0 M0,{ym} L.5,{ym}")
        }),
    ),
    (
        '╜',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,.5 L{xq},.5 L{xq},0 M{xm},.5 L{xm},0")
        }),
    ),
    (
        '╝',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,{ym} L{xm},{ym} L{xm},0 M0,{yq} L{xq},{yq} L{xq},0")
        }),
    ),
    (
        '╞',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("{TOP_TO_BOTTOM} M.5,{ym} L1,{ym} M.5,{yq} L1,{yq}")
        }),
    ),
    (
        '╟',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M{xm},0 L{xm},1 M{xq},0 L{xq},1 M{xq},.5 L1,.5")
        }),
    ),
    (
        '╠',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M{xm},0 L{xm},1 M1,{yq} L{xq},{yq} L{xq},1 M1,{ym} L{xq},{ym} L{xq},0")
        }),
    ),
    (
        '╡',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("{TOP_TO_BOTTOM} M0,{ym} L.5,{ym} M0,{yq} L.5,{yq}")
        }),
    ),
    (
        '╢',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,.5 L{xm},.5 M{xm},0 L{xm},1 M{xq},0 L{xq},1")
        }),
    ),
    (
        '╣',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M{xq},0 L{xq},1 M0,{yq} L{xm},{yq} L{xm},1 M0,{ym} L{xm},{ym} L{xm},0")
        }),
    ),
    (
        '╤',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,{ym} L1,{ym} M0,{yq} L1,{yq} M.5,{yq} L.5,1")
        }),
    ),
    (
        '╥',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("{LEFT_TO_RIGHT} M{xm},.5 L{xm},1 M{xq},.5 L{xq},1")
        }),
    ),
    (
        '╦',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,{ym} L1,{ym} M0,{yq} L{xm},{yq} L{xm},1 M1,{yq} L{xq},{yq} L{xq},1")
        }),
    ),
    (
        '╧',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M.5,0 L.5,{ym} M0,{ym} L1,{ym} M0,{yq} L1,{yq}")
        }),
    ),
    (
        '╨',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("{LEFT_TO_RIGHT} M{xm},.5 L{xm},0 M{xq},.5 L{xq},0")
        }),
    ),
    (
        '╩',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("M0,{yq} L1,{yq} M0,{ym} L{xm},{ym} L{xm},0 M1,{ym} L{xq},{ym} L{xq},0")
        }),
    ),
    (
        '╪',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("{TOP_TO_BOTTOM} M0,{ym} L1,{ym} M0,{yq} L1,{yq}")
        }),
    ),
    (
        '╫',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!("{LEFT_TO_RIGHT} M{xm},0 L{xm},1 M{xq},0 L{xq},1")
        }),
    ),
    (
        '╬',
        func!(|xp, yp| {
            let [xm, xq, ym, yq] = quad(xp, yp);
            format!(
                "M0,{yq} L{xm},{yq} L{xm},1 M1,{yq} L{xq},{yq} L{xq},1 M0,{ym} L{xm},{ym} L{xm},0 M1,{ym} L{xq},{ym} L{xq},0"
            )
        }),
    ),
    // Diagonal
    ('╱', nrm!("M1,0 L0,1")),
    ('╲', nrm!("M0,0 L1,1")),
    ('╳', nrm!("M1,0 L0,1 M0,0 L1,1")),
    // Mixed weight
    ('╼', mix!(middle_to_left!(), middle_to_right!())),
    ('╽', mix!(middle_to_top!(), middle_to_bottom!())),
    ('╾', mix!(middle_to_right!(), middle_to_left!())),
    ('╿', mix!(middle_to_bottom!(), middle_to_top!())),
    ('┍', mix!(middle_to_bottom!(), middle_to_right!())),
    ('┎', mix!(middle_to_right!(), middle_to_bottom!())),
    ('┑', mix!(middle_to_bottom!(), middle_to_left!())),
    ('┒', mix!(middle_to_left!(), middle_to_bottom!())),
    ('┕', mix!(middle_to_top!(), middle_to_right!())),
    ('┖', mix!(middle_to_right!(), middle_to_top!())),
    ('┙', mix!(middle_to_top!(), middle_to_left!())),
    ('┚', mix!(middle_to_left!(), middle_to_top!())),
    ('┝', mix!(top_to_bottom!(), middle_to_right!())),
    ('┞', mix!(right_to_bottom!(), middle_to_top!())),
    ('┟', mix!(top_to_right!(), middle_to_bottom!())),
    ('┠', mix!(middle_to_right!(), top_to_bottom!())),
    ('┡', mix!(middle_to_bottom!(), top_to_right!())),
    ('┢', mix!(middle_to_top!(), right_to_bottom!())),
    ('┥', mix!(top_to_bottom!(), middle_to_left!())),
    ('┦', mix!(left_to_bottom!(), middle_to_top!())),
    ('┧', mix!(top_to_left!(), middle_to_bottom!())),
    ('┨', mix!(middle_to_left!(), top_to_bottom!())),
    ('┩', mix!(middle_to_bottom!(), top_to_left!())),
    ('┪', mix!(middle_to_top!(), left_to_bottom!())),
    ('┭', mix!(right_to_bottom!(), middle_to_left!())),
    ('┮', mix!(left_to_bottom!(), middle_to_right!())),
    ('┯', mix!(middle_to_bottom!(), left_to_right!())),
    ('┰', mix!(left_to_right!(), middle_to_bottom!())),
    ('┱', mix!(middle_to_right!(), left_to_bottom!())),
    ('┲', mix!(middle_to_left!(), right_to_bottom!())),
    ('┵', mix!(top_to_right!(), middle_to_left!())),
    ('┶', mix!(top_to_left!(), middle_to_right!())),
    ('┷', mix!(middle_to_top!(), left_to_right!())),
    ('┸', mix!(left_to_right!(), middle_to_top!())),
    ('┹', mix!(middle_to_right!(), top_to_left!())),
    ('┺', mix!(middle_to_left!(), top_to_right!())),
    (
        '┽',
        mix!(
            concat!(top_to_bottom!(), " ", middle_to_right!()),
            middle_to_left!()
        ),
    ),
    (
        '┾',
        mix!(
            concat!(top_to_bottom!(), " ", middle_to_left!()),
            middle_to_right!()
        ),
    ),
    ('┿', mix!(top_to_bottom!(), left_to_right!())),
    (
        '╀',
        mix!(
            concat!(left_to_right!(), " ", middle_to_bottom!()),
            middle_to_top!()
        ),
    ),
    (
        '╁',
        mix!(
            concat!(middle_to_top!(), " ", left_to_right!()),
            middle_to_bottom!()
        ),
    ),
    ('╂', mix!(left_to_right!(), top_to_bottom!())),
    ('╃', mix!(right_to_bottom!(), top_to_left!())),
    ('╄', mix!(left_to_bottom!(), top_to_right!())),
    ('╅', mix!(top_to_right!(), left_to_bottom!())),
    ('╆', mix!(top_to_left!(), right_to_bottom!())),
    (
        '╇',
        mix!(
            middle_to_bottom!(),
            concat!(middle_to_top!(), " ", left_to_right!())
        ),
    ),
    (
        '╈',
        mix!(
            middle_to_top!(),
            concat!(left_to_right!(), " ", middle_to_bottom!())
        ),
    ),
    (
        '╉',
        mix!(
            middle_to_right!(),
            concat!(top_to_bottom!(), " ", middle_to_left!())
        ),
    ),
    (
        '╊',
        mix!(
            middle_to_left!(),
            concat!(top_to_bottom!(), " ", middle_to_right!())
        ),
    ),
    // Dashed
    ('╌', nrm!(two_dashes_horizontal!())),
    ('╍', bld!(two_dashes_horizontal!())),
    ('┄', nrm!(three_dashes_horizontal!())),
    ('┅', bld!(three_dashes_horizontal!())),
    ('┈', nrm!(four_dashes_horizontal!())),
    ('┉', bld!(four_dashes_horizontal!())),
    ('╎', nrm!(two_dashes_vertical!())),
    ('╏', bld!(two_dashes_vertical!())),
    ('┆', nrm!(three_dashes_vertical!())),
    ('┇', bld!(three_dashes_vertical!())),
    ('┊', nrm!(four_dashes_vertical!())),
    ('┋', bld!(four_dashes_vertical!())),
    // Curved
    ('╭', curve!(+, "M.5,1", 1)),
    ('╮', curve!(+, "M.5,1", 0)),
    ('╯', curve!(-, "M.5,0", 0)),
    ('╰', curve!(-, "M.5,0", 1)),
];

/// `IVectorShape` fill or stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorType {
    Fill,
    Stroke,
}

/// `IVectorShape`.
#[derive(Debug, Clone, Copy)]
pub struct VectorShape {
    pub d: &'static str,
    pub ty: VectorType,
    pub left_padding: f64,
    pub right_padding: f64,
}

/// `powerlineDefinitions`. The line variants draw beyond the cell and get clipped to ensure the end
/// of the line is not visible. Original symbols defined in https://github.com/powerline/fontpatcher
pub const POWERLINE: &[(char, VectorShape)] = &[
    // Git branch
    (
        '\u{E0A0}',
        VectorShape {
            d: "M.3,1 L.03,1 L.03,.88 C.03,.82,.06,.78,.11,.73 C.15,.7,.2,.68,.28,.65 L.43,.6 C.49,.58,.53,.56,.56,.53 C.59,.5,.6,.47,.6,.43 L.6,.27 L.4,.27 L.69,.1 L.98,.27 L.78,.27 L.78,.46 C.78,.52,.76,.56,.72,.61 C.68,.66,.63,.67,.56,.7 L.48,.72 C.42,.74,.38,.76,.35,.78 C.32,.8,.31,.84,.31,.88 L.31,1 M.3,.5 L.03,.59 L.03,.09 L.3,.09 L.3,.655",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 0.0,
        },
    ),
    // L N
    (
        '\u{E0A1}',
        VectorShape {
            d: "M.7,.4 L.7,.47 L.2,.47 L.2,.03 L.355,.03 L.355,.4 L.705,.4 M.7,.5 L.86,.5 L.86,.95 L.69,.95 L.44,.66 L.46,.86 L.46,.95 L.3,.95 L.3,.49 L.46,.49 L.71,.78 L.69,.565 L.69,.5",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 0.0,
        },
    ),
    // Lock
    (
        '\u{E0A2}',
        VectorShape {
            d: "M.25,.94 C.16,.94,.11,.92,.11,.87 L.11,.53 C.11,.48,.15,.455,.23,.45 L.23,.3 C.23,.25,.26,.22,.31,.19 C.36,.16,.43,.15,.51,.15 C.59,.15,.66,.16,.71,.19 C.77,.22,.79,.26,.79,.3 L.79,.45 C.87,.45,.91,.48,.91,.53 L.91,.87 C.91,.92,.86,.94,.77,.94 L.24,.94 M.53,.2 C.49,.2,.45,.21,.42,.23 C.39,.25,.38,.27,.38,.3 L.38,.45 L.68,.45 L.68,.3 C.68,.27,.67,.25,.64,.23 C.61,.21,.58,.2,.53,.2 M.58,.82 L.58,.66 C.63,.65,.65,.63,.65,.6 C.65,.58,.64,.57,.61,.56 C.58,.55,.56,.54,.52,.54 C.48,.54,.46,.55,.43,.56 C.4,.57,.39,.59,.39,.6 C.39,.63,.41,.64,.46,.66 L.46,.82 L.57,.82",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 0.0,
        },
    ),
    // Right triangle solid
    (
        '\u{E0B0}',
        VectorShape {
            d: "M0,0 L1,.5 L0,1",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 2.0,
        },
    ),
    // Right triangle line
    (
        '\u{E0B1}',
        VectorShape {
            d: "M-1,-.5 L1,.5 L-1,1.5",
            ty: VectorType::Stroke,
            left_padding: 1.0,
            right_padding: 1.0,
        },
    ),
    // Left triangle solid
    (
        '\u{E0B2}',
        VectorShape {
            d: "M1,0 L0,.5 L1,1",
            ty: VectorType::Fill,
            left_padding: 2.0,
            right_padding: 0.0,
        },
    ),
    // Left triangle line
    (
        '\u{E0B3}',
        VectorShape {
            d: "M2,-.5 L0,.5 L2,1.5",
            ty: VectorType::Stroke,
            left_padding: 1.0,
            right_padding: 1.0,
        },
    ),
    // Right semi-circle solid
    (
        '\u{E0B4}',
        VectorShape {
            d: "M0,0 L0,1 C0.552,1,1,0.776,1,.5 C1,0.224,0.552,0,0,0",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 1.0,
        },
    ),
    // Right semi-circle line
    (
        '\u{E0B5}',
        VectorShape {
            d: "M.2,1 C.422,1,.8,.826,.78,.5 C.8,.174,0.422,0,.2,0",
            ty: VectorType::Stroke,
            left_padding: 0.0,
            right_padding: 1.0,
        },
    ),
    // Left semi-circle solid
    (
        '\u{E0B6}',
        VectorShape {
            d: "M1,0 L1,1 C0.448,1,0,0.776,0,.5 C0,0.224,0.448,0,1,0",
            ty: VectorType::Fill,
            left_padding: 1.0,
            right_padding: 0.0,
        },
    ),
    // Left semi-circle line
    (
        '\u{E0B7}',
        VectorShape {
            d: "M.8,1 C0.578,1,0.2,.826,.22,.5 C0.2,0.174,0.578,0,0.8,0",
            ty: VectorType::Stroke,
            left_padding: 1.0,
            right_padding: 0.0,
        },
    ),
    // Lower left triangle
    (
        '\u{E0B8}',
        VectorShape {
            d: "M-.5,-.5 L1.5,1.5 L-.5,1.5",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 0.0,
        },
    ),
    // Backslash separator
    (
        '\u{E0B9}',
        VectorShape {
            d: "M-.5,-.5 L1.5,1.5",
            ty: VectorType::Stroke,
            left_padding: 1.0,
            right_padding: 1.0,
        },
    ),
    // Lower right triangle
    (
        '\u{E0BA}',
        VectorShape {
            d: "M1.5,-.5 L-.5,1.5 L1.5,1.5",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 0.0,
        },
    ),
    // Upper left triangle
    (
        '\u{E0BC}',
        VectorShape {
            d: "M1.5,-.5 L-.5,1.5 L-.5,-.5",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 0.0,
        },
    ),
    // Forward slash separator
    (
        '\u{E0BD}',
        VectorShape {
            d: "M1.5,-.5 L-.5,1.5",
            ty: VectorType::Stroke,
            left_padding: 1.0,
            right_padding: 1.0,
        },
    ),
    // Upper right triangle
    (
        '\u{E0BE}',
        VectorShape {
            d: "M-.5,-.5 L1.5,1.5 L1.5,-.5",
            ty: VectorType::Fill,
            left_padding: 0.0,
            right_padding: 0.0,
        },
    ),
];
/// Forward slash separator redundant / backslash separator redundant: upstream copies the
/// definitions of E0BD and E0B9 to these two code points.
pub const POWERLINE_ALIASES: &[(char, char)] =
    &[('\u{E0BB}', '\u{E0BD}'), ('\u{E0BF}', '\u{E0B9}')];

fn lookup<T>(table: &'static [(char, T)], c: char) -> Option<&'static T> {
    table.iter().find(|(k, _)| *k == c).map(|(_, v)| v)
}

fn powerline_definition(c: char) -> Option<&'static VectorShape> {
    let c = POWERLINE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == c)
        .map_or(c, |(_, target)| *target);
    lookup(POWERLINE, c)
}

// LUNETERM: has_custom_char (deviation 3). The builder asks this per cell, so the code-point
// ranges that hold every definition reject the common case before any table is scanned.
/// Whether [`try_draw_custom_char`] has a definition for `c`.
pub fn has_custom_char(c: char) -> bool {
    let u = c as u32;
    let in_range = (0x2500..=0x259F).contains(&u)
        || (0x1FB70..=0x1FB97).contains(&u)
        || (0xE0A0..=0xE0BF).contains(&u);
    in_range
        && (lookup(BLOCK_ELEMENTS, c).is_some()
            || lookup(PATTERN_CHARACTERS, c).is_some()
            || lookup(BOX_DRAWING, c).is_some()
            || powerline_definition(c).is_some())
}

/// `Math.round`: halves round towards +infinity (`f64::round` rounds them away from zero).
pub fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

// LUNETERM: D17 (deviation 1). Upstream snaps a clamped coordinate to `Math.round(v + 0.5) - 0.5`,
// which centres a line on a pixel and is crisp only for odd widths (at 2x the line straddles
// three rows: 127/255/127). `snap(v, lw)` centres it so both edges of an `lw`-wide line fall on
// pixel boundaries; for lw 1 and 3 (every 1x case) it equals upstream exactly.
/// Snap the centre of a line of width `lw` so its edges are pixel boundaries.
pub fn snap(v: f64, lw: f64) -> f64 {
    js_round(v - lw / 2.0) + lw / 2.0
}

/// Try drawing a custom block element or box drawing character, returning whether it was
/// successfully drawn. `c` must be exactly one character.
#[allow(clippy::too_many_arguments)]
pub fn try_draw_custom_char(
    ctx: &mut GlyphCanvas,
    c: &str,
    x_offset: f64,
    y_offset: f64,
    device_cell_width: f64,
    device_cell_height: f64,
    font_size: f64,
    device_pixel_ratio: f64,
) -> bool {
    let mut chars = c.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return false;
    };

    if let Some(def) = lookup(BLOCK_ELEMENTS, ch) {
        draw_block_element_char(
            ctx,
            def,
            x_offset,
            y_offset,
            device_cell_width,
            device_cell_height,
        );
        return true;
    }

    if let Some(def) = lookup(PATTERN_CHARACTERS, ch) {
        draw_pattern_char(
            ctx,
            def,
            x_offset,
            y_offset,
            device_cell_width,
            device_cell_height,
        );
        return true;
    }

    if let Some(def) = lookup(BOX_DRAWING, ch) {
        draw_box_drawing_char(
            ctx,
            def,
            x_offset,
            y_offset,
            device_cell_width,
            device_cell_height,
            device_pixel_ratio,
        );
        return true;
    }

    if let Some(def) = powerline_definition(ch) {
        draw_powerline_char(
            ctx,
            def,
            x_offset,
            y_offset,
            device_cell_width,
            device_cell_height,
            font_size,
            device_pixel_ratio,
        );
        return true;
    }

    false
}

fn draw_block_element_char(
    ctx: &mut GlyphCanvas,
    char_definition: &[BlockVector],
    x_offset: f64,
    y_offset: f64,
    device_cell_width: f64,
    device_cell_height: f64,
) {
    for &(bx, by, bw, bh) in char_definition {
        let x_eighth = device_cell_width / 8.0;
        let y_eighth = device_cell_height / 8.0;
        ctx.fill_rect(
            x_offset + f64::from(bx) * x_eighth,
            y_offset + f64::from(by) * y_eighth,
            f64::from(bw) * x_eighth,
            f64::from(bh) * y_eighth,
        );
    }
}

// LUNETERM: deviation 2. The glyph is a colourless coverage mask, so there is no fillStyle to
// parse and no `CanvasPattern` cache: coverage = pattern x 255, and the atlas caches the image.
fn draw_pattern_char(
    ctx: &mut GlyphCanvas,
    char_definition: &[&[u8]],
    x_offset: f64,
    y_offset: f64,
    device_cell_width: f64,
    device_cell_height: f64,
) {
    ctx.fill_pattern(
        char_definition,
        x_offset,
        y_offset,
        device_cell_width,
        device_cell_height,
    );
}

/// Draws the following box drawing characters by mapping a subset of SVG d attribute
/// instructions to canvas draw calls.
///
/// Box styles:       ┎┰┒┍┯┑╓╥╖╒╤╕ ┏┳┓┌┲┓┌┬┐┏┱┐
/// ┌─┬─┐ ┏━┳━┓ ╔═╦═╗ ┠╂┨┝┿┥╟╫╢╞╪╡ ┡╇┩├╊┫┢╈┪┣╉┤
/// │ │ │ ┃ ┃ ┃ ║ ║ ║ ┖┸┚┕┷┙╙╨╜╘╧╛ └┴┘└┺┛┗┻┛┗┹┘
/// ├─┼─┤ ┣━╋━┫ ╠═╬═╣ ┏┱┐┌┲┓┌┬┐┌┬┐ ┏┳┓┌┮┓┌┬┐┏┭┐
/// │ │ │ ┃ ┃ ┃ ║ ║ ║ ┡╃┤├╄┩├╆┪┢╅┤ ┞╀┦├┾┫┟╁┧┣┽┤
/// └─┴─┘ ┗━┻━┛ ╚═╩═╝ └┴┘└┴┘└┺┛┗┹┘ └┴┘└┶┛┗┻┛┗┵┘
///
/// Other:
/// ╭─╮ ╲ ╱ ╷╻╎╏┆┇┊┋ ╺╾╴ ╌╌╌ ┄┄┄ ┈┈┈
/// │ │  ╳  ╽╿╎╏┆┇┊┋ ╶╼╸ ╍╍╍ ┅┅┅ ┉┉┉
/// ╰─╯ ╱ ╲ ╹╵╎╏┆┇┊┋
fn draw_box_drawing_char(
    ctx: &mut GlyphCanvas,
    char_definition: &[(u32, Def)],
    x_offset: f64,
    y_offset: f64,
    device_cell_width: f64,
    device_cell_height: f64,
    device_pixel_ratio: f64,
) {
    // `ctx.strokeStyle = ctx.fillStyle`: a canvas has no colour here
    // `Object.entries` lists integer keys ascending; the tables list weights ascending
    for &(font_weight, instructions) in char_definition {
        ctx.begin_path();
        ctx.line_width = device_pixel_ratio * f64::from(font_weight);
        let actual_instructions = match instructions {
            Def::Func(f) => {
                let xp = 0.15;
                let yp = 0.15 / device_cell_height * device_cell_width;
                f(xp, yp)
            }
            Def::Static(s) => s.to_owned(),
        };
        for instruction in actual_instructions.split(' ') {
            apply_instruction(
                ctx,
                instruction,
                device_cell_width,
                device_cell_height,
                x_offset,
                y_offset,
                true,
                device_pixel_ratio,
                0.0,
                0.0,
            );
        }
        ctx.stroke();
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_powerline_char(
    ctx: &mut GlyphCanvas,
    char_definition: &VectorShape,
    x_offset: f64,
    y_offset: f64,
    device_cell_width: f64,
    device_cell_height: f64,
    font_size: f64,
    device_pixel_ratio: f64,
) {
    // Clip the cell to make sure drawing doesn't occur beyond bounds
    ctx.clip(x_offset, y_offset, device_cell_width, device_cell_height);

    ctx.begin_path();
    // Scale the stroke with DPR and font size
    let css_line_width = font_size / 12.0;
    ctx.line_width = device_pixel_ratio * css_line_width;
    for instruction in char_definition.d.split(' ') {
        apply_instruction(
            ctx,
            instruction,
            device_cell_width,
            device_cell_height,
            x_offset,
            y_offset,
            false,
            device_pixel_ratio,
            char_definition.left_padding * (css_line_width / 2.0),
            char_definition.right_padding * (css_line_width / 2.0),
        );
    }
    if char_definition.ty == VectorType::Stroke {
        ctx.stroke();
    } else {
        ctx.fill();
    }
}

/// One SVG instruction (`M`, `L` or `C` plus comma-separated numbers) as canvas calls: the body
/// shared by `drawBoxDrawingChar` and `drawPowerlineChar`.
#[allow(clippy::too_many_arguments)]
fn apply_instruction(
    ctx: &mut GlyphCanvas,
    instruction: &str,
    device_cell_width: f64,
    device_cell_height: f64,
    x_offset: f64,
    y_offset: f64,
    do_clamp: bool,
    device_pixel_ratio: f64,
    left_padding: f64,
    right_padding: f64,
) {
    let mut it = instruction.chars();
    // an empty instruction (upstream: `svgToCanvasInstructionMap[undefined]`) is skipped
    let Some(kind) = it.next() else { return };
    // svgToCanvasInstructionMap: only C, L and M exist (upstream logs the others and continues)
    if !matches!(kind, 'C' | 'L' | 'M') {
        return;
    }
    let args: Vec<&str> = it.as_str().split(',').collect();
    if args.len() < 2 || args[0].is_empty() || args[1].is_empty() {
        return;
    }
    let Some(a) = translate_args(
        &args,
        device_cell_width,
        device_cell_height,
        x_offset,
        y_offset,
        do_clamp,
        device_pixel_ratio,
        left_padding,
        right_padding,
        ctx.line_width,
    ) else {
        return;
    };
    // LUNETERM: deviation 4. A missing or NaN coordinate skips the instruction (upstream hands
    // NaN to the canvas, which ignores the call).
    let need = if kind == 'C' { 6 } else { 2 };
    let Some(a) = a.get(..need).filter(|a| a.iter().all(|v| v.is_finite())) else {
        return;
    };
    match kind {
        'M' => ctx.move_to(a[0], a[1]),
        'L' => ctx.line_to(a[0], a[1]),
        _ => ctx.bezier_curve_to(a[0], a[1], a[2], a[3], a[4], a[5]),
    }
}

fn clamp(value: f64, max: f64, min: f64) -> f64 {
    value.min(max).max(min)
}

/// JS `parseFloat(e) || parseInt(e)`; NaN when neither parses.
fn parse_float_or_int(e: &str) -> f64 {
    let f = js_parse_float(e);
    if f != 0.0 && !f.is_nan() {
        return f;
    }
    js_parse_int(e)
}

/// The longest numeric prefix of `s` that matches `[+-]digits[.digits][e[+-]digits]`.
fn js_parse_float(s: &str) -> f64 {
    let b = s.trim_start().as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - start
    };
    let int = digits(&mut i);
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        let mut j = i + 1;
        frac = digits(&mut j);
        if frac > 0 || int > 0 {
            i = j;
        }
    }
    if int == 0 && frac == 0 {
        return f64::NAN;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if digits(&mut j) > 0 {
            i = j;
        }
    }
    std::str::from_utf8(&b[..i])
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}

/// JS `parseInt(s)` (radix 10): the leading `[+-]digits`.
fn js_parse_int(s: &str) -> f64 {
    let b = s.trim_start().as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let start = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == start {
        return f64::NAN;
    }
    std::str::from_utf8(&b[..i])
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}

/// `translateArgs`. `line_width` is only used by the D17 snap. `None` where upstream throws
/// ("Too few arguments for instruction").
#[allow(clippy::too_many_arguments)]
pub fn translate_args(
    args: &[&str],
    cell_width: f64,
    cell_height: f64,
    x_offset: f64,
    y_offset: f64,
    do_clamp: bool,
    device_pixel_ratio: f64,
    left_padding: f64,
    right_padding: f64,
    line_width: f64,
) -> Option<Vec<f64>> {
    let mut result: Vec<f64> = args.iter().map(|e| parse_float_or_int(e)).collect();

    if result.len() < 2 {
        return None;
    }

    for x in (0..result.len()).step_by(2) {
        // Translate from 0-1 to 0-cellWidth
        result[x] *=
            cell_width - (left_padding * device_pixel_ratio) - (right_padding * device_pixel_ratio);
        // Ensure coordinate doesn't escape cell bounds and round to the nearest 0.5 to ensure a crisp
        // line at 100% devicePixelRatio
        if do_clamp && result[x] != 0.0 {
            result[x] = clamp(snap(result[x], line_width), cell_width, 0.0);
        }
        // Apply the cell's offset (ie. x*cellWidth)
        result[x] += x_offset + (left_padding * device_pixel_ratio);
    }

    for y in (1..result.len()).step_by(2) {
        // Translate from 0-1 to 0-cellHeight
        result[y] *= cell_height;
        // Ensure coordinate doesn't escape cell bounds and round to the nearest 0.5 to ensure a crisp
        // line at 100% devicePixelRatio
        if do_clamp && result[y] != 0.0 {
            result[y] = clamp(snap(result[y], line_width), cell_height, 0.0);
        }
        // Apply the cell's offset (ie. x*cellHeight)
        result[y] += y_offset;
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws `c` on a `w` x `h` canvas the way `SwashRaster` does (offset 0).
    fn draw(c: char, w: u32, h: u32, scale: f64, size: f64) -> GlyphCanvas {
        let mut ctx = GlyphCanvas::new(w, h);
        let s = c.to_string();
        assert!(try_draw_custom_char(
            &mut ctx,
            &s,
            0.0,
            0.0,
            f64::from(w),
            f64::from(h),
            size,
            scale
        ));
        ctx
    }

    fn at2(c: char) -> GlyphCanvas {
        draw(c, 16, 34, 2.0, 13.0)
    }

    fn at1(c: char) -> GlyphCanvas {
        draw(c, 8, 17, 1.0, 13.0)
    }

    fn px(c: &GlyphCanvas, x: u32, y: u32) -> u8 {
        c.pixels()[(y * c.width() + x) as usize]
    }

    fn row(c: &GlyphCanvas, y: u32) -> Vec<u8> {
        (0..c.width()).map(|x| px(c, x, y)).collect()
    }

    fn col(c: &GlyphCanvas, x: u32) -> Vec<u8> {
        (0..c.height()).map(|y| px(c, x, y)).collect()
    }

    fn count(c: &GlyphCanvas, f: impl Fn(u8) -> bool) -> usize {
        c.pixels().iter().filter(|&&v| f(v)).count()
    }

    #[test]
    fn has_custom_char_covers_exactly_the_upstream_tables() {
        for u in 0x2500..=0x259Fu32 {
            assert!(has_custom_char(char::from_u32(u).expect("char")), "{u:X}");
        }
        for u in [0xE0A0, 0xE0A1, 0xE0A2] {
            assert!(has_custom_char(char::from_u32(u).expect("char")), "{u:X}");
        }
        for u in 0xE0B0..=0xE0BFu32 {
            assert!(has_custom_char(char::from_u32(u).expect("char")), "{u:X}");
        }
        let legacy = (0x1FB70..=0x1FB97u32)
            .filter(|&u| has_custom_char(char::from_u32(u).expect("char")))
            .count();
        assert_eq!(legacy, 31);
        for c in [
            'a',
            '\u{E0A3}',
            '\u{F07B}',
            '\u{24FF}',
            '\u{25A0}',
            '\u{1FB8C}',
        ] {
            assert!(!has_custom_char(c), "{c:?}");
        }
        assert_eq!(BOX_DRAWING.len(), 128);
        assert_eq!(BLOCK_ELEMENTS.len(), 60);
        assert_eq!(PATTERN_CHARACTERS.len(), 3);
        assert_eq!(POWERLINE.len() + POWERLINE_ALIASES.len(), 19);
        // every table key is distinct
        let mut all: Vec<char> = BLOCK_ELEMENTS.iter().map(|e| e.0).collect();
        all.extend(PATTERN_CHARACTERS.iter().map(|e| e.0));
        all.extend(BOX_DRAWING.iter().map(|e| e.0));
        all.extend(POWERLINE.iter().map(|e| e.0));
        all.extend(POWERLINE_ALIASES.iter().map(|e| e.0));
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n);
    }

    #[test]
    fn light_horizontal_is_two_rows_at_2x_and_one_at_1x() {
        let c = at2('─');
        for y in 0..34 {
            let want = if y == 16 || y == 17 { 255 } else { 0 };
            assert_eq!(row(&c, y), vec![want; 16], "row {y}");
        }
        let c = at1('─');
        for y in 0..17 {
            let want = if y == 8 { 255 } else { 0 };
            assert_eq!(row(&c, y), vec![want; 8], "row {y}");
        }
    }

    #[test]
    fn heavy_horizontal_is_six_rows_at_2x_and_three_at_1x() {
        let c = at2('━');
        for y in 0..34 {
            let want = if (14..=19).contains(&y) { 255 } else { 0 };
            assert_eq!(row(&c, y), vec![want; 16], "row {y}");
        }
        let c = at1('━');
        for y in 0..17 {
            let want = if (7..=9).contains(&y) { 255 } else { 0 };
            assert_eq!(row(&c, y), vec![want; 8], "row {y}");
        }
    }

    #[test]
    fn light_vertical_runs_the_full_height() {
        let c = at2('│');
        for x in 0..16 {
            let want = if x == 7 || x == 8 { 255 } else { 0 };
            assert_eq!(col(&c, x), vec![want; 34], "col {x}");
        }
    }

    #[test]
    fn blocks_and_shades() {
        assert!(at2('█').pixels().iter().all(|&v| v == 255));

        let c = at2('▀');
        for y in 0..34 {
            let want = if y <= 16 { 255 } else { 0 };
            assert_eq!(row(&c, y), vec![want; 16], "▀ row {y}");
        }

        let c = at2('▁');
        for y in 0..34 {
            let r = row(&c, y);
            match y {
                0..=28 => assert_eq!(r, vec![0; 16], "▁ row {y}"),
                29 => assert!(r.iter().all(|&v| (i32::from(v) - 64).abs() <= 2), "{r:?}"),
                _ => assert_eq!(r, vec![255; 16], "▁ row {y}"),
            }
        }

        let c = at2('▌');
        for x in 0..16 {
            let want = if x < 8 { 255 } else { 0 };
            assert_eq!(col(&c, x), vec![want; 34], "▌ col {x}");
        }

        let c = at2('░');
        assert_eq!(count(&c, |v| v == 255), 68);
        assert_eq!(count(&c, |v| v == 0), 16 * 34 - 68);
        assert_eq!(px(&c, 0, 0), 255);
    }

    /// Which edges a corner/tee/curve reaches: left, right, top, bottom.
    const JOINS: &[(char, [bool; 4], bool)] = &[
        ('┌', [false, true, false, true], false),
        ('┐', [true, false, false, true], false),
        ('└', [false, true, true, false], false),
        ('┘', [true, false, true, false], false),
        ('├', [false, true, true, true], false),
        ('┤', [true, false, true, true], false),
        ('┬', [true, true, false, true], false),
        ('┴', [true, true, true, false], false),
        ('┼', [true, true, true, true], false),
        ('╭', [false, true, false, true], true),
        ('╮', [true, false, false, true], true),
        ('╯', [true, false, true, false], true),
        ('╰', [false, true, true, false], true),
    ];

    #[test]
    fn corners_and_tees_join_their_neighbours() {
        for scale in [2.0, 1.0] {
            let (w, h) = if scale == 2.0 { (16, 34) } else { (8, 17) };
            let (hl, vl) = (draw('─', w, h, scale, 13.0), draw('│', w, h, scale, 13.0));
            for &(ch, [l, r, t, b], curved) in JOINS {
                let g = draw(ch, w, h, scale, 13.0);
                let tol = if curved { 24 } else { 0 };
                let check = |edge: Vec<u8>, reference: Vec<u8>, connects: bool, what: &str| {
                    for (i, (&e, &want)) in edge.iter().zip(&reference).enumerate() {
                        let want = if connects { want } else { 0 };
                        assert!(
                            (i32::from(e) - i32::from(want)).abs() <= tol,
                            "{ch} at {scale}x, {what} edge px {i}: {e} vs {want}\n{edge:?}"
                        );
                    }
                };
                check(col(&g, 0), col(&hl, 0), l, "left");
                check(col(&g, w - 1), col(&hl, w - 1), r, "right");
                check(row(&g, 0), row(&vl, 0), t, "top");
                check(row(&g, h - 1), row(&vl, h - 1), b, "bottom");
            }
        }
    }

    #[test]
    fn mixed_weight_corner_has_a_bold_right_arm_and_a_normal_down_arm() {
        let c = at2('┍');
        for y in 14..=19 {
            assert_eq!(px(&c, 15, y), 255, "row {y}");
        }
        assert_eq!(px(&c, 15, 13), 0);
        assert_eq!(px(&c, 15, 20), 0);
        assert_eq!(px(&c, 7, 33), 255);
        assert_eq!(px(&c, 8, 33), 255);
        assert_eq!(px(&c, 6, 33), 0);
        assert_eq!(px(&c, 9, 33), 0);
    }

    #[test]
    fn powerline_triangles_fill_the_row_and_keep_their_padding() {
        let c = at2('\u{E0B0}');
        assert!(px(&c, 0, 0) > 0 && px(&c, 0, 33) > 0);
        for y in 2..=31 {
            assert_eq!(px(&c, 0, y), 255, "row {y}");
        }
        for y in 0..34 {
            assert_eq!(px(&c, 14, y), 0, "col 14 row {y}");
            assert_eq!(px(&c, 15, y), 0, "col 15 row {y}");
        }
        let c = at2('\u{E0B2}');
        assert!(px(&c, 15, 0) > 0 && px(&c, 15, 33) > 0);
        for y in 0..34 {
            assert_eq!(px(&c, 0, y), 0);
            assert_eq!(px(&c, 1, y), 0);
        }
    }

    #[test]
    fn every_definition_rasterizes_to_exactly_the_cell_without_panicking() {
        let mut chars: Vec<char> = Vec::new();
        chars.extend(BLOCK_ELEMENTS.iter().map(|e| e.0));
        chars.extend(PATTERN_CHARACTERS.iter().map(|e| e.0));
        chars.extend(BOX_DRAWING.iter().map(|e| e.0));
        chars.extend(POWERLINE.iter().map(|e| e.0));
        chars.extend(POWERLINE_ALIASES.iter().map(|e| e.0));
        for (w, h, scale) in [(8, 17, 1.0), (16, 34, 2.0), (12, 25, 1.5)] {
            for &ch in &chars {
                let c = draw(ch, w, h, scale, 13.0);
                assert_eq!(c.pixels().len(), (w * h) as usize, "{ch:?}");
                assert!(c.pixels().iter().any(|&v| v != 0), "{ch:?} drew nothing");
            }
        }
        for &ch in &chars {
            let mut c = GlyphCanvas::new(0, 0);
            let s = ch.to_string();
            assert!(try_draw_custom_char(
                &mut c, &s, 0.0, 0.0, 0.0, 0.0, 13.0, 2.0
            ));
            assert!(c.into_image().is_none());
        }
        // not exactly one character, or no definition
        let mut c = GlyphCanvas::new(8, 17);
        assert!(!try_draw_custom_char(
            &mut c,
            "─\u{301}",
            0.0,
            0.0,
            8.0,
            17.0,
            13.0,
            1.0
        ));
        assert!(!try_draw_custom_char(
            &mut c, "", 0.0, 0.0, 8.0, 17.0, 13.0, 1.0
        ));
        assert!(!try_draw_custom_char(
            &mut c, "a", 0.0, 0.0, 8.0, 17.0, 13.0, 1.0
        ));
        assert!(c.into_image().is_none());
    }

    #[test]
    fn helpers_mirror_javascript() {
        assert_eq!(js_round(-0.5), 0.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-1.5), -1.0);
        assert_eq!(snap(8.0, 1.0), 8.5);
        assert_eq!(snap(17.0, 2.0), 17.0);
        assert_eq!(snap(17.0, 3.0), 17.5);
        assert_eq!(js_parse_float(".5"), 0.5);
        assert_eq!(js_parse_float("-.5"), -0.5);
        assert_eq!(js_parse_float("1.5abc"), 1.5);
        assert_eq!(js_parse_float("1e-3"), 0.001);
        assert!(js_parse_float("x").is_nan());
        assert!(js_parse_float(".").is_nan());
        assert_eq!(parse_float_or_int("0"), 0.0);
        assert_eq!(parse_float_or_int("0.0667"), 0.0667);
        // parseFloat(".0") is 0 (falsy) and parseInt(".0") is NaN: upstream yields NaN
        assert!(parse_float_or_int(".0").is_nan());
    }

    #[test]
    fn bad_instructions_are_skipped_not_panicked_on() {
        let mut c = GlyphCanvas::new(8, 17);
        for ins in ["", "X1,2", "M", "M1", "M,1", "Mx,y", "C1,2,3", "L.5,"] {
            apply_instruction(&mut c, ins, 8.0, 17.0, 0.0, 0.0, true, 1.0, 0.0, 0.0);
        }
        c.stroke();
        assert!(c.into_image().is_none());
    }

    #[test]
    fn dashes_and_double_lines_leave_gaps_and_stay_apart() {
        // ╌ has a gap in the middle of the cell (.4 to .6)
        let c = at2('╌');
        assert_eq!(px(&c, 8, 16), 0);
        assert_eq!(px(&c, 3, 16), 255);
        // ═ is two lines, one on each side of the centre
        let c = at2('═');
        let rows: Vec<u32> = (0..34).filter(|&y| px(&c, 4, y) == 255).collect();
        assert!(
            rows.len() >= 2 && rows.first() < Some(&16) && rows.last() > Some(&17),
            "{rows:?}"
        );
        assert_eq!(px(&c, 4, 17), 0);
    }
}
