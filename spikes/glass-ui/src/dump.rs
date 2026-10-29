//! Scripted offscreen frame dumps (`GLASS_UI_DUMP_FRAMES=<dir>`): the pixel instrument that the
//! library extraction is gated on.
//!
//! Each scene renders into its own offscreen texture of the surface format with a frozen clock,
//! and the texture is read back (`lune_ui::dump`: GPU readback only, no window or screen capture) and
//! written as an RGBA PNG with alpha preserved. Compare two dump directories with the `dump_compare`
//! example.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::state::{GlassStyle, Hit, MenuKind, Modal, Page, Popup, Section, State};
use lune_ui::dump;

use crate::{Renderer, draw_frame};

/// Logical window size of every scene (the spike window's initial size).
const W: u32 = 1040;
const H: u32 = 660;

/// Default frame clock offset from the anchors: fades are done, menus fully open.
const SETTLED: Duration = Duration::from_secs(10);

type AfterWarmUp = fn(&mut State) -> Result<(), String>;

struct Scene {
    name: &'static str,
    scale: f32,
    /// Frame clock offset from the anchors.
    age: Duration,
    setup: fn(&mut State),
    /// Runs after the warm-up frame, once `st.hits` is known.
    after_warm_up: Option<AfterWarmUp>,
}

const fn scene(name: &'static str, setup: fn(&mut State)) -> Scene {
    Scene {
        name,
        scale: 2.0,
        age: SETTLED,
        setup,
        after_warm_up: None,
    }
}

const APP_MENU: Popup = Popup::Menu {
    kind: MenuKind::App,
    at: (500.0, 120.0),
};

/// The fixed scene list, in file-number order.
fn scenes() -> Vec<Scene> {
    vec![
        scene("tab-zsh", |_| {}),
        scene("tab-build", |st| st.active = 1),
        scene("tab-claude", |st| st.active = 2),
        scene("tab-colors", |st| st.active = 3),
        scene("settings-appearance", |st| {
            st.page = Page::Settings(Section::Appearance);
        }),
        scene("settings-terminal", |st| {
            st.page = Page::Settings(Section::Terminal);
        }),
        scene("settings-about", |st| {
            st.page = Page::Settings(Section::About);
        }),
        scene("menu-app", |st| st.popup = Some(APP_MENU)),
        Scene {
            age: Duration::from_millis(90),
            ..scene("menu-app-opening", |st| st.popup = Some(APP_MENU))
        },
        scene("menu-tab", |st| {
            st.popup = Some(Popup::Menu {
                kind: MenuKind::Tab(1),
                at: (300.0, 60.0),
            });
        }),
        Scene {
            after_warm_up: Some(|st| {
                let (id, anchor) = st
                    .hits
                    .iter()
                    .find_map(|(r, h)| match h {
                        Hit::Combo(id) => Some((*id, *r)),
                        _ => None,
                    })
                    .ok_or("no combo on the Appearance page")?;
                st.popup = Some(Popup::Combo { id, anchor });
                Ok(())
            }),
            ..scene("combo", |st| {
                st.page = Page::Settings(Section::Appearance);
            })
        },
        scene("modal-confirm-close", |st| {
            st.modal = Some(Modal::ConfirmClose {
                tabs: vec![1],
                dont_ask: false,
            });
        }),
        scene("modal-rename", |st| {
            st.modal = Some(Modal::Rename {
                tab: 0,
                text: "zsh".to_owned(),
            });
        }),
        scene("modal-about", |st| st.modal = Some(Modal::About)),
        scene("glass-off", |st| st.settings.glass = GlassStyle::Off),
        scene("unfocused", |st| st.focused = false),
        scene("selection", |st| {
            st.tabs[0].selection = Some(((2, 1), (20, 3)));
        }),
        Scene {
            scale: 1.0,
            ..scene("tab-zsh", |_| {})
        },
    ]
}

/// A fresh state for one scene: all clock anchors at `base`, the mouse parked off the window
/// so nothing is hovered, focused.
fn fresh_state(base: Instant) -> State {
    let mut st = State::new();
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        // Same as the live app: no glass on this OS.
        st.settings.glass = GlassStyle::Off;
    }
    st.focused = true;
    st.mouse = (-100.0, -100.0);
    st.hover = None;
    st.started = base;
    st.switched_at = base;
    st.popup_at = base;
    st.closing = None;
    st.now = base;
    st
}

/// Renders every scene and writes `NN-name@Sx.png` into `dir`.
pub fn dump_frames(r: &mut Renderer, st: &mut State, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let base = Instant::now();
    for (i, scene) in scenes().into_iter().enumerate() {
        let file = format!("{:02}-{}@{}x.png", i + 1, scene.name, scene.scale as u32);
        render_scene(r, st, base, &scene, &dir.join(&file))
            .map_err(|e| format!("scene {file}: {e}"))?;
        eprintln!("dumped {file}");
    }
    Ok(())
}

fn render_scene(
    r: &mut Renderer,
    st: &mut State,
    base: Instant,
    scene: &Scene,
    path: &Path,
) -> Result<(), String> {
    *st = fresh_state(base);
    (scene.setup)(st);
    st.now = base + scene.age;
    let s = scene.scale;
    let size = ((W as f32 * s) as u32, (H as f32 * s) as u32);
    let texture = dump::target(&r.gpu.device, r.gpu.config.format, size);
    let view = texture.create_view(&Default::default());
    let frame = |r: &mut Renderer, st: &mut State| -> Result<(), String> {
        draw_frame(r, st, s, &view, size)?;
        st.hover = None;
        r.frame.end_frame(&mut r.text);
        Ok(())
    };
    // Warm-up: fills `st.hits` and the scroll extent; the second frame is the one captured.
    frame(r, st)?;
    if let Some(f) = scene.after_warm_up {
        f(st)?;
    }
    frame(r, st)?;
    let rgba = dump::read_rgba(&r.gpu.device, &r.gpu.queue, &texture)?;
    dump::write_png(path, size, &rgba)
}
