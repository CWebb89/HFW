# Where HFW stands

Written to be read in two minutes, by someone who does not work on this code.
Every number here is measured, and the command that measures it is named.

Updated 2026-11-17 (second pass: preset bundles, plate packing, accelerated time
estimate).

## In one paragraph

HFW is our own slicing engine: it takes a 3D model and a printer profile and
writes printable G-code. It works end to end for **one extruder, one material,
on parts that arrive as STL or 3MF**. It is not yet wired into next-layer, and it
cannot replace libslic3r — that library is not just a slicer in that app, it is
the project model, the settings system, the mesh importers, arrange, cut, hollow,
paint and the G-code preview. Details in `ENGINE-PARITY.md`.

## The three numbers

| what | number | what it means | how it is measured |
|---|---|---|---|
| Settings coverage | **299 of 364** (82%) | of every setting the reference slicers define and a one-extruder machine can use, 299 change the output | `python3 tools/gen-settings-ledger.py --check` |
| Single-extruder coverage | **299 of 307** (97%) | the same, ignoring the 57 rows that need a second extruder | same |
| Automated gate | **76 sections, all passing** | every feature is exercised by running the real program and reading the real file | `fish scripts/test-hfw.fish` |

Two more that matter when deciding whether to trust the output:

- **Against two independent slicers**, deposited plastic agrees within 3% on flat
  parts and at 100% infill; on curved parts at low infill we deposit 12–16% less,
  and a printed test decided that the references over-fill sloped surfaces rather
  than that we under-fill (`ORACLE.md`).
- **Next-layer would show 175 of its 352 setting controls** with HFW as the
  engine, because the rest are settings we cannot honour. That gap is measured by
  next-layer's own gate, not by us.

## What is actually finished

- Parsing and slicing: STL and 3MF, exact integer geometry, plates of several
  objects, per-object overrides from a 3MF, deterministic output.
- Walls: variable-width beads from a distance field, thin walls, corner recovery,
  size compensation, elephant-foot compensation.
- Infill: twelve patterns, per-island solid threshold, ironing, bridging, the
  two-line anchor that joins fill lines along the wall.
- Supports: normal and tree bodies, interface layers, build-plate-only mode.
- Seams and surface: scarf ramps, staggered seams, fuzzy skin.
- Emission: two dialects, machine limits, pressure advance, cooling, thumbnails,
  pauses, substitutions, and a verifier that re-reads the file and checks it.

## What is not finished, and what it costs

| item | cost | notes |
|---|---|---|
| **Start-G-code placeholders in a machine preset** | 2–3 days | the one thing between a real Orca bundle and a printable file: `[bed_temperature_initial_layer_single]` and friends are templated into `machine_start_gcode`, and we substitute only our own vocabulary. Intake now names them before slicing |
| 6 ledger rows still `partial` or `missing` | days to a week | the largest is lightning infill, a spanning tree grown down from the surface; also snug support, routing outside the part, and the slicing-mode fill rules |
| 57 multi-material rows | blocked on hardware | there is no toolchanger; the setting means nothing until there is one |
| Tiled distance field | 2–3 days | a 200 mm plate at fine resolution is refused rather than run |
| Internal bridges | 2–3 days | every bridge we detect today is over air, not over infill |
| Per-object regions and layer-height profiles | a week | next-layer's per-object UI offers settings we cannot scope to a region or a height range |

## What changed in this pass

Three items came off the engine-parity list, and one of them found a bug that had
been in every run since presets were read:

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
  sphere 21.8 against 21 — where the old sum was 5–6% low.
- **Plates are packed** with MaxRects and centred, and a part that cannot fit is
  refused before slicing instead of after.

Two bugs found on the way, both older than this work: a preset's `bed_shape` never
reached the bed size that placement and the fit check use (every `--preset` run was
centred on a 220 mm bed, whatever the machine), and `--scale` was applied *after*
laying out a plate, so scaled parts overlapped or passed a fit check they no longer
met.

## What needs a decision from Curtis

1. **Nothing is committed.** The engine tree has been changing for weeks without a
   commit, and there is **no git remote configured**. Committing needs a decision
   on how it is split up; pushing needs a repository name and whether it is public.
2. **The swap into next-layer is not the next thing.** The engine is not the
   blocker in next-layer: next-layer's own finish line is *two real prints through
   the app*. Adding HFW before that would hide 177 controls and add a second
   engine to debug at the same time.
3. **What "done" means for the ledger.** Its stated finish line is 100% applied,
   which is unreachable while 57 rows need a toolchanger. The realistic line is
   "applied, n/a, or blocked on hardware, each with the reason written down".

## The next three actions, in order

1. **Start-G-code placeholders.** Until this is done, a job from a real machine
   preset produces a file that verifies as unreadable. It is the last thing between
   `--bundle` and "a user's own printer, sliced".
2. Per-object regions and layer-height profiles — the settings next-layer exposes
   per object that we can only apply plate-wide.
3. The ledger tail that is real work: lightning infill, snug support, external
   travel routing, slicing fill modes. Then the tiled field and internal bridges.

## Reading order

- `README.md` — what it is and how to run it.
- `STATUS.md` — this page.
- `ROADMAP.md` — the work order, with the reasoning for it.
- `ENGINE-PARITY.md` — what must be true before it can replace libslic3r.
- `SETTINGS-LEDGER.md` — the generated, complete per-setting inventory.
- `ORACLE.md` — the measured comparison against OrcaSlicer and PrusaSlicer.
