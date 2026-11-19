#!/usr/bin/env python3
"""Measures wall coverage from a sliced layer's own SVG.

The band inside the surface is the region the walls are supposed to fill. Beads
are stroked from the SVG at their own stroke width, so what the slicer claims to
deposit is measured rather than inferred from the model: uncovered area is band
minus beads, and beads outside the surface are beads minus the region.

Usage:
    scripts/wallcov.py layer.svg [band_mm]
"""

import re
import sys

import numpy as np
from shapely.geometry import LineString, Polygon
from shapely.ops import unary_union

NUM = r"(-?\d+\.?\d*)"


def read(path):
    svg = open(path).read()
    rings = []
    segs = []
    for m in re.finditer(r'<path d="([^"]+)"([^>]*)>', svg):
        d, attrs = m.group(1), m.group(2)
        if "stroke-dasharray" in attrs:
            for sub in re.split(r"M", d):
                p = [(float(a), float(b)) for a, b in re.findall(NUM + r"[, ]\s*" + NUM, sub)]
                if len(p) >= 3:
                    rings.append(Polygon(p).buffer(0))
        else:
            w = float(re.search(r'stroke-width="([\d.]+)"', attrs).group(1))
            p = [(float(a), float(b)) for a, b in re.findall(NUM + r"[, ]\s*" + NUM, d)]
            if len(p) >= 2:
                segs.append((p, w))
    return rings, segs


def main(path, band_mm):
    rings, segs = read(path)
    rings.sort(key=lambda r: -r.area)
    region = rings[0]
    for r in rings[1:]:
        region = region.symmetric_difference(r)
    beads = unary_union([LineString(p).buffer(w / 2, cap_style=1, resolution=8) for p, w in segs])
    band = region.difference(region.buffer(-band_mm))
    uncovered = band.difference(beads)
    outside = beads.difference(region.buffer(0.02))
    centrelines = unary_union([LineString(p) for p, _ in segs])
    boundary = region.boundary
    d = np.array([centrelines.distance(boundary.interpolate(t, normalized=True)) for t in np.linspace(0, 1, 4000)])
    parts = list(uncovered.geoms) if hasattr(uncovered, "geoms") else ([] if uncovered.is_empty else [uncovered])
    big = sorted((g.area for g in parts), reverse=True)[:3]
    print(f"file={path} band_area={band.area:.2f} uncovered_mm2={uncovered.area:.3f} uncovered_pct={100 * uncovered.area / band.area:.2f} outside_surface_mm2={outside.area:.3f}")
    print(f"surface_to_centreline_mm min={d.min():.3f} mean={d.mean():.3f} max={d.max():.3f} ideal=0.200")
    print(f"largest_uncovered_blobs_mm2={[round(a, 4) for a in big]} blobs_over_0.005={sum(g.area > 0.005 for g in parts)}")


if __name__ == "__main__":
    main(sys.argv[1], float(sys.argv[2]) if len(sys.argv) > 2 else 1.2)
