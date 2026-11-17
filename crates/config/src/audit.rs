//! What a profile asks for and will not get.
//!
//! Four settings in this project failed in a row without anything reporting it: a
//! retraction that lifted nothing because `retract_lift` was absent and defaulted
//! to zero, an E feedrate ceiling of 25 mm/s that silently capped every retraction,
//! a `brim_type` the writer never read, and a limit pair the firmware does not
//! implement. Each one printed, and each one was wrong in a way no component
//! mentioned.
//!
//! This module is the report that would have caught them. It does not judge a
//! profile; it names two things a person cannot see by reading a settings sheet:
//! a value a machine limit overrides, and a value of zero that switches a feature
//! off. Both are silent by construction -- the setting is present, applied, and
//! has no effect.

use crate::PrinterConfig;

/// Why a setting does not do what it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// A machine limit is lower than the value, so the value never applies.
    Masked,
    /// The value is zero, so the feature it controls is switched off.
    Off,
}

impl Severity {
    /// The word the report leads with.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Masked => "masked",
            Severity::Off => "off",
        }
    }
}

/// One setting that will not behave as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// What kind of silence this is.
    pub severity: Severity,
    /// The setting, its value, the limit that overrides it, and the value that
    /// applies instead, in one sentence.
    pub message: String,
}

impl Finding {
    /// `masked: outer_wall_speed is 300 mm/s ...`.
    pub fn line(&self) -> String {
        format!("{}: {}", self.severity.label(), self.message)
    }
}

/// Every setting in `cfg` that a limit overrides or a zero disables.
///
/// Ordered masked-before-off, and within each, by the order below, so two runs
/// over the same profile print the same report.
pub fn audit(cfg: &PrinterConfig) -> Vec<Finding> {
    let mut masked = Vec::new();
    let mut off = Vec::new();

    let xy = min_positive(cfg.machine_max_feedrate_x, cfg.machine_max_feedrate_y);
    if xy > 0.0 {
        let speeds: [(&str, f64); 22] = [
            ("outer_wall_speed", cfg.outer_wall_speed),
            ("inner_wall_speed", cfg.inner_wall_speed),
            ("thin_speed", cfg.thin_speed),
            ("solid_speed", cfg.solid_speed),
            ("top_solid_speed", cfg.top_solid_speed),
            ("infill_speed", cfg.infill_speed),
            ("support_speed", cfg.support_speed),
            ("support_interface_speed", cfg.support_interface_speed),
            ("ironing_speed", cfg.ironing_speed),
            ("bridge_speed", cfg.bridge_speed),
            ("over_bridge_speed", cfg.over_bridge_speed),
            ("travel_speed", cfg.travel_speed),
            ("first_layer_speed", cfg.first_layer_speed),
            ("first_layer_outer_speed", cfg.first_layer_outer_speed),
            ("first_layer_inner_speed", cfg.first_layer_inner_speed),
            ("first_layer_perimeter_speed", cfg.first_layer_perimeter_speed),
            ("first_layer_solid_speed", cfg.first_layer_solid_speed),
            ("first_layer_infill_speed", cfg.first_layer_infill_speed),
            ("max_speed", cfg.max_speed),
            (
                "filament_infill_max_speed",
                cfg.filament_infill_max_speed,
            ),
            (
                "filament_infill_max_crossing_speed",
                cfg.filament_infill_max_crossing_speed,
            ),
            ("overhang_speeds[0]", cfg.overhang_speeds[0]),
        ];
        for (name, value) in speeds {
            if value > xy {
                masked.push(Finding {
                    severity: Severity::Masked,
                    message: format!(
                        "{name} asks {value:.0} mm/s; the machine ceiling is {xy:.0} mm/s, so it prints at {xy:.0}"
                    ),
                });
            }
        }
        for (index, rung) in cfg.overhang_speeds.iter().enumerate().skip(1) {
            if *rung > xy {
                masked.push(Finding {
                    severity: Severity::Masked,
                    message: format!(
                        "overhang_speeds[{index}] asks {rung:.0} mm/s; the machine ceiling is {xy:.0} mm/s"
                    ),
                });
            }
        }
    }

    let e = cfg.machine_max_feedrate_e;
    if e > 0.0 {
        for (name, value) in [
            ("retract_speed", cfg.retract_speed),
            ("deretract_speed", cfg.deretract_speed),
        ] {
            if value > e {
                masked.push(Finding {
                    severity: Severity::Masked,
                    message: format!(
                        "{name} asks {value:.0} mm/s; the extruder ceiling is {e:.0} mm/s, so every one runs at {e:.0}"
                    ),
                });
            }
        }
    }

    let accel = min_positive(cfg.machine_max_accel_x, cfg.machine_max_accel_y);
    if accel > 0.0 {
        let accelerations: [(&str, f64); 12] = [
            ("default_accel", cfg.default_accel),
            ("initial_layer_accel", cfg.initial_layer_accel),
            ("outer_wall_accel", cfg.outer_wall_accel),
            ("inner_wall_accel", cfg.inner_wall_accel),
            ("perimeter_accel", cfg.perimeter_accel),
            ("solid_accel", cfg.solid_accel),
            ("top_solid_accel", cfg.top_solid_accel),
            ("infill_accel", cfg.infill_accel),
            ("support_accel", cfg.support_accel),
            ("support_interface_accel", cfg.support_interface_accel),
            ("bridge_acceleration", cfg.bridge_acceleration),
            ("travel_acceleration", cfg.travel_acceleration),
        ];
        for (name, value) in accelerations {
            if value > accel {
                masked.push(Finding {
                    severity: Severity::Masked,
                    message: format!(
                        "{name} asks {value:.0} mm/s^2; the machine ceiling is {accel:.0} mm/s^2, so it runs at {accel:.0}"
                    ),
                });
            }
        }
    }

    let flow = match (
        cfg.max_volumetric_speed > 0.0,
        cfg.filament_max_volumetric_speed > 0.0,
    ) {
        (true, true) => cfg
            .max_volumetric_speed
            .min(cfg.filament_max_volumetric_speed),
        (true, false) => cfg.max_volumetric_speed,
        (false, true) => cfg.filament_max_volumetric_speed,
        (false, false) => 0.0,
    };
    let bead = cfg.line_width * cfg.layer_height;
    if flow > 0.0 && bead > 0.0 {
        let allowed = flow / bead;
        let fastest: Vec<(&str, f64)> = [
            ("outer_wall_speed", cfg.outer_wall_speed),
            ("inner_wall_speed", cfg.inner_wall_speed),
            ("solid_speed", cfg.solid_speed),
            ("infill_speed", cfg.infill_speed),
        ]
        .into_iter()
        .filter(|(_, value)| *value > allowed)
        .collect();
        for (name, value) in fastest {
            masked.push(Finding {
                severity: Severity::Masked,
                message: format!(
                    "{name} asks {value:.0} mm/s; a {} x {:.2} mm bead at the {flow:.1} mm^3/s flow ceiling allows {allowed:.0} mm/s",
                    fmt_mm(cfg.line_width),
                    cfg.layer_height
                ),
            });
        }
    }

    if cfg.retract_len == 0.0 {
        off.push(Finding {
            severity: Severity::Off,
            message: "retract_len is 0: nothing retracts, so every travel keeps its pressure and strings"
                .to_owned(),
        });
    } else if cfg.z_hop == 0.0 {
        off.push(Finding {
            severity: Severity::Off,
            message: format!(
                "z_hop is 0 with a {:.2} mm retraction: travels cross printed perimeters at print height",
                cfg.retract_len
            ),
        });
    }
    if cfg.pressure_advance == 0.0 {
        off.push(Finding {
            severity: Severity::Off,
            message: "pressure_advance is 0: every corner and speed change carries its own pressure error"
                .to_owned(),
        });
    }
    if cfg.nozzle_temp == 0.0 && cfg.first_nozzle_temp == 0.0 {
        off.push(Finding {
            severity: Severity::Off,
            message: "nozzle_temp and first_nozzle_temp are both 0: the print never asks for heat"
                .to_owned(),
        });
    }
    if cfg.bed_temp == 0.0 && cfg.first_layer_bed_temp == 0.0 {
        off.push(Finding {
            severity: Severity::Off,
            message:
                "bed_temp and first_layer_bed_temp are both 0: the plate stays cold for the whole print"
                    .to_owned(),
        });
    }
    masked.extend(off);
    masked
}

/// The lower of two limits, where zero means the profile does not state one.
fn min_positive(a: f64, b: f64) -> f64 {
    match (a > 0.0, b > 0.0) {
        (true, true) => a.min(b),
        (true, false) => a,
        (false, true) => b,
        (false, false) => 0.0,
    }
}

/// `0.45`, without the trailing zeros `{}` would print.
fn fmt_mm(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PrinterConfig {
        let mut cfg = PrinterConfig::default();
        cfg.retract_len = 0.8;
        cfg.retract_speed = 35.0;
        cfg.deretract_speed = 35.0;
        cfg.z_hop = 0.4;
        cfg.pressure_advance = 0.04;
        cfg.machine_max_feedrate_x = 500.0;
        cfg.machine_max_feedrate_y = 500.0;
        cfg.machine_max_feedrate_e = 120.0;
        cfg.machine_max_accel_x = 20000.0;
        cfg.machine_max_accel_y = 20000.0;
        cfg.nozzle_temp = 210.0;
        cfg.first_nozzle_temp = 210.0;
        cfg.bed_temp = 60.0;
        cfg.first_layer_bed_temp = 60.0;
        cfg
    }

    #[test]
    fn a_profile_whose_settings_all_apply_reports_nothing() {
        assert_eq!(audit(&base()), Vec::new());
    }

    #[test]
    fn a_retraction_that_lifts_nothing_is_named() {
        let mut cfg = base();
        cfg.z_hop = 0.0;
        let findings = audit(&cfg);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].severity, Severity::Off);
        assert!(
            findings[0].message.contains("z_hop is 0"),
            "{}",
            findings[0].message
        );
    }

    #[test]
    fn an_extruder_ceiling_caps_the_retraction_it_masks() {
        let mut cfg = base();
        cfg.machine_max_feedrate_e = 25.0;
        let findings = audit(&cfg);
        assert!(
            findings.iter().any(|f| f.severity == Severity::Masked
                && f.message.contains("retract_speed asks 35")
                && f.message.contains("runs at 25")),
            "{findings:?}"
        );
    }

    #[test]
    fn a_speed_above_the_xy_ceiling_is_reported_with_both_numbers() {
        let mut cfg = base();
        cfg.outer_wall_speed = 300.0;
        cfg.machine_max_feedrate_x = 200.0;
        let findings = audit(&cfg);
        let line = findings
            .iter()
            .find(|f| f.message.starts_with("outer_wall_speed"))
            .unwrap_or_else(|| panic!("{findings:?}"));
        assert!(line.message.contains("asks 300"), "{}", line.message);
        assert!(line.message.contains("ceiling is 200"), "{}", line.message);
    }

    #[test]
    fn an_acceleration_above_the_machine_ceiling_is_reported() {
        let mut cfg = base();
        cfg.outer_wall_accel = 9000.0;
        cfg.machine_max_accel_x = 5000.0;
        let findings = audit(&cfg);
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("outer_wall_accel asks 9000")
                    && f.message.contains("5000")),
            "{findings:?}"
        );
    }

    #[test]
    fn the_flow_ceiling_reports_the_speed_it_allows() {
        let mut cfg = base();
        cfg.line_width = 0.45;
        cfg.layer_height = 0.2;
        cfg.filament_max_volumetric_speed = 8.0;
        cfg.outer_wall_speed = 200.0;
        let findings = audit(&cfg);
        assert!(
            findings
                .iter()
                .any(|f| f.message.starts_with("outer_wall_speed asks 200")
                    && f.message.contains("89 mm/s")),
            "{findings:?}"
        );
    }

    #[test]
    fn a_first_layer_temperature_that_falls_through_is_not_a_finding() {
        let mut cfg = base();
        cfg.first_layer_bed_temp = 0.0;
        cfg.first_nozzle_temp = 0.0;
        assert_eq!(audit(&cfg), Vec::new());
    }

    #[test]
    fn a_masked_finding_prints_its_class_first() {
        let mut cfg = base();
        cfg.retract_len = 0.0;
        let line = audit(&cfg)[0].line();
        assert!(line.starts_with("off: retract_len is 0"), "{line}");
    }
}
