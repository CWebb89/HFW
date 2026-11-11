//! Exact signed distance to the region boundary on a uniform grid.

use hfw_kernel::{Bounds, Region, SegIndex, V2};

/// Sentinel for "no segment index recorded".
pub const NONE: u32 = u32::MAX;

/// Memory budget for one distance field.
///
/// The field stores four parallel arrays per cell. A part large enough to
/// exceed this budget at the requested resolution is rejected with a clear
/// error rather than exhausting memory.
pub const MAX_FIELD_CELLS: usize = 32_000_000;

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

/// Failure modes when building a distance field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldError {
    /// The requested resolution needs more cells than the field budget allows.
    TooLarge {
        /// Cell count required.
        cells: usize,
        /// Cell count permitted.
        limit: usize,
    },
    /// The requested cell size is not a positive finite number.
    InvalidResolution,
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FieldError::TooLarge { cells, limit } => write!(
                f,
                "distance field needs {cells} cells but the limit is {limit}; \
                 raise --width for a coarser field or slice the model in pieces"
            ),
            FieldError::InvalidResolution => {
                f.write_str("distance field resolution must be a positive finite number")
            }
        }
    }
}

impl std::error::Error for FieldError {}

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

/// Signed distance field sampling of one region.
pub struct Field {
    /// Sampling grid.
    pub grid: Grid,
    /// Inside/outside flag per cell.
    pub inside: Vec<bool>,
    /// Distance to the nearest boundary segment; infinite when beyond `cap`.
    pub d: Vec<f64>,
    /// Index of the nearest boundary segment.
    pub seg: Vec<u32>,
    /// Closest point on that segment.
    pub foot: Vec<V2>,
}

impl Field {
    /// Samples the distance field of `region`, truncated at `cap`.
    pub fn build(
        region: &Region,
        index: &SegIndex,
        grid: Grid,
        cap: f64,
    ) -> Result<Field, FieldError> {
        if !(grid.h.is_finite() && grid.h > 0.0) {
            return Err(FieldError::InvalidResolution);
        }
        if grid.len() > MAX_FIELD_CELLS {
            return Err(FieldError::TooLarge {
                cells: grid.len(),
                limit: MAX_FIELD_CELLS,
            });
        }
        let inside = scan_inside(region, &grid);
        let n = grid.len();
        let mut d = vec![0.0; n];
        let mut seg = vec![NONE; n];
        let mut foot = vec![V2::ZERO; n];
        for (id, flag) in inside.iter().enumerate() {
            if !*flag {
                continue;
            }
            match index.nearest(grid.pos_id(id), cap) {
                Some(hit) => {
                    d[id] = hit.d;
                    seg[id] = hit.seg;
                    foot[id] = hit.foot;
                }
                None => d[id] = f64::INFINITY,
            }
        }
        Ok(Field {
            grid,
            inside,
            d,
            seg,
            foot,
        })
    }
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

    #[test]
    fn field_reports_distance_to_the_nearest_edge() {
        let r = square(0.0, 0.0, 10.0, 10.0);
        let idx = SegIndex::new(r.segments(), r.bounds().grown(5.0), 1.0);
        let g = Grid::covering(r.bounds(), 0.25);
        let f = Field::build(&r, &idx, g, 6.0).expect("field");
        let id = g.id(20, 20);
        assert!((f.d[id] - 5.0).abs() < 0.5);
        assert!(f.d[id].is_finite());
    }

    #[test]
    fn oversized_grids_are_rejected_before_allocating() {
        let r = square(0.0, 0.0, 10.0, 10.0);
        let idx = SegIndex::new(r.segments(), r.bounds().grown(5.0), 1.0);
        let mut big = Bounds::empty();
        big.add(V2::new(-1000.0, -1000.0));
        big.add(V2::new(1000.0, 1000.0));
        let g = Grid::covering(big, 0.001);
        assert!(matches!(
            Field::build(&r, &idx, g, 5.0),
            Err(FieldError::TooLarge { .. })
        ));
    }

    #[test]
    fn non_positive_resolution_is_rejected() {
        let r = square(0.0, 0.0, 10.0, 10.0);
        let idx = SegIndex::new(r.segments(), r.bounds().grown(5.0), 1.0);
        let g = Grid::covering(r.bounds(), 1.0);
        let mut bad = g;
        bad.h = 0.0;
        assert!(matches!(
            Field::build(&r, &idx, bad, 5.0),
            Err(FieldError::InvalidResolution)
        ));
    }
}
