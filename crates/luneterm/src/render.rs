//! One frame, composed (plan M9 step 4): the toolkit `Ui` draws the shell, the termview draws the
//! grid into the shell's slot, and the frame renderer puts both into a target. The window and the
//! frame dumps call the same [`Renderer::draw`], so a dump shows what the window shows.

use std::time::{Duration, Instant};

use lune_termview::{
    CellMetrics, Cursor, Grid, GridLayout, SelectionRange, TermFonts, TermTheme, TermView,
    ViewState, ViewStats,
};
use lune_ui::GlassLayer;
use lune_ui::color::premul;
use lune_ui::custom::CustomDraw;
use lune_ui::frame::FrameRenderer;
use lune_ui::gpu::Gpu;
use lune_ui::input::{FrameOutput, keymap};
use lune_ui::lunegit_dark::THEME;
use lune_ui::motion::{Cadence, MotionTier};
use lune_ui::near_black;
use lune_ui::paint::Rect;
use lune_ui::scene::{FrameParams, LayerId};
use lune_ui::shortcuts::Shortcuts;
use lune_ui::strings::{DisplayFormat, StringTable};
use lune_ui::text::{Spec, Style, TextCache, TextRendering};
use lune_ui::theme::{AcrylicLevel, Theme};
use lune_ui::ui::{FrameConfig, Sense, Ui, UiStore};
use lune_ui::widgets::menu::{MenuOptions, MenuPlace, MenuResponse, MenuRow};
use lune_ui::widgets::tab_strip::BusyCue;
use lune_ui::widgets::toast::Toasts;
use lune_ui::widgets::tone::Tone;

use crate::fonts;
use crate::links;
use crate::session::Geometry;
use crate::settings::{Settings, Theme as LookTheme, WindowOpacity};
use crate::settings_page::{self, PageOut, PageView};
use crate::shell::{
    self, Chrome, GridPlacement, Layout, Modal, ModalOut, ScrollView, ShellView, StripTab,
    TabContext,
};
use crate::tabs::TabId;

/// The saturation the backdrop blur applies (Luneium's menu glass; no backdrop is requested in M9).
const BACKDROP_SATURATION: f32 = 1.8;

/// The toolkit theme for the app's look, for a window with or without OS glass. The look picks the
/// layers under every glass style: Luneterm the near-black ones (the toolkit's `near_black::THEME`,
/// whole, so floating surfaces take it too), Lunegit its own (`THEME`). Without OS glass (Linux, or a
/// material the OS refused) every layer is opaque, whatever `opacity` is.
///
/// `opacity` (p = 0, .25, .5, .75, 1) gives up that share of what each layer still lets through:
/// a layer's alpha A becomes A + (1 - A) * p, so 1 - A' = (1 - A)(1 - p), and a floating surface's
/// fill alpha a becomes a + (255 - a) * p. `P0` returns the look's own theme unchanged. Only
/// alphas move, never a colour.
pub fn theme(glass: bool, look: LookTheme, opacity: WindowOpacity) -> Theme {
    let base = match look {
        LookTheme::Luneterm => near_black::THEME,
        LookTheme::Lunegit => THEME,
    };
    if !glass {
        return base.with_acrylic(AcrylicLevel::Opaque);
    }
    let p = opacity.fraction();
    if p == 0.0 {
        return base;
    }
    let mut t = base;
    for layer in [&mut t.glass.window, &mut t.glass.panel, &mut t.glass.card] {
        raise_tint(layer, p);
    }
    let a = f32::from(t.menu_glass[3]);
    t.menu_glass[3] = (a + (255.0 - a) * p).round().clamp(0.0, 255.0) as u8;
    t
}

/// Raises a black glass layer's tint so its composite alpha A becomes A + (1 - A) * p.
///
/// The layers are tinted black at alpha 255, so the tint's effective opacity is
/// `tint_opacity * curve.black_max_opacity` (the toolkit's `acrylic_colors`). With t' = t + (1 - t) * p
/// and the grey luminosity layer under it unchanged, 1 - A' = (1 - t')(1 - l) = (1 - A)(1 - p).
/// The lever is the tint, not `material_opacity`: that one adds grey and lightens the glass.
fn raise_tint(layer: &mut GlassLayer, p: f32) {
    let t = f64::from(layer.tint_opacity) * layer.curve.black_max_opacity;
    let raised = (t + (1.0 - t) * f64::from(p)).clamp(0.0, 1.0);
    layer.tint_opacity = 1.0;
    layer.curve.black_max_opacity = raised;
}

/// The tab header's wash over the card, for the look [`theme`] returns the theme of: the near-black
/// look's with its theme (the app's own value, see `shell::NEAR_BLACK_HEADER_TINT`), Lunegit's with
/// Lunegit's (luneui-rs D8: the header is a material of the look, so the app passes it).
pub fn header_tint(look: LookTheme) -> [u8; 4] {
    match look {
        LookTheme::Luneterm => shell::NEAR_BLACK_HEADER_TINT,
        LookTheme::Lunegit => shell::HEADER_TINT,
    }
}

/// What a frame shows besides the shell.
pub struct Content<'a> {
    /// The active tab's own label for the title band; `None` shows the app's name alone.
    pub title: Option<&'a str>,
    pub tabs: &'a [StripTab<'a>],
    pub active_tab: Option<u64>,
    /// A dialog over everything: it takes the keyboard and the IME.
    pub modal: Option<Modal<'a>>,
    pub maximized: bool,
    pub grid: &'a mut Grid,
    /// The selected cells, in viewport rows.
    pub selection: &'a [SelectionRange],
    /// The link id under the pointer (0: none), underlined. Only a link that would open is one.
    pub hover_link: u32,
    /// The link modifier is held over a hovered link: the pointer becomes a hand (the click opens).
    pub link_hand: bool,
    /// The tooltip for the hovered link; the toolkit shows it after its delay.
    pub link_tip: Option<LinkTip<'a>>,
    /// The app's toasts, drawn over everything; `None` draws none.
    pub toasts: Option<&'a mut Toasts>,
    pub cursor: Cursor,
    pub blink_on: bool,
    pub scroll: Option<ScrollView>,
    pub chrome: Chrome,
    /// The terminal owns the keyboard (D4): the frame asks for an IME at the cursor cell.
    pub ime: bool,
    /// The IME's composing text, drawn over the cursor cell while `ime` is set.
    pub preedit: Option<&'a str>,
    /// An OSC 52 write for the OS clipboard (already gated by the setting).
    pub clipboard_write: Option<String>,
    /// Ask the OS for the clipboard's text (a paste; the answer comes in a later frame's input).
    pub clipboard_read: bool,
    /// The settings page, covering the card; `None` shows the terminal.
    pub page: Option<PageView<'a>>,
    /// An open menu, drawn over everything but the dialog.
    pub menu: Option<MenuDraw<'a>>,
    /// The cue the active tab's outline runs, `None` when it is not busy or the setting is off.
    pub busy: Option<BusyCue>,
    /// The outline's colour while it runs.
    pub busy_tone: Tone,
    /// The opacity `grid` is drawn at: 1 outside a tab-switch crossfade ([`crate::fade`]).
    pub grid_alpha: f32,
    /// The grids of the tabs still fading out under `grid`, back to front.
    pub fading: Vec<FadingGrid<'a>>,
}

/// A tab's grid fading out under the active tab's during a tab-switch crossfade. It draws as the
/// tab last showed it: no selection, no link hover, an unfocused cursor.
#[derive(Debug)]
pub struct FadingGrid<'a> {
    pub id: TabId,
    pub grid: &'a mut Grid,
    pub cursor: Cursor,
    /// The tab's own terminal colours.
    pub theme: &'a TermTheme,
    /// 0..=1.
    pub alpha: f32,
}

/// What the hovered link's tooltip says and where it hangs.
#[derive(Debug, Clone, Copy)]
pub struct LinkTip<'a> {
    /// The link's cells, logical px: the tooltip's anchor.
    pub rect: Rect,
    /// Where the link goes, checked and shortened for display ([`crate::links::tip_url`]).
    pub url: &'a str,
    /// Names the link, so another link starts its own delay.
    pub key: u64,
}

/// A menu to draw this frame.
#[derive(Debug)]
pub struct MenuDraw<'a> {
    pub rows: Vec<MenuRow<'a>>,
    pub place: MenuPlace,
    pub name: &'a str,
}

impl std::fmt::Debug for Content<'_> {
    /// Never the clipboard's or the composing text (I16).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Content")
            .field("maximized", &self.maximized)
            .field("tabs", &self.tabs.len())
            .field("modal", &self.modal.is_some())
            .field("selection", &self.selection.len())
            .field("ime", &self.ime)
            .field("clipboard_write", &self.clipboard_write.is_some())
            .field("clipboard_read", &self.clipboard_read)
            .finish_non_exhaustive()
    }
}

/// Where the shell puts things in a window of a given size and scale.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub layout: Layout,
    pub placement: GridPlacement,
    pub metrics: CellMetrics,
    pub scale: f32,
}

impl Placed {
    /// The session's geometry for this placement: the cell size in logical px, the grid area in
    /// physical px.
    pub fn geometry(&self) -> Geometry {
        let scale = if self.scale.is_finite() && self.scale > 0.0 {
            self.scale
        } else {
            1.0
        };
        let m = &self.metrics;
        Geometry {
            cols: self.placement.cols,
            rows: self.placement.rows,
            cell_px: (m.cell_w as f32 / scale, m.cell_h as f32 / scale),
            pixel_size: (
                (self.placement.cols as u32).saturating_mul(m.cell_w),
                (self.placement.rows as u32).saturating_mul(m.cell_h),
            ),
        }
    }

    /// Cell and pixel mapping for the grid.
    pub fn grid_layout(&self) -> GridLayout {
        GridLayout::new(self.placement.origin, self.metrics)
    }

    /// One terminal line in logical px.
    pub fn cell_h_logical(&self) -> f32 {
        self.metrics.cell_h as f32 / self.metrics.scale
    }
}

/// What [`Renderer::draw`] hands back.
#[derive(Debug)]
pub struct Drawn {
    /// The window operations, repaint request and clipboard requests of the frame, the shell
    /// chrome's commands included.
    pub output: FrameOutput,
    /// What the user asked of the tabs this frame.
    pub tab_events: Vec<lune_ui::widgets::tab_strip::TabEvent>,
    /// The dialog's answer; `None` while none is shown.
    pub modal: Option<ModalOut>,
    /// The settings page's request; `None` while it is not shown.
    pub page: Option<PageOut>,
    /// What the open menu reported; `None` while none is shown.
    pub menu: Option<MenuResponse>,
    /// The `⋯` button was pressed: its rect.
    pub overflow: Option<Rect>,
    /// A tab asked for its context menu: which tab, and where it opens.
    pub tab_context: Option<TabContext>,
    /// The grid size the frame was laid out for.
    pub cols: usize,
    pub rows: usize,
}

/// Where the time of one [`Renderer::draw`] went, and the grid work it did: recorded only while
/// [`Renderer::set_phase_trace`] is on (the app's `LUNETERM_TRACE_SWITCH`). Times are from `start`,
/// the entry of `draw`; each is the end of its phase.
#[derive(Debug, Clone, Copy)]
pub struct DrawPhases {
    pub start: Instant,
    /// The toolkit frame (shell, strip, page, menus) built.
    pub ui: Duration,
    /// The active tab's grid view updated.
    pub active: Duration,
    /// The fading views assigned (the pool grown, owners swapped).
    pub assign: Duration,
    /// The fading views updated.
    pub fading: Duration,
    /// `draw_with` returned (prepare, encode, submit).
    pub draw_with: Duration,
    /// Views made by `assign_fading` in this draw.
    pub views_created: u32,
    /// Rows the active view built, and whether that was every row.
    pub active_rows: usize,
    pub active_full: bool,
    /// Rows the fading views built.
    pub fading_rows: usize,
    /// Rasterizer calls of every view in this draw: font glyphs, procedural glyphs.
    pub font_rasters: u64,
    pub custom_rasters: u64,
}

impl DrawPhases {
    fn new() -> DrawPhases {
        DrawPhases {
            start: Instant::now(),
            ui: Duration::ZERO,
            active: Duration::ZERO,
            assign: Duration::ZERO,
            fading: Duration::ZERO,
            draw_with: Duration::ZERO,
            views_created: 0,
            active_rows: 0,
            active_full: false,
            fading_rows: 0,
            font_rasters: 0,
            custom_rasters: 0,
        }
    }
}

/// The GPU, the fonts and everything built on them, for one device.
pub struct Renderer {
    pub gpu: Gpu,
    pub text: TextCache,
    frame: FrameRenderer,
    term: TermView,
    /// Views for the grids fading out in a tab-switch crossfade, made when first needed.
    fading: Vec<FadingView>,
    /// The colour format the views render into, and the terminal font, for making more views.
    format: wgpu::TextureFormat,
    term_fonts: TermFonts,
    store: UiStore,
    strings: StringTable,
    shortcuts: Shortcuts,
    theme: Theme,
    header_tint: [u8; 4],
    tier: MotionTier,
    blur_sigma: f32,
    /// The window opacity setting in force; [`theme`] applies it.
    window_opacity: WindowOpacity,
    term_theme: TermTheme,
    font_size: f32,
    /// How glyphs are rasterized: the one value the frame's `FrameParams.text` and every grid
    /// view's `update` get, so the UI text and the terminal draw the same look.
    text_rendering: TextRendering,
    /// The latest draw's phases, while the trace is on; `None` costs one check per phase.
    phases: Option<DrawPhases>,
}

/// Puts the item that holds each of `ids` at its own index in `pool` (items are swapped, so each
/// keeps what it holds), and gives an id no item holds an item that holds none of `ids`. Returns,
/// per id, whether it was not held (its item needs a full rebuild). `pool` has at least `ids.len()`
/// items; `owner` says which tab an item holds. Ids are distinct.
fn arrange_pool<T>(
    pool: &mut [T],
    owner: impl Fn(&T) -> Option<TabId>,
    ids: &[TabId],
) -> Vec<bool> {
    let n = pool.len();
    // `want[i]`: the original index of the item that goes to index `i`.
    let mut want: Vec<Option<usize>> = vec![None; n];
    let mut taken = vec![false; n];
    for (i, id) in ids.iter().enumerate() {
        if let Some(j) = pool.iter().position(|t| owner(t) == Some(*id)) {
            want[i] = Some(j);
            taken[j] = true;
        }
    }
    let fresh: Vec<bool> = want.iter().take(ids.len()).map(Option::is_none).collect();
    // The rest of the items, in order, fill what is still open.
    let mut rest = (0..n).filter(|j| !taken[*j]);
    for w in want.iter_mut().filter(|w| w.is_none()) {
        *w = rest.next();
    }
    // Apply the permutation with swaps; `at[p]` is the original index now at position `p`.
    let mut at: Vec<usize> = (0..n).collect();
    for (i, w) in want.iter().enumerate() {
        let Some(w) = *w else { continue };
        if let Some(p) = at.iter().skip(i).position(|o| *o == w) {
            pool.swap(i, i + p);
            at.swap(i, i + p);
        }
    }
    fresh
}

/// A [`TermView`] that draws one fading tab's grid, and which tab and colours it holds.
struct FadingView {
    view: TermView,
    owner: Option<TabId>,
    theme: TermTheme,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer").finish_non_exhaustive()
    }
}

impl Renderer {
    /// A renderer over `gpu` for targets of `format`. `text` holds the fonts (system fonts or
    /// not, the caller's choice); `glass` is whether the OS glass is behind the window.
    pub fn new(
        mut gpu: Gpu,
        mut text: TextCache,
        format: wgpu::TextureFormat,
        settings: &Settings,
        glass: bool,
    ) -> Renderer {
        let frame = FrameRenderer::new(&mut gpu, format);
        let term_fonts = fonts::term_fonts(&mut text, settings);
        let term_theme = TermTheme::lunegit_one_dark();
        let font_size = term_fonts.size;
        let term = TermView::new(&gpu.device, format, term_fonts.clone(), term_theme.clone());
        Renderer {
            gpu,
            text,
            frame,
            term,
            fading: Vec::new(),
            format,
            term_fonts,
            store: UiStore::default(),
            strings: crate::strings::english(),
            shortcuts: Shortcuts::default(),
            theme: theme(glass, settings.theme, settings.window_opacity),
            header_tint: header_tint(settings.theme),
            tier: settings.tier(),
            blur_sigma: settings.menu_blur.sigma(),
            window_opacity: settings.window_opacity,
            term_theme,
            font_size,
            text_rendering: settings.text_rendering(),
            phases: None,
        }
    }

    /// Records where each draw's time goes ([`DrawPhases`]); off by default.
    pub fn set_phase_trace(&mut self, on: bool) {
        self.phases = on.then(DrawPhases::new);
    }

    /// The latest draw's phases, while the trace is on.
    pub fn phases(&self) -> Option<&DrawPhases> {
        self.phases.as_ref()
    }

    /// Every grid view's counters summed: the rasterizer calls (font, procedural).
    fn raster_totals(&self) -> (u64, u64) {
        let views = std::iter::once(&self.term).chain(self.fading.iter().map(|f| &f.view));
        views.fold((0, 0), |(f, c), v| {
            let s: ViewStats = v.stats();
            (f + s.font_rasters, c + s.custom_rasters)
        })
    }

    /// Forgets the toolkit's retained state (focus, scroll offsets, open dialogs, animations). The
    /// frame dumps call it between scenes so one scene cannot leak into the next.
    pub fn reset_ui(&mut self) {
        self.store = UiStore::default();
    }

    /// The look and whether OS glass is behind the window: the look's glass theme, or its opaque one
    /// (P22). The glass style does not enter: it picks the OS material, the look the layers.
    pub fn set_glass(&mut self, glass: bool, look: LookTheme) {
        self.theme = theme(glass, look, self.window_opacity);
        self.header_tint = header_tint(look);
    }

    /// The window opacity of the setting, with the glass and look the theme is built for: only the
    /// theme is rebuilt, the OS material is not touched.
    pub fn set_window_opacity(&mut self, opacity: WindowOpacity, glass: bool, look: LookTheme) {
        self.window_opacity = opacity;
        self.set_glass(glass, look);
    }

    /// How glyphs are rasterized, for the UI text and every terminal view alike (the
    /// `font_smoothing` setting: smooth, the look luneui-rs chose in D296, or the hinted look of
    /// before). Takes effect on the next frame: each view rebuilds its rows when the value it was given changes.
    pub fn set_text_rendering(&mut self, text: TextRendering) {
        self.text_rendering = text;
    }

    /// The text setting in force.
    pub fn text_rendering(&self) -> TextRendering {
        self.text_rendering
    }

    /// The frame's parameters: the one place `FrameParams.text` is set, from the same value every
    /// terminal view's `update` gets.
    fn frame_params(&self, size: (u32, u32), scale: f32) -> FrameParams {
        FrameParams {
            size,
            scale,
            blur_sigma: self.blur_sigma * scale,
            saturation: BACKDROP_SATURATION,
            text: self.text_rendering,
            ..FrameParams::default()
        }
    }

    /// The menu and dialog blur of the setting.
    pub fn set_blur(&mut self, sigma: f32) {
        self.blur_sigma = sigma;
    }

    /// The motion tier of the setting.
    pub fn set_tier(&mut self, tier: MotionTier) {
        self.tier = tier;
    }

    /// New terminal fonts (the setting's family and size). The caller marks every grid dirty and
    /// sends the new geometry to every session.
    pub fn set_fonts(&mut self, settings: &Settings) {
        let fonts = fonts::term_fonts(&mut self.text, settings);
        self.font_size = fonts.size;
        for v in &mut self.fading {
            v.view.set_fonts(fonts.clone());
            v.owner = None;
        }
        self.term_fonts = fonts.clone();
        self.term.set_fonts(fonts);
    }

    /// Gives each fading layer a view, **at the pool index equal to the layer's index**: the scene
    /// reserves slots `1..=layers` and `draw` hands exactly the first `layers` views to the frame
    /// renderer, so a view anywhere else would be updated and never drawn (or the reverse: a
    /// view drawn with another tab's rows). The view that already holds a tab is swapped into place
    /// (it keeps its rows); a layer no view holds takes the view at its index, whose rows are then
    /// rebuilt from the tab's grid. The pool grows to the number of layers and never shrinks (the
    /// views are cheap to keep); views past the layers hold no tab.
    fn assign_fading(&mut self, layers: &mut [FadingGrid<'_>]) {
        while self.fading.len() < layers.len() {
            let theme = layers
                .get(self.fading.len())
                .map_or_else(|| self.term_theme.clone(), |l| l.theme.clone());
            let view = TermView::new(
                &self.gpu.device,
                self.format,
                self.term_fonts.clone(),
                theme.clone(),
            );
            if let Some(p) = &mut self.phases {
                p.views_created += 1;
            }
            self.fading.push(FadingView {
                view,
                owner: None,
                theme,
            });
        }
        let ids: Vec<TabId> = layers.iter().map(|l| l.id).collect();
        let fresh = arrange_pool(&mut self.fading, |v| v.owner, &ids);
        for (i, (l, fresh)) in layers.iter_mut().zip(fresh).enumerate() {
            let fv = &mut self.fading[i];
            if fresh {
                fv.owner = Some(l.id);
                l.grid.mark_all_dirty();
            }
            if fv.theme != *l.theme {
                fv.view.set_theme(l.theme.clone());
                fv.theme = l.theme.clone();
            }
        }
        for fv in self.fading.iter_mut().skip(layers.len()) {
            fv.owner = None;
        }
    }

    /// The app's string table (the page, the menus and the toolkit's own texts).
    pub fn strings(&self) -> &StringTable {
        &self.strings
    }

    /// The toolkit theme in force.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// New terminal colours (a colour set or restore by the child). The caller marks the grid
    /// dirty so every row is rebuilt.
    pub fn set_term_theme(&mut self, theme: TermTheme) {
        self.term.set_theme(theme.clone());
        self.term_theme = theme;
    }

    /// The terminal colours now in force.
    pub fn term_theme(&self) -> &TermTheme {
        &self.term_theme
    }

    /// Where everything goes in a window of `size` logical px at `scale`.
    pub fn place(&mut self, size: (f32, f32), scale: f32) -> Placed {
        let metrics = self.term.metrics(&mut self.text.fonts, scale);
        let layout = Layout::new(Rect::new(0.0, 0.0, size.0, size.1));
        let placement = shell::place_grid(layout.body, metrics);
        Placed {
            layout,
            placement,
            metrics,
            scale,
        }
    }

    /// The grid (cols, rows) that fits a window of `size` logical px at `scale`.
    pub fn grid_size(&mut self, size: (f32, f32), scale: f32) -> (usize, usize) {
        let placed = self.place(size, scale);
        (placed.placement.cols, placed.placement.rows)
    }

    /// Draws one frame of `input`'s size and scale into `target` (`size` physical px). The grid is
    /// drawn as it is: the caller resizes it (a session's Update does, so a window resize shows
    /// the old screen until the new one arrives, not a blank one). The caller presents `target`
    /// and calls `gpu.end_frame(&mut text)`.
    pub fn draw(
        &mut self,
        input: &lune_ui::input::RawInput,
        mut content: Content<'_>,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) -> Result<Drawn, String> {
        let rasters_before = if self.phases.is_some() {
            self.phases = Some(DrawPhases::new());
            self.raster_totals()
        } else {
            (0, 0)
        };
        let scale = input.scale;
        let placed = self.place(input.size, scale);
        let Placed {
            layout,
            placement,
            metrics,
            ..
        } = placed;

        let config = FrameConfig {
            theme: self.theme,
            tier: self.tier,
            busy_cadence: Cadence::BUSY,
            strings: &self.strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &self.shortcuts,
        };
        let mut ui = Ui::begin(&mut self.store, input, config, &mut self.text);
        let modal_open = content.modal.is_some();
        let view = ShellView {
            title: content.title,
            maximized: content.maximized,
            focused: input.focused,
            scroll: content.scroll,
            cell_h: metrics.cell_h as f32 / metrics.scale,
            chrome: content.chrome,
            tabs: content.tabs,
            active_tab: content.active_tab,
            modal: content.modal.take(),
            toasts: content.toasts.take(),
            page_open: content.page.is_some(),
            busy: content.busy,
            busy_tone: content.busy_tone,
            overflow_name: self.strings.tr("menu.more"),
            header_tint: self.header_tint,
            fading_slots: content.fading.len(),
        };
        let shell_out = shell::draw(&mut ui, view);
        let mut page_out = None;
        if let Some(page) = content.page.take() {
            page_out = Some(settings_page::draw(
                &mut ui,
                &self.strings,
                page,
                layout.card,
            ));
        }
        let menu_resp = content.menu.take().map(|m| {
            let options = MenuOptions {
                place: m.place,
                anchor_id: None,
                name: m.name,
            };
            ui.menu_with("app-menu", &m.rows, &options)
        });
        if content.link_hand {
            ui.set_cursor(lune_ui::input::CursorIcon::Pointer);
        }
        if let Some(tip) = content.link_tip {
            // The tooltip hangs from a hover-only region over the link's cells, on the page layer
            // under the menus and dialogs: they cover it and the toolkit hides the tip.
            ui.painter().on(LayerId::Page);
            let id = ui.id(("link-tip", tip.key));
            let anchor = ui.interact(id, tip.rect, Sense::HOVER);
            let hint = self.strings.tr(links::hint_key(cfg!(target_os = "macos")));
            ui.tooltip(&anchor, &format!("{}\n{hint}", tip.url));
        }

        let cursor = content.cursor;
        let cols = content.grid.cols();
        // With a dialog open the keyboard and the IME are the toolkit's.
        let cell = (content.ime && cols > 0 && !modal_open && page_out.is_none()).then(|| {
            placed
                .grid_layout()
                .cell_rect(cursor.row, cursor.col.min(cols - 1), 1)
        });
        let page_ime = page_out.as_ref().and_then(|p: &PageOut| p.ime_area);
        ui.set_ime_area(if modal_open {
            shell_out.ime_area
        } else {
            page_ime.or(cell)
        });
        if let (Some(cell), Some(text)) = (cell, content.preedit.filter(|t| !t.is_empty())) {
            let (fg, bg) = (self.term_theme.foreground, self.term_theme.background);
            let line = placed.cell_h_logical();
            let spec = Spec::plain(text, Style::mono(self.font_size, line, fg));
            let p = ui.painter();
            // Above the terminal's slot (a custom draw sits under its layer's text): the page
            // layer, clipped to the body.
            p.on(LayerId::Page);
            p.push_clip(layout.body);
            let width = p.measure(&spec).0.max(cell.w);
            let body = layout.body;
            let x = cell.x.min(body.x + body.w - width).max(body.x);
            let rect = Rect::new(x, cell.y, width, cell.h);
            p.fill(rect, 0.0, bg);
            p.text(&spec, x, cell.y);
            p.fill_over(
                Rect::new(x, cell.y + cell.h - 1.0, width, 1.0),
                0.0,
                premul(fg),
            );
            p.pop_clip();
        }
        if let Some(text) = content.clipboard_write.take() {
            ui.clipboard_write(text);
        }
        if content.clipboard_read {
            ui.clipboard_read();
        }
        let (scene, mut output) = ui.finish();
        if let Some(p) = &mut self.phases {
            p.ui = p.start.elapsed();
        }
        output.commands.extend(shell_out.commands);
        let (tab_events, modal) = (shell_out.tab_events, shell_out.modal);

        let view_state = ViewState {
            cursor,
            focused: input.focused,
            blink_on: content.blink_on,
            selection: content.selection,
            hover_link: content.hover_link,
        };
        self.term.set_opacity(content.grid_alpha);
        self.term.update(
            &mut self.text.fonts,
            content.grid,
            &view_state,
            placement.origin,
            scale,
            self.text_rendering,
        );
        if let Some(p) = &mut self.phases {
            p.active = p.start.elapsed();
            let s = self.term.stats();
            (p.active_rows, p.active_full) = (s.last_rows, s.last_full);
        }
        // The tabs fading out under it (a tab-switch crossfade), each in a view of its own.
        self.assign_fading(&mut content.fading);
        if let Some(p) = &mut self.phases {
            p.assign = p.start.elapsed();
        }
        for (layer, fv) in content.fading.iter_mut().zip(self.fading.iter_mut()) {
            fv.view.set_opacity(layer.alpha);
            let faded = ViewState {
                cursor: layer.cursor,
                focused: false,
                blink_on: content.blink_on,
                selection: &[],
                hover_link: 0,
            };
            fv.view.update(
                &mut self.text.fonts,
                layer.grid,
                &faded,
                placement.origin,
                scale,
                self.text_rendering,
            );
            if let Some(p) = &mut self.phases {
                p.fading_rows += fv.view.stats().last_rows;
            }
        }
        if let Some(p) = &mut self.phases {
            p.fading = p.start.elapsed();
        }
        let params = self.frame_params(size, scale);
        // Slot 0 is the active tab's grid, the rest are the fading ones (`shell::TERM_SLOT`).
        // Only the views of this frame's layers go in: slot `n` is `self.fading[n - 1]`.
        let shown = content.fading.len();
        let mut customs: Vec<&mut dyn CustomDraw> = Vec::with_capacity(1 + shown);
        customs.push(&mut self.term);
        customs.extend(
            self.fading
                .iter_mut()
                .take(shown)
                .map(|fv| &mut fv.view as &mut dyn CustomDraw),
        );
        self.frame.draw_with(
            &mut self.gpu,
            &mut self.text,
            &scene,
            target,
            params,
            &mut customs,
        )?;
        if self.phases.is_some() {
            let (font, custom) = self.raster_totals();
            if let Some(p) = &mut self.phases {
                p.draw_with = p.start.elapsed();
                p.font_rasters = font.saturating_sub(rasters_before.0);
                p.custom_rasters = custom.saturating_sub(rasters_before.1);
            }
        }
        Ok(Drawn {
            output,
            tab_events,
            modal,
            page: page_out,
            menu: menu_resp,
            overflow: shell_out.overflow,
            tab_context: shell_out.tab_context,
            cols: placement.cols,
            rows: placement.rows,
        })
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn the_luneterm_look_is_the_near_black_theme_whole() {
        // Floating surfaces fill from `menu_glass`: the whole theme, not only its layers.
        let t = theme(true, LookTheme::Luneterm, WindowOpacity::P0);
        assert_eq!(t, near_black::THEME);
        assert_eq!(t.glass, near_black::GLASS);
        assert_eq!(t.menu_glass, near_black::MENU_GLASS);
        assert_ne!(t.glass, THEME.glass);
        assert_ne!(t.menu_glass, THEME.menu_glass);
    }

    #[test]
    fn the_lunegit_look_is_lunegits_theme_whole() {
        let t = theme(true, LookTheme::Lunegit, WindowOpacity::P0);
        assert_eq!(t, THEME);
        assert_eq!(t.glass, THEME.glass);
        assert_eq!(t.menu_glass, THEME.menu_glass);
    }

    /// Linux, `Off`, a refused material: each look's own layers, opaque.
    #[test]
    fn no_glass_is_the_looks_own_theme_made_opaque() {
        for (look, base) in [
            (LookTheme::Luneterm, near_black::THEME),
            (LookTheme::Lunegit, THEME),
        ] {
            let t = theme(false, look, WindowOpacity::P0);
            assert_eq!(t, base.with_acrylic(AcrylicLevel::Opaque), "{look:?}");
            assert_eq!(t.menu_glass, base.menu_glass, "{look:?}");
        }
    }

    /// The three layers of a theme, by name.
    fn layers(t: &Theme) -> [(&'static str, GlassLayer); 3] {
        [
            ("window", t.glass.window),
            ("panel", t.glass.panel),
            ("card", t.glass.card),
        ]
    }

    fn base_theme(look: LookTheme) -> Theme {
        match look {
            LookTheme::Luneterm => near_black::THEME,
            LookTheme::Lunegit => THEME,
        }
    }

    /// 0% gives the look's own theme, bit for bit: the approved look does not move.
    #[test]
    fn zero_opacity_is_the_theme_before_the_setting_existed() {
        for look in LookTheme::ALL.iter().copied() {
            assert_eq!(
                theme(true, look, WindowOpacity::P0),
                base_theme(look),
                "{look:?}"
            );
        }
    }

    /// 1 - A' = (1 - A)(1 - p) for every layer of every look (the tint alpha is a u8, so within
    /// 1/255), and 100% is solid: every layer's alpha is exactly 1.
    #[test]
    fn each_layer_gives_up_the_share_p_of_what_it_lets_through() {
        for look in LookTheme::ALL.iter().copied() {
            let base = base_theme(look);
            for w in WindowOpacity::ALL.iter().copied() {
                let p = w.fraction();
                let t = theme(true, look, w);
                for ((name, was), (_, now)) in layers(&base).into_iter().zip(layers(&t)) {
                    let a = was.premultiplied()[3];
                    let a2 = now.premultiplied()[3];
                    let want = (1.0 - a) * (1.0 - p);
                    assert!(
                        ((1.0 - a2) - want).abs() <= 1.0 / 255.0,
                        "{look:?} {name} p={p}: 1-A' = {} want {want}",
                        1.0 - a2
                    );
                    if w == WindowOpacity::P100 {
                        assert_eq!(a2, 1.0, "{look:?} {name} is solid at 100%");
                    }
                }
            }
        }
    }

    /// More opacity never lets more through: the alpha of every layer and of the floating fill
    /// only grows from 0% to 100%.
    #[test]
    fn opacity_only_ever_adds_alpha() {
        for look in LookTheme::ALL.iter().copied() {
            let mut last: Option<Theme> = None;
            for w in WindowOpacity::ALL.iter().copied() {
                let t = theme(true, look, w);
                if let Some(prev) = &last {
                    for ((name, was), (_, now)) in layers(prev).into_iter().zip(layers(&t)) {
                        assert!(
                            now.premultiplied()[3] >= was.premultiplied()[3],
                            "{look:?} {name} {w:?}"
                        );
                    }
                    assert!(t.menu_glass[3] >= prev.menu_glass[3], "{look:?} {w:?}");
                }
                last = Some(t);
            }
        }
    }

    /// Floating surfaces: a' = a + (255 - a) * p, rounded; solid at 100%.
    #[test]
    fn the_floating_fill_follows_the_same_share() {
        for look in LookTheme::ALL.iter().copied() {
            let a = f32::from(base_theme(look).menu_glass[3]);
            for w in WindowOpacity::ALL.iter().copied() {
                let got = theme(true, look, w).menu_glass[3];
                let want = (a + (255.0 - a) * w.fraction()).round() as u8;
                assert_eq!(got, want, "{look:?} {w:?}");
            }
            assert_eq!(theme(true, look, WindowOpacity::P100).menu_glass[3], 255);
        }
    }

    /// Only alphas and opacities move: the tint colour, the grey layer and every colour of the
    /// floating fill stay.
    #[test]
    fn opacity_moves_alphas_and_nothing_else() {
        for look in LookTheme::ALL.iter().copied() {
            let base = base_theme(look);
            for w in WindowOpacity::ALL.iter().copied() {
                let t = theme(true, look, w);
                for ((name, was), (_, now)) in layers(&base).into_iter().zip(layers(&t)) {
                    assert_eq!(now.tint, was.tint, "{look:?} {name} {w:?}");
                    assert_eq!(
                        now.material_opacity, was.material_opacity,
                        "{look:?} {name} {w:?}"
                    );
                    assert_eq!(now.curve.mid_point, was.curve.mid_point);
                    assert_eq!(
                        now.curve.mid_point_max_opacity,
                        was.curve.mid_point_max_opacity
                    );
                    assert_eq!(now.curve.white_max_opacity, was.curve.white_max_opacity);
                }
                assert_eq!(t.menu_glass[..3], base.menu_glass[..3], "{look:?} {w:?}");
                // Everything but the glass and the floating fill is the base theme's.
                let mut rest = t;
                rest.glass = base.glass;
                rest.menu_glass = base.menu_glass;
                assert_eq!(rest, base, "{look:?} {w:?}");
            }
        }
    }

    /// An opaque window has nothing to raise: the theme is the same at every opacity.
    #[test]
    fn without_glass_the_opacity_changes_nothing() {
        for look in LookTheme::ALL.iter().copied() {
            let want = theme(false, look, WindowOpacity::P0);
            for w in WindowOpacity::ALL.iter().copied() {
                assert_eq!(theme(false, look, w), want, "{look:?} {w:?}");
            }
        }
    }

    /// Every (glass, look) to its theme and header tint; the glass style is no input of either,
    /// the look picks the layers under every style (ui-polish D3).
    #[test]
    fn every_glass_and_look_pair_has_its_theme_and_tint() {
        let opaque = AcrylicLevel::Opaque;
        let table = [
            (
                true,
                LookTheme::Luneterm,
                near_black::THEME,
                shell::NEAR_BLACK_HEADER_TINT,
            ),
            (true, LookTheme::Lunegit, THEME, shell::HEADER_TINT),
            (
                false,
                LookTheme::Luneterm,
                near_black::THEME.with_acrylic(opaque),
                shell::NEAR_BLACK_HEADER_TINT,
            ),
            (
                false,
                LookTheme::Lunegit,
                THEME.with_acrylic(opaque),
                shell::HEADER_TINT,
            ),
        ];
        assert_eq!(table.len(), 2 * LookTheme::ALL.len());
        for (glass, look, want_theme, want_tint) in table {
            assert_eq!(
                theme(glass, look, WindowOpacity::P0),
                want_theme,
                "{glass} {look:?}"
            );
            assert_eq!(header_tint(look), want_tint, "{look:?}");
        }
        assert_eq!(shell::NEAR_BLACK_HEADER_TINT, [0xFF, 0xFF, 0xFF, 0x10]);
        assert_ne!(shell::NEAR_BLACK_HEADER_TINT, shell::HEADER_TINT);
        assert_ne!(
            theme(true, LookTheme::Luneterm, WindowOpacity::P0),
            theme(true, LookTheme::Lunegit, WindowOpacity::P0)
        );
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;

    /// The pool as the tab each view holds; `None` holds nothing.
    fn arranged(mut pool: Vec<Option<TabId>>, ids: &[TabId]) -> (Vec<Option<TabId>>, Vec<bool>) {
        let fresh = arrange_pool(&mut pool, |t| *t, ids);
        (pool, fresh)
    }

    const A: TabId = 1;
    const B: TabId = 2;
    const C: TabId = 3;

    /// F1: two quick switches leave the pool `[A, B]`; later B leaves alone. B's view must sit at
    /// index 0, the one slot the scene draws for a single layer, and keep its rows.
    #[test]
    fn a_tab_fading_alone_gets_the_first_view_and_keeps_its_rows() {
        let (pool, fresh) = arranged(vec![Some(A), Some(B)], &[B]);
        assert_eq!(pool, vec![Some(B), Some(A)]);
        assert_eq!(fresh, vec![false]);
    }

    #[test]
    fn closing_a_fading_tab_moves_the_other_into_the_drawn_slot() {
        // `[A, B]` fading, then A is closed: the layers are `[B]`.
        let (pool, fresh) = arranged(vec![Some(A), Some(B)], &[B]);
        assert_eq!(pool[0], Some(B));
        assert_eq!(fresh, vec![false]);
    }

    #[test]
    fn layers_in_another_order_swap_their_views() {
        let (pool, fresh) = arranged(vec![Some(A), Some(B)], &[B, A]);
        assert_eq!(pool, vec![Some(B), Some(A)]);
        assert_eq!(fresh, vec![false, false]);
    }

    #[test]
    fn a_tab_no_view_holds_takes_a_view_that_holds_no_layer_and_is_rebuilt() {
        let (pool, fresh) = arranged(vec![Some(A), Some(B)], &[C]);
        assert_eq!(fresh, vec![true]);
        // Index 0 is some view of the pool (its stale rows are rebuilt by the caller).
        assert!(pool[0] == Some(A) || pool[0] == Some(B));
    }

    /// The case a one-pass swap gets wrong: a layer that holds no view sits at an index whose
    /// view holds a later layer.
    #[test]
    fn a_view_holding_a_later_layer_is_not_taken_for_an_earlier_one() {
        let (pool, fresh) = arranged(vec![Some(C), Some(A), None], &[A, B, C]);
        assert_eq!(pool[0], Some(A));
        assert_eq!(pool[2], Some(C));
        assert_eq!(pool[1], None);
        assert_eq!(fresh, vec![false, true, false]);
    }

    #[test]
    fn every_view_stays_in_the_pool_and_a_longer_pool_keeps_the_rest_behind() {
        let (pool, fresh) = arranged(vec![Some(C), None, Some(B), Some(A)], &[A, B]);
        assert_eq!(&pool[..2], &[Some(A), Some(B)]);
        assert_eq!(fresh, vec![false, false]);
        let mut sorted = pool.clone();
        sorted.sort();
        assert_eq!(sorted, vec![None, Some(A), Some(B), Some(C)]);
        // No layers: nothing moves needlessly and nothing is fresh.
        let (_, fresh) = arranged(vec![Some(A)], &[]);
        assert!(fresh.is_empty());
    }
}

#[cfg(test)]
mod text_tests {
    use super::*;
    use crate::shell::HOST;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
    const SIZE: (u32, u32) = (640, 400);

    fn renderer() -> Option<Renderer> {
        renderer_with(&Settings::default().clamped())
    }

    fn renderer_with(settings: &Settings) -> Option<Renderer> {
        let gpu = match pollster::block_on(Gpu::new(lune_ui::gpu::instance(None), None)) {
            Ok(gpu) => gpu,
            Err(e) if crate::dump::require_gpu() => {
                panic!("LUNE_REQUIRE_GPU=1 but no usable wgpu adapter: {e}")
            }
            Err(e) => {
                eprintln!("SKIPPED: no wgpu adapter: {e}");
                return None;
            }
        };
        Some(Renderer::new(
            gpu,
            fonts::bundled_text_cache(),
            FORMAT,
            settings,
            true,
        ))
    }

    /// `set_glass` is the one door: the renderer's theme and tint are the pair's.
    #[test]
    fn set_glass_swaps_the_theme_and_the_tint_together() {
        let Some(mut r) = renderer() else { return };
        for glass in [true, false] {
            for look in LookTheme::ALL.iter().copied() {
                r.set_glass(glass, look);
                assert_eq!(
                    r.theme(),
                    &theme(glass, look, WindowOpacity::P0),
                    "{glass} {look:?}"
                );
                assert_eq!(r.header_tint, header_tint(look), "{glass} {look:?}");
            }
        }
    }

    /// The opacity setter rebuilds the theme for the pair in force, and `set_glass` keeps using it.
    #[test]
    fn set_window_opacity_rebuilds_the_theme_and_set_glass_keeps_it() {
        let Some(mut r) = renderer() else { return };
        for look in LookTheme::ALL.iter().copied() {
            for w in WindowOpacity::ALL.iter().copied() {
                r.set_window_opacity(w, true, look);
                assert_eq!(r.theme(), &theme(true, look, w), "{look:?} {w:?}");
                r.set_glass(false, look);
                assert_eq!(r.theme(), &theme(false, look, w), "{look:?} {w:?}");
                r.set_glass(true, look);
                assert_eq!(r.theme(), &theme(true, look, w), "{look:?} {w:?}");
            }
        }
    }

    #[test]
    fn the_setting_picks_the_starting_text_rendering() {
        let Some(r) = renderer() else { return };
        assert_eq!(r.text_rendering(), TextRendering::SMOOTH);
        let off = Settings {
            font_smoothing: false,
            ..Settings::default()
        };
        let Some(r) = renderer_with(&off) else { return };
        assert_eq!(r.text_rendering(), TextRendering::OFF);
    }

    /// One value: in one frame the frame's `FrameParams.text`, the active grid's view and every
    /// fading view (two of them, so the pool grows past one) get the renderer's setting, and a
    /// change reaches all of them on the next frame.
    #[test]
    fn one_setting_reaches_the_frame_and_every_grid_view() {
        let Some(mut r) = renderer() else { return };
        let texture = lune_ui::dump::target(&r.gpu.device, FORMAT, SIZE);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let input = lune_ui::input::RawInput {
            size: (SIZE.0 as f32, SIZE.1 as f32),
            scale: 1.0,
            time: std::time::Duration::ZERO,
            focused: true,
            events: Vec::new(),
        };
        let theme = TermTheme::lunegit_one_dark();
        let (mut active, mut fade_a, mut fade_b) =
            (Grid::new(20, 5), Grid::new(20, 5), Grid::new(20, 5));
        for text in [
            TextRendering::SMOOTH,
            TextRendering::OFF,
            TextRendering::SMOOTH,
        ] {
            r.set_text_rendering(text);
            assert_eq!(r.frame_params((SIZE.0, SIZE.1), 1.0).text, text);
            let content = Content {
                title: None,
                tabs: &[],
                active_tab: None,
                modal: None,
                maximized: false,
                grid: &mut active,
                selection: &[],
                hover_link: 0,
                link_hand: false,
                link_tip: None,
                toasts: None,
                cursor: Cursor::default(),
                blink_on: true,
                scroll: None,
                chrome: HOST,
                ime: false,
                preedit: None,
                clipboard_write: None,
                clipboard_read: false,
                page: None,
                menu: None,
                busy: None,
                busy_tone: Tone::Neutral,
                grid_alpha: 1.0,
                fading: vec![
                    FadingGrid {
                        id: 10,
                        grid: &mut fade_a,
                        cursor: Cursor::default(),
                        theme: &theme,
                        alpha: 0.5,
                    },
                    FadingGrid {
                        id: 11,
                        grid: &mut fade_b,
                        cursor: Cursor::default(),
                        theme: &theme,
                        alpha: 0.25,
                    },
                ],
            };
            r.draw(&input, content, &view, SIZE).expect("draw");
            r.gpu.end_frame(&mut r.text);
            assert_eq!(r.term.builder().text_rendering(), text, "active view");
            assert_eq!(r.fading.len(), 2);
            for (i, fv) in r.fading.iter().enumerate() {
                assert_eq!(fv.view.builder().text_rendering(), text, "fading view {i}");
            }
        }
    }
}

/// The offscreen tab-switch harness (plan step 3): the real [`Renderer`] with two tabs of dense
/// full grids, switched A→B→A→B the way the app does it (`App::active_changed`: both grids marked
/// dirty, the crossfade started, the old tab drawn by a fading view). Prints the phase timings
/// of every frame of each switch to stderr; numbers only. Ignored by default (it is timed); in a
/// release build it fails when a switch's first frame takes 100 ms or more (plan step 4 (c)):
/// `cargo test -p luneterm --lib [--release] switch_harness -- --ignored --nocapture`.
#[cfg(test)]
mod switch_harness {
    use super::*;
    use crate::fade::{self, Fade};
    use crate::shell::HOST;
    use lune_termview::{Attrs, Cell, Color};

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
    const SCALE: f32 = 2.0;
    /// The simulated display: the fade's clock advances one refresh per frame, so every run draws
    /// the same number of fade frames whatever a frame costs.
    const REFRESH: Duration = Duration::from_micros(16_667);

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    /// A full screen of printable ASCII, palette colours on the default background, 20% bold.
    fn dense(cols: usize, rows: usize, seed: u64) -> Grid {
        let mut rng = Rng(seed);
        let mut g = Grid::new(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                let mut cell = Cell::default();
                cell.set_char(char::from(b'!' + (rng.next() % 94) as u8));
                cell.fg = Color::Palette((rng.next() % 16) as u8);
                if rng.next() % 5 == 0 {
                    cell.attrs.insert(Attrs::BOLD);
                }
                g.set(row, col, cell);
            }
        }
        g
    }

    fn renderer(text: TextRendering) -> Option<Renderer> {
        let gpu = match pollster::block_on(Gpu::new(lune_ui::gpu::instance(None), None)) {
            Ok(gpu) => gpu,
            Err(e) if crate::dump::require_gpu() => {
                panic!("LUNE_REQUIRE_GPU=1 but no usable wgpu adapter: {e}")
            }
            Err(e) => {
                eprintln!("SKIPPED: no wgpu adapter: {e}");
                return None;
            }
        };
        let mut r = Renderer::new(
            gpu,
            fonts::bundled_text_cache(),
            FORMAT,
            &Settings::default().clamped(),
            true,
        );
        r.set_text_rendering(text);
        r.set_phase_trace(true);
        Some(r)
    }

    /// The smallest logical window (in 8 px steps) whose grid holds `cols` x `rows`.
    fn window_for(r: &mut Renderer, cols: usize, rows: usize) -> (f32, f32) {
        let mut size = (400.0, 300.0);
        while r.grid_size(size, SCALE).0 < cols {
            size.0 += 8.0;
        }
        while r.grid_size(size, SCALE).1 < rows {
            size.1 += 8.0;
        }
        size
    }

    /// One frame's numbers: the draw's phases and the GPU wait after it, ms from the draw's start.
    struct FrameRow {
        p: DrawPhases,
        gpu: f64,
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1e3
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(
        r: &mut Renderer,
        target: &wgpu::TextureView,
        size: (u32, u32),
        logical: (f32, f32),
        tabs: &mut [Grid; 2],
        active: usize,
        alphas: &fade::Alphas,
        theme: &TermTheme,
    ) -> FrameRow {
        let input = lune_ui::input::RawInput {
            size: logical,
            scale: SCALE,
            time: Duration::ZERO,
            focused: true,
            events: Vec::new(),
        };
        let strip = [
            StripTab {
                id: 0,
                title: "a",
                progress: None,
            },
            StripTab {
                id: 1,
                title: "b",
                progress: None,
            },
        ];
        let [a, b] = tabs;
        let (grid, other) = if active == 0 { (a, b) } else { (b, a) };
        let other_id = 1 - active as TabId;
        let fading: Vec<FadingGrid<'_>> = alphas
            .outgoing
            .iter()
            .find(|(id, _)| *id == other_id)
            .map(|(_, alpha)| FadingGrid {
                id: other_id,
                grid: other,
                cursor: Cursor::default(),
                theme,
                alpha: *alpha,
            })
            .into_iter()
            .collect();
        let content = Content {
            title: None,
            tabs: &strip,
            active_tab: Some(active as TabId),
            modal: None,
            maximized: false,
            grid,
            selection: &[],
            hover_link: 0,
            link_hand: false,
            link_tip: None,
            toasts: None,
            cursor: Cursor::default(),
            blink_on: true,
            scroll: None,
            chrome: HOST,
            ime: false,
            preedit: None,
            clipboard_write: None,
            clipboard_read: false,
            page: None,
            menu: None,
            busy: None,
            busy_tone: Tone::Neutral,
            grid_alpha: alphas.incoming,
            fading,
        };
        r.draw(&input, content, target, size).expect("draw");
        r.gpu.end_frame(&mut r.text);
        let _ = r.gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let p = *r.phases().expect("phases on");
        FrameRow {
            p,
            gpu: ms(p.start.elapsed()),
        }
    }

    fn print(label: &str, n: usize, f: usize, row: &FrameRow) {
        let p = &row.p;
        eprintln!(
            "harness: {label} switch={n} f={f} ui={:.2} active={:.2} assign={:.2} fading={:.2} \
             draw_with={:.2} gpu_done={:.2} views_created={} rows_active={} full={} \
             rows_fading={} rasters_font={} rasters_custom={}",
            ms(p.ui),
            ms(p.active),
            ms(p.assign),
            ms(p.fading),
            ms(p.draw_with),
            row.gpu,
            p.views_created,
            p.active_rows,
            u8::from(p.active_full),
            p.fading_rows,
            p.font_rasters,
            p.custom_rasters,
        );
    }

    /// The timed check (plan step 4 (c)): a switch's first frame, from the draw's start to the GPU
    /// done, must stay under this in a release build. A debug build only prints.
    const FIRST_FRAME_LIMIT_MS: f64 = 100.0;

    /// Runs one size and text setting; returns the slowest first frame after a switch, ms.
    fn run(text: TextRendering, cols: usize, rows: usize) -> f64 {
        let Some(mut r) = renderer(text) else {
            return 0.0;
        };
        let logical = window_for(&mut r, cols, rows);
        let size = (
            (logical.0 * SCALE).round() as u32,
            (logical.1 * SCALE).round() as u32,
        );
        let texture = lune_ui::dump::target(&r.gpu.device, FORMAT, size);
        let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let theme = TermTheme::lunegit_one_dark();
        let mut tabs = [
            dense(cols, rows, 0x9E37_79B9),
            dense(cols, rows, 0x7F4A_7C15),
        ];
        let build = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let smooth = if text == TextRendering::SMOOTH {
            "on"
        } else {
            "off"
        };
        let label = format!(
            "{build} text={smooth} grid={cols}x{rows} target={}x{}",
            size.0, size.1
        );
        let mut fade = Fade::default();
        let mut now = Instant::now();
        let mut active = 0usize;
        // Tab A settles: its first frame builds everything, then two idle frames.
        for f in 0..3 {
            let (alphas, _) = fade.tick(now, |_| true);
            let row = frame(
                &mut r, &target, size, logical, &mut tabs, active, &alphas, &theme,
            );
            print(&label, 0, f, &row);
            now += REFRESH;
        }
        let mut worst = 0.0f64;
        for n in 1..=4 {
            // `App::active_changed`: the fade starts, both grids are drawn from scratch.
            let old = active;
            active = 1 - active;
            fade.switch(now, fade::FULL, Some(old as TabId), active as TabId);
            tabs[old].mark_all_dirty();
            tabs[active].mark_all_dirty();
            let mut f = 1;
            loop {
                let (alphas, running) = fade.tick(now, |_| true);
                let row = frame(
                    &mut r, &target, size, logical, &mut tabs, active, &alphas, &theme,
                );
                print(&label, n, f, &row);
                if f == 1 {
                    worst = worst.max(row.gpu);
                }
                now += REFRESH;
                f += 1;
                if !running {
                    break;
                }
            }
        }
        eprintln!("harness: {label} first_frame_max={worst:.2}");
        worst
    }

    #[test]
    #[ignore = "timed; run by hand in release (plan steps 3 and 4)"]
    fn tab_switch_phases() {
        let texts: &[TextRendering] = match std::env::var("LUNETERM_DUMP_TEXT").as_deref() {
            Ok("on") => &[TextRendering::SMOOTH],
            Ok("off") => &[TextRendering::OFF],
            _ => &[TextRendering::SMOOTH, TextRendering::OFF],
        };
        for &text in texts {
            for (cols, rows) in [(240, 63), (120, 40)] {
                let worst = run(text, cols, rows);
                if !cfg!(debug_assertions) {
                    assert!(
                        worst < FIRST_FRAME_LIMIT_MS,
                        "a switch's first frame took {worst:.2} ms (limit {FIRST_FRAME_LIMIT_MS})"
                    );
                }
            }
        }
    }
}
