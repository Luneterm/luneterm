//! OS glass behind the window.
//!
//! macOS: the effect view is inserted into the window's frame view *below* the content view (the
//! same place Avalonia puts its `_blurBehind`), so it can never cover what wgpu draws.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassMode {
    /// Lunegit's material (NSVisualEffectMaterialLight), kept active when unfocused.
    Vibrancy,
    /// Exactly Lunegit: material follows window state; app shows solid #333333 when unfocused.
    VibrancyLunegit,
    LiquidClear,
    LiquidRegular,
    /// No OS glass: what Linux gets.
    Opaque,
}

impl GlassMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vibrancy => "Vibrancy (stays on when unfocused)",
            Self::VibrancyLunegit => "Vibrancy (Lunegit-exact)",
            Self::LiquidClear => "Liquid Glass · clear",
            Self::LiquidRegular => "Liquid Glass · regular",
            Self::Opaque => "Opaque (Linux fallback)",
        }
    }

    #[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
    pub fn is_glass(self) -> bool {
        self != Self::Opaque
    }
}

// `Light` is deprecated by Apple but is exactly what Avalonia (and so Lunegit) uses.
#[allow(deprecated)]
#[cfg(target_os = "macos")]
pub fn apply(window: &winit::window::Window, mode: GlassMode) -> Result<(), String> {
    use window_vibrancy::{
        LiquidGlassOptions, NSGlassEffectViewStyle, NSVisualEffectMaterial, NSVisualEffectState,
        apply_liquid_glass, apply_vibrancy, clear_liquid_glass, clear_vibrancy,
    };

    let frame = macos::FrameView::of(window).ok_or("no NSWindow frame view")?;
    // Clearing an effect that was never applied is a no-op (`Ok(false)`).
    clear_vibrancy(&frame).map_err(|e| e.to_string())?;
    clear_liquid_glass(&frame).map_err(|e| e.to_string())?;

    let result = match mode {
        GlassMode::Vibrancy => apply_vibrancy(
            &frame,
            NSVisualEffectMaterial::Light,
            Some(NSVisualEffectState::Active),
            None,
        ),
        GlassMode::VibrancyLunegit => apply_vibrancy(
            &frame,
            NSVisualEffectMaterial::Light,
            Some(NSVisualEffectState::FollowsWindowActiveState),
            None,
        ),
        GlassMode::LiquidClear => apply_liquid_glass(
            &frame,
            LiquidGlassOptions::new(NSGlassEffectViewStyle::Clear),
        ),
        GlassMode::LiquidRegular => apply_liquid_glass(
            &frame,
            LiquidGlassOptions::new(NSGlassEffectViewStyle::Regular),
        ),
        GlassMode::Opaque => Ok(()),
    };
    result.map_err(|e| e.to_string())
}

/// Windows 11 DWM system backdrops. Avalonia (Lunegit) builds its acrylic from the WinUI
/// composition recipe (host backdrop → blur → saturation → luminosity/tint); DWM's acrylic is the
/// system version of the same recipe.
#[cfg(target_os = "windows")]
pub fn apply(window: &winit::window::Window, mode: GlassMode) -> Result<(), String> {
    use window_vibrancy::{
        apply_acrylic, apply_mica, apply_tabbed, clear_acrylic, clear_mica, clear_tabbed,
    };
    // Clearing a backdrop that was never applied is harmless.
    let _ = clear_acrylic(window);
    let _ = clear_mica(window);
    let _ = clear_tabbed(window);
    let result = match mode {
        GlassMode::Vibrancy | GlassMode::VibrancyLunegit => apply_acrylic(window, None),
        GlassMode::LiquidClear => apply_mica(window, Some(true)),
        GlassMode::LiquidRegular => apply_tabbed(window, Some(true)),
        GlassMode::Opaque => Ok(()),
    };
    result.map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn apply(_window: &winit::window::Window, mode: GlassMode) -> Result<(), String> {
    if mode.is_glass() {
        Err("no OS glass on this platform; Linux uses the opaque theme".into())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_app_kit::NSView;
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    };

    /// The NSWindow's frame view (superview of winit's content view), exposed as a window handle
    /// so `window-vibrancy` inserts its effect view there, below the content view.
    pub(super) struct FrameView(NonNull<c_void>);

    impl FrameView {
        pub(super) fn of(window: &winit::window::Window) -> Option<Self> {
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
}
