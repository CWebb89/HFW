//! Slice repair: close gaps in a cross-section and thin its rings.
//!
//! A sliced cross-section inherits every hairline the mesh had. Two facets that
//! meet a few hundredths of a millimetre apart slice as two contours with a
//! crack between them, and a hole that narrow prints as nothing but nozzle
//! pressure. `slice_closing_radius` is upstream's answer and this is the same
//! contract: a crack narrower than twice the radius is filled, a hole smaller
//! than the radius is dropped.
//!
//! The operation is a morphological closing: grow the cross-section by the
//! radius, shrink it back by the same. A shape with no crack in it comes back
//! unchanged; a crack narrower than the growth fills and cannot reopen. It is
//! done by polygon offset, in `hfw_kernel::offset_overlay`, which resolves the
//! crossings the growth creates by winding number rather than by pairing the
//! moved edges back up.
//!
//! This replaces a pairwise pull that moved nearby boundary points together. The
//! pull was cheap but it decided the topology itself — which pair of contour ends
//! a crack joins — and on a real plate it decided wrong often enough to be worth
//! abandoning. A closing does not have that failure mode: the topology falls out
//! of the union, so nothing depends on picking the right pair.

use hfw_kernel::offset::{closing, Join};
use hfw_kernel::{ring_signed_area, seg_closest, Region, V2};

/// Chord error allowed where a corner is rounded.
///
/// Closing uses miter joins, so this only matters if a caller asks for round
/// ones: at a wall width it is a fifth of a percent of the radius, well under the
/// 1 µm that a slicer treats as meaningful.
const ARC_TOL: f64 = 0.001;

fn ring_length(ring: &[V2]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| ring[i].dist(ring[(i + 1) % n])).sum()
}

/// Fills cracks narrower than `2 * radius` and drops holes smaller than the
/// radius.
///
/// Returns the region unchanged when the radius is zero or negative.
pub fn close_gaps(region: &Region, radius: f64) -> Region {
    if radius <= 0.0 || region.is_empty() {
        return region.clone();
    }
    Region::new(closing(&region.rings, radius, ARC_TOL, Join::Miter))
}

/// Douglas-Peucker over each ring, keeping a ring whole when simplifying it would
/// cross itself, flip its winding or move more area than the tolerance allows.
///
/// A ring whose simplification would cross itself, flip its winding or move more
/// area than the tolerance allows is kept at full resolution: the tolerance is a
/// promise about detail, not permission to lose a feature.
pub fn simplify_region(region: &Region, tolerance: f64) -> Region {
    if tolerance <= 0.0 || region.is_empty() {
        return region.clone();
    }
    let rings: Vec<Vec<V2>> = region
        .rings
        .iter()
        .map(|ring| {
            let simple = simplify_ring(ring, tolerance);
            let budget = tolerance * ring_length(ring);
            let sane = simple.len() >= 3
                && simple.len() <= ring.len()
                && ring_signed_area(&simple).signum() == ring_signed_area(ring).signum()
                && (ring_signed_area(&simple) - ring_signed_area(ring)).abs() <= budget;
            if sane {
                simple
            } else {
                ring.clone()
            }
        })
        .collect();
    Region::new(rings)
}

/// Douglas-Peucker on a closed ring, anchored at its first point.
fn simplify_ring(ring: &[V2], tolerance: f64) -> Vec<V2> {
    if ring.len() < 4 {
        return ring.to_vec();
    }
    let mut points = ring.to_vec();
    points.push(ring[0]);
    let last = points.len() - 1;
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[last] = true;
    let mut stack = vec![(0usize, last)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut worst = 0.0;
        let mut at = a;
        for k in a + 1..b {
            let (d, _) = seg_closest(points[k], points[a], points[b]);
            if d > worst {
                worst = d;
                at = k;
            }
        }
        if worst > tolerance {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    (0..last).filter(|&k| keep[k]).map(|k| points[k]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lattice the offset runs on, in millimetres: closing snaps to it,
    /// grows and shrinks on it, and snaps back, so a point that survives moves
    /// by no more than half a unit.
    const LATTICE: f64 = 1e-6;

    fn is_hole(ring: &[V2]) -> bool {
        ring_signed_area(ring) < 0.0
    }

    fn square(x0: f64, y0: f64, side: f64) -> Vec<V2> {
        vec![
            V2::new(x0, y0),
            V2::new(x0 + side, y0),
            V2::new(x0 + side, y0 + side),
            V2::new(x0, y0 + side),
        ]
    }

    fn twin_plates(gap: f64) -> Region {
        Region::new(vec![square(0.0, 0.0, 10.0), square(10.0 + gap, 0.0, 10.0)])
    }

    /// No point may move further than the lattice rounding, half a unit, and the
    /// closing may rotate a ring's starting point or reverse it.
    fn shifted(a: &[V2], b: &[V2]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let near = |p: &V2, q: &V2| p.dist(*q) <= LATTICE;
        let matched = |offset: usize, reversed: bool| {
            (0..a.len()).all(|k| {
                let j = if reversed {
                    (offset + a.len() - k) % a.len()
                } else {
                    (offset + k) % a.len()
                };
                near(&a[k], &b[j])
            })
        };
        (0..a.len()).any(|offset| matched(offset, false) || matched(offset, true))
    }

    #[test]
    fn a_crack_narrower_than_the_radius_is_filled() {
        let region = twin_plates(0.05);
        assert!(
            !region.contains(V2::new(10.025, 5.0)),
            "the fixture starts cracked"
        );
        let closed = close_gaps(&region, 0.049);
        assert_eq!(closed.rings.len(), 1, "the two plates join into one");
        assert!(
            closed.contains(V2::new(10.025, 5.0)),
            "a 0.05 mm crack is narrower than the 0.098 mm reach and should be filled"
        );
        assert!(
            (closed.area() - 200.5).abs() < 0.01,
            "filling the crack should add exactly its area, got {}",
            closed.area()
        );
    }

    #[test]
    fn a_gap_wider_than_twice_the_radius_is_left_alone() {
        let region = twin_plates(0.2);
        let closed = close_gaps(&region, 0.049);
        assert!(
            !closed.contains(V2::new(10.1, 5.0)),
            "a 0.2 mm gap is wider than the 0.098 reach and must stay open"
        );
        assert_eq!(closed.rings.len(), 2, "the plates stay separate");
        for (before, after) in region.rings.iter().zip(&closed.rings) {
            assert!(
                shifted(&Region::new(vec![before.clone()]).rings[0], after),
                "an untouched ring may only move by the lattice"
            );
        }
    }

    #[test]
    fn a_hole_smaller_than_the_radius_is_dropped() {
        let region = Region::new(vec![square(0.0, 0.0, 20.0), square(10.0, 10.0, 0.05)]);
        assert_eq!(
            region.rings.len(),
            2,
            "the fixture starts as a plate with a hole"
        );
        assert!(is_hole(&Region::new(region.rings.clone()).rings[1]));
        let closed = close_gaps(&region, 0.049);
        assert_eq!(closed.rings.len(), 1, "a 0.05 mm hole should close");
        assert_eq!(
            close_gaps(&region, 0.02).rings.len(),
            2,
            "a 0.02 mm radius is a tenth of the hole's area and must leave it"
        );
    }

    #[test]
    fn a_small_hole_is_filled_and_a_notch_is_welded() {
        let plate = Region::new(vec![square(0.0, 0.0, 20.0), square(10.0, 5.0, 0.04)]);
        let filled = close_gaps(&plate, 0.3);
        assert!(
            filled.contains(V2::new(10.02, 5.02)),
            "a 0.04 mm hole is air and closing must fill it"
        );
        let notch = Region::new(vec![vec![
            V2::new(0.0, 0.0),
            V2::new(20.0, 0.0),
            V2::new(20.0, 20.0),
            V2::new(10.02, 20.0),
            V2::new(10.0, 12.0),
            V2::new(9.98, 20.0),
            V2::new(0.0, 20.0),
        ]]);
        let welded = close_gaps(&notch, 0.2);
        assert!(
            welded.area() > 399.9,
            "a 0.04 mm notch is air and closing must weld it, {} became {}",
            notch.area(),
            welded.area()
        );
    }

    #[test]
    fn a_thin_spike_survives() {
        let spike = Region::new(vec![vec![
            V2::new(0.0, 0.0),
            V2::new(20.0, 0.0),
            V2::new(20.0, 20.0),
            V2::new(10.02, 20.0),
            V2::new(10.02, 25.0),
            V2::new(9.98, 25.0),
            V2::new(9.98, 20.0),
            V2::new(0.0, 20.0),
        ]]);
        assert!(
            (spike.area() - 400.2).abs() < 0.01,
            "fixture area {}",
            spike.area()
        );
        let kept = close_gaps(&spike, 0.2);
        assert!(
            kept.contains(V2::new(10.0, 22.0)),
            "a 0.04 mm spike is material, and closing never removes material"
        );
        assert!(
            (kept.area() - spike.area()).abs() < 0.01,
            "closing must not eat a spike, {} became {}",
            spike.area(),
            kept.area()
        );
    }

    #[test]
    fn a_slit_is_filled_from_the_mouth_in() {
        let plate = vec![
            V2::new(0.0, 0.0),
            V2::new(20.0, 0.0),
            V2::new(20.0, 20.0),
            V2::new(10.025, 20.0),
            V2::new(10.025, 5.0),
            V2::new(9.975, 5.0),
            V2::new(9.975, 20.0),
            V2::new(0.0, 20.0),
        ];
        let region = Region::new(vec![plate]);
        assert_eq!(region.rings.len(), 1);
        assert!(
            !region.contains(V2::new(10.0, 15.0)),
            "the slit starts open"
        );
        let closed = close_gaps(&region, 0.049);
        assert!(
            closed.contains(V2::new(10.0, 15.0)),
            "a 0.05 mm slit should be welded shut along its length"
        );
        assert!(
            (closed.area() - 400.0).abs() < 1.0,
            "welding a slit fills it without moving the outer boundary, got {}",
            closed.area()
        );
    }

    #[test]
    fn a_layer_without_a_crack_comes_back_unchanged() {
        let region = Region::new(vec![square(0.0, 0.0, 20.0), square(5.0, 5.0, 5.0)]);
        let closed = close_gaps(&region, 0.049);
        assert_eq!(closed.rings.len(), region.rings.len());
        for (before, after) in region.rings.iter().zip(&closed.rings) {
            assert!(
                shifted(before, after),
                "a closed ring may only move by the lattice"
            );
        }
        assert!((closed.area() - region.area()).abs() < 1e-4);
        assert_eq!(simplify_region(&region, 0.0).rings, region.rings);
    }

    #[test]
    fn simplification_spends_its_tolerance_on_detail() {
        let mut ring = Vec::new();
        for k in 0..64 {
            let a = std::f64::consts::TAU * k as f64 / 64.0;
            ring.push(V2::new(20.0 * a.cos(), 20.0 * a.sin()));
        }
        let region = Region::new(vec![ring]);
        let coarse = simplify_region(&region, 0.2);
        let fine = simplify_region(&region, 0.001);
        assert!(
            coarse.rings[0].len() <= region.rings[0].len() / 2,
            "a 0.2 mm tolerance should drop most of a 64-point circle, kept {}",
            coarse.rings[0].len()
        );
        assert_eq!(
            fine.rings[0].len(),
            region.rings[0].len(),
            "a tolerance well under the sagitta must keep every point"
        );
        assert!(
            (coarse.area() - region.area()).abs() < 0.02 * region.area(),
            "simplifying a circle must not change its area by more than the tolerance allows"
        );
    }
}
