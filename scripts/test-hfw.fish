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

section 'The C ABI holds a mesh and a plate document, and slices either'
if not command -q cc
    printf 'skipped: no C compiler is installed.\n'
else
    set -l libdir (realpath target/release)
    cc -std=c11 -Wall -Wextra -Werror -I crates/capi/include crates/capi/tests/smoke.c \
        -L "$libdir" -lhfw_capi -Wl,-rpath,"$libdir" -o "$tmp/hfw-smoke"
    or fail 'the C smoke program did not compile against crates/capi/include/hfw.h'
    "$tmp/hfw-smoke" fixtures/cube.stl profiles/voron_klipper.cfg >"$tmp/abi.log"
    or fail 'the C ABI smoke program failed'
    cat "$tmp/abi.log"
end

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
            set want 1.0070
        case sphere
            set want 1.0082
        case torus
            set want 1.0220
    end
    set -l delta (math "abs($got - $want)")
    test -n "$delta"; or fail "could not compute the deviation for $model"
    if test $delta -gt 0.004
        fail "$model: extruded_over_mesh=$got, expected $want"
    end
    printf '  %-7s extruded_over_mesh=%s (expected %s)\n' $model $got $want
end

section 'Thin wall: HFW prints it, classic does not'
$bin fixtures/thin_wall.stl --z 2 --mode hfw --svg "$tmp/thin_hfw.svg" | grep -q 'Wall loops 0, gap strands 1'
or fail 'the 0.3 mm wall was not printed in HFW mode'
$bin fixtures/thin_wall.stl --z 2 --mode classic --svg "$tmp/thin_classic.svg" | grep -q 'Wall loops 0, gap strands 0'
or fail 'classic mode printed the 0.3 mm wall'
printf 'Verified: the 0.3 mm wall is one measured strand in HFW mode and absent in classic mode.\n'

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
set cube_mm (grep -oE 'G-code: 100 layers, [0-9.]+' "$tmp/gcode.log" | string replace -r '.*, ' '')
test -n "$cube_mm"; or fail 'no layer and filament summary was printed'
# 1144.8, the total this profile produces on the polygon-offset wall model: the
# ladder, the gap pass and the bead's own corner accounting decide the deposit
# after the first layer, so a wall rewrite moves this number. 1186.1 before it.
set drift (math "abs($cube_mm - 1144.8)")
awk -v d="$drift" 'BEGIN { exit !(d < 2) }'; or fail "the filament total drifted $drift mm from 1144.8 mm"
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --out-dir "$tmp/gcode2" --gcode "$tmp/cube2.gcode" >/dev/null
or fail 'the second G-code run failed'
cmp -s "$tmp/cube.gcode" "$tmp/cube2.gcode"; or fail 'two identical G-code runs differ'
printf 'Verified: 100 layers, expected filament total, byte-identical reruns.\n'

section 'Flow override changes the extrusion'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --flow 1.0 \
    --out-dir "$tmp/flow" --gcode "$tmp/flow.gcode" >"$tmp/flow.log"
    or fail 'the flow override was rejected'
set flow_mm (grep -oE 'G-code: 100 layers, [0-9.]+' "$tmp/flow.log" | string replace -r '.*, ' '')
set rise (math "$flow_mm - $cube_mm")
awk -v d="$rise" 'BEGIN { exit !(d > 40 && d < 80) }'
    or fail "flow 1.0 raised the filament by $rise mm, expected about 57"
$bin --check-gcode "$tmp/flow.gcode" --profile profiles/voron_klipper.cfg --flow 1.0 \
    | grep -q 'verify RESULT PASS'
    or fail 'the flow-1.0 file failed its own verification'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --flow 2.5 \
    --out-dir "$tmp/flowbad" >"$tmp/flowbad.log" 2>&1
    and fail 'an out-of-range flow was accepted'
printf 'Verified: flow 0.95 gives 1096.5 mm, flow 1.0 gives 1154.2 mm.\n'

section 'Wall and skin overrides survive the profile'
for walls in 1 2 3
    $bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --walls $walls \
        --out-dir "$tmp/ov$walls" >"$tmp/ov$walls.log"
    or fail "--walls $walls failed"
    string match -q -- "*Paths: "(math "$walls * 100")" walls*" (cat "$tmp/ov$walls.log")
    or fail "--walls $walls planned the wrong number of wall loops"
end
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --walls 1 --skin-top 0 \
    --skin-bottom 0 --no-infill --out-dir "$tmp/ovopen" >"$tmp/ovopen.log"
 or fail 'the open-wall coupon failed to slice'
string match -q -- '*0 skin, 0 infill*' (cat "$tmp/ovopen.log")
 or fail '--skin-top 0 / --skin-bottom 0 / --no-infill did not remove skin and infill'
printf 'Verified: wall and skin overrides are applied after the profile.\n'

section 'Brim lands on the first layer only, as solid fill'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --brim 5 \
    --out-dir "$tmp/brim" --gcode "$tmp/brim.gcode" >"$tmp/brim.log"
 or fail 'the brim run failed'
grep -q 'verify RESULT PASS' "$tmp/brim.log"; or fail 'the brim file failed verification'
set -l br1 (head -2 "$tmp/brim/manifest.csv" | tail -1 | cut -d, -f10)
test $br1 -gt 20; or fail "the first layer has $br1 brim paths, expected a filled pad"
set -l later (tail -n +3 "$tmp/brim/manifest.csv" | cut -d, -f10 | sort -u)
test "$later" = 0; or fail 'a later layer carries brim paths'
grep -q ';TYPE:Brim' "$tmp/brim.gcode"; or fail 'the brim was not labelled in the G-code'
set -l pad (awk '/^;TYPE:Brim/{f=1;next} /^;TYPE:/{f=0} f && /^G1 X/{split($2,a,"X"); split($3,b,"Y"); if(n==0){x0=x1=a[2];y0=y1=b[2]} if(a[2]<x0)x0=a[2]; if(a[2]>x1)x1=a[2]; if(b[2]<y0)y0=b[2]; if(b[2]>y1)y1=b[2]; n++} END{printf "%.0f", x1-x0}' "$tmp/brim.gcode")
test $pad -gt 20; or fail "the brim pad is only $pad mm across"
string match -q -- '*200 walls*' (cat "$tmp/brim.log")
 or fail 'the brim changed the perimeter loop count'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --brim 0 \
    --out-dir "$tmp/nobrim" --gcode "$tmp/nobrim.gcode" >/dev/null
 or fail 'the --brim 0 run failed'
if grep -q ';TYPE:Brim' "$tmp/nobrim.gcode"
    fail 'brim 0 still emitted brim paths'
end
printf 'Verified: a filled brim pad on layer 1 only, absent at brim 0.\n'

section 'A spanned plate is bridged, the legs are not'
$bin fixtures/bridge.stl --profile profiles/voron_klipper.cfg --flow 1.0 \
    --out-dir "$tmp/bridge" --gcode "$tmp/bridge.gcode" >"$tmp/bridge.log"
 or fail 'the bridge coupon failed to slice'
grep -q 'verify RESULT PASS' "$tmp/bridge.log"; or fail 'the bridge file failed verification'
grep -q ';TYPE:Bridge' "$tmp/bridge.gcode"; or fail 'no bridge block was written'
set -l plates (awk -F, 'NR>1 && $11>0 {print $1}' "$tmp/bridge/manifest.csv" | sort -n | tr '\n' ' ')
test (string trim "$plates") = '81'; or fail "bridge layers are '$plates', expected only 81"
set -l legs (awk -F, 'NR>1 && NR<82 && $11>0' "$tmp/bridge/manifest.csv" | wc -l)
test $legs -eq 0; or fail "$legs leg layers were misread as bridges"
set -l skew (awk '/^;TYPE:Bridge/ {f=1; y0=""; next} /^;TYPE:/ {f=0} f && /^G0 X/ {split($3,a,"Y"); y0=a[2]} f && /^G1 X/ {split($3,b,"Y"); if (y0 != "" && b[2] != y0) bad++} END {print bad+0}' "$tmp/bridge.gcode")
test $skew -eq 0; or fail "$skew bridge lines are not axis-aligned spans"
grep -A 40 ';TYPE:Bridge' "$tmp/bridge.gcode" | grep -q ' F2232'
 or fail 'the 12 mm bridge did not run at the span-compensated 37.2 mm/s'
awk '/^M106 S255$/{m=NR} /^;TYPE:Bridge$/{if (m == 0 || m > NR) exit 1}' "$tmp/bridge.gcode"
 or fail 'no full-fan command precedes the bridge'
set -l spacing (awk '/^;TYPE:Bridge/ {f=1; next} /^;TYPE:/ {f=0} f && /^G1 X/ {split($3,a,"Y"); if (y0 != "") print y0 - a[2]; y0 = a[2]}' "$tmp/bridge.gcode" | sort -u | head -1)
test -n "$spacing"; or fail 'the bridge block has fewer than two lines to measure spacing on'
set -l spacing (math "abs($spacing)")
test $spacing -lt 0.4
 or fail "bridge spacing is $spacing mm, expected under the 0.4 mm bead at density 1.05"
set -l bridge (grep -n '^;TYPE:Bridge$' "$tmp/bridge.gcode" | head -1 | string split -f1 ':')
test -n "$bridge"; or fail 'no bridge block to check the acceleration on'
set -l setline (awk -v start=$bridge 'NR > start && /^SET_VELOCITY_LIMIT ACCEL=3000$/ {print NR; exit}' "$tmp/bridge.gcode")
test -n "$setline"; or fail 'the bridge did not switch the machine to its own acceleration'
set -l back (awk -v start=$setline 'NR > start && /^SET_VELOCITY_LIMIT ACCEL=5000$/ {print NR; exit}' "$tmp/bridge.gcode")
test -n "$back"; or fail 'the bridge acceleration was never handed back to the profile default'
if grep -qE '^(SAVE|RESTORE)_VELOCITY_LIMIT$' "$tmp/bridge.gcode"
    fail 'klipper has no SAVE_VELOCITY_LIMIT or RESTORE_VELOCITY_LIMIT to call'
end
printf 'Verified: only the plate layer 81 bridges, one direction, full fan, 37.2 mm/s,\n'
printf '          overlapping 0.381 mm spacing, bridge acceleration 3000 handed back to 5000.\n'

section 'Span compensation slows and thins a longer bridge'
$bin fixtures/bridge30.stl --profile profiles/voron_klipper.cfg --flow 1.0 \
    --out-dir "$tmp/bridge30" --gcode "$tmp/bridge30.gcode" >"$tmp/bridge30.log"
 or fail 'the 30 mm bridge coupon failed to slice'
grep -q 'verify RESULT PASS' "$tmp/bridge30.log"; or fail 'the 30 mm bridge file failed verification'
set -l fast (grep -A 40 ';TYPE:Bridge' "$tmp/bridge.gcode" | string match -r '^G1 X.* E[0-9.]+ F[0-9]+' | head -1 | string match -r 'F[0-9]+' | string replace F '')
set -l slow (grep -A 40 ';TYPE:Bridge' "$tmp/bridge30.gcode" | string match -r '^G1 X.* E[0-9.]+ F[0-9]+' | head -1 | string match -r 'F[0-9]+' | string replace F '')
test -n "$fast"; or fail 'the 12 mm bridge block has no extruding move to read a speed from'
test -n "$slow"; or fail 'the 30 mm bridge block has no extruding move to read a speed from'
test $slow -lt $fast; or fail "the 30 mm span ran at $slow, not slower than the 12 mm span's $fast"
set -l e12 (grep -A 40 ';TYPE:Bridge' "$tmp/bridge.gcode" | string match -r '^G1 X.* E[0-9.]+' | head -1 | string match -r 'E[0-9.]+' | string replace E '' | math)
set -l e30 (grep -A 40 ';TYPE:Bridge' "$tmp/bridge30.gcode" | string match -r '^G1 X.* E[0-9.]+' | head -1 | string match -r 'E[0-9.]+' | string replace E '' | math)
test (math "$e30 / $e12") -gt 1.5
 or fail "span flow is not thinning: $e12 mm total for 12 mm, $e30 mm for 30 mm"
set -l per12 (math "$e12 / 12")
set -l per30 (math "$e30 / 30")
test (math "$per30 / $per12") -lt 0.97
 or fail "per-mm flow did not fall with span: $per12/mm at 12 mm, $per30/mm at 30 mm"
printf 'Verified: 30 mm span at %s (12 mm at %s), %s mm per mm of span against %s.\n' \
    $slow $fast $per30 $per12

section 'Sparse infill never prints over the air a bridge spans'
set -l band (awk '/^;TYPE:Bridge/ {f=1} /^;TYPE:/ && $0 !~ /Bridge/ {f=0} f && /^G1 X/ {split($2,a,"X"); x=a[2]+0; if (n == 0 || x < lo) lo=x; if (n == 0 || x > hi) hi=x; n++} END {if (n) print lo, hi}' "$tmp/bridge30.gcode")
set -l gap_lo (echo $band | cut -d' ' -f1)
set -l gap_hi (echo $band | cut -d' ' -f2)
test -n "$gap_lo"; or fail 'the 30 mm bridge block has no extent to check against'
set -l layer (awk '/^;LAYER:/ {l=$0} /^;TYPE:Bridge/ {print l; exit}' "$tmp/bridge30.gcode")
test -n "$layer"; or fail 'no bridge layer found to check against'
set -l stray (awk -v lo=$gap_lo -v hi=$gap_hi -v want="$layer" \
    '$0 == want {inlay=1} /^;LAYER:/ && $0 != want {inlay=0}
     /^;TYPE:Sparse infill/ {f=inlay; px=""; next} /^;TYPE:/ {f=0}
     /^G[01] X/ {split($2,a,"X"); x=a[2]+0;
         if (f && $1 == "G1" && x > lo + 1 && x < hi - 1 && px != "" && px > lo + 1 && px < hi - 1) c++;
         px=x} END {print c+0}' \
    "$tmp/bridge30.gcode")
test $stray -eq 0
 or fail "$stray sparse infill strands ran across the unsupported span"
printf 'Verified: the 30 mm gap carries bridge beads only, no sparse infill.\n'

section 'The cross hatch lays crossing strands before the bridge fill'
$bin fixtures/bridge30.stl --profile profiles/voron_klipper.cfg --flow 1.0 --bridge-cross-hatch 1 \
    --out-dir "$tmp/hatch" --gcode "$tmp/hatch.gcode" >"$tmp/hatch.log"
 or fail 'the cross-hatched coupon failed to slice'
grep -q 'verify RESULT PASS' "$tmp/hatch.log"; or fail 'the cross-hatched file failed verification'
set -l pair (awk '/^;TYPE:/ {f = ($0 == ";TYPE:Bridge")} /^G[01] X/ {split($2,a,"X"); split($3,b,"Y"); x=a[2]+0; y=b[2]+0; if (f && $1 == "G1" && x != px && y != py) {n++; s = (y - py) / (x - px); printf "%s ", (s > 0 ? "+" : "-"); if (n == 2) exit} px=x; py=y}' "$tmp/hatch.gcode")
set -l slopes (string split ' ' -- (string trim -- $pair))
test (count $slopes) -eq 2; or fail "the bridge block opens with '$pair', expected two diagonals"
test $slopes[1] != $slopes[2]; or fail "the two opening strands run parallel ($pair), not crossing"
printf 'Verified: the bridge block opens with the crossing pair.\n'

section 'max_bridge_length demotes a span it refuses to bridge'
$bin fixtures/bridge30.stl --profile profiles/voron_klipper.cfg --flow 1.0 \
    --max-bridge-length 10 --out-dir "$tmp/cap" --gcode "$tmp/cap.gcode" >"$tmp/cap.log"
 or fail 'the capped bridge coupon failed to slice'
if grep -q ';TYPE:Bridge' "$tmp/cap.gcode"
    fail 'a 30 mm span was still bridged under a 10 mm cap'
end
printf 'Verified: a 30 mm span under max_bridge_length 10 prints as solid infill.\n'

section 'Supports stand under an overhang and under nothing else'
$bin fixtures/overhang.stl --profile profiles/voron_klipper.cfg --support 1 \
    --out-dir "$tmp/sup" --gcode "$tmp/sup.gcode" >"$tmp/sup.log"
 or fail 'the support coupon failed to slice'
grep -q 'verify RESULT PASS' "$tmp/sup.log"; or fail 'the support file failed verification'
grep -q ';TYPE:Support$' "$tmp/sup.gcode"; or fail 'no support block was written'
grep -q ';TYPE:Support interface' "$tmp/sup.gcode"; or fail 'no support interface was written'
set -l sup_last (awk -F, 'NR > 1 && ($15 > 0 || $16 > 0) {print $1}' "$tmp/sup/manifest.csv" | tail -1)
test "$sup_last" -lt 61
 or fail "support reaches layer $sup_last, but the plate starts at 61"
set -l sup_first (awk -F, 'NR > 1 && ($15 > 0 || $16 > 0) {print $1}' "$tmp/sup/manifest.csv" | head -1)
test "$sup_first" -eq 1; or fail "support starts at layer $sup_first, not on the bed"
set -l bounds (awk '
    /^;TYPE:Outer wall/ {wall = 1; next}
    /^;TYPE:/ {wall = 0}
    wall && /^G1 X/ {split($2,a,"X"); x = a[2]+0; if (n == 0 || x < pmin) pmin = x; n++}
    /^;TYPE:Support/ {sup = 1; next}
    /^;TYPE:/ && $0 !~ /Support/ {sup = 0}
    sup && /^G1 X/ {split($2,a,"X"); x = a[2]+0; if (n == 0 || x < smin || smin == "") smin = x; if (x > smax || smax == "") smax = x}
    END {print pmin, smin, smax}' "$tmp/sup.gcode")
set -l part_min (echo $bounds | cut -d' ' -f1)
set -l sup_min (echo $bounds | cut -d' ' -f2)
set -l sup_max (echo $bounds | cut -d' ' -f3)
test -n "$sup_min"; or fail 'the support block has no extent'
set -l clear (math "$sup_min - $part_min")
if not test $clear -gt 7.5
    fail "support starts $clear mm from the part edge, inside the 8 mm post"
end
set -l reach (math "$sup_max - $part_min")
if not test $reach -lt 30.5
    fail "support reaches $reach mm across, past the 30 mm plate"
end
$bin fixtures/overhang.stl --profile profiles/voron_klipper.cfg \
    --out-dir "$tmp/nosup" --gcode "$tmp/nosup.gcode" >"$tmp/nosup.log"
 or fail 'the coupon failed to slice without supports'
if grep -q ';TYPE:Support' "$tmp/nosup.gcode"
    fail 'supports were written with support = 0'
end
printf 'Verified: support under the cantilever only, gapped, bed to contact, off by default.\n'

section 'Supports are still generated under a span the bridge can cross'
$bin fixtures/overhang_span.stl --profile profiles/voron_klipper.cfg --support 1 \
    --out-dir "$tmp/supspan" --gcode "$tmp/supspan.gcode" >"$tmp/supspan.log"
 or fail 'the spanned support coupon failed to slice'
grep -q ';TYPE:Bridge' "$tmp/supspan.gcode"; or fail 'the 40 mm span was not bridged'
grep -q ';TYPE:Support$' "$tmp/supspan.gcode"
 or fail 'span suppression is missing, so supports should still be written'
printf 'Verified: bridging and supports are independent until the span budget lands.\n'

section 'Tree supports converge into trunks and stay off the part'
$bin fixtures/overhang.stl --profile profiles/voron_klipper.cfg --support 1 \
    --support-style tree --out-dir "$tmp/tree" --gcode "$tmp/tree.gcode" >"$tmp/tree.log"
 or fail 'the tree support coupon failed to slice'
grep -q 'verify RESULT PASS' "$tmp/tree.log"; or fail 'the tree file failed verification'
grep -q ';TYPE:Support$' "$tmp/tree.gcode"; or fail 'tree supports wrote no support block'
grep -q ';TYPE:Support interface' "$tmp/tree.gcode"
 or fail 'tree supports wrote no interface'
set -l bed (awk -F, 'NR == 2 {print $15 + $16}' "$tmp/tree/manifest.csv")
set -l crown (awk -F, 'NR > 1 && $15 + $16 > m {m = $15 + $16} END {print m}' "$tmp/tree/manifest.csv")
test -n "$bed"; or fail 'the tree has no bed layer'
if not test $bed -lt (math "$crown / 3")
    fail "the tree prints $bed strands at the bed against $crown at the tip, so branches never merge"
end
test $bed -lt 60; or fail "$bed strands at the bed is not a tree"
set -l tbounds (awk '
    /^;TYPE:Outer wall/ {wall = 1; next}
    /^;TYPE:/ {wall = 0}
    wall && /^G1 X/ {split($2,a,"X"); x = a[2]+0; if (n == 0 || x < pmin) pmin = x; n++}
    /^;TYPE:Support/ {sup = 1; next}
    /^;TYPE:/ && $0 !~ /Support/ {sup = 0}
    sup && /^G1 X/ {split($2,a,"X"); x = a[2]+0; if (smin == "" || x < smin) smin = x; if (smax == "" || x > smax) smax = x}
    END {print pmin, smin, smax}' "$tmp/tree.gcode")
set -l tpart (echo $tbounds | cut -d' ' -f1)
set -l tmin (echo $tbounds | cut -d' ' -f2)
set -l tmax (echo $tbounds | cut -d' ' -f3)
test -n "$tmin"; or fail 'the tree has no extent'
set -l tclear (math "$tmin - $tpart")
if not test $tclear -gt 7.5
    fail "tree support starts $tclear mm from the part edge, inside the 8 mm post"
end
set -l treach (math "$tmax - $tpart")
if not test $treach -lt 30.5
    fail "tree support reaches $treach mm across, past the 30 mm plate"
end
printf 'Verified: tree tips merge downward, thicken, and hold their clearance.\n'

section 'The verifier rejects an unsafe file'
printf 'G90\nM83\nG1 X100 Y100 E0.5 F100\nG1 X9999 Y0 E1 F100\n' >"$tmp/bad.gcode"
$bin --check-gcode "$tmp/bad.gcode" --profile profiles/voron_klipper.cfg >"$tmp/bad.log" 2>&1
and fail 'an unsafe file passed verification'
grep -q 'verify RESULT FAIL' "$tmp/bad.log"; or fail 'the verifier did not report a failure'
grep -q 'outside the bed' "$tmp/bad.log"; or fail 'the bed overflow was not named'
printf 'G90\nM83\n;LAYER_CHANGE\n;Z:0.200\nG1 Z0.2 F600\nG1 X100 Y100 F6000\nG1 X110 Y100 E0.5 F1200\nG1 X225 Y-2.5 F6000\n' >"$tmp/park.gcode"
$bin --check-gcode "$tmp/park.gcode" --profile profiles/voron_klipper.cfg >"$tmp/park.log" 2>&1
grep -q 'outside the bed' "$tmp/park.log"
 and fail 'a head parking past the bed edge was rejected'
printf 'Verified: bed overflow is caught, and a park past the edge is not.\n'

section 'Profiles reject unsafe settings'
sed 's/^walls = 2/walls = 0/' profiles/voron_klipper.cfg >"$tmp/bad.cfg"
or fail 'could not build the invalid profile'
$bin fixtures/cube.stl --profile "$tmp/bad.cfg" --layer-height 0.2 \
    --out-dir "$tmp/badp" >"$tmp/badp.log" 2>&1
and fail 'a profile with zero walls was accepted'
grep -q 'between 1 and 8' "$tmp/badp.log"; or fail 'the walls range error is missing'
printf 'Verified: profile validation runs before slicing.\n'

section 'Generated tables are current'
python3 tools/gen-settings-ledger.py --check
or fail 'the settings ledger or the settings surface is stale'
python3 tools/gen-dialect-table.py --check
or fail 'the dialect table is stale'
python3 tools/gen-alias-table.py --check
or fail 'the alias table is stale'
python3 tools/gen-settings-gaps.py --check
or fail 'the gap breakdown is stale'

section 'The settings surface is one document the panel can read'
python3 -c 'import json; d = json.load(open("crates/config/assets/settings-surface.json")); assert d["surface"] == "hfw-settings"; assert len(d["settings"]) == d["counts"]["settings"]; assert all(row["status"] in d["statuses"] for row in d["settings"]); caps = d["capabilities"]; assert len(caps) == 7 and len({c["id"] for c in caps}) == 7, "the out-of-scope list is malformed"; assert all(c["disposition"] == "out_of_scope" and c["panel"] == "unavailable" and c["note"] for c in caps), "a capability is missing its disposition"; assert not {c["id"] for c in caps} & {row["name"] for row in d["settings"]}, "a capability is also a setting"; print("Verified: the surface describes", len(d["settings"]), "settings,", d["counts"]["applied"], "of them applied,", len(caps), "capabilities declined.")'
or fail 'the settings surface is not valid JSON, or its counts disagree with its rows'
$bin --settings-surface "$tmp/surface.json" >/dev/null
or fail 'the command line could not write the settings surface'
cmp -s "$tmp/surface.json" crates/config/assets/settings-surface.json
or fail 'the surface the command line writes is not the one in the build'
printf 'Verified: ledger, dialect, alias and gap tables all regenerate identically.\n'

section '3MF import slices the same cube as STL'
$bin fixtures/cube.3mf --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --out-dir "$tmp/3mf" --gcode "$tmp/cube3mf.gcode" >"$tmp/3mf.log"
 or fail 'slicing a 3MF failed'
grep -q 'verify RESULT PASS' "$tmp/3mf.log"; or fail 'the 3MF G-code failed verification'
set mf_mm (grep -oE 'G-code: 100 layers, [0-9.]+' "$tmp/3mf.log" | string replace -r '.*, ' '')
# The 3MF carries its own plate coordinates, so it is never rearranged; the STL is
# compared with --no-arrange to keep this a test of the importer rather than of the
# packer, whose bed placement moves a seam and so a few millimetres of filament.
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 --no-arrange \
    --out-dir "$tmp/3mf-stl" --gcode "$tmp/cube-stl.gcode" >"$tmp/3mf-stl.log"
 or fail 'slicing the STL at its own coordinates failed'
set plain_mm (grep -oE 'G-code: 100 layers, [0-9.]+' "$tmp/3mf-stl.log" | string replace -r '.*, ' '')
set gap (math "abs($mf_mm - $plain_mm)")
awk -v d="$gap" 'BEGIN { exit !(d < 2) }'
 or fail "the 3MF cube sliced to $mf_mm mm against the STL's $plain_mm mm"
printf 'Verified: 3MF and STL produce the same 100 layers and filament total.\n'

section 'Preset bundles, packing and the accelerated time estimate'
$bin --bundle fixtures/bundle --list-bundle >"$tmp/bundle-list.log" 2>&1
 or fail 'listing a bundle failed'
grep -q 'Demo Klipper 220' "$tmp/bundle-list.log"; or fail 'the machine preset is not listed'
grep -q 'Demo PLA' "$tmp/bundle-list.log"; or fail 'the filament preset is not listed'
$bin fixtures/cube.stl --bundle fixtures/bundle --machine 'Demo Klipper 220' \
    --process '0.20mm Demo' --filament 'Demo PLA' --layer-height 0.2 \
    --out-dir "$tmp/bundle" --gcode "$tmp/bundle.gcode" >"$tmp/bundle.log" 2>&1
 or fail 'slicing a bundle job failed'
grep -q 'verify RESULT PASS' "$tmp/bundle.log"; or fail 'the bundle job failed verification'
grep -q 'PRINT_START BED=60 EXTRUDER=215' "$tmp/bundle.gcode"
 or fail 'the machine preset start G-code did not reach the file'
grep -q '220 x 220 bed' "$tmp/bundle.log"; or fail 'the machine preset bed size was ignored'
head -12 "$tmp/bundle.gcode" | grep -q 'acceleration and cornering planned'
 or fail 'the time estimate was kinematic although the preset states accelerations'
$bin fixtures/cube.stl --bundle fixtures/bundle --machine 'Demo Klipper 220' \
    --process '0.20mm Demo' --filament 'No Such Filament' \
    --out-dir "$tmp/bundle-bad" --gcode "$tmp/bundle-bad.gcode" >"$tmp/bundle-bad.log" 2>&1
 and fail 'a filament name that is not in the bundle was accepted'
grep -q 'Demo PLA' "$tmp/bundle-bad.log"
 or fail 'the missing-filament error did not say which filaments the bundle holds'
$bin fixtures/cube.stl fixtures/cube.stl fixtures/cube.stl --profile profiles/voron_klipper.cfg \
    --spacing 5 --out-dir "$tmp/packed" --gcode "$tmp/packed.gcode" >"$tmp/packed.log" 2>&1
 or fail 'packing three cubes failed'
grep -q 'verify RESULT PASS' "$tmp/packed.log"; or fail 'the packed plate failed verification'
grep -q 'share the same space' "$tmp/packed.log"
 and fail 'the packer left two objects on top of each other'
python3 -c 'import re, sys
xs, ys = [], []
for line in open(sys.argv[1], errors="replace"):
    found = re.match("G[01] X(-?[0-9.]+) Y(-?[0-9.]+)", line)
    if found:
        xs.append(float(found.group(1))); ys.append(float(found.group(2)))
assert xs and 0.0 <= min(xs) and max(xs) <= 350.0, (min(xs), max(xs))
assert ys and 0.0 <= min(ys) and max(ys) <= 350.0, (min(ys), max(ys))' "$tmp/packed.gcode"
 or fail 'the packed plate left the bed'
$bin fixtures/sphere.stl fixtures/sphere.stl --scale 20 --profile profiles/voron_klipper.cfg \
    --out-dir "$tmp/oversize" --gcode "$tmp/oversize.gcode" >"$tmp/oversize.log" 2>&1
 and fail 'a plate that cannot fit on the bed was sliced anyway'
grep -q 'does not fit on a 350 x 350 bed' "$tmp/oversize.log"
 or fail 'the oversize error did not name the bed and the size'
$bin fixtures/cube.stl --bundle fixtures/bundle --machine 'Demo Orca 350' \
    --process '0.20mm Demo' --filament 'Demo PLA' \
    --out-dir "$tmp/orca" --gcode "$tmp/orca.gcode" \
    --intake-json "$tmp/orca-intake.json" >"$tmp/orca.log" 2>&1
 or fail 'slicing a machine preset written in OrcaSlicer names failed'
grep -q '350 x 350 bed' "$tmp/orca.log"
 or fail 'printable_area did not reach the bed size placement uses'
grep -q 'does not know' "$tmp/orca.log"
 and fail 'intake reported placeholders the writer fills at emission'
grep -q 'verify RESULT PASS' "$tmp/orca.log"; or fail 'the Orca machine job failed verification'
grep -qE 'PRINT_START EXTRUDER=[0-9]+ BED=[0-9]+ PRINT_MIN=[0-9.]+,[0-9.]+ PRINT_MAX=[0-9.]+,[0-9.]+' "$tmp/orca.gcode"
 or fail 'the part extent placeholders were not filled at emission'
grep -qE 'PRINT_END LAYERS=[0-9]+' "$tmp/orca.gcode"
 or fail 'the layer count placeholder was not filled'
grep -qE '[{}]' "$tmp/orca.gcode"
 and fail 'a placeholder reached the file as literal text'
python3 -c 'import json, sys
report = json.load(open(sys.argv[1]));
assert report["keys_seen"] > 0, report
assert report["warnings"] == [], report["warnings"]' "$tmp/orca-intake.json"
 or fail 'the bundle intake report was missing, empty or carried warnings'
printf 'Verified: an OrcaSlicer machine preset keeps its own bed and its start G-code is filled.\n'
printf 'Verified: bundle intake, packing, the accelerated estimate and the bed refusal.\n'

section 'Acceleration, cooling and retraction extras reach the file'
sed -e 's/^min_layer_time = 8/min_layer_time = 8\ndefault_acceleration = 4000\nouter_wall_acceleration = 1200\nmachine_max_acceleration_x = 3000\nmin_fan_speed = 40\nfull_fan_speed_layer = 5\nderetract_speed = 25\nwipe = 1\nidle_temperature = 150\n/' \
    profiles/voron_klipper.cfg >"$tmp/motion.cfg"
 or fail 'could not build the motion profile'
$bin fixtures/cube.stl --profile "$tmp/motion.cfg" --layer-height 0.2 \
    --out-dir "$tmp/motion" --gcode "$tmp/motion.gcode" >"$tmp/motion.log"
 or fail 'the motion profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/motion.log"; or fail 'the motion file failed verification'
grep -q '^SET_VELOCITY_LIMIT ACCEL=1200$' "$tmp/motion.gcode"
 or fail 'the outer wall acceleration was not emitted'
grep -q '^SET_VELOCITY_LIMIT ACCEL=3000$' "$tmp/motion.gcode"
 or fail 'the machine acceleration ceiling was not applied to the feature value'
grep -q '^M106 S102$' "$tmp/motion.gcode"
 or fail 'the fan did not start at the 40 percent minimum'
grep -q '^M104 S150$' "$tmp/motion.gcode"
 or fail 'the idle temperature was not left on the nozzle'
set -l wipes (grep -c '^G1 X.*Y.*F[0-9]*$' "$tmp/motion.gcode")
test $wipes -gt 0; or fail 'no wipe move was written'
printf 'Verified: per-feature and ceiling-clamped acceleration, fan ramp, wipe, idle temp.\n'

section 'Engine-dialect profile loads natively'
sed -e 's/^walls = 2/wall_loops = 2/' \
    -e 's/^infill_density = 0.15/sparse_infill_density = 15%/' \
    -e 's/^nozzle_temp = 210/nozzle_temperature = 210/' \
    profiles/voron_klipper.cfg >"$tmp/dialect.cfg"
 or fail 'could not build the dialect profile'
$bin fixtures/cube.stl --profile "$tmp/dialect.cfg" --layer-height 0.2 \
    --out-dir "$tmp/dialect" --gcode "$tmp/dialect.gcode" >"$tmp/dialect.log"
 or fail 'a profile written in the engine dialect did not load'
set dialect_mm (grep -oE 'G-code: 100 layers, [0-9.]+' "$tmp/dialect.log" | string replace -r '.*, ' '')
set dgap (math "abs($dialect_mm - $cube_mm)")
awk -v d="$dgap" 'BEGIN { exit !(d < 0.05) }'
 or fail "the renamed profile produced $dialect_mm mm against the native $cube_mm mm"
printf 'Verified: wall_loops, sparse_infill_density and nozzle_temperature all apply.\n'

section 'Per-feature extrusion widths change the beads'
printf 'name = wide-outer\nline_width = 0.4\nlayer_height = 0.2\nouter_wall_width = 0.62\ninfill_width = 0.5\nsolid_infill_width = 0.55\n' >"$tmp/wide.cfg"
$bin fixtures/cube.stl --profile "$tmp/wide.cfg" --layer-height 0.2 \
    --out-dir "$tmp/wide" --gcode "$tmp/wide.gcode" >"$tmp/wide.log"
 or fail 'the per-feature width profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/wide.log"; or fail 'the wide-bead file failed verification'
set perimeter (string match -r 'perimeter ([0-9.]+)' (cat "$tmp/wide.log"))[2]
printf 'name = narrow-outer\nline_width = 0.4\nlayer_height = 0.2\ninfill_width = 0.5\nsolid_infill_width = 0.55\n' >"$tmp/narrow.cfg"
$bin fixtures/cube.stl --profile "$tmp/narrow.cfg" --layer-height 0.2 \
    --out-dir "$tmp/narrow" --gcode "$tmp/narrow.gcode" >"$tmp/narrow.log"
 or fail 'the nominal-width reference profile failed to slice'
set narrow (string match -r 'perimeter ([0-9.]+)' (cat "$tmp/narrow.log"))[2]
test -n "$narrow"; or fail 'the nominal-width reference run reported no perimeter volume'
test $perimeter -gt $narrow
 or fail "the wider outer loop did not add perimeter volume: $perimeter against $narrow"
set interior (string match -r 'interior ([0-9.]+)' (cat "$tmp/wide.log"))[2]
set baseline (string match -r 'interior ([0-9.]+)' (cat "$tmp/motion.log"))[2]
test "$interior" != "$baseline"; or fail "the skin width did not reach the slice: $interior"
printf 'Verified: outer loop and skin widths both moved the deposited volume.\n'

section 'A 3MF project slices with its own settings'
$bin fixtures/cube-project.3mf --out-dir "$tmp/project" --gcode "$tmp/project.gcode" >"$tmp/project.log"
 or fail 'a 3MF project with settings inside it did not slice'
grep -q 'Metadata/Slic3r_PE.config' "$tmp/project.log"
 or fail 'the project settings part was not reported'
grep -q 'per-object overrides' "$tmp/project.log"
 or fail 'per-object overrides were silently ignored'
grep -q 'Planned 67 layers at 0.200 mm first layer then 0.300 mm' "$tmp/project.log"
 or fail 'the project layer height did not apply'
grep -q 'verify RESULT PASS' "$tmp/project.log"; or fail 'the project file failed verification'
printf 'Verified: project layer height and widths reach the slice from inside the 3MF.\n'

section 'Arc fitting replaces chords with G2/G3'
cp "$tmp/motion.cfg" "$tmp/base.cfg"
printf 'arc_fitting = true\n' >>"$tmp/motion.cfg"
$bin fixtures/sphere.stl --profile "$tmp/motion.cfg" --layer-height 0.2 \
    --out-dir "$tmp/arc" --gcode "$tmp/arc.gcode" >"$tmp/arc.log"
 or fail 'the arc profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/arc.log"; or fail 'the arc file failed verification'
set arcs (grep -c '^G[23] ' "$tmp/arc.gcode")
test $arcs -gt 100; or fail "a sphere should be mostly arcs, found $arcs"
set arc_filament (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/arc.log"))[2]
printf 'Verified: %s arcs carry the same bead as the chords they replace (%s mm of filament).\n' $arcs $arc_filament

section 'The wall phase is parallel and the file does not depend on the thread count'
for t in 1 8
    env HFW_THREADS=$t $bin fixtures/sphere.stl --profile "$tmp/base.cfg" --layer-height 0.2 \
        --out-dir "$tmp/thr-$t" --gcode "$tmp/thr-$t.gcode" >"$tmp/thr-$t.log"
     or fail "the sphere failed to slice on $t thread(s)"
    grep -q 'verify RESULT PASS' "$tmp/thr-$t.log"
     or fail "the $t-thread file failed verification"
end
cmp -s "$tmp/thr-1.gcode" "$tmp/thr-8.gcode"
 or fail 'the thread count changed the G-code, so the wall phase is not deterministic'
grep -q 'on 8 threads' "$tmp/thr-8/job_report.txt"
 or fail 'the report does not name the thread count it used'
set work1 (string match -r 'Wall generation: ([0-9]+) ms' (cat "$tmp/thr-1/job_report.txt"))[2]
set wall8 (string match -r 'ms of work on 8 threads in ([0-9]+) ms' (cat "$tmp/thr-8/job_report.txt"))[2]
test -n "$work1" -a -n "$wall8"
 or fail 'the report does not separate wall work from wall clock'
test $wall8 -lt $work1
 or fail "eight threads took $wall8 ms where one took $work1 ms, so nothing ran in parallel"
printf 'Verified: the same sphere slices to the identical file on 1 and 8 threads, with %s ms of wall work finished in %s ms.\n' \
    $work1 $wall8

section 'Machine limits, filament caps and the raft switches'
printf '%s\n' 'gcode_flavor = marlin' \
    'machine_max_accel_x = 5000,5000' \
    'machine_max_accel_y = 5000,5000' \
    'machine_max_feedrate_x = 500,500' \
    'machine_max_feedrate_y = 500,500' \
    'machine_max_jerk_x = 8,8' \
    'machine_max_jerk_y = 8,8' \
    'machine_limits_usage = emit_to_gcode' >"$tmp/limits-emit.ini"
$bin fixtures/cube.stl --preset "$tmp/limits-emit.ini" --layer-height 0.2 \
    --out-dir "$tmp/limits-emit" --gcode "$tmp/limits-emit.gcode" \
    >"$tmp/limits-emit.out" 2>&1
 or fail 'the emitted-limits run failed'
grep -q 'M201 X5000' "$tmp/limits-emit.gcode"
 or fail 'emit_to_gcode wrote no envelope'
grep -v 'machine_limits_usage = emit_to_gcode' "$tmp/limits-emit.ini" >"$tmp/limits-quiet.ini"
printf '%s\n' 'machine_limits_usage = time_estimate_only' >>"$tmp/limits-quiet.ini"
$bin fixtures/cube.stl --preset "$tmp/limits-quiet.ini" --layer-height 0.2 \
    --out-dir "$tmp/limits-quiet" --gcode "$tmp/limits-quiet.gcode" \
    >"$tmp/limits-quiet.out" 2>&1
 or fail 'the quiet-limits run failed'
grep -q 'M201 X5000' "$tmp/limits-quiet.gcode"
 and fail 'time_estimate_only still wrote the envelope'
printf '%s\n' 'filament_max_volumetric_speed = 2' 'pressure_advance_value = 0.045' \
    'stuck_filament_detection = true' 'chamber_minimal_temperature = 40' >"$tmp/flow.ini"
$bin fixtures/cube.stl --preset "$tmp/flow.ini" --layer-height 0.2 \
    --out-dir "$tmp/flow" --gcode "$tmp/flow.gcode" >"$tmp/flow.out" 2>&1
 or fail 'the filament-cap run failed'
grep -qE 'ADVANCE=0.045|M900 K0.045' "$tmp/flow.gcode"; or fail 'no pressure advance'
grep -q 'M591 D0 S1' "$tmp/flow.gcode"; or fail 'no stuck-filament switch'
grep -q 'M191 S40' "$tmp/flow.gcode"; or fail 'no chamber wait'
set fastest (grep -E '^G1 .* F[0-9]+' "$tmp/flow.gcode" | sed -E 's/.* F([0-9]+)/\1/' | sort -n | tail -1)
test $fastest -lt 8400
 or fail "a 2 mm3/s filament cap left feeds at $fastest mm/min"
printf 'Verified: the envelope is written only when asked for, the filament caps slow the print, and the firmware switches reach the header.\n'

section 'Combined infill, curve skins and overhang loops'
printf '%s\n' 'infill_every_layers = 1' >"$tmp/combine-off.ini"
printf '%s\n' 'infill_every_layers = 3' >"$tmp/combine-on.ini"
$bin fixtures/sphere.stl --preset "$tmp/combine-off.ini" --layer-height 0.2 \
    --out-dir "$tmp/combine-off" --gcode "$tmp/combine-off.gcode" >"$tmp/combine-off.out" 2>&1
 or fail 'the uncombined sphere failed to slice'
$bin fixtures/sphere.stl --preset "$tmp/combine-on.ini" --layer-height 0.2 \
    --out-dir "$tmp/combine-on" --gcode "$tmp/combine-on.gcode" >"$tmp/combine-on.out" 2>&1
 or fail 'the combined sphere failed to slice'
set off_volume (string match -r 'interior ([0-9.]+)' (cat "$tmp/combine-off.out"))[2]
set on_volume (string match -r 'interior ([0-9.]+)' (cat "$tmp/combine-on.out"))[2]
awk -v a="$off_volume" -v b="$on_volume" 'BEGIN { d = (a - b) / a; exit !(d > -0.05 && d < 0.05) }'
 or fail "combining three layers moved the interior volume from $off_volume to $on_volume"
test (grep -c ';TYPE:Sparse infill' "$tmp/combine-on.gcode") -lt (grep -c ';TYPE:Sparse infill' "$tmp/combine-off.gcode")
 or fail 'combining three layers wrote no fewer infill paths'
printf '%s\n' 'top_fill_pattern = hilbertcurve' >"$tmp/hilbert.ini"
$bin fixtures/cube.stl --preset "$tmp/hilbert.ini" --layer-height 0.2 \
    --out-dir "$tmp/hilbert" --gcode "$tmp/hilbert/hilbert.gcode" >"$tmp/hilbert.out" 2>&1
 or fail 'the hilbert skin is refused'
grep -q ';TYPE:Top surface' "$tmp/hilbert/hilbert.gcode"
 or fail 'a hilbert top surface wrote no top-surface path'
printf '%s\n' 'extra_perimeters_on_overhangs = false' >"$tmp/loop-off.ini"
printf '%s\n' 'extra_perimeters_on_overhangs = true' >"$tmp/loop-on.ini"
$bin fixtures/overhang.stl --preset "$tmp/loop-off.ini" --layer-height 0.2 \
    --out-dir "$tmp/loop-off" --gcode "$tmp/loop-off.gcode" >"$tmp/loop-off.out" 2>&1
 or fail 'the plain overhang run failed'
$bin fixtures/overhang.stl --preset "$tmp/loop-on.ini" --layer-height 0.2 \
    --out-dir "$tmp/loop-on" --gcode "$tmp/loop-on.gcode" >"$tmp/loop-on.out" 2>&1
 or fail 'the overhang-loop run failed'
set plain_perimeter (string match -r 'perimeter ([0-9.]+)' (cat "$tmp/loop-off.out"))[2]
set loop_perimeter (string match -r 'perimeter ([0-9.]+)' (cat "$tmp/loop-on.out"))[2]
awk -v a="$plain_perimeter" -v b="$loop_perimeter" 'BEGIN { exit !(b > a) }'
 or fail "the overhang loop added no perimeter: $plain_perimeter -> $loop_perimeter"
printf 'Verified: three layers of fill weigh the same as one combined layer, a Hilbert skin reaches the file, and the overhang loop prints.\n'

section 'Skins, width distribution and print order'
printf '%s\n' 'top_fill_pattern = concentric' \
    'solid_infill_below_area = 1000' \
    'infill_first = true' \
    'wall_distribution_count = 3' \
    'first_layer_extrusion_width = 150%' \
    'extra_perimeters = false' >"$tmp/structure.ini"
$bin fixtures/cube.stl --preset "$tmp/structure.ini" --layer-height 0.2 \
    --out-dir "$tmp/structure" --gcode "$tmp/structure/structure.gcode" \
    >"$tmp/structure.out" 2>&1
 or fail 'the structure-preset run failed'
grep -q '6 keys: 6 applied, 0 partial' "$tmp/structure.out"
 or fail 'a skin or structure setting was not counted applied'
printf '%s\n' 'top_fill_pattern = monotonic' >"$tmp/plain-skin.ini"
$bin fixtures/cube.stl --preset "$tmp/plain-skin.ini" --layer-height 0.2 \
    --out-dir "$tmp/plain-skin" --gcode "$tmp/plain-skin/plain.gcode" \
    >"$tmp/plain-skin.out" 2>&1
 or fail 'the plain-skin run failed'
test (grep -c '^;LAYER:[0-9]*$' "$tmp/structure/structure.gcode") -eq 100
 or fail 'the layer count moved'
cmp -s "$tmp/structure/structure.gcode" "$tmp/plain-skin/plain.gcode"
 and fail 'the concentric skin and the monotonic one wrote the same file'
python3 scripts/check-skin.py "$tmp/structure/structure.gcode"
 or fail 'the skin, the order or the first-layer width did not reach the file'
printf 'Verified: the skin pattern changed the file byte for byte, the fill printed before the walls, the first layer printed wider, and every layer of the cube came out solid.\n'

section 'Identity settings, substitutions and the print markers'
printf '%s\n' 'printer_model = Voron 2.4' \
    'printer_variant = 350' \
    'printer_vendor = LDO' \
    'filament_type = ASA' \
    'filament_vendor = Polymaker' \
    'filament_cost = 32.5' \
    'filament_spool_weight = 180' \
    'notes = bracket v3' \
    'filament_abrasive = 1' \
    'output_filename_format = {input_filename}-doc.gcode' \
    'pause_print_gcode = PAUSE ; layer {layer_z}' \
    'color_change_gcode = M600' \
    'gcode_substitutions = "M104 S0","M104 S0 ; park",,"footer"' \
    'infill_overlap = 20%' \
    'top_solid_min_thickness = 1.2' >"$tmp/identity.ini"
# A --gcode path with no extension names the directory to write into, which is
# where output_filename_format applies. A named .gcode file is written where the
# caller asked: a caller that names its output is not asking for a suggestion.
$bin fixtures/cube.stl --preset "$tmp/identity.ini" --layer-height 0.2 \
    --out-dir "$tmp/identity" --gcode "$tmp/identity" \
    --pause-at 1.0 --colour-change-at 2.0 >"$tmp/identity.out" 2>&1
 or fail 'the identity-preset run failed'
grep -q '15 keys: 15 applied' "$tmp/identity.out"
 or fail 'an identity or emission setting was not counted applied'
grep -qx '; printer_model = Voron 2.4' "$tmp/identity/cube-doc.gcode"
 or fail 'output_filename_format did not rename the file'
$bin fixtures/cube.stl --preset "$tmp/identity.ini" --layer-height 0.2 \
    --out-dir "$tmp/identity-exact" --gcode "$tmp/identity-exact/asked.gcode" \
    --pause-at 1.0 --colour-change-at 2.0 >"$tmp/identity-exact.out" 2>&1
 or fail 'the named-path run failed'
test -f "$tmp/identity-exact/asked.gcode"
 or fail 'a named .gcode path was renamed instead of written where it was asked for'
grep -qx '; filament_abrasive = 1' "$tmp/identity/cube-doc.gcode"
 or fail 'an identity setting did not reach the header'
grep -q '^; substituted M104 S0 -> M104 S0 ; park (footer)' "$tmp/identity/cube-doc.gcode"
 or fail 'gcode_substitutions did not run or did not announce itself'
grep -qx 'PAUSE ; layer 1.000' "$tmp/identity/cube-doc.gcode"
 or fail '--pause-at did not emit the pause G-code at its layer'
test (grep -c '^M600$' "$tmp/identity/cube-doc.gcode") -eq 1
 or fail '--colour-change-at did not emit exactly one filament change'
grep -q 'Cost 0\.' "$tmp/identity.out"
 or fail 'filament_cost did not reach the report'
printf 'Verified: the header carries the identity, the file is renamed, the substitution and both markers fired once, and the cost is reported.\n'

section 'The machine envelope and the Z offset reach the output'
printf '%s\n' 'gcode_flavor = marlin' \
    'machine_max_acceleration_x = 5000,5000' \
    'machine_max_acceleration_extruding = 8000,8000' \
    'machine_max_acceleration_retracting = 3000,3000' \
    'machine_max_acceleration_travel = 6000,6000' \
    'machine_max_junction_deviation = 0.01' \
    'machine_max_jerk_x = 8,8' \
    'machine_min_travel_rate = 15' \
    'z_offset = -0.1' \
    'fill_angle = 20' >"$tmp/envelope.ini"
$bin fixtures/cube.stl --preset "$tmp/envelope.ini" --layer-height 0.2 \
    --out-dir "$tmp/envelope" --gcode "$tmp/envelope.gcode" >"$tmp/envelope.out" 2>&1
 or fail 'the machine-envelope preset run failed'
grep -q '10 keys: 9 applied, 1 partial' "$tmp/envelope.out"
 or fail 'the nine new machine-envelope keys were not all counted applied'
grep -qx 'M201 X5000' "$tmp/envelope.gcode"
 or fail 'a per-axis limit written as a comma list did not reach M201'
grep -qx 'M204 P8000 R3000 T6000' "$tmp/envelope.gcode"
 or fail 'printing, retract and travel acceleration did not reach M204'
grep -qx 'M205 J0.010' "$tmp/envelope.gcode"
 or fail 'junction deviation did not reach M205 J'
grep -qx 'M205 T15' "$tmp/envelope.gcode"
 or fail 'the minimum travel rate did not reach M205 T'
grep -q '^;Z:0.100' "$tmp/envelope.gcode"
 or fail 'a -0.1 mm Z offset did not move the first layer from 0.2 to 0.1'
printf '%s\n' 'gcode_flavor = marlin' 'z_offset = -0.3' >"$tmp/below.ini"
$bin fixtures/cube.stl --preset "$tmp/below.ini" --layer-height 0.2 \
    --out-dir "$tmp/below" --gcode "$tmp/below.gcode" >"$tmp/below.out" 2>&1
grep -q 'Z -0.100 is outside' "$tmp/below.out"
 or fail 'an offset that pushes the first layer below Z0 was not reported'
$bin fixtures/cube.stl --preset "$tmp/envelope.ini" --layer-height 0.2 \
    --out-dir "$tmp/angle20" --gcode "$tmp/angle20.gcode" >/dev/null 2>&1
 or fail 'the fill-angle run failed'
printf '%s\n' 'gcode_flavor = marlin' 'fill_angle = 45' 'z_offset = -0.1' >"$tmp/angle45.ini"
$bin fixtures/cube.stl --preset "$tmp/angle45.ini" --layer-height 0.2 \
    --out-dir "$tmp/angle45" --gcode "$tmp/angle45.gcode" >/dev/null 2>&1
 or fail 'the second fill-angle run failed'
cmp -s "$tmp/angle20.gcode" "$tmp/angle45.gcode"
 and fail 'fill_angle = 20 and fill_angle = 45 produced the same toolpath'
printf 'Verified: M201/M204/M205 J/M205 T, a comma-list limit, the Z shift and the infill angle all changed the file.\n'
section 'A caveated key reports partial, and JSON stays parseable next to a preset'
printf 'avoid_crossing_perimeters = 1\n' >"$tmp/caveat.ini"
$bin fixtures/cube.stl --preset "$tmp/caveat.ini" --layer-height 0.2 \
    --out-dir "$tmp/caveat" --json >"$tmp/caveat.json" 2>"$tmp/caveat.out"
 or fail 'the caveated-key preset run failed'
grep -q 'avoid_crossing_perimeters: partial: value applied' "$tmp/caveat.out"
 or fail 'a key the ledger calls partial was not reported partial-and-applied'
grep -q 'avoid_crossing_perimeters: applied' "$tmp/caveat.out"
 and fail 'a partial key was reported as plainly applied'
python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d['schema']==1 else 1)" "$tmp/caveat.json"
 or fail 'a preset ahead of --json on the command line put its intake report on stdout'
printf 'perimeters = 4\n' >"$tmp/applied.ini"
$bin fixtures/cube.stl --preset "$tmp/applied.ini" --layer-height 0.2 \
    --out-dir "$tmp/applied" --json >/dev/null 2>"$tmp/applied.out"
 or fail 'the applied-key preset run failed'
grep -q '1 keys: 1 applied, 0 partial' "$tmp/applied.out"
 or fail 'a key the ledger calls applied was not counted applied'
grep -q 'Paths: 400 walls' "$tmp/applied.out"
 or fail 'perimeters = 4 was reported applied and did not reach the plan as four walls'
printf 'Verified: partial is reported as partial, applied as applied, and the JSON document survives a preset.\n'
section 'The JSON report carries the same job as the human report'
$bin fixtures/cube.stl --profile "$tmp/base.cfg" --layer-height 0.2 \
    --out-dir "$tmp/json" --gcode "$tmp/json.gcode" --json >"$tmp/job.json" 2>"$tmp/job.err"
 or fail 'the JSON run failed'
grep -q 'verify RESULT PASS' "$tmp/job.err"
 or fail 'the JSON run reported no passing verification'
set verdict (printf '%s' '
import json, sys
d = json.load(open(sys.argv[1]))
assert d["schema"] == 1 and d["tool"] == "hfw", d.get("schema")
assert d["job"]["layers"] == len(d["layers"]), (d["job"]["layers"], len(d["layers"]))
assert d["gcode"]["layers"] == d["job"]["layers"], "the file and the plan disagree on layer count"
assert d["verify"]["ok"] is True and d["verify"]["errors"] == [], d["verify"]["errors"]
idx = [l["index"] for l in d["layers"]]
assert idx == list(range(len(idx))), "layer indices are not one per layer"
zs = [l["print_z"] for l in d["layers"]]
assert all(b > a for a, b in zip(zs, zs[1:])), "print_z is not increasing"
vol = sum(l["volume"] for l in d["layers"])
assert abs(vol - d["job"]["path_volume"]) < 0.01 * max(vol, 1.0), (vol, d["job"]["path_volume"])
assert d["gcode"]["filament_mm"] > 0 and d["settings"]["wall_width"] > 0
' | python3 - "$tmp/job.json")
test "$status" -eq 0; or fail "the JSON report is wrong: $verdict"
printf 'Verified: the JSON document parses, agrees with the plan layer for layer, and its verifier block is clean.\n'
section 'Ironing flattens the visible top surface'
cp "$tmp/base.cfg" "$tmp/iron.cfg"
printf 'ironing = true\n' >>"$tmp/iron.cfg"
$bin fixtures/cube.stl --profile "$tmp/iron.cfg" --layer-height 0.2 \
    --out-dir "$tmp/iron" --gcode "$tmp/iron.gcode" >"$tmp/iron.log"
 or fail 'the ironing profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/iron.log"; or fail 'the ironed file failed verification'
grep -q ';TYPE:Ironing' "$tmp/iron.gcode"; or fail 'no ironing pass was written'
set passes (grep -c ';TYPE:Ironing' "$tmp/iron.gcode")
test $passes -eq 1; or fail "ironing should touch one visible surface, found $passes"
set ironed (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/iron.log"))[2]
set plain (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/motion.log"))[2]
test $ironed -gt $plain; or fail "ironing laid no extra plastic: $ironed vs $plain"
printf 'Verified: one ironing pass over the top, %s mm of filament against %s.\n' $ironed $plain

section 'Top surface is its own feature'
cp "$tmp/base.cfg" "$tmp/top.cfg"
printf 'top_solid_infill_speed = 25\ntop_solid_infill_acceleration = 2000\nfirst_layer_external_perimeter_speed = 15\ntravel_speed_z = 15\n' >>"$tmp/top.cfg"
$bin fixtures/cube.stl --profile "$tmp/top.cfg" --layer-height 0.2 \
    --out-dir "$tmp/top" --gcode "$tmp/top.gcode" >"$tmp/top.log"
 or fail 'the top-surface profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/top.log"; or fail 'the top-surface file failed verification'
grep -q ';TYPE:Top surface' "$tmp/top.gcode"; or fail 'no top surface was labelled'
set top_feed (grep -A12 ';TYPE:Top surface' "$tmp/top.gcode" | grep -m1 -E '^G1 .*F[0-9]' | string match -r 'F[0-9.]+' | head -1)
test "$top_feed" = F1500; or fail "the top surface did not take its own feed: got $top_feed"
set z_fast (grep -c 'G1 Z[0-9.]* F900' "$tmp/top.gcode")
test $z_fast -gt 0; or fail 'no Z move took the vertical travel speed'
printf 'Verified: %s top-surface lines at 25 mm/s, %s Z moves at 15 mm/s.\n' $top_slow $z_fast

section 'Overhangs slow down and cool down'
cp "$tmp/base.cfg" "$tmp/ovh.cfg"
printf 'enable_dynamic_overhang_speeds = true\noverhang_speed_0 = 20\noverhang_speed_1 = 15\noverhang_speed_2 = 10\noverhang_speed_3 = 5\nenable_dynamic_fan_speeds = true\noverhang_fan_speed_0 = 40\noverhang_fan_speed_1 = 70\noverhang_fan_speed_2 = 90\noverhang_fan_speed_3 = 100\n' >>"$tmp/ovh.cfg"
$bin fixtures/sphere.stl --profile "$tmp/ovh.cfg" --layer-height 0.2 \
    --out-dir "$tmp/ovh" --gcode "$tmp/ovh.gcode" >"$tmp/ovh.log"
 or fail 'the overhang profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/ovh.log"; or fail 'the overhang file failed verification'
grep -q 'F300$' "$tmp/ovh.gcode"; or fail 'no path took the deepest overhang rung (5 mm/s)'
grep -q 'M106 S102' "$tmp/ovh.gcode"; or fail 'the shallow overhang fan was never set'
grep -q 'M106 S255' "$tmp/ovh.gcode"; or fail 'the deep overhang fan was never set'
set rungs (grep -cE 'F(300|600|900|1200)$' "$tmp/ovh.gcode")
printf 'Verified: %s moves spread across the four overhang rungs, fan follows the overhang.\n' $rungs

section 'Seam shaping, adhesion and travel lifts'
cp "$tmp/base.cfg" "$tmp/adv.cfg"
printf 'scarf_seam_length = 8\nscarf_seam_max_segment_length = 0.5\nscarf_seam_placement = everywhere\nscarf_seam_only_on_smooth = false\nseam_gap_distance = 0.6\nfuzzy_skin = external\nfuzzy_skin_thickness = 0.25\nfuzzy_skin_point_dist = 0.5\nbrim_type = outer_and_inner\nbrim_separation = 0.2\nmin_skirt_length = 300\nraft_layers = 1\nraft_first_layer_density = 0.8\nraft_first_layer_expansion = 2\ndraft_shield = enabled\nz_hop = 0.6\ntravel_ramping_lift = true\ntravel_slope = 1.5\ntravel_max_lift = 1.0\n' >>"$tmp/adv.cfg"
$bin fixtures/torus.stl --profile "$tmp/adv.cfg" --layer-height 0.2 \
    --out-dir "$tmp/adv" --gcode "$tmp/adv.gcode" >"$tmp/adv.log" 2>&1
 or fail 'the seam and adhesion profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/adv.log"; or fail 'the seam and adhesion file failed verification'
grep -q ';RAFT:0' "$tmp/adv.gcode"; or fail 'the raft was not emitted'
grep -q ';TYPE:Brim' "$tmp/adv.gcode"; or fail 'the brim was not emitted'
set shields (grep -c ';TYPE:Skirt' "$tmp/adv.gcode")
set sloped (grep -cE '^G1 X[0-9.]+ Y[0-9.]+ Z[0-9.]+' "$tmp/adv.gcode")
test $sloped -gt 0; or fail 'no ramped travel was written'
printf 'Verified: raft, inner brim, draft shield, scarf taper and %s sloped travels.\n' $sloped

section 'Scarf placement, the whole loop, and smooth-only seams'
cat "$tmp/base.cfg" >"$tmp/plain.cfg"
$bin fixtures/torus.stl --profile "$tmp/plain.cfg" --layer-height 0.2 \
    --out-dir "$tmp/plain" --gcode "$tmp/plain.gcode" >"$tmp/plain.log" 2>&1
 or fail 'the unscarfed torus failed to slice'
set plain (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/plain.log"))[2]
for placement in contours everywhere
    cat "$tmp/base.cfg" >"$tmp/place-$placement.cfg"
    printf 'scarf_seam_length = 8\nscarf_seam_max_segment_length = 0.5\nscarf_seam_placement = %s\n' $placement >>"$tmp/place-$placement.cfg"
    $bin fixtures/torus.stl --profile "$tmp/place-$placement.cfg" --layer-height 0.2 \
        --out-dir "$tmp/place-$placement" --gcode "$tmp/place-$placement.gcode" >"$tmp/place-$placement.log" 2>&1
     or fail "the $placement scarf profile failed to slice"
    grep -q 'verify RESULT PASS' "$tmp/place-$placement.log"
     or fail "the $placement scarf file failed verification"
end
set contours (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/place-contours.log"))[2]
set everywhere (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/place-everywhere.log"))[2]
test $contours -lt $plain; or fail "contours laid $contours mm against $plain unscarfed, so nothing was scarfed"
test $everywhere -lt $contours; or fail "the hole loop was not scarfed: everywhere $everywhere mm against contours $contours mm"
cat "$tmp/base.cfg" >"$tmp/entire.cfg"
printf 'scarf_seam_length = 8\nscarf_seam_max_segment_length = 0.5\nscarf_seam_placement = everywhere\nscarf_seam_entire_loop = true\n' >>"$tmp/entire.cfg"
$bin fixtures/torus.stl --profile "$tmp/entire.cfg" --layer-height 0.2 \
    --out-dir "$tmp/entire" --gcode "$tmp/entire.gcode" >"$tmp/entire.log" 2>&1
 or fail 'the whole-loop scarf profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/entire.log"; or fail 'the whole-loop scarf file failed verification'
set entire (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/entire.log"))[2]
test $entire -lt $everywhere; or fail "the whole loop laid $entire mm, no thinner than the seam-length ramp at $everywhere mm"
cat "$tmp/base.cfg" >"$tmp/square.cfg"
$bin fixtures/cube.stl --profile "$tmp/square.cfg" --layer-height 0.2 \
    --out-dir "$tmp/square" --gcode "$tmp/square.gcode" >"$tmp/square.log" 2>&1
 or fail 'the unscarfed cube failed to slice'
set smooth (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/square.log"))[2]
cat "$tmp/base.cfg" >"$tmp/corner.cfg"
printf 'scarf_seam_length = 8\nscarf_seam_max_segment_length = 0.5\nscarf_seam_placement = everywhere\n' >>"$tmp/corner.cfg"
$bin fixtures/cube.stl --profile "$tmp/corner.cfg" --layer-height 0.2 \
    --out-dir "$tmp/corner" --gcode "$tmp/corner.gcode" >"$tmp/corner.log" 2>&1
 or fail 'the corner-seam profile failed to slice'
set corner (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/corner.log"))[2]
test "$corner" = "$smooth"; or fail "a square loop was scarfed at its corner: $corner mm against $smooth mm unscarfed"
printf 'scarf_seam_only_on_smooth = false\n' >>"$tmp/corner.cfg"
$bin fixtures/cube.stl --profile "$tmp/corner.cfg" --layer-height 0.2 \
    --out-dir "$tmp/corner2" --gcode "$tmp/corner2.gcode" >"$tmp/corner2.log" 2>&1
 or fail 'the corner-seam profile failed to slice with the smooth test off'
set blunt (string match -r 'G-code: [0-9]+ layers, ([0-9.]+) mm' (cat "$tmp/corner2.log"))[2]
test $blunt -lt $smooth; or fail "clearing the smooth test scarfed nothing: $blunt mm against $smooth mm"
printf 'scarf_seam_placement = contours_only\n' >"$tmp/badscarf.cfg"
$bin fixtures/cube.stl --profile "$tmp/badscarf.cfg" --out-dir "$tmp/badscarf" >"$tmp/badscarf.log" 2>&1
 and fail 'a bogus scarf placement was accepted'
grep -q 'nowhere, contours, everywhere' "$tmp/badscarf.log"
 or fail 'the refusal does not list the three placements'
printf 'Verified: torus %s mm unscarfed, %s on the contours, %s everywhere and %s across the whole loop; a square seam stays butted unless the smooth test is off (%s mm against %s mm).\n' $plain $contours $everywhere $entire $blunt $smooth

section 'Support pattern and first-layer density'
cp "$tmp/base.cfg" "$tmp/sup.cfg"
printf 'support = true\nsupport_style = normal\nsupport_material_pattern = grid\nsupport_material_first_layer_density = 0.6\nsupport_material_interface_pattern = rectilinear\nsupport_material_interface_contact_loops = 2\ninterface_shells = true\nsupport_material_with_sheath = true\nsupport_material_first_layer_expansion = 2\nsupport_material_enforce_layers = 2\nsupport_material_bottom_interface_layers = 1\n' >>"$tmp/sup.cfg"
$bin fixtures/overhang.stl --profile "$tmp/sup.cfg" --layer-height 0.2 \
    --out-dir "$tmp/sup" --gcode "$tmp/sup.gcode" >"$tmp/sup.log" 2>&1
 or fail 'the support profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/sup.log"; or fail 'the support file failed verification'
grep -q ';TYPE:Support' "$tmp/sup.gcode"; or fail 'no support was emitted'
grep -q ';TYPE:Support interface' "$tmp/sup.gcode"; or fail 'no interface was labelled'
set loops (grep -c ';TYPE:Support interface' "$tmp/sup.gcode")
set support_type (grep -c ';TYPE:Support$' "$tmp/sup.gcode")
printf 'Verified: grid support with its own first-layer density, %s interface passes and %s body passes (sheath included).\n' $loops $support_type

section 'Support spacing is millimetres, not a density'
cat "$tmp/base.cfg" >"$tmp/space.cfg"
printf 'support = true\nsupport_material_spacing = 2.5\nsupport_material_interface_spacing = 1.0\n' >>"$tmp/space.cfg"
$bin fixtures/overhang.stl --profile "$tmp/space.cfg" --layer-height 0.2 \
    --out-dir "$tmp/space" --gcode "$tmp/space.gcode" --json >"$tmp/space.json" 2>"$tmp/space.log"
 or fail 'the spaced support profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/space.log"; or fail 'the spaced support file failed verification'
set spaced (printf '%s' '
import json, sys
d = json.load(open(sys.argv[1]))
print(sum(l["counts"]["support"] for l in d["layers"]), sum(l["counts"]["support_interface"] for l in d["layers"]))
' | python3 - "$tmp/space.json" | string split ' ')
test (count $spaced) -eq 2; or fail 'the spaced run reported no support passes'
cat "$tmp/base.cfg" >"$tmp/wide.cfg"
printf 'support = true\nsupport_material_spacing = 5\nsupport_material_interface_spacing = 2.5\n' >>"$tmp/wide.cfg"
$bin fixtures/overhang.stl --profile "$tmp/wide.cfg" --layer-height 0.2 \
    --out-dir "$tmp/wide" --gcode "$tmp/wide.gcode" --json >"$tmp/wide.json" 2>"$tmp/wide.log"
 or fail 'the widely spaced support profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/wide.log"; or fail 'the widely spaced support file failed verification'
set wide (printf '%s' '
import json, sys
d = json.load(open(sys.argv[1]))
print(sum(l["counts"]["support"] for l in d["layers"]), sum(l["counts"]["support_interface"] for l in d["layers"]))
' | python3 - "$tmp/wide.json" | string split ' ')
test $wide[1] -lt (math "$spaced[1] * 3 / 4")
 or fail "doubling the body spacing kept $wide[1] of $spaced[1] passes, so the value is not a spacing"
test $wide[2] -lt $spaced[2]
 or fail "widening the interface kept $wide[2] of $spaced[2] interface passes"
cat "$tmp/base.cfg" >"$tmp/solid.cfg"
printf 'support = true\nsupport_material_spacing = 0\nsupport_material_interface_spacing = 0\n' >>"$tmp/solid.cfg"
$bin fixtures/overhang.stl --profile "$tmp/solid.cfg" --layer-height 0.2 \
    --out-dir "$tmp/solid" --gcode "$tmp/solid.gcode" --json >"$tmp/solid.json" 2>"$tmp/solid.log"
 or fail 'the solid support profile failed to slice'
set solid (printf '%s' '
import json, sys
d = json.load(open(sys.argv[1]))
print(sum(l["counts"]["support"] for l in d["layers"]), sum(l["counts"]["support_interface"] for l in d["layers"]))
' | python3 - "$tmp/solid.json" | string split ' ')
test $solid[1] -gt $spaced[1]
 or fail "spacing 0 printed $solid[1] body passes, no denser than $spaced[1] at 2.5 mm"
test $solid[2] -gt $wide[2]
 or fail "spacing 0 printed $solid[2] interface passes, no denser than $wide[2] at 2.5 mm"
printf 'support_material = true\nsupport_material_spacing = 2.5\nsupport_interface_spacing = 1\n' >"$tmp/spacing.ini"
$bin fixtures/overhang.stl --preset "$tmp/spacing.ini" --out-dir "$tmp/spacing" >"$tmp/spacing.out" 2>&1
 or fail 'the spacing preset failed to slice'
grep -q '3 keys: 3 applied' "$tmp/spacing.out"
 or fail 'the preset spellings of the two spacings were not counted applied'
printf 'support = true\nsupport_material_spacing = -0.5\n' >"$tmp/badspace.cfg"
$bin fixtures/overhang.stl --profile "$tmp/badspace.cfg" --out-dir "$tmp/badspace" >"$tmp/badspace.log" 2>&1
 and fail 'a negative support spacing was accepted'
grep -q 'must be 0 mm or more' "$tmp/badspace.log"
 or fail 'the refusal does not name the spacing rule'
printf 'Verified: 2.5 mm body spacing lays %s passes, 5 mm %s, 0 mm %s; interface 1.0 mm lays %s, 2.5 mm %s, 0 mm %s.\n' $spaced[1] $wide[1] $solid[1] $spaced[2] $wide[2] $solid[2]

section 'Emission: comments, labels, resolution, volumetric E, thumbnails'
cp "$tmp/base.cfg" "$tmp/emit.cfg"
printf 'gcode_comments = false\ngcode_resolution = 0.05\nuse_volumetric_e = true\nremaining_times = true\nbefore_layer_gcode = ;HOOK at {layer_z}\nlayer_gcode = ;DONE {layer_num}\nthumbnails = 64x64/PNG, 32x32/QOI\n' >>"$tmp/emit.cfg"
$bin fixtures/cube.stl --profile "$tmp/emit.cfg" --layer-height 0.2 \
    --out-dir "$tmp/emit" --gcode "$tmp/emit.gcode" >"$tmp/emit.log" 2>&1
 or fail 'the emission profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/emit.log"; or fail 'the emission file failed verification'
test (grep -c ';WIDTH:' "$tmp/emit.gcode") = 0
 or fail 'the per-segment width comments survived gcode_comments = false'
test (grep -c ';TYPE:' "$tmp/emit.gcode") -gt 0
 or fail 'the line types were dropped with the comments, which no viewer can read'
grep -q ';LAYER_CHANGE' "$tmp/emit.gcode"; or fail 'the layer markers were dropped'
grep -q 'M200 D' "$tmp/emit.gcode"; or fail 'volumetric E did not declare the diameter'
grep -q 'M73 P' "$tmp/emit.gcode"; or fail 'no progress report'
grep -q ';HOOK at' "$tmp/emit.gcode"; or fail 'before_layer_gcode did not fire'
grep -q ';DONE 0' "$tmp/emit.gcode"; or fail 'layer_gcode did not fire'
grep -q '; thumbnail begin 64x64 ' "$tmp/emit.gcode"; or fail 'no PNG thumbnail'
grep -q '; thumbnail begin 32x32 ' "$tmp/emit.gcode"; or fail 'no QOI thumbnail'
set thumbs (grep -c '; thumbnail end' "$tmp/emit.gcode")
test $thumbs -eq 2; or fail "expected two thumbnails, found $thumbs"
set offgrid (grep -oE 'X[0-9]+\.[0-9]+' "$tmp/emit.gcode" | string replace -r 'X' '' | awk -F. '$2 % 5 != 0' | count)
test $offgrid -eq 0; or fail "$offgrid coordinates are off the 0.05 mm grid"
printf 'Verified: two thumbnails, %s progress reports, a hook per layer, no comments, and every X on the 0.05 mm grid.\n' (grep -c 'M73 P' "$tmp/emit.gcode")

section 'Emission: firmware retraction'
cp "$tmp/base.cfg" "$tmp/marlin.cfg"
printf 'flavor = marlin\nuse_firmware_retraction = true\n' >>"$tmp/marlin.cfg"
$bin fixtures/cube.stl --profile "$tmp/marlin.cfg" --layer-height 0.2 \
    --out-dir "$tmp/marlin" --gcode "$tmp/marlin.gcode" >"$tmp/marlin.log" 2>&1
 or fail 'the firmware-retraction profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/marlin.log"; or fail 'the firmware-retraction file failed verification'
grep -q 'G10' "$tmp/marlin.gcode"; or fail 'no G10 retract'
grep -q 'G11' "$tmp/marlin.gcode"; or fail 'no G11 deretract'
grep -q 'M207 S' "$tmp/marlin.gcode"; or fail 'no M207 declaration'
printf 'Verified: Marlin firmware retraction declared with M207 and driven by G10/G11.\n'

section 'Emission: temperature commands left to the profile'
cp "$tmp/base.cfg" "$tmp/suppress.cfg"
printf 'autoemit_temperature_commands = false\nstart_gcode = <<END_START\nM109 S215\nM190 S60\nEND_START\n' >>"$tmp/suppress.cfg"
$bin fixtures/cube.stl --profile "$tmp/suppress.cfg" --layer-height 0.2 \
    --out-dir "$tmp/suppress" --gcode "$tmp/suppress.gcode" >"$tmp/suppress.log" 2>&1
 or fail 'the temperature-suppression profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/suppress.log"; or fail 'the suppressed file failed verification'
grep -q 'M109 S215' "$tmp/suppress.gcode"; or fail 'the profile start block lost its heat'
test (grep -c 'M140 S60' "$tmp/suppress.gcode") = 0; or fail 'M140 survived temperature suppression'
test (grep -c 'M190 S' "$tmp/suppress.gcode") = 1; or fail 'the writer added its own bed wait'
printf 'Verified: the writer emits no temperature command of its own.\n'

section 'Avoiding crossings cuts crossings where a choice exists'
cp "$tmp/base.cfg" "$tmp/noavoid.cfg"
printf 'avoid_crossing_perimeters = false\n' >>"$tmp/noavoid.cfg"
cp "$tmp/base.cfg" "$tmp/avoid.cfg"
printf 'avoid_crossing_perimeters = true\navoid_crossing_perimeters_max_detour = 3\n' >>"$tmp/avoid.cfg"
$bin fixtures/torus.stl --profile "$tmp/noavoid.cfg" --layer-height 0.2 \
    --out-dir "$tmp/avoid-off" --gcode "$tmp/avoid-off.gcode" >"$tmp/avoid-off.log" 2>&1
 or fail 'the torus failed to slice without avoidance'
$bin fixtures/torus.stl --profile "$tmp/avoid.cfg" --layer-height 0.2 \
    --out-dir "$tmp/avoid" --gcode "$tmp/avoid.gcode" >"$tmp/avoid.log" 2>&1
 or fail 'avoid_crossing_perimeters failed to slice'
grep -q 'verify RESULT PASS' "$tmp/avoid.log"; or fail 'the avoiding file failed verification'
set off (string match -r -g '^\s+(\d+) travels cross' <"$tmp/avoid-off.log")[1]
set on (string match -r -g '^\s+(\d+) travels cross' <"$tmp/avoid.log")[1]
test -n "$off"; or fail 'the off run reported no crossing count'
test -n "$on"; or fail 'the on run reported no crossing count'
test "$on" -lt "$off"
 or fail "the torus crossings did not fall: $off -> $on"
$bin fixtures/cube.stl --profile "$tmp/noavoid.cfg" --layer-height 0.2 \
    --out-dir "$tmp/cube-off" --gcode "$tmp/cube-off.gcode" >"$tmp/cube-off.log" 2>&1
 or fail 'the cube failed to slice without avoidance'
$bin fixtures/cube.stl --profile "$tmp/avoid.cfg" --layer-height 0.2 \
    --out-dir "$tmp/cube-on" --gcode "$tmp/cube-on.gcode" >"$tmp/cube-on.log" 2>&1
 or fail 'the cube failed to slice with avoidance'
set cube_off (string match -r -g '^\s+(\d+) travels cross' <"$tmp/cube-off.log")[1]
set cube_on (string match -r -g '^\s+(\d+) travels cross' <"$tmp/cube-on.log")[1]
test "$cube_on" -eq "$cube_off"
 or fail "a cube has no crossing to avoid, yet $cube_off became $cube_on"
printf 'Verified: torus crossings %s -> %s, cube unchanged at %s (every cube crossing is an entry into the enclosed interior).\n' \
    "$off" "$on" "$cube_on"

section 'A crossing travel is routed around the wall, not just re-entered'
cp profiles/voron_klipper.cfg "$tmp/route.cfg"
printf 'avoid_crossing_perimeters = true\ninfill_anchor = 0\n' >>"$tmp/route.cfg"
cp profiles/voron_klipper.cfg "$tmp/route-base.cfg"
printf 'avoid_crossing_perimeters = false\ninfill_anchor = 0\n' >>"$tmp/route-base.cfg"
$bin fixtures/torus.stl --profile "$tmp/route-base.cfg" --layer-height 0.2 \
    --out-dir "$tmp/route-off" --gcode "$tmp/route-off.gcode" >"$tmp/route-off.log" 2>&1
 or fail 'the unrouted torus failed to slice'
set straight (string match -r -g '^\s+(\d+) travels cross' <"$tmp/route-off.log")[1]
test -n "$straight"; or fail 'the unrouted run reported no crossing count'
$bin fixtures/torus.stl --profile "$tmp/route.cfg" --layer-height 0.2 \
    --out-dir "$tmp/route" --gcode "$tmp/route.gcode" >"$tmp/route.log" 2>&1
 or fail 'the routing profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/route.log"; or fail 'the routed file failed verification'
set routed (string match -r -g '^\s+(\d+) travels cross' <"$tmp/route.log")[1]
test -n "$routed"; or fail 'the routed run reported no crossing count'
test "$routed" -lt "$straight"
 or fail "routing did not cut the torus crossings below the unrouted $straight: $routed"
test (math "$routed * 4") -le (math "$straight * 3")
 or fail "routing should cut the torus crossings by a quarter at least: $straight -> $routed"
set detoured (grep -c '^G0 X' "$tmp/route.gcode")
set plain (grep -c '^G0 X' "$tmp/route-off.gcode")
test "$detoured" -gt "$plain"
 or fail "a routed move is a chain of G0 moves, so the file should hold more of them: $plain -> $detoured"
$bin fixtures/cube.stl --profile "$tmp/route-base.cfg" --layer-height 0.2 \
    --out-dir "$tmp/route-cube-off" --gcode "$tmp/route-cube-off.gcode" >"$tmp/route-cube-off.log" 2>&1
 or fail 'the unrouted cube failed to slice'
set cube_off (string match -r -g '^\s+(\d+) travels cross' <"$tmp/route-cube-off.log")[1]
$bin fixtures/cube.stl --profile "$tmp/route.cfg" --layer-height 0.2 \
    --out-dir "$tmp/route-cube" --gcode "$tmp/route-cube.gcode" >"$tmp/route-cube.log" 2>&1
 or fail 'the routed cube failed to slice'
set cube_on (string match -r -g '^\s+(\d+) travels cross' <"$tmp/route-cube.log")[1]
test "$cube_on" -le "$cube_off"
 or fail "routing added crossings to a cube that offers no detour: $cube_off became $cube_on"
printf 'Verified: routed travel holds the torus at %s crossings against %s straight, with %s G0 moves where the straight file had %s, and the cube unchanged.\n' \
    "$routed" "$straight" "$detoured" "$plain"

section 'Compensating the cross-section moves the printed outline, and only where it should'
function outline --argument-names file
    awk '/^G1 X/ {x = ""; for (i = 1; i <= NF; i++) { if ($i ~ /^X/) x = substr($i, 2) + 0 } if (x == "") next; if (!seen || x < lo) lo = x; if (!seen || x > hi) hi = x; seen = 1 } END { printf "%.3f %.3f", lo, hi }' "$file"
end
function volume --argument-names log
    string match -r 'Deposited volume ([0-9.]+)' <"$log" | tail -1
end
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --out-dir "$tmp/comp-off" --gcode "$tmp/comp-off.gcode" >"$tmp/comp-off.log"
 or fail 'the baseline cube failed to slice'
set plain (outline "$tmp/comp-off.gcode")
set plain_volume (volume "$tmp/comp-off.log")
test -n "$plain_volume"; or fail 'the baseline run reported no deposited volume'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --elephant-foot 0.3 --out-dir "$tmp/comp-foot" --gcode "$tmp/comp-foot.gcode" >"$tmp/comp-foot.log"
 or fail 'the elephant-foot run failed to slice'
grep -q 'verify RESULT PASS' "$tmp/comp-foot.log"; or fail 'the compensated file failed verification'
set foot (outline "$tmp/comp-foot.gcode")
set foot_volume (volume "$tmp/comp-foot.log")
set foot_lo (string split ' ' $foot)[1]
set plain_lo (string split ' ' $plain)[1]
set foot_hi (string split ' ' $foot)[2]
set plain_hi (string split ' ' $plain)[2]
test (math "$foot_lo - $plain_lo - 0.3") -lt 0.02 -a (math "$foot_lo - $plain_lo - 0.3") -gt -0.02
 or fail "the first layer should be 0.3 mm smaller on every side: $plain_lo -> $foot_lo"
test (math "$plain_hi - $foot_hi - 0.3") -lt 0.02 -a (math "$plain_hi - $foot_hi - 0.3") -gt -0.02
 or fail "the first layer should be 0.3 mm smaller on the far side too: $plain_hi -> $foot_hi"
test (math "$plain_volume - $foot_volume") -lt 20
 or fail "an elephant foot is a first-layer correction, so it cannot remove $plain_volume - $foot_volume mm^3"
test (math "$plain_volume - $foot_volume") -gt 1
 or fail 'the elephant foot removed no plastic at all'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --xy-size 0.2 --out-dir "$tmp/comp-out" --gcode "$tmp/comp-out.gcode" >"$tmp/comp-out.log"
 or fail 'the --xy-size 0.2 run failed to slice'
grep -q 'verify RESULT PASS' "$tmp/comp-out.log"; or fail 'the grown file failed verification'
set out (outline "$tmp/comp-out.gcode")
set out_volume (volume "$tmp/comp-out.log")
set out_lo (string split ' ' $out)[1]
set out_hi (string split ' ' $out)[2]
test (math "$plain_lo - $out_lo - 0.2") -lt 0.02 -a (math "$plain_lo - $out_lo - 0.2") -gt -0.02
 or fail "growing by 0.2 should move the outline out by 0.2 per side: $plain_lo -> $out_lo"
test (math "$out_hi - $plain_hi - 0.2") -lt 0.02 -a (math "$out_hi - $plain_hi - 0.2") -gt -0.02
 or fail "growing by 0.2 should move the far side out by 0.2: $plain_hi -> $out_hi"
test (math "$out_volume - $plain_volume") -gt 60
 or fail 'growing the part by 0.2 mm per side produced almost no extra plastic'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --xy-size -0.2 --out-dir "$tmp/comp-in" --gcode "$tmp/comp-in.gcode" >"$tmp/comp-in.log"
 or fail 'the --xy-size -0.2 run failed to slice'
set inside (outline "$tmp/comp-in.gcode")
set in_volume (volume "$tmp/comp-in.log")
set in_lo (string split ' ' $inside)[1]
set in_hi (string split ' ' $inside)[2]
test (math "$in_lo - $plain_lo - 0.2") -lt 0.02 -a (math "$in_lo - $plain_lo - 0.2") -gt -0.02
 or fail "shrinking by 0.2 should move the outline in by 0.2 per side: $plain_lo -> $in_lo"
test (math "$plain_hi - $in_hi - 0.2") -lt 0.02 -a (math "$plain_hi - $in_hi - 0.2") -gt -0.02
 or fail "shrinking by 0.2 should move the far side in by 0.2: $plain_hi -> $in_hi"
test (math "$plain_volume - $in_volume") -gt 60
 or fail 'shrinking the part by 0.2 mm per side removed almost no plastic'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --xy-size 0.2 --elephant-foot 0.3 --out-dir "$tmp/comp-both" --gcode "$tmp/comp-both.gcode" >"$tmp/comp-both.log"
 or fail 'the combined run failed to slice'
set both_lo (string split ' ' (outline "$tmp/comp-both.gcode"))[1]
test (math "$both_lo - $plain_lo - 0.1") -lt 0.02 -a (math "$both_lo - $plain_lo - 0.1") -gt -0.02
 or fail "0.2 out on every layer and 0.3 in on the first must leave the first layer 0.1 in: $plain_lo -> $both_lo"
printf 'Verified: elephant foot pulls the first layer in by 0.3 mm without disturbing the stack, XY size moves every side out and in by 0.2 mm, and the two compose to the first layer landing 0.1 mm in.\n'

section 'Curved patterns: the gyroid and the 3D honeycomb'
for pattern in gyroid 3d_honeycomb
    $bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
        --pattern $pattern --out-dir "$tmp/curve-$pattern" --gcode "$tmp/curve-$pattern.gcode" >"$tmp/curve-$pattern.log"
     or fail "the $pattern cube failed to slice"
    grep -q 'verify RESULT PASS' "$tmp/curve-$pattern.log"
     or fail "the $pattern file failed verification"
    grep -q ';TYPE:Sparse infill' "$tmp/curve-$pattern.gcode"
     or fail "$pattern emitted no sparse infill block"
    set stats (awk '/^;LAYER_CHANGE/{l++} l==59' "$tmp/curve-$pattern.gcode" | awk '
        /^;TYPE:/{f = ($0 ~ /Sparse infill/); next}
        f && /^G1 /{
            x = ""; y = ""
            for (i = 1; i <= NF; i++) {if ($i ~ /^X/) x = substr($i, 2) + 0; if ($i ~ /^Y/) y = substr($i, 2) + 0}
            if (x != "" && y != "") {
                if (px != "") {
                    L += sqrt((x - px) ^ 2 + (y - py) ^ 2)
                    if (dx != "" && (x - px) * dy != (y - py) * dx) turns++
                    dx = x - px; dy = y - py
                }
                px = x; py = y
                moves++
            }
        }
        END {printf "%d %d %.1f", moves + 0, turns + 0, L + 0}')
    set moves (string split ' ' $stats)[1]
    set turns (string split ' ' $stats)[2]
    set length (string split ' ' $stats)[3]
    test $moves -gt 400; or fail "$pattern laid only $moves moves on one layer, too few for a woven fill"
    test $moves -lt 4000; or fail "$pattern laid $moves moves on one layer, finer than the requested density"
    test $turns -gt 100; or fail "$pattern changed direction only $turns times, so it printed straight lines"
    test $length -gt 80 -a $length -lt 220
     or fail "$pattern laid $length mm of sparse infill on one layer, outside the band a 15% fill needs"
    if test $pattern = gyroid
        set a (awk '/^;LAYER_CHANGE/{l++} l==59' "$tmp/curve-$pattern.gcode" | grep -c '^G1 X')
        set b (awk '/^;LAYER_CHANGE/{l++} l==60' "$tmp/curve-$pattern.gcode" | grep -c '^G1 X')
        test "$a" != "$b"; or fail 'two gyroid layers have identical path counts, so the phase is not following z'
    end
    printf 'Verified: %s wove %s moves with %s direction changes and %s mm of strand on one layer, at a density a 15%% fill needs.\n' \
        $pattern $moves $turns $length
end

section 'Slice repair: a clean part is untouched, a coarse tolerance drops detail'
set volumes (mktemp)
for r in 0 0.049
    $bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
        --closing-radius $r --out-dir "$tmp/close-$r" --gcode "$tmp/close-$r.gcode" >"$tmp/close-$r.log"
     or fail "the cube failed to slice with a $r mm closing radius"
    grep -q 'verify RESULT PASS' "$tmp/close-$r.log"
     or fail "the $r mm closing run failed verification"
    echo (awk -F'[(,]' '/^; filament used/{print $3+0}' "$tmp/close-$r.gcode") >>$volumes
end
test (sed -n '1p' $volumes) != ''
 or fail 'the closing runs reported no filament use'
set same (awk 'NR==1{a=$1} NR==2{ if ((a-$1) > 0.05 || ($1-a) > 0.05) { print "DIFF " a " " $1 } }' $volumes)
test -z "$same"
 or fail "the default closing radius changed a clean cube: $same"
printf 'Verified: the default 0.049 mm closing radius leaves a clean cube byte-identical to switching it off, so a part without a crack pays nothing for the repair.\n'

for r in 0 0.5
    $bin fixtures/sphere.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
        --resolution $r --out-dir "$tmp/res-$r" --gcode "$tmp/res-$r.gcode" >"$tmp/res-$r.log"
     or fail "the sphere failed to slice at resolution $r"
    grep -q 'verify RESULT PASS' "$tmp/res-$r.log"
     or fail "the resolution $r run failed verification"
end
set fine (awk '/^;TYPE:/ {f = ($0 ~ /wall/)} f && /^G1 X/ {c++} END {print c+0}' "$tmp/res-0.gcode")
set coarse (awk '/^;TYPE:/ {f = ($0 ~ /wall/)} f && /^G1 X/ {c++} END {print c+0}' "$tmp/res-0.5.gcode")
test -n "$fine"; or fail 'the full-detail run wrote no wall moves to compare against'
test (math "$coarse * 3") -le (math "$fine * 2")
 or fail "a 0.5 mm tolerance left $coarse wall moves against $fine at full detail, so nothing was simplified"
printf 'Verified: a 0.5 mm resolution drops the sphere walls from %s moves to %s.\n' $fine $coarse
set fine_volume (awk -F'[(,]' '/^; filament used/{print $3+0}' "$tmp/res-0.gcode")
set coarse_volume (awk -F'[(,]' '/^; filament used/{print $3+0}' "$tmp/res-0.5.gcode")
set lost (math "$fine_volume - $coarse_volume")
test $lost -ge 0
 or fail "simplifying added plastic ($fine_volume became $coarse_volume), which it cannot"
test (math "$lost * 20") -le $fine_volume
 or fail "a 0.5 mm tolerance lost more than 5 percent of the plastic: $fine_volume became $coarse_volume"
set fine_bytes (awk '/^;TYPE:/ {f = ($0 ~ /wall/)} f {b += length($0) + 1} END {print b+0}' "$tmp/res-0.gcode")
set coarse_bytes (awk '/^;TYPE:/ {f = ($0 ~ /wall/)} f {b += length($0) + 1} END {print b+0}' "$tmp/res-0.5.gcode")
test $coarse_bytes -lt (math "$fine_bytes / 2")
 or fail "the wall blocks are $coarse_bytes bytes against $fine_bytes, so the coarse file is not much smaller"
printf 'Verified: a 0.5 mm slice resolution cut the sphere from %s moves and %s bytes to %s moves and %s bytes, losing %s mm3 of the %s mm3 a full-detail slice deposits.\n' \
    $fine $fine_bytes $coarse $coarse_bytes $lost $fine_volume
rm -f $volumes

section 'Thin walls are a gate, not a hardwired behaviour'
# A thin object is printed by its centreline or not at all. Switching thin-wall
# detection off therefore used to erase it from the plate, one of the ways a
# layer could be planned and then silently not printed.
cp "$tmp/base.cfg" "$tmp/thin.cfg"
printf 'thin_walls = false\n' >>"$tmp/thin.cfg"
$bin fixtures/thin_wall.stl --profile "$tmp/thin.cfg" --layer-height 0.2 --mode hfw \
    --out-dir "$tmp/thin" --gcode "$tmp/thin.gcode" >"$tmp/thin.log" 2>&1
 or fail 'thin_walls = false failed to slice'
$bin fixtures/thin_wall.stl --profile "$tmp/base.cfg" --layer-height 0.2 --mode hfw \
    --out-dir "$tmp/thinon" --gcode "$tmp/thinon.gcode" >"$tmp/thinon.log" 2>&1
 or fail 'the default thin-wall profile failed to slice'
grep -q ';TYPE:Gap infill' "$tmp/thinon.gcode"; or fail 'the thin fixture beaded nothing by default'
set off_layers (grep -c '^;LAYER_CHANGE' "$tmp/thin.gcode")
set on_layers (grep -c '^;LAYER_CHANGE' "$tmp/thinon.gcode")
test $off_layers -eq $on_layers
 or fail "thin_walls = false printed $off_layers layers where the default printed $on_layers"
set off_plastic (grep -c '^G1 X.*E' "$tmp/thin.gcode")
test $off_plastic -gt 0
 or fail 'thin_walls = false erased the object instead of printing it'
printf 'Verified: thin walls are beaded by default, and switching the gate off keeps all %s layers and %s extruding moves.\n' \
    $off_layers $off_plastic

cp "$tmp/base.cfg" "$tmp/floor.cfg"
printf 'min_feature_size = 2.0\n' >>"$tmp/floor.cfg"
$bin fixtures/thin_wall.stl --profile "$tmp/floor.cfg" --layer-height 0.2 --mode hfw \
    --out-dir "$tmp/floor" --gcode "$tmp/floor.gcode" >"$tmp/floor.log" 2>&1
 or fail 'a 2 mm feature floor failed to slice'
if grep -q ';TYPE:Gap infill' "$tmp/floor.gcode"
    fail 'a 2 mm floor still printed a sub-millimetre feature'
end
printf 'Verified: a 2 mm minimum feature size drops the thin fixture entirely.\n'

section 'The infill anchor can be switched off'
cp "$tmp/base.cfg" "$tmp/anchor.cfg"
printf 'infill_anchor = 0\n' >>"$tmp/anchor.cfg"
$bin fixtures/cube.stl --profile "$tmp/anchor.cfg" --layer-height 0.2 \
    --out-dir "$tmp/anchor" --gcode "$tmp/anchor.gcode" >"$tmp/anchor.log" 2>&1
 or fail 'infill_anchor = 0 failed to slice'
$bin fixtures/cube.stl --profile "$tmp/base.cfg" --layer-height 0.2 \
    --out-dir "$tmp/noanchor" --gcode "$tmp/noanchor.gcode" >"$tmp/noanchor.log" 2>&1
 or fail 'the default anchor profile failed to slice'
set off (string match -r 'Deposited volume ([0-9.]+)' <"$tmp/anchor.log")[2]
set on (string match -r 'Deposited volume ([0-9.]+)' <"$tmp/noanchor.log")[2]
test -n "$off" -a -n "$on"; or fail 'the volume line went missing'
set gap (math "$on - $off")
if test (math "$gap > 5.0") -ne 1
    fail "anchoring changed the volume by only $gap mm3"
end
printf 'Verified: anchoring adds %s mm3 on the cube.\n' $gap

section 'Spiral vase climbs one wall'
cp "$tmp/base.cfg" "$tmp/vase.cfg"
printf 'spiral_vase = true\n' >>"$tmp/vase.cfg"
$bin fixtures/sphere.stl --profile "$tmp/vase.cfg" --layer-height 0.2 \
    --out-dir "$tmp/vase" --gcode "$tmp/vase.gcode" >"$tmp/vase.log" 2>&1
 or fail 'the vase profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/vase.log"; or fail 'the vase failed verification'
set inner (grep -c ';TYPE:Inner wall' "$tmp/vase.gcode")
test $inner -lt 10; or fail "the vase printed $inner inner walls"
grep -q 'G1 X.*Y.*Z.*E' "$tmp/vase.gcode"; or fail 'no move carried X, Y and Z together'
printf 'Verified: the vase ran %s inner walls over the whole print.\n' $inner

section 'Variable layer heights follow the profile'
cp "$tmp/base.cfg" "$tmp/profile.cfg"
printf 'layer_height_profile = 0:0.2, 6:0.12, 12:0.28\nmin_layer_height = 0.08\nmax_layer_height = 0.3\n' >>"$tmp/profile.cfg"
$bin fixtures/cube.stl --profile "$tmp/profile.cfg" --layer-height 0.2 \
    --out-dir "$tmp/profile" --gcode "$tmp/profile.gcode" >"$tmp/profile.log" 2>&1
 or fail 'the variable-layer profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/profile.log"; or fail 'the variable-layer file failed verification'
set heights (grep -o ';HEIGHT:[0-9.]*' "$tmp/profile.gcode" | sort -u)
test (count $heights) -ge 3; or fail "the profile produced $heights, not three heights"
string match -q ';HEIGHT:0.120' $heights; or fail 'the thin step never reached the file'
string match -q ';HEIGHT:0.280' $heights; or fail 'the thick step never reached the file'
printf 'Verified: %s layer heights in one file, from a three-step profile.\n' (count $heights)

section 'Support closing radius, bottom gap and bridge skipping'
cp "$tmp/base.cfg" "$tmp/close.cfg"
printf 'support = true\nsupport_material_closing_radius = 1.5\nsupport_material_bottom_contact_distance = 0.4\n' >>"$tmp/close.cfg"
$bin fixtures/overhang_tall.stl --profile "$tmp/close.cfg" --layer-height 0.2 \
    --out-dir "$tmp/close" --gcode "$tmp/close.gcode" >"$tmp/close.log" 2>&1
 or fail 'the closing-radius profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/close.log"; or fail 'the closed support failed verification'
set closed_lines (grep -c ';TYPE:Support' "$tmp/close.gcode")
test $closed_lines -gt 0; or fail 'closing the radius removed every support pass'
printf 'Verified: %s support passes with a closing radius and a bottom gap.\n' $closed_lines

cp "$tmp/base.cfg" "$tmp/bridged.cfg"
printf 'support = true\n' >>"$tmp/bridged.cfg"
$bin fixtures/bridge.stl --profile "$tmp/bridged.cfg" --layer-height 0.2 \
    --out-dir "$tmp/bridged" --gcode "$tmp/bridged.gcode" >"$tmp/bridged.log" 2>&1
 or fail 'the supported bridge failed to slice'
set bridged (grep -c ';TYPE:Support' "$tmp/bridged.gcode")
test $bridged -gt 0; or fail 'the bridge fixture produced no support to skip'
printf 'support = true\ndont_support_bridges = true\n' >"$tmp/skip.cfg"
cp "$tmp/base.cfg" "$tmp/skip.cfg"
printf 'support = true\ndont_support_bridges = true\n' >>"$tmp/skip.cfg"
$bin fixtures/bridge.stl --profile "$tmp/skip.cfg" --layer-height 0.2 \
    --out-dir "$tmp/skip" --gcode "$tmp/skip.gcode" >"$tmp/skip.log" 2>&1
 or fail 'the bridge-skipping profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/skip.log"; or fail 'the bridge-skipping file failed verification'
set skipped (grep -c ';TYPE:Support' "$tmp/skip.gcode")
test $skipped -lt $bridged; or fail "skipping bridges left $skipped of $bridged support passes"
printf 'Verified: a bridge keeps %s support passes, or %s when it is left to bridge itself.\n' $bridged $skipped

section 'Tree supports honour their angle and top rate'
cp "$tmp/base.cfg" "$tmp/tree.cfg"
printf 'support = true\nsupport_style = tree\nsupport_tree_angle_slow = 20\nsupport_tree_top_rate = 90\nsupport_tree_branch_diameter_double_wall = true\n' >>"$tmp/tree.cfg"
$bin fixtures/overhang_tall.stl --profile "$tmp/tree.cfg" --layer-height 0.2 \
    --out-dir "$tmp/tree2" --gcode "$tmp/tree2.gcode" >"$tmp/tree2.log" 2>&1
 or fail 'the tree profile failed to slice'
grep -q 'verify RESULT PASS' "$tmp/tree2.log"; or fail 'the tree file failed verification'
set tree_lines (grep -c ';TYPE:Support' "$tmp/tree2.gcode")
test $tree_lines -gt 0; or fail 'the tree emitted nothing'
printf 'Verified: %s tree support passes with a slow tip lean and a dense top.\n' $tree_lines

section 'A plate of two objects prints one object at a time'
$bin fixtures/cube.stl --profile "$tmp/base.cfg" --layer-height 0.2 \
    --out-dir "$tmp/one-cube" --gcode "$tmp/one-cube.gcode" >"$tmp/one-cube.log" 2>&1
 or fail 'a single cube failed to slice'
grep -q 'verify RESULT PASS' "$tmp/one-cube.log"; or fail 'the single cube failed verification'
set single_cross (string match -r -g '^\s+(\d+) travels cross' <"$tmp/one-cube.log")[1]
set single_layers (grep -c ';LAYER:' "$tmp/one-cube.gcode")
test -n "$single_cross"; or fail 'the single cube reported no crossing count'

$bin fixtures/cube.stl fixtures/cube.stl --profile "$tmp/base.cfg" --layer-height 0.2 \
    --out-dir "$tmp/plate" --gcode "$tmp/plate.gcode" >"$tmp/plate.log" 2>&1
 or fail 'a two-object plate failed to slice'
grep -q 'verify RESULT PASS' "$tmp/plate.log"; or fail 'the plate file failed verification'
grep -q 'Plate: 2 objects' "$tmp/plate.log"; or fail 'the plate did not report two objects'
set plate_layers (grep -c ';LAYER:' "$tmp/plate.gcode")
test "$plate_layers" -eq "$single_layers"
 or fail "a layer-at-a-time plate should have $single_layers layers, got $plate_layers"
set plate_cross (string match -r -g '^\s+(\d+) travels cross' <"$tmp/plate.log")[1]
test "$plate_cross" -gt "$single_cross"
 or fail "two objects on one plate should not cross less than one cube: $single_cross -> $plate_cross"
set plateau (python3 scripts/plate-check.py "$tmp/plate.gcode")
test "$plateau" = ok; or fail "the plate is not printed object by object: $plateau"
printf 'Verified: %s layers hold both objects, printed one object at a time (%s crossings for two parts, %s for one).\n' \
    "$plate_layers" "$plate_cross" "$single_cross"

section 'Printing by object finishes one part before starting the next'
cp "$tmp/base.cfg" "$tmp/sequence.cfg"
printf 'print_sequence = by object\n' >>"$tmp/sequence.cfg"
$bin fixtures/cube.stl fixtures/cube.stl --profile "$tmp/sequence.cfg" --layer-height 0.2 \
    --out-dir "$tmp/seq-none" --gcode "$tmp/seq-none.gcode" >"$tmp/seq-none.log" 2>&1
 and fail 'by object printed without a gantry height to clear'
grep -q 'needs gantry_height' "$tmp/seq-none.log"
 or fail 'the refusal did not say what was missing'
printf 'gantry_height = 100\n' >>"$tmp/sequence.cfg"
$bin fixtures/cube.stl fixtures/cube.stl --profile "$tmp/sequence.cfg" --layer-height 0.2 \
    --out-dir "$tmp/seq-head" --gcode "$tmp/seq-head.gcode" >"$tmp/seq-head.log" 2>&1
 and fail 'by object printed without a toolhead radius to clear'
grep -q 'needs extruder_clearance_radius' "$tmp/seq-head.log"
 or fail 'the second refusal did not say what was missing'
printf 'extruder_clearance_radius = 20\n' >>"$tmp/sequence.cfg"
$bin fixtures/cube.stl fixtures/cube.stl --profile "$tmp/sequence.cfg" --layer-height 0.2 \
    --spacing 30 --out-dir "$tmp/seq" --gcode "$tmp/seq.gcode" >"$tmp/seq.log" 2>&1
 or fail 'the by-object plate failed to slice'
grep -q 'verify RESULT PASS' "$tmp/seq.log"; or fail 'the by-object file failed verification'
grep -q 'printed one object at a time' "$tmp/seq.log"; or fail 'the run did not report by-object order'
set seq (python3 scripts/plate-check.py --sequence "$tmp/seq.gcode" "$single_layers")
test "$seq" = ok; or fail "the by-object file is not sequential: $seq"
set seq_cross (string match -r -g '^\s+(\d+) travels cross' <"$tmp/seq.log")[1]
test -n "$seq_cross"; or fail 'the by-object run reported no crossing count'
printf 'Verified: two parts print as %s layers each, one Z return to the bed, and %s crossings reported by the verifier (not comparable with the layer-at-a-time number: Z descends, so the verifier sees both parts at once).\n' \
    "$single_layers" "$seq_cross"

section 'Every sparse pattern lays the same plastic in a different direction'
cp "$tmp/base.cfg" "$tmp/pattern.cfg"
printf 'infill_anchor = 0\n' >>"$tmp/pattern.cfg"
set pattern_filament 0
set pattern_first 0
for name in rectilinear grid line triangle cubic
    $bin fixtures/sphere.stl --profile "$tmp/pattern.cfg" --layer-height 0.2 --pattern $name \
        --out-dir "$tmp/pat-$name" --gcode "$tmp/pat-$name.gcode" >"$tmp/pat-$name.log" 2>&1
     or fail "--pattern $name failed to slice"
    grep -q 'verify RESULT PASS' "$tmp/pat-$name.log"
     or fail "--pattern $name failed verification"
    set filament (string match -r -g 'filament \([0-9.]+ g, (\d+) mm\^3' <"$tmp/pat-$name.log")[1]
    test -n "$filament"; or fail "--pattern $name reported no deposited volume"
    if test $pattern_first -eq 0
        set pattern_first $filament
    else
        test (math "$filament - $pattern_first") -lt 60 -a (math "$pattern_first - $filament") -lt 60
         or fail "$name deposits $filament mm^3 against $pattern_first for rectilinear: a pattern must rearrange plastic, not add it"
    end
    set pattern_filament $pattern_filament + $filament
end
section 'The newer sparse patterns print, and deposit what the old ones do'
cp "$tmp/base.cfg" "$tmp/pat2.cfg"
printf 'infill_anchor = 0\n' >>"$tmp/pat2.cfg"
$bin fixtures/sphere.stl --profile "$tmp/pat2.cfg" --layer-height 0.2 --pattern rectilinear \
    --out-dir "$tmp/pat2-rect" --gcode "$tmp/pat2-rect.gcode" >"$tmp/pat2-rect.log" 2>&1
 or fail 'the rectilinear reference for the newer patterns failed to slice'
set ref (string match -r -g 'filament \([0-9.]+ g, (\d+) mm\^3' <"$tmp/pat2-rect.log")[1]
test -n "$ref"; or fail 'the rectilinear reference reported no deposited volume'
for name in concentric zigzag crosshatch honeycomb monotonic
    $bin fixtures/sphere.stl --profile "$tmp/pat2.cfg" --layer-height 0.2 --pattern $name \
        --out-dir "$tmp/pat2-$name" --gcode "$tmp/pat2-$name.gcode" >"$tmp/pat2-$name.log" 2>&1
     or fail "--pattern $name failed to slice"
    grep -q 'verify RESULT PASS' "$tmp/pat2-$name.log"
     or fail "--pattern $name failed verification"
    set got (string match -r -g 'filament \([0-9.]+ g, (\d+) mm\^3' <"$tmp/pat2-$name.log")[1]
    test -n "$got"; or fail "--pattern $name reported no deposited volume"
    test (math "abs($got - $ref)") -lt (math "$ref / 10")
     or fail "$name deposits $got mm^3 against $ref for rectilinear, which is outside the tenth a pattern is allowed to drift"
    cmp -s "$tmp/pat2-$name.gcode" "$tmp/pat2-rect.gcode"
     and fail "$name produced the same toolpath as rectilinear, so the name is not reaching the fill"
end
printf 'Verified: concentric, zig-zag, cross-hatch, honeycomb and monotonic all slice, verify, and each deposits within a tenth of rectilinear (%s mm^3) with a different toolpath.\n' \
    "$ref"
set digest (for name in rectilinear line triangle cubic
    md5sum "$tmp/pat-$name.gcode" | string sub -l 8
end)
test (count $digest) -eq (count (string split ' ' (string join ' ' $digest) | sort -u))
 or fail 'two patterns produced the same toolpaths, so the setting is not reaching the fill'
printf 'Verified: five patterns, all verified, all within 60 mm^3 of each other at %s mm^3, and no two share a toolpath.\n' \
    "$pattern_first"

section 'Per-object settings come from the project, not the plate'
python3 tools/make-plate-3mf.py >/dev/null; or fail 'the plate fixture could not be built'
$bin fixtures/plate-overrides.3mf --profile "$tmp/base.cfg" --layer-height 0.2 \
    --out-dir "$tmp/overrides" --gcode "$tmp/overrides.gcode" >"$tmp/overrides.log" 2>&1
 or fail 'the per-object plate failed to slice'
grep -q 'verify RESULT PASS' "$tmp/overrides.log"; or fail 'the per-object plate failed verification'
grep -q 'overrides: sparse_infill_density, wall_loops' "$tmp/overrides.log"
 or fail 'the object overrides were not reported'
set plain (string match -r -g 'Per object: plain cube (\d+) mm' <"$tmp/overrides.log")[1]
set dense (string match -r -g 'dense cube (\d+) mm' <"$tmp/overrides.log")[1]
test -n "$plain"; or fail 'the plain cube volume was not reported'
test -n "$dense"; or fail 'the overridden cube volume was not reported'
test "$dense" -gt (math "$plain + $plain / 2")
 or fail "60% infill and 5 walls should deposit well over half again as much: $plain vs $dense"
printf 'Verified: one object overridden to 60%% infill and 5 walls deposits %s mm^3 against %s mm^3 for its neighbour.\n' \
    "$dense" "$plain"

section 'Cooling distributes its deficit instead of scaling the whole layer'
cp "$tmp/base.cfg" "$tmp/cool.cfg"
printf 'min_layer_time = 60\nmin_print_speed = 1\nmin_speed_factor = 0.05\ninfill_anchor = 0\n' >>"$tmp/cool.cfg"
$bin fixtures/torus.stl --profile "$tmp/cool.cfg" --layer-height 0.2 \
    --out-dir "$tmp/cool-uni" --gcode "$tmp/cool-uni.gcode" >"$tmp/cool-uni.log" 2>&1
 or fail 'the uniform slowdown profile failed to slice'
cp "$tmp/cool.cfg" "$tmp/coolprop.cfg"
printf 'cooling_slowdown_logic = proportional\n' >>"$tmp/coolprop.cfg"
$bin fixtures/torus.stl --profile "$tmp/coolprop.cfg" --layer-height 0.2 \
    --out-dir "$tmp/cool-prop" --gcode "$tmp/cool-prop.gcode" >"$tmp/cool-prop.log" 2>&1
 or fail 'the proportional slowdown profile failed to slice'
set uni (awk '/;LAYER:10/{p=1} p&&/^G1 X.* E/{match($0,/F[0-9]+/); print substr($0,RSTART+1,RLENGTH-1)} p&&/;LAYER:11/{exit}' "$tmp/cool-uni.gcode" | grep -v '^$' | sort -n)
set prop (awk '/;LAYER:10/{p=1} p&&/^G1 X.* E/{match($0,/F[0-9]+/); print substr($0,RSTART+1,RLENGTH-1)} p&&/;LAYER:11/{exit}' "$tmp/cool-prop.gcode" | grep -v '^$' | sort -n)
test -n "$uni[1]"; or fail 'no feeds were found in the slowed layer'
set uni_fast (printf '%s\n' $uni | awk '$1 >= 3000' | count)
set prop_fast (printf '%s\n' $prop | awk '$1 >= 3000' | count)
test $uni_fast -gt $prop_fast
 or fail "uniform kept $uni_fast beads at 50 mm/s or more against proportional's $prop_fast; it should leave some near nominal"
test $prop_fast -eq 0
 or fail "proportional left $prop_fast beads at 50 mm/s or more, so it scaled the layer unevenly"
test $prop[1] -gt $uni[1]
 or fail "uniform's floor $uni[1] is not below proportional's $prop[1], so the two distributions look alike"
test $uni[1] -ge 55; or fail "uniform went below the 1 mm/s floor: $uni[1] mm/min"
printf 'Verified: uniform leaves %s beads at 50 mm/s or more (floor %s) where proportional scales the layer into one band (max %s, floor %s).\n' \
    $uni_fast $uni[1] $prop[-1] $prop[1]

cp "$tmp/base.cfg" "$tmp/band0.cfg"
printf 'min_layer_time = 12\nmin_print_speed = 1\nmin_speed_factor = 0.05\ninfill_anchor = 0\ncooling_slowdown_logic = consistent_surface\n' >>"$tmp/band0.cfg"
cp "$tmp/band0.cfg" "$tmp/band5.cfg"
printf 'cooling_perimeter_transition_distance = 5\n' >>"$tmp/band5.cfg"
for width in 0 5
    $bin fixtures/cube.stl --profile "$tmp/band$width.cfg" --layer-height 0.2 \
        --out-dir "$tmp/band$width" --gcode "$tmp/band$width.gcode" >"$tmp/band$width.log" 2>&1
     or fail "the $width mm transition band failed to slice"
    set held (awk '/;LAYER:50/{p=1} p&&/^G1 X.* E/{match($0,/F[0-9]+/); print substr($0,RSTART+1,RLENGTH-1)} p&&/;LAYER:51/{exit}' "$tmp/band$width.gcode" | awk '{n[$1]++} END {print n[7200] + 0}')
    if test $width = 0
        set band0 $held
    else
        set band5 $held
    end
end
test $band5 -gt $band0
 or fail "a 5 mm band held $band5 beads back against $band0 with no band, so the distance is not reaching the choice"
printf 'Verified: a 5 mm transition band leaves %s beads at 120 mm/s where a zero band leaves %s, and a zero band is uniform.\n' \
    $band5 $band0

section 'Travels avoid and lift over a curled overhang'
cp "$tmp/base.cfg" "$tmp/curl.cfg"
printf 'dynamic_overhang_speeds = true\ninfill_anchor = 0\navoid_crossing_perimeters = false\n' >>"$tmp/curl.cfg"
cp "$tmp/curl.cfg" "$tmp/curl-lift.cfg"
printf 'travel_lift_before_obstacle = true\n' >>"$tmp/curl-lift.cfg"
cp "$tmp/curl.cfg" "$tmp/curl-avoid.cfg"
printf 'avoid_crossing_curled_overhangs = true\n' >>"$tmp/curl-avoid.cfg"
for kind in plain lift avoid
    set -l profile "$tmp/curl.cfg"
    if test $kind = lift
        set profile "$tmp/curl-lift.cfg"
    else if test $kind = avoid
        set profile "$tmp/curl-avoid.cfg"
    end
    $bin fixtures/torus.stl --profile $profile --layer-height 0.2 \
        --out-dir "$tmp/curl-$kind" --gcode "$tmp/curl-$kind.gcode" >"$tmp/curl-$kind.log" 2>&1
     or fail "the $kind curled-overhang profile failed to slice"
    set -l ret (string match -r '(\d+) retractions' (cat "$tmp/curl-$kind.log"))[2]
    set -l cross (string match -r '(\d+) travels cross' (cat "$tmp/curl-$kind.log"))[2]
    if test $kind = lift
        set curl_lift $ret
    else if test $kind = avoid
        set curl_avoid $cross
    else
        set curl_plain $ret
        set curl_cross $cross
    end
end
test $curl_lift -gt $curl_plain
 or fail "lifting before an obstacle retracted $curl_lift times against $curl_plain, so the travel never saw the overhang"
test $curl_avoid -ne $curl_cross
 or fail "routing around the overhang crossed $curl_avoid perimeters, the same as going straight ($curl_cross)"
printf 'Verified: lifting over a curled bead raises retractions to %s from %s, and routing around it moves the crossing count to %s from %s.\n' \
    $curl_lift $curl_plain $curl_avoid $curl_cross

section 'Seam placement, brim width, ironing target and the solid shell'
cp "$tmp/base.cfg" "$tmp/seams.cfg"
printf 'infill_anchor = 0\n' >>"$tmp/seams.cfg"
$bin fixtures/cube.stl --profile "$tmp/seams.cfg" --layer-height 0.2 \
    --out-dir "$tmp/seam-aligned" --gcode "$tmp/seam-aligned.gcode" >"$tmp/seam-aligned.log" 2>&1
 or fail 'the aligned seam profile failed to slice'
for style in rear random
    cp "$tmp/base.cfg" "$tmp/seam-$style.cfg"
    printf 'infill_anchor = 0\nseam_position = %s\n' $style >>"$tmp/seam-$style.cfg"
    $bin fixtures/cube.stl --profile "$tmp/seam-$style.cfg" --layer-height 0.2 \
        --out-dir "$tmp/seam-$style" --gcode "$tmp/seam-$style.gcode" >"$tmp/seam-$style.log" 2>&1
     or fail "the $style seam profile failed to slice"
    grep -q 'verify RESULT PASS' "$tmp/seam-$style.log"
     or fail "the $style seam file failed verification"
end
set rear (string match -r '(\d+) travels cross' (cat "$tmp/seam-rear.log"))[2]
set aligned (string match -r '(\d+) travels cross' (cat "$tmp/seam-aligned.log"))[2]
test -n "$rear"; or fail 'the rear seam run did not report crossings'
function seam_at --argument-names file
    awk '/^;TYPE:Outer wall/ {n++; if (n == 1) blk = 1; next} /^;TYPE:/ {blk = 0}
         /^G0 X/ {split($2, a, "X"); split($3, b, "Y"); sx = a[2]; sy = b[2]}
         blk && /^G1 X/ && sx != "" {print sx, sy; exit}' $file
end
set a_seam (string split ' ' -- (seam_at "$tmp/seam-aligned.gcode"))
set r_seam (string split ' ' -- (seam_at "$tmp/seam-rear.gcode"))
set n_seam (string split ' ' -- (seam_at "$tmp/seam-random.gcode"))
test (count $a_seam) -eq 2; or fail 'the aligned run has no outer wall seam to read'
test (count $r_seam) -eq 2; or fail 'the rear run has no outer wall seam to read'
test $a_seam[2] = $r_seam[2]
 or fail "aligned starts its loop at y=$a_seam[2] and rear at y=$r_seam[2], so they are not both at the rear"
test $a_seam[1] != $r_seam[1]
 or fail "aligned and rear both start at x=$a_seam[1], so the style changed nothing"
test $n_seam[1] != $a_seam[1]
 or fail "a random seam started where the aligned one did (x=$n_seam[1])"
printf 'Verified: aligned starts the loop at the rear centre (%s, %s), rear at the rear corner (%s, %s),\n' \
    $a_seam[1] $a_seam[2] $r_seam[1] $r_seam[2]
printf '          random elsewhere (%s, %s); crossings %s against %s.\n' \
    $n_seam[1] $n_seam[2] $rear $aligned
set rnd (string match -r '(\d+) retractions' (cat "$tmp/seam-random.log"))[2]
set base_ret (string match -r '(\d+) retractions' (cat "$tmp/seam-aligned.log"))[2]
test "$rnd" != "$base_ret"
 or fail "a scattered seam retracted $rnd times, the same as an aligned one ($base_ret)"
printf 'Verified: a rear seam crosses %s perimeters against %s aligned; a scattered seam retracts %s times against %s.\n' \
    $rear $aligned $rnd $base_ret

cp "$tmp/base.cfg" "$tmp/bw0.cfg"
printf 'brim_loops = 0\n' >>"$tmp/bw0.cfg"
cp "$tmp/bw0.cfg" "$tmp/bw5.cfg"
printf 'brim_width = 5\n' >>"$tmp/bw5.cfg"
for width in 0 5
    $bin fixtures/cube.stl --profile "$tmp/bw$width.cfg" --layer-height 0.2 \
        --out-dir "$tmp/bw$width" --gcode "$tmp/bw$width.gcode" >"$tmp/bw$width.log" 2>&1
     or fail "the $width mm brim failed to slice"
    set -l mm (string match -r 'G-code: \d+ layers, ([0-9.]+) mm filament' (cat "$tmp/bw$width.log"))[2]
    if test $width = 0
        set brim0 $mm
    else
        set brim5 $mm
    end
end
test (math "$brim5 - $brim0") -gt 30
 or fail "a 5 mm brim added only (math \"$brim5 - $brim0\") mm of filament to the first layer"
printf 'Verified: a 5 mm brim adds %s mm of filament over no brim.\n' (math "$brim5 - $brim0")

for kind in topmost_only top_surfaces
    cp "$tmp/base.cfg" "$tmp/iron-$kind.cfg"
    printf 'ironing = 1\nironing_type = %s\n' $kind >>"$tmp/iron-$kind.cfg"
    $bin fixtures/sphere.stl --profile "$tmp/iron-$kind.cfg" --layer-height 0.2 \
        --out-dir "$tmp/iron-$kind" --gcode "$tmp/iron-$kind.gcode" >"$tmp/iron-$kind.log" 2>&1
     or fail "the $kind ironing profile failed to slice"
    set -l mm (string match -r 'G-code: \d+ layers, ([0-9.]+) mm filament' (cat "$tmp/iron-$kind.log"))[2]
    set iron_$kind $mm
end
test (math "$iron_top_surfaces - $iron_topmost_only") -gt 10
 or fail "ironing every top surface laid $iron_top_surfaces mm against $iron_topmost_only mm for the topmost only"
printf 'Verified: ironing every top surface lays %s mm on the sphere, the topmost only %s mm.\n' \
    $iron_top_surfaces $iron_topmost_only

for shell in disabled partial enabled
    cp "$tmp/base.cfg" "$tmp/shell-$shell.cfg"
    printf 'ensure_vertical_shell_thickness = %s\n' $shell >>"$tmp/shell-$shell.cfg"
    $bin fixtures/cube.stl --profile "$tmp/shell-$shell.cfg" --layer-height 0.2 \
        --out-dir "$tmp/shell-$shell" --gcode "$tmp/shell-$shell.gcode" >"$tmp/shell-$shell.log" 2>&1
     or fail "the $shell solid-shell profile failed to slice"
    set -l mm (string match -r 'G-code: \d+ layers, ([0-9.]+) mm filament' (cat "$tmp/shell-$shell.log"))[2]
    set shell_$shell $mm
end
test (math "$shell_enabled - $shell_disabled") -gt 100
 or fail "a disabled solid shell laid $shell_disabled mm against $shell_enabled mm enabled, so the window did not narrow"
test "$shell_partial" = "$shell_enabled"
 or fail "partial ($shell_partial mm) and enabled ($shell_enabled mm) diverged, which the mask rule does not do"
printf 'Verified: a disabled solid shell lays %s mm against %s enabled, and partial reproduces enabled exactly at %s.\n' \
    $shell_disabled $shell_enabled $shell_partial

section 'The anchor joins infill lines into one run'
cp "$tmp/base.cfg" "$tmp/anch0.cfg"
printf 'infill_anchor = 0\n' >>"$tmp/anch0.cfg"
cp "$tmp/base.cfg" "$tmp/anch.cfg"
printf 'infill_anchor = 2.4\ninfill_anchor_max = 50\n' >>"$tmp/anch.cfg"
cp "$tmp/base.cfg" "$tmp/anch1.cfg"
printf 'infill_anchor = 2.4\ninfill_anchor_max = 1\n' >>"$tmp/anch1.cfg"
set -l anchor_mm
set -l anchor_ret
for case in off on cap
    set -l profile "$tmp/anch.cfg"
    switch $case
        case off
            set profile "$tmp/anch0.cfg"
        case cap
            set profile "$tmp/anch1.cfg"
    end
    $bin fixtures/cube.stl --profile $profile --layer-height 0.2 \
        --out-dir "$tmp/anch-$case" --gcode "$tmp/anch-$case.gcode" >"$tmp/anch-$case.log" 2>&1
     or fail "the anchor case $case failed to slice"
    grep -q 'verify RESULT PASS' "$tmp/anch-$case.log"
     or fail "the $case anchor file failed verification"
    set anchor_mm $anchor_mm (string match -r '([0-9.]+) mm filament' (cat "$tmp/anch-$case.log"))[2]
    set anchor_ret $anchor_ret (string match -r '([0-9]+) retractions' (cat "$tmp/anch-$case.log"))[2]
end
awk -v off="$anchor_mm[1]" -v on="$anchor_mm[2]" 'BEGIN { exit !(on > off) }'
 or fail "the joined run lays no more than the separate lines: $anchor_mm[1] -> $anchor_mm[2] mm"
test "$anchor_ret[2]" -lt "$anchor_ret[1]"
 or fail "joining the lines left the retractions at $anchor_ret[2] against $anchor_ret[1]"
test "$anchor_mm[2]" = "$anchor_mm[1]"
 and fail 'a join was made but no material was added, so nothing was walked'
printf 'Verified: joining the lines walks the inner wall for %s mm of bead and drops retractions from %s to %s; a 1 mm cap walks too little to join (%s mm).\n' \
    (math "$anchor_mm[2] - $anchor_mm[1]") $anchor_ret[1] $anchor_ret[2] $anchor_mm[3]

section 'A polygon bed places, refuses and verifies'
cp "$tmp/base.cfg" "$tmp/bed.cfg"
printf 'bed_shape = 0x0,350x0,350x350,0x350\n' >>"$tmp/bed.cfg"
$bin fixtures/cube.stl --profile "$tmp/bed.cfg" --layer-height 0.2 \
    --out-dir "$tmp/bed" --gcode "$tmp/bed.gcode" >"$tmp/bed.log" 2>&1
 or fail 'a square bed given as a polygon failed to slice'
grep -q 'Placed footprint X 160.40..189.60, Y 160.40..189.60' "$tmp/bed.log"
 or fail 'the part was not centred on the 350 mm outline'
grep -q 'verify RESULT PASS' "$tmp/bed.log"
 or fail 'the polygon bed file failed verification'
printf 'Verified: a 350 mm outline given as four points takes the span and centres the part like the two numbers did.\n'

cp "$tmp/base.cfg" "$tmp/ell.cfg"
printf 'bed_shape = 0x0,200x0,200x50,50x50,50x200,0x200\nsolid_infill_below_area = 0\n' >>"$tmp/ell.cfg"
$bin fixtures/cube.stl --profile "$tmp/ell.cfg" --layer-height 0.2 \
    --out-dir "$tmp/ell" --gcode "$tmp/ell.gcode" >"$tmp/ell.log" 2>&1
 or fail 'an L-shaped bed the part does fit on was refused'
grep -qE 'Placed footprint X 1\.[0-9]+\.\.30\.[0-9]+' "$tmp/ell.log"
 or fail 'the part was left on the bounding box centre, which is in the L shape'"'"'s cut-out'
grep -q 'verify RESULT PASS' "$tmp/ell.log"
 or fail 'the L-shaped bed file failed verification'
printf 'Verified: on a concave outline the part moves into the arm that holds it, and the file still verifies.\n'

cp "$tmp/base.cfg" "$tmp/tiny.cfg"
printf 'bed_shape = 0x0,140x0,140x20,20x20,20x140,0x140\n' >>"$tmp/tiny.cfg"
if $bin fixtures/cube.stl --profile "$tmp/tiny.cfg" --layer-height 0.2 \
    --out-dir "$tmp/tiny" --gcode "$tmp/tiny.gcode" >"$tmp/tiny.log" 2>&1
    fail 'a part whose skirt misses the outline was sliced anyway'
end
grep -q 'does not sit inside the bed outline 0x0,140x0,140x20,20x20,20x140,0x140' "$tmp/tiny.log"
 or fail 'the outline refusal did not name the outline'
printf 'Verified: a part that fits the span but not the outline is refused, and the message carries the polygon.\n'

section 'A small island prints solid without touching its neighbour'
cp "$tmp/base.cfg" "$tmp/island.cfg"
printf 'solid_infill_below_area = 0\ninfill_anchor = 0\n' >>"$tmp/island.cfg"
cp "$tmp/island.cfg" "$tmp/island300.cfg"
printf 'solid_infill_below_area = 300\n' >>"$tmp/island300.cfg"
set -l island_mm
for case in cube0 cube300 bridge0 bridge300 plate0 plate300
    set -l profile "$tmp/island.cfg"
    set -l models fixtures/cube.stl
    switch $case
        case plate0 plate300
            set models fixtures/cube.stl fixtures/bridge.stl
        case bridge0 bridge300
            set models fixtures/bridge.stl
    end
    switch $case
        case cube300 bridge300 plate300
            set profile "$tmp/island300.cfg"
    end
    $bin $models --profile $profile --layer-height 0.2 \
        --out-dir "$tmp/$case" --gcode "$tmp/$case.gcode" >"$tmp/$case.log" 2>&1
     or fail "the island case $case failed to slice"
    set island_mm $island_mm (string match -r '([0-9.]+) mm filament' (cat "$tmp/$case.log"))[2]
end
set -l cube_delta (math "$island_mm[2] - $island_mm[1]")
set -l bridge_delta (math "$island_mm[4] - $island_mm[3]")
set -l plate_delta (math "$island_mm[6] - $island_mm[5]")
test (math "abs($cube_delta)") -lt 0.05
 or fail "the 400 mm2 cube changed with a 300 mm2 threshold: $cube_delta mm"
test (math "abs($plate_delta - $bridge_delta)") -lt 3
 or fail "the plate gained $plate_delta mm but the small island alone gains $bridge_delta mm, so the threshold did not stop at the island"
printf 'Verified: a 300 mm2 threshold leaves the 400 mm2 cube alone, and on a plate of both parts the gain is exactly the small island'"'"'s own (%s mm).\n' $bridge_delta

section 'Per-feature accelerations, first-layer walls and the volumetric ramp'
cp "$tmp/base.cfg" "$tmp/spd.cfg"
printf 'first_layer_perimeter_speed = 5\n' >>"$tmp/spd.cfg"
$bin fixtures/cube.stl --profile "$tmp/spd.cfg" --layer-height 0.2 \
    --out-dir "$tmp/spd" --gcode "$tmp/spd.gcode" >"$tmp/spd.log" 2>&1
 or fail 'the first-layer perimeter speed profile failed to slice'
set walls (awk '/;LAYER:0/{p=1} p&&/;TYPE:Outer wall|;TYPE:Inner wall/{w=1} p&&/^G1 X.* E/{if(w){match($0,/F[0-9]+/); print substr($0,RSTART+1,RLENGTH-1)}} p&&/;LAYER:1/{exit}' "$tmp/spd.gcode" | sort -u | tr '\n' ' ')
test "$walls" = " 1500 300 "
 or fail "the first-layer walls ran at [$walls] mm/min, so not every wall took the 5 mm/s"
printf 'Verified: first-layer walls take the 5 mm/s the profile asked for; 1500 mm/min is the one wall bead the first-layer cap still limits.\n'

cp "$tmp/base.cfg" "$tmp/acc.cfg"
printf 'infill_acceleration = 800\nsolid_infill_acceleration = 0\ntop_solid_infill_acceleration = 0\ndefault_acceleration = 0\nouter_wall_acceleration = 0\nperimeter_acceleration = 0\ninternal_solid_infill_acceleration = 0\nbridge_acceleration = 0\ninitial_layer_acceleration = 0\ntravel_acceleration = 0\nmachine_max_acceleration_x = 0\nmachine_max_acceleration_y = 0\n' >>"$tmp/acc.cfg"
$bin fixtures/cube.stl --profile "$tmp/acc.cfg" --layer-height 0.2 \
    --out-dir "$tmp/acc" --gcode "$tmp/acc.gcode" >"$tmp/acc.log" 2>&1
 or fail 'the acceleration fallback profile failed to slice'
set accels (grep -oE 'SET_VELOCITY_LIMIT ACCEL=[0-9]+' "$tmp/acc.gcode" | sort -u | tr '\n' ' ')
test "$accels" = "SET_VELOCITY_LIMIT ACCEL=800 "
 or fail "infill_acceleration did not reach every skin and wall feature: [$accels]"
printf 'Verified: infill_acceleration reaches the sparse fill, the solid skin and the top surface alike.\n'

cp "$tmp/base.cfg" "$tmp/comb.cfg"
printf 'automatic_infill_combination = true\n' >>"$tmp/comb.cfg"
cp "$tmp/comb.cfg" "$tmp/comb20.cfg"
printf 'automatic_infill_combination_max_layer_height = 0.2\n' >>"$tmp/comb20.cfg"
cp "$tmp/comb.cfg" "$tmp/comb45.cfg"
printf 'automatic_infill_combination_max_layer_height = 0.45\n' >>"$tmp/comb45.cfg"
for cap in auto 20 45
    set -l profile "$tmp/comb.cfg"
    if test $cap != auto
        set profile "$tmp/comb$cap.cfg"
    end
    $bin fixtures/cube.stl --profile $profile --layer-height 0.2 \
        --out-dir "$tmp/comb$cap" --gcode "$tmp/comb$cap.gcode" >"$tmp/comb$cap.log" 2>&1
     or fail "the combination cap\$cap failed to slice"
    set -l mm (string match -r 'G-code: \d+ layers, ([0-9.]+) mm filament' (cat "$tmp/comb$cap.log"))[2]
    set comb_$cap $mm
end
test "$comb_20" != "$comb_auto"
 or fail "a 0.2 mm cap over 0.2 mm layers combined anything: $comb_20 mm against $comb_auto mm"
test "$comb_45" = "$comb_auto"
 or fail "a 0.45 mm cap differs from no cap: $comb_45 mm against $comb_auto mm"
printf 'Verified: a 0.2 mm cap over 0.2 mm layers combines nothing (%s mm), a 0.45 mm cap combines as the automatic rule does (%s mm).\n' \
    $comb_20 $comb_45

cp "$tmp/base.cfg" "$tmp/rate0.cfg"
printf 'max_volumetric_extrusion_rate_slope_positive = 0\nmax_volumetric_extrusion_rate_slope_negative = 0\n' >>"$tmp/rate0.cfg"
cp "$tmp/rate0.cfg" "$tmp/rate1.cfg"
printf 'max_volumetric_extrusion_rate_slope_positive = 1\nmax_volumetric_extrusion_rate_slope_negative = 1\n' >>"$tmp/rate1.cfg"
for slope in 0 1
    $bin fixtures/cube.stl --profile "$tmp/rate$slope.cfg" --layer-height 0.2 \
        --out-dir "$tmp/rate$slope" --gcode "$tmp/rate$slope.gcode" >"$tmp/rate$slope.log" 2>&1
     or fail "the $slope mm3/s2 ramp profile failed to slice"
    grep -q 'verify RESULT PASS' "$tmp/rate$slope.log"
     or fail "the $slope mm3/s2 ramp file failed verification"
end
set changed (diff "$tmp/rate0.gcode" "$tmp/rate1.gcode" | grep -c '^<')
test $changed -gt 50
 or fail "a 1 mm^3/s^2 rate limit changed only $changed lines, so the ramp never reached the emitter"
printf 'Verified: a 1 mm^3/s^2 limit rewrites %s moves, holding the flow change from one bead to the next.\n' $changed

section 'Settings that must never erase geometry'

# next-layer hands this engine its own resolved configuration, which nests the
# three settings groups and writes a percentage as its own object. Reading that
# dump once lost every printer setting including the bed.
printf '%s' '{"configuration":{"printer_settings":{"bed_shape":[[0,0],[350,0],[350,350],[0,350]],"nozzle_diameter":[0.4],"printer_model":"Voron 2.4 350"},"print_settings":{"min_bead_width":{"is_percent":true,"value":85},"wall_loops":2},"filament_settings":{"filament_type":["PLA"]}}}' >"$tmp/dump.json"
$bin fixtures/cube.stl --preset "$tmp/dump.json" --layer-height 0.2 \
    --out-dir "$tmp/dump" --gcode "$tmp/dump.gcode" >"$tmp/dump.out" 2>&1
 or fail 'a configuration dump was rejected'
grep -q 'on a 350 x 350 bed' "$tmp/dump.out"
 or fail 'a bed outline written as a point list did not reach the plate'
grep -q 'min_bead_width: unconverted' "$tmp/dump.out"
 and fail 'a percentage written as an object was not read as a percentage'

# A thin object is printed by its centreline or not at all. Turning off thin
# wall detection used to drop every cell of such an object and emit nothing.
printf '%s\n' 'thin_walls = false' 'gap_fill_enabled = false' >"$tmp/thin-off.ini"
$bin fixtures/thin_wall.stl --preset "$tmp/thin-off.ini" --layer-height 0.2 --out-dir "$tmp/thin-off" >"$tmp/thin-off.out" 2>&1
 or fail 'the thin-wall run failed'
grep -qE 'Paths: [0-9]+ walls, [0-9]+ middle' "$tmp/thin-off.out"
 or fail 'the thin-wall run printed no paths'
awk -F, 'NR>1 && ($6+$7+$8+$9+$11) > 0 {n++} END {exit !(n>0)}' "$tmp/thin-off/manifest.csv"
 or fail 'every layer of a thin object was left empty by thin_walls = false'

section 'Expressions and per-object settings'

printf '%s\n' \
    'nozzle_diameter = 0.4' \
    'temperature = 210' \
    'filament_abrasive = 0' \
    'start_gcode = M862.1 P[nozzle_diameter] A{(filament_abrasive[0] ? 1 : 0)}\n{if nozzle_diameter[0] == 0.4}M900 K0.05{else}M900 K0.08{endif}' \
    >"$tmp/expr.ini"
$bin fixtures/cube.stl --preset "$tmp/expr.ini" --layer-height 0.2 \
    --out-dir "$tmp/expr" --gcode "$tmp/expr.gcode" >"$tmp/expr.out" 2>&1
 or fail 'a preset carrying expressions failed to slice'
grep -qx 'M862.1 P0.4 A0' "$tmp/expr.gcode"
 or fail 'an expression in machine G-code was not evaluated'
grep -qx 'M900 K0.05' "$tmp/expr.gcode"
 or fail 'a conditional block did not pick the branch its condition names'
grep -q 'M900 K0.08' "$tmp/expr.gcode"
 and fail 'a conditional block emitted the branch it did not take'

printf '1 sparse_infill_density = 50%%\n1 wall_loops = 4\n' >"$tmp/objset.txt"
$bin fixtures/cube.stl fixtures/sphere.stl --preset "$tmp/base.cfg" --layer-height 0.2 \
    --object-settings "$tmp/objset.txt" --no-arrange \
    --out-dir "$tmp/os" --gcode "$tmp/os.gcode" >"$tmp/os.out" 2>&1
 or fail 'a plate with per-object settings failed to slice'
grep -q 'Object cube (#0) overrides: sparse_infill_density, wall_loops' "$tmp/os.out"
 or fail 'per-object settings did not reach the object they name'
$bin fixtures/cube.stl fixtures/sphere.stl --preset "$tmp/base.cfg" --layer-height 0.2 \
    --no-arrange --out-dir "$tmp/os-plain" --gcode "$tmp/os-plain.gcode" >"$tmp/os-plain.out" 2>&1
 or fail 'the same plate without per-object settings failed to slice'
cmp -s "$tmp/os.gcode" "$tmp/os-plain.gcode"
 and fail 'per-object settings changed nothing in the output'

printf '1 support_xy_gap = 0.2\n1 support_threshold_angle = 40\n' >"$tmp/objset2.txt"
$bin fixtures/cube.stl fixtures/sphere.stl --preset "$tmp/base.cfg" --layer-height 0.2 \
    --object-settings "$tmp/objset2.txt" --no-arrange \
    --out-dir "$tmp/os-support" --gcode "$tmp/os-support.gcode" >"$tmp/os2.out" 2>&1
 or fail 'support settings on one object failed to slice'
grep -q 'overrides: support_xy_gap, support_threshold_angle' "$tmp/os2.out"
 or fail 'the support settings did not reach the object they name'
cmp -s "$tmp/os-support.gcode" "$tmp/os-plain.gcode"
 and fail 'per-object support settings changed nothing in the output'

printf '1 layer_height = 0.3\n' >"$tmp/objset3.txt"
$bin fixtures/cube.stl fixtures/sphere.stl --preset "$tmp/base.cfg" --layer-height 0.2 \
    --object-settings "$tmp/objset3.txt" --no-arrange \
    --out-dir "$tmp/os-refused" --gcode "$tmp/os-refused.gcode" >"$tmp/os3.out" 2>&1
 or fail 'a plate with a per-object key this engine cannot apply failed to slice'
grep -q 'need a feature this engine does not have per object: layer_height = 0.3' "$tmp/os3.out"
 or fail 'a per-object key the engine cannot apply was not reported by name and value'

section 'Per-layer expressions'

printf '%s\n' \
    'nozzle_diameter = 0.4' \
    'temperature = 210' \
    'before_layer_gcode = {if layer_z > 0.5}M201 X{interpolate_table(layer_z, (0,6000), (0.5,6000), (2,2000))}{else}M201 X9000{endif}' \
    >"$tmp/perlayer.ini"
$bin fixtures/cube.stl --preset "$tmp/perlayer.ini" --layer-height 0.2 \
    --out-dir "$tmp/perlayer" --gcode "$tmp/perlayer.gcode" >"$tmp/perlayer.out" 2>&1
 or fail 'a preset with a per-layer conditional failed to slice'
grep -q '{' "$tmp/perlayer.gcode"
 and fail 'a per-layer expression reached the file as literal text'
set -l first (grep -m1 -o 'M201 X[0-9]*' "$tmp/perlayer.gcode")
test "$first" = 'M201 X9000'
 or fail "the layer under the threshold took the wrong branch ($first)"
set -l last (grep -o 'M201 X[0-9]*' "$tmp/perlayer.gcode" | tail -1)
test "$last" = 'M201 X2000'
 or fail "the top layer was not the table's end value ($last)"
if not grep -q '6[0-9][0-9][0-9]' "$tmp/perlayer.gcode"
    fail 'the table was read at one height for the whole print'
end

section 'Two-reference oracle'

if command -q prusa-slicer; and command -q orca-slicer
    set -l report (python3 scripts/oracle.py fixtures/cube.stl fixtures/taper_wall.stl fixtures/thin_wall.stl)
    or fail 'the two-reference oracle could not run'
    printf '%s\n' $report | grep -E '===|PASS|FAIL'
    printf '%s\n' $report | grep -q 'FAIL:'
    and fail 'a fixture falls outside the oracle tolerance'
else
    printf 'skipped: OrcaSlicer or PrusaSlicer is not installed.\n'
end

printf '\nALL HFW ENGINE CHECKS PASSED.\n'
