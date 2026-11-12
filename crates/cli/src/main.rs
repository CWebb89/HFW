//! Headless command-line front end for the HFW slicing engine.

#![forbid(unsafe_code)]

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use hfw_config::PrinterConfig;
use hfw_engine::{
    evaluate_all, evaluate_perimeter, run, slice_plane, InfillPattern, Job, JobTotals, MaskCache,
    Mesh, MeshDiagnostics, MeshRepairReport, MeshTransform, Metrics, Mode, PlannedSlice, Settings,
    SupportSettings, SupportStyle, MANIFEST_HEADER,
};

const USAGE: &str = "\
Usage:
  One layer:   hfw <model.stl> --z <mm> --svg <output.svg> [options]
  All layers:  hfw <model.stl> --layer-height <mm> --out-dir <folder> [options]
  Print file:  hfw <model.stl> --profile <p.cfg> --gcode <out.gcode>
  Check file:  hfw --check-gcode <out.gcode> --profile <p.cfg>

Slice:
  --z <mm>                    Slice and plan one plane at this height.
  --svg <path>                Write the single-layer SVG here.
  --layer-height <mm>         Regular layer height; plans the whole stack.
  --first-layer-height <mm>   First layer height (default: layer height).
  --out-dir <folder>          Write layer SVGs, manifest.csv and job_report.txt.

Model preparation:
  --units mm|inch             Interpret STL coordinates (default mm).
  --scale <factor>            Extra uniform scale (default 1.0).
  --repair-basic              Run the conservative mesh repair pass first.

Walls:
  --width <mm>                Extrusion width (default 0.4).
  --walls <n>                 Perimeter loops (default 3).
  --brim <n>                  First-layer adhesion brim loops (default 0).
  --brim-gap <mm>             Clearance between part and brim (default 0.1).
  --mode hfw|classic          Variable-width field walls, or plain offsets.
  --no-reference              Omit the exact slice outline from the SVG.

Infill:
  --no-infill                 Skip infill entirely.
  --density <f>               Sparse density in 0..1 (default 0.15).
  --pattern rectilinear|grid  Sparse pattern (default rectilinear).
  --skin-top <n>              Solid layers under a top surface (default 5).
  --skin-bottom <n>           Solid layers above a bottom surface (default 4).
  --max-bridge-length <mm>    Longest span printed as a bridge; longer spans
                                print as ordinary solid infill.
  --bridge-cross-hatch <0|1>  Print two crossing strands across each gap before
                                the bridge fill.
  --support <0|1>             Generate supports under overhangs.
  --support-style <style>     normal (projected columns) or tree.

Printer:
  --profile <path>            Printer, filament and process profile.
  --flow <f>                  Extrusion multiplier 0.5..1.5, overriding the
                              profile. 1.0 prints the geometric volume.
  --gcode <path>              Emit G-code for the whole job (needs --profile,
                              plans every layer, then verifies the result).
  --check-gcode <path>        Verify an existing G-code file and exit.

Evaluation:
  --every <n>                 Plan walls on every Nth layer, plus the last.
  --metrics <mm>              Measure coverage at this sampling resolution.

Other:
  --help                      Show this help.

A profile supplies defaults for width, walls, density, pattern, skin layers and
layer heights; any option given on the command line wins. See profiles/ for
voron_klipper.cfg and generic_marlin.cfg.

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
    input: String,
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
    brim_gap: Option<f64>,
    max_bridge_length: Option<f64>,
    cross_hatch: Option<bool>,
    support: Option<bool>,
    support_style: Option<SupportStyle>,
    skin_top: Option<usize>,
    skin_bottom: Option<usize>,
    metrics_res: Option<f64>,
    profile: Option<PathBuf>,
    profile_config: Option<PrinterConfig>,
    flow: Option<f64>,
    gcode: Option<PathBuf>,
    check_gcode: Option<PathBuf>,
}

impl Options {
    fn base() -> Options {
        Options {
            input: String::new(),
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
            brim_gap: None,
            max_bridge_length: None,
            cross_hatch: None,
            support: None,
            support_style: None,
            skin_top: None,
            skin_bottom: None,
            metrics_res: None,
            profile: None,
            profile_config: None,
            flow: None,
            gcode: None,
            check_gcode: None,
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

    fn apply_profile(&mut self, cfg: &PrinterConfig) {
        self.settings.wall.w = cfg.line_width;
        self.settings.wall.walls = cfg.walls;
        self.settings.brim_loops = cfg.brim_loops;
        self.settings.brim_gap = cfg.brim_gap;
        if let Some(ip) = self.settings.infill.as_mut() {
            ip.density = cfg.infill_density;
            ip.top_layers = cfg.top_layers;
            ip.bottom_layers = cfg.bottom_layers;
            ip.pattern = if cfg.infill_pattern == "grid" {
                InfillPattern::Grid
            } else {
                InfillPattern::Rectilinear
            };
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
        }
        if cfg.support {
            let mut sp = SupportSettings::new(cfg.line_width);
            sp.threshold_angle = cfg.support_threshold_angle;
            sp.xy_gap = cfg.support_xy_gap;
            sp.z_gap = cfg.support_z_gap;
            sp.interface_layers = cfg.support_interface_layers;
            sp.density = cfg.support_density;
            sp.interface_density = cfg.support_interface_density;
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
            self.settings.support = Some(sp);
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
            o.input = arguments.next().unwrap_or_default();
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
            "--brim" => o.brim = Some(count(&next_value(&mut arguments, "--brim")?, "--brim")?),
            "--brim-gap" => {
                o.brim_gap = Some(number(
                    &next_value(&mut arguments, "--brim-gap")?,
                    "--brim-gap",
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
                    other => return Err(format!("--mode must be hfw or classic, got {other}")),
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
                match value.as_str() {
                    "rectilinear" | "grid" => o.pattern = Some(value),
                    other => {
                        return Err(format!(
                            "--pattern must be rectilinear or grid, got {other}"
                        ))
                    }
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
            "--gcode" => o.gcode = Some(PathBuf::from(next_value(&mut arguments, "--gcode")?)),
            "--check-gcode" => {
                o.check_gcode = Some(PathBuf::from(next_value(&mut arguments, "--check-gcode")?))
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
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            unknown => return Err(format!("unknown option: {unknown}\n\n{USAGE}")),
        }
    }
    if o.check_gcode.is_none() && o.input.is_empty() {
        return Err(format!("missing model path\n\n{USAGE}"));
    }
    Ok(Some(o))
}

fn run_cli() -> Result<(), String> {
    let Some(mut o) = parse_arguments()? else {
        return Ok(());
    };

    if let Some(path) = o.check_gcode.clone() {
        return run_check(&o, &path);
    }
    if let Some(config) = o.profile_config.clone() {
        o.apply_profile(&config);
    }
    o.apply_overrides();
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
    } else if let Some(ip) = o.settings.infill.as_mut() {
        if let Some(density) = o.density {
            ip.density = density;
        }
        if let Some(pattern) = o.pattern.as_deref() {
            ip.pattern = if pattern == "grid" {
                InfillPattern::Grid
            } else {
                InfillPattern::Rectilinear
            };
        }
        ip.width = o.settings.wall.w;
        ip.overlap = 0.1 * o.settings.wall.w;
        ip.skin_res = o.settings.wall.w / 4.0;
        ip.step = o.settings.wall.w / 4.0;
        if ip.density == 0.0 && ip.top_layers == 0 && ip.bottom_layers == 0 {
            o.settings.infill = None;
        }
    }
    o.settings.metrics_res = o.metrics_res;

    let mut mesh = Mesh::load_stl(&o.input).map_err(|error| error.to_string())?;
    if mesh.triangle_count() == 0 {
        return Err("the STL contains no triangles".to_owned());
    }

    let unit_scale = if o.units == "inch" { 25.4 } else { 1.0 };
    let total_scale = unit_scale * o.scale;
    if (total_scale - 1.0).abs() > f64::EPSILON {
        mesh = mesh
            .transformed(MeshTransform {
                scale: [total_scale; 3],
                ..MeshTransform::default()
            })
            .map_err(|error| error.to_string())?;
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
        return run_single(&o, &mesh, before, after, repair);
    }
    run_stack(&o, &mesh, before, after, repair, total_scale)
}

fn run_check(o: &Options, path: &PathBuf) -> Result<(), String> {
    let cfg = o
        .profile_config
        .as_ref()
        .ok_or_else(|| "--check-gcode needs --profile".to_owned())?;
    let text = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let report = hfw_gcode::verify(&text, cfg);
    println!("Checked {} against profile '{}'.", path.display(), cfg.name);
    println!(
        "  {} layers, {} moves, {:.1} mm filament ({:.0} mm^3), estimated {:.0} s",
        report.layers, report.moves, report.filament_mm, report.volume_mm3, report.est_seconds
    );
    println!(
        "  bbox X {:.2}..{:.2}  Y {:.2}..{:.2}  Z {:.2}..{:.2}",
        report.bbox_min[0],
        report.bbox_max[0],
        report.bbox_min[1],
        report.bbox_max[1],
        report.bbox_min[2],
        report.bbox_max[2]
    );
    for warning in &report.warnings {
        println!("  warning: {warning}");
    }
    for error in &report.errors {
        println!("  error: {error}");
    }
    if report.ok() {
        println!("verify RESULT PASS");
        Ok(())
    } else {
        println!("verify RESULT FAIL");
        Err(format!("{} G-code error(s)", report.errors.len()))
    }
}

fn run_single(
    o: &Options,
    mesh: &Mesh,
    before: MeshDiagnostics,
    after: MeshDiagnostics,
    repair: Option<MeshRepairReport>,
) -> Result<(), String> {
    let z_mm = o
        .z_mm
        .ok_or_else(|| "missing required --z value".to_owned())?;
    let svg_path = o
        .svg
        .as_ref()
        .ok_or_else(|| "missing required --svg path".to_owned())?;

    let (slice, region) = slice_plane(mesh, z_mm).map_err(|error| error.to_string())?;
    let walls = hfw_engine::generate_walls(&region, &o.settings.wall, o.settings.mode)
        .map_err(|error| error.to_string())?;
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
    };
    if let Some(ip) = o.settings.infill {
        let regions = vec![region.clone()];
        let mut cache = MaskCache::new(&regions, ip.skin_res, ip.top_layers + ip.bottom_layers + 1);
        planned.paths.extend(hfw_engine::infill_layer(
            &region,
            &mut cache,
            0,
            &o.settings.wall,
            &ip,
        ));
    }
    planned.recount();
    fs::write(svg_path, planned.to_svg())
        .map_err(|error| format!("could not write {}: {error}", svg_path.display()))?;

    print_mesh("Mesh", after);
    match repair {
        Some(report) => print_repair(report),
        None => println!("Basic repair was not requested; the imported mesh was not modified."),
    }
    if before != after {
        println!("Mesh changed during preparation.");
    }
    println!(
        "Sliced {} triangles at Z={z_mm:.6} mm: {} contours, {} duplicate segments removed.",
        mesh.triangle_count(),
        slice.contours.len(),
        slice.diagnostics.duplicate_segments_removed
    );
    println!(
        "Planned {} paths in {:.1} ms: {} walls, {} middle, {} skin, {} infill.",
        planned.paths.len(),
        walls.stats.ms_total,
        planned.counts.walls,
        planned.counts.middle,
        planned.counts.skin,
        planned.counts.infill
    );
    println!(
        "Field nodes {}, thin {}, dropped {}; ridge vertices {} on {} branches.",
        walls.stats.nodes,
        walls.stats.thin_nodes,
        walls.stats.dropped_nodes,
        walls.stats.ridge_vertices,
        walls.stats.ridge_branches
    );
    if let Some(res) = o.settings.metrics_res {
        let m = evaluate_perimeter(&region, &planned.paths, &o.settings.wall, res);
        let full = evaluate_all(&region, &planned.paths, &o.settings.wall, res);
        print_metrics(&m);
        println!(
            "All paths at {res} mm: covered {:.4}, under {:.4}, over {:.4}, spill {:.4}.",
            full.bead_area / full.area_region.max(1e-9),
            full.under,
            full.over,
            full.spill
        );
    }
    println!("Wrote {}", svg_path.display());
    println!("No printable G-code was generated.");
    Ok(())
}

fn run_stack(
    o: &Options,
    mesh: &Mesh,
    before: MeshDiagnostics,
    after: MeshDiagnostics,
    repair: Option<MeshRepairReport>,
    total_scale: f64,
) -> Result<(), String> {
    let layer_height = o
        .layer_height
        .ok_or_else(|| "missing required --layer-height value".to_owned())?;
    let first_layer_height = o.first_layer_height.unwrap_or(layer_height);
    let out_dir = o
        .out_dir
        .as_ref()
        .ok_or_else(|| "missing required --out-dir folder".to_owned())?;

    let job = run(mesh, first_layer_height, layer_height, &o.settings)
        .map_err(|error| error.to_string())?;
    let mesh_volume = mesh.volume_mm3();

    fs::create_dir_all(out_dir)
        .map_err(|error| format!("could not create {}: {error}", out_dir.display()))?;

    let mut manifest = String::from(MANIFEST_HEADER);
    manifest.push('\n');
    let mut totals = JobTotals::default();
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

    println!(
        "Planned {} layers at {first_layer_height:.3} mm first layer then {layer_height:.3} mm.",
        job.layers.len()
    );
    println!(
        "Paths: {} walls, {} middle, {} skin, {} infill; {:.1} m of extrusion in {:.1} s.",
        job.layers.iter().map(|l| l.counts.walls).sum::<usize>(),
        totals.middle,
        totals.skin,
        totals.infill,
        totals.length / 1000.0,
        (job.stats.ms_slice + job.stats.ms_walls + job.stats.ms_infill) / 1000.0
    );
    let extruded = job.volume.0 + job.volume.1;
    println!(
        "Deposited volume {extruded:.2} mm^3 (perimeter {:.2}, interior {:.2}); {} starts/stops.",
        job.volume.0, job.volume.1, totals.starts_stops
    );
    if mesh_volume > 0.0 {
        println!(
            "extruded_over_mesh={:.4} (mesh {mesh_volume:.1} mm^3; only meaningful with --every 1 and --density 1.0)",
            extruded / mesh_volume
        );
    }
    println!(
        "Slowest layer {} took {:.1} ms; {} of {} layers planned.",
        job.worst_layer + 1,
        job.worst_layer_ms,
        job.stats.processed_layers,
        job.stats.layers
    );
    if !job.skipped.is_empty() {
        println!(
            "WARNING: {} layer(s) had no closed cross-section and were left empty:",
            job.skipped.len()
        );
        for (index, reason) in job.skipped.iter().take(5) {
            println!("  layer {}: {reason}", index + 1);
        }
        if job.skipped.len() > 5 {
            println!("  ... and {} more", job.skipped.len() - 5);
        }
    }
    if !job.metrics.is_empty() {
        let n = job.metrics.len() as f64;
        let mean_under = job.metrics.iter().map(|m| m.under).sum::<f64>() / n;
        let mean_over = job.metrics.iter().map(|m| m.over).sum::<f64>() / n;
        let mean_viol = job.metrics.iter().map(|m| m.width_violation).sum::<f64>() / n;
        println!(
            "Mean perimeter coverage: under {mean_under:.4}, over {mean_over:.4}, width violation {mean_viol:.4}."
        );
    }
    println!("Layer SVGs, manifest and report: {}", out_dir.display());
    match &o.gcode {
        Some(path) => write_gcode(o, &job, path),
        None => {
            println!("No printable G-code was generated (no --profile and --gcode).");
            Ok(())
        }
    }
}

fn write_gcode(o: &Options, job: &Job, path: &PathBuf) -> Result<(), String> {
    let cfg = o
        .profile_config
        .as_ref()
        .ok_or_else(|| "--gcode needs --profile".to_owned())?;
    if o.settings.every != 1 {
        return Err("--gcode needs every layer planned; drop --every".to_owned());
    }
    let object = std::path::Path::new(&o.input)
        .file_stem()
        .and_then(|s| s.to_str());
    let written =
        hfw_gcode::generate(&job.layers, cfg, object).map_err(|error| error.to_string())?;
    fs::write(path, &written.text)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    let s = &written.stats;
    println!(
        "G-code: {} layers, {:.1} mm filament ({:.2} g, {:.0} mm^3), estimated {:.1} min, {:.0} mm travel, {} retractions.",
        s.layers,
        s.filament_mm,
        s.filament_g,
        s.volume_mm3,
        s.est_seconds / 60.0,
        s.travel_mm,
        s.retractions
    );
    println!(
        "Placed footprint X {:.2}..{:.2}, Y {:.2}..{:.2}, top Z {:.2} on a {} x {} bed.",
        s.bbox_min.x, s.bbox_max.x, s.bbox_min.y, s.bbox_max.y, s.top_z, cfg.bed_x, cfg.bed_y
    );
    let report = hfw_gcode::verify(&written.text, cfg);
    for warning in &report.warnings {
        println!("  warning: {warning}");
    }
    for error in &report.errors {
        println!("  error: {error}");
    }
    if report.ok() {
        println!("verify RESULT PASS (independent check of the written file)");
    } else {
        println!("verify RESULT FAIL");
        return Err(format!(
            "{} G-code error(s); the file was written but must not be printed",
            report.errors.len()
        ));
    }
    println!("Wrote {}", path.display());
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
    let _ = writeln!(text, "Input: {}", o.input);
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
        "Distance field resolution: {:.4} mm ({} cells per millimetre)",
        o.settings.wall.h,
        1.0 / o.settings.wall.h
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
        "Wall generation: {:.0} ms total, slowest layer {} at {:.1} ms",
        job.stats.ms_walls,
        job.worst_layer + 1,
        job.worst_layer_ms
    );
    let _ = writeln!(
        &mut text,
        "Timing: slice {:.0} ms, walls {:.0} ms, infill {:.0} ms, metrics {:.0} ms",
        job.stats.ms_slice, job.stats.ms_walls, job.stats.ms_infill, job.stats.ms_metrics
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
        "Thin-wall field cells: {}; dropped unprintable cells: {}; ridge vertices: {}",
        job.stats.thin_nodes, job.stats.dropped_nodes, job.stats.ridge_vertices
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
    println!(
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
    println!(
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
    println!(
        "Perimeter at {:.0} paths: covered {:.4}, under {:.4}, over {:.4}, spill {:.4}.",
        m.lines,
        m.bead_area / m.area_target.max(1e-9),
        m.under,
        m.over,
        m.spill
    );
    println!(
        "  width {:.3}..{:.3} mm, violation {:.4} of length, {:.4} mm^2 of transverse variation per mm",
        m.min_width, m.max_width, m.width_violation, m.width_tv_per_mm
    );
}
