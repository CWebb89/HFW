//! The C surface, exercised the way a C caller would exercise it.

use std::ffi::{CStr, CString};
use std::fs;

use hfw_capi::{
    hfw_abi_version, hfw_buffer_free, hfw_gcode_free, hfw_gcode_len, hfw_gcode_text,
    hfw_gcode_verified, hfw_job_cancel, hfw_job_poll, hfw_job_start, hfw_job_take_gcode,
    hfw_job_wait, hfw_last_error, hfw_mesh_bounds, hfw_mesh_free, hfw_mesh_from_bytes,
    hfw_mesh_load_file, hfw_mesh_paint_get, hfw_mesh_paint_set, hfw_mesh_transform,
    hfw_mesh_triangle_count, hfw_mesh_volume, hfw_plate_add, hfw_plate_arrange, hfw_plate_bounds,
    hfw_plate_free, hfw_plate_new, hfw_plate_object_add_volume, hfw_plate_object_bounds,
    hfw_plate_object_clear_settings, hfw_plate_object_count, hfw_plate_object_duplicate,
    hfw_plate_object_enabled, hfw_plate_object_layer_heights, hfw_plate_object_layer_heights_get,
    hfw_plate_object_mirror, hfw_plate_object_name, hfw_plate_object_paint,
    hfw_plate_object_paint_get, hfw_plate_object_placement, hfw_plate_object_remove,
    hfw_plate_object_remove_volume, hfw_plate_object_rotate, hfw_plate_object_set,
    hfw_plate_object_volume_count, hfw_plate_object_volume_kind, hfw_plate_open_project,
    hfw_plate_preview_json, hfw_plate_project_bytes, hfw_plate_slice, hfw_slice_meshes,
    hfw_string_free, HfwBounds, HfwJobProgress, HfwTransform, HFW_JOB_CANCELLED, HFW_JOB_DONE,
    HFW_JOB_STAGE_FINISHED, HFW_VOLUME_NEGATIVE,
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
    assert_eq!(
        text.matches("\"disposition\": \"out_of_scope\"").count(),
        6,
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
fn a_volume_crosses_the_abi_and_its_kind_reads_back() {
    let path = c_string(&fixture("cube.stl"));
    let cube = unsafe { hfw_mesh_load_file(path.as_ptr()) };
    assert!(!cube.is_null(), "load failed: {}", last_error());
    let plate = hfw_plate_new();
    let id = unsafe { hfw_plate_add(plate, cube) };
    assert_ne!(id, 0);
    assert_eq!(unsafe { hfw_plate_object_volume_count(plate, id) }, 0);

    let cutter = unsafe { hfw_mesh_from_bytes(std::ptr::null(), 0) };
    assert!(cutter.is_null(), "an empty mesh is refused");

    let face_path = c_string(&fixture("cube.stl"));
    let cutter = unsafe { hfw_mesh_load_file(face_path.as_ptr()) };
    assert_eq!(
        unsafe { hfw_plate_object_add_volume(plate, id, HFW_VOLUME_NEGATIVE, cutter) },
        0,
        "the volume attaches: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_object_volume_count(plate, id) }, 1);

    let mut kind = -1;
    assert_eq!(
        unsafe { hfw_plate_object_volume_kind(plate, id, 0, &mut kind) },
        0
    );
    assert_eq!(kind, HFW_VOLUME_NEGATIVE, "the kind reads back");

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
        -1,
        "an unknown kind is refused"
    );
    assert!(
        last_error().contains("volume kind 7 does not exist"),
        "the refusal lists the kinds: {}",
        last_error()
    );

    assert_eq!(
        unsafe { hfw_plate_object_remove_volume(plate, id, 0) },
        0,
        "the volume detaches: {}",
        last_error()
    );
    assert_eq!(unsafe { hfw_plate_object_volume_count(plate, id) }, 0);

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
