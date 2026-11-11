//! Deterministic 2D geometry primitives for slicing.
//!
//! XY coordinates are stored as signed integer nanometres (`1 mm = 1_000_000`
//! units). Floating-point values are accepted only at explicit conversion
//! boundaries. Arithmetic used for orientation and area is performed in `i128`.

#![forbid(unsafe_code)]

use std::fmt;

/// Number of internal coordinate units in one millimetre.
pub const UNITS_PER_MM: i64 = 1_000_000;

/// Absolute coordinate limit in internal units (1,000,000 mm).
///
/// This intentionally leaves substantial headroom for `i128` cross products
/// while rejecting corrupt or implausible model coordinates early.
pub const MAX_ABS_COORD: i64 = 1_000_000_000_000;

/// A signed integer coordinate in nanometres.
pub type Coord = i64;

/// A point in the XY plane.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point2 {
    x: Coord,
    y: Coord,
}

impl Point2 {
    /// Creates a point from checked internal integer coordinates.
    pub fn new(x: Coord, y: Coord) -> Result<Self, GeometryError> {
        if !(-MAX_ABS_COORD..=MAX_ABS_COORD).contains(&x)
            || !(-MAX_ABS_COORD..=MAX_ABS_COORD).contains(&y)
        {
            return Err(GeometryError::CoordinateOutOfRange);
        }
        Ok(Self { x, y })
    }

    /// Returns the X coordinate in nanometres.
    pub const fn x(self) -> Coord {
        self.x
    }

    /// Returns the Y coordinate in nanometres.
    pub const fn y(self) -> Coord {
        self.y
    }

    /// Converts millimetres to integer nanometres, rejecting invalid values.
    pub fn from_mm(x: f64, y: f64) -> Result<Self, GeometryError> {
        Self::new(mm_to_coord(x)?, mm_to_coord(y)?)
    }

    /// Returns the point in millimetres.
    pub fn to_mm(self) -> [f64; 2] {
        [
            self.x as f64 / UNITS_PER_MM as f64,
            self.y as f64 / UNITS_PER_MM as f64,
        ]
    }
}

/// Converts one millimetre value into the checked integer coordinate domain.
pub fn mm_to_coord(mm: f64) -> Result<Coord, GeometryError> {
    if !mm.is_finite() {
        return Err(GeometryError::NonFiniteCoordinate);
    }

    let scaled = mm * UNITS_PER_MM as f64;
    if !scaled.is_finite() || scaled.abs() > MAX_ABS_COORD as f64 {
        return Err(GeometryError::CoordinateOutOfRange);
    }

    Ok(scaled.round() as Coord)
}

/// A non-degenerate line segment with canonical endpoint order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Segment2 {
    a: Point2,
    b: Point2,
}

impl Segment2 {
    /// Creates a segment and canonicalizes its endpoint order.
    pub fn new(a: Point2, b: Point2) -> Result<Self, GeometryError> {
        if a == b {
            return Err(GeometryError::ZeroLengthSegment);
        }

        let (a, b) = if a < b { (a, b) } else { (b, a) };
        Ok(Self { a, b })
    }

    /// Returns the lexicographically smaller endpoint.
    pub const fn a(self) -> Point2 {
        self.a
    }

    /// Returns the lexicographically larger endpoint.
    pub const fn b(self) -> Point2 {
        self.b
    }

    /// Returns the other endpoint, or `None` if `point` is not on this segment.
    pub fn other_endpoint(self, point: Point2) -> Option<Point2> {
        if point == self.a {
            Some(self.b)
        } else if point == self.b {
            Some(self.a)
        } else {
            None
        }
    }
}

/// A simple ordered contour. The first point is not repeated at the end.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Contour {
    points: Vec<Point2>,
}

impl Contour {
    /// Creates a contour, removing consecutive duplicates and a repeated
    /// closing point. Self-intersections are retained for the caller to detect.
    pub fn new(points: Vec<Point2>) -> Result<Self, GeometryError> {
        let mut cleaned = Vec::with_capacity(points.len());

        for point in points {
            if cleaned.last().copied() != Some(point) {
                cleaned.push(point);
            }
        }

        if cleaned.len() > 1 && cleaned.first() == cleaned.last() {
            cleaned.pop();
        }

        if cleaned.len() < 3 {
            return Err(GeometryError::TooFewContourPoints);
        }

        if cleaned.iter().any(|point| {
            point.x < -MAX_ABS_COORD
                || point.x > MAX_ABS_COORD
                || point.y < -MAX_ABS_COORD
                || point.y > MAX_ABS_COORD
        }) {
            return Err(GeometryError::CoordinateOutOfRange);
        }

        let contour = Self { points: cleaned };
        if contour.signed_area2() == 0 {
            return Err(GeometryError::ZeroAreaContour);
        }

        Ok(contour)
    }

    /// Returns the contour vertices without a repeated closing vertex.
    pub fn points(&self) -> &[Point2] {
        &self.points
    }

    /// Returns the number of vertices.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Returns true when the contour has no vertices.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Returns twice the signed area in square internal-coordinate units.
    ///
    /// Positive values mean counter-clockwise winding.
    pub fn signed_area2(&self) -> i128 {
        let mut area = 0_i128;
        for index in 0..self.points.len() {
            let a = self.points[index];
            let b = self.points[(index + 1) % self.points.len()];
            area += (a.x as i128) * (b.y as i128) - (b.x as i128) * (a.y as i128);
        }
        area
    }

    /// Returns the signed area in mm².
    pub fn signed_area_mm2(&self) -> f64 {
        self.signed_area2() as f64 / (2.0 * UNITS_PER_MM as f64 * UNITS_PER_MM as f64)
    }

    /// Returns the absolute area in mm².
    pub fn area_mm2(&self) -> f64 {
        self.signed_area_mm2().abs()
    }

    /// Returns true when this contour is counter-clockwise.
    pub fn is_ccw(&self) -> bool {
        self.signed_area2() > 0
    }

    /// Reverses and rotates the contour into a deterministic representation.
    ///
    /// The lexicographically smallest point becomes the first point.
    pub fn normalize_winding(&mut self, counter_clockwise: bool) {
        if self.is_ccw() != counter_clockwise {
            self.points.reverse();
        }

        if let Some((minimum_index, _)) = self
            .points
            .iter()
            .enumerate()
            .min_by_key(|(_, point)| **point)
        {
            self.points.rotate_left(minimum_index);
        }
    }

    /// Classifies a point relative to this contour.
    pub fn locate(&self, point: Point2) -> PointLocation {
        let mut inside = false;

        for index in 0..self.points.len() {
            let a = self.points[index];
            let b = self.points[(index + 1) % self.points.len()];

            let cross = orient(a, b, point);
            if cross == 0
                && point.x >= a.x.min(b.x)
                && point.x <= a.x.max(b.x)
                && point.y >= a.y.min(b.y)
                && point.y <= a.y.max(b.y)
            {
                return PointLocation::Boundary;
            }

            let crosses_ray = (a.y > point.y) != (b.y > point.y);
            if crosses_ray {
                // For a horizontal ray to +X, the sign of this determinant
                // tells which side of the edge the ray intersection occupies.
                if (b.y > a.y && cross > 0) || (b.y < a.y && cross < 0) {
                    inside = !inside;
                }
            }
        }

        if inside {
            PointLocation::Inside
        } else {
            PointLocation::Outside
        }
    }

    /// Returns the axis-aligned bounding box.
    pub fn bounds(&self) -> Aabb2 {
        let first = self.points[0];
        let mut bounds = Aabb2 {
            min: first,
            max: first,
        };

        for point in self.points.iter().copied().skip(1) {
            bounds.expand(point);
        }
        bounds
    }
}

/// Location of a point relative to a contour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointLocation {
    /// The point is outside the contour.
    Outside,
    /// The point is strictly inside the contour.
    Inside,
    /// The point lies on the contour boundary.
    Boundary,
}

/// Axis-aligned bounding box in integer XY coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Aabb2 {
    /// Minimum corner.
    pub min: Point2,
    /// Maximum corner.
    pub max: Point2,
}

impl Aabb2 {
    /// Expands the bounds to include a point.
    pub fn expand(&mut self, point: Point2) {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
    }

    /// Returns width in internal coordinate units.
    pub fn width(&self) -> i128 {
        self.max.x as i128 - self.min.x as i128
    }

    /// Returns height in internal coordinate units.
    pub fn height(&self) -> i128 {
        self.max.y as i128 - self.min.y as i128
    }
}

/// Exact orientation determinant for three integer points.
pub fn orient(a: Point2, b: Point2, c: Point2) -> i128 {
    (b.x as i128 - a.x as i128) * (c.y as i128 - a.y as i128)
        - (b.y as i128 - a.y as i128) * (c.x as i128 - a.x as i128)
}

/// Errors raised while constructing or converting geometry primitives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeometryError {
    /// A coordinate was NaN or infinite.
    NonFiniteCoordinate,
    /// A coordinate exceeds the supported model space.
    CoordinateOutOfRange,
    /// A segment has identical endpoints.
    ZeroLengthSegment,
    /// A contour has fewer than three distinct vertices.
    TooFewContourPoints,
    /// A contour has zero signed area.
    ZeroAreaContour,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NonFiniteCoordinate => "coordinate is not finite",
            Self::CoordinateOutOfRange => "coordinate is outside the supported range",
            Self::ZeroLengthSegment => "segment endpoints are identical",
            Self::TooFewContourPoints => "contour requires at least three distinct points",
            Self::ZeroAreaContour => "contour has zero signed area",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GeometryError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: Coord, y: Coord) -> Point2 {
        Point2::new(x, y).expect("point in supported coordinate range")
    }

    fn square() -> Contour {
        Contour::new(vec![
            p(0, 0),
            p(10_000_000, 0),
            p(10_000_000, 10_000_000),
            p(0, 10_000_000),
        ])
        .expect("valid square")
    }

    #[test]
    fn millimetre_conversion_is_nanometre_scaled() {
        let point = Point2::from_mm(12.345678, -0.000001).expect("valid point");
        assert_eq!(point, p(12_345_678, -1));
        assert_eq!(point.to_mm(), [12.345678, -0.000001]);
    }

    #[test]
    fn rejects_non_finite_and_out_of_range_coordinates() {
        assert_eq!(
            mm_to_coord(f64::NAN),
            Err(GeometryError::NonFiniteCoordinate)
        );
        assert_eq!(
            mm_to_coord(f64::INFINITY),
            Err(GeometryError::NonFiniteCoordinate)
        );
        assert_eq!(
            mm_to_coord(1_000_001.0),
            Err(GeometryError::CoordinateOutOfRange)
        );
    }

    #[test]
    fn point_constructor_enforces_coordinate_bounds() {
        assert_eq!(
            Point2::new(MAX_ABS_COORD + 1, 0),
            Err(GeometryError::CoordinateOutOfRange)
        );
    }

    #[test]
    fn square_area_and_winding_are_exact() {
        let contour = square();
        assert_eq!(contour.signed_area2(), 200_000_000_000_000);
        assert_eq!(contour.area_mm2(), 100.0);
        assert!(contour.is_ccw());
    }

    #[test]
    fn point_location_distinguishes_inside_outside_and_boundary() {
        let contour = square();
        assert_eq!(
            contour.locate(p(5_000_000, 5_000_000)),
            PointLocation::Inside
        );
        assert_eq!(
            contour.locate(p(15_000_000, 5_000_000)),
            PointLocation::Outside
        );
        assert_eq!(
            contour.locate(p(10_000_000, 5_000_000)),
            PointLocation::Boundary
        );
    }

    #[test]
    fn contour_normalization_is_deterministic() {
        let mut contour =
            Contour::new(vec![p(10, 10), p(0, 10), p(0, 0), p(10, 0)]).expect("valid square");
        contour.normalize_winding(true);
        assert!(contour.is_ccw());
        assert_eq!(contour.points()[0], p(0, 0));
    }

    #[test]
    fn repeated_closing_vertex_is_removed() {
        let contour = Contour::new(vec![p(0, 0), p(100, 0), p(100, 100), p(0, 100), p(0, 0)])
            .expect("valid square");
        assert_eq!(contour.len(), 4);
    }
}
