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
    let (panes, circles) =
        app.read_with(cx, |a, _| (a.panes.len(), a.known_workspace_names().len()));
    for _ in 0..5 {
        draw(cx);
    }
    let renders = || {
        use std::sync::atomic::Ordering::Relaxed;
        (
            super::panels::RAIL_RENDERS.load(Relaxed),
            super::panels::MAIN_RENDERS.load(Relaxed),
        )
    };
    let n = 200u32;
    let time = |cx: &mut gpui::TestAppContext,
                tick: &dyn Fn(&mut SeanceApp, &mut gpui::Context<SeanceApp>)| {
        let before = renders();
        let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
        for _ in 0..n {
            app.update(cx, |a, cx| tick(a, cx));
            let t = Instant::now();
            draw(cx);
            let d = t.elapsed();
            total += d;
            worst = worst.max(d);
        }
        let after = renders();
        (total, worst, (after.0 - before.0, after.1 - before.1))
    };
    // What the spinner timer does now, then what it did before the split.
    let (total, worst, (rail, main)) = time(cx, &|a, cx| a.spinner_repaint(cx));
    eprintln!(
        "[redraw-bench] {panes} panes, {circles} circles, {n} spinner ticks: avg {:.2}ms max {:.2}ms per draw; rail built {rail}x, main column built {main}x",
        total.as_secs_f64() * 1000.0 / n as f64,
        worst.as_secs_f64() * 1000.0
    );
    assert_eq!(
        (rail, main),
        (n as u64, 0),
        "a spinner tick must build the rail alone"
    );
    let (total, worst, (rail, main)) = time(cx, &|_, cx| cx.notify());
    eprintln!(
        "[redraw-bench] same, but notifying the whole app (a real state change): avg {:.2}ms max {:.2}ms; rail built {rail}x, main column built {main}x",
        total.as_secs_f64() * 1000.0 / n as f64,
        worst.as_secs_f64() * 1000.0
    );
    assert_eq!(
        (rail, main),
        (n as u64, n as u64),
        "a real state change redraws both"
    );
    // A forced full refresh rebuilds every view, cached or not: the cost the
    // window had before the panels were split.
    let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
    for _ in 0..n {
        cx.update_window(win.into(), |_, w, _| w.refresh()).unwrap();
        let t = Instant::now();
        draw(cx);
        let d = t.elapsed();
        total += d;
        worst = worst.max(d);
    }
    eprintln!(
        "[redraw-bench] forced full refresh (every view rebuilt): avg {:.2}ms max {:.2}ms",
        total.as_secs_f64() * 1000.0 / n as f64,
        worst.as_secs_f64() * 1000.0
    );
}

/// The launch bug: a window that starts in Chat paints no terminal, and one in
/// Classic with no circle picked shows every pane a few rows tall. Neither may
/// tell the daemon a pane's size. Run through the throwaway
/// daemon with `SEANCE_BENCH_MODE=chat` (the script writes view.json).
#[gpui::test]
#[ignore = "needs a throwaway daemon: SEANCE_BENCH_MODE=chat|classic scripts/redraw-bench.sh"]
fn a_launch_sends_no_resize(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
    });
    let win = cx.add_window(SeanceApp::new);
    let app = win.root(cx).unwrap();
    cx.executor().allow_parking();
    for _ in 0..60 {
        cx.run_until_parked();
        cx.update_window(win.into(), |_, w, cx| w.draw(cx).clear())
            .unwrap();
        std::thread::sleep(Duration::from_millis(50));
    }
    let mode = app.read_with(cx, |a, _| a.view.mode);
    let sent: Vec<(String, (u16, u16))> = app.read_with(cx, |a, cx| {
        a.panes
            .iter()
            .filter_map(|p| {
                p.remote_terminal()
                    .map(|t| (p.slug.clone(), t.read(cx).sent_size()))
            })
            .filter(|(_, s)| *s != (0, 0))
            .collect()
    });
    eprintln!(
        "[redraw-bench] launched in {mode:?}: {} panes were resized: {:?}",
        sent.len(),
        &sent[..sent.len().min(5)]
    );
    // The one circle that is picked fills the window: that size is real.
    let tiny: Vec<_> = sent.iter().filter(|(_, s)| s.0 < 40 || s.1 < 10).collect();
    assert!(
        tiny.is_empty(),
        "panes told the daemon a tiny size: {tiny:?}"
    );
}
