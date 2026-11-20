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

hfw_mesh_load_file(path) -> *mut HfwMesh              // STL or 3MF
hfw_mesh_from_bytes(bytes, len) -> *mut HfwMesh       // STL bytes
hfw_mesh_transform(const *HfwMesh, HfwTransform) -> *mut HfwMesh   // new handle, source kept
hfw_mesh_bounds(const *HfwMesh, *HfwBounds) -> i32
hfw_mesh_triangle_count(const *HfwMesh) -> usize
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
volume — one carrying settings of its own — is not in the ABI, because a layer under
one carries two settings at once and the planner does not yet split a layer.

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

## Carrying it, until the port lands

`hfw-cli --plate <project> --profile <p.cfg> --gcode <out>` reads a plate file and slices
through `hfw_plate::Plate`, not through a bag of meshes. That is the only input that
carries paint, volumes, per-object layer heights, the timeline and the in-layer strokes:
an STL has nowhere to put any of them, so a GUI that ships STLs is a GUI that silently
drops every one of those capabilities. The run reports what the document carried
(`Carries: paint support on 12 facet(s); volumes 1 modifier, 1 negative; ...`) and warns
about an in-layer stroke whose Z matches no printed layer instead of dropping it in
silence.

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
