//! Tests for the keyboard protocol state (plan M7 step 3). Ported from Lunegit's
//! `XtermJsVtEngineKeyboardProtocolTests.cs` (22 Fact; real xterm.js in V8 with literal
//! expectations), names kept recognisable in snake case, literals as in Lunegit:
//!  - ported (20): every case but the two below, including `Ris_AlsoResetsTerminalModes`,
//!    which reads `Modes::application_cursor_keys` where Lunegit generated a key sequence.
//!  - moved to step 5 (the key encoder): `GenerateKeyEvent_FollowsTheLiveProtocolState` and
//!    `GenerateKeyEvent_UsesTheActiveScreensFlags`.
//!
//! Added here (not in Lunegit's suite): the handler return values (which decide whether xterm's
//! handler still runs), the `Modes` fields, `reset_keyboard_protocol`, `Terminal::reset`,
//! chunk invariance, the bounds on the state, and `disable_stdin`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::engine::HostEvent;
use crate::headless::terminal::Terminal;
use crate::parser::types::FunctionIdentifier;
use crate::services::options_service::OptionValue;

const ESC: &str = "\u{1b}";

/// A terminal with the standard extensions plus a capture of every reply of the next write.
struct Harness {
    term: Terminal,
}

impl Harness {
    fn new() -> Self {
        let mut term = Terminal::new([OptionValue::Cols(40), OptionValue::Rows(4)]);
        term.install_standard_extensions(&StandardExtensions::default())
            .unwrap();
        Harness { term }
    }

    /// Writes `chunk` and returns the host replies it produced, concatenated.
    fn write(&mut self, chunk: &str) -> String {
        self.term.write_str(chunk);
        self.term
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                HostEvent::Response(bytes) => Some(String::from_utf8(bytes).unwrap()),
                _ => None,
            })
            .collect()
    }

    /// `CSI ? u`, returning the reported flags.
    fn query_kitty(&mut self) -> u32 {
        let reply = self.write(&format!("{ESC}[?u"));
        let digits = reply
            .strip_prefix(&format!("{ESC}[?"))
            .and_then(|r| r.strip_suffix('u'))
            .unwrap_or_else(|| panic!("no kitty reply: {reply:?}"));
        digits.parse().unwrap()
    }

    /// `CSI ? 4 m`, returning the reported modifyOtherKeys level.
    fn query_modify_other_keys(&mut self) -> u32 {
        let reply = self.write(&format!("{ESC}[?4m"));
        let digits = reply
            .strip_prefix(&format!("{ESC}[>4;"))
            .and_then(|r| r.strip_suffix('m'))
            .unwrap_or_else(|| panic!("no modifyOtherKeys reply: {reply:?}"));
        digits.parse().unwrap()
    }
}

// ---- 1. CSI ? u query ------------------------------------------------------------------------

#[test]
fn kitty_query_bare_form_replies_with_zero_flags() {
    let mut h = Harness::new();
    assert_eq!(h.write(&format!("{ESC}[?u")), format!("{ESC}[?0u"));
    // `CSI ? 0 u` arrives as the same [0] params: the same query.
    assert_eq!(h.write(&format!("{ESC}[?0u")), format!("{ESC}[?0u"));
}

#[test]
fn kitty_query_with_non_zero_param_stays_silent() {
    let mut h = Harness::new();
    assert_eq!(h.write(&format!("{ESC}[?5u")), "");
    assert_eq!(h.write(&format!("{ESC}[?0;1u")), "");
    // positive re-check: the handler still exists (not a vacuous pass)
    assert_eq!(h.write(&format!("{ESC}[?u")), format!("{ESC}[?0u"));
}

// ---- 2. push / set / OR / AND-NOT, masked to {1,4} -------------------------------------------

#[test]
fn push_stores_flags_masked_to_supported_set() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>5u"));
    assert_eq!(h.query_kitty(), 5);
    // 2, 8 and 16 are unsupported: masked away so the reply stays honest
    h.write(&format!("{ESC}[>31u"));
    assert_eq!(h.query_kitty(), 5);
}

#[test]
fn set_or_and_not_modify_the_current_entry() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[=31u")); // mode omitted = 1 (set), masked
    assert_eq!(h.query_kitty(), 5);

    h.write(&format!("{ESC}[=4u"));
    assert_eq!(h.query_kitty(), 4);
    h.write(&format!("{ESC}[=1;2u")); // OR
    assert_eq!(h.query_kitty(), 5);

    h.write(&format!("{ESC}[=4;3u")); // AND-NOT
    assert_eq!(h.query_kitty(), 1);

    h.write(&format!("{ESC}[=5;0u")); // mode 0 = 1 (set)
    assert_eq!(h.query_kitty(), 5);
}

#[test]
fn set_unknown_mode_leaves_flags_unchanged() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[=5u"));
    h.write(&format!("{ESC}[=1;4u"));
    h.write(&format!("{ESC}[=0;99u"));
    assert_eq!(h.query_kitty(), 5);
}

// ---- 3. kitty stack model ----------------------------------------------------------------------

#[test]
fn set_then_push_then_pop_keeps_the_set_entry_kitty_model() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[=3u")); // creates the entry; stored masked: 3 & 0b101 = 1
    h.write(&format!("{ESC}[>5u"));
    h.write(&format!("{ESC}[<u"));
    // under kitty's model the set entry survives push+pop (xterm.js master's would leave 0)
    assert_eq!(h.query_kitty(), 1);
}

// ---- 4. pop edge cases + bounded stack ---------------------------------------------------------

#[test]
fn pop_on_empty_stack_is_zero_and_pop_zero_pops_one() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[<u"));
    assert_eq!(h.query_kitty(), 0);

    h.write(&format!("{ESC}[>1u"));
    h.write(&format!("{ESC}[>5u"));
    h.write(&format!("{ESC}[<0u"));
    assert_eq!(h.query_kitty(), 1);

    h.write(&format!("{ESC}[<99u")); // popping past empty empties the stack
    assert_eq!(h.query_kitty(), 0);
}

#[test]
fn stack_is_bounded_to_eight_entries_evicting_the_oldest() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>4u")); // the entry that must be evicted
    for _ in 0..8 {
        h.write(&format!("{ESC}[>1u")); // 9 pushes in total
    }
    for _ in 0..7 {
        h.write(&format!("{ESC}[<u"));
    }
    assert_eq!(h.query_kitty(), 1);
    h.write(&format!("{ESC}[<u"));
    // an unbounded stack would now expose the oldest push (4); bounded to 8, it was evicted
    assert_eq!(h.query_kitty(), 0);
}

#[test]
fn stack_depth_stays_at_eight_whatever_is_pushed() {
    let mut h = Harness::new();
    for i in 0..1000 {
        h.write(&format!("{ESC}[>{}u", i % 8));
    }
    let keyboard = &h.term.core().input_handler().ext.keyboard;
    assert_eq!(keyboard, &{
        // 8 entries left on main: the last eight pushes (992..=999 -> 0..=7, masked)
        let mut expected = KeyboardProtocol::default();
        for i in 992..1000 {
            expected.push(false, i % 8);
        }
        expected
    });
}

// ---- 5. separate stacks per screen -------------------------------------------------------------

#[test]
fn main_and_alternate_screens_have_separate_stacks() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>1u"));
    h.write(&format!("{ESC}[?1049h"));
    assert_eq!(h.query_kitty(), 0);

    h.write(&format!("{ESC}[>5u"));
    h.write(&format!("{ESC}[?1049l"));
    assert_eq!(h.query_kitty(), 1);

    h.write(&format!("{ESC}[?1049h"));
    assert_eq!(h.query_kitty(), 5);
}

#[test]
fn screen_switch_mid_chunk_is_honoured_per_sequence() {
    let mut h = Harness::new();
    // one write: push on main, switch, query on alt, switch back, query on main
    let replies = h.write(&format!(
        "{ESC}[>1u{ESC}[?1049h{ESC}[?u{ESC}[?1049l{ESC}[?u"
    ));
    assert_eq!(replies, format!("{ESC}[?0u{ESC}[?1u"));
}

#[test]
fn alternate_screen_47_and_1047_select_the_alternate_stack_too() {
    for mode in [47, 1047] {
        let mut h = Harness::new();
        h.write(&format!("{ESC}[>1u{ESC}[?{mode}h"));
        assert_eq!(h.query_kitty(), 0, "{mode}");
        h.write(&format!("{ESC}[>4u{ESC}[?{mode}l"));
        assert_eq!(h.query_kitty(), 1, "{mode}");
    }
}

// ---- 6. RIS / DECSTR ----------------------------------------------------------------------------

#[test]
fn ris_clears_both_stacks_and_modify_other_keys_and_still_resets_the_screen() {
    let mut h = Harness::new();
    h.write(&format!("hello{ESC}[1m"));
    h.write(&format!("{ESC}[>1u{ESC}[?1049h{ESC}[>5u{ESC}[>4;2m"));

    h.write(&format!("{ESC}c"));

    // xterm.js's own fullReset still ran: back on the main screen, grid cleared, cursor homed
    let vp = h.term.viewport();
    assert!(!vp.alt_screen);
    assert_eq!((vp.cursor.x, vp.cursor.y), (0, 0));
    assert_eq!(vp.cells[0][0].text, "");

    assert_eq!(h.query_kitty(), 0);
    assert_eq!(h.query_modify_other_keys(), 0);
    h.write(&format!("{ESC}[?1049h"));
    assert_eq!(h.query_kitty(), 0);
    h.write(&format!("{ESC}[?1049l"));

    // handlers survive the reset
    h.write(&format!("{ESC}[>1u"));
    assert_eq!(h.query_kitty(), 1);
}

#[test]
fn ris_also_resets_terminal_modes() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[?1h")); // DECCKM
    assert!(h.term.modes().application_cursor_keys);
    h.write(&format!("{ESC}c"));
    assert!(!h.term.modes().application_cursor_keys);
}

#[test]
fn decstr_clears_both_stacks_and_modify_other_keys() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>1u{ESC}[?1049h{ESC}[>5u{ESC}[>4;2m"));

    h.write(&format!("{ESC}[!p"));

    assert_eq!(h.query_kitty(), 0);
    assert_eq!(h.query_modify_other_keys(), 0);
    h.write(&format!("{ESC}[?1049l"));
    assert_eq!(h.query_kitty(), 0);
}

#[test]
fn decstr_still_runs_xterm_soft_reset() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[?25l")); // hide cursor; DECSTR shows it again (xterm.js softReset)
    assert!(!h.term.viewport().cursor.visible);

    h.write(&format!("{ESC}[!p"));

    assert!(h.term.viewport().cursor.visible);
}

#[test]
fn alt_screen_switches_reset_nothing() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>5u{ESC}[>4;2m"));
    h.write(&format!("{ESC}[?1049h{ESC}[?1049l{ESC}[?1049h{ESC}[?1049l"));
    assert_eq!(h.query_kitty(), 5);
    assert_eq!(h.query_modify_other_keys(), 2);
}

#[test]
fn terminal_reset_and_the_host_hook_clear_the_state() {
    let mut h = Harness::new();
    let set = format!("{ESC}[>1u{ESC}[?1049h{ESC}[>5u{ESC}[>4;2m{ESC}[?1049l");
    h.write(&set);
    h.term.reset();
    assert_eq!((h.query_kitty(), h.query_modify_other_keys()), (0, 0));
    h.write(&format!("{ESC}[?1049h"));
    assert_eq!(h.query_kitty(), 0);
    h.write(&format!("{ESC}[?1049l"));

    h.write(&set);
    assert_eq!(h.query_kitty(), 1);
    h.term.reset_keyboard_protocol();
    assert_eq!((h.query_kitty(), h.query_modify_other_keys()), (0, 0));
    h.write(&format!("{ESC}[?1049h"));
    assert_eq!(h.query_kitty(), 0);
    // the hook resets the protocol state only
    h.write("abc");
    h.term.reset_keyboard_protocol();
    assert_eq!(h.term.buffers().active().x, 3);
}

// ---- 7. XTMODKEYS / XTQMODKEYS ------------------------------------------------------------------

#[test]
fn modify_other_keys_set_query_clamp_and_reset() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>4;2m"));
    assert_eq!(h.write(&format!("{ESC}[?4m")), format!("{ESC}[>4;2m"));

    h.write(&format!("{ESC}[>4;3m")); // clamped
    assert_eq!(h.query_modify_other_keys(), 2);

    h.write(&format!("{ESC}[>4m")); // Pv omitted -> 0
    assert_eq!(h.query_modify_other_keys(), 0);

    h.write(&format!("{ESC}[>4;1m"));
    h.write(&format!("{ESC}[>m")); // resource omitted -> reset
    assert_eq!(h.query_modify_other_keys(), 0);

    h.write(&format!("{ESC}[>4;2m"));
    h.write(&format!("{ESC}[>4n")); // disable
    assert_eq!(h.query_modify_other_keys(), 0);
}

#[test]
fn modify_other_keys_other_resources_are_ignored_and_other_queries_silent() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>4;2m"));

    assert_eq!(h.write(&format!("{ESC}[>1;2m")), "");
    assert_eq!(h.query_modify_other_keys(), 2);

    assert_eq!(h.write(&format!("{ESC}[>1n")), "");
    assert_eq!(h.query_modify_other_keys(), 2);

    assert_eq!(h.write(&format!("{ESC}[?0m")), "");
    assert_eq!(h.write(&format!("{ESC}[?1m")), "");
}

#[test]
fn xtqmodkeys_answers_once_per_resource_4() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>4;1m"));
    assert_eq!(
        h.write(&format!("{ESC}[?4;1;4m")),
        format!("{ESC}[>4;1m{ESC}[>4;1m")
    );
}

// ---- 8. sub-param forms are ignored ------------------------------------------------------------

#[test]
fn sub_param_forms_change_no_state() {
    let mut h = Harness::new();
    h.write(&format!("{ESC}[>4;1m"));

    h.write(&format!("{ESC}[>1:2u"));
    h.write(&format!("{ESC}[=5:1u"));
    h.write(&format!("{ESC}[>4;2:1m"));
    h.write(&format!("{ESC}[>4:1n"));

    assert_eq!(h.query_kitty(), 0);
    assert_eq!(h.query_modify_other_keys(), 1);
    assert_eq!(h.write(&format!("{ESC}[?0:1u")), "");
    assert_eq!(h.write(&format!("{ESC}[?4:1m")), "");
}

#[test]
fn huge_params_are_masked_and_clamped() {
    let mut h = Harness::new();
    // xterm's Params caps values at int32 max; the stored state must stay inside its bounds
    h.write(&format!("{ESC}[>99999999999u"));
    assert_eq!(h.query_kitty(), 2_147_483_647 & 5);

    h.write(&format!("{ESC}[>4;99999999999m"));
    assert_eq!(h.query_modify_other_keys(), 2);

    // pop and set with the largest values neither overflow nor leave the bounds
    h.write(&format!("{ESC}[<99999999999u"));
    assert_eq!(h.query_kitty(), 0);
    h.write(&format!("{ESC}[=2147483647;2147483647u"));
    h.write(&format!("{ESC}[=2147483647;3u"));
    assert_eq!(h.query_kitty(), 0);
    h.write(&format!("{ESC}[=2147483647;2u"));
    assert_eq!(h.query_kitty(), 5);
}

// ---- 9. reply ordering ---------------------------------------------------------------------------

#[test]
fn kitty_reply_and_da1_reply_keep_stream_order() {
    let mut h = Harness::new();

    let forward = h.write(&format!("{ESC}[?u{ESC}[c"));
    assert!(
        forward.starts_with(&format!("{ESC}[?0u{ESC}[?")) && forward.ends_with('c'),
        "{forward:?}"
    );

    let reverse = h.write(&format!("{ESC}[c{ESC}[?u"));
    assert!(reverse.ends_with(&format!("c{ESC}[?0u")), "{reverse:?}");
}

// ---- 10. nothing reaches the grid ------------------------------------------------------------------

#[test]
fn protocol_sequences_print_nothing_to_the_grid() {
    let mut h = Harness::new();
    h.write(&format!(
        "{ESC}[>5u{ESC}[=1;2u{ESC}[<u{ESC}[?u{ESC}[>4;2m{ESC}[?4m{ESC}[>4n"
    ));
    let vp = h.term.viewport();
    assert!(vp.cells[0].iter().all(|cell| cell.text.is_empty()));
    assert_eq!(vp.cursor.x, 0);
}

// ---- 11. added: Modes, return values, chunking, disable_stdin ------------------------------------

#[test]
fn modes_report_the_active_screens_flags_and_the_level() {
    let mut h = Harness::new();
    assert_eq!(
        (
            h.term.modes().kitty_keyboard_flags,
            h.term.modes().modify_other_keys
        ),
        (0, 0)
    );
    h.write(&format!("{ESC}[>5u{ESC}[>4;2m"));
    assert_eq!(
        (
            h.term.modes().kitty_keyboard_flags,
            h.term.modes().modify_other_keys
        ),
        (5, 2)
    );
    h.write(&format!("{ESC}[?1049h"));
    // separate stack, global level
    assert_eq!(
        (
            h.term.modes().kitty_keyboard_flags,
            h.term.modes().modify_other_keys
        ),
        (0, 2)
    );
    h.write(&format!("{ESC}[>1u"));
    assert_eq!(h.term.modes().kitty_keyboard_flags, 1);
    h.write(&format!("{ESC}[?1049l"));
    assert_eq!(h.term.modes().kitty_keyboard_flags, 5);
}

/// Each handler's return value decides whether xterm's own handler (here: a handler registered
/// before the install, so it is tried after the extension's) still runs.
#[test]
fn handler_return_values_match_driver_js() {
    let identifiers = [
        ('?', 'u'),
        ('>', 'u'),
        ('<', 'u'),
        ('=', 'u'),
        ('>', 'm'),
        ('>', 'n'),
        ('?', 'm'),
    ];
    // (sequence, consumed by the extension)
    let cases = [
        ("[?u", true),
        ("[?0u", true),
        ("[?5u", false),
        ("[?0;1u", false),
        ("[?0:1u", false),
        ("[>u", true),
        ("[>5u", true),
        ("[>1:2u", false),
        ("[<u", true),
        ("[<3u", true),
        ("[<1:2u", false),
        ("[=1u", true),
        ("[=1;2u", true),
        ("[=1;99u", true),
        ("[=5:1u", false),
        ("[>m", true),
        ("[>0m", true),
        ("[>4;2m", true),
        ("[>4m", true),
        ("[>5m", false),
        ("[>1;2m", false),
        ("[>4;2:1m", false),
        ("[>4n", true),
        ("[>1n", false),
        ("[>n", false),
        ("[>4:1n", false),
        ("[?4m", true),
        ("[?1;4m", true),
        ("[?0m", false),
        ("[?m", false),
        ("[?4:1m", false),
    ];
    for (sequence, consumed) in cases {
        let mut term = Terminal::new([OptionValue::Cols(40), OptionValue::Rows(4)]);
        let fell_through = Arc::new(AtomicU32::new(0));
        for (prefix, final_byte) in identifiers {
            let seen = Arc::clone(&fell_through);
            term.register_csi_handler(
                FunctionIdentifier::with_prefix(prefix, final_byte),
                move |_, _| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    true
                },
            )
            .unwrap();
        }
        term.install_standard_extensions(&StandardExtensions::default())
            .unwrap();
        term.write_str(&format!("{ESC}{sequence}"));
        assert_eq!(
            fell_through.load(Ordering::SeqCst),
            u32::from(!consumed),
            "{sequence:?}: consumed by the extension: {consumed}"
        );
    }
}

#[test]
fn decstr_handler_returns_false_so_older_handlers_run() {
    let mut term = Terminal::new([]);
    let seen = Arc::new(AtomicU32::new(0));
    let seen_in = Arc::clone(&seen);
    term.register_csi_handler(
        FunctionIdentifier::with_intermediates("!", 'p'),
        move |_, _| {
            seen_in.fetch_add(1, Ordering::SeqCst);
            false
        },
    )
    .unwrap();
    term.install_standard_extensions(&StandardExtensions::default())
        .unwrap();
    term.write_str(&format!("{ESC}[!p"));
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[test]
fn state_changes_are_chunk_invariant() {
    let stream = format!(
        "{ESC}[>1u{ESC}[?1049h{ESC}[>5u{ESC}[=4;3u{ESC}[>4;2m{ESC}[?u{ESC}[?4m{ESC}[c{ESC}[<u{ESC}[?1049l{ESC}[?u"
    );
    let mut whole = Harness::new();
    let whole_replies = whole.write(&stream);
    let mut bytes = Harness::new();
    let mut byte_replies = String::new();
    for b in stream.bytes() {
        bytes.term.write(&[b]);
        for event in bytes.term.drain_events() {
            if let HostEvent::Response(r) = event {
                byte_replies.push_str(&String::from_utf8(r).unwrap());
            }
        }
    }
    assert_eq!(whole_replies, byte_replies);
    assert_eq!(whole.term.modes(), bytes.term.modes());
    assert_eq!(
        whole.term.core().input_handler().ext.keyboard,
        bytes.term.core().input_handler().ext.keyboard
    );
}

#[test]
fn disable_stdin_suppresses_replies_but_not_state() {
    let mut h = Harness::new();
    h.term.set_option(OptionValue::DisableStdin(true)).unwrap();
    assert_eq!(
        h.write(&format!("{ESC}[>5u{ESC}[?u{ESC}[>4;2m{ESC}[?4m")),
        ""
    );
    assert_eq!(h.term.modes().kitty_keyboard_flags, 5);
    assert_eq!(h.term.modes().modify_other_keys, 2);
}

#[test]
fn nothing_is_handled_without_the_install() {
    let mut term = Terminal::new([]);
    term.write_str(&format!("{ESC}[>5u{ESC}[?u{ESC}[>4;2m{ESC}[?4m"));
    assert_eq!(term.drain_events(), []);
    assert_eq!(term.modes().kitty_keyboard_flags, 0);
    assert_eq!(term.modes().modify_other_keys, 0);
}
