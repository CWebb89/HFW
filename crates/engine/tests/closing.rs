//! Closing a cross-section, against real sliced layers rather than drawn shapes.
//!
//! Closing is grow by the radius and shrink back. A shape with no crack in it has
//! to come back unchanged, which is the property every test here checks: the
//! sphere fixture for the general case, a torus for holes, and the Revolver plate
//! — 18 mechanical parts on one plate, 449 layers, ring counts from 2 to 49 —
//! because that is the job this engine is asked to slice.
//!
//! The tolerances are measured, not aspirational. Over all 449 plate layers the
//! closing ratio stays in `[0.99811, 1.00000]`: a miter join is clipped where a
//! corner is sharper than the miter limit allows, which takes a few micrometres
//! off such a tip, and that is the same thing PrusaSlicer does. The worst layer is
//! z=46.4, a thin and sharply cornered section. The ring count changes on 42
//! layers, always near a tangency the closing opens or a hole it swallows, and
//! never by more than a couple of rings with the area held.

use std::path::PathBuf;

use hfw_engine::{slice_plane, Mesh};
use hfw_kernel::offset::{closing, offset_rings, Join};
use hfw_kernel::{ring_signed_area, Region, V2};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn plate() -> Mesh {
    Mesh::load_model("/home/cwebb/downloads/Revolver All Parts.3mf").expect("the plate loads")
}

fn area(rings: &[Vec<V2>]) -> f64 {
    rings.iter().map(|r| ring_signed_area(r)).sum::<f64>().abs()
}

fn sphere_layer(z: f64) -> Region {
    let mesh = Mesh::load_stl(fixture("sphere.stl")).expect("fixture loads");
    let (_, raw) = slice_plane(&mesh, z).expect("slice");
    raw
}

#[test]
fn closing_a_sphere_layer_returns_the_same_outline() {
    for z in [29.1225, 29.3525] {
        let raw = sphere_layer(z);
        assert_eq!(raw.rings.len(), 1, "z={z} should slice to one ring");
        let closed = closing(&raw.rings, 0.049, 0.002, Join::Miter);
        assert_eq!(closed.len(), 1, "z={z} changed the ring count");
        assert!(
            (area(&raw.rings) - area(&closed)).abs() < 1e-2,
            "z={z} area {} became {}",
            area(&raw.rings),
            area(&closed)
        );
    }
}

#[test]
fn closing_is_the_identity_on_every_sphere_layer() {
    let mesh = Mesh::load_stl(fixture("sphere.stl")).expect("fixture loads");
    let mut slices = 0;
    for k in 0..220 {
        let z = 0.05 + k as f64 * 0.1;
        let (_, raw) = slice_plane(&mesh, z).expect("slice");
        if raw.rings.is_empty() {
            continue;
        }
        slices += 1;
        let closed = closing(&raw.rings, 0.049, 0.002, Join::Miter);
        assert_eq!(
            closed.len(),
            raw.rings.len(),
            "z={z}: {} rings became {}",
            raw.rings.len(),
            closed.len()
        );
        let ratio = area(&closed) / area(&raw.rings);
        assert!(
            (ratio - 1.0).abs() < 0.005,
            "z={z}: area {} became {}",
            area(&raw.rings),
            area(&closed)
        );
        for ring in &closed {
            assert!(
                ring_signed_area(ring).abs() > 1e-6,
                "z={z}: a sliver ring of area {}",
                ring_signed_area(ring)
            );
        }
    }
    assert!(slices > 100, "only {slices} layers had geometry");
}

#[test]
fn closing_a_torus_slice_keeps_its_hole() {
    let mesh = Mesh::load_stl(fixture("torus.stl")).expect("fixture loads");
    let (_, raw) = slice_plane(&mesh, 5.0).expect("slice");
    assert_eq!(raw.rings.len(), 2, "a torus slice is an annulus");
    let closed = closing(&raw.rings, 0.049, 0.002, Join::Miter);
    assert_eq!(closed.len(), 2, "the hole survives closing");
    assert!(
        (area(&raw.rings) - area(&closed)).abs() < 0.1,
        "area {} became {}",
        area(&raw.rings),
        area(&closed)
    );
}

/// Every sixteenth layer of the plate: the segments that carry the most rings,
/// the ones where the closing legitimately opens a tangency or swallows a hole,
/// and the thin sharp section at z=46.4 that loses the most area of any layer.
#[test]
fn closing_a_plate_layer_moves_the_area_only_as_much_as_a_join_can() {
    let mesh = plate();
    let mut layers = 0;
    let mut lowest = (1.0f64, 0.0);
    let mut highest = (1.0f64, 0.0);
    for k in (0..449).step_by(16) {
        let z = 0.2 + k as f64 * 0.2;
        let (_, raw) = slice_plane(&mesh, z).expect("slice");
        if raw.rings.is_empty() {
            continue;
        }
        layers += 1;
        let before = area(&raw.rings);
        let closed = closing(&raw.rings, 0.049, 0.002, Join::Miter);
        assert!(!closed.is_empty(), "z={z}: closing removed the layer");
        for ring in &closed {
            assert!(
                ring_signed_area(ring).abs() > 1e-6,
                "z={z}: a ring of area {} survived",
                ring_signed_area(ring)
            );
        }
        let ratio = area(&closed) / before;
        if ratio < lowest.0 {
            lowest = (ratio, z);
        }
        if ratio > highest.0 {
            highest = (ratio, z);
        }
    }
    assert!(layers >= 25, "only {layers} layers had geometry");
    assert!(
        lowest.0 > 0.997,
        "z={:.1} lost {:.4}% of its area",
        lowest.1,
        (1.0 - lowest.0) * 100.0
    );
    assert!(
        highest.0 < 1.003,
        "z={:.1} gained {:.4}% of its area",
        highest.1,
        (highest.0 - 1.0) * 100.0
    );
}

#[test]
fn closing_a_plate_layer_keeps_the_ring_count_it_can() {
    let mesh = plate();
    for z in [0.2, 2.4, 8.6, 10.0, 12.8, 20.0, 40.4, 46.4] {
        let (_, raw) = slice_plane(&mesh, z).expect("slice");
        let before = raw.rings.len();
        assert!(before > 1, "z={z}: {before} rings is not a plate layer");
        let closed = closing(&raw.rings, 0.049, 0.002, Join::Miter);
        let after = closed.len();
        assert!(
            after + 3 >= before && after <= before + 8,
            "z={z}: {before} rings became {after}"
        );
        assert!(
            after >= 2,
            "z={z}: {before} rings collapsed to {after}, losing the outline and its holes"
        );
    }
}

#[test]
fn closing_twice_changes_nothing_more() {
    let mesh = plate();
    let (_, raw) = slice_plane(&mesh, 10.0).expect("slice");
    let once = closing(&raw.rings, 0.049, 0.002, Join::Round);
    let twice = closing(&once, 0.049, 0.002, Join::Round);
    assert!(
        (area(&once) - area(&twice)).abs() < 1e-2,
        "a closing of a closed layer moved its area from {:.3} to {:.3}",
        area(&once),
        area(&twice)
    );
}

#[test]
fn growing_is_monotone_in_the_distance() {
    let mesh = plate();
    let (_, raw) = slice_plane(&mesh, 10.0).expect("slice");
    let mut last = area(&raw.rings);
    for step in 1..=20 {
        let grown = offset_rings(&raw.rings, step as f64 * 0.01, 0.002, Join::Round);
        let now = area(&grown);
        assert!(
            now >= last - 1e-6,
            "growing by {} mm shrank the area from {last:.3} to {now:.3}",
            step as f64 * 0.01
        );
        last = now;
    }
}
