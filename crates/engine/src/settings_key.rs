//! Per-object settings, one key at a time.
//!
//! A preset states a whole profile and [`Settings::apply_printer`] reads all of
//! it. A settings panel states one value for one object, and a project file
//! carries per-object overrides as individual keys; both need the same
//! validation, the same names and the same meaning as the profile path, or a
//! setting would mean one thing job-wide and another on one object.
//!
//! [`Settings::apply_key`] is that entry point. It accepts a key in either
//! spelling a caller may have — this engine's own name for it, or the dialect
//! name an engine under a different brand uses — and refuses anything it cannot
//! apply, naming the key and what it wanted instead, so a panel can show the
//! reason rather than silently printing something else.
//!
//! Order does not matter to a caller: a support parameter turns support on by
//! itself, and `support = 0` drops it, so a panel may send its keys in any order
//! without one of them seen as a mistake.
//!
//! [`PER_OBJECT_KEYS`] is the published list: the settings a panel may offer per
//! object, which is the print-level subset. Filament and machine keys stay on
//! the printer and on the plate, because they are properties of the machine, not
//! of the part.

use hfw_infill::{IroningTarget, SkinPattern, VerticalShell};

use crate::{infill_pattern, skin_pattern, Mode, Settings};

/// Why a per-object setting was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// No setting of this name can be set on an object.
    Unknown(String),
    /// The key exists, but the value is not one it accepts.
    Value {
        /// The key, as it was given.
        key: String,
        /// The value, as it was given.
        value: String,
        /// What the key does accept, in the words a panel can show.
        expected: &'static str,
    },
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Unknown(key) => write!(formatter, "`{key}` is not a per-object setting"),
            KeyError::Value {
                key,
                value,
                expected,
            } => write!(formatter, "`{key}` expects {expected}, not `{value}`"),
        }
    }
}

impl std::error::Error for KeyError {}

/// Every setting a panel may set on one object.
///
/// These are the print-level settings: the ones that describe what one part
/// prints like, as opposed to which machine prints it or which material is
/// loaded. [`Settings::apply_key`] also accepts the dialect spelling of each
/// name; this list is the engine's own spelling.
pub const PER_OBJECT_KEYS: &[&str] = &[
    "automatic_infill_combination",
    "bottom_fill_pattern",
    "bottom_layers",
    "bridge_angle",
    "bridge_cross_hatch",
    "bridge_density",
    "bridge_line_width",
    "brim_gap",
    "elefant_foot_compensation",
    "ensure_vertical_shell_thickness",
    "extra_perimeters",
    "extra_perimeters_on_overhangs",
    "fill_angle",
    "gap_fill_enabled",
    "infill_anchor",
    "infill_anchor_max",
    "infill_combination_max_layer_height",
    "infill_density",
    "infill_every_layers",
    "infill_overlap",
    "infill_pattern",
    "infill_width",
    "ironing",
    "ironing_spacing",
    "ironing_type",
    "line_width",
    "min_feature_size",
    "only_one_perimeter_first_layer",
    "outer_wall_width",
    "perimeter_generator",
    "resolution",
    "slice_closing_radius",
    "solid_infill_below_area",
    "solid_infill_every_layers",
    "solid_infill_width",
    "spiral_vase",
    "support",
    "support_density",
    "support_interface_layers",
    "support_spacing",
    "support_threshold_angle",
    "support_xy_gap",
    "support_z_gap",
    "thin_walls",
    "top_fill_pattern",
    "top_infill_width",
    "top_layers",
    "top_one_perimeter_type",
    "wall_distribution_count",
    "wall_transition_angle",
    "walls",
    "xy_size_compensation",
];

impl Settings {
    /// The settings a panel may offer per object.
    pub fn per_object_keys() -> &'static [&'static str] {
        PER_OBJECT_KEYS
    }

    /// Sets one print-level setting, by this engine's name or the dialect's.
    ///
    /// Values are written the way a preset writes them: `0.45`, `0.45mm`, `45%`,
    /// `true`, or the name of a choice. A key this engine cannot set on one
    /// object is refused rather than ignored, because a value that reaches
    /// nothing is worse than one that is refused where it was typed.
    pub fn apply_key(&mut self, key: &str, value: &str) -> Result<(), KeyError> {
        let name = hfw_config::native_for(key.trim().to_ascii_lowercase().as_str())
            .unwrap_or(key.trim().to_ascii_lowercase().as_str())
            .to_owned();
        let key_name = name.clone();
        let refused = |expected: &'static str| KeyError::Value {
            key: key_name.clone(),
            value: value.trim().to_owned(),
            expected,
        };
        match name.as_str() {
            "spiral_vase" => self.spiral_vase = boolean(value).ok_or_else(|| refused(ON_OFF))?,
            "ironing" => self.ironing = boolean(value).ok_or_else(|| refused(ON_OFF))?,
            "ironing_type" => {
                self.ironing_target = match lower(value).as_str() {
                    "all_solid" | "all" => IroningTarget::AllSolid,
                    "topmost_only" | "topmost" => IroningTarget::TopmostOnly,
                    "top" | "top_surfaces" => IroningTarget::TopSurfaces,
                    _ => return Err(refused("all_solid, topmost_only or top")),
                }
            }
            "ironing_spacing" => {
                self.ironing_spacing = millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "brim_gap" => self.brim_gap = millimetres(value).ok_or_else(|| refused(LENGTH))?,
            "elefant_foot_compensation" | "elephant_foot_compensation" => {
                self.elephant_foot = millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "xy_size_compensation" => {
                self.xy_size = millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "slice_closing_radius" => {
                self.closing_radius = millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "resolution" => self.resolution = millimetres(value).ok_or_else(|| refused(LENGTH))?,
            "walls" => {
                self.wall.walls =
                    count(value, 1).ok_or_else(|| refused("a whole number of at least 1"))?
            }
            "line_width" => self.wall.w = width(value).ok_or_else(|| refused(LENGTH))?,
            "outer_wall_width" => {
                self.wall.w_outer = width(value).ok_or_else(|| refused(LENGTH))?
            }
            "thin_walls" => self.wall.thin_walls = boolean(value).ok_or_else(|| refused(ON_OFF))?,
            "gap_fill_enabled" => {
                self.wall.gap_fill = boolean(value).ok_or_else(|| refused(ON_OFF))?
            }
            "wall_distribution_count" => {
                self.wall.distribution =
                    count(value, 1).ok_or_else(|| refused("a whole number of at least 1"))?
            }
            "wall_transition_angle" => {
                self.wall.corner_deg = number(value).ok_or_else(|| refused(NUMBER))?
            }
            "min_feature_size" => {
                self.wall.min_feature = millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "perimeter_generator" => {
                self.mode = match lower(value).as_str() {
                    "classic" => Mode::Classic,
                    "hfw" | "arachne" => Mode::Hfw,
                    _ => return Err(refused("classic, hfw or arachne")),
                }
            }
            "extra_perimeters" => {
                self.extra_perimeters = boolean(value).ok_or_else(|| refused(ON_OFF))?
            }
            "extra_perimeters_on_overhangs" => {
                self.extra_perimeters_overhangs = boolean(value).ok_or_else(|| refused(ON_OFF))?
            }
            "only_one_perimeter_first_layer" => {
                self.one_perimeter_first_layer = boolean(value).ok_or_else(|| refused(ON_OFF))?
            }
            "top_one_perimeter_type" => {
                self.one_perimeter_top = match lower(value).as_str() {
                    "none" => String::new(),
                    "top" | "topmost" => lower(value),
                    _ => return Err(refused("none, top or topmost")),
                }
            }
            "infill_density" => {
                self.infill_mut(refused)?.density =
                    fraction(value).ok_or_else(|| refused(PERCENT))?
            }
            "infill_width" => {
                let width = width(value).ok_or_else(|| refused(LENGTH))?;
                self.infill_mut(refused)?.width = width
            }
            "solid_infill_width" => {
                self.infill_mut(refused)?.solid_width =
                    millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "top_infill_width" => {
                self.infill_mut(refused)?.top_width =
                    millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "bridge_line_width" => {
                self.infill_mut(refused)?.bridge.width =
                    millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "top_fill_pattern" => {
                let pattern = skin(value).ok_or_else(|| refused(SKIN))?;
                self.infill_mut(refused)?.top_pattern = pattern
            }
            "bottom_fill_pattern" => {
                let pattern = skin(value).ok_or_else(|| refused(SKIN))?;
                self.infill_mut(refused)?.bottom_pattern = pattern
            }
            "infill_pattern" => {
                let pattern = infill_pattern(value).ok_or_else(|| refused(PATTERN))?;
                self.infill_mut(refused)?.pattern = pattern
            }
            "solid_infill_every_layers" => {
                self.infill_mut(refused)?.solid_every =
                    count(value, 0).ok_or_else(|| refused("a whole number"))?
            }
            "solid_infill_below_area" => {
                let area = number(value).ok_or_else(|| refused(NUMBER))?;
                self.infill_mut(refused)?.solid_below_area = area
            }
            "infill_every_layers" => {
                self.infill_mut(refused)?.combine_layers =
                    count(value, 1).ok_or_else(|| refused("a whole number of at least 1"))?
            }
            "automatic_infill_combination" => {
                self.infill_mut(refused)?.combine_auto =
                    boolean(value).ok_or_else(|| refused(ON_OFF))?
            }
            "infill_combination_max_layer_height" => {
                let height = millimetres(value).ok_or_else(|| refused(LENGTH))?;
                self.infill_mut(refused)?.combine_max_height = height
            }
            "top_layers" => {
                self.infill_mut(refused)?.top_layers =
                    count(value, 0).ok_or_else(|| refused("a whole number"))?
            }
            "bottom_layers" => {
                self.infill_mut(refused)?.bottom_layers =
                    count(value, 0).ok_or_else(|| refused("a whole number"))?
            }
            "ensure_vertical_shell_thickness" => {
                let shell = match lower(value).as_str() {
                    "disabled" | "off" => VerticalShell::Disabled,
                    "partial" => VerticalShell::Partial,
                    "enabled" | "on" => VerticalShell::Enabled,
                    _ => return Err(refused("disabled, partial or enabled")),
                };
                self.infill_mut(refused)?.vertical_shell = shell
            }
            "infill_overlap" => {
                let width = self.infill_mut(refused)?.width;
                let overlap = fraction(value).ok_or_else(|| refused(PERCENT))?;
                self.infill_mut(refused)?.overlap = if overlap >= 0.0 {
                    overlap * width
                } else {
                    0.1 * width
                }
            }
            "fill_angle" => {
                self.infill_mut(refused)?.fill_angle =
                    number(value).ok_or_else(|| refused(NUMBER))?
            }
            "bridge_density" => {
                self.infill_mut(refused)?.bridge.density =
                    fraction(value).ok_or_else(|| refused(PERCENT))?
            }
            "bridge_cross_hatch" => {
                self.infill_mut(refused)?.bridge.cross_hatch =
                    boolean(value).ok_or_else(|| refused(ON_OFF))?
            }
            "bridge_angle" => {
                let angle = number(value).ok_or_else(|| refused(NUMBER))?;
                self.infill_mut(refused)?.bridge.angle =
                    if angle >= 0.0 { Some(angle) } else { None }
            }
            "max_bridge_length" => {
                let length = millimetres(value).ok_or_else(|| refused(LENGTH))?;
                self.infill_mut(refused)?.bridge.max_length = length
            }
            "infill_anchor" => {
                let anchor = millimetres(value).ok_or_else(|| refused(LENGTH))?;
                self.infill_mut(refused)?.anchor = anchor
            }
            "infill_anchor_max" => {
                let anchor = millimetres(value).ok_or_else(|| refused(LENGTH))?;
                self.infill_mut(refused)?.anchor_max = anchor
            }
            "support" => {
                // The dialect names what to support rather than whether to: `none`,
                // `everywhere` and `buildplate` are the switches, and `enforcers_only`
                // turns support on with the painted facets as its only seeds.
                let text = lower(value);
                let painted_only = text == "enforcers_only";
                let on = if painted_only {
                    true
                } else {
                    match text.as_str() {
                        "enabled" | "enable" | "everywhere" | "auto" | "buildplate"
                        | "build_plate" => true,
                        "disabled" | "disable" | "none" => false,
                        _ => boolean(&text).ok_or_else(|| refused(ON_OFF))?,
                    }
                };
                self.support = if on {
                    let mut params = crate::SupportSettings::new(self.wall.w);
                    params.painted_only = painted_only;
                    Some(params)
                } else {
                    None
                }
            }
            "support_threshold_angle" => {
                self.support_mut().threshold_angle = number(value).ok_or_else(|| refused(NUMBER))?
            }
            "support_xy_gap" => {
                self.support_mut().xy_gap = millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "support_z_gap" => {
                self.support_mut().z_gap = millimetres(value).ok_or_else(|| refused(LENGTH))?
            }
            "support_interface_layers" => {
                self.support_mut().interface_layers =
                    count(value, 0).ok_or_else(|| refused("a whole number"))?
            }
            "support_density" => {
                self.support_mut().density = fraction(value).ok_or_else(|| refused(PERCENT))?
            }
            "support_spacing" => {
                let support = self.support_mut();
                let spacing = millimetres(value).ok_or_else(|| refused(LENGTH))?;
                support.density = if spacing <= 0.0 {
                    1.0
                } else {
                    (support.width / spacing).min(1.0)
                };
            }
            _ => return Err(KeyError::Unknown(name)),
        }
        Ok(())
    }

    /// The value of `key`, written the way [`Settings::apply_key`] reads it.
    ///
    /// This is the inverse of `apply_key` for the published keys: a project file
    /// records settings as name and value, and a value this returns always parses
    /// back to the settings it came from. A key that is not published is refused
    /// rather than guessed at, so a caller cannot save something it could not load.
    pub fn key_value(&self, key: &str) -> Result<String, KeyError> {
        let name = hfw_config::native_for(key.trim().to_ascii_lowercase().as_str())
            .unwrap_or(key.trim().to_ascii_lowercase().as_str())
            .to_owned();
        let read = match name.as_str() {
            "spiral_vase" => flag(self.spiral_vase),
            "ironing" => flag(self.ironing),
            "ironing_type" => match self.ironing_target {
                IroningTarget::AllSolid => "all_solid",
                IroningTarget::TopmostOnly => "topmost_only",
                IroningTarget::TopSurfaces => "top_surfaces",
            }
            .to_owned(),
            "ironing_spacing" => decimal(self.ironing_spacing),
            "brim_gap" => decimal(self.brim_gap),
            "elefant_foot_compensation" => decimal(self.elephant_foot),
            "xy_size_compensation" => decimal(self.xy_size),
            "slice_closing_radius" => decimal(self.closing_radius),
            "resolution" => decimal(self.resolution),
            "walls" => self.wall.walls.to_string(),
            "line_width" => decimal(self.wall.w),
            "outer_wall_width" => decimal(self.wall.w_outer),
            "thin_walls" => flag(self.wall.thin_walls),
            "gap_fill_enabled" => flag(self.wall.gap_fill),
            "wall_distribution_count" => self.wall.distribution.to_string(),
            "wall_transition_angle" => decimal(self.wall.corner_deg),
            "min_feature_size" => decimal(self.wall.min_feature),
            "perimeter_generator" => match self.mode {
                Mode::Classic => "classic",
                Mode::Field => "field",
                Mode::Hfw => "hfw",
            }
            .to_owned(),
            "extra_perimeters" => flag(self.extra_perimeters),
            "extra_perimeters_on_overhangs" => flag(self.extra_perimeters_overhangs),
            "only_one_perimeter_first_layer" => flag(self.one_perimeter_first_layer),
            "top_one_perimeter_type" => match self.one_perimeter_top.as_str() {
                "" => "none".to_owned(),
                other => other.to_owned(),
            },
            "infill_density" => percent(self.infill_or(&name)?.density),
            "infill_width" => decimal(self.infill_or(&name)?.width),
            "solid_infill_width" => decimal(self.infill_or(&name)?.solid_width),
            "top_infill_width" => decimal(self.infill_or(&name)?.top_width),
            "bridge_line_width" => decimal(self.infill_or(&name)?.bridge.width),
            "top_fill_pattern" => {
                crate::skin_pattern_name(self.infill_or(&name)?.top_pattern).to_owned()
            }
            "bottom_fill_pattern" => {
                crate::skin_pattern_name(self.infill_or(&name)?.bottom_pattern).to_owned()
            }
            "infill_pattern" => {
                crate::infill_pattern_name(self.infill_or(&name)?.pattern).to_owned()
            }
            "solid_infill_every_layers" => self.infill_or(&name)?.solid_every.to_string(),
            "solid_infill_below_area" => decimal(self.infill_or(&name)?.solid_below_area),
            "infill_every_layers" => self.infill_or(&name)?.combine_layers.to_string(),
            "automatic_infill_combination" => flag(self.infill_or(&name)?.combine_auto),
            "infill_combination_max_layer_height" => {
                decimal(self.infill_or(&name)?.combine_max_height)
            }
            "top_layers" => self.infill_or(&name)?.top_layers.to_string(),
            "bottom_layers" => self.infill_or(&name)?.bottom_layers.to_string(),
            "ensure_vertical_shell_thickness" => match self.infill_or(&name)?.vertical_shell {
                VerticalShell::Disabled => "disabled",
                VerticalShell::Partial => "partial",
                VerticalShell::Enabled => "enabled",
            }
            .to_owned(),
            "infill_overlap" => {
                let params = self.infill_or(&name)?;
                let share = if params.width > 0.0 {
                    params.overlap / params.width
                } else {
                    0.0
                };
                percent(share)
            }
            "fill_angle" => decimal(self.infill_or(&name)?.fill_angle),
            "bridge_density" => percent(self.infill_or(&name)?.bridge.density),
            "bridge_cross_hatch" => flag(self.infill_or(&name)?.bridge.cross_hatch),
            "bridge_angle" => decimal(self.infill_or(&name)?.bridge.angle.unwrap_or(-1.0)),
            "max_bridge_length" => decimal(self.infill_or(&name)?.bridge.max_length),
            "infill_anchor" => decimal(self.infill_or(&name)?.anchor),
            "infill_anchor_max" => decimal(self.infill_or(&name)?.anchor_max),
            "support" => flag(self.support.is_some()),
            "support_threshold_angle" => decimal(self.support_or(&name)?.threshold_angle),
            "support_xy_gap" => decimal(self.support_or(&name)?.xy_gap),
            "support_z_gap" => decimal(self.support_or(&name)?.z_gap),
            "support_interface_layers" => self.support_or(&name)?.interface_layers.to_string(),
            "support_density" => percent(self.support_or(&name)?.density),
            "support_spacing" => {
                let params = self.support_or(&name)?;
                if params.density > 0.0 {
                    decimal(params.width / params.density)
                } else {
                    decimal(0.0)
                }
            }
            _ => return Err(KeyError::Unknown(name)),
        };
        Ok(read)
    }

    fn infill_or(&self, key: &str) -> Result<&crate::InfillParams, KeyError> {
        self.infill.as_ref().ok_or_else(|| KeyError::Value {
            key: key.to_owned(),
            value: String::new(),
            expected: "an object with infill enabled",
        })
    }

    fn support_or(&self, key: &str) -> Result<&crate::SupportParams, KeyError> {
        self.support.as_ref().ok_or_else(|| KeyError::Value {
            key: key.to_owned(),
            value: String::new(),
            expected: "an object with support enabled",
        })
    }

    fn infill_mut(
        &mut self,
        refused: impl Fn(&'static str) -> KeyError,
    ) -> Result<&mut crate::InfillParams, KeyError> {
        self.infill
            .as_mut()
            .ok_or_else(|| refused("an object with infill enabled"))
    }

    fn support_mut(&mut self) -> &mut crate::SupportSettings {
        let width = self.wall.w;
        self.support
            .get_or_insert_with(|| crate::SupportSettings::new(width))
    }
}

const ON_OFF: &str = "1, 0, true, false, yes, no, on or off";
const LENGTH: &str = "a length in millimetres";
const NUMBER: &str = "a number";
const PERCENT: &str = "a fraction, or a percentage such as 25%";
const PATTERN: &str = "an infill pattern this engine draws";
const SKIN: &str = "a skin pattern this engine draws";

fn flag(value: bool) -> String {
    if value {
        "1".to_owned()
    } else {
        "0".to_owned()
    }
}

fn decimal(value: f64) -> String {
    let mut text = format!("{value}");
    if text == "-0" {
        text = "0".to_owned();
    }
    text
}

fn percent(share: f64) -> String {
    format!("{}%", decimal(share * 100.0))
}

fn lower(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn number(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

fn millimetres(value: &str) -> Option<f64> {
    let text = value.trim().trim_end_matches("mm").trim();
    text.parse().ok()
}

fn width(value: &str) -> Option<f64> {
    millimetres(value).filter(|width| *width > 0.0)
}

fn count(value: &str, least: usize) -> Option<usize> {
    let count: usize = value.trim().parse().ok()?;
    (count >= least).then_some(count)
}

fn boolean(value: &str) -> Option<bool> {
    match lower(value).as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn fraction(value: &str) -> Option<f64> {
    let text = value.trim();
    if let Some(percent) = text.strip_suffix('%') {
        let value: f64 = percent.trim().parse().ok()?;
        return Some((value / 100.0).clamp(0.0, 1.0));
    }
    let value: f64 = text.parse().ok()?;
    Some(value.clamp(0.0, 1.0))
}

fn skin(value: &str) -> Option<SkinPattern> {
    skin_pattern(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[(&str, &str, &str)] = &[
        ("automatic_infill_combination", "1", "0"),
        ("bottom_fill_pattern", "concentric", "rectilinear"),
        ("bottom_layers", "6", "2"),
        ("bridge_angle", "30", "-1"),
        ("bridge_cross_hatch", "1", "0"),
        ("bridge_density", "50%", "100%"),
        ("bridge_line_width", "0.5", "0.4"),
        ("brim_gap", "0.2", "0.1"),
        ("elefant_foot_compensation", "0.2", "0"),
        ("ensure_vertical_shell_thickness", "partial", "enabled"),
        ("extra_perimeters", "0", "1"),
        ("extra_perimeters_on_overhangs", "0", "1"),
        ("fill_angle", "45", "0"),
        ("gap_fill_enabled", "1", "0"),
        ("infill_anchor", "2", "0"),
        ("infill_anchor_max", "12", "20"),
        ("infill_combination_max_layer_height", "0.4", "0.2"),
        ("infill_density", "35%", "5%"),
        ("infill_every_layers", "2", "1"),
        ("infill_overlap", "12%", "0%"),
        ("infill_pattern", "gyroid", "rectilinear"),
        ("infill_width", "0.45", "0.3"),
        ("ironing", "1", "0"),
        ("ironing_spacing", "0.12", "0.05"),
        ("ironing_type", "topmost_only", "all_solid"),
        ("line_width", "0.42", "0.5"),
        ("min_feature_size", "0.1", "0.4"),
        ("only_one_perimeter_first_layer", "1", "0"),
        ("outer_wall_width", "0.4", "0.5"),
        ("perimeter_generator", "classic", "hfw"),
        ("resolution", "0.01", "0"),
        ("slice_closing_radius", "0.05", "0"),
        ("solid_infill_below_area", "70", "0"),
        ("solid_infill_every_layers", "0", "2"),
        ("solid_infill_width", "0.44", "0.3"),
        ("spiral_vase", "0", "1"),
        ("support", "1", "0"),
        ("support_density", "25%", "100%"),
        ("support_interface_layers", "2", "0"),
        ("support_spacing", "3", "0.5"),
        ("support_threshold_angle", "45", "0"),
        ("support_xy_gap", "0.2", "0"),
        ("support_z_gap", "0.15", "0"),
        ("thin_walls", "1", "0"),
        ("top_fill_pattern", "concentric", "rectilinear"),
        ("top_infill_width", "0.44", "0.3"),
        ("top_layers", "7", "3"),
        ("top_one_perimeter_type", "topmost", "none"),
        ("wall_distribution_count", "2", "1"),
        ("wall_transition_angle", "15", "0"),
        ("walls", "4", "5"),
        ("xy_size_compensation", "0.1", "0"),
    ];

    #[test]
    fn the_published_list_is_the_one_the_engine_accepts() {
        let published = Settings::per_object_keys();
        for (key, _, _) in SAMPLE {
            assert!(
                published.contains(key),
                "`{key}` is accepted but not published"
            );
        }
        assert_eq!(
            published.len(),
            SAMPLE.len(),
            "a published key has no sample"
        );
        for key in published {
            assert!(SAMPLE.iter().any(|(name, _, _)| name == key));
        }
    }

    #[test]
    fn every_published_key_sets_something() {
        for (key, value, alternative) in SAMPLE {
            let before = format!("{:?}", Settings::new(0.4, 3));
            let answers = [value, alternative]
                .iter()
                .map(|candidate| {
                    let mut settings = Settings::new(0.4, 3);
                    settings
                        .apply_key(key, candidate)
                        .unwrap_or_else(|error| panic!("`{key}` = `{candidate}`: {error}"));
                    format!("{settings:?}") != before
                })
                .collect::<Vec<bool>>();
            assert!(
                answers.iter().any(|changed| *changed),
                "`{key}` sets nothing for `{value}` or `{alternative}`"
            );
        }
    }

    #[test]
    fn every_published_key_reads_back_as_the_value_it_was_set_with() {
        for (key, value, alternative) in SAMPLE {
            for candidate in [value, alternative] {
                let mut settings = Settings::new(0.4, 3);
                settings
                    .apply_key(key, candidate)
                    .unwrap_or_else(|error| panic!("`{key}` = `{candidate}`: {error}"));
                let written = settings
                    .key_value(key)
                    .unwrap_or_else(|error| panic!("`{key}` cannot be read back: {error}"));
                let mut reloaded = Settings::new(0.4, 3);
                reloaded.apply_key(key, &written).unwrap_or_else(|error| {
                    panic!("`{key}` was written as `{written}` and refused: {error}")
                });
                assert_eq!(
                    format!("{:?}", settings),
                    format!("{:?}", reloaded),
                    "`{key}` = `{candidate}` came back as `{written}` and changed the settings"
                );
            }
        }
    }

    #[test]
    fn the_dialects_support_spellings_say_what_to_support() {
        let mut settings = Settings::new(0.4, 3);
        settings.apply_key("support", "everywhere").unwrap();
        let everywhere = settings.support.expect("everywhere turns support on");
        assert!(!everywhere.painted_only);

        settings.apply_key("support", "none").unwrap();
        assert!(settings.support.is_none(), "`none` turns support off");

        settings.apply_key("support", "enforcers_only").unwrap();
        let painted = settings.support.expect("`enforcers_only` turns support on");
        assert!(
            painted.painted_only,
            "`enforcers_only` supports only what the painter marked"
        );

        let refused = settings.apply_key("support", "sometimes");
        assert!(
            refused.is_err(),
            "an unknown spelling is refused, not guessed"
        );
    }

    #[test]
    fn a_key_that_is_not_published_is_not_written_either() {
        let settings = Settings::new(0.4, 3);
        assert_eq!(
            settings.key_value("layer_height"),
            Err(KeyError::Unknown("layer_height".to_owned()))
        );
    }

    #[test]
    fn a_value_the_key_cannot_hold_names_the_key_and_the_value() {
        let mut settings = Settings::new(0.4, 3);
        let error = settings.apply_key("walls", "four").expect_err("refused");
        match error {
            KeyError::Value { key, value, .. } => {
                assert_eq!(key, "walls");
                assert_eq!(value, "four");
            }
            other => panic!("{other:?}"),
        }
        assert!(settings.apply_key("layer_height", "0.2").is_err());
        let unknown = settings.apply_key("no_such_setting", "1").unwrap_err();
        assert_eq!(unknown, KeyError::Unknown("no_such_setting".to_owned()));
    }

    #[test]
    fn the_dialect_spelling_reaches_the_same_field() {
        let mut native = Settings::new(0.4, 3);
        let mut dialect = native.clone();
        native.apply_key("infill_density", "40%").expect("native");
        dialect
            .apply_key("sparse_infill_density", "40%")
            .expect("dialect");
        assert_eq!(
            format!("{:?}", native.infill.as_ref().map(|infill| infill.density)),
            format!("{:?}", dialect.infill.as_ref().map(|infill| infill.density)),
        );
        assert_eq!(native.infill.map(|infill| infill.density), Some(0.4));
    }

    #[test]
    fn a_support_parameter_implies_support_and_off_drops_it() {
        let mut settings = Settings::new(0.4, 3);
        assert!(settings.support.is_none());
        settings.apply_key("support_xy_gap", "0.2").expect("gap");
        assert_eq!(settings.support.map(|support| support.xy_gap), Some(0.2));
        settings.apply_key("support", "0").expect("support off");
        assert!(settings.support.is_none());
        settings.apply_key("support", "1").expect("support on");
        settings.apply_key("support_xy_gap", "0.3").expect("gap");
        assert_eq!(settings.support.map(|support| support.xy_gap), Some(0.3));
    }
}
