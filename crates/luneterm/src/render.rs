//! One frame, composed (plan M9 step 4): the toolkit `Ui` draws the shell, the termview draws the
//! grid into the shell's slot, and the frame renderer puts both into a target. The window and the
//! frame dumps call the same [`Renderer::draw`], so a dump shows what the window shows.

use lune_termview::{
    CellMetrics, Cursor, Grid, GridLayout, SelectionRange, TermFonts, TermTheme, TermView,
    ViewState,
};
use lune_ui::color::premul;
use lune_ui::custom::CustomDraw;
use lune_ui::frame::FrameRenderer;
use lune_ui::gpu::Gpu;
use lune_ui::input::{FrameOutput, keymap};
use lune_ui::lunegit_dark::THEME;
use lune_ui::motion::MotionTier;
use lune_ui::near_black;
use lune_ui::paint::Rect;
use lune_ui::scene::{FrameParams, LayerId};
use lune_ui::shortcuts::Shortcuts;
use lune_ui::strings::{DisplayFormat, StringTable};
use lune_ui::text::{Spec, Style, TextCache};
use lune_ui::theme::{AcrylicLevel, Theme};
use lune_ui::ui::{FrameConfig, Sense, Ui, UiStore};
use lune_ui::widgets::menu::{MenuOptions, MenuPlace, MenuResponse, MenuRow};
use lune_ui::widgets::tab_strip::BusyCue;
use lune_ui::widgets::toast::Toasts;
use lune_ui::widgets::tone::Tone;

use crate::fonts;
use crate::links;
use crate::session::Geometry;
use crate::settings::{GlassStyle, Settings};
use crate::settings_page::{self, PageOut, PageView};
use crate::shell::{
    self, Chrome, GridPlacement, Layout, Modal, ModalOut, ScrollView, ShellView, StripTab,
};
use crate::tabs::TabId;

/// The saturation the backdrop blur applies (Luneium's menu glass; no backdrop is requested in M9).
const BACKDROP_SATURATION: f32 = 1.8;

/// The toolkit theme for a window with or without OS glass: without it (Linux, or a material the
/// OS refused) every glass layer is opaque. With it, the default style is the near-black look (the
/// toolkit's `near_black::THEME`, whole, so floating surfaces take it too) and the liquid styles
/// keep Lunegit's layers.
pub fn theme(glass: bool, style: GlassStyle) -> Theme {
    match (glass, style) {
        (true, GlassStyle::Default) => near_black::THEME,
        (true, _) => THEME,
        (false, _) => THEME.with_acrylic(AcrylicLevel::Opaque),
    }
}

/// The tab header's wash over the card, for the theme [`theme`] returns for the same arguments: the
/// near-black look's with its theme (the app's own value, see `shell::NEAR_BLACK_HEADER_TINT`), the
/// Lunegit one otherwise (luneui-rs D8: the header is a material of the look, so the app passes it).
pub fn header_tint(glass: bool, style: GlassStyle) -> [u8; 4] {
    match (glass, style) {
        (true, GlassStyle::Default) => shell::NEAR_BLACK_HEADER_TINT,
        _ => shell::HEADER_TINT,
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
    /// The rows after the toolkit's four on a tab's context menu.
    pub tab_menu_extra: &'a [&'a str],
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
    /// The grid size the frame was laid out for.
    pub cols: usize,
    pub rows: usize,
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
    term_theme: TermTheme,
    font_size: f32,
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
            theme: theme(glass, settings.glass),
            header_tint: header_tint(glass, settings.glass),
            tier: settings.tier(),
            blur_sigma: settings.menu_blur.sigma(),
            term_theme,
            font_size,
        }
    }

    /// Forgets the toolkit's retained state (focus, scroll offsets, open dialogs, animations). The
    /// frame dumps call it between scenes so one scene cannot leak into the next.
    pub fn reset_ui(&mut self) {
        self.store = UiStore::default();
    }

    /// Whether OS glass is behind the window: the glass theme, or the opaque one (P22).
    pub fn set_glass(&mut self, glass: bool, style: GlassStyle) {
        self.theme = theme(glass, style);
        self.header_tint = header_tint(glass, style);
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

    /// Gives each fading layer a view: the one that already holds that tab, else a free one
    /// (made when the pool is short), whose rows are then rebuilt from the tab's grid. Returns the
    /// view index per layer. The pool never shrinks (a fade is 250 ms; the views are cheap to keep).
    fn assign_fading(&mut self, layers: &mut [FadingGrid<'_>]) -> Vec<usize> {
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
            self.fading.push(FadingView {
                view,
                owner: None,
                theme,
            });
        }
        let mut used = vec![false; self.fading.len()];
        let mut slots = vec![usize::MAX; layers.len()];
        for (i, l) in layers.iter().enumerate() {
            if let Some(j) = self.fading.iter().position(|v| v.owner == Some(l.id)) {
                slots[i] = j;
                used[j] = true;
            }
        }
        for (i, l) in layers.iter_mut().enumerate() {
            if slots[i] != usize::MAX {
                continue;
            }
            let Some(j) = used.iter().position(|u| !*u) else {
                continue;
            };
            used[j] = true;
            slots[i] = j;
            self.fading[j].owner = Some(l.id);
            l.grid.mark_all_dirty();
        }
        for (l, &j) in layers.iter().zip(&slots) {
            if let Some(fv) = self.fading.get_mut(j)
                && fv.theme != *l.theme
            {
                fv.view.set_theme(l.theme.clone());
                fv.theme = l.theme.clone();
            }
        }
        slots
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
            menu_extra: content.tab_menu_extra,
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
        );
        // The tabs fading out under it (a tab-switch crossfade), each in a view of its own.
        let slots = self.assign_fading(&mut content.fading);
        for (layer, slot) in content.fading.iter_mut().zip(slots) {
            let Some(fv) = self.fading.get_mut(slot) else {
                continue;
            };
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
            );
        }
        let params = FrameParams {
            size,
            scale,
            blur_sigma: self.blur_sigma * scale,
            saturation: BACKDROP_SATURATION,
            ..FrameParams::default()
        };
        // Slot 0 is the active tab's grid, the rest are the fading ones (`shell::TERM_SLOT`).
        let mut customs: Vec<&mut dyn CustomDraw> = Vec::with_capacity(1 + self.fading.len());
        customs.push(&mut self.term);
        customs.extend(
            self.fading
                .iter_mut()
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
        Ok(Drawn {
            output,
            tab_events,
            modal,
            page: page_out,
            menu: menu_resp,
            overflow: shell_out.overflow,
            cols: placement.cols,
            rows: placement.rows,
        })
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    const LIQUID: [GlassStyle; 2] = [GlassStyle::LiquidClear, GlassStyle::LiquidRegular];

    #[test]
    fn default_glass_is_the_near_black_theme_whole() {
        // Floating surfaces fill from `menu_glass`: the whole theme, not only its layers.
        let t = theme(true, GlassStyle::Default);
        assert_eq!(t, near_black::THEME);
        assert_eq!(t.glass, near_black::GLASS);
        assert_eq!(t.menu_glass, near_black::MENU_GLASS);
        assert_ne!(t.glass, THEME.glass);
        assert_ne!(t.menu_glass, THEME.menu_glass);
    }

    #[test]
    fn liquid_styles_keep_lunegits_layers() {
        for style in LIQUID {
            let t = theme(true, style);
            assert_eq!(t, THEME, "{style:?}");
            assert_eq!(t.glass, THEME.glass, "{style:?}");
            assert_eq!(t.menu_glass, THEME.menu_glass, "{style:?}");
        }
    }

    #[test]
    fn no_glass_is_opaque_whatever_the_style() {
        // Linux, `Off`, a refused material: opaque layers, never `with_acrylic` on the near-black
        // theme (its layer would come out a lighter grey).
        let opaque = THEME.with_acrylic(AcrylicLevel::Opaque);
        for style in GlassStyle::ALL.iter().copied() {
            let t = theme(false, style);
            assert_eq!(t, opaque, "{style:?}");
            assert_eq!(t.glass, opaque.glass, "{style:?}");
            assert_eq!(t.menu_glass, opaque.menu_glass, "{style:?}");
        }
    }

    #[test]
    fn the_header_tint_follows_the_theme_in_use() {
        assert_eq!(
            header_tint(true, GlassStyle::Default),
            shell::NEAR_BLACK_HEADER_TINT
        );
        assert_eq!(shell::NEAR_BLACK_HEADER_TINT, [0xFF, 0xFF, 0xFF, 0x10]);
        for style in LIQUID {
            assert_eq!(header_tint(true, style), shell::HEADER_TINT, "{style:?}");
        }
        for style in GlassStyle::ALL.iter().copied() {
            assert_eq!(header_tint(false, style), shell::HEADER_TINT, "{style:?}");
        }
        assert_ne!(shell::NEAR_BLACK_HEADER_TINT, shell::HEADER_TINT);
    }

    #[test]
    fn a_style_change_with_the_same_glass_changes_theme_and_tint() {
        // `Effect::Glass` hands the renderer the glass bool and the style; the pair is the whole
        // input, so a Default <-> Liquid switch cannot be a no-op.
        assert_ne!(
            theme(true, GlassStyle::Default),
            theme(true, GlassStyle::LiquidRegular)
        );
        assert_ne!(
            header_tint(true, GlassStyle::Default),
            header_tint(true, GlassStyle::LiquidRegular)
        );
    }
}
