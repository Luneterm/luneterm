//! `luneterm ctl`, tab control from programs: Protocol 1 (the orch contract) with its revision 3
//! amendment, `tab close <id> [--any-origin]`. The core both ends share: the grammar and limits
//! of the words ([`parse`]), the framing on the wire ([`wire`]) and the JSON lines of the replies
//! ([`reply`]); these open nothing, [`wire`] reads from the reader it is given. Around it:
//! [`endpoint`] (where an instance listens), [`server`] (the app's end), [`client`]
//! (`luneterm ctl`), [`token`] (what names an instance) and [`handler`] (what the app does with
//! a request).
//!
//! A request comes from any process of the same user, so it is untrusted: nothing here panics,
//! every length is checked before it is allocated, and input over a limit is refused, never cut.

mod args;
pub mod client;
pub mod endpoint;
pub mod handler;
pub mod reply;
pub mod server;
pub mod token;
pub mod wire;

use std::path::PathBuf;

pub use args::{
    MAX_GROUP_CHARS, MAX_PROGRAM_BYTES, MAX_PROGRAM_WORDS, MAX_TITLE_CHARS, Usage, label_ok, parse,
    token_ok, words,
};

use crate::tabs::TabId;

/// The protocol `ctl status` reports.
pub const PROTOCOL: i64 = 1;

/// How a `ctl` run ends: Protocol 1's exit codes. A reply's status byte is the same number
/// (every one but [`Code::Unreachable`], which only the client can find).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Code {
    /// Done.
    Done = 0,
    /// Any other failure, including no answer within 2 s.
    Failed = 1,
    /// Bad usage, or an invalid or over-limit argument.
    Usage = 2,
    /// Tab control is turned off in settings.
    ControlOff = 3,
    /// No running Luneterm could be reached.
    Unreachable = 4,
    /// No such tab.
    NoSuchTab = 5,
    /// Refused: the tab was not created through `ctl` and `--any-origin` was not given.
    Refused = 6,
}

/// One command, parsed and within every limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// `status`
    Status,
    /// `tab list`
    TabList,
    /// `tab new`
    TabNew {
        /// Absolute; whether it is a directory is the caller's to check.
        cwd: PathBuf,
        group: Option<String>,
        title: Option<String>,
        focus: bool,
        /// The program and its arguments; empty for the user's shell.
        argv: Vec<String>,
    },
    /// `tab close`
    TabClose { id: TabId, any_origin: bool },
    /// `tab focus`
    TabFocus(TabId),
    /// `tab group`: the tab and the group's name.
    TabGroup(TabId, String),
    /// `tab ungroup`
    TabUngroup(TabId),
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use super::wire::{MAX_WORDS, decode_request, encode_request};
    use super::*;

    const DEFAULT_SEEDS: u64 = 500;
    const CASES_PER_SEED: usize = 24;

    /// splitmix64: reproducible, no dependency.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % n.max(1) as u64) as usize
        }

        fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
            from[self.below(from.len())]
        }
    }

    /// Hands out the bytes in random pieces, as a socket may.
    struct Pieces<'a, 'r> {
        left: &'a [u8],
        rng: &'r mut Rng,
    }

    impl Read for Pieces<'_, '_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = (1 + self.rng.below(97)).min(buf.len()).min(self.left.len());
            let (head, tail) = self.left.split_at(n);
            buf[..n].copy_from_slice(head);
            self.left = tail;
            Ok(n)
        }
    }

    fn absolute() -> &'static str {
        if cfg!(windows) { r"C:\tmp" } else { "/tmp" }
    }

    const GRAMMAR: &[&str] = &[
        "status",
        "tab",
        "list",
        "new",
        "close",
        "focus",
        "group",
        "ungroup",
        "--cwd",
        "--group",
        "--title",
        "--focus",
        "--any-origin",
        "--",
        "--force",
        "--cwd=/tmp",
    ];
    const IDS: &[&str] = &[
        "0",
        "1",
        "7",
        "42",
        "18446744073709551615",
        "18446744073709551616",
        "007",
        "-1",
        "+1",
        "1x",
        "",
    ];

    fn text(r: &mut Rng) -> String {
        const CHARS: &[char] = &[
            'a', 'Z', '0', ' ', ' ', '-', '_', '/', '\\', '"', '\0', '\t', '\n', '\u{1b}',
            '\u{7f}', '\u{85}', '\u{a0}', 'é', '日', '🎉', '\u{200d}', '\u{200e}', '\u{202e}',
            '\u{2028}', '\u{2029}', '\u{2066}', '\u{3000}',
        ];
        let len = match r.below(40) {
            0 => 60 + r.below(80),
            1 => 4090 + r.below(10),
            2 if r.below(8) == 0 => 65_530 + r.below(12),
            _ => r.below(10),
        };
        // One char repeated for the long ones, so some of them pass the limits' checks.
        if len > 20 {
            return "q".repeat(len);
        }
        (0..len).map(|_| CHARS[r.below(CHARS.len())]).collect()
    }

    /// A name that is good half the time.
    fn name(r: &mut Rng) -> String {
        if r.below(2) == 0 {
            r.pick(&["a", "orch: x", "two  spaces", "--any-origin"])
                .to_owned()
        } else {
            text(r)
        }
    }

    fn word(r: &mut Rng) -> String {
        match r.below(5) {
            0 | 1 => r.pick(GRAMMAR).to_owned(),
            2 => r.pick(IDS).to_owned(),
            3 => absolute().to_owned(),
            _ => text(r),
        }
    }

    /// A command that is often right, then sometimes damaged.
    fn command(r: &mut Rng) -> Vec<String> {
        let owned = |w: &[&str]| w.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        let id = r.pick(IDS);
        let mut words = match r.below(8) {
            0 => owned(&["status"]),
            1 => owned(&["tab", "list"]),
            2 | 3 => {
                let mut words = owned(&["tab", "new", "--cwd", absolute()]);
                if r.below(2) == 0 {
                    words.extend(["--group".to_owned(), name(r)]);
                }
                if r.below(2) == 0 {
                    words.extend(["--title".to_owned(), name(r)]);
                }
                if r.below(3) == 0 {
                    words.push("--focus".to_owned());
                }
                if r.below(2) == 0 {
                    words.push("--".to_owned());
                    let n = match r.below(30) {
                        0 => 254 + r.below(5),
                        _ => 1 + r.below(5),
                    };
                    words.extend((0..n).map(|_| word(r)));
                }
                words
            }
            4 => match r.below(3) {
                0 => owned(&["tab", "close", id]),
                1 => owned(&["tab", "close", id, "--any-origin"]),
                _ => owned(&["tab", "close", "--any-origin", id]),
            },
            5 => owned(&["tab", "focus", id]),
            6 => {
                let mut words = owned(&["tab", "group", id]);
                words.push(name(r));
                words
            }
            _ => owned(&["tab", "ungroup", id]),
        };
        for _ in 0..r.below(8).saturating_sub(5) {
            let at = r.below(words.len() + 1);
            match r.below(3) {
                0 => words.insert(at, word(r)),
                1 if at < words.len() => {
                    words.remove(at);
                }
                _ if at < words.len() => words[at] = word(r),
                _ => words.push(word(r)),
            }
        }
        words
    }

    /// Bytes near a valid frame, or nowhere near one.
    fn frame(r: &mut Rng) -> Vec<u8> {
        let words = if r.below(50) == 0 {
            vec![String::new(); MAX_WORDS - 1 + r.below(2)]
        } else {
            command(r)
        };
        let mut bytes = encode_request(&words).unwrap_or_else(|_| b"LTC1".to_vec());
        for _ in 0..r.below(4) {
            let at = r.below(bytes.len() + 1);
            match r.below(6) {
                0 if at < bytes.len() => bytes[at] ^= 1 << r.below(8),
                1 => bytes.truncate(at),
                2 => bytes.extend((0..r.below(16)).map(|_| r.next_u64() as u8)),
                3 if at + 4 <= bytes.len() => {
                    let huge = [u32::MAX, 1 << 31, 131_072, 131_073, 301, 65_537][r.below(6)];
                    bytes[at..at + 4].copy_from_slice(&huge.to_le_bytes());
                }
                4 => bytes.insert(at, r.next_u64() as u8),
                _ => {
                    bytes.truncate(r.below(5));
                    bytes.extend((0..r.below(64)).map(|_| r.next_u64() as u8));
                }
            }
        }
        bytes
    }

    /// What one seed accepted: requests by kind, and frames.
    #[derive(Default)]
    struct Accepted {
        kinds: [usize; 7],
        frames: usize,
    }

    fn kind(request: &Request) -> usize {
        match request {
            Request::Status => 0,
            Request::TabList => 1,
            Request::TabNew { .. } => 2,
            Request::TabClose { .. } => 3,
            Request::TabFocus(_) => 4,
            Request::TabGroup(..) => 5,
            Request::TabUngroup(_) => 6,
        }
    }

    fn run(seed: u64, accepted: &mut Accepted) {
        let mut r = Rng(seed);
        for _ in 0..CASES_PER_SEED {
            // The parser: whatever it accepts goes over the wire as the same words and parses to
            // the same request on the far side.
            let words = command(&mut r);
            if let Ok(request) = parse(&words) {
                accepted.kinds[kind(&request)] += 1;
                let bytes = encode_request(&words).expect("an accepted request fits a frame");
                let mut pieces = Pieces {
                    left: &bytes,
                    rng: &mut r,
                };
                let read = decode_request(&mut pieces).expect("its own frame decodes");
                assert!(pieces.left.is_empty(), "seed {seed}: bytes left over");
                assert!(read == words, "seed {seed}: other words");
                assert!(parse(&read) == Ok(request), "seed {seed}: other request");
            }
            // The decoder: whatever it accepts re-encodes to the very bytes it consumed.
            let bytes = frame(&mut r);
            let mut pieces = Pieces {
                left: &bytes,
                rng: &mut r,
            };
            if let Ok(words) = decode_request(&mut pieces) {
                accepted.frames += 1;
                let consumed = bytes.len() - pieces.left.len();
                let again = encode_request(&words).expect("what decodes fits a frame");
                assert!(again == bytes[..consumed], "seed {seed}: other bytes");
                // Only that it does not panic: a damaged frame may hold any words.
                let _ = parse(&words);
            }
        }
    }

    #[test]
    fn a_seeded_fuzz_of_the_decoder_and_the_parser_never_panics_and_keeps_the_words() {
        let seeds = std::env::var("LUNETERM_CTL_FUZZ_SEEDS").map_or(DEFAULT_SEEDS, |v| {
            v.parse().expect("LUNETERM_CTL_FUZZ_SEEDS is a number")
        });
        let mut accepted = Accepted::default();
        for seed in 0..seeds {
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(seed, &mut accepted);
            }));
            assert!(ran.is_ok(), "seed {seed} failed");
        }
        // The generators do reach the accepting paths.
        if seeds >= DEFAULT_SEEDS {
            assert!(
                accepted.kinds.iter().all(|&n| n > 100),
                "{:?}",
                accepted.kinds
            );
            assert!(accepted.frames > 1000, "{}", accepted.frames);
        }
    }
}
