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
    }
}

/// The sprite as a grid of `cell`-point squares. Whole device pixels keep it
/// crisp: 1.5 and 2.5 points land on 3 and 5 pixels on a 2x screen.
pub(super) fn render(sprite: &Sprite, cell: f32) -> AnyElement {
    let body = rgb(BODY[sprite.color]);
    let ink = rgb(INK);
    div()
        .flex_none()
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
}
