//! Whole-region offset: grow or shrink a cross-section as one solid.
//!
//! Offsetting each ring on its own is wrong the moment two features are close:
//! outward offsets of neighbouring outlines overlap, and an inward offset of a
//! hole either vanishes or turns inside out. The region is therefore treated as
//! a set, not as a list of rings: sample its signed distance on a grid, take the
//! iso-line at the requested offset, and rebuild the rings from that contour. A
//! hole smaller than twice the offset closes by itself, and features that grow
//! into each other merge, both without a special case.
//!
//! The grid decides only which iso-line to follow, never where its vertices sit:
//! every traced vertex is then walked onto the exact offset from the contour it
//! came from, so the result is as accurate as the offset ladder the wall
//! generator uses, not as accurate as the grid.

use hfw_kernel::{Region, SegIndex, V2};

use crate::contour;
use crate::field::{scan_inside, Grid};
use crate::WallsError;

/// Offsets `region` by `grow` millimetres, positive outward, at grid resolution
/// `h`.
///
/// Returns the region unchanged when `grow` is zero. A shrinking offset that
/// removes every feature returns an empty region: an object thinner than twice
/// the offset is gone, which is what the caller asked for.
pub fn offset_region(region: &Region, grow: f64, h: f64) -> Result<Region, WallsError> {
    if grow == 0.0 || region.is_empty() {
        return Ok(region.clone());
    }
    let reach = grow.abs() + 4.0 * h;
    let bounds = region.bounds().grown(reach);
    let index = SegIndex::new(region.segments(), bounds.grown(2.0 * reach), 2.0 * reach);
    let grid = Grid::covering(bounds, h);
    let inside = scan_inside(region, &grid);
    let phi: Vec<f64> = (0..grid.len())
        .map(|id| signed(&index, grid.pos_id(id), inside[id], reach) + grow)
        .collect();
    let nan = vec![f64::NAN; grid.len()];
    let mut rings = Vec::new();
    for raw in contour::trace(&grid, &phi, &nan, &nan) {
        if !raw.closed || raw.pts.len() < 3 {
            continue;
        }
        let ring: Vec<V2> = raw
            .pts
            .iter()
            .map(|&p| project(p, grow, &index, reach, h))
            .collect();
        rings.push(ring);
    }
    Ok(Region::new(rings))
}

/// Signed distance to the region boundary from one point: positive inside,
/// infinite when the boundary is further than `reach` away.
fn signed(index: &SegIndex, p: V2, inside: bool, reach: f64) -> f64 {
    match index.nearest(p, reach) {
        Some(hit) if inside => hit.d,
        Some(hit) => -hit.d,
        None if inside => f64::INFINITY,
        None => f64::NEG_INFINITY,
    }
}

/// Walks `p` onto the exact distance `grow` from the nearest contour edge.
///
/// The traced point is within half a cell of the iso-line, so a few steps of
/// Newton along the outward normal converge; the step is capped so a bad
/// nearest-edge guess in a corner cannot throw the vertex across the part. The
/// side of the boundary the iso-line sits on follows from the offset's sign: an
/// outward offset lands outside the region, an inward one inside.
fn project(mut p: V2, grow: f64, index: &SegIndex, reach: f64, h: f64) -> V2 {
    let inside = grow < 0.0;
    for _ in 0..6 {
        let Some(hit) = index.nearest(p, reach) else {
            break;
        };
        let out = p - hit.foot;
        let len = out.len();
        if len < 1e-12 {
            break;
        }
        let distance = if inside { hit.d } else { -hit.d };
        let error = distance + grow;
        if error.abs() < 1e-9 {
            break;
        }
        p = p + out.norm() * error.clamp(-0.75 * h, 0.75 * h);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f64) -> Vec<V2> {
        vec![
            V2::new(0.0, 0.0),
            V2::new(size, 0.0),
            V2::new(size, size),
            V2::new(0.0, size),
        ]
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<V2> {
        vec![
            V2::new(x0, y0),
            V2::new(x1, y0),
            V2::new(x1, y1),
            V2::new(x0, y1),
        ]
    }

    fn centered(size: f64, at: f64) -> Vec<V2> {
        rect(at, at, at + size, at + size)
    }

    fn annulus(outer: f64, inner: f64) -> Region {
        Region::new(vec![square(outer), centered(inner, 0.5 * (outer - inner))])
    }

    fn extents(region: &Region) -> (f64, f64, f64, f64) {
        let b = region.bounds();
        (b.min.x, b.min.y, b.max.x, b.max.y)
    }

    #[test]
    fn growing_a_square_moves_every_side_out() {
        let grown = offset_region(&Region::new(vec![square(10.0)]), 0.5, 0.1).expect("field");
        let (x0, y0, x1, y1) = extents(&grown);
        assert!((x0 + 0.5).abs() < 0.02, "left {x0}");
        assert!((y0 + 0.5).abs() < 0.02, "bottom {y0}");
        assert!((x1 - 10.5).abs() < 0.02, "right {x1}");
        assert!((y1 - 10.5).abs() < 0.02, "top {y1}");
        assert_eq!(grown.rings.len(), 1, "a convex square stays one ring");
        assert!((grown.area() - 121.0).abs() < 0.5, "area {}", grown.area());
    }

    #[test]
    fn shrinking_a_square_moves_every_side_in() {
        let shrunk = offset_region(&Region::new(vec![square(10.0)]), -0.5, 0.1).expect("field");
        let (x0, y0, x1, y1) = extents(&shrunk);
        assert!((x0 - 0.5).abs() < 0.02, "left {x0}");
        assert!((y0 - 0.5).abs() < 0.02, "bottom {y0}");
        assert!((x1 - 9.5).abs() < 0.02, "right {x1}");
        assert!((y1 - 9.5).abs() < 0.02, "top {y1}");
    }

    #[test]
    fn growing_closes_a_hole_between_two_features() {
        let region = Region::new(vec![square(10.0), rect(11.0, 4.0, 13.0, 6.0)]);
        let grown = offset_region(&region, 1.2, 0.1).expect("field");
        assert_eq!(
            grown.rings.len(),
            1,
            "a 2 mm gap closes into one solid well before 1.2 mm a side"
        );
    }

    #[test]
    fn shrinking_opens_a_ring_into_a_hole() {
        let ring = annulus(20.0, 4.0);
        let shrunk = offset_region(&ring, -1.0, 0.1).expect("field");
        assert_eq!(shrunk.rings.len(), 2, "the hole grows, the outline shrinks");
        assert!(
            (shrunk.area() - (18.0 * 18.0 - 6.0 * 6.0)).abs() < 1.0,
            "{}",
            shrunk.area()
        );
    }

    #[test]
    fn a_hole_smaller_than_twice_the_growth_is_filled() {
        let ring = annulus(20.0, 2.0);
        let grown = offset_region(&ring, 1.2, 0.1).expect("field");
        assert_eq!(
            grown.rings.len(),
            1,
            "the 2 mm hole closes at 1.2 mm a side"
        );
        assert!(grown.contains(V2::new(10.0, 10.0)), "the hole is solid now");
    }

    #[test]
    fn a_feature_thinner_than_twice_the_shrink_disappears() {
        let thin = Region::new(vec![vec![
            V2::new(0.0, 0.0),
            V2::new(30.0, 0.0),
            V2::new(30.0, 0.3),
            V2::new(0.0, 0.3),
        ]]);
        let shrunk = offset_region(&thin, -0.4, 0.1).expect("field");
        assert!(shrunk.is_empty(), "0.3 mm cannot survive 0.4 mm a side");
    }

    #[test]
    fn zero_growth_is_a_no_op() {
        let region = annulus(20.0, 4.0);
        let same = offset_region(&region, 0.0, 0.1).expect("field");
        assert_eq!(same.rings.len(), region.rings.len());
        assert!((same.area() - region.area()).abs() < 1e-9);
    }
}
