//! Routes a travel around the beads already printed on its own layer.
//!
//! A travel that cuts a wall leaves a scar the part keeps, so the writer asks
//! this module for a way round before emitting the straight line. The walls are
//! the layer's own perimeter loops; each is offset both ways by a clearance and
//! those offset rings are the waypoints a route may use, so a routed move hugs a
//! wall at a fixed distance instead of guessing how close it can pass.
//!
//! A graph edge is only allowed when the whole segment stays clear of every wall
//! by at least that clearance, which is checked as a distance between segments.
//! Distance is the right test rather than a polygon test: the walls are open
//! outlines, not filled regions, so a route through the far side of a part is
//! legal exactly when no bead is within reach of it.
//!
//! Two things are deliberately out of scope, because they would need geometry
//! this layer does not carry: support columns and curled overhangs from earlier
//! layers are not obstacles here, so `travel_avoid_supports` and
//! `avoid_crossing_curled_overhangs` remain unimplemented.

use hfw_kernel::V2;

/// Clearance between a routed travel and a printed bead, in bead widths.
///
/// One bead of centre-to-centre distance from the wall's own line puts the route
/// clear of the bead's edge with room for the machine to land on it, so a route
/// that grazes a wall at exactly one bead is the tightest useful one.
const CLEARANCE_IN_BEADS: f64 = 0.6;

/// Most vertices one loop may contribute to the graph.
///
/// A slice outline can carry thousands of points; a graph that big costs more
/// than the crossing it avoids. Every vertex beyond this is skipped uniformly,
/// which keeps the loop's shape and its corners.
const MAX_VERTICES_PER_LOOP: usize = 28;

/// Total graph size above which routing is skipped outright.
const MAX_GRAPH_NODES: usize = 260;

/// A route round a layer's walls.
#[derive(Clone, Debug)]
pub struct Route {
    /// Points to pass through between the start and the end, the end excluded.
    pub via: Vec<V2>,
    /// Length of the whole routed move, in millimetres.
    pub length: f64,
}

/// Attempts a route from `start` to `end` that crosses none of `loops`.
///
/// `bead` is the bead width whose perimeter the route has to clear, and
/// `max_detour` is the longest extra distance worth travelling: zero means no
/// limit, matching the dialect's own reading of the setting.
///
/// Returns `None` when the straight line crosses nothing, when no route fits the
/// detour budget, when the graph is too big to be worth building, or when the
/// route found cannot be proven clear. The caller then emits the straight move it
/// would have emitted anyway, so a refusal is never worse than today's behaviour.
pub fn detour(start: V2, end: V2, loops: &[Vec<V2>], bead: f64, max_detour: f64) -> Option<Route> {
    if loops.is_empty() || bead <= 0.0 || start.dist(end) < 1e-6 {
        return None;
    }
    let clearance = bead * CLEARANCE_IN_BEADS;
    let reach = corridor(start, end, clearance * 2.0);
    let near: Vec<Vec<V2>> = loops
        .iter()
        .filter(|loop_| boxes_meet(&bbox(loop_), &reach))
        .map(|loop_| simplify(loop_, MAX_VERTICES_PER_LOOP))
        .filter(|loop_| loop_.len() >= 3)
        .collect();
    if near.is_empty() {
        return None;
    }
    let walls = segments(&near);
    if !touches(start, end, &walls) {
        return None;
    }
    let mut nodes = vec![start, end];
    let mut rings: Vec<(usize, usize)> = Vec::new();
    for loop_ in &near {
        for distance in [clearance, -clearance] {
            if let Some(ring) = offset(loop_, distance) {
                let from = nodes.len();
                nodes.extend(ring);
                rings.push((from, nodes.len()));
            }
        }
    }
    if nodes.len() > MAX_GRAPH_NODES {
        return None;
    }
    let near_walls: Vec<(V2, V2)> = near.iter().flat_map(|loop_| segments_1(loop_)).collect();
    let via = shortest(&nodes, &rings, &near_walls, clearance)?;
    let length = polyline_length(start, &via, end);
    let budget = if max_detour > 0.0 {
        max_detour
    } else {
        f64::INFINITY
    };
    if length > start.dist(end) + budget {
        return None;
    }
    if !clear_route(start, &via, end, &walls) {
        return None;
    }
    Some(Route { via, length })
}

/// Total length of a move that visits `via` between `start` and `end`.
fn polyline_length(start: V2, via: &[V2], end: V2) -> f64 {
    let mut total = 0.0;
    let mut from = start;
    for point in via {
        total += from.dist(*point);
        from = *point;
    }
    total + from.dist(end)
}

/// Runs Dijkstra over the visibility graph and returns the waypoints.
fn shortest(
    nodes: &[V2],
    rings: &[(usize, usize)],
    walls: &[(V2, V2)],
    clearance: f64,
) -> Option<Vec<V2>> {
    let total = nodes.len();
    let neighbours = ring_mates(rings, total);
    let mut best = vec![f64::INFINITY; total];
    let mut previous = vec![usize::MAX; total];
    let mut settled = vec![false; total];
    best[0] = 0.0;
    loop {
        let mut current = usize::MAX;
        let mut current_cost = f64::INFINITY;
        for i in 0..total {
            if !settled[i] && best[i] < current_cost {
                current = i;
                current_cost = best[i];
            }
        }
        if current == usize::MAX || current == 1 {
            break;
        }
        settled[current] = true;
        for next in 0..total {
            if settled[next] || next == current {
                continue;
            }
            let along_ring = neighbours
                .get(current)
                .is_some_and(|mates| mates.contains(&next));
            let limit = clearance * 0.98;
            if !along_ring && !leg_clear(nodes[current], nodes[next], walls, limit) {
                continue;
            }
            let step = nodes[current].dist(nodes[next]);
            if current_cost + step < best[next] {
                best[next] = current_cost + step;
                previous[next] = current;
            }
        }
    }
    if !best[1].is_finite() {
        return None;
    }
    let mut via = Vec::new();
    let mut at = previous[1];
    while at != usize::MAX && at != 0 {
        via.push(nodes[at]);
        at = previous[at];
    }
    via.reverse();
    Some(via)
}

/// For each node, the two neighbours on its own offset ring.
fn ring_mates(rings: &[(usize, usize)], total: usize) -> Vec<[usize; 2]> {
    let mut mates = vec![[usize::MAX; 2]; total];
    for &(from, to) in rings {
        let len = to - from;
        if len < 3 {
            continue;
        }
        for (offset, slot) in mates[from..to].iter_mut().enumerate() {
            *slot = [from + (offset + 1) % len, from + (offset + len - 1) % len];
        }
    }
    mates
}

/// Every segment of a set of loops.
fn segments(loops: &[Vec<V2>]) -> Vec<(V2, V2)> {
    loops.iter().flat_map(|loop_| segments_1(loop_)).collect()
}

/// Every segment of one loop.
fn segments_1(points: &[V2]) -> Vec<(V2, V2)> {
    let n = points.len();
    (0..n).map(|i| (points[i], points[(i + 1) % n])).collect()
}

/// True when the move passes through any wall.
fn touches(a: V2, b: V2, walls: &[(V2, V2)]) -> bool {
    walls
        .iter()
        .any(|&(c, d)| segment_distance(a, b, c, d) < 1e-9)
}

/// True when a leg keeps its clearance from every wall it could have avoided.
///
/// A travel begins where the previous extrusion ended, which is on a wall, and it
/// ends on one too. A wall that passes through either end of the leg cannot be
/// avoided by any route, so it is exempt; every other wall has to keep `limit`.
fn leg_clear(a: V2, b: V2, walls: &[(V2, V2)], limit: f64) -> bool {
    for &(c, d) in walls {
        if point_segment_distance(a, c, d) < 1e-6 || point_segment_distance(b, c, d) < 1e-6 {
            continue;
        }
        if segment_distance(a, b, c, d) <= limit {
            return false;
        }
    }
    true
}

/// True when every leg of the route keeps off the walls entirely.
fn clear_route(start: V2, via: &[V2], end: V2, walls: &[(V2, V2)]) -> bool {
    let mut from = start;
    for point in via.iter().chain(std::iter::once(&end)) {
        if !leg_clear(from, *point, walls, 1e-9) {
            return false;
        }
        from = *point;
    }
    true
}

/// Shortest distance between two segments, zero when they cross.
fn segment_distance(a: V2, b: V2, c: V2, d: V2) -> f64 {
    if segments_cross(a, b, c, d) {
        return 0.0;
    }
    let mut best = point_segment_distance(a, c, d);
    best = best.min(point_segment_distance(b, c, d));
    best = best.min(point_segment_distance(c, a, b));
    best.min(point_segment_distance(d, a, b))
}

/// True when two segments cross at a point interior to both.
fn segments_cross(a: V2, b: V2, c: V2, d: V2) -> bool {
    let straddles = |p: f64, q: f64| -> bool { (p > 1e-9 && q < -1e-9) || (p < -1e-9 && q > 1e-9) };
    let orient = |p: V2, q: V2, r: V2| -> f64 { (q - p).cross(r - p) };
    straddles(orient(a, b, c), orient(a, b, d)) && straddles(orient(c, d, a), orient(c, d, b))
}

/// Distance from a point to a segment.
fn point_segment_distance(p: V2, a: V2, b: V2) -> f64 {
    let along = b - a;
    let length = along.len2();
    if length < 1e-12 {
        return p.dist(a);
    }
    let t = ((p - a).dot(along) / length).clamp(0.0, 1.0);
    p.dist(a + along * t)
}

/// Offsets a loop by `distance`, positive outward.
///
/// The offset at each vertex follows the two adjacent edges' outward normals,
/// with the miter length capped: at a sharp corner an uncapped miter shoots out
/// by many bead widths, and a route round it would be longer than the wall is
/// worth. The cap turns that corner into a short arc instead.
fn offset(points: &[V2], distance: f64) -> Option<Vec<V2>> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    let turn = signed_area(points);
    if turn.abs() < 1e-9 {
        return None;
    }
    let sign = if turn > 0.0 { -1.0 } else { 1.0 };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let previous = points[(i + n - 1) % n];
        let here = points[i];
        let next = points[(i + 1) % n];
        let before = (here - previous).norm().perp() * sign;
        let after = (next - here).norm().perp() * sign;
        let sum = before + after;
        let denom = 1.0 + before.dot(after);
        if denom.abs() < 1e-6 {
            out.push(here + before * distance);
            continue;
        }
        let mut miter = sum * (distance / denom);
        let cap = distance.abs() * 3.0;
        if miter.len().abs() > cap {
            miter = miter.norm() * cap;
        }
        let point = here + miter;
        if point.x.is_finite() && point.y.is_finite() {
            out.push(point);
        }
    }
    if out.len() < 3 {
        None
    } else {
        Some(out)
    }
}

/// Keeps at most `limit` points, taking every `k`-th so corners stay put.
fn simplify(points: &[V2], limit: usize) -> Vec<V2> {
    if points.len() <= limit {
        return points.to_vec();
    }
    let stride = points.len().div_ceil(limit);
    let mut out = Vec::with_capacity(limit + 1);
    let mut i = 0;
    while i < points.len() {
        out.push(points[i]);
        i += stride;
    }
    out
}

/// Twice the signed area, positive for a counter-clockwise loop.
fn signed_area(points: &[V2]) -> f64 {
    let n = points.len();
    let mut sum = 0.0;
    for i in 0..n {
        sum += points[i].cross(points[(i + 1) % n]);
    }
    sum
}

/// Axis-aligned box of a polygon.
fn bbox(points: &[V2]) -> (f64, f64, f64, f64) {
    let mut box_ = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for point in points {
        box_.0 = box_.0.min(point.x);
        box_.1 = box_.1.min(point.y);
        box_.2 = box_.2.max(point.x);
        box_.3 = box_.3.max(point.y);
    }
    box_
}

/// True when two boxes share any area.
fn boxes_meet(a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64)) -> bool {
    a.2 >= b.0 && b.2 >= a.0 && a.3 >= b.1 && b.3 >= a.1
}

/// The area a route is allowed to wander into, as a box.
///
/// Only loops that stand in the way or sit near the straight move join the graph;
/// the far side of a busy layer is not part of this journey.
fn corridor(start: V2, end: V2, pad: f64) -> (f64, f64, f64, f64) {
    (
        start.x.min(end.x) - pad,
        start.y.min(end.y) - pad,
        start.x.max(end.x) + pad,
        start.y.max(end.y) + pad,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(low: f64, high: f64) -> Vec<V2> {
        vec![
            V2::new(low, low),
            V2::new(high, low),
            V2::new(high, high),
            V2::new(low, high),
        ]
    }

    fn circle(centre: V2, radius: f64, count: usize) -> Vec<V2> {
        (0..count)
            .map(|i| {
                let t = i as f64 / count as f64 * std::f64::consts::TAU;
                V2::new(centre.x + radius * t.cos(), centre.y + radius * t.sin())
            })
            .collect()
    }

    fn cuts(start: V2, via: &[V2], end: V2, loops: &[Vec<V2>]) -> bool {
        let walls = segments(loops);
        !clear_route(start, via, end, &walls)
    }

    #[test]
    fn a_travel_across_a_square_goes_round_it() {
        let loop_ = vec![square(0.0, 10.0)];
        let start = V2::new(-5.0, 5.0);
        let end = V2::new(15.0, 5.0);
        let route = detour(start, end, &loop_, 0.4, 0.0).expect("a way round exists");
        assert!(route.length > start.dist(end), "the route is longer");
        assert!(route.length < 45.0, "and not absurdly so: {}", route.length);
        assert!(!route.via.is_empty(), "it passes through waypoints");
        assert!(
            !cuts(start, &route.via, end, &loop_),
            "the route must not cut the wall"
        );
    }

    #[test]
    fn a_travel_that_does_not_cross_is_left_alone() {
        let loop_ = vec![square(0.0, 10.0)];
        let start = V2::new(-5.0, -5.0);
        let end = V2::new(-5.0, 15.0);
        assert!(detour(start, end, &loop_, 0.4, 0.0).is_none());
    }

    #[test]
    fn a_budget_below_the_needed_detour_refuses() {
        let loop_ = vec![square(0.0, 10.0)];
        let start = V2::new(-5.0, 5.0);
        let end = V2::new(15.0, 5.0);
        assert!(detour(start, end, &loop_, 0.4, 0.0).is_some());
        assert!(
            detour(start, end, &loop_, 0.4, 1.0).is_none(),
            "one millimetre of extra travel cannot clear a ten millimetre wall"
        );
    }

    #[test]
    fn a_route_never_enters_a_wall_on_any_of_forty_pairs() {
        let ring = circle(V2::new(0.0, 0.0), 20.0, 120);
        let loops = vec![ring.clone()];
        let mut routed = 0;
        for i in 0..40 {
            let a = i as f64 / 40.0 * std::f64::consts::TAU;
            let b = a + std::f64::consts::PI * (0.5 + i as f64 / 80.0);
            let start = V2::new(30.0 * a.cos(), 30.0 * a.sin());
            let end = V2::new(30.0 * b.cos(), 30.0 * b.sin());
            if let Some(route) = detour(start, end, &loops, 0.4, 0.0) {
                assert!(
                    !cuts(start, &route.via, end, &loops),
                    "route {i} cuts the ring it was asked to go round"
                );
                assert!(route.length.is_finite());
                routed += 1;
            }
        }
        assert!(
            routed > 30,
            "most of the forty pairs needed a route: {routed}"
        );
    }

    #[test]
    fn a_large_loop_is_simplified_rather_than_refused() {
        let big = circle(V2::new(0.0, 0.0), 20.0, 4000);
        let simple = simplify(&big, MAX_VERTICES_PER_LOOP);
        assert!(simple.len() <= MAX_VERTICES_PER_LOOP);
        assert!(offset(&simple, 0.44).is_some());
    }

    #[test]
    fn an_annulus_route_stays_in_the_band() {
        let outer = circle(V2::new(0.0, 0.0), 17.0, 200);
        let inner = circle(V2::new(0.0, 0.0), 7.0, 100);
        let loops = vec![outer, inner];
        let start = V2::new(-12.0, 0.0);
        let end = V2::new(12.0, 0.0);
        let route = detour(start, end, &loops, 0.4, 0.0).expect("a way round the hole exists");
        assert!(!cuts(start, &route.via, end, &loops));
        assert!(route.length > start.dist(end));
    }
}
