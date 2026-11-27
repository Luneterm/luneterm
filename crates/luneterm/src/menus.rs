//! The app's menus (plan M10 step 6): the `⋯` app menu, the terminal's context menu and the tab's
//! context menu, as plain data (what rows, which action each picks, which shortcut hint it shows)
//! that the renderer turns into the toolkit's menu. The tab strip owns no menu since luneui-rs
//! 0.3.0: it reports the right-click and the app opens [`MenuKind::Tab`], whose picks act on the
//! tab it was opened on ([`tab_op`]).

use lune_ui::icon;
use lune_ui::strings::{DisplayFormat, StringTable};
use lune_ui::widgets::menu::{MenuItem, MenuRow};

use crate::groups::GroupId;
use crate::input::{AppCommand, AppShortcuts};
use crate::pane;
use crate::tabs::TabId;

/// What a picked row asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    NewTab,
    /// A new shell tab in a new group (the group editor asks for its name and colour).
    NewTabGroup,
    Close,
    CloseOthers,
    CloseAll,
    Rename,
    Duplicate,
    CopyCwd,
    /// Put the menu's tab into a new group (through the group editor).
    AddToNewGroup,
    /// Put the menu's tab into this open group.
    AddToGroup(GroupId),
    RemoveFromGroup,
    /// Rename or recolour the menu's group.
    EditGroup,
    /// Close every tab of the menu's group, after asking.
    CloseGroup,
    NewTabInGroup,
    CollapseGroup,
    ExpandGroup,
    /// Drop the group and keep its tabs.
    Ungroup,
    /// Show the side pane (the saved choice).
    ShowSidePane,
    /// Hide the side pane: the top bar.
    HideSidePane,
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
            MenuAction::ShowSidePane | MenuAction::HideSidePane => Some(AppCommand::TogglePane),
            MenuAction::Copy => Some(AppCommand::Copy),
            MenuAction::Paste => Some(AppCommand::Paste),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            MenuAction::NewTab => "menu.newTab",
            MenuAction::NewTabGroup => "menu.newTabGroup",
            MenuAction::AddToNewGroup => "menu.addToNewGroup",
            MenuAction::AddToGroup(_) => "menu.addToGroup",
            MenuAction::RemoveFromGroup => "menu.removeFromGroup",
            MenuAction::EditGroup => "menu.editGroup",
            MenuAction::CloseGroup => "menu.closeGroup",
            MenuAction::NewTabInGroup => "menu.newTabInGroup",
            MenuAction::CollapseGroup => "menu.collapseGroup",
            MenuAction::ExpandGroup => "menu.expandGroup",
            MenuAction::Ungroup => "menu.ungroup",
            MenuAction::Close => "menu.close",
            MenuAction::CloseOthers => "menu.closeOthers",
            MenuAction::CloseAll => "menu.closeAll",
            MenuAction::Rename => "menu.rename",
            MenuAction::Duplicate => "menu.duplicate",
            MenuAction::CopyCwd => "menu.copyCwd",
            MenuAction::ShowSidePane => "menu.showSidePane",
            MenuAction::HideSidePane => "menu.hideSidePane",
            MenuAction::Settings => "menu.settings",
            MenuAction::About => "menu.about",
            MenuAction::Copy => "menu.copy",
            MenuAction::Paste => "menu.paste",
            MenuAction::SelectAll => "menu.selectAll",
            MenuAction::Clear => "menu.clear",
        }
    }

    /// The row's icon. Within one menu no two rows show the same icon (a test checks it); the
    /// codicon names are those of the Nerd icons in RznMono, the mono font the toolkit draws icons with.
    /// The rows that join a tab to a named group have none: there is one per group.
    fn glyph(self) -> Option<&'static str> {
        match self {
            MenuAction::NewTab | MenuAction::NewTabInGroup => Some(icon::ADD),
            MenuAction::NewTabGroup | MenuAction::AddToNewGroup => Some(ICON_GROUP),
            MenuAction::AddToGroup(_) => None,
            MenuAction::RemoveFromGroup | MenuAction::Ungroup => Some(ICON_UNGROUP),
            MenuAction::EditGroup => Some(ICON_SYMBOL_COLOR),
            MenuAction::CloseGroup => Some(ICON_TRASH),
            MenuAction::CollapseGroup => Some(ICON_COLLAPSE_ALL),
            MenuAction::ExpandGroup => Some(ICON_EXPAND_ALL),
            MenuAction::Close => Some(icon::CLOSE),
            MenuAction::CloseOthers => Some(icon::CLOSE_ALL),
            MenuAction::CloseAll | MenuAction::Clear => Some(icon::CLEAR_ALL),
            MenuAction::Rename => Some(icon::EDIT),
            MenuAction::Duplicate => Some(ICON_TERMINAL),
            MenuAction::CopyCwd | MenuAction::Copy => Some(icon::COPY),
            MenuAction::ShowSidePane | MenuAction::HideSidePane => Some(pane::TOGGLE_GLYPH),
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
/// The codicon `group-by-ref-type`.
const ICON_GROUP: &str = "\u{eb97}";
/// The codicon `ungroup-by-ref-type`.
const ICON_UNGROUP: &str = "\u{eb98}";
/// The codicon `symbol-color`.
const ICON_SYMBOL_COLOR: &str = "\u{eb5c}";
/// The codicon `trash`.
const ICON_TRASH: &str = "\u{ea81}";
/// The codicon `collapse-all`.
const ICON_COLLAPSE_ALL: &str = "\u{eac5}";
/// The codicon `expand-all`.
const ICON_EXPAND_ALL: &str = "\u{eb95}";

/// The most chars of a group's name a menu row shows, the `\u{2026}` of a cut one included.
pub const MENU_NAME_CHARS: usize = 40;

/// `name` as a menu row shows it: whole, or cut to [`MENU_NAME_CHARS`] chars ending in `\u{2026}`.
fn menu_name(name: &str) -> String {
    if name.chars().nth(MENU_NAME_CHARS).is_none() {
        return name.to_owned();
    }
    let cut: String = name.chars().take(MENU_NAME_CHARS - 1).collect();
    format!("{}\u{2026}", cut.trim_end())
}

/// The shortcut hint of `action` on this OS, empty when it has no shortcut. The text is the chord
/// as the table binds it (D13).
pub fn hint(action: MenuAction, macos: bool) -> &'static str {
    use MenuAction::{Close, Copy, HideSidePane, NewTab, Paste, Settings, ShowSidePane};
    match (action, macos) {
        (NewTab, true) => "\u{2318}T",
        (Close, true) => "\u{2318}W",
        (Settings, true) => "\u{2318},",
        (ShowSidePane | HideSidePane, true) => "\u{2318}B",
        (Copy, true) => "\u{2318}C",
        (Paste, true) => "\u{2318}V",
        (NewTab, false) => "Ctrl+Shift+T",
        (Close, false) => "Ctrl+Shift+W",
        (Settings, false) => "Ctrl+,",
        (ShowSidePane | HideSidePane, false) => "Ctrl+Shift+B",
        (Copy, false) => "Ctrl+Shift+C",
        (Paste, false) => "Ctrl+Shift+V",
        _ => "",
    }
}

/// Which menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    /// The `⋯` menu at the strip's right end, or the pane's footer; `pane` is whether the saved
    /// view is the side pane (the row hides it, else shows it).
    App { pane: bool },
    /// The terminal's context menu; `selection` is whether text is selected (Copy needs it).
    Terminal { selection: bool },
    /// A tab's context menu; `closable` is whether the tab can be closed (Close is only for a
    /// closable tab). It has no shortcut hints.
    Tab { closable: bool },
    /// A group's own menu (the side pane's card header); `collapsed` picks Collapse or Expand.
    Group { collapsed: bool },
}

/// The groups a menu is about, as they were when it opened: its rows do not move under the
/// pointer when a program regroups tabs meanwhile.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MenuGroups {
    /// The group of the tab a tab menu is for, or the group a group menu is for.
    pub own: Option<GroupId>,
    /// The open groups, in tab order, with their names.
    pub open: Vec<(GroupId, String)>,
}

impl std::fmt::Debug for MenuGroups {
    /// Counts only: a program can choose a name, and program text is kept out of a `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuGroups")
            .field("own", &self.own)
            .field("open", &self.open.len())
            .finish()
    }
}

/// One row of a menu.
#[derive(Clone, PartialEq, Eq)]
pub struct Entry {
    /// `None` is a separator.
    pub action: Option<MenuAction>,
    pub glyph: Option<&'static str>,
    pub text: String,
    pub shortcut: String,
    pub disabled: bool,
}

impl std::fmt::Debug for Entry {
    /// The text's length only: a group's name may be in it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("action", &self.action)
            .field("text_chars", &self.text.chars().count())
            .field("disabled", &self.disabled)
            .finish()
    }
}

/// A menu's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    pub kind: MenuKind,
    pub entries: Vec<Entry>,
}

const SEPARATOR: Option<MenuAction> = None;

fn actions(kind: MenuKind, groups: &MenuGroups) -> Vec<Option<MenuAction>> {
    use MenuAction::*;
    match kind {
        MenuKind::App { pane } => vec![
            Some(NewTab),
            Some(NewTabGroup),
            SEPARATOR,
            Some(Close),
            Some(CloseOthers),
            Some(CloseAll),
            SEPARATOR,
            Some(Rename),
            Some(Duplicate),
            Some(CopyCwd),
            SEPARATOR,
            Some(if pane { HideSidePane } else { ShowSidePane }),
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
        MenuKind::Tab { closable } => {
            let mut rows: Vec<Option<MenuAction>> = closable
                .then_some(Some(Close))
                .into_iter()
                .chain([
                    Some(CloseOthers),
                    Some(CloseAll),
                    Some(Rename),
                    Some(Duplicate),
                    Some(CopyCwd),
                    SEPARATOR,
                    Some(AddToNewGroup),
                ])
                .collect();
            rows.extend(
                groups
                    .open
                    .iter()
                    .filter(|(g, _)| Some(*g) != groups.own)
                    .map(|(g, _)| Some(AddToGroup(*g))),
            );
            // The group's own entries are here too, so the top bar needs no side pane.
            if groups.own.is_some() {
                rows.extend([
                    Some(RemoveFromGroup),
                    SEPARATOR,
                    Some(EditGroup),
                    Some(CloseGroup),
                ]);
            }
            rows
        }
        MenuKind::Group { collapsed } => vec![
            Some(EditGroup),
            Some(if collapsed {
                ExpandGroup
            } else {
                CollapseGroup
            }),
            Some(NewTabInGroup),
            Some(Ungroup),
            SEPARATOR,
            Some(CloseGroup),
        ],
    }
}

impl MenuModel {
    /// The rows of `kind` with their texts from `strings` and the hints of this OS, with no open
    /// group.
    pub fn new(kind: MenuKind, strings: &StringTable, macos: bool) -> MenuModel {
        MenuModel::with_groups(kind, &MenuGroups::default(), strings, macos)
    }

    /// [`MenuModel::new`] about `groups`: a tab menu offers every open group but its tab's own.
    pub fn with_groups(
        kind: MenuKind,
        groups: &MenuGroups,
        strings: &StringTable,
        macos: bool,
    ) -> MenuModel {
        let hinted = !matches!(kind, MenuKind::Tab { .. } | MenuKind::Group { .. });
        let entries = actions(kind, groups)
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
                    text: match a {
                        MenuAction::AddToGroup(g) => {
                            let name = groups
                                .open
                                .iter()
                                .find(|(id, _)| *id == g)
                                .map_or_else(String::new, |(_, n)| menu_name(n));
                            strings.tr_fmt(a.key(), &[&name], &DisplayFormat)
                        }
                        _ => strings.tr(a.key()).to_owned(),
                    },
                    shortcut: if hinted {
                        hint(a, macos).to_owned()
                    } else {
                        String::new()
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
            MenuKind::App { .. } => "menu.app.name",
            MenuKind::Terminal { .. } => "menu.terminal.name",
            MenuKind::Tab { .. } => "menu.tab.name",
            MenuKind::Group { .. } => "menu.group.name",
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
    /// Put this tab into a new group, through the group editor.
    AddToNewGroup(TabId),
    /// Put this tab into this group.
    AddToGroup(TabId, GroupId),
    RemoveFromGroup(TabId),
}

/// What a pick of a group's menu, or a group row of a tab menu, does to that group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupOp {
    Edit(GroupId),
    /// Close the group's tabs, after asking.
    Close(GroupId),
    NewTab(GroupId),
    SetCollapsed(GroupId, bool),
    Ungroup(GroupId),
}

/// The effect of `action` on the group `own` (the menu's); `None` for an action that is no group's.
pub fn group_op(action: MenuAction, own: GroupId) -> Option<GroupOp> {
    match action {
        MenuAction::EditGroup => Some(GroupOp::Edit(own)),
        MenuAction::CloseGroup => Some(GroupOp::Close(own)),
        MenuAction::NewTabInGroup => Some(GroupOp::NewTab(own)),
        MenuAction::CollapseGroup => Some(GroupOp::SetCollapsed(own, true)),
        MenuAction::ExpandGroup => Some(GroupOp::SetCollapsed(own, false)),
        MenuAction::Ungroup => Some(GroupOp::Ungroup(own)),
        _ => None,
    }
}

/// The effect of `action` picked from the tab menu opened on `target`; `None` for an action the
/// tab menu has no row for, and for the rows about the tab's group ([`group_op`]).
pub fn tab_op(action: MenuAction, target: TabId) -> Option<TabOp> {
    match action {
        MenuAction::Close => Some(TabOp::Close(target)),
        MenuAction::CloseOthers => Some(TabOp::CloseOthers(target)),
        MenuAction::CloseAll => Some(TabOp::CloseAll),
        MenuAction::Rename => Some(TabOp::Rename(target)),
        MenuAction::Duplicate | MenuAction::CopyCwd => Some(TabOp::ActivateThen(target, action)),
        MenuAction::AddToNewGroup => Some(TabOp::AddToNewGroup(target)),
        MenuAction::AddToGroup(g) => Some(TabOp::AddToGroup(target, g)),
        MenuAction::RemoveFromGroup => Some(TabOp::RemoveFromGroup(target)),
        _ => None,
    }
}

/// Every string key the menus ask for.
pub fn required_keys() -> Vec<String> {
    let mut keys: Vec<String> = [
        "menu.app.name",
        "menu.terminal.name",
        "menu.tab.name",
        "menu.group.name",
        "menu.more",
    ]
    .iter()
    .map(|k| (*k).to_owned())
    .collect();
    // Two groups, the tab in one: every row a tab menu can have.
    let groups = MenuGroups {
        own: Some(GroupId(1)),
        open: vec![(GroupId(1), String::new()), (GroupId(2), String::new())],
    };
    for kind in [
        MenuKind::App { pane: false },
        MenuKind::App { pane: true },
        MenuKind::Terminal { selection: true },
        MenuKind::Tab { closable: true },
        MenuKind::Group { collapsed: true },
        MenuKind::Group { collapsed: false },
    ] {
        for a in actions(kind, &groups).into_iter().flatten() {
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
        let m = model(MenuKind::App { pane: true }, true);
        let texts: Vec<&str> = m.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "New Tab",
                "New Tab Group\u{2026}",
                "",
                "Close",
                "Close Others",
                "Close All",
                "",
                "Rename\u{2026}",
                "Duplicate Tab",
                "Copy Working Directory",
                "",
                "Hide Side Pane",
                "",
                "Settings\u{2026}",
                "About Luneterm"
            ]
        );
        assert_eq!(m.rows().len(), 15);
        // A pick maps to its action; a separator picks nothing.
        assert_eq!(m.action_at(0), Some(MenuAction::NewTab));
        assert_eq!(m.action_at(1), Some(MenuAction::NewTabGroup));
        assert_eq!(m.action_at(2), None);
        assert_eq!(m.action_at(11), Some(MenuAction::HideSidePane));
        assert_eq!(m.action_at(13), Some(MenuAction::Settings));
        assert_eq!(m.action_at(14), Some(MenuAction::About));
        assert_eq!(m.entries[1].shortcut, "", "New Tab Group has no shortcut");
        assert_eq!(m.entries[11].shortcut, "\u{2318}B");
        assert_eq!(m.action_at(99), None);
        // With the top bar the row shows the pane.
        let bar = model(MenuKind::App { pane: false }, false);
        assert_eq!(bar.entries[11].text, "Show Side Pane");
        assert_eq!(bar.entries[11].shortcut, "Ctrl+Shift+B");
        assert_eq!(bar.action_at(11), Some(MenuAction::ShowSidePane));
        assert_eq!(bar.entries.len(), 15);
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
            NewTabGroup,
            AddToNewGroup,
            AddToGroup(GroupId(1)),
            RemoveFromGroup,
            EditGroup,
            CloseGroup,
            NewTabInGroup,
            CollapseGroup,
            ExpandGroup,
            Ungroup,
            Close,
            CloseOthers,
            CloseAll,
            Rename,
            Duplicate,
            CopyCwd,
            ShowSidePane,
            HideSidePane,
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
        let m = model(MenuKind::App { pane: true }, true);
        assert_eq!(m.entries[0].shortcut, "\u{2318}T");
        assert_eq!(m.entries[4].text, "Close Others");
        assert_eq!(m.entries[4].shortcut, "", "Close Others has no shortcut");
    }

    /// Open groups `api` (1) and `orch: shop` (2), the menu's tab or group being `own`.
    fn groups(own: Option<u64>) -> MenuGroups {
        MenuGroups {
            own: own.map(GroupId),
            open: vec![
                (GroupId(1), "api".to_owned()),
                (GroupId(2), "orch: shop".to_owned()),
            ],
        }
    }

    fn texts(m: &MenuModel) -> Vec<&str> {
        m.entries.iter().map(|e| e.text.as_str()).collect()
    }

    fn tab_menu(closable: bool, groups: &MenuGroups) -> MenuModel {
        MenuModel::with_groups(
            MenuKind::Tab { closable },
            groups,
            &crate::strings::english(),
            true,
        )
    }

    #[test]
    fn the_tab_menu_rows_for_a_closable_tab_and_for_one_that_is_not() {
        let closable = model(MenuKind::Tab { closable: true }, true);
        assert_eq!(
            texts(&closable),
            [
                "Close",
                "Close Others",
                "Close All",
                "Rename\u{2026}",
                "Duplicate Tab",
                "Copy Working Directory",
                "",
                "Add Tab to New Group\u{2026}",
            ]
        );
        let fixed = model(MenuKind::Tab { closable: false }, true);
        assert_eq!(
            texts(&fixed),
            [
                "Close Others",
                "Close All",
                "Rename\u{2026}",
                "Duplicate Tab",
                "Copy Working Directory",
                "",
                "Add Tab to New Group\u{2026}",
            ]
        );
        for m in [&closable, &fixed] {
            assert!(m.entries.iter().all(|e| e.shortcut.is_empty()), "no hints");
            assert!(
                m.entries
                    .iter()
                    .all(|e| e.action.is_none() || e.glyph.is_some()),
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
        assert_eq!(closable.action_at(7), Some(MenuAction::AddToNewGroup));
        assert_eq!(closable.action_at(8), None);
    }

    #[test]
    fn an_ungrouped_tab_is_offered_every_group_in_tab_order() {
        let m = tab_menu(true, &groups(None));
        assert_eq!(
            &texts(&m)[7..],
            [
                "Add Tab to New Group\u{2026}",
                "Add Tab to Group \u{201c}api\u{201d}",
                "Add Tab to Group \u{201c}orch: shop\u{201d}",
            ]
        );
        assert_eq!(m.action_at(8), Some(MenuAction::AddToGroup(GroupId(1))));
        assert_eq!(m.action_at(9), Some(MenuAction::AddToGroup(GroupId(2))));
        // The rows that join a group have no icon: there is one per group.
        assert!(m.entries[8..].iter().all(|e| e.glyph.is_none()));
    }

    #[test]
    fn a_grouped_tab_is_offered_the_other_groups_and_its_own_groups_entries() {
        let m = tab_menu(true, &groups(Some(1)));
        assert_eq!(
            &texts(&m)[6..],
            [
                "",
                "Add Tab to New Group\u{2026}",
                "Add Tab to Group \u{201c}orch: shop\u{201d}",
                "Remove from Group",
                "",
                "Edit Group\u{2026}",
                "Close Group\u{2026}",
            ]
        );
        let at: Vec<_> = (6..13).filter_map(|i| m.action_at(i)).collect();
        assert_eq!(
            at,
            [
                MenuAction::AddToNewGroup,
                MenuAction::AddToGroup(GroupId(2)),
                MenuAction::RemoveFromGroup,
                MenuAction::EditGroup,
                MenuAction::CloseGroup,
            ]
        );
        // A tab in the only group is offered no group to join.
        let only = MenuGroups {
            own: Some(GroupId(1)),
            open: vec![(GroupId(1), "api".to_owned())],
        };
        assert!(
            !tab_menu(true, &only)
                .entries
                .iter()
                .any(|e| matches!(e.action, Some(MenuAction::AddToGroup(_))))
        );
    }

    #[test]
    fn the_group_menu_offers_collapse_or_expand_and_close_last() {
        for (collapsed, middle) in [(false, "Collapse Group"), (true, "Expand Group")] {
            let m = MenuModel::with_groups(
                MenuKind::Group { collapsed },
                &groups(Some(1)),
                &crate::strings::english(),
                true,
            );
            assert_eq!(
                texts(&m),
                [
                    "Edit Group\u{2026}",
                    middle,
                    "New Tab in Group",
                    "Ungroup",
                    "",
                    "Close Group\u{2026}",
                ]
            );
            assert!(m.entries.iter().all(|e| e.shortcut.is_empty()), "no hints");
            assert_eq!(m.name_key(), "menu.group.name");
            let own = GroupId(1);
            let ops: Vec<_> = (0..6)
                .filter_map(|i| m.action_at(i))
                .map(|a| group_op(a, own).expect("every row is the group's"))
                .collect();
            assert_eq!(
                ops,
                [
                    GroupOp::Edit(own),
                    GroupOp::SetCollapsed(own, !collapsed),
                    GroupOp::NewTab(own),
                    GroupOp::Ungroup(own),
                    GroupOp::Close(own),
                ]
            );
        }
    }

    #[test]
    fn a_long_group_name_is_cut_in_its_row_and_the_action_keeps_the_group() {
        let long = "x".repeat(MENU_NAME_CHARS + 1);
        let exact = "y".repeat(MENU_NAME_CHARS);
        let g = MenuGroups {
            own: None,
            open: vec![(GroupId(1), long), (GroupId(2), exact.clone())],
        };
        let m = tab_menu(true, &g);
        let cut = format!("{}\u{2026}", "x".repeat(MENU_NAME_CHARS - 1));
        assert_eq!(cut.chars().count(), MENU_NAME_CHARS);
        assert_eq!(
            texts(&m)[8..],
            [
                format!("Add Tab to Group \u{201c}{cut}\u{201d}"),
                format!("Add Tab to Group \u{201c}{exact}\u{201d}"),
            ]
        );
        assert_eq!(m.action_at(8), Some(MenuAction::AddToGroup(GroupId(1))));
        // A name with a brace is text, not a placeholder.
        let braces = MenuGroups {
            own: None,
            open: vec![(GroupId(1), "{0} {1}".to_owned())],
        };
        assert_eq!(
            texts(&tab_menu(true, &braces))[8],
            "Add Tab to Group \u{201c}{0} {1}\u{201d}"
        );
    }

    #[test]
    fn a_menus_debug_shows_no_group_name() {
        let g = MenuGroups {
            own: None,
            open: vec![(GroupId(1), "hunter2".to_owned())],
        };
        for shown in [format!("{g:?}"), format!("{:?}", tab_menu(true, &g))] {
            assert!(!shown.contains("hunter2"), "{shown}");
        }
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
            MenuKind::App { pane: false },
            MenuKind::App { pane: true },
            MenuKind::Terminal { selection: true },
            MenuKind::Terminal { selection: false },
            MenuKind::Tab { closable: true },
            MenuKind::Tab { closable: false },
        ];
        let with_groups = [
            (MenuKind::Tab { closable: true }, groups(None)),
            (MenuKind::Tab { closable: true }, groups(Some(1))),
            (MenuKind::Group { collapsed: true }, groups(Some(1))),
            (MenuKind::Group { collapsed: false }, groups(Some(1))),
        ];
        let kinds = kinds
            .into_iter()
            .map(|k| (k, MenuGroups::default()))
            .chain(with_groups);
        for (kind, g) in kinds {
            let m = MenuModel::with_groups(kind, &g, &crate::strings::english(), true);
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
        assert_eq!(tab_op(AddToNewGroup, 7), Some(TabOp::AddToNewGroup(7)));
        assert_eq!(
            tab_op(AddToGroup(GroupId(3)), 7),
            Some(TabOp::AddToGroup(7, GroupId(3)))
        );
        assert_eq!(tab_op(RemoveFromGroup, 7), Some(TabOp::RemoveFromGroup(7)));
        for other in [
            NewTab,
            NewTabGroup,
            EditGroup,
            CloseGroup,
            ShowSidePane,
            HideSidePane,
            Settings,
            About,
            Copy,
            Paste,
            SelectAll,
            Clear,
        ] {
            assert_eq!(
                tab_op(other, 7),
                None,
                "{other:?} is no row of the tab menu"
            );
        }
        // every row of the menu maps: to the tab, or to the tab's group
        for kind in [
            MenuKind::Tab { closable: true },
            MenuKind::Tab { closable: false },
        ] {
            for g in [groups(None), groups(Some(2))] {
                for a in actions(kind, &g).into_iter().flatten() {
                    let own = g.own.unwrap_or(GroupId::NONE);
                    assert!(
                        tab_op(a, 7).is_some() != group_op(a, own).is_some(),
                        "{a:?} maps to one of the two"
                    );
                }
            }
        }
    }
}
