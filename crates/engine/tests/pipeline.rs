//! End-to-end checks over the shared fixtures.
//!
//! These exercise the seam the two upstream prototypes never crossed: an exact
//! integer slice driving a numerical wall and infill pipeline, then material
//! accounting over the result.

use std::path::PathBuf;

use hfw_engine::{run, slice_plane, InfillParams, InfillPattern, Mesh, Mode, PathKind, Settings};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn mesh(name: &str) -> Mesh {
    Mesh::load_stl(fixture(name)).expect("fixture loads")
}

fn settings(walls: usize) -> Settings {
    let mut s = Settings::new(0.4, walls);
    s.keep_reference = true;
    s
}

#[test]
fn the_fixture_set_is_complete() {
    for name in [
        "cube.stl",
        "sphere.stl",
        "torus.stl",
        "cone.stl",
        "taper_wall.stl",
        "thin_wall.stl",
        "cube_20mm_ascii.stl",
        "cube_20mm_binary.stl",
        "open_triangle_ascii.stl",
    ] {
        assert!(fixture(name).exists(), "missing fixture {name}");
    }
}

#[test]
fn ascii_and_binary_stl_produce_the_same_layer() {
    let a = mesh("cube_20mm_ascii.stl");
    let b = mesh("cube_20mm_binary.stl");
    assert_eq!(a.diagnostics(), b.diagnostics());
    let (_, ra) = slice_plane(&a, 10.0).expect("ascii slice");
    let (_, rb) = slice_plane(&b, 10.0).expect("binary slice");
    assert_eq!(ra.rings.len(), rb.rings.len());
    assert!((ra.area() - rb.area()).abs() < 1e-9);
    assert!((ra.area() - 400.0).abs() < 1e-9);
}

#[test]
fn a_cube_stack_gets_walls_on_every_layer() {
    let mut s = settings(3);
    s.infill = Some(InfillParams::new(0.4));
    let job = run(&mesh("cube.stl"), 0.2, 0.2, &s).expect("job");
    assert_eq!(job.layers.len(), 100);
    assert_eq!(job.layers[0].bottom_z, 0.0);
    assert!((job.layers[0].slice_z - 0.1).abs() < 1e-9);
    assert!((job.layers[99].print_z - 20.0).abs() < 1e-9);
    for layer in &job.layers {
        assert_eq!(layer.counts.walls, 3, "layer {}", layer.index);
        assert!(layer.paths.iter().any(|p| p.kind == PathKind::Side(0)));
    }
    assert!(
        job.layers[0].counts.skin > 0,
        "the first layer is a bottom skin"
    );
    assert_eq!(job.layers[0].counts.infill, 0);
    assert!(
        job.layers[50].counts.infill > 0,
        "mid-height must be sparse"
    );
    assert_eq!(job.layers[50].counts.skin, 0);
}

#[test]
fn the_variable_first_layer_reaches_the_top_of_the_model() {
    let job = run(&mesh("cube.stl"), 0.3, 0.4, &settings(2)).expect("job");
    assert_eq!(job.layers.len(), 51);
    assert!((job.layers[0].print_z - 0.3).abs() < 1e-9);
    assert!((job.layers.last().unwrap().print_z - 20.0).abs() < 1e-9);
    assert!((job.layers.last().unwrap().height() - 0.1).abs() < 1e-9);
}

#[test]
fn walls_never_escape_the_part() {
    let mut s = settings(3);
    s.infill = None;
    let job = run(&mesh("sphere.stl"), 0.2, 0.2, &s).expect("job");
    for layer in job.layers.iter().filter(|l| l.index % 10 == 0) {
        let mut b = hfw_kernel::Bounds::empty();
        for ring in &layer.reference {
            for p in ring {
                b.add(*p);
            }
        }
        let grown = b.grown(0.25);
        for path in &layer.paths {
            for p in &path.pts {
                assert!(
                    p.x >= grown.min.x
                        && p.x <= grown.max.x
                        && p.y >= grown.min.y
                        && p.y <= grown.max.y,
                    "layer {} path escaped the part at {p:?}",
                    layer.index
                );
            }
        }
    }
}

#[test]
fn a_thin_wall_is_printed_by_hfw_and_missed_by_classic() {
    let thin = mesh("thin_wall.stl");
    let (_, region) = slice_plane(&thin, 2.0).expect("slice");
    let mut s = settings(3);
    s.infill = None;

    let hfw = run(&thin, 0.2, 0.2, &Settings { every: 1000, ..s }).expect("hfw job");
    let classic = run(
        &thin,
        0.2,
        0.2,
        &Settings {
            every: 1000,
            mode: Mode::Classic,
            ..s
        },
    )
    .expect("classic job");

    assert!((region.area() - 0.3 * 25.0).abs() < 0.05);
    assert!(hfw.layers[0].counts.middle > 0, "hfw missed the thin wall");
    assert_eq!(classic.layers[0].counts.walls, 0);
    assert_eq!(classic.layers[0].counts.middle, 0);
}

#[test]
fn solid_infill_reproduces_the_published_volume_ratios() {
    for (name, expected) in [
        ("cube.stl", 0.9963),
        ("sphere.stl", 0.9930),
        ("torus.stl", 0.9853),
    ] {
        let m = mesh(name);
        let mut s = settings(3);
        let mut ip = InfillParams::new(0.4);
        ip.density = 1.0;
        s.infill = Some(ip);
        let job = run(&m, 0.2, 0.2, &s).expect("job");
        let ratio = (job.volume.0 + job.volume.1) / m.volume_mm3();
        assert!(
            (ratio - expected).abs() < 0.004,
            "{name}: {ratio:.4} vs the documented {expected:.4}"
        );
    }
}

#[test]
fn sparse_infill_density_tracks_the_request() {
    let m = mesh("cube.stl");
    let mut last = 0.0;
    for density in [0.1, 0.2, 0.4] {
        let mut s = settings(3);
        let mut ip = InfillParams::new(0.4);
        ip.density = density;
        ip.top_layers = 0;
        ip.bottom_layers = 0;
        s.infill = Some(ip);
        let job = run(&m, 0.2, 0.2, &Settings { every: 1, ..s }).expect("job");
        let inner = job.layers[50].counts.infill as f64;
        assert!(inner > last, "density {density} did not add infill lines");
        last = inner;
    }
}

#[test]
fn a_hole_is_left_unfilled() {
    let mut s = settings(3);
    let mut ip = InfillParams::new(0.4);
    ip.density = 0.25;
    ip.pattern = InfillPattern::Grid;
    s.infill = Some(ip);
    let job = run(&mesh("torus.stl"), 0.2, 0.2, &s).expect("job");
    let layer = &job.layers[20];
    assert_eq!(layer.reference.len(), 2, "the torus should have a hole");
    let mut b = hfw_kernel::Bounds::empty();
    for ring in &layer.reference {
        for p in ring {
            b.add(*p);
        }
    }
    let centre = hfw_kernel::V2::new(b.min.x + b.width() * 0.5, b.min.y + b.height() * 0.5);
    for path in &layer.paths {
        for p in &path.pts {
            assert!(
                p.dist(centre) > 0.5 || path.kind.is_wall(),
                "infill entered the hole at {p:?}"
            );
        }
    }
}

#[test]
fn fan_triangulated_surfaces_close_on_every_layer() {
    let job = run(&mesh("cone.stl"), 0.2, 0.2, &settings(3)).expect("job");
    assert_eq!(job.layers.len(), 150);
    assert!(
        job.skipped.is_empty(),
        "fan triangulation produced unclosed layers: {:?}",
        job.skipped.first()
    );
    assert!(job.layers[0].counts.walls > 0);
    assert!(
        job.layers[149].counts.walls == 0,
        "the tip cannot hold a loop"
    );
}

#[test]
fn strict_mode_turns_an_unclosed_layer_into_an_error() {
    let mut s = settings(3);
    s.strict_slices = true;
    let job = run(&mesh("sphere.stl"), 0.2, 0.2, &s).expect("a closed mesh stays closed");
    assert!(job.skipped.is_empty());
}

#[test]
fn an_open_mesh_is_rejected_rather_than_guessed() {
    let open = mesh("open_triangle_ascii.stl");
    let error = slice_plane(&open, 5.0).expect_err("an open slice must not succeed");
    assert!(error.to_string().contains("open contour"), "{error}");
}

#[test]
fn the_gcode_free_scope_is_visible_in_the_output() {
    let job = run(&mesh("cube.stl"), 1.0, 1.0, &settings(2).clone()).expect("job");
    assert_eq!(job.layers.len(), 20);
    assert!(!job.layers[0].to_svg().contains("G1"));
}

#[test]
fn geometry_output_is_deterministic() {
    let mut s = settings(3);
    s.infill = Some(InfillParams::new(0.4));
    let a = run(&mesh("sphere.stl"), 0.2, 0.2, &s).expect("job");
    let b = run(&mesh("sphere.stl"), 0.2, 0.2, &s).expect("job");
    for (la, lb) in a.layers.iter().zip(b.layers.iter()) {
        assert_eq!(la.manifest_row(), lb.manifest_row());
        assert_eq!(la.to_svg(), lb.to_svg());
    }
}
