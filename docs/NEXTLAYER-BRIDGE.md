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

### Proposed versioned C ABI (`crates/capi`)

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
