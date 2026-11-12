//! Signing in to Claude Code from inside the app, with no terminal: find the
//! binary, install it ourselves with a real progress number, run
//! `claude auth login` as a child we hold on to, and read `claude auth status
//! --json` for who is signed in and when that runs out.
//!
//! Everything that decides something is a pure function with a test (URL
//! extraction from the login output, the manifest and checksum, the expiry
//! window); the processes around them are thin. Everything here blocks: run
//! it on a background thread.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the user's login shell gets to report its PATH.
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// Where `claude` usually lands, in the order we look when the user's shell
/// doesn't say: the native installer, Homebrew (Apple silicon, then Intel),
/// npm's global prefix, and the old `claude migrate-installer` home.
fn fallback_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local/bin/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        home.join(".npm-global/bin/claude"),
        home.join(".claude/local/claude"),
    ]
}

/// The first `claude` that exists: along the login shell's PATH, in PATH
/// order (what Terminal would run), then the usual install spots. A path
/// that isn't absolute is skipped. `exists` is a parameter so the order is
/// testable without touching the disk.
fn pick_claude(
    shell_path: Option<&str>,
    home: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    shell_path
        .into_iter()
        .flat_map(|p| p.split(':'))
        .filter(|d| d.starts_with('/'))
        .map(|d| Path::new(d).join("claude"))
        .chain(fallback_paths(home))
        .find(|p| exists(p))
}

/// The PATH of the user's login shell. Interactive too (`-lic`), so what
/// `.zshrc` adds (nvm, mise, brew shellenv) counts, which `bash -lc` never
/// saw. We read PATH rather than ask `command -v`, which answers with alias
/// text when `claude` is an alias. Our own marker line picks the answer out
/// of whatever the rc files print.
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| s.starts_with('/'))
        .unwrap_or_else(|| "/bin/zsh".into());
    let mut child = Command::new(shell)
        .args(["-lic", "printf '\\n__SEANCE_PATH__%s\\n' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(s) = stdout.as_mut() {
            let _ = std::io::Read::read_to_string(s, &mut buf);
        }
        buf
    });
    let deadline = std::time::Instant::now() + SHELL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = reader.join().unwrap_or_default();
    out.lines()
        .rev()
        .find_map(|l| l.strip_prefix("__SEANCE_PATH__"))
        .map(str::to_string)
}

pub(crate) fn find_claude() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    pick_claude(login_shell_path().as_deref(), &home, |p| p.is_file())
}

// ---- reading the login's output ----

/// `s` without terminal escape sequences (colors, cursor moves, OSC-8 links).
pub fn strip_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.next() {
            // CSI: parameters, then a final byte in @..~
            Some('[') => {
                for n in it.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \
            Some(']') => {
                while let Some(n) = it.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The sign-in link the login prints ("If the browser didn't open, visit:
/// <url>"). It arrives as an OSC-8 hyperlink with the URL inside the escape
/// and again as the visible text, so take the first complete https URL from
/// either, escapes stripped.
pub fn visit_url(output: &str) -> Option<String> {
    let at = output.find("visit:")?;
    let rest = &output[at + "visit:".len()..];
    let plain = strip_escapes(rest);
    let url = plain.split_whitespace().next()?;
    // The text can be cut mid-line while the child is still writing; the
    // escape form carries the same URL ahead of it, so prefer that.
    let link = rest
        .find("\u{1b}]8;;")
        .map(|i| &rest[i + "\u{1b}]8;;".len()..])
        .and_then(|r| r.split(['\u{1b}', '\u{7}']).next())
        .filter(|u| u.starts_with("https://"));
    let pick = link.unwrap_or(url);
    (pick.starts_with("https://") && pick.len() > "https://".len()).then(|| pick.to_string())
}

// ---- who is signed in, and until when ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub logged_in: bool,
    pub email: Option<String>,
    /// `loginExpiresAt`, as unix seconds.
    pub expires_at: Option<i64>,
}

pub fn parse_status(json: &str) -> Option<Status> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(Status {
        logged_in: v.get("loggedIn")?.as_bool()?,
        email: v.get("email").and_then(|e| e.as_str()).map(str::to_string),
        expires_at: v
            .get("loginExpiresAt")
            .and_then(|e| e.as_str())
            .and_then(parse_utc),
    })
}

/// "2026-11-28T06:22:15.024Z" (UTC, the international date-time format) as
/// unix seconds. Only the shape `claude` prints; anything else is None.
pub fn parse_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

/// How soon before it runs out the corner card starts to ask.
pub const EXPIRY_WINDOW_DAYS: i64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// Runs out in this many days (rounded up, at least 1).
    ExpiresIn(i64),
    /// Signed out, or the date has passed.
    Expired,
}

/// Does this status warrant the "sign in again" card? Signed out always;
/// signed in only when an expiry date is near or past. A sign-in with no
/// expiry (an API key, say) never nags.
pub fn expiry_notice(status: &Status, now: i64) -> Option<Notice> {
    if !status.logged_in {
        return Some(Notice::Expired);
    }
    let left = status.expires_at? - now;
    if left <= 0 {
        Some(Notice::Expired)
    } else if left <= EXPIRY_WINDOW_DAYS * 86400 {
        Some(Notice::ExpiresIn((left + 86399) / 86400))
    } else {
        None
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Run `cmd` and return its stdout, or None if it didn't finish in `limit`.
fn capture(mut cmd: Command, limit: Duration) -> Option<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.read_to_string(&mut buf);
        }
        buf
    });
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// `claude auth status --json`, read. None when it can't be asked or read.
pub fn fetch_status(claude: &Path) -> Option<Status> {
    let mut cmd = Command::new(claude);
    cmd.args(["auth", "status", "--json"]);
    parse_status(&capture(cmd, Duration::from_secs(10))?)
}

// ---- installing ----

const RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";

/// The manifest's key for this Mac's chip.
pub fn platform_key(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" | "arm64" => Some("darwin-arm64"),
        "x86_64" => Some("darwin-x64"),
        _ => None,
    }
}

/// (sha256, size in bytes) for `platform` out of a release manifest.
pub fn parse_manifest(json: &str, platform: &str) -> Option<(String, u64)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let p = v.get("platforms")?.get(platform)?;
    let sum = p.get("checksum")?.as_str()?.to_ascii_lowercase();
    let size = p.get("size")?.as_u64().filter(|s| *s > 0)?;
    is_sha256(&sum).then_some((sum, size))
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A version the feed hands back must look like one before it goes in a URL.
pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() < 40
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && v.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Does the checksum we computed match the manifest's? (Case-insensitive; an
/// empty or malformed answer never matches.)
pub fn checksum_matches(actual: &str, expected: &str) -> bool {
    is_sha256(actual) && is_sha256(expected) && actual.eq_ignore_ascii_case(expected)
}

/// Download progress as a whole percent, never past 99 until the file is
/// checked and set up.
pub fn percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((done.min(total) * 100 / total) as u8).min(99)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    Starting,
    Downloading { done: u64, total: u64 },
    Verifying,
    SettingUp,
    Failed(String),
    Done(PathBuf),
}

pub type SharedProgress = Arc<Mutex<Progress>>;

fn set(p: &SharedProgress, v: Progress) {
    if let Ok(mut g) = p.lock() {
        *g = v;
    }
}

/// Install Claude Code ourselves, the way Anthropic's installer script does
/// but with progress we can show: latest version, manifest, the binary for
/// this chip, sha256 against the manifest, then `<binary> install` (which
/// puts `claude` in ~/.local/bin). Blocking. `cancel` stops it between and
/// during steps. The end state is in `progress`.
pub fn install(progress: &SharedProgress, cancel: &AtomicBool) {
    let result = install_inner(progress, cancel);
    set(
        progress,
        match result {
            Ok(path) => Progress::Done(path),
            Err(e) => Progress::Failed(e),
        },
    );
}

fn curl_text(url: &str) -> Result<String, String> {
    let mut cmd = Command::new("/usr/bin/curl");
    cmd.args(["-fsSL", "--connect-timeout", "15", "--max-time", "60", url]);
    capture(cmd, Duration::from_secs(70))
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "Couldn't reach the Claude download server. Check your connection.".into())
}

fn install_inner(progress: &SharedProgress, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let stopped = || cancel.load(Ordering::SeqCst);
    let platform = platform_key(std::env::consts::ARCH)
        .ok_or("This Mac's chip isn't supported by the Claude installer.")?;
    set(progress, Progress::Starting);
    let version = curl_text(&format!("{RELEASES}/latest"))?.trim().to_string();
    if !valid_version(&version) {
        return Err("The Claude download server sent something unexpected.".into());
    }
    let manifest = curl_text(&format!("{RELEASES}/{version}/manifest.json"))?;
    let (sum, total) = parse_manifest(&manifest, platform)
        .ok_or("The Claude download server sent something unexpected.")?;
    if stopped() {
        return Err("Cancelled.".into());
    }

    let dir = std::env::temp_dir().join(format!("seance-claude-install-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make a download folder: {e}"))?;
    let file = dir.join(format!("claude-{version}-{platform}"));
    let cleanup = |dir: &Path| {
        let _ = std::fs::remove_dir_all(dir);
    };

    set(progress, Progress::Downloading { done: 0, total });
    let mut child = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--connect-timeout", "15", "-o"])
        .arg(&file)
        .arg(format!("{RELEASES}/{version}/{platform}/claude"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start the download: {e}"))?;
    let status = loop {
        if stopped() {
            let _ = child.kill();
            let _ = child.wait();
            cleanup(&dir);
            return Err("Cancelled.".into());
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                let done = std::fs::metadata(&file).map_or(0, |m| m.len());
                set(progress, Progress::Downloading { done, total });
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => {
                cleanup(&dir);
                return Err(format!("The download stopped: {e}"));
            }
        }
    };
    if !status.success() {
        cleanup(&dir);
        return Err("The download didn't finish. Check your connection and try again.".into());
    }

    set(progress, Progress::Verifying);
    let mut cmd = Command::new("/usr/bin/shasum");
    cmd.args(["-a", "256"]).arg(&file);
    let actual = capture(cmd, Duration::from_secs(120))
        .and_then(|o| o.split_whitespace().next().map(str::to_string))
        .unwrap_or_default();
    if !checksum_matches(&actual, &sum) {
        cleanup(&dir);
        return Err("The download didn't match its checksum, so it wasn't installed.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755));
    }

    set(progress, Progress::SettingUp);
    let mut child = Command::new(&file)
        .arg("install")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            cleanup(&dir);
            format!("Couldn't run the installer: {e}")
        })?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let ok = loop {
        if stopped() || Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            cleanup(&dir);
            return Err(if stopped() {
                "Cancelled.".into()
            } else {
                "Setting up Claude Code took too long.".into()
            });
        }
        match child.try_wait() {
            Ok(Some(s)) => break s.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break false,
        }
    };
    cleanup(&dir);
    if !ok {
        return Err("Setting up Claude Code didn't finish.".into());
    }
    find_claude().ok_or_else(|| "Claude Code installed, but Seance can't find it.".into())
}

// ---- the login child ----

/// A running `claude auth login`: stdout collected for the link, stdin held
/// open so a pasted code can reach it, and gone when this is dropped.
pub struct Login {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Arc<Mutex<String>>,
    started: Instant,
}

/// The longest a login is left running.
pub const LOGIN_CAP: Duration = Duration::from_secs(10 * 60);

impl Login {
    pub fn start(claude: &Path) -> Result<Login, String> {
        let mut child = Command::new(claude)
            .args(["auth", "login"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start the Claude sign-in: {e}"))?;
        let stdin = child.stdin.take();
        let out = Arc::new(Mutex::new(String::new()));
        if let Some(mut stdout) = child.stdout.take() {
            let sink = out.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stdout.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut g) = sink.lock() {
                        g.push_str(&String::from_utf8_lossy(&buf[..n]));
                    }
                }
            });
        }
        Ok(Login {
            child,
            stdin,
            out,
            started: Instant::now(),
        })
    }

    /// The link to open by hand, once the login has printed it.
    pub fn url(&self) -> Option<String> {
        visit_url(&self.out.lock().ok()?)
    }

    /// Hand the code from the browser page to the login. False if it can't
    /// be written.
    pub fn send_code(&mut self, code: &str) -> bool {
        let code = code.trim();
        if code.is_empty() {
            return false;
        }
        self.stdin
            .as_mut()
            .is_some_and(|s| writeln!(s, "{code}").and_then(|_| s.flush()).is_ok())
    }

    /// Has the login process ended (signed in, or gave up)?
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    pub fn over_cap(&self) -> bool {
        self.started.elapsed() > LOGIN_CAP
    }
}

impl Drop for Login {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---- the first-run hold and marker ----

static HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// While Claude is missing or logged out, nothing starts a Claude pane on its
/// own (new circles, the Seance Custom helper): each would only show a login
/// screen, and the first-run card is where that gets sorted out. Set while the
/// check runs and the card is up; released when it passes or is dismissed.
pub fn hold_new_claude(on: bool) {
    HELD.store(on, std::sync::atomic::Ordering::SeqCst);
}

#[allow(dead_code)] // read by the standalone's new-circle start
pub fn claude_held() -> bool {
    HELD.load(std::sync::atomic::Ordering::SeqCst)
}

/// May a new circle or helper start Claude now?
#[allow(dead_code)] // the standalone's new-circle gate
pub fn may_start_claude(starts_here: bool, held: bool) -> bool {
    starts_here && !held
}

const MARKER: &str = "claude_ready";

fn marker_path() -> Option<PathBuf> {
    crate::state::state_dir().ok().map(|d| d.join(MARKER))
}

/// Has the check passed on this machine before?
#[allow(dead_code)] // the standalone's first-run check
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
    fn claude_is_found_in_the_shell_path_first_then_the_usual_spots() {
        let home = Path::new("/Users/me");
        let has = |present: &'static [&'static str]| {
            move |p: &Path| present.iter().any(|x| Path::new(x) == p)
        };
        let all = &[
            "/opt/homebrew/bin/claude",
            "/Users/me/.local/bin/claude",
            "/usr/local/bin/claude",
            "/Users/me/.npm-global/bin/claude",
            "/Users/me/.claude/local/claude",
            "/Users/me/.nvm/v22/bin/claude",
        ];
        // The shell's PATH wins, in PATH order (an nvm install from .zshrc).
        assert_eq!(
            pick_claude(
                Some("/x:/Users/me/.nvm/v22/bin:/opt/homebrew/bin"),
                home,
                has(all)
            ),
            at("/Users/me/.nvm/v22/bin/claude")
        );
        // No shell answer: the fixed list, native installer first.
        assert_eq!(
            pick_claude(None, home, has(all)),
            at("/Users/me/.local/bin/claude")
        );
        let rest = &all[2..5];
        let order: Vec<_> = (0..3)
            .map(|i| pick_claude(None, home, has(&rest[i..])))
            .collect();
        assert_eq!(
            order,
            vec![
                at("/usr/local/bin/claude"),
                at("/Users/me/.npm-global/bin/claude"),
                at("/Users/me/.claude/local/claude"),
            ]
        );
        assert_eq!(
            pick_claude(
                None,
                home,
                has(&["/opt/homebrew/bin/claude", "/usr/local/bin/claude"])
            ),
            at("/opt/homebrew/bin/claude")
        );
        // Relative PATH entries are ignored; nothing anywhere is None.
        assert_eq!(pick_claude(Some("bin:.:"), home, has(&[])), None);
    }

    /// What the login really printed (link shortened), OSC-8 and all.
    const LOGIN_OUT: &str = "Opening browser to sign in…\nIf the browser didn't open, visit: \u{1b}]8;;https://claude.com/cai/oauth/authorize?code=true&state=abc\u{1b}\\https://claude.com/cai/oauth/authorize?code=true&state=abc\u{1b}]8;;\u{1b}\\\nPaste code here if prompted > ";

    #[test]
    fn the_link_comes_out_of_the_hyperlink_line() {
        assert_eq!(
            visit_url(LOGIN_OUT).as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&state=abc")
        );
        // Plain text, no escapes.
        assert_eq!(
            visit_url("visit: https://x.example/a?b=1\nPaste").as_deref(),
            Some("https://x.example/a?b=1")
        );
        // Not printed yet, or not a link: nothing.
        assert_eq!(visit_url("Opening browser to sign in…\n"), None);
        assert_eq!(visit_url("visit: http://insecure.example"), None);
        assert_eq!(visit_url("visit: "), None);
        assert_eq!(
            strip_escapes("\u{1b}[1mhi\u{1b}[0m \u{1b}]8;;u\u{7}t"),
            "hi t"
        );
    }

    #[test]
    fn status_json_gives_email_and_expiry() {
        let s = parse_status(
            r#"{"loggedIn":true,"email":"a@b.c","loginExpiresAt":"2026-11-28T06:22:15.024Z"}"#,
        )
        .unwrap();
        assert!(s.logged_in);
        assert_eq!(s.email.as_deref(), Some("a@b.c"));
        assert_eq!(s.expires_at, Some(1_795_846_935));
        let out = parse_status(r#"{"loggedIn":false,"loginExpiresAt":null}"#).unwrap();
        assert_eq!(
            (out.logged_in, out.email, out.expires_at),
            (false, None, None)
        );
        assert_eq!(parse_status("not json"), None);
        assert_eq!(parse_status(r#"{"email":"x"}"#), None);
        assert_eq!(parse_utc("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn the_card_shows_inside_three_days_and_when_signed_out() {
        let day = 86400;
        let now = 1_000_000_000;
        let st = |logged_in, exp: Option<i64>| Status {
            logged_in,
            email: None,
            expires_at: exp,
        };
        let n = |s: Status| expiry_notice(&s, now);
        assert_eq!(n(st(true, Some(now + 16 * day))), None);
        assert_eq!(n(st(true, Some(now + 3 * day + 1))), None);
        assert_eq!(n(st(true, Some(now + 3 * day))), Some(Notice::ExpiresIn(3)));
        assert_eq!(
            n(st(true, Some(now + 2 * day - 5))),
            Some(Notice::ExpiresIn(2))
        );
        assert_eq!(n(st(true, Some(now + 60))), Some(Notice::ExpiresIn(1)));
        assert_eq!(n(st(true, Some(now))), Some(Notice::Expired));
        assert_eq!(n(st(true, Some(now - day))), Some(Notice::Expired));
        assert_eq!(n(st(false, None)), Some(Notice::Expired));
        // Signed in with no expiry (an API key) is left alone.
        assert_eq!(n(st(true, None)), None);
    }

    #[test]
    fn the_manifest_and_checksum_gate_the_install() {
        let sum = "a".repeat(64);
        let json = format!(
            r#"{{"platforms":{{"darwin-arm64":{{"checksum":"{}","size":323702464}},"darwin-x64":{{"checksum":"zz","size":5}}}}}}"#,
            sum.to_uppercase()
        );
        assert_eq!(
            parse_manifest(&json, "darwin-arm64"),
            Some((sum.clone(), 323_702_464))
        );
        assert_eq!(parse_manifest(&json, "darwin-x64"), None, "bad checksum");
        assert_eq!(parse_manifest(&json, "linux-x64"), None);
        assert_eq!(platform_key("aarch64"), Some("darwin-arm64"));
        assert_eq!(platform_key("x86_64"), Some("darwin-x64"));
        assert_eq!(platform_key("riscv64"), None);
        assert!(checksum_matches(&sum, &sum.to_uppercase()));
        assert!(!checksum_matches(&sum, &"b".repeat(64)));
        assert!(!checksum_matches("", ""), "empty never matches");
        assert!(valid_version("2.1.322"));
        assert!(!valid_version("../etc"));
        assert!(!valid_version(""));
        assert!(!valid_version("<html>"));
    }

    #[test]
    fn download_percent_stops_short_of_done() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(50, 100), 50);
        assert_eq!(
            percent(100, 100),
            99,
            "verifying and setting up still to go"
        );
        assert_eq!(percent(500, 100), 99);
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
}
