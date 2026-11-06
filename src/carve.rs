//! The carved look (`Texture::carved`, castle): square plates with hard
//! pixel bevels, as if cut from the wall. Widgets pass through these helpers
//! and get back their stock rounded look on every other scheme, so the
//! builders carry no `if castle`.
//!
//! A bevel is one canvas of a few `paint_quad` strips in whole pixels: an
//! outline, a lit top and left, a shadowed bottom and right. No blur, no
//! gradient, no transparency. A plate's wear is the skin's tile at its own
//! corner (`skin::worn`), the same cached sprite the rail uses.

use gpui::{
    canvas, point, px, size, Bounds, Hsla, IntoElement, ParentElement, Pixels, Styled, Window,
};
use seance_core::schemes::Carve;

#[cfg(test)]
thread_local! {
    /// Shot tests: draw the scheme as it was before carving (no carving, no
    /// recolored plates, no chain), to set beside the carved render.
    pub static TODAY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether shots are drawing the pre-carving look.
#[cfg(test)]
pub fn today() -> bool {
    TODAY.with(|t| t.get())
}
#[cfg(not(test))]
pub fn today() -> bool {
    false
}

/// How the active scheme carves its widgets, if it does.
pub fn style() -> Option<Carve> {
    if today() {
        return None;
    }
    crate::skin::active().and_then(|t| t.carved)
}

/// Whether the active scheme carves its widgets.
pub fn on() -> bool {
    style().is_some()
}

/// The rounding a widget has on schemes that don't carve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stock {
    Sm,
    Md,
    Lg,
    Full,
}

/// How a carved widget's edge is cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// Message plates: a 2 px lit and shadowed step inside a 1 px outline.
    Plate,
    /// Buttons, tabs, chips: a 1 px step.
    Control,
    /// The composer: a slot cut into the wall, so the lit side is below.
    Slot,
}

impl Cut {
    fn step(self) -> f32 {
        match self {
            Cut::Plate => 2.,
            Cut::Control | Cut::Slot => 1.,
        }
    }
}

/// Wear on a plate, as the tile's opacity over its fill.
pub const WORN: f32 = 0.45;
/// Dim stone behind Chat's messages, as the tile's opacity over the ground.
pub const STONE: f32 = 0.55;

/// A widget's corners: square when carved, else the stock rounding.
pub fn corners<E: Styled>(el: E, stock: Stock) -> E {
    corners_for(el, stock, on())
}

fn corners_for<E: Styled>(el: E, stock: Stock, carved: bool) -> E {
    if carved {
        return el;
    }
    match stock {
        Stock::Sm => el.rounded_sm(),
        Stock::Md => el.rounded_md(),
        Stock::Lg => el.rounded_lg(),
        Stock::Full => el.rounded_full(),
    }
}

/// A plate's bevel tones, from its fill: lit, shadow, outline.
pub fn tones(fill: Hsla) -> (Hsla, Hsla, Hsla) {
    let c = fill.to_rgb();
    let mix = |to: f32, t: f32| {
        Hsla::from(gpui::Rgba {
            r: c.r + (to - c.r) * t,
            g: c.g + (to - c.g) * t,
            b: c.b + (to - c.b) * t,
            a: 1.,
        })
    };
    // A fill already near white or black still shows its edges: the lit
    // step lightens by `t` of the way to white, the others darken.
    (mix(1., 0.3), mix(0., 0.45), mix(0., 0.75))
}

/// One bevel strip in logical pixels from the widget's top-left.
#[derive(Debug, PartialEq)]
pub struct Strip {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Lit,
    Shadow,
    Outline,
}

/// The strips of a `w` by `h` widget cut as `cut`: the outline first, then
/// the shadow and the lit step inside it. Empty when the widget is too small
/// to hold a step. Never overlap, never leave the widget.
pub fn strips(w: f32, h: f32, cut: Cut) -> Vec<Strip> {
    let t = cut.step();
    let (w, h) = (w.floor(), h.floor());
    if w < 2. * (1. + t) + 2. || h < 2. * (1. + t) + 2. {
        return Vec::new();
    }
    let (lit, shadow) = match cut {
        Cut::Slot => (Tone::Shadow, Tone::Lit),
        _ => (Tone::Lit, Tone::Shadow),
    };
    let s = |x, y, w, h, tone| Strip { x, y, w, h, tone };
    vec![
        s(0., 0., w, 1., Tone::Outline),
        s(0., h - 1., w, 1., Tone::Outline),
        s(0., 1., 1., h - 2., Tone::Outline),
        s(w - 1., 1., 1., h - 2., Tone::Outline),
        // The bottom and right own their corners; the top and left stop short.
        s(1., h - 1. - t, w - 2., t, shadow),
        s(w - 1. - t, 1., t, h - 2. - t, shadow),
        s(1., 1., w - 2. - t, t, lit),
        s(1., 1. + t, t, h - 2. - 2. * t, lit),
    ]
}

/// Cut `el` as `cut` over `fill` when the scheme carves; unchanged else. Call
/// it before adding children so the bevel paints under them.
pub fn carve<E: ParentElement + Styled>(el: E, fill: Hsla, cut: Cut) -> E {
    let Some(style) = style() else {
        return el;
    };
    let (lit, shadow, outline) = tones(fill);
    // A printed block's edge is the ink itself.
    let ink = crate::theme::SeancePalette::text();
    el.relative().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint(bounds, cut, style, [lit, shadow, outline, ink], window)
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

/// [`corners`] and [`carve`] together: the usual widget.
pub fn plate<E: ParentElement + Styled>(el: E, fill: Hsla, stock: Stock, cut: Cut) -> E {
    carve(corners(el, stock), fill, cut)
}

/// [`plate`] worn by the wall's stone: message bubbles. A printed block's
/// plates are flat inked colors, so only the stone skin wears its tile.
pub fn worn_plate<E: ParentElement + Styled>(el: E, fill: Hsla, stock: Stock) -> E {
    let el = corners(el, stock);
    if style() != Some(Carve::Stone) {
        return carve(el, fill, Cut::Plate);
    }
    carve(
        el.relative().children(crate::skin::worn(WORN)),
        fill,
        Cut::Plate,
    )
}

fn paint(bounds: Bounds<Pixels>, cut: Cut, style: Carve, tones: [Hsla; 4], window: &mut Window) {
    // Whole pixels at both ends, so a fractional width never smears an edge.
    let (x0, y0) = (
        f32::from(bounds.origin.x).round(),
        f32::from(bounds.origin.y).round(),
    );
    let x1 = f32::from(bounds.origin.x + bounds.size.width).round();
    let y1 = f32::from(bounds.origin.y + bounds.size.height).round();
    let (w, h) = (x1 - x0, y1 - y0);
    let ss = match style {
        Carve::Stone => strips(w, h, cut),
        Carve::Block => block_strips(w, h, cut),
    };
    for s in ss {
        let color = match (style, s.tone) {
            (Carve::Block, _) => tones[3],
            (_, Tone::Lit) => tones[0],
            (_, Tone::Shadow) => tones[1],
            (_, Tone::Outline) => tones[2],
        };
        let r = Bounds::new(point(px(x0 + s.x), px(y0 + s.y)), size(px(s.w), px(s.h)));
        window.paint_quad(gpui::fill(r, color));
    }
}

/// A run along an edge: where it starts, how long, how thick.
type Run = (f32, f32, f32);

/// Irregular runs along an edge `len` long, as a gouge leaves them: 3 to 9 px
/// long, thickness `base` mostly, a pixel over now and then, a pixel under
/// (a chip out of the line). Deterministic in `seed`, so a plate keeps its
/// edge from frame to frame.
fn rough(len: f32, seed: u64, base: f32) -> Vec<Run> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (mut at, mut out) = (0., Vec::new());
    while at < len {
        let run = (3 + next() % 7) as f32;
        let run = run.min(len - at);
        // A thin line never breaks (it would read as a dashed rule); a thick
        // one can run a pixel thin.
        let thick = match next() % 4 {
            0 if base > 1. => base - 1.,
            3 => base + 1.,
            _ => base,
        };
        out.push((at, run, thick));
        at += run;
    }
    out
}

/// A hand-cut block's edge: an ink outline of broken, uneven runs. All whole
/// pixels, within the widget, with no run on another.
pub fn block_strips(w: f32, h: f32, cut: Cut) -> Vec<Strip> {
    let (w, h) = (w.floor(), h.floor());
    let base = if cut == Cut::Plate { 2. } else { 1. };
    let reach = base + 1.;
    if w < 2. * reach + 4. || h < 2. * reach + 4. {
        return Vec::new();
    }
    let seed = (w as u64) << 20 ^ (h as u64) << 4 ^ base as u64;
    let s = |x, y, w, h| Strip {
        x,
        y,
        w,
        h,
        tone: Tone::Outline,
    };
    let mut out = Vec::new();
    for (at, len, t) in rough(w, seed, base) {
        out.push(s(at, 0., len, t));
    }
    for (at, len, t) in rough(w, seed ^ 0x5555, base) {
        out.push(s(at, h - t, len, t));
    }
    let inner = h - 2. * reach;
    for (at, len, t) in rough(inner, seed ^ 0xAAAA, base) {
        out.push(s(0., reach + at, t, len));
    }
    for (at, len, t) in rough(inner, seed ^ 0xF0F0, base) {
        out.push(s(w - t, reach + at, t, len));
    }
    out
}

/// The selected circle row's ink on a printed-block skin: a hand-inked slab
/// with the text reversed out of it. `None` on every other skin.
pub struct Slab {
    pub fill: Hsla,
    pub name: Hsla,
    pub sub: Hsla,
}

pub fn slab() -> Option<Slab> {
    if style() != Some(Carve::Block) {
        return None;
    }
    use crate::theme::SeancePalette as P;
    let (ink, paper) = (P::text(), P::bg());
    let sub = {
        let (a, b) = (paper.to_rgb(), ink.to_rgb());
        Hsla::from(gpui::Rgba {
            r: a.r + (b.r - a.r) * 0.25,
            g: a.g + (b.g - a.g) * 0.25,
            b: a.b + (b.b - a.b) * 0.25,
            a: 1.,
        })
    };
    Some(Slab {
        fill: ink,
        name: paper,
        sub,
    })
}

/// The slab as a layer under the row's contents: a body inset 2 px with its
/// four edges run out to a ragged 0 to 2 px.
pub fn slab_strips(w: f32, h: f32) -> Vec<Strip> {
    let (w, h) = (w.floor(), h.floor());
    if w < 12. || h < 12. {
        return Vec::new();
    }
    let seed = (w as u64) << 20 ^ (h as u64) << 4 ^ 0x51AB;
    let s = |x, y, w, h| Strip {
        x,
        y,
        w,
        h,
        tone: Tone::Outline,
    };
    let mut out = vec![s(2., 2., w - 4., h - 4.)];
    for (at, len, t) in rough(w - 4., seed, 1.) {
        out.push(s(2. + at, 2. - t, len, t));
    }
    for (at, len, t) in rough(w - 4., seed ^ 0x5555, 1.) {
        out.push(s(2. + at, h - 2., len, t));
    }
    for (at, len, t) in rough(h - 4., seed ^ 0xAAAA, 1.) {
        out.push(s(2. - t, 2. + at, t, len));
    }
    for (at, len, t) in rough(h - 4., seed ^ 0xF0F0, 1.) {
        out.push(s(w - 2., 2. + at, t, len));
    }
    out
}

/// Paints [`slab_strips`] in `fill` over the whole row, as a layer.
pub fn slab_layer(fill: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (x0, y0) = (
                f32::from(bounds.origin.x).round(),
                f32::from(bounds.origin.y).round(),
            );
            let w = f32::from(bounds.origin.x + bounds.size.width).round() - x0;
            let h = f32::from(bounds.origin.y + bounds.size.height).round() - y0;
            for s in slab_strips(w, h) {
                let r = Bounds::new(point(px(x0 + s.x), px(y0 + s.y)), size(px(s.w), px(s.h)));
                window.paint_quad(gpui::fill(r, fill));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Radiating hatch rays around the header creature, one place only: a ring
/// of short ink-and-accent strokes in 2 px cells, no animation. Put it as an
/// absolute child of the creature's relative wrapper, before the creature.
pub fn rays() -> Option<impl IntoElement> {
    if style() != Some(Carve::Block) {
        return None;
    }
    let reach = px(RAY_REACH);
    Some(
        canvas(
            |_, _, _| {},
            |bounds, _, window, _| paint_rays(bounds, window),
        )
        .absolute()
        .top(-reach)
        .bottom(-reach)
        .left(-reach)
        .right(-reach),
    )
}

/// How far the rays reach past the creature's box.
const RAY_REACH: f32 = 9.;
const RAYS: usize = 26;

/// Ray cells as (col, row) in 2 px cells from the center, for a ray at
/// `angle` from `r0` to `r1` px: stepped along the ray, one cell each.
pub fn ray_cells(angle: f32, r0: f32, r1: f32) -> Vec<(i32, i32)> {
    let mut cells: Vec<(i32, i32)> = Vec::new();
    let mut r = r0;
    while r <= r1 {
        let c = (
            (angle.cos() * r / 2.).round() as i32,
            (angle.sin() * r / 2.).round() as i32,
        );
        if cells.last() != Some(&c) {
            cells.push(c);
        }
        r += 1.;
    }
    cells
}

fn paint_rays(bounds: Bounds<Pixels>, window: &mut Window) {
    use crate::theme::SeancePalette as P;
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (cx, cy) = (
        (f32::from(bounds.origin.x) + w / 2.).round(),
        (f32::from(bounds.origin.y) + h / 2.).round(),
    );
    // From just outside the creature's box (the bounds less the reach).
    let inner = ((w - 2. * RAY_REACH).min(h - 2. * RAY_REACH) / 2.).max(0.) + 1.;
    for i in 0..RAYS {
        let a = std::f32::consts::TAU * i as f32 / RAYS as f32;
        // Long and short rays alternate, each a little off.
        let len = [RAY_REACH - 1., 4., 7., 5.][i % 4];
        let color = if i % 2 == 0 { P::flame() } else { P::text() };
        for (col, row) in ray_cells(a, inner, inner + len) {
            let r = Bounds::new(
                point(px(cx + col as f32 * 2. - 1.), px(cy + row as f32 * 2. - 1.)),
                size(px(2.), px(2.)),
            );
            window.paint_quad(gpui::fill(r, color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(s: &[Strip]) -> f32 {
        s.iter().map(|s| s.w * s.h).sum()
    }

    #[test]
    fn strips_tile_the_frame_without_overlap_or_leaving_it() {
        for cut in [Cut::Plate, Cut::Control, Cut::Slot] {
            for (w, h) in [(40., 20.), (221., 37.), (64.5, 22.7), (400., 120.)] {
                let ss = strips(w, h, cut);
                let (fw, fh) = (f32::floor(w), f32::floor(h));
                assert!(!ss.is_empty());
                for s in &ss {
                    assert!(s.x >= 0. && s.y >= 0. && s.x + s.w <= fw && s.y + s.h <= fh);
                    assert!(s.x.fract() == 0. && s.y.fract() == 0. && s.w.fract() == 0.);
                }
                // The ring of outline, lit and shadow covers exactly the
                // frame's edge band: no pixel twice.
                let t = cut.step();
                let band = fw * fh - (fw - 2. * (1. + t)) * (fh - 2. * (1. + t));
                assert_eq!(area(&ss), band, "{cut:?} {w}x{h}");
                for (i, a) in ss.iter().enumerate() {
                    for b in &ss[i + 1..] {
                        let apart = a.x + a.w <= b.x
                            || b.x + b.w <= a.x
                            || a.y + a.h <= b.y
                            || b.y + b.h <= a.y;
                        assert!(apart, "{cut:?} {w}x{h} overlap {a:?} {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_slot_swaps_the_lit_side_and_a_tiny_widget_gets_no_bevel() {
        let tone_at = |cut, x: f32, y: f32| {
            strips(40., 20., cut)
                .into_iter()
                .find(|s| x >= s.x && x < s.x + s.w && y >= s.y && y < s.y + s.h)
                .map(|s| s.tone)
        };
        assert_eq!(tone_at(Cut::Control, 10., 1.), Some(Tone::Lit));
        assert_eq!(tone_at(Cut::Slot, 10., 1.), Some(Tone::Shadow));
        assert_eq!(tone_at(Cut::Control, 10., 18.), Some(Tone::Shadow));
        assert!(strips(5., 5., Cut::Plate).is_empty());
    }

    #[test]
    fn the_bevel_tones_order_from_lit_to_outline() {
        let (lit, shadow, outline) = tones(Hsla::from(gpui::rgb(0x3A3A3D)));
        assert!(lit.l > shadow.l && shadow.l > outline.l);
    }

    #[test]
    fn a_scheme_that_does_not_carve_keeps_its_stock_corners() {
        use gpui::{div, Styled as _};
        for stock in [Stock::Sm, Stock::Md, Stock::Lg, Stock::Full] {
            let mut plain = corners_for(div(), stock, false);
            let mut carved = corners_for(div(), stock, true);
            assert!(plain.style().corner_radii.top_left.is_some(), "{stock:?}");
            assert!(carved.style().corner_radii.top_left.is_none(), "{stock:?}");
        }
        for s in &seance_core::schemes::SCHEMES {
            let carved = s.texture.as_ref().is_some_and(|t| t.carved.is_some());
            assert_eq!(
                carved,
                ["castle", "linocut-paper", "linocut-night"].contains(&s.id),
                "{}",
                s.id
            );
        }
    }

    /// The least contrast `color` has over any pixel of `scheme`'s tile laid
    /// over `ground` at `opacity`.
    fn worst(scheme: &seance_core::schemes::Scheme, color: u32, ground: u32, opacity: f32) -> f64 {
        let tile = image::load_from_memory(scheme.texture.as_ref().unwrap().png)
            .unwrap()
            .into_rgb8();
        tile.pixels()
            .map(|p| {
                let px = u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]);
                seance_core::schemes::contrast(
                    color,
                    seance_core::schemes::mix(ground, px, opacity),
                )
            })
            .fold(f64::MAX, f64::min)
    }

    #[test]
    fn every_ink_on_a_plate_holds_4_5_to_1_where_the_skin_wears_its_tile() {
        use crate::app::chat::me_ink;
        use seance_core::schemes::{contrast, SCHEMES};
        for s in SCHEMES
            .iter()
            .filter(|s| s.texture.as_ref().is_some_and(|t| t.carved.is_some()))
        {
            let t = s.texture.as_ref().unwrap();
            for (who, plate) in [("brian", s.me_bubble), ("lead", s.lead_bubble)] {
                let i = me_ink(true, plate).unwrap_or_else(|| panic!("{} {who}", s.id));
                // Stone wears the tile at `WORN`; a printed block's plates are flat.
                let wear = if t.carved == Some(Carve::Stone) {
                    (WORN * t.strength).min(1.)
                } else {
                    0.
                };
                for (what, ink) in [("text and links", i.ink), ("time and ticks", i.soft)] {
                    assert!(worst(s, ink, i.bg, wear) >= 4.5, "{} {who} {what}", s.id);
                }
                // The quote is an opaque tint on the plate, not on the tile.
                assert!(contrast(i.ink, i.quote_bg) >= 4.5, "{} {who} quote", s.id);
            }
        }
    }

    #[test]
    fn text_and_dim_text_hold_over_the_tile_behind_chat_and_on_the_rail() {
        use seance_core::schemes::SCHEMES;
        // The linocut skins; castle's rail is held to its own, older numbers
        // (docs/THEME.md).
        for s in SCHEMES.iter().filter(|s| s.id.starts_with("linocut")) {
            let (c, strength) = (&s.chrome, s.texture.as_ref().unwrap().strength);
            // Chat's ground at `STONE`, the rail and strips at 0.6.
            for (ground, a) in [(c.bg, STONE), (c.bg_elevated, 0.6)] {
                let a = (a * strength).min(1.);
                assert!(worst(s, c.text, ground, a) >= 7.0, "{} text", s.id);
                assert!(worst(s, c.text_dim, ground, a) >= 4.5, "{} dim", s.id);
            }
        }
    }

    #[test]
    fn a_rough_edge_is_whole_pixels_inside_its_widget_and_stable() {
        for cut in [Cut::Plate, Cut::Control, Cut::Slot] {
            for (w, h) in [(40., 20.), (221., 37.), (64.5, 22.7), (400., 120.)] {
                let a = block_strips(w, h, cut);
                assert_eq!(a, block_strips(w, h, cut), "same size, same edge");
                assert!(!a.is_empty());
                for s in &a {
                    assert!(s.x >= 0. && s.y >= 0. && s.w > 0. && s.h > 0.);
                    assert!(s.x + s.w <= f32::floor(w) && s.y + s.h <= f32::floor(h));
                    assert!(s.x.fract() == 0. && s.y.fract() == 0.);
                }
                // A thin line never breaks into a dashed rule: every run at
                // least 1 px thick.
                assert!(a.iter().all(|s| s.w.min(s.h) >= 1.));
            }
        }
        assert!(block_strips(8., 8., Cut::Plate).is_empty());
        // The slab's body and edges stay inside the row.
        for s in slab_strips(221., 37.) {
            assert!(s.x >= 0. && s.y >= 0. && s.x + s.w <= 221. && s.y + s.h <= 37.);
        }
    }

    #[test]
    fn rays_are_cells_out_from_the_ring_and_none_on_other_skins() {
        let cells = ray_cells(0.3, 16., 24.);
        assert!(cells.len() >= 3);
        let r = |c: &(i32, i32)| ((c.0 * c.0 + c.1 * c.1) as f32).sqrt() * 2.;
        assert!(cells.iter().all(|c| r(c) >= 13. && r(c) <= 27.));
        // Not a block skin here (tests run on the default scheme).
        assert!(rays().is_none() && slab().is_none());
    }

    #[test]
    fn the_two_plates_of_a_skin_differ_at_a_glance() {
        use seance_core::schemes::{contrast, SCHEMES};
        for s in SCHEMES.iter().filter(|s| s.lead_bubble.is_some()) {
            let (me, lead) = (s.me_bubble.unwrap(), s.lead_bubble.unwrap());
            assert!(contrast(me.bg, lead.bg) >= 1.5, "{}", s.id);
        }
    }
}
