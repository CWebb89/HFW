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

/** An opaque snapshot of a plate document, owned by the library. */
typedef struct HfwSnapshot HfwSnapshot;

/** An opaque preset bundle: every preset under one directory, indexed. */
typedef struct HfwBundle HfwBundle;

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
 * hide or disable everything that is not `applied`. Its `capabilities` array is the
 * parity backlog — every feature libslic3r has and this engine does not yet — with
 * `title`, `state` (`unbuilt`, `blocked` or `excluded`), `panel`, `note`, `cost` and,
 * when blocked, `blocked_on`, so a panel shows those controls as unavailable and says
 * what closing them takes. Painting is not on that list: it is an ABI of its own
 * below, and the channels it carries are read by the support generator and the seam
 * placer.
 */
const char *hfw_settings_surface_json(void);

/**
 * Opens the preset bundle under `root`: every preset file below it, indexed by kind and
 * name.
 *
 * A bundle is the directory a front end keeps its presets in, in the layout
 * `docs/PRESETS.md` describes: files of `[machine:name]`, `[process:name]` and
 * `[filament:name]` sections, one preset per file, both that and a single-preset file
 * discovered recursively. The handle is the whole surface a preset store needs -- the
 * machines, the profiles one machine accepts, and what a selection resolves to -- and it
 * is opaque because the indexing is the library's business.
 *
 * Returns NULL with the reason when the directory holds no presets or cannot be read.
 * Release the handle with `hfw_bundle_free`.
 */
HfwBundle *hfw_bundle_open(const char *root);

/** Releases a bundle handle. A NULL pointer is ignored. */
void hfw_bundle_free(HfwBundle *bundle);

/**
 * Re-reads the directory the bundle was opened from.
 *
 * This is what makes a preset library live: a file added, edited or deleted while the
 * application runs is indexed on the next call, with no relaunch and no second handle.
 * The old index is replaced only when the new one reads, so a bundle that goes unreadable
 * leaves the caller with what it had and the reason why. Returns 0 on success, -1 with
 * the reason on failure.
 */
int32_t hfw_bundle_reload(HfwBundle *bundle);

/**
 * Returns the machines a bundle holds, as JSON:
 *
 *     {"printer_models":[{"name","vendor_name","vendor_id","technology":"FFF",
 *       "variants":[{"id","printer_profiles":[{"name","extruders_cnt",
 *       "bed":{"width","height","max_print_height"}}]}]}]}
 *
 * One printer profile per machine, because a bundle here has one hardware configuration
 * per machine preset where the reference slicers group several. `name` is what the other
 * two calls are made with. Release the string with `hfw_string_free`.
 */
char *hfw_bundle_printer_models(const HfwBundle *bundle);

/**
 * Returns the profiles one machine accepts, as JSON:
 *
 *     {"print_profiles":[{"name","filament_profiles":[...]}]}
 *
 * Every process in the bundle, each listing every filament: a bundle here does not pair a
 * material with a process, and a panel reads the union of the per-process lists. Returns
 * NULL with the reason when the machine is not in the bundle. Release the string with
 * `hfw_string_free`.
 */
char *hfw_bundle_profiles_for(const HfwBundle *bundle, const char *printer);

/**
 * Returns what a machine, a process and materials resolve to, as one JSON document:
 *
 *     {"preset":{...},"configuration":{"print_settings":{...},
 *      "filament_settings":{...},"printer_settings":{...}},"warnings":[...]}
 *
 * The shape a preset store already parses. Every setting carries the dialect name a panel
 * files it under and the JSON type its value takes, taken from the same generated surface
 * the panel reads, so the two cannot disagree about a name, a location or a type.
 * `filaments` is a list of material preset names, of which the first non-empty one is
 * used: this engine prints with one extruder.
 *
 * The machine is required; a process or a material the bundle does not hold is a warning
 * in the document rather than a failure. Release the string with `hfw_string_free`.
 */
char *hfw_bundle_resolved_config(const HfwBundle *bundle, const char *printer,
                                 const char *print, const char *const *filaments,
                                 size_t count);

/**
 * Returns one line saying what a bundle holds: its counts and any warnings. Release the
 * string with `hfw_string_free`.
 */
char *hfw_bundle_summary(const HfwBundle *bundle);

/** Returns this thread's last error message, or NULL when the last call succeeded. */
const char *hfw_last_error(void);

/**
 * Loads a mesh from an STL, OBJ or 3MF file.
 *
 * Release the result with hfw_mesh_free(). Returns NULL on failure.
 */
HfwMesh *hfw_mesh_load_file(const char *path);

/**
 * The file extensions hfw_mesh_load_file() reads, comma separated with the leading
 * dot: ".3mf,.stl,.obj". The pointer lives for the life of the process; do not free
 * it.
 */
const char *hfw_mesh_load_extensions(void);

/**
 * Writes a mesh as the bytes of an STL file.
 *
 * A non-zero `binary` selects the binary form, which is what a slicer reads; zero
 * writes the ASCII form, which is text a project file can store and a diff can read.
 * The bytes are `*out_len` long and are released with hfw_buffer_free(); returns
 * NULL on failure.
 */
char *hfw_mesh_stl_bytes(const HfwMesh *mesh, int binary, size_t *out_len);

/**
 * Builds a mesh from the bytes of an STL file held in memory.
 *
 * Release the result with hfw_mesh_free(). Returns NULL on failure.
 */
HfwMesh *hfw_mesh_from_bytes(const uint8_t *bytes, size_t len);

/**
 * Builds a mesh from millimetre coordinates and triangle indices.
 *
 * `vertices` holds `vertex_count` interleaved x, y, z triples, `indices`
 * `triangle_count` triples of vertex numbers. Nothing is welded and no triangle is
 * dropped, so triangle `i` of the handle is triangle `i` of `indices`: per-triangle
 * data a caller holds in its own order stays aligned with it.
 *
 * Release the result with hfw_mesh_free(). Returns NULL on failure.
 */
HfwMesh *hfw_mesh_new(const double *vertices, size_t vertex_count, const uint32_t *indices,
                      size_t triangle_count);

/**
 * Returns a new handle holding `mesh` moved by `transform`.
 *
 * `mesh` is unchanged and stays the caller's to free. Rotation is in degrees,
 * applied X then Y then Z; translation is in millimetres. Returns NULL on
 * failure.
 */
HfwMesh *hfw_mesh_transform(const HfwMesh *mesh, HfwTransform transform);

/**
 * Cuts a mesh with a plane, returning the part below it and the part above it.
 *
 * Both halves are closed, because the cut's own opening is capped; a cut that passes
 * through a hole caps the hole's ring too. `normal` and `point` are three doubles each, in
 * millimetres, and the plane passes through `point` facing `normal`. A cut that misses the
 * mesh puts it all in one half and leaves the other empty.
 *
 * Returns 0 on success and -1 when the mesh or either output slot is null, the normal has
 * no direction, or a coordinate is out of range. Both handles must be released with
 * `hfw_mesh_free`.
 */
int32_t hfw_mesh_cut(const HfwMesh *mesh, const double *normal, const double *point,
                     HfwMesh **below, HfwMesh **above);

/**
 * How many separate shells a mesh holds.
 *
 * A shell is a group of triangles joined to each other, so two copies of a part imported as
 * one mesh report two. Use it with `hfw_mesh_part` to split one mesh into one per shell.
 * Returns 0 for a null handle.
 */
size_t hfw_mesh_part_count(const HfwMesh *mesh);

/**
 * One shell of a mesh, by index, in the order its first triangle appears.
 *
 * Returns NULL when the mesh is null or the index is past the last shell. The handle must
 * be released with `hfw_mesh_free`.
 */
HfwMesh *hfw_mesh_part(const HfwMesh *mesh, size_t index);

/**
 * Builds a mesh from a string of text, extruded from a font file.
 *
 * The mesh is a closed prism: the glyphs of `text` at `size_mm`, raised from `z_mm` to
 * `z_mm + height_mm`, laid out left to right from the origin with the baseline at
 * `y = 0`. Feed it to `hfw_plate_object_add_volume` as `HFW_VOLUME_PART` to print it
 * raised, or as `HFW_VOLUME_NEGATIVE` to engrave it: the slice unions or subtracts the
 * prism per layer, the same way it handles any other volume, so no boolean is involved.
 *
 * `face_index` selects a face inside a font collection; a plain `.ttf` or `.otf` has
 * one face, at index 0. A CFF-flavoured OpenType file is refused by name. Returns NULL
 * when the font cannot be read, when no glyph in `text` is drawn by it, or when the
 * size, the height or the coordinates are out of range.
 */
HfwMesh *hfw_text_mesh(const char *font_path, uint32_t face_index, const char *text,
                      double size_mm, double height_mm, double z_mm);

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

/** An object's settings are named the way this engine names them. */
#define HFW_DIALECT_ENGINE 0

/** An object's settings are named the way Bambu Studio and OrcaSlicer name them. */
#define HFW_DIALECT_ORCA 1

/** A setting was kept on an object because this engine has no setting by that name. */
#define HFW_FOREIGN_UNKNOWN_NAME 0

/** A setting was kept on an object because this engine does not accept that value. */
#define HFW_FOREIGN_UNUSABLE_VALUE 1

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

/**
 * Vertex and index access.
 *
 * `hfw_mesh_vertices` writes `count` triples of millimetres and `hfw_mesh_indices`
 * writes `count` triangles, both in the mesh's own order -- the order its paint is
 * indexed by, so a reader that keeps these triangles keeps its paint aligned. A
 * buffer shorter than the mesh is an error, never a partial write.
 */
size_t hfw_mesh_vertex_count(const HfwMesh *mesh);
int32_t hfw_mesh_vertices(const HfwMesh *mesh, double *out, size_t count);
size_t hfw_mesh_index_count(const HfwMesh *mesh);
int32_t hfw_mesh_indices(const HfwMesh *mesh, uint32_t *out, size_t count);

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
 * How many build plates the document holds, which is never zero, or -1 when the
 * handle is null.
 *
 * A plate is an index a front end can hold: adding one appends, removing one moves the
 * plates after it down, and the objects on it come with the index rather than with a
 * handle. Object ids run over the whole document, so one id means one object whichever
 * plate it is on.
 */
ptrdiff_t hfw_plate_bed_count(const HfwPlate *plate);

/** Appends an empty, unnamed, unlocked plate and returns its index, or -1 on error. */
ptrdiff_t hfw_plate_bed_add(HfwPlate *plate);

/**
 * Removes the plate at `bed`, and everything on it.
 *
 * The last plate cannot be removed and a locked plate cannot be. The plates after it
 * move down one, so an object on plate 3 is on plate 2 afterwards; every id stays what
 * it was and the removed objects' ids are retired rather than reused. Returns 0 on
 * success, -1 with the reason otherwise.
 */
int32_t hfw_plate_bed_remove(HfwPlate *plate, size_t bed);

/**
 * Names the plate at `bed`. An empty name clears it, so the front end's own wording
 * ("Plate 2") applies again.
 *
 * Returns 0 on success, -1 when the document has no such plate.
 */
int32_t hfw_plate_bed_name(HfwPlate *plate, size_t bed, const char *name);

/**
 * Writes the plate's own name into `out` and returns the bytes it needs excluding the
 * terminating NUL.
 *
 * An unnamed plate reports 0 and writes an empty string: the wording a user reads for a
 * plate with no name of its own belongs to the front end. A buffer that is too short
 * writes nothing and reports the length, so a caller can size and retry. Returns -1
 * when the document has no such plate.
 */
ptrdiff_t hfw_plate_bed_name_get(const HfwPlate *plate, size_t bed, char *out, size_t len);

/**
 * Returns 1 when the plate at `bed` is locked, 0 when it is not, and -1 when the
 * document has no such plate.
 *
 * A lock is enforced here where the engine is the one changing the document's shape --
 * removing the plate, moving an object onto or off it, adding to it -- and a front end
 * disables the rest of that plate's gestures itself.
 */
int32_t hfw_plate_bed_locked(const HfwPlate *plate, size_t bed);

/**
 * Locks or unlocks the plate at `bed`. Returns 0 on success, -1 when the document has
 * no such plate.
 */
int32_t hfw_plate_bed_set_locked(HfwPlate *plate, size_t bed, int32_t locked);

/**
 * Adds a copy of `mesh` to the plate at `bed` and returns the new object's id, or 0 on
 * failure -- including when that plate is locked, since adding to it changes it.
 */
uint64_t hfw_plate_add_on_bed(HfwPlate *plate, size_t bed, const HfwMesh *mesh);

/** Which plate object `id` is on, or -1 when the object is not in the document. */
ptrdiff_t hfw_plate_object_bed(const HfwPlate *plate, uint64_t id);

/**
 * Moves object `id` to the plate at `bed`.
 *
 * The object keeps its id, its geometry, its placement, its paint and its overrides: an
 * id means one object in the document, not one object per plate, so a selection or an
 * undo step holding it still names it after the move. Both plates are checked, so
 * moving off a locked plate and moving onto one are both refused. Returns 0 on success,
 * -1 with the reason.
 */
int32_t hfw_plate_object_set_bed(HfwPlate *plate, uint64_t id, size_t bed);

/**
 * Writes the ids of the objects on the plate at `bed` into `out`, in the order they
 * were added, and returns how many the plate carries.
 *
 * One call with a NULL `out` sizes the next. Returns 0 for a plate that does not exist
 * as well as for an empty one; ask `hfw_plate_bed_count` when that matters.
 */
size_t hfw_plate_bed_object_ids(const HfwPlate *plate, size_t bed, uint64_t *out, size_t len);

/**
 * Slices the enabled objects on the plate at `bed`, with `profile` as the job's
 * settings.
 *
 * This is how one plate of a multi-plate document is printed; `hfw_plate_slice` is the
 * whole document, which on a one-plate document is the same thing. Release the result
 * with `hfw_gcode_free`.
 */
HfwGcode *hfw_plate_bed_slice(const HfwPlate *plate, size_t bed, const char *profile,
                              double first_layer_height, double layer_height);

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
 * Moves object `id` from a row-major 4x4 matrix, sixteen doubles.
 *
 * The matrix composes the way this engine does: scale, then rotations about X, Y
 * and Z in degrees, then translation in millimetres -- `T * Rz * Ry * Rx * S`.
 * Anything else (shear, a collapsed axis, a perspective row) is refused rather
 * than truncated, because the plate records a scale triple and an Euler triple.
 *
 * A caller that keeps transforms as matrices uses this instead of decomposing
 * them into `HfwTransform`. Returns 0, or -1 when the object is not on the plate,
 * a pointer is null, or the matrix is not representable.
 */
int32_t hfw_plate_object_placement_matrix(HfwPlate *plate, uint64_t id, const double *matrix);

/**
 * Reads object `id`'s placement back as a row-major 4x4 matrix.
 *
 * `out` receives sixteen doubles. Returns 0, or -1 when the object is not on the
 * plate or `out` is null.
 */
int32_t hfw_plate_object_placement_matrix_get(const HfwPlate *plate, uint64_t id, double *out);

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
/** A volume that prints the part of the object it covers with its own settings. */
#define HFW_VOLUME_MODIFIER 4

/**
 * Attaches a volume to object `id`, at its geometry's own coordinates, and copies
 * the geometry.
 *
 * The volume shares the object's placement, so moving the part moves what it cuts
 * and what it forbids support in. A negative volume's cross-section is subtracted
 * from every layer before walls, infill and skin are planned, so the hole it cuts
 * gets walls of its own; a part volume is united the same way.
 *
 * Returns the volume's id, which is never zero and never reused -- including after
 * `hfw_plate_object_remove_volume` -- or 0 when the object is not on the plate,
 * `mesh` is null, the geometry has no triangles, or `kind` is not one of the five.
 * Address a volume by this id, not by its position: removing one renumbers the ones
 * after it, and a selection or a settings panel holding a position would then be
 * pointing at a different volume.
 */
uint64_t hfw_plate_object_add_volume(HfwPlate *plate, uint64_t id, int32_t kind,
                                    const HfwMesh *mesh);

/**
 * Attaches a volume and places it inside the object, from a row-major 4x4 matrix.
 *
 * `matrix` has the same shape and rules as `hfw_plate_object_placement_matrix`, and
 * composes on top of the object's own placement: the volume follows its part when the
 * part moves and can be moved, turned or scaled within the object without rewriting
 * any triangles. Returns the volume's id, or 0 on the refusals
 * `hfw_plate_object_add_volume` reports, plus a matrix that shears, collapses an axis
 * or is not affine.
 */
uint64_t hfw_plate_object_add_volume_placed(HfwPlate *plate, uint64_t id, int32_t kind,
                                           const HfwMesh *mesh, const double *matrix);

/**
 * Moves volume `volume` inside its object, from a row-major 4x4 matrix.
 *
 * Returns 0 on success, -1 when the object or volume does not exist or the matrix is
 * not representable.
 */
int32_t hfw_plate_object_volume_placement(HfwPlate *plate, uint64_t id, uint64_t volume,
                                          const double *matrix);

/**
 * Reads volume `volume`'s placement back as a row-major 4x4 matrix, sixteen doubles.
 *
 * Returns 0 on success, -1 when the object or volume does not exist or `out` is null.
 */
int32_t hfw_plate_object_volume_placement_get(const HfwPlate *plate, uint64_t id,
                                              uint64_t volume, double *out);

/**
 * Sets one setting on modifier volume `volume` of object `id`.
 *
 * Only a modifier volume has a region of its own to print differently, so a key set on
 * any other kind is refused rather than stored and ignored. The key is a settings key,
 * the same one an object override uses, applied on top of the object's settings and of
 * whatever the volume already carries. Returns 0 on success, -1 when the object or
 * volume does not exist, the volume is not a modifier, or the key or value is unknown.
 */
int32_t hfw_plate_volume_set(HfwPlate *plate, uint64_t id, uint64_t volume, const char *key,
                             const char *value);

/**
 * Detaches volume `volume` from object `id`. Every other volume keeps its id.
 *
 * Returns 0 on success, -1 when the object is not on the plate or it has no volume
 * with that id.
 */
int32_t hfw_plate_object_remove_volume(HfwPlate *plate, uint64_t id, uint64_t volume);

/**
 * How many volumes object `id` carries, or -1 when it is not on the plate.
 */
ptrdiff_t hfw_plate_object_volume_count(const HfwPlate *plate, uint64_t id);

/**
 * Writes every volume id of object `id` into `out`, in the order they were added.
 *
 * Returns how many the object carries, so one call with a NULL `out` sizes the next.
 */
size_t hfw_plate_object_volume_ids(const HfwPlate *plate, uint64_t id, uint64_t *out,
                                  size_t len);

/**
 * Writes which kind of volume `volume` of object `id` is into `out`.
 *
 * Returns 0 on success, -1 when the object is not on the plate, `out` is null, or
 * the object has no volume with that id.
 */
int32_t hfw_plate_object_volume_kind(const HfwPlate *plate, uint64_t id, uint64_t volume,
                                     int32_t *out);

/**
 * Sets the plate's in-layer strokes: beads printed inside a layer that the model does
 * not describe. This is the calibration channel -- a temperature tower's labels, a flow
 * test's lines.
 *
 * `count` strokes are described by parallel arrays: the print Z of the layer each
 * belongs to, the number of points it has, its line width, layer height and speed.
 * `points` holds every stroke's vertices back to back as interleaved x, y plate
 * coordinates, `starts` where each stroke begins in `points`, and `prefixes` the
 * G-code written before each stroke, verbatim (a temperature band changes the nozzle
 * temperature just before it draws). A `count` of 0 clears the strokes.
 *
 * Returns 0 on success, -1 when the counts do not agree, a stroke has fewer than two
 * points, a width, height or speed is not positive, or a Z is not above the bed.
 */
int32_t hfw_plate_set_in_layer(HfwPlate *plate, const double *zs, const size_t *counts,
                               const double *widths, const double *heights, const double *speeds,
                               const size_t *starts, const double *points,
                               const char *const *prefixes, size_t count);

/** Number of in-layer strokes on the plate, or -1 when the handle is null. */
ptrdiff_t hfw_plate_in_layer_count(const HfwPlate *plate);

/**
 * How many points stroke `index` has, or -1 when there is no stroke at that index.
 *
 * This sizes the buffer `hfw_plate_in_layer_get` fills.
 */
ptrdiff_t hfw_plate_in_layer_points(const HfwPlate *plate, size_t index);

/**
 * Reads stroke `index` back: its Z, its point count in `points`, its width, height and
 * speed. `points` receives up to `capacity` points as interleaved x, y and `prefix` up
 * to `len` bytes of its G-code including the terminator; either may be NULL to skip
 * it. Returns 0 on success, -1 when the index is past the end or `capacity` is too
 * small for the stroke.
 */
int32_t hfw_plate_in_layer_get(const HfwPlate *plate, size_t index, double *z, double *width,
                               double *height, double *speed, double *points, size_t capacity,
                               char *prefix, size_t len);

/** A marker that writes its own G-code at its height. */
#define HFW_MARKER_CUSTOM 0
/** A marker that stops the print, with the profile's `pause_print_gcode`. */
#define HFW_MARKER_PAUSE 1
/** A marker that changes filament, with the profile's `color_change_gcode`. */
#define HFW_MARKER_COLOUR 2

/**
 * Sets the plate's timeline markers, sorted by Z.
 *
 * `kinds` holds one `HFW_MARKER_CUSTOM`/`HFW_MARKER_PAUSE`/`HFW_MARKER_COLOUR` per
 * marker, `zs` its print heights in millimetres, and `texts` its G-code -- read only
 * for `HFW_MARKER_CUSTOM`, where a marker with no G-code in it is refused. A `count`
 * of zero clears the timeline. Markers are a property of the plate rather than the
 * profile, so two plates on one printer can pause at different heights.
 *
 * Returns 0 on success, -1 when a kind is not one of the three, a Z is not a finite
 * height above the bed, a custom marker has no text, or a pointer is null.
 */
int32_t hfw_plate_set_markers(HfwPlate *plate, const int32_t *kinds, const double *zs,
                              const char *const *texts, size_t count);

/** Number of markers on the plate's timeline, or -1 when the handle is null. */
ptrdiff_t hfw_plate_marker_count(const HfwPlate *plate);

/**
 * Reads marker `index` back: its kind, its Z, and its G-code.
 *
 * `text` receives up to `len` bytes of the marker's G-code including the terminator,
 * truncated to fit. `kind`, `z` and `text` may each be NULL to skip that field.
 *
 * Returns 0 on success, -1 when the handle is null or the index is past the end.
 */
int32_t hfw_plate_marker_get(const HfwPlate *plate, size_t index, int32_t *kind, double *z,
                             char *text, size_t len);

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
 * uniform height, and -1 when the object is not on the plate. A NULL `out` or a
 * `len` too small for the pairs reports the count the object has and writes nothing,
 * so one call sizes the buffer another fills.
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

/**
 * Slices the plate, places it on the bed the profile describes, and returns
 * G-code for it.
 *
 * The same call as `hfw_plate_slice` except that the plate is laid on the bed
 * rather than printed where it sits: `hfw_plate_slice` keeps the coordinates the
 * plate holds and this one moves it, which is what `hfw-cli --place` does. A
 * plate that came from a document was placed by whoever laid it out; bare
 * geometry with no document around it has no position to keep. Returns NULL on
 * failure.
 */
HfwGcode *hfw_plate_slice_placed(const HfwPlate *plate,
                                 const char *profile,
                                 double first_layer_height,
                                 double layer_height);

/** Releases a plate, and with it every object on it. A NULL handle is ignored. */
void hfw_plate_free(HfwPlate *plate);

/**
 * Takes the plate as it is now and returns it as a handle.
 *
 * This is what an undo step is: the whole document -- every object with its id,
 * geometry, placement, paint, settings, volumes and layer heights, the timeline, the
 * in-layer strokes, the caller's own bytes and the counter that hands out the next
 * id -- kept so it can be put back exactly. Taking one copies no triangles: the
 * snapshot shares each object's geometry with the plate, and the first write to an
 * object after this copies that object's geometry and leaves the snapshot holding
 * what it was taken with. A stack of snapshots therefore costs the document plus the
 * edits made since the first of them, not one document per step.
 *
 * The snapshot owns its shares: freeing the plate leaves it intact, and it can be
 * restored into any plate. Release it with `hfw_snapshot_free`. Returns NULL when
 * `plate` is null.
 */
HfwSnapshot *hfw_plate_snapshot(const HfwPlate *plate);

/**
 * Replaces everything `plate` holds with everything `snapshot` holds.
 *
 * The snapshot is not consumed: the same one restores again, which is what redo is.
 * A restored document prints, reads back and reopens as the one that was
 * snapshotted, ids included, so a selection or a paint session holding an id is
 * still holding the object it named. Returns 0 on success, -1 when either handle is
 * null.
 */
int32_t hfw_plate_restore(HfwPlate *plate, const HfwSnapshot *snapshot);

/** Releases a snapshot handle. A NULL pointer is ignored. */
void hfw_snapshot_free(HfwSnapshot *snapshot);

/** How many objects a snapshot holds, or -1 when the handle is null. */
ptrdiff_t hfw_snapshot_object_count(const HfwSnapshot *snapshot);

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

/**
 * Reading a plate back.
 *
 * A document the application holds has to survive being handed to the engine and
 * returned, so every part of it can be read: the ids, each object's name, placement,
 * enabled flag, geometry (with its paint) and settings, and each volume's geometry
 * and settings.
 *
 * `hfw_plate_object_ids` reports the count even for a NULL or short buffer, so one
 * call sizes a buffer it can also fill. Geometry comes back as a copy the caller
 * frees with `hfw_mesh_free`; a name that does not fit its buffer reports the length
 * it needs and writes nothing.
 */
/**
 * The caller's own bytes, saved and reopened with the plate.
 *
 * The engine never reads them: they are the application's state that has to travel
 * with the document. `len` of zero clears them; a NULL pointer with a non-zero length
 * is an error. `hfw_plate_metadata_get` returns them NUL-terminated with their real
 * length, so binary metadata survives, and returns NULL when there are none.
 */
int32_t hfw_plate_set_metadata(HfwPlate *plate, const char *bytes, size_t len);
char *hfw_plate_metadata_get(const HfwPlate *plate, size_t *out_len);

size_t hfw_plate_object_ids(const HfwPlate *plate, uint64_t *out, size_t len);
ptrdiff_t hfw_plate_object_name_get(const HfwPlate *plate, uint64_t id, char *out,
                                    size_t len);
int32_t hfw_plate_object_placement_get(const HfwPlate *plate, uint64_t id,
                                       HfwTransform *out);
int32_t hfw_plate_object_enabled_get(const HfwPlate *plate, uint64_t id, int32_t *out);
HfwMesh *hfw_plate_object_mesh(const HfwPlate *plate, uint64_t id);
char *hfw_plate_object_settings_json(const HfwPlate *plate, uint64_t id);
HfwMesh *hfw_plate_object_volume_mesh(const HfwPlate *plate, uint64_t id, uint64_t volume);
char *hfw_plate_object_volume_settings_json(const HfwPlate *plate, uint64_t id,
                                            uint64_t volume);

/**
 * Names volume `volume` of object `id`. An empty name clears it, so the front end's
 * own wording ("Negative volume 1") applies again.
 *
 * Returns 0 on success, -1 when the object or volume does not exist.
 */
int32_t hfw_plate_object_volume_name(HfwPlate *plate, uint64_t id, uint64_t volume,
                                     const char *name);

/**
 * Writes the name of volume `volume` of object `id` into `out` and returns the bytes it
 * needs excluding the terminating NUL.
 *
 * An unnamed volume reports 0 and writes an empty string: the wording a user reads
 * belongs to the front end, which is the side that knows the kind it drew. A buffer
 * that is too short writes nothing and reports the length, so a caller can size and
 * retry. Returns -1 when the object or volume does not exist.
 */
ptrdiff_t hfw_plate_object_volume_name_get(const HfwPlate *plate, uint64_t id,
                                          uint64_t volume, char *out, size_t len);

/**
 * Returns 1 when volume `volume` of object `id` is part of the geometry, 0 when it is
 * hidden, and -1 when the object or volume does not exist.
 */
int32_t hfw_plate_object_volume_visible(const HfwPlate *plate, uint64_t id, uint64_t volume);

/**
 * Shows or hides volume `volume` of object `id`.
 *
 * A hidden volume is not printed and not applied: a negative volume stops cutting and a
 * modifier stops changing the region under it, which is what the application does with
 * a volume the user hid in the viewport. Returns 0 on success, -1 when the object or
 * volume does not exist.
 */
int32_t hfw_plate_object_volume_set_visible(HfwPlate *plate, uint64_t id, uint64_t volume,
                                            int32_t visible);

/**
 * Labels object `id` the way a user did, keeping the name it arrived with.
 *
 * Name and label are separate because a rename has to be reversible and a save writes
 * the file's own name back; what a list shows is the label when there is one and the
 * name otherwise. An empty label clears it. Returns 0 on success, -1 when the object is
 * not on the plate.
 */
int32_t hfw_plate_object_label(HfwPlate *plate, uint64_t id, const char *label);

/**
 * Writes object `id`'s label into `out` and returns the bytes it needs excluding the
 * terminating NUL. An unlabelled object reports 0 and writes an empty string; a buffer
 * that is too short writes nothing. Returns -1 when the object is not on the plate.
 */
ptrdiff_t hfw_plate_object_label_get(const HfwPlate *plate, uint64_t id, char *out, size_t len);

/**
 * Records the file object `id` was imported from, or clears it with an empty string.
 *
 * The path is the object's own, not the caller's bytes: a panel shows it, a reload needs
 * it, and an undo that lost it is a document that cannot say where its parts came from.
 * Returns 0 on success, -1 when the object is not on the plate.
 */
int32_t hfw_plate_object_source(HfwPlate *plate, uint64_t id, const char *path);

/**
 * Writes object `id`'s source path into `out` and returns the bytes it needs excluding
 * the terminating NUL. An object that came from nowhere reports 0 and writes an empty
 * string. Returns -1 when the object is not on the plate.
 */
ptrdiff_t hfw_plate_object_source_get(const HfwPlate *plate, uint64_t id, char *out, size_t len);

/**
 * Which vocabulary object `id`'s settings are named in: `HFW_DIALECT_ENGINE` for this
 * engine's own names, `HFW_DIALECT_ORCA` for Bambu Studio and OrcaSlicer names, which is
 * what a save uses to decide whose names to write. Returns -1 when the object is not on
 * the plate.
 */
int32_t hfw_plate_object_dialect(const HfwPlate *plate, uint64_t id);

/**
 * Records which vocabulary object `id`'s settings are named in. Returns 0 on success, -1
 * when the object is not on the plate or the dialect is not one this engine knows.
 */
int32_t hfw_plate_object_set_dialect(HfwPlate *plate, uint64_t id, int32_t dialect);

/**
 * Applies one setting to object `id`, or keeps it on the object with the reason this
 * engine would not take it.
 *
 * This is what an import does with another slicer's per-object settings: the ones this
 * engine understands become overrides, and the rest stay on the object verbatim with why
 * they were not applied, so a save writes them back out and a panel can report what was
 * dropped. Returns 0 when the setting was applied, 1 when it was kept instead, and -1
 * when the object is not on the plate -- a file carrying names this engine has never
 * heard of is expected, not an error.
 */
int32_t hfw_plate_object_apply_foreign(HfwPlate *plate, uint64_t id, const char *key,
                                       const char *value);

/**
 * Keeps one setting on object `id` verbatim, with why it was not applied: `reason` is
 * `HFW_FOREIGN_UNKNOWN_NAME` or `HFW_FOREIGN_UNUSABLE_VALUE`.
 *
 * This is how a rebuilt document re-attaches what it read back. Returns 0 on success, -1
 * when the object is not on the plate or the reason is not one of those.
 */
int32_t hfw_plate_object_foreign_set(HfwPlate *plate, uint64_t id, const char *key,
                                     const char *value, int32_t reason);

/** Drops every setting object `id` kept from a foreign file. 0 on success, -1 on error. */
int32_t hfw_plate_object_foreign_clear(HfwPlate *plate, uint64_t id);

/**
 * Returns object `id`'s kept settings as a JSON array, in the order they were refused:
 * `[{"key":…,"value":…,"reason":…,"note":…}]`.
 *
 * The key and value are the file's own spelling, which is what a save writes back; the
 * reason is `unknown_name` or `unusable_value`, and the note is the sentence a panel
 * shows. Release the string with `hfw_string_free`. Returns NULL when the object is not on
 * the plate.
 */
char *hfw_plate_object_foreign_json(const HfwPlate *plate, uint64_t id);

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
