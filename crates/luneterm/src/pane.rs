//! The side pane (plan D20-D22, D26-D35): which of the two views a window shows, the pane's
//! rects, the bar view's show button, the pane's own controls (the hide button, the divider of
//! the footer and the width drag at its right edge) and the tab list: loose tabs, group cards and
//! the New Tab button, scrolling between the header and the footer. The window's drag regions,
//! the footer's bell and `⋯` buttons and the terminal card are the shell's ([`crate::shell`]).
//! A tab's progress report shines over its title; the running-command outline is the top bar's
//! only.

use std::f32::consts::{FRAC_PI_2, PI};

use lune_ui::access::{Action, Node, Role, State};
use lune_ui::color::{Rgba, with_alpha};
use lune_ui::icon;
use lune_ui::input::CursorIcon;
use lune_ui::motion::MotionTier;
use lune_ui::paint::{Painter, Rect};
use lune_ui::text::{Spec, Style};
use lune_ui::theme::Theme;
use lune_ui::ui::{Sense, Ui};
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
}

/// What the list reported in a frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListOut {
    pub events: Vec<PaneEvent>,
    /// A row asked for the tab menu.
    pub tab_context: Option<TabContext>,
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
/// what the user asked of it.
pub fn list(ui: &mut Ui<'_>, rects: &PaneRects, list: &PaneList<'_>) -> ListOut {
    let view = viewport(rects);
    let mut out = ListOut::default();
    if view.w <= 0.0 || view.h <= 0.0 {
        return out;
    }
    let layout = layout_list(list.rows, rects.list);
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
    ui.scroll_area(LIST_KEY, view, &options, |ui, _| {
        let id = ui.id("pane-tabs");
        // The strip's name: it is the same list of sessions.
        let node = Node::new(id, Role::TabList, "Terminal sessions", view);
        ui.node_container(node, |ui| {
            let theme = *ui.theme();
            for (color, card) in &layout.cards {
                let [r, g, b] = color.tone(list.look).fill;
                let fill = [r, g, b, 255];
                let p = ui.painter();
                p.fill(*card, CARD_ROUND, with_alpha(fill, CARD_TINT));
                p.ring(*card, CARD_ROUND, 1.0, with_alpha(fill, CARD_RING), false);
            }
            for (row, rect) in list.rows.iter().zip(&layout.rows) {
                match row {
                    PaneRow::Tab(t) => tab_row(ui, &theme, t, *rect, &mut out),
                    PaneRow::Group(g) => header(ui, &theme, g, list.look, *rect, &mut out),
                }
            }
            new_tab_button(ui, &theme, layout.new_tab, &mut out);
        });
        let extent = Extent::new(f64::from(view.w), f64::from(layout.height));
        ((), extent)
    });
    out
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
fn dot(p: &mut Painter<'_, lune_ui::ui::WidgetId>, theme: &Theme, x: f32, row: Rect) {
    let rect = Rect::new(x, row.y + (row.h - DOT) / 2.0, DOT, DOT);
    p.fill(rect, DOT / 2.0, theme.attention());
}

/// A tab's row: the glyph, the title (shining while the tab reports progress), the unread dot,
/// and on the active or hovered row the close button (D33-D35). A press activates the tab.
fn tab_row(ui: &mut Ui<'_>, theme: &Theme, tab: &TabRow<'_>, rect: Rect, out: &mut ListOut) {
    let id = ui.id(("pane-row", tab.id));
    let close_id = ui.id(("pane-close", tab.id));
    let close_rect = Rect::new(
        rect.right() - CLOSE_INSET - CLOSE_SIZE,
        rect.y + (rect.h - CLOSE_SIZE) / 2.0,
        CLOSE_SIZE,
        CLOSE_SIZE,
    );
    let row = ui.interact(id, rect, Sense::CLICK);
    // Registered after its row, so it is on top.
    let close = ui.interact(close_id, close_rect, Sense::CLICK);
    let hovered = row.hovered || close.hovered;
    let shows_close = tab.active || hovered || close.pressed;

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

    let (tier, now) = (ui.tier(), ui.time());
    if tab.progress.is_some() && tier != MotionTier::Off {
        ui.request_busy_frame(rect);
    }
    // A shining title keeps its base colour, so the band shows.
    let bright = tab.active || (tab.unread && tab.progress.is_none());
    let color = if bright {
        theme.fg_bright
    } else {
        theme.fg_secondary
    };
    let indent = if tab.group.is_some() {
        MEMBER_INDENT
    } else {
        LOOSE_INDENT
    };
    let glyph_x = rect.x + indent;
    let title_x = glyph_x + GLYPH + GLYPH_GAP;
    // The close button's room is kept whether it shows or not, so a title never re-elides; its
    // glyph's left edge is where the title (or the dot) stops.
    let close_glyph_x = close_rect.x + (CLOSE_SIZE - CLOSE_GLYPH) / 2.0;
    let dot_x = close_glyph_x - GLYPH_GAP - DOT;
    let title_end = if tab.unread { dot_x } else { close_glyph_x } - GLYPH_GAP;

    let p = ui.painter();
    if tab.active {
        p.fill(rect, ROW_ROUND, theme.surface_selected);
    } else if hovered {
        p.fill(rect, ROW_ROUND, theme.surface_hover);
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
    let close_name = ui.tr("luneui.tab-strip.close");
    if shows_close {
        let state = ButtonState {
            hovered: close.hovered,
            pressed: close.pressed,
            focus_visible: false,
        };
        let button = IconButton {
            glyph: icon::CLOSE,
            glyph_size: CLOSE_GLYPH,
            color: Some(theme.fg_tertiary),
            radius: CLOSE_ROUND,
            name: close_name,
        };
        let look = ButtonLook::of(theme, ButtonKind::Ghost, state);
        paint_icon(ui.painter(), close_rect, &button, &look);
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
        let mut node = Node::new(close_id, Role::Button, close_name, close_rect);
        node.actions.insert(Action::Default);
        ui.node(node);
    }
}

/// A card's header: the chevron, the name in the group's text colour, on a collapsed card with an
/// unread member the dot, the count and the `+` button (D26, D34, D35). A press toggles the
/// card.
fn header(
    ui: &mut Ui<'_>,
    theme: &Theme,
    group: &GroupRow,
    look: LookTheme,
    rect: Rect,
    out: &mut ListOut,
) {
    let id = ui.id(("pane-group", group.id.0));
    let plus = Rect::new(
        rect.right() - PLUS_INSET - PLUS_SIZE,
        rect.y + (rect.h - PLUS_SIZE) / 2.0,
        PLUS_SIZE,
        PLUS_SIZE,
    );
    let header = ui.interact(id, rect, Sense::CLICK);
    let [r, g, b] = group.color.tone(look).text;
    let text = [r, g, b, 255];

    let p = ui.painter();
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
    let count_x = plus.x - HEADER_GAP - p.measure(&count).0;
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
    let button = IconButton {
        glyph: icon::ADD,
        glyph_size: PLUS_GLYPH,
        color: Some(theme.fg_muted),
        radius: PLUS_ROUND,
        name,
    };
    let add = ui.icon_button(("pane-group-add", group.id.0), plus, &button);
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
fn new_tab_button(ui: &mut Ui<'_>, theme: &Theme, rect: Rect, out: &mut ListOut) {
    let id = ui.id("pane-new-tab");
    let button = ui.interact(id, rect, Sense::CLICK);
    let name = ui.tr("menu.newTab");
    let p = ui.painter();
    if button.hovered {
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
    use lune_ui::input::{Event, Modifiers, PointerButton, RawInput, ScrollDelta, keymap};
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
    }

    impl Harness {
        fn new() -> Harness {
            Harness {
                store: UiStore::default(),
                text: crate::fonts::bundled_text_cache(),
                reveal: false,
                time: Duration::ZERO,
            }
        }

        fn frame(&mut self, rows: &[PaneRow<'_>], events: Vec<Event>) -> (ListOut, Vec<Node>) {
            let input = RawInput {
                size: (1040.0, 660.0),
                scale: 1.0,
                time: self.time,
                focused: true,
                events,
            };
            self.time += Duration::from_millis(400);
            let strings = crate::strings::english();
            let shortcuts = Shortcuts::default();
            let config = FrameConfig {
                theme: THEME,
                tier: MotionTier::Off,
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
}
