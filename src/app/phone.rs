//! Settings > "Use Seance on your phone": a short guide, each step checking
//! itself with a green tick. Invoke (the phone chat) runs inside the app; the
//! phone reaches it over the owner's own Tailscale. The checks and the
//! `tailscale serve` commands run on the daemon (`invoke.rs`); this is the card.

use gpui::{div, prelude::*, px, Context, SharedString, Window};
use gpui_component::StyledExt as _;

use crate::invoke::Guide;
use crate::theme::SeancePalette;

use super::SeanceApp;

/// Under the QR code: a phone that is not on the tailnet can't open the link.
pub(super) const SCAN_NOTE: &str =
    "Scan with a phone listed in step 3. Other phones can't open it.";

/// The Tailscale download page for the Mac.
const DOWNLOAD_URL: &str = "https://tailscale.com/download/mac";

#[derive(Default)]
pub(super) struct Phone {
    pub open: bool,
    pub guide: Option<Guide>,
    /// A check or a button is running.
    pub busy: bool,
    /// Step (c) can't be detected unless the phone shows in the tailnet: the
    /// person says it is done.
    pub phone_done: bool,
    /// What the last button said (an error, or the one-time Serve prompt).
    pub note: Option<String>,
    /// The page that turns Serve/HTTPS on for this tailnet, once, when asked.
    pub enable_url: Option<String>,
    /// Which phone's install steps are showing.
    pub tab: PhoneTab,
    /// The guide's scroll, which the "More below" cue reads.
    pub scroll: gpui::ScrollHandle,
}

/// The install steps come in two flavours: iPhone first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PhoneTab {
    #[default]
    IPhone,
    Android,
}

const IPHONE_STEPS: [&str; 5] = [
    "Scan the QR code above with the Camera app and open the link in Safari. \
     Not Chrome: an iPhone only installs it from Safari.",
    "Tap the Share button, then Add to Home Screen, then Add.",
    "Open Invoke from its new icon on your Home Screen.",
    "Tap Allow when it asks about notifications.",
    "Notifications need iOS 16.4 or newer, and only work from the Home Screen icon.",
];

const ANDROID_STEPS: [&str; 4] = [
    "Scan the QR code above with your camera and open the link in Chrome.",
    "Tap Install app (or the three dots, then Add to Home screen).",
    "Open Invoke from its new icon.",
    "Tap Allow when it asks about notifications.",
];

/// The words of the "Put Invoke on your phone" step for one tab: the numbered
/// how-to, then what the tick is waiting for.
pub(super) fn install_text(tab: PhoneTab, g: &Guide) -> String {
    let steps: &[&str] = match tab {
        PhoneTab::IPhone => &IPHONE_STEPS,
        PhoneTab::Android => &ANDROID_STEPS,
    };
    let mut lines: Vec<String> = Vec::new();
    for (i, st) in steps.iter().enumerate() {
        // The last iPhone line is a note, not a step.
        if matches!(tab, PhoneTab::IPhone) && i == steps.len() - 1 {
            lines.push(format!("Note: {st}"));
        } else {
            lines.push(format!("{}. {st}", i + 1));
        }
    }
    lines.push(if on_phone(g) {
        "Invoke is on your phone and notifications are on.".into()
    } else {
        "This ticks when your phone has opened Invoke and you have tapped Allow.".into()
    });
    lines.join("\n")
}

/// Has a phone opened Invoke and turned notifications on? That is the one
/// thing a phone leaves behind (a stored push subscription), so it is the tick.
pub(super) fn on_phone(g: &Guide) -> bool {
    g.subscribed > 0
}

/// Which step is the first one not done, 0 based (5: all done). Pure, so the
/// order of the guide is tested.
pub(super) fn next_step(g: &Guide, phone_done: bool) -> usize {
    if !g.installed {
        0
    } else if !g.signed_in {
        1
    } else if !(phone_done || g.phones_online > 0) {
        2
    } else if !g.link_on {
        3
    } else {
        4
    }
}

/// A button on a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Act {
    GetTailscale,
    OpenTailscale,
    Done,
    LinkOn { busy: bool },
    LinkOff,
    Enable,
}

/// One step of the guide, as data: its tick, its words, and its buttons. The
/// card draws the buttons UNDER the step's text, inside its column, so they can
/// never sit beside text that has not wrapped (and be pushed off the card).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Step {
    pub done: bool,
    pub title: &'static str,
    pub body: String,
    pub actions: Vec<Act>,
    /// A line under the buttons: an error, or Tailscale's one-time prompt.
    pub note: Option<String>,
}

/// Step 3's words: each phone on the owner's account by name and system, so
/// the phone in your hand can be told from another one that is online.
pub(super) fn phones_text(phones: &[crate::invoke::PhoneInfo], phone_done: bool) -> String {
    let mut lines: Vec<String> = phones
        .iter()
        .map(|p| {
            if p.online {
                format!("{} ({}) is online.", p.name, p.os)
            } else {
                format!("{} ({}), offline: open Tailscale on it.", p.name, p.os)
            }
        })
        .collect();
    if !phones.iter().any(|p| p.online) {
        lines.insert(0, "No phone is on your Tailscale yet.".into());
        if !phone_done {
            lines.push(
                "Install Tailscale on your phone and sign in with the same account. If \
                 it still does not show here, press Done once it is signed in."
                    .into(),
            );
        }
    }
    lines.join("\n")
}

/// The four steps (the QR step is drawn after them once the link is on).
pub(super) fn steps(
    g: &Guide,
    phone_done: bool,
    busy: bool,
    note: Option<&str>,
    enable_page: bool,
) -> Vec<Step> {
    let phone_ok = phone_done || g.phones_online > 0;
    let mut link_actions = Vec::new();
    if !g.link_on && g.signed_in && g.invoke_running {
        link_actions.push(Act::LinkOn { busy });
    }
    if g.link_on {
        link_actions.push(Act::LinkOff);
    }
    if enable_page {
        link_actions.push(Act::Enable);
    }
    vec![
        Step {
            done: g.installed,
            title: "1. Tailscale on this Mac",
            body: if g.installed {
                "Tailscale is installed.".into()
            } else {
                "Install the Tailscale app, then come back here.".into()
            },
            actions: if g.installed {
                vec![]
            } else {
                vec![Act::GetTailscale]
            },
            note: None,
        },
        Step {
            done: g.signed_in,
            title: "2. Signed in on this Mac",
            body: match (&g.login, g.signed_in) {
                (Some(l), true) => format!("Signed in as {l}."),
                _ => "Open Tailscale from the menu bar and sign in.".into(),
            },
            actions: if g.installed && !g.signed_in {
                vec![Act::OpenTailscale]
            } else {
                vec![]
            },
            note: None,
        },
        Step {
            done: phone_ok,
            title: "3. Tailscale on your phone",
            body: phones_text(&g.phone_list, phone_done),
            actions: if g.phones_online == 0 && !phone_done {
                vec![Act::Done]
            } else {
                vec![]
            },
            note: None,
        },
        Step {
            done: g.link_on,
            title: "4. Turn on the phone link",
            body: if g.link_on {
                "The phone link is on.".into()
            } else if !g.invoke_running {
                "Seance's phone chat is not running yet. Give it a moment, then check again.".into()
            } else {
                "Makes Seance reachable from your phone over HTTPS (needed for the app and \
                 for notifications). Tailscale may ask once to turn on HTTPS for your account."
                    .into()
            },
            actions: link_actions,
            note: note.map(str::to_string),
        },
    ]
}

impl SeanceApp {
    pub(super) fn open_phone_guide(&mut self, cx: &mut Context<Self>) {
        self.phone.open = true;
        self.phone.note = None;
        cx.notify();
        self.refresh_phone(cx);
        // Keep the ticks current while the card is open (a sign-in or an
        // install happens in another app).
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            let Some(this) = this.upgrade() else { break };
            let open = this.update(cx, |app: &mut SeanceApp, cx| {
                if app.phone.open {
                    app.refresh_phone(cx);
                }
                app.phone.open
            });
            if !open {
                break;
            }
        })
        .detach();
    }

    fn refresh_phone(&mut self, cx: &mut Context<Self>) {
        if self.phone.busy {
            return;
        }
        let client = std::sync::Arc::clone(&self.client);
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_executor()
                .spawn(async move { client.invoke_guide() })
                .await;
            let _ = this.update(cx, |app: &mut SeanceApp, cx| {
                if let Ok(v) = got {
                    if let Ok(g) = serde_json::from_value::<Guide>(v) {
                        app.phone.guide = Some(g);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn phone_link(&mut self, on: bool, cx: &mut Context<Self>) {
        self.phone.busy = true;
        self.phone.note = None;
        self.phone.enable_url = None;
        cx.notify();
        let client = std::sync::Arc::clone(&self.client);
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_executor()
                .spawn(async move { client.invoke_serve(on) })
                .await;
            let _ = this.update(cx, |app: &mut SeanceApp, cx| {
                app.phone.busy = false;
                match got {
                    Ok(v) => match v["outcome"].as_str() {
                        Some("needs_enable") => {
                            app.phone.enable_url = v["url"].as_str().map(str::to_string);
                            app.phone.note = Some(
                                "Tailscale needs HTTPS turned on once for your account (phone \
                                 notifications need it). Open the page, press Enable, then \
                                 press Turn on phone link again."
                                    .into(),
                            );
                        }
                        Some("failed") => {
                            app.phone.note = Some(
                                v["message"]
                                    .as_str()
                                    .unwrap_or("Tailscale could not do that.")
                                    .to_string(),
                            );
                        }
                        _ => {}
                    },
                    Err(e) => app.phone.note = Some(format!("{e:#}")),
                }
                cx.notify();
                app.refresh_phone(cx);
            });
        })
        .detach();
    }

    pub(super) fn render_phone(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.phone.open {
            return None;
        }
        let g = self.phone.guide.as_ref();
        let step = g.map_or(0, |g| next_step(g, self.phone.phone_done));
        let tick = |done: bool| {
            div()
                .w(px(18.))
                .flex_none()
                .text_sm()
                .text_color(if done {
                    SeancePalette::success()
                } else {
                    SeancePalette::text_faint()
                })
                .child(if done { "✓" } else { "○" })
        };
        let button = |id: &'static str, label: SharedString, primary: bool| {
            let b = div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(SeancePalette::border()));
            if primary {
                b.bg(SeancePalette::surface())
                    .text_color(SeancePalette::flame())
                    .child(label)
            } else {
                b.text_color(SeancePalette::text_faint()).child(label)
            }
        };
        // One step: the tick, then a column holding the title, the text and
        // (under the text, never beside it) the step's buttons. The column may
        // shrink below its content (`min_w 0`), so the text wraps inside the
        // card instead of running off its right edge.
        let row = |done: bool,
                   title: &'static str,
                   body: String,
                   note: Option<String>,
                   actions: Vec<gpui::AnyElement>| {
            let mut col = div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .w_full()
                        .text_sm()
                        .font_semibold()
                        .text_color(SeancePalette::text())
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(SeancePalette::text_dim())
                        .child(body),
                );
            if let Some(note) = note {
                col = col.child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(SeancePalette::flame())
                        .child(note),
                );
            }
            if !actions.is_empty() {
                col = col.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .pt_1()
                        .children(actions),
                );
            }
            div()
                .w_full()
                .flex()
                .flex_row()
                .gap_2()
                .child(tick(done))
                .child(col)
        };
        // The guide's steps; `phone_shell` frames them in a card that fits the
        // window and scrolls. The padding is here, inside the scroll, so the
        // last line keeps its room below.
        let mut card = div().flex().flex_col().gap_3().p_4().child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(SeancePalette::text())
                .child("Use Seance on your phone"),
        );
        let Some(g) = g else {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("Checking..."),
            );
            return Some(self.phone_shell(card, window, cx));
        };
        if !g.bundled {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("This build of Seance does not include the phone chat."),
            );
            return Some(self.phone_shell(card, window, cx));
        }
        card = card.child(
            div()
                .w_full()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .child(
                    "Your phone reaches Seance over Tailscale, a private network that \
                     is free for one person. Only you can open it.",
                ),
        );
        let enable_url = self.phone.enable_url.clone();
        let steps = steps(
            g,
            self.phone.phone_done,
            self.phone.busy,
            self.phone.note.as_deref(),
            enable_url.is_some(),
        );
        for (i, st) in steps.into_iter().enumerate() {
            let buttons: Vec<gpui::AnyElement> =
                st.actions
                    .iter()
                    .map(|act| match *act {
                        Act::GetTailscale => {
                            button("phone-download", "Get Tailscale".into(), step == i)
                                .on_click(cx.listener(|_, _, _, _| {
                                    crate::sysopen::open_detached(DOWNLOAD_URL)
                                }))
                                .into_any_element()
                        }
                        Act::OpenTailscale => {
                            button("phone-open-ts", "Open Tailscale".into(), step == i)
                                .on_click(cx.listener(|_, _, _, _| {
                                    crate::sysopen::open_detached("tailscale://")
                                }))
                                .into_any_element()
                        }
                        Act::Done => button("phone-done", "Done".into(), step == i)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.phone.phone_done = true;
                                cx.notify();
                            }))
                            .into_any_element(),
                        Act::LinkOn { busy } => button(
                            "phone-link-on",
                            if busy {
                                "Working..."
                            } else {
                                "Turn on phone link"
                            }
                            .into(),
                            step == i,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.phone.busy {
                                this.phone_link(true, cx);
                            }
                        }))
                        .into_any_element(),
                        Act::LinkOff => button("phone-link-off", "Turn off".into(), false)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.phone.busy {
                                    this.phone_link(false, cx);
                                }
                            }))
                            .into_any_element(),
                        Act::Enable => {
                            let url = enable_url.clone().unwrap_or_default();
                            button("phone-enable", "Open the Tailscale page".into(), true)
                                .on_click(cx.listener(move |_, _, _, _| {
                                    crate::sysopen::open_detached(&url)
                                }))
                                .into_any_element()
                        }
                    })
                    .collect();
            card = card.child(row(st.done, st.title, st.body, st.note, buttons));
        }
        // Step 5: the address and its QR code.
        if let (true, Some(url)) = (g.link_on, g.url.clone()) {
            card = card.child(row(
                false,
                "5. Open it on your phone",
                "Scan this with your phone's camera. The next step puts it on your Home Screen."
                    .into(),
                None,
                vec![],
            ));
            card = card.child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(qr(&url))
                    .child(
                        div()
                            .id("phone-url")
                            .max_w_full()
                            .text_xs()
                            .text_color(SeancePalette::text())
                            .child(url),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_xs()
                            .text_color(SeancePalette::text_dim())
                            .child(SCAN_NOTE),
                    ),
            );
        }
        // Step 6: put Invoke on the phone, with a tab for each kind.
        if g.link_on && g.url.is_some() {
            let tab = self.phone.tab;
            let tabs = vec![
                button("phone-tab-iphone", "iPhone".into(), tab == PhoneTab::IPhone)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.phone.tab = PhoneTab::IPhone;
                        cx.notify();
                    }))
                    .into_any_element(),
                button(
                    "phone-tab-android",
                    "Android".into(),
                    tab == PhoneTab::Android,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.phone.tab = PhoneTab::Android;
                    cx.notify();
                }))
                .into_any_element(),
            ];
            card = card.child(row(
                on_phone(g),
                "6. Put Invoke on your phone",
                install_text(tab, g),
                None,
                tabs,
            ));
        }
        card = card.child(
            div()
                .w_full()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .child("Your phone reaches Seance only while this Mac is awake."),
        );
        Some(self.phone_shell(card, window, cx))
    }

    /// The card: sized to the window, the steps scrolling inside it with a
    /// "More below" cue while some are out of sight, and Close inside its
    /// bottom-right corner.
    fn phone_shell(
        &self,
        content: gpui::Div,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let window_h = f32::from(window.viewport_size().height);
        let scroll = &self.phone.scroll;
        let (offset, max) = (scroll.offset(), scroll.max_offset());
        let more = more_below(f32::from(offset.y), f32::from(max.y));
        let bg = SeancePalette::bg_elevated();
        div()
            .id("phone-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.5))
            .occlude()
            .child(
                div()
                    .id("phone-card")
                    .w(px(480.))
                    .max_h(px(card_max_height(window_h)))
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .border_1()
                    .border_color(SeancePalette::flame_dim())
                    .bg(bg)
                    .overflow_hidden()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id("phone-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .track_scroll(scroll)
                                    // The cue reads the offset, which a wheel
                                    // turn changes after this frame.
                                    .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                                    .child(content),
                            )
                            .when(more, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .bottom_0()
                                        .left_0()
                                        .right_0()
                                        .h(px(44.))
                                        .flex()
                                        .items_end()
                                        .justify_center()
                                        .pb_1()
                                        .bg(gpui::linear_gradient(
                                            180.,
                                            gpui::linear_color_stop(bg.opacity(0.), 0.),
                                            gpui::linear_color_stop(bg, 0.75),
                                        ))
                                        .text_xs()
                                        .text_color(SeancePalette::flame())
                                        .child("More below \u{2193}"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .px_4()
                            .pb_3()
                            .pt_1()
                            .child(
                                div()
                                    .id("phone-close")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .text_sm()
                                    .cursor_pointer()
                                    .bg(SeancePalette::surface())
                                    .text_color(SeancePalette::text())
                                    .hover(|s| s.bg(SeancePalette::border()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.phone.open = false;
                                        cx.notify();
                                    }))
                                    .child("Close"),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// The card is at most this tall: the window less a margin, never over the
/// old fixed height.
fn card_max_height(window_h: f32) -> f32 {
    (window_h - 48.).clamp(240., 640.)
}

/// Whether steps lie below the visible part: the guide overflows
/// (`max_y` is how far it can scroll) and is not at its end (`offset_y` runs
/// from 0 down to `-max_y`).
fn more_below(offset_y: f32, max_y: f32) -> bool {
    max_y > 1. && -offset_y < max_y - 1.
}

/// The QR code for `text`, drawn as a grid of dark and light squares. White
/// background with a quiet margin, so any phone camera reads it.
fn qr(text: &str) -> gpui::AnyElement {
    use qrcode::types::Color;
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return div().into_any_element();
    };
    let n = code.width();
    let colors = code.to_colors();
    let cell = px(4.);
    let mut grid = div()
        .flex()
        .flex_col()
        .p(px(12.))
        .bg(gpui::white())
        .rounded_md();
    for y in 0..n {
        let mut row = div().flex().flex_row();
        for x in 0..n {
            let dark = colors[y * n + x] == Color::Dark;
            row = row.child(div().w(cell).h(cell).bg(if dark {
                gpui::black()
            } else {
                gpui::white()
            }));
        }
        grid = grid.child(row);
    }
    grid.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guide() -> Guide {
        Guide {
            bundled: true,
            invoke_running: true,
            port: 9667,
            ..Default::default()
        }
    }

    #[test]
    fn the_more_below_cue_shows_until_the_end_is_reached() {
        // Fits: nothing to scroll. Overflows: shown at the top and middle,
        // gone at the end.
        assert!(!more_below(0., 0.));
        assert!(more_below(0., 220.));
        assert!(more_below(-100., 220.));
        assert!(!more_below(-220., 220.));
        assert!(!more_below(-219.5, 220.));
    }

    #[test]
    fn the_card_fits_the_window() {
        assert_eq!(card_max_height(890.), 640.);
        assert_eq!(card_max_height(500.), 452.);
        assert_eq!(card_max_height(100.), 240.);
    }

    #[test]
    fn the_guide_points_at_the_first_step_not_done() {
        let mut g = guide();
        assert_eq!(next_step(&g, false), 0); // Tailscale not installed
        g.installed = true;
        assert_eq!(next_step(&g, false), 1); // not signed in
        g.signed_in = true;
        assert_eq!(next_step(&g, false), 2); // phone not seen
                                             // A phone online ticks step 3 by itself; "Done" does too.
        g.phones_online = 1;
        assert_eq!(next_step(&g, false), 3);
        g.phones_online = 0;
        assert_eq!(next_step(&g, true), 3);
        g.link_on = true;
        assert_eq!(next_step(&g, true), 4);
    }

    /// Layout regression (0.28.2): the step buttons sat to the right of text
    /// that did not wrap, off the card, so "Turn on phone link" was invisible.
    /// Every button now belongs to its step and is drawn under the step's text;
    /// this pins which step owns which button.
    #[test]
    fn every_button_belongs_to_a_step_and_so_sits_under_its_text() {
        // Nothing done yet: only step 1 offers a button.
        let g = guide();
        let st = steps(&g, false, false, None, false);
        assert_eq!(st.len(), 4);
        assert_eq!(st[0].actions, [Act::GetTailscale]);
        assert!(st[1].actions.is_empty() && st[3].actions.is_empty());
        // Installed, signed out: Open Tailscale, on step 2.
        let g = Guide {
            installed: true,
            ..guide()
        };
        assert_eq!(
            steps(&g, false, false, None, false)[1].actions,
            [Act::OpenTailscale]
        );
        // Signed in, no phone yet: Done on step 3, and the link button on 4.
        let g = Guide {
            installed: true,
            signed_in: true,
            ..guide()
        };
        let st = steps(&g, false, false, None, false);
        assert_eq!(st[2].actions, [Act::Done]);
        assert_eq!(st[3].actions, [Act::LinkOn { busy: false }]);
        // Working: the same button, marked busy.
        assert_eq!(
            steps(&g, false, true, None, false)[3].actions,
            [Act::LinkOn { busy: true }]
        );
        // The error line and the one-time enable button are step 4's too.
        let st = steps(&g, true, false, Some("HTTPS is off"), true);
        assert_eq!(st[3].note.as_deref(), Some("HTTPS is off"));
        assert_eq!(st[3].actions, [Act::LinkOn { busy: false }, Act::Enable]);
        assert!(st[2].actions.is_empty());
        // Link on: Turn off replaces Turn on.
        let g = Guide { link_on: true, ..g };
        assert_eq!(
            steps(&g, true, false, None, false)[3].actions,
            [Act::LinkOff]
        );
        // Invoke not running: nothing to turn on.
        let g = Guide {
            installed: true,
            signed_in: true,
            invoke_running: false,
            ..guide()
        };
        assert!(steps(&g, true, false, None, false)[3].actions.is_empty());
    }

    fn status_with(peers: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "BackendState": "Running",
            "Self": { "UserID": 7, "DNSName": "mac.tail0000.ts.net." },
            "User": { "7": { "LoginName": "alice@example.com" } },
            "Peer": peers
        })
    }

    fn wording(peers: serde_json::Value, done: bool) -> String {
        let s = crate::invoke::parse_status(&status_with(peers));
        phones_text(&s.phone_list, done)
    }

    /// Brian's live test: one phone online was his Android while he scanned with
    /// a work iPhone that was not on the tailnet. Step 3 now names the phones.
    #[test]
    fn step_three_names_the_phones_it_sees() {
        // None on the account (another account's phone and a laptop don't count).
        let none = serde_json::json!({
            "a": { "UserID": 9, "OS": "iOS", "Online": true, "DNSName": "other.tail0000.ts.net." },
            "b": { "UserID": 7, "OS": "macOS", "Online": true, "DNSName": "laptop.tail0000.ts.net." }
        });
        let t = wording(none.clone(), false);
        assert!(t.starts_with("No phone is on your Tailscale yet."), "{t}");
        assert!(!t.contains("other") && !t.contains("laptop"));
        // Once the person says Done, the install nag goes.
        assert_eq!(wording(none, true), "No phone is on your Tailscale yet.");

        // One online.
        let one = serde_json::json!({
            "a": { "UserID": 7, "OS": "android", "Online": true, "DNSName": "nostradamus.tail0000.ts.net." }
        });
        assert_eq!(wording(one, false), "nostradamus (Android) is online.");

        // One online and one offline, online first.
        let two = serde_json::json!({
            "a": { "UserID": 7, "OS": "iOS", "Online": false, "DNSName": "work-iphone.tail0000.ts.net." },
            "b": { "UserID": 7, "OS": "android", "Online": true, "DNSName": "nostradamus.tail0000.ts.net." }
        });
        assert_eq!(
            wording(two, false),
            "nostradamus (Android) is online.\nwork-iphone (iOS), offline: open Tailscale on it."
        );

        // Only an offline phone: told so, and none counts as online.
        let off = serde_json::json!({
            "a": { "UserID": 7, "OS": "iOS", "Online": false, "HostName": "Work iPhone" }
        });
        let t = wording(off, true);
        assert_eq!(
            t,
            "No phone is on your Tailscale yet.\nWork iPhone (iOS), offline: open Tailscale on it."
        );
        assert_eq!(
            SCAN_NOTE,
            "Scan with a phone listed in step 3. Other phones can't open it."
        );
    }

    #[test]
    fn the_qr_matrix_is_square_and_has_dark_modules() {
        let code = qrcode::QrCode::new(b"https://mac.tail0000.ts.net:8443").unwrap();
        assert_eq!(code.to_colors().len(), code.width() * code.width());
        assert!(code
            .to_colors()
            .iter()
            .any(|c| *c == qrcode::types::Color::Dark));
    }

    #[test]
    fn the_install_step_ticks_only_once_a_phone_has_subscribed() {
        let g = guide();
        assert!(!on_phone(&g));
        assert!(install_text(PhoneTab::IPhone, &g).contains("This ticks when"));
        let g = Guide { subscribed: 1, ..g };
        assert!(on_phone(&g));
        assert!(install_text(PhoneTab::Android, &g).contains("notifications are on"));
    }

    #[test]
    fn each_tab_has_its_own_steps_and_iphone_is_first() {
        assert_eq!(PhoneTab::default(), PhoneTab::IPhone);
        let g = guide();
        let i = install_text(PhoneTab::IPhone, &g);
        let a = install_text(PhoneTab::Android, &g);
        assert!(i.contains("Safari") && i.contains("Add to Home Screen") && i.contains("16.4"));
        assert!(a.contains("Chrome") && a.contains("Install app") && !a.contains("Safari"));
        assert!(i.contains("Note: ") && !a.contains("Note: "));
    }
}
