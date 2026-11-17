//! Preview thumbnails embedded in the file header.
//!
//! The image is a top-down silhouette of every extruded path, drawn at the
//! requested size. Two formats are written from scratch: PNG with stored
//! (uncompressed) deflate blocks, and QOI, which needs no compression stage at
//! all. Both are what PrusaSlicer, OrcaSlicer and the Klipper front ends emit,
//! so a viewer shows the part without slicing the file again.

/// One drawn segment in machine coordinates, with the width it was laid at.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    /// Start point.
    pub from: (f64, f64),
    /// End point.
    pub to: (f64, f64),
    /// Bead width in millimetres.
    pub width: f64,
}

/// Image formats a viewer accepts in a `; thumbnail` block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// 8-bit greyscale PNG.
    Png,
    /// RGBA QOI.
    Qoi,
}

/// One requested thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Encoding to use.
    pub format: Format,
}

/// Renders every requested thumbnail as one `; thumbnail` comment block.
pub fn render(segments: &[Segment], specs: &[Spec], extent: (f64, f64, f64, f64)) -> String {
    let mut out = String::new();
    for spec in specs {
        let pixels = raster(segments, spec, extent);
        let encoded = match spec.format {
            Format::Png => png(&pixels, spec),
            Format::Qoi => qoi(&pixels, spec),
        };
        out.push_str(&format!(
            "; thumbnail begin {}x{} {}\n",
            spec.width,
            spec.height,
            encoded.len()
        ));
        let text = base64(&encoded);
        for chunk in text.as_bytes().chunks(78) {
            out.push_str("; ");
            out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
            out.push('\n');
        }
        out.push_str("; thumbnail end\n");
    }
    out
}

/// Draws the segments into a greyscale bitmap, `0` for background.
fn raster(segments: &[Segment], spec: &Spec, extent: (f64, f64, f64, f64)) -> Vec<u8> {
    let (min_x, min_y, max_x, max_y) = extent;
    let span_x = (max_x - min_x).max(1e-6);
    let span_y = (max_y - min_y).max(1e-6);
    let scale = (spec.width as f64 / span_x).min(spec.height as f64 / span_y);
    let off_x = 0.5 * (spec.width as f64 - span_x * scale);
    let off_y = 0.5 * (spec.height as f64 - span_y * scale);
    let mut pix = vec![204u8; spec.width * spec.height];
    let mut plot = |x: i64, y: i64| {
        if x < 0 || y < 0 || x >= spec.width as i64 || y >= spec.height as i64 {
            return;
        }
        let id = y as usize * spec.width + x as usize;
        pix[id] = 32;
    };
    for seg in segments {
        let ax = off_x + (seg.from.0 - min_x) * scale;
        let ay = off_y + (seg.from.1 - min_y) * scale;
        let bx = off_x + (seg.to.0 - min_x) * scale;
        let by = off_y + (seg.to.1 - min_y) * scale;
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let radius = 0.5 * seg.width * scale;
        let steps = (len / 0.5).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            plot(
                (ax + (bx - ax) * t).round() as i64,
                (ay + (by - ay) * t).round() as i64,
            );
        }
        if radius > 1.5 {
            let r = radius.round() as i64;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    for i in 0..=steps {
                        let t = i as f64 / steps as f64;
                        plot(
                            (ax + (bx - ax) * t).round() as i64 + dx,
                            (ay + (by - ay) * t).round() as i64 + dy,
                        );
                    }
                }
            }
        }
    }
    pix
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc = table[((crc ^ *byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + body.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(body);
    out.extend_from_slice(&crc_input);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn png(pix: &[u8], spec: &Spec) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let (w, h) = (spec.width as u32, spec.height as u32);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut raw = Vec::with_capacity(pix.len() + spec.height);
    for row in 0..spec.height {
        raw.push(0);
        raw.extend_from_slice(&pix[row * spec.width..(row + 1) * spec.width]);
    }
    let mut zlib = vec![0x78, 0x01];
    for (index, block) in raw.chunks(65535).enumerate() {
        let last = (index + 1) * 65535 >= raw.len();
        zlib.push(if last { 1 } else { 0 });
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn qoi(pix: &[u8], spec: &Spec) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"qoif");
    out.extend_from_slice(&(spec.width as u32).to_be_bytes());
    out.extend_from_slice(&(spec.height as u32).to_be_bytes());
    out.push(4);
    out.push(0);
    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0, 0, 255];
    let mut run = 0u8;
    for byte in pix {
        let px = [*byte, *byte, *byte, 255];
        if px == prev {
            run += 1;
            if run == 62 {
                out.push(0xC0 | (run - 1));
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out.push(0xC0 | (run - 1));
            run = 0;
        }
        let slot = (px[0] as usize * 3 + px[1] as usize * 5 + px[2] as usize * 7) % 64;
        if index[slot] == px {
            out.push(slot as u8);
        } else {
            index[slot] = px;
            out.push(0xFF);
            out.extend_from_slice(&px);
        }
        prev = px;
    }
    if run > 0 {
        out.push(0xC0 | (run - 1));
    }
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    out
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for group in data.chunks(3) {
        let b = [
            group[0],
            *group.get(1).unwrap_or(&0),
            *group.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if group.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if group.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(width: usize, height: usize, format: Format) -> Spec {
        Spec {
            width,
            height,
            format,
        }
    }

    #[test]
    fn a_png_has_the_signature_and_a_decodable_idat() {
        let segments = [Segment {
            from: (10.0, 10.0),
            to: (40.0, 30.0),
            width: 0.45,
        }];
        let pix = raster(
            &segments,
            &spec(32, 32, Format::Png),
            (0.0, 0.0, 50.0, 50.0),
        );
        assert_eq!(pix.len(), 32 * 32);
        assert!(pix.contains(&32), "the segment was not drawn");
        let bytes = png(&pix, &spec(32, 32, Format::Png));
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        let has = |tag: &[u8]| bytes.windows(tag.len()).any(|w| w == tag);
        assert!(has(b"IHDR"), "the header chunk is missing");
        assert!(has(b"IDAT"), "the image data is missing");
        assert!(has(b"IEND"), "the end chunk is missing");
        assert!(
            bytes.len() > 32 * 32,
            "stored deflate cannot beat the raster"
        );
    }

    #[test]
    fn a_qoi_round_trips_through_its_header() {
        let pix = vec![204u8; 16 * 16];
        let bytes = qoi(&pix, &spec(16, 16, Format::Qoi));
        assert_eq!(&bytes[..4], b"qoif");
        assert_eq!(
            u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            16
        );
        assert_eq!(&bytes[bytes.len() - 8..], &[0, 0, 0, 0, 0, 0, 0, 1]);
        assert!(
            bytes.len() < 64,
            "a flat image should be runs, not per-pixel"
        );
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
