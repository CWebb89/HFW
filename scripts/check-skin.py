#!/usr/bin/env python3
"""Check the skin, the first-layer width and the print order in one file.

The gate calls this with the G-code a preset produced and expects silence: every
condition either holds or the process exits non-zero with the reason, which is
what a gate section reports.
"""

from __future__ import annotations

import math
import sys

REASON = "the skin, the order or the first-layer width did not reach the file"
WIDEST_FIRST_LAYER = 0.045
MIN_LAYER_MOVES = 40
LONG_MOVE = 5.0


def layer_body(text: str, index: int) -> str:
    marker = f";LAYER:{index}\n"
    start = text.index(marker)
    return text[start + len(marker) : text.index(";LAYER:", start + len(marker))]


def main() -> int:
    text = open(sys.argv[1], encoding="utf-8").read()
    body = layer_body(text, 0)
    px, py, widest = 0.0, 0.0, 0.0
    for line in body.splitlines():
        if not line.startswith("G1 "):
            continue
        fields = {word[0]: word[1:] for word in line.split()[1:]}
        x = float(fields["X"]) if "X" in fields else px
        y = float(fields["Y"]) if "Y" in fields else py
        length = math.hypot(x - px, y - py)
        if "E" in fields and float(fields["E"]) > 0.0 and length > LONG_MOVE:
            widest = max(widest, float(fields["E"]) / length)
        px, py = x, y
    if widest < WIDEST_FIRST_LAYER:
        raise SystemExit(f"the widest first-layer bead was {widest:.4f} mm per mm")

    mid = layer_body(text, 40)
    types = [line for line in mid.splitlines() if line.startswith(";TYPE:")]
    if not types or "wall" in types[0].lower():
        raise SystemExit(f"infill_first put a wall first: {types[:3]}")
    moves = [line for line in mid.splitlines() if line.startswith("G1")]
    if len(moves) < MIN_LAYER_MOVES:
        raise SystemExit(f"layer 40 only carried {len(moves)} moves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
