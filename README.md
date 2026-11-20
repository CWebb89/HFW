# HFW

A headless FFF slicing engine in Rust: a model in, printable G-code out, plus an
independent verifier that re-reads the file and checks it before anyone prints it.

It is the merge of two earlier prototypes (a variable-width wall generator and an
integer slicing pipeline) and it is the engine next-layer slices with. Since
2026-11-18 that is the only engine the application has: the plate is handed over,
verified G-code comes back, and there is no switch and no fallback
(`next-layer/src/qt/src/HfwEngine.cpp`). What is left is the rest of that
library's job — the plate model, settings, preview, project files — which is
planned in `next-layer/docs/rust-engine-plan.md` and is Rust work here, not a
reason to keep the C++ engine.

## Read this first

| document | answers |
|---|---|
| `docs/START-HERE.md` | the whole picture — both projects, how they fit, what to run, how to work on this with the agent |
| `docs/README.md` | what every other document is for, and which ones are history |
| `docs/STATUS.md` | where the project stands, in two minutes, with the numbers |
| `docs/ROADMAP.md` | the order of work and why it is that order |
| `docs/ENGINE-PARITY.md` | the capability checklist, each item with how it is measured |
| `docs/SETTINGS-LEDGER.md` | every setting, applied or not, generated from the reference dialect |
| `docs/ORACLE.md` | measured agreement with OrcaSlicer and PrusaSlicer |
| `docs/EXPLAINED.md` | how each stage works, without jargon |

## Build and check

```fish
cargo build --release --workspace
fish scripts/test-hfw.fish
```

The gate runs formatting, Clippy with `-D warnings`, 325 unit and integration
tests, the release build, and 76 end-to-end sections that run the real binary and
read the real output: layer counts, determinism, material ratios, every infill
pattern, the verifier, profile rejection, and the reference comparison when both
other slicers are installed. It ends with `ALL HFW ENGINE CHECKS PASSED.`

## Use

```fish
# one layer, as an SVG where stroke width is the real bead width
hfw fixtures/cube.stl --z 10 --svg /tmp/layer.svg

# a full stack with a per-layer report
hfw fixtures/sphere.stl --layer-height 0.2 --out-dir /tmp/sphere

# printable G-code for a Voron, verified before you trust it
hfw fixtures/cube.stl --profile profiles/voron_klipper.cfg --gcode /tmp/cube.gcode

# a plate: several STLs, or a 3MF whose build items carry their own placement
hfw a.stl b.stl project.3mf --profile profiles/voron_klipper.cfg --gcode plate.gcode

# a whole profile collection: pick one machine, process and filament
hfw --bundle /opt/orca-slicer/resources/profiles --list-bundle
hfw part.stl --bundle /opt/orca-slicer/resources/profiles \
    --machine "Voron 2.4 350 0.4 nozzle" --process "0.20mm Standard @Voron" \
    --filament "Generic PLA" --gcode part.gcode

# check a file from any slicer
hfw --check-gcode /tmp/cube.gcode --profile profiles/voron_klipper.cfg

# the same job as JSON, for a caller rather than a person
hfw fixtures/cube.stl --profile profiles/voron_klipper.cfg --json --gcode /tmp/cube.gcode
```

`--json` writes one document to stdout (schema, mesh counts, the settings actually
applied, per-layer counts and volumes, the file's totals, the verifier's verdict)
and sends prose to stderr, so a caller never scrapes text. `hfw --help` lists the
rest.

## What works today

One extruder, one material, STL and 3MF, single and multi-object plates:

- **Slicing** on exact integer nanometres; open or branching contours are
  reported, never invented.
- **Walls** as beads divided from the layer's measured section, so a feature narrower
  than the perimeters it asked for is filled by its own beads rather than by a strand
  beside them; a classic offset ladder is kept for comparison, and a gap pass remains
  as the fallback for a section whose middle bead is a ridge rather than a level set.
- **Infill**: twelve patterns — rectilinear, line, grid, triangle, cubic, gyroid,
  3d honeycomb, concentric, zig-zag, cross-hatch, honeycomb, monotonic — a
  per-island solid threshold, ironing, and a two-line anchor that joins fill lines
  by walking the wall.
- **Supports**: normal and tree bodies, interface layers, build-plate-only mode,
  and bridging that skips a span already anchored at both ends.
- **Surface**: scarf seams, staggered inner seams, seam gap, fuzzy skin, brims
  including hole brims, raft, draft shield.
- **Emission**: Klipper and Marlin dialects, machine limits, pressure advance,
  per-feature cooling and acceleration, arcs, thumbnails, pauses, substitutions, and
  a time estimate planned move by move with the machine's own cornering model.
- **Plates**: several objects packed onto the bed, a quarter turn allowed, centred,
  and refused before slicing when they cannot fit. Preset bundles (a vendor profile
  tree or a sectioned vendor file) resolve to one machine, one process, one filament.

`docs/STATUS.md` holds the numbers; `docs/SETTINGS-LEDGER.md` holds the setting by
setting truth, including what is missing and why.

## What it does not do

Multi-material and toolchangers. OBJ/AMF/STEP. Support painting, per-object regions
and layer-height profiles, and internal bridges. A machine preset's start G-code is
templated with the reference placeholders (`[bed_temperature_initial_layer_single]`
and the like); every one of them is substituted at intake or at emission, and a
placeholder that nothing can fill now fails the verifier instead of reaching the
printer as text.

Each of those is on the parity list with a cost beside it, except multi-material,
which waits for hardware.

## Licence

MIT OR Apache-2.0, at your option — `LICENSE-MIT`, `LICENSE-APACHE`. No
third-party source was copied; `MIGRATION.md` records what came from the two
prototypes and how provenance was checked.
