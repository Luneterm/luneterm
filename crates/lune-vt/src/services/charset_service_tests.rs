//! Tests for xterm.js src/common/services/CharsetService.ts @ f447274. Upstream has no tests;
//! these follow the TypeScript behaviour line by line (`CharsetService.ts:17-33`).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::data::charsets::charset;

fn dec() -> Option<&'static Charset> {
    charset('0')
}

fn uk() -> Option<&'static Charset> {
    charset('A')
}

fn same(a: Option<&'static Charset>, b: Option<&'static Charset>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

#[test]
fn starts_with_no_charset_on_g0() {
    let cs = CharsetService::new();
    assert!(cs.charset.is_none());
    assert_eq!(cs.glevel(), 0);
}

#[test]
fn designators_used_below_are_distinct_sets() {
    assert!(dec().is_some());
    assert!(uk().is_some());
    assert!(!same(dec(), uk()));
}

#[test]
fn reset_clears_charset_designations_and_level() {
    let mut cs = CharsetService::new();
    cs.set_g_charset(1, dec());
    cs.set_g_level(1);
    assert!(same(cs.charset, dec()));
    cs.reset();
    assert!(cs.charset.is_none());
    assert_eq!(cs.glevel(), 0);
    // The designation for G1 is gone too.
    cs.set_g_level(1);
    assert!(cs.charset.is_none());
}

#[test]
fn set_g_level_switches_to_the_designated_set() {
    let mut cs = CharsetService::new();
    cs.set_g_charset(1, dec());
    cs.set_g_charset(2, uk());
    // Designating a non-current level does not change the current charset.
    assert!(cs.charset.is_none());
    cs.set_g_level(1);
    assert_eq!(cs.glevel(), 1);
    assert!(same(cs.charset, dec()));
    cs.set_g_level(2);
    assert!(same(cs.charset, uk()));
    // A level with nothing designated gives none.
    cs.set_g_level(3);
    assert_eq!(cs.glevel(), 3);
    assert!(cs.charset.is_none());
    cs.set_g_level(0);
    assert!(cs.charset.is_none());
}

#[test]
fn set_g_charset_on_the_current_level_updates_charset() {
    let mut cs = CharsetService::new();
    cs.set_g_charset(0, dec());
    assert!(same(cs.charset, dec()));
    cs.set_g_charset(0, uk());
    assert!(same(cs.charset, uk()));
    // Designating "none" (US) on the current level clears it.
    cs.set_g_charset(0, None);
    assert!(cs.charset.is_none());
}

#[test]
fn set_g_charset_on_another_level_leaves_charset_alone() {
    let mut cs = CharsetService::new();
    cs.set_g_charset(0, dec());
    cs.set_g_charset(1, uk());
    assert!(same(cs.charset, dec()));
    cs.set_g_level(1);
    assert!(same(cs.charset, uk()));
    cs.set_g_charset(0, None);
    assert!(same(cs.charset, uk()));
    cs.set_g_level(0);
    assert!(cs.charset.is_none());
}

#[test]
fn out_of_range_levels_never_panic() {
    let mut cs = CharsetService::new();
    cs.set_g_charset(4, dec());
    cs.set_g_charset(usize::MAX, dec());
    // Reading an undesignated slot gives none (JS: `undefined`).
    cs.set_g_level(4);
    assert_eq!(cs.glevel(), 4);
    assert!(cs.charset.is_none());
    cs.set_g_level(usize::MAX);
    assert!(cs.charset.is_none());
    // Writing at the current out-of-range level still sets `charset` immediately, as upstream.
    cs.set_g_charset(usize::MAX, uk());
    assert!(same(cs.charset, uk()));
    // The documented difference: it is not remembered for a later read.
    cs.set_g_level(0);
    cs.set_g_level(usize::MAX);
    assert!(cs.charset.is_none());
    cs.reset();
    assert_eq!(cs.glevel(), 0);
}
