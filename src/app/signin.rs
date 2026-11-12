//! Signing in to Claude without a terminal: the card that installs Claude
//! Code with a real progress bar, runs `claude auth login` itself, waits for
//! the browser, takes a pasted code if the login asks for one, and says who
//! signed in. The same card, in the corner, is "Sign in again" when the
//! sign-in is about to expire or already has (`Relogin`).
//!
//! The logic that decides things is in `claude_signin.rs` and the pure
//! functions below (`step_for_progress`, `step_for_poll`); the rest is the
//! timer that drives them and the drawing. All blocking work (the install,
//! `claude auth status`) runs on the background executor.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{div, prelude::*, px, Context, Entity, Focusable as _, SharedString, Window};
use gpui_component::input::{Input, InputState};
use gpui_component::StyledExt as _;

use crate::claude_signin::{self as signin, Login, Notice, Progress, SharedProgress, Status};
use crate::theme::SeancePalette;

use super::views::ViewMode;
use super::SeanceApp;

/// Which card this is: the first-run one over the window, or the corner one
/// that asks to sign in again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Origin {
    /// Only the standalone build's first-run check raises this.
    #[allow(dead_code)]
    FirstRun,
    Relogin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Claude Code isn't here: offer the install.
    Install,
    Installing {
        pct: u8,
        label: String,
    },
    InstallFailed(String),
    /// Installed, not signed in (or signed in again): offer the sign-in.
    SignIn,
    /// The login is running and the browser is open.
    Waiting,
    /// The link was copied: the login may want the code from the page.
    Paste,
    Failed(String),
    Done {
        email: Option<String>,
    },
}

/// One run of the card.
pub(super) struct Flow {
    pub(super) origin: Origin,
    pub(super) step: Step,
    claude: Option<PathBuf>,
    progress: Option<SharedProgress>,
    cancel: Arc<AtomicBool>,
    login: Option<Login>,
    code: Option<Entity<InputState>>,
    spin: usize,
    ticks: u32,
    /// A status read is in flight.
    polling: bool,
}

#[derive(Default)]
pub(super) struct Ui {
    pub(super) flow: Option<Flow>,
    ticking: bool,
    /// The "sign in again" notice from the last check, if one is due.
    notice: Option<Notice>,
    /// "Later" was pressed; the next check shows it again.
    dismissed: bool,
    watching: bool,
}

impl Ui {
    /// Is the first-run card up (so the welcome card waits its turn)?
    #[allow(dead_code)]
    pub(super) fn first_run_up(&self) -> bool {
        self.flow
            .as_ref()
            .is_some_and(|f| f.origin == Origin::FirstRun)
    }
}

/// How often the card looks at its work.
const TICK: Duration = Duration::from_millis(300);
/// Ticks between looks at `claude auth status` while waiting (about 2 s).
const POLL_EVERY: u32 = 7;
/// How often the corner card's check runs.
const RECHECK: Duration = Duration::from_secs(30 * 60);

// ---- the pure parts ----

/// What the install's progress looks like on the card.
pub(super) fn step_for_progress(p: &Progress) -> Step {
    match p {
        Progress::Starting => Step::Installing {
            pct: 0,
            label: "Starting...".into(),
        },
        Progress::Downloading { done, total } => {
            let pct = signin::percent(*done, *total);
            Step::Installing {
                pct,
                label: format!("Downloading... {pct}%"),
            }
        }
        Progress::Verifying => Step::Installing {
            pct: 99,
            label: "Checking the download...".into(),
        },
        Progress::SettingUp => Step::Installing {
            pct: 99,
            label: "Setting up...".into(),
        },
        Progress::Failed(e) => Step::InstallFailed(e.clone()),
        Progress::Done(_) => Step::SignIn,
    }
}

/// What a look at `claude auth status` does to a card that is waiting for
/// the sign-in. Signed in wins over everything (the login exits the moment
/// it succeeds, so "exited" and "signed in" arrive together); only then does
/// a finished login or the 10-minute cap end the wait.
pub(super) fn step_for_poll(
    step: &Step,
    status: Option<&Status>,
    login_exited: bool,
    over_cap: bool,
) -> Option<Step> {
    if !matches!(step, Step::Waiting | Step::Paste) {
        return None;
    }
    if let Some(s) = status.filter(|s| s.logged_in) {
        return Some(Step::Done {
            email: s.email.clone(),
        });
    }
    if over_cap {
        return Some(Step::Failed(
            "Nothing came back after 10 minutes, so Seance stopped waiting.".into(),
        ));
    }
    if login_exited {
        return Some(Step::Failed("The sign-in ended before it finished.".into()));
    }
    None
}

/// The corner card's headline for a notice.
pub(super) fn notice_title(n: Notice) -> String {
    match n {
        Notice::ExpiresIn(1) => "Your Claude sign-in expires in 1 day".into(),
        Notice::ExpiresIn(d) => format!("Your Claude sign-in expires in {d} days"),
        Notice::Expired => "Your Claude sign-in has expired".into(),
    }
}

/// Should the corner card show now?
pub(super) fn show_notice(
    notice: Option<Notice>,
    dismissed: bool,
    in_chat: bool,
    flow_up: bool,
) -> bool {
    notice.is_some() && !dismissed && in_chat && !flow_up
}

const SPIN: [&str; 4] = ["◜", "◝", "◞", "◟"];

enum Tick {
    Stop,
    Continue,
    Fetch(PathBuf),
}

impl SeanceApp {
    // ---- starting and stopping ----

    /// Show the card at `step` (a first-run verdict). Leaves a card that is
    /// already up alone.
    pub(super) fn signin_begin(
        &mut self,
        origin: Origin,
        step: Step,
        claude: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.signin.flow.is_some() {
            return;
        }
        self.signin.flow = Some(Flow {
            origin,
            step,
            claude,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            login: None,
            code: None,
            spin: 0,
            ticks: 0,
            polling: false,
        });
        cx.notify();
    }

    /// Close the card and stop whatever it was running.
    pub(super) fn signin_close(&mut self, cx: &mut Context<Self>) {
        if let Some(f) = self.signin.flow.take() {
            f.cancel.store(true, Ordering::SeqCst);
        }
        cx.notify();
    }

    fn flow_mut(&mut self) -> Option<&mut Flow> {
        self.signin.flow.as_mut()
    }

    // ---- the buttons ----

    fn flow_install(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.flow_mut() else { return };
        let progress: SharedProgress = Arc::new(Mutex::new(Progress::Starting));
        let cancel = Arc::new(AtomicBool::new(false));
        f.progress = Some(progress.clone());
        f.cancel = cancel.clone();
        f.step = step_for_progress(&Progress::Starting);
        cx.background_executor()
            .spawn(async move { signin::install(&progress, &cancel) })
            .detach();
        self.flow_tick_loop(cx);
        cx.notify();
    }

    /// Stop the install and go back to the offer.
    fn flow_cancel_install(&mut self, cx: &mut Context<Self>) {
        if let Some(f) = self.flow_mut() {
            f.cancel.store(true, Ordering::SeqCst);
            f.progress = None;
            f.step = Step::Install;
        }
        cx.notify();
    }

    fn flow_sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.flow_mut() else { return };
        f.login = None;
        f.code = None;
        let claude = f.claude.clone().or_else(signin::find_claude);
        let Some(claude) = claude else {
            f.step = Step::Install;
            cx.notify();
            return;
        };
        match Login::start(&claude) {
            Ok(login) => {
                f.claude = Some(claude);
                f.login = Some(login);
                f.step = Step::Waiting;
                f.ticks = 0;
                f.polling = false;
                self.flow_tick_loop(cx);
            }
            Err(e) => f.step = Step::Failed(e),
        }
        cx.notify();
    }

    /// Stop the login and go back to where the card started.
    fn flow_cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.flow_mut() else { return };
        f.login = None;
        f.code = None;
        match f.origin {
            Origin::Relogin => self.signin_close(cx),
            Origin::FirstRun => {
                f.step = Step::SignIn;
                cx.notify();
            }
        }
    }

    /// "Didn't open? Copy link": the link goes on the clipboard and the card
    /// asks for the code the page will show.
    fn flow_copy_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self
            .signin
            .flow
            .as_ref()
            .and_then(|f| f.login.as_ref())
            .and_then(|l| l.url());
        if let Some(url) = url {
            let _ = crate::clipboard::copy_text_to_clipboard(&url, cx);
        }
        let Some(f) = self.flow_mut() else { return };
        if f.code.is_none() {
            f.code = Some(cx.new(|cx| InputState::new(window, cx).placeholder("Paste the code")));
        }
        f.step = Step::Paste;
        if let Some(input) = &f.code {
            let focus = input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn flow_send_code(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.flow_mut() else { return };
        let code = f
            .code
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        if f.login.as_mut().is_some_and(|l| l.send_code(&code)) {
            f.step = Step::Waiting;
        }
        cx.notify();
    }

    fn flow_back_to_waiting(&mut self, cx: &mut Context<Self>) {
        if let Some(f) = self.flow_mut() {
            f.step = Step::Waiting;
        }
        cx.notify();
    }

    /// E's button: first run lands in the circle and lets the held Claude
    /// starts go; the corner card just goes away.
    fn flow_continue(&mut self, cx: &mut Context<Self>) {
        let origin = self.signin.flow.as_ref().map(|f| f.origin);
        self.signin_close(cx);
        match origin {
            Some(Origin::FirstRun) => {
                crate::claude_check::hold_new_claude(false);
                self.land_after_login(cx);
            }
            _ => self.signin.notice = None,
        }
    }

    /// Signed in: put the user in the circle where they were, on its lead.
    pub(super) fn land_after_login(&mut self, cx: &mut Context<Self>) {
        if let Some(ws) = self.selected_workspace.clone() {
            let _ = self.client.set_focus(None, Some(ws));
        }
        cx.notify();
    }

    // ---- the timer ----

    fn flow_tick_loop(&mut self, cx: &mut Context<Self>) {
        if self.signin.ticking {
            return;
        }
        self.signin.ticking = true;
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| loop {
            cx.background_executor().timer(TICK).await;
            let Ok(tick) = this.update(cx, |this, cx| this.flow_tick(cx)) else {
                return;
            };
            match tick {
                Tick::Stop => {
                    let _ = this.update(cx, |this, _| this.signin.ticking = false);
                    return;
                }
                Tick::Continue => {}
                Tick::Fetch(claude) => {
                    let status = cx
                        .background_executor()
                        .spawn(async move { signin::fetch_status(&claude) })
                        .await;
                    if this
                        .update(cx, |this, cx| this.flow_status(status, cx))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })
        .detach();
    }

    fn flow_tick(&mut self, cx: &mut Context<Self>) -> Tick {
        let Some(f) = self.signin.flow.as_mut() else {
            return Tick::Stop;
        };
        f.spin = f.spin.wrapping_add(1);
        let tick = match &f.step {
            Step::Installing { .. } => {
                let p = f
                    .progress
                    .as_ref()
                    .and_then(|p| p.lock().ok().map(|g| g.clone()));
                if let Some(p) = p {
                    if let Progress::Done(path) = &p {
                        f.claude = Some(path.clone());
                    }
                    f.step = step_for_progress(&p);
                }
                Tick::Continue
            }
            Step::Waiting | Step::Paste => {
                f.ticks += 1;
                let exited = f.login.as_mut().is_none_or(|l| l.exited());
                let capped = f.login.as_ref().is_some_and(|l| l.over_cap());
                let due = f.ticks % POLL_EVERY == 0 || exited || capped;
                match (&f.claude, due && !f.polling) {
                    (Some(c), true) => {
                        f.polling = true;
                        Tick::Fetch(c.clone())
                    }
                    _ => Tick::Continue,
                }
            }
            _ => Tick::Stop,
        };
        cx.notify();
        tick
    }

    /// A look at `claude auth status` came back.
    fn flow_status(&mut self, status: Option<Status>, cx: &mut Context<Self>) {
        let Some(f) = self.signin.flow.as_mut() else {
            return;
        };
        f.polling = false;
        let exited = f.login.as_mut().is_none_or(|l| l.exited());
        let capped = f.login.as_ref().is_some_and(|l| l.over_cap());
        if let Some(next) = step_for_poll(&f.step, status.as_ref(), exited, capped) {
            if matches!(next, Step::Done { .. }) && f.origin == Origin::FirstRun {
                crate::claude_check::mark_passed();
            }
            f.login = None;
            f.step = next;
        }
        cx.notify();
    }

    // ---- the corner card's check ----

    /// Look at the sign-in on launch and every 30 minutes, off the UI thread.
    /// macOS, local only: with a hub the panes' Claude isn't this one.
    pub(super) fn start_relogin_watch(&mut self, cx: &mut Context<Self>) {
        let local = cfg!(target_os = "macos") && crate::ACTIVE_TUNNEL.get().is_none();
        if !local || self.signin.watching {
            return;
        }
        self.signin.watching = true;
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            // Let the window settle first.
            cx.background_executor().timer(Duration::from_secs(8)).await;
            loop {
                let notice = cx
                    .background_executor()
                    .spawn(async {
                        let status = signin::find_claude().and_then(|c| signin::fetch_status(&c));
                        status.and_then(|s| signin::expiry_notice(&s, signin::now_secs()))
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.signin.notice = notice;
                        this.signin.dismissed = false;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(RECHECK).await;
            }
        })
        .detach();
    }

    // ---- drawing ----

    pub(super) fn render_signin(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let in_chat = self.view.mode == ViewMode::Chat;
        let Some(f) = &self.signin.flow else {
            if !show_notice(self.signin.notice, self.signin.dismissed, in_chat, false) {
                return None;
            }
            let notice = self.signin.notice?;
            return Some(self.relogin_offer(notice, cx));
        };
        let corner = f.origin == Origin::Relogin;
        let first = f.origin == Origin::FirstRun;
        let spinner = SPIN[f.spin % SPIN.len()];

        let button = |id: &'static str, label: &'static str, primary: bool| {
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
        let link = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .text_xs()
                .underline()
                .text_color(SeancePalette::text_dim())
                .cursor_pointer()
                .hover(|s| s.text_color(SeancePalette::text()))
                .child(label)
        };
        let row = |children: Vec<gpui::AnyElement>| {
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .children(children)
                .into_any_element()
        };
        let later = |this: &mut Self, cx: &mut Context<Self>| this.signin_dismiss(cx);
        let links =
            || {
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_center()
                    .child(link("signin-copy", "Didn't open? Copy link").on_click(
                        cx.listener(|this, _, window, cx| this.flow_copy_link(window, cx)),
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(SeancePalette::text_faint())
                            .child("·"),
                    )
                    .child(
                        link("signin-again", "Try again")
                            .on_click(cx.listener(|this, _, _, cx| this.flow_sign_in(cx))),
                    )
                    .into_any_element()
            };

        let (title, body, extra, actions): (
            SharedString,
            SharedString,
            Vec<gpui::AnyElement>,
            Vec<gpui::AnyElement>,
        ) = match &f.step {
            Step::Install => (
                "Claude Code isn't on this Mac yet".into(),
                "Seance runs Claude Code. It takes about a minute to install, and \
                 nothing else on your Mac changes."
                    .into(),
                vec![],
                vec![
                    button("signin-later", "Not now", false)
                        .on_click(cx.listener(move |this, _, _, cx| later(this, cx)))
                        .into_any_element(),
                    button("signin-install", "Install Claude Code", true)
                        .on_click(cx.listener(|this, _, _, cx| this.flow_install(cx)))
                        .into_any_element(),
                ],
            ),
            Step::Installing { pct, label } => (
                "Installing Claude Code".into(),
                "Almost there. You can keep this window open.".into(),
                vec![
                    div()
                        .h(px(6.))
                        .w_full()
                        .rounded_full()
                        .bg(SeancePalette::border())
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(SeancePalette::flame())
                                .w(gpui::relative(f32::from(*pct) / 100.)),
                        )
                        .into_any_element(),
                    div()
                        .text_xs()
                        .text_color(SeancePalette::text_faint())
                        .child(label.clone())
                        .into_any_element(),
                ],
                vec![button("signin-cancel", "Cancel", false)
                    .on_click(cx.listener(|this, _, _, cx| this.flow_cancel_install(cx)))
                    .into_any_element()],
            ),
            Step::InstallFailed(msg) => (
                "Couldn't install Claude Code".into(),
                msg.clone().into(),
                vec![],
                vec![
                    button("signin-later", "Not now", false)
                        .on_click(cx.listener(move |this, _, _, cx| later(this, cx)))
                        .into_any_element(),
                    button("signin-retry", "Try again", true)
                        .on_click(cx.listener(|this, _, _, cx| this.flow_install(cx)))
                        .into_any_element(),
                ],
            ),
            Step::SignIn => (
                "Sign in with Claude".into(),
                "Seance runs on your Claude plan, so you sign in once with your Claude \
                 account."
                    .into(),
                vec![],
                vec![
                    button("signin-later", "Not now", false)
                        .on_click(cx.listener(move |this, _, _, cx| later(this, cx)))
                        .into_any_element(),
                    button("signin-go", "Sign in with Claude", true)
                        .on_click(cx.listener(|this, _, _, cx| this.flow_sign_in(cx)))
                        .into_any_element(),
                ],
            ),
            Step::Waiting => (
                "Your browser opened".into(),
                "Approve there, then come back.".into(),
                vec![
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .text_color(SeancePalette::text_dim())
                        .child(div().text_color(SeancePalette::flame()).child(spinner))
                        .child("Waiting for you to approve...")
                        .into_any_element(),
                    links(),
                ],
                vec![button("signin-cancel", "Cancel", false)
                    .on_click(cx.listener(|this, _, _, cx| this.flow_cancel_sign_in(cx)))
                    .into_any_element()],
            ),
            Step::Paste => (
                "Paste your code".into(),
                "The page in your browser shows a code. Paste it here.".into(),
                vec![
                    match &f.code {
                        Some(input) => div().w_full().child(Input::new(input)).into_any_element(),
                        None => div().into_any_element(),
                    },
                    links(),
                ],
                vec![
                    button("signin-back", "Back", false)
                        .on_click(cx.listener(|this, _, _, cx| this.flow_back_to_waiting(cx)))
                        .into_any_element(),
                    button("signin-code", "Continue", true)
                        .on_click(cx.listener(|this, _, _, cx| this.flow_send_code(cx)))
                        .into_any_element(),
                ],
            ),
            Step::Failed(msg) => (
                "Sign-in didn't finish".into(),
                msg.clone().into(),
                vec![],
                vec![
                    button("signin-cancel", "Cancel", false)
                        .on_click(cx.listener(|this, _, _, cx| this.flow_cancel_sign_in(cx)))
                        .into_any_element(),
                    button("signin-retry", "Try again", true)
                        .on_click(cx.listener(|this, _, _, cx| this.flow_sign_in(cx)))
                        .into_any_element(),
                ],
            ),
            Step::Done { email } => (
                "You're signed in".into(),
                SharedString::default(),
                vec![div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child(div().text_color(SeancePalette::success()).child("✓"))
                    .child(match email {
                        Some(e) => format!("Signed in as {e}"),
                        None => "Signed in".to_string(),
                    })
                    .into_any_element()],
                vec![button("signin-continue", "Continue", true)
                    .on_click(cx.listener(|this, _, _, cx| this.flow_continue(cx)))
                    .into_any_element()],
            ),
        };

        let card = div()
            .id("signin-card")
            .w(px(if corner { 340. } else { 460. }))
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
            .when(!body.is_empty(), |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(SeancePalette::text_dim())
                        .child(body),
                )
            })
            .children(extra)
            .child(row(actions));

        let wrap = div().id("signin-overlay").absolute();
        Some(
            if first {
                // Over the window: the dimmed Chat shows behind it, and clicks
                // stay off what is underneath.
                wrap.inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::black().opacity(0.5))
                    .occlude()
            } else {
                wrap.bottom_4().right_4()
            }
            .child(card)
            .into_any_element(),
        )
    }

    fn signin_dismiss(&mut self, cx: &mut Context<Self>) {
        // "Not now" on first run lets the held Claude starts go.
        if self.signin.first_run_up() {
            crate::claude_check::hold_new_claude(false);
        }
        self.signin_close(cx);
    }

    /// The corner card before the button is pressed (sheet G).
    fn relogin_offer(&self, notice: Notice, cx: &Context<Self>) -> gpui::AnyElement {
        let button = |id: &'static str, label: &'static str, primary: bool| {
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
        div()
            .id("relogin-card")
            .absolute()
            .bottom_4()
            .right_4()
            .w(px(340.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(SeancePalette::flame_dim())
            .bg(SeancePalette::bg_elevated())
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(SeancePalette::text())
                    .child(notice_title(notice)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(SeancePalette::text_dim())
                    .child("Sign in again so your circles keep working."),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("relogin-later", "Later", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.signin.dismissed = true;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button("relogin-go", "Sign in again", true).on_click(cx.listener(
                            |this, _, _, cx| {
                                let claude = signin::find_claude();
                                this.signin_begin(Origin::Relogin, Step::SignIn, claude, cx);
                                this.flow_sign_in(cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(logged_in: bool, email: Option<&str>) -> Status {
        Status {
            logged_in,
            email: email.map(str::to_string),
            expires_at: None,
        }
    }

    #[test]
    fn install_progress_maps_to_card_states() {
        assert_eq!(
            step_for_progress(&Progress::Downloading {
                done: 58,
                total: 100
            }),
            Step::Installing {
                pct: 58,
                label: "Downloading... 58%".into()
            }
        );
        assert!(matches!(
            step_for_progress(&Progress::Verifying),
            Step::Installing { pct: 99, .. }
        ));
        // A finished install goes straight to the sign-in offer.
        assert_eq!(
            step_for_progress(&Progress::Done("/x/claude".into())),
            Step::SignIn
        );
        assert_eq!(
            step_for_progress(&Progress::Failed("no network".into())),
            Step::InstallFailed("no network".into())
        );
    }

    #[test]
    fn waiting_ends_on_sign_in_first_then_exit_or_the_cap() {
        let done = |e: &str| Step::Done {
            email: Some(e.into()),
        };
        let signed = status(true, Some("a@b.c"));
        let out = status(false, None);
        for step in [Step::Waiting, Step::Paste] {
            // Signed in wins, even when the login has exited or timed out.
            assert_eq!(
                step_for_poll(&step, Some(&signed), true, true),
                Some(done("a@b.c"))
            );
            assert_eq!(step_for_poll(&step, Some(&out), false, false), None);
            assert_eq!(step_for_poll(&step, None, false, false), None);
            // The login ended without signing in: say so, with Try again.
            assert!(matches!(
                step_for_poll(&step, Some(&out), true, false),
                Some(Step::Failed(_))
            ));
            assert!(matches!(
                step_for_poll(&step, Some(&out), false, true),
                Some(Step::Failed(_))
            ));
        }
        // Other steps ignore a poll.
        assert_eq!(
            step_for_poll(&Step::SignIn, Some(&signed), false, false),
            None
        );
        assert_eq!(
            step_for_poll(&Step::Install, Some(&signed), false, false),
            None
        );
    }

    #[test]
    fn the_corner_card_shows_only_in_chat_when_due_and_not_dismissed() {
        let n = Some(Notice::ExpiresIn(2));
        assert!(show_notice(n, false, true, false));
        assert!(!show_notice(None, false, true, false));
        assert!(!show_notice(n, true, true, false), "Later");
        assert!(!show_notice(n, false, false, false), "not in Chat");
        assert!(!show_notice(n, false, true, true), "a card is already up");
        assert_eq!(
            notice_title(Notice::ExpiresIn(2)),
            "Your Claude sign-in expires in 2 days"
        );
        assert_eq!(
            notice_title(Notice::ExpiresIn(1)),
            "Your Claude sign-in expires in 1 day"
        );
        assert_eq!(
            notice_title(Notice::Expired),
            "Your Claude sign-in has expired"
        );
    }
}
