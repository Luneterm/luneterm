//! Platforms without an OS backdrop material.

use super::Material;

pub(super) fn apply(_window: &winit::window::Window, material: Material) -> Result<(), String> {
    if material.is_glass() {
        Err("no OS glass on this platform; use the opaque material".into())
    } else {
        Ok(())
    }
}
