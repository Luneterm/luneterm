//! C ABI for embedding the engine in Lunegit through P/Invoke behind its `IVtEngine`.
//!
//! Rules: every exported symbol is prefixed `luneterm_`, never panics across the boundary
//! (`panic = "abort"` in release), and bumps [`ABI_VERSION`] on any breaking change.

/// Incremented on every breaking change to the exported functions.
pub const ABI_VERSION: u32 = 1;

/// ABI version, so the host can refuse a mismatched library at load time.
#[unsafe(no_mangle)]
pub extern "C" fn luneterm_abi_version() -> u32 {
    ABI_VERSION
}
