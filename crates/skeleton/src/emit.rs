//! Bead centrelines from a skeleton, joined where they meet.
//!
//! Every sample of every edge carries the layer's width across it, so a
//! cross-section there is a segment perpendicular to the edge with beads laid
//! across it. Walking the samples of one edge therefore gives one polyline per
//! bead, and the polylines have to be joined where two edges meet or a wall is
//! printed as a forest of short strands with a bead's width of overlap at every
//! junction.
//!
//! The join is by boundary: each bead end knows which boundary segment the bead it
//! belongs to is running beside, because the sample's section recorded which
//! segment each of its two rays reached. Two ends beside the same segment are the
//! same bead and the chain continues along that wall, which is what makes a bead
//! follow a contour instead of turning at every junction. Ends whose segments
//! differ but which reach nearly the same point are joined too, and the rest are
//! left as ends — that is where a bead count has to change, and that is the
//! transition stage's job.

use hfw_kernel::{Region, RegionIndex, V2};

use crate::bead::{self, Limited, Section, Strategy};
use crate::graph::{normals, Graph, NodeId};

/// How beads are placed and joined.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// The nominal bead width, in millimetres.
    pub optimal: f64,
    /// The most walls one section may be filled with.
    pub walls: usize,
    /// How far apart two bead ends may be and still be one bead, in millimetres.
    ///
    /// Ends further apart than this are not the same bead: a junction is where the
    /// width changes, and joining across a gap half a bead wide would fold a wall
    /// onto itself.
    pub join_distance: f64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            optimal: 0.4,
            walls: 3,
            join_distance: 0.6,
        }
    }
}

/// One bead centreline.
#[derive(Clone, Debug)]
pub struct Bead {
    /// Points in millimetres; a closed bead does not repeat its first point.
    pub pts: Vec<V2>,
    /// Extrusion width at each point.
    pub widths: Vec<f64>,
    /// True when the bead returns to where it started.
    pub closed: bool,
}

impl Bead {
    /// Polyline length, including the closing segment.
    pub fn length(&self) -> f64 {
        let mut s = 0.0;
        for i in 1..self.pts.len() {
            s += self.pts[i - 1].dist(self.pts[i]);
        }
        if self.closed && self.pts.len() > 2 {
            s += self.pts[self.pts.len() - 1].dist(self.pts[0]);
        }
        s
    }

    /// Area this bead covers when laid down, in square millimetres.
    ///
    /// The width varies along a bead, so each segment is charged its mean width.
    pub fn area(&self) -> f64 {
        let n = self.pts.len();
        let mut s = 0.0;
        for i in 1..n {
            s += self.pts[i - 1].dist(self.pts[i]) * 0.5 * (self.widths[i - 1] + self.widths[i]);
        }
        if self.closed && n > 2 {
            s += self.pts[n - 1].dist(self.pts[0]) * 0.5 * (self.widths[n - 1] + self.widths[0]);
        }
        s
    }
}

/// What one layer produced.
#[derive(Clone, Debug, Default)]
pub struct Output {
    /// Bead centrelines.
    pub beads: Vec<Bead>,
    /// Bead ends that found no partner. Zero is a layer whose beads all close.
    pub open_ends: usize,
    /// Area the beads cover when laid down, in square millimetres.
    pub area: f64,
}

/// One bead of one or more edges, running from one node to another.
#[derive(Clone, Debug)]
struct Piece {
    pts: Vec<V2>,
    widths: Vec<f64>,
    head: NodeId,
    tail: NodeId,
    /// Boundary segment the bead runs beside at each end, or `u32::MAX`.
    head_site: u32,
    tail_site: u32,
    dead: bool,
}

/// Lays beads along every edge of the skeleton and joins them at the nodes.
///
/// Every edge carries beads, central or not, because a count can only change along
/// an edge in the transition stage and that stage does not exist yet. The cost of
/// that is visible and measured: on a round part the walls run down the medial arms
/// and overlap instead of following the contour, so the beads cover more area than
/// the walls a profile asked for. `docs/WALLS-SKELETON.md` records the numbers.
pub fn emit(graph: &Graph, region: &Region, p: &Params) -> Output {
    let inside = RegionIndex::new(&region.rings);
    let strategy = Limited {
        optimal: p.optimal,
        max_count: p.walls,
    };
    let mut pieces: Vec<Piece> = Vec::new();
    for pair in graph.pairs() {
        if pair.samples.len() < 2 || pair.spans.len() != pair.samples.len() {
            continue;
        }
        let middle = pair.spans[pair.spans.len() / 2];
        let count = bead::count_for(middle.thickness(), p.optimal, p.walls);
        if count == 0 {
            continue;
        }
        let normals = normals(&pair.samples);
        let mut lanes: Vec<(Vec<V2>, Vec<f64>, f64, f64)> =
            vec![(Vec::new(), Vec::new(), 0.0, 0.0); count];
        for (k, &q) in pair.samples.iter().enumerate() {
            let span = pair.spans[k];
            let cross = strategy.compute(
                Section {
                    thickness: span.thickness(),
                    skew: span.skew(),
                },
                count,
            );
            for (j, lane) in lanes.iter_mut().enumerate() {
                let (Some(&offset), Some(&width)) = (cross.locations.get(j), cross.widths.get(j))
                else {
                    continue;
                };
                lane.0.push(q + normals[k] * offset);
                lane.1.push(width);
                if k == 0 {
                    lane.2 = offset;
                }
                lane.3 = offset;
            }
        }
        let (head_span, tail_span) = (pair.spans[0], pair.spans[pair.spans.len() - 1]);
        for (pts, widths, first_offset, last_offset) in lanes {
            if pts.len() < 2 {
                continue;
            }
            pieces.push(Piece {
                pts,
                widths,
                head: pair.a,
                tail: pair.b,
                head_site: side_site(head_span, first_offset),
                tail_site: side_site(tail_span, last_offset),
                dead: false,
            });
        }
    }

    let open_ends = join(&mut pieces, p.join_distance);
    let beads: Vec<Bead> = pieces
        .iter()
        .filter(|piece| !piece.dead && piece.pts.len() >= 2)
        .flat_map(|piece| clip(piece.to_bead(), &inside, p.optimal))
        .filter(|bead| bead.length() >= p.optimal)
        .collect();
    let area = beads.iter().map(Bead::area).sum();
    Output {
        beads,
        open_ends,
        area,
    }
}

/// Splits a bead into the runs of it that lie in the layer.
///
/// A bead is laid across a section measured by two rays, and a section is a chord:
/// where the layer is concave between the ray's two hits, the middle of the chord is
/// outside it. Printing that middle is printing outside the part, so the bead is cut
/// at every point that leaves the layer and only the runs at least a bead long
/// survive.
fn clip(bead: Bead, inside: &RegionIndex, shortest: f64) -> Vec<Bead> {
    let mut out = Vec::new();
    let mut pts: Vec<V2> = Vec::new();
    let mut widths: Vec<f64> = Vec::new();
    let mut runs: Vec<(Vec<V2>, Vec<f64>)> = Vec::new();
    for (i, &q) in bead.pts.iter().enumerate() {
        if inside.contains(q) {
            pts.push(q);
            widths.push(bead.widths[i]);
        } else if !pts.is_empty() {
            runs.push((std::mem::take(&mut pts), std::mem::take(&mut widths)));
        }
    }
    if !pts.is_empty() {
        runs.push((pts, widths));
    }
    let closed = bead.closed;
    if runs.len() == 1 && runs[0].0.len() == bead.pts.len() {
        out.push(bead);
        return out;
    }
    for (pts, widths) in runs {
        let piece = Bead {
            pts,
            widths,
            closed: false,
        };
        if piece.length() >= shortest {
            out.push(piece);
        }
    }
    let _ = closed;
    out
}

impl Piece {
    /// The finished bead this piece describes.
    fn to_bead(&self) -> Bead {
        let last = self.pts.len() - 1;
        let closes = self.head == self.tail && self.pts[0].dist(self.pts[last]) < 1e-9;
        let mut pts = self.pts.clone();
        let mut widths = self.widths.clone();
        if closes {
            pts.pop();
            widths.pop();
        }
        Bead {
            pts,
            widths,
            closed: closes,
        }
    }
}

/// Which boundary segment a bead at `offset` from a sample runs beside.
fn side_site(span: crate::graph::Span, offset: f64) -> u32 {
    if offset > span.skew() {
        span.ahead_site
    } else {
        span.back_site
    }
}

/// One free end of a piece: the node it sits at, which piece, which end, where it
/// reaches and which boundary it was following.
#[derive(Clone, Copy, Debug)]
struct End {
    node: NodeId,
    piece: usize,
    head: bool,
    point: V2,
    site: u32,
    /// Direction the bead points in as it reaches this end, away from its piece.
    dir: V2,
}

/// Joins bead ends that meet at a node.
///
/// Ends at the same node are matched in two passes: first those beside the same
/// boundary segment, then those that merely reach the same point, both within
/// `distance`. Returns how many ends are left over.
fn join(pieces: &mut [Piece], distance: f64) -> usize {
    let mut ends: Vec<End> = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        if piece.dead {
            continue;
        }
        let last = piece.pts.len() - 1;
        ends.push(End {
            node: piece.head,
            piece: i,
            head: true,
            point: piece.pts[0],
            site: piece.head_site,
            dir: outward(piece.pts[0], piece.pts[1.min(last)]),
        });
        ends.push(End {
            node: piece.tail,
            piece: i,
            head: false,
            point: piece.pts[last],
            site: piece.tail_site,
            dir: outward(piece.pts[last], piece.pts[last.saturating_sub(1)]),
        });
    }
    ends.sort_by(|a, b| {
        a.node
            .cmp(&b.node)
            .then_with(|| a.point.x.total_cmp(&b.point.x))
    });

    let mut start = 0;
    while start < ends.len() {
        let node = ends[start].node;
        let mut stop = start;
        while stop < ends.len() && ends[stop].node == node {
            stop += 1;
        }
        let reach = distance.max(3.0 * 0.4);
        match_at(pieces, &ends[start..stop], reach, true);
        match_at(pieces, &ends[start..stop], reach, false);
        start = stop;
    }

    let mut open = 0;
    for piece in pieces.iter().filter(|piece| !piece.dead) {
        open += 2;
        if piece.head == piece.tail && piece.pts[0].dist(piece.pts[piece.pts.len() - 1]) < 1e-9 {
            open -= 2;
        }
    }
    open
}

fn match_at(pieces: &mut [Piece], ends: &[End], distance: f64, same_site: bool) {
    let mut used: Vec<bool> = ends.iter().map(|end| pieces[end.piece].dead).collect();
    for i in 0..ends.len() {
        if used[i] {
            continue;
        }
        let mut best: Option<(usize, f64)> = None;
        for j in (i + 1)..ends.len() {
            if used[j] || ends[j].piece == ends[i].piece {
                continue;
            }
            if same_site && ends[j].site != ends[i].site {
                continue;
            }
            if ends[i].dir.dot(ends[j].dir) > -0.2 {
                continue;
            }
            let gap = ends[i].point.dist(ends[j].point);
            if gap <= distance && best.is_none_or(|(_, d)| gap < d) {
                best = Some((j, gap));
            }
        }
        let Some((j, _)) = best else {
            continue;
        };
        used[i] = true;
        used[j] = true;
        splice(
            pieces,
            ends[i].piece,
            ends[i].head,
            ends[j].piece,
            ends[j].head,
            ends[i].point.lerp(ends[j].point, 0.5),
        );
        for (k, end) in ends.iter().enumerate() {
            if pieces[end.piece].dead {
                used[k] = true;
            }
        }
    }
}

/// Direction pointing away from a piece at one of its ends.
fn outward(here: V2, inward: V2) -> V2 {
    let d = here - inward;
    if d.len() > 1e-12 {
        d.norm()
    } else {
        V2::ZERO
    }
}

/// Joins one end of `first` to one end of `second`, through a shared point.
fn splice(
    pieces: &mut [Piece],
    first: usize,
    first_head: bool,
    second: usize,
    second_head: bool,
    point: V2,
) {
    let head = {
        let a = &pieces[first];
        if first_head {
            a.tail
        } else {
            a.head
        }
    };
    let tail = {
        let b = &pieces[second];
        if second_head {
            b.tail
        } else {
            b.head
        }
    };
    let (head_site, tail_site) = {
        let a = &pieces[first];
        let b = &pieces[second];
        let hs = if first_head { a.tail_site } else { a.head_site };
        let ts = if second_head {
            b.tail_site
        } else {
            b.head_site
        };
        (hs, ts)
    };

    let mut pts: Vec<V2> = Vec::new();
    let mut widths: Vec<f64> = Vec::new();
    {
        let a = &pieces[first];
        if first_head {
            pts.extend(a.pts.iter().rev().copied());
            widths.extend(a.widths.iter().rev().copied());
        } else {
            pts.extend(a.pts.iter().copied());
            widths.extend(a.widths.iter().copied());
        }
    }
    let w_here = {
        let b = &pieces[second];
        let w_b = if second_head {
            b.widths[0]
        } else {
            b.widths[b.widths.len() - 1]
        };
        (widths[widths.len() - 1] + w_b) * 0.5
    };
    if pts[pts.len() - 1].dist(point) > 1e-12 {
        pts.push(point);
        widths.push(w_here);
    }
    {
        let b = &pieces[second];
        if second_head {
            pts.extend(b.pts.iter().copied());
            widths.extend(b.widths.iter().copied());
        } else {
            pts.extend(b.pts.iter().rev().copied());
            widths.extend(b.widths.iter().rev().copied());
        }
    }
    if pts.len() > 1 && pts[pts.len() - 1].dist(pts[pts.len() - 2]) < 1e-12 {
        pts.pop();
        widths.pop();
    }

    pieces[first] = Piece {
        pts,
        widths,
        head,
        tail,
        head_site,
        tail_site,
        dead: false,
    };
    pieces[second].dead = true;
    pieces[second].pts.clear();
    pieces[second].widths.clear();
}
