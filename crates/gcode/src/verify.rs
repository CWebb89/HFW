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
    /// Travel moves whose straight line crosses a perimeter already printed on
    /// the same layer.
    ///
    /// This is the only number that says whether travel avoidance did anything:
    /// a move into an enclosed interior crosses the wall that encloses it no
    /// matter which entry point is chosen, so the count never reaches zero.
    /// A Z hop is counted, because the nozzle still drags across the wall.
    pub travel_crossings: usize,
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
                // Only a command this verifier reads numerically needs a number in
                // its parameters. `M862.6 P"Input shaper"` and `M115 U6.4.0+11974`
                // are firmware handshakes; their text is the machine's business.
                if is_numeric_command(code) {
                    bad = Some(format!("unreadable value in '{token}'"));
                }
            }
        }
    }
    Some(Command { code, params, bad })
}

/// Whether a command's parameters are all read as numbers, so a word among them is
/// a broken value rather than a handshake string.
fn is_numeric_command(code: &str) -> bool {
    matches!(
        code,
        "G0" | "G1"
            | "G2"
            | "G3"
            | "G28"
            | "G90"
            | "G91"
            | "G92"
            | "M82"
            | "M83"
            | "M104"
            | "M106"
            | "M109"
            | "M140"
            | "M190"
            | "M200"
    )
}

fn get(command: &Command<'_>, key: char) -> Option<f64> {
    command.params.iter().find(|p| p.0 == key).map(|p| p.1)
}

/// True when the `;TYPE:` marker names a perimeter the nozzle can cut across.
fn perimeter_type(tag: &str) -> bool {
    matches!(tag, "Outer wall" | "Inner wall" | "Gap infill")
}

/// True when `a`-`b` crosses `c`-`d`; touching an endpoint does not count.
fn segments_cross(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    if b.0.max(a.0) < c.0.min(d.0)
        || c.0.max(d.0) < a.0.min(b.0)
        || b.1.max(a.1) < c.1.min(d.1)
        || c.1.max(d.1) < a.1.min(b.1)
    {
        return false;
    }
    let orient = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| -> f64 {
        (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
    };
    let straddles = |p: f64, q: f64| -> bool { (p > 1e-9 && q < -1e-9) || (p < -1e-9 && q > 1e-9) };
    straddles(orient(a, b, c), orient(a, b, d)) && straddles(orient(c, d, a), orient(c, d, b))
}

/// True when the arc sweeps clockwise, as `G2` does.
fn arc_is_clockwise(code: &str) -> bool {
    code == "G2"
}

/// Length of the arc from `(x0, y0)` to `(x1, y1)` about a centre, or `None`
/// when the two points do not lie on one circle.
///
/// A start and end point that coincide describe a full circle, which is the one
/// case where the chord says nothing about the sweep; it is counted as a whole
/// turn in the commanded direction.
fn arc_length(x0: f64, y0: f64, x1: f64, y1: f64, i: f64, j: f64, clockwise: bool) -> Option<f64> {
    let (cx, cy) = (x0 + i, y0 + j);
    let start_r = ((x0 - cx).powi(2) + (y0 - cy).powi(2)).sqrt();
    let end_r = ((x1 - cx).powi(2) + (y1 - cy).powi(2)).sqrt();
    if !start_r.is_finite() || start_r < 1e-6 {
        return None;
    }
    if (end_r - start_r).abs() > 0.05 {
        return None;
    }
    let a0 = (y0 - cy).atan2(x0 - cx);
    let a1 = (y1 - cy).atan2(x1 - cx);
    let mut sweep = a1 - a0;
    if clockwise {
        while sweep >= 0.0 {
            sweep -= std::f64::consts::TAU;
        }
    } else {
        while sweep <= 0.0 {
            sweep += std::f64::consts::TAU;
        }
    }
    Some(sweep.abs() * start_r)
}

/// Checks a G-code file against a printer profile.
pub fn verify(gcode: &str, cfg: &PrinterConfig) -> VerifyReport {
    let mut r = VerifyReport::default();
    let area = cfg.filament_area();
    let mut volumetric = false;
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
    let mut in_perimeter = false;
    let mut walls: Vec<((f64, f64), (f64, f64))> = Vec::new();
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
    // How far a machine's own prime line runs past the bed edge: a Prusa profile
    // primes at Y -2.5, and a 5 mm line leaves room for a wider one.
    let prime_margin = 10.0;
    let max_errors = 25;
    for (index, line) in gcode.lines().enumerate() {
        let number = index + 1;
        if let Some(rest) = line.trim().strip_prefix(';') {
            let tag = rest.trim();
            if tag == "LAYER_CHANGE" {
                r.layers += 1;
                walls.clear();
            } else if let Some(kind) = tag.strip_prefix("TYPE:") {
                in_perimeter = perimeter_type(kind.trim());
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
            "M200" => {
                if get(&c, 'D').is_some_and(|d| d > 0.0) {
                    volumetric = true;
                }
            }
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
            "G0" | "G1" | "G2" | "G3" => {
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
                // Plastic outside the bed is a ruined print; the head going there is
                // every machine's park, prime and wipe. A reference profile primes
                // along Y -2.5 and parks at X bed+5 as a matter of course, so the
                // outline is held to moves that extrude, grown by a prime line.
                let prints = extrusion.is_some_and(|delta| delta > 0.0);
                if prints
                    && x.is_finite()
                    && (x < min_x - prime_margin - tolerance
                        || x > half_x + prime_margin + tolerance)
                {
                    push(
                        &mut r,
                        max_errors,
                        format!("line {number}: X {x:.3} is outside the bed"),
                    );
                }
                if prints && x.is_finite() && y.is_finite() && cfg.bed_shape.len() >= 3 {
                    let (sx, sy) = if cfg.origin_center {
                        (half_x, half_y)
                    } else {
                        (0.0, 0.0)
                    };
                    if !cfg.bed_contains_within(x + sx, y + sy, prime_margin) {
                        push(
                            &mut r,
                            max_errors,
                            format!("line {number}: X {x:.3} Y {y:.3} is outside the bed outline"),
                        );
                    }
                }
                if prints
                    && y.is_finite()
                    && (y < min_y - prime_margin - tolerance
                        || y > half_y + prime_margin + tolerance)
                {
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
                let arc = if c.code == "G2" || c.code == "G3" {
                    match (get(&c, 'I'), get(&c, 'J')) {
                        (Some(i), Some(j)) if moved_xy => {
                            match arc_length(
                                previous.0,
                                previous.1,
                                x,
                                y,
                                i,
                                j,
                                arc_is_clockwise(c.code),
                            ) {
                                Some(length) => Some(length),
                                None => {
                                    push(
                                        &mut r,
                                        max_errors,
                                        format!(
                                            "line {number}: arc endpoints are not on one circle about I{i} J{j}"
                                        ),
                                    );
                                    Some(0.0)
                                }
                            }
                        }
                        _ => {
                            push(
                                &mut r,
                                max_errors,
                                format!("line {number}: an arc needs both I and J"),
                            );
                            Some(0.0)
                        }
                    }
                } else {
                    None
                };
                let distance = match arc {
                    Some(length) => length,
                    None if moved_xy => {
                        ((x - previous.0).powi(2) + (y - previous.1).powi(2)).sqrt()
                    }
                    None => 0.0,
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
                                let descends_legally = cfg.print_sequence
                                    == hfw_config::PrintSequence::ByObject
                                    && cfg.gantry_height > 0.0
                                    && max_extrusion_z <= cfg.gantry_height + 1e-6;
                                if !descends_legally {
                                    push(
                                        &mut r,
                                        max_errors,
                                        format!(
                                            "line {number}: extruding at Z {z:.3} below an earlier layer at {max_extrusion_z:.3}"
                                        ),
                                    );
                                }
                            }
                            max_extrusion_z = max_extrusion_z.max(z);
                            if r.first_extrusion_z == 0.0 {
                                r.first_extrusion_z = z;
                            }
                        }
                        let volume = if volumetric { delta } else { delta * area };
                        r.volume_mm3 += volume;
                        r.filament_mm += if volumetric { delta / area } else { delta };
                        r.est_seconds += distance / speed;
                        let flow = volume / (distance / speed);
                        // A zero ceiling is the reference slicers' spelling of "no
                        // limit", so it is not a ceiling to exceed.
                        if cfg.max_volumetric_speed > 0.0 && flow > cfg.max_volumetric_speed * 1.05
                        {
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
                        if in_perimeter && moved_xy {
                            walls.push((previous, (x, y)));
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
                            if moved_xy
                                && walls
                                    .iter()
                                    .any(|&(c, d)| segments_cross(previous, (x, y), c, d))
                            {
                                r.travel_crossings += 1;
                            }
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
