//! The rail and the main column as their own gpui views.
//!
//! `SeanceApp` is one big view: a `cx.notify()` on it re-lays-out the whole
//! window (about 55 ms with two dozen circles). These two views hold a handle
//! to the app and build the same elements it always built, but gpui can skip
//! a cached view that is not dirty. So a tick that only moves the spinner
//! notifies the rail alone, and the tiles are not laid out again.
//!
//! A view that reads the app is dirtied by any notify of the app, so every
//! real state change still redraws both.

use gpui::{div, prelude::*, Context, Entity, WeakEntity, Window};

use std::sync::atomic::{AtomicU64, Ordering};

use super::SeanceApp;

/// How many times each panel has been built: the counters the redraw tests read.
pub(super) static RAIL_RENDERS: AtomicU64 = AtomicU64::new(0);
pub(super) static MAIN_RENDERS: AtomicU64 = AtomicU64::new(0);

pub(super) struct RailView {
    app: WeakEntity<SeanceApp>,
    _watch: gpui::Subscription,
}

impl RailView {
    /// Redraws whenever `app` notifies, so a real state change reaches it.
    pub(super) fn new(app: &Entity<SeanceApp>, cx: &mut Context<Self>) -> Self {
        Self {
            app: app.downgrade(),
            _watch: cx.observe(app, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for RailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        RAIL_RENDERS.fetch_add(1, Ordering::Relaxed);
        let active = window.is_window_active();
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| {
                app.render_sidebar(active, cx).into_any_element()
            }),
            None => div().into_any_element(),
        }
    }
}

pub(super) struct MainView {
    app: WeakEntity<SeanceApp>,
    _watch: gpui::Subscription,
}

impl MainView {
    /// Redraws whenever `app` notifies, so a real state change reaches it.
    pub(super) fn new(app: &Entity<SeanceApp>, cx: &mut Context<Self>) -> Self {
        Self {
            app: app.downgrade(),
            _watch: cx.observe(app, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        MAIN_RENDERS.fetch_add(1, Ordering::Relaxed);
        let active = window.is_window_active();
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| app.render_main_column(active, cx)),
            None => div().into_any_element(),
        }
    }
}

impl SeanceApp {
    /// Everything right of the rail in Classic: the strips above the grid and
    /// the grid itself.
    pub(super) fn render_main_column(
        &mut self,
        active: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let hold = hold_resizes(
            self.selected_workspace.as_deref(),
            self.known_workspace_names().len(),
        );
        for p in &self.panes {
            if let Some(t) = p.remote_terminal() {
                t.read(cx).set_resize_hold(hold);
            }
        }
        // Timed section construction (SEANCE_DEBUG_RENDER=1 reports per-section
        // avg/max ms — pinpoints which surface makes frames expensive).
        let t1 = std::time::Instant::now();
        let asks_el: Vec<gpui::AnyElement> = self
            .render_asks(cx)
            .into_iter()
            .map(|e| e.into_any_element())
            .collect();
        let shelf_el = self.render_minimize_shelf(active, cx).into_any_element();
        let stage_el = self.render_stage_strip(active, cx).into_any_element();
        let awaken_el = self.render_awaken_bar(cx);
        let daemon_err_el = self.render_daemon_error_bar(cx);
        let t2 = std::time::Instant::now();
        let tiles_el = self.render_tiles(active, cx).into_any_element();
        let t3 = std::time::Instant::now();
        self.render_probe.add("strips", t2 - t1);
        self.render_probe.add("tiles", t3 - t2);

        // min_w_0 is load-bearing: without it the main column's min-content
        // width (sum of tile mins) blocks window shrink and the right edge of
        // the last pane goes off-screen.
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            // Above everything: if the daemon won't have us, nothing below
            // this line is going to explain itself.
            .child(daemon_err_el)
            .children(asks_el)
            .child(shelf_el)
            .child(stage_el)
            .child(awaken_el)
            .child(tiles_el)
            .into_any_element()
    }
}

/// Classic with no circle picked lays every pane of every circle into one
/// grid, a few rows each. That is not a size anyone chose, and every pane
/// would tell the daemon it. A launch painted exactly this for a frame or two
/// and left all of Brian's terminals about 25 columns by 5 rows.
pub(super) fn hold_resizes(selected: Option<&str>, circles: usize) -> bool {
    selected.is_none() && circles > 1
}

#[cfg(test)]
mod tests {
    use super::hold_resizes;

    #[test]
    fn a_grid_of_every_circle_does_not_resize_panes() {
        assert!(hold_resizes(None, 24));
        // A picked circle, or a lone one, is a size someone chose.
        assert!(!hold_resizes(Some("a"), 24));
        assert!(!hold_resizes(None, 1));
        assert!(!hold_resizes(None, 0));
    }
}
