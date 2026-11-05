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
            MOVES[mv][((now - start) / 240 % 4) as usize]
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
}
