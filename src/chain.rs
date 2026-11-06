//! The iron chain that frames the selected chat on the castle skin: face-on
//! ring links alternating with edge-on bars, a ring at each corner. Drawn
//! like the torches (`crate::torch`): hard texels, one quad per horizontal
//! run of one color, in one canvas, so it adds no elements per pixel. It is
//! static and paints only when its row does.

use gpui::{canvas, point, px, size, Bounds, Hsla, IntoElement, Pixels, Styled, Window};

/// Points per texel, as the torches' chat marks use.
const SCALE: f32 = 2.;
/// A ring's long side, the corner ring's side and every link's thickness.
const RING: usize = 7;
const CORNER: usize = 3;
const THICK: usize = 3;
/// How far a bar's tips reach over the rings it joins.
const TUCK: usize = 2;
/// A bar's visible span between two rings, texels.
const GAP: usize = 3;

const BODY: u32 = 0x7A756D;
const LIT: u32 = 0xC4BEB0;
const DARK: u32 = 0x3C3935;
const HOLE: u32 = 0x0E0D0C;

/// A link's texels, row by row; `.` is clear.
type Sprite = Vec<Vec<char>>;

fn color(c: char) -> Option<u32> {
    match c {
        'i' => Some(BODY),
        'L' => Some(LIT),
        'd' => Some(DARK),
        'o' => Some(HOLE),
        _ => None,
    }
}

fn rows(lines: &[&str]) -> Sprite {
    lines.iter().map(|l| l.chars().collect()).collect()
}

fn transpose(s: &Sprite) -> Sprite {
    (0..s[0].len())
        .map(|x| s.iter().map(|r| r[x]).collect())
        .collect()
}

/// A ring seen face-on, lying along a horizontal run.
fn ring() -> Sprite {
    rows(&[".iLiii.", "Loooooi", ".ddddd."])
}

fn corner() -> Sprite {
    rows(&["Lii", "ioi", "iid"])
}

/// An edge-on link `span` texels long between two rings. Its tips reach
/// `TUCK` texels over each ring: the ring's wall stays on top and the tip
/// shows in the hole beyond it, so the bar reads as threaded through. A bar
/// at a run's end has no tip toward the corner, whose hole is one texel.
fn bar(span: usize, tip_l: bool, tip_r: bool) -> Sprite {
    let flat = |c: char| -> Vec<char> {
        let mut r = vec!['.'; TUCK];
        r.extend(std::iter::repeat(c).take(span));
        r.extend(std::iter::repeat('.').take(TUCK));
        r
    };
    let tip = |on: bool| if on { 'i' } else { '.' };
    let mut top = flat('i');
    top[TUCK] = 'L';
    let mut mid = flat('i');
    mid[0] = tip(tip_l);
    mid[TUCK + span + 1] = tip(tip_r);
    vec![top, mid, flat('d')]
}

/// Where a run's links sit between its two corner rings: `rings` are the
/// start texels of the face-on rings, `bars` the (start, span) of each bar.
/// All whole texels, closing exactly: the first bar starts at 0, the last
/// ends at `len`.
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub rings: Vec<usize>,
    pub bars: Vec<(usize, usize)>,
}

/// Lay out a run `len` texels long, between the corners: rings with a bar
/// either side, as many as keep the gaps nearest `GAP`, the leftover spread
/// a texel at a time over the gaps so nothing is ever clipped.
pub fn layout(len: usize) -> Run {
    let m = ((len.saturating_sub(GAP)) as f32 / (RING + GAP) as f32).round() as usize;
    // Never so many that a gap drops under two texels.
    let m = m.min(len.saturating_sub(2) / (RING + 2));
    let gaps = m + 1;
    let free = len - RING * m;
    let (base, extra) = (free / gaps, free % gaps);
    let (mut rings, mut bars, mut at) = (Vec::new(), Vec::new(), 0);
    for i in 0..gaps {
        // Spread the extra texels evenly: gap `i` takes one when the running
        // share steps up.
        let span = base + usize::from((i + 1) * extra / gaps > i * extra / gaps);
        bars.push((at, span));
        at += span;
        if i < m {
            rings.push(at);
            at += RING;
        }
    }
    Run { rings, bars }
}

/// One painted rectangle, in texels.
#[derive(Debug, PartialEq, Eq)]
struct Quad {
    x: usize,
    y: usize,
    w: usize,
    color: u32,
}

fn put(out: &mut Vec<Quad>, s: &Sprite, ox: usize, oy: usize) {
    for (dy, row) in s.iter().enumerate() {
        let mut x = 0;
        while x < row.len() {
            let Some(c) = color(row[x]) else {
                x += 1;
                continue;
            };
            let start = x;
            while x < row.len() && row[x] == row[start] {
                x += 1;
            }
            out.push(Quad {
                x: ox + start,
                y: oy + dy,
                w: x - start,
                color: c,
            });
        }
    }
}

/// The whole frame's quads for a `w` by `h` texel row: rings and corners
/// first, then the bars over their ends.
fn quads(w: usize, h: usize) -> Vec<Quad> {
    let mut out = Vec::new();
    if w < 2 * CORNER + 2 || h < 2 * CORNER + 2 {
        return out;
    }
    let (across, down) = (layout(w - 2 * CORNER), layout(h - 2 * CORNER));
    let (ring_h, ring_v, c) = (ring(), transpose(&ring()), corner());
    for (x, y) in [
        (0, 0),
        (w - CORNER, 0),
        (0, h - CORNER),
        (w - CORNER, h - CORNER),
    ] {
        put(&mut out, &c, x, y);
    }
    for &r in &across.rings {
        put(&mut out, &ring_h, CORNER + r, 0);
        put(&mut out, &ring_h, CORNER + r, h - THICK);
    }
    for &r in &down.rings {
        put(&mut out, &ring_v, 0, CORNER + r);
        put(&mut out, &ring_v, w - THICK, CORNER + r);
    }
    // Bars last: their tips sit in the rings' holes, over the dark.
    let last = across.bars.len() - 1;
    for (i, &(at, span)) in across.bars.iter().enumerate() {
        let b = bar(span, i > 0, i < last);
        put(&mut out, &b, CORNER + at - TUCK, 0);
        put(&mut out, &b, CORNER + at - TUCK, h - THICK);
    }
    let last = down.bars.len() - 1;
    for (i, &(at, span)) in down.bars.iter().enumerate() {
        let b = transpose(&bar(span, i > 0, i < last));
        put(&mut out, &b, 0, CORNER + at - TUCK);
        put(&mut out, &b, w - THICK, CORNER + at - TUCK);
    }
    out
}

/// The chain over a selected row on the castle skin, else nothing. Absolute
/// over the whole row, so the row's contents stay where they were.
pub fn frame() -> Option<impl IntoElement> {
    (crate::skin::lamp() == Some(seance_core::schemes::Lamp::Torch)).then(|| {
        canvas(|_, _, _| {}, |bounds, _, window, _| paint(bounds, window))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
    })
}

fn paint(bounds: Bounds<Pixels>, window: &mut Window) {
    // Whole device-independent points, so the hard texels never blur.
    let o = bounds.origin;
    let (ox, oy) = (f32::from(o.x).round(), f32::from(o.y).round());
    let w = (f32::from(bounds.size.width) / SCALE) as usize;
    let h = (f32::from(bounds.size.height) / SCALE) as usize;
    for q in quads(w, h) {
        let r = Bounds::new(
            point(px(ox + q.x as f32 * SCALE), px(oy + q.y as f32 * SCALE)),
            size(px(q.w as f32 * SCALE), px(SCALE)),
        );
        window.paint_quad(gpui::fill(r, Hsla::from(gpui::rgb(q.color))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_closes_evenly_at_every_width() {
        // Texels for the narrowest sidebar (160 pt) up past the widest
        // (520 pt), and down to the shortest row a chat row takes (36 pt).
        for len in 12..=270 {
            let r = layout(len);
            assert_eq!(r.bars.len(), r.rings.len() + 1, "len {len}");
            // Bars and rings tile the run end to end, whole texels, no overlap.
            let mut at = 0;
            for (i, &(start, span)) in r.bars.iter().enumerate() {
                assert_eq!(start, at, "len {len} bar {i}");
                assert!((2..=6).contains(&span), "len {len} gap {span}");
                at += span;
                if let Some(&ring) = r.rings.get(i) {
                    assert_eq!(ring, at, "len {len} ring {i}");
                    at += RING;
                }
            }
            assert_eq!(at, len, "len {len} closes on the corner");
        }
    }

    #[test]
    fn the_frame_stays_inside_its_row_and_has_a_corner_link_each() {
        for (w, h) in [(80, 20), (116, 24), (260, 30), (45, 17)] {
            let q = quads(w, h);
            assert!(q.iter().all(|r| r.x + r.w <= w && r.y < h), "{w}x{h}");
            // A corner's top-left texel is lit; only corners put it at the
            // very edge in both axes.
            for (x, y) in [
                (0, 0),
                (w - CORNER, 0),
                (0, h - CORNER),
                (w - CORNER, h - CORNER),
            ] {
                assert!(
                    q.iter().any(|r| (r.x, r.y) == (x, y)),
                    "{w}x{h} corner {x},{y}"
                );
            }
            // The middle of the row is never painted.
            assert!(q
                .iter()
                .all(|r| r.y < THICK || r.y >= h - THICK || r.x < THICK || r.x >= w - THICK));
        }
    }

    #[test]
    fn a_tiny_row_draws_nothing_rather_than_clip() {
        assert!(quads(6, 30).is_empty() && quads(30, 6).is_empty());
    }
}
