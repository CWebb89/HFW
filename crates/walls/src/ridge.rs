//! Medial-axis (ridge) extraction and per-vertex bead assignment.
//!
//! Ridge vertices are found where the boundary-normal directions of two
//! neighbouring field cells oppose each other, then linked across cells into
//! branches. Each branch vertex gets a bead decomposition from its local
//! inscribed radius; blending and hysteresis keep neighbouring layers and
//! neighbouring arc positions consistent.

use std::collections::HashSet;

use hfw_kernel::{seg_closest, Bounds, PointIndex, SegIndex, V2};

use crate::field::Field;
use crate::params::{Beading, Params};

/// A linked set of ridge vertices.
pub struct RidgeGraph {
    /// Vertex positions.
    pub pos: Vec<V2>,
    /// Inscribed radius at each vertex.
    pub r: Vec<f64>,
    /// Adjacency lists.
    pub adj: Vec<Vec<u32>>,
}

/// One open or closed chain of ridge vertices.
pub struct Branch {
    /// Vertex indices along the branch.
    pub ids: Vec<u32>,
    /// True when the branch returns to its first vertex.
    pub closed: bool,
}

/// Extracted medial axis with per-vertex beading.
pub struct Ridge {
    /// Vertex graph.
    pub graph: RidgeGraph,
    /// Branches of that graph.
    pub branches: Vec<Branch>,
    /// Smoothed inscribed radius per vertex.
    pub rs: Vec<f64>,
    /// Bead decomposition per vertex; `None` when the feature is too narrow.
    pub bd: Vec<Option<Beading>>,
    /// Spatial index over vertex positions.
    pub points: PointIndex,
    /// Largest smoothed radius.
    pub rmax: f64,
}

fn link(adj: &mut [Vec<u32>], a: usize, b: usize) {
    if a == b {
        return;
    }
    if !adj[a].contains(&(b as u32)) {
        adj[a].push(b as u32);
    }
    if !adj[b].contains(&(a as u32)) {
        adj[b].push(a as u32);
    }
}

fn crossing(
    f: &Field,
    index: &SegIndex,
    a: usize,
    b: usize,
    cos_c: f64,
    d_min: f64,
    cap: f64,
) -> Option<(V2, f64)> {
    if !(f.inside[a] && f.inside[b]) {
        return None;
    }
    let (da, db) = (f.d[a], f.d[b]);
    if !da.is_finite() || !db.is_finite() || da < d_min || db < d_min {
        return None;
    }
    let (sa, sb) = (f.seg[a], f.seg[b]);
    if sa == sb {
        return None;
    }
    let pa = f.grid.pos_id(a);
    let pb = f.grid.pos_id(b);
    let ua = (pa - f.foot[a]).norm();
    let ub = (pb - f.foot[b]).norm();
    if ua.dot(ub) >= cos_c {
        return None;
    }
    if f.foot[a].dist(f.foot[b]) < 0.5 * f.grid.h {
        return None;
    }
    let (a0, a1) = index.seg(sa);
    let (b0, b1) = index.seg(sb);
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let m = 0.5 * (lo + hi);
        let p = pa.lerp(pb, m);
        let fa = seg_closest(p, a0, a1).0;
        let fb = seg_closest(p, b0, b1).0;
        if fa <= fb {
            lo = m;
        } else {
            hi = m;
        }
    }
    let c = pa.lerp(pb, 0.5 * (lo + hi));
    let r = match index.nearest(c, cap + f.grid.h) {
        Some(h) => h.d,
        None => 0.5 * (da + db),
    };
    Some((c, r))
}

/// Builds the raw ridge graph from field normal directions.
pub fn extract(f: &Field, index: &SegIndex, p: &Params) -> RidgeGraph {
    let g = &f.grid;
    let n = g.len();
    let cos_c = p.central_deg.to_radians().cos();
    let d_min = 0.5 * g.h;
    let cap = p.cap();
    let mut pos: Vec<V2> = Vec::new();
    let mut rad: Vec<f64> = Vec::new();
    let mut hid = vec![-1i32; n];
    let mut vid = vec![-1i32; n];
    for iy in 0..g.ny {
        for ix in 0..g.nx - 1 {
            let a = g.id(ix, iy);
            if let Some((c, r)) = crossing(f, index, a, a + 1, cos_c, d_min, cap) {
                hid[a] = pos.len() as i32;
                pos.push(c);
                rad.push(r);
            }
        }
    }
    for iy in 0..g.ny - 1 {
        for ix in 0..g.nx {
            let a = g.id(ix, iy);
            if let Some((c, r)) = crossing(f, index, a, a + g.nx, cos_c, d_min, cap) {
                vid[a] = pos.len() as i32;
                pos.push(c);
                rad.push(r);
            }
        }
    }
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); pos.len()];
    for iy in 0..g.ny - 1 {
        for ix in 0..g.nx - 1 {
            let a = g.id(ix, iy);
            let es = [hid[a], hid[a + g.nx], vid[a], vid[a + 1]];
            let ids: Vec<usize> = es
                .iter()
                .filter(|&&e| e >= 0)
                .map(|&e| e as usize)
                .collect();
            if ids.len() == 2 {
                link(&mut adj, ids[0], ids[1]);
            } else if ids.len() >= 3 {
                let mut c = V2::ZERO;
                for &i in &ids {
                    c = c + pos[i];
                }
                c = c * (1.0 / ids.len() as f64);
                let r = match index.nearest(c, cap + g.h) {
                    Some(h) => h.d,
                    None => ids.iter().map(|&i| rad[i]).sum::<f64>() / ids.len() as f64,
                };
                let j = pos.len();
                pos.push(c);
                rad.push(r);
                adj.push(Vec::new());
                for &i in &ids {
                    link(&mut adj, i, j);
                }
            }
        }
    }
    RidgeGraph { pos, r: rad, adj }
}

fn remove_vertex(adj: &mut [Vec<u32>], alive: &mut [bool], v: usize) {
    let nbrs = std::mem::take(&mut adj[v]);
    for nb in nbrs {
        adj[nb as usize].retain(|&x| x as usize != v);
    }
    alive[v] = false;
}

/// Removes short spurs until the graph is stable.
pub fn prune_spurs(g: &mut RidgeGraph, spur: f64, min_r: f64) {
    let n = g.pos.len();
    let mut alive = vec![true; n];
    loop {
        let mut changed = false;
        for s in 0..n {
            if !alive[s] || g.adj[s].len() != 1 {
                continue;
            }
            let mut chain = vec![s];
            let mut len = 0.0;
            let mut prev = s;
            let mut cur = g.adj[s][0] as usize;
            let mut guard = 0usize;
            let junction;
            loop {
                len += g.pos[prev].dist(g.pos[cur]);
                let deg = g.adj[cur].len();
                if deg == 2 && guard <= n {
                    chain.push(cur);
                    let a = &g.adj[cur];
                    let nxt = if a[0] as usize == prev { a[1] } else { a[0] } as usize;
                    prev = cur;
                    cur = nxt;
                    guard += 1;
                    continue;
                }
                junction = deg >= 3;
                break;
            }
            let max_r = chain
                .iter()
                .chain(std::iter::once(&cur))
                .map(|&v| g.r[v])
                .fold(0.0, f64::max);
            if len < spur && (junction || max_r < min_r) {
                for &v in &chain {
                    remove_vertex(&mut g.adj, &mut alive, v);
                }
                if !junction {
                    remove_vertex(&mut g.adj, &mut alive, cur);
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut map = vec![u32::MAX; n];
    let mut pos = Vec::new();
    let mut r = Vec::new();
    for (v, keep) in alive.iter().enumerate() {
        if *keep {
            map[v] = pos.len() as u32;
            pos.push(g.pos[v]);
            r.push(g.r[v]);
        }
    }
    let mut adj: Vec<Vec<u32>> = Vec::with_capacity(pos.len());
    for (v, keep) in alive.iter().enumerate() {
        if *keep {
            adj.push(g.adj[v].iter().map(|&x| map[x as usize]).collect());
        }
    }
    g.pos = pos;
    g.r = r;
    g.adj = adj;
}

/// Splits the graph into open and closed branches.
pub fn branches(g: &RidgeGraph) -> Vec<Branch> {
    let n = g.pos.len();
    let mut used: HashSet<(u32, u32)> = HashSet::new();
    let key = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
    let mut out = Vec::new();
    for s in 0..n {
        if g.adj[s].len() == 2 {
            continue;
        }
        for &nb in &g.adj[s] {
            if used.contains(&key(s as u32, nb)) {
                continue;
            }
            let mut ids = vec![s as u32];
            let mut prev = s as u32;
            let mut cur = nb;
            used.insert(key(prev, cur));
            loop {
                ids.push(cur);
                let a = &g.adj[cur as usize];
                if a.len() != 2 {
                    break;
                }
                let nxt = if a[0] == prev { a[1] } else { a[0] };
                used.insert(key(cur, nxt));
                prev = cur;
                cur = nxt;
            }
            out.push(Branch { ids, closed: false });
        }
    }
    for s in 0..n {
        if g.adj[s].is_empty() {
            out.push(Branch {
                ids: vec![s as u32],
                closed: false,
            });
        }
    }
    for s in 0..n {
        if g.adj[s].len() != 2 {
            continue;
        }
        let first = g.adj[s][0];
        if used.contains(&key(s as u32, first)) {
            continue;
        }
        let mut ids = vec![s as u32];
        let mut prev = s as u32;
        let mut cur = first;
        used.insert(key(prev, cur));
        while cur as usize != s {
            ids.push(cur);
            let a = &g.adj[cur as usize];
            let nxt = if a[0] == prev { a[1] } else { a[0] };
            used.insert(key(cur, nxt));
            prev = cur;
            cur = nxt;
        }
        out.push(Branch { ids, closed: true });
    }
    out
}

fn smooth_branch(g: &RidgeGraph, b: &Branch, half: f64) -> Vec<f64> {
    let m = b.ids.len();
    let raw: Vec<f64> = b.ids.iter().map(|&i| g.r[i as usize]).collect();
    if half <= 0.0 || m < 3 {
        return raw;
    }
    let mut s = vec![0.0; m];
    for k in 1..m {
        s[k] = s[k - 1] + g.pos[b.ids[k - 1] as usize].dist(g.pos[b.ids[k] as usize]);
    }
    let total = if b.closed {
        s[m - 1] + g.pos[b.ids[m - 1] as usize].dist(g.pos[b.ids[0] as usize])
    } else {
        s[m - 1]
    };
    let arc = |i: usize, j: usize| -> f64 {
        let d = (s[i] - s[j]).abs();
        if b.closed {
            d.min(total - d)
        } else {
            d
        }
    };
    let mut out = raw.clone();
    let lo = if b.closed { 0 } else { 1 };
    let hi = if b.closed { m } else { m - 1 };
    for k in lo..hi {
        let mut sw = 1.0;
        let mut sv = raw[k];
        if b.closed {
            for o in 1..=m / 2 {
                let j = (k + o) % m;
                let d = arc(k, j);
                if d >= half {
                    break;
                }
                let w = 1.0 - d / half;
                sw += w;
                sv += w * raw[j];
                if !(m % 2 == 0 && o == m / 2) {
                    let j2 = (k + m - o) % m;
                    let d2 = arc(k, j2);
                    if d2 < half {
                        let w2 = 1.0 - d2 / half;
                        sw += w2;
                        sv += w2 * raw[j2];
                    }
                }
            }
        } else {
            for j in (0..k).rev() {
                let d = s[k] - s[j];
                if d >= half {
                    break;
                }
                let w = 1.0 - d / half;
                sw += w;
                sv += w * raw[j];
            }
            for j in k + 1..m {
                let d = s[j] - s[k];
                if d >= half {
                    break;
                }
                let w = 1.0 - d / half;
                sw += w;
                sv += w * raw[j];
            }
        }
        out[k] = sv / sw;
    }
    out
}

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn assign_beadings(g: &RidgeGraph, brs: &[Branch], rs: &[f64], p: &Params) -> Vec<Option<Beading>> {
    let nv = g.pos.len();
    let mut tr: Vec<Option<(usize, usize, f64)>> = vec![None; nv];
    let mut set = vec![false; nv];
    let nmax = 2 * p.walls;
    for b in brs {
        let m = b.ids.len();
        if m == 0 {
            continue;
        }
        let nu: Vec<f64> = b.ids.iter().map(|&i| 2.0 * rs[i as usize] / p.w).collect();
        let mut st = vec![0usize; m];
        st[0] = p.state_of(nu[0]);
        for k in 1..m {
            let prev = st[k - 1];
            if prev == 0 || nu[k] < p.nu_min() {
                st[k] = p.state_of(nu[k]);
                continue;
            }
            let mut c = prev;
            while c < nmax && nu[k] > c as f64 + 0.5 + p.hyst {
                c += 1;
            }
            while c > 1 && nu[k] < c as f64 - 0.5 - p.hyst {
                c -= 1;
            }
            st[k] = c;
        }
        let mut s = vec![0.0; m];
        for k in 1..m {
            s[k] = s[k - 1] + g.pos[b.ids[k - 1] as usize].dist(g.pos[b.ids[k] as usize]);
        }
        let mut local: Vec<(usize, usize, f64)> = (0..m).map(|k| (st[k], st[k], 0.0)).collect();
        for k in 0..m - 1 {
            if st[k] == st[k + 1] || st[k] == 0 || st[k + 1] == 0 {
                continue;
            }
            let sf = 0.5 * (s[k] + s[k + 1]);
            for v in 0..m {
                let d = s[v] - sf;
                if d.abs() < 0.5 * p.trans_len {
                    let lam = smoothstep(d / p.trans_len + 0.5);
                    local[v] = (st[k], st[k + 1], lam);
                }
            }
        }
        for (k, &id) in b.ids.iter().enumerate() {
            if !set[id as usize] {
                set[id as usize] = true;
                tr[id as usize] = Some(local[k]);
            }
        }
    }
    (0..nv)
        .map(|i| match tr[i] {
            Some((lo, hi, lam)) => p.compose(lo, hi, lam, rs[i]),
            None => p.compose(
                p.state_of(2.0 * rs[i] / p.w),
                p.state_of(2.0 * rs[i] / p.w),
                0.0,
                rs[i],
            ),
        })
        .collect()
}

/// Extracts, prunes, smooths and classifies the medial axis.
pub fn build(f: &Field, index: &SegIndex, p: &Params) -> Ridge {
    let mut graph = extract(f, index, p);
    prune_spurs(&mut graph, p.spur, 0.5 * p.min_feature);
    let brs = branches(&graph);
    let mut rs = graph.r.clone();
    for b in &brs {
        let sm = smooth_branch(&graph, b, p.smooth);
        for (k, &id) in b.ids.iter().enumerate() {
            let is_end = !b.closed && (k == 0 || k + 1 == b.ids.len());
            if !is_end {
                rs[id as usize] = sm[k];
            }
        }
    }
    let bd = assign_beadings(&graph, &brs, &rs, p);
    let rmax = rs.iter().cloned().fold(0.0, f64::max);
    let mut bb = Bounds::empty();
    for &q in &graph.pos {
        bb.add(q);
    }
    if graph.pos.is_empty() {
        bb.add(V2::ZERO);
    }
    let bounds = bb.grown(p.ext * rmax + 4.0 * p.w + 1.0);
    let points = PointIndex::new(graph.pos.clone(), bounds, 4.0 * f.grid.h);
    Ridge {
        graph,
        branches: brs,
        rs,
        bd,
        points,
        rmax,
    }
}
