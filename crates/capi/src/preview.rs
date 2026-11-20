//! The preview document: every planned path, with the label the G-code gives it.
//!
//! A viewer needs the paths a job will print before it prints them, grouped by
//! what they are. That grouping already exists — the writer labels each bead
//! `Outer wall`, `Top surface`, `Sparse infill` and the rest — so the preview is
//! built by calling that same function rather than by keeping a second table
//! that could disagree with the file a printer actually runs.
//!
//! The document is JSON: one entry per layer, one polyline per bead, with the
//! per-vertex width, the flow multiplier and the label. It is the same data the
//! command line puts in `--json-paths`, in the shape a settings panel and a
//! viewport both read without linking the engine.

use std::fmt::Write as _;

use hfw_engine::{Job, Path as Line};
use hfw_gcode::{feature_of, Feature};

/// The preview document for a planned job.
pub fn document(job: &Job) -> String {
    let mut out = String::new();
    out.push_str("{\"preview\":\"hfw-preview\",\"version\":1,\"roles\":[");
    let roles = [
        Feature::Skirt,
        Feature::Brim,
        Feature::Support,
        Feature::SupportInterface,
        Feature::Outer,
        Feature::Inner,
        Feature::Thin,
        Feature::Solid,
        Feature::TopSolid,
        Feature::Sparse,
        Feature::Ironing,
        Feature::Bridge,
    ];
    for (index, role) in roles.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        push_text(&mut out, role.label());
    }
    let _ = write!(out, "],\"layers\":[");
    for (position, layer) in job.layers.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"index\":{},\"bottom_z\":{},\"z\":{},\"print_z\":{},\"first\":{},\"paths\":[",
            layer.index, layer.bottom_z, layer.slice_z, layer.print_z, layer.first
        );
        for (index, path) in layer.paths.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            push_path(&mut out, path);
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

fn push_path(out: &mut String, path: &Line) {
    out.push_str("{\"role\":");
    push_text(out, feature_of(path.kind).label());
    let _ = write!(out, ",\"closed\":{},\"flow\":{}", path.closed, path.flow);
    out.push_str(",\"width\":[");
    for (index, width) in path.width.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(out, "{width}");
    }
    out.push_str("],\"points\":[");
    for (index, point) in path.pts.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(out, "[{},{}]", point.x, point.y);
    }
    out.push_str("]}");
}

fn push_text(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_job() -> Job {
        let path = format!("{}/../../fixtures/cube.stl", env!("CARGO_MANIFEST_DIR"));
        let mesh = hfw_engine::Mesh::load_stl(&path).expect("the fixture loads");
        let settings = hfw_engine::Settings::default();
        let objects = [(&mesh, None, &settings)];
        hfw_plate::slice_placed(objects, 0.2, 0.2, hfw_engine::PlateOrder::ByLayer)
            .expect("the cube slices")
    }

    #[test]
    fn the_document_carries_every_path_with_a_label_the_gcode_uses() {
        let job = cube_job();
        let text = document(&job);
        assert!(text.starts_with("{\"preview\":\"hfw-preview\""));
        assert!(text.contains("\"Outer wall\""));
        let paths = job
            .layers
            .iter()
            .map(|layer| layer.paths.len())
            .sum::<usize>();
        assert_eq!(text.matches("{\"role\":").count(), paths);
        assert_eq!(text.matches("\"points\":[").count(), paths);
    }

    #[test]
    fn the_same_job_produces_the_same_document() {
        let job = cube_job();
        assert_eq!(document(&job), document(&job));
    }
}
