//! Shared helpers for the GPU tests: a headless device with no window or surface.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// One device at a time in a test process. Windows' software adapter (WARP, `d3d10warp.dll`, what
/// the CI VM has) crashes the process when several devices render at once: 15 of 25 parallel runs
/// of `golden` died with an access violation, 0 of 25 with `--test-threads=1`.
static GPU: Mutex<()> = Mutex::new(());

/// Held while a test uses its device. Bind it before the device so it is dropped after it.
pub(crate) type GpuLock = MutexGuard<'static, ()>;

/// The turn at the device, a headless device, its queue, and a `<name> <backend> <device_type>`
/// line for the adapter. Waits for the test before it to finish.
///
/// Without an adapter this prints a `SKIPPED` line and returns `None`, unless the environment
/// variable `LUNE_REQUIRE_GPU=1` is set, in which case it panics.
pub(crate) fn device() -> Option<(GpuLock, wgpu::Device, wgpu::Queue, String)> {
    // A failed test poisons the lock; the next test's device does not depend on that one.
    let lock = GPU.lock().unwrap_or_else(PoisonError::into_inner);
    match request() {
        Ok((device, queue, line)) => Some((lock, device, queue, line)),
        Err(e) => {
            if std::env::var("LUNE_REQUIRE_GPU").is_ok_and(|v| v == "1") {
                panic!("LUNE_REQUIRE_GPU=1 but no usable wgpu adapter: {e}");
            }
            eprintln!("SKIPPED: no wgpu adapter: {e}");
            None
        }
    }
}

fn request() -> Result<(wgpu::Device, wgpu::Queue, String), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|e| e.to_string())?;
    let info = adapter.get_info();
    let line = format!("{} {:?} {:?}", info.name, info.backend, info.device_type);
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| e.to_string())?;
    Ok((device, queue, line))
}
