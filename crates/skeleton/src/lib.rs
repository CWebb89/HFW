//! Skeletal trapezoidation of a sliced layer.
//!
//! The wall stage needs more than a stack of offsets: a bead width is continuous,
//! so where a layer is thinner than a whole number of beads the widths themselves
//! have to change rather than a spare strand being squeezed in afterwards. The
//! structure that answers "how wide is the layer here, and where does that width
//! change" is the medial axis, and the form of it that survives contact with a
//! tessellated outline is the skeletal trapezoidation: the Voronoi diagram of the
//! outline's own segments, walked as a graph.
//!
//! This crate is the wall generator, stage by stage: it builds the graph, gives
//! every node the distance to the boundary it is nearest to, discretizes the edges
//! so a bead can follow them, marks which edges are wide enough to hold whole
//! beads, and lays bead centrelines along them. It does not yet move a bead count
//! along an edge, and it is not wired into the wall stage.
//!
//! ```no_run
//! use hfw_kernel::Region;
//! use hfw_skeleton::{
//!     build, classify::classify, emit, emit::Params as EmitParams, classify::Params as ClassifyParams, Params,
//! };
//!
//! let region = Region::default();
//! let mut skeleton = build(&region, &Params::default())?;
//! classify(&mut skeleton, &ClassifyParams::default());
//! let beads = emit(&skeleton, &region, &EmitParams::default());
//! # Ok::<(), hfw_skeleton::Error>(())
//! ```

#![forbid(unsafe_code)]

pub mod bead;
pub mod build;
pub mod classify;
pub mod emit;
pub mod graph;

pub use build::{build, prepare, Error, Params};
pub use emit::{emit, Bead};
pub use graph::{EdgeId, Graph, HalfEdge, Kind, Node, NodeId, Pair, PairId};
