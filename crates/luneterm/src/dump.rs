//! Frame dumps (`LUNETERM_DUMP_FRAMES=<dir>`): the shell and a demo grid drawn offscreen on a
//! headless device and written as PNGs, with no window and no screen capture (the pixels come from
//! reading the offscreen target back). The frames are what the window would show, through the same
//! [`Renderer::draw`].
//!
//! The scenes are the shell with one default tab, and the tab strip, the close-confirm and rename
//! dialogs and a failed start (M10 step 2), then the settings page in its sections, the menus, a
//! palette switch and the opaque look Linux gets (step 6), and the tab menus with groups, the group
//! editor and the close-group question in both looks, and the side pane: its frame and its tab list
//! (groups, collapsed cards, unread marks, progress, a long list, the menus asked from it, and
//! drags of its rows and cards held in mid-air). A dialog or menu is drawn after its entry
//! animation, and a menu, scroll or drag the toolkit owns is driven with scripted input.
//!
//! The fonts are the window's, system fonts included (the demo has an emoji), so the PNGs are
//! reproducible on one machine and may differ between machines.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use lune_termview::{SelectionKind, SelectionRange};
use lune_ui::dump::{read_rgba, target, write_png};
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::{Event, Modifiers, PointerButton, RawInput, ScrollDelta};
use lune_ui::paint::Rect;
use lune_ui::text::TextRendering;
use lune_ui::widgets::menu::MenuPlace;
use lune_ui::widgets::popover::{PlaceAlign, Side};
use lune_ui::widgets::toast::Toasts;
use lune_ui::widgets::tone::Tone;

use crate::app::failure_grid;
use crate::demo::{demo_cursor, demo_grid};
use crate::fonts;
use crate::groups::{GroupColor, GroupId};
use crate::links;
use crate::menus::{MenuGroups, MenuKind, MenuModel};
use crate::notices::{Admit, Inbox, TOAST_LIFE, ToastSlots};
use crate::pane::{self, PaneEvent, PaneList};
use crate::render::{Content, InboxDraw, LinkTip, MenuDraw, Renderer};
use crate::settings::{DEFAULT_PANE_WIDTH, MAX_PANE_WIDTH, MIN_PANE_WIDTH};
use crate::settings::{Settings, Theme};
use crate::settings_page::{PageState, PageView, Section};
use crate::shell::{
    CLOSE_SESSION_TITLE, Chrome, HOST, InboxButton, InboxList, Layout, Modal, NoticeTexts,
    ScrollView, StripTab, TAB_CLOSABLE, WINDOWS_CHROME, close_group_message, notice_toast,
};
use crate::spawn::SpawnFailure;
use crate::tabs::{TabSet, confirm_message};

/// The offscreen colour format (the surface's preferred one).
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
/// The window's first size, logical px.
const SIZE: (f32, f32) = (1040.0, 660.0);

/// What a scene shows over the terminal.
#[derive(Debug, Clone, Copy)]
enum Overlay {
    None,
    /// The close-confirm dialog for `n` sessions.
    Confirm(usize),
    /// The rename dialog with this text in its field.
    Rename(&'static str),
    /// The group editor, new or editing, with this name and colour among the other groups
    /// `taken`.
    GroupEditor {
        editing: bool,
        name: &'static str,
        color: GroupColor,
        taken: &'static [&'static str],
    },
    /// The question before the group `name` and its `tabs` tabs are closed.
    CloseGroup {
        name: &'static str,
        tabs: usize,
    },
}

/// One notice a scene's inbox receives, for the tab at this index (the real `Inbox` calls, so the
/// strip's cue comes from the same rule the app uses).
#[derive(Debug, Clone, Copy)]
enum SceneNotice {
    Bell(usize),
    Message(usize, &'static str, &'static str),
}

/// A scene whose inbox holds more than a few notices: how it is filled (the real `Inbox` calls,
/// with explicit times; tab `n` is the scene's `n`-th tab, counted from 1), whether the list is
/// open, how far it is scrolled (wheel lines at its middle), and whether the bell is mid-ring: how
/// long before the scene's last frame the ring started (the last frame is the picture).
#[derive(Debug, Clone, Copy)]
struct InboxScene {
    fill: fn(&mut Inbox),
    open: bool,
    scroll: f32,
    ring_ago_ms: Option<u64>,
}

/// The clock of the inbox scenes: the entries' ages are counted from here.
const INBOX_NOW: Duration = Duration::from_secs(100_000);

/// The mockup's four rows: a message from `claude` now, two bells from `cargo build` 2 min ago, an
/// older message from `claude`, and an OSC 777 `Build finished` from `colors` an hour ago that is
/// read (the tab was looked at: `settle` on it, as the app does).
fn fill_four(inbox: &mut Inbox) {
    let ago = |secs: u64| INBOX_NOW - Duration::from_secs(secs);
    inbox.message(
        3,
        "Claude Code".to_owned(),
        "Claude is waiting for your input".to_owned(),
        ago(540),
    );
    inbox.message(4, "Build finished".to_owned(), String::new(), ago(3600));
    inbox.bell(2, 2, ago(120));
    inbox.message(
        3,
        "Claude Code".to_owned(),
        "Claude needs your permission to use Bash".to_owned(),
        ago(0),
    );
    inbox.settle(Some(4), true, |_| true);
}

/// The mockup's four rows, every one read: each tab looked at once through `settle`, as the app does
/// (the bell goes back to the plain one).
fn fill_read(inbox: &mut Inbox) {
    fill_four(inbox);
    for tab in 1..=4 {
        inbox.settle(Some(tab), true, |_| true);
    }
    assert_eq!(inbox.unread(), 0);
}

/// 50 rows: a list that scrolls, with an 80-character title and a 240-character body in it.
fn fill_long(inbox: &mut Inbox) {
    // Exact lengths: the caps of D9 (title 80, body 240) are what a program can send at most.
    let exact = |text: &str, n: usize| -> String { text.chars().cycle().take(n).collect() };
    let title80 = exact(
        "A program title that is far too long for one line of the list. ",
        80,
    );
    let body240 = exact(
        "A message body that runs on and on: it is wrapped to two lines under its title and cut with an ellipsis, so that no row grows past what the panel can show, whatever the program printed. ",
        240,
    );
    assert_eq!(
        (title80.chars().count(), body240.chars().count()),
        (80, 240)
    );
    for i in 0..50u64 {
        let at = INBOX_NOW - Duration::from_secs(i * 420);
        let tab = i % 4 + 1;
        let (title, body) = match i {
            10 => (title80.clone(), "A short body".to_owned()),
            11 => ("Claude Code".to_owned(), body240.clone()),
            _ if i % 3 == 0 => (String::new(), format!("Build {i} finished: 1342 passed")),
            _ => (
                "Claude Code".to_owned(),
                format!("Claude needs your permission to run command {i}"),
            ),
        };
        inbox.message(tab, title, body, at);
    }
    inbox.settle(Some(4), true, |_| true);
}

/// The settings page a scene shows.
#[derive(Debug, Clone, Copy)]
struct PageScene {
    section: Section,
    /// The combo list that is open, if any.
    combo: Option<crate::settings::SettingId>,
    /// The notice banner is up.
    restart: bool,
}

/// An open menu, forced by the dump. The tab's own menu is not forced: the scripted right-click
/// makes the frame report the request and the dump opens the menu then, as the app does.
#[derive(Debug, Clone, Copy)]
enum MenuScene {
    None,
    App,
    Terminal { selection: bool },
}

/// A link on the demo grid with the pointer on it from frame `move_at` on.
#[derive(Debug, Clone, Copy)]
struct LinkScene {
    move_at: usize,
}

/// Where the link scenes' URL is on the demo grid: a free row, and the column it starts at.
const LINK_ROW: usize = 13;
const LINK_COL: usize = 6;
const LINK_TEXT: &str = "see https://example.com/docs/getting-started?lang=en for details";

/// Input the dump sends at the start of a frame.
#[derive(Debug, Clone, Copy)]
enum Input {
    Move(f32, f32),
    /// The pointer comes to the point and the primary button goes down, and stays down.
    Press(f32, f32),
    SecondaryClick(f32, f32),
    /// A wheel of `y` lines with the pointer at the point.
    Wheel(f32, f32, f32),
}

impl Input {
    fn events(self) -> Vec<Event> {
        let mods = Modifiers::default();
        match self {
            Input::Move(x, y) => vec![Event::PointerMoved { pos: (x, y) }],
            Input::Press(x, y) => vec![
                Event::PointerMoved { pos: (x, y) },
                Event::PointerButton {
                    pos: (x, y),
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: mods,
                },
            ],
            Input::SecondaryClick(x, y) => ["press", "release"]
                .iter()
                .map(|k| Event::PointerButton {
                    pos: (x, y),
                    button: PointerButton::Secondary,
                    pressed: *k == "press",
                    modifiers: mods,
                })
                .collect(),
            Input::Wheel(x, y, lines) => vec![Event::Scroll {
                pos: (x, y),
                delta: ScrollDelta::Lines { x: 0.0, y: lines },
                modifiers: mods,
            }],
        }
    }
}

/// A group of a pane list scene: its name, colour and collapsed flag, and its tabs (the index of
/// the first and how many).
#[derive(Debug, Clone, Copy)]
struct DumpGroup {
    name: &'static str,
    color: GroupColor,
    collapsed: bool,
    tabs: (usize, usize),
}

/// A group of a list scene.
const fn dump_group(
    name: &'static str,
    color: GroupColor,
    collapsed: bool,
    first: usize,
    count: usize,
) -> DumpGroup {
    DumpGroup {
        name,
        color,
        collapsed,
        tabs: (first, count),
    }
}

/// One dumped frame: a file name, a scale, the scroll state the thumb shows, the tabs (none: the
/// single unlabelled tab of a fresh window), whether the window is focused, and what lies over it.
#[derive(Debug, Clone, Copy)]
struct Scene {
    name: &'static str,
    scale: u32,
    scroll: Option<ScrollView>,
    tabs: &'static [&'static str],
    active: usize,
    focused: bool,
    overlay: Overlay,
    /// The shell never started in this tab: the grid tells why.
    failed: bool,
    /// OS glass is behind the window; `false` is the opaque look (P22).
    glass: bool,
    /// The look whose layers and header tint the scene draws with.
    look: Theme,
    page: Option<PageScene>,
    menu: MenuScene,
    /// The open groups a tab menu shows, and the index of the group of the menu's tab.
    menu_groups: (&'static [&'static str], Option<usize>),
    /// The terminal palette, by name.
    palette: &'static str,
    /// Input at the start of frame `.0`.
    script: &'static [(usize, Input)],
    /// A selection on the demo grid.
    selected: bool,
    /// A URL on the demo grid under the pointer: underlined, and its tooltip once the delay has
    /// passed.
    link: Option<LinkScene>,
    /// What the inbox holds: the strip cues the tabs with unread entries that are not active.
    notices: &'static [SceneNotice],
    /// Each message of `notices` also earns a notification toast, pushed through the app's own
    /// path (the slots, then `notice_toast`).
    toasts: bool,
    /// The tabs (by index) that show an OSC 9;4 progress report, with its tone.
    progress: &'static [(usize, Tone)],
    /// A full inbox (the header button, and the list when open); the clock is [`INBOX_NOW`].
    inbox: Option<InboxScene>,
    /// The saved choice is the side pane, this wide; `None` is the top bar.
    pane: Option<u32>,
    /// The pane's tab list and the groups in it; `None` draws the pane's frame with no list.
    list: Option<&'static [DumpGroup]>,
    /// The chrome the frame is drawn with.
    chrome: Chrome,
    /// The window's width, logical px (its height is [`SIZE`]'s).
    width: u32,
}

/// The pane at its default width.
const PANE: Option<u32> = Some(DEFAULT_PANE_WIDTH);

/// The inbox with what the scene's notices put in it: the footer bell, the list closed.
const NOTICE_BELL: Option<InboxScene> = Some(InboxScene {
    fill: |_| {},
    open: false,
    scroll: 0.0,
    ring_ago_ms: None,
});

/// The mockup's four rows, the list open.
const FOUR_OPEN: Option<InboxScene> = Some(InboxScene {
    fill: fill_four,
    open: true,
    scroll: 0.0,
    ring_ago_ms: None,
});

/// 200 lines of history above a 35-line screen, scrolled to the middle: the thumb shows.
const SCROLLED: ScrollView = ScrollView {
    viewport_top: 100,
    base_y: 200,
    total_lines: 235,
};

const FOUR: &[&str] = &["zsh", "cargo build", "claude", "colors"];
const MANY: &[&str] = &[
    "tab 1", "tab 2", "tab 3", "tab 4", "tab 5", "tab 6", "tab 7", "tab 8", "tab 9", "tab 10",
    "tab 11", "tab 12", "tab 13", "tab 14", "tab 15", "tab 16",
];
const LONG: &[&str] =
    &["A tab title far longer than the widest tab the strip will ever draw for it, 80 chars!!"];

/// Open groups of a menu scene, one with a name past what a menu row shows.
const MENU_GROUPS: &[&str] = &[
    "api",
    "orch: a very long group name that runs well past sixty chars!!!",
    "infra",
];
/// A group's name at its longest, with no space to break at.
const LONGEST_NAME: &str = "orchestration-worktree-tab-groups-and-control-implementer-batch1";

const ATTENTION: &[SceneNotice] = &[
    SceneNotice::Bell(1),
    SceneNotice::Message(2, "Claude Code", "Claude needs your permission to use Bash"),
];

const TOAST: &[SceneNotice] = &[SceneNotice::Message(
    2,
    "Claude Code",
    "Claude needs your permission to use Bash",
)];
const TOAST_PLAIN: &[SceneNotice] = &[SceneNotice::Message(2, "", "Build finished: 1342 passed")];

/// The tabs of the pane list scenes, mock F's: a loose tab, then three groups.
const MOCK_TABS: &[&str] = &["zsh", "server", "logs", "claude", "web", "worker", "deploy"];
const MOCK_GROUPS: &[DumpGroup] = &[
    dump_group("api", GroupColor::Blue, false, 1, 3),
    dump_group("orch: shop", GroupColor::Pink, false, 4, 2),
    dump_group("infra", GroupColor::Yellow, true, 6, 1),
];
/// An unread member of an expanded card (`claude`) and of the collapsed one (`deploy`).
const MOCK_NOTICES: &[SceneNotice] = &[SceneNotice::Bell(3), SceneNotice::Bell(6)];
/// The scene of mock F: `server` active.
const MOCK: Scene = Scene {
    tabs: MOCK_TABS,
    active: 1,
    notices: MOCK_NOTICES,
    inbox: NOTICE_BELL,
    pane: PANE,
    list: Some(MOCK_GROUPS),
    ..BASE
};
/// Two loose tabs (the second's title 80 chars long), then one collapsed card of each colour, each
/// with an unread member; the first card's name is 64 chars long.
const COLOR_TABS: &[&str] = &[
    "zsh",
    "cargo test --workspace --all-features -- --nocapture 2>&1 | tee target/run.log &",
    "build",
    "server",
    "publish",
    "deploy",
    "watch",
    "web",
    "notes",
    "etl",
    "rollout",
];
const COLOR_GROUPS: &[DumpGroup] = &[
    dump_group(LONGEST_NAME, GroupColor::Grey, true, 2, 1),
    dump_group("api", GroupColor::Blue, true, 3, 1),
    dump_group("release", GroupColor::Red, true, 4, 1),
    dump_group("infra", GroupColor::Yellow, true, 5, 1),
    dump_group("tests", GroupColor::Green, true, 6, 1),
    dump_group("orch: shop", GroupColor::Pink, true, 7, 1),
    dump_group("docs", GroupColor::Purple, true, 8, 1),
    dump_group("data", GroupColor::Cyan, true, 9, 1),
    dump_group("deploy", GroupColor::Orange, true, 10, 1),
];
const COLOR_NOTICES: &[SceneNotice] = &[
    SceneNotice::Bell(2),
    SceneNotice::Bell(3),
    SceneNotice::Bell(4),
    SceneNotice::Bell(5),
    SceneNotice::Bell(6),
    SceneNotice::Bell(7),
    SceneNotice::Bell(8),
    SceneNotice::Bell(9),
    SceneNotice::Bell(10),
];
const COLORS: Scene = Scene {
    tabs: COLOR_TABS,
    notices: COLOR_NOTICES,
    inbox: NOTICE_BELL,
    pane: PANE,
    list: Some(COLOR_GROUPS),
    ..BASE
};
/// Sixty tabs in ten expanded groups, the active one deep in the list.
const SIXTY_TABS: &[&str] = &[
    "tab 1", "tab 2", "tab 3", "tab 4", "tab 5", "tab 6", "tab 7", "tab 8", "tab 9", "tab 10",
    "tab 11", "tab 12", "tab 13", "tab 14", "tab 15", "tab 16", "tab 17", "tab 18", "tab 19",
    "tab 20", "tab 21", "tab 22", "tab 23", "tab 24", "tab 25", "tab 26", "tab 27", "tab 28",
    "tab 29", "tab 30", "tab 31", "tab 32", "tab 33", "tab 34", "tab 35", "tab 36", "tab 37",
    "tab 38", "tab 39", "tab 40", "tab 41", "tab 42", "tab 43", "tab 44", "tab 45", "tab 46",
    "tab 47", "tab 48", "tab 49", "tab 50", "tab 51", "tab 52", "tab 53", "tab 54", "tab 55",
    "tab 56", "tab 57", "tab 58", "tab 59", "tab 60",
];
const SIXTY_GROUPS: &[DumpGroup] = &[
    dump_group("area 1", GroupColor::Grey, false, 0, 6),
    dump_group("area 2", GroupColor::Blue, false, 6, 6),
    dump_group("area 3", GroupColor::Red, false, 12, 6),
    dump_group("area 4", GroupColor::Yellow, false, 18, 6),
    dump_group("area 5", GroupColor::Green, false, 24, 6),
    dump_group("area 6", GroupColor::Pink, false, 30, 6),
    dump_group("area 7", GroupColor::Purple, false, 36, 6),
    dump_group("area 8", GroupColor::Cyan, false, 42, 6),
    dump_group("area 9", GroupColor::Orange, false, 48, 6),
    dump_group("area 10", GroupColor::Blue, false, 54, 6),
];
/// A progress report in a loose tab and in two members, one tone each.
const SHINES: &[(usize, Tone)] = &[(0, Tone::Neutral), (4, Tone::Warn), (5, Tone::Err)];
/// Points of mock F's list: over the `logs` row, and over the `api` card's header.
const OVER_LOGS_ROW: (f32, f32) = (110.0, 155.0);
const OVER_API_HEADER: (f32, f32) = (110.0, 90.0);

/// Drags of mock F's list: the press in the second frame, the pointer held where the third
/// takes it. `logs` to the end of its card; `zsh` between the `orch: shop` and `infra` cards;
/// `logs` over the collapsed `infra` header; the `orch: shop` card by its header to above `api`,
/// and over `api`'s rows, where nothing may drop; and `logs` into the gap under its card.
const DRAG_WITHIN: &[(usize, Input)] = &drag((110.0, 155.0), (110.0, 196.0));
const DRAG_LOOSE: &[(usize, Input)] = &drag((110.0, 50.0), (110.0, 318.0));
const DRAG_INTO: &[(usize, Input)] = &drag((110.0, 155.0), (110.0, 339.0));
const DRAG_CARD: &[(usize, Input)] = &drag((110.0, 231.0), (110.0, 80.0));
const DRAG_REFUSED: &[(usize, Input)] = &drag((110.0, 231.0), (110.0, 155.0));
const DRAG_OUT: &[(usize, Input)] = &drag((110.0, 155.0), (110.0, 210.0));

const fn drag(from: (f32, f32), to: (f32, f32)) -> [(usize, Input); 2] {
    [
        (1, Input::Press(from.0, from.1)),
        (2, Input::Move(to.0, to.1)),
    ]
}

/// A press on a row activates its tab, so a dragged tab is the active one; a press on a header
/// activates nothing (mock F's `server` stays).
const D1: Scene = Scene {
    active: 2,
    script: DRAG_WITHIN,
    ..MOCK
};
const D2: Scene = Scene {
    active: 0,
    script: DRAG_LOOSE,
    ..MOCK
};
const D3: Scene = Scene {
    active: 2,
    script: DRAG_INTO,
    ..MOCK
};
const D4: Scene = Scene {
    script: DRAG_CARD,
    ..MOCK
};
const D5: Scene = Scene {
    script: DRAG_REFUSED,
    ..MOCK
};
const D6: Scene = Scene {
    active: 2,
    script: DRAG_OUT,
    ..MOCK
};

const BASE: Scene = Scene {
    name: "",
    scale: 2,
    scroll: None,
    tabs: &[],
    active: 0,
    focused: true,
    overlay: Overlay::None,
    failed: false,
    glass: true,
    look: Theme::Luneterm,
    page: None,
    menu: MenuScene::None,
    menu_groups: (&[], None),
    palette: "One Dark",
    script: &[],
    selected: false,
    link: None,
    notices: &[],
    toasts: false,
    progress: &[],
    inbox: None,
    pane: None,
    list: None,
    chrome: HOST,
    width: SIZE.0 as u32,
};

const fn page(section: Section) -> Option<PageScene> {
    Some(PageScene {
        section,
        combo: None,
        restart: false,
    })
}

/// The system fonts the dumps' font combo offers: fixed, so a dump is the same on every machine.
const DUMP_SYSTEM_FONTS: [&str; 3] = ["Cascadia Code", "JetBrains Mono", "Menlo"];

/// A pointer over the page's content (so a wheel scrolls it).
const OVER_PAGE: (f32, f32) = (600.0, 300.0);
/// The second tab (`cargo build`, index 1) of the strip.
const OVER_TAB: (f32, f32) = (170.0, 50.0);

const SCENES: &[Scene] = &[
    Scene {
        name: "shell@1x.png",
        scale: 1,
        ..BASE
    },
    Scene {
        name: "shell@2x.png",
        ..BASE
    },
    Scene {
        name: "shell-scrolled@1x.png",
        scale: 1,
        scroll: Some(SCROLLED),
        ..BASE
    },
    Scene {
        name: "shell-scrolled@2x.png",
        scroll: Some(SCROLLED),
        ..BASE
    },
    Scene {
        name: "tabs@2x.png",
        tabs: FOUR,
        ..BASE
    },
    Scene {
        name: "tabs@1x.png",
        scale: 1,
        tabs: FOUR,
        ..BASE
    },
    Scene {
        name: "tabs-second@2x.png",
        tabs: FOUR,
        active: 1,
        ..BASE
    },
    Scene {
        name: "tabs-unfocused@2x.png",
        tabs: FOUR,
        focused: false,
        ..BASE
    },
    Scene {
        name: "tabs-many@2x.png",
        tabs: MANY,
        active: 15,
        ..BASE
    },
    Scene {
        name: "tabs-long@2x.png",
        tabs: LONG,
        ..BASE
    },
    Scene {
        name: "confirm-one@2x.png",
        tabs: FOUR,
        overlay: Overlay::Confirm(1),
        ..BASE
    },
    Scene {
        name: "confirm-many@2x.png",
        tabs: FOUR,
        overlay: Overlay::Confirm(3),
        ..BASE
    },
    Scene {
        name: "rename@2x.png",
        tabs: FOUR,
        overlay: Overlay::Rename("zsh"),
        ..BASE
    },
    Scene {
        name: "spawn-failed@2x.png",
        failed: true,
        ..BASE
    },
    // Step 6: the settings page.
    Scene {
        name: "settings-appearance@2x.png",
        tabs: FOUR,
        page: page(Section::Appearance),
        ..BASE
    },
    Scene {
        name: "settings-appearance@1x.png",
        scale: 1,
        tabs: FOUR,
        page: page(Section::Appearance),
        ..BASE
    },
    Scene {
        name: "settings-terminal@2x.png",
        tabs: FOUR,
        page: page(Section::Terminal),
        ..BASE
    },
    Scene {
        name: "settings-terminal-end@2x.png",
        tabs: FOUR,
        page: page(Section::Terminal),
        script: &[
            (1, Input::Move(OVER_PAGE.0, OVER_PAGE.1)),
            (2, Input::Wheel(OVER_PAGE.0, OVER_PAGE.1, -60.0)),
        ],
        ..BASE
    },
    Scene {
        name: "settings-about@2x.png",
        tabs: FOUR,
        page: page(Section::About),
        ..BASE
    },
    Scene {
        name: "settings-combo@2x.png",
        tabs: FOUR,
        page: Some(PageScene {
            section: Section::Appearance,
            combo: Some(crate::settings::SettingId::Glass),
            restart: false,
        }),
        ..BASE
    },
    Scene {
        name: "settings-font-combo@2x.png",
        tabs: FOUR,
        page: Some(PageScene {
            section: Section::Terminal,
            combo: Some(crate::settings::SettingId::Font),
            restart: false,
        }),
        ..BASE
    },
    Scene {
        name: "settings-restart@2x.png",
        tabs: FOUR,
        page: Some(PageScene {
            section: Section::Terminal,
            combo: None,
            restart: true,
        }),
        ..BASE
    },
    // Menus.
    Scene {
        name: "menu-app@2x.png",
        tabs: FOUR,
        menu: MenuScene::App,
        ..BASE
    },
    Scene {
        name: "menu-tab@2x.png",
        tabs: FOUR,
        script: &[
            (1, Input::Move(OVER_TAB.0, OVER_TAB.1)),
            (1, Input::SecondaryClick(OVER_TAB.0, OVER_TAB.1)),
        ],
        ..BASE
    },
    Scene {
        name: "menu-terminal@2x.png",
        tabs: FOUR,
        menu: MenuScene::Terminal { selection: true },
        selected: true,
        ..BASE
    },
    Scene {
        name: "menu-terminal-empty@2x.png",
        tabs: FOUR,
        menu: MenuScene::Terminal { selection: false },
        ..BASE
    },
    // Links (M10 step 4): the underline at once, the tooltip after the toolkit's delay.
    Scene {
        name: "link-hover@2x.png",
        tabs: FOUR,
        link: Some(LinkScene { move_at: 3 }),
        ..BASE
    },
    Scene {
        name: "link-tip@2x.png",
        tabs: FOUR,
        link: Some(LinkScene { move_at: 1 }),
        ..BASE
    },
    Scene {
        name: "link-tip@1x.png",
        scale: 1,
        tabs: FOUR,
        link: Some(LinkScene { move_at: 1 }),
        ..BASE
    },
    // A palette switch reaches the terminal.
    Scene {
        name: "palette@2x.png",
        tabs: FOUR,
        palette: "Dracula",
        ..BASE
    },
    // The Lunegit look: its layers and header tint, with glass (the proof the near-black look did
    // not move them), and with the app menu open (its floating-surface fill is its own).
    Scene {
        name: "lunegit@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        ..BASE
    },
    Scene {
        name: "lunegit-menu@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        menu: MenuScene::App,
        ..BASE
    },
    Scene {
        name: "lunegit-opaque@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        glass: false,
        ..BASE
    },
    // The opaque look (no OS glass: Linux, or Off).
    Scene {
        name: "opaque@2x.png",
        tabs: FOUR,
        glass: false,
        ..BASE
    },
    Scene {
        name: "opaque@1x.png",
        scale: 1,
        tabs: FOUR,
        glass: false,
        ..BASE
    },
    Scene {
        name: "opaque-settings@2x.png",
        tabs: FOUR,
        glass: false,
        page: page(Section::Appearance),
        ..BASE
    },
    Scene {
        name: "opaque-menu@2x.png",
        tabs: FOUR,
        glass: false,
        menu: MenuScene::App,
        ..BASE
    },
    Scene {
        name: "opaque-confirm@2x.png",
        tabs: FOUR,
        glass: false,
        overlay: Overlay::Confirm(1),
        ..BASE
    },
    // The attention cue: the second and third tabs have unread entries (a bell, a message), the
    // third also reports progress; the active first tab shows none.
    Scene {
        name: "tabs-attention@2x.png",
        tabs: FOUR,
        notices: ATTENTION,
        progress: &[(2, Tone::Neutral)],
        ..BASE
    },
    Scene {
        name: "lunegit-attention@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        notices: ATTENTION,
        progress: &[(2, Tone::Neutral)],
        ..BASE
    },
    // The notification toast: a message from the third tab while the first is in view (the
    // third also shows its dot); the toast sits in the card's corner.
    Scene {
        name: "toast@2x.png",
        tabs: FOUR,
        notices: TOAST,
        toasts: true,
        ..BASE
    },
    // An OSC 9 message has no title: the header names the tab, then the message.
    Scene {
        name: "toast-plain@2x.png",
        tabs: FOUR,
        notices: TOAST_PLAIN,
        toasts: true,
        ..BASE
    },
    Scene {
        name: "lunegit-toast@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        notices: TOAST,
        toasts: true,
        ..BASE
    },
    Scene {
        name: "opaque-toast@2x.png",
        tabs: FOUR,
        glass: false,
        notices: TOAST,
        toasts: true,
        ..BASE
    },
    // The inbox: the header bell (the list closed), and the list open with the mockup's four rows,
    // long and many rows scrolled, in both looks and the opaque one; then the bell with everything
    // read, and the bell 83 ms into its ring.
    Scene {
        name: "inbox-button@2x.png",
        tabs: FOUR,
        inbox: Some(InboxScene {
            fill: fill_four,
            open: false,
            scroll: 0.0,
            ring_ago_ms: None,
        }),
        ..BASE
    },
    Scene {
        name: "inbox@2x.png",
        tabs: FOUR,
        inbox: Some(InboxScene {
            fill: fill_four,
            open: true,
            scroll: 0.0,
            ring_ago_ms: None,
        }),
        ..BASE
    },
    Scene {
        name: "inbox-long@2x.png",
        tabs: FOUR,
        inbox: Some(InboxScene {
            fill: fill_long,
            open: true,
            scroll: -8.0,
            ring_ago_ms: None,
        }),
        ..BASE
    },
    Scene {
        name: "lunegit-inbox@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        inbox: Some(InboxScene {
            fill: fill_four,
            open: true,
            scroll: 0.0,
            ring_ago_ms: None,
        }),
        ..BASE
    },
    Scene {
        name: "opaque-inbox@2x.png",
        tabs: FOUR,
        glass: false,
        inbox: Some(InboxScene {
            fill: fill_four,
            open: true,
            scroll: 0.0,
            ring_ago_ms: None,
        }),
        ..BASE
    },
    Scene {
        name: "inbox-read@2x.png",
        tabs: FOUR,
        inbox: Some(InboxScene {
            fill: fill_read,
            open: false,
            scroll: 0.0,
            ring_ago_ms: None,
        }),
        ..BASE
    },
    Scene {
        name: "inbox-ring@2x.png",
        tabs: FOUR,
        inbox: Some(InboxScene {
            fill: fill_four,
            open: false,
            scroll: 0.0,
            ring_ago_ms: Some(RING_SCENE_AGO_MS),
        }),
        ..BASE
    },
    Scene {
        name: "menu-tab-ungrouped@2x.png",
        tabs: FOUR,
        menu_groups: (MENU_GROUPS, None),
        script: &[
            (1, Input::Move(OVER_TAB.0, OVER_TAB.1)),
            (1, Input::SecondaryClick(OVER_TAB.0, OVER_TAB.1)),
        ],
        ..BASE
    },
    Scene {
        name: "menu-tab-grouped@2x.png",
        tabs: FOUR,
        menu_groups: (MENU_GROUPS, Some(0)),
        script: &[
            (1, Input::Move(OVER_TAB.0, OVER_TAB.1)),
            (1, Input::SecondaryClick(OVER_TAB.0, OVER_TAB.1)),
        ],
        ..BASE
    },
    Scene {
        name: "group-new@2x.png",
        tabs: FOUR,
        overlay: Overlay::GroupEditor {
            editing: false,
            name: "Group 1",
            color: GroupColor::Grey,
            taken: &[],
        },
        ..BASE
    },
    Scene {
        name: "group-edit@2x.png",
        tabs: FOUR,
        overlay: Overlay::GroupEditor {
            editing: true,
            name: "api",
            color: GroupColor::Purple,
            taken: &["orch: shop"],
        },
        ..BASE
    },
    Scene {
        name: "group-invalid@2x.png",
        tabs: FOUR,
        overlay: Overlay::GroupEditor {
            editing: false,
            name: "api",
            color: GroupColor::Blue,
            taken: &["api"],
        },
        ..BASE
    },
    Scene {
        name: "group-close@2x.png",
        tabs: FOUR,
        overlay: Overlay::CloseGroup {
            name: "api",
            tabs: 3,
        },
        ..BASE
    },
    Scene {
        name: "lunegit-menu-tab-ungrouped@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        menu_groups: (MENU_GROUPS, None),
        script: &[
            (1, Input::Move(OVER_TAB.0, OVER_TAB.1)),
            (1, Input::SecondaryClick(OVER_TAB.0, OVER_TAB.1)),
        ],
        ..BASE
    },
    Scene {
        name: "lunegit-menu-tab-grouped@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        menu_groups: (MENU_GROUPS, Some(0)),
        script: &[
            (1, Input::Move(OVER_TAB.0, OVER_TAB.1)),
            (1, Input::SecondaryClick(OVER_TAB.0, OVER_TAB.1)),
        ],
        ..BASE
    },
    Scene {
        name: "lunegit-group-new@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        overlay: Overlay::GroupEditor {
            editing: false,
            name: "Group 1",
            color: GroupColor::Grey,
            taken: &[],
        },
        ..BASE
    },
    Scene {
        name: "lunegit-group-edit@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        overlay: Overlay::GroupEditor {
            editing: true,
            name: "api",
            color: GroupColor::Purple,
            taken: &["orch: shop"],
        },
        ..BASE
    },
    Scene {
        name: "lunegit-group-invalid@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        overlay: Overlay::GroupEditor {
            editing: false,
            name: "api",
            color: GroupColor::Blue,
            taken: &["api"],
        },
        ..BASE
    },
    Scene {
        name: "lunegit-group-close@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        overlay: Overlay::CloseGroup {
            name: "api",
            tabs: 3,
        },
        ..BASE
    },
    Scene {
        name: "group-close-long@2x.png",
        tabs: FOUR,
        overlay: Overlay::CloseGroup {
            name: LONGEST_NAME,
            tabs: 1,
        },
        ..BASE
    },
    // The side pane view (the frame: header, footer, the empty list): both looks, the footer with
    // and without the bell, the menus opening upward from it, Windows chrome, opaque, the settings
    // page in the card, and the width's two limits.
    Scene {
        name: "pane@2x.png",
        tabs: FOUR,
        pane: PANE,
        ..BASE
    },
    Scene {
        name: "lunegit-pane@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        pane: PANE,
        ..BASE
    },
    Scene {
        name: "pane-inbox@2x.png",
        tabs: FOUR,
        notices: ATTENTION,
        inbox: NOTICE_BELL,
        pane: PANE,
        ..BASE
    },
    Scene {
        name: "lunegit-pane-inbox@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        notices: ATTENTION,
        inbox: NOTICE_BELL,
        pane: PANE,
        ..BASE
    },
    Scene {
        name: "pane-menu-app@2x.png",
        tabs: FOUR,
        menu: MenuScene::App,
        pane: PANE,
        ..BASE
    },
    Scene {
        name: "pane-inbox-open@2x.png",
        tabs: FOUR,
        inbox: FOUR_OPEN,
        pane: PANE,
        ..BASE
    },
    Scene {
        name: "pane-windows@2x.png",
        tabs: FOUR,
        pane: PANE,
        chrome: WINDOWS_CHROME,
        ..BASE
    },
    Scene {
        name: "lunegit-pane-windows@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        pane: PANE,
        chrome: WINDOWS_CHROME,
        ..BASE
    },
    Scene {
        name: "pane-opaque@2x.png",
        tabs: FOUR,
        glass: false,
        notices: ATTENTION,
        inbox: NOTICE_BELL,
        pane: PANE,
        ..BASE
    },
    Scene {
        name: "pane-settings@2x.png",
        tabs: FOUR,
        notices: ATTENTION,
        inbox: NOTICE_BELL,
        page: page(Section::Appearance),
        pane: PANE,
        ..BASE
    },
    Scene {
        name: "lunegit-pane-200@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        notices: ATTENTION,
        inbox: NOTICE_BELL,
        pane: Some(MIN_PANE_WIDTH),
        ..BASE
    },
    Scene {
        name: "pane-420@2x.png",
        tabs: FOUR,
        notices: ATTENTION,
        inbox: NOTICE_BELL,
        pane: Some(MAX_PANE_WIDTH),
        ..BASE
    },
    // The top bar with the drawn caption buttons, its show button at the band's left, and the
    // window one pixel too narrow for the pane that is the saved choice (no show button).
    Scene {
        name: "bar-windows@2x.png",
        tabs: FOUR,
        notices: ATTENTION,
        progress: &[(2, Tone::Neutral)],
        chrome: WINDOWS_CHROME,
        ..BASE
    },
    Scene {
        name: "lunegit-bar-windows@2x.png",
        tabs: FOUR,
        look: Theme::Lunegit,
        notices: ATTENTION,
        progress: &[(2, Tone::Neutral)],
        chrome: WINDOWS_CHROME,
        ..BASE
    },
    Scene {
        name: "bar-narrow@2x.png",
        tabs: FOUR,
        notices: ATTENTION,
        progress: &[(2, Tone::Neutral)],
        pane: PANE,
        width: 737,
        ..BASE
    },
    // The side pane's tab list: mock F in both looks, opaque and at 1x, the active tab in a
    // collapsed card, a hovered row, every colour collapsed with unread members, a long scrolled
    // list, progress shines, and the card's and a row's menus.
    Scene {
        name: "pane-list@2x.png",
        ..MOCK
    },
    Scene {
        name: "pane-list@1x.png",
        scale: 1,
        ..MOCK
    },
    Scene {
        name: "lunegit-pane-list@2x.png",
        look: Theme::Lunegit,
        ..MOCK
    },
    Scene {
        name: "lunegit-pane-list@1x.png",
        scale: 1,
        look: Theme::Lunegit,
        ..MOCK
    },
    Scene {
        name: "pane-list-opaque@2x.png",
        glass: false,
        ..MOCK
    },
    Scene {
        name: "pane-list-collapsed-active@2x.png",
        active: 6,
        notices: &[SceneNotice::Bell(3)],
        ..MOCK
    },
    Scene {
        name: "lunegit-pane-list-collapsed-active@2x.png",
        active: 6,
        notices: &[SceneNotice::Bell(3)],
        look: Theme::Lunegit,
        ..MOCK
    },
    Scene {
        name: "pane-list-hover@2x.png",
        script: &[(1, Input::Move(OVER_LOGS_ROW.0, OVER_LOGS_ROW.1))],
        ..MOCK
    },
    Scene {
        name: "pane-list-colors@2x.png",
        ..COLORS
    },
    Scene {
        name: "lunegit-pane-list-colors@2x.png",
        look: Theme::Lunegit,
        ..COLORS
    },
    Scene {
        name: "pane-list-colors-opaque@2x.png",
        glass: false,
        ..COLORS
    },
    Scene {
        name: "pane-list-long@2x.png",
        tabs: SIXTY_TABS,
        active: 45,
        notices: &[SceneNotice::Bell(43)],
        inbox: NOTICE_BELL,
        pane: PANE,
        list: Some(SIXTY_GROUPS),
        ..BASE
    },
    // The same list under the pointer: the scroll thumb in its gutter.
    Scene {
        name: "pane-list-long-hover@2x.png",
        tabs: SIXTY_TABS,
        active: 45,
        notices: &[SceneNotice::Bell(43)],
        inbox: NOTICE_BELL,
        pane: PANE,
        list: Some(SIXTY_GROUPS),
        script: &[(1, Input::Move(OVER_LOGS_ROW.0, OVER_LOGS_ROW.1))],
        ..BASE
    },
    Scene {
        name: "pane-list-shine@2x.png",
        progress: SHINES,
        ..MOCK
    },
    Scene {
        name: "lunegit-pane-list-shine@2x.png",
        progress: SHINES,
        look: Theme::Lunegit,
        ..MOCK
    },
    Scene {
        name: "pane-list-menu-group@2x.png",
        script: &[
            (1, Input::Move(OVER_API_HEADER.0, OVER_API_HEADER.1)),
            (
                1,
                Input::SecondaryClick(OVER_API_HEADER.0, OVER_API_HEADER.1),
            ),
        ],
        ..MOCK
    },
    Scene {
        name: "pane-list-menu-tab@2x.png",
        script: &[
            (1, Input::Move(OVER_LOGS_ROW.0, OVER_LOGS_ROW.1)),
            (1, Input::SecondaryClick(OVER_LOGS_ROW.0, OVER_LOGS_ROW.1)),
        ],
        ..MOCK
    },
    // Drags held in mid-air (D24), at 2x and 1x.
    Scene {
        name: "pane-drag-d1@2x.png",
        ..D1
    },
    Scene {
        name: "lunegit-pane-drag-d1@2x.png",
        look: Theme::Lunegit,
        ..D1
    },
    Scene {
        name: "pane-drag-d1@1x.png",
        scale: 1,
        ..D1
    },
    Scene {
        name: "lunegit-pane-drag-d1@1x.png",
        scale: 1,
        look: Theme::Lunegit,
        ..D1
    },
    Scene {
        name: "pane-drag-d2@2x.png",
        ..D2
    },
    Scene {
        name: "pane-drag-d2@1x.png",
        scale: 1,
        ..D2
    },
    Scene {
        name: "pane-drag-d3@2x.png",
        ..D3
    },
    Scene {
        name: "lunegit-pane-drag-d3@2x.png",
        look: Theme::Lunegit,
        ..D3
    },
    Scene {
        name: "pane-drag-d3@1x.png",
        scale: 1,
        ..D3
    },
    Scene {
        name: "lunegit-pane-drag-d3@1x.png",
        scale: 1,
        look: Theme::Lunegit,
        ..D3
    },
    Scene {
        name: "pane-drag-d4@2x.png",
        ..D4
    },
    Scene {
        name: "lunegit-pane-drag-d4@2x.png",
        look: Theme::Lunegit,
        ..D4
    },
    Scene {
        name: "pane-drag-d4@1x.png",
        scale: 1,
        ..D4
    },
    Scene {
        name: "lunegit-pane-drag-d4@1x.png",
        scale: 1,
        look: Theme::Lunegit,
        ..D4
    },
    Scene {
        name: "pane-drag-d5@2x.png",
        ..D5
    },
    Scene {
        name: "pane-drag-d5@1x.png",
        scale: 1,
        ..D5
    },
    Scene {
        name: "pane-drag-d6@2x.png",
        ..D6
    },
    Scene {
        name: "pane-drag-d6@1x.png",
        scale: 1,
        ..D6
    },
];

/// How long before the last frame the ring scene's ring started: the first peak of the swing.
const RING_SCENE_AGO_MS: u64 = 83;

/// The menu rows' area for a scene: the app menu hangs from the `⋯` button, the terminal's
/// opens at a point on the demo grid.
const TERMINAL_MENU_AT: (f32, f32) = (420.0, 240.0);

/// Why a dump did not happen.
#[derive(Debug)]
pub enum DumpError {
    /// No usable GPU adapter or device: not a failure of the app.
    NoGpu(String),
    Failed(String),
}

/// Renders the scenes into `dir` (created if absent) and returns the files written, with the
/// app's text setting (smoothed glyphs).
pub fn dump_frames(dir: &Path) -> Result<Vec<PathBuf>, DumpError> {
    dump_frames_text(dir, TextRendering::SMOOTH)
}

/// [`dump_frames`] at an explicit text setting.
pub fn dump_frames_text(dir: &Path, text: TextRendering) -> Result<Vec<PathBuf>, DumpError> {
    dump_frames_gated_text(dir, text).map(|v| v.into_iter().map(|(p, _)| p).collect())
}

/// [`dump_frames`] with the least alpha of each frame (an opaque scene must have 255 everywhere).
pub fn dump_frames_gated(dir: &Path) -> Result<Vec<(PathBuf, u8)>, DumpError> {
    dump_frames_gated_text(dir, TextRendering::SMOOTH)
}

/// The environment variable that picks the dump's text setting.
pub const DUMP_TEXT_VAR: &str = "LUNETERM_DUMP_TEXT";

/// Reads [`DUMP_TEXT_VAR`]'s value: unset or `on` is smoothed glyphs (what the app draws), `off`
/// the unsmoothed look. Anything else is an error that names the variable and nothing else.
pub fn parse_dump_text(value: Option<&str>) -> Result<TextRendering, String> {
    match value {
        None | Some("on") => Ok(TextRendering::SMOOTH),
        Some("off") => Ok(TextRendering::OFF),
        Some(_) => Err(format!("{DUMP_TEXT_VAR} must be `on` or `off`")),
    }
}

/// [`dump_frames_gated`] at an explicit text setting.
pub fn dump_frames_gated_text(
    dir: &Path,
    text_rendering: TextRendering,
) -> Result<Vec<(PathBuf, u8)>, DumpError> {
    let gpu = pollster::block_on(Gpu::new(gpu::instance(None), None)).map_err(DumpError::NoGpu)?;
    std::fs::create_dir_all(dir)
        .map_err(|e| DumpError::Failed(format!("create {}: {e}", dir.display())))?;
    let (text, took) = fonts::text_cache();
    eprintln!("system fonts loaded in {} ms", took.as_millis());
    let settings = Settings::default().clamped();
    let mut renderer = Renderer::new(gpu, text, FORMAT, &settings, true);
    renderer.set_text_rendering(text_rendering);
    let mut written = Vec::new();
    for scene in SCENES {
        let path = dir.join(scene.name);
        let alpha = dump_scene(&mut renderer, scene, &path)
            .map_err(|e| DumpError::Failed(format!("{}: {e}", scene.name)))?;
        eprintln!("dumped {}", scene.name);
        written.push((path, alpha));
    }
    Ok(written)
}

/// The frames drawn per scene: the first starts what animates (a dialog opening, a tab revealed),
/// the last, after the focus animations, is the picture. A scene's clock starts at zero on a fresh UI
/// state, so a scene cannot show what an earlier one left behind.
const FRAME_TIMES_MS: [u64; 4] = [0, 800, 1600, 2400];

/// The groups a menu about group `own` shows, from a scene's set.
fn set_groups(set: &TabSet<()>, own: Option<GroupId>) -> MenuGroups {
    MenuGroups {
        own,
        open: set
            .groups()
            .map(|(g, group)| (g, group.name.clone()))
            .collect(),
    }
}

/// Draws `scene` into a fresh target, warming up first as the toolkit's own dump does, and writes
/// the last frame.
fn dump_scene(r: &mut Renderer, scene: &Scene, path: &Path) -> Result<u8, String> {
    r.reset_ui();
    r.set_glass(scene.glass, scene.look);
    r.set_term_theme(crate::palettes::theme_of(scene.palette));
    let scale = scene.scale as f32;
    let window = (scene.width as f32, SIZE.1);
    let size = (scene.width * scene.scale, SIZE.1 as u32 * scene.scale);
    let pane_width = scene.pane.unwrap_or(DEFAULT_PANE_WIDTH);
    r.set_pane(scene.pane.is_some(), pane_width);
    let placement = r.place_for(window, scale, scene.chrome).placement;
    let (cols, rows) = (placement.cols, placement.rows);
    let view_kind = pane::resolve(scene.pane.is_some(), pane_width, window.0, scene.chrome);
    let layout = Layout::for_view(Rect::new(0.0, 0.0, window.0, window.1), false, view_kind);
    let failure = SpawnFailure {
        kind: std::io::ErrorKind::NotFound,
        program: "pwsh.exe".to_owned(),
    };
    let mut cursor = demo_cursor();
    let mut grid = if scene.failed {
        cursor.visible = false;
        failure_grid(cols, rows, &failure)
    } else {
        demo_grid(cols, rows)
    };
    // A link scene: the URL is in the grid, shown as hovered as the app shows it (the same
    // `Hover`), with its tooltip hanging from its cells.
    let mut link_hover = None;
    let mut link_tip = None;
    if scene.link.is_some() {
        for (x, ch) in LINK_TEXT.chars().enumerate() {
            let mut cell = lune_termview::Cell::default();
            cell.set_char(ch);
            grid.set(LINK_ROW, LINK_COL + x, cell);
        }
        let uris = links::UriTable::new();
        let span = links::link_at(&grid, LINK_ROW, LINK_COL + 8, &uris)
            .ok_or("the demo URL is not a link")?;
        let rect = r
            .place_for(window, scale, scene.chrome)
            .grid_layout()
            .cell_rect(span.row, span.start, span.end + 1 - span.start);
        let url = links::tip_url(&span.uri).ok_or("the demo URL does not pass the check")?;
        let (hover, id) = links::Hover::show(span, &mut grid);
        link_hover = Some((hover, id));
        link_tip = Some((rect, url));
    }
    let texture = target(&r.gpu.device, FORMAT, size);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // The strip's tabs: ids are the places, so a scene's tabs are the same every run.
    let labelled = !scene.tabs.is_empty();
    let titles: &[&str] = if labelled { scene.tabs } else { &["Terminal"] };
    let active = scene.active.min(titles.len().saturating_sub(1));
    let mut inbox = Inbox::new();
    for notice in scene.notices {
        match *notice {
            SceneNotice::Bell(i) => inbox.bell(i as u64 + 1, 1, Duration::ZERO),
            SceneNotice::Message(i, title, body) => inbox.message(
                i as u64 + 1,
                title.to_owned(),
                body.to_owned(),
                Duration::ZERO,
            ),
        }
    }
    if let Some(i) = scene.inbox {
        (i.fill)(&mut inbox);
    }
    let strip: Vec<StripTab<'_>> = titles
        .iter()
        .enumerate()
        .map(|(i, title)| StripTab {
            id: i as u64 + 1,
            title,
            progress: scene
                .progress
                .iter()
                .find(|(tab, _)| *tab == i)
                .map(|(_, tone)| *tone),
            attention: inbox.tab_unread(i as u64 + 1) > 0 && i != active,
        })
        .collect();
    // The pane list's rows, through the app's own set and row builder: the set gives the same
    // tabs the ids of the strip.
    let mut set: TabSet<()> = TabSet::new();
    for _ in titles {
        set.push(());
    }
    for group in scene.list.unwrap_or_default() {
        for i in group.tabs.0..group.tabs.0 + group.tabs.1 {
            set.set_group(i as u64 + 1, group.name);
        }
        let id = set
            .group_named(group.name)
            .ok_or("a scene's group has no tab")?;
        set.recolor_group(id, group.color);
        set.set_collapsed(id, group.collapsed);
    }
    set.activate(active as u64 + 1);
    let rows = pane::rows(&set, &strip);
    // The toasts, through the app's path: the slots admit, `notice_toast` builds, the slots are told.
    let strings = crate::strings::english();
    let inbox_rows = crate::shell::inbox_rows(&inbox, INBOX_NOW, &strings, |id| {
        titles.get(id as usize - 1).map(|t| (*t).to_owned())
    });
    let inbox_message_rows: Vec<_> = inbox_rows.iter().map(|(_, r)| r.row()).collect();
    let inbox_open = scene.inbox.is_some_and(|i| i.open);
    let inbox_name = crate::shell::inbox_button_name(strings.tr("inbox.name"), inbox.unread());
    let mut inbox_rect: Option<Rect> = None;
    let mut toasts = Toasts::default();
    let mut slots: ToastSlots<_, Rect> = ToastSlots::new();
    if scene.toasts {
        let texts = NoticeTexts::of(&strings);
        let accent = r.theme().attention();
        for notice in scene.notices {
            let SceneNotice::Message(i, title, body) = *notice else {
                continue;
            };
            let tab = i as u64 + 1;
            let Admit::New { .. } = slots.admit(tab, Duration::ZERO) else {
                return Err("a scene toast was not admitted".to_owned());
            };
            let (spec, extras) = notice_toast(titles[i], title, body, &texts, accent, TOAST_LIFE);
            let id = toasts.push_with(spec, extras, Duration::ZERO);
            slots.shown(id, tab, Duration::ZERO);
        }
    }
    let label = labelled.then(|| titles[active]);
    let message = match scene.overlay {
        Overlay::Confirm(n) => confirm_message(n),
        Overlay::CloseGroup { name, tabs } => close_group_message(&strings, name, tabs),
        _ => String::new(),
    };
    let close_group_title = strings.tr("group.close.title").to_owned();
    let mut dont_ask = false;
    let mut name = String::new();
    // Set in each frame that draws the editor.
    let mut color;
    let taken: Vec<String> = match scene.overlay {
        Overlay::GroupEditor { taken, .. } => taken.iter().map(|t| (*t).to_owned()).collect(),
        _ => Vec::new(),
    };
    let menu_groups = MenuGroups {
        own: scene.menu_groups.1.map(|i| GroupId(i as u64 + 1)),
        open: scene
            .menu_groups
            .0
            .iter()
            .enumerate()
            .map(|(i, n)| (GroupId(i as u64 + 1), (*n).to_owned()))
            .collect(),
    };
    let page_settings = Settings::default().clamped();
    let mut edited = page_settings.clone();
    let mut page_state = scene.page.map(|p| {
        let mut state = PageState::new(p.section, &page_settings);
        state.combo = p.combo;
        state
    });
    let selection = [SelectionRange {
        start: (2, 4),
        end: (4, 30),
        kind: SelectionKind::Linear,
    }];
    let mut model = match scene.menu {
        MenuScene::None => None,
        MenuScene::App => Some(MenuModel::new(
            MenuKind::App {
                pane: scene.pane.is_some(),
            },
            &strings,
            cfg!(target_os = "macos"),
        )),
        MenuScene::Terminal { selection } => Some(MenuModel::new(
            MenuKind::Terminal { selection },
            &strings,
            cfg!(target_os = "macos"),
        )),
    };
    let mut place = match scene.menu {
        MenuScene::App => {
            // As the app: below the strip's button, above the pane's.
            let (side, align) = if layout.pane.is_some() {
                (Side::Top, PlaceAlign::Start)
            } else {
                (Side::Bottom, PlaceAlign::End)
            };
            MenuPlace::Anchor {
                rect: layout.overflow,
                side,
                align,
                gap: 4.0,
            }
        }
        _ => MenuPlace::At(TERMINAL_MENU_AT),
    };

    for (k, ms) in FRAME_TIMES_MS.iter().enumerate() {
        let mut events: Vec<Event> = scene
            .script
            .iter()
            .filter(|(frame, _)| *frame == k)
            .flat_map(|(_, input)| input.events())
            .collect();
        if let (Some(link), Some((rect, _))) = (scene.link, &link_tip)
            && link.move_at == k
        {
            events.push(Event::PointerMoved {
                pos: (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
            });
        }
        // The list scrolled: the wheel at its middle, once it has been drawn.
        if let (Some(i), Some(rect)) = (scene.inbox, inbox_rect)
            && k == 2
            && i.scroll != 0.0
        {
            events.push(Event::PointerMoved {
                pos: (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
            });
            events.extend(
                Input::Wheel(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0, i.scroll).events(),
            );
        }
        let input = RawInput {
            size: window,
            scale,
            time: Duration::from_millis(*ms),
            focused: scene.focused,
            events,
        };
        let modal = match scene.overlay {
            Overlay::None => None,
            Overlay::Confirm(_) => Some(Modal::Confirm {
                title: CLOSE_SESSION_TITLE,
                message: &message,
                dont_ask: Some(&mut dont_ask),
            }),
            Overlay::CloseGroup { .. } => Some(Modal::Confirm {
                title: &close_group_title,
                message: &message,
                dont_ask: None,
            }),
            Overlay::GroupEditor {
                editing,
                name: text,
                color: picked,
                ..
            } => {
                text.clone_into(&mut name);
                color = picked;
                Some(Modal::GroupEditor {
                    editing,
                    text: &mut name,
                    color: &mut color,
                    taken: &taken,
                    look: scene.look,
                    focus: k == 1,
                })
            }
            Overlay::Rename(text) => {
                text.clone_into(&mut name);
                Some(Modal::Rename {
                    text: &mut name,
                    // As the app: the caret goes into the field on the second frame.
                    focus: k == 1,
                })
            }
        };
        let content = Content {
            title: label,
            tabs: &strip,
            active_tab: Some(active as u64 + 1),
            pane_list: scene.list.map(|_| PaneList {
                rows: &rows,
                look: scene.look,
                // As the app's first frame.
                reveal: k == 0,
            }),
            modal,
            maximized: false,
            grid: &mut grid,
            selection: if scene.selected { &selection } else { &[] },
            hover_link: link_hover.as_ref().map_or(0, |(_, id)| *id),
            link_hand: false,
            link_tip: link_tip
                .as_ref()
                .filter(|_| scene.link.is_some_and(|l| l.move_at <= k))
                .map(|(rect, url)| LinkTip {
                    rect: *rect,
                    url,
                    key: 1,
                }),
            toasts: scene.toasts.then_some(&mut toasts),
            cursor,
            blink_on: true,
            scroll: scene.scroll,
            chrome: scene.chrome,
            ime: false,
            preedit: None,
            clipboard_write: None,
            clipboard_read: false,
            page: page_state.as_mut().map(|state| PageView {
                state,
                settings: &mut edited,
                restart: scene.page.is_some_and(|p| p.restart),
                focused: scene.focused,
                system_fonts: &DUMP_SYSTEM_FONTS,
            }),
            menu: model.as_ref().map(|m| MenuDraw {
                rows: m.rows(),
                place,
                name: strings.tr(m.name_key()),
            }),
            inbox: scene.inbox.map(|_| InboxDraw {
                button: InboxButton {
                    name: &inbox_name,
                    unread: inbox.unread() > 0,
                    // Explicit times: the ring began before the last frame, and only that frame
                    // is the picture (the earlier ones draw the plain yellow bell).
                    ring: scene.inbox.and_then(|i| i.ring_ago_ms).and_then(|ago| {
                        let last = *FRAME_TIMES_MS.last().unwrap_or(&0);
                        (*ms == last).then(|| Duration::from_millis(last - ago))
                    }),
                    open: inbox_open,
                },
                list: inbox_open.then(|| InboxList {
                    name: strings.tr("inbox.name"),
                    title: strings.tr("inbox.title"),
                    clear_all: strings.tr("inbox.clearAll"),
                    rows: &inbox_message_rows,
                }),
            }),
            busy: None,
            busy_tone: Tone::Neutral,
            grid_alpha: 1.0,
            fading: Vec::new(),
        };
        let drawn = r.draw(&input, content, &view, size)?;
        // The app opens the tab menu from the request the strip or a pane row reports, and a
        // card's menu from its header (the next frame shows it).
        let mut asked = None;
        if let Some(ctx) = drawn.tab_context {
            let groups = match scene.list {
                Some(_) => set_groups(&set, set.group_of(ctx.id)),
                None => menu_groups.clone(),
            };
            asked = Some((
                MenuKind::Tab {
                    closable: TAB_CLOSABLE,
                },
                groups,
                ctx.at,
            ));
        }
        for event in &drawn.pane_events {
            if let PaneEvent::GroupMenu { id, at } = *event {
                let collapsed = set.group(id).is_some_and(|g| g.collapsed);
                let kind = MenuKind::Group { collapsed };
                asked = Some((kind, set_groups(&set, Some(id)), at));
            }
        }
        if let Some((kind, groups, at)) = asked {
            model = Some(MenuModel::with_groups(
                kind,
                &groups,
                &strings,
                cfg!(target_os = "macos"),
            ));
            place = MenuPlace::At(at);
        }
        if let Some(out) = drawn.inbox {
            inbox_rect = Some(out.rect);
        }
        slots.sync(toasts.live(), input.time, |id| toasts.is_live(id));
        r.gpu.end_frame(&mut r.text);
    }
    let rgba = read_rgba(&r.gpu.device, &r.gpu.queue, &texture)?;
    let min_alpha = rgba.chunks_exact(4).map(|px| px[3]).min().unwrap_or(0);
    write_png(path, size, &rgba)?;
    Ok(min_alpha)
}

/// What a dump result means for the process: its exit code and the line to print to stderr.
/// A missing adapter is SKIPPED (exit 0) unless `require_gpu`, which makes it a failure.
fn exit_for(result: &Result<Vec<PathBuf>, DumpError>, require_gpu: bool) -> (u8, Option<String>) {
    match result {
        Ok(_) => (0, None),
        Err(DumpError::NoGpu(e)) if !require_gpu => {
            (0, Some(format!("SKIPPED: no wgpu adapter: {e}")))
        }
        Err(DumpError::NoGpu(e)) => (
            1,
            Some(format!(
                "LUNE_REQUIRE_GPU=1 but no usable wgpu adapter: {e}"
            )),
        ),
        Err(DumpError::Failed(e)) => (1, Some(format!("dump: {e}"))),
    }
}

/// Runs the dump for `LUNETERM_DUMP_FRAMES` and prints the written files.
pub fn run(dir: &Path) -> ExitCode {
    // Checked before anything is written; a value that is not UTF-8 is as wrong as any other.
    let var = match std::env::var(DUMP_TEXT_VAR) {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
    };
    let text = match parse_dump_text(var.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let result = dump_frames_text(dir, text);
    if let Ok(files) = &result {
        for f in files {
            println!("{}", f.display());
        }
    }
    let (code, line) = exit_for(&result, require_gpu());
    if let Some(line) = line {
        eprintln!("{line}");
    }
    ExitCode::from(code)
}

/// `LUNE_REQUIRE_GPU=1`: a missing adapter fails instead of skipping.
pub fn require_gpu() -> bool {
    std::env::var("LUNE_REQUIRE_GPU").is_ok_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luneterm-dump-{}-{tag}", std::process::id()));
        assert!(!dir.exists(), "{} already exists", dir.display());
        dir
    }

    #[test]
    fn a_missing_adapter_is_skipped_unless_required() {
        let none = || Err(DumpError::NoGpu("none".into()));
        let (code, line) = exit_for(&none(), false);
        assert_eq!(code, 0);
        assert!(line.is_some_and(|l| l.starts_with("SKIPPED")));
        let (code, line) = exit_for(&none(), true);
        assert_eq!(code, 1);
        assert!(line.is_some_and(|l| l.contains("LUNE_REQUIRE_GPU=1")));
        assert_eq!(exit_for(&Ok(Vec::new()), true), (0, None));
        let failed = Err(DumpError::Failed("disk".into()));
        assert_eq!(exit_for(&failed, false).0, 1);
    }

    #[test]
    fn the_dump_text_value_is_on_off_or_an_error_naming_the_variable() {
        assert_eq!(parse_dump_text(None), Ok(TextRendering::SMOOTH));
        assert_eq!(parse_dump_text(Some("on")), Ok(TextRendering::SMOOTH));
        assert_eq!(parse_dump_text(Some("off")), Ok(TextRendering::OFF));
        for bad in ["", "ON", "bogus", "1", "off "] {
            let e = parse_dump_text(Some(bad)).expect_err(bad);
            assert!(e.contains(DUMP_TEXT_VAR), "{e}");
            assert!(bad.is_empty() || !e.contains(bad), "{e}");
        }
    }

    /// The text setting changes the glyph pixels and nothing else: the same sizes, and the
    /// smoothed frame differs from the unsmoothed one.
    #[test]
    fn the_text_setting_changes_pixels_not_geometry() {
        let (on, off) = (scratch("text-on"), scratch("text-off"));
        let a = match dump_frames_text(&on, TextRendering::SMOOTH) {
            Ok(f) => f,
            Err(DumpError::NoGpu(e)) if !require_gpu() => {
                eprintln!("SKIPPED: no wgpu adapter: {e}");
                return;
            }
            Err(e) => panic!("dump: {e:?}"),
        };
        let b = dump_frames_text(&off, TextRendering::OFF).expect("off dump");
        assert_eq!(a.len(), b.len());
        let mut changed = 0;
        for (x, y) in a.iter().zip(&b) {
            let (bx, by) = (
                std::fs::read(x).expect("read"),
                std::fs::read(y).expect("read"),
            );
            let dims = |bytes: &[u8]| (bytes[16..24].to_vec(), bytes.len() > 24);
            assert_eq!(dims(&bx).0, dims(&by).0, "{} changed size", x.display());
            changed += usize::from(bx != by);
        }
        assert!(changed > 0, "no dump differs between on and off");
    }

    /// Every scene is written, is the right size, is not blank, and two runs give byte-identical
    /// files.
    #[test]
    fn two_dumps_are_byte_identical() {
        let (a, b) = (scratch("a"), scratch("b"));
        let gated = match dump_frames_gated(&a) {
            Ok(f) => f,
            Err(DumpError::NoGpu(e)) if !require_gpu() => {
                eprintln!("SKIPPED: no wgpu adapter: {e}");
                return;
            }
            Err(e) => panic!("dump: {e:?}"),
        };
        // An opaque scene has no transparent pixel (P22); the glass scenes are not held to it.
        for (path, alpha) in &gated {
            let opaque = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("opaque"));
            if opaque {
                assert_eq!(*alpha, 255, "{} has a transparent pixel", path.display());
            }
        }
        assert_eq!(
            gated
                .iter()
                .filter(|(p, _)| p.to_string_lossy().contains("opaque"))
                .count(),
            11
        );
        let first: Vec<PathBuf> = gated.into_iter().map(|(p, _)| p).collect();
        let second = dump_frames(&b).expect("second dump");
        assert_eq!(first.len(), SCENES.len());
        for (x, y) in first.iter().zip(&second) {
            assert_eq!(x.file_name(), y.file_name());
            let (bx, by) = (
                std::fs::read(x).expect("read"),
                std::fs::read(y).expect("read"),
            );
            assert!(!bx.is_empty());
            assert_eq!(bx, by, "{} differs between runs", x.display());
        }
        // The 2x frame is a different picture from the 1x one, and the thumb changes the frame.
        let bytes = |p: &PathBuf| std::fs::read(p).expect("read");
        assert_ne!(bytes(&first[0]), bytes(&first[1]));
        assert_ne!(bytes(&first[0]), bytes(&first[2]));
        // The step 6 scenes differ from the shell they are drawn over: a palette switch, a page,
        // a menu and the opaque look each change the picture.
        let named = |n: &str| {
            first
                .iter()
                .find(|p| p.file_name().and_then(|f| f.to_str()) == Some(n))
                .map(bytes)
                .unwrap_or_else(|| panic!("{n} was not dumped"))
        };
        let tabs = named("tabs@2x.png");
        for n in [
            "palette@2x.png",
            "opaque@2x.png",
            "lunegit@2x.png",
            "lunegit-menu@2x.png",
            "lunegit-opaque@2x.png",
            "menu-app@2x.png",
            "menu-tab@2x.png",
            "settings-appearance@2x.png",
        ] {
            assert_ne!(named(n), tabs, "{n} looks like the plain tabs scene");
        }
        assert_ne!(
            named("menu-terminal@2x.png"),
            named("menu-terminal-empty@2x.png")
        );
        assert_ne!(
            named("settings-terminal@2x.png"),
            named("settings-terminal-end@2x.png")
        );
        assert_ne!(
            named("settings-terminal@2x.png"),
            named("settings-restart@2x.png")
        );
        assert_ne!(
            named("settings-appearance@2x.png"),
            named("settings-combo@2x.png")
        );
        assert_ne!(
            named("settings-terminal@2x.png"),
            named("settings-font-combo@2x.png")
        );
        // The group scenes: a menu with groups differs from the plain tab menu, the editor's
        // three states from each other, and the close-group question from the close-session one.
        for pre in ["", "lunegit-"] {
            let of = |n: &str| named(&format!("{pre}{n}@2x.png"));
            assert_ne!(of("menu-tab-ungrouped"), named("menu-tab@2x.png"), "{pre}");
            assert_ne!(of("menu-tab-ungrouped"), of("menu-tab-grouped"), "{pre}");
            assert_ne!(of("group-new"), of("group-edit"), "{pre}");
            assert_ne!(of("group-new"), of("group-invalid"), "{pre}");
            assert_ne!(of("group-close"), named("confirm-one@2x.png"), "{pre}");
        }
        // The pane view differs from the bar view, and its states from each other.
        assert_ne!(named("pane@2x.png"), tabs);
        assert_ne!(named("pane@2x.png"), named("pane-inbox@2x.png"));
        assert_ne!(named("pane@2x.png"), named("pane-windows@2x.png"));
        assert_ne!(named("pane@2x.png"), named("pane-menu-app@2x.png"));
        assert_ne!(named("pane-inbox@2x.png"), named("pane-inbox-open@2x.png"));
        assert_ne!(named("pane-inbox@2x.png"), named("pane-settings@2x.png"));
        assert_ne!(named("tabs-attention@2x.png"), named("bar-windows@2x.png"));
        // The tooltip is not there at the first dwell and is there after the delay.
        assert_ne!(named("link-hover@2x.png"), named("link-tip@2x.png"));
        assert_ne!(named("link-hover@2x.png"), tabs);
        std::fs::remove_dir_all(&a).expect("remove a");
        std::fs::remove_dir_all(&b).expect("remove b");
    }
}
