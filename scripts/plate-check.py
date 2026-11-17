#!/usr/bin/env python3
"""Check plate ordering in an emitted G-code file.

Two modes, both used by scripts/test-hfw.fish:

  plate-check.py PLATE.gcode
      A layer-at-a-time plate: every layer has to visit one object, finish it,
      and only then move to the next. Measured as the number of times extrusion
      X crosses the plate midpoint inside a layer.

  plate-check.py --sequence SEQ.gcode SINGLE_LAYERS
      A by-object plate: the object count doubles the layer count, Z returns to
      the bed exactly once per extra object, and each object climbs steadily.
"""

import re
import sys

EXTRUSION = re.compile(r"G1 X(-?[0-9.]+) Y-?[0-9.]+ E([0-9.]+)")
TYPE = re.compile(r";TYPE:([^\n]+)")

# Skirt and brim wrap the whole plate, so they visit every object on purpose.
PLATE_WIDE = {"Skirt", "Brim"}


def layers(text):
    return text.split(";LAYER:")[1:]


def extrusion_x(block):
    xs = []
    current = ""
    for line in block.splitlines():
        kind = TYPE.match(line.strip())
        if kind is not None:
            current = kind.group(1).strip()
            continue
        if current in PLATE_WIDE:
            continue
        match = EXTRUSION.match(line.strip())
        if match is not None:
            xs.append(float(match.group(1)))
    return xs


def check_grouped(path):
    text = open(path).read()
    blocks = layers(text)
    xs = [x for block in blocks for x in extrusion_x(block)]
    if not xs:
        return "no extrusion at all"
    mid = widest_gap_midpoint(xs)
    if mid is None:
        return "the plate has only one object to visit"
    total = 0
    for index, block in enumerate(blocks):
        side = None
        switches = 0
        for x in extrusion_x(block):
            here = x > mid
            if side is None:
                side = here
            elif here is not side:
                switches += 1
                side = here
        if switches > 2:
            return f"layer {index + 1} switches objects {switches} times"
        total += switches
    if total > 2 * len(blocks):
        return f"{total} object swaps across {len(blocks)} layers"
    return "ok"


def widest_gap_midpoint(xs):
    """Midpoint of the widest unprinted stretch in X.

    For a plate of parts laid out side by side that stretch is the gap between
    two parts, so the midpoint separates them. A single part with a wider hole
    would confuse it, which is why this is a test helper for a known plate.
    """
    ordered = sorted(set(round(x, 2) for x in xs))
    best = 0.0
    mid = None
    for index in range(1, len(ordered)):
        gap = ordered[index] - ordered[index - 1]
        if gap > best:
            best = gap
            mid = 0.5 * (ordered[index] + ordered[index - 1])
    return mid


def check_sequence(path, single_layers):
    text = open(path).read()
    zs = [float(z) for z in re.findall(r";Z:([0-9.]+)", text)]
    if len(zs) != 2 * single_layers:
        return f"{len(zs)} layers, expected {2 * single_layers}"
    drops = [index for index in range(1, len(zs)) if zs[index] < zs[index - 1] - 1e-9]
    if len(drops) != 1:
        return f"{len(drops)} Z drops, expected exactly one object switch"
    if drops[0] != single_layers:
        return f"the object switch happens at layer {drops[0] + 1}, not {single_layers + 1}"
    first = zs[:single_layers]
    second = zs[single_layers:]
    if any(first[i] <= first[i - 1] for i in range(1, len(first))):
        return "the first object does not climb steadily"
    if any(second[i] <= second[i - 1] for i in range(1, len(second))):
        return "the second object does not climb steadily"
    return "ok"


def main(argv):
    if "--sequence" in argv:
        args = [argument for argument in argv if argument != "--sequence"]
        if len(args) != 3:
            print("usage: plate-check.py --sequence SEQ.gcode SINGLE_LAYERS")
            return 2
        result = check_sequence(args[1], int(args[2]))
    else:
        if len(argv) != 2:
            print("usage: plate-check.py PLATE.gcode")
            return 2
        result = check_grouped(argv[1])
    print(result)
    return 0 if result == "ok" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
