"""Export a Blender asset to the engine's runtime glTF, headless, by the export convention.

    blender --background --python-exit-code 1 <asset>.blend --python scripts/blender/export_glb.py -- \
        --out <dir> [--asset NAME] [--collection NAME] [--hover METRES] [--notes TEXT] [--no-quantise]

The convention is in docs/content-authoring.md, "Blender export (A1)": metres, Z up in Blender
(+Y up in the file), the model faces -Y in Blender (+Z in the engine), feet on z = 0; triangulated
meshes; one UV set read (TEXCOORD_0); baked MikkTSpace TANGENT (ADR-0022 amendment 1); material
slots named `<asset>` or `<asset>-<suffix>` for shading models, never texture sets; no images,
vertex colours, Draco or meshopt; joint names by the engine's rule, at most 32 per armature;
clips sampled at 30 fps (clip names are free: `animations/<asset>.toml` maps them to engine states,
ADR-0012); no mirrored (negative) scale; corner normals quantised to the glTF exporter's precision
before tangents are computed (`quantised_normals`, ADR-0022 amendment 3).

Writes `<out>/<asset>.glb` and `<out>/<asset>.provenance-stub.toml` (a draft ADR-0022 entry to
finish by hand). Exits 1, writing nothing, when the scene breaks the convention or the script
fails: the file is exported under a temporary name, its glTF JSON checked, and only then renamed.
`--python-exit-code 1` also makes Blender exit 1 if the script cannot even start. Then run the
engine's own checks on it:
`content-check --source <asset>.source.toml` (loader limits, baked tangents, feet, texel density).

A script that calls the helpers itself (`check_scene`, `gltf_options`, `verify_glb`) instead of
`main` must run the export, and the normal-map bake that goes with it, inside
`with quantised_normals(objects):`, as `main` does (see that function).
"""
import contextlib
import json
import os
import struct
import sys
import traceback

import bpy
import numpy as np

try:
    from io_scene_gltf2.io.com.constants import ROUNDING_DIGIT
except ImportError:  # the exporter's own value (4) in every Blender this convention supports
    ROUNDING_DIGIT = 4
    print("export_glb: warning: io_scene_gltf2's ROUNDING_DIGIT not found; assuming 4 (check "
          "that the glTF exporter still rounds normals to 4 decimals)", flush=True)

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from moba_blender import argv_after_dashdash, valid_name  # noqa: E402

# Per armature: the Low tier's skin cap. Stricter than `crates/assets/src/budget.rs`, which only
# marks a model over 32 joints as Over on Low and lets Medium (48) and High (64) take it: an
# export the Low tier cannot show is refused here.
MAX_JOINTS = 32
MAX_VERTICES = 65536       # ADR-0007 loader limits
MAX_INDICES = 393216
MAX_GLB_BYTES = 16 << 20
MAX_JSON_BYTES = 1 << 20


def parse_args(argv):
    opts = {"out": None, "asset": None, "collection": None, "hover": 0.0, "notes": None,
            "no_quantise": False}
    i = 0
    while i < len(argv):
        key = argv[i]
        if key == "--no-quantise":
            opts["no_quantise"] = True
            i += 1
        elif key in ("--out", "--asset", "--collection", "--hover", "--notes") and i + 1 < len(argv):
            opts[key[2:]] = argv[i + 1]
            i += 2
        else:
            raise SystemExit(f"export_glb: unknown argument {key!r}")
    if not opts["out"]:
        raise SystemExit("export_glb: --out <dir> is required")
    opts["hover"] = float(opts["hover"])
    if not opts["asset"]:
        opts["asset"] = os.path.splitext(os.path.basename(bpy.data.filepath))[0]
    return opts


def export_objects(collection):
    """Objects to export: the named collection's, else every visible mesh and armature outside a
    collection whose name starts with HIGH (bake sources stay in the .blend)."""
    if collection:
        coll = bpy.data.collections.get(collection)
        if coll is None:
            raise SystemExit(f"export_glb: no collection {collection!r}")
        objs = list(coll.all_objects)
    else:
        objs = [o for o in bpy.context.scene.objects
                if not any(c.name.upper().startswith("HIGH") for c in o.users_collection)
                and o.visible_get()]
    return [o for o in objs if o.type in ("MESH", "ARMATURE")]


def check_scene(opts, objs):
    """The convention checks; returns (errors, warnings)."""
    errors, warnings = [], []
    scene = bpy.context.scene
    asset = opts["asset"]
    if not valid_name(asset, 16):
        errors.append(f"asset name {asset!r} is not 1..=16 of [A-Za-z0-9_-]")
    if scene.unit_settings.system not in ("METRIC", "NONE") or abs(scene.unit_settings.scale_length - 1.0) > 1e-6:
        errors.append("scene units must be metres with unit scale 1.0")
    if scene.render.fps != 30 or scene.render.fps_base != 1.0:
        errors.append(f"scene frame rate is {scene.render.fps / scene.render.fps_base:g} fps; clips are sampled at 30")
    meshes = [o for o in objs if o.type == "MESH"]
    if not meshes:
        errors.append("nothing to export: no visible mesh outside HIGH collections")
    for o in objs:
        scale = o.matrix_world.to_scale()
        if o.matrix_world.to_3x3().determinant() < 0:
            errors.append(f"{o.name}: mirrored (negative) scale; flips its winding and tangent "
                          "handedness (apply it, then recalculate normals)")
        elif any(abs(s - 1.0) > 1e-4 for s in scale):
            errors.append(f"{o.name}: unapplied scale {tuple(round(s, 4) for s in scale)} (apply it)")
    # Feet are measured in the rest pose at frame 0, not wherever the timeline or an action
    # left the rig.
    armatures = [o for o in objs if o.type == "ARMATURE"]
    saved_frame = scene.frame_current
    saved_pose = {o.name: o.data.pose_position for o in armatures}
    for o in armatures:
        o.data.pose_position = "REST"
    scene.frame_set(0)
    depsgraph = bpy.context.evaluated_depsgraph_get()
    min_z = float("inf")
    try:
        min_z = mesh_checks(meshes, depsgraph, asset, errors, warnings)
    finally:
        for o in armatures:
            o.data.pose_position = saved_pose[o.name]
        scene.frame_set(saved_frame)
    if meshes and abs(min_z - opts["hover"]) > 0.01:
        errors.append(f"lowest vertex at z = {min_z:.3f} m in the rest pose at frame 0; the convention "
                      f"is z = {opts['hover']:.3f} (feet on the ground, or --hover)")
    for o in armatures:
        names = [b.name for b in o.data.bones]
        if len(names) > MAX_JOINTS:
            errors.append(f"armature {o.name}: {len(names)} joints; the exporter refuses over "
                          f"{MAX_JOINTS} per skin (the Low tier's cap, stricter than budget.rs)")
        bad = [j for j in names if not valid_name(j, 32)]
        if bad:
            errors.append(f"armature {o.name}: joint names outside [A-Za-z0-9_-]{{1,32}}: {bad}")
    clips = sorted(a.name for a in bpy.data.actions if a.users)
    if clips:
        warnings.append(f"clips {clips}: names are free; map them to engine states in "
                        f"animations/{asset}.toml (ADR-0012)")
    return errors, warnings


def mesh_checks(meshes, depsgraph, asset, errors, warnings):
    """Triangles, UVs, colour attributes and slot names; returns the lowest world z."""
    min_z = float("inf")
    for o in meshes:
        ev = o.evaluated_get(depsgraph)
        me = ev.to_mesh()
        try:
            bad = sum(1 for p in me.polygons if p.loop_total != 3)
            if bad:
                errors.append(f"{o.name}: {bad} faces are not triangles (triangulate before baking)")
            if not me.uv_layers:
                errors.append(f"{o.name}: no UV map (TEXCOORD_0 is required)")
            elif len(me.uv_layers) > 1:
                warnings.append(f"{o.name}: {len(me.uv_layers)} UV maps; the engine reads only the first")
            if me.color_attributes:
                warnings.append(f"{o.name}: colour attributes are not exported")
            for v in me.vertices:
                min_z = min(min_z, (ev.matrix_world @ v.co).z)
        finally:
            ev.to_mesh_clear()
        for slot in o.material_slots:
            name = slot.material.name if slot.material else ""
            if not (name == asset or (name.startswith(asset + "-") and valid_name(name, 32))):
                errors.append(f"{o.name}: material slot {name!r} is not `{asset}` or `{asset}-<suffix>`")
    return min_z


def exporter_normals(normals):
    """The glTF exporter's treatment of corner normals (io_scene_gltf2 `primitive_extract.py`,
    `__get_normals`): float32, rounded to `ROUNDING_DIGIT` decimals, then renormalised. Negative
    zeros are made positive, so every weld (Blender's, the exporter's de-duplication and the
    engine's bitwise MikkTSpace weld) sees one zero."""
    q = np.round(np.asarray(normals, dtype=np.float32), ROUNDING_DIGIT)
    norms = np.linalg.norm(q, axis=1, keepdims=True)
    np.divide(q, norms, out=q, where=norms != 0)
    q[q == 0.0] = 0.0
    return q


def quantise(normals, rounds=8):
    """`normals` moved to a fixed point of `exporter_normals`, so the exporter writes each one
    unchanged and two distinct normals here can never become one written normal."""
    q = exporter_normals(normals)
    for _ in range(rounds):
        again = exporter_normals(q)
        if np.array_equal(again.view(np.uint32), q.view(np.uint32)):
            return q
        q = again
    raise RuntimeError("corner normals did not settle under the exporter's rounding")


# The `custom_normal` attribute types a mesh can carry: foreach key, width, numpy type. Free
# normals are FLOAT_VECTOR (on the corner, point or face domain), tangent-space ones INT16_2D.
_CUSTOM_NORMAL_TYPES = {
    "FLOAT_VECTOR": ("vector", 3, np.float32),
    "INT16_2D": ("value", 2, np.int32),
}

# The guard's tolerance between the normals the exporter would read without and with the block:
# about 0.01 degrees, above quantisation's own change (at most about 0.005 degrees).
_GUARD_DEGREES = 0.01


def _corner_normals(me):
    corners = len(me.loops)
    out = np.empty(corners * 3, dtype=np.float32)
    me.corner_normals.foreach_get("vector", out)
    return out.reshape(corners, 3)


def exported_normals(obj):
    """The corner normals the glTF exporter reads for `obj` (io_scene_gltf2 `nodes.py`, with
    `export_apply` and skins on): the mesh data itself when the object has no modifiers, else
    the evaluated mesh with its Armature modifiers hidden, shape keys and other modifiers
    applied."""
    if not obj.modifiers:
        return _corner_normals(obj.data)
    hidden = [(m, m.show_viewport) for m in obj.modifiers if m.type == "ARMATURE"]
    for m, _ in hidden:
        m.show_viewport = False
    try:
        evaluated = obj.evaluated_get(bpy.context.evaluated_depsgraph_get())
        me = evaluated.to_mesh()
        try:
            return _corner_normals(me)
        finally:
            evaluated.to_mesh_clear()
    finally:
        for m, shown in hidden:
            m.show_viewport = shown


def _normal_changers(obj):
    """What on `obj` can change its exported normals: enabled non-Armature modifiers and shape
    keys with a non-zero value."""
    names = [f"modifier {m.name!r} ({m.type})" for m in obj.modifiers
             if m.type != "ARMATURE" and m.show_viewport]
    keys = obj.data.shape_keys
    if keys is not None:
        names += [f"shape key {k.name!r} = {k.value:g}" for k in keys.key_blocks
                  if k != keys.reference_key and k.value != 0.0]
    return ", ".join(names) or "its modifier stack"


def _merged_corners(me, raw, q):
    """Corners whose normal the rounding made equal to another corner's at the same vertex and
    UV: where Blender's tangent groups merge, and the tangents change from an unquantised
    export."""
    vertex = np.empty(len(me.loops), dtype=np.int32)
    me.loops.foreach_get("vertex_index", vertex)
    columns = [vertex.view(np.uint32)[:, None]]
    if me.uv_layers:
        uv = np.empty(len(me.loops) * 2, dtype=np.float32)
        me.uv_layers[0].data.foreach_get("uv", uv)
        columns.append(uv.reshape(-1, 2).view(np.uint32))

    def distinct(normals):
        keys = np.hstack(columns + [np.ascontiguousarray(normals).view(np.uint32)])
        return len(np.unique(keys, axis=0))

    return distinct(raw) - distinct(q)


def _check_exported(obj, before):
    """The guard: within the block, the exporter must read the quantised normals, and they must
    be the normals it would have read without the block, to 0.01 degrees."""
    after = exported_normals(obj)
    fix = ("apply the modifiers that change normals (or set the shape keys to 0) before "
           "exporting, or export with --no-quantise (ADR-0022 amendment 3)")
    corners = len(obj.data.loops)
    if len(after) != corners or len(before) != corners:
        raise RuntimeError(f"{obj.name}: {_normal_changers(obj)} changes topology (the corner "
                           f"count: {corners} in the mesh, {len(after)} exported), so its "
                           "normals cannot be quantised before tangents; apply it before "
                           "exporting (a Triangulate: the mesh must already be triangulated), "
                           "or export with --no-quantise (ADR-0022 amendment 3)")
    # In float64: in float32, arccos near 1 cannot resolve angles under about 0.02 degrees.
    a, b = before.astype(np.float64), after.astype(np.float64)
    lengths = np.linalg.norm(a, axis=1) * np.linalg.norm(b, axis=1)
    angle = np.arctan2(np.linalg.norm(np.cross(a, b), axis=1), np.einsum("ij,ij->i", a, b))
    angle[(lengths == 0) & np.any(before != after, axis=1)] = np.pi
    worst = float(np.degrees(angle.max(initial=0.0)))
    if worst > _GUARD_DEGREES:
        raise RuntimeError(f"{obj.name}: {_normal_changers(obj)} changes the corner normals "
                           f"(up to {worst:.2f} degrees from the quantised mesh normals), so they "
                           f"cannot be quantised before tangents; {fix}")
    if not np.array_equal(exporter_normals(after).view(np.uint32), after.view(np.uint32)):
        raise RuntimeError(f"{obj.name}: {_normal_changers(obj)} moves the quantised normals off "
                           f"the exporter's rounding; {fix}")


@contextlib.contextmanager
def quantised_normals(objs):
    """Within the block, the corner normals of every mesh in `objs` are quantised to the glTF
    exporter's precision (`quantise`) and stored exactly, as free float3 custom normals.

    Blender's MikkTSpace (`Mesh.calc_tangents`, which the exporter calls) welds corners whose
    position, normal and UV are exactly equal. The exporter then rounds normals to 4 digits, so
    corners whose normals differed by less than 1e-4 become identical in the file except for
    TANGENT: the file holds split vertices, and the engine's MikkTSpace on it (GLB-MIKK) welds
    them and disagrees. Quantising first makes Blender's weld see the normals the file holds.
    Cycles bakes against the same tangents, so bake the normal map inside this block too
    (`bake_normals`), or the map will not match the exported tangents where groups merge
    (ADR-0022 amendment 3).

    The normals quantised are the mesh data's, before modifiers and shape keys. The guard then
    evaluates each object as the exporter will and raises, naming the object and its modifiers
    or shape keys, unless the exporter would read the quantised normals, unchanged from what it
    would read without the block to 0.01 degrees, with the same corner count (a Mirror, Edge
    Split, Smooth by Angle, Weighted Normal, Bend, Subdivision or Triangulate modifier, or a
    non-zero shape key with modifiers, fails it). Armature modifiers are hidden for this, as the
    exporter hides them for skinned meshes.

    In memory only: on exit each mesh gets its previous `custom_normal` back (or none), also
    after an exception. Do not save the .blend inside the block: that makes the quantised free
    normals permanent, and free normals do not follow deformation, so posed renders would shade
    with rest-pose normals. Save baked images with `image.save()` instead. Yields a dict:
    `moved`, the corners whose normal the rounding changed, and `merged`, the corners it made
    equal to another at the same vertex and UV (where tangents change)."""
    saved = []
    stats = {"moved": 0, "merged": 0}
    try:
        meshes = [o for o in objs if o.type == "MESH"]
        before = {o.name: exported_normals(o) for o in meshes}
        for me in {o.data for o in meshes}:
            raw = _corner_normals(me)
            old = me.attributes.get("custom_normal")
            snapshot = None
            if old is not None:
                if old.data_type not in _CUSTOM_NORMAL_TYPES or (
                        old.data_type == "INT16_2D" and old.domain != "CORNER"):
                    raise RuntimeError(f"{me.name}: custom_normal is {old.data_type} on "
                                       f"{old.domain}, not a custom normal")
                key, width, kind = _CUSTOM_NORMAL_TYPES[old.data_type]
                values = np.empty(len(old.data) * width, dtype=kind)
                old.data.foreach_get(key, values)
                snapshot = (old.data_type, old.domain, key, values)
            q = quantise(raw)
            if old is not None:
                me.attributes.remove(old)
            saved.append((me, snapshot))
            stats["moved"] += int(np.count_nonzero(
                np.any(q.view(np.uint32) != raw.view(np.uint32), axis=1)))
            stats["merged"] += _merged_corners(me, raw, q)
            me.attributes.new("custom_normal", "FLOAT_VECTOR", "CORNER").data.foreach_set(
                "vector", q.ravel())
            me.update()
        for o in meshes:
            _check_exported(o, before[o.name])
        yield stats
    finally:
        failures = []
        for me, snapshot in saved:
            try:
                if "custom_normal" in me.attributes:
                    me.attributes.remove(me.attributes["custom_normal"])
                if snapshot is not None:
                    data_type, domain, key, values = snapshot
                    me.attributes.new("custom_normal", data_type, domain).data.foreach_set(
                        key, values)
                me.update()
            except Exception as e:  # noqa: BLE001 - restore every other mesh, then report
                failures.append(f"{me.name}: {e}")
        if failures:
            raise RuntimeError(f"custom normals not restored: {'; '.join(failures)}")


def bake_normals(objs, **bake):
    """Cycles' tangent-space normal bake inside `quantised_normals(objs)`, so the map matches
    the tangents the export writes. The caller sets up the bake as for `bpy.ops.object.bake`
    (Cycles, the low-poly object active, an active Image Texture node in its material, and for
    selected-to-active the high-poly sources selected) and saves the image afterwards with
    `image.save()`. `bake` is passed on, for example `use_selected_to_active=True,
    cage_extrusion=0.02`; `normal_space` defaults to "TANGENT". The bake runs in the rest
    pose, as `check_scene` measures it (free normals do not follow deformation): every armature
    in `objs` or deforming one of them is set to `pose_position = "REST"` for the bake and
    restored afterwards. Returns the `quantised_normals` stats."""
    bake.setdefault("normal_space", "TANGENT")
    armatures = {o for o in objs if o.type == "ARMATURE"}
    armatures |= {m.object for o in objs for m in o.modifiers
                  if m.type == "ARMATURE" and m.object is not None}
    saved = [(a, a.data.pose_position) for a in armatures]
    try:
        for a, _ in saved:
            a.data.pose_position = "REST"
        with quantised_normals(objs) as stats:
            bpy.ops.object.bake(type="NORMAL", **bake)
    finally:
        for a, pose in saved:
            a.data.pose_position = pose
    return stats


def gltf_options(path):
    """The exporter's keyword arguments, keeping only those this Blender's exporter knows."""
    wanted = {
        "filepath": path, "export_format": "GLB", "use_selection": True, "export_yup": True,
        "export_apply": True, "export_texcoords": True, "export_normals": True,
        "export_tangents": True, "export_materials": "EXPORT", "export_image_format": "NONE",
        "export_vertex_color": "NONE", "export_colors": False, "export_attributes": False,
        "export_skins": True, "export_all_influences": False, "export_animations": True,
        "export_animation_mode": "ACTIONS", "export_force_sampling": True, "export_frame_step": 1,
        "export_optimize_animation_size": False, "export_draco_mesh_compression_enable": False,
        "export_cameras": False, "export_lights": False, "export_extras": False,
        "export_morph": False,
    }
    known = bpy.ops.export_scene.gltf.get_rna_type().properties.keys()
    return {k: v for k, v in wanted.items() if k in known}


def verify_glb(path):
    """Reads the written file's glTF JSON back: the convention's file-level rules."""
    errors = []
    with open(path, "rb") as f:
        data = f.read()
    if len(data) < 20:
        return ["shorter than a GLB header"], {}
    magic, version, length = struct.unpack_from("<III", data, 0)
    if magic != 0x46546C67 or version != 2 or length != len(data):
        return ["not a GLB v2 file"], {}
    jlen, _ = struct.unpack_from("<II", data, 12)
    doc = json.loads(data[20:20 + jlen])
    if len(data) > MAX_GLB_BYTES:
        errors.append(f"{len(data)} B: over the loader's 16 MiB")
    if jlen > MAX_JSON_BYTES:
        errors.append(f"JSON chunk {jlen} B: over the loader's 1 MiB")
    if doc.get("extensionsRequired"):
        errors.append(f"extensionsRequired {doc['extensionsRequired']}")
    if doc.get("images"):
        errors.append(f"{len(doc['images'])} embedded images (the runtime file carries none)")
    acc = doc.get("accessors", [])
    if any("sparse" in a for a in acc):
        errors.append("sparse accessors")
    verts = idx = 0
    for mesh in doc.get("meshes", []):
        for prim in mesh["primitives"]:
            at = prim["attributes"]
            verts += acc[at["POSITION"]]["count"]
            if "indices" in prim:
                idx += acc[prim["indices"]]["count"]
            for need in ("NORMAL", "TEXCOORD_0", "TANGENT"):
                if need not in at:
                    errors.append(f"mesh {mesh.get('name')!r}: a primitive without {need}")
            if any(k.startswith("COLOR_") for k in at):
                errors.append(f"mesh {mesh.get('name')!r}: vertex colours")
    if verts > MAX_VERTICES:
        errors.append(f"{verts} vertices: over the loader's {MAX_VERTICES}")
    if idx > MAX_INDICES:
        errors.append(f"{idx} indices: over the loader's {MAX_INDICES}")
    splits = None
    if not errors:
        try:
            splits = tangent_splits(doc, glb_bin(data, jlen))
        except ValueError as e:
            errors.append(f"cannot read the vertex streams back: {e}")
    return errors, {"bytes": len(data), "vertices": verts, "triangles": idx // 3,
                    "tangent_splits": splits,
                    "joints": [len(s["joints"]) for s in doc.get("skins", [])],
                    "clips": [a.get("name") for a in doc.get("animations", [])],
                    "materials": [m.get("name") for m in doc.get("materials", [])]}


def glb_bin(data, jlen):
    """The GLB's BIN chunk (the bytes after the JSON chunk), or b"" without one."""
    start = 20 + ((jlen + 3) & ~3)
    if start + 8 > len(data):
        return b""
    blen, kind = struct.unpack_from("<II", data, start)
    if kind != 0x004E4942 or start + 8 + blen > len(data):
        raise ValueError("bad BIN chunk")
    return data[start + 8:start + 8 + blen]


def float_stream(doc, bin_chunk, index, width):
    """Accessor `index` as a (count, width) float32 array (FLOAT components, BIN chunk only)."""
    a = doc["accessors"][index]
    if a.get("componentType") != 5126 or "bufferView" not in a:
        raise ValueError(f"accessor {index} is not float")
    view = doc["bufferViews"][a["bufferView"]]
    stride = view.get("byteStride", 4 * width)
    start = view.get("byteOffset", 0) + a.get("byteOffset", 0)
    count = a["count"]
    if count < 1 or stride < 4 * width or start + (count - 1) * stride + 4 * width > len(bin_chunk):
        raise ValueError(f"accessor {index} is out of the BIN chunk")
    return np.ndarray((count, width), dtype="<f4", buffer=bin_chunk, offset=start,
                      strides=(stride, 4))


def tangent_splits(doc, bin_chunk):
    """Vertices in the file that share POSITION, NORMAL and TEXCOORD_0 (bit for bit, within one
    glTF mesh; the engine merges every mesh of the file and welds across them, which this does
    not count) with another vertex but have a different TANGENT: the symptom of tangents computed
    on normals other than the written ones, which the engine's MikkTSpace welds and averages
    (GLB-MIKK). Information, an upper bound: MikkTSpace itself can give one welded vertex two
    tangents (two fans of triangles meeting only at that vertex, by its grouping), and the engine
    reproduces those. The Void Monolith (A3) exports 1032 without `quantised_normals` and 2 with
    it, at GLB-MIKK 1.0000 (ADR-0022 amendment 3)."""
    split = 0
    for mesh in doc.get("meshes", []):
        groups = {}
        for prim in mesh["primitives"]:
            at = prim["attributes"]
            streams = [float_stream(doc, bin_chunk, at[name], width) for name, width in
                       (("POSITION", 3), ("NORMAL", 3), ("TEXCOORD_0", 2), ("TANGENT", 4))]
            keys = np.ascontiguousarray(np.hstack(streams[:3]) + np.float32(0.0)).view(np.uint32)
            tangents = np.ascontiguousarray(streams[3]).view(np.uint32)
            for key, tangent in zip(map(bytes, keys), map(bytes, tangents)):
                groups.setdefault(key, []).append(tangent)
        split += sum(len(ts) for ts in groups.values() if len(set(ts)) > 1)
    return split


def toml_string(text):
    return json.dumps(text, ensure_ascii=False)


def provenance_stub(opts, info):
    asset = opts["asset"]
    blend = os.path.basename(bpy.data.filepath) or "an unsaved scene"
    version = ".".join(str(v) for v in bpy.app.version)
    notes = opts["notes"] or "REPLACE: the reference board or model sheet this was modelled from"
    return (
        f"# Draft ADR-0022 entry for models/{asset}.glb, written by scripts/blender/export_glb.py.\n"
        "# Merge it into content/models/provenance.toml and finish `notes` by hand. It is not a\n"
        "# provenance.toml itself, so the gate never reads this file.\n"
        f"# Export: {info['vertices']} vertices, {info['triangles']} triangles, joints "
        f"{json.dumps(info['joints'])}, clips {json.dumps(info['clips'])}.\n"
        "[[asset]]\n"
        f"files = [{toml_string(asset + '.glb')}, {toml_string(asset + '.lod')}]\n"
        f"title = {toml_string(asset)}\n"
        'source = "original"\n'
        'author = "MOBA Engine contributors"\n'
        'licence = "MIT OR Apache-2.0"\n'
        f"made_with = {toml_string(f'Blender {version} (scripts/blender/export_glb.py) from {blend}')}\n"
        f"notes = {toml_string(notes)}\n"
    )


def main():
    opts = parse_args(argv_after_dashdash())
    objs = export_objects(opts["collection"])
    errors, warnings = check_scene(opts, objs)
    for w in warnings:
        print(f"export_glb: warning: {w}", flush=True)
    if errors:
        for e in errors:
            print(f"export_glb: error: {e}", flush=True)
        sys.exit(1)
    os.makedirs(opts["out"], exist_ok=True)
    path = os.path.join(opts["out"], opts["asset"] + ".glb")
    # Exported under a temporary name and renamed only once verified, so an error or an
    # exception never leaves an unchecked file at `path`.
    partial = os.path.join(opts["out"], "." + opts["asset"] + ".partial.glb")
    try:
        bpy.ops.object.select_all(action="DESELECT")
        for o in objs:
            o.select_set(True)
        quantising = (contextlib.nullcontext({"moved": 0, "merged": 0}) if opts["no_quantise"]
                      else quantised_normals(objs))
        try:
            with quantising as stats:
                bpy.ops.export_scene.gltf(**gltf_options(partial))
        except RuntimeError as e:
            print(f"export_glb: error: {e}", flush=True)
            sys.exit(1)
        errors, info = verify_glb(partial)
        info["normals_quantised"] = stats["moved"]
        info["normal_groups_merged"] = stats["merged"]
        if opts["no_quantise"]:
            print("export_glb: warning: --no-quantise: tangents computed on unrounded normals, as "
                  "before ADR-0022 amendment 3; the file may hold vertices that differ only in "
                  f"TANGENT ({info.get('tangent_splits')}) and GLB-MIKK may disagree", flush=True)
        elif stats["merged"]:
            print(f"export_glb: note: {stats['merged']} corners merged; tangents follow the "
                  "quantised normals, so the normal map must be baked inside quantised_normals "
                  "(bake_normals); ignore this if it was", flush=True)
        if errors:
            for e in errors:
                print(f"export_glb: error in {path}: {e}", flush=True)
            sys.exit(1)
        os.replace(partial, path)
    finally:
        if os.path.exists(partial):
            os.remove(partial)
    stub = os.path.join(opts["out"], opts["asset"] + ".provenance-stub.toml")
    with open(stub, "w", encoding="utf-8", newline="\n") as f:
        f.write(provenance_stub(opts, info))
    print(f"export_glb: wrote {path} ({json.dumps(info)}) and {stub}", flush=True)


if __name__ == "__main__":
    # Blender keeps running (and exits 0) after an uncaught exception in a --python script.
    try:
        main()
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001 - any failure must be exit 1
        traceback.print_exc()
        sys.exit(1)
