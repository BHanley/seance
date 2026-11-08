//! First-run welcome card: shown on macOS in Local mode when there is no
//! usable Claude Code. The check itself lives in `claude_check.rs`; this is
//! the small card and the two buttons that open an install or login pane.

use gpui::{div, prelude::*, px, Context, SharedString, WeakEntity};
use gpui_component::StyledExt as _;

use crate::claude_check::{self, Verdict};
use crate::pane::SpawnRequest;
use crate::theme::SeancePalette;

use super::SeanceApp;

pub(super) enum FirstRun {
    /// Nothing to show: not Local-on-Mac, already passed, or still checking.
    Off,
    Card {
        verdict: Verdict,
        /// A recheck is running.
        checking: bool,
    },
}

impl SeanceApp {
    /// Run the check off the UI thread. Skipped on Linux, in remote mode (the
    /// panes run on the hub, not here) and once the check has passed.
    pub(super) fn start_first_run_check(&mut self, cx: &mut Context<Self>) {
        let local = cfg!(target_os = "macos") && crate::ACTIVE_TUNNEL.get().is_none();
        if !local || claude_check::already_passed() {
            return;
        }
        self.run_first_run_check(cx);
    }

    fn run_first_run_check(&mut self, cx: &mut Context<Self>) {
        if let FirstRun::Card { checking, .. } = &mut self.first_run {
            *checking = true;
            cx.notify();
        }
        let task = cx
            .background_executor()
            .spawn(async { claude_check::check() });
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let verdict = task.await;
            let _ = this.update(cx, |this, cx| {
                if verdict == Verdict::Ok {
                    claude_check::mark_passed();
                    this.first_run = FirstRun::Off;
                } else {
                    this.first_run = FirstRun::Card {
                        verdict,
                        checking: false,
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn first_run_open_pane(&mut self, cx: &mut Context<Self>) {
        let FirstRun::Card { verdict, .. } = &self.first_run else {
            return;
        };
        let (name, command) = match verdict {
            Verdict::Missing => ("install-claude", claude_check::install_pane_command()),
            Verdict::LoggedOut(path) => ("claude-login", claude_check::login_command(path)),
            Verdict::Ok => return,
        };
        self.spawn_internal(
            SpawnRequest {
                name: name.into(),
                cwd: None,
                command: Some(command),
                workspace: self.selected_workspace.clone(),
                file: None,
            },
            cx,
        );
    }

    pub(super) fn render_first_run(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let FirstRun::Card { verdict, checking } = &self.first_run else {
            return None;
        };
        let (title, body, action) = match verdict {
            Verdict::Missing => (
                "Welcome to Seance",
                "Seance runs Claude Code, which needs a Claude plan. It isn't \
                 installed on this Mac yet.",
                "Install Claude Code",
            ),
            _ => (
                "One more step",
                "Claude Code is installed but not logged in. Open it and sign \
                 in with your Claude plan.",
                "Log in to Claude Code",
            ),
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
        Some(
            div()
                .id("first-run-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.5))
                // Keep clicks off the panes underneath.
                .occlude()
                .child(
                    div()
                        .id("first-run-card")
                        .w(px(420.))
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
                                .child(title),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(SeancePalette::text_dim())
                                .child(body),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap_2()
                                .child(button("first-run-later", "Not now".into(), false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.first_run = FirstRun::Off;
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    button(
                                        "first-run-recheck",
                                        if *checking {
                                            "Checking..."
                                        } else {
                                            "Check again"
                                        }
                                        .into(),
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.run_first_run_check(cx);
                                        },
                                    )),
                                )
                                .child(button("first-run-action", action.into(), true).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.first_run_open_pane(cx);
                                    }),
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}
