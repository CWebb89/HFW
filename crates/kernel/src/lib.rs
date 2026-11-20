//! Floating-point planar geometry and spatial indices for the wall stage.
//!
//! The mesh and slicing crates work in exact integer nanometres. Everything
//! after the slice boundary is approximate by nature: offsets, distance
//! fields and medial axes are solved numerically. This crate holds that
//! numerical kernel in millimetres.

#![forbid(unsafe_code)]

pub mod cap;
pub mod geom;
pub mod index;
pub mod offset;

pub use geom::{ring_contains, ring_signed_area, seg_closest, Bounds, Region, V2};
pub use index::{Hit, PointIndex, RegionIndex, SegIndex};
pub use offset::{closing, difference_rings, intersect_rings, offset_rings, union_rings, Join};
