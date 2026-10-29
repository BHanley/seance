//! AFK chip, top right on Stage and Wall. Brian's `~/Claude/bin/afk` keeps
//! the Mac awake with its lid closed and has leads text him on his phone
//! (Invoke); this chip shows its mark and flips it, the same chip as
//! Invoke's. Nothing shows where that script isn't installed.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{div, prelude::*, px, AnyElement, Context, PromptLevel, SharedString, Window};

use crate::theme::SeancePalette;

use super::util::tip;
use super::SeanceApp;

/// Width of the chip, so the Stage strip can leave room for it.
pub(super) const AFK_W: f32 = 96.;
/// Render runs far more often than the mark can change.
const RECHECK: Duration = Duration::from_secs(2);
/// An older mark is stale, as in the prompt gate and Invoke.
const STALE_SECS: u64 = 12 * 3600;

struct Seen {
    at: Option<Instant>,
    script: Option<PathBuf>,
    on: bool,
    busy: bool,
}

static SEEN: Mutex<Seen> = Mutex::new(Seen {
    at: None,
    script: None,
    on: false,
    busy: false,
});

fn seen() -> std::sync::MutexGuard<'static, Seen> {
    SEEN.lock().unwrap_or_else(|e| e.into_inner())
}

fn mark_on(home: &PathBuf) -> bool {
    let Ok(text) = std::fs::read_to_string(home.join(".local/state/afk/on")) else {
        return false;
    };
    let Ok(since) = text.trim().parse::<u64>() else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    now.saturating_sub(since) <= STALE_SECS
}

/// (script, on, busy), rereading the mark when it's due.
fn state() -> (Option<PathBuf>, bool, bool) {
    let mut s = seen();
    if s.at.map_or(true, |t| t.elapsed() >= RECHECK) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        s.script = home
            .as_ref()
            .map(|h| h.join("Claude/bin/afk"))
            .filter(|p| p.exists());
        s.on = home.as_ref().is_some_and(mark_on);
        s.at = Some(Instant::now());
    }
    (s.script.clone(), s.on, s.busy)
}

impl SeanceApp {
    pub(super) fn render_afk_chip(&self, key: &str, cx: &Context<Self>) -> Option<AnyElement> {
        let (script, on, busy) = state();
        script.as_ref()?;
        let label = match (busy, on) {
            (true, true) => "Turning off…",
            (true, false) => "Turning on…",
            (false, true) => "✓ AFK ON",
            (false, false) => "AFK OFF",
        };
        Some(
            div()
                .id(SharedString::from(format!("{key}-afk")))
                .flex_none()
                .w(px(AFK_W))
                .flex()
                .justify_center()
                .py(px(4.))
                .rounded_full()
                .border_1()
                .text_xs()
                .when(on, |d| {
                    d.bg(SeancePalette::flame().opacity(0.16))
                        .border_color(gpui::transparent_black())
                        .text_color(SeancePalette::flame())
                        .font_semibold()
                })
                .when(!on, |d| {
                    d.border_color(SeancePalette::border())
                        .text_color(SeancePalette::text_dim())
                })
                .when(!busy, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.text_color(SeancePalette::text()))
                        .tooltip(tip(if on {
                            "AFK: the Mac stays awake with its lid closed and leads text you in Invoke"
                        } else {
                            "go AFK: keep the Mac awake with its lid closed, leads text you in Invoke"
                        }))
                        .on_click(cx.listener(|this, _, window, cx| this.ask_afk(window, cx)))
                })
                .child(label)
                .into_any_element(),
        )
    }

    /// Confirm, then run `afk on` or `afk off` off the UI thread.
    fn ask_afk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(script), on, false) = state() else {
            return;
        };
        let (msg, detail, yes) = if on {
            (
                "Turn AFK off?",
                "The Mac sleeps with its lid closed again.",
                "Turn off",
            )
        } else {
            (
                "Turn AFK on?",
                "The Mac stays awake with its lid closed and leads text you in Invoke.",
                "Turn on",
            )
        };
        let answer = window.prompt(PromptLevel::Info, msg, Some(detail), &[yes, "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            seen().busy = true;
            let _ = this.update(cx, |_, cx| cx.notify());
            cx.background_executor()
                .spawn(async move {
                    let _ = std::process::Command::new("/bin/bash")
                        .arg(script)
                        .arg(if on { "off" } else { "on" })
                        .status();
                })
                .await;
            {
                let mut s = seen();
                s.busy = false;
                s.at = None;
            }
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}
