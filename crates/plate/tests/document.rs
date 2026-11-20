//! The document model, exercised the way a front end would use it.

use hfw_engine::{Mesh, MeshTransform, PlateOrder, Settings};
use hfw_plate::{Plate, PlateError};

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
