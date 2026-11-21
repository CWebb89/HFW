#!/usr/bin/env python3
"""Everything the generators read from outside this repository, in one list.

The generated tables used to be built by reading next-layer's own source. That
made the gate depend on a tree somebody else is porting module by module: every
time a file this repository read moved from C++ to Rust, a section of the gate
went red and the fifty-eight sections behind it went unverified. Tolerating each
new shape is not a fix; owning the data is.

So the dialect is now this repository's data. `crates/config/assets/dialect.json`
holds the renames, the importer's table and the Orca name snapshot, and the
generators read that file. `tools/refresh-dialect.py` proposes an update from
next-layer when somebody asks for one -- deliberately, as a reviewed diff, not as
a side effect of building.

What remains external is listed once, in `EXTERNAL_INPUTS`, with every path each
input may live at. A missing input reports every path it tried rather than a red
gate at section 29, and no generator may shrink a table because a file was
absent: that is the one failure mode a generated inventory must not have.

`docs/NEXTLAYER-BRIDGE.md` (P8.5) states, for each entry, why this repository
does not own it and what happens if it goes.
"""

from __future__ import annotations

import json
import re
from collections import OrderedDict
from pathlib import Path

HFW = Path(__file__).resolve().parent.parent
NEXT_LAYER = next(
    (path for path in (HFW.parent, HFW.parent.parent) if (path / "next-layer").is_dir()),
    HFW.parent,
) / "next-layer"
DOMAIN = NEXT_LAYER / "upstream/src/slic3r-domain/src/Slic3r/Domain"
LEGACY = (
    NEXT_LAYER
    / "upstream/src/slic3r-shared/src/Slic3r/Biz/Config/Legacy/PrintConfig.cpp"
)
PANEL = NEXT_LAYER / "src/qt/assets/settings-layout.json"
FOREIGN_CPP = NEXT_LAYER / "src/qt/src/ForeignSettings.cpp"
FOREIGN_RS = NEXT_LAYER / "src/app/src/ForeignSettings.rs"
IMPORT_CPP = NEXT_LAYER / "src/qt/src/PresetImport.cpp"
IMPORT_RS = NEXT_LAYER / "src/app/src/PresetImport.rs"
ORCA_CONFIG = Path.home() / ".config/OrcaSlicer"

ASSETS = HFW / "crates/config/assets"
DIALECT = ASSETS / "dialect.json"

LOOP_AXES = ("x", "y", "z", "e")
LOOP_PREFIXES = ("machine_max_feedrate_", "machine_max_acceleration_",
                 "machine_max_jerk_")

ADD_RE = re.compile(r'def\s*=\s*defs\.add\(\s*"([a-z0-9_]+)",\s*typeid\(([^)]*)\)\s*\)')
LEGACY_ADD_RE = re.compile(
    r'def\s*=\s*this->add\(\s*"([a-z0-9_]+)",\s*(co[A-Za-z]+)\s*\)'
)

ORCA_METADATA = {
    "inherits", "inherits_group", "from", "name", "instantiation", "version",
    "is_custom_defined", "setting_id", "filament_id", "filament_vendor",
    "compatible_printers", "compatible_printers_condition",
    "other_compatible_printers", "printer_model", "printer_variant",
    "printer_settings_id", "print_settings_id", "filament_settings_id",
    "default_print_profile", "default_filament_profile", "type", "url",
    "description", "rename", "include", "upward_compatible_machine",
    "compatible_machine", "different_settings_to_system", "exclude_selected",
}

EXTERNAL_INPUTS = (
    {
        "id": "definitions",
        "what": "the FDM engine's own setting definitions: name, type, location, "
                "category, default, unit, enum spellings",
        "owner": "upstream's: vendored in next-layer/upstream, not this repository's "
                 "to redefine",
        "candidates": ((DOMAIN / "ConfigDefsFDM.cpp", "modern"),),
    },
    {
        "id": "common",
        "what": "the options FFF and SLA share",
        "owner": "upstream's, same table",
        "candidates": ((DOMAIN / "ConfigCommon.cpp", "modern"),),
    },
    {
        "id": "legacy",
        "what": "the legacy table, for names the modern table dropped",
        "owner": "upstream's, same table",
        "candidates": ((LEGACY, "legacy"),),
    },
    {
        "id": "panel",
        "what": "which settings the application's panel actually offers, which is "
                "what makes a ledger row reachable or internal",
        "owner": "next-layer's own UI inventory. A JSON resource, not ported code, "
                 "so it has no Rust twin; it moves only if the panel is redesigned",
        "candidates": ((PANEL, "panel"),),
    },
    {
        "id": "orcaconfig",
        "what": "which names Orca profiles actually carry, measured on this machine",
        "owner": "the user's own profile directory, not a repository",
        "candidates": ((ORCA_CONFIG, "orca"),),
    },
    {
        "id": "gated",
        "what": "the Orca/Bambu rename table next-layer's oracle gates",
        "owner": "was next-layer's; this repository now owns the copy it accepts. "
                 "Read Rust-first when both exist, because the port is where the "
                 "application's behaviour lives and the C++ may be a leftover. "
                 "Refreshed by tools/refresh-dialect.py, never read by the gate",
        "candidates": ((FOREIGN_RS, "gated_rs"), (FOREIGN_CPP, "gated_cpp")),
    },
    {
        "id": "importer",
        "what": "the rest of the renames next-layer's preset importer applies",
        "owner": "was next-layer's; this repository now owns the copy it accepts. "
                 "Read Rust-first when both exist, for the same reason. "
                 "Refreshed by tools/refresh-dialect.py, never read by the gate",
        "candidates": ((IMPORT_RS, "importer_rs"), (IMPORT_CPP, "importer_cpp")),
    },
)

BY_ID = {entry["id"]: entry for entry in EXTERNAL_INPUTS}


class Missing(Exception):
    """One or more external inputs are absent, with every path tried."""

    def __init__(self, entries):
        self.entries = entries
        lines = []
        for entry, paths in entries:
            lines.append(f"missing {entry['id']}: {entry['what']}")
            for path in paths:
                lines.append(f"  tried: {path}")
            lines.append(f"  owner: {entry['owner']}")
        super().__init__("\n".join(lines))


def locate(entry_id: str) -> Path:
    entry = BY_ID[entry_id]
    paths = [path for path, _ in entry["candidates"]]
    for path in paths:
        if path.exists():
            return path
    raise Missing([(entry, paths)])


def optional(entry_id: str) -> Path | None:
    for path, _ in BY_ID[entry_id]["candidates"]:
        if path.exists():
            return path
    return None


def require(entry_ids: tuple = ("definitions", "common", "legacy", "panel")) -> None:
    """Raises one `Missing` that lists every absent input and every path tried."""
    absent = []
    for entry_id in entry_ids:
        entry = BY_ID[entry_id]
        paths = [path for path, _ in entry["candidates"]]
        if not any(path.exists() for path in paths):
            absent.append((entry, paths))
    if absent:
        raise Missing(absent)


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return re.sub(r"//[^\n]*", "", text)


def first_arg(block: str, call: str) -> str:
    start = block.find(f"{call}(")
    if start == -1:
        return ""
    depth = 0
    for index in range(start + len(call) + 1, len(block)):
        char = block[index]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            if depth == 0:
                return block[start + len(call) + 1:index].strip()
            depth -= 1
        elif char == "," and depth == 0:
            return block[start + len(call) + 1:index].strip()
    return ""


def clean_default(raw: str) -> str:
    flat = " ".join(raw.split())
    flat = flat.replace("std::string(", "").replace("std::vector<std::string>", "")
    if len(flat) > 48:
        flat = flat[:45] + "..."
    return flat


def modern_definitions(text: str, source: str) -> OrderedDict:
    text = strip_comments(text)
    entries = OrderedDict()
    matches = list(ADD_RE.finditer(text))
    for index, match in enumerate(matches):
        name, ctype = match.group(1), match.group(2)
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        block = text[match.end():end]
        locations = re.findall(r"def->location = (\w+);", block)
        locations += re.findall(r"def->locations = \{([^}]*)\}", block)
        category = re.search(r"def->category = ConfigItemDef::Category::(\w+);", block)
        default = first_arg(block, "init_with") or first_arg(block, "SET_DEFAULT")
        enums = re.findall(r'int\([A-Za-z_:\w]+\)\s*,\s*"([a-z0-9_]+)"', block)
        unit = re.search(r'def->sidetext = L\("([^"]+)"\)', block)
        if not unit:
            unit = re.search(r'def->units = \{L\("([^"]+)"\)', block)
        entries[name] = {
            "type": ctype,
            "locations": ",".join(sorted(set(locations))) or "?",
            "category": category.group(1) if category else "",
            "default": clean_default(default) if default else "",
            "enums": enums,
            "unit": unit.group(1) if unit else "",
            "source": source,
        }
    return entries


def loop_generated(text: str, source: str) -> dict:
    entries = {}
    for prefix in LOOP_PREFIXES:
        marker = f'defs.add("{prefix}" + axis.name'
        if marker not in text:
            continue
        block_start = text.index(marker)
        block_end = text.find("def = defs.add(", block_start + len(marker))
        block = text[block_start:block_end if block_end != -1 else len(text)]
        version = re.search(r'"([a-z0-9_]+)"', block)
        for axis in LOOP_AXES:
            entries[f"{prefix}{axis}"] = {
                "type": "std::vector<double>",
                "locations": "Printer",
                "category": "Machine Limits",
                "default": " ".join((version.group(1) if version else "").split())[:48],
                "enums": [],
                "unit": "",
                "source": source,
            }
    return entries


def legacy_types(text: str) -> dict:
    types: dict[str, str] = {}
    for name, ctype in LEGACY_ADD_RE.findall(text):
        types.setdefault(name, ctype)
    return types


def engine_definitions() -> OrderedDict:
    """The engine's setting table: FDM definitions, the shared ones, the loops."""
    entries = OrderedDict()
    fdm = locate("definitions")
    fdm_text = fdm.read_text(encoding="utf-8", errors="replace")
    entries.update(modern_definitions(fdm_text, fdm.name))
    entries.update(loop_generated(fdm_text, fdm.name))
    common = locate("common")
    for name, entry in modern_definitions(
        common.read_text(encoding="utf-8", errors="replace"), common.name
    ).items():
        entry["category"] = entry["category"] or "Common"
        entries.setdefault(name, entry)
    return entries


def legacy_names() -> dict:
    path = locate("legacy")
    return legacy_types(path.read_text(encoding="utf-8", errors="replace"))


def panel_surface() -> set:
    path = locate("panel")
    layout = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    keys = set()
    for pages in layout.get("tabs", {}).values():
        for page in pages:
            for section in page.get("sections", []):
                keys.update(section.get("settings", []))
    return keys


def orca_names_measured() -> set | None:
    path = optional("orcaconfig")
    if path is None:
        return None
    found = set()
    for profile in path.glob("**/*.json"):
        try:
            data = json.loads(profile.read_text(encoding="utf-8", errors="replace"))
        except Exception:
            continue
        if not isinstance(data, dict):
            continue
        for key, value in data.items():
            if key in ORCA_METADATA or not re.fullmatch(r"[a-z0-9_]+", key):
                continue
            if isinstance(value, (str, int, float, bool, list)):
                found.add(key)
    return found


FOREIGN_RS_RE = re.compile(r"const RENAMED: \[\(&str, &str\); (\d+)\] = \[(.*?)\n\];", re.S)
FOREIGN_CPP_RE = re.compile(
    r'\{QStringLiteral\("([a-z0-9_]+)"\),\s*QStringLiteral\("([a-z0-9_]+)"\)\}'
)
FOREIGN_PAIR_RE = re.compile(r'\(\s*"([a-z0-9_]+)"\s*,\s*"([a-z0-9_]+)"\s*\)')
IMPORT_RS_RE = re.compile(
    r'source:\s*"([a-z0-9_]+)"\s*,\s*target:\s*"([a-z0-9_]+)"'
)
IMPORT_CPP_RE = re.compile(
    r'[{(]\s*"([a-z0-9_]+)"\s*,\s*"([a-z0-9_]+)"\s*,\s*Transform::\w+'
)


def gated_renames_from(path: Path) -> list:
    """Orca/Bambu name -> engine name, from the C++ table or its Rust port."""
    text = path.read_text(encoding="utf-8", errors="replace")
    if path.suffix == ".rs":
        table = FOREIGN_RS_RE.search(text)
        if table is None:
            raise SystemExit(f"{path}: no `const RENAMED` table to read")
        pairs = FOREIGN_PAIR_RE.findall(table.group(2))
        if len(pairs) != int(table.group(1)):
            raise SystemExit(
                f"{path}: the table declares {table.group(1)} renames and "
                f"{len(pairs)} were read; the syntax changed under this parser"
            )
        return pairs
    return FOREIGN_CPP_RE.findall(text)


def rust_function_body(text: str, name: str) -> str:
    """The body of `fn <name>(...)`, braces balanced, so a table is read without its
    unit tests.

    The ported importer builds its table inside `fn mapping_table()` and its tests
    construct throwaway `Mapping` rows of their own; reading the whole file reads
    those too and puts `x` in the dialect.
    """
    start = text.find(f"fn {name}(")
    if start == -1:
        raise SystemExit(f"no `fn {name}` to read the table from")
    brace = text.find("{", start)
    depth = 0
    for index in range(brace, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[brace:index + 1]
    raise SystemExit(f"`fn {name}` is not closed")


def importer_renames_from(path: Path) -> list:
    """The importer's own renames, from the C++ table or its Rust port."""
    text = path.read_text(encoding="utf-8", errors="replace")
    if path.suffix == ".rs":
        pairs = IMPORT_RS_RE.findall(rust_function_body(text, "mapping_table"))
        if not pairs:
            raise SystemExit(f"{path}: no rename rows in `fn mapping_table`")
        return pairs
    pairs = IMPORT_CPP_RE.findall(text)
    if not pairs:
        raise SystemExit(f"{path}: no rename rows matched; the syntax changed")
    return pairs


def asset() -> dict:
    if not DIALECT.exists():
        raise Missing([(BY_ID["gated"], [DIALECT]),
                       (BY_ID["importer"], [DIALECT])])
    return json.loads(DIALECT.read_text(encoding="utf-8"))


def gated_renames() -> dict:
    return {source: target for source, target in asset()["gated_renames"]}


def importer_renames() -> dict:
    """First row wins, which is the order next-layer's ported table documents."""
    pairs: dict[str, str] = {}
    for source, target in asset()["importer_renames"]:
        pairs.setdefault(source, target)
    return pairs


def dialect_renames() -> dict:
    """Orca/Bambu spelling -> engine spelling, from both owned tables.

    The importer carries the keys the application renames and the gated table the
    ones its oracle checks. They overlap and disagree on four keys, so the gated
    table wins: `overhang_1_4_speed` and its neighbours are quarter-of-the-perimeter
    speeds whose index the two tables number differently, and `infill_combination`
    means different layers there. A key only the importer renames is still a real
    rename -- without them a machine preset's `printable_area` and
    `machine_max_speed_x` never reach this engine.
    """
    pairs = importer_renames()
    pairs.update(gated_renames())
    return pairs


def orca_names() -> set | None:
    names = asset().get("orca_names")
    return set(names) if names else None
