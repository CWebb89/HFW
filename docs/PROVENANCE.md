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
| `walls` | distance field, medial-axis ridge extraction, spur pruning, bead-count hysteresis, iso-line tracing, Newton projection, corner recovery, width-aware Douglas–Peucker | medial-axis literature as a concept; Douglas & Peucker 1973 for the simplifier; the beading scheme is own design (`DESIGN.md` §2) |
| `infill` | skin masks from a bounded ring of layer masks; line families clipped by a distance query | own design (`DESIGN.md` §7c) |
| `toolpath` | planned layers, per-feature counters, coverage and spill metrics, SVG export | own design |
| `config` | `key = value` printer profiles with validation | own design; the setting names follow slicer conventions so profiles are portable |
| `gcode` | path ordering, seam placement, retraction, z-hop, per-segment extrusion from bead width, speeds, cooling, skirt, placement; independent verifier | public G-code command sets (Marlin and Klipper user documentation); the bead volume model subtracts the two corners a wall bead leaves unfilled, `h²(1 − π/4)`, which is the correction both reference slicers apply to perimeter flow |
| `engine` | job orchestration across the pipeline | own design |
| `cli` | headless command-line front end | own design |

Arachne (Kuipers et al. 2020) is known only as the idea of variable-width walls.
Its skeletal trapezoidation, bead-counting rule and implementation details are
not used. Slic3r and PrusaSlicer are named only as the thing this engine is
measured against in `docs/ORACLE.md` and as the origin of the original idea of a
slicing engine in this repository.

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
