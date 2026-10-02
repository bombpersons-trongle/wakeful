#!/usr/bin/env python3
"""Generates a synthetic depth map for the devroom's background.

This is the placeholder for the real Blender exporter (phase 2) — it
encodes the same convention: one channel per background pixel, the ray
distance from the scene camera normalized over the scene's depth_range.

The devroom camera: (0, 6, 9) looking at the origin, fov 45 vertical.
The map encodes the floor plane at y=0 (the walkable plane characters
stand on) plus a floating foreground slab at 10m along the rays,
covering the lower-center of the frame — the test occluder the player
walks behind and in front of.
"""

import math
import struct
import zlib

W, H = 320, 240
DEPTH_RANGE = 32.0

CAM = (0.0, 6.0, 9.0)
TARGET = (0.0, 0.0, 0.0)
FOV = 45.0
FLOOR_Y = 0.0
BEHIND = 20.0  # past the visible floor: just far

# A world-space slab (an AABB) partitioning the walkway: the player
# behind it is occluded, their head pokes over the top.
SLAB = ((-0.9, 0.9), (0.0, 1.3), (-1.0, -0.6))


def cross(a, b):
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def norm(v):
    n = math.sqrt(sum(c * c for c in v))
    return [c / n for c in v]


forward = norm([t - c for t, c in zip(TARGET, CAM)])
right = norm(cross(forward, (0.0, 1.0, 0.0)))
up_cam = cross(right, forward)

tan_half = math.tan(math.radians(FOV) * 0.5)
aspect = W / H

rows = []
for y in range(H):
    ndc_y = 1.0 - 2.0 * ((y + 0.5) / H)
    row = bytearray(W)
    for x in range(W):
        ndc_x = ((x + 0.5) / W) * 2.0 - 1.0
        ray = norm([
            forward[i] + right[i] * (ndc_x * tan_half * aspect) + up_cam[i] * (ndc_y * tan_half)
            for i in range(3)
        ])
        dist = BEHIND
        if ray[1] < -1e-4:  # the floor plane at y=0
            d = (FLOOR_Y - CAM[1]) / ray[1]
            if 0.0 < d < BEHIND:
                dist = d
        # Ray-AABB of the slab: the entry distance if the ray enters the
        # box ahead of the camera.
        (xa, xb), (ya, yb), (za, zb) = SLAB
        # Ray-AABB slab method: the entry/exit planes; a hit needs the
        # entry at or before the exit and ahead of the camera.
        (xa, xb), (ya, yb), (za, zb) = SLAB
        din = max(
            (xa - CAM[0]) / ray[0] if abs(ray[0]) > 1e-6 else -1e9,
            (ya - CAM[1]) / ray[1] if abs(ray[1]) > 1e-6 else -1e9,
            (za - CAM[2]) / ray[2] if abs(ray[2]) > 1e-6 else -1e9,
        )
        dout = min(
            (xb - CAM[0]) / ray[0] if abs(ray[0]) > 1e-6 else 1e9,
            (yb - CAM[1]) / ray[1] if abs(ray[1]) > 1e-6 else 1e9,
            (zb - CAM[2]) / ray[2] if abs(ray[2]) > 1e-6 else 1e9,
        )
        if 0.0 < din <= dout and din < dist:
            dist = din
        row[x] = min(255, round(dist / DEPTH_RANGE * 255))
    rows.append(bytes(row))

raw = b"".join(b"\x00" + row for row in rows)


def chunk(tag, payload):
    return (
        struct.pack(">I", len(payload))
        + tag
        + payload
        + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
    )


png = (
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 0, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(raw))
    + chunk(b"IEND", b"")
)

with open("assets/backgrounds/devroom_depth.png", "wb") as fh:
    fh.write(png)
print(f"wrote assets/backgrounds/devroom_depth.png ({W}x{H}, range {DEPTH_RANGE}m)")
