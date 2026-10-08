#!/usr/bin/env python3
"""Generates the chibi placeholder character models in assets/models/:
character.glb (the hero) and goblin.glb (a green palette variant).

Blocky FF7-field-model proportions (~2.7 heads tall), rigid limbs animated
with node TRS channels — no skinning. This file is the reference for the
character rig convention; every character model should use the same joint
node names and clip set so the engine drives them identically:

    joints: hips torso head hair arm_l arm_r leg_l leg_r
    clips:  idle walk run        (looped by the engine)
            pick_up shrug wave attack   (one-shots)
            die                (one-shot; sinks the body under the floor)

Usage: python3 tools/generate_character.py  (from the repo root)
"""

import json
import math
import os
import struct

OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "models")

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

# (material name -> rgb) per model: same rig, different paint.
CHARACTER_PALETTE = {
    SKIN: (0.98, 0.80, 0.65),
    HAIR: (0.22, 0.13, 0.08),
    SHIRT: (0.20, 0.55, 0.55),
    PANTS: (0.35, 0.30, 0.45),
    BOOTS: (0.20, 0.20, 0.25),
}
GOBLIN_PALETTE = {
    SKIN: (0.38, 0.58, 0.26),
    HAIR: (0.24, 0.34, 0.16),
    SHIRT: (0.45, 0.38, 0.26),
    PANTS: (0.28, 0.26, 0.22),
    BOOTS: (0.16, 0.15, 0.12),
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

# One envelope per euler axis. A big overhead slash: arm coils up
# behind the head, sweeps down through the front, settles. Composing
# multiple envelopes on one axis fights itself; one chain reads clean.
def attack(t):
    return {
        # coil up overhead, then a full downward arc
        "arm_r": {"r": (
            eased([(0.0, 0.0), (0.3, -150), (0.4, -150), (0.65, 80), (0.9, 0.0)], t) * DEG,
            0,
            -10 * DEG * math.sin(math.pi * min(t / 0.9, 1.0)),
        )},
        "torso": {"r": (
            eased([(0.0, 0.0), (0.3, -12), (0.4, -12), (0.65, 18), (0.9, 0.0)], t) * DEG,
            eased([(0.0, 0.0), (0.3, -35), (0.65, 40), (0.9, 0.0)], t) * DEG,
            0,
        )},
        "arm_l": {"r": (eased([(0.0, 0.0), (0.35, -40), (0.7, 0.0)], t) * DEG, 0, 0)},
        "head": {"r": (eased([(0.0, 0.0), (0.3, -18), (0.65, 8), (0.9, 0.0)], t) * DEG, 0, 0)},
        "hips": {"t": (0, eased([(0.0, 0.0), (0.3, 0.02), (0.65, -0.02), (0.9, 0.0)], t), 0)},
    }
C["attack"] = clip_oneshot(0.9, 54, attack)

# Casting: both arms rise, palms out, and the body leans back into it
# — the held pose a spell resolves out of. Arms only (no hips), so the
# engine's locomotion keeps control of the body while the cast plays.
def cast(t):
    raise_e = eased([(0.0, 0.0), (0.35, 1.0), (0.75, 1.0), (1.0, 0.0)], t)
    return {
        "arm_r": {"r": (0, -110 * DEG * raise_e, -25 * DEG * raise_e)},
        "arm_l": {"r": (0, 110 * DEG * raise_e, 25 * DEG * raise_e)},
        "torso": {"r": (eased([(0.0, 0.0), (0.35, -14), (0.75, -14), (1.0, 0.0)], t) * DEG, 0, 0)},
        "head": {"r": (eased([(0.0, 0.0), (0.35, 12), (0.75, 12), (1.0, 0.0)], t) * DEG, 0, 0)},
    }
C["cast"] = clip_oneshot(1.2, 40, cast)

# Death reads as sinking into the ground: the hips translate down
# until the whole body is below the floor, and the clip HOLDS there.
# Two constraints shaped this:
#   - the glTF root belongs to the engine's transform sync, so the
#     clip drives the hips joint, never the root;
#   - a bevy animation only rewrites the properties its curves target,
#     so the die must NOT use scale — idle and the rest animate hips
#     translation, which resurrects the body when they resume. A
#     scale-to-zero death would leave a stale scale-0 forever after.
# (glTF cannot animate material opacity, so a true fade is out.)
def die(t):
    sink = eased([(0.0, 0.0), (0.12, 0.03), (0.25, -0.08), (0.8, -1.55)], t)
    return {
        "hips": {"t": (0, sink, 0)},
    }
C["die"] = clip_oneshot(1.4, 56, die)


def build(filename, name, palette):
    w = Writer()
    material_index = {material: w.add_material(material, rgb) for material, rgb in palette.items()}

    node_index = {}
    for joint, parent, translation, boxes in PARTS:
        node = {"name": joint, "translation": list(translation)}
        if boxes:
            meshes = [box(size, center) for size, center, _ in boxes]
            materials = [material_index[material] for _, _, material in boxes]
            node["mesh"] = w.add_mesh(joint, meshes, materials)
        node_index[joint] = len(w.nodes)
        w.nodes.append(node)
    for joint, parent, _, _ in PARTS:
        if parent is not None:
            w.nodes[node_index[parent]].setdefault("children", []).append(node_index[joint])

    animations = [
        w.add_animation(clip, duration, frames, node_index)
        for clip, (duration, frames) in C.items()
    ]

    gltf = {
        "asset": {"version": "2.0", "generator": "wakeful tools/generate_character.py"},
        "scene": 0,
        "scenes": [{"name": name, "nodes": [node_index["character"]]}],
        "nodes": w.nodes,
        "meshes": w.meshes,
        "materials": w.materials,
        "animations": animations,
        "buffers": [{"byteLength": len(w.bin)}],
        "bufferViews": w.views,
        "accessors": w.accessors,
    }
    path = os.path.join(OUT, filename)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(w.glb(gltf))
    clips = ", ".join(sorted(C))
    print(f"wrote {os.path.normpath(path)} ({len(w.bin)} byte buffer, clips: {clips})")


def main():
    build("character.glb", "character", CHARACTER_PALETTE)
    build("goblin.glb", "goblin", GOBLIN_PALETTE)


if __name__ == "__main__":
    main()
