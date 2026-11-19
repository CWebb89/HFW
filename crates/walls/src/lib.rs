//! Wall generation by polygon offset, with a gap pass over what the offsets leave.
//!
//! A layer's walls are the boundary of the layer eroded by half a bead, so they are
//! a ladder of polygon offsets of the layer itself. Where the ladder runs out — a
//! feature narrower than the loops, or a width that is not a whole number of beads —
//! the uncovered remainder is measured across and printed as one strand of that
//! width. Nothing is sampled on a grid, so the work follows the boundary and not the
//! area. `poly` holds both passes; the rest of this module is the path type, the
//! adhesion brim and the mesh utilities shared with the other stages.
//!
//! `beads` is the alternative that does sample: a section that divides into beads of
//! its own measured width, traced from a rasterised distance field. It is reachable
//! as [`Mode::Field`] and is not the default, because its cost follows the area under
//! the layer rather than the length of its boundary.

#![forbid(unsafe_code)]

mod beads;
pub mod contour;
pub mod field;
pub mod offset;
pub mod params;
mod poly;
pub mod repair;

use hfw_kernel::{Region, SegIndex, V2};
use params::{Mode, Params};

pub use field::Grid;
pub use offset::offset_region;
pub use params::{Mode as WallMode, Params as WallParams};
pub use repair::{close_gaps, simplify_region};

/// Semantic class of a generated path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Concentric side loop, counting outwards from the boundary.
    Side(u8),
    /// Bead straddling a medial-axis centreline.
    Middle,
    /// Sparse infill line.
    Infill,
    /// Solid skin line.
    Solid,
    /// Solid skin line on the visible surface, where the layer above does not
    /// cover it. Printed at the top-surface speed and acceleration when those
    /// are set, because that is the face the print is judged on.
    TopSolid,
    /// Adhesion brim loop, outside the part on the first layer.
    Brim,
    /// Skin line printed over air, anchored at both ends.
    Bridge,
    /// Slow, near-zero-flow pass that flattens an already printed top surface.
    Ironing,
    /// Crossing strand laid down before the bridge fill, so the fill spans
    /// material instead of bare air.
    BridgeAnchor,
    /// Sacrificial column under an overhang.
    Support,
    /// Dense top layers of a support column, in contact with the part.
    SupportInterface,
}

impl Kind {
    /// True for paths that form the perimeter shell.
    pub fn is_wall(self) -> bool {
        matches!(self, Kind::Side(_) | Kind::Middle)
    }
}

/// One polyline path with a per-vertex extrusion width.
#[derive(Clone, Debug)]
pub struct Line {
    /// Vertices in millimetres; closed lines do not repeat the first vertex.
    pub pts: Vec<V2>,
    /// Extrusion width at each vertex.
    pub width: Vec<f64>,
    /// True when the path is a closed loop.
    pub closed: bool,
    /// Semantic class of this path.
    pub kind: Kind,
    /// Extrusion multiplier for this path; 1.0 is normal flow.
    ///
    /// Combined infill is printed on one layer where two would have been, so its
    /// bead carries twice the plastic at twice the spacing. The volume has to
    /// live somewhere, and a wider bead would not fit a 0.4 nozzle, so it is a
    /// flow multiplier on an unchanged bead.
    pub flow: f64,
    /// Share of this path that hangs over air, `0..=1`.
    ///
    /// Filled in by the engine once the layer below is known: a generated path
    /// cannot tell whether the material under it exists. Zero means either "fully
    /// supported" or "nobody measured", which is why the writer only looks at it
    /// when a profile asks for overhang speeds or fans.
    pub overhang: f64,
    /// Object on the plate this path belongs to, counted from zero.
    ///
    /// The engine plans a plate as one body, so this is filled in by the caller
    /// that knows where each object sits. It only changes print order: paths are
    /// printed object by object within a layer.
    pub object: u16,
}

impl Line {
    /// Polyline length, including the closing segment.
    pub fn length(&self) -> f64 {
        let n = self.pts.len();
        let mut s = 0.0;
        for i in 1..n {
            s += self.pts[i - 1].dist(self.pts[i]);
        }
        if self.closed && n > 1 {
            s += self.pts[n - 1].dist(self.pts[0]);
        }
        s
    }

    /// Mean extrusion width along the path.
    pub fn mean_width(&self) -> f64 {
        if self.width.is_empty() {
            return 0.0;
        }
        self.width.iter().sum::<f64>() / self.width.len() as f64
    }
}

/// Counters and timings for one wall generation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Perimeter loops emitted.
    pub loops: usize,
    /// Closed rings those loops contain.
    pub rings: usize,
    /// Gap-fill strands emitted.
    pub gaps: usize,
    /// Path vertices emitted.
    pub points: usize,
    /// Milliseconds spent offsetting.
    pub ms_offset: f64,
    /// Milliseconds spent on the gap pass.
    pub ms_gap: f64,
    /// Total milliseconds.
    pub ms_total: f64,
}

/// Wall generation output.
pub struct Output {
    /// Generated paths.
    pub lines: Vec<Line>,
    /// Counters and timings.
    pub stats: Stats,
}

/// Generates walls for one slice region.
///
/// The whole layer is offset as one set, so parts that are close together erode
/// into each other exactly as printed beads do, and a plate pays for the loops it
/// emits rather than for the empty area between its parts.
pub fn generate(region: &Region, p: &Params, mode: Mode) -> Output {
    match mode {
        Mode::Hfw => poly::generate_with_gaps(region, p),
        Mode::Classic => poly::generate(region, p),
        Mode::Field => beads::generate(region, p),
    }
}

fn corner(a: V2, na: V2, b: V2, nb: V2, h: f64) -> Option<V2> {
    let det = na.cross(nb);
    if det.abs() < 1e-6 {
        return None;
    }
    let ca = na.dot(a);
    let cb = nb.dot(b);
    let m = V2::new((ca * nb.y - cb * na.y) / det, (na.x * cb - nb.x * ca) / det);
    let mid = a.lerp(b, 0.5);
    if m.dist(a) < 2.0 * h && m.dist(b) < 2.0 * h && m.dist(mid) > 0.1 * h {
        Some(m)
    } else {
        None
    }
}

fn point_seg(p: V2, a: V2, b: V2) -> (f64, f64) {
    let ab = b - a;
    let l2 = ab.len2();
    let t = if l2 > 0.0 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((p - (a + ab * t)).len(), t)
}

/// Simplifies a polyline while holding both position and width within tolerance.
pub fn simplify(pts: &[V2], w: &[f64], tol: f64, wtol: f64, closed: bool) -> (Vec<V2>, Vec<f64>) {
    let mut p: Vec<V2> = pts.to_vec();
    let mut ww: Vec<f64> = w.to_vec();
    if closed {
        p.push(pts[0]);
        ww.push(w[0]);
    }
    let n = p.len();
    if n < 3 {
        return (pts.to_vec(), w.to_vec());
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut worst = 0.0;
        let mut idx = a;
        for k in a + 1..b {
            let (dist, t) = point_seg(p[k], p[a], p[b]);
            let wl = ww[a] + (ww[b] - ww[a]) * t;
            let score = (dist / tol).max((ww[k] - wl).abs() / wtol);
            if score > worst {
                worst = score;
                idx = k;
            }
        }
        if worst > 1.0 {
            keep[idx] = true;
            stack.push((a, idx));
            stack.push((idx, b));
        }
    }
    let mut op = Vec::new();
    let mut ow = Vec::new();
    let last = if closed { n - 1 } else { n };
    for k in 0..last {
        if keep[k] {
            op.push(p[k]);
            ow.push(ww[k]);
        }
    }
    if closed && op.len() < 3 {
        return (pts.to_vec(), w.to_vec());
    }
    (op, ow)
}

fn finish(raw: contour::RawLine, index: &SegIndex, p: &Params, kind: Kind) -> Option<Line> {
    let n = raw.pts.len();
    if n < 2 {
        return None;
    }
    let h = p.h;
    let mut pts = raw.pts;
    let mut nrm = vec![V2::ZERO; n];
    for k in 0..n {
        let target = raw.lv[k];
        if !target.is_finite() {
            continue;
        }
        let mut x = pts[k];
        for _ in 0..4 {
            match index.nearest(x, target + 4.0 * h) {
                Some(hit) => {
                    let u = (x - hit.foot).norm();
                    let err = hit.d - target;
                    nrm[k] = u;
                    if err.abs() < 1e-8 {
                        break;
                    }
                    x = x - u * err.clamp(-0.75 * h, 0.75 * h);
                }
                None => break,
            }
        }
        pts[k] = x;
    }
    let wid: Vec<f64> = raw
        .w
        .iter()
        .map(|&v| if v.is_finite() { v } else { p.w })
        .collect();
    let cos_corner = p.corner_deg.to_radians().cos();
    let m = if raw.closed { n } else { n - 1 };
    let mut op: Vec<V2> = Vec::with_capacity(n + 8);
    let mut ow: Vec<f64> = Vec::with_capacity(n + 8);
    for k in 0..n {
        op.push(pts[k]);
        ow.push(wid[k]);
        if k < m {
            let b = (k + 1) % n;
            if nrm[k] != V2::ZERO && nrm[b] != V2::ZERO && nrm[k].dot(nrm[b]) < cos_corner {
                if let Some(c) = corner(pts[k], nrm[k], pts[b], nrm[b], h) {
                    op.push(c);
                    ow.push(0.5 * (wid[k] + wid[b]));
                }
            }
        }
    }
    let (sp, sw) = simplify(&op, &ow, p.simplify, 0.05 * p.w, raw.closed);
    let line = Line {
        pts: sp,
        width: sw,
        closed: raw.closed && n > 2,
        kind,
        overhang: 0.0,
        object: 0,
        flow: 1.0,
    };
    if line.pts.len() < 2 || line.length() < 3.0 * h {
        return None;
    }
    if line.closed && line.pts.len() < 3 {
        return None;
    }
    Some(line)
}

/// Which side of the part outline an adhesion brim grows on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrimSide {
    /// Outside the part only, the usual pad on the bed.
    Outer,
    /// Inside the part's holes only, so a hole gets a lip.
    Inner,
    /// Both.
    Both,
    /// No brim.
    None,
}

/// Solid adhesion pad around one layer's boundary.
///
/// The band from `gap` to `gap + loops * w` outside the part is filled with
/// raster lines one bead wide, spaced `w` apart, so the brim is a continuous
/// sheet rather than a set of rings with valleys between them. Line ends are
/// clipped to the band by the distance field, which makes the pad follow the
/// part outline — a round part gets a round brim. A single closed contour at
/// the outer level finishes the edge, because the raster staircase alone reads
/// as a polygon.
///
/// Rows are sampled every `max(1, round(w / h))` grid rows so the bead pitch
/// matches the extrusion width regardless of field resolution.
pub fn brim(
    region: &Region,
    p: &Params,
    loops: usize,
    gap: f64,
    separation: f64,
    side: BrimSide,
) -> Vec<Line> {
    if loops == 0 || region.is_empty() || side == BrimSide::None {
        return Vec::new();
    }
    let band = loops as f64 * p.w + (loops as f64 - 1.0) * separation.max(0.0);
    let reach = gap + band + p.w;
    let rb = region.bounds().grown(reach + 6.0 * p.h + p.w);
    let index = SegIndex::new(region.segments(), rb, 6.0 * p.h);
    let grid = Grid::covering(region.bounds().grown(reach + 2.0 * p.h), p.h);
    let inside = field::scan_inside(region, &grid);
    let n = grid.len();
    let (lo, hi) = (gap, gap + band);
    let mut dist = vec![0.0f64; n];
    for (id, flag) in inside.iter().enumerate() {
        if *flag {
            continue;
        }
        dist[id] = match index.nearest(grid.pos_id(id), hi + p.w) {
            Some(hit) => hit.d,
            None => hi + p.w,
        };
    }
    let enclosed = hole_areas(region, &grid, &inside);
    let wanted = |id: usize| match side {
        BrimSide::Outer => !enclosed[id],
        BrimSide::Inner => enclosed[id],
        BrimSide::Both => true,
        BrimSide::None => false,
    };
    let stride = ((p.w / grid.h).round() as usize).max(1);
    let mut out: Vec<Line> = Vec::new();
    for iy in (0..grid.ny).step_by(stride) {
        let row = &dist[iy * grid.nx..(iy + 1) * grid.nx];
        let mut ix = 0;
        while ix < grid.nx {
            let here = iy * grid.nx + ix;
            if row[ix] < lo || row[ix] > hi || !wanted(here) {
                ix += 1;
                continue;
            }
            let start = ix;
            while ix < grid.nx && row[ix] >= lo && row[ix] <= hi && wanted(iy * grid.nx + ix) {
                ix += 1;
            }
            let a = grid.pos(start, iy);
            let c = grid.pos(ix - 1, iy);
            if (c.x - a.x).abs() < p.w {
                continue;
            }
            out.push(Line {
                pts: vec![a, c],
                width: vec![p.w; 2],
                closed: false,
                kind: Kind::Brim,
                flow: 1.0,
                overhang: 0.0,
                object: 0,
            });
        }
    }
    let mut phi = vec![0.0f64; n];
    let bw = vec![p.w; n];
    let lv = vec![hi; n];
    for (id, flag) in inside.iter().enumerate() {
        phi[id] = if *flag { -1.0 } else { dist[id] - hi };
    }
    for raw in contour::trace(&grid, &phi, &bw, &lv) {
        let keep = raw.pts.first().is_some_and(|q| wanted(grid_id(&grid, *q)));
        if let Some(line) = finish(raw, &index, p, Kind::Brim) {
            if keep {
                out.push(line);
            }
        }
    }
    out
}

fn grid_id(grid: &Grid, p: V2) -> usize {
    let ix = (((p.x - grid.origin.x) / grid.h).floor() as isize).clamp(0, grid.nx as isize - 1);
    let iy = (((p.y - grid.origin.y) / grid.h).floor() as isize).clamp(0, grid.ny as isize - 1);
    iy as usize * grid.nx + ix as usize
}

/// Marks the empty cells enclosed by an outer ring, which is where an inner brim
/// belongs: the band grows into a hole instead of away from the part.
fn hole_areas(region: &Region, grid: &Grid, inside: &[bool]) -> Vec<bool> {
    let mut out = vec![false; inside.len()];
    let rings: Vec<&Vec<V2>> = region
        .rings
        .iter()
        .filter(|r| r.len() >= 3 && ring_area(r) > 0.0)
        .collect();
    if rings.is_empty() {
        return out;
    }
    for (id, solid) in inside.iter().enumerate() {
        if *solid {
            continue;
        }
        let q = grid.pos_id(id);
        out[id] = rings.iter().any(|r| contains(r, q));
    }
    out
}

fn ring_area(ring: &[V2]) -> f64 {
    let n = ring.len();
    let mut sum = 0.0;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        sum += a.x * b.y - b.x * a.y;
    }
    0.5 * sum
}

fn contains(ring: &[V2], q: V2) -> bool {
    let n = ring.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let a = ring[i];
        let b = ring[j];
        if (a.y > q.y) != (b.y > q.y) {
            let x = a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if q.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f64) -> Region {
        Region::new(vec![vec![
            V2::new(0.0, 0.0),
            V2::new(size, 0.0),
            V2::new(size, size),
            V2::new(0.0, size),
        ]])
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<V2> {
        vec![
            V2::new(x0, y0),
            V2::new(x1, y0),
            V2::new(x1, y1),
            V2::new(x0, y1),
        ]
    }

    fn sheet(length: f64, width: f64) -> Region {
        Region::new(vec![rect(0.0, 0.0, length, width)])
    }

    fn mean_width_of(lines: &[Line], loop_index: u8) -> Option<f64> {
        let widths: Vec<f64> = lines
            .iter()
            .filter(|line| line.kind == Kind::Side(loop_index))
            .map(|line| line.mean_width())
            .collect();
        if widths.is_empty() {
            None
        } else {
            Some(widths.iter().sum::<f64>() / widths.len() as f64)
        }
    }

    fn inset_of(lines: &[Line], loop_index: u8, size: f64) -> Option<f64> {
        let line = lines
            .iter()
            .find(|line| line.kind == Kind::Side(loop_index))?;
        let point = line.pts[0];
        Some(point.x.min(point.y).min(size - point.x).min(size - point.y))
    }

    fn gaps(lines: &[Line]) -> Vec<&Line> {
        lines.iter().filter(|l| l.kind == Kind::Middle).collect()
    }

    #[test]
    fn a_configured_outer_width_reaches_the_outer_loop_only() {
        let region = square(20.0);
        let plain = generate(&region, &Params::new(0.4, 3), Mode::Classic);
        assert!((mean_width_of(&plain.lines, 0).expect("outer") - 0.4).abs() < 0.05);
        assert!((mean_width_of(&plain.lines, 1).expect("inner") - 0.4).abs() < 0.05);

        let mut wide = Params::new(0.4, 3);
        wide.w_outer = 0.62;
        let output = generate(&region, &wide, Mode::Classic);
        assert!(
            (mean_width_of(&output.lines, 0).expect("outer") - 0.62).abs() < 0.05,
            "the outer loop should carry the configured width"
        );
        assert!(
            (mean_width_of(&output.lines, 1).expect("inner") - 0.4).abs() < 0.05,
            "the inner loops should keep the nominal width"
        );
    }

    #[test]
    fn a_wider_outer_loop_keeps_the_band_contiguous() {
        let region = square(20.0);
        let mut wide = Params::new(0.4, 3);
        wide.w_outer = 0.62;
        let output = generate(&region, &wide, Mode::Classic);

        let first = inset_of(&output.lines, 0, 20.0).expect("outer");
        let second = inset_of(&output.lines, 1, 20.0).expect("middle");
        let third = inset_of(&output.lines, 2, 20.0).expect("inner");
        assert!(
            (second - first - 0.51).abs() < 0.06,
            "spacing {first} -> {second} should be half the outer width plus half the inner one"
        );
        assert!(
            (third - second - 0.4).abs() < 0.06,
            "spacing {second} -> {third} should be the nominal bead width"
        );
        assert!(
            (first - 0.31).abs() < 0.05,
            "the outer loop should sit half its own width inside the surface, got {first}"
        );
    }

    #[test]
    fn a_straight_edge_becomes_four_vertices_and_not_a_staircase() {
        let region = square(20.0);
        let output = generate(&region, &Params::new(0.4, 3), Mode::Classic);
        for line in &output.lines {
            assert!(
                line.pts.len() <= 8,
                "a square loop needs its corners and nothing else, got {} vertices",
                line.pts.len()
            );
        }
    }

    #[test]
    fn a_field_mode_sheet_narrower_than_its_walls_is_filled_by_them() {
        let p = Params::new(0.4, 2);
        let region = sheet(30.0, 1.0);
        let output = generate(&region, &p, Mode::Field);
        let area: f64 = output.lines.iter().map(Line::length).sum();
        assert!(area > 0.0, "the sheet is printable");
        assert!(
            gaps(&output.lines).is_empty(),
            "a 1 mm sheet divides into two 0.5 mm beads, so nothing is left for a strand"
        );
        assert!(
            output
                .lines
                .iter()
                .all(|l| l.width.iter().all(|w| *w > 0.24 - 1e-9)),
            "neither bead is below the printable floor"
        );
    }

    #[test]
    fn turning_gap_fill_off_drops_the_strand_and_keeps_the_loop() {
        let p = Params::new(0.4, 2);
        let region = sheet(30.0, 2.0);
        let on = generate(&region, &p, Mode::Hfw);
        let off = generate(
            &region,
            &Params {
                gap_fill: false,
                ..p
            },
            Mode::Hfw,
        );
        assert!(!gaps(&on.lines).is_empty(), "2 mm holds a middle strip");
        assert!(gaps(&off.lines).is_empty(), "the switch did not take");
        assert_eq!(off.stats.loops, on.stats.loops, "the loops are untouched");
    }

    #[test]
    fn classic_mode_prints_no_gap_strand_at_all() {
        let p = Params::new(0.4, 2);
        let output = generate(&sheet(30.0, 1.0), &p, Mode::Classic);
        assert!(gaps(&output.lines).is_empty());
        assert!(!output.lines.is_empty(), "the loop is still there");
    }

    #[test]
    fn a_remainder_shorter_than_three_of_its_own_widths_is_not_a_strand() {
        let p = Params::new(0.4, 3);
        let mut stub = Vec::new();
        let mut stats = Stats::default();
        poly::gap_fill(&[rect(0.0, 0.0, 1.0, 0.5)], &p, &mut stub, &mut stats);
        assert!(
            stub.is_empty(),
            "a 1 x 0.5 mm patch is a dab on the layer, got {} strand(s)",
            stub.len()
        );

        let mut strip = Vec::new();
        poly::gap_fill(&[rect(0.0, 0.0, 30.0, 0.5)], &p, &mut strip, &mut stats);
        assert_eq!(strip.len(), 1, "a 30 mm strip is a strand worth laying");
        assert_eq!(strip[0].kind, Kind::Middle);
        let run = strip[0].length();
        let width = strip[0].mean_width();
        assert!(
            run >= 3.0 * width,
            "the strand is {run:.2} mm of {width:.2} mm width"
        );
    }

    #[test]
    fn a_feature_narrower_than_the_floor_prints_nothing() {
        let mut p = Params::new(0.4, 2);
        p.min_feature = 1.5;
        let output = generate(&sheet(30.0, 1.0), &p, Mode::Hfw);
        assert!(
            output.lines.is_empty(),
            "a 1 mm sheet is under a 1.5 mm floor, got {} paths",
            output.lines.len()
        );
        p.min_feature = 0.1;
        let kept = generate(&sheet(30.0, 1.0), &p, Mode::Hfw);
        assert!(!kept.lines.is_empty(), "and it comes back below the floor");
    }
    #[test]
    fn a_hole_is_walled_from_the_material_side() {
        let mut region = square(20.0);
        let mut hole = rect(8.0, 8.0, 12.0, 12.0);
        hole.reverse();
        region.rings.push(hole);
        let region = Region::new(region.rings);
        let output = generate(&region, &Params::new(0.4, 2), Mode::Classic);
        let outer = output
            .lines
            .iter()
            .filter(|l| l.kind == Kind::Side(0))
            .count();
        assert_eq!(outer, 2, "the outer wall and the hole wall, one each");
        let hole_ring = output
            .lines
            .iter()
            .filter(|l| l.kind == Kind::Side(0))
            .find(|l| {
                l.pts
                    .iter()
                    .all(|q| q.x > 1.0 && q.x < 19.0 && q.y > 1.0 && q.y < 19.0)
            })
            .expect("a loop inside the part");
        assert!(
            hole_ring
                .pts
                .iter()
                .all(|q| q.x < 7.9 || q.x > 12.1 || q.y < 7.9 || q.y > 12.1),
            "the hole loop stands off the hole by half a bead"
        );
    }

    #[test]
    fn two_parts_on_one_layer_each_get_their_loops() {
        let region = Region::new(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(20.0, 0.0, 30.0, 10.0),
        ]);
        let output = generate(&region, &Params::new(0.4, 2), Mode::Classic);
        let first = output
            .lines
            .iter()
            .filter(|l| l.kind == Kind::Side(0))
            .count();
        assert_eq!(first, 2, "one loop per part");
    }

    #[test]
    fn a_first_layer_scaled_band_is_wider_than_the_rest() {
        let region = square(20.0);
        let p = Params::new(0.4, 2);
        let wide = p.scaled_to(0.6);
        let base = generate(&region, &p, Mode::Classic);
        let first = generate(&region, &wide, Mode::Classic);
        let width = |o: &Output| mean_width_of(&o.lines, 1).expect("inner loop");
        assert!(
            width(&first) > width(&base) * 1.4,
            "the scaled band should be half again as wide: {} against {}",
            width(&base),
            width(&first)
        );
    }
}
