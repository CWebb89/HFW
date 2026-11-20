//! Glyph outlines from a TrueType or OpenType font file.
//!
//! A font is a container of tables; this module reads the ones an extruded letter
//! needs and nothing else: the character map, the horizontal metrics, the glyph
//! locator and the glyph outlines themselves. Curves are quadratic Béziers, which
//! TrueType stores as an on-curve point between two off-curve points; they are
//! flattened to polylines at a caller-chosen tolerance, so no downstream stage has to
//! know a font exists.
//!
//! What this does not read: CFF/`OTTO` outlines (a different curve algebra and a
//! different table), GPOS/GSUB kerning and shaping, and hinting. A font whose outlines
//! are CFF is refused by name rather than returned as empty glyphs.
//!
//! Outlines are returned in font units with Y up, as the font stores them. A caller
//! that wants millimetres divides by [`Face::units_per_em`] and multiplies by the
//! requested size; [`Face::outline_mm`] does exactly that.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

mod cmap;
mod glyf;
mod kern;
mod sfnt;

pub use glyf::{Contour, Outline};

/// A point in font units, Y up.
pub type Point = [f64; 2];

/// Why a font could not be read, or a glyph could not be resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontError {
    /// The file is shorter than the table it claims to contain.
    Truncated {
        /// What was being read.
        what: &'static str,
        /// The byte offset the reader needed.
        offset: usize,
        /// Bytes available at that offset.
        available: usize,
    },
    /// The first four bytes are not a container this reader knows.
    NotAFont {
        /// The four-byte tag that was found.
        tag: String,
    },
    /// The font is a CFF-flavoured OpenType file, whose outlines are not TrueType.
    CffOutlines,
    /// A table the face needs is absent.
    MissingTable {
        /// The four-byte table tag.
        tag: &'static str,
    },
    /// A table is present but malformed.
    BadTable {
        /// The four-byte table tag.
        tag: &'static str,
        /// What was wrong.
        reason: &'static str,
    },
    /// The glyph identifier is outside the face's glyph range.
    NoSuchGlyph {
        /// The identifier that was asked for.
        glyph: u16,
        /// How many glyphs the face holds.
        glyphs: u16,
    },
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated {
                what,
                offset,
                available,
            } => write!(
                f,
                "the font ends inside {what}: needed byte {offset}, {available} bytes left"
            ),
            Self::NotAFont { tag } => {
                write!(f, "{tag:?} is not a font container this reader knows")
            }
            Self::CffOutlines => write!(
                f,
                "this font stores CFF outlines (OTTO), which need a different outline reader"
            ),
            Self::MissingTable { tag } => write!(f, "the font has no {tag} table"),
            Self::BadTable { tag, reason } => write!(f, "the {tag} table is malformed: {reason}"),
            Self::NoSuchGlyph { glyph, glyphs } => {
                write!(f, "glyph {glyph} is outside this face's {glyphs} glyphs")
            }
        }
    }
}

impl std::error::Error for FontError {}

/// A parsed font file: its tables and the metrics a laid-out glyph needs.
#[derive(Clone, Debug)]
pub struct Face {
    bytes: Vec<u8>,
    tables: BTreeMap<[u8; 4], Range<usize>>,
    units_per_em: u16,
    glyphs: u16,
    long_loca: bool,
    long_metrics: u16,
    advances: Vec<u16>,
    kerning: kern::Kerning,
    cmap: cmap::CharMap,
    face_index: u32,
}

impl Face {
    /// Parses a font file. `face_index` selects a face inside a collection.
    pub fn parse(bytes: &[u8], face_index: u32) -> Result<Self, FontError> {
        let tables = sfnt::directory(bytes, face_index)?;
        let table = |tag: &[u8; 4]| sfnt::table(bytes, &tables, tag);
        let head = sfnt::reader(table(b"head")?, "head")?;
        if head.bytes(0, 54).is_err() {
            return Err(FontError::BadTable {
                tag: "head",
                reason: "shorter than 54 bytes",
            });
        }
        let units_per_em = head.u16_at(18)?;
        let long_loca = head.i16_at(50)? == 1;
        if units_per_em == 0 {
            return Err(FontError::BadTable {
                tag: "head",
                reason: "unitsPerEm is zero",
            });
        }
        let glyphs = sfnt::reader(table(b"maxp")?, "maxp")?.u16_at(4)?;
        let hhea = sfnt::reader(table(b"hhea")?, "hhea")?;
        let long_metrics = hhea.u16_at(34)?;
        let hmtx = sfnt::reader(table(b"hmtx")?, "hmtx")?;
        if long_metrics == 0 {
            return Err(FontError::BadTable {
                tag: "hhea",
                reason: "numberOfHMetrics is zero",
            });
        }
        let mut advances = Vec::with_capacity(usize::from(glyphs));
        for glyph in 0..usize::from(glyphs) {
            let metric = glyph.min(usize::from(long_metrics) - 1);
            advances.push(hmtx.u16_at(metric * 4)?);
        }
        let cmap = cmap::CharMap::parse(table(b"cmap")?)?;
        let kerning = kern::Kerning::parse(table(b"GPOS").ok(), table(b"kern").ok())?;
        Ok(Self {
            cmap,
            kerning,
            bytes: bytes.to_vec(),
            tables,
            units_per_em,
            glyphs,
            long_loca,
            long_metrics,
            advances,
            face_index,
        })
    }

    /// Reads a font file from disk.
    pub fn load(path: &std::path::Path, face_index: u32) -> Result<Self, FontError> {
        let bytes = std::fs::read(path).map_err(|error| {
            let _ = error;
            FontError::MissingTable { tag: "file" }
        })?;
        Self::parse(&bytes, face_index)
    }

    /// Design units per em, the divisor between font units and a requested size.
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// How many glyphs the face holds.
    pub fn glyph_count(&self) -> u16 {
        self.glyphs
    }

    /// Which face in a collection this is.
    pub fn face_index(&self) -> u32 {
        self.face_index
    }

    /// The glyph a character maps to, or `None` when the font has no glyph for it.
    pub fn glyph_index(&self, character: char) -> Option<u16> {
        self.cmap.glyph(character)
    }

    /// The advance width of a glyph in font units.
    pub fn advance(&self, glyph: u16) -> Option<u16> {
        self.advances.get(usize::from(glyph)).copied()
    }

    /// The advance width of a glyph in millimetres at the given size.
    pub fn advance_mm(&self, glyph: u16, size_mm: f64) -> Option<f64> {
        self.advance(glyph)
            .map(|advance| f64::from(advance) / f64::from(self.units_per_em) * size_mm)
    }

    /// The kerning between two glyphs in font units, negative when they tighten.
    ///
    /// Read from the `kern` feature in GPOS when the font has one, and from the legacy
    /// `kern` table otherwise; a pair the font does not mention is zero, which is most
    /// of them.
    pub fn kerning(&self, left: u16, right: u16) -> i16 {
        self.kerning.pair(left, right)
    }

    /// The kerning between two glyphs in millimetres at the given size.
    pub fn kerning_mm(&self, left: u16, right: u16, size_mm: f64) -> f64 {
        f64::from(self.kerning(left, right)) / f64::from(self.units_per_em) * size_mm
    }

    /// A table by tag, for a caller that needs raw bytes.
    pub fn table(&self, tag: [u8; 4]) -> Option<&[u8]> {
        let range = self.tables.get(&tag)?;
        self.bytes.get(range.clone())
    }

    /// The outline of one glyph in font units, flattened at `tolerance` font units.
    pub fn outline(&self, glyph: u16, tolerance: f64) -> Result<Outline, FontError> {
        if glyph >= self.glyphs {
            return Err(FontError::NoSuchGlyph {
                glyph,
                glyphs: self.glyphs,
            });
        }
        glyf::outline(
            &self.bytes,
            &self.tables,
            glyph,
            self.long_loca,
            tolerance.abs().max(f64::MIN_POSITIVE),
            0,
        )
    }

    /// The outline of one glyph in millimetres at `size_mm`, Y up, with the baseline at
    /// Y = 0 and the origin at X = 0.
    pub fn outline_mm(
        &self,
        glyph: u16,
        size_mm: f64,
        tolerance_mm: f64,
    ) -> Result<Outline, FontError> {
        let scale = size_mm / f64::from(self.units_per_em);
        let tolerance = tolerance_mm / scale;
        let mut outline = self.outline(glyph, tolerance)?;
        outline.scale(scale);
        Ok(outline)
    }

    /// The outline of a string, laid out left to right with advances and pair kerning
    /// applied. Returns the contours and the total advance in millimetres.
    ///
    /// Kerning is applied between glyphs that end up next to each other in the run, so a
    /// character the font has no glyph for does not break the pair around it.
    pub fn layout_mm(
        &self,
        text: &str,
        size_mm: f64,
        tolerance_mm: f64,
    ) -> Result<(Outline, f64), FontError> {
        let mut placed = Outline::default();
        let mut pen = 0.0_f64;
        let mut previous: Option<u16> = None;
        for character in text.chars() {
            let Some(glyph) = self.glyph_index(character) else {
                continue;
            };
            if let Some(previous) = previous {
                pen += self.kerning_mm(previous, glyph, size_mm);
            }
            let mut outline = self.outline_mm(glyph, size_mm, tolerance_mm)?;
            outline.shift(pen, 0.0);
            placed.append(outline);
            pen += self.advance_mm(glyph, size_mm).unwrap_or(0.0);
            previous = Some(glyph);
        }
        Ok((placed, pen))
    }

    /// Number of long horizontal metrics, the point after which advances repeat.
    pub fn long_metrics(&self) -> u16 {
        self.long_metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_font() -> Option<Face> {
        let candidates = [
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/FiraCode-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        candidates
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .map(|bytes| Face::parse(&bytes, 0).expect("the system font parses"))
    }

    #[test]
    fn a_font_that_is_not_a_font_is_refused_by_name() {
        let error = Face::parse(b"hello world, not a font at all", 0).unwrap_err();
        assert!(matches!(error, FontError::NotAFont { .. }), "{error}");
        assert!(error.to_string().contains("hell"), "{error}");
    }

    #[test]
    fn a_cff_flavoured_font_says_so_instead_of_returning_empty_glyphs() {
        let mut bytes = std::fs::read("/usr/share/fonts/gnu-free/FreeSans.otf").unwrap_or_default();
        if bytes.is_empty() {
            return;
        }
        bytes.truncate(12);
        let error = Face::parse(&bytes, 0).unwrap_err();
        assert_eq!(error, FontError::CffOutlines);
    }

    #[test]
    fn a_system_font_maps_letters_to_glyphs_with_advances() {
        let Some(face) = system_font() else {
            return;
        };
        assert!(face.units_per_em() >= 16, "{}", face.units_per_em());
        let a = face.glyph_index('A').expect("'A' maps to a glyph");
        let b = face.glyph_index('B').expect("'B' maps to a glyph");
        assert_ne!(a, b);
        assert!(face.advance(a).unwrap() > 0);
        assert!(
            face.glyph_index('\u{10FFFF}').is_none() || face.glyph_index('\u{10FFFF}') != Some(0)
        );
    }

    #[test]
    fn an_outline_is_closed_contours_with_the_letter_inside_its_em_square() {
        let Some(face) = system_font() else {
            return;
        };
        let a = face.glyph_index('A').unwrap();
        let outline = face.outline_mm(a, 10.0, 0.01).expect("'A' has an outline");
        assert!(!outline.contours.is_empty());
        let units = f64::from(face.units_per_em());
        let cap_height_mm = 7.0;
        for contour in &outline.contours {
            assert!(
                contour.points.len() >= 3,
                "a contour with {}",
                contour.points.len()
            );
            for point in &contour.points {
                assert!(point[0] > -0.2 * 10.0 && point[0] < 10.0, "{point:?}");
                assert!(
                    point[1] > -0.4 * 10.0 && point[1] < cap_height_mm + 1.0,
                    "{point:?}"
                );
                assert!(point[0].is_finite() && point[1].is_finite());
            }
        }
        let _ = units;
    }

    #[test]
    fn a_lower_tolerance_never_removes_points() {
        let Some(face) = system_font() else {
            return;
        };
        let o = face.glyph_index('o').unwrap();
        let coarse = face.outline(o, 2.0).unwrap().point_count();
        let fine = face.outline(o, 0.05).unwrap().point_count();
        assert!(fine >= coarse, "{fine} < {coarse}");
    }

    #[test]
    fn a_space_has_no_contours_but_an_advance() {
        let Some(face) = system_font() else {
            return;
        };
        let space = face.glyph_index(' ').expect("the space maps to a glyph");
        assert!(face.outline(space, 0.5).unwrap().contours.is_empty());
        assert!(face.advance_mm(space, 10.0).unwrap() > 0.5);
    }

    #[test]
    fn a_glyph_outside_the_face_is_refused_with_both_numbers() {
        let Some(face) = system_font() else {
            return;
        };
        let error = face.outline(face.glyph_count(), 1.0).unwrap_err();
        match error {
            FontError::NoSuchGlyph { glyph, glyphs } => {
                assert_eq!(glyph, glyphs);
            }
            other => panic!("{other}"),
        }
    }

    #[test]
    fn a_string_is_laid_out_left_to_right_in_one_outline() {
        let Some(face) = system_font() else {
            return;
        };
        let (outline, width) = face.layout_mm("HFW", 8.0, 0.01).unwrap();
        let single = face
            .outline_mm(face.glyph_index('H').unwrap(), 8.0, 0.01)
            .unwrap();
        assert!(outline.contours.len() > single.contours.len());
        assert!(width > 8.0, "three letters at 8 mm are {width} mm wide");
        let bounds = outline.bounds();
        assert!(bounds.2 < width + 1.0, "{bounds:?} against {width}");
    }
}
