//! Wall generation parameters and bead decomposition.

/// Which wall-generation strategy to run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Hybrid field walls: variable-width beading from the medial axis.
    #[default]
    Hfw,
    /// Classic fixed-width concentric loops only.
    Classic,
}

/// Nominal extrusion width in millimetres.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Nominal extrusion width.
    pub w: f64,
    /// Width of the outermost loop; zero means `w`.
    ///
    /// PrusaSlicer and OrcaSlicer let the visible surface be printed with its
    /// own bead width (`external_perimeter_extrusion_width`) while the loops
    /// behind it keep the nominal one. The ladder is cumulative, so a wider
    /// outer bead pushes every loop behind it inwards by exactly the difference
    /// instead of leaving a gap or an overlap.
    pub w_outer: f64,
    /// Number of requested perimeter loops.
    pub walls: usize,
    /// Narrowest bead the generator will emit.
    pub w_min: f64,
    /// Widest bead the generator will emit.
    pub w_max: f64,
    /// Distance field and iso-band resolution.
    pub h: f64,
    /// Opening angle above which a medial-axis vertex counts as a ridge root.
    pub central_deg: f64,
    /// Features narrower than this are treated as thin walls.
    pub min_feature: f64,
    /// True collapses sub-width features into a single trace; false drops them.
    pub thin_walls: bool,
    /// Medial-axis radius smoothing half-window, in millimetres.
    pub smooth: f64,
    /// Medial-axis branches shorter than this are pruned.
    pub spur: f64,
    /// Medial-axis extension factor used when matching thin walls.
    pub ext: f64,
    /// Corner opening angle below which a sharp vertex is reinserted.
    pub corner_deg: f64,
    /// Douglas-Peucker tolerance in millimetres.
    pub simplify: f64,
    /// Arc length over which a bead-count change is blended.
    pub trans_len: f64,
    /// Bead-count hysteresis.
    pub hyst: f64,
    /// Middle bead wider than this is emitted as a centre line.
    pub taper_min: f64,
    /// Emit the thin bead that closes a gap between the shell and the infill.
    pub gap_fill: bool,
    /// Number of loops over which the outer-width difference is spread.
    pub distribution: usize,
}

impl Params {
    /// Creates parameters for width `w` and `walls` loops.
    pub fn new(w: f64, walls: usize) -> Params {
        Params {
            w,
            w_outer: 0.0,
            walls,
            w_min: 0.6 * w,
            w_max: 1.5 * w,
            h: w / 8.0,
            central_deg: 135.0,
            min_feature: 0.25 * w,
            thin_walls: true,
            smooth: w,
            spur: 2.0 * w,
            ext: 1.5,
            corner_deg: 12.0,
            simplify: 0.002,
            trans_len: 2.0 * w,
            hyst: 0.1,
            taper_min: 0.25 * w,
            gap_fill: true,
            distribution: 1,
        }
    }

    /// The same parameters with every width-dependent number scaled to `w`.
    ///
    /// A layer printed at its own bead width needs its thresholds moved with it:
    /// the narrowest bead, the widest one, the field resolution and the
    /// transition length are all fractions of the nominal width, so scaling them
    /// by the same ratio keeps the generator behaving the way the profile tuned
    /// it instead of falling back to the defaults for the new width.
    pub fn scaled_to(&self, w: f64) -> Params {
        if !(w.is_finite() && w > 0.0) || w == self.w {
            return *self;
        }
        let ratio = w / self.w;
        Params {
            w,
            w_outer: if self.w_outer > 0.0 {
                self.w_outer * ratio
            } else {
                0.0
            },
            w_min: self.w_min * ratio,
            w_max: self.w_max * ratio,
            h: self.h * ratio,
            min_feature: self.min_feature * ratio,
            smooth: self.smooth * ratio,
            spur: self.spur * ratio,
            trans_len: self.trans_len * ratio,
            taper_min: self.taper_min * ratio,
            ..*self
        }
    }

    /// The same parameters asking for `walls` loops.
    pub fn with_walls(&self, walls: usize) -> Params {
        Params { walls, ..*self }
    }

    /// Radius beyond which no wall can exist.
    pub fn cap(&self) -> f64 {
        self.thick_radius() + 2.0 * self.h
    }

    /// Outer radius of the requested wall band.
    pub fn thick_radius(&self) -> f64 {
        if self.walls == 0 {
            return 0.0;
        }
        if self.distribution.max(1) < 2 {
            return self.outer_width() + (self.walls - 1) as f64 * self.w;
        }
        let last = self.walls - 1;
        self.loop_offset(last) + 0.5 * self.loop_width(last)
    }

    /// Width of the outermost loop, falling back to the nominal width.
    pub fn outer_width(&self) -> f64 {
        if self.w_outer > 0.0 {
            self.w_outer
        } else {
            self.w
        }
    }

    /// Share of the outer-width difference that loop `i` carries.
    ///
    /// Upstream spreads the difference between the requested thickness and the
    /// optimal one over the loops with a parabolic weight whose radius is
    /// `wall_distribution_count`, centred on the middle loop of the band. This
    /// engine has no leftover thickness — the medial axis decides each bead from
    /// the geometry — so the difference it has to place is the one between the
    /// outer loop and the inner ones, and it is placed by the same weights.
    fn distribution_weight(&self, i: usize) -> f64 {
        let radius = self.distribution.max(1);
        if radius < 2 || self.walls < 2 || i >= self.walls {
            return if i == 0 { 1.0 } else { 0.0 };
        }
        let radius = radius.min(self.walls);
        let middle = (self.walls - 1) as f64 / 2.0;
        let scale = 1.0 / (radius - 1) as f64;
        let raw = |idx: usize| (1.0 - (scale * (idx as f64 - middle)).powi(2)).max(0.0);
        let total: f64 = (0..self.walls).map(raw).sum();
        if total <= 0.0 {
            return if i == 0 { 1.0 } else { 0.0 };
        }
        raw(i) / total
    }

    /// Width of loop `i`, counting outwards from zero.
    pub fn loop_width(&self, i: usize) -> f64 {
        if self.distribution.max(1) < 2 {
            return if i == 0 { self.outer_width() } else { self.w };
        }
        let delta = self.outer_width() - self.w;
        if delta == 0.0 {
            return self.w;
        }
        self.w + delta * self.distribution_weight(i)
    }

    /// Distance from the model surface to the centre of loop `i`.
    pub fn loop_offset(&self, i: usize) -> f64 {
        if self.distribution.max(1) < 2 {
            let outer = self.outer_width();
            return if i == 0 {
                0.5 * outer
            } else {
                outer + (i as f64 - 0.5) * self.w
            };
        }
        let mut offset = 0.5 * self.loop_width(0);
        let mut previous = self.loop_width(0);
        for k in 1..=i {
            let width = self.loop_width(k);
            offset += 0.5 * (previous + width);
            previous = width;
        }
        offset
    }

    /// Thinnest feature, expressed in beads.
    pub fn nu_min(&self) -> f64 {
        self.min_feature / self.w
    }

    /// Quantises a feature width in beads to a bead count.
    pub fn state_of(&self, nu: f64) -> usize {
        if nu < self.nu_min() {
            return 0;
        }
        (nu.round() as i64).clamp(1, (2 * self.walls) as i64) as usize
    }

    /// Composes a blended bead layout between two bead counts.
    pub fn compose(&self, lo: usize, hi: usize, lam: f64, r: f64) -> Option<Beading> {
        if lo == 0 || hi == 0 {
            return None;
        }
        if lo == hi || lam <= 0.0 {
            return Some(config(lo, r));
        }
        if lam >= 1.0 {
            return Some(config(hi, r));
        }
        Some(Beading::blend(&config(lo, r), &config(hi, r), lam, r))
    }
}

impl Default for Params {
    fn default() -> Params {
        Params::new(0.4, 3)
    }
}

/// Maximum number of bead edges tracked per vertex.
pub const MAXC: usize = 12;

/// A symmetric decomposition of a feature of half-width `r` into beads.
///
/// `e` lists the bead boundaries from the centreline outwards. An odd bead
/// count keeps a middle bead straddling the centreline, so the layout is
/// stored as `k` edges with `half` set.
#[derive(Clone, Copy, Debug)]
pub struct Beading {
    /// Number of stored bead boundaries.
    pub k: usize,
    /// True when the innermost bead straddles the centreline.
    pub half: bool,
    /// Bead boundary radii from the centreline outwards.
    pub e: [f64; MAXC + 1],
}

impl Beading {
    /// Number of beads extending past the centreline on one side.
    pub fn full_cells(&self) -> usize {
        self.k - self.half as usize
    }

    /// Centre radius of bead `i`.
    pub fn center(&self, i: usize) -> f64 {
        0.5 * (self.e[i] + self.e[i + 1])
    }

    /// Width of bead `i`.
    pub fn width(&self, i: usize) -> f64 {
        self.e[i + 1] - self.e[i]
    }

    /// Width of the bead straddling the centreline.
    pub fn middle_width(&self, r: f64) -> f64 {
        2.0 * (r - self.e[self.k - 1])
    }

    fn blend(a: &Beading, b: &Beading, lam: f64, r: f64) -> Beading {
        let k = a.k.max(b.k);
        let mut e = [r; MAXC + 1];
        for (j, slot) in e.iter_mut().enumerate().take(k + 1) {
            let ea = if j <= a.k { a.e[j] } else { r };
            let eb = if j <= b.k { b.e[j] } else { r };
            *slot = (1.0 - lam) * ea + lam * eb;
        }
        let half = match (a.half, b.half) {
            (false, false) => false,
            (true, true) => true,
            (false, true) => lam > 0.0,
            (true, false) => lam < 0.5,
        };
        Beading { k, half, e }
    }
}

/// Even bead layout for `n` beads across a feature of half-width `r`.
pub fn config(n: usize, r: f64) -> Beading {
    let mut e = [r; MAXC + 1];
    let m = n / 2;
    if n % 2 == 0 {
        for (j, slot) in e.iter_mut().enumerate().take(m + 1) {
            *slot = j as f64 * (r / m as f64);
        }
        Beading {
            k: m,
            half: false,
            e,
        }
    } else {
        let b = 2.0 * r / n as f64;
        for (j, slot) in e.iter_mut().enumerate().take(m + 1) {
            *slot = j as f64 * b;
        }
        e[m + 1] = r;
        Beading {
            k: m + 1,
            half: true,
            e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_bead_counts_are_symmetric_about_the_centreline() {
        let b = config(4, 1.0);
        assert_eq!(b.k, 2);
        assert!(!b.half);
        assert_eq!(b.full_cells(), 2);
        assert!((b.center(0) - 0.25).abs() < 1e-12);
        assert!((b.width(1) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn odd_bead_counts_keep_a_middle_bead() {
        let b = config(3, 1.5);
        assert_eq!(b.k, 2);
        assert!(b.half);
        assert_eq!(b.full_cells(), 1);
        assert!((b.middle_width(1.5) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn bead_count_quantises_narrow_features_to_zero() {
        let p = Params::new(0.4, 3);
        assert_eq!(p.state_of(0.1), 0);
        assert_eq!(p.state_of(1.4), 1);
        assert_eq!(p.state_of(9.0), 6);
    }

    #[test]
    fn composition_selects_the_closer_layout() {
        let p = Params::new(0.4, 3);
        assert!(p.compose(0, 2, 0.5, 1.0).is_none());
        assert_eq!(p.compose(2, 2, 0.9, 1.0).unwrap().k, 1);
        assert_eq!(p.compose(2, 4, 0.0, 1.0).unwrap().k, 1);
        assert_eq!(p.compose(2, 4, 1.0, 1.0).unwrap().k, 2);
    }

    #[test]
    fn a_wider_outer_loop_pushes_the_loops_behind_it_inwards() {
        let mut p = Params::new(0.4, 3);
        assert!((p.loop_offset(0) - 0.2).abs() < 1e-12);
        assert!((p.loop_offset(2) - 1.0).abs() < 1e-12);
        assert!((p.thick_radius() - 1.2).abs() < 1e-12);

        p.w_outer = 0.6;
        assert!((p.loop_width(0) - 0.6).abs() < 1e-12);
        assert!((p.loop_width(1) - 0.4).abs() < 1e-12);
        assert!((p.loop_offset(0) - 0.3).abs() < 1e-12);
        assert!((p.loop_offset(1) - 0.8).abs() < 1e-12);
        assert!((p.loop_offset(2) - 1.2).abs() < 1e-12);
        assert!((p.thick_radius() - 1.4).abs() < 1e-12);
        assert!((p.cap() - (1.4 + 2.0 * p.h)).abs() < 1e-12);
    }

    #[test]
    fn a_narrower_outer_loop_pulls_the_loops_behind_it_outwards() {
        let mut p = Params::new(0.4, 2);
        p.w_outer = 0.3;
        assert!((p.loop_offset(0) - 0.15).abs() < 1e-12);
        assert!((p.loop_offset(1) - 0.5).abs() < 1e-12);
        assert!((p.thick_radius() - 0.7).abs() < 1e-12);
    }

    #[test]
    fn a_distribution_count_spreads_the_outer_width_difference() {
        let mut p = Params::new(0.4, 5);
        p.w_outer = 0.6;
        let serrated: Vec<f64> = (0..5).map(|i| p.loop_width(i)).collect();
        assert!((serrated[0] - 0.6).abs() < 1e-12, "one loop carries it all");
        assert!(serrated[1..].iter().all(|w| (w - 0.4).abs() < 1e-12));

        p.distribution = 3;
        let spread: Vec<f64> = (0..5).map(|i| p.loop_width(i)).collect();
        assert!(
            spread[1] > 0.4 && spread[1] < 0.6,
            "the second loop takes part of the difference, got {}",
            spread[1]
        );
        let total: f64 = spread.iter().sum();
        assert!(
            (total - (0.6 + 4.0 * 0.4)).abs() < 1e-12,
            "the band width is unchanged by spreading it"
        );
        assert!(
            (spread[0] - 0.6).abs() > 1e-6 || (spread[4] - 0.4).abs() > 1e-6,
            "the difference moved"
        );
    }

    #[test]
    fn a_scaled_band_moves_every_width_derived_number_with_it() {
        let p = Params::new(0.4, 3);
        let wide = p.scaled_to(0.6);
        assert!((wide.w - 0.6).abs() < 1e-12);
        assert!((wide.w_min - 0.36).abs() < 1e-12);
        assert!((wide.h - 0.075).abs() < 1e-12);
        assert!((wide.trans_len - 1.2).abs() < 1e-12);
        assert_eq!(wide.walls, p.walls, "the loop count is not a width");
        assert_eq!(wide.gap_fill, p.gap_fill);
        let same = p.scaled_to(0.0);
        assert!((same.w - 0.4).abs() < 1e-12, "zero leaves the band alone");
    }
}
