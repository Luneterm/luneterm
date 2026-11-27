//! Where a running Luneterm listens for `ctl`, and how the client finds it: one trait per thing
//! the server and the client do, one implementation per OS ([`Native`]).
//!
//! The endpoint is the security boundary of `ctl`: only processes of the user who runs the app may
//! reach it. An implementation enforces that itself (who may open it, and who is on the other end
//! of each connection) and fails closed. Nothing it reports carries a path.

use std::fmt;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Instant;

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::UnixEndpoint as Native;
#[cfg(not(unix))]
pub use unsupported::NoEndpoint as Native;

/// Why an endpoint could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// Something is not as it must be: what.
    Refused(&'static str),
    /// A call failed: what was being done, and how.
    Io(&'static str, io::ErrorKind),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::Refused(why) => f.write_str(why),
            BindError::Io(doing, kind) => write!(f, "cannot {doing} ({kind})"),
        }
    }
}

impl std::error::Error for BindError {}

/// One connection. Nothing passes until a deadline is set; a read or a write at or past the
/// deadline fails with [`io::ErrorKind::TimedOut`], however much has passed before it.
pub trait Connection: Read + Write {
    fn set_deadline(&mut self, deadline: Instant);
}

/// Ends a waiting [`Listener::accept`] from another thread.
pub trait Interrupt: Send + 'static {
    /// Ends the accept that waits now and every later one.
    fn interrupt(&self);
}

/// An instance's open endpoint. Dropping it takes it down.
pub trait Listener: Send + 'static {
    type Connection: Connection;
    type Interrupt: Interrupt;

    fn interrupter(&self) -> io::Result<Self::Interrupt>;

    /// The next connection of a process of this user, waiting until `deadline` at the latest.
    /// `Ok(None)`: the deadline passed, or the listener was interrupted. A peer of another user
    /// is closed without an answer and never returned.
    fn accept(&mut self, deadline: Instant) -> io::Result<Option<Self::Connection>>;
}

/// A system's way to listen for `ctl` and to find who listens. `config_dir` is the app's config
/// directory; an implementation that does not need it ignores it.
pub trait Endpoint {
    type Listener: Listener<Connection = Self::Connection>;
    type Connection: Connection;

    /// Opens the endpoint of the instance `token`.
    fn bind(config_dir: &Path, token: &str) -> Result<Self::Listener, BindError>;

    /// Connects to the instance `token`, failing with [`io::ErrorKind::TimedOut`] at `deadline`.
    fn connect(config_dir: &Path, token: &str, deadline: Instant) -> io::Result<Self::Connection>;

    /// The tokens of the endpoints there are to try. Some may be left over from an instance that
    /// is gone: only [`Endpoint::connect`] tells.
    fn instances(config_dir: &Path) -> Vec<String>;
}

/// A directory of a test's own, for the tests of the endpoint and of the server.
#[cfg(all(test, unix))]
pub(crate) mod scratch {
    use std::fs::DirBuilder;
    use std::io::ErrorKind;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A new directory (mode 0700) with a short name, for `sun_path`; removed on drop.
    #[derive(Debug)]
    pub(crate) struct Scratch(PathBuf);

    impl Scratch {
        pub(crate) fn new() -> Scratch {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            loop {
                let n = NEXT.fetch_add(1, Ordering::Relaxed);
                let dir = std::env::temp_dir().join(format!("ltu{}-{n}", std::process::id()));
                match DirBuilder::new().mode(0o700).create(&dir) {
                    Ok(()) => return Scratch(dir),
                    // Not ours: left alone.
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                    Err(e) => panic!("cannot create a scratch directory: {e}"),
                }
            }
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // `new` created it.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(not(unix))]
mod unsupported {
    //! No endpoint on this system: nothing binds and nothing is found.

    use std::io::{self, Read, Write};
    use std::path::Path;
    use std::time::Instant;

    use super::{BindError, Connection, Endpoint, Interrupt, Listener};

    #[derive(Debug)]
    pub struct NoEndpoint;

    /// There is no value of it: no listener and no connection ever exists.
    #[derive(Debug)]
    pub enum Never {}

    impl Read for Never {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            match *self {}
        }
    }

    impl Write for Never {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            match *self {}
        }

        fn flush(&mut self) -> io::Result<()> {
            match *self {}
        }
    }

    impl Connection for Never {
        fn set_deadline(&mut self, _: Instant) {
            match *self {}
        }
    }

    impl Interrupt for Never {
        fn interrupt(&self) {
            match *self {}
        }
    }

    impl Listener for Never {
        type Connection = Never;
        type Interrupt = Never;

        fn interrupter(&self) -> io::Result<Never> {
            match *self {}
        }

        fn accept(&mut self, _: Instant) -> io::Result<Option<Never>> {
            match *self {}
        }
    }

    impl Endpoint for NoEndpoint {
        type Listener = Never;
        type Connection = Never;

        fn bind(_: &Path, _: &str) -> Result<Never, BindError> {
            Err(BindError::Refused(
                "tab control is not available on this system",
            ))
        }

        fn connect(_: &Path, _: &str, _: Instant) -> io::Result<Never> {
            Err(io::ErrorKind::Unsupported.into())
        }

        fn instances(_: &Path) -> Vec<String> {
            Vec::new()
        }
    }
}
