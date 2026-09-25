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

# ---------------------------------------------------------------- geometry

def box(size, center):
    """A box mesh's vertices/normals/indices, centered at `center`."""
    sx, sy, sz = size[0] / 2, size[1] / 2, size[2] / 2
    cx, cy, cz = center
    faces = [
        # normal, 4 corners (CCW from outside)
        ((0, 0, 1), [(cx - sx, cy - sy, cz + sz), (cx + sx, cy - sy, cz + sz), (cx + sx, cy + sy, cz + sz), (cx - sx, cy + sy, cz + sz)]),
        ((0, 0, -1), [(cx + sx, cy - sy, cz - sz), (cx - sx, cy - sy, cz - sz), (cx - sx, cy + sy, cz - sz), (cx + sx, cy + sy, cz - sz)]),
        ((1, 0, 0), [(cx + sx, cy - sy, cz + sz), (cx + sx, cy - sy, cz - sz), (cx + sx, cy + sy, cz - sz), (cx + sx, cy + sy, cz + sz)]),
        ((-1, 0, 0), [(cx - sx, cy - sy, cz - sz), (cx - sx, cy - sy, cz + sz), (cx - sx, cy + sy, cz + sz), (cx - sx, cy + sy, cz - sz)]),
        ((0, 1, 0), [(cx - sx, cy + sy, cz + sz), (cx + sx, cy + sy, cz + sz), (cx + sx, cy + sy, cz - sz), (cx - sx, cy + sy, cz - sz)]),
        ((0, -1, 0), [(cx - sx, cy - sy, cz - sz), (cx + sx, cy - sy, cz - sz), (cx + sx, cy - sy, cz + sz), (cx - sx, cy - sy, cz + sz)]),
    ]
    positions, normals, indices = [], [], []
    for normal, corners in faces:
        base = len(positions)
        positions += corners
        normals += [normal] * 4
        indices += [base, base + 1, base + 2, base, base + 2, base + 3]
    return positions, normals, indices

# Chibi proportions (world units, Y up, authored facing +Z). ~1.15 total,
# head+hair over a third of it.
SKIN = "skin"
HAIR = "hair"
SHIRT = "shirt"
PANTS = "pants"
BOOTS = "boots"

# (node name, parent, translation, [(mesh size, mesh center, material)])
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

MATERIAL_COLORS = {
    SKIN: (0.98, 0.80, 0.65),
    HAIR: (0.22, 0.13, 0.08),
    SHIRT: (0.20, 0.55, 0.55),
    PANTS: (0.35, 0.30, 0.45),
    BOOTS: (0.20, 0.20, 0.25),
}

# --------------------------------------------------------------- animation

def quat_from_euler(rx, ry, rz):
    """XYZ euler (radians) to unit quaternion, applied X then Y then Z."""
    cx, sx = math.cos(rx / 2), math.sin(rx / 2)
    cy, sy = math.cos(ry / 2), math.sin(ry / 2)
    cz, sz = math.cos(rz / 2), math.sin(rz / 2)
    return [
        sx * cy * cz - cx * sy * sz,
        cx * sy * cz + sx * cy * sz,
        cx * cy * sz - sx * sy * cz,
        cx * cy * cz + sx * sy * sz,
    ]

DEG = math.pi / 180
TAU = 2 * math.pi

def eased(steps, t):
    """Piecewise-smooth one-shot envelope: steps = [(t_end, value), ...]."""
    (t0, v0) = steps[0]
    if t <= t0:
        return v0
    for t1, v1 in steps[1:]:
        if t <= t1:
            u = (t - t0) / (t1 - t0)
            u = u * u * (3 - 2 * u)  # smoothstep between keyframes
            return v0 + (v1 - v0) * u
        (t0, v0) = (t1, v1)
    return v0

def clip_loop(duration, samples, channels):
    """channels: node -> fn(t 0..1) -> {'r': (rx,ry,rz) or None, 't': (x,y,z) or None}."""
    frames = []
    for i in range(samples):
        t = i / samples
        frames.append((duration * t, t, channels(t)))
    return duration, frames

def clip_oneshot(duration, samples, channels):
    frames = []
    for i in range(samples + 1):
        t = i / samples
        frames.append((duration * t, t, channels(t)))
    return duration, frames

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

# ------------------------------------------------------------- glTF output

class Writer:
    """Accumulates binary blobs and glTF structures, then emits a GLB."""

    def __init__(self):
        self.bin = bytearray()
        self.views = []   # bufferViews
        self.accessors = []
        self.meshes = []
        self.materials = []
        self.nodes = []

    def add_view(self, data, target=None):
        while len(self.bin) % 4:
            self.bin.append(0)
        view = {"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(data)}
        if target:
            view["target"] = target
        self.bin += data
        self.views.append(view)
        return len(self.views) - 1

    def add_accessor(self, view, kind, count, component=5126, mins=None, maxs=None):
        acc = {
            "bufferView": view,
            "componentType": component,
            "count": count,
            "type": kind,
        }
        if mins is not None:
            acc["min"], acc["max"] = mins, maxs
        self.accessors.append(acc)
        return len(self.accessors) - 1

    def add_mesh(self, name, primitives, material_indexes):
        """primitives: [(positions, normals, indices)] sharing one mesh."""
        prims = []
        for primitive, material in zip(primitives, material_indexes):
            positions, normals, indices = primitive
            pos_view = self.add_view(struct.pack(f"<{len(positions) * 3}f", *[c for p in positions for c in p]), 34962)
            pos_min = [min(p[i] for p in positions) for i in range(3)]
            pos_max = [max(p[i] for p in positions) for i in range(3)]
            pos_acc = self.add_accessor(pos_view, "VEC3", len(positions), mins=pos_min, maxs=pos_max)
            nrm_view = self.add_view(struct.pack(f"<{len(normals) * 3}f", *[c for n in normals for c in n]), 34962)
            nrm_acc = self.add_accessor(nrm_view, "VEC3", len(normals))
            idx_view = self.add_view(struct.pack(f"<{len(indices)}H", *indices), 34963)
            idx_acc = self.add_accessor(idx_view, "SCALAR", len(indices), component=5123)
            prims.append({
                "attributes": {"POSITION": pos_acc, "NORMAL": nrm_acc},
                "indices": idx_acc,
                "material": material,
            })
        self.meshes.append({"name": name, "primitives": prims})
        return len(self.meshes) - 1

    def add_material(self, name, rgb):
        self.materials.append({
            "name": name,
            "pbrMetallicRoughness": {
                "baseColorFactor": [*rgb, 1.0],
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0,
            },
        })
        return len(self.materials) - 1

    def add_animation(self, name, duration, frames, node_index):
        """frames: [(time, t01, {node: {'r': (rx,ry,rz) or 't': (x,y,z)}})]."""
        samplers = []
        channels = []
        kinds = []
        for _, _, parts in frames:
            for node, spec in parts.items():
                if spec.get("r") is not None and ("r", node) not in kinds:
                    kinds.append(("r", node))
                if spec.get("t") is not None and ("t", node) not in kinds:
                    kinds.append(("t", node))
        for kind, node in kinds:
            times = [f[0] for f in frames]
            t_view = self.add_view(struct.pack(f"<{len(times)}f", *times))
            t_acc = self.add_accessor(t_view, "SCALAR", len(times), mins=[0.0], maxs=[duration])
            if kind == "r":
                values = [quat_from_euler(*f[2][node]["r"]) for f in frames]
                out = struct.pack(f"<{len(values) * 4}f", *[c for q in values for c in q])
                out_view = self.add_view(out)
                out_acc = self.add_accessor(out_view, "VEC4", len(values))
                path = "rotation"
            else:
                values = [f[2][node]["t"] for f in frames]
                out = struct.pack(f"<{len(values) * 3}f", *[c for v in values for c in v])
                out_view = self.add_view(out)
                out_acc = self.add_accessor(out_view, "VEC3", len(values))
                path = "translation"
            samplers.append({"input": t_acc, "output": out_acc, "interpolation": "LINEAR"})
            channels.append({"sampler": len(samplers) - 1, "target": {"node": node_index[node], "path": path}})
        return {"name": name, "channels": channels, "samplers": samplers}

    def glb(self, gltf_json):
        json_bytes = json.dumps(gltf_json, separators=(",", ":")).encode()
        while len(json_bytes) % 4:
            json_bytes += b" "
        while len(self.bin) % 4:
            self.bin.append(0)
        total = 12 + 8 + len(json_bytes) + (8 + len(self.bin) if self.bin else 0)
        out = struct.pack("<III", 0x46546C67, 2, total)
        out += struct.pack("<II", len(json_bytes), 0x4E4F534A) + json_bytes
        if self.bin:
            out += struct.pack("<II", len(self.bin), 0x004E4942) + self.bin
        return out


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
