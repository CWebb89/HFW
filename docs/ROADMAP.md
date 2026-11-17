# Roadmap

The order of work, so the next action is never a question. State as of
2026-11-17; the numbers here are regenerated, never typed by hand where a script
can produce them.

Related: `STATUS.md` for the plain-language summary, `ENGINE-PARITY.md` for the
gate list the swap depends on, `SETTINGS-LEDGER.md` for the per-setting
inventory (generated), `SETTINGS-GAPS.md` for what is left and what it costs
(generated).

## What this project is for

Two jobs, in this order:

1. **Be the engine next-layer ships.** That is the reason the project exists.
2. **Be a reference-quality slicer on its own**, so it can be checked by printing
   and compared against OrcaSlicer and PrusaSlicer without either being present.

The settings ledger exists to make job 1 measurable: next-layer's rule is that no
control may be shown for a setting the engine ignores, so the ledger has to be
true before the engines meet.

## Standing

**383 engine settings, 364 applicable, 299 applied (82%), 6 partial, 59 missing,
19 n/a.** Outside the buckets a one-extruder machine cannot use, 299 of 307 is
applied (97%).

**Finish line, restated 2026-11-17:** every row is *applied*, *n/a*, or *blocked
on hardware* with the reason written beside it. The old wording ("100% applied")
was unreachable while 57 rows wait for a toolchanger, and a finish line nobody
can reach stops being read.

## The order of work

### 1. Land what exists (now)

The tree has been worked on for weeks without a commit. Nothing else should start
until it is committed in logical pieces, with a remote decided.

### 2. Finish the ledger tail that is real work

Seven rows. In cost order:

| row | what it costs |
|---|---|
| `slicing_mode` | the even-odd fill rule and small-hole closing at slice time; the largest of the four and the only one that may need a region boolean |
| `fill_pattern` | everything but lightning is drawn (`docs/EXPLAINED.md`); lightning is a spanning tree grown down from the top surface, worth a week on its own |
| `support_material_style` | snug is a boundary-following variant of the grid body; organic is a second tree generator |
| `avoid_crossing_perimeters` | routes around this layer's own loops today; routing outside the part needs an external graph |
| `ensure_vertical_shell_thickness` | `disabled` narrows the solid window; `partial` and `enabled` coincide because the difference upstream is a void region a coverage grid cannot express |
| `max_volumetric_extrusion_rate_slope_positive` / `_negative` | solved per bead path against its widest section; upstream splits the move, which is what closes the remaining boundary step |

The 57 multi-material rows are **not** in this list. They become work when a
toolchanger exists.

### 3. Work the engine parity list, top to bottom

`ENGINE-PARITY.md` §1–§9 plus §10–§11. It is ordered by what blocks a real
project in the app, not by what is interesting. Nothing in the ledger appears on
it, which is the clearest sign the ledger is no longer the main path.

### 4. Swap behind a flag, in the app, not here

Boundary first in next-layer: a project in, G-code and a report out. Then both
engines linked, HFW behind a flag, every gate measured on real projects. Then the
default flips. Deleting libslic3r is the last step and the only irreversible one.

## What is deliberately not planned

- **Multi-material, toolchanger, wipe tower.** No hardware, no second caller.
- **A GUI.** This is an engine; next-layer is the GUI.
- **SLA.** Different machine class, nothing in common below the mesh.
- **Becoming a general 3D kernel.** Boolean operations, sculpting and repair
  beyond what slicing needs belong to whoever owns the model, not here.

## How the ledger campaign went

Ten batches between 2026-11-11 and 2026-11-17 took the ledger from 240 applied to
299, and the record of each batch — what was tried, what failed, what it cost — is
in the project memory's archive (`~/.pi/agent/projects-memory/HFW/archive/`,
`2026-11-16-to-27-settings-batches.md`). It is not repeated here; this file is for
what comes next.

Two lessons from it that change how work is done here:

- **A ledger row is a measurement, not a feature.** Several batches moved rows by
  finding they were mis-sorted (multi-material keys filed as algorithms) rather
  than by implementing anything, and that is a legitimate way to move a number
  only because the bucket is what a person reads.
- **Every feature perturbs the gate's pinned numbers.** The two-line infill anchor
  moved the shipped cube from 1096.9 to 1192.1 mm and six gate sections had to be
  isolated with `infill_anchor = 0` to keep testing their own feature. Expect it;
  do not paper over it by loosening the assertion.
