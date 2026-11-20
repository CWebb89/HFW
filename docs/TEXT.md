# Text as geometry

A string printed here is a prism: the font's glyph outlines, capped top and bottom,
walled along every contour, placed as its own object and sliced like any other mesh.
That is the whole design. There is no text engine and no triangle-level boolean — a
letter is geometry the pipeline already understands, and the slice does the union.

## What exists

`crates/font` reads a TrueType or OpenType file directly: the table directory (and
`.ttc` collections), `cmap` formats 4 and 12, `head`, `hhea`, `maxp`, `hmtx`, `loca`,
and `glyf` outlines, simple and composite, with quadratic Béziers flattened at a
caller-chosen tolerance. Y is up, the origin is the baseline's left edge, and
`Face::layout_mm` returns the contours of a whole string plus its advance width.

`crates/text` turns that outline into a solid: contours are grouped into boundaries and
holes by the even-odd rule (not by the font's stored direction, which is the font's
choice), each hole is bridged into its boundary along the shortest segment that crosses
nothing, and the caps are ear-clipped while every contour gets its own wall. The result
is a closed mesh with no inconsistently oriented edges — a test asserts exactly that.

`hfw-cli --text` slices it:

```fish
hfw-cli --text HFW --text-font /usr/share/fonts/TTF/FiraCode-Regular.ttf \
    --text-size 12 --text-height 0.8 --layer-height 0.2 \
    --profile profiles/voron_klipper.cfg --gcode text.gcode
```

The gate runs that job and requires four layers, the letters' own walls, a verifier
`RESULT PASS`, and refusals that name what is missing. It also runs the FreeType
comparison when `HFW_PY` is set.

## How it is measured

- `cargo test -p hfw-font` — 15 tests on cmap, glyf flags, composite glyphs, flattening
  tolerance and the implied-midpoint rule.
- `cargo test -p hfw-font --test freetype` — a real font, every glyph's advance and ink
  box against FreeType through `scripts/font-oracle.py`. Agreement: **0.00002 mm** on
  advances, **0.00218 mm** on ink at 10 mm, which is the oracle's own raster
  resolution.
- `cargo test -p hfw-text` — extrusion volume against polygon area × height (200 mm³ for
  a 10×10×2 box, 300 mm³ for a 20×20 square with a 10×10 hole), and a letter with a
  counter keeping its hole and closing the surface.

## What is still missing

This row of `docs/PARITY-BACKLOG.md` is not finished. What is left:

1. **Kerning and shaping.** Advances are plain `hmtx` widths, so `AV` is spaced like
   `A` `V`. GPOS/GSUB kerning and complex-script shaping are not read.
2. **Placing text on an object's surface.** The prism is built at the origin with its
   baseline on `y = 0`; the application rotates and places it, and the arranger moves it
   with the object. There is no call that snaps a string onto a sloped or cylindrical
   face, and none that finds a face's centroid for you.
3. **Fonts without `glyf`.** CFF-flavoured OpenType is refused by name; the outlines need
   a different curve algebra to read.

## What the application gets

`hfw_text_mesh(font_path, face_index, text, size_mm, height_mm, z_mm)` returns a mesh
handle, so the panel builds a string without parsing a font itself and without linking
this crate's reader twice. Feed it to `hfw_plate_object_add_volume` as
`HFW_VOLUME_PART` to print it raised or `HFW_VOLUME_NEGATIVE` to engrave it; the ABI test
sinks three letters into a cube's face and requires the slice to deposit **3.2 mm less
filament** than the same plate without them.
