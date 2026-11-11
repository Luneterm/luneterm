//! The app's menus (plan M10 step 6): the `⋯` app menu, the terminal's context menu and the tab's
//! context menu, as plain data (what rows, which action each picks, which shortcut hint it shows)
//! that the renderer turns into the toolkit's menu. The tab strip owns no menu since luneui-rs
//! 0.3.0: it reports the right-click and the app opens [`MenuKind::Tab`], whose picks act on the
//! tab it was opened on ([`tab_op`]).

use lune_ui::icon;
use lune_ui::strings::StringTable;
use lune_ui::widgets::menu::{MenuItem, MenuRow};

use crate::input::{AppCommand, AppShortcuts};
use crate::tabs::TabId;

/// What a picked row asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    NewTab,
    Close,
    CloseOthers,
    CloseAll,
    Rename,
    Duplicate,
    CopyCwd,
    Settings,
    About,
    Copy,
    Paste,
    SelectAll,
    Clear,
}

impl MenuAction {
    /// The app shortcut bound to this action, when one is (D13 and M9's): the hint shows only
    /// for these.
    pub fn shortcut(self) -> Option<AppCommand> {
        match self {
            MenuAction::NewTab => Some(AppCommand::NewTab),
            MenuAction::Close => Some(AppCommand::CloseTab),
            MenuAction::Settings => Some(AppCommand::Settings),
            MenuAction::Copy => Some(AppCommand::Copy),
            MenuAction::Paste => Some(AppCommand::Paste),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            MenuAction::NewTab => "menu.newTab",
            MenuAction::Close => "menu.close",
            MenuAction::CloseOthers => "menu.closeOthers",
            MenuAction::CloseAll => "menu.closeAll",
            MenuAction::Rename => "menu.rename",
            MenuAction::Duplicate => "menu.duplicate",
            MenuAction::CopyCwd => "menu.copyCwd",
            MenuAction::Settings => "menu.settings",
            MenuAction::About => "menu.about",
            MenuAction::Copy => "menu.copy",
            MenuAction::Paste => "menu.paste",
            MenuAction::SelectAll => "menu.selectAll",
            MenuAction::Clear => "menu.clear",
        }
    }

    /// The row's icon. Within one menu no two rows show the same icon (a test checks it); the
    /// codicon names are those of the Nerd Font the toolkit draws icons from.
    fn glyph(self) -> Option<&'static str> {
        match self {
            MenuAction::NewTab => Some(icon::ADD),
            MenuAction::Close => Some(icon::CLOSE),
            MenuAction::CloseOthers => Some(icon::CLOSE_ALL),
            MenuAction::CloseAll | MenuAction::Clear => Some(icon::CLEAR_ALL),
            MenuAction::Rename => Some(icon::EDIT),
            MenuAction::Duplicate => Some(ICON_TERMINAL),
            MenuAction::CopyCwd | MenuAction::Copy => Some(icon::COPY),
            MenuAction::Settings => Some(icon::GEAR),
            MenuAction::About => Some(icon::INFO),
            MenuAction::Paste => Some(ICON_CLIPPY),
            MenuAction::SelectAll => Some(ICON_LIST_SELECTION),
        }
    }
}

/// The codicon `terminal` (the toolkit's icon list has no app glyphs).
const ICON_TERMINAL: &str = "\u{ea85}";
/// The codicon `clippy`.
const ICON_CLIPPY: &str = "\u{eac0}";
/// The codicon `list-selection`.
const ICON_LIST_SELECTION: &str = "\u{eb85}";

/// The shortcut hint of `action` on this OS, empty when it has no shortcut. The text is the chord
/// as the table binds it (D13).
pub fn hint(action: MenuAction, macos: bool) -> &'static str {
    use MenuAction::{Close, Copy, NewTab, Paste, Settings};
    match (action, macos) {
        (NewTab, true) => "\u{2318}T",
        (Close, true) => "\u{2318}W",
        (Settings, true) => "\u{2318},",
        (Copy, true) => "\u{2318}C",
        (Paste, true) => "\u{2318}V",
        (NewTab, false) => "Ctrl+Shift+T",
        (Close, false) => "Ctrl+Shift+W",
        (Settings, false) => "Ctrl+,",
        (Copy, false) => "Ctrl+Shift+C",
        (Paste, false) => "Ctrl+Shift+V",
        _ => "",
    }
}

/// Which menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    /// The `⋯` menu at the strip's right end.
    App,
    /// The terminal's context menu; `selection` is whether text is selected (Copy needs it).
    Terminal { selection: bool },
    /// A tab's context menu; `closable` is whether the tab can be closed (Close is only for a
    /// closable tab). It has no shortcut hints and no separators.
    Tab { closable: bool },
}

/// One row of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `None` is a separator.
    pub action: Option<MenuAction>,
    pub glyph: Option<&'static str>,
    pub text: String,
    pub shortcut: String,
    pub disabled: bool,
}

/// A menu's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    pub kind: MenuKind,
    pub entries: Vec<Entry>,
}

const SEPARATOR: Option<MenuAction> = None;

fn actions(kind: MenuKind) -> Vec<Option<MenuAction>> {
    use MenuAction::*;
    match kind {
        MenuKind::App => vec![
            Some(NewTab),
            SEPARATOR,
            Some(Close),
            Some(CloseOthers),
            Some(CloseAll),
            SEPARATOR,
            Some(Rename),
            Some(Duplicate),
            Some(CopyCwd),
            SEPARATOR,
            Some(Settings),
            Some(About),
        ],
        MenuKind::Terminal { .. } => vec![
            Some(Copy),
            Some(Paste),
            SEPARATOR,
            Some(SelectAll),
            Some(Clear),
            SEPARATOR,
            Some(NewTab),
        ],
        MenuKind::Tab { closable } => closable
            .then_some(Some(Close))
            .into_iter()
            .chain([
                Some(CloseOthers),
                Some(CloseAll),
                Some(Rename),
                Some(Duplicate),
                Some(CopyCwd),
            ])
            .collect(),
    }
}

impl MenuModel {
    /// The rows of `kind` with their texts from `strings` and the hints of this OS.
    pub fn new(kind: MenuKind, strings: &StringTable, macos: bool) -> MenuModel {
        let is_tab = matches!(kind, MenuKind::Tab { .. });
        let entries = actions(kind)
            .into_iter()
            .map(|action| match action {
                None => Entry {
                    action: None,
                    glyph: None,
                    text: String::new(),
                    shortcut: String::new(),
                    disabled: false,
                },
                Some(a) => Entry {
                    action,
                    glyph: a.glyph(),
                    text: strings.tr(a.key()).to_owned(),
                    shortcut: if is_tab {
                        String::new()
                    } else {
                        hint(a, macos).to_owned()
                    },
                    disabled: a == MenuAction::Copy
                        && kind == (MenuKind::Terminal { selection: false }),
                },
            })
            .collect();
        MenuModel { kind, entries }
    }

    /// The toolkit's rows, in the order of `entries` (a pick's index is an index into both).
    pub fn rows(&self) -> Vec<MenuRow<'_>> {
        self.entries
            .iter()
            .map(|e| match e.action {
                None => MenuRow::Separator,
                Some(_) => MenuRow::Entry(MenuItem {
                    glyph: e.glyph,
                    text: &e.text,
                    shortcut: &e.shortcut,
                    disabled: e.disabled,
                    ..MenuItem::default()
                }),
            })
            .collect()
    }

    /// The action of the row picked at `index`; `None` for a separator or a disabled row.
    pub fn action_at(&self, index: usize) -> Option<MenuAction> {
        self.entries
            .get(index)
            .filter(|e| !e.disabled)
            .and_then(|e| e.action)
    }

    /// The accessible name of the menu.
    pub fn name_key(&self) -> &'static str {
        match self.kind {
            MenuKind::App => "menu.app.name",
            MenuKind::Terminal { .. } => "menu.terminal.name",
            MenuKind::Tab { .. } => "menu.tab.name",
        }
    }
}

/// What a pick of the tab menu does, to the tab the menu was opened on (not the active one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabOp {
    /// Close this tab.
    Close(TabId),
    /// Close every tab but this one.
    CloseOthers(TabId),
    CloseAll,
    /// Rename this tab.
    Rename(TabId),
    /// Make this tab the active one, then run the action as if picked from the `⋯` menu (it acts
    /// on the active tab).
    ActivateThen(TabId, MenuAction),
}

/// The effect of `action` picked from the tab menu opened on `target`; `None` for an action the
/// tab menu has no row for.
pub fn tab_op(action: MenuAction, target: TabId) -> Option<TabOp> {
    match action {
        MenuAction::Close => Some(TabOp::Close(target)),
        MenuAction::CloseOthers => Some(TabOp::CloseOthers(target)),
        MenuAction::CloseAll => Some(TabOp::CloseAll),
        MenuAction::Rename => Some(TabOp::Rename(target)),
        MenuAction::Duplicate | MenuAction::CopyCwd => Some(TabOp::ActivateThen(target, action)),
        _ => None,
    }
}

/// Every string key the menus ask for.
pub fn required_keys() -> Vec<String> {
    let mut keys: Vec<String> = [
        "menu.app.name",
        "menu.terminal.name",
        "menu.tab.name",
        "menu.more",
    ]
    .iter()
    .map(|k| (*k).to_owned())
    .collect();
    for kind in [
        MenuKind::App,
        MenuKind::Terminal { selection: true },
        MenuKind::Tab { closable: true },
    ] {
        for a in actions(kind).into_iter().flatten() {
            keys.push(a.key().to_owned());
        }
    }
    keys
}

/// Whether `shortcuts` binds the command a hint advertises.
pub fn hint_is_bound(action: MenuAction, shortcuts: &AppShortcuts) -> bool {
    action.shortcut().is_none_or(|cmd| shortcuts.binds(cmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(kind: MenuKind, macos: bool) -> MenuModel {
        MenuModel::new(kind, &crate::strings::english(), macos)
    }

    #[test]
    fn the_app_menu_has_the_rows_and_separators_of_the_spec_in_order() {
        let m = model(MenuKind::App, true);
        let texts: Vec<&str> = m.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "New Tab",
                "",
                "Close",
                "Close Others",
                "Close All",
                "",
                "Rename\u{2026}",
                "Duplicate Tab",
                "Copy Working Directory",
                "",
                "Settings\u{2026}",
                "About Luneterm"
            ]
        );
        assert_eq!(m.rows().len(), 12);
        // A pick maps to its action; a separator picks nothing.
        assert_eq!(m.action_at(0), Some(MenuAction::NewTab));
        assert_eq!(m.action_at(1), None);
        assert_eq!(m.action_at(10), Some(MenuAction::Settings));
        assert_eq!(m.action_at(11), Some(MenuAction::About));
        assert_eq!(m.action_at(99), None);
    }

    #[test]
    fn the_terminal_menu_disables_copy_only_without_a_selection() {
        let with = model(MenuKind::Terminal { selection: true }, false);
        let without = model(MenuKind::Terminal { selection: false }, false);
        let texts: Vec<&str> = with.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            ["Copy", "Paste", "", "Select All", "Clear", "", "New Tab"]
        );
        assert!(!with.entries[0].disabled);
        assert!(without.entries[0].disabled);
        assert_eq!(without.entries.iter().filter(|e| e.disabled).count(), 1);
        assert_eq!(with.action_at(0), Some(MenuAction::Copy));
        assert_eq!(without.action_at(0), None, "a disabled row picks nothing");
        assert_eq!(without.action_at(1), Some(MenuAction::Paste));
        assert_eq!(without.action_at(3), Some(MenuAction::SelectAll));
        assert_eq!(without.action_at(4), Some(MenuAction::Clear));
        assert_eq!(without.action_at(6), Some(MenuAction::NewTab));
    }

    /// A hint shows only for an action whose shortcut the table binds, and the text is the chord.
    #[test]
    fn hints_exist_exactly_for_bound_shortcuts_on_each_os() {
        use MenuAction::*;
        let all = [
            NewTab,
            Close,
            CloseOthers,
            CloseAll,
            Rename,
            Duplicate,
            CopyCwd,
            Settings,
            About,
            Copy,
            Paste,
            SelectAll,
            Clear,
        ];
        for macos in [true, false] {
            let table = AppShortcuts::standard(macos);
            for a in all {
                let shown = !hint(a, macos).is_empty();
                assert_eq!(shown, a.shortcut().is_some(), "{a:?} macos={macos}");
                assert!(hint_is_bound(a, &table), "{a:?} macos={macos}");
            }
        }
        assert_eq!(hint(NewTab, true), "\u{2318}T");
        assert_eq!(hint(Settings, true), "\u{2318},");
        assert_eq!(hint(NewTab, false), "Ctrl+Shift+T");
        assert_eq!(hint(Settings, false), "Ctrl+,");
        assert_eq!(hint(Copy, false), "Ctrl+Shift+C");
        assert_eq!(hint(Paste, false), "Ctrl+Shift+V");
        assert_eq!(hint(Clear, true), "");
        let m = model(MenuKind::App, true);
        assert_eq!(m.entries[0].shortcut, "\u{2318}T");
        assert_eq!(m.entries[3].shortcut, "", "Close Others has no shortcut");
    }

    #[test]
    fn the_tab_menu_rows_for_a_closable_tab_and_for_one_that_is_not() {
        let closable = model(MenuKind::Tab { closable: true }, true);
        let texts: Vec<&str> = closable.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Close",
                "Close Others",
                "Close All",
                "Rename\u{2026}",
                "Duplicate Tab",
                "Copy Working Directory"
            ]
        );
        let fixed = model(MenuKind::Tab { closable: false }, true);
        let texts: Vec<&str> = fixed.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Close Others",
                "Close All",
                "Rename\u{2026}",
                "Duplicate Tab",
                "Copy Working Directory"
            ]
        );
        for m in [&closable, &fixed] {
            assert!(
                m.entries.iter().all(|e| e.action.is_some()),
                "no separators"
            );
            assert!(m.entries.iter().all(|e| e.shortcut.is_empty()), "no hints");
            assert!(
                m.entries.iter().all(|e| e.glyph.is_some()),
                "every row has an icon"
            );
            assert!(m.entries.iter().all(|e| !e.disabled));
            assert_eq!(m.rows().len(), m.entries.len());
            assert_eq!(m.name_key(), "menu.tab.name");
        }
        assert_eq!(closable.action_at(0), Some(MenuAction::Close));
        assert_eq!(fixed.action_at(0), Some(MenuAction::CloseOthers));
        assert_eq!(closable.action_at(5), Some(MenuAction::CopyCwd));
        assert_eq!(closable.action_at(6), None);
    }

    #[test]
    fn the_tab_menu_name_is_a_string_and_every_required_key_resolves() {
        let strings = crate::strings::english();
        assert_eq!(strings.tr("menu.tab.name"), "Tab menu");
        for key in required_keys() {
            assert_ne!(strings.tr(&key), key, "{key} has no text");
        }
    }

    /// Icons that draw the same shape count as one (the toolkit's `CLOSE` and `CHROME_CLOSE` are both
    /// an X).
    fn visible_icon(glyph: &str) -> &str {
        if glyph == icon::CHROME_CLOSE {
            icon::CLOSE
        } else {
            glyph
        }
    }

    /// No two rows of one menu show the same icon (an empty slot, `None`, may repeat).
    #[test]
    fn no_menu_shows_the_same_icon_twice() {
        let kinds = [
            MenuKind::App,
            MenuKind::Terminal { selection: true },
            MenuKind::Terminal { selection: false },
            MenuKind::Tab { closable: true },
            MenuKind::Tab { closable: false },
        ];
        for kind in kinds {
            let m = model(kind, true);
            let icons: Vec<&str> = m
                .entries
                .iter()
                .filter_map(|e| e.glyph)
                .map(visible_icon)
                .collect();
            for (i, a) in icons.iter().enumerate() {
                assert!(!icons[..i].contains(a), "{kind:?}: {a:?} appears twice");
            }
        }
        // the equivalence the test relies on: the two X glyphs fold to one
        assert_eq!(visible_icon(icon::CHROME_CLOSE), visible_icon(icon::CLOSE));
    }

    /// A tab-menu pick acts on the tab the menu was opened on (here 7), whichever is active.
    #[test]
    fn a_tab_menu_pick_acts_on_its_target_tab() {
        use MenuAction::*;
        assert_eq!(tab_op(Close, 7), Some(TabOp::Close(7)));
        assert_eq!(tab_op(CloseOthers, 7), Some(TabOp::CloseOthers(7)));
        assert_eq!(tab_op(CloseAll, 7), Some(TabOp::CloseAll));
        assert_eq!(tab_op(Rename, 7), Some(TabOp::Rename(7)));
        assert_eq!(
            tab_op(Duplicate, 7),
            Some(TabOp::ActivateThen(7, Duplicate))
        );
        assert_eq!(tab_op(CopyCwd, 7), Some(TabOp::ActivateThen(7, CopyCwd)));
        for other in [NewTab, Settings, About, Copy, Paste, SelectAll, Clear] {
            assert_eq!(
                tab_op(other, 7),
                None,
                "{other:?} is no row of the tab menu"
            );
        }
        // every row of the menu maps
        for kind in [
            MenuKind::Tab { closable: true },
            MenuKind::Tab { closable: false },
        ] {
            for a in actions(kind).into_iter().flatten() {
                assert!(tab_op(a, 7).is_some(), "{a:?}");
            }
        }
    }
}
