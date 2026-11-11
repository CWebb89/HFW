//! Errors raised while preparing a G-code job.

use std::fmt;

use hfw_config::ConfigError;

/// Everything that can stop G-code generation.
#[derive(Clone, Debug, PartialEq)]
pub enum GcodeError {
    /// The printer profile is invalid.
    Config(ConfigError),
    /// There is nothing to print.
    NoLayers,
    /// The part plus its skirt does not fit the bed.
    DoesNotFit {
        /// Required width including skirt, in millimetres.
        width: f64,
        /// Required depth including skirt, in millimetres.
        depth: f64,
        /// Bed width in millimetres.
        bed_x: f64,
        /// Bed depth in millimetres.
        bed_y: f64,
    },
    /// The part is taller than the machine.
    TooTall {
        /// Part height in millimetres.
        height: f64,
        /// Machine Z limit in millimetres.
        max_z: f64,
    },
}

impl From<ConfigError> for GcodeError {
    fn from(e: ConfigError) -> GcodeError {
        GcodeError::Config(e)
    }
}

impl fmt::Display for GcodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GcodeError::Config(e) => write!(f, "{e}"),
            GcodeError::NoLayers => write!(f, "no layers to print"),
            GcodeError::DoesNotFit {
                width,
                depth,
                bed_x,
                bed_y,
            } => write!(
                f,
                "part plus skirt is {width:.1} x {depth:.1} mm but the bed is {bed_x:.0} x {bed_y:.0} mm (2 mm safety margin)"
            ),
            GcodeError::TooTall { height, max_z } => write!(
                f,
                "part is {height:.1} mm tall but max_z is {max_z:.0} mm"
            ),
        }
    }
}

impl std::error::Error for GcodeError {}
