# Decisions

`DECISION-<n>: options / choice / reason / how to reverse`.

---

## DECISION-1: which integer polygon boolean

**Options**: (a) hand-written exact boolean in `hfw-geometry`; (b) `i_overlay`
9.2 (MIT OR Apache-2.0); (c) `clipper2-rust` 1.2 (BSL-1.0, already a dev
dependency for a parity test).

**Choice**: (b), through the existing `hfw_kernel::offset` wrappers. `union_rings`,
`difference_rings` and `intersect_rings` were already there; the slicing path now
uses them for the layer region instead of nesting by containment depth.

**Reason**: it was already a dependency of `hfw-kernel` and already the engine
behind the offsets and the closing, so layer booleans run on the same lattice as
every other set operation (1 nm, addressed from the shared bounds). A second
boolean library would put two answers to the same question in one pipeline.

**How to reverse**: `union_rings` and `difference_rings` are the only entry points
`region_from_contours` uses. Swapping the crate means reimplementing those two
functions; no caller sees the library.

**Licence**: MIT OR Apache-2.0, compatible with the workspace licence. Recorded in
`PROVENANCE.md` under third-party crates.

---

## DECISION-2: winding, not nesting depth, decides holes

**Options**: (a) keep normalising every contour by even/odd nesting depth; (b)
carry the triangle's orientation through the slice and treat counter-clockwise as
material, clockwise as a hole.

**Choice**: (b).

**Reason**: (a) cannot tell a hole from a second shell drawn inside the first, and
it XORs two overlapping shells into an L-shape: 232 mm² where 632 is material.
(b) is what the mesh already says, and it makes the union meaningful.

**Cost**: a mesh whose faces are wound inconsistently would produce wrong
material. That is why the always-on repair pass orients closed components outward
before slicing (DECISION-4).

**How to reverse**: `region_from_contours` in `crates/engine/src/lib.rs` is the
single place that reads winding; `hfw_slicing` would need the nesting normaliser
back.

---

## DECISION-3: gap closing ladder and branch fragments

**Options**: (a) keep failing the whole layer when the outline is open; (b) join
nearest endpoints with one fixed tolerance; (c) a staged ladder with a
command-line ceiling, and the least-turn walk at junctions.

**Choice**: (c), with `--close-gaps` (default 2 mm, max 10 mm).

**Reason**: a real STL routinely carries a sub-millimetre crack where two facets
nearly meet, and closing it at the smallest stage that works both preserves the
geometry that is there and reports how far the mesh was from watertight through
`SliceDiagnostics::gap_closed_nm`. A fixed tolerance cannot do both. Fragments
that had to guess at a junction are dropped rather than joined, because joining
them prints a spur twice.

**How to reverse**: `stitch_contours` in `crates/slicing/src/lib.rs`; the ladder
constant is `GAP_LADDER`.

---

## DECISION-4: mesh repair runs on every job

**Options**: (a) only with `--repair-basic`; (b) always, reporting what changed.

**Choice**: (b). `--repair-basic` still parses and is now redundant.

**Reason**: welding exact duplicates, dropping zero-area and duplicate triangles
and orienting closed components outward are safe by construction — none of them
can turn a correct mesh into a wrong one — and the slice now depends on face
winding (DECISION-2), so a mesh that needs orientation repair must not slice
differently because a flag was forgotten. `Job::repair` reports the counters.

**Not done**: welding vertices within 10 nm. `Mesh::new` welds exactly after
nanometre quantisation; a 10 nm tolerance needs a spatial hash and was not
implemented. Consequence: two vertices 1–10 nm apart stay distinct and may leave a
hairline crack, which the gap ladder closes.

**How to reverse**: the call is one line at the top of `hfw_engine::run`.

---

## DECISION-5: a gap strand deposits the rectangle

**Options**: (a) subtract the free-bead corner term from every wall-class bead;
(b) subtract it for `Kind::Side` only.

**Choice**: (b).

**Reason**: a middle strand fills a slot whose corners its two neighbours occupy.
(a) withheld 14% of a 0.3 mm rib's plastic by construction: `thin_wall.stl` was
−24.3% against the references. (b) measures −11.7% and the remainder is the
reference policy of substituting a full-width bead, recorded as an expected
divergence.

**How to reverse**: `hfw_toolpath::metrics::section_area`, and the two matching
inverses in `crates/gcode/src/{writer,verify}.rs`.

---

## DECISION-6: `crates/skeleton` is derived, and should go

**Options**: (a) keep it as the variable-width generator; (b) clean-room rewrite
from `BEHAVIOR-NOTES.md` by a different route; (c) delete it.

**Choice**: none taken — this needs the owner. The recommendation is (c), delete.

**Reason**: `crates/skeleton` (~2,000 lines) is not used by any shipped wall path,
and `docs/WALLS-SKELETON.md` states its stage list was read from OrcaSlicer's
Arachne directory (`SkeletalTrapezoidation`, AGPL-3.0) and follows
`WallToolPaths::generate` stage for stage. The module boundaries match that list
one for one: build → classify → bead → emit. That is a derived structure, not a
coincidence of subject matter, and it is a licence risk for an MIT OR Apache-2.0
tree that next-layer ships. Keeping it (a) keeps the risk and the unused code;
(b) is a week's work and would land the same value as the P2.6 redistribution
design without the provenance question.

**How to reverse**: it is one crate directory plus its `Cargo.toml` member entry;
the tree has no other reference to it. Deletion is not done here because removing
files is the owner's call.

---

## DECISION-7: the spacing model is still width-per-loop, not bead-spacing

**Options**: (a) space loops by `w − h(1 − π/4)` (the actual deposited spacing) and
give loop 1 the mean of the outer and inner spacing; (b) leave the ladder spacing
at one nominal width per loop and let the gap pass cover the remainder.

**Choice for this pass**: (b) — the change is planned, not rejected.

**Reason**: (a) is correct physics and removes the unmodelled groove between
beads, but it moves every loop centre and therefore every pinned volume in the
gate and every oracle number. Doing it without re-measuring all of them would be
a guess dressed as a fix. The work needs the layer height inside `Params`, which
today has no height at all.

**How to reverse**: nothing to reverse; (a) is the work item.

---

## DECISION-8: one commit for P1's three task IDs instead of three

**Options**: (a) split the P1 diff by task ID; (b) one commit naming P1.1–P1.3.

**Choice**: (b), with the reason here.

**Reason**: the winding change, the staged closing and the always-on repair touch
the same functions in the same files (the stitcher carries the winding, the ladder
closes through it, the repair supplies the winding). Splitting them would produce
commits that do not build.

**How to reverse**: not needed; history is local and unpushed.
