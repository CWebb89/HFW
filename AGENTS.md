# HFW

The slicing engine: mesh in, verified G-code out. Rust, 12 crates, ~32k lines,
no third-party maths library. `docs/START-HERE.md` is the orientation; this file
is the rules for working in the repository.

`~/dev/next-layer` is the application that runs it. It calls `hfw-cli` as a
subprocess today and will call `crates/capi` once phase 1 of
`~/dev/next-layer/docs/rust-engine-plan.md` lands. The application is the grader:
nothing in this repository is "done" because it compiles here.

## Layout

- `crates/` — the engine. `mesh` (parse, weld, repair), `geometry` (integer
  nanometre points, exact predicates), `kernel` (float regions, indices, offsets),
  `slicing`, `walls`, `infill`, `supports`, `toolpath`, `gcode` (writer and
  independent verifier), `engine` (the pipeline), `cli` (`hfw-cli`), `skeleton`
  (skeletal beads, not the default path).
- `fixtures/` — the test models. Numbers in the documents are measured on these.
- `profiles/` — `voron_klipper.cfg`, `generic_marlin.cfg`.
- `scripts/` — the gate (`test-hfw.fish`), benchmarks, oracle and defect-mesh
  generators. Run by `fish`; Python runs from `.venv/bin/python`.
- `tools/` — ledger and gap generators. Their output is generated; edit the tool.
- `docs/` — see `docs/README.md` for which document answers what.
- `reference/` — third-party reference material, never a source of code.
- `archive/` — the two prototypes this engine was merged from, plus old agent
  session output. Nothing in it is read by the build or the gate.

## Hard rules

- **The gate is the contract.** `fish scripts/test-hfw.fish` must end
  `ALL HFW ENGINE CHECKS PASSED.` It runs fmt, Clippy with `-D warnings`, the
  tests, the release build and ~82 end-to-end sections against the real binary.
  A change that has not run it is not finished.
- **Determinism is a feature.** The same input must produce byte-identical
  G-code, single-threaded and multi-threaded (`HFW_THREADS`). A change that
  breaks it is a defect, not a tradeoff.
- **Clean-room.** Never write code by reading Slic3r, PrusaSlicer, OrcaSlicer,
  CuraEngine or libArachne source. Behaviour is learned by measuring those tools
  through `scripts/oracle.py`, and every module records what it was built from in
  `docs/PROVENANCE.md`. A new crate or a new algorithm gets a row there in the
  same commit.
- **Generated documents are generated.** `docs/SETTINGS-LEDGER.md`,
  `docs/SETTINGS-GAPS.md`, `docs/SCOPE.md` and
  `crates/config/assets/settings-surface.json` come from `tools/gen-settings-*.py`.
  Edit the tool and re-run it; never hand-edit the output.
- **Fixtures are inputs to recorded numbers.** Changing one invalidates every
  figure quoted against it; re-measure in the same commit or do not change it.
- **A number in a document names the command that produced it.** If it cannot be
  re-measured, it does not belong in the document.
- **Nearly zero dependencies.** Today only `mesh` (`zip`) and `gcode` (`regex`)
  touch third-party crates. A new one is named and justified before the manifest
  is edited, and it never replaces code we can write in ~30 lines.
- **Never delete `~/dev/next-layer/upstream/build`** and never `cargo clean`
  without saying which project — here it costs a minute, there it costs hours.
- **Do not add work to the application from here.** An interface change the
  application needs is a next-layer change; `docs/NEXTLAYER-BRIDGE.md` records how
  the two sides currently talk.

## Verifying a change

```fish
fish scripts/test-hfw.fish                     # everything
cargo test --release --workspace               # tests only
cargo clippy --release --workspace -- -D warnings
HFW_PY=.venv/bin/python fish scripts/wallbench.fish    # wall coverage and speed
.venv/bin/python scripts/oracle.py fixtures/cube.stl   # against Prusa/Orca
```

Report in one of three forms and no other: **ran and passed**, **ran what could
be run** (what ran, what did not, why), or **cannot run here** (the command and
what is missing). Timing claims are the median of at least three runs.

## Where the truth is

- Memory store: `~/.pi/agent/projects-memory/HFW/MEMORY.md` — decisions, measured
  facts, dead ends. Read it at the start of a session, write to it at the moment
  something is learned.
- `docs/STATUS.md` — what works, with numbers.
- `docs/ROADMAP.md` — what is next and why that order.
- `docs/DECISIONS.md` — why it is built this way, and how each choice reverses.
- `docs/ENGINE-PARITY.md` — the capability checklist and how each item is measured.
- `docs/SETTINGS-LEDGER.md` — the settings inventory, generated.

## Conventions

- Documentation comments on public items (the workspace warns on `missing_docs`);
  no inline comments — if a line needs one, rename or restructure it.
- No stubs, no TODOs, no placeholder implementations. Finish it or say why not.
- Scripts a user runs are Fish. The agent's own shell is POSIX sh; do not paste
  Fish into it.
- Tests live beside what they test; a fix that changes an output adds the case
  that would have caught it.
