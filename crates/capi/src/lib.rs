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

use hfw_config::PrinterConfig;
use hfw_engine::{Job, Mesh, MeshTransform, Paint, PlateOrder, Settings};
use hfw_plate::{ObjectId, Plate, PlateObject};

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

fn bounds_from(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> HfwBounds {
    HfwBounds {
        min: [x0, y0, z0],
        max: [x1, y1, z1],
    }
}

/// Writes a planned job to a file, and asks the independent verifier about it.
pub(crate) fn write_job(job: &Job, profile: &PrinterConfig) -> Result<HfwGcode, String> {
    let mut write_cfg = profile.clone();
    write_cfg.keep_position = true;
    let written =
        hfw_gcode::generate_with_paint(&job.layers, &write_cfg, Some("plate"), &job.seam_paint)
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
    let objects: Vec<(&Mesh, Option<&Paint>, &Settings)> = meshes
        .iter()
        .map(|(mesh, paint)| (mesh, Some(paint), &settings))
        .collect();
    let job = hfw_plate::slice_placed(objects, first, layer, order_of(&cfg))
        .map_err(|error| error.to_string())?;
    write_job(&job, &cfg)
}

/// Plans the plate the way the command line would, without writing G-code.
fn plan_plate(
    plate: &Plate,
    profile: &str,
    first_layer_height: f64,
    layer_height: f64,
) -> Result<(Job, PrinterConfig), String> {
    let cfg = PrinterConfig::load(Path::new(profile)).map_err(|error| error.to_string())?;
    let mut settings = plate.settings().clone();
    settings.apply_printer(&cfg);
    let (first, layer) = heights_of(&cfg, first_layer_height, layer_height);
    let job = plate
        .slice_using(&settings, first, layer, order_of(&cfg))
        .map_err(|error| error.to_string())?;
    Ok((job, cfg))
}

fn slice_document(
    plate: &Plate,
    profile: &str,
    first_layer_height: f64,
    layer_height: f64,
) -> Result<HfwGcode, String> {
    let (job, cfg) = plan_plate(plate, profile, first_layer_height, layer_height)?;
    write_job(&job, &cfg)
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
        let (job, _) = plan_plate(&plate.plate, &profile, first_layer_height, layer_height)?;
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
        slice_document(&plate.plate, &profile, first_layer_height, layer_height)
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
