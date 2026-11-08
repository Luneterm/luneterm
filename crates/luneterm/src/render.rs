//! One frame, composed (plan M9 step 4): the toolkit `Ui` draws the shell, the termview draws the
//! grid into the shell's slot, and the frame renderer puts both into a target. The window and the
//! frame dumps call the same [`Renderer::draw`], so a dump shows what the window shows.

use lune_termview::{Cursor, Grid, TermTheme, TermView, ViewState};
use lune_ui::frame::FrameRenderer;
use lune_ui::gpu::Gpu;
use lune_ui::input::{FrameOutput, keymap};
use lune_ui::lunegit_dark::THEME;
use lune_ui::motion::MotionTier;
use lune_ui::paint::Rect;
use lune_ui::scene::FrameParams;
use lune_ui::shortcuts::Shortcuts;
use lune_ui::strings::{DisplayFormat, StringTable};
use lune_ui::text::TextCache;
use lune_ui::theme::{AcrylicLevel, Theme};
use lune_ui::ui::{FrameConfig, Ui, UiStore};

use crate::fonts;
use crate::settings::Settings;
use crate::shell::{self, Chrome, Layout, ScrollView, ShellView};

/// The saturation the backdrop blur applies (Luneium's menu glass; no backdrop is requested in M9).
const BACKDROP_SATURATION: f32 = 1.8;

/// The toolkit theme for a window with or without OS glass: without it (Linux, or a material the
/// OS refused) every glass layer is opaque.
pub fn theme(glass: bool) -> Theme {
    if glass {
        THEME
    } else {
        THEME.with_acrylic(AcrylicLevel::Opaque)
    }
}

/// What a frame shows besides the shell.
#[derive(Debug)]
pub struct Content<'a> {
    pub title: &'a str,
    pub maximized: bool,
    pub grid: &'a mut Grid,
    pub cursor: Cursor,
    pub blink_on: bool,
    pub scroll: Option<ScrollView>,
    pub chrome: Chrome,
}

/// What [`Renderer::draw`] hands back.
#[derive(Debug)]
pub struct Drawn {
    /// The window operations, repaint request and clipboard requests of the frame, the shell
    /// chrome's commands included.
    pub output: FrameOutput,
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
    store: UiStore,
    strings: StringTable,
    shortcuts: Shortcuts,
    theme: Theme,
    tier: MotionTier,
    blur_sigma: f32,
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
        let term = TermView::new(
            &gpu.device,
            format,
            term_fonts,
            TermTheme::lunegit_one_dark(),
        );
        Renderer {
            gpu,
            text,
            frame,
            term,
            store: UiStore::default(),
            strings: StringTable::default(),
            shortcuts: Shortcuts::default(),
            theme: theme(glass),
            tier: settings.tier(),
            blur_sigma: settings.menu_blur.sigma(),
        }
    }

    /// The grid (cols, rows) that fits a window of `size` logical px at `scale`.
    pub fn grid_size(&mut self, size: (f32, f32), scale: f32) -> (usize, usize) {
        let metrics = self.term.metrics(&mut self.text.fonts, scale);
        let placement = shell::place_grid(
            Layout::new(Rect::new(0.0, 0.0, size.0, size.1)).body,
            metrics,
        );
        (placement.cols, placement.rows)
    }

    /// Draws one frame of `input`'s size and scale into `target` (`size` physical px). The grid
    /// is resized to what fits the body; the caller presents `target` and calls
    /// `gpu.end_frame(&mut text)`.
    pub fn draw(
        &mut self,
        input: &lune_ui::input::RawInput,
        content: Content<'_>,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) -> Result<Drawn, String> {
        let scale = input.scale;
        let win = Rect::new(0.0, 0.0, input.size.0, input.size.1);
        let metrics = self.term.metrics(&mut self.text.fonts, scale);
        let placement = shell::place_grid(Layout::new(win).body, metrics);
        if content.grid.cols() != placement.cols || content.grid.rows() != placement.rows {
            content.grid.resize(placement.cols, placement.rows);
        }

        let config = FrameConfig {
            theme: self.theme,
            tier: self.tier,
            strings: &self.strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &self.shortcuts,
        };
        let mut ui = Ui::begin(&mut self.store, input, config, &mut self.text);
        let view = ShellView {
            title: content.title,
            maximized: content.maximized,
            focused: input.focused,
            scroll: content.scroll,
            cell_h: metrics.cell_h as f32 / metrics.scale,
            chrome: content.chrome,
        };
        let shell_out = shell::draw(&mut ui, &view);
        let (scene, mut output) = ui.finish();
        output.commands.extend(shell_out.commands);

        let selection = [];
        let view_state = ViewState {
            cursor: content.cursor,
            focused: input.focused,
            blink_on: content.blink_on,
            selection: &selection,
            hover_link: 0,
        };
        self.term.update(
            &mut self.text.fonts,
            content.grid,
            &view_state,
            placement.origin,
            scale,
        );
        let params = FrameParams {
            size,
            scale,
            blur_sigma: self.blur_sigma * scale,
            saturation: BACKDROP_SATURATION,
            ..FrameParams::default()
        };
        self.frame.draw_with(
            &mut self.gpu,
            &mut self.text,
            &scene,
            target,
            params,
            &mut [&mut self.term],
        )?;
        Ok(Drawn {
            output,
            cols: placement.cols,
            rows: placement.rows,
        })
    }
}
