# Roadmap

What is missing, ordered by what unblocks the most. Each item says what it
replaces, what it depends on, and how it can be checked.

## 1. G-code output — done

Shipped in `crates/gcode`. Extrusion from bead volume, travel, retraction,
z-hop, seam placement, Marlin and Klipper dialects, per-feature feed rates,
pressure advance, cooling, skirt, a solid filled brim, bed placement and fit
checks, and an independent verifier that re-reads the written file. What is
still missing:

- **Acceleration and junction deviation.** The time estimate is kinematic only,
  so it is optimistic: 14 minutes for the 20 mm cube against PrusaSlicer's 40.
- **`EXCLUDE_OBJECT_DEFINE` is emitted but off in both shipped profiles.** It
  needs `[exclude_object]` in `printer.cfg`, and a Klipper machine that lacks it
  errors mid-print. Turn it on only after checking the printer config.
- Arc fitting (`G2`/`G3`) is a size decision, not a correctness one.

## 2. Skinning on sloped surfaces — settled by print, 2026-11-11

`docs/ORACLE.md` shows two references agreeing with each other and disagreeing
with this engine by 12–16% on every part with a sloped face, at real infill
densities and not at 100%. The whole deficit sits in the skin class, because the
wall band already covers the staircase surface that both references cover with
solid infill on top of their perimeters.

**Resolved by hardware.** The torus printed at 15% infill with no pinholes, no
gaps between perimeter loops and no droop, and measured 32.00 mm outer diameter
and 8.000 mm tall, both nominal. The 16% volume difference is therefore the
references over-filling sloped surfaces, not this engine starving them. The
field-wall approach covers the staircase with wall beads and needs no skin mask
following the local surface. The oracle tolerances stay as the regression test;
the difference is now a documented efficiency feature, not a defect.

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

- **Bridging — done.** `crates/infill` subtracts the layer below from the
  current layer mask, treats the remainder as air-below cells, and prints them
  as one `Kind::Bridge` family. The direction is chosen per layer from
  0/45/90/135° by the fraction of lines anchored on material one layer down, so
  every bridge bead is pinned at both ends; `bridge_angle` can force one.
  `fixtures/bridge.stl` (two legs, a plate spanning 12 mm) and
  `fixtures/bridge30.stl` (30 mm) are the regressions: only the plate's first
  layer bridges, one axis, and the 30 mm span must run slower and fatter than
  the 12 mm one.
- **Bridge tuning — done.** `bridge_flow`, `bridge_density`,
  `bridge_line_width`, `bridge_angle`, `thick_bridges`, `bridge_fan_speed`,
  `bridge_acceleration` (Klipper: wrapped in `SAVE_VELOCITY_LIMIT` and
  `RESTORE_VELOCITY_LIMIT`) and `max_bridge_length` match the settings both
  reference slicers expose. Two they do not have: `bridge_span_flow` and
  `bridge_span_speed` scale flow and speed per line with the span past
  `bridge_span_ref`, because neither of them varies anything per span. The flow
  term is negative by default: a 30 mm span at 1.10× flow sagged in the middle
  on the Voron, so the bead is thinned with span instead
  (`fixtures/bridge_ladder.stl` measures the whole curve in one print).
  `bridge_cross_hatch` adds the pass neither slicer has: two strands crossing the
  gap corner to corner before the fill, so the fill spans material. Off by
  default; the strands sag on their own and have to earn their place.
- **The gap is bridged once, and only by bridge beads — fixed.**
  `infill_layer` subtracted the air cells from the skin mask and then reused that
  same mask inverted for sparse infill, which made every unsupported cell read as
  "not solid". Sparse infill therefore printed 45° lines straight across each
  gap at solid speed and full flow, under the bridge beads: double extrusion, and
  a full-flow diagonal pulling on a single anchor, which is what lifted the end
  leg of the ladder coupons twice. Sparse now has its own mask (`solid | air`),
  and `sparse_infill_never_prints_over_air` fails if it regresses.
- **Internal bridges.** Every span this engine detects is external: the air
  mask is `own - below`, so cells sitting over sparse infill are never bridge
  cells. OrcaSlicer's `internal_bridge_*` family (over infill, inside the part)
  needs a separate over-sparse classification before those keys mean anything.
- **Supports, stage 1 — done.** `crates/support` seeds from the angle test on
  the same masks the bridge pass reads: a cell seeds when the layer below does
  not reach under it by `h * tan(support_threshold_angle)`. Seeds are pruned by
  area, projected downward into columns, kept `support_xy_gap` clear of the part,
  offset `support_z_gap` below the contact, and capped with
  `support_interface_layers` dense interface layers. `support_on_build_plate_only`
  keeps only columns rooted at layer 0. Filled by `infill::fill_from_mask`,
  which derives lines from the mask itself — a support fill sits *outside* the
  part outline, so clipping it to the region would leave nothing. Emitted as
  `Support` / `Support interface`, ranked before the walls.
  Regressions: `fixtures/overhang.stl` (30 mm plate on an 8 mm post) and
  `fixtures/overhang_span.stl` (two posts, 40 mm span).
- **Supports, stage 2 — done.** `support_style = tree` samples contact tips at
  `support_branch_distance` across each overhang patch, drops each as a
  `support_tip_diameter` tip, and then, one layer down at a time: descends (coning
  out by `support_branch_diameter_angle`, deflecting in 45° steps around the part,
  dropping the branch when nothing is free), attracts toward the nearest branch by
  at most `height * tan(support_branch_angle)`, and fuses on contact with area
  conserved. Each branch prints as one closed bead plus sparse interior fill,
  dense within `support_interface_layers` of a contact.
- **Two tree traps, found the hard way.** (1) Attraction must be unbounded in
  range: under a hard `branch_distance` cutoff, clusters that drifted 5 mm apart
  never converged and the result was 30 parallel columns, not a tree. Distant
  pairs drift at a quarter step, so the lean limit still holds. (2) Fusion must be
  decided by contact, not centre distance — merging two thick trunks moves the
  centre further than one layer of lean, and is still printable because the two
  discs below carry the merged one.
- **Tree limitations.** No per-branch collision *search* beyond the 45° deflection,
  so a canopy directly above a wall gets a gap rather than a branch routed around
  it. Contact tips are isolated at `support_branch_distance` spacing, so the
  overhang between them is carried by bridging, not by support; that is deliberate
  and is what stage 3 generalises.
- **Supports, stage 3 — span-budgeted props.** See `docs/SUPPORTS.md`: supports are
  an interval problem, not an area problem. Once a span can be bridged up to a
  measured `L`, an overhang needs enough props that every free span stays `<= L`,
  which makes support volume scale with `1/L` instead of with overhang area. `L`
  is measured on the machine (>= 50 mm at 0.4 mm beads), not guessed, and the
  bridge pass claims every region it can already cross. No reference slicer does
  this: both decide support from geometry alone and never consult their own
  bridging capability.
- **Bed placement.** Models are sliced where they sit in the file. Add centring,
  `--arrange`, and a build-volume collision check.
- **Adaptive layer height.** The field already gives local surface slope cheaply.

## 6. Configuration

- Typed printer / filament / process profiles with PrusaSlicer-style inheritance.
- Per-object and per-region overrides (modifiers, height ranges).
- 3MF and AMF import, including plate and instance data.

## 7. Physical correctness

- **First physical print passed (2026-11-11).** `cube.gcode`, 100 layers at flow
  1.0, printed to completion on the Voron 2.4 (job `0003E7`, 16 min). Measured
  X, Y and Z all 20.00 mm, first layer and top surface clean, no pillowing. Flow
  1.0 stands; the +10 mm filament delta against the prediction is Klipper's
  `PRINT_START` purge line, which the engine does not predict and Moonraker does
  count.
- **Single-bead wall measures 0.39 mm at four heights** (coupon `wall.gcode`,
  1 wall per layer, 20 mm open tube, job completed 278.4 mm reported / 268.4 mm
  predicted after the purge). 2.5% under a 0.40 nominal is the expected result:
  calipers read the flat face of a bead whose section is a rounded rectangle.
  Corners closed, no gaps, tube square. Bead width, flow and pressure advance
  are all validated; leave flow at 1.0.
- Still unmeasured on hardware: elephant foot on the first layer, and any sloped
  or curved surface — the sphere is sliced and waiting.

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
