//! The Windows endpoint: a named pipe `\\.\pipe\luneterm-ctl-<token>`.
//!
//! Who may reach it:
//!
//!  - the pipe's security descriptor has the user's own SID (the process token's `TokenUser`) as
//!    owner and a protected DACL with one allow ACE, for that SID. Not the token's default owner,
//!    which is `BUILTIN\Administrators` for an elevated administrator;
//!  - remote clients are refused (`PIPE_REJECT_REMOTE_CLIENTS`);
//!  - the server asks which process is at the other end of each connection and drops one whose
//!    token user is another, without an answer;
//!  - the client opens the pipe at identification level only (so a server cannot act as it) and
//!    refuses a server whose token user is not its own.
//!
//! One pipe instance, made with `FILE_FLAG_FIRST_PIPE_INSTANCE` (a name that is taken refuses
//! the bind) and reused for every connection: it is disconnected after each, never closed, so
//! the name stays the instance's for as long as it runs. All I/O is overlapped and waited for
//! with a deadline; a wait that runs out cancels the I/O and waits for the cancel to finish
//! before the buffer and the `OVERLAPPED` go.
//!
//! The config directory is not used: pipe names are machine-wide.

#![allow(unsafe_code)]

use std::ffi::{OsStr, c_void};
use std::io::{self, Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, FALSE, GENERIC_READ, GENERIC_WRITE,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree, TRUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FindClose, FindFirstFileW,
    FindNextFileW, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WIN32_FIND_DATAW, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, ResetEvent, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};

use super::{BindError, Connection, Endpoint, Interrupt, Listener};
use crate::ctl::token_ok;

/// What every instance's pipe name starts with, after `\\.\pipe\`.
const PREFIX: &str = "luneterm-ctl-";
/// The size of each of the pipe's two buffers. A request is at most 128 KiB; a write larger
/// than this waits for the reader, within its deadline.
const BUFFER: u32 = 64 * 1024;
/// The most instances a scan lists. The pipe namespace is the machine's, so other users' pipes of
/// these names count too: this bounds the list, the run's budget bounds the work.
const SCAN_ENTRIES: usize = 4096;
/// A scan's longest wait for one busy pipe before the others have their turn.
const SCAN_SLICE: Duration = Duration::from_millis(250);

/// The endpoint of Windows.
#[derive(Debug)]
pub struct PipeEndpoint;

fn pipe_name(token: &str) -> Vec<u16> {
    wide(&format!(r"\\.\pipe\{PREFIX}{token}"))
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain([0]).collect()
}

fn last_error() -> u32 {
    // SAFETY: no arguments; reads the calling thread's last-error value.
    unsafe { GetLastError() }
}

fn os_error(code: u32) -> io::Error {
    // Win32 error codes are small positive numbers; one that is not stays a plain error.
    i32::try_from(code).map_or_else(
        |_| io::Error::other("unknown error"),
        io::Error::from_raw_os_error,
    )
}

impl Endpoint for PipeEndpoint {
    type Listener = PipeListener;
    type Connection = PipeConnection;

    const SCAN_SLICE: Option<Duration> = Some(SCAN_SLICE);

    fn bind(_config_dir: &Path, token: &str) -> Result<PipeListener, BindError> {
        if !token_ok(token) {
            return Err(BindError::Refused("the instance token is malformed"));
        }
        let failed = |doing| move |e: io::Error| BindError::Io(doing, e.kind());
        let user = own_user().map_err(failed("find the user"))?;
        let descriptor = Descriptor::owner_only(&user).map_err(failed("describe the pipe"))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: FALSE,
        };
        let name = pipe_name(token);
        // SAFETY: `name` is a NUL-terminated UTF-16 string and `attributes` a live
        // `SECURITY_ATTRIBUTES` whose descriptor `descriptor` keeps alive for the call; the
        // flags and sizes are plain values.
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                BUFFER,
                BUFFER,
                0,
                &raw const attributes,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            return Err(match last_error() {
                ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY => {
                    BindError::Refused("the control pipe's name is taken")
                }
                code => failed("create the control pipe")(os_error(code)),
            });
        }
        // SAFETY: `pipe` is a valid handle that nothing else owns: it was just created.
        let pipe = unsafe { OwnedHandle::from_raw_handle(pipe) };
        let setup = failed("set up the control pipe");
        Ok(PipeListener {
            pipe: Arc::new(pipe),
            event: event().map_err(&setup)?,
            interrupt: Arc::new(event().map_err(&setup)?),
            user,
        })
    }

    fn connect(_config_dir: &Path, token: &str, deadline: Instant) -> io::Result<PipeConnection> {
        if !token_ok(token) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let user = own_user()?;
        let name = pipe_name(token);
        let pipe = loop {
            // SAFETY: `name` is a NUL-terminated UTF-16 string; no security attributes and no
            // template. `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION` lets the server learn
            // who the client is but never act as it.
            let pipe = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    ptr::null_mut(),
                )
            };
            if pipe != INVALID_HANDLE_VALUE {
                // SAFETY: a valid handle that nothing else owns: it was just opened.
                break unsafe { OwnedHandle::from_raw_handle(pipe) };
            }
            match last_error() {
                // Another client has the one instance (`PIPE_BUSY`), or the server has just
                // disconnected the last one and not yet listens again (`PIPE_NOT_CONNECTED`):
                // wait for it to be free, within the deadline (looked at only after a try, so a
                // try with no time to wait is still made). A wait that fails (time out, the pipe
                // went) is tried again above.
                ERROR_PIPE_BUSY | ERROR_PIPE_NOT_CONNECTED => {
                    let left = millis_left(deadline);
                    if left == 0 {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    // SAFETY: `name` is a NUL-terminated UTF-16 string; `left` is at least 1,
                    // so it is never the "default time-out" 0 nor `INFINITE`.
                    let free = unsafe { WaitNamedPipeW(name.as_ptr(), left) } != 0;
                    if !free {
                        // Not spun on, should the wait return at once.
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                code => return Err(os_error(code)),
            }
        };
        let mut server = 0;
        // SAFETY: `pipe` is an open client end of a pipe; `server` is a writable u32.
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &raw mut server) } == 0 {
            return Err(os_error(last_error()));
        }
        if process_user(server)? != user {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        PipeConnection::new(Arc::new(pipe), Side::Client)
    }

    fn instances(_config_dir: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let pattern = wide(r"\\.\pipe\*");
        let mut data = WIN32_FIND_DATAW::default();
        // SAFETY: `pattern` is a NUL-terminated UTF-16 string and `data` a writable
        // `WIN32_FIND_DATAW`.
        let search = unsafe { FindFirstFileW(pattern.as_ptr(), &raw mut data) };
        if search == INVALID_HANDLE_VALUE {
            return found;
        }
        let search = Search(search);
        loop {
            let len = data
                .cFileName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(data.cFileName.len());
            if let Ok(name) = std::ffi::OsString::from_wide(&data.cFileName[..len]).into_string()
                && let Some(token) = name.strip_prefix(PREFIX)
                && token_ok(token)
            {
                found.push(token.to_owned());
                if found.len() == SCAN_ENTRIES {
                    break;
                }
            }
            // SAFETY: `search.0` is an open search handle and `data` a writable
            // `WIN32_FIND_DATAW`.
            if unsafe { FindNextFileW(search.0, &raw mut data) } == 0 {
                break;
            }
        }
        found
    }
}

/// An open `FindFirstFileW` search, closed on drop.
struct Search(HANDLE);

impl Drop for Search {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a search handle from `FindFirstFileW`, closed only here.
        unsafe { FindClose(self.0) };
    }
}

// ------------------------------------------------------------------------------- who is who

/// A SID, as its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sid(Vec<u8>);

/// The user of the token `token` (opened with `TOKEN_QUERY`).
fn token_user(token: &OwnedHandle) -> io::Result<Sid> {
    let mut needed = 0;
    // SAFETY: a size query: no buffer, `needed` is a writable u32.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    if needed == 0 {
        return Err(os_error(last_error()));
    }
    // u64s, so the `TOKEN_USER` at its start is aligned.
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buffer` is writable for `needed` bytes (at least) and 8-byte aligned.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            needed,
            &raw mut needed,
        )
    };
    if ok == 0 {
        return Err(os_error(last_error()));
    }
    // SAFETY: the call succeeded, so the buffer starts with a `TOKEN_USER` whose SID points
    // into the buffer, which lives until the end of this function. `GetLengthSid` reads only a
    // valid SID, and its length is the SID's size in bytes.
    let bytes = unsafe {
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let sid = user.User.Sid;
        std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize).to_vec()
    };
    Ok(Sid(bytes))
}

/// The user this process runs as.
fn own_user() -> io::Result<Sid> {
    let mut token = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` is the always-valid pseudo handle; `token` is writable.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(os_error(last_error()));
    }
    // SAFETY: a valid token handle that nothing else owns: it was just opened.
    token_user(&unsafe { OwnedHandle::from_raw_handle(token) })
}

/// The user the process `pid` runs as.
fn process_user(pid: u32) -> io::Result<Sid> {
    // SAFETY: plain values; the result is checked.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
    if process.is_null() {
        return Err(os_error(last_error()));
    }
    // SAFETY: a valid process handle that nothing else owns: it was just opened.
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let mut token = ptr::null_mut();
    // SAFETY: `process` is open with a right that allows opening its token; `token` is
    // writable.
    if unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(os_error(last_error()));
    }
    // SAFETY: a valid token handle that nothing else owns: it was just opened.
    token_user(&unsafe { OwnedHandle::from_raw_handle(token) })
}

/// A security descriptor from `LocalAlloc`, freed on drop.
#[derive(Debug)]
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    /// Owner `user`, and a protected DACL with one ACE that allows `user` everything.
    fn owner_only(user: &Sid) -> io::Result<Descriptor> {
        let mut text = ptr::null_mut();
        // SAFETY: `user` holds a valid SID (from `token_user`), which is only read; `text` is
        // writable and gets a `LocalAlloc`ed string, freed below.
        if unsafe { ConvertSidToStringSidW(user.0.as_ptr().cast_mut().cast(), &raw mut text) } == 0
        {
            return Err(os_error(last_error()));
        }
        // SAFETY: `text` is the NUL-terminated string the call made; read up to the NUL, then
        // freed once.
        let sid = unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            sid
        };
        let sddl = wide(&format!("O:{sid}D:P(A;;GA;;;{sid})"));
        let mut descriptor = ptr::null_mut();
        // SAFETY: `sddl` is a NUL-terminated UTF-16 string; `descriptor` is writable and gets a
        // `LocalAlloc`ed descriptor that `Descriptor` frees; the size is not asked for.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(os_error(last_error()));
        }
        Ok(Descriptor(descriptor))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `LocalAlloc` (through the conversion) and is freed only
        // here.
        unsafe { LocalFree(self.0) };
    }
}

// ----------------------------------------------------------------------------- overlapped I/O

/// A manual-reset event, not set.
fn event() -> io::Result<OwnedHandle> {
    // SAFETY: no attributes and no name; plain values.
    let event = unsafe { CreateEventW(ptr::null(), TRUE, FALSE, ptr::null()) };
    if event.is_null() {
        return Err(os_error(last_error()));
    }
    // SAFETY: a valid event handle that nothing else owns: it was just created.
    Ok(unsafe { OwnedHandle::from_raw_handle(event) })
}

/// Whether the event `event` is set, now.
fn is_set(event: &OwnedHandle) -> bool {
    // SAFETY: `event` is an open event handle; a zero wait only looks.
    unsafe { WaitForSingleObject(event.as_raw_handle(), 0) == WAIT_OBJECT_0 }
}

/// Milliseconds to `deadline`, rounded up (so a last part of a millisecond is waited for and not
/// spun through), below `INFINITE`; 0 once it has passed.
fn millis_left(deadline: Instant) -> u32 {
    let left = deadline.saturating_duration_since(Instant::now());
    u32::try_from(left.as_nanos().div_ceil(1_000_000))
        .map_or(INFINITE - 1, |ms| ms.min(INFINITE - 1))
}

/// How an overlapped operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// It finished: the bytes it moved, or its error code.
    Finished(Result<u32, u32>),
    /// The deadline or the interrupt came first and it was cancelled. `anyway`: it had finished
    /// before the cancel reached it.
    Cancelled { anyway: bool },
}

/// Starts an overlapped operation on `handle` with `start` (given the `OVERLAPPED` to use) and
/// waits for it until `deadline`, or until `interrupt` is set. It never returns while the
/// operation may still use its buffer or the `OVERLAPPED`.
///
/// # Safety
///
/// `start` must start at most one overlapped operation on `handle` with the `OVERLAPPED` it is
/// given and return its result, and anything that operation uses besides the `OVERLAPPED` must
/// outlive this call.
unsafe fn overlapped(
    handle: HANDLE,
    event: &OwnedHandle,
    deadline: Instant,
    interrupt: Option<&OwnedHandle>,
    start: impl FnOnce(*mut OVERLAPPED) -> i32,
) -> Ended {
    // SAFETY: `event` is an open event handle.
    unsafe { ResetEvent(event.as_raw_handle()) };
    let mut ov = OVERLAPPED {
        hEvent: event.as_raw_handle(),
        ..OVERLAPPED::default()
    };
    let started = start(&raw mut ov);
    if started == 0 {
        let code = last_error();
        if code != ERROR_IO_PENDING {
            // It did not start: nothing uses `ov`.
            return Ended::Finished(Err(code));
        }
    }
    let mut waits = vec![event.as_raw_handle()];
    if let Some(interrupt) = interrupt {
        waits.push(interrupt.as_raw_handle());
    }
    let finished = loop {
        // SAFETY: `waits` holds `waits.len()` (1 or 2) open handles.
        let woke = unsafe {
            WaitForMultipleObjects(
                waits.len() as u32,
                waits.as_ptr(),
                FALSE,
                millis_left(deadline),
            )
        };
        match woke {
            WAIT_OBJECT_0 => break true,
            // Early wakes: waited again until the deadline is truly past.
            WAIT_TIMEOUT if millis_left(deadline) > 0 => {}
            // Timed out, interrupted, or the wait failed: cancelled all the same.
            _ => break false,
        }
    };
    if !finished {
        // SAFETY: `handle` is open and `ov` is the operation's own `OVERLAPPED`. A cancel of an
        // operation that has finished does nothing.
        unsafe { CancelIoEx(handle, &raw const ov) };
    }
    let mut moved = 0;
    // SAFETY: `ov` belongs to the operation started above; waiting (`TRUE`) returns only once
    // it is over, cancelled or not, so nothing uses `ov` or the buffer afterwards.
    let ok = unsafe { GetOverlappedResult(handle, &raw const ov, &raw mut moved, TRUE) };
    let result = if ok == 0 {
        Err(last_error())
    } else {
        Ok(moved)
    };
    if finished {
        Ended::Finished(result)
    } else {
        Ended::Cancelled {
            anyway: result.is_ok(),
        }
    }
}

// ------------------------------------------------------------------------------ the listener

/// The instance's pipe. Dropping it closes the pipe, and its name goes with it.
#[derive(Debug)]
pub struct PipeListener {
    pipe: Arc<OwnedHandle>,
    /// For the overlapped connect.
    event: OwnedHandle,
    /// Set once an interrupt was asked for, and never reset.
    interrupt: Arc<OwnedHandle>,
    /// The only user a peer may be.
    user: Sid,
}

impl PipeListener {
    fn disconnect(&self) {
        // SAFETY: `self.pipe` is the open server end of the pipe.
        unsafe { DisconnectNamedPipe(self.pipe.as_raw_handle()) };
    }

    /// Whether the client now connected runs as `self.user`. One the OS cannot name is not.
    fn peer_is_user(&self) -> bool {
        let mut client = 0;
        // SAFETY: `self.pipe` is the open server end of a connected pipe; `client` is a
        // writable u32.
        let named =
            unsafe { GetNamedPipeClientProcessId(self.pipe.as_raw_handle(), &raw mut client) } != 0;
        named && process_user(client).is_ok_and(|user| user == self.user)
    }
}

impl Listener for PipeListener {
    type Connection = PipeConnection;
    type Interrupt = PipeInterrupt;

    fn interrupter(&self) -> io::Result<PipeInterrupt> {
        Ok(PipeInterrupt(Arc::clone(&self.interrupt)))
    }

    fn accept(&mut self, deadline: Instant) -> io::Result<Option<PipeConnection>> {
        loop {
            // Looked at first: a client already waiting connects without any wait.
            if is_set(&self.interrupt) {
                return Ok(None);
            }
            // A connection that is still open holds the one instance: there is nothing to
            // accept until it is dropped.
            if Arc::strong_count(&self.pipe) > 1 {
                return Err(io::Error::other("the last connection is still open"));
            }
            let pipe = self.pipe.as_raw_handle();
            // SAFETY: `start` starts one `ConnectNamedPipe` on the pipe with the `OVERLAPPED`
            // it is given, and uses nothing else.
            let ended = unsafe {
                overlapped(pipe, &self.event, deadline, Some(&self.interrupt), |ov| {
                    ConnectNamedPipe(pipe, ov)
                })
            };
            match ended {
                // A client came before the connect started (`PIPE_CONNECTED`), or came, wrote
                // and closed its end (`NO_DATA`): connected all the same, and what it wrote is
                // still there to read.
                Ended::Finished(Ok(_) | Err(ERROR_PIPE_CONNECTED | ERROR_NO_DATA)) => {}
                Ended::Finished(Err(code)) => return Err(os_error(code)),
                Ended::Cancelled { anyway } => {
                    if anyway {
                        self.disconnect();
                    }
                    return Ok(None);
                }
            }
            if self.peer_is_user() {
                let side = Side::Server(Arc::clone(&self.interrupt));
                return PipeConnection::new(Arc::clone(&self.pipe), side).map(Some);
            }
            // Unread and unanswered.
            self.disconnect();
        }
    }
}

/// Ends the accept of one [`PipeListener`].
#[derive(Debug)]
pub struct PipeInterrupt(Arc<OwnedHandle>);

impl Interrupt for PipeInterrupt {
    fn interrupt(&self) {
        // SAFETY: `self.0` is an open event handle.
        unsafe { SetEvent(self.0.as_raw_handle()) };
    }
}

// ---------------------------------------------------------------------------- the connection

/// Which end of the pipe a connection is.
#[derive(Debug)]
enum Side {
    /// The listener's instance: disconnected, not closed, when the connection goes. Its reads
    /// and writes end at the listener's interrupt too, so a stopping server does not wait for
    /// the client.
    Server(Arc<OwnedHandle>),
    /// A client's own handle: closed when the connection goes.
    Client,
}

/// A connected pipe, with the deadline its reads and writes keep.
#[derive(Debug)]
pub struct PipeConnection {
    pipe: Arc<OwnedHandle>,
    side: Side,
    /// For the overlapped reads and writes.
    event: OwnedHandle,
    deadline: Instant,
}

impl PipeConnection {
    fn new(pipe: Arc<OwnedHandle>, side: Side) -> io::Result<PipeConnection> {
        Ok(PipeConnection {
            pipe,
            side,
            event: event()?,
            deadline: Instant::now(),
        })
    }

    /// One overlapped transfer by the deadline. The deadline is checked before every transfer,
    /// so a peer that feeds or drains the pipe a little at a time gains no time by it.
    ///
    /// # Safety
    ///
    /// As [`overlapped`]: `start` starts one read or write on the pipe with the `OVERLAPPED` it
    /// is given, into or from a buffer that outlives the call.
    unsafe fn transfer(
        &self,
        start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> i32,
    ) -> io::Result<u32> {
        if millis_left(self.deadline) == 0 {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let pipe = self.pipe.as_raw_handle();
        let interrupt = match &self.side {
            Side::Server(interrupt) => Some(&**interrupt),
            Side::Client => None,
        };
        // SAFETY: the caller's promise.
        match unsafe {
            overlapped(pipe, &self.event, self.deadline, interrupt, |ov| {
                start(pipe, ov)
            })
        } {
            Ended::Finished(Ok(moved)) => Ok(moved),
            Ended::Finished(Err(code)) => Err(os_error(code)),
            Ended::Cancelled { .. } => Err(io::ErrorKind::TimedOut.into()),
        }
    }
}

impl Read for PipeConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let into = buf.as_mut_ptr();
        // SAFETY: one `ReadFile` of at most `len` bytes into `buf`, which is borrowed for the
        // whole call.
        let read =
            unsafe { self.transfer(|pipe, ov| ReadFile(pipe, into, len, ptr::null_mut(), ov)) };
        match read {
            Ok(n) => Ok(n as usize),
            // The other end closed: the end of what it sent.
            Err(e)
                if [ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED, ERROR_NO_DATA]
                    .map(|code| i32::try_from(code).ok())
                    .contains(&e.raw_os_error()) =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }
}

impl Write for PipeConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let from = buf.as_ptr();
        // SAFETY: one `WriteFile` of at most `len` bytes from `buf`, which is borrowed for the
        // whole call.
        let written =
            unsafe { self.transfer(|pipe, ov| WriteFile(pipe, from, len, ptr::null_mut(), ov)) }?;
        Ok(written as usize)
    }

    /// On the server's end: waits, until the deadline at the latest, for the client to close
    /// its end, which it does once it has read the reply. Disconnecting before that would throw
    /// away what it has not read yet. (`FlushFileBuffers` would wait for the reading too, but
    /// with no deadline.) Nothing on the client's end.
    fn flush(&mut self) -> io::Result<()> {
        if matches!(self.side, Side::Server(_)) {
            let mut rest = [0; 64];
            while self.read(&mut rest)? != 0 {}
        }
        Ok(())
    }
}

impl Connection for PipeConnection {
    fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = deadline;
    }
}

impl Drop for PipeConnection {
    fn drop(&mut self) {
        if matches!(self.side, Side::Server(_)) {
            // Frees the instance for the next client. No I/O is pending: every transfer is over
            // before it returns.
            // SAFETY: `self.pipe` is the open server end of the pipe.
            unsafe { DisconnectNamedPipe(self.pipe.as_raw_handle()) };
        }
    }
}

// ------------------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
        OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED,
    };

    use super::*;
    use crate::ctl::token;

    /// `ACCESS_ALLOWED_ACE_TYPE`, which windows-sys keeps in a module of its own.
    const ACCESS_ALLOWED: u8 = 0;

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    fn after(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    fn bound() -> (String, PipeListener) {
        let token = token::new().expect("a token");
        let listener = PipeEndpoint::bind(Path::new("unused"), &token).expect("binds");
        (token, listener)
    }

    fn connect(token: &str) -> PipeConnection {
        PipeEndpoint::connect(Path::new("unused"), token, soon()).expect("connects")
    }

    /// The SID at `sid`, as bytes.
    ///
    /// # Safety
    ///
    /// `sid` points at a valid SID.
    unsafe fn sid_at(sid: PSID) -> Sid {
        // SAFETY: the caller's promise.
        Sid(
            unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize) }
                .to_vec(),
        )
    }

    #[test]
    fn the_pipe_is_the_users_with_one_allow_ace_in_a_protected_dacl() {
        let (_, listener) = bound();
        let user = own_user().expect("the user");
        let mut owner: PSID = ptr::null_mut();
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: the listener's pipe is open; every out pointer is a writable local, and the
        // descriptor the call allocates is freed below, after the last use of `owner` and
        // `dacl`, which point into it.
        let found = unsafe {
            GetSecurityInfo(
                listener.pipe.as_raw_handle(),
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &raw mut owner,
                ptr::null_mut(),
                &raw mut dacl,
                ptr::null_mut(),
                &raw mut descriptor,
            )
        };
        assert_eq!(found, 0, "GetSecurityInfo");
        let _free = Descriptor(descriptor);
        let mut control = 0_u16;
        let mut revision = 0_u32;
        // SAFETY: `descriptor` is the valid descriptor just read; both outs are writable.
        let ok = unsafe {
            GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision)
        };
        assert_ne!(ok, 0);
        assert_ne!(control & SE_DACL_PROTECTED, 0, "the DACL is protected");
        // SAFETY: `owner` points at the descriptor's owner SID.
        assert_eq!(unsafe { sid_at(owner) }, user, "the owner is the user");
        assert!(!dacl.is_null(), "there is a DACL");
        // SAFETY: `dacl` points at the descriptor's ACL.
        assert_eq!(unsafe { (*dacl).AceCount }, 1);
        let mut ace = ptr::null_mut();
        // SAFETY: `dacl` is a valid ACL with one ACE; `ace` is writable.
        assert_ne!(unsafe { GetAce(dacl, 0, &raw mut ace) }, 0);
        // SAFETY: `ace` points at the ACL's ACE, which starts with its header; an allow ACE is
        // an `ACCESS_ALLOWED_ACE` whose SID starts at `SidStart`.
        unsafe {
            let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!(ace.Header.AceType, ACCESS_ALLOWED);
            assert_eq!(sid_at((&raw const ace.SidStart).cast_mut().cast()), user);
        }
    }

    #[test]
    fn a_name_that_is_taken_refuses_the_bind() {
        let (token, listener) = bound();
        assert_eq!(
            PipeEndpoint::bind(Path::new("unused"), &token).err(),
            Some(BindError::Refused("the control pipe's name is taken"))
        );
        drop(listener);
        PipeEndpoint::bind(Path::new("unused"), &token).expect("free again");
    }

    #[test]
    fn a_malformed_token_names_no_pipe() {
        for bad in ["short", "abcdefgh\\x", r"..\..\pipe\x1234567"] {
            assert!(matches!(
                PipeEndpoint::bind(Path::new("unused"), bad),
                Err(BindError::Refused(_))
            ));
            assert_eq!(
                PipeEndpoint::connect(Path::new("unused"), bad, soon())
                    .err()
                    .map(|e| e.kind()),
                Some(io::ErrorKind::InvalidInput)
            );
        }
    }

    #[test]
    fn a_bound_pipe_is_listed_and_reached_until_it_goes() {
        let (token, mut listener) = bound();
        assert!(PipeEndpoint::instances(Path::new("unused")).contains(&token));
        let mut client = connect(&token);
        let mut server = listener
            .accept(soon())
            .expect("accepts")
            .expect("a connection");
        client.set_deadline(soon());
        server.set_deadline(soon());
        client.write_all(b"ping").expect("writes");
        let mut got = [0; 4];
        server.read_exact(&mut got).expect("reads");
        assert_eq!(&got, b"ping");
        server.write_all(b"pong").expect("writes");
        client.read_exact(&mut got).expect("reads");
        assert_eq!(&got, b"pong");
        drop(client);
        server.flush().expect("the client closed");
        drop(server);
        drop(listener);
        assert!(!PipeEndpoint::instances(Path::new("unused")).contains(&token));
        assert_eq!(
            PipeEndpoint::connect(Path::new("unused"), &token, soon())
                .err()
                .map(|e| e.kind()),
            Some(io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn an_accept_ends_at_its_deadline_or_when_interrupted() {
        let (token, mut listener) = bound();
        let start = Instant::now();
        assert!(listener.accept(after(200)).expect("accept").is_none());
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(200) && took < Duration::from_secs(2),
            "{took:?}"
        );
        let interrupt = listener.interrupter().expect("an interrupter");
        let start = Instant::now();
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                interrupt.interrupt();
            });
            assert!(listener.accept(after(10_000)).expect("accept").is_none());
        });
        assert!(start.elapsed() < Duration::from_secs(2));
        // Interrupted for good, even with a client waiting.
        let _client = connect(&token);
        assert!(listener.accept(soon()).expect("accept").is_none());
    }

    #[test]
    fn reads_and_writes_stop_at_the_deadline() {
        let (token, mut listener) = bound();
        let _client = connect(&token);
        let mut server = listener
            .accept(soon())
            .expect("accepts")
            .expect("a connection");
        server.set_deadline(after(200));
        let start = Instant::now();
        let mut buf = [0; 8];
        assert_eq!(
            server.read(&mut buf).err().map(|e| e.kind()),
            Some(io::ErrorKind::TimedOut)
        );
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(200) && took < Duration::from_secs(2),
            "{took:?}"
        );
        // Past the deadline nothing is tried.
        assert_eq!(
            server.read(&mut buf).err().map(|e| e.kind()),
            Some(io::ErrorKind::TimedOut)
        );
        // A client that does not read: writes fill the buffers, then stop at the deadline.
        server.set_deadline(after(300));
        let big = vec![0; 1024 * 1024];
        assert_eq!(
            server.write_all(&big).err().map(|e| e.kind()),
            Some(io::ErrorKind::TimedOut)
        );
        // The flush that waits for the client to close stops at the deadline too.
        server.set_deadline(after(200));
        assert_eq!(
            server.flush().err().map(|e| e.kind()),
            Some(io::ErrorKind::TimedOut)
        );
    }

    #[test]
    fn what_a_peer_sent_before_it_closed_is_still_read() {
        let (token, mut listener) = bound();
        let mut client = connect(&token);
        client.set_deadline(soon());
        client.write_all(b"last words").expect("writes");
        drop(client);
        let mut server = listener
            .accept(soon())
            .expect("accepts")
            .expect("a connection");
        server.set_deadline(soon());
        let mut got = Vec::new();
        server.read_to_end(&mut got).expect("reads");
        assert_eq!(got, b"last words");
    }

    #[test]
    fn a_busy_pipe_is_waited_for_within_the_deadline() {
        let (token, mut listener) = bound();
        let first = connect(&token);
        let server = listener
            .accept(soon())
            .expect("accepts")
            .expect("a connection");
        let start = Instant::now();
        assert_eq!(
            PipeEndpoint::connect(Path::new("unused"), &token, after(300))
                .err()
                .map(|e| e.kind()),
            Some(io::ErrorKind::TimedOut)
        );
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(300) && took < Duration::from_secs(2),
            "{took:?}"
        );
        // Freed while a second client waits: it gets the instance.
        thread::scope(|scope| {
            let second = scope.spawn(|| connect(&token));
            thread::sleep(Duration::from_millis(100));
            drop(first);
            drop(server);
            assert!(listener.accept(soon()).expect("accepts").is_some());
            second.join().expect("join");
        });
    }

    /// A scan's first try: no time to wait, still one look at the pipe.
    #[test]
    fn a_try_without_a_wait_takes_a_free_pipe_and_leaves_a_busy_one_at_once() {
        let (token, _listener) = bound();
        let now = || PipeEndpoint::connect(Path::new("unused"), &token, Instant::now());
        let _first = now().expect("free");
        let start = Instant::now();
        assert_eq!(now().err().map(|e| e.kind()), Some(io::ErrorKind::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn the_connection_still_open_keeps_the_next_accept_out() {
        let (token, mut listener) = bound();
        let _client = connect(&token);
        let server = listener
            .accept(soon())
            .expect("accepts")
            .expect("a connection");
        assert!(listener.accept(after(50)).is_err());
        drop(server);
        assert!(listener.accept(after(50)).expect("accept").is_none());
    }
}
