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
use seance_core::schemes::{Lamp, Texture};

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

/// Lamps (wall torches or neon tubes) in the pane title strips and around
/// Chat: whether the skin hangs any, which drives their tick too.
pub fn torches() -> bool {
    lamp().is_some()
}

/// Which lamp the active skin hangs, if any.
pub fn lamp() -> Option<Lamp> {
    active().and_then(|t| t.lamp)
}

/// Texture strength per surface, scaled by the skin's own `strength`: low
/// enough that the texture reads as a calm ground behind labels, not a
/// pattern fighting them (see docs/THEME.md for the contrast numbers).
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
            .opacity((opacity * tex.strength).min(1.0))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint_tiles(tex, bounds, window, false),
                )
                .size_full(),
            ),
    )
}

/// A texture layer for one element (a plate), at `opacity`: unlike
/// [`backdrop`] its stones start at the element's own corner, so the wear
/// moves with the element instead of swimming under it as it scrolls.
pub fn worn(opacity: f32) -> Option<impl IntoElement> {
    let tex = active()?;
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .opacity(opacity.min(1.0))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint_tiles(tex, bounds, window, true),
                )
                .size_full(),
            ),
    )
}

/// Tiles are anchored to the window origin, so the rail and every title
/// strip share one grid of stones; `own_corner` starts them at the bounds'
/// corner instead (see [`worn`]).
fn paint_tiles(
    tex: &'static Texture,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    own_corner: bool,
) {
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
    let snap = |v: f32| {
        if own_corner {
            v.round()
        } else {
            (v / step).floor() * step
        }
    };
    let (x0, y0) = (
        snap(f32::from(bounds.origin.x)),
        snap(f32::from(bounds.origin.y)),
    );
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

    #[test]
    fn neon_tile_decodes_upscaled_apart_from_castles() {
        let neon = seance_core::schemes::find("neon-district")
            .and_then(|s| s.texture.as_ref())
            .unwrap();
        let castle = seance_core::schemes::find("castle")
            .and_then(|s| s.texture.as_ref())
            .unwrap();
        let a = render_image(neon, 2).unwrap();
        let sz = a.size(0);
        assert_eq!((sz.width.0, sz.height.0), (512, 512));
        assert!(!Arc::ptr_eq(&a, &render_image(castle, 2).unwrap()));
        assert!(Arc::ptr_eq(&a, &render_image(neon, 2).unwrap()));
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
    use crate::carve::{self, Cut, Stock};
    use crate::theme::SeancePalette as P;
    use crate::torch::Torch;
    use gpui::prelude::FluentBuilder as _;
    use gpui::{AnyElement, AppContext as _, Context, FontWeight, Hsla, Render};
    use seance_core::schemes::MeBubble;

    enum Scene {
        Chrome(Vec<&'static str>),
        /// Every lamp state across frames, on this scheme's skin.
        Torches(&'static str),
        /// The Chat view: list rail, header with the lead's torch, plain
        /// bubbles, composer.
        Chat(&'static str),
        /// [`Scene::Chat`] with the narrowest sidebar the app allows.
        ChatNarrow(&'static str),
        /// [`Scene::Chat`] with Brian's bubble set to this fill and ink.
        ChatInk(&'static str, u32, u32),
        /// The Chat header once per torch state.
        ChatHeaders,
        /// A quoted reply bubble and the quote bar over the composer, once
        /// per scheme.
        /// The reply shot; `true` draws each bubble as hovered, arrow showing.
        ChatReply(Vec<&'static str>, bool),
        /// One reply panel with Brian's bubble set to this fill and ink.
        ChatReplyInk(&'static str, u32, u32),
        /// Castle's selected row at the app's heights, so the chain's short
        /// sides show at each.
        ChainRows,
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
                Scene::Torches(id) => {
                    crate::theme::set_active_for_test(id);
                    row.child(torch_sheet())
                }
                Scene::Chat(id) => {
                    crate::theme::set_active_for_test(id);
                    row.child(chat_view(id, None, 220.))
                }
                Scene::ChatNarrow(id) => {
                    crate::theme::set_active_for_test(id);
                    row.child(chat_view(id, None, 160.))
                }
                Scene::ChatInk(id, bg, ink) => {
                    crate::theme::set_active_for_test(id);
                    row.child(chat_view(id, Some(MeBubble { bg: *bg, ink: *ink }), 220.))
                }
                Scene::ChatReply(schemes, hovered) => {
                    let mut row = row;
                    for id in schemes {
                        crate::theme::set_active_for_test(id);
                        row = row.child(chat_reply_panel(id, *hovered, None));
                    }
                    row
                }
                Scene::ChatReplyInk(id, bg, ink) => {
                    crate::theme::set_active_for_test(id);
                    row.child(chat_reply_panel(
                        id,
                        false,
                        Some(MeBubble { bg: *bg, ink: *ink }),
                    ))
                }
                Scene::ChainRows => {
                    crate::theme::set_active_for_test("castle");
                    row.child(div().flex().flex_col().gap(px(24.)).p(px(12.)).children(
                        [36., 52., 70.].map(|h| {
                            div()
                                .w(px(220.))
                                .h(px(h))
                                .relative()
                                .bg(crate::app::chat::selected_row_bg(None))
                                .children(crate::chain::frame())
                        }),
                    ))
                }
                Scene::ChatHeaders => {
                    crate::theme::set_active_for_test("castle");
                    row.child(
                        div().flex_1().flex().flex_col().gap(px(8.)).children(
                            [
                                (Torch::Idle, "idle"),
                                (Torch::Working, "working"),
                                (Torch::NeedsYou, "needs you"),
                                (Torch::Stopped, "stopped"),
                            ]
                            .map(|(t, name)| {
                                div().bg(P::bg()).child(chat_header(
                                    Some(t),
                                    &format!("l6-leveling · {name}"),
                                    None,
                                ))
                            }),
                        ),
                    )
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
                        torch.map(|t| crate::torch::element(t, 0, crate::torch::seed(name), 1.0)),
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

    /// The mock's copy names the skin on screen; Neon District gets its own,
    /// every other scheme keeps Castle's.
    fn neon() -> bool {
        lamp() == Some(Lamp::Tube)
    }

    fn dock() -> impl IntoElement {
        let (title, line) = if neon() {
            ("# Neon District skin", "- steel on chrome only")
        } else {
            ("# Castle skin", "- stone on chrome only")
        };
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
                    .child(title)
                    .child(line)
                    .child("- file bodies stay flat"),
            )
    }

    fn panel(id: &str) -> impl IntoElement {
        let torches = torches();
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

    /// The Chat header as `chat.rs` builds it: stone strip, the lead's torch
    /// at 1.5x, the circle's creature (a stand-in square here), name, chips.
    /// With `decor_w` (the header's width) it also gets Chat's decorative
    /// wall torches and the one by the task count.
    fn chat_header(torch: Option<Torch>, name: &str, decor_w: Option<f32>) -> impl IntoElement {
        use crate::app::chat::decor;
        let line = "running cargo test";
        let wall = decor_w.map_or_else(Vec::new, |w| {
            decor::wall(
                0,
                w,
                decor::header_text_end(torch.is_some(), 1, name, line),
                decor::header_chips_at(w, true),
            )
        });
        let chip = |text: &str, fill: bool| {
            carve::plate(div(), P::surface(), Stock::Sm, Cut::Control)
                .flex_none()
                .px_2()
                .py_0p5()
                .text_xs()
                .text_color(P::text_dim())
                .when(fill || carve::on(), |d| carve::bg(d, P::surface()))
                .child(text.to_string())
        };
        div()
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(P::border())
            .map(|d| under(d, STRIP))
            .children(wall)
            .children(
                torch.map(|t| {
                    crate::torch::element(t, 0, crate::torch::seed("l6-leveling-lead"), 1.5)
                }),
            )
            .child(
                div()
                    .size(px(30.))
                    .rounded_sm()
                    .bg(P::violet().opacity(0.6)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .text_color(P::text())
                            .child(name.to_string()),
                    )
                    .child(div().text_xs().text_color(P::flame()).child(line)),
            )
            .child(chip("☑ 3", false).when(decor_w.is_some(), |d| {
                d.relative().child(decor::by_tasks(0))
            }))
            .child(chip("show terminal", true))
    }

    /// The Chat shot's header width: 900 wide, less padding and the rail.
    const CHAT_HEADER_W: f32 = 900. - 20. - 220.;

    fn chat_view(id: &str, ink_override: Option<MeBubble>, rail_w: f32) -> impl IntoElement {
        // The selected row's pink wash and bar are Neon's; castle's frame is
        // its chain.
        let custom = ink_override
            .or(crate::theme::active().me_bubble)
            .filter(|_| neon());
        // Brian's bubble takes the override; every other follows the scheme.
        let ink_of = move |me: bool| match ink_override {
            Some(c) if me => crate::app::chat::me_ink(true, Some(c)),
            _ => crate::app::chat::active_me_ink(me),
        };
        let torches = torches();
        let (ask, reply) = if neon() {
            (
                "Steel is on the chat rail now. Run the shots?",
                "yes, neon and a flat one",
            )
        } else {
            (
                "Stone is on the chat rail now. Run the shots?",
                "yes, castle and a flat one",
            )
        };
        let row = |name: &str, on: bool, note: &str| {
            div()
                .px_3()
                .py_2()
                .flex()
                .justify_between()
                .text_xs()
                .relative()
                .when(on, |d| {
                    d.bg(crate::app::chat::selected_row_bg(custom))
                        .children(crate::app::chat::selected_row_bar(custom))
                        .children(crate::chain::frame())
                })
                .child(
                    div()
                        .text_color(if on { P::text() } else { P::text_dim() })
                        .child(name.to_string()),
                )
                .child(div().text_color(P::text_faint()).child(note.to_string()))
        };
        // Brian's bubble is the palette's `violet_dim` where the scheme has no
        // `me_bubble`, as in the app; one that has it paints as the app does.
        let bubble = |me: bool, text: &str| {
            let (fill, words) = crate::app::chat::bubble_fill(
                me,
                ink_of(me),
                P::violet_dim(),
                P::surface(),
                P::text(),
            );
            div().flex().when(me, |d| d.justify_end()).child(
                carve::worn_plate(div(), fill, Stock::Lg)
                    .max_w(px(320.))
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(words)
                    .map(|d| carve::bg(d, fill))
                    .child(text.to_string()),
            )
        };
        let rail = div()
            .flex_none()
            .w(px(rail_w))
            .h_full()
            .flex()
            .flex_col()
            .bg(P::bg_elevated())
            .border_r_1()
            .border_color(P::border())
            .map(|d| under(d, RAIL))
            .child(
                div()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(div().text_color(P::flame_dim()).child("✦"))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(P::text())
                                    .child("seance"),
                            ),
                    )
                    .child(
                        // The view switch, as `render_view_switch` draws it.
                        carve::plate(div(), P::bg(), Stock::Md, Cut::Slot)
                            .flex()
                            .p(px(if carve::on() { 3. } else { 2. }))
                            .gap(px(2.))
                            .when(carve::slick(), |d| carve::bg(d, P::bg()))
                            .when(!carve::on(), |d| d.border_1().border_color(P::border()))
                            .children(["Classic", "Stage", "Chat", "Wall"].map(|m| {
                                let on = m == "Chat";
                                let tab = div();
                                let tab = if on {
                                    carve::plate(tab, P::surface(), Stock::Sm, Cut::Control)
                                } else {
                                    carve::corners(tab, Stock::Sm)
                                };
                                tab.flex_1()
                                    .flex()
                                    .justify_center()
                                    .text_xs()
                                    .when(on, |d| carve::bg(d, P::surface()).text_color(P::flame()))
                                    .when(!on, |d| d.text_color(P::text_dim()))
                                    .child(m)
                            })),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(P::border())
                            .bg(P::bg())
                            .text_xs()
                            .text_color(P::text_faint())
                            .child("Search chats"),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .text_color(P::text_faint())
                    .child("ACTIVE"),
            )
            .child(row("l6-leveling", true, "now"))
            .child(row("rudle", false, "3m"))
            .child(row("riff-raiders", false, "1h"))
            .child(
                div()
                    .mt_auto()
                    .px_3()
                    .pb_2()
                    .text_xs()
                    .text_color(P::text_faint())
                    .child(format!("scheme: {id}")),
            )
            .when(torches, |d| {
                d.child(crate::app::chat::decor::by_mark(0))
                    .child(crate::app::chat::decor::rail_post(0))
            });
        let composer = div()
            .flex_none()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_t_1()
            .border_color(P::border())
            .map(|d| under(d, STRIP))
            .when(torches, |d| d.child(crate::app::chat::decor::over_send(0)))
            .child(
                div()
                    .flex()
                    .gap_1p5()
                    .children(["ok", "go", "status?"].map(|t| {
                        carve::plate(div(), P::surface(), Stock::Full, Cut::Control)
                            .px_2()
                            .py_0p5()
                            .when(carve::on(), |d| carve::bg(d, P::surface()))
                            .when(!carve::on(), |d| d.border_1().border_color(P::border()))
                            .text_xs()
                            .text_color(P::text_dim())
                            .child(t)
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        carve::plate(div(), P::surface(), Stock::Md, Cut::Control)
                            .px_2()
                            .py_1p5()
                            .map(|d| carve::bg(d, P::surface()))
                            .text_sm()
                            .child("📎"),
                    )
                    .child(
                        carve::plate(div(), P::bg(), Stock::Md, Cut::Slot)
                            .flex_1()
                            .px_2()
                            .py_1p5()
                            .when(!carve::on(), |d| d.border_1().border_color(P::border()))
                            .map(|d| carve::bg(d, P::bg()))
                            .text_sm()
                            .text_color(P::text_faint())
                            .child("Message the lead"),
                    )
                    .child(
                        carve::plate(div(), P::flame(), Stock::Md, Cut::Control)
                            .px_3()
                            .py_1p5()
                            .map(|d| carve::bg(d, P::flame()))
                            .text_sm()
                            .text_color(P::bg())
                            .child("Send"),
                    ),
            );
        div()
            .flex_1()
            .h_full()
            .flex()
            .overflow_hidden()
            .rounded_md()
            .bg(P::bg())
            .child(rail)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(chat_header(
                        torches.then_some(Torch::Working),
                        "l6-leveling",
                        torches.then_some(CHAT_HEADER_W),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .px_4()
                            .py_3()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when(carve::stone(), |d| {
                                d.relative().children(backdrop(carve::STONE))
                            })
                            .child(bubble(false, ask))
                            .child(bubble(true, reply))
                            .child(bubble(false, "Both written to shots/.")),
                    )
                    .child(composer),
            )
    }

    /// The thread's foot with a reply: the lead's ask, Brian's answer quoting
    /// it, and the quote bar over the composer strip, built with the app's own
    /// quote builders.
    fn chat_reply_panel(id: &str, hovered: bool, custom: Option<MeBubble>) -> impl IntoElement {
        use crate::app::chat::{bubble_fill, hex, me_ink, quote_block, reply_arrow, reply_bar};
        let ask = "Stone is on the chat rail now. Run the shots for castle, neon and one flat scheme, then post them here? The quote above a reply stops at three lines, so this long ask shows the cut.";
        let arrow = |me: bool| hovered.then(|| reply_arrow(i64::from(me)).opacity(1.));
        let bubble = |me: bool, quote: bool, text: &str| {
            let ink = match custom {
                Some(c) if me => me_ink(true, Some(c)),
                _ => crate::app::chat::active_me_ink(me),
            };
            let (fill, words) = bubble_fill(me, ink, P::violet_dim(), P::surface(), P::text());
            div()
                .flex()
                .items_center()
                .when(me, |d| d.justify_end())
                .when(me, |d| d.children(arrow(me)))
                .child(
                    carve::worn_plate(div(), fill, Stock::Lg)
                        .max_w(px(260.))
                        .px_3()
                        .py_2()
                        .text_sm()
                        .text_color(words)
                        .map(|d| carve::bg(d, fill))
                        .when(quote, |d| d.child(quote_block("lead", ask, ink)))
                        .child(text.to_string())
                        // A recolored bubble also shows its time and ticks,
                        // as the app draws them.
                        .when_some(ink, |d, i| {
                            d.child(
                                div()
                                    .mt_0p5()
                                    .flex()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(hex(i.soft))
                                    .child("9:41")
                                    .child(div().text_color(hex(i.soft)).child("✓"))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(hex(i.ink))
                                            .child("✓✓"),
                                    ),
                            )
                        }),
                )
                .when(!me, |d| d.children(arrow(me)))
        };
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_md()
            .bg(P::bg())
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(P::text_faint())
                    .child(format!("scheme: {id}")),
            )
            .child(
                div()
                    .flex_1()
                    .px_3()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .gap_2()
                    .pb_2()
                    .child(bubble(false, false, ask))
                    .child(bubble(true, true, "yes, all three")),
            )
            .child(reply_bar(
                "Both written to shots/.\nNeon looks right.",
                div().px_1().child("✕"),
            ))
            .child(
                div()
                    .flex_none()
                    .p_3()
                    .border_t_1()
                    .border_color(P::border())
                    .map(|d| under(d, STRIP))
                    .child(
                        div()
                            .px_2()
                            .py_1p5()
                            .rounded_md()
                            .border_1()
                            .border_color(P::border())
                            .bg(P::bg())
                            .text_sm()
                            .text_color(P::text_faint())
                            .child("Message the lead"),
                    ),
            )
    }

    /// Only the reply shot: `cargo test --bin seance chat_reply_shot -- --ignored`.
    #[test]
    #[ignore = "writes a PNG via headless Metal; run with --ignored"]
    fn chat_reply_shot() {
        let dir = std::env::var("SKIN_SHOTS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shots"));
        std::fs::create_dir_all(&dir).unwrap();
        render(
            Scene::ChatReply(vec!["candlelit", "castle", "neon-district"], false),
            1200.,
            460.,
            &dir.join("chat-reply.png"),
        );
    }

    /// The reply shot with every bubble hovered, so the arrows show:
    /// `cargo test --bin seance chat_reply_arrow_shot -- --ignored`.
    #[test]
    #[ignore = "writes a PNG via headless Metal; run with --ignored"]
    fn chat_reply_arrow_shot() {
        let dir = std::env::var("SKIN_SHOTS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shots"));
        std::fs::create_dir_all(&dir).unwrap();
        render(
            Scene::ChatReply(vec!["candlelit", "castle", "neon-district"], true),
            1200.,
            460.,
            &dir.join("chat-reply-arrow.png"),
        );
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
        let tubes = lamp() == Some(Lamp::Tube);
        let sd = crate::torch::seed("a");
        let rows: Vec<AnyElement> = states
            .iter()
            .map(|(state, name)| {
                // Tubes hold still between rare events, so start their row
                // a tick before the first one.
                let first = (0..64u64)
                    .find(|&f| {
                        crate::tube::texels(*state, f + 1, sd) != crate::tube::texels(*state, f, sd)
                    })
                    .unwrap_or(0);
                let start = if tubes { first } else { 0 };
                let frames = (start..start + 6).map(|f| {
                    div()
                        .p(px(6.))
                        .relative()
                        .bg(P::bg_elevated())
                        .children(backdrop(TITLE))
                        .child(crate::torch::element(*state, f, sd, 4.0))
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
                            .child(*name),
                    )
                    .children(frames)
                    .into_any_element()
            })
            .collect();
        let standing = div()
            .flex()
            .items_end()
            .gap(px(8.))
            .child(
                div()
                    .w(px(110.))
                    .text_sm()
                    .text_color(P::text())
                    .child("standing"),
            )
            .children((0..6u64).map(|f| {
                div()
                    .p(px(6.))
                    .relative()
                    .bg(P::bg_elevated())
                    .children(backdrop(TITLE))
                    .child(crate::torch::tall_element(
                        f,
                        crate::torch::seed("post"),
                        2.0,
                    ))
            }));
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(rows)
            .child(standing)
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
    fn tile_preview(id: &str, out: &std::path::Path) {
        let tex = seance_core::schemes::find(id)
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

    /// Neon District's two looks for Chat's boxes beside today's: the whole
    /// Chat view, a reply bubble with its quote, and the composer, per look.
    #[test]
    #[ignore = "writes PNGs via headless Metal; run with --ignored"]
    fn neon_slick_shots() {
        use seance_core::schemes::Carve;
        let dir = std::env::var("SKIN_SHOTS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("shots"));
        std::fs::create_dir_all(&dir).unwrap();
        let id = "neon-district";
        carve::TODAY.with(|t| t.set(true));
        render(
            Scene::Chat(id),
            900.,
            460.,
            &dir.join("neon-slick-today-chat.png"),
        );
        carve::TODAY.with(|t| t.set(false));
        for (look, name) in [(Carve::Glass, "glass"), (Carve::Hud, "hud")] {
            carve::LOOK.with(|l| l.set(Some(look)));
            let whole = dir.join(format!("neon-slick-{name}-chat.png"));
            render(Scene::Chat(id), 900., 460., &whole);
            render(
                Scene::ChatReply(vec![id], false),
                330.,
                460.,
                &dir.join(format!("neon-slick-{name}-reply.png")),
            );
            // The harness renders at 2x: crop the composer.
            image::open(&whole)
                .unwrap()
                .crop_imm(460, 690, 1320, 230)
                .save(dir.join(format!("neon-slick-{name}-composer.png")))
                .unwrap();
        }
        carve::LOOK.with(|l| l.set(None));
    }

    /// Castle's carved look beside today's: the whole Chat view each way, a
    /// reply bubble over the composer, the composer, and the narrowest rail.
    #[test]
    #[ignore = "writes PNGs via headless Metal; run with --ignored"]
    fn castle_carved_shots() {
        let dir = std::env::var("SKIN_SHOTS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("shots"));
        std::fs::create_dir_all(&dir).unwrap();
        let today = |on: bool| carve::TODAY.with(|t| t.set(on));
        today(true);
        render(
            Scene::Chat("castle"),
            900.,
            460.,
            &dir.join("castle-today-chat.png"),
        );
        render(
            Scene::ChatReply(vec!["castle"], false),
            330.,
            460.,
            &dir.join("castle-today-reply.png"),
        );
        today(false);
        let whole = dir.join("castle-carved-chat.png");
        render(Scene::Chat("castle"), 900., 460., &whole);
        render(
            Scene::ChatReply(vec!["castle"], false),
            330.,
            460.,
            &dir.join("castle-carved-reply.png"),
        );
        let narrow = dir.join("castle-carved-narrow-chat.png");
        render(Scene::ChatNarrow("castle"), 560., 460., &narrow);
        // The harness renders at 2x: crop the composer and the selected row.
        let crop = |from: &std::path::Path, to: &str, x: u32, y: u32, w: u32, h: u32| {
            image::open(from)
                .unwrap()
                .crop_imm(x, y, w, h)
                .save(dir.join(to))
                .unwrap();
        };
        crop(&whole, "castle-carved-composer.png", 460, 690, 1320, 230);
        crop(&whole, "castle-carved-row.png", 20, 270, 440, 150);
    }

    /// Castle's iron line around the selected row: the whole Chat view, a
    /// crop of the row (the harness renders at 2x), the narrowest sidebar and
    /// the row at the app's heights.
    #[test]
    #[ignore = "writes PNGs via headless Metal; run with --ignored"]
    fn castle_iron_line_shots() {
        let dir = std::env::var("SKIN_SHOTS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("shots"));
        std::fs::create_dir_all(&dir).unwrap();
        let whole = dir.join("castle-iron-line-chat.png");
        render(Scene::Chat("castle"), 900., 460., &whole);
        let crop = |from: &std::path::Path, to: &str, w: u32| {
            image::open(from)
                .unwrap()
                .crop_imm(20, 270, w, 150)
                .save(dir.join(to))
                .unwrap();
        };
        crop(&whole, "castle-iron-line-row.png", 440);
        let narrow = dir.join("castle-iron-line-narrow-chat.png");
        render(Scene::ChatNarrow("castle"), 560., 460., &narrow);
        crop(&narrow, "castle-iron-line-narrow-row.png", 320);
        render(
            Scene::ChainRows,
            270.,
            280.,
            &dir.join("castle-iron-line-heights.png"),
        );
    }

    /// Neon District's bubble ink options, each as the whole Chat view with
    /// the selected row, plus a tight crop of a quoted reply (the harness renders at 2x).
    #[test]
    #[ignore = "writes PNGs via headless Metal; run with --ignored"]
    fn neon_ink_shots() {
        let dir = std::env::var("SKIN_SHOTS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("shots"));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bg, ink) in [
            ("a", 0xFF2FB5, 0xFFFFFF),
            ("b", 0xD6168F, 0xFFFFFF),
            ("c", 0xC8128A, 0xFFF4E0),
        ] {
            render(
                Scene::ChatInk("neon-district", bg, ink),
                900.,
                460.,
                &dir.join(format!("neon-ink-{name}.png")),
            );
            let crop = dir.join(format!("neon-ink-{name}-reply.png"));
            render(
                Scene::ChatReplyInk("neon-district", bg, ink),
                330.,
                460.,
                &crop,
            );
        }
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
        render(
            Scene::Torches("castle"),
            560.,
            560.,
            &dir.join("torch-sheet.png"),
        );
        render(
            Scene::Chat("castle"),
            900.,
            460.,
            &dir.join("chat-castle.png"),
        );
        render(
            Scene::Chat("candlelit"),
            900.,
            460.,
            &dir.join("chat-candlelit.png"),
        );
        render(
            Scene::ChatHeaders,
            560.,
            300.,
            &dir.join("chat-header-torches.png"),
        );
        tile_preview("castle", &dir.join("castle-tile-3x3.png"));
        render(
            Scene::Chrome(vec!["neon-district"]),
            900.,
            420.,
            &dir.join("neon-classic.png"),
        );
        render(
            Scene::Chat("neon-district"),
            900.,
            460.,
            &dir.join("neon-chat.png"),
        );
        render(
            Scene::Torches("neon-district"),
            560.,
            560.,
            &dir.join("neon-tube-sheet.png"),
        );
        tile_preview("neon-district", &dir.join("neon-tile-3x3.png"));
    }
}
