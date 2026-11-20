#!/usr/bin/env python3
"""Generate docs/SETTINGS-LEDGER.md, the settings-coverage checklist for HFW.

Inputs, all on this machine, all machine-generated rather than typed:

  next-layer/upstream/src/slic3r-domain/src/Slic3r/Domain/ConfigDefsFDM.cpp
      the FDM engine's authoritative definition table (what the app enumerates)
  next-layer/upstream/src/slic3r-domain/src/Slic3r/Domain/ConfigCommon.cpp
      options shared by FFF and SLA
  next-layer/upstream/src/slic3r-shared/src/Slic3r/Biz/Config/Legacy/PrintConfig.cpp
      the legacy table, for names the modern table dropped
  crates/config/src/parse.rs
      the key names HFW's parser accepts today
  ~/.config/OrcaSlicer/**/*.json
      which names Orca actually uses in profiles (optional, skipped if absent)

Output is a markdown ledger: one row per setting, with a tier (implementation
cost class) and a status (measured against HFW's parser today). Tiers are
assigned by rule; the rules are listed in the generated document so a wrong
tier is visible rather than buried.
"""

from __future__ import annotations

import json
import re
import sys
from collections import Counter, OrderedDict
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
HFW_PARSE = HFW / "crates/config/src/parse.rs"
SURFACE = HFW / "crates/config/assets/settings-surface.json"
PER_OBJECT = HFW / "crates/engine/src/settings_key.rs"
FOREIGN_SETTINGS = NEXT_LAYER / "src/qt/src/ForeignSettings.cpp"
PRESET_IMPORT = NEXT_LAYER / "src/qt/src/PresetImport.cpp"
LOOP_AXES = ("x", "y", "z", "e")
LOOP_PREFIXES = ("machine_max_feedrate_", "machine_max_acceleration_",
                 "machine_max_jerk_")
PER_TOOL_EXTRAS = ("nozzle_diameter",)
HFW_PROFILES = HFW / "profiles"
ORCA_CONFIG = Path.home() / ".config/OrcaSlicer"

EMIT_PREFIXES = ("start_gcode", "end_gcode", "before_layer_gcode", "layer_gcode",
                 "toolchange_gcode", "between_objects_gcode", "color_change_gcode",
                 "pause_print_gcode", "template_custom_gcode", "custom_parameters")
# Keys this engine defines itself: nothing upstream lists them, so the ledger
# would otherwise hide a setting the engine actually applies.
ENGINE_ONLY = {"gantry_height"}

# Every capability libslic3r has that this engine does not, with what closing it
# costs and what it is waiting on. `state` is `unbuilt` for work that is planned,
# `blocked` for work that cannot start, and `excluded` for the one process the owner
# has ruled out. Each row also reaches the application's panel, which shows the
# control as unavailable until the row is deleted.
#
# The owner's rule, quoted: everything libslic3r has, in Rust, except the tool
# changer and MMU (no hardware) and SLA/MSLA (a different process).
BACKLOG = (
    {
        "id": "text",
        "title": "Text embossing and font loading",
        "state": "unbuilt",
        "panel": "unavailable",
        "cost": "part built: `crates/font` reads TrueType outlines and measured against "
                "FreeType, `crates/text` extrudes them into a closed prism, `hfw-cli "
                "--text` slices one, and `hfw_text_mesh` hands the same prism to the "
                "application as a mesh handle to add as a part or a negative volume. "
                "Left: kerning and shaping (advances are plain `hmtx` widths), placing "
                "text on an object's surface, and CFF-flavoured fonts",
        "note": "`docs/TEXT.md` is the design note and the measured state.",
    },
    {
        "id": "cut_hollow",
        "title": "Cut, hollow and split",
        "state": "unbuilt",
        "panel": "unavailable",
        "cost": "a plane-mesh split that caps both halves, and hollowing as an inward "
                "shell with a wall thickness and drain holes",
        "note": "A cut or hollowed body has to arrive that way today.",
    },
    {
        "id": "nesting",
        "title": "Nesting arranger",
        "state": "unbuilt",
        "panel": "unavailable",
        "cost": "interlocking 2D packing with rotations and a search; the shelf packer "
                "here fills rows and turns a part a quarter turn, so a bed whose parts "
                "only fit interlocked is refused",
        "note": "`hfw_plate_arrange` is the shelf packer, and it stays as the fast path.",
    },
    {
        "id": "amf_import",
        "title": "AMF reading",
        "state": "unbuilt",
        "panel": "unavailable",
        "cost": "an XML model with vertices, triangles and materials — small next to "
                "the 3MF reader already here",
        "note": "STL, 3MF and OBJ are read; AMF is not.",
    },
    {
        "id": "internal_bridges",
        "title": "Internal bridges",
        "state": "unbuilt",
        "panel": "unavailable",
        "cost": "detect a bridge whose support is infill rather than air, and give it "
                "the bridging flow and direction; the external bridge path exists",
        "note": "OrcaSlicer's `internal_bridge_*` settings assume it.",
    },
    {
        "id": "tiled_distance_field",
        "title": "Tiled distance field",
        "state": "unbuilt",
        "panel": "unavailable",
        "cost": "tile the per-layer mask so a 200 mm plate at 0.05 mm stops being "
                "refused for memory (~450 MB today)",
        "note": "A large thin plate at a fine layer height is refused rather than "
                "slowed.",
    },
    {
        "id": "machine_placeholders",
        "title": "Vendor G-code placeholders",
        "state": "unbuilt",
        "panel": "available",
        "cost": "substitute the dialect's whole placeholder set "
                "(`[bed_temperature_initial_layer_single]`, "
                "`{during_print_exhaust_fan_speed_num[current_extruder]}` and the rest) "
                "in start and end G-code",
        "note": "A real vendor machine preset leaves them literal today and the "
                "verifier rejects the file.",
    },
    {
        "id": "slicing_modes",
        "title": "Slicing mode and hole closing",
        "state": "unbuilt",
        "panel": "unavailable",
        "cost": "an even-odd and a close-holes contour policy beside the regular one",
        "note": "`slicing_mode` is the one unapplied setting in this group.",
    },
    {
        "id": "step_import",
        "title": "STEP and IGES import",
        "state": "blocked",
        "panel": "unavailable",
        "cost": "B-rep tessellation needs an OCCT-class kernel, which is the one "
                "dependency this project has refused so far",
        "blocked_on": "the dependency decision: a C++ CAD kernel in the build",
        "note": "Nothing here reads a parametric solid.",
    },
    {
        "id": "differential_slice",
        "title": "A gated corpus against libslic3r",
        "state": "unbuilt",
        "panel": "available",
        "cost": "drive libslic3r through its own API on a fixed corpus and diff the "
                "toolpaths, rather than comparing exported G-code on six fixtures",
        "note": "`scripts/oracle.py` is the fixture-level version of this.",
    },
    {
        "id": "tool_paint",
        "title": "Tool paint: a facet's extruder",
        "state": "blocked",
        "panel": "unavailable",
        "cost": "a third paint channel, consumed by stack assignment",
        "blocked_on": "the tool changer",
        "note": "`Paint` carries support and seam only: a channel nothing reads is a "
                "control that does nothing.",
    },
    {
        "id": "mmu",
        "title": "Multi-material: MMU, AMS, wipe tower",
        "state": "blocked",
        "panel": "unavailable",
        "cost": "per-facet tool assignment, strand ordering, purge and a wipe tower, "
                "and the 56 settings in `docs/SETTINGS-GAPS.md`'s multi-material bucket",
        "blocked_on": "the owner has no tool changer or MMU yet",
        "note": "Deferred by request, not declined.",
    },
    {
        "id": "sla",
        "title": "Resin (SLA/MSLA) slicing",
        "state": "excluded",
        "panel": "unavailable",
        "cost": "a second pipeline: exposure masks per layer, resin supports, and a "
                "writer that commands light instead of extrusion",
        "blocked_on": "the owner has ruled it out — a different process, not a gap",
        "note": "Ruled out by the owner: the pipeline here is filament.",
    },
)

# Settings that belong to the multi-material work, which is blocked rather than
# planned. Membership is a name rule so the backlog can separate "waiting on the tool
# changer" from "to be built" without a second taxonomy.
TOOLCHANGE_MARKERS = (
    "mmu", "wipe_tower", "extruder", "single_extruder", "toolchange", "multitool",
    "filament_loading", "filament_unloading", "filament_stamping", "filament_ramming",
    "filament_cooling", "filament_change_time", "filament_purge", "filament_flush",
    "multimaterial_purging", "purge", "wiping_volumes", "wipe_into",
    "standby_temperature", "ooze_prevention", "parking_pos", "extra_loading_move",
    "interlocking",
    "high_current_on_filament_swap", "cooling_tube",
    "enable_pressure_advance_during_ramming",
)

EMIT_NAMES = {
    "thumbnails_format", "remaining_times",
    "gcode_flavor", "label_objects",
    "use_relative_e_distances", "silent_mode",
    "single_extruder_multi_material",
    "variable_layer_height", "prefer_clockwise_movements", "filament_change_time",
}
META_PREFIXES = ("filament_cost", "filament_density", "filament_spool_weight",
                 "filament_type", "filament_soluble", "filament_abrasive",
                 "filament_vendor", "filament_settings_id", "default_filament_profile",
                 "compatible_printers")
META_NAMES = {"filament_diameter", "nozzle_diameter", "printer_model", "printer_variant",
              "printer_settings_id", "print_settings_id", "inherits", "preset_name",
              "bed_shape", "bed_custom_model", "bed_custom_texture",
              "thumbs_format", "printer_technology", "max_print_height",
              "z_offset", "extruder_clearance_height", "extruder_clearance_radius"}
ALGO_NAMES = {
    "ensure_vertical_shell_thickness", "thin_walls", "gap_fill_enabled",
    "support_material_style", "arc_fitting", "scarf_seam_placement",
    "scarf_seam_only_on_smooth", "scarf_seam_entire_loop",
    
    "extra_perimeters", "extra_perimeters_on_overhangs", "overhangs",
    "avoid_crossing_perimeters", "avoid_crossing_perimeters_max_detour",
    "avoid_crossing_curled_overhangs", "elefant_foot_compensation",
    "thick_bridges", "bridge_flow_ratio", "automatic_infill_combination",
    "infill_anchor", "infill_anchor_max", "infill_every_layers",
    "solid_infill_every_layers", "perimeter_generator", "min_feature_size",
    "wall_transition_angle", "wall_transition_filter_deviation",
    "wall_transition_length", "wall_distribution_count", "xy_size_compensation",
    "slice_closing_radius", "resolution", "gcode_resolution",
    "enable_dynamic_overhang_speeds", "enable_dynamic_fan_speeds",
    "enable_overhang_bridge_fan", "cooling_slowdown_logic",
    "cooling_perimeter_transition_distance",                 "filament_shrinkage_compensation_xy", "filament_shrinkage_compensation_z",
    
    "ironing", "ironing_type", "ironing_flowrate", "ironing_spacing", "ironing_speed",
    "top_one_perimeter_type", "only_one_perimeter_first_layer",
    "enable_pressure_advance_during_ramming", "multimaterial_purging",
}
NOT_APPLICABLE_PREFIXES = ("colour_mix", "color_mix", "sla_", "milling_", "emboss_",
                           "spiral_vase_")
NOT_APPLICABLE_NAMES = {
    "stuck_filament_detection", "binary_gcode",
    "support_material_synchronize_layers",
    "printer_technology", "machine_limits_usage", "bed_custom_model",
    "bed_custom_texture", "thumbnails_format", "preset_names", "profile_vendor",
    "profile_version", "preset_name",
}

# Engine settings that mean the same thing as a native key already listed above,
# under a second name. PrusaSlicer and OrcaSlicer both kept the old spelling
# alive next to the new one, so a profile can write either. Kept separate from
# HFW_ALIAS because that table is one engine setting per native key and this is
# the other direction: several engine names, one key.
HFW_SYNONYMS = {
    "gap_fill_speed": ("thin_speed", "applied", ""),
    "external_perimeter_acceleration": ("outer_wall_accel", "applied", ""),
    "solid_infill_acceleration": ("solid_accel", "applied", ""),
    "first_layer_acceleration": ("initial_layer_accel", "applied", ""),
    "support_material_angle": ("support_threshold_angle", "applied", "the threshold angle's config-file name"),
    "variable_layer_height": ("variable_layer_height", "applied", "gates the profile; without it the stack is uniform"),
    "spiral_vase": ("spiral_vase", "applied", "one wall per layer, rising continuously"),

    "min_layer_height": ("min_layer_height", "applied", "the profile's floor, first extruder's value"),
    "max_layer_height": ("max_layer_height", "applied", "the profile's ceiling, first extruder's value"),
    "infill_acceleration": ("infill_accel", "applied",
                             "sparse, solid and top skin all fall back to it"),
    "pressure_advance_value": (
        "pressure_advance",
        "applied",
        "the figure the engine keeps apart from its on/off switch; this engine reads "
        "both spellings into one value",
    ),
}

HFW_ALIAS = {
    "name": (None, "extension", "preset name, not a config key"),
    "bed_x": (None, "extension", "the outline's span; a profile with bed_shape sets it"),
    "bed_y": (None, "extension", "the outline's span; a profile with bed_shape sets it"),
    "max_z": ("max_print_height", "applied", ""),
    "origin_center": (None, "extension", "bed-origin convention, not a setting"),
    "flavor": ("gcode_flavor", "applied", ""),
    "relative_e": ("use_relative_e_distances", "applied", ""),
    "nozzle_diameter": ("nozzle_diameter", "applied", ""),
    "filament_diameter": ("filament_diameter", "applied", ""),
    "filament_density": ("filament_density", "applied", ""),
    "line_width": ("extrusion_width", "applied", ""),
    "min_bead_width": ("min_bead_width", "applied", ""),
    "layer_height": ("layer_height", "applied", ""),
    "first_layer_height": ("first_layer_height", "applied", ""),
    "walls": ("perimeters", "applied", ""),
    "infill_density": ("fill_density", "applied", ""),
    "infill_pattern": (
        "fill_pattern",
        "partial",
        "rectilinear, grid, line, triangle, cubic, gyroid, 3d_honeycomb, "
        "concentric, zig-zag, cross-hatch, honeycomb (a flat hexagonal lattice, "
        "distinct from the three-dimensional cell) and monotonic are all drawn; "
        "lightning is reported rather than printed, because it is a spanning tree "
        "grown down from the top surface rather than a fill of the layer's area",
    ),
    "top_layers": ("top_solid_layers", "applied", ""),
    "bottom_layers": ("bottom_solid_layers", "applied", ""),
    "nozzle_temp": ("temperature", "applied", ""),
    "first_nozzle_temp": ("first_layer_temperature", "applied", ""),
    "bed_temp": ("bed_temperature", "applied", ""),
    "fan_speed": ("max_fan_speed", "applied", ""),
    "fan_start_layer": ("disable_fan_first_layers", "applied", ""),
    "retract_len": ("retract_length", "applied", ""),
    "retract_speed": ("retract_speed", "applied", ""),
    "retract_min_travel": ("retract_before_travel", "applied", ""),
    "infill_retract_min_travel": (None, "extension", "no dialect equivalent"),
    "z_hop": ("retract_lift", "applied", ""),
    "travel_speed": ("travel_speed", "applied", ""),
    "first_layer_speed": ("first_layer_speed", "applied", ""),
    "outer_wall_speed": ("external_perimeter_speed", "applied", ""),
    "inner_wall_speed": ("perimeter_speed", "applied", ""),
    "thin_speed": ("small_perimeter_speed", "applied", ""),
    "infill_speed": ("infill_speed", "applied", ""),
    "solid_speed": ("solid_infill_speed", "applied", ""),
    "support_speed": ("support_material_speed", "applied", ""),
    "support_interface_speed": ("support_material_interface_speed", "applied", ""),
    "support": ("support_material", "applied", ""),
    "support_threshold_angle": ("support_material_threshold", "applied", ""),
    "support_xy_gap": ("support_material_xy_spacing", "applied", ""),
    "support_z_gap": ("support_material_contact_distance", "applied", ""),
    "support_interface_layers": ("support_material_interface_layers", "applied", ""),
    "support_density": (None, "extension",
                        "a fraction of the bead width, for profiles that prefer it"),
    "support_interface_density": (None, "extension",
                                  "a fraction of the bead width, for profiles that prefer it"),
    "support_on_build_plate_only": ("support_material_buildplate_only", "applied", ""),
    "support_min_area": (None, "extension", "no dialect equivalent"),
    "support_style": ("support_material_style", "partial", "normal and tree only"),
    "support_branch_angle": ("support_tree_angle", "applied", ""),
    "support_branch_distance": ("support_tree_branch_distance", "applied", ""),
    "support_tip_diameter": ("support_tree_tip_diameter", "applied", ""),
    "support_branch_diameter_angle": ("support_tree_branch_diameter_angle",
                                      "applied", ""),
    "support_max_branch_diameter": ("support_tree_branch_diameter", "applied", ""),
    "max_volumetric_speed": ("max_volumetric_speed", "applied", ""),    "default_accel": ("default_acceleration", "applied", ""),
    "outer_wall_accel": ("outer_wall_acceleration", "applied", ""),
    "inner_wall_accel": ("inner_wall_acceleration", "applied", ""),
    "initial_layer_accel": ("initial_layer_acceleration", "applied", ""),
    "solid_accel": ("internal_solid_infill_acceleration", "applied", ""),
    "infill_accel": ("sparse_infill_acceleration", "applied", ""),
    "support_accel": ("support_material_acceleration", "applied", ""),
    "support_interface_accel": ("support_material_interface_acceleration", "applied", ""),
    "machine_max_accel_x": ("machine_max_acceleration_x", "applied", ""),
    "machine_max_accel_y": ("machine_max_acceleration_y", "applied", ""),
    "machine_max_accel_z": ("machine_max_acceleration_z", "applied", ""),
    "machine_max_accel_e": ("machine_max_acceleration_e", "applied", ""),
    "machine_max_feedrate_x": ("machine_max_feedrate_x", "applied", ""),
    "machine_max_feedrate_y": ("machine_max_feedrate_y", "applied", ""),
    "machine_max_feedrate_z": ("machine_max_feedrate_z", "applied", ""),
    "machine_max_feedrate_e": ("machine_max_feedrate_e", "applied", ""),
    "machine_max_jerk_x": ("machine_max_jerk_x", "partial",
                           "Marlin M205 only; Klipper has no jerk limit"),
    "machine_max_jerk_y": ("machine_max_jerk_y", "partial",
                           "Marlin M205 only; Klipper has no jerk limit"),
    "machine_max_jerk_z": ("machine_max_jerk_z", "partial",
                           "Marlin M205 only; Klipper has no jerk limit"),
    "machine_max_jerk_e": ("machine_max_jerk_e", "partial",
                           "Marlin M205 only; Klipper has no jerk limit"),
    "min_fan_speed": ("min_fan_speed", "applied", ""),
    "full_fan_speed_layer": ("full_fan_speed_layer", "applied", ""),
    "fan_below_layer_time": ("fan_below_layer_time", "applied", ""),
    "slow_down_layer_time": ("slow_down_layer_time", "applied", ""),
    "slow_down_min_speed": ("slow_down_min_speed", "applied", ""),
    "skirt_height": ("skirt_height", "applied", ""),
    "idle_temp": ("idle_temperature", "applied", ""),
    "wall_width": ("perimeter_extrusion_width", "applied", ""),
    "outer_wall_width": ("external_perimeter_extrusion_width", "applied",
                         "outer bead width; the loop ladder shifts with it"),
    "infill_width": ("infill_extrusion_width", "applied", ""),
    "solid_infill_width": ("solid_infill_extrusion_width", "applied", ""),
    "support_width": ("support_material_extrusion_width", "applied", ""),
    "support_interface_width": (None, "extension", "no dialect equivalent"),
    "top_solid_speed": ("top_solid_infill_speed", "applied", ""),
    "top_solid_accel": ("top_solid_infill_acceleration", "applied", ""),
    "perimeter_accel": ("perimeter_acceleration", "applied", ""),
    "travel_speed_z": ("travel_speed_z", "applied", ""),
    "first_layer_outer_speed": ("first_layer_external_perimeter_speed", "applied", ""),
    "first_layer_perimeter_speed": ("first_layer_perimeter_speed", "applied", ""),
    "first_layer_inner_speed": ("first_layer_perimeter_speed", "partial",
                                "inner walls only; the outer wall has its own key"),
    "first_layer_thin_speed": ("first_layer_gap_fill_speed", "applied", ""),
    "first_layer_infill_speed": ("first_layer_infill_speed", "applied", ""),
    "first_layer_solid_speed": ("first_layer_solid_infill_speed", "applied", ""),
    "first_layer_top_solid_speed": ("first_layer_top_solid_infill_speed", "applied", ""),
    "first_layer_support_speed": ("first_layer_support_material_speed", "applied", ""),
    "cooling": ("cooling", "applied", "false emits no M106 at all"),
    "fan_always_on": ("fan_always_on", "applied", ""),
    "initial_layer_fan_speed": ("initial_layer_fan_speed", "applied", ""),
    "ironing_fan_speed": ("ironing_fan_speed", "applied", ""),
    "support_interface_fan_speed": ("support_material_interface_fan_speed", "applied", ""),
    "dynamic_fan_speeds": ("enable_dynamic_fan_speeds", "applied",
                           "fan duty per overhang quarter, from the measured overhang share"),
    "dynamic_overhang_speeds": ("enable_dynamic_overhang_speeds", "applied",
                                "speed per overhang quarter, from the measured overhang share"),
    "first_layer_bed_temp": ("first_layer_bed_temperature", "applied", ""),
    "chamber_temp": ("chamber_temperature", "applied", "M141; a machine without a chamber ignores it"),
    "arc_fitting": ("arc_fitting", "applied", "needs ARC_SUPPORT or [gcode_arcs]"),
    "ironing": ("ironing", "applied", ""),
    "ironing_flow": ("ironing_flowrate", "applied", ""),
    "ironing_spacing": ("ironing_spacing", "applied", ""),
    "ironing_speed": ("ironing_speed", "applied", ""),
    "ironing_all_solid": ("ironing_type", "partial",
                          "top surface or every solid layer; no no-top-surfaces mode"),
    "filament_max_volumetric_speed": ("filament_max_volumetric_speed", "applied",
                                      "the filament's own ceiling, enforced together with the machine's"),
    "filament_infill_max_speed": ("filament_infill_max_speed", "applied", ""),
    "filament_infill_max_crossing_speed": ("filament_infill_max_crossing_speed", "applied",
                                           "the cap an infill line meets by default, because infill anchors into the shell"),
    "over_bridge_speed": ("over_bridge_speed", "applied",
                          "solid infill whose midpoint lands on a bridge of the layer below"),
    "internal_bridge_fan_speed": ("internal_bridge_fan_speed", "partial",
                                  "read and applied to every bridge, because the writer cannot tell a buried bridge from a visible one"),
    "enable_overhang_bridge_fan": ("enable_overhang_bridge_fan", "applied", ""),
    "retract_before_wipe": ("retract_before_wipe", "partial",
                            "read as a switch: non-zero retracts then wipes, zero wipes then retracts. A percentage in between is not a partial wipe here"),
    "only_retract_when_crossing_perimeters": ("only_retract_when_crossing_perimeters", "applied", ""),
    "raft_expansion": ("raft_expansion", "applied",
                       "each raft layer above the first grows past the part outline by the value"),
    "first_layer_speed_over_raft": ("first_layer_speed_over_raft", "applied",
                                    "caps the layer printed on top of the raft"),
    "first_layer_acceleration_over_raft": ("first_layer_acceleration_over_raft", "applied", ""),
    "only_one_perimeter_first_layer": ("only_one_perimeter_first_layer", "applied", ""),
    "top_one_perimeter_type": ("top_one_perimeter_type", "applied",
                               "none, top or topmost"),
    "travel_acceleration": ("travel_acceleration", "applied",
                            "wrapped round each travel with a save and a restore"),
    "travel_short_distance_acceleration": ("travel_short_distance_acceleration", "applied",
                                           "travels under a millimetre"),
    "machine_limits_usage": ("machine_limits_usage", "applied",
                             "emit_to_gcode writes the envelope, time_estimate_only clamps the speeds without writing it, ignore does neither"),
    "stuck_filament_detection": ("stuck_filament_detection", "applied", "M591 D0 S1"),
    "shrinkage_xy": ("filament_shrinkage_compensation_xy", "applied",
                     "scales the model up by 100/(100-shrink) in X and Y before slicing"),
    "shrinkage_z": ("filament_shrinkage_compensation_z", "applied",
                    "scales the model in Z before slicing, so the layer schedule follows"),
    "perimeter_generator": ("perimeter_generator", "partial",
                            "classic selects fixed-width offsets and arachne selects this engine's medial-axis beading, which is arachne-class but not Arachne"),
    "wall_transition_length": ("wall_transition_length", "applied", ""),
    "wall_transition_angle": ("wall_transition_angle", "partial",
                              "applied as the sharp-corner reinsertion angle: this engine places beads from the medial axis, so Arachne's transition angle has no exact counterpart"),
    "wall_transition_filter_deviation": ("wall_transition_filter_deviation", "partial",
                                         "applied as the bead-count hysteresis, one hundredth of the percentage in beads"),
    "overhangs": ("overhangs", "applied",
                  "gates the overhang measurement that feeds the overhang speeds and fan"),
    "chamber_minimal_temperature": ("chamber_minimal_temperature", "applied",
                                    "waits with M191 before the first layer"),
    "pressure_advance_value": ("pressure_advance", "applied",
                               "SET_PRESSURE_ADVANCE for Klipper, M900 for Marlin"),    "top_infill_width": ("top_infill_extrusion_width", "applied",
                         "the top skin is drawn with its own bead width while the shell"
                         " layers under it keep the solid width"),
    "first_layer_width": ("first_layer_extrusion_width", "applied",
                          "the whole first layer moves to the new width: walls, skin, infill"
                          " and brim, with every width-derived threshold scaled to match"),
    "deretract_speed": ("deretract_speed", "applied", ""),
    "retract_restart_extra": ("retract_restart_extra", "applied", ""),
    "retract_layer_change": ("retract_layer_change", "applied", ""),
    "wipe": ("wipe", "applied", ""),
    "wipe_distance": ("wipe_distance", "applied", ""),
    "max_speed": ("max_print_speed", "applied", ""),
    "min_layer_time": ("slowdown_below_layer_time", "applied",
                       "the gate below which a layer is stretched, and the time it is"
                       " stretched to"),
    "min_speed_factor": ("min_print_speed", "partial",
                         "a factor, where the dialect states a speed; min_print_speed is"
                         " the dialect key and overrides it when set"),
    "flow": ("extrusion_multiplier", "applied", ""),
    "seam": ("seam_position", "applied",
             "all four; random scatters deterministically from the loop, and aligned keeps"
             " each loop's seam where the loop below put it"),
    "inner_walls_first": ("external_perimeters_first", "partial",
                          "inverted meaning: the dialect flag puts the outer wall first,"
                          " and the parser stores it inverted"),
    "avoid_crossing_curled_overhangs": ("avoid_crossing_curled_overhangs", "applied",
                                        "routes around this layer's overhanging beads, which"
                                        " needs overhang measurement switched on"),
    "travel_lift_before_obstacle": ("travel_lift_before_obstacle", "applied",
                                    "lifts a travel that crosses an overhanging bead; also"
                                    " needs overhang measurement"),
    "cooling_slowdown_logic": ("cooling_slowdown_logic", "applied",
                               "proportional, uniform and consistent_surface"),
    "brim_loops": ("brim_width", "partial",
                   "a width in millimetres, rounded to whole loops; brim_width is the"
                   " dialect key and wins when set"),
    "skirt_loops": ("skirts", "applied", ""),
    "skirt_distance": ("skirt_distance", "applied", ""),
    "brim_loops": ("brim_width", "partial", "loops vs millimetres"),
    "brim_gap": (None, "extension", "no dialect equivalent"),
    "brim_type": ("brim_type", "applied", "all four sides"),
    "brim_separation": ("brim_separation", "applied", ""),
    "min_skirt_length": ("min_skirt_length", "applied", "extends the loop count"),
    "raft_layers": ("raft_layers", "applied", "convex-hull pad under the part"),
    "raft_first_layer_density": ("raft_first_layer_density", "applied", ""),
    "raft_first_layer_expansion": ("raft_first_layer_expansion", "applied", ""),
    "raft_contact_distance": ("raft_contact_distance", "applied", ""),
    "draft_shield": ("draft_shield", "applied", "enabled and limited"),
    "draft_shield_height": (None, "extension", "height a limited shield stops at"),
    "travel_slope": ("travel_slope", "applied", ""),
    "travel_ramping_lift": ("travel_ramping_lift", "applied", ""),
    "travel_max_lift": ("travel_max_lift", "applied", ""),
    "retract_lift_above": ("retract_lift_above", "applied", ""),
    "retract_lift_below": ("retract_lift_below", "applied", ""),
    "seam_gap": ("seam_gap_distance", "applied", ""),
    "staggered_inner_seams": ("staggered_inner_seams", "applied", ""),
    "scarf_seam_length": ("scarf_seam_length", "applied", ""),
    "scarf_seam_max_segment": ("scarf_seam_max_segment_length", "applied", ""),
    "scarf_seam_inner": ("scarf_seam_on_inner_perimeters", "applied", ""),
    "scarf_seam_start_height": ("scarf_seam_start_height", "applied", ""),
    "fuzzy_skin": ("fuzzy_skin", "applied", "none, external, all"),
    "fuzzy_skin_thickness": ("fuzzy_skin_thickness", "applied", ""),
    "fuzzy_skin_point_dist": ("fuzzy_skin_point_dist", "applied", ""),
    "support_pattern": ("support_material_pattern", "partial", "rectilinear and grid"),
    "support_first_layer_density": ("support_material_first_layer_density", "applied", ""),
    "support_interface_pattern": ("support_material_interface_pattern", "applied", ""),
    "support_interface_loops": ("support_material_interface_contact_loops", "applied", "one traced loop per mask"),
    "support_interface_shells": ("interface_shells", "applied", "two traced loops"),
    "support_closing_radius": ("support_material_closing_radius", "applied", "close before projection"),
    "support_bottom_contact_distance": ("support_material_bottom_contact_distance", "applied", "gap at a column's bottom"),
    "support_skip_bridges": ("dont_support_bridges", "applied", "anchored spans are left alone"),
    "support_sheath": ("support_material_with_sheath", "applied", ""),
    "support_first_layer_expansion": ("support_material_first_layer_expansion", "applied", ""),
    "support_enforce_layers": ("support_material_enforce_layers", "applied", ""),
    "support_bottom_interface_layers": ("support_material_bottom_interface_layers", "applied", ""),
    "support_tree_angle_slow": ("support_tree_angle_slow", "applied", ""),
    "support_tree_top_rate": ("support_tree_top_rate", "applied", ""),
    "support_tree_double_wall": ("support_tree_branch_diameter_double_wall", "applied", "doubles the trunk ceiling"),
    "bridge_speed": ("bridge_speed", "applied", ""),
    "bridge_flow": ("bridge_flow_ratio", "applied", ""),
    "bridge_density": (None, "extension", "no dialect equivalent"),
    "bridge_line_width": (None, "extension", "no dialect equivalent"),
    "bridge_angle": ("bridge_angle", "applied", ""),
    "thick_bridges": ("thick_bridges", "applied", ""),
    "bridge_cross_hatch": (None, "extension", "no dialect equivalent"),
    "bridge_fan_speed": ("bridge_fan_speed", "applied", ""),
    "bridge_acceleration": ("bridge_acceleration", "applied", ""),
    "max_bridge_length": (None, "extension", "no dialect equivalent"),
    "bridge_span_flow": (None, "extension", "no dialect equivalent"),
    "bridge_span_speed": (None, "extension", "no dialect equivalent"),
    "bridge_span_ref": (None, "extension", "no dialect equivalent"),
    "emit_print_stats": (None, "extension", "M73/M117 emission"),
    "gcode_comments": ("gcode_comments", "applied", "gates ;TYPE and ;WIDTH"),
    "label_objects": ("gcode_label_objects", "applied", ";OBJECT label"),
    "gcode_resolution": ("gcode_resolution", "applied", "coordinate rounding"),
    "volumetric_e": ("use_volumetric_e", "applied", "E in mm3 plus M200"),
    "firmware_retraction": ("use_firmware_retraction", "applied", "G10/G11 plus M207"),
    "clockwise": ("prefer_clockwise_movements", "applied", "reverses closed loops"),
    "remaining_times": ("remaining_times", "applied", "M73 per layer"),
    "autoemit_temperatures": ("autoemit_temperature_commands", "applied", ""),
    "before_layer_gcode": ("before_layer_gcode", "applied", "layer placeholders filled"),
    "layer_gcode": ("layer_gcode", "applied", "layer placeholders filled"),
    "start_filament_gcode": ("start_filament_gcode", "applied", "one filament"),
    "end_filament_gcode": ("end_filament_gcode", "applied", "one filament"),
    "thumbnails": ("thumbnails", "applied", "PNG and QOI, rendered from the toolpaths"),
    "exclude_object": ("exclude_object", "applied", ""),
    "pressure_advance": ("pressure_advance", "applied", "M900 K for Marlin too"),
    "start_gcode": ("start_gcode", "applied", ""),
    "end_gcode": ("end_gcode", "applied", ""),
}

TIER_RULES = """\
Tier 0e — emit only: the value is a string or flag written into the output file.
Tier 0m — display metadata: price, density, spool weight; the G-code does not change.
Tier 0p — plumbing: printer geometry, firmware dialect, extruder counts.
Tier 1 — constant: a number threaded into a decision HFW already makes.
Tier 2 — algorithm: requires slicing work beyond threading a value.
Tier n/a — not applicable to a Klipper FDM target (SLA, colour mixing, Bambu fork).\
Tier int — not surfaced by any UI and not read here; excluded, with the reason on the row.\
"""

ADD_RE = re.compile(r'def\s*=\s*defs\.add\(\s*"([a-z0-9_]+)",\s*typeid\(([^)]*)\)\s*\)')
LEGACY_ADD_RE = re.compile(r'def\s*=\s*this->add\(\s*"([a-z0-9_]+)",\s*(co[A-Za-z]+)\s*\)')


LAYOUT = NEXT_LAYER / "src/qt/assets/settings-layout.json"


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return re.sub(r"//[^\n]*", "", text)


def app_surface():
    if not LAYOUT.exists():
        return None
    layout = json.loads(LAYOUT.read_text(encoding="utf-8", errors="replace"))
    keys = set()
    for pages in layout.get("tabs", {}).values():
        for page in pages:
            for section in page.get("sections", []):
                keys.update(section.get("settings", []))
    return keys


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


def parse_modern(path: Path):
    text = strip_comments(path.read_text(encoding="utf-8", errors="replace"))
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
            "source": path.name,
        }
    return entries


def parse_legacy(path: Path):
    text = path.read_text(encoding="utf-8", errors="replace")
    types: dict[str, str] = {}
    for name, ctype in LEGACY_ADD_RE.findall(text):
        types.setdefault(name, ctype)
    return set(types), types


def clean_default(raw: str) -> str:
    flat = " ".join(raw.split())
    flat = flat.replace("std::string(", "").replace("std::vector<std::string>", "")
    if len(flat) > 48:
        flat = flat[:45] + "..."
    return flat


def hfw_keys():
    keys = set()
    if HFW_PARSE.exists():
        keys.update(re.findall(r'^\s+"([a-z0-9_]+)" =>', HFW_PARSE.read_text(), re.M))
    for profile in HFW_PROFILES.glob("*.cfg"):
        for line in profile.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            keys.add(line.split("=", 1)[0].strip())
    return {key for key in keys if re.fullmatch(r"[a-z0-9_]+", key)}


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


def orca_keys():
    if not ORCA_CONFIG.is_dir():
        return None
    found = set()
    for path in ORCA_CONFIG.glob("**/*.json"):
        try:
            data = json.loads(path.read_text(encoding="utf-8", errors="replace"))
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


def loop_generated(path: Path):
    text = path.read_text(encoding="utf-8", errors="replace")
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
                "source": path.name,
            }
    return entries


def parse_foreign(path: Path):
    if not path.exists():
        return {}
    pairs = re.findall(
        r'\{QStringLiteral\("([a-z0-9_]+)"\),\s*QStringLiteral\("([a-z0-9_]+)"\)\}',
        path.read_text(encoding="utf-8", errors="replace"),
    )
    return {foreign: engine for foreign, engine in pairs}


def parse_import(path: Path):
    if not path.exists():
        return {}
    pairs = re.findall(
        r'\{"([a-z0-9_]+)",\s*"([a-z0-9_]+)",\s*Transform::\w+,',
        path.read_text(encoding="utf-8", errors="replace"),
    )
    return {foreign: engine for foreign, engine in pairs}


def dialect_renames():
    """Orca/Bambu spelling -> engine spelling, from both proven tables.

    `PresetImport.cpp` carries the keys the application's own importer renames
    and `ForeignSettings.cpp` the ones its oracle gates. They overlap and
    disagree on four keys, so the gated table wins: `overhang_1_4_speed` and
    its neighbours are quarter-of-the-perimeter speeds whose index the two
    files number differently, and `infill_combination` means different layers
    there. A key only the importer renames is still a real rename -- without
    them a machine preset's `printable_area` and `machine_max_speed_x` never
    reach this engine.
    """
    pairs = parse_import(PRESET_IMPORT)
    pairs.update(parse_foreign(FOREIGN_SETTINGS))
    return pairs


def tier_for(name: str, entry: dict, surface: set | None) -> str:
    if surface is not None and name not in surface:
        return "int"
    if name in NOT_APPLICABLE_NAMES or name.startswith(NOT_APPLICABLE_PREFIXES):
        return "n/a"
    locations = entry["locations"].lower()
    if locations and "sla" in locations:
        return "n/a"
    if name.startswith(EMIT_PREFIXES) or name in EMIT_NAMES:
        return "0e"
    if name.startswith(META_PREFIXES) or name in META_NAMES:
        return "0m"
    if entry["type"] in ("std::string", "std::vector<std::string>") and "gcode" in name:
        return "0e"
    if name in ALGO_NAMES:
        return "2"
    if locations in ("printer", "project", "volume", "object"):
        return "0p"
    return "1"


# Keys HFW parses under the same name but honours only part of.
#
# `status_for` reports a same-named key as `applied` because the parser accepts
# it, which hides a partial implementation behind a passing check. These are the
# cases where the name is right and the behaviour is short, so the caveat has to
# come from a table rather than from the alias index.
HFW_CAVEATS = {
    "avoid_crossing_perimeters": (
        "partial",
        "on by default: the emitter routes the travel itself around the loops — the layer's perimeters are offset both ways by a bead of clearance, and a visibility graph over those offset rings yields a path that holds that clearance from every wall, emitted as one retracted chain of moves. A closed loop keeps the seam the profile chose unless the seam is `nearest`, so routing costs distance and never the seam; only an open path has its entry point moved to save the crossing. Partial because supports and curled overhangs from earlier layers are not obstacles, and a travel whose start or end sits against several nested walls can find no route at all and falls back to the straight move",
    ),
    "avoid_crossing_perimeters_max_detour": (
        "applied",
        "caps both the entry-point choice and the routed travel: a route longer than the straight move plus this budget is refused and the straight move is emitted instead. Zero means no limit, so the shipped profiles route without a ceiling",
    ),
    "infill_anchor": ("applied", ""),
    "infill_anchor_max": (
        "applied",
        "caps both the end that reaches into the shell and the segment that walks \
         the inner wall to join it to the next line",
    ),
    "elefant_foot_compensation": (
        "applied",
        "shrinks the first layer's cross-section by the value, on the printed outline, so walls, infill, skin and the brim all follow the compensated layer. The offset is taken on the whole region at once, so a hole smaller than twice the value closes instead of turning inside out. Where the offset merges or opens a feature is decided on a grid of one third of the value (0.02 mm floor), and every vertex is then walked onto the exact offset from the contour it came from",
    ),
    "xy_size_compensation": (
        "applied",
        "grows the object and closes its holes when positive, shrinks it and opens them when negative, on every layer, through the same whole-region offset as the elephant-foot correction. Applied to the sliced cross-section rather than to the mesh, which is where PrusaSlicer applies it too, so the layer height schedule is unaffected",
    ),

    "slice_closing_radius": (
        "applied",
        "fills cracks narrower than twice the value and drops holes smaller than it, on the sliced cross-section before compensation or simplification. Implemented by pulling the two boundary pieces together onto their midpoint rather than by an offset and back in, so a layer without a crack is left bit-identical instead of carrying a grid's error; the cost is that a concave corner is not rounded by the radius the way upstream's closing rounds it",
    ),
    "top_fill_pattern": (
        "applied",
        "all eight patterns are drawn: the five straight and concentric ones from "
        "this engine's own fill, and hilbertcurve, archimedeanchords and "
        "octagramspiral from upstream's plane-path curves clipped to the skin mask. "
        "monotonic and monotoniclines share one geometry because this engine's "
        "emitter decides the joining, not the fill",
    ),
    "bottom_fill_pattern": (
        "applied",
        "the same eight patterns as the top surface",
    ),
    "wall_distribution_count": (
        "applied",
        "spreads the difference between the outer loop's width and the inner ones "
        "over the band with upstream's parabolic weights, centred on the middle "
        "loop. This engine's beading already places each bead from the geometry "
        "through the medial axis, so the only difference it has to distribute is "
        "that outer-width one; one loop is the default and reproduces the "
        "undistributed band exactly",
    ),
    "solid_infill_below_area": (
        "applied",
        "each island is measured on its own, holes deducted, so a small boss on a "
        "large base prints solid while the base around it keeps its sparse fill",
    ),
    "max_volumetric_extrusion_rate_slope_positive": (
        "partial",
        "solved per bead path against the widest section on it, so a widening bead "
        "cannot push its own rate past the cap and a narrowing one pays its wide "
        "end's price. What is not done is what upstream does: splitting the move, "
        "so that two beads of different width meeting at a path boundary cannot "
        "step the rate between them. Measured on taper_wall, that boundary step "
        "reaches 6.2 mm3/s2 against a 4 mm3/s2 cap",
    ),
    "max_volumetric_extrusion_rate_slope_negative": (
        "partial",
        "the same solve and the same gap for the falling direction",
    ),
    "ensure_vertical_shell_thickness": (
        "partial",
        "disabled narrows the solid window to the immediate neighbours; partial and "
        "enabled coincide, because the difference upstream is a void region a coverage "
        "grid cannot express",
    ),
}


def status_for(name: str, hfw: set, alias_index: dict) -> tuple[str, str]:
    if name in hfw:
        if name in HFW_CAVEATS:
            status, note = HFW_CAVEATS[name]
            return status, f"{name} — {note}"
        return "applied", name
    if name in alias_index:
        hfw_key, status, note = alias_index[name]
        return status, f"{hfw_key}" + (f" — {note}" if note else "")
    return "missing", ""


def per_object_names() -> set:
    """The keys `Settings::apply_key` promises a settings panel may set per object."""
    if not PER_OBJECT.exists():
        return set()
    text = PER_OBJECT.read_text(encoding="utf-8", errors="replace")
    start = text.find("pub const PER_OBJECT_KEYS")
    if start < 0:
        return set()
    end = text.find("];", start)
    return set(re.findall(r'"([^"]+)"', text[start:end]))


def surface_document(rows, applicable, applied, partial, missing, alias_index) -> dict:
    """The JSON sidecar next-layer's settings panel reads.

    It carries the same measurement the markdown does, in the form a UI can
    enumerate: every setting's name, the key this engine uses for it, its
    location, type, default, unit and whether the value changes the G-code, plus
    the features this engine will not build, so the panel neither offers a control
    that does nothing nor hides one it could explain. The files are written by the
    same run so a panel and the ledger cannot disagree about a setting.
    """
    locations = sorted({row["locations"] for row in rows if row["locations"]})
    per_object = per_object_names()

    def settable(row) -> bool:
        if row["status"] != "applied":
            return False
        if row["name"] in per_object or row["note"] in per_object:
            return True
        native = alias_index.get(row["name"])
        return bool(native) and native[0] in per_object

    return {
        "surface": "hfw-settings",
        "version": 3,
        "generated_by": "tools/gen-settings-ledger.py",
        "counts": {
            "settings": len(rows),
            "applicable": len(applicable),
            "applied": len(applied),
            "partial": len(partial),
            "missing": len(missing),
            "not_applicable": len(rows) - len(applicable),
        },
        "statuses": ["applied", "partial", "missing", "n/a"],
        "capabilities": [dict(entry) for entry in BACKLOG],
        "locations": locations,
        "settings": [
            {
                "name": row["name"],
                "hfw": row["note"],
                "tier": row["tier"],
                "status": row["status"],
                "type": row["type"],
                "default": row["default"],
                "unit": row["unit"],
                "location": row["locations"] if row["locations"] not in ("", "?") else "Engine",
                "category": row["category"],
                "source": row["source"],
                "per_object": settable(row),
            }
            for row in sorted(rows, key=lambda item: item["name"])
        ],
    }


def backlog_document(rows) -> str:
    """The parity backlog: what libslic3r has, this engine does not, and at what cost."""
    states = [("unbuilt", "To be built"), ("blocked", "Blocked"), ("excluded", "Excluded")]
    out = [
        "# Parity backlog",
        "",
        "Generated by `tools/gen-settings-ledger.py` from the table the settings",
        "surface carries as `capabilities`. Do not edit by hand.",
        "",
        "The rule this list exists to serve: **everything libslic3r has, this engine",
        "has in Rust**, except the tool changer and MMU (no hardware yet) and SLA/MSLA",
        "(a different process). Nothing here is a decision to stay behind — every row",
        "either names the work that closes it or the thing it is waiting on.",
        "",
        "A capability is not a setting. The settings side of the same gap is measured",
        "below, from the same run that writes `SETTINGS-LEDGER.md`; the capability list",
        "is the work a setting cannot express.",
        "",
    ]
    for state, heading in states:
        entries = [entry for entry in BACKLOG if entry["state"] == state]
        if not entries:
            continue
        out += [f"## {heading}", ""]
        if state == "unbuilt":
            out += ["| capability | what closing it costs |", "| --- | --- |"]
            for entry in entries:
                out.append(f"| **{entry['title']}**<br>{entry['note']} | {entry['cost']} |")
        else:
            out += ["| capability | waiting on | what it costs |", "| --- | --- | --- |"]
            for entry in entries:
                waiting = entry.get("blocked_on", "—")
                out.append(f"| **{entry['title']}**<br>{entry['note']} | {waiting} | {entry['cost']} |")
        out.append("")

    applicable = [
        row for row in rows if row["status"] != "n/a" and row.get("tier") != "n/a"
    ]
    unapplied = [row for row in rows if row["status"] in ("missing", "partial")]
    not_scored = [row for row in unapplied if row not in applicable]

    def toolchange(row) -> bool:
        return any(marker in row["name"].lower() for marker in TOOLCHANGE_MARKERS)

    waiting = [row for row in unapplied if toolchange(row)]
    planned = [row for row in unapplied if not toolchange(row)]
    out += [
        "## Settings: libslic3r has them, this engine does not",
        "",
        f"{len(rows)} settings are defined by the reference dialect; {len(unapplied)} of",
        "them are not applied here.",
        "",
        f"The ledger declines to score {len(rows) - len(applicable)} rows as n/a — resin, milling, colour mixing",
        f"and printer identity — so its own unapplied total is {len(unapplied) - len(not_scored)} rather than {len(unapplied)}.",
        "",
        f"{len(waiting)} of the unapplied settings need hardware this machine does not have: a second",
        "extruder, or the filament loading path a single-extruder run never uses. The rest, by name:",
        "",
        "| setting | state | this engine's key |",
        "| --- | --- | --- |",
    ]
    for row in sorted(planned, key=lambda item: item["name"]):
        key = row["note"] or "—"
        out.append(f"| `{row['name']}` | {row['status']} | {key} |")
    out += [
        "",
        "Waiting on hardware — a second extruder, or a filament loading path the"
        " single-extruder pipeline never runs: "
        + ", ".join(f"`{row['name']}`" for row in sorted(waiting, key=lambda item: item["name"]))
        + ".",
        "",
        "`docs/SETTINGS-GAPS.md` buckets the same rows by what closing them costs; this",
        "list is by name so nothing hides inside a total.",
        "",
    ]
    return "\n".join(out)

def main() -> int:
    for required in (DOMAIN / "ConfigDefsFDM.cpp", HFW_PARSE):
        if not required.exists():
            print(f"missing input: {required}", file=sys.stderr)
            return 1

    entries = parse_modern(DOMAIN / "ConfigDefsFDM.cpp")
    entries.update(loop_generated(DOMAIN / "ConfigDefsFDM.cpp"))
    for name in PER_TOOL_EXTRAS:
        entries.setdefault(name, {
            "type": "std::vector<double>",
            "locations": "Tool",
            "category": "Extruder",
            "default": "0.4",
            "enums": [],
            "unit": "mm",
            "source": "ConfigBoxesFDM",
        })
    common = parse_modern(DOMAIN / "ConfigCommon.cpp")
    for name, entry in common.items():
        entry["source"] = "ConfigCommon.cpp"
        entry["category"] = entry["category"] or "Common"
        entries.setdefault(name, entry)

    legacy_names, legacy_types = parse_legacy(LEGACY)
    hfw = hfw_keys()
    orca = orca_keys()
    foreign = parse_foreign(FOREIGN_SETTINGS)

    alias_index = {}
    for hfw_key, (prusa_key, status, note) in HFW_ALIAS.items():
        if prusa_key:
            alias_index[prusa_key] = (hfw_key, status, note)
    for engine_name, (hfw_key, status, note) in HFW_SYNONYMS.items():
        alias_index.setdefault(engine_name, (hfw_key, status, note))

    rows = []
    surface = app_surface()
    for name, entry in entries.items():
        tier = tier_for(name, entry, surface)
        status, via = status_for(name, hfw, alias_index)
        if tier == "int" and status != "applied":
            status, via = "n/a", "not surfaced by any UI and not read here"
        if status == "applied" and via == name:
            via = name
        rows.append({
            "name": name,
            "tier": tier,
            "status": status,
            "type": short_type(entry["type"]),
            "default": entry["default"] or (" / ".join(entry["enums"]) or "—"),
            "unit": entry["unit"],
            "locations": normalize_locations(entry["locations"]),
            "category": entry["category"] or "Uncategorised",
            "note": via if via != name else "",
            "source": entry["source"],
        })

    ledger_names = {row["name"] for row in rows}
    for name in sorted((hfw - ledger_names) & (orca | legacy_names | ENGINE_ONLY)):
        rows.append({
            "name": name,
            "tier": "1",
            "status": "applied",
            "type": "engine",
            "default": "—",
            "unit": "",
            "locations": "",
            "category": "Engine only",
            "note": "accepted by this engine, absent from the upstream definition table",
            "source": "crates/config/src/parse.rs",
        })
    legacy_extra = sorted(
        name for name in legacy_names
        if name not in ledger_names and not name.startswith(("sla_", "milling_"))
    )
    orca_extra = sorted(orca - ledger_names) if orca else []

    categories = OrderedDict()
    for row in sorted(rows, key=lambda r: (r["category"], tier_rank(r["tier"]), r["name"])):
        categories.setdefault(row["category"], []).append(row)

    counts = Counter(row["status"] for row in rows)
    tiers = Counter(row["tier"] for row in rows)
    applicable = [
        r for r in rows if r["tier"] != "n/a" and r["status"] != "n/a"
    ]
    applied = [r for r in applicable if r["status"] == "applied"]
    partial = [r for r in applicable if r["status"] == "partial"]
    missing = [r for r in applicable if r["status"] == "missing"]

    out = []
    add = out.append
    add("# Settings ledger")
    add("")
    add("Generated by `tools/gen-settings-ledger.py`. Do not edit by hand: run the")
    add("script. Every number below is measured from the files named at the top of")
    add("that script on the machine it ran on.")
    add("")
    add("This is the checklist for one question: **of everything an engine is expected")
    add("to understand, what does HFW understand today?** It is not a wish list and it")
    add("is not a design. It is a countable inventory, so that progress is a number")
    add("instead of a feeling.")
    add("")
    add("Three rules come out of this file, and they are the point of it:")
    add("")
    add("1. **No surface may offer a setting the engine ignores.** A control that does")
    add("   nothing is worse than an absent one; the app hides or disables anything")
    add("   whose row here is not `applied`.")
    add("2. **`missing` is a to-do with a cost attached, never a permanent no.** Every")
    add("   row gets implemented, or gets a written reason in its tier.")
    add("3. **The finish line is 100% `applied` or `n/a`.** No row is deleted to make")
    add("   the percentage move.")
    add("")
    add("## What the columns mean")
    add("")
    add("| Column | Meaning |")
    add("|---|---|")
    add("| Setting | the name in the shared PrusaSlicer config dialect |")
    add("| Tier | Cost class, assigned by rule (see below) |")
    add("| Status | measured against HFW's parser and profiles today |")
    add("| HFW key | the key HFW uses for it, when the names differ |")
    add("| Type / Default / Unit | from the engine's definition table |")
    add("| Location | which config box it belongs to: Print, Filament, Printer, Tool, Object |")
    add("")
    add("Status values:")
    add("")
    add("| Status | Meaning |")
    add("|---|---|")
    add("| `applied` | HFW reads this name and the value changes the G-code |")
    add("| `partial` | HFW reads a related name but the semantics differ; the note says how |")
    add("| `missing` | HFW has no name for it and would reject the key |")
    add("| `n/a` | not applicable to a Klipper FDM target; excluded with a reason |")
    add("")
    add("Tier rules:")
    add("")
    add("```")
    add(TIER_RULES)
    add("```")
    add("")
    add("## Where the numbers stand")
    add("")
    add(f"- settings in the engine's FDM table: **{len(rows)}**")
    add(f"- applicable to a Klipper FDM target: **{len(applicable)}**")
    add(f"- `applied` in HFW today: **{len(applied)}** "
        f"({100.0 * len(applied) / max(1, len(applicable)):.0f}%)")
    add(f"- `partial`: **{len(partial)}**")
    add(f"- `missing`: **{len(missing)}**")
    add(f"- `n/a` (excluded with reason): **{len(rows) - len(applicable)}**")
    add(f"- legacy-table names not in the modern table: **{len(legacy_extra)}**")
    add(f"- names Orca profiles use that are not engine settings: "
        f"**{len(orca_extra)}**")
    add("")
    add("By tier, applicable only:")
    add("")
    add("| Tier | Count |")
    add("|---|---|")
    for tier in ("0e", "0m", "0p", "1", "2"):
        add(f"| {tier} | {tiers.get(tier, 0)} |")
    add("")
    add("Tier 2 is the whole of the hard work; tiers 0e, 0m, 0p and 1 are")
    add("mechanical. The number to watch is `applied` climbing while `missing`")
    add("falls to zero.")
    add("")
    add("## The whole table, by category")
    add("")
    for category, group in categories.items():
        missing_here = sum(1 for row in group if row["status"] == "missing")
        add(f"### {category} ({len(group)} settings, {missing_here} missing)")
        add("")
        add("| Setting | Tier | Status | HFW key | Type | Default | Unit | Location |")
        add("|---|---|---|---|---|---|---|---|")
        for row in group:
            default = escape_pipes(row["default"])
            note = escape_pipes(row["note"])
            add(f"| `{row['name']}` | {row['tier']} | {row['status']} | {note} | "
                f"{row['type']} | {default} | {row['unit']} | {row['locations']} |")
        add("")

    if legacy_extra:
        add("## Legacy-table names absent from the modern table")
        add("")
        add("Kept so that an old profile still loads instead of being rejected. These")
        add("are accepted-and-mapped, not implemented as separate behaviour.")
        add("")
        for name in legacy_extra:
            add(f"- `{name}` ({legacy_types.get(name, '?')})")
        add("")

    orca_dialect = []
    orca_unknown = []
    for name in orca_extra:
        engine = foreign.get(name)
        if engine and engine in ledger_names:
            orca_dialect.append((name, engine))
        else:
            orca_unknown.append(name)

    if orca_extra:
        add("## Orca dialect words")
        add("")
        add("Measured from `~/.config/OrcaSlicer`. Orca renamed a large part of the")
        add("dialect, so these are the *same settings under a different word*, not new")
        add("ones. The mapping is next-layer's own table in `ForeignSettings.cpp`, which")
        add("`oracle/foreign_settings_check.py` already gates. HFW must accept every")
        add("left-hand name and apply it to the right-hand behaviour.")
        add("")
        add("| Orca name | Engine setting | HFW status |")
        add("|---|---|---|")
        for orca_name, engine_name in sorted(orca_dialect):
            row = next((r for r in rows if r["name"] == engine_name), None)
            add(f"| `{orca_name}` | `{engine_name}` | {row['status'] if row else '?'} |")
        add("")
        if orca_unknown:
            add("### Orca names with no engine counterpart")
            add("")
            add("Bambu-fork features and Orca extras. They must be *accepted* so an")
            add("Orca preset loads; whether each is *implemented* is a decision recorded")
            add("per name. None of them can be how parity is proven, because the engine")
            add("next-layer already drives does not have them either.")
            add("")
            for name in orca_unknown:
                add(f"- `{name}`")
            add("")

    add("## HFW names with no dialect counterpart")
    add("")
    add("HFW extensions. They are legitimate, but a PrusaSlicer or Orca preset will")
    add("never contain them, so they cannot be how compatibility is proven.")
    add("")
    for hfw_key, (prusa_key, status, note) in sorted(HFW_ALIAS.items()):
        if prusa_key is None:
            add(f"- `{hfw_key}` — {note}")
    add("")

    surface = surface_document(rows, applicable, applied, partial, missing, alias_index)
    target = HFW / "docs/SETTINGS-LEDGER.md"
    text = "\n".join(out) + "\n"
    surface_text = json.dumps(surface, indent=1, sort_keys=False) + "\n"
    backlog = HFW / "docs/PARITY-BACKLOG.md"
    backlog_text = backlog_document(rows)
    if "--check" in sys.argv[1:]:
        stale = []
        for path, expected in ((target, text), (SURFACE, surface_text), (backlog, backlog_text)):
            current = path.read_text(encoding="utf-8") if path.exists() else ""
            if current != expected:
                stale.append(path)
        if stale:
            for path in stale:
                print(f"{path} is stale; run tools/gen-settings-ledger.py", file=sys.stderr)
            return 1
        print(f"settings ledger and surface: {len(rows)} settings, up to date")
        return 0
    target.write_text(text, encoding="utf-8")
    SURFACE.parent.mkdir(parents=True, exist_ok=True)
    SURFACE.write_text(surface_text, encoding="utf-8")
    backlog.write_text(backlog_text, encoding="utf-8")
    print(f"wrote {target}")
    print(f"wrote {SURFACE}")
    print(f"wrote {backlog}")
    print(f"engine settings: {len(rows)}  applicable: {len(applicable)}  "
          f"applied: {len(applied)}  partial: {len(partial)}  missing: {len(missing)}  "
          f"n/a: {len(rows) - len(applicable)}")
    print(f"legacy extras: {len(legacy_extra)}  orca extras: {len(orca_extra)}")
    print(f"tiers: {dict(tiers)}")
    return 0


def tier_rank(tier: str) -> str:
    return tier


def normalize_locations(raw: str) -> str:
    parts = [part.strip() for part in raw.split(",") if part.strip()]
    return ",".join(part[:1].upper() + part[1:] for part in parts) or "?"


def short_type(ctype: str) -> str:
    return (
        ctype.replace("std::", "")
        .replace("vector<", "vec<")
        .replace("string", "str")
    )


def escape_pipes(value: str) -> str:
    return value.replace("|", "\\|") if value else ""


if __name__ == "__main__":
    raise SystemExit(main())
