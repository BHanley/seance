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

/// The active scheme's skin, or `None` on a flat scheme.
pub fn active() -> Option<&'static Texture> {
    crate::theme::active().texture.as_ref()
}

/// Skinned panes are square stone; flat ones keep their rounded frame.
pub fn square() -> bool {
    active().is_some()
}

/// Frame color of an unfocused pane: the skin's stone, else `border`.
pub fn idle_frame() -> gpui::Hsla {
    match active() {
        Some(t) => gpui::rgb(t.frame).into(),
        None => crate::theme::SeancePalette::border(),
    }
}

/// Wall torches in the pane title strips.
pub fn torches() -> bool {
    active().is_some_and(|t| t.torches)
}

/// Texture strength per surface: low enough that the stone reads as a calm
/// ground behind labels, not a pattern fighting them (see docs/THEME.md for
/// the contrast numbers).
pub const RAIL: f32 = 0.6;
/// A strip of tabs or a drawer header.
pub const STRIP: f32 = 0.6;
/// A pane title strip; the focused one lighter so its fill marks focus.
pub const TITLE: f32 = 0.55;
pub const TITLE_ACTIVE: f32 = 0.3;

/// Put a backdrop under a skinned surface: makes it the positioning parent
/// only when there is a skin, so flat schemes keep their exact layout.
/// Call it before adding any other child, so the stone paints behind them.
pub fn under<E: ParentElement + Styled>(el: E, opacity: f32) -> E {
    match backdrop(opacity) {
        Some(b) => el.relative().child(b),
        None => el,
    }
}

/// The texture layer for a chrome surface, or `None` on a flat scheme. Put
/// it first inside a `.relative()` parent; it fills the parent, takes no
/// clicks and paints over the parent's fill at `opacity`, so a fill that
/// carries meaning (the active title strip) still shows through.
pub fn backdrop(opacity: f32) -> Option<impl IntoElement> {
    let tex = active()?;
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
        assert_eq!((sz.width.0, sz.height.0), (512, 512));
        let c = render_image(tex, 1).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
    }
}

/// Screenshots of the skin without a window on screen: gpui's headless
/// context with the Metal offscreen renderer. The layout is a simplified
/// mock of the rail (with its footer), the shelf strip, four panes and a
/// dock with its tab strip (the real app needs a daemon), built from the
/// real tiler, torch painter, skin constants and `SeancePalette`. It is not
/// a capture of the live app.
///
/// `cargo test --bin seance skin_shots -- --ignored`; PNGs land in
/// `shots/` (or `$SKIN_SHOTS_DIR`).
#[cfg(all(test, target_os = "macos"))]
mod shots {
    use super::*;
    use crate::theme::SeancePalette as P;
    use crate::torch::Torch;
    use gpui::prelude::FluentBuilder as _;
    use gpui::{AnyElement, AppContext as _, Context, FontWeight, Hsla, Render};

    /// A fixed clock so every run draws the same frames.
    const NOW: u64 = 1_700_000_000_000;

    enum Scene {
        Chrome(Vec<&'static str>),
        Torches,
    }

    struct Shot(Scene);

    impl Render for Shot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let row = div()
                .flex()
                .size_full()
                .gap(px(10.))
                .p(px(10.))
                .bg(gpui::rgb(0x000000))
                .font_family("Helvetica Neue");
            match &self.0 {
                Scene::Chrome(schemes) => {
                    let mut row = row;
                    for id in schemes {
                        // Elements capture colors when built, so each panel
                        // can be built under its own scheme.
                        crate::theme::set_active_for_test(id);
                        row = row.child(panel(id));
                    }
                    row
                }
                Scene::Torches => {
                    crate::theme::set_active_for_test("castle");
                    row.child(torch_sheet())
                }
            }
        }
    }

    fn label(text: &str, color: Hsla) -> impl IntoElement {
        div().text_xs().text_color(color).child(text.to_string())
    }

    fn sidebar(id: &str) -> impl IntoElement {
        let circle = |name: &str, selected: bool, note: &str| {
            div()
                .px_3()
                .py_1()
                .flex()
                .justify_between()
                .text_xs()
                .when(selected, |d| d.bg(P::flame().opacity(0.22)))
                .child(
                    div()
                        .text_color(if selected { P::text() } else { P::text_dim() })
                        .child(name.to_string()),
                )
                .child(div().text_color(P::text_faint()).child(note.to_string()))
        };
        let button = |text: &str, color: Hsla, grow: bool| {
            div()
                .when(grow, |d| d.flex_1())
                .px_3()
                .py_1p5()
                .rounded_md()
                .flex()
                .justify_center()
                .text_sm()
                .text_color(color)
                .bg(P::surface())
                .child(text.to_string())
        };
        div()
            .relative()
            .flex_none()
            .w(px(190.))
            .h_full()
            .flex()
            .flex_col()
            .bg(P::bg_elevated())
            .border_r_1()
            .border_color(P::border())
            .children(backdrop(RAIL))
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
            .child(div().h(px(6.)))
            .child(circle("l6-leveling", true, "now"))
            .child(circle("rudle", false, "3m"))
            .child(circle("riff-raiders", false, "1h"))
            .child(circle("skins", false, "2d"))
            .child(
                div()
                    .mt_auto()
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(P::text_faint())
                    .child(format!("scheme: {id}")),
            )
            .child(
                // The footer: summon, activity, help.
                div()
                    .flex_none()
                    .p_2()
                    .border_t_1()
                    .border_color(P::border())
                    .flex()
                    .gap_2()
                    .child(button("+ summon", P::flame(), true))
                    .child(button("≋", P::text_dim(), false))
                    .child(button("?", P::violet(), false)),
            )
    }

    fn pane(name: &str, active: bool, torch: Option<Torch>, lines: &[&str]) -> impl IntoElement {
        let scheme = crate::theme::active();
        let (term_fg, term_bg): (Hsla, Hsla) = match scheme.term {
            Some(t) => (gpui::rgb(t.fg).into(), gpui::rgb(t.bg).into()),
            None => (P::text(), P::bg()),
        };
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(!square(), |d| d.rounded_md())
            .border_2()
            .border_color(if active { P::flame() } else { idle_frame() })
            .bg(P::bg())
            .child(
                div()
                    .when(super::active().is_some(), |d| d.relative())
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
                    .children(backdrop(if active { TITLE_ACTIVE } else { TITLE }))
                    .children(
                        torch.map(|t| crate::torch::element(t, NOW, crate::torch::seed(name), 1.0)),
                    )
                    .child(label(
                        if active { "⌨ you" } else { "⚡ claude" },
                        if active { P::flame() } else { P::text_faint() },
                    ))
                    .child(div().size(px(6.)).rounded_full().bg(if active {
                        P::flame()
                    } else {
                        P::text_faint()
                    }))
                    .child(label(
                        name,
                        if active { P::text() } else { P::text_faint() },
                    )),
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
    }

    fn shelf() -> impl IntoElement {
        let chip = |name: &str| {
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .text_xs()
                .bg(P::surface())
                .text_color(P::text_dim())
                .child(name.to_string())
        };
        div()
            .flex_none()
            .px_2()
            .py_1()
            .flex()
            .gap_1()
            .border_b_1()
            .border_color(P::border())
            .bg(P::bg_elevated())
            .map(|d| under(d, STRIP))
            .child(chip("notes.md"))
            .child(chip("scratch · zsh"))
    }

    fn dock() -> impl IntoElement {
        let tab = |name: &str, open: bool| {
            div()
                .h_full()
                .px_2()
                .flex()
                .items_center()
                .text_xs()
                .border_b_2()
                .border_color(if open {
                    P::flame()
                } else {
                    gpui::transparent_black()
                })
                .text_color(if open { P::text() } else { P::text_faint() })
                .child(name.to_string())
        };
        div()
            .flex_none()
            .w(px(200.))
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(P::border())
            .bg(P::bg_elevated())
            .child(
                div()
                    .flex_none()
                    .h(px(30.))
                    .pl_1()
                    .pr_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(P::border())
                    .map(|d| under(d, STRIP))
                    .child(tab("PLAN.md", true))
                    .child(tab("LOG.md", false)),
            )
            .child(
                div()
                    .flex_1()
                    .p_2()
                    .text_xs()
                    .text_color(P::text_dim())
                    .child("# Castle skin")
                    .child("- stone on chrome only")
                    .child("- file bodies stay flat"),
            )
    }

    fn panel(id: &str) -> impl IntoElement {
        let torches = crate::theme::active()
            .texture
            .as_ref()
            .is_some_and(|t| t.torches);
        let t = |s: Torch| torches.then_some(s);
        div()
            .flex_1()
            .h_full()
            .flex()
            .overflow_hidden()
            .rounded_md()
            .bg(P::bg())
            .child(sidebar(id))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(shelf())
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .gap_1()
                                    .child(pane(
                                        "lead · claude",
                                        true,
                                        t(Torch::Working),
                                        &["$ cargo test -p seance-core", "test result: ok", "$ _"],
                                    ))
                                    .child(pane(
                                        "reviewer · claude",
                                        false,
                                        t(Torch::NeedsYou),
                                        &["? Allow edit to src/skin.rs", "  1. yes  2. no"],
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .gap_1()
                                    .child(pane(
                                        "worker · zsh",
                                        false,
                                        t(Torch::Idle),
                                        &["~/work/seance-skins", "$ git log --oneline -1"],
                                    ))
                                    .child(pane(
                                        "build · zsh",
                                        false,
                                        t(Torch::Stopped),
                                        &["[process exited 0]"],
                                    )),
                            ),
                    ),
            )
            .child(dock())
    }

    /// Every state down the side, frames across, at 4 points per texel on
    /// the title strip's stone.
    fn torch_sheet() -> impl IntoElement {
        let states = [
            (Torch::Idle, "idle"),
            (Torch::Working, "working"),
            (Torch::NeedsYou, "needs you"),
            (Torch::Stopped, "stopped"),
        ];
        let rows: Vec<AnyElement> = states
            .iter()
            .map(|(state, name)| {
                let frames = (0..6u64).map(|f| {
                    let now = f * state.frame_ms();
                    div()
                        .p(px(6.))
                        .relative()
                        .bg(P::bg_elevated())
                        .children(backdrop(TITLE))
                        .child(crate::torch::element(
                            *state,
                            now,
                            crate::torch::seed("a"),
                            4.0,
                        ))
                });
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(110.))
                            .text_sm()
                            .text_color(P::text())
                            .child(format!("{name} · {}ms", state.frame_ms())),
                    )
                    .children(frames)
                    .into_any_element()
            })
            .collect();
        div().flex().flex_col().gap(px(8.)).children(rows)
    }

    fn render(scene: Scene, w: f32, h: f32, out: &std::path::Path) {
        let platform = gpui_platform::current_platform(true);
        let mut cx = gpui::HeadlessAppContext::with_platform(
            platform.text_system(),
            Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        let win = cx
            .open_window(size(px(w), px(h)), move |_, cx| cx.new(|_| Shot(scene)))
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

    /// The bare tile repeated 3x3 at 3x, so the repeat and seams show.
    fn tile_preview(out: &std::path::Path) {
        let tex = seance_core::schemes::find("castle")
            .and_then(|s| s.texture.as_ref())
            .unwrap();
        let tile = image::load_from_memory(tex.png).unwrap().into_rgba8();
        let n = tex.size;
        let mut grid = image::RgbaImage::new(n * 3, n * 3);
        for gy in 0..3 {
            for gx in 0..3 {
                image::imageops::replace(&mut grid, &tile, (gx * n) as i64, (gy * n) as i64);
            }
        }
        let big =
            image::imageops::resize(&grid, n * 9, n * 9, image::imageops::FilterType::Nearest);
        big.save(out).expect("save png");
        eprintln!("wrote {}", out.display());
    }

    #[test]
    #[ignore = "writes PNGs via headless Metal; run with --ignored"]
    fn skin_shots() {
        let dir = std::env::var("SKIN_SHOTS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shots"));
        std::fs::create_dir_all(&dir).unwrap();
        render(
            Scene::Chrome(vec!["castle"]),
            900.,
            420.,
            &dir.join("castle-chrome.png"),
        );
        render(
            Scene::Chrome(vec!["castle", "candlelit"]),
            1700.,
            420.,
            &dir.join("castle-vs-candlelit.png"),
        );
        render(Scene::Torches, 560., 470., &dir.join("torch-sheet.png"));
        tile_preview(&dir.join("castle-tile-3x3.png"));
    }
}
