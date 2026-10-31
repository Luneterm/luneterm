//! Shared helpers for the GPU tests: a headless device with no window or surface.

/// A headless device, its queue, and a `<name> <backend> <device_type>` line for the adapter.
///
/// Without an adapter this prints a `SKIPPED` line and returns `None`, unless the environment
/// variable `LUNE_REQUIRE_GPU=1` is set, in which case it panics.
pub(crate) fn device() -> Option<(wgpu::Device, wgpu::Queue, String)> {
    match request() {
        Ok(found) => Some(found),
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
