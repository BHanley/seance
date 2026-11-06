//! The carved look (`Texture::carved`, castle): square plates with hard
//! pixel bevels, as if cut from the wall. Widgets pass through these helpers
//! and get back their stock rounded look on every other scheme, so the
//! builders carry no `if castle`.
//!
//! A bevel is one canvas of a few `paint_quad` strips in whole pixels: an
//! outline, a lit top and left, a shadowed bottom and right. No blur, no
//! gradient, no transparency. A plate's wear is the skin's tile at its own
//! corner (`skin::worn`), the same cached sprite the rail uses.

use gpui::{canvas, point, px, size, Bounds, Hsla, ParentElement, Pixels, Styled, Window};

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

/// Whether the active scheme carves its widgets.
pub fn on() -> bool {
    !today() && crate::skin::active().is_some_and(|t| t.carved)
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
    if !on() {
        return el;
    }
    let (lit, shadow, outline) = tones(fill);
    el.relative().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| paint(bounds, cut, [lit, shadow, outline], window),
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

/// [`plate`] worn by the wall's stone: message bubbles.
pub fn worn_plate<E: ParentElement + Styled>(el: E, fill: Hsla, stock: Stock) -> E {
    let el = corners(el, stock);
    if !on() {
        return el;
    }
    carve(
        el.relative().children(crate::skin::worn(WORN)),
        fill,
        Cut::Plate,
    )
}

fn paint(bounds: Bounds<Pixels>, cut: Cut, tones: [Hsla; 3], window: &mut Window) {
    // Whole pixels at both ends, so a fractional width never smears an edge.
    let (x0, y0) = (
        f32::from(bounds.origin.x).round(),
        f32::from(bounds.origin.y).round(),
    );
    let x1 = f32::from(bounds.origin.x + bounds.size.width).round();
    let y1 = f32::from(bounds.origin.y + bounds.size.height).round();
    for s in strips(x1 - x0, y1 - y0, cut) {
        let color = match s.tone {
            Tone::Lit => tones[0],
            Tone::Shadow => tones[1],
            Tone::Outline => tones[2],
        };
        let r = Bounds::new(point(px(x0 + s.x), px(y0 + s.y)), size(px(s.w), px(s.h)));
        window.paint_quad(gpui::fill(r, color));
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
            let carved = s.texture.as_ref().is_some_and(|t| t.carved);
            assert_eq!(carved, s.id == "castle", "{}", s.id);
        }
    }

    /// The brightest pixel of the castle tile laid over `ground` at `opacity`.
    fn brightest(ground: u32, opacity: f32) -> u32 {
        let tile = image::load_from_memory(seance_core::schemes::CASTLE_STONE.png)
            .unwrap()
            .into_rgb8();
        tile.pixels()
            .map(|p| {
                let px = u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]);
                seance_core::schemes::mix(ground, px, opacity)
            })
            // The brightest for a light ink is the one with the least contrast.
            .max_by(|a, b| {
                seance_core::schemes::contrast(*a, 0xFFFFFF)
                    .total_cmp(&seance_core::schemes::contrast(*b, 0xFFFFFF))
                    .reverse()
            })
            .unwrap()
    }

    #[test]
    fn every_ink_on_a_worn_plate_holds_4_5_to_1_over_its_brightest_stone() {
        use crate::app::chat::me_ink;
        use seance_core::schemes::{contrast, find};
        let castle = find("castle").unwrap();
        let strength = castle.texture.as_ref().unwrap().strength;
        for (who, plate) in [("brian", castle.me_bubble), ("lead", castle.lead_bubble)] {
            let i = me_ink(true, plate).unwrap();
            let worst = brightest(i.bg, (WORN * strength).min(1.));
            assert!(contrast(i.ink, worst) >= 4.5, "{who} text and links");
            assert!(contrast(i.soft, worst) >= 4.5, "{who} time and ticks");
            // The quote is an opaque tint on the plate, not on the stone.
            assert!(contrast(i.ink, i.quote_bg) >= 4.5, "{who} quote");
        }
    }

    #[test]
    fn the_two_plates_differ_and_the_stone_behind_them_stays_readable() {
        use seance_core::schemes::{contrast, find};
        let castle = find("castle").unwrap();
        let (me, lead) = (castle.me_bubble.unwrap(), castle.lead_bubble.unwrap());
        assert_ne!(me.bg, lead.bg);
        // Brian's plate is at least as blue as the lead's is grey, and lighter.
        assert!(contrast(me.bg, lead.bg) >= 1.5);
        let ground = castle.chrome.bg;
        let worst = brightest(
            ground,
            (STONE * castle.texture.as_ref().unwrap().strength).min(1.),
        );
        assert!(contrast(castle.chrome.text, worst) >= 7.0);
        assert!(contrast(castle.chrome.text_dim, worst) >= 4.5);
    }
}
