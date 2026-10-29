//! Settings page. Copy and structure follow Lunegit's `TerminalSettingsView.axaml` and its
//! `en.json` strings; Appearance adds Luneterm's glass options.

use crate::paint::{LayerId, Rect};
use crate::state::{Hit, Page, Popup, Section, SettingId, State};
use crate::terminal_view::{self, glass};
use crate::text::{Spec, Style};
use crate::theme::*;
use crate::widgets::{self, P, label};

enum Control {
    Toggle,
    Combo,
    Stepper,
    Field(&'static str),
}

struct Item {
    id: SettingId,
    title: &'static str,
    desc: &'static str,
    control: Control,
}

struct Group {
    header: &'static str,
    items: Vec<Item>,
}

fn item(id: SettingId, title: &'static str, desc: &'static str, control: Control) -> Item {
    Item {
        id,
        title,
        desc,
        control,
    }
}

fn groups(section: Section) -> Vec<Group> {
    use Control::*;
    use SettingId::*;
    match section {
        Section::Appearance => vec![
            Group {
                header: "Window glass",
                items: vec![
                    item(
                        GlassStyle,
                        "Glass style",
                        "The material behind the window. Lunegit matches Lunegit's terminal exactly. Liquid Glass needs macOS 26 or later. Off uses a solid dark background, which is what Linux always gets.",
                        Combo,
                    ),
                    item(
                        KeepGlassInactive,
                        "Keep glass when the window is inactive",
                        "Lunegit switches to a solid background when its window loses focus. Turn this on to keep the glass while you work in another app.",
                        Toggle,
                    ),
                ],
            },
            Group {
                header: "Animations",
                items: vec![item(
                    Motion,
                    "Motion level",
                    "Controls UI motion. Reduced uses fades only; None disables animation.",
                    Combo,
                )],
            },
        ],
        Section::Terminal => vec![
            Group {
                header: "Appearance",
                items: vec![
                    item(
                        Font,
                        "Font",
                        "Typeface used to render terminal text.",
                        Combo,
                    ),
                    item(FontSize, "Font size", "Text size in the terminal.", Stepper),
                    item(
                        Palette,
                        "Color theme",
                        "ANSI color palette for terminal text. Background stays fixed. Applies to newly opened terminal tabs.",
                        Combo,
                    ),
                    item(
                        AnimateOutline,
                        "Animate the outline while a command runs",
                        "Animates the terminal card's outline while a command is executing, so you can see at a glance that the terminal is busy. Requires shell integration (zsh, bash and PowerShell).",
                        Toggle,
                    ),
                    item(
                        OutlineStyle,
                        "Running animation",
                        "Which animation the outline runs. The app-wide motion setting overrides this: Reduced forces Breath, and None keeps the outline static.",
                        Combo,
                    ),
                    item(
                        OutlineFullScreen,
                        "Also animate during full-screen apps",
                        "Full-screen apps like vim, less and top — and inline interactive apps like Claude Code, a REPL, or an ssh session — keep the shell's command marker set for their whole session, so the outline would animate the entire time. Off by default: the outline stays static while they are in the foreground. Apps that report progress (OSC 9;4) still animate it either way.",
                        Toggle,
                    ),
                ],
            },
            Group {
                header: "Session",
                items: vec![
                    item(
                        Scrollback,
                        "Scrollback length",
                        "Number of lines kept in the terminal scrollback buffer. Applies to new tabs.",
                        Stepper,
                    ),
                    item(
                        DefaultCwd,
                        "Default working directory",
                        "Directory used when opening a new terminal tab. Leave blank to use your home directory.",
                        Field("e.g. /home/user or C:\\Users\\user"),
                    ),
                    item(
                        Shell,
                        "Shell",
                        "Executable used for new terminals. Leave empty to auto-detect.",
                        Field("e.g. /bin/zsh"),
                    ),
                    item(
                        ShellArgs,
                        "Shell arguments",
                        "Arguments passed to the shell. Quotes are supported.",
                        Field(""),
                    ),
                    item(
                        ReportITerm2,
                        "Report as iTerm2 for compatibility",
                        "Luneterm normally identifies itself honestly (TERM_PROGRAM=Luneterm). Turn this on to report iTerm2 instead, which unlocks features some apps enable only for iTerm2 — but also invites apps to assume iTerm2 protocols Luneterm does not implement, such as inline images. Applies to new terminal tabs; open tabs keep the identity they started with.",
                        Toggle,
                    ),
                ],
            },
            Group {
                header: "Behavior",
                items: vec![
                    item(
                        ConfirmClose,
                        "Confirm before closing a running tab",
                        "Show a confirmation dialog before closing a terminal tab with a running session.",
                        Toggle,
                    ),
                    item(
                        OscClipboard,
                        "Allow apps to write to the clipboard",
                        "Lets command-line programs in the terminal copy text to your system clipboard (OSC 52). Turn off to block clipboard writes from terminal apps.",
                        Toggle,
                    ),
                    item(
                        OptionAsMeta,
                        "Use Option as Meta key (macOS)",
                        "When on, Option+key sends Meta (Esc-prefixed) sequences, e.g. Option+B/F to move by word and Option+Delete to delete a word, instead of typing special characters. Has no effect on Windows and Linux, where Alt always acts as Meta.",
                        Toggle,
                    ),
                ],
            },
        ],
        Section::About => Vec::new(),
    }
}

fn control_width(c: &Control) -> f32 {
    match c {
        Control::Toggle => 40.0,
        Control::Combo => 230.0,
        Control::Stepper => 130.0,
        Control::Field(_) => 240.0,
    }
}

pub fn draw(p: &mut P<'_>, st: &mut State) {
    let Page::Settings(section) = st.page else {
        return;
    };
    p.on(LayerId::Page);
    let g = glass(st);
    let card = terminal_view::card_rect(p.window);
    // Lunegit's settings page uses the lighter panel material (`SettingsPageView.axaml`).
    p.acrylic(card, 8.0, g.panel, g.panel, g.noise);
    p.ring(
        card,
        8.0,
        1.0,
        if st.focused { BORDER } else { BORDER_SUBTLE },
        false,
    );
    p.hit(card, Hit::Block);

    // Navigation column.
    let nav = Rect::new(card.x + 10.0, card.y + 12.0, 190.0, card.h - 24.0);
    let back = Rect::new(nav.x, nav.y, nav.w, 32.0);
    if p.hovered(Hit::Back) {
        p.fill(back, ITEM_RADIUS, SURFACE_HOVER);
    }
    p.text_in(
        &widgets::icon(icon::ARROW_LEFT, 13.0, FG_SECONDARY),
        back,
        10.0,
    );
    p.text_in(&label("Back to terminal", FG_SECONDARY), back, 32.0);
    p.hit(back, Hit::Back);
    let heading = Spec::plain("SETTINGS", Style::ui(FONT_S, FG_TERTIARY).medium());
    p.text(&heading, nav.x + 10.0, back.bottom() + 16.0);
    let mut y = back.bottom() + 40.0;
    for (sec, name) in [
        (Section::Appearance, "Appearance"),
        (Section::Terminal, "Terminal"),
        (Section::About, "About"),
    ] {
        let r = Rect::new(nav.x, y, nav.w, 32.0);
        if sec == section {
            p.fill(r, ITEM_RADIUS, SURFACE_SELECTED);
        } else if p.hovered(Hit::Nav(sec)) {
            p.fill(r, ITEM_RADIUS, SURFACE_HOVER);
        }
        let color = if sec == section {
            FG_PRIMARY
        } else {
            FG_SECONDARY
        };
        p.text_in(&label(name, color), r, 12.0);
        p.hit(r, Hit::Nav(sec));
        y += 34.0;
    }
    let divider = Rect::new(nav.right() + 10.0, card.y + 12.0, 1.0, card.h - 24.0);
    p.fill(divider, 0.0, BORDER_SUBTLE);

    // Scrollable content.
    let view = Rect::new(
        divider.right() + 24.0,
        card.y + 1.0,
        card.right() - divider.right() - 48.0,
        card.h - 2.0,
    );
    let max_scroll = (st.content_h - view.h).max(0.0);
    st.scroll = st.scroll.clamp(0.0, max_scroll);
    p.set_clip(Some(view));
    let top = view.y + 20.0 - st.scroll;
    let title = match section {
        Section::Appearance => "Appearance",
        Section::Terminal => "Terminal",
        Section::About => "About",
    };
    p.text(
        &Spec::plain(title, Style::ui(FONT_L, FG_PRIMARY).medium()),
        view.x,
        top,
    );
    let mut y = top + 40.0;
    if section == Section::About {
        y = about(p, view, y);
    } else {
        for group in groups(section) {
            y = draw_group(p, st, view, y, &group) + 16.0;
        }
    }
    st.content_h = y + st.scroll - view.y + 4.0;
    p.set_clip(None);
}

fn draw_group(p: &mut P<'_>, st: &State, view: Rect, y: f32, group: &Group) -> f32 {
    let x = view.x;
    let w = view.w;
    let header = Spec::plain(group.header, Style::ui(FONT_M, FG_PRIMARY).medium());
    let mut cy = y + 14.0;
    p.text(&header, x + 16.0, cy);
    cy += 30.0;
    for it in &group.items {
        let cw = control_width(&it.control);
        let text_w = w - 8.0 - 24.0 - cw - 24.0;
        let title = Spec::plain(it.title, Style::ui(FONT_BASE, FG_PRIMARY));
        let desc = Spec::plain(it.desc, Style::ui(FONT_S + 0.5, FG_SECONDARY)).wrapped(text_w);
        let (_, th) = p.measure(&title);
        let (_, dh) = p.measure(&desc);
        let ctrl_h = if matches!(it.control, Control::Toggle) {
            20.0
        } else {
            32.0
        };
        let row_h = (th + 4.0 + dh).max(ctrl_h) + 20.0;
        let row = Rect::new(x + 4.0, cy, w - 8.0, row_h);
        let modal_open = st.popup.is_some() || st.modal.is_some();
        if !modal_open && row.contains(st.mouse) && view.contains(st.mouse) {
            p.fill(row, ITEM_RADIUS, SURFACE_HOVER);
        }
        p.text(&title, row.x + 12.0, row.y + 10.0);
        p.text(&desc, row.x + 12.0, row.y + 10.0 + th + 4.0);
        let c = Rect::new(
            row.right() - 12.0 - cw,
            row.y + (row_h - ctrl_h) / 2.0,
            cw,
            ctrl_h,
        );
        let s = &st.settings;
        match it.control {
            Control::Toggle => {
                let on = s.toggle(it.id).unwrap_or(false);
                widgets::toggle(p, c, on, p.hovered(Hit::Toggle(it.id)), Hit::Toggle(it.id));
            }
            Control::Combo => {
                let options = s.options(it.id);
                let value = options.get(s.selected(it.id)).map_or("", |o| o.0.as_str());
                let open = matches!(st.popup, Some(Popup::Combo { id, .. }) if id == it.id);
                let hit = Hit::Combo(it.id);
                widgets::combo(p, c, value, p.hovered(hit), open, hit);
            }
            Control::Stepper => {
                let v = s.number(it.id).unwrap_or(0);
                widgets::stepper(p, c, v, Hit::Step(it.id, -1), Hit::Step(it.id, 1));
            }
            Control::Field(placeholder) => {
                let text = match it.id {
                    SettingId::DefaultCwd => s.default_cwd.as_str(),
                    SettingId::Shell => s.shell.as_str(),
                    _ => s.shell_args.as_str(),
                };
                let focused = st.editing == Some(it.id);
                widgets::field(p, c, text, placeholder, focused, Hit::Field(it.id));
            }
        }
        cy += row_h + 2.0;
    }
    let panel = Rect::new(x, y, w, cy - y + 6.0);
    p.ring(panel, 8.0, 1.0, BORDER, false);
    panel.bottom()
}

fn about(p: &mut P<'_>, view: Rect, y: f32) -> f32 {
    let w = view.w.min(560.0);
    let mut y = y;
    p.text(
        &Spec::plain("Luneterm", Style::ui(FONT_M, FG_PRIMARY).medium()),
        view.x,
        y,
    );
    y += 22.0;
    for line in [
        format!("Version {} · glass UI spike (mock data, no shell)", env!("CARGO_PKG_VERSION")),
        "A standalone terminal with Lunegit's glass look. Its engine will be a test-driven Rust port of xterm.js 6.0.0.".to_owned(),
    ] {
        let spec = Spec::plain(line, Style::ui(FONT_BASE, FG_SECONDARY)).wrapped(w);
        y += p.text(&spec, view.x, y).1 + 8.0;
    }
    y += 12.0;
    p.text(
        &Spec::plain(
            "Third-party notices",
            Style::ui(FONT_M, FG_PRIMARY).medium(),
        ),
        view.x,
        y,
    );
    y += 26.0;
    for (name, lic) in [
        ("xterm.js 6.0.0 (engine to be ported)", "MIT"),
        ("Avalonia acrylic noise and colour formulas", "MIT"),
        ("RobotoMono Nerd Font", "Apache 2.0 / MIT"),
        ("RznSans", "OFL 1.1"),
        ("winit, wgpu, glyphon, window-vibrancy", "MIT / Apache 2.0"),
    ] {
        p.text(&label(name, FG_PRIMARY), view.x, y);
        p.text(&label(lic, FG_TERTIARY), view.x + w - 140.0, y);
        y += 24.0;
    }
    y
}
