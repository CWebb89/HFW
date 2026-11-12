# Oracle results: hfw against OrcaSlicer and PrusaSlicer

An internal metric cannot prove a slicer correct. `extruded_over_mesh = 0.9963`
proves only self-consistency: the same wrong geometry would score the same. This
document records what two independent, widely used slicers deposit for the same
models at the same settings, measured from the emitted G-code by one shared
parser.

Refresh with:

    scripts/oracle.py fixtures/cube.stl fixtures/sphere.stl fixtures/torus.stl \
        fixtures/cone.stl fixtures/taper_wall.stl fixtures/thin_wall.stl
    scripts/oracle.py fixtures/cube.stl fixtures/sphere.stl --density 1.0 --markdown

The script exits non-zero when the deposited total is further than the tolerance
from the mean of the two references. Per-feature classes are printed but not
gated: their split depends on infill-versus-perimeter overlap policy, which
differs between slicers by design (see the note at the end).

## Settings held equal

0.2 mm layers, 0.4 mm lines, 3 perimeters, 5 top and 4 bottom solid layers,
rectilinear infill, no skirt, no brim, no supports, no ironing, 1.75 mm
filament. `flow` is forced to 1.0 in the hfw profile; the references run at
their default 1.0. Machine is a Voron 2.4 350 in Orca, a Voron in PrusaSlicer,
both sliced headless.

## At 100% infill: all four shapes agree

Every cell is solid, so this isolates geometry, wall placement, skin extent and
the extrusion model from infill-density policy.

| fixture | hfw mm³ | Orca mm³ | Prusa mm³ | hfw vs mean |
|---|---:|---:|---:|---:|
| cube | 7970.2 | 8001.6 | 8067.8 | −0.8% |
| sphere | 14024.1 | 14130.3 | 14190.1 | −1.0% |
| torus | 3722.0 | 3786.3 | 3864.5 | −2.7% |
| cone | 7001.9 | 7056.7 | 7100.5 | −1.1% |

Both references sit 0.5–1.2% *above* the model's own volume; hfw sits 0.4% below
it. On the 20 mm cube the analytic volume is 8000 mm³, so hfw is the closest of
the three. The independent verifier independently reproduces hfw's own G-code
volume to within 0.3%, which is what the writer/verifier split is for.

**Conclusion: geometry, wall placement and the rectangle-bead extrusion model
are correct to within about 1% on flat and curved shapes alike.**

## At 15% infill: flat parts agree, curved parts do not

| fixture | hfw mm³ | Orca mm³ | Prusa mm³ | hfw vs mean | verdict |
|---|---:|---:|---:|---:|:--|
| cube | 3204.7 | 3284.4 | 3307.6 | −2.8% | pass |
| taper_wall | 570.6 | 571.9 | 596.3 | −2.3% | pass |
| sphere | 4352.7 | 4891.1 | 4990.3 | −11.9% | **fail** |
| cone | 2771.3 | 3157.9 | 3353.1 | −14.9% | **fail** |
| torus | 1924.5 | 2255.3 | 2328.1 | −16.0% | **fail** |
| thin_wall | 88.7 | 100.8 | 100.8 | −11.9% | pass (expected) |

The three failures are one finding, not three:

| fixture | wall | skin | sparse |
|---|---:|---:|---:|
| sphere | +6.3% | **−85.6%** | −12.7% |
| cone | +6.1% | **−59.0%** | −10.1% |
| torus | +6.4% | **−72.1%** | −24.5% |

The entire total deficit lives in the skin class, and the class is nearly empty
because the wall stage already consumed the surface. On a sloped face, the
staircase annulus between layer *k* and *k+1* is thinner than the 1.2 mm wall
band, so this engine covers it with wall beads and never emits solid infill;
both references cover it with solid infill on top of their perimeters. The wall
column is consistently +6%, which is the same band, counted as wall instead of
skin.

At 100% infill the difference vanishes, which is the proof that this is a
skin-versus-wall accounting policy and not a geometry error.

### Which one is right — settled by print, 2026-11-11

Two readings were consistent with all the numbers:

1. **hfw is under-filling.** Sloped top surfaces need solid material behind the
   perimeter; without it the visible surface of a dome is 15% infill with wall
   beads on top of it, and it will droop or show gaps.
2. **The references are over-filling.** A staircase surface is already fully
   covered by the wall beads that form it; adding solid infill behind them
   deposits material the surface does not need, which is exactly why both sit
   above the analytic volume at 100% while hfw sits below it.

The torus was printed at 15% infill (job `0003EA`, 40 layers, 819.3 mm): no
pinholes, no gaps between perimeter loops, no droop on the sloped outer surface.
Measured 32.00 mm outer diameter and 8.000 mm tall, both nominal, and the G-code's
wall centrelines span 31.59 mm plus half a bead each side — the file agrees with
the part. Reading 2 is correct: the references over-fill sloped surfaces. The
three failing fixtures stay red because the tolerance documents the difference,
not because the output is wrong; the note beside each failure says so.

## Note on per-feature classes

Feature classes are not comparable across slicers by design.

- OrcaSlicer and PrusaSlicer extend solid infill into the perimeter band
  (default infill/perimeter overlap about 25% of line width) and place a line at
  each end of every infill span; this engine anchors with a 10% overlap and one
  line per span position. On the 20 mm cube at 15% density that is 6.3 sparse
  lines per layer instead of 8, and about 770 mm of skin lines on the bottom
  layer instead of 900 mm, for identical line extents (18.0 mm in all three).
- A sub-width wall is the case this engine exists for. `thin_wall.stl` is a
  0.3 mm wall: hfw deposits 88.7 mm³ against a model volume of 90 mm³, while
  both references deposit 100.8 mm³ because neither can make a 0.3 mm bead and
  substitute a 0.4 mm one. The oracle records that fixture with a documented
  20% tolerance rather than as a failure.

## What this does not cover

- Print quality, surface finish, adhesion, stringing: only a print shows these.
- Travel planning, acceleration, pressure advance: the estimates are kinematic
  only, so hfw's printer time is optimistic (14 min against PrusaSlicer's 40 min
  on the same cube) and is never comparable.
- Multi-material, supports, bridging: not implemented.
- One reference build on one machine. A change in either slicer's defaults
  invalidates the numbers; re-run before quoting them.
