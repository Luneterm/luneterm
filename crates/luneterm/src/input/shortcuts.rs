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
    /// Scroll the viewport locally.
    Scroll(Scroll),
}

/// The shortcut table. A chord matches only with exactly its modifiers held.
#[derive(Debug, Clone)]
pub struct AppShortcuts {
    entries: Vec<(Chord, AppCommand)>,
}

impl AppShortcuts {
    /// Lunegit's table (TerminalView.cs:1151-1175, :1807-1810): Shift+PageUp/PageDown scroll a
    /// page, Shift+Home/End go to the top and bottom; paste is Cmd+V on macOS and Ctrl+Shift+V
    /// elsewhere (plain Ctrl+V is the shell's literal-next).
    pub fn standard(macos: bool) -> AppShortcuts {
        let v = Chord::key(Key::Character("v".into()));
        let paste = if macos { v.logo() } else { v.ctrl().shift() };
        let shift = |k| Chord::key(Key::Named(k)).shift();
        AppShortcuts {
            entries: vec![
                (paste, AppCommand::Paste),
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
            ],
        }
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
}
