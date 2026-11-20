//! The binary G-code container, built from the format owner's specification.
//!
//! A bgcode file is a ten-byte header and an ordered list of blocks: metadata
//! as INI text, one block per thumbnail, then the G-code itself. What it carries
//! is the same file the ASCII writer produces, so a container cannot change what
//! a machine prints — only how the file is stored and checked.
//!
//! Nothing here compresses or re-encodes a payload. The specification allows
//! `Compression = 0` and `Encoding = 0`, so the bytes of the G-code block are the
//! bytes the writer wrote, and the checksum is taken over data that was never
//! transformed — the one reading of "block checksum" that is unambiguous while
//! no compression is used. A block that claims either optimisation is refused by
//! name rather than decoded on a guess.
//!
//! Source: `libbgcode/doc/specifications.md`, a byte-level document about the
//! container, not slicer source — see `docs/PROVENANCE.md`.

use hfw_config::PrinterConfig;

use crate::thumb::{Format, Image};
use crate::writer::Gcode;

/// File magic, as the specification spells it.
pub const MAGIC: [u8; 4] = *b"GCDE";

/// Container version this writer emits and this reader accepts.
pub const VERSION: u32 = 1;

/// Checksum algorithms the file header may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checksum {
    /// No checksum bytes follow any block.
    None,
    /// A 32-bit CRC follows every block.
    Crc32,
}

/// Block types, numbered as the specification numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockType {
    /// Generic metadata about the file.
    FileMetadata,
    /// The G-code itself.
    Gcode,
    /// Metadata about the software that wrote the file.
    SlicerMetadata,
    /// Metadata about the machine.
    PrinterMetadata,
    /// Metadata about the print.
    PrintMetadata,
    /// One encoded image.
    Thumbnail,
}

impl BlockType {
    /// The type as the file stores it.
    pub fn code(self) -> u16 {
        match self {
            BlockType::FileMetadata => 0,
            BlockType::Gcode => 1,
            BlockType::SlicerMetadata => 2,
            BlockType::PrinterMetadata => 3,
            BlockType::PrintMetadata => 4,
            BlockType::Thumbnail => 5,
        }
    }

    /// The type stored under `code`, if the specification defines one.
    pub fn from_code(code: u16) -> Option<BlockType> {
        match code {
            0 => Some(BlockType::FileMetadata),
            1 => Some(BlockType::Gcode),
            2 => Some(BlockType::SlicerMetadata),
            3 => Some(BlockType::PrinterMetadata),
            4 => Some(BlockType::PrintMetadata),
            5 => Some(BlockType::Thumbnail),
            _ => None,
        }
    }

    /// Bytes of parameters this block type carries.
    fn parameter_bytes(self) -> usize {
        match self {
            BlockType::Thumbnail => 6,
            _ => 2,
        }
    }
}

/// A container read back into what the writer put in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decoded {
    /// The G-code text, every G-code block in file order.
    pub text: String,
    /// The thumbnail payloads, in block order.
    pub thumbnails: Vec<Image>,
    /// The machine's metadata rows, as the file ordered them.
    pub printer: Vec<(String, String)>,
    /// How many blocks the file held, metadata included.
    pub blocks: usize,
}

/// Everything that can stop a bgcode file being read.
#[derive(Clone, Debug, PartialEq)]
pub enum BgcodeError {
    /// The first four bytes are not the container's magic.
    NotBgcode,
    /// The file ends inside a structure that says it needs more bytes.
    Truncated {
        /// What was being read when the bytes ran out.
        what: &'static str,
    },
    /// The version is not one this reader implements.
    UnsupportedVersion(u32),
    /// The checksum algorithm is not one the specification defines.
    UnknownChecksum(u16),
    /// The block type is not one the specification defines.
    UnknownBlock(u16),
    /// A block is compressed, and this implementation reads only stored blocks.
    Compressed {
        /// The block type that carried it.
        block: u16,
        /// The compression algorithm it named.
        algorithm: u16,
    },
    /// A G-code block is encoded, and this implementation reads only plain text.
    Encoded {
        /// The block type that carried it.
        block: u16,
        /// The encoding it named.
        algorithm: u16,
    },
    /// A block's stored checksum disagrees with its data.
    BadChecksum {
        /// The block type the checksum belonged to.
        block: u16,
        /// The checksum the file stores.
        recorded: u32,
        /// The checksum its data produces.
        computed: u32,
    },
    /// A metadata block holds a line that is not `key = value` INI.
    BadMetadata {
        /// The block type carrying the line.
        block: u16,
        /// The line, as the file stores it.
        line: String,
    },
    /// A block that should be text is not valid UTF-8.
    NotText {
        /// The block type carrying the bytes.
        block: u16,
    },
    /// A thumbnail block names a format this engine does not write.
    UnknownImageFormat(u16),
}

impl std::fmt::Display for BgcodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BgcodeError::NotBgcode => write!(f, "not a binary G-code file: the magic is missing"),
            BgcodeError::Truncated { what } => write!(f, "the file ends in the middle of {what}"),
            BgcodeError::UnsupportedVersion(version) => write!(
                f,
                "binary G-code version {version} is not one this engine reads (it reads {VERSION})"
            ),
            BgcodeError::UnknownChecksum(code) => {
                write!(f, "checksum type {code} is not none (0) or CRC32 (1)")
            }
            BgcodeError::UnknownBlock(code) => write!(f, "block type {code} is not defined"),
            BgcodeError::Compressed { block, algorithm } => write!(
                f,
                "block type {block} is compressed with algorithm {algorithm}; this engine reads \
                 stored blocks only, so the file has to be written with compression 0"
            ),
            BgcodeError::Encoded { block, algorithm } => write!(
                f,
                "block type {block} is encoded with algorithm {algorithm}; this engine reads \
                 plain G-code (encoding 0) only"
            ),
            BgcodeError::BadChecksum {
                block,
                recorded,
                computed,
            } => write!(
                f,
                "block type {block} fails its checksum: the file stores {recorded:08X}, its data \
                 produces {computed:08X}"
            ),
            BgcodeError::BadMetadata { block, line } => {
                write!(
                    f,
                    "block type {block} holds a line that is not `key = value`: {line:?}"
                )
            }
            BgcodeError::NotText { block } => {
                write!(f, "block type {block} is not valid UTF-8 text")
            }
            BgcodeError::UnknownImageFormat(code) => {
                write!(f, "thumbnail format {code} is not PNG (0) or QOI (2)")
            }
        }
    }
}

impl std::error::Error for BgcodeError {}

/// Writes a finished G-code file as the binary container.
///
/// The G-code text is stored verbatim, so the file prints exactly what the ASCII
/// one would; only the thumbnails move out of the header comment into blocks of
/// their own.
pub fn encode(gcode: &Gcode, cfg: &PrinterConfig) -> Vec<u8> {
    let mut out = Vec::with_capacity(gcode.text.len() + 4096);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());

    let producer = format!(
        "producer = HFW {}\nversion = {}\n",
        env!("CARGO_PKG_VERSION"),
        VERSION
    );
    block(
        &mut out,
        BlockType::FileMetadata,
        &0u16.to_le_bytes(),
        producer.as_bytes(),
    );
    block(
        &mut out,
        BlockType::PrinterMetadata,
        &0u16.to_le_bytes(),
        ini(&printer_rows(cfg)).as_bytes(),
    );
    for image in &gcode.thumbnails {
        let mut params = [0u8; 6];
        params[0..2].copy_from_slice(&image_format(image.format).to_le_bytes());
        params[2..4].copy_from_slice(&(image.width as u16).to_le_bytes());
        params[4..6].copy_from_slice(&(image.height as u16).to_le_bytes());
        block(&mut out, BlockType::Thumbnail, &params, &image.bytes);
    }
    block(
        &mut out,
        BlockType::PrintMetadata,
        &0u16.to_le_bytes(),
        ini(&print_rows(gcode, cfg)).as_bytes(),
    );
    block(
        &mut out,
        BlockType::SlicerMetadata,
        &0u16.to_le_bytes(),
        ini(&slicer_rows()).as_bytes(),
    );
    block(
        &mut out,
        BlockType::Gcode,
        &0u16.to_le_bytes(),
        gcode.text.as_bytes(),
    );
    out
}

/// Reads a binary container back into its G-code text and its thumbnails.
pub fn decode(bytes: &[u8]) -> Result<Decoded, BgcodeError> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.take(4, "the file header")? != MAGIC {
        return Err(BgcodeError::NotBgcode);
    }
    let version = reader.u32("the file header")?;
    if version != VERSION {
        return Err(BgcodeError::UnsupportedVersion(version));
    }
    let checksum = match reader.u16("the file header")? {
        0 => Checksum::None,
        1 => Checksum::Crc32,
        other => return Err(BgcodeError::UnknownChecksum(other)),
    };

    let mut decoded = Decoded::default();
    while reader.at < reader.bytes.len() {
        let kind = reader.u16("a block header")?;
        let compression = reader.u16("a block header")?;
        let stored = reader.u32("a block header")?;
        let block = BlockType::from_code(kind).ok_or(BgcodeError::UnknownBlock(kind))?;
        if compression != 0 {
            return Err(BgcodeError::Compressed {
                block: kind,
                algorithm: compression,
            });
        }
        let parameters = reader.take(block.parameter_bytes(), "a block's parameters")?;
        let data = reader.take(stored as usize, "a block's data")?;
        if checksum == Checksum::Crc32 {
            let recorded = reader.u32("a block's checksum")?;
            let computed = crate::thumb::crc32(data);
            if recorded != computed {
                return Err(BgcodeError::BadChecksum {
                    block: kind,
                    recorded,
                    computed,
                });
            }
        }
        decoded.blocks += 1;
        let encoding = u16::from_le_bytes([parameters[0], parameters[1]]);
        match block {
            BlockType::Gcode => {
                if encoding != 0 {
                    return Err(BgcodeError::Encoded {
                        block: kind,
                        algorithm: encoding,
                    });
                }
                let text =
                    std::str::from_utf8(data).map_err(|_| BgcodeError::NotText { block: kind })?;
                decoded.text.push_str(text);
            }
            BlockType::Thumbnail => {
                let format = match u16::from_le_bytes([parameters[0], parameters[1]]) {
                    0 => Format::Png,
                    2 => Format::Qoi,
                    other => return Err(BgcodeError::UnknownImageFormat(other)),
                };
                decoded.thumbnails.push(Image {
                    width: u16::from_le_bytes([parameters[2], parameters[3]]) as usize,
                    height: u16::from_le_bytes([parameters[4], parameters[5]]) as usize,
                    format,
                    bytes: data.to_vec(),
                });
            }
            BlockType::PrinterMetadata => {
                decoded.printer = parse_ini(kind, data)?;
            }
            BlockType::FileMetadata | BlockType::PrintMetadata | BlockType::SlicerMetadata => {
                parse_ini(kind, data)?;
            }
        }
    }
    Ok(decoded)
}

fn ini(rows: &[(&str, String)]) -> String {
    let mut out = String::new();
    for (key, value) in rows {
        out.push_str(key);
        out.push_str(" = ");
        out.push_str(value);
        out.push('\n');
    }
    out
}

fn parse_ini(kind: u16, data: &[u8]) -> Result<Vec<(String, String)>, BgcodeError> {
    let text = std::str::from_utf8(data).map_err(|_| BgcodeError::NotText { block: kind })?;
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| BgcodeError::BadMetadata {
                block: kind,
                line: line.to_owned(),
            })?;
        rows.push((key.trim().to_owned(), value.trim().to_owned()));
    }
    Ok(rows)
}

fn image_format(format: Format) -> u16 {
    match format {
        Format::Png => 0,
        Format::Qoi => 2,
    }
}

/// One block: header, parameters, data, checksum.
///
/// `Compression = 0` makes the header eight bytes instead of twelve: the
/// compressed-size field is the one the file leaves out. The checksum is taken
/// over bytes that were never transformed.
fn block(out: &mut Vec<u8>, kind: BlockType, parameters: &[u8], data: &[u8]) {
    out.extend_from_slice(&kind.code().to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(parameters);
    out.extend_from_slice(data);
    out.extend_from_slice(&crate::thumb::crc32(data).to_le_bytes());
}

fn printer_rows(cfg: &PrinterConfig) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    for (key, value) in [
        ("printer_model", &cfg.printer_model),
        ("printer_variant", &cfg.printer_variant),
        ("printer_vendor", &cfg.printer_vendor),
    ] {
        if !value.trim().is_empty() {
            rows.push((key, value.trim().to_owned()));
        }
    }
    rows.push(("nozzle_diameter", number(cfg.nozzle_diameter)));
    rows.push(("bed_x", number(cfg.bed_x)));
    rows.push(("bed_y", number(cfg.bed_y)));
    rows.push(("max_z", number(cfg.max_z)));
    rows
}

fn print_rows(gcode: &Gcode, cfg: &PrinterConfig) -> Vec<(&'static str, String)> {
    let stats = &gcode.stats;
    vec![
        ("layers", stats.layers.to_string()),
        ("estimated_seconds", number(stats.est_seconds)),
        ("filament_mm", number(stats.filament_mm)),
        ("filament_g", number(stats.filament_g)),
        ("volume_mm3", number(stats.volume_mm3)),
        ("layer_height", number(cfg.layer_height)),
        ("first_layer_height", number(cfg.first_layer_height)),
        ("perimeters", cfg.walls.to_string()),
        ("fill_density", number(cfg.infill_density * 100.0)),
        ("nozzle_temperature", number(cfg.nozzle_temp)),
        ("bed_temperature", number(cfg.bed_temp)),
    ]
}

fn slicer_rows() -> Vec<(&'static str, String)> {
    vec![
        ("slicer", "HFW".to_owned()),
        ("slicer_version", env!("CARGO_PKG_VERSION").to_owned()),
    ]
}

/// A number as a profile or a display would write it: at most three decimals,
/// with the trailing zeros a machine never reads left out.
fn number(value: f64) -> String {
    let mut text = format!("{value:.3}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" {
        text = "0".to_owned();
    }
    text
}

/// A cursor over the file that refuses to read past its end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize, what: &'static str) -> Result<&'a [u8], BgcodeError> {
        let end = self
            .at
            .checked_add(count)
            .ok_or(BgcodeError::Truncated { what })?;
        if end > self.bytes.len() {
            return Err(BgcodeError::Truncated { what });
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u16(&mut self, what: &'static str) -> Result<u16, BgcodeError> {
        let bytes = self.take(2, what)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, BgcodeError> {
        let bytes = self.take(4, what)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}
