#!/usr/bin/env python3
"""Generate the castle skin's stone tile: assets/skins/castle-stone.png.

Pixel art, not a photo: rows of uneven stone blocks in a few close greys,
a dark mortar line, ordered (Bayer 4x4) dithering for shading, and a few
moss clusters in two greens. Seamless on every edge by construction: each
row's block widths sum to the tile width, the rows divide the tile height,
and every position is taken mod the tile size.

Usage: python3 scripts/gen-castle-tile.py [--preview out.png]
Needs Pillow and numpy. Output is deterministic (fixed seed).
"""

import argparse
import os

import numpy as np
from PIL import Image

SIZE = 96
ROW_H = 12
WIDTHS = [16, 20, 24, 28, 32]
SEED = 11

# Stone ramp, darkest to lightest: castle grey, kept in a narrow band so
# light text stays readable on top of it.
RAMP = [0x262628, 0x2B2B2D, 0x303032, 0x353537, 0x3A3A3C, 0x404042]
MORTAR = 0x1C1C1E
MOSS = [0x34402A, 0x4A5A31]

BAYER4 = (
    np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]])
    + 0.5
) / 16.0


def rgb(v):
    return ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)


def row_joints(rng):
    """Block widths for one row, summing to SIZE, rotated by a random shift."""
    while True:
        ws, total = [], 0
        while total < SIZE:
            w = int(rng.choice(WIDTHS))
            ws.append(w)
            total += w
        if total == SIZE:
            break
    shift = int(rng.integers(0, SIZE // 4)) * 4
    starts, x = [], 0
    for w in ws:
        starts.append((x + shift) % SIZE)
        x += w
    return sorted(starts)


def build():
    assert SIZE % ROW_H == 0 and SIZE % 4 == 0
    rng = np.random.default_rng(SEED)
    rows = SIZE // ROW_H
    joints = [row_joints(rng) for _ in range(rows)]
    out = np.zeros((SIZE, SIZE), dtype=np.int32)  # ramp index, -1 mortar
    joint_pts = []

    for r in range(rows):
        starts = joints[r]
        n = len(starts)
        tone = rng.uniform(0.8, 3.6, size=n)
        tilt = rng.uniform(-1.4, 1.4, size=n)
        for k in range(n):
            x0 = starts[k]
            w = (starts[(k + 1) % n] - x0) % SIZE or SIZE
            joint_pts.append((x0, r * ROW_H + ROW_H - 1))
            for ly in range(ROW_H):
                y = r * ROW_H + ly
                for lx in range(w):
                    x = (x0 + lx) % SIZE
                    if ly == ROW_H - 1 or lx == 0:
                        out[y, x] = -1
                        continue
                    # Light from the top left: a soft diagonal across each
                    # block, a lit top edge and a shadowed lower edge.
                    v = tone[k] + tilt[k] * ((lx / w) - (ly / ROW_H))
                    if ly == 0:
                        v += 1.2
                    elif ly == ROW_H - 2:
                        v -= 1.2
                    if lx == 1:
                        v += 0.7
                    elif lx == w - 1:
                        v -= 0.7
                    v += rng.normal(0.0, 0.35)
                    idx = int(np.floor(v + BAYER4[y % 4, x % 4] - 0.5))
                    out[y, x] = max(0, min(len(RAMP) - 1, idx))

    img = np.zeros((SIZE, SIZE, 3), dtype=np.uint8)
    for y in range(SIZE):
        for x in range(SIZE):
            i = out[y, x]
            img[y, x] = rgb(MORTAR if i < 0 else RAMP[i])

    # Moss: a few dithered clusters seeded where mortar lines meet, creeping
    # onto the stone. Coordinates wrap, so a cluster may cross an edge.
    picks = rng.choice(len(joint_pts), size=5, replace=False)
    for p in picks:
        cx, cy = joint_pts[p]
        rad = rng.uniform(3.5, 6.0)
        for dy in range(-7, 8):
            for dx in range(-8, 9):
                d = np.hypot(dx * 0.8, dy * 1.4)
                if d > rad:
                    continue
                x, y = (cx + dx) % SIZE, (cy + dy) % SIZE
                density = 1.0 - d / rad
                if density + rng.uniform(-0.25, 0.25) < BAYER4[y % 4, x % 4]:
                    continue
                dark = density < 0.45 or out[y, x] < 0
                img[y, x] = rgb(MOSS[0] if dark else MOSS[1])

    return Image.fromarray(img, "RGB")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--preview", help="also write a 4x nearest-neighbor 3x3 preview here")
    args = ap.parse_args()
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    path = os.path.join(root, "assets", "skins", "castle-stone.png")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tile = build()
    tile.save(path, optimize=True)
    print(f"wrote {path} ({SIZE}x{SIZE})")
    if args.preview:
        grid = Image.new("RGB", (SIZE * 3, SIZE * 3))
        for gy in range(3):
            for gx in range(3):
                grid.paste(tile, (gx * SIZE, gy * SIZE))
        grid = grid.resize((SIZE * 12, SIZE * 12), Image.NEAREST)
        grid.save(args.preview)
        print(f"wrote {args.preview}")


if __name__ == "__main__":
    main()
