# Engine parity — what HFW must do before it can replace libslic3r in next-layer

`next-layer` ships today on PrusaSlicer's `libslic3r` 3.0.0-alpha12 (see
`../../../next-layer/THIRD-PARTY.md`). The plan is for this engine to replace it.
This file is that plan's pass/fail list, so "is HFW ready?" has an answer that is
not a feeling.

Two rules make it checkable:

1. **Every item names how it is measured.** A line here that cannot be measured
   does not belong here.
2. **The swap happens once, behind a boundary, at the end.** Not incrementally
   into the app, and never while both sides are moving.

## Why the order is what it is

`next-layer` already ships 154 finished features and exposes 136 print settings
per object. Its presets are PrusaSlicer/OrcaSlicer-format bundles: thousands of
keys, resolved vendor → machine → process → filament. An engine that cannot
consume those settings does not fail loudly — it silently ignores them, and the
user changes a value in the UI and gets identical G-code. That failure mode is
worse than a crash, and it is why **§1 comes first and blocks everything else**.

## The list

State is dated 2026-11-17 and checked against the code, not against this file's
own history. "Green" means the check below runs and passes today.

| # | Item | Blocks the swap because | State |
|---|------|------------------------|-------|
| 1 | Settings intake: PrusaSlicer-format profile inheritance, or a translation layer with measured coverage | Users' existing machines and presets must work | **green for intake, one blocker left.** `--bundle` indexes a vendor profile tree or a sectioned vendor file, resolves `inherits` across it, picks one machine, one process and one filament (preferring the machine's own vendor), and reports every key as applied/partial/unknown. Orca's whole shipped library — 1463 machines, 3277 processes, 6650 filaments — indexes in under a second. **Blocker: `machine_start_gcode` placeholders.** A real machine preset is templated with `[bed_temperature_initial_layer_single]`, `{during_print_exhaust_fan_speed_num[current_extruder]}` and the like; this engine substitutes a small set of its own and leaves the rest literal, so the verifier rejects the file. Intake now names those placeholders up front instead of after the G-code is written.
| 2 | Per-object and per-region overrides | next-layer exposes 136 of them per object | **partial.** Five keys per object (density, pattern, walls, width, support). No region volumes, no per-object height ranges |
| 3 | Arrange: centring, `--arrange`, build-volume collision | Plates and multi-object jobs are core UI | **green.** A MaxRects packer places every unpositioned object in the gap it wastes least, turns one a quarter turn when that is what makes it fit, centres the arrangement on the bed, and refuses a plate that cannot fit before slicing rather than after planning it. A plate a file already positioned keeps its own layout, and the remaining objects are laid out beside it |
| 4 | Acceleration, junction deviation, honest time estimate | A kinematic estimate is optimistic by 2–3× on short moves | **green.** Burst-free trapezoidal planner with one junction of lookahead: per-move acceleration, junction deviation (Marlin's `M205 J`, or Klipper's `square_corner_velocity` converted the way `klippy/toolhead.py` converts it), and a classic-jerk model for a Marlin profile that sets no deviation. On the two prints whose real duration is recorded, the planned estimate lands within 4% (cube 16.2 min against 16 printed, sphere 21.8 against 21) where the kinematic sum was 5–6% low. A profile that states no acceleration still gets the kinematic number, and the file's header says which one it got |
| 5 | 3MF/AMF import with plates and instances | Most downloaded models are 3MF | **green for 3MF.** ZIP, unit, components, per-item transforms and a project's own settings file. No AMF |
| 6 | Multi-extruder and toolchanger | next-layer's stated differentiator | deliberately deferred |
| 7 | Tiled distance field | A 200 mm plate at 0.05 mm is refused today (~450 MB) | not started |
| 8 | Internal bridges (over infill, inside the part) | Real parts need them; OrcaSlicer's `internal_bridge_*` family assumes them | not started |
| 9 | Differential slice against libslic3r on a fixed corpus | The swap's actual pass/fail | **effectively running at fixture level.** `scripts/oracle.py` compares against PrusaSlicer, which is libslic3r's own consumer, on six fixtures. It is not a gated corpus and it is not driven through libslic3r's API |

Items 10 and 11 are not gates but the app's own UI depends on them, so they are
listed here rather than left to be discovered during the swap:

| # | Item | Why the app needs it |
|---|------|----------------------|
| 10 | The in-layer G-code channel and role-aware fan (next-layer's patch to libslic3r) | The calibration suite extrudes PA lines inside layer 1 through it; the fan role drives per-role cooling. The fan half is done — every feature has its own fan duty; the in-layer channel is not |
| 11 | Support painting (`FacetsAnnotation` equivalent) and per-object layer-height profiles | Both are shipped UI that changes the slice (`--slice-features` measures 0 → 983 support moves and 125 → 187 layers). Variable layer height exists as a profile and as height bounds; painting and per-object profiles do not |

## Detail

### 1. Settings intake

**Today.** Two paths, and both work. The engine reads its own
`profiles/*.cfg` (`profiles/voron_klipper.cfg`, `profiles/generic_marlin.cfg`)
with validation before slicing, and it reads a PrusaSlicer/Orca file through
`--preset`: the dialect table renames the reference key onto the engine name,
`inherits` chains resolve among the files it is given, and every key is reported
back as applied, partial or unknown — never silently dropped. Coverage is the
ledger's 299 of 364 and the ceiling is the UI's own list, not this engine's.

**The bundle layer exists.** `--bundle <dir>` indexes a vendor profile tree (Orca's
shipped layout) or a sectioned vendor file (PrusaSlicer's), resolves `inherits`
across the whole set, and layers one machine, one process and one filament in the
order the reference slicers use — machine, then material, then print settings, so a
process value wins over both. A name the bundle does not hold is an error that
lists the names it does hold; a *parent* the bundle does not hold is a warning,
because an exported preset routinely inherits from a system preset that lives
inside the slicer rather than next to the file.

**The placeholder vocabulary is in.** A machine preset's `machine_start_gcode` is a
template: `[bed_temperature_initial_layer_single]`,
`{during_print_exhaust_fan_speed_num[current_extruder]}`, `[first_layer_print_min[0]]`.
A placeholder naming a setting is substituted at intake, from the preset first and
the engine's configuration second; one whose value belongs to the slice (the part's
extent, the layer number, the layer count) is left for the writer, which fills it
at emission; and one nothing can fill is named in the intake report and fails the
verifier with the file's line number. `--intake-json` writes that report for a
bundle job, so a caller can prove no setting vanished.

The machine's own geometry is read under both spellings: `bed_shape` as
PrusaSlicer writes it and `printable_area` / `printable_height` as Orca writes
them, from the rename table next-layer's importer keeps.

**What next-layer feeds it.** A resolved settings object per plate: the vendor
bundle the engine resolves as a package, plus per-object overrides. Every key
originates in PrusaSlicer/OrcaSlicer naming.

**Check.** For every setting next-layer's settings panel can write, HFW either
honours it or lists it as ignored in a generated report. Coverage target: 100 %
of the settings the UI exposes, with the ignored set named and justified. A
setting that is silently accepted and ignored fails this check.

### 2. Per-object and per-region overrides

**Today.** Not modelled. The toolpath model has one settings set per slice.

**What is needed.** Overrides keyed by object instance, plus region modifiers and
height ranges. A "region" is an extra volume with its own settings.

**Check.** The same two assertions next-layer already measures against
libslic3r: a 0.30 mm per-object layer height must reduce the layer count from
125 to 84, and per-object `support_material: none` must leave zero support moves.

### 3. Arrange

**Today.** `crates/cli/src/arrange.rs` packs a plate: MaxRects with a
best-short-side-fit score, a quarter turn allowed per object, spacing as air
between footprints, and the finished arrangement centred on the bed. Scaling
(units, `--scale`, shrinkage) is applied *before* packing, so the packer works in
the millimetres the bed is measured in. A part that cannot fit is refused with its
size and the bed's, before any layer is planned.

A plate that a 3MF positioned keeps that layout, and the unpositioned objects are
laid out beside it: a file's coordinates are a decision the user made in the
slicer, and mixing those with objects that have no position would need to know
which of them were moved on purpose.

**Check.** Three objects arrange without overlap and inside the bed; an object
larger than the volume is refused with a message naming its size and the bed's.
Both are gate sections.

### 4. Acceleration and junction deviation

**Today.** The commands are emitted — `M201`/`M203`/`M205` for Marlin,
`SET_VELOCITY_LIMIT` for Klipper, and per-feature acceleration with a machine
ceiling. The estimate is `crates/gcode/src/time.rs`: every emitted move goes
through a trapezoidal planner (accelerate, cruise, decelerate) with one junction of
lookahead, so a move's exit speed is decided by the corner it shares with the next.

The cornering model is the machine's own. Marlin gives junction deviation directly
(`M205 J`); Klipper gives `square_corner_velocity` instead and derives the deviation
from it exactly as `klippy/toolhead.py` does, so a 90° corner is timed at `scv`
by construction. A Marlin profile that sets jerk and no deviation gets the classic
jerk model. A profile that states neither is timed kinematically, and the file's
header says so rather than pretending.

**Measured.** Against the two prints whose real duration is recorded on the Voron:
the sphere estimated 21.8 min against 21 printed, the cube 16.2 against 16, where
the kinematic sum was 5–6 % low on both. Against the reference slicers the numbers
are not comparable — they print the same geometry at their own speeds, so Orca
claims 29.9 min and PrusaSlicer 21.0 for a cube this engine plans at 15–16.

**What is still missing.** Heat-up, homing and the start G-code's own work are not
counted by anything (no planner can know them), and the firmware's lookahead through
a fan of very short segments is approximated by the one-junction model.

### 5. 3MF and AMF

**Today.** 3MF import works: the ZIP is opened, the model part parsed, `unit`
honoured, components resolved recursively, and every build item placed by its own
4x3 transform. A 3MF that is a *project* also carries settings
(`Metadata/Slic3r_PE.config` or Bambu's flat JSON) and those are applied. AMF is
not read at all.

**What is still needed.** AMF, and a check that a project 3MF round-trips the way
libslic3r does — plates, instances, per-object config, paint.

**Check.** A multi-plate 3MF imports with the same object count, transforms and
layer counts as libslic3r produces from the same file.

### 6. Multi-extruder and toolchanger

**Today.** Deliberately deferred (`docs/ROADMAP.md`, *What is deliberately not
planned*): no toolchanger on this machine yet, and one caller does not justify
per-tool plumbing.

**What is needed.** `T` numbering, per-tool offsets, per-tool beads, a wipe
tower, and filament per tool.

**Check.** A two-tool slice emits `T0`/`T1` with the offsets from the profile and
a wipe tower that the independent verifier re-reads. Hardware validation is a
separate gate — a printed part per tool.

### 7. Tiled distance field

**Today.** Four parallel arrays per cell of the part's bounding box; a 200 mm
plate at `w/8 = 0.05 mm` is 16 M cells and ~450 MB, refused by `MAX_FIELD_CELLS`
rather than crashed.

**Check.** A 200 mm plate at 0.05 mm plans under 1 GB and reports the same
coverage numbers as the dense field on a small model.

### 8. Internal bridges

**Today.** Every detected span is external: the air mask is `own - below`, so
cells over sparse infill are never bridge cells.

**Check.** A fixture with an internal void whose ceiling sits over sparse infill
produces bridge beads there, and no sparse line crosses air.

### 9. Differential slice

**This is the gate, not a feature.** Same model, same resolved settings, both
engines, one comparison.

**Check.** On a fixed corpus (the `fixtures/` set plus three real models), G-code
from HFW and from libslic3r agree within a stated tolerance on: extruded volume
per feature class, layer count, per-layer path length, and total time estimate.
Divergences beyond tolerance are listed with a cause, exactly as `docs/ORACLE.md`
does for the two reference slicers.

## How the swap happens

1. **Boundary first, in next-layer.** A narrow interface: accept a project
   (objects, transforms, resolved settings), return G-code plus a report. Until
   that exists, the app's engine types are smeared through the plate and preset
   layers and there is nothing to swap against.
2. **Both engines linked.** Ship with libslic3r as the default and HFW behind a
   flag, so every gate above can be measured on real projects in the app itself.
3. **Flip the default** when 1–9 are green.
4. **Delete libslic3r** only after a release cycle with no regression reported.

Step 4 is the only irreversible one. It does not change next-layer's licence:
the app stays AGPL-3.0 while it ships the OrcaSlicer-derived printer catalog and
preset conversions, and `docs/release-readiness.md` says so. What it changes is
that the G-code next-layer emits would come from code in this repository rather
than from a pinned upstream checkout.
