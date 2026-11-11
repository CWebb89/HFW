//! Floating-point planar geometry for the field-and-wall stage.
//!
//! The mesh and slicing crates work in exact integer nanometres. Everything
//! after the slice boundary is approximate by nature: offsets, distance
//! fields and medial axes are solved numerically. This crate holds that
//! numerical kernel in millimetres.

#![forbid(unsafe_code)]

use std::ops::{Add, Mul, Neg, Sub};

/// A point or vector in the printer XY plane, in millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V2 {
    /// X coordinate in millimetres.
    pub x: f64,
    /// Y coordinate in millimetres.
    pub y: f64,
}

impl V2 {
    /// The origin.
    pub const ZERO: V2 = V2 { x: 0.0, y: 0.0 };

    /// Creates a point from millimetres.
    pub const fn new(x: f64, y: f64) -> V2 {
        V2 { x, y }
    }

    /// Dot product.
    pub fn dot(self, o: V2) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// Two-dimensional cross product (z component).
    pub fn cross(self, o: V2) -> f64 {
        self.x * o.y - self.y * o.x
    }

    /// Squared length.
    pub fn len2(self) -> f64 {
        self.dot(self)
    }

    /// Length.
    pub fn len(self) -> f64 {
        self.len2().sqrt()
    }

    /// Unit vector, or zero when the length is zero.
    pub fn norm(self) -> V2 {
        let l = self.len();
        if l > 0.0 {
            self * (1.0 / l)
        } else {
            V2::ZERO
        }
    }

    /// Linear interpolation towards `o`.
    pub fn lerp(self, o: V2, t: f64) -> V2 {
        self + (o - self) * t
    }

    /// Distance to another point.
    pub fn dist(self, o: V2) -> f64 {
        (self - o).len()
    }

    /// Left-hand perpendicular.
    pub fn perp(self) -> V2 {
        V2::new(-self.y, self.x)
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for V2 {
    type Output = V2;
    fn mul(self, k: f64) -> V2 {
        V2::new(self.x * k, self.y * k)
    }
}

impl Neg for V2 {
    type Output = V2;
    fn neg(self) -> V2 {
        V2::new(-self.x, -self.y)
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    /// Lower corner.
    pub min: V2,
    /// Upper corner.
    pub max: V2,
}

impl Bounds {
    /// An inverted box ready for [`Bounds::add`].
    pub fn empty() -> Bounds {
        Bounds {
            min: V2::new(f64::INFINITY, f64::INFINITY),
            max: V2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }

    /// Grows the box to include `p`.
    pub fn add(&mut self, p: V2) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }

    /// Returns the box grown by `m` on every side.
    pub fn grown(&self, m: f64) -> Bounds {
        Bounds {
            min: V2::new(self.min.x - m, self.min.y - m),
            max: V2::new(self.max.x + m, self.max.y + m),
        }
    }

    /// Returns true when no point has been added.
    pub fn is_empty(&self) -> bool {
        !(self.max.x >= self.min.x && self.max.y >= self.min.y)
    }

    /// Width along X.
    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }

    /// Height along Y.
    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }
}

/// Closest point on segment `a`-`b` to `p`, with its distance.
pub fn seg_closest(p: V2, a: V2, b: V2) -> (f64, V2) {
    let ab = b - a;
    let l2 = ab.len2();
    let t = if l2 > 0.0 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let f = a + ab * t;
    ((p - f).len(), f)
}

/// Signed area of a closed ring; positive when counter-clockwise.
pub fn ring_signed_area(r: &[V2]) -> f64 {
    let n = r.len();
    let mut s = 0.0;
    for i in 0..n {
        s += r[i].cross(r[(i + 1) % n]);
    }
    0.5 * s
}

/// Even-odd point-in-ring test.
pub fn ring_contains(r: &[V2], p: V2) -> bool {
    let n = r.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (r[i], r[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn clean_ring(r: Vec<V2>) -> Vec<V2> {
    let mut out: Vec<V2> = Vec::with_capacity(r.len());
    for p in r {
        if out.last().is_none_or(|q| q.dist(p) > 1e-12) {
            out.push(p);
        }
    }
    while out.len() > 1 && out[0].dist(*out.last().unwrap()) <= 1e-12 {
        out.pop();
    }
    out
}

/// A planar region made of closed rings, outer boundaries counter-clockwise.
///
/// Ring orientation is normalised from nesting depth, so callers may pass mesh
/// cross-sections in any order and any winding.
#[derive(Clone, Debug, Default)]
pub struct Region {
    /// Closed rings; depth-even rings are counter-clockwise.
    pub rings: Vec<Vec<V2>>,
}

impl Region {
    /// Builds a region, dropping degenerate rings and normalising winding.
    pub fn new(rings: Vec<Vec<V2>>) -> Region {
        let cleaned: Vec<Vec<V2>> = rings
            .into_iter()
            .map(clean_ring)
            .filter(|r| r.len() >= 3)
            .collect();
        let depths: Vec<usize> = (0..cleaned.len())
            .map(|i| {
                (0..cleaned.len())
                    .filter(|&j| j != i && ring_contains(&cleaned[j], cleaned[i][0]))
                    .count()
            })
            .collect();
        let rings = cleaned
            .into_iter()
            .zip(depths)
            .map(|(mut r, dp)| {
                let ccw = ring_signed_area(&r) > 0.0;
                if ccw != (dp % 2 == 0) {
                    r.reverse();
                }
                r
            })
            .collect();
        Region { rings }
    }

    /// True when there is no printable area.
    pub fn is_empty(&self) -> bool {
        self.rings.is_empty()
    }

    /// Every ring edge as an ordered pair.
    pub fn segments(&self) -> Vec<(V2, V2)> {
        let mut out = Vec::new();
        for r in &self.rings {
            let n = r.len();
            for i in 0..n {
                out.push((r[i], r[(i + 1) % n]));
            }
        }
        out
    }

    /// Bounding box of all rings.
    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for r in &self.rings {
            for &p in r {
                b.add(p);
            }
        }
        b
    }

    /// Signed area, positive for a correctly wound solid region.
    pub fn area(&self) -> f64 {
        self.rings.iter().map(|r| ring_signed_area(r)).sum()
    }

    /// Total ring length.
    pub fn perimeter(&self) -> f64 {
        self.segments().iter().map(|(a, b)| a.dist(*b)).sum()
    }

    /// Even-odd containment across all rings.
    pub fn contains(&self, p: V2) -> bool {
        self.rings.iter().filter(|r| ring_contains(r, p)).count() % 2 == 1
    }
}
