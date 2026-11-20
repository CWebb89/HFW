//! Indexed triangle mesh storage and basic structural validation.
//!
//! STL parsing, 3MF import and optional repair are explicit operations kept
//! separate from the canonical in-memory mesh representation.

#![forbid(unsafe_code)]

mod threemf;

pub use threemf::{
    archive_parts, from_3mf_reader, from_3mf_text, load_3mf, load_3mf_objects,
    metadata_from_reader, model_part_xml, objects_from_3mf_text, three_mf_metadata, write_archive,
    ModelObject,
};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;

use hfw_geometry::{mm_to_coord, Coord, GeometryError, MAX_ABS_COORD, UNITS_PER_MM};

/// A vertex in printer coordinates (Z up), in integer nanometres.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point3 {
    /// X coordinate.
    pub x: Coord,
    /// Y coordinate.
    pub y: Coord,
    /// Z coordinate.
    pub z: Coord,
}

impl Point3 {
    /// Creates a point from internal integer coordinates.
    pub const fn new(x: Coord, y: Coord, z: Coord) -> Self {
        Self { x, y, z }
    }

    /// Converts millimetres into the checked integer coordinate domain.
    pub fn from_mm(x: f64, y: f64, z: f64) -> Result<Self, MeshError> {
        Ok(Self {
            x: mm_to_coord(x)?,
            y: mm_to_coord(y)?,
            z: mm_to_coord(z)?,
        })
    }

    /// Returns this point in millimetres.
    pub fn to_mm(self) -> [f64; 3] {
        const SCALE: f64 = 1_000_000.0;
        [
            self.x as f64 / SCALE,
            self.y as f64 / SCALE,
            self.z as f64 / SCALE,
        ]
    }
}

/// One triangle, referring to three vertices in a [`Mesh`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Triangle(
    /// Three indices into the mesh's vertex array.
    pub [u32; 3],
);

/// An indexed triangle mesh.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Mesh {
    vertices: Vec<Point3>,
    triangles: Vec<Triangle>,
}

/// Structural diagnostics that do not mutate the mesh.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MeshDiagnostics {
    /// Number of vertices.
    pub vertices: usize,
    /// Number of triangles.
    pub triangles: usize,
    /// Number of triangles with duplicate vertex indices.
    pub repeated_index_triangles: usize,
    /// Number of geometrically zero-area triangles.
    pub zero_area_triangles: usize,
    /// Number of edges that do not have exactly two incident triangles.
    pub boundary_or_non_manifold_edges: usize,
    /// Number of edges used by exactly one triangle.
    pub boundary_edges: usize,
    /// Number of edges used by more than two triangles.
    pub non_manifold_edges: usize,
    /// Number of two-triangle edges whose faces traverse the edge in the same direction.
    pub inconsistently_oriented_edges: usize,
}

/// Errors raised while building an indexed mesh.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeshError {
    /// Too many vertices for the `u32` triangle index representation.
    TooManyVertices,
    /// A triangle references a vertex that does not exist.
    InvalidVertexIndex {
        /// Zero-based triangle position in the mesh.
        triangle: usize,
        /// Referenced vertex index that is outside the vertex array.
        index: u32,
    },
    /// A box dimension supplied to [`Mesh::box_mm`] was zero or negative.
    NonPositiveDimension,
    /// The STL file could not be read.
    Io(String),
    /// The input bytes are neither a supported binary nor ASCII STL file.
    InvalidStl(String),
    /// The file is a ZIP but not a readable 3MF model part.
    InvalidThreeMf(String),
    /// A model transform contains invalid parameters.
    InvalidTransform(String),
    /// A mesh coordinate could not be represented.
    Geometry(GeometryError),
    /// Checked integer arithmetic exceeded the supported intermediate range.
    ArithmeticOverflow,
}

impl From<GeometryError> for MeshError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

impl fmt::Display for MeshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyVertices => formatter.write_str("mesh has more than u32::MAX vertices"),
            Self::InvalidVertexIndex { triangle, index } => {
                write!(
                    formatter,
                    "triangle {triangle} references missing vertex {index}"
                )
            }
            Self::NonPositiveDimension => formatter.write_str("box dimensions must be positive"),
            Self::Io(message) => write!(formatter, "could not read STL file: {message}"),
            Self::InvalidStl(message) => write!(formatter, "invalid STL file: {message}"),
            Self::InvalidThreeMf(message) => write!(formatter, "invalid 3MF file: {message}"),
            Self::InvalidTransform(message) => {
                write!(formatter, "invalid mesh transform: {message}")
            }
            Self::Geometry(error) => write!(formatter, "invalid mesh coordinate: {error}"),
            Self::ArithmeticOverflow => {
                formatter.write_str("mesh arithmetic overflowed the supported integer range")
            }
        }
    }
}

impl std::error::Error for MeshError {}

/// Affine transform applied at the model-import boundary.
///
/// Scaling is applied first, followed by X, Y and Z rotations (degrees), then
/// translation. A negative scale mirrors the mesh and automatically reverses
/// triangle winding so outward-facing normals remain outward-facing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshTransform {
    /// Scale factors along X, Y and Z. Values must be finite and non-zero.
    pub scale: [f64; 3],
    /// Euler rotations in degrees, applied X then Y then Z.
    pub rotation_degrees: [f64; 3],
    /// Translation in millimetres.
    pub translation_mm: [f64; 3],
}

impl Default for MeshTransform {
    fn default() -> Self {
        Self {
            scale: [1.0; 3],
            rotation_degrees: [0.0; 3],
            translation_mm: [0.0; 3],
        }
    }
}

/// Counters describing the changes made by [`Mesh::repair_basic`].
///
/// Repair is explicit and returns a new mesh. The source mesh is never
/// modified, and unresolved boundaries/non-manifold topology remain visible
/// in `final_diagnostics`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MeshRepairReport {
    /// Number of coincident vertices merged by exact coordinate equality.
    pub vertices_welded: usize,
    /// Number of unreferenced vertices removed after triangle filtering.
    pub unreferenced_vertices_removed: usize,
    /// Number of repeated-index or zero-area triangles removed.
    pub degenerate_triangles_removed: usize,
    /// Number of duplicate triangles removed regardless of winding.
    pub duplicate_triangles_removed: usize,
    /// Number of triangles flipped to make adjacent manifold faces consistent.
    pub triangles_flipped_for_consistency: usize,
    /// Number of triangles flipped when orienting closed components outward.
    pub triangles_flipped_for_outward_orientation: usize,
    /// Number of closed connected components flipped to outward orientation.
    pub components_flipped_outward: usize,
    /// Number of contradictory orientation constraints encountered.
    pub orientation_conflicts: usize,
    /// Number of edges with more than two incident faces; these are not repaired.
    pub unresolved_non_manifold_edges: usize,
    /// Number of closed orientable connected components recognized.
    pub closed_components: usize,
    /// Diagnostics after all requested repairs.
    pub final_diagnostics: MeshDiagnostics,
}

impl Mesh {
    /// Constructs a mesh after checking every triangle index.
    ///
    /// Degenerate triangles are retained so diagnostics can report them and
    /// the slicer can make an explicit policy decision.
    pub fn new(vertices: Vec<Point3>, triangles: Vec<Triangle>) -> Result<Self, MeshError> {
        if vertices.len() > u32::MAX as usize {
            return Err(MeshError::TooManyVertices);
        }

        for vertex in &vertices {
            if vertex.x < -MAX_ABS_COORD
                || vertex.x > MAX_ABS_COORD
                || vertex.y < -MAX_ABS_COORD
                || vertex.y > MAX_ABS_COORD
                || vertex.z < -MAX_ABS_COORD
                || vertex.z > MAX_ABS_COORD
            {
                return Err(MeshError::Geometry(GeometryError::CoordinateOutOfRange));
            }
        }

        for (triangle_index, triangle) in triangles.iter().enumerate() {
            for index in triangle.0 {
                if index as usize >= vertices.len() {
                    return Err(MeshError::InvalidVertexIndex {
                        triangle: triangle_index,
                        index,
                    });
                }
            }
        }

        Ok(Self {
            vertices,
            triangles,
        })
    }

    /// Creates a mesh from triangle soup, welding coordinates after nanometre
    /// quantization. Vertex IDs follow first appearance in the input; ordered
    /// lookup avoids hash iteration affecting the result.
    pub fn from_triangle_soup(triangle_soup: &[[[f64; 3]; 3]]) -> Result<Self, MeshError> {
        let mut index_by_vertex = BTreeMap::<Point3, u32>::new();
        let mut vertices = Vec::<Point3>::new();
        let mut triangles = Vec::<Triangle>::with_capacity(triangle_soup.len());

        for triangle in triangle_soup {
            let mut indices = [0_u32; 3];

            for (corner, coordinates) in triangle.iter().enumerate() {
                let point = Point3::from_mm(coordinates[0], coordinates[1], coordinates[2])?;

                let index = if let Some(index) = index_by_vertex.get(&point) {
                    *index
                } else {
                    let index =
                        u32::try_from(vertices.len()).map_err(|_| MeshError::TooManyVertices)?;
                    vertices.push(point);
                    index_by_vertex.insert(point, index);
                    index
                };

                indices[corner] = index;
            }

            triangles.push(Triangle(indices));
        }

        Self::new(vertices, triangles)
    }

    /// Returns the mesh vertices.
    pub fn vertices(&self) -> &[Point3] {
        &self.vertices
    }

    /// Returns the mesh triangles.
    pub fn triangles(&self) -> &[Triangle] {
        &self.triangles
    }

    /// Returns the three vertices referenced by a triangle.
    pub fn triangle_vertices(&self, index: usize) -> [Point3; 3] {
        let triangle = self.triangles[index];
        [
            self.vertices[triangle.0[0] as usize],
            self.vertices[triangle.0[1] as usize],
            self.vertices[triangle.0[2] as usize],
        ]
    }

    /// Returns the number of vertices.
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Returns the number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// Returns the minimum and maximum printer-space Z coordinates.
    ///
    /// An empty mesh has no Z bounds.
    pub fn z_bounds(&self) -> Option<(Coord, Coord)> {
        let mut vertices = self.vertices.iter();
        let first = vertices.next()?;
        let (mut minimum, mut maximum) = (first.z, first.z);
        for vertex in vertices {
            minimum = minimum.min(vertex.z);
            maximum = maximum.max(vertex.z);
        }
        Some((minimum, maximum))
    }

    /// Returns the enclosed volume in cubic millimetres.
    ///
    /// This is the divergence-theorem volume of the triangle soup, so it is
    /// only meaningful for a closed, consistently oriented mesh. An open mesh
    /// yields a number that is not a volume, which is why [`Mesh::diagnostics`]
    /// must be read alongside it.
    pub fn volume_mm3(&self) -> f64 {
        let scale = 1.0 / (UNITS_PER_MM as f64).powi(3);
        let mut sum = 0.0_f64;
        for triangle in &self.triangles {
            let [a, b, c] = self.triangle_vertices_for(*triangle);
            let (ax, ay, az) = (a.x as f64, a.y as f64, a.z as f64);
            let (bx, by, bz) = (b.x as f64, b.y as f64, b.z as f64);
            let (cx, cy, cz) = (c.x as f64, c.y as f64, c.z as f64);
            sum += ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx);
        }
        (sum / 6.0 * scale).abs()
    }

    /// Returns true when a triangle has zero geometric area or repeated indices.
    pub fn triangle_is_degenerate(&self, index: usize) -> bool {
        let triangle = self.triangles[index];
        let [a, b, c] = triangle.0;
        if a == b || b == c || c == a {
            return true;
        }
        let [pa, pb, pc] = self.triangle_vertices_for(triangle);
        cross3(pa, pb, pc) == [0, 0, 0]
    }

    /// Computes structural diagnostics without modifying the mesh.
    pub fn diagnostics(&self) -> MeshDiagnostics {
        let mut result = MeshDiagnostics {
            vertices: self.vertices.len(),
            triangles: self.triangles.len(),
            ..MeshDiagnostics::default()
        };

        // For each undirected edge retain both the use count and the number of
        // uses matching its canonical low-index -> high-index direction. A
        // consistently oriented closed two-manifold has one use each way.
        let mut edge_use = BTreeMap::<(u32, u32), (u32, u32)>::new();

        for triangle in &self.triangles {
            let [a, b, c] = triangle.0;
            if a == b || b == c || c == a {
                result.repeated_index_triangles += 1;
            } else {
                let [pa, pb, pc] = self.triangle_vertices_for(*triangle);
                if cross3(pa, pb, pc) == [0, 0, 0] {
                    result.zero_area_triangles += 1;
                }
            }

            for (first, second) in [(a, b), (b, c), (c, a)] {
                let key = if first < second {
                    (first, second)
                } else {
                    (second, first)
                };
                let entry = edge_use.entry(key).or_default();
                entry.0 += 1;
                if first < second {
                    entry.1 += 1;
                }
            }
        }

        for &(uses, canonical_direction_uses) in edge_use.values() {
            if uses != 2 {
                result.boundary_or_non_manifold_edges += 1;
            }
            if uses == 1 {
                result.boundary_edges += 1;
            } else if uses > 2 {
                result.non_manifold_edges += 1;
            } else if uses == 2 && (canonical_direction_uses == 0 || canonical_direction_uses == 2)
            {
                result.inconsistently_oriented_edges += 1;
            }
        }

        result
    }

    /// Returns a transformed copy of this mesh.
    ///
    /// Non-finite parameters, zero scale factors, and transformed coordinates
    /// outside the canonical range are rejected. This method intentionally
    /// does not infer units: use a scale of `25.4` when importing inch-valued
    /// STL coordinates as millimetres.
    pub fn transformed(&self, transform: MeshTransform) -> Result<Self, MeshError> {
        if transform
            .scale
            .iter()
            .any(|value| !value.is_finite() || *value == 0.0)
            || transform
                .rotation_degrees
                .iter()
                .any(|value| !value.is_finite())
            || transform
                .translation_mm
                .iter()
                .any(|value| !value.is_finite())
        {
            return Err(MeshError::InvalidTransform(
                "parameters must be finite and scale factors must be non-zero".to_owned(),
            ));
        }

        let radians = transform.rotation_degrees.map(f64::to_radians);
        let (sx, cx) = radians[0].sin_cos();
        let (sy, cy) = radians[1].sin_cos();
        let (sz, cz) = radians[2].sin_cos();
        let mut vertices = Vec::with_capacity(self.vertices.len());

        for point in &self.vertices {
            let mut x = point.x as f64 / 1_000_000.0 * transform.scale[0];
            let mut y = point.y as f64 / 1_000_000.0 * transform.scale[1];
            let mut z = point.z as f64 / 1_000_000.0 * transform.scale[2];

            // X rotation.
            (y, z) = (y * cx - z * sx, y * sx + z * cx);
            // Y rotation.
            (x, z) = (x * cy + z * sy, -x * sy + z * cy);
            // Z rotation.
            (x, y) = (x * cz - y * sz, x * sz + y * cz);

            vertices.push(Point3::from_mm(
                x + transform.translation_mm[0],
                y + transform.translation_mm[1],
                z + transform.translation_mm[2],
            )?);
        }

        let mut triangles = self.triangles.clone();
        if transform.scale[0] * transform.scale[1] * transform.scale[2] < 0.0 {
            for triangle in &mut triangles {
                triangle.0.swap(1, 2);
            }
        }
        Self::new(vertices, triangles)
    }

    /// Performs a conservative, non-destructive basic mesh repair.
    ///
    /// Exact-coordinate duplicate vertices are welded, degenerate and duplicate
    /// faces are removed, unused vertices are compacted, and manifold face
    /// winding is made consistent. Closed orientable components are then
    /// oriented outward using signed volume. No hole filling, vertex movement,
    /// non-manifold surgery, shell union or self-intersection repair is attempted.
    pub fn repair_basic(&self) -> Result<(Self, MeshRepairReport), MeshError> {
        let mut report = MeshRepairReport::default();

        // First merge exactly coincident vertices while preserving first-use order.
        let mut vertex_map = BTreeMap::<Point3, u32>::new();
        let mut welded_vertices = Vec::<Point3>::new();
        let mut old_to_welded = Vec::<u32>::with_capacity(self.vertices.len());
        for vertex in &self.vertices {
            let index = if let Some(existing) = vertex_map.get(vertex) {
                report.vertices_welded += 1;
                *existing
            } else {
                let index =
                    u32::try_from(welded_vertices.len()).map_err(|_| MeshError::TooManyVertices)?;
                welded_vertices.push(*vertex);
                vertex_map.insert(*vertex, index);
                index
            };
            old_to_welded.push(index);
        }

        let mut unique_faces = BTreeSet::<[u32; 3]>::new();
        let mut filtered = Vec::<Triangle>::with_capacity(self.triangles.len());
        for triangle in &self.triangles {
            let indices = triangle.0.map(|index| old_to_welded[index as usize]);
            if indices[0] == indices[1]
                || indices[1] == indices[2]
                || indices[2] == indices[0]
                || cross3(
                    welded_vertices[indices[0] as usize],
                    welded_vertices[indices[1] as usize],
                    welded_vertices[indices[2] as usize],
                ) == [0, 0, 0]
            {
                report.degenerate_triangles_removed += 1;
                continue;
            }

            let mut canonical = indices;
            canonical.sort_unstable();
            if !unique_faces.insert(canonical) {
                report.duplicate_triangles_removed += 1;
                continue;
            }
            filtered.push(Triangle(indices));
        }

        // Compact unused vertices so indices are dense and reproducible.
        let mut used = vec![false; welded_vertices.len()];
        for triangle in &filtered {
            for index in triangle.0 {
                used[index as usize] = true;
            }
        }
        let mut compact_map = vec![u32::MAX; welded_vertices.len()];
        let mut vertices = Vec::new();
        for (old_index, vertex) in welded_vertices.iter().enumerate() {
            if used[old_index] {
                compact_map[old_index] =
                    u32::try_from(vertices.len()).map_err(|_| MeshError::TooManyVertices)?;
                vertices.push(*vertex);
            } else {
                report.unreferenced_vertices_removed += 1;
            }
        }
        let mut triangles = filtered
            .into_iter()
            .map(|triangle| Triangle(triangle.0.map(|index| compact_map[index as usize])))
            .collect::<Vec<_>>();

        // Build face adjacency from undirected edges. A pair of faces that use
        // an edge in the same direction requires exactly one of them to flip.
        let mut edge_uses = BTreeMap::<(u32, u32), Vec<(usize, bool)>>::new();
        for (face_index, triangle) in triangles.iter().enumerate() {
            let [a, b, c] = triangle.0;
            for (first, second) in [(a, b), (b, c), (c, a)] {
                let (key, forward) = if first < second {
                    ((first, second), true)
                } else {
                    ((second, first), false)
                };
                edge_uses
                    .entry(key)
                    .or_default()
                    .push((face_index, forward));
            }
        }
        report.unresolved_non_manifold_edges =
            edge_uses.values().filter(|uses| uses.len() > 2).count();

        let mut adjacency = vec![Vec::<(usize, bool)>::new(); triangles.len()];
        let mut face_edges = vec![Vec::<(u32, u32)>::with_capacity(3); triangles.len()];
        for (edge, uses) in &edge_uses {
            for (face_index, _) in uses {
                face_edges[*face_index].push(*edge);
            }
            if uses.len() == 2 {
                let (first_face, first_direction) = uses[0];
                let (second_face, second_direction) = uses[1];
                let must_flip = first_direction == second_direction;
                adjacency[first_face].push((second_face, must_flip));
                adjacency[second_face].push((first_face, must_flip));
            }
        }

        let mut flip_state = vec![None::<bool>; triangles.len()];
        let mut components = Vec::<(Vec<usize>, bool)>::new();
        for seed in 0..triangles.len() {
            if flip_state[seed].is_some() {
                continue;
            }
            flip_state[seed] = Some(false);
            let mut queue = std::collections::VecDeque::new();
            queue.push_back(seed);
            let mut component = Vec::new();
            let mut component_closed = true;
            let mut component_conflicted = false;

            while let Some(face) = queue.pop_front() {
                component.push(face);
                for edge in &face_edges[face] {
                    match edge_uses.get(edge).map(Vec::len).unwrap_or(0) {
                        2 => {}
                        _ => component_closed = false,
                    }
                }
                for (neighbor, must_flip) in &adjacency[face] {
                    let expected = flip_state[face].unwrap_or(false) ^ *must_flip;
                    match flip_state[*neighbor] {
                        None => {
                            flip_state[*neighbor] = Some(expected);
                            queue.push_back(*neighbor);
                        }
                        Some(actual) if actual != expected => {
                            report.orientation_conflicts += 1;
                            component_conflicted = true;
                        }
                        Some(_) => {}
                    }
                }
            }
            components.push((component, component_closed && !component_conflicted));
        }

        for (index, should_flip) in flip_state.iter().enumerate() {
            if should_flip.unwrap_or(false) {
                triangles[index].0.swap(1, 2);
                report.triangles_flipped_for_consistency += 1;
            }
        }

        // Closed components have an unambiguous outward direction. Open shells
        // are kept consistently oriented but are not assigned an arbitrary side.
        for (component, closed_and_orientable) in components {
            if !closed_and_orientable {
                continue;
            }
            report.closed_components += 1;
            let mut signed_volume6 = 0_i128;
            for face_index in &component {
                let [a, b, c] = triangles[*face_index]
                    .0
                    .map(|index| vertices[index as usize]);
                signed_volume6 = signed_volume6
                    .checked_add(determinant3(a, b, c).ok_or(MeshError::ArithmeticOverflow)?)
                    .ok_or(MeshError::ArithmeticOverflow)?;
            }
            if signed_volume6 < 0 {
                for face_index in &component {
                    triangles[*face_index].0.swap(1, 2);
                }
                report.components_flipped_outward += 1;
                report.triangles_flipped_for_outward_orientation += component.len();
            }
        }

        let repaired = Self::new(vertices, triangles)?;
        report.final_diagnostics = repaired.diagnostics();
        Ok((repaired, report))
    }

    fn triangle_vertices_for(&self, triangle: Triangle) -> [Point3; 3] {
        [
            self.vertices[triangle.0[0] as usize],
            self.vertices[triangle.0[1] as usize],
            self.vertices[triangle.0[2] as usize],
        ]
    }

    /// Loads a model by file extension: STL or 3MF.
    ///
    /// 3MF carries its own units and per-item placement, so the returned mesh is
    /// already in millimetres and already arranged; an STL is read raw and the
    /// caller applies units as before.
    pub fn load_model<P: AsRef<Path>>(path: P) -> Result<Self, MeshError> {
        let is_3mf = path
            .as_ref()
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("3mf"));
        if is_3mf {
            load_3mf(path)
        } else {
            Self::load_stl(path)
        }
    }

    /// Loads a binary or ASCII STL file and welds coincident vertices.
    ///
    /// STL does not preserve units or object-level project metadata. The caller
    /// must establish the model units and any required transforms separately.
    pub fn load_stl<P: AsRef<Path>>(path: P) -> Result<Self, MeshError> {
        let bytes = fs::read(path).map_err(|error| MeshError::Io(error.to_string()))?;
        Self::from_stl_bytes(&bytes)
    }

    /// Merges several meshes into one, keeping every mesh's own coordinates.
    ///
    /// The merged mesh slices as one body: neighbouring parts become separate
    /// rings in the same cross-section, which is how a plate of parts prints.
    /// Object identity is lost, so a caller that needs it keeps the originals.
    pub fn merged(parts: &[Mesh]) -> Result<Self, MeshError> {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for part in parts {
            let base = u32::try_from(vertices.len()).map_err(|_| MeshError::TooManyVertices)?;
            vertices.extend_from_slice(&part.vertices);
            triangles.extend(part.triangles.iter().map(|triangle| {
                Triangle([
                    triangle.0[0] + base,
                    triangle.0[1] + base,
                    triangle.0[2] + base,
                ])
            }));
        }
        Self::new(vertices, triangles)
    }

    /// Bounding box in millimetres: `(min_x, min_y, max_x, max_y)`.
    ///
    /// `None` when the mesh has no vertices, which callers treat as an error
    /// rather than an empty box at the origin.
    pub fn xy_bounds_mm(&self) -> Option<(f64, f64, f64, f64)> {
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for point in &self.vertices {
            let [x, y, _] = point.to_mm();
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((lx, ly, hx, hy)) => (lx.min(x), ly.min(y), hx.max(x), hy.max(y)),
            });
        }
        bounds
    }

    /// Parses an STL byte buffer (binary STL first, then ASCII STL).
    pub fn from_stl_bytes(bytes: &[u8]) -> Result<Self, MeshError> {
        if bytes.len() >= 84 {
            let triangle_count =
                u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
            let expected_len = 84_usize
                .checked_add(triangle_count.checked_mul(50).ok_or_else(|| {
                    MeshError::InvalidStl(
                        "binary triangle count overflows the file size".to_owned(),
                    )
                })?)
                .ok_or_else(|| {
                    MeshError::InvalidStl("binary file size overflows address space".to_owned())
                })?;

            // The length test is authoritative. Binary headers are arbitrary and
            // may begin with the word "solid", which is common in ASCII STL.
            if expected_len == bytes.len() {
                return Self::from_binary_stl(bytes, triangle_count);
            }
        }

        Self::from_ascii_stl(bytes)
    }

    fn from_binary_stl(bytes: &[u8], triangle_count: usize) -> Result<Self, MeshError> {
        let mut soup = Vec::with_capacity(triangle_count);
        let mut offset = 84;

        for _ in 0..triangle_count {
            // The first 12 bytes are the face normal. They are not trusted: the
            // engine computes geometry from vertices, not STL's optional normal.
            offset += 12;
            let mut triangle = [[0.0_f64; 3]; 3];

            for vertex in &mut triangle {
                for coordinate in vertex {
                    let end = offset + 4;
                    let raw: [u8; 4] = bytes[offset..end]
                        .try_into()
                        .expect("binary STL record length was checked");
                    let value = f32::from_le_bytes(raw);
                    if !value.is_finite() {
                        return Err(MeshError::InvalidStl(
                            "binary STL contains a non-finite vertex".to_owned(),
                        ));
                    }
                    *coordinate = value as f64;
                    offset = end;
                }
            }

            // Two attribute bytes follow each triangle. Optional colour
            // extensions are currently ignored.
            offset += 2;
            soup.push(triangle);
        }

        Self::from_triangle_soup(&soup)
    }

    fn from_ascii_stl(bytes: &[u8]) -> Result<Self, MeshError> {
        let source = std::str::from_utf8(bytes).map_err(|_| {
            MeshError::InvalidStl("file is not a valid binary STL or UTF-8 ASCII STL".to_owned())
        })?;
        let mut tokens = source.split_whitespace();
        let mut vertices = Vec::<[f64; 3]>::new();

        while let Some(token) = tokens.next() {
            if !token.eq_ignore_ascii_case("vertex") {
                continue;
            }

            let mut point = [0.0_f64; 3];
            for coordinate in &mut point {
                let text = tokens.next().ok_or_else(|| {
                    MeshError::InvalidStl("ASCII STL has an incomplete vertex".to_owned())
                })?;
                *coordinate = text.parse::<f64>().map_err(|_| {
                    MeshError::InvalidStl(format!("invalid ASCII STL coordinate: {text}"))
                })?;
                if !coordinate.is_finite() {
                    return Err(MeshError::InvalidStl(
                        "ASCII STL contains a non-finite vertex".to_owned(),
                    ));
                }
            }
            vertices.push(point);
        }

        if vertices.is_empty() {
            return Err(MeshError::InvalidStl(
                "no ASCII STL vertex records were found".to_owned(),
            ));
        }
        if vertices.len() % 3 != 0 {
            return Err(MeshError::InvalidStl(
                "ASCII STL vertex count is not a multiple of three".to_owned(),
            ));
        }

        let soup = vertices
            .chunks_exact(3)
            .map(|points| [points[0], points[1], points[2]])
            .collect::<Vec<_>>();
        Self::from_triangle_soup(&soup)
    }

    /// Creates a consistently wound rectangular box for tests and examples.
    pub fn box_mm(width: f64, depth: f64, height: f64) -> Result<Self, MeshError> {
        if width <= 0.0 || depth <= 0.0 || height <= 0.0 {
            return Err(MeshError::NonPositiveDimension);
        }

        let vertices = vec![
            Point3::from_mm(0.0, 0.0, 0.0)?,
            Point3::from_mm(width, 0.0, 0.0)?,
            Point3::from_mm(width, depth, 0.0)?,
            Point3::from_mm(0.0, depth, 0.0)?,
            Point3::from_mm(0.0, 0.0, height)?,
            Point3::from_mm(width, 0.0, height)?,
            Point3::from_mm(width, depth, height)?,
            Point3::from_mm(0.0, depth, height)?,
        ];

        // Outward-facing triangles. Each face's diagonal is shared once.
        let triangles = vec![
            Triangle([0, 2, 1]),
            Triangle([0, 3, 2]), // bottom
            Triangle([4, 5, 6]),
            Triangle([4, 6, 7]), // top
            Triangle([0, 1, 5]),
            Triangle([0, 5, 4]), // front
            Triangle([1, 2, 6]),
            Triangle([1, 6, 5]), // right
            Triangle([2, 3, 7]),
            Triangle([2, 7, 6]), // back
            Triangle([3, 0, 4]),
            Triangle([3, 4, 7]), // left
        ];

        Self::new(vertices, triangles)
    }
}

fn determinant3(a: Point3, b: Point3, c: Point3) -> Option<i128> {
    let by_cz = (b.y as i128).checked_mul(c.z as i128)?;
    let bz_cy = (b.z as i128).checked_mul(c.y as i128)?;
    let bx_cz = (b.x as i128).checked_mul(c.z as i128)?;
    let bz_cx = (b.z as i128).checked_mul(c.x as i128)?;
    let bx_cy = (b.x as i128).checked_mul(c.y as i128)?;
    let by_cx = (b.y as i128).checked_mul(c.x as i128)?;
    let x_term = (a.x as i128).checked_mul(by_cz.checked_sub(bz_cy)?)?;
    let y_term = (a.y as i128).checked_mul(bx_cz.checked_sub(bz_cx)?)?;
    let z_term = (a.z as i128).checked_mul(bx_cy.checked_sub(by_cx)?)?;
    x_term.checked_sub(y_term)?.checked_add(z_term)
}

fn cross3(a: Point3, b: Point3, c: Point3) -> [i128; 3] {
    let ab = [
        b.x as i128 - a.x as i128,
        b.y as i128 - a.y as i128,
        b.z as i128 - a.z as i128,
    ];
    let ac = [
        c.x as i128 - a.x as i128,
        c.y as i128 - a.y as i128,
        c.z as i128 - a.z as i128,
    ];

    [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_is_closed_and_has_no_degenerate_faces() {
        let mesh = Mesh::box_mm(20.0, 20.0, 20.0).expect("box");
        let report = mesh.diagnostics();
        assert_eq!(report.vertices, 8);
        assert_eq!(report.triangles, 12);
        assert_eq!(report.repeated_index_triangles, 0);
        assert_eq!(report.zero_area_triangles, 0);
        assert_eq!(report.boundary_or_non_manifold_edges, 0);
        assert_eq!(report.boundary_edges, 0);
        assert_eq!(report.non_manifold_edges, 0);
        assert_eq!(report.inconsistently_oriented_edges, 0);
        assert_eq!(mesh.z_bounds(), Some((0, 20_000_000)));
    }

    #[test]
    fn diagnostics_distinguish_open_and_non_manifold_edges() {
        let open = Mesh::new(
            vec![
                Point3::new(0, 0, 0),
                Point3::new(1_000_000, 0, 0),
                Point3::new(0, 1_000_000, 0),
                Point3::new(0, 0, 1_000_000),
            ],
            vec![Triangle([0, 1, 2])],
        )
        .expect("valid open triangle");
        let report = open.diagnostics();
        assert_eq!(report.boundary_edges, 3);
        assert_eq!(report.non_manifold_edges, 0);

        let three_face_edge = Mesh::new(
            vec![
                Point3::new(0, 0, 0),
                Point3::new(1_000_000, 0, 0),
                Point3::new(0, 1_000_000, 0),
                Point3::new(0, -1_000_000, 0),
                Point3::new(0, 0, 1_000_000),
            ],
            vec![
                Triangle([0, 1, 2]),
                Triangle([1, 0, 3]),
                Triangle([0, 1, 4]),
            ],
        )
        .expect("valid indexed mesh");
        assert_eq!(three_face_edge.diagnostics().non_manifold_edges, 1);

        let inconsistent_winding = Mesh::new(
            vec![
                Point3::new(0, 0, 0),
                Point3::new(1_000_000, 0, 0),
                Point3::new(0, 1_000_000, 0),
                Point3::new(0, 0, 1_000_000),
            ],
            vec![Triangle([0, 1, 2]), Triangle([0, 1, 3])],
        )
        .expect("valid indexed mesh");
        assert_eq!(
            inconsistent_winding
                .diagnostics()
                .inconsistently_oriented_edges,
            1
        );
    }

    #[test]
    fn rejects_missing_vertex_indices() {
        let result = Mesh::new(vec![Point3::new(0, 0, 0)], vec![Triangle([0, 1, 0])]);
        assert_eq!(
            result,
            Err(MeshError::InvalidVertexIndex {
                triangle: 0,
                index: 1
            })
        );
    }

    #[test]
    fn rejects_coordinates_outside_the_checked_integer_domain() {
        let result = Mesh::new(vec![Point3::new(i64::MAX, 0, 0)], vec![]);
        assert_eq!(
            result,
            Err(MeshError::Geometry(GeometryError::CoordinateOutOfRange))
        );
    }

    #[test]
    fn rejects_non_positive_box_dimensions() {
        assert_eq!(
            Mesh::box_mm(20.0, 0.0, 20.0),
            Err(MeshError::NonPositiveDimension)
        );
    }

    #[test]
    fn parses_ascii_stl_and_welds_vertices() {
        let stl = b"solid cube\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid cube\n";
        let mesh = Mesh::from_stl_bytes(stl).expect("ASCII STL");
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.triangle_count(), 1);
    }

    #[test]
    fn parses_binary_stl_even_when_header_starts_with_solid() {
        let mut bytes = vec![0_u8; 84 + 50];
        bytes[..5].copy_from_slice(b"solid");
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        let vertices = [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut offset = 84 + 12;
        for vertex in vertices {
            for coordinate in vertex {
                bytes[offset..offset + 4].copy_from_slice(&coordinate.to_le_bytes());
                offset += 4;
            }
        }
        let mesh = Mesh::from_stl_bytes(&bytes).expect("binary STL");
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.triangle_count(), 1);
    }

    #[test]
    fn rejects_unrecognised_stl_data() {
        assert!(matches!(
            Mesh::from_stl_bytes(b"not a model"),
            Err(MeshError::InvalidStl(_))
        ));
    }

    #[test]
    fn triangle_soup_welds_shared_vertices_deterministically() {
        let soup = [
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        ];

        let mesh = Mesh::from_triangle_soup(&soup).expect("valid soup");
        assert_eq!(mesh.vertex_count(), 4);
        assert_eq!(mesh.triangle_count(), 2);
    }
}

#[cfg(test)]
mod preparation_tests {
    use super::*;

    #[test]
    fn transforms_apply_scale_rotation_and_translation() {
        let cube = Mesh::box_mm(2.0, 1.0, 1.0).expect("cube");
        let moved = cube
            .transformed(MeshTransform {
                scale: [2.0, 1.0, 1.0],
                rotation_degrees: [0.0, 0.0, 90.0],
                translation_mm: [10.0, 20.0, 3.0],
            })
            .expect("transformed cube");
        let xs = moved.vertices().iter().map(|p| p.x).min().unwrap();
        let xe = moved.vertices().iter().map(|p| p.x).max().unwrap();
        let ys = moved.vertices().iter().map(|p| p.y).min().unwrap();
        let ye = moved.vertices().iter().map(|p| p.y).max().unwrap();
        let zs = moved.vertices().iter().map(|p| p.z).min().unwrap();
        let ze = moved.vertices().iter().map(|p| p.z).max().unwrap();
        assert_eq!((xs, xe), (9_000_000, 10_000_000));
        assert_eq!((ys, ye), (20_000_000, 24_000_000));
        assert_eq!((zs, ze), (3_000_000, 4_000_000));
    }

    #[test]
    fn mirrored_transform_preserves_outward_winding() {
        let cube = Mesh::box_mm(2.0, 2.0, 2.0).expect("cube");
        let mirrored = cube
            .transformed(MeshTransform {
                scale: [-1.0, 1.0, 1.0],
                ..MeshTransform::default()
            })
            .expect("mirror");
        assert_eq!(mirrored.diagnostics().inconsistently_oriented_edges, 0);
    }

    #[test]
    fn basic_repair_welds_removes_duplicates_and_fixes_one_flipped_face() {
        let cube = Mesh::box_mm(10.0, 10.0, 10.0).expect("cube");
        let mut triangles = cube.triangles().to_vec();
        triangles[0].0.swap(1, 2);
        let duplicate = triangles[1];
        triangles.push(duplicate);
        let vertices = cube
            .vertices()
            .iter()
            .copied()
            .chain([Point3::new(99, 99, 99)])
            .collect();
        let damaged = Mesh::new(vertices, triangles).expect("damaged mesh");
        let (repaired, report) = damaged.repair_basic().expect("repair");
        assert_eq!(report.duplicate_triangles_removed, 1);
        assert_eq!(report.unreferenced_vertices_removed, 1);
        assert_eq!(report.closed_components, 1);
        assert_eq!(report.orientation_conflicts, 0);
        assert_eq!(repaired.triangle_count(), 12);
        assert_eq!(report.final_diagnostics.boundary_edges, 0);
        assert_eq!(report.final_diagnostics.non_manifold_edges, 0);
        assert_eq!(report.final_diagnostics.inconsistently_oriented_edges, 0);
    }

    #[test]
    fn basic_repair_merges_exact_duplicate_vertices_and_removes_degenerate_faces() {
        let vertices = vec![
            Point3::new(0, 0, 0),
            Point3::new(1_000_000, 0, 0),
            Point3::new(0, 1_000_000, 0),
            Point3::new(0, 0, 0),
        ];
        let mesh =
            Mesh::new(vertices, vec![Triangle([0, 1, 2]), Triangle([0, 3, 1])]).expect("mesh");
        let (repaired, report) = mesh.repair_basic().expect("repair");
        assert_eq!(report.vertices_welded, 1);
        assert_eq!(report.degenerate_triangles_removed, 1);
        assert_eq!(repaired.triangle_count(), 1);
    }
}
