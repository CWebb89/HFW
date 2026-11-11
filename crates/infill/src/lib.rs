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
    index: SegIndex,
    grid: &'a SkinGrid,
    solid: &'a [u64],
    thr: f64,
    ip: &'a InfillParams,
    all_solid: bool,
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
    let min_len = cx.ip.width;
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
            cx.grid.is_set(cx.solid, p) == want_solid
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
                        (0.5 * cx.ip.width * tn).min(2.0 * cx.ip.width)
                    }
                    None => 0.0,
                }
            };
            let emit = |a: f64, b: f64, out: &mut Vec<Line>| {
                if b - a >= min_len {
                    let (a, b) = (a - ext(a), b + ext(b));
                    out.push(Line {
                        pts: vec![n * c + u * a, n * c + u * b],
                        width: vec![cx.ip.width; 2],
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
    let grid = *cache.grid();
    let thr = ip.inner_offset(p);
    let rb = region.bounds();
    let index = SegIndex::new(
        region.segments(),
        rb.grown(thr + ip.step + 1.0),
        (0.5 * thr).max(0.3),
    );
    let all_solid = ip.density >= 0.999;
    let cx = Ctx {
        region,
        index,
        grid: &grid,
        solid: &solid,
        thr,
        ip,
        all_solid,
    };
    let dir = if k % 2 == 0 { 45.0f64 } else { 135.0f64 };
    let mut lines = family(&cx, dir.to_radians(), ip.width, true);
    if !all_solid && ip.density > 0.0 {
        match ip.pattern {
            Pattern::Rectilinear => {
                lines.extend(family(&cx, dir.to_radians(), ip.sparse_spacing(), false));
            }
            Pattern::Grid => {
                let sp = ip.sparse_spacing();
                lines.extend(family(&cx, 45.0f64.to_radians(), sp, false));
                lines.extend(family(&cx, 135.0f64.to_radians(), sp, false));
            }
        }
    }
    lines
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
