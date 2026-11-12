//! Layer path assembly and diagnostic export.
//!
//! A planned layer holds two kinds of geometry, deliberately kept apart:
//!
//! * `reference` — the exact model cross-section, straight from the integer
//!   slicer. It is what the mesh actually produced, so it is the right thing
//!   to check a slice against, and it is *not* a printable path.
//! * `paths` — extrudeable bead centre-lines with a width at every vertex,
//!   covering the perimeter shell, thin-wall middle beads, skins and infill.
//!
//! Export formats here are for inspection. Nothing in this crate emits G-code.

#![forbid(unsafe_code)]

pub mod metrics;

use std::fmt::Write as _;

use hfw_kernel::V2;
use hfw_walls::{Kind, Line};

pub use metrics::{evaluate, evaluate_full, extruded_volume, is_interior, line_volume, Metrics};

/// Semantic class of a printable path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// Concentric side loop, counting outwards from the boundary.
    Wall(u8),
    /// Bead straddling a medial-axis centreline.
    Middle,
    /// Solid top or bottom skin.
    Skin,
    /// Sparse infill.
    Infill,
    /// First-layer adhesion brim, outside the part.
    Brim,
    /// Skin printed over air and anchored at both ends.
    Bridge,
    /// Sacrificial support column.
    Support,
    /// Dense support layers in contact with the part.
    SupportInterface,
}

impl Feature {
    /// Maps a wall-generator path class onto a reported feature.
    pub fn of(kind: Kind) -> Option<Feature> {
        match kind {
            Kind::Side(i) => Some(Feature::Wall(i)),
            Kind::Middle => Some(Feature::Middle),
            Kind::Solid => Some(Feature::Skin),
            Kind::Infill => Some(Feature::Infill),
            Kind::Brim => Some(Feature::Brim),
            Kind::Bridge => Some(Feature::Bridge),
            Kind::BridgeAnchor => Some(Feature::Bridge),
            Kind::Support => Some(Feature::Support),
            Kind::SupportInterface => Some(Feature::SupportInterface),
        }
    }

    /// Stable short name used in the CSV manifest.
    pub fn name(self) -> &'static str {
        match self {
            Feature::Wall(_) => "wall",
            Feature::Middle => "middle",
            Feature::Skin => "skin",
            Feature::Infill => "infill",
            Feature::Brim => "brim",
            Feature::Bridge => "bridge",
            Feature::Support => "support",
            Feature::SupportInterface => "support_interface",
        }
    }

    /// SVG stroke colour.
    fn colour(self) -> &'static str {
        match self {
            Feature::Wall(i) => {
                if i == 0 {
                    "#172638"
                } else {
                    "#2f5f8f"
                }
            }
            Feature::Middle => "#1e8c5a",
            Feature::Skin => "#eb9628",
            Feature::Infill => "#c8553d",
            Feature::Brim => "#7d5ba6",
            Feature::Bridge => "#b03a48",
            Feature::Support => "#8a8f98",
            Feature::SupportInterface => "#5f6470",
        }
    }
}

/// Per-feature path and length counters for one layer.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayerCounts {
    /// Number of side loops.
    pub walls: usize,
    /// Number of middle beads.
    pub middle: usize,
    /// Number of skin lines.
    pub skin: usize,
    /// Number of sparse infill lines.
    pub infill: usize,
    /// Number of adhesion brim loops.
    pub brim: usize,
    /// Number of bridge lines.
    pub bridge: usize,
    /// Number of support body lines.
    pub support: usize,
    /// Number of support interface lines.
    pub support_interface: usize,
    /// Total printable path length.
    pub length: f64,
    /// Total deposited volume for this layer.
    pub volume: f64,
    /// Extruder start/stop count.
    pub starts_stops: usize,
}

/// One layer, ready to export.
#[derive(Clone, Debug, Default)]
pub struct PlannedLayer {
    /// Zero-based layer index.
    pub index: usize,
    /// Distance from the bed to the bottom of this layer, in millimetres.
    pub bottom_z: f64,
    /// Height of the slicing plane, in millimetres.
    pub slice_z: f64,
    /// Nozzle height after this layer, in millimetres.
    pub print_z: f64,
    /// Exact model cross-section, for checking the slice.
    pub reference: Vec<Vec<V2>>,
    /// Printable bead paths.
    pub paths: Vec<Line>,
    /// Counters for this layer.
    pub counts: LayerCounts,
}

impl PlannedLayer {
    /// Layer height in millimetres.
    pub fn height(&self) -> f64 {
        self.print_z - self.bottom_z
    }

    /// Recomputes [`LayerCounts`] from the current paths.
    pub fn recount(&mut self) {
        let mut c = LayerCounts::default();
        for path in &self.paths {
            match Feature::of(path.kind) {
                Some(Feature::Wall(_)) => c.walls += 1,
                Some(Feature::Middle) => c.middle += 1,
                Some(Feature::Skin) => c.skin += 1,
                Some(Feature::Infill) => c.infill += 1,
                Some(Feature::Brim) => c.brim += 1,
                Some(Feature::Bridge) => c.bridge += 1,
                Some(Feature::Support) => c.support += 1,
                Some(Feature::SupportInterface) => c.support_interface += 1,
                None => continue,
            }
            c.length += path.length();
            c.volume += line_volume(path, self.height());
            if !path.closed {
                c.starts_stops += 2;
            }
        }
        self.counts = c;
    }

    /// Renders the layer as a diagnostic SVG.
    ///
    /// Coordinates are written in millimetres with Y flipped so the drawing
    /// matches the printer's orientation.
    pub fn to_svg(&self) -> String {
        let mut b = hfw_kernel::Bounds::empty();
        for ring in &self.reference {
            for &p in ring {
                b.add(p);
            }
        }
        for path in &self.paths {
            for &p in &path.pts {
                b.add(p);
            }
        }
        if b.is_empty() {
            b.add(V2::ZERO);
        }
        let pad = 1.0;
        let b = b.grown(pad);
        let (w, h) = (b.width().max(0.001), b.height().max(0.001));
        let mut s = String::new();
        let _ = write!(
            s,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.3}mm\" height=\"{h:.3}mm\" \
             viewBox=\"{:.3} {:.3} {w:.3} {h:.3}\">",
            b.min.x, -b.max.y
        );
        s.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>");
        if !self.reference.is_empty() {
            let mut d = String::new();
            for ring in &self.reference {
                for (i, p) in ring.iter().enumerate() {
                    let _ = write!(
                        d,
                        "{}{:.4},{:.4} ",
                        if i == 0 { "M" } else { "L" },
                        p.x,
                        -p.y
                    );
                }
                d.push('Z');
            }
            let _ = write!(
                s,
                "<path d=\"{d}\" fill=\"#f1f3f7\" fill-rule=\"evenodd\" stroke=\"#9aa3b2\" \
                 stroke-width=\"0.05\" stroke-dasharray=\"0.4,0.4\"/>"
            );
        }
        for path in &self.paths {
            let Some(feature) = Feature::of(path.kind) else {
                continue;
            };
            let m = path.pts.len();
            if m < 2 {
                continue;
            }
            let segn = if path.closed { m } else { m - 1 };
            for k in 0..segn {
                let a = path.pts[k];
                let c = path.pts[(k + 1) % m];
                let wa = path.width[k];
                let wb = path.width[(k + 1) % m];
                let _ = write!(
                    s,
                    "<path d=\"M {:.4} {:.4} L {:.4} {:.4}\" stroke=\"{}\" \
                     stroke-width=\"{:.4}\" stroke-linecap=\"round\" opacity=\"{:.2}\"/>",
                    a.x,
                    -a.y,
                    c.x,
                    -c.y,
                    feature.colour(),
                    0.5 * (wa + wb),
                    if path.kind.is_wall() { 1.0 } else { 0.85 },
                );
            }
        }
        s.push_str("</svg>\n");
        s
    }

    /// One CSV row describing this layer.
    pub fn manifest_row(&self) -> String {
        format!(
            "{},{:.6},{:.6},{:.6},{},{},{},{},{},{},{},{:.4},{:.6},{},{},{}",
            self.index + 1,
            self.bottom_z,
            self.slice_z,
            self.print_z,
            self.reference.len(),
            self.counts.walls,
            self.counts.middle,
            self.counts.skin,
            self.counts.infill,
            self.counts.brim,
            self.counts.bridge,
            self.counts.length,
            self.counts.volume,
            self.counts.starts_stops,
            self.counts.support,
            self.counts.support_interface,
        )
    }
}

/// Header matching [`PlannedLayer::manifest_row`].
pub const MANIFEST_HEADER: &str = "layer_index,layer_bottom_mm,slice_z_mm,print_z_mm,\
reference_contours,wall_paths,middle_paths,skin_paths,infill_paths,brim_paths,\
bridge_paths,path_length_mm,deposited_volume_mm3,starts_stops,support_paths,\
support_interface_paths";

/// Aggregated statistics for a whole job.
#[derive(Clone, Debug, Default)]
pub struct JobTotals {
    /// Layers planned.
    pub layers: usize,
    /// Layers with at least one printable path.
    pub layers_with_paths: usize,
    /// Total printable path length.
    pub length: f64,
    /// Total deposited volume.
    pub volume: f64,
    /// Total extruder start/stop count.
    pub starts_stops: usize,
    /// Total middle-bead path count.
    pub middle: usize,
    /// Total skin path count.
    pub skin: usize,
    /// Total sparse infill path count.
    pub infill: usize,
}

impl JobTotals {
    /// Accumulates one layer.
    pub fn add(&mut self, layer: &PlannedLayer) {
        self.layers += 1;
        if !layer.paths.is_empty() {
            self.layers_with_paths += 1;
        }
        self.length += layer.counts.length;
        self.volume += layer.counts.volume;
        self.starts_stops += layer.counts.starts_stops;
        self.middle += layer.counts.middle;
        self.skin += layer.counts.skin;
        self.infill += layer.counts.infill;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer() -> PlannedLayer {
        let mut l = PlannedLayer {
            index: 0,
            bottom_z: 0.0,
            slice_z: 0.1,
            print_z: 0.2,
            reference: vec![vec![
                V2::new(0.0, 0.0),
                V2::new(20.0, 0.0),
                V2::new(20.0, 20.0),
                V2::new(0.0, 20.0),
            ]],
            paths: vec![
                Line {
                    pts: vec![
                        V2::new(0.2, 0.2),
                        V2::new(19.8, 0.2),
                        V2::new(19.8, 19.8),
                        V2::new(0.2, 19.8),
                    ],
                    width: vec![0.4; 4],
                    closed: true,
                    kind: Kind::Side(0),
                },
                Line {
                    pts: vec![V2::new(1.0, 5.0), V2::new(19.0, 5.0)],
                    width: vec![0.4; 2],
                    closed: false,
                    kind: Kind::Infill,
                },
            ],
            counts: LayerCounts::default(),
        };
        l.recount();
        l
    }

    #[test]
    fn recount_separates_features() {
        let l = layer();
        assert_eq!(l.counts.walls, 1);
        assert_eq!(l.counts.infill, 1);
        assert_eq!(l.counts.starts_stops, 2);
        assert!((l.counts.length - (4.0 * 19.6 + 18.0)).abs() < 1e-6);
    }

    #[test]
    fn volume_matches_length_times_width_times_height() {
        let l = layer();
        let expected = (4.0 * 19.6 + 18.0) * 0.4 * 0.2;
        assert!((l.counts.volume - expected).abs() < 1e-6);
    }

    #[test]
    fn svg_contains_every_feature_and_the_reference() {
        let svg = layer().to_svg();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("#172638"));
        assert!(svg.contains("#c8553d"));
        assert!(svg.contains("fill-rule=\"evenodd\""));
    }

    #[test]
    fn manifest_rows_have_one_column_per_header_field() {
        let header = MANIFEST_HEADER.split(',').count();
        assert_eq!(layer().manifest_row().split(',').count(), header);
    }

    #[test]
    fn totals_accumulate_layers() {
        let l = layer();
        let mut t = JobTotals::default();
        t.add(&l);
        t.add(&l);
        assert_eq!(t.layers, 2);
        assert_eq!(t.layers_with_paths, 2);
        assert!((t.volume - 2.0 * l.counts.volume).abs() < 1e-9);
    }
}
