//! Whole-region offset: grow or shrink a cross-section as one solid.
//!
//! Offsetting each ring on its own is wrong the moment two features are close:
//! outward offsets of neighbouring outlines overlap, and an inward offset of a
//! hole either vanishes or turns inside out. The region is therefore treated as
//! a set, not as a list of rings, and the offset is the polygon offset in
//! `hfw_kernel`: displaced edges are allowed to cross, and the crossings are
//! resolved by winding, so a hole smaller than twice the offset closes, features
//! that grow into each other merge, and a shape thinner than twice a shrink
//! disappears, all without a special case.
//!
//! The join is round because that is the offset by a disc, which is what a
//! physical offset is; a wall loop wants a miter instead and asks the kernel for
//! one directly.

use hfw_kernel::{offset_rings, Join, Region, V2};

/// Largest chord error allowed at a rounded corner for resolution `h`.
///
/// The caller passes its sampling resolution, which used to be the grid the
/// iso-line was traced on. Offsetting no longer samples anything, but the number
/// still carries the accuracy the caller asked for, so it becomes the arc
/// tolerance: half of it keeps a corner at least as close to a true arc as the
/// raster was.
fn arc_tolerance(h: f64) -> f64 {
    if h.is_finite() && h > 0.0 {
        0.5 * h
    } else {
        0.005
    }
}

/// Offsets `region` by `grow` millimetres, positive outward, at resolution `h`.
///
/// Returns the region unchanged when `grow` is zero. A shrinking offset that
/// removes every feature returns an empty region: an object thinner than twice
/// the offset is gone, which is what the caller asked for.
pub fn offset_region(region: &Region, grow: f64, h: f64) -> Region {
    if grow == 0.0 || region.is_empty() {
        return region.clone();
    }
    let rings: Vec<Vec<V2>> = offset_rings(&region.rings, grow, arc_tolerance(h), Join::Round);
    Region::new(rings)
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
        let grown = offset_region(&Region::new(vec![square(10.0)]), 0.5, 0.1);
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
        let shrunk = offset_region(&Region::new(vec![square(10.0)]), -0.5, 0.1);
        let (x0, y0, x1, y1) = extents(&shrunk);
        assert!((x0 - 0.5).abs() < 0.02, "left {x0}");
        assert!((y0 - 0.5).abs() < 0.02, "bottom {y0}");
        assert!((x1 - 9.5).abs() < 0.02, "right {x1}");
        assert!((y1 - 9.5).abs() < 0.02, "top {y1}");
    }

    #[test]
    fn growing_closes_a_hole_between_two_features() {
        let region = Region::new(vec![square(10.0), rect(11.0, 4.0, 13.0, 6.0)]);
        let grown = offset_region(&region, 1.2, 0.1);
        assert_eq!(
            grown.rings.len(),
            1,
            "a 2 mm gap closes into one solid well before 1.2 mm a side"
        );
    }

    #[test]
    fn shrinking_opens_a_ring_into_a_hole() {
        let ring = annulus(20.0, 4.0);
        let shrunk = offset_region(&ring, -1.0, 0.1);
        assert_eq!(shrunk.rings.len(), 2, "the hole grows, the outline shrinks");
        // The outline loses a sharp 18 mm square's worth; the 4 mm hole grows
        // into its disc offset, which is 16 mm2 of square plus 16 mm of edge
        // swept a millimetre plus the rounded corners.
        let expected = 18.0 * 18.0 - (16.0 + 16.0 + std::f64::consts::PI);
        assert!((shrunk.area() - expected).abs() < 0.5, "{}", shrunk.area());
    }

    #[test]
    fn a_hole_smaller_than_twice_the_growth_is_filled() {
        let ring = annulus(20.0, 2.0);
        let grown = offset_region(&ring, 1.2, 0.1);
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
        let shrunk = offset_region(&thin, -0.4, 0.1);
        assert!(shrunk.is_empty(), "0.3 mm cannot survive 0.4 mm a side");
    }

    #[test]
    fn zero_growth_is_a_no_op() {
        let region = annulus(20.0, 4.0);
        let same = offset_region(&region, 0.0, 0.1);
        assert_eq!(same.rings.len(), region.rings.len());
        assert!((same.area() - region.area()).abs() < 1e-9);
    }
}
