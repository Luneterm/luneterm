//! Fake terminal content for the UI spike. No shell, no VT engine.

/// A colour reference resolved against the active palette at draw time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ink {
    Fg,
    Ansi(u8),
}

const FG: Ink = Ink::Fg;
const RED: Ink = Ink::Ansi(1);
const GREEN: Ink = Ink::Ansi(2);
const YELLOW: Ink = Ink::Ansi(3);
const BLUE: Ink = Ink::Ansi(4);
const MAGENTA: Ink = Ink::Ansi(5);
const CYAN: Ink = Ink::Ansi(6);
const BRIGHT_BLACK: Ink = Ink::Ansi(8);
const WHITE: Ink = Ink::Ansi(15);

#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub fg: Ink,
    pub bold: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Line {
    pub spans: Vec<Span>,
}

/// A clickable URL, in cell coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Link {
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
}

#[derive(Debug, Clone)]
pub struct Screen {
    pub lines: Vec<Line>,
    pub cursor: (usize, usize),
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Shell,
    Build,
    Agent,
}

struct Builder {
    lines: Vec<Line>,
    links: Vec<Link>,
}

impl Builder {
    fn new() -> Self {
        Self {
            lines: vec![Line::default()],
            links: Vec::new(),
        }
    }

    fn push(&mut self, text: &str, fg: Ink, bold: bool) -> &mut Self {
        if let Some(line) = self.lines.last_mut() {
            line.spans.push(Span {
                text: text.to_owned(),
                fg,
                bold,
            });
        }
        self
    }

    fn t(&mut self, text: &str) -> &mut Self {
        self.push(text, FG, false)
    }

    fn c(&mut self, text: &str, fg: Ink) -> &mut Self {
        self.push(text, fg, false)
    }

    fn b(&mut self, text: &str, fg: Ink) -> &mut Self {
        self.push(text, fg, true)
    }

    fn link(&mut self, url: &str) -> &mut Self {
        let row = self.lines.len() - 1;
        let col_start = self.width();
        self.links.push(Link {
            row,
            col_start,
            col_end: col_start + url.chars().count(),
        });
        self.push(url, BLUE, false)
    }

    fn width(&self) -> usize {
        self.lines
            .last()
            .map_or(0, |l| l.spans.iter().map(|s| s.text.chars().count()).sum())
    }

    fn nl(&mut self) -> &mut Self {
        self.lines.push(Line::default());
        self
    }

    fn prompt(&mut self) -> &mut Self {
        self.c("~/Projects/Luneterm", CYAN)
            .t(" ")
            .c("\u{e0a0} main", MAGENTA)
            .b(" ❯ ", GREEN)
    }

    fn finish(self) -> Screen {
        let row = self.lines.len() - 1;
        let col = self.width();
        Screen {
            lines: self.lines,
            cursor: (row, col),
            links: self.links,
        }
    }
}

pub fn screen(kind: Kind) -> Screen {
    let mut s = Builder::new();
    match kind {
        Kind::Shell => {
            s.c("Last login: Thu Oct 29 13:27:04 on ttys002", BRIGHT_BLACK)
                .nl();
            s.prompt().t("ls -la").nl();
            s.t("total 48").nl();
            for (perm, size, name, dir) in [
                ("drwxr-xr-x", " 448", ".", true),
                ("drwxr-xr-x", "1760", "..", true),
                ("-rw-r--r--", "1121", "Cargo.toml", false),
                ("-rw-r--r--", "3054", "CLAUDE.md", false),
                ("drwxr-xr-x", " 256", "crates", true),
                ("drwxr-xr-x", " 224", "docs", true),
                ("drwxr-xr-x", "  96", "spikes", true),
            ] {
                s.t(perm).t("  8 ijat  staff  ").t(size).t(" Oct 29 14:02 ");
                if dir {
                    s.b(name, BLUE);
                } else {
                    s.t(name);
                }
                s.nl();
            }
            s.prompt().t("git log --oneline --graph -5").nl();
            for (hash, refs, msg) in [
                (
                    "4f2a9c1",
                    Some("(HEAD -> main)"),
                    "glass: port Avalonia acrylic math",
                ),
                ("9b1e7d0", None, "spike: macOS glass window"),
                (
                    "51c08aa",
                    None,
                    "docs: feasibility, porting guide, bug ledger",
                ),
                (
                    "0d3e6f2",
                    None,
                    "vt: extension registry (newest-first OSC dispatch)",
                ),
                ("a7719b4", None, "chore: cargo workspace scaffold"),
            ] {
                s.c("* ", RED).c(hash, YELLOW).t(" ");
                if let Some(r) = refs {
                    s.b(r, CYAN).t(" ");
                }
                s.t(msg).nl();
            }
            s.prompt().t("echo \"⌘-click: https://xtermjs.org\"").nl();
            s.t("⌘-click: ").link("https://xtermjs.org").nl();
            s.prompt();
        }
        Kind::Build => {
            s.prompt().t("cargo build --release").nl();
            for (name, ver) in [
                ("proc-macro2", "v1.0.95"),
                ("unicode-ident", "v1.0.18"),
                ("bitflags", "v2.9.1"),
                ("objc2", "v0.6.1"),
                ("wgpu-types", "v30.0.1"),
                ("naga", "v30.0.1"),
                ("cosmic-text", "v0.19.0"),
                ("wgpu-core", "v30.0.1"),
                ("glyphon", "v0.12.0"),
                (
                    "luneterm-glass",
                    "v0.1.0 (~/Projects/Luneterm/crates/luneterm-glass)",
                ),
            ] {
                s.b("   Compiling ", GREEN).t(name).t(" ").t(ver).nl();
            }
            s.c("warning", YELLOW)
                .b(": unused variable: `frame`", WHITE)
                .nl();
            s.c("  --> ", BLUE)
                .t("spikes/glass-ui/src/main.rs:212:13")
                .nl();
            s.b("    Building ", CYAN)
                .t("[=====================>     ] 187/214: wgpu-core, naga");
        }
        Kind::Agent => {
            const INNER: usize = 50;
            let rule = "─".repeat(INNER);
            s.c(&format!("╭{rule}╮"), BRIGHT_BLACK).nl();
            for (lead, text, fg, bold) in [
                (" ✻ ", "Glass UI spike", WHITE, true),
                (
                    "   ",
                    "/help for help · /status for setup",
                    BRIGHT_BLACK,
                    false,
                ),
                ("   ", "cwd: ~/Projects/Luneterm", BRIGHT_BLACK, false),
            ] {
                let pad = INNER - lead.chars().count() - text.chars().count();
                s.c("│", BRIGHT_BLACK).b(lead, RED).push(text, fg, bold);
                s.c(&format!("{}│", " ".repeat(pad)), BRIGHT_BLACK).nl();
            }
            s.c(&format!("╰{rule}╯"), BRIGHT_BLACK).nl();
            s.nl();
            s.b(" > ", WHITE)
                .t("build the glass UI on macOS first ✅")
                .nl();
            s.nl();
            s.c(" ⏺ ", GREEN).t("Read TerminalPanelView.axaml").nl();
            s.c("   ⎿  ", BRIGHT_BLACK)
                .c("412 lines", BRIGHT_BLACK)
                .nl();
            s.c(" ⏺ ", GREEN)
                .t("Ported Avalonia acrylic math 🎨 (4 tests pass)")
                .nl();
            s.c(" ⏺ ", YELLOW)
                .t("Wide glyphs: 你好世界 · emoji 👍🏽 · symbols ✓ ⚠️ ★")
                .nl();
            s.nl();
            s.c("  ? for shortcuts", BRIGHT_BLACK);
        }
    }
    s.finish()
}
