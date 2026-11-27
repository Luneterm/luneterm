//! The side pane (plan D20-D22, D26-D35): which of the two views a window shows, the pane's
//! rects, the bar view's show button, the pane's own controls (the hide button, the divider of
//! the footer and the width drag at its right edge) and the tab list: loose tabs, group cards and
//! the New Tab button, scrolling between the header and the footer, with its drag reorder (D24):
//! a tab's row, or a whole card by its header, dropped in a gap of the list. The window's drag
//! regions, the footer's bell and `⋯` buttons and the terminal card are the shell's
//! ([`crate::shell`]). A tab's progress report shines over its title; the running-command outline
//! is the top bar's only.

use std::f32::consts::{FRAC_PI_2, PI};
use std::time::Duration;

use lune_ui::access::{Action, Node, Role, State};
use lune_ui::color::{Rgba, premul, with_alpha};
use lune_ui::drag::{AUTO_SCROLL_TICK, DIM_ALPHA, DROP_LINE_THICKNESS, edge_step};
use lune_ui::icon;
use lune_ui::input::{CursorIcon, NamedKey};
use lune_ui::motion::MotionTier;
use lune_ui::paint::{Painter, Rect, Xform};
use lune_ui::scene::LayerId;
use lune_ui::text::{Spec, Style};
use lune_ui::theme::Theme;
use lune_ui::ui::{Response, Sense, Ui, WidgetId};
use lune_ui::widgets::button::{
    ButtonKind, ButtonLook, ButtonState, IconButton, IconButtonExtras, paint_icon,
};
use lune_ui::widgets::scroll_area::{Bars, Extent, Reveal, ScrollOptions, Target};
use lune_ui::widgets::shine;
use lune_ui::widgets::tone::Tone;

use crate::groups::{CARD_RING, CARD_TINT, GroupColor, GroupId};
use crate::menus::ICON_TERMINAL;
use crate::settings::{MAX_PANE_WIDTH, MIN_PANE_WIDTH, Theme as LookTheme};
use crate::shell::{CARD_MARGIN, Chrome, OVERFLOW_SIZE, StripTab, TITLE_BAND, TabContext, View};
use crate::tabs::{TabId, TabSet};

/// The least width the terminal card keeps beside the pane (D29): below it the top bar shows.
pub const CARD_MIN_WIDTH: f32 = 480.0;
/// The pane's content inset, left and right.
pub const PAD: f32 = 8.0;
/// How far inside the content the footer's divider and buttons start.
pub const FOOTER_INSET: f32 = 4.0;
/// The gap between the footer's divider and its buttons.
pub const FOOTER_GAP: f32 = 8.0;
/// The width of the zone at the pane's right edge that drags its width.
pub const EDGE_ZONE: f32 = 6.0;
/// The codicon `layout-sidebar-left`: the hide button, the show button and their menu row.
pub const TOGGLE_GLYPH: &str = "\u{ebf3}";
/// Where the native traffic lights end (the zoom light's right edge, logical px, measured on a
/// window with a full-size content view) and the room kept after them before the show button.
const LIGHTS_END: f32 = 69.0;
const LIGHTS_GAP: f32 = 7.0;

/// Whether a window `window_w` wide shows a pane `width` wide: the card keeps [`CARD_MIN_WIDTH`]
/// (and its right margin) beside it (D29).
pub fn fits(window_w: f32, width: f32) -> bool {
    window_w >= width + CARD_MIN_WIDTH + CARD_MARGIN
}

/// The view of a frame: the pane when it is the saved choice and the window is wide enough for it
/// at its saved width, else the top bar.
pub fn resolve(wanted: bool, width: u32, window_w: f32, chrome: Chrome) -> View {
    if wanted && fits(window_w, width as f32) {
        View::Pane {
            width: width as f32,
            drawn_caption: chrome.drawn_caption,
        }
    } else {
        View::Bar
    }
}

/// Whether the top bar offers its show button: only while the window is wide enough for the pane
/// the button would show (D29).
pub fn offers_show_button(view: View, window_w: f32, width: u32) -> bool {
    view == View::Bar && fits(window_w, width as f32)
}

/// The widest pane a drag may make in a window `window_w` wide: the file's range, and no more than
/// the card leaves, so a drag never hides the pane it is dragging.
pub fn max_width(window_w: f32) -> u32 {
    let room = (window_w - CARD_MIN_WIDTH - CARD_MARGIN).floor();
    // Inside the range, so the cast is exact (a NaN clamps to NaN and casts to the least).
    (room.clamp(MIN_PANE_WIDTH as f32, MAX_PANE_WIDTH as f32) as u32).max(MIN_PANE_WIDTH)
}

/// The rects of the pane of a window, logical px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneRects {
    /// The whole pane: the window's left column.
    pub rect: Rect,
    /// The title area at its top: the traffic lights' room and the hide button. A press on its
    /// empty part drags the window.
    pub header: Rect,
    /// The tab list's area, between the header and the footer.
    pub list: Rect,
    /// The footer's 1 px divider.
    pub divider: Rect,
    /// The footer, from the divider to the window's bottom.
    pub footer: Rect,
    /// The bell's place at the footer's left.
    pub bell: Rect,
    /// The `⋯` button's place at the footer's right.
    pub menu: Rect,
    /// The zone at the right edge that drags the width.
    pub edge: Rect,
}

impl PaneRects {
    /// The rects of a pane `width` wide at the left of `window`.
    pub fn new(window: Rect, width: f32) -> PaneRects {
        let rect = Rect::new(window.x, window.y, width.min(window.w).max(0.0), window.h);
        let header = Rect::new(rect.x, rect.y, rect.w, TITLE_BAND.min(rect.h));
        // The buttons stand on the card's bottom line.
        let button_y = rect.bottom() - CARD_MARGIN - OVERFLOW_SIZE;
        let divider_y = button_y - FOOTER_GAP - 1.0;
        let inset = PAD + FOOTER_INSET;
        PaneRects {
            rect,
            header,
            list: Rect::new(
                rect.x + PAD,
                header.bottom(),
                (rect.w - 2.0 * PAD).max(0.0),
                (divider_y - header.bottom()).max(0.0),
            ),
            divider: Rect::new(
                rect.x + inset,
                divider_y,
                (rect.w - 2.0 * inset).max(0.0),
                1.0,
            ),
            footer: Rect::new(
                rect.x,
                divider_y,
                rect.w,
                (rect.bottom() - divider_y).max(0.0),
            ),
            bell: Rect::new(rect.x + inset, button_y, OVERFLOW_SIZE, OVERFLOW_SIZE),
            menu: Rect::new(
                rect.right() - inset - OVERFLOW_SIZE,
                button_y,
                OVERFLOW_SIZE,
                OVERFLOW_SIZE,
            ),
            edge: Rect::new(rect.right() - EDGE_ZONE, rect.y, EDGE_ZONE, rect.h),
        }
    }
}

/// The y of a button in the title area: on the line of the native traffic lights where there are
/// some, else centred like the drawn caption buttons.
fn title_button_y(chrome: Chrome) -> f32 {
    if chrome.native_lights { 4.0 } else { 5.0 }
}

/// The hide button, at the right end of the pane's header.
pub fn hide_button(rects: &PaneRects, chrome: Chrome) -> Rect {
    Rect::new(
        rects.rect.right() - PAD - OVERFLOW_SIZE,
        rects.rect.y + title_button_y(chrome),
        OVERFLOW_SIZE,
        OVERFLOW_SIZE,
    )
}

/// The show button of the top bar: right of the native traffic lights where there are some, else
/// at the band's left, on the card's left line.
pub fn show_button(window: Rect, chrome: Chrome) -> Rect {
    let x = if chrome.native_lights {
        LIGHTS_END + LIGHTS_GAP
    } else {
        CARD_MARGIN
    };
    Rect::new(
        window.x + x,
        window.y + title_button_y(chrome),
        OVERFLOW_SIZE,
        OVERFLOW_SIZE,
    )
}

/// What the pane's edge zone saw in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PaneEdge {
    /// A drag of it began this frame.
    pub started: bool,
    /// How far the pointer moved along x since the previous frame while dragging (the first frame:
    /// since the press).
    pub dx: f32,
    /// The press on it is still held.
    pub held: bool,
}

/// A drag of the pane's right edge: the width it began at and the pointer's travel since. The
/// width follows the pointer however far past a limit it went.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidthDrag {
    start: u32,
    travel: f32,
}

impl WidthDrag {
    /// A drag that began with the pane `width` wide.
    pub fn start(width: u32) -> WidthDrag {
        WidthDrag {
            start: width,
            travel: 0.0,
        }
    }

    /// The width after the pointer moved `dx` further, in a window `window_w` wide.
    fn moved(&mut self, dx: f32, window_w: f32) -> u32 {
        if dx.is_finite() {
            self.travel += dx;
        }
        let wanted = (self.start as f32 + self.travel).round();
        // Clamped into the range first, so the cast is exact.
        wanted.clamp(MIN_PANE_WIDTH as f32, max_width(window_w) as f32) as u32
    }
}

/// What the app does after a frame of the edge zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DragStep {
    /// The pane is this wide now (it changed this frame).
    pub width: Option<u32>,
    /// The drag ended with another width than it began with: save it, this once.
    pub save: bool,
}

/// One frame of a width drag: `drag` is the one under way (`None`: none), `edge` what the zone saw
/// (`None`: there was no zone, so a drag under way ends), `width` the pane's width now.
pub fn drag_step(
    drag: &mut Option<WidthDrag>,
    edge: Option<PaneEdge>,
    width: u32,
    window_w: f32,
) -> DragStep {
    if edge.is_some_and(|e| e.started) {
        *drag = Some(WidthDrag::start(width));
    }
    let Some(under_way) = drag else {
        return DragStep::default();
    };
    let mut now = width;
    if let Some(e) = edge.filter(|e| e.started || e.dx != 0.0) {
        now = under_way.moved(e.dx, window_w);
    }
    let start = under_way.start;
    let ended = !edge.is_some_and(|e| e.held);
    if ended {
        *drag = None;
    }
    DragStep {
        width: (now != width).then_some(now),
        save: ended && now != start,
    }
}

/// The pane's own paint under its controls: the footer's divider. The pane has no layer of its own
/// (D33): the window's panel is behind it.
pub fn paint<H: Copy + PartialEq>(p: &mut Painter<'_, H>, rects: &PaneRects, theme: &Theme) {
    p.fill(rects.divider, 0.0, theme.border);
}

/// What the pane's controls asked in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PaneOut {
    /// The hide button was pressed.
    pub toggle: bool,
    /// What the edge zone saw; `None` while the press belongs to the window's own resize edge.
    pub edge: Option<PaneEdge>,
}

/// The pane's controls: the edge zone and the hide button. They are registered after the window's
/// drag regions, so a press on them never drags the window. `window_edge` is whether the pointer is
/// at an edge the app resizes the window by, and `edge_pressed` whether this frame's press landed
/// on one: such a press is the window's and starts no width drag, while a drag already under way
/// goes on past those edges.
pub fn controls(
    ui: &mut Ui<'_>,
    rects: &PaneRects,
    chrome: Chrome,
    name: &str,
    window_edge: bool,
    edge_pressed: bool,
) -> PaneOut {
    let id = ui.id("pane-edge");
    let zone = ui.interact(id, rects.edge, Sense::DRAG);
    let edge = (!edge_pressed).then(|| {
        if (zone.hovered && !window_edge) || zone.dragged {
            ui.set_cursor(CursorIcon::ColResize);
        }
        let moving = zone.drag_started || zone.dragged || zone.drag_stopped;
        PaneEdge {
            started: zone.drag_started && !window_edge,
            dx: if moving { zone.drag_delta.0 } else { 0.0 },
            held: zone.pressed,
        }
    });
    let radius = ui.theme().item_radius;
    let hide = IconButton {
        glyph: TOGGLE_GLYPH,
        glyph_size: 16.0,
        color: None,
        radius,
        name,
    };
    // The pressed look while the pane is shown (D20).
    let open = IconButtonExtras::new().with_open(true);
    let toggle = ui
        .icon_button_with("pane-hide", hide_button(rects, chrome), &hide, &open)
        .clicked;
    PaneOut { toggle, edge }
}

// ------------------------------------------------------------------------------- the tab list

/// The height of a tab's row (D26).
pub const ROW_HEIGHT: f32 = 32.0;
/// The height of a card's header (D26).
pub const HEADER_HEIGHT: f32 = 30.0;
/// The height of the dashed New Tab button.
pub const NEW_TAB_HEIGHT: f32 = 34.0;
/// A card's padding, and the gap between its header and rows.
pub const CARD_PAD: f32 = 3.0;
pub const ROW_GAP: f32 = 1.0;
/// Between two blocks of the list: a loose tab, a card, the New Tab button.
pub const BLOCK_GAP: f32 = 6.0;
/// The corners of a card and of the New Tab button, and of a row.
const CARD_ROUND: f32 = 10.0;
const ROW_ROUND: f32 = 7.0;
/// A loose tab's row stands this far inside the list on both sides (mock F's block padding).
const LOOSE_INSET: f32 = 4.0;
/// A header's side padding, its chevron and the gap between them and the name.
const HEADER_PAD: f32 = 8.0;
const CHEVRON: f32 = 12.0;
const HEADER_GAP: f32 = 8.0;
/// Where a row's glyph starts, from the row's left: in a loose row, and in a member row, whose
/// glyph lines up with its header's name (D26).
pub const LOOSE_INDENT: f32 = 8.0;
pub const MEMBER_INDENT: f32 = HEADER_PAD + CHEVRON + HEADER_GAP;
/// A row's glyph, and the gap between it and the title.
const GLYPH: f32 = 16.0;
const GLYPH_GAP: f32 = 10.0;
/// A row's close button, how far its right edge stays inside the row's, and its glyph.
const CLOSE_SIZE: f32 = 20.0;
const CLOSE_INSET: f32 = 5.0;
const CLOSE_GLYPH: f32 = 14.0;
const CLOSE_ROUND: f32 = 4.0;
/// A card's `+` button, how far it stays inside the header's right edge, its glyph and corners.
const PLUS_SIZE: f32 = 24.0;
const PLUS_INSET: f32 = 2.0;
const PLUS_GLYPH: f32 = 14.0;
const PLUS_ROUND: f32 = 6.0;
/// The unread dot (the strip's size).
const DOT: f32 = 7.0;
/// Font sizes: a tab's title, a group's name, its count.
const TITLE_FONT: f32 = 13.0;
const NAME_FONT: f32 = 12.0;
const COUNT_FONT: f32 = 11.0;
/// The share of a row's text colour its glyph is drawn at.
const GLYPH_ALPHA: f32 = 0.75;
/// The dashes of the New Tab button's outline, and their gaps.
const DASH: (f32, f32) = (4.0, 3.0);
/// The scroll area's key.
const LIST_KEY: &str = "pane-list";
/// The longest step an auto-scroll takes at once: a late frame does not jump the list.
const MAX_SCROLL_STEP: Duration = Duration::from_millis(50);
/// The shadow under the lifted copy of a drag: how far down it falls, its blur and its colour.
const LIFT_DROP: f32 = 2.0;
const LIFT_BLUR: f32 = 6.0;
const LIFT_SHADOW: Rgba = [0, 0, 0, 110];
/// The ring around the collapsed card a drag would drop into.
const INTO_RING: f32 = 2.0;
/// The room the lifted copy leaves between itself and the mark of the slot.
const MARK_CLEAR: f32 = 2.0;

/// A tab's row of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabRow<'a> {
    pub id: TabId,
    pub title: &'a str,
    /// An OSC 9;4 report is active in this tab: its title shines in the tone.
    pub progress: Option<Tone>,
    /// The tab has an unread notice and is not the active one.
    pub unread: bool,
    pub active: bool,
    /// The colour of the tab's group; `None` for a loose tab.
    pub group: Option<GroupColor>,
}

/// The header of a group's card.
#[derive(Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub id: GroupId,
    pub name: String,
    pub color: GroupColor,
    pub collapsed: bool,
    /// How many tabs the group has.
    pub count: usize,
    /// A collapsed card with an unread member: one dot (an expanded card's members show theirs).
    pub unread: bool,
    /// A collapsed card that holds the active tab: its header has the selected look.
    pub holds_active: bool,
}

impl std::fmt::Debug for GroupRow {
    /// The name's length only: a program can choose it, and program text is kept out of a
    /// `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupRow")
            .field("id", &self.id)
            .field("name_chars", &self.name.chars().count())
            .field("color", &self.color)
            .field("collapsed", &self.collapsed)
            .field("count", &self.count)
            .field("unread", &self.unread)
            .field("holds_active", &self.holds_active)
            .finish()
    }
}

/// One row of the list. A header is followed by its member rows unless the card is collapsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneRow<'a> {
    Tab(TabRow<'a>),
    Group(GroupRow),
}

/// The rows of the list: the tabs of `strip`, which are those of `tabs` in its order, with a
/// header before each group's run and the members of a collapsed group left out.
pub fn rows<'a, T>(tabs: &TabSet<T>, strip: &[StripTab<'a>]) -> Vec<PaneRow<'a>> {
    let active = tabs.active();
    let mut rows = Vec::with_capacity(strip.len());
    // The runs: the tabs of one group together, each loose tab alone.
    for run in strip.chunk_by(|a, b| {
        let group = tabs.group_of(a.id);
        group.is_some() && group == tabs.group_of(b.id)
    }) {
        let group = run
            .first()
            .and_then(|t| tabs.group_of(t.id))
            .and_then(|id| Some((id, tabs.group(id)?)));
        if let Some((id, group)) = group {
            let collapsed = group.collapsed;
            rows.push(PaneRow::Group(GroupRow {
                id,
                name: group.name.clone(),
                color: group.color,
                collapsed,
                count: run.len(),
                unread: collapsed && run.iter().any(|t| t.attention),
                holds_active: collapsed && run.iter().any(|t| Some(t.id) == active),
            }));
            if collapsed {
                continue;
            }
        }
        let color = group.map(|(_, g)| g.color);
        rows.extend(run.iter().map(|t| {
            PaneRow::Tab(TabRow {
                id: t.id,
                title: t.title,
                progress: t.progress,
                unread: t.attention,
                active: Some(t.id) == active,
                group: color,
            })
        }));
    }
    rows
}

/// The row that shows the active tab: its own, or the header of the collapsed card holding it.
pub fn active_row(rows: &[PaneRow<'_>]) -> Option<usize> {
    rows.iter().position(|row| match row {
        PaneRow::Tab(t) => t.active,
        PaneRow::Group(g) => g.holds_active,
    })
}

/// Where the list's parts stand when it is not scrolled.
#[derive(Debug, Clone, PartialEq)]
pub struct ListLayout {
    /// The rect of each row, in the order of the rows.
    pub rows: Vec<Rect>,
    /// Each card, with its group's colour.
    pub cards: Vec<(GroupColor, Rect)>,
    /// The New Tab button, after the last block.
    pub new_tab: Rect,
    /// The height of it all.
    pub height: f32,
}

/// The layout of `rows` from the top of `area` (D26): blocks [`BLOCK_GAP`] apart, a loose tab or
/// a card each, then the New Tab button.
pub fn layout_list(rows: &[PaneRow<'_>], area: Rect) -> ListLayout {
    let mut rects = Vec::with_capacity(rows.len());
    let mut cards = Vec::new();
    let mut y = area.y;
    let mut rest = rows.iter().peekable();
    while let Some(row) = rest.next() {
        match row {
            PaneRow::Tab(_) => {
                let w = (area.w - 2.0 * LOOSE_INSET).max(0.0);
                rects.push(Rect::new(area.x + LOOSE_INSET, y, w, ROW_HEIGHT));
                y += ROW_HEIGHT;
            }
            PaneRow::Group(g) => {
                let top = y;
                let x = area.x + CARD_PAD;
                let w = (area.w - 2.0 * CARD_PAD).max(0.0);
                y += CARD_PAD;
                rects.push(Rect::new(x, y, w, HEADER_HEIGHT));
                y += HEADER_HEIGHT;
                while rest
                    .next_if(|r| matches!(r, PaneRow::Tab(t) if t.group.is_some()))
                    .is_some()
                {
                    y += ROW_GAP;
                    rects.push(Rect::new(x, y, w, ROW_HEIGHT));
                    y += ROW_HEIGHT;
                }
                y += CARD_PAD;
                cards.push((g.color, Rect::new(area.x, top, area.w, y - top)));
            }
        }
        y += BLOCK_GAP;
    }
    let new_tab = Rect::new(area.x, y, area.w, NEW_TAB_HEIGHT);
    ListLayout {
        rows: rects,
        cards,
        new_tab,
        height: new_tab.bottom() - area.y,
    }
}

/// What the list draws a frame from.
#[derive(Debug, Clone, Copy)]
pub struct PaneList<'a> {
    pub rows: &'a [PaneRow<'a>],
    /// The look the group colours are drawn for.
    pub look: LookTheme,
    /// The active tab changed, or the user was sent to it: scroll its row into view (D32).
    pub reveal: bool,
}

/// What the user asked of the list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaneEvent {
    /// A tab's row was pressed.
    Activate(TabId),
    /// A row's close button.
    Close(TabId),
    /// A card's header was pressed: collapse it, or expand it.
    ToggleGroup(GroupId),
    /// A card's `+`.
    NewTabIn(GroupId),
    /// The New Tab button: a tab in no group, at the end.
    NewTab,
    /// A card's header menu, to open at `at`.
    GroupMenu { id: GroupId, at: (f32, f32) },
    /// A drag is under way (reported by every frame it lasts): Escape is the list's.
    Dragging,
    /// A drag was released on a slot that is not the item's own place: move it there
    /// ([`commit`]). `index` and `group` are the slot's ([`Slot`]).
    Drop {
        item: DragItem,
        index: usize,
        group: Option<GroupId>,
    },
}

/// What the list reported in a frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListOut {
    pub events: Vec<PaneEvent>,
    /// A row asked for the tab menu.
    pub tab_context: Option<TabContext>,
}

// ------------------------------------------------------------------------------- the drag

/// What a drag of the list moves (D24): a tab by its row, or a whole group by its card's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragItem {
    Tab(TabId),
    Group(GroupId),
}

/// Where a drag would drop its item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    /// The index in the tab order without the dragged tab, or without the dragged group's tabs.
    pub index: usize,
    /// The group a dropped tab is in there; `None` for no group, and for a dragged group.
    pub group: Option<GroupId>,
    pub mark: Mark,
}

/// How a slot shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    /// It is where the item already is: nothing is drawn, and a release changes nothing.
    Stay,
    /// The insertion line, in this rect of the list (not scrolled, as [`layout_list`]'s).
    Line(Rect),
    /// Into the collapsed card whose header is this row.
    Into(usize),
}

/// A block of the list: a loose tab's row, or a card.
#[derive(Debug, Clone, Copy)]
struct Block {
    /// The block's place: the loose row, or the card.
    rect: Rect,
    /// The index of its first row (a card's header), and how many member rows follow a header.
    row: usize,
    members: usize,
    /// The index of its first tab in the tab order, and how many tabs it holds.
    start: usize,
    tabs: usize,
    /// A card's group and whether it is collapsed; `None` for a loose tab.
    group: Option<(GroupId, bool)>,
}

/// The blocks of `rows` where `layout` (the layout of those rows) puts them.
fn blocks(rows: &[PaneRow<'_>], layout: &ListLayout) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut cards = layout.cards.iter();
    let (mut at, mut start) = (0, 0);
    while let (Some(row), Some(&rect)) = (rows.get(at), layout.rows.get(at)) {
        let block = match row {
            PaneRow::Tab(_) => Block {
                rect,
                row: at,
                members: 0,
                start,
                tabs: 1,
                group: None,
            },
            PaneRow::Group(g) => Block {
                rect: cards.next().map_or(rect, |(_, card)| *card),
                row: at,
                members: rows[at + 1..]
                    .iter()
                    .take_while(|r| matches!(r, PaneRow::Tab(t) if t.group.is_some()))
                    .count(),
                start,
                tabs: g.count,
                group: Some((g.id, g.collapsed)),
            },
        };
        at += 1 + block.members;
        start += block.tabs;
        blocks.push(block);
    }
    blocks
}

/// The dragged item as the list shows it.
#[derive(Debug, Clone, Copy)]
struct Held {
    /// Its first row, and the block that row is in.
    row: usize,
    block: usize,
    /// Its place in the tab order and how many tabs it moves.
    from: usize,
    tabs: usize,
    /// The group of a dragged tab.
    group: Option<GroupId>,
    /// Its rect: a tab's row, or a card.
    rect: Rect,
}

impl Held {
    /// `None` when the list does not show the item (its tab is gone, or hidden in a collapsed
    /// card).
    fn find(
        blocks: &[Block],
        rows: &[PaneRow<'_>],
        layout: &ListLayout,
        item: DragItem,
    ) -> Option<Held> {
        blocks.iter().enumerate().find_map(|(i, b)| match item {
            DragItem::Group(g) => b.group.is_some_and(|(id, _)| id == g).then_some(Held {
                row: b.row,
                block: i,
                from: b.start,
                tabs: b.tabs,
                group: None,
                rect: b.rect,
            }),
            DragItem::Tab(id) => {
                let row = (b.row..=b.row + b.members)
                    .find(|r| matches!(rows.get(*r), Some(PaneRow::Tab(t)) if t.id == id))?;
                // A member's rows follow its card's header.
                let within = (row - b.row).saturating_sub(usize::from(b.group.is_some()));
                Some(Held {
                    row,
                    block: i,
                    from: b.start + within,
                    tabs: 1,
                    group: b.group.map(|(g, _)| g),
                    rect: *layout.rows.get(row)?,
                })
            }
        })
    }
}

/// What of the list the pointer is at.
#[derive(Debug, Clone, Copy)]
enum Gap {
    /// The gap before a block.
    Before(usize),
    /// Inside an expanded card, after this many of its member rows.
    Inside(usize, usize),
    /// A collapsed card's header.
    Into(usize),
    /// A dragged card's own place.
    Own,
    /// No drop here: a card dragged into another card.
    Refused,
}

/// The slot of a drag of `item` with the pointer at height `y` of the list (not scrolled, as
/// `layout`, which is [`layout_list`] of `rows` in `area`): the gaps between the rows (D24).
/// `None` where nothing may drop, or when the list does not show the item.
///
/// A tab drops before a loose row from its upper half; before a card from the upper half of an
/// expanded card's header; between the member rows of an expanded card by their middles, down to
/// the card's bottom; and into a collapsed card from anywhere on its header. A card drops only
/// between blocks: another expanded card takes it before itself from its header, and refuses it
/// over its rows.
pub fn slot_at(
    rows: &[PaneRow<'_>],
    layout: &ListLayout,
    area: Rect,
    item: DragItem,
    y: f32,
) -> Option<Slot> {
    let blocks = blocks(rows, layout);
    let held = Held::find(&blocks, rows, layout, item)?;
    let middle = |r: Rect| r.y + r.h / 2.0;
    let gap = blocks.iter().enumerate().find_map(|(i, b)| {
        let header = layout.rows[b.row];
        let before = |limit: f32| (y < limit).then_some(Gap::Before(i));
        match (item, b.group) {
            (_, None) => before(middle(b.rect)),
            (DragItem::Group(_), Some(_)) if i == held.block => {
                (y < b.rect.bottom()).then_some(Gap::Own)
            }
            (DragItem::Group(_), Some((_, true))) => before(middle(b.rect)),
            (DragItem::Group(_), Some((_, false))) => {
                before(header.bottom()).or((y < b.rect.bottom()).then_some(Gap::Refused))
            }
            (DragItem::Tab(_), Some((_, true))) => {
                before(header.y).or((y < header.bottom()).then_some(Gap::Into(i)))
            }
            (DragItem::Tab(_), Some((_, false))) => before(middle(header)).or_else(|| {
                let above = layout.rows[b.row + 1..=b.row + b.members]
                    .iter()
                    .filter(|r| middle(**r) <= y)
                    .count();
                (y < b.rect.bottom()).then_some(Gap::Inside(i, above))
            }),
        }
    });
    let between = |y: f32| {
        let top = y - DROP_LINE_THICKNESS / 2.0;
        Mark::Line(Rect::new(area.x, top, area.w, DROP_LINE_THICKNESS))
    };
    // The place in the order as it is, the group there and the mark.
    let (at, group, mark) = match gap {
        Some(Gap::Own) => (held.from, None, Mark::Stay),
        Some(Gap::Refused) => return None,
        Some(Gap::Before(i)) => {
            let b = blocks.get(i)?;
            // The first gap is at the list's top edge, not above it.
            let y = (b.rect.y - BLOCK_GAP / 2.0).max(area.y + DROP_LINE_THICKNESS / 2.0);
            (b.start, None, between(y))
        }
        None => {
            let last = blocks.last()?;
            let y = last.rect.bottom() + BLOCK_GAP / 2.0;
            (last.start + last.tabs, None, between(y))
        }
        Some(Gap::Inside(i, above)) => {
            let b = blocks.get(i)?;
            // Under the header, or under the member row above.
            let y = layout.rows.get(b.row + above)?.bottom() + ROW_GAP / 2.0;
            let line = Rect::new(
                b.rect.x + CARD_PAD,
                y - DROP_LINE_THICKNESS / 2.0,
                (b.rect.w - 2.0 * CARD_PAD).max(0.0),
                DROP_LINE_THICKNESS,
            );
            (b.start + above, b.group.map(|(g, _)| g), Mark::Line(line))
        }
        Some(Gap::Into(i)) => {
            let b = blocks.get(i)?;
            let group = b.group.map(|(g, _)| g);
            (b.start + b.tabs, group, Mark::Into(b.row))
        }
    };
    // Counted without what is dragged.
    let index = if at >= held.from + held.tabs {
        at - held.tabs
    } else {
        at
    };
    let stays = index == held.from && group == held.group;
    Some(Slot {
        index,
        group,
        mark: if stays { Mark::Stay } else { mark },
    })
}

/// Makes the move a drag released on a slot asks for. `false`, and nothing changes, when the
/// tabs no longer allow it. The active tab stays the same tab.
pub fn commit<T>(
    tabs: &mut TabSet<T>,
    item: DragItem,
    index: usize,
    group: Option<GroupId>,
) -> bool {
    match item {
        DragItem::Tab(id) => tabs.move_tab(id, index, group),
        DragItem::Group(g) => tabs.move_group(g, index),
    }
}

/// A drag of the list under way.
#[derive(Debug, Clone, Copy)]
struct LiveDrag {
    item: DragItem,
    /// The region that holds the pointer.
    id: WidgetId,
    /// The pointer's height below the item's top at the press: where the lifted copy hangs.
    grab: f32,
    /// The pointer's height when it was last in the window.
    y: f32,
    /// How far the last frame drew the list scrolled.
    offset: f64,
    /// While it scrolls by itself: where to, and when it last did.
    scroll: Option<(f64, Duration)>,
    /// What this frame saw of the region: the drag goes on, or it was released.
    held: bool,
    released: bool,
}

/// What the list keeps of a drag between frames, in the UI's store: it goes with a list that is
/// not drawn.
#[derive(Debug, Default)]
struct DragMemory {
    /// The last press on a row or header: the pointer's height below that item's top.
    grab: Option<(DragItem, f32)>,
    live: Option<LiveDrag>,
}

impl DragMemory {
    /// What the region of `item`, whose top is at `top` of the window, saw this frame: a press
    /// arms it, and a drag of it starts, goes on or ends.
    fn track(
        &mut self,
        item: DragItem,
        region: &Response,
        top: f32,
        pointer: Option<(f32, f32)>,
        offset: f32,
    ) {
        if region.press_started
            && let Some((_, y)) = region.press_pos
        {
            self.grab = Some((item, y - top));
        }
        if region.drag_started {
            let y = pointer.map_or(top, |(_, y)| y);
            let grab = self.grab.filter(|(i, _)| *i == item);
            self.live = Some(LiveDrag {
                item,
                id: region.id,
                grab: grab.map_or(y - top, |(_, g)| g),
                y,
                offset: f64::from(offset),
                scroll: None,
                held: false,
                released: false,
            });
        }
        if let Some(live) = self.live.as_mut().filter(|l| l.item == item) {
            live.held |= region.dragged || region.drag_started;
            live.released |= region.drag_stopped;
            live.y = pointer.map_or(live.y, |(_, y)| y);
        }
    }

    fn drags(&self, item: DragItem) -> bool {
        self.live.is_some_and(|l| l.item == item)
    }
}

/// Scrolls the list on while the pointer of a drag is near its top or bottom edge, or past it
/// within the pane (D24): the toolkit's ramp, per frame time.
fn auto_scroll(ui: &mut Ui<'_>, live: &mut LiveDrag, rects: &PaneRects, view: Rect, height: f32) {
    let max = f64::from((height - view.h).max(0.0));
    let from = live.scroll.map_or(live.offset, |(to, _)| to);
    let step = ui
        .pointer_pos()
        .filter(|p| rects.rect.contains(*p))
        .map_or(0.0, |(_, y)| {
            edge_step(
                f64::from(y),
                f64::from(view.y),
                f64::from(view.bottom()),
                from > 0.0,
                from < max - 0.5,
            )
        });
    if step == 0.0 {
        live.scroll = None;
        return;
    }
    let now = ui.time();
    let since = live
        .scroll
        .map_or(Duration::ZERO, |(_, at)| now.saturating_sub(at));
    let ticks = since.min(MAX_SCROLL_STEP).as_secs_f64() / AUTO_SCROLL_TICK.as_secs_f64();
    let to = (from + step * ticks).clamp(0.0, max);
    live.scroll = Some((to, now));
    // A span as tall as the viewport: the area goes exactly there.
    let span = Target::span_y(to, f64::from(view.h));
    ui.scroll_to(LIST_KEY, span, Reveal::Nearest);
    ui.request_repaint_at(now + AUTO_SCROLL_TICK);
}

/// The slot under a drag's pointer: none outside the pane. Above or below the list (the header,
/// the footer) it is the slot at that edge of what shows.
fn target(
    ui: &Ui<'_>,
    rects: &PaneRects,
    rows: &[PaneRow<'_>],
    layout: &ListLayout,
    item: DragItem,
    offset: f32,
) -> Option<Slot> {
    let (_, y) = ui.pointer_pos().filter(|p| rects.rect.contains(*p))?;
    // From the window to the list, here only.
    let y = y.min(rects.list.bottom() - 0.5).max(rects.list.y) + offset;
    slot_at(rows, layout, rects.list, item, y)
}

/// Where the lifted copy stands when the pointer would have it at `copy` and the slot's mark is
/// at `mark`: there, unless it would cover the mark; then moved up or down, whichever is less (a
/// tie: down), to leave [`MARK_CLEAR`] between them.
fn clear_of(copy: Rect, mark: Rect) -> Rect {
    if copy.bottom() <= mark.y || copy.y >= mark.bottom() {
        return copy;
    }
    let up = mark.y - MARK_CLEAR - copy.bottom();
    let down = mark.bottom() + MARK_CLEAR - copy.y;
    let by = if -up < down { up } else { down };
    Rect::new(copy.x, copy.y + by, copy.w, copy.h)
}

/// The picture of a drag over the list (D24): the lifted copy of the row or card, following the
/// pointer over a frosted ground and a soft shadow but standing clear of the slot's mark (the
/// line, or the collapsed card it would drop into), and the mark. Clipped to the list; it takes
/// no input.
fn paint_drag(
    ui: &mut Ui<'_>,
    list: &PaneList<'_>,
    layout: &ListLayout,
    area: Rect,
    live: &LiveDrag,
    slot: Option<Slot>,
    offset: f32,
) {
    let theme = *ui.theme();
    let blocks = blocks(list.rows, layout);
    let Some(held) = Held::find(&blocks, list.rows, layout, live.item) else {
        return;
    };
    let moved = |r: Rect, by: f32| Rect::new(r.x, r.y + by, r.w, r.h);
    let mark = slot.map(|s| s.mark);
    // What the mark covers, where the window shows it.
    let marked = match mark {
        Some(Mark::Line(line)) => Some(moved(line, -offset)),
        Some(Mark::Into(row)) => blocks
            .iter()
            .find(|b| b.row == row)
            .map(|b| moved(b.rect, -offset)),
        Some(Mark::Stay) | None => None,
    };
    let at_pointer = moved(held.rect, live.y - live.grab - held.rect.y);
    let copy = marked.map_or(at_pointer, |m| clear_of(at_pointer, m));
    // From the item's rest place to the copy's.
    let by = copy.y - held.rect.y;
    let card = matches!(live.item, DragItem::Group(_));
    let radius = if card { CARD_ROUND } else { ROW_ROUND };

    let p = ui.painter();
    p.on(LayerId::Popup);
    p.push_clip(area);
    p.shadow(moved(copy, LIFT_DROP), radius, LIFT_BLUR, LIFT_SHADOW);
    p.backdrop(copy, radius);
    p.frost(copy, radius, premul(theme.menu_glass), true);
    let member_rows = if card {
        blocks.get(held.block).map_or(0, |b| b.members)
    } else {
        0
    };
    for i in held.row..=held.row + member_rows {
        let (Some(row), Some(rect)) = (list.rows.get(i), layout.rows.get(i)) else {
            continue;
        };
        let rect = moved(*rect, by);
        match row {
            PaneRow::Group(g) => {
                paint_card(ui.painter(), g.color, list.look, copy);
                paint_header(ui.painter(), &theme, g, list.look, rect);
                let button = plus_button(&theme, "");
                let rest = ButtonState {
                    hovered: false,
                    pressed: false,
                    focus_visible: false,
                };
                let look = ButtonLook::of(&theme, ButtonKind::Ghost, rest);
                paint_icon(ui.painter(), plus_rect(rect), &button, &look);
            }
            PaneRow::Tab(t) => {
                // A dragged row is held: the hover look, unless it is the active tab's. A card's
                // rows are as at rest.
                let fill = if t.active {
                    Some(theme.surface_selected)
                } else {
                    (!card).then_some(theme.surface_hover)
                };
                let look = RowLook {
                    fill,
                    bright: !card || t.active || (t.unread && t.progress.is_none()),
                    close: (!card || t.active).then_some(ButtonState {
                        hovered: false,
                        pressed: false,
                        focus_visible: false,
                    }),
                };
                paint_tab(ui, &theme, t, rect, &look);
            }
        }
    }
    // Above the copy in draw order too.
    let p = ui.painter();
    match (mark, marked) {
        (Some(Mark::Line(_)), Some(line)) => {
            p.fill_over(line, line.h / 2.0, premul(theme.accent));
        }
        (Some(Mark::Into(_)), Some(card)) => {
            p.ring(card, CARD_ROUND, INTO_RING, theme.accent, true);
        }
        _ => {}
    }
    p.pop_clip();
    // The shell draws on the base layer.
    p.on(LayerId::Base);
}

/// The list's scroll viewport: the list's rect, widened to the pane's edge zone so the scroll
/// thumb stands clear of the cards' buttons.
fn viewport(rects: &PaneRects) -> Rect {
    Rect::new(
        rects.list.x,
        rects.list.y,
        (rects.edge.x - rects.list.x).max(0.0),
        rects.list.h,
    )
}

/// Draws the list between the pane's header and footer, scrolled vertically (D32), and returns
/// what the user asked of it. A drag of a row or a card (D24) leaves it in place, dimmed, and
/// draws a copy at the pointer; Escape, the window losing the keyboard or a release outside the
/// pane ends it with nothing moved.
pub fn list(ui: &mut Ui<'_>, rects: &PaneRects, list: &PaneList<'_>) -> ListOut {
    let view = viewport(rects);
    let mut out = ListOut::default();
    if view.w <= 0.0 || view.h <= 0.0 {
        return out;
    }
    let layout = layout_list(list.rows, rects.list);
    let key = ui.id("pane-drag");
    let mut memory = std::mem::take(ui.transient::<DragMemory>(key));
    if let Some(live) = memory.live
        && (ui.consume_key_while_pressed(live.id, NamedKey::Escape) || !ui.input().focused)
    {
        memory.live = None;
    }
    if let Some(live) = &mut memory.live {
        (live.held, live.released) = (false, false);
        auto_scroll(ui, live, rects, view, layout.height);
    }
    if list.reveal
        && let Some(row) = active_row(list.rows).and_then(|i| layout.rows.get(i))
    {
        let span = Target::span_y(f64::from(row.y - view.y), f64::from(row.h));
        ui.scroll_to(LIST_KEY, span, Reveal::Nearest);
    }
    let options = ScrollOptions {
        bars: (Bars::Disabled, Bars::Auto),
        ..ScrollOptions::default()
    };
    // The slot under a drag's pointer, and how far the list is drawn scrolled.
    let (mut slot, mut offset) = (None, 0.0);
    let scrolled = ui.scroll_area(LIST_KEY, view, &options, |ui, scroll| {
        offset = scroll.offset.1 as f32;
        let id = ui.id("pane-tabs");
        // The strip's name: it is the same list of sessions.
        let node = Node::new(id, Role::TabList, "Terminal sessions", view);
        ui.node_container(node, |ui| {
            let theme = *ui.theme();
            for block in blocks(list.rows, &layout) {
                // A card's tint, ring and header; the dragged card is drawn dimmed, whole.
                let mut ghost = false;
                let mut first = block.row;
                if let PaneRow::Group(g) = &list.rows[block.row] {
                    let rect = layout.rows[block.row];
                    let region = ui.interact(ui.id(("pane-group", g.id.0)), rect, Sense::DRAG);
                    let item = DragItem::Group(g.id);
                    let top = block.rect.y - offset;
                    memory.track(item, &region, top, ui.pointer_pos(), offset);
                    ghost = memory.drags(item);
                    if ghost {
                        ui.painter().push_xform(dimmed());
                    }
                    paint_card(ui.painter(), g.color, list.look, block.rect);
                    header(ui, &theme, g, list.look, rect, &region, &mut out);
                    first += 1;
                }
                for i in first..=block.row + block.members {
                    if let PaneRow::Tab(t) = &list.rows[i] {
                        let rect = layout.rows[i];
                        tab_row(ui, &theme, t, rect, offset, &mut memory, &mut out);
                    }
                }
                if ghost {
                    ui.painter().pop_xform();
                }
            }
            let dragging = memory.live.is_some();
            new_tab_button(ui, &theme, layout.new_tab, dragging, &mut out);
            if let Some(live) = memory.live {
                slot = target(ui, rects, list.rows, &layout, live.item, offset);
            }
            // The card a release would drop into, under the text.
            if let Some(Slot {
                mark: Mark::Into(row),
                ..
            }) = slot
                && let Some(header) = layout.rows.get(row)
            {
                ui.painter().fill(*header, ROW_ROUND, theme.surface_hover);
            }
        });
        let extent = Extent::new(f64::from(view.w), f64::from(layout.height));
        ((), extent)
    });
    // A region the list no longer shows lost the pointer: its drag is over.
    if let Some(mut live) = memory.live.take() {
        if live.released {
            if let Some(slot) = slot.filter(|s| s.mark != Mark::Stay) {
                out.events.push(PaneEvent::Drop {
                    item: live.item,
                    index: slot.index,
                    group: slot.group,
                });
            }
        } else if live.held {
            live.offset = scrolled.offset.1;
            paint_drag(ui, list, &layout, rects.list, &live, slot, offset);
            out.events.push(PaneEvent::Dragging);
            memory.live = Some(live);
        }
    }
    *ui.transient::<DragMemory>(key) = memory;
    out
}

/// What a dragged item's rest place is drawn through.
fn dimmed() -> Xform {
    Xform {
        alpha: DIM_ALPHA,
        ..Xform::translate(0.0, 0.0)
    }
}

/// A card's tint and ring (D33).
fn paint_card(p: &mut Painter<'_, WidgetId>, color: GroupColor, look: LookTheme, rect: Rect) {
    let [r, g, b] = color.tone(look).fill;
    let fill = [r, g, b, 255];
    p.fill(rect, CARD_ROUND, with_alpha(fill, CARD_TINT));
    p.ring(rect, CARD_ROUND, 1.0, with_alpha(fill, CARD_RING), false);
}

/// Where a menu asked for from the region `rect` opens: at the pointer when it is over the
/// region, else (a request that did not come from a pointer) at its bottom-left.
fn menu_at(ui: &Ui<'_>, rect: Rect) -> (f32, f32) {
    ui.pointer_pos()
        .filter(|p| rect.contains(*p))
        .unwrap_or((rect.x, rect.bottom()))
}

/// The accessible description of a tab or card with an unread notice, the strip's wording.
fn attention_text(ui: &Ui<'_>, name: &str) -> String {
    format!("{name}, {}", ui.tr("luneui.tab-strip.attention"))
}

/// The colour of the light over a title with a progress report: the strip's.
fn shine_light(theme: &Theme, tone: Tone) -> Rgba {
    match tone {
        Tone::Neutral => theme.fg_bright,
        Tone::Warn => theme.status_warning_light,
        Tone::Err => theme.status_error_light,
        _ => tone.color(theme),
    }
}

/// The attention dot, its left edge at `x`, centred on the line of `row`.
fn dot(p: &mut Painter<'_, WidgetId>, theme: &Theme, x: f32, row: Rect) {
    let rect = Rect::new(x, row.y + (row.h - DOT) / 2.0, DOT, DOT);
    p.fill(rect, DOT / 2.0, theme.attention());
}

/// A row's close button.
fn close_rect(row: Rect) -> Rect {
    Rect::new(
        row.right() - CLOSE_INSET - CLOSE_SIZE,
        row.y + (row.h - CLOSE_SIZE) / 2.0,
        CLOSE_SIZE,
        CLOSE_SIZE,
    )
}

/// How a tab's row is drawn.
struct RowLook {
    fill: Option<Rgba>,
    /// The title in the bright text colour.
    bright: bool,
    /// The close button, in this state; `None` hides it.
    close: Option<ButtonState>,
}

/// A tab's row: the glyph, the title (shining while the tab reports progress), the unread dot,
/// and on the active or hovered row the close button (D33-D35). A press activates the tab; a drag
/// of the row moves it (D24) and activates nothing more.
fn tab_row(
    ui: &mut Ui<'_>,
    theme: &Theme,
    tab: &TabRow<'_>,
    rect: Rect,
    offset: f32,
    memory: &mut DragMemory,
    out: &mut ListOut,
) {
    let id = ui.id(("pane-row", tab.id));
    let close_id = ui.id(("pane-close", tab.id));
    let close_rect = close_rect(rect);
    let row = ui.interact(id, rect, Sense::DRAG);
    // Registered after its row, so it is on top.
    let close = ui.interact(close_id, close_rect, Sense::CLICK);
    let item = DragItem::Tab(tab.id);
    memory.track(item, &row, rect.y - offset, ui.pointer_pos(), offset);
    // Nothing hovers while something is dragged.
    let hovered = memory.live.is_none() && (row.hovered || close.hovered);
    let shows_close = tab.active || hovered || (memory.live.is_none() && close.pressed);

    // A mouse activates on the press, as on the strip; a click with no press here is an
    // assistive technology's.
    let held = *ui.persistent::<bool>(id);
    if row.press_started || (row.clicked && !held) {
        out.events.push(PaneEvent::Activate(tab.id));
    }
    *ui.persistent::<bool>(id) = row.pressed && (row.press_started || held);
    if close.clicked {
        out.events.push(PaneEvent::Close(tab.id));
    }
    if row.secondary_clicked || close.secondary_clicked {
        out.tab_context = Some(TabContext {
            id: tab.id,
            at: menu_at(ui, row.rect),
        });
    }

    let fill = if tab.active {
        Some(theme.surface_selected)
    } else {
        hovered.then_some(theme.surface_hover)
    };
    let look = RowLook {
        fill,
        // A shining title keeps its base colour, so the band shows.
        bright: tab.active || (tab.unread && tab.progress.is_none()),
        close: shows_close.then_some(ButtonState {
            hovered: close.hovered && memory.live.is_none(),
            pressed: close.pressed && memory.live.is_none(),
            focus_visible: false,
        }),
    };
    let dragged = memory.drags(item);
    if dragged {
        ui.painter().push_xform(dimmed());
    }
    paint_tab(ui, theme, tab, rect, &look);
    if dragged {
        ui.painter().pop_xform();
    }

    let mut node = Node::new(id, Role::Tab, tab.title, rect);
    if tab.unread {
        node.description = Some(attention_text(ui, tab.title));
    }
    if tab.active {
        node.states.insert(State::Selected);
    }
    if tab.progress.is_some() {
        node.states.insert(State::Busy);
    }
    for action in [Action::Default, Action::ShowMenu, Action::ScrollIntoView] {
        node.actions.insert(action);
    }
    ui.node(node);
    if shows_close {
        let name = ui.tr("luneui.tab-strip.close");
        let mut node = Node::new(close_id, Role::Button, name, close_rect);
        node.actions.insert(Action::Default);
        ui.node(node);
    }
}

/// What a tab's row at `rect` shows.
fn paint_tab(ui: &mut Ui<'_>, theme: &Theme, tab: &TabRow<'_>, rect: Rect, look: &RowLook) {
    let (tier, now) = (ui.tier(), ui.time());
    if tab.progress.is_some() && tier != MotionTier::Off {
        ui.request_busy_frame(rect);
    }
    let color = if look.bright {
        theme.fg_bright
    } else {
        theme.fg_secondary
    };
    let indent = if tab.group.is_some() {
        MEMBER_INDENT
    } else {
        LOOSE_INDENT
    };
    let close_rect = close_rect(rect);
    let glyph_x = rect.x + indent;
    let title_x = glyph_x + GLYPH + GLYPH_GAP;
    // The close button's room is kept whether it shows or not, so a title never re-elides; its
    // glyph's left edge is where the title (or the dot) stops.
    let close_glyph_x = close_rect.x + (CLOSE_SIZE - CLOSE_GLYPH) / 2.0;
    let dot_x = close_glyph_x - GLYPH_GAP - DOT;
    let title_end = if tab.unread { dot_x } else { close_glyph_x } - GLYPH_GAP;

    let p = ui.painter();
    if let Some(fill) = look.fill {
        p.fill(rect, ROW_ROUND, fill);
    }
    let glyph = icon::glyph(ICON_TERMINAL, GLYPH, with_alpha(color, GLYPH_ALPHA));
    p.text_in(&glyph, Rect::new(glyph_x, rect.y, GLYPH, rect.h), 0.0);
    let title = Spec::plain(tab.title, Style::ui(TITLE_FONT, color));
    let title = p.elide(&title, (title_end - title_x).max(0.0));
    match tab.progress {
        Some(tone) => {
            let light = shine_light(theme, tone);
            shine::paint_at(p, &title, title_x, (rect.y, rect.h), light, tier, now);
        }
        None => {
            p.text_in(&title, Rect::new(title_x, rect.y, 0.0, rect.h), 0.0);
        }
    }
    if tab.unread {
        dot(p, theme, dot_x, rect);
    }
    if let Some(state) = look.close {
        let button = IconButton {
            glyph: icon::CLOSE,
            glyph_size: CLOSE_GLYPH,
            color: Some(theme.fg_tertiary),
            radius: CLOSE_ROUND,
            name: ui.tr("luneui.tab-strip.close"),
        };
        let look = ButtonLook::of(theme, ButtonKind::Ghost, state);
        paint_icon(ui.painter(), close_rect, &button, &look);
    }
}

/// A header's `+` button.
fn plus_rect(header: Rect) -> Rect {
    Rect::new(
        header.right() - PLUS_INSET - PLUS_SIZE,
        header.y + (header.h - PLUS_SIZE) / 2.0,
        PLUS_SIZE,
        PLUS_SIZE,
    )
}

fn plus_button<'a>(theme: &Theme, name: &'a str) -> IconButton<'a> {
    IconButton {
        glyph: icon::ADD,
        glyph_size: PLUS_GLYPH,
        color: Some(theme.fg_muted),
        radius: PLUS_ROUND,
        name,
    }
}

/// What a card's header at `rect` shows under its `+`: the chevron, the name in the group's text
/// colour, on a collapsed card with an unread member the dot, and the count (D26, D34).
fn paint_header(
    p: &mut Painter<'_, WidgetId>,
    theme: &Theme,
    group: &GroupRow,
    look: LookTheme,
    rect: Rect,
) {
    let [r, g, b] = group.color.tone(look).text;
    let text = [r, g, b, 255];
    if group.holds_active {
        p.fill(rect, ROW_ROUND, theme.surface_selected);
    }
    let chevron = if group.collapsed {
        icon::CHEVRON_RIGHT
    } else {
        icon::CHEVRON_DOWN
    };
    let chevron_x = rect.x + HEADER_PAD;
    p.text_centered(
        &icon::glyph(chevron, CHEVRON, text),
        Rect::new(chevron_x, rect.y, CHEVRON, rect.h),
    );
    let count = Spec::plain(
        group.count.to_string(),
        Style::ui(COUNT_FONT, theme.fg_muted),
    );
    let count_x = plus_rect(rect).x - HEADER_GAP - p.measure(&count).0;
    p.text_in(&count, Rect::new(count_x, rect.y, 0.0, rect.h), 0.0);
    let mut name_end = count_x - HEADER_GAP;
    if group.unread {
        let x = name_end - DOT;
        dot(p, theme, x, rect);
        name_end = x - HEADER_GAP;
    }
    let name_x = chevron_x + CHEVRON + HEADER_GAP;
    let name = Spec::plain(&group.name, Style::ui(NAME_FONT, text).medium());
    let name = p.elide(&name, (name_end - name_x).max(0.0));
    p.text_in(&name, Rect::new(name_x, rect.y, 0.0, rect.h), 0.0);
}

/// A card's header and its `+` button (D35); `header` is what its region saw. A click toggles the
/// card; a drag moves the card (D24) and toggles nothing.
fn header(
    ui: &mut Ui<'_>,
    theme: &Theme,
    group: &GroupRow,
    look: LookTheme,
    rect: Rect,
    header: &Response,
    out: &mut ListOut,
) {
    let id = header.id;
    paint_header(ui.painter(), theme, group, look, rect);

    let mut node = Node::new(id, Role::Button, &group.name, rect);
    let tabs = group.count.to_string();
    node.description = Some(if group.unread {
        attention_text(ui, &tabs)
    } else {
        tabs
    });
    if !group.collapsed {
        node.states.insert(State::Expanded);
    }
    if group.holds_active {
        node.states.insert(State::Selected);
    }
    for action in [
        Action::Default,
        Action::Expand,
        Action::Collapse,
        Action::ShowMenu,
        Action::ScrollIntoView,
    ] {
        node.actions.insert(action);
    }
    ui.node(node);
    // After the header, so it is on top.
    let name = ui.tr("menu.newTabInGroup");
    let button = plus_button(theme, name);
    let add = ui.icon_button(("pane-group-add", group.id.0), plus_rect(rect), &button);
    if header.clicked {
        out.events.push(PaneEvent::ToggleGroup(group.id));
    }
    if add.clicked {
        out.events.push(PaneEvent::NewTabIn(group.id));
    }
    if header.secondary_clicked || add.secondary_clicked {
        out.events.push(PaneEvent::GroupMenu {
            id: group.id,
            at: menu_at(ui, header.rect),
        });
    }
}

/// The dashed New Tab button after the last block (D22): no shortcut hint.
fn new_tab_button(ui: &mut Ui<'_>, theme: &Theme, rect: Rect, dragging: bool, out: &mut ListOut) {
    let id = ui.id("pane-new-tab");
    let button = ui.interact(id, rect, Sense::CLICK);
    let name = ui.tr("menu.newTab");
    let p = ui.painter();
    if button.hovered && !dragging {
        p.fill(rect, CARD_ROUND, theme.surface_hover);
    }
    // Half a px inside, so the 1 px stroke lies in the button.
    let outline = rounded_outline(rect.inset(0.5, 0.5), CARD_ROUND - 0.5);
    p.dash(&outline, true, 1.0, DASH.0, DASH.1, theme.border);
    let x = rect.x + LOOSE_INSET + LOOSE_INDENT;
    let glyph = icon::glyph(icon::ADD, GLYPH, theme.fg_muted);
    p.text_in(&glyph, Rect::new(x, rect.y, GLYPH, rect.h), 0.0);
    let label = Spec::plain(name, Style::ui(TITLE_FONT, theme.fg_muted));
    let label_x = x + GLYPH + GLYPH_GAP;
    p.text_in(&label, Rect::new(label_x, rect.y, 0.0, rect.h), 0.0);
    let mut node = Node::new(id, Role::Button, name, rect);
    node.actions.insert(Action::Default);
    ui.node(node);
    if button.clicked {
        out.events.push(PaneEvent::NewTab);
    }
}

/// The outline of `rect` with corners of `radius`, as a closed polyline.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<[f32; 2]> {
    /// The segments of a quarter circle.
    const STEPS: u16 = 8;
    let radius = radius.min(rect.w / 2.0).min(rect.h / 2.0).max(0.0);
    let (left, right) = (rect.x + radius, rect.right() - radius);
    let (top, bottom) = (rect.y + radius, rect.bottom() - radius);
    // Clockwise from the top right corner: each corner's centre and start angle.
    [
        (right, top, -FRAC_PI_2),
        (right, bottom, 0.0),
        (left, bottom, FRAC_PI_2),
        (left, top, PI),
    ]
    .into_iter()
    .flat_map(|(x, y, start)| {
        (0..=STEPS).map(move |i| {
            let angle = start + FRAC_PI_2 * f32::from(i) / f32::from(STEPS);
            [x + radius * angle.cos(), y + radius * angle.sin()]
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{LINUX_CHROME, MACOS_CHROME, WINDOWS_CHROME};
    use lune_ui::access::AccessAction;
    use lune_ui::input::{Event, Key, Modifiers, PointerButton, RawInput, ScrollDelta, keymap};
    use lune_ui::lunegit_dark::THEME;
    use lune_ui::motion::Cadence;
    use lune_ui::shortcuts::Shortcuts;
    use lune_ui::strings::DisplayFormat;
    use lune_ui::text::TextCache;
    use lune_ui::ui::{FrameConfig, UiStore};
    use std::time::Duration;

    const WINDOW: Rect = Rect::new(0.0, 0.0, 1040.0, 660.0);

    #[test]
    fn the_pane_shows_at_its_width_plus_490_and_not_one_pixel_below() {
        assert!(fits(738.0, 248.0));
        assert!(!fits(737.0, 248.0));
        for chrome in [MACOS_CHROME, WINDOWS_CHROME, LINUX_CHROME] {
            assert_eq!(
                resolve(true, 248, 738.0, chrome),
                View::Pane {
                    width: 248.0,
                    drawn_caption: chrome.drawn_caption
                }
            );
            assert_eq!(resolve(true, 248, 737.0, chrome), View::Bar);
            assert_eq!(resolve(false, 248, 1040.0, chrome), View::Bar);
        }
        // The saved width decides: a wider pane needs a wider window.
        assert_eq!(resolve(true, 420, 909.0, MACOS_CHROME), View::Bar);
        assert_ne!(resolve(true, 420, 910.0, MACOS_CHROME), View::Bar);
    }

    #[test]
    fn the_show_button_is_offered_only_by_a_bar_wide_enough_for_the_pane() {
        assert!(offers_show_button(View::Bar, 738.0, 248));
        assert!(!offers_show_button(View::Bar, 737.0, 248));
        let pane = resolve(true, 248, 1040.0, MACOS_CHROME);
        assert!(!offers_show_button(pane, 1040.0, 248));
        // The saved choice is the pane but the window is narrow: bar, no button.
        let narrow = resolve(true, 248, 737.0, MACOS_CHROME);
        assert!(!offers_show_button(narrow, 737.0, 248));
    }

    #[test]
    fn the_pane_rects_follow_d26_and_the_directors_footer() {
        let r = PaneRects::new(WINDOW, 248.0);
        assert_eq!(r.rect, Rect::new(0.0, 0.0, 248.0, 660.0));
        assert_eq!(r.header, Rect::new(0.0, 0.0, 248.0, 34.0));
        assert_eq!(r.edge, Rect::new(242.0, 0.0, 6.0, 660.0));
        // Footer buttons end on the card's bottom line (660 - 10), the divider 8 above them.
        assert_eq!(r.bell, Rect::new(12.0, 626.0, 24.0, 24.0));
        assert_eq!(r.menu, Rect::new(212.0, 626.0, 24.0, 24.0));
        assert_eq!(r.divider, Rect::new(12.0, 617.0, 224.0, 1.0));
        assert_eq!(r.footer, Rect::new(0.0, 617.0, 248.0, 43.0));
        assert_eq!(r.list, Rect::new(8.0, 34.0, 232.0, 583.0));
        // No control overlaps the edge zone.
        for c in [r.bell, r.menu, r.list, hide_button(&r, MACOS_CHROME)] {
            assert!(c.right() <= r.edge.x, "{c:?}");
        }
    }

    #[test]
    fn the_title_buttons_sit_on_the_lights_line_or_the_caption_buttons() {
        let r = PaneRects::new(WINDOW, 248.0);
        assert_eq!(
            hide_button(&r, MACOS_CHROME),
            Rect::new(216.0, 4.0, 24.0, 24.0)
        );
        assert_eq!(
            hide_button(&r, WINDOWS_CHROME),
            Rect::new(216.0, 5.0, 24.0, 24.0)
        );
        assert_eq!(
            show_button(WINDOW, MACOS_CHROME),
            Rect::new(76.0, 4.0, 24.0, 24.0)
        );
        for chrome in [WINDOWS_CHROME, LINUX_CHROME] {
            assert_eq!(
                show_button(WINDOW, chrome),
                Rect::new(10.0, 5.0, 24.0, 24.0)
            );
        }
    }

    #[test]
    fn a_drag_is_limited_by_the_range_and_by_what_the_window_leaves() {
        assert_eq!(max_width(1040.0), 420);
        assert_eq!(max_width(910.0), 420);
        assert_eq!(max_width(800.0), 310);
        assert_eq!(max_width(738.0), 248);
        assert_eq!(max_width(100.0), 200);
        assert_eq!(max_width(f32::NAN), 200);
    }

    /// A frame of the zone: it started, moved `dx`, and is held or not.
    fn edge(started: bool, dx: f32, held: bool) -> Option<PaneEdge> {
        Some(PaneEdge { started, dx, held })
    }

    #[test]
    fn the_width_follows_the_pointer_clamped_and_is_saved_once_at_the_release() {
        let mut drag = None;
        let mut width = 248;
        let mut saves = 0;
        // Idle, then a press that has not moved yet, the start, far past both limits and back.
        let frames = [
            (edge(false, 0.0, false), 248),
            (edge(false, 0.0, true), 248),
            (edge(true, 10.0, true), 258),
            (edge(false, 500.0, true), 420),
            (edge(false, -10.0, true), 420),
            (edge(false, -100.0, true), 420),
            (edge(false, -300.0, true), 348),
            (edge(false, -900.0, true), 200),
            (edge(false, 900.0, true), 348),
            (edge(false, 0.0, true), 348),
        ];
        for (i, (frame, want)) in frames.into_iter().enumerate() {
            let step = drag_step(&mut drag, frame, width, 1040.0);
            width = step.width.unwrap_or(width);
            saves += usize::from(step.save);
            assert_eq!(width, want, "frame {i}");
            assert_eq!(saves, 0, "frame {i}: nothing is saved while the drag runs");
        }
        // The release: one save, and nothing more after it.
        let step = drag_step(&mut drag, edge(false, 2.0, false), width, 1040.0);
        assert_eq!(
            step,
            DragStep {
                width: Some(350),
                save: true
            }
        );
        let after = drag_step(&mut drag, edge(false, 0.0, false), 350, 1040.0);
        assert_eq!(after, DragStep::default());
        assert!(drag.is_none());
    }

    #[test]
    fn a_drag_that_ends_where_it_began_saves_nothing_and_a_vanished_zone_ends_it() {
        let mut drag = None;
        let step = drag_step(&mut drag, edge(true, 20.0, true), 248, 1040.0);
        assert_eq!(step.width, Some(268));
        let back = drag_step(&mut drag, edge(false, -20.0, false), 268, 1040.0);
        assert_eq!(
            back,
            DragStep {
                width: Some(248),
                save: false
            }
        );
        // The view changed under the pointer (no zone): the width so far is saved.
        drag_step(&mut drag, edge(true, 20.0, true), 248, 1040.0);
        let gone = drag_step(&mut drag, None, 268, 1040.0);
        assert_eq!(
            gone,
            DragStep {
                width: None,
                save: true
            }
        );
        assert!(drag.is_none());
        // A move with no drag under way does nothing.
        let stray = drag_step(&mut drag, edge(false, 50.0, true), 248, 1040.0);
        assert_eq!(stray, DragStep::default());
        // A start and release in one frame.
        let quick = drag_step(&mut drag, edge(true, 30.0, false), 248, 1040.0);
        assert_eq!(
            quick,
            DragStep {
                width: Some(278),
                save: true
            }
        );
    }

    // --------------------------------------------------------------------------- the tab list

    /// A tab of the strip: its id and title, no progress, no unread notice.
    fn strip_tab(id: TabId, title: &str) -> StripTab<'_> {
        StripTab {
            id,
            title,
            progress: None,
            attention: false,
        }
    }

    /// The set and strip of a picture of tabs: one `(title, group)` each, `""` for a loose tab;
    /// the first is active.
    fn tabs<'a>(of: &[(&'a str, &str)]) -> (TabSet<()>, Vec<StripTab<'a>>) {
        let mut set = TabSet::new();
        let ids: Vec<TabId> = of.iter().map(|_| set.push(())).collect();
        for (id, (_, group)) in ids.iter().zip(of) {
            if !group.is_empty() {
                set.set_group(*id, group);
            }
        }
        set.activate(ids[0]);
        let strip = ids.iter().zip(of).map(|(id, (t, _))| strip_tab(*id, t));
        let strip = strip.collect();
        (set, strip)
    }

    /// The mock's picture: a loose tab, an expanded group of three and a group of two.
    const MOCK: &[(&str, &str)] = &[
        ("zsh", ""),
        ("server", "api"),
        ("logs", "api"),
        ("claude", "api"),
        ("deploy", "infra"),
        ("db", "infra"),
    ];

    /// Each row as a word: a tab's title, a header's name in brackets.
    fn words(rows: &[PaneRow<'_>]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                PaneRow::Tab(t) => t.title.to_owned(),
                PaneRow::Group(g) => format!("[{}]", g.name),
            })
            .collect()
    }

    fn headers(rows: &[PaneRow<'_>]) -> Vec<GroupRow> {
        rows.iter()
            .filter_map(|r| match r {
                PaneRow::Group(g) => Some(g.clone()),
                PaneRow::Tab(_) => None,
            })
            .collect()
    }

    #[test]
    fn the_rows_follow_the_sets_order_with_collapsed_members_left_out() {
        let (mut set, strip) = tabs(MOCK);
        assert_eq!(
            words(&rows(&set, &strip)),
            [
                "zsh", "[api]", "server", "logs", "claude", "[infra]", "deploy", "db"
            ]
        );
        let infra = set.group_named("infra").expect("group");
        set.set_collapsed(infra, true);
        let built = rows(&set, &strip);
        assert_eq!(
            words(&built),
            ["zsh", "[api]", "server", "logs", "claude", "[infra]"]
        );
        // Member rows carry their group's colour, loose rows none; every count is the group's.
        let api = set.group_named("api").expect("group");
        let color = |g| set.group(g).map(|g| g.color);
        for row in &built {
            if let PaneRow::Tab(t) = row {
                let want = if t.title == "zsh" { None } else { color(api) };
                assert_eq!(t.group, want, "{}", t.title);
            }
        }
        let counts: Vec<_> = headers(&built).iter().map(|g| (g.count, g.color)).collect();
        assert_eq!(
            counts,
            [
                (3, color(api).expect("api")),
                (2, color(infra).expect("infra"))
            ]
        );
        // The set's own order after a move is the rows' order.
        let ids = set.ids();
        assert!(set.move_group(infra, 0));
        let mut moved = strip.clone();
        moved.sort_by_key(|t| set.index_of(t.id));
        assert_ne!(set.ids(), ids);
        assert_eq!(words(&rows(&set, &moved))[0], "[infra]");
    }

    #[test]
    fn two_groups_next_to_each_other_have_a_header_each_and_loose_tabs_none() {
        let (set, strip) = tabs(&[("a", "x"), ("b", "y"), ("c", ""), ("d", "")]);
        assert_eq!(
            words(&rows(&set, &strip)),
            ["[x]", "a", "[y]", "b", "c", "d"]
        );
        assert!(rows(&TabSet::<()>::new(), &[]).is_empty());
    }

    #[test]
    fn a_collapsed_card_has_one_dot_for_its_unread_members_and_an_expanded_one_none() {
        let (mut set, mut strip) = tabs(MOCK);
        // `logs` (expanded), `deploy` and `db` (both in infra) are unread.
        for i in [2, 4, 5] {
            strip[i].attention = true;
        }
        let infra = set.group_named("infra").expect("group");
        set.set_collapsed(infra, true);
        let built = rows(&set, &strip);
        let unread: Vec<_> = headers(&built)
            .iter()
            .map(|g| (g.name.clone(), g.unread))
            .collect();
        assert_eq!(
            unread,
            [("api".to_owned(), false), ("infra".to_owned(), true)]
        );
        let marked: Vec<_> = built
            .iter()
            .filter_map(|r| match r {
                PaneRow::Tab(t) => t.unread.then_some(t.title),
                PaneRow::Group(_) => None,
            })
            .collect();
        assert_eq!(
            marked,
            ["logs"],
            "an expanded card shows the marks on its rows"
        );
        // No unread member: no dot.
        strip[4].attention = false;
        strip[5].attention = false;
        assert!(!headers(&rows(&set, &strip))[1].unread);
        // Expanded again: the dot is gone from the header, the members have theirs.
        strip[5].attention = true;
        set.set_collapsed(infra, false);
        assert!(!headers(&rows(&set, &strip))[1].unread);
    }

    #[test]
    fn the_active_tab_in_a_collapsed_card_selects_the_header_and_is_the_row_to_reveal() {
        let (mut set, strip) = tabs(MOCK);
        let infra = set.group_named("infra").expect("group");
        let deploy = strip[4].id;
        set.activate(deploy);
        let built = rows(&set, &strip);
        assert!(headers(&built).iter().all(|g| !g.holds_active));
        let at = active_row(&built).expect("the active row");
        assert!(matches!(&built[at], PaneRow::Tab(t) if t.title == "deploy" && t.active));
        set.set_collapsed(infra, true);
        assert_eq!(set.active(), Some(deploy), "collapsing moves nothing");
        let built = rows(&set, &strip);
        let held: Vec<_> = headers(&built).iter().map(|g| g.holds_active).collect();
        assert_eq!(held, [false, true]);
        let at = active_row(&built).expect("the header");
        assert!(matches!(&built[at], PaneRow::Group(g) if g.name == "infra"));
        // A loose active tab selects no header.
        set.activate(strip[0].id);
        let built = rows(&set, &strip);
        assert_eq!(active_row(&built), Some(0));
        assert!(headers(&built).iter().all(|g| !g.holds_active));
    }

    #[test]
    fn a_progress_report_reaches_its_row() {
        let (set, mut strip) = tabs(MOCK);
        strip[0].progress = Some(Tone::Neutral);
        strip[2].progress = Some(Tone::Err);
        let shining: Vec<_> = rows(&set, &strip)
            .iter()
            .filter_map(|r| match r {
                PaneRow::Tab(t) => t.progress.map(|p| (t.title.to_owned(), p)),
                PaneRow::Group(_) => None,
            })
            .collect();
        assert_eq!(
            shining,
            [
                ("zsh".to_owned(), Tone::Neutral),
                ("logs".to_owned(), Tone::Err)
            ]
        );
    }

    #[test]
    fn the_blocks_stand_six_apart_and_members_line_up_with_their_headers_name() {
        let (mut set, strip) = tabs(MOCK);
        let infra = set.group_named("infra").expect("group");
        set.set_collapsed(infra, true);
        let built = rows(&set, &strip);
        let area = PaneRects::new(WINDOW, 248.0).list;
        let l = layout_list(&built, area);
        // The loose row: inside the list by the block's padding.
        assert_eq!(l.rows[0], Rect::new(12.0, 34.0, 224.0, 32.0));
        // The expanded card: padding 3, header 30, three rows of 32 each after a 1 px gap.
        assert_eq!(l.cards[0].1, Rect::new(8.0, 72.0, 232.0, 135.0));
        assert_eq!(l.rows[1], Rect::new(11.0, 75.0, 226.0, 30.0));
        assert_eq!(l.rows[2], Rect::new(11.0, 106.0, 226.0, 32.0));
        assert_eq!(l.rows[3].y, 139.0);
        assert_eq!(l.rows[4].bottom(), l.cards[0].1.bottom() - CARD_PAD);
        // The collapsed card: its header alone.
        assert_eq!(l.cards[1].1, Rect::new(8.0, 213.0, 232.0, 36.0));
        assert_eq!(l.new_tab, Rect::new(8.0, 255.0, 232.0, 34.0));
        assert_eq!(l.height, 255.0);
        assert_eq!(l.rows.len(), built.len());
        // The member's glyph starts where the header's name does.
        let name_x = l.rows[1].x + HEADER_PAD + CHEVRON + HEADER_GAP;
        assert_eq!(l.rows[2].x + MEMBER_INDENT, name_x);
        assert_eq!(l.rows[0].x + LOOSE_INDENT, area.x + 12.0);
        // No rows: the button alone, at the top.
        let empty = layout_list(&[], area);
        assert_eq!((empty.new_tab.y, empty.height), (34.0, 34.0));
    }

    #[test]
    fn the_new_tab_outline_stays_in_its_rect_and_closes_on_itself() {
        let r = Rect::new(10.0, 20.0, 100.0, 34.0);
        let outline = rounded_outline(r, 10.0);
        assert_eq!(outline.len(), 36);
        for [x, y] in &outline {
            assert!((r.x - 0.001..=r.right() + 0.001).contains(x), "{x}");
            assert!((r.y - 0.001..=r.bottom() + 0.001).contains(y), "{y}");
        }
        // Clockwise from the top edge: the first point on it, the last on it too.
        assert!((outline[0][1] - r.y).abs() < 0.001);
        assert!((outline[35][1] - r.y).abs() < 0.001);
        // A radius larger than the rect allows is cut to it.
        for [x, y] in rounded_outline(Rect::new(0.0, 0.0, 8.0, 4.0), 10.0) {
            assert!((-0.001..=8.001).contains(&x) && (-0.001..=4.001).contains(&y));
        }
    }

    // ------------------------------------------------------------------- the list, in frames

    /// The list alone in a 1040x660 window at a pane 248 wide: what a frame reported and its
    /// semantic nodes.
    struct Harness {
        store: UiStore,
        text: TextCache,
        reveal: bool,
        time: Duration,
        /// How far each frame moves the clock, the motion tier and whether the window has the
        /// keyboard.
        step: Duration,
        tier: MotionTier,
        focused: bool,
    }

    impl Harness {
        fn new() -> Harness {
            Harness {
                store: UiStore::default(),
                text: crate::fonts::bundled_text_cache(),
                reveal: false,
                time: Duration::ZERO,
                step: Duration::from_millis(400),
                tier: MotionTier::Off,
                focused: true,
            }
        }

        fn frame(&mut self, rows: &[PaneRow<'_>], events: Vec<Event>) -> (ListOut, Vec<Node>) {
            let input = RawInput {
                size: (1040.0, 660.0),
                scale: 1.0,
                time: self.time,
                focused: self.focused,
                events,
            };
            self.time += self.step;
            let strings = crate::strings::english();
            let shortcuts = Shortcuts::default();
            let config = FrameConfig {
                theme: THEME,
                tier: self.tier,
                busy_cadence: Cadence::BUSY,
                strings: &strings,
                arg_format: &DisplayFormat,
                keymap: keymap(),
                shortcuts: &shortcuts,
            };
            let rects = PaneRects::new(WINDOW, 248.0);
            let view = PaneList {
                rows,
                look: LookTheme::Luneterm,
                reveal: std::mem::take(&mut self.reveal),
            };
            let mut ui = Ui::begin(&mut self.store, &input, config, &mut self.text);
            let out = list(&mut ui, &rects, &view);
            let (_, frame) = ui.finish();
            (out, frame.tree.nodes)
        }

        /// The pointer moves to `at`, a frame passes, and a button goes down and up there: the
        /// events of the press frame and of the release frame.
        fn click(
            &mut self,
            rows: &[PaneRow<'_>],
            at: (f32, f32),
            which: PointerButton,
        ) -> (ListOut, ListOut) {
            self.frame(rows, vec![]);
            self.frame(rows, vec![Event::PointerMoved { pos: at }]);
            let press = |pressed| Event::PointerButton {
                pos: at,
                button: which,
                pressed,
                modifiers: Modifiers::default(),
            };
            let down = self.frame(rows, vec![press(true)]).0;
            let up = self.frame(rows, vec![press(false)]).0;
            (down, up)
        }
    }

    fn node<'a>(nodes: &'a [Node], role: Role, name: &str) -> &'a Node {
        nodes
            .iter()
            .find(|n| n.role == role && n.name == name)
            .unwrap_or_else(|| panic!("no {role:?} {name}"))
    }

    fn middle(r: Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn a_row_activates_on_the_press_once_and_its_close_button_closes() {
        let (set, strip) = tabs(MOCK);
        let rows = rows(&set, &strip);
        let layout = layout_list(&rows, PaneRects::new(WINDOW, 248.0).list);
        let logs = strip[2].id;
        let row = layout.rows[3];
        let mut h = Harness::new();
        let (down, up) = h.click(&rows, (row.x + 60.0, row.y + 16.0), PointerButton::Primary);
        assert_eq!(down.events, [PaneEvent::Activate(logs)]);
        assert!(up.events.is_empty(), "{:?}", up.events);
        assert!(down.tab_context.is_none() && up.tab_context.is_none());
        // The close button, shown by the hover over an inactive row: it closes and activates
        // nothing.
        let close = (row.right() - CLOSE_INSET - CLOSE_SIZE / 2.0, row.y + 16.0);
        let (down, up) = h.click(&rows, close, PointerButton::Primary);
        assert!(down.events.is_empty(), "{:?}", down.events);
        assert_eq!(up.events, [PaneEvent::Close(logs)]);
    }

    #[test]
    fn a_secondary_press_on_a_row_or_its_close_button_asks_for_the_tab_menu_there() {
        let (set, strip) = tabs(MOCK);
        let rows = rows(&set, &strip);
        let layout = layout_list(&rows, PaneRects::new(WINDOW, 248.0).list);
        let row = layout.rows[3];
        for at in [
            (row.x + 60.0, row.y + 16.0),
            (row.right() - CLOSE_INSET - CLOSE_SIZE / 2.0, row.y + 16.0),
        ] {
            let (down, up) = Harness::new().click(&rows, at, PointerButton::Secondary);
            let asked = down.tab_context.or(up.tab_context).expect("a menu");
            assert_eq!((asked.id, asked.at), (strip[2].id, at));
            assert!(down.events.is_empty() && up.events.is_empty());
        }
    }

    #[test]
    fn a_header_toggles_on_the_click_its_plus_adds_and_its_menu_is_the_groups() {
        let (set, strip) = tabs(MOCK);
        let rows = rows(&set, &strip);
        let layout = layout_list(&rows, PaneRects::new(WINDOW, 248.0).list);
        let api = set.group_named("api").expect("group");
        let header = layout.rows[1];
        let name = (header.x + 80.0, header.y + 15.0);
        let (down, up) = Harness::new().click(&rows, name, PointerButton::Primary);
        assert!(
            down.events.is_empty(),
            "not on the press: a drag is no click"
        );
        assert_eq!(up.events, [PaneEvent::ToggleGroup(api)]);
        let plus = (
            header.right() - PLUS_INSET - PLUS_SIZE / 2.0,
            header.y + 15.0,
        );
        let (down, up) = Harness::new().click(&rows, plus, PointerButton::Primary);
        assert!(down.events.is_empty());
        assert_eq!(up.events, [PaneEvent::NewTabIn(api)]);
        for at in [name, plus] {
            let (down, up) = Harness::new().click(&rows, at, PointerButton::Secondary);
            let asked: Vec<_> = down.events.into_iter().chain(up.events).collect();
            assert_eq!(asked, [PaneEvent::GroupMenu { id: api, at }]);
        }
    }

    #[test]
    fn the_new_tab_button_asks_for_a_loose_tab_and_the_gaps_ask_for_nothing() {
        let (set, strip) = tabs(MOCK);
        let rows = rows(&set, &strip);
        let layout = layout_list(&rows, PaneRects::new(WINDOW, 248.0).list);
        let at = middle(layout.new_tab);
        let (down, up) = Harness::new().click(&rows, at, PointerButton::Primary);
        assert!(down.events.is_empty());
        assert_eq!(up.events, [PaneEvent::NewTab]);
        // The 6 px between the loose row and the first card.
        let gap = (100.0, layout.rows[0].bottom() + 3.0);
        let (down, up) = Harness::new().click(&rows, gap, PointerButton::Primary);
        assert_eq!((down, up), (ListOut::default(), ListOut::default()));
    }

    /// Rows, headers and buttons have accessible names, and an assistive technology's actions
    /// reach them.
    #[test]
    fn rows_and_headers_are_named_in_the_tree_and_take_actions() {
        let (mut set, mut strip) = tabs(MOCK);
        strip[2].attention = true;
        strip[4].attention = true;
        strip[3].progress = Some(Tone::Neutral);
        let infra = set.group_named("infra").expect("group");
        set.set_collapsed(infra, true);
        let rows = rows(&set, &strip);
        let mut h = Harness::new();
        let (_, nodes) = h.frame(&rows, vec![]);
        let zsh = node(&nodes, Role::Tab, "zsh");
        assert!(zsh.states.contains(State::Selected));
        assert!(zsh.actions.contains(Action::Default));
        assert!(zsh.actions.contains(Action::ShowMenu));
        let logs = node(&nodes, Role::Tab, "logs");
        assert_eq!(logs.description.as_deref(), Some("logs, Needs attention"));
        assert!(!logs.states.contains(State::Selected));
        assert!(
            node(&nodes, Role::Tab, "claude")
                .states
                .contains(State::Busy)
        );
        let api = node(&nodes, Role::Button, "api");
        assert_eq!(api.description.as_deref(), Some("3"));
        assert!(api.states.contains(State::Expanded));
        let infra_node = node(&nodes, Role::Button, "infra");
        assert_eq!(
            infra_node.description.as_deref(),
            Some("2, Needs attention")
        );
        assert!(!infra_node.states.contains(State::Expanded));
        assert!(nodes.iter().all(|n| n.name != "deploy"), "left out");
        node(&nodes, Role::Button, "New Tab");
        node(&nodes, Role::Button, "New Tab in Group");
        node(&nodes, Role::TabList, "Terminal sessions");
        // A press by an assistive technology, with no pointer.
        let act = |target, action| {
            Event::Access(AccessAction {
                target,
                action,
                value: None,
            })
        };
        let (out, _) = h.frame(&rows, vec![act(logs.id, Action::Default)]);
        assert_eq!(out.events, [PaneEvent::Activate(strip[2].id)]);
        let (out, _) = h.frame(&rows, vec![act(infra_node.id, Action::Expand)]);
        assert_eq!(out.events, [PaneEvent::ToggleGroup(infra)]);
        let (out, _) = h.frame(&rows, vec![act(logs.id, Action::ShowMenu)]);
        assert_eq!(out.tab_context.map(|c| c.id), Some(strip[2].id));
    }

    /// Sixty tabs in ten groups: the list scrolls, a reveal brings the active row into view, and
    /// nothing is drawn or pressed outside the list.
    #[test]
    fn a_long_list_scrolls_and_a_reveal_shows_the_active_row() {
        let titles: Vec<String> = (1..=60).map(|i| format!("tab {i}")).collect();
        let groups: Vec<String> = (0..60).map(|i| format!("area {}", i / 6)).collect();
        let picture: Vec<(&str, &str)> = titles
            .iter()
            .zip(&groups)
            .map(|(t, g)| (t.as_str(), g.as_str()))
            .collect();
        let (mut set, strip) = tabs(&picture);
        let list = PaneRects::new(WINDOW, 248.0).list;
        let layout = layout_list(&rows(&set, &strip), list);
        assert!(layout.height > 4.0 * list.h, "{}", layout.height);
        // Where the row named `title` stands in the window, and whether it is in the list.
        let place = |nodes: &[Node], title: &str| {
            let b = node(nodes, Role::Tab, title).bounds;
            (
                b,
                b.y >= list.y - 0.01 && b.bottom() <= list.bottom() + 0.01,
            )
        };
        let mut h = Harness::new();
        let (_, nodes) = h.frame(&rows(&set, &strip), vec![]);
        assert!(!place(&nodes, "tab 46").1);
        assert!(place(&nodes, "tab 1").1);
        set.activate(strip[45].id);
        let built = rows(&set, &strip);
        h.reveal = true;
        h.frame(&built, vec![]);
        let (_, nodes) = h.frame(&built, vec![]);
        let (revealed, inside) = place(&nodes, "tab 46");
        assert!(inside, "{revealed:?}");
        // The nearest edge: from above, the row lands at the bottom.
        assert!(
            (revealed.bottom() - list.bottom()).abs() < 0.01,
            "{revealed:?}"
        );
        // Without a reveal the offset stays where the user scrolled to.
        let over = Event::PointerMoved {
            pos: (100.0, 300.0),
        };
        let wheel = Event::Scroll {
            pos: (100.0, 300.0),
            delta: ScrollDelta::Lines { x: 0.0, y: 3.0 },
            modifiers: Modifiers::default(),
        };
        h.frame(&built, vec![over, wheel]);
        h.frame(&built, vec![]);
        let (_, nodes) = h.frame(&built, vec![]);
        assert!(
            place(&nodes, "tab 46").0.y > revealed.y,
            "wheeled towards the top"
        );
        // Back to the first tab: the reveal brings it to the top edge.
        set.activate(strip[0].id);
        let built = rows(&set, &strip);
        h.reveal = true;
        h.frame(&built, vec![]);
        let (_, nodes) = h.frame(&built, vec![]);
        assert!(place(&nodes, "tab 1").1);
        // A press where the footer is reaches no row, whatever lies there unscrolled.
        let footer = middle(PaneRects::new(WINDOW, 248.0).footer);
        let (down, up) = h.click(&built, footer, PointerButton::Primary);
        assert_eq!((down, up), (ListOut::default(), ListOut::default()));
    }

    /// A collapsed group holding the active tab: the reveal shows its header.
    #[test]
    fn a_reveal_shows_the_header_of_a_collapsed_card_holding_the_active_tab() {
        let titles: Vec<String> = (1..=40).map(|i| format!("tab {i}")).collect();
        let mut picture: Vec<(&str, &str)> = titles.iter().map(|t| (t.as_str(), "")).collect();
        picture.push(("hidden", "last"));
        let (mut set, strip) = tabs(&picture);
        let last = set.group_named("last").expect("group");
        set.set_collapsed(last, true);
        set.activate(strip[40].id);
        let built = rows(&set, &strip);
        let list = PaneRects::new(WINDOW, 248.0).list;
        let mut h = Harness::new();
        h.reveal = true;
        h.frame(&built, vec![]);
        let (_, nodes) = h.frame(&built, vec![]);
        let header = node(&nodes, Role::Button, "last").bounds;
        assert!(
            header.y >= list.y && header.bottom() <= list.bottom() + 0.01,
            "{header:?}"
        );
    }

    // ------------------------------------------------------------------------------ the drag

    /// Mock F's tabs: a loose tab, `api` of three, `shop` of two and `infra` collapsed.
    const CARDS: &[(&str, &str)] = &[
        ("zsh", ""),
        ("server", "api"),
        ("logs", "api"),
        ("claude", "api"),
        ("web", "shop"),
        ("worker", "shop"),
        ("deploy", "infra"),
    ];

    /// [`CARDS`] with `infra` collapsed, `server` active. In the list of a 660 px window: `zsh`
    /// 34..66; the card `api` 72..207 (header 75..105, rows from 106, 139, 172); `shop` 213..315
    /// (header 216..246, rows from 247 and 280); `infra` 321..357 (header 324..354); New Tab
    /// 363..397.
    fn cards() -> (TabSet<()>, Vec<StripTab<'static>>) {
        let (mut set, strip) = tabs(CARDS);
        let infra = set.group_named("infra").expect("group");
        set.set_collapsed(infra, true);
        set.activate(strip[1].id);
        (set, strip)
    }

    fn area() -> Rect {
        PaneRects::new(WINDOW, 248.0).list
    }

    /// The slot of a drag of `item` at `y` of the list of `set`.
    fn slot(set: &TabSet<()>, strip: &[StripTab<'_>], item: DragItem, y: f32) -> Option<Slot> {
        let rows = rows(set, strip);
        slot_at(&rows, &layout_list(&rows, area()), area(), item, y)
    }

    /// A line between blocks, centred at `y`: as wide as the list.
    fn between(y: f32) -> Mark {
        Mark::Line(Rect::new(8.0, y - 1.0, 232.0, 2.0))
    }

    /// A line inside a card, centred at `y`: as wide as its rows.
    fn inside(y: f32) -> Mark {
        Mark::Line(Rect::new(11.0, y - 1.0, 226.0, 2.0))
    }

    #[test]
    fn a_tab_drag_has_every_kind_of_slot() {
        let (set, strip) = cards();
        let group = |name| set.group_named(name);
        let (api, infra) = (group("api"), group("infra"));
        let infra_header = rows(&set, &strip).len() - 1;
        // `logs`, a member in the middle of `api`.
        let logs = DragItem::Tab(strip[2].id);
        let at = |y| slot(&set, &strip, logs, y).map(|s| (s.index, s.group, s.mark));
        let some = |index, group, mark| Some((index, group, mark));
        // The first position: the gap at the list's top, the line inside the list.
        assert_eq!(at(-100.0), some(0, None, between(35.0)));
        assert_eq!(at(49.0), some(0, None, between(35.0)));
        // Between blocks: the middle of the 6 px gap.
        assert_eq!(at(50.0), some(1, None, between(69.0)));
        assert_eq!(at(89.0), some(1, None, between(69.0)));
        // Inside a card: just under the header, and between the member rows.
        assert_eq!(at(90.0), some(1, api, inside(105.5)));
        assert_eq!(at(121.0), some(1, api, inside(105.5)));
        // Its own place, before and after itself: nothing is drawn.
        for y in [122.0, 150.0, 180.0, 187.0] {
            assert_eq!(at(y), some(2, api, Mark::Stay), "{y}");
        }
        // The end of its card, down to the card's bottom, and the first gap below it.
        assert_eq!(at(188.0), some(3, api, inside(204.5)));
        assert_eq!(at(206.9), some(3, api, inside(204.5)));
        assert_eq!(at(207.0), some(3, None, between(210.0)));
        assert_eq!(at(230.0), some(3, None, between(210.0)));
        // A collapsed header is "into its end"; around it the gaps.
        assert_eq!(at(322.0), some(5, None, between(318.0)));
        for y in [324.0, 339.0, 353.9] {
            assert_eq!(at(y), some(6, infra, Mark::Into(infra_header)), "{y}");
        }
        // The last position.
        for y in [354.0, 380.0, 9000.0] {
            assert_eq!(at(y), some(6, None, between(360.0)), "{y}");
        }

        // A loose tab: its own place from either side, then the gaps and cards below.
        let zsh = DragItem::Tab(strip[0].id);
        let at = |y| slot(&set, &strip, zsh, y).map(|s| (s.index, s.group, s.mark));
        for y in [-5.0, 49.0, 51.0, 89.0] {
            assert_eq!(at(y), some(0, None, Mark::Stay), "{y}");
        }
        assert_eq!(at(95.0), some(0, api, inside(105.5)));
        assert_eq!(at(200.0), some(3, api, inside(204.5)));
        assert_eq!(at(318.0), some(5, None, between(318.0)));
        assert_eq!(at(339.0), some(6, infra, Mark::Into(infra_header)));
    }

    /// The only member of a group stays over its own card's inside and leaves the group in the
    /// gap before it, at the same index.
    #[test]
    fn a_sole_member_leaves_its_group_in_the_gaps_around_its_card() {
        let (set, strip) = tabs(&[("a", ""), ("b", "solo"), ("c", "")]);
        let solo = set.group_named("solo");
        let b = DragItem::Tab(strip[1].id);
        // `a` 34..66; the card 72..141 (header 75..105, row 106..138); `c` 147..179.
        let at = |y| slot(&set, &strip, b, y).map(|s| (s.index, s.group, s.mark));
        for y in [90.0, 110.0, 130.0, 140.0] {
            assert_eq!(at(y), Some((1, solo, Mark::Stay)), "{y}");
        }
        assert_eq!(at(80.0), Some((1, None, between(69.0))));
        assert_eq!(at(150.0), Some((1, None, between(144.0))));
    }

    #[test]
    fn a_card_drag_drops_only_between_blocks() {
        let (set, strip) = cards();
        let shop = DragItem::Group(set.group_named("shop").expect("group"));
        let at = |y| slot(&set, &strip, shop, y).map(|s| (s.index, s.group, s.mark));
        let some = |index, mark| Some((index, None, mark));
        // The first position, and the gap above the first card (its header is "before it").
        assert_eq!(at(-9.0), some(0, between(35.0)));
        assert_eq!(at(49.0), some(0, between(35.0)));
        assert_eq!(at(50.0), some(1, between(69.0)));
        assert_eq!(at(104.9), some(1, between(69.0)));
        // Inside another card's rows (and its bottom padding): refused.
        for y in [105.0, 122.0, 155.0, 206.9] {
            assert_eq!(at(y), None, "{y}");
        }
        // Its own place: the gap above it, itself, and the gap and upper half of the next card.
        for y in [207.0, 214.0, 260.0, 314.0, 318.0, 338.0] {
            assert_eq!(at(y), some(4, Mark::Stay), "{y}");
        }
        // The last position.
        for y in [339.0, 356.0, 380.0, 9000.0] {
            assert_eq!(at(y), some(5, between(360.0)), "{y}");
        }
        // A collapsed card moves by its header too, and goes before another from its header.
        let infra = DragItem::Group(set.group_named("infra").expect("group"));
        let at = |y| slot(&set, &strip, infra, y).map(|s| (s.index, s.group, s.mark));
        assert_eq!(at(230.0), some(4, between(210.0)));
        assert_eq!(at(339.0), some(6, Mark::Stay));
        assert_eq!(at(400.0), some(6, Mark::Stay));
    }

    #[test]
    fn a_drag_of_what_the_list_does_not_show_has_no_slot() {
        let (set, strip) = cards();
        // `deploy` is hidden in its collapsed card; there is no tab 99 and no such group.
        for item in [
            DragItem::Tab(strip[6].id),
            DragItem::Tab(99),
            DragItem::Group(GroupId::NONE),
        ] {
            assert_eq!(slot(&set, &strip, item, 100.0), None, "{item:?}");
        }
        assert_eq!(slot(&TabSet::new(), &[], DragItem::Tab(1), 100.0), None);
    }

    /// splitmix64: reproducible, no dependency.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((z ^ (z >> 31)) % n as u64) as usize
        }
    }

    /// Each tab's id and group, in order.
    fn shape(set: &TabSet<()>) -> Vec<(TabId, Option<GroupId>)> {
        set.ids()
            .iter()
            .map(|id| (*id, set.group_of(*id)))
            .collect()
    }

    /// Random sets, random drags: every slot the function gives is a move `TabSet` makes, the
    /// tab lands where its mark showed, in the slot's group, and nothing else changes.
    #[test]
    fn every_slot_of_a_random_drag_commits_where_its_mark_showed() {
        const TITLES: [&str; 12] = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
        let (mut lines, mut intos, mut stays, mut refused) = (0, 0, 0, 0);
        for seed in 0..3000 {
            let mut rng = Rng(seed);
            let n = 1 + rng.below(TITLES.len());
            // Runs of tabs, each a group or loose.
            let mut picture: Vec<(&str, String)> = Vec::new();
            while picture.len() < n {
                let loose = rng.below(3) == 0;
                let name = format!("g{}", picture.len());
                for _ in 0..=rng.below(4) {
                    if picture.len() < n {
                        let group = if loose { String::new() } else { name.clone() };
                        picture.push((TITLES[picture.len()], group));
                    }
                }
            }
            let picture: Vec<(&str, &str)> =
                picture.iter().map(|(t, g)| (*t, g.as_str())).collect();
            let (mut set, strip) = tabs(&picture);
            let groups: Vec<GroupId> = set.groups().map(|(g, _)| g).collect();
            for g in &groups {
                set.set_collapsed(*g, rng.below(3) == 0);
            }
            set.activate(strip[rng.below(n)].id);

            let list = rows(&set, &strip);
            let layout = layout_list(&list, area());
            // A row or a header of the list.
            let pick = &list[rng.below(list.len())];
            let item = match pick {
                PaneRow::Tab(t) => DragItem::Tab(t.id),
                PaneRow::Group(g) => DragItem::Group(g.id),
            };
            let height = layout.height as usize + 60;
            let y = area().y - 30.0 + rng.below(height * 4) as f32 / 4.0;
            let found = slot_at(&list, &layout, area(), item, y);
            let dragged: Vec<TabId> = match item {
                DragItem::Tab(id) => vec![id],
                DragItem::Group(g) => set.members(g),
            };
            // Where each tab shows: its row's middle, or its collapsed card's header's.
            let shown: Vec<(TabId, f32)> = set
                .ids()
                .into_iter()
                .map(|id| {
                    let row = list
                        .iter()
                        .position(|r| match r {
                            PaneRow::Tab(t) => t.id == id,
                            PaneRow::Group(g) => g.collapsed && set.group_of(id) == Some(g.id),
                        })
                        .expect("every tab shows somewhere");
                    let r = layout.rows[row];
                    (id, r.y + r.h / 2.0)
                })
                .collect();
            let Some(found) = found else {
                // Only a card is refused, and only over another expanded card.
                let DragItem::Group(own) = item else {
                    panic!("seed {seed}: a tab drag was refused at {y}");
                };
                let over = blocks(&list, &layout).into_iter().any(|b| {
                    b.group.is_some_and(|(g, collapsed)| g != own && !collapsed)
                        && y >= layout.rows[b.row].bottom()
                        && y < b.rect.bottom()
                });
                assert!(over, "seed {seed}: refused at {y}");
                refused += 1;
                continue;
            };
            let before = shape(&set);
            let active = set.active();
            assert!(
                commit(&mut set, item, found.index, found.group),
                "seed {seed}: {found:?} refused by the set"
            );
            set.assert_invariants();
            assert_eq!(set.active(), active, "seed {seed}");
            let after = shape(&set);
            let others = |of: &[(TabId, Option<GroupId>)]| -> Vec<_> {
                of.iter()
                    .filter(|(id, _)| !dragged.contains(id))
                    .copied()
                    .collect()
            };
            assert_eq!(others(&after), others(&before), "seed {seed}");
            let moved: Vec<_> = after
                .iter()
                .filter(|(id, _)| dragged.contains(id))
                .copied()
                .collect();
            match item {
                DragItem::Tab(id) => assert_eq!(moved, [(id, found.group)], "seed {seed}"),
                DragItem::Group(g) => {
                    let want: Vec<_> = dragged.iter().map(|id| (*id, Some(g))).collect();
                    assert_eq!(moved, want, "seed {seed}");
                }
            }
            // Together, at the slot's index.
            assert_eq!(
                set.ids().iter().position(|id| *id == dragged[0]),
                Some(found.index),
                "seed {seed}"
            );
            let preceding: Vec<TabId> = set.ids()[..found.index].to_vec();
            let above = |limit: f32| -> Vec<TabId> {
                shown
                    .iter()
                    .filter(|(id, y)| !dragged.contains(id) && *y < limit)
                    .map(|(id, _)| *id)
                    .collect()
            };
            match found.mark {
                Mark::Stay => {
                    assert_eq!(after, before, "seed {seed}");
                    stays += 1;
                }
                Mark::Line(line) => {
                    assert_eq!(preceding, above(line.y + line.h / 2.0), "seed {seed}");
                    assert_ne!(after, before, "seed {seed}: a line for no change");
                    lines += 1;
                }
                Mark::Into(row) => {
                    let header = layout.rows[row];
                    assert_eq!(preceding, above(header.bottom()), "seed {seed}");
                    assert!(matches!(&list[row], PaneRow::Group(g) if Some(g.id) == found.group));
                    intos += 1;
                }
            }
        }
        // The seeds reach every outcome.
        for (what, n) in [
            ("line", lines),
            ("into", intos),
            ("stay", stays),
            ("refused", refused),
        ] {
            assert!(n > 50, "{what}: {n}");
        }
    }

    /// The top bar's drop, for every from and to of a list with two groups: no tab changes its
    /// group, and the active tab stays the same.
    #[test]
    fn the_bars_drop_keeps_every_membership_for_every_pair() {
        let picture = [
            ("a", ""),
            ("b", "x"),
            ("c", "x"),
            ("d", "x"),
            ("e", ""),
            ("f", "y"),
            ("g", "y"),
            ("h", ""),
        ];
        let n = picture.len();
        for from in 0..n {
            for to in 0..n {
                let (mut set, strip) = tabs(&picture);
                set.activate(strip[2].id);
                let mut before = shape(&set);
                set.reorder_bar(from, to);
                set.assert_invariants();
                let mut after = shape(&set);
                assert_eq!(set.active(), Some(strip[2].id), "{from}->{to}");
                before.sort_by_key(|(id, _)| *id);
                after.sort_by_key(|(id, _)| *id);
                assert_eq!(after, before, "{from}->{to}");
            }
        }
    }

    #[test]
    fn the_lifted_copy_stands_clear_of_the_mark() {
        let copy = |y: f32| Rect::new(12.0, y, 224.0, 32.0);
        // A line, and a collapsed card.
        for mark in [
            Rect::new(8.0, 100.0, 232.0, 2.0),
            Rect::new(8.0, 100.0, 232.0, 36.0),
        ] {
            // Not over it: where the pointer has it, even closer than the clear room.
            for y in [20.0, 67.0, mark.y - 32.0, mark.bottom(), 300.0] {
                assert_eq!(clear_of(copy(y), mark), copy(y), "{y}");
            }
            // Over it: the nearer side, 2 px clear.
            assert_eq!(clear_of(copy(mark.y - 31.0), mark), copy(mark.y - 34.0));
            assert_eq!(
                clear_of(copy(mark.bottom() - 1.0), mark),
                copy(mark.bottom() + 2.0)
            );
            // Centred on it: down.
            let centred = mark.y + (mark.h - 32.0) / 2.0;
            assert_eq!(clear_of(copy(centred), mark), copy(mark.bottom() + 2.0));
            // Everywhere: never over it, and never moved further than half of both and the gap.
            for i in 0..600 {
                let at = copy(i as f32 / 2.0 - 50.0);
                let placed = clear_of(at, mark);
                assert!(
                    placed.bottom() <= mark.y || placed.y >= mark.bottom(),
                    "{at:?}"
                );
                let most = (at.h + mark.h) / 2.0 + MARK_CLEAR;
                assert!((placed.y - at.y).abs() <= most, "{at:?} {placed:?}");
            }
        }
    }

    // ------------------------------------------------------------------- the drag, in frames

    fn button(at: (f32, f32), pressed: bool) -> Event {
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        }
    }

    fn moved(at: (f32, f32)) -> Event {
        Event::PointerMoved { pos: at }
    }

    fn escape() -> Event {
        Event::Key {
            key: Key::Named(NamedKey::Escape),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        }
    }

    /// Presses at `from` and moves to `to` with the button held: the events of the frame the
    /// drag is first seen in.
    fn start_drag(
        h: &mut Harness,
        list: &[PaneRow<'_>],
        from: (f32, f32),
        to: (f32, f32),
    ) -> (ListOut, ListOut) {
        h.frame(list, vec![]);
        h.frame(list, vec![moved(from)]);
        let down = h.frame(list, vec![button(from, true)]).0;
        let moved = h.frame(list, vec![moved(to)]).0;
        (down, moved)
    }

    #[test]
    fn a_row_drag_reports_itself_and_drops_once_at_the_release() {
        let (set, strip) = cards();
        let list = rows(&set, &strip);
        let logs = strip[2].id;
        let api = set.group_named("api");
        let mut h = Harness::new();
        let (down, dragging) = start_drag(&mut h, &list, (110.0, 155.0), (110.0, 196.0));
        // The press activates, as without a drag; the drag asks for nothing more.
        assert_eq!(down.events, [PaneEvent::Activate(logs)]);
        assert_eq!(dragging.events, [PaneEvent::Dragging]);
        let (still, _) = h.frame(&list, vec![]);
        assert_eq!(still.events, [PaneEvent::Dragging]);
        let (up, _) = h.frame(&list, vec![button((110.0, 196.0), false)]);
        assert_eq!(
            up.events,
            [PaneEvent::Drop {
                item: DragItem::Tab(logs),
                index: 3,
                group: api,
            }]
        );
        let (after, _) = h.frame(&list, vec![]);
        assert_eq!(after, ListOut::default());
        // A move of less than the 4 px threshold is no drag.
        let (_, small) = start_drag(&mut Harness::new(), &list, (110.0, 155.0), (110.0, 158.0));
        assert!(small.events.is_empty(), "{:?}", small.events);
    }

    #[test]
    fn a_release_in_its_own_place_a_refused_one_or_outside_the_pane_moves_nothing() {
        let (set, strip) = cards();
        let list = rows(&set, &strip);
        // `logs` onto itself; `shop`'s header over `api`'s rows; `logs` over the terminal and
        // above the window.
        for (from, to) in [
            ((110.0, 155.0), (110.0, 130.0)),
            ((110.0, 230.0), (110.0, 155.0)),
            ((110.0, 155.0), (400.0, 80.0)),
            ((110.0, 155.0), (110.0, -20.0)),
        ] {
            let mut h = Harness::new();
            let (_, dragging) = start_drag(&mut h, &list, from, to);
            assert_eq!(dragging.events, [PaneEvent::Dragging], "{to:?}");
            let (up, _) = h.frame(&list, vec![button(to, false)]);
            assert!(up.events.is_empty(), "{to:?}: {:?}", up.events);
        }
        // Out and back in: the drop is made.
        let mut h = Harness::new();
        start_drag(&mut h, &list, (110.0, 155.0), (400.0, 80.0));
        h.frame(&list, vec![moved((110.0, 210.0))]);
        let (up, _) = h.frame(&list, vec![button((110.0, 210.0), false)]);
        assert!(matches!(up.events[..], [PaneEvent::Drop { .. }]));
        // Over the header or the footer: the slot at that edge of the list.
        let mut h = Harness::new();
        start_drag(&mut h, &list, (110.0, 155.0), (110.0, 10.0));
        let (up, _) = h.frame(&list, vec![button((110.0, 10.0), false)]);
        assert!(matches!(
            up.events[..],
            [PaneEvent::Drop {
                index: 0,
                group: None,
                ..
            }]
        ));
    }

    #[test]
    fn escape_or_a_lost_window_ends_the_drag_with_nothing_moved() {
        let (set, strip) = cards();
        let list = rows(&set, &strip);
        let to = (110.0, 210.0);
        // Escape.
        let mut h = Harness::new();
        start_drag(&mut h, &list, (110.0, 155.0), to);
        let (cancelled, _) = h.frame(&list, vec![escape()]);
        assert!(cancelled.events.is_empty(), "{:?}", cancelled.events);
        let (still, _) = h.frame(&list, vec![moved((110.0, 215.0))]);
        assert!(
            still.events.is_empty(),
            "the held button drags nothing more"
        );
        let (up, _) = h.frame(&list, vec![button(to, false)]);
        assert!(up.events.is_empty(), "{:?}", up.events);
        // Escape before the release of the same frame cancels too.
        let mut h = Harness::new();
        start_drag(&mut h, &list, (110.0, 155.0), to);
        let (up, _) = h.frame(&list, vec![escape(), button(to, false)]);
        assert!(up.events.is_empty(), "{:?}", up.events);
        // The window loses the keyboard.
        let mut h = Harness::new();
        start_drag(&mut h, &list, (110.0, 155.0), to);
        h.focused = false;
        let (lost, _) = h.frame(&list, vec![]);
        assert!(lost.events.is_empty(), "{:?}", lost.events);
        h.focused = true;
        let (up, _) = h.frame(&list, vec![button(to, false)]);
        assert!(up.events.is_empty(), "{:?}", up.events);
        // The next drag works.
        let (_, again) = start_drag(&mut h, &list, (110.0, 155.0), to);
        assert_eq!(again.events, [PaneEvent::Dragging]);
    }

    #[test]
    fn a_drag_whose_row_goes_ends_with_nothing_moved() {
        let (mut set, strip) = cards();
        let list = rows(&set, &strip);
        let mut h = Harness::new();
        start_drag(&mut h, &list, (110.0, 155.0), (110.0, 210.0));
        // Its card collapses under it (a program's `ctl`, or a menu's shortcut).
        let api = set.group_named("api").expect("group");
        set.set_collapsed(api, true);
        let collapsed = rows(&set, &strip);
        let (gone, _) = h.frame(&collapsed, vec![]);
        assert!(gone.events.is_empty(), "{:?}", gone.events);
        let (up, _) = h.frame(&collapsed, vec![button((110.0, 210.0), false)]);
        assert!(up.events.is_empty(), "{:?}", up.events);
    }

    #[test]
    fn a_header_drag_moves_the_card_and_toggles_nothing() {
        let (set, strip) = cards();
        let list = rows(&set, &strip);
        let shop = set.group_named("shop").expect("group");
        let mut h = Harness::new();
        let (down, dragging) = start_drag(&mut h, &list, (110.0, 230.0), (110.0, 80.0));
        assert!(down.events.is_empty(), "{:?}", down.events);
        assert_eq!(dragging.events, [PaneEvent::Dragging]);
        let (up, _) = h.frame(&list, vec![button((110.0, 80.0), false)]);
        assert_eq!(
            up.events,
            [PaneEvent::Drop {
                item: DragItem::Group(shop),
                index: 1,
                group: None,
            }]
        );
        // Dropped back where it was: no toggle either.
        let mut h = Harness::new();
        start_drag(&mut h, &list, (110.0, 230.0), (110.0, 260.0));
        let (up, _) = h.frame(&list, vec![button((110.0, 260.0), false)]);
        assert!(up.events.is_empty(), "{:?}", up.events);
        // A drag from the `+` or a row's close button drags nothing.
        for from in [(222.0, 231.0), (222.0, 155.0)] {
            let (_, drag) = start_drag(&mut Harness::new(), &list, from, (110.0, 80.0));
            assert!(drag.events.is_empty(), "{from:?}: {:?}", drag.events);
        }
    }

    /// A list longer than the pane: near the bottom edge a drag scrolls it down, near the top
    /// back up, to the ends and no further; the slot counts the scroll.
    #[test]
    fn a_drag_near_an_edge_scrolls_the_list_and_the_slot_follows() {
        let titles: Vec<String> = (1..=40).map(|i| format!("tab {i}")).collect();
        let picture: Vec<(&str, &str)> = titles.iter().map(|t| (t.as_str(), "")).collect();
        let (set, strip) = tabs(&picture);
        let list = rows(&set, &strip);
        let area = area();
        let height = layout_list(&list, area).height;
        for tier in [MotionTier::Off, MotionTier::Full] {
            let mut h = Harness::new();
            h.tier = tier;
            h.step = Duration::from_millis(16);
            // Where the first row is drawn: how far the list is scrolled.
            let top = |h: &mut Harness| {
                let (out, nodes) = h.frame(&list, vec![]);
                let first = node(&nodes, Role::Tab, "tab 1").bounds.y;
                (out, area.y - first)
            };
            let bottom = (110.0, area.bottom() - 1.0);
            start_drag(&mut h, &list, (110.0, 50.0), (110.0, 300.0));
            assert_eq!(top(&mut h).1, 0.0, "{tier:?}: the middle scrolls nothing");
            h.frame(&list, vec![moved(bottom)]);
            // A second at about 700 px/s.
            for _ in 0..60 {
                top(&mut h);
            }
            let after = top(&mut h).1;
            assert!(after > 300.0, "{tier:?}: {after}");
            for _ in 0..200 {
                top(&mut h);
            }
            let (out, end) = top(&mut h);
            assert!((end - (height - area.h)).abs() < 0.5, "{tier:?}: {end}");
            assert_eq!(out.events, [PaneEvent::Dragging]);
            // Back in the middle it stays.
            h.frame(&list, vec![moved((110.0, 300.0))]);
            top(&mut h);
            let rest = top(&mut h).1;
            assert!((top(&mut h).1 - rest).abs() < 0.01, "{tier:?}");
            // The slot under the pointer counts the scroll: the end of the list.
            h.frame(&list, vec![moved(bottom)]);
            for _ in 0..30 {
                top(&mut h);
            }
            let (up, _) = h.frame(&list, vec![button(bottom, false)]);
            let want = PaneEvent::Drop {
                item: DragItem::Tab(strip[0].id),
                index: 39,
                group: None,
            };
            assert_eq!(up.events, [want], "{tier:?}");
            // And up again, over the header, from a row at the end.
            let (out, nodes) = h.frame(&list, vec![]);
            assert!(out.events.is_empty());
            let row = node(&nodes, Role::Tab, "tab 38").bounds;
            start_drag(&mut h, &list, (110.0, row.y + 16.0), (110.0, 10.0));
            for _ in 0..300 {
                top(&mut h);
            }
            assert!(top(&mut h).1.abs() < 0.5, "{tier:?}");
            let (up, _) = h.frame(&list, vec![button((110.0, 10.0), false)]);
            let want = PaneEvent::Drop {
                item: DragItem::Tab(strip[37].id),
                index: 0,
                group: None,
            };
            assert_eq!(up.events, [want], "{tier:?}");
        }
    }

    /// Outside the pane a drag scrolls nothing.
    #[test]
    fn a_drag_beside_the_pane_scrolls_nothing() {
        let titles: Vec<String> = (1..=40).map(|i| format!("tab {i}")).collect();
        let picture: Vec<(&str, &str)> = titles.iter().map(|t| (t.as_str(), "")).collect();
        let (set, strip) = tabs(&picture);
        let list = rows(&set, &strip);
        let mut h = Harness::new();
        start_drag(&mut h, &list, (110.0, 50.0), (500.0, area().bottom() - 1.0));
        for _ in 0..20 {
            h.frame(&list, vec![]);
        }
        let (_, nodes) = h.frame(&list, vec![]);
        assert_eq!(node(&nodes, Role::Tab, "tab 1").bounds.y, area().y);
    }
}
