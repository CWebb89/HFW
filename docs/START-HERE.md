# Start here

Everything needed to understand this engine, the application it feeds, and how to
work on either with the coding agent. Written 2026-11-19, after the repository
moved from `~/dev/HFW/HFW-final` to `~/dev/HFW`. Nothing here is a guess: where a
number is measured, the command that measures it is named; where it comes from
someone else's document, that document is named.

---

## 1. Two programs, not one

**HFW is the engine.** `~/dev/HFW`. A small Rust program: an STL or 3MF goes in,
printable G-code comes out, and a verifier re-reads the file and checks it. It has
no window. It is 32,071 lines across 12 crates.

**next-layer is the application.** `~/dev/next-layer`. The window, the bed, the
settings panel, the layer slider, the 3D preview. About 26,500 lines of C++ and
QML. When you press *Slice*, it runs `hfw-cli` and loads the G-code that comes
back.

An engine and a car. The dashboard does not move the car; it points at the engine.

Two consequences worth holding onto:

- A **build of HFW takes a minute** (Rust, small). A **first build of the
  application takes hours** (C++, on top of PrusaSlicer's dependency tree). They
  are different projects with different costs; nothing is inconsistent when one
  finishes in 60 seconds and the other does not.
- The application **cannot be better than the engine it runs**, and the engine
  cannot be judged from the application's screens alone.

---

## 2. Where things are

| path | what it is | who writes it | cost to rebuild |
|---|---|---|---|
| `~/dev/HFW` | the engine. This repository | us | ~1–2 minutes (`cargo`) |
| `~/dev/HFW/.venv` | Python used by `scripts/` for measurement and SVG checking | us | minutes, needs network |
| `~/dev/HFW/archive/` | the two prototypes the engine was merged from, session junk | nobody | n/a |
| `~/dev/next-layer` | the application | us | ~1 minute incremental |
| `~/dev/next-layer/src/qt` | the QML interface and its C++ glue | us | included above |
| `~/dev/next-layer/src/app` | a Rust backend crate (cxx-qt). **Untracked by git as of 2026-11-19** | us | ~1 minute |
| `~/dev/next-layer/upstream/` | PrusaSlicer 3.0.0-alpha12, 605,785 lines, patched by `patches/`. A dependency with a demolition date | fetched | hours, tens of GB |
| `~/dev/next-layer/upstream/build/` | where all of that is compiled. **Missing as of 2026-11-19** | build | hours |

**Git and GitHub.** Both projects are git repositories with a GitHub remote. A
commit records a snapshot locally; a push uploads it. As of 2026-11-19:

| repository | branch | commits only on this machine | uncommitted |
|---|---|---|---|
| HFW (`CWebb89/HFW`) | `master` | **9** | none in source |
| next-layer (`CWebb89/NextLayer`) | `main` | **25** | 48 files, plus untracked `src/app/` |

Nothing is lost, but GitHub is behind both projects and the application has work
that has never been snapshotted. Nothing is pushed without an explicit request.

---

## 3. Why the application has `upstream/` in it, and why it will not

Slicing is not the only job a slicer application does. Today:

| job | done by |
|---|---|
| mesh → layers → walls → infill → G-code | **HFW** |
| "what is on the plate": objects, transforms, per-object settings | PrusaSlicer's `libslic3r` |
| the settings/config system and preset loading | `libslic3r` |
| arranging objects on the bed | `libslic3r` |
| preview data structures (`libvgcode`, `libpgcode`) | `libslic3r` |
| reading and writing `.3mf` project files | `libslic3r` |
| cut, hollow, text | `libslic3r` |

`tools/bootstrap` fetched PrusaSlicer once, applied seven small patches from
`patches/`, and the application links it. That is why a fresh build is hours: the
PrusaSlicer dependency tree is most of the work.

**The plan deletes it.** `~/dev/next-layer/docs/rust-engine-plan.md` is a phase
list whose definition of done is:

> `git rm -r upstream/` changes nothing about what the application does.

`~/dev/next-layer/docs/engine-boundary.md` is the scoreboard, generated from the
source: **43 of 303 files** under `src/` still reach into the C++ engine, sorted
by the phase that removes each one. Zero rows left means the C++ engine is gone.
Phases 0–6 build Rust replacements; phase 7 lists what simply will not be ported
(text, cut, hollow, STEP import, SLA, MMU colour mixing, nesting arranger; support and
seam painting have since been built in HFW)
— out by decision, not by neglect.

One more reason the plan exists: while the binary links PrusaSlicer, next-layer is
AGPL-3.0. HFW itself is MIT OR Apache-2.0 and clean-room (`PROVENANCE.md`,
`MIGRATION.md`), which is the licence story that survives.

---

## 4. The words

- **build** — turn source into a runnable program.
- **cargo** — builds Rust. **cmake/ninja** — build C++.
- **dependency** — someone else's code the project needs (Qt, PrusaSlicer's library).
- **`target/`** — HFW's build output. `cargo clean` deletes it; costs a minute.
- **`upstream/build/`** — next-layer's build output, including every compiled
  dependency. Deleting it costs hours.
- **repo, commit, push** — a folder with history; a snapshot; an upload of snapshots.
- **working tree / uncommitted** — edits on disk not yet snapshotted.
- **STL / 3MF** — 3D model files. **G-code** — what the printer executes.
- **profile / preset** — printer, material and process settings in a text file.
  HFW has `profiles/voron_klipper.cfg` and `profiles/generic_marlin.cfg`.
- **gate** — the script that says whether the project is healthy, by running it
  and reading the output, never by assuming.

---

## 5. Running the engine

```fish
cd ~/dev/HFW
cargo build --release -p hfw-cli          # ~10 s incremental, ~1–2 min from clean

# printable G-code for the Voron, verified before it is trusted
./target/release/hfw-cli fixtures/cube.stl \
    --profile profiles/voron_klipper.cfg --gcode /tmp/cube.gcode

# re-check a file independently of the program that wrote it
./target/release/hfw-cli --check-gcode /tmp/cube.gcode --profile profiles/voron_klipper.cfg

# one layer as an SVG where the stroke width is the real bead width
./target/release/hfw-cli fixtures/stress_gear.stl --z 3 --no-infill --svg /tmp/gear.svg

# a plate: several STLs, or a 3MF whose build items carry their own placement
./target/release/hfw-cli a.stl b.stl project.3mf --profile profiles/voron_klipper.cfg --gcode plate.gcode

# every gate: fmt, clippy -D warnings, tests, release build, end-to-end sections,
# and the comparison against PrusaSlicer and OrcaSlicer when they are installed
fish scripts/test-hfw.fish               # ends: ALL HFW ENGINE CHECKS PASSED.
```

`TESTING-BY-HAND.md` is the long version of the above, with what each output means.

---

## 6. Running the application

**Two different programs exist, and only one of them cuts with HFW.**

| what you have | slices with | how to tell |
|---|---|---|
| `dist/NextLayer-0.1.2-x86_64.AppImage` (built 2026-11-10) | `libslic3r` | `strings … \| grep -c hfw-cli` → **0** |
| a build from `src/` after 2026-11-18 | **HFW only** | the same command → more than 0 |

The AppImage still usefully **opens** G-code: `… --preview /tmp/cube.gcode`, or
`--preview-stats /tmp/cube.gcode,/tmp/stats.json`. That is the cheap way to look
at the engine's output without building the application.

### Building the application (the expensive one, once)

```fish
cd ~/dev/next-layer
# do NOT re-run tools/bootstrap: the engine source is already fetched and patched
git -C upstream log --oneline | head        # pinned commit + patch commits, clean tree

# configure: downloads the dependency tree. Network, tens of GB, slow.
setsid nohup systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=2G -- \
    cmake -S upstream -B upstream/build -G Ninja -DCMAKE_BUILD_TYPE=Release -DSLIC3R_GUI=ON \
    > ~/nl-configure.log 2>&1 &

# compile: hours the first time, ~1 minute afterwards
setsid nohup systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=2G -- \
    cmake --build upstream/build --target next-layer-qt -j6 > ~/nl-build.log 2>&1 &
tail -f ~/nl-build.log
```

The memory cap is not optional on this machine: 15 GB and zram-only swap, and an
uncapped build has already frozen this desktop once.

The project's own `AGENTS.md` and the agent memory carry the invocation pattern;
follow it rather than inventing flags.

### Launching, and proving which engine it uses

```fish
~/.local/bin/next-layer                     # wrapper around upstream/build/src/next-layer-qt/next-layer-qt
set -x NEXT_LAYER_HFW ~/dev/HFW/target/release/hfw-cli   # optional: pin the engine
```

The application resolves the engine in this order, and refuses to slice if none
exists (no fallback, by design):

1. `NEXT_LAYER_HFW`, if set;
2. `hfw-cli` in `../libexec/next-layer/` beside the binary;
3. `hfw-cli` on `PATH`;
4. `/home/cwebb/dev/HFW/target/release/hfw-cli` (the development path, now correct
   after the repository move).

First launch asks for a printer (it ships none), then: drop an STL on the bed,
*Slice*, step the layer slider, export.

---

## 7. Where the work stands, and what "better" looks like

The engine: 299 of 364 reference settings applied (82%, 97% outside the
multi-material buckets), gate green, and on the 18-part Revolver plate it takes
15.8 s against PrusaSlicer's 9.4 and OrcaSlicer's 14.4 (16 threads). Its output
agrees with both references within 10% on deposited plastic for cube, torus,
sphere, cone. It has **never been printed**: `PRINT-TESTS.md` is an empty sheet.

The improvements that should be visible: overlapping shells now union into one
outline instead of leaving an L-shaped hole; open outlines are closed; a rib
narrower than one bead deposits its measured width instead of a substituted
0.4 mm; walls are back on the polygon ladder with a gap pass after an experiment
with the raster bead generator. On a clean model the G-code is unchanged — that is
expected, because the fixes are for defects.

---

## 8. The documents

`docs/README.md` is the index: eighteen documents, one job each, and which two are
history. The four worth knowing first: `STATUS.md` (where it stands),
`ROADMAP.md` (what is next), `EXPLAINED.md` (how it works, no jargon),
`TESTING-BY-HAND.md` (what to type).

---

## 9. Working on this with the coding agent

- **Name the project at the start of a session.** They have separate memory
  stores: `~/.pi/agent/projects-memory/HFW/MEMORY.md` and
  `~/…/next-layer/MEMORY.md`. The agent reads the right one only if it knows
  which project the turn is about.
- **The gate is the contract.** `fish scripts/test-hfw.fish` for the engine,
  `tools/check` for the application. A result is one of: ran and passed; ran what
  could be run, with what did not; or cannot run here, with the reason.
- **The application has a purpose-built extension** —
  `~/dev/next-layer/.pi/extensions/next-layer-tools.ts` — providing `nl_build`
  (memory-capped build), `nl_oracle`, `nl_smoke`, `nl_cockpit`, each returning a
  verdict rather than a description. It loads when the working directory is inside
  the repository. The engine has no equivalent yet; `AGENTS.md` here says what to
  run instead.
- **Never delete a build tree on instinct.** `cargo clean` in HFW costs a minute;
  removing `~/dev/next-layer/upstream/build` costs hours. Say which project first.
- **Commit per phase, push only on request.** Both repositories are behind GitHub.
- **Write findings into memory at the moment of discovery** — dead ends first,
  they are the most valuable and the easiest to lose.

## 10. Next session: finish `engine-boundary.md`

The goal is `docs/engine-boundary.md` in next-layer with **no rows left**: no
`Slic3r::` in `src/`, and `git rm -r upstream/` changes nothing. The order is
fixed by dependency, not by preference:

```
phase 0 (landed) ─┬─ phase 1 C ABI ─┬─ phase 2 document model ─┬─ phase 3 settings ── phase 6 project files
                  │                 ├─ phase 4 preview
                  │                 └─ phase 5 plate operations
```

1. **`crates/capi`** — a flat, versioned C surface with handles, not Rust types:
   `hfw_mesh_load_file`, `hfw_mesh_from_bytes`, `hfw_mesh_transform`,
   `hfw_mesh_bounds`, `hfw_mesh_free`, `hfw_plate_slice`, `hfw_gcode_free`,
   `hfw_last_error`. `hfw-cli` keeps working; it is how the engine is tested.
   **Landed 2026-11-20**, version 1, declared in `crates/capi/include/hfw.h` and
   exercised from C by the gate; see `NEXTLAYER-BRIDGE.md` for exactly what the
   version covers and what phases 3–4 still owe it.
2. **`crates/plate`** — the document model the application shows: mesh, transform,
   per-object settings, name, plate. Replaces `PlateDocument.cpp` (3,544 lines)
   and retires the most C++ in one step. Ship mesh + transform + settings first;
   undo, selections and per-volume settings are a second pass.
3. **Settings** — move `SettingsModel` (1,552 lines) and `ConfigBoxJson` onto the
   JSON the engine already emits, so one ledger, one naming table.
4. **Preview** — the application's `GcodeView` already parses G-code itself; give
   it roles from the engine's own output and delete ~5,000 lines of
   `libvgcode`+`libpgcode`.
5. **Plate operations** — `Arranger.cpp`, `Orient.cpp`, `TransformController.cpp`.
   Shelf layout ships; a nesting arranger does not (out of scope).
6. **Project files** — `.3mf` writing moves into HFW (`crates/mesh/src/threemf.rs`
   grew the write side; `crates/plate/src/project.rs` owns the project part) so read
   and write cannot drift. Landed: a saved plate reopens and slices to the same
   bytes.
7. **Parity backlog** — every capability libslic3r has and this engine does not is one
   row with a cost and a state, in `docs/PARITY-BACKLOG.md` and in the surface's
   `capabilities` rows the panel renders as unavailable. Blocked is not declined: MMU
   and tool paint wait on the tool changer, STEP on the dependency decision, SLA is
   ruled out as a different process (DECISION-16). Support and seam paint already left
   the list by being built (`docs/PAINT.md`).

### What must be ported over before `upstream/` can go

These exist today only because the C++ engine carries them. Losing one silently
means the application loses a capability it advertises.

| from | what | where it has to land |
|---|---|---|
| `patches/0001` | the build hook that compiles the application's sources inside the engine tree | gone with the tree; CMake in `src/qt` becomes the only build |
| `patches/0002`, `0004` | bottom-surface and internal-bridge extrusion roles | the engine must emit those roles itself |
| `patches/0003`, `0005` | per-role fan speeds | the engine's cooling stage |
| `patches/0006` | the in-layer extrusion channel the application's own G-code uses | the engine's writer |
| `libslic3r` | plate document, settings, arrange, preview input, `.3mf` read/write, tile/transform | phases 1–6 above |
| `libslic3r` | text, cut, hollow, STEP import, SLA, MMU colour, nesting arranger | **phase 7: not ported, marked unavailable in the interface** |
| reference dialect | 7 unapplied algorithms (`avoid_crossing_perimeters`, `ensure_vertical_shell_thickness`, `fill_pattern`, `max_volumetric_extrusion_rate_slope_negative`/`_positive`, `slicing_mode`, `support_material_style`) and `wipe_tower_acceleration` | the engine, `SETTINGS-GAPS.md` says what each costs |
| reference dialect | 57 multi-material settings | the toolchanger work, not before |

Two rules that keep this from turning into a decade: **a phase deletes the C++ it
replaces in the same commit that lands its replacement**, and **no second path
exists at any time** — that is what phase 0 was for.

### Definition of done, checkable

- `src/qt/CMakeLists.txt` links Qt and HFW, nothing else.
- `docs/engine-boundary.md` has no rows (`tools/engine-boundary.py --check`).
- `git rm -r upstream/` changes nothing: the application still slices, previews,
  saves, reopens and prints.
- `tools/check` green, with the oracle's HFW runner.
