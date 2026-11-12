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
        /// The install or login terminal this card opened (its slug). Once set,
        /// the card waits for it instead of opening another.
        opened: Option<String>,
    },
}

/// How often the card looks again once its terminal is open.
const RECHECK: std::time::Duration = std::time::Duration::from_secs(3);

/// What the card's button does: bring an existing terminal forward, or open one.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Open {
    Focus(String),
    Spawn,
}

/// The pane name this verdict's button opens.
fn pane_name(verdict: &Verdict) -> Option<&'static str> {
    match verdict {
        Verdict::Missing => Some("install-claude"),
        Verdict::LoggedOut(_) => Some("claude-login"),
        Verdict::Ok => None,
    }
}

/// One install or login terminal at most: if one is already open (named for
/// this step), the button goes to it. `panes` are (slug, name).
pub(super) fn plan_open(panes: &[(String, String)], name: &str) -> Open {
    panes
        .iter()
        .find(|(_, n)| n == name)
        .map_or(Open::Spawn, |(slug, _)| Open::Focus(slug.clone()))
}

/// What a re-check does to the card.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Next {
    /// Logged in: the card goes away and the check is marked passed.
    Pass,
    /// Still not: keep the card, with the terminal we opened only while the
    /// step is the same one (installing then logging in is a new step).
    Stay(Verdict, Option<String>),
}

pub(super) fn after_check(old: &Verdict, new: Verdict, opened: Option<String>) -> Next {
    if new == Verdict::Ok {
        return Next::Pass;
    }
    let same_step = pane_name(old) == pane_name(&new);
    Next::Stay(new, opened.filter(|_| same_step))
}

impl SeanceApp {
    /// Run the check off the UI thread. Skipped on Linux, in remote mode (the
    /// panes run on the hub, not here) and once the check has passed.
    pub(super) fn start_first_run_check(&mut self, cx: &mut Context<Self>) {
        let local = cfg!(target_os = "macos") && crate::ACTIVE_TUNNEL.get().is_none();
        if !local || claude_check::already_passed() {
            return;
        }
        // Until it passes, nothing starts a Claude pane on its own.
        claude_check::hold_new_claude(true);
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
            let _ = this.update(cx, |this, cx| this.apply_check(verdict, cx));
        })
        .detach();
    }

    /// A check's answer: pass, or keep (or raise) the card.
    fn apply_check(&mut self, verdict: Verdict, cx: &mut Context<Self>) {
        let (old, opened) = match &self.first_run {
            FirstRun::Card {
                verdict, opened, ..
            } => (verdict.clone(), opened.clone()),
            FirstRun::Off => (Verdict::Ok, None),
        };
        // A first answer of Ok has no card to take down.
        if verdict == Verdict::Ok {
            let was_card = matches!(self.first_run, FirstRun::Card { .. });
            claude_check::mark_passed();
            claude_check::hold_new_claude(false);
            self.first_run = FirstRun::Off;
            if was_card {
                self.land_after_login(opened, cx);
            }
            cx.notify();
            return;
        }
        let (verdict, opened) = match after_check(&old, verdict, opened) {
            Next::Pass => unreachable!("Ok handled above"),
            Next::Stay(v, o) => (v, o),
        };
        let watching = opened.is_some();
        self.first_run = FirstRun::Card {
            verdict,
            checking: false,
            opened,
        };
        if watching {
            self.watch_login(cx);
        }
        cx.notify();
    }

    /// Logged in: put the user in the circle where the terminal was, on its
    /// lead, instead of leaving them wherever the card found them.
    fn land_after_login(&mut self, opened: Option<String>, cx: &mut Context<Self>) {
        let ws = opened
            .and_then(|slug| self.panes.iter().find(|p| p.slug == slug))
            .map(|p| p.workspace.clone())
            .or_else(|| self.selected_workspace.clone());
        if let Some(ws) = ws {
            self.selected_workspace = Some(ws.clone());
            let _ = self.client.set_focus(None, Some(ws));
        }
        cx.notify();
    }

    /// While the card waits on its terminal, look again every few seconds, off
    /// the UI thread, until the login lands (or the card is closed). One loop:
    /// each answer goes through `apply_check`, which starts the next wait.
    fn watch_login(&mut self, cx: &mut Context<Self>) {
        if self.first_run_watching {
            return;
        }
        self.first_run_watching = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            cx.background_executor().timer(RECHECK).await;
            let waiting = this
                .update(cx, |this, _| {
                    let waiting = matches!(
                        this.first_run,
                        FirstRun::Card {
                            opened: Some(_),
                            ..
                        }
                    );
                    if !waiting {
                        this.first_run_watching = false;
                    }
                    waiting
                })
                .unwrap_or(false);
            if !waiting {
                return;
            }
            let verdict = cx
                .background_executor()
                .spawn(async { claude_check::check() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.first_run_watching = false;
                this.apply_check(verdict, cx);
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
        let panes: Vec<(String, String)> = self
            .panes
            .iter()
            .map(|p| (p.slug.clone(), p.name.clone()))
            .collect();
        let slug = match plan_open(&panes, name) {
            Open::Focus(slug) => Some(slug),
            Open::Spawn => self.spawn_internal(
                SpawnRequest {
                    name: name.into(),
                    cwd: None,
                    command: Some(command),
                    workspace: self.selected_workspace.clone(),
                    file: None,
                },
                cx,
            ),
        };
        if let Some(slug) = &slug {
            self.focus_first_run_pane(slug);
        }
        if let FirstRun::Card { opened, .. } = &mut self.first_run {
            *opened = slug;
        }
        self.watch_login(cx);
        cx.notify();
    }

    /// Bring the terminal forward.
    fn focus_first_run_pane(&mut self, slug: &str) {
        let Some(ws) = self
            .panes
            .iter()
            .find(|p| p.slug == slug)
            .map(|p| p.workspace.clone())
        else {
            return;
        };
        self.selected_workspace = Some(ws.clone());
        self.active_slug = Some(slug.to_string());
        let _ = self.client.set_focus(Some(slug.to_string()), Some(ws));
    }

    pub(super) fn render_first_run(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let FirstRun::Card {
            verdict,
            checking,
            opened,
        } = &self.first_run
        else {
            return None;
        };
        let waiting = opened.is_some();
        let (title, body, action) = match (verdict, waiting) {
            (Verdict::Missing, false) => (
                "Welcome to Seance",
                "Seance runs Claude Code, which needs a Claude plan. It isn't \
                 installed on this Mac yet.",
                "Install Claude Code",
            ),
            (Verdict::Missing, true) => (
                "Waiting for the install",
                "Finish in the terminal that just opened. This checks every few \
                 seconds and moves on by itself.",
                "Show terminal",
            ),
            (_, false) => (
                "One more step",
                "Claude Code is installed but not logged in. Open it and sign \
                 in with your Claude plan.",
                "Log in to Claude Code",
            ),
            (_, true) => (
                "Waiting for you to sign in",
                "Sign in in the terminal that just opened. This checks every few \
                 seconds and goes away by itself.",
                "Show terminal",
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
        // Before the terminal is open the card covers the window; once it is,
        // it steps to the corner so the terminal can be seen and typed in.
        let wrap = div().id("first-run-overlay").absolute().when_else(
            waiting,
            |d| d.bottom_4().right_4(),
            |d| {
                d.inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::black().opacity(0.5))
                    // Keep clicks off the panes underneath.
                    .occlude()
            },
        );
        Some(
            wrap.child(
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
                                    // Dismissed on purpose: stop holding.
                                    claude_check::hold_new_claude(false);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn panes(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(s, n)| (s.to_string(), n.to_string()))
            .collect()
    }

    fn out(path: &str) -> Verdict {
        Verdict::LoggedOut(PathBuf::from(path))
    }

    #[test]
    fn the_button_reuses_the_terminal_it_already_opened() {
        // None yet: open one. One there: go to it, never a second.
        assert_eq!(
            plan_open(&panes(&[("term", "term")]), "claude-login"),
            Open::Spawn
        );
        let open = panes(&[("term", "term"), ("claude-login-2", "claude-login")]);
        assert_eq!(
            plan_open(&open, "claude-login"),
            Open::Focus("claude-login-2".into())
        );
        // The install terminal is another step's, not this one's.
        assert_eq!(plan_open(&open, "install-claude"), Open::Spawn);
    }

    #[test]
    fn a_recheck_passes_on_login_and_keeps_waiting_otherwise() {
        let opened = Some("claude-login".to_string());
        // Logged in: the card goes away.
        assert_eq!(
            after_check(&out("/c"), Verdict::Ok, opened.clone()),
            Next::Pass
        );
        // Still logged out: keep waiting on the same terminal.
        assert_eq!(
            after_check(&out("/c"), out("/c"), opened.clone()),
            Next::Stay(out("/c"), opened.clone())
        );
        // Installed while the install terminal was open: now it is the login
        // step, a new terminal, so nothing is "opened" yet.
        assert_eq!(
            after_check(&Verdict::Missing, out("/c"), Some("install-claude".into())),
            Next::Stay(out("/c"), None)
        );
        // Not found yet: still the install step.
        assert_eq!(
            after_check(
                &Verdict::Missing,
                Verdict::Missing,
                Some("install-claude".into())
            ),
            Next::Stay(Verdict::Missing, Some("install-claude".into()))
        );
    }
}
