//! The iron chain that frames the selected chat on the castle skin: face-on
//! ring links alternating with edge-on bars, a ring at each corner. Drawn
//! like the torches (`crate::torch`): hard texels, one quad per horizontal
//! run of one color, in one canvas, so it adds no elements per pixel. It is
//! static and paints only when its row does.

use gpui::{canvas, point, px, size, Bounds, Hsla, IntoElement, Pixels, Styled, Window};

/// Points per texel: chunky enough that a ring and its hole read at 1x.
const SCALE: f32 = 3.;
/// Texels a link band hangs above and below the row, onto its neighbours'
/// padding. The band is `BAND` tall on the horizontal runs, so `BAND - OVER`
/// of it sits inside the row (6 pt, against the row's 8 pt padding).
const OVER: usize = 3;
const BAND: usize = 5;
/// The side runs stay inside the row's 12 pt side padding: 3 texels.
const SIDE: usize = 3;
/// Ring lengths along their run, texels.
const RING_H: usize = 8;
const RING_V: usize = 5;
/// How far a bar's tips reach over the rings it joins, texels: the ring's
/// wall (left clear) and what shows in the hole beyond it.
const TUCK_H: usize = 3;
const TUCK_V: usize = 2;
/// The gap between rings a run aims for, texels.
const GAP_H: usize = 4;
const GAP_V: usize = 2;

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

/// A ring seen face-on along a horizontal run: a 6x3 hole in a one-texel
/// wall.
fn ring_h() -> Sprite {
    rows(&[".iLiiii.", "Looooooi", "iooooood", "iooooood", ".dddddd."])
}

/// The corner ring.
fn corner() -> Sprite {
    rows(&[".iLi.", "Loooi", "ioood", "ioood", ".ddd."])
}

/// A narrower ring for the side runs, which have only 3 texels.
fn ring_v() -> Sprite {
    rows(&[".i.", "Loi", "ioi", "iod", ".d."])
}

/// A horizontal edge-on link: `span` texels between two rings across the
/// band's middle three rows, tips in each hole. A bar at a run's end has no
/// tip toward the corner, whose hole is small.
fn bar_h(span: usize, tip_l: bool, tip_r: bool) -> Sprite {
    let w = span + 2 * TUCK_H;
    let blank = || vec!['.'; w];
    let body = |c: char| {
        let mut r = blank();
        r[TUCK_H..TUCK_H + span].fill(c);
        r
    };
    let mut top = body('i');
    top[TUCK_H] = 'L';
    let mut mid = body('i');
    for k in 0..TUCK_H - 1 {
        if tip_l {
            mid[k] = 'i';
        }
        if tip_r {
            mid[w - 1 - k] = 'i';
        }
    }
    vec![blank(), top, mid, body('d'), blank()]
}

/// A vertical edge-on link, as [`bar_h`] but one texel across the hole.
fn bar_v(span: usize, tip_t: bool, tip_b: bool) -> Sprite {
    let h = span + 2 * TUCK_V;
    let mut rows = vec![vec!['.'; 3]; h];
    for row in &mut rows[TUCK_V..TUCK_V + span] {
        *row = vec!['L', 'i', 'd'];
    }
    if tip_t {
        rows[0][1] = 'i';
    }
    if tip_b {
        rows[h - 1][1] = 'i';
    }
    rows
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

/// Lay out a run `len` texels long, between the corners: rings of `ring`
/// texels with a bar either side, as many as keep the gaps nearest `gap`
/// (never under one texel), the leftover spread a texel at a time over the
/// gaps so nothing is ever clipped.
pub fn layout(len: usize, ring: usize, gap: usize) -> Run {
    let m = ((len.saturating_sub(gap)) as f32 / (ring + gap) as f32).round() as usize;
    let m = m.min(len.saturating_sub(1) / (ring + 1));
    let gaps = m + 1;
    let free = len - ring * m;
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
            at += ring;
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

/// The frame's quads for a `w` by `h` texel grid whose top and bottom
/// `OVER` rows hang outside the row: corner rings, face-on rings, then the
/// bars over their ends.
fn quads(w: usize, h: usize) -> Vec<Quad> {
    let mut out = Vec::new();
    let c = BAND;
    if w < 2 * c + 2 || h < 2 * c + 2 {
        return out;
    }
    let (across, down) = (
        layout(w - 2 * c, RING_H, GAP_H),
        layout(h - 2 * c, RING_V, GAP_V),
    );
    let (corner, ring_h, ring_v) = (corner(), ring_h(), ring_v());
    for (x, y) in [(0, 0), (w - c, 0), (0, h - c), (w - c, h - c)] {
        put(&mut out, &corner, x, y);
    }
    for &r in &across.rings {
        put(&mut out, &ring_h, c + r, 0);
        put(&mut out, &ring_h, c + r, h - BAND);
    }
    for &r in &down.rings {
        put(&mut out, &ring_v, 0, c + r);
        put(&mut out, &ring_v, w - SIDE, c + r);
    }
    // Bars last: their tips sit in the rings' holes, over the dark.
    let last = across.bars.len() - 1;
    for (i, &(at, span)) in across.bars.iter().enumerate() {
        let b = bar_h(span, i > 0, i < last);
        put(&mut out, &b, c + at - TUCK_H, 0);
        put(&mut out, &b, c + at - TUCK_H, h - BAND);
    }
    let last = down.bars.len() - 1;
    for (i, &(at, span)) in down.bars.iter().enumerate() {
        let b = bar_v(span, i > 0, i < last);
        put(&mut out, &b, 0, c + at - TUCK_V);
        put(&mut out, &b, w - SIDE, c + at - TUCK_V);
    }
    out
}

/// The chain over a selected row on the castle skin, else nothing. It hangs
/// `OVER` texels past the row's top and bottom and is drawn deferred, above
/// its neighbours; absolute, so layout stays as it was.
pub fn frame() -> Option<impl IntoElement> {
    (!crate::carve::today() && crate::skin::lamp() == Some(seance_core::schemes::Lamp::Torch)).then(
        || {
            let over = px(OVER as f32 * SCALE);
            gpui::deferred(
                canvas(|_, _, _| {}, |bounds, _, window, _| paint(bounds, window))
                    .absolute()
                    .top(-over)
                    .bottom(-over)
                    .left_0()
                    .right_0(),
            )
            .with_priority(1)
        },
    )
}

fn paint(bounds: Bounds<Pixels>, window: &mut Window) {
    // Whole points, so the hard texels never blur.
    let o = bounds.origin;
    let (ox, oy) = (f32::from(o.x).round(), f32::from(o.y).round());
    let w = (f32::from(bounds.size.width) / SCALE) as usize;
    // The row's own height is the bounds less what hangs above and below;
    // round it up so the bottom band's inner edge keeps its clearance.
    let row = (f32::from(bounds.size.height) - 2. * OVER as f32 * SCALE).max(0.);
    let h = (row / SCALE).ceil() as usize + 2 * OVER;
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

    fn closes(len: usize, ring: usize, gap: usize, lo: usize, hi: usize) {
        let r = layout(len, ring, gap);
        assert_eq!(r.bars.len(), r.rings.len() + 1, "len {len}");
        // Bars and rings tile the run end to end, whole texels, no overlap.
        let mut at = 0;
        for (i, &(start, span)) in r.bars.iter().enumerate() {
            assert_eq!(start, at, "len {len} bar {i}");
            assert!((lo..=hi).contains(&span), "len {len} gap {span}");
            at += span;
            if let Some(&pos) = r.rings.get(i) {
                assert_eq!(pos, at, "len {len} ring {i}");
                at += ring;
            }
        }
        assert_eq!(at, len, "len {len} closes on the corner");
    }

    #[test]
    fn a_run_closes_evenly_at_every_width() {
        // Across: the narrowest sidebar (160 pt) up past the widest (520 pt).
        for w in 53..=175 {
            closes(w - 2 * BAND, RING_H, GAP_H, 2, 8);
        }
        // Down: the shortest row (36 pt) to a tall one with a wrapped preview.
        for h in 36usize..=90 {
            let texels = h.div_ceil(3) + 2 * OVER;
            closes(texels - 2 * BAND, RING_V, GAP_V, 1, 5);
        }
    }

    #[test]
    fn the_frame_stays_inside_its_grid_and_has_a_corner_link_each() {
        for (w, h) in [(53, 18), (77, 22), (150, 30), (30, 24)] {
            let q = quads(w, h);
            assert!(q.iter().all(|r| r.x + r.w <= w && r.y < h), "{w}x{h}");
            for (x, y) in [(0, 0), (w - BAND, 0), (0, h - BAND), (w - BAND, h - BAND)] {
                assert!(
                    q.iter().any(|r| (r.x, r.y) == (x + 1, y)),
                    "{w}x{h} corner {x},{y}"
                );
            }
            // Nothing in the middle: inside the side bands and between the
            // horizontal ones.
            assert!(q
                .iter()
                .all(|r| r.y < BAND || r.y >= h - BAND || r.x < BAND || r.x + r.w > w - BAND));
        }
    }

    #[test]
    fn the_inside_of_the_band_leaves_room_for_the_rows_padding() {
        // 6 pt inside the row against 8 pt padding; sides 9 pt against 12.
        assert!(((BAND - OVER) as f32) * SCALE <= 6.);
        assert!((SIDE as f32) * SCALE <= 9.);
    }

    #[test]
    fn a_tiny_row_draws_nothing_rather_than_clip() {
        assert!(quads(8, 30).is_empty() && quads(30, 8).is_empty());
    }
}
