//! The window shell (plan M9 D15): the glass layers, the title band, the terminal's slot and the
//! scrollbar thumb, plus the borderless-window chrome Windows needs. Everything is drawn through
//! the toolkit's `Ui`, one function for the window and for frame dumps.
//!
//! Layout, logical px, of the top bar view: a 34 px title band, then the card inset 10 px from
//! the left, right and bottom. The 32 px tab strip heads the card (the toolkit draws it and the
//! outline around the card); the terminal's body sits under it, inset 10 px inside the card. In
//! the side pane view ([`crate::pane`]) the pane takes the left column, the card has no strip and
//! the band is only the room above the card. The close-confirm and rename dialogs are drawn here
//! too, on the modal layer.

use std::time::Duration;

use lune_termview::{CellMetrics, GridLayout};
use lune_ui::access::{Action, Node, Role, State};
use lune_ui::chrome::CARD_RADIUS;
use lune_ui::color::{Rgba, premul, with_alpha};
use lune_ui::icon;
use lune_ui::input::{CursorIcon, Event, NamedKey, PointerButton, WindowCommand};
use lune_ui::layout::Axis;
use lune_ui::motion::MotionTier;
use lune_ui::paint::Rect;
use lune_ui::scene::LayerId;
use lune_ui::text::{Spec, Style};
use lune_ui::ui::{Dismissal, Sense, Ui};
use lune_ui::widgets::button::{IconButton, IconButtonExtras};
use lune_ui::widgets::caption::{CaptionButton, CaptionOptions};
use lune_ui::widgets::dialog::{Cancel, Dialog, DialogGlass, DialogOptions};
use lune_ui::widgets::field::FieldOptions;
use lune_ui::widgets::message_list::{self, HEADER_HEIGHT, MessageListOptions, MessageRow};
use lune_ui::widgets::popover::{PlaceAlign, PopoverOptions, Side};
use lune_ui::widgets::scrollbar::{self, ThumbGeometry, thumb_geometry};
use lune_ui::widgets::tab_strip::{
    self, BusyCue, Tab as StripItem, TabEvent, TabKey, TabStripExtras, TabStripOptions,
};
use lune_ui::widgets::toast::{
    ToastAction, ToastExtras, ToastKind, ToastResponse, ToastSpec, ToastStackOptions, Toasts,
};
use lune_ui::widgets::tone::Tone;
use lune_ui::window::{ResizeEdge, edge_at};

use crate::groups::{self, GroupColor, NameProblem};
use crate::notices::{self, Age, Inbox, NoticeKind};
use crate::pane::{self, PaneEdge, PaneRects};
use crate::settings::Theme as LookTheme;

/// The title band's height.
pub const TITLE_BAND: f32 = 34.0;
/// The gap between the window and the card (left, right and bottom).
pub const CARD_MARGIN: f32 = 10.0;
/// The gap between the card and the terminal body.
pub const BODY_PAD: f32 = 10.0;
/// The scrollbar thumb's opacity (accent colour).
pub const THUMB_ALPHA: f32 = 0.55;
/// The slot of the active tab's terminal in the frame's custom-draw list; grids fading out in a
/// tab-switch crossfade are the slots after it.
pub const TERM_SLOT: usize = 0;
/// The strip's height: the toolkit's.
pub const STRIP_HEIGHT: f32 = tab_strip::HEIGHT;
/// The strip stops this far short of the card's right edge: the room of the `⋯` button.
pub const STRIP_RESERVE: f32 = 40.0;
/// The `⋯` button: 24 px square, 32 px from the card's right edge and 4 px under its top.
pub const OVERFLOW_SIZE: f32 = 24.0;
/// The gap between the inbox button and the `⋯` button on its right.
pub const INBOX_GAP: f32 = 4.0;
/// The inbox list's width.
pub const INBOX_WIDTH: f32 = 360.0;
/// The most height of the inbox list (header included); the body's height caps it too.
pub const INBOX_MAX_HEIGHT: f32 = 480.0;
/// The inbox button's glyph (the bell, as on a notification toast).
pub const INBOX_GLYPH: &str = "\u{eaa2}";
/// How long the bell rings when something new arrives (D19).
pub const RING_LEN: Duration = Duration::from_millis(1000);
/// A ring starts only if the last one began this long ago or more: a program that rings in a
/// loop makes at most `RING_LEN` of frames per `RING_GAP`.
pub const RING_GAP: Duration = Duration::from_millis(3000);
/// The swing's first peak, degrees (clockwise positive); the swing shrinks to still over `RING_LEN`.
const RING_PEAK_DEGREES: f32 = 14.0;
/// Swings (full back-and-forth cycles) in one ring.
const RING_SWINGS: f32 = 3.0;
/// Half the bell glyph's size: the swing turns about the bell's top, this far above its centre.
const RING_PIVOT: f32 = 8.0;
/// The header's darker band, punched out by the tab outline (luneui-rs D131: the gallery's value;
/// the header layer is a material of the app's preset, so the app passes it). Lunegit's look; the
/// near-black look passes its own (`render::header_tint`).
pub const HEADER_TINT: [u8; 4] = [0x00, 0x00, 0x00, 0x24];
/// The header's wash under the near-black look: a 6% white wash (luneui-rs's own `near_black::HEADER_TINT`
/// is 4% and leaves the header 6 levels or less from the title band over a dark backdrop, 3 at
/// `#30`, and the two cross over a mid one; the tint is the app's, D131). Director, step 13 round 1:
/// band against header 12 / 8 / 3 over `#10` / `#30` / `#60`, header against card about 14.
pub const NEAR_BLACK_HEADER_TINT: [u8; 4] = [0xFF, 0xFF, 0xFF, 0x10];

/// What differs per OS in the chrome: whether the app draws the caption buttons (a borderless
/// window) and how far the centred title keeps clear of the native or drawn buttons.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chrome {
    /// The window has no native frame: draw caption buttons, drag by the band, resize by the edges.
    pub drawn_caption: bool,
    /// The title is clipped this far from both sides of the window.
    pub title_inset: f32,
    /// A double press on the band is the app's to answer (the OS never sees it): maximize on a
    /// borderless window, the System Settings choice on macOS ([`crate::double_click`]).
    pub band_double_click: bool,
    /// The OS draws its window buttons over the band's left end (macOS): the app's own title-area
    /// buttons keep clear of them and share their centre line.
    pub native_lights: bool,
}

/// Windows: a borderless window with drawn caption buttons.
pub const WINDOWS_CHROME: Chrome = Chrome {
    drawn_caption: true,
    // Three 46 px buttons and a margin.
    title_inset: 3.0 * 46.0 + 8.0,
    band_double_click: true,
    native_lights: false,
};

/// macOS keeps its traffic lights (the band is drawn under them).
pub const MACOS_CHROME: Chrome = Chrome {
    drawn_caption: false,
    title_inset: 90.0,
    band_double_click: true,
    native_lights: true,
};

/// Linux keeps the native decorations.
pub const LINUX_CHROME: Chrome = Chrome {
    drawn_caption: false,
    title_inset: 90.0,
    band_double_click: false,
    native_lights: false,
};

/// The chrome of this OS.
#[cfg(target_os = "windows")]
pub const HOST: Chrome = WINDOWS_CHROME;
/// The chrome of this OS.
#[cfg(target_os = "macos")]
pub const HOST: Chrome = MACOS_CHROME;
/// The chrome of this OS.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const HOST: Chrome = LINUX_CHROME;

/// Whether this OS gets an OS glass material at all (Linux has none: an opaque theme instead).
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub const HOST_GLASS: bool = true;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const HOST_GLASS: bool = false;

/// Which of the two views a frame draws (D20): the top bar (today's band and tab strip) or the
/// side pane.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum View {
    #[default]
    Bar,
    /// The pane is `width` logical px wide. With `drawn_caption` the app draws the caption
    /// buttons, so a title band over the terminal column keeps them off the terminal (D27).
    Pane { width: f32, drawn_caption: bool },
}

/// The rects of a window of a given size, logical px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub window: Rect,
    /// The title band. In the pane view it is the room above the card, right of the pane: a press
    /// there drags the window.
    pub band: Rect,
    pub card: Rect,
    /// The tab strip, at the card's top (its right end stays free, see [`STRIP_RESERVE`]); empty
    /// in the pane view.
    pub strip: Rect,
    /// The `⋯` button: in the strip's reserved right end, or at the pane's footer.
    pub overflow: Rect,
    /// The inbox button; drawn only while the inbox has entries. In the bar view it is left of
    /// `⋯` ([`INBOX_GAP`] between them) and the strip is narrower by its width plus the gap; in
    /// the pane view it is at the footer's left.
    pub inbox: Rect,
    pub body: Rect,
    /// The side pane's rects; `None` in the bar view.
    pub pane: Option<PaneRects>,
}

impl Layout {
    /// The bar view's layout of the window `window` (its origin is the window's) with no inbox
    /// button.
    pub fn new(window: Rect) -> Layout {
        Layout::with_inbox(window, false)
    }

    /// The layout of `window` in `view`; `inbox_shown` is whether the inbox button is drawn.
    pub fn for_view(window: Rect, inbox_shown: bool, view: View) -> Layout {
        match view {
            View::Bar => Layout::with_inbox(window, inbox_shown),
            View::Pane {
                width,
                drawn_caption,
            } => Layout::with_pane(window, width, drawn_caption),
        }
    }

    /// The pane view's layout (D26, D27): the pane at the left, the card from its right edge, no
    /// strip.
    fn with_pane(window: Rect, width: f32, drawn_caption: bool) -> Layout {
        let pane = PaneRects::new(window, width);
        let top = if drawn_caption {
            TITLE_BAND
        } else {
            CARD_MARGIN
        };
        let left = pane.rect.right();
        let card = Rect::new(
            left,
            window.y + top,
            (window.right() - left - CARD_MARGIN).max(0.0),
            (window.h - top - CARD_MARGIN).max(0.0),
        );
        let body = Rect::new(
            card.x + BODY_PAD,
            card.y + BODY_PAD,
            (card.w - 2.0 * BODY_PAD).max(0.0),
            (card.h - 2.0 * BODY_PAD).max(0.0),
        );
        Layout {
            window,
            band: Rect::new(
                left,
                window.y,
                (window.right() - left).max(0.0),
                top.min(window.h),
            ),
            card,
            strip: Rect::new(card.x, card.y, 0.0, 0.0),
            overflow: pane.menu,
            inbox: pane.bell,
            body,
            pane: Some(pane),
        }
    }

    /// The bar view's layout with the inbox button shown or not. Only the strip changes: the
    /// button sits in the strip's right end and the body is the same, so the terminal never
    /// resizes for it.
    pub fn with_inbox(window: Rect, inbox_shown: bool) -> Layout {
        let card = Rect::new(
            window.x + CARD_MARGIN,
            window.y + TITLE_BAND,
            (window.w - 2.0 * CARD_MARGIN).max(0.0),
            (window.h - TITLE_BAND - CARD_MARGIN).max(0.0),
        );
        let strip = Rect::new(
            card.x,
            card.y,
            (card.w
                - STRIP_RESERVE
                - if inbox_shown {
                    OVERFLOW_SIZE + INBOX_GAP
                } else {
                    0.0
                })
            .max(0.0),
            STRIP_HEIGHT.min(card.h),
        );
        let overflow = Rect::new(
            card.right() - 32.0,
            card.y + 4.0,
            OVERFLOW_SIZE,
            OVERFLOW_SIZE,
        );
        let body = Rect::new(
            card.x + BODY_PAD,
            card.y + STRIP_HEIGHT + BODY_PAD,
            (card.w - 2.0 * BODY_PAD).max(0.0),
            (card.h - STRIP_HEIGHT - 2.0 * BODY_PAD).max(0.0),
        );
        Layout {
            window,
            band: Rect::new(window.x, window.y, window.w, TITLE_BAND.min(window.h)),
            card,
            strip,
            overflow,
            inbox: Rect::new(
                overflow.x - INBOX_GAP - OVERFLOW_SIZE,
                overflow.y,
                OVERFLOW_SIZE,
                OVERFLOW_SIZE,
            ),
            body,
            pane: None,
        }
    }
}

/// Where the cell grid sits in the body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridPlacement {
    /// The grid's top-left, logical px, snapped to a whole device pixel.
    pub origin: (f32, f32),
    pub cols: usize,
    pub rows: usize,
}

/// The grid that fits `body` for cells of `metrics` (its `scale` is the display's), anchored at the
/// body's top-left on a device pixel.
pub fn place_grid(body: Rect, metrics: CellMetrics) -> GridPlacement {
    let layout = GridLayout::new((body.x, body.y), metrics);
    let scale = if metrics.scale.is_finite() && metrics.scale > 0.0 {
        metrics.scale
    } else {
        1.0
    };
    let (cols, rows) = layout.grid_size(body.w, body.h);
    GridPlacement {
        origin: (
            layout.origin_px.0 as f32 / scale,
            layout.origin_px.1 as f32 / scale,
        ),
        cols,
        rows,
    }
}

/// The scroll state the thumb shows, in lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    pub viewport_top: usize,
    pub base_y: usize,
    pub total_lines: usize,
}

/// The scrollbar of the body: `None` while there is no scrollback (`base_y == 0`) or the numbers
/// do not make a bar. `cell_h` is one line in logical px.
pub fn scroll_geometry(body: Rect, cell_h: f32, scroll: ScrollView) -> Option<ThumbGeometry> {
    if scroll.base_y == 0 {
        return None;
    }
    let line = f64::from(cell_h);
    thumb_geometry(
        Axis::Vertical,
        body,
        scroll.viewport_top as f64 * line,
        scroll.total_lines as f64 * line,
        false,
    )
}

/// The thumb's rect in the body, see [`scroll_geometry`].
pub fn scroll_thumb(body: Rect, cell_h: f32, scroll: ScrollView) -> Option<Rect> {
    scroll_geometry(body, cell_h, scroll).map(|g| g.thumb)
}

/// How far outside the 6 px thumb a press still grabs it (the track is 9 px wide).
const THUMB_GRAB: f32 = 3.0;

/// Whether a press at `pos` (logical px) grabs the thumb.
pub fn thumb_hit(body: Rect, cell_h: f32, scroll: ScrollView, pos: (f32, f32)) -> bool {
    scroll_thumb(body, cell_h, scroll).is_some_and(|t| t.inset(-THUMB_GRAB, 0.0).contains(pos))
}

/// A drag of the scrollbar thumb: pointer movement becomes whole lines to scroll, the rest of a
/// line carried to the next move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThumbDrag {
    last_y: f32,
    rest: f64,
}

impl ThumbDrag {
    /// A drag that started with the pointer at `y` (logical px).
    pub fn start(y: f32) -> ThumbDrag {
        ThumbDrag {
            last_y: y,
            rest: 0.0,
        }
    }

    /// The pointer is now at `y`. Returns the lines to scroll, positive toward newer output, using
    /// the thumb's current geometry (a bar that vanished gives 0).
    pub fn moved(&mut self, y: f32, body: Rect, cell_h: f32, scroll: ScrollView) -> i64 {
        let dy = f64::from(y - self.last_y);
        self.last_y = y;
        let line = f64::from(cell_h);
        let Some(g) = scroll_geometry(body, cell_h, scroll) else {
            return 0;
        };
        if !(line.is_finite() && line > 0.0 && dy.is_finite()) {
            return 0;
        }
        let lines = self.rest + g.offset_delta(dy) / line;
        let whole = lines.trunc();
        self.rest = lines - whole;
        // The offset range is finite, so this is far inside i64.
        whole as i64
    }
}

/// One tab of the strip: the app's id and what the strip shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripTab<'a> {
    pub id: u64,
    pub title: &'a str,
    /// An OSC 9;4 report is active in this tab: its title shines in the tone.
    pub progress: Option<Tone>,
    /// The tab has an unread notice and is not the active one: luneui draws its attention cue (a
    /// dot before the title, a bright title). Independent of `progress`: a tab may have both.
    pub attention: bool,
}

/// The title of the question before a running session is closed (Lunegit's copy).
pub const CLOSE_SESSION_TITLE: &str = "Close Terminal Session";

/// The dialog the frame shows over everything, if any. The text and the colour are the app's
/// state: the dialog edits them in place.
pub enum Modal<'a> {
    /// A close question: its title and message, and the "Don't ask again" box when the answer may
    /// be remembered.
    Confirm {
        title: &'a str,
        message: &'a str,
        dont_ask: Option<&'a mut bool>,
    },
    /// Rename Tab: the field's text. `focus` asks to put the caret in the field (once, a frame
    /// after the dialog opened: the open focus goes to the confirm button).
    Rename { text: &'a mut String, focus: bool },
    /// New Group (`editing` false) or Edit Group: the name's field and the nine colours.
    GroupEditor {
        editing: bool,
        text: &'a mut String,
        color: &'a mut GroupColor,
        /// The names of the other open groups: the field's name may be none of them.
        taken: &'a [String],
        /// The look the colours are drawn for.
        look: LookTheme,
        focus: bool,
    },
}

impl std::fmt::Debug for Modal<'_> {
    /// The variant only: its texts hold tab and group names, which a program can choose.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Modal::Confirm { .. } => "Confirm",
            Modal::Rename { .. } => "Rename",
            Modal::GroupEditor { .. } => "GroupEditor",
        })
    }
}

/// A colour swatch of the group editor: its diameter, the gap between two, and how far the ring
/// of the chosen one reaches outside it (a 2 px gap, then the 2 px ring).
pub const SWATCH: f32 = 24.0;
pub const SWATCH_GAP: f32 = 14.0;
pub const SWATCH_RING: f32 = 4.0;
/// The width of the chosen swatch's ring.
const SWATCH_RING_WIDTH: f32 = 2.0;
/// The editor's parts, below the body's top: the hint line under the field (its room is always
/// kept), the colour label, and the top of the swatches.
const EDITOR_HINT_Y: f32 = 74.0;
const EDITOR_COLOR_LABEL_Y: f32 = 100.0;
const EDITOR_SWATCH_Y: f32 = 122.0;
/// The editor's body height: its footer gap under the swatches' rings is the rename dialog's
/// under its field.
const EDITOR_EXTRA_H: f32 = 142.0;

/// The circle of the `i`-th swatch in a row whose left end and top are `x` and `y`.
pub fn swatch_rect(i: usize, x: f32, y: f32) -> Rect {
    Rect::new(x + i as f32 * (SWATCH + SWATCH_GAP), y, SWATCH, SWATCH)
}

/// The words under the editor's field for `problem`; `None` when there is nothing to say.
fn name_hint(problem: Option<NameProblem>) -> Option<&'static str> {
    match problem? {
        NameProblem::Empty => None,
        NameProblem::Taken => Some("group.editor.taken"),
        NameProblem::TooLong => Some("group.editor.tooLong"),
        NameProblem::EdgeSpace => Some("group.editor.edgeSpace"),
        NameProblem::Control => Some("group.editor.control"),
    }
}

/// The string key of a colour's accessible name.
fn color_name_key(color: GroupColor) -> String {
    format!("group.color.{}", color.as_str())
}

/// Every string key the group dialogs ask for.
pub fn group_required_keys() -> Vec<String> {
    [
        "group.defaultName",
        "group.editor.newTitle",
        "group.editor.editTitle",
        "group.editor.message",
        "group.editor.nameLabel",
        "group.editor.name",
        "group.editor.colorLabel",
        "group.editor.create",
        "group.editor.save",
        "group.editor.taken",
        "group.editor.tooLong",
        "group.editor.edgeSpace",
        "group.editor.control",
        "group.close.title",
        "group.close.confirm",
        "group.close.message",
        "group.close.messageOne",
    ]
    .iter()
    .map(|k| (*k).to_owned())
    .chain(GroupColor::ALL.into_iter().map(color_name_key))
    .collect()
}

/// The question before the group `name` and its `tabs` tabs are closed.
pub fn close_group_message(
    strings: &lune_ui::strings::StringTable,
    name: &str,
    tabs: usize,
) -> String {
    use lune_ui::strings::DisplayFormat;
    if tabs == 1 {
        strings.tr_fmt("group.close.messageOne", &[&name], &DisplayFormat)
    } else {
        strings.tr_fmt("group.close.message", &[&name, &tabs], &DisplayFormat)
    }
}

/// A notification toast's `Go to tab` button: the number its action reports.
pub const TOAST_GO_TO_TAB: u64 = 1;
/// A notification toast's `Dismiss` button.
pub const TOAST_DISMISS: u64 = 2;
/// The gap between the toast stack and the card's right and bottom edges (the mockup's 22 px).
pub const TOAST_MARGIN: f32 = 22.0;
/// The bell glyph of a notification toast.
pub const TOAST_GLYPH: &str = "\u{eaa2}";

/// The toolkit toast of a program notification (D16): the header names the tab (`source`, the
/// tab's title now) with the age `now_text`; `title` (when not empty) is the bold line; `body` is
/// the message. Two buttons, `Dismiss` (plain) then `Go to tab` (primary); it lives `life` with
/// the drain line, and its hold on hover is the toolkit's. The one place a notification toast is
/// built, for the app and for the dump scenes. The texts are the sanitised ones.
pub fn notice_toast(
    source: &str,
    title: &str,
    body: &str,
    texts: &NoticeTexts<'_>,
    accent: Rgba,
    life: Duration,
) -> (ToastSpec, ToastExtras) {
    let mut spec = ToastSpec::new(ToastKind::Info, body);
    spec.actions = vec![
        ToastAction {
            label: texts.dismiss.to_owned(),
            primary: false,
            id: TOAST_DISMISS,
        },
        ToastAction {
            label: texts.go_to_tab.to_owned(),
            primary: true,
            id: TOAST_GO_TO_TAB,
        },
    ];
    let mut extras = ToastExtras::new()
        .with_source(source)
        .with_age(texts.now)
        .with_glyph(TOAST_GLYPH)
        .with_accent(accent)
        .with_timeout(life)
        .with_drain(true);
    if !title.is_empty() {
        extras = extras.with_title(title);
    }
    (spec, extras)
}

/// An update toast's `Update` button (the one that starts the download).
pub const TOAST_UPDATE: u64 = 3;
/// An update toast's `Restart to update` button.
pub const TOAST_RESTART: u64 = 4;
/// How long an update toast lives: longer than a program's notification, it asks a decision.
pub const UPDATE_TOAST_LIFE: Duration = Duration::from_secs(20);

/// Which of the two update toasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateToastKind {
    /// A new version was found: `Dismiss` and `Update`.
    Available,
    /// The new version is downloaded and staged: `Dismiss` and `Restart to update`.
    Ready,
}

/// The toolkit toast of the update flow (D7): no tab, so no source tab name; the source is the
/// app's. `version` is the dotted version text.
pub fn update_toast(
    kind: UpdateToastKind,
    version: &str,
    strings: &lune_ui::strings::StringTable,
    accent: Rgba,
) -> (ToastSpec, ToastExtras) {
    use lune_ui::strings::DisplayFormat;
    let (title, body, label, action) = match kind {
        UpdateToastKind::Available => (
            "toast.update.title",
            "toast.update.body",
            "toast.update.action",
            TOAST_UPDATE,
        ),
        UpdateToastKind::Ready => (
            "toast.update.readyTitle",
            "toast.update.readyBody",
            "toast.update.restart",
            TOAST_RESTART,
        ),
    };
    let mut spec = ToastSpec::new(
        ToastKind::Info,
        strings.tr_fmt(body, &[&version], &DisplayFormat),
    );
    spec.actions = vec![
        ToastAction {
            label: strings.tr("toast.dismiss").to_owned(),
            primary: false,
            id: TOAST_DISMISS,
        },
        ToastAction {
            label: strings.tr(label).to_owned(),
            primary: true,
            id: action,
        },
    ];
    let extras = ToastExtras::new()
        .with_source(strings.tr("toast.update.source"))
        .with_age(strings.tr("toast.now"))
        .with_title(strings.tr(title))
        .with_glyph(icon::ARROW_DOWN)
        .with_accent(accent)
        .with_timeout(UPDATE_TOAST_LIFE)
        .with_drain(true);
    (spec, extras)
}

/// The words of a notification toast, from the string table.
#[derive(Debug, Clone, Copy)]
pub struct NoticeTexts<'a> {
    pub go_to_tab: &'a str,
    pub dismiss: &'a str,
    pub now: &'a str,
}

impl<'a> NoticeTexts<'a> {
    pub fn of(strings: &'a lune_ui::strings::StringTable) -> Self {
        NoticeTexts {
            go_to_tab: strings.tr("toast.goToTab"),
            dismiss: strings.tr("toast.dismiss"),
            now: strings.tr("toast.now"),
        }
    }
}

/// What the shell draws a frame from.
#[derive(Debug)]
pub struct ShellView<'a> {
    /// The active tab's own label (name or program title); `None` shows the app's name alone.
    pub title: Option<&'a str>,
    pub maximized: bool,
    /// The window has keyboard focus (inactive caption glyphs are dimmer).
    pub focused: bool,
    /// The terminal's scroll state, for the thumb.
    pub scroll: Option<ScrollView>,
    /// One terminal line in logical px.
    pub cell_h: f32,
    pub chrome: Chrome,
    /// The view the frame is laid out in.
    pub view: View,
    /// The top bar draws its show-pane button ([`crate::pane::offers_show_button`]).
    pub show_pane_button: bool,
    /// The accessible name of the button the view has: show the side pane, or hide it.
    pub pane_button_name: &'a str,
    pub tabs: &'a [StripTab<'a>],
    pub active_tab: Option<u64>,
    pub modal: Option<Modal<'a>>,
    /// The app's toasts, drawn last, over the dialog.
    pub toasts: Option<&'a mut Toasts>,
    /// The settings page covers the card: no terminal, no thumb, no strip and no `⋯`.
    pub page_open: bool,
    /// The cue the active tab's outline runs, `None` when nothing runs (plan M10 step 6: the
    /// settings decide the cue, the tab decides whether it is busy).
    pub busy: Option<BusyCue>,
    /// The outline's colour while it runs: the active tab's progress tone (error, warning), else
    /// neutral.
    pub busy_tone: Tone,
    /// The accessible name of the `⋯` button.
    pub overflow_name: &'a str,
    /// The inbox button: `Some` while the inbox has entries. The bar view draws it left of `⋯`
    /// and only while no page is open; the pane view always draws it, in the footer.
    pub inbox_button: Option<InboxButton<'a>>,
    /// The wash over the card that makes the tab header: the look's own (`render::header_tint`).
    pub header_tint: [u8; 4],
    /// Grids fading out under the terminal's (a tab-switch crossfade): custom slots
    /// `TERM_SLOT + 1 ..= TERM_SLOT + fading_slots`, drawn first so the active grid is over them.
    pub fading_slots: usize,
}

/// The inbox button of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxButton<'a> {
    /// The accessible name: "Notifications", and the unread count when there is one.
    pub name: &'a str,
    /// Something is unread: the bell is drawn in the attention colour; at rest (all read) it has
    /// the look of the `⋯` button's glyph.
    pub unread: bool,
    /// The start of the bell's ring (the app's clock) while one is running, else `None`. The
    /// shell draws it only under `MotionTier::Full`, and only while `unread`.
    pub ring: Option<Duration>,
    /// The list is open: the button rests on its hover fill.
    pub open: bool,
}

/// The bell's angle `elapsed` after a ring began, radians (clockwise positive): three swings
/// shrinking to still over [`RING_LEN`]; `None` once the ring is over.
pub fn ring_angle(elapsed: Duration) -> Option<f32> {
    if elapsed >= RING_LEN {
        return None;
    }
    let t = elapsed.as_secs_f32() / RING_LEN.as_secs_f32();
    Some(
        RING_PEAK_DEGREES.to_radians()
            * (1.0 - t)
            * (std::f32::consts::TAU * RING_SWINGS * t).sin(),
    )
}

/// Whether a ring may start at `now`: no ring before, or the last one began [`RING_GAP`] ago or more.
pub fn ring_due(last_start: Option<Duration>, now: Duration) -> bool {
    last_start.is_none_or(|last| now.saturating_sub(last) >= RING_GAP)
}

/// The inbox button's accessible name: `name`, and the unread count when there is one.
pub fn inbox_button_name(name: &str, unread: usize) -> String {
    if unread == 0 {
        name.to_owned()
    } else {
        format!("{name} ({unread})")
    }
}

/// What the dialog of a frame asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModalOut {
    pub confirmed: bool,
    pub cancelled: bool,
}

/// What a drawn frame hands back.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellOut {
    /// The terminal body, logical px.
    pub body: Rect,
    /// Window operations the chrome asks for this frame, in order.
    pub commands: Vec<WindowCommand>,
    /// What the user asked of the tabs this frame; the app applies them.
    pub tab_events: Vec<TabEvent>,
    /// The dialog's answer; `None` while none is shown.
    pub modal: Option<ModalOut>,
    /// Where the dialog's text field edits, for the IME; `None` without one.
    pub ime_area: Option<Rect>,
    /// The `⋯` button was pressed: its rect, to anchor the app menu to.
    pub overflow: Option<Rect>,
    /// The inbox button was pressed this frame.
    pub inbox_clicked: bool,
    /// A tab asked for its context menu (a right-click or the assistive "show menu" action).
    pub tab_context: Option<TabContext>,
    /// What the toasts reported: the button pressed (the toast leaves with it).
    pub toast: ToastResponse,
    /// The show or hide button of the side pane was pressed.
    pub pane_toggle: bool,
    /// What the pane's edge zone saw; `None` in the bar view.
    pub pane_edge: Option<PaneEdge>,
}

/// A tab's request for its context menu: the app opens the menu (the strip owns none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabContext {
    /// The tab the menu is for.
    pub id: u64,
    /// Where the menu's top left goes, logical px: the pointer when it is over the tab, else the
    /// tab's bottom-left (a request that did not come from a pointer).
    pub at: (f32, f32),
}

/// Whether the tabs can be closed (the strip's Close button and the tab menu's Close row).
pub const TAB_CLOSABLE: bool = true;

/// The cursor for resizing from `edge`.
pub fn resize_cursor(edge: ResizeEdge) -> CursorIcon {
    match edge {
        ResizeEdge::North | ResizeEdge::South => CursorIcon::NsResize,
        ResizeEdge::East | ResizeEdge::West => CursorIcon::EwResize,
        ResizeEdge::NorthEast | ResizeEdge::SouthWest => CursorIcon::NeswResize,
        ResizeEdge::NorthWest | ResizeEdge::SouthEast => CursorIcon::NwseResize,
    }
}

/// The edge a primary press of this frame landed on, for a window of `size` (logical).
pub fn edge_press(events: &[Event], size: (f32, f32)) -> Option<ResizeEdge> {
    events.iter().find_map(|e| match e {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            ..
        } => edge_at(*pos, size.0, size.1),
        _ => None,
    })
}

/// The window command of a caption button.
pub fn caption_command(button: CaptionButton) -> WindowCommand {
    match button {
        CaptionButton::Minimize => WindowCommand::Minimize,
        CaptionButton::Maximize => WindowCommand::ToggleMaximize,
        CaptionButton::Close => WindowCommand::Close,
    }
}

/// What a press in the title band does: a double click (the second press; a third drags like a single one) runs `double` (where the app answers it:
/// [`Chrome::band_double_click`]; `None` = the OS setting says do nothing), any other press drags
/// the window.
pub fn band_command(
    press_count: u8,
    chrome: Chrome,
    double: impl FnOnce() -> Option<WindowCommand>,
) -> Option<WindowCommand> {
    if press_count == 2 && chrome.band_double_click {
        double()
    } else {
        Some(WindowCommand::DragMove)
    }
}

/// The band's title: `<tab>  ·  Luneterm` in two runs when the tab has a label of its own, the
/// app's name alone otherwise.
fn band_title(label: Option<&str>, theme: &lune_ui::theme::Theme) -> Spec {
    match label {
        Some(name) => Spec {
            runs: vec![
                (
                    name.to_owned(),
                    Style::ui(12.0, theme.fg_secondary).medium(),
                ),
                (
                    "  \u{b7}  Luneterm".to_owned(),
                    Style::ui(12.0, theme.fg_tertiary),
                ),
            ],
            wrap: None,
        },
        None => Spec::plain("Luneterm", Style::ui(12.0, theme.fg_secondary).medium()),
    }
}

/// Draws the shell into `ui` and returns the terminal body, the chrome's window commands, the
/// tab events and the dialog's answer.
///
/// The glass layers go on the fixed base layer (panel over the window, card on top); the
/// terminal's custom slot sits in the body, below the thumb. In the bar view the strip heads the
/// card and its outline wraps it; in the pane view the pane stands left of the card and its footer
/// holds the inbox and `⋯` buttons. The dialogs come last, on the modal layer.
pub fn draw(ui: &mut Ui<'_>, view: ShellView<'_>) -> ShellOut {
    let theme = *ui.theme();
    let win = ui.painter().window;
    // The bar hides its strip buttons under a page; the pane's footer stays (D31).
    let inbox_shown = view.inbox_button.is_some() && (!view.page_open || view.view != View::Bar);
    let layout = Layout::for_view(win, inbox_shown, view.view);
    let show_pane_button = layout.pane.is_none() && view.show_pane_button;
    {
        let p = ui.painter();
        p.on(LayerId::Base);
        let panel = theme.glass.panel.premultiplied();
        let card = theme.glass.card.premultiplied();
        p.acrylic(win, 0.0, panel, [0.0; 4], true);
        p.acrylic(layout.card, CARD_RADIUS, card, panel, true);

        match &layout.pane {
            // The pane shows the tabs, so no title (D27).
            Some(pane) => pane::paint(p, pane, &theme),
            None => {
                // The title, centred in the band and kept clear of the window buttons.
                let mut side = view.chrome.title_inset;
                if show_pane_button {
                    side = side.max(pane::show_button(win, view.chrome).right() + 4.0);
                }
                let spec = band_title(view.title, &theme);
                p.set_clip(Some(Rect::new(
                    side,
                    0.0,
                    (win.w - 2.0 * side).max(0.0),
                    TITLE_BAND,
                )));
                // Optically centred: the line box's centre is 2 pt above the cap block's (room
                // for descenders), so the box moves down 2 pt. Measured on a 2x screenshot: caps
                // 9 pt from the band's top and 13.5 pt from the card before; with descenders the
                // lower gap filled in.
                p.text_centered(&spec, Rect::new(0.0, 2.0, win.w, TITLE_BAND - 2.0));
                p.set_clip(None);
            }
        }

        // With no strip to draw it, the card's outline is the app's: the strip's colours (a
        // dialog or an unfocused window takes the subtle one). The page draws its own.
        if layout.pane.is_some() && !view.page_open {
            let color = if view.focused && view.modal.is_none() {
                theme.border
            } else {
                theme.border_subtle
            };
            p.ring(layout.card, CARD_RADIUS, 1.0, color, false);
        }

        if !view.page_open {
            for slot in 1..=view.fading_slots {
                p.custom(TERM_SLOT + slot, layout.body);
            }
            p.custom(TERM_SLOT, layout.body);
            if let Some(thumb) = view
                .scroll
                .and_then(|s| scroll_thumb(layout.body, view.cell_h, s))
            {
                let color = premul(with_alpha(theme.accent, THUMB_ALPHA));
                p.fill_over(thumb, scrollbar::RADIUS, color);
            }
        }
    }

    let mut commands = Vec::new();
    let mut window_edge = false;
    let pressed_edge = if view.chrome.drawn_caption && !view.maximized {
        if let Some(edge) = ui.pointer_pos().and_then(|p| edge_at(p, win.w, win.h)) {
            window_edge = true;
            ui.set_cursor(resize_cursor(edge));
        }
        edge_press(&ui.input().events, (win.w, win.h))
    } else {
        None
    };
    if let Some(edge) = pressed_edge {
        commands.push(WindowCommand::DragResize(edge));
    }

    // The drag regions first, so the buttons registered after them sit on top: the band, and in
    // the pane view the pane's header.
    let mut drag_regions = vec![("title-band", layout.band)];
    if let Some(pane) = &layout.pane {
        drag_regions.push(("pane-header", pane.header));
    }
    for (name, rect) in drag_regions {
        let id = ui.id(name);
        let region = ui.interact(id, rect, Sense::CLICK);
        if region.press_started && pressed_edge.is_none() {
            // A borderless window maximizes; a native one (macOS) does what the OS setting asks.
            let double: fn() -> Option<WindowCommand> = if view.chrome.drawn_caption {
                || Some(WindowCommand::ToggleMaximize)
            } else {
                crate::double_click::system_action
            };
            commands.extend(band_command(region.press_count, view.chrome, double));
        }
    }
    if view.chrome.drawn_caption {
        let opts = CaptionOptions {
            height: TITLE_BAND,
            maximized: view.maximized,
            active: view.focused,
        };
        if let Some(button) = ui.caption_buttons("caption", win, opts) {
            commands.push(caption_command(button));
        }
    }

    let mut tab_events = Vec::new();
    let mut overflow = None;
    let mut inbox_clicked = false;
    let mut tab_context = None;
    let mut pane_toggle = false;
    let mut pane_edge = None;
    if let Some(pane) = &layout.pane {
        let out = pane::controls(
            ui,
            pane,
            view.chrome,
            view.pane_button_name,
            window_edge || pressed_edge.is_some(),
        );
        pane_toggle = out.toggle;
        pane_edge = out.edge;
        (overflow, inbox_clicked) =
            header_buttons(ui, &layout, view.overflow_name, view.inbox_button);
    } else if !view.page_open {
        // The header of the card drags the window where no tab or button is: its region goes in
        // before the strip's, so what the strip registers sits on top of it.
        let header = Rect::new(
            layout.card.x,
            layout.card.y,
            layout.card.w,
            STRIP_HEIGHT.min(layout.card.h),
        );
        let header_id = ui.id("strip-drag");
        // Since the luneui-rs pin 32d1904 (D290) the strip's viewport is as wide as its tabs and the
        // `+` follows the last tab, so the empty space is no region of the strip and a press there
        // reaches this one: no app-side detection of an "empty press" is needed. Each press drags
        // (count 1): the band is where a double press maximizes.
        let drag = ui.interact(header_id, header, Sense::CLICK);
        if drag.press_started && pressed_edge.is_none() {
            commands.push(WindowCommand::DragMove);
        }

        let modal_open = view.modal.is_some();
        let tabs: Vec<StripItem<'_>> = view
            .tabs
            .iter()
            .map(|t| StripItem {
                key: TabKey(t.id),
                title: t.title,
                closable: TAB_CLOSABLE,
                progress: t.progress,
            })
            .collect();
        let opts = TabStripOptions {
            active: view.active_tab.map(TabKey),
            panel_focused: view.focused && !modal_open,
            busy: view.busy,
            busy_tone: view.busy_tone,
            card: layout.card,
            name: "Terminal sessions",
            header_tint: Some(view.header_tint),
        };
        // The keys of the cued tabs; luneui draws the cue in its attention colour.
        let attention: Vec<TabKey> = view
            .tabs
            .iter()
            .filter(|t| t.attention)
            .map(|t| TabKey(t.id))
            .collect();
        let extras = TabStripExtras::new().with_attention(&attention);
        let strip = ui.tab_strip_with("strip", layout.strip, &tabs, &opts, &extras);
        tab_events = strip.events;
        tab_context = strip.context.map(|(key, rect)| TabContext {
            id: key.0,
            at: ui
                .pointer_pos()
                .filter(|q| rect.contains(*q))
                .unwrap_or((rect.x, rect.bottom())),
        });
        (overflow, inbox_clicked) =
            header_buttons(ui, &layout, view.overflow_name, view.inbox_button);
    }
    if show_pane_button {
        let button = IconButton {
            glyph: pane::TOGGLE_GLYPH,
            glyph_size: 16.0,
            color: None,
            radius: theme.item_radius,
            name: view.pane_button_name,
        };
        pane_toggle = ui
            .icon_button("pane-show", pane::show_button(win, view.chrome), &button)
            .clicked;
    }

    let (modal, ime_area) = match view.modal {
        Some(m) => {
            let (out, ime) = draw_modal(ui, m);
            (Some(out), ime)
        }
        None => (None, None),
    };
    // Inside the card, for every toast of the app (the warning toasts too).
    let toast = match view.toasts {
        Some(toasts) => ui.toasts_with(
            "toasts",
            toasts,
            &ToastStackOptions::new()
                .with_area(layout.card)
                .with_margin(TOAST_MARGIN),
        ),
        None => ToastResponse::default(),
    };
    ShellOut {
        body: layout.body,
        commands,
        tab_events,
        modal,
        ime_area,
        overflow,
        inbox_clicked,
        tab_context,
        toast,
        pane_toggle,
        pane_edge,
    }
}

/// The `⋯` button and, with entries, the inbox button at the places the layout gives them:
/// whether `⋯` was pressed (its rect) and whether the inbox button was.
fn header_buttons(
    ui: &mut Ui<'_>,
    layout: &Layout,
    overflow_name: &str,
    inbox: Option<InboxButton<'_>>,
) -> (Option<Rect>, bool) {
    let theme = *ui.theme();
    let more = IconButton {
        glyph: icon::ELLIPSIS,
        glyph_size: 16.0,
        color: None,
        radius: theme.item_radius,
        name: overflow_name,
    };
    let overflow = ui
        .icon_button("overflow", layout.overflow, &more)
        .clicked
        .then_some(layout.overflow);
    let Some(button) = inbox else {
        return (overflow, false);
    };
    // Unread: the attention colour; all read: the `⋯` button's own (`color: None`).
    let color = button.unread.then(|| theme.attention());
    // The swing replaces the button's own glyph for its 1 s; the fill, hover, press, open
    // state, focus ring and name stay the button's, and its rect never moves (D19).
    let swing = button
        .ring
        .filter(|_| button.unread && ui.tier() == MotionTier::Full)
        .and_then(|start| ring_angle(ui.time().saturating_sub(start)));
    let bell = IconButton {
        glyph: if swing.is_some() { "" } else { INBOX_GLYPH },
        glyph_size: 16.0,
        color,
        radius: theme.item_radius,
        name: button.name,
    };
    let extras = IconButtonExtras::new().with_open(button.open);
    let clicked = ui
        .icon_button_with("inbox-button", layout.inbox, &bell, &extras)
        .clicked;
    if let (Some(theta), Some(color)) = (swing, color) {
        let r = layout.inbox;
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        // Turned about the bell's top: the centre swings round the pivot above it.
        let centre = (
            cx - RING_PIVOT * theta.sin(),
            cy - RING_PIVOT + RING_PIVOT * theta.cos(),
        );
        ui.painter()
            .text_rotated(&icon::glyph(INBOX_GLYPH, 16.0, color), centre, theta);
        let next = ui.time() + Duration::from_millis(1);
        ui.request_repaint_at(next);
    }
    (overflow, clicked)
}

/// What the inbox list shows: the words and the rows (D16 maps entries onto them).
#[derive(Debug)]
pub struct InboxList<'a> {
    /// The list's accessible name.
    pub name: &'a str,
    /// The header's title.
    pub title: &'a str,
    /// The header's text button.
    pub clear_all: &'a str,
    /// Newest first.
    pub rows: &'a [MessageRow<'a>],
}

/// What the inbox list reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InboxOut {
    /// Where the popover is, logical px.
    pub rect: Rect,
    /// Escape, a press outside or a focus loss asked it to close.
    pub dismissed: Option<Dismissal>,
    /// The row a press, Enter or Space activated (an index into the rows).
    pub clicked: Option<usize>,
    /// The header's "Clear all" was pressed.
    pub clear_all: bool,
}

/// The popover of the inbox: below the button, right edges aligned, in the bar view; above it,
/// left edges aligned, from the pane's footer (D28). [`INBOX_GAP`] between, [`INBOX_WIDTH`] wide,
/// as tall as its content up to the smaller of [`INBOX_MAX_HEIGHT`] and the body's height, then it
/// scrolls. `layout` is the frame's. Text is drawn as plain text by the toolkit's rows: never
/// markup, never a link.
pub fn draw_inbox(ui: &mut Ui<'_>, layout: &Layout, list: &InboxList<'_>) -> InboxOut {
    let most = INBOX_MAX_HEIGHT.min(layout.body.h).max(HEADER_HEIGHT);
    let list_h = message_list::height(ui.painter(), list.rows, INBOX_WIDTH, most - HEADER_HEIGHT);
    let size = (INBOX_WIDTH, HEADER_HEIGHT + list_h);
    let mut options = PopoverOptions::new(layout.inbox, size);
    options.anchor_id = Some(ui.id("inbox-button"));
    (options.side, options.align) = match layout.pane {
        Some(_) => (Side::Top, PlaceAlign::Start),
        None => (Side::Bottom, PlaceAlign::End),
    };
    options.gap = INBOX_GAP;
    let (frame, placement, (clear_all, clicked)) =
        ui.popover("inbox-list", &options, |ui, rect| {
            let head = Rect::new(rect.x, rect.y, rect.w, HEADER_HEIGHT);
            let clear =
                ui.message_list_header("inbox-head", head, list.title, Some(list.clear_all));
            let body = Rect::new(rect.x, rect.y + HEADER_HEIGHT, rect.w, list_h);
            let opts = MessageListOptions::new(list.name).with_max_height(list_h);
            let response = ui.message_list("inbox-rows", body, list.rows, &opts);
            (clear, response.clicked)
        });
    InboxOut {
        rect: placement.rect,
        dismissed: frame.dismissed,
        clicked,
        clear_all,
    }
}

/// One inbox row as owned text: what [`inbox_rows`] makes of an entry (D16), borrowed by
/// [`RowText::row`] for the toolkit's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowText {
    pub title: String,
    pub age: String,
    pub body: Option<String>,
    pub meta: Option<String>,
    pub unread: bool,
}

impl RowText {
    /// The toolkit's row over this text.
    pub fn row(&self) -> MessageRow<'_> {
        let mut row = MessageRow::new(&self.title)
            .with_age(&self.age)
            .with_unread(self.unread);
        if let Some(body) = &self.body {
            row = row.with_body(body);
        }
        if let Some(meta) = &self.meta {
            row = row.with_meta(meta);
        }
        row
    }
}

/// The age label of an entry: "now" under a minute, then "2 min", "1 h", "3 d".
pub fn age_label(strings: &lune_ui::strings::StringTable, age: Age) -> String {
    let (key, n) = match age {
        Age::Now => return strings.tr("toast.now").to_owned(),
        Age::Minutes(n) => ("inbox.minutes", n),
        Age::Hours(n) => ("inbox.hours", n),
        Age::Days(n) => ("inbox.days", n),
    };
    strings.tr_fmt(key, &[&n], &lune_ui::strings::DisplayFormat)
}

/// The rows of the list, newest first, with the tab each one belongs to (a row press activates
/// it). `now` is the app clock the entries' times come from; `tab_name` is a tab's title now
/// (D17). The mapping is D16: a bell is "Bell" or "Bell ×N" with the tab name as meta; a titled
/// message shows its title, its body and the tab name; a message without a title shows the tab
/// name as its first line and the message under it, with no meta; a read row's meta ends in
/// "seen" (alone, for a row with no other meta). Only the sanitised text of the inbox is used.
pub fn inbox_rows(
    inbox: &Inbox,
    now: Duration,
    strings: &lune_ui::strings::StringTable,
    tab_name: impl Fn(u64) -> Option<String>,
) -> Vec<(u64, RowText)> {
    let seen = strings.tr("inbox.seen");
    inbox
        .entries()
        .map(|n| {
            let name = tab_name(n.tab).unwrap_or_default();
            let age = age_label(strings, notices::age(now.saturating_sub(n.at)));
            let (title, body, meta) = match &n.kind {
                NoticeKind::Bell => {
                    let bell = strings.tr("inbox.bell");
                    let title = if n.count > 1 {
                        format!("{bell} \u{d7}{}", n.count)
                    } else {
                        bell.to_owned()
                    };
                    (title, None, Some(name))
                }
                NoticeKind::Message { title, body } if title.is_empty() => {
                    (name, Some(body.clone()).filter(|b| !b.is_empty()), None)
                }
                NoticeKind::Message { title, body } => (
                    title.clone(),
                    Some(body.clone()).filter(|b| !b.is_empty()),
                    Some(name),
                ),
            };
            let meta = match (n.unread, meta) {
                (true, meta) => meta.filter(|m| !m.is_empty()),
                (false, Some(m)) if !m.is_empty() => Some(format!("{m} \u{b7} {seen}")),
                (false, _) => Some(seen.to_owned()),
            };
            (
                n.tab,
                RowText {
                    title,
                    age,
                    body,
                    meta,
                    unread: n.unread,
                },
            )
        })
        .collect()
}

/// Every string key the inbox asks for (the toast's "now" is the notices module's).
pub fn inbox_required_keys() -> Vec<String> {
    [
        "inbox.name",
        "inbox.title",
        "inbox.clearAll",
        "inbox.bell",
        "inbox.seen",
        "inbox.minutes",
        "inbox.hours",
        "inbox.days",
    ]
    .iter()
    .map(|k| (*k).to_owned())
    .collect()
}

/// The close-confirm or rename dialog (Lunegit's copy, `en.json`; layout as the spike's), or the
/// group editor.
fn draw_modal(ui: &mut Ui<'_>, modal: Modal<'_>) -> (ModalOut, Option<Rect>) {
    // The card is the menus' glass (the theme's `menu_glass`, raised by Window opacity), frosted
    // over an unblurred scrim: the card blurs what is under it (luneui D411, since v0.7.3, under
    // `scrim_blur: false`), the terminal around it stays readable (director verdict, step 2
    // batch 1). The near-opaque fill of before (alpha 235) hid the frost entirely.
    let glass = DialogGlass::of(ui.theme());
    let options = DialogOptions {
        scrim_blur: false,
        ..DialogOptions::default()
    };
    match modal {
        Modal::Confirm {
            title,
            message,
            dont_ask,
        } => {
            let dialog = Dialog {
                title,
                message,
                extra_h: if dont_ask.is_some() { 34.0 } else { 0.0 },
                confirm: "Close",
                cancel: Cancel::Default,
            };
            let r = ui.dialog_with("confirm-close", &dialog, &options, &glass, |ui, x, y, w| {
                if let Some(dont_ask) = dont_ask {
                    let rect = Rect::new(x, y + 12.0, w, 22.0);
                    ui.checkbox("dont-ask", rect, dont_ask, "Don't ask again");
                }
            });
            (
                ModalOut {
                    confirmed: r.confirmed,
                    cancelled: r.cancelled,
                },
                None,
            )
        }
        Modal::Rename { text, focus } => {
            let dialog = Dialog {
                title: "Rename Tab",
                message: "Enter a new name for this terminal tab.",
                extra_h: 22.0 + 34.0 + 4.0,
                confirm: "Rename",
                cancel: Cancel::Default,
            };
            let mut submitted = false;
            let mut ime = None;
            let r = ui.dialog_with("rename-tab", &dialog, &options, &glass, |ui, x, y, w| {
                let theme = *ui.theme();
                let label = Spec::plain(
                    "TAB NAME",
                    Style::ui(theme.font_s, theme.fg_tertiary).medium(),
                );
                ui.painter().text(&label, x, y + 18.0);
                let field = Rect::new(x, y + 36.0, w, 32.0);
                let id = ui.id("rename-field");
                if focus {
                    ui.request_focus(id);
                }
                let opts = FieldOptions {
                    placeholder: "Tab name",
                    max_chars: Some(crate::title::MAX_NAME_CHARS),
                    label: Some("Tab name"),
                    ..FieldOptions::default()
                };
                let out = ui.text_field_with("rename-field", field, text, &opts);
                submitted = out.submitted;
                ime = out.ime_area;
            });
            (
                ModalOut {
                    confirmed: r.confirmed || submitted,
                    cancelled: r.cancelled && !submitted,
                },
                ime,
            )
        }
        Modal::GroupEditor {
            editing,
            text,
            color,
            taken,
            look,
            focus,
        } => {
            let (title, confirm, message) = {
                let (title, confirm) = if editing {
                    ("group.editor.editTitle", "group.editor.save")
                } else {
                    ("group.editor.newTitle", "group.editor.create")
                };
                (
                    ui.tr(title).to_owned(),
                    ui.tr(confirm).to_owned(),
                    ui.tr("group.editor.message").to_owned(),
                )
            };
            let dialog = Dialog {
                title: &title,
                message: &message,
                extra_h: EDITOR_EXTRA_H,
                confirm: &confirm,
                cancel: Cancel::Default,
            };
            // The button is drawn from the text as the frame began.
            let was_ok = groups::name_problem(text, taken).is_none();
            let options = DialogOptions {
                confirm_enabled: was_ok,
                ..options
            };
            let mut submitted = false;
            let mut ime = None;
            let r = ui.dialog_with("group-editor", &dialog, &options, &glass, |ui, x, y, w| {
                let theme = *ui.theme();
                editor_label(ui, "group.editor.nameLabel", (x, y + 18.0));
                let field = Rect::new(x, y + 36.0, w, 32.0);
                let id = ui.id("group-field");
                if focus {
                    ui.request_focus(id);
                }
                let name = ui.tr("group.editor.name").to_owned();
                let opts = FieldOptions {
                    placeholder: &name,
                    label: Some(&name),
                    // No `max_chars`: it would cut a long paste without a word.
                    ..FieldOptions::default()
                };
                let out = ui.text_field_with("group-field", field, text, &opts);
                submitted = out.submitted;
                ime = out.ime_area;
                if let Some(key) = name_hint(groups::name_problem(text, taken)) {
                    let spec = Spec::plain(
                        ui.tr(key).to_owned(),
                        Style::ui(theme.font_s, theme.status_error),
                    );
                    ui.painter().text(&spec, x, y + EDITOR_HINT_Y);
                }
                editor_label(ui, "group.editor.colorLabel", (x, y + EDITOR_COLOR_LABEL_Y));
                swatches(ui, x, y + EDITOR_SWATCH_Y, color, look);
            });
            let ok = groups::name_problem(text, taken).is_none();
            if ok != was_ok {
                let now = ui.time();
                ui.request_repaint_at(now + Duration::from_millis(1));
            }
            (
                ModalOut {
                    // Judged on the text as it is after this frame's typing.
                    confirmed: ok && (r.confirmed || submitted),
                    cancelled: r.cancelled && !submitted,
                },
                ime,
            )
        }
    }
}

/// A small label of the group editor, as the rename dialog's.
fn editor_label(ui: &mut Ui<'_>, key: &str, (x, y): (f32, f32)) {
    let theme = *ui.theme();
    let style = Style::ui(theme.font_s, theme.fg_tertiary).medium();
    let spec = Spec::plain(ui.tr(key).to_owned(), style);
    ui.painter().text(&spec, x, y);
}

/// The nine colours in a row from `(x, y)`: filled circles, the chosen one ringed. A press, Space
/// or Enter on one chooses it.
fn swatches(ui: &mut Ui<'_>, x: f32, y: f32, chosen: &mut GroupColor, look: LookTheme) {
    let theme = *ui.theme();
    let group = ui.id("group-colors");
    let row = Rect::new(
        x,
        y,
        swatch_rect(GroupColor::ALL.len() - 1, x, y).right() - x,
        SWATCH,
    );
    let name = ui.tr("group.editor.colorLabel").to_owned();
    let node = Node::new(group, Role::RadioGroup, &name, row);
    ui.node_container(node, |ui| {
        // The input first, so every swatch is drawn knowing the choice of this frame.
        let before = *chosen;
        let at: Vec<_> = GroupColor::ALL
            .into_iter()
            .enumerate()
            .map(|(i, color)| {
                let rect = swatch_rect(i, x, y);
                let id = ui.id(("group-color", i));
                let r = ui.interact(id, rect, Sense::CLICK_FOCUS);
                // Both keys are taken whichever came: Enter must not also confirm the dialog.
                let space = ui.consume_key(id, NamedKey::Space);
                let enter = ui.consume_key(id, NamedKey::Enter);
                if r.clicked || space || enter {
                    *chosen = color;
                }
                (color, rect, id, r)
            })
            .collect();
        if *chosen != before {
            ui.request_repaint_at(ui.time() + Duration::from_millis(1));
        }
        for (color, rect, id, r) in at {
            let outer = rect.inset(-SWATCH_RING, -SWATCH_RING);
            let [red, green, blue] = color.tone(look).fill;
            let fill = [red, green, blue, 0xFF];
            let p = ui.painter();
            p.fill(rect, SWATCH / 2.0, fill);
            if *chosen == color {
                p.ring(outer, outer.w / 2.0, SWATCH_RING_WIDTH, fill, true);
            } else if r.hovered {
                p.ring(outer, outer.w / 2.0, 1.0, theme.fg_tertiary, true);
            }
            if r.focus_visible {
                let focus = outer.inset(-2.0, -2.0);
                p.ring(focus, focus.w / 2.0, 1.5, theme.accent, true);
            }
            let name = ui.tr(&color_name_key(color)).to_owned();
            let mut node = Node::new(id, Role::Radio, &name, rect);
            if *chosen == color {
                node.states.insert(State::Checked);
            }
            node.states.insert(State::Focusable);
            node.actions.insert(Action::Default);
            ui.node(node);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts;
    use lune_ui::input::{Modifiers, RawInput, keymap};
    use lune_ui::lunegit_dark::THEME;
    use lune_ui::motion::{Cadence, MotionTier};
    use lune_ui::shortcuts::Shortcuts;
    use lune_ui::strings::{DisplayFormat, StringTable};
    use lune_ui::text::TextCache;
    use lune_ui::ui::{FrameConfig, UiStore};
    use std::time::Duration;

    fn metrics(cell_w: u32, cell_h: u32, scale: f32) -> CellMetrics {
        CellMetrics {
            cell_w,
            cell_h,
            char_h: cell_h - 2,
            char_top: 1,
            baseline: 13,
            font_size: 13.0,
            scale,
        }
    }

    /// The card, body and grid rects for the first window size (1040 x 660), at scales 1 and 2.
    #[test]
    fn layout_pins_the_card_the_body_and_the_grid() {
        let win = Rect::new(0.0, 0.0, 1040.0, 660.0);
        let l = Layout::new(win);
        assert_eq!(l.band, Rect::new(0.0, 0.0, 1040.0, 34.0));
        assert_eq!(l.card, Rect::new(10.0, 34.0, 1020.0, 616.0));
        assert_eq!(l.strip, Rect::new(10.0, 34.0, 980.0, 32.0));
        assert_eq!(l.overflow, Rect::new(998.0, 38.0, 24.0, 24.0));
        assert_eq!(l.body, Rect::new(20.0, 76.0, 1000.0, 564.0));

        // Scale 1: an 8 x 17 cell; scale 2: 16 x 34 (the same cell in logical px).
        let one = place_grid(l.body, metrics(8, 17, 1.0));
        assert_eq!(
            one,
            GridPlacement {
                origin: (20.0, 76.0),
                cols: 125,
                rows: 33
            }
        );
        let two = place_grid(l.body, metrics(16, 34, 2.0));
        assert_eq!(
            two,
            GridPlacement {
                origin: (20.0, 76.0),
                cols: 125,
                rows: 33
            }
        );
    }

    #[test]
    fn the_grid_origin_snaps_to_a_device_pixel() {
        // A body at 20.3 logical px is 40.6 -> 41 device px at scale 2: 20.5 logical.
        let body = Rect::new(20.3, 44.0, 400.0, 300.0);
        let p = place_grid(body, metrics(16, 34, 2.0));
        assert_eq!(p.origin, (20.5, 44.0));
        // A bad scale counts as 1.
        let p = place_grid(body, metrics(8, 17, 0.0));
        assert_eq!(p.origin, (20.0, 44.0));
    }

    #[test]
    fn a_tiny_window_has_empty_rects_not_negative_ones() {
        let l = Layout::new(Rect::new(0.0, 0.0, 15.0, 20.0));
        assert_eq!((l.card.w, l.card.h), (0.0, 0.0));
        assert_eq!((l.strip.w, l.strip.h), (0.0, 0.0));
        assert_eq!((l.body.w, l.body.h), (0.0, 0.0));
        assert_eq!(l.band.h, 20.0);
    }

    /// The pane view at 1040 x 660 and width 248 (the macOS and Linux chrome), and with the drawn
    /// caption buttons, where the card starts under a band over the terminal column.
    #[test]
    fn layout_pins_the_pane_view() {
        let win = Rect::new(0.0, 0.0, 1040.0, 660.0);
        for chrome in [MACOS_CHROME, LINUX_CHROME] {
            let view = pane::resolve(true, 248, win.w, chrome);
            let l = Layout::for_view(win, false, view);
            assert_eq!(l.card, Rect::new(248.0, 10.0, 782.0, 640.0));
            assert_eq!(l.body, Rect::new(258.0, 20.0, 762.0, 620.0));
            assert_eq!(l.band, Rect::new(248.0, 0.0, 792.0, 10.0));
            assert_eq!((l.strip.w, l.strip.h), (0.0, 0.0), "no strip");
            let pane = l.pane.expect("the pane");
            assert_eq!(pane.rect, Rect::new(0.0, 0.0, 248.0, 660.0));
            assert_eq!((l.overflow, l.inbox), (pane.menu, pane.bell));
        }
        let view = pane::resolve(true, 248, win.w, WINDOWS_CHROME);
        let l = Layout::for_view(win, true, view);
        assert_eq!(l.card, Rect::new(248.0, 34.0, 782.0, 616.0));
        assert_eq!(l.body, Rect::new(258.0, 44.0, 762.0, 596.0));
        assert_eq!(l.band, Rect::new(248.0, 0.0, 792.0, 34.0));
        // The inbox button changes no rect of the pane view.
        assert_eq!(l, Layout::for_view(win, false, view));
    }

    #[test]
    fn the_bar_view_is_todays_layout() {
        let win = Rect::new(0.0, 0.0, 1040.0, 660.0);
        for inbox in [false, true] {
            assert_eq!(
                Layout::for_view(win, inbox, View::Bar),
                Layout::with_inbox(win, inbox)
            );
        }
        assert_eq!(Layout::new(win).pane, None);
    }

    /// The grid `place()` sends to a session follows the view's body (D26): the pane takes columns
    /// and gives the strip's rows back.
    #[test]
    fn each_views_body_gives_the_grid_its_own_size() {
        let win = Rect::new(0.0, 0.0, 1040.0, 660.0);
        let grid = |view| {
            let body = Layout::for_view(win, false, view).body;
            let g = place_grid(body, metrics(8, 17, 1.0));
            (g.origin, g.cols, g.rows)
        };
        assert_eq!(grid(View::Bar), ((20.0, 76.0), 125, 33));
        assert_eq!(
            grid(pane::resolve(true, 248, win.w, MACOS_CHROME)),
            ((258.0, 20.0), 95, 36)
        );
        assert_eq!(
            grid(pane::resolve(true, 248, win.w, WINDOWS_CHROME)),
            ((258.0, 44.0), 95, 35)
        );
        assert_eq!(
            grid(pane::resolve(true, 420, win.w, MACOS_CHROME)),
            ((430.0, 20.0), 73, 36)
        );
    }

    #[test]
    fn the_thumb_shows_only_with_scrollback() {
        let body = Rect::new(20.0, 44.0, 1000.0, 595.0);
        let none = ScrollView {
            viewport_top: 0,
            base_y: 0,
            total_lines: 35,
        };
        assert_eq!(scroll_thumb(body, 17.0, none), None);
        // 500 lines, 35 shown, scrolled to the bottom: the thumb is at the bottom of the track.
        let bottom = ScrollView {
            viewport_top: 465,
            base_y: 465,
            total_lines: 500,
        };
        let t = scroll_thumb(body, 17.0, bottom).expect("thumb");
        assert_eq!(t.w, scrollbar::THUMB);
        assert!((t.y + t.h - (body.y + body.h)).abs() < 1.0, "{t:?}");
        let top = ScrollView {
            viewport_top: 0,
            ..bottom
        };
        let t = scroll_thumb(body, 17.0, top).expect("thumb");
        assert!((t.y - body.y).abs() < 1.0, "{t:?}");
        assert!(t.h >= scrollbar::MIN_LENGTH);
    }

    #[test]
    fn dragging_the_thumb_scrolls_in_whole_lines_and_carries_the_rest() {
        let body = Rect::new(20.0, 44.0, 1000.0, 595.0);
        let scroll = ScrollView {
            viewport_top: 100,
            base_y: 465,
            total_lines: 500,
        };
        let g = scroll_geometry(body, 17.0, scroll).expect("bar");
        // A full travel is the whole range: base_y lines.
        let mut drag = ThumbDrag::start(100.0);
        let all = drag.moved(100.0 + g.travel as f32, body, 17.0, scroll);
        assert!((all - 465).abs() <= 1, "{all}");
        // Up is negative; small moves add up without losing the remainder.
        let mut drag = ThumbDrag::start(300.0);
        assert!(drag.moved(290.0, body, 17.0, scroll) < 0);
        let mut drag = ThumbDrag::start(300.0);
        let per_px = 465.0 / g.travel;
        let mut total = 0;
        for i in 1..=200 {
            total += drag.moved(300.0 + i as f32 * 0.1, body, 17.0, scroll);
        }
        let want = (20.0 * per_px).floor() as i64;
        assert!(
            (total - want).abs() <= 1,
            "total {total}, want about {want}"
        );
        // No bar, no scroll.
        let none = ScrollView {
            base_y: 0,
            ..scroll
        };
        assert_eq!(ThumbDrag::start(0.0).moved(50.0, body, 17.0, none), 0);
    }

    #[test]
    fn a_press_grabs_the_thumb_a_little_outside_its_6_px() {
        let body = Rect::new(20.0, 44.0, 1000.0, 595.0);
        let scroll = ScrollView {
            viewport_top: 0,
            base_y: 465,
            total_lines: 500,
        };
        let t = scroll_thumb(body, 17.0, scroll).expect("thumb");
        let mid = t.y + t.h / 2.0;
        assert!(thumb_hit(body, 17.0, scroll, (t.x + 1.0, mid)));
        assert!(thumb_hit(body, 17.0, scroll, (t.x - 2.0, mid)));
        assert!(!thumb_hit(body, 17.0, scroll, (t.x - 10.0, mid)));
        assert!(!thumb_hit(body, 17.0, scroll, (t.x + 1.0, t.y + t.h + 5.0)));
        let none = ScrollView {
            base_y: 0,
            ..scroll
        };
        assert!(!thumb_hit(body, 17.0, none, (t.x + 1.0, mid)));
    }

    #[test]
    fn the_caption_buttons_and_the_band_map_to_window_commands() {
        assert_eq!(
            caption_command(CaptionButton::Minimize),
            WindowCommand::Minimize
        );
        assert_eq!(
            caption_command(CaptionButton::Maximize),
            WindowCommand::ToggleMaximize
        );
        assert_eq!(caption_command(CaptionButton::Close), WindowCommand::Close);
        let drawn = Chrome {
            drawn_caption: true,
            title_inset: 146.0,
            band_double_click: true,
            native_lights: false,
        };
        let native = Chrome {
            drawn_caption: false,
            band_double_click: false,
            ..drawn
        };
        let mac = Chrome {
            drawn_caption: false,
            ..drawn
        };
        let max = || Some(WindowCommand::ToggleMaximize);
        let min = || Some(WindowCommand::Minimize);
        let drag = Some(WindowCommand::DragMove);
        assert_eq!(band_command(1, drawn, max), drag);
        assert_eq!(band_command(2, drawn, max), max());
        assert_eq!(band_command(3, drawn, max), drag, "a third press drags");
        assert_eq!(band_command(2, native, max), drag);
        assert_eq!(band_command(1, mac, min), drag);
        assert_eq!(band_command(2, mac, min), min());
        assert_eq!(band_command(2, mac, || None), None);
    }

    /// One toolkit frame of the shell at 1040 x 660, scale 1, no GPU: what `draw` asks of the window.
    fn frame(
        store: &mut UiStore,
        text: &mut TextCache,
        events: Vec<Event>,
        chrome: Chrome,
        maximized: bool,
    ) -> ShellOut {
        frame_with(store, text, events, chrome, maximized, &[TAB_A, TAB_B])
    }

    const TAB_A: StripTab<'static> = StripTab {
        id: 1,
        title: "zsh",
        progress: None,
        attention: false,
    };
    const TAB_B: StripTab<'static> = StripTab {
        id: 2,
        title: "cargo build",
        progress: None,
        attention: false,
    };

    fn frame_with(
        store: &mut UiStore,
        text: &mut TextCache,
        events: Vec<Event>,
        chrome: Chrome,
        maximized: bool,
        tabs: &[StripTab<'_>],
    ) -> ShellOut {
        frame_tree(store, text, events, chrome, maximized, tabs).0
    }

    /// `frame_with`, and the frame's semantic tree too.
    fn frame_tree(
        store: &mut UiStore,
        text: &mut TextCache,
        events: Vec<Event>,
        chrome: Chrome,
        maximized: bool,
        tabs: &[StripTab<'_>],
    ) -> (ShellOut, String) {
        let input = RawInput {
            size: (1040.0, 660.0),
            scale: 1.0,
            time: Duration::ZERO,
            focused: true,
            events,
        };
        let (strings, shortcuts) = (StringTable::default(), Shortcuts::default());
        let config = FrameConfig {
            theme: THEME,
            tier: MotionTier::Off,
            busy_cadence: Cadence::BUSY,
            strings: &strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &shortcuts,
        };
        let view = ShellView {
            title: None,
            maximized,
            focused: true,
            scroll: None,
            cell_h: 17.0,
            chrome,
            view: View::Bar,
            show_pane_button: false,
            pane_button_name: "Pane",
            tabs,
            active_tab: tabs.first().map(|t| t.id),
            modal: None,
            toasts: None,
            page_open: false,
            busy: None,
            busy_tone: Tone::Neutral,
            overflow_name: "More",
            inbox_button: None,
            header_tint: HEADER_TINT,
            fading_slots: 0,
        };
        let mut ui = Ui::begin(store, &input, config, text);
        let out = draw(&mut ui, view);
        let (_, frame) = ui.finish();
        (out, format!("{:?}", frame.tree))
    }

    /// One frame with `toasts` drawn, at `ms` (tier Off: nothing animates), and what it handed back.
    fn toast_frame(
        store: &mut UiStore,
        text: &mut TextCache,
        toasts: &mut Toasts,
        ms: u64,
    ) -> ShellOut {
        let input = RawInput {
            size: (1040.0, 660.0),
            scale: 1.0,
            time: Duration::from_millis(ms),
            focused: true,
            events: Vec::new(),
        };
        let (strings, shortcuts) = (StringTable::default(), Shortcuts::default());
        let config = FrameConfig {
            theme: THEME,
            tier: MotionTier::Off,
            busy_cadence: Cadence::BUSY,
            strings: &strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &shortcuts,
        };
        let tabs = [TAB_A, TAB_B];
        let view = ShellView {
            title: None,
            maximized: false,
            focused: true,
            scroll: None,
            cell_h: 17.0,
            chrome: HOST,
            view: View::Bar,
            show_pane_button: false,
            pane_button_name: "Pane",
            tabs: &tabs,
            active_tab: Some(1),
            modal: None,
            toasts: Some(toasts),
            page_open: false,
            busy: None,
            busy_tone: Tone::Neutral,
            overflow_name: "More",
            inbox_button: None,
            header_tint: HEADER_TINT,
            fading_slots: 0,
        };
        let mut ui = Ui::begin(store, &input, config, text);
        let out = draw(&mut ui, view);
        let _ = ui.finish();
        out
    }

    const TEXTS: NoticeTexts<'static> = NoticeTexts {
        go_to_tab: "Go to tab",
        dismiss: "Dismiss",
        now: "now",
    };

    /// The two update toasts carry their version, a Dismiss and one primary button with the id the
    /// app acts on; one drawn through a real frame is listed, in the card's corner.
    #[test]
    fn the_update_toasts_name_the_version_and_carry_their_button() {
        let strings = crate::strings::english();
        let accent = THEME.attention();
        for (kind, body, label, id) in [
            (
                UpdateToastKind::Available,
                "Luneterm 0.9.0 is available.",
                "Update",
                TOAST_UPDATE,
            ),
            (
                UpdateToastKind::Ready,
                "Luneterm 0.9.0 is ready. Restarting closes all shells.",
                "Restart to update",
                TOAST_RESTART,
            ),
        ] {
            let (spec, extras) = update_toast(kind, "0.9.0", &strings, accent);
            assert_eq!(spec.message, body);
            let actions: Vec<(&str, bool, u64)> = spec
                .actions
                .iter()
                .map(|a| (a.label.as_str(), a.primary, a.id))
                .collect();
            assert_eq!(
                actions,
                [("Dismiss", false, TOAST_DISMISS), (label, true, id)]
            );
            let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
            let mut toasts = Toasts::default();
            let toast = toasts.push_with(spec, extras, Duration::from_millis(1_000));
            toast_frame(&mut store, &mut text, &mut toasts, 1_100);
            let live = toasts.live();
            assert_eq!(live.len(), 1, "{kind:?} is drawn");
            assert_eq!(live[0].0, toast);
        }
    }

    /// A real `Toasts` through real frames: a notification toast is listed once drawn, sits in
    /// the card's corner 22 px in, leaves the list when its 6 s are up, and the slots follow.
    #[test]
    fn a_notice_toast_is_listed_when_drawn_sits_in_the_card_corner_and_leaves_with_its_timeout() {
        use crate::notices::{Admit, TOAST_LIFE, ToastSlots};
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut toasts = Toasts::default();
        let mut slots: ToastSlots<_, Rect> = ToastSlots::new();
        let (spec, extras) = notice_toast(
            "claude",
            "Claude Code",
            "Claude needs your permission to use Bash",
            &TEXTS,
            THEME.attention(),
            TOAST_LIFE,
        );
        let id = toasts.push_with(spec, extras, Duration::from_millis(1_000));
        slots.shown(id, 3, Duration::from_millis(1_000));
        // Not drawn yet: live for the toolkit, in no list, slot kept.
        assert!(toasts.is_live(id) && toasts.live().is_empty());
        assert!(
            slots
                .sync(toasts.live(), Duration::from_millis(1_010), |i| toasts
                    .is_live(i))
                .is_empty()
        );
        assert!(!slots.seen(id));

        let layout = Layout::new(Rect::new(0.0, 0.0, 1040.0, 660.0));
        let out = toast_frame(&mut store, &mut text, &mut toasts, 1_100);
        assert_eq!(out.toast, ToastResponse::default());
        let live = toasts.live().to_vec();
        assert_eq!(live.len(), 1, "one toast listed");
        assert_eq!(live[0].0, id);
        let r = live[0].1;
        assert!(
            (layout.card.right() - r.right() - TOAST_MARGIN).abs() <= 1.0
                && (layout.card.bottom() - r.bottom() - TOAST_MARGIN).abs() <= 1.0,
            "{r:?} in {:?}",
            layout.card
        );
        slots.sync(&live, Duration::from_millis(1_100), |i| toasts.is_live(i));
        assert!(slots.seen(id));
        // Still listed, same place, 400 ms later: armed.
        toast_frame(&mut store, &mut text, &mut toasts, 1_500);
        slots.sync(toasts.live(), Duration::from_millis(1_500), |i| {
            toasts.is_live(i)
        });
        assert_eq!(
            slots.press_target(id, Duration::from_millis(1_500)),
            Some(3)
        );
        assert_eq!(slots.press_target(id, Duration::from_millis(1_499)), None);
        // 6 s after the push the toast times out: the next frames drop it from the list.
        for ms in [6_900, 7_000, 7_500] {
            toast_frame(&mut store, &mut text, &mut toasts, ms);
        }
        assert!(toasts.live().is_empty(), "{:?}", toasts.live());
        assert!(!toasts.is_live(id));
        assert_eq!(
            slots.sync(toasts.live(), Duration::from_millis(7_500), |i| toasts
                .is_live(i)),
            [id]
        );
        // The P1 case: the tab's next message gets a toast again.
        assert_eq!(
            slots.admit(3, Duration::from_millis(7_600)),
            Admit::New { evict: None }
        );
    }

    #[test]
    fn a_tab_with_attention_is_cued_and_the_others_are_not() {
        let mut cued = TAB_B;
        cued.attention = true;
        let draw_tabs = |tabs: &[StripTab<'_>]| {
            let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
            frame_tree(&mut store, &mut text, Vec::new(), HOST, false, tabs).1
        };
        let plain = draw_tabs(&[TAB_A, TAB_B]);
        let with = draw_tabs(&[TAB_A, cued]);
        assert!(!plain.contains("Needs attention"), "no tab is cued");
        assert_eq!(
            with.matches("Needs attention").count(),
            1,
            "only the cued tab"
        );
        // Both at once: a progress report and the cue are both passed (no app-side priority).
        let mut both = cued;
        both.progress = Some(Tone::Warn);
        assert_eq!(
            draw_tabs(&[TAB_A, both]).matches("Needs attention").count(),
            1
        );
    }

    fn button(pos: (f32, f32), pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        }
    }

    const BORDERLESS: Chrome = Chrome {
        drawn_caption: true,
        title_inset: 146.0,
        band_double_click: true,
        native_lights: false,
    };

    /// The chrome through a real `Ui` frame: the first frame registers the hit regions, later
    /// frames carry the input (the toolkit resolves input against the previous frame's regions).
    #[test]
    fn a_borderless_window_drags_by_the_band_and_maximizes_on_a_double_press() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events, maximized| {
            frame(&mut store, &mut text, events, BORDERLESS, maximized).commands
        };
        assert_eq!(go(vec![], false), vec![]);
        let at = (500.0, 17.0);
        assert_eq!(
            go(vec![button(at, true)], false),
            vec![WindowCommand::DragMove]
        );
        assert_eq!(go(vec![button(at, false)], false), vec![]);
        assert_eq!(
            go(vec![button(at, true)], false),
            vec![WindowCommand::ToggleMaximize]
        );
    }

    #[test]
    fn a_native_window_only_drags_by_the_band() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let native = Chrome {
            drawn_caption: false,
            band_double_click: false,
            ..BORDERLESS
        };
        let mut go = |events| frame(&mut store, &mut text, events, native, false).commands;
        go(vec![]);
        let at = (500.0, 17.0);
        for _ in 0..2 {
            assert_eq!(go(vec![button(at, true)]), vec![WindowCommand::DragMove]);
            go(vec![button(at, false)]);
        }
        // No resize edges and no caption buttons: a press at the corner is just the band.
        assert_eq!(
            go(vec![button((1030.0, 17.0), true)]),
            vec![WindowCommand::DragMove]
        );
    }

    #[test]
    fn the_caption_buttons_ask_for_minimize_maximize_and_close() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events| frame(&mut store, &mut text, events, BORDERLESS, false).commands;
        go(vec![]);
        // Three 46 px buttons along the top right: 902..948, 948..994 and 994..1040.
        for (x, want) in [
            (925.0, WindowCommand::Minimize),
            (971.0, WindowCommand::ToggleMaximize),
            (1017.0, WindowCommand::Close),
        ] {
            let press = go(vec![button((x, 17.0), true)]);
            assert!(!press.contains(&WindowCommand::DragMove), "{press:?}");
            assert_eq!(go(vec![button((x, 17.0), false)]), vec![want]);
        }
    }

    #[test]
    fn a_press_on_the_border_resizes_and_a_maximized_window_does_not() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events, maximized| {
            frame(&mut store, &mut text, events, BORDERLESS, maximized).commands
        };
        go(vec![], false);
        assert_eq!(
            go(vec![button((2.0, 300.0), true)], false),
            vec![WindowCommand::DragResize(ResizeEdge::West)]
        );
        go(vec![button((2.0, 300.0), false)], false);
        // The top border lies in the band: the resize wins over the band's drag.
        assert_eq!(
            go(vec![button((500.0, 2.0), true)], false),
            vec![WindowCommand::DragResize(ResizeEdge::North)]
        );
        go(vec![button((500.0, 2.0), false)], false);
        // Maximized: the same press on the left border asks for nothing.
        assert_eq!(go(vec![button((2.0, 300.0), true)], true), vec![]);
    }

    #[test]
    fn an_edge_press_is_a_primary_press_on_the_border() {
        let size = (1040.0, 660.0);
        let press = |pos, button, pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::default(),
        };
        let on_edge = press((2.0, 300.0), PointerButton::Primary, true);
        assert_eq!(
            edge_press(std::slice::from_ref(&on_edge), size),
            Some(ResizeEdge::West)
        );
        assert_eq!(
            edge_press(
                &[press((1039.0, 659.0), PointerButton::Primary, true)],
                size
            ),
            Some(ResizeEdge::SouthEast)
        );
        // A release, a secondary press and a press inside the window are not edge presses.
        for e in [
            press((2.0, 300.0), PointerButton::Primary, false),
            press((2.0, 300.0), PointerButton::Secondary, true),
            press((500.0, 300.0), PointerButton::Primary, true),
            Event::PointerMoved { pos: (2.0, 300.0) },
        ] {
            assert_eq!(edge_press(&[e], size), None);
        }
        assert_eq!(resize_cursor(ResizeEdge::NorthWest), CursorIcon::NwseResize);
        assert_eq!(resize_cursor(ResizeEdge::South), CursorIcon::NsResize);
    }

    /// One frame of the shell at 1040 x 660 in `view`, with the inbox button and the page as
    /// given.
    #[derive(Clone, Copy)]
    struct PaneShell {
        chrome: Chrome,
        view: View,
        show_pane_button: bool,
        inbox: bool,
        page_open: bool,
    }

    impl PaneShell {
        fn pane(chrome: Chrome) -> PaneShell {
            PaneShell {
                chrome,
                view: pane::resolve(true, 248, 1040.0, chrome),
                show_pane_button: false,
                inbox: true,
                page_open: false,
            }
        }

        fn run(self, store: &mut UiStore, text: &mut TextCache, events: Vec<Event>) -> ShellOut {
            let input = RawInput {
                size: (1040.0, 660.0),
                scale: 1.0,
                time: Duration::ZERO,
                focused: true,
                events,
            };
            let (strings, shortcuts) = (StringTable::default(), Shortcuts::default());
            let config = FrameConfig {
                theme: THEME,
                tier: MotionTier::Off,
                busy_cadence: Cadence::BUSY,
                strings: &strings,
                arg_format: &DisplayFormat,
                keymap: keymap(),
                shortcuts: &shortcuts,
            };
            let tabs = [TAB_A, TAB_B];
            let view = ShellView {
                title: None,
                maximized: false,
                focused: true,
                scroll: None,
                cell_h: 17.0,
                chrome: self.chrome,
                view: self.view,
                show_pane_button: self.show_pane_button,
                pane_button_name: "Pane",
                tabs: &tabs,
                active_tab: Some(1),
                modal: None,
                toasts: None,
                page_open: self.page_open,
                busy: None,
                busy_tone: Tone::Neutral,
                overflow_name: "More",
                inbox_button: self.inbox.then_some(InboxButton {
                    name: "Notifications",
                    unread: true,
                    ring: None,
                    open: false,
                }),
                header_tint: HEADER_TINT,
                fading_slots: 0,
            };
            let mut ui = Ui::begin(store, &input, config, text);
            let out = draw(&mut ui, view);
            let _ = ui.finish();
            out
        }

        /// A press and a release at `at` after the pointer got there: what each reported.
        fn press(
            self,
            store: &mut UiStore,
            text: &mut TextCache,
            at: (f32, f32),
        ) -> (ShellOut, ShellOut) {
            self.run(store, text, vec![]);
            self.run(store, text, vec![Event::PointerMoved { pos: at }]);
            self.run(store, text, vec![]);
            let down = self.run(store, text, vec![button(at, true)]);
            let up = self.run(store, text, vec![button(at, false)]);
            (down, up)
        }
    }

    fn centre(r: Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    /// The pane view's drag regions: the header's empty part and the room above the card drag; no
    /// button and no edge zone does.
    #[test]
    fn the_pane_header_and_the_room_above_the_card_drag_the_window() {
        for chrome in [MACOS_CHROME, LINUX_CHROME, WINDOWS_CHROME] {
            let layout = Layout::for_view(
                Rect::new(0.0, 0.0, 1040.0, 660.0),
                true,
                PaneShell::pane(chrome).view,
            );
            let pane = layout.pane.expect("pane");
            let above = (600.0, if chrome.drawn_caption { 17.0 } else { 7.0 });
            for at in [(150.0, 17.0), above] {
                let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
                let (down, _) = PaneShell::pane(chrome).press(&mut store, &mut text, at);
                assert_eq!(down.commands, vec![WindowCommand::DragMove], "{at:?}");
            }
            for at in [
                centre(pane::hide_button(&pane, chrome)),
                centre(layout.overflow),
                centre(layout.inbox),
                (245.0, 17.0),
                (245.0, 300.0),
                // The list area and the body are no drag regions either.
                (100.0, 300.0),
                (600.0, 300.0),
            ] {
                let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
                let (down, up) = PaneShell::pane(chrome).press(&mut store, &mut text, at);
                assert!(
                    down.commands.is_empty() && up.commands.is_empty(),
                    "{at:?}: {:?} {:?}",
                    down.commands,
                    up.commands
                );
            }
        }
    }

    #[test]
    fn the_panes_buttons_report_what_they_are() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        for page_open in [false, true] {
            let scene = PaneShell {
                page_open,
                ..PaneShell::pane(MACOS_CHROME)
            };
            let layout = Layout::for_view(Rect::new(0.0, 0.0, 1040.0, 660.0), true, scene.view);
            let pane = layout.pane.expect("pane");
            let (_, up) = scene.press(
                &mut store,
                &mut text,
                centre(pane::hide_button(&pane, MACOS_CHROME)),
            );
            assert!(up.pane_toggle && up.overflow.is_none() && !up.inbox_clicked);
            let (_, up) = scene.press(&mut store, &mut text, centre(layout.overflow));
            assert_eq!(up.overflow, Some(layout.overflow), "page {page_open}");
            assert!(!up.pane_toggle);
            // The footer's bell works over the page too (D31).
            let (_, up) = scene.press(&mut store, &mut text, centre(layout.inbox));
            assert!(up.inbox_clicked && !up.pane_toggle, "page {page_open}");
            assert!(up.tab_events.is_empty() && up.tab_context.is_none());
        }
    }

    /// With the drawn caption buttons the band over the terminal column maximizes on a double
    /// press, the buttons work, and the window's edges resize as in the bar view.
    #[test]
    fn a_borderless_pane_view_keeps_the_caption_buttons_and_the_resize_edges() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let scene = PaneShell::pane(WINDOWS_CHROME);
        let go = |store: &mut UiStore, text: &mut TextCache, events| {
            scene.run(store, text, events).commands
        };
        go(&mut store, &mut text, vec![]);
        for at in [(600.0, 17.0), (150.0, 17.0)] {
            let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
            go(&mut store, &mut text, vec![]);
            assert_eq!(
                go(&mut store, &mut text, vec![button(at, true)]),
                vec![WindowCommand::DragMove]
            );
            go(&mut store, &mut text, vec![button(at, false)]);
            assert_eq!(
                go(&mut store, &mut text, vec![button(at, true)]),
                vec![WindowCommand::ToggleMaximize],
                "{at:?}"
            );
        }
        for (x, want) in [
            (925.0, WindowCommand::Minimize),
            (971.0, WindowCommand::ToggleMaximize),
            (1017.0, WindowCommand::Close),
        ] {
            let (down, up) = scene.press(&mut store, &mut text, (x, 17.0));
            assert!(!down.commands.contains(&WindowCommand::DragMove));
            assert_eq!(up.commands, vec![want]);
        }
        for (at, edge) in [
            ((2.0, 300.0), ResizeEdge::West),
            ((100.0, 2.0), ResizeEdge::North),
            ((245.0, 658.0), ResizeEdge::South),
        ] {
            let (down, _) = scene.press(&mut store, &mut text, at);
            assert_eq!(down.commands, vec![WindowCommand::DragResize(edge)]);
            let held = down.pane_edge.is_some_and(|e| e.held || e.started);
            assert!(!held, "{at:?}: the window's edge, not the pane's");
        }
    }

    /// A drag of the pane's edge through real frames: the width follows the pointer, and the end
    /// of the drag asks for one save.
    #[test]
    fn dragging_the_panes_edge_widens_it_and_saves_once() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let scene = PaneShell::pane(MACOS_CHROME);
        let mut drag = None;
        let (mut width, mut saves) = (248u32, 0);
        let moved = |x| Event::PointerMoved { pos: (x, 300.0) };
        let frames = [
            vec![],
            vec![moved(245.0)],
            vec![],
            vec![button((245.0, 300.0), true)],
            vec![moved(275.0)],
            vec![moved(345.0)],
            vec![],
            vec![button((345.0, 300.0), false)],
            vec![],
        ];
        for events in frames {
            // The pane is where the width so far puts it.
            let scene = PaneShell {
                view: pane::resolve(true, width, 1040.0, MACOS_CHROME),
                ..scene
            };
            let out = scene.run(&mut store, &mut text, events);
            assert!(out.commands.is_empty(), "{:?}", out.commands);
            let step = pane::drag_step(&mut drag, out.pane_edge, width, 1040.0);
            width = step.width.unwrap_or(width);
            saves += usize::from(step.save);
        }
        assert_eq!((width, saves), (348, 1));
        assert!(drag.is_none());
    }

    #[test]
    fn the_bar_views_show_button_toggles_and_does_not_drag() {
        for chrome in [MACOS_CHROME, WINDOWS_CHROME, LINUX_CHROME] {
            let at = centre(pane::show_button(
                Rect::new(0.0, 0.0, 1040.0, 660.0),
                chrome,
            ));
            let scene = PaneShell {
                chrome,
                view: View::Bar,
                show_pane_button: true,
                inbox: false,
                page_open: false,
            };
            for page_open in [false, true] {
                let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
                let scene = PaneShell { page_open, ..scene };
                let (down, up) = scene.press(&mut store, &mut text, at);
                assert!(down.commands.is_empty(), "{:?}", down.commands);
                assert!(up.pane_toggle);
                assert_eq!(up.pane_edge, None);
            }
            // Without the button (a narrow window) the same place is the band.
            let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
            let none = PaneShell {
                show_pane_button: false,
                ..scene
            };
            let (down, up) = none.press(&mut store, &mut text, at);
            assert_eq!(down.commands, vec![WindowCommand::DragMove]);
            assert!(!up.pane_toggle);
        }
    }

    /// From the pane's footer the inbox list opens upward, left edges aligned, over the terminal.
    #[test]
    fn the_inbox_list_opens_upward_from_the_panes_footer() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let layout = Layout::for_view(
            Rect::new(0.0, 0.0, 1040.0, 660.0),
            true,
            PaneShell::pane(MACOS_CHROME).view,
        );
        let input = RawInput {
            size: (1040.0, 660.0),
            scale: 1.0,
            time: Duration::ZERO,
            focused: true,
            events: vec![],
        };
        let (strings, shortcuts) = (StringTable::default(), Shortcuts::default());
        let config = FrameConfig {
            theme: THEME,
            tier: MotionTier::Off,
            busy_cadence: Cadence::BUSY,
            strings: &strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &shortcuts,
        };
        let rows = some_rows(3);
        let rows: Vec<_> = rows.iter().map(RowText::row).collect();
        let mut ui = Ui::begin(&mut store, &input, config, &mut text);
        let list = draw_inbox(
            &mut ui,
            &layout,
            &InboxList {
                name: "Notifications",
                title: "Notifications",
                clear_all: "Clear all",
                rows: &rows,
            },
        );
        let _ = ui.finish();
        assert_eq!(list.rect.x, layout.inbox.x);
        assert_eq!(layout.inbox.y - list.rect.bottom(), INBOX_GAP);
        assert!(
            list.rect.right() > layout.card.x,
            "over the terminal column"
        );
        assert!(list.rect.y >= 0.0);
    }

    /// The strip's header: a press on empty space drags the window, a press on a tab activates it
    /// and does not drag.
    #[test]
    fn the_empty_strip_drags_the_window_and_a_tab_does_not() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events| frame(&mut store, &mut text, events, BORDERLESS, false);
        go(vec![]);
        go(vec![]);
        // Strip y is 34..66. Tab 1 is at x 10..118 (108 wide); far to the right is empty.
        let empty = (700.0, 50.0);
        // A real press follows a pointer move: the toolkit resolves what is under the pointer
        // from the frame before.
        go(vec![Event::PointerMoved { pos: empty }]);
        go(vec![]);
        let out = go(vec![button(empty, true)]);
        assert_eq!(out.commands, vec![WindowCommand::DragMove]);
        assert!(out.tab_events.is_empty(), "{:?}", out.tab_events);
        go(vec![button(empty, false)]);
        // Each press on empty space is a drag (the band is where a double press maximizes).
        let out = go(vec![button(empty, true)]);
        assert_eq!(out.commands, vec![WindowCommand::DragMove]);
        go(vec![button(empty, false)]);
        // The reserved right end drags too, beside the `⋯` button (998..1022).
        let out = go(vec![button((1026.0, 50.0), true)]);
        assert_eq!(out.commands, vec![WindowCommand::DragMove]);
        go(vec![button((1026.0, 50.0), false)]);
        // A press on the `⋯` button is the button's: it drags nothing, and the release asks for
        // the app menu at the button's rect.
        let more = (1010.0, 50.0);
        go(vec![Event::PointerMoved { pos: more }]);
        go(vec![]);
        let out = go(vec![button(more, true)]);
        assert!(out.commands.is_empty(), "{:?}", out.commands);
        let out = go(vec![button(more, false)]);
        assert_eq!(
            out.overflow,
            Some(Layout::new(Rect::new(0.0, 0.0, 1040.0, 660.0)).overflow)
        );
        go(vec![Event::PointerMoved { pos: (700.0, 50.0) }]);
        // A press on the second tab activates it; it must not also drag the window.
        let on_tab = (150.0, 50.0);
        go(vec![Event::PointerMoved { pos: on_tab }]);
        go(vec![]);
        let out = go(vec![button(on_tab, true)]);
        assert!(
            out.tab_events.contains(&TabEvent::Activate(TabKey(2))),
            "{:?}",
            out.tab_events
        );
        assert!(
            !out.commands.contains(&WindowCommand::DragMove),
            "{:?}",
            out.commands
        );
    }

    /// The traffic lights and the caption buttons live in the band, above the strip: nothing of
    /// the strip's header reaches into the band's rows.
    #[test]
    fn the_strip_stays_below_the_band_and_the_caption_buttons() {
        let l = Layout::new(Rect::new(0.0, 0.0, 1040.0, 660.0));
        assert!(l.strip.y >= l.band.bottom());
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events| frame(&mut store, &mut text, events, BORDERLESS, false);
        go(vec![]);
        go(vec![]);
        // A caption button press (band, top right) is the button's, not the header's.
        let press = go(vec![button((1017.0, 17.0), true)]);
        assert!(
            !press.commands.contains(&WindowCommand::DragMove),
            "{:?}",
            press.commands
        );
        assert_eq!(
            go(vec![button((1017.0, 17.0), false)]).commands,
            vec![WindowCommand::Close]
        );
    }

    #[test]
    fn the_new_tab_button_asks_for_a_tab() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut go = |events| frame(&mut store, &mut text, events, BORDERLESS, false);
        go(vec![]);
        go(vec![]);
        // The + button: 20 px, 6 px after the last tab (two tabs of 108 and a gap of 5 from x 10:
        // they end at 231), centred in its 32 px.
        let at = (10.0 + 108.0 + 5.0 + 108.0 + 6.0 + 10.0, 50.0);
        go(vec![Event::PointerMoved { pos: at }]);
        go(vec![]);
        let press = go(vec![button(at, true)]);
        assert!(
            press.commands.is_empty(),
            "no window drag from the + button: {:?}",
            press.commands
        );
        let out = go(vec![button(at, false)]);
        assert_eq!(out.tab_events, vec![TabEvent::New], "{:?}", out.tab_events);
        // The close glyph of the active tab (right end of tab 1: x 10..118, glyph 20 px, 8 from
        // the edge) closes, and is no drag either.
        let close = (118.0 - 8.0 - 10.0, 50.0);
        go(vec![Event::PointerMoved { pos: close }]);
        go(vec![]);
        let press = go(vec![button(close, true)]);
        assert!(
            press.commands.is_empty(),
            "no window drag from a close glyph: {:?}",
            press.commands
        );
        let out = go(vec![button(close, false)]);
        assert_eq!(
            out.tab_events,
            vec![TabEvent::Close(TabKey(1))],
            "{:?}",
            out.tab_events
        );
    }

    #[test]
    fn the_band_shows_the_app_name_alone_or_after_the_tab_label() {
        let theme = THEME;
        assert_eq!(band_title(None, &theme).runs.len(), 1);
        assert_eq!(band_title(None, &theme).runs[0].0, "Luneterm");
        let two = band_title(Some("zsh"), &theme);
        assert_eq!(two.runs.len(), 2);
        assert_eq!(two.runs[0].0, "zsh");
        assert_eq!(two.runs[1].0, "  \u{b7}  Luneterm");
    }

    // -------------------------------------------------------------------------- the inbox

    #[test]
    fn the_inbox_button_narrows_only_the_strip() {
        for win in [
            Rect::new(0.0, 0.0, 1040.0, 660.0),
            Rect::new(0.0, 0.0, 640.0, 400.0),
        ] {
            let (off, on) = (
                Layout::with_inbox(win, false),
                Layout::with_inbox(win, true),
            );
            assert_eq!(off, Layout::new(win), "hidden = today's layout");
            assert_eq!(
                on.body, off.body,
                "the terminal never resizes for the button"
            );
            assert_eq!(
                (on.band, on.card, on.overflow),
                (off.band, off.card, off.overflow)
            );
            assert_eq!(on.inbox, off.inbox);
            assert_eq!(on.strip.w, off.strip.w - (on.inbox.w + INBOX_GAP));
            assert_eq!(
                (on.strip.x, on.strip.y, on.strip.h),
                (off.strip.x, off.strip.y, off.strip.h)
            );
            // Same size as `⋯`, 4 px to its left, on its row.
            assert_eq!((on.inbox.w, on.inbox.h), (on.overflow.w, on.overflow.h));
            assert_eq!(on.overflow.x - on.inbox.right(), INBOX_GAP);
            assert_eq!(on.inbox.y, on.overflow.y);
            assert!(
                on.strip.right() <= on.inbox.x,
                "the strip stops before the button"
            );
        }
        // The first window: the exact numbers.
        let on = Layout::with_inbox(Rect::new(0.0, 0.0, 1040.0, 660.0), true);
        assert_eq!(on.inbox, Rect::new(970.0, 38.0, 24.0, 24.0));
        assert_eq!(on.strip, Rect::new(10.0, 34.0, 952.0, 32.0));
    }

    #[test]
    fn the_button_name_carries_the_count_and_is_plain_at_zero() {
        assert_eq!(inbox_button_name("Notifications", 0), "Notifications");
        assert_eq!(inbox_button_name("Notifications", 3), "Notifications (3)");
    }

    #[test]
    fn the_ring_swings_three_times_and_dies_out_in_one_second() {
        let ms = Duration::from_millis;
        assert_eq!(ring_angle(ms(0)), Some(0.0));
        // The first peak at a quarter of the first swing (83 ms): about 14 degrees times (1 - t).
        let peak = ring_angle(ms(83)).expect("inside the ring").to_degrees();
        let want = 14.0 * (1.0 - 0.083);
        assert!(peak > 0.0 && (peak - want).abs() < 1.0, "peak {peak}");
        // Three swings: the sign flips six times between 0 and 1000 ms.
        let flips = (1..1000u64)
            .map(|t| ring_angle(ms(t)).expect("inside the ring").signum())
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[0] != w[1])
            .count();
        assert_eq!(flips, 5, "five zero crossings inside the ring");
        // It shrinks to still, and is over at 1 s.
        let late = ring_angle(ms(999)).expect("inside the ring").to_degrees();
        assert!(late.abs() < 0.2, "late {late}");
        assert_eq!(ring_angle(ms(1000)), None);
        assert_eq!(ring_angle(ms(5000)), None);
    }

    #[test]
    fn a_ring_starts_with_no_ring_before_or_three_seconds_after_the_last() {
        let ms = Duration::from_millis;
        let t = ms(10_000);
        assert!(ring_due(None, t));
        assert!(!ring_due(Some(t), t));
        assert!(!ring_due(Some(t), t + ms(2999)));
        assert!(ring_due(Some(t), t + ms(3000)));
        // A clock that did not move forward never underflows.
        assert!(!ring_due(Some(t), ms(5)));
    }

    fn english() -> StringTable {
        crate::strings::english()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn age_labels_are_now_minutes_hours_days() {
        let t = english();
        let label = |s: u64| age_label(&t, notices::age(secs(s)));
        assert_eq!(label(0), "now");
        assert_eq!(label(59), "now");
        assert_eq!(label(120), "2 min");
        assert_eq!(label(3600), "1 h");
        assert_eq!(label(3 * 86_400), "3 d");
    }

    /// D16: bell, titled message, untitled message; read rows end in "seen".
    #[test]
    fn rows_follow_d16() {
        let t = english();
        let mut inbox = Inbox::new();
        let now = secs(10_000);
        inbox.message(4, "Build finished".into(), String::new(), secs(6_400));
        inbox.message(3, String::new(), "Plain message".into(), secs(9_000));
        inbox.bell(2, 1, secs(9_500));
        inbox.bell(5, 2, secs(9_700));
        inbox.message(3, "Claude Code".into(), "Needs Bash".into(), secs(9_950));
        inbox.settle(Some(4), true, |_| true);
        let names =
            |id: u64| Some(["", "zsh", "cargo build", "claude", "colors"][id as usize].to_owned());
        let mut rows = inbox_rows(&inbox, now, &t, |id| match id {
            5 => Some("five".to_owned()),
            id => names(id),
        });
        let texts: Vec<_> = rows
            .drain(..)
            .map(|(tab, r)| (tab, r.title, r.age, r.body, r.meta, r.unread))
            .collect();
        let row = |tab, title: &str, age: &str, body: Option<&str>, meta: Option<&str>, unread| {
            (
                tab,
                title.to_owned(),
                age.to_owned(),
                body.map(str::to_owned),
                meta.map(str::to_owned),
                unread,
            )
        };
        assert_eq!(
            texts,
            vec![
                row(
                    3,
                    "Claude Code",
                    "now",
                    Some("Needs Bash"),
                    Some("claude"),
                    true
                ),
                row(5, "Bell \u{d7}2", "5 min", None, Some("five"), true),
                row(2, "Bell", "8 min", None, Some("cargo build"), true),
                // No title: the tab's name is the first line, no meta while unread.
                row(3, "claude", "16 min", Some("Plain message"), None, true),
                // Read: dimmed by the toolkit, meta ends in "seen".
                row(
                    4,
                    "Build finished",
                    "1 h",
                    None,
                    Some("colors \u{b7} seen"),
                    false
                ),
            ]
        );
        // A read row with no other meta: "seen" alone.
        let mut one = Inbox::new();
        one.message(3, String::new(), "Plain".into(), secs(1));
        one.settle(Some(3), true, |_| true);
        let rows = inbox_rows(&one, secs(2), &t, |_| Some("claude".to_owned()));
        assert_eq!(rows[0].1.meta.as_deref(), Some("seen"));
        assert!(!rows[0].1.unread);
        // The tab's name is read now (D17): a rename shows.
        let rows = inbox_rows(&one, secs(2), &t, |_| Some("renamed".to_owned()));
        assert_eq!(rows[0].1.title, "renamed");
    }

    /// One frame of the shell with the inbox button, and the list when `open`, at `ms`.
    fn inbox_frame(
        store: &mut UiStore,
        text: &mut TextCache,
        rows: &[MessageRow<'_>],
        open: bool,
        events: Vec<Event>,
        ms: u64,
    ) -> (ShellOut, Option<InboxOut>) {
        let button = InboxButton {
            name: "Notifications (3)",
            unread: true,
            ring: None,
            open,
        };
        let (out, inbox, _, _) =
            button_frame(store, text, rows, Some(button), events, ms, MotionTier::Off);
        (out, inbox)
    }

    /// One frame of the shell with `button` (the list is drawn when it is `open`), at `ms`, under
    /// motion tier `tier`; returns the frame's scene and output too.
    fn button_frame(
        store: &mut UiStore,
        text: &mut TextCache,
        rows: &[MessageRow<'_>],
        button: Option<InboxButton<'_>>,
        events: Vec<Event>,
        ms: u64,
        tier: MotionTier,
    ) -> (
        ShellOut,
        Option<InboxOut>,
        lune_ui::scene::Scene,
        lune_ui::input::FrameOutput,
    ) {
        let open = button.is_some_and(|b| b.open);
        let input = RawInput {
            size: (1040.0, 660.0),
            scale: 1.0,
            time: Duration::from_millis(ms),
            focused: true,
            events,
        };
        let (strings, shortcuts) = (StringTable::default(), Shortcuts::default());
        let config = FrameConfig {
            theme: THEME,
            tier,
            busy_cadence: Cadence::BUSY,
            strings: &strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &shortcuts,
        };
        let tabs = [TAB_A, TAB_B];
        let view = ShellView {
            title: None,
            maximized: false,
            focused: true,
            scroll: None,
            cell_h: 17.0,
            chrome: HOST,
            view: View::Bar,
            show_pane_button: false,
            pane_button_name: "Pane",
            tabs: &tabs,
            active_tab: Some(1),
            modal: None,
            toasts: None,
            page_open: false,
            busy: None,
            busy_tone: Tone::Neutral,
            overflow_name: "More",
            inbox_button: button,
            header_tint: HEADER_TINT,
            fading_slots: 0,
        };
        let mut ui = Ui::begin(store, &input, config, text);
        let out = draw(&mut ui, view);
        let layout = Layout::new(Rect::new(0.0, 0.0, 1040.0, 660.0));
        let list = InboxList {
            name: "Notifications",
            title: "Notifications",
            clear_all: "Clear all",
            rows,
        };
        let inbox = open.then(|| draw_inbox(&mut ui, &layout, &list));
        let (scene, frame) = ui.finish();
        (out, inbox, scene, frame)
    }

    fn some_rows(n: usize) -> Vec<RowText> {
        (0..n)
            .map(|i| RowText {
                title: format!("Title {i}"),
                age: "now".to_owned(),
                body: Some(format!("Body {i}")),
                meta: Some("claude".to_owned()),
                unread: true,
            })
            .collect()
    }

    /// Real frames: the button toggles through `inbox_clicked`; the popover sits 4 px under the
    /// button with its right edge on the button's, 360 wide, as tall as its content up to the
    /// smaller of 480 and the body's height; a row press, Clear all, an outside press and
    /// Escape each report what they are.
    #[test]
    fn the_inbox_button_and_list_report_presses_and_sit_under_the_button() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let layout = Layout::new(Rect::new(0.0, 0.0, 1040.0, 660.0));
        let centre = |r: Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        // The first frame registers the regions; a press and release on the button click it.
        let go = |store: &mut UiStore, text: &mut TextCache, open, events, ms| {
            let rows = some_rows(3);
            let rows: Vec<_> = rows.iter().map(RowText::row).collect();
            inbox_frame(store, text, &rows, open, events, ms)
        };
        let (out, _) = go(&mut store, &mut text, false, vec![], 0);
        assert!(!out.inbox_clicked);
        go(
            &mut store,
            &mut text,
            false,
            vec![button(centre(layout.inbox), true)],
            10,
        );
        let (out, _) = go(
            &mut store,
            &mut text,
            false,
            vec![button(centre(layout.inbox), false)],
            20,
        );
        assert!(out.inbox_clicked, "a press on the button");

        // Open: geometry of the popover.
        let (_, list) = go(&mut store, &mut text, true, vec![], 100);
        let list = list.expect("the list is drawn");
        assert_eq!(list.rect.right(), layout.inbox.right());
        assert_eq!(list.rect.y - layout.inbox.bottom(), INBOX_GAP);
        assert_eq!(list.rect.w, INBOX_WIDTH);
        assert!(
            list.rect.h > HEADER_HEIGHT && list.rect.h < INBOX_MAX_HEIGHT,
            "{:?}",
            list.rect
        );
        assert!(list.dismissed.is_none() && list.clicked.is_none() && !list.clear_all);

        // A row press: the first row's middle.
        let row0 = (list.rect.x + 100.0, list.rect.y + HEADER_HEIGHT + 20.0);
        go(&mut store, &mut text, true, vec![button(row0, true)], 110);
        let (_, hit) = go(&mut store, &mut text, true, vec![button(row0, false)], 120);
        assert_eq!(hit.expect("list").clicked, Some(0));

        // Clear all, in the header's right end.
        let clear = (list.rect.right() - 40.0, list.rect.y + HEADER_HEIGHT / 2.0);
        go(&mut store, &mut text, true, vec![button(clear, true)], 130);
        let (_, hit) = go(&mut store, &mut text, true, vec![button(clear, false)], 140);
        assert!(hit.expect("list").clear_all);

        // A press outside closes it (and is not a click on anything).
        let outside = (300.0, 400.0);
        let (_, hit) = go(
            &mut store,
            &mut text,
            true,
            vec![button(outside, true)],
            150,
        );
        let hit = hit.expect("list");
        assert!(hit.dismissed.is_some(), "an outside press dismisses");
        assert!(hit.clicked.is_none() && !hit.clear_all);
    }

    #[test]
    fn the_list_is_as_tall_as_its_rows_up_to_the_cap_then_scrolls() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let many = some_rows(50);
        let rows: Vec<_> = many.iter().map(RowText::row).collect();
        let (_, list) = inbox_frame(&mut store, &mut text, &rows, true, vec![], 0);
        assert_eq!(list.expect("list").rect.h, INBOX_MAX_HEIGHT);
        // The body's height is the other cap.
        let few = some_rows(1);
        let rows: Vec<_> = few.iter().map(RowText::row).collect();
        let (_, list) = inbox_frame(&mut store, &mut text, &rows, true, vec![], 0);
        assert!(list.expect("list").rect.h < 120.0);
    }

    fn bell_button(unread: bool, ring: Option<Duration>) -> InboxButton<'static> {
        InboxButton {
            name: "Notifications (3)",
            unread,
            ring,
            open: false,
        }
    }

    /// Two warm frames (the second is the one judged), the button as given, tier and time as given.
    fn bell_frames(
        button: Option<InboxButton<'_>>,
        tier: MotionTier,
        ms: u64,
    ) -> (ShellOut, lune_ui::scene::Scene, lune_ui::input::FrameOutput) {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let _ = button_frame(&mut store, &mut text, &[], button, vec![], ms, tier);
        let (out, _, scene, frame) =
            button_frame(&mut store, &mut text, &[], button, vec![], ms, tier);
        (out, scene, frame)
    }

    fn texts_in(scene: &lune_ui::scene::Scene) -> usize {
        scene.in_order().iter().map(|l| l.texts.len()).sum()
    }

    /// No badge (D19): the button adds one text to the frame (its glyph), not a pill with a count;
    /// the accessible name still carries the count.
    #[test]
    fn the_bell_has_no_badge_and_its_name_keeps_the_count() {
        let tier = MotionTier::Full;
        let (_, hidden, _) = bell_frames(None, tier, 5000);
        let (_, shown, frame) = bell_frames(Some(bell_button(true, None)), tier, 5000);
        assert_eq!(texts_in(&shown), texts_in(&hidden) + 1);
        assert!(
            frame
                .tree
                .nodes
                .iter()
                .any(|n| n.name == "Notifications (3)"),
            "the count stays in the name"
        );
        // Read: the same one text, the same name.
        let (_, read, _) = bell_frames(Some(bell_button(false, None)), tier, 5000);
        assert_eq!(texts_in(&read), texts_in(&hidden) + 1);
    }

    /// The ring draws the swinging glyph over the button (one more text, the button's own glyph
    /// is empty) and asks for the next frame; at rest, read, past the ring and under Reduced or
    /// Off nothing is asked and the glyph is the button's own. The button's rect never moves.
    #[test]
    fn the_ring_asks_for_frames_only_while_it_runs_and_never_moves_the_button() {
        let ms = Duration::from_millis;
        let start = Some(ms(5000));
        let full = MotionTier::Full;
        let rest = bell_frames(Some(bell_button(true, None)), full, 5083);
        assert_eq!(
            rest.2.repaint,
            lune_ui::input::Repaint::Idle,
            "yellow at rest"
        );
        let ringing = bell_frames(Some(bell_button(true, start)), full, 5083);
        match ringing.2.repaint {
            lune_ui::input::Repaint::At(t) => assert!(t <= ms(5084), "asked for {t:?}"),
            other => panic!("a ringing bell asks for the next frame, got {other:?}"),
        }
        assert_eq!(texts_in(&ringing.1), texts_in(&rest.1), "one glyph, turned");
        assert_eq!(ringing.0.body, rest.0.body);
        let node = |f: &lune_ui::input::FrameOutput| {
            f.tree
                .nodes
                .iter()
                .find(|n| n.name == "Notifications (3)")
                .map(|n| n.bounds)
        };
        assert_eq!(node(&ringing.2), node(&rest.2));
        assert!(node(&rest.2).is_some());
        // Over (1 s after the start), read mid-ring, and the other tiers: nothing asked.
        for (button, tier, at) in [
            (bell_button(true, start), full, 6000),
            (bell_button(false, start), full, 5083),
            (bell_button(true, start), MotionTier::Reduced, 5083),
            (bell_button(true, start), MotionTier::Off, 5083),
        ] {
            let (_, _, frame) = bell_frames(Some(button), tier, at);
            assert_eq!(
                frame.repaint,
                lune_ui::input::Repaint::Idle,
                "{button:?} {tier:?} {at}"
            );
        }
    }

    // --------------------------------------------------------------------- the group dialogs

    /// One frame of the shell with `modal` over it, with the app's strings: its answer and tree.
    fn modal_frame(
        store: &mut UiStore,
        text: &mut TextCache,
        events: Vec<Event>,
        modal: Modal<'_>,
    ) -> (ModalOut, lune_ui::access::SemanticTree) {
        let input = RawInput {
            size: (1040.0, 660.0),
            scale: 1.0,
            time: Duration::ZERO,
            focused: true,
            events,
        };
        let (strings, shortcuts) = (crate::strings::english(), Shortcuts::default());
        let config = FrameConfig {
            theme: THEME,
            tier: MotionTier::Off,
            busy_cadence: Cadence::BUSY,
            strings: &strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &shortcuts,
        };
        let tabs = [TAB_A];
        let view = ShellView {
            title: None,
            maximized: false,
            focused: true,
            scroll: None,
            cell_h: 17.0,
            chrome: HOST,
            view: View::Bar,
            show_pane_button: false,
            pane_button_name: "Pane",
            tabs: &tabs,
            active_tab: Some(1),
            modal: Some(modal),
            toasts: None,
            page_open: false,
            busy: None,
            busy_tone: Tone::Neutral,
            overflow_name: "More",
            inbox_button: None,
            header_tint: HEADER_TINT,
            fading_slots: 0,
        };
        let mut ui = Ui::begin(store, &input, config, text);
        let out = draw(&mut ui, view);
        let (_, frame) = ui.finish();
        (out.modal.expect("a dialog is drawn"), frame.tree)
    }

    /// The group editor held open over frames, as the app holds it.
    struct Editor {
        store: UiStore,
        text: TextCache,
        name: String,
        color: GroupColor,
        taken: Vec<String>,
        editing: bool,
    }

    impl Editor {
        fn new(editing: bool, name: &str, taken: &[&str]) -> Editor {
            let mut e = Editor {
                store: UiStore::default(),
                text: fonts::bundled_text_cache(),
                name: name.to_owned(),
                color: GroupColor::Grey,
                taken: taken.iter().map(|t| (*t).to_owned()).collect(),
                editing,
            };
            // As the app: the open focus, then the caret into the field.
            e.go(vec![], false);
            e.go(vec![], true);
            e.go(vec![], false);
            e
        }

        fn go(
            &mut self,
            events: Vec<Event>,
            focus: bool,
        ) -> (ModalOut, lune_ui::access::SemanticTree) {
            let modal = Modal::GroupEditor {
                editing: self.editing,
                text: &mut self.name,
                color: &mut self.color,
                taken: &self.taken,
                look: LookTheme::Luneterm,
                focus,
            };
            modal_frame(&mut self.store, &mut self.text, events, modal)
        }

        /// A click at the middle of the node called `name`, and the answer of the release.
        fn click(&mut self, name: &str) -> ModalOut {
            let (_, tree) = self.go(vec![], false);
            let b = tree
                .nodes
                .iter()
                .find(|n| n.name == name)
                .unwrap_or_else(|| panic!("no node {name}"))
                .bounds;
            let at = (b.x + b.w / 2.0, b.y + b.h / 2.0);
            self.go(vec![Event::PointerMoved { pos: at }], false);
            self.go(vec![button(at, true)], false);
            let out = self.go(vec![button(at, false)], false).0;
            self.go(vec![], false);
            out
        }

        fn key(&mut self, key: NamedKey) -> ModalOut {
            let events = [true, false]
                .map(|pressed| Event::Key {
                    key: lune_ui::input::Key::Named(key),
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::default(),
                })
                .to_vec();
            self.go(events, false).0
        }
    }

    const NOTHING: ModalOut = ModalOut {
        confirmed: false,
        cancelled: false,
    };

    #[test]
    fn the_editor_confirms_a_free_name_by_button_and_by_enter() {
        for (editing, label) in [(false, "Create"), (true, "Save")] {
            let mut e = Editor::new(editing, "api", &["infra"]);
            assert!(e.click(label).confirmed, "{label}");
            let mut e = Editor::new(editing, "api", &["infra"]);
            assert!(e.key(NamedKey::Enter).confirmed, "Enter, {label}");
            assert!(e.click("Cancel").cancelled);
        }
    }

    #[test]
    fn the_editor_takes_no_name_it_cannot_use() {
        let long = "x".repeat(crate::ctl::MAX_GROUP_CHARS + 1);
        for bad in ["", "infra", " api", "api ", "a\u{202e}b", &long] {
            let mut e = Editor::new(false, bad, &["infra"]);
            assert_eq!(e.click("Create"), NOTHING, "{bad:?}");
            assert_eq!(e.key(NamedKey::Enter), NOTHING, "Enter, {bad:?}");
            assert_eq!(e.name, bad, "never trimmed or cut");
            assert!(e.click("Cancel").cancelled, "{bad:?}");
        }
    }

    #[test]
    fn typing_makes_the_name_usable_and_the_confirm_button_follows() {
        let mut e = Editor::new(false, "infr", &["infra"]);
        let (out, _) = e.go(vec![Event::Text("a".to_owned())], false);
        assert_eq!((e.name.as_str(), out), ("infra", NOTHING));
        let (_, tree) = e.go(vec![], false);
        let create = tree
            .nodes
            .iter()
            .find(|n| n.name == "Create")
            .expect("button");
        assert!(create.states.contains(State::Disabled), "taken");
        e.go(vec![Event::Text("2".to_owned())], false);
        assert!(e.click("Create").confirmed);
        assert_eq!(e.name, "infra2");
    }

    #[test]
    fn the_hint_says_why_the_name_cannot_be_used_and_nothing_for_an_empty_one() {
        for (problem, key) in [
            (Some(NameProblem::Taken), Some("group.editor.taken")),
            (Some(NameProblem::TooLong), Some("group.editor.tooLong")),
            (Some(NameProblem::EdgeSpace), Some("group.editor.edgeSpace")),
            (Some(NameProblem::Control), Some("group.editor.control")),
            (Some(NameProblem::Empty), None),
            (None, None),
        ] {
            assert_eq!(name_hint(problem), key, "{problem:?}");
        }
        let table = english();
        assert!(
            table
                .tr("group.editor.tooLong")
                .contains(&crate::ctl::MAX_GROUP_CHARS.to_string())
        );
        for key in group_required_keys() {
            assert_ne!(table.tr(&key), key, "{key} has no text");
        }
    }

    #[test]
    fn the_swatches_are_nine_radios_and_a_press_or_a_key_picks_one() {
        let mut e = Editor::new(false, "api", &[]);
        let (_, tree) = e.go(vec![], false);
        let names: Vec<(&str, bool)> = tree
            .nodes
            .iter()
            .filter(|n| n.role == Role::Radio)
            .map(|n| (n.name.as_str(), n.states.contains(State::Checked)))
            .collect();
        assert_eq!(
            names,
            [
                ("Grey", true),
                ("Blue", false),
                ("Red", false),
                ("Yellow", false),
                ("Green", false),
                ("Pink", false),
                ("Purple", false),
                ("Cyan", false),
                ("Orange", false),
            ]
        );
        assert_eq!(e.click("Purple"), NOTHING, "a colour is no answer");
        assert_eq!(e.color, GroupColor::Purple);
        // Space and Enter on the focused swatch choose it and do not answer the dialog.
        assert_eq!(e.click("Red"), NOTHING);
        for key in [NamedKey::Space, NamedKey::Enter] {
            e.color = GroupColor::Grey;
            assert_eq!(e.key(key), NOTHING, "{key:?}");
            assert_eq!(e.color, GroupColor::Red, "{key:?}");
        }
        assert_eq!(e.name, "api", "the keys went to the swatch, not the field");
    }

    #[test]
    fn the_swatch_row_is_as_wide_as_nine_circles_and_eight_gaps() {
        let first = swatch_rect(0, 100.0, 10.0);
        let last = swatch_rect(8, 100.0, 10.0);
        assert_eq!(first, Rect::new(100.0, 10.0, SWATCH, SWATCH));
        assert_eq!(last.right() - first.x, 9.0 * SWATCH + 8.0 * SWATCH_GAP);
        // The rings of neighbours do not touch.
        const { assert!(2.0 * SWATCH_RING < SWATCH_GAP) };
    }

    #[test]
    fn the_close_question_has_the_checkbox_only_when_the_answer_may_be_remembered() {
        let (mut store, mut text) = (UiStore::default(), fonts::bundled_text_cache());
        let mut remember = false;
        let mut count = |dont_ask: Option<&mut bool>| {
            let modal = Modal::Confirm {
                title: "Close Group",
                message: "m",
                dont_ask,
            };
            let (_, tree) = modal_frame(&mut store, &mut text, vec![], modal);
            tree.nodes
                .iter()
                .filter(|n| n.role == Role::Checkbox)
                .count()
        };
        assert_eq!(count(Some(&mut remember)), 1);
        assert_eq!(count(None), 0);
    }

    #[test]
    fn the_close_group_message_counts_the_tabs_and_names_the_group() {
        let t = english();
        assert_eq!(
            close_group_message(&t, "api", 3),
            "Close the group \u{201c}api\u{201d} and its 3 tabs?"
        );
        assert_eq!(
            close_group_message(&t, "{1}", 1),
            "Close the group \u{201c}{1}\u{201d} and its tab?"
        );
    }
}
