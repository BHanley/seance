//! Pixel wall torches for skins that ask for them (castle): one per pane,
//! in its title strip, burning the way the pane is doing. A skin that hangs
//! neon tubes instead (`Lamp::Tube`) gets `crate::tube`'s texels in the same
//! spots from the same `element` calls.
//!
//! A torch is 12x20 texels drawn as a handful of `paint_quad` runs in one
//! canvas, so it adds no elements per pixel. Its frame is the app's tick
//! count: torches step once per `TICK_MS` spinner tick, every state on
//! every tick, and never ask for a render of their own.

use gpui::{canvas, point, px, size, Bounds, Hsla, IntoElement, Pixels, Styled, Window};

pub const W: usize = 12;
pub const H: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Torch {
    /// Calm flame, a slow flicker.
    Idle,
    /// Roaring: tall, bright, fast, throwing sparks.
    Working,
    /// Blown flat sideways: someone has to look.
    NeedsYou,
    /// Out: a dead ember and a wisp of smoke.
    Stopped,
}

/// One torch frame per tick of the app's 240 ms spinner timer, for every
/// state: the torches ride that timer and never run a faster one.
pub const TICK_MS: u64 = 240;

/// Idle's tip sway, one step per frame; no two neighbours match.
const IDLE_SWAY: [i32; 4] = [-1, 0, 1, 0];

/// A per-pane seed so torches don't flicker in step.
pub fn seed(slug: &str) -> u64 {
    slug.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn hash(a: u64, b: u64) -> u64 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x
}

const IRON: u32 = 0x2A2522;
const IRON_LIT: u32 = 0x4A3F35;
const IRON_DARK: u32 = 0x151210;
const EMBER: u32 = 0x6B2A1A;
const EMBER_LIT: u32 = 0x9A3E1E;
const OUTER: u32 = 0xB8461E;
const MID: u32 = 0xE8892A;
const CORE: u32 = 0xFFD27A;
const HOT: u32 = 0xFFF3C4;
const SMOKE: [u32; 3] = [0x6E6A65, 0x5A5652, 0x47443F];

pub type Grid = [[Option<u32>; W]; H];

/// The sconce: a bowl on a short stem, bolted to a wall plate.
fn sconce(g: &mut Grid) {
    for x in 3..9 {
        g[11][x] = Some(if x == 3 { IRON } else { IRON_LIT });
        g[12][x] = Some(IRON);
    }
    g[12][3] = Some(IRON_DARK);
    g[12][8] = Some(IRON_DARK);
    for x in 4..8 {
        g[13][x] = Some(if x == 7 { IRON_DARK } else { IRON });
    }
    for y in 14..17 {
        g[y][5] = Some(IRON_LIT);
        g[y][6] = Some(IRON);
    }
    for y in 17..20 {
        for x in 4..8 {
            g[y][x] = Some(if y == 19 || x == 7 { IRON_DARK } else { IRON });
        }
    }
    g[18][5] = Some(IRON_LIT);
}

/// Flame rising from the bowl (row 10 is its base) `h` rows tall, at most
/// `half` texels either side of centre; the top `wild` rows jitter sideways,
/// by `sway` when given, else at random.
fn flame(g: &mut Grid, h: usize, half: f32, wild: usize, r: u64, sway: Option<i32>, hot: bool) {
    for k in 0..h {
        let y = 10 - k as i32;
        if y < 0 {
            break;
        }
        let t = (k as f32 + 1.0) / (h as f32 + 1.0);
        let w = half * (std::f32::consts::PI * t).sin().powf(0.7) * (1.0 - 0.35 * t);
        let jitter = if k + wild >= h {
            sway.map_or((hash(r, k as u64) % 3) as f32 - 1.0, |s| s as f32)
        } else {
            0.0
        };
        let c = 5.5 + jitter * 0.6;
        let (lo, hi) = ((c - w).round() as i32, (c + w).round() as i32);
        for x in lo..=hi {
            if !(0..W as i32).contains(&x) {
                continue;
            }
            let d = (x as f32 - c).abs() / w.max(0.5);
            let color = if d > 0.7 || k + 1 == h {
                OUTER
            } else if d > 0.35 || k as f32 > h as f32 * 0.6 {
                MID
            } else if hot && k < 2 {
                HOT
            } else {
                CORE
            };
            g[y as usize][x as usize] = Some(color);
        }
    }
}

/// The torch's texels at animation frame `frame` (the tick count). Each
/// state differs from one frame to the next, so every tick shows motion.
pub fn texels(state: Torch, frame: u64, seed: u64) -> Grid {
    let mut g: Grid = [[None; W]; H];
    sconce(&mut g);
    // The seed shifts the cycles so torches don't move in step.
    let f = frame.wrapping_add(seed % 12);
    let r = hash(seed, frame);
    match state {
        Torch::Idle => {
            // Calm: the tip sways a texel each frame and the height
            // breathes by one row every third.
            let h = 6 + usize::from(f % 3 == 0);
            flame(
                &mut g,
                h,
                2.0,
                1,
                r,
                Some(IDLE_SWAY[(f % 4) as usize]),
                false,
            );
        }
        Torch::Working => {
            // A new height every frame, so it never holds a pose.
            let h = 9 + (f % 3) as usize;
            flame(&mut g, h, 3.0, h / 2, r, None, true);
            // A few sparks above and beside the flame.
            for i in 0..3 {
                let s = hash(r, 100 + i);
                if s % 3 == 0 {
                    continue;
                }
                let x = 2 + (s >> 8) as usize % 8;
                let y = (s >> 16) as usize % 4;
                g[y][x] = Some(if s % 2 == 0 { CORE } else { MID });
            }
        }
        Torch::NeedsYou => {
            // Blown flat: a tongue streaming right off the bowl, thinning
            // to its tip, with torn-off streamers racing past it.
            let len = 5 + (f % 3) as usize;
            for i in 0..len {
                let x = 5 + i;
                let t = i as f32 / len as f32;
                let mid = 9.0 - t * 1.6;
                let half = 1.6 * (1.0 - t) + 0.4;
                let (top, bottom) = ((mid - half).round() as usize, (mid + half).round() as usize);
                for y in top..=bottom.min(10) {
                    let edge = y == top || y == bottom;
                    g[y][x] = Some(if t > 0.65 || edge {
                        OUTER
                    } else if t > 0.3 {
                        MID
                    } else {
                        CORE
                    });
                }
            }
            for (row, phase) in [(6usize, 0u64), (8, 1), (10, 2)] {
                let x = 6 + len + ((frame + phase) % 3) as usize;
                for dx in 0..2 {
                    if x + dx < W {
                        g[row][x + dx] = Some(if dx == 0 { MID } else { OUTER });
                    }
                }
            }
        }
        Torch::Stopped => {
            g[10][5] = Some(EMBER);
            g[10][6] = Some(if frame % 3 == 0 { EMBER_LIT } else { EMBER });
            // A wisp rising and swaying; the wave moves up a row a frame.
            for y in 1..10 {
                let phase = (y as f32 + frame as f32) * 0.8;
                let x = (6.0 + 1.6 * phase.sin()).round() as usize;
                let fade = (10 - y) as f32 / 10.0;
                if (y + frame as usize) % 4 == 3 || x >= W {
                    continue;
                }
                let tone = if fade > 0.6 {
                    SMOKE[2]
                } else if fade > 0.3 {
                    SMOKE[1]
                } else {
                    SMOKE[0]
                };
                g[y][x] = Some(tone);
            }
        }
    }
    g
}

/// Whether the active skin hangs neon tubes instead of torches.
fn tubes() -> bool {
    crate::skin::lamp() == Some(seance_core::schemes::Lamp::Tube)
}

/// The active skin's lamp (a torch, or a tube) at `scale` points per texel,
/// at animation frame `frame`.
pub fn element(state: Torch, frame: u64, seed: u64, scale: f32) -> impl IntoElement {
    let grid = if tubes() {
        crate::tube::texels(state, frame, seed)
    } else {
        texels(state, frame, seed)
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint(&grid, bounds, scale, window),
    )
    .flex_none()
    .w(px(W as f32 * scale))
    .h(px(H as f32 * scale))
}

/// The standing torch: narrower than the wall one, and tall.
pub const TALL_W: usize = 8;
pub const TALL_H: usize = 40;

pub type TallGrid = [[Option<u32>; TALL_W]; TALL_H];

/// A standing torch, always calm: the Idle flame and bowl of the wall torch
/// (its columns 2..10) on an iron post with two bands and a foot.
pub fn tall_texels(frame: u64, seed: u64) -> TallGrid {
    let wall = texels(Torch::Idle, frame, seed);
    let mut g: TallGrid = [[None; TALL_W]; TALL_H];
    for y in 0..14 {
        g[y].copy_from_slice(&wall[y][2..2 + TALL_W]);
    }
    for row in g.iter_mut().take(TALL_H - 2).skip(14) {
        row[3] = Some(IRON_LIT);
        row[4] = Some(IRON);
    }
    for y in [20, 30] {
        for x in 2..6 {
            g[y][x] = Some(IRON_DARK);
        }
    }
    for y in TALL_H - 2..TALL_H {
        for x in 1..7 {
            g[y][x] = Some(if y == TALL_H - 1 { IRON_DARK } else { IRON });
        }
    }
    g
}

/// A standing torch (or tube) at `scale` points per texel.
pub fn tall_element(frame: u64, seed: u64, scale: f32) -> impl IntoElement {
    let grid = if tubes() {
        crate::tube::tall_texels(frame, seed)
    } else {
        tall_texels(frame, seed)
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint(&grid, bounds, scale, window),
    )
    .flex_none()
    .w(px(TALL_W as f32 * scale))
    .h(px(TALL_H as f32 * scale))
}

/// One quad per horizontal run of one color.
fn paint<const N: usize>(
    grid: &[[Option<u32>; N]],
    bounds: Bounds<Pixels>,
    scale: f32,
    window: &mut Window,
) {
    let o = bounds.origin;
    for (y, row) in grid.iter().enumerate() {
        let mut x = 0;
        while x < N {
            let Some(c) = row[x] else {
                x += 1;
                continue;
            };
            let start = x;
            while x < N && row[x] == Some(c) {
                x += 1;
            }
            let quad = Bounds::new(
                point(o.x + px(start as f32 * scale), o.y + px(y as f32 * scale)),
                size(px((x - start) as f32 * scale), px(scale)),
            );
            window.paint_quad(gpui::fill(quad, Hsla::from(gpui::rgb(c))));
        }
    }
}

/// Quads one frame of this torch paints (for the cost report and tests).
#[cfg(test)]
pub fn runs(grid: &Grid) -> usize {
    grid.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .filter(|(x, c)| c.is_some() && (*x == 0 || row[x - 1] != **c))
                .count()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_moves_on_every_tick() {
        for s in [Torch::Idle, Torch::Working, Torch::NeedsYou, Torch::Stopped] {
            for name in ["a", "pane", "lead-7"] {
                for f in 0..48 {
                    assert_ne!(
                        texels(s, f, seed(name)),
                        texels(s, f + 1, seed(name)),
                        "{s:?} {name} frame {f}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_standing_torch_moves_on_every_tick_and_keeps_its_flame() {
        for f in 0..48 {
            assert_ne!(
                tall_texels(f, seed("post")),
                tall_texels(f + 1, seed("post"))
            );
        }
        // The wall torch's Idle flame fits the narrower grid whole.
        for f in 0..16 {
            let wall = texels(Torch::Idle, f, seed("post"))
                .iter()
                .take(11)
                .flatten()
                .filter(|c| c.is_some())
                .count();
            let tall = tall_texels(f, seed("post"))
                .iter()
                .take(11)
                .flatten()
                .filter(|c| c.is_some())
                .count();
            assert_eq!(wall, tall, "frame {f}");
        }
    }

    #[test]
    fn only_a_lit_torch_has_flame_and_a_stopped_one_smokes() {
        let flame = |g: &Grid| {
            g.iter()
                .flatten()
                .any(|c| matches!(c, Some(OUTER | MID | CORE | HOT)))
        };
        let smoke = |g: &Grid| {
            g.iter()
                .flatten()
                .any(|c| c.is_some_and(|c| SMOKE.contains(&c)))
        };
        for f in 0..8 {
            for s in [Torch::Idle, Torch::Working, Torch::NeedsYou] {
                let g = texels(s, f, seed("a"));
                assert!(flame(&g) && !smoke(&g), "{s:?} {f}");
            }
            let g = texels(Torch::Stopped, f, seed("a"));
            assert!(!flame(&g) && smoke(&g), "stopped {f}");
        }
    }

    #[test]
    fn a_frame_costs_a_few_dozen_quads_at_most() {
        for s in [Torch::Idle, Torch::Working, Torch::NeedsYou, Torch::Stopped] {
            for f in 0..16 {
                let n = runs(&texels(s, f, seed("pane")));
                assert!(n <= 80, "{s:?} frame {f}: {n} quads");
            }
        }
    }
}
