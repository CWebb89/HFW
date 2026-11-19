//! Wall generation as polygon offsets.
//!
//! A wall is the boundary of a shape eroded by half a bead, so a layer's walls
//! are a ladder of offsets of the layer itself: loop `i` runs at
//! [`Params::loop_offset`] from the surface, and where two features are closer
//! than twice that distance the offset has already merged them, which is what a
//! printed bead does too. Nothing here samples the layer on a grid, so the cost
//! follows the boundary rather than the area, and a straight edge produces one
//! segment instead of a staircase.
//!
//! What the ladder cannot do is finish a feature whose width is not a multiple of
//! the bead: the last loop leaves a strip behind, and in a feature narrower than
//! the ladder the loops stop early. That remainder is what the gap pass prints.
//! Each loop's bead is a band of exactly its own width around its path, so the
//! bands of every loop are the part of the layer already covered; the difference
//! is the uncovered remainder. Where that remainder is narrower than a bead it is
//! a gap, and one strand at its centre, as wide as the gap is across, fills it
//! exactly — measured by shooting a ray along the inward normal from each side and
//! finding where the other side becomes the nearer one, which is the same test
//! that says where a medial axis sits.

use std::collections::HashMap;

use hfw_kernel::{difference_rings, offset_rings, Join, Region, SegIndex, V2};

use crate::params::Params;
use crate::{simplify, Kind, Line, Output, Stats};

/// Largest chord error allowed at a rounded corner.
///
/// Wall loops ask for miter joins and have no arcs at all; this is only used by
/// the round joins of the coverage bands and by the morphological opening, where
/// a fiftieth of a millimetre is far below what a bead can express.
pub(crate) const ARC_TOL: f64 = 0.02;

/// Generates walls for one layer as a ladder of offsets.
pub(crate) fn generate(region: &Region, p: &Params) -> Output {
    let all = std::time::Instant::now();
    let (lines, _, mut stats) = ladder(region, p, false);
    stats.ms_total = all.elapsed().as_secs_f64() * 1e3;
    Output { lines, stats }
}

/// Generates the same ladder, then prints what it left uncovered.
pub(crate) fn generate_with_gaps(region: &Region, p: &Params) -> Output {
    let all = std::time::Instant::now();
    let (mut lines, uncovered, mut stats) = ladder(region, p, true);
    if p.gap_fill || lines.is_empty() {
        let t = std::time::Instant::now();
        gap_fill(&uncovered, p, &mut lines, &mut stats);
        stats.ms_gap = t.elapsed().as_secs_f64() * 1e3;
    }
    stats.ms_total = all.elapsed().as_secs_f64() * 1e3;
    Output { lines, stats }
}

/// The ladder itself: one offset ring per requested loop.
///
/// Also returns the remainder the beads leave behind, which is what the gap pass
/// prints, when the caller asked for it: tracking costs two more offsets per
/// loop, so the plain ladder does not pay for it.
fn ladder(region: &Region, p: &Params, track: bool) -> (Vec<Line>, Vec<Vec<V2>>, Stats) {
    let mut stats = Stats::default();
    let mut lines: Vec<Line> = Vec::new();
    if region.is_empty() {
        return (lines, Vec::new(), stats);
    }
    let viable = printable(region, p);
    if viable.is_empty() {
        return (lines, Vec::new(), stats);
    }
    let mut rest: Vec<Vec<V2>> = viable.rings.clone();
    for i in 0..p.walls {
        let offset = p.loop_offset(i);
        if offset <= 0.0 {
            continue;
        }
        let width = p.loop_width(i);
        let t = std::time::Instant::now();
        let path = offset_rings(&viable.rings, -offset, ARC_TOL, Join::Miter);
        stats.ms_offset += t.elapsed().as_secs_f64() * 1e3;
        if path.is_empty() {
            continue;
        }
        stats.loops += 1;
        stats.rings += path.len();
        let first = lines.len();
        push_loops(&path, width, Kind::Side(i as u8), p, &mut lines, &mut stats);
        if track {
            for line in &lines[first..] {
                rest = without_bead(&rest, std::slice::from_ref(&line.pts), offset, 0.5 * width);
            }
        }
    }
    if track {
        (lines, rest, stats)
    } else {
        (lines, Vec::new(), stats)
    }
}

/// The part of a layer wide enough to print at all.
///
/// Opening the region — erode by half the floor and grow it back — removes every
/// feature narrower than [`Params::min_feature`] and nothing else: the erosion
/// severs a thin neck and drops a thin rib, and the growth cannot bring back what
/// is no longer there. Both halves use a miter join so that a shape wide enough to
/// survive comes back with its corners still on, not rounded off.
pub(crate) fn printable(region: &Region, p: &Params) -> Region {
    let r = 0.5 * p.min_feature;
    if !(r.is_finite() && r > 0.0) || p.min_feature <= 0.5 * p.w {
        return region.clone();
    }
    let eroded = offset_rings(&region.rings, -r, ARC_TOL, Join::Miter);
    if eroded.is_empty() {
        return Region::default();
    }
    let opened = offset_rings(&eroded, r, ARC_TOL, Join::Miter);
    Region::new(opened)
}

/// What one loop's bead leaves uncovered, given the remainder before it.
///
/// The bead of a loop is the band one width wide centred on the loop's path, and
/// the path is the *boundary* of the eroded region — not the eroded region itself.
/// The band is therefore everything within `hw` of that boundary, which the kernel
/// expresses as the difference between growing and shrinking the eroded region: a
/// point deeper inside than `hw` survives the shrink and is not covered, and a
/// point the growth cannot reach is not either. Taking the eroded area itself as
/// the covered set instead would claim the middle of a hairpin the path wraps
/// around, which is exactly the strip that has to be printed.
///
/// The outermost loop is the one case that needs no growth: its bead reaches the
/// surface, so what it leaves behind is simply the region under its inner edge.
/// Asking for that edge by growing the region would land a nanometre from the
/// surface the caller already has, and the difference between two boundaries that
/// close would come back as thousands of slivers.
pub(crate) fn without_bead(
    rest: &[Vec<V2>],
    path: &[Vec<V2>],
    offset: f64,
    hw: f64,
) -> Vec<Vec<V2>> {
    if rest.is_empty() || !hw.is_finite() || hw <= 0.0 {
        return rest.to_vec();
    }
    let inner = offset_rings(path, -hw, ARC_TOL, Join::Miter);
    if offset <= hw + 1e-9 {
        return inner;
    }
    let outer = offset_rings(path, hw, ARC_TOL, Join::Miter);
    let band = difference_rings(&outer, &inner);
    difference_rings(rest, &band)
}

/// Turns closed offset rings into wall lines.
fn push_loops(
    rings: &[Vec<V2>],
    width: f64,
    kind: Kind,
    p: &Params,
    out: &mut Vec<Line>,
    stats: &mut Stats,
) {
    for ring in rings {
        if ring.len() < 3 {
            continue;
        }
        let widths = vec![width; ring.len()];
        let (pts, w) = simplify(ring, &widths, p.simplify, 0.05 * p.w, true);
        if pts.len() < 3 {
            continue;
        }
        stats.points += pts.len();
        out.push(Line {
            pts,
            width: w,
            closed: true,
            kind,
            flow: 1.0,
            overhang: 0.0,
            object: 0,
        });
    }
}

/// One point on a gap's boundary with the strand that would fill the gap from there.
struct Sample {
    centre: V2,
    width: f64,
    primary: bool,
}

/// Where each claimed strand sits, in arc length along the walk.
///
/// A strand is claimed by cell, and a cell claimed a long way back along the
/// boundary is a different strand: the two sides of one gap are far apart in arc
/// length however close they are in space, while two samples of the same strand
/// are one step apart. Storing the arc position is what tells those apart.
type Claims = HashMap<(i64, i64), f64>;

/// Shortest strand this generator will lay, as a multiple of its own width.
///
/// A strand shorter than a few of its own widths is a dab on material the loops
/// already cover, not a wall: a soft crease or a merged fillet leaves a spot where
/// the remainder briefly deepens past a bead, and the walk closes a stub of a run
/// around it. The references drop them the same way, and the same reasoning is why
/// [`crate::beads`] has its own floor in beads.
const MIN_STRAND: f64 = 3.0;

/// Fills the uncovered remainder of a layer where it is narrower than a bead.
pub(crate) fn gap_fill(rest: &[Vec<V2>], p: &Params, out: &mut Vec<Line>, stats: &mut Stats) {
    if rest.is_empty() {
        return;
    }
    let region = Region::new(rest.to_vec());
    if region.is_empty() {
        return;
    }
    let floor = 0.5 * p.w;
    let narrowest = p.min_feature.max(p.w_min).max(floor);
    let widest = p.w_max.max(p.w);
    let bounds = region.bounds().grown(widest + p.w);
    let index = SegIndex::new(region.segments(), bounds.grown(2.0 * widest), 2.0 * widest);
    let step = (0.25 * p.w).clamp(0.02, 0.2);
    let cell = 0.5 * widest;
    let mut claims = Claims::new();
    let mut arc = 0.0;
    let smallest_area = 0.25 * p.w * p.w;
    for ring in &region.rings {
        if hfw_kernel::ring_signed_area(ring).abs() < smallest_area {
            continue;
        }
        let samples = walk(
            ring,
            &index,
            narrowest,
            widest,
            step,
            cell,
            &mut arc,
            &mut claims,
        );
        for run in runs_of(&samples) {
            if run.len() < 2 {
                continue;
            }
            let pts: Vec<V2> = run.iter().map(|s| s.centre).collect();
            let widths: Vec<f64> = run.iter().map(|s| s.width).collect();
            if length_of(&pts) < 0.5 * p.w {
                continue;
            }
            let (sp, sw) = simplify(&pts, &widths, p.simplify, 0.05 * p.w, false);
            if sp.len() < 2 {
                continue;
            }
            let widest_of = sw.iter().fold(0.0f64, |a, b| a.max(*b));
            if length_of(&sp) < MIN_STRAND * widest_of {
                continue;
            }
            stats.gaps += 1;
            stats.points += sp.len();
            out.push(Line {
                pts: sp,
                width: sw,
                closed: false,
                kind: Kind::Middle,
                flow: 1.0,
                overhang: 0.0,
                object: 0,
            });
        }
    }
}

/// Walks one ring at a fixed spacing, sampling the gap across it.
#[allow(clippy::too_many_arguments)]
fn walk(
    ring: &[V2],
    index: &SegIndex,
    narrowest: f64,
    widest: f64,
    step: f64,
    cell: f64,
    arc: &mut f64,
    claims: &mut Claims,
) -> Vec<Sample> {
    let n = ring.len();
    let mut out = Vec::new();
    let mut base = *arc;
    for k in 0..n {
        let a = ring[k];
        let b = ring[(k + 1) % n];
        let edge = b - a;
        let len = edge.len();
        if len < 1e-9 {
            continue;
        }
        let inward = V2::new(-edge.y, edge.x) * (1.0 / len);
        let count = (len / step).ceil().max(1.0) as usize;
        for j in 0..count {
            let t = (j as f64 + 0.5) / count as f64;
            let at = a + edge * t;
            let here = base + t * len;
            let half = side_ray(index, at, inward, widest, step);
            let width = half.map(|h| 2.0 * h).unwrap_or(0.0);
            let mut primary = false;
            if width >= narrowest {
                let centre = at + inward * (0.5 * width);
                let key = cell_of(centre, cell);
                primary = match claims.get(&key) {
                    Some(&there) => (here - there).abs() <= 1.5 * step,
                    None => true,
                };
                if primary {
                    claims.insert(key, here);
                    out.push(Sample {
                        centre,
                        width: width.min(widest),
                        primary,
                    });
                    continue;
                }
            }
            out.push(Sample {
                centre: at,
                width: 0.0,
                primary,
            });
        }
        base += len;
    }
    *arc = base;
    out
}

/// Distance along `u` from a boundary point to where the far side becomes nearer.
///
/// At the start the nearest boundary point is the sample's own side; walking
/// inward, at some distance the opposite side is closer, and that crossing is half
/// the gap across, at the point nearest the middle of the gap.
///
/// A crossing alone is not a gap: at the corner of a wide region the edges either
/// side of it are within a bead of each other, and the ray finds the neighbour
/// long before it finds anything across. What tells a gap apart is the angle of
/// the wall it hit — a gap has two sides that face each other, so the far side has
/// to be roughly parallel to the near one. A corner's other edge is square to it.
fn side_ray(index: &SegIndex, from: V2, u: V2, widest: f64, h: f64) -> Option<f64> {
    let eps = 1e-9;
    let excess = |t: f64| -> f64 {
        match index.nearest(from + u * t, t + 1e-6) {
            Some(hit) => hit.d - t,
            None => -1.0,
        }
    };
    let limit = 0.5 * widest;
    let mut lo = 0.0;
    let mut step = h.min(limit);
    let hi;
    loop {
        let t = (lo + step).min(limit);
        if excess(t) >= -eps {
            if t >= limit {
                return None;
            }
            lo = t;
            step *= 2.0;
        } else {
            hi = t;
            break;
        }
    }
    let (mut a, mut b) = (lo, hi);
    for _ in 0..10 {
        let m = 0.5 * (a + b);
        if excess(m) >= -eps {
            a = m;
        } else {
            b = m;
        }
    }
    let half = 0.5 * (a + b);
    let hit = index.nearest(from + u * half, half + 1e-6)?;
    let (p, q) = index.seg(hit.seg);
    let along = (q - p).norm().dot(u).abs();
    if along <= 0.5 {
        Some(half)
    } else {
        None
    }
}

/// Splits one ring's samples into consecutive runs of primary ones.
fn runs_of(samples: &[Sample]) -> Vec<Vec<Sample>> {
    let n = samples.len();
    let mut runs = Vec::new();
    if n == 0 {
        return runs;
    }
    if samples.iter().all(|s| s.primary) {
        if n >= 2 {
            runs.push(
                samples
                    .iter()
                    .map(|s| Sample {
                        centre: s.centre,
                        width: s.width,
                        primary: true,
                    })
                    .collect(),
            );
        }
        return runs;
    }
    let start = samples.iter().position(|s| !s.primary).unwrap_or(0);
    let mut run: Vec<Sample> = Vec::new();
    for k in 0..n {
        let sample = &samples[(start + k) % n];
        if sample.primary {
            run.push(Sample {
                centre: sample.centre,
                width: sample.width,
                primary: true,
            });
        } else if !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

fn length_of(pts: &[V2]) -> f64 {
    let mut s = 0.0;
    for i in 1..pts.len() {
        s += pts[i - 1].dist(pts[i]);
    }
    s
}

fn cell_of(p: V2, size: f64) -> (i64, i64) {
    ((p.x / size).floor() as i64, (p.y / size).floor() as i64)
}
