//! The plate-file transport: a document the app writes becomes the G-code a printer
//! runs, with paint, volumes, per-object heights, the timeline and the in-layer strokes
//! all still in it.

use std::path::{Path, PathBuf};
use std::process::Command;

use hfw_engine::{InLayerPolyline, Marker, MarkerKind, Mesh, Settings, VolumeKind, V2};
use hfw_plate::Plate;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cube() -> Mesh {
    Mesh::load_model(repo().join("fixtures/cube.stl")).expect("the fixture loads")
}

fn project(dir: &Path) -> PathBuf {
    let mut plate = Plate::new(Settings::default());
    let id = plate.add("cube", cube()).expect("a cube adds");
    let settings = plate.settings().clone();
    let object = plate.object_mut(id).expect("the object is there");
    let modifier = object
        .add_volume(
            VolumeKind::Modifier,
            Mesh::box_mm(8.0, 8.0, 20.0).expect("a box builds"),
        )
        .expect("a modifier volume adds");
    object
        .set_volume_setting(modifier, "sparse_infill_density", "0.9", &settings)
        .expect("the modifier takes settings");
    object
        .add_volume(
            VolumeKind::Negative,
            Mesh::box_mm(3.0, 3.0, 20.0).expect("a box builds"),
        )
        .expect("a negative volume adds");
    object
        .set_layer_heights(vec![(10.0, 0.1)])
        .expect("the object takes its own layer heights");
    object
        .set_paint(hfw_engine::Paint {
            support: vec![2; object.mesh().triangle_count()],
            seam: Vec::new(),
        })
        .expect("the object takes paint");
    plate
        .set_markers(vec![Marker {
            z: 5.0,
            kind: MarkerKind::Pause,
            text: String::new(),
        }])
        .expect("the plate takes a marker");
    plate
        .set_in_layer_polylines(vec![InLayerPolyline {
            z: 0.3,
            points: vec![V2::new(30.0, 30.0), V2::new(40.0, 30.0)],
            line_width: 0.5,
            layer_height: 0.3,
            speed: 20.0,
            prefix: "M104 S240".to_owned(),
        }])
        .expect("the plate takes a stroke");
    let path = dir.join("plate.hfw");
    std::fs::write(&path, plate.to_project().expect("the project writes")).expect("it lands");
    path
}

#[test]
fn a_plate_file_becomes_a_print_file_with_what_it_carries() {
    let dir = std::env::temp_dir().join(format!("hfw-plate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp directory");
    let project = project(&dir);
    let gcode = dir.join("plate.gcode");
    let output = Command::new(env!("CARGO_BIN_EXE_hfw-cli"))
        .arg("--plate")
        .arg(&project)
        .arg("--profile")
        .arg(repo().join("profiles/voron_klipper.cfg"))
        .arg("--layer-height")
        .arg("0.2")
        .arg("--first-layer-height")
        .arg("0.3")
        .arg("--gcode")
        .arg(&gcode)
        .output()
        .expect("the binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the run fails: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 modifier, 1 negative"),
        "the report says what the file carries: {stdout}"
    );
    assert!(
        stdout.contains("paint support on"),
        "the report names the paint: {stdout}"
    );
    assert!(
        stdout.contains("verify RESULT PASS"),
        "the written file passes the independent check: {stdout}"
    );
    let written = std::fs::read_to_string(&gcode).expect("the G-code lands");
    assert_eq!(
        written.matches(";TYPE:Custom").count(),
        1,
        "the plate's in-layer stroke is printed as a bead"
    );
    assert!(
        written.contains("M104 S240"),
        "and its own G-code runs before it"
    );
    assert!(
        written.contains("pause requested at Z5.000"),
        "the plate's timeline reaches the file"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_plate_without_gcode_is_refused() {
    let output = Command::new(env!("CARGO_BIN_EXE_hfw-cli"))
        .args(["--plate", "nothing.hfw", "--layer-height", "0.2"])
        .output()
        .expect("the binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("--plate needs --gcode"),
        "the refusal says what is missing: {stderr}"
    );
}
