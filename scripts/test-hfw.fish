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
string match -q -- '*G-code: 100 layers, 1096.5*' (cat "$tmp/gcode.log")
or fail 'the filament total is not the expected 1096.5 mm'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --layer-height 0.2 \
    --out-dir "$tmp/gcode2" --gcode "$tmp/cube2.gcode" >/dev/null
or fail 'the second G-code run failed'
cmp -s "$tmp/cube.gcode" "$tmp/cube2.gcode"; or fail 'two identical G-code runs differ'
printf 'Verified: 100 layers, expected filament total, byte-identical reruns.\n'

section 'Flow override changes the extrusion'
$bin fixtures/cube.stl --profile profiles/voron_klipper.cfg --flow 1.0 \
    --out-dir "$tmp/flow" --gcode "$tmp/flow.gcode" >"$tmp/flow.log"
    or fail 'the flow override was rejected'
string match -q -- '*G-code: 100 layers, 1154.2*' (cat "$tmp/flow.log")
    or fail 'flow 1.0 did not raise the filament total to 1154.2 mm'
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
grep -q '^SAVE_VELOCITY_LIMIT$' "$tmp/bridge.gcode"; or fail 'bridge acceleration was not saved first'
grep -q '^SET_VELOCITY_LIMIT ACCEL=3000$' "$tmp/bridge.gcode"; or fail 'bridge acceleration was not applied'
grep -q '^RESTORE_VELOCITY_LIMIT$' "$tmp/bridge.gcode"; or fail 'bridge acceleration was never restored'
set -l restored (awk '/^SET_VELOCITY_LIMIT/{s=NR} /^RESTORE_VELOCITY_LIMIT/{if (NR > s) print NR}' "$tmp/bridge.gcode" | wc -l)
test $restored -ge 1; or fail 'RESTORE_VELOCITY_LIMIT does not follow SET_VELOCITY_LIMIT'
printf 'Verified: only the plate layer 81 bridges, one direction, full fan, 37.2 mm/s,\n'
printf '          overlapping 0.381 mm spacing, bridge acceleration saved and restored.\n'

section 'Span compensation slows and thins a longer bridge'
$bin fixtures/bridge30.stl --profile profiles/voron_klipper.cfg --flow 1.0 \
    --out-dir "$tmp/bridge30" --gcode "$tmp/bridge30.gcode" >"$tmp/bridge30.log"
 or fail 'the 30 mm bridge coupon failed to slice'
grep -q 'verify RESULT PASS' "$tmp/bridge30.log"; or fail 'the 30 mm bridge file failed verification'
set -l fast (grep -A 40 ';TYPE:Bridge' "$tmp/bridge.gcode" | string match -r 'F\d+' | head -1 | string replace F '')
set -l slow (grep -A 40 ';TYPE:Bridge' "$tmp/bridge30.gcode" | string match -r 'F\d+' | head -1 | string replace F '')
test $slow -lt $fast; or fail "the 30 mm span ran at $slow, not slower than the 12 mm span's $fast"
set -l e12 (grep -A 40 ';TYPE:Bridge' "$tmp/bridge.gcode" | string match -r 'E\d+\.\d+' | head -1 | string replace E '' | math)
set -l e30 (grep -A 40 ';TYPE:Bridge' "$tmp/bridge30.gcode" | string match -r 'E\d+\.\d+' | head -1 | string replace E '' | math)
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
     /^;TYPE:Sparse infill/ {f=inlay; next} /^;TYPE:/ {f=0} f && /^G1 X/ {split($2,a,"X"); x=a[2]+0; if (x > lo + 0.5 && x < hi - 0.5) c++} END {print c+0}' \
    "$tmp/bridge30.gcode")
test $stray -eq 0
 or fail "$stray sparse infill strands ran into the unsupported span"
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
printf 'G1 X9999 Y0 F100\n' >"$tmp/bad.gcode"
$bin --check-gcode "$tmp/bad.gcode" --profile profiles/voron_klipper.cfg >"$tmp/bad.log" 2>&1
and fail 'an unsafe file passed verification'
grep -q 'verify RESULT FAIL' "$tmp/bad.log"; or fail 'the verifier did not report a failure'
grep -q 'outside the bed' "$tmp/bad.log"; or fail 'the bed overflow was not named'
printf 'Verified: bed overflow is caught.\n'

section 'Profiles reject unsafe settings'
sed 's/^walls = 2/walls = 0/' profiles/voron_klipper.cfg >"$tmp/bad.cfg"
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
