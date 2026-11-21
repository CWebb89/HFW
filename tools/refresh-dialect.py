#!/usr/bin/env python3
"""Proposes an update to `crates/config/assets/dialect.json` from next-layer.

The dialect is this repository's data: which spellings it accepts is a decision
about what this engine reads, not a fact about somebody else's build. This script
is how that decision is revised -- by hand, as a reviewed diff -- when next-layer's
tables gain a rename or the user's Orca installation gains a name.

It is not part of the gate. The gate reads the asset and nothing else, so a port
in next-layer can no longer turn it red; this script reports what next-layer's
tree currently holds, and refuses to write a partial asset.

    tools/refresh-dialect.py            rewrite the asset from next-layer
    tools/refresh-dialect.py --check    say whether the asset still agrees
"""

from __future__ import annotations

import datetime
import json
import sys

import external_inputs as ext


def harvest() -> dict:
    gated_path = ext.locate("gated")
    importer_path = ext.locate("importer")
    gated = ext.gated_renames_from(gated_path)
    importer = ext.importer_renames_from(importer_path)
    orca = ext.orca_names_measured()
    return {
        "format": "hfw-dialect",
        "version": 1,
        "captured": datetime.date.today().isoformat(),
        "note": (
            "The dialect this engine accepts: Orca/Bambu spelling -> engine "
            "spelling. Owned here, so the settings tables are generated from "
            "nothing outside this repository. `gated_renames` wins a disagreement; "
            "`importer_renames` carries the keys next-layer's preset importer "
            "renames and its oracle does not check. `orca_names` is a snapshot of "
            "the names Orca profiles on the machine that ran "
            "tools/refresh-dialect.py actually carry, because a generated table "
            "must not depend on whose profile directory is present."
        ),
        "sources": {
            "gated_renames": str(gated_path.relative_to(ext.NEXT_LAYER)),
            "importer_renames": str(importer_path.relative_to(ext.NEXT_LAYER)),
            "orca_names": str(ext.ORCA_CONFIG),
        },
        "gated_renames": [list(pair) for pair in gated],
        "importer_renames": [list(pair) for pair in importer],
        "orca_names": sorted(orca) if orca else [],
    }


def render(document: dict) -> str:
    return json.dumps(document, indent=1) + "\n"


def main() -> int:
    try:
        document = harvest()
    except ext.Missing as missing:
        print(f"cannot refresh the dialect: {missing}", file=sys.stderr)
        return 1
    text = render(document)
    if "--check" in sys.argv[1:]:
        current = ext.DIALECT.read_text(encoding="utf-8")
        if current != text:
            print(
                f"{ext.DIALECT} no longer matches next-layer; run "
                "tools/refresh-dialect.py and review the diff",
                file=sys.stderr,
            )
            return 1
        print(f"dialect asset agrees with next-layer: {ext.DIALECT}")
        return 0
    ext.DIALECT.parent.mkdir(parents=True, exist_ok=True)
    if ext.DIALECT.exists() and ext.DIALECT.read_text(encoding="utf-8") == text:
        print(f"{ext.DIALECT} unchanged")
        return 0
    ext.DIALECT.write_text(text, encoding="utf-8")
    print(f"wrote {ext.DIALECT}")
    print(
        f"gated renames: {len(document['gated_renames'])}  "
        f"importer renames: {len(document['importer_renames'])}  "
        f"orca names: {len(document['orca_names'])}"
    )
    print("the settings tables now need regenerating: tools/gen-dialect-table.py, "
          "tools/gen-settings-ledger.py")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
