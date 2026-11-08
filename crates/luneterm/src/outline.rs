//! The running-outline policy (plan M10 step 1): when the busy cue should show, and the tracking
//! of the two command signals it needs.
//!
//! [`should_show`] is a port of Lunegit's `TerminalRunningOutlineGate.ShouldShowRunningOutline`
//! (`Lunegit.Core/Settings/TerminalRunningOutlineGate.cs`): pure and total. [`CommandTracker`] is
//! what Lunegit's `PtyTerminalSession` does around it: `command_running` follows the OSC 133
//! marks (`C` seen, no `D` or `A` since), and `interactive` is "mode 2026 was used since `C`, or
//! the user typed while the PTY was not canonical" (a program that consumes keystrokes char by
//! char). It runs on the session's engine thread, which owns the PTY and can probe it; the UI
//! gets the result as `Update::command`.

use lune_pty::RawMode;
use lune_vt::{CommandMarkKind, Progress, ProgressState};

/// What the marks say about the running command (inputs of [`should_show`]). A session starts
/// at the default: no command running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommandState {
    /// OSC 133 `C` seen, no `D` or `A` since.
    pub running: bool,
    /// The running command repaints its own region (mode 2026 since `C`) or consumes typed
    /// input (the PTY was not canonical when the user typed).
    pub interactive: bool,
}

/// Whether the running-outline cue should show.
///
/// OSC 9;4 progress is an independent trigger ahead of both suppression terms; the
/// full-screen-apps opt-in (`animate_in_full_screen_apps`) bypasses both of them.
pub fn should_show(
    progress_active: bool,
    command_running: bool,
    alt_screen: bool,
    animate_in_full_screen_apps: bool,
    interactive: bool,
) -> bool {
    progress_active
        || (command_running && (animate_in_full_screen_apps || (!alt_screen && !interactive)))
}

/// Whether an OSC 9;4 report is an active one (any state 1-4; state 0 removes it).
pub fn progress_active(progress: &Progress) -> bool {
    progress.state != ProgressState::None
}

/// Tracks [`CommandState`] from the marks, the 2026 signal and the user's typing.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandTracker {
    running: bool,
    sync_since_start: bool,
    captured_by_app: bool,
}

impl CommandTracker {
    pub fn new() -> CommandTracker {
        CommandTracker::default()
    }

    /// An OSC 133 mark. `C` starts a command and clears the per-command signals; `D` and `A`
    /// end it (the signals stay until the next `C`, as in Lunegit; the gate ignores them while
    /// nothing runs).
    pub fn mark(&mut self, kind: CommandMarkKind) {
        match kind {
            CommandMarkKind::OutputStart => {
                self.running = true;
                self.sync_since_start = false;
                self.captured_by_app = false;
            }
            CommandMarkKind::CommandFinished | CommandMarkKind::PromptStart => {
                self.running = false;
            }
            CommandMarkKind::PromptEnd => {}
        }
    }

    /// The engine reported mode 2026 inside the running command.
    pub fn sync_update(&mut self) {
        self.sync_since_start = true;
    }

    /// The caller must probe the PTY before [`CommandTracker::user_input`]: until the program is
    /// seen consuming keystrokes the answer can change with each one, and after that it cannot.
    pub fn needs_raw_probe(&self) -> bool {
        !self.captured_by_app
    }

    /// The user typed. `raw` is the PTY's line discipline just before the input was queued.
    /// `Unknown` counts as consuming (Windows, a failed probe): the rule degrades open, as
    /// Lunegit's does.
    pub fn user_input(&mut self, raw: RawMode) {
        if raw != RawMode::Cooked {
            self.captured_by_app = true;
        }
    }

    pub fn state(&self) -> CommandState {
        CommandState {
            running: self.running,
            interactive: self.sync_since_start || self.captured_by_app,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lunegit's `TerminalRunningOutlineGateTests.Cases`, every row, with literal expected
    /// values (not derived from the expression). Columns: progress, running, alt screen,
    /// animate full screen, interactive, expected.
    #[rustfmt::skip]
    const CASES: [(bool, bool, bool, bool, bool, bool); 32] = [
        // progress: an independent trigger, always shows.
        (true,  true,  true,  true,  true,  true),
        (true,  true,  true,  true,  false, true),
        (true,  true,  true,  false, true,  true),
        (true,  true,  true,  false, false, true),
        (true,  true,  false, true,  true,  true),
        (true,  true,  false, true,  false, true),
        (true,  true,  false, false, true,  true),
        (true,  true,  false, false, false, true),
        (true,  false, true,  true,  true,  true),
        (true,  false, true,  true,  false, true),
        (true,  false, true,  false, true,  true),
        (true,  false, true,  false, false, true),
        (true,  false, false, true,  true,  true),
        (true,  false, false, true,  false, true),
        (true,  false, false, false, true,  true),
        (true,  false, false, false, false, true),
        // no progress, command running: suppressed by alt screen or interactive unless the
        // setting is on (which bypasses both).
        (false, true,  true,  true,  true,  true),
        (false, true,  true,  true,  false, true),
        (false, true,  true,  false, true,  false),
        (false, true,  true,  false, false, false),
        (false, true,  false, true,  true,  true),
        (false, true,  false, true,  false, true),
        (false, true,  false, false, true,  false),
        (false, true,  false, false, false, true),
        // no progress, nothing running: never shows.
        (false, false, true,  true,  true,  false),
        (false, false, true,  true,  false, false),
        (false, false, true,  false, true,  false),
        (false, false, true,  false, false, false),
        (false, false, false, true,  true,  false),
        (false, false, false, true,  false, false),
        (false, false, false, false, true,  false),
        (false, false, false, false, false, false),
    ];

    #[test]
    fn the_gate_matches_lunegits_truth_table() {
        for (i, &(progress, running, alt, full, interactive, expected)) in CASES.iter().enumerate()
        {
            assert_eq!(
                should_show(progress, running, alt, full, interactive),
                expected,
                "row {i}: progress={progress} running={running} alt={alt} full={full} \
                 interactive={interactive}"
            );
        }
    }

    #[test]
    fn progress_is_active_for_states_1_to_4_only() {
        let at = |state| progress_active(&Progress { state, percent: 0 });
        assert!(!at(ProgressState::None));
        assert!(at(ProgressState::Normal));
        assert!(at(ProgressState::Error));
        assert!(at(ProgressState::Indeterminate));
        assert!(at(ProgressState::Warning));
    }

    #[test]
    fn running_follows_c_then_d_or_a() {
        let mut t = CommandTracker::new();
        assert_eq!(t.state(), CommandState::default());
        t.mark(CommandMarkKind::PromptStart);
        t.mark(CommandMarkKind::PromptEnd);
        assert!(!t.state().running);
        t.mark(CommandMarkKind::OutputStart);
        assert!(t.state().running);
        t.mark(CommandMarkKind::PromptEnd);
        assert!(t.state().running, "B does not end a command");
        t.mark(CommandMarkKind::CommandFinished);
        assert!(!t.state().running);
        t.mark(CommandMarkKind::OutputStart);
        t.mark(CommandMarkKind::PromptStart);
        assert!(!t.state().running, "A ends it too");
    }

    #[test]
    fn a_2026_signal_makes_the_command_interactive_until_the_next_c() {
        let mut t = CommandTracker::new();
        t.mark(CommandMarkKind::OutputStart);
        assert!(!t.state().interactive);
        t.sync_update();
        assert!(t.state().interactive);
        t.mark(CommandMarkKind::CommandFinished);
        assert!(
            t.state().interactive,
            "kept until the next C, as in Lunegit"
        );
        t.mark(CommandMarkKind::OutputStart);
        assert!(!t.state().interactive, "C clears it");
    }

    #[test]
    fn typing_counts_only_when_the_pty_is_not_canonical() {
        let mut t = CommandTracker::new();
        t.mark(CommandMarkKind::OutputStart);
        // `dotnet build`-style: typing into a cooked PTY only queues for the next command.
        assert!(t.needs_raw_probe());
        t.user_input(RawMode::Cooked);
        assert!(!t.state().interactive);
        assert!(t.needs_raw_probe(), "still probing: the mode can change");
        t.user_input(RawMode::Raw);
        assert!(t.state().interactive);
        assert!(!t.needs_raw_probe(), "sticky once seen");
        t.user_input(RawMode::Cooked);
        assert!(
            t.state().interactive,
            "a dip back to canonical does not flicker the cue"
        );
        t.mark(CommandMarkKind::OutputStart);
        assert!(!t.state().interactive);
        assert!(t.needs_raw_probe());
    }

    #[test]
    fn an_unknown_mode_counts_as_consuming_the_typing() {
        let mut t = CommandTracker::new();
        t.mark(CommandMarkKind::OutputStart);
        t.user_input(RawMode::Unknown);
        assert!(t.state().interactive);
    }
}
