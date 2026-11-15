"""Review renders (roadmap §9.4): the raw frames `review-sheet` turns into contact sheets.

    flock /path/to/gpu.lock \
        blender --background --factory-startup --python-exit-code 1 \
        --python scripts/blender/review_render.py -- <asset>.review.toml [--samples N] [--cpu]

Then `review-sheet <out>/render_meta.json` checks the numbers and composes the sheets.

Everything is lit by the bible's Golden Crossing light (`moba_blender.py`), on a flat ash
ground (`#6B6560`) with a 1.8 m reference post, seen from the engine's default camera
(yaw -0.7, pitch 0.95, 46 m, 60 deg vertical FOV); the exposure is set so the bare ground reads
the measured gameplay mean OKLab L of 0.50 under AgX. These are Cycles renders, not the
engine's: no grading LUT, fog, shadow cascades or rims (ADR-0022 amendment 2 lists the
departures).

The review file (TOML; paths relative to it):

    name = "ember-scarab"                 # sheet name, [A-Za-z0-9_-]
    out = "../export/ember-scarab/review" # the frames and render_meta.json
    samples = 128                         # beauty samples (optional)

    [[models]]                            # 1 to 6 models, laid out left to right
    name = "ember-scarab"
    glb = "../export/ember-scarab/content/models/ember-scarab.glb"
    role = "neutral"                      # hero | minion | neutral | structure | prop
    size_class = "large-monster"          # optional: silhouettes-and-scale.md's classes
    faction = "neutral"                   # forge | void | neutral
    hover_m = 0.0                         # optional
    pose = { clip = "idle", frame = 0 }   # optional; the bind pose otherwise
    tell = ["crest"]                      # optional: joints of the faction tell (SIL-5, SIL-6)
    weapon = ["blade"]                    # optional: joints of the weapon (SIL-7)
    weapon_kind = "melee"                 # melee | ranged, with `weapon`
    [models.textures]                     # source PNGs, the engine's channel packing
    base = "..."; normal = "..."          # required; normal is XYZ in RGB, or set
    normal_xy = true                      # (optional) for X in R and Y in G, Z rebuilt
    mr = "..."                            # R metallic, G roughness (as texture-bake --kind
                                          # data); or metallic = "..." and roughness = "..."
                                          # (each read from R, like content-check)
    emissive = "..."                      # optional mask in R (x colour x strength)
    [models.emissive]                     # required with an emissive map
    colour = "#FFE6BE"; strength = 0.19
    use = "accent"                        # accent (<= +1 EV) | gameplay (<= +2 EV), PBR-EMIT-1

Frames written to `out` (8-bit sRGB AgX unless noted):

- `46m_1440p.png`, `46m_2160p.png`: the lineup from the default camera at 46 m;
- `46m_1440p_ids.png`: flat ids (model i in `ID_COLOURS[i]`, the post white, ground black);
- `46m_1440p_emit.png`: 16-bit linear emission only (no lights, Raw view, exposure 0);
- `zoom_<model>.png`: 2560x1440 at 8 m on the model;
- `turn_<model>_<k>.png`: 640x640, model yawed by k x 45 deg, the default camera framing it;
- `lineup_1440p.png`, `lineup_1440p_ids.png`: every model and the post, framed;
- `sil/<model>/<res>_<k>.png`: flat silhouettes alone at 1080p, 1440p and 1080p x4 (cropped),
  body red, tell green, weapon blue, yaw k x 45 deg.
"""
import json
import math
import os
import sys
import tomllib
import traceback

import bpy
import numpy as np
from bpy_extras.object_utils import world_to_camera_view
from mathutils import Matrix, Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import moba_blender as mb  # noqa: E402

ROLES = {"hero", "minion", "neutral", "structure", "prop"}
FACTIONS = {"forge", "void", "neutral"}
SIZE_CLASSES = {"minion-ranged", "minion-melee", "minion-siege", "hero-s", "hero-m", "hero-l",
                "large-monster", "tower", "core"}
ID_COLOURS = [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 0), (1, 0, 1), (0, 1, 1)]
POST_COLOUR = (1, 1, 1)
YAWS = 8
# `1080p4x` is 1080p at 4x4 samples per pixel: SIL-8 measures parts under 1 px at 1080p on it.
SIL_RES = {"1080p": (1920, 1080), "1440p": (2560, 1440), "1080p4x": (7680, 4320)}
CALIBRATION_RES = (640, 360)


def fail(msg):
    print(f"review_render: error: {msg}", flush=True)
    sys.exit(1)


def load_review(path):
    with open(path, "rb") as f:
        doc = tomllib.load(f)
    base = os.path.dirname(os.path.abspath(path))

    def rel(p):
        return os.path.normpath(os.path.join(base, p))

    allowed = {"name", "out", "samples", "models"}
    if set(doc) - allowed:
        fail(f"unknown keys {sorted(set(doc) - allowed)}")
    if not mb.valid_name(doc.get("name", ""), 32):
        fail("name must be 1..=32 of [A-Za-z0-9_-]")
    models = doc.get("models", [])
    if not 1 <= len(models) <= len(ID_COLOURS):
        fail(f"1 to {len(ID_COLOURS)} [[models]]")
    model_keys = {"name", "glb", "role", "size_class", "faction", "hover_m", "pose", "tell",
                  "weapon", "weapon_kind", "textures", "emissive"}
    names = set()
    for m in models:
        if set(m) - model_keys:
            fail(f"model {m.get('name')}: unknown keys {sorted(set(m) - model_keys)}")
        if not mb.valid_name(m.get("name", ""), 32) or m["name"] in names:
            fail(f"model name {m.get('name')!r} is invalid or repeated")
        names.add(m["name"])
        if m.get("role") not in ROLES:
            fail(f"{m['name']}: role must be one of {sorted(ROLES)}")
        if m.get("faction") not in FACTIONS:
            fail(f"{m['name']}: faction must be one of {sorted(FACTIONS)}")
        if m.get("size_class", "") and m["size_class"] not in SIZE_CLASSES:
            fail(f"{m['name']}: size_class must be one of {sorted(SIZE_CLASSES)}")
        if m.get("weapon") and m.get("weapon_kind") not in ("melee", "ranged"):
            fail(f"{m['name']}: weapon needs weapon_kind = melee | ranged")
        m["glb"] = rel(m["glb"])
        tex = m.get("textures", {})
        if not {"base", "normal"} <= set(tex) or not ("mr" in tex or {"metallic", "roughness"} <= set(tex)):
            fail(f"{m['name']}: textures need base, normal and mr (or metallic and roughness)")
        normal_xy = tex.pop("normal_xy", False)
        if not isinstance(normal_xy, bool):
            fail(f"{m['name']}: textures.normal_xy must be true or false")
        unknown = set(tex) - {"base", "normal", "mr", "metallic", "roughness", "emissive"}
        if unknown:
            fail(f"{m['name']}: unknown textures {sorted(unknown)}")
        m["textures"] = {k: rel(v) for k, v in tex.items()}
        m["normal_xy"] = normal_xy
        em = m.get("emissive")
        if "emissive" in tex and not em:
            # Dropping the emission silently would pass EMIT checks it never rendered.
            fail(f"{m['name']}: textures.emissive needs a [models.emissive] table (colour, strength, use)")
        if em is not None:
            if set(em) - {"colour", "strength", "use"} or not {"colour", "strength"} <= set(em):
                fail(f"{m['name']}: [models.emissive] takes colour, strength and use")
            if em.get("use", "accent") not in ("accent", "gameplay"):
                fail(f"{m['name']}: emissive use must be accent or gameplay")
            em.setdefault("use", "accent")
        for p in [m["glb"], *m["textures"].values()]:
            if not os.path.isfile(p):
                fail(f"{m['name']}: missing {p}")
    doc["out"] = rel(doc.get("out", "review"))
    doc["samples"] = int(doc.get("samples", 128))
    return doc


# --- scene ---------------------------------------------------------------------------------

def clear_scene():
    for o in list(bpy.data.objects):
        bpy.data.objects.remove(o, do_unlink=True)
    scene = bpy.context.scene
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0
    scene.render.fps = 30
    return scene


def image_node(nt, path, non_color):
    node = nt.nodes.new("ShaderNodeTexImage")
    node.image = bpy.data.images.load(path, check_existing=True)
    node.image.colorspace_settings.name = "Non-Color" if non_color else "sRGB"
    node.interpolation = "Smart"
    return node


def red(nt, image):
    """The R channel of an image node (single-channel maps are read from R, like content-check)."""
    sep = nt.nodes.new("ShaderNodeSeparateColor")
    nt.links.new(image.outputs["Color"], sep.inputs["Color"])
    return sep.outputs["Red"]


def normal_colour(nt, image, xy):
    """The normal map's colour; for an XY map, Z = sqrt(1 - X^2 - Y^2) rebuilt from R and G."""
    if not xy:
        return image.outputs["Color"]
    sep = nt.nodes.new("ShaderNodeSeparateColor")
    nt.links.new(image.outputs["Color"], sep.inputs["Color"])
    # Decode to [-1, 1]: v * 2 - 1.
    decoded = []
    for channel in ("Red", "Green"):
        mad = nt.nodes.new("ShaderNodeMath")
        mad.operation = "MULTIPLY_ADD"
        nt.links.new(sep.outputs[channel], mad.inputs[0])
        mad.inputs[1].default_value = 2.0
        mad.inputs[2].default_value = -1.0
        decoded.append(mad)
    xy_vec = nt.nodes.new("ShaderNodeCombineXYZ")
    nt.links.new(decoded[0].outputs[0], xy_vec.inputs["X"])
    nt.links.new(decoded[1].outputs[0], xy_vec.inputs["Y"])
    dot = nt.nodes.new("ShaderNodeVectorMath")
    dot.operation = "DOT_PRODUCT"
    nt.links.new(xy_vec.outputs[0], dot.inputs[0])
    nt.links.new(xy_vec.outputs[0], dot.inputs[1])
    rest = nt.nodes.new("ShaderNodeMath")
    rest.operation = "SUBTRACT"
    rest.use_clamp = True
    rest.inputs[0].default_value = 1.0
    nt.links.new(dot.outputs["Value"], rest.inputs[1])
    z = nt.nodes.new("ShaderNodeMath")
    z.operation = "SQRT"
    nt.links.new(rest.outputs[0], z.inputs[0])
    # Back to colour: (x, y, z) * 0.5 + 0.5, with R and G as stored.
    z_col = nt.nodes.new("ShaderNodeMath")
    z_col.operation = "MULTIPLY_ADD"
    nt.links.new(z.outputs[0], z_col.inputs[0])
    z_col.inputs[1].default_value = 0.5
    z_col.inputs[2].default_value = 0.5
    comb = nt.nodes.new("ShaderNodeCombineColor")
    nt.links.new(sep.outputs["Red"], comb.inputs["Red"])
    nt.links.new(sep.outputs["Green"], comb.inputs["Green"])
    nt.links.new(z_col.outputs[0], comb.inputs["Blue"])
    return comb.outputs["Color"]


def textured_material(model):
    tex, name = model["textures"], model["name"]
    mat = bpy.data.materials.new(f"review-{name}")
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = nt.nodes["Principled BSDF"]
    base = image_node(nt, tex["base"], False)
    nt.links.new(base.outputs["Color"], bsdf.inputs["Base Color"])
    if "mr" in tex:  # the engine's packing (content-authoring.md): R metallic, G roughness
        sep = nt.nodes.new("ShaderNodeSeparateColor")
        nt.links.new(image_node(nt, tex["mr"], True).outputs["Color"], sep.inputs["Color"])
        nt.links.new(sep.outputs["Red"], bsdf.inputs["Metallic"])
        nt.links.new(sep.outputs["Green"], bsdf.inputs["Roughness"])
    else:
        nt.links.new(red(nt, image_node(nt, tex["roughness"], True)), bsdf.inputs["Roughness"])
        nt.links.new(red(nt, image_node(nt, tex["metallic"], True)), bsdf.inputs["Metallic"])
    nmap = nt.nodes.new("ShaderNodeNormalMap")
    normal = image_node(nt, tex["normal"], True)
    nt.links.new(normal_colour(nt, normal, model.get("normal_xy", False)), nmap.inputs["Color"])
    nt.links.new(nmap.outputs["Normal"], bsdf.inputs["Normal"])
    em = model.get("emissive")
    if "emissive" in tex:  # load_review refuses an emissive map without its table
        tint = nt.nodes.new("ShaderNodeMix")
        tint.data_type = "RGBA"
        tint.blend_type = "MULTIPLY"
        tint.inputs["Factor"].default_value = 1.0
        nt.links.new(red(nt, image_node(nt, tex["emissive"], True)), tint.inputs[6])
        tint.inputs[7].default_value = mb.hex_linear(em["colour"]) + (1.0,)
        nt.links.new(tint.outputs[2], bsdf.inputs["Emission Color"])
        bsdf.inputs["Emission Strength"].default_value = float(em["strength"])
    return mat


def flat_material(name, rgb):
    """An emission-only material that shows `rgb` exactly under the Standard view."""
    mat = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    mat.use_nodes = True
    nt = mat.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    emit = nt.nodes.new("ShaderNodeEmission")
    emit.inputs["Color"].default_value = tuple(rgb) + (1.0,)
    emit.inputs["Strength"].default_value = 1.0
    nt.links.new(emit.outputs["Emission"], out.inputs["Surface"])
    return mat


def solid_material(name, hex_colour, roughness):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = mb.hex_linear(hex_colour) + (1.0,)
    bsdf.inputs["Roughness"].default_value = roughness
    return mat


def find_action(clip, actions):
    """The clip among this model's own actions (Blender suffixes repeats across imports: `.001`)."""
    for act in actions:
        base = act.name.split("|")[-1]
        if base == clip or base.rsplit(".", 1)[0] == clip:
            return act
    return None


def import_model(model):
    before = set(bpy.data.objects)
    actions_before = set(bpy.data.actions)
    bpy.ops.import_scene.gltf(filepath=model["glb"])
    actions = sorted((a for a in bpy.data.actions if a not in actions_before), key=lambda a: a.name)
    objs = []
    for o in [o for o in bpy.data.objects if o not in before]:
        # The importer's bone-display shape lives in `glTF_not_exported`; it isn't the model.
        if any(c.name.startswith("glTF_not_exported") for c in o.users_collection):
            bpy.data.objects.remove(o, do_unlink=True)
        else:
            objs.append(o)
    root = bpy.data.objects.new(f"root-{model['name']}", None)
    bpy.context.scene.collection.objects.link(root)
    for o in objs:
        if o.parent is None:
            o.parent = root
    meshes = [o for o in objs if o.type == "MESH"]
    if not meshes:
        fail(f"{model['name']}: the glb has no mesh")
    mat = textured_material(model)
    for o in meshes:
        for slot in o.material_slots:
            slot.material = mat
    pose = model.get("pose") or {}
    applied = False
    arm = next((o for o in objs if o.type == "ARMATURE"), None)
    if pose.get("clip") and arm is not None:
        act = find_action(pose["clip"], actions)
        if act is not None:
            arm.animation_data_create()
            arm.animation_data.action = act
            slots = getattr(act, "slots", None)
            if slots and getattr(arm.animation_data, "action_slot", None) is None:
                arm.animation_data.action_slot = slots[0]
            applied = True
    elif arm is not None and arm.animation_data:
        arm.animation_data.action = None
    if pose.get("clip") and not applied:
        print(f"review_render: warning: {model['name']}: clip {pose['clip']!r} not found; bind pose", flush=True)
    model.update(root=root, objs=objs, meshes=meshes, armature=arm, pose_applied=applied,
                 frame=int(pose.get("frame", 0)))
    return model


def classify_parts(model):
    """Per mesh: each polygon's part (0 body, 1 tell, 2 weapon) by its vertices' dominant joint."""
    tell, weapon = set(model.get("tell", [])), set(model.get("weapon", []))
    known = {g.name for o in model["meshes"] for g in o.vertex_groups}
    missing = (tell | weapon) - known
    if missing:
        fail(f"{model['name']}: joints {sorted(missing)} are not in the mesh's vertex groups")
    parts = {}
    for o in model["meshes"]:
        names = {g.index: g.name for g in o.vertex_groups}
        dominant = []
        for v in o.data.vertices:
            best = max(v.groups, key=lambda g: g.weight, default=None)
            dominant.append(names.get(best.group) if best is not None else None)
        kinds = []
        for p in o.data.polygons:
            js = [dominant[i] for i in p.vertices]
            if sum(j in weapon for j in js) * 2 > len(js):
                kinds.append(2)
            elif sum(j in tell for j in js) * 2 > len(js):
                kinds.append(1)
            else:
                kinds.append(0)
        parts[o.name] = kinds
    model["parts"] = parts


def world_points(model):
    dg = bpy.context.evaluated_depsgraph_get()
    pts = []
    for o in model["meshes"]:
        ev = o.evaluated_get(dg)
        me = ev.to_mesh()
        pts.extend(ev.matrix_world @ v.co for v in me.vertices)
        ev.to_mesh_clear()
    return pts


def bounds(points):
    xs, ys, zs = zip(*((p.x, p.y, p.z) for p in points))
    return Vector((min(xs), min(ys), min(zs))), Vector((max(xs), max(ys), max(zs)))


def place(model, location, yaw_deg=0.0):
    """Moves the model so its bbox centre (x, y) sits on `location` and yaws it about there."""
    root = model["root"]
    root.matrix_world = Matrix.Identity(4)
    bpy.context.view_layer.update()
    lo, hi = bounds(world_points(model))
    centre = Vector(((lo.x + hi.x) / 2, (lo.y + hi.y) / 2, 0.0))
    root.matrix_world = (Matrix.Translation(Vector(location)) @ Matrix.Rotation(math.radians(yaw_deg), 4, "Z")
                         @ Matrix.Translation(-centre))
    bpy.context.view_layer.update()


def set_visible(objs, visible):
    for o in objs:
        o.hide_render = not visible


# --- render --------------------------------------------------------------------------------

class Renderer:
    def __init__(self, scene, samples, cpu):
        self.scene = scene
        self.samples = samples
        self.backend, self.devices = ("CPU", []) if cpu else mb.setup_cycles(scene)
        if cpu:
            scene.render.engine = "CYCLES"
            scene.cycles.device = "CPU"
        scene.render.resolution_percentage = 100
        scene.render.image_settings.file_format = "PNG"
        scene.render.image_settings.color_mode = "RGB"
        scene.render.film_transparent = False
        scene.render.use_compositing = False
        scene.render.use_sequencer = False
        cam_data = bpy.data.cameras.new("review")
        self.cam = bpy.data.objects.new("review", cam_data)
        scene.collection.objects.link(self.cam)
        scene.camera = self.cam
        self.exposure = 0.0

    def mode(self, kind):
        s, c = self.scene, self.scene.cycles
        vs = s.view_settings
        vs.look = "None"
        vs.gamma = 1.0
        if kind == "beauty":
            vs.view_transform, vs.exposure = "AgX", self.exposure
            c.samples, c.use_denoising, c.filter_width = self.samples, True, 1.5
            c.max_bounces = 8
            s.render.image_settings.color_depth = "8"
        elif kind == "calibrate":
            vs.view_transform, vs.exposure = "AgX", self.exposure
            c.samples, c.use_denoising, c.filter_width = 32, True, 1.5
            c.max_bounces = 8
            s.render.image_settings.color_depth = "8"
        elif kind == "emit":
            vs.view_transform, vs.exposure = "Raw", 0.0
            c.samples, c.use_denoising, c.filter_width = 16, False, 1.5
            c.max_bounces = 0
            s.render.image_settings.color_depth = "16"
        elif kind == "flat":
            vs.view_transform, vs.exposure = "Standard", 0.0
            c.samples, c.use_denoising, c.filter_width = 1, False, 0.01
            c.max_bounces = 0
            s.render.image_settings.color_depth = "8"
        c.use_adaptive_sampling = kind == "beauty"

    def aim(self, target, distance, res):
        s = self.scene
        s.render.resolution_x, s.render.resolution_y = res
        s.render.use_border = False
        s.render.use_crop_to_border = False
        mb.orbit_camera(self.cam, target, distance)
        bpy.context.view_layer.update()

    def project(self, points):
        """Pixel bbox [x0, y0, x1, y1] (top-left origin, inclusive-exclusive) of world points."""
        s = self.scene
        w, h = s.render.resolution_x, s.render.resolution_y
        xs, ys = [], []
        for p in points:
            v = world_to_camera_view(s, self.cam, p)
            xs.append(v.x * w)
            ys.append((1.0 - v.y) * h)
        return [math.floor(min(xs)), math.floor(min(ys)), math.ceil(max(xs)), math.ceil(max(ys))]

    def crop(self, box, margin):
        """Renders only `box` (pixels, top-left origin) grown by `margin`; returns the crop."""
        s = self.scene
        w, h = s.render.resolution_x, s.render.resolution_y
        x0, y0 = max(0, box[0] - margin), max(0, box[1] - margin)
        x1, y1 = min(w, box[2] + margin), min(h, box[3] + margin)
        s.render.use_border = s.render.use_crop_to_border = True
        s.render.border_min_x, s.render.border_max_x = x0 / w, x1 / w
        s.render.border_min_y, s.render.border_max_y = 1.0 - y1 / h, 1.0 - y0 / h
        return [x0, y0, x1, y1]

    def shoot(self, path):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        self.scene.render.filepath = path
        bpy.ops.render.render(write_still=True)
        print(f"review_render: wrote {path}", flush=True)


def srgb8_to_oklab_l(rgb):
    lin = np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)
    r, g, b = lin[..., 0], lin[..., 1], lin[..., 2]
    l_ = np.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
    m_ = np.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
    s_ = np.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
    return 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_


def mean_l(path):
    img = bpy.data.images.load(path)
    px = np.array(img.pixels[:], dtype=np.float64).reshape(img.size[1], img.size[0], 4)[..., :3]
    bpy.data.images.remove(img)
    return float(srgb8_to_oklab_l(px).mean())


def calibrate(r, work):
    """Exposure (EV) at which the bare ground's mean OKLab L is the gameplay 0.50, by secant."""
    r.mode("calibrate")
    r.aim((0, 0, 0), mb.CAMERA_DISTANCE, CALIBRATION_RES)
    path = os.path.join(work, "calibrate.png")
    samples = []
    ev = 0.0
    for _ in range(8):
        r.exposure = ev
        r.mode("calibrate")
        r.shoot(path)
        lum = mean_l(path)
        samples.append((ev, lum))
        if abs(lum - mb.TARGET_GROUND_L) < 0.003:
            break
        if len(samples) == 1:
            ev = 1.0 if lum < mb.TARGET_GROUND_L else -1.0
        else:
            (e0, l0), (e1, l1) = samples[-2], samples[-1]
            slope = (l1 - l0) / (e1 - e0) if e1 != e0 else 0.1
            ev = e1 + (mb.TARGET_GROUND_L - l1) / max(slope, 0.01)
    os.remove(path)
    r.exposure, measured = samples[-1]
    return measured, samples


def main():
    args = mb.argv_after_dashdash()
    if not args:
        fail("usage: review_render.py -- <review.toml> [--samples N] [--cpu]")
    review = load_review(args[0])
    if "--samples" in args:
        review["samples"] = int(args[args.index("--samples") + 1])
    cpu = "--cpu" in args
    out = review["out"]
    os.makedirs(out, exist_ok=True)
    # A stale render_meta.json would let review-sheet sign off frames this run did not finish.
    meta_path = os.path.join(out, "render_meta.json")
    if os.path.exists(meta_path):
        os.remove(meta_path)

    scene = clear_scene()
    r = Renderer(scene, review["samples"], cpu)
    mb.add_sky(scene)
    sun = mb.add_sun(scene)
    bpy.ops.mesh.primitive_plane_add(size=400.0, location=(0, 0, 0))
    ground = bpy.context.active_object
    ground.name = "ash-ground"
    ground.data.materials.append(solid_material("ash", mb.ASH_HEX, 0.9))
    bpy.ops.mesh.primitive_cylinder_add(radius=0.1, depth=mb.REFERENCE_POST_M,
                                        location=(0, 0, mb.REFERENCE_POST_M / 2))
    post = bpy.context.active_object
    post.name = "reference-post"
    post.data.materials.append(solid_material("post", "#808080", 0.6))

    models = [import_model(m) for m in review["models"]]
    for m in models:
        classify_parts(m)
    if len({m["frame"] for m in models}) > 1:
        fail("every model's pose frame must be the same (one scene frame)")
    scene.frame_set(models[0]["frame"])
    # Layout: left to right across the screen at the default camera, centred on the origin.
    off = mb.e2b(mb.orbit_offset(mb.CAMERA_YAW, mb.CAMERA_PITCH))
    forward = Vector((-off.x, -off.y, 0.0)).normalized()
    right = forward.cross(Vector((0, 0, 1))).normalized()
    widths = []
    for m in models:
        place(m, (0, 0, 0))
        lo, hi = bounds(world_points(m))
        m["bind_bounds"] = (lo, hi)
        m["height"] = hi.z - lo.z
        m["footprint"] = max(hi.x - lo.x, hi.y - lo.y)
        widths.append(m["footprint"])
    gap = 1.0
    span = sum(widths) + gap * len(widths) + 0.2
    cursor = -span / 2
    for m, w in zip(models, widths):
        m["slot"] = right * (cursor + w / 2)
        cursor += w + gap
    post.location = right * (cursor + 0.1) + Vector((0, 0, mb.REFERENCE_POST_M / 2))
    for m in models:
        place(m, m["slot"])
    bpy.context.view_layer.update()

    meta = {
        "schema": 1, "name": review["name"], "tool": "scripts/blender/review_render.py",
        "blender": ".".join(map(str, bpy.app.version)), "renderer": "Cycles",
        "device": r.backend, "samples": review["samples"],
        "camera": {"yaw": mb.CAMERA_YAW, "pitch": mb.CAMERA_PITCH, "distance_m": mb.CAMERA_DISTANCE,
                   "zoom_distance_m": mb.ZOOM_DISTANCE, "fov_y_deg": mb.CAMERA_FOV_Y_DEG},
        "sun": {"dir": list(mb.SUN_DIR), "colour": list(mb.SUN_COLOR), "intensity": mb.SUN_INTENSITY,
                "elevation_deg": mb.sun_elevation_deg()},
        "sky": {"ibl": mb.IBL, "zenith": list(mb.ZENITH), "horizon": list(mb.HORIZON),
                "sun_color": list(mb.SKY_SUN_COLOR), "sun_glow": mb.SKY_SUN_GLOW,
                "ground": list(mb.GROUND)},
        "ground_hex": mb.ASH_HEX, "lane_hex": mb.LANE_HEX, "l_ref": mb.l_ref(),
        "post_m": mb.REFERENCE_POST_M, "view_transform": "AgX",
    }

    # Exposure: the bare ground at the gameplay mean.
    hidden = [o for m in models for o in m["objs"]] + [post]
    set_visible(hidden, False)
    ground_l, calibration = calibrate(r, out)
    set_visible(hidden, True)
    meta.update(exposure_ev=r.exposure, ground_l_target=mb.TARGET_GROUND_L,
                ground_l_calibrated=ground_l, calibration=[list(s) for s in calibration])

    all_points = {m["name"]: world_points(m) for m in models}
    post_pts = [post.matrix_world @ Vector(c) for c in post.bound_box]
    frames = {}

    def framed(name, res, target, distance):
        r.aim(target, distance, res)
        return {"file": name, "width": res[0], "height": res[1],
                "target": list(target), "distance_m": distance,
                "models": [{"name": m["name"], "bbox": r.project(all_points[m["name"]])} for m in models],
                "post_bbox": r.project(post_pts)}

    # 46 m beauty at 1440p and 4K.
    r.mode("beauty")
    for key, res in (("46m_1440p", (2560, 1440)), ("46m_2160p", (3840, 2160))):
        frames[key] = framed(f"{key}.png", res, (0, 0, 0), mb.CAMERA_DISTANCE)
        r.shoot(os.path.join(out, f"{key}.png"))

    # 8 m zoom per model.
    frames["zoom"] = []
    for m in models:
        lo, hi = bounds(all_points[m["name"]])
        target = ((lo.x + hi.x) / 2, (lo.y + hi.y) / 2, (lo.z + hi.z) / 2)
        f = framed(f"zoom_{m['name']}.png", (2560, 1440), target, mb.ZOOM_DISTANCE)
        f["model"] = m["name"]
        frames["zoom"].append(f)
        r.shoot(os.path.join(out, f["file"]))

    # Lineup: everything framed at the default angles.
    lo, hi = bounds([p for pts in all_points.values() for p in pts] + post_pts)
    half = max((hi - lo).length / 2, 1.0)
    lineup_d = max(mb.ZOOM_DISTANCE, 1.15 * half / math.tan(math.radians(mb.CAMERA_FOV_Y_DEG) / 2))
    centre = tuple((lo + hi) / 2)
    frames["lineup_1440p"] = framed("lineup_1440p.png", (2560, 1440), centre, lineup_d)
    r.shoot(os.path.join(out, "lineup_1440p.png"))

    # Turntable: each model alone, yawed in 45 deg steps, the default camera framing it.
    frames["turntable"] = []
    for m in models:
        others = [o for n in models if n is not m for o in n["objs"]] + [post]
        set_visible(others, False)
        lo, hi = bounds(all_points[m["name"]])
        radius = max((hi - lo).length / 2, 0.5)
        d = 1.25 * radius / math.sin(math.radians(mb.CAMERA_FOV_Y_DEG) / 2)
        files, yaws = [], []
        for k in range(YAWS):
            place(m, (0, 0, 0), k * 360.0 / YAWS)
            pts = world_points(m)
            plo, phi = bounds(pts)
            r.aim((0, 0, (plo.z + phi.z) / 2), d, (640, 640))
            name = f"turn_{m['name']}_{k}.png"
            r.shoot(os.path.join(out, name))
            files.append(name)
            yaws.append(k * 360.0 / YAWS)
        frames["turntable"].append({"model": m["name"], "files": files, "yaws_deg": yaws, "distance_m": d,
                                    "width": 640, "height": 640})
        place(m, m["slot"])
        set_visible(others, True)

    # Emission only, 16-bit linear, at the 46 m 1440p framing.
    r.mode("emit")
    sun.hide_render = True
    strength = scene.world.node_tree.nodes["Background"].inputs["Strength"]
    strength.default_value = 0.0
    r.aim((0, 0, 0), mb.CAMERA_DISTANCE, (2560, 1440))
    r.shoot(os.path.join(out, "46m_1440p_emit.png"))
    frames["46m_1440p"]["emit"] = "46m_1440p_emit.png"
    sun.hide_render = False
    strength.default_value = mb.IBL

    # Flat ids and silhouettes (replaces the materials: every beauty frame is done).
    r.mode("flat")
    scene.world.node_tree.nodes["Background"].inputs["Strength"].default_value = 0.0
    sun.hide_render = True
    ground.data.materials[0] = flat_material("flat-ground", (0, 0, 0))
    post.data.materials[0] = flat_material("flat-post", POST_COLOUR)

    def flat_model(m, colours):
        mats = [flat_material(f"flat-{m['name']}-{i}", c) for i, c in enumerate(colours)]
        for o in m["meshes"]:
            o.data.materials.clear()
            for mat in mats:
                o.data.materials.append(mat)
            kinds = m["parts"][o.name]
            for p, kind in zip(o.data.polygons, kinds):
                p.material_index = kind if len(mats) == 3 else 0

    for i, m in enumerate(models):
        flat_model(m, [ID_COLOURS[i]])
    for key, target, d in (("46m_1440p", (0, 0, 0), mb.CAMERA_DISTANCE),
                           ("lineup_1440p", centre, lineup_d)):
        r.aim(target, d, (2560, 1440))
        name = f"{key}_ids.png"
        r.shoot(os.path.join(out, name))
        frames[key]["ids"] = name
    meta["id_colours"] = {m["name"]: list(ID_COLOURS[i]) for i, m in enumerate(models)}
    meta["post_colour"] = list(POST_COLOUR)

    ground.hide_render = post.hide_render = True
    frames["silhouettes"] = []
    for m in models:
        others = [o for n in models if n is not m for o in n["objs"]]
        set_visible(others, False)
        flat_model(m, [(1, 0, 0), (0, 1, 0), (0, 0, 1)])
        for res_name, res in SIL_RES.items():
            files, crops = [], []
            for k in range(YAWS):
                place(m, (0, 0, 0), k * 360.0 / YAWS)
                pts = world_points(m)
                r.aim((0, 0, 0), mb.CAMERA_DISTANCE, res)
                crop = r.crop(r.project(pts), 6 * res[1] // 1080)
                name = f"sil/{m['name']}/{res_name}_{k}.png"
                r.shoot(os.path.join(out, name))
                files.append(name)
                crops.append(crop)
            frames["silhouettes"].append({"model": m["name"], "res": res_name, "width": res[0],
                                          "height": res[1], "files": files, "crops": crops,
                                          "yaws_deg": [k * 360.0 / YAWS for k in range(YAWS)]})
        set_visible(others, True)
        place(m, m["slot"])

    meta["models"] = [{
        # Relative to render_meta.json, so the render folder can be moved or shared.
        "name": m["name"], "glb": os.path.relpath(m["glb"], out), "role": m["role"], "size_class": m.get("size_class", ""),
        "faction": m["faction"], "hover_m": float(m.get("hover_m", 0.0)),
        "pose": {"clip": (m.get("pose") or {}).get("clip", ""), "frame": m["frame"],
                 "applied": m["pose_applied"]},
        "posed_height_m": m["height"], "posed_footprint_m": m["footprint"],
        "tell": list(m.get("tell", [])), "weapon": list(m.get("weapon", [])),
        "weapon_kind": m.get("weapon_kind", ""),
        "emissive": m.get("emissive") or {},
    } for m in models]
    meta["frames"] = frames
    # Written last and renamed into place: render_meta.json exists only for a finished run.
    partial = meta_path + ".partial"
    with open(partial, "w", encoding="utf-8", newline="\n") as f:
        json.dump(meta, f, indent=1)
    os.replace(partial, meta_path)
    print(f"review_render: wrote {meta_path}", flush=True)


if __name__ == "__main__":
    # Blender keeps running (and exits 0) after an uncaught exception in a --python script.
    try:
        main()
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001 - any failure must be exit 1
        traceback.print_exc()
        sys.exit(1)
