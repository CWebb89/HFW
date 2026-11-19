//! Differential test: this kernel's offset against Clipper2.
//!
//! Clipper2 is the offset every slicer the user compares against actually runs:
//! PrusaSlicer and OrcaSlicer vendor the Slic3r lineage of it, and Cura switched
//! to Clipper2. `clipper2-rust` is a direct port of the same algorithm, so on the
//! same rings at the same join style it is ground truth in the
//! `i_overlay` does not provide.
//!
//! This is a dev-dependency only: the engine ships `i_overlay`, and these tests pin
//! the two together so a change to the lattice, the join, or the fill rule shows up
//! as a number rather than as a model that slices wrong.

use clipper2_rust::{area as clipper_area, ClipperOffset, EndType, JoinType, Paths64};
use hfw_kernel::offset::{closing, offset_rings, Join};
use hfw_kernel::{ring_signed_area, V2};

/// Clipper2's integer lattice for these tests: one micrometre, which is the
/// resolution a slicer's own offset runs at (Slic3r scales by 1e6 for this).
const SCALE: f64 = 1.0e6;

fn to_paths(rings: &[Vec<V2>]) -> Paths64 {
    rings
        .iter()
        .map(|ring| {
            ring.iter()
                .map(|p| {
                    clipper2_rust::Point64::new(
                        (p.x * SCALE).round() as i64,
                        (p.y * SCALE).round() as i64,
                    )
                })
                .collect()
        })
        .collect()
}

fn from_paths(paths: &Paths64) -> Vec<Vec<V2>> {
    paths
        .iter()
        .map(|path| {
            path.iter()
                .map(|p| V2::new(p.x as f64 / SCALE, p.y as f64 / SCALE))
                .collect()
        })
        .collect()
}

fn clipper_offset(rings: &[Vec<V2>], delta: f64, join: Join) -> Vec<Vec<V2>> {
    let mut offset = ClipperOffset::new(3.0, 0.002 * SCALE, false, false);
    let join_type = match join {
        Join::Round => JoinType::Round,
        Join::Miter => JoinType::Miter,
    };
    offset.add_paths(&to_paths(rings), join_type, EndType::Polygon);
    let mut solution: Paths64 = Vec::new();
    offset.execute(delta * SCALE, &mut solution);
    from_paths(&solution)
}

fn clipper_closing(rings: &[Vec<V2>], radius: f64, join: Join) -> Vec<Vec<V2>> {
    let grown = clipper_offset(rings, radius, join);
    clipper_offset(&grown, -radius, join)
}

fn area(rings: &[Vec<V2>]) -> f64 {
    rings.iter().map(|r| ring_signed_area(r)).sum::<f64>().abs()
}

fn clipper_area_of(rings: &[Vec<V2>]) -> f64 {
    to_paths(rings).iter().map(clipper_area).sum::<f64>().abs() / (SCALE * SCALE)
}

fn square(size: f64) -> Vec<Vec<V2>> {
    let h = size * 0.5;
    vec![vec![
        V2::new(-h, -h),
        V2::new(h, -h),
        V2::new(h, h),
        V2::new(-h, h),
    ]]
}

fn circle(centre: V2, radius: f64, points: usize) -> Vec<V2> {
    (0..points)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / points as f64;
            V2::new(centre.x + radius * a.cos(), centre.y + radius * a.sin())
        })
        .collect()
}

fn annulus(outer: f64, inner: f64) -> Vec<Vec<V2>> {
    let mut hole = circle(V2::new(0.0, 0.0), inner, 720);
    hole.reverse();
    vec![circle(V2::new(0.0, 0.0), outer, 720), hole]
}

/// An outline and the piece across a hairline crack from it: the case the closing
/// exists for.
fn cracked_pair(gap: f64) -> Vec<Vec<V2>> {
    let left = vec![
        V2::new(0.0, 0.0),
        V2::new(10.0, 0.0),
        V2::new(10.0, 10.0),
        V2::new(0.0, 10.0),
    ];
    let right = vec![
        V2::new(10.0 + gap, 0.0),
        V2::new(20.0 + gap, 0.0),
        V2::new(20.0 + gap, 10.0),
        V2::new(10.0 + gap, 10.0),
    ];
    vec![left, right]
}

fn assert_agrees(mine: &[Vec<V2>], reference: &[Vec<V2>], what: &str, tolerance: f64) {
    let a = area(mine);
    let b = area(reference);
    let relative = (a - b).abs() / b.max(1e-12);
    assert!(
        relative <= tolerance,
        "{what}: this kernel {a:.6} mm^2, Clipper2 {b:.6} mm^2 ({:.4}% apart)",
        relative * 100.0
    );
    let reference_area = clipper_area_of(reference);
    assert!(
        (b - reference_area).abs() / reference_area < 1e-6,
        "{what}: the reference's own area disagrees with Clipper2's ({b:.6} vs {reference_area:.6})"
    );
}

#[test]
fn growing_a_square_matches_clipper2() {
    for join in [Join::Miter, Join::Round] {
        for delta in [0.05, 0.2, 1.0, 4.0] {
            let mine = offset_rings(&square(10.0), delta, 0.002, join);
            let reference = clipper_offset(&square(10.0), delta, join);
            assert_agrees(
                &mine,
                &reference,
                &format!("{join:?} square grown {delta}"),
                0.002,
            );
        }
    }
}

#[test]
fn shrinking_a_square_matches_clipper2() {
    for join in [Join::Miter, Join::Round] {
        for delta in [-0.05, -0.2, -1.0, -4.9] {
            let mine = offset_rings(&square(10.0), delta, 0.002, join);
            let reference = clipper_offset(&square(10.0), delta, join);
            assert_agrees(
                &mine,
                &reference,
                &format!("{join:?} square shrunk {delta}"),
                0.002,
            );
        }
    }
}

#[test]
fn an_offset_past_the_middle_collapses_in_both() {
    let mine = offset_rings(&square(10.0), -5.5, 0.002, Join::Miter);
    let reference = clipper_offset(&square(10.0), -5.5, Join::Miter);
    assert!(mine.is_empty(), "this kernel kept {mine:?}");
    assert!(reference.is_empty(), "Clipper2 kept {reference:?}");
}

#[test]
fn a_hole_shrinks_as_the_region_grows_in_both() {
    for delta in [0.05, 0.2, 1.0] {
        let rings = annulus(20.0, 5.0);
        let mine = offset_rings(&rings, delta, 0.002, Join::Round);
        let reference = clipper_offset(&rings, delta, Join::Round);
        assert_eq!(mine.len(), reference.len(), "ring count at delta {delta}");
        assert_agrees(&mine, &reference, &format!("annulus grown {delta}"), 0.002);
    }
}

#[test]
fn a_hole_narrower_than_the_offset_disappears_in_both() {
    let rings = annulus(20.0, 1.0);
    let mine = offset_rings(&rings, -1.2, 0.002, Join::Round);
    let reference = clipper_offset(&rings, -1.2, Join::Round);
    assert_eq!(mine.len(), reference.len(), "rings {}", mine.len());
}

#[test]
fn a_crack_wider_than_the_radius_stays_open_in_both() {
    let rings = cracked_pair(0.2);
    let mine = closing(&rings, 0.049, 0.002, Join::Miter);
    let reference = clipper_closing(&rings, 0.049, Join::Miter);
    assert_eq!(mine.len(), 2, "this kernel welded a crack it should not");
    assert_eq!(reference.len(), 2, "Clipper2 welded a crack it should not");
    assert_agrees(&mine, &reference, "closing a 0.2 mm crack", 0.002);
}

#[test]
fn a_crack_narrower_than_twice_the_radius_is_welded_in_both() {
    let rings = cracked_pair(0.05);
    let mine = closing(&rings, 0.049, 0.002, Join::Miter);
    let reference = clipper_closing(&rings, 0.049, Join::Miter);
    assert_eq!(mine.len(), 1, "this kernel left the crack open");
    assert_eq!(reference.len(), 1, "Clipper2 left the crack open");
    assert_agrees(&mine, &reference, "closing a 0.05 mm crack", 0.002);
}

#[test]
fn closing_a_circle_matches_clipper2() {
    for radius in [0.002, 0.049, 0.3] {
        let rings = vec![circle(V2::new(0.0, 0.0), 12.0, 720)];
        let mine = closing(&rings, radius, 0.002, Join::Miter);
        let reference = clipper_closing(&rings, radius, Join::Miter);
        assert_eq!(mine.len(), 1, "radius {radius}");
        assert_agrees(&mine, &reference, &format!("circle closed {radius}"), 0.002);
    }
}

/// The plate's own layers, sampled: 49-ring sections with holes, hairline features
/// and sharp corners, which is what a differential test is for.
#[test]
fn plate_layers_match_clipper2() {
    let mut checked = 0;
    for (z, rings) in sampled_plate_rings() {
        for join in [Join::Miter, Join::Round] {
            let mine = closing(&rings, 0.049, 0.002, join);
            let reference = clipper_closing(&rings, 0.049, join);
            let mine_area = area(&mine);
            let reference_area = area(&reference);
            let relative = (mine_area - reference_area).abs() / reference_area.max(1e-12);
            assert!(
                relative < 0.0005,
                "z={z} {join:?}: {} rings, this kernel {mine_area:.4} mm^2, Clipper2 \
                 {reference_area:.4} mm^2 ({:.3}% apart)",
                rings.len(),
                relative * 100.0
            );
            checked += 1;
        }
    }
    assert!(checked >= 8, "only {checked} layer comparisons ran");
}

/// Ring sets read from the plate, saved as plain coordinates so this test does not
/// depend on a model file or on the slicing crate.
fn sampled_plate_rings() -> Vec<(f64, Vec<Vec<V2>>)> {
    let text = include_str!("plate-rings.txt");
    let mut out = Vec::new();
    let mut z = 0.0;
    let mut rings: Vec<Vec<V2>> = Vec::new();
    let mut ring: Vec<V2> = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("#") => {
                if !ring.is_empty() {
                    rings.push(std::mem::take(&mut ring));
                }
                if !rings.is_empty() {
                    out.push((z, std::mem::take(&mut rings)));
                }
                z = words.next().and_then(|w| w.parse().ok()).unwrap_or(0.0);
            }
            Some("") | None => {
                if !ring.is_empty() {
                    rings.push(std::mem::take(&mut ring));
                }
            }
            Some(x) => {
                let y = words.next().expect("a coordinate pair");
                ring.push(V2::new(
                    x.parse().expect("x is a number"),
                    y.parse().expect("y is a number"),
                ));
            }
        }
    }
    if !ring.is_empty() {
        rings.push(ring);
    }
    if !rings.is_empty() {
        out.push((z, rings));
    }
    out
}
