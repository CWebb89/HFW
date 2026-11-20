#!/usr/bin/env python3
"""Writes fixtures/bed_round.stl, a round bed for the bed-model checks.

A 300 mm disc centred on a 350 mm square plate, 5 mm thick: the corners of the
square are bed a rectangle concedes and a round machine does not, which is the
case `bed_custom_model` exists to catch. The disc is faceted at 96 segments so a
cross-section halfway up is one clean ring rather than a set of chords.
"""

import argparse
import math
import struct
from pathlib import Path

CENTRE = (175.0, 175.0)
RADIUS = 150.0
THICKNESS = 5.0
SEGMENTS = 96


def disc(z0, z1):
    ring = []
    for step in range(SEGMENTS):
        angle = 2.0 * math.pi * step / SEGMENTS
        ring.append(
            (
                CENTRE[0] + RADIUS * math.cos(angle),
                CENTRE[1] + RADIUS * math.sin(angle),
            )
        )
    triangles = []
    centre_bottom = (CENTRE[0], CENTRE[1], z0)
    centre_top = (CENTRE[0], CENTRE[1], z1)
    for step in range(SEGMENTS):
        a = ring[step]
        b = ring[(step + 1) % SEGMENTS]
        triangles.append((centre_bottom, (b[0], b[1], z0), (a[0], a[1], z0)))
        triangles.append((centre_top, (a[0], a[1], z1), (b[0], b[1], z1)))
        triangles.append(((a[0], a[1], z0), (b[0], b[1], z0), (b[0], b[1], z1)))
        triangles.append(((a[0], a[1], z0), (b[0], b[1], z1), (a[0], a[1], z1)))
    return triangles


def normal(a, b, c):
    u = [b[i] - a[i] for i in range(3)]
    w = [c[i] - a[i] for i in range(3)]
    n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]]
    length = math.sqrt(sum(component * component for component in n))
    if length == 0.0:
        return (0.0, 0.0, 0.0)
    return (n[0] / length, n[1] / length, n[2] / length)


def write_stl(path, triangles):
    with path.open("wb") as out:
        out.write(b"hfw round bed".ljust(80, b"\0"))
        out.write(struct.pack("<I", len(triangles)))
        for triangle in triangles:
            out.write(struct.pack("<3f", *normal(*triangle)))
            for vertex in triangle:
                out.write(struct.pack("<3f", *vertex))
            out.write(struct.pack("<H", 0))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "fixtures" / "bed_round.stl",
    )
    options = parser.parse_args()
    triangles = disc(0.0, THICKNESS)
    write_stl(options.out, triangles)
    print(f"wrote {options.out}: {len(triangles)} triangles, radius {RADIUS} mm at {CENTRE}")


if __name__ == "__main__":
    main()
