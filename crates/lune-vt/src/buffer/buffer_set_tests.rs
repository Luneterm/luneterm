//! Port of xterm.js src/common/buffer/BufferSet.test.ts @ f447274 (6 tests), plus tests for the
//! returned `onBufferActivate` events, `reset`, `resize` and the alt buffer's fill and clearing.
//!
//! Adaptations (`adapted:`): `MockOptionsService({ scrollback: 1000 })` and
//! `MockBufferService(80, 24)` are `TerminalOptions::default()` (scrollback 1000) and the sizes
//! `(80, 24)`; `assert.equal(bufferSet.active, bufferSet.normal)` is `!is_alt_active()` (the buffers
//! are owned, not shared references); `alt.markers.length` is `alt().markers().len()`;
//! `activateNormalBuffer`/`activateAltBuffer` take the options / fill attribute explicitly.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::buffer::buffer_line::DEFAULT_ATTR_DATA;
use crate::buffer::cell_data::CellData;

fn set() -> (BufferSet, TerminalOptions) {
    let opts = TerminalOptions {
        scrollback: 1000,
        ..TerminalOptions::default()
    };
    (BufferSet::new(&opts, 80, 24), opts)
}

// ---- constructor ----

#[test]
fn constructor_creates_two_different_buffers_alt_and_normal() {
    let (set, _) = set();
    assert!(set.normal().has_scrollback());
    assert!(!set.alt().has_scrollback());
    assert_ne!(
        set.normal().lines.max_length(),
        set.alt().lines.max_length(),
        "normal has scrollback, alt none"
    );
    assert!(!std::ptr::eq(set.normal(), set.alt()));
}

#[test]
fn constructor_activates_and_fills_the_normal_buffer() {
    let (set, _) = set();
    assert!(!set.is_alt_active());
    assert!(std::ptr::eq(set.active(), set.normal()));
    assert_eq!(set.normal().lines.len(), 24);
    assert_eq!(
        set.alt().lines.len(),
        0,
        "the alt buffer is filled on activation"
    );
    assert_eq!(set.alt().lines.max_length(), 24);
    assert_eq!(set.normal().lines.max_length(), 24 + 1000);
}

#[test]
fn constructor_sets_up_the_tab_stops_of_both_buffers() {
    let (set, _) = set();
    for buffer in [set.normal(), set.alt()] {
        assert_eq!(
            buffer.tabs.iter().copied().collect::<Vec<_>>(),
            (0..80).step_by(8).collect::<Vec<_>>()
        );
    }
}

// ---- activateNormalBuffer / activateAltBuffer ----

#[test]
fn activate_normal_buffer_sets_the_normal_buffer_as_active() {
    let (mut set, opts) = set();
    set.activate_normal_buffer(&opts);
    assert!(std::ptr::eq(set.active(), set.normal()));
    assert!(!set.is_alt_active());
}

#[test]
fn activate_alt_buffer_sets_the_alt_buffer_as_active() {
    let (mut set, _) = set();
    set.activate_alt_buffer(None);
    assert!(std::ptr::eq(set.active(), set.alt()));
    assert!(set.is_alt_active());
}

// ---- cursor handling when swapping buffers ----

fn zeroed() -> (BufferSet, TerminalOptions) {
    let (mut set, opts) = set();
    set.normal_mut().x = 0;
    set.normal_mut().y = 0;
    set.alt_mut().x = 0;
    set.alt_mut().y = 0;
    (set, opts)
}

#[test]
fn the_cursor_stays_put_when_activating_the_alt_buffer() {
    let (mut set, opts) = zeroed();
    set.activate_normal_buffer(&opts);
    set.active_mut().x = 30;
    set.active_mut().y = 10;
    set.activate_alt_buffer(None);
    assert_eq!(set.active().x, 30);
    assert_eq!(set.active().y, 10);
}

#[test]
fn the_cursor_stays_put_when_activating_the_normal_buffer() {
    let (mut set, opts) = zeroed();
    set.activate_alt_buffer(None);
    set.active_mut().x = 30;
    set.active_mut().y = 10;
    set.activate_normal_buffer(&opts);
    assert_eq!(set.active().x, 30);
    assert_eq!(set.active().y, 10);
}

// ---- markers ----

#[test]
fn the_alt_markers_are_cleared_when_the_buffer_is_switched() {
    let (mut set, opts) = set();
    set.activate_alt_buffer(None);
    let id = set.alt_mut().add_marker(1);
    assert_eq!(set.alt().markers().len(), 1);
    set.activate_normal_buffer(&opts);
    assert_eq!(set.alt().markers().len(), 0);
    assert!(set.alt().marker(id).is_none());
    assert_eq!(set.alt_mut().take_disposed_markers(), vec![id]);
}

// ---- onBufferActivate as return values ----

#[test]
fn activation_returns_the_event_exactly_when_upstream_fires() {
    let (mut set, opts) = set();
    // the normal buffer is already active: nothing fires
    assert_eq!(set.activate_normal_buffer(&opts), None);
    set.normal_mut().ydisp = 7;
    assert_eq!(
        set.activate_alt_buffer(None),
        Some(BufferActivate {
            active_ydisp: 0,
            alt_active: true
        })
    );
    // already alt: nothing fires
    assert_eq!(set.activate_alt_buffer(None), None);
    assert_eq!(
        set.activate_normal_buffer(&opts),
        Some(BufferActivate {
            active_ydisp: 7,
            alt_active: false
        })
    );
}

#[test]
fn reset_always_returns_the_event_and_rebuilds_both_buffers() {
    let (mut set, opts) = set();
    set.normal_mut().ydisp = 3;
    set.normal_mut().x = 9;
    set.activate_alt_buffer(None);
    set.alt_mut().add_marker(0);
    let event = set.reset(&opts, 60, 10);
    assert_eq!(
        event,
        BufferActivate {
            active_ydisp: 0,
            alt_active: false
        }
    );
    assert!(!set.is_alt_active());
    assert_eq!((set.normal().cols(), set.normal().rows()), (60, 10));
    assert_eq!((set.alt().cols(), set.alt().rows()), (60, 10));
    assert_eq!(set.normal().lines.len(), 10);
    assert_eq!(set.alt().lines.len(), 0);
    assert_eq!(set.normal().x, 0);
    assert_eq!(set.alt().markers().len(), 0);
    assert_eq!(
        set.normal().tabs.len(),
        8,
        "tab stops every 8 of 60 columns"
    );
    // a reset while the normal buffer is active also fires
    assert_eq!(set.reset(&opts, 60, 10).active_ydisp, 0);
}

// ---- the alt buffer is cleared and refilled ----

#[test]
fn the_alt_buffer_is_filled_on_activation_with_the_fill_attribute() {
    let (mut set, _) = set();
    let attr = AttributeData {
        bg: 0x0100_0003,
        ..DEFAULT_ATTR_DATA
    };
    set.activate_alt_buffer(Some(&attr));
    assert_eq!(set.alt().lines.len(), 24);
    assert_eq!(set.alt().lines.get(5).unwrap().get_bg(0), 0x0100_0003);
}

#[test]
fn the_alt_buffer_is_emptied_when_the_normal_buffer_returns() {
    let (mut set, opts) = set();
    set.activate_alt_buffer(None);
    set.alt_mut().lines.get_mut(0).unwrap().set_cell(
        0,
        &CellData::from_char_data(&(0, "x".to_string(), 1, 'x' as u32)),
    );
    set.alt_mut().ybase = 4;
    set.activate_normal_buffer(&opts);
    assert!(set.alt().lines.is_empty());
    assert_eq!(set.alt().ybase, 0);
    // and refilled blank the next time
    set.activate_alt_buffer(None);
    assert_eq!(set.alt().lines.get(0).unwrap().get_string(0), "");
}

// ---- resize ----

#[test]
fn resize_resizes_both_buffers_and_extends_the_tab_stops() {
    let (mut set, opts) = set();
    set.resize(100, 30, &opts);
    for buffer in [set.normal(), set.alt()] {
        assert_eq!((buffer.cols(), buffer.rows()), (100, 30));
    }
    assert_eq!(set.normal().lines.len(), 30);
    assert_eq!(set.normal().lines.get(0).unwrap().len(), 100);
    // `setupTabStops(100)` goes back to the stop at 72 and fills up to 96
    for buffer in [set.normal(), set.alt()] {
        assert!(buffer.tabs.contains(&80) && buffer.tabs.contains(&96));
        assert!(!buffer.tabs.contains(&100));
    }
}

#[test]
fn setup_tab_stops_applies_to_both_buffers() {
    let (mut set, mut opts) = set();
    opts.tab_stop_width = 4;
    set.setup_tab_stops(None, &opts);
    for buffer in [set.normal(), set.alt()] {
        assert_eq!(
            buffer.tabs.iter().copied().collect::<Vec<_>>(),
            (0..80).step_by(4).collect::<Vec<_>>()
        );
    }
}
