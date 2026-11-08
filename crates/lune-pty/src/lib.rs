//! PTY abstraction.
//!
//! The session layer depends only on [`PtyBackend`] / [`Pty`]. The backend is
//! [`PortablePtyBackend`], a wrapper around the `portable-pty` crate (ConPTY on Windows, openpty
//! on Unix). Unlike Lunegit's Porta.Pty, the backend exposes the master directly (no reflection),
//! so the raw-mode probe can use `tcgetattr` instead of hard-coded termios offsets.
//!
//! Threading: [`Pty::take_reader`] hands the blocking reader to a dedicated read thread,
//! [`Pty::writer`] gives cloneable writers for any thread, and the [`Pty`] itself stays with the
//! owner for control calls (resize, exit status, kill).

#![deny(unsafe_code)]

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod portable;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub use portable::PortablePtyBackend;

/// Restricts the process-wide DLL search to the application directory, System32 and directories
/// added with `AddDllDirectory`, once and permanently (Windows; `Ok(())` elsewhere). A host calls
/// it first thing in `main`, before any library that loads a DLL by bare name creates its
/// objects (the GPU stack loads `dxcompiler.dll` that way), so a copy planted in the current
/// directory or on `PATH` is never found. The first spawn calls it too; the result of the one
/// call is kept. Fails closed: on `Err` the host must not go on to load libraries.
pub fn restrict_dll_search() -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::ensure_dll_search_restricted()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

/// The Windows build number to tell the VT engine's reflow heuristics (the OS build, as ConPTY
/// versions follow it); `None` off Windows or if the OS does not answer.
pub fn conpty_build() -> Option<u32> {
    #[cfg(windows)]
    {
        windows::os_build()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

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

/// Line discipline of the PTY as seen from the terminal side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RawMode {
    /// Canonical input processing is off (`ICANON` clear): an editor, a pager, a readline prompt.
    Raw,
    /// Canonical input processing is on (`ICANON` set): line-buffered input.
    Cooked,
    /// The mode could not be queried (always on Windows).
    #[default]
    Unknown,
}

/// Cloneable handle that writes input to the child. `Send + Sync`; every clone shares the one
/// underlying writer, so concurrent writes never interleave inside one `write_all` call.
#[derive(Clone)]
pub struct PtyWriter {
    inner: Arc<Mutex<Box<dyn io::Write + Send>>>,
}

impl PtyWriter {
    /// Wraps `writer` as the input side of a PTY. For backends outside this crate and for tests.
    pub fn new(writer: Box<dyn io::Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(writer)),
        }
    }

    /// Writes all of `data` and flushes it, under one lock.
    pub fn write_all(&self, data: &[u8]) -> io::Result<()> {
        let mut writer = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("pty writer lock poisoned"))?;
        writer.write_all(data)?;
        writer.flush()
    }
}

impl fmt::Debug for PtyWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtyWriter").finish_non_exhaustive()
    }
}

/// A running child attached to a PTY.
pub trait Pty: Send {
    /// Blocking read of output bytes; `Ok(0)` means the child side closed. Fails once the reader
    /// has been handed out by [`Pty::take_reader`].
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    /// Writes and flushes `data` through the same writer as [`Pty::writer`].
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Tells the child the new terminal size.
    fn resize(&mut self, size: PtySize) -> io::Result<()>;
    /// Exit code once the child has exited, without blocking.
    fn try_exit_code(&mut self) -> io::Result<Option<i32>>;
    /// Terminates the child. A no-op once its exit is known.
    fn kill(&mut self) -> io::Result<()>;
    /// Moves the blocking output reader out, for a dedicated read thread. `Ok(0)` from it means
    /// EOF. Only once: a second call fails, and so does [`Pty::read`] afterwards.
    fn take_reader(&mut self) -> io::Result<Box<dyn io::Read + Send>>;
    /// A writer for any thread; all writers share the one input stream.
    fn writer(&self) -> PtyWriter;
    /// OS process id of the child, if the platform reports one.
    fn process_id(&self) -> Option<u32>;
    /// Current line discipline of the PTY.
    fn raw_mode(&self) -> RawMode {
        RawMode::Unknown
    }
    /// Process id of the PTY's foreground process (group leader), if known.
    fn foreground_process(&self) -> Option<u32> {
        None
    }
}

/// Creates PTYs. One implementation per platform strategy; selected at startup.
pub trait PtyBackend {
    fn spawn(&self, options: &SpawnOptions, size: PtySize) -> io::Result<Box<dyn Pty>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dll_search_restriction_succeeds_and_repeats() {
        assert!(restrict_dll_search().is_ok());
        assert!(restrict_dll_search().is_ok());
    }

    #[test]
    fn the_conpty_build_is_a_windows_build_number_on_windows_only() {
        let build = conpty_build();
        if cfg!(windows) {
            // Windows 10 1809, the first with ConPTY, is build 17763.
            assert!(build.is_some_and(|b| b >= 17763), "{build:?}");
        } else {
            assert_eq!(build, None);
        }
    }
}
