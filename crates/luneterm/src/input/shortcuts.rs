//! The app's own shortcuts (plan M9 D4): matched at event time with the toolkit's `Chord`, before
//! a key reaches the key router. Everything else a terminal could want, Ctrl+C included, is the
//! terminal's.

use lune_ui::input::{Key, Keymap, Modifiers, NamedKey};
use lune_ui::shortcuts::Chord;

use crate::session::Scroll;

/// What an app shortcut asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    /// Paste the clipboard (the app reads it, then sends `Command::Paste`).
    Paste,
    /// Copy the selection. With none selected the key is the terminal's (Ctrl+Shift+C) or
    /// nobody's (Cmd+C): the app asks the table without this entry, [`AppShortcuts::without_copy`].
    Copy,
    /// Scroll the viewport locally.
    Scroll(Scroll),
    /// Open a tab (D13).
    NewTab,
    /// Close the active tab (asks first when a program runs).
    CloseTab,
    /// The next tab to the right, wrapping.
    NextTab,
    /// The next tab to the left, wrapping.
    PrevTab,
    /// The tab at this 0-based place (1..=9 on the keyboard).
    SelectTab(usize),
    /// Open the settings page (D13: Cmd+, on macOS, Ctrl+, elsewhere); leaves it when it is open.
    Settings,
}

impl AppCommand {
    /// Whether a held key must not repeat the command: a held chord would paste, open or close
    /// again and again.
    pub fn once_per_press(self) -> bool {
        matches!(
            self,
            AppCommand::Paste
                | AppCommand::Copy
                | AppCommand::NewTab
                | AppCommand::CloseTab
                | AppCommand::Settings
        )
    }
}

/// The shortcut table. A chord matches only with exactly its modifiers held.
#[derive(Debug, Clone)]
pub struct AppShortcuts {
    entries: Vec<(Chord, AppCommand)>,
}

impl AppShortcuts {
    /// Lunegit's table (TerminalView.cs:1151-1175, :1807-1810): Shift+PageUp/PageDown scroll a
    /// page, Shift+Home/End go to the top and bottom; paste is Cmd+V on macOS and Ctrl+Shift+V
    /// elsewhere (plain Ctrl+V is the shell's literal-next). The tab shortcuts are ours (D13:
    /// Lunegit only binds Meta+T): on macOS Cmd+T, Cmd+W, Cmd+Shift+[ and ], Cmd+1..9; elsewhere
    /// Ctrl+Shift+T, Ctrl+Shift+W, Ctrl+Tab, Ctrl+Shift+Tab, Alt+1..9.
    pub fn standard(macos: bool) -> AppShortcuts {
        let ch = |c: &str| Chord::key(Key::Character(c.into()));
        let v = ch("v");
        let paste = if macos { v.logo() } else { v.ctrl().shift() };
        let c = ch("c");
        let copy = if macos { c.logo() } else { c.ctrl().shift() };
        let shift = |k| Chord::key(Key::Named(k)).shift();
        let mut entries = vec![
            (paste, AppCommand::Paste),
            (copy, AppCommand::Copy),
            (
                shift(NamedKey::PageUp),
                AppCommand::Scroll(Scroll::Pages(-1)),
            ),
            (
                shift(NamedKey::PageDown),
                AppCommand::Scroll(Scroll::Pages(1)),
            ),
            (shift(NamedKey::Home), AppCommand::Scroll(Scroll::Top)),
            (shift(NamedKey::End), AppCommand::Scroll(Scroll::Bottom)),
        ];
        entries.push((
            if macos {
                ch(",").logo()
            } else {
                ch(",").ctrl()
            },
            AppCommand::Settings,
        ));
        if macos {
            entries.push((ch("t").logo(), AppCommand::NewTab));
            entries.push((ch("w").logo(), AppCommand::CloseTab));
            // Shift turns [ and ] into { and } on most layouts; the chord compares the text.
            for (key, cmd) in [
                ("[", AppCommand::PrevTab),
                ("{", AppCommand::PrevTab),
                ("]", AppCommand::NextTab),
                ("}", AppCommand::NextTab),
            ] {
                entries.push((ch(key).logo().shift(), cmd));
            }
            for n in 1..=9usize {
                entries.push((ch(&n.to_string()).logo(), AppCommand::SelectTab(n - 1)));
            }
        } else {
            entries.push((ch("t").ctrl().shift(), AppCommand::NewTab));
            entries.push((ch("w").ctrl().shift(), AppCommand::CloseTab));
            let tab = Chord::key(Key::Named(NamedKey::Tab));
            entries.push((tab.clone().ctrl(), AppCommand::NextTab));
            entries.push((tab.ctrl().shift(), AppCommand::PrevTab));
            for n in 1..=9usize {
                entries.push((ch(&n.to_string()).alt(), AppCommand::SelectTab(n - 1)));
            }
        }
        AppShortcuts { entries }
    }

    /// The same table without Copy: what a Copy chord means when nothing is selected.
    pub fn without_copy(&self) -> AppShortcuts {
        AppShortcuts {
            entries: self
                .entries
                .iter()
                .filter(|(_, cmd)| *cmd != AppCommand::Copy)
                .cloned()
                .collect(),
        }
    }

    /// Whether some chord is bound to `cmd`.
    pub fn binds(&self, cmd: AppCommand) -> bool {
        self.entries.iter().any(|(_, c)| *c == cmd)
    }

    /// The command of the first chord that matches a press of `key` with `held` modifiers.
    pub fn matching(&self, key: &Key, held: Modifiers, keymap: &Keymap) -> Option<AppCommand> {
        self.entries
            .iter()
            .find(|(chord, _)| chord.matches(key, held, keymap))
            .map(|(_, cmd)| *cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        shift: false,
        control: false,
        alt: false,
        logo: false,
    };

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn paste_is_cmd_v_on_macos_and_ctrl_shift_v_elsewhere() {
        let mac = AppShortcuts::standard(true);
        let pc = AppShortcuts::standard(false);
        let logo = Modifiers { logo: true, ..NONE };
        let ctrl = Modifiers {
            control: true,
            ..NONE
        };
        let ctrl_shift = Modifiers {
            control: true,
            shift: true,
            ..NONE
        };
        assert_eq!(
            mac.matching(&ch("v"), logo, &Keymap::MAC),
            Some(AppCommand::Paste)
        );
        assert_eq!(mac.matching(&ch("v"), ctrl, &Keymap::MAC), None);
        assert_eq!(
            pc.matching(&ch("V"), ctrl_shift, &Keymap::PC),
            Some(AppCommand::Paste)
        );
        assert_eq!(pc.matching(&ch("v"), ctrl, &Keymap::PC), None);
        assert_eq!(pc.matching(&ch("v"), logo, &Keymap::PC), None);
    }

    #[test]
    fn shift_page_home_end_scroll_and_the_plain_keys_do_not() {
        let t = AppShortcuts::standard(false);
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        let at = |k, m| t.matching(&Key::Named(k), m, &Keymap::PC);
        assert_eq!(
            at(NamedKey::PageUp, shift),
            Some(AppCommand::Scroll(Scroll::Pages(-1)))
        );
        assert_eq!(
            at(NamedKey::PageDown, shift),
            Some(AppCommand::Scroll(Scroll::Pages(1)))
        );
        assert_eq!(
            at(NamedKey::Home, shift),
            Some(AppCommand::Scroll(Scroll::Top))
        );
        assert_eq!(
            at(NamedKey::End, shift),
            Some(AppCommand::Scroll(Scroll::Bottom))
        );
        assert_eq!(at(NamedKey::PageUp, NONE), None);
        // Extra modifiers do not match.
        let both = Modifiers {
            shift: true,
            control: true,
            ..NONE
        };
        assert_eq!(at(NamedKey::PageUp, both), None);
    }

    fn tab_command(macos: bool, key: Key, held: Modifiers) -> Option<AppCommand> {
        let t = AppShortcuts::standard(macos);
        let keymap = if macos { &Keymap::MAC } else { &Keymap::PC };
        t.matching(&key, held, keymap)
    }

    #[test]
    fn the_tab_shortcuts_follow_d13_on_macos() {
        let logo = Modifiers { logo: true, ..NONE };
        let logo_shift = Modifiers {
            logo: true,
            shift: true,
            ..NONE
        };
        let at = |k: &str, m| tab_command(true, ch(k), m);
        assert_eq!(at("t", logo), Some(AppCommand::NewTab));
        assert_eq!(at("w", logo), Some(AppCommand::CloseTab));
        assert_eq!(at("[", logo_shift), Some(AppCommand::PrevTab));
        assert_eq!(at("{", logo_shift), Some(AppCommand::PrevTab));
        assert_eq!(at("]", logo_shift), Some(AppCommand::NextTab));
        assert_eq!(at("}", logo_shift), Some(AppCommand::NextTab));
        for n in 1..=9usize {
            assert_eq!(at(&n.to_string(), logo), Some(AppCommand::SelectTab(n - 1)));
        }
        assert_eq!(at("0", logo), None);
        // Plain keys and Ctrl chords are the terminal's.
        let ctrl = Modifiers {
            control: true,
            ..NONE
        };
        assert_eq!(at("t", NONE), None);
        assert_eq!(at("t", ctrl), None);
        assert_eq!(at("w", ctrl), None);
        assert_eq!(at("1", NONE), None);
        assert_eq!(tab_command(true, Key::Named(NamedKey::Tab), ctrl), None);
    }

    #[test]
    fn the_tab_shortcuts_follow_d13_elsewhere() {
        let ctrl_shift = Modifiers {
            control: true,
            shift: true,
            ..NONE
        };
        let ctrl = Modifiers {
            control: true,
            ..NONE
        };
        let alt = Modifiers { alt: true, ..NONE };
        let at = |k: &str, m| tab_command(false, ch(k), m);
        assert_eq!(at("t", ctrl_shift), Some(AppCommand::NewTab));
        assert_eq!(at("w", ctrl_shift), Some(AppCommand::CloseTab));
        let tab = |m| tab_command(false, Key::Named(NamedKey::Tab), m);
        assert_eq!(tab(ctrl), Some(AppCommand::NextTab));
        assert_eq!(tab(ctrl_shift), Some(AppCommand::PrevTab));
        assert_eq!(tab(NONE), None);
        for n in 1..=9usize {
            assert_eq!(at(&n.to_string(), alt), Some(AppCommand::SelectTab(n - 1)));
        }
        // Ctrl+T and Ctrl+W alone reach the shell (readline), and Alt+letter is Meta.
        assert_eq!(at("t", ctrl), None);
        assert_eq!(at("w", ctrl), None);
        assert_eq!(at("t", alt), None);
        // Cmd (Win) chords are not ours on these platforms.
        let logo = Modifiers { logo: true, ..NONE };
        assert_eq!(at("t", logo), None);
    }

    #[test]
    fn only_the_commands_that_must_not_repeat_say_so() {
        assert!(AppCommand::Paste.once_per_press());
        assert!(AppCommand::NewTab.once_per_press());
        assert!(AppCommand::CloseTab.once_per_press());
        assert!(!AppCommand::NextTab.once_per_press());
        assert!(!AppCommand::Scroll(Scroll::Top).once_per_press());
    }

    #[test]
    fn copy_is_cmd_c_on_macos_and_ctrl_shift_c_elsewhere_and_ctrl_c_stays_the_terminals() {
        let logo = Modifiers { logo: true, ..NONE };
        let ctrl = Modifiers {
            control: true,
            ..NONE
        };
        let ctrl_shift = Modifiers {
            control: true,
            shift: true,
            ..NONE
        };
        assert_eq!(tab_command(true, ch("c"), logo), Some(AppCommand::Copy));
        assert_eq!(tab_command(true, ch("c"), ctrl), None);
        assert_eq!(
            tab_command(false, ch("C"), ctrl_shift),
            Some(AppCommand::Copy)
        );
        // Plain Ctrl+C is the interrupt, never ours.
        assert_eq!(tab_command(false, ch("c"), ctrl), None);
        assert_eq!(tab_command(false, ch("c"), logo), None);
        // Without a selection the table has no Copy: the chord falls through to the router.
        let t = AppShortcuts::standard(false).without_copy();
        assert_eq!(t.matching(&ch("C"), ctrl_shift, &Keymap::PC), None);
        assert_eq!(
            t.matching(&ch("v"), ctrl_shift, &Keymap::PC),
            Some(AppCommand::Paste),
            "the rest is kept"
        );
        assert!(AppCommand::Copy.once_per_press());
    }

    #[test]
    fn settings_is_cmd_comma_on_macos_and_ctrl_comma_elsewhere() {
        let mac = AppShortcuts::standard(true);
        let pc = AppShortcuts::standard(false);
        let logo = Modifiers { logo: true, ..NONE };
        let ctrl = Modifiers {
            control: true,
            ..NONE
        };
        assert_eq!(
            mac.matching(&ch(","), logo, &Keymap::MAC),
            Some(AppCommand::Settings)
        );
        assert_eq!(mac.matching(&ch(","), ctrl, &Keymap::MAC), None);
        assert_eq!(
            pc.matching(&ch(","), ctrl, &Keymap::PC),
            Some(AppCommand::Settings)
        );
        assert_eq!(pc.matching(&ch(","), logo, &Keymap::PC), None);
        assert_eq!(pc.matching(&ch(","), NONE, &Keymap::PC), None);
        assert!(AppCommand::Settings.once_per_press());
        assert!(mac.binds(AppCommand::Settings) && pc.binds(AppCommand::Settings));
        assert!(!pc.without_copy().binds(AppCommand::Copy));
    }
}
