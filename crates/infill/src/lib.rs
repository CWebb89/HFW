//! Skin classification and wall-clipped infill generation.
//!
//! Two things decide what a layer needs: the region that is already covered by
//! the perimeter shell, and whether a point is a top or bottom skin. Skin
//! membership is computed by intersecting the packed layer masks of the next
//! `top_layers` and the previous `bottom_layers` layers, so a solid patch only
//! appears where the model stays covered vertically.
//!
//! Masks are kept in a small ring rather than for the whole model, which keeps
//! memory flat as the layer count grows.

#![forbid(unsafe_code)]

use hfw_kernel::{Bounds, Region, SegIndex, V2};
use hfw_walls::field::{scan_inside, Grid};
use hfw_walls::{Kind, Line, WallParams};

/// Infill pattern selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Alternating single-direction lines, 45 degrees and 135 degrees.
    Rectilinear,
    /// Both directions on every layer.
    Grid,
}

/// Bridge geometry decided before any G-code is emitted.
#[derive(Clone, Copy, Debug)]
pub struct BridgeParams {
    /// Bridge line density; 1.0 makes adjacent beads touch, 0.1 is a string.
    pub density: f64,
    /// Bridge bead width in millimetres.
    pub width: f64,
    /// Forced bridge direction in degrees; `None` picks the best per layer.
    pub angle: Option<f64>,
    /// Longest span printed as a bridge, in millimetres; 0 means no limit.
    pub max_length: f64,
    /// Print two crossing strands across each gap before the fill lines.
    pub cross_hatch: bool,
}

impl BridgeParams {
    /// Defaults for extrusion width `w`: solid bridge lines, auto direction.
    pub fn new(w: f64) -> BridgeParams {
        BridgeParams {
            density: 1.0,
            width: w,
            angle: None,
            max_length: 0.0,
            cross_hatch: false,
        }
    }

    fn spacing(&self) -> f64 {
        self.width / self.density
    }
}

/// Sparse and solid infill settings.
#[derive(Clone, Copy, Debug)]
pub struct InfillParams {
    /// Sparse infill density in the range (0, 1].
    pub density: f64,
    /// Line pattern.
    pub pattern: Pattern,
    /// Solid layers below each top surface.
    pub top_layers: usize,
    /// Solid layers above each bottom surface.
    pub bottom_layers: usize,
    /// Extrusion width.
    pub width: f64,
    /// Overlap between infill endpoints and the perimeter shell.
    pub overlap: f64,
    /// Mask resolution used for skin classification.
    pub skin_res: f64,
    /// Sampling step used when finding where a line crosses into the shell.
    pub step: f64,
    /// Bridge line settings.
    pub bridge: BridgeParams,
}

impl InfillParams {
    /// Defaults for extrusion width `w`.
    pub fn new(w: f64) -> InfillParams {
        InfillParams {
            density: 0.15,
            pattern: Pattern::Rectilinear,
            top_layers: 5,
            bottom_layers: 4,
            width: w,
            overlap: 0.1 * w,
            skin_res: w / 4.0,
            step: w / 4.0,
            bridge: BridgeParams::new(w),
        }
    }

    /// Radius at which the free area inside the perimeter shell begins.
    pub fn inner_offset(&self, p: &WallParams) -> f64 {
        p.thick_radius() + 0.5 * self.width - self.overlap
    }

    fn sparse_spacing(&self) -> f64 {
        match self.pattern {
            Pattern::Rectilinear => self.width / self.density,
            Pattern::Grid => 2.0 * self.width / self.density,
        }
    }

    fn validate(&self) -> bool {
        self.width.is_finite()
            && self.width > 0.0
            && self.density >= 0.0
            && self.density <= 1.0
            && self.sparse_spacing().is_finite()
            && self.bridge.width.is_finite()
            && self.bridge.width > 0.0
            && self.bridge.spacing().is_finite()
            && self.bridge.spacing() > 0.0
            && self.skin_res.is_finite()
            && self.skin_res > 0.0
    }
}

/// Bit-packed layer mask shared by every layer of one object.
#[derive(Clone, Copy, Debug)]
pub struct SkinGrid {
    /// Sampling grid.
    pub grid: Grid,
    words: usize,
}

fn pack(mask: &[bool]) -> Vec<u64> {
    let mut out = vec![0u64; mask.len().div_ceil(64)];
    for (i, &b) in mask.iter().enumerate() {
        if b {
            out[i / 64] |= 1u64 << (i % 64);
        }
    }
    out
}

impl SkinGrid {
    /// Builds a mask grid covering `regions` at resolution `res`.
    pub fn new(regions: &[Region], res: f64) -> SkinGrid {
        let mut b = Bounds::empty();
        for r in regions {
            for ring in &r.rings {
                for &p in ring {
                    b.add(p);
                }
            }
        }
        if b.is_empty() {
            b.add(V2::ZERO);
        }
        let grid = Grid::covering(b.grown(2.0 * res), res);
        SkinGrid {
            words: grid.len().div_ceil(64),
            grid,
        }
    }

    /// Packs one region into a fresh mask.
    pub fn packed(&self, region: &Region) -> Vec<u64> {
        if region.rings.is_empty() {
            vec![0u64; self.words]
        } else {
            pack(&scan_inside(region, &self.grid))
        }
    }

    /// Tests one point against a packed mask.
    pub fn is_set(&self, bits: &[u64], p: V2) -> bool {
        let ix = ((p.x - self.grid.origin.x) / self.grid.h).round();
        let iy = ((p.y - self.grid.origin.y) / self.grid.h).round();
        if ix < 0.0 || iy < 0.0 || ix >= self.grid.nx as f64 || iy >= self.grid.ny as f64 {
            return false;
        }
        let id = iy as usize * self.grid.nx + ix as usize;
        (bits[id / 64] >> (id % 64)) & 1 == 1
    }
}

/// Bounded cache of packed layer masks used for skin classification.
pub struct MaskCache<'a> {
    regions: &'a [Region],
    grid: SkinGrid,
    capacity: usize,
    masks: std::collections::HashMap<usize, Vec<u64>>,
}

impl<'a> MaskCache<'a> {
    /// Creates a cache holding at most `capacity` masks; it must exceed the
    /// largest `top_layers + bottom_layers + 1` in use.
    pub fn new(regions: &'a [Region], res: f64, capacity: usize) -> MaskCache<'a> {
        let grid = SkinGrid::new(regions, res);
        MaskCache {
            regions,
            grid,
            capacity: capacity.max(1),
            masks: std::collections::HashMap::new(),
        }
    }

    /// The shared mask grid.
    pub fn grid(&self) -> &SkinGrid {
        &self.grid
    }

    /// Word count of one packed layer mask.
    pub fn words(&self) -> usize {
        self.grid.words
    }

    /// Packed mask of layer `k` exactly as the slicer produced it.
    pub fn own_bits(&mut self, k: usize) -> Vec<u64> {
        self.bits(k).to_vec()
    }

    /// Number of masks currently held.
    pub fn resident(&self) -> usize {
        self.masks.len()
    }

    fn bits(&mut self, k: usize) -> &[u64] {
        if !self.masks.contains_key(&k) {
            if self.masks.len() >= self.capacity {
                let keep_from = k.saturating_sub(self.capacity - 1);
                self.masks.retain(|layer, _| *layer >= keep_from);
            }
            let packed = self.grid.packed(&self.regions[k]);
            self.masks.insert(k, packed);
        }
        self.masks.get(&k).map(Vec::as_slice).unwrap_or_default()
    }

    /// Bits of layer `k - 1`, empty on the first layer.
    ///
    /// This is the anchor mask: a bridge line is only a bridge when its ends
    /// land on material that the previous layer actually printed.
    pub fn below_bits(&mut self, k: usize) -> Vec<u64> {
        if k == 0 {
            return vec![0u64; self.grid.words];
        }
        self.bits(k - 1).to_vec()
    }

    /// Bits of layer `k` with nothing printed underneath them.
    ///
    /// These are the cells a bridge has to span. The first layer is exempt
    /// because the bed supports it.
    pub fn air_below_bits(&mut self, k: usize) -> Vec<u64> {
        let own = self.bits(k).to_vec();
        if k == 0 {
            return vec![0u64; own.len()];
        }
        let below = self.bits(k - 1).to_vec();
        own.iter().zip(&below).map(|(o, b)| o & !b).collect()
    }

    /// Bits marking the part of layer `k` that is neither top nor bottom skin.
    ///
    /// A cell stays sparse only while it remains covered `top` layers above and
    /// `bottom` layers below; anything else becomes solid.
    pub fn solid_bits(&mut self, k: usize, top: usize, bottom: usize) -> Vec<u64> {
        let words = self.grid.words;
        let real_top = top.min(self.regions.len() - 1 - k);
        let real_bottom = bottom.min(k);
        let mut cov_top = vec![u64::MAX; words];
        for j in 1..=real_top {
            let bits = self.bits(k + j);
            for (c, b) in cov_top.iter_mut().zip(bits) {
                *c &= *b;
            }
        }
        if real_top < top {
            cov_top.iter_mut().for_each(|w| *w = 0);
        }
        let mut cov_bot = vec![u64::MAX; words];
        for j in 1..=real_bottom {
            let bits = self.bits(k - j);
            for (c, b) in cov_bot.iter_mut().zip(bits) {
                *c &= *b;
            }
        }
        if real_bottom < bottom {
            cov_bot.iter_mut().for_each(|w| *w = 0);
        }
        let own = self.bits(k);
        (0..words)
            .map(|i| own[i] & !(cov_top[i] & cov_bot[i]))
            .collect()
    }
}

struct Ctx<'a> {
    region: &'a Region,
    index: &'a SegIndex,
    grid: &'a SkinGrid,
    mask: &'a [u64],
    thr: f64,
    ip: &'a InfillParams,
    all_solid: bool,
    width: f64,
}

fn family(cx: &Ctx, theta: f64, spacing: f64, want_solid: bool) -> Vec<Line> {
    let u = V2::new(theta.cos(), theta.sin());
    let n = V2::new(-u.y, u.x);
    let (mut cmin, mut cmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for ring in &cx.region.rings {
        for &p in ring {
            let c = n.dot(p);
            cmin = cmin.min(c);
            cmax = cmax.max(c);
        }
    }
    if !(spacing.is_finite() && spacing > 0.0) {
        return Vec::new();
    }
    let i0 = (cmin / spacing).ceil() as i64;
    let i1 = (cmax / spacing).floor() as i64;
    let min_len = cx.width;
    let kind = if want_solid {
        Kind::Solid
    } else {
        Kind::Infill
    };
    let mut out = Vec::new();
    for i in i0..=i1 {
        let c = (i as f64 + 1e-7) * spacing;
        let mut ts: Vec<f64> = Vec::new();
        for ring in &cx.region.rings {
            let m = ring.len();
            for k in 0..m {
                let a = ring[k];
                let b = ring[(k + 1) % m];
                let ca = n.dot(a);
                let cb = n.dot(b);
                if (ca <= c) != (cb <= c) {
                    let f = (c - ca) / (cb - ca);
                    ts.push(u.dot(a + (b - a) * f));
                }
            }
        }
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let valid = |t: f64| -> bool {
            let p = n * c + u * t;
            if cx.index.nearest(p, cx.thr).is_some() {
                return false;
            }
            if cx.all_solid {
                return want_solid;
            }
            cx.grid.is_set(cx.mask, p) == want_solid
        };
        for pair in ts.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let (t0, t1) = (pair[0], pair[1]);
            if t1 - t0 < min_len {
                continue;
            }
            let nsteps = (((t1 - t0) / cx.ip.step).ceil() as usize).max(1);
            let ext = |t: f64| -> f64 {
                let p = n * c + u * t;
                match cx.index.nearest(p, cx.thr + 1e-3) {
                    Some(hit) => {
                        let nb = (p - hit.foot).norm();
                        let cs = u.dot(nb).abs().max(0.2);
                        let tn = (1.0 - cs * cs).max(0.0).sqrt() / cs;
                        (0.5 * cx.width * tn).min(2.0 * cx.width)
                    }
                    None => 0.0,
                }
            };
            let emit = |a: f64, b: f64, out: &mut Vec<Line>| {
                if b - a >= min_len {
                    let (a, b) = (a - ext(a), b + ext(b));
                    out.push(Line {
                        pts: vec![n * c + u * a, n * c + u * b],
                        width: vec![cx.width; 2],
                        closed: false,
                        kind,
                    });
                }
            };
            let mut prev_t = t0;
            let mut prev_v = valid(t0);
            let mut start = if prev_v { Some(t0) } else { None };
            for j in 1..=nsteps {
                let t = t0 + (t1 - t0) * j as f64 / nsteps as f64;
                let v = valid(t);
                if v != prev_v {
                    let (mut lo, mut hi) = (prev_t, t);
                    for _ in 0..26 {
                        let mid = 0.5 * (lo + hi);
                        if valid(mid) == prev_v {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    let tb = 0.5 * (lo + hi);
                    if prev_v {
                        if let Some(s) = start {
                            emit(s, tb, &mut out);
                        }
                        start = None;
                    } else {
                        start = Some(tb);
                    }
                }
                prev_v = v;
                prev_t = t;
            }
            if prev_v {
                if let Some(s) = start {
                    emit(s, t1, &mut out);
                }
            }
        }
    }
    out
}

/// Fraction of `lines` whose two ends land on material printed one layer down.
///
/// A bridge bead is only useful when it is pinned at both ends; a line with a
/// free end droops whatever direction it runs in. The probe looks one bead
/// beyond each end because the family generator already extends bead ends into
/// the wall band, so the endpoint may sit past the anchor.
fn anchored_fraction(lines: &[Line], below: &[u64], grid: &SkinGrid, reach: f64) -> f64 {
    let mut total = 0.0;
    let mut anchored = 0.0;
    for line in lines {
        let (a, b) = (line.pts[0], line.pts[1]);
        let len = a.dist(b);
        if len <= 0.0 {
            continue;
        }
        total += len;
        let u = (b - a) * (1.0 / len);
        let held = |p: V2, out: V2| grid.is_set(below, p) || grid.is_set(below, p + out * reach);
        if held(a, -u) && held(b, u) {
            anchored += len;
        }
    }
    if total > 0.0 {
        anchored / total
    } else {
        0.0
    }
}

/// Generates solid and sparse infill for one layer.
pub fn infill_layer(
    region: &Region,
    cache: &mut MaskCache,
    k: usize,
    p: &WallParams,
    ip: &InfillParams,
) -> Vec<Line> {
    if region.rings.is_empty() || !ip.validate() {
        return Vec::new();
    }
    let solid = cache.solid_bits(k, ip.top_layers, ip.bottom_layers);
    let air = cache.air_below_bits(k);
    let below = cache.below_bits(k);
    let grid = *cache.grid();
    let thr = ip.inner_offset(p);
    let rb = region.bounds();
    let index = SegIndex::new(
        region.segments(),
        rb.grown(thr + ip.step + 1.0),
        (0.5 * thr).max(0.3),
    );
    let all_solid = ip.density >= 0.999;
    let mut covered = solid.clone();
    for (c, a) in covered.iter_mut().zip(&air) {
        *c &= !*a;
    }
    let mut sparse = covered.clone();
    for (s, a) in sparse.iter_mut().zip(&air) {
        *s |= *a;
    }
    let cx = Ctx {
        region,
        index: &index,
        grid: &grid,
        mask: &covered,
        thr,
        ip,
        all_solid,
        width: ip.width,
    };
    let cx_sparse = Ctx {
        mask: &sparse,
        ..cx
    };
    let dir = if k % 2 == 0 { 45.0f64 } else { 135.0f64 };
    let mut lines = family(&cx, dir.to_radians(), ip.width, true);
    if air.iter().any(|w| *w != 0) {
        let cx_bridge = Ctx {
            region,
            index: &index,
            grid: &grid,
            mask: &air,
            thr,
            ip,
            all_solid: false,
            width: ip.bridge.width,
        };
        let candidates: Vec<f64> = match ip.bridge.angle {
            Some(deg) => vec![deg],
            None => vec![0.0, 45.0, 90.0, 135.0],
        };
        let mut best: Option<(f64, Vec<Line>)> = None;
        for deg in candidates {
            let candidate = family(&cx_bridge, deg.to_radians(), ip.bridge.spacing(), true);
            if candidate.is_empty() {
                continue;
            }
            let score = anchored_fraction(&candidate, &below, &grid, ip.bridge.width);
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, candidate));
            }
        }
        if let Some((_, bridge)) = best {
            if ip.bridge.cross_hatch {
                lines.extend(cross_hatch(&air, &grid, ip.bridge.width));
            }
            let limit = ip.bridge.max_length;
            for mut line in bridge {
                let span = line.pts[0].dist(line.pts[1]);
                line.kind = if limit > 0.0 && span > limit {
                    Kind::Solid
                } else {
                    Kind::Bridge
                };
                lines.push(line);
            }
        }
    }
    if !all_solid && ip.density > 0.0 {
        match ip.pattern {
            Pattern::Rectilinear => {
                lines.extend(family(
                    &cx_sparse,
                    dir.to_radians(),
                    ip.sparse_spacing(),
                    false,
                ));
            }
            Pattern::Grid => {
                let sp = ip.sparse_spacing();
                lines.extend(family(&cx_sparse, 45.0f64.to_radians(), sp, false));
                lines.extend(family(&cx_sparse, 135.0f64.to_radians(), sp, false));
            }
        }
    }
    lines
}

/// Two strands per gap, crossing corner to corner.
///
/// Each gap is flooded into a component and its diagonal is clipped to the cells
/// that are actually air, so an irregular overhang gets a chord and not a box.
fn cross_hatch(air: &[u64], grid: &SkinGrid, width: f64) -> Vec<Line> {
    let g = grid.grid;
    let mut visited = vec![false; g.nx * g.ny];
    let mut out = Vec::new();
    for iy in 0..g.ny {
        for ix in 0..g.nx {
            let id = iy * g.nx + ix;
            if visited[id] || !air_cell(air, &g, ix as i64, iy as i64) {
                continue;
            }
            visited[id] = true;
            let mut stack = vec![(ix, iy)];
            let (mut x0, mut x1, mut y0, mut y1) = (ix, ix, iy, iy);
            while let Some((cx, cy)) = stack.pop() {
                x0 = x0.min(cx);
                x1 = x1.max(cx);
                y0 = y0.min(cy);
                y1 = y1.max(cy);
                for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    let (ux, uy) = (cx as i64 + dx, cy as i64 + dy);
                    if ux < 0 || uy < 0 || ux >= g.nx as i64 || uy >= g.ny as i64 {
                        continue;
                    }
                    let (ux, uy) = (ux as usize, uy as usize);
                    let uid = uy * g.nx + ux;
                    if visited[uid] || !air_cell(air, &g, ux as i64, uy as i64) {
                        continue;
                    }
                    visited[uid] = true;
                    stack.push((ux, uy));
                }
            }
            for (ax, ay, bx, by) in [(x0, y0, x1, y1), (x0, y1, x1, y0)] {
                if let Some(line) = chord(air, &g, (ax, ay), (bx, by), width) {
                    out.push(line);
                }
            }
        }
    }
    out
}

fn air_cell(air: &[u64], g: &Grid, ix: i64, iy: i64) -> bool {
    if ix < 0 || iy < 0 || ix >= g.nx as i64 || iy >= g.ny as i64 {
        return false;
    }
    let id = iy as usize * g.nx + ix as usize;
    (air[id / 64] >> (id % 64)) & 1 == 1
}

/// Longest run of air cells along a cell-space chord, as a bridge strand.
fn chord(air: &[u64], g: &Grid, a: (usize, usize), b: (usize, usize), width: f64) -> Option<Line> {
    let into = |t: f64| -> (i64, i64) {
        (
            (a.0 as f64 + (b.0 as f64 - a.0 as f64) * t).round() as i64,
            (a.1 as f64 + (b.1 as f64 - a.1 as f64) * t).round() as i64,
        )
    };
    let inside = |t: f64| -> bool {
        let (ix, iy) = into(t);
        air_cell(air, g, ix, iy)
    };
    let span = (b.0 as f64 - a.0 as f64)
        .abs()
        .max((b.1 as f64 - a.1 as f64).abs());
    let steps = span.ceil().max(1.0) as usize;
    let (mut best, mut best_from, mut run_from) = (0.0f64, 0.0f64, None::<f64>);
    for j in 0..=steps {
        let t = j as f64 / steps as f64;
        if inside(t) {
            let s = *run_from.get_or_insert(t);
            if t - s > best {
                best = t - s;
                best_from = s;
            }
        } else {
            run_from = None;
        }
    }
    let length = best * span * g.h;
    if length < 2.0 * width {
        return None;
    }
    let step = 1.0 / steps as f64;
    let edge = |inside_t: f64, out_t: f64| -> f64 {
        let (mut i, mut o) = (inside_t, out_t);
        for _ in 0..24 {
            let mid = 0.5 * (i + o);
            if inside(mid) {
                i = mid;
            } else {
                o = mid;
            }
        }
        i
    };
    let t0 = edge(best_from, (best_from - step).max(0.0));
    let t1 = edge(best_from + best, (best_from + best + step).min(1.0));
    let at = |t: f64| -> V2 {
        let (ix, iy) = into(t);
        V2::new(g.origin.x + ix as f64 * g.h, g.origin.y + iy as f64 * g.h)
    };
    Some(Line {
        pts: vec![at(t0), at(t1)],
        width: vec![width; 2],
        closed: false,
        kind: Kind::BridgeAnchor,
    })
}

/// Fills an arbitrary packed mask with lines, for callers that already own the
/// mask rather than asking for skin classification.
///
/// Lines are derived from the mask itself, not from a region: a support fill sits
/// *outside* the part, so clipping it to the part's outline would leave nothing.
pub fn fill_from_mask(
    grid: &SkinGrid,
    mask: &[u64],
    width: f64,
    spacing: f64,
    angle: f64,
    kind: Kind,
    step: f64,
) -> Vec<Line> {
    let g = &grid.grid;
    if !(spacing.is_finite() && spacing > 0.0) || !(width.is_finite() && width > 0.0) {
        return Vec::new();
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for iy in 0..g.ny {
        for ix in 0..g.nx {
            let id = iy * g.nx + ix;
            if (mask[id / 64] >> (id % 64)) & 1 == 0 {
                continue;
            }
            lo = lo.min(ix as f64);
            hi = hi.max(ix as f64);
            let _ = iy;
        }
    }
    if !lo.is_finite() {
        return Vec::new();
    }
    let u = V2::new(angle.to_radians().cos(), angle.to_radians().sin());
    let nv = V2::new(-u.y, u.x);
    let at = |ix: f64, iy: f64| -> V2 { V2::new(g.origin.x + ix * g.h, g.origin.y + iy * g.h) };
    let set = |p: V2| -> bool { grid.is_set(mask, p) };
    let (mut cmin, mut cmax) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut tmin, mut tmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for iy in 0..g.ny {
        for ix in 0..g.nx {
            let id = iy * g.nx + ix;
            if (mask[id / 64] >> (id % 64)) & 1 == 0 {
                continue;
            }
            let p = at(ix as f64, iy as f64);
            cmin = cmin.min(nv.dot(p));
            cmax = cmax.max(nv.dot(p));
            tmin = tmin.min(u.dot(p));
            tmax = tmax.max(u.dot(p));
        }
    }
    let mut out = Vec::new();
    let min_len = width;
    let i0 = (cmin / spacing).ceil() as i64;
    let i1 = (cmax / spacing).floor() as i64;
    for i in i0..=i1 {
        let c = i as f64 * spacing;
        let steps = (((tmax - tmin) / step).ceil() as usize).max(1);
        let point = |t: f64| nv * c + u * t;
        let mut start: Option<f64> = None;
        let mut prev = tmin;
        let mut prev_v = set(point(tmin));
        if prev_v {
            start = Some(tmin);
        }
        for j in 1..=steps {
            let t = tmin + (tmax - tmin) * j as f64 / steps as f64;
            let v = set(point(t));
            if v != prev_v {
                let (mut a, mut b) = (prev, t);
                for _ in 0..20 {
                    let mid = 0.5 * (a + b);
                    if set(point(mid)) == prev_v {
                        a = mid;
                    } else {
                        b = mid;
                    }
                }
                let edge = 0.5 * (a + b);
                if prev_v {
                    if let Some(s) = start.take() {
                        emit_span(&point, s, edge, min_len, width, kind, &mut out);
                    }
                } else {
                    start = Some(edge);
                }
            }
            prev = t;
            prev_v = v;
        }
        if prev_v {
            if let Some(s) = start.take() {
                emit_span(&point, s, tmax, min_len, width, kind, &mut out);
            }
        }
    }
    out
}

fn emit_span(
    point: &impl Fn(f64) -> V2,
    a: f64,
    b: f64,
    min_len: f64,
    width: f64,
    kind: Kind,
    out: &mut Vec<Line>,
) {
    if b - a < min_len {
        return;
    }
    let (a, b) = (a - 0.5 * width, b + 0.5 * width);
    out.push(Line {
        pts: vec![point(a), point(b)],
        width: vec![width; 2],
        closed: false,
        kind,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Region {
        Region::new(vec![vec![
            V2::new(x0, y0),
            V2::new(x1, y0),
            V2::new(x1, y1),
            V2::new(x0, y1),
        ]])
    }

    fn stack(n: usize) -> Vec<Region> {
        (0..n).map(|_| square(0.0, 0.0, 20.0, 20.0)).collect()
    }

    #[test]
    fn infill_stays_inside_the_region() {
        let regions = stack(1);
        let mut cache = MaskCache::new(&regions, 0.1, 1);
        let p = WallParams::new(0.4, 3);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.5;
        let lines = infill_layer(&regions[0], &mut cache, 0, &p, &ip);
        assert!(!lines.is_empty());
        for l in &lines {
            for pt in &l.pts {
                assert!(
                    regions[0].contains(*pt)
                        || pt.x.min(pt.y) < 1e-6
                        || (20.0 - pt.x.max(pt.y)) < 1e-6,
                    "line vertex {pt:?} left the part"
                );
            }
        }
    }

    fn spanned_plate(legs: usize, plate: usize) -> Vec<Region> {
        let mut regions: Vec<Region> = (0..legs)
            .map(|_| {
                let mut r = square(0.0, 0.0, 10.0, 20.0);
                r.rings.extend(square(30.0, 0.0, 40.0, 20.0).rings.clone());
                r
            })
            .collect();
        regions.extend((0..plate).map(|_| square(0.0, 0.0, 40.0, 20.0)));
        regions
    }

    #[test]
    fn sparse_infill_never_prints_over_air() {
        let regions = spanned_plate(5, 6);
        let mut cache = MaskCache::new(&regions, 0.1, 12);
        let p = WallParams::new(0.4, 2);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.5;
        let k = 5;
        let lines = infill_layer(&regions[k], &mut cache, k, &p, &ip);

        let in_gap = |pt: V2| pt.x > 11.0 && pt.x < 29.0;
        let sparse: Vec<&Line> = lines.iter().filter(|l| l.kind == Kind::Infill).collect();
        assert!(
            !sparse.is_empty(),
            "the leg tops still need sparse infill to prove the gap check bites"
        );
        for line in sparse {
            assert!(
                !line.pts.iter().any(|pt| in_gap(*pt)),
                "sparse infill {line:?} was laid across the unsupported gap"
            );
        }
        assert!(
            lines.iter().any(|l| l.kind == Kind::Bridge),
            "the gap should still be bridged"
        );
    }

    #[test]
    fn cross_hatch_puts_crossing_strands_before_the_fill() {
        let regions = spanned_plate(5, 6);
        let mut cache = MaskCache::new(&regions, 0.1, 12);
        let p = WallParams::new(0.4, 2);
        let mut ip = InfillParams::new(0.4);
        ip.bridge.cross_hatch = true;
        let k = 5;
        let lines = infill_layer(&regions[k], &mut cache, k, &p, &ip);
        let anchors: Vec<&Line> = lines
            .iter()
            .filter(|l| l.kind == Kind::BridgeAnchor)
            .collect();
        assert_eq!(anchors.len(), 2, "one gap takes two crossing strands");
        let (a, b) = (anchors[0], anchors[1]);
        let slope = |l: &Line| (l.pts[1].y - l.pts[0].y) / (l.pts[1].x - l.pts[0].x).max(1e-9);
        assert!(
            slope(a).signum() != slope(b).signum(),
            "the two strands must cross, not run parallel"
        );
        for l in [a, b] {
            assert!(
                l.pts.iter().all(|pt| pt.x > 9.0 && pt.x < 31.0),
                "an anchor strand left the gap: {l:?}"
            );
            assert!(
                l.pts[0].dist(l.pts[1]) > 15.0,
                "an anchor strand is shorter than the gap it spans"
            );
        }
    }

    #[test]
    fn a_thin_part_is_solid_throughout() {
        let regions = stack(6);
        let mut cache = MaskCache::new(&regions, 0.1, 12);
        let ip = InfillParams::new(0.4);
        let centre = V2::new(10.0, 10.0);
        for k in 0..6 {
            let solid = cache.solid_bits(k, ip.top_layers, ip.bottom_layers);
            assert!(
                cache.grid().is_set(&solid, centre),
                "layer {k} of a 6-layer part should be solid at the centre"
            );
        }
    }

    #[test]
    fn the_interior_of_a_tall_part_stays_sparse() {
        let regions = stack(30);
        let mut cache = MaskCache::new(&regions, 0.1, 12);
        let ip = InfillParams::new(0.4);
        let centre = V2::new(10.0, 10.0);
        let middle = cache.solid_bits(15, ip.top_layers, ip.bottom_layers);
        assert!(
            !cache.grid().is_set(&middle, centre),
            "the middle of a tall part must not be solid"
        );
        let bottom = cache.solid_bits(0, ip.top_layers, ip.bottom_layers);
        assert!(
            cache.grid().is_set(&bottom, centre),
            "the first layer is always a bottom skin"
        );
        let top = cache.solid_bits(29, ip.top_layers, ip.bottom_layers);
        assert!(
            cache.grid().is_set(&top, centre),
            "the last layer is always a top skin"
        );
    }

    #[test]
    fn mask_cache_reuses_slots_without_losing_answers() {
        let regions = stack(20);
        let mut small = MaskCache::new(&regions, 0.1, 3);
        let mut large = MaskCache::new(&regions, 0.1, 40);
        for k in 0..20 {
            assert_eq!(
                small.solid_bits(k, 2, 2),
                large.solid_bits(k, 2, 2),
                "layer {k} disagreed between cache sizes"
            );
        }
    }
}
