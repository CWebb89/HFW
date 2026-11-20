# HFW: exact polygon offset — where the ring walk fails, and what I need to know

> **RESOLVED 2026-11-19.** The ring walk described here was deleted. The offset is
> now a winding-number union on an integer lattice, and the closing that uses it
> took the plate from 104.8 s to 21.9 s with 3DBenchy keeping its outline. Read
> `docs/SLICE-SPEED-STATUS.md` for the outcome and the numbers; this file is kept
> as the record of the failure and the questions that resolved it.

This is a self-contained brief for an outside reviewer. It states what the
algorithm is, what it gets right, the one thing it gets wrong with numbers, and
the specific questions I cannot answer from my own reasoning. Answers in the form
of algorithm/pseudocode is what is wanted; restating that offsets are hard is not.

## Context in one paragraph

HFW is a from-scratch FFF slicing engine in Rust (12 crates, ~32k lines, 397
tests) that is meant to replace libslic3r behind a QML front end. Slicing a
18-part plate takes ~100 s today; the target is 10 s, with PrusaSlicer and Orca
as the reference. Profiling put ~10 s of that in one place: closing cracks in the
extruded wall cross-section, which is `dilate by r` then `erode by r` on the
2D layer. That is a polygon offset, and no offset implementation existed, so I
wrote one. It is now exact on analytic shapes and on sphere slices, and it fails
on a real model. Wiring it in takes the plate from 100.4 s to 22.1 s, so the
performance case for finishing it is settled; the correctness case is the open
problem.

## The API

```rust
pub enum Join { Round, Miter }

pub fn offset_rings(rings: &[Vec<V2>], delta: f64, arc_tol: f64, join: Join) -> Vec<Vec<V2>>;
```

`arc_tol` is the largest chord error allowed when a corner is rounded (`Join::Round`).

Input rings are in normal form: outer counter-clockwise, holes clockwise, so the
material is on the left of every segment. Positive `delta` grows the region,
negative shrinks it. Coordinates are `f64` millimetres (V2 = 2D vector). A wall
width is typically 0.45 mm, a closing radius is half that, and a layer gives 1–20
rings of 100–3000 points each. Rings are simple (non-self-intersecting) on input.

## The algorithm as implemented

1. **`normalise`** — rewinds every ring by nesting parity: a ring inside an odd
   number of others becomes a hole and runs clockwise. Material is on the left
   everywhere, so one normal convention covers outlines and holes.

2. **`displace`** — every edge `a→b` becomes a `Piece` translated `delta` along
   its outward normal (right of a CCW edge). No joins yet.

3. **`split_at_crossings`** — every pair of pieces that cross is cut at the
   crossing, and *both* cuts are recorded against one shared `Junction` id. Pieces
   become `Fragment`s:

   ```rust
   struct Fragment {
       a: V2, b: V2,          // displaced, cut piece
       ring: u32, edge: u32,  // which input ring edge it came from
       t0: f64, t1: f64,      // parameter interval on that edge
       start: Option<u32>,    // junction at a, if any
       end: Option<u32>,      // junction at b, if any
   }
   ```

   So "fragments X and Y meet at a crossing" is a fact carried in the data, not a
   distance test afterwards. That was deliberate: the original version paired
   fragments by proximity and produced junk rings where four strands met within a
   micrometre.

4. **`keep_outer`** — a fragment belongs to the offset boundary only where it is a
   *full* offset from the original material. Test: nearest distance from the
   fragment's midpoint to the input boundary must be `>= |delta| - 1e-6` (exact
   segment distance from a grid index). On a shrink, additionally the point must
   be on the material side of its own source edge. A buried fragment fails and is
   dropped; a fragment valid in its middle but invalid at an end is trimmed by
   bisection onto the exact distance (`walk_back`). Ends sitting on a junction are
   never moved, because moving one would break the pairing.

5. **`stitch`** — walks fragments into rings using `link` (below), inserting a
   `junction` connector between consecutive fragments (miter: line intersection;
   round: arc of radius `|delta|` centred on the original vertex; when the two
   directions are parallel the endpoints are joined straight).

6. **`order`** — re-emits rings nesting-parity-first so callers get outlines
   followed by their holes.

The successor function, which is the part that is wrong:

```rust
fn link(pieces, rings, delta) -> Vec<Option<usize>> {
    // for each fragment:
    //   end is a junction -> the fragment starting at that junction, picked as the
    //                        outgoing one that turns furthest to the right
    //                        (material on the left), else fall through to ring order
    //   end is not a junction -> the next fragment of the same ring in edge order,
    //                        searching forward but only across gaps a join can span
    //   still nothing -> the nearest fragment start within 2*|delta| from another
    //                        ring (that is what welds two outlines across a crack)
}
```

## What is verified

- 16 unit tests: square grown (round-join area 143.14, not 144), square shrunk to
  exactly 64, vanishing square, hole shrinking while the region grows, hole
  swallowed, slit welded into one ring, dumbbell split at a 0.5 mm neck, 720-point
  circle to a true radius, two outlines merging across a 0.05 mm crack, miter
  closing returning a square exactly, identity case.
- Closing is the **identity on all 220 sphere layers** (area delta < 0.01 mm², no
  sliver rings, ring count preserved) and on a torus section.
- Cost: ~0.5 ms per 1000-point contour.

## What is broken

3DBenchy, layer z = 5.5 mm: the slice is one 852-point outline (932.09 mm²) plus
a hole (3.27 mm²). Closing it:

| step | rings | area |
|---|---|---|
| slice | 2 | 932.09 |
| grow +0.049 (miter) | 2 | 938.12 |
| shrink −0.049 | **1** | **3.27** |

The shrink returns the hole and loses the entire outline. Over 240 benchy layers
the cumulative area loss is ~10%. Instrumented: the shrink produces 918 fragments,
893 with no junction at their start and only 3 with a junction at their end, so
almost all continuation decisions are the ring-order rule, and the walk from the
first fragment dies after 27 pieces — the chain that would have to close the ring
does not reach its own start.

Every other test, including full-model layers at other heights and the sphere
sweep, passes. On benchy the growth step is correct (area grows by the expected
amount and keeps both rings), so the defect is specific to the shrink's trimming
and chaining.

## What I tried, and what happened

| attempt | outcome |
|---|---|
| Greedy walk: from each fragment take the nearest unused fragment start within `2r` | Works on analytic shapes; on the sphere it emits hundreds of sliver rings (regression: z=29.1225 gives 5 rings, z=29.3525 gives 17). |
| Explicit junction nodes carried through the split (current) | Sphere fixed and exact. Benchy still loses the outline on the shrink. |
| Enforce a successor *matching* (no two fragments claim the same target) | Broke the **grow** instead of fixing the shrink: an arrival refused at a junction has no valid fallback under that rule, so its chain dies. |
| Move the shrink's side test from the nearest segment to the fragment's own source edge | No change on benchy. |
| Drop the side test entirely and rely on distance alone | Kept fragments that had fallen out through thin necks; geometry wrong in a different way. |

My hypothesis, which is why I stopped rather than trying a fifth variant: after a
split, a fragment's continuation is a fact that has to be carried through the
operation — including for ends cut by the *distance filter* (`walk_back`), not
only for ends cut by a crossing. A trim silently changes where the chain goes, and
that is where the benchy ring breaks. I have no proof of that, and I do not know
the published rule for what the successor of a trimmed end should be.

## The questions

Answer these specifically. Pseudocode or a citation to the exact routine beats a
description.

1. **Is the walk the right shape at all?** Clipper2, which Prusa/Orca/Slic3r/Cura
   all use, does not walk. As I understand it, `ClipperOffset::DoOffset` emits one
   offset path per input path *including* the joins, leaving a polygon soup that
   may self-intersect arbitrarily, and then resolves it with a single boolean
   union: `ClipType::Union` with `FillRule::Positive` for `delta > 0`, and
   `FillRule::Negative` for `delta < 0`. Confirm or correct that, and explain why
   the fill-rule select is exactly the right validity criterion (i.e. why the
   positive-winding region of the soup equals the true offset). If that is right,
   the walk is not a better design than a union — it is a reimplementation of one
   with fewer invariants, and I should stop.

2. **If a union is the correct structure, what is the minimum correct boolean
   engine?** I need to decide between (a) implementing a scanline/Vatti union
   (integer coordinates, NonZero fill, ~1500 lines), (b) porting
   `clipper2-rust`'s `clipper.core` + `clipper.offset` (Boost-1.0, pure Rust
   already exists), (c) using `i_overlay` (MIT/Apache-2.0). Given that the rest of
   the engine is `f64` and slicer geometry has a 1 µm meaningful tolerance, advise
   on integer-vs-float here, and on whether a NonZero union over `f64` inputs can
   be made robust enough to be a slicer's cross-section operator, or whether
   snapping to integer nanometres first is mandatory.

3. **If the walk is worth keeping instead**, what is the correct successor rule
   for an end trimmed by the distance filter (not by a crossing)? Concretely: a
   fragment `F` on edge `e` of ring `R` is valid on `[t0, t1] ⊂ [0,1]` and invalid
   on `[t1, 1]`; the rest of edge `e` is inside the offset, and the next surviving
   piece along the offset boundary belongs to some *other* edge, not necessarily
   the next edge of `R`. What determines which one, and is there a formulation
   (e.g. track the nearest-feature/parity structure, or carry the "which side is
   buried" half-edge pair through the crossing) that is known correct? Name the
   paper if there is one — Chen & McMains is the reference I know of for
   valid-subsegment selection, and I could not make its chaining step work.

4. **The correctness oracle.** Right now I diagnose by area and ring count, which
   is how I know benchy loses 100% of its outline but not *where* the first wrong
   decision is. Is there a standard validation for an offset that localises
   failure, e.g. sampling points and comparing the offset's winding number against
   "distance to the region ≥ delta, and on the material side", computed
   independently of the offset itself? If yes, give the check and the sampling
   criterion. I would rather have a bisectable oracle than another guess.

5. **Rounding.** For a shrink, corners must come out sharp at the *exact* offset
   distance; for a grow, outward corners round with `arc_tol` and inward ones come
   to a miter. My implementation bisects onto the exact distance for trimmed ends
   and clamps miter length by a limit. Is there a standard rule for the miter
   limit and for the minimum arc segment count that a slicer should use, or is it
   purely cosmetic in this context (the result is re-closed and then re-offset by
   half a wall width anyway)?

6. **Is dilate-then-erode even the right primitive for this?** The purpose is
   closing cracks between adjacent wall cross-sections before wall generation, on
   layers that are already simple polygons. Alternatives I see: (i) dilate+erode
   (what this is), (ii) a raster/field closing at the wall field's own resolution
   (that is what runs today, 100 s, too slow), (iii) detect crack pairs
   geometrically and patch the wall loops directly (cheap, but the crack topology
   decides output quality — the current failure mode is exactly "patched the wrong
   pair"). Given the target is *speed* with parity against Prusa, which of these
   does Prusa actually do, and is there a reason to prefer one?

## Practical notes for whoever answers

- Build and test: `cargo test -p hfw-kernel` (16 offset tests) and
  `cargo test -p hfw-engine --test closing` (sphere sweep, torus, and the
  `#[ignore]`d benchy case that reproduces the failure with its numbers).
- The failing input is a plain `Vec<Vec<V2>>`: benchy z=5.5, 852-point outline
  plus one 3.27 mm² hole, delta = −0.049, miter joins. If a reviewer wants the
  exact coordinate list, it is reproducible with the engine's `slice_plane`.
- The code is `crates/kernel/src/offset.rs`, 1027 lines including tests; the
  offset is **not wired into the engine** and must not be until benchy keeps its
  outline.
- Nothing here is committed; the tree is otherwise at HEAD.
