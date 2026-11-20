# Where HFW stands

Written to be read in two minutes, by someone who does not work on this code.
Every number here is measured, and the command that measures it is named.

Updated 2026-11-20 (thirteenth pass: the out-of-scope list — seven features declined
in writing, in the same document the application's panel reads — which settles phase
7 of the engine-replacement plan; the slicing path itself is untouched).

## In one paragraph

HFW is our own slicing engine: it takes a 3D model and a printer profile and
writes printable G-code. It works end to end for **one extruder, one material,
on parts that arrive as STL or 3MF**, and since 2026-11-17 next-layer slices its
plates with it.

What it cannot yet do is take over the *rest* of what libslic3r does in that
application: the project model, the settings system, arrange, preview and project
files. That work is planned phase by phase in `next-layer/docs/rust-engine-plan.md`
and is Rust, here, in this repository — the C++ engine is not the destination for
any of it. `ENGINE-PARITY.md` remains the measured list of what a slicing engine
must honour; it is about parity with the reference slicers now, not about
deciding whether to swap.

## The three numbers

| what | number | what it means | how it is measured |
|---|---|---|---|
| Settings coverage | **299 of 364** (82%) | of every setting the reference slicers define and a one-extruder machine can use, 299 change the output | `python3 tools/gen-settings-ledger.py --check` |
| Single-extruder coverage | **299 of 307** (97%) | the same, ignoring the 57 rows that need a second extruder | same |
| Automated gate | **82 sections, all passing** | every feature is exercised by running the real program and reading the real file | `fish scripts/test-hfw.fish` |

Two more that matter when deciding whether to trust the output:

- **Against two independent slicers**, deposited plastic at 15% infill, re-measured
  2026-11-19 on the current build: cube −1.0%, torus −2.6%, sphere −9.6% and cone −9.4%
  all inside the 10% gate, and `taper_wall` −10.5% with `thin_wall` −24.3% under the
  25% tolerances those two carry on purpose: a wall narrower than a bead prints at its
  measured width here and at a substituted 0.4 mm by both references. On the 18-part
  Revolver plate this engine takes **15.8 s** against PrusaSlicer's 9.4 and
  OrcaSlicer's 14.4 (16 threads, `SLICE-SPEED-STATUS.md` has the breakdown). The Orca
  half of the comparison runs again: `scripts/oracle.py` now loads its presets the way
  OrcaSlicer 2.5.1 insists on, by copying the vendor tree into a datadir instead of
  handing it standalone JSON files.
- **Next-layer would show 175 of its 352 setting controls** with HFW as the
  engine, because the rest are settings we cannot honour. That gap is measured by
  next-layer's own gate, not by us.

## What is actually finished

- Parsing and slicing: STL and 3MF, exact integer geometry, plates of several
  objects, per-object overrides from a 3MF, deterministic output. A layer's material
  is the union of its counter-clockwise contours minus its clockwise ones, and open
  outlines are closed in stages (`--close-gaps`).
- Walls: a ladder of polygon offsets of the layer, one loop at `Params::loop_offset(i)`
  with a miter join, and a gap pass over the strip the beads leave behind so a feature
  narrower than its walls is printed by a strand of its measured width. Beads divided
  from each layer's measured section are kept as `--mode field`, and a plain ladder with
  no gap pass as `--mode classic`. Per-feature bead widths (`outer_wall_width`,
  `wall_distribution_count`), the profile's `resolution` as the simplification tolerance,
  thin walls, corner recovery, size compensation, elephant-foot compensation.
- Infill: twelve patterns, per-island solid threshold, ironing, bridging, the
  two-line anchor that joins fill lines along the wall.
- Supports: normal and tree bodies, interface layers, build-plate-only mode.
- Seams and surface: scarf ramps, staggered seams, fuzzy skin.
- Emission: two dialects, machine limits, pressure advance, cooling, thumbnails,
  pauses, substitutions, and a verifier that re-reads the file and checks it.
- The C ABI (`crates/capi`, version 1): the application holds a mesh or a plate
  without a process, moves it, and asks for G-code, receiving the file plus the
  verifier's own verdict. `crates/capi/include/hfw.h` is the promise; the gate
  compiles a C program against it and runs it. See `NEXTLAYER-BRIDGE.md`.
- The plate document (`crates/plate`): objects with a name, a placement and
  per-object settings, enabled or not, sliced into one job. This is the object
  phase 3 (settings), phase 4 (preview) and phase 6 (project files) attach to.
- A job on its own thread (`hfw_job_start`, `hfw_job_poll`, `hfw_job_cancel`,
  `hfw_job_wait`, `hfw_job_take_gcode`, `hfw_job_free`): progress per object while
  planning and per layer while writing, cancellation observed between objects and
  before the write. A cancelled job yields no file. The threaded job writes
  byte-identical G-code to the synchronous slice — the gate compares the two.
- Scope: `docs/SCOPE.md` (generated) and the surface's `capabilities` array — text,
  cut/hollow, STEP, SLA, MMU and nesting, each with the setting that stands in. None
  of them has a setting the engine parses, so asking for one is refused by name.
  Support and seam painting left that list: they are per-triangle channels, consumed
  inside support generation and by the writer's seam chooser (`docs/PAINT.md`).
- Project files: `Plate::to_project`/`from_project`/`save_project`/`open_project`,
  `hfw_plate_project_bytes`/`hfw_plate_open_project`/`hfw_buffer_free`. A saved plate
  reopened slices to byte-identical G-code — asserted in the plate tests, in the ABI
  tests and in the C smoke program. `Settings::key_value` inverts `apply_key` for
  every published key, and a test walks all 52 to prove the value written reads back.
- Plate operations: `hfw_plate_object_duplicate`, `hfw_plate_object_rotate` (about
  the object's own centre), `hfw_plate_object_mirror` (in place, winding corrected)
  and `hfw_plate_arrange` over a bed and a spacing. Scale, translate and any
  placement remain `hfw_plate_object_placement`. The arranger is one implementation
  in `crates/plate/src/arrange.rs`, used by the command line and by the ABI alike;
  it packs unpositioned objects on a MaxRects layout and leaves objects a project
  file positioned where they are. A part that does not fit is refused with its size
  rather than dropped.
- The preview document (`hfw_plate_preview_json`, `hfw_string_free`): every planned
  bead with its position, per-vertex width, flow multiplier and the label the G-code
  writer gives it, grouped per layer. The label comes from the writer's own mapping,
  so a viewer cannot group a path differently from the `;TYPE:` marker in the file a
  printer runs.
- Per-object settings from one key at a time (`Settings::apply_key`, the ABI's
  `hfw_plate_object_set`, the command line's `--object-settings`): 52 print-level
  settings can be set on a single object, by this engine's name or the dialect's, and
  a key or value that cannot be applied is refused with the reason rather than
  silently dropped.
- The settings surface (`--settings-surface`, `hfw_settings_surface_json`): one JSON
  document describing all 383 settings an engine is expected to understand, of which
  299 are honoured here, with the key, location, type, default, unit and status of
  each, and marks the 47 of them a panel may set on one object. It is generated by
  the same run that writes `docs/SETTINGS-LEDGER.md`, so the panel and the ledger
  cannot disagree, and the `per_object` flag is read from the engine's own published
  list rather than restated by hand.

## What is not finished, and what it costs

| item | cost | notes |
|---|---|---|
| 6 ledger rows still `partial` | days to a week | the largest is lightning infill, a spanning tree grown down from the surface; also snug support, routing outside the part, and the slicing-mode fill rules |
| 57 multi-material rows | blocked on hardware | there is no toolchanger; the setting means nothing until there is one |
| Tiled distance field | 2–3 days | a 200 mm plate at fine resolution is refused rather than run |
| Internal bridges | 2–3 days | every bridge we detect today is over air, not over infill |
| Per-object regions and layer-height profiles | a week | next-layer's per-object UI offers settings we cannot scope to a region or a height range |

## What changed in this pass

**A layer is material, not a nesting puzzle.** Each triangle/plane chord is ordered
by its triangle's outward normal, so a contour arrives wound the way the solid is:
counter-clockwise adds, clockwise removes. The layer region is the union of the
positive contours minus each negative one, in the integer kernel. Two overlapping
boxes now print their union — 632.000 mm² at z=12, one ring, against an L-shaped
232 before — and a shell drawn inside another with the same winding stays material
instead of becoming a hole.

**Open outlines close in stages instead of emptying a layer.** Nearest endpoints are
joined at 0, 0.001, 0.1 and 1 mm, then at `--close-gaps` (default 2 mm, max 10). On
the defect corpus that takes `hole_small` from 116 to 120 planned layers and
`hole_large` from 102 to 106 at the default; `--close-gaps 4` plans all 120. A trail
that had to guess at a junction is a fragment of a self-touching surface and is
dropped rather than joined into a spur.

**Mesh repair runs on every job**, not on `--repair-basic`: welding exact duplicates,
dropping degenerate and duplicate triangles and orienting closed components outward
cannot turn a correct mesh wrong, and the slice now depends on face winding. A mesh
with four faces reversed slices to the same 1611.143 mm³ as the clean cube.

**A gap strand is charged for the slot it fills.** `Kind::Middle` fills the slot
between the loops either side of it, so it deposits the rectangle, not a free bead's
rectangle minus the corner term `h²(1 − π/4)`. That term was 14% of a 0.3 mm rib by
construction: `thin_wall` goes from −24.3% to −11.7% against the two references, and
`taper_wall` from −10.5% to −10.3%.

The wall rewrite of the pass before this one, kept because its reasoning still holds:

Walls went back to the polygon offset ladder, the two phases that dominated the clock
were measured and made cheaper, and nothing about the deposited geometry moved by more
than a per-cent:

- **Walls are offsets again, and the section generator is `--mode field`.** The default
  had been a rasterised distance field since 33b9eb9: cost proportional to the area
  under the layer, a 0.2 mm grid resolving a 0.4 mm bead, a 0.07 mm simplification floor
  that beveled every sharp corner, and a 0.5 mm rib that vanished because the level set
  through its medial axis has no sign change. Loop `i` is now the layer offset inward by
  `Params::loop_offset(i)` — the code `poly` already had — followed by the gap pass over
  what the beads leave. On the stress gear that is 0.38% of the wall band uncovered
  against 4.39%, a 0.266 mm maximum distance from the surface to a bead centreline
  against 8.331, the rib covered, and 127 ms of wall work for 150 layers against 2,247.
- **The infill scanline no longer samples the wall band.** It cut its spans on the
  layer boundary and then asked a segment index 5.1 million times whether each 0.1 mm
  sample was inside the wall. The spans are cut on the inset region instead, so the
  question is answered by construction: infill on the gear fell from 6,089 ms to 746 ms.
- **The travel router was quadratic in the layer and unbounded in the search.** Each of
  the gear's 4,629 travels re-measured every wall loop on its layer, and a travel that
  had no route inside its 2 mm budget exhausted the visibility graph before saying so.
  The loops are measured once and the search is A* bounded by the budget: writing the
  gear's G-code fell from 4,517 ms to 1,249 ms, byte-identical output.

The section-bead pass that came before this one, kept because its reasoning still holds
for `--mode field`:

Walls are beads divided from each layer's measured section instead of a ladder of
offsets, and the defects that showed up on the plate once they were are closed:

- **A bead can no longer be printed twice.** Where a section divides exactly, the
  innermost bead sits on the section's medial axis and the tracer returns its level
  set as two branches one grid cell apart — an 11 mm excursion out and back, 0.13 mm
  apart, which is the lap a wall showed. A fold is now collapsed to the longer of its
  two arcs. On the plate that removed every 180° reversal from the walls.
- **A seam follows the loop it grew out of, by its points.** Pairing loops by centre
  and area swapped two loops of a symmetric part whenever their centres sat nearer the
  wrong loop, which moved both seams to the far side of the part. Pairing counts how
  many of a loop's own sampled points land on the loop below instead.
- **Travels keep clear of printed walls, and the seam wins the argument.**
  `avoid_crossing_perimeters` is on by default with the reference 2 mm detour cap, and
  it no longer rotates a closed loop to save a travel: routing the travel costs
  distance, not the seam. On the Revolver plate that is 13,315 crossings against
  17,912, with the seams as they were.
- **Three settings the section generator was dropping**: `outer_wall_width` and
  `wall_distribution_count` (the ledger claimed them applied), `resolution`, and
  `bridge_acceleration` on a profile with no base acceleration — the last of those set
  a bridge's 3000 and left it for the rest of the print.

The parity list from the pass before this one:

- **`--bundle`** indexes a vendor profile tree (Orca's shipped layout) or a
  sectioned vendor file (PrusaSlicer's), resolves `inherits` across it, and layers
  one machine, one process and one filament. Orca's whole library — 1463 machines,
  3277 processes, 6650 filaments — indexes in under a second. Ambiguous names are
  resolved toward the machine's own vendor, because every vendor ships a
  "Generic PLA".
- **The time estimate is planned, not summed.** Trapezoids with one junction of
  lookahead, junction deviation from the profile (Klipper's
  `square_corner_velocity` converted the way Klipper converts it). On the two prints
  whose real duration is recorded it lands within 4% — cube 16.2 against 16 printed,
  sphere 21.8 against 21 — where the old sum was 5–6% low. With travel routing and the
  profile's own `default_accel = 5000` the same two now plan at 15.7 and 21.6 min.
- **Plates are packed** with MaxRects and centred, and a part that cannot fit is
  refused before slicing instead of after.

Two bugs found on the way, both older than this work: a preset's `bed_shape` never
reached the bed size that placement and the fit check use (every `--preset` run was
centred on a 220 mm bed, whatever the machine), and `--scale` was applied *after*
laying out a plate, so scaled parts overlapped or passed a fit check they no longer
met.

Then the last of the intake work: a machine preset's templated start G-code.

- **Its bed and height are read under Orca's own names.** OrcaSlicer writes
  `printable_area` and `printable_height` where PrusaSlicer writes `bed_shape`; the
  rename table came from the application's oracle-gated `ForeignSettings.cpp` and
  now also from its `PresetImport.cpp`, so a Voron 350 preset is a 350 mm machine
  instead of the default 220. The same addition reads `machine_max_speed_x`, the
  firmware ceiling Orca names and the old table did not.
- **Its placeholders are substituted, and intake only names the ones nothing can
  fill.** `[bed_temperature_initial_layer_single]` and its neighbours come from the
  preset or from the configuration; the part's extent, the layer number and the
  layer count are left for the writer, which is the only place they are known. A
  preset that keeps such a token now fails verification with the token named,
  instead of printing the word. `--intake-json` works for a bundle job too: it was
  written before the bundle resolved, so the report a caller needs to prove no
  setting vanished could not be produced on the path that needed it.

## What needs a decision from Curtis

1. **The tree is committed and not pushed.** Everything up to and including the
   wall rewrite is on `master` at one commit, which leaves the branch six ahead of
   `origin/master`. Pushing is a decision about the repository, not the code:
   `git@github.com:CWebb89/HFW.git` is public, and the recommendation is to make it
   private first — `STATUS.md` and `ENGINE-PARITY.md` describe next-layer's 352
   controls and the 177 that would be hidden, which is unreleased product strategy.
2. **The swap into next-layer is not the next thing.** The engine is not the
   blocker in next-layer: next-layer's own finish line is *two real prints through
   the app*. Adding HFW before that would hide 177 controls and add a second
   engine to debug at the same time.
3. **What "done" means for the ledger.** Its stated finish line is 100% applied,
   which is unreachable while 57 rows need a toolchanger. The realistic line is
   "applied, n/a, or blocked on hardware, each with the reason written down".

## What is left, in the order it should be done

Speed is no longer the problem in slicing, and G-code emission is: 15.8 s for the plate
at 16 threads, of which 3.9 s is the engine and 8.8 s is rendering and re-reading 29 MB
of G-code single-threaded against PrusaSlicer's 9.4 and OrcaSlicer's 14.4. What is left
is spread over four things, and two of them are bigger than they look.

1. **The band's width policy** — the first of its two pieces is done, the second is
   not:
   - **`bead_area` for a slotted strand** is fixed: a `Kind::Middle` strand deposits the
     rectangle, measured −24.3% → −11.7% on `thin_wall` (`DECISIONS.md` DECISION-5).
   - **Redistribution.** Share the leftover band width across the existing loops
     instead of squeezing a separate strand into it, so a 1.0 mm wall is 0.5 + 0.5
     rather than 0.4 + 0.4 + 0.2. `Params` already carries the parabolic weighting
     and `Line` already carries per-vertex width, so it is a change to how widths
     are chosen plus a transition rule, not new geometry. 3-5 days. The spacing model
     it depends on is DECISION-7.

   `DESIGN.md` under *Walls* holds the measurement and the reasoning; the sub-width
   case (`thin_wall`) and the fractional-band case (`sphere`, `cone`, `taper_wall`)
   are the same question asked twice.
2. **Per-object regions and layer-height profiles** — the settings next-layer
   exposes per object that we can only apply plate-wide. A week.
3. **The two measured performance items** (tasks #31, #32): G-code rendering is now the
   only phase left above a second on the plate — 8.8 s of the 15.8, single-threaded,
  29 MB — and infill is 2.29 s across 16 threads. Neither is needed for parity and both
   are rewrites; do them only when 15.8 s stops being fast enough, or when a job profile
   makes them painful. The breakdown is in `SLICE-SPEED-STATUS.md`.
4. **The ledger tail that is real work**: lightning infill, snug support, external
   travel routing, slicing fill modes. Then the tiled field and internal bridges.

Do not do 3 before 1-2. The engine is already at performance parity and behind on
things a user can see.

## Reading order

- `README.md` — what it is and how to run it.
- `STATUS.md` — this page.
- `ROADMAP.md` — the work order, with the reasoning for it.
- `DECISIONS.md` — every choice the code cannot explain by itself, with the reason
  and how to reverse it.
- `BEHAVIOR-NOTES.md` — what each stage does in plain words, independent of any
  source tree.
- `AUDIT-2026-11.md` — one table per phase, `metric | before | after | command`.
- `PROVENANCE.md` — what was read from where, and the licence audit.
- `NEXTLAYER-BRIDGE.md` — how next-layer hands a job to this engine today and the
  ABI it would need instead.
- `ENGINE-PARITY.md` — what must be true before it can replace libslic3r.
- `SETTINGS-LEDGER.md` — the generated, complete per-setting inventory.
- `ORACLE.md` — the measured comparison against OrcaSlicer and PrusaSlicer.
