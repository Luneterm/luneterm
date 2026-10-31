//! Windows parts of the portable-pty backend.

#![allow(unsafe_code)]

use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle};
use std::thread;

use portable_pty::Child;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

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
