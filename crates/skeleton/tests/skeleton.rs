//! What the skeleton has to be true of before the beading stage may rely on it.

use hfw_kernel::{ring_contains, SegIndex, V2};
use hfw_skeleton::classify::{self, Params as ClassifyParams};
use hfw_skeleton::{build, prepare, Graph, Kind, Params};

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
    let region = hfw_kernel::Region::new(rings);
    build(&region, &Params::default()).expect("the skeleton builder must not fail")
}

fn prepared(rings: Vec<Vec<V2>>) -> Vec<(V2, V2)> {
    prepare(&hfw_kernel::Region::new(rings), &Params::default()).segments()
}

fn features(segments: &[(V2, V2)]) -> Vec<(V2, V2)> {
    let mut out: Vec<(V2, V2)> = segments.to_vec();
    for &(a, b) in segments {
        out.push((a, a));
        out.push((b, b));
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
fn a_star_of_two_hundred_reflex_corners_builds() {
    let n = 200;
    let ring: Vec<V2> = (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            let r = if i % 2 == 0 { 5.0 } else { 2.5 };
            V2::new(r * a.cos(), r * a.sin())
        })
        .collect();
    let s = skeleton(vec![ring.clone()]);
    assert!(s.pair_count() > n / 2, "got {} edges", s.pair_count());
    let segments = prepared(vec![ring]);
    let all = features(&segments);
    for pair in s.pairs() {
        for &p in &pair.samples {
            assert!(
                all.iter()
                    .map(|&(a, b)| distance_to_segment(p, a, b))
                    .fold(f64::INFINITY, f64::min)
                    < 5.0,
                "sample {p:?} is nowhere near the star"
            );
        }
    }
    for node in s.nodes() {
        let mut d: Vec<f64> = all
            .iter()
            .map(|&(a, b)| distance_to_segment(node.p, a, b))
            .collect();
        d.sort_by(f64::total_cmp);
        assert!(
            (d[0] - d[1]).abs() < 1e-3,
            "node {:?} is not equidistant: {} and {}",
            node.p,
            d[0],
            d[1]
        );
    }
}

#[test]
fn a_square_gets_a_cross_shaped_skeleton() {
    let s = skeleton(vec![rect(0.0, 0.0, 20.0, 20.0)]);
    assert_eq!(s.node_count(), 5, "four corners and the centre");
    assert_eq!(s.pair_count(), 4, "one arm to each corner");
    let centre = s
        .nodes()
        .iter()
        .find(|n| n.p.dist(V2::new(10.0, 10.0)) < 0.01)
        .expect("the middle of the square is a node of its skeleton");
    assert!(
        (centre.distance - 10.0).abs() < 0.01,
        "half the square's side, got {}",
        centre.distance
    );
}

#[test]
fn a_circle_collapses_to_its_centre() {
    let s = skeleton(vec![circle(0.0, 0.0, 5.0, 64)]);
    for node in s.nodes() {
        if node.distance < 0.5 {
            continue;
        }
        assert!(
            node.p.len() < 0.3,
            "an interior node of a 64-gon's axis is at its centre, got {:?}",
            node
        );
        assert!(
            (node.distance - 5.0).abs() < 0.02,
            "and the whole radius from the boundary, got {}",
            node.distance
        );
    }
}

#[test]
fn a_slot_gets_one_spine_down_its_middle() {
    let s = skeleton(vec![rect(0.0, 0.0, 30.0, 3.0)]);
    let widest = s.nodes().iter().map(|n| n.distance).fold(0.0_f64, f64::max);
    assert!(
        (widest - 1.5).abs() < 0.02,
        "half the slot's width, got {widest}"
    );
    assert!(s.pair_count() <= 8, "got {} edges", s.pair_count());
}

#[test]
fn every_node_sits_where_two_boundary_points_are_equally_near() {
    for rings in [
        vec![rect(0.0, 0.0, 20.0, 20.0)],
        vec![rect(0.0, 0.0, 30.0, 3.0)],
        vec![circle(0.0, 0.0, 5.0, 48)],
        vec![vec![
            V2::new(0.0, 0.0),
            V2::new(20.0, 0.0),
            V2::new(2.0, 2.0),
            V2::new(20.0, 8.0),
        ]],
        vec![rect(0.0, 0.0, 40.0, 40.0), rect(10.0, 10.0, 30.0, 30.0)],
    ] {
        let s = skeleton(rings.clone());
        let all = features(&prepared(rings));
        for node in s.nodes() {
            let mut d: Vec<f64> = all
                .iter()
                .map(|&(a, b)| distance_to_segment(node.p, a, b))
                .collect();
            d.sort_by(f64::total_cmp);
            assert!(
                (d[0] - d[1]).abs() < 1e-3,
                "node {:?} is not equidistant: {} and {}",
                node.p,
                d[0],
                d[1]
            );
            assert!(
                (d[0] - node.distance).abs() < 1e-3,
                "node {:?} reports {} but the boundary is {} away",
                node.p,
                node.distance,
                d[0]
            );
        }
    }
}

#[test]
fn an_edge_never_comes_nearer_the_boundary_than_its_nearer_end() {
    for rings in [
        vec![rect(0.0, 0.0, 20.0, 20.0)],
        vec![circle(0.0, 0.0, 5.0, 48)],
        vec![vec![
            V2::new(0.0, 0.0),
            V2::new(20.0, 0.0),
            V2::new(2.0, 2.0),
            V2::new(20.0, 8.0),
        ]],
    ] {
        let s = skeleton(rings.clone());
        let segments = prepared(rings);
        let index = SegIndex::new(
            segments.clone(),
            hfw_kernel::Bounds::empty().grown(1.0e3),
            2.0,
        );
        for pair in s.pairs() {
            let floor = s.node(pair.a).distance.min(s.node(pair.b).distance);
            for &p in &pair.samples {
                let here = index.nearest(p, 1.0e3).map(|h| h.d).unwrap_or(0.0);
                assert!(
                    here > floor - 0.02,
                    "sample {p:?} is {here} from the boundary, below the edge's end at {floor}"
                );
            }
        }
    }
}

#[test]
fn every_edge_lies_inside_the_region() {
    let rings = vec![rect(0.0, 0.0, 40.0, 40.0), rect(10.0, 10.0, 30.0, 30.0)];
    let region = hfw_kernel::Region::new(rings.clone());
    let inner = rings[1].clone();
    let segments = prepared(rings.clone());
    let s = skeleton(rings);
    for pair in s.pairs() {
        for &p in &pair.samples {
            let on_boundary = segments
                .iter()
                .any(|&(a, b)| distance_to_segment(p, a, b) < 1e-6);
            assert!(
                region.contains(p) || on_boundary,
                "sample {p:?} is outside the region"
            );
            assert!(
                !ring_contains(&inner, p) || on_boundary,
                "sample {p:?} is inside the hole, where nothing can be printed"
            );
        }
    }
}

#[test]
fn an_empty_region_builds_nothing() {
    let s = build(&hfw_kernel::Region::default(), &Params::default()).expect("no diagram to fail");
    assert!(s.is_empty());
    assert_eq!(s.node_count(), 0);
}

#[test]
fn the_same_region_builds_the_same_graph_twice() {
    let rings = vec![rect(0.0, 0.0, 40.0, 40.0), rect(10.0, 10.0, 30.0, 30.0)];
    let a = skeleton(rings.clone());
    let b = skeleton(rings);
    assert_eq!(a.node_count(), b.node_count());
    assert_eq!(a.pair_count(), b.pair_count());
    for (x, y) in a.nodes().iter().zip(b.nodes()) {
        assert_eq!(x.p.x.to_bits(), y.p.x.to_bits());
        assert_eq!(x.p.y.to_bits(), y.p.y.to_bits());
        assert_eq!(x.distance.to_bits(), y.distance.to_bits());
    }
    for (x, y) in a.pairs().iter().zip(b.pairs()) {
        assert_eq!(x.samples.len(), y.samples.len());
        assert_eq!(x.kind, y.kind);
        for (p, q) in x.samples.iter().zip(&y.samples) {
            assert_eq!(p.x.to_bits(), q.x.to_bits());
            assert_eq!(p.y.to_bits(), q.y.to_bits());
        }
    }
}

#[test]
fn a_wide_body_is_central_and_a_tapered_tip_is_not() {
    let s = skeleton(vec![rect(0.0, 0.0, 30.0, 20.0)]);
    let mut s = s;
    classify::classify(&mut s, &ClassifyParams::default());
    assert!(
        s.pairs().iter().any(|p| p.central == Some(true)),
        "the spine of a 20 mm wide body is central"
    );

    let mut taper = skeleton(vec![vec![
        V2::new(0.0, 0.0),
        V2::new(40.0, 0.0),
        V2::new(0.0, 4.0),
    ]]);
    classify::classify(&mut taper, &ClassifyParams::default());
    for pair in taper.pairs() {
        if pair.central == Some(true) {
            let ce = 0.5 * ClassifyParams::default().transitioning_angle.sin();
            let da = taper.node(pair.a).distance;
            let db = taper.node(pair.b).distance;
            let chord = taper.node(pair.a).p.dist(taper.node(pair.b).p);
            assert!(
                (db - da).abs() < chord * ce + 1e-9,
                "a central edge may not change width faster than the transition angle allows"
            );
        }
    }
}

#[test]
fn a_central_stretch_too_short_for_a_transition_is_dissolved() {
    let mut s = skeleton(vec![circle(0.0, 0.0, 5.0, 64)]);
    classify::classify(&mut s, &ClassifyParams::default());
    let dissolved = classify::filter_central(&mut s, 1.0e6);
    assert_eq!(
        dissolved,
        s.pairs().iter().filter(|p| p.central == Some(true)).count() + dissolved,
        "every central edge is named once"
    );
    assert_eq!(
        s.pairs().iter().filter(|p| p.central == Some(true)).count(),
        0,
        "a circle's whole skeleton is shorter than the filter"
    );
}

#[test]
fn a_rib_is_never_central() {
    let mut s = skeleton(vec![rect(0.0, 0.0, 20.0, 20.0)]);
    classify::classify(&mut s, &ClassifyParams::default());
    for pair in s.pairs() {
        if pair.kind == Kind::Rib {
            assert_eq!(pair.central, Some(false));
        }
    }
}

#[test]
fn a_local_maximum_is_the_widest_node_of_its_neighbourhood() {
    let s = skeleton(vec![rect(0.0, 0.0, 30.0, 3.0)]);
    let maxima = s.local_maxima();
    assert!(!maxima.is_empty(), "a slot's spine has a widest point");
    for &m in &maxima {
        let d = s.node(m).distance;
        for &e in s.edges_at(m) {
            assert!(d >= s.node(s.half(e).to).distance);
        }
    }
}
