#!/usr/bin/env python3
"""Generate the castle skin's stone tile: assets/skins/castle-stone.png.

Pixel art in a limited palette (17 colors), after Ian McQue's grimy painted
surfaces: rows of uneven, worn stone blocks in warm grey-browns, chipped
corners, cracks, damp stains and rust streaks running down from the block
joints, moss creeping out of the corners and mortar, heavy ordered (Bayer
8x8) dithered shadow and fine grain.

Seamless on every edge by construction: row heights and block widths sum to
the tile size, the noise is periodic, and every stamp (crack, streak, moss)
writes its pixels mod the tile size.

Usage: python3 scripts/gen-castle-tile.py [--preview out.png]
Needs Pillow and numpy. Output is deterministic (fixed seed).
"""

import argparse
import os

import numpy as np
from PIL import Image

SIZE = 256
SEED = 7

# Palette, darkest first within each group. Kept dark and close so light
# text on top stays readable.
SHADOW = 0x0E0D0C
MORTAR = 0x181614
STONE = [0x221F1C, 0x292623, 0x302C28, 0x38332E, 0x413B35, 0x4B443C]
EDGE = 0x5A5146
RUST = [0x35211A, 0x52301F, 0x6E4223]
MOSS = [0x232A17, 0x323C1E, 0x47532A]
DAMP = [0x1A1E20, 0x252B2C]

BAYER8 = np.array(
    [
        [0, 32, 8, 40, 2, 34, 10, 42],
        [48, 16, 56, 24, 50, 18, 58, 26],
        [12, 44, 4, 36, 14, 46, 6, 38],
        [60, 28, 52, 20, 62, 30, 54, 22],
        [3, 35, 11, 43, 1, 33, 9, 41],
        [51, 19, 59, 27, 49, 17, 57, 25],
        [15, 47, 7, 39, 13, 45, 5, 37],
        [63, 31, 55, 23, 61, 29, 53, 21],
    ]
)
THRESH = (np.tile(BAYER8, (SIZE // 8, SIZE // 8)) + 0.5) / 64.0


def rgb(v):
    return ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)


def periodic_noise(rng, cells):
    """Value noise with `cells` lattice cells across the tile; wraps."""
    g = rng.random((cells, cells))
    step = SIZE / cells
    ys, xs = np.mgrid[0:SIZE, 0:SIZE].astype(float)
    fx, fy = xs / step, ys / step
    x0, y0 = np.floor(fx).astype(int), np.floor(fy).astype(int)
    tx, ty = fx - x0, fy - y0
    tx, ty = tx * tx * (3 - 2 * tx), ty * ty * (3 - 2 * ty)
    x0, y0 = x0 % cells, y0 % cells
    x1, y1 = (x0 + 1) % cells, (y0 + 1) % cells
    a = g[y0, x0] * (1 - tx) + g[y0, x1] * tx
    b = g[y1, x0] * (1 - tx) + g[y1, x1] * tx
    return a * (1 - ty) + b * ty


def fbm(rng, octaves):
    out = np.zeros((SIZE, SIZE))
    total = 0.0
    for cells, amp in octaves:
        out += periodic_noise(rng, cells) * amp
        total += amp
    return out / total


def split(rng, total, lo, hi):
    """Random sizes in [lo, hi] summing exactly to `total`."""
    while True:
        parts, s = [], 0
        while s < total:
            p = int(rng.integers(lo, hi + 1))
            parts.append(p)
            s += p
        over = s - total
        if over == 0:
            return parts
        if parts[-1] - over >= lo:
            parts[-1] -= over
            return parts


def build():
    rng = np.random.default_rng(SEED)
    heights = split(rng, SIZE, 15, 30)

    block = np.full((SIZE, SIZE), -1, dtype=np.int32)  # -1 is mortar
    ly_map = np.zeros((SIZE, SIZE))  # 0 at a block's top, 1 at its bottom
    lx_map = np.zeros((SIZE, SIZE))
    blocks = []  # (x0, y0, w, h)
    joints = []  # where a vertical joint meets the bed joint below it
    y = 0
    for h in heights:
        widths = split(rng, SIZE, 18, 60)
        x = int(rng.integers(0, SIZE))
        for w in widths:
            # Now and then a block splits into two short stacked stones.
            stack = h >= 24 and w <= 40 and rng.random() < 0.3
            parts = [(y, h // 2), (y + h // 2, h - h // 2)] if stack else [(y, h)]
            for by, bh in parts:
                bid = len(blocks)
                blocks.append((x % SIZE, by, w, bh))
                chip = rng.uniform(0.0, 2.6, size=4) * (rng.random(4) < 0.6)  # worn corners
                for dy in range(bh - 1):
                    for dx in range(1, w):
                        ax, bx = dx - 1, w - 1 - dx
                        ay, by_ = dy, bh - 2 - dy
                        if (
                            np.hypot(ax, ay) < chip[0]
                            or np.hypot(bx, ay) < chip[1]
                            or np.hypot(ax, by_) < chip[2]
                            or np.hypot(bx, by_) < chip[3]
                        ):
                            continue
                        px, py = (x + dx) % SIZE, (by + dy) % SIZE
                        block[py, px] = bid
                        ly_map[py, px] = dy / max(1, bh - 2)
                        lx_map[py, px] = dx / max(1, w - 1)
            joints.append((x % SIZE, (y + h - 1) % SIZE))
            x += w
        y += h

    n = len(blocks)
    tone = rng.uniform(0.6, 3.9, size=n)
    tilt = rng.uniform(-0.8, 0.8, size=n)
    mortar = block < 0
    bid = np.where(mortar, 0, block)

    # Stone value on the STONE ramp (0..5), lit from the top left.
    grime = fbm(rng, [(4, 1.0), (8, 0.6), (16, 0.4), (32, 0.25)])
    pools = periodic_noise(rng, 2)  # big shadow pools across the wall
    v = tone[bid] + tilt[bid] * (lx_map - ly_map)
    v += 0.9 * (1 - ly_map) - 1.8 * np.clip((ly_map - 0.55) / 0.45, 0, 1) ** 1.5
    v += (grime - 0.5) * 3.2
    v -= np.clip(0.55 - pools, 0, None) * 3.5
    v += rng.normal(0.0, 0.45, size=(SIZE, SIZE))  # fine grain

    # Cracks: short random walks inside some blocks, a dark line with a
    # lighter lip below it.
    crack = np.zeros((SIZE, SIZE), dtype=bool)
    lip = np.zeros((SIZE, SIZE), dtype=bool)
    for i in rng.choice(n, size=n // 3, replace=False):
        x0, y0, w, h = blocks[i]
        cx = x0 + int(rng.integers(2, max(3, w - 2)))
        cy = y0 + int(rng.integers(0, max(1, h // 3)))
        dx = rng.choice([-1, 1])
        for _ in range(int(rng.integers(6, 22))):
            px, py = cx % SIZE, cy % SIZE
            if block[py, px] != i:
                break
            crack[py, px] = True
            lip[(py + 1) % SIZE, px] = True
            if rng.random() < 0.6:
                cy += 1
            if rng.random() < 0.55:
                cx += dx
            if rng.random() < 0.15:
                dx = -dx
    lip &= ~crack & ~mortar

    # Streaks running down from joints: damp everywhere, rust from a few
    # iron pins. Density fades with distance; edges wander with noise.
    wander = periodic_noise(rng, 16)
    damp = np.zeros((SIZE, SIZE))
    rust = np.zeros((SIZE, SIZE))
    pins = []
    order = rng.permutation(len(joints))
    for k, j in enumerate(order[: int(len(joints) * 0.55)]):
        jx, jy = joints[j]
        is_rust = k % 4 == 0
        length = int(rng.integers(25, 70 if is_rust else 80))
        width = rng.uniform(1.8, 3.0) if is_rust else rng.uniform(2.0, 4.5)
        target = rust if is_rust else damp
        if is_rust:
            pins.append((jx, jy))
        for t in range(length):
            py = (jy + 1 + t) % SIZE
            fade = (1 - t / length) ** 0.8
            sway = (wander[py, jx % SIZE] - 0.5) * 4
            wdt = width * (0.6 + 0.6 * fade)
            for dx in range(-5, 6):
                d = abs(dx - sway)
                if d > wdt:
                    continue
                px = (jx + dx) % SIZE
                target[py, px] = max(target[py, px], fade * (1 - d / (wdt + 0.5)))

    # Moss: blobs at some joints, spreading along mortar and the bottoms of
    # blocks, broken up by noise.
    moss = np.zeros((SIZE, SIZE))
    ys, xs = np.mgrid[0:SIZE, 0:SIZE]
    ragged = periodic_noise(rng, 32)
    for j in rng.choice(len(joints), size=10, replace=False):
        sx, sy = joints[j]
        r = rng.uniform(6, 16)
        ddx = (xs - sx + SIZE // 2) % SIZE - SIZE // 2
        ddy = (ys - sy + SIZE // 2) % SIZE - SIZE // 2
        d = np.hypot(ddx * 0.5, ddy * 1.1) / r + (ragged - 0.5) * 0.9
        moss = np.maximum(moss, np.clip(1 - d, 0, 1))
    tuft = periodic_noise(rng, 64)
    moss = moss * (0.4 + 1.2 * tuft) * np.where(mortar, 1.5, 1.0)
    moss *= moss > 0.15
    v -= damp * 2.2 + rust * 1.0 + moss * 0.8
    idx = np.clip(np.floor(v + THRESH - 0.5), 0, len(STONE) - 1).astype(int)

    # Compose.
    img = np.zeros((SIZE, SIZE, 3), dtype=np.uint8)
    img[:] = np.array([rgb(c) for c in STONE], dtype=np.uint8)[idx]
    # Worn top edges catch the light on the brighter blocks.
    img[(ly_map == 0) & ~mortar & (idx >= 3) & (THRESH < 0.6)] = rgb(EDGE)
    img[lip & (idx >= 2)] = rgb(STONE[4])
    # Mortar: dark, darker still in the shadow pools and here and there.
    img[mortar] = rgb(MORTAR)
    img[mortar & ((pools < 0.4) | (THRESH < 0.35))] = rgb(SHADOW)
    img[crack] = rgb(SHADOW)

    img[(damp > 0.55) & (damp * 0.8 > THRESH)] = rgb(DAMP[0])
    img[(damp > 0.25) & (damp <= 0.55) & (damp * 0.5 > THRESH)] = rgb(DAMP[1])
    on = rust > THRESH * 0.8
    img[on & (rust > 0.7)] = rgb(RUST[2])
    img[on & (rust <= 0.7) & (rust > 0.35)] = rgb(RUST[1])
    img[on & (rust <= 0.35)] = rgb(RUST[0])
    for px, py in pins:  # the iron pin the rust bleeds from
        for dx, dy, c in ((0, 0, RUST[0]), (1, 0, RUST[1]), (0, 1, RUST[1]), (1, 1, RUST[2])):
            img[(py + dy) % SIZE, (px + dx) % SIZE] = rgb(c)

    on = moss > THRESH
    hi = (moss - 0.45 > THRESH) & (ly_map < 0.6) & ~mortar
    img[on & (moss > 0.5) & ~hi] = rgb(MOSS[1])
    img[on & (moss <= 0.5)] = rgb(MOSS[0])
    img[hi] = rgb(MOSS[2])

    return Image.fromarray(img, "RGB")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--preview", help="also write a 2x nearest-neighbor 3x3 preview here")
    args = ap.parse_args()
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    path = os.path.join(root, "assets", "skins", "castle-stone.png")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tile = build()
    colors = len(tile.getcolors(1 << 16))
    tile.save(path, optimize=True)
    print(f"wrote {path} ({SIZE}x{SIZE}, {colors} colors)")
    if args.preview:
        grid = Image.new("RGB", (SIZE * 3, SIZE * 3))
        for gy in range(3):
            for gx in range(3):
                grid.paste(tile, (gx * SIZE, gy * SIZE))
        grid = grid.resize((SIZE * 6, SIZE * 6), Image.NEAREST)
        grid.save(args.preview)
        print(f"wrote {args.preview}")


if __name__ == "__main__":
    main()
