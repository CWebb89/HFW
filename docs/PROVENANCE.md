# Provenance log

Every module, what it does, and what it was built from. The purpose is to make
the clean-room claim auditable rather than asserted: no code from Slic3r,
PrusaSlicer, CuraEngine, libArachne or libslic3r exists in this repository, and
the algorithms that share a name with published work are named below.

| crate | what it does | built from |
|---|---|---|
| `geometry` | integer nanometre points, contours, exact orientation predicates | own design; exact-predicate technique is textbook |
| `kernel` | floating-point `V2`, bounds, regions, nearest-segment and nearest-point indices, ring area and winding | textbook computational geometry; spatial hashing is standard |
| `mesh` | STL parse, welding, topology diagnostics, repair, transforms | STL format description; triangle-plane intersection is standard |
| `slicing` | layer schedule, marginal plane-triangle intersection, segment deduplication, contour assembly | own design |
| `walls` | beads as level sets of the distance to the layer's boundary, widths divided from the section measured along the inward normal, marching-squares tracer carrying per-vertex width, polygon offset ladder with miter joins, tape-ordered bead footprints, gap pass by inward-normal ray, morphological opening for the feature floor, width-aware Douglas–Peucker, raster adhesion brim | level sets of a distance field are the standard construction for a contour-parallel path, and the section division is Wu, Kuipers, Doubrovski and Wang (2020) applied per section rather than per medial edge; the ladder's offset-through-union structure follows Clipper2; Douglas & Peucker 1973 for the simplifier; the tracer, the gap pass and the coverage bands are own design (`docs/WALLS-SKELETON.md`, `SLICE-SPEED-STATUS.md` §The wall rewrite) |
| `skeleton` | Voronoi of the layer outline's own segments walked into a medial-axis graph; nodes carry the distance to the boundary, edges carry discretized geometry and the layer's section at every sample, each edge is marked central or not, and beads are laid along the edges and joined at their ends | the algorithm published by Kuipers, Doubrovski, Wu and Wang (2020), *A framework for adaptive width control of dense contour-parallel toolpaths in fused deposition modeling*, and the structure any skeletal wall generator needs; the central-edge test is that paper's width-to-length angle test, restated in `docs/WALLS-SKELETON.md`. The parabolic-edge sampling is derived here from the definition of a parabola, and the section measurement from sphere tracing, not taken from a source |
| `infill` | skin masks from a bounded ring of layer masks; line families clipped by a distance query | own design (`DESIGN.md` §7c) |
| `toolpath` | planned layers, per-feature counters, coverage and spill metrics, SVG export | own design |
| `config` | `key = value` printer profiles with validation | own design; the setting names follow slicer conventions so profiles are portable |
| `gcode` | path ordering, seam placement, retraction, z-hop, per-segment extrusion from bead width, speeds, cooling, skirt, placement; independent verifier | public G-code command sets (Marlin and Klipper user documentation); the bead volume model subtracts the two corners a wall bead leaves unfilled, `h²(1 − π/4)`, which is the correction both reference slicers apply to perimeter flow |
| `engine` | job orchestration across the pipeline | own design |
| `cli` | headless command-line front end | own design |

Arachne (Kuipers et al. 2020) is the published algorithm `skeleton` implements. Its
structure and constants were compared against a reference implementation of that
algorithm for parity, and no code was translated from it: the reference is
AGPL-3.0 and this tree is MIT OR Apache-2.0, so the boundary is deliberately the
published description plus the behaviour a slicer has to match, not the C++ text.
Slic3r and PrusaSlicer are named only as the thing this engine is measured
against in `docs/ORACLE.md` and as the origin of the original idea of a slicing
engine in this repository.

## Third-party crates

| crate | licence | used for |
|---|---|---|
| `i_overlay` | MIT OR Apache-2.0 | the exact integer polygon boolean and offset in `kernel::offset` |
| `boostvoronoi` | BSL-1.0 | the Voronoi diagram of the outline's segments in `skeleton::build`; a port of Boost.Polygon's algorithm, which is the same one the reference slicers build on |
| `serde`, `toml` and the rest of the tree's dependencies | permissive | serialization and parsing only; `Cargo.lock` is the complete list |

## How the claim was checked

- No file in this repository contains a translation of a C++ source file from
  any of those projects; the data structures, module boundaries and algorithms
  are documented in `DESIGN.md` and `MIGRATION.md`.
- The G-code command set is a public interface, not a protected work: `G1`,
  `M83`, `M104` and the rest are documented commands, and every slicer emits
  them.
- Inspection can only report absence of derivation; it cannot prove that a
  generated codebase contains no memorised fragment. That residual risk is
  inherent to any AI-assisted codebase and is stated rather than hidden.

Written by an AI model directed by the project owner, November 2026. Human
review: pending.

## Audit, 2026-11-19: Arachne identifiers and structure

Append-only. The identifiers the audit looks for are the distinctive ones from
CuraEngine's Arachne, which is AGPL-3.0 and reaches this tree through OrcaSlicer:

```
SkeletalTrapezoidation updateIsCentral filterCentral filterOuterCentral
propagateBeadingsUpward propagateBeadingsDownward generateTransitionMids
generateTransitioningRibs generateTransitionEnds applyTransitions BeadingStrategy
DistributedBeadingStrategy LimitedBeadingStrategy RedistributeBeadingStrategy
stitchToolPaths removeSmallLines separateOutInnerContour discretization_step_size
transition_filter_dist
```

**Code**: `grep -rn '<those names>' crates` returns nothing. No C++ identifier,
no upstream comment and no upstream constant table appears in any Rust file.

**Docs**: all matches are in `docs/WALLS-SKELETON.md` (lines 44–78 and 119, 349),
which is the design note for `crates/skeleton` and states plainly that its stage
list was read from `~/dev/reference/OrcaSlicer/src/libslic3r/Arachne` and follows
`WallToolPaths::generate` in order.

**Structure**, compared by hand, `crates/skeleton/src` against that list:

| upstream stage | this tree | judgement |
|---|---|---|
| prepare the outline | `build::prepare` | generic (offset out/in, simplify, union) |
| build the skeletal trapezoidation | `build::build` → `graph` | derived structure: same half-edge/twin arena, same three edge classes (`boundary`, `rib`, `central`) |
| classify central edges, filter | `classify::classify` | derived structure: same two filters in the same order |
| bead count per location | documented, not implemented | — |
| beading strategies composed | `bead` module (`Widening`, `Distributed`, `Limited`) | derived structure: same composition order |
| transitions | absent (`docs/WALLS-SKELETON.md` error, see below) | — |
| toolpaths | `emit` | derived structure |
| post: stitch, drop short lines | `emit` | derived structure |

**Error found in the note**: `docs/WALLS-SKELETON.md` lists
`transition.rs` in the planned layout, and `crates/skeleton/src/transition.rs`
does not exist. The note describes a stage the crate does not have.

**Judgement**: `crates/skeleton` is a derived work in structure, not in text. The
`SkeletalTrapezoidation` half-edge/twin graph is not a coincidence of subject
matter; the stage decomposition and its order are the upstream algorithm's. It is
**not used by any shipped wall path** (`hfw_walls::Mode::Hfw` is the polygon
ladder, `Mode::Field` the raster), so nothing in a released slice depends on it.

**Recommendation (for the owner, not decided here)**: delete `crates/skeleton` and
the design note's stage list, and let the variable-width work be the P2.6
redistribution design, which needs no Voronoi and no upstream structure. The
alternatives and their consequences are in `DECISIONS.md` DECISION-6.

## Audit, 2026-11-19: infill and support

Same grep over `crates/infill` and `crates/support` for PrusaSlicer's fill and
support identifiers (`FillRectilinear`, `ExtrusionEntityCollection`,
`SupportMaterial`, `TreeSupport`, `Slic3r::`): no matches. The pattern names in
`crates/infill` are the public names of the patterns themselves (`grid`, `gyroid`,
`honeycomb`), which are geometry, not code. No tuning table in either crate is
attributed to upstream; the numbers there are either physics or measured against
`docs/ORACLE.md`.
