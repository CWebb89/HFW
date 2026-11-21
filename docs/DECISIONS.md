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

## DECISION-18: a cap keeps the boundary's own vertices, so the capper bridges instead of sweeping

**Date**: 2026-11-21.

**Decision**: `crates/kernel/src/cap.rs` caps a region by bridging each hole into its
boundary and clipping ears. It does not use a grid, a band sweep, or any decomposition that
places a vertex on the boundary at a height of its own choosing.

**Why**: a cap shares its boundary with the walls beside it. If the cap puts a vertex in
the middle of a wall edge, the two meet at a T-junction: the mesh reports a boundary edge,
a slicer can leave a crack, and `diagnostics()` says the half is not closed. A band sweep
does exactly that — it cuts the region at every vertex height, so any wall edge spanning
several bands gets subdivided. The first version of the cut caps was a sweep and every cut
test failed on boundary edges.

**Tradeoff**: bridging adds a slit, two coincident edges traversed in opposite directions,
and ear clipping has to be careful around it: an ear that spans the slit lies outside the
region while no ring point falls inside it, so the ear test also refuses a candidate whose
interior any ring edge crosses. That check is O(n²) per ear, which is fine for caps.

**Reverses**: swap `cap::triangulate` for a constrained Delaunay triangulator and the two
callers (`text` for a glyph's face, `mesh` for a cut) do not change.

# Decisions

`DECISION-<n>: options / choice / reason / how to reverse`.

---

## DECISION-17: text is a prism, not a boolean

**Options**: (a) no text in the engine, as today; (b) read a font, then union its
glyphs into the mesh with a triangle-level CSG kernel; (c) read a font, extrude each
string into a closed prism, place it as a volume, and let the slice's own region union
and subtraction decide what the layers look like.

**Choice**: (c), `crates/font` + `crates/text` + `hfw-cli --text`.

**Reason**: the engine already unions and subtracts regions per layer with exact
integer predicates — that is what a modifier or negative volume is. A letter only
looks expensive because it arrives as curves; once it is triangles it is an ordinary
object, and CSG on triangle soup would add a kernel, its rounding failure modes and its
non-determinism to buy nothing the per-layer region already gives. Measured: `hfw-cli
--text HFW` slices to four layers at 0.2 mm, writes G-code the independent verifier
passes, and the font reader agrees with FreeType to 0.00218 mm of ink at 10 mm.

**Tradeoff**: embossed text has to be placed as a volume that overlaps the object, so
it cannot modify another object's own mesh (there is nothing to export as a merged STL),
and engraved text has to be a negative volume. Both are the plate's job already — the
application asks for a prism with `hfw_text_mesh` and attaches it with
`hfw_plate_object_add_volume`, so it never parses a font and never runs a boolean.

**How to reverse**: if the application ever needs a single merged mesh with the text in
it, the prism's triangles are the input a boolean would take; adding `--cut`-style
subtraction inside `crates/text` would be that change, and it would earn its own
decision and its own deviations in `docs/TEXT.md`.

## DECISION-16: engine parity is the goal, so every gap is a backlog row

**Supersedes the original DECISION-16 below**, which listed the gaps as declined. The
owner's rule is the opposite: **everything libslic3r has, this engine has in Rust** —
the tool changer and MMU wait on hardware that does not exist here, and SLA/MSLA is a
different process, ruled out on purpose. Painting already moved off the list by being
implemented (support and seam channels, `enforcers_only`).

**Options**: (a) no list at all, so gaps are discovered by users; (b) a per-feature
prose note; (c) one table of every capability libslic3r has and this engine does not,
carrying `state` (`unbuilt`, `blocked`, `excluded`), what closing it costs and what it
waits on, generating both `docs/PARITY-BACKLOG.md` and the `capabilities` rows the
application's panel disables.

**Choice**: (c), `BACKLOG` in `tools/gen-settings-ledger.py`.

**Reason**: parity is a measurable target, so the gap has to be enumerated rather than
remembered. The settings half is already measured — 308 of 372 applicable applied — and the
backlog adds the part a setting cannot express: text, cut and hollow, nesting, AMF,
internal bridges, a tiled field, modifier volumes, vendor placeholders, slicing modes,
and a gated corpus driven through libslic3r's own API. `blocked` is not `declined`: a
blocked row states what it waits on and is deleted when the work starts.

**How to reverse**: a row comes off the backlog by being built, in the commit that
builds it, with the test that proves it. Removing a row without the feature is the
failure this decision exists to prevent, which is why the panel reads the same rows and
the gate checks them from outside Rust.

## DECISION-16 (original, superseded 2026-11-20): six features recorded as declined, from one table

**Options**: (a) leave them unmentioned and let the absence speak; (b) build them;
(c) name them as out of scope in one table that generates both the document and the
rows the application's panel renders as unavailable.

**Choice**: (c) — `CAPABILITIES` in `tools/gen-settings-ledger.py`, which writes
`docs/PARITY-BACKLOG.md` and the surface's `capabilities` array in the same run.

**Reason**: (a) is how a rewrite quietly becomes a decade — every one of them is a
project, and an unmentioned gap is discovered by a user who assumes it exists. (b) is
out of the question: text alone wants a mesh-editing kernel, and STEP wants an
OCCT-class one, which is precisely the dependency this engine exists to not have. (c)
makes "unavailable" a fact the UI reads rather than a paragraph someone remembers, and
it costs one table.

The reason it is cheap is that none of them has a setting today: the engine does not
parse `single_extruder_multi_material` or `wipe_tower`, so asking for one is refused
by name rather than silently ignored — which is the failure mode this decision exists
to prevent, and the ledger is the instrument that proves it.

**How to reverse**: any one of them becomes a project against HFW's own geometry,
never a return to `upstream/`. Removing its row demotes the panel control to available
only once the feature slices and verifies. Paint is the first to go through that door:
support paint is a per-triangle channel in `crates/mesh`, consumed inside
`supports::seed_masks` and carried in the project file, so its row is gone from
`docs/PARITY-BACKLOG.md` while seam and tool paint remain unbuilt (`docs/PAINT.md`). If text
turns out to matter — it is the other one users notice — it is a new crate with its
own provenance row, and `docs/PARITY-BACKLOG.md` loses a line.

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


## A bed mesh becomes the profile's outline, nothing new

`bed_custom_model` does not add a second kind of bed. It slices the mesh halfway up its
height, collapses the collinear vertices an unwelded mesh leaves, and writes the largest
ring into `bed_shape` with the origin moved to the mesh's lower corner. Everything
downstream already handles a polygon bed: the packer uses the outline's box, the writer's
`bed_target` moves a part into a concave or round outline, `bed_contains` refuses corners
off it, and the header names the shape. The alternative — a parallel "the model is the
bed" path — would have duplicated all four and drifted from them.

Reverses by deleting `apply_bed_model` and the profile key; a profile that spells
`bed_shape` by hand keeps working exactly as it did, which is why the fixture is a *round*
bed: it exercises the polygon route in a case a rectangle cannot express.

Holes in a bed mesh are not holes in the printable area. `bed_shape` is one polygon, the
same limit a hand-written outline has, and the ledger row says so rather than pretending
otherwise.

## Slicing mode: one path, two names, one real difference

`even_odd` produces the same output as `regular` and the ledger says so out loud. Slice
assembly already resolves a stack of contours by the even-odd rule — that is what building
a `Region` from a cross-section means — so there is no second algorithm to select, and
writing one that produces identical output would be a branch nobody can observe.

`close_holes` is the real difference and it is one operation on the region: keep the rings
no other ring contains. A hole is a ring inside another, so it goes; an island that sat
inside a hole is inside the outer boundary too, so it goes as well, which is correct —
once the void prints over, the island is part of the same solid. Measured on
`fixtures/torus.stl`: 914.8 mm of filament against 831.5 mm in regular mode, verifier
passing both.

Reverses by dropping the enum and the field: nothing else reads it.

## Support layer synchronisation is a property, not a switch

`support_material_synchronize_layers` is recorded and changes nothing, because the thing it
asks for is already true: this engine plans one layer schedule per job and builds support on
that schedule, so a support layer starts and ends exactly where the object's layer does. The
reference needs the option only because a multi-extruder preset can print support taller than
the part it holds up; there is no second schedule here to coarsen. Turning it off therefore
does not reintroduce the misalignment, and the ledger row says that rather than pretending a
switch exists.

## Travel routing was measured before it was extended

`avoid_crossing_perimeters` gained a staged clearance (0.6, 0.35 and 0.15 bead) so a travel
pressed between nested walls can still be routed, and a raster A* router for layers with more
walls than the visibility graph can hold. Measured on `fixtures/stress_gear.stl`: 777
crossings with the switch off, 757 with the graph; the staged clearance and the raster router
took it to 759 and 758 at twice the emit time. Both were removed. The remaining crossings are
geometric, not routing failures: a travel that starts inside a closed loop cannot leave
without crossing it, and two islands printed in sequence need a route that does not exist at
that layer. Closing those is layer-level planning, and the row stays `partial` until that
exists. The measurement is in the ledger row so the next attempt starts from it.

## Binary G-code: built from the specification

`binary_gcode` writes the bgcode container. Its owner publishes a byte-level specification
(`libbgcode/doc/specifications.md`): file header, block order, block header fields, per-type
parameters, checksum. That is a document about bytes, not slicer code about sampling, which
is what the clean-room rule protects — so `crates/gcode/src/bgcode.rs` was built from it, and
`PROVENANCE.md` names it. No `libbgcode` or PrusaSlicer source was read.

The alternative, deriving the layout from files the reference produced, was rejected: a
hand-derived container that round-trips the fixture and corrupts a print is exactly the class
of defect this repository exists to avoid. The specification was instead implemented twice —
the writer/reader in Rust, and `scripts/bgcode-decode.py` written from the same document with
no shared code — and the gate runs the second against the first: the Python decoder reads the
file the engine wrote, and its text matches the ASCII file's byte for byte.

What a file holds: a ten-byte header naming CRC32, INI metadata blocks (producer, machine,
print totals, slicer), one block per thumbnail, and the G-code verbatim in a block of its
own. Nothing is compressed and nothing is MeatPack-encoded; the specification allows both
`Compression = 0` and `Encoding = 0`, so those are honest values rather than a placeholder,
and a block that claims either is refused by name rather than decoded on a guess. MeatPack
and Deflate stay a size optimisation to add if a printer asks for them.

Two things the implementation settled, both worth knowing before extending it:

- **A stored block's header is eight bytes, not twelve.** Type and compression are two bytes
each, the uncompressed size is four, and the compressed size is the field the file leaves
out when compression is 0. The writer first emitted all four fields; the Rust reader agreed
with it because both were written from one reading, and the independent decoder caught the
disagreement at the first block. That is the reason the second implementation exists.
- **With compression 0 the checksum's coverage is unambiguous.** The specification places a
  block checksum after the block but does not say whether it covers stored or uncompressed
  bytes; while nothing is compressed those are the same bytes, so the question cannot bite.
  It would the moment a compressed block were written, which is one more reason this writer
  emits none.

The text is stored as the ASCII writer produced it, thumbnails aside: the container changes
where the images live and what a corrupt file fails on, never what the machine prints. On
the cube with a 64x64 PNG and a 32x32 QOI, that is 175,440 bytes of container against
176,794 bytes of text and the same G-code. The C ABI still returns the text
(`hfw_gcode_text`); the container is the command line's, and the application gets it when it
asks for bytes rather than a string.

## A volume is addressed by an id, and it carries its own placement

Two changes to the volume half of the plate document, both forced by the same thing: the
application's document addresses a volume by a number it keeps and shows, and its volumes
have a transform of their own inside the object they belong to.

**Identity, not position.** `add_volume` returns a `VolumeId`, and `remove_volume`,
`set_volume_setting`, `volume_mesh`, `volume_settings_json`, `volume_kind` and
`hfw_plate_volume_set` all take one. Removing a volume from the middle of a list renumbers
everything after it, so a position is not an identity: a settings panel, a selection or an
undo step holding "volume 1" would silently retarget itself after the user deleted volume 0.
Ids are per object, never zero and never reused, exactly as object ids are on the plate, and
`hfw_plate_object_volume_ids` reports them in insertion order so a read-back can rebuild the
mapping. They are session identity and not file identity: a reopened project reassigns them
in order, the same way reopening a plate reassigns object ids.

**Placement, not baked geometry.** A volume keeps a `MeshTransform` that composes on top of
the object's, so moving a modifier inside its part is reversible and stays a number a panel
can edit. The slice applies the volume's placement and then the object's, which is two mesh
transforms per volume per slice — the alternative, composing the two placements into one
`T·R·S`, is not closed under composition (a scaled object times a rotated volume shears) and
would have to refuse legitimate placements. Baking at add time was the third option and it
was rejected for the same reason the object keeps its placement rather than its triangles:
an operation a user can undo must not be a mesh rewrite.

The project part grew a `placement` attribute on `<volume>` and the format version moved to
9; an older file opens with every volume at its own coordinates, which is what version 8
meant. Reverses by dropping the id type and the placement field, and by writing no
`placement` attribute — files then read as version 8 plates.

## An undo step is a snapshot, and the geometry in it is shared

`Plate::snapshot` returns the whole document and `Plate::restore` puts it back. The
choice that makes it usable is that geometry is not copied: `PlateObject` and
`PlateVolume` hold `Arc<Mesh>`, so a snapshot is one pointer per object, and the first
write to an object after a snapshot copies that object's mesh through `Arc::make_mut`
and leaves the snapshot with the triangles it was taken with.

The alternative was a snapshot of the *plate's* bytes — a project file in memory — which
is a full serialisation and reparse per undo step: on a plate of 25 MB STL parts that is
seconds and a second copy, and it makes undo depend on the file format staying readable.
The second alternative was sharing the whole `PlateObject` behind one `Arc`, which shares
paint and settings too; it was rejected because `Plate::objects()` hands out a slice of
objects and a `Vec<Arc<PlateObject>>` cannot, so the catalogue would have been paid for
by every caller to save a copy of a byte per triangle.

What a snapshot holds is everything: objects with their ids, geometry, placements, paint,
settings, volumes, layer heights, the timeline, the in-layer strokes, the caller's bytes
and the next-id counter. A partial snapshot is a trap — the caller restores it and the
document prints differently from the one it claims to reproduce — and the counter is in
there so undo followed by an edit cannot reissue an id a selection or a paint session
still holds. Reverses by dropping the snapshot module and the four exports; nothing else
depends on them, and the `Arc` stays a `Mesh` again with one `Arc::new` removed per
construction.

## Several plates live in one handle, indexed like the application's plate bar

A document is N build plates. The application holds them positionally -- `plateCount`,
`activePlate`, `object.plate`, a `QVector<bool>` of locks and a `QStringList` of names --
renumbers `object.plate` when a plate is removed, and hands out object and volume ids
from a single counter over all plates.

`crates/plate` now holds the same thing: `Vec<Bed>` (name, lock) indexed 0..count-1, one
object list where each object carries its bed, and one id counter for the document.

**Why not one handle per plate.** The alternative shape is a plate record with its own
handle, which reads better in isolation and is wrong here for two measurable reasons:
the application's plate bar is an index (a plate is "plate 3", not "the plate this
handle names"), so every gesture would need index-to-handle lookup; and object ids are
global and monotone, so two handles would have to share a counter anyway -- a second
thing to keep in sync where there is now one. The engine's job is to hold the
document's data, not to improve on the application's model of it.

What that fixes for callers: an object id means one object in the whole document. It
survives a move between plates, it is retired when its plate is removed rather than
reused, and `hfw_plate_object_bed` answers "where is it" without a read-back of every
plate. Removing a plate renumbers the plates after it, which is the renumbering the
application already performs on its own objects, so a project file and a live document
agree.

The same commit gives a volume a name and a visibility, because a document that stores
neither is one the application cannot hand over: `setVolumeVisible` had nowhere to go.
Visibility is not cosmetic -- an invisible volume is skipped when slicing, so a hidden
negative volume stops cutting -- so it is part of the model rather than of the view, and
it is in the project file (version 10) and in a snapshot.

Reverses by keeping one bed: the bed array stays a one-element vector, `object.bed` is
always 0, and the twelve bed exports go away.
