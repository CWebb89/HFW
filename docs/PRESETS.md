# Preset intake

How a PrusaSlicer or OrcaSlicer preset gets into this engine, what happens to
every key, and why nothing is ever refused.

## The problem this solves

Two slicers and this engine share one configuration dialect, and every one of
them spells part of it differently:

| OrcaSlicer writes | the engine calls it | this engine calls it |
|---|---|---|
| `wall_loops` | `perimeters` | `walls` |
| `sparse_infill_density` | `fill_density` | `infill_density` |
| `nozzle_temperature` | `temperature` | `nozzle_temp` |
| `support_type` | `support_material_style` | `support_style` |

### One key the dialects do not have

`layer_height_profile` is not a config key in either engine: they store a
per-object profile in the 3MF. HFW has no 3MF writer yet, so the profile lives in
the profile file, written as `z:height` steps:

    layer_height_profile = 0:0.2, 8:0.12, 16:0.28
    min_layer_height = 0.07
    max_layer_height = 0.28

The first layer keeps `first_layer_height` whatever the profile says, every later
layer is clamped into `min_layer_height..=max_layer_height`, and a final layer
thinner than the floor folds into the one below it.

Before this existed, `hfw --profile user-preset.ini` failed with `unknown
setting 'wall_loops'` — technically correct and useless: the user's preset is
not a typo, it is the same setting under the name the rest of the world uses.

## What happens now

```sh
hfw model.stl --preset user-preset.ini --gcode out.gcode
```

`--preset` prints a report and then slices with whatever it could apply:

```
Preset fixtures/orca-export.ini -> '0.20mm Voron Quality'
  25 keys: 18 applied, 0 partial, 5 recognised, 1 unconverted, 1 unknown (72% applied)
    arc_fitting: recognised: part of the dialect, not implemented yet
    support_type: unconverted: value spelling this engine rejects
    input_shaping_freq_x: unknown: not a dialect name
```

Five intake classes, and the class is the whole point:
| Class | Meaning |
|---|---|
| `applied` | the name was understood and the value reached the configuration |
| `partial` | the name maps to a setting whose *meaning* here differs, so the value was not taken |
| `recognised` | a real dialect name (engine or Orca) this engine does not implement yet |
| `unconverted` | the name is understood, the *value spelling* is not (an enum word, a unit) |
| `unknown` | not a dialect name at all — a typo, or a fork's private key |

The split between `recognised` and `unknown` is the useful one: it separates
"work to do" from "not our dialect", which is what `docs/SETTINGS-LEDGER.md`
counts.

`--preset-strict` turns anything but `applied` into a failure, for a pipeline
that wants a guarantee rather than a report. It is off by default deliberately:
an engine that rejects a user's preset teaches them nothing.

## Inheritance

`inherits = Parent` is resolved from the same directory (`Parent.ini` or
`Parent.cfg`), parents first, child values winning key by key. A cycle is an
error naming the chain.

PrusaSlicer's *vendor bundles* (`cache/vendor/*.ini`, sections `[print:…]`) are
several presets in one file. Those are refused with a message saying so; export
the single preset instead. That is a bounded input contract, not a gap: a bundle
has no single answer for "which layer height".

## Why this is not "the engine now does 366 settings"

Accepting a name and implementing its behaviour are different things, and this
page is only about the first. `docs/SETTINGS-LEDGER.md` is the second: it lists
the engine's settings, this crate's coverage of each, and the cost tier of the
gap. The intake report and the ledger count the same thing from opposite ends,
which is why they agree.

## The same intake, from inside a 3MF

A `.3mf` written by a slicer is a project, not just a mesh: the settings travel in
a sibling part inside the same ZIP. `hfw project.3mf` reads
`Metadata/Slic3r_PE.config` (PrusaSlicer and OrcaSlicer write exactly the preset
format above) or `Metadata/project_settings.config` (Bambu Studio and newer Orca
write a flat JSON object of the same names) and pushes it through the identical
rename-and-classify path, so the report you see is the same one `--preset`
prints:

```
Project fixtures/cube-project.3mf -> 'Fixture Project Quality' (from Metadata/Slic3r_PE.config)
  5 keys: 4 applied, 0 partial, 1 recognised, 0 unconverted, 0 unknown (80% applied)
  1 object(s) carry per-object overrides; those are not applied
```

A `--preset` or `--profile` given on the command line still wins, because an
argument typed now is more specific than a setting saved months ago. Per-object
overrides (`Metadata/Slic3r_PE_model.config`) are counted and reported instead of
being applied — the count is the promise that they were not silently dropped.

## Regenerating the tables

Both files are generated; neither is edited by hand.

```sh
python3 tools/gen-settings-ledger.py     # docs/SETTINGS-LEDGER.md
python3 tools/gen-dialect-table.py       # crates/config/src/dialect_table.rs
python3 tools/gen-alias-table.py         # crates/config/src/alias_table.rs
python3 tools/gen-settings-gaps.py       # docs/SETTINGS-GAPS.md
```

All four take `--check`, which compares against the file on disk and exits
non-zero when it is stale; `scripts/test-hfw.fish` runs that check so a generated
table can never drift from its source.

`gen-dialect-table.py` reads next-layer's `ForeignSettings.cpp` (the rename
table its own oracle gates), the engine's definition table, and this crate's
parser. Change `crates/config/src/parse.rs` and the generated table follows;
change neither and the table cannot drift.
