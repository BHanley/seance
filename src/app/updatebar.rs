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

impl SeanceApp {
    /// Look for a newer build when the app opens and every hour.
    pub(super) fn start_update_checks(&mut self, cx: &mut Context<Self>) {
        // The window is up: the app this one replaced is no longer needed.
        selfupdate::cleanup_backup();
        if !selfupdate::enabled_here() {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            let url = selfupdate::feed_url();
            let found = cx
                .background_executor()
                .spawn(async move { selfupdate::check(env!("CARGO_PKG_VERSION"), &url) })
                .await;
            let Some(this) = this.upgrade() else { break };
            let _ = this.update(cx, |app: &mut SeanceApp, cx| {
                // A feed that can't be read now is not worth a bar: try again next hour.
                if let Ok(Some(feed)) = found {
                    if !matches!(app.update, UpdateUi::Working(_)) {
                        app.update = UpdateUi::Ready(feed);
                        cx.notify();
                    }
                }
            });
            cx.background_executor()
                .timer(selfupdate::CHECK_EVERY)
                .await;
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
            div()
                .id("update-bar")
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .p_2()
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
