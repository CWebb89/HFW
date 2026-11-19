//! A machine-readable slice report.
//!
//! The human report is prose: it reads well and parses badly. This module emits
//! the same job as one JSON document on stdout, so a caller — `next-layer`, a
//! test, a script — can read layer counts and totals without scraping text.
//!
//! There is no JSON dependency here on purpose: a writer for one document shape
//! is a hundred lines, and every dependency this crate takes has to be justified
//! against that. Numbers that are not finite become `null`, because JSON has no
//! `NaN` and emitting one would make the whole document unparseable.

use hfw_config::PrinterConfig;
use hfw_engine::{
    InfillParams, InfillPattern, Job, JobTotals, Mode, Path, PathKind as Kind,
    PlannedSlice as PlannedLayer, Settings,
};
use hfw_gcode::{GcodeStats, VerifyReport};

/// One JSON value.
pub enum Node {
    /// `null`. Also what a non-finite number becomes.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A fractional number, printed in shortest round-trip form.
    Num(f64),
    /// A whole number, printed without a decimal point.
    Int(i64),
    /// A string, escaped.
    Str(String),
    /// An array.
    Arr(Vec<Node>),
    /// An object with fixed keys, in the order given.
    Obj(Vec<(&'static str, Node)>),
}

impl Node {
    /// Renders the document with two-space indentation and a trailing newline.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.render(&mut out, 0);
        out.push('\n');
        out
    }

    fn render(&self, out: &mut String, depth: usize) {
        match self {
            Node::Null => out.push_str("null"),
            Node::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Node::Num(value) => {
                if value.is_finite() {
                    out.push_str(&format!("{value}"))
                } else {
                    out.push_str("null")
                }
            }
            Node::Int(value) => out.push_str(&value.to_string()),
            Node::Str(value) => push_escaped(out, value),
            Node::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    item.render(out, depth + 1);
                    if index + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                indent(out, depth);
                out.push(']');
            }
            Node::Obj(items) => {
                if items.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (index, (key, value)) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    push_escaped(out, key);
                    out.push_str(": ");
                    value.render(out, depth + 1);
                    if index + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn push_escaped(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A finite float.
fn num(value: f64) -> Node {
    Node::Num(value)
}

/// A count.
fn int(value: usize) -> Node {
    Node::Int(value as i64)
}

/// A string.
fn text(value: impl Into<String>) -> Node {
    Node::Str(value.into())
}

/// The stable machine name of a path kind, as opposed to its display label.
fn feature_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Side(0) => "outer_wall",
        Kind::Side(_) => "inner_wall",
        Kind::Middle => "gap_fill",
        Kind::Infill => "sparse_infill",
        Kind::Solid => "solid_infill",
        Kind::TopSolid => "top_surface",
        Kind::Brim => "brim",
        Kind::Bridge => "bridge",
        Kind::BridgeAnchor => "bridge_anchor",
        Kind::Ironing => "ironing",
        Kind::Support => "support",
        Kind::SupportInterface => "support_interface",
    }
}

fn pattern_key(pattern: InfillPattern) -> &'static str {
    match pattern {
        InfillPattern::Rectilinear => "rectilinear",
        InfillPattern::Grid => "grid",
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
        InfillPattern::MonotonicLines => "monotonicline",
    }
}

fn mode_key(mode: Mode) -> &'static str {
    match mode {
        Mode::Hfw => "hfw",
        Mode::Classic => "classic",
        Mode::Field => "field",
    }
}

fn infill_node(params: &InfillParams) -> Node {
    Node::Obj(vec![
        ("density", num(params.density)),
        ("pattern", text(pattern_key(params.pattern))),
        ("top_layers", int(params.top_layers)),
        ("bottom_layers", int(params.bottom_layers)),
        ("width", num(params.width)),
        ("solid_width", num(params.solid_width)),
        ("skin_resolution", num(params.skin_res)),
    ])
}

fn settings_node(settings: &Settings, cfg: Option<&PrinterConfig>) -> Node {
    let wall = &settings.wall;
    let mut fields = vec![
        ("walls", int(wall.walls)),
        ("wall_width", num(wall.w)),
        ("outer_wall_width", num(wall.w_outer)),
        ("min_bead_width", num(wall.w_min)),
        ("max_bead_width", num(wall.w_max)),
        ("layer_height", num(wall.h)),
        ("min_feature_size", num(wall.min_feature)),
        ("thin_walls", Node::Bool(wall.thin_walls)),
        ("mode", text(mode_key(settings.mode))),
        ("every", int(settings.every)),
        (
            "infill",
            settings.infill.as_ref().map_or(Node::Null, infill_node),
        ),
        (
            "support",
            settings.support.as_ref().map_or(Node::Null, |sp| {
                Node::Obj(vec![
                    (
                        "style",
                        text(match sp.style {
                            hfw_engine::SupportStyle::Normal => "normal",
                            hfw_engine::SupportStyle::Tree => "tree",
                        }),
                    ),
                    ("interface_layers", int(sp.interface_layers)),
                ])
            }),
        ),
        ("ironing", Node::Bool(settings.ironing)),
        ("brim_loops", int(settings.brim_loops)),
        ("brim_gap", num(settings.brim_gap)),
        ("elephant_foot", num(settings.elephant_foot)),
        ("xy_size", num(settings.xy_size)),
        ("closing_radius", num(settings.closing_radius)),
        ("resolution", num(settings.resolution)),
        ("spiral_vase", Node::Bool(settings.spiral_vase)),
        ("threads", int(settings.threads)),
    ];
    if let Some(cfg) = cfg {
        fields.push((
            "printer",
            Node::Obj(vec![
                ("bed_x", num(cfg.bed_x)),
                ("bed_y", num(cfg.bed_y)),
                ("bed_shape", text(cfg.bed_shape_text())),
                ("max_z", num(cfg.max_z)),
                ("nozzle_diameter", num(cfg.nozzle_diameter)),
                ("filament_diameter", num(cfg.filament_diameter)),
                ("filament_density", num(cfg.filament_density)),
                ("nozzle_temp", num(cfg.nozzle_temp)),
                ("first_nozzle_temp", num(cfg.first_nozzle_temp)),
                ("bed_temp", num(cfg.bed_temp)),
                ("flow", num(cfg.flow)),
                ("arc_fitting", Node::Bool(cfg.arc_fitting)),
                ("gcode_resolution", num(cfg.gcode_resolution)),
            ]),
        ));
    }
    Node::Obj(fields)
}

fn path_node(path: &Path) -> Node {
    let width = path.mean_width();
    Node::Obj(vec![
        ("feature", text(feature_key(path.kind))),
        ("width", num(width)),
        ("closed", Node::Bool(path.closed)),
        ("overhang", num(path.overhang)),
        ("object", int(path.object as usize)),
        ("length", num(path.length())),
        (
            "points",
            Node::Arr(
                path.pts
                    .iter()
                    .map(|p| Node::Arr(vec![num(p.x), num(p.y)]))
                    .collect(),
            ),
        ),
    ])
}

fn layer_node(layer: &PlannedLayer, with_paths: bool) -> Node {
    let c = layer.counts;
    let mut fields = vec![
        ("index", int(layer.index)),
        ("bottom_z", num(layer.bottom_z)),
        ("slice_z", num(layer.slice_z)),
        ("print_z", num(layer.print_z)),
        ("height", num(layer.height())),
        ("vase", Node::Bool(layer.vase)),
        ("first", Node::Bool(layer.first)),
        (
            "counts",
            Node::Obj(vec![
                ("walls", int(c.walls)),
                ("middle", int(c.middle)),
                ("skin", int(c.skin)),
                ("top_skin", int(c.top_skin)),
                ("infill", int(c.infill)),
                ("brim", int(c.brim)),
                ("bridge", int(c.bridge)),
                ("support", int(c.support)),
                ("support_interface", int(c.support_interface)),
                ("ironing", int(c.ironing)),
            ]),
        ),
        ("length", num(c.length)),
        ("volume", num(c.volume)),
        ("starts_stops", int(c.starts_stops)),
        ("paths", int(layer.paths.len())),
        ("reference_contours", int(layer.reference.len())),
    ];
    if with_paths {
        fields.push((
            "geometry",
            Node::Arr(layer.paths.iter().map(path_node).collect()),
        ));
    }
    Node::Obj(fields)
}

/// Everything the document needs that does not come from the job itself.
pub struct Context<'a> {
    /// Input files, in the order they were given.
    pub inputs: Vec<String>,
    /// Input units as the caller named them.
    pub units: String,
    /// Uniform scale applied at import.
    pub scale: f64,
    /// Vertices in the prepared mesh.
    pub vertices: usize,
    /// Triangles in the prepared mesh.
    pub triangles: usize,
    /// Mesh volume in cubic millimetres.
    pub mesh_volume: f64,
    /// Reasons the file is unsafe or wrong, from the verifier.
    pub verify: Option<&'a VerifyReport>,
    /// Totals of the written file, when one was written.
    pub gcode: Option<(&'a GcodeStats, Option<&'a std::path::Path>)>,
}

/// The document for a planned job.
pub fn document(
    context: &Context,
    settings: &Settings,
    cfg: Option<&PrinterConfig>,
    job: &Job,
    totals: &JobTotals,
    with_paths: bool,
) -> Node {
    let stats = &job.stats;
    let mut job_fields = vec![
        ("layers", int(totals.layers)),
        ("layers_with_paths", int(totals.layers_with_paths)),
        ("processed_layers", int(stats.processed_layers)),
        ("path_length", num(totals.length)),
        ("path_volume", num(totals.volume)),
        ("perimeter_volume", num(job.volume.0)),
        ("interior_volume", num(job.volume.1)),
        ("starts_stops", int(totals.starts_stops)),
        (
            "skipped_layers",
            Node::Arr(
                job.skipped
                    .iter()
                    .map(|(index, reason)| {
                        Node::Obj(vec![
                            ("index", int(*index)),
                            ("reason", text(reason.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ];
    job_fields.push((
        "timing",
        Node::Obj(vec![
            ("slice_ms", num(stats.ms_slice)),
            ("offset_ms", num(stats.ms_offset)),
            ("support_ms", num(stats.ms_support)),
            ("walls_ms", num(stats.ms_walls)),
            ("walls_elapsed_ms", num(stats.ms_walls_elapsed)),
            ("infill_ms", num(stats.ms_infill)),
            ("metrics_ms", num(stats.ms_metrics)),
            ("threads", int(stats.threads)),
            ("slowest_layer_ms", num(job.worst_layer_ms)),
            ("slowest_layer", int(job.worst_layer)),
        ]),
    ));

    let gcode = match context.gcode {
        Some((stats, path)) => Node::Obj(vec![
            (
                "path",
                path.map_or(Node::Null, |p| text(p.display().to_string())),
            ),
            ("layers", int(stats.layers)),
            ("filament_mm", num(stats.filament_mm)),
            ("filament_g", num(stats.filament_g)),
            ("volume", num(stats.volume_mm3)),
            ("estimated_seconds", num(stats.est_seconds)),
            ("travel", num(stats.travel_mm)),
            ("travel_crossings", int(stats.travel_crossings)),
            ("retractions", int(stats.retractions)),
            ("top_z", num(stats.top_z)),
            (
                "bbox",
                Node::Obj(vec![
                    ("x_min", num(stats.bbox_min.x)),
                    ("x_max", num(stats.bbox_max.x)),
                    ("y_min", num(stats.bbox_min.y)),
                    ("y_max", num(stats.bbox_max.y)),
                ]),
            ),
        ]),
        None => Node::Null,
    };

    let verify = match context.verify {
        Some(report) => Node::Obj(vec![
            ("ok", Node::Bool(report.errors.is_empty())),
            (
                "errors",
                Node::Arr(report.errors.iter().map(|e| text(e.clone())).collect()),
            ),
            (
                "warnings",
                Node::Arr(report.warnings.iter().map(|w| text(w.clone())).collect()),
            ),
            ("layers", int(report.layers)),
            ("moves", int(report.moves)),
            ("volume", num(report.volume_mm3)),
            ("filament_mm", num(report.filament_mm)),
            ("estimated_seconds", num(report.est_seconds)),
            ("travel_crossings", int(report.travel_crossings)),
            ("first_extrusion_z", num(report.first_extrusion_z)),
        ]),
        None => Node::Null,
    };

    Node::Obj(vec![
        ("schema", int(1)),
        ("tool", text("hfw")),
        (
            "inputs",
            Node::Arr(context.inputs.iter().cloned().map(text).collect()),
        ),
        ("units", text(context.units.clone())),
        ("scale", num(context.scale)),
        (
            "mesh",
            Node::Obj(vec![
                ("vertices", int(context.vertices)),
                ("triangles", int(context.triangles)),
                ("volume", num(context.mesh_volume)),
            ]),
        ),
        ("settings", settings_node(settings, cfg)),
        ("job", Node::Obj(job_fields)),
        ("gcode", gcode),
        ("verify", verify),
        (
            "layers",
            Node::Arr(
                job.layers
                    .iter()
                    .map(|layer| layer_node(layer, with_paths))
                    .collect(),
            ),
        ),
    ])
}

/// The document for a single plane slice.
pub fn slice_document(
    context: &Context,
    settings: &Settings,
    layer: &PlannedLayer,
    contours: &[Vec<(f64, f64)>],
    duplicate_segments_removed: usize,
) -> Node {
    Node::Obj(vec![
        ("schema", int(1)),
        ("tool", text("hfw")),
        ("kind", text("slice")),
        (
            "inputs",
            Node::Arr(context.inputs.iter().cloned().map(text).collect()),
        ),
        ("units", text(context.units.clone())),
        ("scale", num(context.scale)),
        (
            "mesh",
            Node::Obj(vec![
                ("vertices", int(context.vertices)),
                ("triangles", int(context.triangles)),
            ]),
        ),
        ("settings", settings_node(settings, None)),
        (
            "slice",
            Node::Obj(vec![
                ("z", num(layer.slice_z)),
                ("contours", int(contours.len())),
                (
                    "duplicate_segments_removed",
                    int(duplicate_segments_removed),
                ),
                (
                    "rings",
                    Node::Arr(
                        contours
                            .iter()
                            .map(|ring| {
                                Node::Arr(
                                    ring.iter()
                                        .map(|(x, y)| Node::Arr(vec![num(*x), num(*y)]))
                                        .collect(),
                                )
                            })
                            .collect(),
                    ),
                ),
            ]),
        ),
        ("layer", layer_node(layer, true)),
    ])
}

/// The document for a `--check-gcode` run.
/// The intake report as JSON: every key the preset stated and how it was handled.
///
/// A key missing from this document never reached the engine at all, which is the
/// one failure the printed report cannot show -- it lists what the intake saw, and
/// a setting lost while flattening a configuration dump is not in that list. The
/// settings-boundary check in `next-layer` compares this against the keys of the
/// dump it handed over.
pub fn intake_document(report: &hfw_config::IntakeReport) -> Node {
    let mut keys = Vec::new();
    for (key, intake) in report.entries() {
        let label = match intake {
            hfw_config::Intake::Applied => "applied",
            hfw_config::Intake::Partial => "partial",
            hfw_config::Intake::Recognised => "recognised",
            hfw_config::Intake::Unconverted => "unconverted",
            hfw_config::Intake::Unknown => "unknown",
        };
        keys.push(Node::Obj(vec![
            ("name", Node::Str(key.to_owned())),
            ("intake", Node::Str(label.to_owned())),
        ]));
    }
    Node::Obj(vec![
        ("keys_seen", Node::Int(report.len() as i64)),
        ("keys", Node::Arr(keys)),
        (
            "warnings",
            Node::Arr(report.warnings().iter().cloned().map(Node::Str).collect()),
        ),
    ])
}

pub fn check_document(path: &std::path::Path, report: &VerifyReport) -> Node {
    Node::Obj(vec![
        ("schema", int(1)),
        ("tool", text("hfw")),
        ("kind", text("check")),
        ("path", text(path.display().to_string())),
        ("ok", Node::Bool(report.errors.is_empty())),
        (
            "errors",
            Node::Arr(report.errors.iter().map(|e| text(e.clone())).collect()),
        ),
        (
            "warnings",
            Node::Arr(report.warnings.iter().map(|w| text(w.clone())).collect()),
        ),
        ("layers", int(report.layers)),
        ("moves", int(report.moves)),
        ("volume", num(report.volume_mm3)),
        ("filament_mm", num(report.filament_mm)),
        ("estimated_seconds", num(report.est_seconds)),
        ("travel_crossings", int(report.travel_crossings)),
        ("first_extrusion_z", num(report.first_extrusion_z)),
        (
            "bbox_min",
            Node::Arr(report.bbox_min.iter().map(|v| num(*v)).collect()),
        ),
        (
            "bbox_max",
            Node::Arr(report.bbox_max.iter().map(|v| num(*v)).collect()),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_escaped() {
        let mut out = String::new();
        push_escaped(&mut out, "a\"b\\c\nd");
        assert_eq!(out, "\"a\\\"b\\\\c\\nd\"");
    }

    #[test]
    fn control_characters_become_escapes() {
        let mut out = String::new();
        push_escaped(&mut out, "\u{1}");
        assert_eq!(out, "\"\\u0001\"");
    }

    #[test]
    fn non_finite_numbers_become_null() {
        assert_eq!(Node::Num(f64::NAN).to_json().trim(), "null");
        assert_eq!(Node::Num(f64::INFINITY).to_json().trim(), "null");
    }

    #[test]
    fn whole_numbers_keep_no_decimal_point() {
        assert_eq!(Node::Int(100).to_json().trim(), "100");
    }

    #[test]
    fn empty_containers_stay_on_one_line() {
        assert_eq!(Node::Arr(Vec::new()).to_json().trim(), "[]");
        assert_eq!(Node::Obj(Vec::new()).to_json().trim(), "{}");
    }

    #[test]
    fn nesting_is_indented_by_two_spaces() {
        let node = Node::Obj(vec![
            ("a", Node::Int(1)),
            ("b", Node::Arr(vec![Node::Bool(true)])),
        ]);
        assert_eq!(
            node.to_json(),
            "{\n  \"a\": 1,\n  \"b\": [\n    true\n  ]\n}\n"
        );
    }
}
