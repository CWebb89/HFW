#!/usr/bin/env python3
"""Builds `fixtures/plate-overrides.3mf`: two cubes, one with its own settings.

The plate exists to exercise per-object settings end to end: two build items 25
mm apart, with `Metadata/Slic3r_PE_model.config` overriding infill density and
wall count on the second object only. A slicer that ignores the object overrides
prints both cubes identically, which the fixture makes visible as equal deposited
volume per object.

    tools/make-plate-3mf.py
"""

from __future__ import annotations

import zipfile
from importlib import import_module
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / "fixtures/plate-overrides.3mf"
SOURCE = ROOT / "fixtures/cube.stl"

fixture = import_module("make-3mf-fixture")

MODEL_CONFIG = """<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1">
    <metadata type="object" key="name" value="plain cube"/>
  </object>
  <object id="2">
    <metadata type="object" key="name" value="dense cube"/>
    <metadata type="object" key="sparse_infill_density" value="60%"/>
    <metadata type="object" key="wall_loops" value="5"/>
    <volume firstid="0" lastid="11">
      <metadata type="volume" key="sparse_infill_density" value="5%"/>
    </volume>
  </object>
</config>
"""


def main() -> int:
    triangles = fixture.stl_triangles(SOURCE)
    model = fixture.model_xml(triangles).replace(
        '    <item objectid="1"/>',
        '    <item objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 0"/>\n'
        '    <item objectid="2" transform="1 0 0 0 1 0 0 0 1 25 0 0"/>',
    )
    nested = (
        '    <object id="2">\n'
        '      <components>\n'
        '        <component objectid="1"/>\n'
        "      </components>\n"
        "    </object>\n"
    )
    model = model.replace("  </resources>", nested + "  </resources>")
    with zipfile.ZipFile(TARGET, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("[Content_Types].xml", fixture.CONTENT_TYPES)
        archive.writestr("_rels/.rels", fixture.RELS)
        archive.writestr("3D/3dmodel.model", model)
        archive.writestr("Metadata/Slic3r_PE_model.config", MODEL_CONFIG)
    print(f"wrote {TARGET.relative_to(ROOT)}: 2 objects, 25 mm apart, one overridden")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
