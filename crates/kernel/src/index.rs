//! Uniform-grid nearest-neighbour indices over segments and points.
//!
//! Both indices are used heavily by the wall generator, so they share one
//! ring-expanding search: start at the query cell, widen outward, stop once
//! the best distance cannot be improved by the next ring.

use crate::geom::{seg_closest, Bounds, V2};

/// One nearest-segment query result.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    /// Distance from the query point to the segment.
    pub d: f64,
    /// Segment index within the original slice.
    pub seg: u32,
    /// Closest point on that segment.
    pub foot: V2,
}

/// Uniform-grid index over a fixed set of segments.
pub struct SegIndex {
    segs: Vec<(V2, V2)>,
    origin: V2,
    cs: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

fn grid_dims(bounds: Bounds, cs: f64) -> (usize, usize) {
    let nx = ((bounds.width() / cs).ceil() as usize).max(1) + 1;
    let ny = ((bounds.height() / cs).ceil() as usize).max(1) + 1;
    (nx, ny)
}

fn clamp_cell(v: f64, n: usize) -> usize {
    (v.floor().max(0.0) as usize).min(n - 1)
}

impl SegIndex {
    /// Builds an index over `segs` using cells of at least `cs` millimetres.
    pub fn new(segs: Vec<(V2, V2)>, bounds: Bounds, cs: f64) -> SegIndex {
        let (nx, ny) = grid_dims(bounds, cs);
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); nx * ny];
        let origin = bounds.min;
        for (si, (a, b)) in segs.iter().enumerate() {
            let x0 = clamp_cell((a.x.min(b.x) - origin.x) / cs, nx);
            let x1 = clamp_cell((a.x.max(b.x) - origin.x) / cs, nx);
            let y0 = clamp_cell((a.y.min(b.y) - origin.y) / cs, ny);
            let y1 = clamp_cell((a.y.max(b.y) - origin.y) / cs, ny);
            for cy in y0..=y1 {
                for cx in x0..=x1 {
                    cells[cy * nx + cx].push(si as u32);
                }
            }
        }
        SegIndex {
            segs,
            origin,
            cs,
            nx,
            ny,
            cells,
        }
    }

    /// Endpoints of segment `i`.
    pub fn seg(&self, i: u32) -> (V2, V2) {
        self.segs[i as usize]
    }

    /// Number of indexed segments.
    pub fn len(&self) -> usize {
        self.segs.len()
    }

    /// True when no segment was indexed.
    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    fn cell_of(&self, p: V2) -> (i64, i64) {
        let cx = ((p.x - self.origin.x) / self.cs).floor() as i64;
        let cy = ((p.y - self.origin.y) / self.cs).floor() as i64;
        (
            cx.clamp(0, self.nx as i64 - 1),
            cy.clamp(0, self.ny as i64 - 1),
        )
    }

    fn visit(&self, cx: i64, cy: i64, p: V2, best: &mut Hit) {
        if cx < 0 || cy < 0 || cx >= self.nx as i64 || cy >= self.ny as i64 {
            return;
        }
        if best.d.is_finite() {
            let x0 = self.origin.x + cx as f64 * self.cs;
            let y0 = self.origin.y + cy as f64 * self.cs;
            let dx = (x0 - p.x).max(0.0).max(p.x - (x0 + self.cs));
            let dy = (y0 - p.y).max(0.0).max(p.y - (y0 + self.cs));
            if (dx * dx + dy * dy).sqrt() > best.d {
                return;
            }
        }
        for &s in &self.cells[cy as usize * self.nx + cx as usize] {
            let (a, b) = self.segs[s as usize];
            let (d, f) = seg_closest(p, a, b);
            if d < best.d || (d == best.d && s < best.seg) {
                *best = Hit { d, seg: s, foot: f };
            }
        }
    }

    /// Nearest segment within `max_d`, or `None` when nothing is that close.
    pub fn nearest(&self, p: V2, max_d: f64) -> Option<Hit> {
        self.nearest_from(p, max_d, None)
    }

    /// The same query, started from a distance already known to hold.
    ///
    /// Adjacent cells of a distance field almost always share the segment that
    /// is nearest to them, so a bound carried over from the last cell lets the
    /// ring search discard cells that could not improve on it. The answer is
    /// unchanged: a search only stops early once no unscanned cell can be
    /// closer than the best it has.
    pub fn nearest_from(&self, p: V2, max_d: f64, seed: Option<Hit>) -> Option<Hit> {
        if self.segs.is_empty() {
            return None;
        }
        let (cx, cy) = self.cell_of(p);
        let kmax = (max_d / self.cs).ceil() as i64 + 1;
        let mut best = match seed {
            Some(hit) if hit.seg != u32::MAX => {
                let (a, b) = self.segs[hit.seg as usize];
                let (d, foot) = seg_closest(p, a, b);
                Hit {
                    d,
                    seg: hit.seg,
                    foot,
                }
            }
            _ => Hit {
                d: f64::INFINITY,
                seg: u32::MAX,
                foot: V2::ZERO,
            },
        };
        let mut k = 0i64;
        loop {
            if k == 0 {
                self.visit(cx, cy, p, &mut best);
            } else {
                for x in (cx - k)..=(cx + k) {
                    self.visit(x, cy - k, p, &mut best);
                    self.visit(x, cy + k, p, &mut best);
                }
                for y in (cy - k + 1)..=(cy + k - 1) {
                    self.visit(cx - k, y, p, &mut best);
                    self.visit(cx + k, y, p, &mut best);
                }
            }
            if best.d <= k as f64 * self.cs || k >= kmax {
                break;
            }
            k += 1;
        }
        if best.d <= max_d {
            Some(best)
        } else {
            None
        }
    }
}

/// Uniform-grid index over a fixed set of points.
pub struct PointIndex {
    pts: Vec<V2>,
    origin: V2,
    cs: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl PointIndex {
    /// Builds an index over `pts` using cells of at least `cs` millimetres.
    pub fn new(pts: Vec<V2>, bounds: Bounds, cs: f64) -> PointIndex {
        let (nx, ny) = grid_dims(bounds, cs);
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); nx * ny];
        let origin = bounds.min;
        for (i, p) in pts.iter().enumerate() {
            let cx = clamp_cell((p.x - origin.x) / cs, nx);
            let cy = clamp_cell((p.y - origin.y) / cs, ny);
            cells[cy * nx + cx].push(i as u32);
        }
        PointIndex {
            pts,
            origin,
            cs,
            nx,
            ny,
            cells,
        }
    }

    /// True when no point was indexed.
    pub fn is_empty(&self) -> bool {
        self.pts.is_empty()
    }

    /// Number of indexed points.
    pub fn len(&self) -> usize {
        self.pts.len()
    }

    /// Nearest indexed point within `max_d`, with its distance.
    pub fn nearest(&self, p: V2, max_d: f64) -> Option<(u32, f64)> {
        if self.pts.is_empty() {
            return None;
        }
        let cx = (((p.x - self.origin.x) / self.cs).floor() as i64).clamp(0, self.nx as i64 - 1);
        let cy = (((p.y - self.origin.y) / self.cs).floor() as i64).clamp(0, self.ny as i64 - 1);
        let kmax = (max_d / self.cs).ceil() as i64 + 1;
        let mut best: Option<(u32, f64)> = None;
        let mut k = 0i64;
        loop {
            let mut scan = |x: i64, y: i64| {
                if x < 0 || y < 0 || x >= self.nx as i64 || y >= self.ny as i64 {
                    return;
                }
                for &i in &self.cells[y as usize * self.nx + x as usize] {
                    let d = self.pts[i as usize].dist(p);
                    if best.is_none_or(|(bi, bd)| d < bd || (d == bd && i < bi)) {
                        best = Some((i, d));
                    }
                }
            };
            if k == 0 {
                scan(cx, cy);
            } else {
                for x in (cx - k)..=(cx + k) {
                    scan(x, cy - k);
                    scan(x, cy + k);
                }
                for y in (cy - k + 1)..=(cy + k - 1) {
                    scan(cx - k, y);
                    scan(cx + k, y);
                }
            }
            if best.is_some_and(|(_, d)| d <= k as f64 * self.cs) || k >= kmax {
                break;
            }
            k += 1;
        }
        best.filter(|&(_, d)| d <= max_d)
    }
}

/// Banded crossing index over a region's rings, for repeated inside tests.
///
/// Deciding whether a point is inside costs one pass over the rings, and a stage
/// that asks the question for every sample along every edge cannot afford that:
/// the medial axis of a 20,000-vertex layer has hundreds of thousands of samples.
/// The segments are therefore bucketed by the horizontal band they cross, and a
/// query only looks at the band its own `y` falls in.
///
/// The answer is the even-odd rule over every ring, which is the same rule
/// [`super::Region::contains`] applies. A point exactly on the boundary is not
/// guaranteed either answer; callers that care about boundary points test the
/// distance to the rings as well, which is what a wall stage does anyway.
pub struct RegionIndex {
    bands: Vec<Vec<(V2, V2)>>,
    y0: f64,
    h: f64,
}

impl RegionIndex {
    /// Builds an index over a set of closed rings.
    ///
    /// The band height follows the extent, so a shape with a thousand vertices and
    /// one with a hundred thousand both end up with a small number of candidates
    /// per query.
    pub fn new(rings: &[Vec<V2>]) -> RegionIndex {
        let mut bounds = Bounds::empty();
        for ring in rings {
            for p in ring {
                bounds.add(*p);
            }
        }
        if bounds.is_empty() {
            return RegionIndex {
                bands: Vec::new(),
                y0: 0.0,
                h: 1.0,
            };
        }
        let h = (bounds.height() / 256.0).max(0.25);
        let n = ((bounds.height() / h).ceil() as usize).max(1) + 1;
        let mut bands: Vec<Vec<(V2, V2)>> = vec![Vec::new(); n];
        for ring in rings {
            let k = ring.len();
            for i in 0..k {
                let a = ring[i];
                let b = ring[(i + 1) % k];
                if a == b {
                    continue;
                }
                let first = band_of(a.y.min(b.y), bounds.min.y, h, n);
                let last = band_of(a.y.max(b.y), bounds.min.y, h, n);
                for band in &mut bands[first..=last] {
                    band.push((a, b));
                }
            }
        }
        RegionIndex {
            bands,
            y0: bounds.min.y,
            h,
        }
    }

    /// True when the point is inside an odd number of rings.
    pub fn contains(&self, p: V2) -> bool {
        if self.bands.is_empty() {
            return false;
        }
        let band = band_of(p.y, self.y0, self.h, self.bands.len());
        let mut crossings = 0usize;
        for &(a, b) in &self.bands[band] {
            if (a.y > p.y) == (b.y > p.y) {
                continue;
            }
            let t = (p.y - a.y) / (b.y - a.y);
            if a.x + (b.x - a.x) * t > p.x {
                crossings += 1;
            }
        }
        crossings % 2 == 1
    }
}

fn band_of(y: f64, y0: f64, h: f64, n: usize) -> usize {
    let raw = ((y - y0) / h).floor();
    if raw.is_nan() || raw < 0.0 {
        return 0;
    }
    (raw as usize).min(n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<(V2, V2)> {
        vec![
            (V2::new(0.0, 0.0), V2::new(10.0, 0.0)),
            (V2::new(10.0, 0.0), V2::new(10.0, 10.0)),
            (V2::new(10.0, 10.0), V2::new(0.0, 10.0)),
            (V2::new(0.0, 10.0), V2::new(0.0, 0.0)),
        ]
    }

    #[test]
    fn segment_index_finds_the_nearest_edge() {
        let segs = square();
        let index = SegIndex::new(segs.clone(), Bounds::empty().grown(12.0), 1.0);
        let hit = index.nearest(V2::new(5.0, 1.5), 10.0).expect("hit");
        assert!((hit.d - 1.5).abs() < 1e-9);
        assert!((hit.foot.y - 0.0).abs() < 1e-9);
        assert_eq!(index.len(), 4);
    }

    #[test]
    fn segment_index_rejects_queries_beyond_the_cap() {
        let index = SegIndex::new(square(), Bounds::empty().grown(12.0), 1.0);
        assert!(index.nearest(V2::new(5.0, 5.0), 2.0).is_none());
    }

    #[test]
    fn point_index_finds_the_nearest_vertex() {
        let pts = vec![V2::new(0.0, 0.0), V2::new(10.0, 0.0), V2::new(0.0, 10.0)];
        let index = PointIndex::new(pts, Bounds::empty().grown(12.0), 1.0);
        let (id, d) = index.nearest(V2::new(9.0, 1.0), 5.0).expect("hit");
        assert_eq!(id, 1);
        assert!((d - std::f64::consts::SQRT_2).abs() < 1e-9);
        assert!(index.nearest(V2::new(5.0, 5.0), 1.0).is_none());
    }

    #[test]
    fn region_index_agrees_with_the_region_on_every_grid_point() {
        let rings = vec![
            vec![
                V2::new(0.0, 0.0),
                V2::new(40.0, 0.0),
                V2::new(40.0, 30.0),
                V2::new(0.0, 30.0),
            ],
            vec![
                V2::new(10.0, 10.0),
                V2::new(10.0, 20.0),
                V2::new(30.0, 20.0),
                V2::new(30.0, 10.0),
            ],
        ];
        let region = super::super::Region::new(rings.clone());
        let index = RegionIndex::new(&region.rings);
        for i in 0..=200 {
            for j in 0..=150 {
                let p = V2::new(i as f64 * 0.2, j as f64 * 0.2);
                assert_eq!(
                    index.contains(p),
                    region.contains(p),
                    "disagreement at {p:?}"
                );
            }
        }
    }

    #[test]
    fn region_index_reports_nothing_for_empty_rings() {
        assert!(!RegionIndex::new(&[]).contains(V2::ZERO));
    }

    #[test]
    fn empty_indices_return_none_instead_of_panicking() {
        let empty = Bounds::empty().grown(1.0);
        assert!(SegIndex::new(Vec::new(), empty, 1.0)
            .nearest(V2::ZERO, 5.0)
            .is_none());
        assert!(PointIndex::new(Vec::new(), empty, 1.0)
            .nearest(V2::ZERO, 5.0)
            .is_none());
    }
}
