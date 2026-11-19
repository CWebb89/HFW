//! Walls from the section, against the offset ladder and against the layer itself.
//!
//! The ladder draws a loop at a fixed depth with a fixed width; this generator measures
//! the layer's section and divides it among the beads it holds. Both are asked the same
//! question on shapes whose answer is known, and the differences are the point: a disc
//! must come out as concentric rings, a wide section must leave its middle to the
//! infill, a feature narrower than the walls it asked for must be filled by them
//! instead of by a strand.

use hfw_kernel::{Region, RegionIndex, V2};
use hfw_walls::{generate, Kind, Line, WallMode, WallParams};

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

fn area(line: &Line) -> f64 {
    let n = line.pts.len();
    let mut a = 0.0;
    for i in 1..n {
        a += line.pts[i - 1].dist(line.pts[i]) * 0.5 * (line.width[i - 1] + line.width[i]);
    }
    if line.closed && n > 2 {
        a += line.pts[n - 1].dist(line.pts[0]) * 0.5 * (line.width[n - 1] + line.width[0]);
    }
    a
}

fn total(lines: &[Line]) -> f64 {
    lines.iter().map(area).sum()
}

fn walls(rings: Vec<Vec<V2>>) -> (Region, f64, f64, Vec<Line>) {
    let region = Region::new(rings);
    let p = WallParams::new(0.4, 3);
    let ladder = total(&generate(&region, &p, WallMode::Classic).lines);
    let out = generate(&region, &p, WallMode::Hfw);
    let covered = total(&out.lines);
    (region, ladder, covered, out.lines)
}

#[test]
fn a_discs_walls_are_concentric_rings() {
    let (_, ladder, covered, lines) = walls(vec![circle(0.0, 0.0, 10.0, 96)]);
    assert_eq!(lines.len(), 3, "three walls, three rings");
    for line in &lines {
        assert!(line.closed, "a wall of a disc closes on itself");
        assert!(
            line.width.iter().all(|w| (w - 0.4).abs() < 0.02),
            "a 20 mm section holds the walls at the nominal width, got {:?}",
            line.width
        );
        for q in &line.pts {
            let r = q.len();
            assert!(
                (8.5..=10.2).contains(&r),
                "a wall runs at a constant radius from the centre, got {r:.2} at {q:?}"
            );
        }
    }
    assert!(
        (covered / ladder - 1.0).abs() < 0.05,
        "the disc's walls cover {covered:.1} mm² against the ladder's {ladder:.1}"
    );
}

#[test]
fn a_wide_section_leaves_its_middle_to_the_infill() {
    let (_, ladder, covered, lines) = walls(vec![rect(0.0, 0.0, 30.0, 20.0)]);
    assert_eq!(lines.len(), 3, "three walls on a 20 mm section");
    for line in &lines {
        assert!(
            line.width.iter().all(|w| (w - 0.4).abs() < 0.02),
            "nothing to absorb on a wide section, so every bead is nominal"
        );
    }
    assert!(
        (covered / ladder - 1.0).abs() < 0.05,
        "a rectangle's walls cover {covered:.1} mm² against the ladder's {ladder:.1}"
    );
}

#[test]
fn a_wall_narrower_than_its_walls_is_filled_by_them() {
    for (height, low) in [(1.0, 0.78), (1.4, 0.85), (2.0, 0.85)] {
        let (region, _, covered, lines) = walls(vec![rect(0.0, 0.0, 40.0, height)]);
        let band = region_area(&region);
        assert!(
            covered / band > low,
            "a {height} mm wall covers {:.2} of its own area, its beads total {covered:.1} mm² \
             against the layer's {band:.1}",
            covered / band
        );
        assert!(
            lines
                .iter()
                .all(|l| l.width.iter().all(|w| *w <= 0.6 + 1e-9)),
            "no bead is wider than one bead"
        );
        assert!(
            lines
                .iter()
                .all(|l| l.width.iter().all(|w| *w >= 0.24 - 1e-9)),
            "no bead is narrower than the printable floor"
        );
    }
}

#[test]
fn a_profile_outer_width_widens_the_outer_bead_and_pushes_the_rest_in() {
    let region = Region::new(vec![rect(0.0, 0.0, 30.0, 20.0)]);
    let plain = WallParams::new(0.4, 3);
    let mut wide = WallParams::new(0.4, 3);
    wide.w_outer = 0.62;
    let narrow = generate(&region, &plain, WallMode::Hfw).lines;
    let wide = generate(&region, &wide, WallMode::Hfw).lines;
    let outer = |lines: &[Line]| {
        lines
            .iter()
            .find(|l| l.kind == Kind::Side(0))
            .map(|l| l.width.iter().sum::<f64>() / l.width.len() as f64)
            .expect("an outer wall")
    };
    assert!(
        (outer(&narrow) - 0.4).abs() < 0.02,
        "the nominal profile lays a 0.4 mm outer bead, got {:.3}",
        outer(&narrow)
    );
    assert!(
        (outer(&wide) - 0.62).abs() < 0.03,
        "a 0.62 mm outer width reaches the outer bead, got {:.3}",
        outer(&wide)
    );
    let reach = |lines: &[Line]| {
        lines
            .iter()
            .find(|l| l.kind == Kind::Side(0))
            .map(|l| {
                l.pts
                    .iter()
                    .map(|q| q.x.min(q.y))
                    .fold(f64::INFINITY, f64::min)
            })
            .expect("an outer wall")
    };
    assert!(
        (reach(&narrow) - 0.2).abs() < 0.03,
        "a nominal outer bead runs half a bead from the surface, got {:.3}",
        reach(&narrow)
    );
    assert!(
        (reach(&wide) - 0.31).abs() < 0.03,
        "a 0.62 mm outer bead runs half of its own width from the surface, got {:.3}",
        reach(&wide)
    );
}

#[test]
fn a_section_that_divides_exactly_prints_its_middle_bead_once() {
    let (region, _, covered, lines) = walls(vec![rect(0.0, 0.0, 40.0, 1.2)]);
    let band = region_area(&region);
    for line in &lines {
        let pts = &line.pts;
        for i in 1..pts.len() - 1 {
            let a = pts[i - 1];
            let b = pts[i];
            let c = pts[i + 1];
            let t = (a - b).dot(c - b) / ((a - b).len() * (c - b).len()).max(1e-12);
            assert!(
                t < 0.85,
                "a bead doubles back at {b:?}: {a:?} -> {b:?} -> {c:?}"
            );
        }
    }
    assert!(
        covered <= 1.05 * band,
        "a 1.2 mm bar holds 1.2 mm of beads, not a middle bead printed twice: \
         {covered:.1} mm² against the layer's {band:.1}"
    );
}

#[test]
fn no_bead_leaves_the_layer() {
    let region = Region::new(vec![
        rect(0.0, 0.0, 30.0, 20.0),
        circle(15.0, 10.0, 6.0, 64),
    ]);
    let index = hfw_kernel::SegIndex::new(region.segments(), region.bounds(), 2.0);
    let inside = RegionIndex::new(&region.rings);
    let p = WallParams::new(0.4, 3);
    let out = generate(&region, &p, WallMode::Hfw);
    assert!(!out.lines.is_empty());
    for line in &out.lines {
        for q in &line.pts {
            assert!(
                inside.contains(*q) || index.nearest(*q, 0.6).is_some_and(|hit| hit.d < 0.6),
                "bead vertex {q:?} is not on the layer"
            );
        }
    }
}

/// On the real plate the generator must cover what the layer holds, and never leave a
/// sliver narrow enough to need a strand.
///
/// The floor is 0.75 rather than parity because of one known gap: a section holding an
/// odd number of beads has a middle bead whose centre is the section's own midline, and
/// that is a ridge of the field rather than a level set, so it is not traced. It costs
/// the top layer of the drum — 57.4 mm² of layer, thinner than three beads everywhere —
/// about a sixth of its area. The floor is the ratchet that the middle bead has to move.
#[test]
fn the_plate_layers_cover_what_they_hold() {
    for (z, rings) in plate_layers() {
        let (region, ladder, covered, lines) = walls(rings);
        let band = region_area(&region).min(ladder);
        let ratio = covered / band;
        assert!(
            ratio > 0.75 && ratio < 1.15,
            "z={z}: the field generator covers {covered:.1} mm² of the {band:.1} mm² the layer \
             and the ladder hold between them, a ratio of {ratio:.2}"
        );
        assert!(
            lines
                .iter()
                .all(|l| l.width.iter().all(|w| *w >= 0.24 - 1e-9)),
            "z={z}: a strand narrower than the printable floor is a sliver, not a wall"
        );
    }
}

/// No bead is a dot, on any layer of the real plate.
///
/// The plate's 450 layers carry a lot of soft creases and merged fillets, and a spot
/// where the section briefly deepens past a bead centre closes a three-vertex loop
/// around itself: 1,099 of them across the plate before the floor was raised, each one
/// a 0.3 mm dab printed on material already there. They are what reads as a lap or a
/// cap on a printed wall, and the ladder leaves 559 of them of its own.
#[test]
fn no_bead_is_shorter_than_three_of_its_own_widths() {
    assert!(
        !PLATE.is_empty(),
        "the plate fixture is what makes this test real"
    );
    for (z, rings) in plate_layers() {
        let (_, _, _, lines) = walls(rings);
        for line in &lines {
            let width = line.width.iter().sum::<f64>() / line.width.len() as f64;
            let mut run = 0.0;
            for i in 1..line.pts.len() {
                run += line.pts[i - 1].dist(line.pts[i]);
            }
            if line.closed && line.pts.len() > 2 {
                run += line.pts[line.pts.len() - 1].dist(line.pts[0]);
            }
            assert!(
                run >= 3.0 * width - 1e-9,
                "z={z}: a {run:.2} mm bead of {width:.2} mm width is a dot, not a wall"
            );
        }
    }
    const PLATE: &str = include_str!("../../kernel/tests/plate-rings.txt");
}

#[test]
fn two_runs_of_the_same_layer_agree() {
    let region = Region::new(vec![
        rect(0.0, 0.0, 30.0, 20.0),
        circle(15.0, 10.0, 6.0, 64),
    ]);
    let p = WallParams::new(0.4, 3);
    let a = generate(&region, &p, WallMode::Hfw);
    let b = generate(&region, &p, WallMode::Hfw);
    assert_eq!(a.lines.len(), b.lines.len());
    for (x, y) in a.lines.iter().zip(&b.lines) {
        assert_eq!(x.pts, y.pts, "the same layer must trace the same walls");
        assert_eq!(x.width, y.width);
    }
}

fn region_area(region: &Region) -> f64 {
    region
        .rings
        .iter()
        .map(|r| hfw_kernel::ring_signed_area(r).abs())
        .sum()
}

fn plate_layers() -> Vec<(f64, Vec<Vec<V2>>)> {
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
