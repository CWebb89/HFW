//! Glyph outlines: the `loca` locator, the `glyf` records and the flattening of
//! TrueType's quadratic curves.
//!
//! A TrueType glyph is contours of points, each on or off the curve. Two on-curve
//! points with no off-curve point between them are a line; on-off-on is one quadratic
//! Bézier; two consecutive off-curve points imply an on-curve point halfway between
//! them, which is the one rule that makes a stored outline differ from the drawn one.
//! A composite glyph is a list of other glyphs with a transform each, and it is
//! resolved recursively.

use super::{
    sfnt::{reader, table, Directory, Reader},
    FontError, Point,
};

const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME: u8 = 0x10;
const Y_SAME: u8 = 0x20;

const ARG_WORDS: u16 = 0x0001;
const ARGS_ARE_XY: u16 = 0x0002;
const WE_HAVE_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const XY_SCALE: u16 = 0x0040;
const TWO_BY_TWO: u16 = 0x0080;
const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;

/// How deep a composite glyph may nest, so a malformed font cannot recurse forever.
const MAX_DEPTH: u16 = 8;

/// One closed contour, in the order the font stores it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contour {
    /// Points along the contour, each one a node of the flattened polyline.
    pub points: Vec<Point>,
}

/// The contours of a glyph or of a laid-out string, in font units or millimetres.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    /// The closed contours, outermost and holes together, in stored order.
    pub contours: Vec<Contour>,
}

impl Outline {
    /// Total nodes across every contour.
    pub fn point_count(&self) -> usize {
        self.contours
            .iter()
            .map(|contour| contour.points.len())
            .sum()
    }

    /// The outline's `(min_x, min_y, max_x, max_y)`, or zeroes when it is empty.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for point in self.contours.iter().flat_map(|contour| &contour.points) {
            bounds.0 = bounds.0.min(point[0]);
            bounds.1 = bounds.1.min(point[1]);
            bounds.2 = bounds.2.max(point[0]);
            bounds.3 = bounds.3.max(point[1]);
        }
        if bounds.0 > bounds.2 {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            bounds
        }
    }

    /// Scales every point about the origin.
    pub fn scale(&mut self, factor: f64) {
        for point in self
            .contours
            .iter_mut()
            .flat_map(|contour| &mut contour.points)
        {
            point[0] *= factor;
            point[1] *= factor;
        }
    }

    /// Translates every point.
    pub fn shift(&mut self, dx: f64, dy: f64) {
        for point in self
            .contours
            .iter_mut()
            .flat_map(|contour| &mut contour.points)
        {
            point[0] += dx;
            point[1] += dy;
        }
    }

    /// Appends another outline's contours.
    pub fn append(&mut self, other: Self) {
        self.contours.extend(other.contours);
    }

    /// The signed area of a contour, positive when the contour runs anticlockwise.
    pub fn area(contour: &Contour) -> f64 {
        let mut sum = 0.0;
        let points = &contour.points;
        for index in 0..points.len() {
            let current = points[index];
            let next = points[(index + 1) % points.len()];
            sum += current[0] * next[1] - next[0] * current[1];
        }
        sum / 2.0
    }
}

/// A glyph's stored points, needed until the contours are flattened.
struct Raw {
    points: Vec<Point>,
    on_curve: Vec<bool>,
    ends: Vec<usize>,
}

/// Reads one glyph's outline, resolving composites.
pub(super) fn outline(
    bytes: &[u8],
    tables: &Directory,
    glyph: u16,
    long_loca: bool,
    tolerance: f64,
    depth: u16,
) -> Result<Outline, FontError> {
    let loca = reader(table(bytes, tables, b"loca")?, "loca")?;
    let glyf = reader(table(bytes, tables, b"glyf")?, "glyf")?;
    if depth > MAX_DEPTH {
        return Err(FontError::BadTable {
            tag: "glyf",
            reason: "composite glyphs nest deeper than eight",
        });
    }
    let (start, end) = location(&loca, glyph, long_loca)?;
    if end <= start {
        return Ok(Outline::default());
    }
    let record = reader(glyf.bytes(start, end - start)?, "glyf")?;
    let contours = record.i16_at(0)?;
    if contours >= 0 {
        return simple(&record, contours as usize, tolerance);
    }
    composite(bytes, tables, &record, long_loca, tolerance, depth)
}

fn location(loca: &Reader<'_>, glyph: u16, long: bool) -> Result<(usize, usize), FontError> {
    let at = usize::from(glyph) * if long { 4 } else { 2 };
    let (start, end) = if long {
        (loca.u32_at(at)? as usize, loca.u32_at(at + 4)? as usize)
    } else {
        (
            usize::from(loca.u16_at(at)?) * 2,
            usize::from(loca.u16_at(at + 2)?) * 2,
        )
    };
    Ok((start, end))
}

fn simple(record: &Reader<'_>, contours: usize, tolerance: f64) -> Result<Outline, FontError> {
    if contours == 0 {
        return Ok(Outline::default());
    }
    let mut cursor = 10 + contours * 2;
    let mut ends = Vec::with_capacity(contours);
    for index in 0..contours {
        let end = usize::from(record.u16_at(10 + index * 2)?);
        if index > 0 && end <= ends[index - 1] {
            return Err(FontError::BadTable {
                tag: "glyf",
                reason: "contour end points are not increasing",
            });
        }
        ends.push(end);
    }
    let count = ends[contours - 1] + 1;
    cursor += 2 + usize::from(record.u16_at(cursor)?);
    let mut flags = Vec::with_capacity(count);
    while flags.len() < count {
        let flag = record.u8_at(cursor)?;
        cursor += 1;
        flags.push(flag);
        if flag & REPEAT != 0 {
            let repeat = record.u8_at(cursor)?;
            cursor += 1;
            for _ in 0..repeat {
                if flags.len() < count {
                    flags.push(flag);
                }
            }
        }
    }
    let mut xs = Vec::with_capacity(count);
    let mut value = 0_i32;
    for flag in &flags {
        if flag & X_SHORT != 0 {
            let delta = i32::from(record.u8_at(cursor)?);
            cursor += 1;
            value += if flag & X_SAME != 0 { delta } else { -delta };
        } else if flag & X_SAME == 0 {
            value += i32::from(record.i16_at(cursor)?);
            cursor += 2;
        }
        xs.push(value);
    }
    let mut ys = Vec::with_capacity(count);
    let mut value = 0_i32;
    for flag in &flags {
        if flag & Y_SHORT != 0 {
            let delta = i32::from(record.u8_at(cursor)?);
            cursor += 1;
            value += if flag & Y_SAME != 0 { delta } else { -delta };
        } else if flag & Y_SAME == 0 {
            value += i32::from(record.i16_at(cursor)?);
            cursor += 2;
        }
        ys.push(value);
    }
    let raw = Raw {
        points: xs
            .into_iter()
            .zip(ys)
            .map(|(x, y)| [f64::from(x), f64::from(y)])
            .collect(),
        on_curve: flags.iter().map(|flag| flag & ON_CURVE != 0).collect(),
        ends,
    };
    Ok(flatten(&raw, tolerance))
}

fn composite(
    bytes: &[u8],
    tables: &Directory,
    record: &Reader<'_>,
    long_loca: bool,
    tolerance: f64,
    depth: u16,
) -> Result<Outline, FontError> {
    let mut result = Outline::default();
    let mut cursor = 10;
    loop {
        let flags = record.u16_at(cursor)?;
        let component = record.u16_at(cursor + 2)?;
        cursor += 4;
        let (dx, dy) = if flags & ARG_WORDS != 0 {
            let first = i32::from(record.i16_at(cursor)?);
            let second = i32::from(record.i16_at(cursor + 2)?);
            cursor += 4;
            (first, second)
        } else {
            let first = i32::from(record.i8_at(cursor)?);
            let second = i32::from(record.i8_at(cursor + 1)?);
            cursor += 2;
            (first, second)
        };
        let mut matrix = [1.0, 0.0, 0.0, 1.0];
        if flags & WE_HAVE_SCALE != 0 {
            let scale = f2dot14(record, cursor)?;
            cursor += 2;
            matrix = [scale, 0.0, 0.0, scale];
        } else if flags & XY_SCALE != 0 {
            matrix = [
                f2dot14(record, cursor)?,
                0.0,
                0.0,
                f2dot14(record, cursor + 2)?,
            ];
            cursor += 4;
        } else if flags & TWO_BY_TWO != 0 {
            matrix = [
                f2dot14(record, cursor)?,
                f2dot14(record, cursor + 2)?,
                f2dot14(record, cursor + 4)?,
                f2dot14(record, cursor + 6)?,
            ];
            cursor += 8;
        }
        let mut child = outline(bytes, tables, component, long_loca, tolerance, depth + 1)?;
        if flags & ARGS_ARE_XY == 0 {
            let _ = (dx, dy);
        }
        for contour in &mut child.contours {
            for point in &mut contour.points {
                let x = point[0] * matrix[0] + point[1] * matrix[2] + f64::from(dx);
                let y = point[0] * matrix[1] + point[1] * matrix[3] + f64::from(dy);
                point[0] = x;
                point[1] = y;
            }
        }
        result.append(child);
        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    let _ = WE_HAVE_INSTRUCTIONS;
    Ok(result)
}

fn f2dot14(record: &Reader<'_>, offset: usize) -> Result<f64, FontError> {
    Ok(f64::from(record.i16_at(offset)?) / 16384.0)
}

/// Turns stored points into flattened contours.
fn flatten(raw: &Raw, tolerance: f64) -> Outline {
    let mut outline = Outline::default();
    let mut start = 0;
    for &end in &raw.ends {
        let points = &raw.points[start..=end];
        let on_curve = &raw.on_curve[start..=end];
        start = end + 1;
        if points.len() < 2 {
            continue;
        }
        let mut contour = Contour::default();
        let first_on = (0..points.len())
            .find(|index| on_curve[*index])
            .unwrap_or(0);
        let count = points.len();
        let mut current = if on_curve[first_on] {
            points[first_on]
        } else {
            midpoint(points[first_on], points[(first_on + 1) % count])
        };
        contour.points.push(current);
        let mut offset = 1;
        while offset <= count {
            let index = (first_on + offset) % count;
            let next = (index + 1) % count;
            if on_curve[index] {
                let point = points[index];
                if distance(current, point) > 0.0 {
                    contour.points.push(point);
                }
                current = point;
                offset += 1;
                continue;
            }
            let end = if on_curve[next] {
                points[next]
            } else {
                midpoint(points[index], points[next])
            };
            flatten_quad(current, points[index], end, tolerance, &mut contour.points);
            current = end;
            offset += if on_curve[next] { 2 } else { 1 };
        }
        dedup(&mut contour.points);
        if contour.points.len() >= 3 {
            outline.contours.push(contour);
        }
    }
    outline
}

fn flatten_quad(from: Point, control: Point, to: Point, tolerance: f64, out: &mut Vec<Point>) {
    let steps = quad_steps(from, control, to, tolerance);
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        let u = 1.0 - t;
        out.push([
            u * u * from[0] + 2.0 * u * t * control[0] + t * t * to[0],
            u * u * from[1] + 2.0 * u * t * control[1] + t * t * to[1],
        ]);
    }
}

/// How many chords keep a quadratic within `tolerance`. The error of an even split falls
/// with the square of the step count and is bounded by `|P0 - 2P1 + P2| / (8 n^2)`.
fn quad_steps(from: Point, control: Point, to: Point, tolerance: f64) -> usize {
    let second = [
        from[0] - 2.0 * control[0] + to[0],
        from[1] - 2.0 * control[1] + to[1],
    ];
    let magnitude = (second[0] * second[0] + second[1] * second[1]).sqrt();
    let steps = (magnitude / (8.0 * tolerance)).sqrt().ceil();
    if !steps.is_finite() || steps < 1.0 {
        1
    } else {
        (steps as usize).clamp(1, 96)
    }
}

fn midpoint(a: Point, b: Point) -> Point {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

fn distance(a: Point, b: Point) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn dedup(points: &mut Vec<Point>) {
    let mut index = 1;
    while index < points.len() {
        if distance(points[index - 1], points[index]) == 0.0 {
            points.remove(index);
        } else {
            index += 1;
        }
    }
    if points.len() >= 2 && distance(points[0], points[points.len() - 1]) == 0.0 {
        points.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_from(contour: &[(f64, f64, bool)]) -> Raw {
        Raw {
            points: contour.iter().map(|(x, y, _)| [*x, *y]).collect(),
            on_curve: contour.iter().map(|(_, _, on)| *on).collect(),
            ends: vec![contour.len() - 1],
        }
    }

    #[test]
    fn a_square_of_four_on_curve_points_stays_four_points() {
        let raw = raw_from(&[
            (0.0, 0.0, true),
            (10.0, 0.0, true),
            (10.0, 10.0, true),
            (0.0, 10.0, true),
        ]);
        let outline = flatten(&raw, 0.01);
        assert_eq!(outline.contours.len(), 1);
        assert_eq!(outline.contours[0].points.len(), 4);
        assert!((Outline::area(&outline.contours[0]).abs() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn two_curves_meeting_at_an_off_curve_point_imply_the_midpoint() {
        let raw = raw_from(&[
            (0.0, 0.0, true),
            (10.0, 0.0, false),
            (10.0, 10.0, false),
            (0.0, 10.0, true),
        ]);
        let outline = flatten(&raw, 0.05);
        assert_eq!(outline.contours.len(), 1);
        let points = &outline.contours[0].points;
        assert!(points
            .iter()
            .any(|point| (point[0] - 10.0).abs() < 1e-9 && (point[1] - 5.0).abs() < 1e-9));
        assert!(points.len() > 8, "{}", points.len());
        assert!(Outline::area(&outline.contours[0]).abs() > 60.0);
    }

    #[test]
    fn a_finer_tolerance_keeps_more_points_on_a_curve() {
        let contour = [
            (0.0, 0.0, true),
            (50.0, 80.0, false),
            (100.0, 0.0, true),
            (0.0, 0.0, true),
        ];
        let coarse = flatten(&raw_from(&contour), 1.0).point_count();
        let fine = flatten(&raw_from(&contour), 0.02).point_count();
        assert!(fine > coarse, "{fine} not finer than {coarse}");
    }

    #[test]
    fn the_step_count_is_bounded_even_for_a_degenerate_tolerance() {
        assert_eq!(quad_steps([0.0, 0.0], [0.0, 0.0], [0.0, 0.0], 1e-9), 1);
        assert!(quad_steps([0.0, 0.0], [100.0, 0.0], [0.0, 0.0], 1e-12) <= 96);
    }

    #[test]
    fn a_repeated_point_is_removed_and_the_contour_stays_closed_implicitly() {
        let raw = raw_from(&[
            (0.0, 0.0, true),
            (10.0, 0.0, true),
            (10.0, 0.0, true),
            (0.0, 10.0, true),
        ]);
        let outline = flatten(&raw, 0.01);
        assert_eq!(outline.contours[0].points.len(), 3);
    }
}
