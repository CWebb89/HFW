//! Cutting a mesh: a plane through it, and its separate shells.
//!
//! A plane cut is exact where it can be and conservative where it cannot. Each triangle
//! is clipped against the plane by Sutherland–Hodgman, which keeps a triangle's crossing
//! polygon convex, so the pieces need only a fan. The points the plane introduces are
//! snapped to the coordinate grid like every other point in the engine, and a triangle
//! the snap collapses is dropped rather than kept with no area.
//!
//! The cap is the part that makes a half watertight: the segments the plane cut out of
//! the triangles are walked into closed rings, the rings are nested by containment, and
//! each boundary with its holes is capped by `hfw_kernel`'s ear clipper. A mesh that is
//! not closed can leave a segment unmatched, and then the half is returned without a cap
//! — a hole where the input already had one, rather than a guess.

use crate::{Mesh, MeshError, Point3, Triangle};
use hfw_kernel::cap;
use hfw_kernel::V2;
use std::collections::BTreeMap;

/// How far a point may sit from the plane and still count as on it, in millimetres.
const EPSILON: f64 = 1e-9;

/// A plane, as a unit normal and a distance along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutPlane {
    /// Which way the plane faces. Normalised on use.
    pub normal: [f64; 3],
    /// How far along the normal the plane sits, in millimetres.
    pub offset: f64,
}

/// A plane through a point, facing the given normal.
impl CutPlane {
    /// A plane through `point`, facing `normal`, in millimetres.
    pub fn through(normal: [f64; 3], point: [f64; 3]) -> Result<Self, MeshError> {
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if !length.is_finite() || length < 1e-9 {
            return Err(MeshError::InvalidTransform(
                "a cut plane needs a normal with a direction".to_owned(),
            ));
        }
        let unit = [normal[0] / length, normal[1] / length, normal[2] / length];
        let offset = unit[0] * point[0] + unit[1] * point[1] + unit[2] * point[2];
        Ok(Self {
            normal: unit,
            offset,
        })
    }

    /// The signed distance of a point from the plane.
    fn distance(&self, point: [f64; 3]) -> f64 {
        self.normal[0] * point[0] + self.normal[1] * point[1] + self.normal[2] * point[2]
            - self.offset
    }

    /// Projects a point onto the plane, which is where the cut pieces meet.
    fn project(&self, point: [f64; 3]) -> [f64; 3] {
        let distance = self.distance(point);
        [
            point[0] - distance * self.normal[0],
            point[1] - distance * self.normal[1],
            point[2] - distance * self.normal[2],
        ]
    }

    /// Two unit vectors spanning the plane, with `u × v` equal to the normal.
    fn basis(&self) -> ([f64; 3], [f64; 3]) {
        let n = self.normal;
        let helper = if n[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let u = cross(n, helper);
        let u = normalise(u);
        let v = cross(n, u);
        (u, normalise(v))
    }

    /// A point's plane coordinates.
    fn planar(&self, point: [f64; 3], basis: ([f64; 3], [f64; 3])) -> V2 {
        let (u, v) = basis;
        V2::new(
            point[0] * u[0] + point[1] * u[1] + point[2] * u[2],
            point[0] * v[0] + point[1] * v[1] + point[2] * v[2],
        )
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalise(v: [f64; 3]) -> [f64; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length < 1e-12 {
        return [0.0, 0.0, 0.0];
    }
    [v[0] / length, v[1] / length, v[2] / length]
}

/// The pieces a plane cut leaves, each watertight where the input was.
#[derive(Clone, Debug, Default)]
pub struct Cut {
    /// Everything at or below the plane, in the plane's own negative half.
    pub below: Mesh,
    /// Everything at or above the plane.
    pub above: Mesh,
    /// Whether both halves were capped. False when the input was not closed.
    pub capped: bool,
}

impl Mesh {
    /// Cuts the mesh with a plane, capping both halves.
    pub fn cut(&self, plane: CutPlane) -> Result<Cut, MeshError> {
        let mut below = Builder::default();
        let mut above = Builder::default();
        let mut segments = Vec::new();
        for triangle in self.triangles() {
            let corners = [
                self.vertices()[triangle.0[0] as usize],
                self.vertices()[triangle.0[1] as usize],
                self.vertices()[triangle.0[2] as usize],
            ];
            let positions = [corners[0].to_mm(), corners[1].to_mm(), corners[2].to_mm()];
            let distances = positions.map(|point| plane.distance(point));
            let straddles =
                distances.iter().any(|d| *d > EPSILON) && distances.iter().any(|d| *d < -EPSILON);
            if straddles {
                let snap = |point: [f64; 3]| plane.project(point);
                below.push_polygon(&clip(&positions, &distances, false, &snap))?;
                above.push_polygon(&clip(&positions, &distances, true, &snap))?;
                if let Some(segment) = plane_crossing(&positions, &distances, &snap) {
                    segments.push(segment);
                }
                continue;
            }
            if distances.iter().all(|d| *d > -EPSILON) && distances.iter().any(|d| *d > EPSILON) {
                above.push_triangle(corners);
            } else {
                below.push_triangle(corners);
            }
        }
        let capped = self.cap_halves(plane, &segments, &mut below, &mut above);
        Ok(Cut {
            below: below.finish()?,
            above: above.finish()?,
            capped,
        })
    }

    /// Splits the mesh into its separate connected shells, in the order their first
    /// triangle appears.
    pub fn parts(&self) -> Result<Vec<Mesh>, MeshError> {
        let count = self.triangle_count();
        let mut parent: Vec<usize> = (0..count).collect();
        fn root(parent: &mut [usize], index: usize) -> usize {
            let mut current = index;
            while parent[current] != current {
                current = parent[current];
            }
            parent[index] = current;
            current
        }
        let mut first_triangle_by_vertex = BTreeMap::<u32, usize>::new();
        for (index, triangle) in self.triangles().iter().enumerate() {
            for vertex in triangle.0 {
                match first_triangle_by_vertex.get(&vertex) {
                    Some(other) => {
                        let (left, right) = (root(&mut parent, index), root(&mut parent, *other));
                        if left != right {
                            parent[left] = right;
                        }
                    }
                    None => {
                        first_triangle_by_vertex.insert(vertex, index);
                    }
                }
            }
        }
        let mut built: BTreeMap<usize, Builder> = BTreeMap::new();
        for (index, triangle) in self.triangles().iter().enumerate() {
            let owner = root(&mut parent, index);
            let builder = built.entry(owner).or_default();
            let corners = triangle.0.map(|vertex| self.vertices()[vertex as usize]);
            builder.push_triangle(corners);
        }
        built
            .into_values()
            .map(|builder| builder.finish())
            .collect()
    }

    /// Caps both halves from the segments the plane cut out, and reports whether every
    /// segment found a ring to belong to.
    fn cap_halves(
        &self,
        plane: CutPlane,
        segments: &[([f64; 3], [f64; 3])],
        below: &mut Builder,
        above: &mut Builder,
    ) -> bool {
        let rings = match rings(segments) {
            Some(rings) => rings,
            None => return false,
        };
        let basis = plane.basis();
        let mut triangles = Vec::new();
        for (outer, holes) in shells(&rings, plane, basis) {
            let Some(capped) = cap::triangulate(&outer, &holes) else {
                return false;
            };
            for triangle in capped {
                triangles.push(triangle.map(|point| plane.point_at(point, basis)));
            }
        }
        for triangle in &triangles {
            below.push_cap(*triangle, true);
            above.push_cap(*triangle, false);
        }
        true
    }
}

impl CutPlane {
    /// The millimetre point of a plane coordinate on this plane.
    fn point_at(&self, point: V2, basis: ([f64; 3], [f64; 3])) -> [f64; 3] {
        let (u, v) = basis;
        [
            self.offset * self.normal[0] + point.x * u[0] + point.y * v[0],
            self.offset * self.normal[1] + point.x * u[1] + point.y * v[1],
            self.offset * self.normal[2] + point.x * u[2] + point.y * v[2],
        ]
    }
}

/// Clips a triangle to one side of the plane, keeping the side `positive` names.
fn clip(
    positions: &[[f64; 3]; 3],
    distances: &[f64; 3],
    positive: bool,
    snap: &impl Fn([f64; 3]) -> [f64; 3],
) -> Vec<[f64; 3]> {
    let inside = |distance: f64| {
        if positive {
            distance >= 0.0
        } else {
            distance <= 0.0
        }
    };
    let mut polygon = Vec::with_capacity(4);
    for index in 0..3 {
        let next = (index + 1) % 3;
        if inside(distances[index]) {
            polygon.push(positions[index]);
        }
        if inside(distances[index]) != inside(distances[next]) {
            polygon.push(intersection(
                positions[index],
                positions[next],
                distances[index],
                distances[next],
                snap,
            ));
        }
    }
    polygon
}

/// Where an edge crosses the plane, projected exactly onto it.
fn intersection(
    from: [f64; 3],
    to: [f64; 3],
    from_distance: f64,
    to_distance: f64,
    snap: &impl Fn([f64; 3]) -> [f64; 3],
) -> [f64; 3] {
    let span = from_distance - to_distance;
    let t = if span.abs() < f64::MIN_POSITIVE {
        0.0
    } else {
        (from_distance / span).clamp(0.0, 1.0)
    };
    snap([
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
    ])
}

/// The segment a triangle contributes to the cap, or `None` when it does not touch the
/// plane along a length.
fn plane_crossing(
    positions: &[[f64; 3]; 3],
    distances: &[f64; 3],
    snap: &impl Fn([f64; 3]) -> [f64; 3],
) -> Option<([f64; 3], [f64; 3])> {
    let epsilon = EPSILON;
    let mut points: Vec<[f64; 3]> = Vec::with_capacity(2);
    for index in 0..3 {
        let next = (index + 1) % 3;
        if distances[index].abs() <= epsilon {
            points.push(snap(positions[index]));
        }
        let straddles = (distances[index] > epsilon && distances[next] < -epsilon)
            || (distances[index] < -epsilon && distances[next] > epsilon);
        if straddles {
            points.push(intersection(
                positions[index],
                positions[next],
                distances[index],
                distances[next],
                snap,
            ));
        }
    }
    let mut unique: Vec<[f64; 3]> = Vec::with_capacity(2);
    for point in points {
        let exists = unique.iter().any(|known| key(*known) == key(point));
        if !exists {
            unique.push(point);
        }
    }
    match unique.len() {
        2 => Some((unique[0], unique[1])),
        _ => None,
    }
}

/// A quantised key, so two points a floating-point hair apart are the same point.
fn key(point: [f64; 3]) -> [i64; 3] {
    point.map(|value| (value * 1_000_000.0).round() as i64)
}

/// Walks the cut segments into closed rings. `None` when any end is unmatched, which
/// means the input was not closed along the plane.
fn rings(segments: &[([f64; 3], [f64; 3])]) -> Option<Vec<Vec<[f64; 3]>>> {
    if segments.is_empty() {
        return Some(Vec::new());
    }
    let mut by_key: BTreeMap<[i64; 3], Vec<usize>> = BTreeMap::new();
    for (index, (from, to)) in segments.iter().enumerate() {
        by_key.entry(key(*from)).or_default().push(index);
        by_key.entry(key(*to)).or_default().push(index);
    }
    if by_key.values().any(|touching| touching.len() != 2) {
        return None;
    }
    let mut used = vec![false; segments.len()];
    let mut rings = Vec::new();
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut ring = vec![segments[start].0, segments[start].1];
        let mut current = segments[start].1;
        loop {
            let next = by_key
                .get(&key(current))?
                .iter()
                .copied()
                .find(|index| !used[*index]);
            let Some(next) = next else {
                break;
            };
            used[next] = true;
            let (from, to) = segments[next];
            let forward = key(from) == key(current);
            current = if forward { to } else { from };
            if key(current) == key(ring[0]) {
                break;
            }
            ring.push(current);
        }
        if ring.len() >= 3 {
            rings.push(ring);
        } else {
            return None;
        }
    }
    Some(rings)
}

/// Groups the cut rings into a boundary and its holes, by nesting depth.
fn shells(
    rings: &[Vec<[f64; 3]>],
    plane: CutPlane,
    basis: ([f64; 3], [f64; 3]),
) -> Vec<(Vec<V2>, Vec<Vec<V2>>)> {
    let planar: Vec<Vec<V2>> = rings
        .iter()
        .map(|ring| {
            ring.iter()
                .map(|point| plane.planar(*point, basis))
                .collect()
        })
        .collect();
    let areas: Vec<f64> = planar.iter().map(|ring| ring_area(ring).abs()).collect();
    let mut order: Vec<usize> = (0..planar.len()).collect();
    order.sort_by(|left, right| {
        areas[*right]
            .partial_cmp(&areas[*left])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.cmp(right))
    });
    let mut depth = vec![0_usize; planar.len()];
    let mut owner = vec![None; planar.len()];
    for (position, index) in order.iter().enumerate() {
        for smaller in order[position + 1..].iter() {
            if ring_contains(&planar[*index], &planar[*smaller]) {
                depth[*smaller] = depth[*smaller].max(depth[*index] + 1);
                owner[*smaller] = Some(*index);
            }
        }
    }
    let mut shells: Vec<(usize, Vec<V2>, Vec<Vec<V2>>)> = Vec::new();
    for index in 0..planar.len() {
        if depth[index] % 2 == 0 {
            shells.push((index, planar[index].clone(), Vec::new()));
        }
    }
    for index in 0..planar.len() {
        if depth[index] % 2 == 1 {
            let Some(outer) = owner[index] else {
                continue;
            };
            if let Some(entry) = shells.iter_mut().find(|(root, _, _)| *root == outer) {
                entry.2.push(planar[index].clone());
            }
        }
    }
    shells
        .into_iter()
        .map(|(_, outer, holes)| (outer, holes))
        .collect()
}

fn ring_area(ring: &[V2]) -> f64 {
    let mut sum = 0.0;
    for (index, point) in ring.iter().enumerate() {
        let next = ring[(index + 1) % ring.len()];
        sum += point.x * next.y - next.x * point.y;
    }
    sum / 2.0
}

fn ring_contains(outer: &[V2], point: &[V2]) -> bool {
    let Some(probe) = point.first() else {
        return false;
    };
    let mut inside = false;
    let mut previous = outer.len() - 1;
    for index in 0..outer.len() {
        let (a, b) = (outer[index], outer[previous]);
        if (a.y > probe.y) != (b.y > probe.y) {
            let t = (probe.y - a.y) / (b.y - a.y);
            if probe.x < a.x + t * (b.x - a.x) {
                inside = !inside;
            }
        }
        previous = index;
    }
    inside
}

/// Builds a half: its pieces keep the input's winding, its caps are oriented away from
/// the material they close.
#[derive(Default)]
struct Builder {
    vertices: Vec<Point3>,
    triangles: Vec<Triangle>,
    index_by_vertex: BTreeMap<Point3, u32>,
}

impl Builder {
    fn vertex(&mut self, point: Point3) -> Result<u32, MeshError> {
        if let Some(index) = self.index_by_vertex.get(&point) {
            return Ok(*index);
        }
        let index = u32::try_from(self.vertices.len()).map_err(|_| MeshError::TooManyVertices)?;
        self.vertices.push(point);
        self.index_by_vertex.insert(point, index);
        Ok(index)
    }

    fn push_polygon(&mut self, polygon: &[[f64; 3]]) -> Result<(), MeshError> {
        if polygon.len() < 3 {
            return Ok(());
        }
        let first = self.point(polygon[0])?;
        for corner in 1..polygon.len() - 1 {
            let second = self.point(polygon[corner])?;
            let third = self.point(polygon[corner + 1])?;
            self.push_indices(first, second, third);
        }
        Ok(())
    }

    fn push_triangle(&mut self, corners: [Point3; 3]) {
        if let Ok(indices) = self.triangle_of(corners) {
            self.triangles.push(indices);
        }
    }

    fn push_cap(&mut self, corners: [[f64; 3]; 3], below: bool) {
        let Ok(a) = self.point(corners[0]) else {
            return;
        };
        let Ok(b) = self.point(corners[1]) else {
            return;
        };
        let Ok(c) = self.point(corners[2]) else {
            return;
        };
        if below {
            self.push_indices(a, b, c);
        } else {
            self.push_indices(a, c, b);
        }
    }

    fn push_indices(&mut self, a: u32, b: u32, c: u32) {
        if a == b || b == c || a == c {
            return;
        }
        self.triangles.push(Triangle([a, b, c]));
    }

    fn triangle_of(&mut self, corners: [Point3; 3]) -> Result<Triangle, MeshError> {
        let a = self.vertex(corners[0])?;
        let b = self.vertex(corners[1])?;
        let c = self.vertex(corners[2])?;
        Ok(Triangle([a, b, c]))
    }

    fn point(&mut self, point: [f64; 3]) -> Result<u32, MeshError> {
        let snapped = Point3::from_mm(point[0], point[1], point[2])?;
        self.vertex(snapped)
    }

    fn finish(self) -> Result<Mesh, MeshError> {
        Mesh::new(self.vertices, self.triangles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Mesh {
        Mesh::box_mm(20.0, 20.0, 20.0).expect("a cube")
    }

    fn volume(mesh: &Mesh) -> f64 {
        mesh.volume_mm3()
    }

    #[test]
    fn a_plane_through_a_cube_makes_two_capped_halves() {
        let cube = cube();
        let plane = CutPlane::through([0.0, 0.0, 1.0], [0.0, 0.0, 10.0]).expect("a plane");
        let cut = cube.cut(plane).expect("the cube cuts");
        assert!(cut.capped, "a closed cube caps both halves");
        assert!(
            (volume(&cut.below) - 4000.0).abs() < 2.0,
            "{}",
            volume(&cut.below)
        );
        assert!(
            (volume(&cut.above) - 4000.0).abs() < 2.0,
            "{}",
            volume(&cut.above)
        );
        let below = cut.below.diagnostics();
        assert_eq!(below.boundary_edges, 0, "the lower half is closed");
        assert_eq!(below.inconsistently_oriented_edges, 0);
        let above = cut.above.diagnostics();
        assert_eq!(above.boundary_edges, 0, "the upper half is closed");
        assert_eq!(above.inconsistently_oriented_edges, 0);
    }

    #[test]
    fn an_oblique_cut_keeps_the_volume_it_started_with() {
        let cube = cube();
        let plane = CutPlane::through([1.0, 1.0, 1.0], [10.0, 10.0, 10.0]).expect("a plane");
        let cut = cube.cut(plane).expect("the cube cuts");
        assert!(cut.capped);
        let whole = volume(&cut.below) + volume(&cut.above);
        assert!(
            (whole - 8000.0).abs() < 8.0,
            "the halves add up to the cube: {whole}"
        );
        assert_eq!(cut.below.diagnostics().boundary_edges, 0);
    }

    #[test]
    fn a_cut_that_misses_the_mesh_leaves_one_side_empty() {
        let cube = cube();
        let plane = CutPlane::through([0.0, 0.0, 1.0], [0.0, 0.0, 40.0]).expect("a plane");
        let cut = cube.cut(plane).expect("the cube cuts");
        assert!(cut.capped);
        assert_eq!(cut.above.triangle_count(), 0);
        assert_eq!(cut.below.triangle_count(), 12);
        assert!((volume(&cut.below) - 8000.0).abs() < 2.0);
    }

    #[test]
    fn a_plane_through_a_face_leaves_no_cap() {
        let cube = cube();
        let plane = CutPlane::through([0.0, 0.0, 1.0], [0.0, 0.0, 20.0]).expect("a plane");
        let cut = cube.cut(plane).expect("the cube cuts");
        assert_eq!(cut.below.triangle_count(), 12, "the top face is the plane");
        assert!(cut.capped, "a plane that only touches a face needs no cap");
        assert_eq!(cut.above.triangle_count(), 0);
    }

    /// A hollow square tube: outer 20 mm, inner 8 mm, 10 mm tall, watertight.
    fn tube() -> Mesh {
        let outer = [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)];
        let inner = [(8.0, 8.0), (12.0, 8.0), (12.0, 12.0), (8.0, 12.0)];
        let mut vertices = Vec::new();
        for (ring, z) in [(outer, 0.0), (inner, 0.0), (outer, 10.0), (inner, 10.0)] {
            for (x, y) in ring {
                vertices.push([x, y, z]);
            }
        }
        let (o, i, top_o, top_i) = (0_usize, 4_usize, 8_usize, 12_usize);
        let mut soup = Vec::new();
        let mut quad = |a: usize, b: usize, c: usize, d: usize| {
            soup.push([vertices[a], vertices[b], vertices[c]]);
            soup.push([vertices[a], vertices[c], vertices[d]]);
        };
        for k in 0..4 {
            let next = (k + 1) % 4;
            quad(o + k, i + k, i + next, o + next);
            quad(top_o + k, top_o + next, top_i + next, top_i + k);
            quad(o + k, o + next, top_o + next, top_o + k);
            quad(i + k, top_i + k, top_i + next, i + next);
        }
        Mesh::from_triangle_soup(&soup).expect("a hollow tube")
    }

    #[test]
    fn a_cut_through_a_hole_caps_the_hole_too() {
        let tube = tube();
        let report = tube.diagnostics();
        assert_eq!(report.boundary_edges, 0, "the tube is closed");
        assert_eq!(
            report.inconsistently_oriented_edges, 0,
            "and consistently wound"
        );
        assert!(
            (volume(&tube) - 3840.0).abs() < 1e-6,
            "the tube holds 20² − 8² by 10 mm: {}",
            volume(&tube)
        );
        let plane = CutPlane::through([0.0, 0.0, 1.0], [0.0, 0.0, 5.0]).expect("a plane");
        let cut = tube.cut(plane).expect("the tube cuts");
        assert!(cut.capped, "both halves cap");
        assert_eq!(cut.below.diagnostics().boundary_edges, 0);
        assert_eq!(cut.above.diagnostics().boundary_edges, 0);
        assert_eq!(cut.below.diagnostics().inconsistently_oriented_edges, 0);
        assert_eq!(cut.above.diagnostics().inconsistently_oriented_edges, 0);
        let half = 3840.0 / 2.0;
        assert!(
            (volume(&cut.below) - half).abs() < 1e-6,
            "{}",
            volume(&cut.below)
        );
        assert!(
            (volume(&cut.above) - half).abs() < 1e-6,
            "{}",
            volume(&cut.above)
        );
    }

    #[test]
    fn separate_shells_split_into_parts() {
        let left = Mesh::box_mm(2.0, 2.0, 2.0).expect("a box");
        let right = Mesh::transformed(
            &Mesh::box_mm(3.0, 3.0, 3.0).expect("a box"),
            crate::MeshTransform {
                translation_mm: [10.0, 0.0, 0.0],
                ..crate::MeshTransform::default()
            },
        )
        .expect("moved");
        let joined = Mesh::merged(&[left, right]).expect("merged");
        let parts = joined.parts().expect("the shells split");
        assert_eq!(parts.len(), 2);
        let volumes: Vec<f64> = parts.iter().map(volume).collect();
        assert!((volumes[0] - 8.0).abs() < 1e-6, "{volumes:?}");
        assert!((volumes[1] - 27.0).abs() < 1e-6, "{volumes:?}");
    }
}
