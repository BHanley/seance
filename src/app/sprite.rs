//! A tiny pixel creature per circle, drawn on the circle's lead pane (its
//! title strip and Stage's lead card). Generated from the circle slug, so a
//! circle keeps its creature on every machine; `seance-lead-sprite.<slug> = SEED`
//! in terminal.conf swaps in another.
//!
//! 10x10 and mirror-symmetric like an identicon: a body silhouette, two dark
//! eyes inside it, legs on the bottom rows and sometimes ears or an antenna.

use gpui::{div, prelude::*, px, rgb, AnyElement};

pub(super) const SIZE: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cell {
    Empty,
    Body,
    Eye,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Sprite {
    pub cells: [[Cell; SIZE]; SIZE],
    /// Index into [`BODY`].
    pub color: usize,
    /// Where this creature is in its dance, so circles don't move in step.
    pub phase: u64,
}

/// Muted body colors that sit with the candlelit palette: sage, dusty blue,
/// mauve, sand, clay, teal, rose, slate.
const BODY: [u32; 8] = [
    0x8fa67a, 0x7a95b0, 0xa38bb0, 0xb8a27a, 0xb0846e, 0x6fa3a0, 0xb07f8a, 0x8a92a3,
];
const INK: u32 = 0x1b1c18;

/// FNV-1a: stable across runs and machines, unlike std's hasher.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// xorshift64, seeded from the hash.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// 0..n
    fn below(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }
}

/// The creature for `seed` (a circle slug, or its override).
pub(super) fn generate(seed: &str) -> Sprite {
    let mut rng = Rng(fnv1a(seed) | 1);
    let mut cells = [[Cell::Empty; SIZE]; SIZE];
    // Paint the left half (columns 0..5, centre at 4) and mirror it.
    let mut set = |row: usize, half: usize, cell: Cell| {
        cells[row][4 - half] = cell;
        cells[row][5 + half] = cell;
    };

    // Body: rows 2..8, a half-width of 2..=4 that drifts a step per row,
    // at least 3 on the eye rows so the eyes sit inside it.
    let mut widths = [0usize; SIZE];
    let mut w = 2 + rng.below(3);
    for (row, slot) in widths.iter_mut().enumerate().take(8).skip(2) {
        *slot = if matches!(row, 3 | 4) { w.max(3) } else { w };
        for half in 0..*slot {
            set(row, half, Cell::Body);
        }
        w = (w as i64 + rng.below(3) as i64 - 1).clamp(2, 4) as usize;
    }

    // Eyes on row 3 or 4, with body on both sides of each.
    let eye_row = 3 + rng.below(2);
    let eye = 1 + rng.below(widths[eye_row] as u64 - 2);
    set(eye_row, eye, Cell::Eye);

    // Top: nothing, ears at the top row's edge, or an antenna in the middle.
    match rng.below(3) {
        1 => {
            set(1, widths[2] - 1, Cell::Body);
            if rng.below(2) == 0 {
                set(0, widths[2] - 1, Cell::Body);
            }
        }
        2 => {
            set(1, 0, Cell::Body);
            set(0, rng.below(2), Cell::Body);
        }
        _ => {}
    }

    // Legs under the body, with feet that sometimes turn out.
    let leg = 1 + rng.below(widths[7] as u64 - 1);
    set(8, leg, Cell::Body);
    set(9, leg, Cell::Body);
    if rng.below(2) == 0 {
        set(9, leg + 1, Cell::Body);
    }

    Sprite {
        cells,
        color: rng.below(BODY.len() as u64),
        phase: fnv1a(seed),
    }
}

/// Dance moves while the circle works, as (x, y) cell offsets per beat. The
/// first is the basic step (rest, hop left, rest, hop right); every few bars
/// a creature throws in one of the others at random (Brian's ask).
const MOVES: [[(f32, f32); 4]; 5] = [
    [(0., 0.), (-1., -1.), (0., 0.), (1., -1.)],
    // jump
    [(0., 0.), (0., -1.), (0., -2.), (0., -1.)],
    // shimmy
    [(-1., 0.), (1., 0.), (-1., 0.), (1., 0.)],
    // slide left and back
    [(-1., 0.), (-2., 0.), (-1., 0.), (0., 0.)],
    // bounce
    [(0., -1.), (0., 0.), (0., -1.), (0., 0.)],
];

/// This creature's offset at beat `beat`: two-beat bars of the basic step,
/// with a random move in about one bar in four.
fn dance_step(phase: u64, beat: u64) -> (f32, f32) {
    let beat = beat.wrapping_add(phase);
    let bar = beat / 4;
    let roll = (bar ^ phase).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 60;
    let mv = if roll < 4 {
        1 + (roll as usize) % (MOVES.len() - 1)
    } else {
        0
    };
    MOVES[mv][(beat % 4) as usize]
}

/// Poking the creature (a click on it in Chat's header): one dance move and a
/// line each time, and five pokes within [`POKE_WINDOW_MS`] throw a party.
pub(super) const DANCE_MS: u64 = 960;
pub(super) const LINE_MS: u64 = 2400;
pub(super) const PARTY_MS: u64 = 5000;
const POKE_WINDOW_MS: u64 = 3000;
const PARTY_POKES: usize = 5;
pub(super) const PARTY_LINE: &str = "A seance party! The spirits are out.";
const LINES: [&str; 10] = [
    "Who summoned me?",
    "The spirits say: ship it.",
    "Knock once for yes.",
    "I saw nothing.",
    "Boo.",
    "The lamp is warm tonight.",
    "Your tokens are in another castle.",
    "I only dance when you're not looking.",
    "Have you tried /compact?",
    "The veil is thin. So is the XPS's RAM.",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Poke {
    /// Play `MOVES[mv]` and say `line`.
    Dance { mv: usize, line: &'static str },
    /// Every creature dances for [`PARTY_MS`].
    Party,
}

/// Recent pokes (unix ms).
#[derive(Default)]
pub(super) struct Pokes(Vec<u64>);

impl Pokes {
    pub(super) fn poke(&mut self, now: u64) -> Poke {
        self.0.retain(|t| now.saturating_sub(*t) <= POKE_WINDOW_MS);
        self.0.push(now);
        if self.0.len() >= PARTY_POKES {
            self.0.clear();
            return Poke::Party;
        }
        let mut rng = Rng(now.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        Poke::Dance {
            mv: rng.below(MOVES.len() as u64),
            line: LINES[rng.below(LINES.len() as u64)],
        }
    }
}

/// The party's pixel spirits: this many, each alive for [`SPIRIT_LIFE_MS`],
/// starting one after another over the party.
pub(super) const SPIRITS: usize = 40;
const SPIRIT_LIFE_MS: u64 = 2500;

/// Where one spirit is, `t` ms into the party: its offset in points from the
/// creature, its size, how opaque it is, and whether it takes the accent
/// color (else the creature's own).
pub(super) struct Spirit {
    pub dx: f32,
    pub dy: f32,
    pub size: f32,
    pub alpha: f32,
    pub accent: bool,
}

/// None before spirit `i` is out, and after it has faded.
pub(super) fn spirit_at(i: usize, t: u64) -> Option<Spirit> {
    let delay = i as u64 * (PARTY_MS - SPIRIT_LIFE_MS) / SPIRITS as u64;
    let age = t.checked_sub(delay).filter(|a| *a < SPIRIT_LIFE_MS)?;
    let mut rng = Rng((i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let secs = age as f32 / 1000.;
    // Out and down into the view with a little sway, like a drifting ghost.
    let vx = rng.below(280) as f32 - 140.;
    let vy = 20. + rng.below(170) as f32;
    let sway = (secs * 5. + i as f32).sin() * 10.;
    Some(Spirit {
        dx: vx * secs + sway,
        dy: vy * secs,
        size: 4. + rng.below(4) as f32,
        alpha: 1. - age as f32 / SPIRIT_LIFE_MS as f32,
        accent: i % 2 == 0,
    })
}

/// The creature's own body color.
pub(super) fn body_color(sprite: &Sprite) -> gpui::Hsla {
    rgb(BODY[sprite.color]).into()
}

/// When the party ends (unix ms); every creature dances until then.
static PARTY_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn start_party(now: u64) {
    PARTY_UNTIL.store(now + PARTY_MS, std::sync::atomic::Ordering::Relaxed);
}

fn partying(now: u64) -> bool {
    now < PARTY_UNTIL.load(std::sync::atomic::Ordering::Relaxed)
}

/// The sprite as a grid of `cell`-point squares. Whole device pixels keep it
/// crisp: 1.5 and 2.5 points land on 3 and 5 pixels on a 2x screen.
/// `dancing` steps it through [`MOVES`] on the wall clock; the sidebar spinner's
/// 240ms tick already re-renders while any circle works, so it costs no frames.
pub(super) fn render(sprite: &Sprite, cell: f32, dancing: bool) -> AnyElement {
    render_move(sprite, cell, dancing, None)
}

/// [`render`], and with `poked` (a move and when it began) the creature plays
/// that move once, on any circle, working or not. A party dances them all.
pub(super) fn render_move(
    sprite: &Sprite,
    cell: f32,
    dancing: bool,
    poked: Option<(usize, u64)>,
) -> AnyElement {
    let body = rgb(BODY[sprite.color]);
    let ink = rgb(INK);
    let now = super::util::now_ms();
    let (dx, dy) = match poked {
        Some((mv, start)) if now.saturating_sub(start) < DANCE_MS => {
            MOVES[mv][(now.saturating_sub(start) / 240 % 4) as usize]
        }
        _ if dancing || partying(now) => dance_step(sprite.phase, now / 240),
        _ => (0., 0.),
    };
    div()
        .flex_none()
        .relative()
        .left(px(dx * cell))
        .top(px(dy * cell))
        .flex()
        .flex_col()
        .children(sprite.cells.iter().map(|row| {
            div().flex().children(row.iter().map(|c| {
                let d = div().flex_none().size(px(cell));
                match c {
                    Cell::Empty => d,
                    Cell::Body => d.bg(body),
                    Cell::Eye => d.bg(ink),
                }
            }))
        }))
        .into_any_element()
}

/// Two or more panes working dance as a crew (Brian's ask): one paper cutout
/// per pane, the same creature in its own shade, circling on a low ring. gpui
/// has no 3D, so this fakes the web Chat's CSS ring with its numbers. Cutout
/// cells are 2 points to its 2px, so its px read as points here.
const CREW_CELL: f32 = 2.;
const CREW_MAX: usize = 6;
/// A step toward white (positive) or black per pane index, so a pane keeps
/// its shade wherever it is in the crew. 0 is the circle's own color.
const CREW_SHADES: [f32; CREW_MAX] = [0., 0.3, -0.3, 0.55, -0.5, 0.75];
const ORBIT_MS: u64 = 5000;
const SHEET_MS: u64 = 1700;
/// Each sheet runs this far ahead of the one before it.
const SHEET_STAGGER_MS: u64 = 450;
const TILT_DEG: f32 = 20.;
const SWAY_DEG: f32 = 40.;
const HOP: f32 = 3.;
const PERSPECTIVE: f32 = 170.;

/// One cutout of the crew at a moment.
#[derive(Debug, PartialEq)]
pub(super) struct Cutout {
    pub pane: u32,
    /// Its centre from the ring's on screen, in points (y down), perspective
    /// and hop included.
    pub x: f32,
    pub y: f32,
    /// 1 at the front of the ring, -1 at the back.
    pub near: f32,
    /// Perspective: above 1 in front, below behind.
    pub scale: f32,
    /// How far the sheet is turned, in radians; it squeezes by the cosine.
    pub sway: f32,
    /// Its [`CREW_SHADES`] step.
    pub shade: f32,
}

fn crew_radius(n: usize) -> f32 {
    (4.2 * n as f32).max(15.)
}

/// Whether these working panes (indexes) make a crew: two or more.
pub(super) fn is_crew(crew: &[u32]) -> bool {
    crew.len() >= 2
}

/// The first [`CREW_MAX`] of `crew` (pane indexes) at `t` (unix ms), back to
/// front so nearer cutouts are drawn over farther ones. Empty under two.
pub(super) fn crew_at(crew: &[u32], t: u64) -> Vec<Cutout> {
    if !is_crew(crew) {
        return Vec::new();
    }
    let crew = &crew[..crew.len().min(CREW_MAX)];
    let n = crew.len();
    let r = crew_radius(n);
    let tau = std::f32::consts::TAU;
    let (tilt_sin, tilt_cos) = TILT_DEG.to_radians().sin_cos();
    let mut cuts: Vec<Cutout> = crew
        .iter()
        .enumerate()
        .map(|(i, &pane)| {
            // Evenly spaced on the ring, which the tilt shows from above.
            let a = tau * ((t % ORBIT_MS) as f32 / ORBIT_MS as f32 + i as f32 / n as f32);
            let (x, z) = (r * a.sin(), r * a.cos());
            let scale = PERSPECTIVE / (PERSPECTIVE - z * tilt_cos);
            // Sway side to side, hopping at each upright pass.
            let u = ((t + i as u64 * SHEET_STAGGER_MS) % SHEET_MS) as f32 / SHEET_MS as f32;
            let sway = -SWAY_DEG.to_radians() * (tau * u).cos();
            let hop = -HOP * (tau * u).sin().powi(2);
            Cutout {
                pane,
                x: x * scale,
                y: (z * tilt_sin + hop) * scale,
                near: a.cos(),
                scale,
                sway,
                shade: CREW_SHADES[pane as usize % CREW_MAX],
            }
        })
        .collect();
    cuts.sort_by(|a, b| a.near.total_cmp(&b.near));
    cuts
}

/// `color` a `step` of the way to white (positive) or black (negative).
fn shade_rgb(color: u32, step: f32) -> u32 {
    let to = if step > 0. { 255. } else { 0. };
    [16, 8, 0].iter().fold(0, |out, shift| {
        let v = ((color >> shift) & 0xff) as f32;
        out << 8 | (v + (to - v) * step.abs()).round() as u32
    })
}

/// The header's crew from [`crew_at`]'s `cuts`. `poked` moves the whole ring
/// through that move, as [`render_move`] does one creature.
pub(super) fn render_crew(
    sprite: &Sprite,
    cuts: &[Cutout],
    poked: Option<(usize, u64)>,
) -> AnyElement {
    // Half points are whole device pixels on a 2x screen, so cells meet
    // without seams however the sheet is squeezed.
    let snap = |v: f32| (v * 2.).round() / 2.;
    // As tall as the lone creature, so the header keeps its height.
    let height = SIZE as f32 * 2.5;
    let width = 2. * crew_radius(cuts.len()) + SIZE as f32 * CREW_CELL;
    let now = super::util::now_ms();
    let (dx, dy) = match poked {
        Some((mv, start)) if now.saturating_sub(start) < DANCE_MS => {
            MOVES[mv][(now.saturating_sub(start) / 240 % 4) as usize]
        }
        _ => (0., 0.),
    };
    let ink = gpui::Hsla::from(rgb(INK));
    let mut cells = Vec::new();
    for cut in cuts {
        let body = gpui::Hsla::from(rgb(shade_rgb(BODY[sprite.color], cut.shade)));
        // Farther cutouts a little darker, for depth.
        let body = gpui::Hsla {
            l: body.l * (0.9 + 0.1 * cut.near),
            ..body
        };
        let (cw, ch) = (
            CREW_CELL * cut.scale * cut.sway.cos(),
            CREW_CELL * cut.scale,
        );
        let left = width / 2. + cut.x - cw * SIZE as f32 / 2.;
        let top = height / 2. + cut.y - ch * SIZE as f32 / 2.;
        let col = |k: usize| snap(left + k as f32 * cw);
        let row = |k: usize| snap(top + k as f32 * ch);
        // The sheet's thin dark edge, right of each run of cells.
        let edge = snap(cw * 0.4).max(0.5);
        for (y, line) in sprite.cells.iter().enumerate() {
            let at = |l: f32, w: f32| {
                div()
                    .absolute()
                    .left(px(l))
                    .top(px(row(y)))
                    .w(px(w))
                    .h(px(row(y + 1) - row(y)))
            };
            for (x, c) in line.iter().enumerate() {
                let fill = match c {
                    Cell::Empty => continue,
                    Cell::Body => body,
                    Cell::Eye => ink,
                };
                cells.push(at(col(x), col(x + 1) - col(x)).bg(fill));
                if line.get(x + 1).is_none_or(|n| *n == Cell::Empty) {
                    cells.push(at(col(x + 1), edge).bg(gpui::black().opacity(0.5)));
                }
            }
        }
    }
    div()
        .flex_none()
        .relative()
        .left(px(dx * 2.5))
        .top(px(dy * 2.5))
        .w(px(width))
        .h(px(height))
        .children(cells)
        .into_any_element()
}

/// A circle's creature, honoring its `seance-lead-sprite.<slug>` override.
pub(super) fn for_circle(slug: &str) -> Sprite {
    let conf = crate::term_config::get();
    generate(conf.lead_sprites.get(slug).map_or(slug, String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dance_mixes_in_moves_and_circles_differ() {
        let steps = |phase| (0..400).map(|b| dance_step(phase, b)).collect::<Vec<_>>();
        let a = steps(generate("l6-leveling").phase);
        let b = steps(generate("seance-demo").phase);
        assert_ne!(a, b);
        // Mostly the basic step, with some other moves thrown in.
        let basic = a.chunks(4).filter(|bar| bar == &MOVES[0]).count();
        assert!(basic > 40 && basic < 100, "basic bars: {basic}");
        // Never further than two cells from home.
        assert!(a.iter().all(|(x, y)| x.abs() <= 2. && y.abs() <= 2.));
    }

    #[test]
    fn same_slug_same_creature() {
        assert_eq!(generate("l6-leveling"), generate("l6-leveling"));
    }

    #[test]
    fn different_slugs_differ() {
        let slugs = [
            "main",
            "l6-leveling",
            "circle-2",
            "profile",
            "3d-maps",
            "personal",
        ];
        let sprites: Vec<_> = slugs.iter().map(|s| generate(s)).collect();
        for (i, a) in sprites.iter().enumerate() {
            for b in &sprites[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn symmetric_with_two_eyes_inside_the_body() {
        for slug in [
            "main",
            "l6-leveling",
            "circle-2",
            "a",
            "zz-top",
            "x".repeat(40).as_str(),
        ] {
            let s = generate(slug);
            let mut eyes = 0;
            for (r, row) in s.cells.iter().enumerate() {
                for c in 0..SIZE {
                    assert_eq!(row[c], row[SIZE - 1 - c], "{slug} row {r}");
                    if row[c] == Cell::Eye {
                        eyes += 1;
                        assert_eq!(row[c - 1], Cell::Body, "{slug}: eye has body beside it");
                    }
                }
            }
            assert_eq!(eyes, 2, "{slug}");
            assert!(s.cells[9].contains(&Cell::Body), "{slug}: has legs");
        }
    }

    #[test]
    fn five_pokes_in_three_seconds_throw_a_party() {
        let mut p = Pokes::default();
        for i in 0..4 {
            assert!(matches!(p.poke(1_000 + i * 500), Poke::Dance { mv, .. } if mv < MOVES.len()));
        }
        assert_eq!(p.poke(3_000), Poke::Party);
        // The count starts over after a party.
        assert!(matches!(p.poke(3_100), Poke::Dance { .. }));
    }

    #[test]
    fn slow_pokes_never_party() {
        let mut p = Pokes::default();
        for i in 0..20 {
            assert!(matches!(p.poke(i * 1_000), Poke::Dance { .. }));
        }
    }

    #[test]
    fn every_line_comes_up_and_the_party_runs_five_seconds() {
        let mut seen = std::collections::HashSet::new();
        for i in 0..400 {
            if let Poke::Dance { line, .. } = Pokes::default().poke(i * 7_919) {
                seen.insert(line);
            }
        }
        assert_eq!(seen.len(), LINES.len());
        start_party(10_000);
        assert!(partying(10_000 + PARTY_MS - 1));
        assert!(!partying(10_000 + PARTY_MS));
    }

    #[test]
    fn crew_needs_two_and_stops_at_six() {
        assert!(crew_at(&[], 0).is_empty());
        assert!(crew_at(&[4], 0).is_empty());
        assert_eq!(crew_at(&[0, 1], 0).len(), 2);
        let big: Vec<u32> = (0..9).collect();
        let mut panes: Vec<u32> = crew_at(&big, 0).iter().map(|c| c.pane).collect();
        panes.sort();
        assert_eq!(panes, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn crew_draws_back_to_front_as_it_turns() {
        for t in (0..ORBIT_MS).step_by(137) {
            let cuts = crew_at(&[0, 1, 2, 3, 4], t);
            assert!(cuts.windows(2).all(|w| w[0].near <= w[1].near), "t={t}");
            // Nearer is bigger and lower, as seen from above the ring.
            let (back, front) = (&cuts[0], &cuts[4]);
            assert!(front.scale > back.scale && front.y > back.y);
        }
        // Each one comes round to the front in turn.
        let front = |t| crew_at(&[0, 1, 2], t).last().unwrap().pane;
        let fronts: std::collections::HashSet<u32> =
            (0..ORBIT_MS).step_by(100).map(front).collect();
        assert_eq!(fronts.len(), 3);
    }

    #[test]
    fn a_pane_keeps_its_shade_wherever_it_is_in_the_crew() {
        let shade = |crew: &[u32], pane| {
            crew_at(crew, 1234)
                .into_iter()
                .find(|c| c.pane == pane)
                .unwrap()
                .shade
        };
        assert_eq!(shade(&[3, 1], 3), shade(&[1, 3], 3));
        assert_eq!(shade(&[0, 2, 3], 3), shade(&[3, 7], 3));
        assert_ne!(shade(&[1, 3], 1), shade(&[1, 3], 3));
        // Pane 0 wears the circle's own color.
        assert_eq!(shade(&[0, 1], 0), 0.);
        assert_eq!(shade_rgb(0x8fa67a, 0.), 0x8fa67a);
        assert_eq!(shade_rgb(0x808080, 1.), 0xffffff);
        assert_eq!(shade_rgb(0x808080, -1.), 0x000000);
    }

    #[test]
    fn spirits_come_out_in_turn_and_fade() {
        // None is out before the party starts or after it is over.
        assert!((0..SPIRITS).all(|i| spirit_at(i, PARTY_MS + 1).is_none()));
        assert!(spirit_at(SPIRITS - 1, 0).is_none());
        // The first is out at once, and every one is seen at some point.
        assert!(spirit_at(0, 0).is_some());
        for i in 0..SPIRITS {
            assert!((0..=PARTY_MS)
                .step_by(50)
                .any(|t| spirit_at(i, t).is_some()));
        }
        // It fades as it ages and keeps drifting away from the creature.
        let (a, b) = (spirit_at(0, 100).unwrap(), spirit_at(0, 2000).unwrap());
        assert!(a.alpha > b.alpha && b.alpha > 0.);
        assert!(b.dx.hypot(b.dy) > a.dx.hypot(a.dy));
    }
}
