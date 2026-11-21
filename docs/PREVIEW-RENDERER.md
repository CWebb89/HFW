# What draws the toolpath once libvgcode is gone

The phase-4 question, answered before anyone starts it. This document costs the options
and states a recommendation; it is not the implementation.

**Whose decision is it?** Mostly next-layer's. The item that draws lives in
`src/qt/src/GcodeView.cpp`, in the application's scene graph, and the QML that uses it is
the application's. What makes it partly this repository's is the data: the preview
document is produced here (`hfw_plate_preview_json`, `crates/capi/src/preview.rs`), and
one of the options below would put a rasteriser *inside* this engine. So the numbers, the
data contract and this engine's position are recorded here; the choice of widget and the
QML wiring belong in next-layer's `docs/rust-engine-plan.md` phase 4, and that is where
the decision should be copied once taken.

## What the renderer has to do

Today, from `GcodeView.hpp` and `Toolpath.hpp` (2,272 lines across the two, plus
`PreviewData`):

- **10 QML-reachable names on `GcodeView`**: six properties (`cameraEye`, `cameraCenter`,
  `cameraUp`, `fov`, `initialized`, `frameCount`) and four signals
  (`initializedChanged`, `frameCountChanged`, `contentsLoaded`, `legendChanged`), used at
  `Viewport.qml:1275`.
- **Four view types** — feature type, height, line width, speed — plus per-role and
  per-option visibility (hide every outer wall; hide travel), each switching what the
  colour ramp means.
- **Per-bead width**, not a fixed line: the engine plans variable-width beads and the
  preview is how a user sees a thin wall.
- **A layer range and a move range**: `Toolpath` holds 21 properties and 14 invokables,
  including `layerMoveCount`, `firstMove`, `lastMove`, `layerZAt(z)`, `layerAtZ(z)`, and
  `attachView(GcodeView*)` taking a raw pointer from `Viewport.qml:1285`.
- A camera kept in step with the plate scene, which is a separate Qt Quick 3D scene on
  the other side of the same viewport.

## The measurements, and how to repeat them

Everything below was measured on this machine on 2026-11-21. Commands are given because a
number without one is a rumour.

| what | value | how |
| --- | ---: | --- |
| moves in a two-cube plate, 0.2 mm | 9,934 over 200 layers (50/layer) | `hfw-cli --plate fixtures/plate-overrides.3mf … --gcode /tmp/p.gcode; grep -c '^G1 ' /tmp/p.gcode` |
| moves in the sphere fixture, 0.2 mm | 44,560 over 150 layers (297/layer) | same, `fixtures/sphere.stl` |
| slice time, sphere | 313 ms | `time hfw-cli fixtures/sphere.stl …` |
| preview data per move, JSON with paths | 101 B/move (4,513,197 B for 44,560) | `hfw-cli fixtures/sphere.stl … --json --json-paths` |
| a 500k-move slice, by arithmetic | ≈1,700 layers at 294 moves, or 18 spheres; ≈1,000,000 vertices as line segments; ≈50 MB as the current JSON | the two rows above |
| **Qt's own raster engine, 500k × 2 px lines on 1600×950 RGB32** | **293 ms/frame** (1.7 Mlines/s) | a 40-line `QPainter::drawLines` benchmark, `-O2`, Qt 6.12, three passes, best taken |
| the same at 1 px | 440 ms (1.1 Mlines/s) | same benchmark, pen width 1 |
| the same at 100k lines | 59 ms | same benchmark |
| **libvgcode, 108,695 moves, whole app** | 7,348 ms wall, of which 2,500 ms is the smoke's settle timer | `QT_QPA_PLATFORM=offscreen next-layer-qt test.gcode --preview test.gcode --smoke` |
| **libvgcode, 543,475 moves (four copies concatenated)** | 7,868 ms → **+520 ms for +435k moves ≈ 1.2 µs/move** | same, with `cat test.gcode{,,,,} > /tmp/big.gcode` |
| **the current renderer on eglfs — the platform `tools/check` runs `--smoke` on** | **fails**: `GcodeView: offscreen surface invalid on eglfs`, then `smoke: GcodeView never initialized`, while the scene itself rendered `frame 1920x1080, covered 90.10%` | `QT_QPA_PLATFORM=eglfs next-layer-qt … --preview … --smoke` |
| the same on the offscreen platform | works: `gcode view: 4 frames` at 1600×950 | `QT_QPA_PLATFORM=offscreen …` |
| the app's own interactive budget | 40 ms, minimum scale 0.25 | `GcodeView.cpp:81-83` |

Two facts from the Qt 6 documentation, both load-bearing:

- `QSGGeometry` supports **unsigned 32-bit indices** (and 8- and 16-bit), so a single
  geometry is not limited to 65,535 vertices ([QSGGeometry](https://doc.qt.io/qt-6/qsggeometry.html)).
- `QQuick3DGeometry` takes `U32Type` index attributes and has `addSubset(offset, count,
  bounds, name)` — but its `Lines` primitive's width "may not be supported at run time",
  so a variable-width bead cannot be a line primitive
  ([QQuick3DGeometry](https://doc.qt.io/qt-6/qquick3dgeometry.html)).
- `QQuickRhiItem`/`QQuickRhiItemRenderer` are present in the installed Qt 6.12
  (`/usr/include/qt6/QtQuick/qquickrhiitem.h`), public API since 6.7.

## Option 1 — a Rust renderer produces the frame, the item paints it

The current architecture minus libvgcode: a rasteriser writes an RGBA image, and
`QQuickPaintedItem::paint()` blits it, which is what the item already does with
libvgcode's output today.

**Who rasterises:** this engine, on the CPU, into a `1600×950` buffer (or whatever the
item's size is), at whatever resolution the frame budget allows. **Frame budget:**
measured above — 293 ms for 500k moves at full size, 59 ms at 100k. To hold the app's
40 ms interactive budget the frame can carry about **68,000 lines**, which is 14 % of a
500k-move slice: the renderer must therefore draw a *subset* (the current layer plus a
neighbourhood is the honest one, and it is what the layer slider already selects: one
layer is ~294 moves, 0.2 ms) or decimate, and a decimated toolpath is a picture that
lies about density.

**Work:** projection to screen space, a depth bucket or painter's-algorithm ordering,
quad fills for width, the four colour ramps, role/option masks, and the layer/move range
windows. Roughly 800–1,200 lines in a new `crates/preview` plus a C entry point that
returns the frame (or `hfw_plate_preview_json` grows a binary sibling). No new
dependency.

**Riskiest unknown:** whether an honest level of detail exists. The measurement that
settles it: render the app's real job at 68k lines and at full count and compare the
images — if a user cannot tell, the budget is fine; if bands of missing infill appear,
this option needs the "current layer ± N" rule written down as a documented limit.

**Gate that catches a regression:** the best of the four. A frame is an array of bytes
with no GL in it, so `--smoke` passes on eglfs and offscreen alike, and a test can compare
two frames byte for byte — determinism the current renderer cannot offer. The layer
slider's mapping is tested by `--slider-test` exactly as today.

**What it gives up:** 293 ms full frames, a 6 MB blit per paint, and the GPU. It is the
option that guarantees the toolpath never reaches the 3D scene, so camera sync stays and
any future shading (curvature, overhang) is CPU work.

## Option 2 — Qt Quick 3D geometry, the way the plate already works

A `QQuick3DGeometry` subclass fed by Rust, in the same scene as the plate: the pattern
`MeshGeometry` already ships for plate meshes, and the camera sync disappears because it
is the same camera.

**Vertex counts.** A bead is a quad, so 4 vertices and 6 indices per move:

| moves | vertices | index entries | vertex buffer (24 B: pos + value + role) | index buffer (u32) |
| ---: | ---: | ---: | ---: | ---: |
| 9,934 (the plate fixture) | 39,736 | 59,604 | 0.95 MB | 0.24 MB |
| 44,560 (the sphere) | 178,240 | 267,360 | 4.3 MB | 1.1 MB |
| 100,000 (a typical app job) | 400,000 | 600,000 | 9.6 MB | 2.4 MB |
| 500,000 (the worst case) | 2,000,000 | 3,000,000 | 48 MB | 12 MB |

**Does it fit one upload?** Yes: 32-bit indices are supported, so a single geometry with
2M vertices is legal; the limit is the driver's buffer size, which is orders of magnitude
away on any GPU that runs this app. It is one `setVertexData`/`setIndexData` per load,
not per frame.

**The layer slider's vertex range mapping.** Today libvgcode owns it; here it has to be
explicit. Rust keeps two arrays as it builds the geometry — `layer_first_vertex[l + 1]`
and `layer_first_move[l + 1]` — which are what `firstMove`, `lastMove`,
`layerMoveCount`, `layerZAt` and `layerAtZ` are computed from, unchanged as a contract.
Setting a layer range then rewrites the index buffer (12 MB at the 500k worst case, ~5 ms
of memcpy and upload; 0.6 MB at a typical job) or, better, keeps one index buffer per
layer mode and swaps it. Role and option hiding go through the same path — a subset per
feature role via `addSubset`, and hiding a role means an index rebuild without its
entries.

**Work:** a `QQuick3DGeometry` subclass (~200 lines), the Rust geometry builder
(~300 lines, mostly the quad expansion and the two offset arrays), a `CustomMaterial`
with a fragment shader that turns the per-vertex value into the ramp colour (feature type
uses the role attribute directly), and the QML model binding. Roughly 500–800 lines.

**Riskiest unknown:** Qt Quick 3D's custom material pipeline being expressive enough for
the four view types, and the compositing order between 1M transparent quads and the plate
models — a toolpath drawn behind a part it should be inside is worse than a slow
toolpath. The measurement: build one `CustomMaterial` and one 100k-move geometry, then
compare a `--smoke --shot` frame against the current renderer's frame of the same G-code
on the same camera.

**Gate that catches a regression:** `--smoke --preview` (`initialized` and
`frameCount > 0`) and `--slider-test`. Both become *stronger*, because the toolpath is in
the scene the smoke frame already grabs — a toolpath that fails to draw lowers the
coverage figure the gate prints.

**What it gives up:** 60 MB of buffers at the extreme, an index rebuild on every slider
step, and per-vertex width only by expanding quads on the CPU (no instancing in
`QQuick3DGeometry`). It also puts the toolpath inside a lit scene, so materials must be
unlit and depth behaviour stated.

## Option 3 — keep libvgcode alive past `upstream/`

Vendor `libvgcode` (and its `libpgcode` reader) on its own, with GLEW and the private
desktop-GL context the item already builds.

**What it costs:** a vendored GL renderer in the repository forever, its GLEW dependency,
a second GL context per viewer, and a platform split that has to be explained to everyone
who reads the build. The measured numbers say it is fast — 543k moves loaded and drawn in
about 0.6 s — but it is also the only part of the app that **cannot run on the platform
the gate uses**: under `QT_QPA_PLATFORM=eglfs` it prints `GcodeView: offscreen surface
invalid on eglfs` and never initializes, so `--smoke --preview` fails there today while
the rest of the frame renders fine (90.10 % covered).

**Does it contradict "no C++ engine"?** Yes, in the way that matters: the port exists to
remove vendored C++ engine code from the application's backend, and this option keeps a
slice of it — with its own renderer, its own GL loading and its own platform assumptions
— purely to draw. It also means the toolpath is the one thing that cannot be tested
where everything else is.

**Riskiest unknown:** none, which is the problem. Its cost is known and permanent.

**Gate that catches a regression:** only where GL works — `--smoke` on a platform with
desktop GL. On eglfs, which is what the gate runs, it cannot gate anything.

## Option 4 — Qt Quick RHI item: our own pipeline on the scene graph's device

`QQuickRhiItem` (public Qt 6.7+, present in the installed 6.12) renders through `QRhi`
using the **same device the scene graph already has**, so there is no second context, no
GLEW, and it works on whichever backend the app runs (GL, GLES, Vulkan, D3D, Metal). The
toolpath becomes an instanced draw: one instance per move carrying its two endpoints, its
width and its role/value, so the vertex shader expands the quad.

**Why it is interesting:** the buffer is sized by *moves*, not vertices — 500k × 32 B =
16 MB, one draw call — and the layer range is a draw-range change (`instanceOffset` /
`instanceCount`) rather than a rebuild, which is exactly the slider's interaction. Width
is expanded on the GPU, so nothing is duplicated on the CPU.

**Work:** a `QQuickRhiItem` subclass and a `QQuickRhiItemRenderer` (~300 lines), one
pipeline with a vertex and fragment shader (Qt bakes `QShader` from SPIR-V at build time),
two buffers, the `QRhi` resource lifecycle, and the same camera feed the item takes today.
Roughly 700–1,000 lines, and the first shaders in this application.

**Riskiest unknown:** the API and the backend matrix, on this machine in particular —
whether `QQuickRhiItem` behaves under `eglfs`/GLES (the gate's platform) and under
`QT_QUICK_BACKEND=software`, and how much of `QRhi`'s resource lifecycle has to be
re-learned when a frame is dropped. The measurement: an item that draws 500k instanced
quads in the smoke scene, timed under both platforms.

**Gate that catches a regression:** `--smoke --preview` again, and it would be
platform-independent — the same frame on eglfs, offscreen and any future backend.

**What it gives up:** the fewest, at the price of the newest API and shaders. It does not
share the plate scene's materials, so the toolpath and the models compositing correctly is
our own depth work rather than the scene graph's.

## The recommendation

**Option 2, Qt Quick 3D geometry, with quads for per-bead width.** Reasons, in order:

1. It is the only option that removes libvgcode, GLEW and the second GL context *and*
   reuses a pattern this application already ships (`MeshGeometry` plus the plate scene),
   so the port replaces code rather than adding a new rendering discipline. A port that
   keeps the app's idioms is a port that can be reviewed.
2. The measured data fits with room to spare: a typical job is 400k vertices (9.6 MB) and
   the worst case is 2M (48 MB + 12 MB indices), one upload per load, and the GPU
   transforms that in single-digit milliseconds. The layer slider's rebuild is 12 MB at
   the extreme and 0.6 MB normally; the mapping it needs (`layer_first_move`,
   `layer_first_vertex`) is data this engine can produce, and the contract QML sees does
   not change.
3. The gates get stronger rather than weaker: the toolpath lives in the frame the smoke
   already captures, `--slider-test` tests the same range logic, and the failure mode the
   gate would catch is precisely "drew nothing", which is the mode that is silently
   broken today on eglfs.

**Option 4 is the better shape if option 2 runs into its limits** — either the material
pipeline cannot express the four ramps, or 60 MB of buffers is judged too much, or the
index rebuild on a slider drag is visible. Its instanced draw is the right answer to all
three, at the cost of writing the app's first shader.

**Option 1 is rejected for the interactive path and kept for testing.** 293 ms per full
frame is seven times the app's own interactive budget, and the honest way to fit it is a
layer window, which the slider already gives — but that is a picture of one layer, not of
the toolpath. What option 1 *is* good for: the same projection code can render a
reference frame with no GL at all, so a test can compare it byte for byte. That is worth
having whichever renderer ships.

**Option 3 is rejected.** A vendored GL renderer kept only for drawing contradicts the
reason the backend is being ported, and it cannot be gated on the platform the gate runs.

## What is not measured, and what would settle each

| unknown | the measurement |
| --- | --- |
| how many moves a *real* application job has | slice the app's own preset and model with `hfw-cli` and `grep -c '^G1 '`; the app's selftest prints its own slice time, not its moves |
| whether 2M quads composite correctly with the plate models | one `CustomMaterial`, one 100k-move geometry, `--smoke --shot` against the current renderer's frame on the same camera |
| `QQuickRhiItem` under eglfs/GLES and under the software backend | an item drawing 500k instanced quads, taken through `--smoke` on both platforms |
| the frame time of the current renderer alone | not separable from the app's startup; it would need the item to print its own frame time, which it does not |
| GPU upload cost of a 48 MB vertex buffer on this machine | time `setVertexData` plus the first frame for a 2M-vertex geometry |
| the legend's totals against the file | see below: no gate checks them today |

## The gate that has to be added, whichever option wins

`--smoke` proves a frame was drawn; `--slider-test` proves the slider maps to layers.
**Nothing compares the legend's totals to the G-code's.** `PreviewData` accumulates
`totalFilamentMm` per role, and the QML shows it as "Total filament"
(`PreviewLegend.qml:404`) — so a renderer that drops a role, or double-counts one, changes
a number a user checks against their slicer and no gate notices. This engine's writer
already emits the file's own total and the verifier checks it independently, so the gate
is cheap: sum the legend's roles and compare against the file's total line, with the same
tolerance the verifier uses. That gate is worth adding before the renderer changes, so it
can catch the change.
