//! The iron line around the selected chat on the castle skin: a thin solid
//! line with a small open ring on each corner. Drawn like the torches
//! (`crate::torch`): hard texels, one quad per horizontal run of one color,
//! in one canvas, so it adds no elements per pixel. It is static and paints
//! only when its row does.

use gpui::{canvas, point, px, size, Bounds, Hsla, IntoElement, Pixels, Styled, Window};

/// Points per texel.
const SCALE: f32 = 2.;
/// A corner ring's side, texels.
const RING: usize = 5;

const IRON: u32 = 0x75716A;
const LIT: u32 = 0xC4BEB0;
const HOLE: u32 = 0x0E0D0C;

/// The ring's texels, row by row: iron wall, a 3x3 hole, one lit texel.
const RING_ROWS: [&str; RING] = [".iLi.", "ioooi", "ioooi", "ioooi", ".iii."];

fn color(c: char) -> Option<u32> {
    match c {
        'i' => Some(IRON),
        'L' => Some(LIT),
        'o' => Some(HOLE),
        _ => None,
    }
}

/// One painted rectangle, in texels.
#[derive(Debug, PartialEq, Eq)]
struct Quad {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
}

/// The line's and rings' quads for a `w` by `h` texel row: four one-texel
/// runs along the edges, then a ring on each corner over them.
fn quads(w: usize, h: usize) -> Vec<Quad> {
    let mut out = Vec::new();
    if w < 2 * RING + 2 || h < 2 * RING + 2 {
        return out;
    }
    let q = |x, y, w, h| Quad {
        x,
        y,
        w,
        h,
        color: IRON,
    };
    out.push(q(0, 0, w, 1));
    out.push(q(0, h - 1, w, 1));
    out.push(q(0, 1, 1, h - 2));
    out.push(q(w - 1, 1, 1, h - 2));
    for (ox, oy) in [(0, 0), (w - RING, 0), (0, h - RING), (w - RING, h - RING)] {
        for (dy, row) in RING_ROWS.iter().enumerate() {
            let cells: Vec<char> = row.chars().collect();
            let mut x = 0;
            while x < RING {
                let Some(c) = color(cells[x]) else {
                    x += 1;
                    continue;
                };
                let start = x;
                while x < RING && cells[x] == cells[start] {
                    x += 1;
                }
                out.push(Quad {
                    x: ox + start,
                    y: oy + dy,
                    w: x - start,
                    h: 1,
                    color: c,
                });
            }
        }
    }
    out
}

/// The iron line over a selected row on the castle skin, else nothing.
/// Absolute over the whole row and inside it, so layout stays as it was.
pub fn frame() -> Option<impl IntoElement> {
    (!crate::carve::today() && crate::skin::lamp() == Some(seance_core::schemes::Lamp::Torch)).then(
        || {
            canvas(|_, _, _| {}, |bounds, _, window, _| paint(bounds, window))
                .absolute()
                .top_0()
                .left_0()
                .size_full()
        },
    )
}

fn paint(bounds: Bounds<Pixels>, window: &mut Window) {
    // Whole points, so the hard texels never blur.
    let o = bounds.origin;
    let (ox, oy) = (f32::from(o.x).round(), f32::from(o.y).round());
    let w = (f32::from(bounds.size.width) / SCALE) as usize;
    let h = (f32::from(bounds.size.height) / SCALE) as usize;
    for q in quads(w, h) {
        let r = Bounds::new(
            point(px(ox + q.x as f32 * SCALE), px(oy + q.y as f32 * SCALE)),
            size(px(q.w as f32 * SCALE), px(q.h as f32 * SCALE)),
        );
        window.paint_quad(gpui::fill(r, Hsla::from(gpui::rgb(q.color))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texel at (x, y), the last quad over it winning, as painted.
    fn texel(q: &[Quad], x: usize, y: usize) -> Option<u32> {
        q.iter()
            .rev()
            .find(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|r| r.color)
    }

    #[test]
    fn the_line_closes_around_the_row_with_a_ring_on_each_corner_at_every_size() {
        // Sidebar 160 to 520 pt wide, rows 36 to 90 pt tall, in texels.
        for w in 80..=260 {
            for h in 18..=45 {
                let q = quads(w, h);
                assert!(q.iter().all(|r| r.x + r.w <= w && r.y + r.h <= h));
                // Every texel along the four edges is iron, hole or lit
                // wall: the line never breaks.
                for x in 0..w {
                    assert!(texel(&q, x, 0).is_some() && texel(&q, x, h - 1).is_some());
                }
                for y in 0..h {
                    assert!(texel(&q, 0, y).is_some() && texel(&q, w - 1, y).is_some());
                }
                // A ring on each corner, flush with it, hole showing.
                for (ox, oy) in [(0, 0), (w - RING, 0), (0, h - RING), (w - RING, h - RING)] {
                    assert_eq!(texel(&q, ox + 2, oy + 2), Some(HOLE), "{w}x{h} {ox},{oy}");
                    assert_eq!(texel(&q, ox + 2, oy), Some(LIT));
                }
                // Nothing in the middle of the row.
                assert_eq!(texel(&q, w / 2, h / 2), None);
            }
        }
    }

    #[test]
    fn the_line_is_one_texel_thick_and_a_tiny_row_draws_nothing() {
        let q = quads(100, 30);
        // Between the rings the top edge is a single iron texel row.
        assert_eq!(texel(&q, 50, 0), Some(IRON));
        assert_eq!(texel(&q, 50, 1), None);
        assert!(quads(8, 30).is_empty() && quads(30, 8).is_empty());
    }
}
