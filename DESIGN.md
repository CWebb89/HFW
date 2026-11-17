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
  |  hfw-walls: distance field -> medial axis -> bead decomposition -> iso-lines
  |             -> Newton projection -> corner recovery -> simplification
  v
perimeter and middle-bead paths with per-vertex width
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

## Why variable-width walls

A classic slicer walks a fixed ladder of offsets `0.5w, 1.5w, 2.5w ...` from
the boundary. Where the local feature width is not an odd multiple of `w`, the
innermost loop either overlaps its neighbour or leaves a gap. Thin features
worse than that: a 0.3 mm rib with a 0.4 mm nozzle gets no loop at all, because
no `k` produces a legal offset.

This engine measures the local inscribed radius instead and decomposes it into
whole beads. The decomposition is symmetric, so an odd bead count keeps a bead
straddling the centreline; adjacent radii blend over an arc length and a
hysteresis band keeps the count from oscillating. The result is a gapless
ladder from the boundary to the centreline, including features narrower than one
extrusion width.

Side loops are iso-lines of `d(x) - l_k` where `d` is the exact distance to the
nearest boundary segment and `l_k` is the target radius of loop `k`. Thin regions
replace `l_k` with the centre of bead `k` of the local decomposition, which is
why the innermost loop can move off the nominal ladder.

## Module responsibilities

| Crate | Owns | Must not know about |
|---|---|---|
| `hfw-geometry` | integer coordinates, contours, exact predicates | meshes, layers, infill |
| `hfw-kernel` | float vectors, regions, spatial indices | slicing, walls, output |
| `hfw-mesh` | triangles, STL, topology, repair | layers, toolpaths |
| `hfw-slicing` | layer schedule, plane intersection, contour assembly | walls, infill, output |
| `hfw-walls` | distance field, medial axis, bead geometry | infill, export |
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
