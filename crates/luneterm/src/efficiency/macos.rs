//! macOS: the UI thread's quality-of-service class to utility while throttled.
//!
//! The class is per thread (`pthread_set_qos_class_self_np` acts on the caller), so only the
//! thread that first called [`UtilityQos::apply`] may call it again (debug assertion): the UI
//! thread, where the app owns the controller. The PTY threads are untouched, and a child's QoS is
//! reset by exec, so shells keep normal priority (the probe of the plan).
#![allow(unsafe_code)]

use std::thread::ThreadId;

use super::{OsError, OsThrottle};

/// `QOS_CLASS_UTILITY` of `<sys/qos.h>`. The classes are plain `u32`s here, not `libc`'s enum:
/// an enum read from the OS would be undefined behaviour for a value the enum lacks.
const QOS_CLASS_UTILITY: u32 = 0x11;

unsafe extern "C" {
    fn pthread_get_qos_class_np(
        thread: libc::pthread_t,
        class: *mut u32,
        relative_priority: *mut libc::c_int,
    ) -> libc::c_int;
    fn pthread_set_qos_class_self_np(class: u32, relative_priority: libc::c_int) -> libc::c_int;
}

/// A QoS class and its relative priority, as `pthread_get_qos_class_np` reports them.
#[derive(Debug, Clone, Copy)]
struct Qos {
    class: u32,
    relative: libc::c_int,
}

/// The UI thread at utility QoS while applied.
#[derive(Debug, Default)]
pub struct UtilityQos {
    /// The thread that first called, and the class it had then (what `lift` restores).
    first: Option<(ThreadId, Qos)>,
}

impl UtilityQos {
    fn check_thread(&self) {
        if let Some((id, _)) = &self.first {
            debug_assert_eq!(
                *id,
                std::thread::current().id(),
                "the QoS backend acts on the thread that first called it"
            );
        }
    }
}

fn current_qos() -> Result<Qos, OsError> {
    let mut class = 0u32;
    let mut relative: libc::c_int = 0;
    // SAFETY: `pthread_self()` is the calling thread's valid handle; `class` and `relative` are
    // live, writable locals of the types the call takes.
    let rc = unsafe { pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative) };
    if rc == 0 {
        Ok(Qos { class, relative })
    } else {
        Err(OsError {
            call: "pthread_get_qos_class_np",
            code: i64::from(rc),
        })
    }
}

fn set_qos(class: u32, relative: libc::c_int) -> Result<(), OsError> {
    // SAFETY: a plain call that changes the calling thread's own QoS; the arguments are a class
    // this OS reported or a documented constant, with a relative priority of 0 or one it reported.
    let rc = unsafe { pthread_set_qos_class_self_np(class, relative) };
    if rc == 0 {
        Ok(())
    } else {
        Err(OsError {
            call: "pthread_set_qos_class_self_np",
            code: i64::from(rc),
        })
    }
}

impl OsThrottle for UtilityQos {
    const ACTIVE: bool = true;
    /// The PTY threads keep their QoS: no quiet gate.
    const SLOWS_IO_PATH: bool = false;

    fn apply(&mut self) -> Result<(), OsError> {
        self.check_thread();
        if self.first.is_none() {
            self.first = Some((std::thread::current().id(), current_qos()?));
        }
        set_qos(QOS_CLASS_UTILITY, 0)
    }

    fn lift(&mut self) -> Result<(), OsError> {
        self.check_thread();
        match self.first {
            Some((_, q)) => set_qos(q.class, q.relative),
            // Never applied: nothing to restore.
            None => Ok(()),
        }
    }
}
