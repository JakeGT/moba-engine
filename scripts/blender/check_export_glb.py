"""Headless self-check of export_glb.py's normal quantisation (ADR-0022 amendment 3). Run by hand;
CI has no Blender:

    blender --background --factory-startup --python-exit-code 1 \
        --python scripts/blender/check_export_glb.py -- --out <scratch dir> [--content-check <bin>]

Writes only under --out. With --content-check (a built `content-check`), the repro's GLB-MIKK is
read through `content-check --source` as well. Prints one PASS or FAIL line per check and exits 1
on any failure. Checks:

- the minimal repro (one flat quad 1e-5 m out of plane, skewed UVs; art-source
  `checks/results/2026-11-15-followup/mikk_repro.py`): 6 vertices and 4 tangent-split vertices
  exported without `quantised_normals` (GLB-MIKK 0.3333), 4 and 0 inside it (GLB-MIKK 1.0000);
- the block restores each mesh's custom normals exactly: none, FLOAT_VECTOR on the corner,
  point and face domains, INT16_2D (tangent space), mesh data shared by two objects, and
  after an exception inside the block;
- the guard raises, naming the cause, for an Edge Split modifier, a Mirror modifier and
  Subdivision + Triangulate (corner count), a Weighted Normal modifier and a shape key under an
  Armature modifier, and passes an Armature modifier alone.
"""
import os
import re
import subprocess
import sys

import bmesh
import bpy
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import export_glb as EG  # noqa: E402
from moba_blender import argv_after_dashdash  # noqa: E402

FAILURES = []


def check(name, ok, detail=""):
    print(f"{'PASS' if ok else 'FAIL'}  {name}{f'  ({detail})' if detail else ''}", flush=True)
    if not ok:
        FAILURES.append(name)


def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.context.scene.render.fps = 30


def link(me, name):
    ob = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(ob)
    return ob


def repro_quad(name="mikk-repro"):
    """mikk_repro.py's quad: flat, the fourth corner 1e-5 m up, UVs skewed ~11 degrees."""
    me = bpy.data.meshes.new(name)
    bm = bmesh.new()
    vs = [bm.verts.new(p) for p in ((0, 0, 0), (1, 0, 0), (1, 1, 1e-5), (0, 1, 0))]
    face = bm.faces.new(vs)
    face.smooth = False
    uv = bm.loops.layers.uv.new("UVMap")
    for loop, st in zip(face.loops, ((0, 0), (1, 0.2), (1, 1), (0.3, 1))):
        loop[uv].uv = st
    bmesh.ops.triangulate(bm, faces=[face])
    bm.to_mesh(me)
    bm.free()
    me.materials.append(bpy.data.materials.new(name))
    return link(me, name)


def sphere(name, smooth=True):
    me = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_uvsphere(bm, u_segments=8, v_segments=6, radius=1.0)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    for f in bm.faces:
        f.smooth = smooth
    bm.to_mesh(me)
    bm.free()
    return link(me, name)


def export(ob, path, quantised):
    for o in bpy.context.scene.objects:
        o.select_set(o == ob)
    bpy.context.view_layer.objects.active = ob
    if quantised:
        with EG.quantised_normals([ob]) as stats:
            bpy.ops.export_scene.gltf(**EG.gltf_options(path))
    else:
        stats = None
        bpy.ops.export_scene.gltf(**EG.gltf_options(path))
    errors, info = EG.verify_glb(path)
    return errors, info, stats


def mikk(content_check, out, glb):
    """GLB-MIKK's within-5-degrees share from `content-check --source` on `glb`."""
    base = os.path.join(out, "check-base.png")
    if not os.path.exists(base):
        img = bpy.data.images.new("check-base", 16, 16)
        img.pixels[:] = [0.5, 0.5, 0.5, 1.0] * 256
        img.filepath_raw = base
        img.file_format = "PNG"
        img.save()
    manifest = os.path.join(out, "check.source.toml")
    with open(manifest, "w", encoding="utf-8") as f:
        f.write(f'asset = "mikk-repro"\nclass = "prop"\n\n[[models]]\npath = {glb!r}\n\n'
                f'[maps]\nbase = {base!r}\n'.replace("'", '"'))
    run = subprocess.run([content_check, "--source", manifest], capture_output=True, text=True)
    found = re.search(r"GLB-MIKK \([^)]*\)\s+([0-9.]+) within", run.stdout)
    return float(found.group(1)) if found else None


def custom_state(me):
    """The mesh's custom_normal attribute (type, domain, raw values) and its corner normals."""
    a = me.attributes.get("custom_normal")
    attr = None
    if a is not None:
        key, width, kind = EG._CUSTOM_NORMAL_TYPES[a.data_type]
        values = np.empty(len(a.data) * width, dtype=kind)
        a.data.foreach_get(key, values)
        attr = (a.data_type, a.domain, values.tobytes())
    return attr, EG._corner_normals(me).tobytes()


def check_repro(out, content_check):
    for quantised, verts, splits, want in ((False, 6, 4, 0.3333), (True, 4, 0, 1.0)):
        reset()
        ob = repro_quad()
        path = os.path.join(out, f"mikk-repro-{'quantised' if quantised else 'raw'}.glb")
        errors, info, stats = export(ob, path, quantised)
        label = "inside quantised_normals" if quantised else "without quantised_normals"
        check(f"repro {label}: {verts} vertices, {splits} tangent-split",
              not errors and info["vertices"] == verts and info["tangent_splits"] == splits,
              f"{info.get('vertices')} vertices, {info.get('tangent_splits')} split, {errors}")
        if quantised:
            check("repro: 2 corners merged, 3 moved, custom normals gone after the block",
                  stats == {"moved": 3, "merged": 2} and "custom_normal" not in ob.data.attributes,
                  str(stats))
        if content_check:
            got = mikk(content_check, out, path)
            check(f"repro {label}: GLB-MIKK {want:.4f}",
                  got is not None and abs(got - want) < 1e-4, str(got))


def check_restore():
    reset()
    cases = {}
    cases["none"] = sphere("none")
    ob = sphere("float-corner")
    n = EG._corner_normals(ob.data) * np.float32(0.98) + np.float32(0.01)
    ob.data.attributes.new("custom_normal", "FLOAT_VECTOR", "CORNER").data.foreach_set(
        "vector", n.ravel())
    cases["FLOAT_VECTOR corner"] = ob
    ob = sphere("float-point")
    vn = np.array([v.normal[:] for v in ob.data.vertices], dtype=np.float32)
    ob.data.attributes.new("custom_normal", "FLOAT_VECTOR", "POINT").data.foreach_set(
        "vector", vn.ravel())
    cases["FLOAT_VECTOR point"] = ob
    ob = sphere("float-face")
    fn = np.array([f.normal[:] for f in ob.data.polygons], dtype=np.float32) * np.float32(0.97)
    ob.data.attributes.new("custom_normal", "FLOAT_VECTOR", "FACE").data.foreach_set(
        "vector", fn.ravel())
    cases["FLOAT_VECTOR face"] = ob
    ob = sphere("int16")
    rng = np.random.default_rng(1)
    custom = rng.normal(size=(len(ob.data.loops), 3)) * 0.2 + np.array([0.0, 0.0, 1.0])
    custom /= np.linalg.norm(custom, axis=1, keepdims=True)
    ob.data.normals_split_custom_set([tuple(c) for c in custom])
    cases["INT16_2D"] = ob
    shared = link(ob.data, "int16-twin")
    before = {k: custom_state(o.data) for k, o in cases.items()}
    objs = list(cases.values()) + [shared]
    with EG.quantised_normals(objs):
        inside = all(o.data.attributes["custom_normal"].data_type == "FLOAT_VECTOR" for o in objs)
    check("restore: every mesh quantised inside the block", inside)
    for k, o in cases.items():
        check(f"restore: {k} exact after the block", custom_state(o.data) == before[k])
    check("restore: shared mesh data (two objects) quantised and restored once",
          custom_state(shared.data) == before["INT16_2D"])
    try:
        with EG.quantised_normals(objs):
            raise KeyError("inside the block")
    except KeyError:
        pass
    check("restore: exact after an exception inside the block",
          all(custom_state(o.data) == before[k] for k, o in cases.items()))


def rig_for(ob, name="rig"):
    arm = bpy.data.armatures.new(name)
    rig = link(arm, name)
    bpy.context.view_layer.objects.active = rig
    bpy.ops.object.mode_set(mode="EDIT")
    bone = arm.edit_bones.new("b")
    bone.head, bone.tail = (0, 0, 0), (0, 0, 1)
    bpy.ops.object.mode_set(mode="OBJECT")
    group = ob.vertex_groups.new(name="b")
    group.add(list(range(len(ob.data.vertices))), 1.0, "REPLACE")
    mod = ob.modifiers.new("armature", "ARMATURE")
    mod.object = rig
    return rig


def guarded(ob):
    """The guard's error for `ob`, or None; checks the mesh is restored either way."""
    state = custom_state(ob.data)
    error = None
    try:
        with EG.quantised_normals([ob]):
            pass
    except RuntimeError as e:
        error = str(e)
    if custom_state(ob.data) != state:
        error = (error or "") + " [NOT RESTORED]"
    return error


def check_guard():
    reset()
    ob = sphere("edge-split")
    ob.modifiers.new("split", "EDGE_SPLIT").split_angle = np.radians(20.0)
    error = guarded(ob)
    check("guard: Edge Split raises, naming the modifier",
          error is not None and "'split' (EDGE_SPLIT)" in error and "NOT RESTORED" not in error,
          error)
    reset()
    ob = sphere("mirror")
    for v in ob.data.vertices:  # off the mirror plane, so the copy adds corners
        v.co.x += 3.0
    ob.modifiers.new("mirror", "MIRROR")
    error = guarded(ob)
    check("guard: Mirror raises on the corner count",
          error is not None and "corner count" in error and "NOT RESTORED" not in error, error)
    reset()
    ob = sphere("subsurf")
    ob.modifiers.new("subsurf", "SUBSURF")
    ob.modifiers.new("triangulate", "TRIANGULATE")
    error = guarded(ob)
    check("guard: Subdivision + Triangulate raises on the corner count",
          error is not None and "corner count" in error and "SUBSURF" in error, error)
    reset()
    ob = sphere("weighted")
    ob.modifiers.new("weighted", "WEIGHTED_NORMAL").weight = 100
    error = guarded(ob)
    check("guard: Weighted Normal raises, naming the modifier",
          error is not None and "WEIGHTED_NORMAL" in error, error)
    reset()
    ob = sphere("keyed")
    ob.shape_key_add(name="Basis")
    key = ob.shape_key_add(name="squash")
    for point in key.data:
        point.co.z *= 0.5
    key.value = 0.7
    rig_for(ob)
    error = guarded(ob)
    check("guard: a shape key at 0.7 under an Armature raises, naming the key",
          error is not None and "shape key 'squash' = 0.7" in error, error)
    reset()
    ob = sphere("armature-only")
    rig_for(ob)
    error = guarded(ob)
    check("guard: an Armature modifier alone passes", error is None, error)


def main():
    argv = argv_after_dashdash()
    out = argv[argv.index("--out") + 1] if "--out" in argv else None
    if not out:
        raise SystemExit("check_export_glb: --out <dir> is required")
    content_check = argv[argv.index("--content-check") + 1] if "--content-check" in argv else None
    os.makedirs(out, exist_ok=True)
    check_repro(out, content_check)
    check_restore()
    check_guard()
    print(f"check_export_glb: {len(FAILURES)} failed", flush=True)
    sys.exit(1 if FAILURES else 0)


if __name__ == "__main__":
    main()
