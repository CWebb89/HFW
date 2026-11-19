//! How the skeletal wall generator compares with the offset ladder it replaces.
//!
//! The wall stage prints a band around a layer's boundary: three 0.4 mm beads of
//! wall, with the middle of a thick section left to the infill. Both generators are
//! asked for the same band on the same shape, and the comparison is the material
//! they lay down — the area a bead covers, `width × length` summed.
//!
//! Measured on this tree:
//!
//! | shape | ladder | skeletal | ratio |
//! |---|---|---|---|
//! | 30 × 20 rectangle | 114.2 mm² | 102.7 mm² | 0.90 |
//! | 40 × 1 thin wall | 39.9 mm² | 39.0 mm² | 0.98 |
//! | radius 15 ring, 1 mm wall | 72.9 mm² | 448.0 mm² | 6.15 |
//! | radius 10 disc | 70.9 mm² | 1809.8 mm² | 25.5 |
//!
//! Straight-sided and thin shapes agree within 10%. Round ones overrun, and the
//! reason is structural rather than a bug: a bead count cannot change along an edge
//! yet, so where the medial arms of a disc converge the same wall is printed many
//! times. Closing that is what the transition stage is for, and these tests are the
//! ratchet it has to move.

use hfw_kernel::{Region, V2};
use hfw_skeleton::classify::{self, Params as ClassifyParams};
use hfw_skeleton::emit::{self, Params as EmitParams};
use hfw_walls::{generate, WallMode, WallParams};

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

/// Area the offset ladder lays down, from its own per-vertex widths.
fn ladder_area(region: &Region) -> f64 {
    let out = generate(region, &WallParams::new(0.4, 3), WallMode::Hfw);
    out.lines
        .iter()
        .map(|line| {
            let n = line.pts.len();
            let mut a = 0.0;
            for i in 1..n {
                a += line.pts[i - 1].dist(line.pts[i]) * 0.5 * (line.width[i - 1] + line.width[i]);
            }
            if line.closed && n > 2 {
                a += line.pts[n - 1].dist(line.pts[0]) * 0.5 * (line.width[n - 1] + line.width[0]);
            }
            a
        })
        .sum()
}

/// Area the skeletal generator lays down.
fn skeletal_area(region: &Region) -> f64 {
    let mut graph = hfw_skeleton::build(region, &hfw_skeleton::Params::default())
        .expect("the skeleton builder must not fail");
    classify::classify(&mut graph, &ClassifyParams::default());
    emit::emit(&graph, region, &EmitParams::default()).area
}

fn compare(name: &str, rings: Vec<Vec<V2>>, low: f64, high: f64) {
    let region = Region::new(rings);
    let ladder = ladder_area(&region);
    let skeletal = skeletal_area(&region);
    let ratio = skeletal / ladder;
    assert!(
        ratio > low && ratio < high,
        "{name}: the skeletal walls lay down {skeletal:.1} mm² against the ladder's \
         {ladder:.1} mm², a ratio of {ratio:.2} outside {low}..{high}"
    );
}

#[test]
fn a_rectangle_walls_match_the_ladder() {
    compare(
        "30x20 rectangle",
        vec![rect(0.0, 0.0, 30.0, 20.0)],
        0.7,
        1.2,
    );
}

#[test]
fn a_thin_wall_matches_the_ladder() {
    compare("40x1 wall", vec![rect(0.0, 0.0, 40.0, 1.0)], 0.8, 1.2);
}

#[test]
fn a_ring_part_walls_overrun_the_ladder_while_a_count_cannot_change() {
    let rings = vec![circle(0.0, 0.0, 15.0, 96), circle(0.0, 0.0, 14.0, 96)];
    compare("radius 15 ring", rings, 1.0, 9.0);
}

#[test]
fn a_disc_walls_overrun_the_ladder_while_a_count_cannot_change() {
    compare(
        "radius 10 disc",
        vec![circle(0.0, 0.0, 10.0, 96)],
        1.0,
        30.0,
    );
}
