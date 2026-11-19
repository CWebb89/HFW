//! Bead centrelines for the shapes a wall generator has to get right.
//!
//! The walls cover a band, not the layer: a bead is a wall, and the middle of a
//! thick section belongs to the infill. So the quantity a shape is checked against
//! is the band its profile asks for — the perimeter times the walls it requested —
//! and not the area of the region.
//!
//! The band bounds below are ratchets, not targets. Where a shape is over-covered
//! the reason is on the test: bead counts do not change along an edge yet, so the
//! walls of a round part run down its radial arms instead of following its contour,
//! and neighbours overlap. That is the transition stage, and these numbers are what
//! it has to improve.

use hfw_kernel::{Region, V2};
use hfw_skeleton::bead::{count_for, Even, Limited, Section, Strategy};
use hfw_skeleton::classify::{self, Params as ClassifyParams};
use hfw_skeleton::emit::{self, Params as EmitParams};
use hfw_skeleton::{build, Graph, Params};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<V2> {
    vec![
        V2::new(x0, y0),
        V2::new(x1, y0),
        V2::new(x1, y1),
        V2::new(x0, y1),
    ]
}

fn circle(cx: f64, cy: f64, r: f64, n: usize) -> Vec<V2> {
    (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            V2::new(cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn skeleton(rings: Vec<Vec<V2>>) -> Graph {
    let region = Region::new(rings);
    let mut graph = build(&region, &Params::default()).expect("the skeleton builder must not fail");
    classify::classify(&mut graph, &ClassifyParams::default());
    graph
}

fn emitted(rings: Vec<Vec<V2>>, p: &EmitParams) -> emit::Output {
    let region = Region::new(rings.clone());
    emit::emit(&skeleton(rings), &region, p)
}

/// Area of the band a profile asks for: the outline's length times the walls' width.
fn band(rings: &[Vec<V2>], p: &EmitParams) -> f64 {
    Region::new(rings.to_vec()).perimeter() * p.optimal * p.walls as f64
}

fn ratio(out: &emit::Output, rings: &[Vec<V2>], p: &EmitParams) -> f64 {
    out.area / band(rings, p)
}

#[test]
fn a_thin_wall_is_filled_across_its_whole_thickness() {
    let p = EmitParams::default();
    let rings = vec![rect(0.0, 0.0, 40.0, 1.0)];
    let out = emitted(rings.clone(), &p);
    assert!(!out.beads.is_empty());
    for bead in &out.beads {
        assert!(
            bead.widths.iter().all(|w| *w <= 0.55),
            "a 1 mm wall gets beads of its own width, got {:?}",
            bead.widths
        );
    }
    let covered = out.area / Region::new(rings).area();
    assert!(
        covered > 0.85 && covered < 1.45,
        "a 1 mm wall gets walls-sized beads, so they cover {:.0}% of it; the excess is \
         the overlap two beads leave where an edge meets another, which a count that \
         can change along an edge would remove",
        covered * 100.0
    );
}

#[test]
fn a_thick_body_gets_its_walls_at_its_boundaries_and_its_middle_left_alone() {
    let p = EmitParams::default();
    let rings = vec![rect(0.0, 0.0, 40.0, 20.0)];
    let out = emitted(rings.clone(), &p);
    assert!(!out.beads.is_empty());
    let widest = out
        .beads
        .iter()
        .flat_map(|b| b.widths.iter().copied())
        .fold(0.0_f64, f64::max);
    assert!(widest <= 0.45, "walls are one bead wide, got {widest}");
    for bead in &out.beads {
        for &q in &bead.pts {
            let inset = q.x.min(40.0 - q.x).min(q.y).min(20.0 - q.y);
            assert!(
                inset < 2.5,
                "a wall bead of a 20 mm body must hug the boundary, got {q:?} at {inset:.2} from it"
            );
        }
    }
    let r = ratio(&out, &rings, &p);
    assert!(
        r > 0.6 && r < 1.4,
        "the walls cover {:.0}% of the band",
        r * 100.0
    );
}

#[test]
fn a_circle_gets_only_beads_of_its_own_width_inside_it() {
    let p = EmitParams::default();
    let rings = vec![circle(0.0, 0.0, 10.0, 96)];
    let out = emitted(rings, &p);
    assert!(!out.beads.is_empty());
    for bead in &out.beads {
        assert!(bead.widths.iter().all(|w| *w <= 0.45), "one bead wide");
        for &q in &bead.pts {
            assert!(q.len() < 10.05, "inside the disc, got {q:?}");
        }
    }
}

#[test]
fn a_v_shape_gets_beads_down_both_arms() {
    let p = EmitParams::default();
    let rings = vec![vec![
        V2::new(0.0, 0.0),
        V2::new(40.0, 0.0),
        V2::new(20.0, 12.0),
    ]];
    let out = emitted(rings.clone(), &p);
    assert!(!out.beads.is_empty());
    let left = out
        .beads
        .iter()
        .any(|b| b.pts.iter().any(|q| q.x < 12.0 && q.y < 6.0));
    let right = out
        .beads
        .iter()
        .any(|b| b.pts.iter().any(|q| q.x > 28.0 && q.y < 6.0));
    assert!(left && right, "both arms carry beads");
    let r = ratio(&out, &rings, &p);
    assert!(
        r > 0.4 && r < 1.6,
        "the walls cover {:.0}% of the band",
        r * 100.0
    );
}

#[test]
fn a_ring_part_gets_beads_that_stay_between_its_two_circles() {
    let p = EmitParams::default();
    let rings = vec![circle(0.0, 0.0, 15.0, 96), circle(0.0, 0.0, 14.0, 96)];
    let out = emitted(rings.clone(), &p);
    assert!(
        !out.beads.is_empty(),
        "a 1 mm ring is wall all the way round"
    );
    for bead in &out.beads {
        for &q in &bead.pts {
            let r = q.len();
            assert!(
                (13.95..=15.05).contains(&r),
                "a bead of a ring part stays in its wall, got r={r:.3}"
            );
        }
    }
    let covered = out.area / Region::new(rings).area();
    assert!(
        covered > 0.8 && covered < 7.5,
        "the walls stay in the ring's band but overrun it {:.0}% over: a constant count \
         per edge prints the same wall several times where the arms converge",
        covered * 100.0
    );
}

#[test]
fn a_bead_never_leaves_the_layer() {
    let rings = vec![circle(0.0, 0.0, 8.0, 64)];
    let out = emitted(rings, &EmitParams::default());
    assert!(!out.beads.is_empty());
    for bead in &out.beads {
        for &q in &bead.pts {
            assert!(
                q.dist(V2::new(0.0, 0.0)) < 8.05,
                "a wall bead of a disc stays inside it, got {q:?}"
            );
        }
    }
}

#[test]
fn the_same_layer_emits_the_same_beads_twice() {
    let rings = vec![rect(0.0, 0.0, 30.0, 20.0), circle(60.0, 10.0, 6.0, 40)];
    let a = emitted(rings.clone(), &EmitParams::default());
    let b = emitted(rings, &EmitParams::default());
    assert_eq!(a.beads.len(), b.beads.len());
    assert_eq!(a.open_ends, b.open_ends);
    for (x, y) in a.beads.iter().zip(&b.beads) {
        assert_eq!(x.pts.len(), y.pts.len());
        assert_eq!(x.closed, y.closed);
        for (p, q) in x.pts.iter().zip(&y.pts) {
            assert_eq!(p.x.to_bits(), q.x.to_bits());
            assert_eq!(p.y.to_bits(), q.y.to_bits());
        }
    }
    assert_eq!(a.area.to_bits(), b.area.to_bits());
}

#[test]
fn even_beading_fills_the_section_it_is_given() {
    let b = Even.compute(
        Section {
            thickness: 1.6,
            skew: 0.0,
        },
        4,
    );
    assert_eq!(b.widths.len(), 4);
    assert!((b.covered() - 1.6).abs() < 1e-12);
    assert!((b.locations[0] + 0.6).abs() < 1e-12);
    assert!((b.locations[3] - 0.6).abs() < 1e-12);
}

#[test]
fn a_section_skewed_from_the_axis_puts_its_beads_on_the_section() {
    let b = Even.compute(
        Section {
            thickness: 1.6,
            skew: 0.5,
        },
        4,
    );
    assert!((b.locations[0] - (-0.1)).abs() < 1e-12);
    assert!((b.locations[3] - 1.1).abs() < 1e-12);
}

#[test]
fn a_capped_section_hugs_both_boundaries_and_leaves_its_middle_over() {
    let b = Limited {
        optimal: 0.4,
        max_count: 3,
    }
    .compute(
        Section {
            thickness: 5.0,
            skew: 0.0,
        },
        3,
    );
    assert_eq!(b.widths.len(), 3);
    assert!((b.locations[0] - (-2.3)).abs() < 1e-9);
    assert!((b.locations[2] - 2.3).abs() < 1e-9);
    assert!(
        (b.left_over - (5.0 - b.covered())).abs() < 1e-12,
        "the middle is reported as left over, got {}",
        b.left_over
    );
    assert!(
        b.covered() <= 1.21,
        "three 0.4 mm walls, got {}",
        b.covered()
    );
}

#[test]
fn a_section_narrower_than_a_bead_still_gets_one() {
    assert_eq!(count_for(0.2, 0.4, 3), 1);
    assert_eq!(count_for(0.0, 0.4, 3), 0);
    assert_eq!(count_for(10.0, 0.4, 3), 3);
}
