"""Generates the battle arena: an inside-out cube the camera fights
inside, checker-textured so distance and motion read clearly.

Battles are full 3D — no pre-rendered background — so the arena is a
room the camera is placed within: every face winds counter-clockwise
as seen from the inside and its normal points at the room's center.

    python3 tools/generate_arena.py
"""

import math
import os

from generate_cursor import write_png
from gltf import Writer

OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "models", "arena.glb")

# The room: 40 wide, 14 tall, 40 deep; the floor sits at y=0 and the
# combatants form their lines on it. Sized so a corner camera can see
# the whole fight with the walls far off.
W, H, D = 40.0, 14.0, 40.0


def checker_png():
    """An 8x8 two-tone checker, one tile per face UV."""
    checks = 8
    cell = 8
    size = checks * cell
    light, dark = (198, 198, 205, 255), (74, 74, 84, 255)
    rows = []
    for y in range(size):
        row = bytearray()
        for x in range(size):
            check = ((x // cell) + (y // cell)) % 2
            row += bytes(dark if check else light)
        rows.append(b"\x00" + bytes(row))
    raw = b"".join(rows)
    path = os.path.join("/tmp", "arena-checker.png")
    write_png(path, size, size, raw)
    with open(path, "rb") as handle:
        return handle.read()


def norm(v):
    length = math.sqrt(sum(c * c for c in v))
    return tuple(c / length for c in v)


def cross(a, b):
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def face(corner, u, v):
    """One inward-facing quad: corners wind counter-clockwise when
    viewed from inside the room, normal pointing at the center."""
    normal = norm(cross(u, v))
    corners = [corner, add(corner, u), add(add(corner, u), v), add(corner, v)]
    uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
    return corners, normal, uvs


def add(a, b):
    return tuple(x + y for x, y in zip(a, b))


def faces():
    """The six walls. Each (corner, u, v) is ordered so cross(u, v)
    points into the room."""
    x0, x1 = -W / 2, W / 2
    z0, z1 = -D / 2, D / 2
    return [
        face((x0, 0.0, z0), (0.0, 0.0, D), (W, 0.0, 0.0)),    # floor, +Y
        face((x0, H, z0), (W, 0.0, 0.0), (0.0, 0.0, D)),      # ceiling, -Y
        face((x0, 0.0, z1), (0.0, H, 0.0), (W, 0.0, 0.0)),    # front, -Z
        face((x0, 0.0, z0), (W, 0.0, 0.0), (0.0, H, 0.0)),    # back, +Z
        face((x0, 0.0, z0), (0.0, H, 0.0), (0.0, 0.0, D)),    # left, +X
        face((x1, 0.0, z0), (0.0, 0.0, D), (0.0, H, 0.0)),    # right, -X
    ]


def main():
    w = Writer()
    image = w.add_image(checker_png())
    w.images[image]["name"] = "arena-checker"
    texture = w.add_texture(image, w.add_sampler())
    material = w.add_textured_material("checker", texture, unlit=True)

    positions, normals, uvs, indices = [], [], [], []
    for corners, normal, face_uvs in faces():
        base = len(positions)
        positions.extend(corners)
        normals.extend([normal] * 4)
        uvs.extend(face_uvs)
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3])

    w.add_mesh("arena", [(positions, normals, indices, uvs)], [material])
    node = {"name": "arena", "mesh": 0}
    w.nodes.append(node)

    gltf = {
        "asset": {"version": "2.0", "generator": "wakeful tools/generate_arena.py"},
        "extensionsUsed": ["KHR_materials_unlit"],
        "scene": 0,
        "scenes": [{"name": "arena", "nodes": [0]}],
        "nodes": w.nodes,
        "meshes": w.meshes,
        "materials": w.materials,
        "textures": w.textures,
        "images": w.images,
        "samplers": w.samplers,
        "buffers": [{"byteLength": len(w.bin)}],
        "bufferViews": w.views,
        "accessors": w.accessors,
    }

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "wb") as handle:
        handle.write(w.glb(gltf))
    print(f"wrote {os.path.abspath(OUT)} ({len(w.bin)} byte buffer, {W}x{H}x{D} room)")


if __name__ == "__main__":
    main()
