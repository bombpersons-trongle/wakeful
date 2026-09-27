"""Generates assets/models/chest.glb — the treasure chest.

A two-node rig (`base` + `lid`, hinge at the lid's back edge) with a
single `open` clip: the lid swings back about 110 degrees. Scripts play
it with `emote("open")`; the animation driver holds the finished pose
for prop-like models without locomotion clips, so an opened chest stays
open. Run from the repo root: `python3 tools/generate_chest.py`.
"""

import math
import os

from gltf import Writer, box, clip_oneshot, eased

DEG = math.pi / 180

OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "models", "chest.glb")

WOOD = "wood"
WOOD_DARK = "wood_dark"
GOLD = "gold"

MATERIAL_COLORS = {
    WOOD: (0.45, 0.28, 0.15),
    WOOD_DARK: (0.30, 0.18, 0.10),
    GOLD: (0.85, 0.68, 0.20),
}

# (node name, parent, translation, [(mesh size, mesh center, material)]).
# The lid node's translation is the hinge: its back-bottom edge, so a
# negative X rotation tips the lid open backward.
PARTS = [
    ("character", None, (0, 0, 0), None),
    ("base", "character", (0, 0, 0), [
        ((0.52, 0.36, 0.38), (0, 0.18, 0), WOOD),
        ((0.56, 0.06, 0.42), (0, 0.03, 0), WOOD_DARK),
        ((0.08, 0.22, 0.06), (0, 0.25, 0.17), GOLD),
    ]),
    ("lid", "base", (0, 0.36, -0.17), [
        ((0.52, 0.16, 0.38), (0, 0.08, 0.19), WOOD_DARK),
        ((0.10, 0.10, 0.08), (0, 0.06, 0.31), GOLD),
    ]),
]

C = {}  # clip name -> (duration, frames)


def open_amount(t):
    """Swing open over the first 70%, then hold (one-shot hold pose)."""
    # Explicit origin: eased() treats the first step as the curve's
    # start, so this reads "rise from closed to open over the first
    # 70% of the clip, then hold".
    return eased([(0.0, 0.0), (0.7, 1.0), (1.0, 1.0)], t)


C["open"] = clip_oneshot(1.0, 60, lambda t: {
    # Past vertical (110 would stand the lid up): flat-ish back and a
    # visible interior read as "open" from the field camera.
    "lid": {"r": (-160 * DEG * open_amount(t), 0, 0)},
})


def main():
    w = Writer()
    material_index = {name: w.add_material(name, rgb) for name, rgb in MATERIAL_COLORS.items()}

    node_index = {}
    for name, parent, translation, boxes in PARTS:
        node = {"name": name, "translation": list(translation)}
        if boxes:
            meshes = [box(size, center) for size, center, _ in boxes]
            materials = [material_index[material] for _, _, material in boxes]
            node["mesh"] = w.add_mesh(name, meshes, materials)
        node_index[name] = len(w.nodes)
        w.nodes.append(node)
    for name, parent, _, _ in PARTS:
        if parent is not None:
            w.nodes[node_index[parent]].setdefault("children", []).append(node_index[name])

    animations = [
        w.add_animation(name, duration, frames, node_index)
        for name, (duration, frames) in C.items()
    ]

    gltf = {
        "asset": {"version": "2.0", "generator": "wakeful tools/generate_chest.py"},
        "scene": 0,
        "scenes": [{"name": "chest", "nodes": [node_index["character"]]}],
        "nodes": w.nodes,
        "meshes": w.meshes,
        "materials": w.materials,
        "animations": animations,
        "buffers": [{"byteLength": len(w.bin)}],
        "bufferViews": w.views,
        "accessors": w.accessors,
    }
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "wb") as f:
        f.write(w.glb(gltf))
    clips = ", ".join(sorted(C))
    print(f"wrote {os.path.normpath(OUT)} ({len(w.bin)} byte buffer, clips: {clips})")


if __name__ == "__main__":
    main()
