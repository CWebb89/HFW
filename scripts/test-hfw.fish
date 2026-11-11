#!/usr/bin/env fish
# Full verification sequence for the HFW engine.
#
# Usage: ./scripts/test-hfw.fish
# Exits non-zero on the first failure and keeps the log in the terminal.

set -g root (realpath (string join / (dirname (status filename)) ..))
set -g tmp (mktemp -d -t hfw-test.XXXXXX)
set -g bin "$root/target/release/hfw-cli"

function cleanup --on-event fish_exit
    test -d "$tmp"; and rm -rf "$tmp"
end

function fail --argument-names message
    printf 'FAIL: %s\n' "$message" >&2
    exit 1
end

function section --argument-names name
    printf '\n== %s ==\n' "$name"
end

cd $root; or fail "cannot enter $root"

section Formatting
cargo fmt --all; or fail 'cargo fmt failed'
cargo fmt --all -- --check; or fail 'the tree is not formatted'

section Lints
cargo clippy --workspace --all-targets -- -D warnings; or fail 'clippy reported warnings'

section Tests
cargo test --workspace --all-targets --release; or fail 'tests failed'

section 'Release build'
cargo build --release --workspace; or fail 'release build failed'

section 'One layer, exact geometry'
$bin fixtures/cube_20mm_ascii.stl --z 10 --svg "$tmp/one.svg" >"$tmp/one.log"
or fail 'slicing one layer failed'
test -s "$tmp/one.svg"; or fail 'the SVG is empty'
grep -q '<svg' "$tmp/one.svg"; or fail 'the SVG has no root element'
grep -q 'Planned' "$tmp/one.log"; or fail 'no plan summary was printed'

section 'ASCII and binary STL agree'
$bin fixtures/cube_20mm_binary.stl --z 10 --svg "$tmp/binary.svg" >/dev/null
or fail 'slicing the binary STL failed'
set -l ascii_outline (grep -o 'd="[^"]*"' "$tmp/one.svg" | head -1)
set -l binary_outline (grep -o 'd="[^"]*"' "$tmp/binary.svg" | head -1)
test "$ascii_outline" = "$binary_outline"; or fail 'ASCII and binary STL produced different outlines'
printf 'Reference outlines match exactly.\n'

section 'Layer stack'
$bin fixtures/cube.stl --layer-height 0.2 --first-layer-height 0.2 --out-dir "$tmp/stack" >/dev/null
or fail 'slicing the layer stack failed'
set -l layers (count (find "$tmp/stack" -maxdepth 1 -name 'layer_*.svg'))
set -l lines (count (cat "$tmp/stack/manifest.csv"))
test $layers -eq 100; or fail "expected 100 layers, got $layers"
test $lines -eq 101; or fail "expected 101 manifest lines, got $lines"
head -2 "$tmp/stack/manifest.csv" | tail -1 | grep -q '^1,0.000000,0.100000,0.200000'
or fail 'the first manifest row is wrong'
tail -1 "$tmp/stack/manifest.csv" | grep -q '^100,19.800000,19.900000,20.000000'
or fail 'the last manifest row is wrong'
printf 'Verified %s layers and %s manifest lines.\n' $layers $lines

section 'Variable first layer and shortened final layer'
$bin fixtures/cube.stl --layer-height 0.4 --first-layer-height 0.3 --out-dir "$tmp/var" >/dev/null
or fail 'slicing with a variable first layer failed'
set -l var_layers (count (find "$tmp/var" -maxdepth 1 -name 'layer_*.svg'))
test $var_layers -eq 51; or fail "expected 51 layers, got $var_layers"
tail -1 "$tmp/var/manifest.csv" | grep -q '^51,19.900000,19.950000,20.000000'
or fail 'the final layer does not end at 20 mm'
printf 'Verified 51 layers ending exactly at 20 mm.\n'

section 'Material accounting'
for model in cube sphere torus
    set -l log "$tmp/accounting_$model.log"
    $bin fixtures/$model.stl --layer-height 0.2 --density 1.0 \
        --out-dir "$tmp/acc_$model" >$log
    or fail "slicing $model failed"
    set -l got (string replace -r '.*extruded_over_mesh=([0-9.]+).*' '$1' <$log | string match -r '^[0-9.]+$')
    test -n "$got"; or fail "no material ratio was reported for $model"
    set -l want
    switch $model
        case cube
            set want 0.9963
        case sphere
            set want 0.9930
        case torus
            set want 0.9853
    end
    set -l delta (math "abs($got - $want)")
    test -n "$delta"; or fail "could not compute the deviation for $model"
    if test $delta -gt 0.004
        fail "$model: extruded_over_mesh=$got, expected $want"
    end
    printf '  %-7s extruded_over_mesh=%s (expected %s)\n' $model $got $want
end

section 'Thin wall: HFW prints it, classic does not'
$bin fixtures/thin_wall.stl --z 2 --mode hfw --svg "$tmp/thin_hfw.svg" | grep -q '1 middle'
or fail 'the 0.3 mm wall did not become a middle bead in HFW mode'
$bin fixtures/thin_wall.stl --z 2 --mode classic --svg "$tmp/thin_classic.svg" | grep -q '0 walls, 0 middle'
or fail 'classic mode printed the 0.3 mm wall'
printf 'Verified: the 0.3 mm wall is a bead in HFW mode and absent in classic mode.\n'

section 'Fan-triangulated surfaces must close'
$bin fixtures/cone.stl --layer-height 0.2 --every 10 --out-dir "$tmp/cone" >"$tmp/cone.log"
or fail 'slicing the cone failed'
if grep -q 'WARNING: .* layer(s)' "$tmp/cone.log"
    cat "$tmp/cone.log" >&2
    fail 'the cone stack contains unclosed layers'
end
printf 'Verified: every cone layer closed.\n'

section 'An open mesh is refused'
$bin fixtures/open_triangle_ascii.stl --z 5 --svg "$tmp/open.svg" >"$tmp/open.log" 2>&1
and fail 'an open mesh sliced successfully'
grep -qi 'open contour' "$tmp/open.log"; or fail 'the open mesh was refused for the wrong reason'

section 'Geometry output is deterministic'
$bin fixtures/sphere.stl --layer-height 0.2 --every 5 --out-dir "$tmp/d1" >/dev/null
or fail 'the first determinism run failed'
$bin fixtures/sphere.stl --layer-height 0.2 --every 5 --out-dir "$tmp/d2" >/dev/null
or fail 'the second determinism run failed'
diff -r -x job_report.txt "$tmp/d1" "$tmp/d2"; or fail 'two identical runs produced different output'
printf 'Verified: SVGs and manifest are byte-identical (the job report holds timings).\n'

section 'G-code with a printer profile'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --out-dir "$tmp/gcode" --gcode "$tmp/cube.gcode" >"$tmp/gcode.log"
or fail 'G-code generation failed'
grep -q 'verify RESULT PASS' "$tmp/gcode.log"; or fail 'the verifier rejected our own G-code'
grep -q 'M83' "$tmp/cube.gcode"; or fail 'no relative extrusion mode was emitted'
grep -q 'PRINT_START BED=60 EXTRUDER=215' "$tmp/cube.gcode"; or fail 'the start macro was not emitted'
grep -q 'SET_PRESSURE_ADVANCE ADVANCE=0.045' "$tmp/cube.gcode"; or fail 'pressure advance was not emitted'
string match -q -- '*G-code: 100 layers, 1273*' (cat "$tmp/gcode.log")
or fail 'the filament total is not the expected 1273 mm'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --out-dir "$tmp/gcode2" --gcode "$tmp/cube2.gcode" >/dev/null
or fail 'the second G-code run failed'
cmp -s "$tmp/cube.gcode" "$tmp/cube2.gcode"; or fail 'two identical G-code runs differ'
printf 'Verified: 100 layers, expected filament total, byte-identical reruns.\n'

section 'The verifier rejects an unsafe file'
printf 'G1 X9999 Y0 F100\n' >"$tmp/bad.gcode"
$bin --check-gcode "$tmp/bad.gcode" --profile profiles/voron_klipper.cfg >"$tmp/bad.log" 2>&1
and fail 'an unsafe file passed verification'
grep -q 'verify RESULT FAIL' "$tmp/bad.log"; or fail 'the verifier did not report a failure'
grep -q 'outside the bed' "$tmp/bad.log"; or fail 'the bed overflow was not named'
printf 'Verified: bed overflow is caught.\n'

section 'Profiles reject unsafe settings'
sed 's/^walls = 3/walls = 0/' profiles/voron_klipper.cfg >"$tmp/bad.cfg"
or fail 'could not build the invalid profile'
$bin fixtures/cube.stl --profile "$tmp/bad.cfg" --layer-height 0.2 \
    --out-dir "$tmp/badp" >"$tmp/badp.log" 2>&1
and fail 'a profile with zero walls was accepted'
grep -q 'between 1 and 8' "$tmp/badp.log"; or fail 'the walls range error is missing'
printf 'Verified: profile validation runs before slicing.\n'

section 'Two-reference oracle'
if command -q prusa-slicer; and command -q orca-slicer
    python3 scripts/oracle.py fixtures/cube.stl fixtures/taper_wall.stl fixtures/thin_wall.stl \
        | grep -E '===|PASS|FAIL'
    or fail 'the oracle reported a deviation'
else
    printf 'skipped: OrcaSlicer or PrusaSlicer is not installed.\n'
end

printf '\nALL HFW ENGINE CHECKS PASSED.\n'
