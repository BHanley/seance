//! First-run check: on macOS in Local mode with no usable Claude Code, the
//! sign-in card (`signin.rs`) is raised over the window. The check itself
//! lives in `claude_check.rs`; this is only the launch hook and where the
//! user lands once the card is done.

use gpui::{Context, WeakEntity};

use crate::claude_check::{self, Verdict};

use super::signin::{Origin, Step};
use super::SeanceApp;

/// Which card step a first-run verdict opens on; None when there is nothing
/// to show (signed in, or not worth nagging about).
pub(super) fn first_step(verdict: &Verdict) -> Option<Step> {
    match verdict {
        Verdict::Ok => None,
        Verdict::Missing => Some(Step::Install),
        Verdict::LoggedOut(_) => Some(Step::SignIn),
    }
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
        let task = cx
            .background_executor()
            .spawn(async { claude_check::check() });
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let verdict = task.await;
            let _ = this.update(cx, |this, cx| this.apply_check(verdict, cx));
        })
        .detach();
    }

    /// The check's answer: pass, or raise the card.
    fn apply_check(&mut self, verdict: Verdict, cx: &mut Context<Self>) {
        match first_step(&verdict) {
            None => {
                claude_check::mark_passed();
                claude_check::hold_new_claude(false);
            }
            Some(step) => {
                let claude = match verdict {
                    Verdict::LoggedOut(p) => Some(p),
                    _ => None,
                };
                self.signin_begin(Origin::FirstRun, step, claude, cx);
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_verdict_picks_the_first_card() {
        assert_eq!(first_step(&Verdict::Missing), Some(Step::Install));
        assert_eq!(
            first_step(&Verdict::LoggedOut(PathBuf::from("/c"))),
            Some(Step::SignIn)
        );
        assert_eq!(first_step(&Verdict::Ok), None);
    }
}
