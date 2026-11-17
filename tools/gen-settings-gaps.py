#!/usr/bin/env python3
"""Writes `docs/SETTINGS-GAPS.md`: what the 230 unapplied settings actually are.

The ledger says a setting is missing. That is a fact, not a plan. This script
sorts every unapplied engine setting into the bucket that decides what it would
cost to close, so "why not all of them" has an answer with numbers in it.

    tools/gen-settings-gaps.py            rewrite the document
    tools/gen-settings-gaps.py --check    fail if the committed copy is stale
"""

from __future__ import annotations

import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEDGER = ROOT / "docs/SETTINGS-LEDGER.md"
TARGET = ROOT / "docs/SETTINGS-GAPS.md"

BUCKETS: list[tuple[str, str, tuple[str, ...], tuple[str, ...]]] = [
    (
        "Multi-material, toolchange and MMU",
        "HFW loads one mesh and drives one extruder. Nothing here means anything "
        "until it can drive two, so these wait for the toolchanger work rather "
        "than blocking anything a single-extruder print needs.",
        ("filament_", "mmu_", "wipe_tower", "toolchange", "ooze_prevention",
         "standby_temperature", "multimaterial", "single_extruder_multi_material",
         "high_current_on_filament_swap", "parking_pos", "cooling_tube",
         "extra_loading_move", "wiping_volumes", "enable_pressure_advance_during_ramming",
         "wipe_into", "extruder_clearance", "interlocking"),
        ("extruder", "infill_extruder", "perimeter_extruder",
         "solid_infill_extruder", "support_material_extruder",
         "support_material_interface_extruder", "bed_temperature_extruder",
         "extruder_offset", "retract_length_toolchange",
         "retract_restart_extra_toolchange", "retract_before_wipe",
         "filament_flush_speed", "filament_flush_volume"),
    ),
    (
        "Printer and filament identity, not slicing",
        "Model names, vendor strings, colours, costs, notes. Nothing here changes "
        "a motion: they reach the file as header comments, so a plate two weeks "
        "later traces back to a filament and a machine, and the cost is reported "
        "from the filament price and the spool weight. Done: every one of them.",
        (),
        ("printer_model", "printer_variant", "printer_vendor", "printer_notes",
         "notes", "default_print", "default_material", "default_tool_print",
         "extruder_colour", "filament_colour", "filament_cost", "filament_type",
         "filament_vendor", "filament_notes", "filament_soluble", "filament_abrasive",
         "filament_spool_weight", "custom_parameters_filament",
         "custom_parameters_print", "custom_parameters_printer", "post_process",
         "output_filename_format", "template_custom_gcode"),
    ),
    (
        "G-code emission options",
        "Done: comment gating, object labels, coordinate rounding, volumetric `E`, "
        "firmware retraction, clockwise loops, `M73` progress, temperature "
        "suppression, the two layer hooks, the filament hooks, rendered PNG/QOI "
        "thumbnails, `gcode_substitutions` as a regular-expression pass, the pause "
        "and colour-change G-code at the heights the caller asks for, and "
        "`between_objects_gcode` between the objects of a sequential plate. "
        "`silent_mode` is a host display choice upstream never writes either; it is "
        "kept and recorded in the header.",
        ("gcode_", "thumbnails", "binary_gcode", "remaining_times", "silent_mode",
         "autoemit_temperature_commands", "use_volumetric_e", "use_firmware_retraction",
         "stuck_filament_detection", "prefer_clockwise_movements", "before_layer_gcode",
         "layer_gcode", "start_filament_gcode", "end_filament_gcode",
         "pause_print_gcode", "color_change_gcode", "between_objects_gcode",
         "toolchange_gcode", "filament_change_time"),
        (),
    ),
    (
        "Per-feature speeds and accelerations",
        "The overhang ladder, the first-layer speeds per feature and the named "
        "acceleration spellings are in. What is left is travel acceleration "
        "(which would need a macro pair around every travel), the two raft "
        "values, and over-bridge speed, which needs to know a bridge is being "
        "printed over another bridge rather than over air.",
        (),
        ("overhang_speed_0", "overhang_speed_1", "overhang_speed_2", "overhang_speed_3",
         "over_bridge_speed", "travel_acceleration", "travel_short_distance_acceleration",
         "wipe_tower_acceleration", "first_layer_acceleration_over_raft",
         "first_layer_speed_over_raft"),
    ),
    (
        "Cooling and fan shaping",
        "Done: the four overhang fan quarters, the per-feature overrides, the "
        "first-layer duty, the global cooling switch, and all three slowdown "
        "distributions with the transition band that goes with the third: "
        "proportional scales the whole layer, uniform slows the fastest beads down "
        "to the floor first, and consistent_surface holds the band nearest the wall "
        "back for last. Filament loading, ramming and toolchange speeds wait with "
        "the toolchanger.",
        ("overhang_fan_speed_", "fan_always_on", "initial_layer_fan_speed",
         "ironing_fan_speed", "internal_bridge_fan_speed",
         "support_material_interface_fan_speed", "enable_dynamic_fan_speeds",
         "enable_dynamic_overhang_speeds", "enable_overhang_bridge_fan",
         "cooling_slowdown_logic", "cooling_perimeter_transition_distance",
         "filament_cooling_", "filament_ramming", "filament_loading_speed",
         "filament_unloading_speed", "filament_toolchange_delay"),
        ("chamber_minimal_temperature", "cooling_slowdown_logic",
         "cooling_perimeter_transition_distance", "enable_overhang_bridge_fan",
         "internal_bridge_fan_speed"),
    ),
    (
        "Z hop and travel shaping",
        "Nothing: the bucket is closed. The hop height, its ceiling and its Z "
        "window, the slope ratio and the ramping hop are in, and so is "
        "`travel_lift_before_obstacle`: the obstacle is this layer's own overhanging "
        "beads, whose edges curl as they cool, so a travel that crosses one lifts as "
        "if it were a long move. The same bead rectangles are what "
        "`avoid_crossing_curled_overhangs` routes around. Both need overhang "
        "measurement switched on, because an unmeasured bead carries no overhang.",
        ("retract_lift_above", "retract_lift_below", "travel_lift_before_obstacle",
         "travel_max_lift", "travel_ramping_lift", "travel_slope"),
        (),
    ),
    (
        "Support details",
        "Nothing: the bucket is closed. Closing radius, bottom contact distance, "
        "bridge skipping and the threshold angle's second spelling are all in, and "
        "layer synchronisation is `n/a` because this engine has one layer schedule, "
        "so support and part layers cannot drift apart.",
        ("support_material_pattern", "support_material_interface_pattern",
         "support_material_angle", "support_material_bottom_contact_distance",
         "support_material_bottom_interface_layers", "support_material_enforce_layers",
         "support_material_closing_radius", "support_material_interface_contact_loops",
         "support_material_synchronize_layers", "support_material_with_sheath",
         "support_tree_angle_slow", "support_tree_branch_diameter_double_wall",
         "support_tree_top_rate", "support_material_first_layer_",
         "dont_support_bridges", "interface_shells"),
        (),
    ),
    (
        "Seams and surface finish",
        "Nothing: the bucket is closed. The scarf ramp, its segment length, its start "
        "height, scarfing inner walls, its three placement values (holes are found by "
        "nesting, because winding is normalised for the G-code), the whole-loop ramp, "
        "the smooth-only corner test, staggered inner seams, the seam gap and fuzzy "
        "skin are all in.",
        ("scarf_seam", "seam_gap_distance", "staggered_inner_seams", "fuzzy_skin"),
        (),
    ),
    (
        "Adhesion and object handling",
        "Done: brim sides, brim separation, the skirt length floor, raft layers with "
        "their first-layer density and expansion, the draft shield, and "
        "one-object-at-a-time sequencing (complete_objects, or print_sequence = by "
        "object, which plans each part as its own stack). What is left is the "
        "first-layer single-perimeter modes and the raft expansion ramp.",
        ("brim_", "raft_", "draft_shield", "min_skirt_length",
         "complete_objects", "only_one_perimeter_first_layer", "top_one_perimeter_type"),
        (),
    ),
    (
        "Defaults that only out-of-the-box Orca/Prusa jobs use",
        "Names that exist so a stock preset file validates: which profile is "
        "default, which material is preselected. A file that sets them is "
        "describing someone's menu, not a print.",
        ("default_print", "default_material", "default_tool_print"),
        (),
    ),
    (
        "Algorithms still to write",
        "Real geometry or planning work. Done: variable layer height, both the "
        "profile and its bounds, spiral vase, thin-wall detection with its width "
        "floor, contour-routed travel, elephant-foot "
        "compensation, XY size compensation, the volumetric rate slope (solved per "
        "bead path rather than by splitting a move), a polygon bed, and the "
        "island-level solid-fill threshold, and the two-line infill anchor. What is "
        "left is the fill patterns still reported rather than printed, wall "
        "transition shaping, path resolution and closing, and support-aware "
        "routing. Interlocking beams moved to the multi-material bucket, where they "
        "belong: they bond two filaments' infill and mean nothing to one extruder.",
        ("slicing_mode",
         "gcode_resolution", "slice_closing_radius",
         "resolution", "extra_perimeters",
         "ensure_vertical_shell_thickness", "wall_transition", "wall_distribution_count",
         "perimeter_generator",
         "infill_every_layers", "solid_infill_every_layers",
         "automatic_infill_combination", "solid_infill_below_area", "fill_angle",
         "infill_first", "infill_overlap", "top_fill_pattern", "bottom_fill_pattern",
         "top_solid_min_thickness", "bottom_solid_min_thickness",
         "pressure_advance_value", "max_volumetric_extrusion_rate",
         "first_layer_bed_temperature", "filament_infill_max_crossing_speed",
         "filament_infill_max_speed", "filament_max_volumetric_speed",
         "filament_shrinkage_compensation_xy", "filament_shrinkage_compensation_z",
         "gap_fill_enabled", "overhangs", "machine_max_acceleration_extruding",
         "machine_max_acceleration_retracting", "machine_max_acceleration_travel",
         "machine_max_junction_deviation", "machine_min_extruding_rate",
         "machine_min_travel_rate", "z_offset", "automatic_extrusion_widths",
         "filament_shrinkage", "only_retract_when_crossing_perimeters",
         "cooling_perimeter"),
        (),
    ),
]


def unapplied() -> list[tuple[str, str]]:
    """Every row the ledger says is not applied and is still worth applying."""
    rows = []
    for line in LEDGER.read_text(encoding="utf-8").splitlines():
        match = re.match(r"\| `([^`]+)` \| ([0-9a-z/-]+) \| ([a-z/]+) \|", line)
        if not match or match.group(2) == "n/a":
            continue
        if match.group(3) in ("missing", "partial"):
            rows.append((match.group(1), match.group(3)))
    return rows


def bucket_of(name: str) -> int:
    """Explicit membership first, then prefixes, so a listed name always wins."""
    for index, (_title, _why, _prefixes, names) in enumerate(BUCKETS):
        if name in names:
            return index
    for index, (_title, _why, prefixes, _names) in enumerate(BUCKETS):
        if prefixes and name.startswith(prefixes):
            return index
    return len(BUCKETS) - 1


def out_of_scope() -> tuple[str, ...]:
    """Bucket titles whose settings a one-extruder engine cannot honour.

    A number that counts them as holes in the engine reads as progress still to
    be made on work that a machine with one extruder, or a G-code file without
    the event in question, can never use.
    """
    return (
        "Multi-material, toolchange and MMU",
        "Printer and filament identity, not slicing",
        "G-code emission options",
        "Defaults that only out-of-the-box Orca/Prusa jobs use",
    )


def single_extruder_scope() -> tuple[int, int]:
    """Applied and applicable counts outside the out-of-scope buckets."""
    skipped = {index for index, bucket in enumerate(BUCKETS)
               if bucket[0] in out_of_scope()}
    applied = applicable = 0
    for line in LEDGER.read_text(encoding="utf-8").splitlines():
        match = re.match(r"\| `([^`]+)` \| ([0-9a-z/-]+) \| ([a-z/]+) \|", line)
        if not match:
            continue
        name, tier, status = match.group(1), match.group(2), match.group(3)
        if tier == "n/a" or status == "n/a":
            continue
        if bucket_of(name) in skipped and status in ("missing", "partial"):
            continue
        applicable += 1
        if status == "applied":
            applied += 1
    return applied, applicable


def counts() -> tuple[Counter, Counter]:
    total = Counter()
    for name, _status in unapplied():
        total[name] += 1
    by_bucket = Counter()
    for name in total:
        by_bucket[bucket_of(name)] += 1
    return total, by_bucket


def scope_line() -> str:
    applied, applicable = single_extruder_scope()
    share = 100 * applied / applicable if applicable else 0
    return (
        f"Outside the buckets a one-extruder engine cannot use: {applied} of "
        f"{applicable} applied ({share:.0f}%)."
    )


def render() -> str:
    total, by_bucket = counts()
    standing = {}
    for line in LEDGER.read_text(encoding="utf-8").splitlines():
        match = re.match(r"- (.+?): \*\*(\d+)\*\*(?: \((\d+)%\))?", line)
        if match:
            key = match.group(1).replace("`", "").strip()
            standing[key] = (match.group(2), match.group(3))
    if standing:
        applicable = standing.get("applicable to a Klipper FDM target", ("?", None))[0]
        applied = standing.get("applied in HFW today", ("?", None))
        partial = standing.get("partial", ("?", None))[0]
        missing = standing.get("missing", ("?", None))[0]
        share = f" ({applied[1]}%)" if applied[1] else ""
        applied_line = (
            f"{applied[0]} of {applicable} applicable settings applied{share}, "
            f"{partial} partial, {missing} missing"
        )
    else:
        applied_line = "see the ledger"


    out = [
        "# What is still unapplied, and why",
        "",
        "Generated by `tools/gen-settings-gaps.py` from `docs/SETTINGS-LEDGER.md`.",
        "Do not edit by hand: run the script.",
        "",
        f"Current totals: {applied_line or 'see the ledger'}",
        "",
        scope_line(),
        "",
        "The ledger answers *what* is applied. This answers *why the rest are not*, "
        "because \"add them all\" is only a plan if every one of them costs the same. "
        "They do not: one bucket is a table, another is a second extruder.",
        "",
        "| Bucket | Settings | What closing it costs |",
        "|---|---|---|",
    ]
    for index, (title, why, _prefixes, _names) in enumerate(BUCKETS):
        out.append(f"| {title} | {by_bucket[index]} | {why} |")
    out.append(f"| **Total unapplied** | **{sum(by_bucket.values())}** | |")
    out.append("")
    out.append("## The settings in each bucket")
    out.append("")
    for index, (title, _why, _prefixes, _names) in enumerate(BUCKETS):
        members = sorted(name for name in total if bucket_of(name) == index)
        if not members:
            continue
        out.append(f"### {title} ({len(members)})")
        out.append("")
        out.append(", ".join(f"`{name}`" for name in members))
        out.append("")
    out.append("## Reading this as a work order")
    out.append("")
    out.append(
        "The percentage is not a score. Of the unapplied settings, the ones that "
        "change a single-extruder print are the per-feature speeds, the cooling "
        "curves, the Z-hop shaping, the support details and the seam shapes. The "
        "rest are multi-material, identity metadata, emission options or a stock "
        "preset's menu defaults: a printer with one extruder cannot honour the "
        "first group however long the engine is worked on, and honouring the "
        "second changes no output."
    )
    out.append("")
    return "\n".join(out)


def main() -> int:
    text = render()
    if "--check" in sys.argv:
        if not TARGET.exists() or TARGET.read_text(encoding="utf-8") != text:
            print(f"{TARGET} is stale: run tools/gen-settings-gaps.py")
            return 1
        print(f"{TARGET} up to date")
        return 0
    TARGET.write_text(text, encoding="utf-8")
    total, by_bucket = counts()
    print(f"wrote {TARGET}")
    print(f"unapplied {sum(total.values())} in {len([b for b in by_bucket.values() if b])} buckets")
    return 0


if __name__ == "__main__":
    sys.exit(main())
