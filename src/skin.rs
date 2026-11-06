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

/// Screenshots of the skin without a window on screen: gpui's headless
/// context with the Metal offscreen renderer. The layout is a simplified
/// copy of the rail and two panes (the real app needs a daemon), built from
/// the real tiler and the real `SeancePalette`.
///
/// `cargo test --bin seance skin_shots -- --ignored`; PNGs land in
/// `shots/` (or `$SKIN_SHOTS_DIR`).
#[cfg(all(test, target_os = "macos"))]
mod shots {
    use super::*;
    use crate::theme::SeancePalette as P;
    use gpui::prelude::FluentBuilder as _;
    use gpui::{AppContext as _, Context, FontWeight, Hsla, Render};

    struct Shot {
        schemes: Vec<&'static str>,
    }

    impl Render for Shot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut row = div()
                .flex()
                .size_full()
                .gap(px(10.))
                .p(px(10.))
                .bg(gpui::rgb(0x000000));
            for id in &self.schemes {
                // Elements capture colors when built, so each panel can be
                // built under its own scheme.
                crate::theme::set_active_for_test(id);
                row = row.child(panel(id));
            }
            row
        }
    }

    fn panel(id: &str) -> impl IntoElement {
        let scheme = crate::theme::active();
        let (term_fg, term_bg): (Hsla, Hsla) = match scheme.term {
            Some(t) => (gpui::rgb(t.fg).into(), gpui::rgb(t.bg).into()),
            None => (P::text(), P::bg()),
        };
        let circle = |name: &str, selected: bool| {
            div()
                .px_3()
                .py_1()
                .text_xs()
                .when(selected, |d| d.bg(P::flame().opacity(0.22)))
                .text_color(if selected { P::text() } else { P::text_dim() })
                .child(name.to_string())
        };
        let sidebar = div()
            .relative()
            .flex_none()
            .w(px(180.))
            .h_full()
            .flex()
            .flex_col()
            .bg(P::bg_elevated())
            .border_r_1()
            .border_color(P::border())
            .children(backdrop(1.0))
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_1p5()
                    .flex()
                    .gap_2()
                    .border_b_1()
                    .border_color(P::border())
                    .child(div().text_color(P::flame_dim()).child("✦"))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(P::text())
                            .child("seance"),
                    ),
            )
            .child(circle("l6-leveling", true))
            .child(circle("rudle", false))
            .child(circle("riff-raiders", false))
            .child(circle("skins", false))
            .child(
                div()
                    .mt_auto()
                    .px_3()
                    .pb_2()
                    .text_xs()
                    .text_color(P::text_faint())
                    .child(format!("scheme: {id}")),
            );
        let pane = |name: &str, active: bool, lines: &[&str]| {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded_md()
                .map(|d| {
                    if thin_lines() {
                        d.border_1()
                    } else {
                        d.border_2()
                    }
                })
                .border_color(if active { P::flame() } else { P::border() })
                .bg(P::bg())
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .h(px(26.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .bg(if active {
                            P::surface()
                        } else {
                            P::bg_elevated()
                        })
                        .children(backdrop(if active { 0.35 } else { 0.8 }))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if active { P::flame() } else { P::text_faint() })
                                .child("⌨ you"),
                        )
                        .child(div().size(px(6.)).rounded_full().bg(if active {
                            P::flame()
                        } else {
                            P::text_faint()
                        }))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if active { P::text() } else { P::text_faint() })
                                .child(name.to_string()),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .p_2()
                        .bg(term_bg)
                        .font_family("Menlo")
                        .text_xs()
                        .text_color(term_fg)
                        .children(lines.iter().map(|l| div().child(l.to_string()))),
                )
        };
        div()
            .flex_1()
            .h_full()
            .flex()
            .overflow_hidden()
            .rounded_md()
            .font_family("Helvetica Neue")
            .bg(P::bg())
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_1()
                    .child(pane(
                        "lead · claude",
                        true,
                        &[
                            "$ cargo test -p seance-core",
                            "test result: ok. 141 passed",
                            "$ _",
                        ],
                    ))
                    .child(pane(
                        "worker · zsh",
                        false,
                        &[
                            "~/work/seance-skins (bh/skins)",
                            "$ git log --oneline -1",
                            "c5e412e Paint the skin texture",
                        ],
                    )),
            )
    }

    fn render(schemes: Vec<&'static str>, w: f32, h: f32, out: &std::path::Path) {
        let platform = gpui_platform::current_platform(true);
        let mut cx = gpui::HeadlessAppContext::with_platform(
            platform.text_system(),
            Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        let win = cx
            .open_window(size(px(w), px(h)), move |_, cx| {
                cx.new(|_| Shot { schemes })
            })
            .expect("headless window");
        cx.run_until_parked();
        cx.update_window(win.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
        })
        .unwrap();
        let img = cx.capture_screenshot(win.into()).expect("capture");
        img.save(out).expect("save png");
        eprintln!("wrote {} ({}x{})", out.display(), img.width(), img.height());
    }

    #[test]
    #[ignore = "writes PNGs via headless Metal; run with --ignored"]
    fn skin_shots() {
        let dir = std::env::var("SKIN_SHOTS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shots"));
        std::fs::create_dir_all(&dir).unwrap();
        render(
            vec!["castle", "candlelit"],
            1100.,
            330.,
            &dir.join("castle-vs-candlelit.png"),
        );
        render(vec!["castle"], 560., 330., &dir.join("castle-chrome.png"));
    }
}
