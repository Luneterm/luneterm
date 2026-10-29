//! macOS: the effect view is inserted into the window's frame view *below* the content view (the
//! same place Avalonia puts its `_blurBehind`), so it can never cover what wgpu draws.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_app_kit::NSView;
use raw_window_handle::{
    AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
};
use window_vibrancy::{
    LiquidGlassOptions, NSGlassEffectViewStyle, NSVisualEffectMaterial, NSVisualEffectState,
    apply_liquid_glass, apply_vibrancy, clear_liquid_glass, clear_vibrancy,
};

use super::Material;

// `Light` is deprecated by Apple but is exactly what Avalonia (and so Lunegit) uses.
#[allow(deprecated)]
pub(super) fn apply(window: &winit::window::Window, material: Material) -> Result<(), String> {
    let frame = FrameView::of(window).ok_or("no NSWindow frame view")?;
    // Clearing an effect that was never applied is a no-op (`Ok(false)`).
    clear_vibrancy(&frame).map_err(|e| e.to_string())?;
    clear_liquid_glass(&frame).map_err(|e| e.to_string())?;

    let result = match material {
        Material::Vibrancy => apply_vibrancy(
            &frame,
            NSVisualEffectMaterial::Light,
            Some(NSVisualEffectState::Active),
            None,
        ),
        Material::VibrancyFollowsWindow => apply_vibrancy(
            &frame,
            NSVisualEffectMaterial::Light,
            Some(NSVisualEffectState::FollowsWindowActiveState),
            None,
        ),
        Material::LiquidClear => apply_liquid_glass(
            &frame,
            LiquidGlassOptions::new(NSGlassEffectViewStyle::Clear),
        ),
        Material::LiquidRegular => apply_liquid_glass(
            &frame,
            LiquidGlassOptions::new(NSGlassEffectViewStyle::Regular),
        ),
        Material::Opaque => Ok(()),
    };
    result.map_err(|e| e.to_string())
}

/// The NSWindow's frame view (superview of winit's content view), exposed as a window handle
/// so `window-vibrancy` inserts its effect view there, below the content view.
struct FrameView(NonNull<c_void>);

impl FrameView {
    fn of(window: &winit::window::Window) -> Option<Self> {
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: winit guarantees `ns_view` is a live NSView on the main thread while the
        // window exists; we only call it from the event loop (main thread).
        let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
        // SAFETY: main thread (see above); `superview` only reads the view hierarchy.
        let frame = unsafe { view.superview() }?;
        // The frame view is owned by the NSWindow, which outlives this short-lived handle.
        Some(Self(NonNull::from(&*frame).cast()))
    }
}

impl HasWindowHandle for FrameView {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
        // SAFETY: the pointer is a valid NSView for the lifetime of `self` (see `of`).
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}
