//! Capping a region: turning one ring and its holes into triangles.
//!
//! A cap only has to tile its region exactly, but it must not invent boundary: every
//! vertex of the rings has to stay a vertex of the cap, or the cap and the walls beside it
//! meet along a T-junction and the half is not closed. That rules out a grid, a band sweep
//! or any other decomposition that cuts the boundary at heights of its own choosing.
//!
//! So: bridge each hole into the outer ring along a segment that crosses nothing, then
//! clip ears off the joined ring. A bridge is a slit — two coincident edges traversed in
//! opposite directions — and it adds no coordinate that the rings did not already have.
//!
//! An ear is only taken when it is anticlockwise, holds no other ring point, and no ring
//! edge passes through it: with collinear points on the boundary, an ear that spans a slit
//! can lie outside the region while no ring point falls inside it.

use crate::geom::V2;

/// A triangle of a cap, three corners, anticlockwise in the ring's own plane.
pub type CapTriangle = [V2; 3];

/// Triangulates `outer` and the holes inside it. Ring direction does not matter: the outer
/// ring is turned anticlockwise and each hole clockwise, which is what makes a hole
/// subtract rather than add.
///
/// Returns `None` when a ring has fewer than three points, a hole cannot be bridged, or
/// the joined ring cannot be ear clipped, which is the honest answer for a ring that
/// crosses itself.
pub fn triangulate(outer: &[V2], holes: &[Vec<V2>]) -> Option<Vec<CapTriangle>> {
    if outer.len() < 3 {
        return None;
    }
    let mut ring = outer.to_vec();
    if !anticlockwise(&ring) {
        ring.reverse();
    }
    let mut sorted: Vec<Vec<V2>> = holes
        .iter()
        .filter(|hole| hole.len() >= 3)
        .map(|hole| {
            let mut hole = hole.clone();
            if anticlockwise(&hole) {
                hole.reverse();
            }
            hole
        })
        .collect();
    sorted.sort_by(|left, right| {
        let left = left.iter().map(|point| point.x).fold(f64::MIN, f64::max);
        let right = right.iter().map(|point| point.x).fold(f64::MIN, f64::max);
        right
            .partial_cmp(&left)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for hole in &sorted {
        ring = bridge(&ring, hole)?;
    }
    if !anticlockwise(&ring) {
        ring.reverse();
    }
    ear_clip(&ring)
}

/// Clips ears off a ring until three corners are left.
fn ear_clip(ring: &[V2]) -> Option<Vec<CapTriangle>> {
    let mut triangles = Vec::new();
    let mut indices: Vec<usize> = (0..ring.len()).collect();
    let mut guard = 0;
    while indices.len() > 3 {
        guard += 1;
        if guard > ring.len() * ring.len() + 8 {
            return None;
        }
        let mut clipped = false;
        for position in 0..indices.len() {
            let previous = indices[(position + indices.len() - 1) % indices.len()];
            let current = indices[position];
            let next = indices[(position + 1) % indices.len()];
            let triangle = [ring[previous], ring[current], ring[next]];
            if !anticlockwise(&triangle) {
                continue;
            }
            let blocked = indices.iter().any(|candidate| {
                if *candidate == previous || *candidate == current || *candidate == next {
                    return false;
                }
                let point = ring[*candidate];
                if triangle.iter().any(|corner| same(*corner, point)) {
                    return false;
                }
                inside_triangle(point, &triangle)
            });
            if blocked || crosses_ring(ring, &indices, &triangle, previous, current, next) {
                continue;
            }
            triangles.push(triangle);
            indices.remove(position);
            clipped = true;
            break;
        }
        if !clipped {
            return None;
        }
    }
    if indices.len() == 3 {
        let mut last = [ring[indices[0]], ring[indices[1]], ring[indices[2]]];
        if !anticlockwise(&last) {
            last.swap(1, 2);
        }
        if side(last[0], last[1], last[2]) != 0.0 {
            triangles.push(last);
        }
    }
    Some(triangles)
}

/// True when a ring edge passes through the ear's interior.
fn crosses_ring(
    ring: &[V2],
    indices: &[usize],
    triangle: &[V2; 3],
    previous: usize,
    current: usize,
    next: usize,
) -> bool {
    for (position, start) in indices.iter().enumerate() {
        let end = indices[(position + 1) % indices.len()];
        if [previous, current, next].contains(start) || [previous, current, next].contains(&end) {
            continue;
        }
        let (a, b) = (ring[*start], ring[end]);
        for corner in 0..3 {
            let (c, d) = (triangle[corner], triangle[(corner + 1) % 3]);
            if crosses(a, b, c, d) {
                return true;
            }
        }
    }
    false
}

/// Inserts a hole into the boundary along the shortest bridge that crosses nothing.
fn bridge(outer: &[V2], hole: &[V2]) -> Option<Vec<V2>> {
    let start = (0..hole.len()).max_by(|a, b| {
        hole[*a]
            .x
            .partial_cmp(&hole[*b].x)
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let mut best: Option<(usize, f64)> = None;
    for index in 0..outer.len() {
        let distance = outer[index].dist(hole[start]).powi(2);
        if best.map(|(_, known)| distance < known).unwrap_or(true)
            && !blocked(outer, hole, index, start)
        {
            best = Some((index, distance));
        }
    }
    let (index, _) = best?;
    let mut ring = Vec::with_capacity(outer.len() + hole.len() + 2);
    ring.extend_from_slice(&outer[..=index]);
    for step in 0..hole.len() {
        ring.push(hole[(start + step) % hole.len()]);
    }
    ring.push(hole[start]);
    ring.extend_from_slice(&outer[index..]);
    Some(ring)
}

/// True when the bridge from `outer[index]` to `hole[start]` crosses an edge of either.
fn blocked(outer: &[V2], hole: &[V2], index: usize, start: usize) -> bool {
    let a = outer[index];
    let b = hole[start];
    for ring in [outer, hole] {
        for (position, begin) in ring.iter().enumerate() {
            let end = ring[(position + 1) % ring.len()];
            if same(*begin, a) || same(*begin, b) || same(end, a) || same(end, b) {
                continue;
            }
            if crosses(a, b, *begin, end) {
                return true;
            }
        }
    }
    false
}

fn same(a: V2, b: V2) -> bool {
    a.x == b.x && a.y == b.y
}

fn side(p: V2, q: V2, r: V2) -> f64 {
    (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
}

fn crosses(a: V2, b: V2, c: V2, d: V2) -> bool {
    let d1 = side(a, b, c);
    let d2 = side(a, b, d);
    let d3 = side(c, d, a);
    let d4 = side(c, d, b);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn inside_triangle(point: V2, triangle: &[V2; 3]) -> bool {
    side(triangle[0], triangle[1], point) >= 0.0
        && side(triangle[1], triangle[2], point) >= 0.0
        && side(triangle[2], triangle[0], point) >= 0.0
}

fn anticlockwise(ring: &[V2]) -> bool {
    let mut sum = 0.0;
    for (index, point) in ring.iter().enumerate() {
        let next = ring[(index + 1) % ring.len()];
        sum += point.x * next.y - next.x * point.y;
    }
    sum > 0.0
}

#[cfg(test)]
mod holes {
    use crate::geom::{Region, V2};

    fn square(x0: f64, x1: f64) -> Vec<V2> {
        vec![
            V2::new(x0, x0),
            V2::new(x1, x0),
            V2::new(x1, x1),
            V2::new(x0, x1),
        ]
    }

    #[test]
    fn filling_holes_leaves_the_outer_boundary_and_its_area() {
        let ring = Region::new(vec![square(0.0, 10.0), square(4.0, 6.0)]);
        assert_eq!(ring.rings.len(), 2);
        assert!((ring.area() - 96.0).abs() < 1e-9, "area {}", ring.area());
        let filled = ring.clone().with_holes_filled();
        assert_eq!(filled.rings.len(), 1, "the hole ring is gone");
        assert!(
            (filled.area() - 100.0).abs() < 1e-9,
            "area {}",
            filled.area()
        );
        assert!(filled.contains(V2::new(5.0, 5.0)), "the hole now prints");
    }

    #[test]
    fn an_island_inside_a_hole_stays_solid_through_the_fill() {
        let ring = Region::new(vec![square(0.0, 10.0), square(2.0, 8.0), square(4.0, 6.0)]);
        assert_eq!(ring.rings.len(), 3);
        let filled = ring.with_holes_filled();
        assert_eq!(
            filled.rings.len(),
            1,
            "the hole went, and the island inside it is part of the same solid now"
        );
        assert!(
            (filled.area() - 100.0).abs() < 1e-9,
            "area {}",
            filled.area()
        );
        assert!(filled.contains(V2::new(3.0, 3.0)), "the filled hole prints");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(points: &[(f64, f64)]) -> Vec<V2> {
        points.iter().map(|(x, y)| V2::new(*x, *y)).collect()
    }

    fn area(triangles: &[CapTriangle]) -> f64 {
        triangles
            .iter()
            .map(|triangle| {
                let [a, b, c] = *triangle;
                ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)) / 2.0
            })
            .sum()
    }

    fn corners(triangles: &[CapTriangle]) -> Vec<(f64, f64)> {
        let mut corners: Vec<(f64, f64)> = triangles
            .iter()
            .flatten()
            .map(|point| (point.x, point.y))
            .collect();
        corners.sort_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    left.1
                        .partial_cmp(&right.1)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });
        corners.dedup();
        corners
    }

    #[test]
    fn a_square_caps_to_its_own_area() {
        let outer = ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let triangles = triangulate(&outer, &[]).expect("a square triangulates");
        assert!(
            (area(&triangles) - 100.0).abs() < 1e-9,
            "{}",
            area(&triangles)
        );
    }

    #[test]
    fn a_hole_is_removed_whatever_direction_it_is_stored_in() {
        let outer = ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let hole = ring(&[(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0)]);
        let triangles =
            triangulate(&outer, std::slice::from_ref(&hole)).expect("a square with a hole");
        assert!(
            (area(&triangles) - 84.0).abs() < 1e-9,
            "{}",
            area(&triangles)
        );
        let reversed: Vec<V2> = hole.into_iter().rev().collect();
        let triangles = triangulate(&outer, &[reversed]).expect("the same, wound the other way");
        assert!(
            (area(&triangles) - 84.0).abs() < 1e-9,
            "{}",
            area(&triangles)
        );
    }

    #[test]
    fn collinear_points_on_both_rings_cap_without_overlapping() {
        let outer = ring(&[
            (0.0, 0.0),
            (10.0, 0.0),
            (20.0, 0.0),
            (20.0, 10.0),
            (20.0, 20.0),
            (10.0, 20.0),
            (0.0, 20.0),
            (0.0, 10.0),
        ]);
        let hole = ring(&[
            (8.0, 8.0),
            (10.0, 8.0),
            (12.0, 8.0),
            (12.0, 10.0),
            (12.0, 12.0),
            (10.0, 12.0),
            (8.0, 12.0),
            (8.0, 10.0),
        ]);
        let triangles = triangulate(&outer, &[hole]).expect("both rings cap");
        assert!(
            (area(&triangles) - 384.0).abs() < 1e-9,
            "{}",
            area(&triangles)
        );
    }

    #[test]
    fn two_holes_are_removed() {
        let outer = ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let left = ring(&[(2.0, 2.0), (4.0, 2.0), (4.0, 4.0), (2.0, 4.0)]);
        let right = ring(&[(6.0, 6.0), (8.0, 6.0), (8.0, 8.0), (6.0, 8.0)]);
        let triangles = triangulate(&outer, &[left, right]).expect("two holes");
        assert!(
            (area(&triangles) - 92.0).abs() < 1e-9,
            "{}",
            area(&triangles)
        );
    }

    #[test]
    fn a_clockwise_outer_ring_caps_the_same_way() {
        let outer = ring(&[(0.0, 10.0), (10.0, 10.0), (10.0, 0.0), (0.0, 0.0)]);
        let hole = ring(&[(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0)]);
        let triangles = triangulate(&outer, &[hole]).expect("a clockwise square with a hole");
        assert!(
            (area(&triangles) - 84.0).abs() < 1e-9,
            "{}",
            area(&triangles)
        );
    }

    #[test]
    fn a_concave_ring_caps_without_inventing_area() {
        let outer = ring(&[
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 4.0),
            (4.0, 4.0),
            (4.0, 10.0),
            (0.0, 10.0),
        ]);
        let triangles = triangulate(&outer, &[]).expect("an L triangulates");
        assert!(
            (area(&triangles) - 64.0).abs() < 1e-9,
            "{}",
            area(&triangles)
        );
    }

    #[test]
    fn every_ring_vertex_is_a_cap_vertex() {
        let outer = ring(&[
            (0.0, 0.0),
            (10.0, 0.0),
            (20.0, 0.0),
            (20.0, 10.0),
            (20.0, 20.0),
            (10.0, 20.0),
            (0.0, 20.0),
            (0.0, 10.0),
        ]);
        let hole = ring(&[(8.0, 8.0), (12.0, 8.0), (12.0, 12.0), (8.0, 12.0)]);
        let triangles =
            triangulate(&outer, std::slice::from_ref(&hole)).expect("a ring with a hole");
        let corners = corners(&triangles);
        for point in outer.iter().chain(hole.iter()) {
            assert!(
                corners.contains(&(point.x, point.y)),
                "{} {} is missing from the cap",
                point.x,
                point.y
            );
        }
    }
}
