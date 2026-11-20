//! A project file: the plate, and what each part prints like, in one `.3mf`.
//!
//! The geometry goes in the standard model part, so any slicer can open the
//! layouts this engine writes and any project the application already has opens
//! here. What a 3MF has no place for — which objects are switched off, and the
//! per-object settings a panel set — goes in `Metadata/hfw_project.xml`, beside
//! the model rather than inside it, so the geometry stays a valid 3MF even if the
//! reader is not ours.
//!
//! Settings are written as the name and value an operator would type, using
//! [`Settings::key_value`] — the inverse of the parser that accepted them — so a
//! value this file carries always parses back to the settings it came from, and a
//! key this engine does not publish is refused on the way out instead of being
//! saved as text nothing can read.
//!
//! Placement is written into the coordinates: an object is stored where it sits,
//! as `Placed` says. A project is a layout, not a set of editable bodies with
//! poses, and a reader that expected a pose would need to know our transform
//! convention to place a part.

use hfw_engine::{Mesh, Paint, PlateOrder, Settings};
use hfw_mesh::{archive_parts, model_part_xml, objects_from_3mf_text};

use crate::{ObjectId, Plate, PlateError};

/// The part that carries what the 3MF format has no field for.
pub const PROJECT_PART: &str = "Metadata/hfw_project.xml";

/// Version of the project part's own format.
///
/// Version 2 added per-object paint. Version 1 files still open: they simply
/// carry no paint.
pub const PROJECT_VERSION: u32 = 2;

/// Oldest project format this reader accepts.
const OLDEST_PROJECT_VERSION: u32 = 1;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
  <Override PartName="/Metadata/hfw_project.xml" ContentType="application/xml"/>
</Types>
"#;

const RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"#;

impl Plate {
    /// The plate as a project file, ready to write.
    ///
    /// Every enabled and disabled object is kept; a disabled one is written with
    /// its geometry because switching it back on in the panel is the same job as
    /// switching it off.
    pub fn to_project(&self) -> Result<Vec<u8>, PlateError> {
        let placed: Vec<(String, Mesh)> = self
            .objects
            .iter()
            .map(|object| Ok((object.name.clone(), object.placed_mesh()?)))
            .collect::<Result<_, PlateError>>()?;
        let borrowed: Vec<(String, &Mesh)> = placed
            .iter()
            .map(|(name, mesh)| (name.clone(), mesh))
            .collect();
        let parts: Vec<(&str, Vec<u8>)> = vec![
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
            ("_rels/.rels", RELATIONSHIPS.as_bytes().to_vec()),
            ("3D/3dmodel.model", model_part_xml(&borrowed).into_bytes()),
            (PROJECT_PART, project_part(self)?.into_bytes()),
        ];
        hfw_mesh::write_archive(&parts).map_err(|error| PlateError::Slice(error.to_string()))
    }

    /// Reads a project file back into a plate.
    ///
    /// Geometry comes from the model part; the settings and the enabled flags come
    /// from the project part when it is there. A file written by another slicer
    /// has no project part, and opens as geometry with the plate's own settings —
    /// which is what the command line has always done with a 3MF.
    ///
    /// Every object is marked positioned: its coordinates came from the file, so
    /// arranging must not move it.
    pub fn from_project(bytes: &[u8], settings: Settings) -> Result<Self, PlateError> {
        let parts = archive_parts(std::io::Cursor::new(bytes))
            .map_err(|error| PlateError::Slice(error.to_string()))?;
        let model = parts
            .iter()
            .find(|(name, _)| name == "3D/3dmodel.model")
            .or_else(|| parts.iter().find(|(name, _)| name.ends_with(".model")))
            .ok_or_else(|| {
                PlateError::Slice(
                    "the file has no model part: this is a ZIP but not a 3MF".to_owned(),
                )
            })?;
        let xml = std::str::from_utf8(&model.1)
            .map_err(|error| PlateError::Slice(format!("the model part is not text: {error}")))?;
        let objects =
            objects_from_3mf_text(xml).map_err(|error| PlateError::Slice(error.to_string()))?;
        if objects.is_empty() {
            return Err(PlateError::NothingEnabled);
        }
        let extras = parts
            .iter()
            .find(|(name, _)| name == PROJECT_PART)
            .map(|(_, bytes)| String::from_utf8_lossy(bytes).to_string())
            .map(|xml| parse_project(&xml))
            .transpose()?
            .unwrap_or_default();

        let mut plate = Plate::new(settings);
        for (index, object) in objects.into_iter().enumerate() {
            let id = plate.add_positioned(object.name.clone(), object.mesh)?;
            let saved = extras.iter().find(|saved| saved.index == index);
            if let Some(saved) = saved {
                plate.set_object_enabled(id, saved.enabled)?;
                for (key, value) in &saved.settings {
                    plate.set_object_setting(id, key, value)?;
                }
                let object = plate.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
                object.set_paint(Paint {
                    support: saved.support.clone(),
                    seam: saved.seam.clone(),
                })?;
            }
        }
        Ok(plate)
    }

    /// Writes the plate to `path` as a project file.
    pub fn save_project(&self, path: &std::path::Path) -> Result<(), PlateError> {
        let bytes = self.to_project()?;
        std::fs::write(path, bytes)
            .map_err(|error| PlateError::Slice(format!("{}: {error}", path.display())))
    }

    /// Reads a project file from `path`, with `settings` as the plate's layer.
    pub fn open_project(path: &std::path::Path, settings: Settings) -> Result<Self, PlateError> {
        let bytes = std::fs::read(path)
            .map_err(|error| PlateError::Slice(format!("{}: {error}", path.display())))?;
        Plate::from_project(&bytes, settings)
    }

    /// Includes object `id` in the job, or leaves it out.
    pub fn set_object_enabled(&mut self, id: ObjectId, enabled: bool) -> Result<(), PlateError> {
        let object = self.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
        object.set_enabled(enabled);
        Ok(())
    }

    /// The job the file describes, sliced in build order.
    pub fn project_job(
        &self,
        first_layer_height: f64,
        layer_height: f64,
    ) -> Result<hfw_engine::Job, PlateError> {
        self.slice(first_layer_height, layer_height, PlateOrder::ByLayer)
    }
}

#[derive(Debug, Default)]
struct SavedObject {
    index: usize,
    enabled: bool,
    settings: Vec<(String, String)>,
    support: Vec<u8>,
    seam: Vec<u8>,
}

/// The channels a project part writes, with the names it writes them under.
fn paint_channels(paint: &Paint) -> [(&'static str, &Vec<u8>); 2] {
    [("support", &paint.support), ("seam", &paint.seam)]
}

fn project_part(plate: &Plate) -> Result<String, PlateError> {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<hfwProject version=\"{PROJECT_VERSION}\">\n"
    );
    for (index, object) in plate.objects.iter().enumerate() {
        xml.push_str(&format!(
            "  <object index=\"{index}\" enabled=\"{}\">\n",
            if object.enabled { 1 } else { 0 }
        ));
        for key in object.overrides() {
            let settings = object.settings();
            let value = settings
                .and_then(|settings| settings.key_value(key).ok())
                .ok_or_else(|| {
                    PlateError::Setting(hfw_engine::KeyError::Unknown(key.to_owned()))
                })?;
            xml.push_str(&format!(
                "    <setting key=\"{}\" value=\"{}\"/>\n",
                escape(key),
                escape(&value)
            ));
        }
        for (channel, states) in paint_channels(&object.paint) {
            if states.is_empty() {
                continue;
            }
            xml.push_str(&format!(
                "    <paint channel=\"{channel}\" runs=\"{}\"/>\n",
                encode_runs(states)
            ));
        }
        xml.push_str("  </object>\n");
    }
    xml.push_str("</hfwProject>\n");
    Ok(xml)
}

fn parse_project(xml: &str) -> Result<Vec<SavedObject>, PlateError> {
    let version = attribute_in(xml, "hfwProject", "version").unwrap_or_default();
    let version: u32 = version
        .parse()
        .map_err(|_| PlateError::Slice(format!("the project part states version {version:?}")))?;
    if !(OLDEST_PROJECT_VERSION..=PROJECT_VERSION).contains(&version) {
        return Err(PlateError::Slice(format!(
            "this project was written by version {version} of the format and this engine reads \
             versions {OLDEST_PROJECT_VERSION} to {PROJECT_VERSION}"
        )));
    }
    let mut saved = Vec::new();
    for block in elements(xml, "object") {
        let index: usize = attribute_in(&block, "object", "index")
            .ok_or_else(|| PlateError::Slice("a project object has no index".to_owned()))?
            .parse()
            .map_err(|_| {
                PlateError::Slice("a project object's index is not a number".to_owned())
            })?;
        let enabled = attribute_in(&block, "object", "enabled").unwrap_or_else(|| "1".to_owned());
        let mut settings = Vec::new();
        for setting in elements(&block, "setting") {
            let key = attribute_in(&setting, "setting", "key")
                .ok_or_else(|| PlateError::Slice("a setting has no key".to_owned()))?;
            let value = attribute_in(&setting, "setting", "value")
                .ok_or_else(|| PlateError::Slice("a setting has no value".to_owned()))?;
            settings.push((unescape(&key), unescape(&value)));
        }
        let mut support = Vec::new();
        let mut seam = Vec::new();
        for paint in elements(&block, "paint") {
            let channel = attribute_in(&paint, "paint", "channel")
                .ok_or_else(|| PlateError::Slice("a paint block has no channel".to_owned()))?;
            let runs = attribute_in(&paint, "paint", "runs")
                .ok_or_else(|| PlateError::Slice("a paint block has no runs".to_owned()))?;
            match channel.trim() {
                "support" => support = decode_runs(&runs)?,
                "seam" => seam = decode_runs(&runs)?,
                _ => continue,
            }
        }
        saved.push(SavedObject {
            index,
            enabled: enabled.trim() != "0",
            settings,
            support,
            seam,
        });
    }
    saved.sort_by_key(|saved| saved.index);
    Ok(saved)
}

/// Packs a state vector as run lengths, `state:count` separated by commas.
///
/// A painted object is mostly one state — several thousand triangles at `0`
/// between the two ears of a painted overhang — so runs keep the project part
/// readable and short enough to diff by eye.
fn encode_runs(states: &[u8]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < states.len() {
        let state = states[index];
        let mut count = 1;
        while index + count < states.len() && states[index + count] == state {
            count += 1;
        }
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(&format!("{state}:{count}"));
        index += count;
    }
    out
}

fn decode_runs(text: &str) -> Result<Vec<u8>, PlateError> {
    let bad = || PlateError::Slice(format!("the project's paint runs are malformed: {text}"));
    let mut states = Vec::new();
    for run in text.split(',') {
        let (state, count) = run.split_once(':').ok_or_else(bad)?;
        let state: u8 = state.trim().parse().map_err(|_| bad())?;
        let count: usize = count.trim().parse().map_err(|_| bad())?;
        states.extend(std::iter::repeat_n(state, count));
    }
    Ok(states)
}

fn elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start..];
        let tag_end = after.find('>').map(|end| end + 1).unwrap_or(after.len());
        if after[..tag_end].ends_with("/>") {
            out.push(after[..tag_end].to_owned());
            rest = &after[tag_end..];
            continue;
        }
        match after.find(&close) {
            Some(end) => {
                out.push(after[..end + close.len()].to_owned());
                rest = &after[end + close.len()..];
            }
            None => break,
        }
    }
    out
}

fn attribute_in(block: &str, element: &str, name: &str) -> Option<String> {
    let start = block.find(&format!("<{element}"))?;
    let tag_end = block[start..].find('>')? + start;
    let tag = &block[start..tag_end];
    let pattern = format!("{name}=\"");
    let at = tag.find(&pattern)? + pattern.len();
    let rest = &tag[at..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        format!("{}/../../fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    fn cube() -> Mesh {
        Mesh::load_model(std::path::Path::new(&fixture("cube.stl"))).expect("the fixture loads")
    }

    fn drawn_plate() -> Plate {
        let mut plate = Plate::new(Settings::default());
        let first = plate.add("bracket", cube()).expect("a cube adds");
        let second = plate.add("spacer", cube()).expect("a cube adds");
        plate
            .set_object_setting(first, "walls", "3")
            .expect("the key applies");
        plate
            .set_object_setting(first, "infill_density", "35%")
            .expect("the key applies");
        plate
            .set_object_setting(second, "support", "1")
            .expect("the key applies");
        plate
            .set_object_enabled(second, false)
            .expect("the object is disabled");
        plate.rotate(first, 2, 90.0).expect("the part turns");
        plate
            .arrange((250.0, 250.0), 5.0)
            .expect("the plate is laid out");
        plate
    }

    #[test]
    fn a_plate_survives_a_project_save_and_reopen() {
        let plate = drawn_plate();
        let bytes = plate.to_project().expect("the project is written");
        let reopened = Plate::from_project(&bytes, Settings::default()).expect("it reads back");
        assert_eq!(reopened.len(), plate.len());
        let names: Vec<&str> = reopened
            .objects()
            .iter()
            .map(|object| object.name())
            .collect();
        assert_eq!(names, vec!["bracket", "spacer"]);
        assert!(reopened.objects().first().expect("an object").enabled());
        assert!(!reopened.objects().get(1).expect("an object").enabled());
        let first = reopened.objects().first().expect("an object");
        assert_eq!(first.overrides().count(), 2);
        let settings = first
            .settings()
            .expect("the object carries its own settings");
        assert_eq!(settings.wall.walls, 3);
        assert_eq!(
            settings.infill.as_ref().map(|infill| infill.density),
            Some(0.35)
        );
    }

    #[test]
    fn the_job_a_project_describes_prints_exactly_as_the_job_it_was_saved_from() {
        let plate = drawn_plate();
        let bytes = plate.to_project().expect("the project is written");
        let reopened = Plate::from_project(&bytes, Settings::default()).expect("it reads back");
        let written = plate.project_job(0.2, 0.2).expect("the plate slices");
        let read = reopened
            .project_job(0.2, 0.2)
            .expect("the reopened plate slices");
        let config = hfw_config::PrinterConfig::default();
        let before = hfw_gcode::generate(&written.layers, &config, Some("plate"))
            .expect("the saved job is written");
        let after = hfw_gcode::generate(&read.layers, &config, Some("plate"))
            .expect("the reopened job is written");
        assert_eq!(
            before.text, after.text,
            "the project does not print what it saved"
        );
    }

    #[test]
    fn a_file_without_a_project_part_opens_as_geometry() {
        let plate = drawn_plate();
        let bytes = plate.to_project().expect("the project is written");
        let parts = archive_parts(std::io::Cursor::new(&bytes)).expect("the archive reads");
        let stripped: Vec<(&str, Vec<u8>)> = parts
            .iter()
            .filter(|(name, _)| name != PROJECT_PART)
            .map(|(name, bytes)| (name.as_str(), bytes.clone()))
            .collect();
        let plain = hfw_mesh::write_archive(&stripped).expect("the archive is written");
        let reopened = Plate::from_project(&plain, Settings::default()).expect("it reads back");
        assert_eq!(reopened.len(), 2);
        assert!(reopened.objects().iter().all(|object| object.enabled()));
        assert_eq!(
            reopened
                .objects()
                .first()
                .expect("an object")
                .overrides()
                .count(),
            0
        );
    }

    #[test]
    fn a_project_from_a_newer_format_is_refused_with_both_versions() {
        let plate = drawn_plate();
        let bytes = plate.to_project().expect("the project is written");
        let parts = archive_parts(std::io::Cursor::new(&bytes)).expect("the archive reads");
        let rewritten: Vec<(&str, Vec<u8>)> = parts
            .iter()
            .map(|(name, content)| {
                if name == PROJECT_PART {
                    let xml = String::from_utf8_lossy(content)
                        .replace(&format!("version=\"{PROJECT_VERSION}\""), "version=\"9\"");
                    (name.as_str(), xml.into_bytes())
                } else {
                    (name.as_str(), content.clone())
                }
            })
            .collect();
        let bytes = hfw_mesh::write_archive(&rewritten).expect("the archive is written");
        let refused = Plate::from_project(&bytes, Settings::default());
        match refused {
            Err(PlateError::Slice(message)) => {
                assert!(message.contains("version 9"), "message was `{message}`");
                assert!(
                    message.contains("versions 1 to 2"),
                    "the refusal names the range it reads: `{message}`"
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
