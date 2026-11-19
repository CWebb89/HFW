#!/usr/bin/env python3
"""Generates the stress part: a 14-tooth gear with a thin rib and a pinhole.

The gear body exercises long curved walls and shallow tooth flanks, the two bars
on the X axis exercise the join between a wide region and a narrow one, the
vertical slot is a 2.4 mm rib standing on its own, and the pinhole gives the
raster generator a feature it cannot express.

Usage:
    scripts/make_stress_part.py [height_mm] [output.stl]
"""

import sys

import numpy as np
import trimesh
from shapely.geometry import Polygon, Point, box

height = float(sys.argv[1]) if len(sys.argv) > 1 else 6.0
out = sys.argv[2] if len(sys.argv) > 2 else "fixtures/stress_gear.stl"

pts = []
n = 14
for i in range(n):
    a = 2 * np.pi * i / n
    for r, da in ((18, 0), (18, 0.12), (23, 0.2), (23, 0.34), (18, 0.42)):
        t = a + da * 4 * np.pi / n
        pts.append((r * np.cos(t), r * np.sin(t)))

part = Polygon(pts).buffer(0).union(box(-30, -0.25, -17, 0.25)).union(box(17, -3, 31, 3))
part = part.difference(Point(0, 0).buffer(5, resolution=64))
part = part.difference(box(-1.2, 8, 1.2, 14))
part = part.difference(Point(10, 0).buffer(0.9, resolution=32))
if part.geom_type == "MultiPolygon":
    part = max(part.geoms, key=lambda g: g.area)
trimesh.creation.extrude_polygon(part, height).export(out)
