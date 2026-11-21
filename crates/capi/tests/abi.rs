//! The C surface, exercised the way a C caller would exercise it.

use std::ffi::{c_char, CStr, CString};
use std::fs;

use hfw_capi::{
    hfw_abi_version, hfw_buffer_free, hfw_bundle_free, hfw_bundle_open, hfw_bundle_printer_models,
    hfw_bundle_profiles_for, hfw_bundle_reload, hfw_bundle_resolved_config, hfw_bundle_summary,
    hfw_gcode_free, hfw_gcode_len, hfw_gcode_text, hfw_gcode_verified, hfw_job_cancel,
    hfw_job_poll, hfw_job_start, hfw_job_take_gcode, hfw_job_wait, hfw_last_error, hfw_mesh_bounds,
    hfw_mesh_cut, hfw_mesh_free, hfw_mesh_from_bytes, hfw_mesh_index_count, hfw_mesh_indices,
    hfw_mesh_load_extensions, hfw_mesh_load_file, hfw_mesh_new, hfw_mesh_paint_get,
    hfw_mesh_paint_set, hfw_mesh_part, hfw_mesh_part_count, hfw_mesh_stl_bytes, hfw_mesh_transform,
    hfw_mesh_triangle_count, hfw_mesh_vertex_count, hfw_mesh_vertices, hfw_mesh_volume,
    hfw_plate_add, hfw_plate_add_on_bed, hfw_plate_add_positioned, hfw_plate_arrange,
    hfw_plate_bed_add, hfw_plate_bed_count, hfw_plate_bed_locked, hfw_plate_bed_name,
    hfw_plate_bed_name_get, hfw_plate_bed_object_ids, hfw_plate_bed_remove,
    hfw_plate_bed_set_locked, hfw_plate_bed_slice, hfw_plate_bounds, hfw_plate_free,
    hfw_plate_in_layer_count, hfw_plate_in_layer_get, hfw_plate_marker_count, hfw_plate_marker_get,
    hfw_plate_metadata_get, hfw_plate_new, hfw_plate_object_add_volume,
    hfw_plate_object_add_volume_placed, hfw_plate_object_apply_foreign, hfw_plate_object_bed,
    hfw_plate_object_bounds, hfw_plate_object_clear_settings, hfw_plate_object_count,
    hfw_plate_object_dialect, hfw_plate_object_duplicate, hfw_plate_object_enabled,
    hfw_plate_object_enabled_get, hfw_plate_object_foreign_clear, hfw_plate_object_foreign_json,
    hfw_plate_object_foreign_set, hfw_plate_object_ids, hfw_plate_object_label,
    hfw_plate_object_label_get, hfw_plate_object_layer_heights, hfw_plate_object_layer_heights_get,
    hfw_plate_object_mesh, hfw_plate_object_mirror, hfw_plate_object_name,
    hfw_plate_object_name_get, hfw_plate_object_paint, hfw_plate_object_paint_get,
    hfw_plate_object_placement, hfw_plate_object_placement_get, hfw_plate_object_placement_matrix,
    hfw_plate_object_placement_matrix_get, hfw_plate_object_remove, hfw_plate_object_remove_volume,
    hfw_plate_object_rotate, hfw_plate_object_set, hfw_plate_object_set_bed,
    hfw_plate_object_set_dialect, hfw_plate_object_settings_json, hfw_plate_object_source,
    hfw_plate_object_source_get, hfw_plate_object_volume_count, hfw_plate_object_volume_ids,
    hfw_plate_object_volume_kind, hfw_plate_object_volume_mesh, hfw_plate_object_volume_name,
    hfw_plate_object_volume_name_get, hfw_plate_object_volume_placement,
    hfw_plate_object_volume_placement_get, hfw_plate_object_volume_set_visible,
    hfw_plate_object_volume_settings_json, hfw_plate_object_volume_visible, hfw_plate_open_project,
    hfw_plate_preview_json, hfw_plate_project_bytes, hfw_plate_restore, hfw_plate_set_in_layer,
    hfw_plate_set_markers, hfw_plate_set_metadata, hfw_plate_slice, hfw_plate_slice_placed,
    hfw_plate_snapshot, hfw_plate_volume_set, hfw_slice_meshes, hfw_snapshot_free,
    hfw_snapshot_object_count, hfw_string_free, hfw_text_mesh, HfwBounds, HfwJobProgress,
    HfwTransform, HFW_DIALECT_ENGINE, HFW_DIALECT_ORCA, HFW_FOREIGN_UNKNOWN_NAME,
    HFW_FOREIGN_UNUSABLE_VALUE, HFW_JOB_CANCELLED, HFW_JOB_DONE, HFW_JOB_STAGE_FINISHED,
    HFW_MARKER_CUSTOM, HFW_MARKER_PAUSE, HFW_PAINT_SUPPORT, HFW_VOLUME_MODIFIER,
    HFW_VOLUME_NEGATIVE,
};

fn fixture(name: &str) -> String {
    format!("{}/../../fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn profile() -> String {
    format!(
        "{}/../../profiles/voron_klipper.cfg",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn c_string(text: &str) -> CString {
    CString::new(text).expect("a path with an interior NUL byte")
}

/// The X/Y box a file's printed moves cover, in millimetres.
fn footprint(text: &str) -> (f64, f64, f64, f64) {
    let mut out = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for line in text.lines() {
        if !(line.starts_with("G1 ") || line.starts_with("G0 ")) {
            continue;
        }
        let mut x = None;
        let mut y = None;
        for word in line.split_whitespace() {
            if let Some(value) = word.strip_prefix('X').and_then(|v| v.parse::<f64>().ok()) {
                x = Some(value);
            } else if let Some(value) = word.strip_prefix('Y').and_then(|v| v.parse::<f64>().ok()) {
                y = Some(value);
            }
        }
        if let (Some(x), Some(y)) = (x, y) {
            out = (out.0.min(x), out.1.min(y), out.2.max(x), out.3.max(y));
        }
    }
    out
}

fn last_error() -> String {
    let raw = hfw_last_error();
    if raw.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned()
    }
}

fn bounds(mesh: *const hfw_capi::HfwMesh) -> HfwBounds {
    let mut out = HfwBounds {
        min: [f64::NAN; 3],
        max: [f64::NAN; 3],
    };
    let result = unsafe { hfw_mesh_bounds(mesh, &mut out) };
    assert_eq!(result, 0, "bounds reported {}", last_error());
    out
}

#[test]
fn the_version_is_the_one_the_header_declares() {
    assert_eq!(hfw_abi_version(), 1);
}

#[test]
fn a_loaded_cube_reports_its_bounds_and_volume() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let boxed = bounds(mesh);
    assert!((boxed.min[0]).abs() < 1e-6, "min x was {}", boxed.min[0]);
    assert!(
        (boxed.max[0] - 20.0).abs() < 1e-6,
        "max x was {}",
        boxed.max[0]
    );
    assert!(
        (boxed.max[2] - 20.0).abs() < 1e-6,
        "max z was {}",
        boxed.max[2]
    );
    assert_eq!(unsafe { hfw_mesh_triangle_count(mesh) }, 12);
    let volume = unsafe { hfw_mesh_volume(mesh) };
    assert!((volume - 8000.0).abs() < 1e-3, "volume was {volume}");
    unsafe { hfw_mesh_free(mesh) };
}

#[test]
fn the_same_bytes_through_memory_and_through_a_file_agree() {
    let path = c_string(&fixture("cube.stl"));
    let from_file = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!from_file.is_null(), "load failed: {}", last_error());
    let bytes = fs::read(fixture("cube.stl")).expect("the fixture is readable");
    let from_memory = unsafe { hfw_mesh_from_bytes(bytes.as_ptr(), bytes.len()) };
    assert!(!from_memory.is_null(), "parse failed: {}", last_error());
    let a = bounds(from_file);
    let b = bounds(from_memory);
    assert_eq!(a, b);
    assert_eq!(unsafe { hfw_mesh_triangle_count(from_file) }, unsafe {
        hfw_mesh_triangle_count(from_memory)
    });
    unsafe { hfw_mesh_free(from_file) };
    unsafe { hfw_mesh_free(from_memory) };
}

#[test]
fn a_mesh_writes_itself_out_as_stl_and_reads_back() {
    let extensions = unsafe { CStr::from_ptr(hfw_mesh_load_extensions()) }
        .to_str()
        .expect("the extension list is text");
    assert!(
        extensions.contains(".obj") && extensions.contains(".3mf"),
        "{extensions}"
    );

    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let volume = unsafe { hfw_mesh_volume(mesh) };
    let triangles = unsafe { hfw_mesh_triangle_count(mesh) };

    let mut len = 0usize;
    let binary = unsafe { hfw_mesh_stl_bytes(mesh, 1, &mut len) };
    assert!(!binary.is_null(), "write failed: {}", last_error());
    assert_eq!(len, 84 + 50 * triangles as usize, "a binary STL's size");
    let reread = unsafe { hfw_mesh_from_bytes(binary as *const u8, len) };
    assert!(!reread.is_null(), "the bytes read back: {}", last_error());
    assert!((unsafe { hfw_mesh_volume(reread) } - volume).abs() < 1e-3);
    assert_eq!(unsafe { hfw_mesh_triangle_count(reread) }, triangles);
    unsafe {
        hfw_mesh_free(reread);
        hfw_buffer_free(binary, len);
    }

    let mut text_len = 0usize;
    let ascii = unsafe { hfw_mesh_stl_bytes(mesh, 0, &mut text_len) };
    assert!(!ascii.is_null(), "write failed: {}", last_error());
    let text = unsafe { std::slice::from_raw_parts(ascii as *const u8, text_len) };
    assert!(text.starts_with(b"solid ") && text.ends_with(b"endsolid next-layer\n"));
    unsafe {
        hfw_buffer_free(ascii, text_len);
        hfw_mesh_free(mesh);
    }
}

#[test]
fn a_placement_crosses_as_a_row_major_matrix() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, mesh) };
    assert_ne!(id, 0, "add failed: {}", last_error());

    let matrix: [f64; 16] = [
        0.0, -1.0, 0.0, 10.0, //
        2.0, 0.0, 0.0, 20.0, //
        0.0, 0.0, 1.0, 5.0, //
        0.0, 0.0, 0.0, 1.0,
    ];
    let placed = unsafe { hfw_plate_object_placement_matrix(plate, id, matrix.as_ptr()) };
    assert_eq!(placed, 0, "placement failed: {}", last_error());

    let mut read = [0.0_f64; 16];
    let result = unsafe { hfw_plate_object_placement_matrix_get(plate, id, read.as_mut_ptr()) };
    assert_eq!(result, 0, "read failed: {}", last_error());
    for (index, (left, right)) in matrix.iter().zip(read).enumerate() {
        assert!(
            (left - right).abs() < 1e-9,
            "element {index}: {left} came back as {right}"
        );
    }

    let mut euler = HfwTransform::default();
    let result = unsafe { hfw_plate_object_placement_get(plate, id, &mut euler) };
    assert_eq!(result, 0, "euler read failed: {}", last_error());
    assert!((euler.scale[0] - 2.0).abs() < 1e-9, "{:?}", euler.scale);
    assert!((euler.scale[1] - 1.0).abs() < 1e-9, "{:?}", euler.scale);
    assert!(
        (euler.rotation_degrees[2] - 90.0).abs() < 1e-9,
        "{:?}",
        euler.rotation_degrees
    );
    assert_eq!(euler.translation_mm, [10.0, 20.0, 5.0]);

    let mut sheared = matrix;
    sheared[5] = 0.3;
    let refused = unsafe { hfw_plate_object_placement_matrix(plate, id, sheared.as_ptr()) };
    assert_eq!(refused, -1, "a sheared matrix was accepted");
    assert!(
        last_error().contains("shear"),
        "the refusal does not say why: {}",
        last_error()
    );

    unsafe {
        hfw_plate_free(plate);
        hfw_mesh_free(mesh);
    }
}

#[test]
fn a_transform_moves_the_copy_and_leaves_the_source_alone() {
    let path = c_string(&fixture("cube.stl"));
    let source = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!source.is_null(), "load failed: {}", last_error());
    let moved = unsafe {
        hfw_mesh_transform(
            source,
            HfwTransform {
                translation_mm: [10.0, -5.0, 1.5],
                ..HfwTransform::default()
            },
        )
    };
    assert!(!moved.is_null(), "transform failed: {}", last_error());
    let before = bounds(source);
    let after = bounds(moved);
    assert!((after.min[0] - (before.min[0] + 10.0)).abs() < 1e-6);
    assert!((after.min[1] - (before.min[1] - 5.0)).abs() < 1e-6);
    assert!((after.min[2] - (before.min[2] + 1.5)).abs() < 1e-6);
    assert_eq!(before, bounds(source));
    unsafe { hfw_mesh_free(moved) };
    unsafe { hfw_mesh_free(source) };
}

#[test]
fn a_successful_call_clears_an_earlier_error() {
    let missing = c_string(&fixture("there-is-no-such-model.stl"));
    let handle = unsafe { hfw_mesh_load_file(missing.as_ptr()) };
    assert!(handle.is_null());
    assert!(!last_error().is_empty(), "a missing file reported no error");
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null());
    assert_eq!(last_error(), "");
    unsafe { hfw_mesh_free(mesh) };
}

#[test]
fn a_null_handle_is_refused_rather_than_dereferenced() {
    let mut out = HfwBounds {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    assert_eq!(unsafe { hfw_mesh_bounds(std::ptr::null(), &mut out) }, -1);
    assert!(!last_error().is_empty());
    assert_eq!(unsafe { hfw_mesh_triangle_count(std::ptr::null()) }, 0);
    assert!(unsafe { hfw_mesh_volume(std::ptr::null()) }.is_nan());
    unsafe { hfw_mesh_free(std::ptr::null_mut()) };
    unsafe { hfw_gcode_free(std::ptr::null_mut()) };
}

#[test]
fn a_plate_of_two_objects_slices_and_verifies() {
    let path = c_string(&fixture("cube.stl"));
    let left = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let right = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(
        !left.is_null() && !right.is_null(),
        "load failed: {}",
        last_error()
    );
    let placed = unsafe {
        hfw_mesh_transform(
            right,
            HfwTransform {
                translation_mm: [40.0, 0.0, 0.0],
                ..HfwTransform::default()
            },
        )
    };
    assert!(!placed.is_null(), "transform failed: {}", last_error());
    let plate = [left as *const hfw_capi::HfwMesh, placed];
    let profile = c_string(&profile());
    let gcode =
        unsafe { hfw_slice_meshes(plate.as_ptr(), plate.len(), profile.as_ptr(), 0.2, 0.2) };
    assert!(!gcode.is_null(), "slice failed: {}", last_error());
    assert_eq!(unsafe { hfw_gcode_verified(gcode) }, 1);
    let len = unsafe { hfw_gcode_len(gcode) };
    let text = unsafe { CStr::from_ptr(hfw_gcode_text(gcode)) };
    assert_eq!(text.to_bytes().len(), len);
    let text = text.to_str().expect("G-code is ASCII");
    assert!(text.contains(";TYPE:"), "the file carries no feature types");
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with(";LAYER_CHANGE"))
            .count(),
        200,
        "two 20 mm cubes at 0.2 mm are 100 layers each"
    );
    unsafe { hfw_gcode_free(gcode) };
    unsafe { hfw_mesh_free(left) };
    unsafe { hfw_mesh_free(placed) };
    unsafe { hfw_mesh_free(right) };
}

#[test]
fn the_same_plate_slices_to_the_same_bytes_twice() {
    let path = c_string(&fixture("cube.stl"));
    let profile = c_string(&profile());
    let mut texts = Vec::new();
    for _ in 0..2 {
        let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
        assert!(!mesh.is_null(), "load failed: {}", last_error());
        let plate = [mesh as *const hfw_capi::HfwMesh];
        let gcode =
            unsafe { hfw_slice_meshes(plate.as_ptr(), plate.len(), profile.as_ptr(), 0.2, 0.2) };
        assert!(!gcode.is_null(), "slice failed: {}", last_error());
        let text = unsafe { CStr::from_ptr(hfw_gcode_text(gcode)) }
            .to_bytes()
            .to_vec();
        unsafe { hfw_gcode_free(gcode) };
        unsafe { hfw_mesh_free(mesh) };
        texts.push(text);
    }
    assert_eq!(texts[0], texts[1]);
}

#[test]
fn a_plate_holds_objects_by_id_and_names_them() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    assert!(!plate.is_null());
    let first = unsafe { hfw_plate_add(plate, mesh) };
    let second = unsafe { hfw_plate_add(plate, mesh) };
    assert_ne!(first, 0);
    assert_ne!(second, 0);
    assert_ne!(first, second);
    assert_eq!(unsafe { hfw_plate_object_count(plate) }, 2);
    let name = c_string("bracket");
    assert_eq!(
        unsafe { hfw_plate_object_name(plate, first, name.as_ptr()) },
        0
    );
    assert_eq!(
        unsafe { hfw_plate_object_name(plate, 9999, name.as_ptr()) },
        -1
    );
    assert!(!last_error().is_empty());
    assert_eq!(unsafe { hfw_plate_object_remove(plate, second) }, 0);
    assert_eq!(unsafe { hfw_plate_object_count(plate) }, 1);
    assert_eq!(unsafe { hfw_plate_object_remove(plate, second) }, -1);
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(mesh) };
}

#[test]
fn a_plate_object_moves_before_it_is_sliced() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, mesh) };
    let placement = HfwTransform {
        translation_mm: [30.0, 20.0, 0.0],
        ..HfwTransform::default()
    };
    assert_eq!(
        unsafe { hfw_plate_object_placement(plate, id, placement) },
        0
    );
    let mut bounds = HfwBounds {
        min: [f64::NAN; 3],
        max: [f64::NAN; 3],
    };
    assert_eq!(
        unsafe { hfw_plate_object_bounds(plate, id, &mut bounds) },
        0
    );
    assert!(
        (bounds.min[0] - 30.0).abs() < 1e-6,
        "min x {}",
        bounds.min[0]
    );
    assert!(
        (bounds.max[1] - 40.0).abs() < 1e-6,
        "max y {}",
        bounds.max[1]
    );
    let mut whole = HfwBounds {
        min: [f64::NAN; 3],
        max: [f64::NAN; 3],
    };
    assert_eq!(unsafe { hfw_plate_bounds(plate, &mut whole) }, 0);
    assert!(
        (whole.max[0] - 50.0).abs() < 1e-6,
        "plate max x {}",
        whole.max[0]
    );
    let profile = c_string(&profile());
    let gcode = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!gcode.is_null(), "slice failed: {}", last_error());
    assert_eq!(unsafe { hfw_gcode_verified(gcode) }, 1);
    unsafe { hfw_gcode_free(gcode) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(mesh) };
}

#[test]
fn bare_geometry_is_placed_on_the_bed_by_the_placing_slice() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    assert_ne!(unsafe { hfw_plate_add_positioned(plate, mesh) }, 0);
    let profile = c_string(&profile());

    let kept = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!kept.is_null(), "slice failed: {}", last_error());
    let placed = unsafe { hfw_plate_slice_placed(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!placed.is_null(), "placing slice failed: {}", last_error());

    let kept_box = footprint(
        unsafe { CStr::from_ptr(hfw_gcode_text(kept)) }
            .to_str()
            .unwrap(),
    );
    let placed_box = footprint(
        unsafe { CStr::from_ptr(hfw_gcode_text(placed)) }
            .to_str()
            .unwrap(),
    );
    assert!(
        (kept_box.2 - kept_box.0 - (placed_box.2 - placed_box.0)).abs() < 1e-3
            && (kept_box.3 - kept_box.1 - (placed_box.3 - placed_box.1)).abs() < 1e-3,
        "the placing slice changed the footprint: {kept_box:?} against {placed_box:?}"
    );
    let kept_centre = (
        0.5 * (kept_box.0 + kept_box.2),
        0.5 * (kept_box.1 + kept_box.3),
    );
    let placed_centre = (
        0.5 * (placed_box.0 + placed_box.2),
        0.5 * (placed_box.1 + placed_box.3),
    );
    assert!(
        (kept_centre.0 - 10.0).abs() < 0.5 && (kept_centre.1 - 10.0).abs() < 0.5,
        "a kept plate printed where it sits: centre {kept_centre:?}"
    );
    assert!(
        (placed_centre.0 - 175.0).abs() < 0.5 && (placed_centre.1 - 175.0).abs() < 0.5,
        "a placed plate was not centred on the 350 mm bed: centre {placed_centre:?}"
    );
    assert_eq!(unsafe { hfw_gcode_verified(placed) }, 1);
    unsafe { hfw_gcode_free(kept) };
    unsafe { hfw_gcode_free(placed) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(mesh) };
}

#[test]
fn a_disabled_plate_object_leaves_the_job() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, mesh) };
    assert_eq!(unsafe { hfw_plate_object_enabled(plate, id, 0) }, 0);
    let profile = c_string(&profile());
    let gcode = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(gcode.is_null());
    assert!(
        last_error().contains("disabled"),
        "the refusal did not name the reason: {}",
        last_error()
    );
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(mesh) };
}

#[test]
fn the_settings_surface_arrives_as_one_json_document() {
    let raw = hfw_capi::hfw_settings_surface_json();
    assert!(!raw.is_null());
    let text = unsafe { CStr::from_ptr(raw) }
        .to_str()
        .expect("the surface is UTF-8");
    assert!(text.starts_with('{'));
    assert!(text.contains("\"surface\": \"hfw-settings\""));
    assert!(text.contains("\"counts\": {"));
    let named = text.matches("\"name\":").count();
    assert!(named > 300, "only {named} settings in the surface");
    assert!(
        text.matches("\"state\": \"unbuilt\"").count() >= 8,
        "the panel cannot tell which features are unavailable"
    );
    assert_eq!(
        text.matches("\"status\": \"applied\"").count()
            + text.matches("\"status\": \"partial\"").count()
            + text.matches("\"status\": \"missing\"").count()
            + text.matches("\"status\": \"n/a\"").count(),
        named
    );
    let again = unsafe { CStr::from_ptr(hfw_capi::hfw_settings_surface_json()) };
    assert_eq!(text, again.to_str().expect("still UTF-8"));
}

#[test]
fn one_setting_reaches_one_object_and_names_what_it_refuses() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    assert!(!plate.is_null());
    let id = unsafe { hfw_plate_add(plate, mesh) };
    assert_ne!(id, 0);
    let key = c_string("sparse_infill_density");
    let value = c_string("40%");
    assert_eq!(
        unsafe { hfw_plate_object_set(plate, id, key.as_ptr(), value.as_ptr()) },
        0,
        "{}",
        last_error()
    );
    let unknown = c_string("no_such_setting");
    assert_eq!(
        unsafe { hfw_plate_object_set(plate, id, unknown.as_ptr(), value.as_ptr()) },
        -1,
        "an unknown key was accepted"
    );
    assert!(last_error().contains("no_such_setting"), "{}", last_error());
    let walls = c_string("walls");
    let bad = c_string("four");
    assert_eq!(
        unsafe { hfw_plate_object_set(plate, id, walls.as_ptr(), bad.as_ptr()) },
        -1,
        "a value the key cannot hold was accepted"
    );
    assert!(
        last_error().contains("walls") && last_error().contains("four"),
        "{}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_object_clear_settings(plate, id) }, 0);
    assert_eq!(unsafe { hfw_plate_object_clear_settings(plate, 9999) }, -1);
    assert_eq!(
        unsafe { hfw_plate_object_set(plate, 9999, key.as_ptr(), value.as_ptr()) },
        -1
    );
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn the_preview_labels_every_bead_the_way_the_gcode_does() {
    let path = c_string(&fixture("cube.stl"));
    let profile = c_string(&profile());
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    assert!(!plate.is_null());
    let id = unsafe { hfw_plate_add(plate, mesh) };
    assert_ne!(id, 0);
    let preview = unsafe { hfw_plate_preview_json(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!preview.is_null(), "preview failed: {}", last_error());
    let text = unsafe { CStr::from_ptr(preview) }
        .to_str()
        .expect("UTF-8")
        .to_owned();
    unsafe { hfw_string_free(preview) };
    assert!(text.starts_with("{\"preview\":\"hfw-preview\""));
    assert!(
        text.contains("\"Outer wall\""),
        "no perimeter in the preview"
    );
    let beads = text.matches("{\"role\":").count();
    assert!(beads > 100, "only {beads} beads in the preview");
    let gcode = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!gcode.is_null(), "slice failed: {}", last_error());
    let code = unsafe { CStr::from_ptr(hfw_gcode_text(gcode)) }
        .to_str()
        .expect("UTF-8")
        .to_owned();
    for role in [
        "Outer wall",
        "Inner wall",
        "Internal solid infill",
        "Top surface",
    ] {
        if text.contains(&format!("\"role\":\"{role}\"")) {
            assert!(
                code.contains(&format!(";TYPE:{role}")),
                "the preview offers `{role}` and the file never prints it"
            );
        }
    }
    unsafe { hfw_gcode_free(gcode) };
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn a_threaded_job_writes_the_same_file_as_a_synchronous_slice() {
    let path = c_string(&fixture("cube.stl"));
    let profile = c_string(&profile());
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null());
    let plate = hfw_plate_new();
    assert_ne!(unsafe { hfw_plate_add(plate, mesh) }, 0);
    let job = unsafe { hfw_job_start(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!job.is_null(), "start failed: {}", last_error());
    let status = unsafe { hfw_job_wait(job) };
    assert_eq!(status, HFW_JOB_DONE, "job failed: {}", last_error());
    let mut progress = HfwJobProgress::default();
    assert_eq!(unsafe { hfw_job_poll(job, &mut progress) }, HFW_JOB_DONE);
    assert_eq!(progress.stage, HFW_JOB_STAGE_FINISHED);
    assert!(progress.total > 0 && progress.done == progress.total);
    let async_gcode = unsafe { hfw_job_take_gcode(job) };
    assert!(!async_gcode.is_null(), "take failed: {}", last_error());
    let sync_gcode = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!sync_gcode.is_null(), "slice failed: {}", last_error());
    let threaded = unsafe { CStr::from_ptr(hfw_gcode_text(async_gcode)) }
        .to_str()
        .unwrap()
        .to_owned();
    let synchronous = unsafe { CStr::from_ptr(hfw_gcode_text(sync_gcode)) }
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        threaded, synchronous,
        "the threaded job and the slice disagree"
    );
    unsafe { hfw_gcode_free(async_gcode) };
    unsafe { hfw_gcode_free(sync_gcode) };
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn a_cancelled_job_yields_no_file() {
    let path = c_string(&fixture("cube.stl"));
    let profile = c_string(&profile());
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    for _ in 0..24 {
        assert_ne!(unsafe { hfw_plate_add(plate, mesh) }, 0);
    }
    let job = unsafe { hfw_job_start(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!job.is_null());
    assert_eq!(unsafe { hfw_job_cancel(job) }, 0);
    let status = unsafe { hfw_job_wait(job) };
    assert!(
        status == HFW_JOB_CANCELLED || status == HFW_JOB_DONE,
        "unexpected status {status}: {}",
        last_error()
    );
    if status == HFW_JOB_CANCELLED {
        assert!(
            unsafe { hfw_job_take_gcode(job) }.is_null(),
            "a cancelled job produced a file"
        );
    } else {
        let gcode = unsafe { hfw_job_take_gcode(job) };
        assert!(!gcode.is_null());
        unsafe { hfw_gcode_free(gcode) };
    }
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn a_duplicated_object_carries_its_settings_and_turns_in_place() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, mesh) };
    assert_ne!(id, 0);
    let walls = c_string("walls");
    let three = c_string("3");
    assert_eq!(
        unsafe { hfw_plate_object_set(plate, id, walls.as_ptr(), three.as_ptr()) },
        0
    );
    let copy = unsafe { hfw_plate_object_duplicate(plate, id) };
    assert_ne!(copy, 0, "duplicate failed: {}", last_error());
    assert_ne!(copy, id);
    assert_eq!(unsafe { hfw_plate_object_count(plate) }, 2);
    let mut original = HfwBounds {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    let mut duplicated = HfwBounds {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    assert_eq!(
        unsafe { hfw_plate_object_bounds(plate, id, &mut original) },
        0
    );
    assert_eq!(
        unsafe { hfw_plate_object_bounds(plate, copy, &mut duplicated) },
        0
    );
    assert_eq!(
        original.min, duplicated.min,
        "the copy lands on the original"
    );
    assert_eq!(
        unsafe { hfw_plate_object_rotate(plate, copy, 2, 90.0) },
        0,
        "{}",
        last_error()
    );
    let mut turned = HfwBounds {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    assert_eq!(
        unsafe { hfw_plate_object_bounds(plate, copy, &mut turned) },
        0
    );
    let centre_before = original.min[0] + original.max[0];
    let centre_after = turned.min[0] + turned.max[0];
    assert!(
        (centre_before - centre_after).abs() < 1e-6,
        "the turn moved the part"
    );
    let width_before = original.max[0] - original.min[0];
    let width_after = turned.max[1] - turned.min[1];
    assert!(
        (width_before - width_after).abs() < 1e-6,
        "the turn did not swap X for Y"
    );
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn a_plate_is_laid_out_on_the_bed_and_a_part_that_does_not_fit_is_named() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    for _ in 0..4 {
        assert_ne!(unsafe { hfw_plate_add(plate, mesh) }, 0);
    }
    assert_eq!(
        unsafe { hfw_plate_arrange(plate, 250.0, 250.0, 5.0) },
        0,
        "{}",
        last_error()
    );
    let mut bounds = HfwBounds {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    assert_eq!(unsafe { hfw_plate_bounds(plate, &mut bounds) }, 0);
    assert!(bounds.min[0] >= -1e-6 && bounds.min[1] >= -1e-6);
    assert!(bounds.max[0] <= 250.0 && bounds.max[1] <= 250.0);
    let mut widths = Vec::new();
    for id in 1..=4 {
        let mut object = HfwBounds {
            min: [0.0; 3],
            max: [0.0; 3],
        };
        assert_eq!(
            unsafe { hfw_plate_object_bounds(plate, id, &mut object) },
            0
        );
        widths.push((object.min[0], object.min[1]));
    }
    widths.sort_by(|left, right| left.partial_cmp(right).expect("finite"));
    widths.dedup();
    assert_eq!(
        widths.len(),
        4,
        "two objects were placed on top of each other"
    );
    assert_eq!(unsafe { hfw_plate_arrange(plate, 20.0, 20.0, 5.0) }, -1);
    assert!(
        last_error().contains("does not fit"),
        "error was `{}`",
        last_error()
    );
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn a_mirrored_object_still_slices_and_verifies() {
    let path = c_string(&fixture("cube.stl"));
    let profile = c_string(&profile());
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, mesh) };
    let mut before = HfwBounds::default();
    assert_eq!(
        unsafe { hfw_plate_object_bounds(plate, id, &mut before) },
        0
    );
    assert_eq!(
        unsafe { hfw_plate_object_mirror(plate, id, 0) },
        0,
        "{}",
        last_error()
    );
    let mut after = HfwBounds::default();
    assert_eq!(unsafe { hfw_plate_object_bounds(plate, id, &mut after) }, 0);
    assert!(
        (after.min[0] - before.min[0]).abs() < 1e-6,
        "the mirror moved the part"
    );
    assert!(
        (after.max[0] - before.max[0]).abs() < 1e-6,
        "the mirror moved the part"
    );
    let gcode = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!gcode.is_null(), "slice failed: {}", last_error());
    assert_eq!(
        unsafe { hfw_gcode_verified(gcode) },
        1,
        "the verifier refused a mirrored part"
    );
    unsafe { hfw_gcode_free(gcode) };
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn a_plate_round_trips_through_a_project_file_and_still_prints_the_same() {
    let path = c_string(&fixture("cube.stl"));
    let profile = c_string(&profile());
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let first = unsafe { hfw_plate_add(plate, mesh) };
    let second = unsafe { hfw_plate_add(plate, mesh) };
    let walls = c_string("walls");
    let three = c_string("3");
    assert_eq!(
        unsafe { hfw_plate_object_set(plate, first, walls.as_ptr(), three.as_ptr()) },
        0
    );
    let placement = HfwTransform {
        translation_mm: [40.0, 0.0, 0.0],
        ..HfwTransform::default()
    };
    assert_eq!(
        unsafe { hfw_plate_object_placement(plate, second, placement) },
        0
    );
    assert_eq!(unsafe { hfw_plate_object_enabled(plate, second, 0) }, 0);

    let mut len = 0usize;
    let bytes = unsafe { hfw_plate_project_bytes(plate, &mut len) };
    assert!(!bytes.is_null(), "save failed: {}", last_error());
    assert!(len > 1000, "the project is only {len} bytes");

    let reopened = unsafe { hfw_plate_open_project(bytes, len, profile.as_ptr()) };
    assert!(!reopened.is_null(), "open failed: {}", last_error());
    assert_eq!(unsafe { hfw_plate_object_count(reopened) }, 2);
    assert_eq!(unsafe { hfw_plate_object_enabled(reopened, 1, 1) }, 0);
    assert_eq!(unsafe { hfw_plate_object_enabled(reopened, 2, 0) }, 0);

    let original = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    let restored = unsafe { hfw_plate_slice(reopened, profile.as_ptr(), 0.2, 0.2) };
    assert!(!original.is_null(), "slice failed: {}", last_error());
    assert!(
        !restored.is_null(),
        "the reopened plate failed: {}",
        last_error()
    );
    let before = unsafe { CStr::from_ptr(hfw_gcode_text(original)) }
        .to_str()
        .unwrap()
        .to_owned();
    let after = unsafe { CStr::from_ptr(hfw_gcode_text(restored)) }
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(before, after, "the project does not print what it saved");

    unsafe { hfw_gcode_free(original) };
    unsafe { hfw_gcode_free(restored) };
    unsafe { hfw_plate_free(reopened) };
    unsafe { hfw_buffer_free(bytes, len) };
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn paint_round_trips_through_the_mesh_and_the_plate_and_is_refused_when_wrong() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let triangles = unsafe { hfw_mesh_triangle_count(mesh) };

    let mut states = vec![0u8; triangles];
    states[3] = 1;
    states[7] = 2;
    assert_eq!(
        unsafe { hfw_mesh_paint_set(mesh, 0, states.as_ptr(), states.len()) },
        0,
        "the states match the triangle count: {}",
        last_error()
    );
    let mut read = vec![0u8; triangles];
    let written = unsafe { hfw_mesh_paint_get(mesh, 0, read.as_mut_ptr(), read.len()) };
    assert_eq!(
        written as usize, triangles,
        "one byte per triangle comes back"
    );
    assert_eq!(read, states, "the channel reads back what was written");

    let mut seam = vec![0u8; triangles];
    seam[5] = 1;
    assert_eq!(
        unsafe { hfw_mesh_paint_set(mesh, 1, seam.as_ptr(), seam.len()) },
        0,
        "the seam channel takes states: {}",
        last_error()
    );
    let mut seam_read = vec![0u8; triangles];
    let seam_written =
        unsafe { hfw_mesh_paint_get(mesh, 1, seam_read.as_mut_ptr(), seam_read.len()) };
    assert_eq!(seam_written as usize, triangles);
    assert_eq!(
        seam_read, seam,
        "the seam channel reads back its own states"
    );

    let short = [0u8; 3];
    assert_eq!(
        unsafe { hfw_mesh_paint_set(mesh, 0, short.as_ptr(), short.len()) },
        -1,
        "three states cannot describe a twelve-triangle cube"
    );
    assert!(
        last_error().contains("3 states"),
        "error was {}",
        last_error()
    );
    assert_eq!(
        unsafe { hfw_mesh_paint_set(mesh, 4, states.as_ptr(), states.len()) },
        -1,
        "there is no channel 4"
    );

    let profile = c_string(&profile());
    let plate = hfw_plate_new();
    assert!(!plate.is_null(), "plate failed: {}", last_error());
    let id = unsafe { hfw_plate_add(plate, mesh) };
    assert_ne!(id, 0, "add failed: {}", last_error());
    assert_eq!(
        unsafe { hfw_plate_object_paint(plate, id, 0, states.as_ptr(), states.len()) },
        0,
        "the object takes the states: {}",
        last_error()
    );
    let mut object_read = vec![0u8; triangles];
    let object_written = unsafe {
        hfw_plate_object_paint_get(plate, id, 0, object_read.as_mut_ptr(), object_read.len())
    };
    assert_eq!(object_written as usize, triangles);
    assert_eq!(
        object_read, states,
        "the object holds the paint it was given"
    );

    let mut len = 0usize;
    let bytes = unsafe { hfw_plate_project_bytes(plate, &mut len) };
    assert!(!bytes.is_null(), "save failed: {}", last_error());
    let reopened = unsafe { hfw_plate_open_project(bytes.cast(), len, profile.as_ptr()) };
    assert!(!reopened.is_null(), "open failed: {}", last_error());
    let mut saved_read = vec![0u8; triangles];
    let saved_written = unsafe {
        hfw_plate_object_paint_get(reopened, 1, 0, saved_read.as_mut_ptr(), saved_read.len())
    };
    assert_eq!(
        saved_written as usize, triangles,
        "the project carried the paint"
    );
    assert_eq!(
        saved_read, states,
        "the reopened object is painted the same"
    );

    unsafe { hfw_plate_free(reopened) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(mesh) };
}

#[test]
fn a_volume_crosses_the_abi_with_an_id_a_placement_and_its_kind() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!cube.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(id, 0);
    assert_eq!(unsafe { hfw_plate_object_volume_count(plate, id) }, 0);

    let empty = unsafe { hfw_mesh_from_bytes(std::ptr::null(), 0) };
    assert!(empty.is_null(), "an empty mesh is refused");

    let face_path = c_string(&fixture("cube.stl"));
    let cutter = unsafe { hfw_mesh_load_file(face_path.as_ptr()) };

    let first = unsafe { hfw_plate_object_add_volume(plate, id, HFW_VOLUME_NEGATIVE, cutter) };
    assert_ne!(first, 0, "the volume attaches: {}", last_error());
    assert_eq!(unsafe { hfw_plate_object_volume_count(plate, id) }, 1);

    let matrix: [f64; 16] = [
        1.0, 0.0, 0.0, 4.0, //
        0.0, 1.0, 0.0, 5.0, //
        0.0, 0.0, 1.0, 6.0, //
        0.0, 0.0, 0.0, 1.0,
    ];
    let second = unsafe {
        hfw_plate_object_add_volume_placed(plate, id, HFW_VOLUME_MODIFIER, cutter, matrix.as_ptr())
    };
    assert_ne!(second, 0, "a placed volume attaches: {}", last_error());
    assert_ne!(second, first, "two volumes get two ids");

    let mut ids = [0u64; 4];
    let count = unsafe { hfw_plate_object_volume_ids(plate, id, ids.as_mut_ptr(), ids.len()) };
    assert_eq!(count, 2, "both ids are reported");
    assert_eq!(&ids[..2], &[first, second], "in the order they were added");
    assert_eq!(
        unsafe { hfw_plate_object_volume_ids(plate, id, std::ptr::null_mut(), 0) },
        2,
        "a null buffer sizes the call"
    );

    let mut kind = -1;
    assert_eq!(
        unsafe { hfw_plate_object_volume_kind(plate, id, first, &mut kind) },
        0
    );
    assert_eq!(kind, HFW_VOLUME_NEGATIVE, "the kind reads back by id");

    let mut read = [0.0f64; 16];
    assert_eq!(
        unsafe { hfw_plate_object_volume_placement_get(plate, id, second, read.as_mut_ptr()) },
        0,
        "the placement reads back: {}",
        last_error()
    );
    for (index, (left, right)) in matrix.iter().zip(read).enumerate() {
        assert!(
            (left - right).abs() < 1e-9,
            "element {index}: {left} came back as {right}"
        );
    }
    let mut identity = [0.0f64; 16];
    assert_eq!(
        unsafe { hfw_plate_object_volume_placement_get(plate, id, first, identity.as_mut_ptr()) },
        0
    );
    assert_eq!(
        identity,
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        "a volume added without one sits at its own coordinates"
    );

    let moved: [f64; 16] = [
        2.0, 0.0, 0.0, 7.0, //
        0.0, 2.0, 0.0, 8.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ];
    assert_eq!(
        unsafe { hfw_plate_object_volume_placement(plate, id, first, moved.as_ptr()) },
        0,
        "a volume moves inside its object: {}",
        last_error()
    );
    let mut sheared = moved;
    sheared[1] = 0.4;
    assert_eq!(
        unsafe { hfw_plate_object_volume_placement(plate, id, first, sheared.as_ptr()) },
        -1,
        "a matrix that shears is refused"
    );

    let mut beyond = -1;
    assert_eq!(
        unsafe { hfw_plate_object_volume_kind(plate, id, 9, &mut beyond) },
        -1,
        "a volume that does not exist is refused"
    );
    assert!(
        last_error().contains("has no volume 9"),
        "the refusal says which: {}",
        last_error()
    );

    assert_eq!(
        unsafe { hfw_plate_object_add_volume(plate, id, 7, cutter) },
        0,
        "an unknown kind is refused"
    );
    assert!(
        last_error().contains("volume kind 7 does not exist"),
        "the refusal lists the kinds: {}",
        last_error()
    );

    assert_eq!(
        unsafe {
            hfw_plate_volume_set(
                plate,
                id,
                first,
                c_string("sparse_infill_density").as_ptr(),
                c_string("0.9").as_ptr(),
            )
        },
        -1,
        "a negative volume has no region to print differently"
    );

    assert_eq!(
        unsafe { hfw_plate_object_remove_volume(plate, id, first) },
        0,
        "the volume detaches: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_object_volume_count(plate, id) }, 1);
    let mut ids = [0u64; 4];
    let count = unsafe { hfw_plate_object_volume_ids(plate, id, ids.as_mut_ptr(), ids.len()) };
    assert_eq!(count, 1);
    assert_eq!(
        ids[0], second,
        "removing a volume does not renumber the ones after it"
    );
    assert_eq!(
        unsafe { hfw_plate_object_remove_volume(plate, id, first) },
        -1,
        "a removed volume id is gone for good"
    );

    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
    unsafe { hfw_mesh_free(cutter) };
}

#[test]
fn per_object_layer_heights_cross_the_abi_and_read_back() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(id, 0);

    let round_trip = || -> Vec<(f64, f64)> {
        let mut out = vec![0.0f64; 8];
        let written =
            unsafe { hfw_plate_object_layer_heights_get(plate, id, out.as_mut_ptr(), out.len()) };
        assert!(written >= 0, "read failed: {}", last_error());
        out.truncate(written as usize * 2);
        out.chunks_exact(2).map(|pair| (pair[0], pair[1])).collect()
    };
    assert!(
        round_trip().is_empty(),
        "an object has no profile by default"
    );

    let pairs = [0.0, 0.2, 4.0, 0.12];
    assert_eq!(
        unsafe { hfw_plate_object_layer_heights(plate, id, pairs.as_ptr(), 2) },
        0,
        "the profile is accepted: {}",
        last_error()
    );
    assert_eq!(round_trip(), vec![(0.0, 0.2), (4.0, 0.12)]);

    let backwards = [8.0, 0.2, 2.0, 0.12];
    assert_eq!(
        unsafe { hfw_plate_object_layer_heights(plate, id, backwards.as_ptr(), 2) },
        -1,
        "a descending profile is refused"
    );
    assert!(
        last_error().contains("does not come after"),
        "the refusal says what is wrong: {}",
        last_error()
    );

    assert_eq!(
        unsafe { hfw_plate_object_layer_heights(plate, id, std::ptr::null(), 0) },
        0,
        "an empty list clears the profile: {}",
        last_error()
    );
    assert!(round_trip().is_empty());

    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
}

#[test]
fn plate_markers_cross_the_abi_sorted_and_read_back() {
    let plate = hfw_plate_new();
    let custom = c_string("M117 check the seam");
    let kinds = [HFW_MARKER_CUSTOM, HFW_MARKER_PAUSE];
    let zs = [4.0, 1.0];
    let texts = [custom.as_ptr(), std::ptr::null()];
    assert_eq!(
        unsafe { hfw_plate_set_markers(plate, kinds.as_ptr(), zs.as_ptr(), texts.as_ptr(), 2) },
        0,
        "the timeline is accepted: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_marker_count(plate) }, 2);

    let mut kind = -1i32;
    let mut z = 0.0f64;
    let mut text = [0 as c_char; 64];
    assert_eq!(
        unsafe { hfw_plate_marker_get(plate, 0, &mut kind, &mut z, text.as_mut_ptr(), text.len()) },
        0,
        "marker 0 reads: {}",
        last_error()
    );
    assert_eq!(
        kind, HFW_MARKER_PAUSE,
        "the timeline reads back sorted by Z"
    );
    assert_eq!(z, 1.0);
    assert_eq!(unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }, c"");
    assert_eq!(
        unsafe { hfw_plate_marker_get(plate, 1, &mut kind, &mut z, text.as_mut_ptr(), text.len()) },
        0
    );
    assert_eq!(kind, HFW_MARKER_CUSTOM);
    assert_eq!(z, 4.0);
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }
            .to_str()
            .unwrap(),
        "M117 check the seam"
    );
    assert_eq!(
        unsafe { hfw_plate_marker_get(plate, 2, &mut kind, &mut z, text.as_mut_ptr(), text.len()) },
        -1,
        "a marker past the end is refused"
    );

    let unknown = [9];
    let one = [1.0];
    let empty = [std::ptr::null()];
    assert_eq!(
        unsafe { hfw_plate_set_markers(plate, unknown.as_ptr(), one.as_ptr(), empty.as_ptr(), 1) },
        -1,
        "an unknown kind is refused"
    );
    assert!(
        last_error().contains("custom"),
        "the refusal lists the kinds it does know: {}",
        last_error()
    );
    assert_eq!(
        unsafe { hfw_plate_marker_count(plate) },
        2,
        "a refused list left the timeline alone"
    );

    assert_eq!(
        unsafe {
            hfw_plate_set_markers(
                plate,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        },
        0,
        "an empty list clears the timeline"
    );
    assert_eq!(unsafe { hfw_plate_marker_count(plate) }, 0);
    unsafe { hfw_plate_free(plate) };
}

#[test]
fn in_layer_strokes_cross_the_abi_and_read_back() {
    let plate = hfw_plate_new();
    let zs = [0.2f64];
    let counts = [3usize];
    let widths = [0.5f64];
    let heights = [0.2f64];
    let speeds = [25.0f64];
    let starts = [0usize];
    let points = [10.0f64, 10.0, 20.0, 10.0, 20.0, 20.0];
    let prefix = c_string("M104 S240");
    let prefixes = [prefix.as_ptr()];
    assert_eq!(
        unsafe {
            hfw_plate_set_in_layer(
                plate,
                zs.as_ptr(),
                counts.as_ptr(),
                widths.as_ptr(),
                heights.as_ptr(),
                speeds.as_ptr(),
                starts.as_ptr(),
                points.as_ptr(),
                prefixes.as_ptr(),
                1,
            )
        },
        0,
        "the stroke is accepted: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_in_layer_count(plate) }, 1);

    let mut z = 0.0f64;
    let mut width = 0.0f64;
    let mut height = 0.0f64;
    let mut speed = 0.0f64;
    let mut out = [0.0f64; 6];
    let mut text = [0 as c_char; 32];
    assert_eq!(
        unsafe {
            hfw_plate_in_layer_get(
                plate,
                0,
                &mut z,
                &mut width,
                &mut height,
                &mut speed,
                out.as_mut_ptr(),
                3,
                text.as_mut_ptr(),
                text.len(),
            )
        },
        0,
        "the stroke reads back: {}",
        last_error()
    );
    assert_eq!((z, width, height, speed), (0.2, 0.5, 0.2, 25.0));
    assert_eq!(out, [10.0, 10.0, 20.0, 10.0, 20.0, 20.0]);
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }
            .to_str()
            .unwrap(),
        "M104 S240"
    );
    assert_eq!(
        unsafe {
            hfw_plate_in_layer_get(
                plate,
                0,
                &mut z,
                &mut width,
                &mut height,
                &mut speed,
                out.as_mut_ptr(),
                2,
                text.as_mut_ptr(),
                text.len(),
            )
        },
        -1,
        "a buffer too small for the stroke is refused"
    );

    let single = [0usize];
    let broken = [0.0f64, 0.0];
    assert_eq!(
        unsafe {
            hfw_plate_set_in_layer(
                plate,
                zs.as_ptr(),
                single.as_ptr(),
                widths.as_ptr(),
                heights.as_ptr(),
                speeds.as_ptr(),
                starts.as_ptr(),
                broken.as_ptr(),
                prefixes.as_ptr(),
                1,
            )
        },
        -1,
        "a stroke with one point is refused"
    );
    assert_eq!(
        unsafe { hfw_plate_in_layer_count(plate) },
        1,
        "the refused list changed nothing"
    );

    assert_eq!(
        unsafe {
            hfw_plate_set_in_layer(
                plate,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        },
        0,
        "an empty list clears the strokes"
    );
    assert_eq!(unsafe { hfw_plate_in_layer_count(plate) }, 0);
    unsafe { hfw_plate_free(plate) };
}

/// Total extrusion in a G-code document, in millimetres of filament.
fn extruded(gcode: &str) -> f64 {
    gcode
        .lines()
        .filter(|line| line.starts_with("G1 ") || line.starts_with("G2 "))
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix('E'))
                .and_then(|value| value.parse::<f64>().ok())
        })
        .sum()
}

/// A font this machine has, or `None` when it has none.
fn system_font() -> Option<String> {
    [
        "/usr/share/fonts/TTF/FiraCode-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]
    .iter()
    .find(|path| std::path::Path::new(path).exists())
    .map(|path| (*path).to_owned())
}

#[test]
fn a_cut_crosses_the_abi_and_both_halves_are_closed() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!cube.is_null(), "load failed: {}", last_error());
    let whole = unsafe { hfw_mesh_volume(cube) };
    let plane_normal = [0.0_f64, 0.0, 1.0];
    let plane_point = [10.0_f64, 10.0, 10.0];
    let mut below = std::ptr::null_mut();
    let mut above = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            hfw_mesh_cut(
                cube,
                plane_normal.as_ptr(),
                plane_point.as_ptr(),
                &mut below,
                &mut above,
            )
        },
        0,
        "the cut runs: {}",
        last_error()
    );
    assert!(!below.is_null() && !above.is_null());
    let lower = unsafe { hfw_mesh_volume(below) };
    let upper = unsafe { hfw_mesh_volume(above) };
    assert!(
        (lower + upper - whole).abs() < 1.0,
        "the halves add up: {lower} + {upper} against {whole}"
    );
    assert!(
        (lower - whole / 2.0).abs() < 1.0,
        "the cut is through the middle"
    );

    let empty_normal = [0.0_f64, 0.0, 0.0];
    let mut refused = std::ptr::null_mut();
    let mut spare = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            hfw_mesh_cut(
                cube,
                empty_normal.as_ptr(),
                plane_point.as_ptr(),
                &mut refused,
                &mut spare,
            )
        },
        -1,
        "a normal with no direction is refused"
    );
    assert!(refused.is_null());
    assert!(
        last_error().contains("direction"),
        "the refusal says why: {}",
        last_error()
    );

    let surface = [0.0_f64, 0.0, 20.0];
    let mut bottom = std::ptr::null_mut();
    let mut top = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            hfw_mesh_cut(
                cube,
                plane_normal.as_ptr(),
                surface.as_ptr(),
                &mut bottom,
                &mut top,
            )
        },
        0
    );
    assert!((unsafe { hfw_mesh_volume(bottom) } - whole).abs() < 1.0);
    assert_eq!(unsafe { hfw_mesh_triangle_count(top) }, 0);
    unsafe {
        hfw_mesh_free(bottom);
        hfw_mesh_free(top);
        hfw_mesh_free(below);
        hfw_mesh_free(above);
        hfw_mesh_free(cube);
    }
}

#[test]
fn separate_shells_split_across_the_abi() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!cube.is_null(), "load failed: {}", last_error());
    assert_eq!(
        unsafe { hfw_mesh_part_count(cube) },
        1,
        "a cube is one shell"
    );
    let moved = unsafe {
        hfw_mesh_transform(
            cube,
            HfwTransform {
                scale: [1.0, 1.0, 1.0],
                rotation_degrees: [0.0, 0.0, 0.0],
                translation_mm: [40.0, 0.0, 0.0],
            },
        )
    };
    assert!(!moved.is_null(), "transform failed: {}", last_error());
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for mesh in [cube, moved] {
        let count = unsafe { hfw_mesh_index_count(mesh) };
        let corners = count * 3;
        let mut buffer = vec![0.0_f64; corners * 3];
        assert_eq!(
            unsafe { hfw_mesh_vertices(mesh, buffer.as_mut_ptr(), corners) },
            0,
            "vertices read: {}",
            last_error()
        );
        let mut faces = vec![0_u32; count * 3];
        assert_eq!(
            unsafe { hfw_mesh_indices(mesh, faces.as_mut_ptr(), count) },
            0,
            "indices read: {}",
            last_error()
        );
        let offset = (vertices.len() / 3) as u32;
        vertices.extend_from_slice(&buffer);
        indices.extend(faces.iter().map(|index| index + offset));
    }
    let joined = unsafe {
        hfw_mesh_new(
            vertices.as_ptr(),
            vertices.len() / 3,
            indices.as_ptr(),
            indices.len() / 3,
        )
    };
    assert!(
        !joined.is_null(),
        "the joined mesh builds: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_mesh_part_count(joined) }, 2, "two shells");
    let first = unsafe { hfw_mesh_part(joined, 0) };
    let other = unsafe { hfw_mesh_part(joined, 1) };
    assert!(!first.is_null() && !other.is_null());
    let sum = unsafe { hfw_mesh_volume(first) } + unsafe { hfw_mesh_volume(other) };
    assert!(
        (sum - unsafe { hfw_mesh_volume(joined) }).abs() < 1.0,
        "the shells add up: {sum}"
    );
    assert!(
        unsafe { hfw_mesh_part(joined, 2) }.is_null(),
        "a third shell is refused"
    );
    assert!(last_error().contains("no shell 2"), "{}", last_error());
    unsafe {
        hfw_mesh_free(first);
        hfw_mesh_free(other);
        hfw_mesh_free(joined);
        hfw_mesh_free(moved);
        hfw_mesh_free(cube);
    }
}

#[test]
fn text_crosses_the_abi_and_engraving_removes_material() {
    let Some(font) = system_font() else {
        eprintln!("skipped: no TrueType font is installed");
        return;
    };
    let font_path = c_string(&font);
    let label = c_string("HFW");
    let text = unsafe { hfw_text_mesh(font_path.as_ptr(), 0, label.as_ptr(), 12.0, 0.8, 0.0) };
    assert!(!text.is_null(), "text builds: {}", last_error());
    let triangles = unsafe { hfw_mesh_triangle_count(text) };
    assert!(
        triangles > 100,
        "only {triangles} triangles in three letters"
    );
    let mut bounds = HfwBounds {
        min: [f64::NAN; 3],
        max: [f64::NAN; 3],
    };
    assert_eq!(unsafe { hfw_mesh_bounds(text, &mut bounds) }, 0);
    assert!(
        (bounds.max[2] - 0.8).abs() < 1e-6 && bounds.min[2].abs() < 1e-6,
        "the prism spans z 0 to 0.8: {:?}",
        bounds
    );
    assert!(
        bounds.max[0] - bounds.min[0] > 10.0,
        "three letters span more than 10 mm: {:?}",
        bounds
    );

    let empty = c_string("");
    assert!(
        unsafe { hfw_text_mesh(font_path.as_ptr(), 0, empty.as_ptr(), 12.0, 0.8, 0.0) }.is_null()
    );
    assert!(
        last_error().contains("empty"),
        "an empty string is refused by name: {}",
        last_error()
    );
    let missing = c_string("/nonexistent.ttf");
    assert!(
        unsafe { hfw_text_mesh(missing.as_ptr(), 0, label.as_ptr(), 12.0, 0.8, 0.0) }.is_null()
    );
    assert!(
        last_error().contains("nonexistent.ttf"),
        "a missing font is named in the refusal: {}",
        last_error()
    );

    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!cube.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(id, 0);
    let profile = c_string(&profile());
    let bare = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!bare.is_null(), "the bare slice failed: {}", last_error());
    let bare_text = unsafe { CStr::from_ptr(hfw_gcode_text(bare)) }
        .to_str()
        .unwrap()
        .to_owned();

    let sunk = unsafe { hfw_text_mesh(font_path.as_ptr(), 0, label.as_ptr(), 12.0, 0.8, 0.0) };
    assert_ne!(
        unsafe { hfw_plate_object_add_volume(plate, id, HFW_VOLUME_NEGATIVE, sunk) },
        0,
        "the text attaches as a negative volume: {}",
        last_error()
    );
    let engraved = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(
        !engraved.is_null(),
        "the engraved slice failed: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_gcode_verified(engraved) }, 1);
    let engraved_text = unsafe { CStr::from_ptr(hfw_gcode_text(engraved)) }
        .to_str()
        .unwrap()
        .to_owned();
    let before = extruded(&bare_text);
    let after = extruded(&engraved_text);
    println!(
        "engraving 3 letters removed {:.1} mm of filament from {:.1} mm",
        before - after,
        before
    );
    assert!(
        after < before - 1.0,
        "engraved text removed nothing: {before} mm before, {after} mm after"
    );

    unsafe {
        hfw_gcode_free(bare);
        hfw_gcode_free(engraved);
        hfw_mesh_free(sunk);
        hfw_mesh_free(text);
        hfw_mesh_free(cube);
        hfw_plate_free(plate);
    }
}

#[test]
fn a_modifier_volume_takes_settings_across_the_abi() {
    let plate = hfw_plate_new();
    let bytes = fs::read(fixture("cube.stl")).unwrap();
    let cube = unsafe { hfw_mesh_from_bytes(bytes.as_ptr(), bytes.len()) };
    assert!(!cube.is_null(), "a cube loads: {}", last_error());
    let id = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(id, 0, "the object is on the plate: {}", last_error());
    let modifier = unsafe { hfw_mesh_from_bytes(bytes.as_ptr(), bytes.len()) };
    assert!(!modifier.is_null(), "a second cube loads: {}", last_error());
    let volume = unsafe { hfw_plate_object_add_volume(plate, id, HFW_VOLUME_MODIFIER, modifier) };
    assert_ne!(volume, 0, "a modifier volume adds: {}", last_error());
    let mut kind = -1;
    assert_eq!(
        unsafe { hfw_plate_object_volume_kind(plate, id, volume, &mut kind) },
        0,
        "the kind reads back: {}",
        last_error()
    );
    assert_eq!(kind, HFW_VOLUME_MODIFIER);
    let key = c_string("sparse_infill_density");
    let value = c_string("0.9");
    assert_eq!(
        unsafe { hfw_plate_volume_set(plate, id, volume, key.as_ptr(), value.as_ptr()) },
        0,
        "a modifier takes its own settings: {}",
        last_error()
    );
    let unknown = c_string("no_such_setting");
    assert_eq!(
        unsafe { hfw_plate_volume_set(plate, id, volume, unknown.as_ptr(), value.as_ptr()) },
        -1,
        "an unknown key is refused"
    );
    unsafe {
        hfw_mesh_free(modifier);
        hfw_mesh_free(cube);
        hfw_plate_free(plate);
    }
}

#[test]
fn a_mesh_builds_from_arrays_with_its_triangle_order_kept() {
    let vertices: Vec<f64> = vec![
        0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 10.0, 0.0,
        10.0, 10.0, 10.0, 10.0, 0.0, 10.0, 10.0,
    ];
    let indices: Vec<u32> = vec![
        0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 1, 2, 6, 1, 6, 5, 2, 3, 7, 2, 7, 6,
        3, 0, 4, 3, 4, 7,
    ];
    let mesh = unsafe { hfw_mesh_new(vertices.as_ptr(), 8, indices.as_ptr(), 12) };
    assert!(!mesh.is_null(), "a cube builds: {}", last_error());
    assert_eq!(unsafe { hfw_mesh_triangle_count(mesh) }, 12);
    let volume = unsafe { hfw_mesh_volume(mesh) };
    assert!(
        (volume - 1000.0).abs() < 0.5,
        "a 10 mm cube is 1000 mm^3: {volume}"
    );
    let mut bounds = HfwBounds::default();
    assert_eq!(
        unsafe { hfw_mesh_bounds(mesh, &mut bounds) },
        0,
        "{}",
        last_error()
    );
    assert_eq!(bounds.max, [10.0, 10.0, 10.0]);
    assert!(
        unsafe { hfw_mesh_new(vertices.as_ptr(), 8, indices.as_ptr(), 0) }.is_null(),
        "a mesh with no triangles is refused"
    );
    unsafe { hfw_mesh_free(mesh) };
}

/// A plate handed to the engine comes back: every object with its name, its
/// placement, its own settings, the paint on it and the geometry underneath.
#[test]
fn a_plate_reads_back_the_document_it_was_given() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, mesh) };
    let name = c_string("painted bracket");
    assert_eq!(
        unsafe { hfw_plate_object_name(plate, id, name.as_ptr()) },
        0
    );
    let placement = HfwTransform {
        rotation_degrees: [0.0, 0.0, 30.0],
        translation_mm: [25.0, 35.0, 0.0],
        ..HfwTransform::default()
    };
    assert_eq!(
        unsafe { hfw_plate_object_placement(plate, id, placement) },
        0
    );
    let key = c_string("sparse_infill_density");
    let value = c_string("35%");
    assert_eq!(
        unsafe { hfw_plate_object_set(plate, id, key.as_ptr(), value.as_ptr()) },
        0,
        "{}",
        last_error()
    );
    let states = vec![1u8; unsafe { hfw_mesh_triangle_count(mesh) }];
    assert_eq!(
        unsafe {
            hfw_plate_object_paint(plate, id, HFW_PAINT_SUPPORT, states.as_ptr(), states.len())
        },
        0
    );

    let mut ids = [0u64; 4];
    assert_eq!(
        unsafe { hfw_plate_object_ids(plate, ids.as_mut_ptr(), ids.len()) },
        1
    );
    assert_eq!(ids[0], id);

    let mut written = vec![0 as c_char; 64];
    let needed = unsafe { hfw_plate_object_name_get(plate, id, written.as_mut_ptr(), 64) };
    assert_eq!(needed, "painted bracket".len() as isize);
    assert_eq!(
        unsafe { CStr::from_ptr(written.as_ptr()) }.to_str(),
        Ok("painted bracket")
    );
    let short = unsafe { hfw_plate_object_name_get(plate, id, written.as_mut_ptr(), 4) };
    assert_eq!(
        short, needed,
        "a buffer that is too short reports the length it needs"
    );

    let mut read_placement = HfwTransform::default();
    assert_eq!(
        unsafe { hfw_plate_object_placement_get(plate, id, &mut read_placement) },
        0
    );
    assert_eq!(read_placement.rotation_degrees, [0.0, 0.0, 30.0]);
    assert_eq!(read_placement.translation_mm, [25.0, 35.0, 0.0]);

    let mut enabled = 0;
    assert_eq!(
        unsafe { hfw_plate_object_enabled_get(plate, id, &mut enabled) },
        0
    );
    assert_eq!(enabled, 1);

    let settings = unsafe { hfw_plate_object_settings_json(plate, id) };
    assert!(!settings.is_null(), "{}", last_error());
    let settings = unsafe { CStr::from_ptr(settings) }
        .to_string_lossy()
        .into_owned();
    assert_eq!(settings, r#"{"sparse_infill_density":"35%"}"#);

    let geometry = unsafe { hfw_plate_object_mesh(plate, id) };
    assert!(!geometry.is_null(), "{}", last_error());
    let vertices = unsafe { hfw_mesh_vertex_count(geometry) };
    let indices = unsafe { hfw_mesh_index_count(geometry) };
    assert!(vertices > 0 && indices > 0);
    let mut xyz = vec![f64::NAN; vertices * 3];
    assert_eq!(
        unsafe { hfw_mesh_vertices(geometry, xyz.as_mut_ptr(), vertices) },
        0
    );
    assert!(xyz.iter().all(|value| value.is_finite()));
    let mut triangles = vec![0u32; indices * 3];
    assert_eq!(
        unsafe { hfw_mesh_indices(geometry, triangles.as_mut_ptr(), indices) },
        0
    );
    assert!(triangles.iter().all(|index| (*index as usize) < vertices));
    assert_eq!(
        unsafe { hfw_mesh_vertices(geometry, xyz.as_mut_ptr(), vertices - 1) },
        -1,
        "a short buffer is refused rather than half-written"
    );
    let mut states = vec![0u8; indices];
    assert_eq!(
        unsafe { hfw_mesh_paint_get(geometry, HFW_PAINT_SUPPORT, states.as_mut_ptr(), indices) },
        indices as isize,
        "the paint on the object comes back with its geometry"
    );
    assert!(states.iter().all(|state| *state == 1));

    unsafe { hfw_mesh_free(geometry) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(mesh) };
}

/// A volume's kind, geometry and settings come back with the object they belong
/// to, so a document's modifiers survive the trip.
#[test]
fn a_plate_reads_back_the_volumes_of_its_objects() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, mesh) };
    let volume = unsafe { hfw_plate_object_add_volume(plate, id, HFW_VOLUME_MODIFIER, mesh) };
    assert_ne!(volume, 0, "{}", last_error());
    let key = c_string("sparse_infill_density");
    let value = c_string("55%");
    assert_eq!(
        unsafe { hfw_plate_volume_set(plate, id, volume, key.as_ptr(), value.as_ptr()) },
        0,
        "{}",
        last_error()
    );

    let mut kind = -1;
    assert_eq!(
        unsafe { hfw_plate_object_volume_kind(plate, id, volume, &mut kind) },
        0
    );
    assert_eq!(kind, HFW_VOLUME_MODIFIER);
    let settings = unsafe { hfw_plate_object_volume_settings_json(plate, id, volume) };
    assert!(!settings.is_null(), "{}", last_error());
    assert_eq!(
        unsafe { CStr::from_ptr(settings) }.to_str(),
        Ok(r#"{"sparse_infill_density":"55%"}"#)
    );
    let geometry = unsafe { hfw_plate_object_volume_mesh(plate, id, volume) };
    assert!(!geometry.is_null(), "{}", last_error());
    assert_eq!(unsafe { hfw_mesh_triangle_count(geometry) }, unsafe {
        hfw_mesh_triangle_count(mesh)
    });
    assert!(unsafe { hfw_plate_object_volume_mesh(plate, id, 7) }.is_null());

    unsafe { hfw_mesh_free(geometry) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(mesh) };
}

/// The caller's own bytes travel with the plate: they are how the application keeps
/// its own state -- plates, names, tool colours -- in the same file as the document.
#[test]
fn a_plates_callers_own_bytes_survive_the_project() {
    let path = c_string(&fixture("cube.stl"));
    let mesh = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!mesh.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    unsafe { hfw_plate_add(plate, mesh) };

    let mut length = 1usize;
    assert!(unsafe { hfw_plate_metadata_get(plate, &mut length) }.is_null());
    assert_eq!(length, 0, "a plate given nothing carries none");

    let metadata = c_string(r#"{"plates":[{"name":"one"}]}"#);
    assert_eq!(
        unsafe { hfw_plate_set_metadata(plate, metadata.as_ptr(), metadata.as_bytes().len()) },
        0,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { hfw_plate_set_metadata(plate, std::ptr::null(), 4) },
        -1,
        "a null pointer with a length is refused"
    );

    let mut written = 0usize;
    let bytes = unsafe { hfw_plate_project_bytes(plate, &mut written) };
    assert!(!bytes.is_null(), "{}", last_error());
    let project = unsafe { std::slice::from_raw_parts(bytes as *const u8, written) }.to_vec();
    unsafe { hfw_buffer_free(bytes, written) };

    let reopened = unsafe {
        hfw_plate_open_project(
            project.as_ptr() as *const c_char,
            project.len(),
            std::ptr::null(),
        )
    };
    assert!(!reopened.is_null(), "{}", last_error());
    let mut length = 0usize;
    let metadata = unsafe { hfw_plate_metadata_get(reopened, &mut length) };
    assert!(!metadata.is_null(), "{}", last_error());
    assert_eq!(
        unsafe { std::slice::from_raw_parts(metadata as *const u8, length) },
        br#"{"plates":[{"name":"one"}]}"#
    );
    unsafe { hfw_string_free(metadata) };

    assert_eq!(
        unsafe { hfw_plate_set_metadata(reopened, std::ptr::null(), 0) },
        0
    );
    let mut length = 1usize;
    assert!(unsafe { hfw_plate_metadata_get(reopened, &mut length) }.is_null());
    assert_eq!(length, 0, "an empty write clears it");

    unsafe { hfw_plate_free(reopened) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(mesh) };
}

/// An undo step across the boundary: the plate as it was, put back as it was.
#[test]
fn a_snapshot_restores_the_document_through_the_abi() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!cube.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let first = unsafe { hfw_plate_add(plate, cube) };
    let second = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(first, 0);
    assert_ne!(second, 0);
    let name = c_string("bracket");
    assert_eq!(
        unsafe { hfw_plate_object_name(plate, second, name.as_ptr()) },
        0,
        "{}",
        last_error()
    );
    let key = c_string("sparse_infill_density");
    let value = c_string("55%");
    assert_eq!(
        unsafe { hfw_plate_object_set(plate, second, key.as_ptr(), value.as_ptr()) },
        0,
        "{}",
        last_error()
    );
    let modifier = unsafe { hfw_plate_object_add_volume(plate, second, HFW_VOLUME_MODIFIER, cube) };
    assert_ne!(modifier, 0, "{}", last_error());
    let metadata = c_string(r#"{"plates":[{"name":"one"}]}"#);
    assert_eq!(
        unsafe { hfw_plate_set_metadata(plate, metadata.as_ptr(), metadata.as_bytes().len()) },
        0,
        "{}",
        last_error()
    );

    let snapshot = unsafe { hfw_plate_snapshot(plate) };
    assert!(
        !snapshot.is_null(),
        "the snapshot is taken: {}",
        last_error()
    );
    assert_eq!(
        unsafe { hfw_snapshot_object_count(snapshot) },
        2,
        "the snapshot counts what was on the plate"
    );

    assert_eq!(
        unsafe { hfw_plate_object_remove(plate, second) },
        0,
        "{}",
        last_error()
    );
    let third = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(third, 0);
    assert_eq!(unsafe { hfw_plate_object_count(plate) }, 2);

    assert_eq!(
        unsafe { hfw_plate_restore(plate, snapshot) },
        0,
        "the snapshot restores: {}",
        last_error()
    );

    let mut ids = [0u64; 4];
    let count = unsafe { hfw_plate_object_ids(plate, ids.as_mut_ptr(), ids.len()) };
    assert_eq!(count, 2);
    assert_eq!(
        &ids[..2],
        &[first, second],
        "every id came back, and no other"
    );

    let mut text = vec![0i8; 64];
    assert_eq!(
        unsafe { hfw_plate_object_name_get(plate, second, text.as_mut_ptr(), text.len()) },
        7,
        "the name is written: {}",
        last_error()
    );
    assert_eq!(
        unsafe { CStr::from_ptr(text.as_ptr()) }.to_str(),
        Ok("bracket"),
        "the name came back with the object"
    );

    let settings = unsafe { hfw_plate_object_settings_json(plate, second) };
    assert!(!settings.is_null(), "{}", last_error());
    assert_eq!(
        unsafe { CStr::from_ptr(settings) }.to_str(),
        Ok(r#"{"sparse_infill_density":"55%"}"#),
        "the override came back with the object"
    );
    let mut kind = -1;
    assert_eq!(
        unsafe { hfw_plate_object_volume_kind(plate, second, modifier, &mut kind) },
        0,
        "the volume came back by the id it had: {}",
        last_error()
    );
    assert_eq!(kind, HFW_VOLUME_MODIFIER);

    let mut length = 0usize;
    let metadata = unsafe { hfw_plate_metadata_get(plate, &mut length) };
    assert!(!metadata.is_null(), "{}", last_error());
    assert_eq!(
        unsafe { std::slice::from_raw_parts(metadata.cast::<u8>(), length) },
        br#"{"plates":[{"name":"one"}]}"#,
        "the caller's own bytes came back"
    );

    assert_eq!(
        unsafe { hfw_plate_restore(plate, snapshot) },
        0,
        "the same snapshot restores again, which is what redo is"
    );
    assert_eq!(
        unsafe { hfw_plate_restore(plate, std::ptr::null()) },
        -1,
        "a null snapshot is refused rather than restoring nothing"
    );
    assert!(
        unsafe { hfw_plate_snapshot(std::ptr::null()) }.is_null(),
        "a null plate has no snapshot"
    );
    assert_eq!(unsafe { hfw_snapshot_object_count(std::ptr::null()) }, -1);

    unsafe { hfw_snapshot_free(snapshot) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
}

/// A snapshot owns its geometry, so it survives the plate it came from and restores
/// into another one.
#[test]
fn a_snapshot_outlives_the_plate_it_was_taken_from() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(id, 0);
    let snapshot = unsafe { hfw_plate_snapshot(plate) };
    assert!(!snapshot.is_null(), "{}", last_error());
    unsafe { hfw_mesh_free(cube) };
    unsafe { hfw_plate_free(plate) };

    let other = hfw_plate_new();
    assert_eq!(
        unsafe { hfw_plate_restore(other, snapshot) },
        0,
        "a snapshot restores into a plate that never held it: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_object_count(other) }, 1);
    let mut ids = [0u64; 2];
    let count = unsafe { hfw_plate_object_ids(other, ids.as_mut_ptr(), ids.len()) };
    assert_eq!(count, 1);
    assert_eq!(ids[0], id, "the id travels with the snapshot");
    let mut triangles = 0usize;
    let mesh = unsafe { hfw_plate_object_mesh(other, ids[0]) };
    assert!(
        !mesh.is_null(),
        "the geometry survives its plate: {}",
        last_error()
    );
    triangles += unsafe { hfw_mesh_triangle_count(mesh) };
    assert!(triangles > 0, "the restored object has its triangles");

    unsafe { hfw_mesh_free(mesh) };
    unsafe { hfw_snapshot_free(snapshot) };
    unsafe { hfw_plate_free(other) };
}

/// A document of several plates crosses the ABI: plates are indices, and an object
/// keeps its id when it moves between them.
#[test]
fn plates_cross_the_abi_with_their_names_locks_and_objects() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!cube.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    assert_eq!(unsafe { hfw_plate_bed_count(plate) }, 1);

    let first = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(first, 0, "{}", last_error());
    let second_bed = unsafe { hfw_plate_bed_add(plate) };
    assert_eq!(second_bed, 1, "a plate is appended: {}", last_error());
    assert_eq!(unsafe { hfw_plate_bed_count(plate) }, 2);

    let second = unsafe { hfw_plate_add_on_bed(plate, 1, cube) };
    assert_ne!(second, 0, "{}", last_error());
    assert_eq!(
        unsafe { hfw_plate_object_bed(plate, second) },
        1,
        "the object is on the plate it was added to"
    );

    let name = c_string("supports");
    assert_eq!(
        unsafe { hfw_plate_bed_name(plate, 1, name.as_ptr()) },
        0,
        "{}",
        last_error()
    );
    let mut text = vec![0i8; 32];
    assert_eq!(
        unsafe { hfw_plate_bed_name_get(plate, 1, text.as_mut_ptr(), text.len()) },
        8,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { CStr::from_ptr(text.as_ptr()) }.to_str(),
        Ok("supports")
    );
    assert_eq!(
        unsafe { hfw_plate_bed_name_get(plate, 0, text.as_mut_ptr(), text.len()) },
        0,
        "an unnamed plate reports an empty name"
    );

    let mut ids = [0u64; 4];
    assert_eq!(
        unsafe { hfw_plate_bed_object_ids(plate, 1, ids.as_mut_ptr(), ids.len()) },
        1,
        "the plate lists what is on it"
    );
    assert_eq!(ids[0], second);
    assert_eq!(
        unsafe { hfw_plate_bed_object_ids(plate, 0, ids.as_mut_ptr(), ids.len()) },
        1
    );
    assert_eq!(ids[0], first);

    assert_eq!(
        unsafe { hfw_plate_object_set_bed(plate, first, 1) },
        0,
        "an object moves between plates: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_object_bed(plate, first) }, 1);
    assert_eq!(
        unsafe { hfw_plate_object_bed(plate, 99) },
        -1,
        "an object that does not exist has no plate"
    );

    assert_eq!(unsafe { hfw_plate_bed_locked(plate, 0) }, 0);
    assert_eq!(
        unsafe { hfw_plate_bed_set_locked(plate, 0, 1) },
        0,
        "{}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_bed_locked(plate, 0) }, 1);
    assert_eq!(
        unsafe { hfw_plate_add_on_bed(plate, 0, cube) },
        0,
        "a locked plate takes nothing new"
    );
    assert!(
        last_error().contains("locked"),
        "the refusal says why: {}",
        last_error()
    );
    assert_eq!(
        unsafe { hfw_plate_object_set_bed(plate, second, 0) },
        -1,
        "an object cannot be moved onto a locked plate"
    );
    assert_eq!(
        unsafe { hfw_plate_bed_remove(plate, 0) },
        -1,
        "a locked plate cannot be removed"
    );
    assert_eq!(unsafe { hfw_plate_bed_set_locked(plate, 0, 0) }, 0);

    assert_eq!(
        unsafe { hfw_plate_bed_remove(plate, 0) },
        0,
        "the plate is removed: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_bed_count(plate) }, 1);
    assert_eq!(
        unsafe { hfw_plate_object_bed(plate, first) },
        0,
        "the plates after the removal moved down one"
    );
    assert_eq!(
        unsafe { hfw_plate_bed_remove(plate, 0) },
        -1,
        "a document always keeps one plate"
    );
    assert_eq!(unsafe { hfw_plate_bed_count(std::ptr::null()) }, -1);

    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
}

/// Each plate prints on its own, and the profile decides how.
#[test]
fn a_plate_slices_through_the_abi_and_carries_only_its_objects() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let first = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(first, 0);
    let bed = unsafe { hfw_plate_bed_add(plate) };
    assert_eq!(bed, 1);
    let second = unsafe { hfw_plate_add_on_bed(plate, bed as usize, cube) };
    assert_ne!(second, 0);

    let profile = c_string(&profile());
    let gcode = unsafe { hfw_plate_bed_slice(plate, 0, profile.as_ptr(), 0.2, 0.2) };
    assert!(!gcode.is_null(), "the plate slices: {}", last_error());
    assert_eq!(unsafe { hfw_gcode_verified(gcode) }, 1, "the file verifies");
    let text = unsafe { CStr::from_ptr(hfw_gcode_text(gcode)) }
        .to_str()
        .expect("the file is text")
        .to_owned();
    assert!(
        !text.is_empty(),
        "the file for one plate of two is not empty"
    );
    unsafe { hfw_gcode_free(gcode) };

    let alone = hfw_plate_new();
    assert_ne!(unsafe { hfw_plate_add(alone, cube) }, 0);
    let reference = unsafe { hfw_plate_slice(alone, profile.as_ptr(), 0.2, 0.2) };
    assert!(!reference.is_null(), "{}", last_error());
    let reference = unsafe { CStr::from_ptr(hfw_gcode_text(reference)) }
        .to_str()
        .expect("the file is text")
        .to_owned();
    assert_eq!(
        text, reference,
        "a plate's job carries its own objects and no other plate's"
    );

    assert!(
        unsafe { hfw_plate_bed_slice(plate, 9, profile.as_ptr(), 0.2, 0.2) }.is_null(),
        "a plate that does not exist is refused"
    );
    assert!(
        last_error().contains("no plate 10"),
        "the refusal says which: {}",
        last_error()
    );

    unsafe { hfw_plate_free(alone) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
}

/// A volume's name and visibility cross the ABI, and a hidden volume stops applying.
#[test]
fn a_volumes_name_and_visibility_cross_the_abi() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(id, 0);
    let volume = unsafe { hfw_plate_object_add_volume(plate, id, HFW_VOLUME_NEGATIVE, cube) };
    assert_ne!(volume, 0, "{}", last_error());

    assert_eq!(
        unsafe { hfw_plate_object_volume_visible(plate, id, volume) },
        1,
        "a volume is visible when it is attached"
    );
    let mut text = vec![0i8; 32];
    assert_eq!(
        unsafe {
            hfw_plate_object_volume_name_get(plate, id, volume, text.as_mut_ptr(), text.len())
        },
        0,
        "an unattached name reports an empty one: {}",
        last_error()
    );

    let name = c_string("the hole");
    assert_eq!(
        unsafe { hfw_plate_object_volume_name(plate, id, volume, name.as_ptr()) },
        0,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe {
            hfw_plate_object_volume_name_get(plate, id, volume, text.as_mut_ptr(), text.len())
        },
        8,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { CStr::from_ptr(text.as_ptr()) }.to_str(),
        Ok("the hole")
    );

    assert_eq!(
        unsafe { hfw_plate_object_volume_set_visible(plate, id, volume, 0) },
        0,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { hfw_plate_object_volume_visible(plate, id, volume) },
        0
    );

    let profile = c_string(&profile());
    let hidden = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!hidden.is_null(), "the plate slices: {}", last_error());
    let hidden = unsafe { CStr::from_ptr(hfw_gcode_text(hidden)) }
        .to_str()
        .expect("the file is text")
        .to_owned();
    assert_eq!(
        unsafe { hfw_plate_object_volume_set_visible(plate, id, volume, 1) },
        0
    );
    let shown = unsafe { hfw_plate_slice(plate, profile.as_ptr(), 0.2, 0.2) };
    assert!(!shown.is_null(), "{}", last_error());
    let shown = unsafe { CStr::from_ptr(hfw_gcode_text(shown)) }
        .to_str()
        .expect("the file is text")
        .to_owned();
    assert_ne!(
        hidden, shown,
        "hiding a negative volume did not change the file, so the control lies"
    );

    assert_eq!(
        unsafe { hfw_plate_object_volume_visible(plate, id, 9) },
        -1,
        "a volume that does not exist is refused"
    );

    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
}

/// An object's label, source path, dialect and kept foreign settings cross the ABI,
/// and a rebuild from a read-back is the same document.
#[test]
fn a_labels_path_dialect_and_refused_settings_cross_the_abi() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!cube.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(id, 0, "{}", last_error());

    assert_eq!(
        unsafe { hfw_plate_object_dialect(plate, id) },
        HFW_DIALECT_ENGINE,
        "an object is named the way this engine names things until told otherwise"
    );
    let label = c_string("left wing");
    assert_eq!(
        unsafe { hfw_plate_object_label(plate, id, label.as_ptr()) },
        0,
        "{}",
        last_error()
    );
    let source = c_string("/home/user/models/cube.stl");
    assert_eq!(
        unsafe { hfw_plate_object_source(plate, id, source.as_ptr()) },
        0,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { hfw_plate_object_set_dialect(plate, id, HFW_DIALECT_ORCA) },
        0,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { hfw_plate_object_set_dialect(plate, id, 7) },
        -1,
        "a dialect this engine does not know is refused"
    );

    let mut text = vec![0i8; 64];
    assert_eq!(
        unsafe { hfw_plate_object_label_get(plate, id, text.as_mut_ptr(), text.len()) },
        9,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { CStr::from_ptr(text.as_ptr()) }.to_str(),
        Ok("left wing")
    );
    assert_eq!(
        unsafe { hfw_plate_object_source_get(plate, id, text.as_mut_ptr(), text.len()) },
        26,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { CStr::from_ptr(text.as_ptr()) }.to_str(),
        Ok("/home/user/models/cube.stl")
    );
    assert_eq!(
        unsafe { hfw_plate_object_dialect(plate, id) },
        HFW_DIALECT_ORCA
    );

    let key = c_string("sparse_infill_density");
    let value = c_string("40%");
    assert_eq!(
        unsafe { hfw_plate_object_apply_foreign(plate, id, key.as_ptr(), value.as_ptr()) },
        0,
        "a setting this engine knows is applied: {}",
        last_error()
    );
    let refused = c_string("spaghetti_detector");
    assert_eq!(
        unsafe { hfw_plate_object_apply_foreign(plate, id, refused.as_ptr(), value.as_ptr()) },
        1,
        "a name it does not know is kept, not refused as an error"
    );
    let bad_value = c_string("walls");
    let bad = c_string("many");
    assert_eq!(
        unsafe { hfw_plate_object_apply_foreign(plate, id, bad_value.as_ptr(), bad.as_ptr()) },
        1,
        "a value it cannot take is kept too"
    );

    let json = unsafe { hfw_plate_object_foreign_json(plate, id) };
    assert!(!json.is_null(), "{}", last_error());
    let kept = unsafe { CStr::from_ptr(json) }
        .to_str()
        .expect("the JSON is text")
        .to_owned();
    assert_eq!(
        kept,
        r#"[{"key":"spaghetti_detector","value":"40%","reason":"unknown_name","note":"this engine has no setting by that name"},{"key":"walls","value":"many","reason":"unusable_value","note":"this engine does not accept that value"}]"#,
        "the kept settings carry their own text and why"
    );

    let again = hfw_plate_new();
    let copy = unsafe { hfw_plate_add(again, cube) };
    let copy_label = c_string("left wing");
    assert_eq!(
        unsafe { hfw_plate_object_label(again, copy, copy_label.as_ptr()) },
        0
    );
    for (name, text, reason) in [
        ("spaghetti_detector", "40%", HFW_FOREIGN_UNKNOWN_NAME),
        ("walls", "many", HFW_FOREIGN_UNUSABLE_VALUE),
    ] {
        let name = c_string(name);
        let value = c_string(text);
        assert_eq!(
            unsafe {
                hfw_plate_object_foreign_set(again, copy, name.as_ptr(), value.as_ptr(), reason)
            },
            0,
            "a kept setting re-attaches: {}",
            last_error()
        );
    }
    let rebuilt = unsafe { hfw_plate_object_foreign_json(again, copy) };
    assert_eq!(
        unsafe { CStr::from_ptr(rebuilt) }.to_str(),
        Ok(kept.as_str()),
        "a document rebuilt from what it read back keeps the same refusals"
    );
    assert_eq!(
        unsafe {
            let ignored = c_string("x");
            hfw_plate_object_foreign_set(again, copy, ignored.as_ptr(), ignored.as_ptr(), 9)
        },
        -1,
        "a reason this engine does not know is refused"
    );
    assert_eq!(
        unsafe { hfw_plate_object_foreign_clear(again, copy) },
        0,
        "{}",
        last_error()
    );
    let cleared = unsafe { hfw_plate_object_foreign_json(again, copy) };
    assert_eq!(
        unsafe { CStr::from_ptr(cleared) }.to_str(),
        Ok("[]"),
        "a re-import starts from nothing"
    );

    unsafe { hfw_string_free(json) };
    unsafe { hfw_string_free(rebuilt) };
    unsafe { hfw_string_free(cleared) };
    unsafe { hfw_plate_free(again) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
}

/// The label is what a caller shows, and the name is what a save writes back.
#[test]
fn a_label_sits_beside_the_name_through_a_project_round_trip() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, cube) };
    let name = c_string("cube.stl");
    assert_eq!(
        unsafe { hfw_plate_object_name(plate, id, name.as_ptr()) },
        0
    );
    let label = c_string("left wing");
    assert_eq!(
        unsafe { hfw_plate_object_label(plate, id, label.as_ptr()) },
        0
    );

    let mut length = 0usize;
    let bytes = unsafe { hfw_plate_project_bytes(plate, &mut length) };
    assert!(!bytes.is_null(), "save failed: {}", last_error());
    let reopened =
        unsafe { hfw_plate_open_project(bytes.cast(), length, c_string(&profile()).as_ptr()) };
    assert!(!reopened.is_null(), "open failed: {}", last_error());
    let mut text = vec![0i8; 32];
    assert_eq!(
        unsafe { hfw_plate_object_name_get(reopened, id, text.as_mut_ptr(), text.len()) },
        8,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { CStr::from_ptr(text.as_ptr()) }.to_str(),
        Ok("cube.stl")
    );
    assert_eq!(
        unsafe { hfw_plate_object_label_get(reopened, id, text.as_mut_ptr(), text.len()) },
        9,
        "{}",
        last_error()
    );
    assert_eq!(
        unsafe { CStr::from_ptr(text.as_ptr()) }.to_str(),
        Ok("left wing")
    );

    unsafe { hfw_buffer_free(bytes, length) };
    unsafe { hfw_plate_free(reopened) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
}

/// A saved document reopens with the ids it was saved with, so a selection, an undo
/// step or a paint session holding one still names the same object.
#[test]
fn a_project_keeps_object_ids_across_the_abi() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    let plate = hfw_plate_new();
    let first = unsafe { hfw_plate_add(plate, cube) };
    let middle = unsafe { hfw_plate_add(plate, cube) };
    let last = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(middle, 0);
    assert_eq!(
        unsafe { hfw_plate_object_remove(plate, middle) },
        0,
        "{}",
        last_error()
    );
    let mut ids = [0u64; 4];
    let count = unsafe { hfw_plate_object_ids(plate, ids.as_mut_ptr(), ids.len()) };
    assert_eq!(count, 2);

    let mut length = 0usize;
    let bytes = unsafe { hfw_plate_project_bytes(plate, &mut length) };
    assert!(!bytes.is_null(), "save failed: {}", last_error());
    let reopened =
        unsafe { hfw_plate_open_project(bytes.cast(), length, c_string(&profile()).as_ptr()) };
    assert!(!reopened.is_null(), "open failed: {}", last_error());
    let mut read = [0u64; 4];
    let count = unsafe { hfw_plate_object_ids(reopened, read.as_mut_ptr(), read.len()) };
    assert_eq!(count, 2);
    assert_eq!(
        &read[..2],
        &[first, last],
        "opening renumbered the objects, so every id the application holds is stale"
    );
    let invented = unsafe { hfw_plate_add(reopened, cube) };
    assert_ne!(invented, 0);
    assert!(
        invented != first && invented != last && invented != middle,
        "the reopened document handed out id {invented}, which was already used"
    );

    unsafe { hfw_buffer_free(bytes, length) };
    unsafe { hfw_plate_free(reopened) };
    unsafe { hfw_plate_free(plate) };
    unsafe { hfw_mesh_free(cube) };
}

/// A preset bundle crosses the ABI: its machines, one machine's profiles, and what a
/// selection resolves to.
#[test]
fn a_preset_bundle_answers_the_three_questions_a_preset_store_asks() {
    let root = std::env::temp_dir().join(format!("hfw-abi-bundle-{}", std::process::id()));
    let machine_dir = root.join("Vendor/machine");
    let process_dir = root.join("Vendor/process");
    let filament_dir = root.join("Vendor/filament");
    for dir in [&machine_dir, &process_dir, &filament_dir] {
        std::fs::create_dir_all(dir).expect("scratch bundle");
    }
    std::fs::write(
        machine_dir.join("voron.ini"),
        "[machine:Voron 2.4 350]\nbed_x = 350\nbed_y = 350\nmax_z = 330\nnozzle_diameter = 0.4\n",
    )
    .expect("machine preset");
    std::fs::write(
        process_dir.join("standard.ini"),
        "[process:0.20mm Standard]\nlayer_height = 0.2\nwalls = 4\ninfill_density = 0.35\n",
    )
    .expect("process preset");
    std::fs::write(
        filament_dir.join("pla.ini"),
        "[filament:Generic PLA]\nnozzle_temp = 210\n",
    )
    .expect("filament preset");

    let read = |pointer: *mut std::os::raw::c_char| -> String {
        assert!(!pointer.is_null(), "call failed: {}", last_error());
        unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .expect("the document is text")
            .to_owned()
    };

    let path = c_string(&root.display().to_string());
    let bundle = unsafe { hfw_bundle_open(path.as_ptr()) };
    assert!(!bundle.is_null(), "the bundle opens: {}", last_error());

    let printers = unsafe { hfw_bundle_printer_models(bundle) };
    let printers_json = read(printers);
    assert!(
        printers_json.contains("\"printer_models\""),
        "{printers_json}"
    );
    assert!(
        printers_json.contains("\"name\":\"Voron 2.4 350\""),
        "{printers_json}"
    );
    assert!(
        printers_json.contains("\"max_print_height\":330"),
        "{printers_json}"
    );

    let printer = c_string("Voron 2.4 350");
    let profiles = unsafe { hfw_bundle_profiles_for(bundle, printer.as_ptr()) };
    let profiles_json = read(profiles);
    assert!(
        profiles_json.contains("\"name\":\"0.20mm Standard\""),
        "{profiles_json}"
    );
    assert!(
        profiles_json.contains("\"filament_profiles\":[\"Generic PLA\"]"),
        "{profiles_json}"
    );

    let process = c_string("0.20mm Standard");
    let filament = c_string("Generic PLA");
    let filaments = [filament.as_ptr()];
    let resolved = unsafe {
        hfw_bundle_resolved_config(
            bundle,
            printer.as_ptr(),
            process.as_ptr(),
            filaments.as_ptr(),
            1,
        )
    };
    let resolved_json = read(resolved);
    assert!(
        resolved_json.contains("\"configuration\":{"),
        "{resolved_json}"
    );
    assert!(
        resolved_json.contains("\"layer_height\":0.2"),
        "{resolved_json}"
    );
    assert!(
        resolved_json.contains("\"perimeters\":4"),
        "{resolved_json}"
    );
    assert!(
        resolved_json.contains("\"fill_density\":{\"value\":35,\"is_percent\":true}"),
        "{resolved_json}"
    );
    assert!(
        resolved_json.contains("\"temperature\":[210]"),
        "{resolved_json}"
    );
    assert!(resolved_json.contains("\"warnings\":[]"), "{resolved_json}");

    let unknown = c_string("Prusa MK4");
    assert!(
        unsafe { hfw_bundle_profiles_for(bundle, unknown.as_ptr()) }.is_null(),
        "a machine the bundle does not hold is refused"
    );
    assert!(
        last_error().contains("Prusa MK4"),
        "the refusal names it: {}",
        last_error()
    );

    let summary = unsafe { hfw_bundle_summary(bundle) };
    let summary_text = read(summary);
    assert!(summary_text.contains("1 machine(s)"), "{summary_text}");

    std::fs::write(
        process_dir.join("draft.ini"),
        "[process:0.28mm Draft]\nlayer_height = 0.28\n",
    )
    .expect("a preset added while the process runs");
    assert_eq!(
        unsafe { hfw_bundle_reload(bundle) },
        0,
        "the bundle re-reads without a relaunch: {}",
        last_error()
    );
    let reloaded = unsafe { hfw_bundle_profiles_for(bundle, printer.as_ptr()) };
    let reloaded_json = read(reloaded);
    assert!(
        reloaded_json.contains("0.28mm Draft"),
        "a preset added while the process ran is in the list: {reloaded_json}"
    );

    unsafe { hfw_string_free(printers) };
    unsafe { hfw_string_free(profiles) };
    unsafe { hfw_string_free(resolved) };
    unsafe { hfw_string_free(summary) };
    unsafe { hfw_string_free(reloaded) };
    unsafe { hfw_bundle_free(bundle) };
    let _ = std::fs::remove_dir_all(&root);
    assert!(unsafe { hfw_bundle_open(std::ptr::null()) }.is_null());
    assert_eq!(unsafe { hfw_bundle_reload(std::ptr::null_mut()) }, -1);
}
