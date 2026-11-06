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

/// Indexes of the panes whose title strip is on screen: the selected
/// circle's tiled panes (what `render_tiles` lays out), or just the zoomed
/// one. Only the classic view draws title strips. Each pane is
/// `(slug, circle, tiled, popped out)`.
fn on_screen<'a>(
    mode: ViewMode,
    selected: Option<&str>,
    zoomed: Option<&str>,
    panes: impl Iterator<Item = (&'a str, &'a str, bool, bool)>,
) -> Vec<usize> {
    if mode != ViewMode::Classic {
        return Vec::new();
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

    /// Whether any pane on screen shows a torch (never, outside a skin
    /// with torches).
    pub(super) fn torches_on_screen(&self, cx: &App) -> bool {
        crate::skin::torches()
            && self
                .panes_on_screen()
                .into_iter()
                .any(|p| self.pane_torch(p, cx).is_some())
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
            on_screen(ViewMode::Classic, Some("b"), None, it()),
            [2, 3, 6]
        );
        assert_eq!(
            on_screen(ViewMode::Classic, Some("b"), Some("b2"), it()),
            [3]
        );
        // Stage, Chat and the Wall draw no title strips, so no torches.
        for mode in [ViewMode::Stage, ViewMode::Chat, ViewMode::Wall] {
            assert!(
                on_screen(mode, Some("b"), None, it()).is_empty(),
                "{mode:?}"
            );
        }
        // An empty circle: nothing to burn, so no torch render.
        assert!(on_screen(ViewMode::Classic, Some("z"), None, it()).is_empty());
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
