//! Redraw cost harness (not a regression test): how long one window draw takes
//! after a spinner tick, in a headless gpui window against a THROWAWAY daemon.
//! Run it with `scripts/redraw-bench.sh`, which sets up the daemon and state.

use std::time::{Duration, Instant};

use gpui::AppContext as _;

use super::SeanceApp;

#[gpui::test]
#[ignore = "needs a throwaway daemon: scripts/redraw-bench.sh"]
fn spinner_tick_draw_cost(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
    });
    // Without gpui-component's Root: it wants a real macOS window handle.
    let win = cx.add_window(SeanceApp::new);
    let app = win.root(cx).unwrap();
    cx.executor().allow_parking();
    let draw = |cx: &mut gpui::TestAppContext| {
        cx.update_window(win.into(), |_, w, cx| w.draw(cx).clear())
            .unwrap();
    };
    // Let the daemon's pane list arrive.
    let want: usize = std::env::var("SEANCE_BENCH_PANES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(48);
    for _ in 0..200 {
        cx.run_until_parked();
        if app.read_with(cx, |a, _| a.panes.len()) >= want {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (panes, circles) = app.read_with(cx, |a, _| (a.panes.len(), a.known_workspace_names().len()));
    for _ in 0..5 {
        draw(cx);
    }
    let n = 200u32;
    let mut total = Duration::ZERO;
    let mut worst = Duration::ZERO;
    for _ in 0..n {
        app.update(cx, |a, cx| a.spinner_repaint(cx));
        let t = Instant::now();
        draw(cx);
        let d = t.elapsed();
        total += d;
        worst = worst.max(d);
    }
    eprintln!(
        "[redraw-bench] {panes} panes, {circles} circles: {n} spinner ticks, avg {:.2}ms max {:.2}ms per draw",
        total.as_secs_f64() * 1000.0 / n as f64,
        worst.as_secs_f64() * 1000.0
    );
}
