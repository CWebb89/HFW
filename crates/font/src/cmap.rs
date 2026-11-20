//! The character map: which glyph a character selects.
//!
//! Only the Unicode subtables are read — format 4 (BMP, the one every font carries)
//! and format 12 (the whole range, which a font with emoji or CJK needs). Formats 0 and
//! 6 are Mac encodings and are skipped: a font that carries only those has no Unicode
//! map, and a caller asking for a character gets no glyph rather than the wrong one.

use super::{
    sfnt::{reader, Reader},
    FontError,
};

/// A parsed Unicode character map.
#[derive(Clone, Debug, Default)]
pub(super) struct CharMap {
    ranges: Vec<Range>,
}

/// One contiguous mapping, so a lookup is a binary search over ranges.
#[derive(Clone, Copy, Debug)]
struct Range {
    start: u32,
    end: u32,
    first_glyph: u16,
}

impl CharMap {
    /// Reads the best Unicode subtable in a `cmap` table.
    pub(super) fn parse(table: &[u8]) -> Result<Self, FontError> {
        let reader = reader(table, "cmap")?;
        let count = reader.u16_at(2)?;
        let mut best: Option<(u8, usize)> = None;
        for index in 0..usize::from(count) {
            let record = 4 + index * 8;
            let platform = reader.u16_at(record)?;
            let encoding = reader.u16_at(record + 2)?;
            let offset = reader.u32_at(record + 4)? as usize;
            let rank = match (platform, encoding) {
                (3, 10) | (0, 4) | (0, 5) | (0, 6) => 3,
                (3, 1) | (0, 3) => 2,
                (3, 0) | (0, 1) | (0, 2) => 1,
                _ => 0,
            };
            if rank == 0 {
                continue;
            }
            let format = reader.u16_at(offset)?;
            let rank = if format == 12 {
                rank + 3
            } else if format == 4 {
                rank
            } else {
                0
            };
            if rank == 0 {
                continue;
            }
            if best.map(|(existing, _)| rank > existing).unwrap_or(true) {
                best = Some((rank, offset));
            }
        }
        let Some((_, offset)) = best else {
            return Ok(Self::default());
        };
        match reader.u16_at(offset)? {
            4 => Self::format4(&reader, offset),
            12 => Self::format12(&reader, offset),
            _ => Ok(Self::default()),
        }
    }

    fn format4(reader: &Reader<'_>, offset: usize) -> Result<Self, FontError> {
        let segments = usize::from(reader.u16_at(offset + 6)?) / 2;
        let ends = offset + 14;
        let starts = ends + segments * 2 + 2;
        let deltas = starts + segments * 2;
        let ranges_at = deltas + segments * 2;
        let mut ranges = Vec::with_capacity(segments);
        for segment in 0..segments {
            let end = reader.u16_at(ends + segment * 2)?;
            let start = reader.u16_at(starts + segment * 2)?;
            if start > end || start == 0xFFFF {
                continue;
            }
            let delta = reader.i16_at(deltas + segment * 2)?;
            let range = reader.u16_at(ranges_at + segment * 2)?;
            if range == 0 {
                ranges.push(Range {
                    start: u32::from(start),
                    end: u32::from(end),
                    first_glyph: (start.wrapping_add(delta as u16)),
                });
                continue;
            }
            for code in start..=end {
                let index = ranges_at + segment * 2 + usize::from(range);
                let within = code.wrapping_sub(start) as usize;
                if let Ok(glyph) = reader.u16_at(index + within * 2) {
                    if glyph != 0 {
                        ranges.push(Range {
                            start: u32::from(code),
                            end: u32::from(code),
                            first_glyph: glyph.wrapping_add(delta as u16),
                        });
                    }
                }
            }
        }
        Ok(Self { ranges })
    }

    fn format12(reader: &Reader<'_>, offset: usize) -> Result<Self, FontError> {
        let groups = reader.u32_at(offset + 12)? as usize;
        let mut ranges = Vec::with_capacity(groups.min(4096));
        for group in 0..groups {
            let at = offset + 16 + group * 12;
            ranges.push(Range {
                start: reader.u32_at(at)?,
                end: reader.u32_at(at + 4)?,
                first_glyph: reader.u32_at(at + 8)?.min(u32::from(u16::MAX)) as u16,
            });
        }
        Ok(Self { ranges })
    }

    /// The glyph for a character, or `None`.
    pub(super) fn glyph(&self, character: char) -> Option<u16> {
        let code = character as u32;
        let index = self
            .ranges
            .binary_search_by(|range| {
                if code < range.start {
                    std::cmp::Ordering::Greater
                } else if code > range.end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        let range = self.ranges[index];
        let offset = code - range.start;
        let glyph = u32::from(range.first_glyph).checked_add(offset)?;
        u16::try_from(glyph).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_map_maps_nothing() {
        let map = CharMap::default();
        assert!(map.glyph('A').is_none());
    }

    #[test]
    fn a_single_range_is_searched_by_code() {
        let map = CharMap {
            ranges: vec![Range {
                start: 0x41,
                end: 0x5A,
                first_glyph: 40,
            }],
        };
        assert_eq!(map.glyph('A'), Some(40));
        assert_eq!(map.glyph('Z'), Some(40 + 25));
        assert!(map.glyph('a').is_none());
        assert!(map.glyph('\u{100}').is_none());
    }
}
