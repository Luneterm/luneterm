//! Efficiency mode against the real OS: the own process's (Windows) or thread's (macOS) state
//! changes with focus, and a child created inside `Controller::lifted` while the window is in the
//! background runs at normal priority (plan copy-select-efficiency, step 3).
//!
//! A test binary of its own: it changes the state of the test process (Windows: the priority class
//! and EcoQoS of the whole process) and spawns children. Each OS has one `#[test]` that runs its
//! cases in order. On other OSes this file compiles to nothing.

#![cfg(any(windows, target_os = "macos"))]
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::time::Instant;

use lune_pty::{Pty, PtyBackend, PtySize, SpawnOptions};
use luneterm::efficiency::{Controller, HostOs};

fn size() -> PtySize {
    PtySize {
        columns: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// A child that lives for a few seconds, on a real PTY, created through the app's backend.
fn spawn_child() -> Box<dyn Pty> {
    let (program, args): (&str, &[&str]) = if cfg!(windows) {
        ("cmd.exe", &["/c", "ping", "-n", "8", "127.0.0.1"])
    } else {
        ("/bin/sleep", &["6"])
    };
    let options = SpawnOptions {
        program: Some(PathBuf::from(program)),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        cwd: None,
        env: Vec::new(),
    };
    lune_pty::PortablePtyBackend::new()
        .spawn(&options, size())
        .expect("a child on a PTY")
}

#[cfg(target_os = "macos")]
mod os {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    const QOS_CLASS_UTILITY: u32 = 0x11;

    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(class: u32, relative_priority: libc::c_int)
        -> libc::c_int;
        fn pthread_get_qos_class_np(
            thread: libc::pthread_t,
            class: *mut u32,
            relative_priority: *mut libc::c_int,
        ) -> libc::c_int;
    }

    /// The calling thread's QoS class.
    fn qos() -> u32 {
        let mut class = 0u32;
        let mut relative: libc::c_int = 0;
        // SAFETY: the calling thread's own handle and two live, writable locals.
        let rc =
            unsafe { pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative) };
        assert_eq!(rc, 0, "pthread_get_qos_class_np");
        class
    }

    /// What the test binary re-executed as a child reports (`child_reports_qos`): its own QoS.
    pub(crate) fn report_own_qos() {
        match std::env::var("EFFICIENCY_OS_CHILD").as_deref() {
            Ok("utility") => {
                // The known answer for the reader: a child that sets utility itself reports it.
                // SAFETY: a plain call that changes the calling thread's own QoS.
                let rc = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0) };
                assert_eq!(rc, 0);
            }
            Ok(_) => {}
            Err(_) => return,
        }
        println!("QOS={}", qos());
    }

    /// Re-executes this test binary on a PTY through the app's backend (so as the app creates a
    /// shell) and returns the QoS class the child reports, and its pid.
    fn child_qos(mode: &str) -> (u32, Box<dyn Pty>) {
        use std::io::Read;
        let options = SpawnOptions {
            program: Some(std::env::current_exe().expect("this binary")),
            args: [
                "--exact",
                "child_reports_qos",
                "--nocapture",
                "--test-threads=1",
            ]
            .map(str::to_owned)
            .to_vec(),
            cwd: None,
            env: vec![("EFFICIENCY_OS_CHILD".to_owned(), mode.to_owned())],
        };
        let mut pty = lune_pty::PortablePtyBackend::new()
            .spawn(&options, size())
            .expect("the test binary as a child");
        let mut reader = pty.take_reader().expect("reader");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut buf = [0u8; 256];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen).into_owned();
                if let Some(rest) = text.split("QOS=").nth(1)
                    && let Some(num) = rest.split(['\r', '\n']).next()
                    && rest.contains(['\r', '\n'])
                {
                    let _ = tx.send(num.trim().parse::<u32>().ok());
                    return;
                }
            }
            let _ = tx.send(None);
        });
        let class = rx
            .recv_timeout(Duration::from_secs(60))
            .expect("the child answered in time")
            .expect("the child printed its QoS");
        (class, pty)
    }

    /// The scheduling priority `ps` shows for `pid` (the probe saw the same number for a child
    /// made at normal and at utility QoS).
    fn pri(pid: u32) -> String {
        let out = Command::new("ps")
            .args(["-o", "pri=", "-p", &pid.to_string()])
            .output()
            .expect("ps");
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        assert!(!text.is_empty(), "ps found pid {pid}");
        text
    }

    pub(crate) fn cases() {
        // The instrument first: a child that sets utility itself reports utility, one that does
        // nothing reports something else.
        let (known, _k) = child_qos("utility");
        assert_eq!(known, QOS_CLASS_UTILITY, "the reader sees a utility child");
        let (plain, _p) = child_qos("plain");
        assert_ne!(plain, QOS_CLASS_UTILITY, "and a plain child as not utility");

        // The QoS class is per thread: run the cases on a thread of their own, the one the
        // controller acts on.
        std::thread::spawn(move || {
            let before = qos();
            assert_ne!(
                before, QOS_CLASS_UTILITY,
                "the test thread starts above utility"
            );
            let now = Instant::now();
            let mut c = Controller::new(HostOs::default(), true);

            // Focus loss: this thread is at utility; focus gain: back to what it was.
            c.set_focused(false, now);
            assert!(c.applied());
            assert_eq!(qos(), QOS_CLASS_UTILITY, "throttled");
            c.set_focused(true, now);
            assert!(!c.applied());
            assert_eq!(qos(), before, "restored to the class it had");
            c.set_focused(false, now);
            assert_eq!(qos(), QOS_CLASS_UTILITY);

            // A child created inside `lifted` while throttled: the thread is unthrottled while
            // it is created, and throttled again after.
            let mut seen_inside = 0;
            let mut lifted_child = c.lifted(now, || {
                seen_inside = qos();
                spawn_child()
            });
            assert_eq!(
                seen_inside, before,
                "inside `lifted` the thread is not throttled"
            );
            assert_eq!(qos(), QOS_CLASS_UTILITY, "throttled again after");

            // The control: a child created while the thread is not throttled.
            c.set_focused(true, now);
            let mut control = spawn_child();
            std::thread::sleep(Duration::from_millis(300));
            let (a, b) = (
                pri(lifted_child.process_id().expect("pid")),
                pri(control.process_id().expect("pid")),
            );
            assert_eq!(
                a, b,
                "the lifted child runs at the control child's priority"
            );
            let _ = lifted_child.kill();
            let _ = control.kill();

            // What the shells themselves see: a child created through the app's backend inside
            // `lifted` while throttled, and one created WITHOUT `lifted` while throttled, both
            // report a class that is not utility (the QoS does not survive exec, the probe).
            c.set_focused(false, now);
            let (inside, _i) = c.lifted(now, || child_qos("plain"));
            let (without, _w) = child_qos("plain");
            assert_eq!(qos(), QOS_CLASS_UTILITY, "this thread stayed throttled");
            assert_eq!(inside, plain, "a lifted child reports a normal class");
            assert_eq!(
                without, plain,
                "QoS does not survive exec: even an unlifted child"
            );
        })
        .join()
        .expect("the cases ran");
    }
}

#[cfg(windows)]
mod os {
    use super::*;
    use std::ffi::c_void;
    use std::mem::size_of;

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetPriorityClass, GetProcessInformation, IDLE_PRIORITY_CLASS,
        NORMAL_PRIORITY_CLASS, OpenProcess, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, ProcessPowerThrottling,
    };

    /// (priority class, ControlMask, StateMask) of a process handle.
    fn state_of(handle: windows_sys::Win32::Foundation::HANDLE) -> (u32, u32, u32) {
        let mut info = PROCESS_POWER_THROTTLING_STATE {
            Version: 0,
            ControlMask: 0xdead,
            StateMask: 0xdead,
        };
        // SAFETY: `handle` is a live process handle with query rights; `info` is a live, writable
        // `PROCESS_POWER_THROTTLING_STATE` and the size passed is its size.
        let ok = unsafe {
            GetProcessInformation(
                handle,
                ProcessPowerThrottling,
                (&raw mut info).cast::<c_void>(),
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        };
        assert_ne!(ok, 0, "GetProcessInformation");
        // SAFETY: the same live handle.
        let class = unsafe { GetPriorityClass(handle) };
        assert_ne!(class, 0, "GetPriorityClass");
        (class, info.ControlMask, info.StateMask)
    }

    fn own() -> (u32, u32, u32) {
        // SAFETY: the always-valid pseudo handle of the own process.
        state_of(unsafe { GetCurrentProcess() })
    }

    fn state_of_pid(pid: u32) -> (u32, u32, u32) {
        // SAFETY: a plain open of another process with query-only rights.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        assert!(!handle.is_null(), "OpenProcess({pid})");
        let state = state_of(handle);
        // SAFETY: the handle opened above, closed once.
        unsafe { CloseHandle(handle) };
        state
    }

    pub(crate) fn cases() {
        let before = own();
        assert_eq!(
            before.0, NORMAL_PRIORITY_CLASS,
            "the test process starts at normal priority"
        );
        assert_eq!(before.1, 0, "and system-managed");
        let now = Instant::now();
        let mut c = Controller::new(HostOs::default(), true);

        // Focus loss: EcoQoS on and the idle class (the green leaf's recipe).
        c.set_focused(false, now);
        assert!(c.applied());
        let (class, control, state) = own();
        assert_eq!(class, IDLE_PRIORITY_CLASS);
        assert_eq!(
            control & PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            1,
            "Control"
        );
        assert_eq!(state & PROCESS_POWER_THROTTLING_EXECUTION_SPEED, 1, "State");

        // A child created inside `lifted` while throttled: normal class, system-managed.
        let mut seen_inside = (0, 0, 0);
        let mut lifted_child = c.lifted(now, || {
            seen_inside = own();
            spawn_child()
        });
        assert_eq!(
            seen_inside.0, NORMAL_PRIORITY_CLASS,
            "unthrottled while the child is created"
        );
        assert_eq!(own().0, IDLE_PRIORITY_CLASS, "throttled again after");
        let pid = lifted_child.process_id().expect("pid");
        let (class, control, _) = state_of_pid(pid);
        assert_eq!(class, NORMAL_PRIORITY_CLASS, "the lifted child's class");
        assert_eq!(control, 0, "the lifted child's ControlMask");

        // The control (it proves the instrument): a child created while throttled, WITHOUT
        // `lifted`, inherits the idle class and not the EcoQoS tag (the probe of step 0).
        let mut control_child = spawn_child();
        let pid = control_child.process_id().expect("pid");
        let (class, control, _) = state_of_pid(pid);
        assert_eq!(
            class, IDLE_PRIORITY_CLASS,
            "an unlifted child inherits the idle class"
        );
        assert_eq!(control, 0, "and not the EcoQoS tag");

        // Focus gain: back to what it was.
        c.set_focused(true, now);
        assert!(!c.applied());
        assert_eq!(
            own(),
            before,
            "class and QoS control are what they were before"
        );

        let _ = lifted_child.kill();
        let _ = control_child.kill();
    }
}

/// Run as a child of `efficiency_mode_changes_the_os_state_and_children_stay_normal` (macOS),
/// with `EFFICIENCY_OS_CHILD` set: prints the QoS class this process runs at. Without the
/// variable it does nothing.
#[test]
fn child_reports_qos() {
    #[cfg(target_os = "macos")]
    os::report_own_qos();
}

#[test]
fn efficiency_mode_changes_the_os_state_and_children_stay_normal() {
    os::cases();
}
