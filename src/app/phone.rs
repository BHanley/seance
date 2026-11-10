//! Settings > "Use Seance on your phone": a short guide, each step checking
//! itself with a green tick. Invoke (the phone chat) runs inside the app; the
//! phone reaches it over the owner's own Tailscale. The checks and the
//! `tailscale serve` commands run on the daemon (`invoke.rs`); this is the card.

use gpui::{div, prelude::*, px, Context, SharedString};
use gpui_component::StyledExt as _;

use crate::invoke::Guide;
use crate::theme::SeancePalette;

use super::SeanceApp;

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
            body: if g.phones_online > 0 {
                format!(
                    "{} of your phones {} online.",
                    g.phones_online,
                    if g.phones_online == 1 { "is" } else { "are" }
                )
            } else {
                "Install Tailscale on your phone and sign in with the same account. Seance \
                 can't see this step unless the phone is online, so press Done when it is."
                    .into()
            },
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

    pub(super) fn render_phone(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
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
        let mut card = div()
            .id("phone-card")
            .w(px(480.))
            .max_h(px(640.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(SeancePalette::flame_dim())
            .bg(SeancePalette::bg_elevated())
            .child(
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
            return Some(self.phone_shell(card, cx));
        };
        if !g.bundled {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("This build of Seance does not include the phone chat."),
            );
            return Some(self.phone_shell(card, cx));
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
                "Scan this with your phone's camera, then use Share > Add to Home Screen \
                 so it opens like an app."
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
                    ),
            );
        }
        card = card.child(
            div()
                .w_full()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .child("Your phone reaches Seance only while this Mac is awake."),
        );
        Some(self.phone_shell(card, cx))
    }

    fn phone_shell(&self, card: gpui::Stateful<gpui::Div>, cx: &Context<Self>) -> gpui::AnyElement {
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
                div().flex().flex_col().gap_2().child(card).child(
                    div().flex().flex_row().justify_end().gap_2().child(
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

    #[test]
    fn the_qr_matrix_is_square_and_has_dark_modules() {
        let code = qrcode::QrCode::new(b"https://mac.tail0000.ts.net:8443").unwrap();
        assert_eq!(code.to_colors().len(), code.width() * code.width());
        assert!(code
            .to_colors()
            .iter()
            .any(|c| *c == qrcode::types::Color::Dark));
    }
}
