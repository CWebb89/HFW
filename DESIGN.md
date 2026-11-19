# Design

## Pipeline

```
STL bytes
  |  hfw-mesh: parse, weld exact duplicate vertices, diagnose topology
  v
Mesh (integer nanometres)  ------------> diagnostics, opt-in repair
  |  hfw-slicing: layer schedule, per-layer triangle buckets
  v
Contour per layer (integer nanometres, closed, wound by nesting depth)
  |  hfw_engine::region_from_contours      <- the only domain boundary
  v
Region (millimetres, float)
  |  hfw-walls: distance field -> section beads -> gap fallback -> simplification
  v
perimeter loops and measured-width gap strands, each with per-vertex width
  |  hfw-infill: skin masks, wall-clipped sparse and solid fill
  v
PlannedLayer (paths + reference outline + counters)
  |  hfw-toolpath: metrics, SVG, manifest
  v
SVG / CSV / report
```

## Two number domains, one boundary

Mesh work is exact. STL coordinates become integer nanometres once, every
predicate after that is integer arithmetic, and contour reconstruction is
deterministic. Open and branching cross-sections are errors, not guesses.

Measured work is approximate. Distance fields, medial axes, offsets and
infill clipping are floating point millimetres. Converting exact input to float
at one place keeps the exactness claim honest: integer arithmetic never leaks
into the field solver, and float error never leaks back into the mesh.

`region_from_contours` is that one place. Coordinates divide exactly by 10^6,
so the only error introduced is the float conversion itself.

## Walls: beads divided from the measured section

A wall runs parallel to the surface, so its path is a level set of the distance to the
surface: loop `i` sits at the depth bead `i`'s centre belongs at. What decides the
width is the layer's own section, not the profile's nominal bead. Each cell of a
sampling grid measures the section it sits in — its distance to the surface plus the
distance onwards to the far side, marched along the inward normal — and the section
decides how many beads it holds and how wide they are. A 1.4 mm wall holds three
0.467 mm beads; a 20 mm section holds the profile's walls at the nominal width and
leaves its middle to the infill. Each bead is then the zero level of `depth − centre`
on that grid, and the tracer carries the width measured where the bead crosses a cell
boundary, so the output is a variable-width extrusion and the writer derives each
segment's flow from the segment's own width; no firmware support is needed.

The division applies only where the section is narrow enough to need it. A wide section
uses the widths the profile asks for, and those are cumulative the way the ladder's are:
`outer_wall_width` widens bead 0 and pushes every bead behind it in by the difference,
and the count is taken from those widths rather than from the nominal one, because
0.62 + 0.4 + 0.4 does not fit in a 1.2 mm section. The profile's `resolution` is the
tolerance the traced rings are simplified at, floored by what the sampling grid can
express.

This is what a ladder cannot do. A fixed-width loop leaves a strip behind in any
feature whose width is not a whole number of beads, and in a feature narrower than the
ladder the loops stop early; the strip used to be paid off by a gap pass, one strand
down the middle of it, and the strands are what read as laps on a printed wall. The
section division covers the feature with the beads it actually holds instead.

Two things stay from the ladder. `Mode::Classic` still generates it, so the two can be
compared on the same layer — `--mode hfw | classic`. And the gap pass survives as a
fallback inside the field generator, for the one case a level set cannot express: a
section holding an odd number of beads has a middle bead whose centre is the section's
own midline, which is a ridge of the field rather than a level set. Measured on the
Revolver plate, that strip is no longer a defect to chase: 97 strands totalling 492 mm on
688 m of wall at the profile's 3 walls, median 2.2 mm long — the sub-bead sliver the
division leaves, not a bead that failed to trace. At 2 walls it is 37 strands and 30 mm.

Measured against the references on `fixtures/thin_wall.stl`, a single 0.3 mm rib of
90.0 mm³: this engine lays 76.3 mm³ where OrcaSlicer and PrusaSlicer lay 100.8.
Part of that is policy — the references substitute a 0.4 mm bead for a sub-nozzle
feature and deposit 112% of the rib's own volume. The rest is this engine's own
bookkeeping, and it is a defect: the rib is one `Gap` strand per layer, 24.7 mm
long over 60 layers, carrying 71.24 mm³ where the rib's own volume is 90.0 mm³.
Three things take the 21%:

- `bead_area` subtracts the corner area a free-standing oval bead does not fill,
  `h²(1 - pi/4)` = 0.0086 mm² per millimetre at 0.2 mm layers. A strand filling a
  slot has those corners already: its neighbours are the slot's sides. That is
  14% of the rib's volume, withheld by construction.
- The strand is trimmed half a bead at each end, 24.7 mm of a 25 mm rib.
- The measured width comes out 0.287 mm rather than the rib's 0.3.

The first is the one worth fixing and it is not Arachne: a `Kind::Middle` strand
fills a slot, so its area is a rectangle, and the corner correction belongs to
free-standing beads only. That is the first task in the redistribution work below.

`fixtures/taper_wall.stl`, which sweeps the band width from 0.6 mm to 3.0 mm, is
-9.1% against the same two references. It is the same question as a step function:
at each width the ladder either lands on a bead multiple or leaves a sliver.

What a skeletal generator (Arachne, Kuipers et al. 2020) adds beyond this, and this
engine does not do:

- **Redistribution.** Where the band width is not an integer multiple of the bead,
  Arachne widens the existing loops to share the remainder — a 1.0 mm wall with two
  loops becomes 0.5 + 0.5 — instead of printing 0.4 + 0.4 and squeezing a 0.2 mm
  strand between them. The strand is legal here because the floor is half a bead
  (`Params::min_feature.max(0.5 * w)`); anything narrower than that is left as a
  void, which is the mechanism behind the residue on curved parts.
- **Transitions.** Bead count changes across an arc length with hysteresis, so a
  bead starts and stops gradually rather than appearing at a defined point.
  `Params::distribution` already spreads a width difference over the loops with the
  same parabolic weighting, so redistribution is a change to how the widths are
  chosen, not new geometry machinery.

Redistribution is the cheap half of Arachne and it is on the work order
(`docs/STATUS.md`); skeletal trapezoidation is not.

## Module responsibilities

| Crate | Owns | Must not know about |
|---|---|---|
| `hfw-geometry` | integer coordinates, contours, exact predicates | meshes, layers, infill |
| `hfw-kernel` | float vectors, regions, spatial indices | slicing, walls, output |
| `hfw-mesh` | triangles, STL, topology, repair | layers, toolpaths |
| `hfw-slicing` | layer schedule, plane intersection, contour assembly | walls, infill, output |
| `hfw-walls` | distance field, beads divided from the measured section, classic ladder, gap fallback, brim | infill, export |
| `hfw-infill` | skin masks, sparse and solid fill | walls beyond their band |
| `hfw-toolpath` | path assembly, metrics, SVG/CSV | mesh, slicing |
| `hfw-engine` | the pipeline, the seam, job settings | CLI parsing |
| `hfw-cli` | argument parsing and reporting | geometry |

Dependencies point down that table, never up. Two third-party crates are used, and
only at the edges where the standard library has no equivalent:
`zip` (deflate only, `crates/mesh` — reading a 3MF container) and `regex`
(`crates/gcode` — `gcode_substitutions` and the dialect's `=~` matcher). No crate
depends on them transitively, and the geometry, slicing and toolpath crates are
standard library only.

## Invariants

- Every `PlannedLayer.paths` entry has `pts.len() == width.len()`.
- A closed path does not repeat its first vertex.
- Reference outlines are never counted as extruded material.
- Wall paths stay inside the part grown by one extrusion width.
- Slicing is deterministic; beam-style reordering would need a stable sort key,
  so parallel slicing is still an open task rather than a free win.
- Coverage metrics use a jittered grid so axis-aligned model edges do not land
  on sample points, and the sampling window is the part grown by one width, so
  spill is measured just past the outline and not further out.

## Failure policy

| Condition | Behaviour |
|---|---|
| mesh has boundary edges | reported in diagnostics before slicing; repair only with `--repair-basic` |
| a layer has an open or branching cross-section | detected, layer skipped and reported; `--strict` makes it fatal |
| cross-section too thin to print | layer keeps its reference outline, produces no paths |
| part too large for the field at the chosen resolution | hard error naming the cell count and the limit |
| infill density 0 with zero skin layers | infill stage skipped, walls still generated |

## Cost

Wall generation dominates: the field is `O(cells)` nearest-segment queries on a
grid of `part size / (w/8)`. Measured on one core, 0.4 mm nozzle, 0.2 mm layers,
walls plus 15 % infill:

| model | layers | wall time | peak RSS |
|---|---:|---:|---:|
| 20 mm cube | 100 | 3.7 s | 13 MB |
| 30 mm sphere, 16 128 triangles | 150 | 8.5 s | 27 MB |
| torus | 40 | 3.4 s | 31 MB |

Slicing itself is negligible by comparison: the sphere's 150 layers take about
50 ms including bucketing. `--every N` is the escape hatch that trades visual
continuity for time, and is not valid for material accounting.
