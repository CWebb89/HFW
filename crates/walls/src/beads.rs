//! Wall beads as level sets of the distance to the boundary.
//!
//! A wall runs parallel to the surface, so its path is a level set of the distance
//! to the surface — one curve per bead, at the depth that bead's centre sits. That
//! is what the offset ladder draws, and it is also what the reference slicers draw:
//! sliced with Arachne and sliced with a plain ladder, a 20 mm disc's three walls come
//! out as three concentric rings of the same length to the millimetre. The medial axis
//! does not carry wall *paths*; it carries how wide the section is and therefore how
//! many beads it holds.
//!
//! So this module takes the layer's width from the section rather than from the
//! profile's nominal bead. A cell of the sampling grid measures the section it sits
//! in — the distance to the surface towards it, plus the distance onwards to the far
//! side — which is the same measurement [`crate::poly`] makes for its gap pass but
//! taken everywhere instead of only where a gap is suspected. The section decides how
//! many beads it holds and how wide they are: a 1.4 mm wall holds three beads of
//! 0.467, a 20 mm section holds walls at the nominal width and leaves its middle to
//! the infill. Each bead is then the zero level of `depth − centre`, and the tracer in
//! [`crate::contour`] turns that into a polyline carrying the width measured where the
//! bead crosses a cell boundary.
//!
//! This is what removes the sliver the ladder used to leave for the gap pass: the
//! room a feature has is divided among the beads it actually holds, instead of a
//! fixed-width loop leaving a strip behind.

use hfw_kernel::{difference_rings, offset_rings, Join, Region, RegionIndex, SegIndex, V2};

use crate::contour::trace;
use crate::field::{scan_inside, Grid};
use crate::params::Params;
use crate::poly::{gap_fill, printable, without_bead, ARC_TOL};
use crate::{Kind, Line, Output, Stats};
use std::collections::HashMap;

/// What one grid cell knows about the section it sits in.
#[derive(Clone, Copy, Debug)]
struct Cell {
    /// Distance to the surface, positive inside the layer and negative outside.
    depth: f64,
    /// How many beads the section holds, `0` where it was not measured.
    count: usize,
    /// Width of each of those beads, in millimetres.
    width: f64,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell {
            depth: 0.0,
            count: 0,
            width: 0.0,
        }
    }
}

/// Shortest bead this generator will lay, as a multiple of the nominal width.
///
/// A loop three beads around encloses about one bead's worth of area, so anything
/// shorter is a dot whose extrusion lands on material already there — and on a real
/// model those are everywhere: every soft crease and every merged fillet leaves a spot
/// where the section briefly deepens past a bead centre, and the level set closes a
/// three-vertex loop around it. The references drop them for the same reason, and a
/// nib narrower than that cannot be printed as a loop anyway.
const MIN_BEAD: f64 = 3.0;

/// Generates the walls of one layer as level sets of the distance to its boundary.
pub(crate) fn generate(region: &Region, p: &Params) -> Output {
    let t_all = std::time::Instant::now();
    let mut stats = Stats::default();
    let mut lines: Vec<Line> = Vec::new();
    let viable = printable(region, p);
    if viable.is_empty() || p.walls == 0 {
        return Output { lines, stats };
    }

    let reach = band(p) + p.w_max;
    let h = (0.5 * p.w).clamp(0.05, 0.3);
    let bounds = viable.bounds().grown(reach + 2.0 * h);
    let mut grid = Grid::covering(bounds, h);
    if grid.is_empty() {
        return Output { lines, stats };
    }
    grid.origin = grid.origin + V2::new(0.0371 * h, 0.0519 * h);

    let index = SegIndex::new(viable.segments(), bounds.grown(reach), 2.0 * reach);
    let region_index = RegionIndex::new(&viable.rings);
    let inside = scan_inside(&viable, &grid);
    let deep_rings = offset_rings(&viable.rings, -reach, ARC_TOL, Join::Miter);
    let deep = if deep_rings.is_empty() {
        Vec::new()
    } else {
        scan_inside(&Region::new(deep_rings), &grid)
    };

    let t = std::time::Instant::now();
    let mut cells: Vec<Cell> = vec![Cell::default(); grid.len()];
    for id in 0..grid.len() {
        if deep.get(id).copied().unwrap_or(false) {
            cells[id] = Cell {
                depth: 2.0 * reach,
                count: p.walls,
                width: p.w,
            };
            continue;
        }
        let far = reach + 2.0 * h;
        let near = index.nearest(grid.pos_id(id), far).map(|hit| hit.d);
        cells[id].depth = match (inside[id], near) {
            (true, Some(d)) => d,
            (false, Some(d)) => -d,
            (true, None) => 2.0 * reach,
            (false, None) => -far,
        };
    }
    stats.ms_offset += t.elapsed().as_secs_f64() * 1e3;

    let t = std::time::Instant::now();
    for id in 0..grid.len() {
        if cells[id].count > 0 || cells[id].depth <= 0.0 {
            continue;
        }
        let section = section_at(&grid, &cells, id, p);
        cells[id] = Cell {
            depth: cells[id].depth,
            count: section.count,
            width: section.width,
        };
    }
    stats.ms_offset += t.elapsed().as_secs_f64() * 1e3;

    let wind = viable
        .rings
        .first()
        .map(|r| hfw_kernel::ring_signed_area(r).signum())
        .unwrap_or(1.0);
    let mut rest: Vec<Vec<V2>> = viable.rings.clone();
    for k in 0..p.walls {
        let mut level: Vec<f64> = Vec::with_capacity(grid.len());
        let mut width: Vec<f64> = Vec::with_capacity(grid.len());
        let mut centre: Vec<f64> = Vec::with_capacity(grid.len());
        for cell in &cells {
            let holds = cell.count > k && cell.depth > 0.0;
            let nominal = cell.width == p.w;
            let bead = if nominal { p.loop_width(k) } else { cell.width };
            let at = if nominal {
                p.loop_offset(k)
            } else {
                (k as f64 + 0.5) * cell.width
            };
            level.push(cell.depth - at);
            width.push(if holds { bead } else { f64::NAN });
            centre.push(if holds { at } else { f64::NAN });
        }
        let t = std::time::Instant::now();
        let traced = trace(&grid, &level, &width, &centre);
        stats.ms_offset += t.elapsed().as_secs_f64() * 1e3;

        let mut rings: Vec<(Vec<V2>, Vec<f64>, bool)> = Vec::new();
        for raw in traced {
            if raw.pts.len() < 3 || !raw.w.iter().all(|w| w.is_finite() && *w > 0.0) {
                continue;
            }
            if length(&raw.pts, raw.closed) < MIN_BEAD * p.w {
                continue;
            }
            if !raw
                .pts
                .iter()
                .all(|q| on_layer(&region_index, &index, *q, h))
            {
                continue;
            }
            let mut pts = raw.pts;
            if raw.closed && hfw_kernel::ring_signed_area(&pts).signum() != wind {
                pts.reverse();
            }
            let (pts, ww, closed) = unfold(pts, raw.w, raw.closed, 0.5 * p.w);
            rings.push((pts, ww, closed));
        }
        let mut emitted: Vec<(Vec<V2>, Vec<f64>, bool)> = Vec::new();
        for (pts, w, closed) in &rings {
            let tol = p.simplify;
            let (pts, ww) = crate::simplify(pts, w, tol, 0.05 * p.w, *closed);
            if length(&pts, *closed) < MIN_BEAD * p.w {
                continue;
            }
            if *closed && pts.len() < 3 {
                continue;
            }
            if *closed {
                let width = median(&ww);
                let holds = hfw_kernel::ring_signed_area(&pts).abs() >= width * width;
                if !holds {
                    continue;
                }
            }
            emitted.push((pts, ww, *closed));
        }
        if emitted.is_empty() {
            continue;
        }
        if stats.loops <= k {
            stats.loops = k + 1;
        }
        stats.rings += emitted.len();
        for (pts, ww, closed) in emitted {
            rest = without_bead(&rest, std::slice::from_ref(&pts), 0.0, 0.5 * median(&ww));
            stats.points += pts.len();
            lines.push(Line {
                pts,
                width: ww,
                closed,
                kind: Kind::Side(k as u8),
                flow: 1.0,
                overhang: 0.0,
                object: 0,
            });
        }
    }

    if p.gap_fill || lines.is_empty() {
        let t = std::time::Instant::now();
        gap_fill(&rest, p, &mut lines, &mut stats);
        stats.ms_gap = t.elapsed().as_secs_f64() * 1e3;
    }
    stats.ms_total = t_all.elapsed().as_secs_f64() * 1e3;
    Output { lines, stats }
}

/// Collapses a path that doubles back on itself.
///
/// The innermost bead of a section that divides exactly sits on the medial axis, and
/// the level set through it is one grid cell wide: the tracer returns two branches a
/// cell apart, joined where the field turns. Printed, that is the bead laid twice —
/// the lap a wall shows, and the reason a wall looked worse than the ladder it
/// replaced. Where two points of one path come within half a bead of each other but
/// sit more than a bead apart along the path, the arc between them is a fold: the
/// longer arc keeps one pass where there were two. A section that tapers into a spur
/// folds the same way and is cut the same way.
fn unfold(
    mut pts: Vec<V2>,
    mut w: Vec<f64>,
    mut closed: bool,
    limit: f64,
) -> (Vec<V2>, Vec<f64>, bool) {
    for _ in 0..4 {
        let n = pts.len();
        if n < 5 || limit <= 0.0 {
            break;
        }
        let mut arc = vec![0.0f64; n + 1];
        for i in 1..=n {
            arc[i] = arc[i - 1] + pts[i - 1].dist(pts[i % n]);
        }
        let total = arc[n];
        if total <= 0.0 {
            break;
        }
        let cell = limit.max(1e-6);
        let mut seen: HashMap<(i64, i64), Vec<usize>> = HashMap::with_capacity(n);
        let mut best: Option<(f64, usize, usize)> = None;
        for i in 0..n {
            let p = pts[i];
            let kx = (p.x / cell).floor() as i64;
            let ky = (p.y / cell).floor() as i64;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(list) = seen.get(&(kx + dx, ky + dy)) else {
                        continue;
                    };
                    for &j in list {
                        if i - j < 3 {
                            continue;
                        }
                        let gap = arc[i] - arc[j];
                        if gap < 1.0 || total - gap < 1.0 {
                            continue;
                        }
                        if p.dist(pts[j]) >= limit {
                            continue;
                        }
                        let span = gap.min(total - gap);
                        if best.map(|(b, _, _)| span > b).unwrap_or(true) {
                            best = Some((span, j, i));
                        }
                    }
                }
            }
            seen.entry((kx, ky)).or_default().push(i);
        }
        let Some((_, j, i)) = best else {
            break;
        };
        let out = arc[i] - arc[j];
        let keep: Vec<usize> = if out <= total - out {
            (i..j + n).map(|k| k % n).collect()
        } else {
            (j..i).collect()
        };
        let mut np = Vec::with_capacity(keep.len());
        let mut nw = Vec::with_capacity(keep.len());
        for k in keep {
            np.push(pts[k]);
            nw.push(w[k]);
        }
        if np.len() < 2 {
            break;
        }
        pts = np;
        w = nw;
        closed = false;
    }
    (pts, w, closed)
}

/// How many beads the section at a cell holds, and how wide each of them is.
///
/// The section is the distance to the surface plus the distance onwards to the far
/// side, marched along the inward normal. Where the even division of the section by
/// the bead count is a printable width, that division is what is used, which is what
/// fills a feature narrower than the walls it asked for. Where the division would need
/// a bead wider than [`Params::w_max`], the section is wider than the walls being
/// filled with it: the beads are the widths the profile asks for and the middle belongs
/// to the infill.
///
/// The count is taken from those widths rather than from the nominal one, because a
/// wider outer bead leaves room for fewer beads behind it: 0.62 + 0.4 + 0.4 does not fit
/// in a 1.2 mm section, and the third bead's centre would land outside the material.
fn section_at(grid: &Grid, cells: &[Cell], id: usize, p: &Params) -> Cell {
    let depth = cells[id].depth;
    let normal = inward_normal(grid, cells, id);
    let limit = 2.0 * (band(p) + p.w_max);
    let Some(far) = march(grid, cells, grid.pos_id(id), normal, limit) else {
        return Cell {
            depth,
            count: p.walls,
            width: p.w,
        };
    };
    let total = depth + far;
    let mut count = 0;
    let mut room = 0.0;
    for k in 0..p.walls {
        let w = p.loop_width(k);
        if room + 0.5 * w > total {
            break;
        }
        room += w;
        count += 1;
    }
    let count = count.clamp(1, p.walls);
    let even = total / count as f64;
    let width = if even <= p.w_max {
        even.clamp(p.w_min, p.w_max)
    } else {
        p.w
    };
    Cell {
        depth,
        count,
        width,
    }
}

/// Width of the wall band the profile asks for, over every wall.
fn band(p: &Params) -> f64 {
    (0..p.walls).map(|k| p.loop_width(k)).sum()
}

/// Unit normal pointing into the layer at a cell.
fn inward_normal(grid: &Grid, cells: &[Cell], id: usize) -> V2 {
    let (ix, iy) = (id % grid.nx, id / grid.nx);
    let at = |x: usize, y: usize| cells[grid.id(x.min(grid.nx - 1), y.min(grid.ny - 1))].depth;
    let gx = at(ix + 1, iy) - at(ix.saturating_sub(1), iy);
    let gy = at(ix, iy + 1) - at(ix, iy.saturating_sub(1));
    let g = V2::new(gx, gy);
    if g.len() > 1e-12 {
        g.norm()
    } else {
        V2::ZERO
    }
}

/// Distance along `u` from `from` to where the field reaches zero.
///
/// The field is sampled bilinearly on the grid, so the crossing is a bisection of a
/// cheap function rather than a query against the boundary: the section is measured
/// along the same normal everywhere, which is what keeps the bead widths of one
/// section consistent with each other.
fn march(grid: &Grid, cells: &[Cell], from: V2, u: V2, limit: f64) -> Option<f64> {
    if u.len() < 0.5 {
        return None;
    }
    let step = 0.5 * grid.h;
    let mut lo = 0.0;
    let mut hi = None;
    let mut t = step;
    while t <= limit {
        if sample(grid, cells, from + u * t) <= 0.0 {
            hi = Some(t);
            break;
        }
        lo = t;
        t += step;
    }
    let mut hi = hi?;
    for _ in 0..10 {
        let m = 0.5 * (lo + hi);
        if sample(grid, cells, from + u * m) <= 0.0 {
            hi = m;
        } else {
            lo = m;
        }
    }
    Some(0.5 * (lo + hi))
}

/// The field's depth at a world point, bilinearly interpolated.
fn sample(grid: &Grid, cells: &[Cell], p: V2) -> f64 {
    let fx = (p.x - grid.origin.x) / grid.h;
    let fy = (p.y - grid.origin.y) / grid.h;
    if fx < 0.0 || fy < 0.0 {
        return -1.0;
    }
    let (ix, iy) = (fx as usize, fy as usize);
    if ix + 1 >= grid.nx || iy + 1 >= grid.ny {
        return -1.0;
    }
    let (tx, ty) = (fx - ix as f64, fy - iy as f64);
    let a = cells[grid.id(ix, iy)].depth;
    let b = cells[grid.id(ix + 1, iy)].depth;
    let c = cells[grid.id(ix, iy + 1)].depth;
    let d = cells[grid.id(ix + 1, iy + 1)].depth;
    (a + (b - a) * tx) * (1.0 - ty) + (c + (d - c) * tx) * ty
}

/// True when a point is inside the layer or within a cell of its surface.
fn on_layer(region: &RegionIndex, index: &SegIndex, q: V2, h: f64) -> bool {
    region.contains(q)
        || index
            .nearest(q, 1.5 * h)
            .is_some_and(|hit| hit.d <= 1.5 * h)
}

fn length(pts: &[V2], closed: bool) -> f64 {
    let mut s = 0.0;
    for i in 1..pts.len() {
        s += pts[i - 1].dist(pts[i]);
    }
    if closed && pts.len() > 2 {
        s += pts[pts.len() - 1].dist(pts[0]);
    }
    s
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Unused warning guard: `difference_rings` is used by the band bookkeeping above.
#[allow(dead_code)]
fn _keeps_import(rings: &[Vec<V2>], holes: &[Vec<V2>]) -> Vec<Vec<V2>> {
    difference_rings(rings, holes)
}
