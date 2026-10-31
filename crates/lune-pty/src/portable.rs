//! [`PtyBackend`] on the `portable-pty` crate: openpty on Unix, ConPTY on Windows.

use std::fmt::Display;
use std::io::{self, Read};
#[cfg(windows)]
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{Child, CommandBuilder, MasterPty, native_pty_system};

use crate::{Pty, PtyBackend, PtySize, PtyWriter, SpawnOptions};

/// Largest accepted `columns` / `rows`: ConPTY's `COORD` is `i16`, and portable-pty casts with
/// `as i16`, so anything larger would wrap.
const MAX_CELLS: u16 = i16::MAX as u16;

/// Spawns children on `portable-pty`.
///
/// - `program: None` starts the login shell: `$SHELL` (from [`SpawnOptions::env`] if set there,
///   else inherited), else the passwd shell, with argv0 `-<name>` (login(1) convention). On
///   Windows it is `%ComSpec%` (else `cmd.exe`).
/// - The child's environment is the inherited one plus [`SpawnOptions::env`]; this crate adds
///   nothing. One exception from portable-pty: on Unix `SHELL` is set when it is missing.
/// - `columns` and `rows` must be in `1..=32767`; `cwd` must be an existing directory; `args`
///   need a `program`. Violations fail with `InvalidInput` / `NotFound` before any PTY exists.
///
/// Host obligations and platform notes:
///
/// - **Windows hosts must answer `ESC[6n`.** ConPTY asks for the cursor position at startup
///   (portable-pty passes `PSEUDOCONSOLE_INHERIT_CURSOR`) and later pseudoconsole calls may hang
///   until the answer arrives on the writer. The VT engine answers it; this crate does not, since
///   a second answer would be injected input.
/// - **Unix EOF waits for every holder of the tty.** The reader returns `Ok(0)` only once all
///   slave descriptors are closed, so a background job that still holds the tty delays EOF past
///   the shell's exit.
/// - **Windows exit code 259** (`STILL_ACTIVE`) is indistinguishable from "running":
///   [`Pty::try_exit_code`] keeps returning `None` for a child that exits with 259.
/// - On Windows an exit-watcher thread closes the pseudoconsole when the child exits, which is
///   what ends the reader with `Ok(0)` (ConPTY never closes its output on its own).
/// - Dropping a [`Pty`] never blocks: an unfinished child is killed (Unix: `SIGHUP`, then
///   `SIGKILL` after up to 200 ms; Windows: `TerminateProcess`) and reaped on a detached thread,
///   which also closes the PTY.
#[derive(Debug, Default, Clone, Copy)]
pub struct PortablePtyBackend;

impl PortablePtyBackend {
    pub fn new() -> Self {
        Self
    }
}

impl PtyBackend for PortablePtyBackend {
    fn spawn(&self, options: &SpawnOptions, size: PtySize) -> io::Result<Box<dyn Pty>> {
        validate_size(size)?;
        if options.program.is_none() && !options.args.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "args given without a program",
            ));
        }
        if let Some(cwd) = &options.cwd {
            if !cwd.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("cwd is not an existing directory: {}", cwd.display()),
                ));
            }
        }

        let pair = native_pty_system()
            .openpty(to_portable(size))
            .map_err(other)?;
        // Taken before the spawn, so a failure here leaves no child behind.
        let reader = pair.master.try_clone_reader().map_err(other)?;
        let writer = PtyWriter::new(pair.master.take_writer().map_err(other)?);

        let mut cmd = match &options.program {
            None => CommandBuilder::new_default_prog(),
            Some(program) => {
                let mut cmd = CommandBuilder::new(program);
                cmd.args(&options.args);
                cmd
            }
        };
        // portable-pty's base environment also merges the registry environment on Windows (and
        // that replaces an inherited PATH); start again from exactly the inherited one.
        cmd.env_clear();
        for (key, value) in std::env::vars_os() {
            cmd.env(key, value);
        }
        for (key, value) in &options.env {
            cmd.env(key, value);
        }
        if let Some(cwd) = &options.cwd {
            cmd.cwd(cwd);
        }

        let child = pair.slave.spawn_command(cmd).map_err(other)?;
        // Unix EOF needs every slave descriptor closed; on Windows the slave shares the
        // pseudoconsole with the master and would keep it open.
        drop(pair.slave);

        let pid = child.process_id();
        #[cfg(windows)]
        let master = Arc::new(Mutex::new(Some(pair.master)));
        #[cfg(windows)]
        let watched = Arc::clone(&master);
        #[cfg(not(windows))]
        let master = Some(pair.master);

        let pty = PortablePty {
            reader: Some(PtyReader(reader)),
            writer,
            child: Some(child),
            master,
            pid,
            exit: None,
        };

        #[cfg(windows)]
        {
            let Some(child) = pty.child.as_deref() else {
                return Err(io::Error::other("child missing after spawn"));
            };
            // On failure `pty` drops here, which kills and reaps the child off this thread.
            crate::windows::watch_exit(child, move || close_master(&watched))?;
        }

        Ok(Box::new(pty))
    }
}

fn validate_size(size: PtySize) -> io::Result<()> {
    let ok = |n: u16| (1..=MAX_CELLS).contains(&n);
    if ok(size.columns) && ok(size.rows) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "pty size {}x{} outside 1..={MAX_CELLS}",
                size.columns, size.rows
            ),
        ))
    }
}

fn to_portable(size: PtySize) -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: size.rows,
        cols: size.columns,
        pixel_width: size.pixel_width,
        pixel_height: size.pixel_height,
    }
}

fn other(e: impl Display) -> io::Error {
    io::Error::other(format!("{e:#}"))
}

/// Takes the master out under the lock and drops it after the lock is released, so
/// `ClosePseudoConsole` (which can wait for the output to drain) never holds up `resize`.
#[cfg(windows)]
fn close_master(master: &Mutex<Option<Box<dyn MasterPty + Send>>>) {
    let taken = match master.lock() {
        Ok(mut guard) => guard.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    drop(taken);
}

/// The output stream, with EOF conditions folded into `Ok(0)`.
struct PtyReader(Box<dyn Read + Send>);

impl Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.0.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // ConPTY's output pipe breaks when the pseudoconsole closes.
                Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::BrokenPipe => return Ok(0),
                // Unix EIO (slave closed) is already Ok(0) in portable-pty.
                result => return result,
            }
        }
    }
}

#[cfg(windows)]
type Master = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;
#[cfg(not(windows))]
type Master = Option<Box<dyn MasterPty + Send>>;

/// One spawned child. `child` and `master` are `Option` only so `Drop` can move them to the
/// reap thread; they are `Some` for the whole life of the value otherwise.
struct PortablePty {
    reader: Option<PtyReader>,
    writer: PtyWriter,
    child: Option<Box<dyn Child + Send + Sync>>,
    master: Master,
    pid: Option<u32>,
    exit: Option<i32>,
}

impl PortablePty {
    fn child_mut(&mut self) -> io::Result<&mut (dyn Child + Send + Sync)> {
        self.child
            .as_deref_mut()
            .ok_or_else(|| io::Error::other("pty child already released"))
    }
}

impl Pty for PortablePty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.reader {
            Some(reader) => reader.read(buf),
            None => Err(io::Error::other("pty reader was taken")),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.writer.write_all(data)
    }

    fn resize(&mut self, size: PtySize) -> io::Result<()> {
        validate_size(size)?;
        #[cfg(windows)]
        {
            if self.try_exit_code()?.is_some() {
                return Ok(());
            }
            let guard = self
                .master
                .lock()
                .map_err(|_| io::Error::other("pty master lock poisoned"))?;
            match guard.as_ref() {
                Some(master) => master.resize(to_portable(size)).map_err(other),
                // The watcher already closed the pseudoconsole: the child has exited.
                None => Ok(()),
            }
        }
        #[cfg(not(windows))]
        {
            self.master
                .as_ref()
                .ok_or_else(|| io::Error::other("pty master already released"))?
                .resize(to_portable(size))
                .map_err(other)
        }
    }

    fn try_exit_code(&mut self) -> io::Result<Option<i32>> {
        if let Some(code) = self.exit {
            return Ok(Some(code));
        }
        let child = self.child_mut()?;
        #[cfg(unix)]
        let code = crate::unix::try_exit_code(child)?;
        #[cfg(not(unix))]
        // Windows exit codes are u32; bit-cast so NTSTATUS-style codes stay recognisable.
        let code = child.try_wait()?.map(|status| status.exit_code() as i32);
        self.exit = code;
        Ok(code)
    }

    fn kill(&mut self) -> io::Result<()> {
        // Never signal a pid whose exit is known: it may have been reaped and reused.
        if self.try_exit_code()?.is_some() {
            return Ok(());
        }
        // Unix: SIGHUP, then SIGKILL after up to 200 ms. Windows: TerminateProcess(1); this
        // `kill` always reports Ok (portable-pty's own killer inverts the result).
        self.child_mut()?.kill()
    }

    fn take_reader(&mut self) -> io::Result<Box<dyn Read + Send>> {
        match self.reader.take() {
            Some(reader) => Ok(Box::new(reader)),
            None => Err(io::Error::other("pty reader was already taken")),
        }
    }

    fn writer(&self) -> PtyWriter {
        self.writer.clone()
    }

    fn process_id(&self) -> Option<u32> {
        self.pid
    }
}

impl Drop for PortablePty {
    fn drop(&mut self) {
        let exited = matches!(self.try_exit_code(), Ok(Some(_)));
        let mut child = self.child.take();
        #[cfg(windows)]
        let master = Arc::clone(&self.master);
        #[cfg(not(windows))]
        let master = self.master.take();
        let reader = self.reader.take();
        // The Unix writer writes `\n` + VEOF when dropped, which can block on a full input
        // queue; move this clone to the reap thread too.
        let writer = std::mem::replace(&mut self.writer, PtyWriter::new(Box::new(io::sink())));

        let reap = move || {
            if !exited {
                if let Some(child) = child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            drop(reader);
            drop(writer);
            #[cfg(windows)]
            close_master(&master);
            #[cfg(not(windows))]
            drop(master);
            drop(child);
        };
        // If the thread cannot be created the closure (and with it the child handle) is dropped
        // here without a kill; the master's close still hangs up the session on Unix.
        let _ = thread::Builder::new()
            .name("lune-pty-reap".into())
            .spawn(reap);
    }
}
