//! The setup kit's three cards: the first-run tiles, the checklist with
//! switches (Seance menu > Set up your Claude…) and the Make it yours hub.
//! The on/off logic is `crate::kit`; this file only draws and routes clicks.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{div, prelude::*, px, AnyElement, Context, Window};
use gpui_component::StyledExt as _;

use crate::control::ControlRequest;
use crate::kit::{self, Item, Status, ITEMS};
use crate::runtime::protocol::GuiRequest;
use crate::theme::SeancePalette;

use super::SeanceApp;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum KitView {
    #[default]
    Off,
    Tiles,
    Checklist,
    Hub,
}

static REQUEST: AtomicU8 = AtomicU8::new(0);

/// A menu handler has no app handle: it raises this, the next render opens it.
pub(crate) fn request(view: KitView) {
    REQUEST.store(view as u8 + 1, Ordering::SeqCst);
}

fn take_request() -> Option<KitView> {
    match REQUEST.swap(0, Ordering::SeqCst) {
        2 => Some(KitView::Tiles),
        3 => Some(KitView::Checklist),
        4 => Some(KitView::Hub),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    Theme,
    Font,
    Sound,
}

#[derive(Default)]
pub(super) struct KitUi {
    view: KitView,
    statuses: Vec<Status>,
    error: Option<String>,
    picker: Option<Picker>,
}

/// The four ask-Claude chips in the hub.
const PROMPTS: [&str; 4] = [
    "Make me a rule: always show me a screenshot first",
    "Make a skill for my weekly status update",
    "Make the sidebar text bigger",
    "Every Monday, summarize what I shipped",
];

/// "4 of 5 on": the switches that can be on, not the coming-soon one.
fn on_count(statuses: &[Status]) -> (usize, usize) {
    let live = statuses
        .iter()
        .filter(|s| **s != Status::ComingSoon)
        .count();
    (statuses.iter().filter(|s| **s == Status::On).count(), live)
}

impl SeanceApp {
    fn kit_refresh(&mut self) {
        let env = kit::Env::real();
        self.kit.statuses = ITEMS.iter().map(|i| kit::status(*i, &env)).collect();
    }

    pub(super) fn kit_open(&mut self, view: KitView, cx: &mut Context<Self>) {
        self.kit_refresh();
        self.kit.view = view;
        self.kit.error = None;
        self.kit.picker = None;
        cx.notify();
    }

    /// First run on a fresh install shows the tiles. An install that upgrades
    /// into this version never does (see `kit::first_run`). Runs once.
    pub(super) fn start_kit_first_run(&mut self, cx: &mut Context<Self>) {
        if self.kit_checked || self.empty_window {
            return;
        }
        self.kit_checked = true;
        if kit::first_run(&kit::Env::real(), crate::seance_custom::load_created()) {
            self.kit_open(KitView::Tiles, cx);
        }
    }

    /// Menu requests, picked up by one window.
    pub(super) fn poll_kit_request(&mut self, cx: &mut Context<Self>) {
        if self.empty_window {
            return;
        }
        if let Some(view) = take_request() {
            self.kit_open(view, cx);
        }
    }

    fn kit_toggle(&mut self, item: Item, cx: &mut Context<Self>) {
        let env = kit::Env::real();
        let result = match kit::status(item, &env) {
            Status::On => kit::off(item, &env),
            Status::Off => kit::on(item, &env),
            _ => return,
        };
        self.kit.error = result.err();
        self.kit_refresh();
        cx.notify();
    }

    fn kit_close(&mut self, cx: &mut Context<Self>) {
        self.kit.view = KitView::Off;
        cx.notify();
    }

    pub(super) fn render_kit(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let body = match self.kit.view {
            KitView::Off => return None,
            KitView::Tiles => self.kit_tiles(cx),
            KitView::Checklist => self.kit_checklist(cx),
            KitView::Hub => self.kit_hub(cx),
        };
        Some(
            div()
                .id("kit-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.5))
                .occlude()
                .child(body)
                .into_any_element(),
        )
    }

    fn kit_card(&self) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .rounded_lg()
            .border_1()
            .border_color(SeancePalette::flame_dim())
            .bg(SeancePalette::bg_elevated())
    }

    fn kit_done(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .id("kit-done")
            .px_4()
            .py_1()
            .rounded_md()
            .text_sm()
            .font_semibold()
            .cursor_pointer()
            .bg(SeancePalette::flame())
            .text_color(SeancePalette::bg())
            .child("Done")
            .on_click(cx.listener(|this, _, _, cx| this.kit_close(cx)))
            .into_any_element()
    }

    fn kit_status(&self, i: usize) -> Status {
        self.kit.statuses.get(i).copied().unwrap_or(Status::Off)
    }

    fn kit_error_line(&self) -> Option<AnyElement> {
        self.kit.error.as_ref().map(|e| {
            div()
                .text_xs()
                .text_color(SeancePalette::danger())
                .child(e.clone())
                .into_any_element()
        })
    }

    /// First run: six tiles. Tap one to turn it on or off.
    fn kit_tiles(&self, cx: &Context<Self>) -> AnyElement {
        let tiles = ITEMS.iter().enumerate().map(|(i, item)| {
            let item = *item;
            let status = self.kit_status(i);
            let live = status != Status::ComingSoon;
            let t = div()
                .id(("kit-tile", i))
                .w(px(230.))
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(if status == Status::On {
                    SeancePalette::flame_dim()
                } else {
                    SeancePalette::border()
                })
                .bg(SeancePalette::surface())
                .when(!live, |d| d.opacity(0.5))
                .when(live, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(SeancePalette::border()))
                        .on_click(cx.listener(move |this, _, _, cx| this.kit_toggle(item, cx)))
                })
                .child(div().text_color(SeancePalette::flame()).child(item.glyph()))
                .child(
                    div()
                        .font_semibold()
                        .text_color(SeancePalette::text())
                        .child(item.title()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(SeancePalette::text_dim())
                        .child(item.why()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if status == Status::On {
                            SeancePalette::flame()
                        } else {
                            SeancePalette::text_faint()
                        })
                        .child(status.label()),
                );
            t.into_any_element()
        });
        self.kit_card()
            .w(px(760.))
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .text_color(SeancePalette::flame())
                    .child("Set up your Claude"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("Six extras. Tap one to see it and turn it on."),
            )
            .child(div().flex().flex_row().flex_wrap().gap_3().children(tiles))
            .children(self.kit_error_line())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .child(self.kit_done(cx)),
            )
            .into_any_element()
    }

    /// The same six as rows with switches, from the Seance menu.
    fn kit_checklist(&self, cx: &Context<Self>) -> AnyElement {
        let rows = ITEMS.iter().enumerate().map(|(i, item)| {
            let item = *item;
            let status = self.kit_status(i);
            let on = status == Status::On;
            let live = matches!(status, Status::On | Status::Off);
            let switch = div()
                .id(("kit-switch", i))
                .flex_none()
                .w(px(38.))
                .h(px(22.))
                .p(px(3.))
                .rounded_full()
                .flex()
                .items_center()
                .when(on, |d| d.justify_end().bg(SeancePalette::flame_dim()))
                .when(!on, |d| d.bg(SeancePalette::border()))
                .when(!live, |d| d.opacity(0.5))
                .when(live, |d| {
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.kit_toggle(item, cx)))
                })
                .child(
                    div()
                        .w(px(16.))
                        .h(px(16.))
                        .rounded_full()
                        .bg(SeancePalette::text()),
                );
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .py_2()
                .border_b_1()
                .border_color(SeancePalette::border())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_color(SeancePalette::text()).child(item.title()))
                        .child(
                            div()
                                .text_sm()
                                .text_color(SeancePalette::text_faint())
                                .child(match status {
                                    Status::AlreadySetUp | Status::ComingSoon => {
                                        format!("{}. {}", status.label(), item.why())
                                    }
                                    _ => item.why().to_string(),
                                }),
                        ),
                )
                .child(switch)
        });
        self.kit_card()
            .w(px(520.))
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .text_color(SeancePalette::flame())
                    .child("Set up your Claude"),
            )
            .child(div().text_sm().text_color(SeancePalette::text_dim()).child(
                "Extras that make Claude work the way you do. Turn on what you like; \
                 you can change these any time.",
            ))
            .child(div().flex().flex_col().children(rows))
            .children(self.kit_error_line())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(SeancePalette::text_faint())
                            .child("Nothing is shared. All of it lives on this Mac."),
                    )
                    .child(self.kit_done(cx)),
            )
            .into_any_element()
    }

    fn hub_row(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .py_2()
            .border_b_1()
            .border_color(SeancePalette::border())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(div().text_color(SeancePalette::text()).child(label))
            .child(
                div()
                    .text_color(SeancePalette::flame())
                    .child(format!("{value} ›")),
            )
            .into_any_element()
    }

    /// The options under an open picker row.
    fn hub_options(&self, which: Picker, cx: &Context<Self>) -> AnyElement {
        let opt = |id: (&'static str, usize), label: String, ticked: bool| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .rounded_md()
                .hover(|s| s.bg(SeancePalette::surface()))
                .text_color(if ticked {
                    SeancePalette::flame()
                } else {
                    SeancePalette::text_dim()
                })
                .child(format!("{}{label}", if ticked { "● " } else { "   " }))
        };
        let col = div().flex().flex_col().pb_2();
        match which {
            Picker::Theme => col.children(seance_core::schemes::SCHEMES.iter().enumerate().map(
                |(i, s)| {
                    let id = s.id;
                    opt(
                        ("kit-theme", i),
                        s.label.to_string(),
                        crate::theme::active().id == id,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let _ = this.client.send(GuiRequest::Ctl {
                            req: ControlRequest::Theme {
                                id: Some(id.to_string()),
                                scope: None,
                                from: None,
                            },
                        });
                        cx.notify();
                    }))
                },
            )),
            Picker::Font => {
                let saved = crate::ui_font::load();
                col.children(crate::ui_font::FONTS.iter().enumerate().map(|(i, f)| {
                    let id = f.id;
                    opt(
                        ("kit-font", i),
                        f.label.to_string(),
                        crate::ui_font::ticked(saved, id),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::pick_ui_font(id, cx);
                    }))
                }))
            }
            Picker::Sound => {
                let saved = crate::lead_sound::load();
                col.children(
                    crate::lead_sound::ROWS
                        .iter()
                        .enumerate()
                        .map(|(i, (label, _))| {
                            opt(
                                ("kit-sound", i),
                                label.to_string(),
                                crate::lead_sound::ticked(saved.as_ref(), i),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| match i {
                                    // Custom…: the helper circle picks one with you.
                                    4 => {
                                        this.kit.view = KitView::Off;
                                        crate::seance_custom::request_open();
                                        cx.notify();
                                    }
                                    5 => crate::pick_custom_file(cx),
                                    _ => crate::pick_lead_sound_tag(crate::SOUND_TAGS[i], cx),
                                },
                            ))
                        }),
                )
            }
        }
        .into_any_element()
    }

    fn kit_hub(&self, cx: &Context<Self>) -> AnyElement {
        let font_label = {
            let saved = crate::ui_font::load();
            crate::ui_font::FONTS
                .iter()
                .find(|f| crate::ui_font::ticked(saved, f.id))
                .map_or("System", |f| f.label)
        };
        let sound_label = {
            let saved = crate::lead_sound::load();
            crate::lead_sound::ROWS
                .iter()
                .enumerate()
                .find(|(i, _)| crate::lead_sound::ticked(saved.as_ref(), *i))
                .map_or("Thump", |(_, (l, _))| l)
        };
        let (n, of) = on_count(&self.kit.statuses);
        let pick = |p: Picker| {
            move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                this.kit.picker = if this.kit.picker == Some(p) {
                    None
                } else {
                    Some(p)
                };
                cx.notify();
            }
        };
        let section = |t: &'static str| {
            div()
                .pt_2()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .child(t)
        };
        let mut look = div().flex().flex_col();
        for (which, id, label, value) in [
            (
                Picker::Theme,
                "kit-row-theme",
                "Theme",
                crate::theme::active().label.to_string(),
            ),
            (Picker::Font, "kit-row-font", "Font", font_label.to_string()),
            (
                Picker::Sound,
                "kit-row-sound",
                "Lead reply sound",
                sound_label.to_string(),
            ),
        ] {
            look = look.child(self.hub_row(id, label, value, pick(which), cx));
            if self.kit.picker == Some(which) {
                look = look.child(self.hub_options(which, cx));
            }
        }
        let left = self
            .kit_card()
            .w(px(380.))
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .text_color(SeancePalette::flame())
                    .child("Make it yours"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("Seance Custom"),
            )
            .child(section("LOOK"))
            .child(look)
            .child(section("SETUP"))
            .child(self.hub_row(
                "kit-row-setup",
                "Set up your Claude",
                format!("{n} of {of} on"),
                |this, _, cx| this.kit_open(KitView::Checklist, cx),
                cx,
            ));
        let chips = PROMPTS.iter().enumerate().map(|(i, p)| {
            let p: &'static str = p;
            div()
                .id(("kit-chip", i))
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(SeancePalette::border())
                .text_sm()
                .text_color(SeancePalette::text())
                .cursor_pointer()
                .hover(|s| s.bg(SeancePalette::surface()))
                .child(p)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.open_custom_with(p, window, cx) {
                        this.kit.view = KitView::Off;
                    } else {
                        this.kit.error = Some("The Seance Custom circle is gone.".into());
                    }
                    cx.notify();
                }))
        });
        let right = self
            .kit_card()
            .w(px(400.))
            .child(
                div()
                    .text_xs()
                    .text_color(SeancePalette::text_faint())
                    .child("ASK CLAUDE TO CHANGE ANYTHING"),
            )
            .children(chips)
            .children(self.kit_error_line())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .child(self.kit_done(cx)),
            );
        div()
            .flex()
            .flex_row()
            .gap_3()
            .items_start()
            .child(left)
            .child(right)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_requests_come_back_once_and_in_order() {
        let _ = take_request();
        assert_eq!(take_request(), None);
        request(KitView::Hub);
        assert_eq!(take_request(), Some(KitView::Hub));
        assert_eq!(take_request(), None);
        request(KitView::Checklist);
        assert_eq!(take_request(), Some(KitView::Checklist));
    }

    #[test]
    fn the_count_leaves_out_coming_soon() {
        use Status::*;
        assert_eq!(
            on_count(&[ComingSoon, On, On, AlreadySetUp, Off, On]),
            (3, 5)
        );
    }
}
