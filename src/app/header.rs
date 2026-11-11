//! The header Chat, Classic and Stage share: the mark with the AFK chip, the
//! classic / stage / chat switch under it, and the square "+" each view puts
//! at the end of its own row. In Chat and Classic the "+" opens a menu of
//! New circle and New space; in Stage the tabs row's "+" adds a space and the
//! circle row's adds a circle.
//!
//! What a menu item or a chord does lives in two pure functions, `new_flow`
//! and `new_kind_for_chord`, so the table is tested without a window.

use gpui::{
    anchored, deferred, div, point, prelude::*, px, Anchor, AnyElement, ClickEvent, Context,
    SharedString, Window,
};

use crate::theme::SeancePalette;

use super::util::{sidebar_press_no_select, tip};
use super::views::ViewMode;
use super::SeanceApp;

/// The "+" button's side, the same in every view.
pub(super) const PLUS_SIZE: f32 = 26.;
/// The menu card's width.
const MENU_W: f32 = 230.;

/// What a "+" or a chord makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NewKind {
    Circle,
    Space,
}

/// Which existing flow a New runs, per view. Circles land in the space on
/// show; Chat opens the new circle's chat, Stage and Classic name it in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NewFlow {
    ChatCircle,
    ClassicCircle,
    StageCircle,
    Space,
}

/// What a "+" button does when clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlusAction {
    /// Open the New circle / New space menu.
    Menu,
    /// Make one thing straight away (Stage's two rows).
    Direct(NewKind),
}

/// The flow for `kind` in `mode`. The Wall has no "+", so none there.
pub(super) fn new_flow(mode: ViewMode, kind: NewKind) -> Option<NewFlow> {
    match (mode, kind) {
        (ViewMode::Wall, _) => None,
        (_, NewKind::Space) => Some(NewFlow::Space),
        (ViewMode::Chat, NewKind::Circle) => Some(NewFlow::ChatCircle),
        (ViewMode::Stage, NewKind::Circle) => Some(NewFlow::StageCircle),
        (ViewMode::Classic, NewKind::Circle) => Some(NewFlow::ClassicCircle),
    }
}

/// The chord for New circle and New space. ctrl+shift+n and cmd+shift+n are
/// already "new pane", so New space is not shift+cmd+n. On a mac: cmd+n is a
/// circle, cmd+option+n a space. Elsewhere ctrl+n belongs to the terminal, so
/// ctrl+alt+n is a circle and ctrl+alt+shift+n a space.
pub(super) fn new_kind_for_chord(key: &str, mods: &gpui::Modifiers, mac: bool) -> Option<NewKind> {
    if key != "n" {
        return None;
    }
    if mac {
        if !mods.platform || mods.control || mods.shift {
            return None;
        }
        return Some(if mods.alt {
            NewKind::Space
        } else {
            NewKind::Circle
        });
    }
    if !mods.control || !mods.alt || mods.platform {
        return None;
    }
    Some(if mods.shift {
        NewKind::Space
    } else {
        NewKind::Circle
    })
}

/// The chord as the menu shows it.
fn chord_label(kind: NewKind, mac: bool) -> &'static str {
    match (kind, mac) {
        (NewKind::Circle, true) => "⌘N",
        (NewKind::Space, true) => "⌥⌘N",
        (NewKind::Circle, false) => "ctrl+alt+n",
        (NewKind::Space, false) => "ctrl+alt+shift+n",
    }
}

impl SeanceApp {
    /// Make a circle or a space the way the view's own "+" does.
    pub(super) fn run_new(&mut self, kind: NewKind, window: &mut Window, cx: &mut Context<Self>) {
        self.new_menu = None;
        let Some(flow) = new_flow(self.view.mode, kind) else {
            cx.notify();
            return;
        };
        match flow {
            NewFlow::ChatCircle => {
                // New circles land in the space shown, and open here.
                self.rail_space = Some(self.shown_space());
                let name = self.create_workspace(window, cx);
                self.stage.new_circle = Some(name.clone());
                self.chat.open_new_circle(&name);
            }
            NewFlow::ClassicCircle => {
                let _ = self.create_workspace(window, cx);
            }
            NewFlow::StageCircle => {
                // New circles land in the rail's space; make that this one.
                self.rail_space = Some(self.shown_space());
                let name = self.create_workspace(window, cx);
                self.stage.new_circle = Some(name);
            }
            NewFlow::Space => self.create_space(window, cx),
        }
        cx.notify();
    }

    /// The logo row (mark, AFK chip) with the view switch under it at full
    /// width. `switch_key` keeps each view's switch ids apart.
    pub(super) fn render_brand_block(
        &self,
        mark_key: &str,
        switch_key: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    // The mark opens the connected guis and the theme list;
                    // the AFK chip is absent where the script is not installed.
                    .child(self.render_seance_mark(mark_key, cx))
                    .children(self.render_afk_chip(mark_key, cx)),
            )
            .child(self.render_view_switch(switch_key, cx))
            .into_any_element()
    }

    /// The square "+": one size, border and colour in every view.
    pub(super) fn render_plus_button(
        &self,
        id: &str,
        tooltip: &'static str,
        action: PlusAction,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(id.to_string()))
            .flex_none()
            .size(px(PLUS_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .border_1()
            .border_color(SeancePalette::border())
            .bg(SeancePalette::surface())
            .text_sm()
            .text_color(SeancePalette::text_dim())
            .cursor_pointer()
            .hover(|s| {
                s.border_color(SeancePalette::flame_dim())
                    .text_color(SeancePalette::text())
            })
            .tooltip(tip(tooltip))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_this, _, window, cx| {
                    sidebar_press_no_select(window, cx);
                }),
            )
            .on_click(
                cx.listener(move |this, ev: &ClickEvent, window, cx| match action {
                    PlusAction::Menu => {
                        this.new_menu = if this.new_menu.is_some() {
                            None
                        } else {
                            Some(ev.position())
                        };
                        cx.notify();
                    }
                    PlusAction::Direct(kind) => this.run_new(kind, window, cx),
                }),
            )
            .child("+")
            .into_any_element()
    }

    /// The menu a "+" opens: New circle and New space, with their chords,
    /// under the click and over a backdrop that catches the click-away.
    pub(super) fn render_new_menu(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let at = self.new_menu?;
        let mac = cfg!(target_os = "macos");
        let row = |id: &'static str, label: &'static str, kind: NewKind| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .text_color(SeancePalette::text())
                .cursor_pointer()
                .hover(|s| s.bg(SeancePalette::surface()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_new(kind, window, cx);
                }))
                .child(label)
                .child(
                    div()
                        .text_xs()
                        .text_color(SeancePalette::text_faint())
                        .child(chord_label(kind, mac)),
                )
        };
        let card = div()
            .id("new-menu")
            .w(px(MENU_W))
            .flex()
            .flex_col()
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(SeancePalette::border())
            .bg(SeancePalette::bg_elevated())
            .shadow_lg()
            // Clicks inside the card must not reach the backdrop.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(row("new-menu-circle", "New circle", NewKind::Circle))
            .child(row("new-menu-space", "New space", NewKind::Space));
        Some(
            deferred(
                div()
                    .id("new-menu-backdrop")
                    .absolute()
                    .inset_0()
                    // Swallow mouse events so a click-away does not also land
                    // in the terminal underneath.
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.new_menu = None;
                        cx.notify();
                    }))
                    .child(
                        anchored()
                            .position(point(at.x + px(12.), at.y + px(14.)))
                            .anchor(Anchor::TopRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(card),
                    ),
            )
            .with_priority(10)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(platform: bool, control: bool, alt: bool, shift: bool) -> gpui::Modifiers {
        gpui::Modifiers {
            platform,
            control,
            alt,
            shift,
            ..Default::default()
        }
    }

    #[test]
    fn each_menu_item_runs_the_flow_of_the_view_it_is_in() {
        let circle = NewKind::Circle;
        assert_eq!(new_flow(ViewMode::Chat, circle), Some(NewFlow::ChatCircle));
        assert_eq!(
            new_flow(ViewMode::Classic, circle),
            Some(NewFlow::ClassicCircle)
        );
        assert_eq!(
            new_flow(ViewMode::Stage, circle),
            Some(NewFlow::StageCircle)
        );
        for mode in [ViewMode::Chat, ViewMode::Classic, ViewMode::Stage] {
            assert_eq!(new_flow(mode, NewKind::Space), Some(NewFlow::Space));
        }
    }

    #[test]
    fn the_wall_makes_nothing() {
        assert_eq!(new_flow(ViewMode::Wall, NewKind::Circle), None);
        assert_eq!(new_flow(ViewMode::Wall, NewKind::Space), None);
    }

    #[test]
    fn cmd_n_is_a_circle_and_cmd_option_n_a_space_on_a_mac() {
        assert_eq!(
            new_kind_for_chord("n", &mods(true, false, false, false), true),
            Some(NewKind::Circle)
        );
        assert_eq!(
            new_kind_for_chord("n", &mods(true, false, true, false), true),
            Some(NewKind::Space)
        );
    }

    #[test]
    fn ctrl_alt_n_is_a_circle_and_ctrl_alt_shift_n_a_space_elsewhere() {
        assert_eq!(
            new_kind_for_chord("n", &mods(false, true, true, false), false),
            Some(NewKind::Circle)
        );
        assert_eq!(
            new_kind_for_chord("n", &mods(false, true, true, true), false),
            Some(NewKind::Space)
        );
    }

    #[test]
    fn other_chords_are_left_alone() {
        // "new pane" keeps ctrl+shift+n and cmd+shift+n.
        for mac in [true, false] {
            assert_eq!(
                new_kind_for_chord("n", &mods(false, true, false, true), mac),
                None
            );
            assert_eq!(
                new_kind_for_chord("n", &mods(true, false, false, true), mac),
                None
            );
            assert_eq!(
                new_kind_for_chord("n", &mods(false, false, false, false), mac),
                None
            );
            assert_eq!(
                new_kind_for_chord("m", &mods(true, true, true, false), mac),
                None
            );
        }
        // Plain ctrl+n and cmd+n off a mac stay with the terminal.
        assert_eq!(
            new_kind_for_chord("n", &mods(false, true, false, false), false),
            None
        );
        assert_eq!(
            new_kind_for_chord("n", &mods(true, false, false, false), false),
            None
        );
    }
}
