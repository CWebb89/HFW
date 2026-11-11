# Roadmap

What is missing, ordered by what unblocks the most. Each item says what it
replaces, what it depends on, and how it can be checked.

## 1. G-code output — done

Shipped in `crates/gcode`. Extrusion from bead volume, travel, retraction,
z-hop, seam placement, Marlin and Klipper dialects, per-feature feed rates,
pressure advance, cooling, skirt, bed placement and fit checks, and an
independent verifier that re-reads the written file. What is still missing:

- **Acceleration and junction deviation.** The time estimate is kinematic only,
  so it is optimistic: 14 minutes for the 20 mm cube against PrusaSlicer's 40.
- **`EXCLUDE_OBJECT_DEFINE` is emitted but off in both shipped profiles.** It
  needs `[exclude_object]` in `printer.cfg`, and a Klipper machine that lacks it
  errors mid-print. Turn it on only after checking the printer config.
- Arc fitting (`G2`/`G3`) is a size decision, not a correctness one.

## 2. Skinning on sloped surfaces — the measured open question

`docs/ORACLE.md` shows two references agreeing with each other and disagreeing
with this engine by 12–16% on every part with a sloped face, at real infill
densities and not at 100%. The whole deficit sits in the skin class, because the
wall band already covers the staircase surface that both references cover with
solid infill on top of their perimeters.

Volume cannot settle which behaviour is right; it is a surface-quality question.
The next action is a print of the sphere and the torus, inspected for gaps and
droop under the wall beads. If the surfaces are fine, this difference is the
field-wall approach being more efficient and should be documented as a feature.
If they are not, the skin mask needs to follow the local surface instead of the
layer-mask ring, and the oracle tolerance becomes the regression test.

## 3. A tiled distance field — the memory ceiling

The field stores four parallel arrays for every cell of the part bounding box.
A 200 × 200 mm plate at `w/8 = 0.05 mm` is 16 M cells and about 450 MB; today it
is refused by `MAX_FIELD_CELLS` rather than crashing. Allocate 64 × 64 tiles
only where the band near the boundary reaches, and compute the level function
on demand instead of materialising `phi`, `bw` and `lv` per level.

Check: a 200 mm plate at 0.05 mm resolution must plan in under 1 GB and produce
the same coverage numbers as the dense field on a small model.

## 4. Parallel slicing and wall generation

Slicing is currently sequential and is the reference implementation for any
future parallel version. Layers are independent except for the skin mask ring,
which needs a bounded window. Use a deterministic fan-out over layer ranges and
keep output ordering by index.

Check: identical bytes to the sequential run, and a speedup on a 16-core part.

## 5. Features that decide whether prints succeed

- **Supports.** None exist. Generate them from the overhang map implied by the
  skin masks, with interface layers and a separation gap.
- **Bridging.** The skin mask knows where a layer is unsupported below; that is
  the same signal a bridge detector needs.
- **Bed placement.** Models are sliced where they sit in the file. Add centring,
  `--arrange`, and a build-volume collision check.
- **Adaptive layer height.** The field already gives local surface slope cheaply.

## 6. Configuration

- Typed printer / filament / process profiles with PrusaSlicer-style inheritance.
- Per-object and per-region overrides (modifiers, height ranges).
- 3MF and AMF import, including plate and instance data.

## 7. Physical correctness

- Flow and pressure advance, junction deviation, acceleration limits.
- Volumetric flow caps and cooling per layer time.
- Filament cross-section, shrinkage, elephant-foot compensation.
- Multi-extruder and toolchanger support. Not deferred because it is easy but
  because it is premature: there is no toolchanger on this machine yet, and one
  caller does not justify per-tool plumbing in the toolpath model. When the
  Voron toolchanger arrives, add `T` numbering, per-tool offsets and per-tool
  beads as part of the engine rebuild, not as a stub now.

## Known geometry limitations

- Corner recovery uses a two-normal intersection with a distance sanity check;
  very acute corners can still round off.
- Middle beads can overlap their neighbouring side loop by up to about 1.8 ×
  width where the decomposition changes, which shows up as local
  over-extrusion at splits.
- The wall-clipped infill uses a distance query rather than a true polygon
  offset. It is correct for the shapes tested but is not a general
  Clipper-class boolean backend; self-intersecting or barely-touching regions
  are not handled.
- The cone tip demonstrates the thin-layer policy: a layer can keep its
  reference cross-section and produce no printable path. G-code output will need
  an explicit decision (skip the layer, or print a single dot).
