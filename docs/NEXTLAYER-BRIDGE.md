# Next-layer bridge audit (P8)

Read-only toward next-layer. Written 2026-11-19 against
`~/dev/next-layer/src/qt/src/HfwEngine.cpp` at the state on disk. No file in
next-layer was modified.

`next-layer-qt` is not built in this checkout (`build/src/next-layer-qt/` is
absent), so nothing in this file that needs the application binary was measured.
Those rows say **not measured**; the interface facts come from the C++ that runs
it.

---

## P8.1 How a setting reaches HFW

The application does not pass settings one by one. It writes its settings model to
a JSON config (`--dump-config <path>`, or a preset the user saved), and hands the
engine that file plus one STL per object:

```
hfw-cli --preset <config.json> <object 1>.stl <object 2>.stl ... \
        --no-arrange --gcode <out.gcode> [--object-settings <overrides.txt>]
```

`--no-arrange` is deliberate: the plate geometry is the application's, and the
engine must not lay the objects out again. Per-object overrides travel in a text
file of `<input index> <setting> = <value>` lines, one section per object index in
the order the models were handed over. The engine reports any override it could
not apply on stdout, and the application forwards those lines to stderr.

| setting | how it reaches HFW | value via `hfw-cli --profile` | match? |
|---|---|---|---|
| layer height, first layer height | config key → `layer_height` / `first_layer_height` | same key | same reader, not measured |
| wall count | config `wall_loops` | same | same reader |
| outer wall width | config `outer_wall_width` | same | same reader (applied since the wall rewrite) |
| first layer width | config `first_layer_extrusion_width` | same | same reader |
| gap fill | config `gap_fill_enabled` | same | same reader |
| infill density, pattern | config, per object possible | same, per object via `--object-settings` | same reader |
| infill overlap | config `infill_overlap` | same | same reader |
| seam position | config `seam_position` | same | same reader |
| support enable, style | config `support_material*` | same | same reader |
| printer limits, temperatures | the preset the app exported | same | same reader |
| resolution | config `resolution` | same | same reader |
| plate arrangement | `--no-arrange`, application owns X/Y | `--no-arrange` on the CLI | match by construction |

**Not measured**: the G-code diff of the same model sliced through the application
versus through `hfw-cli --preset`. That needs the application built and a dumped
config, and it is the one acceptance item of P8.1 still open.

## P8.2 Does next-layer rewrite HFW's output?

No. `HfwEngine.cpp` writes each object's mesh to a temporary STL, runs the engine,
checks the exit status, checks the G-code file is non-empty, and checks the
engine's own verifier line (`verify RESULT PASS`). It reads the file for preview.
It does not edit toolpaths, reorder moves or rewrite G-code.

## P8.3 Coupling inventory

Files under `next-layer/src/qt/src` that reference libslic3r types are the parts
that would need HFW to answer instead. The types that matter, and what HFW must
expose for them:

| libslic3r surface | used for | HFW must expose |
|---|---|---|
| `Slic3r::Model` / `ModelObject` / `ModelVolume` | plate model, per-object transforms and settings | a plate JSON: objects with mesh handle, transform, per-object settings |
| `Slic3r::Print` / `PrintObject` / `Layer` | slice results for preview | per-layer toolpaths with role, per-vertex width and height, Z |
| `Slic3r::ConfigBase` / `DynamicPrintConfig` | settings model, per-object overrides | settings JSON in the same shape the CLI already reads, plus a report of what could not apply |
| `Slic3r::Pointf3s` / `Polygon` / `ExPolygon` | preview geometry | the toolpath buffer above; no polygon types need crossing the boundary |
| `Slic3r::GCodeProcessor` / `GCodeAnalyzer` | preview and estimates from G-code | either the toolpath buffer (preferred) or keep the reader in next-layer |
| `Slic3r::place_objects` / arrange | plate layout | `hfw-cli` already packs and centres; the application wants arrange as a callable operation |
| progress reporting, cancel | UI | callbacks in the ABI below |

### The C ABI (`crates/capi`)

**Landed 2026-11-20, version 1 — meshes and the plate document.** What shipped is
the mesh-and-slice half of the block below, flat and handle-based, declared in
`crates/capi/include/hfw.h`:

```
hfw_abi_version() -> u32
hfw_settings_surface_json() -> *const c_char     // static, process-lifetime, do not free
hfw_last_error() -> *const c_char                     // per thread, cleared by success

hfw_mesh_load_file(path) -> *mut HfwMesh              // STL, OBJ or 3MF
hfw_mesh_load_extensions() -> *const c_char            // static: ".3mf,.stl,.obj"
hfw_mesh_stl_bytes(const *HfwMesh, binary, *out_len) -> *mut c_char  // STL out, hfw_buffer_free
hfw_mesh_from_bytes(bytes, len) -> *mut HfwMesh       // STL bytes
hfw_mesh_transform(const *HfwMesh, HfwTransform) -> *mut HfwMesh   // new handle, source kept
hfw_mesh_bounds(const *HfwMesh, *HfwBounds) -> i32
hfw_mesh_triangle_count(const *HfwMesh) -> usize
hfw_mesh_vertex_count(const *HfwMesh) -> usize          // reading geometry back out
hfw_mesh_vertices(const *HfwMesh, double *out, count) -> i32
hfw_mesh_index_count(const *HfwMesh) -> usize
hfw_mesh_indices(const *HfwMesh, uint32_t *out, count) -> i32
hfw_mesh_paint_set(*HfwMesh, channel, const u8 *states, len) -> i32
hfw_mesh_paint_get(const *HfwMesh, channel, u8 *out, len) -> ptrdiff_t
hfw_mesh_new(const double *vertices, count, const uint32_t *indices, count) -> *mut HfwMesh
hfw_mesh_volume(const *HfwMesh) -> f64
hfw_mesh_free(*HfwMesh)

hfw_plate_new() -> *mut HfwPlate
hfw_plate_add(*HfwPlate, const *HfwMesh) -> u64       // copies the mesh; 0 on failure
hfw_plate_add_positioned(*HfwPlate, const *HfwMesh) -> u64
hfw_plate_object_count(const *HfwPlate) -> usize
hfw_plate_object_name(*HfwPlate, id, name) -> i32
hfw_plate_object_placement(*HfwPlate, id, HfwTransform) -> i32
hfw_plate_object_bake(*HfwPlate, id) -> i32
hfw_plate_object_enabled(*HfwPlate, id, i32) -> i32
hfw_plate_object_remove(*HfwPlate, id) -> i32
hfw_plate_object_bounds(const *HfwPlate, id, *HfwBounds) -> i32
hfw_plate_bounds(const *HfwPlate, *HfwBounds) -> i32
hfw_plate_object_add_volume(*HfwPlate, id, kind, const *HfwMesh) -> i32
hfw_plate_object_remove_volume(*HfwPlate, id, index) -> i32
hfw_plate_object_volume_count(const *HfwPlate, id) -> ptrdiff_t
hfw_plate_object_volume_kind(const *HfwPlate, id, index, *i32) -> i32

// Reading the document back, so a plate can be rebuilt from one
hfw_plate_object_ids(const *HfwPlate, u64 *out, len) -> usize      // count even for a NULL out
hfw_plate_object_name_get(const *HfwPlate, id, char *out, len) -> ptrdiff_t
hfw_plate_object_placement_get(const *HfwPlate, id, *HfwTransform) -> i32
hfw_plate_object_enabled_get(const *HfwPlate, id, *i32) -> i32
hfw_plate_object_mesh(const *HfwPlate, id) -> *mut HfwMesh          // copy, paint included
hfw_plate_object_settings_json(const *HfwPlate, id) -> *mut c_char  // what `_set` accepts
hfw_plate_object_volume_mesh(const *HfwPlate, id, index) -> *mut HfwMesh
hfw_plate_object_volume_settings_json(const *HfwPlate, id, index) -> *mut c_char
hfw_plate_set_in_layer(*HfwPlate, const f64 *zs, const usize *counts,
                       const f64 *widths, const f64 *heights, const f64 *speeds,
                       const usize *starts, const f64 *points,
                       const char *const *prefixes, count) -> i32
hfw_plate_in_layer_count(const *HfwPlate) -> ptrdiff_t
hfw_plate_in_layer_get(const *HfwPlate, index, *f64 z, *f64 width, *f64 height,
                       *f64 speed, *f64 points, capacity, char *prefix, len) -> i32
hfw_plate_set_markers(*HfwPlate, const i32 *kinds, const double *zs,
                      const char *const *texts, count) -> i32
hfw_plate_marker_count(const *HfwPlate) -> ptrdiff_t
hfw_plate_marker_get(const *HfwPlate, index, *i32 kind, *f64 z, char *text, len) -> i32
hfw_plate_object_layer_heights(*HfwPlate, id, const double *pairs, count) -> i32
hfw_plate_object_layer_heights_get(const *HfwPlate, id, double *out, len) -> ptrdiff_t
hfw_plate_slice(const *HfwPlate, profile_path, first_layer_height, layer_height) -> *mut HfwGcode
hfw_plate_free(*HfwPlate)

hfw_slice_meshes(const **HfwMesh, count, profile_path, first_layer_height, layer_height) -> *mut HfwGcode
hfw_gcode_text(const *HfwGcode) -> *const c_char      // NUL-terminated, owned by the handle
hfw_gcode_len(const *HfwGcode) -> usize
hfw_gcode_verified(const *HfwGcode) -> i32            // the verifier's own verdict
hfw_gcode_free(*HfwGcode)
```

The plate half is phase 2's document model: objects keep a stable id (never zero,
surviving the removal of their neighbours), a name, geometry, a placement and an
enabled flag. Moving an object does not rewrite its triangles —
`hfw_plate_object_bake` is the call that folds the placement in, for a caller whose
own transform stack has finished composing. Object ids are what a view holds, so
`hfw_plate_object_remove` leaves every other id valid.

A volume is geometry attached to an object, sharing its placement so a move takes
it along: `HFW_VOLUME_PART` and `HFW_VOLUME_NEGATIVE` change the layer's outline
(the first unites, the second subtracts, both before walls are planned, so a hole
gets walls of its own), and `HFW_VOLUME_SUPPORT_BLOCKER`/`_ENFORCER` act over the
volume's whole height through the same mask a painted facet uses. A *modifier*
volume — one carrying settings of its own — is `HFW_VOLUME_MODIFIER`, and its settings
arrive through `hfw_plate_volume_set` (see below).

Layer heights are per object, as `(z, height)` pairs: a layer whose bottom sits at
or above a `z` uses that height until the next pair takes over, and an empty list
means the plate's uniform height. `by layer` merges the objects on **Z**, not on
layer number — two objects stepping differently do not agree on what layer 7 is —
so the file changes Z whenever either object's next layer is higher. Objects that
step alike therefore come out exactly as a round-robin put them.

Markers are the plate's timeline: a command at a print height, of the three kinds the
writer knows — custom G-code, a pause, a colour change. They belong to the plate and
not to the profile, because two plates on one printer can pause at different heights.
A typed marker takes its command from the printer profile (`pause_print_gcode`,
`color_change_gcode`); a user with their own line to write wants a custom marker,
whose text is written verbatim. A tool-change marker is not in the set: HFW has one
extruder and no per-tool filament, so emitting `T{n}` would name a tool the engine
cannot know exists.

A modifier volume is the fifth kind: it prints the part of the object it covers with its
own settings, and those arrive as settings keys through `hfw_plate_volume_set`. A
modifier with no settings is refused at slice time instead of being weakened to a part,
because a part-shaped copy of the region would print every bead in it twice.

In-layer strokes are the calibration channel: beads printed inside a layer that the
model does not describe, because no slice can produce them and each band is printed at
its own width, height and speed on purpose. They are written as real beads — travel,
extrude, retract — so E, position and the time estimate agree with the rest of the
file, under `;TYPE:Custom`, the label both reference slicers use. `prefixes` is G-code
written immediately before a stroke, which is how a temperature band changes the nozzle
temperature at the moment it starts drawing.

### Reading a plate back (2026-11-20)

A document the application holds has to survive being handed to the engine and
returned, or the port can only go one way. Every part of a plate is therefore
readable: the ids (in insertion order, `hfw_plate_object_ids` reporting the count
even for a NULL buffer so one call sizes another), each object's name, placement,
enabled flag, settings and geometry with **its paint attached**, and each volume's
id, kind, placement, geometry and settings (`hfw_plate_object_volume_ids`, then the
id-addressed getters). `hfw_plate_object_settings_json` returns exactly the
key/value rows `hfw_plate_object_set` accepts, so a rebuilt plate takes the same
settings it gave.

`next-layer`'s `--plate-readback-test` is the measure: it builds a document with a
modifier, a per-object layer height, an override, 360 painted facets, a marker and a
stroke; reads all of it back through the ABI; builds a *second* plate from nothing but
what was read; and requires the two project files to be identical byte for byte
(13,650 bytes). A field the engine holds but cannot give up therefore fails a gate
rather than going missing quietly.

### An undo step is a snapshot, and it costs pointers (2026-11-21)

Reading a plate back answers "what is in the engine"; undo asks a second question —
"what was in it, so it can be put back". `PlateDocument::Snapshot` holds the objects,
the selection and the active plate, and it is cheap only because the app shares each
object's immutable mesh between snapshots; a document the engine owns has to be able
to do the same thing, or every undo step is a copy of the plate.

`hfw_plate_snapshot` returns the plate as an opaque handle, and `hfw_plate_restore`
puts one back. What it holds is the whole document — objects with their ids,
geometry, placements, paint, settings, volumes and layer heights, the timeline, the
in-layer strokes, the caller's bytes, and the counter that hands out the next id —
because a restore that left any of those behind would be a document that prints
differently from the one it claims to reproduce. The id counter is in there so that
undoing and then adding a part cannot hand out an id a selection, a paint session or
another undo step still holds.

**The geometry is shared, not copied.** Each object's mesh is behind an `Arc` in
`crates/plate`, so a snapshot is one pointer per object and one counter bump; the
first write to an object after that copies its geometry (`Arc::make_mut`) and leaves
the snapshot holding the triangles it was taken with. That is the same trade the
application already made with `shared_ptr<TriangleMesh>`, which is why a stack of
undo steps here costs the document plus the edits made since, not one document per
step — `crates/plate/src/snapshot.rs` asserts the pointer identity, the copy on
write, and that a snapshot outlives its plate and restores into another one.

Two consequences worth stating. A snapshot is *not* a project file: restoring keeps
the plate in memory and touches no bytes, so an undo step neither re-parses 25 MB of
STL nor risks the file being the thing that disagrees. And a restore is total: what
the snapshot does not hold, the plate does not keep.

The visible defect this closed while being tested: `Settings::key_value` wrote a
percentage as `f64` multiplication would, so a panel that set 55 % read back
`55.00000000000001%`. It now prints the shortest decimal that parses back to the
share it came from, and `crates/engine/src/settings_key.rs` pins six of them — a
read-back that a settings panel shows has to be the text the engine took.

### What next-layer has not declared (2026-11-21)

`next-layer/src/app/src/hfw.rs` declares **52** of the **121** functions
`crates/capi/include/hfw.h` exports, so a 69-row gap sits between what the engine
can do and what the application has told its own compiler about. Recounted
2026-11-21 by adding exports, against the header and against that file, one row
per symbol:

    grep -oE '\bhfw_[a-z0-9_]+\s*\(' crates/capi/include/hfw.h | sort -u | wc -l
    grep -oE '\bfn hfw_[a-z0-9_]+' ../next-layer/src/app/src/hfw.rs | sort -u | wc -l

The number in the previous version of this paragraph, 28 of 89, was the gap measured
before the application's own boundary file grew: the mesh-and-slice core is still
the base of it, and the project file, the snapshot read-back, the timeline, the
in-layer strokes and the caller's bytes have since been declared too. What remains
is the document's edit gestures and the threaded job — **nothing is missing from the
engine for those**; the gap is on the application's side of the boundary.

Module names are next-layer's own: `HfwEngine` (the boundary helpers in
`HfwEngine.cpp` — snapshot, round trip, slice), `PlateDocument` (the document and
its edit gestures), `SliceRuntime` (the plate structs and the slicing call),
`PresetStore` (presets and the settings vocabulary). `—` means nothing in
next-layer would call it today.

| export | what it does | next-layer module | result |
| --- | --- | --- | --- |
| `hfw_settings_surface_json` | every setting with its dialect name, status and per-object flag, plus the capability backlog | PresetStore | static, do not free |
| `hfw_mesh_load_extensions` | the extensions `hfw_mesh_load_file` reads, for a file dialog | PlateDocument (`importWildcards`) | static, do not free |
| `hfw_mesh_part_count` | how many separate shells a mesh holds | — nothing splits shells today | none |
| `hfw_mesh_part` | one shell as its own mesh handle | — same | `hfw_mesh_free` |
| `hfw_text_mesh` | a text string extruded into a closed prism | PlateDocument (`Text.cpp`, emboss) | `hfw_mesh_free` |
| `hfw_plate_object_name` | renames an object in place | PlateDocument (`renameObject`) | none |
| `hfw_plate_object_bake` | folds an object's placement into its triangles | SliceRuntime (before the one-STL handoff) | none |
| `hfw_plate_object_duplicate` | copies an object and returns the copy's id | PlateDocument (`duplicateObject`) | none |
| `hfw_plate_object_rotate` | turns an object about its own centre on one axis | PlateDocument (`rotateSelection`) | none |
| `hfw_plate_object_mirror` | mirrors an object in place, winding corrected | PlateDocument (mirror tool) | none |
| `hfw_plate_object_set` | one print-level setting on one object, by name and value | PlateDocument (`setObjectSetting`) | none |
| `hfw_plate_object_apply_foreign` | applies a setting, or keeps it with why it was dropped | PlateDocument (`applyForeignSettings`) | none |
| `hfw_plate_object_foreign_set` | keeps a setting verbatim with its reason, for a rebuild | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_foreign_clear` | drops what an object kept from a foreign file | PlateDocument (re-import) | none |
| `hfw_plate_object_foreign_json` | the kept settings, their own text and why, as JSON | PlateDocument (the dropped-settings panel) | `hfw_string_free` |
| `hfw_plate_object_label` | renames an object as a user did, keeping its name | PlateDocument (`renameObject`) | none |
| `hfw_plate_object_label_get` | reads a label back apart from the name | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_source` | records the file the object came from | PlateDocument (`importObject`) | none |
| `hfw_plate_object_source_get` | reads that path back | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_dialect` | whose names the object's settings are written in | PlateDocument (export) | none |
| `hfw_plate_object_set_dialect` | records it, so a save writes the right vocabulary | PlateDocument (`applyForeignSettings`) | none |
| `hfw_plate_object_clear_settings` | drops every override an object carries | PlateDocument | none |
| `hfw_plate_object_add_volume` | attaches a part, negative or modifier volume, returns its id | PlateDocument (`addVolume`) | none |
| `hfw_plate_object_add_volume_placed` | the same, with the volume placed inside the object | PlateDocument (`addVolume`) | none |
| `hfw_plate_object_volume_placement` | moves or scales a volume inside its object | PlateDocument (`setVolumePosition`, `setVolumeScale`, `setVolumeRotation`) | none |
| `hfw_plate_object_volume_placement_get` | reads a volume's placement back as a matrix | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_volume_ids` | every volume id of an object, in the order they were added | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_volume_name` | names a volume | PlateDocument (`renameVolume`) | none |
| `hfw_plate_object_volume_name_get` | reads a volume's name back | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_volume_visible` | whether a volume is applied at all, not only drawn | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_volume_set_visible` | shows or hides a volume, which stops it cutting | PlateDocument (`setVolumeVisible`) | none |
| `hfw_plate_object_remove_volume` | detaches a volume by id | PlateDocument (`removeVolume`) | none |
| `hfw_plate_object_volume_count` | how many volumes an object carries | PlateDocument (`volumes`) | none |
| `hfw_plate_object_volume_kind` | which of the five kinds a volume is, by id | PlateDocument (`volumes`) | none |
| `hfw_plate_volume_set` | one setting on a modifier volume, on top of the object's | PlateDocument (`setVolumeSetting`) | none |
| `hfw_plate_object_bounds` | one object's bounds on the bed | PlateDocument (`volumeInfo`, object size) | none |
| `hfw_plate_object_ids` | every id on the plate, in insertion order | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_bed_count` | how many build plates the document holds | PlateDocument (`plateCount`) | none |
| `hfw_plate_bed_add` | appends an empty plate, returns its index | PlateDocument (`addPlate`) | none |
| `hfw_plate_bed_remove` | removes a plate and its objects, renumbering the rest | PlateDocument (`removePlate`) | none |
| `hfw_plate_bed_name` | names a plate | PlateDocument (`setPlateName`) | none |
| `hfw_plate_bed_name_get` | reads a plate's own name back | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_bed_locked` | whether a plate is read-only | PlateDocument (`plateLocked`) | none |
| `hfw_plate_bed_set_locked` | locks or unlocks a plate | PlateDocument (`setPlateLocked`) | none |
| `hfw_plate_add_on_bed` | adds an object to a named plate | PlateDocument (`addObject`) | none |
| `hfw_plate_object_bed` | which plate an object is on | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_set_bed` | moves an object between plates, keeping its id | PlateDocument (drag to another plate) | none |
| `hfw_plate_bed_object_ids` | the ids on one plate, in the order they were added | PlateDocument (`idsOnPlate`) | none |
| `hfw_plate_bed_slice` | slices one plate of the document | SliceRuntime (`sliceObjectsFor`) | `hfw_gcode_free` |
| `hfw_plate_object_name_get` | reads an object's name back | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_placement_get` | reads an object's placement back | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_enabled_get` | reads whether an object prints | HfwEngine (`readPlateSnapshot`) | none |
| `hfw_plate_object_mesh` | an object's geometry as a new handle, paint included | HfwEngine (`readPlateSnapshot`, round trip) | `hfw_mesh_free` |
| `hfw_plate_object_volume_mesh` | a volume's geometry as a new handle | HfwEngine (`readPlateSnapshot`) | `hfw_mesh_free` |
| `hfw_plate_project_bytes` | the plate as a project `.3mf`, with this engine's own part beside it | HfwEngine (`savePlateDocument`) | `hfw_buffer_free` |
| `hfw_plate_open_project` | a plate read back from project bytes | HfwEngine (`readPlateDocument`) | `hfw_plate_free` |
| `hfw_plate_snapshot` | the whole document as it is now, sharing its geometry with the plate | PlateDocument (`pushUndo`) | `hfw_snapshot_free` |
| `hfw_plate_restore` | puts a snapshot back, ids and id counter included | PlateDocument (`restore`) | none |
| `hfw_snapshot_free` | releases a snapshot; the geometry is shared, so this frees pointers | every undo stack that pops a step | owns itself |
| `hfw_snapshot_object_count` | how many objects a snapshot holds | — nothing needs it that cannot read the plate | none |
| `hfw_plate_slice_placed` | slices the plate after centring it on the profile's bed | SliceRuntime (`sliceWithHfw(placeOnBed)`) | `hfw_gcode_free` |
| `hfw_slice_meshes` | slices a bare list of meshes with no document | — every next-layer slice goes through a plate | `hfw_gcode_free` |
| `hfw_plate_preview_json` | every planned bead per layer as JSON, labelled as the file is | SliceRuntime (preview, phase 4) | `hfw_string_free` |
| `hfw_job_start` | plans a plate on its own thread, over a private copy of it | SliceRuntime (async slice) | `hfw_job_free` |
| `hfw_job_poll` | status and progress of that job, never blocking | SliceRuntime (progress bar) | none |
| `hfw_job_cancel` | asks the job to stop; a stopped job yields no file | SliceRuntime (cancel button) | none |
| `hfw_job_wait` | blocks until the job finishes | SliceRuntime | none |
| `hfw_job_take_gcode` | takes the finished file; the job handle is consumed | SliceRuntime | `hfw_gcode_free` |
| `hfw_job_free` | releases a job, cancelling and joining it first | SliceRuntime | owns itself |

Read as a work list, the split is clean: 33 rows are `PlateDocument`'s edit gestures
(name and label, duplicate, rotate, mirror, bake, settings and the foreign ones it
keeps, paint, volumes with their names, visibility, ids and placements, layer heights,
the plates and what moves between them, the undo step, the cut and text tools), 20 are
what `readPlateSnapshot` and the project file still need to read a plate back, 10 are
`SliceRuntime` and its timeline (slice of a plate, preview, strokes, the threaded job),
1 is `PresetStore`, 2 are the free functions every getter's caller needs, and 4 have no caller in next-layer at all:
the two mesh-part calls and `hfw_slice_meshes`, because a plate object and not a bare
mesh is the only thing the application slices, plus `hfw_snapshot_object_count`, which
reports what a snapshot holds when reading the plate after a restore says the same
thing.

### What a document carries across (2026-11-21)

next-layer's `src/app/src/bridge.rs` has settled how a document crosses: geometry as
binary STL bytes, a placement as sixteen `f64` row-major, paint as a triangle index and
a bitstream, layer heights as flat `(z, height)` pairs. Each is checked below against
the export that carries it, and the one that did not match was changed rather than
written down as a difference to live with. A volume needed the same treatment one
level down, which is where the fifth entry comes from.

**A volume is a placed, identified thing.** The fifth concept, and the one the port
graph's cycle needs most: `PlateDocument` addresses every volume gesture by a stable
volume id (`addVolume` returns one, `removeVolume`/`selectVolume`/`volumeInfo`/
`setVolumePosition`/`setVolumeScale`/`setVolumeRotation` take one) and its volumes carry
a transform of their own inside the object, while this ABI addressed volumes by
position in a list that renumbers on removal and could only take geometry baked at
add time. Volumes now have ids that are never zero and never reused, and a placement
that composes on top of the object's —
`hfw_plate_object_add_volume_placed`, `hfw_plate_object_volume_placement` and
`_get`, `hfw_plate_object_volume_ids` — so a volume can be moved inside its part
without rewriting triangles, and a removal or a read-back cannot make a panel or a
selection point at a different volume. The project file carries it as a tenth
number-group (`placement="…"` on `<volume>`, version 9); an older file opens with
every volume at its own coordinates.

**A plate is an index, and an object id means one object in the document.** The sixth
concept, and the last one a document needs before `PlateDocument` can hold nothing of
its own: the application has N plates, each with a name and a lock, and every object
carries the plate it belongs to. This engine had one bed per handle and no idea which
plate an object was on, so a document with three plates could only travel as the
application's own bytes in `metadata`.

The shape is a **bed array inside one handle**, not a handle per plate. The
application indexes plates positionally (`0..plateCount-1`), renumbers `object.plate`
when a plate is removed, and hands out object and volume ids from **one counter over
all plates**; a handle per plate would turn each of those into a mapping to maintain --
index to handle, and a counter shared between handles. So `crates/plate` holds
`Vec<Bed>` (`name`, `locked`) indexed the same way, one object list where each object
says which bed it is on, and one id counter for the document. Consequences worth
stating, because they are the questions the application will ask:

- **An object id is never per-plate.** It names one object in the document, it keeps
  its id when it moves between plates (`hfw_plate_object_set_bed`), and it is never
  reused after its object or its whole plate is removed -- the same rule ids already
  had, now across plates rather than within one.
- **Removing a plate renumbers the rest**, exactly as the application does to its own
  objects: an object on plate 3 is on plate 2 afterwards, and every id survives. The
  last plate cannot be removed, and a locked plate cannot be.
- **The lock is data here and is enforced where the engine changes the document's
  shape** -- removing the plate, moving an object onto or off it, adding to it. A
  front end disables the rest of that plate's gestures itself, which is what a lock
  means in the plate bar.
- **A plate slices on its own** (`hfw_plate_bed_slice`): the job carries that plate's
  objects and no other plate's, which is what makes `sliceObjectsFor(plate)` a call on
  the document instead of a filter in it. `hfw_plate_slice` stays the whole document,
  which on a one-plate document is the same thing; `hfw_plate_arrange` packs each plate
  in its own bed frame.
- **A volume has a name and a visibility, and hiding one stops it applying**, not only
  drawing it: `PlateDocument::sliceObjectsFor` skips an invisible volume, so a hidden
  negative volume cuts nothing. Both travel in the project file (format version 10) and
  in a snapshot.

**An object knows its label, where it came from, whose vocabulary it speaks, and
what it dropped.** The seventh concept, and the last thing `PlateDocument` keeps in a
side table keyed by object id: `Object` holds `name`, `label`, `path`,
`foreignSettings` and `orcaDialect`, and `displayName()` is `label` if it is set and
`name` otherwise. This engine had `name` alone, and `ignored_overrides` recorded a
refused key without its value or its reason, so anything the panel wanted to say about a
dropped setting had to live outside the document -- and outlive an undo only because the
snapshot happens to restore `next_id`.

The shape, and why each part is where it is:

- **Name and label are two fields.** `name` is what the file or model gave, `label` is
  the user's rename, and `display_name()` is the only one of the two a caller shows.
  One field would make a rename destroy what a save has to write back, and a rename no
  longer reversible.
- **The source path belongs to the object** (`source`, empty meaning "came from
  nowhere"), not to the caller's opaque bytes: a panel shows it, a reload needs it, and
  an undo that lost it is a document that cannot say where its parts came from.
- **A kept foreign setting is a record, not a name**: `ForeignSetting { key, value,
  reason }` with the **key and value verbatim in the file's own spelling** -- that is
  what a second save writes back out -- and a **typed** reason (`unknown_name`,
  `unusable_value`) rather than a sentence, so a panel can group ("9 names, 3 values")
  and a test can assert which. The reason is derived where the refusal happens, from
  `KeyError`, rather than parsed back out of an error message by the caller.
- **The dialect is per object** (`engine` or `orca`), because the vocabulary belongs to
  the object, not the plate: one document can hold a part from each slicer.
- **`apply_foreign_setting` applies or keeps, and says which**: the return value is
  "applied" or "kept", and neither is an error, because a file from another slicer
  carrying names this engine has never heard of is expected.
- **All of it rides the snapshot and the project file** (format version 11): an undo
  that lost why a setting was dropped would be a panel that lies about the document it
  is showing.

**A project file carries identities.** Found by auditing the phase-2 definition of done
rather than by adding a feature: the project part recorded each object's *index* and
nothing else, so opening renumbered every object and every volume from 1 in file order,
and the id counter restarted. A selection, an undo step or a paint session holding id 7
after a save named a different object, and a reopened document could hand out an id that
a removed object or volume already had. Format version 11 writes `id` and `next_volume`
per object, `id` per volume and `next_id` on the root, and the loader restores them by
position (`add_restored`, `adopt_last_volume_id`) rather than by looking an id up mid-load,
which is how a first attempt swapped two ids in a file that was itself consistent.

`next-layer`'s `ProjectFile.rs` is not a competing writer for the same bytes: it writes
the application's own JSON (`format: "next-layer project"`, version 1), which is now read
only, so a project saved before the document moved into the engine still opens.
`PlateDocument::saveProject` writes this engine's 3MF through `savePlateDocument`, which
is authoritative.

**Geometry — binary STL bytes.** `ProjectObjectRecord.mesh: Vec<u8>`, and the same for
a volume. Exports: `hfw_mesh_stl_bytes(mesh, 1, &len)` out, `hfw_mesh_from_bytes` in.
**Already that shape.** The writer emits the 84-byte header plus 50 bytes per triangle
and computes the face normal from the triangle, which is exactly what `from_bytes`
reads; `crates/mesh`'s two round-trip tests and the ABI test pin it. `hfw_mesh_new`
exists beside it for a caller that needs its own triangle order kept — triangle `i` of
the handle is triangle `i` of the indices, no welding and no triangle dropped — which
STL bytes do not promise once repair removes a degenerate one.

**Transform — sixteen `f64`, row-major, 4x4.** `ProjectObjectRecord.matrix`. Export:
`hfw_plate_object_placement`. **It was not that shape:** the declared call takes
`HfwTransform`, nine doubles of scale, Euler degrees and translation. Added
`hfw_plate_object_placement_matrix` and `hfw_plate_object_placement_matrix_get`, with
the conversion in `crates/mesh` (`MeshTransform::to_matrix`/`from_matrix`); covered in
`tests/abi.rs` and from C in `tests/smoke.c`.

- The conventions already agree. next-layer sets `m_matrix = rotation * scale` and the
translation after it, with `rotation` built as `Rz * Ry * Rx`
(`upstream/src/slicer-domain/src/Slic3r/Domain/Transformation.cpp:60` and `:149`), and
`MeshTransform` applies scale, then the X, Y and Z rotations, then the translation — the
same `T * Rz * Ry * Rx * S`.
- A matrix that shears, collapses an axis, or is not affine is refused with the reason,
never truncated: a scale triple and an Euler triple cannot carry it, and a silently
approximated transform is a part in the wrong place.
- The Euler pair stays. `hfw_plate_object_placement` is already declared in `hfw.rs` and
`hfw_mesh_transform` uses the same struct, so replacing it is a version 2 change and
next-layer's call, not a side effect of this one.

**Paint — one channel per object, addressed by triangle.** The project file holds
`{triangles: [i64 pairs], bitstream: "0110…", states: [1, 3]}`. Exports:
`hfw_plate_object_paint`/`_get` on an object, `hfw_mesh_paint_set`/`_get` on a bare
handle. **It is not that shape, and it should not be:** the ABI takes one `uint8_t` per
triangle. The sparse index-and-bitstream form is the project file's encoding; the
boundary's job is the state of a triangle, and the dense array costs one byte per
triangle — half a megabyte for a 500,000-triangle part, against a 25 MB STL.

- The state numbers, per channel: `HFW_PAINT_SUPPORT` (0) and `HFW_PAINT_SEAM` (1),
  each `0` none, `1` enforcer, `2` blocker. A bitstream carrying more than those three
  states per triangle has nowhere to go and must be refused by the caller.
- Triangle identity is the whole contract: a length that is not the handle's triangle
  count is refused rather than truncated, and `hfw_plate_object_mesh` hands the geometry
  back **with its channel attached**, so a read-back pair is always aligned. Geometry
  through STL bytes and paint through the ABI can only disagree if repair changes the
  triangle list in between, which is why `hfw_mesh_new` documents that it does not.

**Layer height profile — flat `(z, height)` pairs.** Export:
`hfw_plate_object_layer_heights`, read back by `_get`. **Already that shape**: the array
is interleaved, index 0 is `z`, index 1 is the height, and `count` is the number of
pairs. Two conventions to carry: a uniform object sends **zero pairs** rather than one
pair equal to the plate's layer height, and a layer whose bottom sits at or above a `z`
takes that height until the next pair, so the last pair stays in force to the top.

**Tool paint has no channel, and that is final.** HFW drives one extruder, so there is
nothing a third channel could change: the parity backlog lists multi-material as
`blocked` on the tool changer, the ledger's 56 multi-material rows sit in the same
bucket, and a channel nothing reads is a control that does nothing. next-layer's plan —
delete the engine's tool paint, keep support and seam — needs no engine work, and the
project file's `tool_paint` should be dropped on import with the reason shown rather
than carried in the hope of a later pass.

## Carrying it, until the port lands

`hfw-cli --plate <project> --profile <p.cfg> --gcode <out>` reads a plate file and slices
through `hfw_plate::Plate`, not through a bag of meshes. That is the only input that
carries paint, volumes, per-object layer heights, the timeline and the in-layer strokes:
an STL has nowhere to put any of them, so a GUI that ships STLs is a GUI that silently
drops every one of those capabilities. The run reports what the document carried
(`Carries: paint support on 12 facet(s); volumes 1 modifier, 1 negative; ...`) and warns
about an in-layer stroke whose Z matches no printed layer instead of dropping it in
silence.

A plate prints where the document put it, so `--plate` keeps every object's position;
`--place` centres the whole document on the bed instead, which is what the application
asks for when the model came from a command line and has no document around it. With a
profile, the profile decides every setting the job prints with and the document adds the
objects, their overrides, their volumes, their layer heights, the timeline and the
strokes; without one, the plate is sliced on the settings it carries.

The ABI carries both placements: `hfw_plate_slice` is the one that keeps the document's
coordinates and `hfw_plate_slice_placed` is the one that centres it, so an application
that holds a plate in memory never has to write a file to get the command line's
`--place`.

`hfw_slice_meshes` is the version-1 shortcut for a caller with no document. It was
called `hfw_plate_slice` for the first hour of its life, before the document existed
and the name was needed for the real thing; nothing outside this repository has been
built against the old name.

Meshes handed to a slice call are already placed on the bed: the ABI does not
arrange a plate, and the returned file keeps the coordinates it was given
(`keep_position`). A zero layer height takes the profile's own. The settings reader
and the plate merge are the engine's, not copies, so a preset means the same thing
through the ABI as through `hfw-cli`. A plate's own settings are what the profile is
layered onto for one job and not stored, so slicing under two profiles does not
leave the second on the document.

Nothing is missing from phase 4. The preview is `hfw_plate_preview_json`; the long
job is `hfw_job_start` and friends, polled rather than pushed. The job-shaped surface
below is still the target; what shipped is the part that had to exist first, and the
symbols above are the part of it that is now a promise.

The settings surface (`hfw_settings_surface_json`) is phase 3's document: one JSON
object with `counts`, `statuses`, `locations` and a `settings` array, each row
carrying `name` (the dialect name), `hfw` (this engine's key when it differs),
`tier`, `status`, `type`, `default`, `unit`, `location`, `category`, `source` and
`per_object`. A panel hides or disables every row whose status is not `applied` —
that is the ledger's first rule, enforced from data instead of from discipline — and
only offers a per-object control where `per_object` is true.

Setting one object's value across the ABI:

```c
if (hfw_plate_object_set(plate, id, "sparse_infill_density", "40%") != 0)
    show(hfw_last_error());     /* `…` is not a per-object setting, or what it accepts */
```

Saving and reopening a project:

```c
size_t len = 0;
char *project = hfw_plate_project_bytes(plate, &len);   /* a .3mf */
/* write `project`, `len` bytes, wherever the app keeps projects */
hfw_buffer_free(project, len);

HfwPlate *reopened = hfw_plate_open_project(bytes, len, profile_path);
```

The model part is a standard 3MF, so other slicers open the layout; the enabled flags
and each object's settings live in `Metadata/hfw_project.xml`. Geometry is stored where
it sits, so every object comes back marked positioned. A reopened plate slices to the
same bytes as the plate it was saved from — that is the test, not a promise.

Editing the plate:

```c
uint64_t copy = hfw_plate_object_duplicate(plate, id);   /* same place, same settings */
hfw_plate_object_rotate(plate, copy, 2, 90.0);           /* about its own centre */
hfw_plate_object_mirror(plate, copy, 0);                 /* in place, still slices */
hfw_plate_arrange(plate, 250.0, 250.0, 5.0);             /* shelf layout, centred */
```

Scale, translate and any composed placement go through `hfw_plate_object_placement`
with an `HfwTransform`. Nothing here rewrites triangles: an object keeps its geometry
and gains a placement, so a scale or a turn is exact and reversible until the job is
sliced.

Running the job without freezing the caller:

```c
HfwJob *job = hfw_job_start(plate, profile_path, 0.2, 0.2);
HfwJobProgress p;
while (hfw_job_poll(job, &p) == HFW_JOB_RUNNING) {
    redraw(p.stage, p.done, p.total);   /* your timer calls this */
}
HfwGcode *gcode = hfw_job_take_gcode(job);   /* NULL if cancelled or failed */
```

`hfw_job_cancel` stops the job between objects, and a cancelled job produces no file at
all. The plate is copied into the job, so it can keep being edited while the job runs.

Drawing the job before printing it:

```c
char *preview = hfw_plate_preview_json(plate, profile_path, 0.2, 0.2);
if (preview != NULL) {
    draw(preview);          /* {"layers":[{"index":0,"z":0.2,"paths":[{"role":"Outer wall",...}]}]} */
    hfw_string_free(preview);
}
```

Each path carries `role` (the `;TYPE:` label), `closed`, `flow`, `width` per vertex and
`points`, so a viewer needs no engine types and no G-code parse.

The object takes the plate's settings on its first override, so the value is a delta
from the job and the keys may arrive in any order. `hfw_plate_object_clear_settings`
drops the override and returns the object to the job's settings.

The rest of the proposal stands as written:

```
hfw_abi_version() -> u32

hfw_job_new(plate_json, settings_json) -> *mut HfwJob
hfw_job_add_mesh(*mut HfwJob, name, vertices_xyz_f32, triangle_count) -> i32
hfw_job_set_progress(*mut HfwJob, cb, *mut void)
hfw_job_cancel(*mut HfwJob)              // safe from another thread
hfw_job_run(*mut HfwJob) -> i32          // 0 ok, negative error
hfw_job_gcode_path(*mut HfwJob) -> *const c_char
hfw_job_layer_count(*mut HfwJob) -> usize
hfw_job_toolpaths(*mut HfwJob, layer, *mut HfwToolpathSlice) -> i32
hfw_job_report_json(*mut HfwJob) -> *mut c_char   // stats, warnings, unapplied settings
hfw_job_warning_count(*mut HfwJob) -> usize
hfw_job_warning(*mut HfwJob, index, *mut HfwWarning)
hfw_job_free(*mut HfwJob)
hfw_string_free(*mut c_char)
```

`HfwToolpathSlice` is `{ kind: u32, point_count, points: *const f32 (xyz), widths: *const f32, closed: u8, object: u16 }`.
`HfwWarning` is `{ code: u32, layer: i32, message: *const c_char }`.

Error taxonomy, the codes HFW already distinguishes: bad model (parse, coordinate
range), no closed cross-section on a layer (location and measured gap, with
`strict` making it fatal), plate fit, vase-with-support, and mesh preparation
failure. The report JSON carries per-class volumes, timings, the mesh repair
counters and the list of settings that could not apply — which is what the
application needs to show 175 of 352 controls rather than silently differing.

## P8.4 Preview data

Today the application previews by reading G-code through its own processor. Two
replacements are possible:

- **Structured toolpath buffer** (recommended): HFW hands over the same `Line`
  data it writes G-code from — role, per-vertex width, per-path flow, layer Z. No
  parsing, no inference, and the preview cannot disagree with the file.
  Cost: one ABI call per layer and a copy of roughly `points × (12 + 4)` bytes.
- **A Rust G-code reader** moved into the engine: keeps the current
  read-the-file shape, but re-derives role and width from E values, which is
  exactly the inference the buffer avoids.

Measured cost of either: **not measured** (needs the application built).

## Arrangement: the engine packs rectangles, so `Arranger` stays C++ (2026-11-21)

**Decision: silhouette/NFP packing is not in scope for `crates/plate`.** `hfw_plate_arrange`
is not a replacement for next-layer's `Arranger`, and next-layer should record `Arranger`
as blocked-with-a-reason rather than port it onto it.

The two are not the same algorithm with different constants:

| | `crates/plate/src/arrange.rs` | next-layer `Arranger.cpp` |
| --- | --- | --- |
| footprint | the object's XY **bounding box** plus half the spacing | the **silhouette**: `project_mesh` with the translation zeroed, the polylines unioned into `ExPolygons` (holes kept) |
| packer | MaxRects, best-short-side-fit, free space kept as rectangles | the engine's own **NFP** kernel, `Strategy::Overfit`, `scaled_offset = 3 mm` in int32 micrometres |
| orientation | a quarter turn, or none | whatever `allow_rotations` lets the nester try |

Why it stays out of the engine:

- **The engine is not the plater.** A plate document carries the position of every
  object and `--arrange`/`hfw_plate_arrange` leaves a positioned object where the file
  put it. Arrangement exists for a command line handed bare meshes; the layout a user
  sees is the application's, and it already runs there.
- **It is a different class of work, not a bigger table.** Rectangle packing is a shelf
  heuristic over a list of rectangles. Silhouette nesting needs the bed-plane outline of
  a mesh (projection plus an exact integer union of up to 100k edges), no-fit polygons
  between non-convex outlines *with holes*, and a search over placements and rotations.
  Rough cost, if it were ever in scope: a `minkowski`/NFP pair in `crates/kernel` (the
  workspace has no third-party maths library, so this is written here), ~1500–2500 lines
  in `crates/plate`, a packing-quality measure (bed utilisation, not "did it fit") and a
  fixture set of concave parts — and it must stay deterministic under `HFW_THREADS`.
- **The gain is real but small on this engine's inputs.** Measured 2026-11-21 by
  projecting each fixture's vertices and comparing the convex hull with the bounding
  box (`trimesh` + `shapely`, so an upper bound on what silhouette packing can recover;
  a concave part gains more, a cube gains nothing): sphere and cone 0.79, torus 0.79,
  `stress_gear` 0.66, `taper_wall` 0.54, and every box-like fixture 1.00. A rectangle
  therefore reserves up to 46% more bed than the part occupies — but on a 350 mm bed with
  a handful of parts that changes what fits only when the plate is nearly full, or when a
  part has a concavity another part could sit in, which is exactly the case a rectangle
  packer cannot do at all.
- **The gate cannot see the difference**, which is the trap: `arrange.rs`'s tests use
  cubes, and two cubes pack identically under both. A port that "passes" would have
  silently degraded every curved or concave part on a full plate.

If rectangle packing is ever judged good enough, that is a decision to record, not a
default: the cheap middle ground is the convex hull of the XY projection with
convex–convex no-fit polygons (a Minkowski sum, ~200 lines, quarter turns only), which
recovers the round-part case above and still cannot nest into a concavity. Until then
`Arranger` is a legitimate C++ holdout, and the one thing that must not happen is
`hfw_plate_arrange` being proposed as its replacement because both are called "arrange".
