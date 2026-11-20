//! Windows: EcoQoS plus the idle priority class, Task Manager's own recipe (the green leaf in its
//! Status column; the probe of step 0 showed that neither half alone does it).
//!
//! `apply` remembers the priority class found the first time (a user may have started Luneterm
//! `/low`), turns on `PROCESS_POWER_THROTTLING_EXECUTION_SPEED` and sets the idle class. `lift`
//! sets the control mask to 0 (back to system-managed; NOT an explicit "high QoS", which would pin
//! the process at High even when minimized) and restores the class. The idle class is process-wide,
//! so it slows the PTY threads too: the controller's quiet gate (`SLOWS_IO_PATH`) keeps it off
//! while a tab prints. A child created while it is on inherits the idle class (not the EcoQoS tag),
//! so every child creation runs inside `Controller::lifted`.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::mem::size_of;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetPriorityClass, IDLE_PRIORITY_CLASS,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetPriorityClass,
    SetProcessInformation,
};

use super::{OsError, OsThrottle};

/// EcoQoS and the idle class on the own process.
#[derive(Debug, Default)]
pub struct EcoQos {
    /// The class found at the first `apply`.
    saved_class: Option<u32>,
}

fn last_error(call: &'static str) -> OsError {
    // SAFETY: no arguments; reads the calling thread's last-error value.
    let code = unsafe { GetLastError() };
    OsError {
        call,
        code: i64::from(code),
    }
}

/// `SetProcessInformation(ProcessPowerThrottling)` on the own process with `control` / `state`
/// (both `EXECUTION_SPEED` to throttle, both 0 to hand the decision back to the system).
fn set_throttling(control: u32, state: u32) -> Result<(), OsError> {
    let info = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: control,
        StateMask: state,
    };
    // SAFETY: `GetCurrentProcess` is the always-valid pseudo handle; `info` is a live
    // `PROCESS_POWER_THROTTLING_STATE` and the size passed is its size.
    let ok = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&raw const info).cast::<c_void>(),
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    if ok == 0 {
        Err(last_error("SetProcessInformation"))
    } else {
        Ok(())
    }
}

fn set_class(class: u32) -> Result<(), OsError> {
    // SAFETY: the pseudo handle of the own process; `class` is a priority class this OS reported
    // or `IDLE_PRIORITY_CLASS`.
    let ok = unsafe { SetPriorityClass(GetCurrentProcess(), class) };
    if ok == 0 {
        Err(last_error("SetPriorityClass"))
    } else {
        Ok(())
    }
}

impl OsThrottle for EcoQos {
    const ACTIVE: bool = true;
    /// The idle class is process-wide, the PTY threads included: the quiet gate applies.
    const SLOWS_IO_PATH: bool = true;

    fn apply(&mut self) -> Result<(), OsError> {
        if self.saved_class.is_none() {
            // SAFETY: the pseudo handle of the own process.
            let class = unsafe { GetPriorityClass(GetCurrentProcess()) };
            if class == 0 {
                return Err(last_error("GetPriorityClass"));
            }
            self.saved_class = Some(class);
        }
        set_throttling(
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        )?;
        set_class(IDLE_PRIORITY_CLASS)
    }

    fn lift(&mut self) -> Result<(), OsError> {
        // Both halves are tried; the first error is the one reported.
        let eco = set_throttling(0, 0);
        let class = match self.saved_class {
            Some(class) => set_class(class),
            None => Ok(()),
        };
        eco.and(class)
    }
}
