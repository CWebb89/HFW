# Painting, in the engine

What "paint" has to mean in HFW, in what order, and why the first step is the one
it is. Written 2026-11-20 because painting is the capability that decides whether
the application can leave the C++ engine at all (`~/dev/next-layer/upstream/`), and
because doing it badly would mean two paint models — the app's and the engine's —
that disagree about what the user painted.

## What the application paints today

Three independent channels, each a per-triangle state on the object's mesh
(`Slic3r::Domain::TriangleSelector::TriangleSplittingData`):

| channel | what a painted facet means |
| --- | --- |
| support | an *enforcer* (support must reach it) or a *blocker* (no support here, ever) |
| seam | the seam may, must, or must not be placed on this facet |
| tool | which extruder prints this facet |

The app paints by ray-casting from the cursor into the mesh and writing the hit
triangle's state, with a brush radius in millimetres. Painting survives a project
save (the 3MF carries the states) and reaches the slicer through the project file.

## The model

One channel per purpose, one byte per triangle per channel:

```rust
/// Per-triangle paint, indexed by the mesh's own triangle order.
pub struct Paint {
    /// 0 none, 1 enforcer, 2 blocker.
    pub support: Vec<u8>,
    /// 0 none, 1 preferred, 2 enforced, 3 blocked.
    pub seam: Vec<u8>,
    /// Extruder index, 0 for the first.
    pub tool: Vec<u8>,
}
```

Rules that keep it honest:

- **Empty means unpainted.** A channel shorter than the triangle count is treated as
  "no state anywhere", so an object that was never painted costs nothing.
- **Paint is an input to a decision, never a decision.** A support enforcer adds
  overhang seeds; a blocker removes them. Nothing paints support *paths* directly,
  so a painted plate still goes through the same support generator, verifier and
  G-code writer as an unpainted one.
- **Paint follows the mesh through transforms.** `hfw_mesh_transform` and
  `hfw_plate_object_placement` move triangles without reordering them, so states
  stay valid; `bake` likewise. Cut, split and repair that change the triangle order
  must clear the states rather than guess.
- **A refused paint is refused loudly.** A state vector of the wrong length, or a
  channel the current object cannot use, is an error with the reason, never a
  silently ignored write — the application's rule against a control that does
  nothing applies to the ABI too.

## ABI

```c
/* Sets one channel's states, one per triangle in the mesh's own order. */
int32_t hfw_plate_object_paint(HfwPlate *plate, uint64_t id, int32_t channel,
                               const uint8_t *states, size_t len);

/* Reads them back, so the application can render what the engine holds. */
int32_t hfw_plate_object_paint_get(const HfwPlate *plate, uint64_t id,
                                   int32_t channel, uint8_t *out, size_t len);

/* Channels: 0 support, 1 seam, 2 tool. States are the tables above. */
```

`hfw_mesh_*` gets the same pair so painting does not require a plate, and
`hfw_plate_project_bytes`/`hfw_plate_open_project` carry the channels in
`Metadata/hfw_project.xml` beside the per-object settings they already carry.

## Order of work, and what each step delivers

1. **Support paint.** The highest value and the only channel with a real geometric
   consumer: `supports::seed_masks` decides where support starts, and painting is
   exactly a statement about that decision. An enforcer adds seeds inside the
   painted footprint at the painted heights; a blocker removes them. Tested by
   slicing the same object twice, once painted, and requiring the support path
   count to rise and fall by the painted area.
2. **Seam paint, done.** `hfw_engine::seam_paint` carries the facets themselves — a
   seam is a point on a wall, so the distance from a candidate vertex to the painted
   face is what decides — and the writer consults them twice. `path_from` puts an
   outer wall's seam on the rear-most painted vertex; the aligned-seam pass leaves a
   painted loop pinned instead of pulling it back to the seam one layer below, and
   never lands on a blocked vertex. Reach is one bead width, because the outer wall's
   centre-line runs half a bead inside the surface the facet lies on. Measured by
   slicing a cube twice: unpainted, the seam starts on one corner, painted, on the
   painted face. A loop whose every vertex is blocked keeps the profile's seam — a
   seam has to start somewhere — and that is the one place paint yields.
3. **Tool paint.** Needed for a tool changer, and blocked on multi-material being
   out of scope today (`docs/SCOPE.md`). Nothing carries it yet, because a channel
   nothing consumes is a field a panel could write and no stage would read.

## What it costs from here

Tool paint is a plate-level change to how stacks are assigned to extruders, and it
cannot start until the machine side exists.

## What this does not do

- **It does not make HFW a painting application.** Brush geometry, the cursor ray,
  the brush radius and the undo stack stay in the application; the engine owns the
  states and what they mean to the toolpath.
- **It does not paint text, cut or hollow.** Those are still declined in
  `crates/config/assets/settings-surface.json` under `capabilities`, and a paint
  channel does not change that.
