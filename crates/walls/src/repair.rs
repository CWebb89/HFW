//! Slice repair: close gaps in a cross-section and thin its rings.
//!
//! A sliced cross-section inherits every hairline the mesh had. Two facets that
//! meet a few hundredths of a millimetre apart slice as two contours with a
//! crack between them, and a hole that narrow prints as nothing but nozzle
//! pressure. `slice_closing_radius` is upstream's answer and this is the same
//! contract: a crack narrower than twice the radius is filled, a hole smaller
//! than the radius is dropped.
//!
//! It is not upstream's algorithm. Slic3r offsets the whole cross-section out by
//! the radius and back in, a morphological closing on integer polygons. Doing
//! that here would mean sampling a distance field at a hundredth of a
//! millimetre across the whole part, and every ring would carry the grid's
//! error even where nothing needed closing. Instead the boundary is moved only
//! where the crack is: two boundary points nearer than twice the radius, with
//! air between them, are pulled together onto their midpoint until they
//! coincide. Rings that are joined neither wander nor change shape, so a layer
//! without a crack comes back bit-identical.
//!
//! The "air between them" test is what separates a crack from a thin wall. Both
//! put two contour points within twice the radius; a closing fills the first and
//! must leave the second alone, because closing only ever adds material. The
//! midpoint settles it: outside the region means the pair bounds a gap.

use hfw_kernel::{ring_signed_area, seg_closest, Bounds, Region, V2};

/// How many times the boundary may be pulled together before giving up.
///
/// Moving one pair can bring another within reach, and a tapering crack needs a
/// few passes to close along its length. It converges: every pass halves the
/// gaps it touches, and a pass that moves nothing ends the loop.
const PASSES: usize = 8;

/// A ring counts as a hole when its winding is clockwise after nesting has
/// normalised the region.
fn is_hole(ring: &[V2]) -> bool {
    ring_signed_area(ring) < 0.0
}

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
    let reach = 2.0 * radius;
    let mut rings = region.rings.clone();
    for _ in 0..PASSES {
        let probe = Region {
            rings: rings.clone(),
        };
        let grid = SegGrid::new(&probe, reach);
        let moves = pull(&grid, &probe, reach);
        let mut moved = false;
        for (index, ring) in probe.rings.iter().enumerate() {
            for (k, target) in moves[index].iter().enumerate() {
                if let Some(target) = target {
                    if target.dist(ring[k]) > 1e-6 {
                        rings[index][k] = *target;
                        moved = true;
                    }
                }
            }
        }
        if !moved {
            break;
        }
    }
    let floor = std::f64::consts::PI * radius * radius;
    let kept: Vec<Vec<V2>> = Region::new(rings)
        .rings
        .into_iter()
        .filter(|ring| {
            let area = ring_signed_area(ring).abs();
            if area < 1e-9 {
                return false;
            }
            !is_hole(ring) || area >= floor
        })
        .collect();
    Region::new(kept)
}

/// One pass of the pull: for every pair of boundary pieces nearer than `reach`
/// with air between them, where their endpoints should move to.
///
/// Pairs are segments, not vertices, and the air is judged between the two
/// segment midpoints. That is what makes a long slit close: judged at a corner,
/// a slit that ends in solid material reads as a wall, so the two walls would
/// converge at the mouth and stay apart at the bottom. Judged at mid-length the
/// air is unambiguous, and both walls move together along their whole length.
///
/// A vertex may be pulled by more than one pair, for instance where a crack
/// meets a straight edge, so the pulls are averaged and no single pair wins.
fn pull(grid: &SegGrid, region: &Region, reach: f64) -> Vec<Vec<Option<V2>>> {
    let mut sum: Vec<Vec<(V2, f64)>> = region
        .rings
        .iter()
        .map(|ring| vec![(V2::new(0.0, 0.0), 0.0); ring.len()])
        .collect();
    for segment in 0..grid.len() {
        let (owner, at) = grid.tag(segment);
        let count = region.rings[owner].len();
        let (a, b) = grid.seg(segment);
        let mid = (a + b) * 0.5;
        let mut candidates: Vec<(f64, u32)> = Vec::new();
        let sign = ring_signed_area(&region.rings[owner]).signum();
        for other in grid.around(mid) {
            let (theirs, their_at) = grid.tag(other);
            if theirs == owner && same_piece(region, owner, at, their_at, sign) {
                continue;
            }
            let (c, d) = grid.seg(other);
            let their_mid = (c + d) * 0.5;
            let gap = mid.dist(their_mid);
            if gap > 0.0 && gap < reach {
                candidates.push((gap, other));
            }
        }
        candidates.sort_by(|x, y| x.0.total_cmp(&y.0));
        for (_, other) in candidates {
            let (c, d) = grid.seg(other);
            if !faces_air(grid, region, mid, (c + d) * 0.5) {
                continue;
            }
            for vertex in [at, (at + 1) % count] {
                let target = if vertex == at { a } else { b };
                let (_, foot) = seg_closest(target, c, d);
                let slot = &mut sum[owner][vertex];
                slot.0 = slot.0 + (target + foot) * 0.5;
                slot.1 += 1.0;
            }
            break;
        }
    }
    sum.into_iter()
        .map(|ring| {
            ring.into_iter()
                .map(|(total, count)| (count > 0.0).then(|| total * (1.0 / count)))
                .collect()
        })
        .collect()
}

/// True when two segments are the same piece of boundary and so cannot bound a
/// gap.
///
/// The segment itself and its neighbours on a convex corner are the same piece:
/// there is material behind the shared vertex, so nothing can be closed there.
/// A reflex corner is different — that is a notch, and its two sides are exactly
/// what a closing has to weld, so the pair must stay eligible.
fn same_piece(region: &Region, owner: usize, at: usize, their_at: usize, sign: f64) -> bool {
    if their_at == at {
        return true;
    }
    let ring = &region.rings[owner];
    let n = ring.len();
    let (a, b) = (ring[at], ring[(at + 1) % n]);
    let follows = (at + 1) % n == their_at;
    let precedes = (their_at + 1) % n == at;
    if !follows && !precedes {
        return false;
    }
    let (c, d) = (ring[their_at], ring[(their_at + 1) % n]);
    let turn = if follows {
        (b.x - a.x) * (d.y - c.y) - (b.y - a.y) * (d.x - c.x)
    } else {
        (d.x - c.x) * (b.y - a.y) - (d.y - c.y) * (b.x - a.x)
    };
    turn * sign > 0.0
}

/// True when the strip between two boundary midpoints holds air, not solid.
///
/// A crack and a thin wall put two contour pieces equally close, and only the
/// first may be filled: closing adds material, it never removes it. Three
/// samples down the middle settle it — the midpoint and a quarter of the gap
/// either side — counting only those clear of the boundary, with the air in the
/// majority. A wall keeps its samples inside the region because they run along
/// its length; a crack keeps them in the air. A tie keeps the material.
fn faces_air(grid: &SegGrid, region: &Region, here: V2, there: V2) -> bool {
    let gap = here.dist(there);
    if gap <= 0.0 {
        return false;
    }
    let mid = (here + there) * 0.5;
    let step = V2::new((there.y - here.y) / gap, (here.x - there.x) / gap) * (0.25 * gap);
    let (mut air, mut solid) = (0, 0);
    for sample in [mid, mid + step, mid - step] {
        if grid.clearance(sample) < 1e-6 {
            continue;
        }
        if region.contains(sample) {
            solid += 1;
        } else {
            air += 1;
        }
    }
    air > solid
}

/// Simplifies every ring to within `tolerance` millimetres.
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

/// Uniform grid of segment ids keyed by the cells a segment's bounding box
/// touches. Rebuilt on every pass because every pass moves the boundary.
struct SegGrid {
    origin: V2,
    cs: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
    segs: Vec<(V2, V2)>,
    tags: Vec<(usize, usize)>,
}

impl SegGrid {
    fn new(rings: &Region, cs: f64) -> SegGrid {
        let mut segs = Vec::new();
        let mut tags = Vec::new();
        let mut bounds = Bounds::empty();
        for (index, ring) in rings.rings.iter().enumerate() {
            let n = ring.len();
            for k in 0..n {
                let (a, b) = (ring[k], ring[(k + 1) % n]);
                bounds.add(a);
                bounds.add(b);
                segs.push((a, b));
                tags.push((index, k));
            }
        }
        let cs = cs.max(1e-6);
        let nx = ((bounds.width() / cs).ceil() as usize).max(1) + 1;
        let ny = ((bounds.height() / cs).ceil() as usize).max(1) + 1;
        let origin = V2::new(bounds.min.x, bounds.min.y);
        let mut cells = vec![Vec::new(); nx * ny];
        for (i, (a, b)) in segs.iter().enumerate() {
            let (x0, x1) = cell_range(a.x.min(b.x), a.x.max(b.x), origin.x, cs, nx);
            let (y0, y1) = cell_range(a.y.min(b.y), a.y.max(b.y), origin.y, cs, ny);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    cells[y * nx + x].push(i as u32);
                }
            }
        }
        SegGrid {
            origin,
            cs,
            nx,
            ny,
            cells,
            segs,
            tags,
        }
    }

    fn around(&self, p: V2) -> Vec<u32> {
        let cx = (((p.x - self.origin.x) / self.cs).floor().max(0.0) as usize).min(self.nx - 1);
        let cy = (((p.y - self.origin.y) / self.cs).floor().max(0.0) as usize).min(self.ny - 1);
        let mut out = Vec::new();
        for y in cy.saturating_sub(1)..=(cy + 1).min(self.ny - 1) {
            for x in cx.saturating_sub(1)..=(cx + 1).min(self.nx - 1) {
                out.extend_from_slice(&self.cells[y * self.nx + x]);
            }
        }
        out
    }

    fn seg(&self, i: u32) -> (V2, V2) {
        self.segs[i as usize]
    }

    fn len(&self) -> u32 {
        self.segs.len() as u32
    }

    /// Distance from `p` to the nearest boundary segment within one cell.
    fn clearance(&self, p: V2) -> f64 {
        self.around(p)
            .into_iter()
            .map(|i| {
                let (a, b) = self.seg(i);
                seg_closest(p, a, b).0
            })
            .fold(f64::INFINITY, f64::min)
    }

    fn tag(&self, i: u32) -> (usize, usize) {
        self.tags[i as usize]
    }
}

fn cell_range(low: f64, high: f64, origin: f64, cs: f64, n: usize) -> (usize, usize) {
    let at = |v: f64| (((v - origin) / cs).floor().max(0.0) as usize).min(n - 1);
    (at(low), at(high))
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

    #[test]
    fn a_crack_narrower_than_the_radius_is_filled() {
        let region = twin_plates(0.05);
        assert!(
            !region.contains(V2::new(10.025, 5.0)),
            "the fixture starts cracked"
        );
        let closed = close_gaps(&region, 0.049);
        assert!(
            closed.contains(V2::new(10.025, 5.0)),
            "a 0.05 mm crack is narrower than the 0.098 mm reach and should be filled"
        );
        assert!(
            (closed.area() - 200.5).abs() < 0.05,
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
        assert_eq!(
            closed.rings[0], region.rings[0],
            "an untouched ring stays bit-identical"
        );
        assert_eq!(
            closed.rings[1], region.rings[1],
            "an untouched ring stays bit-identical"
        );
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
            (closed.area() - (400.0 - 0.05 * 15.0 + 0.05 * 15.0)).abs() < 1.0,
            "welding a slit fills it without moving the outer boundary, got {}",
            closed.area()
        );
    }

    #[test]
    fn a_layer_without_a_crack_comes_back_identical() {
        let region = Region::new(vec![square(0.0, 0.0, 20.0), square(5.0, 5.0, 5.0)]);
        let closed = close_gaps(&region, 0.049);
        assert_eq!(closed.rings, region.rings);
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
