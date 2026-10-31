//! Unix parts of the portable-pty backend.

use std::io;
use std::os::unix::process::ExitStatusExt;

use portable_pty::Child;

/// Non-blocking exit code. A signal death reports `128 + signo` (shell convention); portable-pty's
/// own `ExitStatus` reports 1 for every signal, so the std child is read directly.
pub(crate) fn try_exit_code(child: &mut (dyn Child + Send + Sync)) -> io::Result<Option<i32>> {
    let child: &mut dyn Child = child;
    if let Some(std_child) = child.downcast_mut::<std::process::Child>() {
        return Ok(std_child.try_wait()?.map(|status| {
            status
                .code()
                .or_else(|| status.signal().map(|signo| 128 + signo))
                .unwrap_or(1)
        }));
    }
    // Not reached with portable-pty 0.9.0 (its Unix child is a std::process::Child).
    Ok(child
        .try_wait()?
        .map(|status| i32::try_from(status.exit_code()).unwrap_or(1)))
}
