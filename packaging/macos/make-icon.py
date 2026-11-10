#!/usr/bin/env python3
"""Draws icon.png, the 1024x1024 application icon: the Spooky ghost on a dark
rounded square, in the application's own palette.

Standard library only. The PNG is committed; run this to regenerate it:

    python3 packaging/macos/make-icon.py packaging/macos/icon.png
"""

import math
import struct
import sys
import zlib

SIZE = 1024
SAMPLES = 2  # per axis, for antialiasing
BACKGROUND_TOP = (36, 40, 50)
BACKGROUND_BOTTOM = (22, 25, 31)
GHOST = (207, 211, 220)
EYE = (34, 37, 46)


def inside_rounded_square(x, y, inset=100, radius=190):
    """Whether (x, y) is inside the icon tile, a rounded square."""
    lo, hi = inset, SIZE - inset
    cx = min(max(x, lo + radius), hi - radius)
    cy = min(max(y, lo + radius), hi - radius)
    return lo <= x <= hi and lo <= y <= hi and math.hypot(x - cx, y - cy) <= radius


def inside_ghost(x, y):
    """Whether (x, y) is inside the ghost: a dome, straight sides and a hem of
    three round scallops."""
    left, right = 296, 728
    width = right - left
    radius = width / 2
    centre_x, head_y, base_y = (left + right) / 2, 430, 720
    if y < head_y:
        return math.hypot(x - centre_x, y - head_y) <= radius
    if y <= base_y:
        return left <= x <= right
    scallop = width / 6
    return any(
        math.hypot(x - (left + (2 * k + 1) * scallop), y - base_y) <= scallop
        for k in range(3)
    )


def inside_eye(x, y):
    return any(math.hypot(x - ex, y - 462) <= 34 for ex in (444, 580))


def colour_at(x, y):
    """RGB of one sample point, or None outside the tile."""
    if not inside_rounded_square(x, y):
        return None
    if inside_ghost(x, y):
        return EYE if inside_eye(x, y) else GHOST
    t = (y - 100) / (SIZE - 200)
    return tuple(round(a + (b - a) * t) for a, b in zip(BACKGROUND_TOP, BACKGROUND_BOTTOM))


def pixel(px, py):
    total = [0, 0, 0]
    covered = 0
    for sy in range(SAMPLES):
        for sx in range(SAMPLES):
            colour = colour_at(px + (sx + 0.5) / SAMPLES, py + (sy + 0.5) / SAMPLES)
            if colour is not None:
                covered += 1
                for i in range(3):
                    total[i] += colour[i]
    if covered == 0:
        return (0, 0, 0, 0)
    alpha = round(255 * covered / (SAMPLES * SAMPLES))
    return (*(round(c / covered) for c in total), alpha)


def png(width, height, rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(b"\x00" + bytes(row) for row in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    rows = []
    for py in range(SIZE):
        row = []
        for px in range(SIZE):
            row.extend(pixel(px, py))
        rows.append(row)
    with open(sys.argv[1], "wb") as out:
        out.write(png(SIZE, SIZE, rows))


if __name__ == "__main__":
    main()
