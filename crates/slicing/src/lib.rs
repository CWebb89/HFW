//! Triangle/plane intersection and deterministic contour reconstruction.
//!
//! Each contour keeps the winding its triangles gave it, so overlapping shells
//! and holes survive to the region builder instead of being re-decided by
//! containment depth. Open outlines are closed in stages, nearest endpoints
//! first, before a layer is reported as unprintable.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

use hfw_geometry::{mm_to_coord, Contour, Coord, GeometryError, Point2, Segment2, MAX_ABS_COORD};
use hfw_mesh::{Mesh, Point3};

/// A horizontal slicing plane, represented by its printer-space Z coordinate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlicePlane {
    /// Z coordinate in integer nanometres.
    pub z: Coord,
}

impl SlicePlane {
    /// Creates a slicing plane from millimetres.
    pub fn from_mm(z_mm: f64) -> Result<Self, SliceError> {
        Ok(Self {
            z: mm_to_coord(z_mm)?,
        })
    }
}

/// One physical layer in a print schedule.
///
/// `bottom_z` and `top_z` describe the deposited layer interval. `slice_z` is
/// deliberately placed at its midpoint so slicing normally avoids coplanar
/// top and bottom faces. `print_z` is the top surface height used by motion and
/// G-code planning later.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayerSpec {
    /// Zero-based layer index.
    pub index: usize,
    /// Bottom of this layer in nanometres.
    pub bottom_z: Coord,
    /// Top of this layer in nanometres.
    pub top_z: Coord,
    /// Midpoint plane used to intersect the model, in nanometres.
    pub slice_z: Coord,
    /// Final nozzle Z for this layer, in nanometres.
    pub print_z: Coord,
}

/// A validated sequence of layer heights for one object's Z range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerSchedule {
    layers: Vec<LayerSpec>,
}

impl LayerSchedule {
    /// Upper bound prevents a corrupt model or a microscopic layer-height
    /// setting from allocating an unbounded number of layer records.
    pub const MAX_LAYERS: usize = 2_000_000;

    /// Creates a schedule using integer nanometre coordinates.
    ///
    /// The first layer uses `first_layer_height`; all later layers use
    /// `layer_height`. The final layer may be shorter to end exactly at `z_max`.
    pub fn new(
        z_min: Coord,
        z_max: Coord,
        first_layer_height: Coord,
        layer_height: Coord,
    ) -> Result<Self, SliceError> {
        if !(-MAX_ABS_COORD..=MAX_ABS_COORD).contains(&z_min)
            || !(-MAX_ABS_COORD..=MAX_ABS_COORD).contains(&z_max)
            || z_max <= z_min
        {
            return Err(SliceError::InvalidZRange { z_min, z_max });
        }
        if first_layer_height <= 0 || layer_height <= 0 {
            return Err(SliceError::NonPositiveLayerHeight);
        }

        let mut layers = Vec::new();
        let mut bottom_z = z_min;

        while bottom_z < z_max {
            if layers.len() >= Self::MAX_LAYERS {
                return Err(SliceError::TooManyLayers {
                    limit: Self::MAX_LAYERS,
                });
            }

            let requested_height = if layers.is_empty() {
                first_layer_height
            } else {
                layer_height
            };
            let remaining = z_max - bottom_z;
            let actual_height = requested_height.min(remaining);
            let top_z = bottom_z
                .checked_add(actual_height)
                .ok_or(SliceError::LayerCoordinateOverflow)?;
            let slice_z = bottom_z + actual_height / 2;

            layers.push(LayerSpec {
                index: layers.len(),
                bottom_z,
                top_z,
                slice_z,
                print_z: top_z,
            });
            bottom_z = top_z;
        }

        Ok(Self { layers })
    }

    /// Creates a schedule from millimetres, converting each input once into
    /// the engine's canonical integer coordinate system.
    pub fn from_mm(
        z_min_mm: f64,
        z_max_mm: f64,
        first_layer_height_mm: f64,
        layer_height_mm: f64,
    ) -> Result<Self, SliceError> {
        Self::new(
            mm_to_coord(z_min_mm)?,
            mm_to_coord(z_max_mm)?,
            mm_to_coord(first_layer_height_mm)?,
            mm_to_coord(layer_height_mm)?,
        )
    }

    /// Creates a schedule from a layer-height profile in millimetres.
    ///
    /// The profile is a list of `(z, height)` pairs in ascending `z`: the layer
    /// whose bottom sits at or above `z` uses `height` until the next pair takes
    /// over, so a profile is a step function of Z that a GUI can draw. The first
    /// layer keeps `first_layer_height` whatever the profile says, because bed
    /// adhesion is a property of the first layer and not of the model, and any
    /// height outside `min_height..=max_height` is clamped into it.
    ///
    /// Each step is a whole number of layers of the requested height, and the
    /// last layer is trimmed to land exactly on `z_max`. An empty profile falls
    /// back to a uniform stack of `layer_height`.
    pub fn from_profile(
        z_min: Coord,
        z_max: Coord,
        first_layer_height: Coord,
        layer_height: Coord,
        profile: &[(f64, f64)],
        min_height: f64,
        max_height: f64,
    ) -> Result<Self, SliceError> {
        if profile.is_empty() {
            return Self::new(z_min, z_max, first_layer_height, layer_height);
        }
        if !(-MAX_ABS_COORD..=MAX_ABS_COORD).contains(&z_min)
            || !(-MAX_ABS_COORD..=MAX_ABS_COORD).contains(&z_max)
            || z_max <= z_min
        {
            return Err(SliceError::InvalidZRange { z_min, z_max });
        }
        if first_layer_height <= 0 || layer_height <= 0 {
            return Err(SliceError::NonPositiveLayerHeight);
        }
        let ceiling_mm = if max_height > 0.0 {
            max_height
        } else {
            NO_CEILING_MM
        };
        if !(min_height.is_finite() && ceiling_mm.is_finite())
            || min_height <= 0.0
            || ceiling_mm < min_height
        {
            return Err(SliceError::NonPositiveLayerHeight);
        }
        let floor = mm_to_coord(min_height)?;
        let ceiling = mm_to_coord(ceiling_mm)?;
        let mut steps: Vec<(Coord, Coord)> = Vec::with_capacity(profile.len());
        for (z, height) in profile {
            if !(z.is_finite() && height.is_finite()) {
                return Err(SliceError::NonPositiveLayerHeight);
            }
            let requested = mm_to_coord(*height)?;
            if requested <= 0 {
                return Err(SliceError::NonPositiveLayerHeight);
            }
            steps.push((mm_to_coord(*z)?, requested.clamp(floor, ceiling)));
        }
        steps.sort_by_key(|(z, _)| *z);

        let mut layers: Vec<LayerSpec> = Vec::new();
        let mut bottom_z = z_min;
        while bottom_z < z_max {
            if layers.len() >= Self::MAX_LAYERS {
                return Err(SliceError::TooManyLayers {
                    limit: Self::MAX_LAYERS,
                });
            }
            let requested_height = if layers.is_empty() {
                first_layer_height
            } else {
                let wanted = height_at(&steps, bottom_z, layer_height, floor, ceiling);
                match next_step(&steps, bottom_z) {
                    Some(next) if next - bottom_z > 0 && next - bottom_z < wanted => {
                        let room = next - bottom_z;
                        if room >= floor {
                            room
                        } else {
                            wanted
                        }
                    }
                    _ => wanted,
                }
            };
            let remaining = z_max - bottom_z;
            let actual_height = requested_height.min(remaining);
            let top_z = bottom_z
                .checked_add(actual_height)
                .ok_or(SliceError::LayerCoordinateOverflow)?;
            let slice_z = bottom_z + actual_height / 2;
            layers.push(LayerSpec {
                index: layers.len(),
                bottom_z,
                top_z,
                slice_z,
                print_z: top_z,
            });
            bottom_z = top_z;
        }
        trim_thin_top(&mut layers, floor);
        Ok(Self { layers })
    }

    /// Returns the scheduled layers in bottom-to-top order.
    pub fn layers(&self) -> &[LayerSpec] {
        &self.layers
    }

    /// Returns the number of scheduled layers.
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// Returns true when the schedule has no layers.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }
}

/// Ceiling used when a profile asks for no upper layer-height bound.
const NO_CEILING_MM: f64 = 1.0e6;

/// Folds a final layer thinner than the floor into the one below it.
///
/// The stack has to end exactly on `z_max`, so the last step is whatever is
/// left over. Left over can be thinner than the machine prints, and a layer
/// below the floor is one the first layer's height would have avoided: merging
/// it into its neighbour keeps the top of the print where the model puts it and
/// the count of layers printable.
fn trim_thin_top(layers: &mut Vec<LayerSpec>, floor: Coord) {
    if layers.len() < 2 {
        return;
    }
    let last = layers[layers.len() - 1];
    if last.top_z - last.bottom_z >= floor {
        return;
    }
    layers.pop();
    let previous = layers.len() - 1;
    let bottom = layers[previous].bottom_z;
    let top = last.top_z;
    layers[previous].index = previous;
    layers[previous].top_z = top;
    layers[previous].slice_z = bottom + (top - bottom) / 2;
    layers[previous].print_z = top;
}

/// The first profile step above `z`.
fn next_step(steps: &[(Coord, Coord)], z: Coord) -> Option<Coord> {
    steps.iter().map(|(at, _)| *at).find(|at| *at > z)
}

/// The clamped layer height a profile asks for at `z`.
fn height_at(
    steps: &[(Coord, Coord)],
    z: Coord,
    fallback: Coord,
    floor: Coord,
    ceiling: Coord,
) -> Coord {
    let mut chosen = fallback;
    for (at, height) in steps {
        if *at > z {
            break;
        }
        chosen = *height;
    }
    chosen.clamp(floor, ceiling)
}

/// Counts useful diagnostics for one mesh/plane intersection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SliceDiagnostics {
    /// Total triangles inspected.
    pub triangles_tested: usize,
    /// Triangles that generated a non-zero intersection segment.
    pub intersecting_triangles: usize,
    /// Triangles exactly coplanar with the plane and deliberately skipped.
    pub coplanar_triangles_skipped: usize,
    /// Zero-area or repeated-index triangles deliberately skipped.
    pub degenerate_triangles_skipped: usize,
    /// Segments before duplicate removal.
    pub raw_segments: usize,
    /// Duplicate segments removed before contour reconstruction.
    pub duplicate_segments_removed: usize,
    /// Segments that met three or more others at one point, which a closed
    /// outline cannot: the mesh touches itself there.
    pub branch_points: usize,
    /// Outline fragments abandoned because no closing stage could join them.
    pub contours_dropped: usize,
    /// Endpoint pairs joined by one of the staged closing passes.
    pub gaps_closed: usize,
    /// Largest gap this layer needed closed, in nanometres; zero when none.
    pub gap_closed_nm: Coord,
    /// Closed contours reconstructed.
    pub contours_reconstructed: usize,
}

/// Closed contours produced at one Z coordinate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SliceLayer {
    /// The plane used to produce this layer.
    pub plane: SlicePlane,
    /// Reconstructed contours, wound as the mesh wound them: material on the
    /// left of the contour, so an outer boundary is counter-clockwise and a
    /// hole clockwise. Overlapping shells are left overlapping for the region
    /// builder to unite.
    pub contours: Vec<Contour>,
    /// Diagnostic counters for this operation.
    pub diagnostics: SliceDiagnostics,
}

impl SliceLayer {
    /// Exports this layer as a small, self-contained SVG drawing.
    ///
    /// The output uses nanometre coordinates in its viewBox and millimetres
    /// for physical width/height. Non-zero fill handles overlapping shells as
    /// a union and holes by their winding.
    pub fn to_svg(&self, padding_mm: f64) -> Result<String, SliceError> {
        use std::fmt::Write as _;

        if padding_mm < 0.0 {
            return Err(SliceError::NegativeSvgPadding);
        }
        let padding = mm_to_coord(padding_mm)? as i128;

        let mut min_x = None::<i128>;
        let mut min_y = None::<i128>;
        let mut max_x = None::<i128>;
        let mut max_y = None::<i128>;
        for point in self.contours.iter().flat_map(|contour| contour.points()) {
            let x = point.x() as i128;
            let y = point.y() as i128;
            min_x = Some(min_x.map_or(x, |value| value.min(x)));
            min_y = Some(min_y.map_or(y, |value| value.min(y)));
            max_x = Some(max_x.map_or(x, |value| value.max(x)));
            max_y = Some(max_y.map_or(y, |value| value.max(y)));
        }

        let (view_x, view_y, view_width, view_height) = match (min_x, min_y, max_x, max_y) {
            (Some(min_x), Some(min_y), Some(max_x), Some(max_y)) => {
                let width = (max_x - min_x + 2 * padding).max(1);
                let height = (max_y - min_y + 2 * padding).max(1);
                (min_x - padding, -max_y - padding, width, height)
            }
            _ => (0, 0, 1_000_000, 1_000_000),
        };

        let width_mm = view_width as f64 / 1_000_000.0;
        let height_mm = view_height as f64 / 1_000_000.0;
        let mut svg = String::new();
        writeln!(
            &mut svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width_mm:.6}mm" height="{height_mm:.6}mm" viewBox="{view_x} {view_y} {view_width} {view_height}">"#
        )
        .expect("writing to String cannot fail");
        writeln!(
            &mut svg,
            r##"  <path d="{}" fill="#dce7f5" fill-rule="nonzero" stroke="#172638" stroke-width="100000" stroke-linejoin="round"/>"##,
            self.contours
                .iter()
                .map(|contour| {
                    let mut path = String::new();
                    for (index, point) in contour.points().iter().enumerate() {
                        let command = if index == 0 { 'M' } else { 'L' };
                        write!(&mut path, "{command} {} {} ", point.x(), -point.y())
                            .expect("writing to String cannot fail");
                    }
                    path.push('Z');
                    path
                })
                .collect::<Vec<_>>()
                .join(" ")
        )
        .expect("writing to String cannot fail");
        svg.push_str("</svg>\n");
        Ok(svg)
    }
}

/// Errors raised when a mesh cannot be represented as closed slice contours.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SliceError {
    /// Coordinate conversion failed.
    Geometry(GeometryError),
    /// The intersection graph contains an endpoint with degree one.
    OpenContour {
        /// The endpoint which could not be joined.
        endpoint: Point2,
    },
    /// An intersection vertex has more than two incident segments.
    BranchingContour {
        /// The branching point.
        point: Point2,
        /// Number of incident segments.
        degree: usize,
    },
    /// A contour could not be constructed from the reconstructed loop.
    InvalidContour(GeometryError),
    /// Traversal exceeded the maximum possible number of edges.
    TraversalDidNotClose,
    /// SVG padding was negative.
    NegativeSvgPadding,
    /// The mesh or requested schedule has no positive Z extent.
    InvalidZRange {
        /// Minimum Z coordinate.
        z_min: Coord,
        /// Maximum Z coordinate.
        z_max: Coord,
    },
    /// First or regular layer height was zero or negative.
    NonPositiveLayerHeight,
    /// The schedule exceeded the defensive maximum layer count.
    TooManyLayers {
        /// Maximum supported layer count for one schedule.
        limit: usize,
    },
    /// Layer boundary arithmetic exceeded the coordinate representation.
    LayerCoordinateOverflow,
}

impl From<GeometryError> for SliceError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

impl fmt::Display for SliceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Geometry(error) => write!(formatter, "invalid slice plane: {error}"),
            Self::OpenContour { endpoint } => {
                write!(
                    formatter,
                    "open contour at ({}, {})",
                    endpoint.x(),
                    endpoint.y()
                )
            }
            Self::BranchingContour { point, degree } => write!(
                formatter,
                "branching contour at ({}, {}) with degree {degree}",
                point.x(),
                point.y()
            ),
            Self::InvalidContour(error) => {
                write!(formatter, "invalid reconstructed contour: {error}")
            }
            Self::TraversalDidNotClose => {
                formatter.write_str("contour traversal did not close within the segment limit")
            }
            Self::NegativeSvgPadding => formatter.write_str("SVG padding must not be negative"),
            Self::InvalidZRange { z_min, z_max } => write!(
                formatter,
                "invalid model Z range: minimum {z_min} must be below maximum {z_max}"
            ),
            Self::NonPositiveLayerHeight => {
                formatter.write_str("first-layer and regular layer heights must be positive")
            }
            Self::TooManyLayers { limit } => write!(
                formatter,
                "layer schedule exceeds the safety limit of {limit} layers"
            ),
            Self::LayerCoordinateOverflow => {
                formatter.write_str("layer schedule exceeds the supported coordinate range")
            }
        }
    }
}

impl std::error::Error for SliceError {}

/// Options that act while a plane's segments are assembled into contours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliceOptions {
    /// Largest endpoint gap the closing ladder may bridge, in millimetres.
    ///
    /// The ladder tries a nanometre, a micrometre, a tenth of a millimetre and
    /// a millimetre before this value, so a mesh whose facets nearly meet is
    /// closed at the smallest gap that works and the reported stage says how
    /// far the mesh was from watertight.
    pub close_gaps: f64,
}

impl Default for SliceOptions {
    fn default() -> SliceOptions {
        SliceOptions { close_gaps: 2.0 }
    }
}

/// The closing ladder, in millimetres, ending at [`SliceOptions::close_gaps`].
const GAP_LADDER: [f64; 4] = [0.0, 0.001, 0.1, 1.0];

/// Largest closing gap this crate accepts, in millimetres.
pub const MAX_CLOSE_GAPS: f64 = 10.0;

/// One triangle/plane crossing, directed so material lies on its left.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Directed {
    from: Point2,
    to: Point2,
}

impl Directed {
    fn key(self) -> Result<Segment2, GeometryError> {
        Segment2::new(self.from, self.to)
    }
}

/// Slices a mesh at one horizontal plane with the default closing ladder.
///
/// This version:
/// - uses integer mesh coordinates and integer output coordinates;
/// - treats a triangle/plane crossing using exact integer Z classification;
/// - rounds interpolated XY coordinates to the nearest internal unit;
/// - keeps each contour's winding, so a hole stays a hole and a second shell
///   stays material;
/// - joins open outlines in stages, nearest endpoints first;
/// - returns an error only for a gap the last stage cannot bridge.
///
/// Overlapping shells are left overlapping; uniting them is the region
/// builder's job.
pub fn slice_mesh(mesh: &Mesh, plane: SlicePlane) -> Result<SliceLayer, SliceError> {
    slice_mesh_with(mesh, plane, SliceOptions::default())
}

/// Slices a mesh at one plane with explicit closing options.
pub fn slice_mesh_with(
    mesh: &Mesh,
    plane: SlicePlane,
    options: SliceOptions,
) -> Result<SliceLayer, SliceError> {
    slice_mesh_candidates(
        mesh,
        plane,
        0..mesh.triangle_count(),
        mesh.triangle_count(),
        options,
    )
}

fn slice_mesh_candidates<I>(
    mesh: &Mesh,
    plane: SlicePlane,
    candidates: I,
    candidate_count: usize,
    options: SliceOptions,
) -> Result<SliceLayer, SliceError>
where
    I: IntoIterator<Item = usize>,
{
    let mut diagnostics = SliceDiagnostics {
        triangles_tested: candidate_count,
        ..SliceDiagnostics::default()
    };
    let close_gaps = options.close_gaps.clamp(0.0, MAX_CLOSE_GAPS);

    let mut unique_segments = BTreeMap::<Segment2, Directed>::new();

    for triangle_index in candidates {
        if mesh.triangle_is_degenerate(triangle_index) {
            diagnostics.degenerate_triangles_skipped += 1;
            continue;
        }
        let [a, b, c] = mesh.triangle_vertices(triangle_index);
        if a.z == plane.z && b.z == plane.z && c.z == plane.z {
            diagnostics.coplanar_triangles_skipped += 1;
            continue;
        }

        let mut points = Vec::<Point2>::with_capacity(3);
        for (first, second) in [(a, b), (b, c), (c, a)] {
            collect_edge_intersection(first, second, plane.z, &mut points)?;
        }
        deduplicate_points(&mut points);

        if points.len() == 2 {
            let segment = directed_chord(points[0], points[1], a, b, c);
            let key = segment.key()?;
            match unique_segments.entry(key) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(segment);
                }
                std::collections::btree_map::Entry::Occupied(slot) => {
                    if slot.get().to == segment.from {
                        slot.remove();
                    }
                }
            }
            diagnostics.raw_segments += 1;
            diagnostics.intersecting_triangles += 1;
        }
    }

    diagnostics.duplicate_segments_removed = diagnostics.raw_segments - unique_segments.len();

    let segments: Vec<Directed> = unique_segments.into_values().collect();
    let mut contours = stitch_contours(&segments, &mut diagnostics, close_gaps)?;

    contours.sort_by(|left, right| {
        right
            .signed_area2()
            .abs()
            .cmp(&left.signed_area2().abs())
            .then_with(|| left.points().cmp(right.points()))
    });

    diagnostics.contours_reconstructed = contours.len();

    Ok(SliceLayer {
        plane,
        contours,
        diagnostics,
    })
}

/// Slices a mesh at every midpoint in a validated layer schedule.
///
/// This is intentionally sequential and acts as the correctness reference for
/// future layer-parallel execution. It returns no partial result: if any layer
/// has an open or branching contour, the whole call returns that diagnostic.
pub fn slice_mesh_layers(
    mesh: &Mesh,
    schedule: &LayerSchedule,
) -> Result<Vec<SliceLayer>, SliceError> {
    slice_mesh_layers_lenient(mesh, schedule)
        .into_iter()
        .collect()
}

/// Slices every scheduled layer, keeping per-layer failures instead of aborting.
///
/// A single degenerate plane — a cone tip collapsing to a point, a layer that
/// grazes a horizontal face — should not discard the rest of the stack. The
/// caller decides whether a failed layer is a warning or a hard error; see
/// [`slice_mesh_layers`] for the strict form.
pub fn slice_mesh_layers_lenient(
    mesh: &Mesh,
    schedule: &LayerSchedule,
) -> Vec<Result<SliceLayer, SliceError>> {
    slice_mesh_layers_with(mesh, schedule, SliceOptions::default())
}

/// Slices every scheduled layer with explicit closing options, keeping failures.
pub fn slice_mesh_layers_with(
    mesh: &Mesh,
    schedule: &LayerSchedule,
    options: SliceOptions,
) -> Vec<Result<SliceLayer, SliceError>> {
    // Each triangle is assigned only to scheduled planes within its Z range.
    // Very tall triangles can overlap a large number of layers; cap the total
    // index references and use the simple reference path if bucketing would
    // consume too much memory. In either case output order is deterministic.
    const MAX_BUCKET_REFERENCES: usize = 2_000_000;
    let mut buckets = (0..schedule.len())
        .map(|_| Vec::<usize>::new())
        .collect::<Vec<_>>();
    let layer_specs = schedule.layers();
    let mut references = 0_usize;
    let mut should_fallback = false;

    for triangle_index in 0..mesh.triangle_count() {
        let [a, b, c] = mesh.triangle_vertices(triangle_index);
        let min_z = a.z.min(b.z).min(c.z);
        let max_z = a.z.max(b.z).max(c.z);
        let first = layer_specs.partition_point(|layer| layer.slice_z < min_z);
        let end = layer_specs.partition_point(|layer| layer.slice_z <= max_z);
        let count = end.saturating_sub(first);
        references = match references.checked_add(count) {
            Some(value) if value <= MAX_BUCKET_REFERENCES => value,
            _ => {
                should_fallback = true;
                break;
            }
        };
        for bucket in buckets.iter_mut().take(end).skip(first) {
            bucket.push(triangle_index);
        }
    }

    if should_fallback {
        return layer_specs
            .iter()
            .map(|layer| slice_mesh_with(mesh, SlicePlane { z: layer.slice_z }, options))
            .collect();
    }

    let mut layers = Vec::with_capacity(schedule.len());
    for (layer, candidates) in layer_specs.iter().zip(buckets.iter()) {
        layers.push(slice_mesh_candidates(
            mesh,
            SlicePlane { z: layer.slice_z },
            candidates.iter().copied(),
            candidates.len(),
            options,
        ));
    }
    layers
}

/// Orients a triangle/plane chord so that material lies on its left.
///
/// The chord is the intersection of the triangle's plane with a horizontal
/// plane, so it is perpendicular to the horizontal part of the triangle's
/// outward normal. That perpendicular, rotated a quarter turn, is the direction
/// a boundary of the cross-section runs when the solid is on its left; the two
/// endpoints only have to be put in that order.
fn directed_chord(p: Point2, q: Point2, a: Point3, b: Point3, c: Point3) -> Directed {
    let normal_x = (b.y as i128 - a.y as i128) * (c.z as i128 - a.z as i128)
        - (b.z as i128 - a.z as i128) * (c.y as i128 - a.y as i128);
    let normal_y = (b.z as i128 - a.z as i128) * (c.x as i128 - a.x as i128)
        - (b.x as i128 - a.x as i128) * (c.z as i128 - a.z as i128);
    let along_x = -normal_y;
    let along_y = normal_x;
    let dot = (q.x() as i128 - p.x() as i128) * along_x + (q.y() as i128 - p.y() as i128) * along_y;
    if dot > 0 {
        Directed { from: p, to: q }
    } else if dot < 0 {
        Directed { from: q, to: p }
    } else if p < q {
        Directed { from: p, to: q }
    } else {
        Directed { from: q, to: p }
    }
}

fn collect_edge_intersection(
    a: Point3,
    b: Point3,
    plane_z: Coord,
    output: &mut Vec<Point2>,
) -> Result<(), SliceError> {
    let a_on_plane = a.z == plane_z;
    let b_on_plane = b.z == plane_z;

    if a_on_plane && b_on_plane {
        output.push(Point2::new(a.x, a.y)?);
        output.push(Point2::new(b.x, b.y)?);
    } else if a_on_plane {
        output.push(Point2::new(a.x, a.y)?);
    } else if b_on_plane {
        output.push(Point2::new(b.x, b.y)?);
    } else if (a.z < plane_z && b.z > plane_z) || (a.z > plane_z && b.z < plane_z) {
        output.push(interpolate_edge(a, b, plane_z)?);
    }
    Ok(())
}

/// Interpolates the crossing of edge `a`-`b` with plane `plane_z`.
///
/// Endpoints are canonicalised first. Two triangles that share an edge see it
/// in opposite winding order, and rounding a negative offset is not the mirror
/// of rounding a positive one, so without canonicalisation the same crossing
/// can come back one nanometre apart and the contour fails to close.
fn interpolate_edge(a: Point3, b: Point3, plane_z: Coord) -> Result<Point2, SliceError> {
    let (a, b) = if (a.x, a.y, a.z) <= (b.x, b.y, b.z) {
        (a, b)
    } else {
        (b, a)
    };
    let z_delta = (b.z - a.z) as i128;
    let z_offset = (plane_z - a.z) as i128;

    let x_delta = (b.x - a.x) as i128;
    let y_delta = (b.y - a.y) as i128;

    let x_offset = div_round_nearest(x_delta * z_offset, z_delta);
    let y_offset = div_round_nearest(y_delta * z_offset, z_delta);

    Ok(Point2::new(
        (a.x as i128 + x_offset) as Coord,
        (a.y as i128 + y_offset) as Coord,
    )?)
}

fn div_round_nearest(numerator: i128, denominator: i128) -> i128 {
    debug_assert_ne!(denominator, 0);

    let quotient = numerator / denominator;
    let remainder = numerator % denominator;

    if remainder.abs() * 2 >= denominator.abs() {
        quotient
            + if numerator.signum() == denominator.signum() {
                1
            } else {
                -1
            }
    } else {
        quotient
    }
}

fn deduplicate_points(points: &mut Vec<Point2>) {
    let mut unique = Vec::with_capacity(points.len());
    for point in points.iter().copied() {
        if !unique.contains(&point) {
            unique.push(point);
        }
    }
    *points = unique;
}

fn stitch_contours(
    segments: &[Directed],
    diagnostics: &mut SliceDiagnostics,
    close_gaps: f64,
) -> Result<Vec<Contour>, SliceError> {
    if segments.is_empty() {
        return Ok(Vec::new());
    }

    let mut outgoing = BTreeMap::<Point2, Vec<usize>>::new();
    let mut incoming_count = BTreeMap::<Point2, usize>::new();
    for (index, segment) in segments.iter().enumerate() {
        outgoing.entry(segment.from).or_default().push(index);
        *incoming_count.entry(segment.to).or_default() += 1;
    }

    let mut used = vec![false; segments.len()];
    for (point, indices) in &outgoing {
        let arrivals = incoming_count.get(point).copied().unwrap_or(0);
        if arrivals + indices.len() > 2 {
            diagnostics.branch_points += 1;
        }
    }
    let mut open_started: Vec<Point2> = outgoing
        .keys()
        .copied()
        .filter(|point| incoming_count.get(point).copied().unwrap_or(0) == 0)
        .collect();
    open_started.sort();

    let mut trails: Vec<(Vec<Point2>, bool)> = Vec::new();
    for start in open_started {
        if let Some(trail) = walk(start, segments, &outgoing, &mut used) {
            trails.push(trail);
        }
    }
    for indices in outgoing.values() {
        for &index in indices {
            if used[index] {
                continue;
            }
            if let Some(trail) = walk(segments[index].from, segments, &outgoing, &mut used) {
                trails.push(trail);
            }
        }
    }

    let mut contours: Vec<Contour> = Vec::new();
    let mut open: Vec<Vec<Point2>> = Vec::new();
    let mut hangs: Vec<bool> = Vec::new();
    for (trail, chose_at_junction) in trails {
        if trail.len() >= 4 && trail.first() == trail.last() {
            contours.push(Contour::new(trail).map_err(SliceError::InvalidContour)?);
        } else {
            let start_junction = outgoing
                .get(&trail[0])
                .is_some_and(|indices| indices.len() > 1);
            hangs.push(chose_at_junction || start_junction);
            open.push(trail);
        }
    }

    let limit = close_gaps.clamp(0.0, MAX_CLOSE_GAPS);
    let mut stages: Vec<f64> = GAP_LADDER
        .iter()
        .copied()
        .filter(|stage| *stage <= limit)
        .collect();
    if stages.last().is_none_or(|last| *last < limit) {
        stages.push(limit);
    }

    let mut closed: Vec<bool> = vec![false; open.len()];
    let mut dropped: Vec<bool> = vec![false; open.len()];
    loop {
        for stage in &stages {
            let budget = mm_to_coord(*stage)?;
            let budget_squared = (budget as i128) * (budget as i128);
            loop {
                let mut best: Option<(i128, Point2, Point2, usize, usize)> = None;
                for i in 0..open.len() {
                    if closed[i] {
                        continue;
                    }
                    let end = *open[i].last().expect("a trail has a point");
                    for j in 0..open.len() {
                        if closed[j] {
                            continue;
                        }
                        let start = open[j][0];
                        let distance = gap_squared(end, start);
                        if distance > budget_squared {
                            continue;
                        }
                        let candidate = (distance, end, start, i, j);
                        if best.is_none_or(|current| candidate < current) {
                            best = Some(candidate);
                        }
                    }
                }
                let Some((distance, _, _, i, j)) = best else {
                    break;
                };
                if i == j {
                    closed[i] = true;
                } else {
                    let tail = std::mem::take(&mut open[j]);
                    open[i].extend(tail.into_iter().skip(1));
                    hangs[i] |= hangs[j];
                    closed[j] = true;
                }
                if distance > 0 {
                    diagnostics.gaps_closed += 1;
                    diagnostics.gap_closed_nm = diagnostics.gap_closed_nm.max(budget);
                }
            }
            if closed.iter().all(|done| *done) {
                break;
            }
        }
        if closed.iter().all(|done| *done) {
            break;
        }
        let fragment = (0..open.len()).find(|index| !closed[*index] && hangs[*index]);
        let Some(index) = fragment else {
            diagnostics.contours_dropped += 1;
            let index = (0..open.len())
                .find(|index| !closed[*index])
                .expect("a trail is still open");
            return Err(SliceError::OpenContour {
                endpoint: *open[index].last().expect("a trail has a point"),
            });
        };
        closed[index] = true;
        dropped[index] = true;
        diagnostics.contours_dropped += 1;
    }

    for (index, trail) in open.into_iter().enumerate() {
        if dropped[index] || trail.len() < 3 {
            continue;
        }
        contours.push(Contour::new(trail).map_err(SliceError::InvalidContour)?);
    }

    Ok(contours)
}

/// Squared distance between two lattice points, without overflowing `i64`.
fn gap_squared(a: Point2, b: Point2) -> i128 {
    let dx = b.x() as i128 - a.x() as i128;
    let dy = b.y() as i128 - a.y() as i128;
    dx * dx + dy * dy
}

/// How a branch is ranked: it continues somewhere, it turns least, it comes first.
type Choice = (bool, i128, std::cmp::Reverse<usize>);

/// Follows unused segments from `start`, keeping the straightest way at a branch.
///
/// The flag is true when the walk had to choose between two unused segments, so
/// the trail it produced is decided by a heuristic and cannot be trusted as part
/// of a gap that is later closed.
fn walk(
    start: Point2,
    segments: &[Directed],
    outgoing: &BTreeMap<Point2, Vec<usize>>,
    used: &mut [bool],
) -> Option<(Vec<Point2>, bool)> {
    if used.iter().all(|done| *done) {
        return None;
    }
    let mut points = vec![start];
    let mut current = start;
    let mut direction: Option<(i128, i128)> = None;
    let mut chose = false;
    while let Some(indices) = outgoing.get(&current) {
        let mut best: Option<(Choice, usize)> = None;
        let mut seen = 0_usize;
        let mut only = 0_usize;
        for &index in indices {
            if used[index] {
                continue;
            }
            seen += 1;
            only = index;
            if seen > 1 {
                break;
            }
        }
        if seen == 0 {
            break;
        }
        if seen == 1 {
            let to = segments[only].to;
            used[only] = true;
            direction = Some((
                to.x() as i128 - current.x() as i128,
                to.y() as i128 - current.y() as i128,
            ));
            points.push(to);
            if to == start {
                break;
            }
            current = to;
            if points.len() > segments.len() + 1 {
                break;
            }
            continue;
        }
        chose = true;
        for &index in indices {
            if used[index] {
                continue;
            }
            let to = segments[index].to;
            let step = (
                to.x() as i128 - current.x() as i128,
                to.y() as i128 - current.y() as i128,
            );
            let score = match direction {
                Some(dir) => dir.0 * step.0 + dir.1 * step.1,
                None => 0,
            };
            let continues = outgoing
                .get(&to)
                .is_some_and(|next| next.iter().any(|other| !used[*other]));
            let key = (continues, score, std::cmp::Reverse(index));
            if best.is_none_or(|(current_key, _)| key > current_key) {
                best = Some((key, index));
            }
        }
        let Some((_, index)) = best else {
            break;
        };
        used[index] = true;
        let to = segments[index].to;
        direction = Some((
            to.x() as i128 - current.x() as i128,
            to.y() as i128 - current.y() as i128,
        ));
        points.push(to);
        if to == start {
            break;
        }
        current = to;
        if points.len() > segments.len() + 1 {
            break;
        }
    }
    Some((points, chose))
}
#[cfg(test)]
mod tests {
    use super::*;
    use hfw_mesh::Mesh;

    fn height_mm(layer: LayerSpec) -> f64 {
        (layer.top_z - layer.bottom_z) as f64 / 1e6
    }

    #[test]
    fn twenty_mm_cube_has_one_hundred_regular_layers() {
        let schedule = LayerSchedule::from_mm(0.0, 20.0, 0.2, 0.2).expect("valid layer schedule");
        assert_eq!(schedule.len(), 100);
        assert_eq!(schedule.layers()[0].bottom_z, 0);
        assert_eq!(schedule.layers()[0].top_z, 200_000);
        assert_eq!(schedule.layers()[0].slice_z, 100_000);
        assert_eq!(schedule.layers()[99].top_z, 20_000_000);
        assert_eq!(schedule.layers()[99].slice_z, 19_900_000);
    }

    #[test]
    fn first_layer_can_differ_and_last_layer_can_be_shorter() {
        let schedule = LayerSchedule::from_mm(0.0, 1.0, 0.3, 0.4).expect("valid variable schedule");
        assert_eq!(schedule.len(), 3);
        assert_eq!(schedule.layers()[0].top_z, 300_000);
        assert_eq!(schedule.layers()[1].top_z, 700_000);
        assert_eq!(schedule.layers()[2].bottom_z, 700_000);
        assert_eq!(schedule.layers()[2].top_z, 1_000_000);
        assert_eq!(
            schedule.layers()[2].top_z - schedule.layers()[2].bottom_z,
            300_000
        );
    }

    #[test]
    fn a_profile_changes_the_height_part_way_up_and_clamps_to_the_bounds() {
        let schedule = LayerSchedule::from_profile(
            0,
            2_000_000,
            300_000,
            200_000,
            &[(0.0, 0.2), (1.0, 0.1), (1.5, 0.5)],
            0.12,
            0.28,
        )
        .expect("valid profile schedule");
        let heights: Vec<f64> = schedule.layers().iter().map(|l| height_mm(*l)).collect();
        assert!(
            (heights[0] - 0.3).abs() < 1e-9,
            "the first layer kept its own height"
        );
        assert!(
            heights
                .iter()
                .skip(1)
                .all(|h| (0.12..=0.28 + 1e-9).contains(h)),
            "every later layer stayed inside the bounds"
        );
        assert!(
            heights.iter().any(|h| (h - 0.12).abs() < 1e-9),
            "the 0.1 step was clamped up to the floor"
        );
        assert!(
            heights.iter().any(|h| (h - 0.28).abs() < 1e-9),
            "the 0.5 step was clamped down to the ceiling"
        );
        let top = schedule.layers().last().expect("layers").top_z;
        assert_eq!(top, 2_000_000, "the stack still ends on z_max");
    }

    #[test]
    fn a_final_layer_below_the_floor_is_folded_into_its_neighbour() {
        let schedule =
            LayerSchedule::from_profile(0, 1_050_000, 200_000, 200_000, &[(0.0, 0.2)], 0.1, 0.3)
                .expect("valid profile");
        let heights: Vec<f64> = schedule.layers().iter().map(|l| height_mm(*l)).collect();
        assert_eq!(
            schedule.layers().last().expect("a last layer").top_z,
            1_050_000
        );
        assert!(
            heights.iter().all(|h| *h >= 0.1 - 1e-9),
            "a 0.05 mm layer survived: {heights:?}"
        );
        assert_eq!(heights.len(), 5);
    }

    #[test]
    fn an_empty_profile_falls_back_to_a_uniform_stack() {
        let uniform = LayerSchedule::from_mm(0.0, 1.0, 0.2, 0.2).expect("uniform");
        let from_empty = LayerSchedule::from_profile(0, 1_000_000, 200_000, 200_000, &[], 0.1, 0.3)
            .expect("empty profile");
        assert_eq!(uniform, from_empty);
    }

    #[test]
    fn a_profile_out_of_order_is_sorted_and_a_bad_bound_is_rejected() {
        let sorted = LayerSchedule::from_profile(
            0,
            1_000_000,
            200_000,
            200_000,
            &[(0.6, 0.1), (0.4, 0.2)],
            0.1,
            0.3,
        )
        .expect("sorted profile");
        let heights: Vec<f64> = sorted.layers().iter().map(|l| height_mm(*l)).collect();
        assert!((heights[1] - 0.2).abs() < 1e-9);
        assert!((heights[3] - 0.1).abs() < 1e-9);
        assert_eq!(
            LayerSchedule::from_profile(0, 1_000_000, 200_000, 200_000, &[(0.0, 0.2)], 0.3, 0.1),
            Err(SliceError::NonPositiveLayerHeight)
        );
    }

    #[test]
    fn layer_schedule_rejects_zero_or_negative_heights_and_bad_ranges() {
        assert_eq!(
            LayerSchedule::from_mm(0.0, 1.0, 0.0, 0.2),
            Err(SliceError::NonPositiveLayerHeight)
        );
        assert_eq!(
            LayerSchedule::from_mm(2.0, 1.0, 0.2, 0.2),
            Err(SliceError::InvalidZRange {
                z_min: 2_000_000,
                z_max: 1_000_000,
            })
        );
    }

    #[test]
    fn schedule_slices_every_layer_and_preserves_cube_area() {
        let mesh = Mesh::box_mm(20.0, 20.0, 20.0).expect("cube");
        let schedule = LayerSchedule::from_mm(0.0, 20.0, 0.2, 0.2).expect("schedule");
        let layers = slice_mesh_layers(&mesh, &schedule).expect("all layers slice");
        assert_eq!(layers.len(), 100);
        assert!(layers.iter().all(|layer| layer.contours.len() == 1));
        assert!(layers
            .iter()
            .all(|layer| (layer.contours[0].area_mm2() - 400.0).abs() < 1e-9));
    }

    #[test]
    fn slices_twenty_millimetre_cube_into_one_four_hundred_square_mm_contour() {
        let mesh = Mesh::box_mm(20.0, 20.0, 20.0).expect("cube");
        let layer =
            slice_mesh(&mesh, SlicePlane::from_mm(10.0).expect("plane")).expect("closed slice");

        assert_eq!(layer.contours.len(), 1);
        assert!(layer.contours[0].is_ccw());
        assert!((layer.contours[0].area_mm2() - 400.0).abs() < 1e-9);
        assert_eq!(layer.diagnostics.contours_reconstructed, 1);
    }

    #[test]
    fn slicing_at_bottom_and_top_planes_is_well_defined() {
        let mesh = Mesh::box_mm(20.0, 20.0, 20.0).expect("cube");

        for z in [0.0, 20.0] {
            let layer = slice_mesh(&mesh, SlicePlane::from_mm(z).expect("plane"))
                .expect("closed boundary slice");
            assert_eq!(layer.contours.len(), 1, "z={z}");
            assert!((layer.contours[0].area_mm2() - 400.0).abs() < 1e-9);
        }
    }

    fn combine_boxes(boxes: &[(f64, f64, f64, f64, f64, f64)]) -> Mesh {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();

        for &(x, y, z, width, depth, height) in boxes {
            let mesh = Mesh::box_mm(width, depth, height).expect("box");
            let base = vertices.len() as u32;
            for point in mesh.vertices() {
                vertices.push(Point3::new(
                    point.x + (x * 1_000_000.0) as i64,
                    point.y + (y * 1_000_000.0) as i64,
                    point.z + (z * 1_000_000.0) as i64,
                ));
            }
            for triangle in mesh.triangles() {
                triangles.push(hfw_mesh::Triangle([
                    base + triangle.0[0],
                    base + triangle.0[1],
                    base + triangle.0[2],
                ]));
            }
        }

        Mesh::new(vertices, triangles).expect("combined boxes")
    }

    #[test]
    fn reconstructs_multiple_disconnected_islands() {
        let mesh = combine_boxes(&[
            (0.0, 0.0, 0.0, 10.0, 10.0, 10.0),
            (20.0, 0.0, 0.0, 10.0, 10.0, 10.0),
        ]);
        let layer =
            slice_mesh(&mesh, SlicePlane::from_mm(5.0).expect("plane")).expect("two islands");

        assert_eq!(layer.contours.len(), 2);
        assert!(layer.contours.iter().all(Contour::is_ccw));
        assert!((layer.contours.iter().map(Contour::area_mm2).sum::<f64>() - 200.0).abs() < 1e-9);
    }

    fn chord(x1: i64, y1: i64, x2: i64, y2: i64) -> Directed {
        Directed {
            from: Point2::new(x1 * 1_000_000, y1 * 1_000_000).expect("point"),
            to: Point2::new(x2 * 1_000_000, y2 * 1_000_000).expect("point"),
        }
    }

    #[test]
    fn a_self_touching_outline_keeps_its_closed_part() {
        let segments = vec![
            chord(0, 0, 10, 0),
            chord(10, 0, 10, 10),
            chord(10, 10, 0, 10),
            chord(0, 10, 0, 0),
            chord(10, 0, 20, 0),
        ];
        let mut diagnostics = SliceDiagnostics::default();
        let contours =
            stitch_contours(&segments, &mut diagnostics, 2.0).expect("a closed mesh is walked");

        assert_eq!(diagnostics.branch_points, 1);
        assert_eq!(contours.len(), 1);
        assert!((contours[0].area_mm2() - 100.0).abs() < 1e-6);
        assert_eq!(diagnostics.contours_dropped, 1);
    }

    #[test]
    fn an_open_outline_is_an_error_until_the_gap_limit_reaches_it() {
        let segments = vec![
            chord(0, 0, 10, 0),
            chord(10, 0, 10, 10),
            chord(10, 10, 0, 10),
        ];
        let mut diagnostics = SliceDiagnostics::default();
        let error = stitch_contours(&segments, &mut diagnostics, 2.0)
            .expect_err("a 10 mm gap is not guessed away");
        assert!(matches!(error, SliceError::OpenContour { .. }), "{error}");

        let mut diagnostics = SliceDiagnostics::default();
        let contours = stitch_contours(&segments, &mut diagnostics, 10.0)
            .expect("a limit of 10 mm bridges a 10 mm gap");
        assert_eq!(contours.len(), 1);
        assert!((contours[0].area_mm2() - 100.0).abs() < 1e-6);
        assert_eq!(diagnostics.gaps_closed, 1);
        assert_eq!(diagnostics.gap_closed_nm, 10_000_000);
    }

    #[test]
    fn a_hairline_gap_closes_at_the_first_stage() {
        let segments = vec![
            chord(0, 0, 10, 0),
            chord(10, 0, 10, 10),
            chord(10, 10, 0, 10),
            Directed {
                from: Point2::new(0, 10_000_000).expect("point"),
                to: Point2::new(0, 1_000).expect("point"),
            },
        ];
        let mut diagnostics = SliceDiagnostics::default();
        let contours = stitch_contours(&segments, &mut diagnostics, 2.0)
            .expect("a one micrometre gap is closed");
        assert_eq!(contours.len(), 1);
        assert_eq!(diagnostics.gaps_closed, 1);
        assert_eq!(diagnostics.gap_closed_nm, 1_000);
        assert!((contours[0].area_mm2() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn nested_contours_keep_the_winding_the_mesh_gave_them() {
        let mesh = combine_boxes(&[
            (0.0, 0.0, 0.0, 30.0, 30.0, 10.0),
            (10.0, 10.0, 0.0, 10.0, 10.0, 10.0),
        ]);
        let layer =
            slice_mesh(&mesh, SlicePlane::from_mm(5.0).expect("plane")).expect("nested contours");

        assert_eq!(layer.contours.len(), 2);
        assert!(layer.contours.iter().all(Contour::is_ccw));
        assert!((layer.contours[0].area_mm2() - 900.0).abs() < 1e-9);
        assert!((layer.contours[1].area_mm2() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn open_mesh_is_reported_instead_of_silently_dropping_a_wall() {
        let mesh = Mesh::new(
            vec![
                Point3::from_mm(0.0, 0.0, 0.0).expect("point"),
                Point3::from_mm(10.0, 0.0, 10.0).expect("point"),
                Point3::from_mm(0.0, 10.0, 10.0).expect("point"),
            ],
            vec![hfw_mesh::Triangle([0, 1, 2])],
        )
        .expect("mesh");

        let result = slice_mesh(&mesh, SlicePlane::from_mm(5.0).expect("plane"));
        assert!(matches!(result, Err(SliceError::OpenContour { .. })));
    }

    #[test]
    fn degenerate_triangles_are_skipped_and_reported() {
        let mut vertices = Mesh::box_mm(20.0, 20.0, 20.0)
            .expect("cube")
            .vertices()
            .to_vec();
        let mut triangles = Mesh::box_mm(20.0, 20.0, 20.0)
            .expect("cube")
            .triangles()
            .to_vec();
        let base = vertices.len() as u32;
        vertices.extend([
            Point3::from_mm(30.0, 0.0, 0.0).expect("point"),
            Point3::from_mm(31.0, 0.0, 5.0).expect("point"),
            Point3::from_mm(32.0, 0.0, 10.0).expect("point"),
        ]);
        triangles.push(hfw_mesh::Triangle([base, base + 1, base + 2]));

        let mesh = Mesh::new(vertices, triangles).expect("mesh");
        let layer =
            slice_mesh(&mesh, SlicePlane::from_mm(5.0).expect("plane")).expect("cube slice");

        assert_eq!(layer.contours.len(), 1);
        assert_eq!(layer.diagnostics.degenerate_triangles_skipped, 1);
    }

    #[test]
    fn exports_svg_with_physical_dimensions_and_closed_path() {
        let mesh = Mesh::box_mm(20.0, 10.0, 5.0).expect("box");
        let layer = slice_mesh(&mesh, SlicePlane::from_mm(2.5).expect("plane")).expect("slice");
        let svg = layer.to_svg(1.0).expect("SVG");
        assert!(svg.contains("width=\"22.000000mm\""));
        assert!(svg.contains("height=\"12.000000mm\""));
        assert!(svg.contains("fill-rule=\"nonzero\""));
        assert!(svg.contains('Z'));
    }

    #[test]
    fn empty_mesh_produces_an_empty_layer() {
        let mesh = Mesh::default();
        let layer =
            slice_mesh(&mesh, SlicePlane::from_mm(0.2).expect("plane")).expect("empty slice");
        assert!(layer.contours.is_empty());
        assert_eq!(layer.diagnostics.triangles_tested, 0);
    }
}

#[cfg(test)]
mod acceleration_tests {
    use super::*;
    use hfw_mesh::Mesh;

    #[test]
    fn bucketed_layer_stack_matches_the_single_plane_reference() {
        let mesh = Mesh::box_mm(20.0, 13.0, 7.3).expect("box");
        let schedule = LayerSchedule::from_mm(0.0, 7.3, 0.2, 0.17).expect("schedule");
        let accelerated = slice_mesh_layers(&mesh, &schedule).expect("bucketed stack");
        assert_eq!(accelerated.len(), schedule.len());
        for (spec, layer) in schedule.layers().iter().zip(accelerated.iter()) {
            let reference = slice_mesh(&mesh, SlicePlane { z: spec.slice_z }).expect("reference");
            assert_eq!(layer.plane, reference.plane);
            assert_eq!(layer.contours, reference.contours);
            assert!(layer.diagnostics.triangles_tested <= mesh.triangle_count());
        }
        assert!(accelerated.iter().all(|layer| layer.contours.len() == 1));
    }
}
