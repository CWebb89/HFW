#!/usr/bin/env python3
"""Decode a binary G-code file (bgcode) without any of the engine's code.

A second implementation of the container, written from
`libbgcode/doc/specifications.md` alone, so a file the engine calls valid is
read by something that was not written beside it. It walks the header and the
blocks, checks every block's CRC32 against its stored data, and writes the
G-code text back out.

Nothing here decompresses: a block that names a compression is refused, as is a
G-code block that names an encoding. That is the same limit the engine's writer
has, so a file this script accepts is one the writer could have produced.

    scripts/bgcode-decode.py out.bgcode --gcode out.gcode
"""

from __future__ import annotations

import argparse
import struct
import sys
import zlib
from pathlib import Path

MAGIC = b"GCDE"
VERSION = 1
HEADER_BYTES = 10

CHECKSUMS = {0: "none", 1: "CRC32"}

TYPES = {
    0: "file metadata",
    1: "gcode",
    2: "slicer metadata",
    3: "printer metadata",
    4: "print metadata",
    5: "thumbnail",
}

IMAGE_FORMATS = {0: "PNG", 1: "JPG", 2: "QOI"}


class Bad(Exception):
    """The file does not follow the specification."""


def parameter_bytes(kind: int) -> int:
    """Block parameters a type carries: three u16 for an image, one for the rest."""
    return 6 if kind == 5 else 2


def walk(data: bytes) -> dict:
    """Reads the container into its blocks, refusing anything malformed."""
    if len(data) < HEADER_BYTES:
        raise Bad(f"the file is {len(data)} bytes, shorter than the {HEADER_BYTES}-byte header")
    if data[:4] != MAGIC:
        raise Bad("not a binary G-code file: the magic is missing")
    version, checksum_type = struct.unpack_from("<IH", data, 4)
    if version != VERSION:
        raise Bad(f"version {version} is not the version this decoder implements ({VERSION})")
    if checksum_type not in CHECKSUMS:
        raise Bad(f"checksum type {checksum_type} is neither none (0) nor CRC32 (1)")

    at = HEADER_BYTES
    blocks = []
    text = []
    while at < len(data):
        if at + 8 > len(data):
            raise Bad(f"a block header starts at byte {at} and the file ends at {len(data)}")
        kind, compression, size = struct.unpack_from("<HHI", data, at)
        at += 8
        if kind not in TYPES:
            raise Bad(f"block type {kind} is not defined")
        if compression != 0:
            raise Bad(f"{TYPES[kind]} block is compressed with algorithm {compression}")
        stored = size
        parameters = parameter_bytes(kind)
        checksum_bytes = 4 if checksum_type else 0
        if at + parameters + stored + checksum_bytes > len(data):
            raise Bad(f"{TYPES[kind]} block claims {stored} bytes and the file does not hold them")
        params = data[at : at + parameters]
        at += parameters
        payload = data[at : at + stored]
        at += stored
        recorded = None
        if checksum_bytes:
            recorded = struct.unpack_from("<I", data, at)[0]
            at += checksum_bytes
            computed = zlib.crc32(payload) & 0xFFFFFFFF
            if recorded != computed:
                raise Bad(
                    f"{TYPES[kind]} block fails its checksum: "
                    f"the file stores {recorded:08X}, its data produces {computed:08X}"
                )
        block = {
            "type": TYPES[kind],
            "bytes": stored,
            "checksum": recorded,
        }
        if kind == 1:
            encoding = struct.unpack_from("<H", params, 0)[0]
            if encoding != 0:
                raise Bad(f"gcode block is encoded with algorithm {encoding}")
            text.append(payload.decode("utf-8"))
        elif kind == 5:
            image_format, width, height = struct.unpack_from("<HHH", params, 0)
            if image_format not in IMAGE_FORMATS:
                raise Bad(f"thumbnail format {image_format} is not one the specification defines")
            block["image"] = f"{width}x{height} {IMAGE_FORMATS[image_format]}"
        else:
            encoding = struct.unpack_from("<H", params, 0)[0]
            if encoding != 0:
                raise Bad(f"{TYPES[kind]} block is not INI-encoded (encoding {encoding})")
            block["rows"] = len([line for line in payload.decode("utf-8").splitlines() if line.strip()])
        blocks.append(block)

    return {
        "version": version,
        "checksum": CHECKSUMS[checksum_type],
        "blocks": blocks,
        "text": "".join(text),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Decode a binary G-code file.")
    parser.add_argument("file", type=Path, help="the .bgcode file")
    parser.add_argument("--gcode", type=Path, help="write the G-code text here")
    parser.add_argument(
        "--print-text",
        action="store_true",
        help="write the G-code text to stdout instead of the block summary",
    )
    arguments = parser.parse_args()

    try:
        data = arguments.file.read_bytes()
        decoded = walk(data)
    except (Bad, UnicodeDecodeError) as error:
        print(f"{arguments.file}: {error}", file=sys.stderr)
        return 2

    if arguments.print_text:
        sys.stdout.write(decoded["text"])
        return 0

    if arguments.gcode is not None:
        arguments.gcode.write_text(decoded["text"], encoding="utf-8")

    print(
        f"{arguments.file}: version {decoded['version']}, "
        f"checksum {decoded['checksum']}, {len(decoded['blocks'])} blocks"
    )
    for index, block in enumerate(decoded["blocks"]):
        detail = block.get("image") or (
            f"{block['rows']} rows" if "rows" in block else f"{block['bytes']} bytes of text"
        )
        print(f"  block {index}: {block['type']}, {detail}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
