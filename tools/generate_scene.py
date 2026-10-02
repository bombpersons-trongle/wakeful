#!/usr/bin/env python3
"""Generates wakeful scene assets from a Blender file: the rendered
background ("plate"), its matching depth map, and the .scene file that
wires them up.

    blender -b raw_assets/scenes/village_1.blend -P tools/generate_scene.py \
        -- [--name village_1] [--resolution 640x480]

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

Reserved for later work (do not break these):
- Camera pan: a plate bigger than the game's 320x240 view exists so a
  sub-viewport window can slide across it (following the player,
  clamped at the plate edges). When that engine feature lands, the
  scene format gains a serde-defaulted plate record carrying the window
  fov (tan(win/2) = tan(plate/2) * 240 / plate_height) and slide ranges;
  until then there is exactly one fov and it is the plate's.
- Walk mesh: a "Walkable" collection in the blend will be projected onto
  the walkable grid by this script; for now walkable stays None.
"""

import argparse
import math
import os
import struct
import sys
import zlib

import bpy
from mathutils import Vector

# The in-game view: a 320x240 window onto the plate.
WINDOW = (320, 240)


def parse_args():
    argv = sys.argv
    extra = argv[argv.index("--") + 1 :] if "--" in argv else []
    parser = argparse.ArgumentParser(description="blend -> wakeful scene assets")
    parser.add_argument(
        "--name",
        default=None,
        help="asset basename (default: the blend file's name)",
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
    if args.name is None:
        if not bpy.data.filepath:
            sys.exit("no name given and the file is not saved")
        args.name = os.path.splitext(os.path.basename(bpy.data.filepath))[0]
    return args.name, plate


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


def render_color(plate, path):
    scene = bpy.context.scene
    scene.render.resolution_x = plate[0]
    scene.render.resolution_y = plate[1]
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGB"
    scene.render.image_settings.color_depth = "8"
    scene.render.filepath = path
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


def scene_ron(origin, target, fov, name, depth_range, pan=None):
    pos = ", ".join(ron_float(c) for c in origin)
    look = ", ".join(ron_float(c) for c in target)
    pan_line = (
        f"    pan: Some((plate: ({pan[0]}, {pan[1]}), window: (320, 240))),\n"
        if pan is not None
        else ""
    )
    return f"""(
    camera: (
        position: ({pos}),
        target: ({look}),
        fov_degrees: {ron_float(math.degrees(fov))},
    ),
    walkable: None,
    background: Some("backgrounds/{name}.png"),
    depth_map: Some("backgrounds/{name}_depth.png"),
    depth_range: {ron_float(depth_range)},
{pan_line}    teleporters: [],
    actors: [],
)
"""


def main():
    name, plate = parse_args()
    print(f"[generate_scene] name={name} plate={plate[0]}x{plate[1]}")

    render_color(plate, f"assets/backgrounds/{name}.png")
    print(f"[generate_scene] wrote assets/backgrounds/{name}.png")

    camera = bpy.context.scene.camera
    if camera is None:
        sys.exit("the blend has no active camera")
    grid = ray_grid(camera, plate)
    hits = [d for row in grid for d in row if d is not None]
    depth_range = max(1.0, math.ceil(max(hits))) if hits else 1.0
    write_gray_png(
        f"assets/backgrounds/{name}_depth.png",
        plate,
        encode_rows(grid, depth_range),
    )
    print(
        f"[generate_scene] wrote assets/backgrounds/{name}_depth.png "
        f"(range {depth_range}m)"
    )

    origin = to_game(camera.matrix_world.translation)
    forward = to_game(camera.matrix_world.to_quaternion() @ Vector((0.0, 0.0, -1.0)))
    target = tuple(o + f * 10.0 for o, f in zip(origin, forward))
    # The scene's fov is the plate's: the depth card unprojects the map
    # through it, so the two must agree exactly.
    _, fov = plate_fovs(camera, plate)
    scene_path = f"assets/scenes/{name}.scene"
    with open(scene_path, "w") as fh:
        fh.write(
            scene_ron(
                origin,
                target,
                fov,
                name,
                depth_range,
                pan=plate if plate != WINDOW else None,
            )
        )
    print(f"[generate_scene] wrote {scene_path}")
    print(
        "[generate_scene] camera pose: position="
        f"{tuple(round(c, 3) for c in origin)} "
        f"target={tuple(round(c, 3) for c in target)} "
        f"fov={math.degrees(fov):.2f}deg"
    )


if __name__ == "__main__":
    main()
