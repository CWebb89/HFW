# Testing HFW by hand

Written for someone who does not write Rust. Every command is copy-paste. Every block shows
**what you should see** and **what it means** before you type it.

---

## 0. The three words you need

- **fixture** — a test model that ships with HFW. `fixtures/torus.stl` is a donut, `fixtures/cube.stl`
  is a 20 mm cube. They never change, so a number measured on them is comparable to a number in
  this document.
- **profile** — a text file of printer settings. `profiles/voron_klipper.cfg` is your Voron.
- **gate** — the automatic test script. It runs everything and prints PASS or a failure. It is the
  only thing allowed to call the engine "working".

## 1. Get into the project once

Every command below assumes you are in the project directory. Fish:

```fish
cd ~/dev/HFW/HFW-final
```

If `cd` fails, the path is wrong — stop there, nothing else will work.

## 2. What "the program" is

There is no window. The program is one executable file, built from source, sitting at:

```
~/dev/HFW/HFW-final/target/debug/hfw-cli
```

That is the slicer. You run it like any terminal command: give it a model, a profile, and tell it
where to write the output. If that file does not exist, build it once:

```fish
cargo build
```

That takes a minute or two and prints a lot of text ending in `Finished`. It does not need internet.

## 3. The keyword in every command: `--gcode`

The slicer writes a pile of output (per-layer images, a report, a manifest) into a **directory** you
choose with `--out-dir`, and the actual printable file into a **path** you choose with `--gcode`.
Point both at `/tmp` while testing, so you never overwrite anything real:

- `--out-dir /tmp/t1` — the scratch folder
- `--gcode /tmp/t1.gcode` — the file you would send to the printer

Nothing is sent anywhere. No printer is contacted. It writes files and exits.

---

# Part A — the thing we just built: travel that avoids printed walls

## A1. What the feature is, in printer terms

After the slicer prints a loop of a wall, the nozzle has to get to the start of the next thing it
prints. It moves there **without extruding**. That move is called travel.

If the straight line between those two points passes over a wall that is *already printed*, the
nozzle physically drags across hot plastic. Bad outcomes: the part gets knocked, the wall gets a
scar, blobs get dragged into a later layer.

The setting `avoid_crossing_perimeters` does two things. First it picks a better **starting point** for
 each loop, so the travel leaves the loop on the free side. Then, if the travel would still cut a
wall, it **routes the move around the wall** instead of through it: the nozzle goes round the printed
loop, a bead of clearance away from it, as one retracted move. HFW then counts how many travels
still cross a printed wall and prints that count. **Lower is better.**

## A2. Measure it with the feature off

```fish
./target/debug/hfw-cli fixtures/torus.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --out-dir /tmp/t1 --gcode /tmp/t1.gcode | grep 'cross a printed'
```

The `| grep 'cross a printed'` at the end means: ignore all the other output, show me only the line
containing those words. Without it you get twenty lines and the one that matters scrolls past.

Expected:

```
  93 travels cross a printed perimeter
```

**93** is your baseline. It is a number measured from the G-code that was actually written, by a
separate checker that re-reads the file — not a number the planner made up about itself.

## A3. Turn it on and measure again

Make a copy of the profile and add two lines to the copy. Never edit the original; the copy goes in
`/tmp`:

```fish
cat profiles/voron_klipper.cfg > /tmp/avoid.cfg
printf 'avoid_crossing_perimeters = true\navoid_crossing_perimeters_max_detour = 3\n' >> /tmp/avoid.cfg
```

Two lines in that second command, and they mean different things:

- `avoid_crossing_perimeters = true` — turn the feature on.
- `avoid_crossing_perimeters_max_detour = 3` — allow the nozzle to travel up to 3 mm further than the
  straight line to get round a wall. **Zero means no limit**, which is what the check below uses.

Now the same slice with the copy, first with that 3 mm cap, then with no cap at all:

```fish
./target/debug/hfw-cli fixtures/torus.stl --profile /tmp/avoid.cfg --layer-height 0.2 --out-dir /tmp/t2 --gcode /tmp/t2.gcode | grep 'cross a printed'
cat profiles/voron_klipper.cfg > /tmp/free.cfg
printf 'avoid_crossing_perimeters = true\n' >> /tmp/free.cfg
./target/debug/hfw-cli fixtures/torus.stl --profile /tmp/free.cfg --layer-height 0.2 --out-dir /tmp/t2b --gcode /tmp/t2b.gcode | grep 'cross a printed'
```

Expected:

```
  71 travels cross a printed perimeter
  48 travels cross a printed perimeter
```

93 → 71 with a 3 mm budget, and 93 → 48 with no budget at all: 45 of the drags are gone. The
remaining 48 are travels whose start or end is wedged against several nested walls, where no route
holds a bead of clearance; those fall back to the straight move rather than pretend.

## A4. The test that proves it is not faking it

A cube has no way out. Its walls enclose the interior completely, so every single crossing is a move
**into** that enclosure — no choice of starting point can help. If the feature improves the cube, it
is cheating. So run the cube both ways:

```fish
./target/debug/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --out-dir /tmp/c1 --gcode /tmp/c1.gcode | grep 'cross a printed'
./target/debug/hfw-cli fixtures/cube.stl --profile /tmp/avoid.cfg --layer-height 0.2 --out-dir /tmp/c2 --gcode /tmp/c2.gcode | grep 'cross a printed'
```

Expected, both times:

```
  100 travels cross a printed perimeter
```

**Identical is the pass.** The improvement on the torus plus no change on the cube together mean the
feature is choosing better entry points and routing round walls where that is possible, and refusing
to invent a route where the part encloses the space.

One more check that the routing is real and not just a nicer entry point: count the moves in the file.

```fish
./target/debug/hfw-cli fixtures/torus.stl --profile /tmp/free.cfg --layer-height 0.2 --out-dir /tmp/t2b --gcode /tmp/t2b.gcode >/dev/null
grep -c '^G0 X' /tmp/t1.gcode /tmp/t2b.gcode
```

Expected:

```
/tmp/t1.gcode:2390
/tmp/t2b.gcode:2417
```

A routed move is several `G0` moves instead of one, so the routed file has more of them. Fewer
crossings **and** more moves is a detour; fewer crossings with the same moves would mean nothing
changed but the bookkeeping.

## A5. The test that proves the cap is real

`max_detour` is supposed to limit how far the nozzle is willing to go round. Set it absurdly small and
the improvement must shrink — if it does not, the cap is decoration:

```fish
cat profiles/voron_klipper.cfg > /tmp/tight.cfg
printf 'avoid_crossing_perimeters = true\navoid_crossing_perimeters_max_detour = 0.5\n' >> /tmp/tight.cfg
./target/debug/hfw-cli fixtures/torus.stl --profile /tmp/tight.cfg --layer-height 0.2 --out-dir /tmp/t3 --gcode /tmp/t3.gcode | grep 'cross a printed'
```

Expected:

```
  86 travels cross a printed perimeter
```

Half a millimetre buys almost nothing, three millimetres buys 22, no limit buys 45. **The ladder
86 → 71 → 48 is the cap working** — the smaller the budget, the fewer walls it is worth going round.

## A6. Your own model

Swap the fixture for one of your parts and the profile for your own:

```fish
./target/debug/hfw-cli /path/to/your-part.stl --profile /tmp/avoid.cfg --layer-height 0.2 --out-dir /tmp/mine --gcode /tmp/mine.gcode | grep 'cross a printed'
./target/debug/hfw-cli /path/to/your-part.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --out-dir /tmp/mine2 --gcode /tmp/mine2.gcode | grep 'cross a printed'
```

Rules for reading the result:

- **Second number lower than first** — the feature found room to work on your part.
- **Both numbers identical** — your part is like the cube: the crossings are all entries into
  enclosed interiors, or the loops are too small to give a choice. Not a bug.
- **Second number higher** — a real bug. Save both G-codes and say so; do not print from them.
- **No line at all** — the slice failed. Re-run without the `| grep` part to see the error message.

## A7. Look at what you actually got

The layer images are drawable with any image viewer:

```fish
ls /tmp/t2/
```

You will see a `.svg` per layer plus a report. Open one (Kitty shows SVG, or `python3 -m http.server`
in that folder and click it). Walls are the outlines; travels are the thin lines. The difference
between `/tmp/t1` (feature off) and `/tmp/t2` (on) is where those thin lines run.

## A8. What the automatic tests check (and why they are stronger)

```fish
cargo test -p hfw-gcode the_writer_counts
cargo test -p hfw-gcode the_crossing_count
```

Expected, each: a line with `... ok` and `1 passed`.

- The first proves the writer's own count equals the independent checker's count on the same file.
  Those two disagreed once (175 vs 93) and that disagreement was a genuine bug, so this test exists
  to keep them equal.
- The second is the torus/cube pair from A3 and A4, run automatically, so nobody can merge a change
  that makes the cube "better" by cheating.

`cargo test -p hfw-gcode` without a name runs all 65 tests in that crate instead of just one.

## A9. The gate — the only "is it all working" answer

```fish
fish scripts/test-hfw.fish
```

Expected:

- about 40 blocks, each starting with `==` and printing a `Verified: ...` line
- the crossing block prints:
  `Verified: torus crossings 187 -> 169, cube unchanged at 200 (every cube crossing is an entry into the enclosed interior).`
- ends with `ALL HFW ENGINE CHECKS PASSED.`

It takes about 3.5 minutes. If any block fails it prints `FAIL:` with the reason and stops. **The
fixture numbers in the gate (187, 169, 200) are not the same as the ones in A2–A4 (93, 75, 100)** —
the gate slices at a different layer height and wall count, so it measures a different file. Both
sets are correct; only compare numbers produced the same way.

## A10. What none of this proves

- It does not prove your printer will like the G-code. Only a print does that, and you said you want
  to test prints last.
- It does not prove the part is strong or dimensionally right. That is the two-reference oracle block
  in the gate (it compares volume against two independent references) and, ultimately, calipers.
- It does not exercise the voice idea, the phone, or anything in next-layer.

---

# Part B — several objects on one plate

## B1. Two parts, two files

Give the slicer both paths. Order matters only for plate position:

```fish
cd ~/dev/HFW/HFW-final
./target/debug/hfw-cli fixtures/cube.stl fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --out-dir /tmp/p2 --gcode /tmp/p2.gcode
```

Expected, in the last lines:

```
Plate: 2 objects (cube, cube) printed one layer at a time.
G-code: 100 layers, 2189.1 mm filament (6.53 g, 5265 mm^3), estimated 17.2 min, 15053 mm travel, 442 retractions.
Placed footprint X 147.90..202.10, Y 160.40..189.60, top Z 20.00 on a 350 x 350 bed.
  300 travels cross a printed perimeter
verify RESULT PASS (independent check of the written file)
```

How to read it:

- **`Plate: 2 objects`** — the slicer knows there are two parts. It did not merge them into one anonymous blob.
- **`100 layers`** — not 200. Both parts are printed in the same pass, layer by layer. This is the normal way a plate prints.
- **`Placed footprint X 147.90..202.10`** — 54.2 mm wide. One cube alone is `X 160.40..189.60`, i.e. 29.2 mm: a 20 mm part plus the skirt around it. Two of them 5 mm apart are 20 + 5 + 20 = 45 mm of part, 54.2 with the skirt. **The span is what to check**: it grew by a whole part, so the second cube sits beside the first instead of inside it.
- **`2190 mm of filament` against `1097` for one cube** — twice the plastic, i.e. two parts really are there.
- **`verify RESULT PASS`** — an independent checker re-read the file and agreed.

## B2. Prove the parts print one at a time

A plate printed "one layer at a time" must finish object A's layer before touching object B, otherwise the nozzle flies back and forth across the plate on every layer. There is a script for exactly that check:

```fish
python3 scripts/plate-check.py /tmp/p2.gcode
```

Expected:

```
ok
```

What it measures: inside each layer, how many times the printed X position jumps across the gap between the two parts. Grouped printing gives one jump per layer; ungrouped gives several. To see it fail, feed it a file made before this feature existed, or one where both cubes are a single object:

```fish
python3 scripts/plate-check.py /tmp/some-old-file.gcode
```

which reports something like `layer 1 switches objects 3 times`. A single object made of two bodies has no rule saying which body to finish first, so the slicer zig-zags — and the check catches it.

## B3. Spacing and layout controls

```fish
./target/debug/hfw-cli fixtures/cube.stl fixtures/cube.stl --profile profiles/voron_klipper.cfg \
    --layer-height 0.2 --spacing 20 --out-dir /tmp/p20 --gcode /tmp/p20.gcode
```

`--spacing 20` puts 20 mm between the parts instead of the default 5. The footprint goes from `X 147.90..202.10` (54.2 mm) to `X 140.40..209.60` (69.2 mm) — exactly the extra 15 mm — while the layer count and the filament length stay where they were. A part that no longer fits the bed is refused by the verifier with a bed-overflow error, not silently clipped.

`--no-arrange` keeps each file's own coordinates. Use it when your STLs already carry a plate layout:

```fish
./target/debug/hfw-cli fixtures/cube.stl fixtures/cube.stl --profile profiles/voron_klipper.cfg \
    --layer-height 0.2 --no-arrange --out-dir /tmp/p0 --gcode /tmp/p0.gcode
```

Both cubes sit at the origin then, so what you get is one cube's worth of plastic — 1096.9 mm of filament, the same footprint as a single cube — and a warning:

```
warning: cube and cube share the same space on the plate; they print on top of each other (use --spacing without --no-arrange to lay them out)
```

Do not print that. The warning is the only thing standing between you and two parts printed inside each other: nothing stops overlapping models, because a 3MF legitimately uses overlapping volumes for modifiers.

## B4. A plate from a slicer project

A 3MF from Orca or Prusa already carries its layout, so nothing is rearranged:

```fish
./target/debug/hfw-cli /tmp/two-cubes.3mf --profile profiles/voron_klipper.cfg --layer-height 0.2 --out-dir /tmp/p3 --gcode /tmp/p3.gcode
```

Expected: `Plate: 2 objects (object 1, object 1)`, footprint `X 145.40..204.60` (the file's own 30 mm gap), `verify RESULT PASS`. Each build item in the file is one object; components inside an item stay attached to it.

To make such a file yourself: arrange two parts in Orca, save the project as 3MF, slice it here. The parts keep the positions you gave them.

## B5. Printing one whole part before the next

`print_sequence = by object` finishes part A completely before starting part B. It is the setting that gets the fewest crossings, and the one that can crash the printer if the geometry does not allow it, so it refuses to run without two facts about your machine.

**Why it needs `gantry_height`:** part A is a solid 20 mm tower when the nozzle goes back down to Z 0.2 for part B. The toolhead has to pass over that tower. If nobody states how much clearance the toolhead has, the slicer will not guess, because the guess drives your nozzle through your finished part.

**Where that number lives:** both shipped profiles carry the key, set to `0`, with a comment saying what to measure:

```fish
grep -A6 'gantry_height' profiles/voron_klipper.cfg
```

To fill it in on your own machine: home the printer, bring the nozzle down to the bed, and measure from the nozzle tip up to the lowest part of the moving gantry — the underside of the X extrusion, or a Z belt anchor, whichever is lower. That distance in millimetres goes in the profile, and `print_sequence = by object` starts working for parts shorter than it. `0` keeps it refusing, which is the safe state. For one run you can state it without editing the profile:

```fish
./target/debug/hfw-cli fixtures/cube.stl fixtures/cube.stl --profile /tmp/seq.cfg \
    --layer-height 0.2 --gantry-height 100 --out-dir /tmp/s1 --gcode /tmp/s1.gcode
```

```fish
cat profiles/voron_klipper.cfg > /tmp/seq.cfg
printf 'print_sequence = by object\n' >> /tmp/seq.cfg
./target/debug/hfw-cli fixtures/cube.stl fixtures/cube.stl --profile /tmp/seq.cfg --layer-height 0.2 --out-dir /tmp/s0 --gcode /tmp/s0.gcode
```

Expected, and it is the *correct* answer, not a bug:

```
error: print_sequence = by object needs gantry_height in the profile: the toolhead has to pass over a finished object, and guessing that height would drive it through one
```

Now state a clearance bigger than the part:

```fish
printf 'gantry_height = 100\n' >> /tmp/seq.cfg
./target/debug/hfw-cli fixtures/cube.stl fixtures/cube.stl --profile /tmp/seq.cfg --layer-height 0.2 --out-dir /tmp/s1 --gcode /tmp/s1.gcode
```

Expected:

```
Plate: 2 objects (cube, cube) printed one object at a time.
G-code: 200 layers, 2193.2 mm filament (6.54 g, 5275 mm^3), estimated 28.3 min, 12534 mm travel, 443 retractions.
verify RESULT PASS (independent check of the written file)
```

- **`200 layers`** — 100 per part. The layer count doubles; that is the signature of this mode.
- **`printed one object at a time`** — the report says which mode it used.
- **`verify RESULT PASS`** — including a new check: Z may only come back down to the bed if what is already printed is shorter than `gantry_height`. The verifier rejects the file otherwise.

Prove the order in the file itself:

```fish
python3 scripts/plate-check.py --sequence /tmp/s1.gcode 100
```

Expected `ok`. It asserts: 200 layers, exactly one Z return to the bed, at exactly the boundary between the two parts, and each part climbing steadily.

## B6. Per-object settings

A 3MF project can give one object its own settings. Orca and Prusa store them in `Metadata/Slic3r_PE_model.config` inside the file; HFW reads object-level entries and applies them to that object only.

The fixture does it for you:

```fish
python3 tools/make-plate-3mf.py
./target/debug/hfw-cli fixtures/plate-overrides.3mf --profile profiles/voron_klipper.cfg --layer-height 0.2 --out-dir /tmp/ov --gcode /tmp/ov.gcode
```

Expected:

```
Object dense cube (#2) overrides: sparse_infill_density, wall_loops.
Plate: 2 objects (plain cube, dense cube) printed one layer at a time.
Per object: plain cube 2758 mm^3, dense cube 6115 mm^3.
G-code: 200 layers, 3513.2 mm filament (10.48 g, 8450 mm^3), estimated 28.6 min, 15201 mm travel, 546 retractions.
verify RESULT PASS (independent check of the written file)
```

How to read it:

- **`overrides: sparse_infill_density, wall_loops`** — the file asked for 60% infill and 5 walls on one object, and both were applied. Any key it cannot apply per object is listed separately as needing a feature this engine does not have, so nothing is dropped quietly.
- **`plain cube 2758 mm^3, dense cube 6115 mm^3`** — the same cube twice, and the dense one holds more than twice the plastic. That is the whole point of the setting. Two equal numbers would mean the overrides were ignored.
- **`200 layers`** — with per-object settings the file carries one layer block per object per Z, so both parts still print layer by layer, 100 Z levels each.
- **`verify RESULT PASS`** — the written file still passes the independent checker.

What you can override per object today: infill density, infill pattern (any of the seven names
`--pattern` accepts), wall count, extrusion width, and support on/off. Layer height is refused on
purpose — the plate shares one layer schedule, so a per-object value would either be ignored or
desynchronise the stack.

## B7. What still needs building in this area

- **Modifier volumes and height ranges.** A per-object override applies to the whole object. Region-scoped settings (`<metadata type="volume">`, layer ranges) are counted and reported, never applied.
- **Speeds, temperatures and cooling per object.** Those live in the printer config, and the writer emits one file for one config; applying them per object needs the emitter to carry a per-path config.
- **Per-object `;OBJECT:` labels.** One name covers the file. Nothing needs it to print, but Klipper's `EXCLUDE_OBJECT` does.
- **One-at-a-time clearance checks** are per-pair bounding boxes, not swept geometry: an L-shaped part can be closer than its box says.

---

# Part C — infill patterns

Seven patterns, chosen with `--pattern` on the command line or `sparse_infill_pattern` in a profile:
`rectilinear`, `grid`, `line`, `triangle`, `cubic`, `gyroid`, `3d_honeycomb`.

## C1. Print the same part five ways and compare the plastic

```fish
for p in rectilinear grid line triangle cubic
    ./target/debug/hfw-cli fixtures/sphere.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --pattern $p --out-dir /tmp/p-$p --gcode /tmp/p-$p.gcode | grep 'G-code:'
end
```

Expected, one line per pattern:

```
G-code: 150 layers, 1487.8 mm filament (4.44 g, 3578 mm^3), estimated 18.9 min
G-code: 150 layers, 1494.6 mm filament (4.46 g, 3595 mm^3), estimated 18.9 min
G-code: 150 layers, 1487.8 mm filament (4.44 g, 3578 mm^3), estimated 18.9 min
G-code: 150 layers, 1484.8 mm filament (4.43 g, 3571 mm^3), estimated 18.9 min
G-code: 150 layers, 1487.8 mm filament (4.44 g, 3579 mm^3), estimated 18.9 min
```

Every pattern deposits the same plastic, within 24 mm^3 of each other at 15% density. That is the
point: the pattern decides **where** the plastic goes, not how much of it there is. A pattern that
quietly printed twice as much would be a bug, and the small spread is what rules that out.

## C2. The directions are the difference

One layer of sparse infill is a set of parallel lines. The pattern decides which directions appear:

| Pattern | Directions, one layer | Between layers |
|---|---|---|
| `rectilinear` | 45° | then 135° |
| `grid` | 45° and 135° | the same every layer |
| `line` | 45° | the same every layer |
| `triangle` | 45°, 105°, 165° | shifts 90° on the next layer |
| `cubic` | 0° | then 45°, 90°, 135°, repeating |

`rectilinear` and `line` differ only in whether the direction alternates. Their filament counts match
to the tenth of a millimetre and their files do not, which is exactly what should happen.

## C3. The two curved fills

The gyroid and the 3D honeycomb are not line families: each layer is a set of
curves, and the curves change with the layer height. That is what makes them
stiff in all three directions instead of stacking vertical tubes.

```fish
for p in gyroid 3d_honeycomb
    ./target/debug/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --pattern $p --out-dir /tmp/curve-$p --gcode /tmp/curve-$p.gcode | grep 'G-code:'
end
```

Expected:

```
G-code: 100 layers, 1074.9 mm filament (3.21 g, 2585 mm^3), …
G-code: 100 layers, 1130.1 mm filament (3.37 g, 2718 mm^3), …
```

Now the numbers that say the shape is right rather than just non-empty. One
layer's sparse block, counted:

```fish
for p in rectilinear gyroid 3d_honeycomb
    printf '%-13s ' $p
    awk '/^;LAYER_CHANGE/{l++} l==59' /tmp/curve-$p.gcode 2>/dev/null | awk '/^;TYPE:Sparse infill/{f=1;next} /^;TYPE:/{f=0} f && /^G1 /{c++} END{print c+0, "moves"}'
end
```

A rectilinear layer is six to eleven moves — straight lines need no sampling. A
curved layer is **1246 moves for the gyroid and 1477 for the honeycomb**, because
each strand is walked in steps small enough to follow the curve.

## C4. Prove the pattern follows the height

Two adjacent gyroid layers must differ, or the fill is a set of vertical tubes:

```fish
for l in 59 60
    awk -v want=$l '/^;LAYER_CHANGE/{n++} n==want' /tmp/curve-gyroid.gcode | grep -c '^G1 X'
end
```

Expected: two different counts. The same test on a rectilinear file gives one
count repeated with a different angle, which is exactly what a two-direction
pattern is supposed to do.

## C6. Slice repair: the closing radius and the resolution

Two settings act on the sliced cross-section before anything is drawn.

`slice_closing_radius` (default 0.049 mm) fills a crack narrower than twice the
value and drops a hole smaller than it. `resolution` (default 0) thins the slice,
dropping vertices whose removal moves the outline by less than the tolerance.

Start with the honest test — that a part with no crack pays nothing:

```fish
for r in 0 0.049
    ./target/release/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg \
        --layer-height 0.2 --closing-radius $r --out-dir /tmp/close-$r --gcode /tmp/close-$r.gcode
    grep 'filament used' /tmp/close-$r.gcode
end
```

Expected: **the same line twice**, 1096.9 mm for both. A clean cube has no gap
to close, so the repair must not touch it.

Now the resolution, on the part where it matters:

```fish
for r in 0 0.5
    ./target/release/hfw-cli fixtures/sphere.stl --profile profiles/voron_klipper.cfg \
        --layer-height 0.2 --resolution $r --out-dir /tmp/res-$r --gcode /tmp/res-$r.gcode
    printf '%s moves, %s bytes: ' (grep -c '^G1 X' /tmp/res-$r.gcode) (wc -c </tmp/res-$r.gcode)
    grep 'filament used' /tmp/res-$r.gcode
end
```

Expected:

```
41171 moves, 1382441 bytes: ; filament used = 1487.8 mm (4.44 g, 3578 mm3)
 8033 moves,  386731 bytes: ; filament used = 1457.3 mm (4.35 g, 3505 mm3)
```

Five times fewer machine moves and a file a third of the size, for 2% of the
plastic — that is what a printer cannot render. Anything more than a few percent
means the tolerance is eating real geometry, and the ring guard should have
refused it: a ring that would cross itself or move more area than
`tolerance × perimeter` is kept at full detail instead.

Watch the sloped faces of the sphere, not its silhouette: the equator looks the
same at both tolerances, because DP only removes points the chord already
follows.

## C7. What these settings are not

The closing does not round concave corners by the radius the way PrusaSlicer's
does, and a filled crack leaves two coincident outlines rather than one merged
one — the wall generator works from the region's distance field, so both print
the same walls. A hairline *solid* is never removed: closing only adds material,
which is the property the "crack or thin wall" test exists to protect.

## C5. What these patterns are not

`honeycomb`, `concentric`, `zig-zag`, `cross-hatch`, `lightning` and `monotonic` have no geometry
in this engine. A profile that asks for one keeps the default and the intake reports the name as
unapplied; it is never printed as something else under the requested name. `cubic` here is the flat
projection of an interlocking fill, not Slic3r's cubic, whose strands also weave within a single
layer's thickness.

The two curved fills are ports of the upstream algorithms, so the shapes are the upstream shapes:
the gyroid's cell is `2*pi*width/(density*2.44)` and the honeycomb's module is `2*width/density`,
both straight out of their source files. Measured against PrusaSlicer slicing the same part at 15%
with a 0.45 mm bead: its honeycomb pitch is 2.71 mm and this engine's at 0.4 mm is 2.67 mm, a
ratio of exactly the bead widths.


---

# Part D — size compensation

Two settings, one operation. `elefant_foot_compensation` shrinks the **first
layer** to remove the squashed lip a part gets where it meets the bed.
`xy_size_compensation` grows or shrinks **every layer**, which is how you tune a
press fit or a hole that prints undersized.

Both offset the layer's cross-section as one solid, before walls, infill, skin,
brim and support read it. That matters for the observable consequence: a hole
smaller than twice the correction closes instead of turning inside out, and two
features that grow into each other merge.

## D1. Measure the outline, not the volume

The number that proves this works is where the nozzle actually goes, so read the
coordinates out of the G-code. One baseline first:

```fish
./target/debug/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --out-dir /tmp/d0 --gcode /tmp/d0.gcode
awk '/^G1 X/ {for (i=1;i<=NF;i++) if ($i ~ /^X/) {x = substr($i,2)+0; if (!seen || x < lo) lo = x; if (!seen || x > hi) hi = x; seen = 1}} END {printf "%.3f .. %.3f\n", lo, hi}' /tmp/d0.gcode
```

Expected:

```
160.400 .. 189.600
```

That is the far edges of the walls — the part spans 29.2 mm because the outer
wall's centreline sits half a bead inside the 30 mm face. Every number below is
diffed against these two.

## D2. The elephant foot moves the first layer only

```fish
./target/debug/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --elephant-foot 0.3 --out-dir /tmp/d1 --gcode /tmp/d1.gcode
awk '/^G1 X/ {for (i=1;i<=NF;i++) if ($i ~ /^X/) {x = substr($i,2)+0; if (!seen || x < lo) lo = x; if (!seen || x > hi) hi = x; seen = 1}} END {printf "%.3f .. %.3f\n", lo, hi}' /tmp/d1.gcode
```

Expected:

```
160.700 .. 189.300
```

0.3 mm in on both sides — and because the highest and lowest X in the whole file
come from the first layer, those two numbers are the first layer's outline: no
later layer moves. The printed plastic tells the same story: 2757.7 mm³ before,
2753.0 mm³ after. A 0.3 mm shrink applied to all 100 layers would have removed
roughly 140 mm³, so 4.7 mm³ is the first layer alone.

## D3. XY size moves every layer

```fish
for v in 0.2 -0.2
    ./target/debug/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --xy-size $v --out-dir /tmp/d-$v --gcode /tmp/d-$v.gcode | grep 'Deposited volume'
    awk '/^G1 X/ {for (i=1;i<=NF;i++) if ($i ~ /^X/) {x = substr($i,2)+0; if (!seen || x < lo) lo = x; if (!seen || x > hi) hi = x; seen = 1}} END {printf "%.3f .. %.3f\n", lo, hi}' /tmp/d-$v.gcode
end
```

Expected:

```
160.200 .. 189.800
Deposited volume 2849.14 mm^3 …
160.600 .. 189.400
Deposited volume 2665.54 mm^3 …
```

0.2 mm out per side, 0.2 mm in per side, and the plastic moves with it: +91 mm³
for the growth, -92 mm³ for the shrink, which is the 0.8 mm of extra perimeter per
loop times three loops times 0.4 mm of bead times 100 layers.

## D4. The two compose

A first layer that is both grown by 0.2 and shrunk by 0.3 must land 0.1 mm in:

```fish
./target/debug/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --xy-size 0.2 --elephant-foot 0.3 --out-dir /tmp/d2 --gcode /tmp/d2.gcode
awk '/^G1 X/ {for (i=1;i<=NF;i++) if ($i ~ /^X/) {x = substr($i,2)+0; if (!seen || x < lo) lo = x; if (!seen || x > hi) hi = x; seen = 1}} END {printf "%.3f .. %.3f\n", lo, hi}' /tmp/d2.gcode
```

Expected:

```
160.500 .. 189.500
```

If the two settings each recomputed the outline from the original, this would
land at 160.200 or 160.700 instead. One offset, one answer.

## D5. What the gate checks that you cannot see here

`crates/walls/src/offset.rs` carries the cases a cube cannot show: a 2 mm hole
closes at 1.2 mm of growth, a 0.3 mm rib disappears at 0.4 mm of shrink, two
features 1 mm apart merge into one solid, and a ring eroded by 1 mm keeps its
hole while losing its outer millimetre. The gate section runs the numbers above
and asserts every one of them, including that the elephant foot cannot remove
more than a first layer's worth of plastic.
## D1. The wall phase on one core against all of them

Wall generation is the slow half of a slice, and a layer's walls depend on
nothing but that layer's cross-section, so the layers are handed out to threads
in one contiguous chunk each. The thread count is an environment variable, not a
profile key: it changes how long the job takes and nothing about the file.

```fish
for t in 1 8
    env HFW_THREADS=$t ./target/release/hfw-cli fixtures/sphere.stl \
        --profile profiles/voron_klipper.cfg --layer-height 0.2 \
        --out-dir /tmp/thr-$t --gcode /tmp/thr-$t.gcode >/dev/null
    grep 'Wall generation' /tmp/thr-$t/job_report.txt
end
cmp /tmp/thr-1.gcode /tmp/thr-8.gcode; and echo identical
```

Expected on this machine (16 hardware threads), give or take:

```
Wall generation: 4705 ms of work on 1 threads in 4729 ms, slowest layer N at 47 ms
Wall generation: 5414 ms of work on 8 threads in 1003 ms, slowest layer N at 74 ms
identical
```

Read the two numbers separately. **Work** is the sum of what each layer cost and
**wall clock** is what you waited; the gap between them is the speedup, about 5×
at eight threads and 6× at sixteen, because the slice, the compensation pass, the
infill and the writer are still serial. Work rises a little with the thread count
and the "slowest layer" rises more, because each layer's stopwatch now includes
time the OS spent running somebody else — only the wall clock is comparable
between runs.

`cmp` is the assertion that matters: a parallel phase that changes the G-code has
a race in it, and this is the cheapest way to see one.

## D2. Why infill is still serial

Infill and support share a rolling mask cache — a few layers of packed coverage
bits, reused across neighbours. Making that parallel means either duplicating the
cache per thread (memory, and the masks are the expensive part) or locking it
(the mask lookups dominate the phase). Neither is worth 700 ms yet; it becomes
worth it when a full plate is the thing that feels slow.

## E1. The JSON report

`--json` puts one document on stdout and every human line on stderr, so the file
can go straight into `jq` or a program. It is the same job the prose describes —
there is no second code path computing it.

```fish
./target/release/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg \
    --layer-height 0.2 --out-dir /tmp/j --gcode /tmp/j.gcode --json \
    > /tmp/job.json 2> /tmp/job.err

python3 -c "import json; d=json.load(open('/tmp/job.json')); \
print(d['job']['layers'], d['gcode']['filament_mm'], d['verify']['ok'])"

jq '.layers[0], .settings.printer' /tmp/job.json
```

Expect `100 1096.87... true` for the cube. Three things to check by hand:

1. `d['job']['layers']` equals `len(d['layers'])` equals the `G-code: N layers`
   line on stderr. If those three disagree, a layer is missing from one view.
2. `sum(l['volume'] for l in d['layers'])` equals `d['job']['path_volume']`. That
   is the per-layer totals against the job total, computed by different sums.
3. `d['verify']['ok']` is true and `errors` is empty. A file that fails
   verification still produces a document — with `ok: false` and the reasons —
   and the process still exits non-zero, because a caller needs to read the
   failure, not just observe that something went wrong.

`--json-paths` adds `layers[].geometry`: every path with its feature name,
width, closed flag, overhang share, length and vertices. The cube is 772 KB that
way, the sphere about 5 MB, which is the reason it is a second flag rather than
the default.

## E15. The newer sparse patterns

```fish
for p in rectilinear concentric zigzag crosshatch honeycomb monotonic
    printf '%-12s ' $p
    ./target/release/hfw-cli fixtures/sphere.stl --profile profiles/voron_klipper.cfg \
        --layer-height 0.2 --pattern $p --out-dir /tmp/pat-$p --gcode /tmp/pat-$p.gcode \
        2>&1 | grep -oE '[0-9]+ mm\^3|verify RESULT (PASS|FAIL)' | tr '\n' ' '
    echo
end
```

Expected: every pattern verifies, and the deposited total sits within a tenth of
rectilinear's **3903 mm³** — measured on this build as concentric 3765, zig-zag
3913, cross-hatch 4078, honeycomb 3626, monotonic 3903 for the same density. A
pattern that lands on exactly 3903 has not reached the fill; one that doubles it
is drawing each cell twice.

Then open one layer and look at it:

```fish
for p in concentric zigzag honeycomb crosshatch
    echo $p
    ./target/release/hfw-cli fixtures/cube.stl --profile profiles/voron_klipper.cfg \
        --z 5 --svg /tmp/layer-$p.svg --pattern $p >/dev/null
end
```

Concentric is nested loops following the square; zig-zag is one continuous line
running back and forth; honeycomb is six-sided cells and nothing else; cross-hatch
is two families at right angles to each other, 45 degrees off the walls.

## E14. The two-line infill anchor

```fish
printf 'infill_anchor = 0\n' > /tmp/anch0.cfg
printf 'infill_anchor = 2.4\ninfill_anchor_max = 50\n' > /tmp/anch.cfg
for p in anch0 anch
    ./target/release/hfw-cli fixtures/cube.stl --profile /tmp/$p.cfg \
        --layer-height 0.2 --out-dir /tmp/$p --gcode /tmp/$p.gcode 2>&1 | grep 'G-code:'
end
```

Expected **1192.1 mm and 201 retractions** with the anchor, against **1096.9 mm and
246 retractions** without: 95 mm of bead is laid along the inner wall to join the
lines into one run, and 45 travels disappear with it. Open one layer's sparse fill
and the lines alternate direction instead of each starting at the same side.

The walk must stay inside the wall band, never on the part outline:

```fish
awk '/;LAYER:50$/{p=1} p&&/;TYPE:Sparse infill/{s=1} p&&/;TYPE:/&&!/Sparse/{s=0} p&&s&&/^G1 X/{match($0,/X[-0-9.]+/);print substr($0,RSTART+1,RLENGTH-1)} p&&/;LAYER:51$/{exit}' /tmp/anch.gcode | sort -n | sed -n '1p;$p'
```

Expected **101.213** and **118.788** — the 20 mm cube's inner boundary at the
shipped three-wall band, not 100 and 120. And `infill_anchor_max = 1` over
`infill_anchor = 2.4` leaves the file at the unanchored 1096.9 mm, because no walk
that short can reach the next line.

## E12. A polygon bed

```fish
printf 'bed_shape = 0x0,350x0,350x350,0x350\n' > /tmp/poly.cfg
./target/release/hfw-cli fixtures/cube.stl --profile /tmp/poly.cfg \
    --layer-height 0.2 --out-dir /tmp/poly --gcode /tmp/poly/p.gcode 2>&1 | grep Placed
```

Expected **`Placed footprint X 160.40..189.60, Y 160.40..189.60 ... on a 350 x 350
bed`** — the outline's span reached `bed_x`/`bed_y` and the part centres on it.
Then a concave outline, where the bounding box centre is in the cut-out:

```fish
printf 'bed_shape = 0x0,200x0,200x50,50x50,50x200,0x200\n' > /tmp/ell.cfg
./target/release/hfw-cli fixtures/cube.stl --profile /tmp/ell.cfg \
    --layer-height 0.2 --out-dir /tmp/ell --gcode /tmp/ell/e.gcode 2>&1 | grep Placed
```

Expected **`X 1.40..30.60`** — the part moved into the 200 mm arm. And an outline
that leaves no room for the part at all:

```fish
printf 'bed_shape = 0x0,140x0,140x20,20x20,20x140,0x140\n' > /tmp/u.cfg
./target/release/hfw-cli fixtures/cube.stl --profile /tmp/u.cfg \
    --layer-height 0.2 --out-dir /tmp/u --gcode /tmp/u/u.gcode
```

Expected **`error: part plus skirt is 27.6 x 27.6 mm and does not sit inside the
bed outline 0x0,140x0,140x20,20x20,20x140,0x140 (2 mm safety margin)`**.

## E13. An island-level solid threshold

```fish
printf 'solid_infill_below_area = 300\n' > /tmp/island.cfg
./target/release/hfw-cli fixtures/cube.stl --profile /tmp/island.cfg \
    --layer-height 0.2 --out-dir /tmp/is --gcode /tmp/is/cube.gcode 2>&1 | grep 'G-code:'
./target/release/hfw-cli fixtures/cube.stl fixtures/bridge.stl --profile /tmp/island.cfg \
    --layer-height 0.2 --out-dir /tmp/is --gcode /tmp/is/plate.gcode 2>&1 | grep 'G-code:'
```

The cube is 400 mm² and must come out at its usual **1096.9 mm** whether the
threshold is 300 or 0. On the plate the gain must be the small part's own: with
the profile's default density, **+333.4 mm** against the same plate at 0. A
threshold still measured per layer would see 400 + 241 mm² and change nothing.

## E11. The volumetric ramp, per-feature acceleration and first-layer walls

```fish
printf '%s\n' 'max_volumetric_extrusion_rate_slope_positive = 1' \
    'max_volumetric_extrusion_rate_slope_negative = 1' > /tmp/rate.cfg
./target/release/hfw-cli fixtures/cube.stl --profile /tmp/rate.cfg \
    --layer-height 0.2 --out-dir /tmp/rate --gcode /tmp/rate/tight.gcode 2>&1 | grep 'G-code:'
```

Compare against the same file with the two slopes at 0: **122 moves** change, and
every changed move is slower or faster than it was, never relocated. The limit is
enforced between consecutive bead *paths*, not inside a split move, so a
verification that scans individual G1 lines will still find steps where a single
bead's own width changes.

```fish
printf '%s\n' 'infill_acceleration = 800' 'solid_infill_acceleration = 0' \
    'top_solid_infill_acceleration = 0' 'internal_solid_infill_acceleration = 0' \
    'outer_wall_acceleration = 0' 'perimeter_acceleration = 0' \
    'default_acceleration = 0' 'initial_layer_acceleration = 0' \
    'machine_max_acceleration_x = 0' 'machine_max_acceleration_y = 0' > /tmp/acc.cfg
./target/release/hfw-cli fixtures/cube.stl --profile /tmp/acc.cfg \
    --layer-height 0.2 --out-dir /tmp/acc --gcode /tmp/acc/a.gcode
grep -oE 'SET_VELOCITY_LIMIT ACCEL=[0-9]+' /tmp/acc/a.gcode | sort -u
```

Expected exactly **`SET_VELOCITY_LIMIT ACCEL=800`**: the sparse fill, the solid
skin and the top surface all fell through to `infill_acceleration`. The printer's
own ceiling has to be zeroed for this check, or its header line shows a second
value and hides the answer.

```fish
printf 'first_layer_perimeter_speed = 5\n' > /tmp/flp.cfg
./target/release/hfw-cli fixtures/cube.stl --profile /tmp/flp.cfg \
    --layer-height 0.2 --out-dir /tmp/flp --gcode /tmp/flp/f.gcode
awk '/;LAYER:0/{p=1} p&&/;TYPE:Outer wall|;TYPE:Inner wall/{w=1} p&&/^G1 X.* E/{if(w){match($0,/F[0-9]+/); print substr($0,RSTART+1,RLENGTH-1)}} p&&/;LAYER:1/{exit}' /tmp/flp/f.gcode | sort -u
```

Expected `300` and `1500`: both walls take 5 mm/s (300 mm/min), and 1500 is the
first-layer cap still limiting the one wall bead that has no per-feature key.

## E10. Cooling spreads its deficit

The slowdown rows are one mechanism with three distributions.

```fish
printf '%s\n' 'min_layer_time = 60' 'min_print_speed = 1' 'min_speed_factor = 0.05' \
    > /tmp/cool.cfg
./target/release/hfw-cli fixtures/torus.stl --profile /tmp/cool.cfg \
    --layer-height 0.2 --out-dir /tmp/cool --gcode /tmp/cool/uni.gcode 2>&1 | grep 'G-code:'
printf 'cooling_slowdown_logic = proportional\n' >> /tmp/cool.cfg
./target/release/hfw-cli fixtures/torus.stl --profile /tmp/cool.cfg \
    --layer-height 0.2 --out-dir /tmp/cool --gcode /tmp/cool/prop.gcode 2>&1 | grep 'G-code:'
```

Both spend the same total time; what differs is which beads carry it. In layer 10
of the uniform file one bead keeps **3600 mm/min**, the interior sits on the
**300 mm/min** floor (5 mm/s), and nothing is between; the proportional file has
no bead above **634 mm/min** and floors at **317**, because one factor scaled the
whole layer.

```fish
printf '%s\n' 'min_layer_time = 12' 'min_print_speed = 1' 'min_speed_factor = 0.05' \
    'cooling_slowdown_logic = consistent_surface' > /tmp/band.cfg
printf 'cooling_perimeter_transition_distance = 5\n' >> /tmp/band.cfg
```

On `cube.stl`, layer 50, the 5 mm band leaves **6** beads at 120 mm/s where a
zero band leaves **4**: the band is the set of beads held back to the second pass,
so a zero distance reproduces `uniform` exactly.

Two things to know before changing this code:

- **The floor has two floors.** `min_print_speed` is the dialect's, in mm/s, and
  `min_speed_factor` is this engine's older fraction of a bead's nominal speed.
  Whichever is set is applied; when both are, the higher speed wins, which is why
  `slow_down_min_speed` still behaves as it always did.
- **The fan now reads the stretched time**, not the nominal one, so a layer that
  was slowed for cooling blows as hard as upstream would.

## E9. Seams, brim width, ironing target and the solid shell

```fish
for style in aligned rear random
    printf 'seam_position = %s\n' $style > /tmp/seam.cfg
    ./target/release/hfw-cli fixtures/cube.stl --profile /tmp/seam.cfg \
        --layer-height 0.2 --out-dir /tmp/seam --gcode /tmp/seam/$style.gcode \
        2>&1 | grep -E 'travels cross|retractions'
end
```

Expected with `profiles/voron_klipper.cfg` as the base: aligned **100**
crossings and **246** retractions, rear **45** crossings and **156** retractions
(the seam sits on the rear vertex, so consecutive loops start near each other),
random **100** crossings but **346** retractions (every loop starts somewhere new).

Then, on the same cube: `brim_width = 5` adds **43.2 mm** of filament over no
brim at all (1140.1 against 1096.9, 12 loops of a 0.4 mm bead),
`ensure_vertical_shell_thickness = disabled` lays **938.5 mm** against **1096.9**
enabled — and `partial` reproduces
enabled exactly, which is deliberate, not a bug: upstream's `partial` differs
only in a void region that a coverage grid cannot express.

On `sphere.stl`, `ironing_type = top_surfaces` lays **1516.8 mm** against
**1494.6** for `topmost_only`: the sphere's cap has many visible top skins, and
the cube has one, so the two targets coincide there.

## E8. Scarf placement, the whole loop and smooth seams

The scarf rows are three knobs on one behaviour: where a joint may go, how far
it spreads, and whether a corner qualifies.

```fish
for style in nowhere contours everywhere
    printf '%s\n' 'scarf_seam_length = 8' 'scarf_seam_max_segment_length = 0.5' \
        "scarf_seam_placement = $style" > /tmp/scarf.cfg
    ./target/release/hfw-cli fixtures/torus.stl --profile /tmp/scarf.cfg \
        --layer-height 0.2 --out-dir /tmp/scarf --gcode /tmp/scarf/$style.gcode \
        2>&1 | grep 'G-code:'
end
```

Expected on the torus: **689.0 mm** with `nowhere`, **679.1 mm** on `contours`,
**669.2 mm** with `everywhere`. One loop's worth of graduation is worth about
9.9 mm, so the second number means the outer loops were scarfed and the third
means the hole was too.

```fish
printf '%s\n' 'scarf_seam_length = 8' 'scarf_seam_placement = everywhere' \
    'scarf_seam_entire_loop = true' > /tmp/scarf.cfg
./target/release/hfw-cli fixtures/torus.stl --profile /tmp/scarf.cfg \
    --layer-height 0.2 --out-dir /tmp/scarf --gcode /tmp/scarf/entire.gcode \
    2>&1 | grep 'G-code:'
```

Expected **595.8 mm**: the graduation spans the loop instead of one seam length
either side of it.

Then the corner test, on a part whose loop is a square:

```fish
printf '%s\n' 'scarf_seam_length = 8' 'scarf_seam_placement = everywhere' \
    > /tmp/scarf.cfg
./target/release/hfw-cli fixtures/cube.stl --profile /tmp/scarf.cfg \
    --layer-height 0.2 --out-dir /tmp/scarf --gcode /tmp/scarf/corner.gcode \
    2>&1 | grep 'G-code:'
```

Expected **1096.9 mm**, the same as the cube with no scarf settings at all: the
seam is placed on a corner and `scarf_seam_only_on_smooth` defaults to true.
Append `scarf_seam_only_on_smooth = false` and it prints **1072.2 mm**.

Two things to know before changing this code:

- **`contours` cannot use winding.** `path_from` already reverses closed paths
  whose signed area is negative, so holes and contours both come out
  counter-clockwise by the time the writer sees them. Hole-ness is nesting depth
  among the loops of one wall class (`mark_holes`), which is what `contours`
  reads.
- **The default is off.** `scarf_seam_placement` defaults to `nowhere`, so a
  profile that sets only `scarf_seam_length` no longer scarfs. That is the
  reference dialect's contract, and it is why every test and profile that wants
  a scarf now names the placement.

## E7. Support spacing is millimetres

The two preset keys are spacings, not densities, and the interface's preset
default of 0 means solid. Set them in a profile and count the passes in the JSON
plan.

```fish
printf '%s\n' 'support = true' 'support_material_spacing = 2.5' \
    'support_material_interface_spacing = 1.0' > /tmp/space.cfg
./target/release/hfw-cli fixtures/overhang.stl --profile /tmp/space.cfg \
    --out-dir /tmp/space --json \
    | python3 -c 'import json,sys; d=json.load(sys.stdin); print(sum(l["counts"]["support"] for l in d["layers"]), sum(l["counts"]["support_interface"] for l in d["layers"]))'
```

Expected, on the overhang coupon:

- **684 body passes and 58 interface passes** at 2.5 mm and 1.0 mm. The bead is
  0.45 mm, so the body density is 0.45 / 2.5 = 0.18 and the fill's own line pitch
  comes back out at exactly 2.5 mm.
- **342 and 24** when both are doubled: the pass count halves, which is what a
  spacing does. A density read as a spacing would not halve.
- **4104 and 144** with both at 0: solid, and the interface default in both
  preset dialects.
- a spacing of -0.5 is refused with `must be 0 mm or more, or -1 to keep the
  density in charge`. Minus one is this engine's sentinel for "leave
  `support_density` alone"; `support_density` and `support_interface_density` are
  still accepted and are still fractions.

## E6. Machine limits, filament caps and firmware switches

Everything a profile can say to the machine rather than to the geometry, in one
file.

```fish
printf '%s\n' 'gcode_flavor = marlin' \
    'machine_limits_usage = emit_to_gcode' \
    'machine_max_accel_x = 5000,5000' \
    'machine_max_jerk_x = 8,8' \
    'filament_max_volumetric_speed = 2' \
    'pressure_advance_value = 0.045' \
    'stuck_filament_detection = true' \
    'chamber_minimal_temperature = 40' > /tmp/head.ini

./target/release/hfw-cli fixtures/cube.stl --preset /tmp/head.ini \
    --layer-height 0.2 --out-dir /tmp/head --gcode /tmp/head/part.gcode

grep -E '^M(201|204|205|900|591|191)' /tmp/head/part.gcode | head -8
grep -E '^G1 .* F[0-9]+' /tmp/head/part.gcode | sed -E 's/.* F([0-9]+)/\1/' | sort -n | tail -1
```

Expected:

- `M201 X5000` on its own line before the first move — the envelope is written
  only because the profile asked for `emit_to_gcode`. Change it to
  `time_estimate_only` and the line disappears while the speeds stay clamped;
  `ignore` drops both.
- `M900 K0.045` for Marlin (`SET_PRESSURE_ADVANCE ADVANCE=0.045` on Klipper).
- `M591 D0 S1` and `M191 S40` in the header.
- the largest feed in the file is below 8400 mm/min: a 2 mm³/s filament ceiling
  at 0.4 × 0.2 × 1.0 is 3000 mm/min of filament, and the fact that it shows up in
  the file at all is the point — the ceiling is enforced and then clamped again
  by the verifier.

Two of these are easy to get backwards, and both are tested:

- `retract_before_wipe` is a percentage upstream; here any non-zero value retracts
  and then wipes, and zero wipes first. `fixtures/cube.stl` with `wipe = 1` and
  `retract_before_wipe = 0` produces move-then-`G1 E-` pairs; with the default it
  does not.
- `filament_shrinkage_compensation_xy` is read as the number of percent. `0.8`
  means eight tenths of one percent, not eighty.

## E5. Combined infill

The oldest speed trick there is, and the one that is easiest to get wrong.

```fish
printf '%s\n' 'infill_every_layers = 1' > /tmp/one.ini
printf '%s\n' 'infill_every_layers = 3' > /tmp/three.ini

./target/release/hfw-cli fixtures/sphere.stl --preset /tmp/one.ini \
    --layer-height 0.2 --out-dir /tmp/one --gcode /tmp/one/part.gcode
./target/release/hfw-cli fixtures/sphere.stl --preset /tmp/three.ini \
    --layer-height 0.2 --out-dir /tmp/three --gcode /tmp/three/part.gcode

grep -c ';TYPE:Sparse infill' /tmp/one/part.gcode /tmp/three/part.gcode
```

Expected:

- the three-layer file prints **fewer** sparse infill paths: 78 against 141 on
  this fixture, because only the top layer of each group carries any.
- the interior volume barely moves: 1801.50 mm³ against 1802.11 (0.03%). If it drops by a third,
  the fill was combined without the flow multiplier: the same plastic has to land
  in one pass, so `E` per move triples. That is what the engine's own verifier
  catches first, because a combined bead above the volumetric limit is exactly
  the mistake you cannot see by eye until the print is short of plastic.
- the combined layer's beads are `;WIDTH:0.400` on the same spacing as before —
  the flow changed, not the geometry, since a wider bead would not fit the nozzle.

The ceiling is the nozzle diameter: `infill_combination_max_layer_height` lowers
it, and `automatic_infill_combination = true` combines as far as that ceiling
allows without a layer count.

`extra_perimeters_on_overhangs = true` adds one loop around the unsupported part
of a layer — `fixtures/overhang.stl` grows its perimeter volume from 1194.24 to
1201.34 mm³ with the switch on and not at all on a cube, which is the check that the loop
follows the overhang rather than the outline.

## E4. Skin patterns, a wider first layer and print order

Four settings on one file, each visible without a microscope.

```fish
printf '%s\n' 'top_fill_pattern = concentric' \
    'solid_infill_below_area = 1000' \
    'infill_first = true' \
    'first_layer_extrusion_width = 150%' > /tmp/skin.ini

./target/release/hfw-cli fixtures/cube.stl --preset /tmp/skin.ini \
    --layer-height 0.2 --out-dir /tmp/skin --gcode /tmp/skin/part.gcode

grep -A2 ';TYPE:Outer wall' /tmp/skin/part.gcode | head -4
sed -n '/;LAYER:40$/,/;LAYER:41$/p' /tmp/skin/part.gcode | grep ';TYPE:' | head -3
```

Expected:

- the first layer's wall beads carry `;WIDTH:0.600`, not `;WIDTH:0.400` —
  `first_layer_extrusion_width = 150%` of a 0.4 nozzle. Every layer above is
  unchanged, which is the point: the setting touches layer one and nothing else.
- layer 40, a mid part layer, starts with `;TYPE:Internal solid infill` and only
  then reaches `;TYPE:Outer wall` — that is `infill_first`. Without it the walls
  come first.
- with `solid_infill_below_area = 1000`, a 400 mm² cube is under the threshold,
  so **every** layer prints solid. Drop the line and the same command prints
  sparse fill in the middle.
- `top_fill_pattern = concentric` changes the top face from straight beads to
  closed loops inset one bead at a time. Set it to `monotonic` and the file
  differs byte for byte; set it to `hilbertcurve` and the run is refused by name,
  which is deliberate — a wrong-looking top face is worse than a missing option.

`scripts/check-skin.py` is the scripted version of the first two checks; the gate
runs it against the file the preset produced.

## E3. The header, the substitutions and the markers

Three things you can check by eye, all on one file.

```fish
printf '%s\n' 'printer_model = Voron 2.4' \
    'printer_variant = 350' \
    'filament_type = ASA' \
    'filament_vendor = Polymaker' \
    'filament_cost = 32.5' \
    'filament_spool_weight = 180' \
    'notes = bracket v3' \
    'output_filename_format = {input_filename}-doc.gcode' \
    'pause_print_gcode = PAUSE ; layer {layer_z}' \
    'color_change_gcode = M600' \
    'gcode_substitutions = "M104 S0","M104 S0 ; park",,"footer"' \
    'infill_overlap = 20%' \
    'top_solid_min_thickness = 1.2' > /tmp/identity.ini

./target/release/hfw-cli fixtures/cube.stl --preset /tmp/identity.ini \
    --layer-height 0.2 --out-dir /tmp/identity --gcode /tmp/identity/asked.gcode \
    --pause-at 1.0 --colour-change-at 2.0

head -16 /tmp/identity/cube-doc.gcode
grep -n 'substituted\|PAUSE\|M600' /tmp/identity/cube-doc.gcode
```

Expected:

- the file is named `cube-doc.gcode`, not `asked.gcode` — `output_filename_format`
  renamed it.
- the header carries `; printer_model = Voron 2.4`, `; filament_type = ASA`,
  `; notes = bracket v3`.
- line 1 is `; substituted M104 S0 -> M104 S0 ; park (footer)` — a substitution
  announces itself so a file you did not slice cannot be mistaken for one you did.
- exactly one `PAUSE ; layer 1.000` and exactly one `M600`, each under the `;Z:`
  marker of its own layer.
- the report ends with a `Cost 0.13 at 32.50/kg ... 180 g spool leaves 175.9 g`
  line.

The pause is the useful one for a single-extruder machine: it is how a colour
change happens without a toolchanger. Stop the machine, swap the filament, resume.
`--pause-at` takes a comma list (`--pause-at 3,6,9`) and a height that no layer
contains writes nothing at all — that is why the gate counts the lines.

Two things to know about the substitutions: a pattern that does not compile fails
the run rather than passing silently, and the four entries are comma-separated, so
a value containing a comma has to be quoted.

## E2. The machine envelope and the Z offset

`z_offset` is added to every Z coordinate in the file; it does not move the sliced
layers. The machine limits go into the preamble as firmware commands, and the
flavour decides which ones: Klipper has no `M204`, and its acceleration limit is
the whole `SET_VELOCITY_LIMIT` line rather than a printing ceiling.

```fish
printf '%s\n' 'gcode_flavor = marlin' \
    'machine_max_acceleration_x = 5000,5000' \
    'machine_max_acceleration_extruding = 8000,8000' \
    'machine_max_acceleration_retracting = 3000,3000' \
    'machine_max_acceleration_travel = 6000,6000' \
    'machine_max_junction_deviation = 0.01' \
    'z_offset = -0.1' > /tmp/limits.ini

./target/release/hfw-cli fixtures/cube.stl --preset /tmp/limits.ini \
    --layer-height 0.2 --out-dir /tmp/limits --gcode /tmp/limits.gcode

grep -E '^M20[145]' /tmp/limits.gcode
grep -m3 '^;Z:' /tmp/limits.gcode
```

Expected:

```
M201 X5000
M204 P8000 R3000 T6000
M205 J0.010
;Z:0.100
;Z:0.300
;Z:0.500
```

Three things to check by hand:

1. `5000,5000` reached `M201 X5000`. The dialect writes a per-axis limit as a
   list on a multi-extruder machine and the engine reads only the first entry;
   the value is the machine's, not an average.
2. The first `;Z:` is `0.100`, not `0.200`: a `-0.1` offset moved it. Every later
   layer is moved by the same amount, and no X/Y move changed.
3. `z_offset = -0.3` with a 0.2 mm first layer is *refused* by the verifier:
   `line 28: Z -0.100 is outside 0..250`. That is the useful failure, because a
   machine asked to go below Z0 faults instead of printing.

To see the angle, slice the same file twice with `fill_angle = 20` and
`fill_angle = 45`: the file differs on every infill line and nowhere else. The
angle is what you change to stop an infill direction from matching the part's
weak axis, so it is the cheapest strength knob in this file.

```fish
printf '%s\n' 'fill_angle = 20' > /tmp/angle.ini
./target/release/hfw-cli fixtures/cube.stl --preset /tmp/angle.ini \
    --layer-height 0.2 --out-dir /tmp/angle --gcode /tmp/angle.gcode
diff <(grep '^G1 X' /tmp/limits.gcode) <(grep '^G1 X' /tmp/angle.gcode) | head
```

`--json` also works on a single plane (`--z`, which returns the slice rings too)
and on `--check-gcode`, which returns only the verifier's view of a file.
