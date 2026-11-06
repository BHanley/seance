//! Pixel wall torches for skins that ask for them (castle): one per pane,
//! in its title strip, burning the way the pane is doing.
//!
//! A torch is 12x20 texels drawn as a handful of `paint_quad` runs in one
//! canvas, so it adds no elements per pixel. Its frame comes from the wall
//! clock, like the lead creature in `app/sprite.rs`; the app's torch tick
//! (see `tick_ms`) only asks for a render as often as the fastest torch on
//! screen needs one.

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

impl Torch {
    /// How long one animation frame lasts.
    pub fn frame_ms(self) -> u64 {
        match self {
            Torch::Working => 100,
            Torch::NeedsYou => 125,
            Torch::Idle => 200,
            Torch::Stopped => 400,
        }
    }
}

/// How often the app should render for these torches: the fastest frame
/// among them, or `None` (no tick at all) when none are on screen.
pub fn tick_ms(on_screen: impl IntoIterator<Item = Torch>) -> Option<u64> {
    on_screen.into_iter().map(Torch::frame_ms).min()
}

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

type Grid = [[Option<u32>; W]; H];

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
/// `half` texels either side of centre; the top `wild` rows jitter sideways.
fn flame(g: &mut Grid, h: usize, half: f32, wild: usize, r: u64, hot: bool) {
    for k in 0..h {
        let y = 10 - k as i32;
        if y < 0 {
            break;
        }
        let t = (k as f32 + 1.0) / (h as f32 + 1.0);
        let w = half * (std::f32::consts::PI * t).sin().powf(0.7) * (1.0 - 0.35 * t);
        let jitter = if k + wild >= h {
            (hash(r, k as u64) % 3) as f32 - 1.0
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

/// The torch's texels at animation frame `frame`.
pub fn texels(state: Torch, frame: u64, seed: u64) -> Grid {
    let mut g: Grid = [[None; W]; H];
    sconce(&mut g);
    let r = hash(seed, frame);
    match state {
        Torch::Idle => {
            // Mostly still: the tip sways every other frame and the
            // height breathes by one row now and then.
            let r = hash(seed, frame / 2);
            let h = 6 + usize::from(r % 4 == 0);
            flame(&mut g, h, 2.0, 1, r, false);
        }
        Torch::Working => {
            let h = 9 + (r % 3) as usize;
            flame(&mut g, h, 3.0, h / 2, r, true);
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
            let len = 5 + (r % 3) as usize;
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

/// A torch at `scale` points per texel, frame picked from `now` (unix ms).
pub fn element(state: Torch, now: u64, seed: u64, scale: f32) -> impl IntoElement {
    let frame = now / state.frame_ms();
    let grid = texels(state, frame, seed);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint(&grid, bounds, scale, window),
    )
    .flex_none()
    .w(px(W as f32 * scale))
    .h(px(H as f32 * scale))
}

/// One quad per horizontal run of one color.
fn paint(grid: &Grid, bounds: Bounds<Pixels>, scale: f32, window: &mut Window) {
    let o = bounds.origin;
    for (y, row) in grid.iter().enumerate() {
        let mut x = 0;
        while x < W {
            let Some(c) = row[x] else {
                x += 1;
                continue;
            };
            let start = x;
            while x < W && row[x] == Some(c) {
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
    fn the_fastest_torch_on_screen_sets_the_tick_and_none_means_no_tick() {
        assert_eq!(tick_ms([]), None);
        assert_eq!(tick_ms([Torch::Stopped]), Some(400));
        assert_eq!(tick_ms([Torch::Idle, Torch::Stopped]), Some(200));
        assert_eq!(
            tick_ms([Torch::Idle, Torch::Working, Torch::NeedsYou]),
            Some(100)
        );
        // Nothing renders faster than 10 fps.
        for t in [Torch::Idle, Torch::Working, Torch::NeedsYou, Torch::Stopped] {
            assert!(t.frame_ms() >= 100, "{t:?}");
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
