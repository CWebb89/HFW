//! A uniform sampling grid and the inside test the brim and the infill masks need.
//!
//! Walls no longer sample distance on a grid: they offset polygons. What is left
//! here is the raster the adhesion brim and the infill mask cache still lay down,
//! which needs only a grid and a row-scan of the region's edges.

use hfw_kernel::{Bounds, Region, V2};

/// A uniform sampling grid in millimetres.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    /// World position of cell (0, 0).
    pub origin: V2,
    /// Cell size in millimetres.
    pub h: f64,
    /// Cell count along X.
    pub nx: usize,
    /// Cell count along Y.
    pub ny: usize,
}

impl Grid {
    /// Smallest grid covering `b` with two cells of margin.
    pub fn covering(b: Bounds, h: f64) -> Grid {
        let nx = (b.width() / h).ceil() as usize + 2;
        let ny = (b.height() / h).ceil() as usize + 2;
        Grid {
            origin: b.min,
            h,
            nx,
            ny,
        }
    }

    /// Total cell count.
    pub fn len(&self) -> usize {
        self.nx * self.ny
    }

    /// True when the grid has no cells.
    pub fn is_empty(&self) -> bool {
        self.nx == 0 || self.ny == 0
    }

    /// Flat index of a cell.
    pub fn id(&self, ix: usize, iy: usize) -> usize {
        iy * self.nx + ix
    }

    /// World position of a cell.
    pub fn pos(&self, ix: usize, iy: usize) -> V2 {
        V2::new(
            self.origin.x + ix as f64 * self.h,
            self.origin.y + iy as f64 * self.h,
        )
    }

    /// World position of a flat index.
    pub fn pos_id(&self, id: usize) -> V2 {
        self.pos(id % self.nx, id / self.nx)
    }
}

/// Marks cells whose centre lies inside the region.
///
/// Rows are scanned with a small deterministic jitter so that grid lines which
/// coincide with horizontal model edges do not double-count crossings.
pub fn scan_inside(region: &Region, g: &Grid) -> Vec<bool> {
    let jit = 3.7123e-4 * g.h;
    let mut rows: Vec<Vec<f64>> = vec![Vec::new(); g.ny];
    for ring in &region.rings {
        let n = ring.len();
        for i in 0..n {
            let a = ring[i];
            let b = ring[(i + 1) % n];
            if a.y == b.y {
                continue;
            }
            let (lo, hi) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
            let j0 = (((lo - g.origin.y - jit) / g.h).ceil()).max(0.0) as usize;
            let j1f = ((hi - g.origin.y - jit) / g.h).ceil() - 1.0;
            if j1f < 0.0 {
                continue;
            }
            let j1 = (j1f as usize).min(g.ny - 1);
            if j0 > j1 {
                continue;
            }
            let slope = (b.x - a.x) / (b.y - a.y);
            for (offset, row) in rows[j0..=j1].iter_mut().enumerate() {
                let j = j0 + offset;
                let y = g.origin.y + j as f64 * g.h + jit;
                row.push(a.x + (y - a.y) * slope);
            }
        }
    }
    let mut mask = vec![false; g.len()];
    for (j, xs) in rows.iter_mut().enumerate() {
        xs.sort_by(|p, q| p.partial_cmp(q).unwrap());
        let mut k = 0;
        while k + 1 < xs.len() {
            let i0 = (((xs[k] - g.origin.x) / g.h).ceil()).max(0.0) as usize;
            let i1f = ((xs[k + 1] - g.origin.x) / g.h).ceil() - 1.0;
            if i1f >= 0.0 {
                let i1 = (i1f as usize).min(g.nx - 1);
                for i in i0..=i1 {
                    mask[j * g.nx + i] = true;
                }
            }
            k += 2;
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Region {
        Region::new(vec![vec![
            V2::new(x0, y0),
            V2::new(x1, y0),
            V2::new(x1, y1),
            V2::new(x0, y1),
        ]])
    }

    #[test]
    fn scan_marks_the_interior_only() {
        let r = square(0.0, 0.0, 10.0, 10.0);
        let g = Grid::covering(r.bounds(), 0.5);
        let mask = scan_inside(&r, &g);
        assert!(mask[g.id(10, 10)], "the centre must be inside");
        assert!(
            !mask[g.id(g.nx - 1, g.ny - 1)],
            "the far corner must be outside"
        );
        assert!(!mask[g.id(g.nx - 1, 10)], "the far X edge must be outside");
    }
}
