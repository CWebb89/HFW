# Behavior notes

What a stage does, written in plain words from behavior and measurement rather
than from upstream source. Each note names its inputs, outputs, ordering, units,
thresholds and edge cases, so an implementation can be written from the note. No
note here was produced by reading libslic3r, PrusaSlicer or OrcaSlicer source; the
protocol and what was consulted is in `PROVENANCE.md`.

---

## BN-slicing: from triangles to one layer's material

**Input**: a triangle mesh in integer nanometres, one horizontal plane at `z`,
`close_gaps` in millimetres.

**Output**: closed contours on that plane, each wound so the material is on its
left; plus counters: triangles tested, segments found, duplicates removed, branch
points, gaps closed, largest gap closed, contours reconstructed, contours dropped.

**Ordering**

1. For every triangle whose vertices do not all sit on the plane, intersect its
   three edges with the plane. Keep at most two distinct points. Round
   interpolation to the nearest nanometre with the edge endpoints canonicalised
   first, so two triangles sharing an edge produce the same point.
2. Orient the chord by the triangle's outward normal: the horizontal part of the
   normal `n` gives the in-plane run direction `(-n.y, n.x)`, and the chord is
   ordered to agree with it. A chord is perpendicular to that direction by
   construction, so the test is never near zero except for a degenerate triangle,
   which is skipped.
3. Deduplicate by unordered endpoint pair. If the same pair arrives with the
   opposite direction, the surface is double-sided there and both are removed.
4. Chain segments from the end of one to the start of the next. Prefer the
   continuation that has an unused segment of its own over one that dead-ends, and
   among those the smallest turn, and among those the lowest segment index. A
   chain that returns to its start is a closed contour.
5. Join the remaining open chains in stages. In each stage, repeatedly join the
   pair of endpoints (end of one chain to the start of another, or a chain's own
   end to its own start) whose distance is smallest and within the stage's limit.
   Ties break by endpoint coordinates, then by chain index. Stage limits are 0,
   0.001, 0.1 and 1 mm, followed by `close_gaps` (default 2 mm, accepted range 0
   to 10).
6. After the ladder, an open chain that had to choose between two continuations
   at a junction is a fragment of a self-touching surface: drop it, count it, and
   retry the ladder on what remains. Anything still open is reported as an open
   contour at its last point and the layer is not printable; under `--strict` the
   job fails.

**Edge cases**: contours entirely on the plane are skipped (they are horizontal
faces, not a cross-section); a zero-area contour is an error from the contour
type; an open outline narrower than the ladder is closed rather than dropped, so
a mesh defect costs a little geometry rather than a whole layer.

**Why the winding is kept**: the set of contours a layer produces is material, not
a nesting puzzle. Two shells that overlap must print their union, and a shell
inside another with the same winding is still material. Both facts need the
triangle's own orientation, so it is carried through the chain instead of being
re-decided from containment depth.

---

## BN-outline: from contours to a printable region

**Input**: the layer's contours, integer nanometres.

**Output**: a region of nested rings in millimetres, outer counter-clockwise.

**Rule**: every counter-clockwise contour adds material; every clockwise contour
removes it. All additions are united first (an integer polygon union, positive
fill rule), then each removal is subtracted in turn. The result is re-normalised
so an even nesting depth is an outline and an odd depth a hole, and rings are kept
in descending area order.

**Why per-hole subtraction**: subtracting the union of all holes would make a
hole inside a hole behave as material, which is what it is; unioning them first
loses that.

---

## BN-flow: bead cross-section and the slot a strand fills

**Input**: bead width `w`, layer height `h`, bead class.

**Output**: deposited cross-section in mm².

- **Outer and inner wall**: `w·h − h²(1 − π/4)`, floored at half the rectangle. The
  two long sides of a bead pressed between the nozzle and the wall beside it round
  off, so its four corners are not filled. At 0.4 × 0.2 mm the term is 10.7% of
  the rectangle. Reference files agree with this to within a percent on
  wall-heavy models.
- **Gap strand** (`Kind::Middle`): `w·h`, the full rectangle. A strand fills a slot
  whose corners its neighbours already occupy, so subtracting the corner term
  withholds plastic the slot demands — 14% of a 0.3 mm rib by construction.
- **Skin, infill, support, brim**: `w·h`. Neighbouring beads merge and the region
  is filled solid.

The inverse is used by the verifier to name the width from `E`: for a wall bead
`w = V/(L·h) + h(1 − π/4)`; for everything else `w = V/(L·h)`.

---

## BN-spacing: the loop ladder

Current, measured behavior (a planned change is recorded as DECISION-7):

- Loop 0's centre sits at half the outer loop's width from the model surface.
- Loops after it are spaced one nominal width apart, centre to centre, adjusted
  when the outer width differs from the nominal one so the difference is shared
  across the band by a parabolic weight whose radius is `wall_distribution_count`.
- The wall band therefore reserves one nominal width per loop, while the flow model
  deposits the rectangle minus the corner term. The band is slightly wider than
  the plastic in it; the gap pass covers the remainder where it is narrower than a
  bead.
- A feature narrower than the floor (`0.25·w`) is dropped before the ladder runs; a
  feature between the floor and two beads is covered by the ladder plus the gap
  pass; a strand shorter than three of its own widths is not laid because it is a
  dab on material the loops already cover.
