//! The welcome: three short cards the first time the standalone app opens.
//! Skip on any card ends it for good (a marker in the state folder); Help >
//! Show Welcome Again brings it back without touching the marker.

use std::path::{Path, PathBuf};

use gpui::{div, prelude::*, px, Context};
use gpui_component::StyledExt as _;

use crate::theme::SeancePalette;

use super::SeanceApp;

const MARKER: &str = "welcome-seen";

/// Title and words of each card.
pub(super) const CARDS: [(&str, &str); 3] = [
    (
        "Welcome to Seance.",
        "Each circle is a Claude you can talk to. Start one for any project and it \
         remembers where you left off.",
    ),
    (
        "Chat is where you talk.",
        "The terminal is where Claude works. Peek in anytime, or just stay in Chat.",
    ),
    (
        "Want Claude in your pocket?",
        "Set up your phone in about 5 minutes and get a ping when Claude needs you.",
    ),
];

#[derive(Default)]
pub(super) struct Welcome {
    pub open: bool,
    pub card: usize,
}

/// Show it on the standalone app, once.
pub(super) fn should_show(standalone: bool, seen: bool) -> bool {
    standalone && !seen
}

fn marker_in(dir: &Path) -> PathBuf {
    dir.join(MARKER)
}

pub(super) fn seen_in(dir: &Path) -> bool {
    marker_in(dir).exists()
}

pub(super) fn mark_seen_in(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(marker_in(dir), b"1");
}

fn mark_seen() {
    if let Ok(dir) = crate::state::state_dir() {
        mark_seen_in(&dir);
    }
}

/// What the buttons do. Pure, so the flow is tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Press {
    Next,
    Skip,
    SetUpPhone,
    MaybeLater,
}

/// Where the card goes after a press: the next card, or closed (None). Every
/// way out of the welcome except Next marks it seen.
pub(super) fn after(card: usize, press: Press) -> (Option<usize>, bool) {
    match press {
        Press::Next if card + 1 < CARDS.len() => (Some(card + 1), false),
        Press::Next | Press::Skip | Press::SetUpPhone | Press::MaybeLater => (None, true),
    }
}

impl SeanceApp {
    pub(super) fn start_welcome(&mut self, cx: &mut Context<Self>) {
        let seen = crate::state::state_dir()
            .map(|d| seen_in(&d))
            .unwrap_or(true);
        if should_show(crate::claude_check::starts_claude_here(), seen) {
            self.welcome = Welcome {
                open: true,
                card: 0,
            };
            cx.notify();
        }
    }

    /// Help > Show Welcome Again.
    pub(super) fn show_welcome_again(&mut self, cx: &mut Context<Self>) {
        self.welcome = Welcome {
            open: true,
            card: 0,
        };
        cx.notify();
    }

    fn welcome_press(&mut self, press: Press, cx: &mut Context<Self>) {
        let (next, seen) = after(self.welcome.card, press);
        // The very first time through, the setup kit's tiles come after the
        // welcome (not after Show Welcome Again, and not when the phone guide
        // opens). `kit::first_run` keeps it to once.
        let first_time = seen
            && press != Press::SetUpPhone
            && crate::state::state_dir().is_ok_and(|d| !seen_in(&d));
        if seen {
            mark_seen();
        }
        if first_time && crate::kit::first_run(&crate::kit::Env::real(), false) {
            self.kit_open(super::kit_ui::KitView::Tiles, cx);
        }
        match next {
            Some(card) => self.welcome.card = card,
            None => self.welcome.open = false,
        }
        if press == Press::SetUpPhone {
            self.open_phone_guide(cx);
        }
        cx.notify();
    }

    /// Drawn only when the Claude check is not showing its own card.
    pub(super) fn render_welcome(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        if !self.welcome.open || !matches!(self.first_run, super::first_run::FirstRun::Off) {
            return None;
        }
        let card = self.welcome.card.min(CARDS.len() - 1);
        let (title, body) = CARDS[card];
        let last = card + 1 == CARDS.len();
        let button = |id: &'static str, label: &'static str, primary: bool, press: Press| {
            let b = div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(SeancePalette::border()));
            let b = if primary {
                b.bg(SeancePalette::surface())
                    .text_color(SeancePalette::flame())
            } else {
                b.text_color(SeancePalette::text_faint())
            };
            b.child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.welcome_press(press, cx)))
        };
        let dots = div()
            .flex()
            .flex_row()
            .gap_1()
            .children((0..CARDS.len()).map(|i| {
                div().w(px(6.)).h(px(6.)).rounded_full().bg(if i == card {
                    SeancePalette::flame()
                } else {
                    SeancePalette::border()
                })
            }));
        let buttons = if last {
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(button(
                    "welcome-later",
                    "Maybe later",
                    false,
                    Press::MaybeLater,
                ))
                .child(button(
                    "welcome-phone",
                    "Set up phone",
                    true,
                    Press::SetUpPhone,
                ))
        } else {
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(button("welcome-skip", "Skip", false, Press::Skip))
                .child(button("welcome-next", "Next", true, Press::Next))
        };
        Some(
            div()
                .id("welcome-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.5))
                .occlude()
                .child(
                    div()
                        .id("welcome-card")
                        .w(px(420.))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(SeancePalette::flame_dim())
                        .bg(SeancePalette::bg_elevated())
                        .child(
                            div()
                                .w_full()
                                .text_lg()
                                .font_semibold()
                                .text_color(SeancePalette::flame())
                                .child(title),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_sm()
                                .text_color(SeancePalette::text())
                                .child(body),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .child(dots)
                                .child(buttons),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_once_on_the_standalone_app_only() {
        assert!(should_show(true, false));
        assert!(!should_show(true, true));
        assert!(!should_show(false, false));
    }

    #[test]
    fn the_seen_flag_persists_in_the_state_folder() {
        let dir = std::env::temp_dir().join(format!("welcome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!seen_in(&dir));
        mark_seen_in(&dir);
        assert!(seen_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_walks_the_cards_and_every_other_way_out_marks_it_seen() {
        assert_eq!(after(0, Press::Next), (Some(1), false));
        assert_eq!(after(1, Press::Next), (Some(2), false));
        assert_eq!(after(2, Press::Next), (None, true));
        for card in 0..CARDS.len() {
            for press in [Press::Skip, Press::SetUpPhone, Press::MaybeLater] {
                assert_eq!(after(card, press), (None, true));
            }
        }
    }

    #[test]
    fn the_copy_is_the_agreed_three_cards() {
        assert_eq!(CARDS.len(), 3);
        assert!(CARDS[0].0.starts_with("Welcome to Seance"));
        assert!(CARDS[2].1.contains("about 5 minutes"));
    }
}
