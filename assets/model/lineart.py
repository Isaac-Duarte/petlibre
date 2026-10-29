"""Trace the PLAF103 model into hidden-line line art for the site's hero.

Run with Blender 5.2 (not a host tool; Blender's bundled Python):

    blender -b assets/model/plaf103.blend -P assets/model/lineart.py

The drawing is the model's own geometry as render.py's card camera sees it:
silhouettes (an edge between a face toward the camera and one away from it),
creases (faces meeting at a sharp angle) and open boundaries. Every edge is
sampled and each sample ray-cast from the camera, so only visible stretches
survive. The result is assets/renders/card-lines.json: the paths grouped by
part and line kind, so CSS can weight and color them by role, plus the visible
hotspots projected into the same coordinates, so a layout can point at parts.
It is data rather than an SVG file because the site draws it with its own
tokens and a strict CSP: no inline styles, so nothing to inject.
"""

import json
import math
import sys
import time
from pathlib import Path

import bmesh
import bpy
from mathutils import Vector

sys.path.insert(0, str(Path(__file__).resolve().parent))
import render  # noqa: E402  (Blender adds no script directory to the path)

OUT = render.RENDERS / "card-lines.json"
VIEW = "card"

# Drawing units: the camera frame is SIZE across, origin top left.
SIZE = 1000
CREASE = math.radians(32)
STEP_PX = 1.2
# A sample is visible when the first surface its ray meets is the sample's
# own: within this distance of it. That holds at grazing silhouettes and
# doesn't depend on which way the model's normals point.
REACH = 0.0025
# Hidden stretches this many samples long or shorter, between visible ones,
# are ray-cast noise along a crease, not occlusion.
BRIDGE = 10
SIMPLIFY_PX = 0.35
SHORTEST_PX = 1.5
PAD_PX = 12

# Draw order, which the site's draw-in follows: the outline first, the print
# and the food last.
GROUPS = [
    ("body", "outline"),
    ("body", "edge"),
    ("bowl", "outline"),
    ("bowl", "edge"),
    ("body", "crease"),
    ("bowl", "crease"),
    ("window", "outline"),
    ("window", "edge"),
    ("window", "crease"),
    ("icon", "outline"),
    ("icon", "edge"),
    ("icon", "crease"),
    ("food", "outline"),
    ("food", "edge"),
]


def part(name):
    if name.startswith(("KibbleWin", "KibbleBowl")):
        return "food"
    if name.startswith("Icon_"):
        return "icon"
    if name == "WindowGlass":
        return "window"
    if name.startswith("Bowl"):
        return "bowl"
    return "body"


def camera():
    spec = render.CAMERAS[render.VIEWS[VIEW].camera]
    scene = bpy.context.scene
    scene.render.resolution_x = scene.render.resolution_y = SIZE

    distance = spec.width * spec.focal / 36.0
    az, el = math.radians(spec.azimuth), math.radians(spec.elevation)
    data = bpy.data.cameras.new("LineCam")
    data.lens = spec.focal
    data.sensor_fit = "HORIZONTAL"
    data.sensor_width = 36.0
    cam = bpy.data.objects.new("LineCam", data)
    cam.location = spec.target + Vector((math.sin(az) * math.cos(el), -math.cos(az) * math.cos(el), math.sin(el))) * distance
    render.aim(cam, spec.target)
    scene.collection.objects.link(cam)
    scene.camera = cam
    # A new object's world matrix is stale until the view layer updates, and
    # the hotspot projection reads it.
    bpy.context.view_layer.update()
    return cam


def candidates(eye):
    """World-space edges that could draw: silhouettes, creases and boundaries."""
    depsgraph = bpy.context.evaluated_depsgraph_get()
    found = []
    for obj in list(bpy.data.objects):
        if obj.hide_render or obj.type not in {"MESH", "CURVE", "FONT"} or obj.name == "HopperFill":
            continue
        role = part(obj.name)
        evaluated = obj.evaluated_get(depsgraph)
        bm = bmesh.new()
        bm.from_mesh(evaluated.to_mesh())
        bm.transform(obj.matrix_world)
        bm.normal_update()

        for edge in bm.edges:
            a, b = edge.verts[0].co.copy(), edge.verts[1].co.copy()
            view = (eye - (a + b) / 2).normalized()
            faces = edge.link_faces
            if len(faces) == 1:
                # Flat prints (the ring icons are curves) draw every boundary.
                if obj.type != "MESH" or faces[0].normal.dot(view) > -0.05:
                    found.append((role, "edge", a, b))
            elif len(faces) == 2:
                n1, n2 = faces[0].normal, faces[1].normal
                f1, f2 = n1.dot(view), n2.dot(view)
                if (f1 > 0) != (f2 > 0):
                    found.append((role, "outline", a, b))
                # Kibble reads as outlines; its facets would only add noise.
                elif f1 > 0 and role != "food" and n1.angle(n2, 0.0) > CREASE:
                    found.append((role, "crease", a, b))

        bm.free()
        evaluated.to_mesh_clear()
    return found


def bridged(flags):
    out = list(flags)
    i = 0
    while i < len(out):
        if out[i]:
            i += 1
            continue
        j = i
        while j < len(out) and not out[j]:
            j += 1
        if 0 < i and j < len(out) and j - i <= BRIDGE:
            out[i:j] = [True] * (j - i)
        i = j
    return out


def visible_runs(edges, cam):
    scene = bpy.context.scene
    depsgraph = bpy.context.evaluated_depsgraph_get()
    eye = cam.matrix_world.translation

    def project(p):
        v = render.world_to_camera_view(scene, cam, p)
        return (v.x * SIZE, (1.0 - v.y) * SIZE)

    def visible(p):
        direction = p - eye
        hit, location, *_ = scene.ray_cast(depsgraph, eye, direction.normalized(), distance=direction.length + REACH)
        return not hit or (location - p).length < REACH

    runs = {}
    for role, kind, a, b in edges:
        steps = max(1, math.ceil(math.dist(project(a), project(b)) / STEP_PX))
        points = [a.lerp(b, i / steps) for i in range(steps + 1)]
        run = []
        for p, seen in zip(points, bridged([visible(p) for p in points])):
            if seen:
                run.append(project(p))
                continue
            if len(run) > 1:
                runs.setdefault((role, kind), []).append(run)
            run = []
        if len(run) > 1:
            runs.setdefault((role, kind), []).append(run)
    return runs


def key(p):
    return (round(p[0], 1), round(p[1], 1))


def chain(runs):
    """Join runs that share endpoints into long polylines."""
    ends = {}
    for i, run in enumerate(runs):
        ends.setdefault(key(run[0]), []).append(i)
        ends.setdefault(key(run[-1]), []).append(i)

    used = [False] * len(runs)
    lines = []
    for i in range(len(runs)):
        if used[i]:
            continue
        used[i] = True
        line = list(runs[i])
        # Grow from the tail, then turn the line round and grow the other end.
        for _ in range(2):
            while True:
                tip = key(line[-1])
                nxt = next((j for j in ends.get(tip, []) if not used[j]), None)
                if nxt is None:
                    break
                used[nxt] = True
                line.extend((runs[nxt] if key(runs[nxt][0]) == tip else runs[nxt][::-1])[1:])
            line.reverse()
        lines.append(line)
    return lines


def simplify(points):
    """Douglas-Peucker: drop points within SIMPLIFY_PX of the line through their neighbors."""
    if len(points) < 3:
        return points
    (x1, y1), (x2, y2) = points[0], points[-1]
    span = math.hypot(x2 - x1, y2 - y1)

    def off(p):
        if span < 1e-9:
            return math.dist(p, points[0])
        return abs((y2 - y1) * p[0] - (x2 - x1) * p[1] + x2 * y1 - y2 * x1) / span

    index = max(range(1, len(points) - 1), key=lambda i: off(points[i]))
    if off(points[index]) <= SIMPLIFY_PX:
        return [points[0], points[-1]]
    return simplify(points[: index + 1])[:-1] + simplify(points[index:])


def main():
    started = time.monotonic()
    render.unbrand()
    render.truthful()
    cam = camera()

    # Anchors first: the hotspot projection needs the window glass, which the
    # tracing then hides so the food behind it can show.
    spots = render.hotspots(VIEW, SIZE)["hotspots"]
    anchors = {name: [round(s["x"] * SIZE, 1), round(s["y"] * SIZE, 1)] for name, s in spots.items() if s["visible"]}

    edges = candidates(cam.matrix_world.translation)
    render.hide(bpy.data.objects["WindowGlass"])
    runs = visible_runs(edges, cam)

    groups, xs, ys = [], [], []
    for role, kind in GROUPS + sorted(set(runs) - set(GROUPS)):
        paths = []
        for line in chain(runs.get((role, kind), [])):
            line = simplify(line)
            if sum(math.dist(line[i], line[i + 1]) for i in range(len(line) - 1)) < SHORTEST_PX:
                continue
            xs += [p[0] for p in line]
            ys += [p[1] for p in line]
            paths.append((line[0][1], line[0][0], "M" + " L".join(f"{x:.1f} {y:.1f}" for x, y in line)))
        if paths:
            # Top to bottom within a group, so a staggered draw-in runs downward.
            groups.append({"part": role, "line": kind, "paths": [d for *_, d in sorted(paths)]})

    x0, y0 = math.floor(min(xs)) - PAD_PX, math.floor(min(ys)) - PAD_PX
    box = [x0, y0, math.ceil(max(xs)) + PAD_PX - x0, math.ceil(max(ys)) + PAD_PX - y0]

    # One group per line, so a diff names the part that changed.
    body = ",\n".join(f"    {json.dumps(g)}" for g in groups)
    OUT.write_text(f'{{\n  "view": "{VIEW}",\n  "viewBox": {json.dumps(box)},\n  "anchors": {json.dumps(anchors)},\n  "groups": [\n{body}\n  ]\n}}\n')
    count = sum(len(g["paths"]) for g in groups)
    print(f"traced {OUT.name}: {count} paths from {len(edges)} edges in {time.monotonic() - started:.1f} s", flush=True)


main()
