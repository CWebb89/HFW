/**
 * The C surface of the HFW slicing engine.
 *
 * Implemented by crates/capi; the gate compiles and runs tests/smoke.c against
 * this header and the built library, so the two cannot drift apart silently.
 *
 * Every call reports failure two ways: a null pointer or a negative integer, and
 * a message on hfw_last_error(). A call that succeeds clears the message.
 */

#ifndef HFW_H
#define HFW_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Version of the surface this header declares. */
#define HFW_ABI_VERSION 1u

/** An opaque mesh owned by the library. */
typedef struct HfwMesh HfwMesh;

/** An opaque plate document owned by the library. */
typedef struct HfwPlate HfwPlate;

/** An opaque slice result owned by the library. */
typedef struct HfwGcode HfwGcode;

/** Axis-aligned bounds of a mesh, in millimetres. */
typedef struct HfwBounds {
    double min[3];
    double max[3];
} HfwBounds;

/** A placement applied to a mesh. */
typedef struct HfwTransform {
    double scale[3];
    double rotation_degrees[3];
    double translation_mm[3];
} HfwTransform;

/** Returns the version of the surface the library exports. */
uint32_t hfw_abi_version(void);

/**
 * Returns the settings surface as a JSON document.
 *
 * The pointer refers to a NUL-terminated string that lives for the life of the
 * process; do not free it. Each setting carries its dialect name, the key this
 * engine uses, its location, type, default, unit and whether the value changes
 * the G-code (`applied`, `partial`, `missing` or `n/a`), so a settings panel can
 * hide or disable everything that is not `applied`. Its `capabilities` array lists
 * the features this engine declines — paint, text, cut and hollow, STEP, SLA, MMU,
 * a nesting arranger — with `disposition`, `panel`, `note` and the setting to use
 * instead, so a panel shows those controls as unavailable and says why.
 */
const char *hfw_settings_surface_json(void);

/** Returns this thread's last error message, or NULL when the last call succeeded. */
const char *hfw_last_error(void);

/**
 * Loads a mesh from an STL or 3MF file.
 *
 * Release the result with hfw_mesh_free(). Returns NULL on failure.
 */
HfwMesh *hfw_mesh_load_file(const char *path);

/**
 * Builds a mesh from the bytes of an STL file held in memory.
 *
 * Release the result with hfw_mesh_free(). Returns NULL on failure.
 */
HfwMesh *hfw_mesh_from_bytes(const uint8_t *bytes, size_t len);

/**
 * Returns a new handle holding `mesh` moved by `transform`.
 *
 * `mesh` is unchanged and stays the caller's to free. Rotation is in degrees,
 * applied X then Y then Z; translation is in millimetres. Returns NULL on
 * failure.
 */
HfwMesh *hfw_mesh_transform(const HfwMesh *mesh, HfwTransform transform);

/**
 * Writes the mesh's bounds in millimetres into `out`.
 *
 * Returns 0 on success and -1 when the mesh has no area or no height.
 */
int32_t hfw_mesh_bounds(const HfwMesh *mesh, HfwBounds *out);

/** Returns the triangle count, or 0 for a NULL handle. */
size_t hfw_mesh_triangle_count(const HfwMesh *mesh);

/** Painted support: 0 none, 1 enforcer, 2 blocker per triangle. */
#define HFW_PAINT_SUPPORT 0

/** Painted seam: 0 none, 1 enforcer, 2 blocker per triangle; applied to the outer wall. */
#define HFW_PAINT_SEAM 1

/**
 * Writes one paint channel onto `mesh`.
 *
 * `states` points at `len` bytes, one per triangle in the mesh's own order, each
 * 0 (none), 1 (enforcer) or 2 (blocker). A `len` of 0 clears the channel.
 *
 * Returns 0 on success, -1 when the handle is NULL, the length is not the triangle
 * count, or a state is outside the three.
 */
int32_t hfw_mesh_paint_set(HfwMesh *mesh, int32_t channel, const uint8_t *states,
                           size_t len);

/**
 * Reads one paint channel back off `mesh`.
 *
 * `out` holds `len` bytes; the return value is the number of bytes written, 0 when
 * the channel was never painted, and -1 when the handle is NULL, the channel does
 * not exist, or `len` is smaller than the triangle count.
 */
ptrdiff_t hfw_mesh_paint_get(const HfwMesh *mesh, int32_t channel, uint8_t *out,
                           size_t len);

/** Returns the enclosed volume in cubic millimetres, or NaN for a NULL handle. */
double hfw_mesh_volume(const HfwMesh *mesh);

/** Releases a mesh handle. A NULL handle is ignored. */
void hfw_mesh_free(HfwMesh *mesh);

/**
 * Slices a list of meshes and returns G-code for it.
 *
 * `meshes` points at `count` live handles, each already placed where it belongs
 * on the bed: nothing is arranged. Use this when there is no document; a caller
 * that has one holds an HfwPlate and slices that instead. `profile` names a
 * printer profile. Both layer heights are in millimetres; a value of zero or less
 * takes the profile's own. Returns NULL on failure.
 */
HfwGcode *hfw_slice_meshes(const HfwMesh *const *meshes,
                           size_t count,
                           const char *profile,
                           double first_layer_height,
                           double layer_height);

/** Creates an empty plate with the engine's default settings. */
HfwPlate *hfw_plate_new(void);

/**
 * Adds a copy of `mesh` to the plate. `mesh` stays the caller's to free.
 *
 * Returns the new object's id, which is never zero, or 0 on failure.
 */
uint64_t hfw_plate_add(HfwPlate *plate, const HfwMesh *mesh);

/**
 * Adds `mesh` to the plate as a file placed it, so arranging leaves it alone.
 *
 * Returns the new object's id, which is never zero, or 0 on failure.
 */
uint64_t hfw_plate_add_positioned(HfwPlate *plate, const HfwMesh *mesh);

/** Returns how many objects the plate holds, enabled or not. */
size_t hfw_plate_object_count(const HfwPlate *plate);

/** Names object `id`. Returns 0 on success, -1 when the object is not on the plate. */
int32_t hfw_plate_object_name(HfwPlate *plate, uint64_t id, const char *name);

/**
 * Moves object `id` without touching its triangles.
 *
 * Returns 0 on success, -1 when the object is not on the plate.
 */
int32_t hfw_plate_object_placement(HfwPlate *plate, uint64_t id, HfwTransform transform);

/**
 * Folds object `id`'s placement into its geometry and resets the placement.
 *
 * Returns 0 on success, -1 when the object is not on the plate or the placement
 * cannot be applied.
 */
int32_t hfw_plate_object_bake(HfwPlate *plate, uint64_t id);

/**
 * Includes object `id` in the job, or leaves it out.
 *
 * Returns 0 on success, -1 when the object is not on the plate.
 */
int32_t hfw_plate_object_enabled(HfwPlate *plate, uint64_t id, int32_t enabled);

/**
 * Copies object `id` and returns the copy's id, or 0 on error.
 *
 * Geometry, placement, settings and overrides are copied; the copy lands exactly
 * on top of the original, named `<original> copy`, so move it or lay the plate out
 * with `hfw_plate_arrange`.
 */
uint64_t hfw_plate_object_duplicate(HfwPlate *plate, uint64_t id);

/**
 * Turns object `id` about its own centre on the bed by `degrees`, around `axis`:
 * 0 for X, 1 for Y, 2 for Z. Rotations on the same axis add up.
 *
 * Returns 0, or -1 when the object is unknown or the axis or angle is refused.
 */
int32_t hfw_plate_object_rotate(HfwPlate *plate, uint64_t id, int32_t axis,
                                double degrees);

/**
 * Mirrors object `id` through its own centre on the bed, along `axis`: 0 for X,
 * 1 for Y, 2 for Z. The part stays where it is and still slices.
 *
 * Returns 0, or -1 when the object is unknown or the axis is refused.
 */
int32_t hfw_plate_object_mirror(HfwPlate *plate, uint64_t id, int32_t axis);

/**
 * Lays the plate out on a `bed_x` by `bed_y` bed, leaving `spacing` millimetres
 * between any two objects. Objects a project file positioned keep their
 * coordinates and the rest are packed around them.
 *
 * Geometry is never rewritten, only the placement. A part that does not fit is
 * refused with its size in `hfw_last_error` rather than omitted.
 */
int32_t hfw_plate_arrange(HfwPlate *plate, double bed_x, double bed_y,
                          double spacing);

/**
 * Sets one setting on one object, by name and value as a settings panel states
 * it. The object takes the plate's settings on the first override, so the value
 * is a delta from the job and keys may arrive in any order. Returns 0, or -1
 * when the object is unknown or the key or value cannot be applied; then
 * `hfw_last_error` names the key and what it accepts.
 *
 * The accepted names are the rows `hfw_settings_surface_json` marks
 * `per_object`, in either the engine's spelling or the dialect's.
 */
int32_t hfw_plate_object_set(HfwPlate *plate, uint64_t id, const char *key,
                             const char *value);

/** Drops object `id`'s own settings, so it prints with the plate's again. */
int32_t hfw_plate_object_clear_settings(HfwPlate *plate, uint64_t id);

/**
 * Writes one paint channel onto object `id`.
 *
 * `states` points at `len` bytes, one per triangle of the object's geometry, each
 * 0 (none), 1 (enforcer) or 2 (blocker); a `len` of 0 clears the channel. Placement,
 * rotation and baking preserve the triangle order, so states stay valid across
 * them.
 *
 * Returns 0 on success, -1 when the object is not on the plate, the length is not
 * the triangle count, or a state is outside the three.
 */
int32_t hfw_plate_object_paint(HfwPlate *plate, uint64_t id, int32_t channel,
                               const uint8_t *states, size_t len);

/**
 * Reads one paint channel back off object `id`.
 *
 * Returns the number of bytes written, 0 when the channel was never painted, and
 * -1 when the object is not on the plate, the channel does not exist, or `len` is
 * smaller than the triangle count.
 */
ptrdiff_t hfw_plate_object_paint_get(const HfwPlate *plate, uint64_t id,
                                   int32_t channel, uint8_t *out, size_t len);

/** A volume that adds material to its object. */
#define HFW_VOLUME_PART 0
/** A volume that removes material from its object. */
#define HFW_VOLUME_NEGATIVE 1
/** A volume support may not enter, over the volume's whole height. */
#define HFW_VOLUME_SUPPORT_BLOCKER 2
/** A volume support has to reach, over the volume's whole height. */
#define HFW_VOLUME_SUPPORT_ENFORCER 3

/**
 * Attaches a volume to object `id`, in the object's own coordinates, and copies the
 * geometry.
 *
 * The volume shares the object's placement, so moving the part moves what it cuts
 * and what it forbids support in. A negative volume's cross-section is subtracted
 * from every layer before walls, infill and skin are planned, so the hole it cuts
 * gets walls of its own; a part volume is united the same way.
 *
 * Returns 0 on success, -1 when the object is not on the plate, `mesh` is null, the
 * geometry has no triangles, or `kind` is not one of the four.
 */
int32_t hfw_plate_object_add_volume(HfwPlate *plate, uint64_t id, int32_t kind,
                                   const HfwMesh *mesh);

/**
 * Detaches volume `index` from object `id`, counted from zero.
 *
 * Returns 0 on success, -1 when the object is not on the plate or it has no volume
 * at that index.
 */
int32_t hfw_plate_object_remove_volume(HfwPlate *plate, uint64_t id, size_t index);

/**
 * How many volumes object `id` carries, or -1 when it is not on the plate.
 */
ptrdiff_t hfw_plate_object_volume_count(const HfwPlate *plate, uint64_t id);

/**
 * Writes which kind of volume `index` of object `id` is into `out`.
 *
 * Returns 0 on success, -1 when the object is not on the plate, `out` is null, or
 * the object has no volume at that index.
 */
int32_t hfw_plate_object_volume_kind(const HfwPlate *plate, uint64_t id, size_t index,
                                     int32_t *out);

/**
 * Sets object `id`'s own layer heights, as `count` interleaved `(z, height)` pairs of
 * millimetres; a `count` of 0 clears them back to the plate's uniform layer height.
 *
 * A layer whose bottom sits at or above a `z` uses that height until the next pair
 * takes over. Two objects on one plate may step differently: `by layer` interleaves
 * them on Z, so a layer change happens when either object's next layer is higher.
 *
 * Returns 0 on success, -1 when the object is not on the plate, the pairs do not
 * rise, or a height is not positive.
 */
int32_t hfw_plate_object_layer_heights(HfwPlate *plate, uint64_t id, const double *pairs,
                                       size_t count);

/**
 * Reads object `id`'s own layer heights back as interleaved `(z, height)` pairs.
 *
 * Returns the number of pairs written, 0 when the object prints at the plate's
 * uniform height, and -1 when the object is not on the plate or `len` pairs do not
 * fit in `out`.
 */
ptrdiff_t hfw_plate_object_layer_heights_get(const HfwPlate *plate, uint64_t id, double *out,
                                             size_t len);

/**
 * Removes object `id` from the plate. Every other object keeps its id.
 *
 * Returns 0 on success, -1 when the object is not on the plate.
 */
int32_t hfw_plate_object_remove(HfwPlate *plate, uint64_t id);

/**
 * Writes object `id`'s bounds on the bed, in millimetres, into `out`.
 *
 * Returns 0 on success, -1 when the object is not on the plate or has no
 * geometry.
 */
int32_t hfw_plate_object_bounds(const HfwPlate *plate, uint64_t id, HfwBounds *out);

/**
 * Writes the bounds of every object on the plate, in millimetres, into `out`.
 *
 * Returns 0 on success, -1 when the plate holds no geometry.
 */
int32_t hfw_plate_bounds(const HfwPlate *plate, HfwBounds *out);

/**
 * Slices the plate and returns G-code for it.
 *
 * The profile's process settings are layered onto the plate's for this job
 * without being stored, so slicing under one profile and then another does not
 * leave the second on the document. Objects are sliced where they are: the file
 * keeps the coordinates the plate holds. Returns NULL on failure.
 */
HfwGcode *hfw_plate_slice(const HfwPlate *plate,
                          const char *profile,
                          double first_layer_height,
                          double layer_height);

/** Releases a plate, and with it every object on it. A NULL handle is ignored. */
void hfw_plate_free(HfwPlate *plate);

/** Returns the G-code text, NUL-terminated, owned by the handle. */
const char *hfw_gcode_text(const HfwGcode *gcode);

/** Returns the G-code length in bytes, excluding the terminating NUL. */
size_t hfw_gcode_len(const HfwGcode *gcode);

/** Returns 1 when the engine's verifier passed the G-code, 0 when it did not. */
int32_t hfw_gcode_verified(const HfwGcode *gcode);

/** Releases a G-code handle. A NULL handle is ignored. */
void hfw_gcode_free(HfwGcode *gcode);

/**
 * Plans the plate and returns the preview document as JSON, or NULL on error.
 *
 * One entry per layer, one polyline per bead, each carrying the label the G-code
 * writer gives it (`Outer wall`, `Top surface`, ...), the per-vertex width and
 * the flow multiplier, so a viewer groups paths exactly as the printer's own
 * `;TYPE:` markers do. The string is owned by the caller: release it with
 * `hfw_string_free`.
 */
char *hfw_plate_preview_json(const HfwPlate *plate, const char *profile,
                             double first_layer_height, double layer_height);

/** Releases a string this library returned. A NULL pointer is ignored. */
void hfw_string_free(char *text);

/**
 * Writes the plate out as a project file and reports its length in `out_len`, or
 * returns NULL on error.
 *
 * The bytes are a `.3mf`: the layout in the standard model part, so any slicer can
 * open it, and which objects are switched off plus each object's settings in
 * `Metadata/hfw_project.xml` beside it. Release them with `hfw_buffer_free`.
 */
char *hfw_plate_project_bytes(const HfwPlate *plate, size_t *out_len);

/** Releases a byte buffer this library returned, with the length it reported. */
void hfw_buffer_free(char *bytes, size_t len);

/**
 * Opens a project file and returns the plate it describes, or NULL on error.
 *
 * `profile` is a printer profile path, or NULL for the engine's defaults; it is
 * the plate's settings layer, as a slice takes it. Every object comes back marked
 * positioned, because its coordinates came from the file.
 */
HfwPlate *hfw_plate_open_project(const char *bytes, size_t len, const char *profile);

/** A job running on its own thread. */
typedef struct HfwJob HfwJob;

/** `hfw_job_poll` result: the job is still running. */
#define HFW_JOB_RUNNING 0
/** `hfw_job_poll` result: the job finished and its G-code is available. */
#define HFW_JOB_DONE 1
/** `hfw_job_poll` result: the job was cancelled and produced nothing. */
#define HFW_JOB_CANCELLED 2
/** `hfw_job_poll` result: the job failed; `hfw_last_error` says why. */
#define HFW_JOB_FAILED (-1)

/** Progress stage: planning the plate, one object at a time. */
#define HFW_JOB_STAGE_SLICING 0
/** Progress stage: writing and verifying the file. */
#define HFW_JOB_STAGE_WRITING 1
/** Progress stage: the G-code is ready. */
#define HFW_JOB_STAGE_FINISHED 2

/** Where a running job has got to. */
typedef struct HfwJobProgress {
    /** One of the `HFW_JOB_STAGE_*` values. */
    int32_t stage;
    /** Work done in the current stage: objects planned, or layers written. */
    size_t done;
    /** Work to do in the current stage, or zero while it is not yet known. */
    size_t total;
} HfwJobProgress;

/**
 * Starts a job for `plate` with `profile` applied, on its own thread, or NULL
 * when the thread cannot start.
 *
 * The plate is copied into the job, so the caller may keep editing the plate — or
 * free it — while the job runs.
 */
HfwJob *hfw_job_start(const HfwPlate *plate, const char *profile,
                      double first_layer_height, double layer_height);

/**
 * Reports the job's status and copies its progress into `out`, without blocking.
 *
 * `out` may be NULL when only the status is wanted.
 */
int32_t hfw_job_poll(HfwJob *job, HfwJobProgress *out);

/**
 * Asks the job to stop. A job that has already finished is unaffected.
 *
 * Cancellation is observed before each object is planned and once more before the
 * file is written, so a cancelled job yields no G-code at all rather than a
 * partial file. Returns 0, or -1 for a NULL handle.
 */
int32_t hfw_job_cancel(HfwJob *job);

/** Waits for the job to finish and reports its final status. */
int32_t hfw_job_wait(HfwJob *job);

/**
 * Takes the job's G-code, or NULL when it has not finished, failed or was
 * cancelled; `hfw_last_error` then says which. The job handle is consumed and
 * must not be used again; release the G-code with `hfw_gcode_free`.
 */
HfwGcode *hfw_job_take_gcode(HfwJob *job);

/**
 * Releases a job handle, cancelling it first and waiting for its thread. A NULL
 * handle is ignored.
 */
void hfw_job_free(HfwJob *job);

#ifdef __cplusplus
}
#endif

#endif
