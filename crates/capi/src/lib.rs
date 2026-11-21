//! The C ABI of the HFW slicing engine.
//!
//! This is the boundary next-layer crosses to hold geometry without a process:
//! a mesh is loaded once, transformed in place, and handed back with the rest of
//! a plate when it is time to slice. Handles, not Rust types, cross the
//! boundary — the caller owns a pointer and nothing else knows what is behind it.
//!
//! The surface is deliberately flat and versioned. [`HFW_ABI_VERSION`] changes
//! whenever a symbol disappears, a signature changes, or a struct passed by value
//! gains, loses or reorders a field, so a caller compiled against an older header
//! can refuse rather than corrupt memory. `include/hfw.h` is the C declaration of
//! everything below, and the two are kept in step by the gate, which compiles a C
//! program against the header and runs it against this library.
//!
//! Every entry point reports failure two ways: a null pointer or a negative
//! integer, and a message on [`hfw_last_error`], which is per-thread. A call that
//! succeeds clears it, so a caller never reads a stale message.
//!
//! # Why this crate allows `unsafe`
//!
//! The workspace forbids unsafe code, and this crate lifts that for itself alone.
//! A C ABI cannot be written without it: a pointer that crossed a language
//! boundary has to be dereferenced, and a handle handed back by a caller has to be
//! rebuilt with [`Box::from_raw`] before it can be dropped. Nothing else in the
//! workspace relaxed the same lint, and nothing here is reachable from Rust code
//! that could have avoided it.

#![deny(unsafe_op_in_unsafe_fn)]

use std::cell::RefCell;
use std::ffi::{c_char, c_int, CStr, CString};
use std::path::Path;
use std::ptr;
use std::sync::OnceLock;

mod job;
mod preview;

use hfw_config::Bundle;
use hfw_config::PrinterConfig;
use hfw_engine::{
    InLayerPolyline, Job, Marker, MarkerKind, Mesh, MeshTransform, Paint, PlateOrder, Settings,
    VolumeKind,
};
use hfw_font::Face;
use hfw_mesh::CutPlane;
use hfw_plate::{Dialect, ForeignRefusal, ObjectId, Plate, PlateObject, PlateSnapshot, VolumeId};
use hfw_text::text_prism;

/// What `hfw_mesh_load_file` reads, in the order the app's file dialog offers them.
static LOAD_EXTENSIONS: &CStr = c".3mf,.stl,.obj";

/// Version of the surface this library exports.
///
/// It changes whenever a symbol is removed, a signature changes, or a struct
/// passed across the boundary gains, loses or reorders a field.
pub const HFW_ABI_VERSION: u32 = 1;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Axis-aligned bounds of a mesh, in millimetres.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HfwBounds {
    /// Lower corner, `[x, y, z]`.
    pub min: [f64; 3],
    /// Upper corner, `[x, y, z]`.
    pub max: [f64; 3],
}

/// A placement applied to a mesh.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HfwTransform {
    /// Scale factors along X, Y and Z; each must be finite and non-zero.
    pub scale: [f64; 3],
    /// Euler rotations in degrees, applied X then Y then Z.
    pub rotation_degrees: [f64; 3],
    /// Translation in millimetres.
    pub translation_mm: [f64; 3],
}

impl Default for HfwTransform {
    fn default() -> Self {
        Self {
            scale: [1.0; 3],
            rotation_degrees: [0.0; 3],
            translation_mm: [0.0; 3],
        }
    }
}

/// An opaque mesh owned by this library.
pub struct HfwMesh {
    mesh: Mesh,
    paint: Paint,
}

/// An opaque plate document owned by this library.
pub struct HfwPlate {
    plate: Plate,
}

/// An opaque snapshot of a plate document, owned by this library.
pub struct HfwSnapshot {
    snapshot: PlateSnapshot,
}

/// An opaque preset bundle: the presets under one directory, indexed.
pub struct HfwBundle {
    bundle: Bundle,
    root: String,
}

/// An opaque slice result owned by this library.
pub struct HfwGcode {
    text: CString,
    bytes: usize,
    verified: bool,
}

fn set_error(message: &str) {
    let text = message.replace('\0', " ");
    LAST_ERROR.with(|slot| *slot.borrow_mut() = CString::new(text).ok());
}

fn clear_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

fn report<T>(result: Result<T, String>, on_error: T) -> T {
    match result {
        Ok(value) => {
            clear_error();
            value
        }
        Err(message) => {
            set_error(&message);
            on_error
        }
    }
}

/// Reads three doubles the caller handed over as a point or a direction.
unsafe fn read_point(value: *const f64, what: &str) -> Result<[f64; 3], String> {
    if value.is_null() {
        return Err(format!("{what} is null"));
    }
    let slice = unsafe { std::slice::from_raw_parts(value, 3) };
    if slice.iter().any(|part| !part.is_finite()) {
        return Err(format!("{what} holds a value that is not a number"));
    }
    Ok([slice[0], slice[1], slice[2]])
}

/// Turns a `*const c_char` the caller handed over into a `String`.
unsafe fn text_of(value: *const c_char, what: &str) -> Result<String, String> {
    if value.is_null() {
        return Err(format!("{what} is null"));
    }
    let slice = unsafe { CStr::from_ptr(value) };
    slice
        .to_str()
        .map(str::to_owned)
        .map_err(|_| format!("{what} is not valid UTF-8"))
}

/// Borrows one paint channel's states out of a C array.
unsafe fn paint_states(states: *const u8, len: usize) -> Result<Vec<u8>, String> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if states.is_null() {
        return Err("paint states is null".to_owned());
    }
    Ok(unsafe { std::slice::from_raw_parts(states, len) }.to_vec())
}

/// Borrows the mesh behind a handle.
unsafe fn mesh_of<'a>(handle: *const HfwMesh) -> Result<&'a HfwMesh, String> {
    if handle.is_null() {
        return Err("mesh handle is null".to_owned());
    }
    Ok(unsafe { &*handle })
}

/// Borrows the mesh behind a handle, writable.
unsafe fn mesh_mut<'a>(handle: *mut HfwMesh) -> Result<&'a mut HfwMesh, String> {
    if handle.is_null() {
        return Err("mesh handle is null".to_owned());
    }
    Ok(unsafe { &mut *handle })
}

/// Borrows the plate behind a handle.
unsafe fn plate_of<'a>(handle: *const HfwPlate) -> Result<&'a HfwPlate, String> {
    if handle.is_null() {
        return Err("plate handle is null".to_owned());
    }
    Ok(unsafe { &*handle })
}

/// Borrows the plate behind a handle, for changing it.
unsafe fn plate_mut<'a>(handle: *mut HfwPlate) -> Result<&'a mut HfwPlate, String> {
    if handle.is_null() {
        return Err("plate handle is null".to_owned());
    }
    Ok(unsafe { &mut *handle })
}

fn object_of(plate: &Plate, id: u64) -> Result<&PlateObject, String> {
    let id = ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
    plate
        .object(id)
        .ok_or_else(|| format!("no object {id} is on the plate"))
}

/// Blocking heights for one job: the profile's own when the caller passes zero.
pub(crate) fn heights_of(profile: &PrinterConfig, first: f64, layer: f64) -> (f64, f64) {
    let first = if first > 0.0 {
        first
    } else {
        profile.first_layer_height
    };
    let layer = if layer > 0.0 {
        layer
    } else {
        profile.layer_height
    };
    (first, layer)
}

fn bounds_of(mesh: &Mesh) -> Result<HfwBounds, String> {
    let (x0, y0, x1, y1) = mesh
        .xy_bounds_mm()
        .ok_or_else(|| "the mesh has no area to report bounds for".to_owned())?;
    let (z0, z1) = mesh
        .z_bounds()
        .ok_or_else(|| "the mesh has no height to report bounds for".to_owned())?;
    Ok(bounds_from(
        x0,
        y0,
        z0 as f64 / 1_000_000.0,
        x1,
        y1,
        z1 as f64 / 1_000_000.0,
    ))
}

/// Appends `text` as a JSON string, escaped.
fn push_json_text(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
}

/// A JSON object of string rows, with every key and value escaped.
fn json_object(rows: &[(String, String)]) -> String {
    let mut json = String::from("{");
    for (index, (key, value)) in rows.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        push_json_text(&mut json, key);
        json.push(':');
        push_json_text(&mut json, value);
    }
    json.push('}');
    json
}

fn bounds_from(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> HfwBounds {
    HfwBounds {
        min: [x0, y0, z0],
        max: [x1, y1, z1],
    }
}

/// Writes a planned job to a file, and asks the independent verifier about it.
///
/// The text form is the ABI's: `hfw_gcode_text` hands the caller a string, so a
/// profile that asks for the binary container is honoured by the command line and
/// not here, where there would be no text to return and no thumbnails in it.
///
/// `positioned` is what the command line calls `--place`, inverted: a positioned
/// job prints where the document put it, and an unpositioned one is laid on the
/// bed the profile describes first, which is the only thing that centres a plate
/// that arrived as bare geometry.
pub(crate) fn write_job(
    job: &Job,
    profile: &PrinterConfig,
    positioned: bool,
) -> Result<HfwGcode, String> {
    let mut write_cfg = profile.clone();
    write_cfg.keep_position = positioned;
    write_cfg.binary_gcode = false;
    let written = hfw_gcode::generate_with_extras(
        &job.layers,
        &write_cfg,
        Some("plate"),
        &hfw_gcode::Extras {
            seam_paint: &job.seam_paint,
            markers: &job.markers,
            in_layer: &job.in_layer,
        },
    )
    .map_err(|error| error.to_string())?;
    let verified = hfw_gcode::verify(&written.text, &write_cfg).ok();
    let bytes = written.text.len();
    let text = CString::new(written.text)
        .map_err(|_| "the generated G-code contains a NUL byte".to_owned())?;
    Ok(HfwGcode {
        text,
        bytes,
        verified,
    })
}

/// The order this profile prints a plate in.
///
/// One reader for every path: a command line, a synchronous slice and a job
/// thread all ask this, so `print_sequence = by object` means the same thing to
/// all three instead of being honoured by one and ignored by the rest.
fn order_of(cfg: &PrinterConfig) -> PlateOrder {
    if cfg.print_sequence == hfw_config::PrintSequence::ByObject {
        PlateOrder::ByObject
    } else {
        PlateOrder::ByLayer
    }
}

fn slice_meshes(
    handles: &[*const HfwMesh],
    profile: &str,
    first_layer_height: f64,
    layer_height: f64,
) -> Result<HfwGcode, String> {
    let cfg = PrinterConfig::load(Path::new(profile)).map_err(|error| error.to_string())?;
    let mut settings = Settings::default();
    settings.apply_printer(&cfg);
    let (first, layer) = heights_of(&cfg, first_layer_height, layer_height);
    let mut meshes = Vec::with_capacity(handles.len());
    for handle in handles {
        let mesh = unsafe { mesh_of(*handle) }?;
        meshes.push((mesh.mesh.clone(), mesh.paint.clone()));
    }
    let objects: Vec<hfw_plate::PlacedObject> = meshes
        .iter()
        .map(|(mesh, paint)| hfw_plate::PlacedObject::new(mesh, Some(paint), &settings))
        .collect();
    let job = hfw_plate::slice_placed(objects, first, layer, order_of(&cfg))
        .map_err(|error| error.to_string())?;
    write_job(&job, &cfg, true)
}

/// Plans the plate the way the command line would, without writing G-code.
fn plan_plate(
    plate: &Plate,
    profile: &str,
    first_layer_height: f64,
    layer_height: f64,
    bed: Option<usize>,
) -> Result<(Job, PrinterConfig), String> {
    let cfg = PrinterConfig::load(Path::new(profile)).map_err(|error| error.to_string())?;
    let mut settings = plate.settings().clone();
    settings.apply_printer(&cfg);
    // The command line finishes the infill geometry from the width it ended up with,
    // after every option has had its say, and nothing else writes G-code this
    // application prints. Doing it here as well is what makes a slice through the
    // library the same file as a slice through the command line: without it the skin
    // resolution and the overlap keep the profile's own numbers, and the two differ
    // by a seam's worth of toolpath on every layer.
    if let Some(infill) = settings.infill.as_mut() {
        infill.overlap = 0.1 * infill.width;
        infill.skin_res = infill.width / 4.0;
        infill.step = infill.width / 4.0;
    }
    let (first, layer) = heights_of(&cfg, first_layer_height, layer_height);
    let job = match bed {
        Some(bed) => plate.slice_bed_using(&settings, bed, first, layer, order_of(&cfg)),
        None => plate.slice_using(&settings, first, layer, order_of(&cfg)),
    }
    .map_err(|error| error.to_string())?;
    Ok((job, cfg))
}

fn slice_document(
    plate: &Plate,
    profile: &str,
    first_layer_height: f64,
    layer_height: f64,
    positioned: bool,
    bed: Option<usize>,
) -> Result<HfwGcode, String> {
    let (job, cfg) = plan_plate(plate, profile, first_layer_height, layer_height, bed)?;
    write_job(&job, &cfg, positioned)
}

/// Returns the version of the surface this library exports.
#[no_mangle]
pub extern "C" fn hfw_abi_version() -> u32 {
    HFW_ABI_VERSION
}

/// Plans the plate and returns the preview document, NUL-terminated.
///
/// Every planned bead carries the label the G-code writer gives it, so a viewer
/// groups paths exactly as the printer's own `;TYPE:` markers do. The string is
/// owned by the caller: release it with [`hfw_string_free`].
///
/// # Safety
///
/// `plate` must be a live handle and `profile` a NUL-terminated path.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_preview_json(
    plate: *const HfwPlate,
    profile: *const c_char,
    first_layer_height: f64,
    layer_height: f64,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        let plate = unsafe { plate_of(plate) }?;
        let profile = unsafe { text_of(profile, "profile") }?;
        let (job, _) = plan_plate(
            &plate.plate,
            &profile,
            first_layer_height,
            layer_height,
            None,
        )?;
        let document = preview::document(&job);
        CString::new(document)
            .map(|text| text.into_raw())
            .map_err(|_| "the preview document carries a NUL byte".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// A job running on its own thread.
///
/// Planning and writing a plate takes minutes, and a settings panel that freezes
/// for minutes reads as a crash. A job is started on a thread of its own, polled
/// for the stage and the counters it has reached, and cancelled if the plate is
/// altered before it finishes. Nothing crosses the boundary but numbers: a C
/// callback that unwound on a foreign thread is not something this library can
/// make safe, so the caller polls from the timer it already runs.
pub struct HfwJob {
    job: job::Job,
}

/// What a poll reports: the job is still running.
pub const HFW_JOB_RUNNING: c_int = job::RUNNING;
/// What a poll reports: the job finished and its G-code is available.
pub const HFW_JOB_DONE: c_int = job::DONE;
/// What a poll reports: the job was cancelled and produced nothing.
pub const HFW_JOB_CANCELLED: c_int = job::CANCELLED;
/// What a poll reports: the job failed; [`hfw_last_error`] says why.
pub const HFW_JOB_FAILED: c_int = job::FAILED;

/// Stage a job is in while it plans the plate, one object at a time.
pub const HFW_JOB_STAGE_SLICING: c_int = job::Stage::Slicing as c_int;
/// Stage a job is in while it writes and verifies the file.
pub const HFW_JOB_STAGE_WRITING: c_int = job::Stage::Writing as c_int;
/// Stage a job is in once its G-code is ready.
pub const HFW_JOB_STAGE_FINISHED: c_int = job::Stage::Finished as c_int;

/// Where a running job has got to.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HfwJobProgress {
    /// One of the `HFW_JOB_STAGE_*` values.
    pub stage: c_int,
    /// Work done in the current stage: objects planned, or layers written.
    pub done: usize,
    /// Work to do in the current stage, or zero while it is not yet known.
    pub total: usize,
}

/// Starts a job for `plate` with `profile` applied, on its own thread.
///
/// The plate is copied into the job, so the caller may keep editing the plate —
/// or free it — while the job runs. Returns NULL when the thread cannot start.
///
/// # Safety
///
/// `plate` must be a live handle and `profile` a NUL-terminated path.
#[no_mangle]
pub unsafe extern "C" fn hfw_job_start(
    plate: *const HfwPlate,
    profile: *const c_char,
    first_layer_height: f64,
    layer_height: f64,
) -> *mut HfwJob {
    let build = || -> Result<*mut HfwJob, String> {
        let plate = unsafe { plate_of(plate) }?;
        let profile = unsafe { text_of(profile, "profile") }?;
        let job = job::start(&plate.plate, &profile, first_layer_height, layer_height)?;
        Ok(Box::into_raw(Box::new(HfwJob { job })))
    };
    report(build(), ptr::null_mut())
}

/// Reports the job's status and copies its progress into `out`, without blocking.
///
/// The status is `HFW_JOB_RUNNING`, `HFW_JOB_DONE`, `HFW_JOB_CANCELLED` or
/// `HFW_JOB_FAILED`; `out` may be NULL when only the status is wanted.
///
/// # Safety
///
/// `job` must be a live handle and `out`, when not NULL, must be writable.
#[no_mangle]
pub unsafe extern "C" fn hfw_job_poll(job: *mut HfwJob, out: *mut HfwJobProgress) -> c_int {
    let Some(job) = (unsafe { job.as_ref() }) else {
        set_error("job handle is null");
        return HFW_JOB_FAILED;
    };
    let (status, progress) = job.job.poll();
    if !out.is_null() {
        unsafe {
            *out = HfwJobProgress {
                stage: progress.stage,
                done: progress.done,
                total: progress.total,
            };
        }
    }
    if status == HFW_JOB_FAILED {
        let message = job
            .job
            .failure()
            .unwrap_or_else(|| "the job failed".to_owned());
        set_error(&message);
    }
    status
}

/// Asks the job to stop. A job that has already finished is unaffected.
///
/// Cancellation is observed before each object is planned and once more before
/// the file is written, so a cancelled job yields no G-code at all rather than a
/// partial file. Returns 0, or -1 for a null handle.
///
/// # Safety
///
/// `job` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_job_cancel(job: *mut HfwJob) -> c_int {
    let Some(job) = (unsafe { job.as_ref() }) else {
        set_error("job handle is null");
        return -1;
    };
    job.job.cancel();
    0
}

/// Waits for the job to finish and reports its final status.
///
/// # Safety
///
/// `job` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_job_wait(job: *mut HfwJob) -> c_int {
    let Some(job) = (unsafe { job.as_ref() }) else {
        set_error("job handle is null");
        return HFW_JOB_FAILED;
    };
    job.job.wait();
    let handle = job as *const HfwJob as *mut HfwJob;
    unsafe { hfw_job_poll(handle, ptr::null_mut()) }
}

/// Takes the job's G-code, or NULL when it has not finished, failed or was
/// cancelled; [`hfw_last_error`] then says which.
///
/// The job handle is consumed: the caller must not use it afterwards. Release the
/// G-code with [`hfw_gcode_free`].
///
/// # Safety
///
/// `job` must be a live handle, and must not be used again after this call.
#[no_mangle]
pub unsafe extern "C" fn hfw_job_take_gcode(job: *mut HfwJob) -> *mut HfwGcode {
    if job.is_null() {
        set_error("job handle is null");
        return ptr::null_mut();
    }
    let job = *unsafe { Box::from_raw(job) };
    match job.job.take_gcode() {
        Ok(gcode) => {
            clear_error();
            Box::into_raw(Box::new(gcode))
        }
        Err(message) => {
            set_error(&message);
            ptr::null_mut()
        }
    }
}

/// Releases a job handle, cancelling it first and waiting for its thread. A null
/// handle is ignored.
///
/// # Safety
///
/// `job` must be a live handle that has not been passed to
/// [`hfw_job_take_gcode`].
#[no_mangle]
pub unsafe extern "C" fn hfw_job_free(job: *mut HfwJob) {
    if job.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(job) });
}

/// Copies object `id` on the plate and returns the copy's id, or zero on error.
///
/// Geometry, placement, settings and overrides are copied; the copy is placed
/// exactly on top of the original and named after it, so a caller that wants it
/// beside its neighbour moves it with [`hfw_plate_object_placement`] or lays the
/// plate out with [`hfw_plate_arrange`].
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_duplicate(plate: *mut HfwPlate, id: u64) -> u64 {
    let build = || -> Result<u64, String> {
        let plate = unsafe { plate_mut(plate) }?;
        let object = ObjectId::from_number(id).ok_or_else(|| format!("no object has id {id}"))?;
        let copy = plate
            .plate
            .duplicate(object)
            .map_err(|error| error.to_string())?;
        Ok(copy.get())
    };
    report(build(), 0)
}

/// Turns object `id` about its own centre on the bed by `degrees` around `axis`:
/// 0 for X, 1 for Y, 2 for Z.
///
/// The centre stays where it was, so the part spins in place, and rotations on the
/// same axis add up. Returns 0, or -1 when the object is not on the plate or the
/// axis or angle is not one this engine accepts.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_rotate(
    plate: *mut HfwPlate,
    id: u64,
    axis: c_int,
    degrees: f64,
) -> c_int {
    let build = || -> Result<c_int, String> {
        let plate = unsafe { plate_mut(plate) }?;
        let object = ObjectId::from_number(id).ok_or_else(|| format!("no object has id {id}"))?;
        let axis =
            usize::try_from(axis).map_err(|_| format!("axis {axis} is not one of 0, 1, 2"))?;
        plate
            .plate
            .rotate(object, axis, degrees)
            .map_err(|error| error.to_string())?;
        Ok(0)
    };
    report(build(), -1)
}

/// Mirrors object `id` through its own centre on the bed, along `axis`: 0 for X,
/// 1 for Y, 2 for Z.
///
/// The part stays where it is — a mirror about the bed origin would move it off the
/// plate — and the winding is corrected, so it still slices. Returns 0, or -1 when
/// the object is unknown or the axis is refused.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_mirror(
    plate: *mut HfwPlate,
    id: u64,
    axis: c_int,
) -> c_int {
    let build = || -> Result<c_int, String> {
        let plate = unsafe { plate_mut(plate) }?;
        let object = ObjectId::from_number(id).ok_or_else(|| format!("no object has id {id}"))?;
        let axis =
            usize::try_from(axis).map_err(|_| format!("axis {axis} is not one of 0, 1, 2"))?;
        plate
            .plate
            .mirror(object, axis)
            .map_err(|error| error.to_string())?;
        Ok(0)
    };
    report(build(), -1)
}

/// Lays the plate out on a `bed_x` by `bed_y` bed, leaving `spacing` millimetres
/// between any two objects.
///
/// Objects a project file positioned keep their coordinates and the rest are
/// packed around them; the geometry is never rewritten, only the placement. A part
/// that does not fit is refused with its size in [`hfw_last_error`] rather than
/// omitted, because an omitted part prints a different job.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_arrange(
    plate: *mut HfwPlate,
    bed_x: f64,
    bed_y: f64,
    spacing: f64,
) -> c_int {
    let build = || -> Result<c_int, String> {
        let plate = unsafe { plate_mut(plate) }?;
        if !(bed_x > 0.0 && bed_y > 0.0) {
            return Err(format!("a {bed_x} x {bed_y} bed has no room"));
        }
        if !spacing.is_finite() || spacing < 0.0 {
            return Err(format!("a spacing of {spacing} mm is not a distance"));
        }
        plate
            .plate
            .arrange((bed_x, bed_y), spacing)
            .map_err(|error| error.to_string())?;
        Ok(0)
    };
    report(build(), -1)
}

/// Returns the plate as a project file, and writes its length into `out_len`.
///
/// The bytes are a `.3mf`: the layout in the standard model part, so any slicer can
/// open it, and what a 3MF has no field for — which objects are off, and each
/// object's settings — in `Metadata/hfw_project.xml` beside it. Release them with
/// [`hfw_buffer_free`].
///
/// # Safety
///
/// `plate` must be a live handle and `out_len` must be writable.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_project_bytes(
    plate: *const HfwPlate,
    out_len: *mut usize,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        let plate = unsafe { plate_of(plate) }?;
        if out_len.is_null() {
            return Err("the length pointer is null".to_owned());
        }
        let bytes = plate
            .plate
            .to_project()
            .map_err(|error| error.to_string())?
            .into_boxed_slice();
        let len = bytes.len();
        let pointer = Box::into_raw(bytes) as *mut c_char;
        unsafe {
            *out_len = len;
        }
        Ok(pointer)
    };
    report(build(), ptr::null_mut())
}

/// Releases a byte buffer this library returned, with the length it reported.
///
/// # Safety
///
/// `bytes` and `len` must be the pointer and length one call returned, and must
/// not be freed twice.
#[no_mangle]
pub unsafe extern "C" fn hfw_buffer_free(bytes: *mut c_char, len: usize) {
    if bytes.is_null() || len == 0 {
        return;
    }
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(bytes as *mut u8, len)) });
}

/// Opens a project file and returns the plate it describes, or NULL on error.
///
/// `profile` is a printer profile path, or NULL for the engine's defaults; it is
/// the plate's own settings layer, exactly as a slice takes it. Every object is
/// marked positioned, because its coordinates came from the file.
///
/// # Safety
///
/// `bytes` must be readable for `len` bytes, and `profile` a NUL-terminated path
/// or NULL.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_open_project(
    bytes: *const c_char,
    len: usize,
    profile: *const c_char,
) -> *mut HfwPlate {
    let build = || -> Result<*mut HfwPlate, String> {
        if bytes.is_null() {
            return Err("the project pointer is null".to_owned());
        }
        let data = unsafe { std::slice::from_raw_parts(bytes as *const u8, len) };
        let settings = match unsafe { text_of(profile, "profile") } {
            Ok(path) => {
                let config =
                    PrinterConfig::load(Path::new(&path)).map_err(|error| error.to_string())?;
                let mut settings = Settings::default();
                settings.apply_printer(&config);
                settings
            }
            Err(_) => Settings::default(),
        };
        let plate = Plate::from_project(data, settings).map_err(|error| error.to_string())?;
        Ok(Box::into_raw(Box::new(HfwPlate { plate })))
    };
    report(build(), ptr::null_mut())
}

/// Stores the caller's own bytes on the plate, saved and reopened with it.
///
/// The engine never reads them; they are the application's state that has to travel
/// with the document -- which plates exist, what they are called, which tool has
/// which colour -- and storing them here is what keeps a saved plate one file.
/// Passing a `len` of zero clears them.
///
/// Returns 0 on success and -1 on a null plate or a null pointer with a non-zero
/// length.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call, and `bytes` readable for
/// `len` bytes or null.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_set_metadata(
    plate: *mut HfwPlate,
    bytes: *const c_char,
    len: usize,
) -> c_int {
    let set = || -> Result<c_int, String> {
        let plate = unsafe { plate_mut(plate) }?;
        if len == 0 {
            plate.plate.set_metadata(Vec::new());
            return Ok(0);
        }
        if bytes.is_null() {
            return Err("metadata bytes are null".to_owned());
        }
        let data = unsafe { std::slice::from_raw_parts(bytes as *const u8, len) };
        plate.plate.set_metadata(data.to_vec());
        Ok(0)
    };
    report(set(), -1)
}

/// Returns the caller's own bytes as a NUL-terminated string, reporting its length in
/// `out_len`, or NULL when the plate carries none.
///
/// Release it with `hfw_string_free`. The length is the real one, so bytes that are
/// not text -- a document whose own format is binary -- survive a round trip.
///
/// # Safety
///
/// `plate` must be a live handle and `out_len` writable for one `size_t`.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_metadata_get(
    plate: *const HfwPlate,
    out_len: *mut usize,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        let plate = unsafe { plate_of(plate) }?;
        if out_len.is_null() {
            return Err("the length pointer is null".to_owned());
        }
        let metadata = plate.plate.metadata();
        if metadata.is_empty() {
            unsafe { *out_len = 0 };
            return Ok(ptr::null_mut());
        }
        unsafe { *out_len = metadata.len() };
        CString::new(metadata)
            .map(CString::into_raw)
            .map_err(|_| "the metadata contains a NUL".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// Writes the ids of the plate's objects into `out`, and returns how many there
/// are.
///
/// The count is reported even when `out` is null or too short, so a caller sizes
/// its buffer with one call it can also use to read. Ids are stable for as long as
/// the object is on the plate and are never reused.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for `len` 64-bit ids, or null.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_ids(
    plate: *const HfwPlate,
    out: *mut u64,
    len: usize,
) -> usize {
    let count = || -> Result<usize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let objects = plate.plate.objects();
        if !out.is_null() {
            let buffer = unsafe { std::slice::from_raw_parts_mut(out, len) };
            for (slot, object) in buffer.iter_mut().zip(objects) {
                *slot = object.id().get();
            }
        }
        Ok(objects.len())
    };
    report(count(), 0)
}

/// Writes object `id`'s name into `out` and returns how many bytes it needs,
/// excluding the terminating NUL.
///
/// A buffer that is too short writes nothing and reports the length the name
/// needs, so a caller can retry with the right size. Returns -1 when the object is
/// not on the plate or the name cannot be written.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for `len` bytes, or null.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_name_get(
    plate: *const HfwPlate,
    id: u64,
    out: *mut c_char,
    len: usize,
) -> isize {
    let read = || -> Result<isize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        let name = object.name();
        let needed = name.len();
        if out.is_null() || len < needed + 1 {
            return Ok(needed as isize);
        }
        let buffer = unsafe { std::slice::from_raw_parts_mut(out as *mut u8, len) };
        buffer[..needed].copy_from_slice(name.as_bytes());
        buffer[needed] = 0;
        Ok(needed as isize)
    };
    report(read(), -1)
}

/// Writes object `id`'s placement into `out`.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for one `HfwTransform`.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_placement_get(
    plate: *const HfwPlate,
    id: u64,
    out: *mut HfwTransform,
) -> c_int {
    let read = || -> Result<c_int, String> {
        let plate = unsafe { plate_of(plate) }?;
        if out.is_null() {
            return Err("output pointer is null".to_owned());
        }
        let object = object_of(&plate.plate, id)?;
        let placement = object.placement();
        unsafe {
            out.write(HfwTransform {
                scale: placement.scale,
                rotation_degrees: placement.rotation_degrees,
                translation_mm: placement.translation_mm,
            });
        }
        Ok(0)
    };
    report(read(), -1)
}

/// Reads object `id`'s placement back as a row-major 4x4 matrix.
///
/// This is the inverse of [`hfw_plate_object_placement_matrix`], and the shape a
/// caller that keeps transforms as matrices wants: the sixteen doubles compose to
/// the same placement, so a document snapshot does not have to convert twice.
///
/// Returns 0 on success and -1 when the object is not on the plate or `out` is null.
///
/// # Safety
///
/// `out` must point at sixteen writable doubles.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_placement_matrix_get(
    plate: *const HfwPlate,
    id: u64,
    out: *mut f64,
) -> c_int {
    let read = || -> Result<c_int, String> {
        let plate = unsafe { plate_of(plate) }?;
        if out.is_null() {
            return Err("output pointer is null".to_owned());
        }
        let object = object_of(&plate.plate, id)?;
        let matrix = object.placement().to_matrix();
        unsafe {
            std::ptr::copy_nonoverlapping(matrix.as_ptr(), out, 16);
        }
        Ok(0)
    };
    report(read(), -1)
}

/// Writes 1 into `out` when object `id` is switched on, 0 when it is not.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for one `int32_t`.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_enabled_get(
    plate: *const HfwPlate,
    id: u64,
    out: *mut c_int,
) -> c_int {
    let read = || -> Result<c_int, String> {
        let plate = unsafe { plate_of(plate) }?;
        if out.is_null() {
            return Err("output pointer is null".to_owned());
        }
        let object = object_of(&plate.plate, id)?;
        unsafe { *out = i32::from(object.enabled()) };
        Ok(0)
    };
    report(read(), -1)
}

/// Returns a copy of object `id`'s own geometry, in the object's frame, or NULL on
/// error.
///
/// The copy carries the object's paint channels, so a reader that keeps the mesh
/// keeps what was painted on it. Release it with [`hfw_mesh_free`].
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_mesh(plate: *const HfwPlate, id: u64) -> *mut HfwMesh {
    let build = || -> Result<*mut HfwMesh, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        Ok(Box::into_raw(Box::new(HfwMesh {
            mesh: object.mesh().clone(),
            paint: object.paint().clone(),
        })))
    };
    report(build(), ptr::null_mut())
}

/// Returns object `id`'s own settings as a JSON object of key/value strings, or
/// NULL on error.
///
/// The values are what [`hfw_plate_object_set`] accepts, so a document can be read
/// back and rebuilt setting for setting. Release the string with
/// [`hfw_string_free`].
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_settings_json(
    plate: *const HfwPlate,
    id: u64,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        let settings = object.effective_settings(plate.plate.settings());
        let mut rows: Vec<(String, String)> = Vec::new();
        for key in object.overrides() {
            let value = settings
                .key_value(key)
                .map_err(|error| format!("`{key}` cannot be read back: {error}"))?;
            rows.push((key.to_owned(), value));
        }
        let json = json_object(&rows);
        CString::new(json)
            .map(CString::into_raw)
            .map_err(|_| "the settings JSON contains a NUL".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// Returns a copy of volume `volume` of object `id`, or NULL on error.
///
/// Release it with [`hfw_mesh_free`]. The geometry is in the volume's own frame; the
/// volume's placement says where that frame sits inside the object.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_mesh(
    plate: *const HfwPlate,
    id: u64,
    volume: u64,
) -> *mut HfwMesh {
    let build = || -> Result<*mut HfwMesh, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        let volume = volume_of(object, id, volume)?;
        Ok(Box::into_raw(Box::new(HfwMesh {
            mesh: volume.mesh().clone(),
            paint: Paint::default(),
        })))
    };
    report(build(), ptr::null_mut())
}

/// Returns volume `volume` of object `id` as a JSON object of the settings its
/// region prints with, or NULL on error.
///
/// Release the string with [`hfw_string_free`].
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_settings_json(
    plate: *const HfwPlate,
    id: u64,
    volume: u64,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        let volume = volume_of(object, id, volume)?;
        let rows: Vec<(String, String)> = volume
            .overrides()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        CString::new(json_object(&rows))
            .map(CString::into_raw)
            .map_err(|_| "the settings JSON contains a NUL".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// Writes volume `volume` of object `id` into `out` as a row-major 4x4 matrix, the
/// same shape an object's placement uses.
///
/// Returns 0 on success and -1 when the object or volume does not exist or `out` is
/// null.
///
/// # Safety
///
/// `plate` must be a live handle and `out` must be writable for sixteen doubles.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_name(
    plate: *mut HfwPlate,
    id: u64,
    volume: u64,
    name: *const c_char,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let name = unsafe { text_of(name, "name") }?;
        let plate = unsafe { &mut *plate };
        let object = plate
            .plate
            .object_mut(id_from_number(id)?)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        let held = object
            .volume_mut(volume_id(volume)?)
            .map_err(|error| error.to_string())?;
        held.set_name(name);
        Ok(0)
    };
    report(set(), -1)
}

/// Writes the name of volume `volume` of object `id` into `out`, and returns the
/// bytes it needs excluding the terminating NUL.
///
/// An unnamed volume reports 0 and writes an empty string: the wording a user reads
/// ("Negative volume 1") belongs to the front end, which is the side that knows the
/// kind it drew. A buffer that is too short writes nothing and reports the length, so
/// a caller can size and retry. Returns -1 when the object or volume does not exist.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for `len` bytes, or null.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_name_get(
    plate: *const HfwPlate,
    id: u64,
    volume: u64,
    out: *mut c_char,
    len: usize,
) -> isize {
    let read = || -> Result<isize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        let held = volume_of(object, id, volume)?;
        let name = held.name();
        let needed = name.len();
        if out.is_null() || len < needed + 1 {
            return Ok(needed as isize);
        }
        let buffer = unsafe { std::slice::from_raw_parts_mut(out as *mut u8, len) };
        buffer[..needed].copy_from_slice(name.as_bytes());
        buffer[needed] = 0;
        Ok(needed as isize)
    };
    report(read(), -1)
}

/// Returns 1 when volume `volume` of object `id` is part of the geometry, 0 when it
/// is hidden, and -1 when the object or volume does not exist.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_visible(
    plate: *const HfwPlate,
    id: u64,
    volume: u64,
) -> i32 {
    let read = || -> Result<i32, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        let held = volume_of(object, id, volume)?;
        Ok(i32::from(held.visible()))
    };
    report(read(), -1)
}

/// Shows or hides volume `volume` of object `id`.
///
/// A hidden volume is not printed **and not applied**: a negative volume stops cutting
/// and a modifier stops changing the region under it, which is what the application
/// does with a volume the user hid. Returns 0 on success, -1 when the object or volume
/// does not exist.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_set_visible(
    plate: *mut HfwPlate,
    id: u64,
    volume: u64,
    visible: c_int,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let plate = unsafe { &mut *plate };
        let object = plate
            .plate
            .object_mut(id_from_number(id)?)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        let held = object
            .volume_mut(volume_id(volume)?)
            .map_err(|error| error.to_string())?;
        held.set_visible(visible != 0);
        Ok(0)
    };
    report(set(), -1)
}

/// Reads volume `volume` of object `id`'s placement back as a row-major 4x4 matrix,
/// sixteen doubles.
///
/// Returns 0 on success and -1 when the object or volume does not exist or `out` is
/// null.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for sixteen doubles.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_placement_get(
    plate: *const HfwPlate,
    id: u64,
    volume: u64,
    out: *mut f64,
) -> c_int {
    let read = || -> Result<c_int, String> {
        let plate = unsafe { plate_of(plate) }?;
        if out.is_null() {
            return Err("output pointer is null".to_owned());
        }
        let object = object_of(&plate.plate, id)?;
        let matrix = volume_of(object, id, volume)?.placement().to_matrix();
        unsafe {
            std::ptr::copy_nonoverlapping(matrix.as_ptr(), out, 16);
        }
        Ok(0)
    };
    report(read(), -1)
}

/// Releases a string this library returned, such as the preview document. A null
/// pointer is ignored.
///
/// # Safety
///
/// `text` must be a pointer returned by a function in this library that
/// transfers ownership, and must not be freed twice.
#[no_mangle]
pub unsafe extern "C" fn hfw_string_free(text: *mut c_char) {
    if text.is_null() {
        return;
    }
    drop(unsafe { CString::from_raw(text) });
}

/// Opens the preset bundle under `root`: every preset file below it, indexed by kind
/// and name.
///
/// A bundle here is the directory a front end keeps its presets in, in the layout
/// `docs/PRESETS.md` describes: files of `[machine:name]`, `[process:name]` and
/// `[filament:name]` sections, one preset per file, either form, discovered
/// recursively. The handle is the whole surface a preset store needs -- the machines,
/// the profiles one machine accepts, and what a selection resolves to -- and it is
/// opaque because the indexing is this library's business.
///
/// `reload` re-reads the same directory, so a preset added or edited while the
/// application runs is picked up without relaunching it. Release the handle with
/// `hfw_bundle_free`. Returns NULL with the reason when the directory holds no presets
/// or cannot be read.
///
/// # Safety
///
/// `root` must be a readable NUL-terminated path.
#[no_mangle]
pub unsafe extern "C" fn hfw_bundle_open(root: *const c_char) -> *mut HfwBundle {
    let open = || -> Result<*mut HfwBundle, String> {
        let root = unsafe { text_of(root, "root") }?;
        let bundle = Bundle::discover(Path::new(&root)).map_err(|error| error.to_string())?;
        Ok(Box::into_raw(Box::new(HfwBundle { bundle, root })))
    };
    report(open(), ptr::null_mut())
}

/// Releases a bundle handle. A null handle is ignored.
///
/// # Safety
///
/// `bundle` must be a handle from [`hfw_bundle_open`] that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn hfw_bundle_free(bundle: *mut HfwBundle) {
    if bundle.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(bundle) });
}

/// Re-reads the directory the bundle was opened from.
///
/// This is what makes a preset library live: a file added, edited or deleted while the
/// application runs is indexed on the next call, with no relaunch and no second handle.
/// The old index is replaced only when the new one reads, so a bundle that goes
/// unreadable leaves the caller with what it had and the reason why. Returns 0 on
/// success, -1 with the reason on failure.
///
/// # Safety
///
/// `bundle` must be a live handle, writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_bundle_reload(bundle: *mut HfwBundle) -> i32 {
    let reload = || -> Result<i32, String> {
        if bundle.is_null() {
            return Err("bundle handle is null".to_owned());
        }
        let held = unsafe { &mut *bundle };
        let fresh = Bundle::discover(Path::new(&held.root)).map_err(|error| error.to_string())?;
        held.bundle = fresh;
        Ok(0)
    };
    report(reload(), -1)
}

/// Returns the machines a bundle holds, as JSON.
///
/// `{"printer_models":[{"name","vendor_name","vendor_id","technology":"FFF",
/// "variants":[{"id","printer_profiles":[{"name","extruders_cnt",
/// "bed":{"width","height","max_print_height"}}]}]}]}`. One printer profile per
/// machine, because a bundle here has one hardware configuration per machine preset
/// where the reference slicers group several. `name` is what the other two calls are
/// made with. Release the string with `hfw_string_free`.
///
/// # Safety
///
/// `bundle` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_bundle_printer_models(bundle: *const HfwBundle) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        if bundle.is_null() {
            return Err("bundle handle is null".to_owned());
        }
        let held = unsafe { &*bundle };
        CString::new(hfw_config::presets::printer_models(&held.bundle))
            .map(CString::into_raw)
            .map_err(|_| "the printer list contains a NUL".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// Returns the profiles one machine accepts, as JSON.
///
/// `{"print_profiles":[{"name","filament_profiles":[...]}]}`: every process in the
/// bundle, each listing every filament. Returns NULL with the reason when the machine is
/// not in the bundle. Release the string with `hfw_string_free`.
///
/// # Safety
///
/// `bundle` must be a live handle and `printer` a readable string.
#[no_mangle]
pub unsafe extern "C" fn hfw_bundle_profiles_for(
    bundle: *const HfwBundle,
    printer: *const c_char,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        if bundle.is_null() {
            return Err("bundle handle is null".to_owned());
        }
        let printer = unsafe { text_of(printer, "printer") }?;
        let held = unsafe { &*bundle };
        let json = hfw_config::presets::profiles_for(&held.bundle, &printer)
            .map_err(|error| error.to_string())?;
        CString::new(json)
            .map(CString::into_raw)
            .map_err(|_| "the profile list contains a NUL".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// Returns what a machine, a process and materials resolve to, as one JSON document.
///
/// `{"preset":{...},"configuration":{"print_settings":{...},
/// "filament_settings":{...},"printer_settings":{...}},"warnings":[...]}` -- the
/// shape a preset store already parses. Every setting carries the dialect name a panel
/// files it under and the JSON type its value takes, taken from the same generated
/// surface the panel reads, so the two cannot disagree about a name, a location or a
/// type. `filaments` is a list of material preset names, of which the first non-empty
/// one is used: this engine prints with one extruder.
///
/// The machine is required; a process or a material the bundle does not hold is a
/// warning in the document rather than a failure. Release the string with
/// `hfw_string_free`.
///
/// # Safety
///
/// `bundle` must be a live handle, `printer` and `print` readable strings, and
/// `filaments` either null or `count` readable strings.
#[no_mangle]
pub unsafe extern "C" fn hfw_bundle_resolved_config(
    bundle: *const HfwBundle,
    printer: *const c_char,
    print: *const c_char,
    filaments: *const *const c_char,
    count: usize,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        if bundle.is_null() {
            return Err("bundle handle is null".to_owned());
        }
        let printer = unsafe { text_of(printer, "printer") }?;
        let print = unsafe { text_of(print, "print") }?;
        let mut names = Vec::new();
        if !filaments.is_null() {
            let handles = unsafe { std::slice::from_raw_parts(filaments, count) };
            for handle in handles {
                names.push(unsafe { text_of(*handle, "filament") }?);
            }
        }
        let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
        let held = unsafe { &*bundle };
        let json = hfw_config::presets::resolved_config(&held.bundle, &printer, &print, &borrowed)
            .map_err(|error| error.to_string())?;
        CString::new(json)
            .map(CString::into_raw)
            .map_err(|_| "the resolved configuration contains a NUL".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// Returns one line saying what a bundle holds: its counts and any warnings.
///
/// Release the string with `hfw_string_free`.
///
/// # Safety
///
/// `bundle` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_bundle_summary(bundle: *const HfwBundle) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        if bundle.is_null() {
            return Err("bundle handle is null".to_owned());
        }
        let held = unsafe { &*bundle };
        CString::new(hfw_config::presets::summary(&held.bundle))
            .map(CString::into_raw)
            .map_err(|_| "the bundle summary contains a NUL".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// Returns the settings surface as a JSON document.
///
/// The pointer is to a NUL-terminated copy that lives for the life of the
/// process, so the caller must not free it. It is the same document
/// `hfw-cli --settings-surface` writes and `docs/SETTINGS-LEDGER.md` is
/// generated alongside, so a panel built from it and the engine cannot disagree
/// about whether a setting is honoured.
#[no_mangle]
pub extern "C" fn hfw_settings_surface_json() -> *const c_char {
    static SURFACE: OnceLock<CString> = OnceLock::new();
    SURFACE
        .get_or_init(|| CString::new(hfw_config::surface::json()).unwrap_or_default())
        .as_ptr()
}

/// Returns the dialect this crate accepts as a JSON document.
///
/// The pointer refers to a NUL-terminated string that lives for the life of the process;
/// do not free it. `renames` maps an Orca/Bambu spelling to the engine spelling,
/// `applied` and `partial` map an engine setting to this engine's key, `orca_only` lists
/// dialect names the engine has no setting for, and `engine_names` is every setting the
/// engine defines. An importer that reads this accepts exactly the spellings this engine
/// accepts, instead of keeping a table of its own.
#[no_mangle]
pub extern "C" fn hfw_dialect_json() -> *const c_char {
    static DIALECT: OnceLock<CString> = OnceLock::new();
    DIALECT
        .get_or_init(|| CString::new(hfw_config::dialect::json()).unwrap_or_default())
        .as_ptr()
}

/// Returns the message of the last failed call on this thread.
///
/// The pointer stays valid until the next call on this thread. It is null when
/// the last call succeeded.
#[no_mangle]
pub extern "C" fn hfw_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(ptr::null(), |text| text.as_ptr())
    })
}

/// Loads a mesh from an STL or 3MF file.
///
/// # Safety
///
/// `path` must be a NUL-terminated string. The returned handle must be released
/// with [`hfw_mesh_free`], and never used after it.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_load_file(path: *const c_char) -> *mut HfwMesh {
    report(
        unsafe { text_of(path, "path") }.and_then(|path| {
            Mesh::load_model(Path::new(&path))
                .map(|mesh| {
                    Box::into_raw(Box::new(HfwMesh {
                        mesh,
                        paint: Paint::default(),
                    }))
                })
                .map_err(|error| error.to_string())
        }),
        ptr::null_mut(),
    )
}

/// The file extensions [`hfw_mesh_load_file`] reads, comma separated with the
/// leading dot: `.3mf,.stl,.obj`.
///
/// The pointer refers to a NUL-terminated string that lives for the life of the
/// process; do not free it.
#[no_mangle]
pub extern "C" fn hfw_mesh_load_extensions() -> *const c_char {
    LOAD_EXTENSIONS.as_ptr()
}

/// Cuts a mesh with a plane, returning the part below and the part above.
///
/// Both halves are closed: the cut's own opening is capped, and a cut that passes through
/// a hole caps the hole's ring too. `normal` and `point` are three doubles each, in
/// millimetres; the plane passes through `point` facing `normal`. A cut that misses the
/// mesh puts it in one half and leaves the other with no triangles.
///
/// On failure both outputs are NULL and the error names the reason, which is a normal with
/// no direction or a coordinate out of range. Each returned handle must be released with
/// [`hfw_mesh_free`].
///
/// # Safety
///
/// `normal` and `point` must point at three readable doubles each, and `below` and
/// `above` must point at writable handle slots.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_cut(
    mesh: *const HfwMesh,
    normal: *const f64,
    point: *const f64,
    below: *mut *mut HfwMesh,
    above: *mut *mut HfwMesh,
) -> c_int {
    let outcome = (|| -> Result<(Mesh, Mesh), String> {
        let mesh = unsafe { mesh.as_ref() }.ok_or_else(|| "mesh is null".to_owned())?;
        let normal = unsafe { read_point(normal, "normal") }?;
        let point = unsafe { read_point(point, "point") }?;
        let plane = CutPlane::through(normal, point).map_err(|error| error.to_string())?;
        let cut = mesh.mesh.cut(plane).map_err(|error| error.to_string())?;
        Ok((cut.below, cut.above))
    })();
    match outcome {
        Ok((below_mesh, above_mesh)) => {
            if below.is_null() || above.is_null() {
                set_error("below or above is null");
                return -1;
            }
            unsafe {
                below.write(Box::into_raw(Box::new(HfwMesh {
                    mesh: below_mesh,
                    paint: Paint::default(),
                })));
                above.write(Box::into_raw(Box::new(HfwMesh {
                    mesh: above_mesh,
                    paint: Paint::default(),
                })));
            }
            0
        }
        Err(message) => {
            set_error(&message);
            -1
        }
    }
}

/// How many separate shells a mesh holds.
///
/// A shell is a group of triangles joined to each other, so a plate with two copies of a
/// part imported as one mesh reports two. Use it with [`hfw_mesh_part`] to split one into
/// one mesh per shell.
///
/// # Safety
///
/// `mesh` must be a handle from this library that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_part_count(mesh: *const HfwMesh) -> usize {
    let Some(mesh) = (unsafe { mesh.as_ref() }) else {
        return 0;
    };
    mesh.mesh.parts().map(|parts| parts.len()).unwrap_or(0)
}

/// One shell of a mesh, by index, in the order its first triangle appears.
///
/// Returns NULL when the mesh is null or the index is past the last shell; the handle must
/// be released with [`hfw_mesh_free`].
///
/// # Safety
///
/// `mesh` must be a handle from this library that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_part(mesh: *const HfwMesh, index: usize) -> *mut HfwMesh {
    report(
        (|| -> Result<*mut HfwMesh, String> {
            let mesh = unsafe { mesh.as_ref() }.ok_or_else(|| "mesh is null".to_owned())?;
            let parts = mesh.mesh.parts().map_err(|error| error.to_string())?;
            let part = parts
                .into_iter()
                .nth(index)
                .ok_or_else(|| format!("the mesh has no shell {index}"))?;
            Ok(Box::into_raw(Box::new(HfwMesh {
                mesh: part,
                paint: Paint::default(),
            })))
        })(),
        ptr::null_mut(),
    )
}

/// Builds a mesh from a string of text, extruded from a font file.
///
/// The mesh is a closed prism: the glyphs of `text` at `size_mm`, raised from `z_mm`
/// to `z_mm + height_mm`, laid out left to right from the origin. Add it to a plate
/// object with [`hfw_plate_object_add_volume`] as a part volume to print it raised, or
/// as a negative volume to engrave it — the slice unions or subtracts it per layer, so
/// no boolean is involved and the result is as exact as any other volume.
///
/// `face_index` selects a face inside a font collection; 0 is the first, and 0 is what
/// a plain `.ttf` or `.otf` has.
///
/// # Safety
///
/// `font_path` must be a NUL-terminated string and `text` must be a NUL-terminated
/// UTF-8 string. The returned handle must be released with [`hfw_mesh_free`], and
/// never used after it.
#[no_mangle]
pub unsafe extern "C" fn hfw_text_mesh(
    font_path: *const c_char,
    face_index: u32,
    text: *const c_char,
    size_mm: f64,
    height_mm: f64,
    z_mm: f64,
) -> *mut HfwMesh {
    report(
        (|| -> Result<*mut HfwMesh, String> {
            let path = unsafe { text_of(font_path, "font_path") }?;
            let label = unsafe { text_of(text, "text") }?;
            if label.trim().is_empty() {
                return Err("text is empty".to_owned());
            }
            let face = Face::load(Path::new(&path), face_index)
                .map_err(|error| format!("{path}: {error}"))?;
            let (prism, _) = text_prism(&face, &label, size_mm, height_mm, 0.0, 0.0, z_mm)
                .map_err(|error| error.to_string())?;
            let mesh = prism.mesh().map_err(|error| error.to_string())?;
            if mesh.triangle_count() == 0 {
                return Err(format!("{path} draws no glyph in {label:?}"));
            }
            Ok(Box::into_raw(Box::new(HfwMesh {
                mesh,
                paint: Paint::default(),
            })))
        })(),
        ptr::null_mut(),
    )
}

/// Builds a mesh from the bytes of an STL file held in memory.
///
/// # Safety
///
/// `bytes` must point at `len` readable bytes and must not be null. The returned
/// handle must be released with [`hfw_mesh_free`], and never used after it.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_from_bytes(bytes: *const u8, len: usize) -> *mut HfwMesh {
    let build = || -> Result<*mut HfwMesh, String> {
        if bytes.is_null() {
            return Err("bytes is null".to_owned());
        }
        if len == 0 {
            return Err("bytes is empty".to_owned());
        }
        let data = unsafe { std::slice::from_raw_parts(bytes, len) };
        let mesh = Mesh::from_stl_bytes(data).map_err(|error| error.to_string())?;
        Ok(Box::into_raw(Box::new(HfwMesh {
            mesh,
            paint: Paint::default(),
        })))
    };
    report(build(), ptr::null_mut())
}

/// Writes a mesh as the bytes of an STL file.
///
/// A non-zero `binary` selects the binary form, which is what a slicer reads; zero
/// writes the ASCII form, which is text a project file can store and a diff can read.
/// Either way the face normal is computed from the triangle. The pointer is
/// `*out_len` bytes long and must be released with [`hfw_buffer_free`].
///
/// # Safety
///
/// `mesh` must be a live handle that has not been freed, `out_len` must point at a
/// writable `size_t`, and the bytes must not be used after they are freed.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_stl_bytes(
    mesh: *const HfwMesh,
    binary: c_int,
    out_len: *mut usize,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        let mesh = unsafe { mesh.as_ref() }.ok_or_else(|| "mesh is null".to_owned())?;
        if out_len.is_null() {
            return Err("the length pointer is null".to_owned());
        }
        let bytes = mesh.mesh.to_stl_bytes(binary != 0).into_boxed_slice();
        let len = bytes.len();
        let pointer = Box::into_raw(bytes) as *mut c_char;
        unsafe {
            *out_len = len;
        }
        Ok(pointer)
    };
    report(build(), ptr::null_mut())
}

/// Builds a mesh from millimetre coordinates and triangle indices.
///
/// `vertices` holds `vertex_count` interleaved x, y, z triples and `indices`
/// `triangle_count` triples of vertex numbers. Nothing is welded and no triangle is
/// dropped, so triangle `i` of the handle is triangle `i` of `indices` -- which is what
/// a caller that holds per-triangle data in its own order needs.
///
/// # Safety
///
/// `vertices` must point at `3 * vertex_count` readable doubles and `indices` at
/// `3 * triangle_count` readable numbers, and neither may be null. The returned
/// handle must be released with [`hfw_mesh_free`], and never used after it.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_new(
    vertices: *const f64,
    vertex_count: usize,
    indices: *const u32,
    triangle_count: usize,
) -> *mut HfwMesh {
    let build = || -> Result<*mut HfwMesh, String> {
        if vertices.is_null() {
            return Err("vertices is null".to_owned());
        }
        if indices.is_null() {
            return Err("indices is null".to_owned());
        }
        if vertex_count == 0 {
            return Err("the mesh has no vertices".to_owned());
        }
        if triangle_count == 0 {
            return Err("the mesh has no triangles".to_owned());
        }
        let coordinates = unsafe { std::slice::from_raw_parts(vertices, vertex_count * 3) };
        let corners = unsafe { std::slice::from_raw_parts(indices, triangle_count * 3) };
        let points: Vec<[f64; 3]> = coordinates
            .chunks_exact(3)
            .map(|vertex| [vertex[0], vertex[1], vertex[2]])
            .collect();
        let faces: Vec<[u32; 3]> = corners
            .chunks_exact(3)
            .map(|face| [face[0], face[1], face[2]])
            .collect();
        let mesh = Mesh::from_millimetres(&points, &faces).map_err(|error| error.to_string())?;
        Ok(Box::into_raw(Box::new(HfwMesh {
            mesh,
            paint: Paint::default(),
        })))
    };
    report(build(), ptr::null_mut())
}

/// Returns a new handle holding `mesh` moved by `transform`.
///
/// The source handle is unchanged and stays the caller's to free. A mirroring
/// scale reverses triangle winding so outward-facing normals stay outward.
///
/// # Safety
///
/// `mesh` must be a live handle. The returned handle must be released with
/// [`hfw_mesh_free`], and never used after it.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_transform(
    mesh: *const HfwMesh,
    transform: HfwTransform,
) -> *mut HfwMesh {
    let build = || -> Result<*mut HfwMesh, String> {
        let source = unsafe { mesh_of(mesh) }?;
        let moved = source
            .mesh
            .transformed(MeshTransform {
                scale: transform.scale,
                rotation_degrees: transform.rotation_degrees,
                translation_mm: transform.translation_mm,
            })
            .map_err(|error| error.to_string())?;
        Ok(Box::into_raw(Box::new(HfwMesh {
            mesh: moved,
            paint: source.paint.clone(),
        })))
    };
    report(build(), ptr::null_mut())
}

/// Writes the mesh's bounds in millimetres into `out`.
///
/// Returns 0 on success and -1 when the mesh has no cross-section or no height.
///
/// # Safety
///
/// `mesh` must be a live handle and `out` must be writable for one
/// [`HfwBounds`].
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_bounds(mesh: *const HfwMesh, out: *mut HfwBounds) -> c_int {
    let write = || -> Result<c_int, String> {
        let source = unsafe { mesh_of(mesh) }?;
        if out.is_null() {
            return Err("out is null".to_owned());
        }
        let bounds = bounds_of(&source.mesh)?;
        unsafe { out.write(bounds) };
        Ok(0)
    };
    report(write(), -1)
}

/// Returns the mesh's triangle count, or 0 for a null handle.
///
/// # Safety
///
/// `mesh` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_triangle_count(mesh: *const HfwMesh) -> usize {
    let count = || -> Result<usize, String> {
        let source = unsafe { mesh_of(mesh) }?;
        Ok(source.mesh.triangle_count())
    };
    report(count(), 0)
}

/// An object's settings are named the way this engine names them.
pub const HFW_DIALECT_ENGINE: i32 = 0;

/// An object's settings are named the way Bambu Studio and OrcaSlicer name them.
pub const HFW_DIALECT_ORCA: i32 = 1;

/// A setting was kept because this engine has no setting by that name.
pub const HFW_FOREIGN_UNKNOWN_NAME: i32 = 0;

/// A setting was kept because this engine does not accept that value.
pub const HFW_FOREIGN_UNUSABLE_VALUE: i32 = 1;

/// Painted support: enforcers and blockers.
pub const HFW_PAINT_SUPPORT: i32 = 0;

/// Painted seam: enforcers and blockers, applied to the outer wall.
pub const HFW_PAINT_SEAM: i32 = 1;

/// Writes one paint channel onto `mesh`.
///
/// `states` points at `len` bytes, one per triangle in the mesh's own triangle
/// order, each of them 0 (none), 1 (enforcer) or 2 (blocker). A `len` of zero
/// clears the channel. Returns 0 on success and -1 when the handle is null, the
/// length is not the triangle count, or a state is not one of the three.
///
/// # Safety
///
/// `mesh` must be a live handle writable for the call, and `states` must point at
/// `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_paint_set(
    mesh: *mut HfwMesh,
    channel: c_int,
    states: *const u8,
    len: usize,
) -> c_int {
    let set = || -> Result<c_int, String> {
        let target = unsafe { mesh_mut(mesh) }?;
        let states = unsafe { paint_states(states, len)? };
        match channel {
            HFW_PAINT_SEAM => {
                target.paint.seam = states;
                target
                    .paint
                    .validate(target.mesh.triangle_count())
                    .map_err(|error| error.to_string())?;
                Ok(0)
            }
            HFW_PAINT_SUPPORT => {
                target.paint.support = states;
                target
                    .paint
                    .validate(target.mesh.triangle_count())
                    .map_err(|error| error.to_string())?;
                Ok(0)
            }
            other => Err(format!("paint channel {other} does not exist")),
        }
    };
    report(set(), -1)
}

/// Reads one paint channel back off `mesh`.
///
/// `out` receives one byte per triangle, in the mesh's own order, and `len` states
/// how many bytes `out` can hold. Returns the number of bytes written on success,
/// and -1 when the handle is null, the channel does not exist, or `len` is not the
/// triangle count.
///
/// # Safety
///
/// `mesh` must be a live handle, and `out` must point at `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_paint_get(
    mesh: *const HfwMesh,
    channel: c_int,
    out: *mut u8,
    len: usize,
) -> isize {
    let get = || -> Result<isize, String> {
        let source = unsafe { mesh_of(mesh) }?;
        let states = match channel {
            HFW_PAINT_SUPPORT => &source.paint.support,
            HFW_PAINT_SEAM => &source.paint.seam,
            other => return Err(format!("paint channel {other} does not exist")),
        };
        let triangles = source.mesh.triangle_count();
        if states.len() != triangles {
            return Ok(0);
        }
        if states.len() > len || (!states.is_empty() && out.is_null()) {
            return Err(format!(
                "{len} bytes were offered for {} triangles",
                states.len()
            ));
        }
        unsafe { std::ptr::copy_nonoverlapping(states.as_ptr(), out, states.len()) };
        Ok(states.len() as isize)
    };
    report(get(), -1)
}

/// Returns the mesh's enclosed volume in cubic millimetres, or NaN for a null
/// handle.
///
/// # Safety
///
/// `mesh` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_volume(mesh: *const HfwMesh) -> f64 {
    let volume = || -> Result<f64, String> {
        let source = unsafe { mesh_of(mesh) }?;
        Ok(source.mesh.volume_mm3())
    };
    report(volume(), f64::NAN)
}

/// Returns how many vertices the mesh has, or 0 for a null handle.
///
/// # Safety
///
/// `mesh` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_vertex_count(mesh: *const HfwMesh) -> usize {
    let count = || -> Result<usize, String> {
        let source = unsafe { mesh_of(mesh) }?;
        Ok(source.mesh.vertices().len())
    };
    report(count(), 0)
}

/// Writes the mesh's vertices into `out` as `count` triples of millimetres.
///
/// `out` must hold `hfw_mesh_vertex_count` triples; a shorter buffer is an error
/// rather than a partial write, because a caller that mis-sized it would render a
/// mesh with vertices missing. Returns 0 on success and -1 on a null handle or a
/// buffer that is too short.
///
/// # Safety
///
/// `mesh` must be a live handle and `out` must be writable for `count * 3`
/// doubles.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_vertices(
    mesh: *const HfwMesh,
    out: *mut f64,
    count: usize,
) -> c_int {
    let read = || -> Result<c_int, String> {
        let source = unsafe { mesh_of(mesh) }?;
        if out.is_null() {
            return Err("the vertex buffer is null".to_owned());
        }
        let vertices = source.mesh.vertices();
        if count < vertices.len() {
            return Err(format!(
                "the vertex buffer holds {count} and the mesh has {}",
                vertices.len()
            ));
        }
        let buffer = unsafe { std::slice::from_raw_parts_mut(out, count * 3) };
        for (slot, vertex) in buffer.chunks_exact_mut(3).zip(vertices) {
            slot.copy_from_slice(&vertex.to_mm());
        }
        Ok(0)
    };
    report(read(), -1)
}

/// Returns how many triangles the mesh has, or 0 for a null handle.
///
/// # Safety
///
/// `mesh` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_index_count(mesh: *const HfwMesh) -> usize {
    let count = || -> Result<usize, String> {
        let source = unsafe { mesh_of(mesh) }?;
        Ok(source.mesh.triangles().len())
    };
    report(count(), 0)
}

/// Writes the mesh's triangle indices into `out`, three per triangle.
///
/// The order is the mesh's own, which is the order paint is indexed by, so a
/// reader that keeps these triangles keeps the paint it read with them aligned.
/// Returns 0 on success and -1 on a null handle or a buffer that is too short.
///
/// # Safety
///
/// `mesh` must be a live handle and `out` must be writable for `count * 3`
/// 32-bit indices.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_indices(
    mesh: *const HfwMesh,
    out: *mut u32,
    count: usize,
) -> c_int {
    let read = || -> Result<c_int, String> {
        let source = unsafe { mesh_of(mesh) }?;
        if out.is_null() {
            return Err("the index buffer is null".to_owned());
        }
        let triangles = source.mesh.triangles();
        if count < triangles.len() {
            return Err(format!(
                "the index buffer holds {count} triangles and the mesh has {}",
                triangles.len()
            ));
        }
        let buffer = unsafe { std::slice::from_raw_parts_mut(out, count * 3) };
        for (slot, triangle) in buffer.chunks_exact_mut(3).zip(triangles) {
            slot.copy_from_slice(&triangle.0);
        }
        Ok(0)
    };
    report(read(), -1)
}

/// Releases a mesh handle. A null handle is ignored.
///
/// # Safety
///
/// `mesh` must be a handle returned by this library that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn hfw_mesh_free(mesh: *mut HfwMesh) {
    if mesh.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(mesh) });
}

/// Slices a list of meshes and returns G-code for it.
///
/// `meshes` points at `count` live handles, each already placed where it belongs
/// on the bed: this call does not arrange anything. It is the version-1 shortcut
/// for a caller that has no document; a caller that does holds a plate instead
/// ([`hfw_plate_new`] and friends) and slices that. `profile` names a printer
/// profile; both layer heights are in millimetres, and a value of zero or less
/// takes the profile's own. The returned handle's text is the file, and it carries
/// the engine's independent verifier verdict.
///
/// # Safety
///
/// `meshes` must point at `count` readable handles that each stay alive for the
/// call, and `profile` must be a NUL-terminated string. The returned handle must
/// be released with [`hfw_gcode_free`], and never used after it.
#[no_mangle]
pub unsafe extern "C" fn hfw_slice_meshes(
    meshes: *const *const HfwMesh,
    count: usize,
    profile: *const c_char,
    first_layer_height: f64,
    layer_height: f64,
) -> *mut HfwGcode {
    let build = || -> Result<*mut HfwGcode, String> {
        if meshes.is_null() {
            return Err("meshes is null".to_owned());
        }
        if count == 0 {
            return Err("the plate holds no meshes".to_owned());
        }
        let profile = unsafe { text_of(profile, "profile") }?;
        let handles = unsafe { std::slice::from_raw_parts(meshes, count) };
        slice_meshes(handles, &profile, first_layer_height, layer_height)
            .map(|gcode| Box::into_raw(Box::new(gcode)))
    };
    report(build(), ptr::null_mut())
}

/// Creates an empty plate with the engine's default settings.
///
/// Release it with [`hfw_plate_free`].
#[no_mangle]
pub extern "C" fn hfw_plate_new() -> *mut HfwPlate {
    clear_error();
    Box::into_raw(Box::new(HfwPlate {
        plate: Plate::new(Settings::default()),
    }))
}

/// Adds a copy of `mesh` to the plate, named after its handle count.
///
/// The mesh stays the caller's to free; the plate holds its own copy. Returns the
/// new object's id, which is never zero, or 0 on failure.
///
/// # Safety
///
/// `plate` must be a live handle, writable for the call, and `mesh` must be a live
/// handle that outlives it.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_add(plate: *mut HfwPlate, mesh: *const HfwMesh) -> u64 {
    let add = || -> Result<u64, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let source = unsafe { mesh_of(mesh) }?;
        let plate = unsafe { &mut *plate };
        let name = format!("object {}", plate.plate.len() + 1);
        plate
            .plate
            .add(name, source.mesh.clone())
            .map(|id| id.get())
            .map_err(|error| error.to_string())
    };
    report(add(), 0)
}

/// Adds `mesh` to the plate as a file placed it, so arranging leaves it alone.
///
/// Returns the new object's id, which is never zero, or 0 on failure.
///
/// # Safety
///
/// `plate` must be a live handle, writable for the call, and `mesh` must be a live
/// handle that outlives it.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_add_positioned(
    plate: *mut HfwPlate,
    mesh: *const HfwMesh,
) -> u64 {
    let add = || -> Result<u64, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let source = unsafe { mesh_of(mesh) }?;
        let plate = unsafe { &mut *plate };
        let name = format!("object {}", plate.plate.len() + 1);
        plate
            .plate
            .add_positioned(name, source.mesh.clone())
            .map(|id| id.get())
            .map_err(|error| error.to_string())
    };
    report(add(), 0)
}

/// Returns how many objects the plate holds, enabled or not.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_count(plate: *const HfwPlate) -> usize {
    let count = || -> Result<usize, String> { Ok(unsafe { plate_of(plate) }?.plate.len()) };
    report(count(), 0)
}

/// Names object `id` on the plate.
///
/// Returns 0 on success and -1 when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call, and `name` must be a
/// NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_name(
    plate: *mut HfwPlate,
    id: u64,
    name: *const c_char,
) -> c_int {
    let rename = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let name = unsafe { text_of(name, "name") }?;
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.set_name(name);
        Ok(0)
    };
    report(rename(), -1)
}

/// Labels object `id` the way a user did, keeping the name it arrived with.
///
/// Name and label are separate because a rename has to be reversible and a save writes
/// the file's own name back; what a list shows is the label when there is one and the
/// name otherwise. An empty label clears it. Returns 0 on success, -1 when the object is
/// not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call and `label` a readable string.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_label(
    plate: *mut HfwPlate,
    id: u64,
    label: *const c_char,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let label = unsafe { text_of(label, "label") }?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id_from_number(id)?)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.set_label(label);
        Ok(0)
    };
    report(set(), -1)
}

/// Writes object `id`'s label into `out` and returns the bytes it needs excluding the
/// terminating NUL.
///
/// An unlabelled object reports 0 and writes an empty string. A buffer that is too
/// short writes nothing and reports the length, so a caller can size and retry. Returns
/// -1 when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for `len` bytes, or null.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_label_get(
    plate: *const HfwPlate,
    id: u64,
    out: *mut c_char,
    len: usize,
) -> isize {
    let read = || -> Result<isize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        unsafe { write_text(object.label(), out, len) }
    };
    report(read(), -1)
}

/// Records the file object `id` was imported from, or clears it with an empty string.
///
/// The path is the object's own, not the caller's bytes: a panel shows it, a reload
/// needs it, and an undo that lost it is a document that cannot say where its parts came
/// from. Returns 0 on success, -1 when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call and `path` a readable string.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_source(
    plate: *mut HfwPlate,
    id: u64,
    path: *const c_char,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let path = unsafe { text_of(path, "path") }?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id_from_number(id)?)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.set_source(path);
        Ok(0)
    };
    report(set(), -1)
}

/// Writes object `id`'s source path into `out` and returns the bytes it needs excluding
/// the terminating NUL.
///
/// An object that came from nowhere -- a primitive -- reports 0 and writes an empty
/// string. A buffer that is too short writes nothing and reports the length. Returns -1
/// when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for `len` bytes, or null.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_source_get(
    plate: *const HfwPlate,
    id: u64,
    out: *mut c_char,
    len: usize,
) -> isize {
    let read = || -> Result<isize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        unsafe { write_text(object.source(), out, len) }
    };
    report(read(), -1)
}

/// Which vocabulary object `id`'s settings are named in: `HFW_DIALECT_ENGINE` (0) for
/// this engine's own names, `HFW_DIALECT_ORCA` (1) for Bambu Studio and OrcaSlicer
/// names. Returns -1 when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_dialect(plate: *const HfwPlate, id: u64) -> i32 {
    let read = || -> Result<i32, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        Ok(dialect_number(object.dialect()))
    };
    report(read(), -1)
}

/// Records which vocabulary object `id`'s settings are named in, which is what a save
/// uses to decide whose names to write.
///
/// Returns 0 on success, -1 when the object is not on the plate or the dialect is not
/// one this engine knows.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_set_dialect(
    plate: *mut HfwPlate,
    id: u64,
    dialect: i32,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let dialect = dialect_of(dialect)?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id_from_number(id)?)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.set_dialect(dialect);
        Ok(0)
    };
    report(set(), -1)
}

/// Applies one setting to object `id`, or keeps it on the object with the reason this
/// engine would not take it.
///
/// This is what an import does with another slicer's per-object settings: the ones this
/// engine understands become overrides and the rest stay on the object verbatim, with
/// why they were not applied, so a save writes them back out and a panel can report what
/// was dropped. Returns 0 when the setting was applied, 1 when it was kept instead, and
/// -1 when the object is not on the plate -- a file from another slicer carrying names
/// this engine has never heard of is expected, not an error.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call, and `key` and `value` readable
/// strings.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_apply_foreign(
    plate: *mut HfwPlate,
    id: u64,
    key: *const c_char,
    value: *const c_char,
) -> i32 {
    let apply = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let key = unsafe { text_of(key, "key") }?;
        let value = unsafe { text_of(value, "value") }?;
        let plate = unsafe { &mut *plate };
        let applied = plate
            .plate
            .apply_foreign_setting(id_from_number(id)?, &key, &value)
            .map_err(|error| error.to_string())?;
        Ok(i32::from(!applied))
    };
    report(apply(), -1)
}

/// Keeps one setting on object `id` verbatim, with why it was not applied.
///
/// This is how a rebuilt document re-attaches what it read back: `reason` is
/// `HFW_FOREIGN_UNKNOWN_NAME` (0) or `HFW_FOREIGN_UNUSABLE_VALUE` (1). Returns 0 on
/// success, -1 when the object is not on the plate or the reason is not one of those.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call, and `key` and `value` readable
/// strings.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_foreign_set(
    plate: *mut HfwPlate,
    id: u64,
    key: *const c_char,
    value: *const c_char,
    reason: i32,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let key = unsafe { text_of(key, "key") }?;
        let value = unsafe { text_of(value, "value") }?;
        let reason = refusal_of(reason)?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id_from_number(id)?)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.record_foreign(key, value, reason);
        Ok(0)
    };
    report(set(), -1)
}

/// Drops every setting object `id` kept from a foreign file.
///
/// Returns 0 on success, -1 when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_foreign_clear(plate: *mut HfwPlate, id: u64) -> i32 {
    let clear = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id_from_number(id)?)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.clear_foreign_settings();
        Ok(0)
    };
    report(clear(), -1)
}

/// Returns object `id`'s kept settings as a JSON array, in the order they were refused:
/// `[{"key":…,"value":…,"reason":…,"note":…}]`.
///
/// The key and value are the file's own spelling, which is what a save writes back; the
/// reason is `unknown_name` or `unusable_value` and the note is the sentence a panel
/// shows. Release the string with `hfw_string_free`. Returns NULL when the object is not
/// on the plate.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_foreign_json(
    plate: *const HfwPlate,
    id: u64,
) -> *mut c_char {
    let build = || -> Result<*mut c_char, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        let mut json = String::from("[");
        for (index, held) in object.foreign_settings().iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push('{');
            push_json_text(&mut json, "key");
            json.push(':');
            push_json_text(&mut json, held.key());
            json.push(',');
            push_json_text(&mut json, "value");
            json.push(':');
            push_json_text(&mut json, held.value());
            json.push(',');
            push_json_text(&mut json, "reason");
            json.push(':');
            push_json_text(&mut json, held.reason().name());
            json.push(',');
            push_json_text(&mut json, "note");
            json.push(':');
            push_json_text(&mut json, held.reason().note());
            json.push('}');
        }
        json.push(']');
        CString::new(json)
            .map(CString::into_raw)
            .map_err(|_| "the foreign settings JSON contains a NUL".to_owned())
    };
    report(build(), ptr::null_mut())
}

/// Moves object `id` on the plate without touching its triangles.
///
/// Returns 0 on success and -1 when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_placement(
    plate: *mut HfwPlate,
    id: u64,
    transform: HfwTransform,
) -> c_int {
    let place = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.set_placement(MeshTransform {
            scale: transform.scale,
            rotation_degrees: transform.rotation_degrees,
            translation_mm: transform.translation_mm,
        });
        Ok(0)
    };
    report(place(), -1)
}

/// Moves object `id` from a row-major 4x4 matrix.
///
/// `matrix` holds sixteen doubles and composes the way this engine does: scale,
/// then rotations about X, Y and Z in degrees, then translation in millimetres.
/// A matrix that also shears, collapses an axis or carries a perspective row is
/// refused rather than truncated, because the plate records a scale and an Euler
/// triple and cannot carry anything else.
///
/// Returns 0 on success and -1 when the object is not on the plate, either pointer
/// is null, or the matrix is not representable.
///
/// # Safety
///
/// `matrix` must point at sixteen readable doubles and `plate` must be a live
/// handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_placement_matrix(
    plate: *mut HfwPlate,
    id: u64,
    matrix: *const f64,
) -> c_int {
    let place = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        if matrix.is_null() {
            return Err("matrix pointer is null".to_owned());
        }
        let mut values = [0.0_f64; 16];
        for (slot, value) in values
            .iter_mut()
            .zip(unsafe { std::slice::from_raw_parts(matrix, 16) })
        {
            *slot = *value;
        }
        let transform = MeshTransform::from_matrix(values).map_err(|error| error.to_string())?;
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.set_placement(transform);
        Ok(0)
    };
    report(place(), -1)
}

/// Folds object `id`'s placement into its geometry and resets the placement.
///
/// Returns 0 on success and -1 when the object is not on the plate or the
/// placement cannot be applied.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_bake(plate: *mut HfwPlate, id: u64) -> c_int {
    let bake = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.bake_placement().map_err(|error| error.to_string())?;
        Ok(0)
    };
    report(bake(), -1)
}

/// Includes object `id` in the job, or leaves it out.
///
/// Returns 0 on success and -1 when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_enabled(
    plate: *mut HfwPlate,
    id: u64,
    enabled: c_int,
) -> c_int {
    let set = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        object.set_enabled(enabled != 0);
        Ok(0)
    };
    report(set(), -1)
}

/// Removes object `id` from the plate.
///
/// Returns 0 on success and -1 when the object is not on the plate. Every other
/// object keeps its id.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_remove(plate: *mut HfwPlate, id: u64) -> c_int {
    let remove = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        unsafe { &mut *plate }
            .plate
            .remove(id)
            .map(|_| 0)
            .ok_or_else(|| format!("no object {id} is on the plate"))
    };
    report(remove(), -1)
}

/// Sets one setting on one object, by name and value as a panel states it.
///
/// The object takes the plate's settings the first time a key reaches it, so the
/// override is a delta from the job rather than a fresh set of defaults, and
/// order does not matter: keys may arrive in any sequence. Returns 0 on success
/// and -1 when the object is not on the plate or the engine cannot apply that key
/// or value, in which case `hfw_last_error` names the key and what it accepts.
///
/// The names accepted are the ones `hfw_settings_surface_json` marks
/// `per_object`, in either the engine's spelling or the dialect's.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call, and `key` and `value`
/// must be NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_set(
    plate: *mut HfwPlate,
    id: u64,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    let set = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let key = unsafe { text_of(key, "key") }?;
        let value = unsafe { text_of(value, "value") }?;
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        unsafe { &mut *plate }
            .plate
            .set_object_setting(id, &key, &value)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(set(), -1)
}

/// Drops object `id`'s own settings, so it prints with the plate's again.
///
/// Returns 0 on success and -1 when the object is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_clear_settings(plate: *mut HfwPlate, id: u64) -> c_int {
    let clear = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        unsafe { &mut *plate }
            .plate
            .clear_object_settings(id)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(clear(), -1)
}

/// Writes one paint channel onto object `id`.
///
/// `states` points at `len` bytes, one per triangle of the object's geometry, each
/// of them 0 (none), 1 (enforcer) or 2 (blocker); a `len` of zero clears the
/// channel. The states are indexed in the mesh's own triangle order, which
/// placement, rotation and baking all preserve. Returns 0 on success and -1 when
/// the object is not on the plate, the length is not the triangle count, or a
/// state is not one of the three.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call, and `states` must point at
/// `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_paint(
    plate: *mut HfwPlate,
    id: u64,
    channel: c_int,
    states: *const u8,
    len: usize,
) -> c_int {
    let set = || -> Result<c_int, String> {
        if channel != HFW_PAINT_SUPPORT && channel != HFW_PAINT_SEAM {
            return Err(format!("paint channel {channel} does not exist"));
        }
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let states = unsafe { paint_states(states, len)? };
        let object = unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        let mut paint = object.paint().clone();
        paint.support = states;
        object
            .set_paint(paint)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(set(), -1)
}

/// Reads one paint channel back off object `id`.
///
/// `out` receives one byte per triangle, in the object's mesh order, and `len`
/// states how many bytes `out` can hold. Returns the number of bytes written on
/// success, and -1 when the object is not on the plate, the channel does not exist,
/// or the buffer is too small.
///
/// # Safety
///
/// `plate` must be a live handle, and `out` must point at `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_paint_get(
    plate: *const HfwPlate,
    id: u64,
    channel: c_int,
    out: *mut u8,
    len: usize,
) -> isize {
    let get = || -> Result<isize, String> {
        if channel != HFW_PAINT_SUPPORT && channel != HFW_PAINT_SEAM {
            return Err(format!("paint channel {channel} does not exist"));
        }
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &*plate }
            .plate
            .object(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        let states = if channel == HFW_PAINT_SEAM {
            &object.paint().seam
        } else {
            &object.paint().support
        };
        if states.is_empty() {
            return Ok(0);
        }
        if states.len() > len || out.is_null() {
            return Err(format!(
                "{len} bytes were offered for {} triangles",
                states.len()
            ));
        }
        unsafe { std::ptr::copy_nonoverlapping(states.as_ptr(), out, states.len()) };
        Ok(states.len() as isize)
    };
    report(get(), -1)
}

/// Writes object `id`'s bounds on the bed, in millimetres, into `out`.
///
/// Returns 0 on success and -1 when the object is not on the plate or has no
/// geometry.
///
/// # Safety
///
/// `plate` must be a live handle, and `out` must be writable for one `HfwBounds`.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_bounds(
    plate: *const HfwPlate,
    id: u64,
    out: *mut HfwBounds,
) -> c_int {
    let read = || -> Result<c_int, String> {
        let plate = unsafe { plate_of(plate) }?;
        if out.is_null() {
            return Err("out is null".to_owned());
        }
        let object = object_of(&plate.plate, id)?;
        let (x0, y0, x1, y1) = object.bounds().map_err(|error| error.to_string())?;
        let (z0, z1) = object.z_bounds().map_err(|error| error.to_string())?;
        unsafe { out.write(bounds_from(x0, y0, z0, x1, y1, z1)) };
        Ok(0)
    };
    report(read(), -1)
}

/// Writes the bounds of every object on the plate, in millimetres, into `out`.
///
/// Returns 0 on success and -1 when the plate holds no geometry.
///
/// # Safety
///
/// `plate` must be a live handle, and `out` must be writable for one `HfwBounds`.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bounds(plate: *const HfwPlate, out: *mut HfwBounds) -> c_int {
    let read = || -> Result<c_int, String> {
        let plate = unsafe { plate_of(plate) }?;
        if out.is_null() {
            return Err("out is null".to_owned());
        }
        let (x0, y0, x1, y1) = plate
            .plate
            .bounds()
            .ok_or_else(|| "the plate holds no object with geometry".to_owned())?;
        let (z0, z1) = plate
            .plate
            .z_bounds()
            .ok_or_else(|| "the plate holds no object with geometry".to_owned())?;
        unsafe { out.write(bounds_from(x0, y0, z0, x1, y1, z1)) };
        Ok(0)
    };
    report(read(), -1)
}

/// Slices the plate and returns G-code for it.
///
/// The profile's process settings are layered onto the plate's for this job
/// without being stored, so slicing with one profile and then another does not
/// leave the second on the document. An object that carries its own settings keeps
/// them. `profile` names a printer profile; both layer heights are in millimetres,
/// and a value of zero or less takes the profile's own. Meshes are sliced where
/// they are: the file keeps the coordinates the plate holds.
///
/// # Safety
///
/// `plate` must be a live handle and `profile` must be a NUL-terminated string. The
/// returned handle must be released with [`hfw_gcode_free`], and never used after
/// it.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_slice(
    plate: *const HfwPlate,
    profile: *const c_char,
    first_layer_height: f64,
    layer_height: f64,
) -> *mut HfwGcode {
    let build = || -> Result<*mut HfwGcode, String> {
        let plate = unsafe { plate_of(plate) }?;
        let profile = unsafe { text_of(profile, "profile") }?;
        slice_document(
            &plate.plate,
            &profile,
            first_layer_height,
            layer_height,
            true,
            None,
        )
        .map(|gcode| Box::into_raw(Box::new(gcode)))
    };
    report(build(), ptr::null_mut())
}

/// Slices the plate and places it on the bed before writing it.
///
/// The same call as [`hfw_plate_slice`], except that the plate is laid on the bed
/// the profile describes instead of printed where it sits: the file keeps the
/// plate's own coordinates in that call and this one moves it, which is what
/// `hfw-cli --place` does. A plate that came from a document was placed by whoever
/// laid it out and wants [`hfw_plate_slice`]; bare geometry with no document
/// around it has no position to keep, and wants this.
///
/// # Safety
///
/// `plate` must be a live handle and `profile` must be a NUL-terminated string. The
/// returned handle must be released with [`hfw_gcode_free`], and never used after
/// it.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_slice_placed(
    plate: *const HfwPlate,
    profile: *const c_char,
    first_layer_height: f64,
    layer_height: f64,
) -> *mut HfwGcode {
    let build = || -> Result<*mut HfwGcode, String> {
        let plate = unsafe { plate_of(plate) }?;
        let profile = unsafe { text_of(profile, "profile") }?;
        slice_document(
            &plate.plate,
            &profile,
            first_layer_height,
            layer_height,
            false,
            None,
        )
        .map(|gcode| Box::into_raw(Box::new(gcode)))
    };
    report(build(), ptr::null_mut())
}

/// Releases a plate handle, and with it every object on it. A null handle is
/// ignored.
///
/// # Safety
///
/// `plate` must be a handle returned by [`hfw_plate_new`] that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_free(plate: *mut HfwPlate) {
    if plate.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(plate) });
}

/// Takes the plate as it is now, and returns it as a handle.
///
/// This is what an undo step is: the whole document -- every object with its id,
/// geometry, placement, paint, settings, volumes and layer heights, the timeline, the
/// in-layer strokes, the caller's own bytes and the counter that hands out the next
/// id -- kept so it can be put back exactly. Taking one copies no triangles: the
/// snapshot shares each object's geometry with the plate, and the first write to an
/// object after this copies that object's geometry and leaves the snapshot holding
/// what it was taken with. A stack of snapshots therefore costs the document plus
/// every edit made since the first of them, not one document per step.
///
/// The snapshot owns its shares: freeing the plate leaves it intact, and it can be
/// restored into any plate. Release it with [`hfw_snapshot_free`]. Returns NULL when
/// `plate` is null.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_snapshot(plate: *const HfwPlate) -> *mut HfwSnapshot {
    let build = || -> Result<*mut HfwSnapshot, String> {
        let plate = unsafe { plate_of(plate) }?;
        Ok(Box::into_raw(Box::new(HfwSnapshot {
            snapshot: plate.plate.snapshot(),
        })))
    };
    report(build(), ptr::null_mut())
}

/// Replaces everything `plate` holds with everything `snapshot` holds.
///
/// The snapshot is not consumed: the same one restores again, which is what redo is.
/// A document that is restored prints, reads back and reopens as the one that was
/// snapshotted, ids included, so a selection or a paint session holding an id is
/// holding the object it named. Returns 0 on success, -1 when either handle is null.
///
/// # Safety
///
/// Both handles must be live, and `plate` writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_restore(
    plate: *mut HfwPlate,
    snapshot: *const HfwSnapshot,
) -> i32 {
    let restore = || -> Result<i32, String> {
        if snapshot.is_null() {
            return Err("snapshot handle is null".to_owned());
        }
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let snapshot = unsafe { &*snapshot };
        let plate = unsafe { &mut *plate };
        plate.plate.restore(&snapshot.snapshot);
        Ok(0)
    };
    report(restore(), -1)
}

/// Releases a snapshot handle. A null handle is ignored.
///
/// # Safety
///
/// `snapshot` must be a handle returned by [`hfw_plate_snapshot`] that has not been
/// freed.
#[no_mangle]
pub unsafe extern "C" fn hfw_snapshot_free(snapshot: *mut HfwSnapshot) {
    if snapshot.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(snapshot) });
}

/// How many objects a snapshot holds, or -1 when the handle is null.
///
/// # Safety
///
/// `snapshot` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_snapshot_object_count(snapshot: *const HfwSnapshot) -> isize {
    if snapshot.is_null() {
        set_error("snapshot handle is null");
        return -1;
    }
    clear_error();
    unsafe { &*snapshot }.snapshot.object_count() as isize
}

/// How many build plates the document holds, which is never zero, or -1 when the
/// handle is null.
///
/// A plate is an index a front end can hold: adding one appends, removing one moves
/// the plates after it down, and the objects on it come with the index rather than
/// with a handle.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_count(plate: *const HfwPlate) -> isize {
    let read = || -> Result<isize, String> {
        let plate = unsafe { plate_of(plate) }?;
        Ok(plate.plate.bed_count() as isize)
    };
    report(read(), -1)
}

/// Appends an empty, unnamed, unlocked plate and returns its index, or -1 on error.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_add(plate: *mut HfwPlate) -> isize {
    let add = || -> Result<isize, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        Ok(unsafe { &mut *plate }.plate.add_bed() as isize)
    };
    report(add(), -1)
}

/// Removes the plate at `bed`, and everything on it.
///
/// The last plate cannot be removed and a locked plate cannot be, because neither
/// leaves a document a front end can keep drawing. The plates after it move down
/// one, so an object on plate 3 is on plate 2 afterwards; every id stays what it was
/// and the removed objects' ids are retired rather than reused. Returns 0 on success,
/// -1 with the reason otherwise.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_remove(plate: *mut HfwPlate, bed: usize) -> i32 {
    let remove = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        unsafe { &mut *plate }
            .plate
            .remove_bed(bed)
            .map_err(|error| error.to_string())?;
        Ok(0)
    };
    report(remove(), -1)
}

/// Names the plate at `bed`.
///
/// An empty name clears it, so the front end's own wording ("Plate 2") applies
/// again. Returns 0 on success, -1 when the document has no such plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call and `name` a readable string.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_name(
    plate: *mut HfwPlate,
    bed: usize,
    name: *const c_char,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let name = unsafe { text_of(name, "name") }?;
        let plate = unsafe { &mut *plate };
        let bed = plate
            .plate
            .bed_mut(bed)
            .ok_or_else(|| format!("the document has no plate {}", bed + 1))?;
        bed.set_name(name);
        Ok(0)
    };
    report(set(), -1)
}

/// Writes the plate's own name into `out`, and returns the bytes it needs excluding
/// the terminating NUL.
///
/// An unnamed plate reports 0 and writes an empty string: the wording a user reads
/// for a plate with no name of its own belongs to the front end. A buffer that is too
/// short writes nothing and reports the length, so a caller can size and retry.
/// Returns -1 when the document has no such plate.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for `len` bytes, or null.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_name_get(
    plate: *const HfwPlate,
    bed: usize,
    out: *mut c_char,
    len: usize,
) -> isize {
    let read = || -> Result<isize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let held = plate
            .plate
            .bed(bed)
            .ok_or_else(|| format!("the document has no plate {}", bed + 1))?;
        let name = held.name();
        let needed = name.len();
        if out.is_null() || len < needed + 1 {
            return Ok(needed as isize);
        }
        let buffer = unsafe { std::slice::from_raw_parts_mut(out as *mut u8, len) };
        buffer[..needed].copy_from_slice(name.as_bytes());
        buffer[needed] = 0;
        Ok(needed as isize)
    };
    report(read(), -1)
}

/// Returns 1 when the plate at `bed` is locked, 0 when it is not, and -1 when the
/// document has no such plate.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_locked(plate: *const HfwPlate, bed: usize) -> i32 {
    let read = || -> Result<i32, String> {
        let plate = unsafe { plate_of(plate) }?;
        let held = plate
            .plate
            .bed(bed)
            .ok_or_else(|| format!("the document has no plate {}", bed + 1))?;
        Ok(i32::from(held.locked()))
    };
    report(read(), -1)
}

/// Locks or unlocks the plate at `bed`.
///
/// A lock is data here: the engine enforces it where it is the one changing the
/// document's shape -- removing the plate, moving an object onto or off it, adding to
/// it -- and a front end disables the rest of the plate's gestures itself. Returns 0
/// on success, -1 when the document has no such plate.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_set_locked(
    plate: *mut HfwPlate,
    bed: usize,
    locked: c_int,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let plate = unsafe { &mut *plate };
        let held = plate
            .plate
            .bed_mut(bed)
            .ok_or_else(|| format!("the document has no plate {}", bed + 1))?;
        held.set_locked(locked != 0);
        Ok(0)
    };
    report(set(), -1)
}

/// Adds a copy of `mesh` to the plate at `bed`, named after the document's object
/// count, and returns the new object's id or 0 on failure.
///
/// A locked plate refuses, since adding to it changes it. The mesh stays the caller's
/// to free.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call and `mesh` a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_add_on_bed(
    plate: *mut HfwPlate,
    bed: usize,
    mesh: *const HfwMesh,
) -> u64 {
    let add = || -> Result<u64, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let source = unsafe { mesh_of(mesh) }?;
        let plate = unsafe { &mut *plate };
        let name = format!("object {}", plate.plate.len() + 1);
        let id = plate
            .plate
            .add_on_bed(bed, name, source.mesh.clone())
            .map_err(|error| error.to_string())?;
        Ok(id.get())
    };
    report(add(), 0)
}

/// Which plate object `id` is on, or -1 when the object is not in the document.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_bed(plate: *const HfwPlate, id: u64) -> isize {
    let read = || -> Result<isize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let object = object_of(&plate.plate, id)?;
        Ok(object.bed() as isize)
    };
    report(read(), -1)
}

/// Moves object `id` to the plate at `bed`.
///
/// The object keeps its id, its geometry, its placement, its paint and its overrides:
/// an id means one object in the document, not one object per plate, so a selection or
/// an undo step holding it still names it after the move. Both plates are checked, so
/// moving off a locked plate and moving onto one are both refused. Returns 0 on
/// success, -1 with the reason.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_set_bed(
    plate: *mut HfwPlate,
    id: u64,
    bed: usize,
) -> i32 {
    let set = || -> Result<i32, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        unsafe { &mut *plate }
            .plate
            .set_object_bed(id, bed)
            .map_err(|error| error.to_string())?;
        Ok(0)
    };
    report(set(), -1)
}

/// Writes the ids of the objects on the plate at `bed` into `out`, in the order they
/// were added, and returns how many the plate carries.
///
/// One call with a NULL `out` sizes the next. Returns 0 when the document has no such
/// plate, which cannot be told from an empty plate by the count alone; ask
/// `hfw_plate_bed_count` first.
///
/// # Safety
///
/// `plate` must be a live handle and `out` writable for `len` ids, or null.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_object_ids(
    plate: *const HfwPlate,
    bed: usize,
    out: *mut u64,
    len: usize,
) -> usize {
    let read = |()| -> Result<usize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let ids: Vec<u64> = plate
            .plate
            .objects_on(bed)
            .map(|object| object.id().get())
            .collect();
        if !out.is_null() && len >= ids.len() {
            let buffer = unsafe { std::slice::from_raw_parts_mut(out, len) };
            buffer[..ids.len()].copy_from_slice(&ids);
        }
        Ok(ids.len())
    };
    report(read(()), 0)
}

/// Slices the enabled objects on the plate at `bed`, with `profile` as the job's
/// settings.
///
/// Release the result with `hfw_gcode_free`. Returns NULL with the reason on failure,
/// including when the document has no such plate and when the plate has nothing
/// enabled on it.
///
/// # Safety
///
/// `plate` must be a live handle and `profile` a readable path.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_bed_slice(
    plate: *const HfwPlate,
    bed: usize,
    profile: *const c_char,
    first_layer_height: f64,
    layer_height: f64,
) -> *mut HfwGcode {
    let build = || -> Result<*mut HfwGcode, String> {
        let plate = unsafe { plate_of(plate) }?;
        let profile = unsafe { text_of(profile, "profile") }?;
        slice_document(
            &plate.plate,
            &profile,
            first_layer_height,
            layer_height,
            true,
            Some(bed),
        )
        .map(|gcode| Box::into_raw(Box::new(gcode)))
    };
    report(build(), ptr::null_mut())
}

/// Returns the G-code text, NUL-terminated.
///
/// The pointer is owned by the handle and stays valid until [`hfw_gcode_free`].
///
/// # Safety
///
/// `gcode` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_gcode_text(gcode: *const HfwGcode) -> *const c_char {
    if gcode.is_null() {
        set_error("gcode handle is null");
        return ptr::null();
    }
    let owned = unsafe { &*gcode };
    clear_error();
    owned.text.as_ptr()
}

/// Returns the G-code length in bytes, excluding the terminating NUL.
///
/// # Safety
///
/// `gcode` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_gcode_len(gcode: *const HfwGcode) -> usize {
    if gcode.is_null() {
        set_error("gcode handle is null");
        return 0;
    }
    let owned = unsafe { &*gcode };
    clear_error();
    owned.bytes
}

/// Returns 1 when the engine's independent verifier passed the G-code, 0 when it
/// found an error, and 0 for a null handle.
///
/// # Safety
///
/// `gcode` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_gcode_verified(gcode: *const HfwGcode) -> c_int {
    if gcode.is_null() {
        set_error("gcode handle is null");
        return 0;
    }
    let owned = unsafe { &*gcode };
    clear_error();
    c_int::from(owned.verified)
}

/// Releases a G-code handle. A null handle is ignored.
///
/// # Safety
///
/// `gcode` must be a handle returned by this library that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn hfw_gcode_free(gcode: *mut HfwGcode) {
    if gcode.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(gcode) });
}

/// A volume that adds material to its object.
/// A marker that writes its own G-code at its height.
pub const HFW_MARKER_CUSTOM: c_int = 0;
/// A marker that stops the print, with the profile's `pause_print_gcode`.
pub const HFW_MARKER_PAUSE: c_int = 1;
/// A marker that changes filament, with the profile's `color_change_gcode`.
pub const HFW_MARKER_COLOUR: c_int = 2;

/// A volume that adds material to its object.
pub const HFW_VOLUME_PART: c_int = 0;
/// A volume that removes material from its object.
pub const HFW_VOLUME_NEGATIVE: c_int = 1;
/// A volume support may not enter.
pub const HFW_VOLUME_SUPPORT_BLOCKER: c_int = 2;
/// A volume support has to reach.
pub const HFW_VOLUME_SUPPORT_ENFORCER: c_int = 3;
/// A volume that prints the part of the object it covers with its own settings.
pub const HFW_VOLUME_MODIFIER: c_int = 4;

fn volume_kind(kind: c_int) -> Result<VolumeKind, String> {
    match kind {
        HFW_VOLUME_PART => Ok(VolumeKind::Part),
        HFW_VOLUME_NEGATIVE => Ok(VolumeKind::Negative),
        HFW_VOLUME_SUPPORT_BLOCKER => Ok(VolumeKind::SupportBlocker),
        HFW_VOLUME_SUPPORT_ENFORCER => Ok(VolumeKind::SupportEnforcer),
        HFW_VOLUME_MODIFIER => Ok(VolumeKind::Modifier),
        other => Err(format!(
            "volume kind {other} does not exist: it is one of 0 part, 1 negative, 2 support \
             blocker, 3 support enforcer or 4 modifier"
        )),
    }
}

fn volume_code(kind: VolumeKind) -> c_int {
    match kind {
        VolumeKind::Part => HFW_VOLUME_PART,
        VolumeKind::Negative => HFW_VOLUME_NEGATIVE,
        VolumeKind::SupportBlocker => HFW_VOLUME_SUPPORT_BLOCKER,
        VolumeKind::SupportEnforcer => HFW_VOLUME_SUPPORT_ENFORCER,
        VolumeKind::Modifier => HFW_VOLUME_MODIFIER,
    }
}

/// One volume of one object, by the id the caller was handed when it was attached.
fn volume_of(
    object: &PlateObject,
    object_id: u64,
    volume: u64,
) -> Result<&hfw_plate::PlateVolume, String> {
    let id = volume_id(volume)?;
    object.volume(id).ok_or_else(|| {
        format!(
            "object {object_id} has no volume {id}: it has {}",
            object.volumes().len()
        )
    })
}

fn volume_id(volume: u64) -> Result<VolumeId, String> {
    VolumeId::from_number(volume)
        .ok_or_else(|| format!("volume id {volume} is not a volume: ids are never zero"))
}

fn id_from_number(id: u64) -> Result<ObjectId, String> {
    ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())
}

/// Writes `text` into a caller's buffer and reports the bytes it needs.
///
/// A buffer that is too short writes nothing and reports the length, so a caller can
/// size the call and retry rather than truncating a name.
unsafe fn write_text(text: &str, out: *mut c_char, len: usize) -> Result<isize, String> {
    let needed = text.len();
    if out.is_null() || len < needed + 1 {
        return Ok(needed as isize);
    }
    let buffer = unsafe { std::slice::from_raw_parts_mut(out as *mut u8, len) };
    buffer[..needed].copy_from_slice(text.as_bytes());
    buffer[needed] = 0;
    Ok(needed as isize)
}

fn dialect_number(dialect: Dialect) -> i32 {
    match dialect {
        Dialect::Engine => HFW_DIALECT_ENGINE,
        Dialect::Orca => HFW_DIALECT_ORCA,
    }
}

fn dialect_of(number: i32) -> Result<Dialect, String> {
    match number {
        HFW_DIALECT_ENGINE => Ok(Dialect::Engine),
        HFW_DIALECT_ORCA => Ok(Dialect::Orca),
        other => Err(format!(
            "dialect {other} is not one this engine knows: {HFW_DIALECT_ENGINE} is its own \
             names and {HFW_DIALECT_ORCA} is Bambu Studio or OrcaSlicer's"
        )),
    }
}

fn refusal_of(number: i32) -> Result<ForeignRefusal, String> {
    match number {
        HFW_FOREIGN_UNKNOWN_NAME => Ok(ForeignRefusal::UnknownName),
        HFW_FOREIGN_UNUSABLE_VALUE => Ok(ForeignRefusal::UnusableValue),
        other => Err(format!(
            "reason {other} is not one of {HFW_FOREIGN_UNKNOWN_NAME} (no such name) or \
             {HFW_FOREIGN_UNUSABLE_VALUE} (a value this engine refuses)"
        )),
    }
}

/// Reads a row-major 4x4 matrix a caller passed, as this engine's transform.
///
/// # Safety
///
/// `matrix` must point at sixteen readable doubles.
unsafe fn matrix_of(matrix: *const f64) -> Result<MeshTransform, String> {
    if matrix.is_null() {
        return Err("matrix pointer is null".to_owned());
    }
    let mut values = [0.0_f64; 16];
    for (slot, value) in values
        .iter_mut()
        .zip(unsafe { std::slice::from_raw_parts(matrix, 16) })
    {
        *slot = *value;
    }
    MeshTransform::from_matrix(values).map_err(|error| error.to_string())
}

/// Attaches a volume to object `id`, at its geometry's own coordinates.
///
/// The volume shares the object's placement, so moving the part moves what it cuts
/// and what it forbids support in. `kind` is one of `HFW_VOLUME_PART`,
/// `HFW_VOLUME_NEGATIVE`, `HFW_VOLUME_SUPPORT_BLOCKER`, `HFW_VOLUME_SUPPORT_ENFORCER`
/// or `HFW_VOLUME_MODIFIER`; the mesh is copied, so the caller keeps its own.
///
/// Returns the volume's id, which is never zero and never reused, or 0 when the
/// object is not on the plate, `mesh` is null, the geometry has no triangles, or
/// `kind` is not one of the five. Use the id, not a position: removing a volume does
/// not renumber the ones after it, and a position does.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call and `mesh` a live read-only
/// handle for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_add_volume(
    plate: *mut HfwPlate,
    id: u64,
    kind: c_int,
    mesh: *const HfwMesh,
) -> u64 {
    unsafe { hfw_plate_object_add_volume_placed(plate, id, kind, mesh, std::ptr::null()) }
}

/// Attaches a volume to object `id` and places it inside the object.
///
/// `matrix` is a row-major 4x4 affine matrix, the shape
/// [`hfw_plate_object_placement_matrix`] takes, and it composes on top of the
/// object's own placement: a volume follows its part when the part moves and can be
/// moved, turned or scaled within it without rewriting any triangles. A matrix that
/// shears, collapses an axis or is not affine is refused rather than truncated.
///
/// Returns the volume's id, or 0 on the same refusals [`hfw_plate_object_add_volume`]
/// reports plus an unrepresentable matrix.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call, `mesh` a live read-only
/// handle, and `matrix` a null pointer or sixteen readable doubles.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_add_volume_placed(
    plate: *mut HfwPlate,
    id: u64,
    kind: c_int,
    mesh: *const HfwMesh,
    matrix: *const f64,
) -> u64 {
    let add = || -> Result<u64, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        if mesh.is_null() {
            return Err("mesh handle is null".to_owned());
        }
        let kind = volume_kind(kind)?;
        let geometry = unsafe { &*mesh }.mesh.clone();
        let placement = if matrix.is_null() {
            MeshTransform::default()
        } else {
            unsafe { matrix_of(matrix) }?
        };
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?
            .add_volume_placed(kind, geometry, placement)
            .map(VolumeId::get)
            .map_err(|error| error.to_string())
    };
    report(add(), 0)
}

/// Moves volume `volume` inside its object, from a row-major 4x4 matrix.
///
/// The matrix has the same shape and rules as
/// [`hfw_plate_object_placement_matrix`]: scale, then rotations about X, Y and Z in
/// degrees, then translation in millimetres. Returns 0 on success and -1 when the
/// object or volume does not exist or the matrix is not representable.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call and `matrix` must point at
/// sixteen readable doubles.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_placement(
    plate: *mut HfwPlate,
    id: u64,
    volume: u64,
    matrix: *const f64,
) -> c_int {
    let place = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let placement = unsafe { matrix_of(matrix) }?;
        let object_id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let volume_id = VolumeId::from_number(volume)
            .ok_or_else(|| format!("volume id {volume} is not a volume: ids are never zero"))?;
        unsafe { &mut *plate }
            .plate
            .object_mut(object_id)
            .ok_or_else(|| format!("no object {object_id} is on the plate"))?
            .set_volume_placement(volume_id, placement)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(place(), -1)
}

/// Sets one setting on modifier volume `volume` of object `id`.
///
/// Only a modifier volume has a region of its own to print differently, so a key set on
/// any other kind is refused rather than stored and ignored. The key is a settings key,
/// the same one an object override uses, and it is applied on top of the object's
/// settings and of whatever the volume already carries.
///
/// Returns 0 on success and -1 when the object or volume does not exist, the volume is
/// not a modifier, or the key or value is not a setting this engine knows.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call, and `key`/`value` live C
/// strings.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_volume_set(
    plate: *mut HfwPlate,
    id: u64,
    volume: u64,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    let set = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let key = unsafe { text_of(key, "key") }?;
        let value = unsafe { text_of(value, "value") }?;
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let volume = VolumeId::from_number(volume)
            .ok_or_else(|| "volume id 0 is not a volume".to_owned())?;
        let plate = unsafe { &mut *plate };
        let settings = plate.plate.settings().clone();
        plate
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?
            .set_volume_setting(volume, &key, &value, &settings)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(set(), -1)
}

/// Detaches volume `volume` from object `id`. Every other volume keeps its id.
///
/// Returns 0 on success and -1 when the object is not on the plate or it has no
/// volume with that id.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_remove_volume(
    plate: *mut HfwPlate,
    id: u64,
    volume: u64,
) -> c_int {
    let remove = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let volume = VolumeId::from_number(volume)
            .ok_or_else(|| "volume id 0 is not a volume".to_owned())?;
        unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?
            .remove_volume(volume)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(remove(), -1)
}

/// Writes every volume id of object `id` into `out`, in the order they were added.
///
/// Returns how many the object carries, so one call with a null `out` sizes the next.
/// A count larger than `len` fills `len` of them and still reports the total.
///
/// # Safety
///
/// `plate` must be a live handle and `out` must be null or writable for `len` ids.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_ids(
    plate: *const HfwPlate,
    id: u64,
    out: *mut u64,
    len: usize,
) -> usize {
    let read = || -> Result<usize, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &*plate }
            .plate
            .object(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        let ids = object.volume_ids();
        if !out.is_null() {
            for (slot, id) in ids.iter().take(len).enumerate() {
                unsafe { *out.add(slot) = id.get() };
            }
        }
        Ok(ids.len())
    };
    report(read(), 0)
}

/// How many volumes object `id` carries, or -1 when it is not on the plate.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_count(plate: *const HfwPlate, id: u64) -> isize {
    let count = || -> Result<isize, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &*plate }
            .plate
            .object(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        Ok(object.volumes().len() as isize)
    };
    report(count(), -1)
}

/// Writes which kind of volume `volume` of object `id` is into `out`.
///
/// Returns 0 on success and -1 when the object is not on the plate, `out` is null,
/// or the object has no volume with that id.
///
/// # Safety
///
/// `plate` must be a live handle and `out` must be writable for one `int32_t`.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_volume_kind(
    plate: *const HfwPlate,
    id: u64,
    volume: u64,
    out: *mut c_int,
) -> c_int {
    let kind = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        if out.is_null() {
            return Err("output pointer is null".to_owned());
        }
        let object_id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &*plate }
            .plate
            .object(object_id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        let volume = volume_of(object, id, volume)?;
        unsafe { *out = volume_code(volume.kind()) };
        Ok(0)
    };
    report(kind(), -1)
}

/// Sets object `id`'s own layer heights, as `count` interleaved `(z, height)` pairs
/// of millimetres.
///
/// A layer whose bottom sits at or above a `z` uses that height until the next pair
/// takes over; a `count` of zero clears the object back to the plate's uniform
/// layer height. The pairs are checked here for a rising `z` and a positive height,
/// and against the profile's thinnest and thickest allowed step when the object is
/// sliced. Two objects on one plate may step differently: the file is interleaved on
/// Z, so a layer change happens when either object's next layer is higher.
///
/// Returns 0 on success and -1 when the object is not on the plate, the pairs do not
/// rise, or a height is not positive.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call and `pairs` must point at
/// `count * 2` readable doubles.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_layer_heights(
    plate: *mut HfwPlate,
    id: u64,
    pairs: *const f64,
    count: usize,
) -> c_int {
    let set = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        if count > 0 && pairs.is_null() {
            return Err("layer height pointer is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let flat = if count == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(pairs, count * 2) }.to_vec()
        };
        let heights: Vec<(f64, f64)> = flat
            .chunks_exact(2)
            .map(|pair| (pair[0], pair[1]))
            .collect();
        unsafe { &mut *plate }
            .plate
            .object_mut(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?
            .set_layer_heights(heights)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(set(), -1)
}

/// Reads object `id`'s own layer heights back as interleaved `(z, height)` pairs.
///
/// Returns the number of pairs on success, 0 when the object prints at the plate's
/// uniform height, and -1 when the object is not on the plate or `len` pairs do not
/// fit in `out`.
///
/// # Safety
///
/// `plate` must be a live handle and `out` must point at `len * 2` writable doubles.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_object_layer_heights_get(
    plate: *const HfwPlate,
    id: u64,
    out: *mut f64,
    len: usize,
) -> isize {
    let get = || -> Result<isize, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let id =
            ObjectId::from_number(id).ok_or_else(|| "object id 0 is not an object".to_owned())?;
        let object = unsafe { &*plate }
            .plate
            .object(id)
            .ok_or_else(|| format!("no object {id} is on the plate"))?;
        let heights = object.layer_heights();
        if heights.is_empty() {
            return Ok(0);
        }
        if out.is_null() || len < heights.len() {
            return Ok(heights.len() as isize);
        }
        for (index, (z, height)) in heights.iter().enumerate() {
            unsafe {
                *out.add(index * 2) = *z;
                *out.add(index * 2 + 1) = *height;
            }
        }
        Ok(heights.len() as isize)
    };
    report(get(), -1)
}

/// Sets the plate's timeline markers, sorted by Z.
///
/// `kinds` holds one [`HFW_MARKER_CUSTOM`]/`HFW_MARKER_PAUSE`/`HFW_MARKER_COLOUR` per
/// marker, `zs` its print heights in millimetres, and `texts` its G-code — read only
/// for `HFW_MARKER_CUSTOM`, where a marker with no G-code in it is refused rather
/// than written into the file doing nothing. `count` of zero clears the timeline.
///
/// Returns 0 on success and -1 when a kind is not one of the three, a Z is not a
/// finite height above the bed, a custom marker has no text, or a pointer is null.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call; `kinds`, `zs` and `texts` must
/// point at `count` readable elements each, with every `texts` entry a live C string.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_set_markers(
    plate: *mut HfwPlate,
    kinds: *const i32,
    zs: *const f64,
    texts: *const *const c_char,
    count: usize,
) -> c_int {
    let set = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        if count > 0 && (kinds.is_null() || zs.is_null() || texts.is_null()) {
            return Err("a marker array is null".to_owned());
        }
        let mut markers = Vec::with_capacity(count);
        for index in 0..count {
            let (kind, z, text) = unsafe { (*kinds.add(index), *zs.add(index), *texts.add(index)) };
            let kind = match kind {
                HFW_MARKER_CUSTOM => MarkerKind::Custom,
                HFW_MARKER_PAUSE => MarkerKind::Pause,
                HFW_MARKER_COLOUR => MarkerKind::ColourChange,
                other => {
                    return Err(format!(
                        "marker {index} has kind {other}, which is not one of \
                         {HFW_MARKER_CUSTOM} (custom), {HFW_MARKER_PAUSE} (pause) or \
                         {HFW_MARKER_COLOUR} (colour change)"
                    ))
                }
            };
            let text = if text.is_null() {
                String::new()
            } else {
                unsafe { std::ffi::CStr::from_ptr(text) }
                    .to_string_lossy()
                    .into_owned()
            };
            markers.push(Marker { z, kind, text });
        }
        unsafe { &mut *plate }
            .plate
            .set_markers(markers)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(set(), -1)
}

/// Number of markers on the plate's timeline, or -1 when the handle is null.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_marker_count(plate: *const HfwPlate) -> isize {
    let count = || -> Result<isize, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        Ok(unsafe { &*plate }.plate.markers().len() as isize)
    };
    report(count(), -1)
}

/// Reads marker `index` back: its kind, its Z, and its G-code.
///
/// `text` receives up to `len` bytes of the marker's G-code including the terminator,
/// truncated to fit. Returns 0 on success and -1 when the handle is null or the index
/// is past the end.
///
/// # Safety
///
/// `plate` must be a live handle; `kind` and `z` must be writable, and `text` must be
/// writable for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_marker_get(
    plate: *const HfwPlate,
    index: usize,
    kind: *mut i32,
    z: *mut f64,
    text: *mut c_char,
    len: usize,
) -> c_int {
    let read = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let plate = unsafe { &*plate };
        let marker = plate.plate.markers().get(index).ok_or_else(|| {
            format!(
                "the plate has {} markers, so marker {index} does not exist",
                plate.plate.markers().len()
            )
        })?;
        if !kind.is_null() {
            unsafe {
                *kind = match marker.kind {
                    MarkerKind::Custom => HFW_MARKER_CUSTOM,
                    MarkerKind::Pause => HFW_MARKER_PAUSE,
                    MarkerKind::ColourChange => HFW_MARKER_COLOUR,
                };
            }
        }
        if !z.is_null() {
            unsafe { *z = marker.z };
        }
        if !text.is_null() && len > 0 {
            let mut bytes = marker.text.as_bytes().to_vec();
            bytes.truncate(len - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), text.cast::<u8>(), bytes.len());
                *text.add(bytes.len()) = 0;
            }
        }
        Ok(0)
    };
    report(read(), -1)
}

/// Sets the plate's in-layer strokes: beads printed inside a layer that the model
/// does not describe.
///
/// This is the calibration channel — a temperature tower's labels, a flow test's
/// lines. `count` strokes are described by five parallel arrays: the print Z of the
/// layer each belongs to, the number of points it has, its line width, its layer
/// height and its speed. `points` holds every stroke's vertices back to back as
/// interleaved `x, y` plate coordinates, `starts` where each stroke's points begin in
/// `points`, and `prefixes` the G-code written before each stroke, verbatim (a
/// temperature band changes the nozzle temperature just before it draws). Any pointer
/// may be null for a `count` of zero, which clears the strokes.
///
/// Returns 0 on success and -1 when the counts do not agree, a stroke has fewer than
/// two points, a width, height or speed is not positive, or a Z is not a height above
/// the bed.
///
/// # Safety
///
/// `plate` must be a live handle writable for the call; every array must point at
/// `count` readable elements, `points` at twice the sum of `counts`, and every
/// `prefixes` entry a live C string.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_set_in_layer(
    plate: *mut HfwPlate,
    zs: *const f64,
    counts: *const usize,
    widths: *const f64,
    heights: *const f64,
    speeds: *const f64,
    starts: *const usize,
    points: *const f64,
    prefixes: *const *const c_char,
    count: usize,
) -> c_int {
    let set = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        if count > 0
            && (zs.is_null()
                || counts.is_null()
                || widths.is_null()
                || heights.is_null()
                || speeds.is_null()
                || starts.is_null()
                || prefixes.is_null())
        {
            return Err("an in-layer stroke array is null".to_owned());
        }
        let mut strokes = Vec::with_capacity(count);
        for index in 0..count {
            let (z, point_count, width, height, speed, start, prefix) = unsafe {
                (
                    *zs.add(index),
                    *counts.add(index),
                    *widths.add(index),
                    *heights.add(index),
                    *speeds.add(index),
                    *starts.add(index),
                    *prefixes.add(index),
                )
            };
            if point_count < 2 {
                return Err(format!(
                    "stroke {index} has {point_count} point(s), and a stroke needs at least two"
                ));
            }
            if points.is_null() {
                return Err(format!(
                    "stroke {index} has {point_count} points and the point array is null"
                ));
            }
            let mut stroke_points = Vec::with_capacity(point_count);
            for step in 0..point_count {
                let at = (start + step) * 2;
                stroke_points.push(hfw_engine::V2::new(unsafe { *points.add(at) }, unsafe {
                    *points.add(at + 1)
                }));
            }
            let prefix = if prefix.is_null() {
                String::new()
            } else {
                unsafe { std::ffi::CStr::from_ptr(prefix) }
                    .to_string_lossy()
                    .into_owned()
            };
            strokes.push(InLayerPolyline {
                z,
                points: stroke_points,
                line_width: width,
                layer_height: height,
                speed,
                prefix,
            });
        }
        unsafe { &mut *plate }
            .plate
            .set_in_layer_polylines(strokes)
            .map(|()| 0)
            .map_err(|error| error.to_string())
    };
    report(set(), -1)
}

/// Number of in-layer strokes on the plate, or -1 when the handle is null.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_in_layer_count(plate: *const HfwPlate) -> isize {
    let count = || -> Result<isize, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        Ok(unsafe { &*plate }.plate.in_layer_polylines().len() as isize)
    };
    report(count(), -1)
}

/// How many points stroke `index` has, or -1 when the handle is null or it has no
/// stroke at that index.
///
/// This is what sizes the buffer `hfw_plate_in_layer_get` fills.
///
/// # Safety
///
/// `plate` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_in_layer_points(plate: *const HfwPlate, index: usize) -> isize {
    let points = || -> Result<isize, String> {
        let plate = unsafe { plate_of(plate) }?;
        let stroke = plate.plate.in_layer_polylines().get(index).ok_or_else(|| {
            format!(
                "the plate has {} in-layer strokes, so stroke {index} does not exist",
                plate.plate.in_layer_polylines().len()
            )
        })?;
        Ok(stroke.points.len() as isize)
    };
    report(points(), -1)
}

/// Reads stroke `index` back: its Z, its point count, its width, height and speed.
///
/// `points` receives up to `capacity` points as interleaved `x, y` and `prefix`
/// receives up to `len` bytes of its G-code including the terminator, truncated to
/// fit. Either may be null to skip it. Returns 0 on success, -1 when the handle is
/// null, the index is past the end, or `points` is too small for the stroke.
///
/// # Safety
///
/// `plate` must be a live handle; every out pointer must be writable for what it
/// receives.
#[no_mangle]
pub unsafe extern "C" fn hfw_plate_in_layer_get(
    plate: *const HfwPlate,
    index: usize,
    z: *mut f64,
    width: *mut f64,
    height: *mut f64,
    speed: *mut f64,
    points: *mut f64,
    capacity: usize,
    prefix: *mut c_char,
    len: usize,
) -> c_int {
    let read = || -> Result<c_int, String> {
        if plate.is_null() {
            return Err("plate handle is null".to_owned());
        }
        let plate = unsafe { &*plate };
        let stroke = plate.plate.in_layer_polylines().get(index).ok_or_else(|| {
            format!(
                "the plate has {} in-layer strokes, so stroke {index} does not exist",
                plate.plate.in_layer_polylines().len()
            )
        })?;
        if !points.is_null() && capacity < stroke.points.len() {
            return Err(format!(
                "stroke {index} has {} points, more than the {capacity} asked for",
                stroke.points.len()
            ));
        }
        if !z.is_null() {
            unsafe { *z = stroke.z };
        }
        if !width.is_null() {
            unsafe { *width = stroke.line_width };
        }
        if !height.is_null() {
            unsafe { *height = stroke.layer_height };
        }
        if !speed.is_null() {
            unsafe { *speed = stroke.speed };
        }
        if !points.is_null() {
            for (step, point) in stroke.points.iter().enumerate() {
                unsafe {
                    *points.add(step * 2) = point.x;
                    *points.add(step * 2 + 1) = point.y;
                }
            }
        }
        if !prefix.is_null() && len > 0 {
            let mut bytes = stroke.prefix.as_bytes().to_vec();
            bytes.truncate(len - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), prefix.cast::<u8>(), bytes.len());
                *prefix.add(bytes.len()) = 0;
            }
        }
        Ok(0)
    };
    report(read(), -1)
}
