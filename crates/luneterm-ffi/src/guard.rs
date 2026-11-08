//! The call guard: the handle, its lock, panic containment and the pointer helpers every export
//! goes through (M8 plan, ABI 1 rules R1-R3 and R5, D1, D4).
//!
//! Protocol, in this order, for every export:
//! 1. out-params are checked and zeroed ([`out_init`]), so no error leaves a stale value behind;
//! 2. every other pointer argument is validated ([`bytes_in`], [`ref_in`]), without touching the
//!    engine, so a bad argument never poisons it;
//! 3. [`with_engine`] checks the handle, takes a *shared* reference to it, and runs the body
//!    inside `catch_unwind` under `try_lock`: a concurrent call is [`LUNETERM_ERR_BUSY`], a
//!    panic is [`LUNETERM_ERR_PANIC`] and poisons the lock, and every later call on that
//!    handle is [`LUNETERM_ERR_POISONED`];
//! 4. results are written to the out-params only after the body returned `Ok`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, TryLockError};

use lune_vt::Terminal;

use crate::output::{EventsSlot, ViewportSlot};

/// Success.
pub const LUNETERM_OK: i32 = 0;
/// A required pointer (the handle, an out-param, or input data with a nonzero length) is NULL.
pub const LUNETERM_ERR_NULL_POINTER: i32 = -1;
/// An enum out of range, a size over a cap, a misaligned pointer, a length over `PTRDIFF_MAX`,
/// or an XTVERSION name lune-vt refuses.
pub const LUNETERM_ERR_INVALID_ARGUMENT: i32 = -2;
/// Text input that must be UTF-8 is not.
pub const LUNETERM_ERR_INVALID_UTF8: i32 = -3;
/// A result does not fit the ABI's integer type.
pub const LUNETERM_ERR_TOO_LARGE: i32 = -4;
/// A panic was caught inside the call; the handle is now poisoned.
pub const LUNETERM_ERR_PANIC: i32 = -5;
/// An earlier call panicked; only `luneterm_engine_free` still works on this handle.
pub const LUNETERM_ERR_POISONED: i32 = -6;
/// Another call is in progress on this handle (the host must serialize calls per handle).
pub const LUNETERM_ERR_BUSY: i32 = -7;

/// What a handle owns: the engine and its output slots (D3).
#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) term: Terminal,
    /// Filled by `luneterm_engine_viewport` only.
    pub(crate) viewport: ViewportSlot,
    /// Filled by `luneterm_engine_drain_events` only.
    pub(crate) events: EventsSlot,
    /// Replaced by every encoder call (key, char, mouse, focus, paste) only.
    pub(crate) input: Vec<u8>,
}

impl Inner {
    pub(crate) fn new(term: Terminal) -> Inner {
        Inner {
            term,
            viewport: ViewportSlot::default(),
            events: EventsSlot::default(),
            input: Vec::new(),
        }
    }
}

// The handle is shared across host threads (`Mutex<Inner>: Sync` needs `Inner: Send`). Proven by
// the compiler, never by an `unsafe impl` (D1).
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Inner>();
};

/// The opaque engine handle (`typedef struct LunetermEngine LunetermEngine;` in C). Created by
/// `luneterm_engine_new`, released by `luneterm_engine_free`.
#[derive(Debug)]
pub struct LunetermEngine(pub(crate) Mutex<Inner>);

impl LunetermEngine {
    pub(crate) fn new(inner: Inner) -> Self {
        LunetermEngine(Mutex::new(inner))
    }
}

/// Checks the handle, locks it without blocking and runs `body` with panics contained.
///
/// Pointer validation of the other arguments must already be done (R2). The returned `Err`
/// is a status code. The raw pointer is only ever turned into a shared reference: concurrent
/// callers each get `&`, and the mutex arbitrates.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle from `luneterm_engine_new` that has not been
/// freed (and is not being freed concurrently).
pub(crate) unsafe fn with_engine<R>(
    engine: *mut LunetermEngine,
    body: impl FnOnce(&mut Inner) -> Result<R, i32>,
) -> Result<R, i32> {
    if engine.is_null() {
        return Err(LUNETERM_ERR_NULL_POINTER);
    }
    if !engine.is_aligned() {
        return Err(LUNETERM_ERR_INVALID_ARGUMENT);
    }
    // SAFETY: non-null and aligned (checked above); live and not freed by the caller's contract.
    // Shared reference only, never `&mut` from the raw pointer.
    let engine: &LunetermEngine = unsafe { &*engine };
    let outcome = catch_unwind(AssertUnwindSafe(|| match engine.0.try_lock() {
        // A panic in `body` unwinds through `guard`, which poisons the mutex.
        Ok(mut guard) => body(&mut guard),
        Err(TryLockError::WouldBlock) => Err(LUNETERM_ERR_BUSY),
        Err(TryLockError::Poisoned(_)) => Err(LUNETERM_ERR_POISONED),
    }));
    match outcome {
        Ok(result) => result,
        Err(payload) => {
            discard_payload(payload);
            Err(LUNETERM_ERR_PANIC)
        }
    }
}

/// Drops a caught panic payload without formatting it; a payload whose `Drop` panics is leaked.
pub(crate) fn discard_payload(payload: Box<dyn std::any::Any + Send>) {
    if let Err(inner) = catch_unwind(AssertUnwindSafe(move || drop(payload))) {
        std::mem::forget(inner);
    }
}

/// Checks an out-param and writes `zero` to it (R1).
///
/// # Safety
///
/// `out` is NULL, misaligned, or valid for a write of `T` for `'a`.
pub(crate) unsafe fn out_init<'a, T: Copy>(out: *mut T, zero: T) -> Result<&'a mut T, i32> {
    if out.is_null() {
        return Err(LUNETERM_ERR_NULL_POINTER);
    }
    if !out.is_aligned() {
        return Err(LUNETERM_ERR_INVALID_ARGUMENT);
    }
    // SAFETY: non-null and aligned (checked above), writable by the caller's contract. `write`
    // does not read or drop the old (possibly uninitialized) value.
    unsafe {
        out.write(zero);
        Ok(&mut *out)
    }
}

/// Reads an input struct pointer (null → `ERR_NULL_POINTER`, misaligned → `ERR_INVALID_ARGUMENT`).
///
/// # Safety
///
/// `ptr` is NULL, misaligned, or valid for a read of an initialized `T`.
pub(crate) unsafe fn ref_in<T: Copy>(ptr: *const T) -> Result<T, i32> {
    if ptr.is_null() {
        return Err(LUNETERM_ERR_NULL_POINTER);
    }
    if !ptr.is_aligned() {
        return Err(LUNETERM_ERR_INVALID_ARGUMENT);
    }
    // SAFETY: non-null and aligned (checked above), readable by the caller's contract. Every
    // field of the ABI's input structs is an integer or a pointer, so any bit pattern is valid.
    Ok(unsafe { ptr.read() })
}

/// An input byte range (R3): `(NULL, 0)` and `(p, 0)` are empty, `(NULL, n > 0)` is
/// `ERR_NULL_POINTER`, a length over `isize::MAX` is `ERR_INVALID_ARGUMENT`.
///
/// # Safety
///
/// When `len > 0` and `ptr` is non-null, `ptr` is valid for reads of `len` bytes for `'a`.
pub(crate) unsafe fn bytes_in<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], i32> {
    if len == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() {
        return Err(LUNETERM_ERR_NULL_POINTER);
    }
    if isize::try_from(len).is_err() {
        return Err(LUNETERM_ERR_INVALID_ARGUMENT);
    }
    // SAFETY: non-null, `u8` has alignment 1, `len <= isize::MAX` (checked above); readable for
    // `len` bytes by the caller's contract.
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// Collapses a body result into a status code.
pub(crate) fn status(result: Result<(), i32>) -> i32 {
    match result {
        Ok(()) => LUNETERM_OK,
        Err(code) => code,
    }
}

/// Test-only: take the handle's lock from the test, as an in-flight call would hold it.
#[cfg(test)]
pub(crate) fn lock_for_test(engine: &LunetermEngine) -> std::sync::MutexGuard<'_, Inner> {
    match engine.0.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
