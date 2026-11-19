//! Wall generation parameters: the loop ladder and the floor a feature must clear.

/// Which wall-generation strategy to run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Loops plus the gap pass that fills what the ladder leaves over.
    #[default]
    Hfw,
    /// Concentric loops only, with no gap pass.
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
    /// Raster resolution used by the adhesion brim.
    ///
    /// Walls are offsets of the layer and no longer sample anything; the brim
    /// still fills its band row by row, and this is the row pitch it works at.
    pub h: f64,
    /// Narrowest feature that prints at all.
    ///
    /// Anything thinner is dropped before the ladder runs, which is what keeps a
    /// sliver a fraction of a bead wide from being smeared onto the layer.
    pub min_feature: f64,
    /// True prints a narrow feature even with the gap pass switched off.
    pub thin_walls: bool,
    /// Corner opening angle below which a sharp vertex is reinserted.
    pub corner_deg: f64,
    /// Douglas-Peucker tolerance in millimetres.
    pub simplify: f64,
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
            h: w / 6.0,
            min_feature: 0.25 * w,
            thin_walls: true,
            corner_deg: 12.0,
            simplify: 0.002,
            gap_fill: true,
            distribution: 1,
        }
    }

    /// The same parameters with every width-dependent number scaled to `w`.
    ///
    /// A layer printed at its own bead width needs its thresholds moved with it:
    /// the narrowest bead, the widest one, the brim resolution and the feature
    /// floor are all fractions of the nominal width, so scaling them by the same
    /// ratio keeps the generator behaving the way the profile tuned it instead of
    /// falling back to the defaults for the new width.
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
            ..*self
        }
    }

    /// The same parameters asking for `walls` loops.
    pub fn with_walls(&self, walls: usize) -> Params {
        Params { walls, ..*self }
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
}

impl Default for Params {
    fn default() -> Params {
        Params::new(0.4, 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!((wide.h - wide.w / 6.0).abs() < 1e-12);
        assert!((wide.min_feature - 0.15).abs() < 1e-12);
        assert_eq!(wide.walls, p.walls, "the loop count is not a width");
        assert_eq!(wide.gap_fill, p.gap_fill);
        let same = p.scaled_to(0.0);
        assert!((same.w - 0.4).abs() < 1e-12, "zero leaves the band alone");
    }
}
