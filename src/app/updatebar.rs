//! The "Update ready" bar: a newer Seance exists, one click installs it.
//! The work is in `selfupdate.rs`; this is the little bar and its two clicks.

use gpui::{div, prelude::*, px, Context};
use gpui_component::StyledExt as _;

use crate::selfupdate::{self, Feed};
use crate::theme::SeancePalette;

use super::SeanceApp;

#[derive(Default)]
pub(super) enum UpdateUi {
    /// Nothing newer (or updating is off here).
    #[default]
    Off,
    Ready(Feed),
    Working(Feed),
    Failed(Feed, String),
}

/// The strip the bar sits in: pinned to the window's bottom edge, full width,
/// over whatever view is showing.
fn bar_shell() -> gpui::Stateful<gpui::Div> {
    div()
        .id("update-bar")
        .debug_selector(|| "update-bar".into())
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .p_2()
}

impl SeanceApp {
    /// Look for a newer build when the app opens and every hour.
    pub(super) fn start_update_checks(&mut self, cx: &mut Context<Self>) {
        // The window is up: the app this one replaced is no longer needed.
        selfupdate::cleanup_backup();
        cx.spawn(async move |this, cx| loop {
            let found = cx
                .background_executor()
                .spawn(async move { selfupdate::check_now() })
                .await;
            // Off here (logged once): nothing to look for, ever.
            if matches!(found, selfupdate::Outcome::Off(_)) {
                break;
            }
            let Some(this) = this.upgrade() else { break };
            let _ = this.update(cx, |app: &mut SeanceApp, cx| {
                // A feed that can't be read now is not worth a bar: try again next hour.
                if let selfupdate::Outcome::Ready(feed) = found {
                    app.show_update(feed, cx);
                }
            });
            cx.background_executor()
                .timer(selfupdate::CHECK_EVERY)
                .await;
        })
        .detach();
    }

    fn show_update(&mut self, feed: Feed, cx: &mut Context<Self>) {
        if !matches!(self.update, UpdateUi::Working(_)) {
            self.update = UpdateUi::Ready(feed);
            cx.notify();
        }
    }

    /// "Check for Updates…" in the Seance menu: look now, and say what came of
    /// it in a native dialog (so the answer shows however the window is laid
    /// out). A newer build also raises the bar, and the dialog offers to
    /// install it.
    pub(super) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { selfupdate::check_now() })
                .await;
            let say = match &found {
                selfupdate::Outcome::Off(m) | selfupdate::Outcome::UpToDate(m) => m.clone(),
                selfupdate::Outcome::Failed(e) => {
                    format!("Seance could not look for updates: {e}")
                }
                selfupdate::Outcome::Ready(f) => format!(
                    "Seance {} is ready to install.{}",
                    f.version,
                    if f.notes.trim().is_empty() {
                        String::new()
                    } else {
                        format!("\n\n{}", f.notes.trim())
                    }
                ),
            };
            let ready = matches!(found, selfupdate::Outcome::Ready(_));
            if let selfupdate::Outcome::Ready(feed) = found {
                let Some(this) = this.upgrade() else { return };
                let _ = this.update(cx, |app: &mut SeanceApp, cx| app.show_update(feed, cx));
            }
            let pressed = cx
                .background_executor()
                .spawn(async move {
                    if ready {
                        selfupdate::dialog(&say, &["Later", "Update"])
                    } else {
                        selfupdate::dialog(&say, &["OK"])
                    }
                })
                .await;
            if ready && pressed.as_deref() == Some("Update") {
                if let Some(this) = this.upgrade() {
                    let _ = this.update(cx, |app: &mut SeanceApp, cx| app.run_update(cx));
                }
            }
        })
        .detach();
    }

    fn run_update(&mut self, cx: &mut Context<Self>) {
        let feed = match &self.update {
            UpdateUi::Ready(f) | UpdateUi::Failed(f, _) => f.clone(),
            _ => return,
        };
        self.update = UpdateUi::Working(feed.clone());
        cx.notify();
        let for_job = feed.clone();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    let app = selfupdate::app_path()
                        .ok_or_else(|| anyhow::anyhow!("Seance is not running from an app"))?;
                    selfupdate::apply(&for_job, &app)
                })
                .await;
            let _ = this.update(cx, |app: &mut SeanceApp, cx| match done {
                Ok(()) => {
                    // The click is the consent for restarting sessions.
                    selfupdate::grant_consent();
                    cx.restart();
                }
                Err(e) => {
                    app.update = UpdateUi::Failed(feed, format!("{e:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_update_bar(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let (text, button, error) = match &self.update {
            UpdateUi::Off => return None,
            UpdateUi::Ready(f) => (
                format!("Update ready: Seance {}", f.version),
                Some("Update"),
                false,
            ),
            UpdateUi::Working(f) => (format!("Updating to Seance {}...", f.version), None, false),
            UpdateUi::Failed(_, msg) => (
                format!("Update did not work: {msg}"),
                Some("Try again"),
                true,
            ),
        };
        let notes = match &self.update {
            UpdateUi::Ready(f) if !f.notes.trim().is_empty() => Some(f.notes.trim().to_string()),
            _ => None,
        };
        Some(
            bar_shell()
                .child(
                    div()
                        .max_w(px(640.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(SeancePalette::flame_dim())
                        .bg(SeancePalette::bg_elevated())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(if error {
                                            SeancePalette::flame()
                                        } else {
                                            SeancePalette::text()
                                        })
                                        .child(text),
                                )
                                .children(notes.map(|n| {
                                    div()
                                        .text_xs()
                                        .text_color(SeancePalette::text_dim())
                                        .child(n)
                                })),
                        )
                        .children(button.map(|label| {
                            div()
                                .id("update-go")
                                .flex_none()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .bg(SeancePalette::surface())
                                .text_color(SeancePalette::flame())
                                .hover(|s| s.bg(SeancePalette::border()))
                                .on_click(cx.listener(|this, _, _, cx| this.run_update(cx)))
                                .child(label)
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, Window};

    /// The window as the app lays it out: a full-size flex root holding the
    /// views, with the bar as one more child.
    struct Root;
    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("seance-root")
                .size_full()
                .flex()
                .child(div().flex_1().h_full().child("the views"))
                .child(bar_shell().child(div().child("Update ready: Seance 9.9.9")))
        }
    }

    #[gpui::test]
    fn the_bar_lands_inside_the_window_at_its_bottom_edge(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Root);
        cx.run_until_parked();
        let bar = cx
            .debug_bounds("update-bar")
            .expect("the bar was not laid out at all");
        let win = cx.update(|window, _| window.bounds());
        eprintln!("bar {bar:?} window {win:?}");
        assert!(
            bar.size.height > gpui::px(10.),
            "bar has no height: {bar:?}"
        );
        assert!(bar.size.width > gpui::px(100.), "bar has no width: {bar:?}");
        assert!(
            bar.bottom() <= win.size.height + gpui::px(1.),
            "bar is below the window: {bar:?}"
        );
        assert!(
            bar.top() >= gpui::px(0.),
            "bar is above the window: {bar:?}"
        );
    }
}
