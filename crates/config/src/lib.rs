//! Printer, filament and process profiles.
//!
//! A profile is a flat `key = value` file with `#` comments and `<<TAG` heredoc
//! blocks for G-code fragments. Every key is validated at parse time, so a
//! typo is an error rather than a silently ignored setting. Unknown keys are
//! refused deliberately: a profile that quietly drops `retract_len` produces
//! G-code that looks fine and prints badly.

#![forbid(unsafe_code)]

mod error;
mod parse;

pub use error::ConfigError;

/// G-code dialect the profile targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// Klipper, including `SET_PRINT_STATS_INFO` and `EXCLUDE_OBJECT_DEFINE`.
    Klipper,
    /// Marlin, without Klipper-specific commands.
    Marlin,
}

/// Where the seam of a closed loop is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seam {
    /// Rotate every loop to the same absolute direction from the centroid.
    Aligned,
    /// Start each loop at the point nearest the previous end position.
    Nearest,
}

/// A complete, validated printer and process profile.
#[derive(Clone, Debug)]
pub struct PrinterConfig {
    /// Profile name, echoed into the G-code banner.
    pub name: String,
    /// Travel limit along X in millimetres.
    pub bed_x: f64,
    /// Travel limit along Y in millimetres.
    pub bed_y: f64,
    /// Travel limit along Z in millimetres.
    pub max_z: f64,
    /// True when the bed origin is at its centre rather than its front left.
    pub origin_center: bool,
    /// Target G-code dialect.
    pub flavor: Flavor,
    /// True to emit relative extruder moves (`M83`).
    pub relative_e: bool,
    /// Nozzle diameter in millimetres.
    pub nozzle_diameter: f64,
    /// Filament diameter in millimetres.
    pub filament_diameter: f64,
    /// Filament density in grams per cubic centimetre.
    pub filament_density: f64,
    /// Nominal extrusion width in millimetres.
    pub line_width: f64,
    /// Narrowest bead the process is willing to print.
    pub min_bead_width: f64,
    /// Layer height after the first layer, in millimetres.
    pub layer_height: f64,
    /// First layer height in millimetres.
    pub first_layer_height: f64,
    /// Perimeter loop count.
    pub walls: usize,
    /// Sparse infill density in `0..=1`.
    pub infill_density: f64,
    /// Sparse infill pattern name.
    pub infill_pattern: String,
    /// Solid skin layers over the top surface.
    pub top_layers: usize,
    /// Solid skin layers under the bottom surface.
    pub bottom_layers: usize,
    /// Steady-state nozzle temperature in degrees Celsius.
    pub nozzle_temp: f64,
    /// First layer nozzle temperature in degrees Celsius.
    pub first_nozzle_temp: f64,
    /// Bed temperature in degrees Celsius.
    pub bed_temp: f64,
    /// Part cooling fan duty in percent, `0..=100`.
    pub fan_speed: f64,
    /// Layer index at which the fan is switched on.
    pub fan_start_layer: usize,
    /// Retraction distance in millimetres; zero disables retraction.
    pub retract_len: f64,
    /// Retraction speed in millimetres per second.
    pub retract_speed: f64,
    /// Shortest travel that triggers a retraction away from infill.
    pub retract_min_travel: f64,
    /// Shortest travel that triggers a retraction between infill moves.
    pub infill_retract_min_travel: f64,
    /// Z hop height in millimetres; zero disables hopping.
    pub z_hop: f64,
    /// Travel speed in millimetres per second.
    pub travel_speed: f64,
    /// First layer speed in millimetres per second.
    pub first_layer_speed: f64,
    /// Outer wall speed in millimetres per second.
    pub outer_wall_speed: f64,
    /// Inner wall speed in millimetres per second.
    pub inner_wall_speed: f64,
    /// Speed for beads from the medial axis, in millimetres per second.
    pub thin_speed: f64,
    /// Sparse infill speed in millimetres per second.
    pub infill_speed: f64,
    /// Solid skin speed in millimetres per second.
    pub solid_speed: f64,
    /// Volumetric flow ceiling in cubic millimetres per second.
    pub max_volumetric_speed: f64,
    /// Machine speed ceiling in millimetres per second.
    pub max_speed: f64,
    /// Minimum time per layer used to slow small layers down.
    pub min_layer_time: f64,
    /// Lower bound on the layer-time speed scale.
    pub min_speed_factor: f64,
    /// Extrusion multiplier applied to every bead.
    pub flow: f64,
    /// Seam placement rule for closed loops.
    pub seam: Seam,
    /// True to print inner walls before the outer wall.
    pub inner_walls_first: bool,
    /// Number of skirt loops on the first layer.
    pub skirt_loops: usize,
    /// Gap between the skirt and the part, in millimetres.
    pub skirt_distance: f64,
    /// True to emit `SET_PRINT_STATS_INFO` for Klipper layer tracking.
    pub emit_print_stats: bool,
    /// True to emit `EXCLUDE_OBJECT_DEFINE` so the part can be cancelled.
    pub exclude_object: bool,
    /// Pressure advance applied at the start of the print, if non-zero.
    pub pressure_advance: f64,
    /// Start G-code, with `{placeholders}` substituted before emission.
    pub start_gcode: String,
    /// End G-code, with `{placeholders}` substituted before emission.
    pub end_gcode: String,
}

impl Default for PrinterConfig {
    fn default() -> PrinterConfig {
        PrinterConfig {
            name: "generic".to_owned(),
            bed_x: 220.0,
            bed_y: 220.0,
            max_z: 250.0,
            origin_center: false,
            flavor: Flavor::Marlin,
            relative_e: true,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            filament_density: 1.24,
            line_width: 0.4,
            min_bead_width: 0.2,
            layer_height: 0.2,
            first_layer_height: 0.2,
            walls: 3,
            infill_density: 0.15,
            infill_pattern: "rectilinear".to_owned(),
            top_layers: 5,
            bottom_layers: 4,
            nozzle_temp: 210.0,
            first_nozzle_temp: 215.0,
            bed_temp: 60.0,
            fan_speed: 100.0,
            fan_start_layer: 2,
            retract_len: 0.8,
            retract_speed: 35.0,
            retract_min_travel: 1.5,
            infill_retract_min_travel: 12.0,
            z_hop: 0.0,
            travel_speed: 150.0,
            first_layer_speed: 20.0,
            outer_wall_speed: 40.0,
            inner_wall_speed: 60.0,
            thin_speed: 30.0,
            infill_speed: 80.0,
            solid_speed: 60.0,
            max_volumetric_speed: 12.0,
            max_speed: 300.0,
            min_layer_time: 8.0,
            min_speed_factor: 0.25,
            flow: 1.0,
            seam: Seam::Aligned,
            inner_walls_first: true,
            skirt_loops: 2,
            skirt_distance: 3.0,
            emit_print_stats: false,
            exclude_object: false,
            pressure_advance: 0.0,
            start_gcode:
                "M140 S{bed_temp}\nM104 S150\nG28\nM190 S{bed_temp}\nM109 S{first_nozzle_temp}"
                    .to_owned(),
            end_gcode: "M104 S0\nM140 S0\nM107\nM84".to_owned(),
        }
    }
}

impl PrinterConfig {
    /// Cross-sectional area of the filament, in square millimetres.
    pub fn filament_area(&self) -> f64 {
        std::f64::consts::PI * 0.25 * self.filament_diameter * self.filament_diameter
    }

    /// Substitutes `{placeholders}` in a G-code fragment.
    pub fn fill(&self, text: &str, extra: &[(&str, String)]) -> String {
        let mut out = text.to_owned();
        let base = [
            ("{bed_temp}", format!("{}", self.bed_temp)),
            ("{nozzle_temp}", format!("{}", self.nozzle_temp)),
            ("{first_nozzle_temp}", format!("{}", self.first_nozzle_temp)),
            ("{bed_x}", format!("{}", self.bed_x)),
            ("{bed_y}", format!("{}", self.bed_y)),
            ("{max_z}", format!("{}", self.max_z)),
            ("{pressure_advance}", format!("{}", self.pressure_advance)),
        ];
        for (key, value) in base.iter().chain(extra.iter()) {
            out = out.replace(key, value);
        }
        out
    }
}
