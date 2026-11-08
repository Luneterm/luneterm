//! Tests for `core_terminal.rs`, written from `CoreTerminal.ts` (upstream has no test file):
//! the resize clamp, each host duty (listener) the plan lists, the Windows heuristics and the
//! register forwards.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use super::*;
use crate::services::options_service::LogLevel;
use crate::services::options_service::{WindowOptions, WindowsPty};
use crate::services::osc_link_service::OscLinkData;

fn core(cols: usize, rows: usize, options: impl IntoIterator<Item = OptionValue>) -> CoreTerminal {
    let mut values = vec![OptionValue::Cols(cols), OptionValue::Rows(rows)];
    values.extend(options);
    CoreTerminal::new(values)
}

fn link(id: Option<&str>, uri: &str) -> OscLinkData {
    OscLinkData {
        id: id.map(str::to_owned),
        uri: uri.to_owned(),
    }
}

/// Registers `data` at the cursor of the active buffer, as the OSC 8 handler will.
fn register_link(t: &mut CoreTerminal, data: OscLinkData) -> u32 {
    let services = &mut t.input_handler_mut().services;
    services.osc_link.register_link(&mut services.buffer, data)
}

fn link_count(t: &CoreTerminal) -> usize {
    t.input_handler().services().osc_link.len()
}

fn is_wrapped(t: &CoreTerminal, row: usize) -> bool {
    t.buffers().active().lines.get(row).unwrap().is_wrapped
}

// --- resize clamp -------------------------------------------------------------------------

#[test]
fn resize_clamps_to_the_minimum_size_before_the_buffers_see_it() {
    let mut t = core(80, 24, []);
    t.resize(0, 0);
    assert_eq!((t.cols(), t.rows()), (2, 1));
    t.resize(1, 5);
    assert_eq!((t.cols(), t.rows()), (2, 5));
    // no hang, and the buffers took the clamped size
    t.write("ab文cd\r\nxyz".as_bytes());
    assert_eq!(t.buffers().active().lines.get(0).unwrap().len(), 2);
}

#[test]
fn resize_reports_full_damage() {
    let mut t = core(10, 5, []);
    t.input_handler_mut().take_damage();
    t.resize(12, 6);
    assert!(t.input_handler_mut().take_damage().full);
}

// --- marker disposal -> OscLinkService ------------------------------------------------------

#[test]
fn a_link_whose_line_is_trimmed_is_gone_after_write() {
    let mut t = core(10, 3, [OptionValue::Scrollback(2)]);
    let id = register_link(&mut t, link(None, "https://a"));
    assert_eq!(link_count(&t), 1);
    // 3 rows + 2 scrollback: the fifth line feed trims line 0
    t.write(b"\n\n\n\n");
    assert_eq!(link_count(&t), 1, "line 0 still in the buffer");
    t.write(b"\n");
    assert_eq!(link_count(&t), 0);
    assert!(
        t.input_handler()
            .services()
            .osc_link
            .get_link_data(id)
            .is_none()
    );
}

#[test]
fn re_registering_the_same_link_after_its_line_is_trimmed_in_the_same_write_gets_a_new_id() {
    let mut t = core(10, 3, [OptionValue::Scrollback(2)]);
    let ids = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&ids);
    // stands in for the OSC 8 handler: forward, then register (input_handler.rs module notes)
    t.register_osc_handler(9999, move |h, _| {
        h.forward_disposed_markers();
        let services = &mut h.services;
        let id = services
            .osc_link
            .register_link(&mut services.buffer, link(Some("x"), "https://a"));
        sink.lock().unwrap().push(id);
        true
    });

    // without trimming in between the link is reused
    t.write(b"\x1b]9999;\x07\n\x1b]9999;\x07");
    {
        let ids = ids.lock().unwrap();
        assert_eq!(ids[0], ids[1]);
    }

    // trimmed in the same write: upstream freed the link, so the same id;uri is a new link
    let mut t = core(10, 3, [OptionValue::Scrollback(2)]);
    let ids = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&ids);
    t.register_osc_handler(9999, move |h, _| {
        h.forward_disposed_markers();
        let services = &mut h.services;
        let id = services
            .osc_link
            .register_link(&mut services.buffer, link(Some("x"), "https://a"));
        sink.lock().unwrap().push(id);
        true
    });
    t.write(b"\x1b]9999;\x07\n\n\n\n\n\x1b]9999;\x07");
    let ids = ids.lock().unwrap();
    assert_ne!(ids[0], ids[1]);
    assert_eq!(link_count(&t), 1);
}

#[test]
fn an_alt_buffer_link_is_dropped_when_the_normal_buffer_is_activated() {
    let mut t = core(10, 3, []);
    let normal_link = register_link(&mut t, link(None, "https://normal"));
    {
        let services = &mut t.input_handler_mut().services;
        services.buffer.buffers.activate_alt_buffer(None).unwrap();
    }
    let alt_link = register_link(&mut t, link(None, "https://alt"));
    assert_eq!(link_count(&t), 2);
    {
        let services = &mut t.input_handler_mut().services;
        let opts = services.options.options();
        services
            .buffer
            .buffers
            .activate_normal_buffer(opts)
            .unwrap();
    }
    // the switch happened inside a write upstream; any public mutation forwards
    t.write(b"");
    let links = &t.input_handler().services().osc_link;
    assert!(links.get_link_data(alt_link).is_none());
    assert!(links.get_link_data(normal_link).is_some());
}

/// Step 7: the same through the escape sequences, inside one `write`: the alt buffer's link is
/// freed by `CSI ? 1049 l`, the normal buffer's stays.
#[test]
fn leaving_the_alt_screen_through_a_write_frees_the_alt_buffer_links() {
    let mut t = core(10, 3, []);
    let normal_link = register_link(&mut t, link(None, "https://normal"));
    t.write(b"\x1b[?1049h");
    let alt_link = register_link(&mut t, link(None, "https://alt"));
    assert_eq!(link_count(&t), 2);
    t.write(b"\x1b[?1049l");
    let links = &t.input_handler().services().osc_link;
    assert!(links.get_link_data(alt_link).is_none());
    assert!(links.get_link_data(normal_link).is_some());
    // and in one write with the entry
    t.write(b"\x1b[?47h");
    let alt_link = register_link(&mut t, link(None, "https://alt2"));
    t.write(b"x\x1b[?47l");
    let links = &t.input_handler().services().osc_link;
    assert!(links.get_link_data(alt_link).is_none());
}

/// Step 7: RIS inside a write frees the links of both buffers (assumed decision Q9).
#[test]
fn ris_inside_a_write_frees_the_links() {
    let mut t = core(10, 3, []);
    register_link(&mut t, link(None, "https://normal"));
    t.write(b"\x1b[?1049h");
    register_link(&mut t, link(None, "https://alt"));
    assert_eq!(link_count(&t), 2);
    t.write(b"\x1bc");
    assert_eq!(link_count(&t), 0);
}

#[test]
fn every_public_mutation_forwards_disposed_markers() {
    type Mutation = fn(&mut CoreTerminal);
    let mutations: [(&str, Mutation); 9] = [
        ("write", |t| t.write(b"")),
        ("resize", |t| t.resize(11, 3)),
        ("scroll_lines", |t| t.scroll_lines(-1)),
        ("scroll_pages", |t| t.scroll_pages(1)),
        ("scroll_to_top", |t| t.scroll_to_top()),
        ("scroll_to_bottom", |t| t.scroll_to_bottom()),
        ("scroll_to_line", |t| t.scroll_to_line(0)),
        ("reset", |t| t.reset()),
        ("set_option", |t| {
            t.set_option(OptionValue::ConvertEol(true)).unwrap();
        }),
    ];
    for (name, mutation) in mutations {
        let mut t = core(10, 3, []);
        register_link(&mut t, link(None, "https://a"));
        // dispose the link's marker without forwarding
        t.input_handler_mut()
            .services
            .buffer
            .buffers
            .normal_mut()
            .clear_all_markers();
        assert_eq!(link_count(&t), 1, "{name}: not forwarded yet");
        mutation(&mut t);
        assert_eq!(link_count(&t), 0, "{name}");
    }
}

#[test]
fn reset_frees_the_links_of_the_replaced_buffers() {
    let mut t = core(10, 3, []);
    register_link(&mut t, link(Some("x"), "https://a"));
    let id = register_link(&mut t, link(None, "https://b"));
    t.reset();
    assert_eq!(link_count(&t), 0);
    assert!(
        t.input_handler()
            .services()
            .osc_link
            .get_link_data(id)
            .is_none()
    );
    assert_eq!(t.input_handler().services().osc_link.marker_count(), 0);
}

// --- option side effects ------------------------------------------------------------------

#[test]
fn shrinking_the_scrollback_option_trims_the_buffer() {
    let mut t = core(10, 5, [OptionValue::Scrollback(100)]);
    // four line feeds move the cursor, 46 scroll
    t.write(&[b'\n'; 50]);
    assert_eq!(t.buffers().normal().lines.len(), 51);
    t.set_option(OptionValue::Scrollback(10)).unwrap();
    let normal = t.buffers().normal();
    assert_eq!(normal.lines.len(), 15);
    assert_eq!(normal.lines.max_length(), 15);
    assert_eq!(normal.ybase, 10);
}

#[test]
fn the_tab_stop_width_option_resets_the_tab_stops() {
    let mut t = core(20, 3, []);
    t.write(b"\t");
    assert_eq!(t.buffers().active().x, 8);
    t.set_option(OptionValue::TabStopWidth(4)).unwrap();
    t.write(b"\r\t");
    assert_eq!(t.buffers().active().x, 4);
    assert!(t.buffers().alt().tabs.contains(&4));
}

#[test]
fn the_log_level_option_sets_the_log_service_level() {
    let mut t = core(10, 3, []);
    assert_eq!(t.input_handler().services().log.level(), LogLevel::Info);
    t.set_option(OptionValue::LogLevel(LogLevel::Debug))
        .unwrap();
    assert_eq!(t.input_handler().services().log.level(), LogLevel::Debug);
}

#[test]
fn a_rejected_option_changes_nothing() {
    let mut t = core(10, 3, []);
    assert!(t.set_option(OptionValue::TabStopWidth(0)).is_err());
    assert_eq!(t.options().tab_stop_width, 8);
}

// --- Windows wrapping heuristics (D6) -------------------------------------------------------

fn heuristics(options: Vec<OptionValue>) -> bool {
    core(5, 3, options).input_handler().windows_heuristics
}

fn conpty(build: u32) -> OptionValue {
    OptionValue::WindowsPty(Some(WindowsPty {
        backend: Some(PtyBackendKind::Conpty),
        build_number: Some(build),
    }))
}

#[test]
fn windows_heuristics_follow_windows_mode_and_windows_pty() {
    assert!(!heuristics(vec![]));
    assert!(heuristics(vec![OptionValue::WindowsMode(true)]));
    assert!(heuristics(vec![conpty(19045)]));
    assert!(!heuristics(vec![conpty(21376)]));
    // a build number decides alone, even with windowsMode
    assert!(!heuristics(vec![
        OptionValue::WindowsMode(true),
        conpty(21376)
    ]));
    let winpty = OptionValue::WindowsPty(Some(WindowsPty {
        backend: Some(PtyBackendKind::Winpty),
        build_number: Some(19045),
    }));
    assert!(!heuristics(vec![OptionValue::WindowsMode(true), winpty]));
    // a backend without a build number falls back to windowsMode
    let backend_only = OptionValue::WindowsPty(Some(WindowsPty {
        backend: Some(PtyBackendKind::Conpty),
        build_number: None,
    }));
    assert!(!heuristics(vec![backend_only]));
}

#[test]
fn windows_heuristics_toggle_with_set_option_and_survive_reset() {
    let mut t = core(5, 3, []);
    t.write(b"abcde\r\n");
    assert!(!is_wrapped(&t, 1));

    let mut t = core(5, 3, []);
    t.set_option(OptionValue::WindowsMode(true)).unwrap();
    assert!(t.input_handler().windows_heuristics);
    // line feed half: the last cell of the line above holds 'e'
    t.write(b"abcde\r\n");
    assert!(is_wrapped(&t, 1));

    t.reset();
    assert!(t.input_handler().windows_heuristics, "options are kept");
    t.write(b"abcde\r\n");
    assert!(is_wrapped(&t, 1));

    t.set_option(OptionValue::WindowsMode(false)).unwrap();
    assert!(!t.input_handler().windows_heuristics);
    t.reset();
    t.write(b"abcde\r\n");
    assert!(!is_wrapped(&t, 1));
}

#[test]
fn windows_heuristics_csi_h_handler_marks_the_line_before_the_cursor_moves() {
    for (on, expected) in [(false, false), (true, true)] {
        let mut t = core(5, 3, [OptionValue::WindowsMode(on)]);
        // CUD moves to row 1 without a line feed; CUP then runs the heuristic on row 1
        t.write(b"abcde\x1b[B");
        assert!(!is_wrapped(&t, 1));
        t.write(b"\x1b[3;1H");
        assert_eq!(is_wrapped(&t, 1), expected, "heuristics {on}");
        // cursorPosition still ran
        assert_eq!((t.buffers().active().x, t.buffers().active().y), (0, 2));
    }
}

// --- reset ----------------------------------------------------------------------------------

#[test]
fn reset_resets_the_services_and_reports_full_damage() {
    let mut t = core(10, 3, []);
    t.write(b"hello\r\n\x0e");
    {
        let services = &mut t.input_handler_mut().services;
        services.core.dec_private_modes.bracketed_paste_mode = true;
        services.core.modes.insert_mode = true;
        services.mouse.set_active_protocol("ANY").unwrap();
    }
    t.input_handler_mut().take_damage();
    t.reset();
    let services = t.input_handler().services();
    assert_eq!(
        services
            .buffer
            .buffers
            .normal()
            .translate_buffer_line_to_string(0, true, 0, None),
        ""
    );
    assert_eq!(services.buffer.buffer().y, 0);
    assert!(!services.core.dec_private_modes.bracketed_paste_mode);
    assert!(!services.core.modes.insert_mode);
    assert_eq!(services.mouse.active_protocol(), "NONE");
    assert_eq!(services.charset.glevel(), 0);
    let damage = t.input_handler_mut().take_damage();
    assert!(damage.full && damage.scrolled);
}

// --- scroll API and input -------------------------------------------------------------------

#[test]
fn scroll_api_moves_ydisp_and_reports_scroll_damage() {
    let mut t = core(10, 5, []);
    t.write(&[b'\n'; 20]);
    assert_eq!(t.buffers().active().ydisp, 16);
    t.input_handler_mut().take_damage();

    t.scroll_lines(-3);
    assert_eq!(t.buffers().active().ydisp, 13);
    assert!(t.input_handler_mut().take_damage().scrolled);

    t.scroll_pages(-1); // rows - 1 = 4 lines
    assert_eq!(t.buffers().active().ydisp, 9);
    t.scroll_to_line(2);
    assert_eq!(t.buffers().active().ydisp, 2);
    t.scroll_to_top();
    assert_eq!(t.buffers().active().ydisp, 0);
    t.input_handler_mut().take_damage();
    t.scroll_to_top(); // no change, no event
    assert!(!t.input_handler_mut().take_damage().scrolled);
    t.scroll_to_bottom();
    assert_eq!(t.buffers().active().ydisp, 16);
    assert!(t.input_handler_mut().take_damage().scrolled);
}

#[test]
fn user_input_scrolls_to_the_bottom_and_becomes_a_response() {
    let mut t = core(10, 5, []);
    t.write(&[b'\n'; 20]);
    t.scroll_to_top();
    t.input("é", true);
    assert_eq!(t.buffers().active().ydisp, 16);
    assert_eq!(
        t.input_handler_mut().take_events(),
        [HostEvent::Response("é".as_bytes().to_vec())]
    );

    // not user input: no scroll
    t.scroll_to_top();
    t.input("x", false);
    assert_eq!(t.buffers().active().ydisp, 0);

    // disableStdin: nothing at all
    let mut t = core(10, 5, [OptionValue::DisableStdin(true)]);
    t.input("x", true);
    assert!(t.input_handler_mut().take_events().is_empty());
}

// --- register forwards ----------------------------------------------------------------------

#[test]
fn a_registered_csi_t_handler_runs_only_for_allowed_window_options() {
    let mut t = core(10, 3, []);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&calls);
    t.register_csi_handler(FunctionIdentifier::new('t'), move |_, params| {
        sink.lock().unwrap().push(params.get(0));
        true
    })
    .unwrap();
    t.write(b"\x1b[18t");
    assert!(calls.lock().unwrap().is_empty());
    t.set_option(OptionValue::WindowOptions(WindowOptions {
        get_win_size_chars: true,
        ..WindowOptions::default()
    }))
    .unwrap();
    t.write(b"\x1b[18t\x1b[14t");
    assert_eq!(*calls.lock().unwrap(), [Some(18)]);
}

#[test]
fn registered_handlers_run_before_the_built_in_ones_and_can_be_disposed() {
    let mut t = core(10, 3, []);
    let id = t
        .register_esc_handler(FunctionIdentifier::new('D'), |_| true)
        .unwrap();
    t.write(b"\x1bD");
    assert_eq!(t.buffers().active().y, 0, "consumed before IND");
    t.dispose_handler(id);
    t.write(b"\x1bD");
    assert_eq!(t.buffers().active().y, 1);
}
