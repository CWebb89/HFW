//! Independent G-code safety verification.
//!
//! The verifier parses the finished file as text, tracks position, extruder
//! state and heater state itself, and applies printer physics to what it finds.
//! It shares no state with the writer, so a bug in the writer shows up here as
//! an error rather than as agreement.

use hfw_config::PrinterConfig;

/// Result of checking one G-code file.
#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    /// Reasons the file is unsafe or wrong.
    pub errors: Vec<String>,
    /// Reasons to look twice, but not blockers.
    pub warnings: Vec<String>,
    /// Layer count read from `;LAYER_CHANGE` markers.
    pub layers: usize,
    /// `G0`/`G1` move count.
    pub moves: usize,
    /// Deposited volume implied by the `E` values.
    pub volume_mm3: f64,
    /// Filament consumed in millimetres.
    pub filament_mm: f64,
    /// Estimated print time in seconds.
    pub est_seconds: f64,
    /// Lowest printed position.
    pub bbox_min: [f64; 3],
    /// Highest printed position.
    pub bbox_max: [f64; 3],
    /// Z of the first extruding move.
    pub first_extrusion_z: f64,
}

impl VerifyReport {
    /// True when no error was recorded.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

struct Command<'a> {
    code: &'a str,
    params: Vec<(char, f64)>,
    bad: Option<String>,
}

fn parse_line(line: &str) -> Option<Command<'_>> {
    let body = line.split(';').next().unwrap_or("").trim();
    if body.is_empty() {
        return None;
    }
    let mut tokens = body.split_whitespace();
    let code = tokens.next()?;
    let mut params = Vec::new();
    let mut bad = None;
    for token in tokens {
        let mut chars = token.chars();
        let letter = match chars.next() {
            Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
            _ => continue,
        };
        let rest = chars.as_str();
        if rest.is_empty() {
            continue;
        }
        match rest.parse::<f64>() {
            Ok(v) if v.is_finite() => params.push((letter, v)),
            Ok(_) => bad = Some(format!("non-finite value in '{token}'")),
            Err(_) => {
                if (code.starts_with('G') || code.starts_with('M')) && !rest.contains('=') {
                    bad = Some(format!("unreadable value in '{token}'"));
                }
            }
        }
    }
    Some(Command { code, params, bad })
}

fn get(command: &Command<'_>, key: char) -> Option<f64> {
    command.params.iter().find(|p| p.0 == key).map(|p| p.1)
}

/// Checks a G-code file against a printer profile.
pub fn verify(gcode: &str, cfg: &PrinterConfig) -> VerifyReport {
    let mut r = VerifyReport::default();
    let area = cfg.filament_area();
    let (mut x, mut y, mut z) = (f64::NAN, f64::NAN, f64::NAN);
    let mut e = 0.0;
    let mut relative_e = false;
    let mut absolute_xyz = true;
    let mut feed = 0.0;
    let mut ready = false;
    let mut nozzle_target = 0.0;
    let mut height = f64::NAN;
    let mut max_extrusion_z = f64::NEG_INFINITY;
    let mut retract_state = 0.0;
    let mut saw_heater_off = false;
    let mut saw_fan_on = false;
    let (mut low, mut high) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let half_x = if cfg.origin_center {
        0.5 * cfg.bed_x
    } else {
        cfg.bed_x
    };
    let half_y = if cfg.origin_center {
        0.5 * cfg.bed_y
    } else {
        cfg.bed_y
    };
    let min_x = if cfg.origin_center { -half_x } else { 0.0 };
    let min_y = if cfg.origin_center { -half_y } else { 0.0 };
    let tolerance = 1e-3;
    let max_errors = 25;
    for (index, line) in gcode.lines().enumerate() {
        let number = index + 1;
        if let Some(rest) = line.trim().strip_prefix(';') {
            let tag = rest.trim();
            if tag == "LAYER_CHANGE" {
                r.layers += 1;
            } else if let Some(h) = tag.strip_prefix("HEIGHT:") {
                height = h.trim().parse().unwrap_or(f64::NAN);
            }
            continue;
        }
        let Some(c) = parse_line(line) else { continue };
        if let Some(bad) = &c.bad {
            push(&mut r, max_errors, format!("line {number}: {bad}"));
            continue;
        }
        match c.code {
            "G90" => absolute_xyz = true,
            "G91" => absolute_xyz = false,
            "M82" => relative_e = false,
            "M83" => relative_e = true,
            "G92" => {
                if let Some(v) = get(&c, 'E') {
                    e = v;
                }
            }
            "G28" => {
                x = f64::NAN;
                y = f64::NAN;
                z = f64::NAN;
            }
            "M104" | "M109" => {
                if let Some(s) = get(&c, 'S') {
                    if s > 0.0 && !(150.0..=320.0).contains(&s) {
                        push(
                            &mut r,
                            max_errors,
                            format!("line {number}: nozzle target {s} C is outside 150..320"),
                        );
                    }
                    if s == 0.0 {
                        nozzle_target = 0.0;
                        ready = false;
                        if c.code == "M104" {
                            saw_heater_off = true;
                        }
                    } else {
                        nozzle_target = s;
                        if c.code == "M109" && s >= 170.0 {
                            ready = true;
                        }
                    }
                }
            }
            "M140" | "M190" => {
                if let Some(s) = get(&c, 'S') {
                    if s > 130.0 {
                        push(
                            &mut r,
                            max_errors,
                            format!("line {number}: bed target {s} C is above 130"),
                        );
                    }
                }
            }
            "M106" => {
                if get(&c, 'S').is_some_and(|s| s > 0.0) {
                    saw_fan_on = true;
                }
            }
            "G0" | "G1" => {
                r.moves += 1;
                if let Some(f) = get(&c, 'F') {
                    feed = f;
                    if f <= 0.0 {
                        push(
                            &mut r,
                            max_errors,
                            format!("line {number}: feedrate {f} must be positive"),
                        );
                    }
                    if f > cfg.max_speed * 60.0 + 1e-6 {
                        push(
                            &mut r,
                            max_errors,
                            format!(
                                "line {number}: feedrate {f:.0} mm/min is above max_speed {:.0} mm/s",
                                cfg.max_speed
                            ),
                        );
                    }
                }
                let previous = (x, y);
                let extrusion = match get(&c, 'E') {
                    Some(v) => {
                        let delta = if relative_e { v } else { v - e };
                        if !relative_e {
                            e = v;
                        }
                        Some(delta)
                    }
                    None => None,
                };
                if let Some(v) = get(&c, 'X') {
                    x = if absolute_xyz {
                        v
                    } else if x.is_finite() {
                        x + v
                    } else {
                        f64::NAN
                    };
                }
                if let Some(v) = get(&c, 'Y') {
                    y = if absolute_xyz {
                        v
                    } else if y.is_finite() {
                        y + v
                    } else {
                        f64::NAN
                    };
                }
                if let Some(v) = get(&c, 'Z') {
                    z = if absolute_xyz {
                        v
                    } else if z.is_finite() {
                        z + v
                    } else {
                        f64::NAN
                    };
                }
                if x.is_finite() && (x < min_x - tolerance || x > half_x + tolerance) {
                    push(
                        &mut r,
                        max_errors,
                        format!("line {number}: X {x:.3} is outside the bed"),
                    );
                }
                if y.is_finite() && (y < min_y - tolerance || y > half_y + tolerance) {
                    push(
                        &mut r,
                        max_errors,
                        format!("line {number}: Y {y:.3} is outside the bed"),
                    );
                }
                if z.is_finite() && (z < -tolerance || z > cfg.max_z + tolerance) {
                    push(
                        &mut r,
                        max_errors,
                        format!("line {number}: Z {z:.3} is outside 0..{}", cfg.max_z),
                    );
                }
                let moved_xy = previous.0.is_finite() && previous.1.is_finite();
                let distance = if moved_xy {
                    ((x - previous.0).powi(2) + (y - previous.1).powi(2)).sqrt()
                } else {
                    0.0
                };
                let speed = (feed / 60.0).max(1e-6);
                match extrusion {
                    Some(delta) if delta > 1e-9 && distance > 1e-6 => {
                        if !ready {
                            push(
                                &mut r,
                                max_errors,
                                format!(
                                    "line {number}: extrusion before the nozzle was heated with M109 (target {nozzle_target})"
                                ),
                            );
                        }
                        if z.is_finite() {
                            if z < max_extrusion_z - 1e-6 {
                                push(
                                    &mut r,
                                    max_errors,
                                    format!(
                                        "line {number}: extruding at Z {z:.3} below an earlier layer at {max_extrusion_z:.3}"
                                    ),
                                );
                            }
                            max_extrusion_z = max_extrusion_z.max(z);
                            if r.first_extrusion_z == 0.0 {
                                r.first_extrusion_z = z;
                            }
                        }
                        let volume = delta * area;
                        r.volume_mm3 += volume;
                        r.filament_mm += delta;
                        r.est_seconds += distance / speed;
                        let flow = volume / (distance / speed);
                        if flow > cfg.max_volumetric_speed * 1.05 {
                            push(
                                &mut r,
                                max_errors,
                                format!(
                                    "line {number}: volumetric flow {flow:.1} mm3/s is above the limit {:.1}",
                                    cfg.max_volumetric_speed
                                ),
                            );
                        }
                        if height.is_finite() && height > 0.0 {
                            let width = volume / (distance * height);
                            if width > 4.0 * cfg.nozzle_diameter || width < 0.05 {
                                push(
                                    &mut r,
                                    max_errors,
                                    format!(
                                        "line {number}: implausible extrusion width {width:.3} mm"
                                    ),
                                );
                            }
                        }
                        for (i, value) in [x, y, z].iter().enumerate() {
                            let from = match i {
                                0 => previous.0,
                                1 => previous.1,
                                _ => *value,
                            };
                            low[i] = low[i].min(*value).min(from);
                            high[i] = high[i].max(*value).max(from);
                        }
                    }
                    Some(delta) if distance <= 1e-6 => {
                        retract_state += delta;
                        if delta.abs() > 10.0 {
                            push(
                                &mut r,
                                max_errors,
                                format!("line {number}: single filament move of {delta:.2} mm"),
                            );
                        }
                        r.est_seconds += delta.abs() / speed;
                    }
                    _ => {
                        if distance > 1e-6 {
                            r.est_seconds += distance / speed;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if r.moves == 0 {
        r.errors.push("no G0/G1 moves found".to_owned());
    }
    if r.volume_mm3 <= 0.0 {
        r.errors.push("the file extrudes no material".to_owned());
    }
    if r.layers == 0 {
        r.errors.push("no ;LAYER_CHANGE markers found".to_owned());
    }
    if !saw_heater_off {
        r.warnings
            .push("no M104 S0 found: the heater may stay on after the print".to_owned());
    }
    if cfg.fan_speed > 0.0 && !saw_fan_on {
        r.warnings
            .push("no fan-on command (M106 S>0) found".to_owned());
    }
    if r.first_extrusion_z > 0.0 && r.first_extrusion_z > cfg.first_layer_height * 1.5 {
        r.errors.push(format!(
            "first extrusion is at Z {:.3}, expected about {}",
            r.first_extrusion_z, cfg.first_layer_height
        ));
    }
    if retract_state.abs() > cfg.retract_len + 1e-6 {
        r.errors.push(format!(
            "retraction does not balance: net {:.4} mm left over",
            retract_state
        ));
    }
    r.bbox_min = low;
    r.bbox_max = high;
    r
}

fn push(report: &mut VerifyReport, limit: usize, message: String) {
    if report.errors.len() < limit {
        report.errors.push(message);
    } else if report.errors.len() == limit {
        report.errors.push("(further errors suppressed)".to_owned());
    }
}
