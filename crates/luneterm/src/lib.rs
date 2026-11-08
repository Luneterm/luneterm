//! Luneterm app library: the session core and the app shell, so the app's logic is testable and
//! M10 builds on it (plan M9 D19).

#![forbid(unsafe_code)]

pub mod app;
pub mod cwd;
pub mod demo;
pub mod dump;
pub mod fonts;
pub mod input;
pub mod outline;
pub mod render;
pub mod session;
pub mod settings;
pub mod shell;
pub mod spawn;
pub mod tabs;
pub mod title;
