//! First-run check: is there a usable Claude Code on this Mac?
//!
//! macOS Local mode only (the caller gates that). Two questions, asked off the
//! UI thread: is `claude` installed, and is it logged in. [`decide`] turns the
//! two answers into a [`Verdict`]; the probes that produce them are passed in
//! so the decision is testable with fakes. Once the check passes, a marker in
//! the state dir stops it from ever running again.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Do new circles open with Claude as their lead? Only the standalone Mac
/// build: macOS, local (not a thin client to a hub), not Brian's own setup
/// (`seance-personal`), and no `seance-new-circle-command` of the user's own.
pub fn new_circles_start_claude(
    macos: bool,
    remote: bool,
    personal: bool,
    custom_command: bool,
) -> bool {
    macos && !remote && !personal && !custom_command
}

/// [`new_circles_start_claude`] for this process.
pub fn starts_claude_here() -> bool {
    new_circles_start_claude(
        cfg!(target_os = "macos"),
        crate::ACTIVE_TUNNEL.get().is_some(),
        crate::term_config::personal(),
        crate::term_config::get()
            .seance_new_circle_command
            .is_some(),
    )
}

static HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// While Claude is missing or logged out, nothing starts a Claude pane on its
/// own (new circles, the Seance Custom helper): each would only show a login
/// screen, and the first-run card is where that gets sorted out. Set while the
/// check runs and the card is up; released when it passes or is dismissed.
pub fn hold_new_claude(on: bool) {
    HELD.store(on, std::sync::atomic::Ordering::SeqCst);
}

pub fn claude_held() -> bool {
    HELD.load(std::sync::atomic::Ordering::SeqCst)
}

/// May a new circle or helper start Claude now?
pub fn may_start_claude(starts_here: bool, held: bool) -> bool {
    starts_here && !held
}

/// Anthropic's official native installer.
pub const INSTALL_COMMAND: &str = "curl -fsSL https://claude.ai/install.sh | bash";

/// What the install pane runs: the installer, then a pause so the output stays
/// readable instead of the pane closing the moment it finishes.
pub fn install_pane_command() -> String {
    format!(
        "bash -c '{INSTALL_COMMAND}; echo; echo \"Done. Go back to the Seance card and press Check again.\"; read -n 1 -s'"
    )
}

/// How long `claude auth status` gets before the answer is "unknown".
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

const MARKER: &str = "claude_ready";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Installed and logged in, or not worth nagging about.
    Ok,
    /// No `claude` binary found.
    Missing,
    /// Found at this path, but not logged in.
    LoggedOut(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    LoggedIn,
    LoggedOut,
    /// The probe timed out or printed something unreadable.
    Unknown,
}

/// Missing beats everything; an unreadable auth answer never shows a card,
/// since a card that cries wolf is worse than a pane that says "log in".
pub fn decide(found: Option<PathBuf>, auth: impl FnOnce(&Path) -> Auth) -> Verdict {
    let Some(path) = found else {
        return Verdict::Missing;
    };
    match auth(&path) {
        Auth::LoggedOut => Verdict::LoggedOut(path),
        Auth::LoggedIn | Auth::Unknown => Verdict::Ok,
    }
}

/// The real check. Blocking: run it on a background thread.
pub fn check() -> Verdict {
    decide(find_claude(), auth_status)
}

/// A login shell sees what Terminal sees (PATH from `.zprofile`, brew, mise);
/// the native installer's default home is the fallback.
pub(crate) fn find_claude() -> Option<PathBuf> {
    let out = Command::new("bash")
        .args(["-lc", "command -v claude"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .trim()
        .to_string();
    // `command -v` can print an alias or function body; only a path counts.
    if out.status.success() && line.starts_with('/') && Path::new(&line).is_file() {
        return Some(PathBuf::from(line));
    }
    let home = std::env::var_os("HOME")?;
    let local = PathBuf::from(home).join(".local/bin/claude");
    local.is_file().then_some(local)
}

fn auth_status(claude: &Path) -> Auth {
    let Ok(mut child) = Command::new(claude)
        .args(["auth", "status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Auth::Unknown;
    };
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(s) = stdout.as_mut() {
            let _ = std::io::Read::read_to_string(s, &mut buf);
        }
        buf
    });
    let deadline = std::time::Instant::now() + AUTH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Auth::Unknown;
            }
        }
    }
    parse_auth(&reader.join().unwrap_or_default())
}

fn parse_auth(json: &str) -> Auth {
    match serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("loggedIn").and_then(|b| b.as_bool()))
    {
        Some(true) => Auth::LoggedIn,
        Some(false) => Auth::LoggedOut,
        None => Auth::Unknown,
    }
}

/// The command a login pane runs: the path we found, quoted, else `claude`.
pub fn login_command(found: &Path) -> String {
    format!("'{}'", found.display().to_string().replace('\'', r"'\''"))
}

fn marker_path() -> Option<PathBuf> {
    crate::state::state_dir().ok().map(|d| d.join(MARKER))
}

/// Has the check passed on this machine before?
pub fn already_passed() -> bool {
    marker_path().is_some_and(|p| p.exists())
}

pub fn mark_passed() {
    if let Some(p) = marker_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, b"1");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(p: &str) -> Option<PathBuf> {
        Some(PathBuf::from(p))
    }

    #[test]
    fn a_hold_stops_new_claude_starts_until_the_check_passes() {
        assert!(may_start_claude(true, false));
        assert!(!may_start_claude(true, true), "held: logged out or missing");
        assert!(!may_start_claude(false, false), "not the standalone build");
        hold_new_claude(true);
        assert!(claude_held());
        hold_new_claude(false);
        assert!(!claude_held());
    }

    #[test]
    fn only_the_standalone_mac_build_starts_circles_with_claude() {
        // (macos, remote, personal, custom command)
        assert!(new_circles_start_claude(true, false, false, false));
        // Brian's own Seance keeps making plain circles.
        assert!(!new_circles_start_claude(true, false, true, false));
        // A thin client's panes run on the hub; Linux is not the Mac build.
        assert!(!new_circles_start_claude(true, true, false, false));
        assert!(!new_circles_start_claude(false, false, false, false));
        // A command the user chose for new circles is theirs to keep.
        assert!(!new_circles_start_claude(true, false, false, true));
    }

    #[test]
    fn missing_binary_is_missing_without_asking_auth() {
        let v = decide(None, |_| panic!("auth must not run without a binary"));
        assert_eq!(v, Verdict::Missing);
    }

    #[test]
    fn logged_out_carries_the_path() {
        let v = decide(at("/x/claude"), |_| Auth::LoggedOut);
        assert_eq!(v, Verdict::LoggedOut(PathBuf::from("/x/claude")));
    }

    #[test]
    fn logged_in_is_ok() {
        assert_eq!(decide(at("/x/claude"), |_| Auth::LoggedIn), Verdict::Ok);
    }

    #[test]
    fn unknown_auth_does_not_nag() {
        assert_eq!(decide(at("/x/claude"), |_| Auth::Unknown), Verdict::Ok);
    }

    #[test]
    fn auth_json_parses_three_ways() {
        assert_eq!(parse_auth(r#"{"loggedIn": true, "x": 1}"#), Auth::LoggedIn);
        assert_eq!(parse_auth(r#"{"loggedIn": false}"#), Auth::LoggedOut);
        assert_eq!(parse_auth("not json"), Auth::Unknown);
        assert_eq!(parse_auth("{}"), Auth::Unknown);
    }

    #[test]
    fn login_command_quotes_paths() {
        assert_eq!(login_command(Path::new("/a b/claude")), "'/a b/claude'");
        assert_eq!(login_command(Path::new("/a'b/claude")), r"'/a'\''b/claude'");
    }
}
