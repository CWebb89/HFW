//! Building the skeleton from a sliced region.
//!
//! Four steps, and the order matters: the outline is first simplified and thinned
//! into a shape a Voronoi diagram can survive, then each connected part gets its
//! own diagram, then every diagram is walked into a [`Graph`] with the geometry
//! discretized and everything outside the part dropped, and each sample is given
//! the width the layer has there.
//!
//! The preparation step is not defensive programming for an input that cannot
//! happen. A slice outline arrives with near-duplicate points, hairline slivers
//! and edges that meet at almost-zero angles, all of which the Voronoi sweep
//! treats as numerical ties.

use std::fmt;

use boostvoronoi::prelude::{Builder, Diagram, Edge, Line, Point, SourceCategory, VertexIndex};
use hfw_kernel::{ring_contains, ring_signed_area, Region, RegionIndex, SegIndex, V2};

use crate::graph::{Graph, Kind, Node, NodeId, Span};

/// Lattice units per millimetre, matching the kernel's integer overlay.
const UNITS_PER_MM: f64 = 1.0e6;

/// Largest number of samples one edge is discretized into, which bounds the work
/// a pathological edge can cost.
const MAX_STEPS: usize = 4096;

/// How the skeleton is built.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Spacing of the sample points along an edge, in millimetres.
    ///
    /// An edge of the medial axis is where the layer's width changes, so its
    /// geometry is sampled finely enough for a bead to follow it. 0.8 mm is what
    /// both reference slicers use.
    pub discretization: f64,
    /// The largest distance the outline may move while it is being simplified, in
    /// millimetres.
    ///
    /// The diagram costs about 35 µs per boundary vertex, so the vertex count is
    /// the price, and a slice of a curved surface arrives with thousands of
    /// vertices no bead can show. Douglas–Peucker keeps every dropped point inside
    /// this distance of the simplified outline, which is a bound the whole wall
    /// stage already lives within: the slice's own `resolution` is 0.012 mm.
    pub tolerance: f64,
    /// The closest two boundary vertices may be after simplification, in
    /// millimetres.
    ///
    /// A Voronoi sweep treats two vertices closer than this as a tie and produces
    /// a diagram that is not planar, so they are merged on the way in.
    pub min_spacing: f64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            discretization: 0.8,
            tolerance: 0.012,
            min_spacing: 0.005,
        }
    }
}

/// What can go wrong while building a skeleton.
#[derive(Clone, Debug)]
pub enum Error {
    /// The outline could not be turned into a diagram at all.
    Voronoi(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Voronoi(m) => write!(f, "voronoi of the layer outline failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Reduces an outline to the vertices a Voronoi sweep can afford and survive.
///
/// Both steps have a bound that holds: Douglas–Peucker never moves the outline by
/// more than `tolerance`, and dropping a vertex that is within `min_spacing` of
/// the one before it never moves it by more than `min_spacing`. A vertex cannot
/// simply be dropped because it is close to its neighbours' chord, though — a run
/// of nearly collinear vertices each within tolerance of its *original* neighbours
/// can collapse a whole lobe, which on the plate fixture turned a 1186 mm² outline
/// into a 478 mm² one. That is why the simplification is Douglas–Peucker, which
/// measures against the chord it actually intends to keep.
pub fn prepare(region: &Region, p: &Params) -> Region {
    if region.is_empty() {
        return region.clone();
    }
    let rings = region
        .rings
        .iter()
        .map(|ring| thin(&simplify_ring(ring, p.tolerance), p.min_spacing))
        .filter(|ring| ring.len() >= 3)
        .collect();
    Region::new(rings)
}

/// Douglas–Peucker on a closed ring.
///
/// A ring has no ends to anchor the recursion, so it is split at the two vertices
/// farthest apart and each half is simplified on its own. Every dropped vertex is
/// within `tolerance` of the chord that replaces it, which is the whole guarantee
/// the diagram needs.
fn simplify_ring(ring: &[V2], tolerance: f64) -> Vec<V2> {
    let n = ring.len();
    if n < 4 || tolerance.is_nan() || tolerance <= 0.0 {
        return ring.to_vec();
    }
    let mut far = 0usize;
    let mut best = 0.0;
    for (i, p) in ring.iter().enumerate() {
        let d = ring[0].dist(*p);
        if d > best {
            best = d;
            far = i;
        }
    }
    if far == 0 {
        return ring.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    douglas_peucker(ring, 0, far, tolerance, &mut keep, false);
    douglas_peucker(ring, far, 0, tolerance, &mut keep, true);
    let out: Vec<V2> = (0..n).filter(|&i| keep[i]).map(|i| ring[i]).collect();
    if out.len() < 3 {
        ring.to_vec()
    } else {
        out
    }
}

fn douglas_peucker(
    ring: &[V2],
    from: usize,
    to: usize,
    tolerance: f64,
    keep: &mut [bool],
    wraps: bool,
) {
    let n = ring.len();
    let mut stack = vec![(from, to)];
    while let Some((a, b)) = stack.pop() {
        let span = if wraps {
            (b + n - a) % n
        } else {
            b.saturating_sub(a)
        };
        if span < 2 {
            continue;
        }
        let (pa, pb) = (ring[a], ring[b]);
        let mut worst = 0.0;
        let mut idx = a;
        let mut k = (a + 1) % n;
        while k != b {
            let d = distance_to_line(ring[k], pa, pb);
            if d > worst {
                worst = d;
                idx = k;
            }
            k = (k + 1) % n;
        }
        if worst > tolerance {
            keep[idx] = true;
            stack.push((a, idx));
            stack.push((idx, b));
        }
    }
}

fn distance_to_line(p: V2, a: V2, b: V2) -> f64 {
    let ab = b - a;
    let len = ab.len();
    if len < 1e-12 {
        return p.dist(a);
    }
    (ab.cross(p - a) / len).abs()
}

fn thin(ring: &[V2], min_spacing: f64) -> Vec<V2> {
    if min_spacing.is_nan() || min_spacing <= 0.0 {
        return ring.to_vec();
    }
    let mut spaced: Vec<V2> = Vec::with_capacity(ring.len());
    for &p in ring {
        if spaced.last().is_none_or(|&q: &V2| q.dist(p) >= min_spacing) {
            spaced.push(p);
        }
    }
    while spaced.len() >= 3 && spaced[0].dist(spaced[spaced.len() - 1]) < min_spacing {
        spaced.pop();
    }
    spaced
}

/// Builds the skeleton of a region.
///
/// Every connected part gets its own diagram: a plate is a dozen parts that cannot
/// see each other, and the sweep's cost is per vertex, so one diagram for the whole
/// plate is one large problem where a dozen small ones would do. It is also the
/// shape the answer wants — a bead cannot cross from one part to another, so
/// nothing is lost by refusing to look.
///
/// # Errors
///
/// Returns [`Error::Voronoi`] when a part's diagram cannot be built, which on a
/// prepared outline means the part was too degraded to describe. The caller
/// decides whether to fail the layer or fall back to offsets; the skeleton does not
/// drop walls on its own.
pub fn build(region: &Region, p: &Params) -> Result<Graph, Error> {
    let prepared = prepare(region, p);
    if prepared.is_empty() {
        return Ok(Graph::new());
    }
    let mut graph = Graph::new();
    for part in parts(&prepared) {
        let part = Region::new(part);
        let segments = part.segments();
        if segments.len() < 3 {
            continue;
        }
        let lattice = quantize(&segments);
        let diagram = Builder::<i64>::default()
            .with_segments(lattice.iter().copied())
            .and_then(Builder::build)
            .map_err(|e| Error::Voronoi(e.to_string()))?;
        convert(&diagram, &segments, &part, p, &mut graph);
    }
    graph.sort_incident();
    Ok(graph)
}

/// Splits a region into connected parts: one outer ring with the holes inside it.
///
/// The ring list is flat, and a hole belongs to whichever outer ring contains it
/// and is smallest — the innermost enclosing boundary, which is what "inside the
/// same piece of material" means.
fn parts(region: &Region) -> Vec<Vec<Vec<V2>>> {
    let rings = &region.rings;
    let depths: Vec<usize> = (0..rings.len())
        .map(|i| {
            (0..rings.len())
                .filter(|&j| j != i && ring_contains(&rings[j], rings[i][0]))
                .count()
        })
        .collect();
    let outers: Vec<usize> = (0..rings.len()).filter(|&i| depths[i] % 2 == 0).collect();
    let mut parts: Vec<Vec<Vec<V2>>> = outers.iter().map(|&i| vec![rings[i].clone()]).collect();
    for i in 0..rings.len() {
        if depths[i] % 2 == 0 {
            continue;
        }
        let mut best: Option<(usize, f64)> = None;
        for (slot, &outer) in outers.iter().enumerate() {
            if !ring_contains(&rings[outer], rings[i][0]) {
                continue;
            }
            let area = ring_signed_area(&rings[outer]).abs();
            if best.is_none_or(|(_, a)| area < a) {
                best = Some((slot, area));
            }
        }
        if let Some((slot, _)) = best {
            parts[slot].push(rings[i].clone());
        }
    }
    parts
}

/// The outline as integer segment endpoints, one entry per input segment.
///
/// The diagram reports the index of the segment a cell belongs to, so the list
/// may not skip anything: a segment too short to survive quantization is still
/// passed on, which keeps every later index aligned.
fn quantize(segments: &[(V2, V2)]) -> Vec<Line<i64>> {
    segments
        .iter()
        .map(|&(a, b)| {
            Line::new(
                Point::new(unit(a.x), unit(a.y)),
                Point::new(unit(b.x), unit(b.y)),
            )
        })
        .collect()
}

fn unit(v: f64) -> i64 {
    (v * UNITS_PER_MM).round() as i64
}

fn millimetres(v: f64) -> f64 {
    v / UNITS_PER_MM
}

/// What a diagram cell was built from.
#[derive(Clone, Copy, Debug)]
enum Site {
    /// A boundary vertex, which is a focus of every parabolic edge around it.
    Point(V2),
    /// A boundary segment, whose supporting line is the directrix of those
    /// parabolas.
    Segment(V2, V2),
}

/// Walks one part's diagram into the graph, keeping only what is inside the part.
///
/// The Voronoi diagram covers the plane, and the boundary separates the part from
/// the rest of it: the same segment is a source of skeleton on both sides, and only
/// the side the part contains can be printed. An edge is kept only when its whole
/// length is inside, with points on the boundary itself allowed, which is what
/// stops an edge that merely crosses the boundary from dragging an exterior node
/// into the graph. A point on a hole's boundary has the material on the far side of
/// it, so the same test drops the skeleton a hole would grow inside itself.
fn convert(diagram: &Diagram, segments: &[(V2, V2)], part: &Region, p: &Params, graph: &mut Graph) {
    let index = SegIndex::new(segments.to_vec(), part.bounds().grown(1.0), 2.0);
    let inside = RegionIndex::new(&part.rings);
    let mut seen: Vec<Option<NodeId>> = vec![None; diagram.num_vertices()];

    for i in 0..diagram.num_edges() {
        let id = diagram.edge_index_unchecked(i);
        let Ok(edge) = diagram.edge(id) else {
            continue;
        };
        let Ok(twin_id) = edge.twin() else {
            continue;
        };
        if id.u32() >= twin_id.u32() {
            continue;
        }
        let Ok(twin) = diagram.edge(twin_id) else {
            continue;
        };
        let (Some(va), Some(vb)) = (edge.vertex0(), twin.vertex0()) else {
            continue;
        };
        if va == vb {
            continue;
        }
        let (Ok(a), Ok(b)) = (diagram.vertex(va), diagram.vertex(vb)) else {
            continue;
        };
        let pa = V2::new(millimetres(a.x()), millimetres(a.y()));
        let pb = V2::new(millimetres(b.x()), millimetres(b.y()));
        let samples = discretize(diagram, edge, twin, segments, pa, pb, p);
        if !samples
            .iter()
            .all(|&q| inside.contains(q) || index.nearest(q, 1e-3).is_some())
        {
            continue;
        }
        let spans = spans(&samples, &index);
        let na = node(graph, &mut seen, va, pa, &index);
        let nb = node(graph, &mut seen, vb, pb, &index);
        if na != nb {
            graph.connect(na, nb, Kind::Voronoi, samples, spans);
        }
    }
}

fn node(
    graph: &mut Graph,
    seen: &mut [Option<NodeId>],
    vertex: VertexIndex,
    p: V2,
    index: &SegIndex,
) -> NodeId {
    if let Some(id) = seen[vertex.usize()] {
        return id;
    }
    let distance = index.nearest(p, 1.0e3).map(|hit| hit.d).unwrap_or(0.0);
    let id = graph.push_node(Node { p, distance });
    seen[vertex.usize()] = Some(id);
    id
}

/// Measures the layer's width across every sample of an edge.
///
/// Each sample gets a ray sent both ways along its normal, and the ray is marched
/// by the distance to the nearest boundary: the disc of that radius is empty, so
/// the whole step is free, and the march converges in a handful of steps. That is
/// how the width of a section is found without knowing the angle the two walls make
/// with the skeleton, which is the number a bead needs and the medial distance is
/// not.
fn spans(samples: &[V2], index: &SegIndex) -> Vec<Span> {
    let normals = crate::graph::normals(samples);
    samples
        .iter()
        .zip(&normals)
        .map(|(&q, &n)| {
            let (ahead, ahead_site) = cast(index, q, n).unwrap_or((0.0, u32::MAX));
            let (back, back_site) = cast(index, q, -n).unwrap_or((0.0, u32::MAX));
            Span {
                ahead,
                back,
                ahead_site,
                back_site,
            }
        })
        .collect()
}

/// Marches along a direction until it reaches the boundary.
///
/// Returns how far it travelled and which boundary segment it hit.
fn cast(index: &SegIndex, from: V2, dir: V2) -> Option<(f64, u32)> {
    let limit = 1.0e3;
    let mut t = 0.0;
    for _ in 0..64 {
        let p = from + dir * t;
        let hit = index.nearest(p, limit)?;
        if hit.d <= 1e-4 {
            return Some((t + hit.d, hit.seg));
        }
        t += hit.d;
        if t > limit {
            return None;
        }
    }
    None
}

fn site(diagram: &Diagram, edge: &Edge, segments: &[(V2, V2)]) -> Option<Site> {
    let cell = diagram.cell(edge.cell().ok()?).ok()?;
    let &(start, end) = segments.get(cell.source_index().usize())?;
    Some(match cell.source_category() {
        SourceCategory::Segment => Site::Segment(start, end),
        SourceCategory::SegmentStart | SourceCategory::SinglePoint => Site::Point(start),
        SourceCategory::SegmentEnd => Site::Point(end),
    })
}

/// The sample points of one diagram edge, from `a` to `b`.
fn discretize(
    diagram: &Diagram,
    edge: &Edge,
    twin: &Edge,
    segments: &[(V2, V2)],
    a: V2,
    b: V2,
    p: &Params,
) -> Vec<V2> {
    if !edge.is_curved() {
        return densify(a, b, p.discretization);
    }
    let (Some(left), Some(right)) = (site(diagram, edge, segments), site(diagram, twin, segments))
    else {
        return densify(a, b, p.discretization);
    };
    match (left, right) {
        (Site::Point(focus), Site::Segment(s, e)) | (Site::Segment(s, e), Site::Point(focus)) => {
            parabola(focus, (s, e), a, b, p.discretization)
        }
        _ => densify(a, b, p.discretization),
    }
}

/// Points along a straight edge, both ends included.
fn densify(a: V2, b: V2, step: f64) -> Vec<V2> {
    let len = a.dist(b);
    let steps = if step > 0.0 {
        (len / step).ceil() as usize
    } else {
        1
    }
    .clamp(1, MAX_STEPS);
    (0..=steps)
        .map(|i| a.lerp(b, i as f64 / steps as f64))
        .collect()
}

/// Points along a parabolic edge, both ends included.
///
/// A parabolic Voronoi edge is the set of points as far from a boundary vertex as
/// from the line through the boundary segment beside it, so it is exactly the
/// parabola with that vertex as focus and that line as directrix. In the frame
/// where the directrix is the x axis the parabola is `y = ((x - fx)^2 + fy^2) /
/// (2 fy)`, which is what makes sampling it a matter of walking `x`.
fn parabola(focus: V2, directrix: (V2, V2), a: V2, b: V2, step: f64) -> Vec<V2> {
    let (origin, along) = directrix;
    let axis = along - origin;
    if axis.len() < 1e-12 {
        return densify(a, b, step);
    }
    let u = axis.norm();
    let mut n = u.perp();
    if (focus - origin).dot(n) < 0.0 {
        n = -n;
    }
    let fy = (focus - origin).dot(n);
    if fy.is_nan() || fy <= 1e-9 {
        return densify(a, b, step);
    }
    let fx = (focus - origin).dot(u);
    let local = |p: V2| (p - origin).dot(u);
    let (ax, bx) = (local(a), local(b));
    let steps = if step > 0.0 {
        ((bx - ax).abs() / step).ceil() as usize
    } else {
        1
    }
    .clamp(1, MAX_STEPS);
    let mut out = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let x = if i == 0 {
            ax
        } else if i == steps {
            bx
        } else {
            ax + (bx - ax) * (i as f64 / steps as f64)
        };
        let y = ((x - fx) * (x - fx) + fy * fy) / (2.0 * fy);
        out.push(origin + u * x + n * y);
    }
    out[0] = a;
    let last = out.len() - 1;
    out[last] = b;
    out
}
