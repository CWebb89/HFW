# The print ecosystem, explained

Written for the person building it, not for a compiler. Read this when the
numbers stop meaning anything.

Last measured: **2026-11-16**.

---

## 1. The pipeline: from a mesh to a printer

Every slicer on earth, yours included, does the same four things.

```
model.3mf / .stl
      │
      │  ① SLICING      cut the shape with a horizontal plane every 0.2 mm
      ▼                 → each layer becomes a set of closed outlines (contours)
   outlines
      │
      │  ② PLANNING     decide what to draw inside each outline
      ▼                 → walls: loops stepping inward from the edge
                        → skin: solid fill where a surface is exposed
                        → sparse infill: the cheap lattice inside
                        → supports: columns under anything overhanging air
                        → every path carries a width in millimetres
   paths
      │
      │  ③ G-CODE       turn each path into machine moves
      ▼                 → G1 X.. Y.. E.. F..   go here, push this much, this fast
   out.gcode → Klipper → Voron
```

Four numbers make the whole thing work.

| Number | Unit | What it controls |
|---|---|---|
| **Width** | mm | how wide the squashed bead is; 0.4 for a 0.4 mm nozzle |
| **Height** | mm | layer thickness |
| **Extrusion (E)** | mm of filament | `width × height × length` gives a volume; volume ÷ filament cross-section (π × 1.75²/4) gives the filament to push |
| **Speed (F)** | mm/min in the file | the config says mm/s; Klipper multiplies by 60 |

That extrusion formula is the whole reason an independent verifier is possible:
re-read the file, recompute E from the geometry, and any mistake shows up as a
number that does not match. It is also why flow is expressed as a scale — flow
1.0 means "the formula is right and the hotend agrees".

---

## 2. The four projects, and what each one is for

| # | Project | In plain words | Where it stands |
|---|---|---|---|
| 1 | **next-layer** | the slicing **app** — the window, the settings, the slice button | two real prints recorded; needs one release artifact |
| 2 | **layer** | printer **control panel** | released |
| 3 | **HFW** | the slicing **engine** — geometry in, G-code out | 177 of 356 applicable engine settings reproduced |
| 4 | **LayerMind** | **print health** — watches a job and explains a failure | frozen on purpose: records and explains, no new features |

The order is next-layer → layer → HFW → LayerMind, and there is one reason for
it: **next-layer is the grader.** A new engine cannot be called correct without
something to compare it against, and the app with two recorded prints is that
something. HFW can be *started* any time; it cannot be *finished* before there is
a product to check it against.

---

## 3. The number that confuses everybody: "only 48%?"

There are two slicing engines in play, and the percentage belongs to only one.

- **Today, next-layer slices with libslic3r** — the actual PrusaSlicer/OrcaSlicer
  engine, compiled in. It supports all 352 settings the app shows.
  **The app is not missing anything.** Slice a benchy tonight and it uses the
  same engine Prusa and Orca use.
- **HFW is the replacement** — the Rust engine being written so the ecosystem
  does not spend the next decade standing on someone else's C++.
  Its number is: *how many of those 352 settings can my engine reproduce?*
  Today: **177 of 356 applicable**.

So the honest sentence is:

> next-layer offers 352 settings today through libslic3r. HFW reproduces 177 of
> them. The swap happens when the numbers meet — and next-layer's own gate hides
> every control HFW cannot honour (352 → 177 with HFW behind it), so the app can
> never show a knob that does nothing.

### Why not add all 352 at once

Because "add a setting" means one of three very different jobs:

1. **Threading** — minutes. The value already exists in the pipeline and needs a
   name, a default, a validation range, and a line in the writer. `skirt_height`
   was this.
2. **A new code path** — hours to days. Nothing computes it yet: `ironing` needed
   a second pass over the visible top surface, `gap_fill` needs leftover-space
   geometry, `scarf_joint_seam` needs a different seam shape.
3. **An algorithm** — days. `arc_fitting` needed curve detection in the writer
   *and* arc reading in the verifier; `variable_layer_height` breaks the
   assumption that every layer is the same thickness, which reaches into
   slicing, planning and every per-layer structure.

The tier-2 list is mostly class 3. Nothing forbids them; they are simply real
work, and claiming them early would be a lie in the direction of the person
reading the ledger. Arc fitting and ironing stopped being class 3 on 2026-11-16,
and the per-feature speeds and accelerations followed the same day.

### Why a percentage is the wrong score

The unapplied settings are not one kind of thing. `docs/SETTINGS-GAPS.md` sorts
every one of them into a bucket, and the buckets do not resemble each other:

| Bucket | Settings | What closing it needs |
|---|---|---|
| Multi-material, toolchange and MMU | 67 | a second extruder |
| Algorithms still to write | 47 | days each, in the slicer core |
| Printer and filament identity | 23 | nothing: no G-code depends on them |
| G-code emission options | 5 | an event the planner does not produce, or a regex engine |
| Per-feature speeds and accelerations | 6 | travel acceleration, raft values, over-bridge |
| Cooling and fan shaping | 5 | internal-bridge fan, cooling-logic variants |
| Support details | 0 | closed |
| Adhesion and object handling | 3 | first-layer single-perimeter modes, raft expansion |
| Seams and surface finish | 3 | scarf placement choices, whole-loop scarf |
| Z hop and travel shaping | 1 | a lift before an obstacle |
| Defaults only stock presets set | 0 | closed |

Two of the buckets need hardware this engine does not drive, and one changes no
output at all. A percentage that adds those to "one more table entry" tells you
nothing about how finished the engine is; the bucket does. `SETTINGS-GAPS.md` is
the generated version of this table with the full reasoning and every key named,
and it is the one to read when a number here has aged. What is left that a
single-extruder print can actually feel: the last three seam options, a lift
before an obstacle, the two support closing options, the first-layer
single-perimeter modes, and support-aware travel routing. Printing several
objects on one plate, one object at a time within a layer or one whole object at
a time, is done, as is routing a travel around the walls it would have cut, and
compensating a cross-section for the squeeze at the bed or for a press fit.

Both size corrections are one operation: offset the layer's cross-section by a
signed distance before anything reads it, so walls, infill, skin, brim and
support all follow the same outline. The offset is taken on the region as a set
rather than on each ring, because offsetting rings one at a time overlaps
neighbours and turns holes inside out; a hole smaller than twice the correction
closes on its own, and two features that grow into each other merge.

Two of the twelve sparse fills are curves rather than line families: the gyroid
and the 3D honeycomb. They are ports of the upstream algorithms rather than
lookalikes, because a pattern people already trust is worth more than one that
merely looks similar. Both follow the layer height, which is what makes them
equally stiff along all three axes instead of stacking vertical tubes, and both
are clipped to the free area by walking the curve and bisecting the crossings.

Three more are neither straight families nor curves. **Concentric** traces contours
of the free area inward, one bead per spacing, reusing the same chamfer-distance
tracer the solid skin uses. **Honeycomb** (the flat one, not the 3D cell) lays a
hexagonal lattice and draws each hexagon's own sides, clipped to the free area;
the side length comes from the density, because every side is shared by two cells,
and each side is drawn once. **Zig-zag** is a line family whose scanlines are
joined end to end, so the nozzle crosses to the next line instead of lifting; the
connector is tested against the free area first, because two spans of one scanline
can be separated by a hole.

The remaining fill, lightning, is the only one not drawn. It is not a fill of the
layer's area at all: it grows a tree down from the top surface, and getting that
right is a real algorithm rather than a pattern.

Slice repair is the other place where the obvious implementation is the wrong
one. Closing a crack by offsetting the cross-section out by the radius and back
in means sampling a distance field at a hundredth of a millimetre across the
whole part, and every ring then carries the grid's error even where nothing was
wrong. Instead the two boundary pieces that bound the crack are pulled onto
their midpoint, and only they move: a layer without a crack comes out identical,
bit for bit. The pair has to be judged at the middle of each boundary piece
rather than at a corner, because a slit that ends in solid material looks like a
thin wall from its end — which is why a closing that only inspects vertices
converges at the mouth of a slot and stays open at the bottom.

### Where the ledger lives

`HFW/HFW-final/docs/SETTINGS-LEDGER.md` — one row per engine setting, with a cost
tier and a measured status. It is **generated** by
`tools/gen-settings-ledger.py`, which reads the engine's real definition table,
HFW's parser, and Orca's config on this disk. Re-run it after any parser change
and the numbers move by themselves.

`HFW/HFW-final/docs/SETTINGS-GAPS.md` is its companion: the same unapplied rows,
sorted into what closing each one would take. Generated by
`tools/gen-settings-gaps.py`.

Status meanings:

| Status | Meaning |
|---|---|
| `applied` | HFW reads the name and the value changes the G-code |
| `partial` | HFW reads a related name but the meaning differs; the note says how |
| `missing` | HFW has no name for it and would reject the key |
| `n/a` | excluded with a written reason |

The finish line is **100% `applied` or `n/a`**. No row is deleted to move the
percentage.

---

## 4. What each project is still missing

### HFW (the engine)

Missing, beyond raw setting coverage:

- Scarf seams as a first-class feature, gap fill as its own feature, bridge
  anchoring variants
- Region-scoped settings: modifier volumes, height ranges. Per-*object* overrides
  work (infill, walls, width, support); per-object speeds and temperatures do not
- `brim_ears`, and multi-material (an MMU or toolchanger is not implemented, and
  there is no toolchanger on this machine to test one against)
- Acceleration-aware *time estimate* (it emits acceleration; the estimate ignores it)

Already working, which is more than a weekend experiment: arc fitting (the sphere
at 0.2 mm comes out as 8 415 `G2`/`G3` where a chorded slice needs 41 171 moves),
a wall phase that runs on every core with byte-identical output, ironing over the
visible top surface, a top surface with its own speed and acceleration, first-layer
speeds per feature, walls with medial-axis beading, skin classification,
sparse/sparse-grid infill with the gyroid and 3D honeycomb, closing-radius and
resolution slice repair, bridging with span-budgeted supports, tree supports, 3MF
import including project settings, the full PrusaSlicer/Orca dialect intake, a
machine-readable JSON report of the whole job, and an independent verifier that
re-reads the G-code it just wrote.

### next-layer (the app)

Its gaps are **product** gaps, not slicing gaps, because it borrows libslic3r:
release artifact, onboarding, profile management. The engine swap behind a flag
is wired and gated; it is not switched on.

### Where HFW is ahead

- The **verifier**: every written file is re-read and checked for bed fit, feed
  ceilings, heater-before-extrusion, Z monotonicity, volumetric flow, bead width
  plausibility and retraction balance.
- **Span-budgeted bridging**: it consults its own bridging capability before
  deciding a region needs support. Neither reference slicer does this.
- The **ledger**: coverage is measured, never claimed.

---

## 5. How the two engines are compared

Three mechanisms, all automated:

1. **The settings ledger** (HFW) — what the engine understands, measured against
   the engine's own definition table.
2. **The capability list** (next-layer) —
   `oracle/engine_capabilities_check.py --from-ledger` turns the ledger into the
   list of settings the app may show when HFW is the engine behind it. The gate
   asserts the list length equals the ledger's `applied` count, so the two
   numbers cannot drift apart.
3. **The differential slice** (both) — the same model through HFW and through
   libslic3r, compared on layer count, filament length and timing.

---

## 6. Orders of work, when everything looks equally urgent

1. **next-layer**: two prints (already done, needs recording) → one release.
2. **layer**: already released; keep it that way.
3. **HFW**: ledger each pass, biggest tier-2 items first.
4. **LayerMind**: frozen.

Inside HFW, the value ordering is print quality per hour spent. Arc fitting and
ironing are done, which is why they are no longer on the list: next is scarf seams
and gap fill, then variable layer height.

---

## 7. Where the traps are

Things learned the hard way, kept because they will bite again:

- **Generated files must be `rustfmt`-stable.** A generator whose output
  `cargo fmt` rewrites fails its own staleness check forever. The generators now
  format their own output before writing.
- **A generator that iterates a `set` is nondeterministic** across processes.
  The ledger once flipped a setting's type between runs for this reason.
- **Percentage extrusion widths resolve against the final nozzle diameter**, so
  they must be re-resolved after the whole profile is read, not where they appear.
- **The wall ladder is cumulative.** A wider outer bead shifts every loop behind
  it by the full difference; computing offsets as `(i + 0.5) × w` leaves a ridge.
- **Fish has no heredocs**, and `math "1.5 > 1"` is rejected — use `test`.
- **A batch edit that fails on one hunk applies nothing.** Re-read the file.
- **"Top surface" is not "top skin".** Skin is solid for several layers so the
  surface has backing; only the layer the nozzle can reach is visible. Ironing
  has to ask the layer above, not the skin thickness, or it irons five layers.
- **An arc needs its verifier.** Emitting `G2`/`G3` without teaching the file
  checker to measure arc length makes every flow and width check wrong from the
  first curve onward.

---

## 8. Model choice, as of the same date

`~/.pi/agent/models.json` configures `deepseek/deepseek-v4.1-flash` (OpenRouter
routing pinned to DeepSeek). Third-party pricing trackers put V4 Pro around
$0.66/M in and $1.98/M out against Flash's $0.15/M and $0.60/M, with Flash
roughly 2.6× lower median latency and the same 1M context. *Those pages are
third-party routers, not DeepSeek's own price sheet.*

Practical rule: stay on Flash until a task loops twice on the same fix or a
multi-file refactor falls apart. That is the signal that the ~4× price buys
something. Paying it in advance is a guess dressed up as planning.
