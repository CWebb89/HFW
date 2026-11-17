//! 3MF import: a ZIP container with one or more XML model parts.
//!
//! 3MF is what OrcaSlicer, PrusaSlicer, Bambu Studio and next-layer all write,
//! so reading it is the difference between "import my project" and "re-export as
//! STL first". Only the parts that carry printable geometry are read: objects,
//! their meshes, components that reference other objects, and build items with
//! their transforms. Materials, colours, surface annotations and print settings
//! are ignored, not guessed at.
//!
//! Every build item is applied with its own 4x3 transform, so a plate of several
//! parts arrives at the positions it was arranged in, in millimetres, regardless
//! of the file's `unit` attribute.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::{Mesh, MeshError, Point3};

/// One parsed `<object>`, before its build transform is applied.
#[derive(Default)]
struct Object {
    name: String,
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    components: Vec<(u32, Affine)>,
}

/// One body read from a plate, with the name the file gave it.
#[derive(Clone, Debug)]
pub struct ModelObject {
    /// Object id from the model part, which is what per-object settings use.
    pub id: u32,
    /// Object name from the model part, or `object {id}` when it has none.
    pub name: String,
    /// Geometry in plate coordinates: the build item's transform is applied.
    pub mesh: Mesh,
}

/// A 3MF 4x3 affine transform, stored row-major as the file writes it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine {
    rows: [[f64; 3]; 3],
    translation: [f64; 3],
}

impl Default for Affine {
    fn default() -> Self {
        Affine {
            rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            translation: [0.0; 3],
        }
    }
}

impl Affine {
    /// Reads the twelve numbers of a `transform` attribute.
    ///
    /// The 3MF specification writes the matrix as four rows of three: the first
    /// three rows are the X, Y and Z basis vectors, the last is the translation.
    /// A point therefore maps as `p' = basis * p + translation`.
    fn parse(text: &str) -> Result<Affine, MeshError> {
        let values: Vec<f64> = text
            .split_whitespace()
            .map(|token| {
                token.parse::<f64>().map_err(|_| {
                    MeshError::InvalidThreeMf(format!("transform value {token:?} is not a number"))
                })
            })
            .collect::<Result<_, _>>()?;
        if values.len() != 12 {
            return Err(MeshError::InvalidThreeMf(format!(
                "a transform has 12 numbers, this one has {}",
                values.len()
            )));
        }
        Ok(Affine {
            rows: [
                [values[0], values[1], values[2]],
                [values[3], values[4], values[5]],
                [values[6], values[7], values[8]],
            ],
            translation: [values[9], values[10], values[11]],
        })
    }

    fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let mut out = self.translation;
        for (axis, row) in self.rows.iter().enumerate() {
            out[axis] += row[0] * p[0] + row[1] * p[1] + row[2] * p[2];
        }
        out
    }

    /// Composition `self * inner`: apply `inner` first, then `self`.
    fn then(&self, inner: &Affine) -> Affine {
        let mut rows = [[0.0f64; 3]; 3];
        for (row_index, row) in self.rows.iter().enumerate() {
            let mut composed = [0.0f64; 3];
            for (column, slot) in composed.iter_mut().enumerate() {
                *slot = (0..3).map(|k| row[k] * inner.rows[k][column]).sum();
            }
            rows[row_index] = composed;
        }
        Affine {
            rows,
            translation: self.apply(inner.translation),
        }
    }
}

/// Millimetres per model unit, as the `unit` attribute declares them.
fn unit_scale(unit: &str) -> Result<f64, MeshError> {
    match unit {
        "micron" => Ok(0.001),
        "millimeter" | "" => Ok(1.0),
        "centimeter" => Ok(10.0),
        "inch" => Ok(25.4),
        "foot" => Ok(304.8),
        "meter" => Ok(1000.0),
        other => Err(MeshError::InvalidThreeMf(format!(
            "unknown model unit {other:?}"
        ))),
    }
}

/// An attribute value from a start tag, or `None` when the tag carries none.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(at) = rest.find(name) {
        let after = &rest[at + name.len()..];
        let before = &rest[..at];
        let boundary = before
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || c == '<');
        if boundary {
            let trimmed = after.trim_start();
            if let Some(value) = trimmed.strip_prefix('=') {
                let value = value.trim_start();
                if let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'') {
                    if let Some(end) = value[1..].find(quote) {
                        return Some(&value[1..1 + end]);
                    }
                }
            }
        }
        rest = after;
    }
    None
}

fn number(tag: &str, name: &str) -> Result<f64, MeshError> {
    let raw = attribute(tag, name)
        .ok_or_else(|| MeshError::InvalidThreeMf(format!("a tag is missing its {name}")))?;
    raw.trim()
        .parse::<f64>()
        .map_err(|_| MeshError::InvalidThreeMf(format!("{name}={raw:?} is not a number")))
}

fn index(tag: &str, name: &str) -> Result<u32, MeshError> {
    let raw = attribute(tag, name)
        .ok_or_else(|| MeshError::InvalidThreeMf(format!("a tag is missing its {name}")))?;
    raw.trim()
        .parse::<u32>()
        .map_err(|_| MeshError::InvalidThreeMf(format!("{name}={raw:?} is not an index")))
}

/// One build item: an object plus the transform that places it on the plate.
struct Item {
    object: u32,
    transform: Affine,
}

/// Parsed model part: objects by id, and the build items in file order.
struct Model {
    objects: HashMap<u32, Object>,
    items: Vec<Item>,
    unit: f64,
}

/// Splits the model XML into start tags and the text between them.
///
/// Only what a 3MF model part needs is handled: declarations and comments are
/// skipped, and every start tag is handed to the caller with its raw attribute
/// text. Element content is never interpreted, so this cannot be fooled by a
/// value that looks like markup.
fn tags(xml: &str) -> impl Iterator<Item = &str> {
    let mut rest = xml;
    std::iter::from_fn(move || loop {
        let start = rest.find('<')?;
        let after = &rest[start..];
        if let Some(skip) = after.strip_prefix("<!--") {
            let end = skip.find("-->")?;
            rest = &skip[end + 3..];
            continue;
        }
        if after.starts_with("<?") || after.starts_with("<!") {
            let end = after.find('>')?;
            rest = &after[end + 1..];
            continue;
        }
        let end = after.find('>')?;
        let tag = &after[1..end];
        rest = &after[end + 1..];
        return Some(tag);
    })
}

fn parse_model(xml: &str) -> Result<Model, MeshError> {
    let mut objects: HashMap<u32, Object> = HashMap::new();
    let mut items = Vec::new();
    let mut unit = 1.0;
    let mut current: Option<(u32, Object)> = None;
    let mut in_vertices = false;
    let mut in_triangles = false;

    for tag in tags(xml) {
        let name = tag
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        match name {
            "model" => {
                if let Some(declared) = attribute(tag, "unit") {
                    unit = unit_scale(declared.trim())?;
                }
            }
            "object" => {
                let id = index(tag, "id")?;
                let named = Object {
                    name: attribute(tag, "name").unwrap_or("").trim().to_owned(),
                    ..Object::default()
                };
                current = Some((id, named));
            }
            "/object" => {
                if let Some((id, object)) = current.take() {
                    objects.insert(id, object);
                }
            }
            "vertices" => in_vertices = true,
            "/vertices" => in_vertices = false,
            "triangles" => in_triangles = true,
            "/triangles" => in_triangles = false,
            "vertex" if in_vertices => {
                let point = [number(tag, "x")?, number(tag, "y")?, number(tag, "z")?];
                if let Some((_, object)) = current.as_mut() {
                    object.vertices.push(point);
                }
            }
            "triangle" if in_triangles => {
                let triangle = [index(tag, "v1")?, index(tag, "v2")?, index(tag, "v3")?];
                if let Some((_, object)) = current.as_mut() {
                    object.triangles.push(triangle);
                }
            }
            "component" => {
                let id = index(tag, "objectid")?;
                let transform = match attribute(tag, "transform") {
                    Some(text) => Affine::parse(text)?,
                    None => Affine::default(),
                };
                if let Some((_, object)) = current.as_mut() {
                    object.components.push((id, transform));
                }
            }
            "item" => {
                let object = index(tag, "objectid")?;
                let transform = match attribute(tag, "transform") {
                    Some(text) => Affine::parse(text)?,
                    None => Affine::default(),
                };
                items.push(Item { object, transform });
            }
            _ => {}
        }
    }
    if objects.is_empty() {
        return Err(MeshError::InvalidThreeMf(
            "the model part contains no objects".to_owned(),
        ));
    }
    if items.is_empty() {
        let mut ids: Vec<u32> = objects.keys().copied().collect();
        ids.sort_unstable();
        items = ids
            .into_iter()
            .map(|object| Item {
                object,
                transform: Affine::default(),
            })
            .collect();
    }
    Ok(Model {
        objects,
        items,
        unit,
    })
}

fn collect(
    model: &Model,
    object: u32,
    transform: Affine,
    depth: usize,
    vertices: &mut Vec<Point3>,
    triangles: &mut Vec<[u32; 3]>,
) -> Result<(), MeshError> {
    if depth > 32 {
        return Err(MeshError::InvalidThreeMf(
            "components nest more than 32 deep".to_owned(),
        ));
    }
    let source = model.objects.get(&object).ok_or_else(|| {
        MeshError::InvalidThreeMf(format!("a build item references missing object {object}"))
    })?;
    let base = u32::try_from(vertices.len()).map_err(|_| MeshError::TooManyVertices)?;
    for point in &source.vertices {
        let placed = transform.apply(*point);
        let scaled = [
            placed[0] * model.unit,
            placed[1] * model.unit,
            placed[2] * model.unit,
        ];
        vertices.push(Point3::from_mm(scaled[0], scaled[1], scaled[2])?);
    }
    for triangle in &source.triangles {
        triangles.push([triangle[0] + base, triangle[1] + base, triangle[2] + base]);
    }
    for (child, child_transform) in &source.components {
        let combined = transform.then(child_transform);
        collect(model, *child, combined, depth + 1, vertices, triangles)?;
    }
    Ok(())
}

/// Reads a 3MF file into one mesh, with every build item placed by its
/// transform.
///
/// Several build items become one mesh: the slicer sees one object made of
/// several bodies, which is what a plate of parts is. Positions are kept, so the
/// result slices where the file arranged it.
pub fn load_3mf<P: AsRef<Path>>(path: P) -> Result<Mesh, MeshError> {
    let xml = model_text(path.as_ref())?;
    from_3mf_text(&xml)
}

/// Reads a 3MF into its build items, one [`ModelObject`] each.
///
/// [`load_3mf`] merges the plate into one mesh, which is what a slicer needs to
/// print several bodies at once but destroys the fact that they are separate
/// objects. A caller that wants to order, count or report per object uses this
/// instead: component sub-objects stay inside the item that instantiates them,
/// and every item's transform is already applied.
pub fn load_3mf_objects<P: AsRef<Path>>(path: P) -> Result<Vec<ModelObject>, MeshError> {
    let xml = model_text(path.as_ref())?;
    objects_from_3mf_text(&xml)
}

/// Parses a model part in memory into one object per build item.
pub fn objects_from_3mf_text(xml: &str) -> Result<Vec<ModelObject>, MeshError> {
    let model = parse_model(xml)?;
    let mut out = Vec::with_capacity(model.items.len());
    for item in &model.items {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        collect(
            &model,
            item.object,
            item.transform,
            0,
            &mut vertices,
            &mut triangles,
        )?;
        if triangles.is_empty() {
            continue;
        }
        let name = model
            .objects
            .get(&item.object)
            .map(|object| object.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("object {}", item.object));
        let mesh = Mesh::new(
            vertices,
            triangles.into_iter().map(crate::Triangle).collect(),
        )?;
        out.push(ModelObject {
            id: item.object,
            name,
            mesh,
        });
    }
    if out.is_empty() {
        return Err(MeshError::InvalidThreeMf(
            "the model part contains no printable build items".to_owned(),
        ));
    }
    Ok(out)
}

fn model_text(path: &Path) -> Result<String, MeshError> {
    let file = std::fs::File::open(path)
        .map_err(|error| MeshError::Io(format!("{}: {error}", path.display())))?;
    model_text_from_reader(file)
}

fn model_text_from_reader<R: Read + std::io::Seek>(reader: R) -> Result<String, MeshError> {
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| MeshError::InvalidThreeMf(format!("not a ZIP container: {error}")))?;
    let mut name = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| MeshError::InvalidThreeMf(format!("unreadable archive: {error}")))?;
        let entry_name = entry.name().to_owned();
        if entry_name == "3D/3dmodel.model" {
            name = Some(entry_name);
            break;
        }
        if name.is_none() && entry_name.ends_with(".model") {
            name = Some(entry_name);
        }
    }
    let name = name.ok_or_else(|| {
        MeshError::InvalidThreeMf("no .model part: this is a ZIP but not a 3MF".to_owned())
    })?;
    let mut entry = archive
        .by_name(&name)
        .map_err(|error| MeshError::InvalidThreeMf(format!("cannot open {name}: {error}")))?;
    let mut xml = String::new();
    entry
        .read_to_string(&mut xml)
        .map_err(|error| MeshError::InvalidThreeMf(format!("cannot read {name}: {error}")))?;
    Ok(xml)
}

/// Reads a 3MF from any reader, for callers holding bytes already.
pub fn from_3mf_reader<R: Read + std::io::Seek>(reader: R) -> Result<Mesh, MeshError> {
    from_3mf_text(&model_text_from_reader(reader)?)
}

/// Parses a model part that is already in memory.
pub fn from_3mf_text(xml: &str) -> Result<Mesh, MeshError> {
    let model = parse_model(xml)?;
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for item in &model.items {
        collect(
            &model,
            item.object,
            item.transform,
            0,
            &mut vertices,
            &mut triangles,
        )?;
    }
    if triangles.is_empty() {
        return Err(MeshError::InvalidThreeMf(
            "the model part contains no triangles".to_owned(),
        ));
    }
    Mesh::new(
        vertices,
        triangles.into_iter().map(crate::Triangle).collect(),
    )
}

/// Reads the metadata parts a 3MF project carries alongside its geometry.
///
/// A 3MF written by a slicer is a *project*: the geometry lives in a `.model`
/// part and the print settings live in a sibling part under `Metadata/`.
/// PrusaSlicer and OrcaSlicer write `Slic3r_PE.config` (a `key = value` preset)
/// and `Slic3r_PE_model.config` (per-object overrides); Bambu Studio and newer
/// Orca write `project_settings.config` (a flat JSON object). Every part under
/// `Metadata/` is returned in archive order with its raw bytes, so the caller
/// decides what to understand — geometry loading ignores them entirely.
pub fn three_mf_metadata<P: AsRef<Path>>(path: P) -> Result<Vec<(String, Vec<u8>)>, MeshError> {
    let file = std::fs::File::open(path.as_ref())
        .map_err(|error| MeshError::Io(format!("{}: {error}", path.as_ref().display())))?;
    metadata_from_reader(file)
}

/// Reads the `Metadata/` parts from any seekable reader.
///
/// A reader that is not a ZIP archive yields an empty list rather than an error:
/// asking an STL for its project settings should say "none", not "corrupt".
pub fn metadata_from_reader<R: Read + std::io::Seek>(
    reader: R,
) -> Result<Vec<(String, Vec<u8>)>, MeshError> {
    let Ok(mut archive) = zip::ZipArchive::new(reader) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| MeshError::InvalidThreeMf(format!("unreadable archive: {error}")))?;
        let name = entry.name().to_owned();
        if !name.starts_with("Metadata/") || name.ends_with('/') {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| MeshError::InvalidThreeMf(format!("cannot read {name}: {error}")))?;
        found.push((name, bytes));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TETRA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <object id="2" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="10" y="0" z="0"/>
          <vertex x="0" y="10" z="0"/>
          <vertex x="0" y="0" z="10"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="2" v3="1"/>
          <triangle v1="0" v2="1" v3="3"/>
          <triangle v1="1" v2="2" v3="3"/>
          <triangle v1="2" v2="0" v3="3"/>
        </triangles>
      </mesh>
    </object>
  </resources>
  <build>
    <item objectid="2"/>
  </build>
</model>
"#;

    #[test]
    fn a_plate_keeps_its_build_items_as_separate_objects() {
        let plate = TETRA.replace(
            "    <item objectid=\"2\"/>",
            "    <item objectid=\"2\"/>\n    <item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 40 0 0\"/>",
        );
        assert!(plate.contains("40 0 0"), "the second item was inserted");
        let objects = objects_from_3mf_text(&plate).expect("parses");
        assert_eq!(objects.len(), 2, "one object per build item");
        let (low, _, high, _) = objects[1].mesh.xy_bounds_mm().expect("second object");
        assert!((low - 40.0).abs() < 1e-6, "the item transform is applied");
        assert!((high - 50.0).abs() < 1e-6);
        let merged = from_3mf_text(&plate).expect("the merged view still works");
        assert_eq!(merged.vertex_count(), 8);
    }

    #[test]
    fn reads_vertices_and_triangles() {
        let mesh = from_3mf_text(TETRA).expect("parses");
        assert_eq!(mesh.vertex_count(), 4);
        assert_eq!(mesh.triangle_count(), 4);
        let corners = mesh
            .vertices()
            .iter()
            .map(|p| p.to_mm())
            .fold([f64::NEG_INFINITY; 3], |acc, p| {
                [acc[0].max(p[0]), acc[1].max(p[1]), acc[2].max(p[2])]
            });
        assert!((corners[0] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn applies_the_build_transform() {
        let placed = TETRA.replace(
            "<item objectid=\"2\"/>",
            "<item objectid=\"2\" transform=\"2 0 0 0 2 0 0 0 2 5 6 7\"/>",
        );
        let mesh = from_3mf_text(&placed).expect("parses");
        let lowest = mesh
            .vertices()
            .iter()
            .map(|p| p.to_mm())
            .fold([f64::INFINITY; 3], |acc, p| {
                [acc[0].min(p[0]), acc[1].min(p[1]), acc[2].min(p[2])]
            });
        assert!((lowest[0] - 5.0).abs() < 1e-9);
        assert!((lowest[1] - 6.0).abs() < 1e-9);
        assert!((lowest[2] - 7.0).abs() < 1e-9);
        let highest = mesh
            .vertices()
            .iter()
            .map(|p| p.to_mm())
            .fold([f64::NEG_INFINITY; 3], |acc, p| {
                [acc[0].max(p[0]), acc[1].max(p[1]), acc[2].max(p[2])]
            });
        assert!((highest[0] - 25.0).abs() < 1e-9, "scale is not applied");
    }

    #[test]
    fn converts_the_declared_unit() {
        let inches = TETRA.replace("unit=\"millimeter\"", "unit=\"inch\"");
        let mesh = from_3mf_text(&inches).expect("parses");
        let highest = mesh
            .vertices()
            .iter()
            .map(|p| p.to_mm()[0])
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (highest - 254.0).abs() < 1e-6,
            "10 inch is 254 mm, got {highest}"
        );
    }

    #[test]
    fn resolves_components_with_their_own_transforms() {
        let with_component = TETRA
            .replace(
                "<object id=\"2\" type=\"model\">",
                "<object id=\"7\" type=\"model\"><components><component objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 100 0 0\"/></components></object>\n    <object id=\"2\" type=\"model\">",
            )
            .replace("<item objectid=\"2\"/>", "<item objectid=\"7\"/>");
        let mesh = from_3mf_text(&with_component).expect("parses");
        assert_eq!(mesh.triangle_count(), 4);
        let lowest = mesh
            .vertices()
            .iter()
            .map(|p| p.to_mm()[0])
            .fold(f64::INFINITY, f64::min);
        assert!((lowest - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_plate_of_two_parts_stays_apart() {
        let plate = TETRA.replace(
            "<item objectid=\"2\"/>",
            "<item objectid=\"2\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 50 0 0\"/>",
        );
        let mesh = from_3mf_text(&plate).expect("parses");
        assert_eq!(mesh.triangle_count(), 8);
        let xs: Vec<f64> = mesh.vertices().iter().map(|p| p.to_mm()[0]).collect();
        assert!(xs.iter().any(|x| (*x - 50.0).abs() < 1e-9));
        assert!(xs.iter().any(|x| x.abs() < 1e-9));
    }

    #[test]
    fn a_file_without_geometry_is_refused() {
        let empty = "<model><resources></resources><build/></model>";
        let error = from_3mf_text(empty).unwrap_err().to_string();
        assert!(error.contains("no objects"), "{error}");
    }
}
