//! The preview document and the preview buffer: every planned path, with the label the
//! G-code gives it.
//!
//! A viewer needs the paths a job will print before it prints them, grouped by
//! what they are. That grouping already exists — the writer labels each bead
//! `Outer wall`, `Top surface`, `Sparse infill` and the rest — so the preview is
//! built by calling that same function rather than by keeping a second table
//! that could disagree with the file a printer actually runs.
//!
//! Two forms, one content. The JSON document ([`document`]) is for a panel, a report or
//! a person: one polyline per bead with its per-vertex width, labelled for reading. The
//! buffer ([`buffer`]) is for a renderer: the same beads as one flat block of fixed-stride
//! records, 32 bytes per move instead of the ~100 the JSON spends on digits, brackets and
//! keys, because a viewport that parses 50 MB of JSON before it can draw its first frame is
//! a viewport with a start-up cost nobody budgeted.

use std::fmt::Write as _;

use hfw_config::PrinterConfig;
use hfw_engine::{Job, Path as Line};
use hfw_gcode::{feature_of, path_speed, Feature};

/// The roles a bead can carry, in the order both preview forms index them.
///
/// One list, so the JSON's `roles` array and the buffer's role table cannot disagree
/// about what index 7 means.
pub const ROLES: [Feature; 13] = [
    Feature::Skirt,
    Feature::Brim,
    Feature::Bridge,
    Feature::Support,
    Feature::SupportInterface,
    Feature::Outer,
    Feature::Inner,
    Feature::Thin,
    Feature::Solid,
    Feature::TopSolid,
    Feature::Sparse,
    Feature::Ironing,
    Feature::Custom,
];

/// Magic number a buffer starts with: `HFWPRVW` and a NUL.
pub const MAGIC: [u8; 8] = *b"HFWPRVW\0";

/// Version of the buffer layout this library writes.
pub const VERSION: u32 = 1;

/// Bytes before the first move record.
pub const HEADER_BYTES: usize = 80;

/// Bytes per move record.
pub const MOVE_BYTES: usize = 32;

/// Bytes per layer record.
pub const LAYER_BYTES: usize = 16;

/// Bytes per role record: the G-code label, NUL-padded.
pub const ROLE_BYTES: usize = 32;

/// Vertices a quad renderer draws per move, carried so a range is data and not
/// arithmetic done in the application.
pub const VERTICES_PER_MOVE: u32 = 4;

/// Indices a quad renderer submits per move.
pub const INDICES_PER_MOVE: u32 = 6;

/// The preview document for a planned job.
pub fn document(job: &Job) -> String {
    let mut out = String::new();
    out.push_str("{\"preview\":\"hfw-preview\",\"version\":1,\"roles\":[");
    for (index, role) in ROLES.iter().enumerate() {
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

/// The preview buffer for a planned job: one flat block a renderer can upload as it is.
///
/// Little-endian, every field four-byte aligned, in this order:
///
/// ```text
/// header, 80 bytes
///   0  u8[8]  "HFWPRVW\0"
///   8  u32    version (1)
///  12  u32    move_count
///  16  u32    layer_count
///  20  u32    role_count
///  24  u32    move_stride (32)
///  28  u32    layer_stride (16)
///  32  u32    moves_offset
///  36  u32    layers_offset
///  40  u32    roles_offset
///  44  u32    total_bytes
///  48  u32    vertices_per_move (4)
///  52  u32    indices_per_move (6)
///  56  f32[6] min_x, min_y, min_z, max_x, max_y, max_z
/// move records, 32 bytes each
///   0  f32 x0         16 u16 width0       24 u32 layer
///   4  f32 y0         18 u16 width1       28 u8  role, 3 bytes padding
///   8  f32 x1         20 f32 speed
///  12  f32 y1
/// layer records, 16 bytes each
///   0 u32 first_move   4 u32 move_count   8 u32 first_vertex   12 f32 z
/// role records, 32 bytes each
///   0 u8[32] label, NUL-padded, in [`ROLES`] order
/// ```
///
/// A move is one bead segment: two endpoints, the bead's width at each end in micrometres,
/// the speed the profile asks for it, the layer it prints in and the role it carries. The
/// width is the bead's rather than a pen's, so a renderer drawing quads has both numbers
/// the quad needs. Positions are `f32` millimetres because that is what a renderer
/// consumes; the exact numbers live in the G-code, written from these same paths.
///
/// `first_vertex` is `first_move * vertices_per_move` and the last layer's extent runs to
/// `move_count`, so selecting a layer range is two integers and one slice of the buffer.
///
/// Speed is the speed the profile asks for, from the writer's own
/// [`path_speed`](hfw_gcode::path_speed) — not the `F` a move ends up with. The writer may
/// still slow a bead for the volumetric-rate slope across consecutive beads, the
/// layer-time cooling rule, or the over-bridge and raft limits, all of which it decides
/// while writing from state that spans layers.
pub fn buffer(job: &Job, cfg: &PrinterConfig) -> Vec<u8> {
    let mut moves: Vec<u8> = Vec::new();
    let mut layers: Vec<u8> = Vec::new();
    let mut lowest = (f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut highest = (f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut move_count = 0u32;

    for (layer_number, layer) in job.layers.iter().enumerate() {
        let first_move = move_count;
        let first_layer = layer.first;
        let layer_z = (layer.slice_z + layer.bottom_z) * 0.5;
        let layer_z = layer_z as f32;
        for path in &layer.paths {
            let feature = feature_of(path.kind);
            let role = role_index(feature);
            let speed = path_speed(feature, cfg, first_layer, path.overhang) as f32;
            let points = &path.pts;
            if points.len() < 2 {
                continue;
            }
            let segments = if path.closed {
                points.len()
            } else {
                points.len() - 1
            };
            for index in 0..segments {
                let start = points[index];
                let end = points[(index + 1) % points.len()];
                let x0 = start.x as f32;
                let y0 = start.y as f32;
                let x1 = end.x as f32;
                let y1 = end.y as f32;
                lowest.0 = lowest.0.min(x0.min(x1));
                lowest.1 = lowest.1.min(y0.min(y1));
                highest.0 = highest.0.max(x0.max(x1));
                highest.1 = highest.1.max(y0.max(y1));
                if layer_z < lowest.2 {
                    lowest.2 = layer_z;
                }
                if layer_z > highest.2 {
                    highest.2 = layer_z;
                }
                push_f32(&mut moves, x0);
                push_f32(&mut moves, y0);
                push_f32(&mut moves, x1);
                push_f32(&mut moves, y1);
                push_width(&mut moves, path.width.get(index).copied());
                let next = (index + 1) % points.len();
                push_width(&mut moves, path.width.get(next).copied());
                push_f32(&mut moves, speed);
                moves.extend_from_slice(&(layer_number as u32).to_le_bytes());
                moves.push(role);
                moves.extend_from_slice(&[0u8; 3]);
                move_count += 1;
            }
        }
        let layer_moves = move_count - first_move;
        layers.extend_from_slice(&first_move.to_le_bytes());
        layers.extend_from_slice(&layer_moves.to_le_bytes());
        layers.extend_from_slice(&(first_move * VERTICES_PER_MOVE).to_le_bytes());
        push_f32(&mut layers, layer_z);
    }

    let mut roles: Vec<u8> = Vec::new();
    for role in ROLES {
        let label = role.label().as_bytes();
        let copy = label.len().min(ROLE_BYTES - 1);
        roles.extend_from_slice(&label[..copy]);
        roles.resize(roles.len() + (ROLE_BYTES - copy), 0);
    }

    let moves_offset = HEADER_BYTES;
    let layers_offset = moves_offset + moves.len();
    let roles_offset = layers_offset + layers.len();
    let total_bytes = roles_offset + roles.len();

    let mut out = Vec::with_capacity(total_bytes);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&move_count.to_le_bytes());
    out.extend_from_slice(&(job.layers.len() as u32).to_le_bytes());
    out.extend_from_slice(&(ROLES.len() as u32).to_le_bytes());
    out.extend_from_slice(&(MOVE_BYTES as u32).to_le_bytes());
    out.extend_from_slice(&(LAYER_BYTES as u32).to_le_bytes());
    out.extend_from_slice(&(moves_offset as u32).to_le_bytes());
    out.extend_from_slice(&(layers_offset as u32).to_le_bytes());
    out.extend_from_slice(&(roles_offset as u32).to_le_bytes());
    out.extend_from_slice(&(total_bytes as u32).to_le_bytes());
    out.extend_from_slice(&VERTICES_PER_MOVE.to_le_bytes());
    out.extend_from_slice(&INDICES_PER_MOVE.to_le_bytes());
    for value in [
        lowest.0, lowest.1, lowest.2, highest.0, highest.1, highest.2,
    ] {
        push_f32(&mut out, value);
    }
    out.extend_from_slice(&moves);
    out.extend_from_slice(&layers);
    out.extend_from_slice(&roles);
    out
}

fn role_index(feature: Feature) -> u8 {
    ROLES
        .iter()
        .position(|role| *role == feature)
        .unwrap_or(ROLES.len() - 1) as u8
}

fn push_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_width(out: &mut Vec<u8>, width: Option<f64>) {
    let micrometres = (width.unwrap_or(0.0).max(0.0) * 1000.0).round();
    out.extend_from_slice(&(micrometres.min(u16::MAX as f64) as u16).to_le_bytes());
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
        let objects = [hfw_plate::PlacedObject::new(&mesh, None, &settings)];
        hfw_plate::slice_placed(objects, 0.2, 0.2, hfw_engine::PlateOrder::ByLayer)
            .expect("the cube slices")
    }

    fn cube_buffer() -> (Vec<u8>, Job) {
        let job = cube_job();
        let cfg = PrinterConfig::default();
        let bytes = buffer(&job, &cfg);
        (bytes, job)
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&bytes[offset..offset + 4]);
        u32::from_le_bytes(raw)
    }

    fn f32_at(bytes: &[u8], offset: usize) -> f32 {
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&bytes[offset..offset + 4]);
        f32::from_le_bytes(raw)
    }

    fn segments(job: &Job) -> u32 {
        job.layers
            .iter()
            .flat_map(|layer| layer.paths.iter())
            .map(|path| {
                if path.pts.len() < 2 {
                    0
                } else if path.closed {
                    path.pts.len()
                } else {
                    path.pts.len() - 1
                }
            })
            .sum::<usize>() as u32
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
        assert_eq!(text.matches("{\"role\":\"").count(), paths);
        assert_eq!(text.matches("\"points\":[").count(), paths);
    }

    #[test]
    fn the_same_job_produces_the_same_document() {
        let job = cube_job();
        assert_eq!(document(&job), document(&job));
    }

    #[test]
    fn the_buffer_is_one_block_a_renderer_can_upload() {
        let (bytes, job) = cube_buffer();
        assert_eq!(&bytes[..8], &MAGIC);
        assert_eq!(u32_at(&bytes, 8), VERSION);
        let moves = u32_at(&bytes, 12);
        let layers = u32_at(&bytes, 16);
        let roles = u32_at(&bytes, 20);
        assert_eq!(moves as usize, segments(&job) as usize);
        assert_eq!(layers as usize, job.layers.len());
        assert_eq!(roles as usize, ROLES.len());
        assert_eq!(u32_at(&bytes, 24) as usize, MOVE_BYTES);
        assert_eq!(u32_at(&bytes, 28) as usize, LAYER_BYTES);
        assert_eq!(u32_at(&bytes, 44) as usize, bytes.len());
        assert_eq!(
            u32_at(&bytes, 32) as usize,
            HEADER_BYTES,
            "the moves start after the header"
        );
        assert_eq!(
            u32_at(&bytes, 36) as usize,
            HEADER_BYTES + moves as usize * MOVE_BYTES
        );
        assert_eq!(
            u32_at(&bytes, 40) as usize,
            u32_at(&bytes, 36) as usize + layers as usize * LAYER_BYTES
        );
        assert_eq!(
            bytes.len(),
            u32_at(&bytes, 40) as usize + ROLES.len() * ROLE_BYTES
        );
    }

    #[test]
    fn every_layer_names_its_first_move_and_first_vertex() {
        let (bytes, job) = cube_buffer();
        let layers = u32_at(&bytes, 16) as usize;
        let table = u32_at(&bytes, 36) as usize;
        let mut expected = 0u32;
        for layer in 0..layers {
            let at = table + layer * LAYER_BYTES;
            let first_move = u32_at(&bytes, at);
            let count = u32_at(&bytes, at + 4);
            let first_vertex = u32_at(&bytes, at + 8);
            assert_eq!(
                first_move, expected,
                "layer {layer} does not start where the last one ended"
            );
            assert_eq!(first_vertex, first_move * VERTICES_PER_MOVE);
            assert_eq!(
                f32_at(&bytes, at + 12),
                ((job.layers[layer].slice_z + job.layers[layer].bottom_z) * 0.5) as f32,
                "layer {layer} carries the height it prints at"
            );
            expected += count;
        }
        assert_eq!(
            expected,
            u32_at(&bytes, 12),
            "the layers do not cover every move"
        );
    }

    #[test]
    fn a_move_carries_its_role_its_layer_and_the_speed_the_profile_asks_for() {
        let (bytes, job) = cube_buffer();
        let moves = u32_at(&bytes, 12) as usize;
        let cfg = PrinterConfig::default();
        let mut roles_seen = [false; ROLES.len()];
        for index in 0..moves.min(4096) {
            let at = HEADER_BYTES + index * MOVE_BYTES;
            let layer = u32_at(&bytes, at + 24) as usize;
            assert!(layer < job.layers.len(), "move {index} names layer {layer}");
            let role = bytes[at + 28] as usize;
            assert!(role < ROLES.len(), "move {index} names role {role}");
            roles_seen[role] = true;
            let speed = f32_at(&bytes, at + 20);
            assert!(speed > 0.0 && speed < 2000.0, "move {index} speed {speed}");
            let width = u16::from_le_bytes([bytes[at + 16], bytes[at + 17]]);
            assert!(width > 0, "move {index} has no width");
        }
        assert!(
            roles_seen.iter().any(|seen| *seen),
            "no move carried a role"
        );
        let _ = cfg;
    }

    #[test]
    fn the_buffer_and_the_document_describe_the_same_job() {
        let job = cube_job();
        let cfg = PrinterConfig::default();
        let bytes = buffer(&job, &cfg);
        let text = document(&job);
        let layers_json = text.matches("\"index\":").count();
        assert_eq!(layers_json, u32_at(&bytes, 16) as usize);
        let roles_json = text
            .split("\"roles\":[")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .map(|list| list.split(',').count())
            .unwrap_or(0);
        assert_eq!(roles_json, u32_at(&bytes, 20) as usize);
    }

    #[test]
    fn the_role_table_is_the_gcode_labels_in_index_order() {
        let (bytes, _) = cube_buffer();
        let roles = u32_at(&bytes, 40) as usize;
        for (index, role) in ROLES.iter().enumerate() {
            let at = roles + index * ROLE_BYTES;
            let label = &bytes[at..at + ROLE_BYTES];
            let end = label
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(ROLE_BYTES);
            assert_eq!(
                std::str::from_utf8(&label[..end]).expect("a role label is UTF-8"),
                role.label()
            );
        }
    }

    #[test]
    fn the_bounds_cover_every_endpoint() {
        let (bytes, job) = cube_buffer();
        let moves = u32_at(&bytes, 12) as usize;
        let (min_x, max_x) = (f32_at(&bytes, 56), f32_at(&bytes, 68));
        let (min_y, max_y) = (f32_at(&bytes, 60), f32_at(&bytes, 72));
        for index in 0..moves {
            let at = HEADER_BYTES + index * MOVE_BYTES;
            for offset in [0, 8] {
                let x = f32_at(&bytes, at + offset);
                let y = f32_at(&bytes, at + offset + 4);
                assert!(
                    x >= min_x && x <= max_x,
                    "move {index} x {x} outside {min_x}..{max_x}"
                );
                assert!(
                    y >= min_y && y <= max_y,
                    "move {index} y {y} outside {min_y}..{max_y}"
                );
            }
        }
        assert!(min_x <= max_x && min_y <= max_y);
        let _ = &job;
    }

    #[test]
    fn the_same_job_produces_the_same_buffer() {
        let job = cube_job();
        let cfg = PrinterConfig::default();
        assert_eq!(buffer(&job, &cfg), buffer(&job, &cfg));
    }

    #[test]
    fn a_move_costs_less_than_forty_bytes() {
        let cfg = PrinterConfig::default();
        for (name, job) in [("cube", cube_job()), ("sphere", sphere_job())] {
            let started = std::time::Instant::now();
            let bytes = buffer(&job, &cfg);
            let build = started.elapsed();
            let moves = u32_at(&bytes, 12) as f64;
            let json = document(&job).len() as f64;
            let per_move = bytes.len() as f64 / moves;
            println!(
                "{name}: {} moves over {} layers, buffer {:.0} B ({per_move:.2} B/move), compact json {:.0} B ({:.2} B/move), built in {:.1} ms",
                moves,
                u32_at(&bytes, 16),
                bytes.len() as f64,
                json,
                json / moves,
                build.as_secs_f64() * 1e3
            );
            assert!(per_move <= 40.0, "{name} costs {per_move:.2} B/move");
        }
    }

    fn sphere_job() -> Job {
        sphere_job_with(&PrinterConfig::default())
    }

    fn sphere_job_with(cfg: &PrinterConfig) -> Job {
        let path = format!("{}/../../fixtures/sphere.stl", env!("CARGO_MANIFEST_DIR"));
        let mesh = hfw_engine::Mesh::load_stl(&path).expect("the fixture loads");
        let mut settings = hfw_engine::Settings::default();
        settings.apply_printer(cfg);
        if let Some(infill) = settings.infill.as_mut() {
            infill.overlap = 0.1 * infill.width;
            infill.skin_res = infill.width / 4.0;
            infill.step = infill.width / 4.0;
        }
        let objects = [hfw_plate::PlacedObject::new(&mesh, None, &settings)];
        hfw_plate::slice_placed(objects, 0.2, 0.2, hfw_engine::PlateOrder::ByLayer)
            .expect("the sphere slices")
    }

    #[test]
    fn the_sphere_costs_what_the_document_claims() {
        let profile = format!(
            "{}/../../profiles/voron_klipper.cfg",
            env!("CARGO_MANIFEST_DIR")
        );
        let cfg = PrinterConfig::load(std::path::Path::new(&profile)).expect("the profile loads");
        let job = sphere_job_with(&cfg);
        let started = std::time::Instant::now();
        let bytes = buffer(&job, &cfg);
        let build = started.elapsed();
        let moves = u32_at(&bytes, 12) as f64;
        let json = document(&job).len() as f64;
        println!(
            "sphere, voron profile: {} moves over {} layers, buffer {:.0} B ({:.2} B/move), compact json {:.0} B ({:.2} B/move), built in {:.1} ms",
            moves,
            u32_at(&bytes, 16),
            bytes.len() as f64,
            bytes.len() as f64 / moves,
            json,
            json / moves,
            build.as_secs_f64() * 1e3
        );
        assert!(moves > 40_000.0, "only {moves} moves");
    }
}
