# Physical print tests

The oracle compares G-code and deposited volume. It cannot see a seam, a corner or
a bridge. This is the plan for the part a human has to judge, and the score sheet
to write the result on. **No claim of print equivalence is made until the sheet is
filled in.**

Same printer (Voron), same filament, same profile (`profiles/voron_klipper.cfg`
plus the machine's own start G-code), same layer height and temperatures. Slice
each model twice: once with HFW, once with OrcaSlicer at matched settings. Print
both, label them, and inspect without knowing which is which where possible.

## Models and what each one tests

| model | tests | what to look at |
|---|---|---|
| Benchy | everything at once, coarse | hull line, chimney, overhang underside, bow, top surface |
| 20 mm calibration cube | dimensional accuracy, corners | calipers on X/Y/Z, corner sharpness, top surface finish |
| thin-wall tower: ribs 0.3 / 0.4 / 0.5 / 0.8 / 1.2 mm | sub-bead widths, the gap pass | every rib present and continuous, no doubled strand, no groove beside it |
| overhang test 20–70° | bridge/overhang speed and cooling | sag under each angle, curl at 60–70° |
| bridge span 10 / 20 / 40 mm | bridge direction and fan | underside strands straight and unbroken, sag measured at mid-span |
| stress gear (`fixtures/stress_gear.stl`, 6 mm) | wall coverage, small holes, thin rib | the 0.5 mm rib present, tooth tips sharp, 5 mm bore round |
| Revolver plate (if printed) | plate-scale behaviour | first layer across all 18 parts, no stringing between parts |

## Checklist per pair

1. **Wall gaps** — hold up to a light; any gap between perimeters. Score 0/1/2.
2. **Corner sharpness** — tooth tips and cube corners; bevelled? rounded? Score 0/1/2.
3. **Seam** — visually locate it; is it aligned layer to layer, is it a ridge or a
   scar. Score 0/1/2 and note its position.
4. **Top surface** — ripples, gaps between lines, pillowing. Score 0/1/2.
5. **Bridge sag** — measure the lowest point under the 20 mm bridge with calipers.
6. **Dimensional accuracy** — cube X and Y at mid-height, Z overall.
7. **Sub-bead ribs** — present, continuous, and does the width look right for each.
8. **Stringing** — hairs between separate parts on the plate. Score 0/1/2.

## Score sheet

Write one block per model per engine into `docs/real-prints.md`:

```
model:        <name>
engine:       HFW <commit>  |  OrcaSlicer <version>
profile:      <file and the values that differ from the other run>
filament:     <brand, colour, batch>
nozzle/temps: <nozzle, hotend, bed, fan>
result:
  wall gaps          0|1|2   <observation>
  corner sharpness   0|1|2   <observation>
  seam               0|1|2   <observation and position>
  top surface        0|1|2   <observation>
  bridge sag         <mm>    <span tested>
  dimensions         X <mm>  Y <mm>  Z <mm>
  sub-bead ribs      <pass/fail per width>
  stringing          0|1|2   <observation>
  photo              <path>
```

A difference is only a finding when it is visible and repeatable; note it once and
move on. Anything HFW loses by a visible margin becomes an issue with the print as
its evidence, not a guess from the G-code.
