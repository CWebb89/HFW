//! Headless command-line front end for the HFW slicing engine.

#![forbid(unsafe_code)]

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

mod json;

/// True when the human report goes to stderr because `--json` owns stdout.
static HUMAN_TO_STDERR: AtomicBool = AtomicBool::new(false);

macro_rules! outln {
    () => {
        if HUMAN_TO_STDERR.load(Ordering::Relaxed) {
            eprintln!()
        } else {
            println!()
        }
    };
    ($($arg:tt)*) => {
        if HUMAN_TO_STDERR.load(Ordering::Relaxed) {
            eprintln!($($arg)*)
        } else {
            println!($($arg)*)
        }
    };
}

use hfw_config::PrinterConfig;
use hfw_config::{Bundle, Intake, IntakeReport, Kind, Preset, PrintSequence, ProjectSettings};
use hfw_engine::{
    evaluate_all, evaluate_perimeter, infill_pattern, run, slice_plane, InfillPattern, Job,
    JobTotals, MaskCache, Mesh, MeshDiagnostics, MeshRepairReport, MeshTransform, Metrics, Mode,
    PlannedSlice, PlateOrder, Settings, SupportSettings, SupportStyle, MANIFEST_HEADER,
};

const USAGE: &str = "\
Usage:
  One layer:   hfw <model.stl> --z <mm> --svg <output.svg> [options]
  All layers:  hfw <model.stl> --layer-height <mm> --out-dir <folder> [options]
  Print file:  hfw <model.stl> --profile <p.cfg> --gcode <out.gcode>
  A plate:     hfw <a.stl> <b.stl> <plate.3mf> --profile <p.cfg> --gcode <out.gcode>
  A bundle:    hfw <model.stl> --bundle <dir> --machine <n> --process <n>
                               --filament <n> --gcode <out.gcode>
  Check file:  hfw --check-gcode <out.gcode> --profile <p.cfg>

Environment:
  HFW_THREADS=<n>             Threads for the wall phase; unset asks the machine.

Slice:
  --z <mm>                    Slice and plan one plane at this height.
  --svg <path>                Write the single-layer SVG here.
  --layer-height <mm>         Regular layer height; plans the whole stack.
  --first-layer-height <mm>   First layer height (default: layer height).
  --out-dir <folder>          Write layer SVGs, manifest.csv and job_report.txt.
                              Optional: a run that only writes G-code skips them.

Model preparation:
  --units mm|inch             Interpret STL coordinates (default mm).
  --scale <factor>            Extra uniform scale (default 1.0).
  --repair-basic              Run the conservative mesh repair pass first.
  --spacing <mm>              Gap between automatically placed objects
                                (default 5).
  --no-arrange                Keep every model's own coordinates instead of
                                laying separate files out side by side.
  --gantry-height <mm>        Override the profile's gantry_height for this run:
                                the clearance the head has over a finished part,
                                which print_sequence = by object needs.

Several model paths make one plate: each 3MF build item is its own object, and
an STL file is placed to the right of everything already on the plate. Objects
are printed one object at a time within each layer, so travels stay near the
part they belong to. An object that came from a 3MF project can carry its own
settings (infill density and pattern, wall count, extrusion width, support);
anything else per object is reported as needing a feature this engine does not
have, never quietly dropped.

Walls:
  --width <mm>                Extrusion width (default 0.4).
  --walls <n>                 Perimeter loops (default 3).
  --object-settings <file>    Per-object settings, one `<input> <key> = <value>` per
                              line; <input> is the 1-based order of the models given.
  --brim <n>                  First-layer adhesion brim loops (default 0).
  --brim-gap <mm>             Clearance between part and brim (default 0.1).
  --elephant-foot <mm>        Shrink the first layer by this much to remove the
                              squashed lip a part gets at the bed (default 0).
  --xy-size <mm>              Grow (+) or shrink (-) every layer; negative opens
                              holes, positive closes them (default 0).
  --mode hfw|classic|field    Offset ladder with gap fill, plain offsets, or
                              variable-width beads traced from a distance field.
  --no-reference              Omit the exact slice outline from the SVG.

Infill:
  --no-infill                 Skip infill entirely.
  --density <f>               Sparse density in 0..1 (default 0.15).
  --pattern <name>            Sparse pattern: rectilinear, grid, line,
                        triangle, cubic, gyroid, 3d_honeycomb,
                        concentric, zigzag, crosshatch, honeycomb,
                        monotonic
                              (default rectilinear).
  --skin-top <n>              Solid layers under a top surface (default 5).
  --skin-bottom <n>           Solid layers above a bottom surface (default 4).
  --closing-radius <mm>       Fill cracks narrower than twice this while slicing
  --close-gaps <mm>           Bridge outline gaps up to this wide (default 2, max 10)
                              (default 0.049, 0 disables).
  --resolution <mm>           Drop slice detail below this deviation; 0 keeps
                              every vertex (default 0).
  --max-bridge-length <mm>    Longest span printed as a bridge; longer spans
                                print as ordinary solid infill.
  --bridge-cross-hatch <0|1>  Print two crossing strands across each gap before
                                the bridge fill.
  --support <0|1>             Generate supports under overhangs.
  --support-style <style>     normal (projected columns) or tree.

Printer:
  --profile <path>            Printer, filament and process profile in this
                              engine's own key names; an unknown key is an error.
  --preset <path>             A PrusaSlicer or OrcaSlicer preset. Engine setting
                              names are accepted, Orca's renames are translated,
                              `inherits` is resolved, and nothing is refused: the
                              report says what was applied and what was not.
  --preset-strict             Fail instead of reporting when a preset carries a
                              key this engine cannot apply.
  --intake-json <path>        Write the intake report as JSON: every key the
                              preset stated and how it was handled. A key that is
                              missing from this document never reached the
                              engine, which is the one failure the printed report
                              cannot show.
  --settings-surface <path>   Write the settings surface as JSON and exit: every
                              setting an engine is expected to understand, whether
                              this one honours it, and where it belongs, plus the
                              features this engine declines and what stands in.
  --bundle <dir>              A whole profile collection: a directory of presets
                              or one sectioned vendor file.
  --machine <name>            Machine preset of that name in the bundle.
  --process <name>            Print-settings preset of that name in the bundle.
  --filament <name>           Material preset of that name in the bundle.
  --list-bundle               Name every preset in --bundle and stop.
  --flow <f>                  Extrusion multiplier 0.5..1.5, overriding the
                              profile. 1.0 prints the geometric volume.
  --gcode <path>              Emit G-code for the whole job (needs --profile,
                              plans every layer, then verifies the result).
  --check-gcode <path>        Verify an existing G-code file and exit.

Evaluation:
  --every <n>                 Plan walls on every Nth layer, plus the last.
  --json                      Print the job as one JSON document on stdout and
                              send the human report to stderr.
  --json-paths                Also include every path vertex in that document.
  --metrics <mm>              Measure coverage at this sampling resolution.

Other:
  --help                      Show this help.

A profile supplies defaults for width, walls, density, pattern, skin layers and
layer heights; any option given on the command line wins. See profiles/ for
voron_klipper.cfg and generic_marlin.cfg.

A preset is what another slicer exports. `--preset` prints an intake report:
how many keys were applied, how many are recognised but not implemented, and
which names this engine does not know. `docs/SETTINGS-LEDGER.md` is the list the
two counts come from.

Single-layer mode plans one isolated plane, so every cell of it counts as a top
and bottom surface: expect skin rather than sparse infill. Use a full stack for
skin and sparse behaviour.

G-code is only emitted when --gcode is given. The emitted file is then checked
by an independent verifier; a failing check is reported and exits non-zero.";

fn main() -> ExitCode {
    match run_cli() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    inputs: Vec<String>,
    z_mm: Option<f64>,
    svg: Option<PathBuf>,
    layer_height: Option<f64>,
    first_layer_height: Option<f64>,
    out_dir: Option<PathBuf>,
    units: String,
    scale: f64,
    repair: bool,
    settings: Settings,
    no_infill: bool,
    density: Option<f64>,
    pattern: Option<String>,
    wall_width: Option<f64>,
    walls: Option<usize>,
    brim: Option<usize>,
    object_settings: Option<PathBuf>,
    brim_gap: Option<f64>,
    elephant_foot: Option<f64>,
    xy_size: Option<f64>,
    closing_radius: Option<f64>,
    close_gaps: Option<f64>,
    resolution: Option<f64>,
    max_bridge_length: Option<f64>,
    cross_hatch: Option<bool>,
    support: Option<bool>,
    support_style: Option<SupportStyle>,
    skin_top: Option<usize>,
    skin_bottom: Option<usize>,
    metrics_res: Option<f64>,
    spacing: Option<f64>,
    arrange: bool,
    gantry_height: Option<f64>,
    profile: Option<PathBuf>,
    profile_config: Option<PrinterConfig>,
    preset_strict: bool,
    preset_report: Option<IntakeReport>,
    bundle: Option<PathBuf>,
    machine: Option<String>,
    filament: Option<String>,
    process: Option<String>,
    list_bundle: bool,
    flow: Option<f64>,
    gcode: Option<PathBuf>,
    check_gcode: Option<PathBuf>,
    pause_at: Vec<f64>,
    colour_change_at: Vec<f64>,
    json: bool,
    json_paths: bool,
    /// Where the intake report goes as JSON, one entry per key the preset stated.
    intake_json: Option<PathBuf>,
}

impl Options {
    fn base() -> Options {
        Options {
            inputs: Vec::new(),
            z_mm: None,
            svg: None,
            layer_height: None,
            first_layer_height: None,
            out_dir: None,
            units: "mm".to_owned(),
            scale: 1.0,
            repair: false,
            settings: Settings::default(),
            no_infill: false,
            density: None,
            pattern: None,
            wall_width: None,
            walls: None,
            brim: None,
            object_settings: None,
            brim_gap: None,
            elephant_foot: None,
            xy_size: None,
            closing_radius: None,
            close_gaps: None,
            resolution: None,
            max_bridge_length: None,
            cross_hatch: None,
            support: None,
            support_style: None,
            skin_top: None,
            skin_bottom: None,
            metrics_res: None,
            spacing: None,
            arrange: true,
            gantry_height: None,
            profile: None,
            profile_config: None,
            preset_strict: false,
            preset_report: None,
            bundle: None,
            machine: None,
            filament: None,
            process: None,
            list_bundle: false,
            flow: None,
            gcode: None,
            check_gcode: None,
            pause_at: Vec::new(),
            colour_change_at: Vec::new(),
            json: false,
            json_paths: false,
            intake_json: None,
        }
    }

    fn apply_overrides(&mut self) {
        if let Some(w) = self.wall_width {
            self.settings.wall.w = w;
        }
        if let Some(n) = self.walls {
            self.settings.wall.walls = n;
        }
        if let Some(n) = self.brim {
            self.settings.brim_loops = n;
        }
        if let Some(g) = self.brim_gap {
            self.settings.brim_gap = g;
        }
        if let Some(v) = self.elephant_foot {
            self.settings.elephant_foot = v;
        }
        if let Some(v) = self.xy_size {
            self.settings.xy_size = v;
        }
        if let Some(v) = self.closing_radius {
            self.settings.closing_radius = v;
        }
        if let Some(v) = self.close_gaps {
            self.settings.close_gaps = v.clamp(0.0, hfw_engine::MAX_CLOSE_GAPS);
        }
        if let Some(v) = self.resolution {
            self.settings.resolution = v;
        }
        if let (Some(l), Some(ip)) = (self.max_bridge_length, self.settings.infill.as_mut()) {
            ip.bridge.max_length = l;
        }
        if let Some(ip) = self.settings.infill.as_mut() {
            if let Some(n) = self.skin_top {
                ip.top_layers = n;
            }
            if let Some(n) = self.skin_bottom {
                ip.bottom_layers = n;
            }
        }
    }

    fn apply_project_settings(&mut self) -> Result<(), String> {
        if self.profile_config.is_some() || self.inputs.is_empty() {
            return Ok(());
        }
        let Some(first) = self.inputs.first().cloned() else {
            return Ok(());
        };
        if !first.to_ascii_lowercase().ends_with(".3mf") {
            return Ok(());
        }
        let parts = hfw_engine::three_mf_metadata(&first).map_err(|error| error.to_string())?;
        let Some(project) = ProjectSettings::from_metadata(&parts) else {
            return Ok(());
        };
        let (config, report) = PrinterConfig::from_preset(project.preset());
        outln!(
            "Project {} -> '{}' (from {})",
            first,
            config.name,
            project.part()
        );
        print_report(&report);
        if project.object_overrides() > 0 {
            outln!(
                "  {} object(s) carry per-object overrides; those are not applied",
                project.object_overrides()
            );
        }
        self.profile_config = Some(config);
        self.preset_report = Some(report);
        Ok(())
    }

    fn apply_profile(&mut self, cfg: &PrinterConfig) {
        self.settings.apply_printer(cfg);
        if let Ok(text) = std::env::var("HFW_THREADS") {
            if let Ok(count) = text.trim().parse::<usize>() {
                self.settings.threads = count;
            }
        }
        if self.layer_height.is_none() {
            self.layer_height = Some(cfg.layer_height);
        }
        if self.first_layer_height.is_none() {
            self.first_layer_height = Some(cfg.first_layer_height);
        }
    }
}

fn next_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn print_intake(path: &std::path::Path, config: &PrinterConfig, report: &IntakeReport) {
    outln!("Preset {} -> '{}'", path.display(), config.name);
    print_report(report);
}

fn print_report(report: &IntakeReport) {
    outln!("  {}", report.summary());
    for (key, intake) in report.gaps() {
        let label = match intake {
            Intake::Applied => "applied",
            Intake::Partial => "partial: value applied, this engine's meaning differs",
            Intake::Recognised => "recognised: part of the dialect, not implemented yet",
            Intake::Unconverted => "unconverted: value spelling this engine rejects",
            Intake::Unknown => "unknown: not a dialect name",
        };
        outln!("    {key}: {label}");
    }
    for warning in report.warnings() {
        outln!("  warning: {warning}");
    }
}

/// Resolves `--bundle` into one machine, process and filament, and layers them.
///
/// The three names are looked up across the whole bundle by kind, so a bundle
/// organised as `machine/`, `process/`, `filament/` and one that is a single
/// sectioned vendor file are used the same way. A parent preset that lives inside
/// the slicer rather than in the bundle is reported and skipped: the file is
/// still a usable preset, and saying which parent was missing is what makes the
/// omission checkable.
fn resolve_bundle(o: &mut Options) -> Result<(), String> {
    let root = o
        .bundle
        .clone()
        .ok_or_else(|| "--bundle needs a directory".to_owned())?;
    let bundle = Bundle::discover(&root).map_err(|error| error.to_string())?;
    outln!("Bundle {}: {}", root.display(), bundle.summary());
    for warning in bundle.warnings() {
        outln!("  warning: {warning}");
    }
    if o.list_bundle {
        for kind in [Kind::Machine, Kind::Process, Kind::Filament] {
            let names = bundle.names(kind);
            outln!("{} ({}):", kind.label(), names.len());
            for name in names {
                outln!("  {name}");
            }
        }
        return Ok(());
    }
    let machine = o
        .machine
        .clone()
        .ok_or_else(|| "--bundle needs --machine <name>".to_owned())?;
    let process = o
        .process
        .clone()
        .ok_or_else(|| "--bundle needs --process <name>".to_owned())?;
    let filament = o
        .filament
        .clone()
        .ok_or_else(|| "--bundle needs --filament <name>".to_owned())?;
    let loaded = bundle
        .load(&machine, &process, &filament)
        .map_err(|error| error.to_string())?;
    for warning in loaded.resolution().warnings() {
        outln!("  warning: {warning}");
    }
    outln!(
        "Bundle job: {machine} + {filament} + {process} -> '{}'",
        loaded.config().name
    );
    print_report(loaded.report());
    if o.preset_strict && !loaded.report().is_complete() {
        return Err(format!(
            "--preset-strict: {} of {} keys were not applied",
            loaded.report().len() - loaded.report().count(Intake::Applied),
            loaded.report().len()
        ));
    }
    o.profile_config = Some(loaded.config().clone());
    o.preset_report = Some(loaded.report().clone());
    o.profile = Some(root);
    Ok(())
}

fn number(value: &str, option: &str) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("invalid {option} value: {value}"))?;
    if !parsed.is_finite() {
        return Err(format!("{option} must be a finite number"));
    }
    Ok(parsed)
}

fn positive(value: &str, option: &str) -> Result<f64, String> {
    let parsed = number(value, option)?;
    if parsed <= 0.0 {
        return Err(format!("{option} must be greater than zero"));
    }
    Ok(parsed)
}

fn count(value: &str, option: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid {option} value: {value}"))
}

fn parse_arguments() -> Result<Option<Options>, String> {
    let mut arguments = env::args().skip(1).peekable();
    let mut o = Options::base();
    match arguments.peek() {
        None => return Err(USAGE.to_owned()),
        Some(first) if first.starts_with('-') => {}
        Some(_) => {
            o.inputs.push(arguments.next().unwrap_or_default());
        }
    }

    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--z" => o.z_mm = Some(number(&next_value(&mut arguments, "--z")?, "--z")?),
            "--svg" => o.svg = Some(PathBuf::from(next_value(&mut arguments, "--svg")?)),
            "--layer-height" => {
                o.layer_height = Some(positive(
                    &next_value(&mut arguments, "--layer-height")?,
                    "--layer-height",
                )?)
            }
            "--first-layer-height" => {
                o.first_layer_height = Some(positive(
                    &next_value(&mut arguments, "--first-layer-height")?,
                    "--first-layer-height",
                )?)
            }
            "--out-dir" => {
                o.out_dir = Some(PathBuf::from(next_value(&mut arguments, "--out-dir")?))
            }
            "--units" => {
                o.units = next_value(&mut arguments, "--units")?;
                if o.units != "mm" && o.units != "inch" {
                    return Err("--units must be either mm or inch".to_owned());
                }
            }
            "--scale" => o.scale = positive(&next_value(&mut arguments, "--scale")?, "--scale")?,
            "--repair-basic" => o.repair = true,
            "--width" => {
                o.wall_width = Some(positive(
                    &next_value(&mut arguments, "--width")?,
                    "--width",
                )?)
            }
            "--walls" => o.walls = Some(count(&next_value(&mut arguments, "--walls")?, "--walls")?),
            "--object-settings" => {
                o.object_settings = Some(PathBuf::from(next_value(
                    &mut arguments,
                    "--object-settings",
                )?))
            }
            "--brim" => o.brim = Some(count(&next_value(&mut arguments, "--brim")?, "--brim")?),
            "--brim-gap" => {
                o.brim_gap = Some(number(
                    &next_value(&mut arguments, "--brim-gap")?,
                    "--brim-gap",
                )?)
            }
            "--elephant-foot" => {
                o.elephant_foot = Some(number(
                    &next_value(&mut arguments, "--elephant-foot")?,
                    "--elephant-foot",
                )?)
            }
            "--xy-size" => {
                o.xy_size = Some(number(
                    &next_value(&mut arguments, "--xy-size")?,
                    "--xy-size",
                )?)
            }
            "--closing-radius" => {
                o.closing_radius = Some(number(
                    &next_value(&mut arguments, "--closing-radius")?,
                    "--closing-radius",
                )?)
            }
            "--close-gaps" => {
                o.close_gaps = Some(number(
                    &next_value(&mut arguments, "--close-gaps")?,
                    "--close-gaps",
                )?)
            }
            "--resolution" => {
                o.resolution = Some(number(
                    &next_value(&mut arguments, "--resolution")?,
                    "--resolution",
                )?)
            }
            "--max-bridge-length" => {
                o.max_bridge_length = Some(number(
                    &next_value(&mut arguments, "--max-bridge-length")?,
                    "--max-bridge-length",
                )?)
            }
            "--bridge-cross-hatch" => {
                o.cross_hatch = Some(
                    match next_value(&mut arguments, "--bridge-cross-hatch")?.as_str() {
                        "1" => true,
                        "0" => false,
                        other => {
                            return Err(format!("invalid --bridge-cross-hatch value: {other}"))
                        }
                    },
                )
            }
            "--support" => {
                o.support = Some(match next_value(&mut arguments, "--support")?.as_str() {
                    "1" | "on" => true,
                    "0" | "off" => false,
                    other => return Err(format!("invalid --support value: {other}")),
                })
            }
            "--support-style" => {
                o.support_style = Some(
                    match next_value(&mut arguments, "--support-style")?.as_str() {
                        "normal" => SupportStyle::Normal,
                        "tree" => SupportStyle::Tree,
                        other => {
                            return Err(format!(
                                "--support-style must be normal or tree, got {other}"
                            ))
                        }
                    },
                )
            }
            "--mode" => {
                let value = next_value(&mut arguments, "--mode")?;
                o.settings.mode = match value.as_str() {
                    "hfw" => Mode::Hfw,
                    "classic" => Mode::Classic,
                    "field" => Mode::Field,
                    other => {
                        return Err(format!("--mode must be hfw, classic or field, got {other}"))
                    }
                };
            }
            "--no-reference" => o.settings.keep_reference = false,
            "--no-infill" => o.no_infill = true,
            "--density" => {
                let value = number(&next_value(&mut arguments, "--density")?, "--density")?;
                if !(0.0..=1.0).contains(&value) {
                    return Err("--density must be within 0..1".to_owned());
                }
                o.density = Some(value);
            }
            "--pattern" => {
                let value = next_value(&mut arguments, "--pattern")?;
                if infill_pattern(&value).is_some() {
                    o.pattern = Some(value);
                } else {
                    return Err(format!(
                        "--pattern must be rectilinear, grid, line, triangle, cubic, gyroid, 3d_honeycomb, concentric, zigzag, crosshatch, honeycomb or monotonic, got {value}"
                    ));
                }
            }
            "--skin-top" => {
                o.skin_top = Some(count(
                    &next_value(&mut arguments, "--skin-top")?,
                    "--skin-top",
                )?)
            }
            "--skin-bottom" => {
                o.skin_bottom = Some(count(
                    &next_value(&mut arguments, "--skin-bottom")?,
                    "--skin-bottom",
                )?)
            }
            "--every" => {
                let value = count(&next_value(&mut arguments, "--every")?, "--every")?;
                if value == 0 {
                    return Err("--every must be at least 1".to_owned());
                }
                o.settings.every = value;
            }
            "--profile" => {
                let path = PathBuf::from(next_value(&mut arguments, "--profile")?);
                let config = PrinterConfig::load(&path).map_err(|error| error.to_string())?;
                o.profile = Some(path);
                o.profile_config = Some(config);
            }
            "--preset" => {
                let path = PathBuf::from(next_value(&mut arguments, "--preset")?);
                let preset = Preset::load(&path).map_err(|error| error.to_string())?;
                let (config, report) = PrinterConfig::from_preset(&preset);
                print_intake(&path, &config, &report);
                o.profile = Some(path);
                o.profile_config = Some(config);
                o.preset_report = Some(report);
            }
            "--preset-strict" => o.preset_strict = true,
            "--bundle" => o.bundle = Some(PathBuf::from(next_value(&mut arguments, "--bundle")?)),
            "--list-bundle" => o.list_bundle = true,
            "--machine" => o.machine = Some(next_value(&mut arguments, "--machine")?),
            "--filament" => o.filament = Some(next_value(&mut arguments, "--filament")?),
            "--process" => o.process = Some(next_value(&mut arguments, "--process")?),
            "--gcode" => o.gcode = Some(PathBuf::from(next_value(&mut arguments, "--gcode")?)),
            "--pause-at" => {
                let value = next_value(&mut arguments, "--pause-at")?;
                o.pause_at = heights(&value).map_err(|e| format!("--pause-at: {e}"))?;
            }
            "--colour-change-at" => {
                let value = next_value(&mut arguments, "--colour-change-at")?;
                o.colour_change_at =
                    heights(&value).map_err(|e| format!("--colour-change-at: {e}"))?;
            }
            "--check-gcode" => {
                o.check_gcode = Some(PathBuf::from(next_value(&mut arguments, "--check-gcode")?))
            }
            "--json" => o.json = true,
            "--settings-surface" => {
                let path = PathBuf::from(next_value(&mut arguments, "--settings-surface")?);
                fs::write(&path, hfw_config::surface::json())
                    .map_err(|error| format!("could not write {}: {error}", path.display()))?;
                outln!("Wrote the settings surface to {}", path.display());
                return Ok(None);
            }
            "--intake-json" => {
                o.intake_json = Some(PathBuf::from(next_value(&mut arguments, "--intake-json")?))
            }
            "--json-paths" => {
                o.json = true;
                o.json_paths = true;
            }
            "--strict" => o.settings.strict_slices = true,
            "--flow" => {
                let value = number(&next_value(&mut arguments, "--flow")?, "--flow")?;
                if !(0.5..=1.5).contains(&value) {
                    return Err("--flow must be within 0.5..1.5".to_owned());
                }
                o.flow = Some(value);
            }
            "--metrics" => {
                o.metrics_res = Some(positive(
                    &next_value(&mut arguments, "--metrics")?,
                    "--metrics",
                )?)
            }
            "--spacing" => {
                o.spacing = Some(positive(
                    &next_value(&mut arguments, "--spacing")?,
                    "--spacing",
                )?)
            }
            "--no-arrange" => o.arrange = false,
            "--gantry-height" => {
                o.gantry_height = Some(positive(
                    &next_value(&mut arguments, "--gantry-height")?,
                    "--gantry-height",
                )?)
            }
            "--help" | "-h" => {
                outln!("{USAGE}");
                return Ok(None);
            }
            unknown if !unknown.starts_with('-') => o.inputs.push(unknown.to_owned()),
            unknown => return Err(format!("unknown option: {unknown}\n\n{USAGE}")),
        }
    }
    let listing = o.list_bundle && o.bundle.is_some();
    if o.check_gcode.is_none() && o.inputs.is_empty() && !listing {
        return Err(format!("missing model path\n\n{USAGE}"));
    }
    if o.preset_strict {
        match o.preset_report.as_ref() {
            None if o.bundle.is_some() => {}
            None => return Err("--preset-strict needs --preset or --bundle".to_owned()),
            Some(report) if !report.is_complete() => {
                return Err(format!(
                    "--preset-strict: {} of {} keys were not applied",
                    report.len() - report.count(Intake::Applied),
                    report.len()
                ))
            }
            Some(_) => {}
        }
    }
    Ok(Some(o))
}

fn run_cli() -> Result<(), String> {
    if std::env::args().any(|argument| argument == "--json" || argument == "--json-paths") {
        HUMAN_TO_STDERR.store(true, Ordering::Relaxed);
    }
    let Some(mut o) = parse_arguments()? else {
        return Ok(());
    };

    if let Some(path) = o.check_gcode.clone() {
        return run_check(&o, &path);
    }
    if o.bundle.is_some() {
        resolve_bundle(&mut o)?;
        if o.list_bundle {
            return Ok(());
        }
    }
    if let Some(path) = o.intake_json.clone() {
        let report = o
            .preset_report
            .as_ref()
            .ok_or_else(|| "--intake-json needs --preset or --bundle".to_owned())?;
        fs::write(&path, json::intake_document(report).to_json())
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    }
    o.apply_project_settings()?;
    if let Some(config) = o.profile_config.clone() {
        o.apply_profile(&config);
    }
    o.apply_overrides();
    if let Some(height) = o.gantry_height {
        if let Some(cfg) = o.profile_config.as_mut() {
            cfg.gantry_height = height;
        }
    }
    if let Some(flow) = o.flow {
        if let Some(cfg) = o.profile_config.as_mut() {
            cfg.flow = flow;
        }
    }
    if let Some(hatch) = o.cross_hatch {
        if let Some(cfg) = o.profile_config.as_mut() {
            cfg.bridge_cross_hatch = hatch;
        }
        if let Some(ip) = o.settings.infill.as_mut() {
            ip.bridge.cross_hatch = hatch;
        }
    }
    if let Some(on) = o.support {
        if !on {
            o.settings.support = None;
        } else if o.settings.support.is_none() {
            o.settings.support = Some(SupportSettings::new(o.settings.wall.w));
        }
    }
    if let (Some(style), Some(sp)) = (o.support_style, o.settings.support.as_mut()) {
        sp.style = style;
    }

    if o.no_infill {
        o.settings.infill = None;
    } else {
        let configured_infill = o
            .profile_config
            .as_ref()
            .map(|cfg| cfg.infill_width)
            .unwrap_or(0.0);
        let nominal = o.settings.wall.w;
        if let Some(ip) = o.settings.infill.as_mut() {
            if let Some(density) = o.density {
                ip.density = density;
            }
            if let Some(pattern) = o.pattern.as_deref() {
                ip.pattern = infill_pattern(pattern).unwrap_or(InfillPattern::Rectilinear);
            }
            ip.width = if configured_infill > 0.0 {
                configured_infill
            } else {
                nominal
            };
            ip.overlap = 0.1 * ip.width;
            ip.skin_res = ip.width / 4.0;
            ip.step = ip.width / 4.0;
            if ip.density == 0.0 && ip.top_layers == 0 && ip.bottom_layers == 0 {
                o.settings.infill = None;
            }
        }
    }
    o.settings.metrics_res = o.metrics_res;

    let unit_scale = if o.units == "inch" { 25.4 } else { 1.0 };
    let shrink = o.profile_config.as_ref().map_or([1.0; 3], |cfg| {
        let factor = |percent: f64| {
            if percent > 0.0 && percent < 100.0 {
                100.0 / (100.0 - percent)
            } else {
                1.0
            }
        };
        [
            factor(cfg.shrinkage_xy),
            factor(cfg.shrinkage_xy),
            factor(cfg.shrinkage_z),
        ]
    });
    let total_scale = unit_scale * o.scale;
    let factors = [
        total_scale * shrink[0],
        total_scale * shrink[1],
        total_scale * shrink[2],
    ];

    // Scaling happens before arranging, because the packer works in bed
    // millimetres and a plate laid out at model size then scaled would overlap
    // itself and could pass a bed check it no longer fits.
    let plate = load_plate(&o, factors)?;
    let mut mesh = Mesh::merged(
        &plate
            .iter()
            .map(|part| part.mesh.clone())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())?;
    if mesh.triangle_count() == 0 {
        return Err("the model contains no triangles".to_owned());
    }

    let before = mesh.diagnostics();
    let repair = if o.repair {
        let (repaired, report) = mesh.repair_basic().map_err(|error| error.to_string())?;
        mesh = repaired;
        Some(report)
    } else {
        None
    };
    let after = mesh.diagnostics();

    let single = o.z_mm.is_some() || o.svg.is_some();
    if single && (o.layer_height.is_some() || o.out_dir.is_some()) {
        return Err("single-layer options cannot be mixed with full-stack options".to_owned());
    }

    if single {
        return run_single(&o, &mesh, before, after, repair, total_scale);
    }
    run_stack(&o, &mesh, &plate, before, after, repair, total_scale)
}

/// One body on the plate, with the placement it prints at.
struct PlatePart {
    /// Model object id from the file, or 0 for an STL that has no ids.
    id: u32,
    /// File stem, or the object name the 3MF gave it.
    name: String,
    /// Geometry already in plate coordinates.
    mesh: Mesh,
    /// XY bounds in plate coordinates: `(min x, min y, max x, max y)`.
    bounds: (f64, f64, f64, f64),
    /// True when the input file placed it, so arranging leaves it alone.
    positioned: bool,
    /// Settings this object prints with: the plate's, plus its own overrides.
    settings: Settings,
    /// Override keys that were applied, for the report.
    applied_overrides: Vec<String>,
    /// Override keys this engine cannot apply per object, for the report.
    ignored_overrides: Vec<String>,
}

impl PlatePart {
    fn new(
        id: u32,
        name: String,
        mesh: Mesh,
        positioned: bool,
        settings: Settings,
    ) -> Result<Self, String> {
        let bounds = mesh
            .xy_bounds_mm()
            .ok_or_else(|| format!("{name} has no vertices"))?;
        Ok(Self {
            id,
            name,
            mesh,
            bounds,
            positioned,
            settings,
            applied_overrides: Vec::new(),
            ignored_overrides: Vec::new(),
        })
    }

    fn refresh_bounds(&mut self) {
        if let Some(bounds) = self.mesh.xy_bounds_mm() {
            self.bounds = bounds;
        }
    }

    fn has_overrides(&self) -> bool {
        !self.applied_overrides.is_empty()
    }
}

/// Gap left between automatically placed objects, in millimetres.
const PLATE_SPACING_MM: f64 = 5.0;

/// Tolerance around an object's box when deciding which object a path belongs
/// to, in millimetres. Perimeter, brim and support paths stick out past the
/// model, so a strict box test would mislabel them.
const OBJECT_LABEL_TOLERANCE_MM: f64 = 5.0;

fn load_plate(o: &Options, factors: [f64; 3]) -> Result<Vec<PlatePart>, String> {
    let mut parts = Vec::new();
    for input in &o.inputs {
        let path = PathBuf::from(input);
        let is_3mf = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("3mf"));
        if is_3mf {
            let overrides = hfw_engine::three_mf_metadata(&path)
                .map(|parts| hfw_config::object_overrides(&parts))
                .unwrap_or_default();
            for object in hfw_engine::load_3mf_objects(&path).map_err(|error| error.to_string())? {
                let override_ = overrides.iter().find(|override_| override_.id == object.id);
                let name = match override_.and_then(|override_| override_.name.clone()) {
                    Some(named) => named,
                    None => object.name,
                };
                let mut part =
                    PlatePart::new(object.id, name, object.mesh, true, o.settings.clone())?;
                if let Some(override_) = override_ {
                    apply_object_overrides(&mut part, &override_.pairs);
                }
                parts.push(part);
            }
        } else {
            let mesh = Mesh::load_model(&path).map_err(|error| error.to_string())?;
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("model")
                .to_owned();
            parts.push(PlatePart::new(0, name, mesh, false, o.settings.clone())?);
        }
    }
    if parts.is_empty() {
        return Err("no model path".to_owned());
    }
    if let Some(path) = &o.object_settings {
        apply_object_settings_file(&mut parts, path)?;
    }
    scale_plate(&mut parts, factors)?;
    arrange_plate(&mut parts, o)?;
    seat_on_bed(&mut parts)?;
    for (index, part) in parts.iter().enumerate() {
        for other in &parts[index + 1..] {
            if boxes_overlap(part.bounds, other.bounds) {
                outln!(
                    "warning: {} and {} share the same space on the plate; they print on top of each other \
                     (use --spacing without --no-arrange to lay them out)",
                    part.name, other.name
                );
            }
        }
    }
    for part in &parts {
        if part.has_overrides() {
            outln!(
                "Object {} (#{}) overrides: {}.",
                part.name,
                part.id,
                part.applied_overrides.join(", ")
            );
        }
        if !part.ignored_overrides.is_empty() {
            outln!(
                "  {} override(s) on {} need a feature this engine does not have per object: {}.",
                part.ignored_overrides.len(),
                part.name,
                part.ignored_overrides.join(", ")
            );
        }
    }
    Ok(parts)
}

/// Scales every object's mesh, then its bounding box.
///
/// Units, `--scale` and the profile's shrinkage are one multiplication each axis,
/// so they are applied together and once, before anything reads an object's size.
fn scale_plate(parts: &mut [PlatePart], factors: [f64; 3]) -> Result<(), String> {
    if factors
        .iter()
        .all(|value| (*value - 1.0).abs() <= f64::EPSILON)
    {
        return Ok(());
    }
    for part in parts {
        part.mesh = part
            .mesh
            .transformed(MeshTransform {
                scale: factors,
                ..MeshTransform::default()
            })
            .map_err(|error| error.to_string())?;
        part.refresh_bounds();
    }
    Ok(())
}

/// Lifts any object that hangs below the bed until it rests on it.
///
/// The bed is z=0, so an object placed partly underneath it cannot be printed as
/// it stands. Lifting only the objects that need it keeps every other object
/// where the file put it; lifting the plate by its lowest point instead would
/// move objects that had nothing wrong with them, and would change the height
/// relationship between parts that a plate's layout exists to fix.
fn seat_on_bed(parts: &mut [PlatePart]) -> Result<(), String> {
    for part in parts.iter_mut() {
        let Some((z_min, _)) = part.mesh.z_bounds() else {
            continue;
        };
        let below_nm = -z_min;
        if below_nm <= 0 {
            continue;
        }
        let lift = below_nm as f64 / 1_000_000.0;
        part.mesh = part
            .mesh
            .transformed(MeshTransform {
                translation_mm: [0.0, 0.0, lift],
                ..MeshTransform::default()
            })
            .map_err(|error| error.to_string())?;
        part.refresh_bounds();
        outln!(
            "warning: {} hangs {lift:.3} mm below the bed; lifted to sit on it, which \
             moves it relative to the rest of the plate",
            part.name
        );
    }
    Ok(())
}

/// Applies a project's per-object settings to one object's planner settings.
///
/// Only settings the planner can actually honour per object are read; anything
/// else is counted and named, never silently dropped. Layer height is refused
/// on purpose: the whole plate shares one layer schedule, so a per-object value
/// would either be ignored or desynchronise the stack.
/// Reads per-object settings another application holds for a plate.
///
/// A 3MF carries its own per-object settings; a plate built from separate models
/// - which is what next-layer hands this engine - has nowhere to put them.
///
/// One line per setting keeps the format small enough to read and to write:
///
/// ```text
/// # <input> is the 1-based order of the model paths given
/// 1 sparse_infill_density = 50%
/// 2 wall_loops = 4
/// ```
fn apply_object_settings_file(parts: &mut [PlatePart], path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (which, setting) = line.split_once(char::is_whitespace).ok_or_else(|| {
            format!(
                "{} line {}: expected `<input> <setting> = <value>`",
                path.display(),
                line_number + 1
            )
        })?;
        let which: usize = which.parse().map_err(|_| {
            format!(
                "{} line {}: `{which}` is not an input number",
                path.display(),
                line_number + 1
            )
        })?;
        let (key, value) = setting.split_once('=').ok_or_else(|| {
            format!(
                "{} line {}: expected `<setting> = <value>` after the input number",
                path.display(),
                line_number + 1
            )
        })?;
        let count = parts.len();
        let part = which
            .checked_sub(1)
            .and_then(|index| parts.get_mut(index))
            .ok_or_else(|| {
                format!(
                    "{} line {}: input {which} is not on the plate ({count} model(s))",
                    path.display(),
                    line_number + 1
                )
            })?;
        apply_object_overrides(part, &[(key.trim().to_owned(), value.trim().to_owned())]);
    }
    Ok(())
}

fn apply_object_overrides(part: &mut PlatePart, pairs: &[(String, String)]) {
    for (key, value) in pairs {
        match part.settings.apply_key(key, value) {
            Ok(()) => part.applied_overrides.push(key.clone()),
            Err(_) => part.ignored_overrides.push(format!("{key} = {value}")),
        }
    }
}

/// Maps a profile's or the command line's pattern name to the engine's pattern.
///
/// Places the objects no file positioned, packing them onto the bed.
///
/// A 3MF already carries a plate layout, so those objects keep their coordinates
/// and the STLs are laid out to the right of them, wrapping as the bed runs out.
/// When nothing was positioned — several STLs, or one, which is the common case —
/// every object is packed instead: the packer turns one a quarter turn when that
/// is what makes it fit, and the arrangement is centred on the bed. A part that
/// cannot be made to fit is refused here, naming its size, rather than being
/// emitted and rejected later by the writer's fit check.
fn arrange_plate(parts: &mut [PlatePart], o: &Options) -> Result<(), String> {
    if !o.arrange {
        return Ok(());
    }
    let config = o.profile_config.clone().unwrap_or_default();
    let bed = (config.bed_x, config.bed_y);
    let spacing = o.spacing.unwrap_or(PLATE_SPACING_MM);
    let footprints: Vec<hfw_plate::arrange::Footprint> = parts
        .iter()
        .map(|part| hfw_plate::arrange::Footprint {
            bounds: part.bounds,
            positioned: part.positioned,
        })
        .collect();
    let transforms = hfw_plate::arrange::layout(&footprints, bed, spacing).map_err(|error| {
        format!(
            "{} does not fit on a {} x {} bed: it needs {:.1} x {:.1} mm with the \
             spacing this plate uses, and leaving it out would print a different job",
            parts[error.item].name, bed.0, bed.1, error.footprint.0, error.footprint.1
        )
    })?;
    for (part, transform) in parts.iter_mut().zip(transforms) {
        if part.positioned {
            continue;
        }
        part.mesh = part
            .mesh
            .transformed(transform)
            .map_err(|error| error.to_string())?;
        part.refresh_bounds();
    }
    Ok(())
}

/// Labels every planned path with the object it belongs to.
///
/// The engine slices the plate as one body, so object identity has to be put back
/// afterwards: a path's centroid falls inside exactly one object's box on a
/// normal plate, and the nearest box decides when it does not. The label only
/// changes print order, never geometry, so a wrong guess is a worse travel plan
/// rather than a wrong part.
fn label_objects(job: &mut Job, plate: &[PlatePart]) {
    if plate.len() < 2 {
        return;
    }
    for layer in &mut job.layers {
        for line in &mut layer.paths {
            if line.pts.is_empty() {
                continue;
            }
            let mut sum_x = 0.0;
            let mut sum_y = 0.0;
            for point in &line.pts {
                sum_x += point.x;
                sum_y += point.y;
            }
            let count = line.pts.len() as f64;
            line.object = object_at(sum_x / count, sum_y / count, plate);
        }
    }
}

/// Plans a plate in `by object` order: one whole object, then the next.
///
/// Plans a plate object by object and assembles the file in `order`.
///
/// This is the path taken whenever an object carries its own settings, and
/// whenever `print_sequence = by object`. Each object is sliced and planned on
/// its own, so its own wall count, infill density or support decision applies to
/// it alone; the stacks are then interleaved by layer, or concatenated.
///
/// By-object order is checked first, because both rules there are collisions if
/// wrong, and the check itself lives in `hfw_plate::check_sequential` so this path
/// and the C ABI cannot disagree about what is safe. Nothing is planned before it
/// passes: a refused plate produces no G-code at all.
fn plate_job(
    o: &Options,
    plate: &[PlatePart],
    first_layer_height: f64,
    layer_height: f64,
    diagnostics: MeshDiagnostics,
    order: PlateOrder,
) -> Result<Job, String> {
    if order == PlateOrder::ByObject {
        if o.profile_config.is_none() {
            return Err("print_sequence = by object needs --profile".to_owned());
        }
        let clearance = hfw_plate::Clearance::of(&o.settings).map_err(|error| error.to_string())?;
        let mut placed = Vec::with_capacity(plate.len());
        for part in plate {
            let height = part
                .mesh
                .z_bounds()
                .map(|(low, high)| (high - low) as f64 / 1_000_000.0)
                .unwrap_or(0.0);
            placed.push((part.name.as_str(), part.bounds, height));
        }
        hfw_plate::check_sequential(clearance, placed).map_err(|error| error.to_string())?;
    }

    let mut stacks = Vec::with_capacity(plate.len());
    for part in plate {
        stacks.push(
            run(&part.mesh, first_layer_height, layer_height, &part.settings)
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(hfw_engine::merge_stacks(stacks, order, diagnostics))
}

/// Deposited volume per object, in cubic millimetres, in plate order.
///
/// Read out of the planned paths rather than the meshes, so it reports what the
/// file will actually deposit: a per-object infill override shows up here as one
/// part holding more plastic than the other.
fn object_volumes(job: &Job, plate: &[PlatePart]) -> Vec<f64> {
    let mut volumes = vec![0.0; plate.len()];
    for layer in &job.layers {
        let height = layer.height();
        for line in &layer.paths {
            if let Some(slot) = volumes.get_mut(usize::from(line.object)) {
                *slot += hfw_engine::line_volume(line, height);
            }
        }
    }
    volumes
}

fn far_enough_apart(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64), spacing: f64) -> bool {
    let gap_x = (b.0 - a.2).max(a.0 - b.2);
    let gap_y = (b.1 - a.3).max(a.1 - b.3);
    gap_x >= spacing || gap_y >= spacing
}

fn heights(value: &str) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let height: f64 = entry
            .parse()
            .map_err(|_| format!("`{entry}` is not a Z height in millimetres"))?;
        if !height.is_finite() || height <= 0.0 {
            return Err(format!("`{entry}` is not a height above the bed"));
        }
        out.push(height);
    }
    if out.is_empty() {
        return Err("no heights given".to_owned());
    }
    Ok(out)
}

fn boxes_overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    !far_enough_apart(a, b, 0.0)
}

fn object_at(x: f64, y: f64, plate: &[PlatePart]) -> u16 {
    let mut best = 0u16;
    let mut best_distance = f64::INFINITY;
    for (index, part) in plate.iter().enumerate() {
        let (min_x, min_y, max_x, max_y) = part.bounds;
        if x >= min_x - OBJECT_LABEL_TOLERANCE_MM
            && x <= max_x + OBJECT_LABEL_TOLERANCE_MM
            && y >= min_y - OBJECT_LABEL_TOLERANCE_MM
            && y <= max_y + OBJECT_LABEL_TOLERANCE_MM
        {
            return u16::try_from(index).unwrap_or(u16::MAX);
        }
        let centre_x = 0.5 * (min_x + max_x);
        let centre_y = 0.5 * (min_y + max_y);
        let distance = (x - centre_x) * (x - centre_x) + (y - centre_y) * (y - centre_y);
        if distance < best_distance {
            best_distance = distance;
            best = u16::try_from(index).unwrap_or(u16::MAX);
        }
    }
    best
}

fn run_check(o: &Options, path: &PathBuf) -> Result<(), String> {
    let cfg = o
        .profile_config
        .as_ref()
        .ok_or_else(|| "--check-gcode needs --profile".to_owned())?;
    let text = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let report = hfw_gcode::verify(&text, cfg);
    outln!("Checked {} against profile '{}'.", path.display(), cfg.name);
    outln!(
        "  {} layers, {} moves, {:.1} mm filament ({:.0} mm^3), estimated {:.0} s",
        report.layers,
        report.moves,
        report.filament_mm,
        report.volume_mm3,
        report.est_seconds
    );
    outln!(
        "  {} travels cross a printed perimeter",
        report.travel_crossings
    );
    outln!(
        "  bbox X {:.2}..{:.2}  Y {:.2}..{:.2}  Z {:.2}..{:.2}",
        report.bbox_min[0],
        report.bbox_max[0],
        report.bbox_min[1],
        report.bbox_max[1],
        report.bbox_min[2],
        report.bbox_max[2]
    );
    for warning in &report.warnings {
        outln!("  warning: {warning}");
    }
    for error in &report.errors {
        outln!("  error: {error}");
    }
    if o.json {
        println!("{}", json::check_document(path, &report).to_json());
    }
    if report.ok() {
        outln!("verify RESULT PASS");
        Ok(())
    } else {
        outln!("verify RESULT FAIL");
        Err(format!("{} G-code error(s)", report.errors.len()))
    }
}

fn run_single(
    o: &Options,
    mesh: &Mesh,
    before: MeshDiagnostics,
    after: MeshDiagnostics,
    repair: Option<MeshRepairReport>,
    total_scale: f64,
) -> Result<(), String> {
    let z_mm = o
        .z_mm
        .ok_or_else(|| "missing required --z value".to_owned())?;
    let svg_path = o
        .svg
        .as_ref()
        .ok_or_else(|| "missing required --svg path".to_owned())?;

    let (slice, region) = slice_plane(mesh, z_mm).map_err(|error| error.to_string())?;
    let walls = hfw_engine::generate_walls(&region, &o.settings.wall, o.settings.mode);
    let mut planned = PlannedSlice {
        index: 0,
        bottom_z: z_mm,
        slice_z: z_mm,
        print_z: z_mm,
        reference: if o.settings.keep_reference {
            region.rings.clone()
        } else {
            Vec::new()
        },
        paths: walls.lines,
        counts: Default::default(),
        vase: false,
        first: true,
    };
    if let Some(ip) = o.settings.infill {
        let regions = vec![region.clone()];
        let cache = MaskCache::new(&regions, ip.skin_res, ip.top_layers + ip.bottom_layers + 1);
        planned.paths.extend(hfw_engine::infill_layer(
            &region,
            &cache,
            0,
            0.0,
            &o.settings.wall,
            &ip,
            None,
        ));
    }
    planned.recount();
    fs::write(svg_path, planned.to_svg())
        .map_err(|error| format!("could not write {}: {error}", svg_path.display()))?;

    print_mesh("Mesh", after);
    match repair {
        Some(report) => print_repair(report),
        None => outln!("Basic repair was not requested; the imported mesh was not modified."),
    }
    if before != after {
        outln!("Mesh changed during preparation.");
    }
    outln!(
        "Sliced {} triangles at Z={z_mm:.6} mm: {} contours, {} duplicate segments removed.",
        mesh.triangle_count(),
        slice.contours.len(),
        slice.diagnostics.duplicate_segments_removed
    );
    outln!(
        "Planned {} paths in {:.1} ms: {} walls, {} middle, {} skin, {} infill.",
        planned.paths.len(),
        walls.stats.ms_total,
        planned.counts.walls,
        planned.counts.middle,
        planned.counts.skin,
        planned.counts.infill
    );
    outln!(
        "Wall loops {}, gap strands {}; {} vertices in {:.1} ms of offsetting.",
        walls.stats.loops,
        walls.stats.gaps,
        walls.stats.points,
        walls.stats.ms_offset
    );
    if let Some(res) = o.settings.metrics_res {
        let m = evaluate_perimeter(&region, &planned.paths, &o.settings.wall, res);
        let full = evaluate_all(&region, &planned.paths, &o.settings.wall, res);
        print_metrics(&m);
        outln!(
            "All paths at {res} mm: covered {:.4}, under {:.4}, over {:.4}, spill {:.4}.",
            full.bead_area / full.area_region.max(1e-9),
            full.under,
            full.over,
            full.spill
        );
    }
    outln!("Wrote {}", svg_path.display());
    outln!("No printable G-code was generated.");
    if o.json {
        let rings: Vec<Vec<(f64, f64)>> = region
            .rings
            .iter()
            .map(|ring| ring.iter().map(|p| (p.x, p.y)).collect())
            .collect();
        let context = json::Context {
            inputs: o.inputs.clone(),
            units: o.units.clone(),
            scale: total_scale,
            vertices: after.vertices,
            triangles: after.triangles,
            mesh_volume: 0.0,
            verify: None,
            gcode: None,
        };
        let document = json::slice_document(
            &context,
            &o.settings,
            &planned,
            &rings,
            slice.diagnostics.duplicate_segments_removed,
        );
        println!("{}", document.to_json());
    }
    Ok(())
}

fn run_stack(
    o: &Options,
    mesh: &Mesh,
    plate: &[PlatePart],
    before: MeshDiagnostics,
    after: MeshDiagnostics,
    repair: Option<MeshRepairReport>,
    total_scale: f64,
) -> Result<(), String> {
    let layer_height = o
        .layer_height
        .ok_or_else(|| "missing required --layer-height value".to_owned())?;
    let first_layer_height = o.first_layer_height.unwrap_or(layer_height);
    // Layer SVGs are for looking at a slice; a run that only wants the G-code has
    // nothing to put in a folder.
    let out_dir = o.out_dir.as_ref();

    let by_object = o
        .profile_config
        .as_ref()
        .is_some_and(|cfg| cfg.print_sequence == PrintSequence::ByObject);
    let per_object = plate.iter().any(|part| part.has_overrides());
    let order = if by_object {
        PlateOrder::ByObject
    } else {
        PlateOrder::ByLayer
    };
    let job = if plate.len() > 1 && (by_object || per_object) {
        plate_job(o, plate, first_layer_height, layer_height, before, order)?
    } else {
        let mut job = run(mesh, first_layer_height, layer_height, &o.settings)
            .map_err(|error| error.to_string())?;
        label_objects(&mut job, plate);
        job
    };
    if plate.len() > 1 {
        let names: Vec<&str> = plate.iter().map(|part| part.name.as_str()).collect();
        let order = if by_object {
            "one object at a time"
        } else {
            "one layer at a time"
        };
        outln!(
            "Plate: {} objects ({}) printed {order}.",
            plate.len(),
            names.join(", ")
        );
        let volumes = object_volumes(&job, plate);
        let per_part: Vec<String> = plate
            .iter()
            .zip(volumes)
            .map(|(part, volume)| format!("{} {:.0} mm^3", part.name, volume))
            .collect();
        outln!("Per object: {}.", per_part.join(", "));
    }
    let mesh_volume = mesh.volume_mm3();

    let mut totals = JobTotals::default();
    if let Some(out_dir) = out_dir {
        fs::create_dir_all(out_dir)
            .map_err(|error| format!("could not create {}: {error}", out_dir.display()))?;

        let mut manifest = String::from(MANIFEST_HEADER);
        manifest.push('\n');
        for layer in &job.layers {
            let path = out_dir.join(format!("layer_{:05}.svg", layer.index + 1));
            fs::write(&path, layer.to_svg())
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            manifest.push_str(&layer.manifest_row());
            manifest.push('\n');
            totals.add(layer);
        }
        let manifest_path = out_dir.join("manifest.csv");
        fs::write(&manifest_path, manifest)
            .map_err(|error| format!("could not write {}: {error}", manifest_path.display()))?;

        let report = job_report(
            &ReportInput {
                options: o,
                mesh,
                before,
                after,
                repair,
                total_scale,
                mesh_volume,
            },
            &job,
            &totals,
        );
        let report_path = out_dir.join("job_report.txt");
        fs::write(&report_path, report)
            .map_err(|error| format!("could not write {}: {error}", report_path.display()))?;
    }

    outln!(
        "Planned {} layers at {first_layer_height:.3} mm first layer then {layer_height:.3} mm.",
        job.layers.len()
    );
    outln!(
        "Paths: {} walls, {} middle, {} skin, {} infill; {:.1} m of extrusion in {:.1} s.",
        job.layers.iter().map(|l| l.counts.walls).sum::<usize>(),
        totals.middle,
        totals.skin,
        totals.infill,
        totals.length / 1000.0,
        (job.stats.ms_slice + job.stats.ms_walls_elapsed + job.stats.ms_infill) / 1000.0
    );
    let extruded = job.volume.0 + job.volume.1;
    outln!(
        "Deposited volume {extruded:.2} mm^3 (perimeter {:.2}, interior {:.2}); {} starts/stops.",
        job.volume.0,
        job.volume.1,
        totals.starts_stops
    );
    if mesh_volume > 0.0 {
        outln!(
            "extruded_over_mesh={:.4} (mesh {mesh_volume:.1} mm^3; only meaningful with --every 1 and --density 1.0)",
            extruded / mesh_volume
        );
    }
    outln!(
        "Slowest layer {} took {:.1} ms; {} of {} layers planned.",
        job.worst_layer + 1,
        job.worst_layer_ms,
        job.stats.processed_layers,
        job.stats.layers
    );
    if !job.skipped.is_empty() {
        outln!(
            "WARNING: {} layer(s) had no closed cross-section and were left empty:",
            job.skipped.len()
        );
        for (index, reason) in job.skipped.iter().take(5) {
            outln!("  layer {}: {reason}", index + 1);
        }
        if job.skipped.len() > 5 {
            outln!("  ... and {} more", job.skipped.len() - 5);
        }
    }
    if !job.metrics.is_empty() {
        let n = job.metrics.len() as f64;
        let mean_under = job.metrics.iter().map(|m| m.under).sum::<f64>() / n;
        let mean_over = job.metrics.iter().map(|m| m.over).sum::<f64>() / n;
        let mean_viol = job.metrics.iter().map(|m| m.width_violation).sum::<f64>() / n;
        outln!(
            "Mean perimeter coverage: under {mean_under:.4}, over {mean_over:.4}, width violation {mean_viol:.4}."
        );
    }
    let positioned = plate.iter().any(|part| part.positioned);
    let written = match &o.gcode {
        Some(path) => Some(write_gcode(o, &job, path, positioned)?),
        None => {
            outln!("No printable G-code was generated (no --profile and --gcode).");
            None
        }
    };
    if o.json {
        let context = json::Context {
            inputs: o.inputs.clone(),
            units: o.units.clone(),
            scale: total_scale,
            vertices: after.vertices,
            triangles: after.triangles,
            mesh_volume,
            verify: written.as_ref().map(|w| &w.report),
            gcode: written.as_ref().map(|w| (&w.stats, Some(w.path.as_path()))),
        };
        let document = json::document(
            &context,
            &o.settings,
            o.profile_config.as_ref(),
            &job,
            &totals,
            o.json_paths,
        );
        println!("{}", document.to_json());
    }
    match &written {
        Some(w) if !w.report.ok() => Err(format!(
            "{} G-code error(s); the file was written but must not be printed",
            w.report.errors.len()
        )),
        _ => Ok(()),
    }
}

/// What `write_gcode` produced, so the caller can report it and decide the exit code.
struct Written {
    stats: hfw_gcode::GcodeStats,
    report: hfw_gcode::VerifyReport,
    path: PathBuf,
}

fn write_gcode(o: &Options, job: &Job, path: &Path, positioned: bool) -> Result<Written, String> {
    let base = o
        .profile_config
        .as_ref()
        .ok_or_else(|| "--gcode needs --profile".to_owned())?;
    let mut marked = base.clone();
    marked.pause_heights = o.pause_at.clone();
    marked.colour_change_heights = o.colour_change_at.clone();
    marked.keep_position = positioned;
    let cfg = &marked;
    if o.settings.every != 1 {
        return Err("--gcode needs every layer planned; drop --every".to_owned());
    }
    let object = std::path::Path::new(o.inputs.first().map(String::as_str).unwrap_or("plate"))
        .file_stem()
        .and_then(|s| s.to_str());
    let written =
        hfw_gcode::generate_with_markers(&job.layers, cfg, object, &job.seam_paint, &job.markers)
            .map_err(|error| error.to_string())?;
    let path = output_path(cfg, o, path)?;
    fs::write(&path, &written.text)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    run_post_process(cfg, &path)?;
    let s = &written.stats;
    outln!(
        "G-code: {} layers, {:.1} mm filament ({:.2} g, {:.0} mm^3), estimated {:.1} min, {:.0} mm travel, {} retractions.",
        s.layers,
        s.filament_mm,
        s.filament_g,
        s.volume_mm3,
        s.est_seconds / 60.0,
        s.travel_mm,
        s.retractions
    );
    outln!(
        "Placed footprint X {:.2}..{:.2}, Y {:.2}..{:.2}, top Z {:.2} on a {} x {} bed.",
        s.bbox_min.x,
        s.bbox_max.x,
        s.bbox_min.y,
        s.bbox_max.y,
        s.top_z,
        cfg.bed_x,
        cfg.bed_y
    );
    let report = hfw_gcode::verify(&written.text, cfg);
    for finding in cfg.audit() {
        outln!("  audit: {}", finding.line());
    }
    outln!(
        "  {} travels cross a printed perimeter",
        report.travel_crossings
    );
    for warning in &report.warnings {
        outln!("  warning: {warning}");
    }
    for error in &report.errors {
        outln!("  error: {error}");
    }
    if report.ok() {
        outln!("verify RESULT PASS (independent check of the written file)");
    } else {
        outln!("verify RESULT FAIL");
    }
    outln!("Wrote {}", path.display());
    if cfg.filament_cost > 0.0 {
        let cost = s.filament_g / 1000.0 * cfg.filament_cost;
        outln!(
            "Cost {:.2} at {:.2}/kg, {:.1} g of the spool used{}",
            cost,
            cfg.filament_cost,
            s.filament_g,
            if cfg.filament_spool_weight > 0.0 {
                format!(
                    ", {:.0} g spool leaves {:.1} g",
                    cfg.filament_spool_weight,
                    cfg.filament_spool_weight - s.filament_g
                )
            } else {
                String::new()
            }
        );
    }
    Ok(Written {
        stats: written.stats,
        report,
        path: path.to_path_buf(),
    })
}

/// Applies `output_filename_format` to the path a caller did not name.
///
/// The dialect writes the name as a template over `{input_filename}`,
/// `{layer_height}`, `{nozzle_diameter}`, `{printer_model}` and
/// `{filament_type}`. It applies only when the caller asked for a directory
/// rather than a file: `--gcode out.gcode` is an instruction, not a suggestion,
/// and a caller that passes it — another program, a test, a script — cannot be
/// expected to hunt for the file it asked for. A format still holding a
/// placeholder this engine cannot fill keeps the caller's name instead of
/// writing braces into a path.
fn output_path(cfg: &PrinterConfig, o: &Options, asked: &Path) -> Result<PathBuf, String> {
    let format = cfg.output_filename_format.trim();
    let named = asked
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gcode"));
    if format.is_empty() || named {
        return Ok(asked.to_path_buf());
    }
    let stem = std::path::Path::new(o.inputs.first().map(String::as_str).unwrap_or("plate"))
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("plate")
        .to_owned();
    let year = 0;
    let _ = year;
    let name = format
        .replace("{input_filename}", &stem)
        .replace("{layer_height}", &format!("{}mm", cfg.layer_height))
        .replace("{nozzle_diameter}", &format!("{}mm", cfg.nozzle_diameter))
        .replace("{printer_model}", cfg.printer_model.trim())
        .replace("{filament_type}", cfg.filament_type.trim());
    let name = name.trim();
    if name.contains('{') || name.contains('}') {
        return Ok(asked.to_path_buf());
    }
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(format!(
            "output_filename_format `{format}` must produce a plain file name"
        ));
    }
    if named {
        return Ok(asked.to_path_buf());
    }
    Ok(asked.join(name))
}

/// Runs `post_process` on the finished file, one command per `;`-separated entry.
fn run_post_process(cfg: &PrinterConfig, path: &Path) -> Result<(), String> {
    for command in cfg.post_process.split(';') {
        let command = command.trim();
        if command.is_empty() {
            continue;
        }
        let mut words: Vec<String> = command.split_whitespace().map(|w| w.to_owned()).collect();
        if words.is_empty() {
            continue;
        }
        let program = words.remove(0);
        let status = std::process::Command::new(&program)
            .args(words)
            .arg(path)
            .status()
            .map_err(|error| format!("post_process `{command}` did not start: {error}"))?;
        if !status.success() {
            return Err(format!("post_process `{command}` exited with {status}"));
        }
        outln!("Ran post_process `{command}`");
    }
    Ok(())
}

struct ReportInput<'a> {
    options: &'a Options,
    mesh: &'a Mesh,
    before: MeshDiagnostics,
    after: MeshDiagnostics,
    repair: Option<MeshRepairReport>,
    total_scale: f64,
    mesh_volume: f64,
}

fn job_report(input: &ReportInput, job: &Job, totals: &JobTotals) -> String {
    let ReportInput {
        options: o,
        mesh,
        before,
        after,
        repair,
        total_scale,
        mesh_volume,
    } = *input;
    let mut text = String::new();
    let _ = writeln!(text, "HFW engine — job report");
    let _ = writeln!(text, "Input: {}", o.inputs.join(", "));
    let _ = writeln!(
        &mut text,
        "Input units: {}; combined uniform scale: {total_scale:.6}",
        o.units
    );
    let _ = writeln!(
        &mut text,
        "Mesh: {} vertices, {} triangles",
        mesh.vertex_count(),
        mesh.triangle_count()
    );
    let _ = writeln!(
        &mut text,
        "Extrusion width: {:.3} mm; walls: {}; mode: {:?}",
        o.settings.wall.w, o.settings.wall.walls, o.settings.mode
    );
    let _ = writeln!(
        &mut text,
        "Infill: {}",
        match o.settings.infill {
            None => "disabled".to_owned(),
            Some(ip) => format!(
                "{:?} at density {:.3}, {} top and {} bottom skin layers",
                ip.pattern, ip.density, ip.top_layers, ip.bottom_layers
            ),
        }
    );
    let _ = writeln!(
        &mut text,
        "Slice simplification: {:.4} mm; brim raster: {:.4} mm",
        o.settings.resolution, o.settings.wall.h
    );
    let _ = writeln!(&mut text, "Layers: {}", job.layers.len());
    if !job.skipped.is_empty() {
        let _ = writeln!(
            &mut text,
            "Layers skipped for an unclosed cross-section: {}",
            job.skipped.len()
        );
        for (index, reason) in &job.skipped {
            let _ = writeln!(&mut text, "  layer {}: {reason}", index + 1);
        }
    }
    let _ = writeln!(
        &mut text,
        "Triangles tested across the stack: {}",
        job.stats.triangles_tested
    );
    let _ = writeln!(
        &mut text,
        "Wall generation: {:.0} ms of work on {} threads in {:.0} ms, slowest layer {} at {:.1} ms",
        job.stats.ms_walls,
        job.stats.threads,
        job.stats.ms_walls_elapsed,
        job.worst_layer + 1,
        job.worst_layer_ms
    );
    let _ = writeln!(
        &mut text,
        "Timing: slice {:.0} ms, compensating {:.0} ms, walls {:.0} ms of work in {:.0} ms, infill {:.0} ms, metrics {:.0} ms",
        job.stats.ms_slice,
        job.stats.ms_offset,
        job.stats.ms_walls,
        job.stats.ms_walls_elapsed,
        job.stats.ms_infill,
        job.stats.ms_metrics
    );
    let _ = writeln!(
        &mut text,
        "Paths: {} walls, {} middle, {} skin, {} infill",
        job.layers.iter().map(|l| l.counts.walls).sum::<usize>(),
        totals.middle,
        totals.skin,
        totals.infill
    );
    let _ = writeln!(
        &mut text,
        "Extrusion: {:.1} mm length, {} starts/stops",
        totals.length, totals.starts_stops
    );
    let _ = writeln!(
        &mut text,
        "Deposited volume: {:.3} mm^3 (perimeter {:.3}, interior {:.3})",
        job.volume.0 + job.volume.1,
        job.volume.0,
        job.volume.1
    );
    let _ = writeln!(&mut text, "Mesh volume: {mesh_volume:.3} mm^3");
    if mesh_volume > 0.0 {
        let _ = writeln!(
            &mut text,
            "extruded_over_mesh: {:.4}",
            (job.volume.0 + job.volume.1) / mesh_volume
        );
    }
    let _ = writeln!(
        &mut text,
        "Wall loops: {}; gap strands: {}; wall vertices: {}",
        job.stats.wall_loops, job.stats.gap_strands, job.stats.wall_points
    );
    let _ = writeln!(&mut text, "\nMesh diagnostics before preparation:");
    append_mesh(&mut text, before);
    let _ = writeln!(&mut text, "\nMesh diagnostics after preparation:");
    append_mesh(&mut text, after);
    match repair {
        Some(r) => {
            let _ = writeln!(&mut text, "\nExplicit basic-repair report:");
            let _ = writeln!(&mut text, "  vertices welded: {}", r.vertices_welded);
            let _ = writeln!(
                &mut text,
                "  degenerate and duplicate triangles removed: {} / {}",
                r.degenerate_triangles_removed, r.duplicate_triangles_removed
            );
            let _ = writeln!(
                &mut text,
                "  triangles flipped for consistency / orientation: {} / {}",
                r.triangles_flipped_for_consistency, r.triangles_flipped_for_outward_orientation
            );
            let _ = writeln!(&mut text, "  closed components: {}", r.closed_components);
            let _ = writeln!(
                &mut text,
                "  unresolved non-manifold edges: {}; orientation conflicts: {}",
                r.unresolved_non_manifold_edges, r.orientation_conflicts
            );
        }
        None => {
            let _ = writeln!(&mut text, "\nBasic repair was not requested.");
        }
    }
    let _ = writeln!(
        &mut text,
        "\nThis report describes planned bead paths only. No G-code was generated."
    );
    text
}

fn print_mesh(label: &str, d: MeshDiagnostics) {
    outln!(
        "{label}: {} vertices, {} triangles, {} boundary edges, {} non-manifold edges, \
         {} inconsistent-orientation edges, {} degenerate triangles.",
        d.vertices,
        d.triangles,
        d.boundary_edges,
        d.non_manifold_edges,
        d.inconsistently_oriented_edges,
        d.zero_area_triangles + d.repeated_index_triangles
    );
}

fn append_mesh(text: &mut String, d: MeshDiagnostics) {
    let _ = writeln!(text, "  vertices: {}", d.vertices);
    let _ = writeln!(text, "  triangles: {}", d.triangles);
    let _ = writeln!(text, "  boundary edges: {}", d.boundary_edges);
    let _ = writeln!(text, "  non-manifold edges: {}", d.non_manifold_edges);
    let _ = writeln!(
        text,
        "  inconsistent-orientation edges: {}",
        d.inconsistently_oriented_edges
    );
    let _ = writeln!(
        text,
        "  degenerate triangles: {}",
        d.zero_area_triangles + d.repeated_index_triangles
    );
}

fn print_repair(r: MeshRepairReport) {
    outln!(
        "Basic repair: welded {} vertices, removed {} degenerate and {} duplicate triangles, \
         flipped {} for consistency and {} for outward orientation; {} closed components.",
        r.vertices_welded,
        r.degenerate_triangles_removed,
        r.duplicate_triangles_removed,
        r.triangles_flipped_for_consistency,
        r.triangles_flipped_for_outward_orientation,
        r.closed_components
    );
}

fn print_metrics(m: &Metrics) {
    outln!(
        "Perimeter at {:.0} paths: covered {:.4}, under {:.4}, over {:.4}, spill {:.4}.",
        m.lines,
        m.bead_area / m.area_target.max(1e-9),
        m.under,
        m.over,
        m.spill
    );
    outln!(
        "  width {:.3}..{:.3} mm, violation {:.4} of length, {:.4} mm^2 of transverse variation per mm",
        m.min_width, m.max_width, m.width_violation, m.width_tv_per_mm
    );
}
