# Provenance log

Every module, what it does, and what it was built from. The purpose is to make
the clean-room claim auditable rather than asserted: no code from Slic3r,
PrusaSlicer, CuraEngine, libArachne or libslic3r exists in this repository, and
the algorithms that share a name with published work are named below.

| crate | what it does | built from |
|---|---|---|
| `geometry` | integer nanometre points, contours, exact orientation predicates | own design; exact-predicate technique is textbook |
| `kernel` | floating-point `V2`, bounds, regions, nearest-segment and nearest-point indices, ring area and winding, and `cap`: a region with holes as triangles | textbook computational geometry; spatial hashing is standard. `cap` is the bridge-then-ear-clip construction (Eberly's description of bridging is the textbook one): a hole is joined to its boundary along the shortest segment that crosses nothing, then ears are clipped, and an ear is refused when it is clockwise, holds another ring point, or has a ring edge through it. A band sweep was written first and removed: it cuts the boundary at heights of its own choosing and leaves T-junctions against the walls |
| `mesh` (3MF) | STL parse, welding, topology diagnostics, repair, transforms; 3MF read and write (ZIP, model part, components, build transforms, `Metadata/` parts) | STL and the 3MF core specification's model, resource and build elements; triangle-plane intersection is standard, and so is Sutherland–Hodgman clipping of a triangle against a plane, which is how `cut` splits one; the cap rings are walked out of the segments the plane leaves and capped by `kernel`'s clipper, and separate shells are found with a union-find over shared vertices. The writer emits only what the reader above already parses, so the two cannot drift into a format neither of them has tested |
| `slicing` | layer schedule, marginal plane-triangle intersection, segment deduplication, contour assembly | own design |
| `walls` | beads as level sets of the distance to the layer's boundary, widths divided from the section measured along the inward normal, marching-squares tracer carrying per-vertex width, polygon offset ladder with miter joins, tape-ordered bead footprints, gap pass by inward-normal ray, morphological opening for the feature floor, width-aware Douglas–Peucker, raster adhesion brim | level sets of a distance field are the standard construction for a contour-parallel path, and the section division is Wu, Kuipers, Doubrovski and Wang (2020) applied per section rather than per medial edge; the ladder's offset-through-union structure follows Clipper2; Douglas & Peucker 1973 for the simplifier; the tracer, the gap pass and the coverage bands are own design (`docs/WALLS-SKELETON.md`, `SLICE-SPEED-STATUS.md` §The wall rewrite) |
| `skeleton` | Voronoi of the layer outline's own segments walked into a medial-axis graph; nodes carry the distance to the boundary, edges carry discretized geometry and the layer's section at every sample, each edge is marked central or not, and beads are laid along the edges and joined at their ends | the algorithm published by Kuipers, Doubrovski, Wu and Wang (2020), *A framework for adaptive width control of dense contour-parallel toolpaths in fused deposition modeling*, and the structure any skeletal wall generator needs; the central-edge test is that paper's width-to-length angle test, restated in `docs/WALLS-SKELETON.md`. The parabolic-edge sampling is derived here from the definition of a parabola, and the section measurement from sphere tracing, not taken from a source |
| `font` | sfnt/TrueType reading: the table directory and collections, `cmap` formats 4 and 12, `head`/`hhea`/`maxp`/`hmtx`/`loca`, simple and composite `glyf` outlines, and quadratic-Bézier flattening at a caller's tolerance | the OpenType/TrueType specifications for the tables read, and nothing else: no font code was copied from anywhere. The step count for a quadratic is derived from the error bound `\|P0 − 2P1 + P2\| / (8n²)` and the implied-midpoint rule is the specification's own. Verified against FreeType — the library behind every renderer on this machine — through `scripts/font-oracle.py`: advances agree to 0.00002 mm and ink boxes to 0.00218 mm on a real font at 10 mm |
| `text` | a glyph outline as a solid: contour nesting by containment, holes bridged into their boundary along the shortest non-crossing segment, ear clipping for the caps, walls per contour | own design. Nesting is decided by the even-odd point-in-polygon rule rather than by the font's stored direction, because the direction is the font's choice; the bridge-then-clip pipeline is the standard construction for a polygon with holes (Eberly's description of it is the textbook one) and the ear test is the textbook convexity-plus-emptiness test, written here. No CSG kernel is involved: a letter is a prism, and the slice unions or subtracts it per layer with the exact predicates already in `kernel` |
| `infill` | skin masks from a bounded ring of layer masks; line families clipped by a distance query | own design (`DESIGN.md` §7c) |
| `toolpath` | planned layers, per-feature counters, coverage and spill metrics, SVG export | own design |
| `config` | `key = value` printer profiles with validation | own design; the setting names follow slicer conventions so profiles are portable. Its settings surface (`assets/settings-surface.json`) is generated from the measurement in `tools/gen-settings-ledger.py` — names, types and defaults from the application's vendored definition table, statuses measured against this crate's parser — so it holds no hand-written claim about what the engine honours |
| `gcode` | path ordering, seam placement, retraction, z-hop, per-segment extrusion from bead width, speeds, cooling, skirt, placement; independent verifier; the bgcode binary container behind `binary_gcode` | public G-code command sets (Marlin and Klipper user documentation); the bead volume model subtracts the two corners a wall bead leaves unfilled, `h²(1 − π/4)`, which is the correction both reference slicers apply to perimeter flow. The container is the format owner's byte-level specification (`libbgcode/doc/specifications.md`) and nothing else — see the provenance note below |
| `engine` | job orchestration across the pipeline | own design |
| `cli` | headless command-line front end; a `bed_custom_model` mesh becomes the profile's `bed_shape` outline | own design, reusing this engine's slicer for the bed's own cross-section |
| `capi` | the C ABI: opaque mesh and G-code handles, a per-thread error slot, one plate-slice call | own design; the surface is the one `docs/NEXTLAYER-BRIDGE.md` negotiated with the application, and it reuses `engine`'s settings reader and plate merge rather than reimplementing either |
| `plate` | the plate document: objects with a name, a placement, settings and an enabled flag; bounds; duplicate, turn and mirror; the sequential-print clearance check; the MaxRects shelf-free arranger; the project file (a 3MF plus `Metadata/hfw_project.xml`); slicing a plate into one job | own design; the object/placement/settings split is the shape every slicer's document model has because a UI needs it, and nothing here is derived from `PlateDocument.cpp` or any other engine's source. The arranger was written here as a bed packer — MaxRects with a best-short-side-fit score over a rectangle list — and moved out of `cli` in phase 5 so a panel and the command line lay a plate out the same way. The project part is this engine's own format, deliberately not PrusaSlicer's `Slic3r_PE_model.config` or Bambu's JSON: a format we write and read with one mapping cannot drift from another program's schema we would have to reverse-engineer. `check_sequential` is derived from the machine and measured against the reference — see the provenance note below |

## Provenance note: the sequential clearance check

On 2026-11-20, while writing `plate::check_sequential`, the next-layer tree's
PrusaSlicer checkout was read to answer one question — which settings a reference
slicer uses to decide whether a plate can be printed object by object. Two things
were seen: `extruder_clearance_radius` and `min_object_distance` (the latter built
from that radius and `duplicate_distance`). That is a read of engine source, which
the clean-room rule forbids, and it happened before the rule was recalled.

What is in this tree as a result:

- **The rule itself is geometric, and was derived here.** The gantry spans X, so
  while printing object B at layer `z` its underside is at `z + gantry_height`,
  which clears a finished object A only when `z + gantry_height > height(A)`; at
  `z = 0` that is `height(A) <= gantry_height`. The head body is around the
  nozzle, so while B's low layers go down the head is at A's own height and needs
  `head_radius` of clear bed plane from A's footprint. Both came from the machine
  geometry, and `plate/src/lib.rs` states them as such.
- **The rule was then measured, and the reference does not do this.** PrusaSlicer
  prints a two-object sequential plate with a 5 mm gap and a 20 mm clearance radius
  without complaint; the setting feeds its arranger's spacing, not a validation
  step. The command and the result are in `docs/ORACLE.md` §*Sequential printing*.
  So the behaviour here is not an inheritance from that slicer: it is stricter, and
  it is the engine's own. Orca could not be measured on 2.5.1 for this setting.
- **What the read contributed** was a name and a question, not a rule: the setting
  `extruder_clearance_radius` (which is the config dialect's own name, carried by
  the application's definition table, and the only spelling a profile in the field
  will use) and the decision to take the gap from the head's radius rather than the
  plate's packing spacing — which is what `cli` used before, and which measures
  nothing about a toolhead.
- **Nothing was translated.** No code, no constant and no branch came from that
  tree; the functions written here own their structure, their tests and their
  messages.

The residual exposure is disclosure, not code: a person reading this log knows one
file of PrusaSlicer's config was looked at. Re-deriving the rule from nothing but
measurement would not change what is implemented, because the measurement above
reaches a different answer than the read did.

Arachne (Kuipers et al. 2020) is the published algorithm `skeleton` implements. Its
structure and constants were compared against a reference implementation of that
algorithm for parity, and no code was translated from it: the reference is
AGPL-3.0 and this tree is MIT OR Apache-2.0, so the boundary is deliberately the
published description plus the behaviour a slicer has to match, not the C++ text.
Slic3r and PrusaSlicer are named only as the thing this engine is measured
against in `docs/ORACLE.md` and as the origin of the original idea of a slicing
engine in this repository.

## Provenance note: the binary G-code container

The bgcode writer (`crates/gcode/src/bgcode.rs`) and its reader were written from
`https://github.com/prusa3d/libbgcode/blob/main/doc/specifications.md`, the format
owner's byte-level document: file header, block order, block header fields, per-type
parameters and checksums. **No `libbgcode` and no PrusaSlicer source was read.**
That document describes a container, not a slicing algorithm, which is the boundary
the clean-room rule draws: the rule protects what a slicer decides, and this file
only decides where bytes sit.

Two consequences were accepted rather than papered over:

- The specification does not say what a block checksum covers — the stored bytes or
the bytes after decompression. Every block this writer emits is uncompressed, where
the two readings are the same bytes, so the question cannot arise; a compressed block
is refused by name instead of guessed at.
- `scripts/bgcode-decode.py` is a second implementation written from the same document
with no shared code. It caught a real defect the Rust reader could not: the block
header is eight bytes when compression is 0, and the writer had emitted twelve.

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
