//! Bead paths to machine commands.

use std::fmt::Write as _;

use hfw_config::{Flavor, PrintSequence, PrinterConfig, Seam};
use hfw_kernel::{Bounds, V2};
use hfw_toolpath::PlannedLayer;
use hfw_walls::{Kind, Line};

use crate::error::GcodeError;
use crate::route;

/// Print feature a path belongs to, in the order the machine prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// First-layer adhesion skirt.
    Skirt,
    /// First-layer adhesion brim, attached to the part.
    Brim,
    /// Skin printed over air, anchored at both ends.
    Bridge,
    /// Sacrificial support column.
    Support,
    /// Dense support layers in contact with the part.
    SupportInterface,
    /// Outermost wall loop.
    Outer,
    /// Second and later wall loops.
    Inner,
    /// Bead from the medial axis, narrower than a nominal line.
    Thin,
    /// Solid top or bottom skin.
    Solid,
    /// Solid skin on the visible top surface.
    TopSolid,
    /// Sparse infill.
    Sparse,
    /// Slow, near-zero-flow pass that flattens a top surface.
    Ironing,
}

impl Feature {
    /// `;TYPE:` label, matching the names OrcaSlicer and PrusaSlicer emit so
    /// that viewers, Moonraker and analytic tooling group features the same way.
    pub fn label(self) -> &'static str {
        match self {
            Feature::Skirt => "Skirt",
            Feature::Brim => "Brim",
            Feature::Bridge => "Bridge",
            Feature::Support => "Support",
            Feature::SupportInterface => "Support interface",
            Feature::Outer => "Outer wall",
            Feature::Inner => "Inner wall",
            Feature::Thin => "Gap infill",
            Feature::Solid => "Internal solid infill",
            Feature::TopSolid => "Top surface",
            Feature::Sparse => "Sparse infill",
            Feature::Ironing => "Ironing",
        }
    }

    fn rank(self, cfg: &PrinterConfig) -> i32 {
        match self {
            Feature::Skirt => -3,
            Feature::Brim => -2,
            Feature::Support | Feature::SupportInterface => -1,
            Feature::Outer => {
                if cfg.infill_first || cfg.inner_walls_first {
                    20
                } else {
                    0
                }
            }
            Feature::Inner => {
                if cfg.infill_first {
                    21
                } else {
                    10
                }
            }
            Feature::Thin => {
                if cfg.infill_first {
                    22
                } else if cfg.inner_walls_first {
                    0
                } else {
                    30
                }
            }
            Feature::Solid | Feature::TopSolid => {
                if cfg.infill_first {
                    0
                } else {
                    40
                }
            }
            Feature::Sparse => {
                if cfg.infill_first {
                    1
                } else {
                    41
                }
            }
            Feature::Ironing => 42,
            Feature::Bridge => 43,
        }
    }
}

/// One ordered bead path in machine coordinates.
#[derive(Clone, Debug)]
pub struct Path {
    /// Vertices in millimetres.
    pub pts: Vec<V2>,
    /// Extrusion width at each vertex.
    pub width: Vec<f64>,
    /// True when the path closes back onto its first vertex.
    pub closed: bool,
    /// Feature this path belongs to.
    pub feature: Feature,
    /// Print order rank; lower ranks print first.
    pub rank: i32,
    /// Unsupported span this path bridges, in millimetres; 0 when it bridges none.
    pub span: f64,
    /// Extrusion multiplier for this path; 1.0 is normal flow.
    pub flow: f64,
    /// Share of this path that hangs over air, `0..=1`.
    pub overhang: f64,
    /// True when this closed wall loop sits inside another loop of the same
    /// wall index, so it bounds a hole rather than the part.
    pub hole: bool,
}

/// Totals for one emitted file.
#[derive(Clone, Debug, Default)]
pub struct GcodeStats {
    /// Layers emitted.
    pub layers: usize,
    /// Deposited volume in cubic millimetres.
    pub volume_mm3: f64,
    /// Filament consumed in millimetres.
    pub filament_mm: f64,
    /// Filament consumed in grams.
    pub filament_g: f64,
    /// Estimated print time in seconds, without acceleration.
    pub est_seconds: f64,
    /// Total travel distance in millimetres.
    pub travel_mm: f64,
    /// Travels whose straight line crosses a perimeter already printed on the layer.
    ///
    /// Counted where the travel is emitted, so it is the file's own number rather
    /// than a plan's estimate of it.
    pub travel_crossings: usize,
    /// Retraction count.
    pub retractions: usize,
    /// Lowest printed XY.
    pub bbox_min: V2,
    /// Highest printed XY.
    pub bbox_max: V2,
    /// Highest printed Z.
    pub top_z: f64,
}

/// A finished G-code file and its totals.
#[derive(Clone, Debug)]
pub struct Gcode {
    /// Complete file contents.
    pub text: String,
    /// Totals for that file.
    pub stats: GcodeStats,
}

/// Axis-aligned extent of a planned job.
#[derive(Clone, Copy, Debug)]
pub struct ModelExtent {
    /// Footprint in XY.
    pub xy: Bounds,
    /// Lowest layer bottom.
    pub min_z: f64,
    /// Highest layer top.
    pub max_z: f64,
}

impl ModelExtent {
    /// Measures the extent of the planned layers.
    pub fn of(layers: &[PlannedLayer]) -> ModelExtent {
        let mut xy = Bounds::empty();
        let (mut min_z, mut max_z) = (f64::INFINITY, f64::NEG_INFINITY);
        for layer in layers {
            for ring in &layer.reference {
                for p in ring {
                    xy.add(*p);
                }
            }
            if layer.bottom_z.is_finite() {
                min_z = min_z.min(layer.bottom_z);
                max_z = max_z.max(layer.print_z);
            }
        }
        if xy.is_empty() {
            xy.add(V2::ZERO);
        }
        ModelExtent {
            xy,
            min_z: if min_z.is_finite() { min_z } else { 0.0 },
            max_z: if max_z.is_finite() { max_z } else { 0.0 },
        }
    }
}

/// A run of path segments emitted as one machine move.
#[derive(Clone, Copy, Debug)]
enum Run {
    /// Straight move to the next vertex.
    Line { end: usize },
    /// `G2`/`G3` arc through the vertices up to and including `end`.
    Arc {
        start: usize,
        end: usize,
        centre: V2,
        clockwise: bool,
        length: f64,
    },
}

/// The circle through three points, or `None` when they are collinear.
fn circumcircle(a: V2, b: V2, c: V2) -> Option<(V2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None;
    }
    let (aa, bb, cc) = (a.dot(a), b.dot(b), c.dot(c));
    let ux = (aa * (b.y - c.y) + bb * (c.y - a.y) + cc * (a.y - b.y)) / d;
    let uy = (aa * (c.x - b.x) + bb * (a.x - c.x) + cc * (b.x - a.x)) / d;
    let centre = V2::new(ux, uy);
    Some((centre, centre.dist(a)))
}

/// Signed area of the turn at `b` along `a -> b -> c`; positive is a left turn.
fn turn_sign(a: V2, b: V2, c: V2) -> f64 {
    (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x)
}

/// Splits a path into straight moves and arcs.
///
/// A run replaces a straight line only when every vertex it covers lies within
/// `tolerance` of one circle, the turn is consistent in direction, and the run
/// spans at least two segments. That is the difference between a curve and a
/// staircase: a shape traced as forty chords comes out as a handful of arcs,
/// while a genuinely straight wall stays a `G1` because the circumcircle of
/// collinear points does not exist.
fn fit_runs(pts: &[V2], closed: bool, tolerance: f64) -> Vec<Run> {
    let n = pts.len();
    if n < 3 {
        return (0..n.saturating_sub(1))
            .map(|i| Run::Line { end: i + 1 })
            .collect();
    }
    let last = if closed { n } else { n - 1 };
    let at = |i: usize| pts[i % n];
    let mut runs = Vec::with_capacity(n);
    let mut start = 0usize;
    while start < last {
        if start + 2 > last {
            runs.push(Run::Line { end: start + 1 });
            start += 1;
            continue;
        }
        let (a, b, c) = (at(start), at(start + 1), at(start + 2));
        let turn = turn_sign(a, b, c);
        let Some((centre, radius)) = circumcircle(a, b, c) else {
            runs.push(Run::Line { end: start + 1 });
            start += 1;
            continue;
        };
        if turn.abs() < 1e-9 || radius > 1e4 {
            runs.push(Run::Line { end: start + 1 });
            start += 1;
            continue;
        }
        let sign = turn.signum();
        let mut end = start + 2;
        let mut outer = end;
        while end < last {
            let p = at(end + 1);
            if (centre.dist(p) - radius).abs() > tolerance {
                break;
            }
            if turn_sign(at(end - 1), at(end), p).signum() != sign {
                break;
            }
            end += 1;
            outer = end;
        }
        if end <= start + 1 {
            runs.push(Run::Line { end: start + 1 });
            start += 1;
            continue;
        }
        let mut length = 0.0;
        let mut previous = at(start);
        for i in (start + 1)..=outer {
            let p = at(i);
            let chord = previous.dist(p);
            let half = 0.5 * chord / radius;
            let sweep = 2.0 * half.clamp(-1.0, 1.0).asin();
            length += sweep * radius;
            previous = p;
        }
        runs.push(Run::Arc {
            start,
            end: outer,
            centre,
            clockwise: sign < 0.0,
            length,
        });
        start = outer;
    }
    runs
}

fn noise(i: usize, lane: u64) -> f64 {
    let mut x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ lane;
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

fn perimeter(pts: &[V2], closed: bool) -> f64 {
    let n = pts.len();
    if n < 2 {
        return 0.0;
    }
    let segments = if closed { n } else { n - 1 };
    (0..segments).map(|i| pts[i].dist(pts[(i + 1) % n])).sum()
}

fn unit(p: V2) -> Option<V2> {
    let len = (p.x * p.x + p.y * p.y).sqrt();
    if len < 1e-12 {
        None
    } else {
        Some(V2::new(p.x / len, p.y / len))
    }
}

fn resample(pts: &[V2], width: &[f64], closed: bool, step: f64) -> (Vec<V2>, Vec<f64>) {
    let n = pts.len();
    if n < 2 || step <= 0.0 {
        return (pts.to_vec(), width.to_vec());
    }
    let segments = if closed { n } else { n - 1 };
    let mut out_pts = Vec::with_capacity(n);
    let mut out_w = Vec::with_capacity(n);
    for i in 0..segments {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let len = a.dist(b);
        out_pts.push(a);
        out_w.push(width[i]);
        if len <= step {
            continue;
        }
        let pieces = (len / step).ceil() as usize;
        for k in 1..pieces {
            let f = k as f64 / pieces as f64;
            out_pts.push(V2::new(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f));
            let wa = width[i];
            let wb = width[(i + 1) % n];
            out_w.push(wa + (wb - wa) * f);
        }
    }
    if !closed {
        out_pts.push(pts[n - 1]);
        out_w.push(width[n - 1]);
    }
    (out_pts, out_w)
}

fn displace_fuzzy(pts: &mut [V2], thickness: f64, point_dist: f64) {
    let n = pts.len();
    if n < 3 || thickness <= 0.0 {
        return;
    }
    let original = pts.to_vec();
    for i in 1..n {
        let prev = original[(i + n - 1) % n];
        let next = original[(i + 1) % n];
        let tangent = match unit(V2::new(next.x - prev.x, next.y - prev.y)) {
            Some(t) => t,
            None => continue,
        };
        let normal = V2::new(-tangent.y, tangent.x);
        let amp = (noise(i, 0x5EED) - 0.5) * 2.0 * thickness;
        let lane = noise(i.wrapping_mul(2_654_435_761), 0xF00D) * point_dist * 0.5;
        pts[i] = V2::new(
            original[i].x + normal.x * amp + tangent.x * lane,
            original[i].y + normal.y * amp + tangent.y * lane,
        );
    }
}

/// Resampled bead with a flow multiplier per point.
struct Scarved {
    pts: Vec<V2>,
    width: Vec<f64>,
    flow: Vec<f64>,
}

/// Graduates the two ends of a closed wall instead of butting them.
fn scarf(path: &Path, cfg: &PrinterConfig, z: f64) -> Option<Scarved> {
    if !path.closed || cfg.scarf_seam_length <= 0.0 {
        return None;
    }
    match cfg.scarf_seam_placement.as_str() {
        "everywhere" => {}
        "contours" if !path.hole => {}
        _ => return None,
    }
    let outer = path.feature == Feature::Outer;
    let inner = path.feature == Feature::Inner;
    if !outer && !(inner && cfg.scarf_seam_inner) {
        return None;
    }
    if cfg.scarf_seam_start_height > 0.0 && z < cfg.scarf_seam_start_height {
        return None;
    }
    if cfg.scarf_seam_only_on_smooth && !smooth_seam(&path.pts) {
        return None;
    }
    let step = if cfg.scarf_seam_max_segment > 0.0 {
        cfg.scarf_seam_max_segment
    } else {
        0.5
    };
    let (pts, width) = resample(&path.pts, &path.width, true, step);
    let n = pts.len();
    if n < 4 {
        return None;
    }
    let total = perimeter(&pts, true);
    if total <= 1e-6 {
        return None;
    }
    let reach = if cfg.scarf_seam_entire_loop {
        0.5 * total
    } else {
        cfg.scarf_seam_length.min(0.5 * total)
    };
    let mut flow = vec![1.0f64; n];
    let mut walked = 0.0;
    for i in 0..n {
        let through = walked / reach;
        let back = (total - walked) / reach;
        flow[i] = through.min(1.0).min(back).max(0.0);
        walked += pts[i].dist(pts[(i + 1) % n]);
    }
    for f in flow.iter_mut() {
        *f = f.max(0.15);
    }
    Some(Scarved { pts, width, flow })
}

/// True when the loop's first vertex, where the seam sits, turns by no more
/// than the corner threshold the reference slicers call smooth.
fn smooth_seam(pts: &[V2]) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let incoming = (pts[0] - pts[n - 1]).norm();
    let outgoing = (pts[1] - pts[0]).norm();
    incoming.dot(outgoing).clamp(-1.0, 1.0).acos() <= 10f64.to_radians()
}

/// Flags the closed wall loops that bound a hole rather than the part.
///
/// A layer's loops are nested, so a loop is a hole when an odd number of wider
/// loops of its own wall class enclose it. `path_from` normalises winding for
/// the G-code, which is why nesting, not winding, decides.
fn mark_holes(paths: &mut [Path]) {
    let mut rings: Vec<(u8, f64, usize, V2)> = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let class = match p.feature {
            Feature::Outer => 0,
            Feature::Inner => 1,
            _ => continue,
        };
        if p.closed && p.pts.len() >= 3 {
            rings.push((
                class,
                hfw_kernel::ring_signed_area(&p.pts).abs(),
                i,
                p.pts[0],
            ));
        }
    }
    let mut holes = vec![false; paths.len()];
    for &(index, area, target, probe) in &rings {
        let depth = rings
            .iter()
            .filter(|&&(other, wider, id, _)| {
                other == index
                    && id != target
                    && wider > area
                    && hfw_kernel::ring_contains(&paths[id].pts, probe)
            })
            .count();
        holes[target] = depth % 2 == 1;
    }
    for (i, hole) in holes.into_iter().enumerate() {
        paths[i].hole = hole;
    }
}

/// Opens a closed loop by `gap` millimetres so the seam leaves a hole.
fn open_by_gap(pts: &[V2], width: &[f64], gap: f64) -> Option<(Vec<V2>, Vec<f64>)> {
    let total = perimeter(pts, true);
    if gap <= 0.0 || total <= gap * 2.0 {
        return None;
    }
    let target = total - gap;
    let n = pts.len();
    let mut walked = 0.0;
    let mut out_pts = Vec::with_capacity(n);
    let mut out_w = Vec::with_capacity(n);
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let len = a.dist(b);
        if walked + len >= target {
            let f = (target - walked) / len;
            out_pts.push(a);
            out_w.push(width[i]);
            out_pts.push(V2::new(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f));
            let wb = width[(i + 1) % n];
            out_w.push(width[i] + (wb - width[i]) * f);
            return Some((out_pts, out_w));
        }
        out_pts.push(a);
        out_w.push(width[i]);
        walked += len;
    }
    None
}

/// The loop's rearmost vertex, on the bed's +Y side.
fn rear_index(pts: &[V2]) -> usize {
    let mut best = 0;
    for (i, p) in pts.iter().enumerate() {
        let b = pts[best];
        if p.y > b.y + 1e-9 || ((p.y - b.y).abs() <= 1e-9 && p.x > b.x) {
            best = i;
        }
    }
    best
}

/// A seam scatter derived from the loop itself, so it is reproducible and
/// still moves from layer to layer as the cross-section changes.
fn scatter_index(pts: &[V2]) -> usize {
    let first = pts[0];
    let mut h = (first.x.abs() * 1000.0) as u64;
    h ^= ((first.y.abs() * 1000.0) as u64).rotate_left(17);
    h ^= (pts.len() as u64).rotate_left(31);
    let h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (h >> 7) as usize % pts.len()
}

fn seam_index(pts: &[V2]) -> usize {
    let n = pts.len();
    let mut best = 0;
    let mut best_key = (-1i32, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for i in 0..n {
        let a = pts[(i + n - 1) % n];
        let b = pts[i];
        let c = pts[(i + 1) % n];
        let u = (b - a).norm();
        let v = (c - b).norm();
        let turn = u.dot(v).clamp(-1.0, 1.0).acos();
        let key = ((turn > 0.4) as i32, (b.y * 1e6).round() / 1e6, -b.x);
        if key.0 > best_key.0
            || (key.0 == best_key.0
                && (key.1 > best_key.1 || (key.1 == best_key.1 && key.2 > best_key.2)))
        {
            best_key = key;
            best = i;
        }
    }
    best
}

/// Limits how fast the volumetric rate may change from one bead to the next.
///
/// The plastic in the nozzle is a spring: a bead that starts asking for three
/// times the flow of the one before it arrives late and wide at its start. The
/// reference dialects cap that change with a slope in cubic millimetres per
/// second squared, per direction, and this is the same cap solved per bead.
///
/// A rate that would climb too fast is slowed; the bead's own duration grows as
/// it slows, which is why the limit is the root of a quadratic rather than a
/// division. A rate that would fall too fast is sped up toward the previous one,
/// which is what the reference equaliser does too, and the result is capped by
/// `max_speed` so the machine is never asked for more than it advertised.
///
/// The section is the *widest* the bead gets, not its average. A bead path that
/// widens inside itself runs at one speed for its whole length, so its rate steps
/// up where the width does; solving against the widest section is the only choice
/// that keeps that step inside the cap without splitting the move. The cost is
/// that a path narrowing at its far end is slowed by its own wide end.
fn limit_rate_slope(
    ordered: &[Path],
    lengths: &[f64],
    speeds: &mut [f64],
    cfg: &PrinterConfig,
    height: f64,
    prev: f64,
) -> f64 {
    let up = cfg.max_volumetric_rate_slope_positive;
    let down = cfg.max_volumetric_rate_slope_negative;
    let mut last = prev;
    for (i, path) in ordered.iter().enumerate() {
        let len = lengths[i];
        if len <= 1e-9 || path.feature == Feature::Ironing {
            continue;
        }
        let width = path.width.iter().cloned().fold(0.0f64, f64::max);
        let section = width * height * cfg.flow * path.flow;
        if section <= 1e-12 {
            continue;
        }
        let mut speed = speeds[i];
        let mut rate = section * speed;
        if up > 0.0 && last > 0.0 && rate > last {
            let b = last;
            let c = up * len;
            let cap = (b + (b * b + 4.0 * section * c).sqrt()) / (2.0 * section);
            if speed > cap {
                speed = cap;
                rate = section * speed;
            }
        }
        if down > 0.0 && last > 0.0 && rate < last {
            let b = last;
            let disc = b * b - 4.0 * section * down * len;
            if disc > 0.0 {
                let root = (b + disc.sqrt()) / (2.0 * section);
                if speed < root {
                    speed = if cfg.max_speed > 0.0 {
                        root.min(cfg.max_speed.max(speeds[i]))
                    } else {
                        root
                    };
                    rate = section * speed;
                }
            }
        }
        speeds[i] = speed;
        last = rate;
    }
    last
}

/// Stretches a layer's speeds so it takes `target` seconds.
///
/// `proportional` scales every bead by one factor, which is the cheapest way to
/// buy time and the one that leaves the whole layer looking uniform. `uniform`
/// and `consistent_surface` spend the deficit on the fastest beads first, down to
/// the profile's floor, and reach for the walls only once the interior is spent;
/// `consistent_surface` holds back the band
/// [`PrinterConfig::cooling_perimeter_transition_distance`] millimetres wide next
/// to the wall, so the visible surface keeps its speed while the interior cools.
///
/// Returns the layer's time after the slowdown.
fn apply_cooling(
    ordered: &[Path],
    speeds: &mut [f64],
    lengths: &[f64],
    cfg: &PrinterConfig,
    target: f64,
) -> f64 {
    let time = |speeds: &[f64]| -> f64 {
        lengths
            .iter()
            .zip(speeds)
            .map(|(len, speed)| len / speed.max(1e-9))
            .sum()
    };
    if target <= 0.0 {
        return time(speeds);
    }
    let floor_of = |nominal: f64| -> f64 {
        let by_factor = nominal * cfg.min_speed_factor;
        match cfg.min_print_speed {
            0.0 => {
                if cfg.slow_down_min_speed > 0.0 {
                    cfg.slow_down_min_speed
                        .min(nominal)
                        .max(by_factor.min(nominal))
                } else {
                    by_factor
                }
            }
            speed => speed.min(nominal).max(by_factor.min(nominal)),
        }
    };
    let mut current = time(speeds);
    if current >= target {
        return current;
    }
    if cfg.cooling_slowdown_logic == "proportional" {
        let factor = (current / target).clamp(cfg.min_speed_factor, 1.0);
        let nominal = speeds.to_vec();
        for (speed, was) in speeds.iter_mut().zip(nominal) {
            *speed = (was * factor).max(floor_of(was));
        }
        return time(speeds);
    }
    let mut wall_pts: Vec<(V2, V2)> = Vec::new();
    let band = cfg.cooling_perimeter_transition_distance;
    if cfg.cooling_slowdown_logic == "consistent_surface" && band > 0.0 {
        for p in ordered
            .iter()
            .filter(|p| matches!(p.feature, Feature::Outer | Feature::Inner))
        {
            for pair in p.pts.windows(2) {
                wall_pts.push((pair[0], pair[1]));
            }
            if p.closed && p.pts.len() > 2 {
                wall_pts.push((p.pts[p.pts.len() - 1], p.pts[0]));
            }
        }
    }
    let in_band = |p: &Path| -> bool {
        if wall_pts.is_empty() {
            return false;
        }
        let n = p.pts.len();
        let mid = if p.closed || n < 2 {
            p.pts[n / 2]
        } else {
            (p.pts[0] + p.pts[n - 1]) * 0.5
        };
        wall_pts
            .iter()
            .any(|&(a, b)| point_segment(mid, a, b) < band)
    };
    let wall = |p: &Path| matches!(p.feature, Feature::Outer | Feature::Inner | Feature::Thin);
    let order: Vec<usize> = (0..ordered.len()).collect();
    let pass = |order: &[usize], pat: &dyn Fn(&Path) -> bool| -> Vec<usize> {
        let mut out: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| pat(&ordered[i]))
            .collect();
        out.sort_by(|&a, &b| {
            speeds[b]
                .partial_cmp(&speeds[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    };
    let interior = |p: &Path| !wall(p) && !in_band(p);
    let rest = |p: &Path| wall(p) || in_band(p);
    let mut deficit = target - current;
    for pass in [pass(&order, &interior), pass(&order, &rest)] {
        for i in pass {
            if deficit <= 1e-9 {
                break;
            }
            let nominal = speeds[i];
            let floor = floor_of(nominal);
            if nominal <= floor {
                continue;
            }
            let len = lengths[i];
            let now = len / nominal;
            let at_floor = len / floor;
            let extra = at_floor - now;
            if extra <= deficit {
                speeds[i] = floor;
                deficit -= extra;
            } else {
                speeds[i] = (len / (now + deficit)).max(floor);
                deficit = 0.0;
            }
        }
    }
    current = time(speeds);
    current
}

/// Distance from `p` to the segment `a`-`b`.
fn point_segment(p: V2, a: V2, b: V2) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    if len2 <= 1e-12 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.dist(a + ab * t)
}

fn trim_open(pts: &[V2], width: &[f64], floor: f64) -> Option<(Vec<V2>, Vec<f64>)> {
    let first = width.iter().position(|&x| x >= floor)?;
    let last = width.iter().rposition(|&x| x >= floor)?;
    if last <= first {
        return None;
    }
    let mut out_pts = Vec::new();
    let mut out_width = Vec::new();
    if first > 0 {
        let t = (floor - width[first - 1]) / (width[first] - width[first - 1]);
        out_pts.push(pts[first - 1].lerp(pts[first], t));
        out_width.push(floor);
    }
    for k in first..=last {
        out_pts.push(pts[k]);
        out_width.push(width[k].max(floor));
    }
    if last + 1 < pts.len() {
        let t = (width[last] - floor) / (width[last] - width[last + 1]);
        out_pts.push(pts[last].lerp(pts[last + 1], t));
        out_width.push(floor);
    }
    Some((out_pts, out_width))
}

fn feature_of(kind: Kind) -> Feature {
    match kind {
        Kind::Side(0) => Feature::Outer,
        Kind::Side(_) => Feature::Inner,
        Kind::Middle => Feature::Thin,
        Kind::Solid => Feature::Solid,
        Kind::TopSolid => Feature::TopSolid,
        Kind::Infill => Feature::Sparse,
        Kind::Brim => Feature::Brim,
        Kind::Bridge | Kind::BridgeAnchor => Feature::Bridge,
        Kind::Support => Feature::Support,
        Kind::SupportInterface => Feature::SupportInterface,
        Kind::Ironing => Feature::Ironing,
    }
}

fn path_from(line: &Line, tx: f64, ty: f64, cfg: &PrinterConfig) -> Option<Path> {
    let mut pts: Vec<V2> = line
        .pts
        .iter()
        .map(|p| V2::new(p.x + tx, p.y + ty))
        .collect();
    let mut width = line.width.clone();
    if line.closed {
        for w in width.iter_mut() {
            *w = w.max(cfg.min_bead_width);
        }
    } else if matches!(line.kind, Kind::Side(_) | Kind::Middle) {
        let trimmed = trim_open(&pts, &width, cfg.min_bead_width)?;
        pts = trimmed.0;
        width = trimmed.1;
    }
    if line.closed && hfw_kernel::ring_signed_area(&pts) < 0.0 {
        pts.reverse();
        width.reverse();
    }
    let feature = feature_of(line.kind);
    let span = if feature == Feature::Bridge && pts.len() > 1 {
        pts[0].dist(pts[pts.len() - 1])
    } else {
        0.0
    };
    let mut path = Path {
        pts,
        width,
        closed: line.closed,
        feature,
        rank: object_rank(
            line.object,
            match line.kind {
                Kind::BridgeAnchor => feature.rank(cfg) - 1,
                _ => feature.rank(cfg),
            },
        ),
        span,
        overhang: line.overhang,
        hole: false,
        flow: if feature == Feature::Ironing {
            cfg.ironing_flow
        } else {
            line.flow
        },
    };
    if path.closed && path.pts.len() >= 3 {
        let at = match cfg.seam {
            Seam::Aligned => Some(seam_index(&path.pts)),
            Seam::Rear => Some(rear_index(&path.pts)),
            Seam::Random => Some(scatter_index(&path.pts)),
            Seam::Nearest => None,
        };
        if let Some(at) = at {
            path.pts.rotate_left(at);
            path.width.rotate_left(at);
        }
    }
    if path.closed && cfg.staggered_inner_seams {
        if let Kind::Side(index) = line.kind {
            if index > 0 {
                let by = path.pts.len() / (index as usize + 1) + 1;
                let by = by % path.pts.len();
                path.pts.rotate_left(by);
                path.width.rotate_left(by);
            }
        }
    }
    let fuzzy = match cfg.fuzzy_skin.as_str() {
        "all" => matches!(line.kind, Kind::Side(_)),
        "external" => matches!(line.kind, Kind::Side(0)),
        _ => false,
    };
    if fuzzy && line.closed && cfg.fuzzy_skin_thickness > 0.0 {
        let (mut pts, mut width) = resample(
            &path.pts,
            &path.width,
            true,
            cfg.fuzzy_skin_point_dist.max(0.01),
        );
        displace_fuzzy(
            &mut pts,
            cfg.fuzzy_skin_thickness,
            cfg.fuzzy_skin_point_dist,
        );
        if pts.len() == width.len() && pts.len() >= 3 {
            width
                .iter_mut()
                .for_each(|w| *w = w.max(cfg.min_bead_width));
            path.pts = pts;
            path.width = width;
        }
    }
    Some(path)
}

/// How many nearby entry points of one loop are worth testing.
const ENTRY_CANDIDATES: usize = 8;

/// True when the straight travel `a`-`b` crosses one of `segments`.
///
/// Footprint rings of this layer's overhanging beads.
///
/// A bead that hangs over air curls as it cools, so a travel across it is a
/// travel into plastic that is no longer flat. The rings are the beads' own
/// rectangles: one per segment, `width` wide, so the router and the lift test
/// see the plastic rather than the centre line.
fn curled_obstacles(paths: &[Path]) -> Vec<Vec<V2>> {
    let mut out = Vec::new();
    for p in paths.iter().filter(|p| p.overhang > 0.0) {
        let n = p.pts.len();
        let segments = if p.closed { n } else { n.saturating_sub(1) };
        for i in 0..segments {
            let a = p.pts[i];
            let b = p.pts[(i + 1) % n];
            let d = b - a;
            if d.dot(d) < 1e-12 {
                continue;
            }
            let half = p.width.get(i).copied().unwrap_or(0.4) * 0.5;
            let off = V2::new(-d.y, d.x).norm() * half;
            out.push(vec![a + off, b + off, b - off, a - off]);
        }
    }
    out
}

/// A touch at an endpoint is not a crossing: an infill line that starts on the
/// wall it was clipped by must not be pushed away from it.
fn crosses(a: V2, b: V2, segments: &[(V2, V2)]) -> bool {
    let lo = V2::new(a.x.min(b.x), a.y.min(b.y));
    let hi = V2::new(a.x.max(b.x), a.y.max(b.y));
    segments.iter().any(|&(c, d)| {
        let slo = V2::new(c.x.min(d.x), c.y.min(d.y));
        let shi = V2::new(c.x.max(d.x), c.y.max(d.y));
        if shi.x < lo.x || slo.x > hi.x || shi.y < lo.y || slo.y > hi.y {
            return false;
        }
        straddles_edges(a, b, c, d)
    })
}

/// True when two segments cross, with a touch at an endpoint not counting.
fn straddles_edges(a: V2, b: V2, c: V2, d: V2) -> bool {
    let orient =
        |p: V2, q: V2, r: V2| -> f64 { (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x) };
    let straddles = |p: f64, q: f64| -> bool { (p > 1e-9 && q < -1e-9) || (p < -1e-9 && q > 1e-9) };
    straddles(orient(a, b, c), orient(a, b, d)) && straddles(orient(c, d, a), orient(c, d, b))
}

/// True when the travel `a`-`b` passes through the ring.
fn ring_crosses(a: V2, b: V2, ring: &[V2]) -> bool {
    let n = ring.len();
    if n < 2 {
        return false;
    }
    (0..n).any(|i| straddles_edges(a, b, ring[i], ring[(i + 1) % n]))
}

/// The loop's nearest entry point to `pos`, then the rest in order.
fn entry_candidates(p: &Path, pos: V2) -> Vec<(usize, f64)> {
    let mut ranked: Vec<(usize, f64)> = p.pts.iter().map(|q| q.dist(pos)).enumerate().collect();
    ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(ENTRY_CANDIDATES);
    ranked
}

fn nearest_entry(p: &Path, pos: V2) -> f64 {
    p.pts
        .iter()
        .map(|q| q.dist(pos))
        .fold(f64::INFINITY, f64::min)
}

fn is_perimeter(f: Feature) -> bool {
    matches!(f, Feature::Outer | Feature::Inner | Feature::Thin)
}

/// The entry a path should take, as `(index, reverse)`, or `None` to keep its own.
///
/// A closed loop may be entered at any vertex and an open line at either end, so
/// the choice is scored over the pair of travels it touches: the one arriving and
/// the one leaving towards `next`. Scoring only the arrival loses, because moving
/// an entry moves the exit, and the next travel inherits it. `cap` limits how much
/// extra travel is worth it, and zero means no limit, which is the dialect default.
fn choose_entry(
    path: &Path,
    from: V2,
    drawn: &[(V2, V2)],
    cap: f64,
    next: Option<V2>,
) -> Option<(usize, bool)> {
    if path.pts.len() < 2 || drawn.is_empty() {
        return None;
    }
    let score = |entry: V2, exit: V2| -> usize {
        usize::from(crosses(from, entry, drawn))
            + match next {
                Some(next) => usize::from(crosses(exit, next, drawn)),
                None => 0,
            }
    };
    let last = path.pts.len() - 1;
    let keep_exit = if path.closed {
        path.pts[0]
    } else {
        path.pts[last]
    };
    let keep = score(path.pts[0], keep_exit);
    if keep == 0 {
        return None;
    }
    let nearest = nearest_entry(path, from);
    let mut best: Option<(usize, usize, bool, f64)> = None;
    if path.closed {
        for (at, d) in entry_candidates(path, from) {
            if cap > 0.0 && d - nearest > cap {
                continue;
            }
            let entry = path.pts[at];
            let cost = score(entry, entry);
            let better = best.is_none_or(|(c, _, _, bd)| cost < c || (cost == c && d < bd));
            if cost < keep && better {
                best = Some((cost, at, false, d));
            }
        }
    }
    if !path.closed {
        let d = path.pts[last].dist(from);
        let fits = cap <= 0.0 || d - nearest <= cap;
        let cost = score(path.pts[last], path.pts[0]);
        let better = best.is_none_or(|(c, _, _, bd)| cost < c || (cost == c && d < bd));
        if fits && cost < keep && better {
            best = Some((cost, 0, true, d));
        }
    }
    best.map(|(_, at, reverse, _)| (at, reverse))
}

/// Rank spacing that keeps one object's paths ahead of the next object's.
///
/// Features rank within `-3..=43`, so a stride of 1000 leaves room for future
/// features without letting one object's order leak into another's. Support and
/// skirt ranks are negative, which is why a later object's support still lands
/// after the previous object's infill.
const OBJECT_RANK_STRIDE: i32 = 1000;

fn object_rank(object: u16, rank: i32) -> i32 {
    i32::from(object) * OBJECT_RANK_STRIDE + rank
}

fn order(mut paths: Vec<Path>, mut pos: V2, seam: Seam) -> (Vec<Path>, V2, f64) {
    let mut out = Vec::with_capacity(paths.len());
    let mut travel = 0.0;
    while !paths.is_empty() {
        let mut pick = 0;
        let mut best = f64::INFINITY;
        let mut reverse = false;
        for (i, p) in paths.iter().enumerate() {
            let (d, rev) = if p.closed {
                if seam == Seam::Nearest {
                    (nearest_entry(p, pos), false)
                } else {
                    (p.pts[0].dist(pos), false)
                }
            } else {
                let start = p.pts[0].dist(pos);
                let end = p.pts[p.pts.len() - 1].dist(pos);
                if end < start {
                    (end, true)
                } else {
                    (start, false)
                }
            };
            if d < best {
                best = d;
                pick = i;
                reverse = rev;
            }
        }
        let mut p = paths.swap_remove(pick);
        if p.closed && seam == Seam::Nearest {
            let mut at = 0;
            let mut nearest = f64::INFINITY;
            for (i, q) in p.pts.iter().enumerate() {
                let d = q.dist(pos);
                if d < nearest {
                    nearest = d;
                    at = i;
                }
            }
            p.pts.rotate_left(at);
            p.width.rotate_left(at);
        }
        if reverse {
            p.pts.reverse();
            p.width.reverse();
        }
        travel += best;
        pos = if p.closed {
            p.pts[0]
        } else {
            p.pts[p.pts.len() - 1]
        };
        out.push(p);
    }
    (out, pos, travel)
}

/// Acceleration for a feature, or 0 when the profile asks for none.
///
/// A feature value wins over `default_accel`; the first layer wins over both; a
/// machine ceiling in the profile caps whatever comes out. Zero at the end means
/// nothing is emitted, which is how a profile that never mentions acceleration
/// keeps the machine's own setting.
fn base_accel(feature: Feature, cfg: &PrinterConfig, first_layer: bool) -> f64 {
    let mut own = match feature {
        Feature::Outer => cfg.outer_wall_accel,
        Feature::Inner => cfg.inner_wall_accel,
        Feature::Solid => cfg.solid_accel,
        Feature::TopSolid => cfg.top_solid_accel,
        Feature::Sparse => cfg.infill_accel,
        Feature::Support => cfg.support_accel,
        Feature::SupportInterface => cfg.support_interface_accel,
        Feature::Bridge => cfg.bridge_acceleration,
        Feature::Ironing => 0.0,
        Feature::Skirt | Feature::Brim | Feature::Thin => 0.0,
    };
    if own == 0.0 && matches!(feature, Feature::Outer | Feature::Inner) {
        own = cfg.perimeter_accel;
    }
    if own == 0.0 && feature == Feature::TopSolid {
        own = cfg.solid_accel;
    }
    if own == 0.0 && matches!(feature, Feature::Solid | Feature::TopSolid) {
        own = cfg.infill_accel;
    }
    let mut accel = if own > 0.0 { own } else { cfg.default_accel };
    if first_layer && cfg.initial_layer_accel > 0.0 {
        accel = cfg.initial_layer_accel;
    }
    let ceiling = xy_ceiling(cfg.machine_max_accel_x, cfg.machine_max_accel_y);
    if ceiling > 0.0 && accel > 0.0 {
        accel = accel.min(ceiling);
    }
    accel
}

/// The lower of two axis ceilings, with 0 meaning unset.
fn xy_ceiling(x: f64, y: f64) -> f64 {
    match (x, y) {
        (0.0, 0.0) => 0.0,
        (0.0, y) => y,
        (x, 0.0) => x,
        (x, y) => x.min(y),
    }
}

/// Movement axis a feed belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// X or Y, whichever is slower.
    XY,
    /// Z.
    Z,
    /// Extruder only.
    E,
}

/// Fastest feed the machine accepts, in millimetres per second; 0 means none.
fn feed_ceiling(cfg: &PrinterConfig, axis: Axis) -> f64 {
    match axis {
        Axis::XY => xy_ceiling(cfg.machine_max_feedrate_x, cfg.machine_max_feedrate_y),
        Axis::Z => cfg.machine_max_feedrate_z,
        Axis::E => cfg.machine_max_feedrate_e,
    }
}

/// Machine limit preamble, or an empty string when the profile sets none.
///
/// Klipper takes the ceiling as a g-code command and has no jerk; Marlin takes
/// `M201`/`M203`/`M205`. Emitting a command the flavour does not implement would
/// fail on the machine, so each flavour gets only its own.
fn machine_limits(cfg: &PrinterConfig) -> String {
    let mut text = String::new();
    let accel = xy_ceiling(cfg.machine_max_accel_x, cfg.machine_max_accel_y);
    let velocity = feed_ceiling(cfg, Axis::XY);
    match cfg.flavor {
        Flavor::Klipper => {
            if velocity > 0.0 && accel > 0.0 {
                let _ = writeln!(
                    text,
                    "SET_VELOCITY_LIMIT VELOCITY={velocity:.0} ACCEL={accel:.0}"
                );
            } else if velocity > 0.0 {
                let _ = writeln!(text, "SET_VELOCITY_LIMIT VELOCITY={velocity:.0}");
            } else if accel > 0.0 {
                let _ = writeln!(text, "SET_VELOCITY_LIMIT ACCEL={accel:.0}");
            }
        }
        Flavor::Marlin => {
            let accelerations = [
                ("X", cfg.machine_max_accel_x),
                ("Y", cfg.machine_max_accel_y),
                ("Z", cfg.machine_max_accel_z),
                ("E", cfg.machine_max_accel_e),
            ];
            if accelerations.iter().any(|(_, value)| *value > 0.0) {
                text.push_str("M201");
                for (name, value) in accelerations {
                    if value > 0.0 {
                        let _ = write!(text, " {name}{value:.0}");
                    }
                }
                text.push('\n');
            }
            if cfg.machine_max_accel_extruding > 0.0
                || cfg.machine_max_accel_retracting > 0.0
                || cfg.machine_max_accel_travel > 0.0
            {
                text.push_str("M204");
                if cfg.machine_max_accel_extruding > 0.0 {
                    let _ = write!(text, " P{:.0}", cfg.machine_max_accel_extruding);
                }
                if cfg.machine_max_accel_retracting > 0.0 {
                    let _ = write!(text, " R{:.0}", cfg.machine_max_accel_retracting);
                }
                if cfg.machine_max_accel_travel > 0.0 {
                    let _ = write!(text, " T{:.0}", cfg.machine_max_accel_travel);
                }
                text.push('\n');
            }
            let feeds = [
                ("X", cfg.machine_max_feedrate_x),
                ("Y", cfg.machine_max_feedrate_y),
                ("Z", cfg.machine_max_feedrate_z),
                ("E", cfg.machine_max_feedrate_e),
            ];
            if feeds.iter().any(|(_, value)| *value > 0.0) {
                text.push_str("M203");
                for (name, value) in feeds {
                    if value > 0.0 {
                        let _ = write!(text, " {name}{:.0}", value * 60.0);
                    }
                }
                text.push('\n');
            }
            let jerks = [
                ("X", cfg.machine_max_jerk_x),
                ("Y", cfg.machine_max_jerk_y),
                ("Z", cfg.machine_max_jerk_z),
                ("E", cfg.machine_max_jerk_e),
            ];
            if jerks.iter().any(|(_, value)| *value > 0.0) {
                text.push_str("M205");
                for (name, value) in jerks {
                    if value > 0.0 {
                        let _ = write!(text, " {name}{value:.1}");
                    }
                }
                text.push('\n');
            }
            if cfg.machine_max_junction_deviation > 0.0 {
                let _ = writeln!(text, "M205 J{:.3}", cfg.machine_max_junction_deviation);
            }
            if cfg.machine_min_extruding_rate > 0.0 || cfg.machine_min_travel_rate > 0.0 {
                text.push_str("M205");
                if cfg.machine_min_extruding_rate > 0.0 {
                    let _ = write!(text, " S{:.0}", cfg.machine_min_extruding_rate);
                }
                if cfg.machine_min_travel_rate > 0.0 {
                    let _ = write!(text, " T{:.0}", cfg.machine_min_travel_rate);
                }
                text.push('\n');
            }
        }
    }
    text
}

/// Fan duty for a layer, in PWM counts out of 255.
///
/// Three rules, in the order a slicer applies them: nothing before the start
/// layer; full speed on a layer printed faster than `fan_below_layer_time`; a
/// linear ramp from `min_fan_speed` to `fan_speed` between the start layer and
/// `full_fan_speed_layer`, which collapses to the old instant-on behaviour when
/// the profile leaves the ramp unset.
fn fan_pwm_for(layer: usize, cfg: &PrinterConfig, layer_time: f64) -> i64 {
    if !cfg.cooling {
        return 0;
    }
    if layer == 0 && cfg.initial_layer_fan_speed >= 0.0 {
        return (cfg.initial_layer_fan_speed * 2.55).round() as i64;
    }
    if layer < cfg.fan_start_layer || cfg.fan_speed <= 0.0 {
        return if cfg.fan_always_on {
            let floor = if cfg.min_fan_speed > 0.0 {
                cfg.min_fan_speed
            } else {
                cfg.fan_speed
            };
            (floor * 2.55).round() as i64
        } else {
            0
        };
    }
    let full = if cfg.full_fan_speed_layer > cfg.fan_start_layer {
        cfg.full_fan_speed_layer
    } else {
        cfg.fan_start_layer
    };
    let percent = if cfg.fan_below_layer_time > 0.0 && layer_time < cfg.fan_below_layer_time {
        cfg.fan_speed
    } else if full > cfg.fan_start_layer && layer < full {
        let span = (full - cfg.fan_start_layer) as f64;
        let at = (layer - cfg.fan_start_layer) as f64 / span;
        let low = if cfg.min_fan_speed > 0.0 {
            cfg.min_fan_speed
        } else {
            cfg.fan_speed
        };
        low + (cfg.fan_speed - low) * at
    } else {
        cfg.fan_speed
    };
    (percent * 2.55).round() as i64
}

/// Which quarter of an overhang a path is in, `0..=3`.
///
/// The share of the path that hangs over air is the measurement this engine has:
/// a wall leaning out with 30% of its length over nothing behaves like the
/// shallow case, one that is 90% over air behaves like the severe case. A path
/// with no overhang returns `None`, so a fully supported wall keeps its feature
/// speed and fan.
fn overhang_quarter(overhang: f64) -> Option<usize> {
    if overhang <= 0.0 {
        return None;
    }
    let quarter = (overhang * 4.0).ceil() as usize;
    Some(quarter.clamp(1, 4) - 1)
}

fn base_speed(feature: Feature, cfg: &PrinterConfig, first_layer: bool) -> f64 {
    let clamp = |speed: f64| match feed_ceiling(cfg, Axis::XY) {
        0.0 => speed,
        ceiling => speed.min(ceiling),
    };
    let top_solid = if cfg.top_solid_speed > 0.0 {
        cfg.top_solid_speed
    } else {
        cfg.solid_speed
    };
    let speed = match feature {
        Feature::Skirt => cfg.first_layer_speed,
        Feature::Brim => cfg.first_layer_speed,
        Feature::Outer => cfg.outer_wall_speed,
        Feature::Inner => cfg.inner_wall_speed,
        Feature::Thin => cfg.thin_speed,
        Feature::Solid => cfg.solid_speed,
        Feature::TopSolid => top_solid,
        Feature::Sparse => cfg.infill_speed,
        Feature::Bridge => cfg.bridge_speed,
        Feature::Support => cfg.support_speed,
        Feature::SupportInterface => cfg.support_interface_speed,
        Feature::Ironing => cfg.ironing_speed,
    };
    if !first_layer || feature == Feature::Bridge {
        return clamp(speed);
    }
    let own_first = match feature {
        Feature::Outer => {
            cfg.first_layer_outer_speed
                .max(if cfg.first_layer_perimeter_speed > 0.0 {
                    cfg.first_layer_perimeter_speed
                } else {
                    0.0
                })
        }
        Feature::Inner => {
            cfg.first_layer_inner_speed
                .max(if cfg.first_layer_perimeter_speed > 0.0 {
                    cfg.first_layer_perimeter_speed
                } else {
                    0.0
                })
        }
        Feature::Thin => cfg.first_layer_thin_speed,
        Feature::Solid => cfg.first_layer_solid_speed,
        Feature::TopSolid => cfg.first_layer_top_solid_speed,
        Feature::Sparse => cfg.first_layer_infill_speed,
        Feature::Support | Feature::SupportInterface => cfg.first_layer_support_speed,
        Feature::Skirt | Feature::Brim | Feature::Bridge | Feature::Ironing => 0.0,
    };
    if own_first > 0.0 {
        clamp(own_first)
    } else {
        clamp(speed.min(cfg.first_layer_speed))
    }
}

/// The feature speed, replaced by the overhang ladder when the profile asks.
///
/// A zero rung means "no opinion", so a profile can set two of the four and let
/// the other two fall through to the feature speed rather than to zero.
fn path_speed(feature: Feature, cfg: &PrinterConfig, first_layer: bool, overhang: f64) -> f64 {
    if cfg.dynamic_overhang_speeds && !first_layer {
        if let Some(quarter) = overhang_quarter(overhang) {
            let rung = cfg.overhang_speeds[quarter];
            if rung > 0.0 {
                return match xy_ceiling(cfg.machine_max_feedrate_x, cfg.machine_max_feedrate_y) {
                    0.0 => rung,
                    ceiling => rung.min(ceiling),
                };
            }
        }
    }
    base_speed(feature, cfg, first_layer)
}

/// The volumetric flow a path may not exceed, in mm³/s; zero means no limit.
///
/// Two numbers can set it: the machine's own ceiling and the filament's. The
/// lower wins, because a filament that cannot take the flow is a property of the
/// plastic and not of the printer.
fn volumetric_limit(cfg: &PrinterConfig) -> f64 {
    match (
        cfg.max_volumetric_speed > 0.0,
        cfg.filament_max_volumetric_speed > 0.0,
    ) {
        (true, true) => cfg
            .max_volumetric_speed
            .min(cfg.filament_max_volumetric_speed),
        (true, false) => cfg.max_volumetric_speed,
        (false, true) => cfg.filament_max_volumetric_speed,
        (false, false) => 0.0,
    }
}

/// Speed ceiling that belongs to the filament rather than to the printer.
///
/// Sparse infill carries two: the plain one, and the one for infill that reaches
/// into the shell. This engine anchors every infill line into the perimeter by
/// default, so the crossing ceiling is the one an infill line usually meets; with
/// the anchor switched off the plain ceiling applies.
fn filament_speed_limit(feature: Feature, cfg: &PrinterConfig) -> f64 {
    if feature != Feature::Sparse {
        return 0.0;
    }
    match (
        cfg.filament_infill_max_speed > 0.0,
        cfg.filament_infill_max_crossing_speed > 0.0,
        cfg.infill_anchor != 0.0,
    ) {
        (true, true, true) => cfg
            .filament_infill_max_speed
            .min(cfg.filament_infill_max_crossing_speed),
        (true, true, false) => cfg.filament_infill_max_speed,
        (true, false, _) => cfg.filament_infill_max_speed,
        (false, true, true) => cfg.filament_infill_max_crossing_speed,
        _ => 0.0,
    }
}

/// Bridge extrusion height: one layer, or one nozzle diameter for thick bridges.
fn bridge_height(cfg: &PrinterConfig, layer_height: f64) -> f64 {
    if cfg.thick_bridges {
        cfg.nozzle_diameter
    } else {
        layer_height
    }
}

/// Bridge flow multiplier, adjusted with the span past `bridge_span_ref`.
///
/// Sag is not linear in flow, and the sign is not obvious: an external bridge
/// that droops hangs from its own mass, so thinning the strand past a few beads
/// helps more than fattening it. Both slicers under comparison apply one flow to
/// every span, so the coefficient cannot be measured from them either; the
/// ladder coupon in `fixtures/bridge_ladder.stl` gives four spans in one print
/// for tuning it.
fn bridge_flow_scale(cfg: &PrinterConfig, span: f64) -> f64 {
    let over = (span - cfg.bridge_span_ref).max(0.0);
    cfg.bridge_flow * (1.0 + cfg.bridge_span_flow * over)
}

/// Adhesion brim loops, from the width when the profile states one.
///
/// A width is how the reference dialects state a brim and a loop count is how
/// this engine draws it, so the count is the width divided by a bead and rounded
/// to the nearest whole loop; one loop is the floor, because a brim that rounds
/// to zero is a brim the profile asked for and did not get.
fn brim_loops(cfg: &PrinterConfig) -> usize {
    if cfg.brim_width > 0.0 {
        ((cfg.brim_width / cfg.line_width).round() as usize).max(1)
    } else {
        cfg.brim_loops
    }
}

/// Bridge speed for a span, falling with span and never below a floor.
///
/// A long bridge pulls its own bead into a straight line only while the bead is
/// still molten; slowing down gives each line more time to cool against the
/// anchors before the next one lands on it.
fn bridge_speed_span(cfg: &PrinterConfig, span: f64) -> f64 {
    let over = (span - cfg.bridge_span_ref).max(0.0);
    let scale = (1.0 - cfg.bridge_span_speed * over).max(0.4);
    (cfg.bridge_speed * scale).max(5.0)
}

fn skirt_paths(first: &PlannedLayer, tx: f64, ty: f64, cfg: &PrinterConfig) -> Vec<Path> {
    if cfg.skirt_loops == 0 {
        return Vec::new();
    }
    let mut b = Bounds::empty();
    for ring in &first.reference {
        for p in ring {
            b.add(*p);
        }
    }
    if b.is_empty() {
        return Vec::new();
    }
    let mut loops = cfg.skirt_loops;
    if cfg.min_skirt_length > 0.0 {
        let mut total = 0.0;
        let mut n = loops;
        while total < cfg.min_skirt_length && n < 200 {
            let off = cfg.skirt_distance + (n as f64 + 0.5) * cfg.line_width;
            let per = 2.0 * ((b.max.x - b.min.x) + (b.max.y - b.min.y)) + 8.0 * off;
            total += per;
            if total < cfg.min_skirt_length {
                n += 1;
            }
        }
        loops = n.max(loops);
    }
    let mut out = Vec::with_capacity(loops);
    for i in 0..loops {
        let off = cfg.skirt_distance + (i as f64 + 0.5) * cfg.line_width;
        let (x0, y0) = (b.min.x + tx - off, b.min.y + ty - off);
        let (x1, y1) = (b.max.x + tx + off, b.max.y + ty + off);
        out.push(Path {
            pts: vec![
                V2::new(x0, y0),
                V2::new(x1, y0),
                V2::new(x1, y1),
                V2::new(x0, y1),
            ],
            width: vec![cfg.line_width; 4],
            closed: true,
            feature: Feature::Skirt,
            rank: -1,
            span: 0.0,
            flow: 1.0,
            overhang: 0.0,
            hole: false,
        });
    }
    out
}

fn hull(mut pts: Vec<V2>) -> Vec<V2> {
    pts.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
    });
    pts.dedup_by(|a, b| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: V2, a: V2, b: V2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<V2> = Vec::new();
    for p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0.0 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<V2> = Vec::new();
    for p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0.0 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Raster lines inside a convex polygon, one bead wide and `step` apart.
fn grow(poly: &[V2], reach: f64) -> Vec<V2> {
    if reach <= 0.0 || poly.len() < 3 {
        return poly.to_vec();
    }
    let cx = poly.iter().map(|p| p.x).sum::<f64>() / poly.len() as f64;
    let cy = poly.iter().map(|p| p.y).sum::<f64>() / poly.len() as f64;
    poly.iter()
        .map(|p| {
            let dx = p.x - cx;
            let dy = p.y - cy;
            let len = (dx * dx + dy * dy).sqrt().max(1e-9);
            V2::new(p.x + dx / len * reach, p.y + dy / len * reach)
        })
        .collect()
}

fn raster_fill(poly: &[V2], step: f64, width: f64, feature: Feature, rank: i32) -> Vec<Path> {
    if poly.len() < 3 || step <= 0.0 {
        return Vec::new();
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in poly {
        lo = lo.min(p.y);
        hi = hi.max(p.y);
    }
    let rows = ((hi - lo) / step).floor() as usize;
    let mut out = Vec::with_capacity(rows + 1);
    for row in 0..=rows {
        let y = lo + (row as f64 + 0.5) * step;
        if y > hi {
            break;
        }
        let mut xs: Vec<f64> = Vec::new();
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            if (a.y > y) == (b.y > y) {
                continue;
            }
            xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let start = *xs.first().unwrap_or(&0.0);
        let end = *xs.last().unwrap_or(&0.0);
        if end - start < width {
            continue;
        }
        out.push(Path {
            pts: vec![V2::new(start, y), V2::new(end, y)],
            width: vec![width; 2],
            closed: false,
            feature,
            rank,
            span: 0.0,
            flow: 1.0,
            overhang: 0.0,
            hole: false,
        });
    }
    out
}

/// Room the skirt or brim takes around the part, in millimetres.
fn skirt_margin(cfg: &PrinterConfig) -> f64 {
    let skirt = if cfg.skirt_loops > 0 {
        cfg.skirt_distance + cfg.skirt_loops as f64 * cfg.line_width
    } else {
        0.0
    };
    let brim = if brim_loops(cfg) > 0 {
        cfg.brim_gap + brim_loops(cfg) as f64 * cfg.line_width
    } else {
        0.0
    };
    skirt.max(brim)
}

fn placement(extent: ModelExtent, cfg: &PrinterConfig) -> (f64, f64) {
    let target = if cfg.origin_center {
        (0.0, 0.0)
    } else if cfg.bed_shape.len() >= 3 {
        bed_target(extent, cfg).unwrap_or((0.5 * cfg.bed_x, 0.5 * cfg.bed_y))
    } else {
        (0.5 * cfg.bed_x, 0.5 * cfg.bed_y)
    };
    (
        target.0 - 0.5 * (extent.xy.min.x + extent.xy.max.x),
        target.1 - 0.5 * (extent.xy.min.y + extent.xy.max.y),
    )
}

/// A centre that puts the part's footprint inside a bed that is not a rectangle.
///
/// The bounding box centre is tried first because it matches what a rectangular
/// bed does, then the outline's area centroid, which is where a round or L-shaped
/// bed expects a part to sit, then a five millimetre lattice over the outline for
/// the awkward case of a part that only fits off centre.
fn bed_target(extent: ModelExtent, cfg: &PrinterConfig) -> Option<(f64, f64)> {
    let margin = skirt_margin(cfg) + 1.0;
    let hw = 0.5 * extent.xy.width() + margin;
    let hd = 0.5 * extent.xy.height() + margin;
    let fits = |target: (f64, f64)| {
        [
            (target.0 - hw, target.1 - hd),
            (target.0 + hw, target.1 - hd),
            (target.0 + hw, target.1 + hd),
            (target.0 - hw, target.1 + hd),
        ]
        .iter()
        .all(|&(x, y)| cfg.bed_contains(x, y))
    };
    let shape = &cfg.bed_shape;
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut area = 0.0;
    for i in 0..shape.len() {
        let (x0, y0) = shape[i];
        let (x1, y1) = shape[(i + 1) % shape.len()];
        let cross = x0 * y1 - x1 * y0;
        area += cross;
        cx += (x0 + x1) * cross;
        cy += (y0 + y1) * cross;
    }
    let centroid = if area.abs() > 1e-9 {
        Some((cx / (3.0 * area), cy / (3.0 * area)))
    } else {
        None
    };
    let candidates = [Some((0.5 * cfg.bed_x, 0.5 * cfg.bed_y)), centroid];
    for candidate in candidates.into_iter().flatten() {
        if fits(candidate) {
            return Some(candidate);
        }
    }
    let mut y = 0.0;
    while y <= cfg.bed_y {
        let mut x = 0.0;
        while x <= cfg.bed_x {
            if fits((x, y)) {
                return Some((x, y));
            }
            x += 2.0;
        }
        y += 2.0;
    }
    None
}

fn check_fit(extent: ModelExtent, cfg: &PrinterConfig) -> Result<(), GcodeError> {
    let margin = skirt_margin(cfg);
    let width = extent.xy.width() + 2.0 * margin;
    let depth = extent.xy.height() + 2.0 * margin;
    if width > cfg.bed_x - 2.0 || depth > cfg.bed_y - 2.0 {
        return Err(GcodeError::DoesNotFit {
            width,
            depth,
            bed_x: cfg.bed_x,
            bed_y: cfg.bed_y,
        });
    }
    if cfg.bed_shape.len() >= 3 {
        let (tx, ty) = placement(extent, cfg);
        let low = (extent.xy.min.x + tx - margin, extent.xy.min.y + ty - margin);
        let high = (extent.xy.max.x + tx + margin, extent.xy.max.y + ty + margin);
        let corners = [
            (low.0, low.1),
            (high.0, low.1),
            (high.0, high.1),
            (low.0, high.1),
        ];
        if corners.iter().any(|&(x, y)| !cfg.bed_contains(x, y)) {
            return Err(GcodeError::OutsideBedShape {
                width,
                depth,
                outline: cfg.bed_shape_text(),
            });
        }
    }
    let height = extent.max_z - extent.min_z;
    if height > cfg.max_z {
        return Err(GcodeError::TooTall {
            height,
            max_z: cfg.max_z,
        });
    }
    Ok(())
}

struct Writer<'a> {
    cfg: &'a PrinterConfig,
    out: String,
    pos: V2,
    z: f64,
    e_abs: f64,
    retracted: bool,
    last_width: f64,
    last_f: f64,
    last_feature: Option<Feature>,
    previous_feature: Option<Feature>,
    fan_pwm: i64,
    fan_saved: Option<i64>,
    accel_saved: bool,
    accel: f64,
    last_dir: V2,
    no_hop: bool,
    drawn: Vec<(V2, V2)>,
    /// Footprint rings of this layer's overhanging beads, which curl as they cool.
    curled: Vec<Vec<V2>>,
    /// Volumetric rate of the last bead written, carried across layers so the
    /// slope limit has something to measure the layer's first bead against.
    last_rate: f64,
    /// Closed perimeter loops of the layer being written, as drawn.
    ///
    /// Travel routing needs the loops as loops: a bead to go round is a polygon,
    /// and a bag of segments cannot be walked.
    loops: Vec<Vec<V2>>,
    /// Widest bead drawn so far on this layer, in millimetres.
    ///
    /// A route clears the widest bead it might pass, because a narrow wall is not
    /// worth routing round but a wide one is a scar.
    layer_bead: f64,
    stats: GcodeStats,
    bbox_set: bool,
    object_label: Option<String>,
    labelled: bool,
    segs: Vec<crate::thumb::Segment>,
    spiral: Option<Spiral>,
    /// Bridge segments of the layer below, in machine coordinates.
    ///
    /// Solid infill printed over one of these is the case `over_bridge_speed`
    /// exists for: it has air under it, and the bridge under that air, so the
    /// bead has nothing solid to squash into.
    /// True while writing the first layer of the object on top of a raft.
    over_raft: bool,
    prev_bridges: Vec<(V2, V2)>,
    /// Bridges of the layer being written, moved into `prev_bridges` at its end.
    layer_bridges: Vec<(V2, V2)>,
}

/// A wall that climbs while it prints.
///
/// `z` is the height of the next vertex and `done` is how much of the loop has
/// already been laid, so the rise is spread over the path's own length rather
/// than over its point count: a long straight run must lift as much as a short
/// one cut into the same number of vertices.
struct Spiral {
    z: f64,
    from: f64,
    rise: f64,
    length: f64,
    done: f64,
}

impl Writer<'_> {
    /// True when a solid path lands on a bridge of the layer below.
    fn path_over_bridge(&self, path: &Path) -> bool {
        if self.prev_bridges.is_empty() || path.pts.is_empty() {
            return false;
        }
        if !matches!(path.feature, Feature::Solid | Feature::TopSolid) {
            return false;
        }
        let mid = path.pts[path.pts.len() / 2];
        self.prev_bridges
            .iter()
            .any(|(a, b)| hfw_kernel::seg_closest(mid, *a, *b).0 < self.layer_bead)
    }

    fn restore_fan(&mut self) {
        if let Some(pwm) = self.fan_saved.take() {
            let _ = writeln!(self.out, "M106 S{pwm}");
        }
    }

    /// Switches the fan for one path when the profile asks for it.
    ///
    /// The layer's own duty stays in `fan_pwm`; this borrows it for the duration
    /// of a single feature, which is how an ironing pass or an overhanging wall
    /// gets a different fan without disturbing the ramp around it.
    fn set_path_fan(&mut self, path: &Path) {
        if !self.cfg.dynamic_fan_speeds && self.cfg.ironing_fan_speed < 0.0 {
            return;
        }
        if path.feature == Feature::Bridge && !self.cfg.enable_overhang_bridge_fan {
            return;
        }
        if path.feature == Feature::Bridge && self.cfg.internal_bridge_fan_speed >= 0.0 {
            let pwm = (self.cfg.internal_bridge_fan_speed * 2.55).round() as i64;
            if pwm != self.fan_pwm {
                if self.fan_saved.is_none() {
                    self.fan_saved = Some(self.fan_pwm);
                }
                let _ = writeln!(self.out, "M106 S{pwm}");
                self.fan_pwm = pwm;
            }
            return;
        }
        let percent = match path.feature {
            Feature::Ironing if self.cfg.ironing_fan_speed >= 0.0 => self.cfg.ironing_fan_speed,
            Feature::SupportInterface if self.cfg.support_interface_fan_speed >= 0.0 => {
                self.cfg.support_interface_fan_speed
            }
            _ if self.cfg.dynamic_fan_speeds => match overhang_quarter(path.overhang) {
                Some(quarter) => self.cfg.overhang_fan_speeds[quarter],
                None => return,
            },
            _ => return,
        };
        if percent < 0.0 {
            return;
        }
        let pwm = (percent * 2.55).round() as i64;
        if pwm == self.fan_pwm {
            return;
        }
        if self.fan_saved.is_none() {
            self.fan_saved = Some(self.fan_pwm);
        }
        let _ = writeln!(self.out, "M106 S{pwm}");
        self.fan_pwm = pwm;
    }

    fn restore_accel(&mut self) {
        if self.accel_saved {
            self.accel_saved = false;
            let _ = writeln!(self.out, "RESTORE_VELOCITY_LIMIT");
        }
        self.accel = 0.0;
    }

    fn restore_limits(&mut self) {
        self.restore_fan();
        self.restore_accel();
    }

    /// Switches the machine to `accel` mm/s^2, or leaves it alone at 0.
    ///
    /// Klipper's limit is a saved/restored pair around the change; Marlin takes
    /// `M204 S`, a print-wide setting, so it is restored to `default_accel` when
    /// the profile has one and otherwise left at the last feature's value.
    fn apply_accel(&mut self, accel: f64) {
        if accel <= 0.0 || (accel - self.accel).abs() < 1.0 {
            return;
        }
        match self.cfg.flavor {
            Flavor::Klipper => {
                if !self.accel_saved {
                    self.accel_saved = true;
                    let _ = writeln!(self.out, "SAVE_VELOCITY_LIMIT");
                }
                let _ = writeln!(self.out, "SET_VELOCITY_LIMIT ACCEL={accel:.0}");
            }
            Flavor::Marlin => {
                let _ = writeln!(self.out, "M204 S{accel:.0}");
            }
        }
        self.accel = accel;
    }

    /// Rounds a coordinate to the profile's resolution grid.
    fn snap(&self, v: f64) -> f64 {
        let res = self.cfg.gcode_resolution;
        if res <= 0.0 {
            v
        } else {
            (v / res).round() * res
        }
    }

    /// Writes a `;` comment when the profile asks for comments.
    fn comment(&mut self, text: &str) {
        if self.cfg.gcode_comments {
            self.out.push_str(text);
            self.out.push('\n');
        }
    }

    fn touch(&mut self, p: V2) {
        if !self.bbox_set {
            self.stats.bbox_min = p;
            self.stats.bbox_max = p;
            self.bbox_set = true;
        } else {
            self.stats.bbox_min.x = self.stats.bbox_min.x.min(p.x);
            self.stats.bbox_min.y = self.stats.bbox_min.y.min(p.y);
            self.stats.bbox_max.x = self.stats.bbox_max.x.max(p.x);
            self.stats.bbox_max.y = self.stats.bbox_max.y.max(p.y);
        }
    }

    fn e_move(&mut self, delta: f64, speed: f64) {
        let speed = match feed_ceiling(self.cfg, Axis::E) {
            0.0 => speed,
            ceiling => speed.min(ceiling),
        };
        if self.cfg.relative_e {
            let _ = writeln!(self.out, "G1 E{:.5} F{:.0}", delta, speed * 60.0);
        } else {
            self.e_abs += delta;
            let _ = writeln!(self.out, "G1 E{:.5} F{:.0}", self.e_abs, speed * 60.0);
        }
        self.last_f = speed * 60.0;
    }

    fn retract(&mut self) {
        if self.cfg.firmware_retraction {
            if !self.retracted {
                self.out.push_str("G10\n");
                self.retracted = true;
                self.stats.retractions += 1;
                self.stats.est_seconds += self.cfg.retract_len / self.cfg.retract_speed;
            }
            return;
        }
        if self.cfg.retract_len > 0.0 && !self.retracted {
            self.e_move(-self.cfg.retract_len, self.cfg.retract_speed);
            self.retracted = true;
            self.stats.retractions += 1;
            self.stats.est_seconds += self.cfg.retract_len / self.cfg.retract_speed;
        }
    }

    fn unretract(&mut self) {
        if self.cfg.firmware_retraction {
            if self.retracted {
                self.out.push_str("G11\n");
                self.retracted = false;
                let distance = self.cfg.retract_len + self.cfg.retract_restart_extra;
                self.stats.est_seconds += distance / self.cfg.retract_speed;
            }
            return;
        }
        if self.retracted {
            let speed = if self.cfg.deretract_speed > 0.0 {
                self.cfg.deretract_speed
            } else {
                self.cfg.retract_speed
            };
            let distance = self.cfg.retract_len + self.cfg.retract_restart_extra;
            self.e_move(distance, speed);
            self.retracted = false;
            self.stats.est_seconds += distance / speed;
        }
    }

    /// Drags the nozzle back along the bead it just laid, without extruding.
    ///
    /// Wiping happens after the retraction, moving away from the seam rather
    /// than through it, so the strand that would otherwise be left as a blob at
    /// the start of the next feature is stretched thin over printed material.
    fn wipe(&mut self) {
        if !self.cfg.wipe || self.cfg.wipe_distance <= 0.0 {
            return;
        }
        if self.last_dir.len() < 1e-9 {
            return;
        }
        let to = self.pos + self.last_dir * -self.cfg.wipe_distance;
        let speed = match feed_ceiling(self.cfg, Axis::XY) {
            0.0 => self.cfg.travel_speed,
            ceiling => self.cfg.travel_speed.min(ceiling),
        };
        let _ = writeln!(
            self.out,
            "G1 X{:.3} Y{:.3} F{:.0}",
            self.snap(to.x),
            self.snap(to.y),
            speed * 60.0
        );
        self.last_f = speed * 60.0;
        self.stats.travel_mm += self.cfg.wipe_distance;
        self.stats.est_seconds += self.cfg.wipe_distance / speed;
        self.pos = to;
    }

    fn z_move(&mut self, z: f64) {
        let own = if self.cfg.travel_speed_z > 0.0 {
            self.cfg.travel_speed_z
        } else {
            10.0
        };
        let feed = match feed_ceiling(self.cfg, Axis::Z) {
            0.0 => own,
            ceiling => own.min(ceiling),
        };
        let _ = writeln!(self.out, "G1 Z{:.3} F{:.0}", z, feed * 60.0);
        self.last_f = feed * 60.0;
    }

    fn travel(&mut self, to: V2, threshold: f64) {
        let to = V2::new(self.snap(to.x), self.snap(to.y));
        let d = self.pos.dist(to);
        if d < 1e-9 {
            return;
        }
        let from = V2::new(self.snap(self.pos.x), self.snap(self.pos.y));
        let curled_obstacles: Vec<Vec<V2>>;
        let obstacles: &[Vec<V2>] =
            if self.cfg.avoid_crossing_curled_overhangs && !self.curled.is_empty() {
                curled_obstacles = self
                    .loops
                    .iter()
                    .cloned()
                    .chain(self.curled.iter().cloned())
                    .collect();
                &curled_obstacles
            } else {
                &self.loops
            };
        let avoids = self.cfg.avoid_crossing_perimeters || self.cfg.avoid_crossing_curled_overhangs;
        let detour = if avoids && !obstacles.is_empty() {
            route::detour(
                from,
                to,
                obstacles,
                self.layer_bead,
                self.cfg.avoid_crossing_perimeters_max_detour,
            )
        } else {
            None
        };
        let via: Vec<V2> = detour
            .as_ref()
            .map(|route| {
                route
                    .via
                    .iter()
                    .map(|p| V2::new(self.snap(p.x), self.snap(p.y)))
                    .collect()
            })
            .unwrap_or_default();
        let length = detour.as_ref().map_or(d, |route| route.length);
        if detour.is_none() && crosses(from, to, &self.drawn) {
            self.stats.travel_crossings += 1;
        }
        let crossed = crosses(from, to, &self.drawn);
        let over_curl = self.cfg.travel_lift_before_obstacle
            && self.curled.iter().any(|ring| ring_crosses(from, to, ring));
        let long = length > threshold || over_curl;
        let may_retract = !self.cfg.only_retract_when_crossing_perimeters || crossed;
        if long && may_retract {
            if self.cfg.retract_before_wipe > 0.0 {
                self.retract();
                self.wipe();
            } else {
                self.wipe();
                self.retract();
            }
        }
        let travel_accel = if d < 1.0 && self.cfg.travel_short_distance_acceleration > 0.0 {
            self.cfg.travel_short_distance_acceleration
        } else {
            self.cfg.travel_acceleration
        };
        let kept_accel = self.accel;
        if travel_accel > 0.0 && !self.no_hop {
            self.apply_accel(travel_accel);
        }
        let mut hopped = false;
        let mut lift = self.cfg.z_hop;
        if self.cfg.travel_max_lift > 0.0 {
            lift = lift.min(self.cfg.travel_max_lift);
        }
        let above_ok = self.cfg.retract_lift_above <= 0.0 || self.z <= self.cfg.retract_lift_above;
        let below_ok = self.cfg.retract_lift_below <= 0.0 || self.z >= self.cfg.retract_lift_below;
        let ramp = self.cfg.travel_ramping_lift && self.cfg.travel_slope > 0.0;
        if long && lift > 0.0 && !self.no_hop && above_ok && below_ok && !ramp {
            self.z_move(self.z + lift);
            hopped = true;
        }
        let travel_speed = match feed_ceiling(self.cfg, Axis::XY) {
            0.0 => self.cfg.travel_speed,
            ceiling => self.cfg.travel_speed.min(ceiling),
        };
        for point in &via {
            let _ = writeln!(
                self.out,
                "G0 X{:.3} Y{:.3} F{:.0}",
                point.x,
                point.y,
                travel_speed * 60.0
            );
        }
        let _ = writeln!(
            self.out,
            "G0 X{:.3} Y{:.3} F{:.0}",
            self.snap(to.x),
            self.snap(to.y),
            travel_speed * 60.0
        );
        self.last_f = travel_speed * 60.0;
        if travel_accel > 0.0 && !self.no_hop {
            if kept_accel > 0.0 {
                self.apply_accel(kept_accel);
            } else {
                self.restore_accel();
            }
        }
        if ramp && long && lift > 0.0 && !self.no_hop && above_ok && below_ok && via.is_empty() {
            let climb = (d / self.cfg.travel_slope).min(lift);
            let slope_feed = (travel_speed / self.cfg.travel_slope)
                .max(1.0)
                .min(travel_speed);
            let _ = writeln!(
                self.out,
                "G1 Z{:.3} F{:.0}",
                self.z + climb,
                slope_feed * 60.0
            );
            let _ = writeln!(
                self.out,
                "G1 X{:.3} Y{:.3} Z{:.3} F{:.0}",
                to.x,
                to.y,
                self.z,
                travel_speed * 60.0 * 0.5
            );
            self.last_f = travel_speed * 60.0 * 0.5;
            self.pos = to;
            self.stats.travel_mm += d;
            self.stats.est_seconds += d / travel_speed;
            self.no_hop = false;
            return;
        }
        if hopped {
            self.z_move(self.z);
        }
        self.no_hop = false;
        self.pos = to;
        self.stats.travel_mm += length;
        self.stats.est_seconds += length / travel_speed;
    }

    /// Moves a path's entry so the travel into it does not cut a printed perimeter.
    fn enter(&mut self, path: &mut Path, next: Option<V2>) {
        if !self.cfg.avoid_crossing_perimeters || self.drawn.is_empty() {
            return;
        }
        let cap = self.cfg.avoid_crossing_perimeters_max_detour;
        if let Some((at, reverse)) = choose_entry(path, self.pos, &self.drawn, cap, next) {
            if reverse {
                path.pts.reverse();
                path.width.reverse();
            } else if at > 0 {
                path.pts.rotate_left(at);
                path.width.rotate_left(at);
            }
        }
    }

    fn extrude(
        &mut self,
        path: &mut Path,
        height: f64,
        speed: f64,
        first_layer: bool,
        next: Option<V2>,
    ) {
        self.enter(path, next);
        let is_infill = |f: Option<Feature>| {
            matches!(
                f,
                Some(Feature::Solid) | Some(Feature::Sparse) | Some(Feature::Bridge)
            )
        };
        let threshold = if is_infill(self.previous_feature) && is_infill(Some(path.feature)) {
            self.cfg.infill_retract_min_travel
        } else {
            self.cfg.retract_min_travel
        };
        self.travel(path.pts[0], threshold);
        if is_perimeter(path.feature) {
            let n = path.pts.len();
            let count = if path.closed { n } else { n.saturating_sub(1) };
            let snapped: Vec<V2> = path
                .pts
                .iter()
                .map(|q| V2::new(self.snap(q.x), self.snap(q.y)))
                .collect();
            for i in 0..count {
                self.drawn.push((snapped[i], snapped[(i + 1) % n]));
            }
            if path.closed && n >= 3 {
                if path.feature == Feature::Bridge {
                    let n = path.pts.len();
                    let segments = if path.closed { n } else { n.saturating_sub(1) };
                    for i in 0..segments {
                        self.layer_bridges
                            .push((path.pts[i], path.pts[(i + 1) % n]));
                    }
                }
                self.loops.push(snapped);
            }
            for width in &path.width {
                if *width > self.layer_bead {
                    self.layer_bead = *width;
                }
            }
        }
        self.unretract();
        self.previous_feature = Some(path.feature);
        if !self.labelled {
            if let Some(name) = self.object_label.clone() {
                let _ = writeln!(self.out, ";OBJECT:{name}");
            }
            self.labelled = true;
        }
        if self.last_feature != Some(path.feature) {
            let label = format!(";TYPE:{}", path.feature.label());
            self.comment(&label);
            if path.feature == Feature::Bridge {
                if self.fan_saved.is_none() {
                    self.fan_saved = Some(self.fan_pwm);
                    let pwm = (self.cfg.bridge_fan_speed * 2.55).round() as i64;
                    let _ = writeln!(self.out, "M106 S{pwm}");
                }
            } else {
                self.restore_fan();
            }
            let accel = if self.over_raft && self.cfg.first_layer_acceleration_over_raft > 0.0 {
                self.cfg.first_layer_acceleration_over_raft
            } else {
                base_accel(path.feature, self.cfg, first_layer)
            };
            self.apply_accel(accel);
            self.set_path_fan(path);
            self.last_feature = Some(path.feature);
        }
        let (height, flow) = if path.feature == Feature::Bridge {
            (
                bridge_height(self.cfg, height),
                self.cfg.flow * bridge_flow_scale(self.cfg, path.span),
            )
        } else {
            (height, self.cfg.flow * path.flow)
        };
        let scarfed = scarf(path, self.cfg, self.z);
        let opened = match (&scarfed, path.closed, self.cfg.seam_gap) {
            (None, true, gap) if gap > 0.0 => open_by_gap(&path.pts, &path.width, gap),
            _ => None,
        };
        let (mut pts, mut widths, mut ramp, closed) = match (scarfed, opened) {
            (Some(s), _) => (s.pts, s.width, s.flow, true),
            (None, Some((p, wd))) => {
                let n = p.len();
                (p, wd, vec![1.0; n], false)
            }
            (None, None) => {
                let n = path.pts.len();
                (
                    path.pts.clone(),
                    path.width.clone(),
                    vec![1.0; n],
                    path.closed,
                )
            }
        };
        if closed && self.cfg.clockwise && hfw_kernel::ring_signed_area(&pts) > 0.0 {
            pts.reverse();
            widths.reverse();
            ramp.reverse();
        }
        for p in pts.iter_mut() {
            p.x = self.snap(p.x);
            p.y = self.snap(p.y);
        }
        let n = pts.len();
        if n < 2 {
            return;
        }
        let segments = if closed { n } else { n - 1 };
        let area = self.cfg.filament_area();
        let arcs = self.cfg.arc_fitting
            && is_perimeter(path.feature)
            && self.spiral.is_none()
            && ramp.iter().all(|f| (f - 1.0).abs() < 1e-9);
        if let Some(spiral) = self.spiral.as_mut() {
            spiral.done = 0.0;
            spiral.length = (0..segments)
                .map(|i| pts[i].dist(pts[(i + 1) % n]))
                .sum::<f64>()
                .max(1e-9);
            spiral.z = spiral.from;
        }
        let runs = if arcs {
            fit_runs(&pts, closed, self.cfg.arc_fitting_tolerance)
        } else {
            (0..segments).map(|i| Run::Line { end: i + 1 }).collect()
        };
        for run in runs {
            let (from, to, len, arc) = match run {
                Run::Line { end } => {
                    let front = pts[(end - 1) % n];
                    let back = pts[end % n];
                    (end - 1, end, front.dist(back), None)
                }
                Run::Arc {
                    start,
                    end,
                    centre,
                    clockwise,
                    length,
                } => (start, end, length, Some((centre, clockwise))),
            };
            if len < 1e-4 {
                continue;
            }
            let b = pts[to % n];
            self.last_dir = (b - pts[(to - 1) % n]).norm();
            let width = match arc {
                Some(_) => {
                    let mut sum = 0.0;
                    for k in from..=to {
                        sum += widths[k % n];
                    }
                    sum / (to - from + 1) as f64
                }
                None => 0.5 * (widths[from % n] + widths[to % n]),
            };
            let mut taper = 0.0;
            for k in from..=to {
                taper += ramp[k % n];
            }
            taper /= (to - from + 1) as f64;
            let width = width * taper;
            let volume = len * width * height * flow;
            let delta = if self.cfg.volumetric_e {
                volume
            } else {
                volume / area
            };
            if !self.cfg.thumbnails.is_empty() {
                self.segs.push(crate::thumb::Segment {
                    from: (pts[from % n].x, pts[from % n].y),
                    to: (b.x, b.y),
                    width,
                });
            }
            if (width - self.last_width).abs() > 0.02 {
                let note = format!(";WIDTH:{width:.3}");
                self.comment(&note);
                self.last_width = width;
            }
            let feed = speed * 60.0;
            let f_part = if (feed - self.last_f).abs() > 0.5 {
                format!(" F{feed:.0}")
            } else {
                String::new()
            };
            let e_value = if self.cfg.relative_e {
                format!("{delta:.5}")
            } else {
                self.e_abs += delta;
                format!("{:.5}", self.e_abs)
            };
            let spiral_z = self.spiral.as_mut().map(|spiral| {
                spiral.done += len;
                let frac = (spiral.done / spiral.length).clamp(0.0, 1.0);
                spiral.z = spiral.from + spiral.rise * frac;
                spiral.z
            });
            match arc {
                Some((centre, clockwise)) => {
                    let origin = pts[from % n];
                    let code = if clockwise { "G2" } else { "G3" };
                    let _ = writeln!(
                        self.out,
                        "{code} X{:.3} Y{:.3} I{:.3} J{:.3} E{e_value}{f_part}",
                        self.snap(b.x),
                        self.snap(b.y),
                        self.snap(centre.x - origin.x),
                        self.snap(centre.y - origin.y)
                    );
                }
                None => {
                    let b_x = self.snap(b.x);
                    let b_y = self.snap(b.y);
                    match spiral_z {
                        Some(z) => {
                            let _ = writeln!(
                                self.out,
                                "G1 X{b_x:.3} Y{b_y:.3} Z{z:.3} E{e_value}{f_part}"
                            );
                        }
                        None => {
                            let _ = writeln!(self.out, "G1 X{b_x:.3} Y{b_y:.3} E{e_value}{f_part}");
                        }
                    }
                }
            }
            self.last_f = feed;
            self.pos = b;
            for k in from..=to {
                self.touch(pts[k % n]);
            }
            self.stats.volume_mm3 += volume;
            self.stats.est_seconds += len / speed;
        }
        if let Some(spiral) = self.spiral.take() {
            self.z = spiral.from + spiral.rise;
        }
    }
}

/// Emits a complete G-code file for a planned job.
///
/// `object_name` is used only when the profile enables `EXCLUDE_OBJECT_DEFINE`;
/// it names the single printed object so the machine can cancel it.
pub fn generate(
    layers: &[PlannedLayer],
    cfg: &PrinterConfig,
    object_name: Option<&str>,
) -> Result<Gcode, GcodeError> {
    cfg.validate()?;
    if layers.is_empty() {
        return Err(GcodeError::NoLayers);
    }
    let extent = ModelExtent::of(layers);
    check_fit(extent, cfg)?;
    let (tx, ty) = placement(extent, cfg);
    let z0 = extent.min_z;
    let layer_count = layers.len();
    let mut w = Writer {
        cfg,
        out: String::new(),
        pos: if cfg.origin_center {
            V2::ZERO
        } else {
            V2::new(0.5 * cfg.bed_x, 0.5 * cfg.bed_y)
        },
        z: 0.0,
        e_abs: 0.0,
        retracted: true,
        last_width: -1.0,
        last_f: -1.0,
        last_feature: None,
        previous_feature: None,
        fan_pwm: 0,
        fan_saved: None,
        accel_saved: false,
        accel: 0.0,
        last_dir: V2::ZERO,
        no_hop: false,
        drawn: Vec::new(),
        curled: Vec::new(),
        last_rate: 0.0,
        loops: Vec::new(),
        layer_bead: 0.0,
        stats: GcodeStats::default(),
        bbox_set: false,
        object_label: object_name.filter(|_| cfg.label_objects).map(sanitise),
        labelled: false,
        spiral: None,
        segs: Vec::new(),
        over_raft: false,
        prev_bridges: Vec::new(),
        layer_bridges: Vec::new(),
    };
    let extra = [("{total_layers}", layer_count.to_string())];
    w.out.push_str(&cfg.fill(&cfg.start_gcode, &extra));
    w.out.push('\n');
    let _ = &extra;
    w.out.push_str("G90\n");
    w.out
        .push_str(if cfg.relative_e { "M83\n" } else { "M82\n" });
    w.out.push_str("G92 E0\n");
    if !cfg.fan_always_on {
        w.out.push_str("M107\n");
    }
    let bed_first = if cfg.first_layer_bed_temp > 0.0 {
        cfg.first_layer_bed_temp
    } else {
        cfg.bed_temp
    };
    let start = cfg.fill(&cfg.start_gcode, &extra);
    let start_waits = start.contains("M190") || start.contains("M191");
    if !cfg.start_filament_gcode.trim().is_empty() {
        w.out.push_str(&cfg.fill(&cfg.start_filament_gcode, &extra));
        w.out.push('\n');
    }
    if cfg.autoemit_temperatures && cfg.first_layer_bed_temp > 0.0 {
        let _ = writeln!(w.out, "M140 S{bed_first}");
        if !start_waits {
            let _ = writeln!(w.out, "M190 S{bed_first}");
        }
    }
    if cfg.chamber_temp > 0.0 {
        let _ = writeln!(w.out, "M141 S{}", cfg.chamber_temp);
    }
    if cfg.chamber_minimal_temperature > 0.0 {
        let _ = writeln!(w.out, "M191 S{}", cfg.chamber_minimal_temperature);
    }
    if cfg.stuck_filament_detection {
        let _ = writeln!(w.out, "M591 D0 S1");
    }
    if cfg.machine_limits_usage == "emit_to_gcode" {
        w.out.push_str(&machine_limits(cfg));
    }
    if cfg.volumetric_e {
        let _ = writeln!(w.out, "M200 D{}", cfg.filament_diameter);
    }
    if cfg.firmware_retraction && cfg.flavor == Flavor::Marlin {
        let _ = writeln!(
            w.out,
            "M207 S{} F{}",
            cfg.retract_len,
            cfg.retract_speed * 60.0
        );
    }
    if cfg.autoemit_temperatures {
        let _ = writeln!(w.out, "M109 S{}", cfg.first_nozzle_temp);
    }
    if cfg.pressure_advance > 0.0 {
        match cfg.flavor {
            Flavor::Klipper => {
                let _ = writeln!(
                    w.out,
                    "SET_PRESSURE_ADVANCE ADVANCE={}",
                    cfg.pressure_advance
                );
            }
            Flavor::Marlin => {
                let _ = writeln!(w.out, "M900 K{}", cfg.pressure_advance);
            }
        }
    }
    if cfg.flavor == Flavor::Klipper && cfg.emit_print_stats {
        let _ = writeln!(w.out, "SET_PRINT_STATS_INFO TOTAL_LAYER={layer_count}");
    }
    if cfg.exclude_object {
        if let Some(name) = object_name {
            let _ = writeln!(
                w.out,
                "EXCLUDE_OBJECT_DEFINE NAME={} CENTER={:.3},{:.3}",
                sanitise(name),
                0.5 * (extent.xy.min.x + extent.xy.max.x) + tx,
                0.5 * (extent.xy.min.y + extent.xy.max.y) + ty
            );
        }
    }
    let first = layers
        .iter()
        .find(|l| !l.paths.is_empty() || !l.reference.is_empty());
    let skirt = match first {
        Some(layer) => skirt_paths(layer, tx, ty, cfg),
        None => Vec::new(),
    };
    let footprint: Vec<V2> = layers
        .iter()
        .find(|l| !l.paths.is_empty())
        .map(|l| {
            l.paths
                .iter()
                .flat_map(|line| line.pts.iter().map(|p| V2::new(p.x + tx, p.y + ty)))
                .collect()
        })
        .unwrap_or_default();
    let hull_pts = hull(footprint);
    let raft_z = if cfg.raft_layers > 0 {
        cfg.raft_layers as f64 * cfg.first_layer_height + cfg.raft_contact_distance
    } else {
        0.0
    };
    let raft_poly = grow(&hull_pts, cfg.raft_first_layer_expansion);
    if cfg.raft_layers > 0 && raft_poly.len() >= 3 {
        for i in 0..cfg.raft_layers {
            let z = (i as f64 + 1.0) * cfg.first_layer_height + cfg.z_offset;
            let density = if i == 0 {
                cfg.raft_first_layer_density
            } else {
                0.5
            };
            let poly = if i == 0 {
                raft_poly.clone()
            } else {
                let extra = cfg.raft_expansion * i as f64;
                if extra > 0.0 {
                    grow(&hull_pts, extra)
                } else {
                    hull_pts.clone()
                }
            };
            let mut paths = raster_fill(
                &poly,
                cfg.line_width / density.max(0.05),
                cfg.line_width,
                Feature::Brim,
                0,
            );
            let _ = writeln!(
                w.out,
                ";LAYER_CHANGE\n;Z:{:.3}\n;HEIGHT:{:.3}\n;RAFT:{}",
                z, cfg.first_layer_height, i
            );
            w.z_move(z);
            w.z = z;
            w.no_hop = true;
            w.last_feature = None;
            w.previous_feature = None;
            let count = paths.len();
            for i in 0..paths.len() {
                let next = paths.get(i + 1).map(|q| q.pts[0]);
                w.extrude(
                    &mut paths[i],
                    cfg.first_layer_height,
                    cfg.first_layer_speed,
                    true,
                    next,
                );
            }
            if count > 0 {
                w.restore_limits();
            }
        }
    }
    let mut printed = 0usize;
    let z_shift = cfg.z_offset;
    for layer in layers {
        let height = layer.height();
        let z = layer.print_z - z0 + raft_z + z_shift;
        let z_bottom = layer.bottom_z - z0 + raft_z + z_shift;
        let first_layer = layer.first || printed == 0;
        w.over_raft = cfg.raft_layers > 0 && printed == cfg.raft_layers;
        let mut paths: Vec<Path> = layer
            .paths
            .iter()
            .filter_map(|l| path_from(l, tx, ty, cfg))
            .collect();
        mark_holes(&mut paths);
        if paths.is_empty() && !first_layer {
            continue;
        }
        if cfg.autoemit_temperatures
            && printed == 1
            && cfg.first_layer_bed_temp > 0.0
            && cfg.bed_temp != cfg.first_layer_bed_temp
        {
            let _ = writeln!(w.out, "M140 S{}", cfg.bed_temp);
        }
        let mut ordered: Vec<Path> = Vec::new();
        let mut sim = w.pos;
        let mut travel_total = 0.0;
        let with_skirt = printed < cfg.skirt_height && !skirt.is_empty();
        if with_skirt {
            let (o, end, travelled) = order(skirt.clone(), sim, Seam::Nearest);
            ordered.extend(o);
            sim = end;
            travel_total += travelled;
        }
        let shielded = match cfg.draft_shield.as_str() {
            "enabled" => true,
            "limited" => z <= cfg.draft_shield_height + 1e-9,
            _ => false,
        };
        if shielded {
            let off = cfg.skirt_distance;
            ordered.push(Path {
                pts: vec![
                    V2::new(extent.xy.min.x + tx - off, extent.xy.min.y + ty - off),
                    V2::new(extent.xy.max.x + tx + off, extent.xy.min.y + ty - off),
                    V2::new(extent.xy.max.x + tx + off, extent.xy.max.y + ty + off),
                    V2::new(extent.xy.min.x + tx - off, extent.xy.max.y + ty + off),
                ],
                width: vec![cfg.line_width; 4],
                closed: true,
                feature: Feature::Skirt,
                rank: -2,
                span: 0.0,
                flow: 1.0,
                overhang: 0.0,
                hole: false,
            });
        }
        paths.sort_by_key(|p| p.rank);
        let mut groups: Vec<(i32, Vec<Path>)> = Vec::new();
        for p in paths {
            match groups.last_mut() {
                Some((rank, group)) if *rank == p.rank => group.push(p),
                _ => groups.push((p.rank, vec![p])),
            }
        }
        let mut ordered: Vec<Path> = ordered;
        for (_, group) in groups {
            let (o, end, travelled) = order(group, sim, cfg.seam);
            ordered.extend(o);
            sim = end;
            travel_total += travelled;
        }
        w.prev_bridges = std::mem::take(&mut w.layer_bridges);
        let raft_speed = if w.over_raft {
            cfg.first_layer_speed_over_raft
        } else {
            0.0
        };
        let limit = volumetric_limit(cfg);
        let speeds: Vec<f64> = ordered
            .iter()
            .map(|p| {
                let widest = p.width.iter().cloned().fold(0.0, f64::max).max(1e-6);
                if p.feature == Feature::Bridge {
                    let section = widest
                        * bridge_height(cfg, height)
                        * cfg.flow
                        * bridge_flow_scale(cfg, p.span);
                    let speed = bridge_speed_span(cfg, p.span).min(cfg.max_speed);
                    if limit > 0.0 {
                        speed.min(limit / section)
                    } else {
                        speed
                    }
                } else {
                    let mut speed = path_speed(p.feature, cfg, first_layer, p.overhang);
                    if limit > 0.0 {
                        speed = speed.min(limit / (widest * height * cfg.flow * p.flow));
                    }
                    let owned = filament_speed_limit(p.feature, cfg);
                    if owned > 0.0 {
                        speed = speed.min(owned);
                    }
                    if cfg.over_bridge_speed > 0.0 && w.path_over_bridge(p) {
                        speed = speed.min(cfg.over_bridge_speed);
                    }
                    if raft_speed > 0.0 {
                        speed = speed.min(raft_speed);
                    }
                    speed
                }
            })
            .collect();
        let mut lengths = Vec::with_capacity(ordered.len());
        let mut extrude_time = 0.0;
        for (p, speed) in ordered.iter().zip(&speeds) {
            let n = p.pts.len();
            let segments = if p.closed { n } else { n.saturating_sub(1) };
            let mut len = 0.0;
            for i in 0..segments {
                len += p.pts[i].dist(p.pts[(i + 1) % n]);
            }
            lengths.push(len);
            extrude_time += len / speed;
        }
        let layer_time = extrude_time + travel_total / cfg.travel_speed;
        let target = if first_layer {
            0.0
        } else {
            let mut target = cfg.min_layer_time;
            if cfg.slow_down_layer_time > 0.0 && layer_time < cfg.slow_down_layer_time {
                target = target.max(cfg.slow_down_layer_time);
            }
            if layer_time >= target {
                0.0
            } else {
                target
            }
        };
        let mut speeds = speeds;
        let cooled = apply_cooling(&ordered, &mut speeds, &lengths, cfg, target);
        w.last_rate = limit_rate_slope(&ordered, &lengths, &mut speeds, cfg, height, w.last_rate);
        if !cfg.before_layer_gcode.trim().is_empty() {
            let per_layer = [
                ("{layer_num}", printed.to_string()),
                ("{layer_z}", format!("{z:.3}")),
                ("{total_layers}", layer_count.to_string()),
            ];
            w.out
                .push_str(&cfg.fill(&cfg.before_layer_gcode, &per_layer));
            w.out.push('\n');
        }
        let _ = writeln!(
            w.out,
            ";LAYER_CHANGE\n;Z:{:.3}\n;HEIGHT:{:.3}\n;LAYER:{}",
            z, height, printed
        );
        if layer.first && !first_layer && cfg.print_sequence == PrintSequence::ByObject {
            let text = cfg.between_objects_gcode.trim();
            if !text.is_empty() {
                let per = [
                    ("{layer_num}", printed.to_string()),
                    ("{total_layers}", layer_count.to_string()),
                ];
                w.out.push_str(&cfg.fill(text, &per));
                w.out.push('\n');
            }
        }
        let markers = [
            ("pause", &cfg.pause_heights, &cfg.pause_print_gcode),
            (
                "colour change",
                &cfg.colour_change_heights,
                &cfg.color_change_gcode,
            ),
        ];
        for (kind, heights, text) in markers {
            if !marker_fires(heights, z_bottom - z_shift + z_shift, z) {
                continue;
            }
            let text = text.trim();
            if text.is_empty() {
                let _ = writeln!(w.out, "; {kind} requested at Z{:.3} with no g-code set", z);
                continue;
            }
            let per = [
                ("{layer_z}", format!("{z:.3}")),
                ("{layer_num}", printed.to_string()),
                ("{total_layers}", layer_count.to_string()),
            ];
            w.out.push_str(&cfg.fill(text, &per));
            w.out.push('\n');
        }
        if cfg.remaining_times && printed > 0 {
            let done = (printed * 100) / layer_count.max(1);
            let total = w.stats.est_seconds * 100.0 / done as f64;
            let left = (total - w.stats.est_seconds).max(0.0);
            let _ = writeln!(w.out, "M73 P{done} R{:.0}", (left / 60.0).ceil().max(0.0));
        }
        if cfg.flavor == Flavor::Klipper && cfg.emit_print_stats {
            let _ = writeln!(w.out, "SET_PRINT_STATS_INFO CURRENT_LAYER={}", printed + 1);
        }
        if !cfg.relative_e {
            w.out.push_str("G92 E0\n");
            w.e_abs = 0.0;
        }
        if cfg.autoemit_temperatures
            && printed == 1
            && (cfg.first_nozzle_temp - cfg.nozzle_temp).abs() > 0.5
        {
            let _ = writeln!(w.out, "M104 S{}", cfg.nozzle_temp);
        }
        let desired_fan = fan_pwm_for(printed, cfg, cooled);
        if desired_fan != w.fan_pwm {
            w.fan_pwm = desired_fan;
            let _ = writeln!(w.out, "M106 S{desired_fan}");
        }
        if cfg.retract_layer_change {
            w.retract();
        }
        if layer.vase {
            if (w.z - z_bottom).abs() > 1e-6 {
                w.z_move(z_bottom);
                w.z = z_bottom;
            }
            w.spiral = Some(Spiral {
                z: z_bottom,
                from: z_bottom,
                rise: z - z_bottom,
                length: 0.0,
                done: 0.0,
            });
        } else {
            w.spiral = None;
            w.z_move(z);
            w.z = z;
        }
        w.stats.top_z = z.max(w.stats.top_z);
        w.last_feature = None;
        w.previous_feature = None;
        w.no_hop = true;
        w.drawn.clear();
        w.loops.clear();
        w.curled = curled_obstacles(&ordered);
        w.layer_bead = 0.0;
        for i in 0..ordered.len() {
            let next = ordered.get(i + 1).map(|q| q.pts[0]);
            w.extrude(&mut ordered[i], height, speeds[i], first_layer, next);
        }
        w.restore_limits();
        if !cfg.layer_gcode.trim().is_empty() {
            let per_layer = [
                ("{layer_num}", printed.to_string()),
                ("{layer_z}", format!("{z:.3}")),
                ("{total_layers}", layer_count.to_string()),
            ];
            w.out.push_str(&cfg.fill(&cfg.layer_gcode, &per_layer));
            w.out.push('\n');
        }
        printed += 1;
    }
    w.stats.layers = printed;
    w.retract();
    let top = (w.z + 10.0).min(cfg.max_z);
    w.z_move(top);
    if !cfg.end_filament_gcode.trim().is_empty() {
        w.out.push_str(&cfg.fill(&cfg.end_filament_gcode, &extra));
        w.out.push('\n');
    }
    w.out.push_str(&cfg.fill(&cfg.end_gcode, &extra));
    w.out.push('\n');
    if cfg.idle_temp > 0.0 {
        let _ = writeln!(w.out, "M104 S{:.0}", cfg.idle_temp);
    }
    let area = cfg.filament_area();
    w.stats.filament_mm = w.stats.volume_mm3 / area;
    w.stats.filament_g = w.stats.volume_mm3 / 1000.0 * cfg.filament_density;
    let stats = w.stats.clone();
    let mut head = String::new();
    let _ = writeln!(
        head,
        "; generated by hfw (clean-room Rust engine) - ALPHA, verify before printing"
    );
    let _ = writeln!(head, "; printer = {}", cfg.name);
    let _ = writeln!(head, "; layers = {}", stats.layers);
    let _ = writeln!(
        head,
        "; layer_height = {} first_layer_height = {}",
        cfg.layer_height, cfg.first_layer_height
    );
    let _ = writeln!(
        head,
        "; line_width = {} walls = {} infill = {} {}",
        cfg.line_width, cfg.walls, cfg.infill_density, cfg.infill_pattern
    );
    let _ = writeln!(
        head,
        "; nozzle_temp = {} bed_temp = {}",
        cfg.nozzle_temp, cfg.bed_temp
    );
    let _ = writeln!(
        head,
        "; estimated printing time = {:.0} s",
        stats.est_seconds
    );
    let _ = writeln!(
        head,
        "; filament used = {:.1} mm ({:.2} g, {:.0} mm3)",
        stats.filament_mm, stats.filament_g, stats.volume_mm3
    );
    for (label, value) in metadata_lines(cfg) {
        let _ = writeln!(head, "; {label} = {value}");
    }
    if !cfg.thumbnails.is_empty() {
        let specs: Vec<crate::thumb::Spec> = cfg
            .thumbnails
            .iter()
            .map(|t| crate::thumb::Spec {
                width: t.width,
                height: t.height,
                format: if t.png {
                    crate::thumb::Format::Png
                } else {
                    crate::thumb::Format::Qoi
                },
            })
            .collect();
        let margin = 1.0;
        let extent = (
            (stats.bbox_min.x - margin).min(stats.bbox_max.x),
            (stats.bbox_min.y - margin).min(stats.bbox_max.y),
            stats.bbox_max.x.max(stats.bbox_min.x) + margin,
            stats.bbox_max.y.max(stats.bbox_min.y) + margin,
        );
        head.push_str(&crate::thumb::render(&w.segs, &specs, extent));
    }
    let substituted = substitute(&format!("{head}{}", w.out), cfg)?;
    Ok(Gcode {
        text: substituted,
        stats,
    })
}

/// The identity settings that reach the file as header comments.
///
/// These change no motion, and that is the point: the file records what it was
/// sliced for, which is how a plate two weeks later traces back to a filament
/// and a machine. A profile that leaves one empty writes no line for it.
fn metadata_lines(cfg: &PrinterConfig) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let pairs = [
        ("printer_model", &cfg.printer_model),
        ("printer_variant", &cfg.printer_variant),
        ("printer_vendor", &cfg.printer_vendor),
        ("printer_notes", &cfg.printer_notes),
        ("notes", &cfg.notes),
        ("filament_type", &cfg.filament_type),
        ("filament_vendor", &cfg.filament_vendor),
        ("filament_colour", &cfg.filament_colour),
        ("filament_notes", &cfg.filament_notes),
        ("custom_parameters_print", &cfg.custom_parameters_print),
        (
            "custom_parameters_filament",
            &cfg.custom_parameters_filament,
        ),
        ("custom_parameters_printer", &cfg.custom_parameters_printer),
        ("template_custom_gcode", &cfg.template_custom_gcode),
        ("default_print", &cfg.default_print),
        ("default_material", &cfg.default_material),
        ("default_tool_print", &cfg.default_tool_print),
    ];
    for (label, value) in pairs {
        let value = value.trim();
        if !value.is_empty() {
            out.push((label, value.replace('\n', " ")));
        }
    }
    if cfg.filament_soluble {
        out.push(("filament_soluble", "1".to_owned()));
    }
    if cfg.filament_abrasive {
        out.push(("filament_abrasive", "1".to_owned()));
    }
    if cfg.silent_mode {
        out.push(("silent_mode", "1".to_owned()));
    }
    out
}

/// Applies `gcode_substitutions` to the finished text.
///
/// The dialect writes four entries per pass: what to find, what to put there,
/// flag letters and a note. `r` marks a regular expression (this always uses
/// one, which is why a literal pattern needs no escape beyond what regex
/// syntax already means), `i` ignores case, `w` requires whole words, `s` lets
/// `.` cross newlines. A pass that fails to compile is an error rather than a
/// silent no-op: a substitution a profile relies on going missing is a print
/// that runs the wrong commands.
fn substitute(text: &str, cfg: &PrinterConfig) -> Result<String, GcodeError> {
    if cfg.gcode_substitutions.is_empty() {
        return Ok(text.to_owned());
    }
    let mut out = text.to_owned();
    for pass in &cfg.gcode_substitutions {
        let mut pattern = if pass.flags.contains('r') {
            pass.pattern.clone()
        } else {
            regex::escape(&pass.pattern)
        };
        if pass.flags.contains('w') {
            pattern = format!("\\b{pattern}\\b");
        }
        let mut builder = regex::RegexBuilder::new(&pattern);
        builder.case_insensitive(pass.flags.contains('i'));
        builder.dot_matches_new_line(pass.flags.contains('s'));
        let regex = builder
            .build()
            .map_err(|error| GcodeError::BadSubstitution {
                pattern: pass.pattern.clone(),
                reason: error.to_string(),
            })?;
        let replaced = regex.replace_all(&out, pass.format.as_str()).into_owned();
        let note = pass.note.trim();
        let marker = if note.is_empty() {
            format!("; substituted {} -> {}", pass.pattern, pass.format)
        } else {
            format!("; substituted {} -> {} ({note})", pass.pattern, pass.format)
        };
        out = format!("{marker}\n{replaced}");
    }
    Ok(out)
}

/// True when a requested height falls inside the layer being emitted.
fn marker_fires(heights: &[f64], z_bottom: f64, z: f64) -> bool {
    heights
        .iter()
        .any(|marker| *marker > z_bottom + 1e-9 && *marker <= z + 1e-9)
}

fn sanitise(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("HFW_MODEL");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loop_of(pts: &[(f64, f64)]) -> Path {
        Path {
            pts: pts.iter().map(|&(x, y)| V2::new(x, y)).collect(),
            width: vec![0.4; pts.len()],
            closed: true,
            feature: Feature::Inner,
            rank: 10,
            span: 0.0,
            flow: 1.0,
            overhang: 0.0,
            hole: false,
        }
    }

    fn line_of(pts: &[(f64, f64)]) -> Path {
        let mut path = loop_of(pts);
        path.closed = false;
        path
    }

    fn wall() -> Vec<(V2, V2)> {
        let a = V2::new(3.9, -1.0);
        let b = V2::new(4.1, -1.0);
        let c = V2::new(4.1, 1.0);
        let d = V2::new(3.9, 1.0);
        vec![(a, b), (b, c), (c, d), (d, a)]
    }

    fn origin() -> V2 {
        V2::new(0.0, 0.0)
    }

    #[test]
    fn a_slope_limit_solves_the_cap_against_the_widest_section() {
        let cfg = PrinterConfig {
            max_volumetric_rate_slope_positive: 1.0,
            max_volumetric_rate_slope_negative: 1.0,
            flow: 1.0,
            ..PrinterConfig::default()
        };
        let height = 0.2;
        let speed = 25.0;

        let uniform = line_of(&[(0.0, 0.0), (10.0, 0.0)]);
        let mut widths = vec![0.4; uniform.pts.len()];
        widths[0] = 0.3;
        widths[1] = 0.6;
        let varying = Path {
            width: widths,
            ..line_of(&[(0.0, 0.0), (10.0, 0.0)])
        };

        let slow = [0.4 * height * speed];
        let mut speeds = vec![speed, speed];
        let last = limit_rate_slope(
            &[uniform.clone(), varying.clone()],
            &[10.0, 10.0],
            &mut speeds,
            &cfg,
            height,
            slow[0],
        );
        assert!(
            speeds[1] < speed,
            "the widening bead was left at {} mm/s",
            speeds[1]
        );
        let widest = 0.6 * height * speeds[1];
        let allowed = slow[0] + 1.0 * 10.0 / speeds[1];
        assert!(
            widest <= allowed + 1e-6,
            "the bead's widest section still steps the rate past the cap"
        );
        assert!(last > 0.0);

        let mut flat = vec![speed, speed];
        let mut widths_uniform = varying.clone();
        widths_uniform.width = vec![0.4; 2];
        limit_rate_slope(
            &[uniform, widths_uniform],
            &[10.0, 10.0],
            &mut flat,
            &cfg,
            height,
            slow[0],
        );
        assert!(
            flat[1] > speeds[1],
            "a uniform bead must not be slowed as far as a widening one"
        );
    }

    #[test]
    fn an_object_rank_keeps_objects_in_block_order() {
        assert_eq!(object_rank(0, -3), -3);
        assert_eq!(object_rank(1, -3), 997);
        assert!(
            object_rank(1, -3) > object_rank(0, 43),
            "a later object's support must print after the previous object's infill"
        );
        assert_eq!(object_rank(2, 43), 2043);
    }

    #[test]
    fn a_travel_that_only_touches_a_wall_is_not_a_crossing() {
        let seg = [(V2::new(5.0, -5.0), V2::new(5.0, 5.0))];
        assert!(crosses(origin(), V2::new(10.0, 0.0), &seg));
        assert!(
            !crosses(V2::new(5.0, 0.0), V2::new(10.0, 0.0), &seg),
            "a move that starts on the wall it was clipped by is not a crossing"
        );
        assert!(!crosses(V2::new(0.0, 20.0), V2::new(10.0, 20.0), &seg));
    }

    #[test]
    fn the_loop_entry_that_avoids_a_printed_wall_wins() {
        let round = loop_of(&[(5.0, 0.0), (5.0, 10.0), (-6.0, 10.0), (-6.0, 0.0)]);
        assert_eq!(
            choose_entry(&round, origin(), &wall(), 0.0, None),
            Some((3, false))
        );
        assert_eq!(
            choose_entry(&round, origin(), &[], 0.0, None),
            None,
            "with nothing printed there is nothing to avoid"
        );
    }

    #[test]
    fn the_detour_cap_keeps_the_nearest_entry() {
        let round = loop_of(&[(5.0, 0.0), (5.0, 10.0), (-6.0, 10.0), (-6.0, 0.0)]);
        assert_eq!(
            choose_entry(&round, origin(), &wall(), 0.5, None),
            None,
            "the only free entry is a whole millimetre further out"
        );
    }

    #[test]
    fn a_free_entry_that_sends_the_next_travel_through_a_wall_is_passed_over() {
        let round = loop_of(&[(5.0, 0.0), (5.0, 10.0), (-6.0, 10.0), (-6.0, 0.0)]);
        let drawn = wall();
        assert_eq!(
            choose_entry(&round, origin(), &drawn, 0.0, Some(V2::new(5.0, 0.0))),
            Some((1, false)),
            "entering at (-6,0) is free on the way in but its exit runs straight \
             through the wall, so the far corner wins instead"
        );
    }

    #[test]
    fn an_open_line_is_flipped_when_its_near_end_crosses() {
        let line = line_of(&[(6.0, 0.0), (6.0, 5.0)]);
        assert_eq!(
            choose_entry(&line, origin(), &wall(), 0.0, None),
            Some((0, true)),
            "the near end sits behind the wall, so the line prints towards the nozzle"
        );
        assert_eq!(
            choose_entry(&line, origin(), &wall(), 0.5, None),
            None,
            "flipping adds 1.8 mm, which a half-millimetre cap refuses"
        );
        assert_eq!(
            choose_entry(&line, origin(), &[], 0.0, None),
            None,
            "the near end is free once nothing is printed"
        );
    }
}
