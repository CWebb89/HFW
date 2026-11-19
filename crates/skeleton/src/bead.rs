//! Turning a section of a layer into beads.
//!
//! A bead is a fixed-width extrusion, so a section of width `t` can only be filled
//! by a whole number of them, and the choice of that number and of the widths is
//! what makes a wall look even or not. A section is measured across the skeleton, so
//! its beads are described as signed offsets from the skeleton sample: the first
//! bead touches one boundary, the last touches the other, and nothing is left over
//! when the width divides.
//!
//! Nothing here adds or removes a bead part-way along an edge yet: a section gets
//! one count, and a count that cannot hold its beads evenly is reported as left
//! over. That is the transition stage's problem, and it is why this module is one
//! step and not the whole wall generator.

/// The layer's cross-section at one sample of the skeleton.
#[derive(Clone, Copy, Debug, Default)]
pub struct Section {
    /// Width of the section, in millimetres.
    pub thickness: f64,
    /// How far the section's centre is from the sample, along the sample's left
    /// normal.
    ///
    /// The skeleton is where two boundaries are equally near, which is the middle of
    /// the layer only between two parallel walls: in the arm of a wedge the middle
    /// sits to one side of it, and a bead placed on the axis would print off-centre.
    pub skew: f64,
}

/// The beads that fill one section of a layer.
#[derive(Clone, Debug, Default)]
pub struct Beading {
    /// Extrusion width of each bead, from one side of the section to the other.
    pub widths: Vec<f64>,
    /// Centre of each bead as a signed offset from the section's centre line,
    /// ascending, in millimetres.
    pub locations: Vec<f64>,
    /// Width of the section the beads do not cover, in millimetres. Never negative.
    pub left_over: f64,
}

impl Beading {
    /// Width of the section the beads do cover.
    pub fn covered(&self) -> f64 {
        self.widths.iter().sum()
    }
}

/// How a section's width is turned into beads.
pub trait Strategy {
    /// Fills a section with at most `count` beads.
    fn compute(&self, section: Section, count: usize) -> Beading;
}

/// Splits a section into equal beads, centred on it.
///
/// Equal widths are what a printer lays down when nothing forces a difference, and
/// centring keeps a symmetric layer symmetric. Whatever the widths cannot cover —
/// nothing, when the section divides evenly — is reported as left over rather than
/// hidden by widening one bead past what the nozzle can do.
#[derive(Clone, Copy, Debug)]
pub struct Even;

impl Strategy for Even {
    fn compute(&self, section: Section, count: usize) -> Beading {
        if count == 0 || section.thickness.is_nan() || section.thickness <= 0.0 {
            return Beading {
                widths: Vec::new(),
                locations: Vec::new(),
                left_over: section.thickness.max(0.0),
            };
        }
        let width = section.thickness / count as f64;
        let locations = (0..count)
            .map(|j| section.skew - section.thickness / 2.0 + width * (j as f64 + 0.5))
            .collect();
        Beading {
            widths: vec![width; count],
            locations,
            left_over: 0.0,
        }
    }
}

/// Caps the number of beads at the perimeters a profile asked for.
///
/// A section wider than the requested perimeters can fill gets its beads at the
/// optimal width and the rest left over, which is what the infill is there to take:
/// printing a fourth wall inside three requested ones is neither what the profile
/// says nor what the part needs.
#[derive(Clone, Copy, Debug)]
pub struct Limited {
    /// The widest bead worth printing, in millimetres.
    pub optimal: f64,
    /// The most beads a section may be filled with.
    pub max_count: usize,
}

impl Strategy for Limited {
    fn compute(&self, section: Section, count: usize) -> Beading {
        let count = count.min(self.max_count);
        if count == 0 || section.thickness.is_nan() || section.thickness <= 0.0 {
            return Beading {
                widths: Vec::new(),
                locations: Vec::new(),
                left_over: section.thickness.max(0.0),
            };
        }
        let wanted = if self.optimal > 0.0 {
            ((section.thickness / self.optimal).round() as usize).max(1)
        } else {
            count
        };
        if wanted <= count {
            return Even.compute(section, count);
        }
        let t = section.thickness;
        let w = self.optimal;
        let mut locations = Vec::with_capacity(count);
        let widths = vec![w; count];
        let first_side = count.div_ceil(2);
        let second_side = count - first_side;
        for j in 0..first_side {
            locations.push(section.skew - t / 2.0 + w * (j as f64 + 0.5));
        }
        for j in 0..second_side {
            locations.push(section.skew + t / 2.0 - w * (j as f64 + 0.5));
        }
        let covered: f64 = widths.iter().sum();
        Beading {
            widths,
            locations,
            left_over: (t - covered).max(0.0),
        }
    }
}

/// The bead count a section of `thickness` wants, given a nominal width.
///
/// At least one bead for any printable width: a section narrower than a bead is
/// still material, and printing it thin is better than not printing it.
pub fn count_for(thickness: f64, optimal: f64, max_count: usize) -> usize {
    if thickness.is_nan() || thickness <= 0.0 || max_count == 0 {
        return 0;
    }
    if optimal.is_nan() || optimal <= 0.0 {
        return 1;
    }
    ((thickness / optimal).round() as usize).clamp(1, max_count)
}
