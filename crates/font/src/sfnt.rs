//! The table directory: which tables a file holds and where each one starts.

use super::FontError;
use std::collections::BTreeMap;
use std::ops::Range;

/// The byte range of each table in the file, by four-byte tag.
pub(super) type Directory = BTreeMap<[u8; 4], Range<usize>>;

/// Reads the table directory of one face.
pub(super) fn directory(bytes: &[u8], face_index: u32) -> Result<Directory, FontError> {
    let head = reader(bytes, "the sfnt header")?;
    let tag = head.bytes(0, 4)?;
    match tag {
        [0x00, 0x01, 0x00, 0x00] | b"true" | b"typ1" => directory_at(bytes, 0),
        b"OTTO" => Err(FontError::CffOutlines),
        b"ttcf" => {
            let faces = head.u32_at(8)?;
            let index = face_index as usize;
            if index >= faces as usize {
                return Err(FontError::BadTable {
                    tag: "ttcf",
                    reason: "the collection does not hold that face",
                });
            }
            directory_at(bytes, head.u32_at(12 + index * 4)? as usize)
        }
        _ => Err(FontError::NotAFont {
            tag: String::from_utf8_lossy(tag).trim().to_owned(),
        }),
    }
}

fn directory_at(bytes: &[u8], base: usize) -> Result<Directory, FontError> {
    let reader = reader(bytes, "the sfnt header")?;
    let count = reader.u16_at(base + 4)?;
    let mut tables = Directory::new();
    for index in 0..usize::from(count) {
        let record = base + 12 + index * 16;
        let mut tag = [0_u8; 4];
        tag.copy_from_slice(reader.bytes(record, 4)?);
        let offset = reader.u32_at(record + 8)? as usize;
        let length = reader.u32_at(record + 12)? as usize;
        reader.bytes(offset, length)?;
        tables.insert(tag, offset..offset + length);
    }
    Ok(tables)
}

/// A table's bytes by tag, or an error naming what is missing.
pub(super) fn table<'a>(
    bytes: &'a [u8],
    tables: &Directory,
    tag: &[u8; 4],
) -> Result<&'a [u8], FontError> {
    let range = tables.get(tag).ok_or(FontError::MissingTable {
        tag: match tag {
            b"head" => "head",
            b"maxp" => "maxp",
            b"hhea" => "hhea",
            b"hmtx" => "hmtx",
            b"loca" => "loca",
            b"glyf" => "glyf",
            b"cmap" => "cmap",
            _ => "table",
        },
    })?;
    bytes.get(range.clone()).ok_or(FontError::BadTable {
        tag: "table",
        reason: "a table runs past the end of the file",
    })
}

/// A bounds-checked big-endian reader, named by what it is reading.
#[derive(Clone, Copy)]
pub(super) struct Reader<'a> {
    bytes: &'a [u8],
    what: &'static str,
}

impl<'a> Reader<'a> {
    /// The slice `[offset, offset + length)`.
    pub(super) fn bytes(&self, offset: usize, length: usize) -> Result<&'a [u8], FontError> {
        self.bytes
            .get(offset..offset.saturating_add(length))
            .ok_or(FontError::Truncated {
                what: self.what,
                offset,
                available: self.bytes.len().saturating_sub(offset),
            })
    }

    /// An unsigned byte.
    pub(super) fn u8_at(&self, offset: usize) -> Result<u8, FontError> {
        Ok(self.bytes(offset, 1)?[0])
    }

    /// A signed byte.
    pub(super) fn i8_at(&self, offset: usize) -> Result<i8, FontError> {
        Ok(self.u8_at(offset)? as i8)
    }

    /// A big-endian unsigned 16-bit integer.
    pub(super) fn u16_at(&self, offset: usize) -> Result<u16, FontError> {
        let bytes = self.bytes(offset, 2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// A big-endian signed 16-bit integer.
    pub(super) fn i16_at(&self, offset: usize) -> Result<i16, FontError> {
        Ok(self.u16_at(offset)? as i16)
    }

    /// A big-endian unsigned 32-bit integer.
    pub(super) fn u32_at(&self, offset: usize) -> Result<u32, FontError> {
        let bytes = self.bytes(offset, 4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

/// Wraps a table's bytes in a reader named after the table.
pub(super) fn reader<'a>(bytes: &'a [u8], what: &'static str) -> Result<Reader<'a>, FontError> {
    Ok(Reader { bytes, what })
}
