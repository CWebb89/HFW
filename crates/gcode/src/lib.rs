//! G-code emission and an independent safety verifier.
//!
//! The writer turns planned bead paths into machine commands. Each segment's
//! extrusion is derived from the segment's own width, so a variable-width bead
//! is correct on firmware that knows nothing about variable widths: the `E`
//! value carries the volume. The `;WIDTH:` comments are annotation only.
//!
//! The verifier re-reads the finished file as text and applies printer physics
//! to it, rather than trusting the writer's bookkeeping. It is the only check
//! that can catch a bug in the writer, because it shares no state with it.
//!
//! The time estimate runs the same way the machine would: every emitted move goes
//! through a trapezoidal planner with the profile's acceleration and cornering
//! model, so a file full of short segments is not reported as if the nozzle
//! reached every commanded speed instantly.

#![forbid(unsafe_code)]

mod error;
mod route;
mod time;
mod writer;

pub mod thumb;
pub mod verify;

pub use error::GcodeError;
pub use time::{Move, Planner, Tune};
pub use verify::{verify, VerifyReport};
pub use writer::{feature_of, generate, Feature, Gcode, GcodeStats, ModelExtent, Path};

pub use hfw_toolpath::{bead_area, bead_width};
