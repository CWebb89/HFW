# Where HFW stands

Written to be read in two minutes, by someone who does not work on this code.
Every number here is measured, and the command that measures it is named.

Updated 2026-11-19 (fourth pass: walls as section beads, seam placement, the gate
re-baselined).

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
| Automated gate | **80 sections, all passing** | every feature is exercised by running the real program and reading the real file | `fish scripts/test-hfw.fish` |

Two more that matter when deciding whether to trust the output:

- **Against two independent slicers**, deposited plastic at 15% infill, re-measured
  2026-11-19: cube −1.1%, torus −4.2% and cone −9.9% inside the 10% gate, the sphere at
  −10.2% just outside it — the one row out — and `taper_wall` −23.1% with
  `thin_wall` −24.3% under the 25% tolerances those two carry on purpose: a wall
  narrower than a bead prints at its measured width here and at a substituted 0.4 mm by
  both references. On the 18-part Revolver plate this engine sliced in 11.4 s against
  PrusaSlicer's 9.4 and OrcaSlicer's 14.4 (measured 2026-11-19, before the wall stage).
  The Orca half of that comparison cannot be re-run on the installed OrcaSlicer 2.5.1,
  which rejects the standalone preset files the script loads; a fresh run compares
  against PrusaSlicer alone and says so with `reference spread 0.0%` (`ORACLE.md`).
- **Next-layer would show 175 of its 352 setting controls** with HFW as the
  engine, because the rest are settings we cannot honour. That gap is measured by
  next-layer's own gate, not by us.

## What is actually finished

- Parsing and slicing: STL and 3MF, exact integer geometry, plates of several
  objects, per-object overrides from a 3MF, deterministic output.
- Walls: beads divided from each layer's measured section, so a feature narrower than
  the perimeters it asked for is filled by its own beads rather than by a strand beside
  them; per-feature bead widths (`outer_wall_width`, `wall_distribution_count`), the
  profile's `resolution` as the simplification tolerance, thin walls, corner recovery,
  size compensation, elephant-foot compensation. A classic offset ladder is kept for
  comparison (`--mode classic`), and the gap pass survives as the fallback for a section
  whose middle bead is a ridge rather than a level set.
- Infill: twelve patterns, per-island solid threshold, ironing, bridging, the
  two-line anchor that joins fill lines along the wall.
- Supports: normal and tree bodies, interface layers, build-plate-only mode.
- Seams and surface: scarf ramps, staggered seams, fuzzy skin.
- Emission: two dialects, machine limits, pressure advance, cooling, thumbnails,
  pauses, substitutions, and a verifier that re-reads the file and checks it.

## What is not finished, and what it costs

| item | cost | notes |
|---|---|---|
| 6 ledger rows still `partial` | days to a week | the largest is lightning infill, a spanning tree grown down from the surface; also snug support, routing outside the part, and the slicing-mode fill rules |
| 57 multi-material rows | blocked on hardware | there is no toolchanger; the setting means nothing until there is one |
| Tiled distance field | 2–3 days | a 200 mm plate at fine resolution is refused rather than run |
| Internal bridges | 2–3 days | every bridge we detect today is over air, not over infill |
| Per-object regions and layer-height profiles | a week | next-layer's per-object UI offers settings we cannot scope to a region or a height range |

## What changed in this pass

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

Speed is no longer the problem: 10.8 s for the plate against 9.4 and 14.4. What is
left is spread over four things, and two of them are bigger than they look.

1. **The band's width policy** — two pieces, in this order:
   - **`bead_area` for a slotted strand.** A `Kind::Middle` gap strand fills a slot
     whose corners its neighbours are, so the free-bead corner correction
     `h²(1 − π/4)` is wrong for it: it withholds 14% of a 0.3 mm rib's volume by
     construction. `fixtures/thin_wall.stl` is −24.3% and this is most of it. Hours,
     not days, but it moves every pinned volume in the tests.
   - **Redistribution.** Share the leftover band width across the existing loops
     instead of squeezing a separate strand into it, so a 1.0 mm wall is 0.5 + 0.5
     rather than 0.4 + 0.4 + 0.2. `Params` already carries the parabolic weighting
     and `Line` already carries per-vertex width, so it is a change to how widths
     are chosen plus a transition rule, not new geometry. 3-5 days.

   `DESIGN.md` under *Walls* holds the measurement and the reasoning; the sub-width
   case (`thin_wall`) and the fractional-band case (`sphere`, `cone`, `taper_wall`)
   are the same question asked twice.
2. **Per-object regions and layer-height profiles** — the settings next-layer
   exposes per object that we can only apply plate-wide. A week.
3. **The two measured performance items** (tasks #31, #32): infill still rasterises
   at 3.75 s and G-code writing is 3.6 s of the 10.8. Neither is needed for parity
   and both are rewrites; do them only when 10.8 s stops being fast enough, or when
   a job profile makes them painful. The breakdown is in `SLICE-SPEED-STATUS.md`.
4. **The ledger tail that is real work**: lightning infill, snug support, external
   travel routing, slicing fill modes. Then the tiled field and internal bridges.

Do not do 3 before 1-2. The engine is already at performance parity and behind on
things a user can see.

## Reading order

- `README.md` — what it is and how to run it.
- `STATUS.md` — this page.
- `ROADMAP.md` — the work order, with the reasoning for it.
- `ENGINE-PARITY.md` — what must be true before it can replace libslic3r.
- `SETTINGS-LEDGER.md` — the generated, complete per-setting inventory.
- `ORACLE.md` — the measured comparison against OrcaSlicer and PrusaSlicer.
