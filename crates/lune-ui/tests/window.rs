//! Edge mapping and double-click timing of `lune_ui::window`.

use std::time::{Duration, Instant};

use lune_ui::window::{DoubleClick, edge_at};
use winit::window::ResizeDirection as D;

const W: f32 = 400.0;
const H: f32 = 300.0;

#[test]
fn corners() {
    assert_eq!(edge_at((0.0, 0.0), W, H), Some(D::NorthWest));
    assert_eq!(edge_at((W - 1.0, 0.0), W, H), Some(D::NorthEast));
    assert_eq!(edge_at((0.0, H - 1.0), W, H), Some(D::SouthWest));
    assert_eq!(edge_at((W - 1.0, H - 1.0), W, H), Some(D::SouthEast));
}

#[test]
fn edges() {
    assert_eq!(edge_at((0.0, 100.0), W, H), Some(D::West));
    assert_eq!(edge_at((W - 1.0, 100.0), W, H), Some(D::East));
    assert_eq!(edge_at((100.0, 0.0), W, H), Some(D::North));
    assert_eq!(edge_at((100.0, H - 1.0), W, H), Some(D::South));
}

#[test]
fn border_is_six_pixels() {
    assert_eq!(edge_at((5.9, 100.0), W, H), Some(D::West));
    assert_eq!(edge_at((6.0, 100.0), W, H), None);
    assert_eq!(edge_at((W - 6.0, 100.0), W, H), Some(D::East));
    assert_eq!(edge_at((W - 6.1, 100.0), W, H), None);
}

#[test]
fn interior_is_none() {
    assert_eq!(edge_at((200.0, 150.0), W, H), None);
}

#[test]
fn double_click_gap_is_strictly_under_400ms() {
    let t0 = Instant::now();
    let mut d = DoubleClick::new();
    assert!(!d.click(t0), "first click is never a double");
    assert!(d.click(t0 + Duration::from_millis(399)));

    let mut d = DoubleClick::new();
    d.click(t0);
    assert!(!d.click(t0 + Duration::from_millis(400)));
}
