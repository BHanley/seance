//! Neon tubes for skins that hang them (neon-district) in place of torches:
//! the same spots, footprints, states and tick as `crate::torch`, drawn as a
//! small glass tube on a metal bracket.
//!
//! Idle burns steady cyan with a rare flicker; working buzzes magenta with
//! sparks; needs-you blinks amber; stopped is a dead tube that sputters now
//! and then. Rare events run on a fixed period with a per-tube phase, so
//! every tube has one in any window of that length and no two decorative
//! tubes share one. Glow is solid pre-mixed color (quads have no alpha).

use crate::torch::{Grid, TallGrid, Torch, H, TALL_H, TALL_W, W};

/// Idle flickers once per this many ticks (about 5.8 s at 240 ms).
pub const IDLE_PERIOD: u64 = 24;
/// A dead tube sputters once per this many ticks (about 9.6 s).
pub const DEAD_PERIOD: u64 = 40;

const STEEL: u32 = 0x2C2B3A;
const STEEL_LIT: u32 = 0x4A4860;
const STEEL_DARK: u32 = 0x15141E;
const DEAD_EDGE: u32 = 0x2A2438;
const DEAD_GAS: u32 = 0x15121E;

/// A tube color from faint glow to white-hot core.
#[derive(Clone, Copy)]
struct Hue {
    glow: u32,
    mid: u32,
    bright: u32,
    core: u32,
}

const CYAN: Hue = Hue {
    glow: 0x0A3D48,
    mid: 0x0A93A3,
    bright: 0x12F7FF,
    core: 0xD2FFFF,
};
const MAGENTA: Hue = Hue {
    glow: 0x4A1140,
    mid: 0xB01F80,
    bright: 0xFF2FB5,
    core: 0xFFD3EE,
};
const AMBER: Hue = Hue {
    glow: 0x4A3010,
    mid: 0xA87418,
    bright: 0xFFB627,
    core: 0xFFF0C8,
};

/// How lit a stretch of glass is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Lit {
    Dead,
    Dim,
    On,
    Hot,
}

fn hash(a: u64, b: u64) -> u64 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x
}

/// Glass four texels wide with its left edge at column `x`, rows `top..bot`,
/// each row lit as `lit(row)` says, with a halo one column out each side.
fn glass<const N: usize>(
    g: &mut [[Option<u32>; N]],
    x: usize,
    top: usize,
    bot: usize,
    hue: Hue,
    lit: impl Fn(usize) -> Lit,
) {
    for y in top..bot {
        let l = lit(y);
        let (edge, gas_l, gas_r) = match l {
            Lit::Dead => (DEAD_EDGE, DEAD_GAS, DEAD_GAS),
            Lit::Dim => (hue.glow, hue.mid, hue.mid),
            Lit::On => (hue.mid, hue.core, hue.bright),
            Lit::Hot => (hue.bright, hue.core, hue.core),
        };
        g[y][x] = Some(edge);
        g[y][x + 1] = Some(gas_l);
        g[y][x + 2] = Some(gas_r);
        g[y][x + 3] = Some(edge);
        // The halo: every other row when on, three in four when hot.
        let halo = match l {
            Lit::Hot => y % 4 != 3,
            Lit::On => y % 2 == 0,
            _ => false,
        };
        if halo && y > top && y + 1 < bot {
            if x > 0 {
                g[y][x - 1] = Some(hue.glow);
            }
            if x + 4 < N {
                g[y][x + 4] = Some(hue.glow);
            }
        }
    }
}

/// The wall bracket: a clip over the tube, a cap under it, a short arm and
/// a riveted wall plate.
fn bracket(g: &mut Grid) {
    for x in 3..9 {
        g[1][x] = Some(if x == 3 || x == 8 {
            STEEL_DARK
        } else {
            STEEL_LIT
        });
        g[2][x] = Some(STEEL);
    }
    for x in 4..8 {
        g[16][x] = Some(STEEL_LIT);
        g[17][x] = Some(if x == 7 { STEEL_DARK } else { STEEL });
    }
    g[18][5] = Some(STEEL_LIT);
    g[18][6] = Some(STEEL);
    for x in 3..9 {
        g[19][x] = Some(STEEL_DARK);
    }
    g[19][4] = Some(STEEL_LIT);
    g[19][7] = Some(STEEL_LIT);
}

/// Where in its period a tube is: `seed` shifts it so tubes stay apart.
fn beat(frame: u64, seed: u64, period: u64) -> u64 {
    frame.wrapping_add(seed % period) % period
}

/// Idle: steady, but once a period it drops to dim for a tick and then a
/// stretch of it cuts out for the next.
fn idle_lit(frame: u64, seed: u64, top: usize, bot: usize) -> impl Fn(usize) -> Lit {
    let k = beat(frame, seed, IDLE_PERIOD);
    let span = bot - top;
    let cut = top + (hash(seed, frame / IDLE_PERIOD) as usize) % (span - span / 3);
    move |y| match k {
        0 => Lit::Dim,
        1 if (cut..cut + span / 3).contains(&y) => Lit::Dead,
        _ => Lit::On,
    }
}

/// A tube's texels at animation frame `frame` (the tick count).
pub fn texels(state: Torch, frame: u64, seed: u64) -> Grid {
    let mut g: Grid = [[None; W]; H];
    bracket(&mut g);
    let (top, bot) = (3, 16);
    match state {
        Torch::Idle => glass(&mut g, 4, top, bot, CYAN, idle_lit(frame, seed, top, bot)),
        Torch::Working => {
            // Buzzing: hot and lit swap every tick, a random band runs hot
            // on the lit ticks, and sparks jump off the caps.
            let hot = (frame.wrapping_add(seed)) % 2 == 0;
            let r = hash(seed, frame);
            let band = top + (r as usize) % (bot - top - 3);
            glass(&mut g, 4, top, bot, MAGENTA, |y| {
                if hot || (band..band + 3).contains(&y) {
                    Lit::Hot
                } else {
                    Lit::On
                }
            });
            for i in 0..3 {
                let s = hash(r, 100 + i);
                if s % 3 == 0 {
                    continue;
                }
                let x = [1, 2, 9, 10][(s >> 8) as usize % 4];
                let y = [0, 2, 3, 15, 16, 17][(s >> 16) as usize % 6];
                g[y][x] = Some(if s % 2 == 0 {
                    MAGENTA.core
                } else {
                    MAGENTA.bright
                });
            }
        }
        Torch::NeedsYou => {
            // Blinking: blazing, then dim, every tick.
            let on = (frame.wrapping_add(seed)) % 2 == 0;
            glass(&mut g, 4, top, bot, AMBER, |_| {
                if on {
                    Lit::Hot
                } else {
                    Lit::Dim
                }
            });
        }
        Torch::Stopped => {
            // Dead glass; once a period a stretch sputters faintly for a
            // tick, then a single texel glows as it dies.
            let k = beat(frame, seed, DEAD_PERIOD);
            let at = top + (hash(seed, frame / DEAD_PERIOD) as usize) % (bot - top - 5);
            glass(&mut g, 4, top, bot, CYAN, |_| Lit::Dead);
            match k {
                0 => {
                    for y in at..at + 5 {
                        g[y][5] = Some(CYAN.glow);
                        g[y][6] = Some(CYAN.glow);
                    }
                }
                1 => g[at + 2][5] = Some(CYAN.glow),
                _ => {}
            }
        }
    }
    g
}

/// The standing tube: a tall cyan tube on a post with two bands and a foot,
/// flickering as Idle does.
pub fn tall_texels(frame: u64, seed: u64) -> TallGrid {
    let mut g: TallGrid = [[None; TALL_W]; TALL_H];
    for x in 2..6 {
        g[0][x] = Some(if x == 2 { STEEL_DARK } else { STEEL_LIT });
        g[1][x] = Some(STEEL);
        g[28][x] = Some(STEEL_LIT);
        g[29][x] = Some(if x == 5 { STEEL_DARK } else { STEEL });
    }
    let (top, bot) = (2, 28);
    glass(&mut g, 2, top, bot, CYAN, idle_lit(frame, seed, top, bot));
    for row in g.iter_mut().take(TALL_H - 2).skip(30) {
        row[3] = Some(STEEL_LIT);
        row[4] = Some(STEEL);
    }
    for y in [32, 35] {
        for x in 2..6 {
            g[y][x] = Some(STEEL_DARK);
        }
    }
    for y in TALL_H - 2..TALL_H {
        for x in 1..7 {
            g[y][x] = Some(if y == TALL_H - 1 { STEEL_DARK } else { STEEL });
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::torch::{runs, seed};
    use std::collections::HashSet;

    const STATES: [Torch; 4] = [Torch::Idle, Torch::Working, Torch::NeedsYou, Torch::Stopped];
    const NAMES: [&str; 4] = ["a", "pane", "lead-7", "chat-decor-wall-3"];

    #[test]
    fn buzzing_and_blinking_tubes_change_on_every_tick() {
        for s in [Torch::Working, Torch::NeedsYou] {
            for name in NAMES {
                for f in 0..96 {
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
    fn no_tube_holds_one_pattern_for_48_ticks() {
        for s in STATES {
            for name in NAMES {
                for start in [0u64, 7, 33, 1000] {
                    let seen: HashSet<_> = (start..start + 48)
                        .map(|f| texels(s, f, seed(name)))
                        .collect();
                    assert!(seen.len() > 1, "{s:?} {name} from {start}");
                }
            }
        }
        for start in [0u64, 7, 33, 1000] {
            let seen: HashSet<_> = (start..start + 48)
                .map(|f| tall_texels(f, seed("post")))
                .collect();
            assert!(seen.len() > 1, "standing tube from {start}");
        }
    }

    #[test]
    fn idle_and_dead_tubes_mostly_hold_still() {
        // Rare means rare: a calm tube shows its steady pose on most ticks.
        for name in NAMES {
            let sd = seed(name);
            for (s, period) in [(Torch::Idle, IDLE_PERIOD), (Torch::Stopped, DEAD_PERIOD)] {
                let steady = (0..period * 4)
                    .filter(|&f| texels(s, f, sd) == texels(s, f + 2, sd))
                    .count() as u64;
                assert!(steady >= period * 4 * 3 / 4, "{s:?} {name}: {steady}");
            }
        }
    }

    #[test]
    fn decorative_tubes_flicker_out_of_step() {
        for period in [IDLE_PERIOD, DEAD_PERIOD] {
            let phases: HashSet<u64> = crate::app::chat::decor::SEEDS
                .iter()
                .map(|n| seed(n) % period)
                .collect();
            assert_eq!(phases.len(), crate::app::chat::decor::SEEDS.len());
        }
    }

    #[test]
    fn each_state_has_its_own_color_and_a_dead_tube_is_dark() {
        let has = |g: &Grid, h: Hue| {
            g.iter()
                .flatten()
                .any(|c| matches!(c, Some(c) if *c == h.bright || *c == h.mid))
        };
        for f in 0..8 {
            let sd = seed("a");
            let idle = texels(Torch::Idle, f, sd);
            assert!(has(&idle, CYAN) && !has(&idle, MAGENTA) && !has(&idle, AMBER));
            let work = texels(Torch::Working, f, sd);
            assert!(has(&work, MAGENTA) && !has(&work, CYAN));
            let ask = texels(Torch::NeedsYou, f, sd);
            assert!(has(&ask, AMBER) && !has(&ask, MAGENTA));
            let dead = texels(Torch::Stopped, f, sd);
            assert!(!has(&dead, CYAN) && !has(&dead, MAGENTA) && !has(&dead, AMBER));
        }
    }

    #[test]
    fn a_tube_frame_costs_a_few_dozen_quads_at_most() {
        for s in STATES {
            for f in 0..48 {
                let n = runs(&texels(s, f, seed("pane")));
                assert!(n <= 80, "{s:?} frame {f}: {n} quads");
            }
        }
    }
}
