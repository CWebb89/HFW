import os
import sys

import numpy as np
import trimesh

out = sys.argv[1] if len(sys.argv) > 1 else "fixtures/defects"
os.makedirs(out, exist_ok=True)


def drop(mesh, cx, tol):
    c = mesh.triangles_center
    keep = ~((c[:, 0] > cx) & (np.abs(c[:, 1]) < tol) & (np.abs(c[:, 2]) < tol))
    return trimesh.Trimesh(mesh.vertices, mesh.faces[keep], process=False)


drop(trimesh.creation.icosphere(subdivisions=5, radius=12), 10.5, 0.35).export(
    f"{out}/hole_small.stl"
)
drop(trimesh.creation.icosphere(subdivisions=4, radius=12), 8.0, 1.5).export(
    f"{out}/hole_large.stl"
)

a = trimesh.creation.box((20, 20, 20))
b = trimesh.creation.box((20, 20, 20))
b.apply_translation((8, 6, 4))
trimesh.util.concatenate([a, b]).export(f"{out}/overlap_boxes.stl")

t = trimesh.creation.box((20, 20, 20))
f = t.faces.copy()
f[:4] = f[:4][:, ::-1]
trimesh.Trimesh(t.vertices, f, process=False).export(f"{out}/flipped_faces.stl")

trimesh.creation.box((20, 20, 20)).export(f"{out}/cube_reference.stl")
