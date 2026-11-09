//! Finishing an update of the standalone Mac app.
//!
//! Updating is a drag: the user drops a new Seance.app over the old one and
//! opens it. The daemon the old app started is still running, and client and
//! daemon must be the exact same build, so the new app is refused at the door.
//! When that happens in macOS Local mode, the app offers (never silently) to
//! restart sessions: sleep every restorable circle through the old daemon,
//! stop it, start the new one, wake the circles that were slept. A Mac with a
//! remote launch preference never gets here, since its daemon is not ours to
//! restart.
//!
//! The old daemon refuses a client whose hello names a different build, and
//! that includes the request to sleep. So the sleep calls here introduce
//! themselves with the daemon's own build (read from its refusal). Only the
//! `list` and `sleep` requests are sent that way, both unchanged for many
//! releases; a daemon too old to know them answers with an error and nothing
//! is stopped.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};

use crate::control::{bind_socket_path, ControlRequest, ControlResponse};

/// `0.27.0` or `v0.27.0` as numbers; a `-rc1` or `+build` tail is dropped.
pub(crate) fn parse_version(v: &str) -> Option<Vec<u64>> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// True when `a` is strictly older than `b` (missing parts count as 0).
fn is_older(a: &str, b: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (parse_version(a), parse_version(b)) else {
        return false;
    };
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a < b
}

/// The build of an older local daemon, read from its refusal text
/// (`version mismatch: daemon is seance X, client is Y …`).
fn refused_daemon_build(refusal: &str) -> Option<String> {
    let rest = refusal
        .split("version mismatch: daemon is seance ")
        .nth(1)?;
    let build = rest.split(',').next()?.trim();
    (parse_version(build).is_some()).then(|| build.to_string())
}

/// Should the app offer to restart sessions? Returns the old daemon's build
/// when the local daemon refused us for being an older build, on macOS in
/// Local mode. A newer daemon (the user dragged in an older app) is not an
/// update, so it falls through to the normal error.
pub fn stale_local_daemon(macos: bool, remote: bool, refusal: &str) -> Option<String> {
    if !macos || remote {
        return None;
    }
    let old = refused_daemon_build(refusal)?;
    is_older(&old, env!("CARGO_PKG_VERSION")).then_some(old)
}

/// One request to the old daemon, introducing ourselves as its own build.
fn ask_as(build: &str, req: &ControlRequest) -> Result<ControlResponse> {
    let stream = UnixStream::connect(bind_socket_path()).context("connect to the old daemon")?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut writer = stream.try_clone()?;
    writer.write_all(seance_core::protocol::hello_line_with("ctl", build).as_bytes())?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    let mut resp = String::new();
    BufReader::new(stream).read_line(&mut resp)?;
    serde_json::from_str(resp.trim()).context("old daemon sent an unreadable reply")
}

/// Every awake pane in the daemon's list, as (pane, circle), in list order.
/// Which of them can sleep is the daemon's call: a plain shell refuses and
/// ends with the old daemon, as it always did.
fn awake_panes(list: &serde_json::Value) -> Vec<(String, String)> {
    list["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| !p["asleep"].as_bool().unwrap_or(false))
        .filter_map(|p| {
            Some((
                p["slug"].as_str()?.to_string(),
                p["workspace"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// What a sleep request left asleep, so the same thing wakes afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Slept {
    /// This pane alone (a daemon that can sleep one pane).
    Pane { pane: String, circle: String },
    /// The whole circle (an older daemon ignores `pane` and sleeps the circle,
    /// or refuses it when a plain shell is in it).
    Circle(String),
}

/// How a successful sleep answer reads: the daemon names the pane when it
/// slept just that one; an older one answers for the circle.
fn slept_from(data: &serde_json::Value, pane: &str, circle: &str) -> Slept {
    if data["pane"].as_str() == Some(pane) {
        Slept::Pane {
            pane: pane.to_string(),
            circle: circle.to_string(),
        }
    } else {
        Slept::Circle(circle.to_string())
    }
}

/// The requests that put `slept` back, one per pane and one per circle.
fn wake_requests(slept: &[Slept]) -> Vec<ControlRequest> {
    let mut seen: Vec<&Slept> = Vec::new();
    let mut out = Vec::new();
    for s in slept {
        if seen.contains(&s) {
            continue;
        }
        seen.push(s);
        let (workspace, pane) = match s {
            Slept::Pane { pane, circle } => (circle.clone(), Some(pane.clone())),
            Slept::Circle(circle) => (circle.clone(), None),
        };
        out.push(ControlRequest::Wake {
            workspace: Some(workspace),
            pane,
            scope: None,
            from: Some("updater".into()),
        });
    }
    out
}

fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Stop the local daemon (SIGTERM, then wait for it to go). Only call once
/// the restorable circles are asleep: whatever is still running dies here.
fn stop_daemon(pid_file: &Path) -> Result<()> {
    let text = std::fs::read_to_string(pid_file).context("no daemon pid file")?;
    let pid: i32 = text.trim().parse().context("bad daemon pid file")?;
    if !process_alive(pid) {
        return Ok(());
    }
    let cmd = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    if !(cmd.contains("seance") && cmd.contains("daemon")) {
        bail!("pid {pid} is not a seance daemon, not stopping it");
    }
    // SAFETY: plain SIGTERM to the verified daemon pid.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    for _ in 0..100 {
        if !process_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(anyhow!("the old daemon did not stop within 10s"))
}

/// Panes and circles woken, as the count reported to the user.
fn wake_count(slept: &[Slept]) -> usize {
    wake_requests(slept).len()
}

/// Sleep what can sleep, replace the old daemon with this build's, wake the
/// circles that were slept. Returns how many circles were put back.
pub fn restart_sessions(old_build: &str) -> Result<usize> {
    let list = ask_as(
        old_build,
        &ControlRequest::List {
            scope: None,
            from: Some("updater".into()),
        },
    )?;
    if !list.ok {
        bail!(
            "the old daemon would not list its circles: {}",
            list.error.unwrap_or_default()
        );
    }
    let mut slept: Vec<Slept> = Vec::new();
    for (pane, circle) in awake_panes(&list.data.unwrap_or_default()) {
        let req = ControlRequest::Sleep {
            workspace: Some(circle.clone()),
            pane: Some(pane.clone()),
            scope: None,
            from: Some("updater".into()),
        };
        // Each Claude pane sleeps on its own, so a plain shell in the same
        // circle no longer keeps it awake. A shell refuses and ends with the
        // daemon; it comes back fresh.
        if let Ok(r) = ask_as(old_build, &req) {
            if r.ok {
                slept.push(slept_from(&r.data.unwrap_or_default(), &pane, &circle));
            }
        }
    }
    stop_daemon(&crate::runtime::daemon_pid_path())?;
    crate::daemon::ensure_local_daemon()?;
    for req in wake_requests(&slept) {
        // This build's own hello, on the local socket (never $SEANCE_SOCKET).
        let _ = ask_as(env!("CARGO_PKG_VERSION"), &req);
    }
    Ok(wake_count(&slept))
}

/// Ask the human, with a native dialog (the main window does not exist yet).
/// True only on an explicit click of the restart button.
#[cfg(target_os = "macos")]
pub fn confirm_restart() -> bool {
    let script = "display dialog \"Seance was updated. Restart sessions to finish.\" \
        with title \"Seance\" buttons {\"Not now\", \"Restart sessions\"} \
        default button \"Restart sessions\" cancel button \"Not now\"";
    Command::new("osascript")
        .args(["-e", script])
        .output()
        .is_ok_and(|o| {
            o.status.success() && String::from_utf8_lossy(&o.stdout).contains("Restart sessions")
        })
}

#[cfg(not(target_os = "macos"))]
pub fn confirm_restart() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFUSAL: &str =
        "version mismatch: daemon is seance 0.1.0, client is 0.27.0 — upgrade the older side";

    #[test]
    fn older_local_daemon_on_a_mac_offers_the_restart() {
        assert_eq!(
            stale_local_daemon(true, false, REFUSAL).as_deref(),
            Some("0.1.0")
        );
        // The refusal arrives wrapped by anyhow context in the real flow.
        assert!(stale_local_daemon(true, false, &format!("local daemon: {REFUSAL}")).is_some());
    }

    #[test]
    fn never_for_remote_linux_or_other_errors() {
        assert!(stale_local_daemon(true, true, REFUSAL).is_none());
        assert!(stale_local_daemon(false, false, REFUSAL).is_none());
        assert!(stale_local_daemon(true, false, "connect to daemon: refused").is_none());
        assert!(stale_local_daemon(true, false, "").is_none());
    }

    #[test]
    fn a_newer_daemon_is_not_an_update() {
        let newer = "version mismatch: daemon is seance 99.0.0, client is 0.27.0 — x";
        assert!(stale_local_daemon(true, false, newer).is_none());
        assert!(is_older("0.9.0", "0.27.0"));
        assert!(!is_older("0.27.0", "0.27.0"));
        assert!(!is_older("0.28.0-rc1", "0.27.0"));
        assert!(!is_older("junk", "0.27.0"));
    }

    #[test]
    fn every_awake_pane_is_asked_to_sleep_on_its_own() {
        let list = serde_json::json!({"panes": [
            {"slug": "sh", "workspace": "a", "asleep": false},
            {"slug": "cl", "workspace": "a", "asleep": false},
            {"slug": "old", "workspace": "a", "asleep": true},
            {"slug": "x", "workspace": "b"},
            {"workspace": "b"},
        ]});
        // A shell and a Claude pane in one circle are both asked, singly.
        assert_eq!(
            awake_panes(&list),
            vec![
                ("sh".to_string(), "a".to_string()),
                ("cl".to_string(), "a".to_string()),
                ("x".to_string(), "b".to_string()),
            ]
        );
        assert!(awake_panes(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn what_woke_matches_what_slept() {
        let one = serde_json::json!({"workspace": "a", "pane": "cl", "slept": 1});
        let old = serde_json::json!({"workspace": "b", "slept": 2});
        let slept = vec![
            slept_from(&one, "cl", "a"),
            slept_from(&old, "p1", "b"),
            slept_from(&old, "p2", "b"),
        ];
        assert_eq!(
            slept[0],
            Slept::Pane {
                pane: "cl".into(),
                circle: "a".into()
            }
        );
        assert_eq!(slept[1], Slept::Circle("b".into()));
        // The old daemon's circle wakes once, not once per pane.
        let reqs = wake_requests(&slept);
        assert_eq!(reqs.len(), 2);
        assert!(matches!(&reqs[0], ControlRequest::Wake { pane: Some(p), .. } if p == "cl"));
        assert!(
            matches!(&reqs[1], ControlRequest::Wake { pane: None, workspace: Some(w), .. } if w == "b")
        );
    }
}
