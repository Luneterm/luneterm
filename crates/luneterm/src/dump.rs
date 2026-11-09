//! Frame dumps (`LUNETERM_DUMP_FRAMES=<dir>`): the shell and a demo grid drawn offscreen on a
//! headless device and written as PNGs, with no window and no screen capture (the pixels come from
//! reading the offscreen target back). The frames are what the window would show, through the same
//! [`Renderer::draw`].
//!
//! The scenes are the shell with one default tab, and the tab strip, the close-confirm and rename
//! dialogs and a failed start (M10 step 2), then the settings page in its sections, the menus, a
//! palette switch and the opaque look Linux gets (step 6); a dialog or menu is drawn after its
//! entry animation, and a menu or scroll the toolkit owns is driven with scripted input.
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
use lune_ui::widgets::menu::MenuPlace;
use lune_ui::widgets::popover::{PlaceAlign, Side};
use lune_ui::widgets::tone::Tone;

use crate::app::failure_grid;
use crate::demo::{demo_cursor, demo_grid};
use crate::fonts;
use crate::links;
use crate::menus::{self, MenuKind, MenuModel};
use crate::render::{Content, LinkTip, MenuDraw, Renderer};
use crate::settings::Settings;
use crate::settings_page::{PageState, PageView, Section};
use crate::shell::{HOST, Layout, Modal, ScrollView, StripTab};
use crate::spawn::SpawnFailure;
use crate::tabs::confirm_message;

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
}

/// The settings page a scene shows.
#[derive(Debug, Clone, Copy)]
struct PageScene {
    section: Section,
    /// The Glass style list is open.
    combo: bool,
    /// The notice banner is up.
    restart: bool,
}

/// An open menu, forced by the dump (the tab's own menu is the toolkit's: scripted input).
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
    SecondaryClick(f32, f32),
    /// A wheel of `y` lines with the pointer at the point.
    Wheel(f32, f32, f32),
}

impl Input {
    fn events(self) -> Vec<Event> {
        let mods = Modifiers::default();
        match self {
            Input::Move(x, y) => vec![Event::PointerMoved { pos: (x, y) }],
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
    page: Option<PageScene>,
    menu: MenuScene,
    /// The terminal palette, by name.
    palette: &'static str,
    /// Input at the start of frame `.0`.
    script: &'static [(usize, Input)],
    /// A selection on the demo grid.
    selected: bool,
    /// A URL on the demo grid under the pointer: underlined, and its tooltip once the delay has
    /// passed.
    link: Option<LinkScene>,
}

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
    page: None,
    menu: MenuScene::None,
    palette: "One Dark",
    script: &[],
    selected: false,
    link: None,
};

const fn page(section: Section) -> Option<PageScene> {
    Some(PageScene {
        section,
        combo: false,
        restart: false,
    })
}

/// A pointer over the page's content (so a wheel scrolls it).
const OVER_PAGE: (f32, f32) = (600.0, 300.0);
/// The second tab (`cargo build`, index 1) of the strip.
const OVER_TAB: (f32, f32) = (170.0, 50.0);

const SCENES: [Scene; 34] = [
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
            combo: true,
            restart: false,
        }),
        ..BASE
    },
    Scene {
        name: "settings-restart@2x.png",
        tabs: FOUR,
        page: Some(PageScene {
            section: Section::Terminal,
            combo: false,
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
];

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

/// Renders the scenes into `dir` (created if absent) and returns the files written.
pub fn dump_frames(dir: &Path) -> Result<Vec<PathBuf>, DumpError> {
    dump_frames_gated(dir).map(|v| v.into_iter().map(|(p, _)| p).collect())
}

/// [`dump_frames`] with the least alpha of each frame (an opaque scene must have 255 everywhere).
pub fn dump_frames_gated(dir: &Path) -> Result<Vec<(PathBuf, u8)>, DumpError> {
    let gpu = pollster::block_on(Gpu::new(gpu::instance(None), None)).map_err(DumpError::NoGpu)?;
    std::fs::create_dir_all(dir)
        .map_err(|e| DumpError::Failed(format!("create {}: {e}", dir.display())))?;
    let (text, took) = fonts::text_cache();
    eprintln!("system fonts loaded in {} ms", took.as_millis());
    let settings = Settings::default().clamped();
    let mut renderer = Renderer::new(gpu, text, FORMAT, &settings, true);
    let mut written = Vec::new();
    for scene in SCENES {
        let path = dir.join(scene.name);
        let alpha = dump_scene(&mut renderer, &scene, &path)
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

/// Draws `scene` into a fresh target, warming up first as the toolkit's own dump does, and writes
/// the last frame.
fn dump_scene(r: &mut Renderer, scene: &Scene, path: &Path) -> Result<u8, String> {
    r.reset_ui();
    r.set_glass(scene.glass);
    r.set_term_theme(crate::palettes::theme_of(scene.palette));
    let scale = scene.scale as f32;
    let size = (SIZE.0 as u32 * scene.scale, SIZE.1 as u32 * scene.scale);
    let (cols, rows) = r.grid_size(SIZE, scale);
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
        let rect = r.place(SIZE, scale).grid_layout().cell_rect(
            span.row,
            span.start,
            span.end + 1 - span.start,
        );
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
    let strip: Vec<StripTab<'_>> = titles
        .iter()
        .enumerate()
        .map(|(i, title)| StripTab {
            id: i as u64 + 1,
            title,
            progress: None,
        })
        .collect();
    let active = scene.active.min(strip.len().saturating_sub(1));
    let label = labelled.then(|| titles[active]);
    let message = match scene.overlay {
        Overlay::Confirm(n) => confirm_message(n),
        _ => String::new(),
    };
    let mut dont_ask = false;
    let mut name = String::new();
    let page_settings = Settings::default().clamped();
    let mut edited = page_settings.clone();
    let mut page_state = scene.page.map(|p| {
        let mut state = PageState::new(p.section, &page_settings);
        // The Glass style row is the first combo of the Appearance section.
        state.combo = p.combo.then_some(crate::settings::SettingId::Glass);
        state
    });
    let selection = [SelectionRange {
        start: (2, 4),
        end: (4, 30),
        kind: SelectionKind::Linear,
    }];
    let strings = crate::strings::english();
    let model = match scene.menu {
        MenuScene::None => None,
        MenuScene::App => Some(MenuModel::new(
            MenuKind::App,
            &strings,
            cfg!(target_os = "macos"),
        )),
        MenuScene::Terminal { selection } => Some(MenuModel::new(
            MenuKind::Terminal { selection },
            &strings,
            cfg!(target_os = "macos"),
        )),
    };
    let place = match scene.menu {
        MenuScene::App => MenuPlace::Anchor {
            rect: Layout::new(Rect::new(0.0, 0.0, SIZE.0, SIZE.1)).overflow,
            side: Side::Bottom,
            align: PlaceAlign::End,
            gap: 4.0,
        },
        _ => MenuPlace::At(TERMINAL_MENU_AT),
    };
    let extra = menus::tab_extra(&strings);
    let extra: [&str; 2] = [extra[0].as_str(), extra[1].as_str()];

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
        let input = RawInput {
            size: SIZE,
            scale,
            time: Duration::from_millis(*ms),
            focused: scene.focused,
            events,
        };
        let modal = match scene.overlay {
            Overlay::None => None,
            Overlay::Confirm(_) => Some(Modal::Confirm {
                message: &message,
                dont_ask: &mut dont_ask,
            }),
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
            toasts: None,
            cursor,
            blink_on: true,
            scroll: scene.scroll,
            chrome: HOST,
            ime: false,
            preedit: None,
            clipboard_write: None,
            clipboard_read: false,
            page: page_state.as_mut().map(|state| PageView {
                state,
                settings: &mut edited,
                restart: scene.page.is_some_and(|p| p.restart),
                focused: scene.focused,
            }),
            menu: model.as_ref().map(|m| MenuDraw {
                rows: m.rows(),
                place,
                name: strings.tr(m.name_key()),
            }),
            busy: None,
            busy_tone: Tone::Neutral,
            tab_menu_extra: &extra,
        };
        r.draw(&input, content, &view, size)?;
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
    let result = dump_frames(dir);
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
                .is_some_and(|n| n.starts_with("opaque"));
            if opaque {
                assert_eq!(*alpha, 255, "{} has a transparent pixel", path.display());
            }
        }
        assert_eq!(
            gated
                .iter()
                .filter(|(p, _)| p.to_string_lossy().contains("opaque"))
                .count(),
            5
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
        // The tooltip is not there at the first dwell and is there after the delay.
        assert_ne!(named("link-hover@2x.png"), named("link-tip@2x.png"));
        assert_ne!(named("link-hover@2x.png"), tabs);
        std::fs::remove_dir_all(&a).expect("remove a");
        std::fs::remove_dir_all(&b).expect("remove b");
    }
}
