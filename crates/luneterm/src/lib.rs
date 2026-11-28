//! Luneterm app library: the session core and the app shell, so the app's logic is testable and
//! M10 builds on it (plan M9 D19).

// `unsafe` only in platform modules (links/open.rs, Windows; os_event.rs and efficiency/macos.rs,
// macOS; efficiency/windows.rs, ctl/endpoint/windows.rs and ctl/token.rs's `random`, Windows;
// shell_integration/mod.rs and ctl/endpoint/unix.rs, Unix), each block with a SAFETY comment.
#![deny(unsafe_code)]

pub mod app;
pub mod ctl;
pub mod cwd;
pub mod demo;
pub mod double_click;
pub mod dump;
pub mod efficiency;
pub mod fade;
pub mod fonts;
pub mod groups;
pub mod input;
pub mod links;
pub mod menus;
pub mod notices;
mod os_event;
pub mod outline;
pub mod pacing;
pub mod palettes;
pub mod pane;
pub mod persist;
mod raise;
pub mod render;
pub mod session;
pub mod settings;
pub mod settings_page;
pub mod shell;
pub mod shell_integration;
pub mod spawn;
pub mod strings;
pub mod tabs;
pub mod title;
pub mod update;
