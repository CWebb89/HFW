//! Text as a mesh the slicer already understands.
//!
//! A glyph arriving from [`hfw_font`] is a set of closed contours in millimetres: an
//! outer boundary and, for a letter like `o` or `B`, one or more holes. Extruding that
//! into a solid means three things — cap the ring at the bottom, cap it at the top, and
//! wall every contour between the two — and the first of those is the only hard part,
//! because a polygon with holes is not a polygon until the holes are bridged into the
//! boundary.
//!
//! This module does it without a triangle-level boolean: a letter is placed as a prism
//! and the slice unions or subtracts it per layer, where the engine already works with
//! exact predicates. Embossing therefore needs no CSG kernel — a raised letter is a part
//! volume that overlaps the object, and an engraved one is a negative volume.

#![forbid(unsafe_code)]

use hfw_font::{Contour, Face, FontError, Outline};
use hfw_geometry::Coord;
use hfw_kernel as kernel;
use hfw_kernel::V2;
use hfw_mesh::{Mesh, MeshError, Point3, Triangle};

/// A prism could not be built from the contours it was given.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextError {
    /// The font could not be read or had no outline for a character.
    Font(FontError),
    /// The mesh could not be constructed.
    Mesh(MeshError),
    /// The height was zero or negative, so the prism would have no volume.
    NoHeight,
    /// The size was zero or negative.
    NoSize,
    /// The contours of a glyph could not be triangulated.
    Untriangulatable {
        /// How many contours were being capped.
        contours: usize,
    },
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Font(error) => write!(f, "{error}"),
            Self::Mesh(error) => write!(f, "{error}"),
            Self::NoHeight => write!(f, "text needs a positive height"),
            Self::NoSize => write!(f, "text needs a positive size"),
            Self::Untriangulatable { contours } => write!(
                f,
                "the {contours} contours of this text could not be triangulated"
            ),
        }
    }
}

impl std::error::Error for TextError {}

impl From<FontError> for TextError {
    fn from(error: FontError) -> Self {
        Self::Font(error)
    }
}

impl From<MeshError> for TextError {
    fn from(error: MeshError) -> Self {
        Self::Mesh(error)
    }
}

/// A ring of the extruded solid: a boundary or a hole, in the order the font stores it.
type Ring = Vec<[f64; 2]>;

/// One boundary and the holes inside it.
#[derive(Clone, Debug)]
struct Shell {
    outer: Ring,
    holes: Vec<Ring>,
}

/// A solid prism: its caps and its walls, in millimetres.
#[derive(Clone, Debug, Default)]
pub struct Prism {
    /// The bottom cap's triangles, each three positions in millimetres.
    pub triangles: Vec<[[f64; 3]; 3]>,
}

impl Prism {
    /// Extrudes an outline between `z` and `z + height`.
    pub fn from_outline(outline: &Outline, height: f64, z: f64) -> Result<Self, TextError> {
        if !height.is_finite() || height <= 0.0 {
            return Err(TextError::NoHeight);
        }
        let shells = shells(outline);
        if shells.is_empty() {
            return Ok(Self::default());
        }
        let mut triangles = Vec::new();
        let top = z + height;
        for shell in &shells {
            for cap in triangulate_shell(shell)? {
                let mut points = cap;
                if !ccw(&points) {
                    points.swap(1, 2);
                }
                let flat = |point: [f64; 2], z: f64| [point[0], point[1], z];
                triangles.push([
                    flat(points[0], top),
                    flat(points[1], top),
                    flat(points[2], top),
                ]);
                triangles.push([flat(points[0], z), flat(points[2], z), flat(points[1], z)]);
            }
            for ring in std::iter::once(&shell.outer).chain(shell.holes.iter()) {
                for index in 0..ring.len() {
                    let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
                    let (c, d) = ([a[0], a[1], top], [b[0], b[1], top]);
                    let (e, f) = ([b[0], b[1], z], [a[0], a[1], z]);
                    triangles.push([c, e, d]);
                    triangles.push([c, f, e]);
                }
            }
        }
        Ok(Self { triangles })
    }

    /// The prism as a mesh, with coordinates rounded to the engine's nanometres.
    pub fn mesh(&self) -> Result<Mesh, TextError> {
        let mut vertices: Vec<Point3> = Vec::new();
        let mut triangles = Vec::with_capacity(self.triangles.len());
        for triangle in &self.triangles {
            let mut index = [0_u32; 3];
            for (slot, position) in triangle.iter().enumerate() {
                let point = Point3::from_mm(position[0], position[1], position[2])?;
                let found = vertices.iter().position(|existing| *existing == point);
                index[slot] = match found {
                    Some(existing) => existing as u32,
                    None => {
                        vertices.push(point);
                        (vertices.len() - 1) as u32
                    }
                };
            }
            if index[0] != index[1] && index[1] != index[2] && index[0] != index[2] {
                triangles.push(Triangle([index[0], index[1], index[2]]));
            }
        }
        Ok(Mesh::new(vertices, triangles)?)
    }

    /// True when the prism encloses no triangles at all.
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }
}

/// Extrudes a string at a size, with its baseline at `z` and its origin at `(x, y)`.
pub fn text_prism(
    face: &Face,
    text: &str,
    size_mm: f64,
    height_mm: f64,
    x: f64,
    y: f64,
    z: f64,
) -> Result<(Prism, f64), TextError> {
    if !size_mm.is_finite() || size_mm <= 0.0 {
        return Err(TextError::NoSize);
    }
    if !height_mm.is_finite() || height_mm <= 0.0 {
        return Err(TextError::NoHeight);
    }
    let (mut outline, width) = face.layout_mm(text, size_mm, size_mm / 2000.0)?;
    outline.shift(x, y);
    let prism = Prism::from_outline(&outline, height_mm, z)?;
    Ok((prism, width))
}

/// The plaintext extent of a string, in millimetres, without building geometry.
pub fn text_width_mm(face: &Face, text: &str, size_mm: f64) -> f64 {
    text.chars()
        .filter_map(|character| face.glyph_index(character))
        .filter_map(|glyph| face.advance_mm(glyph, size_mm))
        .sum()
}

/// Groups contours into boundaries and the holes nested inside them.
fn shells(outline: &Outline) -> Vec<Shell> {
    let rings: Vec<Ring> = outline
        .contours
        .iter()
        .map(|contour| contour.points.clone())
        .filter(|ring| ring.len() >= 3)
        .collect();
    let depths: Vec<usize> = rings
        .iter()
        .enumerate()
        .map(|(index, ring)| {
            rings
                .iter()
                .enumerate()
                .filter(|(other, candidate)| *other != index && contains(candidate, &ring[0]))
                .count()
        })
        .collect();
    let mut shells: Vec<Shell> = Vec::new();
    let mut owners: Vec<Option<usize>> = vec![None; rings.len()];
    for (index, ring) in rings.iter().enumerate() {
        if depths[index] % 2 == 0 {
            owners[index] = Some(shells.len());
            let mut outer = ring.clone();
            if !counterclockwise_ring(&outer) {
                outer.reverse();
            }
            shells.push(Shell {
                outer,
                holes: Vec::new(),
            });
        }
    }
    for (index, ring) in rings.iter().enumerate() {
        if depths[index] % 2 == 1 {
            let mut best: Option<(usize, usize)> = None;
            for (other, candidate) in rings.iter().enumerate() {
                if other == index || !contains(candidate, &ring[0]) {
                    continue;
                }
                let depth = depths[other];
                if depths[other] % 2 == 0 && best.map(|(_, d)| depth > d).unwrap_or(true) {
                    best = match owners[other] {
                        Some(owner) => Some((owner, depth)),
                        None => best,
                    };
                }
            }
            if let Some((owner, _)) = best {
                let mut hole = ring.clone();
                if counterclockwise_ring(&hole) {
                    hole.reverse();
                }
                shells[owner].holes.push(hole);
            }
        }
    }
    shells
}

/// True when `outer` contains `point`, by the even-odd rule along +X.
fn contains(outer: &[[f64; 2]], point: &[f64; 2]) -> bool {
    let mut inside = false;
    let mut j = outer.len() - 1;
    for i in 0..outer.len() {
        let (a, b) = (outer[i], outer[j]);
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let t = (point[1] - a[1]) / (b[1] - a[1]);
            if point[0] < a[0] + t * (b[0] - a[0]) {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// True when a ring runs anticlockwise in the font's own coordinates.
fn counterclockwise_ring(ring: &[[f64; 2]]) -> bool {
    let mut sum = 0.0;
    for (index, point) in ring.iter().enumerate() {
        let next = ring[(index + 1) % ring.len()];
        sum += point[0] * next[1] - next[0] * point[1];
    }
    sum > 0.0
}

fn ccw(triangle: &[[f64; 2]; 3]) -> bool {
    let (a, b, c) = (triangle[0], triangle[1], triangle[2]);
    (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]) > 0.0
}

/// Triangles for a shell's cap, from the shared ear-clipping capper in `hfw_kernel`.
fn triangulate_shell(shell: &Shell) -> Result<Vec<[[f64; 2]; 3]>, TextError> {
    let outer = to_v2(&shell.outer);
    let holes: Vec<Vec<V2>> = shell.holes.iter().map(|hole| to_v2(hole)).collect();
    kernel::cap::triangulate(&outer, &holes)
        .map(|triangles| {
            triangles
                .iter()
                .map(|triangle| {
                    [
                        [triangle[0].x, triangle[0].y],
                        [triangle[1].x, triangle[1].y],
                        [triangle[2].x, triangle[2].y],
                    ]
                })
                .collect()
        })
        .ok_or(TextError::Untriangulatable {
            contours: shell.holes.len() + 1,
        })
}

fn to_v2(ring: &[[f64; 2]]) -> Vec<V2> {
    ring.iter()
        .map(|point| V2::new(point[0], point[1]))
        .collect()
}

impl Prism {
    /// Total signed volume of the prism in cubic millimetres, for a test to compare
    /// against a polygon's area times its height.
    pub fn volume_mm3(&self) -> f64 {
        let mut total = 0.0;
        for triangle in &self.triangles {
            let [a, b, c] = *triangle;
            total += a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]);
        }
        total / 6.0
    }
}

/// A contour's area in square millimetres, for a test to check a cap against.
pub fn contour_area_mm2(contour: &Contour) -> f64 {
    Outline::area(contour)
}

/// The z a prism starts at, in the engine's coordinate domain.
pub fn z_coord_mm(z: f64) -> Result<Coord, TextError> {
    hfw_geometry::mm_to_coord(z).map_err(|error| TextError::Mesh(MeshError::Geometry(error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(points: &[(f64, f64)]) -> Ring {
        points.iter().map(|(x, y)| [*x, *y]).collect()
    }

    fn text_outline() -> Outline {
        Outline {
            contours: vec![Contour {
                points: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            }],
        }
    }

    #[test]
    fn a_square_extrudes_to_a_closed_box_with_the_right_volume() {
        let prism = Prism::from_outline(&text_outline(), 2.0, 0.0).unwrap();
        assert_eq!(prism.triangles.len(), 4 + 8);
        let mesh = prism.mesh().unwrap();
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.vertex_count(), 8);
        let volume = mesh.volume_mm3();
        assert!((volume - 200.0).abs() < 1e-6, "{volume}");
    }

    #[test]
    fn a_hole_is_a_ring_inside_the_boundary_and_the_volume_shows_it() {
        let outline = Outline {
            contours: vec![
                Contour {
                    points: ring(&[(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)]),
                },
                Contour {
                    points: ring(&[(5.0, 5.0), (5.0, 15.0), (15.0, 15.0), (15.0, 5.0)]),
                },
            ],
        };
        let prism = Prism::from_outline(&outline, 1.0, 0.0).unwrap();
        let volume = prism.volume_mm3();
        assert!((volume - (400.0 - 100.0)).abs() < 1e-6, "{volume}");
        let mesh = prism.mesh().unwrap();
        assert!(mesh.volume_mm3() > 0.0, "the mesh's own volume is positive");
    }

    #[test]
    fn a_negative_or_zero_height_is_refused() {
        assert_eq!(
            Prism::from_outline(&text_outline(), 0.0, 0.0).unwrap_err(),
            TextError::NoHeight
        );
        assert_eq!(
            Prism::from_outline(&text_outline(), -1.0, 0.0).unwrap_err(),
            TextError::NoHeight
        );
    }

    #[test]
    fn an_empty_outline_produces_nothing_rather_than_an_error() {
        let prism = Prism::from_outline(&Outline::default(), 1.0, 0.0).unwrap();
        assert!(prism.is_empty());
        assert_eq!(prism.mesh().unwrap().triangle_count(), 0);
    }

    #[test]
    fn nesting_is_decided_by_containment_not_by_stored_direction() {
        let outline = Outline {
            contours: vec![
                Contour {
                    points: ring(&[(0.0, 0.0), (0.0, 20.0), (20.0, 20.0), (20.0, 0.0)]),
                },
                Contour {
                    points: ring(&[(5.0, 5.0), (15.0, 5.0), (15.0, 15.0), (5.0, 15.0)]),
                },
            ],
        };
        let shells = shells(&outline);
        assert_eq!(shells.len(), 1);
        assert_eq!(shells[0].holes.len(), 1);
        let volume = Prism::from_outline(&outline, 1.0, 0.0)
            .unwrap()
            .volume_mm3();
        assert!((volume - 300.0).abs() < 1e-6, "{volume}");
    }

    #[test]
    fn a_system_font_letter_slices_into_a_positive_volume() {
        let Some(face) = system_face() else {
            return;
        };
        let (prism, width) = text_prism(&face, "HFW", 10.0, 1.2, 0.0, 0.0, 0.0).unwrap();
        assert!(width > 15.0, "{width}");
        let mesh = prism.mesh().unwrap();
        assert!(mesh.triangle_count() > 100, "{}", mesh.triangle_count());
        let volume = mesh.volume_mm3();
        assert!(volume > 20.0 && volume < 400.0, "{volume}");
        let (low, high) = mesh.z_bounds().unwrap();
        assert_eq!(low, hfw_geometry::mm_to_coord(0.0).unwrap());
        assert_eq!(high, hfw_geometry::mm_to_coord(1.2).unwrap());
    }

    #[test]
    fn a_letter_with_a_counter_keeps_its_hole_and_closes_the_surface() {
        let Some(face) = system_face() else {
            return;
        };
        let (prism, _) = text_prism(&face, "o", 20.0, 1.0, 0.0, 0.0, 0.0).unwrap();
        let mesh = prism.mesh().unwrap();
        let diagnostics = mesh.diagnostics();
        assert_eq!(
            diagnostics.inconsistently_oriented_edges, 0,
            "{diagnostics:?}"
        );
        assert_eq!(diagnostics.boundary_edges, 0, "the prism is not closed");
        let glyph = face
            .outline_mm(face.glyph_index('o').unwrap(), 20.0, 0.01)
            .unwrap();
        let bounds = glyph.bounds();
        let boxed = (bounds.2 - bounds.0) * (bounds.3 - bounds.1);
        let volume = prism.volume_mm3();
        assert!(
            volume < boxed * 0.6,
            "{volume} against a bounding prism of {boxed}: the counter was filled in"
        );
        assert!(volume > boxed * 0.1, "{volume} is too thin to be a letter");
    }

    fn system_face() -> Option<Face> {
        let candidates = [
            "/usr/share/fonts/TTF/FiraCode-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ];
        candidates
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .map(|bytes| Face::parse(&bytes, 0).expect("the system font parses"))
    }
}
