# HFW-final

A headless FFF slicing engine in Rust: STL in, printable G-code out.

It is the merge of the two prototypes in `../HFW-CLAUDE` (a variable-width wall
generator) and `../HFW-GPT` (a deterministic integer slicing pipeline). This one
goes end to end: a validated mesh, an exact integer slice, a variable-width bead
plan, a validated printer profile, and a G-code file that an independent
verifier re-reads and safety-checks before anyone prints it.

**Reference-priced.** The deposited volume is compared against OrcaSlicer and
PrusaSlicer on the same models at the same settings. At 100% infill the three
agree within 0.8–2.7%; at 15% infill flat parts agree within 3% and curved parts
differ, with the cause identified. See `docs/ORACLE.md`.

## Numbers

Material accounting is the strongest single test: fill every layer 100 % and the
deposited plastic must equal the model volume. Measured on this build, 0.4 mm
nozzle, 0.2 mm layers, `--density 1.0`:

| model | extruded / mesh volume |
|---|---:|
| 20 mm cube | 0.9963 |
| 30 mm sphere (16 128 triangles) | 0.9930 |
| torus | 0.9853 |

Those three figures are unchanged from the wall generator's published results,
which is the proof that the port through the new integer slicer is faithful.

## Build and verify

```fish
cargo build --release --workspace
./scripts/test-hfw.fish
```

`scripts/test-hfw.fish` runs formatting, Clippy with `-D warnings`, 94 tests, the
release build, and fourteen end-to-end CLI checks: layer counts, deterministic
output, the three material ratios, G-code emission and verification, profile
rejection, and the two-reference oracle when OrcaSlicer and PrusaSlicer are
installed.

## Use

```fish
# one layer, look at it
cargo run -p hfw-cli --release -- fixtures/cube.stl --z 10 --svg /tmp/layer.svg

# a full stack with diagnostics
cargo run -p hfw-cli --release -- fixtures/sphere.stl \
  --layer-height 0.2 --out-dir /tmp/sphere

# printable G-code for a Voron, checked before you trust it
cargo run -p hfw-cli --release -- fixtures/cube.stl \
  --profile profiles/voron_klipper.cfg --out-dir /tmp/cube --gcode /tmp/cube.gcode

# check a file from any slicer
cargo run -p hfw-cli --release -- \
  --check-gcode /tmp/cube.gcode --profile profiles/voron_klipper.cfg
```

Output per layer: an SVG where stroke width is the real bead width, the exact
model cross-section drawn as a dashed reference, and a `manifest.csv` row with
path counts, length, volume and start/stop count. `job_report.txt` adds mesh
diagnostics, timings, material totals and any skipped layers.

Run `hfw-cli --help` for the full option list: width, loop count, infill
density and pattern, skin layers, wall mode, sampling resolution, strictness,
printer profile and G-code output.

## What it does today

- ASCII and binary STL, vertex welding, topology diagnostics, opt-in repair.
- Transforms: per-axis scale, Euler rotation, translation, mirror-safe winding.
- Checked integer nanometre coordinates for slicing; open or branching contours
  are detected and reported, never invented.
- Layer schedule with a distinct first layer and a shortened final layer.
- Per-layer triangle bucketing with a bounded-memory fallback.
- Exact distance field, medial-axis extraction, variable-width walls, thin-wall
  centre beads, corner recovery, width-aware simplification.
- Top/bottom skin classification from a bounded ring of layer masks.
- Wall-clipped rectilinear and grid infill, at a density you ask for.
- Supports: overhang seeding by angle on the same masks the bridge pass uses,
  projected columns with XY/Z clearance, dense interface layers, build-plate-only
  mode; and tree supports that sample contact tips, lean at a bounded angle,
  fuse on contact and cone out into trunks. Stage 3 of `docs/SUPPORTS.md`
  (span-budgeted props) is next.
- Coverage, spill and width statistics against the model cross-section.
- Validated `key = value` printer profiles, with Voron/Klipper and generic
  Marlin examples in `profiles/`.
- G-code: seam placement, path ordering, retraction, z-hop, per-segment
  extrusion from the bead's own width, pressure advance, cooling, skirt, a
  solid filled brim, bridge detection with its own direction, speed and fan,
  the full bridge tuning set from both reference slicers plus per-span flow and
  speed compensation, bed placement and fit checks.
- An independent verifier that re-reads the written file and checks bed limits,
  feedrates, temperatures, heater-before-extrusion, Z monotonicity, volumetric
  flow, extrusion-width plausibility and retraction balance.

## What it does not do

No span-budgeted props yet (stage 3),
no multi-material, no toolchanger, no acceleration model, no 3MF/AMF/OBJ, no
adaptive layer height.
The brim is first-layer only and has no `brim_ears`. `docs/ROADMAP.md` ranks the
gap, and `docs/ORACLE.md` records the measured difference to two established
slicers.

## Licence

MIT OR Apache-2.0, at your option — `LICENSE-MIT`, `LICENSE-APACHE`.

No third-party source was copied. Both parent prototypes are original work
written for this project, and neither is a translation of Slic3r or any other
slicer. See `MIGRATION.md` for what was carried over and how the provenance was
checked.
