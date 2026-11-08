//! Lifecycle and control exports (M8 plan step 2): new, free, write, resize, size, scroll, idle
//! and the two gates. Every export follows the protocol in [`crate::guard`].

use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::TryLockError;

use lune_vt::services::options_service::OptionValue;
use lune_vt::{ExtensionError, StandardExtensions, Terminal};

use crate::guard::{
    Inner, LUNETERM_ERR_BUSY, LUNETERM_ERR_INVALID_ARGUMENT, LUNETERM_ERR_INVALID_UTF8,
    LUNETERM_ERR_PANIC, LUNETERM_ERR_TOO_LARGE, LUNETERM_OK, LunetermEngine, bytes_in,
    discard_payload, out_init, ref_in, status, with_engine,
};
use crate::types::{
    LUNETERM_MAX_COLS, LUNETERM_MAX_ROWS, LUNETERM_MAX_SCROLLBACK, LUNETERM_UNICODE_6,
    LUNETERM_UNICODE_11, LUNETERM_UNICODE_11_VS16, LunetermConfig,
};

/// Creates an engine from `config` and stores the handle in `*out` (`*out = NULL` on every
/// error). The standard extensions are always installed; no logger is installed.
///
/// # Safety
///
/// `config` is NULL or points to a readable `LunetermConfig` whose `xtversion` is readable for
/// `xtversion_len` bytes (when that is nonzero); `out` is NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_new(
    config: *const LunetermConfig,
    out: *mut *mut LunetermEngine,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `out`.
    let out = match unsafe { out_init(out, std::ptr::null_mut()) } {
        Ok(out) => out,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `config`.
    let config = match unsafe { ref_in(config) } {
        Ok(config) => config,
        Err(code) => return code,
    };
    let Ok(xtversion_len) = usize::try_from(config.xtversion_len) else {
        return LUNETERM_ERR_INVALID_ARGUMENT;
    };
    // SAFETY: forwarded from this function's contract on `config.xtversion`.
    let xtversion = match unsafe { bytes_in(config.xtversion, xtversion_len) } {
        Ok(bytes) => bytes,
        Err(code) => return code,
    };
    let settings = match Settings::check(&config, xtversion) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    match catch_unwind(AssertUnwindSafe(|| settings.build())) {
        Ok(Ok(inner)) => {
            *out = Box::into_raw(Box::new(LunetermEngine::new(inner)));
            LUNETERM_OK
        }
        Ok(Err(code)) => code,
        Err(payload) => {
            discard_payload(payload);
            LUNETERM_ERR_PANIC
        }
    }
}

/// A config that passed the checks that need no engine.
struct Settings {
    cols: usize,
    rows: usize,
    scrollback: usize,
    unicode: u32,
    extensions: StandardExtensions,
}

impl Settings {
    fn check(config: &LunetermConfig, xtversion: &[u8]) -> Result<Settings, i32> {
        if config.cols > LUNETERM_MAX_COLS
            || config.rows > LUNETERM_MAX_ROWS
            || config.scrollback > LUNETERM_MAX_SCROLLBACK
        {
            return Err(LUNETERM_ERR_INVALID_ARGUMENT);
        }
        if !matches!(
            config.unicode,
            LUNETERM_UNICODE_6 | LUNETERM_UNICODE_11 | LUNETERM_UNICODE_11_VS16
        ) {
            return Err(LUNETERM_ERR_INVALID_ARGUMENT);
        }
        let xtversion = if xtversion.is_empty() {
            None
        } else {
            match std::str::from_utf8(xtversion) {
                Ok(name) => Some(name.to_owned()),
                Err(_) => return Err(LUNETERM_ERR_INVALID_UTF8),
            }
        };
        Ok(Settings {
            cols: widen(config.cols).max(2),
            rows: widen(config.rows).max(1),
            scrollback: widen(config.scrollback),
            unicode: config.unicode,
            extensions: StandardExtensions {
                xtversion,
                vs16: config.unicode == LUNETERM_UNICODE_11_VS16,
                clipboard_write: config.clipboard_write != 0,
            },
        })
    }

    fn build(self) -> Result<Inner, i32> {
        let mut term = Terminal::new([
            OptionValue::Cols(self.cols),
            OptionValue::Rows(self.rows),
            OptionValue::Scrollback(self.scrollback),
        ]);
        match term.install_standard_extensions(&self.extensions) {
            Ok(()) => {}
            Err(
                ExtensionError::InvalidXtversion
                | ExtensionError::Identifier(_)
                | ExtensionError::AlreadyInstalled,
            ) => return Err(LUNETERM_ERR_INVALID_ARGUMENT),
        }
        if self.unicode == LUNETERM_UNICODE_11
            && term.unicode_mut().set_active_version("11").is_err()
        {
            return Err(LUNETERM_ERR_INVALID_ARGUMENT);
        }
        // `Terminal::new` silently drops an option that fails validation: read the values back.
        if term.cols() != self.cols
            || term.rows() != self.rows
            || term.options().scrollback != self.scrollback
        {
            return Err(LUNETERM_ERR_INVALID_ARGUMENT);
        }
        Ok(Inner::new(term))
    }
}

/// `u32` to `usize` (the crate builds for 64-bit targets only).
fn widen(value: u32) -> usize {
    value as usize
}

/// Frees an engine. NULL is OK. A handle with a call in progress is `ERR_BUSY` and not freed;
/// a poisoned handle is freed. A panic while dropping is `ERR_PANIC` and the memory is leaked.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle from `luneterm_engine_new`. After `OK` (or
/// `ERR_PANIC`) it must not be used again. Freeing while another thread may *start* a call on
/// the same handle is a host bug no check can catch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_free(engine: *mut LunetermEngine) -> i32 {
    if engine.is_null() {
        return LUNETERM_OK;
    }
    if !engine.is_aligned() {
        return LUNETERM_ERR_INVALID_ARGUMENT;
    }
    {
        // SAFETY: non-null and aligned (checked above), live by the caller's contract. Shared
        // reference only; it ends with this block, before the handle is dropped.
        let shared: &LunetermEngine = unsafe { &*engine };
        // Poisoned or free: the guard (if any) is released at the end of the statement.
        if let Err(TryLockError::WouldBlock) = shared.0.try_lock() {
            return LUNETERM_ERR_BUSY;
        }
    }
    // SAFETY: from `Box::into_raw` in `luneterm_engine_new`, not freed before (caller's
    // contract), and no call holds it (checked above). `drop_in_place` runs the destructor
    // without freeing the allocation, so a panic in it leaves the memory leaked, not freed.
    let dropped = catch_unwind(AssertUnwindSafe(|| unsafe {
        std::ptr::drop_in_place(engine)
    }));
    match dropped {
        Ok(()) => {
            // SAFETY: the same `Box` allocation; its contents are already dropped, and
            // `ManuallyDrop<T>` has `T`'s layout, so this only deallocates.
            drop(unsafe { Box::from_raw(engine.cast::<ManuallyDrop<LunetermEngine>>()) });
            LUNETERM_OK
        }
        Err(payload) => {
            discard_payload(payload);
            LUNETERM_ERR_PANIC
        }
    }
}

/// Feeds PTY output to the engine: any bytes, split UTF-8 and split sequences allowed.
/// `(NULL, 0)` is OK. Events queue until drained.
///
/// # Safety
///
/// `engine` as for every handle export; `data` is readable for `len` bytes when `len > 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_write(
    engine: *mut LunetermEngine,
    data: *const u8,
    len: usize,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `data`.
    let data = match unsafe { bytes_in(data, len) } {
        Ok(data) => data,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    status(unsafe {
        with_engine(engine, |inner| {
            inner.term.write(data);
            Ok(())
        })
    })
}

/// Resizes the grid. Over a cap: `ERR_INVALID_ARGUMENT` and nothing changes; below 2 x 1 is
/// clamped.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_resize(
    engine: *mut LunetermEngine,
    cols: u32,
    rows: u32,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `engine`.
    status(unsafe {
        with_engine(engine, |inner| {
            if cols > LUNETERM_MAX_COLS || rows > LUNETERM_MAX_ROWS {
                return Err(LUNETERM_ERR_INVALID_ARGUMENT);
            }
            inner.term.resize(widen(cols), widen(rows));
            Ok(())
        })
    })
}

/// The grid size. Both out-params are zeroed first and filled only on `OK`.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `cols` and `rows` are NULL, misaligned, or
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_size(
    engine: *mut LunetermEngine,
    cols: *mut u32,
    rows: *mut u32,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `cols` and `rows`.
    let (cols, rows) = match unsafe { (out_init(cols, 0), out_init(rows, 0)) } {
        (Ok(cols), Ok(rows)) => (cols, rows),
        (Err(code), _) | (_, Err(code)) => return code,
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    let size = unsafe {
        with_engine(engine, |inner| {
            match (
                u32::try_from(inner.term.cols()),
                u32::try_from(inner.term.rows()),
            ) {
                (Ok(c), Ok(r)) => Ok((c, r)),
                _ => Err(LUNETERM_ERR_TOO_LARGE),
            }
        })
    };
    match size {
        Ok((c, r)) => {
            *cols = c;
            *rows = r;
            LUNETERM_OK
        }
        Err(code) => code,
    }
}

/// Scrolls the viewport by `delta` lines (negative = up).
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_scroll_lines(
    engine: *mut LunetermEngine,
    delta: i32,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `engine`.
    status(unsafe {
        with_engine(engine, |inner| {
            inner.term.scroll_lines(i64::from(delta));
            Ok(())
        })
    })
}

/// Scrolls the viewport to the top of the scrollback.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_scroll_to_top(engine: *mut LunetermEngine) -> i32 {
    // SAFETY: forwarded from this function's contract on `engine`.
    status(unsafe {
        with_engine(engine, |inner| {
            inner.term.scroll_to_top();
            Ok(())
        })
    })
}

/// Scrolls the viewport to the bottom.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_scroll_to_bottom(engine: *mut LunetermEngine) -> i32 {
    // SAFETY: forwarded from this function's contract on `engine`.
    status(unsafe {
        with_engine(engine, |inner| {
            inner.term.scroll_to_bottom();
            Ok(())
        })
    })
}

/// Runs one batch of deferred memory cleanup; `*more = 1` when work remains (call again while
/// the host is idle).
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle; `more` is NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_idle(engine: *mut LunetermEngine, more: *mut u8) -> i32 {
    // SAFETY: forwarded from this function's contract on `more`.
    let more = match unsafe { out_init(more, 0) } {
        Ok(more) => more,
        Err(code) => return code,
    };
    // SAFETY: forwarded from this function's contract on `engine`.
    match unsafe { with_engine(engine, |inner| Ok(inner.term.idle())) } {
        Ok(remaining) => {
            *more = u8::from(remaining);
            LUNETERM_OK
        }
        Err(code) => code,
    }
}

/// Turns the OSC 52 clipboard-write gate on (nonzero) or off.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_set_clipboard_write(
    engine: *mut LunetermEngine,
    enabled: u8,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `engine`.
    status(unsafe {
        with_engine(engine, |inner| {
            inner.term.set_clipboard_write(enabled != 0);
            Ok(())
        })
    })
}

/// Clears the kitty keyboard stacks and the modifyOtherKeys level (Lunegit ledger row 11).
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_engine_reset_keyboard_protocol(
    engine: *mut LunetermEngine,
) -> i32 {
    // SAFETY: forwarded from this function's contract on `engine`.
    status(unsafe {
        with_engine(engine, |inner| {
            inner.term.reset_keyboard_protocol();
            Ok(())
        })
    })
}

/// Test only (feature `panic-probe`, never in a shipping build): panics inside the call guard,
/// so the host can check that the panic is caught (`ERR_PANIC`) and the handle poisoned.
///
/// # Safety
///
/// `engine` is NULL, misaligned, or a live handle.
#[cfg(feature = "panic-probe")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luneterm_test_panic(engine: *mut LunetermEngine) -> i32 {
    // SAFETY: forwarded from this function's contract on `engine`.
    status(unsafe {
        with_engine(engine, |_| -> Result<(), i32> {
            panic!("luneterm_test_panic")
        })
    })
}

#[cfg(test)]
mod tests {
    //! The exports called through raw pointers, as a C host calls them.

    use std::ptr::{null, null_mut};
    use std::sync::mpsc;

    use lune_vt::HostEvent;

    use super::*;
    use crate::guard::{LUNETERM_ERR_NULL_POINTER, lock_for_test};
    use crate::{
        LunetermDamage, LunetermEvents, LunetermModes, LunetermViewport,
        luneterm_engine_drain_events, luneterm_engine_modes, luneterm_engine_take_damage,
        luneterm_engine_viewport,
    };

    fn config(cols: u32, rows: u32) -> LunetermConfig {
        LunetermConfig {
            cols,
            rows,
            scrollback: 0,
            unicode: LUNETERM_UNICODE_6,
            xtversion: null(),
            xtversion_len: 0,
            clipboard_write: 0,
            reserved: [0; 7],
        }
    }

    /// `luneterm_engine_new`, checking that `*out` is NULL on every error.
    fn try_new(config: &LunetermConfig) -> Result<*mut LunetermEngine, i32> {
        let mut out = std::ptr::dangling_mut::<LunetermEngine>();
        // SAFETY: both pointers come from live references.
        let code = unsafe { luneterm_engine_new(config, &mut out) };
        if code == LUNETERM_OK {
            assert!(!out.is_null());
            Ok(out)
        } else {
            assert!(out.is_null(), "*out must be NULL on error {code}");
            Err(code)
        }
    }

    fn new_engine(cols: u32, rows: u32) -> *mut LunetermEngine {
        try_new(&config(cols, rows)).expect("engine")
    }

    fn free(engine: *mut LunetermEngine) {
        // SAFETY: a live handle from `new_engine`, not used afterwards.
        assert_eq!(unsafe { luneterm_engine_free(engine) }, LUNETERM_OK);
    }

    fn size(engine: *mut LunetermEngine) -> (u32, u32) {
        let (mut cols, mut rows) = (u32::MAX, u32::MAX);
        // SAFETY: a live handle and two live out-params.
        let code = unsafe { luneterm_engine_size(engine, &mut cols, &mut rows) };
        assert_eq!(code, LUNETERM_OK);
        (cols, rows)
    }

    fn write(engine: *mut LunetermEngine, data: &[u8]) -> i32 {
        // SAFETY: a live handle (or one a test made invalid on purpose) and a live slice.
        unsafe { luneterm_engine_write(engine, data.as_ptr(), data.len()) }
    }

    /// Runs `f` on the engine behind a live handle, under its lock.
    fn with_inner<R>(engine: *mut LunetermEngine, f: impl FnOnce(&mut Inner) -> R) -> R {
        // SAFETY: a live handle; shared reference, as the exports take it.
        let shared = unsafe { &*engine };
        f(&mut lock_for_test(shared))
    }

    /// The handle moved by one byte: never dereferenced, only checked.
    fn misaligned<T>(base: *mut T) -> *mut T {
        base.cast::<u8>().wrapping_add(1).cast::<T>()
    }

    fn assert_usable(engine: *mut LunetermEngine) {
        assert_eq!(write(engine, b"ok"), LUNETERM_OK);
        let _ = size(engine);
    }

    /// Every handle export on `engine`, with valid other arguments.
    fn every_handle_export(engine: *mut LunetermEngine) -> Vec<(&'static str, i32)> {
        let (mut cols, mut rows, mut more) = (7u32, 7u32, 7u8);
        let mut viewport = LunetermViewport {
            cols: 7,
            ..LunetermViewport::EMPTY
        };
        let mut events = LunetermEvents {
            count: 7,
            ..LunetermEvents::EMPTY
        };
        let mut damage = LunetermDamage {
            first_row: 7,
            ..LunetermDamage::default()
        };
        let mut modes = LunetermModes {
            wraparound: 7,
            ..LunetermModes::default()
        };
        // SAFETY: the handle is whatever the test passes (NULL, misaligned or live); every other
        // pointer is a live local.
        #[cfg_attr(not(feature = "panic-probe"), allow(unused_mut))]
        let mut codes = unsafe {
            vec![
                ("write", luneterm_engine_write(engine, b"x".as_ptr(), 1)),
                ("resize", luneterm_engine_resize(engine, 10, 5)),
                ("size", luneterm_engine_size(engine, &mut cols, &mut rows)),
                ("scroll_lines", luneterm_engine_scroll_lines(engine, -1)),
                ("scroll_to_top", luneterm_engine_scroll_to_top(engine)),
                ("scroll_to_bottom", luneterm_engine_scroll_to_bottom(engine)),
                ("idle", luneterm_engine_idle(engine, &mut more)),
                (
                    "set_clipboard_write",
                    luneterm_engine_set_clipboard_write(engine, 1),
                ),
                (
                    "reset_keyboard_protocol",
                    luneterm_engine_reset_keyboard_protocol(engine),
                ),
                ("viewport", luneterm_engine_viewport(engine, &mut viewport)),
                (
                    "drain_events",
                    luneterm_engine_drain_events(engine, &mut events),
                ),
                (
                    "take_damage",
                    luneterm_engine_take_damage(engine, &mut damage),
                ),
                ("modes", luneterm_engine_modes(engine, &mut modes)),
            ]
        };
        let failed = codes.iter().all(|(_, code)| *code != LUNETERM_OK);
        if failed {
            assert_eq!((cols, rows, more), (0, 0, 0), "out-params zeroed on error");
            assert_eq!(
                (
                    viewport.cols,
                    events.count,
                    damage.first_row,
                    modes.wraparound
                ),
                (0, 0, 0, 0),
                "output structs zeroed on error"
            );
            assert!(viewport.cells.is_null() && events.events.is_null());
        }
        #[cfg(feature = "panic-probe")]
        if engine.is_null() || !engine.is_aligned() {
            // SAFETY: NULL or misaligned, so the probe never reaches the engine.
            codes.push(("test_panic", unsafe { crate::luneterm_test_panic(engine) }));
        }
        codes
    }

    #[test]
    fn a_null_handle_is_a_null_pointer_error_for_every_export() {
        for (name, code) in every_handle_export(null_mut()) {
            assert_eq!(code, LUNETERM_ERR_NULL_POINTER, "{name}");
        }
        // SAFETY: NULL is a valid argument to free.
        assert_eq!(unsafe { luneterm_engine_free(null_mut()) }, LUNETERM_OK);
    }

    #[test]
    fn a_misaligned_handle_is_an_invalid_argument_and_the_real_one_stays_usable() {
        let engine = new_engine(80, 24);
        let bad = misaligned(engine);
        for (name, code) in every_handle_export(bad) {
            assert_eq!(code, LUNETERM_ERR_INVALID_ARGUMENT, "{name}");
        }
        // SAFETY: misaligned, so free refuses it without touching it.
        assert_eq!(
            unsafe { luneterm_engine_free(bad) },
            LUNETERM_ERR_INVALID_ARGUMENT
        );
        assert_usable(engine);
        free(engine);
    }

    #[test]
    fn a_live_handle_accepts_every_export() {
        let engine = new_engine(80, 24);
        for (name, code) in every_handle_export(engine) {
            assert_eq!(code, LUNETERM_OK, "{name}");
        }
        assert_eq!(size(engine), (10, 5));
        free(engine);
    }

    #[test]
    fn size_out_params_null_or_misaligned_are_refused_and_zeroed() {
        let engine = new_engine(80, 24);
        let mut buf = [u32::MAX; 4];
        let (mut cols, mut rows) = (9u32, 9u32);
        // SAFETY: a live handle; out-params are NULL, live locals, or a misaligned pointer
        // into `buf` that `size` must refuse before writing.
        unsafe {
            assert_eq!(
                luneterm_engine_size(engine, null_mut(), &mut rows),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(rows, 0);
            assert_eq!(
                luneterm_engine_size(engine, &mut cols, null_mut()),
                LUNETERM_ERR_NULL_POINTER
            );
            assert_eq!(cols, 0);
            let bad = misaligned(buf.as_mut_ptr());
            assert_eq!(
                luneterm_engine_size(engine, bad, &mut rows),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
            assert_eq!(
                luneterm_engine_size(engine, &mut cols, bad),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
        }
        assert_eq!(
            buf,
            [u32::MAX; 4],
            "a misaligned out-param is never written"
        );
        assert_usable(engine);
        assert_eq!(size(engine), (80, 24));
        free(engine);
    }

    #[test]
    fn idle_with_a_null_out_param_is_refused() {
        // `uint8_t*` has alignment 1: it cannot be misaligned.
        let engine = new_engine(80, 24);
        // SAFETY: a live handle and a NULL out-param.
        assert_eq!(
            unsafe { luneterm_engine_idle(engine, null_mut()) },
            LUNETERM_ERR_NULL_POINTER
        );
        assert_usable(engine);
        let mut more = 9u8;
        for _ in 0..1000 {
            // SAFETY: a live handle and a live out-param.
            assert_eq!(
                unsafe { luneterm_engine_idle(engine, &mut more) },
                LUNETERM_OK
            );
            if more == 0 {
                break;
            }
        }
        assert_eq!(more, 0);
        free(engine);
    }

    #[test]
    fn new_with_null_or_misaligned_pointers_is_refused_and_out_is_null() {
        let cfg = config(80, 24);
        let mut out = std::ptr::dangling_mut::<LunetermEngine>();
        // SAFETY: NULL config and a live out.
        assert_eq!(
            unsafe { luneterm_engine_new(null(), &mut out) },
            LUNETERM_ERR_NULL_POINTER
        );
        assert!(out.is_null());
        // SAFETY: a live config and a NULL out.
        assert_eq!(
            unsafe { luneterm_engine_new(&cfg, null_mut()) },
            LUNETERM_ERR_NULL_POINTER
        );
        let mut buf = [0u64; 8];
        let bad_cfg = misaligned(buf.as_mut_ptr()).cast::<LunetermConfig>();
        out = std::ptr::dangling_mut();
        // SAFETY: a misaligned config is refused before it is read.
        assert_eq!(
            unsafe { luneterm_engine_new(bad_cfg, &mut out) },
            LUNETERM_ERR_INVALID_ARGUMENT
        );
        assert!(out.is_null());
        let bad_out = misaligned(buf.as_mut_ptr()).cast::<*mut LunetermEngine>();
        // SAFETY: a misaligned out is refused before it is written.
        assert_eq!(
            unsafe { luneterm_engine_new(&cfg, bad_out) },
            LUNETERM_ERR_INVALID_ARGUMENT
        );
        assert_eq!(buf, [0; 8]);
    }

    #[test]
    fn write_null_with_zero_length_is_ok_and_with_a_length_is_refused() {
        let engine = new_engine(80, 24);
        // SAFETY: a live handle; NULL data is refused for len > 0 and never read for len 0;
        // the huge length is refused before the (live) pointer is read.
        unsafe {
            assert_eq!(luneterm_engine_write(engine, null(), 0), LUNETERM_OK);
            assert_eq!(
                luneterm_engine_write(engine, null(), 1),
                LUNETERM_ERR_NULL_POINTER
            );
            let huge = (isize::MAX as usize) + 1;
            assert_eq!(
                luneterm_engine_write(engine, b"x".as_ptr(), huge),
                LUNETERM_ERR_INVALID_ARGUMENT
            );
        }
        assert_usable(engine);
        free(engine);
    }

    #[test]
    fn new_size_caps_are_inclusive_and_over_a_cap_is_refused() {
        let ok = |cfg: LunetermConfig| {
            let engine = try_new(&cfg).expect("within the caps");
            let got = size(engine);
            let scrollback = with_inner(engine, |inner| inner.term.options().scrollback);
            free(engine);
            (got, scrollback)
        };
        assert_eq!(ok(config(4096, 1)).0, (4096, 1));
        assert_eq!(ok(config(2, 2048)).0, (2, 2048));
        let mut cfg = config(80, 24);
        cfg.scrollback = 100_000;
        assert_eq!(ok(cfg).1, 100_000);
        assert_eq!(
            try_new(&config(4097, 1)),
            Err(LUNETERM_ERR_INVALID_ARGUMENT)
        );
        assert_eq!(
            try_new(&config(2, 2049)),
            Err(LUNETERM_ERR_INVALID_ARGUMENT)
        );
        assert_eq!(
            try_new(&config(u32::MAX, u32::MAX)),
            Err(LUNETERM_ERR_INVALID_ARGUMENT)
        );
        cfg.scrollback = 100_001;
        assert_eq!(try_new(&cfg), Err(LUNETERM_ERR_INVALID_ARGUMENT));
        // Below 2 x 1 is clamped, as the engine does.
        assert_eq!(ok(config(0, 0)).0, (2, 1));
        assert_eq!(ok(config(1, 1)).0, (2, 1));
        assert_eq!(ok(config(3, 0)).0, (3, 1));
    }

    #[test]
    fn resize_caps_are_inclusive_and_over_a_cap_changes_nothing() {
        let engine = new_engine(80, 24);
        // SAFETY: a live handle.
        let resize = |cols, rows| unsafe { luneterm_engine_resize(engine, cols, rows) };
        assert_eq!(resize(4096, 1), LUNETERM_OK);
        assert_eq!(size(engine), (4096, 1));
        assert_eq!(resize(2, 2048), LUNETERM_OK);
        assert_eq!(size(engine), (2, 2048));
        assert_eq!(resize(80, 24), LUNETERM_OK);
        assert_eq!(resize(4097, 24), LUNETERM_ERR_INVALID_ARGUMENT);
        assert_eq!(resize(80, 2049), LUNETERM_ERR_INVALID_ARGUMENT);
        assert_eq!(resize(u32::MAX, u32::MAX), LUNETERM_ERR_INVALID_ARGUMENT);
        assert_eq!(size(engine), (80, 24));
        assert_eq!(resize(0, 0), LUNETERM_OK);
        assert_eq!(size(engine), (2, 1));
        assert_eq!(resize(1, 1), LUNETERM_OK);
        assert_eq!(size(engine), (2, 1));
        free(engine);
    }

    fn with_xtversion(bytes: *const u8, len: u64) -> Result<*mut LunetermEngine, i32> {
        let mut cfg = config(80, 24);
        cfg.xtversion = bytes;
        cfg.xtversion_len = len;
        try_new(&cfg)
    }

    fn xtversion_reply(engine: *mut LunetermEngine) -> Option<Vec<u8>> {
        assert_eq!(write(engine, b"\x1b[>q"), LUNETERM_OK);
        with_inner(engine, |inner| {
            inner
                .term
                .drain_events()
                .into_iter()
                .find_map(|event| match event {
                    HostEvent::Response(bytes) => Some(bytes),
                    _ => None,
                })
        })
    }

    fn xtversion(name: &[u8]) -> Result<*mut LunetermEngine, i32> {
        with_xtversion(name.as_ptr(), name.len() as u64)
    }

    #[test]
    fn xtversion_bytes_reach_lune_vt_as_given() {
        // Empty with a non-NULL pointer: unanswered, like NULL/0.
        let engine = with_xtversion(b"name".as_ptr(), 0).expect("empty");
        assert_eq!(xtversion_reply(engine), None);
        free(engine);
        let engine = with_xtversion(null(), 0).expect("NULL/0");
        assert_eq!(xtversion_reply(engine), None);
        free(engine);

        let name = [b'a'; 64];
        let engine = xtversion(&name).expect("64 bytes");
        let mut expected = b"\x1bP>|".to_vec();
        expected.extend_from_slice(&name);
        expected.extend_from_slice(b"\x1b\\");
        assert_eq!(xtversion_reply(engine), Some(expected));
        free(engine);

        assert_eq!(xtversion(&[b'a'; 65]), Err(LUNETERM_ERR_INVALID_ARGUMENT));
        assert_eq!(xtversion(b"ab\x7f"), Err(LUNETERM_ERR_INVALID_ARGUMENT));
        // A NUL is passed on (no truncation) and lune-vt refuses it.
        assert_eq!(xtversion(b"ab\0cd"), Err(LUNETERM_ERR_INVALID_ARGUMENT));
        assert_eq!(xtversion(b"ab\xffcd"), Err(LUNETERM_ERR_INVALID_UTF8));
        // Valid UTF-8 outside 0x20..=0x7E: lune-vt's refusal.
        assert_eq!(
            xtversion("caf\u{e9}".as_bytes()),
            Err(LUNETERM_ERR_INVALID_ARGUMENT)
        );
        assert_eq!(with_xtversion(null(), 3), Err(LUNETERM_ERR_NULL_POINTER));
    }

    /// Cursor column after writing `text` at the home position.
    fn width_of(engine: *mut LunetermEngine, text: &str) -> usize {
        assert_eq!(write(engine, b"\x1b[H\x1b[2J"), LUNETERM_OK);
        assert_eq!(write(engine, text.as_bytes()), LUNETERM_OK);
        with_inner(engine, |inner| inner.term.viewport().cursor.x)
    }

    #[test]
    fn unicode_selects_the_active_version() {
        let mut cfg = config(80, 24);
        let mut widths = Vec::new();
        for unicode in [
            LUNETERM_UNICODE_6,
            LUNETERM_UNICODE_11,
            LUNETERM_UNICODE_11_VS16,
        ] {
            cfg.unicode = unicode;
            let engine = try_new(&cfg).expect("unicode version");
            let version = with_inner(engine, |inner| {
                inner.term.unicode_mut().active_version().to_owned()
            });
            widths.push((
                version,
                width_of(engine, "\u{1F600}"),
                width_of(engine, "\u{2764}\u{FE0F}"),
            ));
            assert_eq!(size(engine), (80, 24));
            free(engine);
        }
        assert_eq!(
            widths,
            [
                ("6".to_owned(), 1, 1),
                ("11".to_owned(), 2, 1),
                ("11-vs16".to_owned(), 2, 2)
            ]
        );
        cfg.unicode = 3;
        assert_eq!(try_new(&cfg), Err(LUNETERM_ERR_INVALID_ARGUMENT));
        cfg.unicode = u32::MAX;
        assert_eq!(try_new(&cfg), Err(LUNETERM_ERR_INVALID_ARGUMENT));
    }

    fn clipboard_writes(engine: *mut LunetermEngine) -> usize {
        assert_eq!(write(engine, b"\x1b]52;c;aGk=\x07"), LUNETERM_OK);
        with_inner(engine, |inner| {
            inner
                .term
                .drain_events()
                .iter()
                .filter(|event| matches!(event, HostEvent::ClipboardWrite(_)))
                .count()
        })
    }

    #[test]
    fn clipboard_write_is_any_nonzero_byte_and_the_gate_flips() {
        let engine = new_engine(80, 24);
        assert_eq!(clipboard_writes(engine), 0, "all-zero config: off");
        free(engine);
        let mut cfg = config(80, 24);
        cfg.clipboard_write = 2;
        let engine = try_new(&cfg).expect("engine");
        assert_eq!(clipboard_writes(engine), 1);
        // SAFETY: a live handle.
        assert_eq!(
            unsafe { luneterm_engine_set_clipboard_write(engine, 0) },
            LUNETERM_OK
        );
        assert_eq!(clipboard_writes(engine), 0);
        // SAFETY: a live handle.
        assert_eq!(
            unsafe { luneterm_engine_set_clipboard_write(engine, 0xFF) },
            LUNETERM_OK
        );
        assert_eq!(clipboard_writes(engine), 1);
        free(engine);
    }

    #[test]
    fn scroll_exports_move_the_viewport() {
        let mut cfg = config(10, 5);
        cfg.scrollback = 100;
        let engine = try_new(&cfg).expect("engine");
        assert_eq!(write(engine, "x\r\n".repeat(30).as_bytes()), LUNETERM_OK);
        let top = || with_inner(engine, |inner| inner.term.viewport().viewport_top);
        let bottom = top();
        assert!(bottom >= 20);
        // SAFETY: a live handle.
        unsafe {
            assert_eq!(luneterm_engine_scroll_lines(engine, -3), LUNETERM_OK);
            assert_eq!(top(), bottom - 3);
            assert_eq!(luneterm_engine_scroll_lines(engine, 2), LUNETERM_OK);
            assert_eq!(top(), bottom - 1);
            assert_eq!(luneterm_engine_scroll_to_top(engine), LUNETERM_OK);
            assert_eq!(top(), 0);
            assert_eq!(luneterm_engine_scroll_to_bottom(engine), LUNETERM_OK);
            assert_eq!(top(), bottom);
            assert_eq!(luneterm_engine_scroll_lines(engine, i32::MIN), LUNETERM_OK);
            assert_eq!(top(), 0);
            assert_eq!(luneterm_engine_scroll_lines(engine, i32::MAX), LUNETERM_OK);
            assert_eq!(top(), bottom);
        }
        free(engine);
    }

    #[test]
    fn reset_keyboard_protocol_clears_the_kitty_flags() {
        let engine = new_engine(80, 24);
        assert_eq!(write(engine, b"\x1b[>1u"), LUNETERM_OK);
        let flags = || with_inner(engine, |inner| inner.term.modes().kitty_keyboard_flags);
        assert_eq!(flags(), 1);
        // SAFETY: a live handle.
        assert_eq!(
            unsafe { luneterm_engine_reset_keyboard_protocol(engine) },
            LUNETERM_OK
        );
        assert_eq!(flags(), 0);
        free(engine);
    }

    #[test]
    fn a_call_while_another_is_in_flight_is_busy_and_free_refuses() {
        let engine = new_engine(80, 24);
        // SAFETY: a live handle; shared reference, as the exports take it.
        let shared: &LunetermEngine = unsafe { &*engine };
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let _guard = lock_for_test(shared);
                locked_tx.send(()).expect("signal");
                release_rx.recv().expect("release");
            });
            locked_rx.recv().expect("locked");
            for (name, code) in every_handle_export(engine) {
                assert_eq!(code, LUNETERM_ERR_BUSY, "{name}");
            }
            // SAFETY: a live handle, which free must refuse while it is locked.
            assert_eq!(unsafe { luneterm_engine_free(engine) }, LUNETERM_ERR_BUSY);
            release_tx.send(()).expect("release");
        });
        assert_usable(engine);
        free(engine);
    }

    #[cfg(feature = "panic-probe")]
    #[test]
    fn a_caught_panic_poisons_the_handle_and_free_still_works() {
        use crate::guard::LUNETERM_ERR_POISONED;

        let engine = new_engine(80, 24);
        // SAFETY: a live handle.
        assert_eq!(
            unsafe { crate::luneterm_test_panic(engine) },
            LUNETERM_ERR_PANIC
        );
        assert_eq!(write(engine, b"x"), LUNETERM_ERR_POISONED);
        let (mut cols, mut rows) = (9u32, 9u32);
        // SAFETY: a live (poisoned) handle and live out-params.
        let code = unsafe { luneterm_engine_size(engine, &mut cols, &mut rows) };
        assert_eq!((code, cols, rows), (LUNETERM_ERR_POISONED, 0, 0));
        for (name, code) in every_handle_export(engine) {
            assert_eq!(code, LUNETERM_ERR_POISONED, "{name}");
        }
        // SAFETY: a live handle. Every export goes through this same guard (and the loop above
        // already covered viewport, drain_events, take_damage and modes).
        let guarded = unsafe { with_engine(engine, |_| Ok(())) };
        assert_eq!(guarded, Err(LUNETERM_ERR_POISONED));
        // SAFETY: a live handle.
        assert_eq!(
            unsafe { crate::luneterm_test_panic(engine) },
            LUNETERM_ERR_POISONED
        );
        free(engine);
    }
}
