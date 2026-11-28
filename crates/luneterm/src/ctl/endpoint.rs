//! Where a running Luneterm listens for `ctl`, and how the client finds it: one trait per thing
//! the server and the client do, one implementation per OS ([`Native`]).
//!
//! The endpoint is the security boundary of `ctl`: only processes of the user who runs the app may
//! reach it. An implementation enforces that itself (who may open it, and who is on the other end
//! of each connection) and fails closed. Nothing it reports carries a path.

use std::fmt;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::UnixEndpoint as Native;
#[cfg(windows)]
pub use windows::PipeEndpoint as Native;

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

    /// `None` where every endpoint [`Endpoint::instances`] lists is this user's: a scan waits for
    /// each until its deadline. `Some` where other users' endpoints can be among them: a scan
    /// does not wait on one for longer than this before the others have their turn.
    const SCAN_SLICE: Option<Duration> = None;

    /// `Some(n)` where [`Endpoint::instances`] lists at most `n + 1` tokens and `n + 1` means
    /// that the list was cut (others' endpoints can fill it): a scan of such a list sends no
    /// request, as one of this user's instances may be past the cut. `None`: no list is refused.
    const SCAN_CUT_AT: Option<usize> = None;

    /// Opens the endpoint of the instance `token`.
    fn bind(config_dir: &Path, token: &str) -> Result<Self::Listener, BindError>;

    /// Connects to the instance `token`, failing with [`io::ErrorKind::TimedOut`] at `deadline`.
    /// With a [`Endpoint::SCAN_SLICE`], an endpoint that is free is connected to even when
    /// `deadline` has just passed: a scan's first try waits for none.
    fn connect(config_dir: &Path, token: &str, deadline: Instant) -> io::Result<Self::Connection>;

    /// The tokens of the endpoints there are to try. Some may be left over from an instance that
    /// is gone: only [`Endpoint::connect`] tells.
    fn instances(config_dir: &Path) -> Vec<String>;
}

/// A directory of a test's own, for the tests of the endpoint and of the server.
#[cfg(test)]
pub(crate) mod scratch {
    use std::fs::DirBuilder;
    use std::io::ErrorKind;
    #[cfg(unix)]
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
                #[cfg_attr(not(unix), allow(unused_mut))]
                let mut builder = DirBuilder::new();
                #[cfg(unix)]
                builder.mode(0o700);
                match builder.create(&dir) {
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
