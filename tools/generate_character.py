#!/usr/bin/env python3
"""Generates assets/models/character.glb: the chibi placeholder character.

Blocky FF7-field-model proportions (~2.7 heads tall), rigid limbs animated
with node TRS channels — no skinning. This file is the reference for the
character rig convention; every character model should use the same joint
node names and clip set so the engine drives them identically:

    joints: hips torso head hair arm_l arm_r leg_l leg_r
    clips:  idle walk run        (looped by the engine)
            pick_up shrug wave   (one-shots)

Usage: python3 tools/generate_character.py  (from the repo root)
"""

import json
import math
import os
import struct

OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "models", "character.glb")

from gltf import box, clip_loop, clip_oneshot, eased, quat_from_euler, Writer

DEG = math.pi / 180
TAU = 2 * math.pi

# Chibi proportions (world units, Y up, authored facing +Z). ~1.15 total,
# head+hair over a third of it.
SKIN = "skin"
HAIR = "hair"
SHIRT = "shirt"
PANTS = "pants"
BOOTS = "boots"

# (node name, parent, translation, [(mesh size, mesh center, material)])

MATERIAL_COLORS = {
    SKIN: (0.98, 0.80, 0.65),
    HAIR: (0.22, 0.13, 0.08),
    SHIRT: (0.20, 0.55, 0.55),
    PANTS: (0.35, 0.30, 0.45),
    BOOTS: (0.20, 0.20, 0.25),
}

PARTS = [
    ("character", None, (0, 0, 0), []),
    ("hips", "character", (0, 0.30, 0), []),
    ("leg_l", "hips", (-0.09, 0, 0), [((0.13, 0.30, 0.15), (0, -0.15, 0), PANTS)]),
    ("leg_r", "hips", (0.09, 0, 0), [((0.13, 0.30, 0.15), (0, -0.15, 0), PANTS)]),
    ("foot_l", "leg_l", (0, 0, 0), [((0.14, 0.09, 0.21), (0, -0.255, 0.03), BOOTS)]),
    ("foot_r", "leg_r", (0, 0, 0), [((0.14, 0.09, 0.21), (0, -0.255, 0.03), BOOTS)]),
    ("torso", "hips", (0, 0, 0), [((0.28, 0.30, 0.18), (0, 0.15, 0), SHIRT)]),
    ("head", "torso", (0, 0.30, 0), [((0.34, 0.40, 0.32), (0, 0.20, 0), SKIN)]),
    ("hair", "head", (0, 0.40, 0), [
        ((0.38, 0.16, 0.36), (0, 0.06, 0), HAIR),
        ((0.12, 0.16, 0.12), (0.10, 0.10, 0.14), HAIR),   # front spike
        ((0.14, 0.18, 0.12), (-0.09, 0.08, -0.14), HAIR),  # back spike
    ]),
    ("arm_l", "torso", (-0.19, 0.28, 0), [
        ((0.10, 0.24, 0.11), (0, -0.12, 0), SHIRT),
        ((0.10, 0.10, 0.10), (0, -0.28, 0), SKIN),
    ]),
    ("arm_r", "torso", (0.19, 0.28, 0), [
        ((0.10, 0.24, 0.11), (0, -0.12, 0), SHIRT),
        ((0.10, 0.10, 0.10), (0, -0.28, 0), SKIN),
    ]),
]

C = {}  # clip name -> (duration, frames)

C["idle"] = clip_loop(2.4, 48, lambda t: {
    "torso": {"r": (2 * DEG * math.sin(TAU * t), 0, 0)},
    "head": {"r": (0, 6 * DEG * math.sin(TAU * t / 2), 0)},
    "hips": {"t": (0, 0.006 * math.sin(TAU * 2 * t), 0)},
    "arm_l": {"r": (0, 0, 2.5 * DEG * math.sin(TAU * t))},
    "arm_r": {"r": (0, 0, -2.5 * DEG * math.sin(TAU * t))},
})

C["walk"] = clip_loop(0.66, 32, lambda t: {
    "leg_l": {"r": (-24 * DEG * math.sin(TAU * t), 0, 0)},
    "leg_r": {"r": (24 * DEG * math.sin(TAU * t), 0, 0)},
    "arm_l": {"r": (16 * DEG * math.sin(TAU * t), 0, 2 * DEG)},
    "arm_r": {"r": (-16 * DEG * math.sin(TAU * t), 0, -2 * DEG)},
    "hips": {"t": (0, 0.012 * (0.5 - 0.5 * math.cos(TAU * 2 * t)), 0)},
    "torso": {"r": (0, 4 * DEG * math.sin(TAU * t), 0)},
})

C["run"] = clip_loop(0.45, 24, lambda t: {
    "leg_l": {"r": (-38 * DEG * math.sin(TAU * t), 0, 0)},
    "leg_r": {"r": (38 * DEG * math.sin(TAU * t), 0, 0)},
    "arm_l": {"r": (28 * DEG * math.sin(TAU * t), 0, 4 * DEG)},
    "arm_r": {"r": (-28 * DEG * math.sin(TAU * t), 0, -4 * DEG)},
    "hips": {
        "t": (0, 0.022 * (0.5 - 0.5 * math.cos(TAU * 2 * t)), 0),
        "r": (0, 3 * DEG * math.sin(TAU * t), 0),
    },
    "torso": {"r": (7 * DEG + 3 * DEG * math.sin(TAU * 2 * t), 0, 0)},
})

def pick_up(t):
    e = eased([(0.0, 0.0), (0.35, 1.0), (0.65, 1.0), (1.0, 0.0)], t)
    return {
        "torso": {"r": (50 * DEG * e, 0, 0)},
        "arm_l": {"r": (-60 * DEG * e, 0, 0)},
        "arm_r": {"r": (-60 * DEG * e, 0, 0)},
        "head": {"r": (-20 * DEG * e, 0, 0)},
    }
C["pick_up"] = clip_oneshot(1.1, 44, pick_up)

def shrug(t):
    e = eased([(0.0, 0.0), (0.2, 1.0), (0.7, 1.0), (1.0, 0.0)], t)
    return {
        "arm_l": {"r": (0, 0, -70 * DEG * e)},
        "arm_r": {"r": (0, 0, 70 * DEG * e)},
        "head": {"r": (0, 0, 8 * DEG * e)},
        "hips": {"t": (0, 0.012 * e, 0)},
    }
C["shrug"] = clip_oneshot(1.2, 48, shrug)

def wave(t):
    raise_e = eased([(0.0, 0.0), (0.2, 1.0), (0.8, 1.0), (1.0, 0.0)], t)
    wiggle = 12 * DEG * math.sin(TAU * 3 * t) if 0.2 < t < 0.8 else 0.0
    return {
        "arm_r": {"r": (0, 0, 135 * DEG * raise_e + wiggle)},
        "head": {"r": (0, 0, -6 * DEG * raise_e)},
        "torso": {"r": (0, -5 * DEG * raise_e, 0)},
    }
C["wave"] = clip_oneshot(1.5, 60, wave)


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
        "asset": {"version": "2.0", "generator": "wakeful tools/generate_character.py"},
        "scene": 0,
        "scenes": [{"name": "character", "nodes": [node_index["character"]]}],
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
