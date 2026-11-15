"""Export a Blender asset to the engine's runtime glTF, headless, by the export convention.

    blender --background --python-exit-code 1 <asset>.blend --python scripts/blender/export_glb.py -- \
        --out <dir> [--asset NAME] [--collection NAME] [--hover METRES] [--notes TEXT]

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
    opts = {"out": None, "asset": None, "collection": None, "hover": 0.0, "notes": None}
    i = 0
    while i < len(argv):
        key = argv[i]
        if key in ("--out", "--asset", "--collection", "--hover", "--notes") and i + 1 < len(argv):
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


# The corner attribute types a mesh's `custom_normal` can have: foreach key, width, numpy type.
_CUSTOM_NORMAL_TYPES = {
    "FLOAT_VECTOR": ("vector", 3, np.float32),
    "INT16_2D": ("value", 2, np.int32),
}


@contextlib.contextmanager
def quantised_normals(objs):
    """Within the block, the corner normals of every mesh in `objs` are quantised to the glTF
    exporter's precision (`quantise`) and stored exactly, as free float3 custom normals.

    Blender's MikkTSpace (`Mesh.calc_tangents`, which the exporter calls) welds corners whose
    position, normal and UV are exactly equal. The exporter then rounds normals to 4 digits, so
    corners whose normals differed by less than 1e-4 become identical in the file except for
    TANGENT: the file holds split vertices, and the engine's MikkTSpace on it (GLB-MIKK) welds
    them and disagrees. Quantising first makes Blender weld what the file welds. Cycles bakes
    against the same tangents, so run the normal-map bake inside this block too, or the map will
    not match the exported tangents where groups merge (ADR-0022 amendment 3).

    In memory only: on exit each mesh gets its previous `custom_normal` back (or none). Do not
    save the .blend inside the block: free normals do not follow deformation, so posed renders
    would shade with rest-pose normals. Yields the number of corners whose normal the rounding
    changed."""
    saved = []
    moved = 0
    try:
        for me in {o.data for o in objs if o.type == "MESH"}:
            corners = len(me.loops)
            raw = np.empty(corners * 3, dtype=np.float32)
            me.corner_normals.foreach_get("vector", raw)
            raw = raw.reshape(corners, 3)
            old = me.attributes.get("custom_normal")
            snapshot = None
            if old is not None:
                if old.domain != "CORNER" or old.data_type not in _CUSTOM_NORMAL_TYPES:
                    raise RuntimeError(f"{me.name}: custom_normal is {old.data_type} on "
                                       f"{old.domain}, not a corner normal")
                key, width, kind = _CUSTOM_NORMAL_TYPES[old.data_type]
                values = np.empty(len(old.data) * width, dtype=kind)
                old.data.foreach_get(key, values)
                snapshot = (old.data_type, key, values)
            q = quantise(raw)
            if old is not None:
                me.attributes.remove(old)
            saved.append((me, snapshot))
            moved += int(np.count_nonzero(np.any(q.view(np.uint32) != raw.view(np.uint32), axis=1)))
            me.attributes.new("custom_normal", "FLOAT_VECTOR", "CORNER").data.foreach_set(
                "vector", q.ravel())
            me.update()
        yield moved
    finally:
        for me, snapshot in saved:
            if "custom_normal" in me.attributes:
                me.attributes.remove(me.attributes["custom_normal"])
            if snapshot is not None:
                data_type, key, values = snapshot
                me.attributes.new("custom_normal", data_type, "CORNER").data.foreach_set(key, values)
            me.update()


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
    glTF mesh) with another vertex but have a different TANGENT: the symptom of tangents computed
    on normals other than the written ones, which the engine's MikkTSpace welds and averages
    (GLB-MIKK). Information, an upper bound: MikkTSpace itself gives a vertex two tangents where
    two fans of its triangles meet only at that vertex, and the engine reproduces those. The Void
    Monolith (A3) exports 1032 without `quantised_normals` and 2 with it, at GLB-MIKK 1.0000
    (ADR-0022 amendment 3)."""
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
        with quantised_normals(objs) as moved:
            bpy.ops.export_scene.gltf(**gltf_options(partial))
        errors, info = verify_glb(partial)
        info["normals_quantised"] = moved
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
