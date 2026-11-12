#!/usr/bin/env python3
"""Writes fixtures/bridge.stl, the coupon that exercises bridge detection.

Two legs carry a plate whose underside spans 12 mm of open air, so every layer
of the plate has a strip with nothing printed below it while both ends rest on
material. The legs and the plate do not share a Z range: layer k either cuts
two separate leg rectangles or one plate rectangle, never a union, which keeps
the cross-section a plain set of rings.
"""

import argparse
import struct
from pathlib import Path

LEG_W = 8.0
DEPTH = 12.0
LEG_H = 16.0
PLATE_H = 4.0


def box(x0, x1, y0, y1, z0, z1):
    v = [
        (x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0),
        (x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1),
    ]
    faces = [
        (0, 3, 2), (0, 2, 1),
        (4, 5, 6), (4, 6, 7),
        (0, 1, 5), (0, 5, 4),
        (1, 2, 6), (1, 6, 5),
        (2, 3, 7), (2, 7, 6),
        (3, 0, 4), (3, 4, 7),
    ]
    return [(v[a], v[b], v[c]) for a, b, c in faces]


def normal(a, b, c):
    u = [b[i] - a[i] for i in range(3)]
    w = [c[i] - a[i] for i in range(3)]
    n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]]
    d = (n[0] ** 2 + n[1] ** 2 + n[2] ** 2) ** 0.5 or 1.0
    return [x / d for x in n]


def ascii_stl(tris, name):
    out = [f"solid {name}"]
    for a, b, c in tris:
        n = normal(a, b, c)
        out.append(f"  facet normal {n[0]:.6e} {n[1]:.6e} {n[2]:.6e}")
        out.append("    outer loop")
        for v in (a, b, c):
            out.append(f"      vertex {v[0]:.6e} {v[1]:.6e} {v[2]:.6e}")
        out.append("    endloop")
        out.append("  endfacet")
    out.append(f"endsolid {name}")
    return "\n".join(out) + "\n"


def binary_stl(tris, name):
    head = name.encode().ljust(80, b"\0")
    body = b"".join(
        struct.pack(
            "<12fH",
            *normal(a, b, c),
            *a,
            *b,
            *c,
            0,
        )
        for a, b, c in tris
    )
    return head + struct.pack("<I", len(tris)) + body


def ladder(spans, leg_w, depth, leg_h, plate_h, end_w):
    """Legs between gaps of the given spans, decked by a plate.

    The last leg is pulled by the sum of every span's cooling contraction while
    an interior leg is pulled equally from both sides, so the end leg takes
    `end_w` and the rest take `leg_w`.
    """
    tris = []
    x = 0.0
    for span in spans:
        tris += box(x, x + leg_w, 0.0, depth, 0.0, leg_h)
        x += leg_w + span
    tris += box(x, x + end_w, 0.0, depth, 0.0, leg_h)
    width = x + end_w
    tris += box(0.0, width, 0.0, depth, leg_h, leg_h + plate_h)
    return tris


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--span", type=float, default=12.0, help="open span in mm")
    ap.add_argument(
        "--ladder",
        default="",
        help="comma-separated spans for a four-rung ladder instead of one span",
    )
    ap.add_argument("--name", default="bridge", help="fixture base name")
    ap.add_argument("--leg-w", type=float, default=LEG_W, help="leg width in mm")
    ap.add_argument(
        "--end-w",
        type=float,
        default=0.0,
        help="width of the last leg in mm (0 repeats --leg-w)",
    )
    ap.add_argument("--depth", type=float, default=DEPTH, help="model depth in mm")
    ap.add_argument("--ascii", action="store_true", help="write ASCII STL")
    args = ap.parse_args()
    if args.ladder:
        spans = [float(v) for v in args.ladder.split(",")]
        end_w = args.end_w if args.end_w > 0.0 else args.leg_w
        tris = ladder(spans, args.leg_w, args.depth, LEG_H, PLATE_H, end_w)
        shape = f"spans {', '.join(f'{s:g}' for s in spans)} mm, end leg {end_w:g} mm"
    else:
        span = args.span
        width = 2 * args.leg_w + span
        tris = []
        tris += box(0.0, args.leg_w, 0.0, args.depth, 0.0, LEG_H)
        tris += box(args.leg_w + span, width, 0.0, args.depth, 0.0, LEG_H)
        tris += box(0.0, width, 0.0, args.depth, LEG_H, LEG_H + PLATE_H)
        shape = f"span {span} mm"
    target = Path(__file__).resolve().parent.parent / "fixtures" / f"{args.name}.stl"
    if args.ascii:
        target.write_text(ascii_stl(tris, args.name))
    else:
        target.write_bytes(binary_stl(tris, args.name))
    print(f"wrote {target} ({len(tris)} triangles, {shape})")


if __name__ == "__main__":
    main()
