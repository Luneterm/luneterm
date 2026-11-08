//! Luneterm app library: the session core and the app shell, so the app's logic is testable and
//! M10 builds on it (plan M9 D19).

// `unsafe` only in platform modules (links/open.rs, Windows), each block with a SAFETY comment.
#![deny(unsafe_code)]

pub mod app;
pub mod cwd;
pub mod demo;
pub mod dump;
pub mod fonts;
pub mod input;
pub mod links;
pub mod menus;
pub mod outline;
pub mod palettes;
pub mod persist;
pub mod render;
pub mod session;
pub mod settings;
pub mod settings_page;
pub mod shell;
pub mod spawn;
pub mod strings;
pub mod tabs;
pub mod title;
