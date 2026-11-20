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

---

## DECISION-9: a C ABI, and the one crate that may write unsafe code

**Options**: (a) keep the subprocess and pass geometry through temporary STL files;
(b) a C ABI in `crates/capi` with opaque handles; (c) a C++ shim around the Rust
crates; (d) Rust types across the boundary through a bindings generator.

**Choice**: (b).

**Reason**: the application has to hold a mesh between events — it shows the mesh,
lets it be transformed, then slices it — and a process boundary cannot carry that
without writing files per interaction. (c) invents a third language in the
boundary that has to be built and debugged on both sides. (d) makes every Rust
type change a C++ recompile and makes the layout of `Vec`, `String` and `enum`
part of the promise; handles keep the promise to pointers and flat structs.

The workspace forbids `unsafe_code`. A C ABI cannot be written without it — a
pointer that crossed the boundary must be dereferenced, and a handle handed back
must be rebuilt with `Box::from_raw` before it can be dropped. `crates/capi`
therefore overrides the lint for itself, and only itself; every other crate keeps
`forbid(unsafe_code)`. The exception is stated here because a silent one is how a
tree loses the property it was proud of.

**How to reverse**: the CLI is untouched and remains the tested path, so the ABI
can be deleted without touching the engine. Version bump — not source change — is
the mechanism for revising the surface, and a caller compiled against an older
header refuses on `hfw_abi_version` mismatch.

**Costs accepted**: the surface in `crates/capi/include/hfw.h` is a compatibility
promise, so a symbol lands once and is removed only by bumping the version. The
crate also duplicates the string conversions and null checks a C caller cannot be
trusted to have done — that is the boundary's job, not the engine's.

---

## DECISION-10: the plate document keeps geometry, placement and settings apart

**Options**: (a) store each object's triangles already where it sits on the bed, as
`hfw-cli` does; (b) store the geometry as loaded plus a placement, and apply the
placement at slice time; (c) store a full transform matrix per object.

**Choice**: (b), with `PlateObject::bake_placement` for a caller that wants (a).

**Reason**: a view moves an object without wanting its triangles rewritten — a
rotate gesture on a drag updates a placement, and the mesh only changes when someone
bakes it. (a) makes every gesture a full vertex rewrite and loses the difference
between "the file placed this" and "the user placed this". (c) is the same idea with
numeric errors to reconcile: `MeshTransform` (scale, Euler degrees, translation) is
already the engine's vocabulary, is exactly what the ABI passes by value, and an
object that needs more than one transform composes them in the front end and bakes.

Settings are layered, not copied: the plate holds the job's settings and an object
holds its own only when it overrides. Copying the plate's into every object on
creation is what `PlatePart` does in the CLI, and it makes a later preset change
reach every object except the ones that were created before it — a silent staleness
that no test would catch because the object was never told what it changed.

`Plate::slice_using` exists because a printer profile has to apply to a job without
being stored on the document: the caller layers the profile onto its own copy of the
settings and passes it, so slicing under two profiles in a row does not leave the
second one on the plate.

**How to reverse**: the placement is one field; deleting it and baking at add time
gives (a) back. The settings layering is one `Option`; resolving it at add time gives
the CLI's behaviour back.

---

## DECISION-11: the settings surface is generated from the ledger's measurement

**Options**: (a) a hand-written Rust table in `crates/config` describing every
setting; (b) a JSON asset generated by `tools/gen-settings-ledger.py` in the same run
that writes `docs/SETTINGS-LEDGER.md`, embedded with `include_str!`; (c) let the
application keep its own `SettingsModel` table and read the ledger's markdown.

**Choice**: (b).

**Reason**: (a) makes two inventories — the ledger's and the engine's — that have to
be kept in step by hand, and the ledger is the one that is measured, so the hand-kept
one would be the wrong one. (c) is a parser for a Markdown table, which is a worse
interface than JSON and still means the application owns a copy of the truth. (b)
costs one `json.dumps` in a tool that already computes every column, and the gate
fails when the asset is stale, so "the panel and the engine disagree" is not a class
of bug that can exist.

The status column is a *measurement*, not a promise: `applied` means this engine
reads the name and the value changes the G-code, checked by the same script that
writes the number in `STATUS.md`. When `upstream/` is deleted in phase 7 the name,
type and default columns lose their source; the statuses do not, so the document
survives with a smaller provenance and a `source` field that says so per row.

**How to reverse**: the asset is one file and one `include_str!`; a Rust table can be
emitted from the same rows if a compile-time check per key is ever wanted.

---

## DECISION-16: seven features are declined in writing, from one table

**Options**: (a) leave them unmentioned and let the absence speak; (b) build them;
(c) name them as out of scope in one table that generates both the document and the
rows the application's panel renders as unavailable.

**Choice**: (c) — `CAPABILITIES` in `tools/gen-settings-ledger.py`, which writes
`docs/SCOPE.md` and the surface's `capabilities` array in the same run.

**Reason**: (a) is how a rewrite quietly becomes a decade — every one of the seven is
a project, and an unmentioned gap is discovered by a user who assumes it exists. (b)
is out of the question: paint and text alone want a mesh-editing kernel, and STEP
wants an OCCT-class one, which is precisely the dependency this engine exists to not
have. (c) makes "unavailable" a fact the UI reads rather than a paragraph someone
remembers, and it costs one table.

The reason it is cheap is that none of the seven has a setting today: the engine does
not parse `single_extruder_multi_material` or `wipe_tower`, so asking for one is
refused by name rather than silently ignored — which is the failure mode this
decision exists to prevent, and the ledger is the instrument that proves it.

**How to reverse**: any one of the seven becomes a project against HFW's own
geometry, never a return to `upstream/`. Removing its row demotes the panel control
to available only once the feature slices and verifies. If paint or text turn out to
matter — they are the two users notice — that is a new crate with its own provenance
row, and `docs/SCOPE.md` loses a line.

## DECISION-15: a project file is a 3MF with our own side part, and settings are written by inverting the parser

**Options**: (a) write the application's existing PrusaSlicer-style project config so
its files keep working untouched; (b) put our settings inside the model part as 3MF
metadata; (c) a standard model part plus `Metadata/hfw_project.xml` in our own
format, with each setting written as the name and value an operator would type.

**Choice**: (c).

**Reason**: (a) means reimplementing a schema from another program's files, and being
wrong about it silently loses a user's settings — the one failure a project file must
not have. (b) puts non-standard elements in a part other tools parse, which is how a
"valid 3MF" stops being valid. (c) keeps the geometry openable everywhere and the
settings ours to define, and the format is small enough that the reader and writer are
one file each in `crates/plate/src/project.rs`, tested by a round trip whose assertion
is byte-identical G-code.

The value side is `Settings::key_value`, the inverse of `Settings::apply_key`, and a
test walks every published key through set → write → read → apply. A key with no
inverse cannot be saved rather than being saved as text nothing can read, which is why
only the print-level keys travel today.

**How to reverse**: the parts are named constants and the format is versioned
(`PROJECT_VERSION`); a reader of a newer file refuses it and names both versions.
Adding plate-wide settings is one more element and one more loop.

## DECISION-14: the plate arranger lives in `crates/plate`, not in `cli`

**Options**: (a) leave the packer in `crates/cli/src/arrange.rs` and let the ABI
reimplement "lay a plate out" for panels; (b) export it from the binary crate;
(c) move it to `crates/plate` as `arrange::{pack, layout, transform_for}` and have the
command line call it.

**Choice**: (c).

**Reason**: (a) is two arrangers that must agree about spacing, centring, quarter
turns and what "does not fit" means — the exact class of drift this plan exists to
remove, and the panel would be the one that drifts. (b) cannot be done: a binary crate
exports nothing. (c) costs one module move, and `layout` also absorbed the command
line's "keep the objects a 3MF positioned and pack the rest beside them" rule, which
had been its own function in `main.rs`.

The object transform is expressed once, as `arrange::transform_for(bounds, placement)`,
so the command line (which bakes the transform into the mesh) and the plate document
(which stores it as a placement) cannot lay a part down differently.

**How to reverse**: the module has no dependency on `Plate` — it takes footprints and
returns `MeshTransform`s — so it can move again without touching either caller.

## DECISION-13: a long job runs on a thread the caller polls, not on a callback

**Options**: (a) keep the ABI synchronous and let the caller's UI hang; (b) a C
progress callback invoked from a worker thread; (c) a job handle with a status, two
counters and a cancel flag, polled from the caller's own timer.

**Choice**: (c).

**Reason**: (a) is what a settings panel does before it reads as a crash — a plate of
twenty parts plans for minutes inside one blocking call. (b) is the obvious answer and
the one this library cannot make safe: a callback that unwinds or longjmps on a thread
the C caller did not create is undefined behaviour this side of the boundary, and
there is no way to stop it. Polling costs the caller one timer tick and keeps every
crossing a plain number.

Cancellation is checked before each object is planned (`slice_using_with_progress`,
which the plain `slice_using` now routes through) and once more before the file is
written. A cancelled job returns `PlateError::Cancelled` and then no G-code at all, so
a partial file can never reach a printer.

**How to reverse**: the thread is forty lines in `crates/capi/src/job.rs` and the
engine's only involvement is an `impl FnMut(usize, usize) -> bool` parameter. Removing
it means restoring the synchronous entry points, which still exist and are unchanged.

## DECISION-12: per-object settings are one key at a time, and refuse rather than ignore

**Options**: (a) keep the command line's hand-written match on five spelling
variants; (b) give `PlateObject` a `PrinterConfig` of its own and re-apply it whole;
(c) `Settings::apply_key(key, value)`, a key-addressed mapping beside
`apply_printer`, shared by the command line, the ABI and the project reader.

**Choice**: (c).

**Reason**: (a) is a second, smaller vocabulary that drifts from the profile path —
the panel would offer `infill_density` and the profile would honour
`sparse_infill_density`, and nothing would catch the day they diverge. (b) makes an
object's settings a whole profile, so every object carries a copy of every setting
and a preset change stops reaching it; it also means a panel has to build a config
text to set one value. (c) keeps the delta small, and the two vocabularies are
joined by the one naming table (`hfw_config::native_for`) that already maps the
dialect onto this engine's keys.

Three rules fall out of it, each of which a test holds:

- A key the engine cannot set on one object is refused with the key and the expected
  value shape, never ignored. The command line reports the refusal in its per-object
  report; the ABI returns -1 and `hfw_last_error` says why.
- Order does not matter: a support parameter turns support on by itself, and
  `support = 0` drops it. A panel sends whatever changed, in whatever order.
- The published list (`PER_OBJECT_KEYS`) is the print-level subset — what one part
  prints like, not which machine or material prints it — and the surface document's
  `per_object` flag is generated from it, so the panel cannot offer a control the
  engine would refuse.

**How to reverse**: `apply_key` is one match; the plate, the ABI and the command line
each call it once. Dropping it means restoring a per-object vocabulary somewhere
else, which is exactly what this decision replaced.

