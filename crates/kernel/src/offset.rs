//! Polygon offsets, positive outward, with the crossings they create resolved by
//! winding number.
//!
//! An offset is not an operation that can be done edge by edge and then stitched
//! back together. Moving every edge out by the same distance makes them cross
//! each other, and the result is a set again only after those crossings have been
//! resolved: on a shrinking offset the shape folds through itself, on a growing
//! one two sides meet. The displacement produces a closed path per input ring
//! that may overlap itself and its neighbours arbitrarily — a polygon soup — and
//! what is left is the part of the plane with positive winding.
//!
//! That is the whole algorithm, and it is why this module is a wrapper rather than
//! a walk along the displaced edges. Selecting the valid portions of a soup and
//! ordering them into rings is a union; doing it by following the pieces produces
//! the same answer only when the case is easy, and a real sliced layer is not.
//!
//! The union runs on an integer lattice at 1 nm, because a union over `f64` has to
//! re-insert intersection points that immediately create new near-crossings, and
//! that is what produces slivers. A closing snaps once, runs both halves on one
//! lattice, and snaps back once, so the points lost to rounding going one way are
//! not different points coming back.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::single::SingleIntOverlay;
use i_overlay::i_float::int::angle::Angle;
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::i_shape::int::shape::{IntContour, IntShapes};
use i_overlay::mesh::int::arc::ArcOptions;
use i_overlay::mesh::int::outline::offset::IntOutlineOffset;
use i_overlay::mesh::int::style::{IntLineJoin, IntOutlineStyle};

use crate::geom::{ring_signed_area, V2};

/// Lattice units per millimetre: one nanometre.
const UNITS_PER_MM: f64 = 1.0e6;

/// The ratio of a miter's reach to the offset at which a miter stops being worth
/// keeping.
///
/// A corner with interior angle `a` reaches `offset / sin(a / 2)`, so the ratio
/// depends on the angle alone: a limit of 3 keeps corners sharper than 38.9
/// degrees square instead of spearing them out, which is also what PrusaSlicer
/// uses, and matching it is what keeps wall placement comparable.
const MITER_LIMIT: f64 = 3.0;

/// How an offset treats a corner that turns away from the material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// Follow a circle of the offset radius: the offset of the set by a disc.
    ///
    /// This is the shape a wall loop wants, because a wall is the boundary of a
    /// disc-shaped brush swept along the outline.
    Round,
    /// Follow the two displaced edges to where they meet.
    ///
    /// A round join and a miter join differ only at corners that the offset
    /// exposes, and they are what makes closing a region round it or not:
    /// growing with a miter keeps a square corner square, so the shrink that
    /// follows gives the original corner back instead of a rounded one.
    Miter,
}

/// Offsets a region's rings by `delta` millimetres, positive outward.
///
/// Rings are expected in the region's normal form (outer counter-clockwise,
/// holes clockwise) and come back in it. `arc_tol` is the largest chord error
/// allowed at a rounded corner. A shrinking offset that removes everything
/// returns no rings: an object thinner than twice the offset is gone.
pub fn offset_rings(rings: &[Vec<V2>], delta: f64, arc_tol: f64, join: Join) -> Vec<Vec<V2>> {
    if rings.is_empty() || delta == 0.0 {
        return rings.to_vec();
    }
    lattice(rings, delta, 0.0, arc_tol, join)
}

/// A closing: grow by `radius` and shrink back, both on one lattice.
///
/// A shape with no crack in it comes back unchanged, which is what makes this the
/// operation for sealing a wall cross-section: the outward half cannot leave a
/// crack behind and the inward half restores the outline.
pub fn closing(rings: &[Vec<V2>], radius: f64, arc_tol: f64, join: Join) -> Vec<Vec<V2>> {
    if rings.is_empty() || radius <= 0.0 {
        return rings.to_vec();
    }
    lattice(rings, radius, -radius, arc_tol, join)
}

/// Combines two ring sets into the part of the plane where both have material.
///
/// Set operations have to run on the same lattice as the offsets, for the same
/// reason the offset does: an intersection point computed in floating point
/// creates new near-crossings the moment it is inserted, and the result turns
/// into slivers. Both sets are snapped once to a lattice addressed from their
/// common bounds, so they stay in the same frame.
pub fn union_rings(a: &[Vec<V2>], b: &[Vec<V2>]) -> Vec<Vec<V2>> {
    operated(a, b, OverlayRule::Union)
}

/// Subtracts `clip` from `subj`: a \\ b.
pub fn difference_rings(subj: &[Vec<V2>], clip: &[Vec<V2>]) -> Vec<Vec<V2>> {
    if clip.is_empty() {
        return subj.to_vec();
    }
    operated(subj, clip, OverlayRule::Difference)
}

/// The part of the plane where both sets have material.
pub fn intersect_rings(a: &[Vec<V2>], b: &[Vec<V2>]) -> Vec<Vec<V2>> {
    operated(a, b, OverlayRule::Intersect)
}

/// Runs one Boolean rule over two ring sets on a shared lattice.
fn operated(a: &[Vec<V2>], b: &[Vec<V2>], rule: OverlayRule) -> Vec<Vec<V2>> {
    if a.is_empty() || b.is_empty() {
        return match rule {
            OverlayRule::Difference | OverlayRule::Intersect => Vec::new(),
            _ => {
                if a.is_empty() {
                    b.to_vec()
                } else {
                    a.to_vec()
                }
            }
        };
    }
    let origin = origin_of(&[a, b]);
    let subj = snap_shapes(a, origin);
    let clip = snap_shapes(b, origin);
    unsnap(subj.overlay(&clip, rule, FillRule::Positive), origin)
}

/// Runs one or two offsets on a single lattice, snapping and unsnapping once.
fn lattice(rings: &[Vec<V2>], first: f64, second: f64, arc_tol: f64, join: Join) -> Vec<Vec<V2>> {
    let origin = centre(rings);
    let mut shapes = snap(rings, origin);
    if shapes.is_empty() {
        return Vec::new();
    }
    for delta in [first, second] {
        if delta == 0.0 {
            continue;
        }
        shapes = match shapes.outline(&style(delta, arc_tol, join)) {
            Ok(next) => next,
            Err(_) => return Vec::new(),
        };
        if shapes.is_empty() {
            return Vec::new();
        }
    }
    unsnap(shapes, origin)
}

/// Where a layer's coordinates are moved to before they are snapped.
///
/// The integer engine needs room for the offsets and joins, so a layer is centred
/// rather than left wherever the plate put it; at 1 nm and a 200 mm part this keeps
/// every coordinate an order of magnitude below the integer ceiling.
fn centre(rings: &[Vec<V2>]) -> V2 {
    origin_of(&[rings])
}

/// Centre of the bounds of several ring sets, which two sets must share to be
/// snapped into one frame.
fn origin_of(sets: &[&[Vec<V2>]]) -> V2 {
    let mut low = V2::new(f64::MAX, f64::MAX);
    let mut high = V2::new(f64::MIN, f64::MIN);
    for set in sets {
        for ring in *set {
            for p in ring {
                low = V2::new(low.x.min(p.x), low.y.min(p.y));
                high = V2::new(high.x.max(p.x), high.y.max(p.y));
            }
        }
    }
    if low.x > high.x {
        return V2::new(0.0, 0.0);
    }
    (low + high) * 0.5
}

/// Snaps rings onto the lattice, nesting them into shapes the integer engine
/// reads as filled sets.
///
/// Snaps rings onto the lattice, one shape per ring.
///
/// This is what the offset engine wants: it takes each shape with its own
/// winding and resolves the displacement by the winding number of the whole
/// soup, so a hole is a clockwise ring beside its outline, not a contour of it.
fn snap(rings: &[Vec<V2>], origin: V2) -> IntShapes<i32> {
    let mut shapes = IntShapes::new();
    for ring in rings {
        match snapped(ring, origin) {
            Some(contour) => shapes.push(vec![contour]),
            None => continue,
        }
    }
    shapes
}

/// One ring snapped to the lattice, or `None` when the snap left it degenerate.
fn snapped(ring: &[V2], origin: V2) -> Option<IntContour<i32>> {
    let mut contour: IntContour<i32> = IntContour::new();
    for p in ring {
        let q = IntPoint::new(
            ((p.x - origin.x) * UNITS_PER_MM).round() as i32,
            ((p.y - origin.y) * UNITS_PER_MM).round() as i32,
        );
        if contour.last() != Some(&q) {
            contour.push(q);
        }
    }
    if contour.len() > 1 && contour.first() == contour.last() {
        contour.pop();
    }
    (contour.len() >= 3).then_some(contour)
}

/// Snaps rings onto the lattice, nesting them into shapes a Boolean reads as a
/// filled set.
///
/// A ring's depth decides whether it adds material or removes it: an even depth
/// is an outline, an odd depth a hole. Orienting them that way before the
/// overlay is what makes the operations independent of which way a caller wound
/// its input, and putting each hole inside its own outline's shape is what the
/// integer fill rule requires — a lone clockwise contour is a negative area, not
/// a hole in anything.
fn snap_shapes(rings: &[Vec<V2>], origin: V2) -> IntShapes<i32> {
    let mut kept: Vec<Vec<V2>> = Vec::with_capacity(rings.len());
    let mut contours: Vec<IntContour<i32>> = Vec::with_capacity(rings.len());
    for ring in rings {
        if let Some(contour) = snapped(ring, origin) {
            kept.push(ring.clone());
            contours.push(contour);
        }
    }

    let depths: Vec<usize> = (0..kept.len())
        .map(|i| {
            (0..kept.len())
                .filter(|&j| j != i && crate::geom::ring_contains(&kept[j], kept[i][0]))
                .count()
        })
        .collect();

    let mut shapes = IntShapes::new();
    for (index, contour) in contours.iter().enumerate() {
        if depths[index] % 2 != 0 {
            continue;
        }
        let mut shape: Vec<IntContour<i32>> = vec![oriented(contour, true)];
        for (other, candidate) in contours.iter().enumerate() {
            if depths[other] == depths[index] + 1
                && crate::geom::ring_contains(&kept[index], kept[other][0])
            {
                shape.push(oriented(candidate, false));
            }
        }
        shapes.push(shape);
    }
    shapes
}

/// Returns the contour wound the way its role asks, reversing if needed.
fn oriented(contour: &IntContour<i32>, counter_clockwise: bool) -> IntContour<i32> {
    let mut area = 0_i128;
    for index in 0..contour.len() {
        let a = contour[index];
        let b = contour[(index + 1) % contour.len()];
        area += (a.x as i128) * (b.y as i128) - (b.x as i128) * (a.y as i128);
    }
    let mut out = contour.clone();
    if (area > 0) != counter_clockwise {
        out.reverse();
    }
    out
}

/// Area below which a ring is lattice debris rather than geometry.
///
/// One square micrometre: a ring that encloses less than that is a few lattice
/// cells across and cannot be printed or measured, and the union leaves such
/// rings behind where two boundaries meet almost tangentially. Upstream's answer
/// to the same problem is to clean the output; this drops what cleaning would.
const DEBRIS: f64 = 1.0e-6;

/// Reads the lattice back, dropping rings whose vertices all collapsed.
fn unsnap(shapes: IntShapes<i32>, origin: V2) -> Vec<Vec<V2>> {
    let mut rings = Vec::new();
    for shape in shapes {
        for contour in shape {
            if contour.len() < 3 {
                continue;
            }
            let ring: Vec<V2> = contour
                .iter()
                .map(|p| {
                    V2::new(
                        origin.x + p.x as f64 / UNITS_PER_MM,
                        origin.y + p.y as f64 / UNITS_PER_MM,
                    )
                })
                .collect();
            if ring_signed_area(&ring).abs() >= DEBRIS {
                rings.push(ring);
            }
        }
    }
    rings
}

/// The integer offset style for a millimetre offset on the lattice.
fn style(delta: f64, arc_tol: f64, join: Join) -> IntOutlineStyle<i32> {
    let reach = (delta.abs() * UNITS_PER_MM).round() as i32;
    let mut style = IntOutlineStyle::<i32>::new(if delta < 0.0 { -reach } else { reach });
    style.join = match join {
        Join::Miter => {
            let angle = 2.0 * (1.0 / MITER_LIMIT).asin();
            IntLineJoin::Miter(Angle::from_radians(angle).expect("finite angle"))
        }
        Join::Round => {
            let step = if delta.abs() > arc_tol {
                2.0 * (1.0 - arc_tol / delta.abs()).acos()
            } else {
                std::f64::consts::FRAC_PI_2
            };
            IntLineJoin::Round(ArcOptions {
                max_step: Angle::from_radians(step).expect("finite angle"),
                ..ArcOptions::default()
            })
        }
    };
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Bounds;

    fn round(rings: &[Vec<V2>], delta: f64, tol: f64) -> Vec<Vec<V2>> {
        offset_rings(rings, delta, tol, Join::Round)
    }

    fn square(size: f64) -> Vec<V2> {
        vec![
            V2::new(0.0, 0.0),
            V2::new(size, 0.0),
            V2::new(size, size),
            V2::new(0.0, size),
        ]
    }

    fn area(rings: &[Vec<V2>]) -> f64 {
        rings.iter().map(|r| ring_signed_area(r)).sum::<f64>().abs()
    }

    #[test]
    fn growing_a_square_adds_a_monotone_boundary() {
        let out = round(&[square(10.0)], 1.0, 0.05);
        assert_eq!(out.len(), 1);
        // A round join of radius 1 takes (4 - pi) away from the square.
        assert!((area(&out) - 143.14).abs() < 0.5, "area {}", area(&out));
        let b = out[0].iter().fold(Bounds::empty(), |mut acc, &p| {
            acc.add(p);
            acc
        });
        assert!((b.min.x + 1.0).abs() < 1e-5);
        assert!((b.max.x - 11.0).abs() < 1e-5);
    }

    #[test]
    fn shrinking_a_square_keeps_the_corners_sharp() {
        let out = round(&[square(10.0)], -1.0, 0.05);
        assert_eq!(out.len(), 1);
        assert!((area(&out) - 64.0).abs() < 0.2, "area {}", area(&out));
    }

    #[test]
    fn shrinking_past_the_middle_removes_the_square() {
        assert!(round(&[square(10.0)], -6.0, 0.05).is_empty());
    }

    #[test]
    fn a_hole_shrinks_when_the_region_grows() {
        let mut hole = square(4.0);
        hole.reverse();
        for p in hole.iter_mut() {
            *p = *p + V2::new(3.0, 3.0);
        }
        let out = round(&[square(10.0), hole], 0.5, 0.05);
        assert_eq!(out.len(), 2, "outer plus shrunken hole");
        let total = area(&out);
        assert!((total - (121.0 - 9.0)).abs() < 0.5, "area {total}");
    }

    #[test]
    fn a_hole_smaller_than_the_offset_closes() {
        let mut hole = square(2.0);
        hole.reverse();
        for p in hole.iter_mut() {
            *p = *p + V2::new(4.0, 4.0);
        }
        let out = round(&[square(10.0), hole], 1.5, 0.05);
        assert_eq!(out.len(), 1, "the hole is swallowed");
        assert!((area(&out) - 167.0).abs() < 1.5, "area {}", area(&out));
    }

    #[test]
    fn a_slit_closes_when_the_region_grows() {
        // A square with a slit cut from one side almost to the opposite side.
        let ring = vec![
            V2::new(0.0, 0.0),
            V2::new(10.0, 0.0),
            V2::new(10.0, 10.0),
            V2::new(5.05, 10.0),
            V2::new(5.05, 0.2),
            V2::new(4.95, 0.2),
            V2::new(4.95, 10.0),
            V2::new(0.0, 10.0),
        ];
        let before = area(std::slice::from_ref(&ring));
        let out = round(&[ring], 0.1, 0.02);
        assert_eq!(out.len(), 1, "the slit closes into one ring");
        let after = area(&out);
        assert!(after > before + 0.5, "area {before} -> {after}");
    }

    #[test]
    fn a_dumbbell_splits_when_the_region_shrinks() {
        let ring = vec![
            V2::new(0.0, 0.0),
            V2::new(4.0, 0.0),
            V2::new(4.0, 1.75),
            V2::new(6.0, 1.75),
            V2::new(6.0, 0.0),
            V2::new(10.0, 0.0),
            V2::new(10.0, 4.0),
            V2::new(6.0, 4.0),
            V2::new(6.0, 2.25),
            V2::new(4.0, 2.25),
            V2::new(4.0, 4.0),
            V2::new(0.0, 4.0),
        ];
        let out = round(&[ring], -0.5, 0.05);
        assert_eq!(out.len(), 2, "the neck is cut through");
        let total = area(&out);
        assert!((total - 2.0 * 9.0).abs() < 1.0, "area {total}");
    }

    #[test]
    fn a_circle_offsets_by_its_radius() {
        let n = 720;
        let ring: Vec<V2> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                V2::new(20.0 * t.cos(), 20.0 * t.sin())
            })
            .collect();
        let out = round(&[ring], 2.0, 0.01);
        assert_eq!(out.len(), 1);
        for &p in &out[0] {
            assert!((p.len() - 22.0).abs() < 0.02, "radius {}", p.len());
        }
    }

    #[test]
    fn shrinking_a_circle_by_its_radius_leaves_nothing() {
        let n = 360;
        let ring: Vec<V2> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                V2::new(10.0 * t.cos(), 10.0 * t.sin())
            })
            .collect();
        let out = round(&[ring], -10.5, 0.05);
        assert!(out.is_empty(), "{} rings left", out.len());
    }

    #[test]
    fn two_outlines_merge_when_the_region_grows_over_the_gap() {
        let mut apart = square(10.0);
        for p in apart.iter_mut() {
            *p = *p + V2::new(10.05, 0.0);
        }
        let out = offset_rings(&[square(10.0), apart], 0.049, 0.002, Join::Miter);
        assert_eq!(out.len(), 1, "a 0.05 mm crack closes into one outline");
        // Two 10 mm squares, the 0.05 mm crack between them, and the 0.049 mm
        // skirt the growth adds all the way round.
        assert!((area(&out) - 203.47).abs() < 0.05, "area {}", area(&out));
        assert!(
            crate::geom::ring_contains(&out[0], V2::new(10.025, 5.0)),
            "the crack itself is material"
        );
    }

    #[test]
    fn miter_closing_gives_a_square_back() {
        let grown = offset_rings(&[square(10.0)], 1.0, 0.002, Join::Miter);
        assert_eq!(grown.len(), 1);
        assert!((area(&grown) - 144.0).abs() < 1e-6, "area {}", area(&grown));
        let back = offset_rings(&grown, -1.0, 0.002, Join::Miter);
        assert_eq!(back.len(), 1);
        assert!((area(&back) - 100.0).abs() < 1e-4, "area {}", area(&back));
        for &p in &back[0] {
            assert!(
                (p.x - 0.0).abs() < 1e-4
                    || (p.x - 10.0).abs() < 1e-4
                    || (p.y - 0.0).abs() < 1e-4
                    || (p.y - 10.0).abs() < 1e-4,
                "corner moved to {p:?}"
            );
        }
    }

    #[test]
    fn zero_offset_is_the_identity() {
        let rings = vec![square(10.0)];
        assert_eq!(round(&rings, 0.0, 0.05), rings);
    }

    fn moved(ring: &[V2], by: V2) -> Vec<V2> {
        ring.iter().map(|&p| p + by).collect()
    }

    #[test]
    fn a_difference_leaves_the_part_of_the_subject_the_clip_does_not_cover() {
        let subj = vec![square(10.0)];
        let clip = vec![moved(&square(4.0), V2::new(3.0, 3.0))];
        let out = difference_rings(&subj, &clip);
        assert_eq!(out.len(), 2, "an inner cut leaves an outer and a hole");
        assert!((area(&out) - 84.0).abs() < 1e-4, "area {}", area(&out));
    }

    #[test]
    fn a_difference_that_clears_the_subject_returns_nothing() {
        let subj = vec![square(4.0)];
        let clip = vec![moved(&square(10.0), V2::new(-3.0, -3.0))];
        assert!(difference_rings(&subj, &clip).is_empty());
    }

    #[test]
    fn a_union_of_two_squares_over_the_gap_is_one_ring() {
        let a = vec![square(10.0)];
        let b = vec![moved(&square(10.0), V2::new(9.0, 0.0))];
        let out = union_rings(&a, &b);
        assert_eq!(out.len(), 1, "an overlap merges into one outline");
        assert!((area(&out) - 190.0).abs() < 1e-4, "area {}", area(&out));
    }

    #[test]
    fn a_union_of_disjoint_squares_keeps_both() {
        let a = vec![square(10.0)];
        let b = vec![moved(&square(4.0), V2::new(20.0, 0.0))];
        let out = union_rings(&a, &b);
        assert_eq!(out.len(), 2);
        assert!((area(&out) - 116.0).abs() < 1e-4, "area {}", area(&out));
    }

    #[test]
    fn an_intersection_is_the_overlap_only() {
        let a = vec![square(10.0)];
        let b = vec![moved(&square(10.0), V2::new(8.0, 0.0))];
        let out = intersect_rings(&a, &b);
        assert_eq!(out.len(), 1);
        assert!((area(&out) - 20.0).abs() < 1e-4, "area {}", area(&out));
    }

    #[test]
    fn an_empty_operand_falls_back_to_the_other_side() {
        let a = vec![square(10.0)];
        let nothing: Vec<Vec<V2>> = Vec::new();
        assert!((area(&union_rings(&a, &nothing)) - 100.0).abs() < 1e-4);
        assert!((area(&union_rings(&nothing, &a)) - 100.0).abs() < 1e-4);
        assert!((area(&difference_rings(&a, &nothing)) - 100.0).abs() < 1e-4);
        assert!(difference_rings(&nothing, &a).is_empty());
        assert!(intersect_rings(&a, &nothing).is_empty());
    }

    #[test]
    fn a_hole_in_the_clip_does_not_cut_the_subject() {
        let subj = vec![square(10.0)];
        let mut hole = moved(&square(2.0), V2::new(4.0, 4.0));
        hole.reverse();
        let clip = vec![moved(&square(6.0), V2::new(2.0, 2.0)), hole];
        let out = difference_rings(&subj, &clip);
        let expect = 100.0 - (36.0 - 4.0);
        assert!((area(&out) - expect).abs() < 1e-4, "area {}", area(&out));
    }
}
