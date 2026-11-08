//! Frame dumps (`LUNETERM_DUMP_FRAMES=<dir>`): the shell and a demo grid drawn offscreen on a
//! headless device and written as PNGs, with no window and no screen capture (the pixels come from
//! reading the offscreen target back). The frames are what the window would show, through the same
//! [`Renderer::draw`].
//!
//! The scenes are the shell with one default tab, and the tab strip, the close-confirm and rename
//! dialogs and a failed start (M10 step 2); a dialog is drawn after its entry animation.
//!
//! The fonts are the window's, system fonts included (the demo has an emoji), so the PNGs are
//! reproducible on one machine and may differ between machines.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use lune_ui::dump::{read_rgba, target, write_png};
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::RawInput;

use crate::app::failure_grid;
use crate::demo::{demo_cursor, demo_grid};
use crate::fonts;
use crate::render::{Content, Renderer};
use crate::settings::Settings;
use crate::shell::{HOST, Modal, ScrollView, StripTab};
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
};

const SCENES: [Scene; 14] = [
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
];

/// Why a dump did not happen.
#[derive(Debug)]
pub enum DumpError {
    /// No usable GPU adapter or device: not a failure of the app.
    NoGpu(String),
    Failed(String),
}

/// Renders the scenes into `dir` (created if absent) and returns the files written.
pub fn dump_frames(dir: &Path) -> Result<Vec<PathBuf>, DumpError> {
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
        dump_scene(&mut renderer, &scene, &path)
            .map_err(|e| DumpError::Failed(format!("{}: {e}", scene.name)))?;
        eprintln!("dumped {}", scene.name);
        written.push(path);
    }
    Ok(written)
}

/// The frames drawn per scene: the first starts what animates (a dialog opening, a tab revealed),
/// the last, after the focus animations, is the picture. A scene's clock starts at zero on a fresh UI
/// state, so a scene cannot show what an earlier one left behind.
const FRAME_TIMES_MS: [u64; 4] = [0, 800, 1600, 2400];

/// Draws `scene` into a fresh target, warming up first as the toolkit's own dump does, and writes
/// the last frame.
fn dump_scene(r: &mut Renderer, scene: &Scene, path: &Path) -> Result<(), String> {
    r.reset_ui();
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

    for (k, ms) in FRAME_TIMES_MS.iter().enumerate() {
        let input = RawInput {
            size: SIZE,
            scale,
            time: Duration::from_millis(*ms),
            focused: scene.focused,
            events: Vec::new(),
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
            selection: &[],
            toasts: None,
            cursor,
            blink_on: true,
            scroll: scene.scroll,
            chrome: HOST,
            ime: false,
            preedit: None,
            clipboard_write: None,
            clipboard_read: false,
        };
        r.draw(&input, content, &view, size)?;
        r.gpu.end_frame(&mut r.text);
    }
    let rgba = read_rgba(&r.gpu.device, &r.gpu.queue, &texture)?;
    write_png(path, size, &rgba)
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
        let first = match dump_frames(&a) {
            Ok(f) => f,
            Err(DumpError::NoGpu(e)) if !require_gpu() => {
                eprintln!("SKIPPED: no wgpu adapter: {e}");
                return;
            }
            Err(e) => panic!("dump: {e:?}"),
        };
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
        std::fs::remove_dir_all(&a).expect("remove a");
        std::fs::remove_dir_all(&b).expect("remove b");
    }
}
