#!/usr/bin/env python3
"""Generate the Linocut skins' tiles: assets/skins/linocut-paper.png and
assets/skins/linocut-night.png.

A hand-cut, hand-printed block: parallel gouge strokes of varied length and
weight (tapered at both ends, the way a V-gouge leaves them), short chatter
marks left in the cleared areas between strokes, ink speckle, and a slight
mis-registration (a second, offset pass in a second ink). Hard pixels, no
antialiasing, a handful of flat colors per tile.

Seamless on every edge by construction: every stroke, mark and speck writes
its pixels mod the tile size.

Usage: python3 scripts/gen-linocut-tile.py [--preview out.png]
Needs Pillow and numpy. Output is deterministic (fixed seeds).
"""

import argparse
import os

import numpy as np
from PIL import Image

SIZE = 256

# (ground, stroke, deep stroke, chatter, speck, ghost): kept close to the
# ground so text on top stays readable (docs/THEME.md has the numbers).
PAPER = dict(
    seed=11,
    ground=0xEDE3CD,
    stroke=0xCDBB95,
    deep=0xB7A27A,
    chatter=0xD6C6A4,
    speck=0xA8926A,
    ghost=0xE3C3A6,
)
NIGHT = dict(
    seed=23,
    ground=0x161216,
    stroke=0x3C2E21,
    deep=0x54402A,
    chatter=0x2E241C,
    speck=0x6A5A44,
    ghost=0x33220F,
)

ANGLE = np.radians(-27.0)


def stroke_mask(rng, n_cells=8):
    """Parallel gouge strokes: one candidate per cell of a jittered grid."""
    heavy = np.zeros((SIZE, SIZE), bool)
    light = np.zeros((SIZE, SIZE), bool)
    cell = SIZE // n_cells
    for cy in range(n_cells):
        for cx in range(n_cells):
            for _ in range(rng.choice([1, 1, 2])):
                if rng.random() < 0.18:
                    continue
                cx0 = cx * cell + rng.uniform(0, cell)
                cy0 = cy * cell + rng.uniform(0, cell)
                length = rng.uniform(16, 78)
                weight = rng.choice([1.2, 1.8, 2.6, 3.4], p=[0.3, 0.4, 0.2, 0.1])
                a = ANGLE + rng.normal(0, 0.07)
                paint_stroke(heavy if weight >= 2.4 else light, cx0, cy0, length, weight, a)
    return heavy, light


def paint_stroke(mask, x0, y0, length, weight, angle):
    dx, dy = np.cos(angle), np.sin(angle)
    r = int(length / 2 + weight + 2)
    ys, xs = np.mgrid[-r : r + 1, -r : r + 1]
    along = xs * dx + ys * dy
    across = -xs * dy + ys * dx
    t = (along + length / 2) / length
    inside = (t >= 0) & (t <= 1)
    # A gouge cuts deepest mid-stroke and runs out at both ends.
    half = weight / 2 * np.sin(np.pi * np.clip(t, 0, 1)) ** 0.6
    hit = inside & (np.abs(across) <= half + 0.15)
    yy = (np.round(y0).astype(int) + ys[hit]) % SIZE
    xx = (np.round(x0).astype(int) + xs[hit]) % SIZE
    mask[yy, xx] = True


def chatter_mask(rng, strokes):
    """Rows of short ticks across the stroke direction, in the gaps."""
    m = np.zeros((SIZE, SIZE), bool)
    dx, dy = np.cos(ANGLE), np.sin(ANGLE)
    px_, py_ = -dy, dx
    for _ in range(46):
        x0, y0 = rng.uniform(0, SIZE, 2)
        n = rng.integers(3, 7)
        for k in range(n):
            cx = x0 + dx * 3.0 * k
            cy = y0 + dy * 3.0 * k
            ln = rng.integers(3, 6)
            for s in range(ln):
                x = int(round(cx + px_ * (s - ln / 2))) % SIZE
                y = int(round(cy + py_ * (s - ln / 2))) % SIZE
                if not strokes[y, x]:
                    m[y, x] = True
    return m


def speck_mask(rng):
    m = rng.random((SIZE, SIZE)) < 0.011
    # A few two-pixel blots, like ink caught in a cleared corner.
    for _ in range(26):
        x, y = rng.integers(0, SIZE, 2)
        m[y % SIZE, x % SIZE] = True
        m[y % SIZE, (x + 1) % SIZE] = True
    return m


def rgb(c):
    return np.array([(c >> 16) & 255, (c >> 8) & 255, c & 255], np.uint8)


def make(spec):
    rng = np.random.default_rng(spec["seed"])
    heavy, light = stroke_mask(rng)
    strokes = heavy | light
    chat = chatter_mask(rng, strokes)
    speck = speck_mask(rng)
    # Mis-registration: the strokes printed again 2 px right and 1 down in a
    # second ink, showing only where the first pass leaves the paper bare.
    ghost = np.roll(np.roll(strokes, 2, axis=1), 1, axis=0) & ~strokes
    img = np.zeros((SIZE, SIZE, 3), np.uint8)
    img[:] = rgb(spec["ground"])
    img[ghost] = rgb(spec["ghost"])
    img[chat] = rgb(spec["chatter"])
    img[light] = rgb(spec["stroke"])
    img[heavy] = rgb(spec["deep"])
    img[speck & ~strokes] = rgb(spec["speck"])
    return img


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--preview")
    args = ap.parse_args()
    out = os.path.join(os.path.dirname(__file__), "..", "assets", "skins")
    tiles = {}
    for name, spec in (("paper", PAPER), ("night", NIGHT)):
        img = make(spec)
        tiles[name] = img
        Image.fromarray(img).save(os.path.join(out, f"linocut-{name}.png"))
    if args.preview:
        sheet = np.concatenate(
            [np.tile(t, (2, 2, 1)) for t in tiles.values()], axis=1
        )
        Image.fromarray(sheet).resize(
            (sheet.shape[1] * 2, sheet.shape[0] * 2), Image.NEAREST
        ).save(args.preview)


if __name__ == "__main__":
    main()
