//! C ABI for embedding the engine in Lunegit through P/Invoke behind its `IVtEngine` (M8).
//!
//! Rules:
//! - every exported symbol is prefixed `luneterm_`;
//! - no panic crosses the boundary: every export runs inside `catch_unwind` ([`guard`]), so the
//!   shipping library must unwind. Build it only with
//!   `cargo build -p luneterm-ffi --profile ffi-release` (`panic = "unwind"`); a
//!   `panic = "abort"` build of this crate is a compile error (M8 D4);
//! - a caught panic poisons the handle; every call but `luneterm_engine_free` then returns
//!   [`LUNETERM_ERR_POISONED`]. The library never installs a panic hook;
//! - outputs (viewport, events) are engine-owned slots: a pointer stays valid until the next call
//!   that fills the same slot, or `luneterm_engine_free` (see the `output` module docs);
//! - [`ABI_VERSION`] is bumped on any breaking change.
//!
//! 64-bit targets only (D6).

#[cfg(panic = "abort")]
compile_error!(
    "luneterm-ffi must be built with panic = \"unwind\" (M8 D4): use `cargo build -p luneterm-ffi --profile ffi-release`"
);

#[cfg(not(target_pointer_width = "64"))]
compile_error!("luneterm-ffi supports 64-bit targets only (ABI 1 layout, M8 D6)");

mod engine;
mod guard;
mod output;
mod types;

pub use engine::*;
pub use guard::{
    LUNETERM_ERR_BUSY, LUNETERM_ERR_INVALID_ARGUMENT, LUNETERM_ERR_INVALID_UTF8,
    LUNETERM_ERR_NULL_POINTER, LUNETERM_ERR_PANIC, LUNETERM_ERR_POISONED, LUNETERM_ERR_TOO_LARGE,
    LUNETERM_OK, LunetermEngine,
};
pub use output::*;
pub use types::*;

/// Incremented on every breaking change to the exported functions.
pub const ABI_VERSION: u32 = 1;

/// ABI version, so the host can refuse a mismatched library at load time.
#[unsafe(no_mangle)]
pub extern "C" fn luneterm_abi_version() -> u32 {
    ABI_VERSION
}
