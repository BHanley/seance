//! Desktop notifications for attention routing (needs-human / ask).
//!
//! Fire-and-forget `notify-send` on Linux, `osascript` on macOS. Silent
//! no-op if the binary is missing.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static LAST_MS: AtomicU64 = AtomicU64::new(0);

/// Dedup window so a thrashing agent can't spam the notification daemon.
const MIN_GAP: Duration = Duration::from_secs(4);

pub fn notify(summary: &str, body: &str) {
    post(&LAST_MS, summary, body);
}

/// Pane alerts (bell, OSC 9/777, finished commands). Own dedup slot, so a
/// bell can't swallow a needs-human or ask notification right behind it.
pub fn notify_alert(summary: &str, body: &str) {
    static LAST_ALERT_MS: AtomicU64 = AtomicU64::new(0);
    post(&LAST_ALERT_MS, summary, body);
}

fn post(last_ms: &AtomicU64, summary: &str, body: &str) {
    let now = Instant::now();
    let now_ms = now.elapsed().as_millis() as u64; // wrong baseline — use wall clock
    let _ = now_ms;
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let prev = last_ms.load(Ordering::Relaxed);
    if wall.saturating_sub(prev) < MIN_GAP.as_millis() as u64 {
        return;
    }
    last_ms.store(wall, Ordering::Relaxed);

    let summary = summary.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        if cfg!(target_os = "macos") {
            // Quotes are escaped for the AppleScript string literals.
            let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                esc(&body),
                esc(&summary)
            );
            let _ = Command::new("osascript")
                .args(["-e", &script])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            return;
        }
        let _ = Command::new("notify-send")
            .args([
                "--app-name=seance",
                "--urgency=normal",
                "--expire-time=12000",
                &summary,
                &body,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Play `bell-audio-path` from `~/.config/seance/terminal.conf` when
/// `bell-features` includes `audio`.
/// Fire-and-forget like [`notify`]: `afplay` on macOS, `pw-play` (falling back
/// to `paplay`) on Linux. A burst of bells inside a second plays once.
pub fn bell_sound() {
    static LAST_BELL_MS: AtomicU64 = AtomicU64::new(0);
    let cfg = crate::term_config::get();
    let Some(path) = cfg.bell_audio_path.clone().filter(|_| cfg.bell_audio) else {
        return;
    };
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if wall.saturating_sub(LAST_BELL_MS.swap(wall, Ordering::Relaxed)) < 1_000 {
        return;
    }
    let volume = cfg.bell_audio_volume.to_string();
    std::thread::spawn(move || {
        let quiet = |c: &mut Command| {
            c.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if cfg!(target_os = "macos") {
            quiet(Command::new("afplay").args(["-v", &volume]).arg(&path));
        } else if !quiet(
            Command::new("pw-play")
                .args(["--volume", &volume])
                .arg(&path),
        ) {
            quiet(Command::new("paplay").arg(&path));
        }
    });
}

pub fn needs_human(pane: &str, note: Option<&str>) {
    let body = match note {
        Some(n) if !n.is_empty() => format!("{pane}: {n}"),
        _ => format!("{pane} needs you"),
    };
    notify("seance · needs human", &body);
}

pub fn ask(from: &str, question: &str) {
    let q = if question.len() > 160 {
        format!("{}…", &question[..160])
    } else {
        question.to_string()
    };
    notify("seance · agent asks", &format!("{from}: {q}"));
}
