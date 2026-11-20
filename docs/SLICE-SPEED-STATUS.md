# Slice speed and parity: where HFW stands, 2026-11-19

Supersedes the plan and status sections of `docs/archive/SLICE-SPEED-BRIEF.md` and closes
`docs/archive/OFFSET-BRIEF.md`. Every number here was measured on this machine, in a release
build, with the commands given.

## Where the clock stands after the fifth pass, 2026-11-19

All of the figures below this section that say 10.8 s for the plate were measured
before 33b9eb9 replaced the offset ladder with a rasterised distance field. That change
made walls area-proportional and the plate slower; this pass reversed it and took the
two phases that then dominated. Measured again on the current build:

| phase, Revolver plate, 16 threads | ms |
|---|---:|
| slicing | 642 |
| compensating | 814 |
| walls | 2,736 of work in 172 of wall clock |
| infill | 2,289 |
| G-code render, write and verify (29 MB) | ~8,800 |
| **total, wall clock** | **15,758** |

```
target/release/hfw-cli "Revolver All Parts.3mf" --profile profiles/voron_klipper.cfg \
    --gcode plate.gcode --out-dir plate
```

G-code rendering is the phase left above a second, and it is single-threaded. On the
150-layer stress gear at one thread, where the same work is 3.4 MB, the whole run is
2.75 s:

```
python3 scripts/make_stress_part.py 30 /tmp/g30.stl
HFW_THREADS=1 target/release/hfw-cli /tmp/g30.stl --profile profiles/voron_klipper.cfg \
    --gcode /tmp/g30.gcode --out-dir /tmp/g30
```

| 150-layer gear, one thread | before this pass | after |
|---|---:|---:|
| walls ms | 2,247 | 127 |
| infill ms | 6,089 | 744 |
| G-code generate ms | 4,517 | 1,249 |
| total s | 13.88 | 2.75 |

`scripts/wallbench.fish` reproduces the whole table, and `scripts/wallcov.py` the
coverage it was bought with: 4.39% of the wall band uncovered and a 0.5 mm rib missing
before, 0.38% and the rib covered after.

## The three slicers on the Revolver plate

`Revolver All Parts.3mf`: 18 separate mechanical parts, 450 layers at 0.2 mm, plate
footprint 195 x 197 mm (one part, "Body Lock", hangs 5.7 mm below the bed as
authored — see the placement note below).

| slicer | wall clock | filament | extruded volume |
|---|---:|---:|---:|
| PrusaSlicer 2.9.7, Voron 2.4 350 profile | 9.4 s | 34368 mm | 82665 mm³ |
| OrcaSlicer 2.4.2, Voron 2.4 350 machine | 14.4 s | 33048 mm | 79490 mm³ |
| **HFW, 2 loops (the shipped profile)** | **10.8 s** (before 33b9eb9; 15.8 s now) | 28789 mm | 69245 mm³ |
| **HFW, 3 loops (matched to the two runs above)** | **11.6 s** (before 33b9eb9) | 31053 mm | 74691 mm³ |

Layer height, 0.4 mm lines, 5 top and 4 bottom solid layers and 15% rectilinear
infill are equal across all three; the perimeter count is not, and that is a
correction to the earlier version of this document, which claimed three perimeters
for a run that used `profiles/voron_klipper.cfg` and its `walls = 2`. The reference
runs were given three, so **three loops is the matched comparison**: HFW is then
**−7.9% against the reference mean**, 2.2 s behind PrusaSlicer and 2.0 s ahead of
OrcaSlicer. The -3.5% this document carried before the skin and walk fixes was
bought by printing a bead on top of the inner perimeter; see the defect section
below.

The plate's own volume is 118298 mm³, so the three slicers deposit 70%, 67% and
63% of it respectively.

**The 6.3 s PrusaSlicer figure in `archive/SLICE-SPEED-BRIEF.md` was wrong.** It came from
a run against PrusaSlicer's default bed, which is smaller than this plate: 19566 mm
of filament against OrcaSlicer's 33047 says plainly that parts were dropped. With a
350 mm machine and the same settings PrusaSlicer takes 9.4 s.

Commands, for reproducing:

```
prusa-slicer --load /tmp/voron350.ini --export-gcode --output plate_prusa.gcode \
    "Revolver All Parts.3mf"
orca-slicer --slice 0 --outputdir /tmp/orcaout --load-settings m.json\;p.json \
    "Revolver All Parts.3mf"
target/release/hfw-cli "Revolver All Parts.3mf" --profile profiles/voron_klipper.cfg \
    --gcode plate.gcode
```

## Where HFW's 10.8 s goes

From `job_report.txt` on a run with `--out-dir`, 16 threads:

| phase | wall | CPU |
|---|---:|---:|
| infill | 3.75 s | not counted |
| G-code formatting and writing (34.6 MB) | ~3.6 s | — |
| wall generation | **1.04 s** | **16.6 s** |
| compensating (closing and offset) | 0.82 s | — |
| verification | 0.75 s | — |
| slicing | 0.64 s | — |
| **total** | **10.8 s** | — |

Both heavy phases are already parallel across layers (`dynamic_map`), so the wall
clock is total work divided by 16 minus the imbalance from the slowest layer. The
slowest layer is layer 3 at 215.8 ms of wall work on its own; the average is 37 ms.

Before the rewrite the same table read: walls 9.07 s of wall clock and 144.4 s of
CPU, infill 8.97 s, G-code 58 MB and ~3.7 s, total 24.0 s. Walls are now 8.7x
cheaper and the file is 40% smaller.

That factor is not the algorithm on its own, and the difference matters for anyone
who touches this again. Polygon walls are proportional to *vertex count*, so run
against the unsimplified contours they are worse than the raster, not better:

| wall phase, Revolver plate | wall clock | CPU | slowest layer |
|---|---:|---:|---:|
| raster (before) | 9.07 s | 144.4 s | 3.76 s |
| polygons, slice simplification off | 10.18 s | 162.8 s | 9.61 s |
| polygons, slice simplification at 0.012 mm | **1.04 s** | **16.6 s** | 0.22 s |

The raster was area-proportional and barely noticed 28 000 vertices in a layer. The
offsets pay for every one of them, eight offsets deep. Simplifying to PrusaSlicer's
own default of 0.012 mm is what turned the rewrite into a win, and it cost 0.1% of
the plate's volume.

## The wall rewrite

Walls used to be rasterised: a distance field sampled over every part at `h`
millimetres, classified cell by cell, traced into iso-lines, and the medial axis
walked for the ridge pass. That is proportional to *area*, and a plate is mostly
area — 144 s of CPU for 450 layers, with a 0.067 mm staircase in every bead and
12 µm between vertices in the G-code.

They are now polygon offsets, the way PrusaSlicer, OrcaSlicer and Cura generate
them:

- **Loops.** Loop `i` is the layer offset inward by `Params::loop_offset(i)` with
  the kernel offset (`crates/kernel/src/offset.rs`, proven against Clipper2 to
  7e-6 relative area) and a miter join, which is what keeps a visible corner on.
  A straight edge comes out as two segments, not two hundred.
- **Coverage.** A loop's bead is the band one bead wide centred on its path, and
  the path is the *boundary* of the eroded layer, not the eroded layer itself. The
  kernel expresses that band as "grew the eroded layer by half a bead, minus shrank
  it by half a bead", in the distance sense — miter joins, so the corner square the
  two beads overlap in counts as covered. Subtracting each band in turn leaves the
  material no loop reaches.
- **Gaps.** Walking that leftover boundary, a ray goes along the inward normal until
  the far side is nearer than the side it started from. That crossing is half the
  gap across, so a strand there, as wide as the whole gap, fills it without reaching
  into the wall. The test needs the far wall to be roughly *parallel* — at a corner
  of a wide region the neighbouring edge is within a bead and would otherwise read
  as a gap. Both sides of one gap propose the same strand, so proposals are claimed
  by cell in arc-length order and the second one is dropped.
- **The floor.** `min_feature` is enforced by opening the layer (erode by half the
  floor, grow it back, both miter), which removes features narrower than the floor
  and nothing else. It only runs when the floor is wider than half a bead, because
  below that it cannot remove anything the ladder would have printed.

Deleted with the field: `walls/src/ridge.rs` (503 lines: medial-axis extraction,
spur pruning, bead-count hysteresis), the distance-field struct, the cell
classifier, the contour-to-loop path with its Newton projection and corner
reinsertion, and five parameters that only the beading read (`central_deg`,
`smooth`, `spur`, `ext`, `taper_min`, `trans_len`, `hyst`). The `WallsError` type
went with them: nothing in the new path can fail.

**One change outside `walls` was needed to make it fast.** The slice simplifier's
default was 0, so a layer arrived with 12 000 to 28 000 vertices and every offset
paid for all of them: the first polygon version took 53 s. PrusaSlicer's `resolution`
default is 0.012 mm, and a 0.4 mm bead cannot show a 12 µm step, so that is the new
default. Layers now arrive with 3 000 to 3 800 vertices, offsets cost 2-5 ms each,
and the volume moved by 0.1% on the plate while the G-code shrank by 40%.

What is left of the old machinery: the adhesion brim still rasterises its band row
by row (first layer only, 0.1 s of the plate), and `walls::offset_region` — the
whole-region grow and shrink the infill and the size compensation call — is now a
kernel offset too, which is why infill dropped from 8.97 s to 3.75 s.

## What still limits the total

Infill, at 3.75 s, still works off a raster mask cache; it is now the largest single
phase and the next candidate for the same treatment. G-code writing at ~3.6 s for
34.6 MB is the second. Neither is a wall problem any more.

## Parity: geometry and volume

### Deposited volume on the fixtures

Each fixture sliced by all three at matched settings (`scripts/oracle.py`), in the
build with both defects below fixed:

| fixture | HFW | Orca | Prusa | HFW vs mean |
|---|---:|---:|---:|---:|
| `cube.stl` | 3266.5 | 3284.4 | 3307.6 | -0.9% |
| `sphere.stl` | 4459.8 | 4891.1 | 4990.3 | -9.7% |
| `cone.stl` | 2937.2 | 3157.9 | 3353.1 | -9.8% |
| `torus.stl` | 2229.9 | 2255.3 | 2328.1 | -2.7% |

All four are inside the oracle's 10% tolerance; before the two fixes below, three of
them were not. The plate stands at -7.9% at the matched three loops.

The residual on the sphere and the cone sits in the class split as much as in the
total: the references print about a loop's worth of "Solid infill" around the
perimeter of every layer whose surface curves outward -- on a sphere that is 193
fragments averaging 0.58 mm on layer 10, which is gap fill of the band, tagged
solid -- while this engine's loops tile that band and leave nothing to fill. Chasing
the last few per cent there would mean laying strands over beads already printed,
the trade this document rejected for the walk budget.

### Where the material was going, and what was fixed

Measured per class on plate parts extracted to their own STL and sliced by all
three at matched settings. Before the fix below:

| class | HFW | Orca | Prusa | HFW vs mean |
|---|---:|---:|---:|---:|
| wall (part 4) | 7720 | 6957 | 7481 | +6.9% |
| skin (part 4) | 3021 | 4491 | 5884 | -41.8% |
| sparse (part 4) | 1841 | 918 | 777 | +117% |
| wall (part 14) | 961 | 920 | 946 | +3.0% |
| skin (part 14) | 609 | 319 | 819 | +7.0% |
| sparse (part 14) | 56 | 136 | 34 | -34.5% |

Walls agree to within 7%. The "skin" and "sparse" columns are partly the same
material counted differently: PrusaSlicer tags the gap fill of a thin feature as
`Solid infill`, so on a part full of thin ribs its skin total carries material HFW
books as `Gap infill` and as sparse. The totals are the numbers to trust here, and
they are why the pinned constants in `solid_infill_reproduces_the_published_volume_ratios`
were re-measured rather than adjusted: at 100% density the cube, sphere and torus now
deposit 1.007, 1.006 and 1.023 of their own volume, because skin overlaps the
innermost loop by the configured overlap exactly as the references' fill does, and
this engine lands within 1% of OrcaSlicer on all three.

**One cause found and fixed: `solid_infill_below_area` was measured on the part
outline instead of on the fill area.** Upstream applies that threshold to its fill
surfaces, which are the layer after the perimeters come off; HFW applied it to the
ring areas, so a part 3 mm wide and 30 mm long scored 90 mm² as an outline and was
filled sparse at 15%, where PrusaSlicer scored the 1.6 by 30 mm strip left inside
the walls and filled it solid. `small_island_bits` now runs on the inset region
(`inset_region` in `crates/infill/src/lib.rs`), the same region the sparse mask is
drawn on. Effect, three-way oracle at 15% density:

| model | before | after the island fix | after the skin and walk fixes |
|---|---:|---:|---:|
| Revolver plate (2 loops) | -11.0% | -5.9% | -10.8% |
| Revolver plate (3 loops, matched) | — | -7.9% | -7.9% |
| cube | -2.8% | -1.6% | -0.9% |
| sphere | -11.9% | -10.6% | -9.7% |
| cone | -14.9% | -10.3% | -9.8% |
| torus | +16.6% | +22.7% | -2.7% |

The two per-part figures in the class table above were measured before the skin and
walk fixes and were not re-measured with them; the extracted-part harness is not in
`scripts/`, and the fixtures cover the same ground.

The plate at two loops moved *away* from the reference mean when the walk was
budgeted, because the walk had been printing a bead on top of the inner perimeter;
at the matched three loops it is -7.9% either way, and the fixtures — which the
oracle gates on — are all inside tolerance for the first time.

The rule is covered by `the_area_threshold_is_measured_inside_the_wall_band` in
`crates/infill`: a 50 by 3.2 mm island is 160 mm² of outline and 64 mm² of fill, and
it has to print solid. The test fails on the old measurement.

### The two defects of this section, both fixed

1. **Skin was clipped at the sparse fill's inset.** `family()` rejected any point
   within `inner_offset` of the boundary for solid lines as well as sparse ones, so
   a top surface narrower than twice that inset had no room for a line at all and
   was left to the perimeters — the staircase annulus on a curved part, and the
   -10% on sphere and cone. Skin now has its own inset,
   `InfillParams::skin_offset`: the shell's inner edge less the overlap, which is
   where PrusaSlicer and OrcaSlicer clip their fill. Measured, joined by the walk
   fix below:

   | model | before | after |
   |---|---:|---:|
   | plate (3 loops) | -7.9% | -7.9% |
   | cube | -1.6% | **-0.9%** |
   | sphere | -10.6% | **-9.7%** |
   | cone | -10.3% | **-9.8%** |
   | torus | +22.7% | **-2.7%** |

   At 100% density the three fixtures now land within 1% of OrcaSlicer
   (`solid_infill_reproduces_the_published_volume_ratios`).

2. **The shell walk chained around a closed shell.** `join_shell_ends` ties sparse
   line ends together with a bead walked along the wall, capped only by
   `anchor_max` (50 mm), so on a torus every line end found a neighbour along the
   shell and a run snaked the whole way round: 1017 mm³ of walk against 305 mm³ of
   sparse fill. Removing the walk altogether over-corrects (plate -24%), because the
   walk is what fills the half-bead strip between the last loop and where the fill
   may start. It is now budgeted per layer and per family at `WALK_SHARE` (1.0) times
   the length of the fill lines it connects: a layer may spend as much wall as it has
   fill, no more. Measured against that share:

   | walk share | torus | plate (3 loops) |
   |---|---:|---:|
   | 0.5 | -5.7% | -8.2% |
   | **1.0** | **-2.7%** | **-7.9%** |
   | 2.0 | +15.3% | -0.2% |

   Two per cent of the plate's volume is therefore the price of not printing a bead
   on top of the inner perimeter wherever a closed shell allows it; before the
   budget, the plate was closer to parity only because the wall was being paid for
   twice.

Both are covered by the fixtures: `scripts/oracle.py fixtures/{cube,sphere,cone,torus}.stl`
passes on all four against the two references, which it did not before.

### A writer statistic that disagreed with the file

`the_writer_counts_the_same_crossings_the_verifier_finds` failed once skin beads
began to start on the wall line: the writer counted 422 travel crossings where its
own verifier found 429. The cause was a precision mismatch, not geometry — the
writer tests crossings against walls snapped to the machine's `gcode_resolution`
(0.0125 mm) while the file carries three decimals, and a travel that starts on a
wall is exactly where a sub-micron difference flips a strict straddle test. The
writer now records the walls it draws on the *printed* grid and counts against
those, formatting through a stack buffer so the cost stays off the hot path. The two
counts agree exactly, on the torus and on the cube.

### Closing and miter joins against Clipper2

`crates/kernel/tests/clipper2_parity.rs` compares the kernel's offset against
`clipper2-rust` — a direct port of the library PrusaSlicer, OrcaSlicer and Cura
run — on squares, circles, annuli, hairline cracks and 80 ring sets sampled from
four plate layers (2 to 43 rings each). Agreement on area, both join styles:

- plate layers: worst 7e-6 relative (0.0007%), at miter limit 3 and
  `arc_tolerance` 0.002 mm;
- squares and circles: within 0.2%;
- topology agrees on every case: a hole narrower than twice the radius vanishes in
  both, a crack narrower than twice the radius welds in both, and a crack wider
  stays open in both.

So the offset's join behaviour matches what PrusaSlicer's own offset does at the
same parameters. The miter limit is 3, which is PrusaSlicer's default.

## Two placement bugs found and fixed today

Both were real, both changed what the plate printed:

1. **The plate was lifted to fit the object that hangs below the bed.** The plate
   as authored has "Body Lock" reaching z = −5.711 mm. HFW sliced the stack from
   the model's own minimum, which moved *every* part 5.711 mm up and produced a
   95.55 mm tall plate — 478 layers where the file has 449. PrusaSlicer lifts only
   the offending object and leaves the rest alone. HFW now does the same
   (`seat_on_bed` in `crates/cli/src/main.rs`), warns about it, and the plate comes
   out 89.84 mm tall in 450 layers.
2. **The plate was re-centred on the bed.** The writer centred every model's
   bounding box, which is right for a lone STL and wrong for a plate whose layout
   the file authored: HFW printed at X 73..272 where the file says X 33..229, while
   PrusaSlicer printed exactly where the file put it. `PrinterConfig::keep_position`
   now stops that for a plate the CLI knows was positioned.

After both, HFW's G-code and PrusaSlicer's occupy the same coordinates: HFW
29.20..228.59 (the low end is the skirt), PrusaSlicer 33.73..228.59.

## Verified

Workspace: **417 tests pass, 0 fail, 0 ignored.**
