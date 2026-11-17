//! Material accounting: how much of the requested area the beads actually cover.
//!
//! Every metric here is measured on a jittered sampling grid, so that grid
//! lines never align with axis-aligned model edges. Cells inside the region but
//! farther than the wall band from any edge are excluded from the target when
//! `full` is false, which is what makes "under" mean "under inside the region
//! the walls are supposed to fill" rather than "under everywhere".
//!
//! The sampling window is the part grown by one extrusion width, so spill is
//! measured where it matters — just past the outline — and a path that strays
//! several millimetres outside the part is not sampled at all.

use hfw_kernel::{Region, SegIndex, V2};
use hfw_walls::field::{scan_inside, Grid};
use hfw_walls::{Kind, Line, WallParams};

/// Coverage, spill and width statistics for one layer.
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    /// Region area inside the sampling grid.
    pub area_region: f64,
    /// Area the paths are expected to cover.
    pub area_target: f64,
    /// Fraction of the target left uncovered.
    pub under: f64,
    /// Fraction of the target covered more than once.
    pub over: f64,
    /// Fraction of the target covered outside the part.
    pub spill: f64,
    /// Number of paths measured.
    pub lines: usize,
    /// Number of open paths, each needing a start and a stop.
    pub open_lines: usize,
    /// Extruder start/stop count.
    pub starts_stops: usize,
    /// Total path length.
    pub length: f64,
    /// Total deposited area.
    pub bead_area: f64,
    /// Fraction of length outside the printable width window.
    pub width_violation: f64,
    /// Total width variation per millimetre of path.
    pub width_tv_per_mm: f64,
    /// Narrowest bead width.
    pub min_width: f64,
    /// Widest bead width.
    pub max_width: f64,
}

struct Coverage {
    grid: Grid,
    inside: Vec<bool>,
    target: Vec<bool>,
    count: Vec<u16>,
}

fn coverage(region: &Region, lines: &[&Line], p: &WallParams, res: f64, full: bool) -> Coverage {
    let rb = region.bounds().grown(p.w);
    let mut grid = Grid::covering(rb, res);
    grid.origin = V2::new(
        grid.origin.x + std::f64::consts::FRAC_1_PI * res,
        grid.origin.y + std::f64::consts::FRAC_1_SQRT_2 * res,
    );
    let inside = scan_inside(region, &grid);
    let index = SegIndex::new(region.segments(), rb.grown(p.cap() + p.w), 8.0 * res);
    let n = grid.len();
    let thick_r = p.thick_radius();
    let mut target = vec![false; n];
    for id in 0..n {
        if inside[id] && (full || index.nearest(grid.pos_id(id), thick_r).is_some()) {
            target[id] = true;
        }
    }
    let mut count = vec![0u16; n];
    let mut stamp = vec![u32::MAX; n];
    for (li, line) in lines.iter().enumerate() {
        let m = line.pts.len();
        let segn = if line.closed { m } else { m.saturating_sub(1) };
        for k in 0..segn {
            let a = line.pts[k];
            let b = line.pts[(k + 1) % m];
            let wa = line.width[k];
            let wb = line.width[(k + 1) % m];
            let rmax = 0.5 * wa.max(wb);
            let x0 = (((a.x.min(b.x) - rmax - grid.origin.x) / res)
                .floor()
                .max(0.0)) as usize;
            let x1 =
                ((((a.x.max(b.x) + rmax - grid.origin.x) / res).ceil()) as usize).min(grid.nx - 1);
            let y0 = (((a.y.min(b.y) - rmax - grid.origin.y) / res)
                .floor()
                .max(0.0)) as usize;
            let y1 =
                ((((a.y.max(b.y) + rmax - grid.origin.y) / res).ceil()) as usize).min(grid.ny - 1);
            let ab = b - a;
            let l2 = ab.len2();
            for iy in y0..=y1 {
                for ix in x0..=x1 {
                    let id = iy * grid.nx + ix;
                    if stamp[id] == li as u32 {
                        continue;
                    }
                    let q = grid.pos(ix, iy);
                    let t_raw = if l2 > 0.0 { (q - a).dot(ab) / l2 } else { 0.0 };
                    if !line.closed && ((k == 0 && t_raw < 0.0) || (k + 1 == segn && t_raw > 1.0)) {
                        continue;
                    }
                    let t = t_raw.clamp(0.0, 1.0);
                    let f: V2 = a + ab * t;
                    let r = 0.5 * (wa + (wb - wa) * t);
                    if q.dist(f) <= r {
                        stamp[id] = li as u32;
                        count[id] += 1;
                    }
                }
            }
        }
    }
    Coverage {
        grid,
        inside,
        target,
        count,
    }
}

fn evaluate_impl(
    region: &Region,
    lines: &[&Line],
    p: &WallParams,
    res: f64,
    full: bool,
) -> Metrics {
    let cm = coverage(region, lines, p, res, full);
    let n = cm.grid.len();
    let cell = res * res;
    let mut area_region = 0.0;
    let mut area_target = 0.0;
    for id in 0..n {
        if cm.inside[id] {
            area_region += cell;
            if cm.target[id] {
                area_target += cell;
            }
        }
    }
    let (mut unc, mut ov, mut sp) = (0.0, 0.0, 0.0);
    for id in 0..n {
        let c = cm.count[id] as f64;
        if cm.inside[id] {
            if cm.target[id] && c == 0.0 {
                unc += cell;
            }
            if c > 1.0 {
                ov += (c - 1.0) * cell;
            }
        } else if c > 0.0 {
            sp += cell;
        }
    }
    let mut mt = Metrics {
        area_region,
        area_target,
        lines: lines.len(),
        min_width: f64::INFINITY,
        max_width: 0.0,
        ..Metrics::default()
    };
    let (mut viol, mut tv) = (0.0, 0.0);
    for line in lines {
        let m = line.pts.len();
        let segn = if line.closed { m } else { m.saturating_sub(1) };
        if !line.closed {
            mt.open_lines += 1;
        }
        for k in 0..segn {
            let a = line.pts[k];
            let b = line.pts[(k + 1) % m];
            let l = a.dist(b);
            let wa = line.width[k];
            let wb = line.width[(k + 1) % m];
            mt.length += l;
            mt.bead_area += l * 0.5 * (wa + wb);
            tv += (wb - wa).abs();
            let wm = 0.5 * (wa + wb);
            if wm < p.w_min * 0.999 || wm > p.w_max * 1.001 {
                viol += l;
            }
            mt.min_width = mt.min_width.min(wa.min(wb));
            mt.max_width = mt.max_width.max(wa.max(wb));
        }
    }
    mt.starts_stops = 2 * mt.open_lines;
    if area_target > 0.0 {
        mt.under = unc / area_target;
        mt.over = ov / area_target;
        mt.spill = sp / area_target;
    }
    if mt.length > 0.0 {
        mt.width_violation = viol / mt.length;
        mt.width_tv_per_mm = tv / mt.length;
    }
    if !mt.min_width.is_finite() {
        mt.min_width = 0.0;
    }
    mt
}

/// Measures the perimeter shell only, against the band it is meant to fill.
pub fn evaluate(region: &Region, lines: &[Line], p: &WallParams, res: f64) -> Metrics {
    let walls: Vec<&Line> = lines.iter().filter(|l| l.kind.is_wall()).collect();
    evaluate_impl(region, &walls, p, res, false)
}

/// Measures every path against the whole region.
pub fn evaluate_full(region: &Region, lines: &[Line], p: &WallParams, res: f64) -> Metrics {
    let all: Vec<&Line> = lines.iter().collect();
    evaluate_impl(region, &all, p, res, true)
}

/// Deposited volume of one path over a layer of the given height.
pub fn line_volume(line: &Line, height: f64) -> f64 {
    let m = line.pts.len();
    let segn = if line.closed { m } else { m.saturating_sub(1) };
    let mut v = 0.0;
    for k in 0..segn {
        let a = line.pts[k];
        let b = line.pts[(k + 1) % m];
        let width = 0.5 * (line.width[k] + line.width[(k + 1) % m]);
        v += a.dist(b) * section_area(line.kind, width, height) * line.flow;
    }
    v
}

/// Cross-section of one path, in square millimetres.
///
/// Walls are beads squeezed between the nozzle and the wall beside them, so their two
/// long sides round off and the corners they do not fill are subtracted. Both
/// reference slicers do that for perimeters, and a profile calibrated against their
/// files over-extrudes by 10.7% at 0.4 × 0.2 mm if this does not. They do **not** do
/// it for skin and infill, where beads merge into their neighbours and the region is
/// filled solid; subtracting there would leave every solid surface 11% short, which
/// is exactly what the oracle's 100%-infill comparison catches.
pub fn section_area(kind: Kind, width: f64, height: f64) -> f64 {
    if kind.is_wall() {
        bead_area(width, height)
    } else {
        width * height
    }
}

/// Cross-section of a wall bead, in square millimetres.
///
/// A bead pressed between the nozzle and the wall beside it is not a rectangle: its
/// two long sides round off into the corners, and the four corners are not filled.
/// Both reference slicers subtract that missing corner area, `h²(1 − π/4)`. At 0.4 mm
/// wide and 0.2 mm tall the term is 10.7% of the rectangle, which is the whole gap
/// between this engine's filament figure and theirs on a wall-heavy model.
///
/// A bead narrower than its own corners would come out thinner than nothing, so half
/// the rectangle stays as a floor for that case.
pub fn bead_area(width: f64, height: f64) -> f64 {
    if height <= 0.0 || width <= 0.0 {
        return 0.0;
    }
    let rectangle = width * height;
    let corners = height * height * (1.0 - std::f64::consts::FRAC_PI_4);
    (rectangle - corners).max(0.5 * rectangle).min(rectangle)
}

/// The width a bead of this volume must have had, the inverse of [`bead_area`].
///
/// The verifier reads `E` values back out of a finished file and has to name the
/// width they imply; that name only compares with the profile's widths if the same
/// corner term is added back.
pub fn bead_width(volume: f64, length: f64, height: f64) -> f64 {
    if length <= 0.0 || height <= 0.0 {
        return 0.0;
    }
    volume / (length * height) + height * (1.0 - std::f64::consts::FRAC_PI_4)
}

/// Total deposited volume, split into perimeter and interior.
pub fn extruded_volume(lines: &[Line], height: f64) -> (f64, f64) {
    let mut wv = 0.0;
    let mut iv = 0.0;
    for l in lines {
        let v = line_volume(l, height);
        if l.kind.is_wall() {
            wv += v;
        } else {
            iv += v;
        }
    }
    (wv, iv)
}

/// True when the path is a middle bead or a skin/infill line.
pub fn is_interior(kind: Kind) -> bool {
    !kind.is_wall()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(w: f64) -> Region {
        Region::new(vec![vec![
            V2::new(0.0, 0.0),
            V2::new(w, 0.0),
            V2::new(w, w),
            V2::new(0.0, w),
        ]])
    }

    fn straight(y: f64, x0: f64, x1: f64, width: f64) -> Line {
        Line {
            pts: vec![V2::new(x0, y), V2::new(x1, y)],
            width: vec![width, width],
            closed: false,
            kind: Kind::Infill,
            flow: 1.0,
            overhang: 0.0,
            object: 0,
        }
    }

    #[test]
    fn a_perfect_fill_has_no_undercoverage() {
        let r = square(20.0);
        let p = WallParams::new(0.5, 0);
        let mut lines = Vec::new();
        let mut y = 0.25;
        while y < 20.0 {
            lines.push(straight(y, 0.0, 20.0, 0.5));
            y += 0.5;
        }
        let m = evaluate_full(&r, &lines, &p, 0.1);
        assert!(m.under < 0.02, "under was {}", m.under);
        assert!(m.spill < 0.02, "spill was {}", m.spill);
        assert!(m.bead_area > 390.0);
    }

    #[test]
    fn a_missing_half_is_reported_as_undercoverage() {
        let r = square(20.0);
        let p = WallParams::new(0.4, 0);
        let mut lines = Vec::new();
        let mut y = 0.25;
        while y < 10.0 {
            lines.push(straight(y, 0.0, 20.0, 0.5));
            y += 0.5;
        }
        let m = evaluate_full(&r, &lines, &p, 0.1);
        assert!((m.under - 0.5).abs() < 0.05, "under was {}", m.under);
    }

    #[test]
    fn spill_just_past_the_outline_is_counted() {
        let r = square(10.0);
        let p = WallParams::new(0.4, 0);
        let lines = vec![straight(5.0, -0.3, 10.3, 0.5)];
        let m = evaluate_full(&r, &lines, &p, 0.1);
        assert!(m.spill > 0.002, "spill was {}", m.spill);
        assert!(m.spill < 0.05, "spill was {}", m.spill);
    }

    #[test]
    fn paths_entirely_inside_produce_no_spill() {
        let r = square(10.0);
        let p = WallParams::new(0.4, 0);
        let lines = vec![straight(5.0, 1.0, 9.0, 0.5)];
        let m = evaluate_full(&r, &lines, &p, 0.1);
        assert_eq!(m.spill, 0.0);
        assert!((m.bead_area - 4.0).abs() < 1e-9);
    }

    #[test]
    fn volume_uses_the_trapezoidal_width_profile() {
        let line = Line {
            pts: vec![V2::new(0.0, 0.0), V2::new(10.0, 0.0)],
            width: vec![0.4, 0.8],
            closed: false,
            kind: Kind::Side(0),
            flow: 1.0,
            overhang: 0.0,
            object: 0,
        };
        assert!((line_volume(&line, 0.2) - 10.0 * bead_area(0.6, 0.2)).abs() < 1e-9);
        let (w, i) = extruded_volume(std::slice::from_ref(&line), 0.2);
        assert!((w - 10.0 * bead_area(0.6, 0.2)).abs() < 1e-9);
        assert_eq!(i, 0.0);
    }
}
