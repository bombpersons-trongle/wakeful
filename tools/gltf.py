"""Shared hand-rolled glTF 2.0 writer for the wakeful asset generators.

Emits GLB containers (JSON + embedded binary) with box-mesh primitives,
node-TRS animation clips, and plain color materials. Imported by the
per-asset generator scripts in this folder; run those from the repo
root: `python3 tools/generate_character.py`.
"""

import json
import math
import struct

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
        self.images = []
        self.samplers = []
        self.textures = []

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

    def add_image(self, png_bytes):
        """Embeds a PNG; returns the image index."""
        while len(self.bin) % 4:
            self.bin.append(0)
        view = {"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(png_bytes)}
        self.bin += png_bytes
        self.views.append(view)
        self.images.append({"bufferView": len(self.views) - 1, "mimeType": "image/png"})
        return len(self.images) - 1

    def add_sampler(self):
        """Bilinear filtering, repeat wrapping: the checker-arena default."""
        self.samplers.append({
            "magFilter": 9729,
            "minFilter": 9729,
            "wrapS": 10497,
            "wrapT": 10497,
        })
        return len(self.samplers) - 1

    def add_texture(self, image_index, sampler_index=0):
        self.textures.append({"sampler": sampler_index, "source": image_index})
        return len(self.textures) - 1

    def add_textured_material(self, name, texture_index, unlit=False):
        material = {
            "name": name,
            "pbrMetallicRoughness": {
                "baseColorTexture": {"index": texture_index},
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0,
            },
        }
        if unlit:
            # Skybox-style surfaces read their texture at full
            # brightness; bevy_gltf maps this to StandardMaterial::unlit.
            material["extensions"] = {"KHR_materials_unlit": {}}
        self.materials.append(material)
        return len(self.materials) - 1

    def add_mesh(self, name, primitives, material_indexes):
        """primitives: [(positions, normals, indices[, uvs])] sharing one mesh."""
        prims = []
        for primitive, material in zip(primitives, material_indexes):
            positions, normals, indices = primitive[:3]
            uvs = primitive[3] if len(primitive) > 3 else None
            pos_view = self.add_view(struct.pack(f"<{len(positions) * 3}f", *[c for p in positions for c in p]), 34962)
            pos_min = [min(p[i] for p in positions) for i in range(3)]
            pos_max = [max(p[i] for p in positions) for i in range(3)]
            pos_acc = self.add_accessor(pos_view, "VEC3", len(positions), mins=pos_min, maxs=pos_max)
            nrm_view = self.add_view(struct.pack(f"<{len(normals) * 3}f", *[c for n in normals for c in n]), 34962)
            nrm_acc = self.add_accessor(nrm_view, "VEC3", len(normals))
            idx_view = self.add_view(struct.pack(f"<{len(indices)}H", *indices), 34963)
            idx_acc = self.add_accessor(idx_view, "SCALAR", len(indices), component=5123)
            attributes = {"POSITION": pos_acc, "NORMAL": nrm_acc}
            if uvs is not None:
                uv_view = self.add_view(
                    struct.pack(f"<{len(uvs) * 2}f", *[c for uv in uvs for c in uv]),
                    34962,
                )
                attributes["TEXCOORD_0"] = self.add_accessor(uv_view, "VEC2", len(uvs))
            prims.append({
                "attributes": attributes,
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

