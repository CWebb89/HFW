//! Pair kerning: the legacy `kern` table, and the `kern` feature in GPOS.
//!
//! A font may carry both. OpenType says the GPOS feature wins when present, which is
//! what this module does — the legacy table is read only when no GPOS pair adjustment
//! was found, and a font whose two disagree is laid out by GPOS.
//!
//! Lookups are applied once each, in lookup-list order, and the contributions are summed
//! — a shaper runs the `kern` feature's lookups in that order, and a font that lists the
//! same lookup under two feature records must not kern twice. Within one lookup the
//! first subtable that covers the pair decides.

use super::sfnt::reader;
use super::FontError;
use std::collections::BTreeMap;

/// The kerning of one glyph pair, in font units, negative when the pair tightens.
pub(crate) type Kern = i16;

/// Every kerning lookup a font carries, GPOS first, each holding its own subtables.
#[derive(Clone, Debug, Default)]
pub(crate) struct Kerning {
    lookups: Vec<Vec<Subtable>>,
}

/// One table of pair values.
#[derive(Clone, Debug)]
enum Subtable {
    /// A `kern` format 0 table: an explicit pair list.
    Pairs(BTreeMap<u32, Kern>),
    /// A GPOS `PairPos` subtable, in either of its two layouts.
    PairPos(PairPos),
}

/// A GPOS pair adjustment: coverage, two class definitions, and either explicit pair
/// sets or a class matrix.
#[derive(Clone, Debug)]
struct PairPos {
    coverage: Coverage,
    classes: Option<(ClassDef, ClassDef)>,
    sets: Vec<Vec<(u16, Kern)>>,
    matrix: Vec<Vec<Option<Kern>>>,
}

impl PairPos {
    fn value(&self, left: u16, right: u16) -> Option<Kern> {
        let index = self.coverage.index(left)?;
        if let Some((first, second)) = &self.classes {
            let row = self.matrix.get(usize::from(first.class(left)))?;
            return row.get(usize::from(second.class(right))).copied().flatten();
        }
        let set = self.sets.get(index)?;
        set.iter()
            .find(|(glyph, _)| *glyph == right)
            .map(|(_, value)| *value)
    }
}

/// Which glyphs a subtable covers, and at what index.
#[derive(Clone, Debug)]
enum Coverage {
    /// Format 1: a sorted glyph list.
    List(Vec<u16>),
    /// Format 2: glyph ranges, each starting at a coverage index.
    Ranges(Vec<(u16, u16, u16)>),
}

impl Coverage {
    fn parse(table: &[u8], offset: usize) -> Result<Self, FontError> {
        let reader = reader(table, "GPOS coverage")?;
        let base = offset;
        match reader.u16_at(base)? {
            1 => {
                let count = reader.u16_at(base + 2)?;
                let mut glyphs = Vec::with_capacity(usize::from(count));
                for index in 0..usize::from(count) {
                    glyphs.push(reader.u16_at(base + 4 + index * 2)?);
                }
                Ok(Self::List(glyphs))
            }
            2 => {
                let count = reader.u16_at(base + 2)?;
                let mut ranges = Vec::with_capacity(usize::from(count));
                for index in 0..usize::from(count) {
                    let record = base + 4 + index * 6;
                    ranges.push((
                        reader.u16_at(record)?,
                        reader.u16_at(record + 2)?,
                        reader.u16_at(record + 4)?,
                    ));
                }
                Ok(Self::Ranges(ranges))
            }
            _ => Err(FontError::BadTable {
                tag: "GPOS",
                reason: "unknown coverage format",
            }),
        }
    }

    fn index(&self, glyph: u16) -> Option<usize> {
        match self {
            Self::List(glyphs) => glyphs.iter().position(|entry| *entry == glyph),
            Self::Ranges(ranges) => ranges.iter().find_map(|(start, end, first)| {
                (*start..=*end)
                    .contains(&glyph)
                    .then(|| usize::from(*first) + usize::from(glyph - start))
            }),
        }
    }
}

/// A glyph-to-class map.
#[derive(Clone, Debug, Default)]
enum ClassDef {
    /// Format 1: contiguous glyphs from `start`, each with its own class.
    #[default]
    Empty,
    Dense(Vec<u16>),
    Ranges(Vec<(u16, u16, u16)>),
}

impl ClassDef {
    fn parse(table: &[u8], offset: usize) -> Result<Self, FontError> {
        let reader = reader(table, "GPOS class definition")?;
        match reader.u16_at(offset)? {
            1 => {
                let start = reader.u16_at(offset + 2)?;
                let count = reader.u16_at(offset + 4)?;
                let mut classes = Vec::with_capacity(usize::from(count));
                for index in 0..usize::from(count) {
                    classes.push(reader.u16_at(offset + 6 + index * 2)?);
                }
                let mut dense = vec![0_u16; usize::from(start) + classes.len()];
                dense[usize::from(start)..].copy_from_slice(&classes);
                Ok(Self::Dense(dense))
            }
            2 => {
                let count = reader.u16_at(offset + 2)?;
                let mut ranges = Vec::with_capacity(usize::from(count));
                for index in 0..usize::from(count) {
                    let record = offset + 4 + index * 6;
                    ranges.push((
                        reader.u16_at(record)?,
                        reader.u16_at(record + 2)?,
                        reader.u16_at(record + 4)?,
                    ));
                }
                Ok(Self::Ranges(ranges))
            }
            _ => Err(FontError::BadTable {
                tag: "GPOS",
                reason: "unknown class definition format",
            }),
        }
    }

    fn class(&self, glyph: u16) -> u16 {
        match self {
            Self::Empty => 0,
            Self::Dense(classes) => classes.get(usize::from(glyph)).copied().unwrap_or(0),
            Self::Ranges(ranges) => ranges
                .iter()
                .find(|(start, end, _)| (*start..=*end).contains(&glyph))
                .map(|(_, _, class)| *class)
                .unwrap_or(0),
        }
    }
}

/// The byte offset of the X advance inside a value record with `format` fields, and how
/// wide the record is. `None` when the record carries no X advance, which is the common
/// case for the second glyph of a pair.
fn value_layout(format: u16) -> (usize, Option<usize>) {
    let sizes = [2, 2, 2, 2, 2, 2, 2, 2];
    let mut offset = 0;
    let mut advance = None;
    for (bit, size) in sizes.iter().enumerate() {
        if format & (1 << bit) != 0 {
            if bit == 2 {
                advance = Some(offset);
            }
            offset += size;
        }
    }
    (offset, advance)
}

impl Kerning {
    /// Reads the pair kerning a font carries, GPOS first and the legacy table after it
    /// only when GPOS had no pair adjustment.
    pub(crate) fn parse(gpos: Option<&[u8]>, kern: Option<&[u8]>) -> Result<Self, FontError> {
        let mut lookups = Vec::new();
        if let Some(gpos) = gpos {
            lookups = gpos_lookups(gpos)?;
        }
        if lookups.is_empty() {
            if let Some(kern) = kern {
                lookups = kern_subtables(kern)?
                    .into_iter()
                    .map(|one| vec![one])
                    .collect();
            }
        }
        Ok(Self { lookups })
    }

    /// The kerning of `left` followed by `right`, in font units.
    pub(crate) fn pair(&self, left: u16, right: u16) -> Kern {
        let mut total = 0_i32;
        for lookup in &self.lookups {
            let value = lookup.iter().find_map(|subtable| match subtable {
                Subtable::Pairs(pairs) => pairs
                    .get(&((u32::from(left) << 16) | u32::from(right)))
                    .copied(),
                Subtable::PairPos(pair_pos) => pair_pos.value(left, right),
            });
            if let Some(value) = value {
                total += i32::from(value);
            }
        }
        total.clamp(i32::from(Kern::MIN), i32::from(Kern::MAX)) as Kern
    }
}

/// Every lookup of the `kern` feature in GPOS, in lookup-list order and once each
/// however many feature records name it.
fn gpos_lookups(gpos: &[u8]) -> Result<Vec<Vec<Subtable>>, FontError> {
    let reader = reader(gpos, "GPOS")?;
    if reader.u16_at(0)? != 1 {
        return Ok(Vec::new());
    }
    let feature_list = reader.u16_at(6)? as usize;
    let lookup_list = reader.u16_at(8)? as usize;
    let feature_count = reader.u16_at(feature_list)?;
    let mut indices = std::collections::BTreeSet::new();
    for index in 0..usize::from(feature_count) {
        let record = feature_list + 2 + index * 6;
        if reader.bytes(record, 4)? != b"kern" {
            continue;
        }
        let feature = feature_list + reader.u16_at(record + 4)? as usize;
        let count = reader.u16_at(feature + 2)?;
        for slot in 0..usize::from(count) {
            indices.insert(reader.u16_at(feature + 4 + slot * 2)?);
        }
    }
    let lookup_count = reader.u16_at(lookup_list)?;
    let mut lookups = Vec::new();
    for lookup in indices {
        if lookup >= lookup_count {
            continue;
        }
        let base = lookup_list + reader.u16_at(lookup_list + 2 + usize::from(lookup) * 2)? as usize;
        let kind = reader.u16_at(base)?;
        let count = reader.u16_at(base + 4)?;
        let mut subtables = Vec::new();
        for slot in 0..usize::from(count) {
            let offset = base + reader.u16_at(base + 6 + slot * 2)? as usize;
            match kind {
                2 => subtables.push(Subtable::PairPos(pair_pos(gpos, offset)?)),
                9 => {
                    let wrapped = reader.u16_at(offset + 2)?;
                    let target = offset + reader.u32_at(offset + 4)? as usize;
                    if wrapped == 2 {
                        subtables.push(Subtable::PairPos(pair_pos(gpos, target)?));
                    }
                }
                _ => {}
            }
        }
        if !subtables.is_empty() {
            lookups.push(subtables);
        }
    }
    Ok(lookups)
}

/// One `PairPos` subtable, in format 1 or format 2.
fn pair_pos(gpos: &[u8], base: usize) -> Result<PairPos, FontError> {
    let reader = reader(gpos, "GPOS pair adjustment")?;
    let coverage = Coverage::parse(gpos, base + reader.u16_at(base + 2)? as usize)?;
    let format1 = reader.u16_at(base + 4)?;
    let format2 = reader.u16_at(base + 6)?;
    let (first_size, first_advance) = value_layout(format1);
    let (second_size, _) = value_layout(format2);
    let advance_at = |record: usize| -> Result<Kern, FontError> {
        match first_advance {
            Some(offset) => reader.i16_at(record + offset),
            None => Ok(0),
        }
    };
    match reader.u16_at(base)? {
        1 => {
            let count = reader.u16_at(base + 8)?;
            let mut sets = Vec::with_capacity(usize::from(count));
            for index in 0..usize::from(count) {
                let set = base + reader.u16_at(base + 10 + index * 2)? as usize;
                let pairs = reader.u16_at(set)?;
                let record_size = 2 + first_size + second_size;
                let mut values = Vec::with_capacity(usize::from(pairs));
                for pair in 0..usize::from(pairs) {
                    let record = set + 2 + pair * record_size;
                    values.push((reader.u16_at(record)?, advance_at(record + 2)?));
                }
                sets.push(values);
            }
            Ok(PairPos {
                coverage,
                classes: None,
                sets,
                matrix: Vec::new(),
            })
        }
        2 => {
            let classes1 = ClassDef::parse(gpos, base + reader.u16_at(base + 8)? as usize)?;
            let classes2 = ClassDef::parse(gpos, base + reader.u16_at(base + 10)? as usize)?;
            let rows = reader.u16_at(base + 12)?;
            let columns = reader.u16_at(base + 14)?;
            let record_size = first_size + second_size;
            let mut matrix = Vec::with_capacity(usize::from(rows));
            for row in 0..usize::from(rows) {
                let mut entries = Vec::with_capacity(usize::from(columns));
                for column in 0..usize::from(columns) {
                    let record = base + 16 + (row * usize::from(columns) + column) * record_size;
                    let value = advance_at(record)?;
                    entries.push((value != 0).then_some(value));
                }
                matrix.push(entries);
            }
            Ok(PairPos {
                coverage,
                classes: Some((classes1, classes2)),
                sets: Vec::new(),
                matrix,
            })
        }
        _ => Err(FontError::BadTable {
            tag: "GPOS",
            reason: "unknown pair adjustment format",
        }),
    }
}

/// Every format 0 subtable of a legacy `kern` table, in the OpenType version (a zero
/// version word) and in Apple's (an `0x00010000` version word, longer headers).
fn kern_subtables(kern: &[u8]) -> Result<Vec<Subtable>, FontError> {
    let reader = reader(kern, "kern")?;
    let apple = reader.u16_at(0)? == 1 && reader.u16_at(2)? == 0;
    let count = if apple {
        reader.u32_at(4)?
    } else {
        u32::from(reader.u16_at(2)?)
    };
    let mut cursor = if apple { 8 } else { 4 };
    let mut subtables = Vec::new();
    for _ in 0..count {
        let header = if apple { 8 } else { 6 };
        if cursor + header > kern.len() {
            break;
        }
        let (length, coverage, body) = if apple {
            (
                reader.u32_at(cursor)? as usize,
                reader.u16_at(cursor + 4)?,
                cursor + 8,
            )
        } else {
            (
                reader.u16_at(cursor + 2)? as usize,
                reader.u16_at(cursor + 4)? >> 8,
                cursor + 6,
            )
        };
        let format = if apple { coverage & 0x00ff } else { coverage };
        if format == 0 && body + 8 <= kern.len() {
            let pairs = reader.u16_at(body)?;
            let mut values = BTreeMap::new();
            for pair in 0..usize::from(pairs) {
                let record = body + 8 + pair * 6;
                let left = reader.u16_at(record)?;
                let right = reader.u16_at(record + 2)?;
                values.insert(
                    (u32::from(left) << 16) | u32::from(right),
                    reader.i16_at(record + 4)?,
                );
            }
            subtables.push(Subtable::Pairs(values));
        }
        cursor += length.max(header);
    }
    Ok(subtables)
}
