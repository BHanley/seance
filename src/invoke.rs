//! Invoke for coworkers, inside the Mac app.
//!
//! Invoke is the phone chat for Seance circles (a small web app plus push
//! notifications). On the standalone Mac build the local daemon runs it from a
//! bundled Python runtime, on 127.0.0.1, and stops it with the daemon; the
//! phone reaches it over the owner's own Tailscale (`tailscale serve`). This
//! module is the Rust side only: when to start it, which port, how its child
//! is kept alive, and the checks behind Settings > "Use Seance on your phone".
//! Invoke's own code is not in this repository: the release build copies it
//! into the app bundle from a directory the builder names.
//!
//! Brian's own Seance (`seance-personal = true`) never starts it: he runs his
//! own Invoke elsewhere.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Where Invoke listens unless that port is taken.
pub const DEFAULT_PORT: u16 = 9667;
/// The https port `tailscale serve` puts Invoke on.
pub const HTTPS_PORT: u16 = 8443;
/// The port the running Invoke listens on (0: not running).
static RUNNING_PORT: AtomicU16 = AtomicU16::new(0);

/// Does this daemon start Invoke? Only the standalone Mac build: macOS, not
/// Brian's own setup, not a thin client, and a bundled runtime to run.
pub fn should_start(macos: bool, personal: bool, remote: bool, has_runtime: bool) -> bool {
    macos && !personal && !remote && has_runtime
}

/// The port to listen on: the last one used if it is free (a phone link that
/// was set up keeps pointing at it), else the default, else the next free one.
pub fn pick_port(prev: Option<u16>, is_free: impl Fn(u16) -> bool) -> Option<u16> {
    prev.into_iter()
        .chain(std::iter::once(DEFAULT_PORT))
        .chain(DEFAULT_PORT + 1..DEFAULT_PORT + 40)
        .find(|p| is_free(*p))
}

fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

// ---- Tailscale -------------------------------------------------------------

/// One of the owner's phones on the tailnet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneInfo {
    /// Its Tailscale name (the MagicDNS label, else the host name).
    pub name: String,
    /// "iOS" or "Android".
    pub os: String,
    pub online: bool,
}

/// What `tailscale status --json` says that the guide needs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TsStatus {
    /// BackendState is Running (signed in and connected).
    pub running: bool,
    /// The Mac's own Tailscale login.
    pub login: Option<String>,
    /// Its MagicDNS name, without the trailing dot.
    pub dns: Option<String>,
    /// Phones (iOS or Android) on the same account.
    pub phones: u32,
    pub phones_online: u32,
    /// The same phones by name, online ones first.
    pub phone_list: Vec<PhoneInfo>,
}

pub fn parse_status(v: &Value) -> TsStatus {
    let mut out = TsStatus {
        running: v["BackendState"] == "Running",
        ..Default::default()
    };
    let me = &v["Self"];
    let uid = me["UserID"].as_i64();
    if let Some(uid) = uid {
        out.login = v["User"][uid.to_string()]["LoginName"]
            .as_str()
            .filter(|l| !l.is_empty())
            .map(str::to_string);
    }
    out.dns = me["DNSName"]
        .as_str()
        .map(|d| d.trim_end_matches('.').to_string())
        .filter(|d| !d.is_empty());
    if let (Some(uid), Some(peers)) = (uid, v["Peer"].as_object()) {
        for p in peers.values() {
            let phone = matches!(p["OS"].as_str(), Some("iOS" | "android"));
            if phone && p["UserID"].as_i64() == Some(uid) {
                let online = p["Online"] == true;
                out.phones += 1;
                if online {
                    out.phones_online += 1;
                }
                let name = p["DNSName"]
                    .as_str()
                    .and_then(|d| d.split('.').next())
                    .filter(|n| !n.is_empty())
                    .or_else(|| p["HostName"].as_str().filter(|n| !n.is_empty()))
                    .unwrap_or("a phone")
                    .to_string();
                let os = match p["OS"].as_str() {
                    Some("iOS") => "iOS",
                    _ => "Android",
                };
                out.phone_list.push(PhoneInfo {
                    name,
                    os: os.into(),
                    online,
                });
            }
        }
        // Online first, then by name, so the list does not jump about.
        out.phone_list
            .sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.cmp(&b.name)));
    }
    out
}

/// The https URL `tailscale serve` already gives Invoke's port, from
/// `tailscale serve status --json`.
pub fn serve_url(v: &Value, port: u16) -> Option<String> {
    let target = format!("127.0.0.1:{port}");
    v["Web"].as_object()?.iter().find_map(|(host, web)| {
        let proxied = web["Handlers"]
            .as_object()?
            .values()
            .any(|h| h["Proxy"].as_str().is_some_and(|p| p.contains(&target)));
        proxied.then(|| format!("https://{host}"))
    })
}

/// The tailscale CLI: `SEANCE_TAILSCALE` first (tests, and so a proof can
/// never reach a real one), then PATH, then the places the Mac app puts it. A
/// Finder launch has a minimal PATH, hence the fixed locations.
pub fn find_tailscale() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("SEANCE_TAILSCALE").filter(|p| !p.is_empty()) {
        // An override that does not exist means "no tailscale", not "look
        // elsewhere": a proof must never fall through to a real one.
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("tailscale")));
    }
    candidates.extend(
        [
            "/usr/local/bin/tailscale",
            "/opt/homebrew/bin/tailscale",
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ]
        .map(PathBuf::from),
    );
    candidates.into_iter().find(|p| p.is_file())
}

/// Run `cli args…` for at most `timeout`, returning what it printed (both
/// streams) and whether it exited 0. A command that is still waiting at the
/// deadline (tailscale waits for a click when Serve is not enabled yet) is
/// killed, and what it said so far is kept.
fn run_capture(cli: &Path, args: &[&str], timeout: Duration) -> (bool, String) {
    let Ok(mut child) = Command::new(cli)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return (false, String::new());
    };
    // What the command printed so far, from two reader threads. A reader
    // that is still blocked after the kill (a grandchild holds the pipe) is
    // left behind rather than waited for.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let drain = |s: Option<Box<dyn std::io::Read + Send>>| {
        let seen = std::sync::Arc::clone(&seen);
        std::thread::spawn(move || {
            let Some(mut s) = s else { return };
            let mut buf = [0u8; 1024];
            while let Ok(n) = std::io::Read::read(&mut s, &mut buf) {
                if n == 0 {
                    break;
                }
                seen.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        })
    };
    let readers = [
        drain(child.stdout.take().map(|s| Box::new(s) as _)),
        drain(child.stderr.take().map(|s| Box::new(s) as _)),
    ];
    let deadline = Instant::now() + timeout;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let grace = Instant::now() + Duration::from_millis(300);
    while readers.iter().any(|r| !r.is_finished()) && Instant::now() < grace {
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = seen.lock().unwrap().clone();
    (ok, text)
}

fn status_json(cli: &Path) -> Option<Value> {
    let (ok, out) = run_capture(cli, &["status", "--json"], Duration::from_secs(8));
    // A stopped or signed-out Tailscale still prints a status document.
    let _ = ok;
    serde_json::from_str(&out).ok()
}

// ---- the guide -------------------------------------------------------------

/// Where each step of "Use Seance on your phone" stands.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guide {
    /// This build carries Invoke at all.
    pub bundled: bool,
    /// Invoke is running on this Mac.
    pub invoke_running: bool,
    pub port: u16,
    /// (a) the Tailscale app or CLI is on this Mac.
    pub installed: bool,
    /// (b) signed in and connected.
    pub signed_in: bool,
    pub login: Option<String>,
    /// (c) a phone on this account shows in the tailnet (it can't be
    /// detected otherwise: the guide offers a manual "Done").
    pub phones: u32,
    pub phones_online: u32,
    pub phone_list: Vec<PhoneInfo>,
    /// (d) the phone link (`tailscale serve`) points at Invoke.
    pub link_on: bool,
    /// (e) the address to open on the phone.
    pub url: Option<String>,
    /// (f) phones that opened Invoke and said yes to notifications: the push
    /// subscriptions Invoke has stored. That is the only trace a phone leaves,
    /// so it is what ticks "Put Invoke on your phone".
    #[serde(default)]
    pub subscribed: u32,
}

/// Pure: the guide from what the probes found. `serve` is the status JSON of
/// `tailscale serve`.
pub fn guide_from(
    bundled: bool,
    port: u16,
    cli: bool,
    status: Option<&TsStatus>,
    serve: Option<&Value>,
) -> Guide {
    let mut g = Guide {
        bundled,
        invoke_running: port != 0,
        port,
        installed: cli,
        ..Default::default()
    };
    if let Some(s) = status {
        g.signed_in = s.running && s.login.is_some();
        g.login = s.login.clone();
        g.phones = s.phones;
        g.phones_online = s.phones_online;
        g.phone_list = s.phone_list.clone();
    }
    if port != 0 {
        g.url = serve.and_then(|v| serve_url(v, port));
        g.link_on = g.url.is_some();
    }
    g
}

/// The guide for this Mac right now (runs `tailscale`, so not on a UI thread).
pub fn guide_now() -> Guide {
    let bundled = runtime_dir().is_some_and(|d| has_runtime(&d));
    let port = RUNNING_PORT.load(Ordering::SeqCst);
    let Some(cli) = find_tailscale() else {
        return guide_from(bundled, port, false, None, None);
    };
    let status = status_json(&cli).map(|v| parse_status(&v));
    let serve = {
        let (_, out) = run_capture(&cli, &["serve", "status", "--json"], Duration::from_secs(8));
        serde_json::from_str::<Value>(&out).ok()
    };
    let mut g = guide_from(bundled, port, true, status.as_ref(), serve.as_ref());
    g.subscribed = subscriptions_now();
    g
}

/// How many entries are in Invoke's `subs.json` (a list of push
/// subscriptions), 0 when it is missing or unreadable.
pub fn count_subscriptions(text: &str) -> u32 {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.as_array().map(|a| a.len() as u32))
        .unwrap_or(0)
}

/// Invoke keeps its state in `INVOKE_STATE`, else `~/.local/state/seance-chat`.
fn subscriptions_now() -> u32 {
    let dir = std::env::var("INVOKE_STATE")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local/state/seance-chat"))
        });
    dir.and_then(|d| std::fs::read_to_string(d.join("subs.json")).ok())
        .map_or(0, |t| count_subscriptions(&t))
}

/// What turning the phone link on or off came to.
#[derive(Debug, PartialEq, Eq)]
pub enum ServeOutcome {
    On(String),
    Off,
    /// Serve/HTTPS has to be switched on once for this tailnet: open this page.
    NeedsEnable(String),
    Failed(String),
}

/// The first https URL in `text` (tailscale prints the page to enable Serve).
pub fn first_url(text: &str) -> Option<String> {
    let start = text.find("https://")?;
    let url: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    Some(url)
}

/// Classify what `tailscale serve …` printed: a command that waits for a
/// click prints the enable page and is killed, so success is the exit code.
pub fn classify_serve(on: bool, ok: bool, out: &str, url: Option<String>) -> ServeOutcome {
    if ok {
        return if on {
            ServeOutcome::On(url.unwrap_or_default())
        } else {
            ServeOutcome::Off
        };
    }
    let lower = out.to_lowercase();
    if lower.contains("serve is not enabled") || lower.contains("login.tailscale.com/f/serve") {
        if let Some(u) = first_url(out) {
            return ServeOutcome::NeedsEnable(u);
        }
    }
    ServeOutcome::Failed(
        out.trim()
            .lines()
            .last()
            .unwrap_or("tailscale failed")
            .to_string(),
    )
}

/// Turn the phone link on (`tailscale serve` for Invoke's port) or off.
pub fn serve(on: bool) -> ServeOutcome {
    let Some(cli) = find_tailscale() else {
        return ServeOutcome::Failed("Tailscale is not installed on this Mac".into());
    };
    let port = RUNNING_PORT.load(Ordering::SeqCst);
    if on && port == 0 {
        return ServeOutcome::Failed("Invoke is not running".into());
    }
    let https = format!("--https={HTTPS_PORT}");
    let backend = format!("http://127.0.0.1:{port}");
    let (ok, out) = if on {
        run_capture(
            &cli,
            &["serve", "--bg", &https, &backend],
            Duration::from_secs(12),
        )
    } else {
        run_capture(&cli, &["serve", &https, "off"], Duration::from_secs(12))
    };
    let url = on.then(|| {
        status_json(&cli)
            .map(|v| parse_status(&v))
            .and_then(|s| s.dns)
            .map(|d| format!("https://{d}:{HTTPS_PORT}"))
            .unwrap_or_default()
    });
    let outcome = classify_serve(on, ok, &out, url);
    if on {
        remember_link(matches!(outcome, ServeOutcome::On(_)));
    } else if outcome == ServeOutcome::Off {
        remember_link(false);
    }
    outcome
}

// ---- running Invoke --------------------------------------------------------

fn support_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support/Seance"))
}

/// Where this version's Invoke and its Python live.
fn runtime_dir() -> Option<PathBuf> {
    Some(
        support_dir()?
            .join("invoke")
            .join(env!("CARGO_PKG_VERSION")),
    )
}

fn has_runtime(dir: &Path) -> bool {
    dir.join("python/bin/python3").is_file() && dir.join("seance-chat").is_file()
}

/// Copy the Invoke the app bundle carries (`Contents/Resources/invoke`) into
/// this version's folder under Application Support, once. The app itself is
/// never what runs, so Finder can replace it (the same reason the daemon runs
/// from a copy). Best effort: no bundled Invoke, or a failed copy, just means
/// no phone support.
pub fn stage_from_bundle(exe: &Path) {
    let Some(resources) = exe
        .parent()
        .and_then(Path::parent)
        .map(|c| c.join("Resources/invoke"))
        .filter(|r| r.is_dir())
    else {
        return;
    };
    let Some(dest) = runtime_dir() else { return };
    let done = dest.join(".complete");
    if done.is_file() {
        return;
    }
    let tmp = dest.with_extension("partial");
    let _ = std::fs::remove_dir_all(&tmp);
    if let Some(parent) = tmp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let copied = Command::new("cp")
        .arg("-R")
        .arg(&resources)
        .arg(&tmp)
        .status()
        .is_ok_and(|s| s.success());
    if !copied {
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }
    // The app was opened by the user; what it unpacks should not be held for a
    // second security check of its own.
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&tmp)
        .output();
    let _ = std::fs::write(tmp.join(".complete"), b"1\n");
    let _ = std::fs::remove_dir_all(&dest);
    if std::fs::rename(&tmp, &dest).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }
    prune_old(dest.parent());
}

/// Keep this version's folder and the newest other one.
fn prune_old(root: Option<&Path>) {
    let Some(root) = root else { return };
    let current = env!("CARGO_PKG_VERSION");
    let names: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| crate::update::parse_version(n).is_some())
        .collect();
    for old in prune_versions(&names, current) {
        let _ = std::fs::remove_dir_all(root.join(old));
    }
}

/// Which version folders to remove: all but `current` and the newest other.
pub fn prune_versions(names: &[String], current: &str) -> Vec<String> {
    let mut others: Vec<(Vec<u64>, &String)> = names
        .iter()
        .filter(|n| n.as_str() != current)
        .filter_map(|n| Some((crate::update::parse_version(n)?, n)))
        .collect();
    others.sort_by(|a, b| b.0.cmp(&a.0));
    others.into_iter().skip(1).map(|(_, n)| n.clone()).collect()
}

fn record_path() -> Option<PathBuf> {
    Some(support_dir()?.join("invoke.json"))
}

fn read_record() -> Value {
    record_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_record(v: &Value) {
    if let Some(p) = record_path() {
        let _ = std::fs::write(p, v.to_string());
    }
}

/// Remember whether the phone link was turned on, so a new port after an
/// update is pointed at again.
fn remember_link(on: bool) {
    let mut rec = read_record();
    rec["link"] = json!(on);
    write_record(&rec);
}

/// An Invoke we started earlier and that outlived its daemon (an upgrade
/// handoff): stop it, so the port is free and there is one of it.
fn stop_stale(rec: &Value) {
    let Some(pid) = rec["pid"].as_i64().filter(|p| *p > 1) else {
        return;
    };
    let cmd = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    if cmd.contains("seance-chat") {
        // SAFETY: plain signal to a pid we just confirmed is our Invoke.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        std::thread::sleep(Duration::from_millis(600));
    }
}

/// Whether a `ps` command line is our Invoke.
fn is_invoke_command(cmd: &str) -> bool {
    cmd.contains("seance-chat")
}

/// Stop the Invoke this app runs so the supervisor starts it again (it reads
/// its config at start). False when none of ours is running.
pub fn restart_running() -> bool {
    let rec = read_record();
    let Some(pid) = rec["pid"].as_i64().filter(|p| *p > 1) else {
        return false;
    };
    let cmd = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    if !is_invoke_command(&cmd) {
        return false;
    }
    // SAFETY: plain signal to a pid just confirmed to be our Invoke.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    true
}

fn spawn_invoke(dir: &Path, port: u16) -> Option<std::process::Child> {
    let support = support_dir()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(support.join("invoke.log"))
        .ok()?;
    let log2 = log.try_clone().ok()?;
    let cli = support.join("cli");
    let path = format!(
        "{}:{}",
        cli.display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into())
    );
    Command::new(dir.join("python/bin/python3"))
        .arg(dir.join("seance-chat"))
        .arg("run")
        .current_dir(dir)
        .env("INVOKE_MODE", "standalone")
        .env("INVOKE_AFK_DIR", crate::afk::dir())
        .env("INVOKE_PORT", port.to_string())
        .env("INVOKE_HTTPS_PORT", HTTPS_PORT.to_string())
        .env("INVOKE_PARENT_PID", std::process::id().to_string())
        .env("PATH", path)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONNOUSERSITE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log2))
        .spawn()
        .ok()
}

/// Keep Invoke running for as long as this daemon does. Called once at daemon
/// start; returns at once when this daemon should not run Invoke.
pub fn start_supervisor() {
    let has_runtime = runtime_dir().is_some_and(|d| has_runtime(&d));
    if !should_start(
        cfg!(target_os = "macos"),
        crate::term_config::personal(),
        crate::ACTIVE_TUNNEL.get().is_some(),
        has_runtime,
    ) {
        return;
    }
    let Some(dir) = runtime_dir() else { return };
    std::thread::Builder::new()
        .name("seance-invoke".into())
        .spawn(move || supervise(&dir))
        .ok();
}

fn supervise(dir: &Path) {
    stop_stale(&read_record());
    let mut backoff = Duration::from_secs(2);
    loop {
        let rec = read_record();
        let prev = rec["port"].as_u64().map(|p| p as u16);
        let Some(port) = pick_port(prev, port_is_free) else {
            std::thread::sleep(Duration::from_secs(30));
            continue;
        };
        let Some(mut child) = spawn_invoke(dir, port) else {
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(60));
            continue;
        };
        let started = Instant::now();
        let mut rec = read_record();
        rec["pid"] = json!(child.id());
        rec["port"] = json!(port);
        write_record(&rec);
        RUNNING_PORT.store(port, Ordering::SeqCst);
        // The phone link was on and the port moved: point it at the new one.
        if rec["link"] == true && prev != Some(port) {
            let _ = serve(true);
        }
        let _ = child.wait();
        RUNNING_PORT.store(0, Ordering::SeqCst);
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(2);
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests;
