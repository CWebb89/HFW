#!/usr/bin/env python3
"""Builds `fixtures/cube.3mf` from `fixtures/cube.stl`.

The fixture exists so the 3MF path is exercised through a real ZIP archive with
real deflate streams, not just through in-memory XML: a reader that only works on
text is not a reader.

    tools/make-3mf-fixture.py
"""

from __future__ import annotations

import struct
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "fixtures/cube.stl"
TARGET = ROOT / "fixtures/cube.3mf"
PROJECT = ROOT / "fixtures/cube-project.3mf"

PROJECT_CONFIG = """# written by the fixture tool, in PrusaSlicer's project format";
name = Fixture Project Quality
perimeters = 3
fill_density = 20%
layer_height = 0.3
infill_extrusion_width = 0.55
support_material = 0
"""

PROJECT_MODEL = """<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1">
    <metadata type="object" key="name" value="cube"/>
  </object>
</config>
"""

CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>
"""

RELS = """<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"""


def stl_triangles(path: Path) -> list[tuple[tuple[float, float, float], ...]]:
    data = path.read_bytes()
    header = data[:80]
    if header.lstrip().startswith(b"solid") and b"facet" in data[:2048]:
        raise SystemExit("ASCII STL not handled: the fixture is binary")
    (count,) = struct.unpack("<I", data[80:84])
    triangles = []
    for index in range(count):
        base = 84 + index * 50
        values = struct.unpack("<12fH", data[base : base + 50])
        points = tuple(tuple(values[3 + 3 * k : 6 + 3 * k]) for k in range(3))
        triangles.append(points)
    return triangles


def model_xml(triangles: list[tuple[tuple[float, float, float], ...]]) -> str:
    vertices: list[tuple[float, float, float]] = []
    ids: dict[tuple[float, float, float], int] = {}
    faces: list[tuple[int, int, int]] = []
    for triangle in triangles:
        face = []
        for point in triangle:
            if point not in ids:
                ids[point] = len(vertices)
                vertices.append(point)
            face.append(ids[point])
        faces.append((face[0], face[1], face[2]))
    lines = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<model unit="millimeter" xml:lang="en-US" '
        'xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">',
        "  <resources>",
        '    <object id="1" type="model">',
        "      <mesh>",
        "        <vertices>",
    ]
    for x, y, z in vertices:
        lines.append(f'          <vertex x="{x:g}" y="{y:g}" z="{z:g}"/>')
    lines.append("        </vertices>")
    lines.append("        <triangles>")
    for a, b, c in faces:
        lines.append(f'          <triangle v1="{a}" v2="{b}" v3="{c}"/>')
    lines.append("        </triangles>")
    lines.append("      </mesh>")
    lines.append("    </object>")
    lines.append("  </resources>")
    lines.append("  <build>")
    lines.append('    <item objectid="1"/>')
    lines.append("  </build>")
    lines.append("</model>")
    return "\n".join(lines) + "\n"


def write(target: Path, triangles: list[tuple[tuple[float, float, float], ...]], project: bool) -> None:
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("[Content_Types].xml", CONTENT_TYPES)
        archive.writestr("_rels/.rels", RELS)
        archive.writestr("3D/3dmodel.model", model_xml(triangles))
        if project:
            archive.writestr("Metadata/Slic3r_PE.config", PROJECT_CONFIG)
            archive.writestr("Metadata/Slic3r_PE_model.config", PROJECT_MODEL)
    print(
        f"wrote {target.relative_to(ROOT)}: {len(triangles)} triangles, "
        f"{target.stat().st_size} bytes"
    )


def main() -> int:
    triangles = stl_triangles(SOURCE)
    write(TARGET, triangles, project=False)
    write(PROJECT, triangles, project=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
