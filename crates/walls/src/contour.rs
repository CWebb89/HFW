//! Marching-squares extraction of iso-lines from a level field.

use std::collections::{HashMap, HashSet};

use hfw_kernel::V2;

use crate::field::Grid;

const NO: usize = usize::MAX;

/// One traced iso-line with per-vertex bead width and target iso-radius.
pub struct RawLine {
    /// Vertex positions.
    pub pts: Vec<V2>,
    /// Bead width sampled at each vertex.
    pub w: Vec<f64>,
    /// Target radius of the wall this vertex belongs to.
    pub lv: Vec<f64>,
    /// True when the line returns to its first vertex.
    pub closed: bool,
}

fn add(map: &mut HashMap<usize, [usize; 2]>, a: usize, b: usize) {
    let e = map.entry(a).or_insert([NO, NO]);
    if e[0] == NO {
        e[0] = b;
    } else {
        e[1] = b;
    }
}

fn walk(
    start: usize,
    nbr: &HashMap<usize, [usize; 2]>,
    visited: &mut HashSet<usize>,
) -> Vec<usize> {
    let mut seq = vec![start];
    visited.insert(start);
    let mut prev = NO;
    let mut cur = start;
    loop {
        let nb = nbr[&cur];
        let mut next = NO;
        for &c in &nb {
            if c != NO && c != prev && !visited.contains(&c) {
                next = c;
                break;
            }
        }
        if next == NO {
            break;
        }
        visited.insert(next);
        seq.push(next);
        prev = cur;
        cur = next;
    }
    seq
}

/// Traces the zero level of `phi`, carrying `bw` and `lv` through the crossing
/// interpolation so each output vertex keeps its local bead geometry.
pub fn trace(g: &Grid, phi: &[f64], bw: &[f64], lv: &[f64]) -> Vec<RawLine> {
    let nx = g.nx;
    let ny = g.ny;
    let mut segs: Vec<(usize, usize)> = Vec::new();
    for iy in 0..ny - 1 {
        for ix in 0..nx - 1 {
            let a = iy * nx + ix;
            let v = [phi[a], phi[a + 1], phi[a + nx + 1], phi[a + nx]];
            let mut code = 0u8;
            for (k, value) in v.iter().enumerate() {
                if *value < 0.0 {
                    code |= 1 << k;
                }
            }
            if code == 0 || code == 15 {
                continue;
            }
            let e = [2 * a, 2 * (a + 1) + 1, 2 * (a + nx), 2 * a + 1];
            let center = 0.25 * (v[0] + v[1] + v[2] + v[3]);
            let mut push = |p: usize, q: usize| segs.push((e[p], e[q]));
            match code {
                1 | 14 => push(3, 0),
                2 | 13 => push(0, 1),
                3 | 12 => push(3, 1),
                4 | 11 => push(1, 2),
                6 | 9 => push(0, 2),
                7 | 8 => push(3, 2),
                5 => {
                    if center < 0.0 {
                        push(0, 1);
                        push(3, 2);
                    } else {
                        push(3, 0);
                        push(1, 2);
                    }
                }
                10 => {
                    if center < 0.0 {
                        push(3, 0);
                        push(1, 2);
                    } else {
                        push(0, 1);
                        push(3, 2);
                    }
                }
                _ => {}
            }
        }
    }
    let mut nbr: HashMap<usize, [usize; 2]> = HashMap::with_capacity(segs.len() * 2);
    for &(a, b) in &segs {
        add(&mut nbr, a, b);
        add(&mut nbr, b, a);
    }
    let mut ids: Vec<usize> = nbr.keys().copied().collect();
    ids.sort_unstable();
    let vert = |e: usize| -> (V2, f64, f64) {
        let a = e / 2;
        let b = if e % 2 == 1 { a + nx } else { a + 1 };
        let (fa, fb) = (phi[a], phi[b]);
        let den = fa - fb;
        let t = if den != 0.0 {
            (fa / den).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let p = g.pos_id(a).lerp(g.pos_id(b), t);
        let (wa, wb) = (bw[a], bw[b]);
        let w = if wa.is_finite() && wb.is_finite() {
            wa + (wb - wa) * t
        } else if wa.is_finite() {
            wa
        } else {
            wb
        };
        let (la, lb) = (lv[a], lv[b]);
        let l = if la.is_finite() && lb.is_finite() {
            la + (lb - la) * t
        } else {
            f64::NAN
        };
        (p, w, l)
    };
    let mut visited: HashSet<usize> = HashSet::with_capacity(ids.len());
    let mut out = Vec::new();
    let mut emit = |seq: Vec<usize>, closed: bool| {
        let mut pts = Vec::with_capacity(seq.len());
        let mut w = Vec::with_capacity(seq.len());
        let mut l = Vec::with_capacity(seq.len());
        for &e in &seq {
            let (p, ww, ll) = vert(e);
            pts.push(p);
            w.push(ww);
            l.push(ll);
        }
        out.push(RawLine {
            pts,
            w,
            lv: l,
            closed,
        });
    };
    for &id in &ids {
        if visited.contains(&id) {
            continue;
        }
        if nbr[&id][1] == NO {
            let seq = walk(id, &nbr, &mut visited);
            emit(seq, false);
        }
    }
    for &id in &ids {
        if visited.contains(&id) {
            continue;
        }
        let seq = walk(id, &nbr, &mut visited);
        let last = *seq.last().unwrap();
        let closed = seq.len() > 2 && nbr[&last].contains(&seq[0]);
        emit(seq, closed);
    }
    out
}
