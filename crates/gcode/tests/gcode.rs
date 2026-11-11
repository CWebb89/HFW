//! G-code writer and verifier checks over the shared fixtures.

use std::path::PathBuf;

use hfw_config::PrinterConfig;
use hfw_engine::{run, InfillPattern, Job, Mesh, Settings};
use hfw_gcode::{generate, verify, Gcode, GcodeError};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn profile(name: &str) -> PrinterConfig {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../profiles")
        .join(name);
    PrinterConfig::load(&path).expect("profile loads")
}

fn voron() -> PrinterConfig {
    profile("voron_klipper.cfg")
}

fn settings(cfg: &PrinterConfig) -> Settings {
    let mut s = Settings::new(cfg.line_width, cfg.walls);
    s.keep_reference = true;
    s.every = 1;
    if let Some(ip) = s.infill.as_mut() {
        ip.density = cfg.infill_density;
        ip.top_layers = cfg.top_layers;
        ip.bottom_layers = cfg.bottom_layers;
        ip.pattern = if cfg.infill_pattern == "grid" {
            InfillPattern::Grid
        } else {
            InfillPattern::Rectilinear
        };
    }
    s
}

fn plan(name: &str, cfg: &PrinterConfig) -> Job {
    let mesh = Mesh::load_stl(fixture(name)).expect("fixture loads");
    run(
        &mesh,
        cfg.first_layer_height,
        cfg.layer_height,
        &settings(cfg),
    )
    .expect("job runs")
}

fn make(name: &str, cfg: &PrinterConfig) -> (Gcode, Job) {
    let job = plan(name, cfg);
    let gcode = generate(&job.layers, cfg, Some("part")).expect("gcode builds");
    (gcode, job)
}

fn first_pos(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"))
}

fn feeds(section: &str) -> Vec<f64> {
    section
        .lines()
        .filter(|l| l.starts_with("G1 X"))
        .filter_map(|l| {
            l.split_whitespace()
                .find(|t| t.starts_with('F'))
                .and_then(|t| t[1..].parse::<f64>().ok())
        })
        .collect()
}

fn layer_section(text: &str, index: usize) -> &str {
    let start = first_pos(text, &format!(";LAYER:{index}\n"));
    match text[start + 1..].find(&format!(";LAYER:{}\n", index + 1)) {
        Some(offset) => &text[start..start + 1 + offset],
        None => &text[start..],
    }
}

#[test]
fn shipped_profiles_parse() {
    profile("voron_klipper.cfg");
    profile("generic_marlin.cfg");
}

#[test]
fn config_rejects_bad_input() {
    let parse = |text: &str| PrinterConfig::parse(text).unwrap_err().to_string();
    assert!(parse("bed_x = abc").contains("needs a number"));
    assert!(parse("bogus_setting = 1").contains("unknown setting"));
    assert!(parse("start_gcode = <<X\nG28").contains("never closed"));
    assert!(parse("nozzle_temp = 500").contains("150 and 320"));
    assert!(parse("layer_height = 0.5").contains("80%"));
    assert!(parse("line_width = 0.1").contains("line_width"));
    assert!(parse("bed_x = 0").contains("greater than zero"));
    assert!(parse("infill_density = 2").contains("between 0 and 1"));
    assert!(parse("walls = 0").contains("between 1 and 8"));
    assert!(parse("seam = sideways").contains("aligned, nearest"));
    assert!(parse("min_bead_width = 0.9").contains("line_width"));
}

#[test]
fn cube_gcode_passes_verification_and_is_centered() {
    let cfg = voron();
    let (gcode, job) = make("cube.stl", &cfg);
    let report = verify(&gcode.text, &cfg);
    assert!(report.ok(), "{:?}", report.errors);
    assert_eq!(report.layers, job.layers.len());
    assert_eq!(gcode.stats.layers, job.layers.len());
    assert!((report.first_extrusion_z - cfg.first_layer_height).abs() < 1e-9);
    let center_x = 0.5 * (report.bbox_min[0] + report.bbox_max[0]);
    let center_y = 0.5 * (report.bbox_min[1] + report.bbox_max[1]);
    assert!((center_x - 0.5 * cfg.bed_x).abs() < 0.05, "x {center_x}");
    assert!((center_y - 0.5 * cfg.bed_y).abs() < 0.05, "y {center_y}");
    assert!((report.bbox_max[2] - 20.0).abs() < 1e-9);
    assert!(gcode.text.contains("M83"));
    assert!(gcode.text.contains("PRINT_START BED=60 EXTRUDER=215"));
    assert!(gcode.text.contains("SET_PRESSURE_ADVANCE ADVANCE=0.045"));
}

#[test]
fn every_layer_planned_covers_the_model() {
    let mut cfg = voron();
    cfg.infill_density = 1.0;
    cfg.flow = 1.0;
    cfg.skirt_loops = 0;
    let (gcode, job) = make("cube.stl", &cfg);
    let planned = job.volume.0 + job.volume.1;
    let report = verify(&gcode.text, &cfg);
    assert!(report.ok(), "{:?}", report.errors);
    let error = (report.volume_mm3 - planned).abs() / planned;
    assert!(
        error < 0.01,
        "{} vs {} ({error})",
        report.volume_mm3,
        planned
    );
    assert!((report.volume_mm3 / 8000.0 - 0.9963).abs() < 0.01);
}

#[test]
fn gcode_volume_matches_verifier_in_both_extruder_modes() {
    let baseline = {
        let mut cfg = voron();
        cfg.flow = 1.0;
        cfg.skirt_loops = 0;
        let (gcode, _) = make("sphere.stl", &cfg);
        verify(&gcode.text, &cfg).volume_mm3
    };
    let mut cfg = voron();
    cfg.flow = 1.0;
    cfg.skirt_loops = 0;
    cfg.relative_e = false;
    let (gcode, job) = make("sphere.stl", &cfg);
    let report = verify(&gcode.text, &cfg);
    assert!(report.ok(), "{:?}", report.errors);
    let planned = job.volume.0 + job.volume.1;
    assert!(
        (report.volume_mm3 - planned).abs() / planned < 0.003,
        "{} vs {planned}",
        report.volume_mm3
    );
    assert!((report.volume_mm3 - baseline).abs() / baseline < 0.001);
}

#[test]
fn heaters_before_motion_and_fan_after_first_layers() {
    let cfg = profile("generic_marlin.cfg");
    let (gcode, _) = make("cube.stl", &cfg);
    let text = &gcode.text;
    assert!(first_pos(text, "G28") < first_pos(text, "M109 S215"));
    assert!(first_pos(text, "M109 S215") < first_pos(text, "G1 X"));
    assert!(first_pos(text, "M106") > first_pos(text, ";LAYER:2"));
    assert!(first_pos(text, "M104 S210") > first_pos(text, ";LAYER:1"));
    assert!(text.trim_end().ends_with("M84"));
    assert!(text.contains("M104 S0") && text.contains("M107"));
}

#[test]
fn first_layer_is_slower_than_later_layers() {
    let mut cfg = voron();
    cfg.min_layer_time = 0.0;
    let (gcode, _) = make("cube.stl", &cfg);
    let first = feeds(layer_section(&gcode.text, 0));
    let later = feeds(layer_section(&gcode.text, 5));
    let max_first = first.iter().cloned().fold(0.0, f64::max);
    let max_later = later.iter().cloned().fold(0.0, f64::max);
    assert!(max_first <= cfg.first_layer_speed * 60.0 + 1e-6);
    assert!(max_later > max_first * 2.0);
}

#[test]
fn short_layers_are_slowed_for_cooling() {
    let mut cfg = voron();
    cfg.min_layer_time = 10_000.0;
    cfg.min_speed_factor = 0.3;
    let (gcode, _) = make("cube.stl", &cfg);
    let later = feeds(layer_section(&gcode.text, 5));
    assert!(!later.is_empty());
    let ceiling = cfg.infill_speed * 60.0 * 0.31 + 1.0;
    assert!(later.iter().all(|&f| f <= ceiling), "{:?}", &later[..]);
}

#[test]
fn retractions_are_balanced_and_bounded() {
    let cfg = voron();
    let (gcode, _) = make("sphere.stl", &cfg);
    assert!(gcode.stats.retractions > 0);
    let mut net = 0.0;
    for line in gcode.text.lines() {
        if let Some(rest) = line.strip_prefix("G1 E") {
            let value: f64 = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .expect("retraction value");
            assert!(value.abs() <= cfg.retract_len + 1e-9);
            net += value;
        }
    }
    assert!(net.abs() <= cfg.retract_len + 1e-6, "net {net}");
}

#[test]
fn oversized_and_too_tall_models_are_refused() {
    let mut small = voron();
    small.bed_x = 10.0;
    small.bed_y = 10.0;
    let job = plan("cube.stl", &small);
    match generate(&job.layers, &small, None) {
        Err(GcodeError::DoesNotFit { .. }) => {}
        Err(other) => panic!("expected DoesNotFit, got {other}"),
        Ok(g) => panic!("expected DoesNotFit, got {} bytes of G-code", g.text.len()),
    }
    let mut short = voron();
    short.max_z = 5.0;
    let job = plan("cube.stl", &short);
    match generate(&job.layers, &short, None) {
        Err(GcodeError::TooTall { .. }) => {}
        Err(other) => panic!("expected TooTall, got {other}"),
        Ok(g) => panic!("expected TooTall, got {} bytes of G-code", g.text.len()),
    }
}

#[test]
fn tapered_beads_never_go_below_the_minimum_width() {
    let cfg = voron();
    let (gcode, _) = make("taper_wall.stl", &cfg);
    let report = verify(&gcode.text, &cfg);
    assert!(report.ok(), "{:?}", report.errors);
    let mut widths = 0;
    for line in gcode.text.lines() {
        if let Some(value) = line.strip_prefix(";WIDTH:") {
            let width: f64 = value.parse().expect("width value");
            assert!(width >= cfg.min_bead_width - 1e-3, "width {width}");
            widths += 1;
        }
    }
    assert!(widths > 0);
}

#[test]
fn gcode_is_deterministic() {
    let cfg = voron();
    let (a, _) = make("torus.stl", &cfg);
    let (b, _) = make("torus.stl", &cfg);
    assert_eq!(a.text, b.text);
}

#[test]
fn origin_center_profiles_place_the_part_on_the_origin() {
    let mut cfg = voron();
    cfg.origin_center = true;
    let (gcode, _) = make("cube.stl", &cfg);
    let report = verify(&gcode.text, &cfg);
    assert!(report.ok(), "{:?}", report.errors);
    assert!((0.5 * (report.bbox_min[0] + report.bbox_max[0])).abs() < 0.05);
    assert!((0.5 * (report.bbox_min[1] + report.bbox_max[1])).abs() < 0.05);
}

#[test]
fn exclude_object_is_emitted_when_enabled() {
    let mut cfg = voron();
    cfg.exclude_object = true;
    let job = plan("cube.stl", &cfg);
    let gcode = generate(&job.layers, &cfg, Some("my part")).expect("gcode builds");
    assert!(gcode.text.contains("EXCLUDE_OBJECT_DEFINE NAME=MY_PART"));
    let plain = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(!plain.text.contains("EXCLUDE_OBJECT_DEFINE"));
}

const GOOD_HEAD: &str = ";LAYER_CHANGE\n;HEIGHT:0.2\nM140 S60\nM109 S210\nG90\nM83\nG92 E0\nG1 Z0.2 F600\nG0 X50 Y50 F3000\n";

fn check(extra: &str) -> hfw_gcode::VerifyReport {
    let cfg = voron();
    verify(&format!("{GOOD_HEAD}{extra}"), &cfg)
}

#[test]
fn verifier_accepts_a_minimal_good_file() {
    let report = check("G0 X100 Y100 F12000\nG1 X110 Y100 E0.33 F3000\nM104 S0\nM106 S255\n");
    assert!(report.ok(), "{:?}", report.errors);
}

#[test]
fn verifier_rejects_unsafe_files() {
    let cfg = voron();
    let cold = verify(
        ";LAYER_CHANGE\n;HEIGHT:0.2\nG90\nM83\nG1 Z0.2 F600\nG0 X100 Y100\nG1 X110 Y100 E0.33 F3000\n",
        &cfg,
    );
    assert!(cold
        .errors
        .iter()
        .any(|e| e.contains("before the nozzle was heated")));
    assert!(check("G0 X999 Y100 F3000\n")
        .errors
        .iter()
        .any(|e| e.contains("outside the bed")));
    assert!(check("G0 X100 Y100 F99999\n")
        .errors
        .iter()
        .any(|e| e.contains("above max_speed")));
    assert!(check("G1 Z500 F600\n")
        .errors
        .iter()
        .any(|e| e.contains("Z 500")));
    let flow = check("G1 X51 Y50 E1 F3000\n");
    assert!(flow.errors.iter().any(|e| e.contains("volumetric flow")));
    assert!(flow.errors.iter().any(|e| e.contains("implausible")));
    assert!(
        check("G1 X100 Y100 E0.0001 F3000\nG1 X101 Y100 E0.0001 F3000\n")
            .errors
            .iter()
            .any(|e| e.contains("implausible"))
    );
    assert!(
        check("G1 X100 Y100 E0.3 F3000\nG1 Z0.2 F600\nG1 X100 Y100 E0.3 F3000\n")
            .errors
            .iter()
            .all(|e| !e.contains("NaN"))
    );
    assert!(
        check("G1 X100 Y100 E0.3 F3000\nG1 Z0.1 F600\nG1 X120 Y100 E0.3 F3000\n")
            .errors
            .iter()
            .any(|e| e.contains("below an earlier layer"))
    );
    assert!(check("G1 Xnan Y100 F3000\n")
        .errors
        .iter()
        .any(|e| e.contains("non-finite") || e.contains("unreadable")));
    assert!(check("G1 E-50 F3000\n")
        .errors
        .iter()
        .any(|e| e.contains("single filament move")));
    assert!(check("M104 S400\n")
        .errors
        .iter()
        .any(|e| e.contains("outside 150..320")));
    assert!(check("M140 S200\n")
        .errors
        .iter()
        .any(|e| e.contains("above 130")));
    assert!(verify("", &cfg).errors.len() >= 2);
}

#[test]
fn verifier_tracks_relative_moves() {
    let report = check("G91\nG1 X60 F3000\nG1 X10 E0.33 F3000\nG90\nM104 S0\n");
    assert!(report.ok(), "{:?}", report.errors);
    assert!((report.bbox_max[0] - 120.0).abs() < 1e-6);
}
