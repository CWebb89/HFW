# Supports

What the two reference slicers do, what this engine will do, and the one thing
neither of them does that this one can.

## What the references actually do

**Detection.** Both classify a surface as needing support by angle. OrcaSlicer's
`support_threshold_angle` supports any overhang whose slope is below the
threshold (default 30°, 0 means fall back to `support_threshold_overlap` as a
percentage of line width); PrusaSlicer's `support_material_threshold` is the same
idea in degrees from vertical. Neither looks at whether the region could simply
be bridged — bridging is a separate pass over air cells, decided after support
regions are already laid out.

**Normal supports.** The overhang footprint is projected down and filled with a
grid or "snug" pattern. Contacts are separated from the model by
`support_object_xy_distance` (0.35 mm) and `support_top_z_distance` (0.2 mm), and
the top `support_interface_layers` (2–3) print at `support_interface_density`
(80–100%) so the overhang has something dense to land on. `support_on_build_plate_only`
drops any column whose projection hits the model.

**Tree/organic supports.** Cura's tree support (Thomas Rahm) and PrusaSlicer's
organic supports (an evolution of it) both: sample nodes on the overhang, drop
each node as a vertical "tip" of `tree_support_tip_diameter`, then grow branches
downward, merging nodes whose XY distance falls under `branch_distance`, and
allow horizontal travel limited by `branch_angle` (40–60° from vertical) so the
branch itself prints without support. Branch radius grows downward by
`branch_diameter_angle`, and cross sections stay circular. Collision against the
model is resolved by deflecting the branch outward and looking for a free path
down; if none exists, the branch is placed on the model if allowed, otherwise
dropped. Orca's "Tree Hybrid" prints normal grid support under large flat
overhangs and trees elsewhere, and "critical regions only" restricts trees to
cantilevers and tails.

**What both get wrong.** Support demand is computed once, from geometry alone.
Neither asks the question this engine already answers on hardware: *can this
span simply be bridged?* A 40 mm cantilever between two walls needs zero support
on this machine — measured, not assumed. Both slicers will fill it.

## Pipeline here

Supports consume the same rasterised masks the bridge pass uses
(`MaskCache`, `SkinGrid`), so no second geometry path is introduced.

1. **Seed.** For layer `k`, a cell needs support when the material one layer down
   does not extend under it by `h * tan(threshold_angle)`. That is the angle test
   expressed on the grid: an overhang at 60° steps out 0.35 mm per 0.2 mm layer,
   so only steeper overhangs seed. Cells whose below-layer is empty always seed.
2. **Prune.** Connected components smaller than `support_min_area` are dropped
   (Orca's "ignore small overhangs").
3. **Project.** Walk layers top-down, accumulating seeds into a running footprint.
   At each layer subtract the part footprint grown by `support_xy_gap`; where the
   accumulation is consumed by the part, the column terminates (support resting on
   the model) unless `support_on_build_plate_only`, in which case it is dropped.
4. **Offset in Z.** Seeds join the running footprint `support_z_gap` layers below
   the overhang they support, which is what leaves the contact clearance.
5. **Interface.** The top `support_interface_layers` of each column print at
   `support_interface_density`; the rest at `support_density`, alternating
   45°/135° per layer.
6. **Emit.** Filled with the same line generator as infill
   (`infill::fill_from_mask`), as `Kind::Support` and `Kind::SupportInterface`,
   ordered before the walls on each layer.

## Staging

| stage | scope |
|---|---|
| 1 | normal supports: grid fill, seeds, projection, gaps, interface, build-plate-only — **done** |
| 2 | tree supports: node sampling, tip, branch growth with `branch_angle`, merge by `branch_distance`, taper, collision avoidance against the part — **done** |
| 3 | span-budgeted supports (below) |

### How the tree is built

Seeds are the same z-shifted masks stage 1 uses, so a tree starts `support_z_gap`
below the overhang and keeps `support_xy_gap` from the part. Contacts are sampled
at `support_branch_distance` across each supported patch, each becoming a tip of
`support_tip_diameter`. Then, one layer at a time downward:

1. **Descend.** Every live branch is placed on the layer below at its own XY position,
   coning out by `support_branch_diameter_angle` and capped at
   `support_max_branch_diameter`. A branch that would enter the part is deflected
   in 45° steps up to two layers of travel; if nothing is free it is dropped, which
   is what removes a branch instead of growing it through the model.
2. **Attract.** Each branch steps toward its nearest neighbour, limited to
   `height * tan(branch_angle)` per layer. Nearer than `support_branch_distance` it
   steps fully; further away it drifts at a quarter step, so separated trunks still
   close a gap during a long descent without violating the lean limit.
3. **Fuse.** Branches whose cross-sections touch are replaced by one node with area
   conserved (`r = sqrt(r1² + r2²)`, capped). Fusion is decided by contact, not by
   centre distance: the merged disc is carried by the two discs under it, so its
   centre may shift further than one layer's lean without printing over air.
4. **Emit.** Each branch prints as one closed bead tracing its outline, plus sparse
   interior fill. Branches whose top layer is within `support_interface_layers` of a
   contact print dense instead.

A large flat canopy degrades gracefully into several parallel trunks where fusion
would need too much lateral travel, which is the same compromise PrusaSlicer's
organic supports make; unlike PrusaSlicer there is no per-branch collision search,
only the deflection test, so a canopy directly above a wall gets a gap rather than
a branch routed around it.

## The method neither slicer has: span-budgeted props

Support demand is an interval problem, not an area problem. Once a span can be
bridged up to a measured length `L`, an overhang does not need support everywhere
— it needs enough **prop points** that every remaining free span is `<= L`.

For a convex overhang footprint, that is the classic minimum-cover problem on the
projected outline: place props so the maximum gap between adjacent props (and
between a prop and the nearest anchor) stays within `L`. For a real mesh it is
solved per layer on the projected footprint's medial axis, greedy from the
longest unspanned chord, then each prop becomes an ordinary column or tree tip.

Three consequences no other slicer has:

- **Support volume scales with `1/L`, not with overhang area.** Doubling the
  measured bridge envelope halves the number of props under a large flat canopy.
- **`L` is empirical, not a setting.** It comes from `bridge_ladder` coupons on
  this printer (`>= 50 mm` measured at 0.4 mm beads, 0.2 mm layers), so the
  planner is calibrated to the machine instead of to a guess.
- **Supports never duplicate a bridge.** Any region the bridge pass can already
  span keeps zero support, so the two features stop competing for the same air.

The props themselves are deliberately dumb — a straight column, or a tree tip in
stage 2 — because the saving comes from *where* they are, not from how they look.

Open question this creates: a prop under a long span is loaded in compression by
the bridge's own tension (the same contraction that lifted the ladder's end leg),
so a prop must be wide enough not to buckle sideways, and its tip needs an
interface layer that sticks. That is what the first hardware coupon for stage 3
has to measure.
