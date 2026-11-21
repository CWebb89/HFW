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
//! Placement is written as a pose, not baked into the coordinates: an object's
//! triangles stay in its own frame and `placement="sx sy sz rx ry rz tx ty tz"`
//! says where it sits. Baking it would print the same and lose the one thing a
//! panel edits -- a reopened object would come back already rotated, its transform
//! reading zero -- so the pose travels and the geometry stays editable.

use hfw_engine::{
    InLayerPolyline, Marker, MarkerKind, Mesh, Paint, PlateOrder, Settings, VolumeKind,
};
use hfw_mesh::{archive_parts, model_part_xml, objects_from_3mf_text, MeshTransform};

use crate::{Dialect, ForeignRefusal, ForeignSetting, ObjectId, Plate, PlateError};

/// The part that carries what the 3MF format has no field for.
pub const PROJECT_PART: &str = "Metadata/hfw_project.xml";

/// Version of the project part's own format.
///
/// Version 2 added per-object paint, version 3 volumes, version 4 per-object layer
/// heights, version 5 the plate's timeline markers, version 6 its in-layer strokes,
/// version 7 the settings a modifier volume prints its region with, version 8 the
/// caller's own bytes, version 9 a volume's own placement inside its object, version
/// 10 more than one plate (each with its name and lock) and a volume's name and
/// visibility, version 11 an object's label, its source path, the dialect its settings
/// are named in and the settings a foreign file carried that this engine kept rather
/// than applied. Older files still open: they simply carry none of what they predate,
/// every object lands on the document's first plate, a volume with no name sits there
/// unnamed and visible, and an object with no label shows the name it arrived with.
pub const PROJECT_VERSION: u32 = 11;

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
        // Geometry is written in the object's own frame, and where it sits on the bed is
        // recorded in the project part. Baking the placement into the triangles prints
        // the same and loses the one thing a panel edits: a reopened object would come
        // back already rotated, with a transform of zero.
        let mut placed: Vec<(String, Mesh)> = self
            .objects
            .iter()
            .map(|object| (object.name.clone(), object.mesh().clone()))
            .collect();
        for object in &self.objects {
            for volume in object.volumes() {
                placed.push((
                    format!("{} ({} volume)", object.name, volume.kind().name()),
                    volume.mesh().clone(),
                ));
            }
        }
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

        let mut parts: Vec<Option<(String, Mesh)>> = objects
            .into_iter()
            .map(|object| Some((object.name, object.mesh)))
            .collect();
        let mut volumes: Vec<SavedPlacement> = Vec::new();
        for saved in &extras.volumes {
            let geometry = parts
                .get_mut(saved.model)
                .and_then(Option::take)
                .map(|(_, mesh)| mesh)
                .ok_or_else(|| {
                    PlateError::Slice(format!(
                        "a volume points at model object {}, which the file does not have",
                        saved.model
                    ))
                })?;
            volumes.push((
                saved.object,
                saved.kind,
                saved.name.clone(),
                saved.visible,
                saved.placement,
                saved.overrides.clone(),
                Some(geometry),
            ));
        }

        let mut plate = Plate::new(settings);
        for _ in 1..extras.beds.len().max(1) {
            plate.add_bed();
        }
        for (index, (name, mesh)) in parts.into_iter().flatten().enumerate() {
            let id = plate.add_positioned(name, mesh)?;
            for (_, kind, name, visible, placement, overrides, geometry) in
                volumes.iter_mut().filter(|saved| saved.0 == index)
            {
                let geometry = geometry.take().ok_or_else(|| {
                    PlateError::Slice(format!(
                        "volume of object {index} appears twice in the file"
                    ))
                })?;
                let settings = plate.settings().clone();
                let object = plate.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
                let volume = object.add_volume_placed(*kind, geometry, *placement)?;
                let stored = object.volume_mut(volume)?;
                stored.set_name(name.clone());
                stored.set_visible(*visible);
                for (key, value) in overrides.iter() {
                    object.set_volume_setting(volume, key, value, &settings)?;
                }
            }
            let saved = extras.objects.iter().find(|saved| saved.index == index);
            if let Some(saved) = saved {
                plate.set_object_enabled(id, saved.enabled)?;
                if saved.bed >= plate.bed_count() {
                    return Err(PlateError::NoSuchBed(saved.bed));
                }
                plate.set_object_bed(id, saved.bed)?;
                let object = plate.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
                object.set_label(saved.label.clone());
                object.set_source(saved.source.clone());
                object.set_dialect(saved.dialect);
                for foreign in &saved.foreign {
                    object.record_foreign(
                        foreign.key().to_owned(),
                        foreign.value().to_owned(),
                        foreign.reason(),
                    );
                }
                if let Some(placement) = saved.placement {
                    let object = plate.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
                    object.set_placement(placement);
                }
                for (key, value) in &saved.settings {
                    plate.set_object_setting(id, key, value)?;
                }
                let object = plate.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
                object.set_paint(Paint {
                    support: saved.support.clone(),
                    seam: saved.seam.clone(),
                })?;
                object.set_layer_heights(saved.layer_heights.clone())?;
            }
        }
        plate.set_markers(extras.markers)?;
        plate.set_in_layer_polylines(extras.in_layer)?;
        plate.set_metadata(extras.metadata);
        for (index, saved) in extras.beds.iter().enumerate() {
            if let Some(bed) = plate.bed_mut(index) {
                bed.set_name(saved.name.clone());
                bed.set_locked(saved.locked);
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
    bed: usize,
    label: String,
    source: String,
    dialect: Dialect,
    foreign: Vec<ForeignSetting>,
    enabled: bool,
    placement: Option<MeshTransform>,
    settings: Vec<(String, String)>,
    support: Vec<u8>,
    seam: Vec<u8>,
    layer_heights: Vec<(f64, f64)>,
}

fn volume_kind(name: &str) -> Option<VolumeKind> {
    let name = name.trim();
    [
        VolumeKind::Part,
        VolumeKind::Negative,
        VolumeKind::SupportBlocker,
        VolumeKind::SupportEnforcer,
        VolumeKind::Modifier,
    ]
    .into_iter()
    .find(|kind| kind.name() == name)
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
        let placement = object.placement();
        xml.push_str(&format!(
            "  <object index=\"{index}\" plate=\"{}\" enabled=\"{}\" label=\"{}\" source=\"{}\" \
             dialect=\"{}\" placement=\"{} {} {} {} {} {} {} {} {}\">\n",
            object.bed(),
            if object.enabled { 1 } else { 0 },
            escape(object.label()),
            escape(object.source()),
            object.dialect().name(),
            placement.scale[0],
            placement.scale[1],
            placement.scale[2],
            placement.rotation_degrees[0],
            placement.rotation_degrees[1],
            placement.rotation_degrees[2],
            placement.translation_mm[0],
            placement.translation_mm[1],
            placement.translation_mm[2]
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
        for (z, height) in object.layer_heights() {
            xml.push_str(&format!(
                "    <layer z=\"{z:.6}\" height=\"{height:.6}\"/>\n"
            ));
        }
        for foreign in object.foreign_settings() {
            xml.push_str(&format!(
                "    <foreign key=\"{}\" value=\"{}\" reason=\"{}\"/>\n",
                escape(foreign.key()),
                escape(foreign.value()),
                foreign.reason().name()
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
    let mut model = plate.objects.len();
    for (owner, object) in plate.objects.iter().enumerate() {
        for volume in object.volumes() {
            let placement = volume.placement();
            let mut block = format!(
                "  <volume object=\"{owner}\" model=\"{model}\" kind=\"{}\" visible=\"{}\" name=\"{}\" placement=\"{} {} {} {} {} {} {} {} {}\"",
                volume.kind().name(),
                if volume.visible() { 1 } else { 0 },
                escape(volume.name()),
                placement.scale[0],
                placement.scale[1],
                placement.scale[2],
                placement.rotation_degrees[0],
                placement.rotation_degrees[1],
                placement.rotation_degrees[2],
                placement.translation_mm[0],
                placement.translation_mm[1],
                placement.translation_mm[2]
            );
            if volume.overrides().is_empty() {
                block.push_str("/>\n");
            } else {
                block.push_str(">\n");
                for (key, value) in volume.overrides() {
                    block.push_str(&format!(
                        "    <setting key=\"{}\" value=\"{}\"/>\n",
                        escape(key),
                        escape(value)
                    ));
                }
                block.push_str("  </volume>\n");
            }
            xml.push_str(&block);
            model += 1;
        }
    }
    for bed in plate.beds_iter() {
        xml.push_str(&format!(
            "  <plate name=\"{}\" locked=\"{}\"/>\n",
            escape(bed.name()),
            if bed.locked() { 1 } else { 0 }
        ));
    }
    for marker in plate.markers() {
        xml.push_str(&format!(
            "  <marker z=\"{:.6}\" kind=\"{}\" text=\"{}\"/>\n",
            marker.z,
            marker_name(marker.kind),
            escape(&marker.text)
        ));
    }
    for stroke in plate.in_layer_polylines() {
        let points: Vec<String> = stroke
            .points
            .iter()
            .map(|p| format!("{:.4} {:.4}", p.x, p.y))
            .collect();
        xml.push_str(&format!(
            "  <inLayer z=\"{:.6}\" width=\"{:.6}\" height=\"{:.6}\" speed=\"{:.4}\" \
             points=\"{}\" prefix=\"{}\"/>\n",
            stroke.z,
            stroke.line_width,
            stroke.layer_height,
            stroke.speed,
            points.join(","),
            escape(&stroke.prefix)
        ));
    }
    if !plate.metadata().is_empty() {
        xml.push_str(&format!(
            "  <metadata encoding=\"base64\">{}</metadata>\n",
            encode_base64(plate.metadata())
        ));
    }
    xml.push_str("</hfwProject>\n");
    Ok(xml)
}

/// The caller's bytes as base64: the metadata is arbitrary and the project part is
/// XML, so escaping it character by character would still fail on a byte sequence
/// that is not text at all.
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let mut quartet = [
            ALPHABET[(first >> 2) as usize],
            ALPHABET[(((first & 0b11) << 4) | (second >> 4)) as usize],
            ALPHABET[(((second & 0b1111) << 2) | (third >> 6)) as usize],
            ALPHABET[(third & 0b111111) as usize],
        ];
        let significant = match chunk.len() {
            1 => 2,
            2 => 3,
            _ => 4,
        };
        for slot in quartet.iter_mut().skip(significant) {
            *slot = b'=';
        }
        out.push_str(std::str::from_utf8(&quartet).unwrap_or_default());
    }
    out
}

fn decode_base64(text: &str) -> Result<Vec<u8>, PlateError> {
    let mut digits = Vec::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => continue,
            other => {
                return Err(PlateError::Slice(format!(
                    "the project's metadata is not base64: {:?} is not a digit",
                    other as char
                )))
            }
        };
        digits.push(value);
    }
    let mut out = Vec::with_capacity(digits.len() / 4 * 3);
    for quartet in digits.chunks(4) {
        let mut word = [0u8; 4];
        word[..quartet.len()].copy_from_slice(quartet);
        out.push((word[0] << 2) | (word[1] >> 4));
        if quartet.len() > 2 {
            out.push((word[1] << 4) | (word[2] >> 2));
        }
        if quartet.len() > 3 {
            out.push((word[2] << 6) | word[3]);
        }
    }
    Ok(out)
}

/// The name a marker's kind is written under.
fn marker_name(kind: MarkerKind) -> &'static str {
    match kind {
        MarkerKind::Custom => "custom",
        MarkerKind::Pause => "pause",
        MarkerKind::ColourChange => "colour",
    }
}

/// The kind a written name stands for.
fn marker_kind(name: &str) -> Result<MarkerKind, PlateError> {
    match name.trim() {
        "custom" => Ok(MarkerKind::Custom),
        "pause" => Ok(MarkerKind::Pause),
        "colour" | "color" => Ok(MarkerKind::ColourChange),
        other => Err(PlateError::Slice(format!(
            "{other:?} is not a marker kind this engine writes"
        ))),
    }
}

/// A project part, split into what describes an object and what describes a volume.
#[derive(Debug, Default)]
struct SavedProject {
    beds: Vec<SavedBed>,
    objects: Vec<SavedObject>,
    volumes: Vec<SavedVolume>,
    markers: Vec<Marker>,
    in_layer: Vec<InLayerPolyline>,
    metadata: Vec<u8>,
}

/// One plate as the file records it: what it is called and whether it is locked.
#[derive(Debug)]
struct SavedBed {
    name: String,
    locked: bool,
}

/// Where one volume came from: which object owns it, which model-part object holds
/// its geometry, and what it does.
/// Where a volume's geometry, placement and settings are waiting to be rebuilt on the
/// plate.
type SavedPlacement = (
    usize,
    VolumeKind,
    String,
    bool,
    MeshTransform,
    Vec<(String, String)>,
    Option<Mesh>,
);

#[derive(Debug)]
struct SavedVolume {
    object: usize,
    model: usize,
    kind: VolumeKind,
    name: String,
    visible: bool,
    placement: MeshTransform,
    overrides: Vec<(String, String)>,
}

fn parse_project(xml: &str) -> Result<SavedProject, PlateError> {
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
        let mut layer_heights = Vec::new();
        for layer in elements(&block, "layer") {
            let z = attribute_in(&layer, "layer", "z")
                .ok_or_else(|| PlateError::Slice("a layer height has no z".to_owned()))?;
            let height = attribute_in(&layer, "layer", "height")
                .ok_or_else(|| PlateError::Slice("a layer height has no height".to_owned()))?;
            let number = |text: &str, what: &str| -> Result<f64, PlateError> {
                text.trim().parse().map_err(|_| {
                    PlateError::Slice(format!("a layer height's {what} is not a number: {text:?}"))
                })
            };
            layer_heights.push((number(&z, "z")?, number(&height, "height")?));
        }
        let placement = match attribute_in(&block, "object", "placement") {
            Some(text) => {
                let numbers: Vec<f64> = text
                    .split_whitespace()
                    .map(|number| number.parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| {
                        PlateError::Slice(format!(
                            "an object's placement is not nine numbers: {text:?}"
                        ))
                    })?;
                if numbers.len() != 9 {
                    return Err(PlateError::Slice(format!(
                        "an object's placement must be nine numbers, and this one has {}",
                        numbers.len()
                    )));
                }
                Some(MeshTransform {
                    scale: [numbers[0], numbers[1], numbers[2]],
                    rotation_degrees: [numbers[3], numbers[4], numbers[5]],
                    translation_mm: [numbers[6], numbers[7], numbers[8]],
                })
            }
            None => None,
        };
        let mut foreign = Vec::new();
        for entry in elements(&block, "foreign") {
            let key = attribute_in(&entry, "foreign", "key")
                .ok_or_else(|| PlateError::Slice("a kept setting names no key".to_owned()))?;
            let value = attribute_in(&entry, "foreign", "value")
                .ok_or_else(|| PlateError::Slice("a kept setting has no value".to_owned()))?;
            let reason = attribute_in(&entry, "foreign", "reason").unwrap_or_default();
            let reason = ForeignRefusal::from_name(&reason).ok_or_else(|| {
                PlateError::Slice(format!(
                    "a kept setting says it was refused because {reason:?}, which is not one \
                     of unknown_name or unusable_value"
                ))
            })?;
            foreign.push(ForeignSetting::new(
                unescape(&key),
                unescape(&value),
                reason,
            ));
        }
        let dialect = match attribute_in(&block, "object", "dialect") {
            Some(text) => Dialect::from_name(&text).ok_or_else(|| {
                PlateError::Slice(format!(
                    "an object says its settings are named in {text:?}, which is not one of \
                     engine or orca"
                ))
            })?,
            None => Dialect::Engine,
        };
        saved.push(SavedObject {
            index,
            bed: attribute_in(&block, "object", "plate")
                .and_then(|bed| bed.trim().parse().ok())
                .unwrap_or(0),
            label: attribute_in(&block, "object", "label")
                .map(|label| unescape(&label))
                .unwrap_or_default(),
            source: attribute_in(&block, "object", "source")
                .map(|source| unescape(&source))
                .unwrap_or_default(),
            dialect,
            foreign,
            enabled: enabled.trim() != "0",
            placement,
            settings,
            support,
            seam,
            layer_heights,
        });
    }
    saved.sort_by_key(|saved| saved.index);
    let mut volumes = Vec::new();
    for block in elements(xml, "volume") {
        let object = attribute_in(&block, "volume", "object")
            .ok_or_else(|| PlateError::Slice("a volume names no object".to_owned()))?;
        let model = attribute_in(&block, "volume", "model")
            .ok_or_else(|| PlateError::Slice("a volume names no model".to_owned()))?;
        let kind = attribute_in(&block, "volume", "kind")
            .ok_or_else(|| PlateError::Slice("a volume names no kind".to_owned()))?;
        let kind = volume_kind(kind.trim()).ok_or_else(|| {
            PlateError::Slice(format!(
                "a volume is written as kind {kind:?}, which this engine does not know"
            ))
        })?;
        let number = |text: &str, what: &str| -> Result<usize, PlateError> {
            text.trim().parse().map_err(|_| {
                PlateError::Slice(format!("a volume's {what} is not a number: {text:?}"))
            })
        };
        let mut overrides = Vec::new();
        for setting in elements(&block, "setting") {
            let key = attribute_in(&setting, "setting", "key")
                .ok_or_else(|| PlateError::Slice("a volume setting names no key".to_owned()))?;
            let value = attribute_in(&setting, "setting", "value")
                .ok_or_else(|| PlateError::Slice("a volume setting has no value".to_owned()))?;
            overrides.push((key.trim().to_owned(), value));
        }
        let placement = match attribute_in(&block, "volume", "placement") {
            Some(text) => {
                let numbers: Vec<f64> = text
                    .split_whitespace()
                    .map(|number| number.parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| {
                        PlateError::Slice(format!(
                            "a volume's placement is not nine numbers: {text:?}"
                        ))
                    })?;
                if numbers.len() != 9 {
                    return Err(PlateError::Slice(format!(
                        "a volume's placement must be nine numbers, and this one has {}",
                        numbers.len()
                    )));
                }
                MeshTransform {
                    scale: [numbers[0], numbers[1], numbers[2]],
                    rotation_degrees: [numbers[3], numbers[4], numbers[5]],
                    translation_mm: [numbers[6], numbers[7], numbers[8]],
                }
            }
            None => MeshTransform::default(),
        };
        volumes.push(SavedVolume {
            object: number(&object, "object")?,
            model: number(&model, "model")?,
            kind,
            name: attribute_in(&block, "volume", "name")
                .map(|name| unescape(&name))
                .unwrap_or_default(),
            visible: attribute_in(&block, "volume", "visible")
                .map(|flag| flag.trim() != "0")
                .unwrap_or(true),
            placement,
            overrides,
        });
    }
    let mut beds = Vec::new();
    for block in elements(xml, "plate") {
        beds.push(SavedBed {
            name: attribute_in(&block, "plate", "name")
                .map(|name| unescape(&name))
                .unwrap_or_default(),
            locked: attribute_in(&block, "plate", "locked")
                .map(|flag| flag.trim() == "1")
                .unwrap_or(false),
        });
    }
    let mut markers = Vec::new();
    for marker in elements(xml, "marker") {
        let z = attribute_in(&marker, "marker", "z")
            .ok_or_else(|| PlateError::Slice("a marker has no z".to_owned()))?;
        let kind = attribute_in(&marker, "marker", "kind")
            .ok_or_else(|| PlateError::Slice("a marker has no kind".to_owned()))?;
        let text = attribute_in(&marker, "marker", "text").unwrap_or_default();
        markers.push(Marker {
            z: z.trim()
                .parse()
                .map_err(|_| PlateError::Slice(format!("a marker's z is not a number: {z:?}")))?,
            kind: marker_kind(&kind)?,
            text,
        });
    }
    let number = |field: &str, text: &str| -> Result<f64, PlateError> {
        text.trim().parse().map_err(|_| {
            PlateError::Slice(format!(
                "an in-layer stroke's {field} is not a number: {text:?}"
            ))
        })
    };
    let metadata = match elements(xml, "metadata").first() {
        Some(block) => decode_base64(&text_between(block, "metadata").unwrap_or_default())?,
        None => Vec::new(),
    };
    let mut in_layer = Vec::new();
    for stroke in elements(xml, "inLayer") {
        let field = |name: &str| {
            attribute_in(&stroke, "inLayer", name)
                .ok_or_else(|| PlateError::Slice(format!("an in-layer stroke has no {name}")))
        };
        let mut points = Vec::new();
        for pair in field("points")?.split(',') {
            let mut xy = pair.split_whitespace();
            let (Some(x), Some(y), None) = (xy.next(), xy.next(), xy.next()) else {
                return Err(PlateError::Slice(format!(
                    "an in-layer stroke's point {pair:?} is not `x y`"
                )));
            };
            let point = |text: &str, axis: &str| -> Result<f64, PlateError> {
                text.parse().map_err(|_| {
                    PlateError::Slice(format!(
                        "an in-layer stroke's {axis} is not a number: {text:?}"
                    ))
                })
            };
            points.push(hfw_kernel::V2::new(point(x, "x")?, point(y, "y")?));
        }
        in_layer.push(InLayerPolyline {
            z: number("z", &field("z")?)?,
            points,
            line_width: number("width", &field("width")?)?,
            layer_height: number("height", &field("height")?)?,
            speed: number("speed", &field("speed")?)?,
            prefix: attribute_in(&stroke, "inLayer", "prefix").unwrap_or_default(),
        });
    }
    Ok(SavedProject {
        beds,
        objects: saved,
        volumes,
        markers,
        in_layer,
        metadata,
    })
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

/// The text an element wraps, with its tags removed.
fn text_between(block: &str, element: &str) -> Option<String> {
    let open_end = block.find(&format!("<{element}"))?;
    let after_open = &block[open_end..];
    let tag_end = after_open.find('>')? + 1;
    let close = format!("</{element}>");
    let end = after_open.find(&close)?;
    if end < tag_end {
        return Some(String::new());
    }
    Some(after_open[tag_end..end].to_owned())
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
mod base64_tests {
    use super::{decode_base64, encode_base64};

    #[test]
    fn base46_round_trips_every_length() {
        for length in 0..40usize {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 7 + 3) as u8).collect();
            let text = encode_base64(&bytes);
            assert_eq!(
                decode_base64(&text).expect("what was encoded decodes"),
                bytes,
                "length {length} encoded as {text}"
            );
        }
    }
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
        let future = PROJECT_VERSION + 1;
        let rewritten: Vec<(&str, Vec<u8>)> = parts
            .iter()
            .map(|(name, content)| {
                if name == PROJECT_PART {
                    let xml = String::from_utf8_lossy(content).replace(
                        &format!("version=\"{PROJECT_VERSION}\""),
                        &format!("version=\"{future}\""),
                    );
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
                assert!(
                    message.contains(&format!("version {future}")),
                    "message was `{message}`"
                );
                assert!(
                    message.contains(&format!("versions 1 to {PROJECT_VERSION}")),
                    "the refusal names the range it reads: `{message}`"
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
