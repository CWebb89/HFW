#!/usr/bin/env python3
"""Generate crates/config/src/dialect_table.rs from the two proven sources.

Sources, both already validated elsewhere:

  next-layer/src/qt/src/ForeignSettings.cpp
      the Orca/Bambu name -> engine name renames, gated by
      next-layer's oracle/foreign_settings_check.py
  next-layer/src/qt/src/PresetImport.cpp
      the rest of the renames its own importer applies, where the gated table
      says nothing (the gated one wins a disagreement)
  next-layer/upstream/.../ConfigDefsFDM.cpp + ConfigCommon.cpp
      the engine's own setting names, the same table the ledger reads
  tools/gen-settings-ledger.py
      this repository's own key names and which engine names they serve

The generated file is a lookup, not a behaviour: it says which names are part
of the dialect, which of them HFW already understands, and which are only
recognised. Re-run after changing crates/config/src/parse.rs.
"""

from __future__ import annotations

import importlib.util
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
HFW = HERE.parent
LEDGER_TOOL = HERE / "gen-settings-ledger.py"


def load_ledger_tool():
    spec = importlib.util.spec_from_file_location("ledger", LEDGER_TOOL)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {LEDGER_TOOL}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


NATIVE_KEYS_RE = re.compile(r'^ {12}"([a-z0-9_]+)" =>', re.M)

PER_TOOL_EXTRAS = ("nozzle_diameter",)


def native_keys(tool) -> list[str]:
    keys = set(NATIVE_KEYS_RE.findall(tool.HFW_PARSE.read_text(encoding="utf-8")))
    return sorted(keys)


def engine_names(tool) -> list[str]:
    names = set(tool.parse_modern(tool.DOMAIN / "ConfigDefsFDM.cpp"))
    names.update(tool.loop_generated(tool.DOMAIN / "ConfigDefsFDM.cpp"))
    names.update(tool.parse_modern(tool.DOMAIN / "ConfigCommon.cpp"))
    names.update(PER_TOOL_EXTRAS)
    return sorted(names)


def renames(tool) -> dict[str, str]:
    """Dialect spelling -> engine name, minus the names this crate defines itself.

    A key `PrinterConfig::assign` already accepts is not a foreign spelling, and
    renaming it away loses its own meaning: the importer calls `print_sequence`
    a boolean and `line_width` an extrusion width, while here both are settings
    with their own value spellings.
    """
    native = set(native_keys(tool))
    return {
        source: target
        for source, target in tool.dialect_renames().items()
        if source not in native
    }


def rust_str(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def rustfmt_text(text: str) -> str:
    """Formats generated Rust the way `cargo fmt` will, so the tree stays clean.

    A generator whose output `cargo fmt` rewrites will fail its own staleness
    check forever; comparing formatted text on both sides is what makes --check
    mean something.
    """
    try:
        result = subprocess.run(
            ["rustfmt", "--edition", "2021", "--emit", "stdout"],
            input=text,
            capture_output=True,
            text=True,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError):
        return text
    if result.returncode != 0 or not result.stdout.strip():
        return text
    return result.stdout


def emit_array(items: list[tuple[str, ...]], width: int = 4) -> str:
    lines = []
    for start in range(0, len(items), width):
        chunk = items[start:start + width]
        rendered = ", ".join(
            "(" + ", ".join(rust_str(part) for part in item) + ")" for item in chunk
        )
        lines.append(f"    {rendered},")
    return "\n".join(lines)


def main() -> int:
    tool = load_ledger_tool()
    status = tool.require_inputs()
    if status != 0:
        return status
    engine = engine_names(tool)
    known = set(engine)

    applied = sorted(
        [(dst, src) for src, (dst, status, _note) in tool.HFW_ALIAS.items()
         if dst and status == "applied" and dst in known]
        + [(src, dst) for src, (dst, status, _note) in tool.HFW_SYNONYMS.items()
           if status == "applied" and src in known]
    )
    partial = sorted(
        [(dst, src) for src, (dst, status, _note) in tool.HFW_ALIAS.items()
         if dst and status == "partial" and dst in known]
        + [(src, dst) for src, (dst, status, _note) in tool.HFW_SYNONYMS.items()
           if status == "partial" and src in known]
        + [(name, name) for name, (status, _note) in tool.HFW_CAVEATS.items()
           if status == "partial" and name in known]
    )
    native = native_keys(tool)
    mapped = {source for source, _ in applied} | {source for source, _ in partial}
    rename_pairs = sorted(
        (source, target)
        for source, target in renames(tool).items()
        if target in known and source not in mapped
    )
    orca_only = sorted(
        key for key in (tool.orca_keys() or set())
        if key not in known and key not in set(native) and key not in dict(rename_pairs)
    )

    out = [
        "//! Generated by `tools/gen-dialect-table.py`. Do not edit by hand.",
        "//!",
        "//! Sources: next-layer's `ForeignSettings.cpp` (the Orca/Bambu renames its",
        "//! own oracle gates) and `PresetImport.cpp` (the rest of its importer's",
        "//! renames), next-layer's vendored engine definition table, and this",
        "//! crate's parser. Re-run the script after either changes.",
        "",
        f"/// Every setting name the FDM engine defines ({len(engine)}).",
        "pub const ENGINE_NAMES: &[&str] = &[",
    ]
    out.append("\n".join(f"    {rust_str(name)}," for name in engine))
    out.append("];")
    out.append("")
    out.append(
        f"/// Dialect spellings that mean an engine setting: Orca/Bambu name -> engine"
        f" name ({len(rename_pairs)})."
    )
    out.append("pub const RENAMES: &[(&str, &str)] = &[")
    out.append(emit_array([(src, dst) for src, dst in rename_pairs]))
    out.append("];")
    out.append("")
    out.append(
        f"/// Engine setting -> this crate's key, semantics identical ({len(applied)})."
    )
    out.append("pub const APPLIED: &[(&str, &str)] = &[")
    out.append(emit_array(applied))
    out.append("];")
    out.append("")
    out.append(
        f"/// Engine setting -> this crate's key, semantics differ ({len(partial)})."
    )
    out.append("pub const PARTIAL: &[(&str, &str)] = &[")
    out.append(emit_array(partial))
    out.append("];")
    out.append("")
    out.append(
        f"/// Key names this crate's own profiles use ({len(native)}), accepted as-is."
    )
    out.append("pub const NATIVE: &[&str] = &[")
    out.append("\n".join(f"    {rust_str(name)}," for name in native))
    out.append("];")
    out.append("")
    out.append(
        f"/// Names Orca profiles carry that the engine has no setting for ({len(orca_only)})."
    )
    out.append("/// They are dialect, not typos: recognised, reported, and not applied.")
    out.append("pub const ORCA_ONLY: &[&str] = &[")
    out.append("\n".join(f"    {rust_str(name)}," for name in orca_only))
    out.append("];")
    out.append("")

    target = HFW / "crates/config/src/dialect_table.rs"
    text = rustfmt_text("\n".join(out) + "\n")
    if "--check" in sys.argv[1:]:
        current = target.read_text(encoding="utf-8") if target.exists() else ""
        if current != text:
            print(f"{target} is stale; run tools/gen-dialect-table.py", file=sys.stderr)
            return 1
        print(f"dialect table: {len(engine)} engine names, up to date")
        return 0
    target.write_text(text, encoding="utf-8")
    print(f"wrote {target}")
    print(f"engine names: {len(engine)}  renames: {len(rename_pairs)}  "
          f"applied: {len(applied)}  partial: {len(partial)}  native: {len(native)}  "
          f"orca-only: {len(orca_only)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
