//! `key = value` profile parsing and validation.

use std::collections::HashMap;
use std::path::Path;

use crate::error::ConfigError;
use crate::{Flavor, PrinterConfig, Seam};

fn number(key: &str, value: &str) -> Result<f64, ConfigError> {
    let text = value.trim();
    match text.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(ConfigError::NotANumber {
            key: key.to_owned(),
            value: text.to_owned(),
        }),
    }
}

fn boolean(key: &str, value: &str) -> Result<bool, ConfigError> {
    match value.trim() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(ConfigError::NotABoolean {
            key: key.to_owned(),
            value: value.trim().to_owned(),
        }),
    }
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

impl PrinterConfig {
    /// Reads and validates a profile from disk.
    pub fn load(path: &Path) -> Result<PrinterConfig, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        PrinterConfig::parse(&text)
    }

    /// Parses and validates a profile from text.
    pub fn parse(text: &str) -> Result<PrinterConfig, ConfigError> {
        let mut c = PrinterConfig::default();
        let mut blocks: HashMap<String, String> = HashMap::new();
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
        c.validate()?;
        Ok(c)
    }

    fn assign(&mut self, line: usize, key: &str, value: &str) -> Result<(), ConfigError> {
        match key {
            "name" => self.name = value.to_owned(),
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
            "min_bead_width" => self.min_bead_width = number(key, value)?,
            "layer_height" => self.layer_height = number(key, value)?,
            "first_layer_height" => self.first_layer_height = number(key, value)?,
            "walls" => self.walls = whole(key, value)?,
            "infill_density" => self.infill_density = number(key, value)?,
            "infill_pattern" => self.infill_pattern = value.to_owned(),
            "top_layers" => self.top_layers = whole(key, value)?,
            "bottom_layers" => self.bottom_layers = whole(key, value)?,
            "nozzle_temp" => self.nozzle_temp = number(key, value)?,
            "first_nozzle_temp" => self.first_nozzle_temp = number(key, value)?,
            "bed_temp" => self.bed_temp = number(key, value)?,
            "fan_speed" => self.fan_speed = number(key, value)?,
            "fan_start_layer" => self.fan_start_layer = whole(key, value)?,
            "retract_len" => self.retract_len = number(key, value)?,
            "retract_speed" => self.retract_speed = number(key, value)?,
            "retract_min_travel" => self.retract_min_travel = number(key, value)?,
            "infill_retract_min_travel" => self.infill_retract_min_travel = number(key, value)?,
            "z_hop" => self.z_hop = number(key, value)?,
            "travel_speed" => self.travel_speed = number(key, value)?,
            "first_layer_speed" => self.first_layer_speed = number(key, value)?,
            "outer_wall_speed" => self.outer_wall_speed = number(key, value)?,
            "inner_wall_speed" => self.inner_wall_speed = number(key, value)?,
            "thin_speed" => self.thin_speed = number(key, value)?,
            "infill_speed" => self.infill_speed = number(key, value)?,
            "solid_speed" => self.solid_speed = number(key, value)?,
            "support_speed" => self.support_speed = number(key, value)?,
            "support_interface_speed" => self.support_interface_speed = number(key, value)?,
            "support" => self.support = boolean(key, value)?,
            "support_threshold_angle" => self.support_threshold_angle = number(key, value)?,
            "support_xy_gap" => self.support_xy_gap = number(key, value)?,
            "support_z_gap" => self.support_z_gap = number(key, value)?,
            "support_interface_layers" => self.support_interface_layers = whole(key, value)?,
            "support_density" => self.support_density = number(key, value)?,
            "support_interface_density" => self.support_interface_density = number(key, value)?,
            "support_on_build_plate_only" => {
                self.support_on_build_plate_only = boolean(key, value)?
            }
            "support_min_area" => self.support_min_area = number(key, value)?,
            "support_style" => {
                self.support_style = match value {
                    "normal" | "tree" => value.to_owned(),
                    _ => {
                        return Err(ConfigError::BadChoice {
                            key: "support_style",
                            value: value.to_owned(),
                            allowed: "normal, tree",
                        })
                    }
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
            "max_speed" => self.max_speed = number(key, value)?,
            "min_layer_time" => self.min_layer_time = number(key, value)?,
            "min_speed_factor" => self.min_speed_factor = number(key, value)?,
            "flow" => self.flow = number(key, value)?,
            "seam" => {
                self.seam = match value {
                    "aligned" => Seam::Aligned,
                    "nearest" => Seam::Nearest,
                    _ => {
                        return Err(ConfigError::BadChoice {
                            key: "seam",
                            value: value.to_owned(),
                            allowed: "aligned, nearest",
                        })
                    }
                }
            }
            "inner_walls_first" => self.inner_walls_first = boolean(key, value)?,
            "skirt_loops" => self.skirt_loops = whole(key, value)?,
            "skirt_distance" => self.skirt_distance = number(key, value)?,
            "brim_loops" => self.brim_loops = whole(key, value)?,
            "brim_gap" => self.brim_gap = number(key, value)?,
            "bridge_speed" => self.bridge_speed = number(key, value)?,
            "bridge_flow" => self.bridge_flow = number(key, value)?,
            "bridge_density" => self.bridge_density = number(key, value)?,
            "bridge_line_width" => self.bridge_line_width = number(key, value)?,
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
            "pressure_advance" => self.pressure_advance = number(key, value)?,
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
            ("max_volumetric_speed", self.max_volumetric_speed),
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
        if !(0.0..=1.0).contains(&self.infill_density) {
            return Err(ConfigError::OutOfRange {
                key: "infill_density",
                rule: "between 0 and 1",
                value: format!("{}", self.infill_density),
            });
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
        if self.min_speed_factor <= 0.0 || self.min_speed_factor > 1.0 {
            return Err(ConfigError::OutOfRange {
                key: "min_speed_factor",
                rule: "in (0, 1]",
                value: format!("{}", self.min_speed_factor),
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
        Ok(())
    }
}
