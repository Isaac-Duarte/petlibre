"""Render the PLAF103 model into the app's and docs' device images.

Run with Blender 5.2 (not a host tool; Blender's bundled Python):

    blender -b assets/model/plaf103.blend -P assets/model/render.py -- [--view NAME] [--size PX] [--samples N]

Every render is unbranded: the hopper lettering and the PCB label are hidden,
so an image never suggests that this project makes the hardware. The stage
(camera, lights, world) is built here rather than saved in the .blend, so the
model file stays a clean product model and every render is reproducible.
Output goes to assets/renders/<view>.png with a transparent background.
"""

import argparse
import math
import sys
from pathlib import Path

import bpy
from mathutils import Vector

# Model space: meters, Z up, the bowl toward -Y (the front).
TARGET = Vector((0.0, -0.03, 0.17))

# name: (azimuth degrees from the front, clockwise from above; elevation degrees; focal mm)
VIEWS = {
    "card": (-32.0, 14.0, 85.0),
    "front": (0.0, 8.0, 85.0),
    "panel": (0.0, 18.0, 160.0),
}

BRAND_OBJECTS = ("Logo", "PCB_ChipLabel")


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser()
    parser.add_argument("--view", choices=sorted(VIEWS), default="card")
    parser.add_argument("--size", type=int, default=1024)
    parser.add_argument("--samples", type=int, default=256)
    return parser.parse_args(argv)


def unbrand():
    for name in BRAND_OBJECTS:
        obj = bpy.data.objects.get(name)
        if obj is not None:
            obj.hide_render = True


def stage(view):
    scene = bpy.context.scene
    azimuth, elevation, focal = VIEWS[view]

    # Frame the whole feeder: distance from the focal length and a fixed
    # margin, so every view of the same size shows the product at one scale.
    distance = 0.82 * focal / 50.0
    az, el = math.radians(azimuth), math.radians(elevation)
    offset = Vector((math.sin(az) * math.cos(el), -math.cos(az) * math.cos(el), math.sin(el)))

    cam_data = bpy.data.cameras.new("RenderCam")
    cam_data.lens = focal
    camera = bpy.data.objects.new("RenderCam", cam_data)
    camera.location = TARGET + offset * distance
    camera.rotation_euler = (TARGET - camera.location).to_track_quat("-Z", "Y").to_euler()
    scene.collection.objects.link(camera)
    scene.camera = camera

    # Soft studio light: a large key from the camera's side, a fill opposite
    # and a rim from behind to separate the dark body from any background.
    for name, energy, size, location in (
        ("Key", 260.0, 1.2, (-1.2, -1.4, 1.6)),
        ("Fill", 90.0, 1.6, (1.5, -0.8, 0.8)),
        ("Rim", 180.0, 0.8, (0.4, 1.4, 1.2)),
        ("Top", 60.0, 1.0, (0.0, 0.0, 2.0)),
    ):
        light_data = bpy.data.lights.new(name, "AREA")
        light_data.energy = energy
        light_data.size = size
        light = bpy.data.objects.new(name, light_data)
        light.location = location
        light.rotation_euler = (Vector((0.0, 0.0, 0.15)) - light.location).to_track_quat("-Z", "Y").to_euler()
        scene.collection.objects.link(light)

    # A shadow catcher keeps the transparent background but grounds the feeder
    # with a soft contact shadow, as a product card expects.
    bpy.ops.mesh.primitive_plane_add(size=4.0, location=(0.0, 0.0, 0.0))
    catcher = bpy.context.active_object
    catcher.is_shadow_catcher = True
    # It must not bounce light: a lit floor would wash the matte black body grey.
    catcher.visible_diffuse = False
    catcher.visible_glossy = False

    world = bpy.data.worlds.new("RenderWorld")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs["Color"].default_value = (0.8, 0.8, 0.82, 1.0)
    world.node_tree.nodes["Background"].inputs["Strength"].default_value = 0.35
    scene.world = world


def configure(size, samples):
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.samples = samples
    scene.cycles.use_denoising = True
    scene.render.film_transparent = True
    scene.render.resolution_x = size
    scene.render.resolution_y = size
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    scene.view_settings.view_transform = "AgX"

    # Use a GPU when one exists; the render is identical in content either way.
    prefs = bpy.context.preferences.addons["cycles"].preferences
    for backend in ("OPTIX", "CUDA", "HIP", "ONEAPI", "METAL"):
        try:
            prefs.compute_device_type = backend
        except TypeError:
            continue
        prefs.get_devices()
        if any(d.type == backend for d in prefs.devices):
            for device in prefs.devices:
                device.use = device.type == backend
            scene.cycles.device = "GPU"
            break


def main():
    args = parse_args()
    unbrand()
    stage(args.view)
    configure(args.size, args.samples)

    out = Path(bpy.data.filepath).resolve().parent.parent / "renders" / f"{args.view}.png"
    bpy.context.scene.render.filepath = str(out)
    bpy.ops.render.render(write_still=True)
    print(f"rendered {out}")


main()
