//! PTY abstraction.
//!
//! The session layer depends only on [`PtyBackend`] / [`Pty`]; the first backend will wrap the
//! `portable-pty` crate (ConPTY on Windows, openpty on Unix). Unlike Lunegit's Porta.Pty, the
//! backend must expose the master fd directly (no reflection) so the raw-mode probe can use
//! `tcgetattr` instead of hard-coded termios offsets.

use std::io;
use std::path::PathBuf;

/// Terminal size in cells (and optionally pixels, for apps that query it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub columns: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

/// What to launch inside the PTY.
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// `None` = the user's login shell.
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Added on top of the inherited environment (`TERM`, `COLORTERM`, `TERM_PROGRAM`, ...).
    pub env: Vec<(String, String)>,
}

/// A running child attached to a PTY.
pub trait Pty: Send {
    /// Blocking read of output bytes; `Ok(0)` means the child side closed.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    fn resize(&mut self, size: PtySize) -> io::Result<()>;
    /// Exit code once the child has exited, without blocking.
    fn try_exit_code(&mut self) -> io::Result<Option<i32>>;
    fn kill(&mut self) -> io::Result<()>;
}

/// Creates PTYs. One implementation per platform strategy; selected at startup.
pub trait PtyBackend {
    fn spawn(&self, options: &SpawnOptions, size: PtySize) -> io::Result<Box<dyn Pty>>;
}
