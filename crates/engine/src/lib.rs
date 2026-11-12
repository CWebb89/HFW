//! Public headless facade for the HFW slicing engine.
//!
//! The pipeline spans two number domains by design. Mesh import and slicing run
//! in exact integer nanometres, so contour reconstruction is deterministic and
//! open or branching cross-sections are rejected instead of guessed. Everything
//! after that — distance fields, medial axes, offsets, infill — is numerical and
//! runs in millimetres. [`region_from_contours`] is the single conversion point.
//!
//! This crate stops at planned bead paths. It does not emit G-code.

#![forbid(unsafe_code)]

use hfw_geometry::Contour;
use hfw_infill::infill_layer as generate_infill;
use hfw_kernel::{Region, V2};
use hfw_slicing::{LayerSchedule, SliceError, SliceLayer};
use hfw_support::SupportParams;
use hfw_toolpath::{evaluate, evaluate_full, extruded_volume, PlannedLayer};

pub use hfw_geometry::{Contour as SliceContour, Point2};
pub use hfw_infill::{
    infill_layer, InfillParams, InfillParams as InfillSettings, MaskCache, Pattern as InfillPattern,
};
pub use hfw_mesh::{Mesh, MeshDiagnostics, MeshError, MeshRepairReport, MeshTransform};
pub use hfw_slicing::{slice_mesh, SliceError as SlicingError, SliceLayer as Slice};
pub use hfw_support::{Style as SupportStyle, SupportParams as SupportSettings};
pub use hfw_toolpath::{
    evaluate as evaluate_perimeter, evaluate_full as evaluate_all, line_volume, JobTotals, Metrics,
    PlannedLayer as PlannedSlice, MANIFEST_HEADER,
};
pub use hfw_walls::field::FieldError;
pub use hfw_walls::{
    generate as generate_walls, Kind as PathKind, Line as Path, WallMode as Mode, WallParams,
};
pub use hfw_walls::{WallsError, MAX_FIELD_CELLS};

/// Engine-wide settings for one job.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Wall generation parameters, including extrusion width and loop count.
    pub wall: WallParams,
    /// Wall strategy.
    pub mode: Mode,
    /// Support settings; `None` skips support generation.
    pub support: Option<SupportParams>,
    /// Infill settings; `None` skips infill entirely.
    pub infill: Option<InfillParams>,
    /// Generate walls on every Nth layer, plus the last. 1 means every layer.
    pub every: usize,
    /// Sampling resolution for coverage metrics; `None` skips metrics.
    pub metrics_res: Option<f64>,
    /// Keep the exact slice outline on each planned layer for inspection.
    pub keep_reference: bool,
    /// Fail the whole job on the first layer that cannot be closed.
    ///
    /// Defaults to false: a single degenerate plane, such as a cone tip that
    /// collapses to a point, is skipped and reported instead of discarding an
    /// otherwise printable stack.
    pub strict_slices: bool,
    /// Adhesion brim loops around the first layer. 0 disables the brim.
    pub brim_loops: usize,
    /// Clearance between the part and the innermost brim loop, in millimetres.
    pub brim_gap: f64,
}

impl Settings {
    /// Defaults for extrusion width `w` and `walls` loops.
    pub fn new(w: f64, walls: usize) -> Settings {
        Settings {
            wall: WallParams::new(w, walls),
            mode: Mode::Hfw,
            infill: Some(InfillParams::new(w)),
            support: None,
            every: 1,
            metrics_res: None,
            keep_reference: true,
            strict_slices: false,
            brim_loops: 0,
            brim_gap: 0.1,
        }
    }
}

impl Default for Settings {
    fn default() -> Settings {
        Settings::new(0.4, 3)
    }
}

/// Errors that can stop a job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineError {
    /// Slicing failed: bad layer height, open cross-section, or bad input range.
    Slice(SliceError),
    /// A wall distance field would exceed the memory budget.
    Walls(WallsError),
}

impl From<SliceError> for EngineError {
    fn from(error: SliceError) -> Self {
        EngineError::Slice(error)
    }
}

impl From<WallsError> for EngineError {
    fn from(error: WallsError) -> Self {
        EngineError::Walls(error)
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Slice(e) => write!(f, "slicing failed: {e}"),
            EngineError::Walls(e) => write!(f, "wall generation failed: {e}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Converts integer slice contours into a millimetre region.
///
/// This is the only place where the exact integer pipeline hands over to the
/// numerical one. Coordinates divide exactly by 1 000 000, so no rounding is
/// introduced beyond the float conversion itself.
pub fn region_from_contours(contours: &[Contour]) -> Region {
    Region::new(
        contours
            .iter()
            .map(|c| {
                c.points()
                    .iter()
                    .map(|p| p.to_mm())
                    .map(|xy| V2::new(xy[0], xy[1]))
                    .collect()
            })
            .collect(),
    )
}

/// Timing and size counters for one job.
#[derive(Clone, Copy, Debug, Default)]
pub struct JobStats {
    /// Layers in the schedule.
    pub layers: usize,
    /// Layers whose walls were generated.
    pub processed_layers: usize,
    /// Triangles intersected across the whole stack.
    pub triangles_tested: usize,
    /// Distance-field cells allocated in total.
    pub field_nodes: usize,
    /// Cells classified as belonging to a thin wall.
    pub thin_nodes: usize,
    /// Cells dropped as unprintable.
    pub dropped_nodes: usize,
    /// Medial-axis vertices extracted in total.
    pub ridge_vertices: usize,
    /// Milliseconds spent slicing.
    pub ms_slice: f64,
    /// Milliseconds spent generating walls.
    pub ms_walls: f64,
    /// Milliseconds spent generating infill.
    pub ms_infill: f64,
    /// Milliseconds spent generating support masks.
    pub ms_support: f64,
    /// Milliseconds spent measuring coverage.
    pub ms_metrics: f64,
}

/// A sliced and planned job.
pub struct Job {
    /// Mesh diagnostics of the mesh that was actually sliced.
    pub mesh_diagnostics: MeshDiagnostics,
    /// Layers in print order.
    pub layers: Vec<PlannedLayer>,
    /// Per-layer coverage metrics, when requested.
    pub metrics: Vec<Metrics>,
    /// Perimeter volume and interior volume.
    pub volume: (f64, f64),
    /// Counters and timings.
    pub stats: JobStats,
    /// Largest per-layer wall generation time, in milliseconds.
    pub worst_layer_ms: f64,
    /// Index of the layer that took [`Job::worst_layer_ms`].
    pub worst_layer: usize,
    /// Layers that produced no closed cross-section, with the reason.
    pub skipped: Vec<(usize, String)>,
}

/// Slices `mesh` and plans beads for every layer.
pub fn run(
    mesh: &Mesh,
    first_layer_height: f64,
    layer_height: f64,
    settings: &Settings,
) -> Result<Job, EngineError> {
    let t_slice = std::time::Instant::now();
    let (schedule, slices) = slice_stack(mesh, first_layer_height, layer_height)?;
    let ms_slice = t_slice.elapsed().as_secs_f64() * 1e3;

    let mut skipped = Vec::new();
    let mut regions: Vec<Region> = Vec::with_capacity(slices.len());
    for (index, slice) in slices.iter().enumerate() {
        match slice {
            Ok(layer) => regions.push(region_from_contours(&layer.contours)),
            Err(error) => {
                if settings.strict_slices {
                    return Err(EngineError::Slice(error.clone()));
                }
                skipped.push((index, error.to_string()));
                regions.push(Region::default());
            }
        }
    }

    let step = settings.every.max(1);
    let mut cache = if settings.infill.is_some() || settings.support.is_some() {
        let needed = settings
            .infill
            .as_ref()
            .map(|ip| ip.top_layers + ip.bottom_layers + 1)
            .unwrap_or(1)
            .max(
                settings
                    .support
                    .as_ref()
                    .map(|sp| sp.interface_layers + 2)
                    .unwrap_or(1),
            );
        let res = settings
            .infill
            .as_ref()
            .map(|ip| ip.skin_res)
            .unwrap_or(settings.wall.w / 4.0);
        Some(MaskCache::new(&regions, res, needed))
    } else {
        None
    };
    let support_ms;
    let supports = match (settings.support.as_ref(), cache.as_mut()) {
        (Some(sp), Some(cache)) => {
            let t = std::time::Instant::now();
            let height = schedule
                .layers()
                .first()
                .map(|s| (s.print_z - s.bottom_z) as f64 / 1e6)
                .unwrap_or(0.2);
            let masks = hfw_support::Supports::build(&regions, cache, sp, height);
            support_ms = t.elapsed().as_secs_f64() * 1e3;
            Some(masks)
        }
        _ => {
            support_ms = 0.0;
            None
        }
    };

    let mut layers = Vec::with_capacity(schedule.len());
    let mut stats = JobStats {
        ms_support: support_ms,
        layers: schedule.len(),
        triangles_tested: slices
            .iter()
            .filter_map(|s| s.as_ref().ok())
            .map(|s| s.diagnostics.triangles_tested)
            .sum(),
        ms_slice,
        ..JobStats::default()
    };
    let mut metrics = Vec::new();
    let (mut worst_ms, mut worst_layer) = (0.0f64, 0usize);
    let last = schedule.len().saturating_sub(1);

    for (k, (spec, _slice)) in schedule.layers().iter().zip(slices.iter()).enumerate() {
        let mut planned = PlannedLayer {
            index: k,
            bottom_z: spec.bottom_z as f64 / 1e6,
            slice_z: spec.slice_z as f64 / 1e6,
            print_z: spec.print_z as f64 / 1e6,
            reference: if settings.keep_reference {
                regions[k].rings.clone()
            } else {
                Vec::new()
            },
            paths: Vec::new(),
            counts: Default::default(),
        };

        if !regions[k].is_empty() && (k % step == 0 || k == last) {
            let t = std::time::Instant::now();
            if let (Some(masks), Some(sp)) = (supports.as_ref(), settings.support.as_ref()) {
                planned.paths.extend(masks.fill(k, sp));
            }
            if k == 0 && settings.brim_loops > 0 {
                planned.paths.extend(hfw_walls::brim(
                    &regions[k],
                    &settings.wall,
                    settings.brim_loops,
                    settings.brim_gap,
                )?);
            }
            let out = hfw_walls::generate(&regions[k], &settings.wall, settings.mode)?;
            let layer_ms = out.stats.ms_total;
            stats.processed_layers += 1;
            stats.field_nodes += out.stats.nodes;
            stats.thin_nodes += out.stats.thin_nodes;
            stats.dropped_nodes += out.stats.dropped_nodes;
            stats.ridge_vertices += out.stats.ridge_vertices;
            stats.ms_walls += layer_ms;
            if layer_ms > worst_ms {
                worst_ms = layer_ms;
                worst_layer = k;
            }
            planned.paths.extend(out.lines);

            if let (Some(ip), Some(cache)) = (settings.infill.as_ref(), cache.as_mut()) {
                let t2 = std::time::Instant::now();
                planned
                    .paths
                    .extend(generate_infill(&regions[k], cache, k, &settings.wall, ip));
                stats.ms_infill += t2.elapsed().as_secs_f64() * 1e3;
            }
            planned.recount();
            if let Some(res) = settings.metrics_res {
                let t2 = std::time::Instant::now();
                let shell: Vec<hfw_walls::Line> = planned
                    .paths
                    .iter()
                    .filter(|l| l.kind.is_wall())
                    .cloned()
                    .collect();
                let mut m = evaluate(&regions[k], &shell, &settings.wall, res);
                let full = evaluate_full(&regions[k], &planned.paths, &settings.wall, res);
                m.area_region = full.area_region;
                metrics.push(m);
                stats.ms_metrics += t2.elapsed().as_secs_f64() * 1e3;
            }
            let _ = t;
        }

        layers.push(planned);
    }

    let mut volume = (0.0, 0.0);
    for layer in &layers {
        let (w, i) = extruded_volume(&layer.paths, layer.height());
        volume.0 += w;
        volume.1 += i;
    }

    Ok(Job {
        mesh_diagnostics: mesh.diagnostics(),
        layers,
        metrics,
        volume,
        stats,
        worst_layer_ms: worst_ms,
        worst_layer,
        skipped,
    })
}

fn slice_stack(
    mesh: &Mesh,
    first_layer_height: f64,
    layer_height: f64,
) -> Result<(LayerSchedule, Vec<Result<SliceLayer, SliceError>>), SliceError> {
    let (z_min, z_max) = mesh
        .z_bounds()
        .ok_or(SliceError::InvalidZRange { z_min: 0, z_max: 0 })?;
    let schedule = LayerSchedule::new(
        z_min,
        z_max,
        hfw_geometry::mm_to_coord(first_layer_height)?,
        hfw_geometry::mm_to_coord(layer_height)?,
    )?;
    let slices = hfw_slicing::slice_mesh_layers_lenient(mesh, &schedule);
    Ok((schedule, slices))
}

/// Slices one plane and returns its millimetre region.
pub fn slice_plane(mesh: &Mesh, z_mm: f64) -> Result<(SliceLayer, Region), EngineError> {
    let layer = slice_mesh(mesh, hfw_slicing::SlicePlane::from_mm(z_mm)?)?;
    let region = region_from_contours(&layer.contours);
    Ok((layer, region))
}

/// Convenience helper: total path length of a job.
pub fn total_length(job: &Job) -> f64 {
    job.layers.iter().map(|l| l.counts.length).sum()
}

/// Convenience helper: total extruder start/stop count of a job.
pub fn total_starts_stops(job: &Job) -> usize {
    job.layers.iter().map(|l| l.counts.starts_stops).sum()
}

/// True when the settings request solid infill everywhere.
pub fn is_solid_infill_infinite_density(settings: &Settings) -> bool {
    settings
        .infill
        .as_ref()
        .is_some_and(|ip| ip.density >= 0.999 && ip.pattern == InfillPattern::Rectilinear)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hfw_geometry::Point2;

    fn cube() -> Mesh {
        Mesh::box_mm(20.0, 20.0, 20.0).expect("cube")
    }

    fn square_contour(side: f64) -> Contour {
        Contour::new(vec![
            Point2::from_mm(0.0, 0.0).unwrap(),
            Point2::from_mm(side, 0.0).unwrap(),
            Point2::from_mm(side, side).unwrap(),
            Point2::from_mm(0.0, side).unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn the_bridge_preserves_exact_areas() {
        let region = region_from_contours(&[square_contour(20.0)]);
        assert_eq!(region.rings.len(), 1);
        assert!((region.area() - 400.0).abs() < 1e-9);
    }

    #[test]
    fn the_bridge_normalises_hole_winding() {
        let outer = square_contour(20.0);
        let mut hole = Contour::new(vec![
            Point2::from_mm(5.0, 5.0).unwrap(),
            Point2::from_mm(5.0, 10.0).unwrap(),
            Point2::from_mm(10.0, 10.0).unwrap(),
            Point2::from_mm(10.0, 5.0).unwrap(),
        ])
        .unwrap();
        hole.normalize_winding(true);
        let region = region_from_contours(&[outer, hole]);
        assert_eq!(region.rings.len(), 2);
        assert!((region.area() - (400.0 - 25.0)).abs() < 1e-9);
        assert_eq!(
            region
                .rings
                .iter()
                .filter(|r| hfw_kernel::ring_signed_area(r) > 0.0)
                .count(),
            1
        );
    }

    #[test]
    fn a_cube_produces_walls_and_infill_on_every_layer() {
        let mut settings = Settings::new(0.4, 3);
        settings.infill = Some(InfillParams::new(0.4));
        let job = run(&cube(), 0.2, 0.2, &settings).expect("job");
        assert_eq!(job.stats.layers, 100);
        assert_eq!(job.layers.len(), 100);
        for layer in &job.layers {
            assert!(
                !layer.paths.is_empty(),
                "layer {} has no paths",
                layer.index
            );
            assert!(
                layer.counts.walls >= 3,
                "layer {} has {} walls",
                layer.index,
                layer.counts.walls
            );
        }
        assert!(job.volume.0 > 0.0);
    }

    #[test]
    fn solid_infill_conserves_the_cube_volume() {
        let mut settings = Settings::new(0.4, 3);
        let mut ip = InfillParams::new(0.4);
        ip.density = 1.0;
        ip.top_layers = 1;
        ip.bottom_layers = 1;
        settings.infill = Some(ip);
        let job = run(&cube(), 0.2, 0.2, &settings).expect("job");
        let volume = job.volume.0 + job.volume.1;
        let ratio = volume / 8000.0;
        assert!(ratio > 0.98, "only {:.4} of the cube was deposited", ratio);
        assert!(ratio < 1.02, "{:.4} is more than the cube", ratio);
    }
}
