//! A whole document, kept and put back: what an undo step is made of.
//!
//! A snapshot is the plate at one moment — its objects with their ids, geometry,
//! placements, paint, settings, volumes, layer heights; its timeline; its in-layer
//! strokes; the caller's own bytes; and the counter that hands out the next object
//! id — so restoring it reproduces the document rather than approximating it. The
//! id counter is in there for the same reason the ids are: undoing to a state and
//! then adding a part must not reuse an id that a selection, a paint session or an
//! undo step of its own still holds.
//!
//! What makes this cheap enough to do before every edit is that geometry is not
//! copied. Each object's mesh is behind an [`Arc`](std::sync::Arc) shared with the
//! live plate, so a snapshot costs one pointer per object and one counter bump on
//! the meshes it shares; the first write to an object after a snapshot clones its
//! mesh and leaves the snapshot holding the triangles it was taken with. A stack of
//! twenty undo steps over a five-hundred-megabyte plate therefore costs the plate
//! plus what was edited, not twenty plates.
//!
//! A snapshot owns its shares and outlives the plate it came from: freeing the
//! plate leaves the snapshot intact and restorable into another one.

use hfw_engine::{InLayerPolyline, Marker, Settings};

use crate::{Bed, Plate, PlateObject};

/// The plate as it was, restorable as it was.
#[derive(Clone, Debug)]
pub struct PlateSnapshot {
    settings: Settings,
    beds: Vec<Bed>,
    objects: Vec<PlateObject>,
    markers: Vec<Marker>,
    in_layer: Vec<InLayerPolyline>,
    metadata: Vec<u8>,
    next_id: u64,
}

impl PlateSnapshot {
    /// How many objects the snapshot holds, enabled or not.
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// How many plates the snapshot holds, which is never zero.
    pub fn bed_count(&self) -> usize {
        self.beds.len()
    }

    /// A plate's name and lock, as the snapshot holds them.
    pub fn bed(&self, bed: usize) -> Option<&Bed> {
        self.beds.get(bed)
    }

    /// The ids the snapshot holds, in the order the objects were added.
    pub fn object_ids(&self) -> Vec<crate::ObjectId> {
        self.objects.iter().map(PlateObject::id).collect()
    }

    /// The painted state the snapshot holds for one object, if it holds it.
    pub fn paint(&self, id: crate::ObjectId) -> Option<&hfw_engine::Paint> {
        self.objects
            .iter()
            .find(|object| object.id() == id)
            .map(PlateObject::paint)
    }

    /// Total triangle count of every object in the snapshot.
    pub fn triangle_count(&self) -> usize {
        self.objects
            .iter()
            .map(PlateObject::triangle_count)
            .sum::<usize>()
    }
}

impl Plate {
    /// The plate as it is now, sharing its geometry with the snapshot.
    ///
    /// Taking one before every edit is what an undo stack is: the call itself
    /// copies no triangles, so its cost is the objects' own bookkeeping.
    pub fn snapshot(&self) -> PlateSnapshot {
        PlateSnapshot {
            settings: self.settings.clone(),
            beds: self.beds.clone(),
            objects: self.objects.clone(),
            markers: self.markers.clone(),
            in_layer: self.in_layer.clone(),
            metadata: self.metadata.clone(),
            next_id: self.next_id,
        }
    }

    /// Replaces everything the plate holds with what `snapshot` holds.
    ///
    /// The snapshot is not consumed: the same one can be restored again, and into
    /// another plate. What it brings back shares geometry with the live plate until
    /// that geometry is written to, so restoring costs no more than taking one.
    pub fn restore(&mut self, snapshot: &PlateSnapshot) {
        self.settings = snapshot.settings.clone();
        self.beds = snapshot.beds.clone();
        self.objects = snapshot.objects.clone();
        self.markers = snapshot.markers.clone();
        self.in_layer = snapshot.in_layer.clone();
        self.metadata = snapshot.metadata.clone();
        self.next_id = snapshot.next_id;
    }
}

#[cfg(test)]
mod tests {
    use crate::Plate;
    use hfw_engine::{Mesh, MeshTransform, Settings};
    use std::sync::Arc;

    fn plate() -> Plate {
        let mut plate = Plate::new(Settings::default());
        for (index, size) in [10.0, 20.0, 30.0].into_iter().enumerate() {
            let mesh = Mesh::box_mm(size, size, size).expect("a box builds");
            plate.add(format!("box {index}"), mesh).expect("a box adds");
        }
        plate
    }

    #[test]
    fn a_snapshot_shares_the_triangles_it_was_taken_with() {
        let plate = plate();
        let snapshot = plate.snapshot();
        assert_eq!(
            Arc::as_ptr(&plate.objects[0].mesh),
            Arc::as_ptr(&snapshot.objects[0].mesh),
            "the snapshot shares the live plate's mesh rather than copying it"
        );
        assert_eq!(
            Arc::strong_count(&snapshot.objects[0].mesh),
            2,
            "live plate and snapshot hold the one allocation"
        );
    }

    #[test]
    fn writing_to_an_object_after_a_snapshot_leaves_the_snapshot_alone() {
        let mut plate = plate();
        let snapshot = plate.snapshot();
        let before = snapshot.triangle_count();
        let address = Arc::as_ptr(&snapshot.objects[0].mesh);

        let id = plate.objects[0].id();
        let object = plate.object_mut(id).expect("the object is there");
        let scaled = object
            .mesh()
            .transformed(MeshTransform {
                scale: [2.0, 2.0, 2.0],
                ..MeshTransform::default()
            })
            .expect("the mesh scales");
        *object.mesh_mut() = scaled;

        assert_eq!(
            Arc::as_ptr(&snapshot.objects[0].mesh),
            address,
            "the snapshot still points at the allocation it was taken with"
        );
        assert_eq!(
            snapshot.triangle_count(),
            before,
            "the snapshot's geometry did not change with the plate's"
        );
        assert_ne!(
            Arc::as_ptr(&plate.objects[0].mesh),
            address,
            "the write copied before it changed anything"
        );
    }

    #[test]
    fn restoring_brings_back_the_id_counter_as_well_as_the_ids() {
        let mut plate = plate();
        let snapshot = plate.snapshot();
        let ids = snapshot.object_ids();
        let invented = plate
            .add("later", Mesh::box_mm(4.0, 4.0, 4.0).expect("a box builds"))
            .expect("a box adds");
        assert!(!ids.contains(&invented));

        plate.restore(&snapshot);
        assert_eq!(
            plate
                .objects()
                .iter()
                .map(|object| object.id())
                .collect::<Vec<_>>(),
            ids,
            "every id came back"
        );
        let again = plate
            .add("again", Mesh::box_mm(4.0, 4.0, 4.0).expect("a box builds"))
            .expect("a box adds");
        assert_eq!(
            again, invented,
            "an id was never handed out twice for the same state"
        );
    }

    #[test]
    fn a_snapshot_outlives_the_plate_it_came_from() {
        let snapshot = {
            let plate = plate();
            plate.snapshot()
        };
        let mut other = Plate::new(Settings::default());
        other.restore(&snapshot);
        assert_eq!(other.len(), 3);
        assert_eq!(other.triangle_count(), snapshot.triangle_count());
    }
}
