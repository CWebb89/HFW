//! The plate document: what is on the bed, and how it becomes one job.
//!
//! Everything above the engine has to answer the same question — "what is on the
//! plate" — whether it is loading a file, arranging it, showing it or slicing it.
//! This crate is that answer, once: a [`Plate`] holds [`PlateObject`]s, each with a
//! name, geometry, a placement, settings and an enabled flag, and [`Plate::slice`]
//! turns the enabled ones into one [`Job`].
//!
//! Geometry is stored as loaded and a placement says where it sits, so moving an
//! object on the plate never rewrites its triangles. A caller that would rather
//! bake a move into the geometry can do that too — [`PlateObject::bake_placement`]
//! folds the placement in and resets it — which is what a front end does once its
//! own transform stack has finished composing.
//!
//! Settings are layered rather than copied. The plate carries the job's settings;
//! an object carries settings of its own only when it has an override, and takes
//! the plate's otherwise. Applying a new printer profile to the plate therefore
//! reaches every object that did not override the key, which is exactly what a
//! preset change has to do.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fmt;

use hfw_engine::{
    merge_stacks, run_with_volumes, InLayerPolyline, Job, Marker, Mesh, MeshDiagnostics, MeshError,
    MeshTransform, Paint, PlateOrder, Settings, Volume, VolumeKind,
};

pub mod arrange;
pub mod project;

/// Identity of one object on the plate.
///
/// It survives removal of the other objects, so a view can hold it across edits.
/// It is never zero, which lets a C caller use zero for "no object".
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectId(u64);

impl ObjectId {
    /// The number behind the handle, never zero.
    pub fn get(self) -> u64 {
        self.0
    }

    /// The handle behind a number a caller stored, or `None` for zero.
    ///
    /// A non-zero number that no longer names an object on the plate is not an
    /// error here: [`Plate::object`] answers that question.
    pub fn from_number(number: u64) -> Option<Self> {
        if number == 0 {
            None
        } else {
            Some(Self(number))
        }
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why the plate could not do what was asked.
#[derive(Clone, Debug, PartialEq)]
pub enum PlateError {
    /// An object has no geometry, so it has no bounds and nothing to slice.
    NoGeometry(String),
    /// A placement could not be applied to the mesh.
    Transform(String),
    /// The engine refused to slice an object.
    Slice(String),
    /// No object on the plate is enabled, so there is nothing to print.
    NothingEnabled,
    /// No object on the plate has this id.
    NoSuchObject(ObjectId),
    /// The object does not accept this setting.
    Setting(hfw_engine::KeyError),
    /// The caller cancelled the job between objects.
    Cancelled,
    /// A part does not fit on the bed, named with the space it needed.
    DoesNotFit {
        /// The object that could not be placed.
        object: String,
        /// Footprint the packer could offer it.
        footprint: (f64, f64),
        /// Bed the packer worked in.
        bed: (f64, f64),
    },
    /// A paint channel does not hold one state per triangle.
    Paint(MeshError),
    /// A volume could not be attached or removed.
    Volume(String),
    /// An object's own layer heights were not usable.
    LayerHeights(String),
}

impl fmt::Display for PlateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlateError::NoGeometry(name) => write!(f, "{name} has no geometry"),
            PlateError::Transform(message) => write!(f, "a placement was refused: {message}"),
            PlateError::Slice(message) => write!(f, "{message}"),
            PlateError::NothingEnabled => {
                write!(f, "every object on the plate is disabled")
            }
            PlateError::NoSuchObject(id) => write!(f, "no object on the plate has id {id}"),
            PlateError::Setting(error) => write!(f, "{error}"),
            PlateError::Volume(message) => write!(f, "{message}"),
            PlateError::LayerHeights(message) => write!(f, "{message}"),
            PlateError::Cancelled => write!(f, "the job was cancelled"),
            PlateError::DoesNotFit {
                object,
                footprint,
                bed,
            } => write!(
                f,
                "{object} does not fit on a {} x {} bed: it needs {:.1} x {:.1} mm with the \
                 spacing this plate uses",
                bed.0, bed.1, footprint.0, footprint.1
            ),
            PlateError::Paint(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PlateError {}

impl From<MeshError> for PlateError {
    fn from(error: MeshError) -> Self {
        PlateError::Transform(error.to_string())
    }
}

/// What the toolhead needs from a plate before it can be printed one object at a
/// time.
///
/// Both numbers come from the printer profile, and zero means unmeasured — the
/// two checks below are the reason a profile has to state them, and guessing
/// either one drives the machine through a finished part.
#[derive(Clone, Copy, Debug)]
pub struct Clearance {
    /// Height the toolhead can pass over a finished object, in millimetres.
    pub gantry_height: f64,
    /// Radius of the toolhead around the nozzle, in millimetres.
    pub head_radius: f64,
}

impl Clearance {
    /// The clearance `settings` state, or the reason they cannot be used.
    pub fn of(settings: &Settings) -> Result<Clearance, PlateError> {
        if settings.gantry_height <= 0.0 {
            return Err(PlateError::Slice(
                "print_sequence = by object needs gantry_height in the profile: the toolhead has \
                 to pass over a finished object, and guessing that height would drive it through one"
                    .to_owned(),
            ));
        }
        if settings.extruder_clearance_radius <= 0.0 {
            return Err(PlateError::Slice(
                "print_sequence = by object needs extruder_clearance_radius in the profile: the \
                 head is beside a finished object while the next one's first layers go down, and \
                 guessing its width would drive it through one"
                    .to_owned(),
            ));
        }
        Ok(Clearance {
            gantry_height: settings.gantry_height,
            head_radius: settings.extruder_clearance_radius,
        })
    }
}

/// One object as the sequential check sees it: `(min_x, min_y, max_x, max_y)` on
/// the bed.
pub type BedBounds = (f64, f64, f64, f64);

/// An object offered to [`check_sequential`]: its name, the space it takes on the
/// bed, and how tall it is in millimetres.
pub type SequentialObject<'a> = (&'a str, BedBounds, f64);

/// Refuses a plate the toolhead cannot print one object at a time.
///
/// Two collisions, two rules. The gantry spans X, so it clears a finished object
/// only while the nozzle is at least `gantry_height` away from that object's top:
/// at the first layer the gantry's underside sits at `gantry_height`, so every
/// object has to be no taller than that or the gantry sweeps through it. The head
/// itself sits around the nozzle, so while the next object's low layers are being
/// printed it is at the height of the one already finished, right beside it: the
/// two have to be at least `head_radius` apart in the bed plane.
///
/// Each entry is a name, the object's bounds on the bed as `(min_x, min_y, max_x,
/// max_y)` and its height in millimetres. Both rules are conservative by
/// construction — they refuse a plate that might collide and never accept one
/// that can — so a plate that fails here is not made safe by trying harder.
pub fn check_sequential<'a>(
    clearance: Clearance,
    objects: impl IntoIterator<Item = SequentialObject<'a>>,
) -> Result<(), PlateError> {
    let objects: Vec<SequentialObject<'_>> = objects.into_iter().collect();
    for (index, (name, _, height)) in objects.iter().enumerate() {
        if *height > clearance.gantry_height {
            return Err(PlateError::Slice(format!(
                "{name} is {height:.1} mm tall and print_sequence = by object cannot pass the \
                 head over it (gantry_height is {:.1} mm)",
                clearance.gantry_height
            )));
        }
        for (other, bounds, _) in &objects[index + 1..] {
            let (a, b) = (objects[index].1, *bounds);
            let gap = (b.0 - a.2).max(a.0 - b.2).max((b.1 - a.3).max(a.1 - b.3));
            if gap < clearance.head_radius {
                return Err(PlateError::Slice(format!(
                    "{name} and {other} are {gap:.1} mm apart and print_sequence = by object \
                     needs {:.1} mm between them, the toolhead's own radius",
                    clearance.head_radius
                )));
            }
        }
    }
    Ok(())
}

/// One body on the plate: what it is called, where it is, and how it prints.
#[derive(Clone, Debug)]
pub struct PlateObject {
    id: ObjectId,
    name: String,
    mesh: Mesh,
    placement: MeshTransform,
    enabled: bool,
    positioned: bool,
    settings: Option<Settings>,
    paint: Paint,
    volumes: Vec<PlateVolume>,
    layer_heights: Vec<(f64, f64)>,
    overrides: BTreeSet<String>,
    ignored_overrides: BTreeSet<String>,
}

/// A second body attached to an object: geometry plus what it does.
///
/// The volume shares the object's placement, so moving a part moves what was cut
/// out of it and what it is not allowed to grow support on. That is the whole
/// reason a volume belongs to an object rather than to the plate.
#[derive(Clone, Debug)]
pub struct PlateVolume {
    kind: VolumeKind,
    mesh: Mesh,
    overrides: Vec<(String, String)>,
}

impl PlateVolume {
    /// What this volume does to the object.
    pub fn kind(&self) -> VolumeKind {
        self.kind
    }

    /// The volume's geometry, in the object's own frame.
    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }

    /// The settings this volume's region prints with, as key/value rows.
    ///
    /// Rows rather than a snapshot of the settings, so a plate preset that changes
    /// after the volume was placed still reaches the region: the rows are applied on
    /// top of whatever the object prints with at slice time.
    pub fn overrides(&self) -> &[(String, String)] {
        &self.overrides
    }
}

impl PlateObject {
    /// The object's identity on the plate.
    pub fn id(&self) -> ObjectId {
        self.id
    }

    /// The name a file or a user gave it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Renames the object.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// The geometry as the document holds it, before the placement.
    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }

    /// The geometry, for a caller that is about to rewrite it — that is what
    /// baking a placement does.
    pub fn mesh_mut(&mut self) -> &mut Mesh {
        &mut self.mesh
    }

    /// Where the geometry sits on the bed.
    pub fn placement(&self) -> MeshTransform {
        self.placement
    }

    /// Moves the object without touching its triangles.
    pub fn set_placement(&mut self, placement: MeshTransform) {
        self.placement = placement;
    }

    /// Folds the placement into the geometry and resets it to identity.
    ///
    /// A front end calls this once its own transforms have finished composing,
    /// so the document holds no placement it has to remember how to undo.
    pub fn bake_placement(&mut self) -> Result<(), PlateError> {
        if is_identity(&self.placement) {
            return Ok(());
        }
        let moved = self.mesh.transformed(self.placement)?;
        self.mesh = moved;
        self.placement = MeshTransform::default();
        Ok(())
    }

    /// Whether this object is printed.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Includes or excludes the object from the job.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// True when the file that was loaded placed this object, rather than the
    /// caller. Arranging leaves such an object where the file put it.
    pub fn positioned(&self) -> bool {
        self.positioned
    }

    /// Marks the object as positioned by its file.
    pub fn set_positioned(&mut self, positioned: bool) {
        self.positioned = positioned;
    }

    /// This object's own settings, when it overrides the plate's.
    pub fn settings(&self) -> Option<&Settings> {
        self.settings.as_ref()
    }

    /// The settings this object prints with: its own, or the plate's.
    pub fn effective_settings<'a>(&'a self, plate: &'a Settings) -> &'a Settings {
        self.settings.as_ref().unwrap_or(plate)
    }

    /// Gives the object settings of its own.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = Some(settings);
    }

    /// The painted states this object carries.
    pub fn paint(&self) -> &Paint {
        &self.paint
    }

    /// Replaces one paint channel, checking it against the object's triangles.
    pub fn set_paint(&mut self, paint: Paint) -> Result<(), PlateError> {
        paint
            .validate(self.mesh.triangle_count())
            .map_err(PlateError::Paint)?;
        self.paint = paint;
        Ok(())
    }

    /// The volumes attached to this object, in the order they were added.
    pub fn volumes(&self) -> &[PlateVolume] {
        &self.volumes
    }

    /// The object's own layer heights, as `(z, height)` pairs in millimetres.
    ///
    /// Empty when the object prints at the plate's uniform layer height. This is
    /// the object's own answer, not the plate's: two parts on one bed can step at
    /// different heights, and the file interleaves them on Z rather than on layer
    /// number.
    pub fn layer_heights(&self) -> &[(f64, f64)] {
        &self.layer_heights
    }

    /// Sets the object's own layer heights, or clears them with an empty list.
    ///
    /// Pairs are `(z, height)`: a layer whose bottom sits at or above `z` uses that
    /// height until the next pair takes over. They are checked for a rising `z` and
    /// a positive height here, and against the profile's thinnest and thickest
    /// allowed step when the object is sliced — the same place the plate's own
    /// profile is checked, so both are refused by the same code.
    pub fn set_layer_heights(&mut self, heights: Vec<(f64, f64)>) -> Result<(), PlateError> {
        let mut last = f64::NEG_INFINITY;
        for (z, height) in &heights {
            if !z.is_finite() || !height.is_finite() || *height <= 0.0 {
                return Err(PlateError::LayerHeights(format!(
                    "{z} mm at {height} mm: a layer height has to be a positive finite number"
                )));
            }
            if *z <= last {
                return Err(PlateError::LayerHeights(format!(
                    "{z} mm does not come after {last} mm: the pairs have to rise"
                )));
            }
            last = *z;
        }
        self.layer_heights = heights;
        Ok(())
    }

    /// Attaches a volume to this object.
    ///
    /// Later volumes act after earlier ones, which only changes the result when two
    /// volumes disagree: a part volume added after a negative one puts the material
    /// back where the second one covers it.
    pub fn add_volume(&mut self, kind: VolumeKind, mesh: Mesh) -> Result<(), PlateError> {
        if mesh.triangle_count() == 0 {
            return Err(PlateError::Volume(
                "a volume has no triangles, so it would change nothing".to_owned(),
            ));
        }
        self.volumes.push(PlateVolume {
            kind,
            mesh,
            overrides: Vec::new(),
        });
        Ok(())
    }

    /// Sets one setting on a modifier volume, counted in the order [`Self::volumes`]
    /// returns them.
    ///
    /// Only a modifier volume reads its own settings, so a key set on any other kind is
    /// refused rather than stored and ignored: a support blocker that quietly accepted
    /// `sparse_infill_density` would look like it had been applied.
    pub fn set_volume_setting(
        &mut self,
        index: usize,
        key: &str,
        value: &str,
        plate: &Settings,
    ) -> Result<(), PlateError> {
        let volume = self.volumes.get(index).ok_or_else(|| {
            PlateError::Volume(format!(
                "volume {index} does not exist: this object has {}",
                self.volumes.len()
            ))
        })?;
        if volume.kind != VolumeKind::Modifier {
            return Err(PlateError::Volume(format!(
                "volume {index} is a {} volume, and only a modifier volume has settings of its own",
                volume.kind.name()
            )));
        }
        let mut settings = self.settings.clone().unwrap_or_else(|| plate.clone());
        for (key, value) in &volume.overrides {
            settings
                .apply_key(key, value)
                .map_err(PlateError::Setting)?;
        }
        settings
            .apply_key(key, value)
            .map_err(PlateError::Setting)?;
        let volume = self.volumes.get_mut(index).ok_or_else(|| {
            PlateError::Volume(format!(
                "volume {index} disappeared while it was being changed"
            ))
        })?;
        volume.overrides.push((key.to_owned(), value.to_owned()));
        Ok(())
    }

    /// The settings this volume's region prints with, resolved over the object's own.
    ///
    /// `None` for every kind that has no settings of its own.
    pub fn resolved_volume_settings(
        &self,
        index: usize,
        plate: &Settings,
    ) -> Result<Option<Settings>, PlateError> {
        let Some(volume) = self.volumes.get(index) else {
            return Ok(None);
        };
        if volume.kind != VolumeKind::Modifier {
            return Ok(None);
        }
        if volume.overrides.is_empty() {
            return Err(PlateError::Volume(format!(
                "volume {index} is a modifier with no settings: its region would print with \
                 the object's own settings, which is what that region prints with anyway, and \
                 the layer would carry every bead in it twice"
            )));
        }
        let mut settings = self.settings.clone().unwrap_or_else(|| plate.clone());
        for (key, value) in &volume.overrides {
            settings
                .apply_key(key, value)
                .map_err(PlateError::Setting)?;
        }
        Ok(Some(settings))
    }

    /// Removes one volume, counted in the order [`Self::volumes`] returns them.
    pub fn remove_volume(&mut self, index: usize) -> Result<(), PlateError> {
        if index >= self.volumes.len() {
            return Err(PlateError::Volume(format!(
                "volume {index} does not exist: this object has {}",
                self.volumes.len()
            )));
        }
        self.volumes.remove(index);
        Ok(())
    }

    /// Drops the object's own settings, so it prints with the plate's again.
    pub fn clear_settings(&mut self) {
        self.settings = None;
    }

    /// The profile keys this object was given and that were honoured.
    pub fn overrides(&self) -> impl Iterator<Item = &str> {
        self.overrides.iter().map(String::as_str)
    }

    /// The profile keys this object was given that the engine cannot scope to an
    /// object, so they were not applied.
    pub fn ignored_overrides(&self) -> impl Iterator<Item = &str> {
        self.ignored_overrides.iter().map(String::as_str)
    }

    /// Records that a key was applied to this object alone.
    pub fn mark_override(&mut self, key: impl Into<String>) {
        self.overrides.insert(key.into());
    }

    /// Records that a key was refused because it cannot be scoped to an object.
    pub fn mark_ignored_override(&mut self, key: impl Into<String>) {
        self.ignored_overrides.insert(key.into());
    }

    /// The geometry with the placement applied.
    pub fn placed_mesh(&self) -> Result<Mesh, PlateError> {
        if is_identity(&self.placement) {
            return Ok(self.mesh.clone());
        }
        Ok(self.mesh.transformed(self.placement)?)
    }

    /// XY bounds on the bed in millimetres: `(min x, min y, max x, max y)`.
    ///
    /// Rotation makes this the bounds of the placed geometry, not a rotation of
    /// the original bounds, so it stays correct for any placement.
    pub fn bounds(&self) -> Result<(f64, f64, f64, f64), PlateError> {
        if is_identity(&self.placement) {
            return self
                .mesh
                .xy_bounds_mm()
                .ok_or_else(|| PlateError::NoGeometry(self.name.clone()));
        }
        self.placed_mesh()?
            .xy_bounds_mm()
            .ok_or_else(|| PlateError::NoGeometry(self.name.clone()))
    }

    /// Z extent on the bed in millimetres, as `(min, max)`.
    pub fn z_bounds(&self) -> Result<(f64, f64), PlateError> {
        if is_identity(&self.placement) {
            let (low, high) = self
                .mesh
                .z_bounds()
                .ok_or_else(|| PlateError::NoGeometry(self.name.clone()))?;
            return Ok((low as f64 / 1_000_000.0, high as f64 / 1_000_000.0));
        }
        let (low, high) = self
            .placed_mesh()?
            .z_bounds()
            .ok_or_else(|| PlateError::NoGeometry(self.name.clone()))?;
        Ok((low as f64 / 1_000_000.0, high as f64 / 1_000_000.0))
    }

    /// The object's triangle count.
    pub fn triangle_count(&self) -> usize {
        self.mesh.triangle_count()
    }
}

/// True when a placement changes nothing.
pub fn is_identity(placement: &MeshTransform) -> bool {
    placement.scale == [1.0; 3]
        && placement.rotation_degrees == [0.0; 3]
        && placement.translation_mm == [0.0; 3]
}

/// The plate: the objects on it, in the order they were added, and the settings
/// the job prints with.
#[derive(Clone, Debug)]
pub struct Plate {
    settings: Settings,
    objects: Vec<PlateObject>,
    markers: Vec<Marker>,
    in_layer: Vec<InLayerPolyline>,
    next_id: u64,
}

impl Plate {
    /// An empty plate printed with `settings`.
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            objects: Vec::new(),
            markers: Vec::new(),
            in_layer: Vec::new(),
            next_id: 0,
        }
    }

    /// The plate's timeline: what runs at a height, earliest first.
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// Replaces the plate's timeline, sorted by Z.
    ///
    /// A marker is a property of the plate rather than of the profile: it belongs to
    /// the job in front of the user, so two plates on one printer can pause at
    /// different heights. A custom marker with no G-code in it, or a Z that is not a
    /// finite height above the bed, is refused here rather than written into a file
    /// that would silently do nothing.
    pub fn set_markers(&mut self, markers: Vec<Marker>) -> Result<(), PlateError> {
        self.markers =
            Marker::sorted(markers).map_err(|error| PlateError::Slice(error.to_string()))?;
        Ok(())
    }

    /// Strokes the plate prints inside a layer that the model does not describe,
    /// earliest layer first.
    pub fn in_layer_polylines(&self) -> &[InLayerPolyline] {
        &self.in_layer
    }

    /// Replaces the plate's in-layer strokes, sorted by Z.
    ///
    /// This is the calibration channel: a temperature tower's labels, a flow test's
    /// lines. A stroke with one point, a non-positive width, height or speed, or a Z
    /// off the bed is refused here rather than printed as a bead with no length.
    pub fn set_in_layer_polylines(
        &mut self,
        strokes: Vec<InLayerPolyline>,
    ) -> Result<(), PlateError> {
        self.in_layer = InLayerPolyline::sorted(strokes)
            .map_err(|error| PlateError::Slice(error.to_string()))?;
        Ok(())
    }

    /// The settings the job prints with, which an object may override.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The job's settings, so a printer profile can be layered onto them.
    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    /// Replaces the job's settings.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }

    /// The objects on the plate, in the order they were added.
    pub fn objects(&self) -> &[PlateObject] {
        &self.objects
    }

    /// The number of objects, enabled or not.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// True when the plate holds no objects.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Adds geometry under a name, at the origin.
    pub fn add(&mut self, name: impl Into<String>, mesh: Mesh) -> Result<ObjectId, PlateError> {
        self.add_with(name.into(), mesh, false)
    }

    /// Adds geometry the file itself placed, which arranging leaves alone.
    pub fn add_positioned(
        &mut self,
        name: impl Into<String>,
        mesh: Mesh,
    ) -> Result<ObjectId, PlateError> {
        self.add_with(name.into(), mesh, true)
    }

    fn add_with(
        &mut self,
        name: String,
        mesh: Mesh,
        positioned: bool,
    ) -> Result<ObjectId, PlateError> {
        if mesh.xy_bounds_mm().is_none() {
            return Err(PlateError::NoGeometry(name));
        }
        self.next_id += 1;
        let id = ObjectId(self.next_id);
        self.objects.push(PlateObject {
            id,
            name,
            mesh,
            placement: MeshTransform::default(),
            enabled: true,
            positioned,
            settings: None,
            paint: Paint::default(),
            volumes: Vec::new(),
            layer_heights: Vec::new(),
            overrides: BTreeSet::new(),
            ignored_overrides: BTreeSet::new(),
        });
        Ok(id)
    }

    /// Copies object `id`, geometry, placement, settings and overrides included,
    /// and returns the new object's id.
    ///
    /// The copy keeps the name with ` copy` appended, because a plate of two
    /// identical names is a list a user cannot choose from.
    pub fn duplicate(&mut self, id: ObjectId) -> Result<ObjectId, PlateError> {
        let mut copy = self.object(id).ok_or(PlateError::NoSuchObject(id))?.clone();
        self.next_id += 1;
        let new_id = ObjectId(self.next_id);
        copy.name = format!("{} copy", copy.name);
        copy.id = new_id;
        self.objects.push(copy);
        Ok(new_id)
    }

    /// Turns object `id` about its own centre on the bed, by `degrees` around the
    /// X, Y or Z axis.
    ///
    /// The centre is kept where it was, so a part spins in place rather than
    /// swinging around the bed origin. Rotation is additive on that one axis, so a
    /// panel's quarter-turn buttons compose the way a user expects.
    pub fn rotate(&mut self, id: ObjectId, axis: usize, degrees: f64) -> Result<(), PlateError> {
        if axis > 2 {
            return Err(PlateError::Transform(format!(
                "axis {axis} is not one of X, Y or Z"
            )));
        }
        if !degrees.is_finite() {
            return Err(PlateError::Transform(
                "the angle is not a number".to_owned(),
            ));
        }
        let object = self.object(id).ok_or(PlateError::NoSuchObject(id))?;
        let mut placement = object.placement;
        placement.rotation_degrees[axis] += degrees;
        self.recentre(id, placement)
    }

    /// Mirrors object `id` through its own centre on the bed, along the X, Y or Z
    /// axis.
    ///
    /// Mirroring flips the triangle winding, which [`Mesh::transformed`] corrects,
    /// and the part stays where it is: a mirror about the bed origin would move it
    /// off the plate.
    pub fn mirror(&mut self, id: ObjectId, axis: usize) -> Result<(), PlateError> {
        if axis > 2 {
            return Err(PlateError::Transform(format!(
                "axis {axis} is not one of X, Y or Z"
            )));
        }
        let object = self.object(id).ok_or(PlateError::NoSuchObject(id))?;
        let mut placement = object.placement;
        placement.scale[axis] = -placement.scale[axis];
        self.recentre(id, placement)
    }

    fn recentre(&mut self, id: ObjectId, mut placement: MeshTransform) -> Result<(), PlateError> {
        let object = self.object(id).ok_or(PlateError::NoSuchObject(id))?;
        let (min_x, min_y, max_x, max_y) = object.bounds()?;
        let turned = object
            .mesh
            .transformed(placement)
            .map_err(|error| PlateError::Transform(error.to_string()))?;
        let (low_x, low_y, high_x, high_y) = turned
            .xy_bounds_mm()
            .ok_or_else(|| PlateError::NoGeometry(object.name.clone()))?;
        placement.translation_mm[0] += 0.5 * (min_x + max_x) - 0.5 * (low_x + high_x);
        placement.translation_mm[1] += 0.5 * (min_y + max_y) - 0.5 * (low_y + high_y);
        let object = self.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
        object.placement = placement;
        Ok(())
    }

    /// Lays every enabled object out on `bed`, leaving `spacing` millimetres of
    /// air between any two objects.
    ///
    /// The rule is [`arrange::layout`]'s: objects a file positioned keep their
    /// coordinates and the rest are packed around them. Placement is replaced, not
    /// composed — arranging is a fresh layout — so a part that does not fit is an
    /// error rather than a silent omission. Geometry is never rewritten.
    pub fn arrange(&mut self, bed: (f64, f64), spacing: f64) -> Result<(), PlateError> {
        let mut footprints = Vec::new();
        let mut ids = Vec::new();
        for object in self.objects.iter().filter(|object| object.enabled) {
            if let Ok(bounds) = object.bounds() {
                footprints.push(arrange::Footprint {
                    bounds,
                    positioned: object.positioned,
                });
                ids.push(object.id);
            }
        }
        if footprints.is_empty() {
            return Ok(());
        }
        let transforms = arrange::layout(&footprints, bed, spacing).map_err(|error| {
            let object = self.object(ids[error.item]);
            PlateError::DoesNotFit {
                object: object.map(|object| object.name.clone()).unwrap_or_default(),
                footprint: error.footprint,
                bed: error.bed,
            }
        })?;
        for (id, transform) in ids.into_iter().zip(transforms) {
            let object = self.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
            object.placement = transform;
        }
        Ok(())
    }

    /// The object with this id.
    pub fn object(&self, id: ObjectId) -> Option<&PlateObject> {
        self.objects.iter().find(|object| object.id == id)
    }

    /// The object with this id, for changing it.
    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut PlateObject> {
        self.objects.iter_mut().find(|object| object.id == id)
    }

    /// Sets one setting on one object.
    ///
    /// The object takes the plate's settings the first time an override reaches
    /// it, so the result is a delta from the job rather than a fresh set of
    /// defaults: a panel that sets `walls` on one part does not also silently
    /// reset its infill density. A key or value this engine cannot apply is
    /// refused with the reason, and the object is left as it was.
    pub fn set_object_setting(
        &mut self,
        id: ObjectId,
        key: &str,
        value: &str,
    ) -> Result<(), PlateError> {
        let plate = self.settings.clone();
        let object = self.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
        let mut settings = object.settings.clone().unwrap_or(plate);
        settings
            .apply_key(key, value)
            .map_err(PlateError::Setting)?;
        object.settings = Some(settings);
        object.mark_override(key.trim().to_ascii_lowercase());
        Ok(())
    }

    /// Drops an object's own settings, so it prints with the plate's again.
    pub fn clear_object_settings(&mut self, id: ObjectId) -> Result<(), PlateError> {
        let object = self.object_mut(id).ok_or(PlateError::NoSuchObject(id))?;
        object.settings = None;
        object.overrides.clear();
        object.ignored_overrides.clear();
        Ok(())
    }

    /// Removes an object, keeping every other id valid.
    pub fn remove(&mut self, id: ObjectId) -> Option<PlateObject> {
        let index = self.objects.iter().position(|object| object.id == id)?;
        Some(self.objects.remove(index))
    }

    /// XY bounds of every object on the plate, in millimetres.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        self.objects
            .iter()
            .filter_map(|object| object.bounds().ok())
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
    }

    /// Z extent of every object on the plate, in millimetres.
    pub fn z_bounds(&self) -> Option<(f64, f64)> {
        self.objects
            .iter()
            .filter_map(|object| object.z_bounds().ok())
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    }

    /// Total triangle count of every object on the plate.
    pub fn triangle_count(&self) -> usize {
        self.objects.iter().map(PlateObject::triangle_count).sum()
    }

    /// Slices every enabled object and merges the stacks into one job.
    ///
    /// Each object is sliced on its own, so its own settings apply to it alone;
    /// the stacks are interleaved by `order`. Layer heights are millimetres, and a
    /// value that is not positive is refused rather than guessed.
    pub fn slice(
        &self,
        first_layer_height: f64,
        layer_height: f64,
        order: PlateOrder,
    ) -> Result<Job, PlateError> {
        self.slice_using(&self.settings, first_layer_height, layer_height, order)
    }

    /// Slices the plate with `settings` as the job-wide layer, without storing
    /// them.
    ///
    /// This is how a printer profile is applied for one job: the caller layers it
    /// onto its own copy of the settings and passes it here, so the document keeps
    /// what it was told rather than what it was last sliced with.
    pub fn slice_using(
        &self,
        settings: &Settings,
        first_layer_height: f64,
        layer_height: f64,
        order: PlateOrder,
    ) -> Result<Job, PlateError> {
        self.slice_using_with_progress(settings, first_layer_height, layer_height, order, |_, _| {
            true
        })
    }

    /// Slices the plate like [`Plate::slice_using`], reporting progress and
    /// stopping when the caller says so.
    ///
    /// `progress` is called before each object is planned, with the number of
    /// objects planned so far and the number to plan, in plate order. Returning
    /// `false` stops the job and yields [`PlateError::Cancelled`] rather than a
    /// partial job, so a cancelled slice can never be written by mistake.
    pub fn slice_using_with_progress(
        &self,
        settings: &Settings,
        first_layer_height: f64,
        layer_height: f64,
        order: PlateOrder,
        progress: impl FnMut(usize, usize) -> bool,
    ) -> Result<Job, PlateError> {
        if first_layer_height <= 0.0 || layer_height <= 0.0 {
            return Err(PlateError::Slice(
                "a layer height has to be positive".to_owned(),
            ));
        }
        let objects: Vec<&PlateObject> = self.objects.iter().filter(|o| o.enabled).collect();
        if objects.is_empty() {
            return Err(PlateError::NothingEnabled);
        }
        if order == PlateOrder::ByObject {
            let clearance = Clearance::of(settings)?;
            let mut placed = Vec::with_capacity(objects.len());
            for object in &objects {
                let (low, high) = object.z_bounds()?;
                placed.push((object.name(), object.bounds()?, high - low));
            }
            check_sequential(clearance, placed)?;
        }
        let mut moved: Vec<Option<Mesh>> = Vec::with_capacity(objects.len());
        let mut moved_volumes: Vec<Vec<Option<Mesh>>> = Vec::with_capacity(objects.len());
        let mut stepped: Vec<Option<Settings>> = Vec::with_capacity(objects.len());
        for object in &objects {
            let identity = is_identity(&object.placement);
            moved.push(if identity {
                None
            } else {
                Some(object.placed_mesh()?)
            });
            let mut volumes = Vec::with_capacity(object.volumes().len());
            for volume in object.volumes() {
                volumes.push(if identity {
                    None
                } else {
                    Some(
                        volume
                            .mesh()
                            .transformed(object.placement)
                            .map_err(|error| PlateError::Transform(error.to_string()))?,
                    )
                });
            }
            moved_volumes.push(volumes);
            stepped.push(if object.layer_heights().is_empty() {
                None
            } else {
                let mut own = object.settings().unwrap_or(settings).clone();
                own.layer_profile = object.layer_heights().to_vec();
                Some(own)
            });
        }
        let placed: Vec<PlacedObject> = objects
            .iter()
            .zip(&moved)
            .zip(&moved_volumes)
            .zip(&stepped)
            .map(|(((object, moved), volumes), stepped)| {
                let modifiers = object
                    .volumes()
                    .iter()
                    .enumerate()
                    .zip(volumes)
                    .map(|((index, volume), moved)| {
                        Ok(PlacedVolume {
                            mesh: moved.as_ref().unwrap_or(volume.mesh()),
                            kind: volume.kind(),
                            settings: object.resolved_volume_settings(index, settings)?,
                        })
                    })
                    .collect::<Result<Vec<_>, PlateError>>()?;
                Ok(PlacedObject {
                    mesh: moved.as_ref().unwrap_or(&object.mesh),
                    paint: Some(&object.paint),
                    settings: stepped
                        .as_ref()
                        .unwrap_or_else(|| object.settings().unwrap_or(settings)),
                    volumes: modifiers,
                })
            })
            .collect::<Result<Vec<_>, PlateError>>()?;
        let mut job =
            slice_placed_with_progress(placed, first_layer_height, layer_height, order, progress)?;
        job.markers = self.markers.clone();
        job.in_layer = self.in_layer.clone();
        Ok(job)
    }
}

/// One object as the slicer takes it: placed geometry, the paint on it, the
/// settings it prints with, and the volumes attached to it.
pub struct PlacedObject<'a> {
    /// Geometry, already placed on the bed.
    pub mesh: &'a Mesh,
    /// Per-triangle paint, or `None` when nothing is painted.
    pub paint: Option<&'a Paint>,
    /// The settings this object prints with.
    pub settings: &'a Settings,
    /// Volumes attached to it, already placed in its frame.
    pub volumes: Vec<PlacedVolume<'a>>,
}

/// One volume as the slicer takes it: geometry, what it does, and the settings its
/// region prints with.
pub struct PlacedVolume<'a> {
    /// The geometry, already placed in the object's frame.
    pub mesh: &'a Mesh,
    /// What the volume does.
    pub kind: VolumeKind,
    /// Settings this volume's region prints with, resolved over the object's own.
    ///
    /// `None` for every kind that has no region of its own, and an error rather than a
    /// pass-through for a modifier, whose whole purpose is to carry them.
    pub settings: Option<Settings>,
}

impl<'a> PlacedObject<'a> {
    /// An object with no volumes attached.
    pub fn new(mesh: &'a Mesh, paint: Option<&'a Paint>, settings: &'a Settings) -> Self {
        PlacedObject {
            mesh,
            paint,
            settings,
            volumes: Vec::new(),
        }
    }
}

/// Slices objects that are already placed on the bed, and merges them into one
/// job.
///
/// This is the whole of "a plate becomes a job" — one implementation, used by
/// [`Plate::slice`] and by callers that hold placed meshes and no document.
pub fn slice_placed<'a>(
    objects: impl IntoIterator<Item = PlacedObject<'a>>,
    first_layer_height: f64,
    layer_height: f64,
    order: PlateOrder,
) -> Result<Job, PlateError> {
    slice_placed_with_progress(objects, first_layer_height, layer_height, order, |_, _| {
        true
    })
}

/// Slices placed objects like [`slice_placed`], reporting each object as it is
/// planned and stopping when `progress` returns `false`.
///
/// Progress is per object, in plate order, because that is where the time goes:
/// the merge afterwards is linear in layers and cannot be interrupted usefully.
pub fn slice_placed_with_progress<'a>(
    objects: impl IntoIterator<Item = PlacedObject<'a>>,
    first_layer_height: f64,
    layer_height: f64,
    order: PlateOrder,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<Job, PlateError> {
    let objects: Vec<PlacedObject> = objects.into_iter().collect();
    let total = objects.len();
    if order == PlateOrder::ByObject {
        let mut clearance: Option<Clearance> = None;
        let mut placed = Vec::with_capacity(objects.len());
        for (index, object) in objects.iter().enumerate() {
            let mesh = object.mesh;
            let own = Clearance::of(object.settings)?;
            clearance = Some(match clearance {
                None => own,
                Some(so_far) => Clearance {
                    gantry_height: so_far.gantry_height.min(own.gantry_height),
                    head_radius: so_far.head_radius.max(own.head_radius),
                },
            });
            let (min_x, min_y, max_x, max_y) = mesh
                .xy_bounds_mm()
                .ok_or_else(|| PlateError::NoGeometry(format!("object {}", index + 1)))?;
            let (low, high) = mesh
                .z_bounds()
                .ok_or_else(|| PlateError::NoGeometry(format!("object {}", index + 1)))?;
            placed.push((
                format!("object {}", index + 1),
                (min_x, min_y, max_x, max_y),
                (high - low) as f64 / 1_000_000.0,
            ));
        }
        if let Some(clearance) = clearance {
            check_sequential(
                clearance,
                placed
                    .iter()
                    .map(|(name, bounds, height)| (name.as_str(), *bounds, *height)),
            )?;
        }
    }
    let mut stacks = Vec::new();
    let mut diagnostics = MeshDiagnostics::default();
    for (done, object) in objects.into_iter().enumerate() {
        if !progress(done, total) {
            return Err(PlateError::Cancelled);
        }
        add_diagnostics(&mut diagnostics, object.mesh.diagnostics());
        let paint = object.paint.cloned().unwrap_or_default();
        let volumes: Vec<Volume> = object
            .volumes
            .iter()
            .map(|volume| Volume {
                mesh: volume.mesh,
                kind: volume.kind,
                settings: volume.settings.as_ref(),
            })
            .collect();
        stacks.push(
            run_with_volumes(
                object.mesh,
                &paint,
                &volumes,
                first_layer_height,
                layer_height,
                object.settings,
            )
            .map_err(|error| PlateError::Slice(error.to_string()))?,
        );
    }
    if stacks.is_empty() {
        return Err(PlateError::NothingEnabled);
    }
    Ok(merge_stacks(stacks, order, diagnostics))
}

/// Counters of several meshes, summed.
///
/// Every field of `MeshDiagnostics` is a count of a defect or of an element, so
/// adding them reports what one mesh holding all of them would report.
fn add_diagnostics(total: &mut MeshDiagnostics, extra: MeshDiagnostics) {
    total.vertices += extra.vertices;
    total.triangles += extra.triangles;
    total.repeated_index_triangles += extra.repeated_index_triangles;
    total.zero_area_triangles += extra.zero_area_triangles;
    total.boundary_or_non_manifold_edges += extra.boundary_or_non_manifold_edges;
    total.boundary_edges += extra.boundary_edges;
    total.non_manifold_edges += extra.non_manifold_edges;
    total.inconsistently_oriented_edges += extra.inconsistently_oriented_edges;
}
