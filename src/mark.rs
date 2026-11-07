//! The gold critter beside "seance" at the top of the rail, with a flame on
//! its head. Gold in every theme, so it reads nothing from `SeancePalette`.

use gpui::{canvas, point, px, size, Bounds, Hsla, IntoElement, Pixels, Styled, Window};

const W: usize = 6;
const H: usize = 10;
/// Points per texel: three device pixels at 2x.
const TEXEL: f32 = 1.5;

const GOLD: u32 = 0xE8A33D;
const PALE: u32 = 0xF6D48A;

#[rustfmt::skip]
const SPRITE: [&[u8; W]; H] = [
    b"...A..",
    b"..AA..",
    b"..AAP.",
    b"..APA.",
    b"A....A",
    b"AAAAAA",
    b"A.AA.A",
    b"AAAAAA",
    b".AAAA.",
    b".A..A.",
];

/// The mark itself, 9x15 points. The caller owns the click and the hover.
pub fn element() -> impl IntoElement {
    canvas(|_, _, _| {}, |bounds, _, window, _| paint(bounds, window))
        .flex_none()
        .w(px(W as f32 * TEXEL))
        .h(px(H as f32 * TEXEL))
}

/// `v` points moved to the nearest device pixel, so hard texels never blur.
fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// One quad per horizontal run of one color.
fn paint(bounds: Bounds<Pixels>, window: &mut Window) {
    let scale = window.scale_factor();
    let ox = snap(f32::from(bounds.origin.x), scale);
    let oy = snap(f32::from(bounds.origin.y), scale);
    let edge = |o: f32, n: usize| snap(o + n as f32 * TEXEL, scale);
    for (y, row) in SPRITE.iter().enumerate() {
        let mut x = 0;
        while x < W {
            let c = row[x];
            let start = x;
            while x < W && row[x] == c {
                x += 1;
            }
            if c == b'.' {
                continue;
            }
            let (left, top) = (edge(ox, start), edge(oy, y));
            let quad = Bounds::new(
                point(px(left), px(top)),
                size(px(edge(ox, x) - left), px(edge(oy, y + 1) - top)),
            );
            let color = if c == b'P' { PALE } else { GOLD };
            window.paint_quad(gpui::fill(quad, Hsla::from(gpui::rgb(color))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texels_stay_three_device_pixels_from_a_fractional_origin() {
        let o = snap(10.25, 2.0);
        for n in 0..H {
            let width = snap(o + (n + 1) as f32 * TEXEL, 2.0) - snap(o + n as f32 * TEXEL, 2.0);
            assert_eq!(width * 2.0, 3.0, "texel {n}");
        }
    }
}
