/**
 * Exercises the header from C, the way the application will.
 *
 * Usage: smoke <model.stl> <profile.cfg>
 *
 * Prints one line per check and returns non-zero on the first failure, so the
 * gate fails on a broken ABI rather than on a broken test harness.
 */

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../include/hfw.h"

static int failures = 0;

static void check(int condition, const char *what) {
    if (condition) {
        printf("  ok   %s\n", what);
        return;
    }
    printf("  FAIL %s\n", what);
    const char *error = hfw_last_error();
    if (error != NULL && error[0] != '\0') {
        printf("       last error: %s\n", error);
    }
    failures++;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s <model.stl> <profile.cfg>\n", argv[0]);
        return 2;
    }

    check(hfw_abi_version() == HFW_ABI_VERSION, "the header and the library agree on the version");

    const char *surface = hfw_settings_surface_json();
    check(surface != NULL && strlen(surface) > 10000, "the settings surface is a document");
    check(surface != NULL && strstr(surface, "\"surface\": \"hfw-settings\"") != NULL,
          "the settings surface names itself");
    check(surface != NULL && strstr(surface, "\"status\": \"applied\"") != NULL,
          "the settings surface reports statuses");

    const char *dialect = hfw_dialect_json();
    check(dialect != NULL && strstr(dialect, "\"dialect\":\"hfw-dialect\"") != NULL,
          "the dialect is a document that names itself");
    check(dialect != NULL && strstr(dialect, "\"wall_loops\":\"perimeters\"") != NULL,
          "the dialect carries the Orca spelling for perimeters");

    HfwMesh *mesh = hfw_mesh_load_file(argv[1]);
    check(mesh != NULL, "a mesh loads from a file");
    if (mesh == NULL) {
        return 1;
    }

    HfwBounds bounds;
    check(hfw_mesh_bounds(mesh, &bounds) == 0, "the bounds are readable");
    check(bounds.max[0] - bounds.min[0] > 1.0, "the bounds span more than a millimetre");
    printf("       x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f\n", bounds.min[0], bounds.max[0],
           bounds.min[1], bounds.max[1], bounds.min[2], bounds.max[2]);
    check(hfw_mesh_triangle_count(mesh) > 0, "the mesh has triangles");
    check(hfw_mesh_volume(mesh) > 1.0, "the mesh encloses a volume");

    const char *extensions = hfw_mesh_load_extensions();
    check(extensions != NULL && strstr(extensions, ".obj") != NULL,
          "the load extensions name the formats, OBJ among them");

    size_t stl_len = 0;
    char *stl = hfw_mesh_stl_bytes(mesh, 1, &stl_len);
    check(stl != NULL, "a mesh writes itself out as binary STL");
    if (stl != NULL) {
        check(stl_len == 84 + 50 * (size_t)hfw_mesh_triangle_count(mesh),
              "the binary file is a header and fifty bytes per triangle");
        HfwMesh *reread = hfw_mesh_from_bytes((const uint8_t *)stl, stl_len);
        check(reread != NULL, "the bytes read back as a mesh");
        if (reread != NULL) {
            check(fabs(hfw_mesh_volume(reread) - hfw_mesh_volume(mesh)) < 1e-3,
                  "the round trip keeps the volume");
            hfw_mesh_free(reread);
        }
        hfw_buffer_free(stl, stl_len);
    }
    char *ascii = hfw_mesh_stl_bytes(mesh, 0, &stl_len);
    check(ascii != NULL && stl_len > 6 && strncmp(ascii, "solid ", 6) == 0,
          "the text form is an ASCII STL");
    if (ascii != NULL) {
        hfw_buffer_free(ascii, stl_len);
    }

    HfwTransform shift;
    memset(&shift, 0, sizeof shift);
    shift.scale[0] = 1.0;
    shift.scale[1] = 1.0;
    shift.scale[2] = 1.0;
    shift.translation_mm[0] = 30.0;
    HfwMesh *moved = hfw_mesh_transform(mesh, shift);
    check(moved != NULL, "a transform produces a new handle");
    if (moved != NULL) {
        HfwBounds shifted;
        check(hfw_mesh_bounds(moved, &shifted) == 0, "the moved copy has bounds");
        check(fabs((shifted.min[0] - bounds.min[0]) - 30.0) < 1e-6,
              "the copy moved by the translation and the source did not");
        hfw_mesh_free(moved);
    }

    const HfwMesh *plate[1] = {mesh};
    HfwGcode *gcode = hfw_slice_meshes(plate, 1, argv[2], 0.2, 0.2);
    check(gcode != NULL, "a list of meshes slices");
    if (gcode != NULL) {
        const char *text = hfw_gcode_text(gcode);
        size_t len = hfw_gcode_len(gcode);
        check(text != NULL && len > 1024, "the G-code is not empty");
        check(text != NULL && strlen(text) == len, "the length agrees with the terminator");
        check(text != NULL && strstr(text, ";TYPE:") != NULL, "the file carries feature types");
        check(hfw_gcode_verified(gcode) == 1, "the engine's verifier passed the file");
        hfw_gcode_free(gcode);
    }

    HfwPlate *document = hfw_plate_new();
    check(document != NULL, "a plate is created");
    if (document != NULL) {
        uint64_t first = hfw_plate_add(document, mesh);
        HfwSnapshot *taken = hfw_plate_snapshot(document);
        check(taken != NULL, "a snapshot is taken");
        check(hfw_snapshot_object_count(taken) == 1, "the snapshot counts what was on the plate");
        uint64_t second = hfw_plate_add(document, mesh);
        check(first != 0 && second != 0 && first != second, "two objects are added by id");
        check(hfw_plate_object_count(document) == 2, "the plate counts both objects");
        check(hfw_plate_object_name(document, first, "bracket") == 0, "an object is named");

        double identity[16] = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};
        check(hfw_plate_object_placement_matrix(document, first, identity) == 0,
              "an object is placed from a row-major 4x4 matrix");
        double readback[16];
        check(hfw_plate_object_placement_matrix_get(document, first, readback) == 0 &&
                  fabs(readback[0] - 1.0) < 1e-9 && fabs(readback[5] - 1.0) < 1e-9 &&
                  fabs(readback[15] - 1.0) < 1e-9,
              "the matrix reads back as it was given");
        double sheared[16] = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};
        sheared[1] = 0.2;
        check(hfw_plate_object_placement_matrix(document, first, sheared) == -1,
              "a matrix that shears is refused rather than truncated");

        HfwTransform placement;
        memset(&placement, 0, sizeof placement);
        placement.scale[0] = 1.0;
        placement.scale[1] = 1.0;
        placement.scale[2] = 1.0;
        placement.translation_mm[0] = 30.0;
        check(hfw_plate_object_placement(document, second, placement) == 0,
              "an object is moved without its triangles");

        HfwBounds whole;
        check(hfw_plate_bounds(document, &whole) == 0, "the plate has bounds");
        check(fabs(whole.max[0] - (bounds.max[0] + 30.0)) < 1e-6,
              "the plate bounds cover the moved object");

        check(hfw_plate_object_enabled(document, first, 0) == 0, "an object is disabled");
        HfwGcode *document_gcode = hfw_plate_slice(document, argv[2], 0.2, 0.2);
        check(document_gcode != NULL, "the plate slices");
        if (document_gcode != NULL) {
            check(hfw_gcode_verified(document_gcode) == 1, "the plate's file verifies");
            hfw_gcode_free(document_gcode);
        }
        check(hfw_plate_object_enabled(document, first, 1) == 0, "an object is enabled again");
        check(hfw_plate_object_remove(document, second) == 0, "an object is removed");
        check(hfw_plate_object_count(document) == 1, "the plate counts one object");
        check(hfw_plate_restore(document, taken) == 0, "the snapshot is restored");
        check(hfw_plate_object_count(document) == 1 &&
                  hfw_plate_restore(document, NULL) == -1,
              "a restored plate holds what was snapshotted, and a null snapshot is refused");
        hfw_snapshot_free(taken);

        ptrdiff_t bed = hfw_plate_bed_add(document);
        check(bed == 1 && hfw_plate_bed_count(document) == 2, "a plate is appended");
        check(hfw_plate_bed_name(document, bed, "supports") == 0, "a plate is named");
        uint64_t on_bed = hfw_plate_add_on_bed(document, bed, mesh);
        check(on_bed != 0 && hfw_plate_object_bed(document, on_bed) == bed,
              "an object is added to the plate it names");
        uint64_t bed_ids[4];
        check(hfw_plate_bed_object_ids(document, bed, bed_ids, 4) == 1 && bed_ids[0] == on_bed,
              "a plate lists its own objects");
        check(hfw_plate_bed_set_locked(document, bed, 1) == 0 &&
                  hfw_plate_bed_locked(document, bed) == 1,
              "a plate is locked");
        check(hfw_plate_add_on_bed(document, bed, mesh) == 0 &&
                  hfw_plate_bed_remove(document, bed) == -1,
              "a locked plate takes nothing and goes nowhere");
        check(hfw_plate_bed_set_locked(document, bed, 0) == 0 &&
                  hfw_plate_bed_remove(document, bed) == 0 &&
                  hfw_plate_object_bed(document, on_bed) == -1,
              "a removed plate takes its objects with it");
        check(hfw_plate_bed_count(document) == 1, "one plate is left");

        char *preview = hfw_plate_preview_json(document, argv[2], 0.2, 0.2);
        check(preview != NULL, "the preview document is produced");
        check(preview != NULL && strstr(preview, "\"preview\":\"hfw-preview\"") != NULL,
              "the preview document names itself");
        check(preview != NULL && strstr(preview, "Outer wall") != NULL,
              "the preview labels a bead the way the G-code does");
        hfw_string_free(preview);
        HfwJob *job = hfw_job_start(document, argv[2], 0.2, 0.2);
        check(job != NULL, "a job starts on its own thread");
        if (job != NULL) {
            HfwJobProgress progress;
            int32_t status;
            do {
                status = hfw_job_poll(job, &progress);
            } while (status == HFW_JOB_RUNNING);
            check(status == HFW_JOB_DONE, "the job finishes");
            check(progress.stage == HFW_JOB_STAGE_FINISHED, "the job reports its last stage");
            check(progress.total > 0 && progress.done == progress.total,
                  "the job reports every layer it planned");
            HfwGcode *async = hfw_job_take_gcode(job);
            check(async != NULL, "the job hands over its G-code");
            HfwGcode *sync = hfw_plate_slice(document, argv[2], 0.2, 0.2);
            check(sync != NULL, "the same plate slices synchronously");
            check(async != NULL && sync != NULL &&
                      strcmp(hfw_gcode_text(async), hfw_gcode_text(sync)) == 0,
                  "the threaded job writes the same file as the synchronous slice");
            if (async != NULL) {
                hfw_gcode_free(async);
            }
            if (sync != NULL) {
                hfw_gcode_free(sync);
            }
        }
        HfwJob *cancelled = hfw_job_start(document, argv[2], 0.2, 0.2);
        check(cancelled != NULL, "a second job starts");
        if (cancelled != NULL) {
            check(hfw_job_cancel(cancelled) == 0, "the job accepts a cancel");
            int32_t status = hfw_job_wait(cancelled);
            check(status == HFW_JOB_DONE || status == HFW_JOB_CANCELLED,
                  "a cancelled job reports a final status");
            if (status == HFW_JOB_CANCELLED) {
                check(hfw_job_take_gcode(cancelled) == NULL,
                      "a cancelled job yields no file");
            } else {
                HfwGcode *done = hfw_job_take_gcode(cancelled);
                check(done != NULL, "a job that finished before the cancel keeps its file");
                if (done != NULL) {
                    hfw_gcode_free(done);
                }
            }
        }
            uint64_t copy = hfw_plate_object_duplicate(document, first);
            check(copy != 0 && copy != first, "an object is duplicated under a new id");
            check(hfw_plate_object_count(document) == 2, "the plate counts the copy");

            size_t project_len = 0;
            char *project = hfw_plate_project_bytes(document, &project_len);
            check(project != NULL && project_len > 1000,
                  "the plate is written as a project file");
            HfwPlate *reopened = hfw_plate_open_project(project, project_len, argv[2]);
            check(reopened != NULL, "the project file opens again");
            check(reopened != NULL && hfw_plate_object_count(reopened) == 2,
                  "the reopened plate holds both objects");
            if (reopened != NULL) {
                HfwGcode *from_project = hfw_plate_slice(reopened, argv[2], 0.2, 0.2);
                HfwGcode *from_plate = hfw_plate_slice(document, argv[2], 0.2, 0.2);
                check(from_project != NULL && from_plate != NULL,
                      "both the plate and the project file slice");
                check(from_project != NULL && from_plate != NULL &&
                          strcmp(hfw_gcode_text(from_project), hfw_gcode_text(from_plate)) == 0,
                      "the project prints exactly what it saved");
                if (from_project != NULL) {
                    hfw_gcode_free(from_project);
                }
                if (from_plate != NULL) {
                    hfw_gcode_free(from_plate);
                }
                hfw_plate_free(reopened);
            }
            check(hfw_plate_open_project("not a zip", 10, NULL) == NULL,
                  "a file that is not a project is refused");
            hfw_buffer_free(project, project_len);

            HfwBounds before_turn;
            check(hfw_plate_object_bounds(document, copy, &before_turn) == 0,
                  "the copy has bounds");
            check(hfw_plate_object_rotate(document, copy, 2, 90.0) == 0,
                  "an object turns about its own centre");
            HfwBounds after_turn;
            check(hfw_plate_object_bounds(document, copy, &after_turn) == 0,
                  "the turned copy has bounds");
            check(fabs((after_turn.min[0] + after_turn.max[0]) -
                       (before_turn.min[0] + before_turn.max[0])) < 1e-6,
                  "the turn keeps the centre where it was");
            check(hfw_plate_arrange(document, 250.0, 250.0, 5.0) == 0,
                  "the plate is laid out on the bed");
            HfwBounds laid_out;
            check(hfw_plate_bounds(document, &laid_out) == 0, "the laid-out plate has bounds");
            check(laid_out.min[0] >= -1e-6 && laid_out.min[1] >= -1e-6,
                  "the laid-out plate starts at the bed edge, not off it");
            check(laid_out.max[0] <= 250.0 && laid_out.max[1] <= 250.0,
                  "the laid-out plate fits the bed");
            check(hfw_plate_arrange(document, 10.0, 10.0, 5.0) == -1,
                  "a bed too small for the plate is refused");
            check(hfw_last_error() != NULL, "the refusal names the part that did not fit");
            check(hfw_plate_object_mirror(document, copy, 0) == 0,
                  "an object is mirrored through its own centre");
            check(hfw_plate_object_mirror(document, copy, 3) == -1,
                  "an axis that is not X, Y or Z is refused");

            uint64_t hole = hfw_plate_object_add_volume(document, first, HFW_VOLUME_NEGATIVE, mesh);
            check(hole != 0, "a negative volume is attached");
            check(hfw_plate_object_volume_name(document, first, hole, "the hole") == 0,
                  "a volume is named");
            char hole_name[32];
            check(hfw_plate_object_volume_name_get(document, first, hole, hole_name,
                                                  sizeof hole_name) == 8 &&
                      strcmp(hole_name, "the hole") == 0,
                  "the volume's name reads back");
            check(hfw_plate_object_volume_visible(document, first, hole) == 1,
                  "a volume is visible when it is attached");
            check(hfw_plate_object_volume_set_visible(document, first, hole, 0) == 0 &&
                      hfw_plate_object_volume_visible(document, first, hole) == 0,
                  "a volume is hidden");
            check(hfw_plate_object_remove_volume(document, first, hole) == 0,
                  "the volume detaches");
            check(hfw_plate_object_volume_visible(document, first, hole) == -1,
                  "a removed volume cannot be asked about");

            check(hfw_plate_object_label(document, first, "left wing") == 0,
                  "an object is labelled");
            check(hfw_plate_object_name(document, first, "bracket") == 0,
                  "and keeps a name of its own");
            char label[32];
            check(hfw_plate_object_label_get(document, first, label, sizeof label) == 9 &&
                      strcmp(label, "left wing") == 0,
                  "the label reads back apart from the name");
            char object_name[32];
            check(hfw_plate_object_name_get(document, first, object_name, sizeof object_name) == 7 &&
                      strcmp(object_name, "bracket") == 0,
                  "the name a label does not replace is still there");
            check(hfw_plate_object_source(document, first, "/tmp/bracket.stl") == 0 &&
                      hfw_plate_object_source_get(document, first, label, sizeof label) == 16 &&
                      strcmp(label, "/tmp/bracket.stl") == 0,
                  "an object records where it came from");
            check(hfw_plate_object_dialect(document, first) == HFW_DIALECT_ENGINE &&
                      hfw_plate_object_set_dialect(document, first, HFW_DIALECT_ORCA) == 0 &&
                      hfw_plate_object_dialect(document, first) == HFW_DIALECT_ORCA,
                  "an object remembers which dialect its settings are named in");
            check(hfw_plate_object_apply_foreign(document, first, "sparse_infill_density", "40%") == 0,
                  "a setting the engine knows is applied");
            check(hfw_plate_object_apply_foreign(document, first, "spaghetti_detector", "3") == 1,
                  "a name it does not know is kept instead");
            char *kept = hfw_plate_object_foreign_json(document, first);
            check(kept != NULL && strstr(kept, "\"reason\":\"unknown_name\"") != NULL &&
                      strstr(kept, "\"value\":\"3\"") != NULL,
                  "the kept setting carries its own text and the reason it was dropped");
            hfw_string_free(kept);
            check(hfw_plate_object_foreign_clear(document, first) == 0,
                  "a re-import starts from nothing");

        hfw_plate_free(document);
    }

    check(hfw_mesh_load_file("/nonexistent/model.stl") == NULL, "a missing file is refused");
    check(hfw_last_error() != NULL, "the refusal left a message");

    hfw_mesh_free(mesh);

    if (failures != 0) {
        printf("C ABI: %d check(s) failed\n", failures);
        return 1;
    }
    printf("C ABI: every check passed\n");
    return 0;
}
