//! Public headless facade for the HFW slicing engine.
//!
//! The pipeline spans two number domains by design. Mesh import and slicing run
//! in exact integer nanometres, so contour reconstruction is deterministic and
//! open or branching cross-sections are rejected instead of guessed. Everything
//! after that — distance fields, medial axes, offsets, infill — is numerical and
//! runs in millimetres. [`region_from_contours`] is the single conversion point.
//!
//! This crate stops at planned bead paths. It does not emit G-code.

#![forbid(unsafe_code)]

use hfw_config::PrinterConfig;
use hfw_geometry::Contour;
use hfw_infill::{infill_layer as generate_infill, Combine};
use hfw_kernel::Region;
use hfw_slicing::{LayerSchedule, SliceError, SliceLayer};
use hfw_support::SupportParams;
use hfw_toolpath::{evaluate, evaluate_full, extruded_volume, PlannedLayer};
use hfw_walls::BrimSide;

pub use hfw_geometry::{Contour as SliceContour, Point2};
mod settings_key;
pub use hfw_infill::{
    infill_layer, InfillParams, InfillParams as InfillSettings, IroningTarget, MaskCache,
    Pattern as InfillPattern, SkinPattern, VerticalShell,
};
pub use hfw_mesh::{
    load_3mf_objects, three_mf_metadata, Mesh, MeshDiagnostics, MeshError, MeshRepairReport,
    MeshTransform, ModelObject, Paint, PAINT_SEAM_BLOCKER, PAINT_SEAM_ENFORCER, PAINT_SEAM_NONE,
    PAINT_SUPPORT_BLOCKER, PAINT_SUPPORT_ENFORCER, PAINT_SUPPORT_NONE,
};
pub use hfw_slicing::{
    slice_mesh, SliceError as SlicingError, SliceLayer as Slice, MAX_CLOSE_GAPS,
};
pub use hfw_support::{PaintRegions, PaintSpan};
pub use hfw_support::{
    Style as SupportStyle, SupportParams as SupportSettings, SupportPattern as SupportFill,
};
pub use hfw_toolpath::{
    evaluate as evaluate_perimeter, evaluate_full as evaluate_all, line_volume, JobTotals, Metrics,
    PlannedLayer as PlannedSlice, MANIFEST_HEADER,
};
pub use hfw_toolpath::{Marker, MarkerError, MarkerKind};
pub use hfw_toolpath::{Painted, SeamPaint};
pub use hfw_walls::{
    generate as generate_walls, Kind as PathKind, Line as Path, WallMode as Mode, WallParams,
};
pub use settings_key::{KeyError, PER_OBJECT_KEYS};

/// Engine-wide settings for one job.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Wall generation parameters, including extrusion width and loop count.
    pub wall: WallParams,
    /// Wall strategy.
    pub mode: Mode,
    /// Support settings; `None` skips support generation.
    pub support: Option<SupportParams>,
    /// Infill settings; `None` skips infill entirely.
    pub infill: Option<InfillParams>,
    /// Generate walls on every Nth layer, plus the last. 1 means every layer.
    pub every: usize,
    /// Sampling resolution for coverage metrics; `None` skips metrics.
    pub metrics_res: Option<f64>,
    /// Keep the exact slice outline on each planned layer for inspection.
    pub keep_reference: bool,
    /// Fail the whole job on the first layer that cannot be closed.
    ///
    /// Defaults to false: a single degenerate plane, such as a cone tip that
    /// collapses to a point, is skipped and reported instead of discarding an
    /// otherwise printable stack.
    pub strict_slices: bool,
    /// Adhesion brim loops around the first layer. 0 disables the brim.
    pub brim_loops: usize,
    /// Clearance between the part and the innermost brim loop, in millimetres.
    pub brim_gap: f64,
    /// Gap between two brim loops, in millimetres.
    pub brim_separation: f64,
    /// Shrink applied to the first layer only, in millimetres.
    ///
    /// Squash against the bed widens the first layer; pulling that cross-section
    /// in by the measured overshoot removes the resulting lip.
    pub elephant_foot: f64,
    /// Growth applied to every layer, in millimetres, positive outward.
    ///
    /// Grows the object and closes its holes when positive, shrinks it and opens
    /// them when negative.
    pub xy_size: f64,
    /// Cracks narrower than twice this are filled while slicing, in millimetres.
    ///
    /// Upstream's default is 0.049: a hairline between two facets is an artefact
    /// of the mesh, not a feature, and a crack that wide prints as a seam rather
    /// than a wall.
    pub closing_radius: f64,
    /// Largest gap between two outline endpoints that slice-time closing may
    /// bridge, in millimetres.
    ///
    /// Slice assembly joins the nearest open endpoints in stages — a nanometre,
    /// a micrometre, a tenth of a millimetre, a millimetre, then this value —
    /// so a mesh whose facets almost meet is closed at the smallest gap that
    /// works. Anything still open at this limit is reported per layer, and is
    /// fatal under [`Settings::strict_slices`].
    pub close_gaps: f64,
    /// Maximum deviation allowed when simplifying a sliced cross-section, in
    /// millimetres. Zero keeps every vertex the slice produced.
    pub resolution: f64,
    /// Side of the part outline the brim grows on.
    pub brim_side: BrimSide,
    /// Iron the visible top surface after the skin has been laid.
    pub ironing: bool,
    /// Which surfaces an ironing pass covers.
    pub ironing_target: IroningTarget,
    /// Distance between ironing passes, in millimetres.
    pub ironing_spacing: f64,
    /// Print one continuously rising wall above the base, with no infill, no
    /// skin and no top surface.
    pub spiral_vase: bool,
    /// Per-height layer steps, `(z, height)` in millimetres; empty means a
    /// uniform stack at `layer_height`.
    pub layer_profile: Vec<(f64, f64)>,
    /// Thinnest layer the profile may ask for, in millimetres.
    pub min_layer_height: f64,
    /// Thickest layer the profile may ask for, in millimetres.
    pub max_layer_height: f64,
    /// Measure how much of each path hangs over air, so the writer can pick an
    /// overhang speed and an overhang fan.
    ///
    /// Off by default: it costs one mask lookup per sampled vertex, and a profile
    /// that never asks for an overhang speed cannot use the answer.
    pub measure_overhang: bool,
    /// Nozzle diameter, in millimetres.
    ///
    /// Only the settings that are defined against the nozzle need it here: the
    /// ceiling on a combined infill layer, and the autamatic width rule.
    pub nozzle: f64,
    /// Bead width of the first layer; zero keeps the nominal width.
    ///
    /// The whole first layer moves with it — walls, skin, infill and brim —
    /// because a first layer printed with one bead width and sliced with another
    /// deposits the wrong amount of plastic everywhere at once.
    pub first_layer_width: f64,
    /// Add a perimeter on layers that step out over the one below.
    pub extra_perimeters: bool,
    /// Add a loop that follows the unsupported part of a layer.
    pub extra_perimeters_overhangs: bool,
    /// Print one perimeter on the first layer and no inner loops.
    pub one_perimeter_first_layer: bool,
    /// One perimeter on flat top surfaces: empty for none, or `top`/`topmost`.
    pub one_perimeter_top: String,
    /// Threads the wall phase may use. Zero asks the machine.
    ///
    /// Only the wall phase is parallel: it is a pure function of one layer's
    /// cross-section, while infill and support share a mask cache that is
    /// deliberately small and would have to be duplicated per thread.
    pub threads: usize,
    /// Height the toolhead can pass over a finished object, in millimetres; zero
    /// means unmeasured.
    ///
    /// Carried here so the sequential check runs inside the one function that
    /// turns a plate into a job, whatever front end asked for it.
    pub gantry_height: f64,
    /// Radius of the toolhead around the nozzle, in millimetres; zero means
    /// unmeasured.
    ///
    /// The gap two objects printed one at a time must keep between them, because
    /// the head is beside a finished part while the next one's first layers go
    /// down.
    pub extruder_clearance_radius: f64,
}

impl Settings {
    /// Defaults for extrusion width `w` and `walls` loops.
    pub fn new(w: f64, walls: usize) -> Settings {
        Settings {
            spiral_vase: false,
            layer_profile: Vec::new(),
            min_layer_height: 0.05,
            max_layer_height: 0.0,
            wall: WallParams::new(w, walls),
            mode: Mode::Hfw,
            infill: Some(InfillParams::new(w)),
            support: None,
            every: 1,
            metrics_res: None,
            keep_reference: true,
            strict_slices: false,
            brim_loops: 0,
            ironing: false,
            ironing_target: IroningTarget::TopSurfaces,
            ironing_spacing: 0.1,
            brim_gap: 0.1,
            brim_separation: 0.0,
            elephant_foot: 0.0,
            xy_size: 0.0,
            closing_radius: 0.049,
            close_gaps: 2.0,
            resolution: 0.0,
            brim_side: BrimSide::Outer,
            measure_overhang: false,
            first_layer_width: 0.0,
            extra_perimeters: false,
            extra_perimeters_overhangs: false,
            one_perimeter_first_layer: false,
            one_perimeter_top: String::new(),
            nozzle: 0.4,
            threads: 0,
            gantry_height: 0.0,
            extruder_clearance_radius: 0.0,
        }
    }
}

impl Default for Settings {
    fn default() -> Settings {
        Settings::new(0.4, 3)
    }
}

impl Settings {
    /// Layers a preset's process keys onto these settings.
    ///
    /// This is the one reader every front end uses: the command line calls it
    /// after parsing its own flags, the C ABI calls it for the profile a caller
    /// handed over. A key added here reaches both at once, and a key missed here
    /// is missed by both — which is what keeps a preset from meaning two things.
    /// Machine and filament keys that only the G-code writer reads stay on the
    /// [`PrinterConfig`] itself.
    pub fn apply_printer(&mut self, cfg: &PrinterConfig) {
        self.spiral_vase = cfg.spiral_vase;
        if cfg.variable_layer_height {
            self.layer_profile = cfg.layer_height_profile.clone();
        }
        self.min_layer_height = cfg.min_layer_height;
        self.max_layer_height = cfg.max_layer();
        self.wall.w = if cfg.wall_width > 0.0 {
            cfg.wall_width
        } else {
            cfg.line_width
        };
        self.wall.w_outer = cfg.outer_wall_width;
        self.ironing = cfg.ironing;
        self.ironing_target = match cfg.ironing_type.as_str() {
            "all_solid" => IroningTarget::AllSolid,
            "topmost_only" => IroningTarget::TopmostOnly,
            _ => IroningTarget::TopSurfaces,
        };
        self.ironing_spacing = cfg.ironing_spacing;
        self.wall.walls = cfg.walls;
        self.wall.thin_walls = cfg.thin_walls;
        self.wall.gap_fill = cfg.gap_fill_enabled;
        self.wall.distribution = cfg.wall_distribution_count;
        self.nozzle = cfg.nozzle_diameter;
        self.first_layer_width = cfg.first_layer_width;
        self.extra_perimeters = cfg.extra_perimeters;
        self.extra_perimeters_overhangs = cfg.extra_perimeters_on_overhangs;
        self.one_perimeter_first_layer = cfg.only_one_perimeter_first_layer;
        self.one_perimeter_top = if cfg.top_one_perimeter_type == "none" {
            String::new()
        } else {
            cfg.top_one_perimeter_type.clone()
        };
        self.gantry_height = cfg.gantry_height;
        self.extruder_clearance_radius = cfg.extruder_clearance_radius;
        self.measure_overhang =
            cfg.overhangs && (cfg.dynamic_overhang_speeds || cfg.dynamic_fan_speeds);
        if cfg.wall_transition_angle > 0.0 {
            self.wall.corner_deg = cfg.wall_transition_angle;
        }
        if cfg.perimeter_generator == "classic" {
            self.mode = Mode::Classic;
        }
        self.wall.min_feature = if cfg.min_feature_size >= 0.0 {
            cfg.min_feature_size
        } else {
            0.25 * cfg.nozzle_diameter
        };
        self.brim_loops = cfg.adhesion_brim_loops();
        self.brim_gap = cfg.brim_gap;
        self.elephant_foot = cfg.elefant_foot_compensation;
        self.xy_size = cfg.xy_size_compensation;
        self.closing_radius = cfg.slice_closing_radius;
        self.resolution = cfg.resolution;
        if let Some(ip) = self.infill.as_mut() {
            ip.density = cfg.infill_density;
            ip.width = if cfg.infill_width > 0.0 {
                cfg.infill_width
            } else {
                cfg.line_width
            };
            ip.solid_width = cfg.solid_infill_width;
            ip.top_width = cfg.top_infill_width;
            ip.top_pattern = skin_pattern(&cfg.top_fill_pattern).unwrap_or(SkinPattern::Monotonic);
            ip.bottom_pattern =
                skin_pattern(&cfg.bottom_fill_pattern).unwrap_or(SkinPattern::Monotonic);
            ip.solid_every = cfg.solid_infill_every_layers;
            ip.solid_below_area = cfg.solid_infill_below_area;
            ip.combine_layers = cfg.infill_every_layers.max(1);
            ip.combine_auto = cfg.automatic_infill_combination;
            ip.combine_max_height = cfg.infill_combination_max_layer_height;
            ip.top_layers = solid_layers(
                cfg.top_layers,
                cfg.top_solid_min_thickness,
                cfg.layer_height,
            );
            ip.bottom_layers = solid_layers(
                cfg.bottom_layers,
                cfg.bottom_solid_min_thickness,
                cfg.layer_height,
            );
            ip.vertical_shell = match cfg.ensure_vertical_shell_thickness.as_str() {
                "disabled" => VerticalShell::Disabled,
                "partial" => VerticalShell::Partial,
                _ => VerticalShell::Enabled,
            };
            ip.overlap = if cfg.infill_overlap >= 0.0 {
                cfg.infill_overlap * ip.width
            } else {
                0.1 * ip.width
            };
            ip.pattern = infill_pattern(&cfg.infill_pattern).unwrap_or(InfillPattern::Rectilinear);
            ip.fill_angle = cfg.fill_angle;
            ip.bridge.density = cfg.bridge_density;
            ip.bridge.cross_hatch = cfg.bridge_cross_hatch;
            ip.bridge.width = if cfg.bridge_line_width > 0.0 {
                cfg.bridge_line_width
            } else {
                cfg.line_width
            };
            ip.bridge.angle = if cfg.bridge_angle >= 0.0 {
                Some(cfg.bridge_angle)
            } else {
                None
            };
            ip.bridge.max_length = cfg.max_bridge_length;
            ip.anchor = if cfg.infill_anchor >= 0.0 {
                cfg.infill_anchor
            } else {
                6.0 * ip.width
            };
            ip.anchor_max = cfg.infill_anchor_max;
        }
        self.support = if cfg.support {
            Some(support_from(cfg))
        } else {
            None
        };
    }
}

/// Maps a profile's infill-pattern name onto the geometry this engine draws.
///
/// The dialect names are Orca's and PrusaSlicer's, and a profile may carry a
/// pattern this engine has no geometry for. Those are reported as unapplied by
/// the intake rather than silently printed as something else, so this returns
/// `None` and the caller decides.
pub fn infill_pattern(name: &str) -> Option<InfillPattern> {
    match name.trim().to_ascii_lowercase().as_str() {
        "grid" => Some(InfillPattern::Grid),
        "rectilinear" | "rectilinear_infill" => Some(InfillPattern::Rectilinear),
        "line" | "lines" => Some(InfillPattern::Line),
        "triangle" | "triangles" => Some(InfillPattern::Triangle),
        "cubic" => Some(InfillPattern::Cubic),
        "gyroid" => Some(InfillPattern::Gyroid),
        "3d_honeycomb" | "3dhoneycomb" | "honeycomb_3d" => Some(InfillPattern::Honeycomb3D),
        "concentric" => Some(InfillPattern::Concentric),
        "zigzag" | "zig_zag" => Some(InfillPattern::ZigZag),
        "crosshatch" | "cross_hatch" => Some(InfillPattern::CrossHatch),
        "honeycomb" | "hexagon" => Some(InfillPattern::Honeycomb),
        "monotonic" => Some(InfillPattern::Monotonic),
        "monotonicline" | "monotoniclines" | "monotonic_line" => {
            Some(InfillPattern::MonotonicLines)
        }
        _ => None,
    }
}

/// The dialect name of an infill pattern: what [`infill_pattern`] reads back.
///
/// A project file records settings as names, so the pair lives here; a name this
/// returns is always one the parser accepts.
pub fn infill_pattern_name(pattern: InfillPattern) -> &'static str {
    match pattern {
        InfillPattern::Grid => "grid",
        InfillPattern::Rectilinear => "rectilinear",
        InfillPattern::Line => "line",
        InfillPattern::Triangle => "triangle",
        InfillPattern::Cubic => "cubic",
        InfillPattern::Gyroid => "gyroid",
        InfillPattern::Honeycomb3D => "3d_honeycomb",
        InfillPattern::Concentric => "concentric",
        InfillPattern::ZigZag => "zigzag",
        InfillPattern::CrossHatch => "crosshatch",
        InfillPattern::Honeycomb => "honeycomb",
        InfillPattern::Monotonic => "monotonic",
        InfillPattern::MonotonicLines => "monotoniclines",
    }
}

/// Maps a profile's skin-pattern name onto the skin fill this engine draws.
///
/// The parser has already refused anything outside this list, so the caller
/// decides what an unknown name means: the profile path takes the default,
/// while a per-object key refuses it and says so.
fn skin_pattern(name: &str) -> Option<SkinPattern> {
    match name.trim().to_ascii_lowercase().as_str() {
        "rectilinear" => Some(SkinPattern::Rectilinear),
        "monotoniclines" => Some(SkinPattern::MonotonicLines),
        "alignedrectilinear" => Some(SkinPattern::AlignedRectilinear),
        "concentric" => Some(SkinPattern::Concentric),
        "hilbertcurve" => Some(SkinPattern::HilbertCurve),
        "archimedeanchords" => Some(SkinPattern::ArchimedeanChords),
        "octagramspiral" => Some(SkinPattern::OctagramSpiral),
        "monotonic" => Some(SkinPattern::Monotonic),
        _ => None,
    }
}

/// The dialect name of a skin pattern: what [`skin_pattern`] reads back.
pub fn skin_pattern_name(pattern: SkinPattern) -> &'static str {
    match pattern {
        SkinPattern::Rectilinear => "rectilinear",
        SkinPattern::MonotonicLines => "monotoniclines",
        SkinPattern::AlignedRectilinear => "alignedrectilinear",
        SkinPattern::Concentric => "concentric",
        SkinPattern::HilbertCurve => "hilbertcurve",
        SkinPattern::ArchimedeanChords => "archimedeanchords",
        SkinPattern::OctagramSpiral => "octagramspiral",
        SkinPattern::Monotonic => "monotonic",
    }
}

/// Solid top and bottom layer count, raised to the thickness the profile asks
/// for when that needs more layers than the count states.
fn solid_layers(configured: usize, min_thickness: f64, layer_height: f64) -> usize {
    if min_thickness <= 0.0 || layer_height <= 0.0 {
        return configured;
    }
    let from_thickness = (min_thickness / layer_height).ceil() as usize;
    configured.max(from_thickness)
}

/// Turns a profile's support keys into the support generator's parameters.
fn support_from(cfg: &PrinterConfig) -> SupportParams {
    let mut sp = SupportParams::new(if cfg.support_width > 0.0 {
        cfg.support_width
    } else {
        cfg.line_width
    });
    sp.interface_width = cfg.support_interface_width;
    let interface_width = if sp.interface_width > 0.0 {
        sp.interface_width
    } else {
        sp.width
    };
    let density_from_spacing = |width: f64, spacing: f64| {
        if spacing == 0.0 {
            1.0
        } else {
            (width / spacing).min(1.0)
        }
    };
    sp.threshold_angle = cfg.support_threshold_angle;
    sp.xy_gap = cfg.support_xy_gap;
    sp.z_gap = cfg.support_z_gap;
    sp.interface_layers = cfg.support_interface_layers;
    sp.density = if cfg.support_spacing >= 0.0 {
        density_from_spacing(sp.width, cfg.support_spacing)
    } else {
        cfg.support_density
    };
    sp.interface_density = if cfg.support_interface_spacing >= 0.0 {
        density_from_spacing(interface_width, cfg.support_interface_spacing)
    } else {
        cfg.support_interface_density
    };
    sp.on_build_plate_only = cfg.support_on_build_plate_only;
    sp.min_area = cfg.support_min_area;
    sp.style = if cfg.support_style == "tree" {
        SupportStyle::Tree
    } else {
        SupportStyle::Normal
    };
    sp.branch_angle = cfg.support_branch_angle;
    sp.branch_distance = cfg.support_branch_distance;
    sp.tip_diameter = cfg.support_tip_diameter;
    sp.branch_diameter_angle = cfg.support_branch_diameter_angle;
    sp.max_branch_diameter = cfg.support_max_branch_diameter;
    sp.pattern = if cfg.support_pattern == "grid" {
        SupportFill::Grid
    } else {
        SupportFill::Rectilinear
    };
    sp.first_layer_density = cfg.support_first_layer_density;
    sp.interface_pattern = match cfg.support_interface_pattern.as_str() {
        "grid" => Some(SupportFill::Grid),
        "rectilinear" => Some(SupportFill::Rectilinear),
        _ => None,
    };
    sp.interface_loops = if cfg.support_interface_shells {
        cfg.support_interface_loops.max(2)
    } else {
        cfg.support_interface_loops
    };
    sp.sheath = cfg.support_sheath;
    sp.first_layer_expansion = cfg.support_first_layer_expansion;
    sp.enforce_layers = cfg.support_enforce_layers;
    sp.bottom_interface_layers = cfg.support_bottom_interface_layers;
    sp.tree_angle_slow = cfg.support_tree_angle_slow;
    sp.tree_top_rate = cfg.support_tree_top_rate;
    sp.tree_double_wall = cfg.support_tree_double_wall;
    sp.closing_radius = cfg.support_closing_radius;
    sp.bottom_contact_distance = cfg.support_bottom_contact_distance;
    sp.skip_bridges = cfg.support_skip_bridges;
    sp.bridge_span = cfg.max_bridge_length;
    sp
}

/// Errors that can stop a job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineError {
    /// Slicing failed: bad layer height, open cross-section, or bad input range.
    Slice(SliceError),
    /// The always-on mesh preparation could not rebuild the mesh.
    Mesh(MeshError),
    /// Spiral vase and support were asked for at once.
    ///
    /// A vase is one wall that climbs continuously; a support pillar is a solid
    /// column that would stand through it. Neither can be dropped to satisfy the
    /// other, so the combination is refused rather than half printed.
    VaseWithSupport,
    /// A painted state the engine does not know.
    Paint(String),
}

impl From<SliceError> for EngineError {
    fn from(error: SliceError) -> Self {
        EngineError::Slice(error)
    }
}

impl From<MeshError> for EngineError {
    fn from(error: MeshError) -> Self {
        EngineError::Mesh(error)
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Slice(e) => write!(f, "slicing failed: {e}"),
            EngineError::Mesh(e) => write!(f, "mesh preparation failed: {e}"),
            EngineError::VaseWithSupport => write!(
                f,
                "spiral vase mode cannot carry support; clear one of the two"
            ),
            EngineError::Paint(message) => write!(f, "paint: {message}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Converts integer slice contours into a millimetre region.
///
/// This is the only place where the exact integer pipeline hands over to the
/// numerical one. Coordinates divide exactly by 1 000 000, so no rounding is
/// introduced beyond the float conversion itself.
///
/// Windings are material semantics, not nesting guesses: every counter-clockwise
/// contour adds material and every clockwise one removes it. Two shells that
/// overlap therefore print their union, and a shell drawn inside another with
/// the same winding is still material rather than a hole.
pub fn region_from_contours(contours: &[Contour]) -> Region {
    if contours.is_empty() {
        return Region::default();
    }
    let mm_ring = |c: &Contour| -> Vec<hfw_kernel::V2> {
        c.points()
            .iter()
            .map(|p| p.to_mm())
            .map(|xy| hfw_kernel::V2::new(xy[0], xy[1]))
            .collect()
    };
    let mut solid: Vec<Vec<hfw_kernel::V2>> = Vec::new();
    let mut holes: Vec<Vec<hfw_kernel::V2>> = Vec::new();
    for contour in contours {
        if contour.is_ccw() {
            solid.push(mm_ring(contour));
        } else {
            holes.push(mm_ring(contour));
        }
    }
    let mut merged: Vec<Vec<hfw_kernel::V2>> = Vec::new();
    for ring in solid {
        merged = if merged.is_empty() {
            vec![ring]
        } else {
            hfw_kernel::union_rings(&merged, std::slice::from_ref(&ring))
        };
    }
    let mut cut: Vec<Vec<hfw_kernel::V2>> = Vec::new();
    for ring in holes {
        cut = if cut.is_empty() {
            vec![ring]
        } else {
            hfw_kernel::union_rings(&cut, std::slice::from_ref(&ring))
        };
    }
    let mut rings = merged;
    for hole in cut {
        rings = hfw_kernel::difference_rings(&rings, std::slice::from_ref(&hole));
    }
    if rings.is_empty() {
        Region::default()
    } else {
        Region::new(rings)
    }
}

/// Timing and size counters for one job.
#[derive(Clone, Copy, Debug, Default)]
pub struct JobStats {
    /// Layers in the schedule.
    pub layers: usize,
    /// Layers whose walls were generated.
    pub processed_layers: usize,
    /// Triangles intersected across the whole stack.
    pub triangles_tested: usize,
    /// Perimeter loops emitted in total.
    pub wall_loops: usize,
    /// Gap-fill strands emitted in total.
    pub gap_strands: usize,
    /// Wall path vertices emitted in total.
    pub wall_points: usize,
    /// Milliseconds spent slicing.
    pub ms_slice: f64,
    /// Milliseconds spent generating walls.
    pub ms_walls: f64,
    /// Milliseconds spent generating infill.
    pub ms_infill: f64,
    /// Milliseconds spent generating support masks.
    pub ms_support: f64,
    /// Milliseconds spent measuring coverage.
    pub ms_metrics: f64,
    /// Milliseconds spent offsetting cross-sections for size compensation.
    pub ms_offset: f64,
    /// Milliseconds the wall phase took, wall clock, across all threads.
    pub ms_walls_elapsed: f64,
    /// Threads the wall phase used; one when the job is too small to split.
    pub threads: usize,
}

/// Wall generation for one layer: the brim that sits under it, the beads
/// themselves, and the generator's counters.
struct WallPass {
    lines: Vec<hfw_walls::Line>,
    stats: hfw_walls::Stats,
}

/// A second body of geometry attached to an object.
///
/// Volumes are how a document says "this part, plus this block subtracted, plus
/// support that must not grow here". They are geometry and nothing else: a
/// modifier volume's per-region *settings* are a separate feature, because a
/// layer carrying two settings is two planned regions, not one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VolumeKind {
    /// Adds material: the volume's cross-section is united with the object's.
    Part,
    /// Removes material: the volume's cross-section is subtracted.
    Negative,
    /// Forbids support inside the volume's footprint, over its height.
    SupportBlocker,
    /// Requires support under the volume's footprint, over its height.
    SupportEnforcer,
}

/// A volume with the mesh it is made of.
#[derive(Clone, Copy, Debug)]
pub struct Volume<'a> {
    /// The geometry, already placed in the object's frame.
    pub mesh: &'a Mesh,
    /// What the volume does.
    pub kind: VolumeKind,
}

/// A sliced and planned job.
pub struct Job {
    /// Mesh diagnostics of the mesh that was actually sliced.
    pub mesh_diagnostics: MeshDiagnostics,
    /// What the always-on mesh preparation changed, if anything.
    pub repair: MeshRepairReport,
    /// Layers in print order.
    pub layers: Vec<PlannedLayer>,
    /// Per-layer coverage metrics, when requested.
    pub metrics: Vec<Metrics>,
    /// Perimeter volume and interior volume.
    pub volume: (f64, f64),
    /// Counters and timings.
    pub stats: JobStats,
    /// Largest per-layer wall generation time, in milliseconds.
    pub worst_layer_ms: f64,
    /// Index of the layer that took [`Job::worst_layer_ms`].
    pub worst_layer: usize,
    /// Layers that produced no closed cross-section, with the reason.
    pub skipped: Vec<(usize, String)>,
    /// Commands the plate runs at a height, earliest first.
    ///
    /// The job carries them rather than the settings because they belong to this
    /// plate, not to the profile: two plates on one printer can pause at different
    /// heights. The writer emits them at the layer that reaches their Z.
    pub markers: Vec<Marker>,
    /// Seam paint, one entry per object label a path can carry.
    ///
    /// Empty when nothing on the plate was painted. The writer indexes it by
    /// `Line::object`, so a plate whose second part is painted carries an empty
    /// entry for its first.
    pub seam_paint: Vec<SeamPaint>,
}

/// How a plate of objects becomes one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateOrder {
    /// Every object's layer N, then every object's layer N+1.
    ByLayer,
    /// One object's whole stack, then the next object's.
    ByObject,
}

/// Composes objects sliced on their own into one plate job.
///
/// Each object is sliced and planned independently, which is what lets two
/// objects on one plate carry different settings *and different layer heights*;
/// the stacks are interleaved into print order here. Paths are relabelled with the
/// object they belong to, because that label is the only thing that changes their
/// order within a layer, and the per-object counters, metrics and skipped layers
/// are folded into the job's.
///
/// `by layer` is a merge on Z, not on layer number: two objects with different layer
/// heights do not agree on what layer 7 is, so each step takes the next layer of
/// whichever object sits lowest. Objects with the same heights therefore come out in
/// object order within a layer, exactly as a round-robin would put them.
pub fn merge_stacks(mut stacks: Vec<Job>, order: PlateOrder, diagnostics: MeshDiagnostics) -> Job {
    let mut job = Job {
        mesh_diagnostics: diagnostics,
        repair: MeshRepairReport::default(),
        layers: Vec::new(),
        metrics: Vec::new(),
        volume: (0.0, 0.0),
        stats: JobStats::default(),
        worst_layer_ms: 0.0,
        worst_layer: 0,
        skipped: Vec::new(),
        markers: Vec::new(),
        seam_paint: Vec::new(),
    };
    let mut sequence: Vec<(usize, usize)> = Vec::new();
    match order {
        PlateOrder::ByObject => {
            for (object, stack) in stacks.iter().enumerate() {
                for index in 0..stack.layers.len() {
                    sequence.push((object, index));
                }
            }
        }
        PlateOrder::ByLayer => {
            let mut cursor = vec![0usize; stacks.len()];
            loop {
                let mut lowest: Option<usize> = None;
                for (candidate, stack) in stacks.iter().enumerate() {
                    let Some(layer) = stack.layers.get(cursor[candidate]) else {
                        continue;
                    };
                    lowest = Some(match lowest {
                        None => candidate,
                        Some(best) => {
                            if layer.print_z < stacks[best].layers[cursor[best]].print_z {
                                candidate
                            } else {
                                best
                            }
                        }
                    });
                }
                let Some(object) = lowest else {
                    break;
                };
                sequence.push((object, cursor[object]));
                cursor[object] += 1;
            }
        }
    }
    for (object, layer_index) in sequence {
        let Some(stack) = stacks.get_mut(object) else {
            continue;
        };
        let base = job.layers.len();
        let Some(mut layer) = stack.layers.get(layer_index).cloned() else {
            continue;
        };
        if layer.paths.is_empty() && !layer.first {
            continue;
        }
        layer.first = layer_index == 0;
        layer.index = base;
        let label = u16::try_from(object).unwrap_or(u16::MAX);
        for line in &mut layer.paths {
            line.object = label;
        }
        job.layers.push(layer);
    }
    for stack in stacks.iter_mut() {
        let base = job.layers.len();
        for (offset, reason) in std::mem::take(&mut stack.skipped) {
            job.skipped.push((base + offset, reason));
        }
        job.metrics.append(&mut stack.metrics);
        job.volume.0 += stack.volume.0;
        job.volume.1 += stack.volume.1;
        add_stats(&mut job.stats, &stack.stats);
        job.seam_paint
            .push(stack.seam_paint.first().cloned().unwrap_or_default());
        if stack.worst_layer_ms > job.worst_layer_ms {
            job.worst_layer_ms = stack.worst_layer_ms;
            job.worst_layer = base + stack.worst_layer;
        }
    }
    if job.seam_paint.iter().all(SeamPaint::is_empty) {
        job.seam_paint.clear();
    }
    job
}

/// Adds one object's counters into the plate's.
fn add_stats(total: &mut JobStats, extra: &JobStats) {
    total.layers += extra.layers;
    total.processed_layers += extra.processed_layers;
    total.triangles_tested += extra.triangles_tested;
    total.wall_loops += extra.wall_loops;
    total.gap_strands += extra.gap_strands;
    total.wall_points += extra.wall_points;
    total.ms_slice += extra.ms_slice;
    total.ms_walls += extra.ms_walls;
    total.ms_infill += extra.ms_infill;
    total.ms_support += extra.ms_support;
    total.ms_metrics += extra.ms_metrics;
    total.ms_offset += extra.ms_offset;
}

/// True when an ironing pass belongs on this layer.
///
/// `top_surfaces` and `all_solid` differ in which mask they iron, not in which
/// layers they run on; `topmost_only` runs on the last layer of the stack, where
/// the highest visible top surface is.
fn ironing_covers(target: IroningTarget, topmost: bool) -> bool {
    match target {
        IroningTarget::TopmostOnly => topmost,
        IroningTarget::TopSurfaces | IroningTarget::AllSolid => true,
    }
}

/// Slices `mesh` and plans beads for every layer, with painted facets taken into
/// account.
///
/// The paint is converted to support footprints before the mesh goes through
/// preparation, so a repair that reorders triangles cannot invalidate the states
/// the caller supplied.
pub fn run_painted(
    mesh: &Mesh,
    paint: &Paint,
    first_layer_height: f64,
    layer_height: f64,
    settings: &Settings,
) -> Result<Job, EngineError> {
    run_with_paint(mesh, paint, &[], first_layer_height, layer_height, settings)
}

/// Slices `mesh` with its painted facets and its volumes, and plans the beads.
///
/// Volume geometry enters at the cross-section: a part volume is united with the
/// layer's outline and a negative one subtracted from it, before anything reads the
/// outline. That is the only point where a volume can act on walls, infill, skin and
/// brim at once, and it is why a hole cut by a negative volume gets walls of its own.
pub fn run_with_volumes(
    mesh: &Mesh,
    paint: &Paint,
    volumes: &[Volume<'_>],
    first_layer_height: f64,
    layer_height: f64,
    settings: &Settings,
) -> Result<Job, EngineError> {
    run_with_paint(
        mesh,
        paint,
        volumes,
        first_layer_height,
        layer_height,
        settings,
    )
}

/// Painted facets as the G-code writer consumes them.
///
/// Seam paint needs the facets themselves, not a footprint: the seam is a point
/// on a wall, so the writer measures the distance from a candidate vertex to the
/// face it was painted on.
pub fn seam_paint(mesh: &Mesh, paint: &Paint) -> Result<Option<SeamPaint>, EngineError> {
    paint.validate(mesh.triangle_count())?;
    if paint.seam.is_empty() {
        return Ok(None);
    }
    let mut seam = SeamPaint::default();
    for (index, state) in paint.seam.iter().enumerate() {
        if *state == PAINT_SEAM_NONE {
            continue;
        }
        let facet: [[f64; 3]; 3] = {
            let points = mesh.triangle_vertices(index);
            [points[0].to_mm(), points[1].to_mm(), points[2].to_mm()]
        };
        match *state {
            PAINT_SEAM_ENFORCER => seam.enforced.push(facet),
            PAINT_SEAM_BLOCKER => seam.blocked.push(facet),
            other => {
                return Err(EngineError::Paint(format!(
                    "triangle {index} has seam state {other}, which is not one of none, enforcer or blocker"
                )))
            }
        }
    }
    Ok(Some(seam))
}

/// Painted facets as the support generator consumes them.
///
/// A facet's footprint is its own projection onto the bed, and its span is the
/// height band between its lowest and highest vertex. An enforcer seeds the layer
/// under that band; a blocker forbids support inside it.
pub fn support_paint(mesh: &Mesh, paint: &Paint) -> Result<Option<PaintRegions>, EngineError> {
    paint.validate(mesh.triangle_count())?;
    if paint.support.is_empty() {
        return Ok(None);
    }
    let mut enforcers = Vec::new();
    let mut blockers = Vec::new();
    for (index, state) in paint.support.iter().enumerate() {
        if *state == PAINT_SUPPORT_NONE {
            continue;
        }
        let points = mesh.triangle_vertices(index);
        let ring: Vec<hfw_kernel::V2> = points
            .iter()
            .map(|point| {
                let [x, y, _] = point.to_mm();
                hfw_kernel::V2::new(x, y)
            })
            .collect();
        let footprint = Region::new(vec![ring]);
        if footprint.rings.is_empty() {
            continue;
        }
        let heights: Vec<f64> = points.iter().map(|point| point.to_mm()[2]).collect();
        let low = heights.iter().copied().fold(f64::INFINITY, f64::min);
        let high = heights.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = PaintSpan {
            footprint,
            bottom: low,
            top: high,
        };
        match *state {
            PAINT_SUPPORT_ENFORCER => enforcers.push(span),
            PAINT_SUPPORT_BLOCKER => blockers.push(span),
            other => {
                return Err(EngineError::Paint(format!(
                    "triangle {index} has support state {other}, which is not one of none, enforcer or blocker"
                )))
            }
        }
    }
    Ok(Some(PaintRegions {
        enforcers,
        blockers,
    }))
}

/// Slices `mesh` and plans beads for every layer.
pub fn run(
    mesh: &Mesh,
    first_layer_height: f64,
    layer_height: f64,
    settings: &Settings,
) -> Result<Job, EngineError> {
    run_with_paint(
        mesh,
        &Paint::default(),
        &[],
        first_layer_height,
        layer_height,
        settings,
    )
}

/// Volumes as the support generator consumes them.
///
/// A support volume acts over its whole height and inside its whole footprint, so it
/// is a painted span whose footprint is the volume's projection onto the bed: the
/// same shape a painted facet produces, which is why the two are merged rather than
/// kept apart downstream.
fn volume_support_paint(volumes: &[Volume<'_>]) -> Result<PaintRegions, EngineError> {
    let mut regions = PaintRegions::default();
    for volume in volumes {
        let states = match volume.kind {
            VolumeKind::SupportEnforcer => PAINT_SUPPORT_ENFORCER,
            VolumeKind::SupportBlocker => PAINT_SUPPORT_BLOCKER,
            VolumeKind::Part | VolumeKind::Negative => continue,
        };
        let paint = Paint {
            support: vec![states; volume.mesh.triangle_count()],
            seam: Vec::new(),
        };
        if let Some(painted) = support_paint(volume.mesh, &paint)? {
            regions.extend(painted);
        }
    }
    Ok(regions)
}

/// Applies part and negative volumes to every layer's outline.
///
/// The volumes are sliced with the object's own schedule, so a volume's cross-section
/// is taken at exactly the plane the object's is. A layer where a volume produces
/// nothing is left alone; a layer where the volume's mesh fails to close is skipped
/// with the rest of the job still slicing, because a volume that grazes a plane is
/// the same degenerate case a small feature is.
fn apply_volumes(
    regions: &mut [Region],
    schedule: &LayerSchedule,
    volumes: &[Volume<'_>],
    settings: &Settings,
) -> Result<(), EngineError> {
    for volume in volumes {
        if !matches!(volume.kind, VolumeKind::Part | VolumeKind::Negative) {
            continue;
        }
        let slices = hfw_slicing::slice_mesh_layers_with(
            volume.mesh,
            schedule,
            hfw_slicing::SliceOptions {
                close_gaps: settings.close_gaps,
            },
        );
        for (index, slice) in slices.iter().enumerate() {
            let Ok(slice) = slice else {
                continue;
            };
            let body = region_from_contours(&slice.contours);
            if body.rings.is_empty() || index >= regions.len() {
                continue;
            }
            let rings = match volume.kind {
                VolumeKind::Part => hfw_kernel::union_rings(&regions[index].rings, &body.rings),
                _ => hfw_kernel::difference_rings(&regions[index].rings, &body.rings),
            };
            regions[index] = Region::new(rings);
        }
    }
    Ok(())
}

fn run_with_paint(
    mesh: &Mesh,
    paint: &Paint,
    volumes: &[Volume<'_>],
    first_layer_height: f64,
    layer_height: f64,
    settings: &Settings,
) -> Result<Job, EngineError> {
    let mut support_regions = support_paint(mesh, paint)?.unwrap_or_default();
    support_regions.extend(volume_support_paint(volumes)?);
    let support_paint = if support_regions.is_empty() {
        None
    } else {
        Some(&support_regions)
    };
    let seam_paint = seam_paint(mesh, paint)?;
    if settings.spiral_vase && settings.support.is_some() {
        return Err(EngineError::VaseWithSupport);
    }
    let (prepared, repair) = mesh.repair_basic()?;
    let mesh = &prepared;
    let t_slice = std::time::Instant::now();
    let (schedule, slices) = slice_stack(mesh, first_layer_height, layer_height, settings)?;
    let ms_slice = t_slice.elapsed().as_secs_f64() * 1e3;

    let mut skipped = Vec::new();
    let mut regions: Vec<Region> = Vec::with_capacity(slices.len());
    for (index, slice) in slices.iter().enumerate() {
        match slice {
            Ok(layer) => regions.push(region_from_contours(&layer.contours)),
            Err(error) => {
                if settings.strict_slices {
                    return Err(EngineError::Slice(error.clone()));
                }
                skipped.push((index, error.to_string()));
                regions.push(Region::default());
            }
        }
    }

    let step = settings.every.max(1);

    apply_volumes(&mut regions, &schedule, volumes, settings)?;

    let t_offset = std::time::Instant::now();
    compensate_all(&mut regions, settings)?;
    let ms_offset = t_offset.elapsed().as_secs_f64() * 1e3;
    let mut cache =
        if settings.infill.is_some() || settings.support.is_some() || settings.measure_overhang {
            let needed = settings
                .infill
                .as_ref()
                .map(|ip| ip.top_layers + ip.bottom_layers + 1)
                .unwrap_or(1)
                .max(
                    settings
                        .support
                        .as_ref()
                        .map(|sp| sp.interface_layers + 2)
                        .unwrap_or(1),
                );
            let res = settings
                .infill
                .as_ref()
                .map(|ip| ip.skin_res)
                .unwrap_or(settings.wall.w / 4.0);
            Some(MaskCache::new(&regions, res, needed))
        } else {
            None
        };
    let support_ms;
    let supports = match (settings.support.as_ref(), cache.as_mut()) {
        (Some(sp), Some(cache)) => {
            let t = std::time::Instant::now();
            let heights: Vec<f64> = schedule
                .layers()
                .iter()
                .map(|s| (s.print_z - s.bottom_z) as f64 / 1e6)
                .collect();
            let masks = hfw_support::Supports::build_with_paint(
                &regions,
                cache,
                sp,
                &heights,
                support_paint,
            );
            support_ms = t.elapsed().as_secs_f64() * 1e3;
            Some(masks)
        }
        _ => {
            support_ms = 0.0;
            None
        }
    };

    let heights: Vec<f64> = schedule
        .layers()
        .iter()
        .map(|spec| (spec.print_z - spec.bottom_z) as f64 / 1e6)
        .collect();
    let combined = match (settings.infill.as_ref(), cache.as_ref()) {
        (Some(ip), Some(cache)) => combine_plan(cache, ip, &heights, settings.nozzle),
        _ => Vec::new(),
    };

    let mut layers = Vec::with_capacity(schedule.len());
    let mut stats = JobStats {
        ms_support: support_ms,
        ms_offset,
        layers: schedule.len(),
        triangles_tested: slices
            .iter()
            .filter_map(|s| s.as_ref().ok())
            .map(|s| s.diagnostics.triangles_tested)
            .sum(),
        ms_slice,
        ..JobStats::default()
    };
    let mut metrics = Vec::new();
    let (mut worst_ms, mut worst_layer) = (0.0f64, 0usize);
    let last = schedule.len().saturating_sub(1);

    let vase_base = if settings.spiral_vase {
        settings
            .infill
            .as_ref()
            .map(|ip| ip.bottom_layers)
            .unwrap_or(0)
            .min(last)
    } else {
        0
    };
    // Infill only reads the mask cache, which never changes once built, so the
    // layers are filled in one wave of their own instead of one after another.
    // On a benchy that is half the remaining serial time.
    let t_infill = std::time::Instant::now();
    let mut infill_for = {
        let fill = |k: usize| -> Option<Vec<Path>> {
            let ip = settings.infill.as_ref()?;
            let cache = cache.as_ref()?;
            if regions[k].is_empty() || (k % step != 0 && k != last) {
                return None;
            }
            if settings.spiral_vase && k >= vase_base {
                return None;
            }
            let layer_ip = if k == 0 && settings.first_layer_width > 0.0 {
                ip.scaled_to(settings.first_layer_width)
            } else {
                *ip
            };
            let print_z = (schedule.layers()[k].print_z) as f64 / 1e6;
            let mut lines: Vec<Path> = Vec::new();
            if settings.extra_perimeters_overhangs {
                lines.extend(hfw_infill::overhang_perimeter(
                    &regions[k],
                    cache,
                    k,
                    &settings.wall,
                    &layer_ip,
                ));
            }
            let combine = combined.get(k).and_then(Option::as_ref).map(|c| Combine {
                strip: c.strip.as_slice(),
                extra: c.extra.as_slice(),
                layers: c.layers,
            });
            lines.extend(generate_infill(
                &regions[k],
                cache,
                k,
                print_z,
                &settings.wall,
                &layer_ip,
                combine.as_ref(),
            ));
            if settings.ironing && ironing_covers(settings.ironing_target, k + 1 == regions.len()) {
                lines.extend(hfw_infill::ironing_layer(
                    &regions[k],
                    cache,
                    k,
                    &layer_ip,
                    settings.ironing_spacing,
                    settings.ironing_target,
                ));
            }
            Some(lines)
        };
        let threads = worker_count(settings, schedule.len());
        let mut slots: Vec<Option<Vec<Path>>> = (0..schedule.len()).map(|_| None).collect();
        if settings.infill.is_some() && cache.is_some() && threads > 1 {
            for (index, lines) in dynamic_map(threads, schedule.len(), &fill) {
                slots[index] = lines;
            }
        } else {
            for (index, slot) in slots.iter_mut().enumerate() {
                *slot = fill(index);
            }
        }
        slots
    };
    stats.ms_infill += t_infill.elapsed().as_secs_f64() * 1e3;

    let steps_out: Vec<bool> = if settings.extra_perimeters {
        regions
            .iter()
            .enumerate()
            .map(|(k, region)| {
                if k < 2 || region.is_empty() {
                    return false;
                }
                let Some(cache) = cache.as_mut() else {
                    return false;
                };
                let air = cache.air_below_bits(k);
                let own = cache.own_bits(k);
                let (mut over, mut all) = (0usize, 0usize);
                for (a, o) in air.iter().zip(&own) {
                    over += (a & o).count_ones() as usize;
                    all += o.count_ones() as usize;
                }
                all > 0 && over as f64 > 0.05 * all as f64
            })
            .collect()
    } else {
        Vec::new()
    };
    let one_wall: Vec<bool> =
        if settings.one_perimeter_first_layer || !settings.one_perimeter_top.is_empty() {
            (0..regions.len())
                .map(|k| {
                    if k == 0 && settings.one_perimeter_first_layer {
                        return true;
                    }
                    match settings.one_perimeter_top.as_str() {
                        "topmost" => k + 1 == regions.len(),
                        "top" => match cache.as_mut() {
                            Some(cache) => cache.top_bits(k).iter().any(|word| *word != 0),
                            None => false,
                        },
                        _ => false,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
    let wall_for = |k: usize| -> Result<Option<WallPass>, EngineError> {
        if regions[k].is_empty() || (k % step != 0 && k != last) {
            return Ok(None);
        }
        let mut lines = Vec::new();
        if k == 0 && settings.brim_loops > 0 {
            lines.extend(hfw_walls::brim(
                &regions[k],
                &settings.wall,
                settings.brim_loops,
                settings.brim_gap,
                settings.brim_separation,
                settings.brim_side,
            ));
        }
        let vase = settings.spiral_vase && k >= vase_base;
        let mut wall = if vase {
            WallParams {
                walls: 1,
                gap_fill: false,
                ..settings.wall
            }
        } else {
            settings.wall
        };
        if k == 0 && settings.first_layer_width > 0.0 {
            wall = wall.scaled_to(settings.first_layer_width);
        }
        if settings.resolution > 0.0 {
            wall.simplify = wall.simplify.max(settings.resolution);
        }
        if !vase && steps_out.get(k).copied().unwrap_or(false) {
            wall = wall.with_walls(wall.walls + 1);
        }
        if !vase && one_wall.get(k).copied().unwrap_or(false) && wall.walls > 1 {
            wall = wall.with_walls(1);
        }
        let out = hfw_walls::generate(&regions[k], &wall, settings.mode);
        if vase {
            lines.extend(out.lines.into_iter().filter(|line| line.kind.is_wall()));
        } else {
            lines.extend(out.lines);
        }
        Ok(Some(WallPass {
            lines,
            stats: out.stats,
        }))
    };

    let threads = worker_count(settings, schedule.len());
    let t_walls = std::time::Instant::now();
    let collected = dynamic_map(threads, schedule.len(), &wall_for);
    let mut passes: Vec<Option<Result<Option<WallPass>, EngineError>>> =
        (0..schedule.len()).map(|_| None).collect();
    for (index, result) in collected {
        passes[index] = Some(result);
    }
    stats.ms_walls_elapsed = t_walls.elapsed().as_secs_f64() * 1e3;
    stats.threads = threads;

    for (k, (spec, _slice)) in schedule.layers().iter().zip(slices.iter()).enumerate() {
        let mut planned = PlannedLayer {
            index: k,
            bottom_z: spec.bottom_z as f64 / 1e6,
            slice_z: spec.slice_z as f64 / 1e6,
            print_z: spec.print_z as f64 / 1e6,
            reference: if settings.keep_reference {
                regions[k].rings.clone()
            } else {
                Vec::new()
            },
            paths: Vec::new(),
            counts: Default::default(),
            vase: false,
            first: k == 0,
        };

        let vase_layer = settings.spiral_vase && k >= vase_base;

        let pass = match passes[k].take() {
            Some(result) => result?,
            None => None,
        };
        if let Some(pass) = pass {
            if vase_layer {
                planned.vase = true;
                stats.ms_walls += pass.stats.ms_total;
                stats.processed_layers += 1;
                planned.paths.extend(pass.lines);
                planned.recount();
                layers.push(planned);
                continue;
            }
            if let (Some(masks), Some(sp)) = (supports.as_ref(), settings.support.as_ref()) {
                planned.paths.extend(masks.fill(k, sp));
            }
            planned.paths.extend(pass.lines);
            let layer_ms = pass.stats.ms_total;
            stats.processed_layers += 1;
            stats.wall_loops += pass.stats.loops;
            stats.gap_strands += pass.stats.gaps;
            stats.wall_points += pass.stats.points;
            stats.ms_walls += layer_ms;
            if layer_ms > worst_ms {
                worst_ms = layer_ms;
                worst_layer = k;
            }

            if let Some(mut lines) = infill_for[k].take() {
                planned.paths.append(&mut lines);
            }
            planned.recount();
            if settings.measure_overhang {
                if let Some(cache) = cache.as_ref() {
                    annotate_overhang(&mut planned.paths, cache, k);
                }
            }
            if let Some(res) = settings.metrics_res {
                let t2 = std::time::Instant::now();
                let shell: Vec<hfw_walls::Line> = planned
                    .paths
                    .iter()
                    .filter(|l| l.kind.is_wall())
                    .cloned()
                    .collect();
                let mut m = evaluate(&regions[k], &shell, &settings.wall, res);
                let full = evaluate_full(&regions[k], &planned.paths, &settings.wall, res);
                m.area_region = full.area_region;
                metrics.push(m);
                stats.ms_metrics += t2.elapsed().as_secs_f64() * 1e3;
            }
        }

        layers.push(planned);
    }

    let mut volume = (0.0, 0.0);
    for layer in &layers {
        let (w, i) = extruded_volume(&layer.paths, layer.height());
        volume.0 += w;
        volume.1 += i;
    }

    Ok(Job {
        mesh_diagnostics: mesh.diagnostics(),
        repair,
        layers,
        metrics,
        volume,
        stats,
        worst_layer_ms: worst_ms,
        worst_layer,
        skipped,
        markers: Vec::new(),
        seam_paint: match seam_paint {
            Some(paint) => vec![paint],
            None => Vec::new(),
        },
    })
}

/// One layer's share of a combined fill, as the infill crate wants it.
struct LayerCombine {
    strip: Vec<u64>,
    extra: Vec<u64>,
    layers: usize,
}

/// Groups layers for `infill_every_layers` and intersects their free areas.
///
/// The rule is upstream's: a group closes when one more layer would push the
/// combined height past the nozzle diameter, or when it already holds the number
/// of layers the profile asked for. The first layer is never combined, because it
/// is the one that has to stick to the bed.
///
/// Only the intersection moves, and it moves into the group's top layer, which is
/// printed at that layer's own Z with the flow of the whole group behind it. A
/// cell that is sparse in one layer and solid in another stays where it is, so a
/// top surface still gets its own skin on every layer it needs.
fn combine_plan(
    cache: &MaskCache,
    ip: &InfillParams,
    heights: &[f64],
    nozzle: f64,
) -> Vec<Option<LayerCombine>> {
    let layers = heights.len();
    let mut plan: Vec<Option<LayerCombine>> = (0..layers).map(|_| None).collect();
    let automatic = ip.combine_auto;
    let wanted = if automatic {
        usize::MAX
    } else {
        ip.combine_layers
    };
    if wanted < 2 || (ip.density <= 0.0 && !automatic) {
        return plan;
    }
    let ceiling = if ip.combine_max_height > 0.0 {
        ip.combine_max_height
    } else {
        nozzle
    };
    let mut free: Vec<Vec<u64>> = Vec::with_capacity(layers);
    for k in 0..layers {
        let (win_top, win_bottom) = ip.window();
        free.push(cache.sparse_own_bits(k, win_top, win_bottom));
    }
    let mut first = 1usize;
    let mut height = 0.0;
    let mut count = 0usize;
    for (k, next) in heights.iter().enumerate().skip(1) {
        let next = *next;
        if count > 0 && (height + next > ceiling + 1e-9 || count >= wanted) {
            close_group(&mut plan, &free, first, k - 1, count);
            height = 0.0;
            count = 0;
            first = k;
        }
        height += next;
        count += 1;
    }
    if count > 0 {
        close_group(&mut plan, &free, first, layers - 1, count);
    }
    plan
}

/// Moves the intersection of a group's free areas into its top layer.
fn close_group(
    plan: &mut [Option<LayerCombine>],
    free: &[Vec<u64>],
    first: usize,
    last: usize,
    count: usize,
) {
    if count < 2 || last >= plan.len() || first >= last {
        return;
    }
    let mut shared = free[last].clone();
    for layer in &free[first..last] {
        for (a, b) in shared.iter_mut().zip(layer) {
            *a &= *b;
        }
    }
    if shared.iter().all(|word| *word == 0) {
        return;
    }
    for slot in plan.iter_mut().take(last).skip(first) {
        *slot = Some(LayerCombine {
            strip: shared.clone(),
            extra: Vec::new(),
            layers: count,
        });
    }
    plan[last] = Some(LayerCombine {
        strip: shared.clone(),
        extra: shared,
        layers: count,
    });
}

/// Fills in the overhanging share of every path on layer `k`.
///
/// A point counts as overhanging when the layer below does not cover it. That is
/// the same question the bridge classifier asks, minus the bridge: a bridge spans
/// a named gap and is already its own feature, while an overhang is the wall or
/// skin that leans out over air.
///
/// Sampling is bounded: at most eight points per path, so a 200 mm loop is
/// measured by a sample rather than by every vertex. The number is a share of the
/// path's own length, which is what the speed ladder and the fan ladder want.
fn annotate_overhang(paths: &mut [hfw_walls::Line], cache: &MaskCache, k: usize) {
    if k == 0 {
        for path in paths.iter_mut() {
            path.overhang = 0.0;
        }
        return;
    }
    let below = cache.below_bits(k);
    let grid = *cache.grid();
    for path in paths.iter_mut() {
        if path.kind == hfw_walls::Kind::Bridge || path.kind == hfw_walls::Kind::BridgeAnchor {
            continue;
        }
        let points = sample_points(path);
        if points.is_empty() {
            continue;
        }
        let air = points.iter().filter(|p| !grid.is_set(&below, **p)).count();
        path.overhang = air as f64 / points.len() as f64;
    }
}

/// Evenly spaced points along a path, the last one included for an open path.
fn sample_points(path: &hfw_walls::Line) -> Vec<hfw_kernel::V2> {
    const SAMPLES: usize = 8;
    let n = path.pts.len();
    if n < 2 {
        return Vec::new();
    }
    if path.closed {
        return path
            .pts
            .iter()
            .step_by((n / SAMPLES).max(1))
            .copied()
            .collect();
    }
    let total = path.length();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(SAMPLES);
    let mut walked = 0.0;
    let mut target = 0.0;
    let stride = total / SAMPLES as f64;
    for pair in path.pts.windows(2) {
        let seg = pair[0].dist(pair[1]);
        while target <= walked + seg + 1e-9 && out.len() < SAMPLES {
            let at = if seg > 0.0 {
                (target - walked) / seg
            } else {
                0.0
            };
            out.push(hfw_kernel::V2::new(
                pair[0].x + (pair[1].x - pair[0].x) * at,
                pair[0].y + (pair[1].y - pair[0].y) * at,
            ));
            target += stride;
        }
        walked += seg;
    }
    out
}

fn slice_stack(
    mesh: &Mesh,
    first_layer_height: f64,
    layer_height: f64,
    settings: &Settings,
) -> Result<(LayerSchedule, Vec<Result<SliceLayer, SliceError>>), SliceError> {
    let (z_min, z_max) = mesh
        .z_bounds()
        .ok_or(SliceError::InvalidZRange { z_min: 0, z_max: 0 })?;
    let schedule = LayerSchedule::from_profile(
        z_min,
        z_max,
        hfw_geometry::mm_to_coord(first_layer_height)?,
        hfw_geometry::mm_to_coord(layer_height)?,
        &settings.layer_profile,
        settings.min_layer_height,
        settings.max_layer_height,
    )?;
    let slices = hfw_slicing::slice_mesh_layers_with(
        mesh,
        &schedule,
        hfw_slicing::SliceOptions {
            close_gaps: settings.close_gaps,
        },
    );
    Ok((schedule, slices))
}

/// Prepares the sliced cross-sections before anything reads them.
///
/// Three settings act here, in the order upstream applies them, and all three
/// are properties of the layer's outline rather than of the paths drawn inside
/// it, so one prepared outline drives walls, infill, skin, brim and support.
///
/// The closing radius fills hairline cracks first: a gap the mesh carried would
/// otherwise be printed as a crack, and growing the part afterwards would not
/// remove it. Elephant-foot and XY size compensation are the same operation —
/// offset the cross-section by a signed distance — and the grid resolution
/// follows that distance, because sampling a small correction at the wall
/// field's resolution would place the edge wherever the sampling happened to
/// land. Simplification comes last, so a crack is closed at full detail before
/// any vertex is dropped.
fn compensate(regions: &mut [Region], settings: &Settings) -> Result<(), EngineError> {
    for (index, region) in regions.iter_mut().enumerate() {
        compensate_one(region, index, settings)?;
    }
    Ok(())
}

/// The same work spread over the layers.
///
/// A layer's outline is compensated from its own geometry and the settings, so
/// which thread prepares it cannot change the result: every layer produces the
/// one it would have produced alone. The first layer is special only in taking
/// the elephant-foot correction, which the region's own index carries.
fn compensate_all(regions: &mut [Region], settings: &Settings) -> Result<(), EngineError> {
    let threads = worker_count(settings, regions.len());
    if threads <= 1 || regions.len() <= 1 {
        return compensate(regions, settings);
    }
    let chunks = (regions.len() / (threads * 8)).max(1);
    let queue: std::sync::Mutex<Vec<(usize, &mut [Region])>> = std::sync::Mutex::new(
        regions
            .chunks_mut(chunks)
            .enumerate()
            .map(|(index, part)| (index * chunks, part))
            .collect(),
    );
    let mut errors: Vec<(usize, EngineError)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let queue = &queue;
                scope.spawn(move || {
                    let mut local: Vec<(usize, EngineError)> = Vec::new();
                    loop {
                        let next = queue.lock().map(|mut queue| queue.pop()).ok().flatten();
                        let Some((base, part)) = next else {
                            break;
                        };
                        for (offset, region) in part.iter_mut().enumerate() {
                            if let Err(error) = compensate_one(region, base + offset, settings) {
                                local.push((base + offset, error));
                            }
                        }
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a planning worker panicked"))
            .collect()
    });
    errors.sort_by_key(|(index, _)| *index);
    match errors.into_iter().next() {
        Some((_, error)) => Err(error),
        None => Ok(()),
    }
}

/// Runs `work` over `0..count` with one shared queue.
///
/// Layers differ in cost by more than an order of magnitude — a small top layer
/// against a first layer with brim and elephant-foot compensation — so handing
/// each worker a fixed slice leaves it waiting on the slowest one. The queue
/// hands the next index to whichever worker is free, and the results come back
/// keyed by index so the output keeps layer order.
fn dynamic_map<T: Send>(
    threads: usize,
    count: usize,
    work: &(dyn Fn(usize) -> T + Sync),
) -> Vec<(usize, T)> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let parts: Vec<Vec<(usize, T)>> = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads.min(count));
        for _ in 0..threads.min(count) {
            let next = &next;
            handles.push(scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if index >= count {
                        break;
                    }
                    local.push((index, work(index)));
                }
                local
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a planning worker panicked"))
            .collect()
    });
    parts.into_iter().flatten().collect()
}

fn compensate_one(
    region: &mut Region,
    index: usize,
    settings: &Settings,
) -> Result<(), EngineError> {
    let ceiling = settings.wall.w / 4.0;
    if region.is_empty() {
        return Ok(());
    }
    if settings.closing_radius > 0.0 {
        *region = hfw_walls::close_gaps(region, settings.closing_radius);
    }
    let grow = settings.xy_size
        - if index == 0 {
            settings.elephant_foot
        } else {
            0.0
        };
    if grow != 0.0 && !region.is_empty() {
        let h = (grow.abs() / 3.0).clamp(0.02, ceiling);
        *region = hfw_walls::offset_region(region, grow, h);
    }
    if settings.resolution > 0.0 && !region.is_empty() {
        *region = hfw_walls::simplify_region(region, settings.resolution);
    }
    Ok(())
}

/// How many workers a per-layer phase should run.
fn worker_count(settings: &Settings, units: usize) -> usize {
    let threads = if settings.threads == 0 {
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
    } else {
        settings.threads
    };
    threads.min(units.max(1))
}

/// Slices one plane and returns its millimetre region.
pub fn slice_plane(mesh: &Mesh, z_mm: f64) -> Result<(SliceLayer, Region), EngineError> {
    let layer = slice_mesh(mesh, hfw_slicing::SlicePlane::from_mm(z_mm)?)?;
    let region = region_from_contours(&layer.contours);
    Ok((layer, region))
}

/// Convenience helper: total path length of a job.
pub fn total_length(job: &Job) -> f64 {
    job.layers.iter().map(|l| l.counts.length).sum()
}

/// Convenience helper: total extruder start/stop count of a job.
pub fn total_starts_stops(job: &Job) -> usize {
    job.layers.iter().map(|l| l.counts.starts_stops).sum()
}

/// True when the settings request solid infill everywhere.
pub fn is_solid_infill_infinite_density(settings: &Settings) -> bool {
    settings
        .infill
        .as_ref()
        .is_some_and(|ip| ip.density >= 0.999 && ip.pattern == InfillPattern::Rectilinear)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hfw_geometry::Point2;

    fn cube() -> Mesh {
        Mesh::box_mm(20.0, 20.0, 20.0).expect("cube")
    }

    fn sphere(radius: f64) -> Mesh {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sphere.stl");
        let _ = radius;
        Mesh::load_stl(&path).expect("fixture loads")
    }

    #[test]
    fn an_overhanging_sphere_reports_which_paths_hang_over_air() {
        let mesh = sphere(10.0);
        let mut settings = Settings::new(0.4, 2);
        settings.measure_overhang = true;
        let job = run(&mesh, 0.2, 0.2, &settings).expect("job runs");
        let measured = job
            .layers
            .iter()
            .flat_map(|l| l.paths.iter())
            .filter(|p| p.overhang > 0.0)
            .count();
        let supported = job
            .layers
            .iter()
            .flat_map(|l| l.paths.iter())
            .filter(|p| p.overhang == 0.0)
            .count();
        assert!(measured > 0, "the lower half of a sphere overhangs");
        assert!(supported > 0, "the upper half does not");
        let over = job
            .layers
            .iter()
            .flat_map(|l| l.paths.iter())
            .map(|p| p.overhang)
            .fold(0.0f64, f64::max);
        assert!(over > 0.5, "no path was mostly over air: worst {over}");
    }

    #[test]
    fn overhang_measurement_is_off_unless_asked() {
        let mesh = sphere(10.0);
        let job = run(&mesh, 0.2, 0.2, &Settings::new(0.4, 2)).expect("job runs");
        let any = job
            .layers
            .iter()
            .flat_map(|l| l.paths.iter())
            .any(|p| p.overhang > 0.0);
        assert!(!any, "overhang was measured without being asked");
    }

    fn square_contour(side: f64) -> Contour {
        Contour::new(vec![
            Point2::from_mm(0.0, 0.0).unwrap(),
            Point2::from_mm(side, 0.0).unwrap(),
            Point2::from_mm(side, side).unwrap(),
            Point2::from_mm(0.0, side).unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn the_bridge_preserves_exact_areas() {
        let region = region_from_contours(&[square_contour(20.0)]);
        assert_eq!(region.rings.len(), 1);
        assert!((region.area() - 400.0).abs() < 1e-9);
    }

    #[test]
    fn the_bridge_subtracts_a_clockwise_contour_as_a_hole() {
        let outer = square_contour(20.0);
        let mut hole = Contour::new(vec![
            Point2::from_mm(5.0, 5.0).unwrap(),
            Point2::from_mm(5.0, 10.0).unwrap(),
            Point2::from_mm(10.0, 10.0).unwrap(),
            Point2::from_mm(10.0, 5.0).unwrap(),
        ])
        .unwrap();
        hole.normalize_winding(false);
        let region = region_from_contours(&[outer, hole]);
        assert_eq!(region.rings.len(), 2);
        assert!((region.area() - (400.0 - 25.0)).abs() < 1e-6);
        assert_eq!(
            region
                .rings
                .iter()
                .filter(|r| hfw_kernel::ring_signed_area(r) > 0.0)
                .count(),
            1
        );
    }

    #[test]
    fn a_counter_clockwise_shell_inside_another_stays_material() {
        let outer = square_contour(20.0);
        let mut inner = square_contour(5.0);
        inner.normalize_winding(true);
        let region = region_from_contours(&[outer, inner]);
        assert_eq!(region.rings.len(), 1);
        assert!((region.area() - 400.0).abs() < 1e-9);
    }

    #[test]
    fn two_overlapping_shells_are_united() {
        let a = square_contour(20.0);
        let mut b = square_contour(20.0);
        b.normalize_winding(true);
        let shifted: Vec<Point2> = b
            .points()
            .iter()
            .map(|p| {
                let mm = p.to_mm();
                Point2::from_mm(mm[0] + 8.0, mm[1] + 6.0).expect("shifted point")
            })
            .collect();
        let b = Contour::new(shifted).expect("shifted square");
        let region = region_from_contours(&[a, b]);
        assert_eq!(region.rings.len(), 1, "the shells merge into one outline");
        assert!((region.area() - 632.0).abs() < 1e-6);
    }

    fn cracked_plates() -> Mesh {
        let left = Mesh::box_mm(10.0, 10.0, 10.0).expect("left plate");
        let right = Mesh::box_mm(10.0, 10.0, 10.0)
            .expect("right plate")
            .transformed(hfw_mesh::MeshTransform {
                translation_mm: [10.05, 0.0, 0.0],
                ..Default::default()
            })
            .expect("shifted right plate");
        Mesh::merged(&[left, right]).expect("a cracked pair")
    }

    #[test]
    fn the_closing_radius_welds_a_hairline_crack() {
        let mesh = cracked_plates();
        let (_, region) = slice_plane(&mesh, 5.0).expect("slice");
        assert_eq!(region.rings.len(), 2, "the fixture starts as two plates");
        let crack = hfw_kernel::V2::new(10.025, 5.0);
        assert!(
            !region.contains(crack),
            "the 0.05 mm crack is air to begin with"
        );
        let mut kept = vec![region.clone()];
        let mut settings = Settings::new(0.4, 2);
        settings.closing_radius = 0.0;
        compensate(&mut kept, &settings).expect("repair");
        assert!(
            !kept[0].contains(crack),
            "a zero radius must leave the crack alone"
        );
        let mut closed = vec![region];
        settings.closing_radius = 0.049;
        compensate(&mut closed, &settings).expect("repair");
        assert!(
            closed[0].contains(crack),
            "twice the default 0.049 mm is wider than the crack, so it must weld"
        );
    }

    #[test]
    fn the_resolution_drops_slice_detail() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/sphere.stl");
        let mesh = Mesh::load_stl(path).expect("the sphere fixture");
        let (_, region) = slice_plane(&mesh, 10.0).expect("slice the equator");
        let mut kept = vec![region.clone()];
        let mut settings = Settings::new(0.4, 2);
        settings.closing_radius = 0.0;
        settings.resolution = 0.0;
        compensate(&mut kept, &settings).expect("no repair");
        assert_eq!(kept[0].rings[0].len(), region.rings[0].len());
        let mut coarse = vec![region.clone()];
        settings.resolution = 0.3;
        compensate(&mut coarse, &settings).expect("repair");
        assert!(
            coarse[0].rings[0].len() < region.rings[0].len(),
            "a 0.3 mm tolerance must drop equator vertices, kept {} of {}",
            coarse[0].rings[0].len(),
            region.rings[0].len()
        );
        assert!(
            (coarse[0].area() - region.area()).abs() < 0.03 * region.area(),
            "simplification must not change the cross-section's area by more than the tolerance allows: {} vs {}",
            coarse[0].area(),
            region.area()
        );
    }

    #[test]
    fn a_cube_produces_walls_and_infill_on_every_layer() {
        let mut settings = Settings::new(0.4, 3);
        settings.infill = Some(InfillParams::new(0.4));
        let job = run(&cube(), 0.2, 0.2, &settings).expect("job");
        assert_eq!(job.stats.layers, 100);
        assert_eq!(job.layers.len(), 100);
        for layer in &job.layers {
            assert!(
                !layer.paths.is_empty(),
                "layer {} has no paths",
                layer.index
            );
            assert!(
                layer.counts.walls >= 3,
                "layer {} has {} walls",
                layer.index,
                layer.counts.walls
            );
        }
        assert!(job.volume.0 > 0.0);
    }

    #[test]
    fn solid_infill_conserves_the_cube_volume() {
        let mut settings = Settings::new(0.4, 3);
        let mut ip = InfillParams::new(0.4);
        ip.density = 1.0;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        settings.infill = Some(ip);
        let job = run(&cube(), 0.2, 0.2, &settings).expect("job");
        let volume = job.volume.0 + job.volume.1;
        // All of the cube is deposited except the corners of its wall beads: a wall
        // bead rounds off into the two corners it does not fill, and that correction
        // is what the reference slicers apply too. Everything inside is solid fill,
        // which keeps the full rectangle.
        let ratio = volume / 8000.0;
        assert!(
            (0.96..=1.02).contains(&ratio),
            "{ratio:.4} of the cube was deposited"
        );
    }
}
