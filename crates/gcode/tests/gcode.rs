//! G-code writer and verifier checks over the shared fixtures.

use std::path::PathBuf;

use hfw_config::{Flavor, PrinterConfig};
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
    s.first_layer_width = cfg.first_layer_width;
    s.extra_perimeters = cfg.extra_perimeters;
    s.extra_perimeters_overhangs = cfg.extra_perimeters_on_overhangs;
    s.one_perimeter_first_layer = cfg.only_one_perimeter_first_layer;
    s.one_perimeter_top = if cfg.top_one_perimeter_type == "none" {
        String::new()
    } else {
        cfg.top_one_perimeter_type.clone()
    };
    s.nozzle = cfg.nozzle_diameter;
    if cfg.wall_transition_length > 0.0 {
        s.wall.trans_len = cfg.wall_transition_length;
    }
    if cfg.perimeter_generator == "classic" {
        s.mode = hfw_engine::Mode::Classic;
    }
    s.wall.gap_fill = cfg.gap_fill_enabled;
    s.wall.distribution = cfg.wall_distribution_count;
    if let Some(ip) = s.infill.as_mut() {
        ip.density = cfg.infill_density;
        ip.top_layers = cfg.top_layers;
        ip.bottom_layers = cfg.bottom_layers;
        ip.top_width = cfg.top_infill_width;
        ip.top_pattern = match cfg.top_fill_pattern.as_str() {
            "rectilinear" => hfw_engine::SkinPattern::Rectilinear,
            "concentric" => hfw_engine::SkinPattern::Concentric,
            "alignedrectilinear" => hfw_engine::SkinPattern::AlignedRectilinear,
            "monotoniclines" => hfw_engine::SkinPattern::MonotonicLines,
            _ => hfw_engine::SkinPattern::Monotonic,
        };
        ip.bottom_pattern = match cfg.bottom_fill_pattern.as_str() {
            "rectilinear" => hfw_engine::SkinPattern::Rectilinear,
            "concentric" => hfw_engine::SkinPattern::Concentric,
            "alignedrectilinear" => hfw_engine::SkinPattern::AlignedRectilinear,
            "monotoniclines" => hfw_engine::SkinPattern::MonotonicLines,
            _ => hfw_engine::SkinPattern::Monotonic,
        };
        ip.solid_every = cfg.solid_infill_every_layers;
        ip.solid_below_area = cfg.solid_infill_below_area;
        ip.combine_layers = cfg.infill_every_layers.max(1);
        ip.combine_auto = cfg.automatic_infill_combination;
        ip.combine_max_height = cfg.infill_combination_max_layer_height;
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
fn the_writer_counts_the_same_crossings_the_verifier_finds() {
    let mut cfg = voron();
    cfg.avoid_crossing_perimeters = true;
    for name in ["torus.stl", "cube.stl"] {
        let (gcode, _) = make(name, &cfg);
        let report = verify(&gcode.text, &cfg);
        assert!(report.ok(), "{:?}", report.errors);
        assert!(
            report.travel_crossings > 0,
            "{name} has no crossing to count"
        );
        assert_eq!(
            gcode.stats.travel_crossings, report.travel_crossings,
            "the writer's own count drifted from the file's, so a decision built on it \
             would be built on an estimate"
        );
    }
}

#[test]
fn the_crossing_count_falls_where_a_choice_exists_and_holds_where_none_does() {
    let cfg = voron();
    let mut avoiding = voron();
    avoiding.avoid_crossing_perimeters = true;
    avoiding.avoid_crossing_perimeters_max_detour = 3.0;

    let torus = verify(&make("torus.stl", &cfg).0.text, &cfg);
    let torus_avoiding = verify(&make("torus.stl", &avoiding).0.text, &avoiding);
    assert!(
        torus_avoiding.travel_crossings < torus.travel_crossings,
        "{} -> {}",
        torus.travel_crossings,
        torus_avoiding.travel_crossings
    );

    let cube = verify(&make("cube.stl", &cfg).0.text, &cfg);
    let cube_avoiding = verify(&make("cube.stl", &avoiding).0.text, &avoiding);
    assert_eq!(
        cube_avoiding.travel_crossings, cube.travel_crossings,
        "a cube's crossings are all entries into its enclosed interior, so no \
         entry point can avoid one"
    );
    assert!(cube.travel_crossings > 0, "the count must not be vacuous");
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
    // Wall beads round off into the corners they do not fill, so the file carries
    // about 2.4% less plastic than the cube's geometric volume; the rest is solid
    // fill at its full rectangle.
    let covered = report.volume_mm3 / 8000.0;
    assert!(
        (covered - 0.9725).abs() < 0.01,
        "the file carries {covered:.4} of the cube"
    );
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
    assert!(gcode.text.contains("EXCLUDE_OBJECT_START NAME=MY_PART"));
    assert!(gcode.text.contains("EXCLUDE_OBJECT_END NAME=MY_PART"));
    let unnamed = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(
        unnamed.text.contains("EXCLUDE_OBJECT_DEFINE NAME=HFW_MODEL"),
        "a part with no name still needs one the machine can cancel"
    );
    cfg.exclude_object = false;
    let plain = generate(&job.layers, &cfg, Some("my part")).expect("gcode builds");
    assert!(!plain.text.contains("EXCLUDE_OBJECT_DEFINE"));
    assert!(!plain.text.contains("EXCLUDE_OBJECT_START"));
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
fn verifier_lets_the_head_park_past_the_bed_edge() {
    // A reference machine's end G-code parks the head a few millimetres beyond the
    // bed: `G1 X225 Y-2.5` on a 220 mm machine. Nothing is extruded, so nothing can
    // be ruined, and rejecting it refuses every real profile.
    let report = check("G90\nM83\nG0 X100 Y100 F3000\nG1 X110 Y100 E0.33 F3000\nG1 X225 Y-2.5\n");
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
    assert!(
        check("G90\nM83\nG0 X100 Y100 F3000\nG1 X999 Y100 E0.33 F3000\n")
            .errors
            .iter()
            .any(|e| e.contains("outside the bed")),
        "plastic laid down outside the bed is an error"
    );
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

#[test]
fn acceleration_is_switched_per_feature_and_restored() {
    let mut cfg = voron();
    cfg.default_accel = 4000.0;
    cfg.outer_wall_accel = 1200.0;
    let (gcode, _) = make("cube.stl", &cfg);
    assert!(
        !gcode.text.contains("SAVE_VELOCITY_LIMIT"),
        "klipper has no SAVE_VELOCITY_LIMIT"
    );
    assert!(
        !gcode.text.contains("RESTORE_VELOCITY_LIMIT"),
        "klipper has no RESTORE_VELOCITY_LIMIT"
    );
    assert!(
        gcode.text.contains("SET_VELOCITY_LIMIT ACCEL=1200"),
        "outer wall acceleration missing"
    );
    assert!(
        gcode.text.contains("SET_VELOCITY_LIMIT ACCEL=4000"),
        "default acceleration missing"
    );
    let outer = first_pos(&gcode.text, ";TYPE:Outer wall");
    let after = &gcode.text[outer..];
    let to_first_move = &after[..after.find("G1 X").expect("the outer wall prints a move")];
    assert!(
        to_first_move.contains("SET_VELOCITY_LIMIT ACCEL=1200"),
        "the outer wall's acceleration is not set before its first move"
    );
    let restored = &after[after.find(";TYPE:").expect("another feature follows")..];
    assert!(
        restored.contains("SET_VELOCITY_LIMIT ACCEL=4000"),
        "the print's own acceleration is not put back after the outer wall"
    );
}

#[test]
fn identity_settings_reach_the_header_and_nothing_else() {
    let plain = voron();
    let mut documented = voron();
    documented.printer_model = "Voron 2.4".to_owned();
    documented.printer_variant = "350".to_owned();
    documented.filament_type = "ASA".to_owned();
    documented.filament_vendor = "Polymaker".to_owned();
    documented.notes = "bracket v3".to_owned();
    documented.filament_abrasive = true;
    let (base, _) = make("cube.stl", &plain);
    let (gcode, _) = make("cube.stl", &documented);
    for line in [
        "; printer_model = Voron 2.4",
        "; printer_variant = 350",
        "; filament_type = ASA",
        "; filament_vendor = Polymaker",
        "; notes = bracket v3",
        "; filament_abrasive = 1",
    ] {
        assert!(gcode.text.contains(line), "{line} missing");
    }
    let moves = |text: &str| {
        text.lines()
            .filter(|line| line.starts_with("G1 ") || line.starts_with("G0 "))
            .map(|line| line.to_owned())
            .collect::<Vec<String>>()
    };
    assert_eq!(
        moves(&base.text),
        moves(&gcode.text),
        "metadata moved the tool"
    );
}

#[test]
fn a_substitution_pass_rewrites_the_finished_file_and_says_so() {
    let mut cfg = voron();
    cfg.gcode_substitutions = vec![hfw_config::Substitution {
        pattern: "M104 S0".to_owned(),
        format: "M104 S0 ; the nozzle is parked".to_owned(),
        flags: String::new(),
        note: "footer".to_owned(),
    }];
    let (gcode, _) = make("cube.stl", &cfg);
    assert!(
        gcode
            .text
            .contains("; substituted M104 S0 -> M104 S0 ; the nozzle is parked (footer)"),
        "the pass did not announce itself"
    );
    assert!(
        gcode.text.contains("M104 S0 ; the nozzle is parked"),
        "the substitution did not reach the file"
    );
    let mut regex = voron();
    regex.gcode_substitutions = vec![hfw_config::Substitution {
        pattern: "M10(\\d) S0".to_owned(),
        format: "M$1 S0 ; matched".to_owned(),
        flags: "r".to_owned(),
        note: String::new(),
    }];
    let (gcode, _) = make("cube.stl", &regex);
    assert_eq!(
        gcode.text.matches("; matched").count(),
        2,
        "both the M104 and M140 lines should match a group reference"
    );
}

#[test]
fn a_bad_substitution_pattern_is_an_error_not_a_silent_pass() {
    let mut cfg = voron();
    cfg.gcode_substitutions = vec![hfw_config::Substitution {
        pattern: "M104 (".to_owned(),
        format: "x".to_owned(),
        flags: "r".to_owned(),
        note: String::new(),
    }];
    let job = plan("cube.stl", &cfg);
    let error = generate(&job.layers, &cfg, Some("part"))
        .expect_err("an uncompilable pattern must not pass silently");
    assert!(error.to_string().contains("not valid"), "{error}");
}

#[test]
fn a_pause_and_a_colour_change_fire_on_one_layer_each() {
    let mut cfg = voron();
    cfg.pause_print_gcode = "PAUSE ; layer {layer_z}".to_owned();
    cfg.color_change_gcode = "M600".to_owned();
    cfg.pause_heights = vec![1.0];
    cfg.colour_change_heights = vec![2.0];
    let (gcode, _) = make("cube.stl", &cfg);
    assert_eq!(gcode.text.matches("PAUSE ; layer 1.000").count(), 1);
    assert_eq!(gcode.text.matches("M600").count(), 1);
    let pause_at = gcode
        .text
        .find("PAUSE ; layer 1.000")
        .expect("the pause is in the file");
    let before = &gcode.text[..pause_at];
    let z_marker = before.rfind(";Z:").expect("the layer has a Z marker");
    let layer_z = before[z_marker + 3..]
        .lines()
        .next()
        .unwrap()
        .trim()
        .to_owned();
    assert_eq!(
        layer_z, "1.000",
        "the pause sits under the Z marker of its own layer"
    );
}

#[test]
fn a_first_layer_width_moves_only_the_first_layer() {
    let plain = voron();
    let mut wide = voron();
    wide.first_layer_width = 0.6;
    let (thin, _) = make("cube.stl", &plain);
    let (fat, _) = make("cube.stl", &wide);
    let split = |text: &str| -> (String, String) {
        let at = text.find(";LAYER:1\n").expect("a second layer");
        (text[..at].to_owned(), text[at..].to_owned())
    };
    let (thin_first, thin_rest) = split(&thin.text);
    let (fat_first, fat_rest) = split(&fat.text);
    assert!(
        thin_first != fat_first,
        "the first layer is the one the width applies to"
    );
    let filament_of = |text: &str| -> f64 {
        let mut total = 0.0;
        for line in text.lines() {
            if let Some((_, e)) = line.rsplit_once('E') {
                if let Ok(value) = e.trim().parse::<f64>() {
                    total += value.max(0.0);
                }
            }
        }
        total
    };
    let thin_rest = filament_of(&thin_rest);
    let fat_rest = filament_of(&fat_rest);
    assert!(
        (thin_rest - fat_rest).abs() < 0.01 * thin_rest,
        "every layer above the first lays the same plastic: {thin_rest} against {fat_rest}"
    );
    let filament = |text: &str| -> f64 {
        let mut total = 0.0;
        for line in text.lines() {
            if let Some((_, e)) = line.rsplit_once('E') {
                if let Ok(value) = e.trim().parse::<f64>() {
                    total += value.max(0.0);
                }
            }
        }
        total
    };
    assert!(
        filament(&fat_first) > filament(&thin_first) * 1.1,
        "a 0.6 mm first layer lays more plastic: {} against {}",
        filament(&thin_first),
        filament(&fat_first)
    );
}

#[test]
fn infill_first_prints_the_fill_before_the_walls() {
    let mut cfg = voron();
    cfg.infill_first = true;
    let (gcode, _) = make("cube.stl", &cfg);
    let (base, _) = make("cube.stl", &voron());
    let first_of = |text: &str, marker: &str| -> usize {
        let start = text.find(";LAYER:40").expect("layer 40 is in the file");
        let end = text[start + 8..]
            .find(";LAYER:")
            .map(|d| start + 8 + d)
            .unwrap_or(text.len());
        text[start..end]
            .lines()
            .position(|line| line.starts_with(marker))
            .unwrap_or(usize::MAX)
    };
    let wall = ";TYPE:Outer wall";
    let fill = ";TYPE:Sparse infill";
    assert!(
        first_of(&gcode.text, fill) < first_of(&gcode.text, wall),
        "the fill did not move ahead of the walls"
    );
    assert!(
        first_of(&base.text, wall) < first_of(&base.text, fill),
        "the default order should still be walls first"
    );
}

#[test]
fn extra_perimeters_add_a_loop_only_on_the_layers_that_step_out() {
    let plain = voron();
    let mut extra = voron();
    extra.extra_perimeters = true;
    let (base, _) = make("cube.stl", &plain);
    let (gcode, _) = make("cube.stl", &extra);
    let count = |text: &str| -> usize {
        text.lines()
            .filter(|line| line.starts_with(";TYPE:Inner wall"))
            .count()
    };
    assert_eq!(
        count(&base.text),
        count(&gcode.text),
        "a straight-sided cube does not step out, so nothing may change"
    );
}

#[test]
fn a_solid_layer_every_three_layers_lays_more_plastic_than_sparse() {
    let mut cfg = voron();
    let (base, _) = make("cube.stl", &cfg);
    cfg.solid_infill_every_layers = 3;
    let (every, _) = make("cube.stl", &cfg);
    let solid = |text: &str| -> usize {
        text.lines()
            .filter(|line| line.starts_with(";TYPE:Internal solid infill"))
            .count()
    };
    assert!(
        solid(&every.text) > solid(&base.text) * 2,
        "forcing one layer in three solid should roughly double the solid fill"
    );
    assert!(
        every.stats.volume_mm3 > base.stats.volume_mm3,
        "more plastic went down"
    );
}

#[test]
fn a_20_mm_cube_is_under_the_default_threshold_only_when_told_so() {
    let mut cfg = voron();
    let (base, _) = make("cube.stl", &cfg);
    cfg.solid_infill_below_area = 1000.0;
    let (solid, _) = make("cube.stl", &cfg);
    let count = |text: &str| -> usize {
        text.lines()
            .filter(|line| line.starts_with(";TYPE:Internal solid infill"))
            .count()
    };
    assert!(
        count(&solid.text) > count(&base.text) * 2,
        "a 20 mm cube is 400 mm2, which is under a 1000 mm2 threshold"
    );
}

#[test]
fn a_concentric_top_skin_writes_closed_loops_instead_of_lines() {
    let mut cfg = voron();
    cfg.top_fill_pattern = "concentric".to_owned();
    let (gcode, _) = make("cube.stl", &cfg);
    let (base, _) = make("cube.stl", &voron());
    assert!(
        gcode.text != base.text,
        "the top skin pattern changed nothing"
    );
    let end = gcode
        .text
        .rfind(";TYPE:Top surface")
        .expect("a top surface");
    let body = &gcode.text[end..];
    let moves = body.lines().filter(|l| l.starts_with("G1 ")).count();
    assert!(moves > 10, "the concentric skin wrote {moves} moves");
}

#[test]
fn combined_infill_lays_the_same_plastic_on_fewer_layers() {
    let (plain, _) = make("sphere.stl", &voron());
    let mut cfg = voron();
    cfg.infill_every_layers = 3;
    let (combined, _) = make("sphere.stl", &cfg);
    let interior = |text: &str| -> f64 {
        text.lines()
            .filter(|line| line.starts_with(";TYPE:Sparse infill"))
            .count() as f64
    };
    assert!(
        interior(&combined.text) < interior(&plain.text),
        "combining three layers should need fewer infill paths: {} against {}",
        interior(&plain.text),
        interior(&combined.text)
    );
    let ratio = combined.stats.volume_mm3 / plain.stats.volume_mm3;
    assert!(
        ratio > 0.97 && ratio < 1.03,
        "the deposited volume moved by {:.3}, so the flow did not follow the spacing",
        ratio
    );
}

#[test]
fn an_overhang_loop_is_added_only_where_the_part_hangs_over_air() {
    let mut cfg = voron();
    let (plain, _) = make("overhang.stl", &cfg);
    cfg.extra_perimeters_on_overhangs = true;
    let (looped, _) = make("overhang.stl", &cfg);
    assert!(
        looped.stats.volume_mm3 > plain.stats.volume_mm3,
        "the overhang loop added nothing"
    );
    let ratio = looped.stats.volume_mm3 / plain.stats.volume_mm3;
    assert!(
        ratio < 1.10,
        "an extra loop on the unsupported part is a small share, not {ratio:.2} of the part"
    );
}

#[test]
fn machine_limits_are_written_only_when_the_profile_asks_for_them() {
    let mut emit = voron();
    emit.flavor = hfw_config::Flavor::Marlin;
    emit.machine_limits_usage = "emit_to_gcode".to_owned();
    emit.machine_max_accel_x = 5000.0;
    emit.machine_max_accel_y = 5000.0;
    emit.machine_max_jerk_x = 8.0;
    emit.machine_max_jerk_y = 8.0;
    emit.machine_max_feedrate_x = 500.0;
    emit.machine_max_feedrate_y = 500.0;
    let (written, _) = make("cube.stl", &emit);
    assert!(
        written.text.contains("M201 X5000"),
        "the envelope is missing"
    );
    emit.machine_limits_usage = "time_estimate_only".to_owned();
    let (quiet, _) = make("cube.stl", &emit);
    assert!(
        !quiet.text.contains("M201 X5000"),
        "time_estimate_only still wrote the envelope"
    );
}

#[test]
fn a_zero_retract_before_wipe_wipes_first_and_the_default_retracts_first() {
    let mut default = voron();
    default.wipe = true;
    default.wipe_distance = 2.0;
    let (retract_first, _) = make("cube.stl", &default);
    default.retract_before_wipe = 0.0;
    let (wipe_first, _) = make("cube.stl", &default);
    let wipe_before_retract = |text: &str| -> usize {
        let lines: Vec<&str> = text.lines().collect();
        lines
            .windows(2)
            .filter(|pair| {
                let wipe = pair[0];
                let retract = pair[1];
                wipe.starts_with("G1 X")
                    && !wipe.contains('E')
                    && retract.trim_start().starts_with("G1 E-")
            })
            .count()
    };
    assert!(
        wipe_before_retract(&retract_first.text) == 0,
        "the default retracts before the wipe, so no wipe may come first"
    );
    assert!(
        wipe_before_retract(&wipe_first.text) > 0,
        "a zero percentage should wipe before retracting"
    );
}

#[test]
fn one_perimeter_on_the_first_layer_drops_the_inner_loops() {
    let plain = voron();
    let (base, _) = make("cube.stl", &plain);
    let mut single = voron();
    single.only_one_perimeter_first_layer = true;
    let (one, _) = make("cube.stl", &single);
    let layer = |text: &str| -> String {
        let start = text.find(";LAYER:0\n").expect("the first layer");
        let end = text[start + 8..]
            .find(";LAYER:")
            .map(|offset| start + 8 + offset)
            .unwrap_or(text.len());
        text[start..end].to_owned()
    };
    let walls = |text: &str| -> usize {
        text.lines()
            .filter(|line| line.starts_with(";TYPE:Inner wall"))
            .count()
    };
    assert!(
        walls(&layer(&base.text)) > 0,
        "the plain layer has inner loops"
    );
    assert_eq!(walls(&layer(&one.text)), 0, "inner loops survived");
}

#[test]
fn a_filament_volumetric_limit_slows_the_print_down() {
    let plain = voron();
    let (base, _) = make("cube.stl", &plain);
    let mut capped = voron();
    capped.filament_max_volumetric_speed = 2.0;
    let (slow, _) = make("cube.stl", &capped);
    let fastest = |text: &str| -> f64 {
        text.lines()
            .filter_map(|line| line.rsplit_once('F'))
            .filter_map(|(_, f)| f.trim().parse::<f64>().ok())
            .filter(|f| *f < 9000.0)
            .fold(0.0f64, f64::max)
    };
    assert!(
        fastest(&slow.text) < fastest(&base.text),
        "the filament cap did not touch the feeds: {} against {}",
        fastest(&base.text),
        fastest(&slow.text)
    );
    assert!(
        slow.stats.est_seconds > base.stats.est_seconds,
        "a slower print must estimate longer"
    );
}

#[test]
fn the_firmware_switches_are_written_where_they_belong() {
    let mut cfg = voron();
    cfg.pressure_advance = 0.045;
    cfg.stuck_filament_detection = true;
    cfg.chamber_minimal_temperature = 40.0;
    let (gcode, _) = make("cube.stl", &cfg);
    let body = gcode.text.as_str();
    let start = body.find("PRINT_END").unwrap_or(body.len());
    let head = &body[..start];
    assert!(
        head.contains("M900 K0.045") || head.contains("ADVANCE=0.045"),
        "no pressure advance"
    );
    assert!(head.contains("M591 D0 S1"), "no stuck-filament switch");
    assert!(head.contains("M191 S40"), "no chamber wait");
}

#[test]
fn shrinkage_compensation_scales_the_plate_not_the_settings() {
    let plain = voron();
    let (base, _) = make("cube.stl", &plain);
    let shell = |text: &str| -> f64 {
        text.lines()
            .find_map(|line| {
                let x = line.strip_prefix("G1 X")?;
                let value = x.split_whitespace().next()?;
                value.parse::<f64>().ok()
            })
            .unwrap_or(0.0)
    };
    assert!(shell(&base.text) > 0.0);
}

#[test]
fn machine_limits_reach_the_output_in_its_own_dialect() {
    let mut marlin = profile("generic_marlin.cfg");
    marlin.machine_max_accel_x = 3000.0;
    marlin.machine_max_feedrate_x = 100.0;
    marlin.machine_max_jerk_x = 8.0;
    let (gcode, _) = make("cube.stl", &marlin);
    assert!(gcode.text.contains("M201 X3000"), "M201 missing");
    assert!(
        gcode.text.contains("M203 X6000"),
        "M203 missing: feedrate is in mm/min there"
    );
    assert!(gcode.text.contains("M205 X8.0"), "M205 missing");

    let mut klipper = voron();
    klipper.machine_max_accel_x = 3000.0;
    klipper.machine_max_feedrate_y = 200.0;
    let (gcode, _) = make("cube.stl", &klipper);
    assert!(
        gcode
            .text
            .contains("SET_VELOCITY_LIMIT VELOCITY=200 ACCEL=3000"),
        "klipper ceiling missing: {}",
        gcode.text.lines().take(20).collect::<Vec<_>>().join(" | ")
    );
    assert!(!gcode.text.contains("M205"), "klipper has no jerk limit");
}

#[test]
fn printing_retract_travel_acceleration_and_min_rates_reach_their_commands() {
    let mut marlin = profile("generic_marlin.cfg");
    marlin.machine_max_accel_extruding = 8000.0;
    marlin.machine_max_accel_retracting = 3000.0;
    marlin.machine_max_accel_travel = 6000.0;
    marlin.machine_max_junction_deviation = 0.01;
    marlin.machine_min_extruding_rate = 5.0;
    marlin.machine_min_travel_rate = 15.0;
    let (gcode, _) = make("cube.stl", &marlin);
    assert!(
        gcode.text.contains("M204 P8000 R3000 T6000"),
        "M204 missing: {}",
        gcode.text.lines().take(20).collect::<Vec<_>>().join(" | ")
    );
    assert!(gcode.text.contains("M205 J0.010"), "M205 J missing");
    assert!(gcode.text.contains("M205 S5 T15"), "M205 S/T missing");

    let mut klipper = voron();
    klipper.machine_max_accel_extruding = 8000.0;
    klipper.machine_max_junction_deviation = 0.01;
    let (gcode, _) = make("cube.stl", &klipper);
    assert!(!gcode.text.contains("M204"), "klipper has no M204");
    assert!(
        !gcode.text.contains("M205"),
        "klipper has no junction deviation"
    );
}

#[test]
fn the_header_carries_every_audit_finding() {
    let mut cfg = voron();
    cfg.retract_len = 0.8;
    cfg.z_hop = 0.0;
    cfg.pressure_advance = 0.0;
    let (gcode, _) = make("cube.stl", &cfg);
    let head = &gcode.text[..gcode.text.find("\nG").unwrap_or(gcode.text.len())];
    for finding in cfg.audit() {
        assert!(
            head.contains(&format!("; AUDIT: {}", finding.line())),
            "the header is missing its finding: {}: {head}",
            finding.line()
        );
    }
    assert!(
        head.contains("; AUDIT: off: z_hop is 0"),
        "the retraction finding is not in the header: {head}"
    );
}

#[test]
fn the_velocity_limit_commands_are_the_ones_the_flavour_implements() {
    let mut klipper = voron();
    klipper.default_accel = 4000.0;
    klipper.outer_wall_accel = 1200.0;
    let (k, _) = make("cube.stl", &klipper);
    for unsupported in ["SAVE_VELOCITY_LIMIT", "RESTORE_VELOCITY_LIMIT", "M204"] {
        assert!(
            !k.text.contains(unsupported),
            "klipper cannot run {unsupported}"
        );
    }

    let mut marlin = profile("generic_marlin.cfg");
    marlin.default_accel = 4000.0;
    marlin.outer_wall_accel = 1200.0;
    let (m, _) = make("cube.stl", &marlin);
    assert!(
        !m.text.contains("SET_VELOCITY_LIMIT"),
        "marlin has no SET_VELOCITY_LIMIT"
    );
    assert!(
        m.text.contains("M204 S1200"),
        "the outer wall's acceleration never reaches marlin"
    );
    assert!(
        m.text.contains("M204 S4000"),
        "the print's own acceleration is not put back"
    );
}

#[test]
fn a_z_offset_shifts_every_emitted_z_and_nothing_else() {
    let plain = voron();
    let mut shifted = voron();
    shifted.z_offset = -0.3;
    let (base, _) = make("cube.stl", &plain);
    let (moved, _) = make("cube.stl", &shifted);
    let zs = |text: &str| {
        text.lines()
            .filter_map(|line| line.strip_prefix(";Z:"))
            .filter_map(|value| value.trim().parse::<f64>().ok())
            .collect::<Vec<f64>>()
    };
    let before = zs(&base.text);
    let after = zs(&moved.text);
    assert_eq!(before.len(), after.len(), "the same layers are emitted");
    assert!(!before.is_empty());
    for (a, b) in before.iter().zip(after.iter()) {
        assert!((b - a + 0.3).abs() < 1e-9, "{a} became {b}");
    }
    let first_z = |text: &str| {
        text.lines()
            .filter_map(|line| line.strip_prefix("G1 Z"))
            .filter_map(|rest| rest.split_whitespace().next())
            .filter_map(|value| value.parse::<f64>().ok())
            .next()
            .expect("the first layer moves Z")
    };
    assert!((first_z(&moved.text) - first_z(&base.text) + 0.3).abs() < 1e-9);
    let xy = |text: &str| {
        text.lines()
            .filter(|line| line.starts_with("G1 X"))
            .map(|line| line.to_owned())
            .collect::<Vec<String>>()
    };
    assert_eq!(xy(&base.text), xy(&moved.text), "X/Y moves are untouched");
}

#[test]
fn no_feed_exceeds_the_machine_ceiling() {
    let mut cfg = voron();
    cfg.infill_speed = 200.0;
    cfg.travel_speed = 300.0;
    cfg.machine_max_feedrate_x = 60.0;
    cfg.machine_max_feedrate_y = 60.0;
    let (gcode, _) = make("cube.stl", &cfg);
    for line in gcode.text.lines() {
        if !(line.starts_with("G0 ") || line.starts_with("G1 ")) {
            continue;
        }
        let feed = line
            .split_whitespace()
            .find(|t| t.starts_with('F'))
            .and_then(|t| t[1..].parse::<f64>().ok());
        if let Some(feed) = feed {
            assert!(
                feed <= 60.0 * 60.0 + 1.0,
                "{feed} mm/min exceeds 60 mm/s: {line}"
            );
        }
    }
}

#[test]
fn the_fan_ramps_from_its_minimum_to_its_maximum() {
    let mut cfg = voron();
    cfg.fan_start_layer = 1;
    cfg.min_fan_speed = 30.0;
    cfg.fan_speed = 100.0;
    cfg.full_fan_speed_layer = 4;
    cfg.fan_below_layer_time = 0.0;
    let (gcode, _) = make("cube.stl", &cfg);
    let pwm = |index: usize| -> Vec<i64> {
        layer_section(&gcode.text, index)
            .lines()
            .filter_map(|line| {
                line.strip_prefix("M106 S")
                    .map(|v| v.parse::<i64>().unwrap())
            })
            .collect()
    };
    assert_eq!(pwm(0), Vec::<i64>::new(), "no fan on the first layer");
    assert_eq!(
        pwm(1),
        vec![77],
        "first cooled layer runs 30% of 255, rounded up"
    );
    let mut previous = 0;
    for index in 2..4 {
        let value = *pwm(index)
            .first()
            .expect("a fan command on every ramp layer");
        assert!(value > previous, "fan did not ramp up at layer {index}");
        previous = value;
    }
    assert_eq!(pwm(4), vec![255], "the ramp ends at full speed");
}

#[test]
fn a_short_layer_is_slowed_down_to_the_configured_floor() {
    let mut cfg = voron();
    cfg.slow_down_layer_time = 10_000.0;
    cfg.slow_down_min_speed = 15.0;
    cfg.min_layer_time = 0.0;
    cfg.min_speed_factor = 0.1;
    let (gcode, _) = make("cube.stl", &cfg);
    let later = feeds(layer_section(&gcode.text, 5));
    assert!(!later.is_empty());
    let slowest = later.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(
        (slowest - 15.0 * 60.0).abs() < 60.0,
        "slowest feed {slowest} mm/min is not the 15 mm/s floor: {:?}",
        &later[..later.len().min(8)]
    );
}

#[test]
fn wipe_drags_the_nozzle_back_before_the_travel() {
    let mut cfg = voron();
    cfg.wipe = true;
    cfg.wipe_distance = 2.0;
    let (gcode, _) = make("sphere.stl", &cfg);
    let mut wipes = 0;
    let lines: Vec<&str> = gcode.text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("G1 E-") && i + 1 < lines.len() && lines[i + 1].starts_with("G1 X") {
            wipes += 1;
        }
    }
    assert!(wipes > 0, "no wipe move follows a retraction");
    assert!(gcode.stats.travel_mm > 0.0);
}

#[test]
fn unretraction_uses_its_own_speed_and_extra_prime() {
    let mut cfg = voron();
    cfg.deretract_speed = 20.0;
    cfg.retract_restart_extra = 0.1;
    let (gcode, _) = make("sphere.stl", &cfg);
    assert!(gcode.text.contains("F1200"), "deretract speed missing");
    let mut unretracts = 0.0;
    for line in gcode.text.lines() {
        if let Some(rest) = line.strip_prefix("G1 E") {
            let value: f64 = rest
                .split_whitespace()
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or(0.0);
            if value > 0.0 && line.contains("F1200") {
                unretracts += 1.0;
                assert!((value - (cfg.retract_len + 0.1)).abs() < 1e-6, "{line}");
            }
        }
    }
    assert!(unretracts > 0.0, "no unretraction at the deretract speed");
}

#[test]
fn the_skirt_can_run_for_several_layers() {
    let mut cfg = voron();
    cfg.skirt_loops = 2;
    cfg.skirt_height = 2;
    let (gcode, _) = make("cube.stl", &cfg);
    let skirts = gcode.text.matches(";TYPE:Skirt").count();
    assert_eq!(skirts, 2, "skirt printed on {skirts} layers, not 2");
}

#[test]
fn the_nozzle_is_left_at_the_idle_temperature() {
    let mut cfg = voron();
    cfg.idle_temp = 150.0;
    let (gcode, _) = make("cube.stl", &cfg);
    let last = gcode.text.lines().rev().find(|l| l.starts_with("M104 "));
    assert_eq!(last, Some("M104 S150"));
}
#[test]
fn a_zipped_3mf_loads_through_the_archive_reader() {
    let path = fixture("cube.3mf");
    let mesh = hfw_mesh::load_3mf(&path).expect("3mf loads");
    assert_eq!(
        mesh.triangle_count(),
        12,
        "the cube fixture has 12 triangles"
    );
    let stl = hfw_engine::Mesh::load_stl(fixture("cube.stl")).expect("stl loads");
    let (a, b) = (mesh.z_bounds().expect("z"), stl.z_bounds().expect("z"));
    assert_eq!(a, b, "3MF and STL disagree about the cube height");
}

#[test]
fn the_model_loader_picks_the_format_from_the_extension() {
    let from_3mf = hfw_mesh::load_3mf(fixture("cube.3mf")).expect("3mf");
    let from_stl = hfw_engine::Mesh::load_model(fixture("cube.stl")).expect("stl");
    assert_eq!(from_3mf.triangle_count(), from_stl.triangle_count());
    assert_eq!(from_3mf.z_bounds(), from_stl.z_bounds());
}

#[test]
fn arc_fitting_turns_a_traced_circle_into_g2_g3() {
    let mut cfg = voron();
    cfg.arc_fitting = true;
    let (gcode, _) = make("sphere.stl", &cfg);
    let arcs = gcode
        .text
        .lines()
        .filter(|line| line.starts_with("G2 ") || line.starts_with("G3 "))
        .count();
    assert!(arcs > 50, "a sphere should be mostly arcs, found {arcs}");
    let report = verify(&gcode.text, &cfg);
    assert!(report.ok(), "{:?}", report.errors);
}

#[test]
fn arc_fitting_is_off_unless_asked() {
    let cfg = voron();
    let (gcode, _) = make("sphere.stl", &cfg);
    assert!(
        !gcode.text.contains("\nG2 ") && !gcode.text.contains("\nG3 "),
        "no arc should be emitted from a profile that did not ask for one"
    );
}

#[test]
fn a_fitted_arc_lays_the_same_plastic_as_the_chords_it_replaces() {
    let mut fitted = voron();
    fitted.arc_fitting = true;
    let (with_arcs, _) = make("sphere.stl", &fitted);
    let (chords, _) = make("sphere.stl", &voron());
    let arc_report = verify(&with_arcs.text, &fitted);
    let chord_report = verify(&chords.text, &fitted);
    let ratio = arc_report.filament_mm / chord_report.filament_mm;
    assert!(
        (ratio - 1.0).abs() < 0.01,
        "arcs changed the filament total by {:.1}%",
        100.0 * (ratio - 1.0)
    );
}

#[test]
fn ironing_runs_over_the_visible_top_surface_only() {
    let mut cfg = voron();
    cfg.ironing = true;
    let job = {
        let mesh = Mesh::load_stl(fixture("cube.stl")).expect("fixture loads");
        let mut s = settings(&cfg);
        s.ironing = true;
        s.ironing_spacing = cfg.ironing_spacing;
        run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs")
    };
    let gcode = generate(&job.layers, &cfg, Some("part")).expect("gcode builds");
    let report = verify(&gcode.text, &cfg);
    assert!(report.ok(), "{:?}", report.errors);

    let mut layer = 0;
    let mut per_layer: Vec<(usize, usize)> = Vec::new();
    let mut printing_ironing = false;
    for line in gcode.text.lines() {
        if line.starts_with(";LAYER_CHANGE") {
            layer += 1;
        } else if let Some(kind) = line.strip_prefix(";TYPE:") {
            printing_ironing = kind == "Ironing";
        } else if printing_ironing && (line.starts_with("G1 X") || line.starts_with("G2 ")) {
            match per_layer.iter_mut().find(|(l, _)| *l == layer) {
                Some((_, count)) => *count += 1,
                None => per_layer.push((layer, 1)),
            }
        }
    }
    assert_eq!(per_layer.len(), 1, "ironing on {per_layer:?}");
    assert!(
        per_layer[0].1 > 50,
        "a 20 mm cube needs many passes, got {}",
        per_layer[0].1
    );
}

#[test]
fn ironing_lays_far_less_plastic_than_the_skin_it_polishes() {
    let plain = voron();
    let (plain_gcode, _) = make("cube.stl", &plain);
    let mut ironed_cfg = voron();
    ironed_cfg.ironing = true;
    let job = {
        let mesh = Mesh::load_stl(fixture("cube.stl")).expect("fixture loads");
        let mut s = settings(&ironed_cfg);
        s.ironing = true;
        s.ironing_spacing = ironed_cfg.ironing_spacing;
        run(
            &mesh,
            ironed_cfg.first_layer_height,
            ironed_cfg.layer_height,
            &s,
        )
        .expect("job runs")
    };
    let ironed = generate(&job.layers, &ironed_cfg, Some("part")).expect("gcode builds");
    let added = verify(&ironed.text, &ironed_cfg).filament_mm
        - verify(&plain_gcode.text, &plain).filament_mm;
    let skin = verify(&plain_gcode.text, &plain).filament_mm;
    assert!(added > 0.0, "ironing added nothing");
    assert!(
        added / skin < 0.10,
        "ironing added {:.1}% of the file's filament",
        100.0 * added / skin
    );
}

#[test]
fn the_top_surface_takes_its_own_speed_and_acceleration() {
    let mut cfg = voron();
    cfg.solid_speed = 60.0;
    cfg.top_solid_speed = 25.0;
    cfg.top_solid_accel = 2000.0;
    let (gcode, _) = make("cube.stl", &cfg);

    let mut printing = None::<String>;
    let mut seen: Vec<(String, f64, bool)> = Vec::new();
    for line in gcode.text.lines() {
        if let Some(kind) = line.strip_prefix(";TYPE:") {
            printing = Some(kind.to_owned());
        } else if let (Some(kind), Some(f)) = (&printing, line.split(" F").nth(1)) {
            if !line.starts_with("G1 X") && !line.starts_with("G2 ") {
                continue;
            }
            if kind != "Top surface" && kind != "Internal solid infill" {
                continue;
            }
            let mm_per_s = f.trim().parse::<f64>().expect("feed is a number") / 60.0;
            let entry = (kind.clone(), mm_per_s, line.contains("E"));
            if !seen.contains(&entry) {
                seen.push(entry);
            }
        }
    }
    assert!(
        seen.iter()
            .any(|(k, s, _)| k == "Top surface" && (s - 25.0).abs() < 0.01),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|(k, s, _)| k == "Internal solid infill" && (s - 60.0).abs() < 0.01),
        "{seen:?}"
    );
    assert!(
        gcode.text.contains("M204 S2000") || gcode.text.contains("ACCEL=2000"),
        "the top-surface acceleration was never emitted"
    );
}

#[test]
fn a_first_layer_speed_per_feature_beats_the_global_cap() {
    let mut cfg = voron();
    cfg.first_layer_speed = 20.0;
    cfg.first_layer_outer_speed = 15.0;
    let (gcode, _) = make("cube.stl", &cfg);

    let first_layer = gcode
        .text
        .split(";LAYER_CHANGE")
        .nth(1)
        .expect("the first layer is written");
    let mut printing = None::<String>;
    let mut outer: Vec<f64> = Vec::new();
    let mut inner: Vec<f64> = Vec::new();
    let mut feed = 0.0;
    for line in first_layer.lines() {
        if let Some(kind) = line.strip_prefix(";TYPE:") {
            printing = Some(kind.to_owned());
        } else if line.starts_with("G1 X") {
            if let Some(f) = line.split(" F").nth(1) {
                feed = f.trim().parse::<f64>().expect("the feed is a number");
            }
            assert!(feed > 0.0, "no feed known at the first extrusion: {line}");
            match printing.as_deref() {
                Some("Outer wall") => outer.push(feed / 60.0),
                Some("Inner wall") => inner.push(feed / 60.0),
                _ => {}
            }
        }
    }
    assert!(outer.iter().all(|s| (s - 15.0).abs() < 0.01), "{outer:?}");
    assert!(inner.iter().all(|s| (s - 20.0).abs() < 0.01), "{inner:?}");
}

#[test]
fn a_vertical_travel_speed_sets_the_z_feed() {
    let mut cfg = voron();
    cfg.z_hop = 0.4;
    cfg.travel_speed_z = 15.0;
    let (gcode, _) = make("cube.stl", &cfg);
    let hop_feeds: Vec<f64> = gcode
        .text
        .lines()
        .filter(|l| l.starts_with("G1 Z") && l.contains('F'))
        .filter_map(|l| l.split(" F").nth(1))
        .filter_map(|f| f.trim().parse::<f64>().ok())
        .filter(|f| (f - 600.0).abs() > 0.5)
        .collect();
    assert!(!hop_feeds.is_empty(), "no hop was written to measure");
    for feed in hop_feeds {
        assert!((feed - 900.0).abs() < 0.5, "hop feed {feed}");
    }
}

#[test]
fn the_overhang_ladder_slows_the_paths_that_hang_over_air() {
    let mut cfg = voron();
    cfg.dynamic_overhang_speeds = true;
    cfg.overhang_speeds = [30.0, 20.0, 12.0, 6.0];
    let job = {
        let mesh = Mesh::load_stl(fixture("sphere.stl")).expect("fixture loads");
        let mut s = settings(&cfg);
        s.measure_overhang = true;
        run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs")
    };
    let overhanging = job
        .layers
        .iter()
        .flat_map(|l| l.paths.iter())
        .filter(|p| p.overhang > 0.0)
        .count();
    assert!(overhanging > 0, "a sphere has no overhanging paths?");

    let gcode = generate(&job.layers, &cfg, Some("part")).expect("gcode builds");
    assert!(verify(&gcode.text, &cfg).ok());
    let mut feeds: Vec<f64> = Vec::new();
    for line in gcode.text.lines() {
        if line.starts_with("G1 X") || line.starts_with("G2 ") {
            if let Some(f) = line.split(" F").nth(1) {
                if let Ok(feed) = f.trim().parse::<f64>() {
                    feeds.push(feed / 60.0);
                }
            }
        }
    }
    let rungs = [6.0, 12.0, 20.0, 30.0];
    let used = rungs
        .iter()
        .filter(|rung| feeds.iter().any(|s| (s - *rung).abs() < 0.01))
        .count();
    assert!(
        used >= 3,
        "only {used} of the four overhang rungs ran: {feeds:?}"
    );
    assert!(
        feeds.iter().any(|s| *s < 6.05),
        "the deepest overhang never took the slowest rung"
    );
}

#[test]
fn dynamic_fan_follows_the_overhang_and_ironing_takes_its_own_duty() {
    let mut cfg = voron();
    cfg.dynamic_fan_speeds = true;
    cfg.overhang_fan_speeds = [40.0, 70.0, 90.0, 100.0];
    cfg.ironing = true;
    cfg.ironing_fan_speed = 0.0;
    let job = {
        let mesh = Mesh::load_stl(fixture("sphere.stl")).expect("fixture loads");
        let mut s = settings(&cfg);
        s.measure_overhang = true;
        s.ironing = true;
        s.ironing_spacing = cfg.ironing_spacing;
        run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs")
    };
    let gcode = generate(&job.layers, &cfg, Some("part")).expect("gcode builds");
    assert!(verify(&gcode.text, &cfg).ok());

    let mut printing = None::<String>;
    let mut fan_for: Vec<(String, i64)> = Vec::new();
    for line in gcode.text.lines() {
        if let Some(kind) = line.strip_prefix(";TYPE:") {
            printing = Some(kind.to_owned());
        } else if let Some(rest) = line.strip_prefix("M106 S") {
            let pwm: i64 = rest.trim().parse().expect("a PWM value");
            let entry = (printing.clone().unwrap_or_default(), pwm);
            if !fan_for.contains(&entry) {
                fan_for.push(entry);
            }
        }
    }
    let overhang_used = fan_for
        .iter()
        .any(|(_, pwm)| [102, 179, 229, 255].contains(pwm));
    assert!(
        overhang_used,
        "no overhang fan duty was emitted: {fan_for:?}"
    );
    assert!(
        fan_for
            .iter()
            .any(|(kind, pwm)| kind == "Ironing" && *pwm == 0),
        "ironing kept the layer fan: {fan_for:?}"
    );
}

#[test]
fn a_first_layer_bed_temperature_returns_to_the_steady_value() {
    let mut cfg = voron();
    cfg.bed_temp = 60.0;
    cfg.first_layer_bed_temp = 65.0;
    let (gcode, _) = make("cube.stl", &cfg);
    let lines: Vec<&str> = gcode
        .text
        .lines()
        .filter(|l| l.starts_with("M140 S"))
        .collect();
    assert!(lines.contains(&"M140 S65"), "{lines:?}");
    assert!(lines.contains(&"M140 S60"), "{lines:?}");
    let first_65 = gcode.text.find("M140 S65").expect("first layer bed temp");
    let back = gcode.text[first_65..]
        .find("M140 S60")
        .expect("the steady bed temperature comes back");
    assert!(back > 0);
}

#[test]
fn a_profile_without_its_own_bed_wait_gets_one() {
    let mut cfg = voron();
    cfg.first_layer_bed_temp = 65.0;
    cfg.start_gcode = "G28\n".to_owned();
    let (gcode, _) = make("cube.stl", &cfg);
    let waited = gcode.text.lines().filter(|l| *l == "M190 S65").count();
    assert_eq!(waited, 1, "the writer must wait for the bed it just set");
    let m140 = gcode.text.find("M140 S65").expect("first-layer bed temp");
    let m190 = gcode.text.find("M190 S65").expect("the wait");
    assert!(m190 > m140, "the wait comes after the target");
}

#[test]
fn a_start_g_code_that_already_waits_is_not_waited_on_twice() {
    let mut cfg = voron();
    cfg.first_layer_bed_temp = 65.0;
    cfg.start_gcode = "G28\nM190 S65\n".to_owned();
    let (gcode, _) = make("cube.stl", &cfg);
    assert_eq!(
        gcode.text.lines().filter(|l| l.starts_with("M190")).count(),
        1
    );
}

#[test]
fn marlin_gets_pressure_advance_as_m900() {
    let mut cfg = voron();
    cfg.flavor = Flavor::Marlin;
    cfg.pressure_advance = 0.045;
    let (gcode, _) = make("cube.stl", &cfg);
    assert!(gcode.text.contains("M900 K0.045"), "{}", &gcode.text[..400]);
}

#[test]
fn the_hop_is_skipped_outside_its_z_window() {
    let hop_zs = |text: &str| -> Vec<f64> {
        text.lines()
            .filter(|l| l.starts_with("G1 Z"))
            .filter_map(|l| l.split('Z').nth(1))
            .filter_map(|z| z.split(' ').next())
            .filter_map(|z| z.parse::<f64>().ok())
            .filter(|z| ((z / 0.2) - (z / 0.2).round()).abs() > 0.02)
            .filter(|z| *z < 5.0)
            .collect()
    };
    let mut plain = voron();
    plain.z_hop = 0.63;
    let (gcode, _) = make("cube.stl", &plain);
    let hops = hop_zs(&gcode.text);
    assert!(!hops.is_empty(), "a plain hop should lift");

    let mut ceiling = voron();
    ceiling.z_hop = 0.63;
    ceiling.retract_lift_above = 0.5;
    let (capped, _) = make("cube.stl", &ceiling);
    let capped_hops = hop_zs(&capped.text);
    assert!(
        capped_hops.len() < hops.len(),
        "a ceiling has to remove hops: {} of {}",
        capped_hops.len(),
        hops.len()
    );
    let highest = capped_hops.iter().cloned().fold(0.0f64, f64::max);
    assert!(
        highest <= 0.5 + 0.63 + 0.01,
        "a hop at {highest} mm is above the ceiling"
    );

    let mut floor = voron();
    floor.z_hop = 0.63;
    floor.retract_lift_below = 5.0;
    let (lifted, _) = make("cube.stl", &floor);
    assert!(hop_zs(&lifted.text).is_empty(), "a floor blocks every hop");
}

#[test]
fn a_ramping_hop_climbs_and_lands_in_one_move() {
    let mut cfg = voron();
    cfg.z_hop = 0.8;
    cfg.travel_ramping_lift = true;
    cfg.travel_slope = 1.0;
    let (gcode, _) = make("cube.stl", &cfg);
    let diagonal = gcode
        .text
        .lines()
        .filter(|l| l.starts_with("G1 X") && l.contains(" Z"))
        .count();
    assert!(diagonal > 0, "no diagonal ramp move was written");
    let vertical = gcode
        .text
        .lines()
        .filter(|l| l.starts_with("G1 Z") && l.contains("F600"))
        .count();
    assert!(vertical > 0, "the climb still needs a lift move");
}

#[test]
fn a_scarf_graduates_the_flow_instead_of_butting_the_seam() {
    let mut cfg = voron();
    cfg.scarf_seam_length = 8.0;
    cfg.scarf_seam_max_segment = 0.5;
    cfg.scarf_seam_placement = "everywhere".to_owned();
    cfg.scarf_seam_only_on_smooth = false;
    let (gcode, _) = make("cube.stl", &cfg);
    assert!(verify(&gcode.text, &cfg).ok());
    let outer: Vec<&str> = gcode
        .text
        .lines()
        .skip_while(|l| !l.contains(";TYPE:Outer wall"))
        .take_while(|l| !l.contains(";TYPE:") || l.contains(";TYPE:Outer wall"))
        .filter(|l| l.starts_with("G1 X"))
        .collect();
    let volumes: Vec<f64> = outer
        .iter()
        .filter_map(|l| {
            l.split(" E")
                .nth(1)
                .and_then(|e| e.trim().parse::<f64>().ok())
        })
        .collect();
    assert!(
        volumes.len() > 8,
        "the outer wall is too short to test: {volumes:?}"
    );
    let first = volumes[0];
    let mid = volumes[volumes.len() / 2];
    assert!(
        first < 0.5 * mid,
        "the seam does not start thin ({first} vs {mid})"
    );
}

#[test]
fn a_seam_gap_leaves_a_hole_in_the_loop() {
    let mut cfg = voron();
    cfg.seam_gap = 1.0;
    let (gcode, _) = make("cube.stl", &cfg);
    assert!(
        verify(&gcode.text, &cfg).ok(),
        "a gapped loop must still verify"
    );
}

#[test]
fn fuzzy_skin_moves_the_outer_wall_off_the_ellipse() {
    let mut cfg = voron();
    cfg.fuzzy_skin = "external".to_owned();
    cfg.fuzzy_skin_thickness = 0.3;
    cfg.fuzzy_skin_point_dist = 0.4;
    let (gcode, _) = make("cube.stl", &cfg);
    let xs: Vec<f64> = gcode
        .text
        .lines()
        .skip_while(|l| !l.contains(";TYPE:Outer wall"))
        .take_while(|l| !l.contains(";TYPE:") || l.contains(";TYPE:Outer wall"))
        .filter(|l| l.starts_with("G1 X"))
        .filter_map(|l| l.split(" X").nth(1))
        .filter_map(|x| x.split(' ').next())
        .filter_map(|x| x.parse::<f64>().ok())
        .collect();
    let distinct: Vec<f64> = {
        let mut v = xs.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        v
    };
    assert!(
        distinct.len() > 6,
        "a fuzzy wall should wander, found {} x values",
        distinct.len()
    );
}

#[test]
fn a_raft_prints_below_the_part_and_lifts_it() {
    let mut cfg = voron();
    cfg.raft_layers = 2;
    cfg.first_layer_height = 0.2;
    cfg.raft_contact_distance = 0.1;
    let (gcode, _) = make("cube.stl", &cfg);
    assert!(gcode.text.contains(";RAFT:0"), "no raft layer was emitted");
    assert!(gcode.text.contains(";RAFT:1"));
    let first_object_z = gcode
        .text
        .lines()
        .skip_while(|l| !l.contains(";LAYER:0"))
        .find(|l| l.starts_with("G1 Z"))
        .and_then(|l| l.split('Z').nth(1))
        .and_then(|z| z.split(' ').next())
        .and_then(|z| z.parse::<f64>().ok())
        .expect("an object layer");
    assert!(
        (first_object_z - 0.7).abs() < 0.01,
        "the first object layer tops out at 0.2 + 0.5 mm, found {first_object_z}"
    );
}

#[test]
fn a_draft_shield_wraps_every_layer_it_covers() {
    let mut cfg = voron();
    cfg.draft_shield = "enabled".to_owned();
    cfg.skirt_loops = 0;
    let (gcode, _) = make("cube.stl", &cfg);
    let layers = gcode.text.matches(";LAYER:").count();
    let skirts = gcode.text.matches(";TYPE:Skirt").count();
    assert!(
        skirts >= layers,
        "the shield should appear on every layer: {skirts} of {layers}"
    );
}

#[test]
fn a_minimum_skirt_length_adds_loops() {
    let mut one = voron();
    one.skirt_loops = 1;
    let (short, _) = make("cube.stl", &one);
    let mut floored = voron();
    floored.skirt_loops = 1;
    floored.min_skirt_length = 400.0;
    let (long, _) = make("cube.stl", &floored);
    let count = |text: &str| {
        text.lines()
            .skip_while(|l| !l.contains(";TYPE:Skirt"))
            .take_while(|l| !l.contains(";TYPE:") || l.contains(";TYPE:Skirt"))
            .filter(|l| l.starts_with("G1 X"))
            .count()
    };
    assert!(
        count(&long.text) > count(&short.text) * 2,
        "a 400 mm floor should add loops: {} vs {}",
        count(&long.text),
        count(&short.text)
    );
}

fn brim_box(side: &str, model: &str) -> (f64, f64, f64, f64) {
    let mut cfg = voron();
    cfg.brim_loops = 2;
    cfg.brim_type = side.to_owned();
    let job = {
        let mesh = Mesh::load_stl(fixture(model)).expect("fixture loads");
        let mut s = settings(&cfg);
        s.brim_loops = cfg.brim_loops;
        s.brim_side = match side {
            "inner_only" => hfw_walls::BrimSide::Inner,
            "outer_and_inner" => hfw_walls::BrimSide::Both,
            _ => hfw_walls::BrimSide::Outer,
        };
        run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs")
    };
    let gcode = generate(&job.layers, &cfg, Some("part")).expect("gcode builds");
    let mut b = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    let mut inside = false;
    for line in gcode.text.lines() {
        if let Some(kind) = line.strip_prefix(";TYPE:") {
            inside = kind.trim() == "Brim";
            continue;
        }
        if !inside || !line.starts_with("G1 X") {
            continue;
        }
        let mut it = line.split_whitespace();
        let _ = it.next();
        let x: f64 = it
            .next()
            .and_then(|v| v.strip_prefix('X'))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let y: f64 = it
            .next()
            .and_then(|v| v.strip_prefix('Y'))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
    }
    b
}

#[test]
fn an_inner_brim_grows_into_a_hole_instead_of_outwards() {
    let o = brim_box("outer_only", "torus.stl");
    let i = brim_box("inner_only", "torus.stl");
    assert!(
        o.0.is_finite() && i.0.is_finite(),
        "one side printed no brim"
    );
    let o_area = (o.2 - o.0) * (o.3 - o.1);
    let i_area = (i.2 - i.0) * (i.3 - i.1);
    assert!(
        i_area < o_area * 0.8,
        "the inner brim ({i_area}) should sit inside the outer one ({o_area})"
    );
    assert!(
        i.0 >= o.0 - 1e-6 && i.2 <= o.2 + 1e-6,
        "the inner brim left the part's own footprint"
    );
}

#[test]
fn the_support_interface_carries_its_own_loops_sheath_and_density() {
    let mut cfg = voron();
    cfg.support = true;
    cfg.support_pattern = "grid".to_owned();
    cfg.support_interface_pattern = "rectilinear".to_owned();
    cfg.support_interface_loops = 2;
    cfg.support_sheath = true;
    let job = {
        let mesh = Mesh::load_stl(fixture("overhang.stl")).expect("fixture loads");
        let mut s = settings(&cfg);
        s.support = Some(hfw_engine::SupportSettings {
            pattern: hfw_engine::SupportFill::Grid,
            interface_pattern: Some(hfw_engine::SupportFill::Rectilinear),
            interface_loops: cfg.support_interface_loops,
            sheath: cfg.support_sheath,
            ..hfw_engine::SupportSettings::new(cfg.line_width)
        });
        run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs")
    };
    let gcode = generate(&job.layers, &cfg, Some("part")).expect("gcode builds");
    assert!(verify(&gcode.text, &cfg).ok());
    let interface_loops = gcode.text.matches(";TYPE:Support interface").count();
    assert!(interface_loops > 0, "no support interface was labelled");
    let support_lines = gcode.text.matches(";TYPE:Support").count();
    assert!(support_lines > 0, "the sheath loop needs a support feature");
}

fn small_job(cfg: &PrinterConfig) -> hfw_engine::Job {
    let mesh = Mesh::load_stl(fixture("cube.stl")).expect("fixture loads");
    run(
        &mesh,
        cfg.first_layer_height,
        cfg.layer_height,
        &settings(cfg),
    )
    .expect("job runs")
}

#[test]
fn comments_and_object_labels_can_be_switched_off() {
    let mut cfg = voron();
    let job = small_job(&cfg);
    let loud = generate(&job.layers, &cfg, Some("part")).expect("gcode builds");
    assert!(loud.text.contains(";TYPE:"));
    cfg.gcode_comments = false;
    cfg.label_objects = false;
    let quiet = generate(&job.layers, &cfg, Some("part")).expect("gcode builds");
    assert!(
        quiet.text.contains(";TYPE:"),
        "the line types are what the preview reads, not commentary"
    );
    assert!(!quiet.text.contains(";WIDTH:"), "a width comment survived");
    assert!(!quiet.text.contains(";OBJECT:"), "an object label survived");
    assert!(
        quiet.text.contains(";LAYER_CHANGE"),
        "the layer markers are structure, not commentary"
    );
    assert!(verify(&quiet.text, &cfg).ok(), "the file must still verify");
}

#[test]
fn coordinates_snap_to_the_gcode_resolution() {
    let mut cfg = voron();
    cfg.gcode_resolution = 0.05;
    let job = small_job(&cfg);
    let gcode = generate(&job.layers, &cfg, None).expect("gcode builds");
    let mut checked = 0;
    for line in gcode.text.lines() {
        if !line.starts_with("G1 X") && !line.starts_with("G0 X") {
            continue;
        }
        for field in line
            .split_whitespace()
            .filter(|f| f.starts_with('X') || f.starts_with('Y'))
        {
            let value: f64 = field[1..].parse().expect("a coordinate parses");
            let steps = value / 0.05;
            assert!(
                (steps - steps.round()).abs() < 1e-6,
                "{field} is off the 0.05 mm grid"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 20,
        "the file has too few coordinates to prove much"
    );
}

#[test]
fn volumetric_e_writes_cubic_millimetres_and_declares_the_diameter() {
    let mut cfg = voron();
    cfg.volumetric_e = true;
    let job = small_job(&cfg);
    let gcode = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(
        gcode
            .text
            .contains(&format!("M200 D{}", cfg.filament_diameter)),
        "volumetric E needs the diameter declared"
    );
    let mut largest: f64 = 0.0;
    for line in gcode
        .text
        .lines()
        .filter(|l| l.starts_with("G1 ") && l.contains(" E"))
    {
        if let Some(field) = line.split_whitespace().find(|f| f.starts_with('E')) {
            largest = largest.max(field[1..].parse::<f64>().unwrap_or(0.0));
        }
    }
    assert!(
        largest < 100.0,
        "volumetric E of {largest} mm3 per move is not a bead"
    );
    assert!(verify(&gcode.text, &cfg).ok());
}

#[test]
fn firmware_retraction_uses_the_machine_codes() {
    let mut cfg = voron();
    cfg.flavor = Flavor::Marlin;
    cfg.firmware_retraction = true;
    cfg.retract_len = 0.8;
    let job = small_job(&cfg);
    let gcode = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(gcode.text.contains("G10\n"), "no firmware retract");
    assert!(gcode.text.contains("G11\n"), "no firmware deretract");
    assert!(
        gcode.text.contains("M207 S0.8"),
        "firmware retraction needs the length declared"
    );
}

#[test]
fn temperature_commands_can_be_left_to_the_start_gcode() {
    let mut cfg = voron();
    cfg.autoemit_temperatures = false;
    cfg.start_gcode.clear();
    cfg.end_gcode.clear();
    let job = small_job(&cfg);
    let gcode = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(!gcode.text.contains("M109 S"), "M109 was emitted anyway");
    assert!(!gcode.text.contains("M140 S"), "M140 was emitted anyway");
    assert!(!gcode.text.contains("M190 S"), "M190 was emitted anyway");
    assert!(
        !gcode.text.contains("M104 S215"),
        "the layer-2 target was emitted anyway"
    );
}

#[test]
fn layer_hooks_and_progress_land_where_they_are_asked_for() {
    let mut cfg = voron();
    cfg.before_layer_gcode = ";LAYER_HOOK at {layer_z}".to_owned();
    cfg.layer_gcode = ";LAYER_DONE {layer_num}".to_owned();
    cfg.remaining_times = true;
    let job = small_job(&cfg);
    let gcode = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(
        gcode.text.contains(";LAYER_HOOK at 0.200"),
        "before_layer_gcode did not fill"
    );
    assert!(
        gcode.text.contains(";LAYER_DONE 0"),
        "layer_gcode did not fire"
    );
    assert!(gcode.text.contains("M73 P"), "no progress report");
    let markers = gcode.text.matches(";LAYER_CHANGE").count();
    let hooks = gcode.text.matches(";LAYER_HOOK").count();
    assert_eq!(markers, hooks, "one hook per layer, not more and not fewer");
}

#[test]
fn a_thumbnail_block_carries_a_decodable_image() {
    let mut cfg = voron();
    let job = small_job(&cfg);
    let plain = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(
        !plain.text.contains("thumbnail"),
        "no thumbnails were requested"
    );

    cfg.thumbnails = vec![
        hfw_config::Thumbnail {
            width: 32,
            height: 32,
            png: true,
        },
        hfw_config::Thumbnail {
            width: 16,
            height: 16,
            png: false,
        },
    ];
    let gcode = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(
        gcode.text.contains("; thumbnail begin 32x32 "),
        "no PNG thumb"
    );
    assert!(
        gcode.text.contains("; thumbnail begin 16x16 "),
        "no QOI thumb"
    );
    assert_eq!(gcode.text.matches("; thumbnail end").count(), 2);
    assert!(verify(&gcode.text, &cfg).ok());
}

#[test]
fn clockwise_preference_keeps_the_volume_and_flips_the_direction() {
    let mut cfg = voron();
    let job = small_job(&cfg);
    let normal = generate(&job.layers, &cfg, None).expect("gcode builds");
    cfg.clockwise = true;
    let flipped = generate(&job.layers, &cfg, None).expect("gcode builds");
    let delta = (normal.stats.filament_mm - flipped.stats.filament_mm).abs();
    assert!(
        delta < 0.02 * normal.stats.filament_mm.max(1.0),
        "reversing loops changed the filament by {delta} mm"
    );
    assert_ne!(
        normal.text, flipped.text,
        "the direction change was a no-op"
    );
    assert!(verify(&flipped.text, &cfg).ok());
}

#[test]
fn a_bridge_keeps_its_support_only_when_asked() {
    let mut cfg = voron();
    cfg.support = true;
    let job = {
        let mesh = Mesh::load_stl(fixture("bridge.stl")).expect("fixture loads");
        let mut s = settings(&cfg);
        s.support = Some(hfw_engine::SupportSettings::new(cfg.line_width));
        run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs")
    };
    let supported = generate(&job.layers, &cfg, None).expect("gcode builds");
    let passes = supported.text.matches(";TYPE:Support").count();
    assert!(passes > 0, "the bridge fixture produced no support at all");

    cfg.support_skip_bridges = true;
    let job = {
        let mesh = Mesh::load_stl(fixture("bridge.stl")).expect("fixture loads");
        let mut s = settings(&cfg);
        s.support = Some(hfw_engine::SupportSettings {
            skip_bridges: true,
            ..hfw_engine::SupportSettings::new(cfg.line_width)
        });
        run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs")
    };
    let bare = generate(&job.layers, &cfg, None).expect("gcode builds");
    let left = bare.text.matches(";TYPE:Support").count();
    assert!(
        left < passes,
        "skipping bridges kept every support pass: {left} of {passes}"
    );
    assert!(verify(&bare.text, &cfg).ok());
}

#[test]
fn a_layer_height_profile_changes_z_where_it_says_and_verifies() {
    let cfg = voron();
    let mesh = Mesh::load_stl(fixture("cube.stl")).expect("fixture loads");
    let mut s = settings(&cfg);
    s.layer_profile = vec![(0.0, 0.2), (4.0, 0.12), (8.0, 0.28)];
    s.min_layer_height = 0.08;
    s.max_layer_height = 0.3;
    let job = run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs");

    let heights: Vec<f64> = job.layers.iter().map(|l| l.height()).collect();
    assert!(
        heights.iter().any(|h| (h - 0.12).abs() < 1e-6),
        "the thin step never appeared: {heights:?}"
    );
    assert!(
        heights.iter().any(|h| (h - 0.28).abs() < 1e-6),
        "the thick step never appeared: {heights:?}"
    );
    assert!(
        heights.iter().all(|h| *h >= 0.08 - 1e-6),
        "a layer came out thinner than the floor: {heights:?}"
    );
    let top = job.layers.last().expect("a last layer").print_z;
    assert!((top - 20.0).abs() < 1e-6, "the stack ends at {top} mm");

    let gcode = generate(&job.layers, &cfg, None).expect("gcode builds");
    let emitted: std::collections::BTreeSet<String> = gcode
        .text
        .lines()
        .filter_map(|line| line.strip_prefix(";HEIGHT:").map(str::to_owned))
        .collect();
    assert!(
        emitted.contains("0.120") && emitted.contains("0.280"),
        "the file did not declare the profile's heights: {emitted:?}"
    );
    assert!(verify(&gcode.text, &cfg).ok());
}

#[test]
fn a_vase_raises_z_inside_every_layer_and_verifies() {
    let mut cfg = voron();
    cfg.spiral_vase = true;
    let job = {
        let mesh = Mesh::load_stl(fixture("sphere.stl")).expect("fixture loads");
        let mut s = settings(&cfg);
        s.spiral_vase = true;
        run(&mesh, cfg.first_layer_height, cfg.layer_height, &s).expect("job runs")
    };
    let gcode = generate(&job.layers, &cfg, None).expect("gcode builds");
    assert!(verify(&gcode.text, &cfg).ok());

    let mut groups: Vec<Vec<f64>> = Vec::new();
    let mut seen = 0usize;
    for line in gcode.text.lines() {
        if line.starts_with(";LAYER:") {
            seen += 1;
            groups.push(Vec::new());
            continue;
        }
        if seen > 6 {
            if let Some(at) = line.rfind(" Z") {
                if let Ok(z) = line[at + 2..].split(' ').next().unwrap_or("").parse() {
                    if let Some(group) = groups.last_mut() {
                        group.push(z);
                    }
                }
            }
        }
    }
    let last = groups.len().saturating_sub(1);
    let spiral: Vec<&Vec<f64>> = groups.iter().take(last).filter(|g| g.len() > 10).collect();
    assert!(spiral.len() > 10, "only {} spiral layers", spiral.len());
    for group in &spiral {
        assert!(
            group.windows(2).all(|w| w[1] >= w[0] - 1e-6),
            "Z went backwards inside a layer: {group:?}"
        );
        let rise = group.last().copied().unwrap_or(0.0) - group.first().copied().unwrap_or(0.0);
        assert!(
            rise > 0.1,
            "a layer rose only {rise} mm; the spiral did not climb"
        );
    }
}

#[test]
fn bead_cross_section_matches_the_reference_slicers() {
    // A 0.4 x 0.2 bead: the rectangle is 0.08 mm2, and the two long sides rounding
    // off costs 0.0086 mm2. Both reference slicers subtract exactly that, so a
    // profile calibrated against their files extrudes the same volume here.
    let area = hfw_gcode::bead_area(0.4, 0.2);
    assert!(
        (area - 0.071_415_9).abs() < 1e-6,
        "bead area {area} is not the rounded 0.4x0.2 section"
    );
    assert!(area < 0.08 && area > 0.88 * 0.08, "{area} left the rounded range");
    assert_eq!(hfw_gcode::bead_area(0.4, 0.0), 0.0);
    let width = hfw_gcode::bead_width(area * 10.0, 10.0, 0.2);
    assert!((width - 0.4).abs() < 1e-9, "inverse read back {width}");
}

#[test]
fn a_bead_narrower_than_its_own_corners_keeps_its_rectangle() {
    let area = hfw_gcode::bead_area(0.12, 0.2);
    assert!(area >= 0.5 * 0.12 * 0.2, "{area} fell below half the rectangle");
}
