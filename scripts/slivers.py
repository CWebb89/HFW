#!/usr/bin/env python3
"""Gap-strand statistics from a slice's own JSON.

The gap pass is what fills the strip the wall loops leave behind, and on a curved
or tapered part it also proposes strands nobody will print: a short stub at a
corner, or a strip that hugs the outermost wall where the section is fractionally
wider than a whole number of beads. Those are the slivers the references remove
before they reach the G-code.

Usage:
    hfw model.stl --layer-height 3 --json --json-paths --out-dir out \\
        | scripts/slivers.py [--near 0.6] [--short 1.5]

Reports, for every gap strand: its length, its width, and how far its own points
sit from the nearest outer-wall path of the same layer.
"""

import argparse
import json
import os
import sys

import numpy as np


def points_of(path):
    return np.asarray(path["points"], dtype=float).reshape(-1, 2)


def length_of(points, closed):
    if len(points) < 2:
        return 0.0
    total = float(np.linalg.norm(np.diff(points, axis=0), axis=1).sum())
    if closed and len(points) > 2:
        total += float(np.linalg.norm(points[0] - points[-1]))
    return total


def distance_to(points, rings):
    if not rings:
        return float("inf")
    best = float("inf")
    for ring in rings:
        a = ring[:-1]
        b = ring[1:]
        ab = b - a
        den = (ab * ab).sum(axis=1)
        den[den == 0.0] = 1e-12
        ap = points[:, None, :] - a[None, :, :]
        t = np.clip((ap * ab[None, :, :]).sum(axis=2) / den[None, :], 0.0, 1.0)
        proj = a[None, :, :] + t[:, :, None] * ab[None, :, :]
        d = np.linalg.norm(points[:, None, :] - proj, axis=2).min()
        best = min(best, float(d))
    return best


def stats(doc, near, short):
    lengths = []
    widths = []
    offsets = []
    for layer in doc.get("layers", []):
        geometry = layer.get("geometry") or []
        walls = [
            points_of(p)
            for p in geometry
            if p.get("feature") == "outer_wall" and len(p.get("points", [])) >= 3
        ]
        for path in geometry:
            if path.get("feature") != "gap_fill":
                continue
            points = points_of(path)
            run = length_of(points, path.get("closed", False))
            lengths.append(run)
            widths.append(float(np.mean(path["width"])))
            offsets.append(distance_to(points, walls))
    if not lengths:
        print("strands=0")
        return
    lengths = np.asarray(lengths)
    widths = np.asarray(widths)
    offsets = np.asarray(offsets)
    slivers = lengths < short
    print(
        "strands={} length_mm mean={:.2f} median={:.2f} max={:.2f} "
        "width_mm mean={:.2f} shortest={:.2f}".format(
            len(lengths),
            lengths.mean(),
            np.median(lengths),
            lengths.max(),
            widths.mean(),
            lengths.min(),
        )
    )
    print(
        "shorter_than_{}mm={} share={:.1f}% their_length_share={:.1f}%".format(
            short,
            int(slivers.sum()),
            100.0 * slivers.sum() / len(lengths),
            100.0 * lengths[slivers].sum() / lengths.sum(),
        )
    )
    close = offsets < near
    print(
        "within_{}mm_of_outer_wall={} share={:.1f}% {}".format(
            near,
            int(close.sum()),
            100.0 * close.sum() / len(lengths),
            "of_which_short={} short_share={:.1f}%".format(
                int((slivers & close).sum()),
                100.0 * (slivers & close).sum() / max(int(close.sum()), 1),
            ),
        )
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", help="JSON file; stdin when omitted")
    parser.add_argument("--near", type=float, default=0.6)
    parser.add_argument("--short", type=float, default=1.5)
    args = parser.parse_args()
    if args.path:
        doc = json.load(open(args.path))
        print("file={}".format(os.path.basename(args.path)))
    else:
        doc = json.load(sys.stdin)
    stats(doc, args.near, args.short)


if __name__ == "__main__":
    main()
