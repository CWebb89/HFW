//! Wavefront OBJ import.
//!
//! Only the two records a mesh needs are read: `v` vertex positions and `f` faces.
//! Normal, texture and parameter records, groups, objects and material libraries are
//! skipped — the app derives normals from the geometry and paints per facet, and the
//! plate has its own object model, so none of that would survive the import.
//!
//! An OBJ has no units and no transforms of its own, so the mesh arrives in the
//! coordinates the file wrote.

use std::fs;
use std::path::Path;

use crate::{Mesh, MeshError};

/// Reads an OBJ file from disk.
pub fn load_obj<P: AsRef<Path>>(path: P) -> Result<Mesh, MeshError> {
    let text = fs::read_to_string(path).map_err(|error| MeshError::Io(error.to_string()))?;
    from_obj_text(&text)
}

/// Parses the text of an OBJ file.
///
/// Face indices are one-based; a negative index counts back from the most recent
/// vertex, as the format allows. A face with more than three corners is fanned from
/// its first vertex, which is exact for the convex polygons a mesh exporter writes
/// and the only triangulation the format itself implies.
pub fn from_obj_text(text: &str) -> Result<Mesh, MeshError> {
    let mut vertices = Vec::<[f64; 3]>::new();
    let mut soup = Vec::<[[f64; 3]; 3]>::new();

    for (line_number, line) in text.lines().enumerate() {
        let content = line.split('#').next().unwrap_or_default();
        let mut fields = content.split_whitespace();
        match fields.next() {
            Some("v") => {
                let mut point = [0.0_f64; 3];
                for coordinate in &mut point {
                    let text = fields
                        .next()
                        .ok_or_else(|| incomplete_vertex(line_number + 1))?;
                    *coordinate = text.parse::<f64>().map_err(|_| {
                        MeshError::InvalidObj(format!(
                            "line {}: {text} is not a coordinate",
                            line_number + 1
                        ))
                    })?;
                    if !coordinate.is_finite() {
                        return Err(MeshError::InvalidObj(format!(
                            "line {}: a coordinate is not finite",
                            line_number + 1
                        )));
                    }
                }
                vertices.push(point);
            }
            Some("f") => {
                let mut corners = Vec::<[f64; 3]>::new();
                for field in fields {
                    let index = vertex_index(field, vertices.len(), line_number + 1)?;
                    corners.push(vertices[index]);
                }
                if corners.len() < 3 {
                    return Err(MeshError::InvalidObj(format!(
                        "line {}: a face has {} corners",
                        line_number + 1,
                        corners.len()
                    )));
                }
                for corner in 1..corners.len() - 1 {
                    soup.push([corners[0], corners[corner], corners[corner + 1]]);
                }
            }
            _ => {}
        }
    }

    if vertices.is_empty() {
        return Err(MeshError::InvalidObj(
            "no vertex records were found".to_owned(),
        ));
    }
    if soup.is_empty() {
        return Err(MeshError::InvalidObj(
            "no face records were found".to_owned(),
        ));
    }
    Mesh::from_triangle_soup(&soup)
}

fn incomplete_vertex(line: usize) -> MeshError {
    MeshError::InvalidObj(format!(
        "line {line}: a vertex record needs three coordinates"
    ))
}

/// Resolves one face corner against the vertex list read so far.
fn vertex_index(field: &str, vertices: usize, line: usize) -> Result<usize, MeshError> {
    let corner = field.split('/').next().unwrap_or_default();
    let value = corner.parse::<i64>().map_err(|_| {
        MeshError::InvalidObj(format!("line {line}: {field} is not a vertex reference"))
    })?;
    let resolved = if value > 0 {
        value - 1
    } else if value < 0 {
        vertices as i64 + value
    } else {
        return Err(MeshError::InvalidObj(format!(
            "line {line}: vertex 0 does not exist"
        )));
    };
    if resolved < 0 || resolved as usize >= vertices {
        return Err(MeshError::InvalidObj(format!(
            "line {line}: {field} names vertex {value}, of {vertices}"
        )));
    }
    Ok(resolved as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: &str = "\
# a unit cube, one quad per face
v 0 0 0
v 10 0 0
v 10 10 0
v 0 10 0
v 0 0 10
v 10 0 10
v 10 10 10
v 0 10 10
f 1 4 3 2
f 5 6 7 8
f 1 2 6 5
f 2 3 7 6
f 3 4 8 7
f 4 1 5 8
";

    #[test]
    fn quads_are_fanned_into_triangles() {
        let mesh = from_obj_text(CUBE).expect("the cube parses");
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.vertex_count(), 8);
    }

    #[test]
    fn negative_indices_count_back_from_the_last_vertex() {
        let mesh = from_obj_text("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n").expect("it parses");
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.vertex_count(), 3);
    }

    #[test]
    fn a_face_slash_form_reference_is_read_for_its_position_only() {
        let mesh =
            from_obj_text("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1/1/1 2/2/2 3/3/3\n").expect("it parses");
        assert_eq!(mesh.triangle_count(), 1);
    }

    #[test]
    fn a_vertex_record_with_two_coordinates_is_refused() {
        let error = from_obj_text("v 0 0\nf 1 1 1\n").expect_err("it is incomplete");
        assert!(error.to_string().contains("three coordinates"), "{error}");
    }

    #[test]
    fn a_face_naming_a_vertex_past_the_end_is_refused() {
        let error =
            from_obj_text("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n").expect_err("vertex 4 is missing");
        assert!(error.to_string().contains("names vertex 4"), "{error}");
    }

    #[test]
    fn a_file_with_no_records_is_refused() {
        let error = from_obj_text("# nothing here\n").expect_err("nothing to build");
        assert!(error.to_string().contains("no vertex records"), "{error}");
    }

    #[test]
    fn a_file_with_vertices_and_no_faces_is_refused() {
        let error = from_obj_text("v 0 0 0\nv 1 0 0\nv 0 1 0\n").expect_err("nothing to build");
        assert!(error.to_string().contains("no face records"), "{error}");
    }
}
