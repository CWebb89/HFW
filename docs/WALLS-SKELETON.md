# Walls: replacing the offset ladder with a skeletal generator

The wall generator is `crates/walls`. It runs in one of two modes: `Mode::Hfw`
(`crates/walls/src/beads.rs`), the default, which divides each layer's measured section
among the beads that section holds; and `Mode::Classic` (`crates/walls/src/poly.rs`), the
ladder of polygon offsets this document set out to replace. The ladder is fast and exact
on the boundary, and it has one structural defect that shows on a print. This document
records the defect with the measurement, what Arachne actually does about it, and how the
same thing is built here in Rust.

## The defect, measured

A ladder of constant-width offsets cannot cover an area whose width is not a whole
number of beads, so the generator measures the remainder and prints it as one strand
per connected piece (`gap_lines`). On the Revolver plate (450 layers, 18 objects,
0.4 mm external width) that is **6,663 gap strands**, and:

| measurement | value |
|---|---|
| gap strands starting within 0.6 mm of the outer wall | 5,224 (78%) |
| gap strands shorter than 1.5 mm | 5,171 (78%) |
| beads wider than 2× nominal, any line type | 0 |

So the remainder is not a wide strand in the middle of the wall band — it is a
sliver along the outer loop itself, thousands of short beads each with two ends on
the visible surface. That is what reads as little laps or caps on the wall. It is
not a width or flow bug: no bead on the plate is wider than 0.84 mm.

Two smaller contributions survive the same way: the miter join at a sharp vertex
overshoots the true offset (a spike of up to `w/2 · cosh` of the corner angle), and
where two loops merge the offset changes topology and the merged loop starts on a
edge that the previous layer covered differently.

## What Arachne does

Read from `~/dev/reference/OrcaSlicer/src/libslic3r/Arachne` (CuraEngine code,
AGPL-3.0 — see Licensing). Stages, in the order `WallToolPaths::generate` runs them:

1. **Prepare the outline** for a Voronoi build: offset out and back by
   `wall_maximum_deviation / 2 - 1` to kill near-self-intersections, simplify at
   0.5 mm / 25 µm deviation, `fixSelfIntersections`, drop degenerate and collinear
   vertices, drop areas under `0.5²` mm, then a Clipper union. Non-negotiable: the
   Voronoi builder and the graph walk below both assume a clean manifold outline.
2. **Build the skeletal trapezoidation** (`SkeletalTrapezoidation::constructFromPolygons`)
   as a Voronoi diagram of the *segments* of the outline, not of points. Every
   diagram edge becomes a half-edge with a twin; parabolic (curved) Voronoi edges are
   discretized at `discretization_step_size = 0.8 mm`. The graph has three edge
   classes: **boundary** (on the outline), **rib** (connects a boundary point to the
   skeleton — a place a bead can start), and **central** (interior skeleton).
3. **Classify** (`updateIsCentral`): an edge is central when the local tapered
   geometry leaves room for a full bead, decided from the beading strategy's
   `transitioning_angle` (60° default basis). `filterCentral` and
   `filterOuterCentral` then drop central stretches shorter than the transition
   length they would need.
4. **Bead count per location** (`updateBeadCount`,
   `propagateBeadingsUpward`/`Downward`): the strategist answers
   `compute(thickness, bead_count)` with `{bead_widths, toolpath_locations,
   left_over}`; counts propagate from the graph's local maxima down the ribs, and
   `dissolveBeadCountRegion` deletes regions whose count would only survive a moment.
5. **Beading strategies compose**, outermost first: `Widening` (prints features
   below `min_feature_size` at reduced width), `Distributed` (hands the leftover
   `thickness - count × optimal` to the middle beads by a parabolic weight), `Limited`
   (caps the count at the requested perimeters and returns the rest as inner
   contour), and `Redistribute` / `OuterWallInset` for the visible-surface width and
   inset. The leftover is absorbed **into the bead widths**, which is exactly what the
   ladder cannot do.
6. **Transitions** (`generateTransitionMids`, `generateTransitioningRibs`,
   `generateTransitionEnds`, `applyTransitions`): every bead-count change is a ramp
   along a central edge whose slope is bounded by the transitioning angle, then
   filtered by `transition_filter_dist = 100 µm` and
   `wall_transition_filter_deviation`, so a one-layer wobble in the count cannot
   print as a corner.
7. **Toolpaths** (`generateJunctions`, `connectJunctions`,
   `generateLocalMaximaSingleBeads`): junctions are `(point, width)` nodes at both
   ends of every graph edge, interpolated along transitions; connecting them yields
   polylines with a width per vertex. Local maxima that are a single bead wide get
   that bead explicitly.
8. **Post** (`stitchToolPaths`, `removeSmallLines`, `separateOutInnerContour`,
   `simplifyToolPaths`, `removeEmptyToolPaths`): stitch loose ends into closed loops,
   drop lines shorter than half the narrowest width along them — the step that would
   have deleted all 5,171 short slivers — and emit the inner contour for the infill
   to start from.

Cost in C++: about 5,500 lines across the directory.

## What HFW already has

- **The output side is done.** `walls::Line` already carries `pts` plus a
  per-vertex `width`, `Kind::Middle` exists for a bead straddling a centreline, the
  writer already interpolates flow along a varying width, and the verifier checks
  nothing at a fixed width. A variable-width generator needs no change below it.
- **The settings are already parsed and unused**: `min_bead_width`,
  `min_feature_size`, `wall_transition_length`, `wall_transition_angle`,
  `wall_transition_filter_deviation`, `wall_distribution_count`
  (`crates/config/src/lib.rs`). The ladder reads none of them; an Arachne-shaped
  generator reads all of them.
- **The region kernel is there**: `Region`, `offset_rings`, `difference_rings`,
  `SegIndex`, and the morphological closing in `walls::repair` supply stage 1.
- **Parallelism and determinism**: layers already run on every core with
  byte-identical output, so the new generator inherits that discipline — no
  `HashMap` iteration order in control flow, no float accumulation order that
  depends on thread scheduling.

## The Rust design

New crate `crates/skeleton`, dependencies: `hfw-kernel` for geometry, and
`boostvoronoi` 0.12.1 (BSL-1.0) for the Voronoi of segments. That crate is a port of
the same Boost.Polygon algorithm Arachne calls, and it exposes what the port needs:
`Edge::is_curved` / `is_linear` for parabolic edges, `twin`, `next`, `prev`,
`CellIndex → source`, and `Line` sources for segments. A hand-written Voronoi of
segments is a month of work on its own and a source of unreproducible crashes; this
is the one place a dependency pays.

```
crates/skeleton/src/
  graph.rs      half-edge arena: nodes, half-edges, edge kinds, per-end junction
  build.rs      outline preparation, Voronoi call, diagram → arena
  classify.rs   is_central, central filtering, outer-central filtering
  bead.rs       BeadingStrategy trait + Widening, Distributed, Limited, Redistribute
  transition.rs transition mids, ribs, ends; filtering by distance and deviation
  emit.rs       junctions → variable-width polylines, stitching, small-line removal
```

Design decisions:

- **Arena, not pointers.** `Vec<Node>` + `Vec<HalfEdge>` with `EdgeIndex(u32)` twins,
  the same shape the crate already uses for its region rings. Node and edge storage
  is created once per layer and reused between layers.
- **Beading is a trait object per layer**, composed exactly as upstream, because the
  composition order is the algorithm: widening, then distribution, then the
  perimeter cap. Three concrete impls plus the cap; the second call site is the
  redistribution pass.
- **Widths in `f64` millimetres, thresholds in the settings' units.** The ladder's
  parameters are already in millimetres; no scaled-integer layer is needed and none
  should be introduced, because `V2` is `f64`.
- **Every iteration sorted** by node index, then edge index. Determinism is a
  project rule, and a graph algorithm is where it breaks silently.
- **Keep the ladder behind a flag** (`--mode hfw | classic`) until the oracle
  passes at equal volume, then leave it as the comparison mode.

## Phases

1. **Skeleton build** (graph + build + classify) — **done**. `crates/skeleton`
   holds the arena (`graph.rs`), the outline preparation and Voronoi walk
   (`build.rs`) and the central-edge test with its short-chain filter
   (`classify.rs`). 16 tests pin the behaviour. Synthetic: a square gives five
   nodes and four arms, a 30 × 3 slot one spine at 1.5 mm, a 64-gon an axis at its
   centre, a 200-point star builds and stays equidistant, every node is
   equidistant from two boundary features, no edge comes nearer the boundary than
   its own ends, nothing survives inside a hole, a tapered tip is not central, and
   two builds of the same region are bit-identical. Real: all four plate layers in
   `crates/kernel/tests/plate-rings.txt` build, every node reports the boundary
   distance it really has, every node and every sample lies inside the layer, and
   `prepare` does not move the boundary.

   Four facts found the hard way, recorded so phase 2 does not rediscover them:
   the diagram reports coordinates in input lattice units, not millimetres;
   `Edge::is_primary` is set on both halves in this port, so a pair is walked by
   `id < twin` instead; a keep-if-the-midpoint-is-inside test lets an edge that
   merely crosses the boundary drag an exterior node in, so every sample has to be
   inside; and containment is asked once per sample, which needs an index —
   `kernel::RegionIndex` exists for that.

   **Preparation is a bounded simplifier plus a spacing floor.** Douglas–Peucker
   never moves the outline by more than its tolerance, and dropping a vertex within
   `min_spacing` of the one before it moves nothing. A vertex may *not* simply be
   dropped because it is close to its neighbours' chord: a run of nearly collinear
   vertices each inside the tolerance of its *original* neighbours collapsed a
   1186 mm² outline into a 478 mm² one, which the boundary-does-not-move test
   caught. On the plate that measurably matters — 13,085 boundary vertices become
   2,435, and the diagram drops from 480 ms to 80 ms.

   **Cost, measured**: the heaviest plate layer (four knurled drums) builds its
   whole skeleton in **130 ms** now that each part gets its own diagram and the
   outline is simplified. A typical layer is a small fraction of that. `emit` on
   four plate layers adds under 100 ms on top. That is still above the 5 ms a
   layer's wall stage would like, and the remaining lever is one diagram per part
   in parallel, which the parts already allow.

   Still open: ribs. Distance to the boundary is measured by the kernel's segment
   index rather than anchored on boundary nodes, which is the same number and one
   fewer structure; ribs come back where the beading stage needs the boundary
   anchor.

2. **Beading and emission** — **done as far as a constant count goes**. `bead.rs`
   turns a section into beads (`Even`, `Limited`, `count_for`), `emit.rs` walks the
   samples of every edge to lay one polyline per bead, measures the section with two
   marched rays, joins bead ends at the nodes by the boundary segment they run
   beside, cuts beads that leave the layer, and drops beads shorter than one bead.
   `Pair::spans` carries the section with every sample, and `Span` carries the two
   ray hits, so a bead is placed on the section rather than on twice the medial
   distance — in the arm of a wedge those differ by `1 / cos θ`, which is why a
   corner is filled rather than left with a taper.

   **The exit criterion as originally written — 1% volume parity against the ladder
   on 100% infill — is not reachable at this stage, and the measurement says why.**
   Wall area against the ladder's, same shapes, `walls = 3`, `w = 0.4`:

   | shape | ladder | skeletal | ratio |
   |---|---|---|---|
   | 30 × 20 rectangle | 114.2 mm² | 102.7 mm² | **0.90** |
   | 40 × 1 thin wall | 39.9 mm² | 39.0 mm² | **0.98** |
   | radius 15 ring, 1 mm wall | 72.9 mm² | 448.0 mm² | 6.15 |
   | radius 10 disc | 70.9 mm² | 1809.8 mm² | 25.5 |

   Straight-sided and thin shapes agree within 10%. Round ones overrun because a
   bead count cannot change along an edge yet: on a disc every medial arm carries
   the same three beads and they pile up where the arms converge, instead of each
   bead turning into a contour as the count drops. That is the transition stage.
   `crates/walls/tests/skeleton_parity.rs` holds those four numbers as a ratchet.

3. **Section beads — done, in `walls`, not in the skeleton crate.** Two experiments
   decided the design and both are cheaper to record than to repeat.

   **First: what a correct wall looks like was measured, not argued.** PrusaSlicer
   2.9.7 sliced the same shapes with `perimeter_generator = arachne` and `classic`,
   three 0.4 mm perimeters, no infill, no solid layers, and the four wall paths were
   measured off the G-code. On a radius-10 disc, Arachne's walls are three concentric
   rings with radii in 8.65–10.2 mm and a total perimeter length of 359 mm — and
   classic's are the same length to the millimetre, and the same 143.4 mm² of bead.
   Arachne does not print a round part's walls along the medial arms; the arms carry
   the width and the count, and the *paths* are the level sets of the distance to the
   boundary. The 25.5× overrun in the table above is not a missing transition, it is
   an emitter walking the wrong curve.

   **Second: a per-vertex width rule bolted onto the offset ladder makes it worse.**
   Measuring the section from each loop vertex's own normal, dividing the section
   evenly among the beads it holds, and nudging the vertex to where that division puts
   it fixes 1.0–1.6 mm sheets (a 1.0 mm wall goes from one 0.4 bead plus a strand to
   three 0.333 beads) and degrades everything else: on the plate fixture it took bead
   area from 2,114 to 2,535 mm², strands from 108 to 116, and gap time from 135 to
   1,896 ms, because a ring's vertex normals are not a section at a corner and a band
   claimed at its narrowest width under-claims coverage. Reverted, not shipped.

   What landed instead is `crates/walls/src/beads.rs`: beads as level sets of the
   distance to the layer's boundary, with the width divided from the section measured
   per grid cell. Each cell measures its section — distance to the surface plus the
   distance onwards to the far side, marched along the inward normal — and the section
   decides how many beads it holds and how wide they are; bead `k` is the zero level of
   `depth − centre_k` and `contour::trace` carries the width through the crossing.
   `Mode::Hfw` is this generator; `Mode::Classic` keeps the ladder so the two can be
   compared on the same layer.

   Wall area against the ladder's, same profile:

   | shape | ladder | section beads | ratio |
   |---|---|---|---|
   | 30 × 20 rectangle | 114.2 mm² | 113.9 mm² | **1.00** |
   | radius 10 disc | 70.9 mm² | 70.7 mm² | **1.00** |
   | radius 15 ring, 1 mm wall | 72.9 mm² | 87.5 mm² | 1.20 |
   | V shape | 72.7 mm² | 72.6 mm² | **1.00** |
   | 40 × 2 sheet | 64.6 mm² | 64.4 mm² | **1.00** |
   | 40 × 1.4 sheet | 63.7 mm² | 55.5 mm² | 0.87, and it covers 55.5 of the layer's 56 mm² |

   The disc and the ring are the two entries this stage existed to move: 25.5 → 1.00
   and 6.15 → 1.20, where the 1.20 is not an overrun but the ladder under-filling a
   1 mm wall. The 1.4 mm sheet is the honest remainder: a section holding an odd number
   of beads has a middle bead on the section's own midline, which is a ridge of the
   field rather than a level set, so it is not traced and its strip is still a strand.

   On the plate fixture, `Mode::Hfw` went from 108 and 82 gap strands on the two thick
   layers to **0 and 0**, with bead area 2,114 → 2,000 mm² and 2,027 → 1,952 mm², at
   1,019 ms against 995 ms for the heaviest layer. The engine's total deposit moved with
   it, closer to the references: cube 1.0070 → 1.0062, sphere 1.0058 → 1.0014, torus
   1.0232 → 1.0093 of the shape's own volume, against OrcaSlicer's 1.0002, 1.0005 and
   1.0022.

   The middle bead is the remaining work, and it is a bead whose path is the medial
   ridge of a thin section — the one thing the skeleton emitter does well. Measured a
   second time on the plate at 3 walls, though, it is not volume but arithmetic: 97
   strands of 492 mm total on 688 m of wall, median 2.2 mm long, all of them the
   sub-bead sliver the division leaves rather than a bead that failed to trace.

   **The part, not the fixtures.** The Revolver plate was sliced in both modes — 18
   objects, 450 layers at 0.2 mm, 2 walls of 0.4 — and the wall paths read back out of
   the G-code. Two defects were only visible there.

   First, the depth field was not total: a cell further than the band's search radius
   from any segment, and not inside the offset that marks the deep interior, kept the
   default depth of zero. Just past the radius that is a razor-thin ring of cells read
   as neither material nor air, and the tracer closed a loop around it. The torus
   fixture carried 0.8% of extra material from exactly that. Now every cell is
   positive inside the layer and negative outside it, at the search radius where the
   measurement runs out.

   Second, a spot where the section briefly deepens past a bead centre — a soft crease,
   a merged fillet, the crease of a screw boss — closes a three-vertex loop around
   itself. Each one is a 0.3 mm dab printed on material already there: the laps and caps
   visible on the drum. The floor is now three beads of path (`MIN_BEAD`), which is
   about one bead's worth of enclosed area, and a nib smaller than that cannot be
   printed as a loop in any case.

   Wall paths per plate, read from the G-code:

   | | wall length | paths | under 2 mm | dots (bbox < 0.6 mm) |
   |---|---|---|---|---|
   | ladder (`Classic`) | 455.4 m | 11,299 | 786 (7.0%) | 559 |
   | field beads, first cut | 438.8 m | 12,876 | 1,864 (14.5%) | 1,099 |
   | field beads, total field | 438.0 m | 12,406 | 1,572 (12.7%) | 889 |
   | field beads, total field + floor | 437.6 m | 11,625 | 791 (6.8%) | **108** |

   Same wall material throughout (71.5 g of perimeter either way), so this is not a
   coverage trade: it is 1,099 dabs removed and nothing else given up.
4. **Post-processing and integration** — done inside the tracer rather than after it:
   the [`MIN_BEAD`] floor drops the dabs a stitch-and-trim pass would have removed,
   `static` contours come out of the same level sets, and `--mode hfw | classic`
   (`Mode`) has been the flag since before the generator existed. The oracle was
   re-baselined: cube 1.0062, sphere 1.0014, torus 1.0013 of the shape's own volume.
5. **Retire the gap pass** — done. The ladder keeps its loops for `Classic` mode, and
   `gap_fill` survives only as the fallback inside the field generator for the strip a
   level set cannot express (an odd bead count's middle, whose centre is the section's
   midline and so a ridge). Measured, that is not work left over: on the Revolver plate
   at the profile's 3 walls it is **97 strands totalling 492 mm on 688 m of wall
   (0.07%)**, median length 2.2 mm; at 2 walls, 37 strands and 30 mm. Nothing on either
   plate is a missing bead — the strands are the sub-bead slivers the division leaves.

Rough size: 2,500–3,000 lines of Rust plus tests, against 5,500 of C++ — smaller
because `f64`, arenas and `Result` remove the defensive plumbing, not because any
stage is skipped.

## Verification

- Existing: `cargo test --workspace`, the four generator `--check` runs,
  `scripts/oracle.py --density 1.0`, and the plate benchmark.
- **wall area against the ladder's**, per shape, which is
  `crates/walls/tests/field_beads.rs`: 1.00 on a rectangle, 1.00 on a disc, 1.00 on
  a V, 1.20 on a 1 mm ring where the ladder under-fills. The knee is
  `crates/walls/tests/skeleton_parity.rs`, which measures the skeleton emitter that
  the middle bead will come from: 6.15 on the ring and 25.5 on the disc, because it
  walks the medial arms.
- **short bead count**: strands under 1.5 mm per layer, currently 78% of the
  ladder's gap pass. `emit` already drops beads shorter than one bead width,
  which is the ladder's missing step.
- **remainder slivers**: covered area minus the union of bead bands, per layer,
  which should approach zero on smooth outlines instead of being paid off in
  strands.
- Visual: the Revolver plate's drum wall is the acceptance case — the laps are
  visible to the naked eye at 0.3 mm layers, so a reslice is the check.
- Print: `docs/TESTING-BY-HAND.md` already names the calibration parts; a 20 mm
  cylinder and the revolver drum go on it.

## Licensing

Arachne is AGPL-3.0 (CuraEngine, and PrusaSlicer's copy too); this tree is
MIT OR Apache-2.0. Reading the source to understand the algorithm is one thing;
translating it file by file into Rust makes a derivative work and would drag the
licence along. The algorithm itself is published — Kuipers, Doubrovski, Wu, Wang,
*"A framework for adaptive width control of dense contour-parallel toolpaths in
fused deposition modeling"* (2020), which `SkeletalTrapezoidation.hpp` cites — and
that paper, plus the behaviour observed in the reference slicers, is the source to
implement from. Function names and constants quoted above are for orientation and
comparison, not a translation guide.

`boostvoronoi` is BSL-1.0: permissive, compatible with this tree's licence, no
copyleft, attribution in `THIRD-PARTY.md`.

## Risks

- **Speed**: measured, not estimated — 130 ms for the heaviest plate layer's
  skeleton and under 100 ms for `emit` over four layers, against a whole plate
  that slices in 12.9 s. One diagram per part in parallel is the remaining lever,
  and the wall phase runs under the same per-layer parallelism the rest of the
  pipeline does.
- **Numerical fragility**: Voronoi of segments is where slicers crash. Upstream's
  stage 1 exists entirely for this. Port it first and do not "simplify" it.
- **Determinism**: float ties in the graph walk must break by index, or the same
  plate prints differently twice.
- **Regression surface**: every print's wall structure changes at once. `--mode
  classic` and the oracle keep the comparison honest while that lands.
