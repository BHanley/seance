//! Skins: the active scheme's optional chrome texture and line weight.
//!
//! gpui has no repeat fill, so a texture is a `canvas` that paints the tile
//! with `paint_image` across its bounds. The tile is decoded and upscaled
//! once per (texture, scale) and cached, so its `RenderImage` id is stable
//! and the sprite atlas uploads it once; each frame only adds one sprite
//! per tile position. Terminal bodies never get a texture.

use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, point, px, size, Bounds, ContentMask, Corners, IntoElement, ParentElement, Pixels,
    RenderImage, Styled, Window,
};
use seance_core::schemes::Texture;

/// Chrome lines draw 1px in the scheme's border color, pane frames included.
pub fn thin_lines() -> bool {
    crate::theme::active().thin_lines
}

/// The texture layer for a chrome surface, or `None` on a flat scheme. Put
/// it first inside a `.relative()` parent; it fills the parent, takes no
/// clicks and paints over the parent's fill at `opacity`, so a fill that
/// carries meaning (the active title strip) still shows through.
pub fn backdrop(opacity: f32) -> Option<impl IntoElement> {
    let tex = crate::theme::active().texture.as_ref()?;
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .opacity(opacity)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint_tiles(tex, bounds, window),
                )
                .size_full(),
            ),
    )
}

/// Tiles are anchored to the window origin, so the rail and every title
/// strip share one grid of stones.
fn paint_tiles(tex: &'static Texture, bounds: Bounds<Pixels>, window: &mut Window) {
    let scale = window.scale_factor();
    let k = scale.round().max(1.0) as u32;
    let Some(image) = render_image(tex, k) else {
        return;
    };
    // One texel per logical pixel: the tile is upscaled by `k` device
    // pixels, so it maps 1:1 onto the screen and the linear sampler can't
    // blur the pixel art.
    let step = (tex.size * k) as f32 / scale;
    let x_end = f32::from(bounds.origin.x + bounds.size.width);
    let y_end = f32::from(bounds.origin.y + bounds.size.height);
    let x0 = (f32::from(bounds.origin.x) / step).floor() * step;
    let y0 = (f32::from(bounds.origin.y) / step).floor() * step;
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        let mut y = y0;
        while y < y_end {
            let mut x = x0;
            while x < x_end {
                let tile = Bounds::new(point(px(x), px(y)), size(px(step), px(step)));
                // Only fails on a bad frame index; 0 always exists.
                let _ = window.paint_image(tile, Corners::default(), image.clone(), 0, false);
                x += step;
            }
            y += step;
        }
    });
}

/// Decoded, `k`x nearest-neighbor upscaled, BGRA tile; built once per
/// texture and scale.
fn render_image(tex: &'static Texture, k: u32) -> Option<Arc<RenderImage>> {
    type Cache = Vec<(usize, u32, Arc<RenderImage>)>;
    static CACHE: Mutex<Cache> = Mutex::new(Vec::new());
    let key = tex.png.as_ptr() as usize;
    let mut cache = CACHE.lock().ok()?;
    if let Some((_, _, img)) = cache.iter().find(|(p, s, _)| *p == key && *s == k) {
        return Some(img.clone());
    }
    // A tile that won't decode paints nothing; the flat fill stays.
    let img = Arc::new(decode(tex, k).ok()?);
    cache.push((key, k, img.clone()));
    Some(img)
}

fn decode(tex: &Texture, k: u32) -> anyhow::Result<RenderImage> {
    let rgba = image::load_from_memory_with_format(tex.png, image::ImageFormat::Png)?.into_rgba8();
    let mut big = image::imageops::resize(
        &rgba,
        rgba.width() * k,
        rgba.height() * k,
        image::imageops::FilterType::Nearest,
    );
    // gpui's atlas wants BGRA.
    for p in big.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    Ok(RenderImage::new(vec![image::Frame::new(big)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn castle_tile_decodes_upscaled_and_cached_once() {
        let tex = seance_core::schemes::find("castle")
            .and_then(|s| s.texture.as_ref())
            .unwrap();
        let a = render_image(tex, 2).unwrap();
        let b = render_image(tex, 2).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "same scale reuses one RenderImage");
        let sz = a.size(0);
        assert_eq!((sz.width.0, sz.height.0), (192, 192));
        let c = render_image(tex, 1).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
    }
}
