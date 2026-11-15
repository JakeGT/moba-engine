"""Export a Blender asset to the engine's runtime glTF, headless, by the export convention.

    blender --background <asset>.blend --python scripts/blender/export_glb.py -- \
        --out <dir> [--asset NAME] [--collection NAME] [--hover METRES] [--notes TEXT]

The convention is in docs/content-authoring.md, "Blender export (A1)": metres, Z up in Blender
(+Y up in the file), the model faces -Y in Blender (+Z in the engine), feet on z = 0; triangulated
meshes; one UV set read (TEXCOORD_0); baked MikkTSpace TANGENT (ADR-0022 amendment 1); material
slots named `<asset>` or `<asset>-<suffix>` for shading models, never texture sets; no images,
vertex colours, Draco or meshopt; joints and clips by the engine's names; clips sampled at 30 fps.

Writes `<out>/<asset>.glb` and `<out>/<asset>.provenance-stub.toml` (a draft ADR-0022 entry to
finish by hand). Exits 1, writing nothing, when the scene breaks the convention, and checks the
written file's glTF JSON afterwards. Then run the engine's own checks on it:
`content-check --source <asset>.source.toml` (loader limits, baked tangents, feet, texel density).
"""
import json
import os
import struct
import sys

import bpy

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from moba_blender import argv_after_dashdash, valid_name  # noqa: E402

CLIP_NAMES = {"idle", "run", "walk", "attack", "cast1", "cast2", "cast3", "death", "recall", "hit"}
MAX_JOINTS = 32            # the Low tier's skin cap (content-authoring.md, budgets)
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
    depsgraph = bpy.context.evaluated_depsgraph_get()
    min_z = float("inf")
    for o in objs:
        scale = o.matrix_world.to_scale()
        if any(abs(s - 1.0) > 1e-4 for s in scale):
            errors.append(f"{o.name}: unapplied scale {tuple(round(s, 4) for s in scale)} (apply it)")
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
    if meshes and abs(min_z - opts["hover"]) > 0.01:
        errors.append(f"lowest vertex at z = {min_z:.3f} m; the convention is z = {opts['hover']:.3f} "
                      "(feet on the ground, or --hover)")
    joints = [b.name for o in objs if o.type == "ARMATURE" for b in o.data.bones]
    if len(joints) > MAX_JOINTS:
        errors.append(f"{len(joints)} joints; the Low tier caps a skin at {MAX_JOINTS}")
    bad = [j for j in joints if not valid_name(j, 32)]
    if bad:
        errors.append(f"joint names outside [A-Za-z0-9_-]{{1,32}}: {bad}")
    for action in bpy.data.actions:
        if action.users and action.name not in CLIP_NAMES:
            warnings.append(f"clip {action.name!r} is not an engine state name ({', '.join(sorted(CLIP_NAMES))})")
    return errors, warnings


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
    data = open(path, "rb").read()
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
    return errors, {"bytes": len(data), "vertices": verts, "triangles": idx // 3,
                    "joints": [len(s["joints"]) for s in doc.get("skins", [])],
                    "clips": [a.get("name") for a in doc.get("animations", [])],
                    "materials": [m.get("name") for m in doc.get("materials", [])]}


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
    bpy.ops.object.select_all(action="DESELECT")
    for o in objs:
        o.select_set(True)
    bpy.ops.export_scene.gltf(**gltf_options(path))
    errors, info = verify_glb(path)
    if errors:
        for e in errors:
            print(f"export_glb: error in {path}: {e}", flush=True)
        os.remove(path)
        sys.exit(1)
    stub = os.path.join(opts["out"], opts["asset"] + ".provenance-stub.toml")
    with open(stub, "w", encoding="utf-8", newline="\n") as f:
        f.write(provenance_stub(opts, info))
    print(f"export_glb: wrote {path} ({json.dumps(info)}) and {stub}", flush=True)


main()
