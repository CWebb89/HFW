"""FreeType's answer for a font, to check this engine's own font reader against.

Not a slicer comparison: FreeType is what every text renderer this machine has uses,
so it is the ground truth for glyph advances and ink bounds. The engine's reader
(`crates/font`) parses the same tables without FreeType; agreement within a thousandth
of the em is the claim.

    .venv/bin/python scripts/font-oracle.py FONT TEXT SIZE_MM [--tolerance MM]

Prints one JSON object: per character, the glyph's advance and ink box in millimetres,
the whole string's advance, and the pair kerning between each two characters that end up
next to each other — measured as the difference between the string's advance and the sum
of its parts, which is how the renderer's own shaper applies kerning, GPOS or legacy.
"""

import json
import sys

from PIL import Image, ImageDraw, ImageFont


def measure(font_path: str, text: str, size_mm: float, scale: int = 20) -> dict:
    pixels_per_mm = 20 * scale
    font = ImageFont.truetype(font_path, size=round(size_mm * pixels_per_mm))
    ascent, descent = font.getmetrics()
    height = ascent + descent
    per_character = []
    for character in text:
        advance = font.getlength(character) / pixels_per_mm
        width = max(int(advance * pixels_per_mm) + 2 * pixels_per_mm, 8)
        image = Image.new("L", (width, height))
        ImageDraw.Draw(image).text((0, 0), character, font=font, fill=255)
        box = image.getbbox()
        if box is None:
            per_character.append({"character": character, "advance": advance, "ink": None})
            continue
        left, top, right, bottom = box
        ink = {
            "min_x": left / pixels_per_mm,
            "max_x": right / pixels_per_mm,
            "max_y": (ascent - top) / pixels_per_mm,
            "min_y": (ascent - bottom) / pixels_per_mm,
        }
        per_character.append({"character": character, "advance": advance, "ink": ink})
    pairs = [
        {
            "left": left["character"],
            "right": right["character"],
            "kern": font.getlength(left["character"] + right["character"])
            / pixels_per_mm
            - left["advance"]
            - right["advance"],
        }
        for left, right in zip(per_character, per_character[1:])
    ]
    return {
        "font": font_path,
        "text": text,
        "size_mm": size_mm,
        "advance": sum(entry["advance"] for entry in per_character),
        "string_advance": font.getlength(text) / pixels_per_mm,
        "characters": per_character,
        "pairs": pairs,
    }


def main(argv: list[str]) -> int:
    if len(argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    font_path, text, size = argv[1], argv[2], float(argv[3])
    print(json.dumps(measure(font_path, text, size)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
