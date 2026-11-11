//! How often the window may rebuild itself without anyone touching it.
//!
//! Every `cx.notify()` on the root re-renders and re-lays-out the WHOLE window,
//! which cost about 55 ms at two dozen circles (a 5 s sample of a running app
//! spent 72% of its main thread in `Window::draw`, half of it in taffy's
//! flexbox layout). The spinner tick, the circle creatures' dance and a
//! streaming pane's grid kicks all end in that notify, so their cadence is
//! what the CPU pays for. The cadence here stretches with what a draw really
//! costs, and drops to a slow tick while the window is not the active one.

use std::sync::atomic::{AtomicU64, Ordering};

/// The spinner / dance / torch beat on a light, active window.
pub(super) const SPINNER_MS: u64 = crate::torch::TICK_MS;
/// The same beat while another app is in front: the window is still visible,
/// but nobody is watching it closely.
const INACTIVE_MS: u64 = 1000;
const SPINNER_MAX_MS: u64 = 1000;
/// Fastest a streaming pane may kick a window frame (about 30 a second).
pub(super) const KICK_MS: u64 = 33;
const KICK_MAX_MS: u64 = 250;
/// Chat's crew dance beat (about 12 a second on a light window).
pub(super) const DANCE_MS: u64 = 83;
const DANCE_MAX_MS: u64 = 333;

/// The recent average cost of one window draw, in microseconds.
static DRAW_US: AtomicU64 = AtomicU64::new(0);

pub(super) fn note_draw_avg_us(us: u64) {
    DRAW_US.store(us, Ordering::Relaxed);
}

pub(super) fn draw_ms() -> u64 {
    DRAW_US.load(Ordering::Relaxed) / 1000
}

/// Wait before the next spinner tick. A busy window never asks for more than
/// about a sixth of a core (the delay is at least six draws long), and an
/// inactive window ticks once a second.
pub(super) fn spinner_delay_ms(active: bool, draw_ms: u64) -> u64 {
    let base = if active { SPINNER_MS } else { INACTIVE_MS };
    base.max(draw_ms * 6).min(SPINNER_MAX_MS)
}

/// Least gap between two grid-driven frame kicks while nobody is typing.
pub(super) fn kick_gap_ms(draw_ms: u64) -> u64 {
    KICK_MS.max(draw_ms * 3).min(KICK_MAX_MS)
}

/// Gap between Chat's crew-dance frames.
pub(super) fn dance_gap_ms(draw_ms: u64) -> u64 {
    DANCE_MS.max(draw_ms * 4).min(DANCE_MAX_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames a timer would ask for in `secs` seconds under a policy: the
    /// counter the redraw problem was missing.
    fn frames(secs: u64, mut next_delay: impl FnMut() -> Option<u64>) -> u64 {
        let (mut t, mut n) = (0, 0);
        while let Some(d) = next_delay() {
            t += d;
            if t > secs * 1000 {
                break;
            }
            n += 1;
        }
        n
    }

    /// A spinner tick with nothing busy asks for no frames at all.
    #[test]
    fn a_static_window_draws_nothing() {
        let tick = |busy: bool| super::super::torches::spinner_tick(busy, false).1;
        assert_eq!(frames(60, || tick(false).then_some(SPINNER_MS)), 0);
    }

    /// The window is behind another app with one pane working and draws that
    /// cost 55 ms: about a frame a second, not four.
    #[test]
    fn an_inactive_busy_window_ticks_once_a_second() {
        let n = frames(60, || Some(spinner_delay_ms(false, 55)));
        assert!(n <= 61, "{n} frames in a minute");
        let duty = n * 55 / 600; // percent of one core
        assert!(duty <= 6, "{duty}% of a core");
    }

    /// The window is in front, still heavy: at most about a sixth of a core.
    #[test]
    fn a_heavy_active_window_stays_under_a_sixth_of_a_core() {
        let n = frames(60, || Some(spinner_delay_ms(true, 55)));
        let duty = n * 55 / 600;
        assert!(duty <= 17, "{duty}% of a core with {n} frames");
    }

    /// A light window keeps the beat it always had: it looks the same.
    #[test]
    fn a_light_active_window_keeps_its_beat() {
        assert_eq!(spinner_delay_ms(true, 4), SPINNER_MS);
        assert_eq!(spinner_delay_ms(true, 0), SPINNER_MS);
        assert_eq!(kick_gap_ms(4), KICK_MS);
        assert_eq!(dance_gap_ms(4), DANCE_MS);
    }

    /// A streaming pane may not kick a heavy window 30 times a second.
    #[test]
    fn a_streaming_pane_cannot_kick_a_heavy_window_at_thirty_a_second() {
        let n = frames(60, || Some(kick_gap_ms(55)));
        assert!(n * 55 / 600 <= 34, "{n} kicks in a minute");
        assert!(kick_gap_ms(55) >= 150);
    }

    #[test]
    fn the_dance_slows_with_a_heavy_window_and_never_stops() {
        assert!(dance_gap_ms(55) >= 200);
        assert!(dance_gap_ms(5000) <= 333);
        assert!(spinner_delay_ms(true, 5000) <= 1000);
        assert!(kick_gap_ms(5000) <= 250);
    }
}
