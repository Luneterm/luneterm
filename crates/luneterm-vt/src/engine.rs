//! Public engine contract.
//!
//! Mirrors Lunegit's `IVtEngine` (`Lunegit.Core/Terminal/IVtEngine.cs`) so Lunegit can later
//! swap its V8-hosted xterm.js for this engine through `luneterm-ffi` without changing callers.
//! Provisional: the viewport/snapshot types land with the buffer port.

/// Something the engine wants the host to act on, produced while processing [`VtEngine::write`].
///
/// Lunegit fires these as synchronous events inside `Write`; here the engine queues them and the
/// host drains them after each write, which keeps the engine free of callbacks into the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Bytes to send back to the application (DA/DSR/DECRQM/XTVERSION replies).
    Response(Vec<u8>),
    /// OSC 52 clipboard write. Queries are deliberately never answered.
    ClipboardWrite { selection: String, data: String },
    /// An OSC the engine and its extensions did not handle (ident and payload length only).
    UnknownOsc { ident: u32, len: usize },
    /// BEL.
    Bell,
    /// Window title changed (OSC 0/2).
    Title(String),
}

/// A VT engine instance: one per terminal session. Not thread-safe by design; the session owns it.
pub trait VtEngine {
    fn columns(&self) -> u16;
    fn rows(&self) -> u16;
    /// Feed raw bytes from the PTY. Partial UTF-8 and partial escape sequences may be split
    /// across calls.
    fn write(&mut self, data: &[u8]);
    fn resize(&mut self, columns: u16, rows: u16);
    fn scroll_lines(&mut self, delta: i32);
    /// Take the events produced since the last call.
    fn drain_events(&mut self) -> Vec<HostEvent>;
}
