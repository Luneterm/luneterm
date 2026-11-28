//! Tab groups: what a group is, its colours and the colour a new one gets. Pure data: no UI, no
//! I/O. Which tabs are in a group, and every move that has to keep a group's tabs together, is
//! [`crate::tabs::TabSet`]'s.

use crate::ctl::{MAX_GROUP_CHARS, label_ok};
use crate::settings::Theme;

/// A group's identity: stable across renames and moves, never reused, even for a new group with
/// an old name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(pub(crate) u64);

impl GroupId {
    /// Names no group: no group ever gets this id.
    pub const NONE: GroupId = GroupId(0);
}

/// One open group.
#[derive(Clone, PartialEq, Eq)]
pub struct Group {
    /// Unique among the open groups, compared exactly.
    pub name: String,
    pub color: GroupColor,
    /// The side pane shows the group's header only. Nothing else depends on it.
    pub collapsed: bool,
}

impl std::fmt::Debug for Group {
    /// The name's length only: a program can choose it, and program text is kept out of a
    /// `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Group")
            .field("name_chars", &self.name.chars().count())
            .field("color", &self.color)
            .field("collapsed", &self.collapsed)
            .finish()
    }
}

/// The colour of a group, saved by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupColor {
    Grey,
    Blue,
    Red,
    Yellow,
    Green,
    Pink,
    Purple,
    Cyan,
    Orange,
}

impl GroupColor {
    /// Every colour, in the order they are offered and given out.
    pub const ALL: [GroupColor; 9] = [
        GroupColor::Grey,
        GroupColor::Blue,
        GroupColor::Red,
        GroupColor::Yellow,
        GroupColor::Green,
        GroupColor::Pink,
        GroupColor::Purple,
        GroupColor::Cyan,
        GroupColor::Orange,
    ];

    /// The name the colour is saved under.
    pub fn as_str(self) -> &'static str {
        match self {
            GroupColor::Grey => "grey",
            GroupColor::Blue => "blue",
            GroupColor::Red => "red",
            GroupColor::Yellow => "yellow",
            GroupColor::Green => "green",
            GroupColor::Pink => "pink",
            GroupColor::Purple => "purple",
            GroupColor::Cyan => "cyan",
            GroupColor::Orange => "orange",
        }
    }

    /// The colour saved under `name`; `None` for any other text.
    pub fn from_name(name: &str) -> Option<GroupColor> {
        GroupColor::ALL.into_iter().find(|c| c.as_str() == name)
    }

    /// The values the colour has under `look`.
    pub fn tone(self, look: Theme) -> GroupTone {
        // The mock editor's nine, under both looks. The text is the fill, except where that is
        // under 4.5:1 on its tinted card: then the same hue, only as much lighter as needed
        // (a test computes every pair).
        let fill = match self {
            GroupColor::Grey => [0xBD, 0xC1, 0xC6],
            GroupColor::Blue => [0x8A, 0xB4, 0xF8],
            GroupColor::Red => [0xF2, 0x8B, 0x82],
            GroupColor::Yellow => [0xFD, 0xD6, 0x63],
            GroupColor::Green => [0x81, 0xC9, 0x95],
            GroupColor::Pink => [0xFF, 0x8B, 0xCB],
            GroupColor::Purple => [0xC5, 0x8A, 0xF9],
            GroupColor::Cyan => [0x78, 0xD9, 0xEC],
            GroupColor::Orange => [0xFC, 0xAD, 0x70],
        };
        let text = match (look, self) {
            // Lunegit's panel is greyer than Luneterm's: fill 4.13:1 and 3.94:1 there.
            (Theme::Lunegit, GroupColor::Red) => [0xF3, 0x97, 0x8F],
            (Theme::Lunegit, GroupColor::Purple) => [0xCD, 0x9A, 0xFA],
            _ => fill,
        };
        GroupTone { fill, text }
    }
}

/// The colour values of one [`GroupColor`] under one look, RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupTone {
    /// The colour itself: a swatch, and the tint and ring of a group's card.
    pub fill: [u8; 3],
    /// Text on a card tinted with `fill`: the group's name in the side pane.
    pub text: [u8; 3],
}

/// The share of `fill` in a group card's tint (mock F).
pub const CARD_TINT: f32 = 0.08;
/// The share of `fill` in the 1 px ring of a group's card (mock F).
pub const CARD_RING: f32 = 0.15;

/// Whether `name` may be typed for a group: the rule `ctl` has ([`label_ok`], at most
/// [`MAX_GROUP_CHARS`]) and no other open group is called so (`taken`). Never trimmed.
pub fn name_problem(name: &str, taken: &[String]) -> Option<NameProblem> {
    if name.is_empty() {
        Some(NameProblem::Empty)
    } else if label_ok(name, MAX_GROUP_CHARS) {
        taken
            .iter()
            .any(|t| t == name)
            .then_some(NameProblem::Taken)
    } else if name.chars().nth(MAX_GROUP_CHARS).is_some() {
        Some(NameProblem::TooLong)
    } else if name.trim() != name {
        Some(NameProblem::EdgeSpace)
    } else {
        Some(NameProblem::Control)
    }
}

/// Why a name typed for a group cannot be one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    TooLong,
    /// Whitespace at either end.
    EdgeSpace,
    /// A control or bidi control character, or a line or paragraph separator.
    Control,
    /// Another open group has the name.
    Taken,
}

/// The first of `name(1)`, `name(2)`, … that is not `taken`: the prefill of a new group.
pub fn first_free_name(name: impl Fn(usize) -> String, taken: &[String]) -> String {
    // One more candidate than there are taken names, so one of them is free unless `name`
    // ignores its number.
    (1..=taken.len() + 1)
        .map(&name)
        .find(|n| !taken.iter().any(|t| t == n))
        .unwrap_or_else(|| name(1))
}

/// The colour a new group gets while the open groups use `in_use` (one entry per group): the
/// first in [`GroupColor::ALL`] that none uses, else the first of the least used.
pub fn default_color(in_use: impl IntoIterator<Item = GroupColor>) -> GroupColor {
    let mut uses = [0usize; GroupColor::ALL.len()];
    for color in in_use {
        uses[color as usize] += 1;
    }
    // Of several equal minima `min_by_key` returns the first.
    GroupColor::ALL
        .into_iter()
        .min_by_key(|c| uses[*c as usize])
        .unwrap_or(GroupColor::Grey)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::tabs::{TabId, TabSet};

    type Set = TabSet<()>;

    /// A set from a picture: one char per tab, `.` for a tab in no group, a letter for the group
    /// of that name. The first tab is active.
    fn set(picture: &str) -> (Set, Vec<TabId>) {
        let mut s = Set::new();
        let ids: Vec<TabId> = picture.chars().map(|_| s.push(())).collect();
        for (id, c) in ids.iter().zip(picture.chars()) {
            if c != '.' {
                s.set_group(*id, &c.to_string());
            }
        }
        if let Some(first) = ids.first() {
            s.activate(*first);
        }
        s.assert_invariants();
        assert_eq!(shape(&s), picture, "the picture is not a possible list");
        (s, ids)
    }

    /// The picture of a set.
    fn shape(s: &Set) -> String {
        s.ids()
            .into_iter()
            .map(|id| match s.group_of(id).and_then(|g| s.group(g)) {
                Some(group) => group.name.clone(),
                None => ".".to_owned(),
            })
            .collect()
    }

    fn named(s: &Set, name: &str) -> GroupId {
        s.group_named(name).expect("a group of that name")
    }

    /// `ids` reordered as the positions in `order` say.
    fn at(ids: &[TabId], order: &[usize]) -> Vec<TabId> {
        order.iter().map(|&i| ids[i]).collect()
    }

    // ---------------------------------------------------------------------------- colours

    #[test]
    fn there_are_nine_colours_and_their_names_round_trip() {
        let names: Vec<_> = GroupColor::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            names,
            [
                "grey", "blue", "red", "yellow", "green", "pink", "purple", "cyan", "orange"
            ]
        );
        for c in GroupColor::ALL {
            assert_eq!(GroupColor::from_name(c.as_str()), Some(c));
        }
        for other in ["", "Grey", "gray", "blue ", "black"] {
            assert_eq!(GroupColor::from_name(other), None, "{other:?}");
        }
    }

    #[test]
    fn a_new_group_gets_the_first_unused_colour_else_the_least_used() {
        use GroupColor::{Blue, Grey, Orange, Red};
        assert_eq!(default_color([]), Grey);
        assert_eq!(default_color([Grey]), Blue);
        assert_eq!(default_color([Grey, Red]), Blue);
        assert_eq!(default_color([Blue, Orange]), Grey);
        // All used once: the first. Grey twice: the next one used once.
        assert_eq!(default_color(GroupColor::ALL), Grey);
        assert_eq!(
            default_color(GroupColor::ALL.into_iter().chain([Grey])),
            Blue
        );
        // Only orange is used once, everything else twice.
        let mut twice: Vec<_> = GroupColor::ALL.into_iter().chain(GroupColor::ALL).collect();
        twice.pop();
        assert_eq!(default_color(twice), Orange);
    }

    #[test]
    fn a_groups_debug_shows_the_names_length_never_the_name() {
        let (mut s, ids) = set(".");
        let g = s.set_group(ids[0], "hunter2");
        for shown in [format!("{:?}", s.group(g)), format!("{s:?}")] {
            assert!(!shown.contains("hunter2"), "{shown}");
        }
    }

    /// WCAG 2 relative luminance of an sRGB colour in 0..=1.
    fn luminance(rgb: [f32; 3]) -> f32 {
        let lin = rgb.map(|c| {
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        });
        0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]
    }

    fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn unit(rgb: [u8; 3]) -> [f32; 3] {
        rgb.map(|v| f32::from(v) / 255.0)
    }

    /// The darkest and the lightest colour behind the glass that the looks are judged over
    /// (docs/glass-styling.md, the header tint: `#10`, `#30`, `#60`): what the OS material shows,
    /// not the raw desktop under it.
    const BACKDROPS: [u8; 2] = [0x10, 0x60];

    /// What lies under a card: the window panel of `look` over each of [`BACKDROPS`] (glass),
    /// and the opaque panel without glass.
    fn card_backdrops(look: Theme) -> Vec<[f32; 3]> {
        let panel = |glass| {
            crate::render::theme(glass, look, crate::settings::WindowOpacity::P0)
                .glass
                .panel
                .premultiplied()
        };
        let over = |p: [f32; 4], back: f32| [0, 1, 2].map(|i| p[i] + (1.0 - p[3]) * back);
        BACKDROPS
            .iter()
            .map(|&b| over(panel(true), f32::from(b) / 255.0))
            .chain([over(panel(false), 0.0)])
            .collect()
    }

    /// The card of `color` over `back`: its fill's tint.
    fn card(color: GroupColor, look: Theme, back: [f32; 3]) -> [f32; 3] {
        let fill = unit(color.tone(look).fill);
        [0, 1, 2].map(|i| fill[i] * CARD_TINT + back[i] * (1.0 - CARD_TINT))
    }

    /// The contrast of a group's text on its tinted card, the worst over the backdrops.
    fn worst_ratio(color: GroupColor, look: Theme) -> f32 {
        let text = unit(color.tone(look).text);
        card_backdrops(look)
            .into_iter()
            .map(|back| contrast(text, card(color, look, back)))
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn the_contrast_instrument_gives_the_known_ratios() {
        let (black, white) = ([0.0; 3], [1.0; 3]);
        assert!((contrast(black, white) - 21.0).abs() < 0.01);
        assert!((contrast(white, white) - 1.0).abs() < 0.001);
        // #767676 on white is the textbook 4.54:1.
        assert!((contrast(unit([0x76; 3]), white) - 4.54).abs() < 0.01);
        // The panel is not opaque, so the two backdrops differ, and the opaque panel covers its own.
        for look in Theme::ALL.iter().copied() {
            let b = card_backdrops(look);
            assert_eq!(b.len(), 3);
            assert!(b[0][0] < b[1][0], "{look:?} {b:?}");
        }
    }

    #[test]
    fn every_colours_text_is_readable_on_its_card_under_both_looks() {
        for look in Theme::ALL.iter().copied() {
            for color in GroupColor::ALL {
                let ratio = worst_ratio(color, look);
                println!("{look:?} {}: {ratio:.2}", color.as_str());
                assert!(ratio >= 4.5, "{look:?} {}: {ratio:.2}", color.as_str());
            }
        }
    }

    /// The unread dot is the theme's attention colour on every card: a non-text mark, so 3:1
    /// over each backdrop.
    #[test]
    fn the_unread_dot_stands_out_on_every_colours_card_under_both_looks() {
        for look in Theme::ALL.iter().copied() {
            let dot =
                crate::render::theme(true, look, crate::settings::WindowOpacity::P0).attention();
            assert_eq!(dot[3], 255, "opaque, so the card does not change it");
            let dot = unit([dot[0], dot[1], dot[2]]);
            for color in GroupColor::ALL {
                let ratios: Vec<f32> = card_backdrops(look)
                    .into_iter()
                    .map(|back| contrast(dot, card(color, look, back)))
                    .collect();
                println!(
                    "dot {look:?} {}: #10 {:.2}, #60 {:.2}, opaque {:.2}",
                    color.as_str(),
                    ratios[0],
                    ratios[1],
                    ratios[2]
                );
                assert!(
                    ratios.iter().all(|r| *r >= 3.0),
                    "{look:?} {}: {ratios:?}",
                    color.as_str()
                );
            }
        }
    }

    #[test]
    fn a_typed_group_name_follows_the_label_rule_and_must_be_free() {
        let taken = vec!["api".to_owned(), "orch: shop".to_owned()];
        let at = |n: &str| name_problem(n, &taken);
        assert_eq!(at("infra"), None);
        assert_eq!(at("API"), None, "names are compared exactly");
        assert_eq!(at("a b"), None);
        assert_eq!(at(&"é".repeat(MAX_GROUP_CHARS)), None);
        assert_eq!(at(""), Some(NameProblem::Empty));
        assert_eq!(at("api"), Some(NameProblem::Taken));
        assert_eq!(
            at(&"é".repeat(MAX_GROUP_CHARS + 1)),
            Some(NameProblem::TooLong)
        );
        for edge in [" api2", "api2 ", " ", "\u{3000}x"] {
            assert_eq!(at(edge), Some(NameProblem::EdgeSpace), "{edge:?}");
        }
        for bad in ["a\nb", "a\tb", "a\u{202e}b", "a\u{2028}b", "a\u{7}b"] {
            assert_eq!(at(bad), Some(NameProblem::Control), "{bad:?}");
        }
        // Everything the rule accepts is accepted by `ctl`'s rule and vice versa.
        for n in ["x", "orch: shop2", " x", "a\nb", ""] {
            assert_eq!(
                name_problem(n, &[]).is_none(),
                label_ok(n, MAX_GROUP_CHARS),
                "{n:?}"
            );
        }
    }

    #[test]
    fn a_new_groups_prefill_is_the_first_free_number() {
        let name = |n: usize| format!("Group {n}");
        let taken = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert_eq!(first_free_name(name, &[]), "Group 1");
        assert_eq!(first_free_name(name, &taken(&["api"])), "Group 1");
        assert_eq!(first_free_name(name, &taken(&["Group 1"])), "Group 2");
        assert_eq!(
            first_free_name(name, &taken(&["Group 2", "Group 1", "Group 3"])),
            "Group 4"
        );
        assert_eq!(
            first_free_name(name, &taken(&["Group 1", "Group 3"])),
            "Group 2"
        );
    }

    // ------------------------------------------------------------------------- membership

    #[test]
    fn colours_follow_the_open_groups_only() {
        let (mut s, ids) = set("...");
        let color = |s: &Set, g| s.group(g).expect("group").color;
        let a = s.set_group(ids[0], "a");
        let b = s.set_group(ids[1], "b");
        assert_eq!(
            (color(&s, a), color(&s, b)),
            (GroupColor::Grey, GroupColor::Blue)
        );
        // Grey is free again once its group is gone.
        s.close(ids[0]);
        let c = s.set_group(ids[2], "c");
        assert_eq!(color(&s, c), GroupColor::Grey);
        // A tab alone in its group frees that colour before the new group picks one.
        let d = s.set_group(ids[2], "d");
        assert_eq!(color(&s, d), GroupColor::Grey);
        assert_eq!(s.group(c), None);
    }

    #[test]
    fn a_new_group_leaves_the_tab_in_place_unless_that_splits_its_old_group() {
        // No old group, and either end of an old run: in place.
        for (picture, moved, want) in [
            ("...", 1, ".n."),
            ("aaa.", 0, "naa."),
            ("aaa.", 2, "aan."),
            (".a.", 1, ".n."),
        ] {
            let (mut s, ids) = set(picture);
            let g = s.set_group(ids[moved], "n");
            s.assert_invariants();
            assert_eq!(shape(&s), want, "{picture} {moved}");
            assert_eq!(s.ids(), ids, "{picture} {moved}");
            assert_eq!(s.group_of(ids[moved]), Some(g));
            assert!(!s.group(g).expect("group").collapsed);
        }
        // Between members: after the old run.
        let (mut s, ids) = set(".aaaa.");
        s.set_group(ids[2], "n");
        s.assert_invariants();
        assert_eq!(shape(&s), ".aaan.");
        assert_eq!(s.ids(), at(&ids, &[0, 1, 3, 4, 2, 5]));
        assert_eq!(s.active(), Some(ids[0]));
    }

    #[test]
    fn joining_a_group_puts_the_tab_after_its_last_member() {
        // From the right, from the left, and out of the middle of another group on either side.
        let cases: [(&str, usize, &[usize], &str); 4] = [
            ("aa..", 3, &[0, 1, 3, 2], "aaa."),
            ("..aa", 0, &[1, 2, 3, 0], ".aaa"),
            ("bbbaa", 1, &[0, 2, 3, 4, 1], "bbaaa"),
            ("aabbb", 3, &[0, 1, 3, 2, 4], "aaabb"),
        ];
        for (picture, moved, order, want) in cases {
            let (mut s, ids) = set(picture);
            let a = named(&s, "a");
            assert_eq!(s.set_group(ids[moved], "a"), a);
            s.assert_invariants();
            assert_eq!(shape(&s), want, "{picture} {moved}");
            assert_eq!(s.ids(), at(&ids, order), "{picture}");
        }
        // The last member of a group leaves it, and the group is gone.
        let (mut s, ids) = set("b.a");
        let b = named(&s, "b");
        s.set_group(ids[0], "a");
        s.assert_invariants();
        assert_eq!(shape(&s), ".aa");
        assert_eq!(s.group(b), None);
        assert_eq!(s.group_named("b"), None);
    }

    #[test]
    fn setting_the_group_a_tab_is_in_changes_nothing_and_an_unknown_tab_makes_none() {
        let (mut s, ids) = set("aaa.");
        let a = named(&s, "a");
        s.set_collapsed(a, true);
        let before = s.group(a).cloned();
        for id in &ids[..3] {
            assert_eq!(s.set_group(*id, "a"), a);
            assert_eq!(s.ids(), ids);
        }
        assert_eq!(s.group(a).cloned(), before);
        assert_eq!(s.set_group(999, "z"), GroupId::NONE);
        assert_eq!(s.group_named("z"), None);
        assert_eq!(s.groups().count(), 1);
        // Names are compared exactly.
        let upper = s.set_group(ids[3], "A");
        assert_ne!(upper, a);
        s.assert_invariants();
    }

    #[test]
    fn a_dropped_groups_id_never_comes_back() {
        let (mut s, ids) = set("..");
        let first = s.set_group(ids[0], "a");
        s.ungroup(ids[0]);
        assert_eq!(s.group(first), None);
        let second = s.set_group(ids[0], "a");
        assert_ne!(first, second);
        assert_ne!(second, GroupId::NONE);
    }

    #[test]
    fn ungroup_moves_a_tab_between_members_to_after_the_run() {
        for (moved, order, want) in [
            (1, [0, 1, 2, 3, 4], "..aa."),
            (2, [0, 1, 3, 2, 4], ".aa.."),
            (3, [0, 1, 2, 3, 4], ".aa.."),
        ] {
            let (mut s, ids) = set(".aaa.");
            s.ungroup(ids[moved]);
            s.assert_invariants();
            assert_eq!(shape(&s), want, "{moved}");
            assert_eq!(s.ids(), at(&ids, &order), "{moved}");
        }
        // The only member, a tab in no group, and no tab.
        let (mut s, ids) = set(".a");
        let a = named(&s, "a");
        s.ungroup(ids[0]);
        s.ungroup(999);
        assert_eq!(shape(&s), ".a");
        s.ungroup(ids[1]);
        assert_eq!((shape(&s).as_str(), s.group(a)), ("..", None));
        assert_eq!(s.ids(), ids);
    }

    #[test]
    fn ungroup_all_keeps_the_tabs_in_place() {
        let (mut s, ids) = set("aab.");
        let a = named(&s, "a");
        s.ungroup_all(a);
        s.assert_invariants();
        assert_eq!(shape(&s), "..b.");
        assert_eq!(s.ids(), ids);
        assert_eq!(s.group(a), None);
        s.ungroup_all(a);
        assert_eq!(shape(&s), "..b.");
    }

    #[test]
    fn members_groups_and_close_ids_follow_the_tab_order() {
        let (mut s, ids) = set("aa.b");
        let (a, b) = (named(&s, "a"), named(&s, "b"));
        assert_eq!(s.members(a), at(&ids, &[0, 1]));
        assert_eq!(s.close_group_ids(a), at(&ids, &[0, 1]));
        assert_eq!(s.members(GroupId::NONE), Vec::<TabId>::new());
        let order = |s: &Set| s.groups().map(|(g, _)| g).collect::<Vec<_>>();
        assert_eq!(order(&s), [a, b]);
        assert!(s.move_group(b, 0));
        assert_eq!(order(&s), [b, a]);
        for id in s.close_group_ids(a) {
            s.close(id);
            s.assert_invariants();
        }
        assert_eq!(order(&s), [b]);
    }

    #[test]
    fn renaming_keeps_names_unique_and_recolouring_changes_the_colour_only() {
        let (mut s, _) = set("ab");
        let (a, b) = (named(&s, "a"), named(&s, "b"));
        assert!(!s.rename_group(a, "b"), "taken");
        assert_eq!(shape(&s), "ab");
        assert!(s.rename_group(a, "a"), "its own name");
        assert!(s.rename_group(a, "c"));
        assert_eq!((shape(&s).as_str(), named(&s, "c")), ("cb", a));
        assert!(s.recolor_group(b, GroupColor::Orange));
        assert_eq!(s.group(b).expect("b").color, GroupColor::Orange);
        assert_eq!(s.group(b).expect("b").name, "b");
        assert!(!s.rename_group(GroupId::NONE, "x"));
        assert!(!s.recolor_group(GroupId::NONE, GroupColor::Red));
        s.assert_invariants();
    }

    // --------------------------------------------------------------------------- collapse

    #[test]
    fn collapse_is_a_flag_that_neither_moves_nor_follows_the_active_tab() {
        let (mut s, ids) = set("aa.");
        let a = named(&s, "a");
        let collapsed = |s: &Set| s.group(a).expect("a").collapsed;
        // The active tab is a member: it stays active.
        s.set_collapsed(a, true);
        assert!(collapsed(&s));
        assert_eq!(s.active(), Some(ids[0]));
        // Activating a member does not expand, whichever way.
        s.activate(ids[1]);
        s.activate_index(0);
        s.activate_step(1);
        assert_eq!(s.active(), Some(ids[1]));
        assert!(collapsed(&s));
        s.expand_for(ids[2]);
        s.expand_for(999);
        assert!(collapsed(&s), "not its group");
        s.expand_for(ids[0]);
        assert!(!collapsed(&s));
        assert_eq!(s.active(), Some(ids[1]));
        s.set_collapsed(GroupId::NONE, true);
        // Every tab in collapsed groups is still allowed, and stepping runs over all of them.
        let (mut s, ids) = set("ab");
        for g in [named(&s, "a"), named(&s, "b")] {
            s.set_collapsed(g, true);
        }
        s.activate_step(1);
        assert_eq!(s.active(), Some(ids[1]));
    }

    // -------------------------------------------------------------------------- new tabs

    #[test]
    fn a_background_insert_changes_neither_the_active_tab_nor_the_others_order() {
        for (at_index, want_at) in [(0, 0), (1, 1), (2, 4), (3, 4), (4, 4), (5, 5), (99, 5)] {
            let (mut s, ids) = set(".aaa.");
            let next = s.next_id();
            let new = s.insert((), at_index, false);
            s.assert_invariants();
            assert_eq!(new, next);
            assert_eq!(s.index_of(new), Some(want_at), "{at_index}");
            assert_eq!(s.group_of(new), None);
            assert_eq!(s.active(), Some(ids[0]));
            let others: Vec<_> = s.ids().into_iter().filter(|i| *i != new).collect();
            assert_eq!(others, ids);
        }
        let (mut s, _) = set("..");
        let new = s.insert((), 1, true);
        assert_eq!(s.active(), Some(new));
        assert!(s.next_id() > new);
    }

    #[test]
    fn a_tab_opened_by_hand_joins_the_group_it_was_opened_from() {
        // From a grouped tab, whichever member: after the last one.
        for reference in [1, 2] {
            let (mut s, ids) = set(".aab.");
            let new = s.insert_by_hand((), Some(ids[reference]));
            s.assert_invariants();
            assert_eq!(shape(&s), ".aaab.");
            assert_eq!(s.index_of(new), Some(3));
            assert_eq!(s.active(), Some(new));
        }
        // From a tab in no group, from none and from one that is gone: at the end, in none.
        for case in 0..3 {
            let (mut s, ids) = set(".aa.");
            let reference = [Some(ids[0]), None, Some(999)][case];
            let new = s.insert_by_hand((), reference);
            assert_eq!(shape(&s), ".aa..", "{reference:?}");
            assert_eq!(s.index_of(new), Some(4));
            assert_eq!(s.active(), Some(new));
        }
    }

    #[test]
    fn closing_the_last_member_drops_the_group_and_the_neighbour_rule_ignores_collapse() {
        let (mut s, ids) = set(".a.");
        let a = named(&s, "a");
        s.close(ids[1]);
        s.assert_invariants();
        assert_eq!(s.group(a), None);

        let (mut s, ids) = set(".aa");
        let a = named(&s, "a");
        s.set_collapsed(a, true);
        s.close(ids[0]);
        assert_eq!(
            s.active(),
            Some(ids[1]),
            "the right neighbour, collapsed or not"
        );
        assert!(s.group(a).expect("a").collapsed);
    }

    // ----------------------------------------------------------------------------- moves

    #[test]
    fn a_pane_drop_into_a_group_takes_a_slot_inside_or_at_either_end_of_its_run() {
        // ".aa.." without the moved tab 4 is ".aa." and the run of a is 1..3.
        for slot in 0..=6 {
            let (mut s, ids) = set(".aa..");
            let a = named(&s, "a");
            let ok = s.move_tab(ids[4], slot, Some(a));
            s.assert_invariants();
            assert_eq!(ok, (1..=3).contains(&slot), "{slot}");
            if ok {
                assert_eq!(s.index_of(ids[4]), Some(slot));
                assert_eq!(s.group_of(ids[4]), Some(a));
            } else {
                assert_eq!((s.ids(), shape(&s).as_str()), (ids, ".aa.."), "{slot}");
            }
        }
    }

    #[test]
    fn a_pane_drop_out_of_the_groups_takes_a_slot_between_blocks() {
        // Without tab 0 the list is "aabb.": blocks end at 0, 2, 4 and 5.
        for slot in 0..=6 {
            let (mut s, ids) = set(".aabb.");
            let ok = s.move_tab(ids[0], slot, None);
            s.assert_invariants();
            assert_eq!(ok, [0, 2, 4, 5].contains(&slot), "{slot}");
            if !ok {
                assert_eq!((s.ids(), shape(&s).as_str()), (ids, ".aabb."), "{slot}");
            }
        }
        // A middle member cannot leave from where it is: that would split the run. An end can.
        let (mut s, ids) = set("aaa");
        assert!(!s.move_tab(ids[1], 1, None));
        assert_eq!(shape(&s), "aaa");
        assert!(s.move_tab(ids[0], 0, None));
        assert_eq!(shape(&s), ".aa");
        s.assert_invariants();
    }

    #[test]
    fn a_pane_drop_moves_between_groups_and_the_last_member_leaving_drops_the_group() {
        let (mut s, ids) = set("a.bb");
        let (a, b) = (named(&s, "a"), named(&s, "b"));
        s.activate(ids[0]);
        // The only member of `a` stays a member where it is, and cannot go elsewhere as one.
        assert!(s.move_tab(ids[0], 0, Some(a)));
        for slot in 1..=4 {
            assert!(!s.move_tab(ids[0], slot, Some(a)), "{slot}");
        }
        assert_eq!((s.ids(), shape(&s).as_str()), (ids.clone(), "a.bb"));
        // Into `b`: `a` is gone, and nothing more can be dropped into it.
        assert!(s.move_tab(ids[0], 1, Some(b)));
        s.assert_invariants();
        assert_eq!(shape(&s), ".bbb");
        assert_eq!(s.ids(), at(&ids, &[1, 0, 2, 3]));
        assert_eq!(s.group(a), None);
        assert!(!s.move_tab(ids[1], 0, Some(a)), "a group that is gone");
        assert!(!s.move_tab(999, 0, None));
        assert_eq!(
            s.active(),
            Some(ids[0]),
            "the active tab stays the same tab"
        );
        // The only member leaves its group where it is.
        let (mut s, ids) = set(".a.");
        assert!(s.move_tab(ids[1], 1, None));
        assert_eq!((shape(&s).as_str(), s.groups().count()), ("...", 0));
    }

    #[test]
    fn a_whole_group_moves_between_blocks_only_and_keeps_its_order() {
        // Without `a` the list is ".bb.": blocks end at 0, 1, 3 and 4.
        for slot in 0..=5 {
            let (mut s, ids) = set(".aabb.");
            let a = named(&s, "a");
            let ok = s.move_group(a, slot);
            s.assert_invariants();
            assert_eq!(ok, [0, 1, 3, 4].contains(&slot), "{slot}");
            if ok {
                assert_eq!(s.index_of(ids[1]), Some(slot));
                assert_eq!(s.index_of(ids[2]), Some(slot + 1));
                assert_eq!(s.members(a), at(&ids, &[1, 2]));
            } else {
                assert_eq!(s.ids(), ids, "{slot}");
                assert_eq!(shape(&s), ".aabb.", "{slot}");
            }
            assert_eq!(s.active(), Some(ids[0]), "{slot}");
        }
        let (mut s, _) = set("a");
        assert!(!s.move_group(GroupId::NONE, 0));
    }

    #[test]
    fn a_bar_drop_never_changes_a_group() {
        // A grouped tab is clamped into its run.
        for (from, to, order) in [
            (1, 0, [0, 1, 2, 3, 4]),
            (1, 4, [0, 2, 3, 1, 4]),
            (3, 0, [0, 3, 1, 2, 4]),
            (2, 1, [0, 2, 1, 3, 4]),
        ] {
            let (mut s, ids) = set(".aaa.");
            s.reorder_bar(from, to);
            s.assert_invariants();
            assert_eq!(shape(&s), ".aaa.", "{from}->{to}");
            assert_eq!(s.ids(), at(&ids, &order), "{from}->{to}");
        }
        // A tab in no group dropped inside a run lands at the nearer end, a tie after.
        // Without tab 0 the run of `a` is 0..4.
        for (to, want_at) in [(0, 0), (1, 0), (2, 4), (3, 4), (4, 4)] {
            let (mut s, ids) = set(".aaaa");
            s.reorder_bar(0, to);
            s.assert_invariants();
            assert_eq!(s.index_of(ids[0]), Some(want_at), "{to}");
            assert_eq!(s.group_of(ids[0]), None);
        }
        // The only member of a group has nowhere to go.
        let (mut s, ids) = set(".a.");
        for to in 0..3 {
            s.reorder_bar(1, to);
            assert_eq!((s.ids(), shape(&s).as_str()), (ids.clone(), ".a."), "{to}");
        }
    }

    // ------------------------------------------------------------------------ random ops

    const DEFAULT_SEEDS: u64 = 300;
    const OPS_PER_SEED: usize = 200;
    const NAMES: [&str; 5] = ["a", "A", "b", "orch: x", "日本"];

    /// splitmix64: reproducible, no dependency.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % n.max(1) as u64) as usize
        }

        fn flip(&mut self) -> bool {
            self.below(2) == 0
        }
    }

    /// Everything a caller can see of a set.
    #[derive(Clone, PartialEq, Debug)]
    struct Seen {
        tabs: Vec<(TabId, Option<GroupId>)>,
        groups: Vec<(GroupId, Group)>,
        active: Option<TabId>,
    }

    impl Seen {
        fn of(s: &Set) -> Seen {
            Seen {
                tabs: s.ids().into_iter().map(|i| (i, s.group_of(i))).collect(),
                groups: s.groups().map(|(i, g)| (i, g.clone())).collect(),
                active: s.active(),
            }
        }

        fn at(&self, id: TabId) -> Option<usize> {
            self.tabs.iter().position(|(i, _)| *i == id)
        }

        fn group_of(&self, id: TabId) -> Option<GroupId> {
            self.at(id).and_then(|i| self.tabs[i].1)
        }

        fn without(&self, id: TabId) -> Vec<(TabId, Option<GroupId>)> {
            self.tabs.iter().copied().filter(|t| t.0 != id).collect()
        }

        fn ids(&self) -> Vec<TabId> {
            self.tabs.iter().map(|t| t.0).collect()
        }

        fn members(&self, g: GroupId) -> Vec<TabId> {
            let of = self.tabs.iter().filter(|t| t.1 == Some(g));
            of.map(|t| t.0).collect()
        }
    }

    /// Whether every group's tabs are next to each other: the rule the moves are checked
    /// against, written without the set's own helpers.
    fn whole(tabs: &[(TabId, Option<GroupId>)]) -> bool {
        let mut closed = HashSet::new();
        let mut open = None;
        for &(_, group) in tabs {
            if group != open {
                if group.is_some_and(|g| closed.contains(&g)) {
                    return false;
                }
                closed.extend(open);
                open = group;
            }
        }
        true
    }

    /// The checker the random test trusts, on lists whose answer is known.
    #[test]
    fn the_whole_check_sees_a_split_group() {
        let (a, b) = (Some(GroupId(1)), Some(GroupId(2)));
        let tabs = |groups: &[Option<GroupId>]| -> Vec<(TabId, Option<GroupId>)> {
            (1..).zip(groups.iter().copied()).collect()
        };
        assert!(whole(&tabs(&[])));
        assert!(whole(&tabs(&[None, a, a, b, None, None])));
        for split in [
            &[a, None, a][..],
            &[a, b, a],
            &[a, a, b, b, a],
            &[None, b, None, b],
        ] {
            assert!(!whole(&tabs(split)), "{split:?}");
        }
    }

    /// How often the generator reached the cases that matter.
    #[derive(Default, Debug)]
    struct Reached {
        closed_active_grouped: usize,
        sole_member_regrouped: usize,
        split_avoided: usize,
        collapsed_the_active_group: usize,
        dropped_into: usize,
        dropped_out: usize,
        drop_refused: usize,
        sole_member_dropped: usize,
        group_moved: usize,
        group_refused: usize,
        bar_clamped: usize,
        bar_pushed_out: usize,
    }

    struct Run<'a> {
        s: Set,
        rng: Rng,
        last_tab: TabId,
        groups_seen: HashSet<GroupId>,
        last_group: u64,
        reached: &'a mut Reached,
    }

    impl Run<'_> {
        /// A tab of the set, mostly; now and then one that does not exist.
        fn some_tab(&mut self) -> TabId {
            let ids = self.s.ids();
            match self.rng.below(ids.len() + 1) {
                i if i < ids.len() => ids[i],
                _ => 1_000_000,
            }
        }

        /// A group of the set, mostly; now and then one that is gone.
        fn some_group(&mut self) -> GroupId {
            let open: Vec<_> = self.s.groups().map(|(g, _)| g).collect();
            match self.rng.below(open.len() + 1) {
                i if i < open.len() => open[i],
                _ => GroupId(self.rng.below(self.last_group as usize + 2) as u64),
            }
        }

        fn new_tab(&mut self, id: TabId, promised: TabId) {
            assert_eq!(id, promised, "next_id");
            assert!(id > self.last_tab, "tab id {id} again");
            self.last_tab = id;
        }

        fn step(&mut self) {
            let before = Seen::of(&self.s);
            let len = before.tabs.len();
            // A long list closes more, so every length stays common.
            let op = if len > 10 && self.rng.flip() {
                3
            } else {
                self.rng.below(15)
            };
            match op {
                0 => self.by_hand(&before),
                1 => self.insert(&before),
                2 => {
                    let promised = self.s.next_id();
                    let id = self.s.push(());
                    self.new_tab(id, promised);
                    let after = Seen::of(&self.s);
                    assert_eq!(after.tabs.last(), Some(&(id, None)));
                    assert_eq!(after.without(id), before.tabs);
                    assert_eq!(after.active, Some(id));
                }
                3 => self.close(&before),
                4 => {
                    match self.rng.below(3) {
                        0 => {
                            let id = self.some_tab();
                            assert_eq!(self.s.activate(id), before.at(id).is_some());
                        }
                        1 => self.s.activate_step(self.rng.below(7) as isize - 3),
                        _ => {
                            let i = self.rng.below(len + 2);
                            assert_eq!(self.s.activate_index(i), i < len);
                        }
                    }
                    let after = Seen::of(&self.s);
                    assert_eq!(
                        (after.tabs, after.groups),
                        (before.tabs, before.groups),
                        "activation changes the active tab only"
                    );
                }
                5 => {
                    let id = self.some_tab();
                    let name = NAMES[self.rng.below(NAMES.len())];
                    self.set_group(&before, id, name);
                }
                6 if self.rng.flip() => {
                    // A tab between two members of its group, into a group not there yet.
                    let between = before.tabs.windows(3);
                    let between: Vec<_> = between
                        .filter(|w| w[0].1.is_some() && w[0].1 == w[1].1 && w[1].1 == w[2].1)
                        .map(|w| w[1].0)
                        .collect();
                    if let Some(&id) = between.get(self.rng.below(between.len())) {
                        let name = format!("new {}", self.last_group);
                        self.set_group(&before, id, &name);
                    }
                }
                6 => {
                    // A group's only member.
                    let sole = before.groups.iter().map(|(g, _)| before.members(*g));
                    let sole: Vec<_> = sole.filter(|m| m.len() == 1).map(|m| m[0]).collect();
                    if let Some(&id) = sole.get(self.rng.below(sole.len())) {
                        let old = before.group_of(id);
                        let name = NAMES[self.rng.below(NAMES.len())];
                        self.set_group(&before, id, name);
                        if self.s.group_of(id) != old {
                            self.reached.sole_member_regrouped += 1;
                            assert!(old.is_some_and(|g| self.s.group(g).is_none()));
                        }
                    }
                }
                7 => self.ungroup(&before),
                8 => {
                    let g = self.some_group();
                    if self.rng.below(3) == 0 {
                        self.s.ungroup_all(g);
                        let want: Vec<_> = before
                            .tabs
                            .iter()
                            .map(|&(i, own)| (i, own.filter(|o| *o != g)))
                            .collect();
                        let after = Seen::of(&self.s);
                        assert_eq!((after.tabs, after.active), (want, before.active));
                    } else {
                        assert_eq!(self.s.close_group_ids(g), before.members(g));
                        assert_eq!(self.s.members(g), before.members(g));
                    }
                }
                9 => {
                    let g = self.some_group();
                    let open = self.s.group(g).is_some();
                    if self.rng.flip() {
                        let name = NAMES[self.rng.below(NAMES.len())];
                        let taken = self.s.group_named(name).is_some_and(|o| o != g);
                        assert_eq!(self.s.rename_group(g, name), open && !taken);
                    } else {
                        let color = GroupColor::ALL[self.rng.below(GroupColor::ALL.len())];
                        assert_eq!(self.s.recolor_group(g, color), open);
                    }
                    let after = Seen::of(&self.s);
                    assert_eq!((after.tabs, after.active), (before.tabs, before.active));
                }
                10 => self.collapse(&before),
                11 | 12 => self.move_tab(&before),
                13 => self.move_group(&before),
                _ => self.reorder_bar(&before),
            }
            self.s.assert_invariants();
            for (g, _) in self.s.groups() {
                // A group is new only with an id above every earlier one.
                if self.groups_seen.insert(g) {
                    assert!(g.0 > self.last_group, "{g:?} again");
                    self.last_group = g.0;
                }
            }
            // Only closing the last tab, or never having activated one, leaves none active.
            assert!(self.s.active().is_some() || self.s.is_empty() || before.active.is_none());
        }

        fn by_hand(&mut self, before: &Seen) {
            let reference = self.rng.flip().then(|| self.some_tab());
            let group = reference.and_then(|r| before.group_of(r));
            let promised = self.s.next_id();
            let id = self.s.insert_by_hand((), reference);
            self.new_tab(id, promised);
            let after = Seen::of(&self.s);
            let want_at = match group {
                Some(g) => {
                    let last = before.tabs.iter().rposition(|t| t.1 == Some(g));
                    last.expect("the reference's group has members") + 1
                }
                None => before.tabs.len(),
            };
            let mut want = before.tabs.clone();
            want.insert(want_at, (id, group));
            assert_eq!((after.tabs, after.active), (want, Some(id)));
        }

        fn insert(&mut self, before: &Seen) {
            let at = self.rng.below(before.tabs.len() + 3);
            let activate = self.rng.flip();
            let promised = self.s.next_id();
            let id = self.s.insert((), at, activate);
            self.new_tab(id, promised);
            let after = Seen::of(&self.s);
            // The first index from `at` on where a tab in no group keeps every run whole.
            let at = at.min(before.tabs.len());
            let want = (at..=before.tabs.len())
                .map(|i| {
                    let mut tabs = before.tabs.clone();
                    tabs.insert(i, (id, None));
                    tabs
                })
                .find(|tabs| whole(tabs));
            assert_eq!(Some(after.tabs), want);
            let active = if activate { Some(id) } else { before.active };
            assert_eq!(after.active, active);
        }

        fn close(&mut self, before: &Seen) {
            let id = match before.active {
                Some(active) if self.rng.flip() => active,
                _ => self.some_tab(),
            };
            let closed = self.s.close(id).is_some();
            assert_eq!(closed, before.at(id).is_some());
            let after = Seen::of(&self.s);
            assert_eq!(after.tabs, before.without(id));
            let want = match before.at(id) {
                Some(at) if before.active == Some(id) => {
                    if before.tabs[at].1.is_some() {
                        self.reached.closed_active_grouped += 1;
                    }
                    after.tabs.get(at).or(after.tabs.last()).map(|t| t.0)
                }
                _ => before.active,
            };
            assert_eq!(after.active, want);
        }

        fn set_group(&mut self, before: &Seen, id: TabId, name: &str) {
            let existing = self.s.group_named(name);
            let g = self.s.set_group(id, name);
            let after = Seen::of(&self.s);
            let Some(at) = before.at(id) else {
                assert_eq!((g, &after), (GroupId::NONE, before));
                return;
            };
            assert_eq!(after.group_of(id), Some(g));
            assert_eq!(self.s.group(g).map(|g| g.name.as_str()), Some(name));
            assert_eq!(after.without(id), before.without(id));
            assert_eq!(after.active, before.active);
            let now = after.at(id).expect("still listed");
            let old = before.group_of(id);
            match existing {
                Some(existing) => {
                    assert_eq!(g, existing);
                    if old == Some(g) {
                        assert_eq!(&after, before);
                    } else {
                        assert_eq!(after.members(g).last(), Some(&id), "joins at the end");
                    }
                }
                None => {
                    assert!(!before.groups.iter().any(|(i, _)| *i == g));
                    let splits = old.is_some_and(|o| {
                        at > 0
                            && before.tabs[at - 1].1 == Some(o)
                            && before.tabs.get(at + 1).is_some_and(|t| t.1 == Some(o))
                    });
                    if splits {
                        self.reached.split_avoided += 1;
                        let old = old.expect("a tab that splits has a group");
                        assert_eq!(after.tabs[now - 1].1, Some(old), "after the old run");
                        assert!(now > at);
                    } else {
                        assert_eq!(now, at, "in place");
                    }
                }
            }
        }

        fn ungroup(&mut self, before: &Seen) {
            let id = self.some_tab();
            self.s.ungroup(id);
            let after = Seen::of(&self.s);
            let Some(old) = before.group_of(id) else {
                assert_eq!(&after, before);
                return;
            };
            assert_eq!(after.group_of(id), None);
            assert_eq!(after.without(id), before.without(id));
            assert_eq!(after.active, before.active);
            let members = before.members(old);
            let between = members.first() != Some(&id) && members.last() != Some(&id);
            let stays = match members.last() {
                Some(&last) if between => last,
                _ => id,
            };
            let want = before.at(stays).expect("listed before");
            assert_eq!(after.at(id), Some(want));
        }

        fn collapse(&mut self, before: &Seen) {
            let (g, want) = if self.rng.flip() {
                let of_active = before.active.and_then(|a| before.group_of(a));
                let g = match of_active {
                    Some(g) if self.rng.flip() => g,
                    _ => self.some_group(),
                };
                let collapsed = self.rng.flip();
                if collapsed && before.active.and_then(|a| before.group_of(a)) == Some(g) {
                    self.reached.collapsed_the_active_group += 1;
                }
                self.s.set_collapsed(g, collapsed);
                (Some(g), collapsed)
            } else {
                let id = self.some_tab();
                self.s.expand_for(id);
                (before.group_of(id), false)
            };
            let after = Seen::of(&self.s);
            let mut expected = before.clone();
            for (i, group) in &mut expected.groups {
                if Some(*i) == g {
                    group.collapsed = want;
                }
            }
            assert_eq!(after, expected);
        }

        fn move_tab(&mut self, before: &Seen) {
            let id = match self.rng.below(4) {
                // A grouped tab onto its own place is the case a random slot rarely reaches.
                0 => before.active.unwrap_or(1),
                _ => self.some_tab(),
            };
            let own = before.group_of(id);
            let slot = match before.at(id) {
                Some(at) if self.rng.below(3) == 0 => at,
                _ => self.rng.below(before.tabs.len() + 2),
            };
            let target = match self.rng.below(4) {
                0 => None,
                1 => own,
                _ => Some(self.some_group()),
            };
            let moved = self.s.move_tab(id, slot, target);
            let after = Seen::of(&self.s);

            // What the contract allows, worked out from the list alone.
            let rest = before.without(id);
            let sole = own.is_some_and(|g| before.members(g) == [id]);
            let allowed = before.at(id).filter(|_| slot <= rest.len()).and_then(|at| {
                if target.is_some_and(|g| before.members(g).is_empty()) {
                    return None;
                }
                if sole && target == own && slot != at {
                    return None;
                }
                let mut tabs = rest.clone();
                tabs.insert(slot, (id, target));
                whole(&tabs).then_some(tabs)
            });
            assert_eq!(
                moved,
                allowed.is_some(),
                "{before:?} {id} {slot} {target:?}"
            );
            match allowed {
                Some(tabs) => {
                    assert_eq!((after.tabs, after.active), (tabs, before.active));
                    if sole {
                        self.reached.sole_member_dropped += 1;
                    }
                    match target {
                        Some(_) if target != own => self.reached.dropped_into += 1,
                        None if own.is_some() => self.reached.dropped_out += 1,
                        _ => {}
                    }
                }
                None => {
                    assert_eq!(&after, before, "a refused move changes nothing");
                    self.reached.drop_refused += 1;
                }
            }
        }

        fn move_group(&mut self, before: &Seen) {
            let g = self.some_group();
            let slot = self.rng.below(before.tabs.len() + 2);
            let moved = self.s.move_group(g, slot);
            let after = Seen::of(&self.s);
            let (run, mut rest): (Vec<_>, Vec<_>) =
                before.tabs.iter().partition(|t| t.1 == Some(g));
            let allowed = (!run.is_empty() && slot <= rest.len()).then(|| {
                rest.splice(slot..slot, run);
                rest
            });
            let allowed = allowed.filter(|tabs| whole(tabs));
            assert_eq!(moved, allowed.is_some(), "{before:?} {g:?} {slot}");
            match allowed {
                Some(tabs) => {
                    assert_eq!((after.tabs, after.active), (tabs, before.active));
                    self.reached.group_moved += 1;
                }
                None => {
                    assert_eq!(&after, before, "a refused move changes nothing");
                    self.reached.group_refused += 1;
                }
            }
        }

        fn reorder_bar(&mut self, before: &Seen) {
            let len = before.tabs.len();
            let loose: Vec<_> = (0..len).filter(|&i| before.tabs[i].1.is_none()).collect();
            let from = match loose.get(self.rng.below(loose.len())) {
                Some(&i) if self.rng.flip() => i,
                _ => self.rng.below(len + 1),
            };
            let to = self.rng.below(len + 1);
            self.s.reorder_bar(from, to);
            let after = Seen::of(&self.s);
            if from >= len || to >= len {
                assert_eq!(&after, before);
                return;
            }
            let (id, own) = before.tabs[from];
            assert_eq!(after.group_of(id), own, "membership");
            assert_eq!(after.without(id), before.without(id));
            assert_eq!(after.active, before.active);
            // The nearest index to `to` that keeps every run whole; of two as near, the later.
            let rest = before.without(id);
            let want = (0..=rest.len())
                .filter(|&i| {
                    let mut tabs = rest.clone();
                    tabs.insert(i, (id, own));
                    let sole = own.is_some_and(|g| before.members(g) == [id]);
                    whole(&tabs) && (!sole || i == from)
                })
                .min_by_key(|&i| (i.abs_diff(to), usize::MAX - i))
                .expect("where it was always fits");
            assert_eq!(after.at(id), Some(want), "{before:?} {from}->{to}");
            if want != to {
                match own {
                    Some(_) => self.reached.bar_clamped += 1,
                    None => self.reached.bar_pushed_out += 1,
                }
            }
            assert_eq!(after.ids().len(), len);
        }
    }

    #[test]
    fn random_operations_keep_every_group_whole_and_do_what_each_says() {
        let seeds = std::env::var("LUNETERM_GROUPS_FUZZ_SEEDS").map_or(DEFAULT_SEEDS, |v| {
            v.parse().expect("LUNETERM_GROUPS_FUZZ_SEEDS is a number")
        });
        let mut reached = Reached::default();
        for seed in 0..seeds {
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut run = Run {
                    s: Set::new(),
                    rng: Rng(seed),
                    last_tab: 0,
                    groups_seen: HashSet::new(),
                    last_group: 0,
                    reached: &mut reached,
                };
                for _ in 0..OPS_PER_SEED {
                    run.step();
                }
            }));
            assert!(ran.is_ok(), "seed {seed} failed");
        }
        // The generator does reach the cases that matter.
        if seeds >= DEFAULT_SEEDS {
            let counts = [
                reached.closed_active_grouped,
                reached.sole_member_regrouped,
                reached.split_avoided,
                reached.collapsed_the_active_group,
                reached.dropped_into,
                reached.dropped_out,
                reached.drop_refused,
                reached.sole_member_dropped,
                reached.group_moved,
                reached.group_refused,
                reached.bar_clamped,
                reached.bar_pushed_out,
            ];
            assert!(counts.iter().all(|&n| n > 20), "{reached:?}");
        }
    }
}
