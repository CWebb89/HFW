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

use hfw_kernel::{Bounds, Hit, Region, SegIndex, V2};
use hfw_walls::field::{scan_inside, Grid};
use hfw_walls::{Kind, Line, WallParams};

mod curves;

/// Infill pattern selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Alternating single-direction lines, 45 degrees and 135 degrees.
    Rectilinear,
    /// Both directions on every layer.
    Grid,
    /// One direction on every layer, never alternating.
    Line,
    /// Three directions sixty degrees apart, so the part resists pull in any
    /// horizontal direction rather than favouring one.
    Triangle,
    /// One direction per layer, cycling through four so consecutive layers cross.
    ///
    /// This is the flat projection of an interlocking fill: it is not Slic3r's
    /// cubic, whose strands also weave vertically within one layer's thickness.
    /// What it buys is a part that is equally strong along both horizontal axes
    /// without printing twice the plastic on any single layer.
    Cubic,
    /// Waves of the gyroid surface, phase-shifted by height.
    ///
    /// The only pattern here that is the same in every direction and the same
    /// on every layer: neighbours never line up vertically, so the part is
    /// equally stiff along all three axes, and every strand has something to
    /// land on at a shallow angle instead of nothing at all.
    Gyroid,
    /// Slices through a stack of truncated octahedra, alternating rows and
    /// columns by layer.
    Honeycomb3D,
    /// Loops inset from the sparse region's own edge, so the fill follows the
    /// part instead of a lattice drawn over it.
    Concentric,
    /// Rectilinear lines joined end to end at each scanline boundary, so the
    /// nozzle crosses to the next line instead of lifting.
    ZigZag,
    /// Two families at forty-five degrees either side of the fill angle.
    ///
    /// Geometrically this is `Pattern::Grid` turned by forty-five degrees, and it
    /// deposits the same plastic; the name is upstream's, and the direction is
    /// what a person choosing it is choosing.
    CrossHatch,
    /// A flat hexagonal lattice, drawn as the hexagons' own sides.
    ///
    /// Not `Pattern::Honeycomb3D`: that is a three-dimensional cell sliced into
    /// curves, this is a plane figure whose every bead is a straight side, so it
    /// deposits the same amount on every layer and never needs a curve fitter.
    Honeycomb,
    /// One direction on every layer, never alternated.
    Monotonic,
    /// The same geometry as `Pattern::Monotonic`, kept as its own name because
    /// the dialect spells it separately.
    MonotonicLines,
}

/// Pattern used to draw a solid surface.
///
/// These are the skin half of the dialect's fill-pattern list, restricted to the
/// ones this engine can draw. A skin is short beads inside a mask, so a pattern
/// here decides the bead direction and the order the beads run in, which is what
/// the visible face is judged on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SkinPattern {
    /// Direction flips between layers, so a solid block crosses itself.
    Rectilinear,
    /// One direction on every layer, each bead drawn the same way round.
    #[default]
    Monotonic,
    /// One direction on every layer, with the beads left unconnected.
    MonotonicLines,
    /// Forty-five degrees on every layer, never flipped.
    AlignedRectilinear,
    /// Loops inset from the surface edge, each one a bead wide.
    Concentric,
    /// One space-filling curve visiting every bead-sized cell of the face once.
    HilbertCurve,
    /// A single spiral, one bead wide per turn.
    ArchimedeanChords,
    /// An eight-pointed spiral, one bead wide per turn.
    OctagramSpiral,
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

/// How far the solid shell reaches between a skin and the sparse fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerticalShell {
    /// Solid only where a neighbour is missing: one layer under each skin.
    Disabled,
    /// The full top/bottom window, skin footprints only.
    Partial,
    /// The full window, including the wedge under a slope.
    Enabled,
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
    /// How far the solid shell reaches between the skin and the sparse fill.
    pub vertical_shell: VerticalShell,
    /// Extrusion width.
    pub width: f64,
    /// Extrusion width for solid (skin) fill; zero means `width`.
    ///
    /// A solid skin is drawn with its beads touching, so its width is also its
    /// spacing. PrusaSlicer and OrcaSlicer let the skin be printed with its own
    /// bead width (`solid_infill_extrusion_width`) rather than the sparse one.
    pub solid_width: f64,
    /// Extrusion width for the top surface; zero means `solid_width`.
    pub top_width: f64,
    /// Pattern for the top surface.
    pub top_pattern: SkinPattern,
    /// Pattern for the bottom surface.
    pub bottom_pattern: SkinPattern,
    /// Force the interior solid every Nth layer; zero leaves it sparse.
    pub solid_every: usize,
    /// Layers whose whole area is below this print solid, in square millimetres.
    pub solid_below_area: f64,
    /// Overlap between infill endpoints and the perimeter shell.
    pub overlap: f64,
    /// Mask resolution used for skin classification.
    pub skin_res: f64,
    /// Sampling step used when finding where a line crosses into the shell.
    pub step: f64,
    /// Bridge line settings.
    pub bridge: BridgeParams,
    /// Longest segment tying an infill line to the perimeter, in millimetres.
    ///
    /// The dialect default is 600 percent of the infill width, which is looser
    /// than the geometric taper, so the taper stays the binding limit unless the
    /// profile tightens the anchor.
    pub anchor: f64,
    /// Ceiling on that segment, in millimetres.
    pub anchor_max: f64,
    /// Direction sparse infill prints on the first layer, in degrees.
    ///
    /// Every pattern is turned by this angle; the families that alternate by
    /// layer add 90 degrees on the odd ones, so the dialect's 45 gives the usual
    /// 45/135 cross-hatch.
    pub fill_angle: f64,
    /// Combine the sparse fill of N layers into one thicker layer; 1 is off.
    ///
    /// The engine hands this crate the intersection of the layers' free areas and
    /// the layers it replaces; the fill is then drawn on the top layer of the
    /// group at N times the spacing and N times the flow, so the plastic that two
    /// layers would have carried is laid in one pass.
    pub combine_layers: usize,
    /// Combine automatically, up to the nozzle diameter; no layer count needed.
    pub combine_auto: bool,
    /// Ceiling on a combined layer's height, in millimetres; 0 uses the nozzle.
    pub combine_max_height: f64,
}

/// One layer's share of a combined fill.
///
/// `strip` marks the cells whose sparse fill moves up to the top layer of the
/// group, and `extra` is the intersection itself, drawn at `layers` times the
/// spacing and the same multiple of the flow.
pub struct Combine<'a> {
    /// Cells this layer must not fill, because the plastic moved up.
    pub strip: &'a [u64],
    /// Cells the top layer of the group fills for the whole group.
    pub extra: &'a [u64],
    /// Number of layers the extra cells stand for.
    pub layers: usize,
}

impl InfillParams {
    /// Defaults for extrusion width `w`.
    pub fn new(w: f64) -> InfillParams {
        InfillParams {
            density: 0.15,
            pattern: Pattern::Rectilinear,
            top_layers: 5,
            bottom_layers: 4,
            vertical_shell: VerticalShell::Enabled,
            width: w,
            solid_width: 0.0,
            top_width: 0.0,
            top_pattern: SkinPattern::Monotonic,
            bottom_pattern: SkinPattern::Monotonic,
            solid_every: 0,
            solid_below_area: 70.0,
            overlap: 0.1 * w,
            skin_res: w / 4.0,
            step: w / 4.0,
            bridge: BridgeParams::new(w),
            anchor: 6.0 * w,
            anchor_max: 50.0,
            fill_angle: 45.0,
            combine_layers: 1,
            combine_auto: false,
            combine_max_height: 0.0,
        }
    }

    /// Layers above and below that count as covered when deciding solid.
    ///
    /// [`VerticalShell::Disabled`] narrows the window to the immediate
    /// neighbours, so the solid shell is one layer thick under each skin and the
    /// wedge under a slope is sparse.
    pub fn window(&self) -> (usize, usize) {
        if self.vertical_shell == VerticalShell::Disabled {
            (1, 1)
        } else {
            (self.top_layers, self.bottom_layers)
        }
    }

    /// Width of the solid-skin beads, falling back to the nominal width.
    pub fn solid_width(&self) -> f64 {
        if self.solid_width > 0.0 {
            self.solid_width
        } else {
            self.width
        }
    }

    /// Width of the top surface's beads, falling back to the solid width.
    pub fn top_solid_width(&self) -> f64 {
        if self.top_width > 0.0 {
            self.top_width
        } else {
            self.solid_width()
        }
    }

    /// The same parameters with every width-dependent number scaled to `w`.
    ///
    /// A layer printed at its own bead width needs its spacing, its skin
    /// resolution and its bridge settings moved with it, or the line search and
    /// the mask classification would still be sampling at the nominal width.
    pub fn scaled_to(&self, w: f64) -> InfillParams {
        if !(w.is_finite() && w > 0.0) || w == self.width {
            return *self;
        }
        let ratio = w / self.width;
        InfillParams {
            width: w,
            solid_width: self.solid_width * ratio,
            top_width: self.top_width * ratio,
            overlap: self.overlap * ratio,
            skin_res: self.skin_res * ratio,
            step: self.step * ratio,
            anchor: self.anchor * ratio,
            bridge: BridgeParams {
                width: self.bridge.width * ratio,
                ..self.bridge
            },
            ..*self
        }
    }

    /// How far an infill line may reach into the shell, in millimetres.
    ///
    /// Zero disables the anchor, which leaves each infill line ending on the
    /// inner shell boundary instead of tying into it.
    pub fn anchor_length(&self) -> f64 {
        if self.anchor <= 0.0 || self.anchor_max <= 0.0 {
            return 0.0;
        }
        self.anchor.min(self.anchor_max)
    }

    /// Radius at which the free area inside the perimeter shell begins.
    pub fn inner_offset(&self, p: &WallParams) -> f64 {
        p.thick_radius() + 0.5 * self.width - self.overlap
    }

    /// Radius at which a solid skin line may begin.
    ///
    /// The ordinary inset keeps a bead's plastic clear of the shell, which is right
    /// for sparse fill: a line that reached the wall would lay plastic over a bead
    /// already printed there. A skin is the opposite case, because it has to meet
    /// the wall it borders or the two leave a gap between them, so it is clipped
    /// where the reference slicers clip their fill: at the shell's inner edge less
    /// the overlap, one bead inside the innermost loop's path. Measured at the
    /// ordinary inset instead, a top surface narrower than twice the inset has no
    /// room for a line at all, which is what left the staircase annulus on a curved
    /// part to the perimeters.
    pub fn skin_offset(&self, p: &WallParams) -> f64 {
        (p.thick_radius() - self.overlap).max(0.0)
    }

    fn sparse_spacing(&self) -> f64 {
        let families = match self.pattern {
            Pattern::Grid | Pattern::CrossHatch => 2.0,
            Pattern::Triangle => 3.0,
            _ => 1.0,
        };
        families * self.width / self.density
    }

    /// Pitch of the pattern's repeating cell, in millimetres.
    ///
    /// A curved fill is anchored to a grid so that consecutive layers line up
    /// with the same lattice instead of drifting with the bounding box; the cell
    /// is the period of that lattice. `None` for the line families, which have no
    /// lattice to align to.
    ///
    /// Upstream's fills are handed the extrusion spacing, not the spacing that
    /// already accounts for density, and divide by density themselves; the same
    /// split is kept here so the cell sizes match what people have printed.
    pub fn pattern_cell(&self) -> Option<f64> {
        match self.pattern {
            Pattern::Gyroid => {
                let adjusted = (self.density * curves::GYROID_DENSITY_ADJUST).max(1e-9);
                Some(std::f64::consts::TAU * self.width / adjusted)
            }
            Pattern::Honeycomb3D => Some(2.0 * self.width / self.density.max(1e-9)),
            _ => None,
        }
    }

    /// True for patterns whose paths are curves rather than straight lines.
    pub fn is_curved(&self) -> bool {
        matches!(self.pattern, Pattern::Gyroid | Pattern::Honeycomb3D)
    }

    /// The directions this pattern prints on the layer at height index `k`.
    ///
    /// A pattern with more than one direction gives every direction its own
    /// family at `sparse_spacing()`, which already accounts for the count, so the
    /// deposited volume per layer stays tied to `density` rather than to the
    /// pattern's name.
    fn sparse_angles(&self, k: usize) -> Vec<f64> {
        let base = if k % 2 == 0 {
            self.fill_angle
        } else {
            self.fill_angle + 90.0
        };
        match self.pattern {
            Pattern::Rectilinear => vec![base],
            Pattern::Line | Pattern::Monotonic | Pattern::MonotonicLines => vec![self.fill_angle],
            Pattern::Grid => vec![self.fill_angle, self.fill_angle + 90.0],
            Pattern::CrossHatch => vec![base + 45.0, base + 135.0],
            Pattern::Triangle => vec![base, base + 60.0, base + 120.0],
            Pattern::Cubic => vec![self.fill_angle + 45.0 * (k % 4) as f64],
            Pattern::ZigZag => vec![base],
            Pattern::Gyroid | Pattern::Honeycomb3D | Pattern::Concentric | Pattern::Honeycomb => {
                Vec::new()
            }
        }
    }

    fn validate(&self) -> bool {
        self.width.is_finite()
            && self.width > 0.0
            && self.solid_width.is_finite()
            && self.solid_width >= 0.0
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

/// Packed layer masks used for skin classification.
///
/// The masks are built when the cache is created, over a closed range of
/// layers, and never change afterwards. That is what lets the caller classify
/// several layers at once from different threads: reading a mask takes `&self`,
/// so the same cache can be shared, where a lazily filled one could not.
///
/// The range has to cover every layer the reader asks about, which means the
/// layers just below the first one read and just above the last, as far as the
/// deepest skin window reaches.
pub struct MaskCache<'a> {
    regions: &'a [Region],
    grid: SkinGrid,
    lo: usize,
    masks: Vec<Vec<u64>>,
}

impl<'a> MaskCache<'a> {
    /// Masks for every layer.
    ///
    /// `capacity` is kept for the callers that sized it, but nothing is evicted:
    /// the whole stack has to be in hand for the layers to be classified in
    /// parallel.
    pub fn new(regions: &'a [Region], res: f64, capacity: usize) -> MaskCache<'a> {
        let hi = regions.len().saturating_sub(1);
        let _ = capacity;
        MaskCache::window(regions, res, 0, hi)
    }

    /// Masks for layers `lo..=hi`.
    pub fn window(regions: &'a [Region], res: f64, lo: usize, hi: usize) -> MaskCache<'a> {
        let grid = SkinGrid::new(regions, res);
        let hi = hi.min(regions.len().saturating_sub(1));
        let lo = lo.min(hi);
        let masks = (lo..=hi).map(|k| grid.packed(&regions[k])).collect();
        MaskCache {
            regions,
            grid,
            lo,
            masks,
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

    /// First layer this cache holds a mask for.
    pub fn first_layer(&self) -> usize {
        self.lo
    }

    /// Packed mask of layer `k` exactly as the slicer produced it.
    pub fn own_bits(&self, k: usize) -> Vec<u64> {
        self.bits(k).to_vec()
    }

    /// Number of masks currently held.
    pub fn resident(&self) -> usize {
        self.masks.len()
    }

    fn bits(&self, k: usize) -> &[u64] {
        match k.checked_sub(self.lo).and_then(|i| self.masks.get(i)) {
            Some(bits) => bits,
            None => {
                debug_assert!(false, "layer {k} is outside the mask window");
                &[]
            }
        }
    }

    /// Bits of layer `k - 1`, empty on the first layer.
    ///
    /// This is the anchor mask: a bridge line is only a bridge when its ends
    /// land on material that the previous layer actually printed.
    pub fn below_bits(&self, k: usize) -> Vec<u64> {
        if k == 0 {
            return vec![0u64; self.grid.words];
        }
        self.bits(k - 1).to_vec()
    }

    /// Bits of layer `k` with nothing printed underneath them.
    ///
    /// These are the cells a bridge has to span. The first layer is exempt
    /// because the bed supports it.
    pub fn air_below_bits(&self, k: usize) -> Vec<u64> {
        let own = self.bits(k).to_vec();
        if k == 0 {
            return vec![0u64; own.len()];
        }
        let below = self.bits(k - 1).to_vec();
        own.iter().zip(&below).map(|(o, b)| o & !b).collect()
    }

    /// Bits of layer `k` whose fill is sparse: inside the outline, not solid
    /// skin, and not bridging over air.
    ///
    /// Combined infill moves the intersection of these across a group of
    /// layers, so the answer has to be the same classification the fill itself
    /// uses; anything else and a combined layer would either print twice or
    /// leave a hole.
    pub fn sparse_own_bits(&self, k: usize, top: usize, bottom: usize) -> Vec<u64> {
        let solid = self.solid_bits(k, top, bottom);
        let air = self.air_below_bits(k);
        let own = self.own_bits(k);
        own.iter()
            .zip(solid.iter().zip(&air))
            .map(|(o, (s, a))| o & !((s & !a) | a))
            .collect()
    }

    /// Bits of layer `k` that form the visible top surface.
    ///
    /// A cell is visible when layer `k` covers it and the layer directly above
    /// does not. That is stricter than the skin rule on purpose: skin is solid
    /// for several layers so the surface has something underneath, but only the
    /// outermost one is the surface a nozzle can actually flatten.
    pub fn top_bits(&self, k: usize) -> Vec<u64> {
        let own = self.bits(k).to_vec();
        if k + 1 >= self.regions.len() {
            return own;
        }
        let above = self.bits(k + 1).to_vec();
        own.iter().zip(&above).map(|(o, a)| o & !a).collect()
    }

    /// Bits of layer `k` that are solid because they sit within `top` layers
    /// of the surface above.
    ///
    /// This is the top-surface half of [`MaskCache::solid_bits`]: the same
    /// coverage test, with the bottom condition dropped. A line inside these
    /// cells is the face the print is judged on, so the writer gives it the
    /// top-surface speed and acceleration when a profile sets them.
    pub fn top_skin_bits(&self, k: usize, top: usize) -> Vec<u64> {
        let words = self.grid.words;
        let real_top = top.min(self.regions.len() - 1 - k);
        let mut cov = vec![u64::MAX; words];
        for j in 1..=real_top {
            let bits = self.bits(k + j);
            for (c, b) in cov.iter_mut().zip(bits) {
                *c &= *b;
            }
        }
        if real_top < top {
            cov.iter_mut().for_each(|w| *w = 0);
        }
        let own = self.bits(k).to_vec();
        own.iter().zip(&cov).map(|(o, c)| o & !c).collect()
    }

    /// Bits marking the part of layer `k` that is neither top nor bottom skin.
    ///
    /// A cell stays sparse only while it remains covered `top` layers above and
    /// `bottom` layers below; anything else becomes solid.
    pub fn solid_bits(&self, k: usize, top: usize, bottom: usize) -> Vec<u64> {
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

#[derive(Clone, Copy)]
struct Ctx<'a> {
    region: &'a Region,
    index: &'a SegIndex,
    grid: &'a SkinGrid,
    mask: &'a [u64],
    thr: f64,
    thr_solid: f64,
    ip: &'a InfillParams,
    all_solid: bool,
    width: f64,
    anchor: f64,
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
    let thr = if want_solid { cx.thr_solid } else { cx.thr };
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
            if cx.index.nearest(p, thr).is_some() {
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
                if cx.anchor <= 0.0 {
                    return 0.0;
                }
                let p = n * c + u * t;
                match cx.index.nearest(p, cx.thr + 1e-3) {
                    Some(hit) => {
                        let nb = (p - hit.foot).norm();
                        let cs = u.dot(nb).abs().max(0.2);
                        let tn = (1.0 - cs * cs).max(0.0).sqrt() / cs;
                        (0.5 * cx.width * tn).min(2.0 * cx.width).min(cx.anchor)
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
                        overhang: 0.0,
                        object: 0,
                        flow: 1.0,
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

/// One corner of a flat-topped hexagon, at sixty-degree steps.
fn hex_corner(centre: V2, side: f64, k: usize) -> V2 {
    let angle = std::f64::consts::FRAC_PI_3 * k as f64;
    centre + V2::new(side * angle.cos(), side * angle.sin())
}

fn straight(a: V2, b: V2, width: f64, kind: Kind) -> Line {
    Line {
        pts: vec![a, b],
        width: vec![width; 2],
        closed: false,
        kind,
        overhang: 0.0,
        object: 0,
        flow: 1.0,
    }
}

/// A flat hexagonal lattice, drawn as the hexagons' own sides.
///
/// Every side is shared by two cells, so the bead length per unit area is
/// `1 / (side * sqrt(3) / 2)`; setting that equal to the density over the bead
/// width gives the side length below, which is upstream's own relation. Cells are
/// laid out on a flat-topped lattice and each side is clipped to the fill mask, so
/// a hexagon straddling the region's edge contributes the part of itself that is
/// inside rather than nothing.
fn honeycomb_fill(cx: &Ctx, spacing: f64) -> Vec<Line> {
    let side = 2.0 / 3.0f64.sqrt() * spacing;
    let rb = cx.region.bounds();
    let (dx, dy) = (1.5 * side, 3.0f64.sqrt() * side);
    let i0 = ((rb.min.x - 2.0 * side) / dx).floor() as i64;
    let i1 = ((rb.max.x + 2.0 * side) / dx).ceil() as i64;
    let j0 = ((rb.min.y - 2.0 * side) / dy).floor() as i64;
    let j1 = ((rb.max.y + 2.0 * side) / dy).ceil() as i64;
    let mut out = Vec::new();
    let mut drawn: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();
    for i in i0..=i1 {
        for j in j0..=j1 {
            let centre = V2::new(i as f64 * dx, (j as f64 + 0.5 * (i & 1) as f64) * dy);
            let mut previous = hex_corner(centre, side, 5);
            for corner in 0..6 {
                let next = hex_corner(centre, side, corner);
                let mid = (previous + next) * 0.5;
                let key = (
                    (mid.x * 1000.0).round() as i64,
                    (mid.y * 1000.0).round() as i64,
                );
                if drawn.insert(key) {
                    side_spans(cx, previous, next, &mut out);
                }
                previous = next;
            }
        }
    }
    out
}

/// Emits the part of one straight side that lies inside the fill mask.
///
/// A side wholly inside is one bead and costs three point tests; only a side that
/// crosses the boundary is sampled, because a lattice over a 200 mm plate has
/// thousands of sides and each sampled side is thirty lookups.
fn side_spans(cx: &Ctx, a: V2, b: V2, out: &mut Vec<Line>) {
    let inside = |p: V2| cx.grid.is_set(cx.mask, p) && cx.index.nearest(p, cx.thr).is_none();
    if inside(a) && inside(b) && inside((a + b) * 0.5) {
        out.push(straight(a, b, cx.width, Kind::Infill));
        return;
    }
    let length = a.dist(b);
    if length <= 0.0 {
        return;
    }
    let at = |t: f64| a + (b - a) * t;
    let steps = ((length / cx.ip.step).ceil() as usize).max(1);
    let mut start = inside(a).then_some(0.0);
    let mut previous = 0.0;
    let mut previous_inside = start.is_some();
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        let here = inside(at(t));
        if here != previous_inside {
            let (mut lo, mut hi) = (previous, t);
            for _ in 0..18 {
                let mid = 0.5 * (lo + hi);
                if inside(at(mid)) == previous_inside {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let edge = 0.5 * (lo + hi);
            if previous_inside {
                if let Some(s) = start.take() {
                    if (edge - s) * length >= cx.width {
                        out.push(straight(at(s), at(edge), cx.width, Kind::Infill));
                    }
                }
            } else {
                start = Some(edge);
            }
        }
        previous = t;
        previous_inside = here;
    }
    if let Some(s) = start {
        if (1.0 - s) * length >= cx.width {
            out.push(straight(at(s), b, cx.width, Kind::Infill));
        }
    }
}

/// Joins a family's lines end to end, so a zig-zag is one path per run.
///
/// The connector is tested against the mask before it is drawn: two spans of one
/// scanline can be separated by a hole, and joining those would lay a bead across
/// it. This is also why a zig-zag skips the shell walk — the join *is* the
/// connection, and doing both would walk the wall and then cross the part.
fn link_zigzag(lines: Vec<Line>, cx: &Ctx, max_gap: f64, anchor: f64) -> Vec<Line> {
    let inside = |p: V2| cx.grid.is_set(cx.mask, p) && cx.index.nearest(p, cx.thr).is_none();
    let mut out = Vec::new();
    let mut run: Option<Line> = None;
    for line in lines {
        let Some(mut joined) = run.take() else {
            run = Some(line);
            continue;
        };
        let end = *joined.pts.last().unwrap();
        let (to_first, to_last) = (end.dist(line.pts[0]), end.dist(*line.pts.last().unwrap()));
        if to_first.min(to_last) > max_gap {
            out.push(joined);
            run = Some(line);
            continue;
        }
        let mut pts = line.pts.clone();
        if to_last < to_first {
            pts.reverse();
        }
        if !inside((end + pts[0]) * 0.5) {
            out.push(joined);
            run = Some(line);
            continue;
        }
        joined.pts.append(&mut pts);
        let n = joined.pts.len();
        joined.width = vec![joined.width[0]; n];
        run = Some(joined);
    }
    if let Some(joined) = run {
        out.push(joined);
    }
    if anchor > 0.0 {
        for run in &mut out {
            let first = run.pts[0];
            if let Some(&second) = run.pts.get(1) {
                if first.dist(second) > 1e-9 {
                    run.pts[0] = first + (first - second).norm() * anchor;
                }
            }
            let n = run.pts.len();
            let last = run.pts[n - 1];
            if n >= 2 && last.dist(run.pts[n - 2]) > 1e-9 {
                run.pts[n - 1] = last + (last - run.pts[n - 2]).norm() * anchor;
            }
        }
    }
    out
}

/// Where each boundary segment sits: which ring, and the vertex it starts at.
///
/// A segment index from `SegIndex` is an index into `Region::segments()`, which
/// walks the rings in order, so this is the map back to the ring geometry.
struct RingMap {
    at: Vec<(usize, usize)>,
}

impl RingMap {
    fn new(rings: &[Vec<V2>]) -> RingMap {
        let mut at = Vec::new();
        for (r, ring) in rings.iter().enumerate() {
            for i in 0..ring.len() {
                at.push((r, i));
            }
        }
        RingMap { at }
    }
}

/// The boundary a connect bead walks, and the index that finds it.
///
/// The walk has to happen on the *inner* edge of the wall band, not on the part
/// outline: a bead laid on the outline is a second pass over the outermost wall,
/// which is the face the print is judged on. `thr` is the same inward offset the
/// sparse mask is drawn from, so the walk sits exactly where the infill stops.
struct Walk {
    rings: Vec<Vec<V2>>,
    map: RingMap,
    index: SegIndex,
}

fn anchor_walk(region: &Region, thr: f64, ip: &InfillParams) -> Option<Walk> {
    if ip.anchor <= 0.0 || ip.anchor_max <= 0.0 || thr <= 0.0 || region.rings.is_empty() {
        return None;
    }
    let index = SegIndex::new(
        region.segments(),
        region.bounds().grown(2.0 * thr),
        (0.5 * thr).max(0.3),
    );
    Some(Walk {
        map: RingMap::new(&region.rings),
        rings: region.rings.clone(),
        index,
    })
}

/// The area inside the wall band: what the sparse fill is drawn on.
///
/// This is the region `solid_infill_below_area` is measured against. Upstream
/// applies that threshold to its fill surfaces, which are the layer's area after
/// the perimeters come off, not to the part outline: an arm 3 mm wide and 30 mm
/// long is a 1500 mm² outline but a 36 mm² infill strip, and it is the strip that
/// decides whether the interior is printed solid.
fn inset_region(region: &Region, thr: f64) -> Region {
    if thr <= 0.0 || region.is_empty() {
        return region.clone();
    }
    hfw_walls::offset_region(region, -thr, (0.25 * thr).max(0.05))
}

/// The ring vertices from one foot to another, feet included, along the shorter way.
///
/// This is the segment that turns two parallel infill lines into one path: the
/// nozzle leaves the first bead, runs along the wall for one spacing, and comes
/// back on the second, so the two lines are joined by a single perimeter bead
/// instead of two separate anchors with a travel between them. The cap is what
/// keeps it from walking the whole outline: a pair whose feet are further apart
/// than `cap` is left alone.
fn ring_walk(rings: &[Vec<V2>], map: &RingMap, a: &Hit, b: &Hit, cap: f64) -> Option<Vec<V2>> {
    let (ra, ia) = *map.at.get(a.seg as usize)?;
    let (rb, ib) = *map.at.get(b.seg as usize)?;
    if ra != rb {
        return None;
    }
    let ring = &rings[ra];
    let n = ring.len();
    if n < 3 {
        return None;
    }
    let forward = || -> (f64, Vec<V2>) {
        let mut out = vec![a.foot];
        let mut len = 0.0;
        let mut at = a.foot;
        let mut i = ia;
        loop {
            let next = (i + 1) % n;
            if next == (ib + 1) % n {
                break;
            }
            let p = ring[next];
            len += at.dist(p);
            out.push(p);
            at = p;
            i = next;
        }
        len += at.dist(b.foot);
        out.push(b.foot);
        (len, out)
    };
    let backward = || -> (f64, Vec<V2>) {
        let mut out = vec![a.foot];
        let mut len = 0.0;
        let mut at = a.foot;
        let mut i = ia;
        loop {
            if i == ib {
                break;
            }
            let p = ring[i];
            len += at.dist(p);
            out.push(p);
            at = p;
            i = (i + n - 1) % n;
        }
        len += at.dist(b.foot);
        out.push(b.foot);
        (len, out)
    };
    let (fwd_len, fwd) = forward();
    let (back_len, back) = backward();
    let (len, walk) = if fwd_len <= back_len {
        (fwd_len, fwd)
    } else {
        (back_len, back)
    };
    (len <= cap).then(|| simplify(&walk, 0.04))
}

/// Drops walk vertices that sit within `tol` of the line through their neighbours.
///
/// The offset ring is traced on a grid, so it arrives with a vertex every few
/// tenths of a millimetre. Those vertices are accurate, but a bead does not need
/// them: they would triple the move count of every layer for a run that is two
/// or three millimetres long, and they would tie the file's size to the offset's
/// grid rather than to the part.
fn simplify(points: &[V2], tol: f64) -> Vec<V2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let (a, b) = (points[0], points[points.len() - 1]);
    let span = b - a;
    let length = a.dist(b);
    let mut far = 0.0;
    let mut at = 0;
    for (i, &p) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let d = if length < 1e-9 {
            p.dist(a)
        } else {
            let (sx, sy) = (span.x, span.y);
            ((a.x - p.x) * sy - (a.y - p.y) * sx).abs() / length
        };
        if d > far {
            far = d;
            at = i;
        }
    }
    if far <= tol {
        return vec![a, b];
    }
    let mut left = simplify(&points[..=at], tol);
    let right = simplify(&points[at..], tol);
    left.pop();
    left.extend(right);
    left
}

/// Joins adjacent infill lines whose facing ends land on the same wall.
///
/// `lines` arrives in the order the family emits it, one line per spacing step,
/// so neighbours in the vector are neighbours on the part. A run then snakes:
/// the first line's far end walks the wall for one spacing, comes back on the
/// second, whose far end walks the wall again for the third. Each step tries
/// both of the next line's ends, so the join happens on whichever side the two
/// feet actually share, and the cap leaves a pair alone when it does not.
/// Appends a vertex unless it sits on the one before it.
///
/// The offset ring is traced at grid resolution, so consecutive vertices can be
/// a fraction of a bead apart, and the infill end usually lands almost exactly on
/// the foot the walk stopped at. A zero-length segment is not a printing move but
/// it does carry a feed and an E value, and the verify pass reads those as an
/// extruded width near zero.
fn push_vertex(line: &mut Line, p: V2, w: f64) {
    if let Some(last) = line.pts.last() {
        if last.dist(p) < 1e-3 {
            return;
        }
    }
    line.pts.push(p);
    line.width.push(w);
}

/// How much shell a layer's sparse fill may spend walking along the wall.
///
/// The walk exists to turn two fill lines into one continuous path, which saves a
/// retraction and a travel. It costs a bead laid over the inner perimeter that is
/// already printed there, so the material is spent twice on the same spot. That is
/// worth it at the rate of one spacing of wall per pair of lines; it is not worth
/// it on a closed shell, where every line end has a neighbour along the wall and a
/// greedy walk chains the whole layer into one long bead around the part. The
/// budget is the fraction of the fill's own path length a layer may spend that
/// way, which scales with the fill being served instead of with the wall's size.
const WALK_SHARE: f64 = 1.0;

/// Total printed length of a run of lines, in millimetres.
fn total_length(lines: &[Line]) -> f64 {
    lines
        .iter()
        .map(|line| line.pts.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>())
        .sum()
}

fn join_shell_ends(
    lines: &mut Vec<Line>,
    walk: &Walk,
    cap: f64,
    budget: f64,
    air: Option<(&[u64], &SkinGrid)>,
) {
    if lines.len() < 2 || cap <= 0.0 || budget <= 0.0 {
        return;
    }
    let supported = |p: V2| match air {
        Some((mask, grid)) => !grid.is_set(mask, p),
        None => true,
    };
    let (rings, map, index) = (&walk.rings, &walk.map, &walk.index);
    let reach = 2.0 * lines[0].width[0] + 2e-3;
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    let mut current = lines[0].clone();
    let mut spent = 0.0;
    for line in lines.iter().skip(1) {
        let last = *current.pts.last().unwrap_or(&line.pts[0]);
        let from = index.nearest(last, reach);
        let mut merged = false;
        if let Some(from) = &from {
            let mut best: Option<(f64, usize, Vec<V2>)> = None;
            for near in [1usize, 0] {
                let Some(hit) = index.nearest(line.pts[near], reach) else {
                    continue;
                };
                let Some(walked) = ring_walk(rings, map, from, &hit, cap) else {
                    continue;
                };
                if !walked.iter().all(|&p| supported(p)) {
                    continue;
                }
                let len: f64 = walked.windows(2).map(|w| w[0].dist(w[1])).sum();
                if best.as_ref().is_none_or(|(shortest, _, _)| len < *shortest) {
                    best = Some((len, near, walked));
                }
            }
            if let Some((len, near, walk)) = best {
                if spent + len <= budget {
                    let width = line.width[0];
                    for &p in walk.iter().skip(1) {
                        push_vertex(&mut current, p, width);
                    }
                    let order = if near == 1 { [1usize, 0] } else { [0, 1] };
                    for k in order {
                        let w = line.width[k.min(line.width.len() - 1)];
                        push_vertex(&mut current, line.pts[k], w);
                    }
                    spent += len;
                    merged = true;
                }
            }
        }
        if !merged {
            out.push(std::mem::replace(&mut current, line.clone()));
        }
    }
    out.push(current);
    *lines = out;
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
///
/// `z` is the layer's print height: the curved patterns' shapes follow it, which
/// is what makes them interlock between layers. The line families ignore it.
#[allow(clippy::too_many_arguments)]
pub fn infill_layer(
    region: &Region,
    cache: &MaskCache,
    k: usize,
    z: f64,
    p: &WallParams,
    ip: &InfillParams,
    combine: Option<&Combine>,
) -> Vec<Line> {
    if region.rings.is_empty() || !ip.validate() {
        return Vec::new();
    }
    let (win_top, win_bottom) = ip.window();
    let solid = cache.solid_bits(k, win_top, win_bottom);
    let top_skin = cache.top_skin_bits(k, ip.top_layers);
    let air = cache.air_below_bits(k);
    let below = cache.below_bits(k);
    let grid = *cache.grid();
    let thr = ip.inner_offset(p);
    let thr_solid = ip.skin_offset(p);
    let rb = region.bounds();
    let index = SegIndex::new(
        region.segments(),
        rb.grown(thr + ip.step + 1.0),
        (0.5 * thr).max(0.3),
    );
    let all_solid = ip.density >= 0.999;
    let inset = inset_region(region, thr);
    let inner = anchor_walk(&inset, thr, ip);
    let mut covered = solid.clone();
    for (c, a) in covered.iter_mut().zip(&air) {
        *c &= !*a;
    }
    let mut sparse = covered.clone();
    for (s, a) in sparse.iter_mut().zip(&air) {
        *s |= *a;
    }
    if let Some(combine) = combine {
        for (s, strip) in sparse.iter_mut().zip(combine.strip) {
            *s |= *strip;
        }
    }
    let mut covered_top = covered.clone();
    for (c, t) in covered_top.iter_mut().zip(&top_skin) {
        *c &= *t;
    }
    let mut covered_bottom = covered.clone();
    for (c, t) in covered_bottom.iter_mut().zip(&top_skin) {
        *c &= !*t;
    }
    let small = if !all_solid && ip.density > 0.0 && ip.solid_below_area > 0.0 {
        small_island_bits(&inset, &grid, ip.solid_below_area)
    } else {
        None
    };
    let every = ip.solid_every > 0 && k % ip.solid_every == 0;
    let mut forced = vec![0u64; sparse.len()];
    if every && !all_solid && ip.density > 0.0 {
        let own = grid.packed(region);
        for (word, (own, skip)) in forced.iter_mut().zip(own.iter().zip(&sparse)) {
            *word = *own & !*skip;
        }
    }
    if let Some(small) = &small {
        for (word, bits) in forced.iter_mut().zip(small) {
            *word |= *bits;
        }
    }
    if !all_solid {
        for (skip, word) in sparse.iter_mut().zip(&forced) {
            *skip |= *word;
        }
    }
    let forced_solid = !all_solid && forced.iter().any(|word| *word != 0);
    let (mut covered_top, mut covered_bottom) = (covered_top, covered_bottom);
    if forced_solid {
        for (t, f) in covered_top.iter_mut().zip(&forced) {
            *t &= !*f;
        }
        for (b, f) in covered_bottom.iter_mut().zip(&forced) {
            *b &= !*f;
        }
    }
    let cx = Ctx {
        region,
        index: &index,
        grid: &grid,
        mask: &covered,
        thr,
        thr_solid,
        ip,
        all_solid,
        width: ip.width,
        anchor: ip.anchor_length(),
    };
    let cx_sparse = Ctx {
        mask: &sparse,
        ..cx
    };
    let solid_width = ip.solid_width();
    let top_width = ip.top_solid_width();
    let mut lines = Vec::new();
    if all_solid {
        let cx_solid = Ctx {
            width: top_width,
            ..cx
        };
        let filled = if ip.top_pattern == SkinPattern::Concentric {
            concentric_skin(&grid, &sparse, top_width, top_width, Kind::Solid)
        } else if matches!(
            ip.top_pattern,
            SkinPattern::HilbertCurve
                | SkinPattern::ArchimedeanChords
                | SkinPattern::OctagramSpiral
        ) {
            curve_skin(&grid, &sparse, ip.top_pattern, top_width)
        } else {
            let mut out = Vec::new();
            for deg in skin_angles(ip.top_pattern, k) {
                out.extend(family(&cx_solid, deg.to_radians(), top_width, true));
            }
            out
        };
        lines.extend(filled);
    } else {
        for (mask, pattern, width) in [
            (&covered_bottom, ip.bottom_pattern, solid_width),
            (&covered_top, ip.top_pattern, top_width),
        ] {
            if mask.iter().all(|w| *w == 0) {
                continue;
            }
            let cx_solid = Ctx {
                mask,
                width,
                all_solid: false,
                ..cx
            };
            let filled = if pattern == SkinPattern::Concentric {
                concentric_skin(&grid, mask, width, width, Kind::Solid)
            } else if matches!(
                pattern,
                SkinPattern::HilbertCurve
                    | SkinPattern::ArchimedeanChords
                    | SkinPattern::OctagramSpiral
            ) {
                curve_skin(&grid, mask, pattern, width)
            } else {
                let mut out = Vec::new();
                for deg in skin_angles(pattern, k) {
                    out.extend(family(&cx_solid, deg.to_radians(), width, true));
                }
                out
            };
            lines.extend(filled);
        }
    }
    if forced_solid {
        let only_sparse = forced.clone();
        let cx_forced = Ctx {
            mask: &only_sparse,
            width: top_width,
            ..cx
        };
        let filled = if ip.top_pattern == SkinPattern::Concentric {
            concentric_skin(&grid, &only_sparse, top_width, top_width, Kind::Solid)
        } else if matches!(
            ip.top_pattern,
            SkinPattern::HilbertCurve
                | SkinPattern::ArchimedeanChords
                | SkinPattern::OctagramSpiral
        ) {
            curve_skin(&grid, &only_sparse, ip.top_pattern, top_width)
        } else {
            let mut out = Vec::new();
            for deg in skin_angles(ip.top_pattern, k) {
                out.extend(family(&cx_forced, deg.to_radians(), top_width, true));
            }
            out
        };
        lines.extend(filled);
    }
    if air.iter().any(|w| *w != 0) {
        let cx_bridge = Ctx {
            region,
            index: &index,
            grid: &grid,
            mask: &air,
            thr,
            thr_solid,
            ip,
            all_solid: false,
            width: ip.bridge.width,
            anchor: ip.anchor_length(),
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
        let spacing = ip.sparse_spacing();
        if ip.is_curved() {
            let rb = region.bounds();
            let cell = ip.pattern_cell().unwrap_or(0.0);
            let origin = if cell > 0.0 {
                V2::new(
                    (rb.min.x / cell).floor() * cell,
                    (rb.min.y / cell).floor() * cell,
                )
            } else {
                rb.min
            };
            let (width, height) = (rb.max.x - origin.x, rb.max.y - origin.y);
            let curves = match ip.pattern {
                Pattern::Gyroid => curves::gyroid(width, height, ip.width, ip.density, z, origin),
                _ => curves::honeycomb3d(width, height, ip.width, ip.density, z, k, origin),
            };
            let clip = curves::Clip {
                index: &index,
                thr,
                grid: &grid.grid,
                excluded: &sparse,
                step: ip.step,
                anchor: ip.anchor_length(),
                width: ip.width,
            };
            lines.extend(curves::curved(&clip, &curves, ip.width));
        }
        let own = grid.packed(region);
        let free: Vec<u64> = own
            .iter()
            .zip(&sparse)
            .map(|(part, skip)| *part & !*skip)
            .collect();
        let cx_free = Ctx {
            mask: &free,
            ..cx_sparse
        };
        if ip.pattern == Pattern::Concentric {
            lines.extend(concentric_skin(
                &grid,
                &free,
                spacing,
                ip.width,
                Kind::Infill,
            ));
        } else if ip.pattern == Pattern::Honeycomb {
            lines.extend(honeycomb_fill(&cx_free, spacing));
        }
        for angle in ip.sparse_angles(k) {
            let mut family_lines = if ip.pattern == Pattern::ZigZag {
                let cx_bare = Ctx {
                    anchor: 0.0,
                    ..cx_sparse
                };
                let bare = family(&cx_bare, angle.to_radians(), spacing, false);
                link_zigzag(bare, &cx_free, 1.6 * spacing, ip.anchor_length())
            } else {
                let mut runs = family(&cx_sparse, angle.to_radians(), spacing, false);
                if let Some(inner) = &inner {
                    let budget = WALK_SHARE * total_length(&runs);
                    join_shell_ends(&mut runs, inner, ip.anchor_max, budget, Some((&air, &grid)));
                }
                runs
            };
            lines.append(&mut family_lines);
        }
    }
    if let Some(combine) = combine {
        if combine.layers > 1 && combine.extra.iter().any(|w| *w != 0) {
            let not_extra: Vec<u64> = combine.extra.iter().map(|word| !*word).collect();
            let cx_combined = Ctx {
                mask: &not_extra,
                width: ip.width,
                all_solid: false,
                ..cx
            };
            let spacing = ip.sparse_spacing();
            for angle in ip.sparse_angles(k) {
                let mut combined_lines = family(&cx_combined, angle.to_radians(), spacing, false);
                if let Some(inner) = &inner {
                    let budget = WALK_SHARE * total_length(&combined_lines);
                    join_shell_ends(
                        &mut combined_lines,
                        inner,
                        ip.anchor_max,
                        budget,
                        Some((&air, &grid)),
                    );
                }
                for line in &mut combined_lines {
                    line.flow = combine.layers as f64;
                }
                lines.extend(combined_lines);
            }
        }
    }
    let top_skin = cache.top_skin_bits(k, ip.top_layers);
    mark_top_skin(&mut lines, &top_skin, &grid.grid);
    lines
}

/// Cells that belong to an island too small to print sparse.
///
/// The reference dialect applies the threshold to each *island*, not to the
/// layer: a small part prints solid, and a small boss standing on a large base
/// prints solid while the base around it stays sparse. Islands are the
/// even-depth rings, each with the odd-depth rings that hang directly off it;
/// a ring's parent is the smallest ring containing it, which is what separates
/// a hole in one island from a hole in a neighbour.
fn small_island_bits(region: &Region, grid: &SkinGrid, threshold: f64) -> Option<Vec<u64>> {
    let n = region.rings.len();
    if n == 0 {
        return None;
    }
    let areas: Vec<f64> = region
        .rings
        .iter()
        .map(|ring| hfw_kernel::ring_signed_area(ring).abs())
        .collect();
    let mut depth = vec![0usize; n];
    let mut parent = vec![usize::MAX; n];
    for i in 0..n {
        let probe = region.rings[i][0];
        for j in 0..n {
            if i == j || !hfw_kernel::ring_contains(&region.rings[j], probe) {
                continue;
            }
            depth[i] += 1;
            if parent[i] == usize::MAX || areas[j] < areas[parent[i]] {
                parent[i] = j;
            }
        }
    }
    let mut mask = vec![0u64; grid.words];
    let mut any = false;
    for island in 0..n {
        if depth[island] % 2 != 0 {
            continue;
        }
        let mut rings: Vec<Vec<V2>> = vec![region.rings[island].clone()];
        let mut net = areas[island];
        for hole in 0..n {
            if depth[hole] % 2 == 1 && parent[hole] == island {
                net -= areas[hole];
                rings.push(region.rings[hole].clone());
            }
        }
        if net > threshold || net <= 0.0 {
            continue;
        }
        let packed = pack(&scan_inside(&Region::new(rings), &grid.grid));
        for (word, bits) in mask.iter_mut().zip(&packed) {
            *word |= *bits;
        }
        any = true;
    }
    any.then_some(mask)
}

/// The directions a skin pattern prints on layer `k`.
///
/// A solid surface is short beads, so the pattern only decides which way they
/// run: flipping between layers (rectilinear) makes a solid block cross itself,
/// keeping one direction (monotonic) leaves the nozzle sweeping the same way
/// across the face every time, and never flipping (aligned) keeps a tall solid
/// column drawn the same way from bottom to top.
fn skin_angles(pattern: SkinPattern, k: usize) -> Vec<f64> {
    match pattern {
        SkinPattern::Rectilinear => {
            if k % 2 == 0 {
                vec![45.0]
            } else {
                vec![135.0]
            }
        }
        SkinPattern::Monotonic | SkinPattern::MonotonicLines => vec![45.0],
        SkinPattern::AlignedRectilinear => vec![45.0],
        SkinPattern::Concentric
        | SkinPattern::HilbertCurve
        | SkinPattern::ArchimedeanChords
        | SkinPattern::OctagramSpiral => Vec::new(),
    }
}

/// Draws `mask` as loops inset from its own boundary, `spacing` apart.
///
/// The loops are the iso-lines of a chamfer distance transform of the mask, so a
/// skin with a hole gets loops inside the hole as well, and a concave corner is
/// followed rather than spanned. Level `j` sits half a bead in from the edge,
/// which is what makes the first loop touch the surface on both sides.
fn concentric_skin(
    grid: &SkinGrid,
    mask: &[u64],
    spacing: f64,
    width: f64,
    kind: Kind,
) -> Vec<Line> {
    concentric_skin_capped(grid, mask, spacing, width, kind, usize::MAX)
}

/// The same loops, but at most `levels` of them.
fn concentric_skin_capped(
    grid: &SkinGrid,
    mask: &[u64],
    spacing: f64,
    width: f64,
    kind: Kind,
    layers_cap: usize,
) -> Vec<Line> {
    let g = &grid.grid;
    if !(spacing.is_finite() && spacing > 0.0) || mask.iter().all(|w| *w == 0) {
        return Vec::new();
    }
    let set: Vec<bool> = (0..g.len())
        .map(|id| (mask[id / 64] >> (id % 64)) & 1 == 1)
        .collect();
    let mut dist = chamfer_cells(g, &set);
    let ceiling = (g.nx + g.ny) as f64;
    for d in dist.iter_mut() {
        if !d.is_finite() {
            *d = ceiling;
        }
    }
    let deepest = dist.iter().cloned().fold(0.0f64, f64::max) * g.h;
    let levels = ((deepest / spacing).floor() as usize)
        .min(g.nx.max(g.ny))
        .min(layers_cap);
    let nan = vec![f64::NAN; g.len()];
    let mut out = Vec::new();
    for j in 0..levels {
        let target = (j as f64 + 0.5) * spacing;
        let phi: Vec<f64> = dist.iter().map(|d| d * g.h - target).collect();
        for raw in hfw_walls::contour::trace(g, &phi, &nan, &nan) {
            if raw.pts.len() < 2 {
                continue;
            }
            let n = raw.pts.len();
            let line = Line {
                pts: raw.pts,
                width: vec![width; n],
                closed: raw.closed,
                kind,
                overhang: 0.0,
                object: 0,
                flow: 1.0,
            };
            if line.length() < width {
                continue;
            }
            out.push(line);
        }
    }
    out
}

/// Draws `mask` with one of the plane-path curves, clipped to the mask.
fn curve_skin(grid: &SkinGrid, mask: &[u64], pattern: SkinPattern, width: f64) -> Vec<Line> {
    let mut bounds = hfw_kernel::Bounds::empty();
    let g = &grid.grid;
    for iy in 0..g.ny {
        for ix in 0..g.nx {
            let id = iy * g.nx + ix;
            if (mask[id / 64] >> (id % 64)) & 1 == 1 {
                bounds.add(g.pos(ix, iy));
            }
        }
    }
    if bounds.is_empty() {
        return Vec::new();
    }
    bounds = bounds.grown(width);
    let path = match pattern {
        SkinPattern::HilbertCurve => curves::hilbert(bounds, width),
        SkinPattern::ArchimedeanChords => curves::archimedean(bounds, width),
        SkinPattern::OctagramSpiral => curves::octagram(bounds, width),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for run in curves::clip_to_mask(g, mask, &path, width) {
        if run.len() < 2 {
            continue;
        }
        let n = run.len();
        out.push(Line {
            pts: run,
            width: vec![width; n],
            closed: false,
            kind: Kind::Solid,
            overhang: 0.0,
            object: 0,
            flow: 1.0,
        });
    }
    out
}

/// One perimeter loop hugging the part of layer `k` that hangs over air.
///
/// Upstream's "extra perimeters on overhangs" exists because a bridge anchor and
/// a leaning wall both want a loop the ordinary wall band does not put there: the
/// band follows the outline, while the unsupported crescent is a feature of its
/// own. The loop is the iso-line of the overhang mask one bead in from its own
/// boundary, so it follows the crescent rather than the part, and it is emitted
/// as the outermost side loop so the writer gives it the outer-wall speed.
pub fn overhang_perimeter(
    region: &Region,
    cache: &MaskCache,
    k: usize,
    p: &WallParams,
    ip: &InfillParams,
) -> Vec<Line> {
    if k == 0 || region.is_empty() || !ip.validate() {
        return Vec::new();
    }
    let own = cache.own_bits(k);
    let below = cache.below_bits(k);
    let mut overhang: Vec<u64> = own.iter().zip(&below).map(|(o, b)| o & !*b).collect();
    let thr = ip.inner_offset(p);
    let index = SegIndex::new(
        region.segments(),
        region.bounds().grown(thr + ip.step + 1.0),
        (0.5 * thr).max(0.3),
    );
    let grid = *cache.grid();
    let g = &grid.grid;
    let mut kept = 0usize;
    for (iy, ix) in (0..g.ny).flat_map(|iy| (0..g.nx).map(move |ix| (iy, ix))) {
        let id = iy * g.nx + ix;
        if (overhang[id / 64] >> (id % 64)) & 1 == 0 {
            continue;
        }
        if index.nearest(g.pos(ix, iy), thr).is_some() {
            overhang[id / 64] &= !(1u64 << (id % 64));
        } else {
            kept += 1;
        }
    }
    if kept < 8 {
        return Vec::new();
    }
    let width = p.outer_width();
    let mut out = concentric_skin_capped(&grid, &overhang, width, width, Kind::Side(0), 1);
    for line in &mut out {
        line.overhang = 1.0;
    }
    out
}

/// Distance in grid cells from every set cell to the nearest unset one.
///
/// Two-pass chamfer with the exact-neighbour weights: four orthogonal passes of
/// one cell and four diagonal ones of the square root of two. The approximation
/// is off by at most two percent, which is under the clearance between two
/// beads, so a loop traced on it lands where a loop traced on an exact field
/// would.
fn chamfer_cells(g: &Grid, set: &[bool]) -> Vec<f64> {
    const DIAG: f64 = std::f64::consts::SQRT_2;
    let (nx, ny) = (g.nx, g.ny);
    let mut d = vec![f64::INFINITY; nx * ny];
    for id in 0..nx * ny {
        if !set[id] {
            d[id] = 0.0;
        }
    }
    let relax = |d: &mut Vec<f64>, id: usize, from: usize, step: f64| {
        let candidate = d[from] + step;
        if candidate < d[id] {
            d[id] = candidate;
        }
    };
    for iy in 0..ny {
        for ix in 0..nx {
            let id = iy * nx + ix;
            if ix > 0 {
                relax(&mut d, id, id - 1, 1.0);
            }
            if iy > 0 {
                relax(&mut d, id, id - nx, 1.0);
            }
            if ix > 0 && iy > 0 {
                relax(&mut d, id, id - nx - 1, DIAG);
            }
            if ix + 1 < nx && iy > 0 {
                relax(&mut d, id, id - nx + 1, DIAG);
            }
        }
    }
    for iy in (0..ny).rev() {
        for ix in (0..nx).rev() {
            let id = iy * nx + ix;
            if ix + 1 < nx {
                relax(&mut d, id, id + 1, 1.0);
            }
            if iy + 1 < ny {
                relax(&mut d, id, id + nx, 1.0);
            }
            if ix + 1 < nx && iy + 1 < ny {
                relax(&mut d, id, id + nx + 1, DIAG);
            }
            if ix > 0 && iy + 1 < ny {
                relax(&mut d, id, id + nx - 1, DIAG);
            }
        }
    }
    d
}

/// Promotes solid lines that land on the visible top surface to `TopSolid`.
///
/// Each line is judged at its midpoint: solid lines here are one bead wide and
/// shorter than a few beads, so a line straddling the skin boundary is off by
/// at most one bead on the last solid layer.
fn mark_top_skin(lines: &mut [Line], top_skin: &[u64], grid: &Grid) {
    for line in lines.iter_mut() {
        if line.kind != Kind::Solid || line.pts.is_empty() {
            continue;
        }
        let mid = midpoint(line);
        let ix = ((mid.x - grid.origin.x) / grid.h).floor() as i64;
        let iy = ((mid.y - grid.origin.y) / grid.h).floor() as i64;
        if cell_set(top_skin, grid, ix, iy) {
            line.kind = Kind::TopSolid;
        }
    }
}

fn midpoint(line: &Line) -> V2 {
    let first = line.pts[0];
    let last = *line.pts.last().unwrap_or(&first);
    V2::new(0.5 * (first.x + last.x), 0.5 * (first.y + last.y))
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
            if visited[id] || !cell_set(air, &g, ix as i64, iy as i64) {
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
                    if visited[uid] || !cell_set(air, &g, ux as i64, uy as i64) {
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

fn cell_set(mask: &[u64], g: &Grid, ix: i64, iy: i64) -> bool {
    if ix < 0 || iy < 0 || ix >= g.nx as i64 || iy >= g.ny as i64 {
        return false;
    }
    let id = iy as usize * g.nx + ix as usize;
    (mask[id / 64] >> (id % 64)) & 1 == 1
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
        cell_set(air, g, ix, iy)
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
        overhang: 0.0,
        object: 0,
        flow: 1.0,
    })
}

/// Which surfaces an ironing pass covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IroningTarget {
    /// Every visible top skin in the stack.
    TopSurfaces,
    /// Only the highest visible top skin.
    TopmostOnly,
    /// Every solid surface, top and bottom.
    AllSolid,
}

/// Ironing passes over the visible top surface of layer `k`.
///
/// Ironing is not another skin fill: it runs after the skin has already been
/// laid, at a fraction of the flow, with the beads nearly touching, so the nozzle
/// drags across the plastic and flattens it. That is why the spacing is its own
/// number (`ironing_spacing`, a tenth of a millimetre by default) instead of the
/// skin width: a wider pass would not press anything flat.
///
/// [`IroningTarget::AllSolid`] irons every solid surface of the layer rather than
/// the visible top only, which is what PrusaSlicer's "all solid layers" option
/// means; the other two targets differ in which layer the caller runs this on.
pub fn ironing_layer(
    region: &Region,
    cache: &MaskCache,
    k: usize,
    ip: &InfillParams,
    spacing: f64,
    target: IroningTarget,
) -> Vec<Line> {
    if region.rings.is_empty() {
        return Vec::new();
    }
    let mask = if target == IroningTarget::AllSolid {
        let (win_top, win_bottom) = ip.window();
        cache.solid_bits(k, win_top, win_bottom)
    } else {
        cache.top_bits(k)
    };
    if mask.iter().all(|w| *w == 0) {
        return Vec::new();
    }
    let width = ip.solid_width();
    let angle = if k % 2 == 0 { 45.0 } else { 135.0 };
    fill_from_mask(
        cache.grid(),
        &mask,
        width,
        spacing,
        angle,
        Kind::Ironing,
        width / 4.0,
    )
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
        overhang: 0.0,
        object: 0,
        flow: 1.0,
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
    fn every_pattern_covers_its_own_directions_and_no_others() {
        let cases: [(Pattern, Vec<f64>, Vec<f64>); 4] = [
            (Pattern::Rectilinear, vec![45.0], vec![135.0]),
            (Pattern::Line, vec![45.0], vec![45.0]),
            (Pattern::Grid, vec![45.0, 135.0], vec![45.0, 135.0]),
            (
                Pattern::Triangle,
                vec![45.0, 105.0, 165.0],
                vec![135.0, 195.0, 255.0],
            ),
        ];
        for (pattern, even, odd) in cases {
            let mut ip = InfillParams::new(0.4);
            ip.pattern = pattern;
            ip.density = 0.2;
            assert_eq!(ip.sparse_angles(0), even, "{pattern:?} on layer 0");
            assert_eq!(ip.sparse_angles(1), odd, "{pattern:?} on layer 1");
            assert_eq!(
                ip.sparse_angles(2),
                ip.sparse_angles(0),
                "{pattern:?} repeats every two layers"
            );
        }
    }

    #[test]
    fn cubic_walks_four_directions_before_it_repeats() {
        let mut ip = InfillParams::new(0.4);
        ip.pattern = Pattern::Cubic;
        let seen: Vec<f64> = (0..4).flat_map(|k| ip.sparse_angles(k)).collect();
        assert_eq!(seen, vec![45.0, 90.0, 135.0, 180.0]);
        assert_eq!(ip.sparse_angles(4), ip.sparse_angles(0));
    }

    fn new_pattern_layer(pattern: Pattern, k: usize) -> Vec<Line> {
        let regions = stack(k + 6);
        let cache = MaskCache::new(&regions, 0.1, 12);
        let p = WallParams::new(0.4, 2);
        let mut ip = InfillParams::new(0.4);
        ip.pattern = pattern;
        ip.density = 0.15;
        ip.anchor = 0.0;
        infill_layer(&regions[k], &cache, k, 0.1 * k as f64, &p, &ip, None)
            .into_iter()
            .filter(|line| line.kind == Kind::Infill)
            .collect()
    }

    #[test]
    fn honeycomb_draws_three_directions_of_hexagon_side() {
        let lines = new_pattern_layer(Pattern::Honeycomb, 10);
        assert!(!lines.is_empty(), "the lattice drew nothing");
        let mut angles: Vec<i64> = lines
            .iter()
            .map(|line| {
                let d = line.pts[line.pts.len() - 1] - line.pts[0];
                ((d.y.atan2(d.x).to_degrees().rem_euclid(180.0)).round() as i64) % 180
            })
            .collect();
        angles.sort_unstable();
        angles.dedup();
        assert_eq!(
            angles,
            vec![0, 60, 120],
            "a hexagon has three side directions"
        );
    }

    #[test]
    fn zigzag_joins_scanlines_instead_of_lifting_between_them() {
        let straight = new_pattern_layer(Pattern::Rectilinear, 10);
        let zigzag = new_pattern_layer(Pattern::ZigZag, 10);
        assert!(!straight.is_empty() && !zigzag.is_empty());
        assert!(
            zigzag.len() < straight.len(),
            "{} zig-zag paths against {} straight ones",
            zigzag.len(),
            straight.len()
        );
        assert!(
            zigzag.iter().any(|line| line.pts.len() > 2),
            "no path crossed to a second line"
        );
    }

    #[test]
    fn concentric_fills_the_interior_with_closed_loops() {
        let loops = new_pattern_layer(Pattern::Concentric, 10);
        assert!(!loops.is_empty(), "no concentric loop was traced");
        assert!(
            loops.iter().filter(|line| line.closed).count() > 0,
            "every loop came back open"
        );
    }

    #[test]
    fn fill_angle_turns_every_pattern_and_alternates_by_ninety() {
        for pattern in [
            Pattern::Rectilinear,
            Pattern::Line,
            Pattern::Grid,
            Pattern::Triangle,
            Pattern::Cubic,
        ] {
            let mut ip = InfillParams::new(0.4);
            ip.pattern = pattern;
            let straight = (0..4)
                .flat_map(|k| ip.sparse_angles(k))
                .collect::<Vec<f64>>();
            ip.fill_angle = 20.0;
            let turned = (0..4)
                .flat_map(|k| ip.sparse_angles(k))
                .collect::<Vec<f64>>();
            let shift = 20.0 - 45.0;
            for (a, b) in straight.iter().zip(turned.iter()) {
                assert!((b - a - shift).abs() < 1e-9, "{pattern:?}: {a} -> {b}");
            }
        }
    }

    #[test]
    fn a_pattern_with_more_directions_lays_the_same_plastic() {
        let mut reference = InfillParams::new(0.4);
        reference.density = 0.2;
        reference.pattern = Pattern::Rectilinear;
        let base = reference.sparse_spacing();
        for (pattern, families) in [
            (Pattern::Line, 1.0),
            (Pattern::Grid, 2.0),
            (Pattern::Triangle, 3.0),
            (Pattern::Cubic, 1.0),
        ] {
            let mut ip = reference;
            ip.pattern = pattern;
            let spacing = ip.sparse_spacing();
            let every_layer: f64 = (0..2)
                .map(|k| ip.sparse_angles(k).len() as f64)
                .sum::<f64>()
                / 2.0;
            let linear = spacing / every_layer;
            assert!(
                (linear - base).abs() < 1e-9,
                "{pattern:?} lays {linear} mm of spacing against {base} for rectilinear"
            );
            assert!(every_layer <= families + 1.0);
        }
    }

    #[test]
    fn infill_stays_inside_the_region() {
        let regions = stack(1);
        let cache = MaskCache::new(&regions, 0.1, 1);
        let p = WallParams::new(0.4, 3);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.5;
        let lines = infill_layer(&regions[0], &cache, 0, 0.0, &p, &ip, None);
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
        let cache = MaskCache::new(&regions, 0.1, 12);
        let p = WallParams::new(0.4, 2);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.5;
        let k = 5;
        let lines = infill_layer(&regions[k], &cache, k, 0.0, &p, &ip, None);

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
    fn the_anchor_caps_how_far_infill_reaches_into_the_shell() {
        let regions = spanned_plate(5, 6);
        let cache = MaskCache::new(&regions, 0.1, 12);
        let p = WallParams::new(0.4, 2);
        let k = 5;
        let span = |anchor: f64| -> f64 {
            let ip = InfillParams {
                density: 0.5,
                anchor,
                ..InfillParams::new(0.4)
            };
            let lines = infill_layer(&regions[k], &cache, k, 0.0, &p, &ip, None);
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for line in lines.iter().filter(|l| l.kind == Kind::Infill) {
                for pt in &line.pts {
                    lo = lo.min(pt.x);
                    hi = hi.max(pt.x);
                }
            }
            if lo.is_finite() && hi.is_finite() {
                hi - lo
            } else {
                0.0
            }
        };
        let spans = [span(0.0), span(0.03), span(0.1), span(2.0)];
        assert!(
            spans.iter().all(|s| *s > 0.0),
            "the fixture produced no sparse infill to measure"
        );
        for pair in spans.windows(2) {
            assert!(
                pair[1] > pair[0],
                "a longer anchor did not reach further: {spans:?}"
            );
        }
    }

    #[test]
    fn a_zero_anchor_leaves_infill_ending_at_the_shell() {
        let ip = InfillParams {
            anchor: 0.0,
            anchor_max: 50.0,
            ..InfillParams::new(0.4)
        };
        assert_eq!(ip.anchor_length(), 0.0);
        let capped = InfillParams {
            anchor: 8.0,
            anchor_max: 3.0,
            ..InfillParams::new(0.4)
        };
        assert_eq!(capped.anchor_length(), 3.0);
    }

    #[test]
    fn cross_hatch_puts_crossing_strands_before_the_fill() {
        let regions = spanned_plate(5, 6);
        let cache = MaskCache::new(&regions, 0.1, 12);
        let p = WallParams::new(0.4, 2);
        let mut ip = InfillParams::new(0.4);
        ip.bridge.cross_hatch = true;
        let k = 5;
        let lines = infill_layer(&regions[k], &cache, k, 0.0, &p, &ip, None);
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
    fn a_sloping_stack_grows_its_shell_under_each_exposed_band() {
        let regions: Vec<Region> = (0..40)
            .map(|k| {
                let r = 18.0 - 0.4 * k as f64;
                let ring: Vec<V2> = (0..128)
                    .map(|i| {
                        let a = std::f64::consts::TAU * i as f64 / 128.0;
                        V2::new(20.0 + r * a.cos(), 20.0 + r * a.sin())
                    })
                    .collect();
                Region::new(vec![ring])
            })
            .collect();
        let cache = MaskCache::new(&regions, 0.1, 12);
        let ip = InfillParams::new(0.4);
        let mut solid_layers = 0;
        for k in 0..20 {
            let solid = cache.solid_bits(k, ip.top_layers, ip.bottom_layers);
            if cache.grid().is_set(&solid, V2::new(20.0 + 16.0, 20.0)) {
                solid_layers += 1;
            }
        }
        assert!(
            solid_layers >= 5,
            "a disc 16 mm out of a section that shrinks 0.4 mm a layer sits inside the \
             shell under the exposed slope, so it is solid: {solid_layers} of 20 layers say so"
        );
    }

    #[test]
    fn a_thin_part_is_solid_throughout() {
        let regions = stack(6);
        let cache = MaskCache::new(&regions, 0.1, 12);
        let ip = InfillParams::new(0.4);
        let centre = V2::new(10.0, 10.0);
        for k in 0..6 {
            let (win_top, win_bottom) = ip.window();
            let solid = cache.solid_bits(k, win_top, win_bottom);
            assert!(
                cache.grid().is_set(&solid, centre),
                "layer {k} of a 6-layer part should be solid at the centre"
            );
        }
    }

    #[test]
    fn the_interior_of_a_tall_part_stays_sparse() {
        let regions = stack(30);
        let cache = MaskCache::new(&regions, 0.1, 12);
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
    fn solid_lines_on_the_top_skin_are_marked_as_top_surface() {
        let regions = stack(8);
        let cache = MaskCache::new(&regions, 0.1, 8);
        let p = WallParams::new(0.4, 3);
        let mut ip = InfillParams::new(0.4);
        ip.top_layers = 2;
        ip.bottom_layers = 1;
        ip.density = 0.4;
        let (mut top, mut plain) = (0usize, 0usize);
        for (k, region) in regions.iter().enumerate() {
            for line in infill_layer(region, &cache, k, 0.0, &p, &ip, None) {
                match line.kind {
                    Kind::TopSolid => top += 1,
                    Kind::Solid => plain += 1,
                    _ => {}
                }
            }
        }
        assert!(top > 0, "no top-surface lines were marked");
        assert!(plain > 0, "every solid line was marked as top surface");
    }

    #[test]
    fn mask_cache_reuses_slots_without_losing_answers() {
        let regions = stack(20);
        let small = MaskCache::new(&regions, 0.1, 3);
        let large = MaskCache::new(&regions, 0.1, 40);
        for k in 0..20 {
            assert_eq!(
                small.solid_bits(k, 2, 2),
                large.solid_bits(k, 2, 2),
                "layer {k} disagreed between cache sizes"
            );
        }
    }

    fn curve_layer(pattern: Pattern, k: usize, z: f64) -> Vec<Line> {
        let regions = stack(k + 6);
        let cache = MaskCache::new(&regions, 0.1, 3);
        let p = WallParams::new(0.4, 2);
        let mut ip = InfillParams::new(0.4);
        ip.pattern = pattern;
        ip.density = 0.15;
        infill_layer(&regions[0], &cache, k, z, &p, &ip, None)
            .into_iter()
            .filter(|l| l.kind == Kind::Infill)
            .collect()
    }

    fn crossings(lines: &[Line], y: f64) -> Vec<f64> {
        let mut out = Vec::new();
        for line in lines {
            for pair in line.pts.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                if (a.y - y) * (b.y - y) < 0.0 {
                    let t = (y - a.y) / (b.y - a.y);
                    out.push(a.x + (b.x - a.x) * t);
                }
            }
        }
        out.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out
    }

    fn pitch(xs: &[f64]) -> Vec<f64> {
        xs.windows(2)
            .map(|w| w[1] - w[0])
            .filter(|d| *d > 0.3)
            .collect()
    }

    #[test]
    fn the_gyroid_cell_follows_the_width_and_the_density() {
        let mut ip = InfillParams::new(0.4);
        ip.pattern = Pattern::Gyroid;
        ip.density = 0.15;
        let expected = std::f64::consts::TAU * 0.4 / (0.15 * 2.44);
        assert!((ip.pattern_cell().unwrap() - expected).abs() < 1e-9);
        ip.width = 0.6;
        assert!(
            ip.pattern_cell().unwrap() > expected,
            "a wider bead opens the cell"
        );
        ip.width = 0.4;
        ip.density = 0.3;
        assert!(
            ip.pattern_cell().unwrap() < expected,
            "twice the density halves the cell"
        );
    }

    #[test]
    fn the_honeycomb_module_follows_the_density() {
        let mut ip = InfillParams::new(0.4);
        ip.pattern = Pattern::Honeycomb3D;
        ip.density = 0.2;
        assert!((ip.pattern_cell().unwrap() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn curved_patterns_lay_plastic_at_the_requested_density() {
        for pattern in [Pattern::Gyroid, Pattern::Honeycomb3D] {
            let lines = curve_layer(pattern, 30, 6.2);
            assert!(!lines.is_empty(), "{pattern:?} printed nothing");
            let length: f64 = lines
                .iter()
                .flat_map(|l| l.pts.windows(2))
                .map(|w| w[0].dist(w[1]))
                .sum();
            let coverage = length * 0.4 / 326.0;
            assert!(
                coverage > 0.1 && coverage < 0.35,
                "{pattern:?} covers {coverage:.3} of the free area at 15% density"
            );
        }
    }

    #[test]
    fn curved_paths_stay_out_of_the_shell() {
        for pattern in [Pattern::Gyroid, Pattern::Honeycomb3D] {
            let lines = curve_layer(pattern, 30, 6.2);
            let (mut total, mut in_shell) = (0usize, 0usize);
            for line in &lines {
                for pt in &line.pts {
                    assert!(
                        pt.x > 0.0 && pt.x < 20.0 && pt.y > 0.0 && pt.y < 20.0,
                        "{pattern:?} put a point at {pt:?}, outside the part"
                    );
                    total += 1;
                    if pt.x < 0.96 || pt.y < 0.96 || pt.x > 19.04 || pt.y > 19.04 {
                        in_shell += 1;
                    }
                }
            }
            assert!(total > 100, "{pattern:?} printed too little to judge");
            assert!(
                in_shell * 20 < total,
                "{pattern:?} left {in_shell} of {total} points inside the shell, more than the anchored ends"
            );
        }
    }

    #[test]
    fn curved_paths_are_curves_not_straight_lines() {
        for pattern in [Pattern::Gyroid, Pattern::Honeycomb3D] {
            let lines = curve_layer(pattern, 30, 6.2);
            let bends = lines
                .iter()
                .filter(|l| l.pts.len() > 2)
                .filter(|l| {
                    let straight = l.pts[0].dist(*l.pts.last().unwrap());
                    let walked: f64 = l.pts.windows(2).map(|w| w[0].dist(w[1])).sum();
                    walked > straight * 1.02
                })
                .count();
            assert!(bends > 4, "{pattern:?} only bent {bends} paths");
        }
    }

    #[test]
    fn consecutive_gyroid_layers_do_not_line_up() {
        let a = curve_layer(Pattern::Gyroid, 30, 6.0);
        let b = curve_layer(Pattern::Gyroid, 31, 6.2);
        let pa = pitch(&crossings(&a, 10.0));
        let pb = pitch(&crossings(&b, 10.0));
        assert!(
            !pa.is_empty() && !pb.is_empty(),
            "a layer has no strands to compare"
        );
        let moved = pa.iter().any(|d| pb.iter().all(|e| (e - d).abs() > 0.05));
        assert!(
            moved || (pa.len() != pb.len()),
            "the gyroid landed in the same place on both layers, so it is not following z"
        );
    }

    #[test]
    fn the_honeycomb_alternates_rows_and_columns() {
        let even = curve_layer(Pattern::Honeycomb3D, 30, 6.0);
        let odd = curve_layer(Pattern::Honeycomb3D, 31, 6.2);
        let spread = |lines: &[Line], axis: bool| -> f64 {
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for line in lines {
                for pt in &line.pts {
                    let v = if axis { pt.x } else { pt.y };
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            hi - lo
        };
        assert!(
            (spread(&even, true) - spread(&odd, true)).abs() > 1e-6
                || (spread(&even, false) - spread(&odd, false)).abs() > 1e-6,
            "both honeycomb layers are the same slice"
        );
    }

    fn skin_lines(ip: &InfillParams, regions: &[Region], k: usize) -> Vec<Line> {
        let cache = MaskCache::new(regions, 0.1, 12);
        let p = WallParams::new(0.4, 2);
        infill_layer(&regions[k], &cache, k, 0.0, &p, ip, None)
            .into_iter()
            .filter(|l| matches!(l.kind, Kind::Solid | Kind::TopSolid))
            .collect()
    }

    #[test]
    fn a_monotonic_skin_runs_one_way_on_every_layer() {
        let regions = stack(10);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 2;
        ip.bottom_layers = 2;
        ip.top_pattern = SkinPattern::Monotonic;
        ip.bottom_pattern = SkinPattern::Monotonic;
        let direction = |lines: &[Line]| -> f64 {
            let line = lines.first().expect("a skin line exists");
            let d = line.pts[1] - line.pts[0];
            let angle = d.y.atan2(d.x).to_degrees();
            (angle + 180.0).rem_euclid(180.0)
        };
        let first = direction(&skin_lines(&ip, &regions, 8));
        let second = direction(&skin_lines(&ip, &regions, 9));
        assert!(
            (first - second).abs() < 1.0,
            "monotonic turned from {first} to {second} on the next layer"
        );
    }

    #[test]
    fn a_rectilinear_skin_turns_ninety_degrees_between_layers() {
        let regions = stack(10);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 2;
        ip.bottom_layers = 2;
        ip.top_pattern = SkinPattern::Rectilinear;
        ip.bottom_pattern = SkinPattern::Rectilinear;
        let direction = |lines: &[Line]| -> f64 {
            let line = lines.first().expect("a skin line exists");
            let d = line.pts[1] - line.pts[0];
            (d.y.atan2(d.x).to_degrees() + 180.0).rem_euclid(180.0)
        };
        let first = direction(&skin_lines(&ip, &regions, 8));
        let second = direction(&skin_lines(&ip, &regions, 9));
        assert!(
            (first - second).abs() > 45.0,
            "rectilinear kept {first} and {second}"
        );
    }

    #[test]
    fn a_concentric_skin_draws_closed_loops_and_no_straight_lines() {
        let regions = stack(10);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 2;
        ip.bottom_layers = 2;
        ip.top_pattern = SkinPattern::Concentric;
        ip.bottom_pattern = SkinPattern::Concentric;
        let lines = skin_lines(&ip, &regions, 9);
        assert!(!lines.is_empty(), "concentric printed nothing");
        assert!(
            lines.iter().all(|l| l.closed && l.pts.len() > 4),
            "a concentric skin is loops, so no line may be a two-point run"
        );
        let mut boxes: Vec<(f64, f64)> = lines
            .iter()
            .map(|l| {
                let mut b = hfw_kernel::Bounds::empty();
                for p in &l.pts {
                    b.add(*p);
                }
                (b.max.x - b.min.x, b.max.y - b.min.y)
            })
            .collect();
        boxes.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        assert!(
            (boxes[0].0 - 20.0).abs() < 1.2,
            "the first loop hugs the edge, got {:.3} mm",
            boxes[0].0
        );
        assert!(
            (boxes[0].0 - boxes[1].0 - 0.8).abs() < 0.25,
            "loops are {:.3} and {:.3} mm wide, which is not one bead apart",
            boxes[0].0,
            boxes[1].0
        );
    }

    #[test]
    fn a_top_width_that_differs_from_the_solid_width_changes_the_bead() {
        let regions = stack(10);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 3;
        ip.bottom_layers = 1;
        let thin = skin_lines(&ip, &regions, 9);
        ip.top_width = 0.3;
        let fine = skin_lines(&ip, &regions, 9);
        let count = |lines: &[Line], w: f64| {
            lines
                .iter()
                .filter(|l| (l.width[0] - w).abs() < 1e-9)
                .count()
        };
        assert!(count(&thin, 0.4) > 0);
        assert!(
            count(&fine, 0.3) > count(&fine, 0.4),
            "the top surface did not move to its own width"
        );
    }

    #[test]
    fn a_solid_layer_every_n_thickens_the_interior_of_a_tall_part() {
        let regions = stack(12);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        let plain = skin_lines(&ip, &regions, 6).len();
        ip.solid_every = 3;
        let forced = skin_lines(&ip, &regions, 6).len();
        assert_eq!(
            plain, 0,
            "a mid part layer is not solid without the setting"
        );
        assert!(forced > 0, "the forced layer is solid");
        let neighbour = skin_lines(&ip, &regions, 7).len();
        assert_eq!(neighbour, 0, "the layers between stay sparse");
    }

    #[test]
    fn a_layer_below_the_area_threshold_is_filled_solid() {
        let tiny = vec![square(0.0, 0.0, 6.0, 6.0); 6];
        let big = vec![square(0.0, 0.0, 20.0, 20.0); 6];
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        ip.solid_below_area = 70.0;
        assert!(
            !skin_lines(&ip, &tiny, 3).is_empty(),
            "36 mm2 is under the 70 mm2 threshold"
        );
        assert!(skin_lines(&ip, &big, 3).is_empty(), "400 mm2 is over it");
        ip.solid_below_area = 500.0;
        assert!(
            !skin_lines(&ip, &big, 3).is_empty(),
            "a higher threshold takes the big layer too"
        );
    }

    #[test]
    fn the_area_threshold_applies_to_each_island_not_to_the_layer() {
        let two = |small: f64| -> Region {
            Region::new(vec![
                vec![
                    V2::new(0.0, 0.0),
                    V2::new(20.0, 0.0),
                    V2::new(20.0, 20.0),
                    V2::new(0.0, 20.0),
                ],
                vec![
                    V2::new(30.0, 0.0),
                    V2::new(30.0 + small, 0.0),
                    V2::new(30.0 + small, small),
                    V2::new(30.0, small),
                ],
            ])
        };
        let regions = vec![two(6.0); 6];
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        ip.solid_below_area = 70.0;
        let solid = skin_lines(&ip, &regions, 3);
        assert!(!solid.is_empty(), "the 36 mm2 island prints solid");
        assert!(
            solid.iter().all(|line| line.pts.iter().all(|p| p.x > 20.0)),
            "the 400 mm2 island stays sparse"
        );
        let everything = infill_layer(
            &regions[3],
            &MaskCache::new(&regions, 0.1, 12),
            3,
            0.0,
            &WallParams::new(0.4, 2),
            &ip,
            None,
        );
        assert!(
            everything
                .iter()
                .any(|line| line.pts.iter().any(|p| p.x < 20.0)),
            "the big island still gets sparse fill"
        );
    }

    #[test]
    fn the_area_threshold_is_measured_inside_the_wall_band() {
        let body = vec![
            V2::new(0.0, 0.0),
            V2::new(30.0, 0.0),
            V2::new(30.0, 30.0),
            V2::new(0.0, 30.0),
        ];
        let arm = vec![
            V2::new(50.0, 13.4),
            V2::new(100.0, 13.4),
            V2::new(100.0, 16.6),
            V2::new(50.0, 16.6),
        ];
        let regions = vec![Region::new(vec![body, arm]); 6];
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.15;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        ip.solid_below_area = 70.0;
        ip.anchor = 0.0;
        ip.anchor_max = 0.0;
        let solid = skin_lines(&ip, &regions, 3);
        let in_arm = |line: &Line| line.pts.iter().all(|p| p.x > 51.0);
        assert!(
            solid.iter().any(in_arm),
            "the arm's 50 by 3.2 mm outline is 160 mm^2, but the strip left between two wall \
             bands is 1.3 by 50 mm and has to print solid"
        );
        assert!(
            !solid.iter().all(in_arm),
            "the 900 mm^2 body is over the threshold and stays sparse"
        );
        let sparse = sparse_layer(&ip, &regions, 3);
        assert!(
            sparse
                .iter()
                .any(|line| line.pts.iter().any(|p| p.x < 29.0)),
            "the body still gets sparse fill"
        );
    }

    fn sparse_layer(ip: &InfillParams, regions: &[Region], k: usize) -> Vec<Line> {
        infill_layer(
            &regions[k],
            &MaskCache::new(regions, 0.1, 12),
            k,
            0.0,
            &WallParams::new(0.4, 3),
            ip,
            None,
        )
        .into_iter()
        .filter(|line| line.kind == Kind::Infill)
        .collect()
    }

    #[test]
    fn the_anchor_joins_two_infill_lines_with_one_perimeter_segment() {
        let regions = stack(12);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.15;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        ip.anchor = 0.0;
        ip.anchor_max = 0.0;
        let loose = sparse_layer(&ip, &regions, 6);
        assert!(loose.len() > 2, "the sparse fill is several separate lines");
        assert!(
            loose.iter().all(|line| line.pts.len() == 2),
            "with no anchor every line stands alone"
        );

        ip.anchor = 2.4;
        ip.anchor_max = 50.0;
        let joined = sparse_layer(&ip, &regions, 6);
        assert_eq!(joined.len(), 1, "one angle gives one continuous run");
        assert!(joined[0].pts.len() > 4, "the run walks the boundary");
        let length = joined[0].length();
        let loose_length: f64 = loose.iter().map(|line| line.length()).sum();
        assert!(
            length > loose_length,
            "the join adds the boundary segments it walks: {length} against {loose_length}"
        );
        let inner = ip.inner_offset(&WallParams::new(0.4, 3));
        let band = inner - 2.0 * ip.width - 1e-6;
        let (low, high) = (band, 20.0 - band);
        assert!(
            joined[0]
                .pts
                .iter()
                .all(|p| p.x >= low && p.x <= high && p.y >= low && p.y <= high),
            "the run has to stay inside the wall band, not on the part outline"
        );
    }

    #[test]
    fn dbg_join() {
        let regions = stack(12);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.15;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        ip.anchor = 2.4;
        ip.anchor_max = 50.0;
        let joined = sparse_layer(&ip, &regions, 6);
        for line in &joined {
            println!(
                "pts={} len={:.2} closed={}",
                line.pts.len(),
                line.length(),
                line.closed
            );
            for p in &line.pts {
                println!("   {:.3} {:.3}", p.x, p.y);
            }
        }
    }

    fn combine_cells(cache: &MaskCache, k: usize, top: usize, bottom: usize) -> Vec<u64> {
        cache.sparse_own_bits(k, top, bottom)
    }

    #[test]
    fn a_combined_layer_lays_the_plastic_of_the_layers_it_replaces() {
        let regions = stack(12);
        let p = WallParams::new(0.4, 2);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        let plain = {
            let cache = MaskCache::new(&regions, 0.1, 12);
            (0..6)
                .map(|k| {
                    infill_layer(&regions[k], &cache, k, 0.0, &p, &ip, None)
                        .iter()
                        .map(|l| l.length() * l.width[0] * l.flow)
                        .sum::<f64>()
                })
                .collect::<Vec<f64>>()
        };
        let cache = MaskCache::new(&regions, 0.1, 12);
        let shared = combine_cells(&cache, 4, 1, 1);
        assert!(
            shared.iter().any(|w| *w != 0),
            "a solid cube has free cells to combine"
        );
        let combine = Combine {
            strip: &shared,
            extra: &shared,
            layers: 3,
        };
        let lines = infill_layer(&regions[4], &cache, 4, 0.0, &p, &ip, Some(&combine));
        let volume: f64 = lines.iter().map(|l| l.length() * l.width[0] * l.flow).sum();
        let plain_volume: f64 = plain[4] * 3.0;
        assert!(
            volume > plain_volume * 0.9 && volume < plain_volume * 1.1,
            "three layers of fill weigh {plain_volume:.2}, the combined one {volume:.2}"
        );
        let scaled = lines.iter().filter(|l| (l.flow - 3.0).abs() < 1e-9).count();
        assert!(scaled > 0, "no line carried the combined flow");
    }

    #[test]
    fn a_stripped_cell_is_left_empty_on_the_lower_layer() {
        let regions = stack(8);
        let p = WallParams::new(0.4, 2);
        let mut ip = InfillParams::new(0.4);
        ip.density = 0.2;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        let cache = MaskCache::new(&regions, 0.1, 8);
        let shared = combine_cells(&cache, 4, 1, 1);
        let strip = Combine {
            strip: &shared,
            extra: &[],
            layers: 3,
        };
        let lines = infill_layer(&regions[4], &cache, 4, 0.0, &p, &ip, Some(&strip));
        let inside = lines.iter().filter(|l| l.kind == Kind::Infill).count();
        assert_eq!(inside, 0, "the strip left sparse fill behind");
    }

    #[test]
    fn every_curve_skin_draws_and_stays_inside_its_mask() {
        for pattern in [
            SkinPattern::HilbertCurve,
            SkinPattern::ArchimedeanChords,
            SkinPattern::OctagramSpiral,
        ] {
            let regions = stack(10);
            let mut ip = InfillParams::new(0.4);
            ip.density = 0.2;
            ip.top_layers = 2;
            ip.bottom_layers = 1;
            ip.top_pattern = pattern;
            ip.bottom_pattern = pattern;
            let lines = skin_lines(&ip, &regions, 9);
            assert!(!lines.is_empty(), "{pattern:?} printed nothing");
            let length: f64 = lines.iter().map(|l| l.length()).sum();
            assert!(
                length > 40.0,
                "{pattern:?} covered only {length:.1} mm of a 20 mm face"
            );
        }
    }

    #[test]
    fn an_overhang_gets_a_loop_that_a_flat_layer_does_not() {
        let mut regions = stack(6);
        regions[4] = square(0.0, 0.0, 26.0, 20.0);
        let cache = MaskCache::new(&regions, 0.1, 12);
        let p = WallParams::new(0.4, 2);
        let ip = InfillParams::new(0.4);
        let overhang = crate::overhang_perimeter(&regions[4], &cache, 4, &p, &ip);
        assert!(!overhang.is_empty(), "the 6 mm lip produced no loop");
        assert!(
            overhang.iter().all(|l| l.kind == Kind::Side(0)),
            "the loop is not reported as the outer wall"
        );
        let flat = MaskCache::new(&regions, 0.1, 12);
        let none = crate::overhang_perimeter(&regions[3], &flat, 3, &p, &ip);
        assert!(
            none.is_empty(),
            "a layer over its own footprint needs no loop"
        );
    }
}
