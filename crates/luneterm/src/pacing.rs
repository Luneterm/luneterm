//! Frame pacing: which present mode the window asks for and how it is resolved (plan
//! render-performance D3, D8). The frame pacer itself joins this module in a later step.

/// The environment variable that picks the present mode for a run (`fifo` or `novsync`).
pub const PRESENT_VAR: &str = "LUNETERM_PRESENT";

/// What the app asks of the surface: wait for the display, or do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentAsk {
    /// `Fifo`: presents wait for the display.
    Fifo,
    /// The first of `Immediate`, `Mailbox` the surface lists, else `Fifo`.
    NoVsync,
}

impl PresentAsk {
    /// The ask's name as the trace line prints it.
    pub fn name(self) -> &'static str {
        match self {
            PresentAsk::Fifo => "Fifo",
            PresentAsk::NoVsync => "NoVsync",
        }
    }
}

/// What the app asks for when `LUNETERM_PRESENT` is unset.
pub const DEFAULT_PRESENT: PresentAsk = PresentAsk::Fifo;

/// Reads [`PRESENT_VAR`]'s value: unset is `None` (the default), `fifo` and `novsync` are the two
/// asks. Anything else is an error that names the variable and nothing else.
pub fn present_from_env(value: Option<&str>) -> Result<Option<PresentAsk>, String> {
    match value {
        None => Ok(None),
        Some("fifo") => Ok(Some(PresentAsk::Fifo)),
        Some("novsync") => Ok(Some(PresentAsk::NoVsync)),
        Some(_) => Err(format!("{PRESENT_VAR} must be `fifo` or `novsync`")),
    }
}

/// The concrete present mode for `ask` on a surface that lists `supported`: `Fifo` for a `Fifo`
/// ask, else the first of `Immediate`, `Mailbox` the surface lists, else `Fifo` (every surface
/// offers it).
pub fn resolve(ask: PresentAsk, supported: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
    match ask {
        PresentAsk::Fifo => Fifo,
        PresentAsk::NoVsync => [Immediate, Mailbox]
            .into_iter()
            .find(|m| supported.contains(m))
            .unwrap_or(Fifo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::PresentMode::{AutoNoVsync, Fifo, FifoRelaxed, Immediate, Mailbox};

    #[test]
    fn env_unset_is_the_default() {
        assert_eq!(present_from_env(None), Ok(None));
    }

    #[test]
    fn env_knows_two_words() {
        assert_eq!(present_from_env(Some("fifo")), Ok(Some(PresentAsk::Fifo)));
        assert_eq!(
            present_from_env(Some("novsync")),
            Ok(Some(PresentAsk::NoVsync))
        );
    }

    #[test]
    fn env_anything_else_is_an_error_naming_only_the_variable() {
        for bad in ["", "FIFO", "mailbox", "fifo ", "secret-value"] {
            let e = present_from_env(Some(bad)).expect_err("rejected");
            assert!(e.contains(PRESENT_VAR), "{e}");
        }
        let e = present_from_env(Some("secret-value")).expect_err("rejected");
        assert!(!e.contains("secret-value"), "{e}");
    }

    #[test]
    fn novsync_takes_immediate_then_mailbox_then_fifo() {
        assert_eq!(resolve(PresentAsk::NoVsync, &[Fifo, Immediate]), Immediate);
        assert_eq!(
            resolve(PresentAsk::NoVsync, &[Fifo, Mailbox, Immediate]),
            Immediate
        );
        assert_eq!(resolve(PresentAsk::NoVsync, &[Fifo, Mailbox]), Mailbox);
        assert_eq!(resolve(PresentAsk::NoVsync, &[Fifo]), Fifo);
        assert_eq!(resolve(PresentAsk::NoVsync, &[Fifo, FifoRelaxed]), Fifo);
        assert_eq!(resolve(PresentAsk::NoVsync, &[]), Fifo);
    }

    #[test]
    fn fifo_ask_is_fifo_whatever_the_surface_lists() {
        for caps in [&[Fifo][..], &[Fifo, Immediate, Mailbox], &[Immediate], &[]] {
            assert_eq!(resolve(PresentAsk::Fifo, caps), Fifo);
        }
        // Never an Auto* mode: the result is always concrete.
        assert_ne!(
            resolve(PresentAsk::NoVsync, &[AutoNoVsync, Fifo]),
            AutoNoVsync
        );
    }
}
