#!/usr/bin/env python3
"""Generate the Neon District skin's steel tile: assets/skins/neon-steel.png.

Pixel art in a limited palette (at most 24 colors): grimy riveted steel
plates in near-black violet, each plate its own shade, deep bevelled seams,
rivets along the plate edges, patchy dithered shadow, oil stains, bright
scratches, rust streaks running down from rivets and seams, one short strip
of worn hazard tape, faint scanlines, and magenta and cyan neon staining
bleeding out of some seams. The grime is as much deep shadow as bright
detail, so the tile's average stays dark and labels on top stay readable.

Seamless on every edge by construction: plate rows and widths sum to the
tile size, the noise is periodic, the scanline period divides the tile, and
every stamp (rivet, rust, oil, scratch, stain, tape) writes its pixels mod the tile size.
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
STAIN_GAIN = 0.8  # neon staining turned down 20%

# Palette, darkest first within each group.
SEAM_DEEP = 0x010003
SEAM = 0x05030A
STEEL = [0x08060E, 0x0D0A17, 0x13101F, 0x1A1628, 0x221D32, 0x2B253D, 0x352E4A]
EDGE = 0x433A58
RIVET = 0x655A7E
RUST = [0x1E0E0C, 0x331710, 0x4C2413, 0x643318]
TAPE = 0x4E3A0C
PINK = [0x2C0B2A, 0x521348, 0x80207A]
CYAN = [0x08232A, 0x0E3C47, 0x15606F]

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
    # Grime draws from its own generator so the plate layout above stays put.
    rng = np.random.default_rng(SEED + 1)
    ys, xs = np.mgrid[0:SIZE, 0:SIZE]

    # Steel value on the STEEL ramp (0..6): each plate its own shade, worn
    # and grimy, sunk in patchy pools of shadow.
    tone = rng.uniform(0.9, 4.9, size=n)
    grime = fbm(rng, [(4, 1.0), (8, 0.7), (16, 0.5), (32, 0.3)])
    pools = fbm(rng, [(2, 1.0), (4, 0.6), (8, 0.3)])
    v = tone[pid] + (grime - 0.5) * 3.6
    v -= np.clip(0.52 - pools, 0, None) * 7.0
    v += rng.normal(0.0, 0.45, size=(SIZE, SIZE))  # fine grain
    # Scanlines: every other row a touch darker (period 2 divides SIZE).
    v -= (ys % 2) * 0.35
    # Bevel: plates lit on top and left, in shade on the bottom and right.
    v += np.where(ly < 2, 1.6, 0) + np.where(lx < 1, 0.9, 0)
    v -= np.where(lyb < 3, 1.8, 0) + np.where(lxr < 2, 1.2, 0)
    # Grime settles along each plate's lower edge.
    v -= np.clip(1 - lyb / 9.0, 0, None) * 1.2

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

    # Rust: streaks running down from rivets and from the top seam of some
    # plates, edges wandering, heaviest at the source.
    wander = periodic_noise(rng, 16)
    rust = np.zeros((SIZE, SIZE))

    def streak(rx, ry, length, width):
        for t in range(length):
            py = (ry + t) % SIZE
            fade = (1 - t / length) ** 0.7
            sway = (wander[py, rx % SIZE] - 0.5) * 4
            wdt = width * (0.5 + 0.7 * fade)
            for dx in range(-5, 6):
                d = abs(dx - sway)
                if d > wdt:
                    continue
                px = (rx + dx) % SIZE
                rust[py, px] = max(rust[py, px], fade * (1 - d / (wdt + 0.6)))

    pick = rng.choice(len(rivets), size=max(1, len(rivets) // 6), replace=False)
    for k in pick:
        rx, ry = rivets[k]
        streak(rx, ry + 2, int(rng.integers(14, 46)), rng.uniform(1.6, 3.4))
    for i in rng.choice(n, size=max(1, n // 2), replace=False):
        x0, y0, w, h = plates[i]
        for _ in range(int(rng.integers(1, 3))):
            sx = (x0 + int(rng.integers(6, max(7, w - 8)))) % SIZE
            streak(sx, y0, int(rng.integers(12, 40)), rng.uniform(1.2, 2.6))
    rust *= 0.45 + 0.9 * periodic_noise(rng, 32)

    # Oil: dark blots pooled on a few plates.
    oil = np.zeros((SIZE, SIZE))
    blot = periodic_noise(rng, 64)
    for _ in range(7):
        cx, cy = rng.integers(0, SIZE, size=2)
        r = rng.uniform(6, 14)
        ddx = (xs - cx + SIZE // 2) % SIZE - SIZE // 2
        ddy = (ys - cy + SIZE // 2) % SIZE - SIZE // 2
        d = np.sqrt(ddx**2 + (ddy * 1.4) ** 2) / r
        oil = np.maximum(oil, np.clip(1.2 - d + (blot - 0.5) * 0.9, 0, 1))
    v -= oil * 3.2

    # Neon staining: glow bleeding out of seams, fading with distance.
    stain_p = np.zeros((SIZE, SIZE))
    stain_c = np.zeros((SIZE, SIZE))
    ragged = periodic_noise(rng, 32)
    for j, i in enumerate(rng.choice(n, size=7, replace=False)):
        x0, y0, w, h = plates[i]
        # Along this plate's bottom seam, over part of its length.
        sx = x0 + int(rng.integers(0, max(1, w // 3)))
        span = int(rng.integers(w // 2, w + 20))
        sy = (y0 + h - 1) % SIZE
        cx = (sx + span / 2) % SIZE
        ddx = (xs - cx + SIZE // 2) % SIZE - SIZE // 2
        ddy = (ys - sy + SIZE // 2) % SIZE - SIZE // 2
        along = np.clip(1 - np.abs(ddx) / (span / 2), 0, 1) ** 0.5
        reach = rng.uniform(10, 20)
        glow = along * np.clip(1 - np.abs(ddy) / reach + (ragged - 0.5) * 0.6, 0, 1)
        if j % 2 == 0:
            stain_p = np.maximum(stain_p, glow)
        else:
            stain_c = np.maximum(stain_c, glow)

    idx = np.clip(np.floor(v + THRESH - 0.5), 0, len(STEEL) - 1).astype(int)

    # Compose.
    img = np.zeros((SIZE, SIZE, 3), dtype=np.uint8)
    img[:] = np.array([rgb(c) for c in STEEL], dtype=np.uint8)[idx]
    # The lit top bevel catches light on the brighter plates.
    img[(ly == 0) & ~seam & (idx >= 3) & (THRESH < 0.6)] = rgb(EDGE)
    img[seam] = rgb(SEAM)
    img[seam & ((pools < 0.5) | (THRESH < 0.4))] = rgb(SEAM_DEEP)
    # Oil at its heart goes near black.
    img[(oil > 0.75) & (THRESH < 0.7) & ~seam] = rgb(SEAM)

    # Scratches: short bright gouges at a slant, on plates only.
    for _ in range(26):
        x, y = rng.uniform(0, SIZE, size=2)
        ang = rng.uniform(-0.5, 0.5) + (np.pi / 2 if rng.random() < 0.3 else 0)
        for t in range(int(rng.integers(5, 18))):
            px = int(x + np.cos(ang) * t) % SIZE
            py = int(y + np.sin(ang) * t) % SIZE
            if not seam[py, px] and rng.random() < 0.85:
                img[py, px] = rgb(EDGE if t % 5 else STEEL[-1])
                img[(py + 1) % SIZE, px] = rgb(STEEL[0])

    on = rust > THRESH * 0.8
    img[on & (rust > 0.75)] = rgb(RUST[3])
    img[on & (rust <= 0.75) & (rust > 0.5)] = rgb(RUST[2])
    img[on & (rust <= 0.5) & (rust > 0.25)] = rgb(RUST[1])
    img[on & (rust <= 0.25)] = rgb(RUST[0])

    for stain, ramp in ((stain_p * STAIN_GAIN, PINK), (stain_c * STAIN_GAIN, CYAN)):
        on = stain * 0.95 > THRESH
        img[on & (stain <= 0.45)] = rgb(ramp[0])
        img[on & (stain > 0.45) & (stain <= 0.75)] = rgb(ramp[1])
        img[on & (stain > 0.75)] = rgb(ramp[2])

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
        img[(ry + 2) % SIZE, rx % SIZE] = rgb(SEAM)

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
    assert colors <= 24, f"{colors} colors"
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
