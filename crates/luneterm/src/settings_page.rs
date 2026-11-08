//! The settings page (plan M10 step 6): a nav column (Back, the three sections) on the card and the
//! toolkit's `Ui::settings_page` for the content column. The page replaces the whole card while it
//! is open. Every [`Settings`] field has a row here; the row's [`SettingId`] says what changing it
//! does (`settings::Effect`), and the app applies that, not this module.
//!
//! The model (sections, groups, rows, option lists, how a stepper steps) is plain data and
//! functions, tested without a window; [`draw`] is the one function that touches the `Ui`.

use lune_ui::color::Rgba;
use lune_ui::icon;
use lune_ui::paint::Rect;
use lune_ui::scene::LayerId;
use lune_ui::strings::{DisplayFormat, StringTable};
use lune_ui::text::{Spec, Style};
use lune_ui::ui::Ui;
use lune_ui::widgets::field::FieldOptions;
use lune_ui::widgets::nav_item::NavEntry;
use lune_ui::widgets::settings_page::SettingsPage;
use lune_ui::widgets::stepper::StepRange;

use crate::fonts;
use crate::palettes::PALETTES;
use crate::session::MAX_SCROLLBACK;
use crate::settings::{
    BlurLevel, GlassStyle, MAX_FONT_SIZE, MAX_NAME_BYTES, MAX_SHELL_BYTES, MIN_FONT_SIZE, Motion,
    OutlineAnim, SettingId, Settings, text_ok,
};
use crate::shell::HOST_GLASS;

/// The card's corner radius.
const CARD_RADIUS: f32 = 8.0;
/// The nav column's width, and the control widths (the spike's).
const NAV_W: f32 = 190.0;
const TOGGLE_W: f32 = 40.0;
const TOGGLE_H: f32 = 20.0;
const COMBO_W: f32 = 230.0;
const STEPPER_W: f32 = 130.0;
const FIELD_W: f32 = 240.0;
const CONTROL_H: f32 = 32.0;
/// The section title's size.
const TITLE_SIZE: f32 = 18.0;
/// The most rows a dropdown shows before it scrolls.
const MAX_LIST_ROWS: usize = 8;
/// How far one stepper click moves the scrollback.
const SCROLLBACK_STEP: u32 = 1000;

/// The OS the glass descriptions and options are written for.
#[cfg(target_os = "macos")]
const OS: &str = "macos";
#[cfg(target_os = "windows")]
const OS: &str = "windows";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const OS: &str = "linux";

// ----------------------------------------------------------------------------------- the model

/// A page of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Appearance,
    Terminal,
    About,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Appearance, Section::Terminal, Section::About];

    fn key(self) -> &'static str {
        match self {
            Section::Appearance => "settings.section.appearance",
            Section::Terminal => "settings.section.terminal",
            Section::About => "settings.section.about",
        }
    }
}

/// The control a row has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Toggle,
    Combo,
    Stepper,
    Field,
}

impl Control {
    fn width(self) -> f32 {
        match self {
            Control::Toggle => TOGGLE_W,
            Control::Combo => COMBO_W,
            Control::Stepper => STEPPER_W,
            Control::Field => FIELD_W,
        }
    }

    fn height(self) -> f32 {
        if self == Control::Toggle {
            TOGGLE_H
        } else {
            CONTROL_H
        }
    }
}

/// One row: a setting, its control and the key its strings start with (`<key>.title`,
/// `<key>.desc`, and `<key>.placeholder` for a field).
#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub id: SettingId,
    pub control: Control,
    pub key: &'static str,
}

/// A bordered group of rows under a header.
#[derive(Debug, Clone, Copy)]
pub struct Group {
    pub header: &'static str,
    pub rows: &'static [Row],
}

const fn row(id: SettingId, control: Control, key: &'static str) -> Row {
    Row { id, control, key }
}

static APPEARANCE: [Group; 2] = [
    Group {
        header: "settings.appearance.window.header",
        rows: &[
            row(
                SettingId::Glass,
                Control::Combo,
                "settings.appearance.glass",
            ),
            row(
                SettingId::MenuBlur,
                Control::Combo,
                "settings.appearance.menuBlur",
            ),
        ],
    },
    Group {
        header: "settings.appearance.animations.header",
        rows: &[row(
            SettingId::Motion,
            Control::Combo,
            "settings.appearance.motion",
        )],
    },
];

static TERMINAL: [Group; 3] = [
    Group {
        header: "settings.terminal.appearance.header",
        rows: &[
            row(
                SettingId::Font,
                Control::Combo,
                "settings.terminal.appearance.font",
            ),
            row(
                SettingId::FontSize,
                Control::Stepper,
                "settings.terminal.appearance.fontSize",
            ),
            row(
                SettingId::Palette,
                Control::Combo,
                "settings.terminal.appearance.colorTheme",
            ),
            row(
                SettingId::AnimateOutline,
                Control::Toggle,
                "settings.terminal.appearance.runningOutline",
            ),
            row(
                SettingId::Outline,
                Control::Combo,
                "settings.terminal.appearance.runningOutlineStyle",
            ),
            row(
                SettingId::OutlineFullScreen,
                Control::Toggle,
                "settings.terminal.appearance.runningOutlineFullScreen",
            ),
        ],
    },
    Group {
        header: "settings.terminal.session.header",
        rows: &[
            row(
                SettingId::Scrollback,
                Control::Stepper,
                "settings.terminal.session.scrollback",
            ),
            row(
                SettingId::DefaultCwd,
                Control::Field,
                "settings.terminal.session.defaultCwd",
            ),
            row(
                SettingId::Shell,
                Control::Field,
                "settings.terminal.session.shell",
            ),
            row(
                SettingId::ShellArgs,
                Control::Field,
                "settings.terminal.session.shellArgs",
            ),
            row(
                SettingId::ReportIterm2,
                Control::Toggle,
                "settings.terminal.session.iterm2Compat",
            ),
            row(
                SettingId::ShellIntegration,
                Control::Toggle,
                "settings.terminal.session.shellIntegration",
            ),
        ],
    },
    Group {
        header: "settings.terminal.behavior.header",
        rows: &[
            row(
                SettingId::ConfirmClose,
                Control::Toggle,
                "settings.terminal.behavior.confirmClose",
            ),
            row(
                SettingId::OscClipboard,
                Control::Toggle,
                "settings.terminal.behavior.oscClipboard",
            ),
            row(
                SettingId::OptionAsMeta,
                Control::Toggle,
                "settings.terminal.behavior.optionAsMeta",
            ),
        ],
    },
];

/// The groups of a section (the About section has none: it is text).
pub fn groups(section: Section) -> &'static [Group] {
    match section {
        Section::Appearance => &APPEARANCE,
        Section::Terminal => &TERMINAL,
        Section::About => &[],
    }
}

/// The notices of the About section: `(name key, licence key)`. Only what this build carries.
const NOTICES: [(&str, &str); 6] = [
    (
        "settings.about.notice.xtermjs",
        "settings.about.notice.xtermjs.license",
    ),
    (
        "settings.about.notice.avalonia",
        "settings.about.notice.avalonia.license",
    ),
    (
        "settings.about.notice.xtermnet",
        "settings.about.notice.xtermnet.license",
    ),
    (
        "settings.about.notice.roboto",
        "settings.about.notice.roboto.license",
    ),
    (
        "settings.about.notice.rznsans",
        "settings.about.notice.rznsans.license",
    ),
    (
        "settings.about.notice.platform",
        "settings.about.notice.platform.license",
    ),
];

/// The key of the description of `row`: the glass row's depends on the OS.
fn desc_key(row: &Row) -> String {
    if row.id == SettingId::Glass {
        format!("{}.desc.{OS}", row.key)
    } else {
        format!("{}.desc", row.key)
    }
}

/// The keys of the option list of a combo row, in the order of [`selected`] and [`choose`].
fn option_keys(id: SettingId) -> Vec<String> {
    let numbered = |base: &str, n: usize| (0..n).map(|i| format!("{base}.{i}")).collect();
    match id {
        SettingId::Glass if !HOST_GLASS => vec![format!(
            "settings.appearance.glass.option.{OS}.{}",
            GlassStyle::ALL.len() - 1
        )],
        SettingId::Glass => numbered(
            &format!("settings.appearance.glass.option.{OS}"),
            GlassStyle::ALL.len(),
        ),
        SettingId::MenuBlur => {
            numbered("settings.appearance.menuBlur.option", BlurLevel::ALL.len())
        }
        SettingId::Motion => numbered("settings.appearance.motion.option", Motion::ALL.len()),
        SettingId::Outline => numbered(
            "settings.terminal.appearance.runningOutlineStyle.option",
            OutlineAnim::ALL.len(),
        ),
        _ => Vec::new(),
    }
}

/// The labels of a combo row's options.
pub fn options(strings: &StringTable, id: SettingId) -> Vec<String> {
    match id {
        SettingId::Palette => PALETTES.iter().map(|p| p.name.to_owned()).collect(),
        SettingId::Font => fonts::TERMINAL_FONTS
            .iter()
            .map(|f| (*f).to_owned())
            .collect(),
        _ => option_keys(id)
            .iter()
            .map(|k| strings.tr(k).to_owned())
            .collect(),
    }
}

/// The index in [`options`] of the value `settings` holds for `id`.
pub fn selected(settings: &Settings, id: SettingId) -> usize {
    let pos = |all: &[&str], name: &str| all.iter().position(|n| *n == name).unwrap_or(0);
    match id {
        // Linux has the one option, Off.
        SettingId::Glass if !HOST_GLASS => 0,
        SettingId::Glass => GlassStyle::ALL
            .iter()
            .position(|g| *g == settings.glass)
            .unwrap_or(0),
        SettingId::MenuBlur => BlurLevel::ALL
            .iter()
            .position(|b| *b == settings.menu_blur)
            .unwrap_or(0),
        SettingId::Motion => Motion::ALL
            .iter()
            .position(|m| *m == settings.motion)
            .unwrap_or(0),
        SettingId::Outline => OutlineAnim::ALL
            .iter()
            .position(|o| *o == settings.outline)
            .unwrap_or(0),
        SettingId::Palette => {
            let names: Vec<&str> = PALETTES.iter().map(|p| p.name).collect();
            pos(&names, &settings.palette)
        }
        SettingId::Font => pos(fonts::TERMINAL_FONTS, &settings.font),
        _ => 0,
    }
}

/// Sets the value of option `i` of `id`; an index past the list changes nothing.
pub fn choose(settings: &mut Settings, id: SettingId, i: usize) {
    match id {
        SettingId::Glass if !HOST_GLASS => {}
        SettingId::Glass => {
            if let Some(g) = GlassStyle::ALL.get(i) {
                settings.glass = *g;
            }
        }
        SettingId::MenuBlur => {
            if let Some(b) = BlurLevel::ALL.get(i) {
                settings.menu_blur = *b;
            }
        }
        SettingId::Motion => {
            if let Some(m) = Motion::ALL.get(i) {
                settings.motion = *m;
            }
        }
        SettingId::Outline => {
            if let Some(o) = OutlineAnim::ALL.get(i) {
                settings.outline = *o;
            }
        }
        SettingId::Palette => {
            if let Some(p) = PALETTES.get(i) {
                settings.palette = p.name.to_owned();
            }
        }
        SettingId::Font => {
            // The first font is the default, saved as the empty name.
            if let Some(f) = fonts::TERMINAL_FONTS.get(i) {
                settings.font = if i == 0 {
                    String::new()
                } else {
                    (*f).to_owned()
                };
            }
        }
        _ => {}
    }
}

/// The flag a toggle row shows.
pub fn flag(settings: &Settings, id: SettingId) -> Option<bool> {
    Some(match id {
        SettingId::AnimateOutline => settings.animate_outline,
        SettingId::OutlineFullScreen => settings.outline_full_screen,
        SettingId::ReportIterm2 => settings.report_iterm2,
        SettingId::ShellIntegration => settings.shell_integration,
        SettingId::ConfirmClose => settings.confirm_close,
        SettingId::OscClipboard => settings.osc_clipboard,
        SettingId::OptionAsMeta => settings.option_as_meta,
        _ => return None,
    })
}

/// Sets the flag of a toggle row; any other id changes nothing.
pub fn set_flag(settings: &mut Settings, id: SettingId, on: bool) {
    match id {
        SettingId::AnimateOutline => settings.animate_outline = on,
        SettingId::OutlineFullScreen => settings.outline_full_screen = on,
        SettingId::ReportIterm2 => settings.report_iterm2 = on,
        SettingId::ShellIntegration => settings.shell_integration = on,
        SettingId::ConfirmClose => settings.confirm_close = on,
        SettingId::OscClipboard => settings.osc_clipboard = on,
        SettingId::OptionAsMeta => settings.option_as_meta = on,
        _ => {}
    }
}

/// The number a stepper row shows and its range.
pub fn number(settings: &Settings, id: SettingId) -> Option<(u32, StepRange)> {
    let range = |min: u32, max: u32, step: u32| StepRange {
        min: f64::from(min),
        max: f64::from(max),
        step: f64::from(step),
    };
    let max_scrollback = u32::try_from(MAX_SCROLLBACK).unwrap_or(u32::MAX);
    match id {
        SettingId::FontSize => Some((settings.font_size, range(MIN_FONT_SIZE, MAX_FONT_SIZE, 1))),
        SettingId::Scrollback => Some((
            settings.scrollback,
            range(0, max_scrollback, SCROLLBACK_STEP),
        )),
        _ => None,
    }
}

/// The value after `steps` clicks on a stepper row (negative is minus), kept in its range.
pub fn step_number(id: SettingId, value: u32, steps: i32) -> u32 {
    let max_scrollback = u32::try_from(MAX_SCROLLBACK).unwrap_or(u32::MAX);
    let (min, max, by) = match id {
        SettingId::FontSize => (MIN_FONT_SIZE, MAX_FONT_SIZE, 1),
        SettingId::Scrollback => (0, max_scrollback, SCROLLBACK_STEP),
        _ => return value,
    };
    let moved = i64::from(value) + i64::from(steps) * i64::from(by);
    u32::try_from(moved.clamp(i64::from(min), i64::from(max))).unwrap_or(min)
}

/// Sets the number of a stepper row, kept in its range.
pub fn set_number(settings: &mut Settings, id: SettingId, value: u32) {
    let Some((_, range)) = number(settings, id) else {
        return;
    };
    let value = value.clamp(range.min as u32, range.max as u32);
    match id {
        SettingId::FontSize => settings.font_size = value,
        SettingId::Scrollback => settings.scrollback = value,
        _ => {}
    }
}

/// The longest a text row may be, in bytes.
fn text_max(id: SettingId) -> usize {
    match id {
        SettingId::DefaultCwd => crate::cwd::MAX_CWD_BYTES,
        SettingId::Shell | SettingId::ShellArgs => MAX_SHELL_BYTES,
        _ => MAX_NAME_BYTES,
    }
}

/// Which text buffer a field row edits.
fn text_slot(id: SettingId) -> Option<usize> {
    match id {
        SettingId::DefaultCwd => Some(0),
        SettingId::Shell => Some(1),
        SettingId::ShellArgs => Some(2),
        _ => None,
    }
}

/// The text a field row holds.
pub fn text(settings: &Settings, id: SettingId) -> Option<&str> {
    match id {
        SettingId::DefaultCwd => Some(&settings.default_cwd),
        SettingId::Shell => Some(&settings.shell),
        SettingId::ShellArgs => Some(&settings.shell_args),
        _ => None,
    }
}

/// Sets the text of a field row when it is acceptable (length and no control character); returns
/// whether it was taken. A refused text leaves the setting as it was.
pub fn set_text(settings: &mut Settings, id: SettingId, value: &str) -> bool {
    if !text_ok(value, text_max(id)) {
        return false;
    }
    match id {
        SettingId::DefaultCwd => settings.default_cwd = value.to_owned(),
        SettingId::Shell => settings.shell = value.to_owned(),
        SettingId::ShellArgs => settings.shell_args = value.to_owned(),
        _ => return false,
    }
    true
}

/// Every string key the page asks for (the strings test checks each is present).
pub fn required_keys() -> Vec<String> {
    let mut keys: Vec<String> = [
        "settings.nav.back",
        "settings.nav.heading",
        "settings.nav.name",
        "settings.page.name",
        "settings.about.name",
        "settings.about.version",
        "settings.about.blurb",
        "settings.about.notices",
        "luneui.settings-page.restart-message",
        "luneui.settings-page.restart-action",
    ]
    .iter()
    .map(|k| (*k).to_owned())
    .collect();
    for s in Section::ALL {
        keys.push(s.key().to_owned());
        for g in groups(s) {
            keys.push(g.header.to_owned());
            for r in g.rows {
                keys.push(format!("{}.title", r.key));
                keys.push(desc_key(r));
                if r.control == Control::Field {
                    keys.push(format!("{}.placeholder", r.key));
                }
                keys.extend(option_keys(r.id));
            }
        }
    }
    for (name, license) in NOTICES {
        keys.push(name.to_owned());
        keys.push(license.to_owned());
    }
    keys
}

// ----------------------------------------------------------------------------------- the layout

/// Where the page puts things inside the card (logical px). At 1040x660 the card is
/// (10, 34, 1020, 616).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageLayout {
    pub card: Rect,
    /// The `Back to terminal` row.
    pub back: Rect,
    /// Where the `SETTINGS` heading's text starts.
    pub heading: (f32, f32),
    /// The section list's top left and width.
    pub nav: Rect,
    pub divider: Rect,
    /// The toolkit page's rect: 4 px inside the card's top, right and bottom, because its right
    /// corners are square and only its top left is rounded.
    pub page: Rect,
}

impl PageLayout {
    pub fn new(card: Rect) -> PageLayout {
        let back = Rect::new(card.x + 10.0, card.y + 12.0, NAV_W, CONTROL_H);
        let nav = Rect::new(back.x, back.bottom() + 40.0, NAV_W, 0.0);
        let divider = Rect::new(
            nav.right() + 10.0,
            card.y + 12.0,
            1.0,
            (card.h - 24.0).max(0.0),
        );
        let page_x = divider.right() + 4.0;
        PageLayout {
            card,
            back,
            heading: (card.x + 18.0, back.bottom() + 16.0),
            nav,
            divider,
            page: Rect::new(
                page_x,
                card.y + 4.0,
                (card.right() - 4.0 - page_x).max(0.0),
                (card.h - 8.0).max(0.0),
            ),
        }
    }

    /// The toolkit page's content viewport: its host inset.
    pub fn viewport(&self) -> Rect {
        let pad = lune_ui::widgets::settings_page::HOST_PAD;
        Rect::new(
            self.page.x + pad,
            self.page.y + pad,
            (self.page.w - 2.0 * pad).max(0.0),
            (self.page.h - 2.0 * pad).max(0.0),
        )
    }
}

// ------------------------------------------------------------------------------------- the draw

/// What the page remembers between frames.
#[derive(Debug, Clone)]
pub struct PageState {
    pub section: Section,
    /// The row whose option list is open.
    pub combo: Option<SettingId>,
    /// The field rows' text while it is edited (default directory, shell, shell arguments).
    texts: [String; 3],
}

impl PageState {
    /// A page on `section`, its fields holding what `settings` has.
    pub fn new(section: Section, settings: &Settings) -> PageState {
        PageState {
            section,
            combo: None,
            texts: [
                settings.default_cwd.clone(),
                settings.shell.clone(),
                settings.shell_args.clone(),
            ],
        }
    }
}

/// What a frame draws the page from.
#[derive(Debug)]
pub struct PageView<'a> {
    pub state: &'a mut PageState,
    /// The settings as the page edits them: the app compares them with its own after the frame.
    pub settings: &'a mut Settings,
    /// The notice banner is shown.
    pub restart: bool,
    /// The window has keyboard focus (the card's ring).
    pub focused: bool,
}

/// What the frame asked of the app.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PageOut {
    pub back: bool,
    /// The banner's action was pressed.
    pub restart_action: bool,
    /// Where the focused field edits, for the IME.
    pub ime_area: Option<Rect>,
}

fn spec(text: &str, size: f32, color: Rgba) -> Spec {
    Spec::plain(text, Style::ui(size, color))
}

/// The room the scroll bar keeps clear on the right: the groups end this far inside the viewport.
const BAR_ROOM: f32 = 12.0;

/// The width of the groups in the viewport `vp`: they end [`BAR_ROOM`] before its right edge, so
/// the toolkit's scroll thumb never sits on a group's ring.
pub fn group_width(vp: Rect) -> f32 {
    (vp.w - BAR_ROOM).max(0.0)
}

/// Draws the page over `card` and returns what the user asked of it.
pub fn draw(ui: &mut Ui<'_>, strings: &StringTable, view: PageView<'_>, card: Rect) -> PageOut {
    let PageView {
        state,
        settings,
        restart,
        focused,
    } = view;
    let theme = *ui.theme();
    let lay = PageLayout::new(card);
    let panel = theme.glass.panel.premultiplied();
    let mut out = PageOut::default();
    {
        // The whole card in the panel layer, then the nav on it; the toolkit's page below uses
        // the same layer and the same `under`, so the two meet with no seam (director, batch 1).
        let p = ui.painter();
        p.on(LayerId::Base);
        p.acrylic(card, CARD_RADIUS, panel, panel, true);
        p.ring(
            card,
            CARD_RADIUS,
            1.0,
            if focused {
                theme.border
            } else {
                theme.border_subtle
            },
            false,
        );
        let heading = Spec::plain(
            strings.tr("settings.nav.heading"),
            Style::ui(theme.font_s, theme.fg_tertiary).medium(),
        );
        p.text(&heading, lay.heading.0, lay.heading.1);
        p.fill(lay.divider, 0.0, theme.border_subtle);
    }

    // Back: a one-entry list that is never the current screen.
    let back = [NavEntry {
        key: 0,
        label: strings.tr("settings.nav.back"),
        icon: Some(icon::ARROW_LEFT),
        enabled: true,
    }];
    let mut never = u64::MAX;
    let r = ui.nav_list(
        "back",
        lay.back,
        &back,
        &mut never,
        strings.tr("settings.nav.back"),
    );
    out.back = r.activated.is_some();

    let entries: Vec<NavEntry<'_>> = Section::ALL
        .iter()
        .map(|s| NavEntry {
            key: *s as u64,
            label: strings.tr(s.key()),
            icon: None,
            enabled: true,
        })
        .collect();
    let mut current = state.section as u64;
    ui.nav_list(
        "sections",
        lay.nav,
        &entries,
        &mut current,
        strings.tr("settings.nav.name"),
    );
    if let Some(s) = Section::ALL.iter().find(|s| **s as u64 == current) {
        if *s != state.section {
            state.combo = None;
        }
        state.section = *s;
    }

    let section = state.section;
    let name = strings.tr(section.key()).to_owned();
    let page = SettingsPage {
        glass: theme.glass.panel,
        under: panel,
        restart,
        name: &name,
    };
    let resp = ui.settings_page(("page", section), lay.page, &page, |ui, vp| {
        content(ui, strings, state, settings, vp, &mut out)
    });
    out.restart_action = resp.restart;
    out
}

/// The section's content in the viewport `vp`; returns its height.
fn content(
    ui: &mut Ui<'_>,
    strings: &StringTable,
    state: &mut PageState,
    settings: &mut Settings,
    vp: Rect,
    out: &mut PageOut,
) -> f32 {
    let theme = *ui.theme();
    let section = state.section;
    let title = Spec::plain(
        strings.tr(section.key()),
        Style::ui(TITLE_SIZE, theme.fg_primary).medium(),
    );
    ui.painter().text(&title, vp.x, vp.y);
    let mut y = vp.y + 40.0;
    if section == Section::About {
        y = about(ui, strings, vp, y);
    } else {
        for group in groups(section) {
            y = draw_group(ui, strings, state, settings, vp, y, group, out) + 16.0;
        }
    }
    y - vp.y + 4.0
}

#[allow(clippy::too_many_arguments)]
fn draw_group(
    ui: &mut Ui<'_>,
    strings: &StringTable,
    state: &mut PageState,
    settings: &mut Settings,
    vp: Rect,
    y: f32,
    group: &Group,
    out: &mut PageOut,
) -> f32 {
    let theme = *ui.theme();
    let (x, w) = (vp.x, group_width(vp));
    let header = Spec::plain(
        strings.tr(group.header),
        Style::ui(theme.font_m, theme.fg_primary).medium(),
    );
    let mut cy = y + 14.0;
    ui.painter().text(&header, x + 16.0, cy);
    cy += 30.0;
    for r in group.rows {
        let cw = r.control.width();
        let text_w = (w - 8.0 - 24.0 - cw - 24.0).max(40.0);
        let title_text = strings.tr(&format!("{}.title", r.key)).to_owned();
        let title = spec(&title_text, theme.font_base, theme.fg_primary);
        let desc = spec(
            strings.tr(&desc_key(r)),
            theme.font_s + 0.5,
            theme.fg_secondary,
        )
        .wrapped(text_w);
        let (_, th) = ui.painter().measure(&title);
        let (_, dh) = ui.painter().measure(&desc);
        let ch = r.control.height();
        let row_h = (th + 4.0 + dh).max(ch) + 20.0;
        let rect = Rect::new(x + 4.0, cy, w - 8.0, row_h);
        if state.combo.is_none() && ui.pointer_in(rect) && ui.pointer_in(vp) {
            ui.painter()
                .fill(rect, theme.item_radius, theme.surface_hover);
        }
        let p = ui.painter();
        p.text(&title, rect.x + 12.0, rect.y + 10.0);
        p.text(&desc, rect.x + 12.0, rect.y + 10.0 + th + 4.0);
        let c = Rect::new(
            rect.right() - 12.0 - cw,
            rect.y + (row_h - ch) / 2.0,
            cw,
            ch,
        );
        control(ui, strings, state, settings, r, c, &title_text, out);
        cy += row_h + 2.0;
    }
    let panel = Rect::new(x, y, w, cy - y + 6.0);
    ui.painter().ring(panel, 8.0, 1.0, theme.border, false);
    panel.bottom()
}

#[allow(clippy::too_many_arguments)]
fn control(
    ui: &mut Ui<'_>,
    strings: &StringTable,
    state: &mut PageState,
    settings: &mut Settings,
    r: &Row,
    c: Rect,
    name: &str,
    out: &mut PageOut,
) {
    let id = r.id;
    match r.control {
        Control::Toggle => {
            let mut on = flag(settings, id).unwrap_or(false);
            ui.toggle(("toggle", id), c, &mut on, name);
            set_flag(settings, id, on);
        }
        Control::Combo => {
            let labels = options(strings, id);
            let sel = selected(settings, id).min(labels.len().saturating_sub(1));
            let shown = labels.get(sel).map_or("", String::as_str);
            let enabled = id != SettingId::Glass || HOST_GLASS;
            ui.with_enabled(enabled, |ui| {
                let mut open = state.combo == Some(id);
                let resp = ui.combo(("combo", id), c, shown, &mut open, name);
                if resp.clicked && !resp.changed {
                    open = false;
                }
                if open {
                    let list: Vec<(&str, bool)> =
                        labels.iter().map(|l| (l.as_str(), true)).collect();
                    let d = ui.dropdown_with(
                        ("list", id),
                        resp.rect,
                        resp.id,
                        &list,
                        sel,
                        &lune_ui::widgets::dropdown::DropdownOptions {
                            max_rows: Some(MAX_LIST_ROWS),
                        },
                    );
                    if let Some(i) = d.picked {
                        choose(settings, id, i);
                    }
                    if d.picked.is_some() || d.dismissed {
                        open = false;
                    }
                }
                if open {
                    state.combo = Some(id);
                } else if state.combo == Some(id) {
                    state.combo = None;
                }
            });
        }
        Control::Stepper => {
            if let Some((value, range)) = number(settings, id) {
                let s = ui.stepper(("stepper", id), c, value, range, name);
                if let Some(set) = s.set {
                    set_number(settings, id, set as u32);
                } else if let Some(steps) = s.step {
                    set_number(settings, id, step_number(id, value, steps));
                }
            }
        }
        Control::Field => {
            let Some(slot) = text_slot(id) else { return };
            let max = text_max(id);
            let placeholder = strings.tr(&format!("{}.placeholder", r.key)).to_owned();
            let buf = &mut state.texts[slot];
            let opts = FieldOptions {
                placeholder: &placeholder,
                error: !text_ok(buf, max),
                max_chars: Some(max),
                label: Some(name),
                ..FieldOptions::default()
            };
            let f = ui.text_field_with(("field", id), c, buf, &opts);
            if f.response.changed {
                // A text the setting refuses stays in the box, marked, and is not saved.
                let value = buf.clone();
                set_text(settings, id, &value);
            }
            out.ime_area = f.ime_area.or(out.ime_area);
        }
    }
}

/// The About section: the app, its version and the notices. Returns the next y.
fn about(ui: &mut Ui<'_>, strings: &StringTable, vp: Rect, y: f32) -> f32 {
    let theme = *ui.theme();
    let w = vp.w.min(560.0);
    let mut y = y;
    let name = Spec::plain(
        strings.tr("settings.about.name"),
        Style::ui(theme.font_m, theme.fg_primary).medium(),
    );
    y += ui.painter().text(&name, vp.x, y).1 + 6.0;
    let version = strings.tr_fmt(
        "settings.about.version",
        &[&env!("CARGO_PKG_VERSION")],
        &DisplayFormat,
    );
    for line in [version.as_str(), strings.tr("settings.about.blurb")] {
        let s = spec(line, theme.font_base, theme.fg_secondary).wrapped(w);
        y += ui.painter().text(&s, vp.x, y).1 + 8.0;
    }
    y += 12.0;
    let header = Spec::plain(
        strings.tr("settings.about.notices"),
        Style::ui(theme.font_m, theme.fg_primary).medium(),
    );
    y += ui.painter().text(&header, vp.x, y).1 + 10.0;
    for (name, license) in NOTICES {
        let p = ui.painter();
        p.text(
            &spec(strings.tr(name), theme.font_base, theme.fg_primary),
            vp.x,
            y,
        );
        p.text(
            &spec(strings.tr(license), theme.font_base, theme.fg_tertiary),
            vp.x + w - 140.0,
            y,
        );
        y += 24.0;
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_exactly_one_row() {
        let mut seen = Vec::new();
        for s in Section::ALL {
            for g in groups(s) {
                for r in g.rows {
                    seen.push(r.id);
                }
            }
        }
        for id in SettingId::ALL {
            assert_eq!(
                seen.iter().filter(|s| **s == id).count(),
                1,
                "{id:?} needs exactly one row"
            );
        }
        assert_eq!(seen.len(), SettingId::ALL.len());
    }

    #[test]
    fn a_row_has_the_control_its_value_needs() {
        let s = Settings::default();
        for sec in Section::ALL {
            for g in groups(sec) {
                for r in g.rows {
                    match r.control {
                        Control::Toggle => assert!(flag(&s, r.id).is_some(), "{:?}", r.id),
                        Control::Stepper => assert!(number(&s, r.id).is_some(), "{:?}", r.id),
                        Control::Field => assert!(text(&s, r.id).is_some(), "{:?}", r.id),
                        Control::Combo => {
                            let n = options(&crate::strings::english(), r.id).len();
                            assert!(n >= 1, "{:?}", r.id);
                            assert!(selected(&s, r.id) < n, "{:?}", r.id);
                        }
                    }
                }
            }
        }
    }

    /// Choosing option `i` and reading the selection back gives `i`, for every option of every
    /// combo row: the labels, the stored values and the selection agree.
    #[test]
    fn choosing_an_option_selects_it_for_every_combo_row() {
        let strings = crate::strings::english();
        for id in SettingId::ALL {
            let labels = options(&strings, id);
            if labels.is_empty() {
                continue;
            }
            for i in 0..labels.len() {
                let mut s = Settings::default();
                choose(&mut s, id, i);
                let want = if id == SettingId::Glass && !HOST_GLASS {
                    0
                } else {
                    i
                };
                assert_eq!(selected(&s, id), want, "{id:?} option {i}");
            }
            // An index past the list changes nothing.
            let mut s = Settings::default();
            choose(&mut s, id, labels.len() + 5);
            assert_eq!(s, Settings::default(), "{id:?}");
        }
    }

    #[test]
    fn each_combo_row_stores_the_value_the_table_names() {
        let mut s = Settings::default();
        choose(&mut s, SettingId::MenuBlur, 2);
        assert_eq!(s.menu_blur, BlurLevel::High);
        choose(&mut s, SettingId::Motion, 1);
        assert_eq!(s.motion, Motion::Reduced);
        choose(&mut s, SettingId::Outline, 1);
        assert_eq!(s.outline, OutlineAnim::Comet);
        choose(&mut s, SettingId::Palette, 4);
        assert_eq!(s.palette, "Dracula");
        choose(&mut s, SettingId::Font, 0);
        assert_eq!(s.font, "", "the first font is the default, saved as empty");
        if HOST_GLASS {
            choose(&mut s, SettingId::Glass, 3);
            assert_eq!(s.glass, GlassStyle::Off);
        } else {
            choose(&mut s, SettingId::Glass, 1);
            assert_eq!(s.glass, GlassStyle::Default, "Linux has no glass to choose");
        }
    }

    #[test]
    fn toggles_flip_their_own_field_only() {
        for id in [
            SettingId::AnimateOutline,
            SettingId::OutlineFullScreen,
            SettingId::ReportIterm2,
            SettingId::ShellIntegration,
            SettingId::ConfirmClose,
            SettingId::OscClipboard,
            SettingId::OptionAsMeta,
        ] {
            let mut s = Settings::default();
            let before = flag(&s, id).expect("a toggle");
            set_flag(&mut s, id, !before);
            assert_eq!(flag(&s, id), Some(!before));
            assert_eq!(Settings::default().changed(&s), vec![id]);
        }
        let mut s = Settings::default();
        set_flag(&mut s, SettingId::Shell, true);
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn steppers_step_in_their_units_and_stay_in_range() {
        use SettingId::{FontSize, Scrollback};
        assert_eq!(step_number(FontSize, 13, 1), 14);
        assert_eq!(step_number(FontSize, 13, -1), 12);
        assert_eq!(step_number(FontSize, 32, 1), 32);
        assert_eq!(step_number(FontSize, 8, -1), 8);
        assert_eq!(step_number(Scrollback, 5000, 1), 6000);
        assert_eq!(step_number(Scrollback, 5000, -1), 4000);
        assert_eq!(step_number(Scrollback, 500, -1), 0);
        assert_eq!(step_number(Scrollback, 100_000, 1), 100_000);
        assert_eq!(step_number(Scrollback, 100_000, 1000), 100_000);
        assert_eq!(step_number(SettingId::Shell, 7, 1), 7);
        let mut s = Settings::default();
        set_number(&mut s, FontSize, 99);
        assert_eq!(s.font_size, 32);
        set_number(&mut s, Scrollback, u32::MAX);
        assert_eq!(s.scrollback, 100_000);
        set_number(&mut s, FontSize, 0);
        assert_eq!(s.font_size, 8);
    }

    #[test]
    fn a_text_row_takes_only_text_the_setting_accepts() {
        let mut s = Settings::default();
        assert!(set_text(&mut s, SettingId::Shell, "/bin/zsh"));
        assert_eq!(s.shell, "/bin/zsh");
        assert!(!set_text(&mut s, SettingId::Shell, "a\nb"));
        assert!(!set_text(
            &mut s,
            SettingId::Shell,
            &"x".repeat(MAX_SHELL_BYTES + 1)
        ));
        assert_eq!(s.shell, "/bin/zsh", "a refused text changes nothing");
        assert!(set_text(&mut s, SettingId::Shell, ""));
        assert!(set_text(&mut s, SettingId::ShellArgs, "-l -i"));
        assert!(set_text(&mut s, SettingId::DefaultCwd, "/tmp"));
        assert!(!set_text(&mut s, SettingId::Palette, "x"));
        assert_eq!(s.clone().clamped(), s);
    }

    /// The geometry table of batch 1, at 1040x660.
    #[test]
    fn the_layout_pins_the_geometry_table() {
        let card = Rect::new(10.0, 34.0, 1020.0, 616.0);
        let l = PageLayout::new(card);
        assert_eq!(l.back, Rect::new(20.0, 46.0, 190.0, 32.0));
        assert_eq!(l.heading, (28.0, 94.0));
        assert_eq!((l.nav.x, l.nav.y, l.nav.w), (20.0, 118.0, 190.0));
        assert_eq!(l.divider, Rect::new(220.0, 46.0, 1.0, 592.0));
        assert_eq!(l.page, Rect::new(225.0, 38.0, 801.0, 608.0));
        assert_eq!(l.viewport(), Rect::new(245.0, 58.0, 761.0, 568.0));
        // The groups end 12 px before the viewport's right edge (1006): the thumb has the room.
        let v = l.viewport();
        assert_eq!(v.x + group_width(v), v.right() - 12.0);
        assert_eq!(v.x + group_width(v), 994.0);
        assert_eq!(group_width(Rect::new(0.0, 0.0, 5.0, 5.0)), 0.0);
        // A tiny card has empty rects, not negative ones.
        let t = PageLayout::new(Rect::new(0.0, 0.0, 30.0, 20.0));
        assert!(t.page.w >= 0.0 && t.page.h >= 0.0 && t.divider.h >= 0.0);
    }

    #[test]
    fn the_about_notices_name_only_what_this_build_carries() {
        let strings = crate::strings::english();
        let names: Vec<&str> = NOTICES.iter().map(|(n, _)| strings.tr(n)).collect();
        assert!(names.iter().any(|n| n.starts_with("xterm.js")));
        assert!(names.iter().any(|n| n.contains("RznSans")));
        assert!(
            !names.iter().any(|n| n.contains("RznMono")),
            "not shipped yet"
        );
        for (_, l) in NOTICES {
            assert!(!strings.tr(l).is_empty());
        }
    }
}
