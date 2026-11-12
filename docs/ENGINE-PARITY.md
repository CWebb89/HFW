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

| # | Item | Blocks the swap because | State |
|---|------|------------------------|-------|
| 1 | Settings intake: PrusaSlicer-format profile inheritance, or a translation layer with measured coverage | Users' existing machines and presets must work | not started |
| 2 | Per-object and per-region overrides | next-layer exposes 136 of them per object | not started |
| 3 | Arrange: centring, `--arrange`, build-volume collision | Plates and multi-object jobs are core UI | collision/fit check exists; arrange does not |
| 4 | Acceleration, junction deviation, honest time estimate | Current estimate is 14 min for a job that took 40 | not started |
| 5 | 3MF/AMF import with plates and instances | Most downloaded models are 3MF | not started |
| 6 | Multi-extruder and toolchanger | next-layer's stated differentiator | deliberately deferred |
| 7 | Tiled distance field | A 200 mm plate at 0.05 mm is refused today (~450 MB) | not started |
| 8 | Internal bridges (over infill, inside the part) | Real parts need them; OrcaSlicer's `internal_bridge_*` family assumes them | not started |
| 9 | Differential slice against libslic3r on a fixed corpus | The swap's actual pass/fail | not started |

Items 10 and 11 are not gates but the app's own UI depends on them, so they are
listed here rather than left to be discovered during the swap:

| # | Item | Why the app needs it |
|---|------|----------------------|
| 10 | The in-layer G-code channel and role-aware fan (next-layer's patch to libslic3r) | The calibration suite extrudes PA lines inside layer 1 through it; the fan role drives per-role cooling |
| 11 | Support painting (`FacetsAnnotation` equivalent) and per-object layer-height profiles | Both are shipped UI that changes the slice (`--slice-features` measures 0 → 983 support moves and 125 → 187 layers) |

## Detail

### 1. Settings intake

**Today.** HFW reads its own `profiles/*.cfg` (`profiles/voron_klipper.cfg`,
`profiles/generic_marlin.cfg`) through `crates/config`, with validation before
slicing. There is no inheritance tree and no PrusaSlicer-name mapping.

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

**Today.** `crates/gcode` places the model and rejects a file that does not fit.

**What is needed.** Centring, an `--arrange` mode that packs plates, and a
collision check that runs before slicing rather than after.

**Check.** A three-object plate arranges without overlap into the build volume;
an object larger than the volume is refused with a message naming the axis and
the overflow.

### 4. Acceleration and junction deviation

**Today.** An explicit open item in `docs/ROADMAP.md` §1: "the time estimate is
kinematic only, so it is optimistic: 14 minutes for the 20 mm cube against
PrusaSlicer's 40."

**What is needed.** A velocity profile that respects the machine's
`max_accel`/`square_corner_velocity` (Klipper) or `M201`/`M205` (Marlin), and a
time estimate derived from it.

**Check.** The 20 mm cube at the shipped Voron profile estimates within 10 % of
the reference slicers' estimates, and within 10 % of a real print's reported
duration. It is not enough to match the slicer — the slicer is being checked
against the printer.

### 5. 3MF and AMF

**What is needed.** Import with plates, instances, transforms and units. HFW
takes STL today; next-layer takes 3MF and the majority of models in the wild are
3MF.

**Check.** A multi-plate 3MF imports with the same object count, transforms and
layer counts as libslic3r produces from the same file.

### 6. Multi-extruder and toolchanger

**Today.** Deliberately deferred in `docs/ROADMAP.md`: no toolchanger on this
machine yet, and one caller does not justify per-tool plumbing.

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
