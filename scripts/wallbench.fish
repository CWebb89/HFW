#!/usr/bin/env fish
# Wall coverage and wall speed, hfw against classic, on one geometry.
#
# Usage: ./scripts/wallbench.fish [--tag <name>]
#   HFW      slicer under test (default target/release/hfw-cli)
#   HFW_PY   interpreter holding shapely, trimesh, numpy, cairosvg (default python3)
#
# Coverage is measured on the stress gear's single mid-height layer; speed is
# measured on the whole 30 mm stack at one thread, so the number does not depend
# on how the machine's cores happen to be loaded.

set -g root (realpath (string join / (dirname (status filename)) ..))
set -g py $HFW_PY
test -n "$py"; or set py python3
set -g hfw $HFW
test -n "$hfw"; or set hfw $root/target/release/hfw-cli
set -g tag ''
if test (count $argv) -ge 2 -a "$argv[1]" = '--tag'
    set tag $argv[2]
end
set -g out /tmp/wallbench$tag
mkdir -p $out

function die --argument-names message
    printf 'FAIL: %s\n' $message >&2
    exit 1
end

cd $root; or die "cannot enter $root"

$py scripts/make_stress_part.py 6 $out/g6.stl; or die 'cannot build the 6 mm gear'
$py scripts/make_stress_part.py 30 $out/g30.stl; or die 'cannot build the 30 mm gear'

printf '\n== coverage, one 3 mm layer ==\n'
for mode in hfw classic
    $hfw $out/g6.stl --z 3 --mode $mode --no-infill --svg $out/g6_$mode.svg >$out/g6_$mode.log
    or die "slicing $mode failed"
    $py scripts/wallcov.py $out/g6_$mode.svg; or die "coverage of $mode failed"
end

printf '\n== speed, 150 layers, one thread ==\n'
for mode in hfw classic
    set -l t0 (date +%s.%N)
    HFW_THREADS=1 $hfw $out/g30.stl --profile profiles/voron_klipper.cfg --mode $mode \
        --gcode $out/g30_$mode.gcode --out-dir $out/g30_$mode >$out/g30_$mode.log
    or die "slicing the 30 mm gear in $mode failed"
    set -l t1 (date +%s.%N)
    printf '%-8s total=%.2fs\n' $mode (math "$t1 - $t0")
    grep '^Timing\|^Wall generation' $out/g30_$mode/job_report.txt
end

printf '\n== reference ==\n'
if type -q prusa-slicer
    set -l ini $out/voron350.ini
    prusa-slicer --load $root/profiles/voron_klipper.cfg --export-ini $ini >/dev/null 2>&1
    set -l t0 (date +%s.%N)
    prusa-slicer --load $ini --export-gcode --output $out/g30_prusa.gcode $out/g30.stl \
        >$out/g30_prusa.log 2>&1
    set -l t1 (date +%s.%N)
    if test -s $out/g30_prusa.gcode
        printf 'prusa    total=%.2fs\n' (math "$t1 - $t0")
    else
        printf 'prusa-slicer is installed but the profile does not load; no reference\n'
    end
else
    printf 'prusa-slicer is not installed; no reference wall clock\n'
end
