# Roadmap

The order of work, so the next action is never a question. State as of
2026-11-17; the numbers here are regenerated, never typed by hand where a script
can produce them.

Related: `../next-layer/docs/ecosystem-status.md` for the verified state of both
projects together, `STATUS.md` for the plain-language summary, `ENGINE-PARITY.md` for the
gate list parity depends on, `next-layer/docs/rust-engine-plan.md` for the phases
that retire the C++ engine, `SETTINGS-LEDGER.md` for the per-setting
inventory (generated), `SETTINGS-GAPS.md` for what is left and what it costs
(generated).

## What this project is for

Two jobs, in this order:

1. **Be the engine next-layer ships.** That is the reason the project exists.
2. **Be a reference-quality slicer on its own**, so it can be checked by printing
   and compared against OrcaSlicer and PrusaSlicer without either being present.

The settings ledger exists to make job 1 measurable: next-layer's rule is that no
control may be shown for a setting the engine ignores, so the ledger has to be
true before the engines meet.

## Standing

**383 engine settings, 364 applicable, 299 applied (82%), 6 partial, 59 missing,
19 n/a.** Outside the buckets a one-extruder machine cannot use, 299 of 307 is
applied (97%).

**Finish line, restated 2026-11-17:** every row is *applied*, *n/a*, or *blocked
on hardware* with the reason written beside it. The old wording ("100% applied")
was unreachable while 57 rows wait for a toolchanger, and a finish line nobody
can reach stops being read.

## The second job: the crates that retire the C++ engine

Replacing libslic3r completely is not a settings problem, so it is not tracked in
the ledger. It is tracked by phase in `next-layer/docs/rust-engine-plan.md`, and
each phase lands as a crate here:

| Phase | Crate | What it is |
| --- | --- | --- |
| 1 | `crates/capi` | The C ABI: mesh handles in, slice out, so the application can hold geometry without a process |
| 2 | `crates/plate` | The plate model: object, mesh, transform, per-object settings, instances |
| 3 | — (extends `crates/config`) | The settings surface next-layer's panel reads, as JSON |
| 5 | — (extends `crates/mesh`, `crates/geometry`) | Load, transform, scale, rotate, shelf layout |
| 6 | — (extends `crates/mesh/threemf.rs`) | Writing a `.3mf` project, not only reading one |

These are additions to what is here, not a rewrite of it: the slicing path those
crates serve is the one already tested by the gate.

## The order of work

### 1. Preset intake finish line (done)

`--bundle` reads a real profile library and layers a job out of it, and the one
vocabulary it lacked is in: a machine preset's `machine_start_gcode` is templated
with `[bed_temperature_initial_layer_single]` and friends, and every placeholder is
now either substituted (a setting's value, from the preset first) or left for the
writer, which knows the part's extent and the layer. A token nothing can fill fails
the verifier with the token named rather than reaching the printer as text. Orca's
`printable_area` and `printable_height` are read too, so the machine preset's bed is
the machine's own.

### 2. Walls: skeletal beading, replacing the offset ladder (next)

The ladder leaves a sliver along the outer loop on rounded geometry and pays it off
as thousands of short gap strands — 5,224 of the Revolver plate's 6,663 start
within 0.6 mm of the outer wall — which is what reads as laps on a printed wall.
The fix is the Arachne shape of the problem: a skeleton from a Voronoi of the layer's
segments, bead widths that absorb the remainder instead of a separate strand, and
transitions between bead counts printed as ramps. The settings are already parsed
and unused; the writer already emits per-vertex widths. `docs/WALLS-SKELETON.md` is
the design, the measured defect it answers, and the five phases.

Phases 1 and 2 are done, with 27 tests in the skeleton crate and four more in
`crates/walls/tests/skeleton_parity.rs`. `crates/skeleton` builds the skeletal
trapezoidation of a layer (one diagram per connected part, a bounded simplifier in
front of it, 130 ms on the heaviest plate layer), measures the layer's section with
two marched rays per sample, turns that into beads, lays one polyline per bead,
joins them at the nodes and cuts anything that leaves the layer.

Against the ladder, wall area for the same profile is **0.90× on a rectangle and
0.98× on a thin wall**, but **6.2× on a ring part and 25× on a disc**: a bead count
cannot change along an edge yet, so where a round part's medial arms converge the
same wall is printed many times. That is the transition stage, and the parity table
in `docs/WALLS-SKELETON.md` is its acceptance test. The wall stage still runs the
ladder, so no print changed.

Phase 3 changed the wall stage. A measurement first: PrusaSlicer's Arachne prints a
disc's three walls as three concentric rings of exactly the same length as the classic
ladder's, so wall *paths* are level sets of the distance to the boundary and the
medial axis carries the width, not the path. `crates/walls/src/beads.rs` now divides
each section among the beads it holds and traces bead `k` as the zero level of
`depth - centre_k`.

Against the ladder: **1.00 on a rectangle, 1.00 on a disc, 1.00 on a V, 1.20 on a
1 mm wall** — where the disc was 25.5 before, when the walls were walked off the
skeleton. On the plate fixture the two thick layers went from 108 and 82 gap strands
to **0 and 0**. `docs/WALLS-SKELETON.md` section 3 has the tables.

What is left is the middle bead of a section holding an odd number of beads: its path
is a ridge of the field rather than a level set, so that strip is still filled by a
strand. `Mode::Classic` keeps the ladder, so the two can be compared layer for layer.

### 3. Per-object scope

next-layer's per-object UI offers settings this engine applies plate-wide: regions
and layer-height ranges. Two call sites exist (the 3MF override table and the plate
report), which is what justifies building it now rather than earlier.

### 4. Finish the ledger tail that is real work

Seven rows. In cost order:

| row | what it costs |
|---|---|
| `slicing_mode` | the even-odd fill rule and small-hole closing at slice time; the largest of the four and the only one that may need a region boolean |
| `fill_pattern` | everything but lightning is drawn (`docs/EXPLAINED.md`); lightning is a spanning tree grown down from the top surface, worth a week on its own |
| `support_material_style` | snug is a boundary-following variant of the grid body; organic is a second tree generator |
| `avoid_crossing_perimeters` | routes around this layer's own loops today; routing outside the part needs an external graph |
| `ensure_vertical_shell_thickness` | `disabled` narrows the solid window; `partial` and `enabled` coincide because the difference upstream is a void region a coverage grid cannot express |
| `max_volumetric_extrusion_rate_slope_positive` / `_negative` | solved per bead path against its widest section; upstream splits the move, which is what closes the remaining boundary step |

The 57 multi-material rows are **not** in this list. They become work when a
toolchanger exists.

### 5. Work the rest of the parity list

`ENGINE-PARITY.md` §7–§11: the tiled distance field, internal bridges, regions
and height profiles, the in-layer G-code channel, and a gated differential corpus
against libslic3r. §1–§5 are done: settings intake, 3MF, arranging, the
acceleration-aware estimate.

### 6. Swap behind a flag, in the app, not here

Boundary first in next-layer: a project in, G-code and a report out. Then both
engines linked, HFW behind a flag, every gate measured on real projects. Then the
default flips. Deleting libslic3r is the last step and the only irreversible one.

## Done, so it stops being asked about

- **Preset bundles.** `--bundle` reads a vendor tree or a sectioned vendor file,
  resolves `inherits`, and layers machine, then filament, then process. Whole Orca
  library indexes in under a second; a name it lacks is an error listing what it has.
- **Plate packing.** MaxRects, quarter turns allowed, centred, refused before
  slicing when it cannot fit.
- **A time estimate with acceleration in it.** Trapezoids with one junction of
  lookahead and the machine's own cornering model; within 4% of the two prints whose
  real duration is recorded.

## What is deliberately not planned

- **Multi-material, toolchanger, wipe tower.** No hardware, no second caller.
- **A GUI.** This is an engine; next-layer is the GUI.
- **SLA.** Different machine class, nothing in common below the mesh.
- **Becoming a general 3D kernel.** Boolean operations, sculpting and repair
  beyond what slicing needs belong to whoever owns the model, not here.

## How the ledger campaign went

Ten batches between 2026-11-11 and 2026-11-17 took the ledger from 240 applied to
299, and the record of each batch — what was tried, what failed, what it cost — is
in the project memory's archive (`~/.pi/agent/projects-memory/HFW/archive/`,
`2026-11-16-to-27-settings-batches.md`). It is not repeated here; this file is for
what comes next.

Two lessons from it that change how work is done here:

- **A ledger row is a measurement, not a feature.** Several batches moved rows by
  finding they were mis-sorted (multi-material keys filed as algorithms) rather
  than by implementing anything, and that is a legitimate way to move a number
  only because the bucket is what a person reads.
- **Every feature perturbs the gate's pinned numbers.** The two-line infill anchor
  moved the shipped cube from 1096.9 to 1192.1 mm and six gate sections had to be
  isolated with `infill_anchor = 0` to keep testing their own feature. Expect it;
  do not paper over it by loosening the assertion.
