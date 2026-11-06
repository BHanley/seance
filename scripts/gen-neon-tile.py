#!/usr/bin/env python3
"""Generate the Neon District skin's steel tile: assets/skins/neon-steel.png.

Pixel art in a limited palette (at most 20 colors): grimy riveted steel
plates in near-black violet, bevelled seams, rivets along the plate edges,
rust bleeding from a few rivets, one short strip of worn hazard tape, faint
scanlines, magenta and cyan neon staining that bleeds out of some seams,
ordered (Bayer 8x8) dithered shadow and fine grain. Kept dark and low
contrast so labels on top stay readable.

Seamless on every edge by construction: plate rows and widths sum to the
tile size, the noise is periodic, the scanline period divides the tile, and
every stamp (rivet, rust, stain, tape) writes its pixels mod the tile size.
Every pixel is picked from the fixed palette, never blended.

Usage:
  uv run --offline --no-project --with numpy --with pillow \
    python scripts/gen-neon-tile.py [--preview out.png]
Output is deterministic (fixed seed).
"""

import argparse
import os

import numpy as np
from PIL import Image

SIZE = 256
SEED = 11

# Palette, darkest first within each group.
SEAM_DEEP = 0x020106
SEAM = 0x06040C
STEEL = [0x0A0812, 0x0E0B18, 0x13101E, 0x181424, 0x1D182A]
EDGE = 0x262033
RIVET = 0x342D44
RUST = [0x1C0E0D, 0x2E150F, 0x442013]
TAPE = 0x3A2A0A
PINK = [0x220A20, 0x3A0E34]
CYAN = [0x071E24, 0x0B3139]

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
    heights = split(rng, SIZE, 36, 76)

    plate = np.full((SIZE, SIZE), -1, dtype=np.int32)  # -1 is a seam
    ly = np.zeros((SIZE, SIZE), dtype=np.int32)  # rows from the plate's top
    lyb = np.zeros((SIZE, SIZE), dtype=np.int32)  # rows from its bottom
    lx = np.zeros((SIZE, SIZE), dtype=np.int32)
    lxr = np.zeros((SIZE, SIZE), dtype=np.int32)
    plates = []  # (x0, y0, w, h)
    y = 0
    for h in heights:
        widths = split(rng, SIZE, 44, 120)
        x = int(rng.integers(0, SIZE))
        for w in widths:
            pid = len(plates)
            plates.append((x % SIZE, y, w, h))
            # A plate is its box less a 2 px seam on its right and bottom.
            for dy in range(h - 2):
                for dx in range(w - 2):
                    px, py = (x + dx) % SIZE, (y + dy) % SIZE
                    plate[py, px] = pid
                    ly[py, px] = dy
                    lyb[py, px] = h - 3 - dy
                    lx[py, px] = dx
                    lxr[py, px] = w - 3 - dx
            x += w
        y += h

    n = len(plates)
    seam = plate < 0
    pid = np.where(seam, 0, plate)
    tone = rng.uniform(0.9, 3.2, size=n)

    # Steel value on the STEEL ramp (0..4), worn and grimy, in pools of shadow.
    grime = fbm(rng, [(4, 1.0), (8, 0.6), (16, 0.4), (32, 0.25)])
    pools = periodic_noise(rng, 2)
    v = tone[pid] + (grime - 0.5) * 2.6
    v -= np.clip(0.55 - pools, 0, None) * 3.0
    v += rng.normal(0.0, 0.4, size=(SIZE, SIZE))  # fine grain
    # Scanlines: every other row a touch darker (period 2 divides SIZE).
    v -= (np.arange(SIZE)[:, None] % 2) * 0.35
    # Bevel: plates lit on top and left, in shade on the bottom and right.
    v += np.where(ly < 2, 0.9, 0) + np.where(lx < 1, 0.5, 0)
    v -= np.where(lyb < 2, 1.0, 0) + np.where(lxr < 1, 0.6, 0)

    # Rivets inset along each plate's edges.
    rivets = []
    for x0, y0, w, h in plates:
        inner_w, inner_h = w - 2, h - 2
        step_x = max(10, inner_w // max(1, inner_w // 14))
        for dx in range(4, inner_w - 4, step_x):
            rivets.append(((x0 + dx) % SIZE, (y0 + 3) % SIZE))
            rivets.append(((x0 + dx) % SIZE, (y0 + inner_h - 5) % SIZE))
        for dy in (inner_h // 2,) if inner_h > 30 else ():
            rivets.append(((x0 + 3) % SIZE, (y0 + dy) % SIZE))
            rivets.append(((x0 + inner_w - 5) % SIZE, (y0 + dy) % SIZE))

    # Rust: streaks running down from a few rivets, edges wandering.
    wander = periodic_noise(rng, 16)
    rust = np.zeros((SIZE, SIZE))
    pick = rng.choice(len(rivets), size=max(1, len(rivets) // 14), replace=False)
    for k in pick:
        rx, ry = rivets[k]
        length = int(rng.integers(8, 30))
        width = rng.uniform(1.6, 3.0)
        for t in range(length):
            py = (ry + 2 + t) % SIZE
            fade = (1 - t / length) ** 0.9
            sway = (wander[py, rx % SIZE] - 0.5) * 3
            wdt = width * (0.6 + 0.6 * fade)
            for dx in range(-4, 5):
                d = abs(dx - sway)
                if d > wdt:
                    continue
                px = (rx + dx) % SIZE
                rust[py, px] = max(rust[py, px], fade * (1 - d / (wdt + 0.5)))
    rust *= 0.5 + periodic_noise(rng, 32)

    # Neon staining: glow bleeding out of a few seams, fading with distance.
    stain_p = np.zeros((SIZE, SIZE))
    stain_c = np.zeros((SIZE, SIZE))
    ys, xs = np.mgrid[0:SIZE, 0:SIZE]
    ragged = periodic_noise(rng, 32)
    for i in rng.choice(n, size=5, replace=False):
        x0, y0, w, h = plates[i]
        # Along this plate's bottom seam, over part of its length.
        sx = x0 + int(rng.integers(0, max(1, w // 2)))
        span = int(rng.integers(w // 3, w))
        sy = (y0 + h - 1) % SIZE
        cx = (sx + span / 2) % SIZE
        ddx = (xs - cx + SIZE // 2) % SIZE - SIZE // 2
        ddy = (ys - sy + SIZE // 2) % SIZE - SIZE // 2
        along = np.clip(1 - np.abs(ddx) / (span / 2), 0, 1) ** 0.6
        reach = rng.uniform(5, 10)
        glow = along * np.clip(1 - np.abs(ddy) / reach + (ragged - 0.5) * 0.5, 0, 1)
        if i % 2 == 0:
            stain_p = np.maximum(stain_p, glow)
        else:
            stain_c = np.maximum(stain_c, glow)

    idx = np.clip(np.floor(v + THRESH - 0.5), 0, len(STEEL) - 1).astype(int)

    # Compose.
    img = np.zeros((SIZE, SIZE, 3), dtype=np.uint8)
    img[:] = np.array([rgb(c) for c in STEEL], dtype=np.uint8)[idx]
    # The lit top bevel catches a little light on the brighter plates.
    img[(ly == 0) & ~seam & (idx >= 2) & (THRESH < 0.5)] = rgb(EDGE)
    img[seam] = rgb(SEAM)
    img[seam & ((pools < 0.45) | (THRESH < 0.3))] = rgb(SEAM_DEEP)

    on = rust > THRESH * 0.9
    img[on & (rust > 0.7)] = rgb(RUST[2])
    img[on & (rust <= 0.7) & (rust > 0.35)] = rgb(RUST[1])
    img[on & (rust <= 0.35)] = rgb(RUST[0])

    for stain, ramp in ((stain_p, PINK), (stain_c, CYAN)):
        on = stain * 0.85 > THRESH
        img[on & (stain <= 0.6)] = rgb(ramp[0])
        img[on & (stain > 0.6)] = rgb(ramp[1])

    # Hazard tape, once: a short strip of worn diagonal stripes across one
    # plate's lower edge, half the stripes scuffed away.
    tx0, ty0, tw, th = plates[int(rng.integers(0, n))]
    strip_y = (ty0 + th - 10) % SIZE
    wear = periodic_noise(rng, 64)
    for dy in range(4):
        for dx in range(28):
            px, py = (tx0 + 8 + dx) % SIZE, (strip_y + dy) % SIZE
            if plate[py, px] < 0:
                continue
            if (dx + dy) % 6 < 3 and wear[py, px] > 0.35:
                img[py, px] = rgb(TAPE)
            else:
                img[py, px] = rgb(SEAM)

    # Rivets: a lit cap with a shadow under it.
    for rx, ry in rivets:
        if plate[ry % SIZE, rx % SIZE] < 0:
            continue
        img[ry % SIZE, rx % SIZE] = rgb(RIVET)
        img[ry % SIZE, (rx + 1) % SIZE] = rgb(EDGE)
        img[(ry + 1) % SIZE, rx % SIZE] = rgb(EDGE)
        img[(ry + 1) % SIZE, (rx + 1) % SIZE] = rgb(SEAM)
        img[(ry + 2) % SIZE, (rx + 1) % SIZE] = rgb(SEAM_DEEP)

    return Image.fromarray(img, "RGB")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--preview", help="also write a 2x nearest-neighbor 3x3 preview here")
    args = ap.parse_args()
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    path = os.path.join(root, "assets", "skins", "neon-steel.png")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tile = build()
    colors = len(tile.getcolors(1 << 16))
    assert colors <= 20, f"{colors} colors"
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
