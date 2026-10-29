//! App state, the settings model (mirrors Lunegit's `TerminalSettings`), and what clicks do.

use std::time::Instant;

use crate::backdrop::GlassMode;
use crate::mock;

// ── Settings ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingId {
    GlassStyle,
    KeepGlassInactive,
    Motion,
    Font,
    FontSize,
    Palette,
    AnimateOutline,
    OutlineStyle,
    OutlineFullScreen,
    Scrollback,
    DefaultCwd,
    Shell,
    ShellArgs,
    ReportITerm2,
    ConfirmClose,
    OscClipboard,
    OptionAsMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassStyle {
    Lunegit,
    LiquidClear,
    LiquidRegular,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Full,
    Reduced,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineAnim {
    Sweep,
    Comet,
    Breath,
}

/// Defaults follow Lunegit's shipped defaults (`TerminalSettings.Default`).
#[derive(Debug, Clone)]
pub struct Settings {
    pub glass: GlassStyle,
    pub keep_glass_inactive: bool,
    pub motion: Motion,
    pub font: usize,
    pub font_size: u32,
    pub palette: usize,
    pub animate_outline: bool,
    pub outline: OutlineAnim,
    pub outline_full_screen: bool,
    pub scrollback: u32,
    pub default_cwd: String,
    pub shell: String,
    pub shell_args: String,
    pub report_iterm2: bool,
    pub confirm_close: bool,
    pub osc_clipboard: bool,
    pub option_as_meta: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            glass: GlassStyle::Lunegit,
            keep_glass_inactive: false,
            motion: Motion::Full,
            font: 0,
            font_size: 13,
            palette: 0,
            animate_outline: true,
            outline: OutlineAnim::Sweep,
            outline_full_screen: false,
            scrollback: 5000,
            default_cwd: String::new(),
            shell: String::new(),
            shell_args: String::new(),
            report_iterm2: false,
            confirm_close: true,
            osc_clipboard: true,
            option_as_meta: false,
        }
    }
}

impl Settings {
    /// Options of a dropdown setting: (label, enabled).
    pub fn options(&self, id: SettingId) -> Vec<(String, bool)> {
        let v = |items: &[&str]| items.iter().map(|s| ((*s).to_owned(), true)).collect();
        match id {
            // The two alternative styles are Liquid Glass on macOS and Mica / Mica Alt on Windows.
            SettingId::GlassStyle if cfg!(target_os = "windows") => v(&[
                "Lunegit (default) · Acrylic",
                "Mica",
                "Mica Alt",
                "Off — solid dark",
            ]),
            SettingId::GlassStyle => v(&[
                "Lunegit (default)",
                "Liquid Glass · clear",
                "Liquid Glass · regular",
                "Off — solid dark",
            ]),
            SettingId::Motion => v(&["Full", "Reduced", "None"]),
            SettingId::Font => vec![
                ("RobotoMono Nerd Font Mono".into(), true),
                ("RznMono (coming soon)".into(), false),
            ],
            SettingId::Palette => crate::theme::PALETTES
                .iter()
                .map(|p| (p.name.to_owned(), true))
                .collect(),
            SettingId::OutlineStyle => v(&["Conic sweep", "Comet", "Breath"]),
            _ => Vec::new(),
        }
    }

    pub fn selected(&self, id: SettingId) -> usize {
        match id {
            SettingId::GlassStyle => self.glass as usize,
            SettingId::Motion => self.motion as usize,
            SettingId::Font => self.font,
            SettingId::Palette => self.palette,
            SettingId::OutlineStyle => self.outline as usize,
            _ => 0,
        }
    }

    pub fn choose(&mut self, id: SettingId, i: usize) {
        match id {
            SettingId::GlassStyle => {
                self.glass = [
                    GlassStyle::Lunegit,
                    GlassStyle::LiquidClear,
                    GlassStyle::LiquidRegular,
                    GlassStyle::Off,
                ][i.min(3)];
            }
            SettingId::Motion => {
                self.motion = [Motion::Full, Motion::Reduced, Motion::None][i.min(2)]
            }
            SettingId::Font => self.font = 0,
            SettingId::Palette => self.palette = i.min(crate::theme::PALETTES.len() - 1),
            SettingId::OutlineStyle => {
                self.outline =
                    [OutlineAnim::Sweep, OutlineAnim::Comet, OutlineAnim::Breath][i.min(2)];
            }
            _ => {}
        }
    }

    pub fn toggle(&self, id: SettingId) -> Option<bool> {
        Some(match id {
            SettingId::KeepGlassInactive => self.keep_glass_inactive,
            SettingId::AnimateOutline => self.animate_outline,
            SettingId::OutlineFullScreen => self.outline_full_screen,
            SettingId::ReportITerm2 => self.report_iterm2,
            SettingId::ConfirmClose => self.confirm_close,
            SettingId::OscClipboard => self.osc_clipboard,
            SettingId::OptionAsMeta => self.option_as_meta,
            _ => return None,
        })
    }

    pub fn flip(&mut self, id: SettingId) {
        let b = match id {
            SettingId::KeepGlassInactive => &mut self.keep_glass_inactive,
            SettingId::AnimateOutline => &mut self.animate_outline,
            SettingId::OutlineFullScreen => &mut self.outline_full_screen,
            SettingId::ReportITerm2 => &mut self.report_iterm2,
            SettingId::ConfirmClose => &mut self.confirm_close,
            SettingId::OscClipboard => &mut self.osc_clipboard,
            SettingId::OptionAsMeta => &mut self.option_as_meta,
            _ => return,
        };
        *b = !*b;
    }

    pub fn number(&self, id: SettingId) -> Option<u32> {
        match id {
            SettingId::FontSize => Some(self.font_size),
            SettingId::Scrollback => Some(self.scrollback),
            _ => None,
        }
    }

    pub fn step(&mut self, id: SettingId, dir: i32) {
        match id {
            SettingId::FontSize => {
                self.font_size = (self.font_size as i32 + dir).clamp(8, 32) as u32;
            }
            SettingId::Scrollback => {
                self.scrollback = (self.scrollback as i32 + dir * 1000).clamp(0, 100_000) as u32;
            }
            _ => {}
        }
    }

    pub fn field(&mut self, id: SettingId) -> Option<&mut String> {
        match id {
            SettingId::DefaultCwd => Some(&mut self.default_cwd),
            SettingId::Shell => Some(&mut self.shell),
            SettingId::ShellArgs => Some(&mut self.shell_args),
            _ => None,
        }
    }

    /// OS glass to request for these settings.
    pub fn glass_mode(&self) -> GlassMode {
        match self.glass {
            GlassStyle::Lunegit if self.keep_glass_inactive => GlassMode::Vibrancy,
            GlassStyle::Lunegit => GlassMode::VibrancyLunegit,
            GlassStyle::LiquidClear => GlassMode::LiquidClear,
            GlassStyle::LiquidRegular => GlassMode::LiquidRegular,
            GlassStyle::Off => GlassMode::Opaque,
        }
    }

    /// Lunegit's `TerminalOutlineAnimationResolver`: the app-wide motion level wins.
    pub fn effective_outline(&self) -> Option<OutlineAnim> {
        if !self.animate_outline {
            return None;
        }
        match self.motion {
            Motion::Full => Some(self.outline),
            Motion::Reduced => Some(OutlineAnim::Breath),
            Motion::None => None,
        }
    }

    pub fn line_height(&self) -> f32 {
        (self.font_size as f32 * 17.0 / 13.0).round()
    }
}

// ── UI state ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Appearance,
    Terminal,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCmd {
    NewTab,
    Duplicate(usize),
    Rename(usize),
    CopyCwd(usize),
    Close(usize),
    CloseOthers(usize),
    CloseAll,
    Settings,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogBtn {
    Cancel,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caption {
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Swallows clicks (modal backdrop, page background).
    Block,
    /// Clicking outside a popup closes it.
    Dismiss,
    TitleBand,
    /// Window caption buttons (Windows draws its own; macOS has traffic lights).
    Caption(Caption),
    Body,
    Tab(usize),
    TabClose(usize),
    NewTab,
    More,
    Menu(MenuCmd),
    Nav(Section),
    Back,
    Toggle(SettingId),
    Combo(SettingId),
    ComboItem(SettingId, usize),
    Step(SettingId, i32),
    Field(SettingId),
    Dialog(DialogBtn),
    DontAsk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    /// The header "…" button.
    App,
    /// Right-click on a tab.
    Tab(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Popup {
    Menu {
        kind: MenuKind,
        at: (f32, f32),
    },
    Combo {
        id: SettingId,
        anchor: crate::paint::Rect,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Rename { tab: usize, text: String },
    ConfirmClose { tabs: Vec<usize>, dont_ask: bool },
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Terminal,
    Settings(Section),
}

pub type Cell = (usize, usize);

#[derive(Debug, Clone)]
pub struct Tab {
    pub title: String,
    pub running: bool,
    pub screen: mock::Screen,
    pub selection: Option<(Cell, Cell)>,
}

impl Tab {
    pub fn new(title: &str, kind: mock::Kind, running: bool) -> Self {
        Self {
            title: title.to_owned(),
            running,
            screen: mock::screen(kind),
            selection: None,
        }
    }
}

/// What the event loop must do after a state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    ApplyGlass,
    DragWindow,
    Caption(Caption),
}

#[derive(Debug)]
pub struct State {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub switched_at: Instant,
    pub started: Instant,
    pub mouse: (f32, f32),
    pub selecting: bool,
    pub cmd: bool,
    pub focused: bool,
    pub settings: Settings,
    pub page: Page,
    pub scroll: f32,
    pub content_h: f32,
    pub popup: Option<Popup>,
    pub modal: Option<Modal>,
    pub editing: Option<SettingId>,
    pub hover: Option<Hit>,
    pub hits: Vec<(crate::paint::Rect, Hit)>,
    /// Terminal body rect and (cell width, line height), from the last frame.
    pub term_body: crate::paint::Rect,
    pub cell: (f32, f32),
    /// Window is maximised (Windows caption shows "restore").
    pub maximized: bool,
}

impl State {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            tabs: vec![
                Tab::new("zsh", mock::Kind::Shell, false),
                Tab::new("cargo build", mock::Kind::Build, true),
                Tab::new("claude", mock::Kind::Agent, false),
            ],
            active: 0,
            switched_at: now,
            started: now,
            mouse: (0.0, 0.0),
            selecting: false,
            cmd: false,
            focused: true,
            settings: Settings::default(),
            page: Page::Terminal,
            scroll: 0.0,
            content_h: 0.0,
            popup: None,
            modal: None,
            editing: None,
            hover: None,
            hits: Vec::new(),
            term_body: crate::paint::Rect::default(),
            cell: (7.8, 17.0),
            maximized: false,
        }
    }

    pub fn hit_at(&self, p: (f32, f32)) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| *h)
    }

    pub fn activate(&mut self, i: usize) {
        if i != self.active && i < self.tabs.len() {
            self.active = i;
            self.switched_at = Instant::now();
        }
    }

    pub fn new_tab(&mut self) {
        let n = self.tabs.len() + 1;
        self.tabs
            .push(Tab::new(&format!("zsh ({n})"), mock::Kind::Shell, false));
        self.activate(self.tabs.len() - 1);
    }

    /// Close tabs, asking first if any is running and the setting says so.
    pub fn request_close(&mut self, mut tabs: Vec<usize>) {
        tabs.retain(|&i| i < self.tabs.len());
        if tabs.is_empty() {
            return;
        }
        if self.settings.confirm_close && tabs.iter().any(|&i| self.tabs[i].running) {
            self.modal = Some(Modal::ConfirmClose {
                tabs,
                dont_ask: false,
            });
        } else {
            self.close_now(tabs);
        }
    }

    fn close_now(&mut self, mut tabs: Vec<usize>) {
        tabs.sort_unstable();
        tabs.dedup();
        let active_title = self.tabs.get(self.active).map(|t| t.title.clone());
        for &i in tabs.iter().rev() {
            self.tabs.remove(i);
        }
        if self.tabs.is_empty() {
            self.tabs.push(Tab::new("zsh", mock::Kind::Shell, false));
        }
        self.active = active_title
            .and_then(|t| self.tabs.iter().position(|x| x.title == t))
            .unwrap_or_else(|| self.active.min(self.tabs.len() - 1));
        self.switched_at = Instant::now();
    }

    pub fn click(&mut self, hit: Option<Hit>) -> Effect {
        let editing_before = self.editing;
        self.editing = None;
        let Some(hit) = hit else {
            self.popup = None;
            return Effect::None;
        };
        match hit {
            Hit::Block => self.editing = editing_before.filter(|_| self.modal.is_some()),
            Hit::Dismiss => self.popup = None,
            Hit::TitleBand => return Effect::DragWindow,
            Hit::Caption(c) => return Effect::Caption(c),
            Hit::Body => {}
            Hit::Tab(i) => self.activate(i),
            Hit::TabClose(i) => self.request_close(vec![i]),
            Hit::NewTab => self.new_tab(),
            Hit::More => {
                self.popup = Some(Popup::Menu {
                    kind: MenuKind::App,
                    at: self.mouse,
                });
            }
            Hit::Menu(cmd) => {
                self.popup = None;
                return self.menu(cmd);
            }
            Hit::Nav(section) => {
                self.page = Page::Settings(section);
                self.scroll = 0.0;
            }
            Hit::Back => self.page = Page::Terminal,
            Hit::Toggle(id) => {
                self.settings.flip(id);
                if id == SettingId::KeepGlassInactive {
                    return Effect::ApplyGlass;
                }
            }
            Hit::Combo(id) => {
                let anchor = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(_, h)| *h == Hit::Combo(id))
                    .map(|(r, _)| *r)
                    .unwrap_or_default();
                self.popup = Some(Popup::Combo { id, anchor });
            }
            Hit::ComboItem(id, i) => {
                self.popup = None;
                if self.settings.options(id).get(i).is_some_and(|o| o.1) {
                    self.settings.choose(id, i);
                    if id == SettingId::GlassStyle {
                        return Effect::ApplyGlass;
                    }
                }
            }
            Hit::Step(id, dir) => self.settings.step(id, dir),
            Hit::Field(id) => self.editing = Some(id),
            Hit::Dialog(btn) => self.dialog(btn),
            Hit::DontAsk => {
                if let Some(Modal::ConfirmClose { dont_ask, .. }) = &mut self.modal {
                    *dont_ask = !*dont_ask;
                }
            }
        }
        Effect::None
    }

    fn menu(&mut self, cmd: MenuCmd) -> Effect {
        match cmd {
            MenuCmd::NewTab => self.new_tab(),
            MenuCmd::Duplicate(i) => {
                if let Some(t) = self.tabs.get(i).cloned() {
                    self.tabs.insert(
                        i + 1,
                        Tab {
                            running: false,
                            selection: None,
                            ..t
                        },
                    );
                    self.activate(i + 1);
                }
            }
            MenuCmd::Rename(i) => {
                if let Some(t) = self.tabs.get(i) {
                    self.modal = Some(Modal::Rename {
                        tab: i,
                        text: t.title.clone(),
                    });
                }
            }
            MenuCmd::CopyCwd(_) => copy_to_clipboard("~/Projects/Luneterm"),
            MenuCmd::Close(i) => self.request_close(vec![i]),
            MenuCmd::CloseOthers(i) => {
                self.request_close((0..self.tabs.len()).filter(|&k| k != i).collect());
            }
            MenuCmd::CloseAll => self.request_close((0..self.tabs.len()).collect()),
            MenuCmd::Settings => {
                self.page = Page::Settings(Section::Appearance);
                self.scroll = 0.0;
            }
            MenuCmd::About => self.modal = Some(Modal::About),
        }
        Effect::None
    }

    pub fn dialog(&mut self, btn: DialogBtn) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        if btn == DialogBtn::Cancel {
            return;
        }
        match modal {
            Modal::Rename { tab, text } => {
                let text = text.trim();
                if !text.is_empty()
                    && let Some(t) = self.tabs.get_mut(tab)
                {
                    t.title = text.to_owned();
                }
            }
            Modal::ConfirmClose { tabs, dont_ask } => {
                if dont_ask {
                    self.settings.confirm_close = false;
                }
                self.close_now(tabs);
            }
            Modal::About => {}
        }
    }

    /// Typed text goes to the rename field or the focused settings field.
    pub fn type_text(&mut self, s: &str) {
        let target = match &mut self.modal {
            Some(Modal::Rename { text, .. }) => Some(text),
            _ => self.editing.and_then(|id| self.settings.field(id)),
        };
        if let Some(t) = target {
            t.extend(s.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn backspace(&mut self) {
        let target = match &mut self.modal {
            Some(Modal::Rename { text, .. }) => Some(text),
            _ => self.editing.and_then(|id| self.settings.field(id)),
        };
        if let Some(t) = target {
            t.pop();
        }
    }

    /// Esc: closes the innermost thing that is open.
    pub fn escape(&mut self) {
        if self.popup.take().is_some()
            || self.modal.take().is_some()
            || self.editing.take().is_some()
        {
            return;
        }
        self.page = Page::Terminal;
    }
}

fn copy_to_clipboard(text: &str) {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
    #[cfg(not(target_os = "macos"))]
    eprintln!("copy: {text}");
}
