//! The iron chain that frames the selected chat on the castle skin: face-on
//! ring links alternating with edge-on bars, a ring at each corner. Drawn
//! like the torches (`crate::torch`): hard texels, one quad per horizontal
//! run of one color, in one canvas, so it adds no elements per pixel. It is
//! static and paints only when its row does.

use gpui::{canvas, point, px, size, Bounds, Hsla, IntoElement, Pixels, Styled, Window};

/// The chain's shape: all four sides, the top and bottom only (each end
/// hooked over a bolt), or all four in heavier iron.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))] // one is picked, in `OPTION`
pub enum ChainOption {
    Frame,
    Rails,
    Heavy,
}

const OPTION: ChainOption = ChainOption::Frame;

#[cfg(test)]
thread_local! {
    /// Shot tests: draw another option than `OPTION`.
    pub static PICK: std::cell::Cell<Option<ChainOption>> = const { std::cell::Cell::new(None) };
}

fn option() -> ChainOption {
    #[cfg(test)]
    if let Some(o) = PICK.with(|p| p.get()) {
        return o;
    }
    OPTION
}

/// A side at least this many texels long holds two bars and a ring, a
/// shorter one a single bar.
const SIDE_TWO: usize = 16;

fn color(c: char) -> Option<u32> {
    match c {
        'i' => Some(0x7A756D),
        'L' => Some(0xC4BEB0),
        'd' => Some(0x3C3935),
        'o' => Some(0x0E0D0C),
        // Heavy iron's brighter glint and its outline.
        'W' => Some(0xE8E2D2),
        'x' => Some(0x050404),
        _ => None,
    }
}

/// A link's texels, row by row; `.` is clear.
type Sprite = Vec<Vec<char>>;

fn rows(lines: &[&str]) -> Sprite {
    lines.iter().map(|l| l.chars().collect()).collect()
}

/// One option's iron, in texels.
struct Style {
    /// Points per texel.
    scale: f32,
    /// How far the band hangs above and below the row.
    over: usize,
    /// The top and bottom bands' height, and the corner rings'.
    band: usize,
    /// The side bands' width.
    side: usize,
    /// A ring's wall: a bar's tip clears it and shows in the hole.
    wall: usize,
    /// How far a bar reaches over the rings it joins, and how much of that
    /// shows in the hole: a side's slit is short, so its tips are shorter.
    tuck: usize,
    tip_v: usize,
    /// The gap between rings the top and bottom runs aim for.
    gap: usize,
    /// Face-on rings: along the top and bottom (the corner rings too, each
    /// holding a side's bar by its outer end), and midway down a long side.
    ring: Sprite,
    ring_v: Sprite,
    /// A bar's cross-section from its top (or left) edge, and the band row
    /// it starts on; the middle row carries the tips.
    bar: Vec<char>,
    bar_at: usize,
    /// No sides: each run's end ring hangs on this bolt instead.
    bolt: Option<Sprite>,
}

fn style(o: ChainOption) -> Style {
    let frame = Style {
        scale: 3.,
        over: 3,
        band: 5,
        side: 3,
        wall: 1,
        tuck: 3,
        tip_v: 1,
        gap: 4,
        ring: rows(&[".iLiiii.", "Looooooi", "iooooood", "iooooood", ".dddddd."]),
        ring_v: rows(&[".L.", "Loi", "ioi", "iod", "iod", ".d."]),
        bar: vec!['i', 'i', 'd'],
        bar_at: 1,
        bolt: None,
    };
    match o {
        ChainOption::Frame => frame,
        ChainOption::Rails => Style {
            bolt: Some(rows(&[".Wi.", "WLid", "Lidx", ".dx."])),
            ..frame
        },
        ChainOption::Heavy => Style {
            scale: 2.,
            over: 4,
            band: 7,
            side: 5,
            wall: 2,
            tuck: 4,
            tip_v: 1,
            gap: 5,
            ring: rows(&[
                ".LWWWWWWWWWWi.",
                "LWiiiiiiiiiiid",
                "Wioooooooooodx",
                "Lioooooooooodx",
                "iioooooooooodx",
                ".dddddddddddx.",
                "..xxxxxxxxxx..",
            ]),
            ring_v: rows(&[
                ".WWi.", "Wiiid", "Wiodx", "Liodx", "iiodx", "iiodx", "iiodx", ".ddx.", "..x..",
            ]),
            bar: vec!['x', 'W', 'i', 'd', 'x'],
            bar_at: 1,
            bolt: None,
        },
    }
}

/// A horizontal edge-on link: `span` texels between two rings, a `tip`
/// texels long in each hole.
fn bar_h(st: &Style, span: usize, tip: usize) -> Sprite {
    let w = span + 2 * st.tuck;
    let mut out = vec![vec!['.'; w]; st.band];
    for (k, &c) in st.bar.iter().enumerate() {
        out[st.bar_at + k][st.tuck..st.tuck + span].fill(c);
    }
    let top = &mut out[st.bar_at][st.tuck];
    if *top == 'i' {
        *top = 'L';
    }
    let mid = &mut out[st.bar_at + st.bar.len() / 2];
    // A tip shows just past the wall, in the hole's near side.
    for k in st.tuck - st.wall - tip..st.tuck - st.wall {
        mid[k] = 'i';
        mid[w - 1 - k] = 'i';
    }
    out
}

/// A vertical edge-on link, [`bar_h`] turned on its side.
fn bar_v(st: &Style, span: usize) -> Sprite {
    let h = bar_h(st, span, st.tip_v);
    (0..h[0].len())
        .map(|y| (0..st.side).map(|x| h[st.bar_at + x][y]).collect())
        .collect()
}

/// Where a run's links sit between its two end rings: `rings` are the
/// start texels of the face-on rings, `bars` the (start, span) of each bar.
/// All whole texels, closing exactly: the first bar starts at 0, the last
/// ends at `len`.
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub rings: Vec<usize>,
    pub bars: Vec<(usize, usize)>,
}

/// Lay out a run `len` texels long, between its end rings: rings of `ring`
/// texels with a bar either side, as many as keep the gaps nearest `gap`
/// (never under one texel).
pub fn layout(len: usize, ring: usize, gap: usize) -> Run {
    let m = ((len.saturating_sub(gap)) as f32 / (ring + gap) as f32).round() as usize;
    spread(len, ring, m.min(len.saturating_sub(1) / (ring + 1)))
}

/// `m` rings of `ring` texels with a bar either side, the leftover spread a
/// texel at a time over the bars so nothing is ever clipped.
fn spread(len: usize, ring: usize, m: usize) -> Run {
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

/// A side `len` texels long: one bar, or two and a ring once it is long
/// enough for both to stay whole.
fn side_run(len: usize, ring: usize) -> Run {
    spread(len, ring, usize::from(len >= SIDE_TWO.max(ring + 2)))
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

/// The chain's quads for a `w` by `h` texel grid whose top and bottom
/// `over` rows hang outside the row: corner rings, face-on rings, then the
/// bars over their ends.
fn quads(st: &Style, w: usize, h: usize) -> Vec<Quad> {
    let mut out = Vec::new();
    let (b, rl, rv) = (st.band, st.ring[0].len(), st.ring_v.len());
    if w < 2 * rl + 2 || h < 2 * b + 2 {
        return out;
    }
    let across = layout(w - 2 * rl, rl, st.gap);
    let down = side_run(h - 2 * b, rv);
    let corners = [(0, 0), (w - rl, 0), (0, h - b), (w - rl, h - b)];
    for (x, y) in corners {
        put(&mut out, &st.ring, x, y);
    }
    for &r in &across.rings {
        put(&mut out, &st.ring, rl + r, 0);
        put(&mut out, &st.ring, rl + r, h - b);
    }
    let sides = st.bolt.is_none();
    if sides {
        for &r in &down.rings {
            put(&mut out, &st.ring_v, 0, b + r);
            put(&mut out, &st.ring_v, w - st.side, b + r);
        }
    }
    // Bars last: their tips sit in the rings' holes, over the dark. The end
    // bars reach into the corner rings, which hold the sides' bars too.
    for &(at, span) in &across.bars {
        let bar = bar_h(st, span, st.tuck - st.wall);
        put(&mut out, &bar, rl + at - st.tuck, 0);
        put(&mut out, &bar, rl + at - st.tuck, h - b);
    }
    if sides {
        for &(at, span) in &down.bars {
            let bar = bar_v(st, span);
            put(&mut out, &bar, 0, b + at - st.tuck);
            put(&mut out, &bar, w - st.side, b + at - st.tuck);
        }
    }
    // Rails: each end ring hangs on a bolt, its head over the ring's wall.
    if let Some(bolt) = &st.bolt {
        let (bw, hy) = (bolt[0].len(), (b - bolt.len()) / 2);
        for (x, y) in corners {
            let hx = if x == 0 { 0 } else { rl - bw };
            put(&mut out, bolt, x + hx, y + hy);
        }
    }
    out
}

/// The chain over a selected row on the castle skin, else nothing. It hangs
/// past the row's top and bottom and is drawn deferred, above its
/// neighbours; absolute, so layout stays as it was.
pub fn frame() -> Option<impl IntoElement> {
    (!crate::carve::today() && crate::skin::lamp() == Some(seance_core::schemes::Lamp::Torch)).then(
        || {
            let st = style(option());
            let over = px(st.over as f32 * st.scale);
            gpui::deferred(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint(&st, bounds, window),
                )
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

fn paint(st: &Style, bounds: Bounds<Pixels>, window: &mut Window) {
    // Whole points, so the hard texels never blur.
    let o = bounds.origin;
    let (ox, oy, s) = (f32::from(o.x).round(), f32::from(o.y).round(), st.scale);
    let w = (f32::from(bounds.size.width) / s) as usize;
    // The row's own height is the bounds less what hangs above and below;
    // round it up so the bottom band's inner edge keeps its clearance.
    let row = (f32::from(bounds.size.height) - 2. * st.over as f32 * s).max(0.);
    let h = (row / s).ceil() as usize + 2 * st.over;
    for q in quads(st, w, h) {
        let r = Bounds::new(
            point(px(ox + q.x as f32 * s), px(oy + q.y as f32 * s)),
            size(px(q.w as f32 * s), px(s)),
        );
        window.paint_quad(gpui::fill(r, Hsla::from(gpui::rgb(q.color))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every grid the app can give the chain: sidebars 160 to 520 pt wide,
    /// rows 36 to 90 pt tall, in this style's texels.
    fn grids(st: &Style) -> impl Iterator<Item = (usize, usize)> + '_ {
        (160..=520).step_by(4).flat_map(move |w: usize| {
            (36usize..=90).map(move |h| {
                let s = st.scale as usize;
                (w / s, h.div_ceil(s) + 2 * st.over)
            })
        })
    }

    /// The texels as painted, later quads over earlier ones.
    fn texels(q: &[Quad], w: usize, h: usize) -> Vec<Vec<Option<u32>>> {
        let mut g = vec![vec![None; w]; h];
        for r in q {
            g[r.y][r.x..r.x + r.w].fill(Some(r.color));
        }
        g
    }

    fn closes(r: &Run, len: usize, ring: usize) {
        assert_eq!(r.bars.len(), r.rings.len() + 1, "len {len}");
        // Bars and rings tile the run end to end, whole texels, no overlap.
        let mut at = 0;
        for (i, &(start, span)) in r.bars.iter().enumerate() {
            assert_eq!(start, at, "len {len} bar {i}");
            assert!(span >= 1, "len {len} bar {i} is whole");
            at += span;
            if let Some(&pos) = r.rings.get(i) {
                assert_eq!(pos, at, "len {len} ring {i}");
                at += ring;
            }
        }
        assert_eq!(at, len, "len {len} closes on the end ring");
    }

    /// A ring drawn whole at (x, y): its walls as the sprite has them, and
    /// dark still showing in its hole past the tips.
    fn whole_ring(g: &[Vec<Option<u32>>], s: &Sprite, x: usize, y: usize) -> bool {
        let mut dark = false;
        for (dy, row) in s.iter().enumerate() {
            for (dx, &c) in row.iter().enumerate() {
                let at = g[y + dy][x + dx];
                if c == 'o' {
                    dark |= at == color('o');
                } else if c != '.' && at != color(c) {
                    return false;
                }
            }
        }
        dark
    }

    /// Frame and heavy: four runs of whole links between four whole corner
    /// rings, and nothing over the row's middle.
    fn four_sides(o: ChainOption) {
        let st = style(o);
        let (b, rl, rv) = (st.band, st.ring[0].len(), st.ring_v.len());
        for (w, h) in grids(&st) {
            closes(&layout(w - 2 * rl, rl, st.gap), w - 2 * rl, rl);
            let down = side_run(h - 2 * b, rv);
            closes(&down, h - 2 * b, rv);
            assert!(down.bars.len() <= 2, "{w}x{h}: one or two bars a side");
            let q = quads(&st, w, h);
            assert!(q.iter().all(|r| r.x + r.w <= w && r.y < h), "{w}x{h}");
            let g = texels(&q, w, h);
            for (x, y) in [(0, 0), (w - rl, 0), (0, h - b), (w - rl, h - b)] {
                assert!(whole_ring(&g, &st.ring, x, y), "{w}x{h} corner {x},{y}");
            }
            for (y, row) in g.iter().enumerate().take(h - b).skip(b) {
                assert!(
                    row[st.side..w - st.side].iter().all(Option::is_none),
                    "{w}x{h} row {y}"
                );
                // Each side is iron the whole way down: no broken bits.
                assert!(row[st.side / 2].is_some() && row[w - 1 - st.side / 2].is_some());
            }
        }
    }

    #[test]
    fn the_frame_closes_in_whole_links_at_every_size() {
        four_sides(ChainOption::Frame);
    }

    #[test]
    fn heavy_closes_in_whole_links_at_every_size() {
        four_sides(ChainOption::Heavy);
    }

    #[test]
    fn rails_hang_on_a_bolt_at_each_end_and_skip_the_sides() {
        let st = style(ChainOption::Rails);
        let (b, rl) = (st.band, st.ring[0].len());
        let bolt = st.bolt.as_ref().unwrap();
        let (bw, bh) = (bolt[0].len(), bolt.len());
        for (w, h) in grids(&st) {
            closes(&layout(w - 2 * rl, rl, st.gap), w - 2 * rl, rl);
            let g = texels(&quads(&st, w, h), w, h);
            for (x, y) in [(0, 0), (w - bw, 0), (0, h - b), (w - bw, h - b)] {
                for (dy, row) in bolt.iter().enumerate() {
                    for (dx, &c) in row.iter().enumerate() {
                        let at = g[y + (b - bh) / 2 + dy][x + dx];
                        assert!(c == '.' || at == color(c), "{w}x{h} bolt {x},{y}");
                    }
                }
            }
            assert!(g[b..h - b].iter().flatten().all(Option::is_none), "{w}x{h}");
        }
    }

    #[test]
    fn every_sprite_is_square_and_leaves_room_for_the_rows_padding() {
        for o in [ChainOption::Frame, ChainOption::Rails, ChainOption::Heavy] {
            let st = style(o);
            for s in [&st.ring, &st.ring_v].into_iter().chain(st.bolt.as_ref()) {
                assert!(s.iter().all(|r| r.len() == s[0].len()), "{o:?}");
            }
            assert_eq!(st.ring.len(), st.band, "{o:?}");
            assert_eq!(st.ring_v[0].len(), st.side, "{o:?}");
            // Inside the row against its 8 pt padding, the sides against 12.
            assert!(((st.band - st.over) as f32) * st.scale <= 6., "{o:?}");
            assert!((st.side as f32) * st.scale <= 10., "{o:?}");
        }
    }

    #[test]
    fn a_tiny_row_draws_nothing_rather_than_clip() {
        let st = style(OPTION);
        assert!(quads(&st, 8, 30).is_empty() && quads(&st, 30, 8).is_empty());
    }
}
