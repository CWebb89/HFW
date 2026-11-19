#!/usr/bin/env python3
"""Three-way slicing oracle: hfw against OrcaSlicer and PrusaSlicer.

The point of this script is that none of the three slicers is trusted. Each one
slices the same STL at matched settings, and the deposited volume is measured
from the emitted G-code by one shared parser, so a disagreement is a real
disagreement rather than a difference in bookkeeping.

Usage:
    scripts/oracle.py fixtures/cube.stl
    scripts/oracle.py fixtures/sphere.stl --density 1.0
    scripts/oracle.py fixtures/*.stl --markdown

The script exits non-zero when any feature class is further than the tolerance
from the mean of the two reference slicers.
"""

import argparse
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

FILAMENT_AREA = 3.141592653589793 * (1.75 / 2) ** 2

ORCA_MACHINE_REL = "Voron/machine/Voron 2.4 350 0.4 nozzle.json"
ORCA_PROCESS_REL = "Voron/process/0.20mm Standard @Voron.json"
ORCA_ROOTS = (
    os.environ.get("HFW_ORCA_PROFILES", ""),
    os.path.expanduser("~/.config/OrcaSlicer/system"),
    "/opt/orca-slicer/resources/profiles",
    os.path.expanduser("~/dev/reference/OrcaSlicer/resources/profiles"),
)
ORCA_ROOT = next(
    (root for root in ORCA_ROOTS
     if root and os.path.isfile(os.path.join(root, ORCA_MACHINE_REL))),
    None,
)
ORCA_MACHINE = os.path.join(ORCA_ROOT, ORCA_MACHINE_REL) if ORCA_ROOT else ""
ORCA_PROCESS = os.path.join(ORCA_ROOT, ORCA_PROCESS_REL) if ORCA_ROOT else ""

WALL = {"outer wall", "inner wall", "perimeter", "external perimeter",
        "internal perimeter", "overhang perimeter"}
SKIN = {"solid infill", "internal solid infill", "top solid infill",
        "top surface", "bottom surface", "bridge infill", "internal bridge"}
SPARSE = {"sparse infill", "internal infill"}
THIN = {"gap infill", "thin wall"}

# Fixtures where a reference is known to be the outlier, with the reason and the
# tolerance that still applies. A sub-width wall is the case this engine exists
# for: both references substitute a full-width bead for a 0.3 mm wall and
# therefore deposit about 12% more plastic than the model contains.
EXPECTED = {
    "thin_wall.stl": ("references substitute a full-width bead for a 0.3 mm wall", 0.25),
    "taper_wall.stl": (
        "the taper passes through widths below one bead, printed at their measured width",
        0.25,
    ),
}


def bucket(label):
    name = label.strip().lower()
    if name in WALL:
        return "wall"
    if name in SKIN:
        return "skin"
    if name in SPARSE:
        return "sparse"
    if name in THIN:
        return "thin"
    if name == "skirt" or name == "skirt/brim":
        return None
    return "other"


def read_profile(path):
    values, block = {}, None
    body, tag = [], None
    for raw in open(path):
        line = raw.strip()
        if tag is not None:
            if line == tag:
                values[block] = "\n".join(body)
                tag, body = None, []
            else:
                body.append(raw.rstrip("\n"))
            continue
        if not line or line.startswith("#"):
            continue
        key, _, value = line.partition("=")
        key, value = key.strip(), value.strip()
        if value.startswith("<<"):
            block, tag = key, value[2:].strip()
            continue
        values[key] = value
    return values


def parse_gcode(path):
    per = {}
    current = "none"
    extruder = 0.0
    relative_e = False
    for line in open(path, errors="replace"):
        if line.startswith(";"):
            if line.startswith(";TYPE:"):
                current = line[6:].strip()
            continue
        body = line.split(";")[0].strip()
        if not body:
            continue
        tokens = body.split()
        code = tokens[0].upper()
        if code == "M82":
            relative_e = False
            continue
        if code == "M83":
            relative_e = True
            continue
        if code == "G92":
            for token in tokens[1:]:
                if token[:1].upper() == "E":
                    try:
                        extruder = float(token[1:])
                    except ValueError:
                        pass
            continue
        if code not in ("G0", "G1"):
            continue
        delta, moved = None, False
        for token in tokens[1:]:
            key = token[:1].upper()
            try:
                value = float(token[1:])
            except ValueError:
                continue
            if key == "E":
                delta = value if relative_e else value - extruder
                if not relative_e:
                    extruder = value
            elif key in ("X", "Y"):
                moved = True
        if delta is None or delta <= 1e-9 or not moved:
            continue
        kind = bucket(current)
        if kind is None:
            continue
        entry = per.setdefault(kind, [0.0, 0.0])
        entry[0] += delta * FILAMENT_AREA
        entry[1] += max(delta, 0.0)
    return {k: v[0] for k, v in per.items()}


def parse_seconds(text):
    """Seconds from "1h 2m 3s", "13m 54s" or "907"."""
    text = text.strip().rstrip(".")
    if re.fullmatch(r"[\d.]+", text):
        return float(text)
    total = 0.0
    for value, unit in re.findall(r"([\d.]+)\s*([hms])", text):
        total += float(value) * {"h": 3600.0, "m": 60.0, "s": 1.0}[unit]
    return total


def parse_time(path):
    """Each slicer own estimate, read from the file it wrote.

    Reading the header rather than re-deriving the time is deliberate: the
    question is what each slicer *says* the job costs, and then whether the three
    of them agree about the same G-code.
    """
    patterns = [
        re.compile(r";\s*estimated printing time \(normal mode\)\s*=\s*(.+)$"),
        re.compile(r";\s*estimated printing time\s*=\s*([\d.]+\s*s)"),
        re.compile(r";\s*PRINTING_TIME\s*[:=]\s*(.+)$"),
        re.compile(r";\s*TIME\s*:\s*([\d.]+)$"),
    ]
    for line in open(path, errors="replace"):
        if not line.startswith(";"):
            continue
        for pattern in patterns:
            found = pattern.search(line)
            if found:
                seconds = parse_seconds(found.group(1))
                if seconds > 0:
                    return seconds
    return None


def time_line(times):
    """One row comparing the three planners own time estimates, in minutes."""
    if not times.get("hfw"):
        return None
    references = [value for name, value in times.items() if name != "hfw" and value]
    mean = sum(references) / len(references) if references else 0.0
    delta = (times["hfw"] - mean) / mean * 100.0 if mean else float("nan")
    cells = " ".join(
        "{:9.1f}".format(times[name] / 60.0) if times.get(name) else "{:>9s}".format("-")
        for name in ("hfw", "orca", "prusa"))
    return "  {:8s} {} {:+8.1f}%".format("time min", cells, delta)


def run(cmd, log):
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        log.write("COMMAND FAILED: " + " ".join(cmd) + "\n")
        log.write(result.stdout[-4000:] + "\n" + result.stderr[-4000:] + "\n")
    return result.returncode == 0


def neutralised_profile(source, work):
    """A copy of the profile with the extrusion multiplier removed.

    `flow` is a calibration offset the user tunes against a measured wall. The
    two reference slicers run at 1.0, so leaving it at 0.95 would put a flat
    5% into every comparison and hide real differences.
    """
    lines = []
    for line in open(source):
        stripped = line.strip()
        if stripped.startswith("flow") and "=" in stripped:
            lines.append("flow = 1.0\n")
        else:
            lines.append(line)
    path = os.path.join(work, "oracle_profile.cfg")
    open(path, "w").writelines(lines)
    return path


def slice_hfw(args, model, work, log):
    out = os.path.join(work, "hfw")
    os.makedirs(out, exist_ok=True)
    gcode = os.path.join(work, "hfw.gcode")
    profile = args.profile if args.keep_flow else neutralised_profile(args.profile, work)
    cmd = [args.hfw, model, "--profile", profile,
           "--walls", str(args.walls),
           "--layer-height", str(args.layer_height),
           "--density", str(args.density),
           "--skin-top", str(args.top), "--skin-bottom", str(args.bottom),
           "--pattern", args.pattern,
           "--out-dir", out, "--gcode", gcode]
    if not run(cmd, log):
        return None
    return gcode


def slice_orca(args, model, work, log):
    out = os.path.join(work, "orca")
    os.makedirs(out, exist_ok=True)
    if not ORCA_ROOT:
        print("missing OrcaSlicer Voron presets; looked in: "
              + ", ".join(r for r in ORCA_ROOTS if r), file=sys.stderr)
        return None
    machine = json.load(open(ORCA_MACHINE))
    machine["layer_gcode"] = "G92 E0"
    process = json.load(open(ORCA_PROCESS))
    process.update({
        "name": "hfw-oracle",
        "from": "User",
        "compatible_printers": [machine["name"]],
        "compatible_printers_condition": "",
        "layer_height": str(args.layer_height),
        "initial_layer_print_height": str(args.layer_height),
        "wall_loops": str(args.walls),
        "top_shell_layers": str(args.top),
        "bottom_shell_layers": str(args.bottom),
        "line_width": str(args.width),
        "outer_wall_line_width": str(args.width),
        "inner_wall_line_width": str(args.width),
        "initial_layer_line_width": str(args.width),
        "sparse_infill_line_width": str(args.width),
        "internal_solid_infill_line_width": str(args.width),
        "top_surface_line_width": str(args.width),
        "sparse_infill_density": f"{args.density * 100:g}%",
        "sparse_infill_pattern": args.pattern,
        "enable_support": "0",
        "brim_type": "no_brim",
        "skirt_loops": "0",
        "enable_prime_tower": "0",
        "ironing_type": "no ironing",
        "gcode_comments": "1",
        "exclude_object": "0",
        "seam_position": "nearest",
        "use_relative_e_distances": "0",
    })
    process.pop("setting_id", None)
    machine_path = os.path.join(work, "orca_machine.json")
    process_path = os.path.join(work, "orca_process.json")
    json.dump(machine, open(machine_path, "w"), indent=1)
    json.dump(process, open(process_path, "w"), indent=1)
    cmd = [args.orca, "--slice", "0", "--outputdir", out,
           "--load-settings", f"{machine_path};{process_path}", model]
    if not run(cmd, log):
        return None
    produced = glob.glob(os.path.join(out, "*.gcode"))
    return produced[0] if produced else None


def slice_prusa(args, model, work, log):
    gcode = os.path.join(work, "prusa.gcode")
    cmd = [args.prusa, "--export-gcode", "--output", gcode,
           "--layer-height", str(args.layer_height),
           "--first-layer-height", str(args.layer_height),
           "--fill-density", f"{args.density * 100:g}%",
           "--fill-pattern", args.pattern,
           "--perimeters", str(args.walls),
           "--top-solid-layers", str(args.top),
           "--bottom-solid-layers", str(args.bottom),
           "--extrusion-width", str(args.width),
           "--first-layer-extrusion-width", str(args.width),
           "--skirts", "0", "--brim-width", "0",
           model]
    if not run(cmd, log):
        return None
    return gcode


def compare(results, tolerance, model):
    """Per-feature volumes are reported as diagnostics; the total is the gate.

    Feature classes are not comparable across slicers by design. OrcaSlicer and
    PrusaSlicer extend solid infill into the perimeter band and add a line at
    each end of every infill span, while this engine's wall beads already cover
    that band. At 15% density those policies move 15-25% of the infill class
    without changing the part, so gating on a class would flag a policy
    difference as a bug. The deposited total is comparable, and that is what the
    tolerance applies to.
    """
    order = ["wall", "skin", "sparse", "thin"]
    rows = []
    for kind in order:
        values = {name: r.get(kind, 0.0) for name, r in results.items()}
        if all(v == 0.0 for v in values.values()):
            continue
        refs = [v for name, v in values.items() if name != "hfw"]
        mean = sum(refs) / len(refs) if refs else 0.0
        hfw = values.get("hfw", 0.0)
        delta = (hfw - mean) / mean * 100.0 if mean > 0 else float("nan")
        rows.append((kind, hfw, values.get("orca", 0.0), values.get("prusa", 0.0), delta))
    total = {name: sum(r.values()) for name, r in results.items()}
    refs = [v for name, v in total.items() if name != "hfw"]
    mean_total = sum(refs) / len(refs) if refs else 0.0
    delta = (total.get("hfw", 0.0) - mean_total) / mean_total if mean_total else float("nan")
    references_agree = 0.0
    if len(refs) == 2 and max(refs) > 0:
        references_agree = abs(refs[0] - refs[1]) / max(refs)
    status = "PASS" if abs(delta) <= tolerance else "FAIL"
    return rows, total, delta * 100.0, abs(delta), status, references_agree


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("models", nargs="+")
    parser.add_argument("--hfw", default="target/release/hfw-cli")
    parser.add_argument("--orca", default=shutil.which("orca-slicer") or "orca-slicer")
    parser.add_argument("--prusa", default=shutil.which("prusa-slicer") or "prusa-slicer")
    parser.add_argument("--profile", default="profiles/voron_klipper.cfg")
    parser.add_argument("--layer-height", type=float, default=0.2)
    parser.add_argument("--density", type=float, default=0.15)
    parser.add_argument("--walls", type=int, default=3)
    parser.add_argument("--width", type=float, default=0.4)
    parser.add_argument("--top", type=int, default=5)
    parser.add_argument("--bottom", type=int, default=4)
    parser.add_argument("--pattern", default="rectilinear")
    parser.add_argument("--tolerance", type=float, default=0.10,
                        help="fractional tolerance against the reference mean")
    parser.add_argument("--markdown", action="store_true")
    parser.add_argument("--keep", action="store_true",
                        help="keep the work directory for inspection")
    parser.add_argument("--keep-flow", action="store_true",
                        help="use the profile's flow multiplier instead of 1.0")
    args = parser.parse_args()

    for tool in (args.hfw, args.orca, args.prusa):
        if not shutil.which(tool) and not os.path.exists(tool):
            print(f"missing tool: {tool}", file=sys.stderr)
            return 2

    failures = 0
    for model in args.models:
        if not os.path.exists(model):
            print(f"missing model: {model}", file=sys.stderr)
            return 2
        work = tempfile.mkdtemp(prefix="hfw-oracle-")
        log = open(os.path.join(work, "oracle.log"), "w")
        print(f"\n=== {model}")
        produced = {}
        hfw = slice_hfw(args, model, work, log)
        if hfw:
            produced["hfw"] = hfw
        orca = slice_orca(args, model, work, log)
        if orca:
            produced["orca"] = orca
        prusa = slice_prusa(args, model, work, log)
        if prusa:
            produced["prusa"] = prusa
        if not hfw:
            print("  hfw produced no G-code; see", os.path.join(work, "oracle.log"))
            failures += 1
        results = {name: parse_gcode(path) for name, path in produced.items()}
        times = {name: parse_time(path) for name, path in produced.items()}
        reason, tolerance = EXPECTED.get(
            os.path.basename(model), (None, args.tolerance))
        rows, total, total_delta, worst, status, agreement = compare(
            results, tolerance, model)

        if args.markdown:
            print("| feature | hfw mm3 | Orca mm3 | Prusa mm3 | hfw vs mean |")
            print("|---|---:|---:|---:|---:|")
            for kind, h, o, p, delta in rows:
                print(f"| {kind} | {h:.1f} | {o:.1f} | {p:.1f} | {delta:+.1f}% |")
            print(f"| **total** | {total.get('hfw', 0):.1f} | {total.get('orca', 0):.1f} | "
                  f"{total.get('prusa', 0):.1f} | {total_delta:+.1f}% |")
            time_row = time_line(times)
            if time_row:
                print(time_row.replace("  ", "| ", 1))
            print(f"\n`{status}` total deviation {total_delta:+.1f}% "
                  f"(tolerance {tolerance * 100:.0f}%), reference spread {agreement * 100:.1f}%"
                  + (f" — expected divergence: {reason}" if reason else "") + "\n")
        else:
            print(f"  {'feature':8s} {'hfw':>10s} {'Orca':>10s} {'Prusa':>10s} {'vs mean':>9s}")
            for kind, h, o, p, delta in rows:
                print(f"  {kind:8s} {h:10.1f} {o:10.1f} {p:10.1f} {delta:+8.1f}%")
            print(f"  {'total':8s} {total.get('hfw', 0):10.1f} {total.get('orca', 0):10.1f} "
                  f"{total.get('prusa', 0):10.1f} {total_delta:+8.1f}%")
            time_row = time_line(times)
            if time_row:
                print(time_row)
            print(f"  {status}: total deviation {total_delta:+.1f}% "
                  f"(tolerance {tolerance * 100:.0f}%), reference spread {agreement * 100:.1f}%")
            if reason:
                print(f"  expected divergence: {reason}")
        log.close()
        if status != "PASS":
            failures += 1
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)
        elif not args.markdown:
            print(f"  kept {work}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
