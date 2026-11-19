//! Which edges of the skeleton are wide enough to hold whole beads.
//!
//! An edge of the medial axis is a place where the layer's width changes, and a
//! bead can only follow it while the width stays clear of the bead count changing.
//! The test is the one the reference slicers use: an edge is *central* when the
//! width it spans, relative to its own length, is smaller than the opening angle
//! the bead transition allows. Written as a diagram, `A` and `B` are its ends and
//! `dR` the width difference between them:
//!
//! ```text
//!                                     _.-'^`      dR is the width change
//!                               _.-'^`           dD is the edge's length
//!                         _.-'^`   dR           sin a = dR / dD, and the edge
//!                   _.-'^`  _______|            is central while dR / dD is
//!             _.-'^`  A             B           below sin(transitioning angle / 2)
//! ```
//!
//! A tapered tip of a feature is not central however short its edges are: below
//! the thickness that holds a single bead, the width has to change continuously
//! and no bead count does.

use crate::graph::{Graph, Kind, PairId};

/// How classification is decided.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// The angle at which a bead count is allowed to transition, in radians.
    ///
    /// This is the wall transition angle: the opening angle of the wedge a
    /// transition needs, and therefore the limit on how fast the width may change
    /// along a central edge. Ten degrees is what the reference slicers default to.
    pub transitioning_angle: f64,
    /// The narrowest layer thickness that can hold a single bead, in
    /// millimetres.
    ///
    /// Both ends of an edge nearer the boundary than half this are the inside of
    /// a tapered feature rather than part of a wall, so the edge is never central.
    pub tip_thickness: f64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            transitioning_angle: 10.0_f64.to_radians(),
            tip_thickness: 0.3,
        }
    }
}

/// Marks every edge central or not.
///
/// Ribs are never central: a rib runs from the skeleton out to the boundary, so
/// its width falls to nothing by construction.
pub fn classify(graph: &mut Graph, p: &Params) {
    let cap = (0.5 * p.transitioning_angle).sin();
    for i in 0..graph.pair_count() {
        let id = PairId(i as u32);
        let pair = graph.pair(id);
        let da = graph.node(pair.a).distance;
        let db = graph.node(pair.b).distance;
        let central = match pair.kind {
            Kind::Rib => false,
            Kind::Voronoi => {
                let chord = graph.node(pair.a).p.dist(graph.node(pair.b).p);
                da.max(db) >= 0.5 * p.tip_thickness && (db - da).abs() < chord * cap
            }
        };
        graph.set_central(id, central);
    }
}

/// Dissolves the central stretches too short to be worth a bead count change.
///
/// A stretch shorter than the distance a transition needs cannot carry one, and
/// leaving it central would make the beading stage print a transition in a place
/// with no room for it. A stretch that contains a local maximum stays: that node
/// is where the bead count of the whole region is decided, and dissolving around
/// it would leave the count undefined.
///
/// Returns the number of edges dissolved.
pub fn filter_central(graph: &mut Graph, max_length: f64) -> usize {
    let mut visited = vec![false; graph.pair_count()];
    let mut dissolved = 0;
    for start in 0..graph.pair_count() {
        if visited[start] || graph.pair(PairId(start as u32)).central != Some(true) {
            continue;
        }
        let mut chain = Vec::new();
        let mut stack = vec![PairId(start as u32)];
        let mut holds_maximum = false;
        visited[start] = true;
        while let Some(id) = stack.pop() {
            chain.push(id);
            let pair = graph.pair(id);
            holds_maximum |= graph.is_local_maximum(pair.a) || graph.is_local_maximum(pair.b);
            for end in [pair.a, pair.b] {
                for &half in graph.edges_at(end) {
                    let next = graph.half(half).pair;
                    if visited[next.usize()] || graph.pair(next).central != Some(true) {
                        continue;
                    }
                    visited[next.usize()] = true;
                    stack.push(next);
                }
            }
        }
        let total: f64 = chain.iter().map(|&id| graph.pair(id).length).sum();
        if total < max_length && !holds_maximum {
            for &id in &chain {
                graph.set_central(id, false);
            }
            dissolved += chain.len();
        }
    }
    dissolved
}
