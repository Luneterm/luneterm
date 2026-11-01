//! Tests for `osc_link_service.rs`: the three cases of upstream `OscLinkService.test.ts`, then
//! cases written from `OscLinkService.ts` for the Rust-side event forwarding (the host drains the
//! buffers' disposal logs into [`OscLinkService::on_markers_disposed`]) and for adding lines.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::buffer::attribute_data::AttributeData;
use crate::services::options_service::{OptionValue, OptionsService};

fn setup() -> (BufferService, OscLinkService) {
    let options = OptionsService::new([OptionValue::Rows(3), OptionValue::Cols(10)]);
    (BufferService::new(options.options()), OscLinkService::new())
}

fn link(id: &str, uri: &str) -> OscLinkData {
    OscLinkData {
        id: Some(id.to_string()),
        uri: uri.to_string(),
    }
}

fn no_id(uri: &str) -> OscLinkData {
    OscLinkData {
        id: None,
        uri: uri.to_string(),
    }
}

/// What the host does after an operation that can dispose markers.
fn forward(buffers: &mut BufferService, osc: &mut OscLinkService) {
    let normal = buffers.buffers.normal_mut().take_disposed_markers();
    osc.on_markers_disposed(&normal);
    let alt = buffers.buffers.alt_mut().take_disposed_markers();
    osc.on_markers_disposed(&alt);
}

// ---- OscLinkService.test.ts ----

#[test]
fn link_ids_are_created_and_fetched_consistently() {
    let (mut buffers, mut osc) = setup();
    let link_id = osc.register_link(&mut buffers, link("foo", "bar"));
    assert_ne!(link_id, 0);
    assert_eq!(osc.register_link(&mut buffers, link("foo", "bar")), link_id);
}

#[test]
fn should_dispose_the_link_id_when_the_last_marker_is_trimmed_from_the_buffer() {
    let (mut buffers, mut osc) = setup();
    // Activate the alt buffer to get 0 scrollback
    buffers.buffers.activate_alt_buffer(None);
    let link_id = osc.register_link(&mut buffers, link("foo", "bar"));
    assert_ne!(link_id, 0);
    buffers.scroll(&AttributeData::default(), false);
    forward(&mut buffers, &mut osc);
    assert_ne!(osc.register_link(&mut buffers, link("foo", "bar")), link_id);
}

#[test]
fn should_fetch_link_data_from_link_id() {
    let (mut buffers, mut osc) = setup();
    let link_id = osc.register_link(&mut buffers, link("foo", "bar"));
    assert_eq!(osc.get_link_data(link_id), Some(&link("foo", "bar")));
}

// ---- written from OscLinkService.ts ----

#[test]
fn ids_start_at_one_and_links_without_an_id_are_never_shared() {
    let (mut buffers, mut osc) = setup();
    let a = osc.register_link(&mut buffers, no_id("http://a"));
    let b = osc.register_link(&mut buffers, no_id("http://a"));
    assert_eq!((a, b), (1, 2));
    assert_eq!(osc.get_link_data(a), Some(&no_id("http://a")));
    assert_eq!(osc.get_link_data(b), Some(&no_id("http://a")));
    assert_eq!(osc.len(), 2);
}

#[test]
fn the_same_string_id_with_another_uri_is_another_link() {
    let (mut buffers, mut osc) = setup();
    let a = osc.register_link(&mut buffers, link("x", "u1"));
    let b = osc.register_link(&mut buffers, link("x", "u2"));
    let c = osc.register_link(&mut buffers, link("y", "u1"));
    assert!(a != b && b != c && a != c);
}

#[test]
fn the_key_joins_id_and_uri_with_two_semicolons_so_that_a_crafted_pair_collides() {
    // Upstream keys `${id};;${uri}`: ("a", "b;;c") and ("a;;b", "c") are the same key.
    let (mut buffers, mut osc) = setup();
    let a = osc.register_link(&mut buffers, link("a", "b;;c"));
    let b = osc.register_link(&mut buffers, link("a;;b", "c"));
    assert_eq!(a, b);
    assert_eq!(osc.get_link_data(a), Some(&link("a", "b;;c")));
}

#[test]
fn unknown_link_ids_have_no_data_and_add_line_ignores_them() {
    let (mut buffers, mut osc) = setup();
    assert_eq!(osc.get_link_data(7), None);
    osc.add_line_to_link(&mut buffers, 7, 1);
    assert_eq!(buffers.buffer().markers().len(), 0);
    assert_eq!(osc.marker_count(), 0);
}

#[test]
fn registering_marks_the_cursor_line_and_the_same_link_on_another_line_adds_a_marker() {
    let (mut buffers, mut osc) = setup();
    buffers.buffer_mut().y = 1;
    let id = osc.register_link(&mut buffers, link("foo", "bar"));
    buffers.buffer_mut().y = 2;
    assert_eq!(osc.register_link(&mut buffers, link("foo", "bar")), id);
    let markers = osc.link_markers(id).unwrap();
    let lines: Vec<i64> = markers
        .iter()
        .map(|m| buffers.buffer().marker(m.id).unwrap().line)
        .collect();
    assert_eq!(lines, vec![1, 2]);
    assert!(markers.iter().all(|m| !m.alt));
    assert_eq!(osc.marker_count(), 2);
}

#[test]
fn a_line_that_already_holds_the_link_gets_no_second_marker() {
    let (mut buffers, mut osc) = setup();
    let id = osc.register_link(&mut buffers, link("foo", "bar"));
    osc.add_line_to_link(&mut buffers, id, 0);
    osc.register_link(&mut buffers, link("foo", "bar"));
    assert_eq!(osc.link_markers(id).unwrap().len(), 1);
    osc.add_line_to_link(&mut buffers, id, 2);
    osc.add_line_to_link(&mut buffers, id, 2);
    assert_eq!(osc.link_markers(id).unwrap().len(), 2);
    assert_eq!(buffers.buffer().markers().len(), 2);
}

#[test]
fn a_registration_remembers_which_buffer_holds_its_marker() {
    let (mut buffers, mut osc) = setup();
    let normal = osc.register_link(&mut buffers, no_id("n"));
    buffers.buffers.activate_alt_buffer(None);
    let alt = osc.register_link(&mut buffers, no_id("a"));
    assert!(!osc.link_markers(normal).unwrap()[0].alt);
    assert!(osc.link_markers(alt).unwrap()[0].alt);
    // The alt link's id is in the alt buffer's set, not the normal one's.
    let alt_marker = osc.link_markers(alt).unwrap()[0].id;
    assert!(buffers.buffers.alt().marker(alt_marker).is_some());
    assert!(buffers.buffers.normal().marker(alt_marker).is_none());
}

#[test]
fn a_link_lives_until_its_last_marker_is_disposed() {
    let (mut buffers, mut osc) = setup();
    let id = osc.register_link(&mut buffers, link("foo", "bar"));
    osc.add_line_to_link(&mut buffers, id, 2);
    buffers.buffer_mut().clear_markers(0);
    forward(&mut buffers, &mut osc);
    assert_eq!(osc.get_link_data(id), Some(&link("foo", "bar")));
    assert_eq!(osc.link_markers(id).unwrap().len(), 1);
    buffers.buffer_mut().clear_markers(2);
    forward(&mut buffers, &mut osc);
    assert_eq!(osc.get_link_data(id), None);
    assert!(osc.is_empty());
    assert_eq!(osc.marker_count(), 0);
    // The key is free again: the same link gets a new id.
    assert_ne!(osc.register_link(&mut buffers, link("foo", "bar")), id);
}

#[test]
fn a_link_without_an_id_is_freed_with_its_marker() {
    let (mut buffers, mut osc) = setup();
    let id = osc.register_link(&mut buffers, no_id("u"));
    buffers.buffer_mut().clear_all_markers();
    forward(&mut buffers, &mut osc);
    assert_eq!(osc.get_link_data(id), None);
}

#[test]
fn nothing_is_freed_until_the_host_forwards_the_disposals() {
    let (mut buffers, mut osc) = setup();
    let id = osc.register_link(&mut buffers, no_id("u"));
    buffers.buffer_mut().clear_all_markers();
    assert!(osc.get_link_data(id).is_some());
    let disposed = buffers.buffer_mut().take_disposed_markers();
    assert_eq!(disposed.len(), 1);
    osc.on_markers_disposed(&disposed);
    assert!(osc.get_link_data(id).is_none());
}

#[test]
fn forwarding_unknown_or_repeated_ids_changes_nothing() {
    let (mut buffers, mut osc) = setup();
    let id = osc.register_link(&mut buffers, no_id("u"));
    osc.on_markers_disposed(&[MarkerId(u64::MAX)]);
    assert!(osc.get_link_data(id).is_some());
    buffers.buffer_mut().clear_all_markers();
    let disposed = buffers.buffer_mut().take_disposed_markers();
    osc.on_markers_disposed(&disposed);
    osc.on_markers_disposed(&disposed);
    assert!(osc.is_empty());
}

#[test]
fn a_scroll_that_trims_nothing_keeps_the_marker_line_and_the_link() {
    let (mut buffers, mut osc) = setup();
    let id = osc.register_link(&mut buffers, no_id("u"));
    buffers.scroll(&AttributeData::default(), false);
    // The scrollback limit is not reached: the marker keeps its line and the link its data.
    forward(&mut buffers, &mut osc);
    let marker = osc.link_markers(id).unwrap()[0].id;
    assert_eq!(buffers.buffer().marker(marker).unwrap().line, 0);
    assert!(osc.get_link_data(id).is_some());
}

#[test]
fn a_marker_added_through_the_service_is_the_only_one_it_creates() {
    let (mut buffers, mut osc) = setup();
    let before = buffers.buffer().markers().len();
    osc.register_link(&mut buffers, link("foo", "bar"));
    osc.register_link(&mut buffers, link("foo", "bar"));
    assert_eq!(buffers.buffer().markers().len(), before + 1);
}

#[test]
fn link_ids_enumerate_the_live_links() {
    let (mut buffers, mut osc) = setup();
    let a = osc.register_link(&mut buffers, no_id("a"));
    let b = osc.register_link(&mut buffers, no_id("b"));
    let mut ids: Vec<u32> = osc.link_ids().collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![a, b]);
}

// ---- caps, `// LUNETERM:` (review S2) ----

#[test]
fn links_past_the_cap_evict_the_least_recently_extended_and_dispose_its_markers() {
    let (mut buffers, mut osc) = setup();
    let first = osc.register_link(&mut buffers, link("keep", "k"));
    let second = osc.register_link(&mut buffers, no_id("old"));
    // extending `first` to another line makes `second` the least recently extended
    osc.add_line_to_link(&mut buffers, first, 1);
    for i in 0..MAX_LINKS - 2 {
        osc.register_link(&mut buffers, no_id(&format!("u{i}")));
    }
    assert_eq!(osc.len(), MAX_LINKS);
    assert!(osc.get_link_data(second).is_some());
    let newest = osc.register_link(&mut buffers, no_id("new"));
    assert_eq!(osc.len(), MAX_LINKS);
    assert!(osc.get_link_data(second).is_none(), "evicted");
    assert!(osc.get_link_data(first).is_some());
    assert!(osc.get_link_data(newest).is_some());
    // the evicted link's marker is gone from the buffer and the table holds no stale marker
    let live = buffers.buffers.normal().markers().len();
    assert_eq!(live, osc.marker_count());
    // forwarding the eviction's disposal changes nothing
    forward(&mut buffers, &mut osc);
    assert_eq!(osc.len(), MAX_LINKS);
}

#[test]
fn an_evicted_link_with_an_id_is_registered_anew() {
    let (mut buffers, mut osc) = setup();
    let a = osc.register_link(&mut buffers, link("a", "u"));
    for i in 0..MAX_LINKS {
        osc.register_link(&mut buffers, no_id(&format!("u{i}")));
    }
    assert!(osc.get_link_data(a).is_none());
    let again = osc.register_link(&mut buffers, link("a", "u"));
    assert_ne!(again, a);
    assert_eq!(osc.get_link_data(again).map(|d| d.uri.as_str()), Some("u"));
}

#[test]
fn markers_past_the_cap_evict_other_links_but_never_the_link_being_extended() {
    let options = OptionsService::new([
        OptionValue::Rows(3),
        OptionValue::Cols(10),
        OptionValue::Scrollback(MAX_LINK_MARKERS + 100),
    ]);
    let mut buffers = BufferService::new(options.options());
    let mut osc = OscLinkService::new();
    for _ in 0..MAX_LINK_MARKERS + 50 {
        buffers.scroll(&AttributeData::default(), false);
    }
    let other = osc.register_link(&mut buffers, no_id("other"));
    let long = osc.register_link(&mut buffers, link("l", "long"));
    // one link over more lines than the marker cap keeps every line
    for y in 0..MAX_LINK_MARKERS + 20 {
        osc.add_line_to_link(&mut buffers, long, y);
    }
    assert!(
        osc.get_link_data(other).is_none(),
        "evicted for the long link"
    );
    assert_eq!(
        osc.link_markers(long).map(<[_]>::len),
        Some(MAX_LINK_MARKERS + 20 + 1)
    );
    assert_eq!(osc.len(), 1);
}

// ---- id wrap, `// LUNETERM:` (review F5) ----

/// Runs `f` on a thread and fails if it does not finish in time (the bug was an endless loop).
fn finishes(f: impl FnOnce() + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        f();
        tx.send(()).unwrap();
    });
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(20)).is_ok(),
        "did not finish (endless loop in enforce_caps?)"
    );
}

#[test]
fn a_wrapped_id_counter_skips_ids_still_in_use() {
    finishes(|| {
        let (mut buffers, mut osc) = setup();
        let a = osc.register_link(&mut buffers, link("a", "u"));
        assert_eq!(a, 1);
        // inject the counter instead of registering 2^32 links
        osc.next_id = u32::MAX;
        let b = osc.register_link(&mut buffers, no_id("b"));
        assert_eq!(b, u32::MAX);
        let c = osc.register_link(&mut buffers, no_id("c"));
        assert_eq!(c, 2, "1 is still in use");
        assert_eq!(osc.get_link_data(a).map(|d| d.uri.as_str()), Some("u"));
        assert_eq!(osc.get_link_data(c).map(|d| d.uri.as_str()), Some("c"));
        // the cap evicts past the wrap (the endless loop of review F5)
        for i in 0..MAX_LINKS + 10 {
            osc.register_link(&mut buffers, no_id(&format!("x{i}")));
        }
        assert_eq!(osc.len(), MAX_LINKS);
        assert_eq!(osc.by_age.len(), osc.len(), "one age key per live link");
        assert_eq!(buffers.buffers.normal().markers().len(), osc.marker_count());
    });
}

#[test]
fn enforce_caps_ends_even_when_an_age_key_outlived_its_entry() {
    finishes(|| {
        let (mut buffers, mut osc) = setup();
        let a = osc.register_link(&mut buffers, no_id("a"));
        // a stale key, older than every live one, for a live id and for a gone one
        osc.by_age.insert(0, a);
        osc.by_age.insert(u64::MAX - 1, 999_999);
        for i in 0..MAX_LINKS + 10 {
            osc.register_link(&mut buffers, no_id(&format!("x{i}")));
        }
        assert_eq!(osc.len(), MAX_LINKS);
    });
}
