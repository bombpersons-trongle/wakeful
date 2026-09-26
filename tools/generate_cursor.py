#!/usr/bin/env python3
"""Generates assets/ui/finger.png: the chunky menu cursor hand.

A pointing hand in the FF7 spirit — index finger aimed right at the
menu text, curled fingers below. Authored as an explicit pixel map so
tweaking the silhouette is a one-array edit; nearest-neighbor scaled to
stay chunky next to the 8px pixel font.

Run from the repo root: python3 tools/generate_cursor.py
"""

import struct
import zlib

# . transparent, o outline, # fill, + shade
PIXELS = [
    "...oo.......",
    "..o##o......",
    ".o####oooo..",
    ".o########o.",
    "o##########o",
    "o##########o",
    "o##########o",
    ".o###o##oo..",
    "..o##o.o#o..",
    "...oo.......",
]

# Nearest-neighbor scale: each pixel becomes SCALE x SCALE in the PNG.
SCALE = 2

PALETTE = {
    "o": (26, 24, 30, 255),
    "#": (238, 236, 226, 255),
    "+": (208, 204, 190, 255),
    ".": (0, 0, 0, 0),
}


def render() -> tuple[int, int, bytes]:
    rows = []
    for line in PIXELS:
        row = b"".join(bytes(PALETTE[ch]) * SCALE for ch in line)
        rows.extend([b"\x00" + row] * SCALE)  # filter 0 per scanline
    width = len(PIXELS[0]) * SCALE
    return width, len(rows), b"".join(rows)


def write_png(path: str, width: int, height: int, raw: bytes) -> None:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data))
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", ihdr))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


def main() -> None:
    width, height, raw = render()
    write_png("assets/ui/finger.png", width, height, raw)
    print(f"assets/ui/finger.png: {width}x{height}")


if __name__ == "__main__":
    main()
