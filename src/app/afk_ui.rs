//! AFK in the app: the Seance menu item and Chat's button. Both flip the one
//! switch the daemon keeps (`FsOp::Afk`) and show what it says; the daemon
//! does the work (see `crate::afk`). Standalone Mac app only.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::Context;

use super::SeanceApp;

/// What the menu's tick shows (the menu is built outside any window).
pub(crate) static AFK_ON: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AfkUi {
    /// This build offers AFK at all (standalone Mac app).
    pub enabled: bool,
    pub on: bool,
    pub mac_awake: bool,
    pub note: Option<String>,
    pub busy: bool,
}

/// The daemon's answer, as the UI keeps it.
pub(super) fn from_json(v: &serde_json::Value, enabled: bool) -> AfkUi {
    AfkUi {
        enabled,
        on: v["on"].as_bool().unwrap_or(false),
        mac_awake: v["mac_awake"].as_bool().unwrap_or(false),
        note: v["note"].as_str().map(str::to_string),
        busy: false,
    }
}

/// Chat's button label: what it is, and the one thing worth saying while on:
/// that the Mac may sleep with the lid closed.
pub(super) fn chip_label(a: &AfkUi) -> Option<String> {
    if !a.enabled {
        return None;
    }
    Some(match (a.on, a.mac_awake) {
        (false, _) => "AFK".into(),
        (true, true) => "AFK on".into(),
        (true, false) => "AFK on, Mac may sleep".into(),
    })
}

pub(super) fn chip_tip(a: &AfkUi) -> String {
    if let Some(n) = &a.note {
        return n.clone();
    }
    if a.on {
        "AFK is on: leads text you on your phone. Touch the keyboard or trackpad to end it.".into()
    } else {
        "AFK: step away. The Mac stays awake with its lid closed and leads text you on your phone."
            .into()
    }
}

impl SeanceApp {
    /// Keep the button and the menu tick current (Invoke's toggle and the
    /// daemon's auto-off change it from outside).
    pub(super) fn start_afk(&mut self, cx: &mut Context<Self>) {
        if !crate::claude_check::starts_claude_here() {
            return;
        }
        self.afk.enabled = true;
        cx.spawn(async move |this, cx| loop {
            let client = match this.update(cx, |app: &mut SeanceApp, _| {
                std::sync::Arc::clone(&app.client)
            }) {
                Ok(c) => c,
                Err(_) => break,
            };
            let got = cx
                .background_executor()
                .spawn(async move { client.afk(None) })
                .await;
            let alive = this
                .update(cx, |app: &mut SeanceApp, cx| {
                    if let Ok(v) = got {
                        app.apply_afk(from_json(&v, true), cx);
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
        })
        .detach();
    }

    fn apply_afk(&mut self, next: AfkUi, cx: &mut Context<Self>) {
        let was = self.afk.on;
        let busy = self.afk.busy;
        self.afk = AfkUi { busy, ..next };
        AFK_ON.store(self.afk.on, Ordering::Relaxed);
        if was != self.afk.on {
            crate::set_app_menu(cx);
        }
        cx.notify();
    }

    /// The menu item and Chat's button.
    pub(super) fn afk_toggle(&mut self, cx: &mut Context<Self>) {
        if !self.afk.enabled || self.afk.busy {
            return;
        }
        self.afk.busy = true;
        let want = !self.afk.on;
        let client = std::sync::Arc::clone(&self.client);
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_executor()
                .spawn(async move { client.afk(Some(want)) })
                .await;
            let _ = this.update(cx, |app: &mut SeanceApp, cx| {
                app.afk.busy = false;
                if let Ok(v) = got {
                    app.apply_afk(from_json(&v, true), cx);
                } else {
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_button_says_what_is_going_on() {
        let off = from_json(&json!({"on": false}), true);
        assert_eq!(chip_label(&off).as_deref(), Some("AFK"));
        let on = from_json(&json!({"on": true, "mac_awake": true}), true);
        assert_eq!(chip_label(&on).as_deref(), Some("AFK on"));
        let warn = from_json(&json!({"on": true, "mac_awake": false, "note": "x"}), true);
        assert_eq!(chip_label(&warn).as_deref(), Some("AFK on, Mac may sleep"));
        assert_eq!(chip_tip(&warn), "x");
    }

    #[test]
    fn other_builds_have_no_button() {
        assert_eq!(chip_label(&AfkUi::default()), None);
        assert_eq!(chip_label(&from_json(&json!({"on": true}), false)), None);
    }
}
