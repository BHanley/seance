//! Which wall torch each pane gets (see `crate::torch`), which panes are on
//! screen to burn one, and how the spinner tick animates them.

use gpui::App;

use crate::pane::Pane;
use crate::torch::Torch;

use super::views::ViewMode;
use super::workspaces::WorkspaceAttention;
use super::SeanceApp;

/// A pane's torch, in the order Stage's `pane_state` reads a pane: exited or
/// asleep (or its process gone, the grey status dot) is out; then needs-you;
/// then working; anything else burns calmly.
pub(super) fn torch_for(
    exited: bool,
    asleep: bool,
    running: bool,
    attention: Option<WorkspaceAttention>,
) -> Torch {
    if exited || asleep || !running {
        return Torch::Stopped;
    }
    match attention {
        Some(WorkspaceAttention::NeedsHuman) => Torch::NeedsYou,
        Some(WorkspaceAttention::Working) => Torch::Working,
        _ => Torch::Idle,
    }
}

fn in_grid(p: &Pane, ws: Option<&str>) -> bool {
    p.tiled && p.popped.is_none() && ws.is_none_or(|w| p.workspace == w)
}

/// Indexes of the panes whose torch is on screen. Classic: the title strips
/// of the selected circle's tiled panes (what `render_tiles` lays out), or
/// just the zoomed one. Chat: the open circle's lead (`chat_lead`), in the
/// thread header. Stage and the Wall draw none. Each pane is
/// `(slug, circle, tiled, popped out)`.
fn on_screen<'a>(
    mode: ViewMode,
    selected: Option<&str>,
    zoomed: Option<&str>,
    chat_lead: Option<&str>,
    panes: impl Iterator<Item = (&'a str, &'a str, bool, bool)>,
) -> Vec<usize> {
    let mut panes = panes;
    match mode {
        ViewMode::Classic => {}
        ViewMode::Chat => {
            return chat_lead
                .and_then(|l| panes.position(|p| p.0 == l))
                .into_iter()
                .collect();
        }
        _ => return Vec::new(),
    }
    let panes: Vec<_> = panes.collect();
    if let Some(z) = zoomed {
        // Zoom shows its pane even when it isn't tiled here.
        if let Some(i) = panes.iter().position(|p| p.0 == z) {
            return vec![i];
        }
    }
    (0..panes.len())
        .filter(|&i| {
            let (_, ws, tiled, popped) = panes[i];
            tiled && !popped && selected.is_none_or(|s| ws == s)
        })
        .collect()
}

/// Chat's decorative torches (rail, header wall, composer) burn whenever
/// Chat shows on a torch scheme, with or without a lead.
fn decor_on_screen(mode: ViewMode, torches: bool) -> bool {
    torches && mode == ViewMode::Chat
}

/// What one 240 ms spinner tick does: torches on screen step a frame,
/// and the window renders once if the spinner or a torch wants it. Torches
/// never add a render the spinner already asked for, and none shows means
/// no render for them at all.
pub(super) fn spinner_tick(spinner: bool, torches: bool) -> (bool, bool) {
    (torches, spinner || torches)
}

impl SeanceApp {
    /// The selected circle's tiled panes, as the grid lays them out.
    pub(super) fn tiled_here(&self) -> Vec<&Pane> {
        let ws = self.selected_workspace.as_deref();
        self.panes.iter().filter(|p| in_grid(p, ws)).collect()
    }

    fn panes_on_screen(&self) -> Vec<&Pane> {
        let picked = on_screen(
            self.view.mode,
            self.selected_workspace.as_deref(),
            self.zoomed_slug.as_deref(),
            self.chat_lead().as_deref(),
            self.panes.iter().map(|p| {
                (
                    p.slug.as_str(),
                    p.workspace.as_str(),
                    p.tiled,
                    p.popped.is_some(),
                )
            }),
        );
        picked.into_iter().map(|i| &self.panes[i]).collect()
    }

    /// This pane's torch, or `None` when the skin has no torches.
    pub(super) fn pane_torch(&self, p: &Pane, cx: &App) -> Option<Torch> {
        if !crate::skin::torches() {
            return None;
        }
        let exited = self.owners.get(&p.slug).is_some_and(|o| o.exited);
        Some(torch_for(
            exited,
            p.asleep,
            p.is_running(cx),
            self.pane_attention(&p.slug),
        ))
    }

    /// Whether any torch is on screen: a pane's, or Chat's decorative ones
    /// (never, outside a skin with torches).
    pub(super) fn torches_on_screen(&self, cx: &App) -> bool {
        let torches = crate::skin::torches();
        decor_on_screen(self.view.mode, torches)
            || (torches
                && self
                    .panes_on_screen()
                    .into_iter()
                    .any(|p| self.pane_torch(p, cx).is_some()))
    }

    /// This pane's torch and the frame to draw it at.
    pub(super) fn pane_torch_frame(&self, p: &Pane, cx: &App) -> Option<(Torch, u64)> {
        self.pane_torch(p, cx).map(|t| (t, self.torch_frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torch_follows_the_pane_state_in_stage_order() {
        use WorkspaceAttention as A;
        // Out beats everything, even a pane asking for you.
        assert_eq!(
            torch_for(true, false, true, Some(A::NeedsHuman)),
            Torch::Stopped
        );
        assert_eq!(
            torch_for(false, true, true, Some(A::Working)),
            Torch::Stopped
        );
        assert_eq!(torch_for(false, false, false, None), Torch::Stopped);
        // Needs-you beats working.
        assert_eq!(
            torch_for(false, false, true, Some(A::NeedsHuman)),
            Torch::NeedsYou
        );
        assert_eq!(
            torch_for(false, false, true, Some(A::Working)),
            Torch::Working
        );
        // Sticky badges don't light the torch up.
        assert_eq!(torch_for(false, false, true, Some(A::Done)), Torch::Idle);
        assert_eq!(torch_for(false, false, true, Some(A::Later)), Torch::Idle);
        assert_eq!(torch_for(false, false, true, None), Torch::Idle);
    }

    #[test]
    fn only_panes_on_screen_light_a_torch_the_tick_can_see() {
        // Ten panes over three circles; "b" is selected with three tiled,
        // one shelved and one popped out.
        let panes = [
            ("a1", "a", true, false),
            ("a2", "a", true, false),
            ("b1", "b", true, false),
            ("b2", "b", true, false),
            ("b3", "b", false, false),
            ("b4", "b", true, true),
            ("b5", "b", true, false),
            ("c1", "c", true, false),
            ("c2", "c", true, false),
            ("c3", "c", true, false),
        ];
        let it = || panes.iter().copied();
        assert_eq!(
            on_screen(ViewMode::Classic, Some("b"), None, None, it()),
            [2, 3, 6]
        );
        assert_eq!(
            on_screen(ViewMode::Classic, Some("b"), Some("b2"), None, it()),
            [3]
        );
        // Stage and the Wall draw no torches, even with a chat lead known.
        for mode in [ViewMode::Stage, ViewMode::Wall] {
            assert!(
                on_screen(mode, Some("b"), None, Some("b1"), it()).is_empty(),
                "{mode:?}"
            );
        }
        // Chat burns only the open circle's lead, in its header, wherever
        // the grid's selection or zoom points.
        assert_eq!(
            on_screen(ViewMode::Chat, Some("b"), Some("b2"), Some("c1"), it()),
            [7]
        );
        // The lead even when shelved: the header shows it, not the grid.
        assert_eq!(
            on_screen(ViewMode::Chat, Some("b"), None, Some("b3"), it()),
            [4]
        );
        // No local lead for the open circle (or no circle open): no torch.
        assert!(on_screen(ViewMode::Chat, Some("b"), None, None, it()).is_empty());
        assert!(on_screen(ViewMode::Chat, Some("b"), None, Some("gone"), it()).is_empty());
        // Classic ignores the chat lead.
        assert_eq!(
            on_screen(ViewMode::Classic, Some("b"), None, Some("c1"), it()),
            [2, 3, 6]
        );
        // An empty circle: nothing to burn, so no torch render.
        assert!(on_screen(ViewMode::Classic, Some("z"), None, None, it()).is_empty());
    }

    #[test]
    fn chat_decor_burns_only_in_chat_on_a_torch_scheme() {
        // Chat with no lead pane still has the rail, wall and composer torches.
        assert!(decor_on_screen(ViewMode::Chat, true));
        assert_eq!(
            spinner_tick(false, decor_on_screen(ViewMode::Chat, true)),
            (true, true)
        );
        // Flat schemes and the other views: none, so no tick for them.
        assert!(!decor_on_screen(ViewMode::Chat, false));
        for mode in [ViewMode::Classic, ViewMode::Stage, ViewMode::Wall] {
            assert!(!decor_on_screen(mode, true), "{mode:?}");
            assert!(!decor_on_screen(mode, false), "{mode:?}");
        }
        assert_eq!(
            spinner_tick(false, decor_on_screen(ViewMode::Stage, true)),
            (false, false)
        );
    }

    #[test]
    fn torches_ride_the_spinner_tick_and_never_render_on_their_own_when_hidden() {
        // (advance a frame, render)
        assert_eq!(spinner_tick(false, false), (false, false));
        assert_eq!(spinner_tick(true, false), (false, true));
        assert_eq!(spinner_tick(false, true), (true, true));
        // Spinner and torches together: one render, not two.
        assert_eq!(spinner_tick(true, true), (true, true));
    }
}
