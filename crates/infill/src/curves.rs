//! Curved sparse fills: the gyroid surface and the 3D honeycomb.
//!
//! Straight-line patterns are families of parallel lines, and the parent module
//! clips them by solving each line against the outline. These two are not
//! families — they are periodic curves sweeping across both axes — so they are
//! generated as polylines in the pattern's own periodic coordinates and clipped
//! point by point, bisecting each crossing onto the boundary the way the line
//! family does.
//!
//! Both are ports of the PrusaSlicer fills of the same name (`FillGyroid.cpp`
//! and `Fill3DHoneycomb.cpp` in the vendored engine), because reproducing the
//! pattern people have already printed is worth more than inventing a similar
//! one. The periodic coordinates are radians: one radian of pattern space is
//! `distance` millimetres, where `distance = spacing / 2.44 / density` for the
//! gyroid and `spacing / density` for the honeycomb.

use hfw_kernel::{SegIndex, V2};
use hfw_walls::field::Grid;
use hfw_walls::{Kind, Line};

/// Weight correction applied to the gyroid's spacing, as upstream.
///
/// The surface crosses a layer plane more often than a line family would at the
/// same nominal spacing, so without it the same density deposits more plastic.
/// The number is upstream's measurement, not a derivation.
pub(crate) const GYROID_DENSITY_ADJUST: f64 = 2.44;
/// Ceiling on the chord tolerance while tracing a wave, in millimetres.
const GYROID_TOLERANCE: f64 = 0.2;
/// Period of the honeycomb's z modulation, in modules.
const HONEYCOMB_Z_PERIOD: f64 = std::f64::consts::SQRT_2;

/// The region a curved pattern must be clipped to.
pub(crate) struct Clip<'a> {
    /// Boundary index of the layer's outline.
    pub index: &'a SegIndex,
    /// Radius inside which the shell already covers the layer.
    pub thr: f64,
    /// Skin mask grid.
    pub grid: &'a Grid,
    /// Cells already covered by skin or by air, so sparse fill stays out.
    pub excluded: &'a [u64],
    /// Sampling step along a curve when looking for a crossing.
    pub step: f64,
    /// Longest tie-in to the perimeter, in millimetres.
    pub anchor: f64,
    /// Bead width.
    pub width: f64,
}

impl Clip<'_> {
    fn valid(&self, p: V2) -> bool {
        if self.index.nearest(p, self.thr).is_some() {
            return false;
        }
        let ix = ((p.x - self.grid.origin.x) / self.grid.h).floor() as i64;
        let iy = ((p.y - self.grid.origin.y) / self.grid.h).floor() as i64;
        if ix < 0 || iy < 0 || ix >= self.grid.nx as i64 || iy >= self.grid.ny as i64 {
            return false;
        }
        let id = iy as usize * self.grid.nx + ix as usize;
        (self.excluded[id / 64] >> (id % 64)) & 1 == 0
    }

    /// How far an end of the path may reach into the shell to tie itself down.
    fn reach(&self, p: V2, dir: V2) -> f64 {
        if self.anchor <= 0.0 {
            return 0.0;
        }
        match self.index.nearest(p, self.thr + 1e-3) {
            Some(hit) => {
                let nb = (p - hit.foot).norm();
                let cs = dir.dot(nb).abs().max(0.2);
                let tn = (1.0 - cs * cs).max(0.0).sqrt() / cs;
                (0.5 * self.width * tn)
                    .min(2.0 * self.width)
                    .min(self.anchor)
            }
            None => 0.0,
        }
    }

    /// Cuts one curve into the runs that land on sparse infill.
    fn emit(&self, path: &[V2], runs: &mut Vec<Vec<V2>>) {
        if path.len() < 2 {
            return;
        }
        let mut run: Vec<V2> = Vec::new();
        let mut previous = path[0];
        let mut was_valid = self.valid(previous);
        if was_valid {
            run.push(previous);
        }
        for &point in &path[1..] {
            let span = previous.dist(point);
            let steps = ((span / self.step).ceil() as usize).max(1);
            for j in 1..=steps {
                let here = previous + (point - previous) * (j as f64 / steps as f64);
                let now_valid = self.valid(here);
                if now_valid != was_valid {
                    let (mut lo, mut hi) = (0.0f64, 1.0f64);
                    for _ in 0..26 {
                        let mid = 0.5 * (lo + hi);
                        if self.valid(previous + (here - previous) * mid) == was_valid {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    run.push(previous + (here - previous) * (0.5 * (lo + hi)));
                    if was_valid {
                        runs.push(std::mem::take(&mut run));
                    }
                } else if now_valid {
                    run.push(here);
                }
                was_valid = now_valid;
            }
            previous = point;
        }
        if was_valid {
            run.push(previous);
            runs.push(run);
        }
    }
}

/// Collects the runs of every curve, clipped and finished.
pub(crate) fn curved(clip: &Clip, curves: &[Vec<V2>], min_len: f64) -> Vec<Line> {
    let mut runs = Vec::new();
    for curve in curves {
        clip.emit(curve, &mut runs);
    }
    let mut out = Vec::new();
    for mut pts in runs {
        let mut total = 0.0;
        for pair in pts.windows(2) {
            total += pair[0].dist(pair[1]);
        }
        if total < min_len || pts.len() < 2 {
            continue;
        }
        let last = pts.len() - 1;
        let head_dir = (pts[0] - pts[1]).norm();
        let tail_dir = (pts[last] - pts[last - 1]).norm();
        let head = clip.reach(pts[0], head_dir);
        let tail = clip.reach(pts[last], tail_dir);
        if head > 0.0 {
            pts[0] = pts[0] + head_dir * head;
        }
        if tail > 0.0 {
            pts[last] = pts[last] + tail_dir * tail;
        }
        let n = pts.len();
        out.push(Line {
            pts,
            width: vec![clip.width; n],
            closed: false,
            kind: Kind::Infill,
            flow: 1.0,
            overhang: 0.0,
            object: 0,
        });
    }
    out
}

/// A wave's lateral position at `x`, in radians.
fn wave_height(x: f64, z_sin: f64, z_cos: f64, vertical: bool, flip: bool) -> f64 {
    use std::f64::consts::PI;
    if vertical {
        let phase = if z_cos < 0.0 { PI } else { 0.0 } + PI;
        let a = (x + phase).sin();
        let b = -z_cos;
        let res = z_sin * (x + phase + if flip { PI } else { 0.0 }).cos();
        let r = (a * a + b * b).sqrt();
        (a / r).asin() + (res / r).asin() + PI
    } else {
        let phase = if z_sin < 0.0 { PI } else { 0.0 };
        let a = (x + phase).cos();
        let b = -z_sin;
        let res = z_cos * (x + phase + if flip { 0.0 } else { PI }).sin();
        let r = (a * a + b * b).sqrt();
        (a / r).asin() + (res / r).asin() + 0.5 * PI
    }
}

/// One period of a wave, refined until no chord is further than `tolerance`
/// from the true curve.
fn one_period(
    width: f64,
    z_cos: f64,
    z_sin: f64,
    vertical: bool,
    flip: bool,
    tolerance: f64,
) -> Vec<V2> {
    use std::f64::consts::{FRAC_PI_2, TAU};
    let limit = TAU.min(width);
    let mut points = Vec::new();
    let mut x = 0.0;
    while x < limit {
        points.push(V2::new(x, wave_height(x, z_sin, z_cos, vertical, flip)));
        x += FRAC_PI_2;
    }
    points.push(V2::new(
        limit,
        wave_height(limit, z_sin, z_cos, vertical, flip),
    ));
    let limit2 = tolerance * tolerance;
    loop {
        let size = points.len();
        let mut extra = Vec::new();
        for i in 1..size {
            let (lp, rp) = (points[i - 1], points[i]);
            let mid_x = lp.x + (rp.x - lp.x) / 2.0;
            let mid = V2::new(mid_x, wave_height(mid_x, z_sin, z_cos, vertical, flip));
            if (mid - lp).cross(mid - rp).abs() > limit2 {
                extra.push(mid);
            }
        }
        if extra.is_empty() {
            break;
        }
        points.extend(extra);
        points.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
    points
}

/// Tiles one period out to `width`, offsets it to `y0`, clamps it to the layer
/// and converts radians to millimetres.
#[allow(clippy::too_many_arguments)]
fn wave(
    period: &[V2],
    width: f64,
    height: f64,
    y0: f64,
    distance: f64,
    z_sin: f64,
    z_cos: f64,
    vertical: bool,
    flip: bool,
) -> Vec<V2> {
    let mut points = period.to_vec();
    let span = points.last().map_or(0.0, |p: &V2| p.x);
    if (span - width).abs() > 1e-12 {
        let n = points.len() - 1;
        points.pop();
        loop {
            let next = points[points.len() - n] + V2::new(span, 0.0);
            let reached = next.x;
            points.push(next);
            if reached >= width - 1e-9 {
                break;
            }
        }
        points.push(V2::new(
            width,
            wave_height(width, z_sin, z_cos, vertical, flip),
        ));
    }
    let mut out = Vec::with_capacity(points.len());
    for p in points {
        let y = (p.y + y0).clamp(0.0, height);
        let (x, y) = if vertical { (y, p.x) } else { (p.x, y) };
        out.push(V2::new(x * distance, y * distance));
    }
    out
}

/// The gyroid waves covering one layer, in millimetres from `origin`.
///
/// `spacing` is the infill spacing at this density and `z` the layer's print
/// height: the pattern's phase follows `z`, which is what makes the fill
/// interlock between layers instead of standing in vertical tubes.
pub(crate) fn gyroid(
    width: f64,
    height: f64,
    spacing: f64,
    density: f64,
    z: f64,
    origin: V2,
) -> Vec<Vec<V2>> {
    use std::f64::consts::{PI, TAU};
    let density_adjusted = (density * GYROID_DENSITY_ADJUST).max(1e-9);
    let distance = spacing / density_adjusted;
    let (z_sin, z_cos) = (z / distance).sin_cos();
    let mut w = (width / distance).ceil() + 1.0;
    let mut h = (height / distance).ceil() + 1.0;
    let vertical = z_sin.abs() <= z_cos.abs();
    let mut lower = 0.0;
    let mut upper = h;
    let mut flip = true;
    if vertical {
        flip = false;
        lower = -PI;
        upper = w - 0.5 * PI;
        std::mem::swap(&mut w, &mut h);
    }
    let tolerance = (spacing / 2.0).min(GYROID_TOLERANCE) / distance;
    let odd = one_period(w, z_cos, z_sin, vertical, flip, tolerance);
    let even = one_period(w, z_cos, z_sin, vertical, !flip, tolerance);
    let mut out = Vec::new();
    let mut y0 = lower;
    while y0 < upper + 1e-9 {
        out.push(wave(&odd, w, h, y0, distance, z_sin, z_cos, vertical, flip));
        y0 += PI;
        if y0 < upper + 1e-9 {
            out.push(wave(
                &even, w, h, y0, distance, z_sin, z_cos, vertical, !flip,
            ));
        }
        y0 += PI;
    }
    let _ = TAU;
    for curve in &mut out {
        for p in curve.iter_mut() {
            *p = *p + origin;
        }
    }
    out
}

/// Points along one zig-zag strand: `base` moves forward, `offset` spreads it
/// sideways.
fn along(offset: f64, base: f64, length: usize) -> Vec<f64> {
    let half = (offset / 2.0).abs();
    let mut out = vec![base - half];
    for i in 0..length {
        out.push(base + i as f64 + half);
        out.push(base + i as f64 + 1.0 - half);
    }
    out.push(base + length as f64 + half);
    out
}

/// Points across one zig-zag strand, alternating sides.
fn across(offset: f64, base: usize, length: usize) -> Vec<f64> {
    let half = offset / 2.0;
    let mut side = 2.0 * (base & 1) as f64 - 1.0;
    let mut out = vec![base as f64 - half * side];
    for i in 0..length {
        side = 2.0 * ((i + base) & 1) as f64 - 1.0;
        out.push(base as f64 + half * side);
        out.push(base as f64 + half * side);
    }
    out.push(base as f64 - half * side);
    out
}

/// The 3D honeycomb strands covering one layer, in millimetres from `origin`.
///
/// Each layer is a slice through a stack of truncated octahedra: the slice is a
/// row of zig-zags, and the sideways spread of those zig-zags follows `z` as a
/// triangle wave, so the strands weave into the layer below instead of stacking.
/// Even layers run the strands one way and odd layers the other, which is where
/// the honeycomb comes from when you look at the part from the side.
pub(crate) fn honeycomb3d(
    width: f64,
    height: f64,
    spacing: f64,
    density: f64,
    z: f64,
    layer: usize,
    origin: V2,
) -> Vec<Vec<V2>> {
    let distance = spacing / (density.max(1e-9));
    let normalised_z = z / distance;
    let wave = ((normalised_z % HONEYCOMB_Z_PERIOD) - 0.5 * HONEYCOMB_Z_PERIOD).abs()
        / HONEYCOMB_Z_PERIOD
        * 4.0
        - 1.0;
    let offset = wave * 0.5;
    let columns = (width / distance).ceil().max(1.0) as usize + 1;
    let rows = (height / distance).ceil().max(1.0) as usize + 1;
    let mut out = Vec::new();
    let clip_to_grid = |pts: &mut Vec<V2>| {
        for p in pts.iter_mut() {
            p.x = p.x.clamp(0.0, columns as f64);
            p.y = p.y.clamp(0.0, rows as f64);
            *p = V2::new(p.x * distance + origin.x, p.y * distance + origin.y);
        }
    };
    if layer % 2 == 0 {
        for x in 0..=columns {
            let side = across(offset, x, rows);
            let along_len = along(offset, 0.0, rows);
            let mut pts: Vec<V2> = side
                .iter()
                .zip(along_len.iter())
                .map(|(a, b)| V2::new(*a, *b))
                .collect();
            if x % 2 == 1 {
                pts.reverse();
            }
            clip_to_grid(&mut pts);
            out.push(pts);
        }
    } else {
        for y in 0..=rows {
            let along_len = along(offset, 0.0, columns);
            let side = across(offset, y, columns);
            let mut pts: Vec<V2> = along_len
                .iter()
                .zip(side.iter())
                .map(|(a, b)| V2::new(*a, *b))
                .collect();
            if y % 2 == 1 {
                pts.reverse();
            }
            clip_to_grid(&mut pts);
            out.push(pts);
        }
    }
    out
}

/// Clips a polyline to a packed mask, keeping only the runs inside it.
///
/// The three plane-path skins are single continuous curves that fill whatever
/// box contains them, so they are clipped against the skin mask rather than
/// against the outline: the mask already carries the shell clearance and the
/// top/bottom classification, and a curve that ignored it would print through
/// the perimeters. Crossings are bisected the same way the line families bisect
/// theirs, to a twentieth of a cell.
pub(crate) fn clip_to_mask(grid: &Grid, mask: &[u64], path: &[V2], min_len: f64) -> Vec<Vec<V2>> {
    let set = |p: V2| -> bool {
        let ix = ((p.x - grid.origin.x) / grid.h).floor();
        let iy = ((p.y - grid.origin.y) / grid.h).floor();
        if ix < 0.0 || iy < 0.0 || ix >= grid.nx as f64 || iy >= grid.ny as f64 {
            return false;
        }
        let id = iy as usize * grid.nx + ix as usize;
        (mask[id / 64] >> (id % 64)) & 1 == 1
    };
    let mut out = Vec::new();
    let mut run: Vec<V2> = Vec::new();
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (va, vb) = (set(a), set(b));
        if va && run.is_empty() {
            run.push(a);
        }
        if va != vb {
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for _ in 0..10 {
                let mid = 0.5 * (lo + hi);
                let p = a + (b - a) * mid;
                if set(p) == va {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let edge = a + (b - a) * (0.5 * (lo + hi));
            if va {
                run.push(edge);
                if run.len() >= 2 && ring_length(&run) >= min_len {
                    out.push(std::mem::take(&mut run));
                } else {
                    run.clear();
                }
            } else {
                run.push(edge);
            }
        } else if va && !run.is_empty() {
            run.push(b);
        }
    }
    if run.len() >= 2 && ring_length(&run) >= min_len {
        out.push(run);
    }
    out
}

fn ring_length(pts: &[V2]) -> f64 {
    pts.windows(2).map(|pair| pair[0].dist(pair[1])).sum()
}

/// The Hilbert curve filling `bounds` with cells `cell` wide.
///
/// Upstream's `hilbert_n_to_xy`, digit by digit: every two bits of the index
/// pick a corner and a new state, so the curve visits each cell of the square
/// exactly once and neighbours stay adjacent across the fold. The square is a
/// power of two so the recursion terminates.
pub(crate) fn hilbert(bounds: hfw_kernel::Bounds, cell: f64) -> Vec<V2> {
    const NEXT: [usize; 16] = [4, 0, 0, 12, 0, 4, 4, 8, 12, 8, 8, 4, 8, 12, 12, 0];
    const DIGIT_X: [usize; 16] = [0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0];
    const DIGIT_Y: [usize; 16] = [0, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1];
    let span = (bounds.width().max(bounds.height()) / cell).ceil().max(1.0) as usize + 1;
    let mut side = 2usize;
    while side < span {
        side <<= 1;
    }
    let mut out = Vec::with_capacity(side * side);
    for n in 0..side * side {
        let mut digits = 0usize;
        let mut counter = n;
        while counter > 0 {
            counter >>= 2;
            digits += 1;
        }
        let mut state = if digits % 2 == 1 { 4 } else { 0 };
        let (mut x, mut y) = (0usize, 0usize);
        for i in (0..digits).rev() {
            state += (n >> (i * 2)) & 3;
            x |= DIGIT_X[state] << i;
            y |= DIGIT_Y[state] << i;
            state = NEXT[state];
        }
        out.push(V2::new(
            bounds.min.x + x as f64 * cell,
            bounds.min.y + y as f64 * cell,
        ));
    }
    out
}

/// The Archimedean spiral `r = a + b theta`, `b` a bead wide per turn.
pub(crate) fn archimedean(bounds: hfw_kernel::Bounds, cell: f64) -> Vec<V2> {
    let centre = V2::new(
        bounds.min.x + 0.5 * bounds.width(),
        bounds.min.y + 0.5 * bounds.height(),
    );
    let reach = 0.5 * (bounds.width().hypot(bounds.height())) + cell;
    let b = cell / std::f64::consts::TAU;
    let mut out = vec![centre];
    let (mut theta, mut r) = (0.0f64, 0.0f64);
    while r < reach {
        let step = 2.0 * (1.0 - 0.05 / r.max(0.05)).clamp(-1.0, 1.0).acos();
        theta += step.max(0.2);
        r = b * theta;
        out.push(V2::new(
            centre.x + r * theta.cos(),
            centre.y + r * theta.sin(),
        ));
    }
    out
}

/// The octagram spiral: fourteen points per turn of an eight-pointed star.
pub(crate) fn octagram(bounds: hfw_kernel::Bounds, cell: f64) -> Vec<V2> {
    let centre = V2::new(
        bounds.min.x + 0.5 * bounds.width(),
        bounds.min.y + 0.5 * bounds.height(),
    );
    let reach = 0.5 * (bounds.width().hypot(bounds.height())) + cell;
    let step = cell * std::f64::consts::SQRT_2;
    let mut out = vec![centre];
    let mut r = 0.0f64;
    while r < reach {
        r += step;
        let rx = r / std::f64::consts::SQRT_2;
        let r2 = r + rx;
        for (dx, dy) in [
            (r, 0.0),
            (r2, rx),
            (rx, rx),
            (rx, r2),
            (0.0, r),
            (-rx, r2),
            (-rx, rx),
            (-r2, rx),
            (-r, 0.0),
            (-r2, -rx),
            (-rx, -rx),
            (-rx, -r2),
            (0.0, -r),
            (rx, -r2),
            (rx, -rx),
            (r2 + step, -rx),
        ] {
            out.push(V2::new(centre.x + dx, centre.y + dy));
        }
    }
    out
}
