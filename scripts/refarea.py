import re
import sys

from shapely.geometry import Polygon

NUM = r"(-?\d+\.?\d*)"


def ref_area(path):
    svg = open(path).read()
    rings = []
    for m in re.finditer(r'<path d="([^"]+)"([^>]*)>', svg):
        if "stroke-dasharray" not in m.group(2):
            continue
        for sub in re.split(r"M", m.group(1)):
            p = [(float(a), float(b)) for a, b in re.findall(NUM + r"[, ]\s*" + NUM, sub)]
            if len(p) >= 3:
                rings.append(Polygon(p).buffer(0))
    rings.sort(key=lambda r: -r.area)
    region = rings[0]
    for r in rings[1:]:
        region = region.symmetric_difference(r)
    return region.area, len(rings)


if __name__ == "__main__":
    area, count = ref_area(sys.argv[1])
    expected = float(sys.argv[2])
    tol = float(sys.argv[3]) if len(sys.argv) > 3 else 0.5
    print(
        f"file={sys.argv[1]} ref_area_mm2={area:.3f} rings={count} "
        f"expected={expected:.3f} ok={abs(area - expected) <= tol}"
    )
    sys.exit(0 if abs(area - expected) <= tol else 1)
