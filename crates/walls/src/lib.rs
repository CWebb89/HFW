//! Variable-width wall generation from an exact distance field.
//!
//! Classic concentric walls walk a fixed offset ladder `k * w` from the
//! boundary. That leaves uncovered area wherever the local feature width is
//! not a multiple of the extrusion width: the last loop either overlaps the
//! previous one or leaves a gap. This crate instead measures each point's
//! distance to the boundary and, where a medial axis is available, the local
//! inscribed radius, then decomposes that radius into a whole number of beads.
//! Bead counts blend along the medial axis and hysteresis keeps the choice
//! stable, so a tapering feature gets a continuous, gapless bead ladder.

#![forbid(unsafe_code)]

mod contour;
pub mod field;
pub mod params;
pub mod ridge;

use hfw_kernel::{Region, SegIndex, V2};
use params::{Mode, Params};

pub use field::{Field, FieldError, Grid, MAX_FIELD_CELLS};
pub use params::{Beading, Mode as WallMode, Params as WallParams};

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

/// One medial-axis branch, kept for diagnostic rendering.
#[derive(Clone, Debug)]
pub struct RidgeDebug {
    /// Branch vertices.
    pub pts: Vec<V2>,
    /// Inscribed radius at each vertex.
    pub r: Vec<f64>,
    /// Bead count at each vertex.
    pub n: Vec<u8>,
    /// True when the branch is closed.
    pub closed: bool,
}

/// Counters and timings for one wall generation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Number of distance-field cells.
    pub nodes: usize,
    /// Cells inside the wall band.
    pub band_nodes: usize,
    /// Cells matched to a thin-wall beading.
    pub thin_nodes: usize,
    /// Cells dropped as unprintable islands.
    pub dropped_nodes: usize,
    /// Medial-axis vertices.
    pub ridge_vertices: usize,
    /// Medial-axis branches.
    pub ridge_branches: usize,
    /// Milliseconds spent building the field.
    pub ms_field: f64,
    /// Milliseconds spent extracting the ridge.
    pub ms_ridge: f64,
    /// Milliseconds spent classifying cells.
    pub ms_classify: f64,
    /// Milliseconds spent tracing iso-lines.
    pub ms_contour: f64,
    /// Total milliseconds.
    pub ms_total: f64,
}

/// Wall generation output.
pub struct Output {
    /// Generated paths.
    pub lines: Vec<Line>,
    /// Medial axis, for diagnostics only.
    pub ridge: Vec<RidgeDebug>,
    /// Counters and timings.
    pub stats: Stats,
}

/// Failure modes when generating walls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WallsError {
    /// The distance field would exceed the memory budget.
    Field(field::FieldError),
}

impl From<field::FieldError> for WallsError {
    fn from(error: field::FieldError) -> Self {
        WallsError::Field(error)
    }
}

impl std::fmt::Display for WallsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WallsError::Field(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for WallsError {}

#[derive(Clone, Copy, PartialEq)]
enum NodeKind {
    Out,
    Deep,
    Thick,
    Dropped,
    Thin,
}

fn ray_hit(index: &SegIndex, p0: V2, u: V2, d0: f64, t_max: f64, h: f64) -> Option<f64> {
    let eps = 1e-9;
    let g = |t: f64| -> f64 {
        let p = p0 + u * t;
        match index.nearest(p, d0 + t + 1e-6) {
            Some(hit) => hit.d - (d0 + t),
            None => -1.0,
        }
    };
    let mut lo = 0.0;
    let mut step = h;
    let hi;
    loop {
        let t = (lo + step).min(t_max);
        if g(t) >= -eps {
            if t >= t_max {
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
    for _ in 0..24 {
        let m = 0.5 * (a + b);
        if g(m) >= -eps {
            a = m;
        } else {
            b = m;
        }
    }
    Some(0.5 * (a + b))
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
    };
    if line.pts.len() < 2 || line.length() < 3.0 * h {
        return None;
    }
    if line.closed && line.pts.len() < 3 {
        return None;
    }
    Some(line)
}

fn classify(
    field: &field::Field,
    index: &SegIndex,
    ridge: Option<&ridge::Ridge>,
    p: &Params,
) -> (Vec<NodeKind>, Vec<u32>) {
    let grid = &field.grid;
    let n_nodes = grid.len();
    let mut kind = vec![NodeKind::Out; n_nodes];
    let mut qid = vec![field::NONE; n_nodes];
    let thick_r = p.thick_radius();
    let usable = ridge.filter(|r| !r.points.is_empty());
    for id in 0..n_nodes {
        if !field.inside[id] {
            continue;
        }
        let d = field.d[id];
        if !d.is_finite() {
            kind[id] = NodeKind::Deep;
            continue;
        }
        kind[id] = NodeKind::Thick;
        let rg = match usable {
            Some(r) => r,
            None => continue,
        };
        let t_max = thick_r - d + p.h;
        if t_max <= 0.0 || d < 1e-9 {
            continue;
        }
        let pos = grid.pos_id(id);
        let reach = p.ext * rg.rmax + 2.0 * p.h;
        if rg.points.nearest(pos, t_max + reach).is_none() {
            continue;
        }
        let u = (pos - field.foot[id]).norm();
        if u == V2::ZERO {
            continue;
        }
        if let Some(t) = ray_hit(index, pos, u, d, t_max, p.h) {
            if d + t >= thick_r {
                continue;
            }
            let c = pos + u * t;
            if let Some((q, dist)) = rg.points.nearest(c, reach) {
                let rq = rg.rs[q as usize];
                if dist <= p.ext * rq + 2.0 * p.h && rq < thick_r && d + t <= rq * 1.05 + p.h {
                    if rg.bd[q as usize].is_none() {
                        kind[id] = NodeKind::Dropped;
                    } else {
                        kind[id] = NodeKind::Thin;
                        qid[id] = q;
                    }
                }
            }
        }
    }
    (kind, qid)
}

fn middle_lines(rg: &ridge::Ridge, p: &Params) -> Vec<Line> {
    let mut out = Vec::new();
    let mid_w = |i: u32| -> Option<f64> {
        let bd = rg.bd[i as usize]?;
        if !bd.half {
            return None;
        }
        let w = bd.middle_width(rg.rs[i as usize]);
        if w >= p.taper_min {
            Some(w)
        } else {
            None
        }
    };
    for br in &rg.branches {
        let mut ids: Vec<u32> = br.ids.clone();
        let mut closed_all = false;
        if br.closed {
            match ids.iter().position(|&i| mid_w(i).is_none()) {
                Some(k) => ids.rotate_left(k),
                None => closed_all = true,
            }
        }
        let mut runs: Vec<Vec<u32>> = Vec::new();
        if closed_all {
            runs.push(ids.clone());
        } else {
            let mut cur: Vec<u32> = Vec::new();
            for &i in &ids {
                if mid_w(i).is_some() {
                    cur.push(i);
                } else if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
            }
            if !cur.is_empty() {
                runs.push(cur);
            }
        }
        for run in runs {
            if run.len() == 1 && br.ids.len() == 1 {
                let i = run[0];
                let d = mid_w(i).unwrap();
                let c = rg.graph.pos[i as usize];
                let half = 0.5 * std::f64::consts::FRAC_PI_4 * d * 2.0;
                out.push(Line {
                    pts: vec![V2::new(c.x - half, c.y), V2::new(c.x + half, c.y)],
                    width: vec![d, d],
                    closed: false,
                    kind: Kind::Middle,
                });
                continue;
            }
            if run.len() < 2 {
                continue;
            }
            let pts: Vec<V2> = run.iter().map(|&i| rg.graph.pos[i as usize]).collect();
            let w: Vec<f64> = run.iter().map(|&i| mid_w(i).unwrap()).collect();
            let (sp, sw) = simplify(&pts, &w, p.simplify, 0.05 * p.w, closed_all);
            let line = Line {
                pts: sp,
                width: sw,
                closed: closed_all,
                kind: Kind::Middle,
            };
            if line.pts.len() >= 2 && line.length() >= 2.0 * p.h {
                out.push(line);
            }
        }
    }
    out
}

/// Generates walls for one slice region.
pub fn generate(region: &Region, p: &Params, mode: Mode) -> Result<Output, WallsError> {
    let t_all = std::time::Instant::now();
    let cap = p.cap();
    let rb = region.bounds();
    let index = SegIndex::new(
        region.segments(),
        rb.grown(cap + 6.0 * p.h + p.w),
        6.0 * p.h,
    );
    let grid = Grid::covering(rb.grown(2.0 * p.h), p.h);
    let t = std::time::Instant::now();
    let field = field::Field::build(region, &index, grid, cap)?;
    let ms_field = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let ridge = if mode == Mode::Hfw {
        Some(ridge::build(&field, &index, p))
    } else {
        None
    };
    let ms_ridge = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let (kind, qid) = classify(&field, &index, ridge.as_ref(), p);
    let ms_classify = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let n_nodes = grid.len();
    let mut lines: Vec<Line> = Vec::new();
    let mut phi = vec![0.0f64; n_nodes];
    let mut bw = vec![f64::NAN; n_nodes];
    let mut lv = vec![f64::NAN; n_nodes];
    for i in 0..p.walls {
        for id in 0..n_nodes {
            bw[id] = f64::NAN;
            lv[id] = f64::NAN;
            phi[id] = match kind[id] {
                NodeKind::Out | NodeKind::Dropped => -p.w,
                NodeKind::Deep => p.w,
                NodeKind::Thick => {
                    let l = (i as f64 + 0.5) * p.w;
                    bw[id] = p.w;
                    lv[id] = l;
                    field.d[id] - l
                }
                NodeKind::Thin => match ridge.as_ref().and_then(|rg| rg.bd[qid[id] as usize]) {
                    Some(bd) if i < bd.full_cells() => {
                        let l = bd.center(i);
                        bw[id] = bd.width(i);
                        lv[id] = l;
                        field.d[id] - l
                    }
                    _ => -p.w,
                },
            };
        }
        for raw in contour::trace(&grid, &phi, &bw, &lv) {
            if let Some(l) = finish(raw, &index, p, Kind::Side(i as u8)) {
                lines.push(l);
            }
        }
    }
    let ms_contour = t.elapsed().as_secs_f64() * 1e3;
    let mut ridge_dbg = Vec::new();
    let (mut rv, mut rb_n) = (0, 0);
    if let Some(rg) = &ridge {
        lines.extend(middle_lines(rg, p));
        rv = rg.graph.pos.len();
        rb_n = rg.branches.len();
        for br in &rg.branches {
            ridge_dbg.push(RidgeDebug {
                pts: br.ids.iter().map(|&i| rg.graph.pos[i as usize]).collect(),
                r: br.ids.iter().map(|&i| rg.rs[i as usize]).collect(),
                n: br
                    .ids
                    .iter()
                    .map(|&i| rg.bd[i as usize].map_or(0, |b| (2 * b.k - b.half as usize) as u8))
                    .collect(),
                closed: br.closed,
            });
        }
    }
    let stats = Stats {
        nodes: n_nodes,
        band_nodes: kind
            .iter()
            .filter(|k| matches!(k, NodeKind::Thick | NodeKind::Thin | NodeKind::Dropped))
            .count(),
        thin_nodes: kind.iter().filter(|&&k| k == NodeKind::Thin).count(),
        dropped_nodes: kind.iter().filter(|&&k| k == NodeKind::Dropped).count(),
        ridge_vertices: rv,
        ridge_branches: rb_n,
        ms_field,
        ms_ridge,
        ms_classify,
        ms_contour,
        ms_total: t_all.elapsed().as_secs_f64() * 1e3,
    };
    Ok(Output {
        lines,
        ridge: ridge_dbg,
        stats,
    })
}
