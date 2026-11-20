//! The reader against FreeType, on a real font, through the system's own renderer.
//!
//! `scripts/font-oracle.py` asks FreeType — the library behind every text renderer on
//! this machine — for each glyph's advance and ink box at a given size. This test asks
//! this crate the same question and requires the same answer to within the oracle's own
//! raster resolution. A font file or a Python with Pillow is not part of the build, so a
//! missing prerequisite skips rather than fails; the gate runs this and prints the
//! deviation it measured.

use hfw_font::Face;
use std::path::{Path, PathBuf};
use std::process::Command;

const FONTS: [&str; 4] = [
    "/usr/share/fonts/TTF/FiraCode-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/FiraSans-Regular.ttf",
];

const TEXT: &str = "Aoé";
const SIZE_MM: f64 = 10.0;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels below the repository root")
        .to_path_buf()
}

fn python() -> Option<PathBuf> {
    if let Ok(named) = std::env::var("HFW_PY") {
        return Some(PathBuf::from(named));
    }
    let candidate = root().join(".venv/bin/python");
    candidate.exists().then_some(candidate)
}

fn font() -> Option<&'static str> {
    FONTS.iter().copied().find(|path| Path::new(path).exists())
}

#[test]
fn advances_and_ink_bounds_agree_with_freetype() {
    let (Some(python), Some(font)) = (python(), font()) else {
        eprintln!("skipped: no font file or no .venv/bin/python with Pillow");
        return;
    };
    let script = root().join("scripts/font-oracle.py");
    let output = Command::new(&python)
        .arg(&script)
        .arg(font)
        .arg(TEXT)
        .arg(format!("{SIZE_MM}"))
        .output()
        .expect("the oracle runs");
    assert!(
        output.status.success(),
        "the oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("the oracle prints UTF-8");
    let oracle = json::parse(&text).unwrap_or_else(|| panic!("the oracle's JSON: {text}"));

    let face = Face::parse(&std::fs::read(font).expect("the font reads"), 0).expect("it parses");
    let (_, width) = face
        .layout_mm(TEXT, SIZE_MM, 0.005)
        .expect("the string lays out");
    let mut worst_advance = 0.0_f64;
    let mut worst_ink = 0.0_f64;
    for row in oracle.index("characters").iter() {
        let character = row.index("character").as_str().expect("a character");
        let character = character.chars().next().expect("one character");
        let glyph = face.glyph_index(character).expect("the font maps it");
        let advance = face.advance_mm(glyph, SIZE_MM).expect("an advance");
        let expected = row.index("advance").as_f64().expect("an advance");
        worst_advance = worst_advance.max((advance - expected).abs());
        let outline = face.outline_mm(glyph, SIZE_MM, 0.005).expect("an outline");
        let bounds = outline.bounds();
        let Some(ink) = row.index("ink").as_object() else {
            continue;
        };
        let field = |name: &str| {
            ink.iter()
                .find(|(key, _)| key == name)
                .and_then(|(_, value)| value.as_f64())
                .unwrap_or_else(|| panic!("the oracle reports {name}"))
        };
        let pairs = [
            (bounds.0, field("min_x")),
            (bounds.2, field("max_x")),
            (bounds.3, field("max_y")),
            (bounds.1, field("min_y")),
        ];
        for (ours, theirs) in pairs {
            worst_ink = worst_ink.max((ours - theirs).abs());
        }
    }
    assert!(
        worst_advance < 1e-3,
        "{TEXT} at {SIZE_MM} mm: advance differs from FreeType by {worst_advance} mm"
    );
    assert!(
        worst_ink < 0.05,
        "{TEXT} at {SIZE_MM} mm: ink bounds differ from FreeType by {worst_ink} mm"
    );
    println!(
        "freeType agreement on {TEXT} at {SIZE_MM} mm: advance within {worst_advance:.5} mm, \
         ink within {worst_ink:.5} mm; laid-out width {width:.4} mm"
    );
}

/// A hand-written JSON reader, because a test dependency on a parser is a dependency
/// this workspace does not carry.
mod json {
    /// One parsed value: an object, an array, a number, a string, or null.
    #[derive(Clone, Debug)]
    pub enum Value {
        /// A JSON object, in document order.
        Object(Vec<(String, Value)>),
        /// A JSON array.
        Array(Vec<Value>),
        /// A number.
        Number(f64),
        /// A string.
        String(String),
        /// `null`, `true` or `false`, none of which this test distinguishes.
        Other,
    }

    impl Value {
        /// A field of an object.
        pub fn index(&self, key: &str) -> &Value {
            static NULL: Value = Value::Other;
            match self {
                Self::Object(fields) => fields
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value)
                    .unwrap_or(&NULL),
                _ => &NULL,
            }
        }

        /// The items of an array, or nothing.
        pub fn iter(&self) -> std::slice::Iter<'_, Value> {
            match self {
                Self::Array(items) => items.iter(),
                _ => [].iter(),
            }
        }

        /// The string value, or `None`.
        pub fn as_str(&self) -> Option<&str> {
            match self {
                Self::String(text) => Some(text),
                _ => None,
            }
        }

        /// The numeric value, or `None`.
        pub fn as_f64(&self) -> Option<f64> {
            match self {
                Self::Number(value) => Some(*value),
                _ => None,
            }
        }

        /// The object's fields, or `None`.
        pub fn as_object(&self) -> Option<&[(String, Value)]> {
            match self {
                Self::Object(fields) => Some(fields),
                _ => None,
            }
        }
    }

    /// Parses a JSON document, or `None` when it is malformed.
    pub fn parse(text: &str) -> Option<Value> {
        let bytes = text.as_bytes();
        let mut cursor = 0;
        let value = parse_value(bytes, &mut cursor)?;
        Some(value)
    }

    fn skip(bytes: &[u8], cursor: &mut usize) {
        while *cursor < bytes.len() && (bytes[*cursor] as char).is_ascii_whitespace() {
            *cursor += 1;
        }
    }

    fn parse_value(bytes: &[u8], cursor: &mut usize) -> Option<Value> {
        skip(bytes, cursor);
        let byte = *bytes.get(*cursor)?;
        match byte {
            b'{' => {
                *cursor += 1;
                let mut fields = Vec::new();
                loop {
                    skip(bytes, cursor);
                    if *bytes.get(*cursor)? == b'}' {
                        *cursor += 1;
                        return Some(Value::Object(fields));
                    }
                    let key = parse_string(bytes, cursor)?;
                    skip(bytes, cursor);
                    if *bytes.get(*cursor)? != b':' {
                        return None;
                    }
                    *cursor += 1;
                    fields.push((key, parse_value(bytes, cursor)?));
                    skip(bytes, cursor);
                    match *bytes.get(*cursor)? {
                        b',' => *cursor += 1,
                        b'}' => {
                            *cursor += 1;
                            return Some(Value::Object(fields));
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                *cursor += 1;
                let mut items = Vec::new();
                loop {
                    skip(bytes, cursor);
                    if *bytes.get(*cursor)? == b']' {
                        *cursor += 1;
                        return Some(Value::Array(items));
                    }
                    items.push(parse_value(bytes, cursor)?);
                    skip(bytes, cursor);
                    match *bytes.get(*cursor)? {
                        b',' => *cursor += 1,
                        b']' => {
                            *cursor += 1;
                            return Some(Value::Array(items));
                        }
                        _ => return None,
                    }
                }
            }
            b'"' => Some(Value::String(parse_string(bytes, cursor)?)),
            b't' | b'f' | b'n' => {
                while *cursor < bytes.len() && (bytes[*cursor] as char).is_ascii_alphanumeric() {
                    *cursor += 1;
                }
                Some(Value::Other)
            }
            _ => {
                let start = *cursor;
                while *cursor < bytes.len()
                    && matches!(
                        bytes[*cursor],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    *cursor += 1;
                }
                let text = std::str::from_utf8(&bytes[start..*cursor]).ok()?;
                Some(Value::Number(text.parse().ok()?))
            }
        }
    }

    fn parse_string(bytes: &[u8], cursor: &mut usize) -> Option<String> {
        if *bytes.get(*cursor)? != b'"' {
            return None;
        }
        *cursor += 1;
        let mut text = String::new();
        loop {
            let byte = *bytes.get(*cursor)?;
            *cursor += 1;
            match byte {
                b'"' => return Some(text),
                b'\\' => {
                    let escape = *bytes.get(*cursor)?;
                    *cursor += 1;
                    match escape {
                        b'u' => {
                            let digits = bytes.get(*cursor..*cursor + 4)?;
                            *cursor += 4;
                            let code =
                                u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
                            text.push(char::from_u32(code)?);
                        }
                        b'n' => text.push('\n'),
                        b't' => text.push('\t'),
                        b'r' => text.push('\r'),
                        other => text.push(other as char),
                    }
                }
                _ => {
                    let start = *cursor - 1;
                    let end = utf8_end(bytes, start);
                    text.push_str(std::str::from_utf8(&bytes[start..end]).ok()?);
                    *cursor = end;
                }
            }
        }
    }

    fn utf8_end(bytes: &[u8], start: usize) -> usize {
        let byte = bytes[start];
        let width = if byte < 0x80 {
            1
        } else if byte >> 5 == 0b110 {
            2
        } else if byte >> 4 == 0b1110 {
            3
        } else {
            4
        };
        (start + width).min(bytes.len())
    }
}
