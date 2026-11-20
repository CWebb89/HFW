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

pub use metrics::{
    bead_area, bead_width, evaluate, evaluate_full, extruded_volume, is_interior, line_volume,
    section_area, Metrics,
};

/// Semantic class of a printable path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// Concentric side loop, counting outwards from the boundary.
    Wall(u8),
    /// Bead straddling a medial-axis centreline.
    Middle,
    /// Solid top or bottom skin.
    Skin,
    /// Solid skin on the visible surface.
    TopSkin,
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
    /// Slow, near-zero-flow pass that flattens a printed top surface.
    Ironing,
}

impl Feature {
    /// Maps a wall-generator path class onto a reported feature.
    pub fn of(kind: Kind) -> Option<Feature> {
        match kind {
            Kind::Side(i) => Some(Feature::Wall(i)),
            Kind::Middle => Some(Feature::Middle),
            Kind::Solid => Some(Feature::Skin),
            Kind::TopSolid => Some(Feature::TopSkin),
            Kind::Infill => Some(Feature::Infill),
            Kind::Brim => Some(Feature::Brim),
            Kind::Bridge => Some(Feature::Bridge),
            Kind::BridgeAnchor => Some(Feature::Bridge),
            Kind::Support => Some(Feature::Support),
            Kind::SupportInterface => Some(Feature::SupportInterface),
            Kind::Ironing => Some(Feature::Ironing),
        }
    }

    /// Stable short name used in the CSV manifest.
    pub fn name(self) -> &'static str {
        match self {
            Feature::Wall(_) => "wall",
            Feature::Middle => "middle",
            Feature::Skin => "skin",
            Feature::TopSkin => "top-skin",
            Feature::Infill => "infill",
            Feature::Brim => "brim",
            Feature::Bridge => "bridge",
            Feature::Support => "support",
            Feature::SupportInterface => "support_interface",
            Feature::Ironing => "ironing",
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
            Feature::TopSkin => "#f2c14e",
            Feature::Infill => "#c8553d",
            Feature::Brim => "#7d5ba6",
            Feature::Bridge => "#b03a48",
            Feature::Ironing => "#5f9ea0",
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
    /// Number of skin lines on the visible top surface.
    pub top_skin: usize,
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
    /// Number of ironing passes over a top surface.
    pub ironing: usize,
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
    /// True when this layer's single wall rises continuously into the next.
    pub vase: bool,
    /// True when this is the first layer of an object.
    ///
    /// A plate printed in `by object` order restarts at Z 0 for every object, so
    /// the writer cannot tell from the layer count which layers sit on the bed.
    pub first: bool,
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
                Some(Feature::TopSkin) => c.top_skin += 1,
                Some(Feature::Infill) => c.infill += 1,
                Some(Feature::Brim) => c.brim += 1,
                Some(Feature::Bridge) => c.bridge += 1,
                Some(Feature::Support) => c.support += 1,
                Some(Feature::SupportInterface) => c.support_interface += 1,
                Some(Feature::Ironing) => c.ironing += 1,
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
    /// Total top-surface skin path count.
    pub top_skin: usize,
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
        self.top_skin += layer.counts.top_skin;
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
                    flow: 1.0,
                    overhang: 0.0,
                    object: 0,
                },
                Line {
                    pts: vec![V2::new(1.0, 5.0), V2::new(19.0, 5.0)],
                    width: vec![0.4; 2],
                    closed: false,
                    kind: Kind::Infill,
                    flow: 1.0,
                    overhang: 0.0,
                    object: 0,
                },
            ],
            counts: LayerCounts::default(),
            vase: false,
            first: true,
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
    fn volume_follows_the_section_each_kind_deposits() {
        let l = layer();
        let wall = bead_area(0.4, 0.2) * 4.0 * 19.6;
        let infill = 0.4 * 0.2 * 18.0;
        assert!((l.counts.volume - (wall + infill)).abs() < 1e-6);
        assert!(wall < 0.9 * 4.0 * 19.6 * 0.4 * 0.2);
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

/// A command the file has to run at a print height.
///
/// This is the plate's timeline, not the profile's: a marker belongs to the job a
/// user is looking at, the way a pause does, so it lives on the plate and travels
/// into the file at its own Z rather than in the settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    /// Print Z the marker fires at, in millimetres. It fires on the first layer whose
    /// bottom is below it and whose top reaches it.
    pub z: f64,
    /// What the marker does when it fires.
    pub kind: MarkerKind,
    /// G-code for [`MarkerKind::Custom`]; ignored for the kinds whose command comes
    /// from the printer profile.
    pub text: String,
}

/// What a [`Marker`] does when its height comes up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkerKind {
    /// Write `text` verbatim.
    Custom,
    /// Stop with the profile's `pause_print_gcode`.
    Pause,
    /// Change filament with the profile's `color_change_gcode`.
    ColourChange,
}

/// Why a marker list cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkerError {
    /// The offending marker, by its index in the list.
    pub index: usize,
    /// What is wrong with it.
    pub reason: String,
}

impl std::fmt::Display for MarkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "marker {}: {}", self.index, self.reason)
    }
}

impl std::error::Error for MarkerError {}

impl Marker {
    /// Returns the markers sorted by Z, refusing any that cannot be written.
    ///
    /// A marker needs a finite Z. `Custom` also needs text: a custom marker with no
    /// G-code in it is a note in the file rather than a command, which is what a pause
    /// whose profile template is empty already becomes.
    pub fn sorted(markers: Vec<Marker>) -> Result<Vec<Marker>, MarkerError> {
        for (index, marker) in markers.iter().enumerate() {
            if !marker.z.is_finite() {
                return Err(MarkerError {
                    index,
                    reason: format!("Z {} is not a number", marker.z),
                });
            }
            if marker.z < 0.0 {
                return Err(MarkerError {
                    index,
                    reason: format!("Z {} mm is below the bed", marker.z),
                });
            }
            if marker.kind == MarkerKind::Custom && marker.text.trim().is_empty() {
                return Err(MarkerError {
                    index,
                    reason: "a custom marker carries no G-code".to_owned(),
                });
            }
        }
        let mut markers = markers;
        markers.sort_by(|a, b| a.z.total_cmp(&b.z));
        Ok(markers)
    }
}

/// A stroke the plate prints inside a layer that the model does not describe.
///
/// This is the calibration channel: a temperature tower's labels, a flow test's
/// lines, a speed tower's bands. They are not part of the mesh, so no slice can
/// produce them, and they carry their own width, height and speed because each band
/// is printed differently on purpose. The writer extrudes them as real beads — travel
/// in, extrude, retract — so E, position and time stay consistent with the rest of
/// the file instead of being pasted in as text.
#[derive(Clone, Debug, PartialEq)]
pub struct InLayerPolyline {
    /// Print Z of the layer the stroke belongs to, in millimetres: the top of that
    /// layer, the same Z the file's own layer markers carry.
    pub z: f64,
    /// The stroke, in plate coordinates in millimetres.
    pub points: Vec<V2>,
    /// Extrusion width of the bead, in millimetres.
    pub line_width: f64,
    /// Height the bead is printed at, in millimetres.
    pub layer_height: f64,
    /// Print speed, in millimetres per second.
    pub speed: f64,
    /// G-code written before the stroke, verbatim.
    ///
    /// A temperature band changes the nozzle temperature immediately before it draws,
    /// so the setting has to travel with the stroke rather than with the layer.
    pub prefix: String,
}

impl InLayerPolyline {
    /// Returns the strokes sorted by Z, refusing any that cannot be printed.
    pub fn sorted(strokes: Vec<InLayerPolyline>) -> Result<Vec<InLayerPolyline>, MarkerError> {
        for (index, stroke) in strokes.iter().enumerate() {
            let fail = |reason: String| Err(MarkerError { index, reason });
            if !stroke.z.is_finite() || stroke.z < 0.0 {
                return fail(format!("Z {} mm is not a height above the bed", stroke.z));
            }
            if stroke.points.len() < 2 {
                return fail(format!(
                    "a stroke of {} point(s) has no length",
                    stroke.points.len()
                ));
            }
            if !stroke
                .points
                .iter()
                .all(|p| p.x.is_finite() && p.y.is_finite())
            {
                return fail("a stroke has a point that is not a number".to_owned());
            }
            for (what, value) in [
                ("line width", stroke.line_width),
                ("layer height", stroke.layer_height),
                ("speed", stroke.speed),
            ] {
                if !value.is_finite() || value <= 0.0 {
                    return fail(format!("{what} {value} mm is not a positive number"));
                }
            }
        }
        let mut strokes = strokes;
        strokes.sort_by(|a, b| a.z.total_cmp(&b.z));
        Ok(strokes)
    }
}

/// Seam paint, as the writer consumes it.
///
/// The facets are triangles in model millimetres: an enforced one says the seam
/// belongs on this face, a blocked one says it never does. The writer tests each
/// candidate seam vertex against them, which is why the paint is geometry here
/// rather than triangle indices — by the time a bead is planned, the triangles it
/// came from are gone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeamPaint {
    /// Facets the seam must be placed on.
    pub enforced: Vec<[[f64; 3]; 3]>,
    /// Facets the seam may not be placed on.
    pub blocked: Vec<[[f64; 3]; 3]>,
}

/// What a paint channel says about a point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Painted {
    /// No painted facet reaches this point.
    Free,
    /// An enforced facet is within reach: the seam belongs here.
    Enforced,
    /// A blocked facet is within reach: the seam may not go here.
    Blocked,
}

impl SeamPaint {
    /// True when neither channel holds a facet.
    pub fn is_empty(&self) -> bool {
        self.enforced.is_empty() && self.blocked.is_empty()
    }

    /// What the paint says about `point`, within `reach` millimetres.
    ///
    /// An enforcer wins over a blocker at the same point: the facet a user
    /// painted *last* is not recorded, so the two cannot be told apart, and
    /// honouring the enforcer keeps a seam on the face that was asked for.
    pub fn state(&self, point: [f64; 3], reach: f64) -> Painted {
        if self
            .enforced
            .iter()
            .any(|facet| point_triangle_distance(point, facet) <= reach)
        {
            return Painted::Enforced;
        }
        if self
            .blocked
            .iter()
            .any(|facet| point_triangle_distance(point, facet) <= reach)
        {
            return Painted::Blocked;
        }
        Painted::Free
    }
}

/// Distance from a point to a triangle, in millimetres.
///
/// The point is projected onto the facet's plane; a projection outside the
/// triangle falls back to the nearest edge, so a vertex of the sliced loop is
/// measured against the face it came from rather than against the plane that
/// face lies in.
pub fn point_triangle_distance(point: [f64; 3], facet: &[[f64; 3]; 3]) -> f64 {
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let mul = |a: [f64; 3], s: f64| [a[0] * s, a[1] * s, a[2] * s];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let length = |a: [f64; 3]| dot(a, a).sqrt();
    let (a, b, c) = (facet[0], facet[1], facet[2]);
    let ab = sub(b, a);
    let ac = sub(c, a);
    let normal = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let normal_len = length(normal);
    if normal_len <= 0.0 {
        return segment_distance(point, a, b).min(segment_distance(point, b, c));
    }
    let to_point = sub(point, a);
    let height = dot(to_point, normal) / normal_len;
    let on_plane = sub(point, mul(normal, height / normal_len));
    if inside_triangle(on_plane, a, b, c) {
        return height.abs();
    }
    segment_distance(point, a, b)
        .min(segment_distance(point, b, c))
        .min(segment_distance(point, c, a))
}

fn inside_triangle(point: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> bool {
    let cross = |p: [f64; 3], q: [f64; 3], r: [f64; 3]| {
        let u = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
        let v = [r[0] - p[0], r[1] - p[1], r[2] - p[2]];
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    };
    let reference = cross(a, b, c);
    let d1 = cross(a, b, point);
    let d2 = cross(b, c, point);
    let d3 = cross(c, a, point);
    let same = |d: [f64; 3]| d[0] * reference[0] + d[1] * reference[1] + d[2] * reference[2] >= 0.0;
    same(d1) && same(d2) && same(d3)
}

fn segment_distance(point: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [point[0] - a[0], point[1] - a[1], point[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if len2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let dx = ap[0] - ab[0] * t;
    let dy = ap[1] - ab[1] * t;
    let dz = ap[2] - ab[2] * t;
    (dx * dx + dy * dy + dz * dz).sqrt()
}
