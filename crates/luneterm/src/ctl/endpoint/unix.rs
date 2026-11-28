//! The Unix endpoint: a stream socket at `<config dir>/run/<token>.sock`.
//!
//! Who may reach it:
//!
//!  - `run` is a directory only the user can enter: a real directory (not a symlink), owned by
//!    the effective user, no group or other bit, in a config directory that no group or other may
//!    write to. The server and the client both check this, and refuse anything else;
//!  - the server asks the OS who is at the other end of each connection and drops a peer of
//!    another user without an answer;
//!  - the client counts only a socket the user owns.
//!
//! A `<token>.sock` name exists only for a socket that already listens: the socket is bound as
//! `<token>.sock.tmp` and renamed. So a refused connect on a `*.sock` means its instance is
//! gone, and the next instance to start removes it.

#![allow(unsafe_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use super::{BindError, Connection, Endpoint, Interrupt, Listener};
use crate::ctl::token_ok;
use crate::persist::create_dir;

const RUN_DIR: &str = "run";
const SOCKET_SUFFIX: &str = ".sock";
const TEMP_SUFFIX: &str = ".sock.tmp";
/// The bytes of `sockaddr_un.sun_path`, the closing NUL included.
const SUN_PATH_BYTES: usize = if cfg!(target_os = "linux") { 108 } else { 104 };
/// A temporary socket older than this is what a crash between bind and rename left.
const TEMP_AGE: Duration = Duration::from_secs(60);
/// The longest one look at a possibly dead socket takes.
const PROBE: Duration = Duration::from_millis(200);
/// The most entries of `run` a scan reads.
const SCAN_ENTRIES: usize = 256;

/// The endpoint of Linux and macOS.
#[derive(Debug)]
pub struct UnixEndpoint;

impl Endpoint for UnixEndpoint {
    type Listener = UnixSocket;
    type Connection = UnixConnection;

    fn bind(config_dir: &Path, token: &str) -> Result<UnixSocket, BindError> {
        if !token_ok(token) {
            return Err(BindError::Refused("the instance token is malformed"));
        }
        let user = euid();
        let run = prepare(config_dir, user)?;
        let temp = run.join(format!("{token}{TEMP_SUFFIX}"));
        // The temporary name is the longer one.
        if temp.as_os_str().len() >= SUN_PATH_BYTES {
            return Err(BindError::Refused("the socket path is too long"));
        }
        sweep(&run, user, SystemTime::now());
        let socket = UnixListener::bind(&temp).map_err(failed("bind the socket"))?;
        let path = run.join(format!("{token}{SOCKET_SUFFIX}"));
        let published = publish(socket, &temp, path, user);
        if published.is_err() {
            // The temporary name is ours: this call bound it.
            let _ = fs::remove_file(&temp);
        }
        published
    }

    fn connect(config_dir: &Path, token: &str, deadline: Instant) -> io::Result<UnixConnection> {
        if !token_ok(token) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let user = euid();
        let run = existing(config_dir, user)
            .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e))?;
        let path = run.join(format!("{token}{SOCKET_SUFFIX}"));
        if !is_own_socket(&path, user) {
            return Err(io::ErrorKind::NotFound.into());
        }
        UnixConnection::new(connect_by(path, deadline)?)
    }

    fn instances(config_dir: &Path) -> Vec<String> {
        let user = euid();
        let Ok(run) = existing(config_dir, user) else {
            return Vec::new();
        };
        let Ok(entries) = fs::read_dir(run) else {
            return Vec::new();
        };
        entries
            .take(SCAN_ENTRIES)
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let token = name.strip_suffix(SOCKET_SUFFIX)?;
                (token_ok(token) && is_own_socket(&entry.path(), user)).then(|| token.to_owned())
            })
            .collect()
    }
}

// --------------------------------------------------------------------------- the directories

fn failed(doing: &'static str) -> impl Fn(io::Error) -> BindError {
    move |e| BindError::Io(doing, e.kind())
}

/// The config directory may be reached through a symlink; no group or other may write to it, or
/// they could replace `run`.
fn check_config(config_dir: &Path) -> Result<(), BindError> {
    let meta = fs::metadata(config_dir).map_err(failed("read the config directory"))?;
    if !meta.is_dir() {
        return Err(BindError::Refused(
            "the config directory is not a directory",
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(BindError::Refused(
            "the config directory is writable by other users",
        ));
    }
    Ok(())
}

/// `run` itself, not what a link there points at: a directory of `user` that only `user` can
/// read, write or enter.
fn check_run(run: &Path, user: u32) -> Result<(), BindError> {
    let meta = fs::symlink_metadata(run).map_err(failed("read the control directory"))?;
    if !meta.file_type().is_dir() {
        return Err(BindError::Refused(
            "the control directory is not a directory",
        ));
    }
    if meta.uid() != user {
        return Err(BindError::Refused(
            "the control directory belongs to another user",
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(BindError::Refused(
            "the control directory is open to other users",
        ));
    }
    Ok(())
}

/// The checked `run` directory, made (mode 0700) if it is not there. What is there already is
/// checked, never repaired.
fn prepare(config_dir: &Path, user: u32) -> Result<PathBuf, BindError> {
    create_dir(config_dir).map_err(failed("create the config directory"))?;
    check_config(config_dir)?;
    let run = config_dir.join(RUN_DIR);
    if let Err(e) = create_dir(&run)
        && fs::symlink_metadata(&run).is_err()
    {
        return Err(failed("create the control directory")(e));
    }
    check_run(&run, user)?;
    Ok(run)
}

/// The checked `run` directory as it is; nothing is made.
fn existing(config_dir: &Path, user: u32) -> Result<PathBuf, BindError> {
    check_config(config_dir)?;
    let run = config_dir.join(RUN_DIR);
    check_run(&run, user)?;
    Ok(run)
}

/// Whether `path` itself (not what a link there points at) is a socket of `user`.
fn is_own_socket(path: &Path, user: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket() && m.uid() == user)
}

/// Removes what instances that are gone left in `run`, and nothing else: a socket of `user` that
/// refuses to connect, and a temporary socket of `user` older than [`TEMP_AGE`] at `now`.
fn sweep(run: &Path, user: u32, now: SystemTime) {
    let Ok(entries) = fs::read_dir(run) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let temporary = name.ends_with(TEMP_SUFFIX);
        if !temporary && !name.ends_with(SOCKET_SUFFIX) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_socket() || meta.uid() != user {
            continue;
        }
        let dead = if temporary {
            meta.modified()
                .ok()
                .and_then(|made| now.duration_since(made).ok())
                .is_some_and(|age| age > TEMP_AGE)
        } else {
            // Only a refusal says nobody listens: any other answer, silence included, leaves it.
            matches!(
                connect_by(path.clone(), Instant::now() + PROBE),
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused
            )
        };
        if dead {
            let _ = fs::remove_file(&path);
        }
    }
}

/// `UnixStream::connect`, given up on at `deadline`: on Linux it waits for as long as the
/// listener's backlog is full.
fn connect_by(path: PathBuf, deadline: Instant) -> io::Result<UnixStream> {
    let (done, result) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("ctl-connect".to_owned())
        .spawn(move || {
            let _ = done.send(UnixStream::connect(path));
        })?;
    result
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

// ------------------------------------------------------------------------------ the listener

/// The file a name stands for.
fn identity(path: &Path) -> io::Result<(u64, u64)> {
    let meta = fs::symlink_metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

/// Gives the bound `socket` its name `path`, with all it needs to be used.
fn publish(
    socket: UnixListener,
    temp: &Path,
    path: PathBuf,
    user: u32,
) -> Result<UnixSocket, BindError> {
    let setup = failed("set up the socket");
    socket.set_nonblocking(true).map_err(&setup)?;
    let (interrupted, interrupt) = UnixStream::pair().map_err(&setup)?;
    interrupt.set_nonblocking(true).map_err(&setup)?;
    let identity = identity(temp).map_err(&setup)?;
    fs::rename(temp, &path).map_err(failed("name the socket"))?;
    Ok(UnixSocket {
        socket,
        path,
        identity,
        user,
        interrupted,
        interrupt,
    })
}

/// A listening socket under its name. Dropping it removes the name, if it still stands for this
/// socket.
#[derive(Debug)]
pub struct UnixSocket {
    socket: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    /// The only user a peer may be.
    user: u32,
    /// Readable once an interrupt was asked for; it is never read, so it stays so.
    interrupted: UnixStream,
    interrupt: UnixStream,
}

impl Listener for UnixSocket {
    type Connection = UnixConnection;
    type Interrupt = UnixInterrupt;

    fn interrupter(&self) -> io::Result<UnixInterrupt> {
        self.interrupt.try_clone().map(UnixInterrupt)
    }

    fn accept(&mut self, deadline: Instant) -> io::Result<Option<UnixConnection>> {
        loop {
            let mut ready = [
                poll_for(&self.socket, libc::POLLIN),
                poll_for(&self.interrupted, libc::POLLIN),
            ];
            if !wait(&mut ready, deadline)? || ready[1].revents != 0 {
                return Ok(None);
            }
            match self.socket.accept() {
                Ok((stream, _)) => {
                    if let Some(connection) = admit(stream, self.user) {
                        return Ok(Some(connection));
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                            | io::ErrorKind::ConnectionAborted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        if identity(&self.path).is_ok_and(|now| now == self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The connection, if the OS says its peer is `user`; otherwise it is closed here, unread and
/// unanswered. A peer the OS cannot name is refused too.
fn admit(stream: UnixStream, user: u32) -> Option<UnixConnection> {
    if peer_uid(&stream).ok()? != user {
        return None;
    }
    UnixConnection::new(stream).ok()
}

/// Ends the accept of one [`UnixSocket`].
#[derive(Debug)]
pub struct UnixInterrupt(UnixStream);

impl Interrupt for UnixInterrupt {
    fn interrupt(&self) {
        // Non-blocking: a full buffer means it is interrupted already.
        let _ = (&self.0).write(&[0]);
    }
}

// ---------------------------------------------------------------------------- the connection

/// A connected socket, non-blocking, with the deadline its reads and writes keep.
#[derive(Debug)]
pub struct UnixConnection {
    stream: UnixStream,
    deadline: Instant,
}

impl UnixConnection {
    fn new(stream: UnixStream) -> io::Result<UnixConnection> {
        stream.set_nonblocking(true)?;
        Ok(UnixConnection {
            stream,
            deadline: Instant::now(),
        })
    }

    /// Runs `transfer` once the socket is ready for `events`, unless the deadline comes first.
    /// The deadline is checked before every transfer, so a peer that feeds or drains the socket
    /// a little at a time gains no time by it.
    fn by_deadline(
        &self,
        events: libc::c_short,
        mut transfer: impl FnMut(&UnixStream) -> io::Result<usize>,
    ) -> io::Result<usize> {
        loop {
            if Instant::now() >= self.deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            match transfer(&self.stream) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    wait(&mut [poll_for(&self.stream, events)], self.deadline)?;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                done => return done,
            }
        }
    }
}

impl Read for UnixConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.by_deadline(libc::POLLIN, |mut stream| stream.read(buf))
    }
}

impl Write for UnixConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.by_deadline(libc::POLLOUT, |mut stream| stream.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Connection for UnixConnection {
    fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = deadline;
    }
}

// ------------------------------------------------------------------------------- the OS calls

fn poll_for(fd: &impl AsRawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd: fd.as_raw_fd(),
        events,
        revents: 0,
    }
}

/// Waits until one of `fds` is ready (`true`, its `revents` set) or `deadline` passes
/// (`false`).
fn wait(fds: &mut [libc::pollfd], deadline: Instant) -> io::Result<bool> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        // Rounded up, so a last part of a millisecond is waited for and not spun through.
        let millis =
            libc::c_int::try_from(left.as_nanos().div_ceil(1_000_000)).unwrap_or(libc::c_int::MAX);
        // SAFETY: `fds` is an exclusively borrowed array of `fds.len()` initialised `pollfd`s,
        // which `poll` reads and writes only within that length. Each descriptor belongs to a
        // socket the caller borrows for the whole call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
        if ready > 0 {
            return Ok(true);
        }
        if ready < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

/// The effective user id.
fn euid() -> u32 {
    // SAFETY: `geteuid` takes no argument, reads no memory and cannot fail.
    unsafe { libc::geteuid() }
}

/// The effective user id of the process at the other end of `stream`, as the kernel recorded it.
#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut peer = libc::ucred {
        pid: 0,
        uid: libc::uid_t::MAX,
        gid: libc::gid_t::MAX,
    };
    let size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let mut len = size;
    // SAFETY: the descriptor is open while `stream` is borrowed. The option's value pointer is
    // an initialised `ucred` that is writable for `len` bytes, its exact size, and `len` is a
    // valid `socklen_t` the kernel writes back to.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut peer).cast(),
            &raw mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if len != size {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(peer.uid)
}

/// The effective user id of the process at the other end of `stream`, as the kernel recorded it.
#[cfg(not(target_os = "linux"))]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut uid = libc::uid_t::MAX;
    let mut gid = libc::gid_t::MAX;
    // SAFETY: the descriptor is open while `stream` is borrowed, and both pointers are to
    // initialised locals of the types `getpeereid` writes.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(test)]
mod tests {
    use std::fs::Permissions;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;
    use crate::ctl::endpoint::scratch::Scratch;

    const TOKEN: &str = "abcdefgh12345678abcdef";
    const OTHER: &str = "zyxwvuts87654321zyxwvu";

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, Permissions::from_mode(mode)).expect("chmod");
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).expect("stat").mode() & 0o7777
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read the directory")
            .map(|e| e.expect("entry").file_name().into_string().expect("a name"))
            .collect();
        names.sort();
        names
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    /// A socket file nobody listens on, as a crash leaves it.
    fn dead_socket(path: &Path) {
        drop(UnixListener::bind(path).expect("bind"));
        assert!(path.exists(), "the file stays when the listener goes");
    }

    #[test]
    fn a_bound_endpoint_is_one_socket_in_a_private_directory_and_goes_with_its_listener() {
        let scratch = Scratch::new();
        let config = scratch.path().join("c");
        let listener = UnixEndpoint::bind(&config, TOKEN).expect("binds");
        assert_eq!(mode(&config), 0o700);
        assert_eq!(mode(&config.join("run")), 0o700);
        // Only the final name: the temporary one was renamed.
        assert_eq!(names(&config.join("run")), [format!("{TOKEN}.sock")]);
        assert_eq!(UnixEndpoint::instances(&config), [TOKEN]);
        UnixEndpoint::connect(&config, TOKEN, soon()).expect("connects");
        drop(listener);
        assert!(names(&config.join("run")).is_empty());
        assert!(UnixEndpoint::instances(&config).is_empty());
        assert!(UnixEndpoint::connect(&config, TOKEN, soon()).is_err());
    }

    #[test]
    fn dropping_a_listener_leaves_a_file_that_replaced_its_socket() {
        let scratch = Scratch::new();
        let listener = UnixEndpoint::bind(scratch.path(), TOKEN).expect("binds");
        let path = scratch.path().join("run").join(format!("{TOKEN}.sock"));
        // Made while the socket still exists, so it cannot be given the socket's inode.
        let other = scratch.path().join("run").join("other");
        fs::write(&other, b"x").expect("write");
        fs::rename(&other, &path).expect("replace");
        drop(listener);
        assert_eq!(fs::read(&path).expect("still there"), b"x");
    }

    #[test]
    fn a_malformed_token_names_no_file() {
        let scratch = Scratch::new();
        let long = "a".repeat(65);
        for token in [
            "",
            "short",
            "../../escape",
            "abcdefgh/ijk",
            "abcdefgh.sock",
            &long,
        ] {
            assert!(
                matches!(
                    UnixEndpoint::bind(scratch.path(), token),
                    Err(BindError::Refused(_))
                ),
                "{token:?}"
            );
            assert!(UnixEndpoint::connect(scratch.path(), token, soon()).is_err());
        }
        assert!(names(scratch.path()).is_empty());
    }

    #[test]
    fn a_config_directory_others_can_write_to_is_refused_and_nothing_is_made_in_it() {
        for bits in [0o770, 0o707, 0o720, 0o702, 0o1777] {
            let scratch = Scratch::new();
            chmod(scratch.path(), bits);
            let refused = UnixEndpoint::bind(scratch.path(), TOKEN).expect_err("refused");
            assert_eq!(
                refused,
                BindError::Refused("the config directory is writable by other users"),
                "{bits:o}"
            );
            assert!(names(scratch.path()).is_empty());
            chmod(scratch.path(), 0o700);
        }
        // Readable by others is allowed: only writing could replace `run`.
        let scratch = Scratch::new();
        chmod(scratch.path(), 0o755);
        UnixEndpoint::bind(scratch.path(), TOKEN).expect("binds");
    }

    #[test]
    fn a_config_directory_that_is_a_file_is_refused() {
        let scratch = Scratch::new();
        let config = scratch.path().join("c");
        fs::write(&config, b"").expect("write");
        assert!(UnixEndpoint::bind(&config, TOKEN).is_err());
        assert!(UnixEndpoint::instances(&config).is_empty());
    }

    #[test]
    fn a_run_directory_with_any_group_or_other_bit_is_refused_and_left_as_it_is() {
        for bits in [0o755, 0o750, 0o705, 0o740, 0o704, 0o710, 0o701, 0o720] {
            let scratch = Scratch::new();
            let run = scratch.path().join("run");
            // A live socket in it, so the client has something it must not count.
            drop(UnixEndpoint::bind(scratch.path(), OTHER).expect("makes run"));
            let _live = UnixListener::bind(run.join(format!("{OTHER}.sock"))).expect("bind");
            chmod(&run, bits);
            assert_eq!(
                UnixEndpoint::bind(scratch.path(), TOKEN).expect_err("refused"),
                BindError::Refused("the control directory is open to other users"),
                "{bits:o}"
            );
            assert!(UnixEndpoint::instances(scratch.path()).is_empty());
            assert!(UnixEndpoint::connect(scratch.path(), OTHER, soon()).is_err());
            assert_eq!(mode(&run), bits, "not repaired");
            assert_eq!(names(&run), [format!("{OTHER}.sock")]);
            // The positive control: the same directory, closed again, is used.
            chmod(&run, 0o700);
            assert_eq!(UnixEndpoint::instances(scratch.path()), [OTHER]);
            UnixEndpoint::connect(scratch.path(), OTHER, soon()).expect("connects");
        }
    }

    #[test]
    fn a_run_directory_that_is_a_link_or_a_file_is_refused() {
        let scratch = Scratch::new();
        let (config, elsewhere) = (scratch.path().join("c"), scratch.path().join("e"));
        create_dir(&config).expect("create");
        create_dir(&elsewhere).expect("create");
        let run = config.join("run");
        let not_a_directory = BindError::Refused("the control directory is not a directory");
        // A link to a private directory of ours is still a link.
        symlink(&elsewhere, &run).expect("link");
        assert_eq!(
            UnixEndpoint::bind(&config, TOKEN).expect_err("refused"),
            not_a_directory
        );
        assert!(names(&elsewhere).is_empty(), "nothing made through it");
        fs::remove_file(&run).expect("unlink");
        // A dangling link: nothing is made at its target.
        symlink(scratch.path().join("nowhere"), &run).expect("link");
        assert_eq!(
            UnixEndpoint::bind(&config, TOKEN).expect_err("refused"),
            not_a_directory
        );
        assert!(!scratch.path().join("nowhere").exists());
        fs::remove_file(&run).expect("unlink");
        fs::write(&run, b"").expect("write");
        assert_eq!(
            UnixEndpoint::bind(&config, TOKEN).expect_err("refused"),
            not_a_directory
        );
    }

    #[test]
    fn a_run_directory_of_another_user_is_refused() {
        let scratch = Scratch::new();
        let run = scratch.path().join("run");
        create_dir(&run).expect("create");
        assert_eq!(check_run(&run, euid()), Ok(()));
        // Only root could make a directory of someone else, so the user asked about changes.
        assert_eq!(
            check_run(&run, euid().wrapping_add(1)),
            Err(BindError::Refused(
                "the control directory belongs to another user"
            ))
        );
        assert!(!is_own_socket(&run, euid()), "a directory is no socket");
    }

    #[test]
    fn a_refusal_names_no_path() {
        let scratch = Scratch::new();
        let config = scratch.path().join("secretname");
        fs::write(&config, b"").expect("write");
        let text = UnixEndpoint::bind(&config, TOKEN)
            .expect_err("refused")
            .to_string();
        assert!(!text.contains("secretname"), "{text}");
        assert!(!text.contains('/'), "{text}");
    }

    #[test]
    fn the_socket_path_may_fill_sun_path_and_no_more() {
        let scratch = Scratch::new();
        let base = scratch.path().as_os_str().as_bytes().len();
        // <base>/<pad>/run/<token>.sock.tmp
        let around = base + 1 + "/run/".len() + TOKEN.len() + TEMP_SUFFIX.len();
        let pad = SUN_PATH_BYTES - 1 - around;
        let fits = scratch.path().join("p".repeat(pad));
        let listener = UnixEndpoint::bind(&fits, TOKEN).expect("the longest path binds");
        UnixEndpoint::connect(&fits, TOKEN, soon()).expect("connects");
        drop(listener);
        let over = scratch.path().join("p".repeat(pad + 1));
        assert_eq!(
            UnixEndpoint::bind(&over, TOKEN).expect_err("refused"),
            BindError::Refused("the socket path is too long")
        );
        // The limit is the OS's own: std refuses the very path that was refused here.
        assert!(UnixListener::bind(over.join("run").join(format!("{TOKEN}.sock.tmp"))).is_err());
    }

    #[test]
    fn a_sweep_removes_only_dead_sockets_and_old_temporary_ones() {
        let scratch = Scratch::new();
        let run = scratch.path();
        dead_socket(&run.join("dead.sock"));
        let _live = UnixListener::bind(run.join("live.sock")).expect("bind");
        fs::write(run.join("file.sock"), b"").expect("write");
        dead_socket(&run.join("target"));
        symlink(run.join("target"), run.join("link.sock")).expect("link");
        dead_socket(&run.join("dead.other"));
        dead_socket(&run.join("half.sock.tmp"));
        fs::write(run.join("file.sock.tmp"), b"").expect("write");
        let made = fs::symlink_metadata(run.join("half.sock.tmp"))
            .and_then(|m| m.modified())
            .expect("mtime");
        let all = names(run);

        // Another user's sweep touches nothing.
        sweep(
            run,
            euid().wrapping_add(1),
            made + Duration::from_secs(3600),
        );
        assert_eq!(names(run), all);

        sweep(run, euid(), made + Duration::from_secs(60));
        let mut left = all.clone();
        left.retain(|n| n != "dead.sock");
        assert_eq!(
            names(run),
            left,
            "a temporary socket 60 s old may still be in use"
        );

        sweep(run, euid(), made + Duration::from_secs(61));
        left.retain(|n| n != "half.sock.tmp");
        assert_eq!(names(run), left);
    }

    #[test]
    fn binding_sweeps_the_dead_sockets() {
        let scratch = Scratch::new();
        drop(UnixEndpoint::bind(scratch.path(), TOKEN).expect("makes run"));
        let run = scratch.path().join("run");
        dead_socket(&run.join(format!("{OTHER}.sock")));
        assert_eq!(UnixEndpoint::instances(scratch.path()), [OTHER]);
        let _listener = UnixEndpoint::bind(scratch.path(), TOKEN).expect("binds");
        assert_eq!(names(&run), [format!("{TOKEN}.sock")]);
    }

    #[test]
    fn a_scan_lists_only_sockets_named_by_a_token() {
        let scratch = Scratch::new();
        drop(UnixEndpoint::bind(scratch.path(), TOKEN).expect("makes run"));
        let run = scratch.path().join("run");
        for name in [
            "short.sock",
            "has.a.dot.sock",
            "abcdefgh.sock.tmp",
            "abcdefgh.socket",
        ] {
            dead_socket(&run.join(name));
        }
        fs::write(run.join("plainfile123.sock"), b"").expect("write");
        symlink(run.join("short.sock"), run.join("alinkname123.sock")).expect("link");
        assert!(UnixEndpoint::instances(scratch.path()).is_empty());
        dead_socket(&run.join(format!("{OTHER}.sock")));
        assert_eq!(UnixEndpoint::instances(scratch.path()), [OTHER]);
    }

    #[test]
    fn a_scan_reads_at_most_256_entries() {
        let scratch = Scratch::new();
        drop(UnixEndpoint::bind(scratch.path(), TOKEN).expect("makes run"));
        let run = scratch.path().join("run");
        for n in 0..SCAN_ENTRIES + 20 {
            dead_socket(&run.join(format!("instance{n:04}.sock")));
        }
        assert_eq!(UnixEndpoint::instances(scratch.path()).len(), SCAN_ENTRIES);
    }

    #[test]
    fn the_kernel_names_the_peer() {
        let (a, b) = UnixStream::pair().expect("pair");
        assert_eq!(peer_uid(&a).expect("peer of a"), euid());
        assert_eq!(peer_uid(&b).expect("peer of b"), euid());
    }

    #[test]
    fn a_peer_of_another_user_is_closed_without_an_answer() {
        let scratch = Scratch::new();
        let mut listener = UnixEndpoint::bind(scratch.path(), TOKEN).expect("binds");
        // No second user to connect as, so the user the listener admits changes.
        listener.user = euid().wrapping_add(1);
        let mut client = UnixEndpoint::connect(scratch.path(), TOKEN, soon()).expect("connects");
        let accepted = listener
            .accept(Instant::now() + Duration::from_millis(300))
            .expect("accept");
        assert!(accepted.is_none());
        client.set_deadline(soon());
        let mut got = Vec::new();
        client.read_to_end(&mut got).expect("end of stream");
        assert!(got.is_empty());
        // The positive control: the same listener admits its own user.
        listener.user = euid();
        let _client = UnixEndpoint::connect(scratch.path(), TOKEN, soon()).expect("connects");
        assert!(listener.accept(soon()).expect("accept").is_some());
    }

    #[test]
    fn an_accept_ends_at_its_deadline_or_when_interrupted() {
        let scratch = Scratch::new();
        let mut listener = UnixEndpoint::bind(scratch.path(), TOKEN).expect("binds");
        let start = Instant::now();
        let deadline = start + Duration::from_millis(100);
        assert!(listener.accept(deadline).expect("accept").is_none());
        assert!(Instant::now() >= deadline);
        let interrupter = listener.interrupter().expect("interrupter");
        let other = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            interrupter.interrupt();
        });
        let start = Instant::now();
        let far = start + Duration::from_secs(60);
        assert!(listener.accept(far).expect("accept").is_none());
        // And every accept after it.
        assert!(listener.accept(far).expect("accept").is_none());
        assert!(start.elapsed() < Duration::from_secs(10));
        other.join().expect("join");
    }

    #[test]
    fn reads_and_writes_stop_at_the_deadline() {
        let (a, _b) = UnixStream::pair().expect("pair");
        let mut a = UnixConnection::new(a).expect("connection");
        // Nothing passes before a deadline is set.
        assert_eq!(
            a.write(b"x").expect_err("no deadline").kind(),
            io::ErrorKind::TimedOut
        );
        let deadline = Instant::now() + Duration::from_millis(100);
        a.set_deadline(deadline);
        assert_eq!(
            a.read(&mut [0; 8]).expect_err("nothing comes").kind(),
            io::ErrorKind::TimedOut
        );
        assert!(Instant::now() >= deadline);
        // The peer never reads: the buffers fill and the write gives up.
        let deadline = Instant::now() + Duration::from_millis(200);
        a.set_deadline(deadline);
        let much = vec![0; 16 << 20];
        assert_eq!(
            a.write_all(&much).expect_err("nobody reads").kind(),
            io::ErrorKind::TimedOut
        );
        assert!(Instant::now() >= deadline);
    }

    #[test]
    fn what_a_peer_sent_before_it_closed_is_still_read() {
        let (a, mut b) = UnixStream::pair().expect("pair");
        b.write_all(b"reply").expect("write");
        drop(b);
        let mut a = UnixConnection::new(a).expect("connection");
        a.set_deadline(soon());
        let mut got = Vec::new();
        a.read_to_end(&mut got).expect("read");
        assert_eq!(got, b"reply");
    }
}
