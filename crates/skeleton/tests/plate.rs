//! The skeleton over real layer outlines, not shapes drawn for a test.
//!
//! The rings are the same plate the offset kernel is checked against, saved as
//! coordinates so this does not depend on a model file or on the slicing crate.

use hfw_kernel::{Region, RegionIndex, SegIndex, V2};
use hfw_skeleton::classify::{self, Params as ClassifyParams};
use hfw_skeleton::emit::{self, Params as EmitParams};
use hfw_skeleton::{build, prepare, Params};

fn plate_rings() -> Vec<(f64, Vec<Vec<V2>>)> {
    let text = include_str!("../../kernel/tests/plate-rings.txt");
    let mut out = Vec::new();
    let mut z = 0.0;
    let mut rings: Vec<Vec<V2>> = Vec::new();
    let mut ring: Vec<V2> = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("#") => {
                if !ring.is_empty() {
                    rings.push(std::mem::take(&mut ring));
                }
                if !rings.is_empty() {
                    out.push((z, std::mem::take(&mut rings)));
                }
                z = words.next().and_then(|w| w.parse().ok()).unwrap_or(0.0);
            }
            Some("") | None => {
                if !ring.is_empty() {
                    rings.push(std::mem::take(&mut ring));
                }
            }
            Some(_) => {
                let mut numbers = line
                    .split_whitespace()
                    .filter_map(|w| w.parse::<f64>().ok());
                if let (Some(x), Some(y)) = (numbers.next(), numbers.next()) {
                    ring.push(V2::new(x, y));
                }
            }
        }
    }
    if !ring.is_empty() {
        rings.push(ring);
    }
    if !rings.is_empty() {
        out.push((z, rings));
    }
    out
}

fn distance_to_segment(p: V2, a: V2, b: V2) -> f64 {
    let ab = b - a;
    let len2 = ab.len2();
    if len2 < 1e-18 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.dist(a + ab * t)
}

#[test]
fn prepare_does_not_move_the_boundary() {
    let layers = plate_rings();
    let params = Params::default();
    for (z, rings) in layers {
        let region = Region::new(rings);
        let prepared = prepare(&region, &params);
        let original = region.segments();
        for ring in &prepared.rings {
            for &p in ring {
                let near = original
                    .iter()
                    .map(|&(a, b)| distance_to_segment(p, a, b))
                    .fold(f64::INFINITY, f64::min);
                assert!(
                    near < params.min_spacing,
                    "z={z}: preparing moved the boundary by {near} at {p:?}"
                );
            }
        }
    }
}

#[test]
fn every_plate_layer_gets_a_skeleton_its_geometry_agrees_with() {
    let layers = plate_rings();
    assert!(layers.len() >= 4, "the fixture carries four layers");
    let params = Params::default();
    for (z, rings) in layers {
        let region = Region::new(rings);
        let mut skeleton = build(&region, &params).expect("a plate layer must build");
        assert!(
            skeleton.pair_count() > 0,
            "z={z}: a plate layer has walls to plan"
        );

        let segments = prepare(&region, &params).segments();
        let index = SegIndex::new(segments.clone(), region.bounds().grown(1.0), 2.0);
        let inside = RegionIndex::new(&region.rings);

        for node in skeleton.nodes() {
            let near = index.nearest(node.p, 1.0e3).map(|hit| hit.d).unwrap_or(0.0);
            assert!(
                (near - node.distance).abs() < 1e-3,
                "z={z}: node {:?} reports {} but the boundary is {near} away",
                node.p,
                node.distance
            );
            assert!(
                inside.contains(node.p) || near < 2.0 * params.min_spacing,
                "z={z}: node {:?} is outside the layer",
                node.p
            );
        }

        for node in skeleton.nodes().iter().step_by(32) {
            let mut d: Vec<f64> = segments
                .iter()
                .map(|&(a, b)| distance_to_segment(node.p, a, b))
                .collect();
            d.sort_by(f64::total_cmp);
            assert!(
                (d[0] - d[1]).abs() < 1e-3,
                "z={z}: node {:?} is not equidistant: {} and {}",
                node.p,
                d[0],
                d[1]
            );
        }

        for pair in skeleton.pairs() {
            let floor = skeleton
                .node(pair.a)
                .distance
                .min(skeleton.node(pair.b).distance);
            for &p in &pair.samples {
                let here = index.nearest(p, 1.0e3).map(|hit| hit.d).unwrap_or(0.0);
                assert!(
                    here > floor - 0.02,
                    "z={z}: sample {p:?} is {here} from the boundary, below the edge's end at {floor}"
                );
                assert!(
                    inside.contains(p) || here < 2.0 * params.min_spacing,
                    "z={z}: sample {p:?} is outside the layer"
                );
            }
        }

        classify::classify(&mut skeleton, &ClassifyParams::default());
        assert!(
            skeleton.pairs().iter().any(|p| p.central == Some(true)),
            "z={z}: a plate layer is wide enough somewhere to hold whole beads"
        );
        assert!(
            !skeleton.local_maxima().is_empty(),
            "z={z}: a plate layer has somewhere for a bead count to start"
        );

        let beads = emit::emit(&skeleton, &region, &EmitParams::default());
        assert!(
            !beads.beads.is_empty(),
            "z={z}: every plate layer has a wall to print"
        );
        for bead in &beads.beads {
            assert!(
                bead.widths.iter().all(|w| *w > 0.0 && *w <= 0.65),
                "z={z}: a bead is one wall wide, at most the section it fills when a \
                 section is too thin to divide"
            );
            for &q in &bead.pts {
                assert!(
                    inside.contains(q) || index.nearest(q, 1e3).is_some_and(|hit| hit.d < 1e-3),
                    "z={z}: bead point {q:?} left the layer"
                );
            }
        }
    }
}
