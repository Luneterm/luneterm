//! Windows parts of the portable-pty backend.

#![allow(unsafe_code)]

use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle};
use std::sync::OnceLock;
use std::thread;

use portable_pty::Child;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, SetDefaultDllDirectories,
};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

/// Outcome of the one `SetDefaultDllDirectories` call: `Err` holds the OS error code. An
/// `io::Error` is not `Clone`, so the code is kept and the error rebuilt on each call.
static DLL_SEARCH: OnceLock<Result<(), i32>> = OnceLock::new();

/// Restricts the process-wide DLL search to the application directory, System32 and directories
/// added with `AddDllDirectory`, once. portable-pty loads `conpty.dll` by bare name; with the
/// default search order that finds a planted copy in the current directory or on `PATH` first.
///
/// Process-wide and permanent: every later bare-name `LoadLibrary` in this process skips the
/// current directory and `PATH`. Fails closed: the call's error is returned on every attempt,
/// and the backend then refuses to spawn.
pub(crate) fn ensure_dll_search_restricted() -> io::Result<()> {
    let result = DLL_SEARCH.get_or_init(|| {
        // SAFETY: plain Win32 call with a valid flag constant and no pointers.
        if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error().raw_os_error().unwrap_or(-1))
        }
    });
    match result {
        Ok(()) => Ok(()),
        Err(code) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("SetDefaultDllDirectories failed (os error {code}); refusing to load conpty"),
        )),
    }
}

/// Starts the exit-watcher thread: it waits on a duplicate of the child's process handle and
/// calls `on_exit` once the child has exited. If the wait itself fails, `on_exit` is not called
/// (the pseudoconsole is then closed when the `Pty` drops, on its reap thread).
pub(crate) fn watch_exit(
    child: &(dyn Child + Send + Sync),
    on_exit: impl FnOnce() + Send + 'static,
) -> io::Result<()> {
    let raw = child
        .as_raw_handle()
        .ok_or_else(|| io::Error::other("child has no process handle"))?;
    // SAFETY: `raw` is the process handle owned by `child`, which is alive for this whole
    // function; the borrow is used only to duplicate it into a handle the thread owns.
    let process: OwnedHandle = unsafe { BorrowedHandle::borrow_raw(raw) }.try_clone_to_owned()?;
    thread::Builder::new()
        .name("lune-pty-exit".into())
        .spawn(move || {
            // SAFETY: `process` is an owned, open process handle that lives until the end of
            // this closure, so it stays valid for the whole wait.
            let result =
                unsafe { WaitForSingleObject(process.as_raw_handle() as HANDLE, INFINITE) };
            drop(process);
            if result == WAIT_OBJECT_0 {
                on_exit();
            }
        })?;
    Ok(())
}
