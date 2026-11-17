# What came from where

Two prototypes, one engine. This file records every borrowing so the design
choices stay auditable and the licence story stays honest.

## Sources

Both parents are original work written for this project: they are not ports of
Slic3r, PrusaSlicer or any other slicer, and no third-party source was copied
into either.

| Directory | Character |
|---|---|
| `../HFW-CLAUDE` | variable-width wall generator from a distance field and medial axis |
| `../HFW-GPT` | deterministic integer slicing pipeline with mesh diagnostics |

**HFW-final is MIT OR Apache-2.0.** An earlier revision of this file claimed
AGPL-3.0-or-later, on the grounds that the GPT package declared that licence and
permissive code can be absorbed into copyleft but not the reverse. That
reasoning was wrong twice over: the declaration was generated metadata, not a
grant, and there is no second party to hold a licence over either parent. The
same manifest invented `repository = "https://github.com/CWebb89/NextLayer"` and
`authors = ["NextLayer contributors"]`; it is scaffolding, not title.

The provenance claim that actually matters is the one against Slic3r, which is
AGPL-3.0. It is answered by architecture, not by a licence string:

| | Slic3r / PrusaSlicer | this engine |
|---|---|---|
| perimeter generation | Clipper-based polygon offsetting, fixed pitch ladder | distance field iso-lines with medial-axis bead decomposition |
| cross-section assembly | its own polygon layer pipeline | marginal intersection, segment deduplication, contour walk, integer nanometres |
| variable width | Arachne skeletal trapezoidation | per-normal-line half-thickness and bead count |
| shared code, names, class structure | — | none |

Inspection can only report absence of derivation, not prove it, and generated
code carries an irreducible memorisation risk that no diff review eliminates.
That caveat is the honest ceiling on the claim.

## Taken from HFW-GPT, unchanged in substance

| Component | Crate |
|---|---|
| checked integer nanometre coordinates, `Point2`, `Contour`, `Aabb2`, exact predicates | `hfw-geometry` |
| indexed mesh, ASCII/binary STL import, vertex welding, `MeshDiagnostics`, `repair_basic`, transforms | `hfw-mesh` |
| `LayerSchedule` with first-layer height and shortened final layer | `hfw-slicing` |
| triangle-to-layer bucketing with the capped-memory full-scan fallback | `hfw-slicing` |
| marching intersection, segment deduplication, contour assembly, open/branching rejection, outer/hole winding normalisation | `hfw-slicing` |
| typed error enums with explicit limits, `#![forbid(unsafe_code)]`, `missing_docs = warn`, workspace layout, `-D warnings` Clippy policy | every crate |

## Taken from HFW-CLAUDE, unchanged in substance

| Component | Crate |
|---|---|
| `V2`, `Bounds`, `Region` with nesting-depth winding, ring predicates | `hfw-kernel` |
| `SegIndex` / `PointIndex` uniform-grid nearest queries | `hfw-kernel` |
| uniform-grid signed distance field and row-scan inside mask | `hfw-walls::field` |
| medial-axis extraction, spur pruning, radius smoothing, bead-count hysteresis and blending | `hfw-walls::ridge` |
| `Params` / `Beading` decomposition | `hfw-walls::params` |
| marching-squares iso-line trace carrying bead width and target radius | `hfw-walls::contour` |
| Newton projection onto the exact radius, corner recovery, width-aware Douglas-Peucker, thin-wall classification and middle beads | `hfw-walls` |
| top/bottom skin masks | `hfw-infill` |
| wall-clipped rectilinear and grid infill with endpoint extension | `hfw-infill` |
| coverage, spill, width-violation metrics and volume accounting | `hfw-toolpath::metrics` |

## New in this merge

Everything below exists only here.

- **The seam.** `hfw_engine::region_from_contours` converts integer slice
  contours into the millimetre `Region` the wall stage consumes. One conversion
  point, no rounding beyond the float cast.
- **Kept both contour policies.** GPT detects and rejects unclosed
  cross-sections; Claude reported them and carried on. Merged as: detect always,
  skip and report by default (a cone tip must not abort a print), with `--strict`
  to restore hard failure. `hfw_slicing::slice_mesh_layers_lenient` is the
  mechanism, `slice_mesh_layers` remains the strict wrapper.
- **Reference geometry kept as a diagnostic.** The exact model cross-section is
  drawn dashed under the beads, so a wrong slice is visible instead of hidden.
  It is never counted as extrusion.
- **Symmetry fix in interpolation.** `interpolate_edge` canonicalises edge
  endpoints before rounding. Without it the same mesh edge, seen in opposite
  winding order by its two triangles, can return crossings one nanometre apart
  and the contour fails to close. This bug was live in the GPT build: a
  128-segment cone lost 14 of 150 layers to it. There is now a regression test.
- **Bounded skin masks.** Skin classification held the packed mask of every
  layer, which grows without limit. It now keeps a ring of
  `top + bottom + 1` masks.
- **Memory guard on the field.** The distance field allocates four parallel
  arrays per cell; a plate-sized part at 0.05 mm resolution needs several
  hundred megabytes. `MAX_FIELD_CELLS` fails with an actionable message instead
  of exhausting memory. The real fix is a tiled field, see `docs/ENGINE-PARITY.md` #7.
- **One path model.** `hfw_walls::Line` (points, per-vertex width, closed, kind)
  is the only printable path type; walls, middle beads, skin and infill all
  produce it, so counting, volume and export are single implementations.
- **Manifest and reports merged.** GPT's deterministic CSV plus Claude's
  material accounting and timings, in one `job_report.txt`.

## Deliberately dropped

- Claude's `Mesh`, `slice`, `stl` and CLI: superseded by the integer pipeline.
- Claude's `svg.rs`: replaced by a width-accurate renderer in `hfw-toolpath`.
- Claude's `shapes.rs` generators: replaced by committed STL fixtures.
- GPT's even/odd scanline infill: Claude's infill is wall-clipped, respects
  skins and supports grid; keeping both would have been two implementations of
  the same stage.
- GPT's `PlanFeature::BoundaryReference` as a path type: it is not printable, so
  it is carried as reference geometry instead of as a path.

## Verification of the port

The three published material-accounting ratios reproduce to the fourth decimal
(0.9963 cube, 0.9930 sphere, 0.9853 torus). Any structural mistake in the port —
a shifted grid, a lost jitter, a changed overlap — moves those numbers. The
integration test `solid_infill_reproduces_the_published_volume_ratios` asserts
them, so a regression fails the suite rather than being noticed later.
