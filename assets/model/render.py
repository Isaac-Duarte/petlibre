"""Render the PLAF103 model into the app's and docs' device images.

Run with Blender 5.2 (not a host tool; Blender's bundled Python):

    blender -b assets/model/plaf103.blend -P assets/model/render.py -- --all
    blender -b assets/model/plaf103.blend -P assets/model/render.py -- --view front --state food-half

Every render is unbranded: the hopper lettering and the PCB label are hidden,
so an image never suggests that this project makes the hardware. The stage
(camera, lights, world) and every state change are applied here rather than
saved in the .blend, so the model file stays a clean product model and every
render is reproducible. Output goes to assets/renders/<view>[-<state>].png
with a transparent background, plus <view>.json for the views with hotspots.
The spec is RENDERS.md beside this file.
"""

import argparse
import json
import math
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

import bpy
import numpy as np
from bpy_extras.object_utils import world_to_camera_view
from mathutils import Matrix, Vector

ASSETS = Path(__file__).resolve().parent.parent
RENDERS = ASSETS / "renders"
CHECKS = ASSETS / "model" / "checks"

# Model space: meters, Z up, the bowl toward -Y (the front).
TARGET = Vector((0.0, -0.03, 0.17))

# The frame width at the target that shows the whole feeder at one scale: the
# first card's 0.82 m of distance per 50 mm of focal length on a 36 mm sensor.
FEEDER_WIDTH = 0.82 * 36.0 / 50.0

BRAND_OBJECTS = ("Logo", "PCB_ChipLabel")

# The ring icons the model lights blue. Only the red fault light is recorded on
# hardware, so every indicator renders unlit (the smoked look of an indicator
# icon with its LED off) and the UI draws light through the hotspots.
LIT_ICONS = ("Icon_Lock", "Icon_WiFi")
UNLIT_ICON = "PL_Icon__Icon_Bowl"

# The window's food is the KibbleWin* pieces behind WindowGlass, in front of the
# bulk HopperFill. The glass spans these heights (world Z).
WINDOW_BOTTOM = 0.186
WINDOW_TOP = 0.303

# Surface finishes matched to owner photographs, applied at render time so the
# model's materials stay as modeled. The body is near-black satin plastic with
# a soft sheen (the model's grey reads as mid grey once lit), the ring is gloss
# black that mirrors the room, and the bowl is mirror-polished steel.
FINISHES = {
    "PL_DarkGreyPlastic": {"Base Color": (0.012, 0.013, 0.014, 1.0), "Roughness": 0.3},
    "PL_MatteBlack": {"Base Color": (0.008, 0.008, 0.009, 1.0), "Roughness": 0.27},
    "PL_GlossBlack": {"Roughness": 0.06, "Coat Roughness": 0.02},
    "PL_Steel": {"Roughness": 0.05},
}


@dataclass(frozen=True)
class Camera:
    # Degrees from the front, clockwise from above.
    azimuth: float
    elevation: float
    focal: float
    target: Vector = TARGET
    # Frame width in meters at the target.
    width: float = FEEDER_WIDTH


# Cameras belong to views, never to states, so a set's images line up.
CAMERAS = {
    "card": Camera(-32.0, 14.0, 85.0),
    "front": Camera(0.0, 8.0, 85.0, Vector((0.0, -0.06, 0.19)), 0.48),
    # The icons span 0.17 m of ring, so this is as tight as the frame gets.
    "panel": Camera(0.0, 10.0, 160.0, Vector((0.0, -0.07, 0.119)), 0.19),
    "top": Camera(-28.0, 50.0, 70.0, Vector((0.07, -0.04, 0.15)), 0.7),
    "outlet": Camera(0.0, 11.0, 100.0, Vector((0.0, -0.09, 0.07)), 0.17),
    "back": Camera(148.0, 14.0, 85.0),
    "bottom": Camera(-32.0, -48.0, 85.0, Vector((0.0, 0.0, 0.07)), 0.5),
}

# The ring icon under each button and indicator. The model has no separate
# keycaps or LED objects, so each one's hotspot is its printed icon.
# - Buttons follow the firmware's roles (docs/firmware/buttons.md): Reset, Mic,
#   Dispense (the portion icon) and Lock. They are the outer four icons; the
#   four indicators sit closer together between them.
# - Indicators follow the operator's names for PA0/PA2/PA4/PB22 ("Wi-Fi", "low
#   food", "locked", "warning"; docs/firmware/startup-diagnostics.md). "Low
#   food" is the bowl icon by elimination, since no observation names the icon
#   that lit.
PARTS = {
    "button_reset": "Icon_Reset",
    "button_mic": "Icon_Mic",
    "button_dispense": "Icon_Portion",
    "button_lock": "Icon_LockButton",
    "led_connection": "Icon_WiFi",
    "led_food": "Icon_Bowl",
    "led_lock": "Icon_Lock",
    "led_fault": "Icon_Warn",
    "window": "WindowGlass",
    # No object of its own: the cavity faces at the front of Base.
    "outlet": None,
    "bowl": "BowlHolder",
}


@dataclass
class Look:
    # Multipliers on the studio rig, by light name.
    energy: dict = field(default_factory=dict)
    # Extra lights as (energy, size, location), laid out like the rig.
    extra: tuple = ()
    floor: bool = True


@dataclass(frozen=True)
class View:
    camera: str
    # state name: the change it makes, given the look to adjust.
    states: dict = field(default_factory=dict)
    # Whether the view without a state is an output.
    bare: bool = True
    # A change every image of the view shares.
    setup: object = None
    hotspots: bool = False


# Scene helpers.


def hide(obj):
    # Viewport visibility too, so the hotspot ray casts see what the render sees.
    obj.hide_render = True
    obj.hide_viewport = True


def descendants(obj):
    yield obj
    for child in obj.children:
        yield from descendants(child)


def world_corners(obj):
    return [obj.matrix_world @ Vector(corner) for corner in obj.bound_box]


def z_range(obj):
    heights = [c.z for c in world_corners(obj)]
    return min(heights), max(heights)


# State changes.


def food(level):
    """Fill the hopper to `level` of the window's height; 0 empties it."""

    def apply(look):
        line = WINDOW_BOTTOM + level * (WINDOW_TOP - WINDOW_BOTTOM)
        for obj in bpy.data.objects:
            if obj.name.startswith("KibbleWin") and sum(z_range(obj)) / 2.0 > line:
                hide(obj)

        fill = bpy.data.objects["HopperFill"]
        if level <= 0.0:
            hide(fill)
        elif level < 1.0:
            # Squash the bulk about its floor so its top meets the line. Its
            # mesh space is world space (no transform, an identity parent).
            low, high = z_range(fill)
            factor = (line - low) / (high - low)
            fill.data.transform(Matrix.Translation((0.0, 0.0, low)) @ Matrix.Diagonal((1.0, 1.0, factor, 1.0)) @ Matrix.Translation((0.0, 0.0, -low)))

    return apply


def lid_open(look):
    # Lift the lid off and lay it flat on the surface, front-right of the
    # feeder, clear of the base and the bowl.
    lid = bpy.data.objects["Lid"]
    low, _ = z_range(lid)
    lid.location += Vector((0.225, -0.02, -low))


def bowl_out(look):
    # Pull the bowl forward along the surface so the outlet shows above it.
    bpy.data.objects["BowlHolder"].location += Vector((0.0, -0.11, 0.0))


def dark(look):
    # On navy the charcoal body needs a brighter edge to separate: a rim three
    # times as strong and a second one from the other side.
    look.energy["Rim"] = 3.0
    look.extra = ((420.0, 0.8, (1.25, 0.65, 1.1)),)


def underside(look):
    # The feeder seen from below: no floor, no bowl, the rig mirrored under it.
    look.floor = False
    for obj in descendants(bpy.data.objects["BowlHolder"]):
        hide(obj)


VIEWS = {
    "card": View("card", {"dark": dark}, hotspots=True),
    "front": View(
        "front",
        {
            "food-full": food(1.0),
            "food-half": food(0.5),
            # Kibble at the bottom of the window only.
            "food-low": food(0.2),
            "food-empty": food(0.0),
        },
        hotspots=True,
    ),
    "panel": View("panel", hotspots=True),
    "top": View("top", {"lid-open": lid_open}, bare=False),
    "bowl": View("card", {"out": bowl_out}, bare=False),
    "outlet": View("outlet"),
    "back": View("back"),
    "bottom": View("bottom", setup=underside),
}


def outputs():
    for name, view in VIEWS.items():
        if view.bare:
            yield name, None
        for state in view.states:
            yield name, state


def output_name(view, state):
    return f"{view}-{state}" if state else view


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser()
    parser.add_argument("--all", action="store_true", help="every view and state, each in a fresh Blender")
    parser.add_argument("--view", choices=sorted(VIEWS), default="card")
    parser.add_argument("--state")
    parser.add_argument("--size", type=int, default=1024)
    parser.add_argument("--samples", type=int, default=512)
    parser.add_argument("--out", type=Path, default=RENDERS)
    parser.add_argument("--check", action="store_true", help="also draw each hotspot view's circles into model/checks/")
    args = parser.parse_args(argv)

    view = VIEWS[args.view]
    if args.state is None and not view.bare and not args.all:
        parser.error(f"--view {args.view} needs --state ({', '.join(view.states)})")
    if args.state is not None and args.state not in view.states:
        parser.error(f"--view {args.view} has no state {args.state!r}")
    return args


# The stage.


def unbrand():
    for name in BRAND_OBJECTS:
        obj = bpy.data.objects.get(name)
        if obj is not None:
            hide(obj)


def truthful():
    # What the model already keeps out of renders stays out of ray casts.
    for obj in bpy.data.objects:
        if obj.hide_render:
            hide(obj)

    # Unlit, a lit icon looks like every other print on the ring: its blue base
    # color alone would still read as light.
    printed = bpy.data.materials[UNLIT_ICON].node_tree.nodes["Principled BSDF"].inputs
    for name in LIT_ICONS:
        for slot in bpy.data.objects[name].material_slots:
            inputs = slot.material.node_tree.nodes["Principled BSDF"].inputs
            for socket in ("Base Color", "Emission Color", "Emission Strength"):
                inputs[socket].default_value = printed[socket].default_value

    # The model leaves the hopper's bulk food without a material, and it shows
    # through the window above partial fills and with the lid off.
    bpy.data.objects["HopperFill"].data.materials.append(bpy.data.materials["PL_Kibble"])

    for material, finish in FINISHES.items():
        inputs = next(n for n in bpy.data.materials[material].node_tree.nodes if n.type == "BSDF_PRINCIPLED").inputs
        for socket, value in finish.items():
            inputs[socket].default_value = value


def aim(obj, target):
    obj.rotation_euler = (target - obj.location).to_track_quat("-Z", "Y").to_euler()


def stage(camera, look):
    scene = bpy.context.scene

    # The distance that fits the frame width on a 36 mm sensor.
    distance = camera.width * camera.focal / 36.0
    az, el = math.radians(camera.azimuth), math.radians(camera.elevation)
    offset = Vector((math.sin(az) * math.cos(el), -math.cos(az) * math.cos(el), math.sin(el)))

    cam_data = bpy.data.cameras.new("RenderCam")
    cam_data.lens = camera.focal
    cam_data.sensor_fit = "HORIZONTAL"
    cam_data.sensor_width = 36.0
    cam_data.clip_start = 0.01
    cam = bpy.data.objects.new("RenderCam", cam_data)
    cam.location = camera.target + offset * distance
    aim(cam, camera.target)
    scene.collection.objects.link(cam)
    scene.camera = cam

    # Showroom light, laid out for the card's front-left camera and turned
    # with each view so every side gets the same look. A black product is
    # shaped by what it reflects, so the key, fill and rim are tall strip
    # softboxes: each lays a clean vertical highlight down the cylinder, with
    # dark body between them. A wide top box lights the lid and the bowl.
    # Views from below mirror the rig. Sizes are (width, height) in meters.
    turn = math.radians(camera.azimuth - CAMERAS["card"].azimuth)
    centre = Vector((0.0, 0.0, 0.15))
    rig = [
        ("Key", 300.0, (0.35, 1.8), (-1.2, -1.4, 1.2)),
        ("Fill", 110.0, (0.25, 1.8), (1.5, -0.8, 0.6)),
        ("Rim", 260.0, (0.2, 1.6), (0.5, 1.4, 1.0)),
        ("Top", 90.0, (1.2, 1.2), (0.0, 0.0, 2.0)),
    ]
    rig += [(f"Rim{i}", energy, (size, size), location) for i, (energy, size, location) in enumerate(look.extra, start=2)]
    for name, energy, (width, height), (x, y, z) in rig:
        light_data = bpy.data.lights.new(name, "AREA")
        light_data.energy = energy * look.energy.get(name, 1.0)
        light_data.shape = "RECTANGLE"
        light_data.size = width
        light_data.size_y = height
        light = bpy.data.objects.new(name, light_data)
        light.location = (
            x * math.cos(turn) + y * math.sin(turn),
            -x * math.sin(turn) + y * math.cos(turn),
            z if camera.elevation >= 0.0 else 2.0 * centre.z - z,
        )
        aim(light, centre)
        scene.collection.objects.link(light)

    # A shadow catcher keeps the transparent background but grounds the feeder
    # with a soft contact shadow, as a product card expects.
    if look.floor:
        bpy.ops.mesh.primitive_plane_add(size=4.0, location=(0.0, 0.0, 0.0))
        catcher = bpy.context.active_object
        catcher.is_shadow_catcher = True
        # It must not bounce light: a lit floor would wash the matte black body grey.
        catcher.visible_diffuse = False
        catcher.visible_glossy = False

    # A studio sweep the camera never sees (the film is transparent) but every
    # gloss surface reflects: dark all round, bright only overhead. A flat
    # grey world gives the ring and bowl nothing to mirror and greys them out.
    world = bpy.data.worlds.new("RenderWorld")
    world.use_nodes = True
    nodes, links = world.node_tree.nodes, world.node_tree.links
    background = next(n for n in nodes if n.type == "BACKGROUND")
    coords = nodes.new("ShaderNodeTexCoord")
    split = nodes.new("ShaderNodeSeparateXYZ")
    ramp = nodes.new("ShaderNodeValToRGB")
    ramp.color_ramp.elements[0].position = 0.6
    ramp.color_ramp.elements[0].color = (0.015, 0.015, 0.017, 1.0)
    ramp.color_ramp.elements[1].position = 0.97
    ramp.color_ramp.elements[1].color = (0.9, 0.9, 0.92, 1.0)
    remap = nodes.new("ShaderNodeMapRange")
    links.new(coords.outputs["Generated"], split.inputs[0])
    links.new(split.outputs["Z"], remap.inputs["Value"])
    remap.inputs["From Min"].default_value = -1.0
    links.new(remap.outputs["Result"], ramp.inputs["Fac"])
    links.new(ramp.outputs["Color"], background.inputs["Color"])
    background.inputs["Strength"].default_value = 0.4
    scene.world = world


def configure(size, samples):
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.samples = samples
    scene.cycles.use_denoising = True
    scene.cycles.seed = 0
    scene.render.film_transparent = True
    scene.render.resolution_x = size
    scene.render.resolution_y = size
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    scene.render.image_settings.color_depth = "8"
    scene.view_settings.view_transform = "AgX"
    # AgX's base look flattens a dark product to grey; this keeps the blacks
    # deep and the highlights crisp without clipping the steel.
    scene.view_settings.look = "AgX - Medium High Contrast"

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
            return backend
    return "CPU"


# Hotspots.


def part_points(part):
    """World points that bound a hotspot part, and the objects that count as it."""
    if PARTS[part] is None:
        base = bpy.data.objects["Base"]
        cavity = base.material_slots.find("PL_Cavity")
        points = [
            base.matrix_world @ base.data.vertices[v].co
            for poly in base.data.polygons
            if poly.material_index == cavity and (base.matrix_world @ poly.center).y < -0.05
            for v in poly.vertices
        ]
        return points, {base}

    owned = set(descendants(bpy.data.objects[PARTS[part]]))
    points = [c for obj in owned if not obj.hide_render for c in world_corners(obj)]
    return points, owned


def reaches(origin, target, owned, tolerance):
    scene = bpy.context.scene
    depsgraph = bpy.context.evaluated_depsgraph_get()
    direction = target - origin
    # Every part faces out from the feeder's vertical axis. One turned past
    # about 75 degrees from the view can't be read even when a ray grazes it.
    # The icons are flat curves, so their own normals don't show this.
    outward = Vector((target.x, target.y, 0.0)).normalized()
    if outward.dot(-direction.normalized()) < 0.25:
        return False

    hit, location, _, _, obj, _ = scene.ray_cast(depsgraph, origin, direction.normalized(), distance=direction.length + tolerance)
    if not hit:
        return True

    # A print can sit flush with the body, so a hit at the part's own center
    # counts as reaching it.
    return obj.original in owned or (location - target).length <= tolerance


def hotspots(view, size):
    scene = bpy.context.scene
    camera = scene.camera
    spots = {}
    for part in PARTS:
        points, owned = part_points(part)
        low = Vector([min(p[i] for p in points) for i in range(3)])
        high = Vector([max(p[i] for p in points) for i in range(3)])
        centre = (low + high) / 2.0

        corners = [Vector((x, y, z)) for x in (low.x, high.x) for y in (low.y, high.y) for z in (low.z, high.z)]
        projected = [world_to_camera_view(scene, camera, c) for c in corners]
        middle = world_to_camera_view(scene, camera, centre)
        span = max(
            max(p.x for p in projected) - min(p.x for p in projected),
            max(p.y for p in projected) - min(p.y for p in projected),
        )
        tolerance = max(0.002, 0.25 * max(high - low))
        framed = 0.0 <= middle.x <= 1.0 and 0.0 <= middle.y <= 1.0

        spots[part] = {
            "x": round(middle.x, 4),
            "y": round(1.0 - middle.y, 4),
            "r": round(span / 2.0, 4),
            "visible": framed and reaches(camera.matrix_world.translation, centre, owned, tolerance),
        }
    return {"view": view, "size": [size, size], "hotspots": spots}


def hotspot_json(data):
    # One line per part, as RENDERS.md shows it, so a diff names the part.
    lines = [f'    "{part}": {json.dumps(spot)}' for part, spot in data["hotspots"].items()]
    return f'{{\n  "view": "{data["view"]}",\n  "size": {json.dumps(data["size"])},\n  "hotspots": {{\n' + ",\n".join(lines) + "\n  }\n}\n"


def draw_check(image_path, data, out):
    # The hotspot circles over the render on white, to check that each lands
    # on its part: a thick ring when visible, a thin one when occluded.
    image = bpy.data.images.load(str(image_path))
    width, height = image.size
    pixels = np.array(image.pixels[:], dtype=np.float32).reshape(height, width, 4)
    alpha = pixels[..., 3:4]
    pixels[..., :3] = pixels[..., :3] * alpha + (1.0 - alpha)
    pixels[..., 3] = 1.0

    rows, cols = np.mgrid[0:height, 0:width]
    spots = data["hotspots"]
    for index, spot in enumerate(spots.values()):
        hue = 2.0 * math.pi * index / len(spots)
        colour = [0.5 + 0.5 * math.cos(hue + 2.0 * math.pi * k / 3.0) for k in range(3)]
        # Blender's pixel rows run bottom up.
        cx, cy = spot["x"] * width, (1.0 - spot["y"]) * height
        radius = max(spot["r"] * width, 4.0)
        ring = np.abs(np.hypot(cols - cx, rows - cy) - radius) < (2.0 if spot["visible"] else 0.75)
        pixels[ring, :3] = colour

    image.pixels[:] = pixels.ravel()
    image.filepath_raw = str(out)
    image.file_format = "PNG"
    image.save()


# Driving.


def optimize(path):
    tool = shutil.which("oxipng")
    if tool is None:
        print(f"warning: oxipng is not on PATH, so {path.name} is not optimized", flush=True)
        return
    subprocess.run([tool, "-o", "4", "--strip", "safe", "-q", str(path)], check=True)


def render_one(args):
    view = VIEWS[args.view]
    look = Look()
    unbrand()
    truthful()
    if view.setup is not None:
        view.setup(look)
    if args.state is not None:
        view.states[args.state](look)
    stage(CAMERAS[view.camera], look)
    device = configure(args.size, args.samples)

    name = output_name(args.view, args.state)
    args.out.mkdir(parents=True, exist_ok=True)
    out = args.out / f"{name}.png"
    bpy.context.scene.render.filepath = str(out)
    started = time.monotonic()
    bpy.ops.render.render(write_still=True)
    elapsed = time.monotonic() - started
    optimize(out)

    if view.hotspots and args.state is None:
        data = hotspots(args.view, args.size)
        (args.out / f"{args.view}.json").write_text(hotspot_json(data))
        if args.check:
            CHECKS.mkdir(exist_ok=True)
            draw_check(out, data, CHECKS / f"{args.view}-hotspots.png")
    print(f"rendered {name} in {elapsed:.1f} s on {device}", flush=True)

    # The cutout is the same image without the contact shadow, for layouts
    # that ground the feeder themselves. The catcher stays in the scene, hidden
    # only from the camera: removing it changes how Cycles lights the body and
    # the cutout would come out darker. The view's hotspots still apply.
    cutout = args.out / f"{name}-cutout.png"
    catchers = [obj for obj in bpy.data.objects if obj.is_shadow_catcher]
    if not catchers:
        shutil.copyfile(out, cutout)
        print(f"rendered {name}-cutout as a copy (no floor)", flush=True)
        return
    for catcher in catchers:
        catcher.visible_camera = False
    bpy.context.scene.render.filepath = str(cutout)
    started = time.monotonic()
    bpy.ops.render.render(write_still=True)
    elapsed = time.monotonic() - started
    optimize(cutout)
    print(f"rendered {name}-cutout in {elapsed:.1f} s on {device}", flush=True)


def render_all(args):
    # Each image gets a fresh Blender, so no state's changes leak into the next
    # and any image regenerated alone comes out the same.
    script = Path(__file__).resolve()
    for view, state in outputs():
        command = [bpy.app.binary_path, "-b", bpy.data.filepath, "--python-exit-code", "1", "-P", str(script), "--", "--view", view]
        command += ["--state", state] if state else []
        command += ["--size", str(args.size), "--samples", str(args.samples), "--out", str(args.out)]
        command += ["--check"] if args.check else []
        result = subprocess.run(command, capture_output=True, text=True)
        lines = [line for line in result.stdout.splitlines() if line.startswith(("rendered", "warning"))]
        print("\n".join(lines), flush=True)
        if result.returncode != 0 or not lines:
            sys.exit(f"{output_name(view, state)} failed:\n{result.stdout[-2000:]}\n{result.stderr[-2000:]}")


def main():
    args = parse_args()
    if args.all:
        render_all(args)
    else:
        render_one(args)


# Blender runs a -P script as __main__; lineart.py imports this one for its
# cameras, unbranding and hotspot projection.
if __name__ == "__main__":
    main()
