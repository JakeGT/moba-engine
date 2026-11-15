"""Shared constants and helpers for the engine's Blender scripts (run inside Blender's Python).

The numbers here are the engine's and the art bible's, never this file's own:

- the default gameplay camera is `Camera::orbit(target, 46.0, -0.7, 0.95, aspect)` with a 60 deg
  vertical FOV (`crates/client/src/app/scene_build.rs`, `crates/render/src/scene.rs`), and the
  zoom camera the same yaw and pitch at 8 m;
- the light is Golden Crossing (`docs/art/lighting-mood.md`, option A): `sun_dir`
  (-0.885, 0.532, 0.465) towards the sun (x east, y up, z towards the screen bottom), `[lighting]`
  sun colour (1.0, 0.80, 0.62), intensity 1.5, IBL 0.35, the analytic zenith / horizon / ground
  sky; the gameplay exposure targets the measured ground mean OKLab L of 0.50;
- `L_ref` is the lane grit `#906B58` (palette-and-value §6) in that sun:
  `luminance(lane) x 1.5 x sin(28 deg)` (material-vocabulary §1.1).

`crates/tools/src/bin/review-sheet` checks `render_meta.json` against the same constants, so a
render made with other numbers is refused rather than signed off.
"""
import math
import os
import sys

import bpy
from mathutils import Vector

CAMERA_YAW = -0.7
CAMERA_PITCH = 0.95
CAMERA_DISTANCE = 46.0
ZOOM_DISTANCE = 8.0
CAMERA_FOV_Y_DEG = 60.0
SUN_DIR = (-0.885, 0.532, 0.465)
SUN_COLOR = (1.0, 0.80, 0.62)
SUN_INTENSITY = 1.5
IBL = 0.35
ZENITH, HORIZON, GROUND = (0.16, 0.22, 0.36), (0.95, 0.62, 0.38), (0.18, 0.11, 0.07)
ASH_HEX = "#6B6560"
LANE_HEX = "#906B58"
TARGET_GROUND_L = 0.50
REFERENCE_POST_M = 1.8
NAME_CHARS = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-")


def argv_after_dashdash():
    """Script arguments after Blender's own (`blender ... --python x.py -- <these>`)."""
    return sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []


def valid_name(name, limit):
    """1..=limit of [A-Za-z0-9_-] (the engine's model, joint and texture name rule)."""
    return 0 < len(name) <= limit and set(name) <= NAME_CHARS


def srgb_to_linear(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def hex_linear(hex_colour):
    """`#RRGGBB` to linear RGB."""
    h = hex_colour.lstrip("#")
    return tuple(srgb_to_linear(int(h[i:i + 2], 16) / 255) for i in (0, 2, 4))


def luminance(rgb):
    return 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]


def sun_elevation_deg():
    x, y, z = SUN_DIR
    return math.degrees(math.asin(y / math.sqrt(x * x + y * y + z * z)))


def l_ref():
    """Material-vocabulary §1.1's reference luminance for Golden Crossing."""
    return luminance(hex_linear(LANE_HEX)) * SUN_INTENSITY * math.sin(math.radians(sun_elevation_deg()))


def e2b(v):
    """Engine / glTF (x, y up, z) to Blender (x, -z, y), as Blender's glTF importer converts."""
    return Vector((v[0], -v[2], v[1]))


def orbit_offset(yaw, pitch):
    """`Camera::orbit`'s eye offset (unit length) in engine coordinates."""
    return (math.cos(yaw) * math.cos(pitch), math.sin(pitch), math.sin(yaw) * math.cos(pitch))


def setup_cycles(scene, prefer=("OPTIX", "CUDA", "HIP")):
    """Cycles on the first GPU backend found, else the CPU. Returns (backend, [device names]).

    Run every render or bake under `flock /home/jaket/Projects/MOBA/handoff/gpu.lock blender ...`
    on the maintainer's machine (the GPU-lock rule): the lock also covers CPU fallbacks.
    """
    scene.render.engine = "CYCLES"
    prefs = bpy.context.preferences.addons["cycles"].preferences
    for kind in prefer:
        try:
            prefs.compute_device_type = kind
        except TypeError:
            continue
        prefs.refresh_devices()
        gpus = [d for d in prefs.devices if d.type == kind]
        if not gpus:
            continue
        for d in prefs.devices:
            d.use = d.type == kind
        scene.cycles.device = "GPU"
        return kind, [d.name for d in gpus]
    scene.cycles.device = "CPU"
    return "CPU", []


def orbit_camera(cam, target, distance, yaw=CAMERA_YAW, pitch=CAMERA_PITCH, fov_y_deg=CAMERA_FOV_Y_DEG):
    """Places `cam` as `Camera::orbit(target, distance, yaw, pitch)` (target in Blender space)."""
    off = e2b(orbit_offset(yaw, pitch))
    cam.location = Vector(target) + off * distance
    cam.rotation_euler = (Vector(target) - cam.location).to_track_quat("-Z", "Y").to_euler()
    cam.data.sensor_fit = "VERTICAL"
    cam.data.angle_y = math.radians(fov_y_deg)
    cam.data.clip_start, cam.data.clip_end = 0.1, 1000.0


def add_sun(scene):
    sun = bpy.data.objects.new("golden_crossing_sun", bpy.data.lights.new("golden_crossing_sun", "SUN"))
    scene.collection.objects.link(sun)
    sun.rotation_euler = e2b(SUN_DIR).normalized().to_track_quat("Z", "Y").to_euler()
    # Engine units: a white Lambert surface facing the sun shows albedo x intensity; Cycles'
    # sun irradiance gives radiance E / pi, so strength = pi x intensity.
    sun.data.energy = math.pi * SUN_INTENSITY
    sun.data.color = SUN_COLOR
    sun.data.angle = math.radians(0.53)
    return sun


def add_sky(scene):
    """The analytic Golden Crossing sky (zenith, horizon, ground) at `ibl_intensity`."""
    world = bpy.data.worlds.new("golden_crossing")
    scene.world = world
    world.use_nodes = True
    nt = world.node_tree
    bg = nt.nodes["Background"]
    tc = nt.nodes.new("ShaderNodeTexCoord")
    sep = nt.nodes.new("ShaderNodeSeparateXYZ")
    nt.links.new(tc.outputs["Generated"], sep.inputs[0])
    up = nt.nodes.new("ShaderNodeMath")
    up.operation = "POWER"
    up.use_clamp = True
    nt.links.new(sep.outputs["Z"], up.inputs[0])
    up.inputs[1].default_value = 0.5
    sky = nt.nodes.new("ShaderNodeMix")
    sky.data_type = "RGBA"
    nt.links.new(up.outputs[0], sky.inputs["Factor"])
    sky.inputs[6].default_value = HORIZON + (1.0,)
    sky.inputs[7].default_value = ZENITH + (1.0,)
    below = nt.nodes.new("ShaderNodeMath")
    below.operation = "LESS_THAN"
    nt.links.new(sep.outputs["Z"], below.inputs[0])
    below.inputs[1].default_value = 0.0
    mix = nt.nodes.new("ShaderNodeMix")
    mix.data_type = "RGBA"
    nt.links.new(below.outputs[0], mix.inputs["Factor"])
    nt.links.new(sky.outputs[2], mix.inputs[6])
    mix.inputs[7].default_value = GROUND + (1.0,)
    nt.links.new(mix.outputs[2], bg.inputs["Color"])
    bg.inputs["Strength"].default_value = IBL
    return world
