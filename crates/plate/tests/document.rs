//! The document model, exercised the way a front end would use it.

use hfw_engine::{Mesh, MeshTransform, PlateOrder, Settings};
use hfw_plate::{ObjectId, Plate, PlateError};

fn fixture(name: &str) -> String {
    format!("{}/../../fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn cube() -> Mesh {
    Mesh::load_model(std::path::Path::new(&fixture("cube.stl"))).expect("the fixture loads")
}

fn plate() -> Plate {
    Plate::new(Settings::default())
}

/// A post under a wide table: the reason support exists.
fn table() -> Mesh {
    let post = Mesh::box_mm(4.0, 20.0, 20.0).expect("a box builds");
    let top = Mesh::box_mm(30.0, 20.0, 5.0)
        .expect("a box builds")
        .transformed(MeshTransform {
            translation_mm: [0.0, 0.0, 20.0],
            ..MeshTransform::default()
        })
        .expect("the table lifts");
    Mesh::merged(&[post, top]).expect("the two boxes merge")
}

#[test]
fn an_object_keeps_its_name_and_identity_across_the_removal_of_its_neighbours() {
    let mut plate = plate();
    let first = plate.add("first", cube()).expect("a cube adds");
    let second = plate.add("second", cube()).expect("a cube adds");
    assert_ne!(first, second);
    assert_eq!(plate.object(first).map(|o| o.name()), Some("first"));
    let removed = plate.remove(second).expect("the object is on the plate");
    assert_eq!(removed.name(), "second");
    assert_eq!(plate.len(), 1);
    assert_eq!(plate.object(first).map(|o| o.id()), Some(first));
    assert_eq!(plate.object(second).map(|o| o.id()), None);
    assert_eq!(plate.remove(second).map(|o| o.id()), None);
}

#[test]
fn a_placement_moves_the_object_but_not_its_triangles() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    let before = plate.object(id).expect("added").mesh().clone();
    let object = plate.object_mut(id).expect("added");
    object.set_placement(MeshTransform {
        translation_mm: [40.0, 5.0, 0.0],
        ..MeshTransform::default()
    });
    let bounds = object.bounds().expect("a cube has bounds");
    assert!((bounds.0 - 40.0).abs() < 1e-6, "min x was {}", bounds.0);
    assert!((bounds.1 - 5.0).abs() < 1e-6, "min y was {}", bounds.1);
    assert_eq!(object.mesh().triangle_count(), before.triangle_count());
    assert_eq!(object.mesh().xy_bounds_mm(), before.xy_bounds_mm());
}

#[test]
fn baking_a_placement_leaves_the_placed_geometry_where_it_was() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    let placement = MeshTransform {
        rotation_degrees: [0.0, 0.0, 30.0],
        translation_mm: [12.0, -3.0, 2.0],
        ..MeshTransform::default()
    };
    let object = plate.object_mut(id).expect("added");
    object.set_placement(placement);
    let placed = object.bounds().expect("bounds");
    let z = object.z_bounds().expect("z bounds");
    object.bake_placement().expect("the placement bakes");
    assert!(hfw_plate::is_identity(&object.placement()));
    let baked = object.bounds().expect("bounds");
    assert!((baked.0 - placed.0).abs() < 1e-6);
    assert!((baked.1 - placed.1).abs() < 1e-6);
    assert!((baked.2 - placed.2).abs() < 1e-6);
    assert!((baked.3 - placed.3).abs() < 1e-6);
    let baked_z = object.z_bounds().expect("z bounds");
    assert!((baked_z.0 - z.0).abs() < 1e-6);
    assert!((baked_z.1 - z.1).abs() < 1e-6);
}

#[test]
fn the_plate_bounds_cover_every_object_on_it() {
    let mut plate = plate();
    let left = plate.add("left", cube()).expect("a cube adds");
    let right = plate.add("right", cube()).expect("a cube adds");
    plate
        .object_mut(right)
        .expect("added")
        .set_placement(MeshTransform {
            translation_mm: [50.0, 0.0, 0.0],
            ..MeshTransform::default()
        });
    let bounds = plate.bounds().expect("the plate has bounds");
    assert!((bounds.0 - 0.0).abs() < 1e-6);
    assert!((bounds.2 - 70.0).abs() < 1e-6, "max x was {}", bounds.2);
    let z = plate.z_bounds().expect("the plate has a height");
    assert!((z.1 - 20.0).abs() < 1e-6, "max z was {}", z.1);
    assert_eq!(plate.triangle_count(), 24);
    assert_eq!(plate.object(left).expect("added").triangle_count(), 12);
}

#[test]
fn an_object_without_settings_prints_with_the_plates() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    assert!(plate.object(id).expect("added").settings().is_none());
    plate.settings_mut().wall.walls = 5;
    assert_eq!(
        plate
            .object(id)
            .expect("added")
            .effective_settings(plate.settings())
            .wall
            .walls,
        5
    );
    let mut own = plate.settings().clone();
    own.wall.walls = 1;
    plate.object_mut(id).expect("added").set_settings(own);
    assert_eq!(
        plate
            .object(id)
            .expect("added")
            .effective_settings(plate.settings())
            .wall
            .walls,
        1
    );
    plate.object_mut(id).expect("added").clear_settings();
    assert_eq!(
        plate
            .object(id)
            .expect("added")
            .effective_settings(plate.settings())
            .wall
            .walls,
        5
    );
}

#[test]
fn a_disabled_object_is_left_out_of_the_job() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    plate.object_mut(id).expect("added").set_enabled(false);
    assert_eq!(
        plate.slice(0.2, 0.2, PlateOrder::ByLayer).err(),
        Some(PlateError::NothingEnabled)
    );
    plate.object_mut(id).expect("added").set_enabled(true);
    let job = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("a cube slices");
    assert_eq!(job.layers.len(), 100);
}

#[test]
fn two_objects_make_one_job_with_one_layer_count() {
    let mut plate = plate();
    plate.add("left", cube()).expect("a cube adds");
    let right = plate.add("right", cube()).expect("a cube adds");
    plate
        .object_mut(right)
        .expect("added")
        .set_placement(MeshTransform {
            translation_mm: [40.0, 0.0, 0.0],
            ..MeshTransform::default()
        });
    let job = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the plate slices");
    assert_eq!(
        job.layers.len(),
        200,
        "two objects, one hundred layers each"
    );
    let labelled: Vec<u16> = job
        .layers
        .iter()
        .flat_map(|layer| layer.paths.iter().map(|line| line.object))
        .collect();
    assert!(labelled.contains(&0) && labelled.contains(&1));
}

#[test]
fn a_non_positive_layer_height_is_refused_rather_than_guessed() {
    let mut plate = plate();
    plate.add("cube", cube()).expect("a cube adds");
    assert!(plate.slice(0.0, 0.2, PlateOrder::ByLayer).is_err());
    assert!(plate.slice(0.2, 0.0, PlateOrder::ByLayer).is_err());
    assert!(plate.slice(-0.2, 0.2, PlateOrder::ByLayer).is_err());
}

#[test]
fn a_job_reports_each_object_it_plans() {
    let mut plate = plate();
    plate.add("first", cube()).expect("a cube adds");
    plate.add("second", cube()).expect("a cube adds");
    let mut seen = Vec::new();
    let settings = Settings::default();
    plate
        .slice_using_with_progress(&settings, 0.2, 0.2, PlateOrder::ByLayer, |done, total| {
            seen.push((done, total));
            true
        })
        .expect("the plate slices");
    assert_eq!(seen, vec![(0, 2), (1, 2)]);
}

#[test]
fn a_job_stopped_between_objects_yields_no_job_at_all() {
    let mut plate = plate();
    plate.add("first", cube()).expect("a cube adds");
    plate.add("second", cube()).expect("a cube adds");
    let settings = Settings::default();
    let stopped =
        plate.slice_using_with_progress(&settings, 0.2, 0.2, PlateOrder::ByLayer, |done, _| {
            done < 1
        });
    assert!(matches!(stopped, Err(PlateError::Cancelled)));
    let unstopped = plate.slice(0.2, 0.2, PlateOrder::ByLayer);
    assert!(
        unstopped.is_ok(),
        "the same plate still slices when not stopped"
    );
}

#[test]
fn a_duplicate_carries_everything_and_takes_a_new_id() {
    let mut plate = plate();
    let first = plate.add("bracket", cube()).expect("a cube adds");
    plate
        .set_object_setting(first, "walls", "3")
        .expect("the key applies");
    let copy = plate.duplicate(first).expect("the object duplicates");
    assert_eq!(plate.len(), 2);
    let original = plate.object(first).expect("the original is still there");
    let duplicated = plate.object(copy).expect("the copy is there");
    assert_eq!(duplicated.name(), "bracket copy");
    assert_eq!(duplicated.overrides().count(), original.overrides().count());
    assert_eq!(
        duplicated.mesh().triangle_count(),
        original.mesh().triangle_count()
    );
    assert_eq!(
        duplicated.bounds().expect("the copy has bounds"),
        original.bounds().expect("the original has bounds")
    );
}

#[test]
fn turning_and_mirroring_keep_a_part_where_it_stood() {
    let mut plate = plate();
    let id = plate.add("bracket", cube()).expect("a cube adds");
    let before = plate
        .object(id)
        .expect("the object is there")
        .bounds()
        .expect("bounds");
    plate.rotate(id, 2, 90.0).expect("the part turns");
    let turned = plate
        .object(id)
        .expect("the object is there")
        .bounds()
        .expect("bounds");
    assert!((0.5 * (before.0 + before.2) - 0.5 * (turned.0 + turned.2)).abs() < 1e-9);
    assert!((0.5 * (before.1 + before.3) - 0.5 * (turned.1 + turned.3)).abs() < 1e-9);
    assert!((before.2 - before.0 - (turned.3 - turned.1)).abs() < 1e-9);
    plate.mirror(id, 0).expect("the part mirrors");
    let mirrored = plate
        .object(id)
        .expect("the object is there")
        .bounds()
        .expect("bounds");
    assert!((turned.0 - mirrored.0).abs() < 1e-9);
    assert!((turned.2 - mirrored.2).abs() < 1e-9);
}

#[test]
fn four_objects_are_laid_out_on_the_bed_without_touching() {
    let mut plate = plate();
    for index in 0..4 {
        plate
            .add(format!("part {index}"), cube())
            .expect("a cube adds");
    }
    plate
        .arrange((250.0, 250.0), 5.0)
        .expect("the plate is laid out");
    let mut corners = Vec::new();
    for object in plate.objects() {
        let (min_x, min_y, max_x, max_y) = object.bounds().expect("bounds");
        assert!(min_x >= -1e-9 && min_y >= -1e-9, "a part fell off the bed");
        assert!(
            max_x <= 250.0 && max_y <= 250.0,
            "a part hangs over the bed"
        );
        corners.push((min_x, min_y));
    }
    corners.sort_by(|left, right| left.partial_cmp(right).expect("finite"));
    corners.dedup();
    assert_eq!(
        corners.len(),
        4,
        "two parts were placed on top of each other"
    );
}

#[test]
fn a_plate_that_cannot_fit_names_the_part_that_does_not() {
    let mut plate = plate();
    plate.add("huge", cube()).expect("a cube adds");
    let refused = plate.arrange((10.0, 10.0), 5.0);
    assert!(matches!(
        refused,
        Err(PlateError::DoesNotFit { ref object, .. }) if object == "huge"
    ));
}

#[test]
fn setting_a_key_on_one_object_records_it_as_an_override() {
    let mut plate = plate();
    let id = plate.add("bracket", cube()).expect("a cube adds");
    plate
        .set_object_setting(id, "walls", "3")
        .expect("the key applies");
    let object = plate.object(id).expect("the object is there");
    let keys: Vec<&str> = object.overrides().collect();
    assert_eq!(keys, vec!["walls"]);
    plate.clear_object_settings(id).expect("the settings clear");
    let object = plate.object(id).expect("the object is there");
    assert_eq!(object.overrides().count(), 0);
    assert!(object.settings().is_none());
}

/// Settings a profile states when it knows the toolhead.
fn clearanced() -> Settings {
    Settings {
        gantry_height: 45.0,
        extruder_clearance_radius: 20.0,
        ..Settings::default()
    }
}

fn placed(translation_x: f64) -> Mesh {
    let mesh = cube();
    mesh.transformed(MeshTransform {
        translation_mm: [translation_x, 0.0, 0.0],
        ..MeshTransform::default()
    })
    .expect("a cube moves")
}

#[test]
fn an_unmeasured_toolhead_is_refused_rather_than_guessed() {
    let mut plate = Plate::new(Settings::default());
    plate.add("first", cube()).expect("a cube adds");
    plate.add("second", placed(60.0)).expect("a cube adds");
    let refused = plate
        .slice(0.2, 0.2, PlateOrder::ByObject)
        .err()
        .expect("an unmeasured gantry is refused");
    assert!(
        refused.to_string().contains("gantry_height"),
        "the refusal names the missing setting: {refused}"
    );

    let half = Settings {
        extruder_clearance_radius: 0.0,
        ..clearanced()
    };
    let mut plate = Plate::new(half);
    plate.add("first", cube()).expect("a cube adds");
    plate.add("second", placed(60.0)).expect("a cube adds");
    let refused = plate
        .slice(0.2, 0.2, PlateOrder::ByObject)
        .err()
        .expect("an unmeasured head is refused");
    assert!(
        refused.to_string().contains("extruder_clearance_radius"),
        "the refusal names the missing setting: {refused}"
    );
}

#[test]
fn two_objects_within_the_toolheads_radius_are_refused() {
    let mut plate = Plate::new(clearanced());
    plate.add("first", cube()).expect("a cube adds");
    plate.add("second", placed(25.0)).expect("a cube adds");
    let refused = plate
        .slice(0.2, 0.2, PlateOrder::ByObject)
        .err()
        .expect("a 5 mm gap is inside a 20 mm radius");
    assert!(
        refused.to_string().contains("first and second"),
        "the refusal names both objects: {refused}"
    );
    plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the same plate prints layer by layer, where nothing passes between the parts");
}

#[test]
fn a_painted_blocker_removes_the_support_a_bare_slice_builds() {
    let mut settings = Settings::default();
    settings.support = Some(hfw_engine::SupportSettings::new(settings.wall.w));
    let mut plate = Plate::new(settings);
    let id = plate.add("table", table()).expect("the table adds");
    let bare = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the table slices");
    let supports = |job: &hfw_engine::Job| -> usize {
        job.layers
            .iter()
            .flat_map(|layer| layer.paths.iter())
            .filter(|line| line.kind == hfw_engine::PathKind::Support)
            .count()
    };
    assert!(supports(&bare) > 0, "the table's overhang needs support");

    let mesh = plate.object(id).expect("added").mesh();
    let triangles = mesh.triangle_count();
    let states: Vec<u8> = (0..triangles)
        .map(|index| {
            let low = mesh
                .triangle_vertices(index)
                .iter()
                .map(|point| point.to_mm()[2])
                .fold(f64::INFINITY, f64::min);
            if low >= 20.0 {
                2
            } else {
                0
            }
        })
        .collect();
    plate
        .object_mut(id)
        .expect("added")
        .set_paint(hfw_engine::Paint {
            support: states,
            seam: Vec::new(),
        })
        .expect("the states match the triangles");
    let blocked = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the painted table slices");
    assert_eq!(
        supports(&blocked),
        0,
        "every support line under a painted blocker is gone"
    );
}

#[test]
fn paint_survives_a_project_round_trip() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    let triangles = plate.object(id).expect("added").mesh().triangle_count();
    let mut states = vec![0u8; triangles];
    states[0] = 1;
    states[triangles - 1] = 2;
    let mut seam = vec![0u8; triangles];
    seam[1] = 2;
    plate
        .object_mut(id)
        .expect("added")
        .set_paint(hfw_engine::Paint {
            support: states,
            seam,
        })
        .expect("the states match the triangles");

    let bytes = plate.to_project().expect("the plate writes");
    let reopened = Plate::from_project(&bytes, Settings::default()).expect("the plate opens");
    let object = reopened
        .objects()
        .first()
        .expect("the project holds the object");
    let support = &object.paint().support;
    assert_eq!(
        support.len(),
        triangles,
        "one state per triangle comes back"
    );
    assert_eq!(support[0], 1, "the enforcer on the first triangle survives");
    assert_eq!(
        support[triangles - 1],
        2,
        "the blocker on the last triangle survives"
    );
    assert_eq!(support[1], 0, "and the untouched triangles are untouched");
    assert_eq!(
        object.paint().seam[1],
        2,
        "the seam channel survives the round trip too"
    );
}

#[test]
fn a_paint_channel_of_the_wrong_length_is_refused() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    let refused = plate
        .object_mut(id)
        .expect("added")
        .set_paint(hfw_engine::Paint {
            support: vec![0u8; 3],
            seam: Vec::new(),
        })
        .expect_err("three states cannot describe a cube");
    assert!(
        refused.to_string().contains("3 states"),
        "the refusal names the mismatch: {refused}"
    );
}

#[test]
fn two_objects_clear_of_the_toolheads_radius_are_accepted() {
    let mut plate = Plate::new(clearanced());
    plate.add("first", cube()).expect("a cube adds");
    plate.add("second", placed(45.0)).expect("a cube adds");
    plate
        .slice(0.2, 0.2, PlateOrder::ByObject)
        .expect("a 25 mm gap clears a 20 mm radius");
}

#[test]
fn an_object_taller_than_the_gantry_clearance_is_refused() {
    let settings = Settings {
        gantry_height: 10.0,
        ..clearanced()
    };
    let mut plate = Plate::new(settings);
    plate.add("tall", cube()).expect("a cube adds");
    let refused = plate
        .slice(0.2, 0.2, PlateOrder::ByObject)
        .err()
        .expect("a 20 mm object cannot pass under a 10 mm gantry");
    assert!(
        refused.to_string().contains("tall is 20.0 mm"),
        "the refusal gives the height and the clearance: {refused}"
    );
}

/// The volume of the object's paths, as a proxy for "there is material here".
fn path_count(job: &hfw_engine::Job) -> usize {
    job.layers
        .iter()
        .map(|layer| layer.paths.len())
        .sum::<usize>()
}

#[test]
fn a_negative_volume_cuts_a_hole_that_gets_walls_of_its_own() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    let bare = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the cube slices");
    let cutter = Mesh::box_mm(6.0, 6.0, 40.0)
        .expect("a box builds")
        .transformed(MeshTransform {
            translation_mm: [7.0, 7.0, 0.0],
            ..MeshTransform::default()
        })
        .expect("the cutter moves into the cube's middle");
    plate
        .object_mut(id)
        .expect("added")
        .add_volume(hfw_engine::VolumeKind::Negative, cutter)
        .expect("the volume attaches");
    let cut = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the cut cube slices");
    assert!(
        path_count(&cut) > path_count(&bare),
        "a hole adds a wall around itself: {} paths against {}",
        path_count(&cut),
        path_count(&bare)
    );
    let inner = cut
        .layers
        .iter()
        .flat_map(|layer| layer.paths.iter())
        .any(|line| matches!(line.kind, hfw_engine::PathKind::Side(index) if index > 0));
    assert!(inner, "the hole's wall is planned as a wall, not as infill");
}

#[test]
fn a_part_volume_adds_where_the_object_has_nothing() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    let bare = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the cube slices");
    let addition = Mesh::box_mm(5.0, 5.0, 30.0)
        .expect("a box builds")
        .transformed(MeshTransform {
            translation_mm: [25.0, 5.0, 0.0],
            ..MeshTransform::default()
        })
        .expect("the box moves onto the bed beside the cube");
    plate
        .object_mut(id)
        .expect("added")
        .add_volume(hfw_engine::VolumeKind::Part, addition)
        .expect("the volume attaches");
    let grown = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the grown cube slices");
    assert!(
        path_count(&grown) > path_count(&bare),
        "the added body prints: {} paths against {}",
        path_count(&grown),
        path_count(&bare)
    );
}

#[test]
fn a_support_blocker_volume_removes_the_support_a_bare_slice_builds() {
    let mut settings = Settings::default();
    settings.support = Some(hfw_engine::SupportSettings::new(settings.wall.w));
    let mut plate = Plate::new(settings);
    let id = plate.add("table", table()).expect("the table adds");
    let supports = |job: &hfw_engine::Job| -> usize {
        job.layers
            .iter()
            .flat_map(|layer| layer.paths.iter())
            .filter(|line| line.kind == hfw_engine::PathKind::Support)
            .count()
    };
    let bare = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the table slices");
    assert!(supports(&bare) > 0, "the table's overhang needs support");
    let blocker = Mesh::box_mm(40.0, 40.0, 20.0).expect("a box builds");
    plate
        .object_mut(id)
        .expect("added")
        .add_volume(hfw_engine::VolumeKind::SupportBlocker, blocker)
        .expect("the volume attaches");
    let blocked = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the blocked table slices");
    assert_eq!(
        supports(&blocked),
        0,
        "a blocker under the whole table leaves nowhere for support to stand"
    );
}

#[test]
fn volumes_survive_a_project_round_trip() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    let cutter = Mesh::box_mm(6.0, 6.0, 40.0)
        .expect("a box builds")
        .transformed(MeshTransform {
            translation_mm: [7.0, 7.0, 0.0],
            ..MeshTransform::default()
        })
        .expect("the cutter moves into the cube's middle");
    plate
        .object_mut(id)
        .expect("added")
        .add_volume(hfw_engine::VolumeKind::Negative, cutter)
        .expect("the volume attaches");
    let bytes = plate.to_project().expect("the project writes");
    let reopened = Plate::from_project(&bytes, Settings::default()).expect("the project reads");
    let object = reopened.object(ObjectId::from_number(1).expect("one is an id"));
    let object = object.expect("the cube is back");
    assert_eq!(object.volumes().len(), 1, "the volume came back");
    assert_eq!(object.volumes()[0].kind(), hfw_engine::VolumeKind::Negative);
    assert_eq!(
        object.volumes()[0].mesh().triangle_count(),
        12,
        "with its geometry"
    );
}

#[test]
fn one_object_steps_differently_and_the_plate_still_merges_on_z() {
    let mut plate = plate();
    let fine = plate.add("fine", cube()).expect("a cube adds");
    plate.add("coarse", cube()).expect("a cube adds");
    plate
        .object_mut(fine)
        .expect("added")
        .set_layer_heights(vec![(0.0, 0.12)])
        .expect("0.12 mm is a usable step");

    let job = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the plate slices");
    let mut last = 0.0;
    for layer in &job.layers {
        let z = layer.print_z;
        assert!(
            z >= last,
            "a by-layer merge runs upwards: {z} mm came after {last} mm"
        );
        last = z;
    }
    let fine_layers = job
        .layers
        .iter()
        .filter(|layer| layer.paths.iter().all(|line| line.object == 0))
        .count();
    let coarse_layers = job
        .layers
        .iter()
        .filter(|layer| layer.paths.iter().all(|line| line.object == 1))
        .count();
    assert!(
        fine_layers > coarse_layers,
        "the 0.12 mm object takes more layers than the 0.2 mm one: {fine_layers} against \
         {coarse_layers}"
    );
}

#[test]
fn a_layer_height_profile_that_does_not_rise_is_refused_before_it_slices() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    let object = plate.object_mut(id).expect("added");
    let backwards = object
        .set_layer_heights(vec![(8.0, 0.2), (2.0, 0.12)])
        .expect_err("a descending z is refused");
    assert!(
        backwards.to_string().contains("does not come after"),
        "the refusal says what is wrong: {backwards}"
    );
    let negative = object
        .set_layer_heights(vec![(0.0, -0.2)])
        .expect_err("a negative height is refused");
    assert!(
        negative.to_string().contains("positive finite"),
        "the refusal says what is wrong: {negative}"
    );
    object
        .set_layer_heights(vec![(0.0, 0.2), (4.0, 0.12)])
        .expect("a rising profile is accepted");
    assert_eq!(object.layer_heights().len(), 2);
}

#[test]
fn layer_heights_survive_a_project_round_trip() {
    let mut plate = plate();
    let id = plate.add("cube", cube()).expect("a cube adds");
    plate
        .object_mut(id)
        .expect("added")
        .set_layer_heights(vec![(0.0, 0.2), (4.0, 0.12)])
        .expect("the profile is accepted");
    let bytes = plate.to_project().expect("the project writes");
    let reopened = Plate::from_project(&bytes, Settings::default()).expect("the project reads");
    let object = reopened
        .object(ObjectId::from_number(1).expect("one is an id"))
        .expect("the cube is back");
    assert_eq!(
        object.layer_heights(),
        [(0.0, 0.2), (4.0, 0.12)],
        "the object kept its own step heights"
    );
}

#[test]
fn the_plate_carries_its_timeline_into_the_job_and_through_a_project() {
    let mut plate = plate();
    plate.add("cube", cube()).expect("a cube adds");
    plate
        .set_markers(vec![
            hfw_engine::Marker {
                z: 4.0,
                kind: hfw_engine::MarkerKind::Custom,
                text: "M117 check the seam".to_owned(),
            },
            hfw_engine::Marker {
                z: 1.0,
                kind: hfw_engine::MarkerKind::Pause,
                text: String::new(),
            },
        ])
        .expect("the timeline is accepted");
    assert_eq!(
        plate.markers()[0].z,
        1.0,
        "the timeline sorts on the way in"
    );

    let job = plate
        .slice(0.2, 0.2, PlateOrder::ByLayer)
        .expect("the plate slices");
    assert_eq!(
        job.markers,
        plate.markers(),
        "the job carries them in order"
    );

    let bytes = plate.to_project().expect("the project writes");
    let reopened = Plate::from_project(&bytes, Settings::default()).expect("the project reads");
    assert_eq!(
        reopened.markers(),
        plate.markers(),
        "the timeline survives the round trip"
    );
}

#[test]
fn a_marker_with_no_g_code_is_refused_by_the_plate() {
    let mut plate = plate();
    let refused = plate
        .set_markers(vec![hfw_engine::Marker {
            z: 2.0,
            kind: hfw_engine::MarkerKind::Custom,
            text: "  ".to_owned(),
        }])
        .expect_err("a custom marker with no G-code is refused");
    assert!(
        refused.to_string().contains("carries no G-code"),
        "{refused}"
    );
    assert!(
        plate.markers().is_empty(),
        "the refused list changed nothing"
    );
}
