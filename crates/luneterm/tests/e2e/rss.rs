//! Resident-set measurement for the flood case (E6): macOS and Linux only.
//!
//! The instrument is `ps -o rss= -p <pid>` on macOS and `VmRSS` in `/proc/self/status` on Linux,
//! both in KiB. [`calibrate`] checks it against an allocation of known size before any number is
//! trusted.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The size the calibration allocates and touches.
pub(crate) const CALIBRATION_BYTES: usize = 64 << 20;
/// The least growth the calibration must see.
pub(crate) const CALIBRATION_MIN: u64 = 56 << 20;

/// Whether this OS has an instrument.
pub(crate) const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// This process's resident set, bytes.
pub(crate) fn resident_bytes() -> Result<u64, String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .map_err(|e| format!("ps: {e}"))?;
        if !out.status.success() {
            return Err(format!("ps exited with {}", out.status));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let kib: u64 = text
            .trim()
            .parse()
            .map_err(|_| "ps printed no number".to_owned())?;
        Ok(kib * 1024)
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status")
            .map_err(|e| format!("/proc/self/status: {e}"))?;
        let line = status
            .lines()
            .find(|l| l.starts_with("VmRSS:"))
            .ok_or("no VmRSS line")?;
        let kib: u64 = line
            .split_whitespace()
            .nth(1)
            .and_then(|n| n.parse().ok())
            .ok_or("VmRSS has no number")?;
        Ok(kib * 1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("no RSS instrument on this OS".to_owned())
    }
}

/// Allocates and touches [`CALIBRATION_BYTES`], and checks the instrument saw at least
/// [`CALIBRATION_MIN`] more than before. Returns the growth it saw.
pub(crate) fn calibrate() -> Result<u64, String> {
    let before = resident_bytes()?;
    // Non-zero bytes, written, so the pages are really resident (a zeroed allocation is lazy).
    let block = std::hint::black_box(vec![0xA5u8; CALIBRATION_BYTES]);
    let during = resident_bytes()?;
    let touched = block.iter().step_by(4096).fold(0u8, |a, b| a ^ b);
    std::hint::black_box(touched);
    drop(block);
    let growth = during.saturating_sub(before);
    if growth < CALIBRATION_MIN {
        return Err(format!(
            "RSS instrument saw {} MiB growth for a 64 MiB allocation (needs at least 56)",
            growth >> 20
        ));
    }
    Ok(growth)
}

/// Samples the resident set on a thread and keeps the highest value.
pub(crate) struct Peak {
    max: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Peak {
    pub(crate) fn start(every: Duration) -> Peak {
        let max = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (m, s) = (Arc::clone(&max), Arc::clone(&stop));
        let handle = thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                if let Ok(v) = resident_bytes() {
                    m.fetch_max(v, Ordering::SeqCst);
                }
                thread::sleep(every);
            }
        });
        Peak {
            max,
            stop,
            handle: Some(handle),
        }
    }

    /// Stops sampling, takes one last sample, and returns the highest value seen.
    pub(crate) fn finish(mut self) -> Result<u64, String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let last = resident_bytes()?;
        self.max.fetch_max(last, Ordering::SeqCst);
        Ok(self.max.load(Ordering::SeqCst))
    }
}

impl Drop for Peak {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
