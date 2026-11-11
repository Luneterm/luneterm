//! Luneterm app library: the session core and the app shell, so the app's logic is testable and
//! M10 builds on it (plan M9 D19).

// `unsafe` only in platform modules (links/open.rs, Windows; os_event.rs, macOS), each block with
// a SAFETY comment.
#![deny(unsafe_code)]

pub mod app;
pub mod cwd;
pub mod demo;
pub mod dump;
pub mod fade;
pub mod fonts;
pub mod input;
pub mod links;
pub mod menus;
mod os_event;
pub mod outline;
pub mod pacing;
pub mod palettes;
pub mod persist;
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
