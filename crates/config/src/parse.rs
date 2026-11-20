//! `key = value` profile parsing and validation.

use std::collections::HashMap;
use std::path::Path;

use crate::dialect::Preset;
use crate::error::ConfigError;
use crate::{
    Flavor, PrintSequence, PrinterConfig, Seam, SliceMode, Substitution, Thumbnail, ThumbnailFormat,
};

fn number(key: &str, value: &str) -> Result<f64, ConfigError> {
    let text = value.trim();
    // The dialect writes a rate or a density as `30%` where this engine keeps the
    // fraction, and a `%` reaching the number reader used to end the whole key as
    // a spelling it could not convert.
    let (body, percent) = match text.strip_suffix('%') {
        Some(body) => (body.trim(), true),
        None => (text, false),
    };
    match body.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(if percent { v / 100.0 } else { v }),
        _ => Err(ConfigError::NotANumber {
            key: key.to_owned(),
            value: text.to_owned(),
        }),
    }
}

/// Reads a percentage, which the dialect writes either as `15%` or as `0.15`.
fn percentage(key: &str, value: &str) -> Result<f64, ConfigError> {
    let text = value.trim();
    if let Some(percent) = text.strip_suffix('%') {
        return Ok(number(key, percent)? / 100.0);
    }
    let plain = number(key, text)?;
    Ok(if plain > 1.0 { plain / 100.0 } else { plain })
}

/// Reads a pressure-advance switch written where the number belongs.
///
/// `enabled`/`true`/`on`/`1` keep the number the profile states elsewhere, and
/// `disabled`/`false`/`off`/`0` turn the feature off. Anything else is the parse
/// error it would have been without this rule.
fn pressure_advance_switch(key: &str, value: &str, current: f64) -> Result<f64, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "enabled" | "enable" | "true" | "on" | "1" => Ok(current),
        "disabled" | "disable" | "false" | "off" | "0" => Ok(0.0),
        _ => Err(ConfigError::NotANumber {
            key: key.to_owned(),
            value: value.trim().to_owned(),
        }),
    }
}

/// Reads a `gcode_substitutions` list, which the dialect writes as groups of
/// four: what to find, what to put there, flag letters, and a note.
fn substitutions(value: &str) -> Result<Vec<Substitution>, ConfigError> {
    let items = split_quoted(value);
    if items.len() % 4 != 0 {
        return Err(ConfigError::OutOfRange {
            key: "gcode_substitutions",
            rule: "groups of four: search, replacement, flags, note",
            value: format!("{} entries", items.len()),
        });
    }
    let mut out = Vec::new();
    for group in items.chunks(4) {
        let flags = group[2].to_ascii_lowercase();
        if let Some(bad) = flags.chars().find(|c| !matches!(c, 'r' | 'i' | 'w' | 's')) {
            return Err(ConfigError::OutOfRange {
                key: "gcode_substitutions",
                rule: "flag letters from r, i, w, s",
                value: bad.to_string(),
            });
        }
        out.push(Substitution {
            pattern: group[0].clone(),
            format: group[1].clone(),
            flags,
            note: group[3].clone(),
        });
    }
    Ok(out)
}

/// Splits a comma list, keeping commas inside double quotes.
fn split_quoted(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for ch in value.trim().chars() {
        match ch {
            '\\' if quoted && !escaped => {
                escaped = true;
                current.push(ch);
            }
            '"' if !escaped => {
                quoted = !quoted;
            }
            ',' if !quoted => {
                let entry = current.trim().trim_matches('"').to_owned();
                out.push(entry);
                current.clear();
            }
            _ => {
                escaped = false;
                current.push(ch);
            }
        }
    }
    let entry = current.trim().trim_matches('"').to_owned();
    if !entry.is_empty() || !out.is_empty() {
        out.push(entry);
    }
    out
}

/// Reads the first entry of a comma list, for settings a printer firmware
/// stores per axis or per tool.
///
/// PrusaSlicer and Orca write `machine_max_acceleration_x = 5000,5000` on a
/// multi-extruder machine and the engine itself reads only `.front()`, so the
/// first entry is the machine's value; a single number is the same list of one.
fn first_number(key: &str, value: &str) -> Result<f64, ConfigError> {
    let head = value.split(',').next().unwrap_or("").trim();
    number(key, head)
}

/// A bed outline written the way the reference dialects write one:
/// `0x0,350x0,350x350,0x350`.
///
/// A configuration dump carries the same outline as a JSON array of points,
/// `[[0,0],[350,0],[350,350],[0,350]]`, which flattens to a plain list of
/// numbers. Both spellings are read: a list with no `x` in it is the numbers of
/// the points in order.
fn bed_outline(value: &str) -> Result<Vec<(f64, f64)>, ConfigError> {
    if !value.contains(['x', 'X']) {
        let corners: Vec<f64> = value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| {
                part.parse::<f64>().map_err(|_| ConfigError::BadValue {
                    key: "bed_shape",
                    why: format!("`{part}` is not a number; a corner reads 350x0"),
                })
            })
            .collect::<Result<_, _>>()?;
        if corners.len() % 2 != 0 {
            return Err(ConfigError::BadValue {
                key: "bed_shape",
                why: format!("{} number(s); points come in pairs", corners.len()),
            });
        }
        return finish_bed(corners.chunks_exact(2).map(|p| (p[0], p[1])).collect());
    }
    let mut points = Vec::new();
    for pair in value.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let split = pair
            .split_once(['x', 'X'])
            .ok_or_else(|| ConfigError::BadValue {
                key: "bed_shape",
                why: format!("`{pair}` is not a point; a corner reads 350x0"),
            })?;
        let corner = |text: &str| -> Result<f64, ConfigError> {
            text.trim()
                .parse::<f64>()
                .map_err(|_| ConfigError::BadValue {
                    key: "bed_shape",
                    why: format!("`{pair}` is not a point; a corner reads 350x0"),
                })
        };
        points.push((corner(split.0)?, corner(split.1)?));
    }
    finish_bed(points)
}

fn finish_bed(points: Vec<(f64, f64)>) -> Result<Vec<(f64, f64)>, ConfigError> {
    if points.len() < 3 {
        return Err(ConfigError::BadValue {
            key: "bed_shape",
            why: format!("{} point(s); a bed needs at least three", points.len()),
        });
    }
    Ok(points)
}

fn boolean(key: &str, value: &str) -> Result<bool, ConfigError> {
    // A switch's spelling differs between the dialects: PrusaSlicer and Orca write
    // `enabled`/`disabled`, some write the switch as a count, and a support switch
    // writes what to support rather than whether to support at all.
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" | "enabled" | "enable" => Ok(true),
        "emit_center" | "emit" => Ok(true),
        "false" | "no" | "off" | "0" | "disabled" | "disable" => Ok(false),
        "everywhere" | "auto" | "buildplate" | "build_plate" => Ok(true),
        "none" => Ok(false),
        other => match other.parse::<f64>() {
            Ok(v) if v.is_finite() => Ok(v != 0.0),
            _ => Err(ConfigError::NotABoolean {
                key: key.to_owned(),
                value: value.trim().to_owned(),
            }),
        },
    }
}

/// Reads a `z:height` step list, the readable form of a layer-height profile.
fn profile(key: &str, value: &str) -> Result<Vec<(f64, f64)>, ConfigError> {
    let mut steps = Vec::new();
    for entry in value
        .split([',', ';'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        let (at, height) = entry
            .split_once(':')
            .ok_or_else(|| ConfigError::NotANumber {
                key: key.to_owned(),
                value: entry.to_owned(),
            })?;
        let (z, h) = (number(key, at)?, number(key, height)?);
        if h <= 0.0 {
            return Err(ConfigError::OutOfRange {
                key: "layer_height_profile",
                rule: "a positive layer height after the colon",
                value: entry.to_owned(),
            });
        }
        steps.push((z, h));
    }
    Ok(steps)
}

/// Reads the first value of a key the dialects store per extruder.
fn leading(key: &str, value: &str) -> Result<f64, ConfigError> {
    let first = value.split([',', ';']).next().unwrap_or("").trim();
    number(key, first)
}

/// Reads a solid-surface fill pattern.
///
/// The dialect offers eight; this engine draws five of them. The three curve
/// patterns are refused rather than replaced by a straight-line skin: a top face
/// is the surface a print is looked at, so the wrong pattern there is a wrong
/// part, not a missing option.
fn skin_pattern(key: &str, value: &str) -> Result<String, ConfigError> {
    const ALLOWED: &str = "rectilinear, monotonic, monotoniclines, alignedrectilinear, \
        concentric, hilbertcurve, archimedeanchords, octagramspiral";
    let lowered = value.trim().to_ascii_lowercase().replace('_', "");
    let name = match lowered.as_str() {
        "rectilinear" | "monotonic" | "monotoniclines" | "alignedrectilinear" | "concentric"
        | "hilbertcurve" | "archimedeanchords" | "octagramspiral" => lowered,
        _ => {
            return Err(ConfigError::BadChoice {
                key: if key == "top_fill_pattern" {
                    "top_fill_pattern"
                } else {
                    "bottom_fill_pattern"
                },
                value: value.trim().to_owned(),
                allowed: ALLOWED,
            })
        }
    };
    Ok(name)
}

/// Reads a percentage as the number of percent, not as a fraction.
///
/// The dialect writes a percentage setting either as `25%` or as `25`, and both
/// mean twenty-five. This is deliberately not the same rule as an infill
/// density, where `0.25` means a quarter: a shrinkage of `0.8` is eight tenths
/// of one percent, and treating it as eighty would scale a model five times.
fn percent_value(key: &str, value: &str) -> Result<f64, ConfigError> {
    let text = value.trim();
    match text.strip_suffix('%') {
        Some(digits) => number(key, digits),
        None => number(key, text),
    }
}

/// Reads a value that must be one of a fixed list of words.
fn choice(key: &str, value: &str, allowed: &'static [&'static str]) -> Result<String, ConfigError> {
    let lowered = value.trim().to_ascii_lowercase();
    if !allowed.contains(&lowered.as_str()) {
        return Err(ConfigError::BadChoice {
            key: if key == "machine_limits_usage" {
                "machine_limits_usage"
            } else {
                "perimeter_generator"
            },
            value: value.trim().to_owned(),
            allowed: match key {
                "machine_limits_usage" => "emit_to_gcode, time_estimate_only, ignore",
                _ => "classic, arachne",
            },
        });
    }
    Ok(lowered)
}

fn whole(key: &str, value: &str) -> Result<usize, ConfigError> {
    let n = number(key, value)?;
    if n < 0.0 || n > u32::MAX as f64 {
        return Err(ConfigError::OutOfRange {
            key: "count",
            rule: "a non-negative whole number",
            value: value.trim().to_owned(),
        });
    }
    Ok(n.round() as usize)
}

/// Reads an extrusion width, which the dialect writes either in millimetres or
/// as a percentage of the nozzle diameter (`112%`).
///
/// A percentage has to be resolved against the *final* nozzle diameter, which a
/// profile may declare after the width, so callers re-run the width keys once
/// every key has been read.
pub(crate) fn width_mm(key: &str, value: &str, nozzle: f64) -> Result<f64, ConfigError> {
    let text = value.trim();
    if let Some(percent) = text.strip_suffix('%') {
        let pct = number(key, percent)?;
        if pct < 0.0 {
            return Err(ConfigError::OutOfRange {
                key: "extrusion width",
                rule: "zero or a positive percentage",
                value: text.to_owned(),
            });
        }
        return Ok(pct / 100.0 * nozzle);
    }
    number(key, value)
}

impl PrinterConfig {
    /// Reads the pressure advance, which arrives either as the number itself or as
    /// a switch with the number under the other spelling.
    ///
    /// The reference slicers and this app's own presets both split it in two: Orca's
    /// filament profile carries `enable_pressure_advance` and `pressure_advance`,
    /// and a next-layer preset carries `pressure_advance` (on/off) and
    /// `pressure_advance_value`. Whichever spelling holds the word, the word means
    /// "use the number beside me", and the off words mean zero. A numeric anywhere
    /// in the pair wins, because two numbers is a profile restating the same value
    /// while a number and a switch is one statement and one permission.
    fn set_pressure_advance(&mut self, key: &str, value: &str) -> Result<(), ConfigError> {
        match number(key, value) {
            Ok(advance) => self.pressure_advance = advance,
            Err(_) => {
                self.pressure_advance = pressure_advance_switch(key, value, self.pressure_advance)?
            }
        }
        Ok(())
    }

    /// Re-resolves every extrusion width against the final nozzle diameter.
    pub(crate) fn resolve_widths(
        &mut self,
        entries: &[(String, String)],
    ) -> Result<(), ConfigError> {
        for (key, value) in entries {
            let resolved = match key.as_str() {
                "wall_width" | "perimeter_extrusion_width" => "wall_width",
                "outer_wall_width" | "external_perimeter_extrusion_width" => "outer_wall_width",
                "infill_width" | "infill_extrusion_width" => "infill_width",
                "solid_infill_width" | "solid_infill_extrusion_width" => "solid_infill_width",
                "first_layer_width" | "first_layer_extrusion_width" => "first_layer_width",
                "top_infill_width" | "top_infill_extrusion_width" => "top_infill_width",
                "support_width" | "support_material_extrusion_width" => "support_width",
                "support_interface_width" => "support_interface_width",
                _ => continue,
            };
            let mm = width_mm(resolved, value, self.nozzle_diameter)?;
            match resolved {
                "wall_width" => self.wall_width = mm,
                "outer_wall_width" => self.outer_wall_width = mm,
                "infill_width" => self.infill_width = mm,
                "solid_infill_width" => self.solid_infill_width = mm,
                "first_layer_width" => self.first_layer_width = mm,
                "top_infill_width" => self.top_infill_width = mm,
                "support_width" => self.support_width = mm,
                _ => self.support_interface_width = mm,
            }
        }
        if self.automatic_extrusion_widths {
            let auto = self.nozzle_diameter * 1.125;
            for width in [
                &mut self.wall_width,
                &mut self.outer_wall_width,
                &mut self.infill_width,
                &mut self.solid_infill_width,
                &mut self.top_infill_width,
                &mut self.first_layer_width,
                &mut self.support_width,
            ] {
                if *width == 0.0 {
                    *width = auto;
                }
            }
        }
        for (key, value) in entries {
            let base = if self.infill_width > 0.0 {
                self.infill_width
            } else {
                self.nozzle_diameter
            };
            match key.as_str() {
                "infill_anchor" => self.infill_anchor = width_mm(key, value, base)?,
                "infill_anchor_max" => self.infill_anchor_max = width_mm(key, value, base)?,
                "min_feature_size" => {
                    self.min_feature_size = width_mm(key, value, self.nozzle_diameter)?
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl PrinterConfig {
    /// Reads and validates a profile from disk.
    ///
    /// A file opening with `{` is a settings document — what this application dumps
    /// and what another slicer exports as JSON — and is read through the dialect
    /// intake, exactly as the command line's `--preset` reads it. Every other file
    /// is a profile in this engine's own `key = value` form.
    pub fn load(path: &Path) -> Result<PrinterConfig, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        if text.trim_start().starts_with('{') {
            let preset = Preset::parse(&text)?;
            let (config, _) = PrinterConfig::from_preset(&preset);
            return Ok(config);
        }
        PrinterConfig::parse(&text)
    }

    /// Parses and validates a profile from text.
    pub fn parse(text: &str) -> Result<PrinterConfig, ConfigError> {
        let mut c = PrinterConfig::default();
        let mut blocks: HashMap<String, String> = HashMap::new();
        let mut widths: Vec<(String, String)> = Vec::new();
        let mut lines = text.lines().enumerate();
        while let Some((ln, raw)) = lines.next() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(ConfigError::MissingEquals { line: ln + 1 });
            };
            let (key, value) = (key.trim(), value.trim());
            if let Some(tag) = value.strip_prefix("<<") {
                let tag = tag.trim().to_owned();
                let mut body: Vec<&str> = Vec::new();
                let mut closed = false;
                for (_, next) in lines.by_ref() {
                    if next.trim() == tag {
                        closed = true;
                        break;
                    }
                    body.push(next);
                }
                if !closed {
                    return Err(ConfigError::UnclosedBlock {
                        line: ln + 1,
                        key: key.to_owned(),
                        tag,
                    });
                }
                blocks.insert(key.to_owned(), body.join("\n"));
                continue;
            }
            c.assign(ln + 1, key, value)?;
            if key.ends_with("_width")
                || matches!(
                    key,
                    "infill_anchor" | "infill_anchor_max" | "min_feature_size"
                )
            {
                widths.push((key.to_owned(), value.to_owned()));
            }
        }
        if let Some(start) = blocks.remove("start_gcode") {
            c.start_gcode = start;
        }
        if let Some(end) = blocks.remove("end_gcode") {
            c.end_gcode = end;
        }
        if let Some(key) = blocks.keys().next() {
            return Err(ConfigError::UnknownBlock { key: key.clone() });
        }
        c.resolve_widths(&widths)?;
        c.adopt_bed_shape();
        c.validate()?;
        Ok(c)
    }

    /// Takes `bed_x` and `bed_y` from the outline's span.
    ///
    /// Placement centres the part on half of each, and the fit check compares the
    /// part against all of each, so a polygon profile that left the two numbers at
    /// their defaults would place and refuse parts against a bed it does not have.
    /// Raises the machine speed ceiling to cover the speeds the profile asks for.
    ///
    /// `max_speed` is this engine's own limit and starts at a conservative
    /// default, but a profile that plots a bead at 350 mm/s has already decided
    /// what its machine can do. Leaving the default in place would make the
    /// writer emit a feed the profile itself asked for and the verifier then
    /// reject it, which is a contradiction rather than a safety check. A profile
    /// that states `max_speed` itself is never raised: that number is a
    /// deliberate throttle.
    pub(crate) fn adopt_speed_ceiling(&mut self) {
        if self.max_speed <= 0.0 {
            return;
        }
        let fastest = [
            self.outer_wall_speed,
            self.inner_wall_speed,
            self.thin_speed,
            self.infill_speed,
            self.solid_speed,
            self.top_solid_speed,
            self.support_speed,
            self.support_interface_speed,
            self.travel_speed,
            self.travel_speed_z,
            self.bridge_speed,
            self.ironing_speed,
            self.first_layer_speed,
            self.first_layer_infill_speed,
            self.first_layer_outer_speed,
            self.first_layer_inner_speed,
            self.first_layer_solid_speed,
            self.first_layer_thin_speed,
            self.first_layer_support_speed,
            self.first_layer_top_solid_speed,
            self.retract_speed,
            self.deretract_speed,
        ]
        .into_iter()
        .fold(0.0f64, f64::max);
        if fastest > self.max_speed {
            self.max_speed = fastest;
        }
    }

    pub(crate) fn adopt_bed_shape(&mut self) {
        if self.bed_shape.len() < 3 {
            return;
        }
        let mut min = (f64::INFINITY, f64::INFINITY);
        let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for &(x, y) in &self.bed_shape {
            min = (min.0.min(x), min.1.min(y));
            max = (max.0.max(x), max.1.max(y));
        }
        self.bed_shape
            .iter_mut()
            .for_each(|point| *point = (point.0 - min.0, point.1 - min.1));
        self.bed_x = max.0 - min.0;
        self.bed_y = max.1 - min.1;
    }

    pub(crate) fn assign(
        &mut self,
        line: usize,
        key: &str,
        value: &str,
    ) -> Result<(), ConfigError> {
        if let Some(native) = crate::alias_table::native_for(key) {
            if native != key {
                let converted = crate::dialect::convert_value(native, value);
                return self
                    .assign(line, native, &converted)
                    .map_err(|error| error.instead_of(key));
            }
        }
        match key {
            "name" => self.name = value.to_owned(),
            "bed_shape" => self.bed_shape = bed_outline(value)?,
            "bed_x" => self.bed_x = number(key, value)?,
            "bed_y" => self.bed_y = number(key, value)?,
            "max_z" => self.max_z = number(key, value)?,
            "origin_center" => self.origin_center = boolean(key, value)?,
            "flavor" => {
                self.flavor = match value {
                    "klipper" => Flavor::Klipper,
                    "marlin" => Flavor::Marlin,
                    _ => {
                        return Err(ConfigError::BadChoice {
                            key: "flavor",
                            value: value.to_owned(),
                            allowed: "klipper, marlin",
                        })
                    }
                }
            }
            "relative_e" => self.relative_e = boolean(key, value)?,
            "nozzle_diameter" => self.nozzle_diameter = number(key, value)?,
            "filament_diameter" => self.filament_diameter = number(key, value)?,
            "filament_density" => self.filament_density = number(key, value)?,
            "line_width" => self.line_width = number(key, value)?,
            "min_bead_width" => self.min_bead_width = width_mm(key, value, self.nozzle_diameter)?,
            "layer_height" => self.layer_height = number(key, value)?,
            "first_layer_height" => self.first_layer_height = number(key, value)?,
            "variable_layer_height" => self.variable_layer_height = boolean(key, value)?,
            "spiral_vase" => self.spiral_vase = boolean(key, value)?,
            "avoid_crossing_perimeters" => self.avoid_crossing_perimeters = boolean(key, value)?,
            "avoid_crossing_curled_overhangs" => {
                self.avoid_crossing_curled_overhangs = boolean(key, value)?
            }
            "travel_lift_before_obstacle" => {
                self.travel_lift_before_obstacle = boolean(key, value)?
            }
            "avoid_crossing_perimeters_max_detour" => {
                self.avoid_crossing_perimeters_max_detour = number(key, value)?
            }
            "thin_walls" => self.thin_walls = boolean(key, value)?,
            "min_feature_size" => {
                self.min_feature_size = width_mm(key, value, self.nozzle_diameter)?
            }
            "infill_anchor" => self.infill_anchor = width_mm(key, value, self.nozzle_diameter)?,
            "infill_anchor_max" => {
                self.infill_anchor_max = width_mm(key, value, self.nozzle_diameter)?
            }
            "layer_height_profile" => self.layer_height_profile = profile(key, value)?,
            "min_layer_height" => self.min_layer_height = leading(key, value)?,
            "max_layer_height" => self.max_layer_height = leading(key, value)?,
            "walls" => self.walls = whole(key, value)?,
            "infill_density" => self.infill_density = number(key, value)?,
            "infill_pattern" => self.infill_pattern = value.to_owned(),
            "top_layers" => self.top_layers = whole(key, value)?,
            "bottom_layers" => self.bottom_layers = whole(key, value)?,
            "nozzle_temp" => self.nozzle_temp = number(key, value)?,
            "first_nozzle_temp" => self.first_nozzle_temp = number(key, value)?,
            "bed_temp" => self.bed_temp = number(key, value)?,
            "first_layer_bed_temp" => self.first_layer_bed_temp = number(key, value)?,
            "chamber_temp" => self.chamber_temp = number(key, value)?,
            "fan_speed" => self.fan_speed = number(key, value)?,
            "cooling" => self.cooling = boolean(key, value)?,
            "fan_always_on" => self.fan_always_on = boolean(key, value)?,
            "initial_layer_fan_speed" => self.initial_layer_fan_speed = number(key, value)?,
            "ironing_fan_speed" => self.ironing_fan_speed = number(key, value)?,
            "support_interface_fan_speed" => self.support_interface_fan_speed = number(key, value)?,
            "dynamic_fan_speeds" => self.dynamic_fan_speeds = boolean(key, value)?,
            "overhang_fan_speed_0" => self.overhang_fan_speeds[0] = number(key, value)?,
            "overhang_fan_speed_1" => self.overhang_fan_speeds[1] = number(key, value)?,
            "overhang_fan_speed_2" => self.overhang_fan_speeds[2] = number(key, value)?,
            "overhang_fan_speed_3" => self.overhang_fan_speeds[3] = number(key, value)?,
            "fan_start_layer" => self.fan_start_layer = whole(key, value)?,
            "retract_len" => self.retract_len = number(key, value)?,
            "retract_speed" => self.retract_speed = number(key, value)?,
            "retract_min_travel" => self.retract_min_travel = number(key, value)?,
            "infill_retract_min_travel" => self.infill_retract_min_travel = number(key, value)?,
            "z_hop" => self.z_hop = number(key, value)?,
            "travel_speed" => self.travel_speed = number(key, value)?,
            "travel_speed_z" => self.travel_speed_z = number(key, value)?,
            "dynamic_overhang_speeds" => self.dynamic_overhang_speeds = boolean(key, value)?,
            "overhang_speed_0" => self.overhang_speeds[0] = number(key, value)?,
            "overhang_speed_1" => self.overhang_speeds[1] = number(key, value)?,
            "overhang_speed_2" => self.overhang_speeds[2] = number(key, value)?,
            "overhang_speed_3" => self.overhang_speeds[3] = number(key, value)?,
            "first_layer_speed" => self.first_layer_speed = number(key, value)?,
            "first_layer_perimeter_speed" => self.first_layer_perimeter_speed = number(key, value)?,
            "first_layer_outer_speed" => self.first_layer_outer_speed = number(key, value)?,
            "first_layer_inner_speed" => self.first_layer_inner_speed = number(key, value)?,
            "first_layer_thin_speed" => self.first_layer_thin_speed = number(key, value)?,
            "first_layer_infill_speed" => self.first_layer_infill_speed = number(key, value)?,
            "first_layer_solid_speed" => self.first_layer_solid_speed = number(key, value)?,
            "first_layer_top_solid_speed" => self.first_layer_top_solid_speed = number(key, value)?,
            "first_layer_support_speed" => self.first_layer_support_speed = number(key, value)?,
            "outer_wall_speed" => self.outer_wall_speed = number(key, value)?,
            "inner_wall_speed" => self.inner_wall_speed = number(key, value)?,
            "thin_speed" => self.thin_speed = number(key, value)?,
            "infill_speed" => self.infill_speed = number(key, value)?,
            "solid_speed" => self.solid_speed = number(key, value)?,
            "top_solid_speed" => self.top_solid_speed = number(key, value)?,
            "support_speed" => self.support_speed = number(key, value)?,
            "support_interface_speed" => self.support_interface_speed = number(key, value)?,
            "support" => {
                // `enforcers_only` is the dialect's "support what the painter marked
                // and nothing else": it turns support on with the painted facets as
                // the only seeds, rather than saying nothing about support at all.
                let text = value.trim().to_ascii_lowercase();
                self.support_painted_only = text == "enforcers_only";
                self.support = if self.support_painted_only {
                    true
                } else {
                    boolean(key, value)?
                }
            }
            "support_threshold_angle" => self.support_threshold_angle = number(key, value)?,
            "support_xy_gap" => self.support_xy_gap = number(key, value)?,
            "support_z_gap" => self.support_z_gap = number(key, value)?,
            "support_interface_layers" => self.support_interface_layers = whole(key, value)?,
            "support_density" => self.support_density = number(key, value)?,
            "support_interface_density" => self.support_interface_density = number(key, value)?,
            "support_material_spacing" => self.support_spacing = number(key, value)?,
            "support_material_interface_spacing" => {
                self.support_interface_spacing = number(key, value)?
            }
            "support_on_build_plate_only" => {
                self.support_on_build_plate_only = boolean(key, value)?
            }
            "support_min_area" => self.support_min_area = number(key, value)?,
            "support_style" => {
                // `support_material_style` in the dialect names the look of the
                // generated support; this engine generates two shapes. A spelling
                // outside the list stays a reported gap rather than a silent guess.
                let style = value.trim().to_ascii_lowercase();
                self.support_style = match style.as_str() {
                    "normal" | "grid" | "snug" | "support_material_style" => "normal".to_owned(),
                    "tree" | "organic" | "tree slim" | "tree strong" | "tree hybrid" => {
                        "tree".to_owned()
                    }
                    _ => return Err(ConfigError::BadChoice {
                        key: "support_style",
                        value: value.to_owned(),
                        allowed:
                            "normal, grid, snug, tree, organic, tree slim, tree strong, tree hybrid",
                    }),
                }
            }
            "support_branch_angle" => self.support_branch_angle = number(key, value)?,
            "support_branch_distance" => self.support_branch_distance = number(key, value)?,
            "support_tip_diameter" => self.support_tip_diameter = number(key, value)?,
            "support_branch_diameter_angle" => {
                self.support_branch_diameter_angle = number(key, value)?
            }
            "support_max_branch_diameter" => self.support_max_branch_diameter = number(key, value)?,
            "max_volumetric_speed" => self.max_volumetric_speed = number(key, value)?,
            "max_volumetric_extrusion_rate_slope_positive" => {
                self.max_volumetric_rate_slope_positive = number(key, value)?
            }
            "max_volumetric_extrusion_rate_slope_negative" => {
                self.max_volumetric_rate_slope_negative = number(key, value)?
            }
            "default_accel" => self.default_accel = number(key, value)?,
            "outer_wall_accel" => self.outer_wall_accel = number(key, value)?,
            "inner_wall_accel" => self.inner_wall_accel = number(key, value)?,
            "perimeter_accel" => self.perimeter_accel = number(key, value)?,
            "top_solid_accel" => self.top_solid_accel = number(key, value)?,
            "initial_layer_accel" => self.initial_layer_accel = number(key, value)?,
            "solid_accel" => self.solid_accel = number(key, value)?,
            "infill_accel" | "infill_acceleration" => self.infill_accel = number(key, value)?,
            "support_accel" => self.support_accel = number(key, value)?,
            "support_interface_accel" => self.support_interface_accel = number(key, value)?,
            "machine_max_accel_x" => self.machine_max_accel_x = first_number(key, value)?,
            "machine_max_accel_y" => self.machine_max_accel_y = first_number(key, value)?,
            "machine_max_accel_z" => self.machine_max_accel_z = first_number(key, value)?,
            "machine_max_accel_e" => self.machine_max_accel_e = first_number(key, value)?,
            "machine_max_feedrate_x" => self.machine_max_feedrate_x = first_number(key, value)?,
            "machine_max_feedrate_y" => self.machine_max_feedrate_y = first_number(key, value)?,
            "machine_max_feedrate_z" => self.machine_max_feedrate_z = first_number(key, value)?,
            "machine_max_feedrate_e" => self.machine_max_feedrate_e = first_number(key, value)?,
            "machine_max_jerk_x" => self.machine_max_jerk_x = first_number(key, value)?,
            "machine_max_jerk_y" => self.machine_max_jerk_y = first_number(key, value)?,
            "machine_max_jerk_z" => self.machine_max_jerk_z = first_number(key, value)?,
            "machine_max_jerk_e" => self.machine_max_jerk_e = first_number(key, value)?,
            "machine_max_acceleration_extruding" => {
                self.machine_max_accel_extruding = first_number(key, value)?
            }
            "machine_max_acceleration_retracting" => {
                self.machine_max_accel_retracting = first_number(key, value)?
            }
            "machine_max_acceleration_travel" => {
                self.machine_max_accel_travel = first_number(key, value)?
            }
            "machine_max_junction_deviation" => {
                self.machine_max_junction_deviation = first_number(key, value)?
            }
            "machine_min_extruding_rate" => {
                self.machine_min_extruding_rate = first_number(key, value)?
            }
            "machine_min_travel_rate" => self.machine_min_travel_rate = first_number(key, value)?,
            "z_offset" => self.z_offset = number(key, value)?,
            "fill_angle" => self.fill_angle = number(key, value)?,
            "between_objects_gcode" => self.between_objects_gcode = value.to_owned(),
            "pause_print_gcode" => self.pause_print_gcode = value.to_owned(),
            "color_change_gcode" => self.color_change_gcode = value.to_owned(),
            "printer_model" => self.printer_model = value.to_owned(),
            "printer_variant" => self.printer_variant = value.to_owned(),
            "printer_vendor" => self.printer_vendor = value.to_owned(),
            "printer_notes" => self.printer_notes = value.to_owned(),
            "notes" => self.notes = value.to_owned(),
            "filament_type" => self.filament_type = value.to_owned(),
            "filament_vendor" => self.filament_vendor = value.to_owned(),
            "filament_colour" => self.filament_colour = value.to_owned(),
            "filament_notes" => self.filament_notes = value.to_owned(),
            "output_filename_format" => self.output_filename_format = value.to_owned(),
            "post_process" => self.post_process = value.to_owned(),
            "custom_parameters_print" => self.custom_parameters_print = value.to_owned(),
            "custom_parameters_filament" => self.custom_parameters_filament = value.to_owned(),
            "custom_parameters_printer" => self.custom_parameters_printer = value.to_owned(),
            "template_custom_gcode" => self.template_custom_gcode = value.to_owned(),
            "default_print" => self.default_print = value.to_owned(),
            "default_material" => self.default_material = value.to_owned(),
            "default_tool_print" => self.default_tool_print = value.to_owned(),
            "filament_cost" => self.filament_cost = number(key, value)?,
            "filament_spool_weight" => self.filament_spool_weight = number(key, value)?,
            "top_solid_min_thickness" => self.top_solid_min_thickness = number(key, value)?,
            "bottom_solid_min_thickness" => self.bottom_solid_min_thickness = number(key, value)?,
            "silent_mode" => self.silent_mode = boolean(key, value)?,
            "filament_soluble" => self.filament_soluble = boolean(key, value)?,
            "filament_abrasive" => self.filament_abrasive = boolean(key, value)?,
            "infill_overlap" => self.infill_overlap = percentage(key, value)?,
            "gcode_substitutions" => self.gcode_substitutions = substitutions(value)?,
            "min_fan_speed" => self.min_fan_speed = number(key, value)?,
            "full_fan_speed_layer" => self.full_fan_speed_layer = whole(key, value)?,
            "fan_below_layer_time" => self.fan_below_layer_time = number(key, value)?,
            "slow_down_layer_time" => self.slow_down_layer_time = number(key, value)?,
            "slow_down_min_speed" => self.slow_down_min_speed = number(key, value)?,
            "skirt_height" => self.skirt_height = whole(key, value)?.max(1),
            "idle_temp" => self.idle_temp = number(key, value)?,
            "deretract_speed" => self.deretract_speed = number(key, value)?,
            "retract_restart_extra" => self.retract_restart_extra = number(key, value)?,
            "retract_layer_change" => self.retract_layer_change = boolean(key, value)?,
            "wipe" => self.wipe = boolean(key, value)?,
            "wipe_distance" => self.wipe_distance = number(key, value)?,
            "max_speed" => self.max_speed = number(key, value)?,
            "min_layer_time" => self.min_layer_time = number(key, value)?,
            "min_speed_factor" => self.min_speed_factor = number(key, value)?,
            "min_print_speed" => self.min_print_speed = number(key, value)?,
            "cooling_slowdown_logic" => {
                let logic = value.trim().to_ascii_lowercase();
                self.cooling_slowdown_logic = if logic.contains("consistent") {
                    "consistent_surface".to_owned()
                } else if logic.contains("proportional") {
                    "proportional".to_owned()
                } else {
                    "uniform".to_owned()
                }
            }
            "cooling_perimeter_transition_distance" => {
                self.cooling_perimeter_transition_distance = number(key, value)?
            }
            "flow" => self.flow = number(key, value)?,
            "seam" | "seam_position" => {
                self.seam = match value {
                    "aligned" => Seam::Aligned,
                    "nearest" => Seam::Nearest,
                    "random" => Seam::Random,
                    "rear" => Seam::Rear,
                    _ => {
                        return Err(ConfigError::BadChoice {
                            key: "seam_position",
                            value: value.to_owned(),
                            allowed: "aligned, nearest, random, rear",
                        })
                    }
                }
            }
            "inner_walls_first" => self.inner_walls_first = boolean(key, value)?,
            "external_perimeters_first" => {
                self.inner_walls_first = !boolean(key, value)?;
            }
            "ensure_vertical_shell_thickness" => {
                self.ensure_vertical_shell_thickness = match value {
                    "disabled" => "disabled".to_owned(),
                    "partial" => "partial".to_owned(),
                    "enabled" => "enabled".to_owned(),
                    _ => {
                        return Err(ConfigError::BadChoice {
                            key: "ensure_vertical_shell_thickness",
                            value: value.to_owned(),
                            allowed: "disabled, partial, enabled",
                        })
                    }
                }
            }
            "print_sequence" => {
                self.print_sequence = match value {
                    "by layer" | "by_layer" | "0" => PrintSequence::ByLayer,
                    "by object" | "by_object" | "1" => PrintSequence::ByObject,
                    _ => {
                        return Err(ConfigError::BadChoice {
                            key: "print_sequence",
                            value: value.to_owned(),
                            allowed: "by layer, by object",
                        })
                    }
                }
            }
            "gantry_height" => self.gantry_height = number(key, value)?,
            "extruder_clearance_radius" => self.extruder_clearance_radius = number(key, value)?,
            "complete_objects" => {
                self.print_sequence = if boolean(key, value)? {
                    PrintSequence::ByObject
                } else {
                    PrintSequence::ByLayer
                }
            }
            "skirt_loops" => self.skirt_loops = whole(key, value)?,
            "elefant_foot_compensation" => self.elefant_foot_compensation = number(key, value)?,
            "xy_size_compensation" => self.xy_size_compensation = number(key, value)?,
            "slice_closing_radius" => self.slice_closing_radius = number(key, value)?,
            "resolution" => self.resolution = number(key, value)?,
            "skirt_distance" => self.skirt_distance = number(key, value)?,
            "brim_loops" => self.brim_loops = whole(key, value)?,
            "brim_width" => self.brim_width = number(key, value)?,
            "brim_gap" => self.brim_gap = number(key, value)?,
            "brim_type" => match value {
                "outer_only" | "outer_and_inner" | "inner_only" | "no_brim" => {
                    self.brim_type = value.to_owned()
                }
                _ => {
                    return Err(ConfigError::BadChoice {
                        key: "brim_type",
                        value: value.to_owned(),
                        allowed: "outer_only, outer_and_inner, inner_only, no_brim",
                    })
                }
            },
            "brim_separation" => self.brim_separation = number(key, value)?,
            "min_skirt_length" => self.min_skirt_length = number(key, value)?,
            "raft_layers" => self.raft_layers = whole(key, value)?,
            "raft_first_layer_density" => self.raft_first_layer_density = number(key, value)?,
            "raft_first_layer_expansion" => self.raft_first_layer_expansion = number(key, value)?,
            "raft_contact_distance" => self.raft_contact_distance = number(key, value)?,
            "draft_shield" => match value {
                "disabled" | "enabled" | "limited" => self.draft_shield = value.to_owned(),
                _ => {
                    return Err(ConfigError::BadChoice {
                        key: "draft_shield",
                        value: value.to_owned(),
                        allowed: "disabled, enabled, limited",
                    })
                }
            },
            "draft_shield_height" => self.draft_shield_height = number(key, value)?,
            "travel_slope" => self.travel_slope = number(key, value)?,
            "travel_ramping_lift" => self.travel_ramping_lift = boolean(key, value)?,
            "travel_max_lift" => self.travel_max_lift = number(key, value)?,
            "retract_lift_above" => self.retract_lift_above = number(key, value)?,
            "retract_lift_below" => self.retract_lift_below = number(key, value)?,
            "seam_gap" | "seam_gap_distance" => self.seam_gap = number(key, value)?,
            "staggered_inner_seams" => self.staggered_inner_seams = boolean(key, value)?,
            "scarf_seam_length" => self.scarf_seam_length = number(key, value)?,
            "scarf_seam_max_segment" => self.scarf_seam_max_segment = number(key, value)?,
            "scarf_seam_on_inner_perimeters" => self.scarf_seam_inner = boolean(key, value)?,
            "scarf_seam_start_height" => self.scarf_seam_start_height = number(key, value)?,
            "scarf_seam_placement" => {
                self.scarf_seam_placement = match value {
                    "nowhere" | "contours" | "everywhere" => value.to_owned(),
                    _ => {
                        return Err(ConfigError::BadChoice {
                            key: "scarf_seam_placement",
                            value: value.to_owned(),
                            allowed: "nowhere, contours, everywhere",
                        })
                    }
                }
            }
            "scarf_seam_entire_loop" => self.scarf_seam_entire_loop = boolean(key, value)?,
            "scarf_seam_only_on_smooth" => self.scarf_seam_only_on_smooth = boolean(key, value)?,
            "support_material_pattern" => match value {
                "rectilinear" | "grid" => self.support_pattern = value.to_owned(),
                _ => {
                    return Err(ConfigError::BadChoice {
                        key: "support_material_pattern",
                        value: value.to_owned(),
                        allowed: "rectilinear, grid",
                    })
                }
            },
            "support_material_interface_pattern" => match value {
                "rectilinear" | "grid" => self.support_interface_pattern = value.to_owned(),
                _ => {
                    return Err(ConfigError::BadChoice {
                        key: "support_material_interface_pattern",
                        value: value.to_owned(),
                        allowed: "rectilinear, grid",
                    })
                }
            },
            "support_material_interface_contact_loops" => {
                // The dialect writes this as a switch, this engine keeps the number
                // of interface loops it asks for.
                self.support_interface_loops = match whole(key, value) {
                    Ok(count) => count,
                    Err(_) if boolean(key, value)? => 1,
                    Err(_) => 0,
                }
            }
            "support_material_closing_radius" => self.support_closing_radius = number(key, value)?,
            "support_material_bottom_contact_distance" => {
                self.support_bottom_contact_distance = number(key, value)?
            }
            "dont_support_bridges" => self.support_skip_bridges = boolean(key, value)?,
            "interface_shells" => self.support_interface_shells = boolean(key, value)?,
            "support_material_with_sheath" => self.support_sheath = boolean(key, value)?,
            "support_material_first_layer_expansion" => {
                self.support_first_layer_expansion = number(key, value)?
            }
            "support_material_enforce_layers" => self.support_enforce_layers = whole(key, value)?,
            "support_material_bottom_interface_layers" => {
                self.support_bottom_interface_layers = whole(key, value)?
            }
            "support_tree_angle_slow" => self.support_tree_angle_slow = number(key, value)?,
            "support_tree_top_rate" => self.support_tree_top_rate = number(key, value)?,
            "support_tree_branch_diameter_double_wall" => {
                self.support_tree_double_wall = boolean(key, value)?
            }
            "gcode_comments" => self.gcode_comments = boolean(key, value)?,
            "gcode_label_objects" => self.label_objects = boolean(key, value)?,
            "gcode_resolution" => self.gcode_resolution = number(key, value)?,
            "use_volumetric_e" => self.volumetric_e = boolean(key, value)?,
            "use_firmware_retraction" => self.firmware_retraction = boolean(key, value)?,
            "prefer_clockwise_movements" => self.clockwise = boolean(key, value)?,
            "remaining_times" => self.remaining_times = boolean(key, value)?,
            "autoemit_temperature_commands" => self.autoemit_temperatures = boolean(key, value)?,
            "before_layer_gcode" => self.before_layer_gcode = value.to_owned(),
            "layer_gcode" => self.layer_gcode = value.to_owned(),
            "start_filament_gcode" => self.start_filament_gcode = value.to_owned(),
            "end_filament_gcode" => self.end_filament_gcode = value.to_owned(),
            "thumbnails" => {
                self.thumbnails = thumbnails(value).map_err(|why| ConfigError::BadValue {
                    key: "thumbnails",
                    why,
                })?
            }
            "thumbnails_format" => {
                self.thumbnails_format =
                    ThumbnailFormat::parse(value).ok_or_else(|| ConfigError::BadChoice {
                        key: "thumbnails_format",
                        value: value.to_owned(),
                        allowed: "PNG, QOI",
                    })?
            }
            "bed_custom_texture" => self.bed_custom_texture = value.to_owned(),
            "bed_custom_model" => self.bed_custom_model = value.to_owned(),
            "binary_gcode" => self.binary_gcode = boolean(key, value)?,
            "support_material_synchronize_layers" => {
                self.support_synchronize_layers = boolean(key, value)?
            }
            "slicing_mode" => {
                self.slicing_mode =
                    SliceMode::parse(value).ok_or_else(|| ConfigError::BadChoice {
                        key: "slicing_mode",
                        value: value.to_owned(),
                        allowed: "regular, even_odd, close_holes",
                    })?
            }
            "printer_technology" => {
                if !matches!(value.trim().to_ascii_uppercase().as_str(), "FFF" | "FDM") {
                    return Err(ConfigError::BadChoice {
                        key: "printer_technology",
                        value: value.to_owned(),
                        allowed: "FFF, FDM",
                    });
                }
            }
            "support_material_first_layer_density" => {
                self.support_first_layer_density = number(key, value)?
            }
            "fuzzy_skin" => match value {
                "none" | "external" | "all" => self.fuzzy_skin = value.to_owned(),
                _ => {
                    return Err(ConfigError::BadChoice {
                        key: "fuzzy_skin",
                        value: value.to_owned(),
                        allowed: "none, external, all",
                    })
                }
            },
            "fuzzy_skin_thickness" => self.fuzzy_skin_thickness = number(key, value)?,
            "fuzzy_skin_point_dist" => self.fuzzy_skin_point_dist = number(key, value)?,
            "bridge_speed" => self.bridge_speed = number(key, value)?,
            "bridge_flow" => self.bridge_flow = number(key, value)?,
            "bridge_density" => self.bridge_density = number(key, value)?,
            "bridge_line_width" => self.bridge_line_width = number(key, value)?,
            "wall_width" => self.wall_width = width_mm(key, value, self.nozzle_diameter)?,
            "outer_wall_width" => {
                self.outer_wall_width = width_mm(key, value, self.nozzle_diameter)?
            }
            "infill_width" => self.infill_width = width_mm(key, value, self.nozzle_diameter)?,
            "solid_infill_width" => {
                self.solid_infill_width = width_mm(key, value, self.nozzle_diameter)?
            }
            "support_width" => self.support_width = width_mm(key, value, self.nozzle_diameter)?,
            "support_interface_width" => {
                self.support_interface_width = width_mm(key, value, self.nozzle_diameter)?
            }
            "first_layer_width" => {
                self.first_layer_width = width_mm(key, value, self.nozzle_diameter)?
            }
            "top_infill_width" => {
                self.top_infill_width = width_mm(key, value, self.nozzle_diameter)?
            }
            "automatic_extrusion_widths" => self.automatic_extrusion_widths = boolean(key, value)?,
            "top_fill_pattern" => {
                self.top_fill_pattern = skin_pattern(key, value)?;
            }
            "bottom_fill_pattern" => {
                self.bottom_fill_pattern = skin_pattern(key, value)?;
            }
            "solid_infill_every_layers" => self.solid_infill_every_layers = whole(key, value)?,
            "solid_infill_below_area" => self.solid_infill_below_area = number(key, value)?,
            "filament_max_volumetric_speed" => {
                self.filament_max_volumetric_speed = number(key, value)?
            }
            "filament_infill_max_speed" => self.filament_infill_max_speed = number(key, value)?,
            "filament_infill_max_crossing_speed" => {
                self.filament_infill_max_crossing_speed = number(key, value)?
            }
            "over_bridge_speed" => self.over_bridge_speed = number(key, value)?,
            "internal_bridge_fan_speed" => self.internal_bridge_fan_speed = number(key, value)?,
            "enable_overhang_bridge_fan" => self.enable_overhang_bridge_fan = boolean(key, value)?,
            "retract_before_wipe" => self.retract_before_wipe = percent_value(key, value)?,
            "only_retract_when_crossing_perimeters" => {
                self.only_retract_when_crossing_perimeters = boolean(key, value)?
            }
            "raft_expansion" => self.raft_expansion = number(key, value)?,
            "first_layer_speed_over_raft" => self.first_layer_speed_over_raft = number(key, value)?,
            "first_layer_acceleration_over_raft" => {
                self.first_layer_acceleration_over_raft = number(key, value)?
            }
            "only_one_perimeter_first_layer" => {
                self.only_one_perimeter_first_layer = boolean(key, value)?
            }
            "top_one_perimeter_type" => {
                let lowered = value.trim().to_ascii_lowercase();
                if !["none", "top", "topmost"].contains(&lowered.as_str()) {
                    return Err(ConfigError::BadChoice {
                        key: "top_one_perimeter_type",
                        value: value.trim().to_owned(),
                        allowed: "none, top, topmost",
                    });
                }
                self.top_one_perimeter_type = lowered;
            }
            "travel_acceleration" => self.travel_acceleration = number(key, value)?,
            "travel_short_distance_acceleration" => {
                self.travel_short_distance_acceleration = number(key, value)?
            }
            "machine_limits_usage" => {
                self.machine_limits_usage = choice(
                    key,
                    value,
                    &["emit_to_gcode", "time_estimate_only", "ignore"],
                )?
            }
            "stuck_filament_detection" => self.stuck_filament_detection = boolean(key, value)?,
            "shrinkage_xy" => self.shrinkage_xy = percent_value(key, value)?,
            "shrinkage_z" => self.shrinkage_z = percent_value(key, value)?,
            "perimeter_generator" => {
                self.perimeter_generator = choice(key, value, &["classic", "arachne"])?
            }
            "wall_transition_length" => {
                self.wall_transition_length = width_mm(key, value, self.nozzle_diameter)?
            }
            "wall_transition_angle" => self.wall_transition_angle = number(key, value)?,
            "wall_transition_filter_deviation" => {
                self.wall_transition_filter_deviation = percent_value(key, value)?
            }
            "overhangs" => self.overhangs = boolean(key, value)?,
            "chamber_minimal_temperature" => self.chamber_minimal_temperature = number(key, value)?,
            "chamber_temperature" => self.chamber_temperature = number(key, value)?,
            "infill_every_layers" => self.infill_every_layers = whole(key, value)?,
            "automatic_infill_combination" => {
                self.automatic_infill_combination = boolean(key, value)?
            }
            "infill_combination_max_layer_height" => {
                self.infill_combination_max_layer_height = number(key, value)?
            }
            "automatic_infill_combination_max_layer_height" => {
                self.infill_combination_max_layer_height = number(key, value)?
            }
            "wall_distribution_count" => self.wall_distribution_count = whole(key, value)?,
            "gap_fill_enabled" => self.gap_fill_enabled = boolean(key, value)?,
            "infill_first" => self.infill_first = boolean(key, value)?,
            "extra_perimeters" => self.extra_perimeters = boolean(key, value)?,
            "extra_perimeters_on_overhangs" => {
                self.extra_perimeters_on_overhangs = boolean(key, value)?
            }
            "arc_fitting" => self.arc_fitting = boolean(key, value)?,
            "arc_fitting_tolerance" => self.arc_fitting_tolerance = number(key, value)?,
            "ironing" => self.ironing = boolean(key, value)?,
            "ironing_all_solid" => self.ironing_all_solid = boolean(key, value)?,
            "ironing_type" => {
                let kind = value.trim().to_ascii_lowercase();
                if kind.contains("no ironing") || kind == "none" || kind.is_empty() {
                    self.ironing = false;
                    self.ironing_type = "top_surfaces".to_owned();
                } else {
                    self.ironing_type = if kind.contains("solid") {
                        "all_solid".to_owned()
                    } else if kind.contains("topmost") {
                        "topmost_only".to_owned()
                    } else {
                        "top_surfaces".to_owned()
                    };
                }
            }
            "ironing_flow" => self.ironing_flow = number(key, value)?,
            "ironing_spacing" => self.ironing_spacing = number(key, value)?,
            "ironing_speed" => self.ironing_speed = number(key, value)?,
            "bridge_angle" => self.bridge_angle = number(key, value)?,
            "thick_bridges" => self.thick_bridges = boolean(key, value)?,
            "bridge_cross_hatch" => self.bridge_cross_hatch = boolean(key, value)?,
            "bridge_fan_speed" => self.bridge_fan_speed = number(key, value)?,
            "bridge_acceleration" => self.bridge_acceleration = number(key, value)?,
            "max_bridge_length" => self.max_bridge_length = number(key, value)?,
            "bridge_span_flow" => self.bridge_span_flow = number(key, value)?,
            "bridge_span_speed" => self.bridge_span_speed = number(key, value)?,
            "bridge_span_ref" => self.bridge_span_ref = number(key, value)?,
            "emit_print_stats" => self.emit_print_stats = boolean(key, value)?,
            "exclude_object" => self.exclude_object = boolean(key, value)?,
            "pressure_advance" | "pressure_advance_value" => {
                self.set_pressure_advance(key, value)?
            }
            other => {
                return Err(ConfigError::UnknownKey {
                    line,
                    key: other.to_owned(),
                })
            }
        }
        Ok(())
    }

    /// Rejects settings the engine cannot print safely.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let positive = [
            ("bed_x", self.bed_x),
            ("bed_y", self.bed_y),
            ("max_z", self.max_z),
            ("nozzle_diameter", self.nozzle_diameter),
            ("filament_diameter", self.filament_diameter),
            ("line_width", self.line_width),
            ("min_bead_width", self.min_bead_width),
            ("layer_height", self.layer_height),
            ("first_layer_height", self.first_layer_height),
            ("min_layer_height", self.min_layer_height),
            ("first_layer_speed", self.first_layer_speed),
            ("outer_wall_speed", self.outer_wall_speed),
            ("inner_wall_speed", self.inner_wall_speed),
            ("thin_speed", self.thin_speed),
            ("infill_speed", self.infill_speed),
            ("solid_speed", self.solid_speed),
            ("support_speed", self.support_speed),
            ("support_interface_speed", self.support_interface_speed),
            ("support_branch_distance", self.support_branch_distance),
            ("support_tip_diameter", self.support_tip_diameter),
            (
                "support_max_branch_diameter",
                self.support_max_branch_diameter,
            ),
            ("travel_speed", self.travel_speed),
            ("max_speed", self.max_speed),
            ("flow", self.flow),
            ("bridge_speed", self.bridge_speed),
            ("bridge_flow", self.bridge_flow),
            ("bridge_density", self.bridge_density),
            ("bridge_span_ref", self.bridge_span_ref),
        ];
        for (key, value) in positive {
            if value <= 0.0 {
                return Err(ConfigError::OutOfRange {
                    key,
                    rule: "greater than zero",
                    value: format!("{value}"),
                });
            }
        }
        if self.walls == 0 || self.walls > 8 {
            return Err(ConfigError::OutOfRange {
                key: "walls",
                rule: "between 1 and 8",
                value: format!("{}", self.walls),
            });
        }
        if self.brim_loops > 20 {
            return Err(ConfigError::OutOfRange {
                key: "brim_loops",
                rule: "between 0 and 20",
                value: format!("{}", self.brim_loops),
            });
        }
        if self.brim_loops > 0 && !(0.0..=1.0).contains(&self.brim_gap) {
            return Err(ConfigError::OutOfRange {
                key: "brim_gap",
                rule: "between 0 and 1 mm",
                value: format!("{}", self.brim_gap),
            });
        }
        if self.elefant_foot_compensation < 0.0 || self.elefant_foot_compensation > 2.0 {
            return Err(ConfigError::OutOfRange {
                key: "elefant_foot_compensation",
                rule: "between 0 and 2 mm",
                value: format!("{}", self.elefant_foot_compensation),
            });
        }
        if !(-1.0..=1.0).contains(&self.xy_size_compensation) {
            return Err(ConfigError::OutOfRange {
                key: "xy_size_compensation",
                rule: "between -1 and 1 mm",
                value: format!("{}", self.xy_size_compensation),
            });
        }
        if !(0.0..=1.0).contains(&self.slice_closing_radius) {
            return Err(ConfigError::OutOfRange {
                key: "slice_closing_radius",
                rule: "between 0 and 1 mm",
                value: format!("{}", self.slice_closing_radius),
            });
        }
        if !(0.0..=1.0).contains(&self.resolution) {
            return Err(ConfigError::OutOfRange {
                key: "resolution",
                rule: "between 0 and 1 mm",
                value: format!("{}", self.resolution),
            });
        }
        if self.z_offset < -2.0 || self.z_offset > 2.0 {
            return Err(ConfigError::OutOfRange {
                key: "z_offset",
                rule: "between -2 and 2 mm",
                value: format!("{}", self.z_offset),
            });
        }
        if !(0.0..=1.0).contains(&self.infill_density) {
            return Err(ConfigError::OutOfRange {
                key: "infill_density",
                rule: "between 0 and 1",
                value: format!("{}", self.infill_density),
            });
        }
        for (key, value) in [
            ("support_material_spacing", self.support_spacing),
            (
                "support_material_interface_spacing",
                self.support_interface_spacing,
            ),
        ] {
            if value < 0.0 && value != -1.0 {
                return Err(ConfigError::OutOfRange {
                    key,
                    rule: "0 mm or more, or -1 to keep the density in charge",
                    value: format!("{value}"),
                });
            }
        }
        if self.min_bead_width > self.line_width {
            return Err(ConfigError::OutOfRange {
                key: "min_bead_width",
                rule: "no wider than line_width",
                value: format!("{}", self.min_bead_width),
            });
        }
        if self.layer_height > 0.8 * self.nozzle_diameter
            || self.first_layer_height > 0.8 * self.nozzle_diameter
        {
            return Err(ConfigError::OutOfRange {
                key: "layer_height",
                rule: "no more than 80% of the nozzle diameter",
                value: format!("{}", self.layer_height),
            });
        }
        if self.min_layer_height <= 0.0 || self.min_layer_height > self.max_layer() {
            return Err(ConfigError::OutOfRange {
                key: "min_layer_height",
                rule: "positive and no more than max_layer_height",
                value: format!("{}", self.min_layer_height),
            });
        }
        if self.max_layer() > 0.8 * self.nozzle_diameter {
            return Err(ConfigError::OutOfRange {
                key: "max_layer_height",
                rule: "no more than 80% of the nozzle diameter",
                value: format!("{}", self.max_layer()),
            });
        }
        for (z, height) in &self.layer_height_profile {
            if !(z.is_finite() && height.is_finite())
                || *height < self.min_layer_height
                || *height > self.max_layer()
            {
                return Err(ConfigError::OutOfRange {
                    key: "layer_height_profile",
                    rule: "heights between min_layer_height and max_layer_height",
                    value: format!("{z}:{height}"),
                });
            }
        }
        if self.line_width < 0.8 * self.nozzle_diameter
            || self.line_width > 2.0 * self.nozzle_diameter
        {
            return Err(ConfigError::OutOfRange {
                key: "line_width",
                rule: "between 0.8x and 2x the nozzle diameter",
                value: format!("{}", self.line_width),
            });
        }
        for (key, value) in [
            ("nozzle_temp", self.nozzle_temp),
            ("first_nozzle_temp", self.first_nozzle_temp),
        ] {
            if !(150.0..=320.0).contains(&value) {
                return Err(ConfigError::OutOfRange {
                    key,
                    rule: "between 150 and 320 C",
                    value: format!("{value}"),
                });
            }
        }
        if !(0.0..=130.0).contains(&self.bed_temp) {
            return Err(ConfigError::OutOfRange {
                key: "bed_temp",
                rule: "between 0 and 130 C",
                value: format!("{}", self.bed_temp),
            });
        }
        if !(0.0..=100.0).contains(&self.fan_speed) {
            return Err(ConfigError::OutOfRange {
                key: "fan_speed",
                rule: "between 0 and 100",
                value: format!("{}", self.fan_speed),
            });
        }
        if self.retract_len < 0.0 || self.retract_len > 10.0 {
            return Err(ConfigError::OutOfRange {
                key: "retract_len",
                rule: "between 0 and 10 mm",
                value: format!("{}", self.retract_len),
            });
        }
        if self.z_hop < 0.0 || self.z_hop > 5.0 {
            return Err(ConfigError::OutOfRange {
                key: "z_hop",
                rule: "between 0 and 5 mm",
                value: format!("{}", self.z_hop),
            });
        }
        if self.raft_layers > 20 {
            return Err(ConfigError::OutOfRange {
                key: "raft_layers",
                rule: "at most 20",
                value: format!("{}", self.raft_layers),
            });
        }
        if !(0.05..=1.0).contains(&self.raft_first_layer_density) {
            return Err(ConfigError::OutOfRange {
                key: "raft_first_layer_density",
                rule: "between 0.05 and 1",
                value: format!("{}", self.raft_first_layer_density),
            });
        }
        if self.brim_separation < 0.0 || self.brim_separation > 5.0 {
            return Err(ConfigError::OutOfRange {
                key: "brim_separation",
                rule: "between 0 and 5 mm",
                value: format!("{}", self.brim_separation),
            });
        }
        if self.scarf_seam_length < 0.0 || self.scarf_seam_length > 100.0 {
            return Err(ConfigError::OutOfRange {
                key: "scarf_seam_length",
                rule: "between 0 and 100 mm",
                value: format!("{}", self.scarf_seam_length),
            });
        }
        if self.seam_gap < 0.0 || self.seam_gap > 5.0 {
            return Err(ConfigError::OutOfRange {
                key: "seam_gap",
                rule: "between 0 and 5 mm",
                value: format!("{}", self.seam_gap),
            });
        }
        if !(0.01..=5.0).contains(&self.fuzzy_skin_point_dist) {
            return Err(ConfigError::OutOfRange {
                key: "fuzzy_skin_point_dist",
                rule: "between 0.01 and 5 mm",
                value: format!("{}", self.fuzzy_skin_point_dist),
            });
        }
        if self.gcode_resolution < 0.0 || self.gcode_resolution > 0.1 {
            return Err(ConfigError::OutOfRange {
                key: "gcode_resolution",
                rule: "between 0 and 0.1 mm",
                value: format!("{}", self.gcode_resolution),
            });
        }
        if self.support_enforce_layers > 100 {
            return Err(ConfigError::OutOfRange {
                key: "support_material_enforce_layers",
                rule: "at most 100",
                value: format!("{}", self.support_enforce_layers),
            });
        }
        if !(5.0..=85.0).contains(&self.support_tree_angle_slow) {
            return Err(ConfigError::OutOfRange {
                key: "support_tree_angle_slow",
                rule: "between 5 and 85 degrees",
                value: format!("{}", self.support_tree_angle_slow),
            });
        }
        if self.support_tree_top_rate <= 0.0 || self.support_tree_top_rate > 200.0 {
            return Err(ConfigError::OutOfRange {
                key: "support_tree_top_rate",
                rule: "in (0, 200] percent",
                value: format!("{}", self.support_tree_top_rate),
            });
        }
        if self.support_first_layer_density < 0.0 || self.support_first_layer_density > 1.0 {
            return Err(ConfigError::OutOfRange {
                key: "support_material_first_layer_density",
                rule: "between 0 and 1",
                value: format!("{}", self.support_first_layer_density),
            });
        }
        if self.travel_max_lift < 0.0 || self.travel_max_lift > 20.0 {
            return Err(ConfigError::OutOfRange {
                key: "travel_max_lift",
                rule: "between 0 and 20 mm",
                value: format!("{}", self.travel_max_lift),
            });
        }
        if self.brim_width < 0.0 {
            return Err(ConfigError::OutOfRange {
                key: "brim_width",
                rule: "zero or a positive width in millimetres",
                value: format!("{}", self.brim_width),
            });
        }
        if self.min_print_speed < 0.0 {
            return Err(ConfigError::OutOfRange {
                key: "min_print_speed",
                rule: "zero or a positive speed in millimetres per second",
                value: format!("{}", self.min_print_speed),
            });
        }
        if self.cooling_perimeter_transition_distance < 0.0 {
            return Err(ConfigError::OutOfRange {
                key: "cooling_perimeter_transition_distance",
                rule: "zero or a positive distance in millimetres",
                value: format!("{}", self.cooling_perimeter_transition_distance),
            });
        }
        if self.min_speed_factor <= 0.0 || self.min_speed_factor > 1.0 {
            return Err(ConfigError::OutOfRange {
                key: "min_speed_factor",
                rule: "in (0, 1]",
                value: format!("{}", self.min_speed_factor),
            });
        }
        if self.solid_infill_below_area < 0.0 {
            return Err(ConfigError::OutOfRange {
                key: "solid_infill_below_area",
                rule: "zero or a positive area in square millimetres",
                value: format!("{}", self.solid_infill_below_area),
            });
        }
        if self.wall_distribution_count == 0 {
            return Err(ConfigError::OutOfRange {
                key: "wall_distribution_count",
                rule: "at least 1",
                value: "0".to_owned(),
            });
        }
        if !(0.1..=1.5).contains(&self.bridge_flow) {
            return Err(ConfigError::OutOfRange {
                key: "bridge_flow",
                rule: "between 0.1 and 1.5",
                value: format!("{}", self.bridge_flow),
            });
        }
        if !(0.1..=1.25).contains(&self.bridge_density) {
            return Err(ConfigError::OutOfRange {
                key: "bridge_density",
                rule: "between 0.1 and 1.25",
                value: format!("{}", self.bridge_density),
            });
        }
        if self.bridge_line_width < 0.0 || self.bridge_line_width > 2.0 * self.nozzle_diameter {
            return Err(ConfigError::OutOfRange {
                key: "bridge_line_width",
                rule: "0 for line_width, or up to 2x the nozzle diameter",
                value: format!("{}", self.bridge_line_width),
            });
        }
        let widths = [
            ("wall_width", self.wall_width),
            ("outer_wall_width", self.outer_wall_width),
            ("infill_width", self.infill_width),
            ("solid_infill_width", self.solid_infill_width),
            ("first_layer_width", self.first_layer_width),
            ("top_infill_width", self.top_infill_width),
            ("support_width", self.support_width),
            ("support_interface_width", self.support_interface_width),
        ];
        for (key, width) in widths {
            if width < 0.0 || width > 3.0 * self.nozzle_diameter {
                return Err(ConfigError::OutOfRange {
                    key,
                    rule: "0 to inherit, or up to 3x the nozzle diameter",
                    value: format!("{width}"),
                });
            }
            if width > 0.0 && width < self.min_bead_width {
                return Err(ConfigError::OutOfRange {
                    key,
                    rule: "at least min_bead_width, or 0 to inherit",
                    value: format!("{width}"),
                });
            }
        }
        if !(0.001..=0.5).contains(&self.arc_fitting_tolerance) {
            return Err(ConfigError::OutOfRange {
                key: "arc_fitting_tolerance",
                rule: "between 0.001 and 0.5 mm",
                value: format!("{}", self.arc_fitting_tolerance),
            });
        }
        if !(0.0..=1.0).contains(&self.ironing_flow) {
            return Err(ConfigError::OutOfRange {
                key: "ironing_flow",
                rule: "between 0 and 1 (0 to 100 percent of normal flow)",
                value: format!("{}", self.ironing_flow),
            });
        }
        if self.ironing_spacing <= 0.0 || self.ironing_spacing > self.nozzle_diameter {
            return Err(ConfigError::OutOfRange {
                key: "ironing_spacing",
                rule: "above 0 and no wider than the nozzle",
                value: format!("{}", self.ironing_spacing),
            });
        }
        if self.ironing_speed <= 0.0 {
            return Err(ConfigError::OutOfRange {
                key: "ironing_speed",
                rule: "a positive speed",
                value: format!("{}", self.ironing_speed),
            });
        }
        if self.bridge_angle != -1.0
            && (self.bridge_angle < 0.0 || !matches!(self.bridge_angle as i64 % 45, 0))
        {
            return Err(ConfigError::OutOfRange {
                key: "bridge_angle",
                rule: "-1 to choose automatically, or a multiple of 45",
                value: format!("{}", self.bridge_angle),
            });
        }
        if !(0.0..=100.0).contains(&self.bridge_fan_speed) {
            return Err(ConfigError::OutOfRange {
                key: "bridge_fan_speed",
                rule: "between 0 and 100",
                value: format!("{}", self.bridge_fan_speed),
            });
        }
        if self.bridge_acceleration < 0.0 || self.bridge_acceleration > 20000.0 {
            return Err(ConfigError::OutOfRange {
                key: "bridge_acceleration",
                rule: "0 to leave acceleration alone, or up to 20000 mm/s^2",
                value: format!("{}", self.bridge_acceleration),
            });
        }
        if self.max_bridge_length < 0.0 {
            return Err(ConfigError::OutOfRange {
                key: "max_bridge_length",
                rule: "0 or greater",
                value: format!("{}", self.max_bridge_length),
            });
        }
        if self.bridge_span_flow < -1.0 || self.bridge_span_flow > 2.0 {
            return Err(ConfigError::OutOfRange {
                key: "bridge_span_flow",
                rule: "between -1 and 2 per mm",
                value: format!("{}", self.bridge_span_flow),
            });
        }
        if self.bridge_span_speed < 0.0 || self.bridge_span_speed > 1.0 {
            return Err(ConfigError::OutOfRange {
                key: "bridge_span_speed",
                rule: "between 0 and 1 per mm",
                value: format!("{}", self.bridge_span_speed),
            });
        }
        if self.support_branch_angle <= 0.0 || self.support_branch_angle >= 80.0 {
            return Err(ConfigError::OutOfRange {
                key: "support_branch_angle",
                rule: "between 0 and 80 degrees",
                value: format!("{}", self.support_branch_angle),
            });
        }
        if self.support_branch_diameter_angle < 0.0 || self.support_branch_diameter_angle >= 45.0 {
            return Err(ConfigError::OutOfRange {
                key: "support_branch_diameter_angle",
                rule: "between 0 and 45 degrees",
                value: format!("{}", self.support_branch_diameter_angle),
            });
        }
        if self.support_tip_diameter >= self.support_max_branch_diameter {
            return Err(ConfigError::OutOfRange {
                key: "support_tip_diameter",
                rule: "smaller than support_max_branch_diameter",
                value: format!("{}", self.support_tip_diameter),
            });
        }
        if self.pressure_advance < 0.0 || self.pressure_advance > 2.0 {
            return Err(ConfigError::OutOfRange {
                key: "pressure_advance",
                rule: "between 0 and 2",
                value: format!("{}", self.pressure_advance),
            });
        }
        let bounded: [(&str, f64, f64, &str); 46] = [
            (
                "default_accel",
                self.default_accel,
                50000.0,
                "0 to leave acceleration alone, or up to 50000 mm/s^2",
            ),
            (
                "outer_wall_accel",
                self.outer_wall_accel,
                50000.0,
                "0 to inherit the default, or up to 50000 mm/s^2",
            ),
            (
                "inner_wall_accel",
                self.inner_wall_accel,
                50000.0,
                "0 to inherit the default, or up to 50000 mm/s^2",
            ),
            (
                "initial_layer_accel",
                self.initial_layer_accel,
                50000.0,
                "0 to inherit the feature, or up to 50000 mm/s^2",
            ),
            (
                "solid_accel",
                self.solid_accel,
                50000.0,
                "0 to inherit the default, or up to 50000 mm/s^2",
            ),
            (
                "infill_accel",
                self.infill_accel,
                50000.0,
                "0 to inherit the default, or up to 50000 mm/s^2",
            ),
            (
                "support_accel",
                self.support_accel,
                50000.0,
                "0 to inherit the default, or up to 50000 mm/s^2",
            ),
            (
                "support_interface_accel",
                self.support_interface_accel,
                50000.0,
                "0 to inherit the default, or up to 50000 mm/s^2",
            ),
            (
                "machine_max_accel_x",
                self.machine_max_accel_x,
                50000.0,
                "0 to leave the machine alone, or up to 50000 mm/s^2",
            ),
            (
                "machine_max_accel_y",
                self.machine_max_accel_y,
                50000.0,
                "0 to leave the machine alone, or up to 50000 mm/s^2",
            ),
            (
                "machine_max_accel_z",
                self.machine_max_accel_z,
                50000.0,
                "0 to leave the machine alone, or up to 50000 mm/s^2",
            ),
            (
                "machine_max_accel_e",
                self.machine_max_accel_e,
                50000.0,
                "0 to leave the machine alone, or up to 50000 mm/s^2",
            ),
            (
                "machine_max_feedrate_x",
                self.machine_max_feedrate_x,
                2000.0,
                "0 to leave the machine alone, or up to 2000 mm/s",
            ),
            (
                "machine_max_feedrate_y",
                self.machine_max_feedrate_y,
                2000.0,
                "0 to leave the machine alone, or up to 2000 mm/s",
            ),
            (
                "machine_max_feedrate_z",
                self.machine_max_feedrate_z,
                2000.0,
                "0 to leave the machine alone, or up to 2000 mm/s",
            ),
            (
                "machine_max_feedrate_e",
                self.machine_max_feedrate_e,
                2000.0,
                "0 to leave the machine alone, or up to 2000 mm/s",
            ),
            (
                "machine_max_jerk_x",
                self.machine_max_jerk_x,
                100.0,
                "0 to leave the machine alone, or up to 100 mm/s",
            ),
            (
                "machine_max_jerk_y",
                self.machine_max_jerk_y,
                100.0,
                "0 to leave the machine alone, or up to 100 mm/s",
            ),
            (
                "machine_max_jerk_z",
                self.machine_max_jerk_z,
                100.0,
                "0 to leave the machine alone, or up to 100 mm/s",
            ),
            (
                "machine_max_jerk_e",
                self.machine_max_jerk_e,
                100.0,
                "0 to leave the machine alone, or up to 100 mm/s",
            ),
            (
                "min_fan_speed",
                self.min_fan_speed,
                100.0,
                "between 0 and 100 percent",
            ),
            (
                "fan_below_layer_time",
                self.fan_below_layer_time,
                600.0,
                "0 to disable, or up to 600 seconds",
            ),
            (
                "slow_down_layer_time",
                self.slow_down_layer_time,
                1e6,
                "0 to disable, or up to 1000000 seconds",
            ),
            (
                "slow_down_min_speed",
                self.slow_down_min_speed,
                self.max_speed,
                "0 to use min_speed_factor, or up to max_speed",
            ),
            (
                "idle_temp",
                self.idle_temp,
                350.0,
                "0 to leave the nozzle alone, or up to 350 C",
            ),
            (
                "deretract_speed",
                self.deretract_speed,
                200.0,
                "0 to use retract_speed, or up to 200 mm/s",
            ),
            (
                "retract_restart_extra",
                self.retract_restart_extra,
                2.0,
                "between 0 and 2 mm",
            ),
            (
                "wipe_distance",
                self.wipe_distance,
                10.0,
                "between 0 and 10 mm",
            ),
            (
                "min_layer_time",
                self.min_layer_time,
                1e6,
                "0 to disable, or up to 1000000 seconds",
            ),
            (
                "top_solid_speed",
                self.top_solid_speed,
                self.max_speed,
                "0 to use solid_speed, or up to max_speed",
            ),
            (
                "travel_speed_z",
                self.travel_speed_z,
                self.max_speed,
                "0 for 10 mm/s, or up to max_speed",
            ),
            (
                "first_layer_outer_speed",
                self.first_layer_outer_speed,
                self.max_speed,
                "0 to cap the wall at first_layer_speed, or up to max_speed",
            ),
            (
                "first_layer_inner_speed",
                self.first_layer_inner_speed,
                self.max_speed,
                "0 to cap the wall at first_layer_speed, or up to max_speed",
            ),
            (
                "first_layer_thin_speed",
                self.first_layer_thin_speed,
                self.max_speed,
                "0 to cap gap fill at first_layer_speed, or up to max_speed",
            ),
            (
                "first_layer_infill_speed",
                self.first_layer_infill_speed,
                self.max_speed,
                "0 to cap infill at first_layer_speed, or up to max_speed",
            ),
            (
                "first_layer_solid_speed",
                self.first_layer_solid_speed,
                self.max_speed,
                "0 to cap skin at first_layer_speed, or up to max_speed",
            ),
            (
                "first_layer_top_solid_speed",
                self.first_layer_top_solid_speed,
                self.max_speed,
                "0 to cap the top surface at first_layer_speed, or up to max_speed",
            ),
            (
                "first_layer_support_speed",
                self.first_layer_support_speed,
                self.max_speed,
                "0 to cap support at first_layer_speed, or up to max_speed",
            ),
            (
                "perimeter_accel",
                self.perimeter_accel,
                50000.0,
                "0 to inherit the default, or up to 50000 mm/s^2",
            ),
            (
                "top_solid_accel",
                self.top_solid_accel,
                50000.0,
                "0 to inherit solid_accel, or up to 50000 mm/s^2",
            ),
            (
                "overhang_fan_speed_0",
                self.overhang_fan_speeds[0],
                100.0,
                "between 0 and 100 percent",
            ),
            (
                "overhang_fan_speed_1",
                self.overhang_fan_speeds[1],
                100.0,
                "between 0 and 100 percent",
            ),
            (
                "overhang_fan_speed_2",
                self.overhang_fan_speeds[2],
                100.0,
                "between 0 and 100 percent",
            ),
            (
                "overhang_fan_speed_3",
                self.overhang_fan_speeds[3],
                100.0,
                "between 0 and 100 percent",
            ),
            (
                "first_layer_bed_temp",
                self.first_layer_bed_temp,
                150.0,
                "0 to use bed_temp, or up to 150 C",
            ),
            (
                "chamber_temp",
                self.chamber_temp,
                100.0,
                "0 to emit nothing, or up to 100 C",
            ),
        ];
        for (key, value, hi, rule) in bounded {
            if value < 0.0 || value > hi {
                return Err(ConfigError::OutOfRange {
                    key,
                    rule,
                    value: format!("{value}"),
                });
            }
        }
        let signed = [
            ("initial_layer_fan_speed", self.initial_layer_fan_speed),
            ("ironing_fan_speed", self.ironing_fan_speed),
            (
                "support_interface_fan_speed",
                self.support_interface_fan_speed,
            ),
        ];
        for (key, value) in signed {
            if !(-1.0..=100.0).contains(&value) {
                return Err(ConfigError::OutOfRange {
                    key,
                    rule: "-1 to follow the layer value, or 0 to 100 percent",
                    value: format!("{value}"),
                });
            }
        }
        let overhang = [
            ("overhang_speed_0", self.overhang_speeds[0]),
            ("overhang_speed_1", self.overhang_speeds[1]),
            ("overhang_speed_2", self.overhang_speeds[2]),
            ("overhang_speed_3", self.overhang_speeds[3]),
        ];
        for (key, value) in overhang {
            if !(0.0..=self.max_speed).contains(&value) {
                return Err(ConfigError::OutOfRange {
                    key,
                    rule: "0 to keep the feature speed, or up to max_speed",
                    value: format!("{value}"),
                });
            }
        }
        if self.skirt_height == 0 || self.skirt_height > 20 {
            return Err(ConfigError::OutOfRange {
                key: "skirt_height",
                rule: "between 1 and 20 layers",
                value: format!("{}", self.skirt_height),
            });
        }
        Ok(())
    }
}
/// Parses a `thumbnails` list such as `300x300/PNG, 64x64/QOI`.
///
/// An empty value asks for no thumbnails; every other entry has to name a size between 1
/// and 2048 pixels. The format is optional — `64x64` takes whatever `thumbnails_format`
/// says — and an entry that names one must name a format a viewer can decode.
fn thumbnails(value: &str) -> Result<Vec<Thumbnail>, String> {
    let mut out = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (size, format) = match entry.split_once('/') {
            Some((size, format)) => (size, Some(format)),
            None => (entry, None),
        };
        let (w, h) = size
            .split_once(['x', 'X'])
            .ok_or_else(|| format!("`{size}` is not WIDTHxHEIGHT"))?;
        let width: usize = w
            .trim()
            .parse()
            .map_err(|_| format!("`{w}` is not a width"))?;
        let height: usize = h
            .trim()
            .parse()
            .map_err(|_| format!("`{h}` is not a height"))?;
        if width == 0 || height == 0 || width > 2048 || height > 2048 {
            return Err(format!("`{size}` is outside 1..2048 pixels"));
        }
        let format = match format {
            Some(name) => Some(
                ThumbnailFormat::parse(name)
                    .ok_or_else(|| format!("`{}` is not PNG or QOI", name.trim()))?,
            ),
            None => None,
        };
        out.push(Thumbnail {
            width,
            height,
            format,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_layers_are_recorded_as_always_synchronised() {
        assert!(
            !PrinterConfig::default().support_synchronize_layers,
            "the dialect's own default is off"
        );
        let on = PrinterConfig::parse("support_material_synchronize_layers = true\n")
            .expect("it parses");
        assert!(on.support_synchronize_layers);
    }

    #[test]
    fn a_slicing_mode_is_read_by_name_and_refused_by_name() {
        for (text, mode) in [
            ("regular", SliceMode::Regular),
            ("even_odd", SliceMode::EvenOdd),
            ("close_holes", SliceMode::CloseHoles),
            ("2", SliceMode::CloseHoles),
            ("EvenOdd", SliceMode::EvenOdd),
        ] {
            let config = PrinterConfig::parse(&format!("slicing_mode = {text}\n"))
                .unwrap_or_else(|error| panic!("{text}: {error}"));
            assert_eq!(config.slicing_mode, mode, "{text}");
        }
        let error = PrinterConfig::parse("slicing_mode = spiral\n").unwrap_err();
        assert!(
            error.to_string().contains("spiral") && error.to_string().contains("close_holes"),
            "{error}"
        );
    }

    #[test]
    fn binary_gcode_is_a_switch_that_starts_off() {
        assert!(!PrinterConfig::default().binary_gcode);
        assert!(
            PrinterConfig::parse("binary_gcode = true\n")
                .expect("it parses")
                .binary_gcode
        );
        assert!(
            !PrinterConfig::parse("binary_gcode = 0\n")
                .expect("it parses")
                .binary_gcode
        );
    }

    #[test]
    fn a_thumbnail_without_a_format_takes_the_profile_format() {
        let config =
            PrinterConfig::parse("thumbnails = 64x64, 300x300/QOI\nthumbnails_format = QOI\n")
                .expect("the profile parses");
        assert_eq!(config.thumbnails.len(), 2);
        assert_eq!(config.thumbnails[0].format, None);
        assert_eq!(config.thumbnails[1].format, Some(ThumbnailFormat::Qoi));
        assert_eq!(config.thumbnails_format, ThumbnailFormat::Qoi);
        let default = PrinterConfig::parse("thumbnails = 64x64\n").expect("it parses");
        assert_eq!(default.thumbnails_format, ThumbnailFormat::Png);
    }

    #[test]
    fn a_format_this_engine_cannot_write_is_refused_by_name() {
        let error = PrinterConfig::parse("thumbnails_format = JPG\n").unwrap_err();
        assert!(
            error.to_string().contains("JPG") && error.to_string().contains("PNG, QOI"),
            "{error}"
        );
        let error = PrinterConfig::parse("thumbnails = 64x64/BMP\n").unwrap_err();
        assert!(error.to_string().contains("BMP"), "{error}");
    }

    #[test]
    fn resin_is_refused_and_a_bed_texture_is_recorded() {
        let error = PrinterConfig::parse("printer_technology = SLA\n").unwrap_err();
        assert!(
            error.to_string().contains("SLA") && error.to_string().contains("FFF"),
            "{error}"
        );
        let config =
            PrinterConfig::parse("printer_technology = FFF\nbed_custom_texture = /tmp/bed.png\n")
                .expect("FFF parses");
        assert_eq!(config.bed_custom_texture, "/tmp/bed.png");
        assert!(PrinterConfig::parse("printer_technology = FDM\n").is_ok());
    }

    #[test]
    fn a_settings_document_loads_as_a_profile() {
        let path =
            std::env::temp_dir().join(format!("hfw-config-document-{}.json", std::process::id()));
        std::fs::write(
            &path,
            "{\"configuration\":{\"print_settings\":{\"layer_height\":0.3,\"perimeters\":3},\"printer_settings\":{\"nozzle_diameter\":0.4}}}",
        )
        .expect("the settings document is written");
        let config = PrinterConfig::load(&path).expect("a settings document loads as a profile");
        let _ = std::fs::remove_file(&path);
        assert!((config.layer_height - 0.3).abs() < 1e-9);
        assert_eq!(config.walls, 3);
    }

    #[test]
    fn a_profile_written_in_the_engine_dialect_parses() {
        let config = PrinterConfig::parse(
            "name = engine-dialect\n\
             perimeters = 4\n\
             fill_density = 15%\n\
             nozzle_temperature = 205\n",
        )
        .expect("engine names parse");
        assert_eq!(config.walls, 4);
        assert!((config.infill_density - 0.15).abs() < 1e-9);
        assert!((config.nozzle_temp - 205.0).abs() < 1e-9);
    }

    #[test]
    fn a_profile_that_says_no_brim_gets_no_brim_loops() {
        let off = PrinterConfig::parse("brim_type = no_brim\nbrim_width = 5\nline_width = 0.4\n")
            .expect("brim settings parse");
        assert_eq!(off.adhesion_brim_loops(), 0);

        let shaped =
            PrinterConfig::parse("brim_type = outer_only\nbrim_width = 5\nline_width = 0.4\n")
                .expect("brim settings parse");
        assert_eq!(shaped.adhesion_brim_loops(), 13);

        let counted = PrinterConfig::parse("brim_loops = 2\nline_width = 0.4\n")
            .expect("brim settings parse");
        assert_eq!(counted.adhesion_brim_loops(), 2);
    }

    #[test]
    fn a_layer_height_profile_is_read_as_z_height_steps() {
        let cfg = PrinterConfig::parse(
            "layer_height_profile = 0:0.2, 8:0.12; 16:0.28\nmin_layer_height = 0.1\nmax_layer_height = 0.3, 0.3\n",
        )
        .expect("the profile parses");
        assert_eq!(
            cfg.layer_height_profile,
            vec![(0.0, 0.2), (8.0, 0.12), (16.0, 0.28)]
        );
        assert!((cfg.min_layer_height - 0.1).abs() < 1e-9);
        assert!((cfg.max_layer_height - 0.3).abs() < 1e-9);
        assert!((cfg.max_layer() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn a_profile_height_outside_the_bounds_or_unreadable_is_refused() {
        assert!(
            PrinterConfig::parse("min_layer_height = 0.1\nlayer_height_profile = 0:0.05\n")
                .is_err(),
            "a profile below min_layer_height was accepted"
        );
        assert!(
            PrinterConfig::parse("layer_height_profile = 0-0.2\n").is_err(),
            "a step without a colon was accepted"
        );
        assert!(
            PrinterConfig::parse("min_layer_height = 0.4\nmax_layer_height = 0.2\n").is_err(),
            "a floor above the ceiling was accepted"
        );
        let bare = PrinterConfig::parse("").expect("an empty profile parses");
        assert!((bare.max_layer() - 0.8 * bare.nozzle_diameter).abs() < 1e-9);
    }

    #[test]
    fn native_names_are_not_aliased_away() {
        assert_eq!(crate::alias_table::native_for("walls"), None);
        assert_eq!(crate::alias_table::native_for("perimeters"), Some("walls"));
        assert_eq!(crate::alias_table::native_for("wall_loops"), Some("walls"));
    }
    #[test]
    fn a_mapping_that_changes_meaning_is_not_an_alias() {
        assert_eq!(
            crate::alias_table::native_for("support_material_spacing"),
            None
        );
        assert_eq!(crate::alias_table::native_for("seam_position"), None);
        assert_eq!(
            crate::alias_table::native_for("external_perimeters_first"),
            None
        );
    }

    #[test]
    fn the_surface_and_first_layer_spellings_reach_their_own_keys() {
        let config = PrinterConfig::parse(
            "top_solid_infill_speed = 25\n\
             top_solid_infill_acceleration = 2000\n\
             first_layer_external_perimeter_speed = 15\n\
             first_layer_infill_speed = 25\n\
             gap_fill_speed = 20\n\
             travel_speed_z = 15\n\
             external_perimeter_acceleration = 3000\n\
             perimeter_acceleration = 2500\n\
             solid_infill_acceleration = 1800\n\
             first_layer_acceleration = 400\n",
        )
        .expect("engine names parse");
        assert!((config.top_solid_speed - 25.0).abs() < 1e-9);
        assert!((config.top_solid_accel - 2000.0).abs() < 1e-9);
        assert!((config.first_layer_outer_speed - 15.0).abs() < 1e-9);
        assert!((config.first_layer_infill_speed - 25.0).abs() < 1e-9);
        assert!((config.thin_speed - 20.0).abs() < 1e-9);
        assert!((config.travel_speed_z - 15.0).abs() < 1e-9);
        assert!((config.outer_wall_accel - 3000.0).abs() < 1e-9);
        assert!((config.perimeter_accel - 2500.0).abs() < 1e-9);
        assert!((config.solid_accel - 1800.0).abs() < 1e-9);
        assert!((config.initial_layer_accel - 400.0).abs() < 1e-9);
    }

    #[test]
    fn the_overhang_and_fan_spellings_reach_their_own_keys() {
        let config = PrinterConfig::parse(
            "enable_dynamic_overhang_speeds = true\n\
             overhang_speed_0 = 30\n\
             overhang_speed_3 = 6\n\
             enable_dynamic_fan_speeds = true\n\
             overhang_fan_speed_0 = 40\n\
             overhang_fan_speed_3 = 100\n\
             initial_layer_fan_speed = 0\n\
             ironing_fan_speed = 10\n\
             support_material_interface_fan_speed = 50\n\
             fan_always_on = true\n\
             first_layer_bed_temperature = 65\n\
             chamber_temperature = 45\n",
        )
        .expect("engine names parse");
        assert!(config.dynamic_overhang_speeds);
        assert!((config.overhang_speeds[0] - 30.0).abs() < 1e-9);
        assert!((config.overhang_speeds[3] - 6.0).abs() < 1e-9);
        assert!(config.dynamic_fan_speeds);
        assert!((config.overhang_fan_speeds[0] - 40.0).abs() < 1e-9);
        assert!((config.overhang_fan_speeds[3] - 100.0).abs() < 1e-9);
        assert!((config.initial_layer_fan_speed - 0.0).abs() < 1e-9);
        assert!((config.ironing_fan_speed - 10.0).abs() < 1e-9);
        assert!((config.support_interface_fan_speed - 50.0).abs() < 1e-9);
        assert!(config.fan_always_on);
        assert!((config.first_layer_bed_temp - 65.0).abs() < 1e-9);
        assert!((config.chamber_temp - 45.0).abs() < 1e-9);
    }

    #[test]
    fn a_fan_duty_below_minus_one_is_refused() {
        let error = PrinterConfig::parse("ironing_fan_speed = -2\n")
            .expect_err("only -1 means 'follow the layer'")
            .to_string();
        assert!(error.contains("ironing_fan_speed"), "{error}");
    }

    #[test]
    fn the_seam_adhesion_and_hop_spellings_reach_their_keys() {
        let config = PrinterConfig::parse(
            "brim_type = outer_and_inner\n\
             brim_separation = 0.3\n\
             min_skirt_length = 150\n\
             raft_layers = 2\n\
             raft_first_layer_density = 0.8\n\
             raft_first_layer_expansion = 3\n\
             raft_contact_distance = 0.15\n\
             draft_shield = limited\n\
             draft_shield_height = 5\n\
             travel_slope = 2\n\
             travel_ramping_lift = true\n\
             travel_max_lift = 1.2\n\
             retract_lift_above = 30\n\
             retract_lift_below = 2\n\
             seam_gap_distance = 0.4\n\
             staggered_inner_seams = true\n\
             scarf_seam_length = 6\n\
             scarf_seam_max_segment_length = 0.6\n\
             scarf_seam_on_inner_perimeters = true\n\
             scarf_seam_start_height = 1.5\n\
             fuzzy_skin = external\n\
             fuzzy_skin_thickness = 0.25\n\
             fuzzy_skin_point_dist = 0.6\n\
             support_material_pattern = grid\n\
             support_material_first_layer_density = 0.4\n",
        )
        .expect("dialect names parse");
        assert_eq!(config.brim_type, "outer_and_inner");
        assert!((config.brim_separation - 0.3).abs() < 1e-9);
        assert!((config.min_skirt_length - 150.0).abs() < 1e-9);
        assert_eq!(config.raft_layers, 2);
        assert!((config.raft_contact_distance - 0.15).abs() < 1e-9);
        assert_eq!(config.draft_shield, "limited");
        assert!((config.travel_slope - 2.0).abs() < 1e-9);
        assert!(config.travel_ramping_lift);
        assert!((config.seam_gap - 0.4).abs() < 1e-9);
        assert!(config.staggered_inner_seams);
        assert!((config.scarf_seam_length - 6.0).abs() < 1e-9);
        assert!(config.scarf_seam_inner);
        assert!((config.scarf_seam_start_height - 1.5).abs() < 1e-9);
        assert_eq!(config.fuzzy_skin, "external");
        assert!((config.fuzzy_skin_point_dist - 0.6).abs() < 1e-9);
        assert_eq!(config.support_pattern, "grid");
        assert!((config.support_first_layer_density - 0.4).abs() < 1e-9);
    }

    #[test]
    fn the_closing_radius_and_resolution_parse_and_hold_their_bounds() {
        let config = PrinterConfig::parse("slice_closing_radius = 0.2\nresolution = 0.05\n")
            .expect("the slice repair pair parses");
        assert!((config.slice_closing_radius - 0.2).abs() < 1e-9);
        assert!((config.resolution - 0.05).abs() < 1e-9);
        let bare = PrinterConfig::parse("").expect("an empty profile parses");
        assert!(
            (bare.slice_closing_radius - 0.049).abs() < 1e-9,
            "the default must be upstream's 0.049, got {}",
            bare.slice_closing_radius
        );
        assert!(
            (bare.resolution - 0.012).abs() < 1e-9,
            "resolution defaults to upstream's 0.012, got {}",
            bare.resolution
        );
        PrinterConfig::parse("resolution = -1\n").expect_err("a negative tolerance is refused");
        PrinterConfig::parse("slice_closing_radius = 4\n")
            .expect_err("a closing radius wider than a wall is refused");
    }

    #[test]
    fn the_travel_avoidance_switch_and_its_cap_parse() {
        let config = PrinterConfig::parse(
            "avoid_crossing_perimeters = true\navoid_crossing_perimeters_max_detour = 5.5\n",
        )
        .expect("the avoidance pair parses");
        assert!(config.avoid_crossing_perimeters);
        assert!((config.avoid_crossing_perimeters_max_detour - 5.5).abs() < 1e-9);
        let bare = PrinterConfig::parse("").expect("an empty profile parses");
        assert!(
            bare.avoid_crossing_perimeters,
            "travel keeps clear of printed walls by default: a travel across a wall \
             leaves a blob on it, and both references avoid it"
        );
        assert!(
            (bare.avoid_crossing_perimeters_max_detour - 2.0).abs() < 1e-9,
            "the detour cap is the reference two millimetres, got {}",
            bare.avoid_crossing_perimeters_max_detour
        );
        let off =
            PrinterConfig::parse("avoid_crossing_perimeters = false\n").expect("the switch parses");
        assert!(
            !off.avoid_crossing_perimeters,
            "and a profile may turn it off"
        );
    }

    #[test]
    fn an_anchor_percentage_is_resolved_over_the_infill_width() {
        let config = PrinterConfig::parse(
            "infill_anchor = 50%\ninfill_width = 0.6\nmin_feature_size = 50%\nnozzle_diameter = 0.8\nline_width = 0.7\n",
        )
        .expect("the anchor parses");
        assert!(
            (config.infill_anchor - 0.3).abs() < 1e-9,
            "50 percent of a 0.6 mm infill width is 0.3 mm, got {}",
            config.infill_anchor
        );
        assert!(
            (config.min_feature_size - 0.4).abs() < 1e-9,
            "50 percent of an 0.8 mm nozzle is 0.4 mm, got {}",
            config.min_feature_size
        );
        assert!((config.infill_anchor_max - 50.0).abs() < 1e-9);
    }

    #[test]
    fn an_unset_anchor_stays_negative_so_zero_can_mean_off() {
        let config = PrinterConfig::parse("infill_anchor = 0\n").expect("zero parses");
        assert!(config.infill_anchor.abs() < 1e-9);
        let bare = PrinterConfig::parse("").expect("an empty profile parses");
        assert!(bare.infill_anchor < 0.0, "unset must not look like zero");
        assert!(bare.min_feature_size < 0.0);
    }

    #[test]
    fn a_print_sequence_names_the_order_it_accepts() {
        let by_object =
            PrinterConfig::parse("print_sequence = by object\n").expect("by object parses");
        assert_eq!(by_object.print_sequence, crate::PrintSequence::ByObject);
        let numeric = PrinterConfig::parse("print_sequence = 0\n").expect("0 parses");
        assert_eq!(numeric.print_sequence, crate::PrintSequence::ByLayer);
        let bare = PrinterConfig::parse("").expect("an empty profile parses");
        assert_eq!(bare.print_sequence, crate::PrintSequence::ByLayer);
        assert!(
            bare.gantry_height.abs() < 1e-9,
            "an unmeasured gantry height must stay zero, not become a guess"
        );
        let error = PrinterConfig::parse("print_sequence = sometimes\n")
            .expect_err("that is not an order")
            .to_string();
        assert!(error.contains("by layer"), "{error}");
    }

    #[test]
    fn a_choice_key_names_what_it_accepts() {
        let error = PrinterConfig::parse("brim_type = sideways\n")
            .expect_err("that is not a brim side")
            .to_string();
        assert!(error.contains("outer_only"), "{error}");
        let error = PrinterConfig::parse("fuzzy_skin = everywhere\n")
            .expect_err("that is not a fuzzy mode")
            .to_string();
        assert!(error.contains("external"), "{error}");
    }

    #[test]
    fn an_alias_value_is_validated_like_the_native_one() {
        let error = PrinterConfig::parse("perimeters = many\n")
            .expect_err("a bad count is still a bad count")
            .to_string();
        assert!(error.contains("perimeters"), "{error}");
    }

    #[test]
    fn a_percentage_width_is_resolved_against_the_final_nozzle() {
        let config = PrinterConfig::parse(
            "perimeter_extrusion_width = 120%\n\
             line_width = 0.6\n\
             nozzle_diameter = 0.6\n\
             external_perimeter_extrusion_width = 0.45\n",
        )
        .expect("percentage widths parse");
        assert!(
            (config.wall_width - 0.72).abs() < 1e-9,
            "{}",
            config.wall_width
        );
        assert!((config.outer_wall_width - 0.45).abs() < 1e-9);
    }

    #[test]
    fn a_narrower_width_than_the_bead_floor_is_refused() {
        let error = PrinterConfig::parse(
            "min_bead_width = 0.3\n\
             infill_extrusion_width = 0.2\n",
        )
        .expect_err("a bead under the floor is a bead the nozzle cannot lay")
        .to_string();
        assert!(error.contains("infill_width"), "{error}");
    }

    #[test]
    fn a_machine_limit_stored_per_axis_reads_its_first_entry() {
        let config = PrinterConfig::parse(
            "machine_max_acceleration_x = 5000,5000\n\
             machine_max_acceleration_extruding = 8000,8000\n\
             machine_max_jerk_y = 12,12\n",
        )
        .expect("a comma list is how the dialect writes a per-axis limit");
        assert!((config.machine_max_accel_x - 5000.0).abs() < 1e-9);
        assert!((config.machine_max_accel_extruding - 8000.0).abs() < 1e-9);
        assert!((config.machine_max_jerk_y - 12.0).abs() < 1e-9);
    }

    #[test]
    fn a_z_offset_outside_two_millimetres_is_refused() {
        let error = PrinterConfig::parse("z_offset = -3\n")
            .expect_err("an offset that large is a machine fault, not a setting")
            .to_string();
        assert!(error.contains("z_offset"), "{error}");
    }

    #[test]
    fn identity_settings_are_read_and_written_into_the_header() {
        let config = PrinterConfig::parse(
            "printer_model = Voron 2.4\n\
             printer_vendor = LDO\n\
             filament_type = ASA\n\
             filament_cost = 32.5\n\
             filament_abrasive = 1\n\
             notes = bracket v3\n\
             silent_mode = true\n\
             default_material = ASA\n",
        )
        .expect("identity settings parse");
        assert_eq!(config.printer_model, "Voron 2.4");
        assert!((config.filament_cost - 32.5).abs() < 1e-9);
        assert!(config.filament_abrasive);
        assert!(config.silent_mode);
    }

    #[test]
    fn a_substitution_list_is_read_in_groups_of_four() {
        let parsed =
            PrinterConfig::parse("gcode_substitutions = \"M104 S0\",\"M104 S5\",i,\"warm\"\n")
                .expect("a four-entry group parses");
        assert_eq!(parsed.gcode_substitutions.len(), 1);
        assert_eq!(parsed.gcode_substitutions[0].pattern, "M104 S0");
        assert_eq!(parsed.gcode_substitutions[0].format, "M104 S5");
        assert_eq!(parsed.gcode_substitutions[0].flags, "i");
        assert_eq!(parsed.gcode_substitutions[0].note, "warm");
    }

    #[test]
    fn a_substitution_group_that_is_not_four_and_a_bad_flag_are_refused() {
        let short = PrinterConfig::parse("gcode_substitutions = a,b,c\n")
            .expect_err("three entries is not a group");
        assert!(short.to_string().contains("groups of four"), "{short}");
        let flag =
            PrinterConfig::parse("gcode_substitutions = a,b,z,\n").expect_err("z is not a flag");
        assert!(flag.to_string().contains("flag letters"), "{flag}");
    }

    #[test]
    fn an_infill_overlap_percentage_reads_as_a_fraction() {
        let percent = PrinterConfig::parse("infill_overlap = 20%\n").expect("percent parses");
        assert!((percent.infill_overlap - 0.2).abs() < 1e-9);
        let plain = PrinterConfig::parse("infill_overlap = 0.25\n").expect("plain parses");
        assert!((plain.infill_overlap - 0.25).abs() < 1e-9);
    }

    #[test]
    fn a_quoted_comma_list_keeps_its_entries_whole() {
        // A preset writes each entry quoted, so the value starts and ends with a
        // quote without being one quoted string.
        let preset = crate::dialect::Preset::parse(
            "gcode_substitutions = \"M104 S0\",\"M104 S5\",,\"warm\"\n",
        )
        .expect("the preset parses");
        let (config, report) = PrinterConfig::from_preset(&preset);
        assert_eq!(config.gcode_substitutions.len(), 1, "{report:?}");
    }

    #[test]
    fn the_new_widths_resolve_against_the_nozzle() {
        let config = PrinterConfig::parse(
            "nozzle_diameter = 0.4\n\
             first_layer_extrusion_width = 150%\n\
             top_infill_extrusion_width = 0.35\n",
        )
        .expect("widths parse");
        assert!(
            (config.first_layer_width - 0.6).abs() < 1e-9,
            "{}",
            config.first_layer_width
        );
        assert!((config.top_infill_width - 0.35).abs() < 1e-9);
    }

    #[test]
    fn automatic_widths_fill_every_unset_one_from_the_nozzle() {
        let plain = PrinterConfig::parse(
            "nozzle_diameter = 0.4\nwall_width = 0.5\nfirst_layer_extrusion_width = 0\n",
        )
        .expect("plain widths parse");
        assert_eq!(plain.wall_width, 0.5, "an explicit width is left alone");
        assert_eq!(
            plain.first_layer_width, 0.0,
            "an unset first layer stays unset"
        );

        let auto = PrinterConfig::parse(
            "nozzle_diameter = 0.4\n\
             automatic_extrusion_widths = true\n\
             wall_width = 0.5\n\
             first_layer_extrusion_width = 0\n",
        )
        .expect("automatic widths parse");
        assert_eq!(auto.wall_width, 0.5);
        assert!((auto.first_layer_width - 0.45).abs() < 1e-9);
        assert!((auto.infill_width - 0.45).abs() < 1e-9);
        assert!((auto.solid_infill_width - 0.45).abs() < 1e-9);
    }

    #[test]
    fn the_skin_and_infill_structure_settings_parse() {
        let config = PrinterConfig::parse(
            "top_fill_pattern = concentric\n\
             bottom_fill_pattern = aligned_rectilinear\n\
             solid_infill_every_layers = 4\n\
             solid_infill_below_area = 12.5\n\
             wall_distribution_count = 3\n\
             gap_fill_enabled = false\n\
             infill_first = true\n\
             extra_perimeters = false\n",
        )
        .expect("the structure settings parse");
        assert_eq!(config.top_fill_pattern, "concentric");
        assert_eq!(config.bottom_fill_pattern, "alignedrectilinear");
        assert_eq!(config.solid_infill_every_layers, 4);
        assert!((config.solid_infill_below_area - 12.5).abs() < 1e-9);
        assert_eq!(config.wall_distribution_count, 3);
        assert!(!config.gap_fill_enabled);
        assert!(config.infill_first);
        assert!(!config.extra_perimeters);
    }

    #[test]
    fn every_skin_pattern_the_dialect_lists_is_read() {
        for name in [
            "rectilinear",
            "monotonic",
            "monotoniclines",
            "alignedrectilinear",
            "concentric",
            "hilbertcurve",
            "archimedeanchords",
            "octagramspiral",
        ] {
            let config = PrinterConfig::parse(&format!("top_fill_pattern = {name}\n"))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(config.top_fill_pattern, name);
        }
    }

    #[test]
    fn a_skin_pattern_the_dialect_does_not_list_is_refused_by_name() {
        let unknown = PrinterConfig::parse("bottom_fill_pattern = spiral\n")
            .expect_err("an unknown pattern is refused");
        assert!(
            unknown.to_string().contains("bottom_fill_pattern"),
            "{unknown}"
        );
        let zero = PrinterConfig::parse("wall_distribution_count = 0\n")
            .expect_err("a distribution of zero divides nothing");
        assert!(
            zero.to_string().contains("wall_distribution_count"),
            "{zero}"
        );
    }

    #[test]
    fn the_machine_and_filament_limits_parse() {
        let config = PrinterConfig::parse(
            "filament_max_volumetric_speed = 8\n\
             filament_infill_max_speed = 45\n\
             filament_infill_max_crossing_speed = 30\n\
             over_bridge_speed = 20\n\
             internal_bridge_fan_speed = 60\n\
             enable_overhang_bridge_fan = false\n\
             retract_before_wipe = 0\n\
             only_retract_when_crossing_perimeters = true\n\
             travel_acceleration = 8000\n\
             travel_short_distance_acceleration = 4000\n\
             machine_limits_usage = ignore\n\
             stuck_filament_detection = true\n\
             chamber_minimal_temperature = 40\n",
        )
        .expect("the limits parse");
        assert_eq!(config.filament_max_volumetric_speed, 8.0);
        assert_eq!(config.filament_infill_max_speed, 45.0);
        assert_eq!(config.filament_infill_max_crossing_speed, 30.0);
        assert_eq!(config.over_bridge_speed, 20.0);
        assert_eq!(config.internal_bridge_fan_speed, 60.0);
        assert!(!config.enable_overhang_bridge_fan);
        assert_eq!(config.retract_before_wipe, 0.0);
        assert!(config.only_retract_when_crossing_perimeters);
        assert_eq!(config.travel_acceleration, 8000.0);
        assert_eq!(config.travel_short_distance_acceleration, 4000.0);
        assert_eq!(config.machine_limits_usage, "ignore");
        assert!(config.stuck_filament_detection);
        assert_eq!(config.chamber_minimal_temperature, 40.0);
    }

    #[test]
    fn the_raft_and_perimeter_structure_settings_parse() {
        let config = PrinterConfig::parse(
            "raft_expansion = 2.5\n\
             first_layer_speed_over_raft = 15\n\
             first_layer_acceleration_over_raft = 500\n\
             only_one_perimeter_first_layer = true\n\
             top_one_perimeter_type = topmost\n\
             perimeter_generator = classic\n\
             wall_transition_length = 0.6\n\
             wall_transition_angle = 12\n\
             wall_transition_filter_deviation = 20%\n\
             overhangs = false\n",
        )
        .expect("the structure settings parse");
        assert_eq!(config.raft_expansion, 2.5);
        assert_eq!(config.first_layer_speed_over_raft, 15.0);
        assert_eq!(config.first_layer_acceleration_over_raft, 500.0);
        assert!(config.only_one_perimeter_first_layer);
        assert_eq!(config.top_one_perimeter_type, "topmost");
        assert_eq!(config.perimeter_generator, "classic");
        assert_eq!(config.wall_transition_length, 0.6);
        assert_eq!(config.wall_transition_angle, 12.0);
        assert!((config.wall_transition_filter_deviation - 20.0).abs() < 1e-9);
        assert!(!config.overhangs);
    }

    #[test]
    fn shrinkage_is_read_as_a_percentage_and_a_word_outside_its_list_is_refused() {
        let config = PrinterConfig::parse("shrinkage_xy = 0.6\nshrinkage_z = 0.2\n")
            .expect("shrinkage parses");
        assert!((config.shrinkage_xy - 0.6).abs() < 1e-9);
        assert!((config.shrinkage_z - 0.2).abs() < 1e-9);
        assert!(PrinterConfig::parse("top_one_perimeter_type = all\n").is_err());
        assert!(PrinterConfig::parse("machine_limits_usage = sometimes\n").is_err());
        assert!(PrinterConfig::parse("perimeter_generator = magical\n").is_err());
    }

    #[test]
    fn pressure_advance_arrives_as_a_number_or_as_a_switch_beside_it() {
        let numeric = PrinterConfig::parse("pressure_advance = 0.04\n").expect("a number parses");
        assert!((numeric.pressure_advance - 0.04).abs() < 1e-9);

        let orca_shape =
            PrinterConfig::parse("pressure_advance = enabled\npressure_advance_value = 0.05\n")
                .expect("the reference slicers' split shape parses");
        assert!((orca_shape.pressure_advance - 0.05).abs() < 1e-9);

        let value_first =
            PrinterConfig::parse("pressure_advance_value = 0.05\npressure_advance = enabled\n")
                .expect("key order does not matter");
        assert!((value_first.pressure_advance - 0.05).abs() < 1e-9);

        let off = PrinterConfig::parse("pressure_advance = disabled\n").expect("a switch parses");
        assert!(off.pressure_advance.abs() < 1e-9);

        assert!(PrinterConfig::parse("pressure_advance = sometimes\n").is_err());
    }

    #[test]
    fn fill_angle_defaults_to_the_dialect_cross_hatch() {
        let config = PrinterConfig::parse("layer_height = 0.2\n").expect("a bare profile parses");
        assert!((config.fill_angle - 45.0).abs() < 1e-9);
        let turned = PrinterConfig::parse("fill_angle = 20\n").expect("any angle parses");
        assert!((turned.fill_angle - 20.0).abs() < 1e-9);
    }
}
