//! Bead paths to machine commands.

use std::fmt::Write as _;

use hfw_config::{Flavor, PrinterConfig, Seam};
use hfw_kernel::{Bounds, V2};
use hfw_toolpath::PlannedLayer;
use hfw_walls::{Kind, Line};

use crate::error::GcodeError;

/// Print feature a path belongs to, in the order the machine prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// First-layer adhesion skirt.
    Skirt,
    /// Outermost wall loop.
    Outer,
    /// Second and later wall loops.
    Inner,
    /// Bead from the medial axis, narrower than a nominal line.
    Thin,
    /// Solid top or bottom skin.
    Solid,
    /// Sparse infill.
    Sparse,
}

impl Feature {
    /// `;TYPE:` label, matching the names OrcaSlicer and PrusaSlicer emit so
    /// that viewers, Moonraker and analytic tooling group features the same way.
    pub fn label(self) -> &'static str {
        match self {
            Feature::Skirt => "Skirt",
            Feature::Outer => "Outer wall",
            Feature::Inner => "Inner wall",
            Feature::Thin => "Gap infill",
            Feature::Solid => "Internal solid infill",
            Feature::Sparse => "Sparse infill",
        }
    }

    fn rank(self, cfg: &PrinterConfig) -> i32 {
        match self {
            Feature::Skirt => -1,
            Feature::Outer => {
                if cfg.inner_walls_first {
                    20
                } else {
                    0
                }
            }
            Feature::Inner => 10,
            Feature::Thin => {
                if cfg.inner_walls_first {
                    0
                } else {
                    30
                }
            }
            Feature::Solid => 40,
            Feature::Sparse => 41,
        }
    }
}

/// One ordered bead path in machine coordinates.
#[derive(Clone, Debug)]
pub struct Path {
    /// Vertices in millimetres.
    pub pts: Vec<V2>,
    /// Extrusion width at each vertex.
    pub width: Vec<f64>,
    /// True when the path closes back onto its first vertex.
    pub closed: bool,
    /// Feature this path belongs to.
    pub feature: Feature,
    /// Print order rank; lower ranks print first.
    pub rank: i32,
}

/// Totals for one emitted file.
#[derive(Clone, Debug, Default)]
pub struct GcodeStats {
    /// Layers emitted.
    pub layers: usize,
    /// Deposited volume in cubic millimetres.
    pub volume_mm3: f64,
    /// Filament consumed in millimetres.
    pub filament_mm: f64,
    /// Filament consumed in grams.
    pub filament_g: f64,
    /// Estimated print time in seconds, without acceleration.
    pub est_seconds: f64,
    /// Total travel distance in millimetres.
    pub travel_mm: f64,
    /// Retraction count.
    pub retractions: usize,
    /// Lowest printed XY.
    pub bbox_min: V2,
    /// Highest printed XY.
    pub bbox_max: V2,
    /// Highest printed Z.
    pub top_z: f64,
}

/// A finished G-code file and its totals.
#[derive(Clone, Debug)]
pub struct Gcode {
    /// Complete file contents.
    pub text: String,
    /// Totals for that file.
    pub stats: GcodeStats,
}

/// Axis-aligned extent of a planned job.
#[derive(Clone, Copy, Debug)]
pub struct ModelExtent {
    /// Footprint in XY.
    pub xy: Bounds,
    /// Lowest layer bottom.
    pub min_z: f64,
    /// Highest layer top.
    pub max_z: f64,
}

impl ModelExtent {
    /// Measures the extent of the planned layers.
    pub fn of(layers: &[PlannedLayer]) -> ModelExtent {
        let mut xy = Bounds::empty();
        let (mut min_z, mut max_z) = (f64::INFINITY, f64::NEG_INFINITY);
        for layer in layers {
            for ring in &layer.reference {
                for p in ring {
                    xy.add(*p);
                }
            }
            if layer.bottom_z.is_finite() {
                min_z = min_z.min(layer.bottom_z);
                max_z = max_z.max(layer.print_z);
            }
        }
        if xy.is_empty() {
            xy.add(V2::ZERO);
        }
        ModelExtent {
            xy,
            min_z: if min_z.is_finite() { min_z } else { 0.0 },
            max_z: if max_z.is_finite() { max_z } else { 0.0 },
        }
    }
}

fn seam_index(pts: &[V2]) -> usize {
    let n = pts.len();
    let mut best = 0;
    let mut best_key = (-1i32, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for i in 0..n {
        let a = pts[(i + n - 1) % n];
        let b = pts[i];
        let c = pts[(i + 1) % n];
        let u = (b - a).norm();
        let v = (c - b).norm();
        let turn = u.dot(v).clamp(-1.0, 1.0).acos();
        let key = ((turn > 0.4) as i32, (b.y * 1e6).round() / 1e6, -b.x);
        if key.0 > best_key.0
            || (key.0 == best_key.0
                && (key.1 > best_key.1 || (key.1 == best_key.1 && key.2 > best_key.2)))
        {
            best_key = key;
            best = i;
        }
    }
    best
}

fn trim_open(pts: &[V2], width: &[f64], floor: f64) -> Option<(Vec<V2>, Vec<f64>)> {
    let first = width.iter().position(|&x| x >= floor)?;
    let last = width.iter().rposition(|&x| x >= floor)?;
    if last <= first {
        return None;
    }
    let mut out_pts = Vec::new();
    let mut out_width = Vec::new();
    if first > 0 {
        let t = (floor - width[first - 1]) / (width[first] - width[first - 1]);
        out_pts.push(pts[first - 1].lerp(pts[first], t));
        out_width.push(floor);
    }
    for k in first..=last {
        out_pts.push(pts[k]);
        out_width.push(width[k].max(floor));
    }
    if last + 1 < pts.len() {
        let t = (width[last] - floor) / (width[last] - width[last + 1]);
        out_pts.push(pts[last].lerp(pts[last + 1], t));
        out_width.push(floor);
    }
    Some((out_pts, out_width))
}

fn feature_of(kind: Kind) -> Feature {
    match kind {
        Kind::Side(0) => Feature::Outer,
        Kind::Side(_) => Feature::Inner,
        Kind::Middle => Feature::Thin,
        Kind::Solid => Feature::Solid,
        Kind::Infill => Feature::Sparse,
    }
}

fn path_from(line: &Line, tx: f64, ty: f64, cfg: &PrinterConfig) -> Option<Path> {
    let mut pts: Vec<V2> = line
        .pts
        .iter()
        .map(|p| V2::new(p.x + tx, p.y + ty))
        .collect();
    let mut width = line.width.clone();
    if line.closed {
        for w in width.iter_mut() {
            *w = w.max(cfg.min_bead_width);
        }
    } else if matches!(line.kind, Kind::Side(_) | Kind::Middle) {
        let trimmed = trim_open(&pts, &width, cfg.min_bead_width)?;
        pts = trimmed.0;
        width = trimmed.1;
    }
    if line.closed && hfw_kernel::ring_signed_area(&pts) < 0.0 {
        pts.reverse();
        width.reverse();
    }
    let feature = feature_of(line.kind);
    let mut path = Path {
        pts,
        width,
        closed: line.closed,
        feature,
        rank: feature.rank(cfg),
    };
    if path.closed && cfg.seam == Seam::Aligned {
        let s = seam_index(&path.pts);
        path.pts.rotate_left(s);
        path.width.rotate_left(s);
    }
    Some(path)
}

fn order(mut paths: Vec<Path>, mut pos: V2, seam: Seam) -> (Vec<Path>, V2, f64) {
    let mut out = Vec::with_capacity(paths.len());
    let mut travel = 0.0;
    while !paths.is_empty() {
        let mut pick = 0;
        let mut best = f64::INFINITY;
        let mut reverse = false;
        for (i, p) in paths.iter().enumerate() {
            let (d, rev) = if p.closed {
                if seam == Seam::Nearest {
                    (
                        p.pts
                            .iter()
                            .map(|q| q.dist(pos))
                            .fold(f64::INFINITY, f64::min),
                        false,
                    )
                } else {
                    (p.pts[0].dist(pos), false)
                }
            } else {
                let start = p.pts[0].dist(pos);
                let end = p.pts[p.pts.len() - 1].dist(pos);
                if end < start {
                    (end, true)
                } else {
                    (start, false)
                }
            };
            if d < best {
                best = d;
                pick = i;
                reverse = rev;
            }
        }
        let mut p = paths.swap_remove(pick);
        if p.closed && seam == Seam::Nearest {
            let mut at = 0;
            let mut nearest = f64::INFINITY;
            for (i, q) in p.pts.iter().enumerate() {
                let d = q.dist(pos);
                if d < nearest {
                    nearest = d;
                    at = i;
                }
            }
            p.pts.rotate_left(at);
            p.width.rotate_left(at);
        }
        if reverse {
            p.pts.reverse();
            p.width.reverse();
        }
        travel += best;
        pos = if p.closed {
            p.pts[0]
        } else {
            p.pts[p.pts.len() - 1]
        };
        out.push(p);
    }
    (out, pos, travel)
}

fn base_speed(feature: Feature, cfg: &PrinterConfig, first_layer: bool) -> f64 {
    let speed = match feature {
        Feature::Skirt => cfg.first_layer_speed,
        Feature::Outer => cfg.outer_wall_speed,
        Feature::Inner => cfg.inner_wall_speed,
        Feature::Thin => cfg.thin_speed,
        Feature::Solid => cfg.solid_speed,
        Feature::Sparse => cfg.infill_speed,
    };
    if first_layer {
        speed.min(cfg.first_layer_speed)
    } else {
        speed
    }
}

fn skirt_paths(first: &PlannedLayer, tx: f64, ty: f64, cfg: &PrinterConfig) -> Vec<Path> {
    if cfg.skirt_loops == 0 {
        return Vec::new();
    }
    let mut b = Bounds::empty();
    for ring in &first.reference {
        for p in ring {
            b.add(*p);
        }
    }
    if b.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(cfg.skirt_loops);
    for i in 0..cfg.skirt_loops {
        let off = cfg.skirt_distance + (i as f64 + 0.5) * cfg.line_width;
        let (x0, y0) = (b.min.x + tx - off, b.min.y + ty - off);
        let (x1, y1) = (b.max.x + tx + off, b.max.y + ty + off);
        out.push(Path {
            pts: vec![
                V2::new(x0, y0),
                V2::new(x1, y0),
                V2::new(x1, y1),
                V2::new(x0, y1),
            ],
            width: vec![cfg.line_width; 4],
            closed: true,
            feature: Feature::Skirt,
            rank: -1,
        });
    }
    out
}

fn placement(extent: ModelExtent, cfg: &PrinterConfig) -> (f64, f64) {
    let target = if cfg.origin_center {
        (0.0, 0.0)
    } else {
        (0.5 * cfg.bed_x, 0.5 * cfg.bed_y)
    };
    (
        target.0 - 0.5 * (extent.xy.min.x + extent.xy.max.x),
        target.1 - 0.5 * (extent.xy.min.y + extent.xy.max.y),
    )
}

fn check_fit(extent: ModelExtent, cfg: &PrinterConfig) -> Result<(), GcodeError> {
    let margin = if cfg.skirt_loops > 0 {
        cfg.skirt_distance + cfg.skirt_loops as f64 * cfg.line_width
    } else {
        0.0
    };
    let width = extent.xy.width() + 2.0 * margin;
    let depth = extent.xy.height() + 2.0 * margin;
    if width > cfg.bed_x - 2.0 || depth > cfg.bed_y - 2.0 {
        return Err(GcodeError::DoesNotFit {
            width,
            depth,
            bed_x: cfg.bed_x,
            bed_y: cfg.bed_y,
        });
    }
    let height = extent.max_z - extent.min_z;
    if height > cfg.max_z {
        return Err(GcodeError::TooTall {
            height,
            max_z: cfg.max_z,
        });
    }
    Ok(())
}

struct Writer<'a> {
    cfg: &'a PrinterConfig,
    out: String,
    pos: V2,
    z: f64,
    e_abs: f64,
    retracted: bool,
    last_width: f64,
    last_f: f64,
    last_feature: Option<Feature>,
    previous_feature: Option<Feature>,
    no_hop: bool,
    stats: GcodeStats,
    bbox_set: bool,
}

impl Writer<'_> {
    fn touch(&mut self, p: V2) {
        if !self.bbox_set {
            self.stats.bbox_min = p;
            self.stats.bbox_max = p;
            self.bbox_set = true;
        } else {
            self.stats.bbox_min.x = self.stats.bbox_min.x.min(p.x);
            self.stats.bbox_min.y = self.stats.bbox_min.y.min(p.y);
            self.stats.bbox_max.x = self.stats.bbox_max.x.max(p.x);
            self.stats.bbox_max.y = self.stats.bbox_max.y.max(p.y);
        }
    }

    fn e_move(&mut self, delta: f64, speed: f64) {
        if self.cfg.relative_e {
            let _ = writeln!(self.out, "G1 E{:.5} F{:.0}", delta, speed * 60.0);
        } else {
            self.e_abs += delta;
            let _ = writeln!(self.out, "G1 E{:.5} F{:.0}", self.e_abs, speed * 60.0);
        }
        self.last_f = speed * 60.0;
    }

    fn retract(&mut self) {
        if self.cfg.retract_len > 0.0 && !self.retracted {
            self.e_move(-self.cfg.retract_len, self.cfg.retract_speed);
            self.retracted = true;
            self.stats.retractions += 1;
            self.stats.est_seconds += self.cfg.retract_len / self.cfg.retract_speed;
        }
    }

    fn unretract(&mut self) {
        if self.retracted {
            self.e_move(self.cfg.retract_len, self.cfg.retract_speed);
            self.retracted = false;
            self.stats.est_seconds += self.cfg.retract_len / self.cfg.retract_speed;
        }
    }

    fn z_move(&mut self, z: f64) {
        let _ = writeln!(self.out, "G1 Z{:.3} F600", z);
        self.last_f = 600.0;
    }

    fn travel(&mut self, to: V2, threshold: f64) {
        let d = self.pos.dist(to);
        if d < 1e-9 {
            return;
        }
        let long = d > threshold;
        if long {
            self.retract();
        }
        let mut hopped = false;
        if long && self.cfg.z_hop > 0.0 && !self.no_hop {
            self.z_move(self.z + self.cfg.z_hop);
            hopped = true;
        }
        let _ = writeln!(
            self.out,
            "G0 X{:.3} Y{:.3} F{:.0}",
            to.x,
            to.y,
            self.cfg.travel_speed * 60.0
        );
        self.last_f = self.cfg.travel_speed * 60.0;
        if hopped {
            self.z_move(self.z);
        }
        self.no_hop = false;
        self.pos = to;
        self.stats.travel_mm += d;
        self.stats.est_seconds += d / self.cfg.travel_speed;
    }

    fn extrude(&mut self, path: &Path, height: f64, speed: f64) {
        let is_infill =
            |f: Option<Feature>| matches!(f, Some(Feature::Solid) | Some(Feature::Sparse));
        let threshold = if is_infill(self.previous_feature) && is_infill(Some(path.feature)) {
            self.cfg.infill_retract_min_travel
        } else {
            self.cfg.retract_min_travel
        };
        self.travel(path.pts[0], threshold);
        self.unretract();
        self.previous_feature = Some(path.feature);
        if self.last_feature != Some(path.feature) {
            let _ = writeln!(self.out, ";TYPE:{}", path.feature.label());
            self.last_feature = Some(path.feature);
        }
        let n = path.pts.len();
        let segments = if path.closed { n } else { n - 1 };
        let area = self.cfg.filament_area();
        for i in 0..segments {
            let a = path.pts[i];
            let b = path.pts[(i + 1) % n];
            let len = a.dist(b);
            if len < 1e-4 {
                continue;
            }
            let width = 0.5 * (path.width[i] + path.width[(i + 1) % n]);
            let volume = len * width * height * self.cfg.flow;
            let delta = volume / area;
            if (width - self.last_width).abs() > 0.02 {
                let _ = writeln!(self.out, ";WIDTH:{:.3}", width);
                self.last_width = width;
            }
            let feed = speed * 60.0;
            let f_part = if (feed - self.last_f).abs() > 0.5 {
                format!(" F{feed:.0}")
            } else {
                String::new()
            };
            if self.cfg.relative_e {
                let _ = writeln!(
                    self.out,
                    "G1 X{:.3} Y{:.3} E{:.5}{}",
                    b.x, b.y, delta, f_part
                );
            } else {
                self.e_abs += delta;
                let _ = writeln!(
                    self.out,
                    "G1 X{:.3} Y{:.3} E{:.5}{}",
                    b.x, b.y, self.e_abs, f_part
                );
            }
            self.last_f = feed;
            self.pos = b;
            self.touch(a);
            self.touch(b);
            self.stats.volume_mm3 += volume;
            self.stats.est_seconds += len / speed;
        }
    }
}

/// Emits a complete G-code file for a planned job.
///
/// `object_name` is used only when the profile enables `EXCLUDE_OBJECT_DEFINE`;
/// it names the single printed object so the machine can cancel it.
pub fn generate(
    layers: &[PlannedLayer],
    cfg: &PrinterConfig,
    object_name: Option<&str>,
) -> Result<Gcode, GcodeError> {
    cfg.validate()?;
    if layers.is_empty() {
        return Err(GcodeError::NoLayers);
    }
    let extent = ModelExtent::of(layers);
    check_fit(extent, cfg)?;
    let (tx, ty) = placement(extent, cfg);
    let z0 = extent.min_z;
    let layer_count = layers.len();
    let mut w = Writer {
        cfg,
        out: String::new(),
        pos: if cfg.origin_center {
            V2::ZERO
        } else {
            V2::new(0.5 * cfg.bed_x, 0.5 * cfg.bed_y)
        },
        z: 0.0,
        e_abs: 0.0,
        retracted: true,
        last_width: -1.0,
        last_f: -1.0,
        last_feature: None,
        previous_feature: None,
        no_hop: false,
        stats: GcodeStats::default(),
        bbox_set: false,
    };
    let extra = [("{total_layers}", layer_count.to_string())];
    w.out.push_str(&cfg.fill(&cfg.start_gcode, &extra));
    w.out.push('\n');
    w.out.push_str("G90\n");
    w.out
        .push_str(if cfg.relative_e { "M83\n" } else { "M82\n" });
    w.out.push_str("G92 E0\nM107\n");
    let _ = writeln!(w.out, "M109 S{}", cfg.first_nozzle_temp);
    if cfg.flavor == Flavor::Klipper && cfg.pressure_advance > 0.0 {
        let _ = writeln!(
            w.out,
            "SET_PRESSURE_ADVANCE ADVANCE={}",
            cfg.pressure_advance
        );
    }
    if cfg.flavor == Flavor::Klipper && cfg.emit_print_stats {
        let _ = writeln!(w.out, "SET_PRINT_STATS_INFO TOTAL_LAYER={layer_count}");
    }
    if cfg.exclude_object {
        if let Some(name) = object_name {
            let _ = writeln!(
                w.out,
                "EXCLUDE_OBJECT_DEFINE NAME={} CENTER={:.3},{:.3}",
                sanitise(name),
                0.5 * (extent.xy.min.x + extent.xy.max.x) + tx,
                0.5 * (extent.xy.min.y + extent.xy.max.y) + ty
            );
        }
    }
    let first = layers
        .iter()
        .find(|l| !l.paths.is_empty() || !l.reference.is_empty());
    let skirt = match first {
        Some(layer) => skirt_paths(layer, tx, ty, cfg),
        None => Vec::new(),
    };
    let mut printed = 0usize;
    for layer in layers {
        let height = layer.height();
        let z = layer.print_z - z0;
        let first_layer = printed == 0;
        let mut paths: Vec<Path> = layer
            .paths
            .iter()
            .filter_map(|l| path_from(l, tx, ty, cfg))
            .collect();
        if paths.is_empty() && !first_layer {
            continue;
        }
        let mut ordered: Vec<Path> = Vec::new();
        let mut sim = w.pos;
        let mut travel_total = 0.0;
        if first_layer && !skirt.is_empty() {
            let (o, end, travelled) = order(skirt.clone(), sim, Seam::Nearest);
            ordered.extend(o);
            sim = end;
            travel_total += travelled;
        }
        paths.sort_by_key(|p| p.rank);
        let mut groups: Vec<(i32, Vec<Path>)> = Vec::new();
        for p in paths {
            match groups.last_mut() {
                Some((rank, group)) if *rank == p.rank => group.push(p),
                _ => groups.push((p.rank, vec![p])),
            }
        }
        for (_, group) in groups {
            let (o, end, travelled) = order(group, sim, cfg.seam);
            ordered.extend(o);
            sim = end;
            travel_total += travelled;
        }
        let speeds: Vec<f64> = ordered
            .iter()
            .map(|p| {
                let widest = p.width.iter().cloned().fold(0.0, f64::max).max(1e-6);
                base_speed(p.feature, cfg, first_layer)
                    .min(cfg.max_volumetric_speed / (widest * height * cfg.flow))
            })
            .collect();
        let mut extrude_time = 0.0;
        for (p, speed) in ordered.iter().zip(&speeds) {
            let n = p.pts.len();
            let segments = if p.closed { n } else { n - 1 };
            let mut len = 0.0;
            for i in 0..segments {
                len += p.pts[i].dist(p.pts[(i + 1) % n]);
            }
            extrude_time += len / speed;
        }
        let layer_time = extrude_time + travel_total / cfg.travel_speed;
        let factor = if first_layer || layer_time >= cfg.min_layer_time {
            1.0
        } else {
            (layer_time / cfg.min_layer_time).clamp(cfg.min_speed_factor, 1.0)
        };
        let _ = writeln!(
            w.out,
            ";LAYER_CHANGE\n;Z:{:.3}\n;HEIGHT:{:.3}\n;LAYER:{}",
            z, height, printed
        );
        if cfg.flavor == Flavor::Klipper && cfg.emit_print_stats {
            let _ = writeln!(w.out, "SET_PRINT_STATS_INFO CURRENT_LAYER={}", printed + 1);
        }
        if !cfg.relative_e {
            w.out.push_str("G92 E0\n");
            w.e_abs = 0.0;
        }
        if printed == 1 && (cfg.first_nozzle_temp - cfg.nozzle_temp).abs() > 0.5 {
            let _ = writeln!(w.out, "M104 S{}", cfg.nozzle_temp);
        }
        if printed == cfg.fan_start_layer && cfg.fan_speed > 0.0 {
            let _ = writeln!(w.out, "M106 S{}", (cfg.fan_speed * 2.55).round() as i64);
        }
        w.retract();
        w.z_move(z);
        w.z = z;
        w.stats.top_z = z.max(w.stats.top_z);
        w.last_feature = None;
        w.previous_feature = None;
        w.no_hop = true;
        for (p, speed) in ordered.iter().zip(&speeds) {
            w.extrude(p, height, speed * factor);
        }
        printed += 1;
    }
    w.stats.layers = printed;
    w.retract();
    let top = (w.z + 10.0).min(cfg.max_z);
    w.z_move(top);
    w.out.push_str(&cfg.fill(&cfg.end_gcode, &extra));
    w.out.push('\n');
    let area = cfg.filament_area();
    w.stats.filament_mm = w.stats.volume_mm3 / area;
    w.stats.filament_g = w.stats.volume_mm3 / 1000.0 * cfg.filament_density;
    let stats = w.stats.clone();
    let mut head = String::new();
    let _ = writeln!(
        head,
        "; generated by hfw (clean-room Rust engine) - ALPHA, verify before printing"
    );
    let _ = writeln!(head, "; printer = {}", cfg.name);
    let _ = writeln!(head, "; layers = {}", stats.layers);
    let _ = writeln!(
        head,
        "; layer_height = {} first_layer_height = {}",
        cfg.layer_height, cfg.first_layer_height
    );
    let _ = writeln!(
        head,
        "; line_width = {} walls = {} infill = {} {}",
        cfg.line_width, cfg.walls, cfg.infill_density, cfg.infill_pattern
    );
    let _ = writeln!(
        head,
        "; nozzle_temp = {} bed_temp = {}",
        cfg.nozzle_temp, cfg.bed_temp
    );
    let _ = writeln!(
        head,
        "; estimated printing time = {:.0} s",
        stats.est_seconds
    );
    let _ = writeln!(
        head,
        "; filament used = {:.1} mm ({:.2} g, {:.0} mm3)",
        stats.filament_mm, stats.filament_g, stats.volume_mm3
    );
    Ok(Gcode {
        text: format!("{head}{}", w.out),
        stats,
    })
}

fn sanitise(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("HFW_MODEL");
    }
    out
}
