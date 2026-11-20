# Slice speed: where the time goes and what it will take to reach 10 s

Written 2026-11-19, for review by someone who has not read this repository. Every
number below was measured on this machine, on two real models, with the engine in
this tree. Where something is an estimate it says so.

## The engine in one paragraph

HFW is a headless FFF slicer in Rust. A triangle mesh and a profile go in; verified
G-code comes out. It replaced libslic3r inside the `next-layer` desktop application,
so the app's slicing is 100% HFW today, with no C++ slicer behind it. The pipeline is:
exact integer triangle/plane slicing into 2D rings, a "closing" pass that heals
cracks in the sliced outline, wall generation, infill, toolpath planning, G-code
writing, and an independent verifier that re-reads the G-code.

Wall generation is the part that matters here. It is a **raster** algorithm: build a
grid of distance-to-boundary values over the part's bounding box (cell size `h`, a
quarter of the extrusion width), extract a medial axis (a "ridge") from that grid,
classify each cell as thick / thin / unprintable using a ray cast across the feature,
and then trace iso-lines of the distance field to get wall loops. It handles thin
features well — that is why it was written this way — but its cost is proportional to
**area**, not to the boundary length, and a 3D-printer cross-section is mostly area.

## Measured state

Model: `Revolver All Parts.3mf`, 18 separate parts on one plate, 478 layers.

> **Correction (2026-11-19):** the 6.3 s PrusaSlicer row below was measured on a
> 350 mm plate sliced with PrusaSlicer's default bed, which is smaller than the
> plate: parts outside it never made it into the G-code (19566 mm of filament
> against OrcaSlicer's 33047). With a Voron 2.4 350 machine and matched settings
> PrusaSlicer takes **9.4 s** and deposits 34368 mm. The numbers in the table are
> kept as they were measured; `docs/SLICE-SPEED-STATUS.md` carries the corrected
> comparison.

| slicer | time |
|---|---|
| PrusaSlicer 2.9.7 (0.2 mm, 2 walls, 15%) | 6.3 s |
| OrcaSlicer 2.4.2 (its own settings, 624 layers) | 16.1 s |
| **HFW today** | **35.2 s** |
| HFW at the start of this work | 128.7 s |

`3DBenchy.stl`, 240 layers: 4.4 s (was 34 s).

Per-phase wall-clock and CPU on the plate (16 threads; CPU is the sum over threads,
so it is ~12x the wall time):

| phase | wall | CPU |
|---|---|---|
| wall generation | 17.6 s | ~280 s |
| closing | 10.5 s | ~148 s |
| contour tracing (part of walls) | 5.3 s | ~85 s |
| distance field (part of walls) | 5.0 s | ~79 s |
| cell classification (part of walls) | 4.7 s | ~75 s |
| medial axis (part of walls) | 1.7 s | ~27 s |
| infill | 3.9 s | — |
| G-code write + verify | 1.5 s | — |
| slicing | 0.7 s | — |
| **total** | **35.2 s** | **~424 s CPU** |

The plate is CPU-bound: 424 s of work across 16 threads. To reach 10 s the *total
CPU* has to fall to about 160 s. No amount of tuning does that; 424 s has to become
~150 s, which means every phase that walks an area must become a phase that walks a
boundary.

## What was built this round

`crates/kernel/src/offset.rs`: an **exact polygon offset**. Given a region's rings
(outer counter-clockwise, holes clockwise) and a distance `delta`, it returns the
region grown (positive) or shrunk (negative) by that distance, as rings.

How it works:
1. Displace every edge by `delta` along its outward normal.
2. Split the displaced edges wherever they cross each other.
3. Drop the pieces whose distance to the original boundary is less than `|delta|`
   (they are buried inside the offset), plus, on a shrink, pieces that fell outside
   the region.
4. Trim surviving piece ends onto the exact distance by bisection.
5. Walk the pieces into rings, filling corners with an arc (`Join::Round`, the
   Minkowski/disc offset a wall loop wants) or a miter (`Join::Miter`, which keeps a
   square corner square, which is what a morphological closing wants).

Correctness is pinned by 16 unit tests: square grown and shrunk, a square that
vanishes, a hole shrinking as the region grows, a hole swallowed, a slit welding shut
into one ring, a dumbbell *splitting* into two rings at a 0.5 mm neck, a 720-point
circle offsetting to a true radius, two outlines merging across a 0.05 mm crack,
miter closing restoring a square exactly, and the identity case.

Speed, measured (release build, one core):

| contour points | one offset |
|---|---|
| 1,000 | 0.53 ms |
| 5,000 | 4.1 ms |
| 20,000 | 24.7 ms |

A typical layer of this plate is ~10,000 points, so four offsets (two wall loops and
a closing's grow + shrink) are ~30 ms of CPU per layer, ~15 s over the plate, about
1 s of wall time. That is the promise: walls and closing from ~28 s to low single
digits.

## Where it stands: the closing attempt and its failure

The cheapest thing to wire first is the **closing** pass, because it is a
morphological closing — grow by the radius, shrink back — which is exactly two
offsets with miter joins. It was wired, and:

- `crates/walls/src/repair.rs`: `close_gaps` was rewritten to `offset(+r, Miter)`
  then `offset(-r, Miter)`, keeping the existing rule that drops holes below `pi r^2`.
  The old implementation (a pairwise "pull" of boundary points toward each other,
  iterated up to 8 passes over a rebuilt segment grid) was deleted. All 31 wall tests
  and the kernel's 16 passed, including: a crack narrower than twice the radius is
  filled to exactly its area, a gap wider than twice the radius is untouched, a small
  hole is filled, a thin spike survives (closing never removes material), a slit is
  welded from the mouth in. Closing with miter joins is *exactly* the identity on a
  shape that is already closed, which is why those last two are exact and not
  approximate.
- Then the full engine suite found a real regression: `vase mode on a sphere`
  printed 8 walls on one layer instead of 1. Chasing it produced this:

```
z=29.1225  raw rings 1 -> closed rings 5
z=29.3525  raw rings 1 -> closed rings 17
```

The one real ring comes back correctly (area 59.673 mm²); the extra 16 have areas
between 1e-4 and 5e-4 mm². They are **junk rings**: pieces the walk could not place
into the real loop, closed into slivers. They are invisible but the wall generator
counts them as separate boundaries and prints a wall for each.

Filtering them by area "fixes" the symptom and breaks something real: at a 0.02 mm
closing radius the intermediate state contains a genuine 0.01 x 0.01 mm hole
(area 1e-4 mm²) that must survive the grow step so the shrink can restore it to
0.05 mm. Any area floor large enough to kill the junk kills that hole too. The
contamination has to be fixed at the source, not filtered out.

**So the closing wiring was reverted.** `close_gaps` is back to the pull
implementation, the suite is green (394 tests), and the polygon offset sits in the
kernel, tested and unused. Nothing in the tree is half-wired.

## The actual defect, stated precisely

Step 5 above, the walk, is a greedy reconstruction: at each piece end it looks for
the piece that starts there and, when several do (a crossing point where three or
more boundaries meet), it picks by a "rightmost turn" score. That heuristic is
correct on clean input — every kernel test passes, including multi-ring merges and
splits — but real sliced geometry has hosts of near-degenerate junctions: a sphere's
UV seam sliced at some heights produces dozens of hairline cracks that closing welds,
each weld creating a junction where four boundary strands meet within a few
micrometres of one another. Greedy choice at those junctions sometimes pairs the
wrong strands, and the leftovers close into slivers.

A production offset does not do this. Clipper/Clipper2 (the library PrusaSlicer,
OrcaSlicer, Slic3r and Cura all use) keeps an explicit outrec list — for each output
vertex, which vertex it came from and which it joins to — instead of rediscovering
pairing from positions, so a junction is resolved by construction. That is the
missing piece and it is not a large one: the crossing detection, the distance
filtering and the joins here are already in place; the walk needs to carry the
pairing through the split step rather than reconstructing it afterwards.

## The plan to 10 s, and what each step buys

1. **Fix the walk** (the defect above): carry the split pairing through to the join
   so a junction is never ambiguous. Cost: the piece identity bookkeeping in
   `split_at_crossings` plus tests built from real sliced layers, not just analytic
   shapes. After this, closing wires in at ~1 s instead of 10.5 s. Suite must stay
   green on the sphere/cube/torus fixtures, and the plate's filament must match to
   within a fraction of a percent.
2. **Wall loops from offsets**: loop *k* is `offset(region, -(k*w + w/2))`. This
   replaces the iso-line tracing, the distance field and part of the classification —
   the 17.6 s wall phase becomes polygon work in the low single digits. The per-vertex
   width machinery, overhang tagging and gap fill must be carried over as-is.
3. **Thin features**: this is the hard part and the reason the current engine uses a
   raster at all. A rib 0.6 mm wide with 0.45 mm walls cannot be printed with
   concentric offsets; it needs one bead of variable width down its centre, and the
   width comes from the medial axis. Two honest options:
   - keep the medial axis but only over the *residual* region that the offsets left
     uncovered (much smaller area, so much cheaper), or
   - extract the medial axis from the polygon instead of a grid — that is what
     Arachne (PrusaSlicer's variable-width engine) does, via a skeletal
     trapezoidation, and it is the real answer but it is a project in itself.
4. **Retire the raster path** only after 2 and 3 pass parity on real models
   (filament within a fraction of a percent of today's output, verifier green,
   visual check on the thin-wall test parts).

Realistic projection with 1 and 2 done: closing ~1 s, walls ~3 s, infill 3.9 s,
write+verify 1.5 s, slicing 0.7 s → **~10–11 s**, which is the target. Step 3 decides
whether that holds for thin-featured parts or only for chunky ones.

## Open questions worth a second opinion

- Is reimplementing the walk properly the right move, or is vendoring a
  battle-tested integer polygon clipper (Clipper2's algorithm, MIT-licensed, ~6k
  lines of C++ to port, or an existing Rust crate such as `geo`/`i_overlay`) the
  better call? The hand-rolled kernel is ~500 lines of logic and passes every test I have;
  the risk is in exactly the case that broke, which is also the case every real
  model hits.
- For thin features, is a coarse raster for *detection only* (say `h = w/4`, ~6x
  fewer cells than today) plus exact offsets for the geometry an acceptable
  compromise, or does it reintroduce enough cost to miss 10 s?
- Is closing on *every* layer the right policy? Today it runs per layer with a
  radius of 0.049 mm. If a layer contains no crack, closing is the identity but
  still costs two offsets. A cheap pre-test (minimum distance between non-adjacent
  boundary segments) using the same segment index would skip most layers — but only
  if it cannot miss a crack, and missing one is worse than spending the 1 s.

## Files touched this round

- `crates/kernel/src/offset.rs` (new, 826 lines including tests) — the exact polygon offset.
- `crates/kernel/src/lib.rs` — `pub mod offset;`
- `crates/walls/src/repair.rs` — was rewritten to use it, then reverted; the file is
  byte-identical to the commit before this round.

Nothing is committed. The full workspace suite is 394 tests, 0 failures.
