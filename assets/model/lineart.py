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
# Past this, two faces fold back on each other (a thin flange or lip). Seen
# from any side it reads as an edge of the part, so it draws as an outline
# whichever way its faces turn; otherwise it flickers between outline and
# crease from facet to facet.
FOLD = math.radians(100)
# A face turned this little away from the camera still counts as facing it
# for creases. The band's lower cone is nearly edge-on, so its facets turn
# just toward and just away from the camera in turn; judged strictly, the
# creases along it would draw only on every other facet.
GRAZE = 0.2
# The sharpest turn a chain takes from one edge to the next.
TURN = math.radians(40)
STEP_PX = 1.2
# A sample is visible when the first surface its ray meets is the sample's
# own: within this distance of it. That holds at grazing silhouettes and
# doesn't depend on which way the model's normals point.
REACH = 0.0025
# Hidden stretches this many samples long or shorter, between visible ones
# along one line, are ray-cast noise in a narrow groove or a seam, not
# occlusion. Bridging runs along whole lines, not single edges, so a gap of a
# few facets closes too.
BRIDGE = 20
# Visible scraps shorter than this, left between real occlusions, are noise.
SPECK_PX = 4.0
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


def kind_of(edge, eye, obj, role):
    """Whether an edge draws, and as what: outline, crease, edge or None."""
    a, b = edge.verts[0].co, edge.verts[1].co
    view = (eye - (a + b) / 2).normalized()
    faces = edge.link_faces
    if len(faces) == 1:
        # Flat prints (the ring icons are curves) draw every boundary.
        return "edge" if obj.type != "MESH" or faces[0].normal.dot(view) > -0.05 else None
    if len(faces) != 2:
        return None
    n1, n2 = faces[0].normal, faces[1].normal
    f1, f2 = n1.dot(view), n2.dot(view)
    angle = n1.angle(n2, 0.0)
    if angle > FOLD and (f1 > 0 or f2 > 0):
        return "outline"
    # Kibble reads as outlines; its facets would only add noise.
    if role != "food" and angle > CREASE and min(f1, f2) > -GRAZE and max(f1, f2) > 0:
        return "crease"
    if (f1 > 0) != (f2 > 0):
        return "outline"
    return None


def walk(pairs, co):
    """Join edges (vertex index pairs) into vertex chains that run straight on.

    At each vertex a chain continues to the unused edge that turns least, and
    stops where every edge turns more than TURN, so a chain follows one line
    of the part (a rim, a seam) instead of zig-zagging across a strip of
    facets.
    """
    links = {}
    for i, j in pairs:
        links.setdefault(i, []).append(j)
        links.setdefault(j, []).append(i)
    used = set()
    chains = []
    for i, j in pairs:
        if (i, j) in used:
            continue
        used.update({(i, j), (j, i)})
        chain = [i, j]
        # Grow from the tail, then turn the chain round and grow the other end.
        for _ in range(2):
            while True:
                prev, tip = chain[-2], chain[-1]
                heading = co[tip] - co[prev]
                turns = [(heading.angle(co[k] - co[tip], math.pi), k) for k in links[tip] if (tip, k) not in used and (co[k] - co[tip]).length > 0]
                if not turns or min(turns)[0] > TURN:
                    break
                nxt = min(turns)[1]
                used.update({(tip, nxt), (nxt, tip)})
                chain.append(nxt)
            chain.reverse()
        chains.append(chain)
    return chains


def candidates(eye):
    """World-space lines that could draw, as point chains by part and kind."""
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

        # Chain each kind's edges along the mesh, so visibility is judged
        # along a whole line rather than edge by edge.
        kinds = {}
        for edge in bm.edges:
            kind = kind_of(edge, eye, obj, role)
            if kind is not None:
                kinds.setdefault(kind, []).append((edge.verts[0].index, edge.verts[1].index))
        bm.verts.ensure_lookup_table()
        co = [v.co.copy() for v in bm.verts]
        for kind, pairs in kinds.items():
            for chain in walk(pairs, co):
                found.append((role, kind, [co[i] for i in chain]))

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
    for role, kind, chain in edges:
        points = [chain[0]]
        for a, b in zip(chain, chain[1:]):
            steps = max(1, math.ceil(math.dist(project(a), project(b)) / STEP_PX))
            points += [a.lerp(b, i / steps) for i in range(1, steps + 1)]
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
    # Trace the designed edges, not their rounding. A bevel modifier turns a
    # sharp edge into a strip of shallow facets, each under CREASE, so the
    # crease would vanish or show only where a facet happens to cross the
    # threshold. Off for the tracing and the ray casts alike, so the two agree;
    # the model file keeps them.
    for obj in bpy.data.objects:
        for modifier in obj.modifiers:
            if modifier.type == "BEVEL":
                modifier.show_viewport = modifier.show_render = False
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
            shortest = SHORTEST_PX if role in ("icon", "food") else SPECK_PX
            if sum(math.dist(line[i], line[i + 1]) for i in range(len(line) - 1)) < shortest:
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
    print(f"traced {OUT.name}: {count} paths from {len(edges)} lines in {time.monotonic() - started:.1f} s", flush=True)


main()
