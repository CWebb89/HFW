#!/usr/bin/env python3
"""Writes an overhang coupon: a post carrying a plate that overhangs it.

The plate sits on a post narrower than the plate, so every layer of plate that is
not above the post has nothing under it. Two versions are useful:

* ``--span 0`` — the plate is flush with the post on one side and cantilevers on
  the other, which is the plain "needs support" case.
* ``--span N`` — a second post is added ``N`` mm away, turning the cantilever
  into a span the bridge pass may claim instead.

Output is a binary STL, the same convention as ``make_bridge_fixture.py``.
"""

import argparse
import struct

POST_W = 8.0
DEPTH = 20.0
POST_H = 12.0
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
    tris = []
    for a, b, c in faces:
        tris.append((v[a], v[b], v[c]))
    return tris


def normal(p, q, r):
    u = (q[0] - p[0], q[1] - p[1], q[2] - p[2])
    w = (r[0] - p[0], r[1] - p[1], r[2] - p[2])
    n = (u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0])
    s = (n[0] ** 2 + n[1] ** 2 + n[2] ** 2) ** 0.5 or 1.0
    return (n[0] / s, n[1] / s, n[2] / s)


def write(path, tris):
    with open(path, "wb") as f:
        f.write(b"overhang coupon".ljust(80, b"\0"))
        f.write(struct.pack("<I", len(tris)))
        for p, q, r in tris:
            f.write(struct.pack("<3f", *normal(p, q, r)))
            for v in (p, q, r):
                f.write(struct.pack("<3f", *v))
            f.write(b"\0\0")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--span", type=float, default=0.0, help="gap to a second post, mm")
    ap.add_argument("--plate", type=float, default=30.0, help="plate length, mm")
    ap.add_argument("--post", type=float, default=POST_H, help="post height, mm")
    ap.add_argument("--name", default="overhang")
    args = ap.parse_args()

    post_w = POST_W
    height = args.post
    tris = box(0.0, post_w, 0.0, DEPTH, 0.0, height)
    width = args.plate
    if args.span > 0.0:
        x = post_w + args.span
        tris += box(x, x + post_w, 0.0, DEPTH, 0.0, height)
        width = max(width, x + post_w)
    tris += box(0.0, width, 0.0, DEPTH, height, height + PLATE_H)
    path = f"fixtures/{args.name}.stl"
    write(path, tris)
    print(f"wrote {path} ({len(tris)} triangles, plate {width:g} mm over a {post_w:g} mm post")


if __name__ == "__main__":
    main()
