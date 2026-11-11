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
}

impl Params {
    /// Creates parameters for width `w` and `walls` loops.
    pub fn new(w: f64, walls: usize) -> Params {
        Params {
            w,
            walls,
            w_min: 0.6 * w,
            w_max: 1.5 * w,
            h: w / 8.0,
            central_deg: 135.0,
            min_feature: 0.25 * w,
            smooth: w,
            spur: 2.0 * w,
            ext: 1.5,
            corner_deg: 12.0,
            simplify: 0.002,
            trans_len: 2.0 * w,
            hyst: 0.1,
            taper_min: 0.25 * w,
        }
    }

    /// Radius beyond which no wall can exist.
    pub fn cap(&self) -> f64 {
        self.walls as f64 * self.w + 2.0 * self.h
    }

    /// Outer radius of the requested wall band.
    pub fn thick_radius(&self) -> f64 {
        self.walls as f64 * self.w
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
}
