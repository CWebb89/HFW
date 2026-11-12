//! Overhang support detection, projection and fill.
//!
//! Supports are computed on the same rasterised layer masks the bridge pass
//! reads (`hfw_infill::MaskCache`), so the two features cannot disagree about
//! where air is. A cell seeds support when the layer below does not reach under
//! it by the horizontal step an overhang at `threshold_angle` makes in one
//! layer; the seeds are then projected downward into columns, kept
//! `xy_gap` clear of the part, offset `z_gap` below the overhang they support,
//! and filled with alternating line directions.
//!
//! See `docs/SUPPORTS.md` for the reference-slicer comparison and the staging.

#![forbid(unsafe_code)]

use hfw_infill::{fill_from_mask, MaskCache, SkinGrid};
use hfw_kernel::Region;
use hfw_walls::field::Grid;
use hfw_walls::{Kind, Line};

/// Shape of the support bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Columns of constant cross-section, projected straight down.
    Normal,
    /// Tips that lean into one another on the way down and merge into trunks.
    Tree,
}

/// One node of a tree support at one layer.
#[derive(Clone, Copy, Debug)]
struct Node {
    x: f64,
    y: f64,
    r: f64,
    depth: usize,
}

/// Support generation settings.
#[derive(Clone, Copy, Debug)]
pub struct SupportParams {
    /// Which body shape to generate.
    pub style: Style,
    /// Steepest overhang that prints without support, in degrees from vertical.
    pub threshold_angle: f64,
    /// Clearance between support and part, horizontally, in millimetres.
    pub xy_gap: f64,
    /// Clearance between the support top and the surface it holds, in mm.
    pub z_gap: f64,
    /// Dense layers immediately under the contact surface.
    pub interface_layers: usize,
    /// Line density of the support body.
    pub density: f64,
    /// Line density of the interface layers.
    pub interface_density: f64,
    /// Drop columns that would have to pass through the part.
    pub on_build_plate_only: bool,
    /// Smallest connected overhang patch worth supporting, in mm^2.
    pub min_area: f64,
    /// Bead width for support lines.
    pub width: f64,
    /// Steepest branch lean from vertical, in degrees.
    pub branch_angle: f64,
    /// Distance under which two branches pull on each other, in millimetres.
    pub branch_distance: f64,
    /// Diameter of the contact tip under the overhang, in millimetres.
    pub tip_diameter: f64,
    /// How fast a branch widens as it descends, in degrees of cone.
    pub branch_diameter_angle: f64,
    /// Largest branch diameter a trunk may reach, in millimetres.
    pub max_branch_diameter: f64,
}

impl SupportParams {
    /// Defaults for extrusion width `w`.
    pub fn new(w: f64) -> SupportParams {
        SupportParams {
            style: Style::Normal,
            threshold_angle: 60.0,
            xy_gap: 0.35,
            z_gap: 0.2,
            interface_layers: 2,
            density: 0.15,
            interface_density: 0.8,
            on_build_plate_only: false,
            min_area: 2.0,
            width: w,
            branch_angle: 40.0,
            branch_distance: 3.0,
            tip_diameter: 0.8,
            branch_diameter_angle: 2.0,
            max_branch_diameter: 8.0,
        }
    }

    fn validate(&self) -> bool {
        let unit = |v: f64| v.is_finite() && v >= 0.0;
        self.threshold_angle.is_finite()
            && self.threshold_angle > 0.0
            && self.threshold_angle < 90.0
            && unit(self.xy_gap)
            && unit(self.z_gap)
            && unit(self.min_area)
            && self.density.is_finite()
            && self.density > 0.0
            && self.density <= 1.0
            && self.interface_density.is_finite()
            && self.interface_density > 0.0
            && self.interface_density <= 1.0
            && self.width.is_finite()
            && self.width > 0.0
            && self.branch_angle.is_finite()
            && self.branch_angle > 0.0
            && self.branch_angle < 80.0
            && self.branch_distance.is_finite()
            && self.branch_distance > 0.0
            && self.tip_diameter.is_finite()
            && self.tip_diameter > 0.0
            && self.branch_diameter_angle.is_finite()
            && self.branch_diameter_angle >= 0.0
            && self.branch_diameter_angle < 45.0
            && self.max_branch_diameter.is_finite()
            && self.max_branch_diameter >= self.tip_diameter
    }
}

/// Per-layer support bodies, ready to fill.
#[derive(Clone, Debug)]
pub struct Supports {
    grid: SkinGrid,
    body: Body,
}

#[derive(Clone, Debug)]
enum Body {
    Empty,
    Columns {
        body: Vec<Vec<u64>>,
        interface: Vec<Vec<u64>>,
    },
    Tree(Tree),
}

/// Node positions per layer, already solved downward.
#[derive(Clone, Debug)]
struct Tree {
    layers: Vec<Vec<Node>>,
}

impl Supports {
    /// Supports for every layer, or none when disabled.
    pub fn build(
        regions: &[Region],
        cache: &mut MaskCache,
        p: &SupportParams,
        height: f64,
    ) -> Supports {
        let grid = *cache.grid();
        if !p.validate() || regions.is_empty() || !(height.is_finite() && height > 0.0) {
            return Supports {
                grid,
                body: Body::Empty,
            };
        }
        let seeds = seed_masks(regions, cache, p, height);
        let body = match p.style {
            Style::Normal => columns(regions, cache, p, &seeds),
            Style::Tree => Body::Tree(build_tree(cache, p, height, &seeds)),
        };
        Supports { grid, body }
    }

    /// Layer count, one entry per sliced layer.
    pub fn layers(&self) -> usize {
        match &self.body {
            Body::Empty => 0,
            Body::Columns { body, .. } => body.len(),
            Body::Tree(tree) => tree.layers.len(),
        }
    }

    /// Filled support paths for layer `k`.
    pub fn fill(&self, k: usize, p: &SupportParams) -> Vec<Line> {
        match &self.body {
            Body::Empty => Vec::new(),
            Body::Columns { body, interface } => fill_columns(&self.grid, body, interface, k, p),
            Body::Tree(tree) => fill_tree(&self.grid, tree, k, p),
        }
    }
}

/// Overhang seeds, shifted `z_gap` below the surface they hold up.
fn seed_masks(
    regions: &[Region],
    cache: &mut MaskCache,
    p: &SupportParams,
    height: f64,
) -> Vec<Vec<u64>> {
    let grid = *cache.grid();
    let words = cache.words();
    let n = regions.len();
    let reach = height * p.threshold_angle.to_radians().tan();
    let mut seeds = Vec::with_capacity(n);
    for k in 0..n {
        let own = cache.own_bits(k);
        let below = cache.below_bits(k);
        let spread = dilate(&below, &grid.grid, reach, words);
        let mut seed: Vec<u64> = own.iter().zip(&spread).map(|(o, s)| o & !s).collect();
        if k == 0 {
            seed.iter_mut().for_each(|w| *w = 0);
        }
        drop_small(&mut seed, &grid.grid, p.min_area);
        seeds.push(seed);
    }
    let z_layers = (p.z_gap / height).round() as usize + 1;
    let mut lowered = vec![vec![0u64; words]; n];
    for k in 0..n {
        if k + z_layers < n {
            lowered[k] = seeds[k + z_layers].clone();
        }
    }
    lowered
}

/// Straight-down columns with dense interface layers.
fn columns(
    regions: &[Region],
    cache: &mut MaskCache,
    p: &SupportParams,
    seeds: &[Vec<u64>],
) -> Body {
    let grid = *cache.grid();
    let words = cache.words();
    let n = regions.len();
    let zero = vec![0u64; words];
    let clearance = p.xy_gap + 0.5 * p.width;
    let mut body = vec![vec![0u64; words]; n];
    let mut run = vec![0u64; words];
    for k in (0..n).rev() {
        run.iter_mut().zip(&seeds[k]).for_each(|(r, s)| *r |= *s);
        let own = cache.own_bits(k);
        let blocked = dilate(&own, &grid.grid, clearance, words);
        let layer: Vec<u64> = run.iter().zip(&blocked).map(|(r, b)| r & !b).collect();
        run.clone_from(&layer);
        body[k] = layer;
    }

    if p.on_build_plate_only {
        let mut rooted = body[0].clone();
        for layer in body.iter_mut().skip(1) {
            rooted.iter_mut().zip(&*layer).for_each(|(r, b)| *r &= *b);
            layer.iter_mut().zip(&rooted).for_each(|(b, r)| *b &= *r);
        }
    }

    let mut interface = vec![vec![0u64; words]; n];
    let mut top: Vec<Vec<u64>> = Vec::with_capacity(n);
    for (k, layer) in body.iter().enumerate() {
        let above = if k + 1 < n { &body[k + 1] } else { &zero };
        top.push(layer.iter().zip(above).map(|(b, a)| b & !a).collect());
    }
    for (k, slot) in interface.iter_mut().enumerate() {
        for t in top.iter().skip(k).take(p.interface_layers.max(1)) {
            slot.iter_mut().zip(t).for_each(|(c, s)| *c |= *s);
        }
    }

    Body::Columns { body, interface }
}

/// Straight-down fill of the column masks.
fn fill_columns(
    grid: &SkinGrid,
    body: &[Vec<u64>],
    interface: &[Vec<u64>],
    k: usize,
    p: &SupportParams,
) -> Vec<Line> {
    let (Some(body), Some(interface)) = (body.get(k), interface.get(k)) else {
        return Vec::new();
    };
    let step = p.width / 4.0;
    let angle = if k % 2 == 0 { 45.0 } else { 135.0 };
    let mut out = Vec::new();
    if interface.iter().any(|w| *w != 0) {
        out.extend(fill_from_mask(
            grid,
            interface,
            p.width,
            p.width / p.interface_density,
            angle,
            Kind::SupportInterface,
            step,
        ));
    }
    if body.iter().any(|w| *w != 0) {
        let mut solid = body.clone();
        for (c, i) in solid.iter_mut().zip(interface) {
            *c &= !*i;
        }
        out.extend(fill_from_mask(
            grid,
            &solid,
            p.width,
            p.width / p.density,
            angle,
            Kind::Support,
            step,
        ));
    }
    out
}

/// Grows tips down from the overhang, letting them lean and merge into trunks.
///
/// The descent solves one constraint per layer: no node may move more than
/// `height * tan(branch_angle)` sideways, so a branch is always printable
/// without support under itself. Contacts are sampled from the z-shifted seeds,
/// so a tree grows in the same clearance envelope a column does.
fn build_tree(cache: &mut MaskCache, p: &SupportParams, height: f64, seeds: &[Vec<u64>]) -> Tree {
    let grid = *cache.grid();
    let g = grid.grid;
    let words = cache.words();
    let n = seeds.len();
    let clearance = p.xy_gap + 0.5 * p.width;
    let tip_r = 0.5 * p.tip_diameter.max(p.width);
    let max_r = 0.5 * p.max_branch_diameter.max(p.tip_diameter);
    let grow = height * p.branch_diameter_angle.to_radians().tan();
    let step = height * p.branch_angle.to_radians().tan();
    let mut layers = vec![Vec::new(); n];
    let mut active: Vec<Node> = Vec::new();
    for k in (0..n).rev() {
        let own = cache.own_bits(k);
        let blocked = dilate(&own, &g, clearance, words);
        let mut next: Vec<Node> = Vec::with_capacity(active.len() + 8);
        for node in &active {
            let grown = Node {
                r: (node.r + grow).min(max_r),
                depth: node.depth + 1,
                ..*node
            };
            if let Some(placed) = place(grown, &blocked, &g, step) {
                next.push(placed);
            }
        }
        let mut heads = Vec::new();
        let spacing = p.branch_distance.max(2.0 * p.tip_diameter);
        for c in contact_nodes(&seeds[k], &grid, tip_r, spacing) {
            if let Some(placed) = place(c, &blocked, &g, step) {
                heads.push(placed);
            }
        }
        next.append(&mut heads);
        merge_nodes(&mut next, max_r);
        attract(&mut next, &blocked, &g, p.branch_distance, step);
        layers[k] = next.clone();
        active = next;
    }
    Tree { layers }
}

/// Moves a node to its place on the layer below, deflecting around the part.
fn place(node: Node, blocked: &[u64], g: &Grid, step: f64) -> Option<Node> {
    if free(blocked, g, node.x, node.y, node.r) {
        return Some(node);
    }
    for i in 1..8 {
        let a = i as f64 * std::f64::consts::FRAC_PI_4;
        for scale in [1.0, 2.0] {
            let x = node.x + a.cos() * step * scale;
            let y = node.y + a.sin() * step * scale;
            if free(blocked, g, x, y, node.r) {
                return Some(Node { x, y, ..node });
            }
        }
    }
    None
}

/// Pulls branches toward their nearest neighbour, one printable step at a time.
///
/// The neighbour search is unbounded on purpose: `branch_distance` decides when
/// two branches have to start converging, but a trunk that has drifted apart
/// from its neighbours still has to close the gap, and it can only do that while
/// it descends. Fusion itself needs contact, so the lean stays angle-limited.
fn attract(nodes: &mut [Node], blocked: &[u64], g: &Grid, distance: f64, step: f64) {
    for i in 0..nodes.len() {
        let mut best = None;
        let mut best_d = f64::INFINITY;
        for (j, other) in nodes.iter().enumerate() {
            if i == j {
                continue;
            }
            let d = ((nodes[i].x - other.x).powi(2) + (nodes[i].y - other.y).powi(2)).sqrt();
            if d < best_d {
                best_d = d;
                best = Some(j);
            }
        }
        let Some(j) = best else { continue };
        if best_d <= 1e-9 {
            continue;
        }
        let reach = if best_d <= distance {
            step
        } else {
            0.25 * step
        };
        let travel = (0.5 * best_d).min(reach);
        let x = nodes[i].x + (nodes[j].x - nodes[i].x) / best_d * travel;
        let y = nodes[i].y + (nodes[j].y - nodes[i].y) / best_d * travel;
        if free(blocked, g, x, y, nodes[i].r) {
            nodes[i].x = x;
            nodes[i].y = y;
        }
    }
}

/// Fuses branches whose cross-sections have collided, conserving their area.
///
/// Fusion is decided by contact, not by distance: once two beads touch, the
/// merged disc is carried by the two discs below it, so the centre may shift
/// further than one layer's lean without printing over air.
fn merge_nodes(nodes: &mut Vec<Node>, max_r: f64) {
    loop {
        let mut pair = None;
        let mut best_d = f64::INFINITY;
        for i in 0..nodes.len() {
            for j in i + 1..nodes.len() {
                let d =
                    ((nodes[i].x - nodes[j].x).powi(2) + (nodes[i].y - nodes[j].y).powi(2)).sqrt();
                if d < nodes[i].r + nodes[j].r && d < best_d {
                    best_d = d;
                    pair = Some((i, j));
                }
            }
        }
        let Some((i, j)) = pair else { return };
        let (a, b) = (nodes[i], nodes[j]);
        let (wa, wb) = (a.r * a.r, b.r * b.r);
        let merged = Node {
            x: (a.x * wa + b.x * wb) / (wa + wb),
            y: (a.y * wa + b.y * wb) / (wa + wb),
            r: ((wa + wb).sqrt()).min(max_r),
            depth: a.depth.min(b.depth),
        };
        nodes.remove(j);
        nodes[i] = merged;
    }
}

/// Contact tips sampled across each supported patch.
fn contact_nodes(mask: &[u64], grid: &SkinGrid, tip_r: f64, spacing: f64) -> Vec<Node> {
    let g = grid.grid;
    let cells = ((spacing / g.h).round() as usize).max(1);
    let mut out = Vec::new();
    let mut seen = vec![false; g.nx * g.ny];
    for iy in 0..g.ny {
        for ix in 0..g.nx {
            let id = iy * g.nx + ix;
            if seen[id] || !get(mask, &g, ix as i64, iy as i64) {
                continue;
            }
            let mut stack = vec![(ix, iy)];
            seen[id] = true;
            let mut members = Vec::new();
            while let Some((cx, cy)) = stack.pop() {
                members.push((cx, cy));
                for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    let (ux, uy) = (cx as i64 + dx, cy as i64 + dy);
                    if ux < 0 || uy < 0 || ux >= g.nx as i64 || uy >= g.ny as i64 {
                        continue;
                    }
                    let (ux, uy) = (ux as usize, uy as usize);
                    let uid = uy * g.nx + ux;
                    if seen[uid] || !get(mask, &g, ux as i64, uy as i64) {
                        continue;
                    }
                    seen[uid] = true;
                    stack.push((ux, uy));
                }
            }
            let (x0, y0) = *members.iter().min().unwrap();
            for (cx, cy) in members {
                if (cx - x0) % cells != 0 || (cy - y0) % cells != 0 {
                    continue;
                }
                out.push(Node {
                    x: g.origin.x + (cx as f64 + 0.5) * g.h,
                    y: g.origin.y + (cy as f64 + 0.5) * g.h,
                    r: tip_r,
                    depth: 0,
                });
            }
        }
    }
    out
}

/// True when a disc of radius `r` clears every blocked cell.
fn free(blocked: &[u64], g: &Grid, x: f64, y: f64, r: f64) -> bool {
    let ix0 = (((x - r - g.origin.x) / g.h).floor().max(0.0)) as usize;
    let iy0 = (((y - r - g.origin.y) / g.h).floor().max(0.0)) as usize;
    let ix1 = (((x + r - g.origin.x) / g.h).ceil().max(0.0) as usize).min(g.nx);
    let iy1 = (((y + r - g.origin.y) / g.h).ceil().max(0.0) as usize).min(g.ny);
    for iy in iy0..iy1 {
        for ix in ix0..ix1 {
            let cx = g.origin.x + (ix as f64 + 0.5) * g.h;
            let cy = g.origin.y + (iy as f64 + 0.5) * g.h;
            if (cx - x).powi(2) + (cy - y).powi(2) <= r * r && get(blocked, g, ix as i64, iy as i64)
            {
                return false;
            }
        }
    }
    true
}

/// Sets every cell inside a disc.
fn stamp_disk(mask: &mut [u64], g: &Grid, x: f64, y: f64, r: f64) {
    let ix0 = (((x - r - g.origin.x) / g.h).floor().max(0.0)) as usize;
    let iy0 = (((y - r - g.origin.y) / g.h).floor().max(0.0)) as usize;
    let ix1 = (((x + r - g.origin.x) / g.h).ceil().max(0.0) as usize).min(g.nx);
    let iy1 = (((y + r - g.origin.y) / g.h).ceil().max(0.0) as usize).min(g.ny);
    for iy in iy0..iy1 {
        for ix in ix0..ix1 {
            let cx = g.origin.x + (ix as f64 + 0.5) * g.h;
            let cy = g.origin.y + (iy as f64 + 0.5) * g.h;
            if (cx - x).powi(2) + (cy - y).powi(2) <= r * r {
                set(mask, g, ix as i64, iy as i64);
            }
        }
    }
}

/// One closed bead tracing the branch outline.
fn ring(node: &Node, width: f64, kind: Kind) -> Line {
    let r = (node.r - 0.5 * width).max(0.5 * width);
    let segments = ((2.0 * std::f64::consts::PI * r / 1.2).ceil() as usize).clamp(8, 64);
    let pts = (0..segments)
        .map(|i| {
            let a = 2.0 * std::f64::consts::PI * i as f64 / segments as f64;
            hfw_kernel::V2::new(node.x + r * a.cos(), node.y + r * a.sin())
        })
        .collect();
    Line {
        pts,
        width: vec![width; segments],
        closed: true,
        kind,
    }
}

/// Wall plus interior fill for every branch on the layer.
fn fill_tree(grid: &SkinGrid, tree: &Tree, k: usize, p: &SupportParams) -> Vec<Line> {
    let Some(nodes) = tree.layers.get(k) else {
        return Vec::new();
    };
    if nodes.is_empty() {
        return Vec::new();
    }
    let g = grid.grid;
    let words = (g.nx * g.ny).div_ceil(64);
    let mut sparse = vec![0u64; words];
    let mut dense = vec![0u64; words];
    let mut out = Vec::new();
    for node in nodes {
        let kind = if node.depth < p.interface_layers {
            Kind::SupportInterface
        } else {
            Kind::Support
        };
        out.push(ring(node, p.width, kind));
        let inner = node.r - p.width;
        if inner > 0.5 * p.width {
            match kind {
                Kind::SupportInterface => stamp_disk(&mut dense, &g, node.x, node.y, inner),
                _ => stamp_disk(&mut sparse, &g, node.x, node.y, inner),
            }
        }
    }
    let angle = if k % 2 == 0 { 45.0 } else { 135.0 };
    let step = p.width / 4.0;
    out.extend(fill_from_mask(
        grid,
        &dense,
        p.width,
        p.width / p.interface_density,
        angle,
        Kind::SupportInterface,
        step,
    ));
    out.extend(fill_from_mask(
        grid,
        &sparse,
        p.width,
        p.width / p.density,
        angle,
        Kind::Support,
        step,
    ));
    out
}

/// Cells within `reach` of a set cell, including the set cells themselves.
fn dilate(mask: &[u64], g: &Grid, reach: f64, words: usize) -> Vec<u64> {
    let mut out = mask.to_vec();
    if !(reach.is_finite() && reach > 0.0) {
        return out;
    }
    let cells = (reach / g.h).ceil() as i64;
    let mut next = vec![0u64; words];
    for _ in 0..cells {
        next.iter_mut().for_each(|w| *w = 0);
        for iy in 0..g.ny {
            for ix in 0..g.nx {
                if !get(&out, g, ix as i64, iy as i64) {
                    continue;
                }
                for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    set(&mut next, g, ix as i64 + dx, iy as i64 + dy);
                }
            }
        }
        out.iter_mut().zip(&next).for_each(|(o, n)| *o |= *n);
    }
    out
}

/// Removes connected components smaller than `min_area` square millimetres.
fn drop_small(mask: &mut [u64], g: &Grid, min_area: f64) {
    if !(min_area.is_finite() && min_area > 0.0) {
        return;
    }
    let min_cells = (min_area / (g.h * g.h)).ceil() as usize;
    let mut seen = vec![false; g.nx * g.ny];
    let mut drop: Vec<usize> = Vec::new();
    for iy in 0..g.ny {
        for ix in 0..g.nx {
            let id = iy * g.nx + ix;
            if seen[id] || !get(mask, g, ix as i64, iy as i64) {
                continue;
            }
            seen[id] = true;
            let mut stack = vec![(ix, iy)];
            let mut component = vec![id];
            while let Some((cx, cy)) = stack.pop() {
                for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    let (ux, uy) = (cx as i64 + dx, cy as i64 + dy);
                    if ux < 0 || uy < 0 || ux >= g.nx as i64 || uy >= g.ny as i64 {
                        continue;
                    }
                    let (ux, uy) = (ux as usize, uy as usize);
                    let uid = uy * g.nx + ux;
                    if seen[uid] || !get(mask, g, ux as i64, uy as i64) {
                        continue;
                    }
                    seen[uid] = true;
                    stack.push((ux, uy));
                    component.push(uid);
                }
            }
            if component.len() < min_cells {
                drop.extend(component);
            }
        }
    }
    for id in drop {
        mask[id / 64] &= !(1u64 << (id % 64));
    }
}

fn get(mask: &[u64], g: &Grid, ix: i64, iy: i64) -> bool {
    if ix < 0 || iy < 0 || ix >= g.nx as i64 || iy >= g.ny as i64 {
        return false;
    }
    let id = iy as usize * g.nx + ix as usize;
    (mask[id / 64] >> (id % 64)) & 1 == 1
}

fn set(mask: &mut [u64], g: &Grid, ix: i64, iy: i64) {
    if ix < 0 || iy < 0 || ix >= g.nx as i64 || iy >= g.ny as i64 {
        return;
    }
    let id = iy as usize * g.nx + ix as usize;
    mask[id / 64] |= 1u64 << (id % 64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use hfw_kernel::{Region, V2};

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Region {
        Region::new(vec![vec![
            V2::new(x0, y0),
            V2::new(x1, y0),
            V2::new(x1, y1),
            V2::new(x0, y1),
        ]])
    }

    fn post_and_table(legs: usize, plate: usize, leg_w: f64, span: f64) -> Vec<Region> {
        let mut regions: Vec<Region> = (0..legs).map(|_| square(0.0, 0.0, leg_w, 20.0)).collect();
        regions.extend((0..plate).map(|_| square(0.0, 0.0, span, 20.0)));
        regions
    }

    fn cell(m: &[u64], grid: &SkinGrid, x: f64, y: f64) -> bool {
        grid.is_set(m, V2::new(x, y))
    }

    fn columns(m: &Supports) -> (&Vec<Vec<u64>>, &Vec<Vec<u64>>) {
        match &m.body {
            Body::Columns { body, interface } => (body, interface),
            other => panic!("expected column supports, got {other:?}"),
        }
    }

    fn tree(m: &Supports) -> &Tree {
        match &m.body {
            Body::Tree(t) => t,
            other => panic!("expected a tree, got {other:?}"),
        }
    }

    #[test]
    fn a_cantilever_seeds_support_under_its_overhang_only() {
        let regions = post_and_table(20, 10, 4.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let p = SupportParams::new(0.4);
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let grid = *cache.grid();
        let overhang = 20;
        assert!(
            cell(&columns(&m).0[overhang - 2], &grid, 20.0, 10.0),
            "the far end of the plate has nothing under it and needs support"
        );
        assert!(
            !cell(&columns(&m).0[overhang - 2], &grid, 2.0, 10.0),
            "the post is solid, nothing to support there"
        );
        assert!(
            cell(&columns(&m).0[0], &grid, 20.0, 10.0),
            "support reaches the bed under the cantilever"
        );
    }

    #[test]
    fn support_keeps_its_distance_from_the_part() {
        let regions = post_and_table(20, 10, 8.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let p = SupportParams::new(0.4);
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let grid = *cache.grid();
        for k in 0..m.layers() {
            assert!(
                !cell(&columns(&m).0[k], &grid, 4.0, 10.0),
                "layer {k} put support inside the post"
            );
        }
    }

    #[test]
    fn the_contact_layer_is_left_empty_by_the_z_gap() {
        let regions = post_and_table(20, 10, 4.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let p = SupportParams::new(0.4);
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let grid = *cache.grid();
        let overhang = 20;
        assert!(
            !cell(&columns(&m).0[overhang], &grid, 20.0, 10.0),
            "support must stop one layer below the overhang"
        );
        assert!(
            cell(&columns(&m).1[overhang - 2], &grid, 20.0, 10.0),
            "the layer under the contact is the interface"
        );
    }

    #[test]
    fn shallow_overhangs_are_not_supported() {
        let mut regions: Vec<Region> = (0..30)
            .map(|i| {
                let out = 0.2 * (i as f64) * 45.0f64.to_radians().tan();
                square(0.0, 0.0, 10.0 + out, 20.0)
            })
            .collect();
        regions.push(square(0.0, 0.0, 30.0, 20.0));
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let p = SupportParams::new(0.4);
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let grid = *cache.grid();
        for k in 10..30 {
            assert!(
                !cell(&columns(&m).0[k], &grid, 5.0, 10.0),
                "a 45 degree wall is printable, layer {k} must not be supported"
            );
        }
    }

    #[test]
    fn a_fine_grid_coupon_still_seeds_support() {
        let regions = post_and_table(12, 4, 8.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.1, 12);
        let p = SupportParams::new(0.4);
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let grid = *cache.grid();
        assert!(
            cell(&columns(&m).0[10], &grid, 20.0, 10.0),
            "the cantilever needs support at the layer under it"
        );
        assert!(
            cell(&columns(&m).1[10], &grid, 20.0, 10.0),
            "the layer under the plate is the interface"
        );
    }

    #[test]
    fn a_tree_merges_its_tips_into_a_trunk_toward_the_bed() {
        let regions = post_and_table(60, 10, 8.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let mut p = SupportParams::new(0.4);
        p.style = Style::Tree;
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let t = tree(&m);
        let tips = t.layers[57].len();
        let trunk = t.layers[0].len();
        assert!(tips > 8, "the plate should hang from many tips, got {tips}");
        assert!(
            trunk * 4 <= tips,
            "{trunk} trunks from {tips} tips is not merging"
        );
        let thickest = t.layers[0].iter().map(|n| n.r).fold(0.0f64, f64::max);
        assert!(
            thickest > 2.0 * t.layers[57][0].r,
            "a merged trunk has to be thicker than a tip"
        );
    }

    #[test]
    fn a_tree_never_grows_through_the_part() {
        let regions = post_and_table(20, 10, 8.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let mut p = SupportParams::new(0.4);
        p.style = Style::Tree;
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let grid = *cache.grid();
        let t = tree(&m);
        for (k, nodes) in t.layers.iter().enumerate() {
            let own = cache.own_bits(k);
            for node in nodes {
                assert!(
                    free(&own, &grid.grid, node.x, node.y, node.r),
                    "a branch at layer {k} is inside the part at ({:.2}, {:.2})",
                    node.x,
                    node.y
                );
            }
        }
    }

    #[test]
    fn a_tree_leans_no_further_than_the_branch_angle_allows() {
        let regions = post_and_table(20, 10, 8.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let mut p = SupportParams::new(0.4);
        p.style = Style::Tree;
        p.branch_distance = 5.0;
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let t = tree(&m);
        let step = 0.2 * p.branch_angle.to_radians().tan();
        for k in 1..t.layers.len() {
            for parent in &t.layers[k] {
                let mut carried = false;
                for child in &t.layers[k - 1] {
                    let d = ((parent.x - child.x).powi(2) + (parent.y - child.y).powi(2)).sqrt();
                    if d <= child.r + step {
                        carried = true;
                    }
                }
                assert!(
                    carried,
                    "a branch at layer {k} has nothing under it to print on"
                );
            }
        }
    }

    #[test]
    fn a_tree_tip_prints_dense_over_a_sparse_trunk() {
        let regions = post_and_table(20, 10, 8.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let mut p = SupportParams::new(0.4);
        p.style = Style::Tree;
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let tip = m.fill(17, &p);
        assert!(!tip.is_empty(), "the contact layer printed nothing");
        assert!(
            tip.iter().all(|l| l.kind == Kind::SupportInterface),
            "the tip under the overhang has to be the dense interface"
        );
        assert!(
            tip.iter().any(|l| l.closed),
            "a branch is printed as a closed ring"
        );
        let trunk = m.fill(4, &p);
        assert!(
            trunk.iter().any(|l| l.kind == Kind::Support),
            "the trunk below the tip is plain support"
        );
    }

    #[test]
    fn tree_supports_stay_under_the_plate() {
        let regions = post_and_table(20, 10, 4.0, 30.0);
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let mut p = SupportParams::new(0.4);
        p.style = Style::Tree;
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let t = tree(&m);
        for nodes in &t.layers {
            for node in nodes {
                assert!(
                    (4.0..=30.0).contains(&node.x) && (0.0..=20.0).contains(&node.y),
                    "a branch wandered to ({:.2}, {:.2}), outside the plate",
                    node.x,
                    node.y
                );
            }
        }
    }

    #[test]
    fn build_plate_only_drops_a_column_that_would_rest_on_the_part() {
        let mut regions: Vec<Region> = (0..10).map(|_| square(0.0, 0.0, 30.0, 20.0)).collect();
        regions.extend((0..10).map(|_| square(0.0, 0.0, 8.0, 20.0)));
        regions.extend((0..10).map(|_| square(0.0, 0.0, 30.0, 20.0)));
        let mut cache = MaskCache::new(&regions, 0.2, 12);
        let mut p = SupportParams::new(0.4);
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        let grid = *cache.grid();
        assert!(
            cell(&columns(&m).0[15], &grid, 20.0, 10.0),
            "support may stand on the part by default"
        );
        assert!(
            !cell(&columns(&m).0[9], &grid, 20.0, 10.0),
            "the column stops at the block below it"
        );
        p.on_build_plate_only = true;
        let m = Supports::build(&regions, &mut cache, &p, 0.2);
        assert!(
            !cell(&columns(&m).0[15], &grid, 20.0, 10.0),
            "a bed-only column cannot float over the part"
        );
    }
}
