#!/usr/bin/env python3
"""Generates wakeful scene assets from a Blender file: the rendered
background ("plate"), its matching depth map, and a .export file holding
the half of the scene Blender owns.

The .scene file itself is finished by `scene-merge`
(`cargo run --bin scene-merge`), which folds the .export into whatever is
already on disk and keeps the actors, teleporters and scene script a
re-export has no business overwriting. `tools/export_scenes.sh` runs both
halves.

    blender -b raw_assets/village_1.blend -P tools/generate_scene.py \
        -- [--name village_1] [--resolution 320x240]

A blend whose "Cameras" collection holds sub-collections carries a scene
per sub-collection and needs no name at all:

    blender -b raw_assets/village_whole.blend -P tools/generate_scene.py \
        -- [--resolution 320x240]

Conventions (must match src/systems/depth_card.rs and
tools/generate_test_depth.py):

- The plate and the depth map share one pixel grid, at `--resolution`
  (default 320x240, the game's virtual screen). The aspect must be 4:3 —
  the in-game view is a 320x240 window onto the plate, and a window can
  only cleanly crop a same-aspect plate.
- Depth is the ray distance from the scene camera, in meters, encoded as
  dist / depth_range * 255 in an 8-bit gray image; no-hit pixels (sky)
  encode depth_range (far). depth_range adapts: the farthest hit rounded
  up to a whole meter.
- The scene file's CameraPose is the pose the plate was baked with:
  position is the Blender camera mapped from Z-up to the game's Y-up by
  (x, y, z) -> (x, z, -y), target is position + mapped forward * 10, and
  fov_degrees is the plate's vertical fov — the fov the depth grid was
  ray-cast with, which the depth card must unproject through. The plate
  size therefore changes detail only, never framing.

A blend can carry more than one scene. A "Cameras" collection whose
children are collections is one scene per child, named after it: each
child holds the camera for that scene plus the walk mesh that scene will
walk on (exported as the scene's triangles, see below). Since a walk mesh
is authoring data rather than plate content, the whole Cameras collection
is excluded from both the render and the depth grid while exporting.

Walk meshes: every mesh object inside a scene's sub-collection is part of
that scene's walkable ground, whatever it is named. Each mesh is read
through the depsgraph, so modifiers (the hill's Shrinkwrap) apply; the
object's own transform is baked into its vertices, and duplicates are
welded at a millimeter so the triangle soup stays small. A triangle with
no area when seen from above (a wall) is dropped: it is never walkable
ground, and keeping it would only slow queries down.

Reserved for later work (do not break these):
- Camera pan: a plate bigger than the game's 320x240 view exists so a
  sub-viewport window can slide across it (following the player,
  clamped at the plate edges). When that engine feature lands, the
  scene format gains a serde-defaulted plate record carrying the window
  fov (tan(win/2) = tan(plate/2) * 240 / plate_height) and slide ranges;
  until then there is exactly one fov and it is the plate's.
"""

import argparse
import contextlib
import math
import os
import struct
import sys
import zlib

import bpy
from mathutils import Vector

# The in-game view: a 320x240 window onto the plate.
WINDOW = (320, 240)
# The collection whose children are the blend's scenes.
CAMERAS = "Cameras"
# Decimal places walk-mesh corners are welded at. A millimeter is finer
# than any character can notice and keeps the vertex list from doubling
# up on shared corners.
WELD_PRECISION = 3


def parse_args():
    argv = sys.argv
    extra = argv[argv.index("--") + 1 :] if "--" in argv else []
    parser = argparse.ArgumentParser(description="blend -> wakeful scene assets")
    parser.add_argument(
        "--name",
        default=None,
        help=(
            "asset basename (default: the blend file's name); ignored by "
            "multi-camera blends, which name each scene after its Cameras "
            "sub-collection"
        ),
    )
    parser.add_argument(
        "--resolution",
        default="320x240",
        help="plate size in pixels, 4:3 (default: 320x240)",
    )
    args = parser.parse_args(extra)
    width, _, height = args.resolution.lower().partition("x")
    plate = (int(width), int(height))
    if plate[0] * 3 != plate[1] * 4:
        sys.exit(
            f"resolution {plate[0]}x{plate[1]} is not 4:3 — the game's view "
            "is a 320x240 window onto the plate, so the plate must share "
            "its aspect (320x240, 640x480, 960x720, ...)"
        )
    return args, plate


def camera_targets(args):
    """The (name, camera) pairs to export, in blend order.

    A "Cameras" collection with sub-collections is one scene per child,
    named after it — that is how one blend carries a whole region.
    Anything else is a single scene: the blend's active camera, named
    after `--name` or the blend file.
    """
    groups = camera_groups()
    if groups:
        if args.name is not None:
            sys.exit(
                "--name does not apply here: this blend's scenes are named "
                f"after their {CAMERAS} sub-collections ({', '.join(g.name for g in groups)})"
            )
        targets = []
        for group in groups:
            cameras = list(cameras_in(group))
            if not cameras:
                sys.exit(f"the {CAMERAS}/{group.name} collection holds no camera")
            if len(cameras) > 1:
                print(
                    f"[generate_scene] warning: {CAMERAS}/{group.name} holds "
                    f"{len(cameras)} cameras; using {cameras[0].name}"
                )
            targets.append((group.name, cameras[0]))
        return targets

    name = args.name
    if name is None:
        if not bpy.data.filepath:
            sys.exit("no name given and the file is not saved")
        name = os.path.splitext(os.path.basename(bpy.data.filepath))[0]
    camera = bpy.context.scene.camera
    if camera is None:
        sys.exit("the blend has no active camera")
    return [(name, camera)]


def camera_groups():
    """The Cameras collection's sub-collections, when the blend has one."""
    cameras = bpy.data.collections.get(CAMERAS)
    return list(cameras.children) if cameras else []


def cameras_in(collection):
    """Every camera in the collection, its own children included."""
    for obj in collection.objects:
        if obj.type == "CAMERA":
            yield obj
    for child in collection.children:
        yield from cameras_in(child)


def walk_meshes_in(collection):
    """Every mesh in the collection, its own children included.

    Whatever a scene's sub-collection holds that is not its camera is that
    scene's walkable ground.
    """
    for obj in collection.objects:
        if obj.type == "MESH":
            yield obj
    for child in collection.children:
        yield from walk_meshes_in(child)


def walk_mesh(collection):
    """The collection's meshes as welded world-space triangles.

    Returns `(vertices, triangles)` ready for the scene file. Reads each
    object through the depsgraph so modifiers apply, then welds duplicate
    corners at [`WELD_PRECISION`] and drops triangles that have no
    footprint on the ground.
    """
    depsgraph = bpy.context.evaluated_depsgraph_get()
    vertices = []
    corners = {}
    triangles = []
    for obj in walk_meshes_in(collection):
        evaluated = obj.evaluated_get(depsgraph)
        mesh = evaluated.to_mesh()
        try:
            mesh.calc_loop_triangles()
            remap = {}
            for vertex in mesh.vertices:
                key = tuple(
                    round(c, WELD_PRECISION) for c in to_game(evaluated.matrix_world @ vertex.co)
                )
                if key not in corners:
                    corners[key] = len(vertices)
                    vertices.append(key)
                remap[vertex.index] = corners[key]
            for triangle in mesh.loop_triangles:
                a, b, c = (remap[i] for i in triangle.vertices)
                if a == b or b == c or a == c:
                    continue
                if not footprint(vertices[a], vertices[b], vertices[c]):
                    continue
                triangles.append((a, b, c))
        finally:
            evaluated.to_mesh_clear()
    return vertices, triangles


def footprint(a, b, c):
    """Whether a triangle covers any area when seen from directly above.

    A vertical wall does not: it can never hold a character standing on it.
    """
    return abs((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])) > 1e-9


@contextlib.contextmanager
def walk_meshes_hidden():
    """Keeps the Cameras collection out of the render and the depth grid.

    Each sub-collection pairs a camera with that scene's walk mesh, which
    must appear in neither the plate nor a depth hit. Excluding the
    collection from the view layer removes it from both; an object's
    `hide_render` flag only reaches the render, not `scene.ray_cast`.
    """
    layer = bpy.context.view_layer.layer_collection.children.get(CAMERAS)
    excluded = layer.exclude if layer is not None else None
    if layer is not None:
        layer.exclude = True
    try:
        yield
    finally:
        if layer is not None:
            layer.exclude = excluded


def assets_root():
    """The repository's assets folder, from this script's own location.

    The exporter may run through flatpak, whose sandbox neither inherits
    the working directory nor writes to relative paths, so where the
    blend file lives decides nothing and neither does the caller's cwd.
    """
    return os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "assets"
    )


def background_path(name):
    return os.path.join(assets_root(), "backgrounds", f"{name}.png")


def depth_map_path(name):
    return os.path.join(assets_root(), "backgrounds", f"{name}_depth.png")


def export_path(name):
    """The scene's Blender-owned half. `scene-merge` folds it into the
    .scene file beside it, which this script never touches."""
    return os.path.join(assets_root(), "scenes", f"{name}.export")


def plate_fovs(camera, plate):
    """The plate's horizontal/vertical fov from the camera lens."""
    data = camera.data
    fit = data.sensor_fit
    if fit == "AUTO":
        fit = "HORIZONTAL" if plate[0] >= plate[1] else "VERTICAL"
    if fit == "HORIZONTAL":
        hfov = 2.0 * math.atan(data.sensor_width / (2.0 * data.lens))
        vfov = 2.0 * math.atan(math.tan(hfov / 2.0) * plate[1] / plate[0])
    else:
        vfov = 2.0 * math.atan(data.sensor_height / (2.0 * data.lens))
        hfov = 2.0 * math.atan(math.tan(vfov / 2.0) * plate[0] / plate[1])
    return hfov, vfov


def window_vfov(plate_vfov, plate):
    """The vertical fov of a 320x240 window onto the plate.

    Not used yet: today the scene's single fov is the plate's (the depth
    card bakes through it). This is the fov the pan feature's plate
    record will carry for the sub-viewport window.
    """
    return 2.0 * math.atan(math.tan(plate_vfov / 2.0) * WINDOW[1] / plate[1])


def to_game(v):
    """Blender Z-up -> game Y-up: (x, y, z) -> (x, z, -y)."""
    return (v[0], v[2], -v[1])


def render_color(camera, plate, path):
    scene = bpy.context.scene
    scene.render.resolution_x = plate[0]
    scene.render.resolution_y = plate[1]
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGB"
    scene.render.image_settings.color_depth = "8"
    scene.render.filepath = path
    scene.camera = camera
    bpy.ops.render.render(write_still=True)


def ray_grid(camera, plate):
    """Per-pixel ray distance from the camera into the scene's geometry."""
    matrix = camera.matrix_world
    origin = matrix.translation
    quat = matrix.to_quaternion()
    forward = quat @ Vector((0.0, 0.0, -1.0))
    right = quat @ Vector((1.0, 0.0, 0.0))
    up = quat @ Vector((0.0, 1.0, 0.0))
    hfov, vfov = plate_fovs(camera, plate)
    tan_h = math.tan(hfov / 2.0)
    tan_v = math.tan(vfov / 2.0)

    depsgraph = bpy.context.evaluated_depsgraph_get()
    grid = []
    for y in range(plate[1]):
        ndc_y = 1.0 - 2.0 * ((y + 0.5) / plate[1])
        row = []
        for x in range(plate[0]):
            ndc_x = ((x + 0.5) / plate[0]) * 2.0 - 1.0
            direction = (forward + right * (ndc_x * tan_h) + up * (ndc_y * tan_v)).normalized()
            hit, location, _, _, _, _ = bpy.context.scene.ray_cast(
                depsgraph, origin, direction
            )
            row.append((location - origin).length if hit else None)
        grid.append(row)
    return grid


def encode_rows(grid, depth_range):
    rows = []
    for row in grid:
        encoded = bytearray(len(row))
        for x, dist in enumerate(row):
            if dist is None:
                encoded[x] = 255  # no-hit: depth_range itself (far)
            else:
                encoded[x] = min(255, round(dist / depth_range * 255))
        rows.append(bytes(encoded))
    return rows


def write_gray_png(path, plate, rows):
    def chunk(tag, payload):
        return (
            struct.pack(">I", len(payload))
            + tag
            + payload
            + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
        )

    raw = b"".join(b"\x00" + row for row in rows)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", plate[0], plate[1], 8, 0, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as fh:
        fh.write(png)


def ron_float(v):
    return repr(round(float(v), 5))


def export_ron(origin, target, fov, name, depth_range, pan=None, walk=None):
    pos = ", ".join(ron_float(c) for c in origin)
    look = ", ".join(ron_float(c) for c in target)
    pan_line = (
        f"    pan: Some((plate: ({pan[0]}, {pan[1]}), window: (320, 240))),\n"
        if pan is not None
        else ""
    )
    if walk is None:
        mesh_line = "    walk_mesh: None,\n"
    else:
        vertices, triangles = walk
        corners = ", ".join(
            f"({ron_float(x)}, {ron_float(y)}, {ron_float(z)})" for x, y, z in vertices
        )
        faces = ", ".join(f"({a}, {b}, {c})" for a, b, c in triangles)
        mesh_line = (
            f"    walk_mesh: Some((\n"
            f"        vertices: [{corners}],\n"
            f"        triangles: [{faces}],\n"
            f"    )),\n"
        )
    return f"""(
    camera: (
        position: ({pos}),
        target: ({look}),
        fov_degrees: {ron_float(math.degrees(fov))},
    ),
{mesh_line}    background: Some("backgrounds/{name}.png"),
    depth_map: Some("backgrounds/{name}_depth.png"),
    depth_range: {ron_float(depth_range)},
{pan_line})
"""


def export_scene(name, camera, plate, walk=None):
    print(f"[generate_scene] {name}: rendering the plate")

    background = background_path(name)
    render_color(camera, plate, background)
    print(f"[generate_scene] wrote {background}")

    grid = ray_grid(camera, plate)
    hits = [d for row in grid for d in row if d is not None]
    depth_range = max(1.0, math.ceil(max(hits))) if hits else 1.0
    depth = depth_map_path(name)
    write_gray_png(depth, plate, encode_rows(grid, depth_range))
    print(f"[generate_scene] wrote {depth} (range {depth_range}m)")

    origin = to_game(camera.matrix_world.translation)
    forward = to_game(camera.matrix_world.to_quaternion() @ Vector((0.0, 0.0, -1.0)))
    target = tuple(o + f * 10.0 for o, f in zip(origin, forward))
    # The scene's fov is the plate's: the depth card unprojects the map
    # through it, so the two must agree exactly.
    _, fov = plate_fovs(camera, plate)
    path = export_path(name)
    with open(path, "w") as fh:
        fh.write(
            export_ron(
                origin,
                target,
                fov,
                name,
                depth_range,
                pan=plate if plate != WINDOW else None,
                walk=walk,
            )
        )
    print(f"[generate_scene] wrote {path}")
    print(
        "[generate_scene] fold it into the scene with: "
        "cargo run --bin scene-merge"
    )
    print(
        "[generate_scene] camera pose: position="
        f"{tuple(round(c, 3) for c in origin)} "
        f"target={tuple(round(c, 3) for c in target)} "
        f"fov={math.degrees(fov):.2f}deg"
    )


def main():
    args, plate = parse_args()
    groups = camera_groups()
    targets = camera_targets(args)
    print(f"[generate_scene] {len(targets)} scene(s) plate={plate[0]}x{plate[1]}")

    # Read the walk meshes before the collection goes out of the view layer:
    # the meshes are authoring data, not plate content.
    meshes = {
        group.name: walk_mesh(group) for group in groups
    }
    for name, mesh in meshes.items():
        print(
            f"[generate_scene] {name}: walk mesh {len(mesh[0])} vertices, "
            f"{len(mesh[1])} triangles"
        )

    with walk_meshes_hidden():
        for name, camera in targets:
            export_scene(name, camera, plate, walk=meshes.get(name))


if __name__ == "__main__":
    main()
