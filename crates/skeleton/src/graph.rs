//! The skeleton graph: nodes, their distance to the boundary, and the
//! discretized edges between them.
//!
//! Geometry is stored once per undirected edge and direction twice: a [`Pair`]
//! holds the polyline, and the two [`HalfEdge`]s that reference it carry the
//! direction, so walking a curve and walking around a node are both cheap.
//! Half-edge `2k` is a pair's forward direction and `2k + 1` its twin, which
//! makes the twin lookup a single xor and keeps the arena free of pointers.

use hfw_kernel::{Bounds, V2};

/// Index of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The index as a slice index.
    pub fn usize(self) -> usize {
        self.0 as usize
    }
}

/// Index of a half-edge; `2k` and `2k + 1` are the twins of pair `k`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeId(pub u32);

impl EdgeId {
    /// The index as a slice index.
    pub fn usize(self) -> usize {
        self.0 as usize
    }

    /// The undirected edge this half-edge belongs to.
    pub fn pair(self) -> PairId {
        PairId(self.0 >> 1)
    }

    /// The half-edge pointing the other way.
    pub fn twin(self) -> EdgeId {
        EdgeId(self.0 ^ 1)
    }
}

/// Index of an undirected edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PairId(pub u32);

impl PairId {
    /// The index as a slice index.
    pub fn usize(self) -> usize {
        self.0 as usize
    }
}

/// What an edge of the graph is a piece of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A part of the Voronoi diagram of the boundary, so a part of the medial
    /// axis.
    Voronoi,
    /// A connector from a skeleton node to the boundary point it is nearest to.
    Rib,
}

/// A point of the skeleton and how far it is from the region's boundary.
#[derive(Clone, Debug)]
pub struct Node {
    /// Position in millimetres.
    pub p: V2,
    /// Distance to the nearest boundary point, in millimetres. Zero for a node
    /// that lies on the boundary itself.
    pub distance: f64,
}

/// What the layer looks like across one sample of an edge.
#[derive(Clone, Copy, Debug, Default)]
#[allow(clippy::struct_field_names)]
pub struct Span {
    /// Distance to the boundary towards the sample's left normal, in millimetres.
    pub ahead: f64,
    /// Distance to the boundary towards its right normal, in millimetres.
    pub back: f64,
    /// Boundary segment the `ahead` ray reached, or `u32::MAX` if none.
    pub ahead_site: u32,
    /// Boundary segment the `back` ray reached, or `u32::MAX` if none.
    pub back_site: u32,
}

impl Span {
    /// Width of the layer across the sample, measured perpendicular to the edge.
    pub fn thickness(&self) -> f64 {
        self.ahead + self.back
    }

    /// How far the section's centre is from the sample, along the left normal.
    ///
    /// The section is not symmetric about the medial axis — a bead runs down the
    /// middle of the layer, and the middle is not the point the boundary is
    /// nearest to except between two parallel walls.
    pub fn skew(&self) -> f64 {
        0.5 * (self.ahead - self.back)
    }
}

/// One undirected edge: its ends, its geometry and its classification.
#[derive(Clone, Debug)]
pub struct Pair {
    /// First end.
    pub a: NodeId,
    /// Second end.
    pub b: NodeId,
    /// What the edge is a piece of.
    pub kind: Kind,
    /// The discretized polyline from `a` to `b`, both ends included.
    pub samples: Vec<V2>,
    /// The layer's width across every sample, parallel to [`Pair::samples`].
    ///
    /// This is what a bead's width comes from: the beads laid across a sample have
    /// to fill exactly the section the two rays measure, and it is not twice the
    /// distance to the nearest boundary — a bead runs down the middle of a section,
    /// and near a corner the middle is further from both walls than the nearest
    /// point is.
    pub spans: Vec<Span>,
    /// Polyline length in millimetres.
    pub length: f64,
    /// Whether the edge is inside a region wide enough to be built of full
    /// beads. `None` until [`crate::classify`] has run.
    pub central: Option<bool>,
}

/// One direction of a [`Pair`].
#[derive(Clone, Copy, Debug)]
pub struct HalfEdge {
    /// The undirected edge this direction belongs to.
    pub pair: PairId,
    /// Where this direction starts.
    pub from: NodeId,
    /// Where this direction ends.
    pub to: NodeId,
}

/// A skeleton: nodes with their distance to the boundary, and the edges between
/// them.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    nodes: Vec<Node>,
    pairs: Vec<Pair>,
    halves: Vec<HalfEdge>,
    incident: Vec<Vec<EdgeId>>,
}

impl Graph {
    /// An empty graph.
    pub fn new() -> Graph {
        Graph::default()
    }

    /// Number of nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of undirected edges.
    pub fn pair_count(&self) -> usize {
        self.pairs.len()
    }

    /// True when the graph has no edges, which is what an empty or entirely
    /// too-narrow layer produces.
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Every node.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// One node.
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.usize()]
    }

    /// Every undirected edge.
    pub fn pairs(&self) -> &[Pair] {
        &self.pairs
    }

    /// One undirected edge.
    pub fn pair(&self, id: PairId) -> &Pair {
        &self.pairs[id.usize()]
    }

    /// One direction of one edge.
    pub fn half(&self, id: EdgeId) -> &HalfEdge {
        &self.halves[id.usize()]
    }

    /// The polyline from the half-edge's start to its end.
    pub fn polyline(&self, id: EdgeId) -> Vec<V2> {
        let pair = self.pair(id.pair());
        if id.usize() % 2 == 0 {
            pair.samples.clone()
        } else {
            pair.samples.iter().rev().copied().collect()
        }
    }

    /// Every half-edge leaving a node, ordered by the node it reaches and then
    /// by its own index, so a walk around a node is reproducible.
    pub fn edges_at(&self, id: NodeId) -> &[EdgeId] {
        &self.incident[id.usize()]
    }

    /// The distance to the boundary at the far end of a half-edge.
    pub fn distance_at_end(&self, id: EdgeId) -> f64 {
        self.node(self.half(id).to).distance
    }

    /// True when no neighbour of the node is farther from the boundary than the
    /// node itself.
    ///
    /// A local maximum is where a bead count is decided: the layer is widest
    /// there, so the number of beads the widest part of the region gets is fixed
    /// at these nodes and propagated along the ribs from them. Two nodes of a
    /// level spine are both maxima, which is what keeps a uniform slot from
    /// dissolving the stretch that carries its bead count.
    pub fn is_local_maximum(&self, id: NodeId) -> bool {
        let d = self.node(id).distance;
        d > 0.0
            && self
                .edges_at(id)
                .iter()
                .all(|&e| self.distance_at_end(e) <= d)
    }

    /// Every node that is a local maximum, in index order.
    pub fn local_maxima(&self) -> Vec<NodeId> {
        (0..self.nodes.len())
            .map(|i| NodeId(i as u32))
            .filter(|&n| self.is_local_maximum(n))
            .collect()
    }

    /// Bounding box of every node.
    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for n in &self.nodes {
            b.add(n.p);
        }
        b
    }

    pub(crate) fn set_central(&mut self, id: PairId, central: bool) {
        self.pairs[id.usize()].central = Some(central);
    }

    pub(crate) fn push_node(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(node);
        self.incident.push(Vec::new());
        id
    }

    pub(crate) fn connect(
        &mut self,
        a: NodeId,
        b: NodeId,
        kind: Kind,
        samples: Vec<V2>,
        spans: Vec<Span>,
    ) -> PairId {
        let length = samples.windows(2).map(|w| w[0].dist(w[1])).sum();
        let id = PairId(self.pairs.len() as u32);
        self.pairs.push(Pair {
            a,
            b,
            kind,
            samples,
            spans,
            length,
            central: None,
        });
        let forward = EdgeId(self.halves.len() as u32);
        self.halves.push(HalfEdge {
            pair: id,
            from: a,
            to: b,
        });
        self.halves.push(HalfEdge {
            pair: id,
            from: b,
            to: a,
        });
        self.incident[a.usize()].push(forward);
        self.incident[b.usize()].push(forward.twin());
        id
    }

    pub(crate) fn sort_incident(&mut self) {
        let nodes = &self.nodes;
        let halves = &self.halves;
        for list in self.incident.iter_mut() {
            list.sort_by(|&l, &r| {
                let lp = nodes[halves[l.usize()].to.usize()].p;
                let rp = nodes[halves[r.usize()].to.usize()].p;
                lp.x.total_cmp(&rp.x)
                    .then_with(|| lp.y.total_cmp(&rp.y))
                    .then_with(|| l.cmp(&r))
            });
        }
    }
}

/// Unit normal at every sample of an edge, turned with its direction.
///
/// The normal at a sample is the perpendicular of the line through its neighbours,
/// so it is defined even at the ends of the edge, and every stage that offsets
/// something from a sample uses this one function: two different conventions would
/// put a bead and the ray that measures it on different sides of the line.
pub fn normals(samples: &[V2]) -> Vec<V2> {
    let n = samples.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let back = samples[i.saturating_sub(1)];
        let ahead = samples[(i + 1).min(n.saturating_sub(1))];
        let t = ahead - back;
        out.push(if t.len() > 1e-12 {
            t.norm().perp()
        } else {
            V2::ZERO
        });
    }
    out
}
