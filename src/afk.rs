//! AFK for the standalone Mac app: step away, keep working from your phone.
//!
//! One state file, `<state dir>/afk/on` (the epoch second it began), is the
//! switch. The Seance menu, Chat's button and Invoke's ··· toggle all write
//! or remove it; the daemon's watcher (`daemon/afkwatch.rs`) notices and does
//! the work: keep the Mac awake with the lid closed (`pmset -a disablesleep`),
//! tell the leads, and end AFK by itself when you come back or the battery
//! runs low. Everything that touches the system sits behind [`Sys`], so the
//! rules are tested with fake readings and no real sleep setting changes.
//!
//! Keeping a Mac awake with the lid closed needs root. The first time only,
//! an admin prompt installs a sudoers entry that allows exactly two commands
//! (`pmset -a disablesleep 1` and `... 0`) for this user, nothing broader.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Seconds without input that count as "you have left"; input after that
/// means you are back.
pub const AWAY_SECS: u64 = 60;
/// AFK ends under this battery percent when unplugged.
pub const LOW_BATT: u32 = 15;
/// How often the idle time and battery are read while AFK is on.
pub const POLL_SECS: u64 = 15;
pub const SUDOERS_FILE: &str = "/etc/sudoers.d/seance-afk";

/// What a lead is told when AFK turns on: one line.
pub const NOTE_FOR_LEADS: &str =
    "AFK is on: your human has left the Mac and reads Invoke on their phone. \
Answer in Chat so it reaches them there, not only in the terminal.";

/// The AFK folder: `SEANCE_AFK_DIR`, else `afk/` in the state folder.
pub fn dir() -> PathBuf {
    if let Ok(d) = std::env::var("SEANCE_AFK_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    crate::state::state_dir()
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("afk")
}

fn on_file(d: &Path) -> PathBuf {
    d.join("on")
}
/// Present while the Mac is being kept awake with the lid closed.
fn mac_file(d: &Path) -> PathBuf {
    d.join("mac-awake")
}
/// Present when WE turned sleep off, so we alone turn it back on.
fn we_set_file(d: &Path) -> PathBuf {
    d.join("we-set")
}
/// The last thing worth telling the person (why it ended, or a warning).
fn note_file(d: &Path) -> PathBuf {
    d.join("note")
}

/// What the buttons and Invoke show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub on: bool,
    pub since_ms: u64,
    pub mac_awake: bool,
    pub note: Option<String>,
}

pub fn status(d: &Path) -> Status {
    let since = std::fs::read_to_string(on_file(d))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok());
    Status {
        on: since.is_some(),
        since_ms: since.unwrap_or(0) * 1000,
        mac_awake: since.is_some() && mac_file(d).exists(),
        note: std::fs::read_to_string(note_file(d))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty()),
    }
}

pub fn status_json(d: &Path) -> serde_json::Value {
    let s = status(d);
    serde_json::json!({
        "on": s.on, "since_ms": s.since_ms, "mac_awake": s.mac_awake, "note": s.note,
    })
}

/// Flip the switch. Turning on clears the old note and the Mac-awake mark (the
/// watcher sets them again once it has done the work).
pub fn set_wanted(d: &Path, on: bool, now_secs: u64) {
    let _ = std::fs::create_dir_all(d);
    if on {
        let _ = std::fs::remove_file(note_file(d));
        let _ = std::fs::remove_file(mac_file(d));
        write_atomic(&on_file(d), &now_secs.to_string());
    } else {
        let _ = std::fs::remove_file(on_file(d));
    }
}

fn write_atomic(path: &Path, text: &str) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

// ---- the sudoers entry -----------------------------------------------------

/// A login name safe to put in a sudoers line.
pub fn valid_user(user: &str) -> bool {
    !user.is_empty()
        && user.len() <= 32
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        && !user.starts_with('-')
}

/// The whole file: this user, exactly these two commands, no password.
pub fn sudoers_text(user: &str) -> String {
    format!(
        "# Seance AFK: lets {user} keep the Mac awake with the lid closed, and nothing else.\n\
         {user} ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0\n"
    )
}

/// The root script behind the admin prompt: check the file first, and only a
/// file that passes goes into place, mode 0440, root:wheel.
pub fn admin_shell(tmp: &Path) -> String {
    let t = tmp.display();
    format!(
        "/usr/sbin/visudo -cf '{t}' && /usr/bin/install -m 0440 -o root -g wheel '{t}' {SUDOERS_FILE}"
    )
}

/// The AppleScript that runs `shell` with the macOS password box.
pub fn applescript(shell: &str) -> String {
    let esc = shell.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "do shell script \"{esc}\" with administrator privileges \
         with prompt \"Seance needs your Mac password once, to keep your Mac awake with the lid closed while AFK is on.\""
    )
}

// ---- readings ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub pct: Option<u32>,
    pub ac: bool,
}

/// `pmset -g batt`: on AC unless it says it draws from the battery; no
/// percentage on a desktop.
pub fn parse_batt(text: &str) -> Battery {
    let ac = !text.contains("Battery Power");
    let pct = text.split('%').next().and_then(|head| {
        let digits: String = head
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        digits.parse().ok()
    });
    Battery {
        pct: if text.contains('%') { pct } else { None },
        ac,
    }
}

/// Whole seconds from `ioreg -c IOHIDSystem` (HIDIdleTime is nanoseconds).
pub fn parse_idle(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.contains("HIDIdleTime"))?;
    let ns: u64 = line.rsplit('=').next()?.trim().parse().ok()?;
    Some(ns / 1_000_000_000)
}

/// Why AFK ended by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Back,
    LowBattery,
}

impl Reason {
    pub fn text(self) -> String {
        match self {
            Reason::Back => "AFK ended: you used the Mac's keyboard or trackpad.".into(),
            Reason::LowBattery => {
                format!(
                    "AFK ended: the battery is under {LOW_BATT}% and the Mac is not plugged in."
                )
            }
        }
    }
}

/// The auto-off rules. Back counts only after the Mac sat idle for a minute,
/// so turning AFK on at the keyboard does not end it at once.
#[derive(Default)]
pub struct AutoOff {
    away: bool,
}

impl AutoOff {
    pub fn step(&mut self, idle: Option<u64>, batt: Battery) -> Option<Reason> {
        if !batt.ac && batt.pct.is_some_and(|p| p < LOW_BATT) {
            return Some(Reason::LowBattery);
        }
        if let Some(i) = idle {
            if i >= AWAY_SECS {
                self.away = true;
            } else if self.away {
                return Some(Reason::Back);
            }
        }
        None
    }
}

// ---- the machine ------------------------------------------------------------

/// Everything that touches the Mac.
pub trait Sys {
    fn idle(&mut self) -> Option<u64>;
    fn battery(&mut self) -> Battery;
    /// `sudo -n pmset -a disablesleep 1|0`; true when it worked.
    fn set_sleep_disabled(&mut self, on: bool) -> bool;
    fn sleep_disabled(&mut self) -> bool;
    /// The sudoers entry is in place (asks once, with the password box, when it is not).
    fn ensure_sudoers(&mut self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// AFK just turned on: tell the leads.
    On,
    /// AFK ended, by itself for `Some(reason)`.
    Off(Option<Reason>),
}

pub struct Machine<S: Sys> {
    sys: S,
    dir: PathBuf,
    applied: bool,
    auto: AutoOff,
    last_poll: u64,
}

impl<S: Sys> Machine<S> {
    /// At launch. A state file left from before is AFK still on. A sleep
    /// setting WE changed and AFK no longer wants (the app quit or crashed
    /// while it was on) is put back.
    pub fn start(sys: S, dir: PathBuf) -> Self {
        let mut m = Machine {
            sys,
            dir,
            applied: false,
            auto: AutoOff::default(),
            last_poll: 0,
        };
        let wanted = status(&m.dir).on;
        m.applied = wanted;
        if !wanted && we_set_file(&m.dir).exists() {
            if m.sys.sleep_disabled() {
                m.sys.set_sleep_disabled(false);
            }
            let _ = std::fs::remove_file(we_set_file(&m.dir));
            let _ = std::fs::remove_file(mac_file(&m.dir));
        }
        m
    }

    /// One look at the switch (every couple of seconds). `now` is epoch seconds.
    pub fn tick(&mut self, now: u64) -> Vec<Event> {
        let wanted = status(&self.dir).on;
        let mut out = Vec::new();
        if wanted && !self.applied {
            self.applied = true;
            self.auto = AutoOff::default();
            self.last_poll = now;
            self.keep_awake();
            out.push(Event::On);
        } else if !wanted && self.applied {
            self.applied = false;
            self.let_sleep();
            out.push(Event::Off(None));
        } else if wanted && now.saturating_sub(self.last_poll) >= POLL_SECS {
            self.last_poll = now;
            let (idle, batt) = (self.sys.idle(), self.sys.battery());
            if let Some(why) = self.auto.step(idle, batt) {
                set_wanted(&self.dir, false, now);
                write_atomic(&note_file(&self.dir), &why.text());
                self.applied = false;
                self.let_sleep();
                out.push(Event::Off(Some(why)));
            }
        }
        out
    }

    fn keep_awake(&mut self) {
        if self.sys.ensure_sudoers() && self.sys.set_sleep_disabled(true) {
            write_atomic(&we_set_file(&self.dir), "1");
            write_atomic(&mac_file(&self.dir), "1");
        } else {
            write_atomic(
                &note_file(&self.dir),
                "AFK is on, but the Mac may sleep when you close the lid: the one-time password step was not completed.",
            );
        }
    }

    fn let_sleep(&mut self) {
        let _ = std::fs::remove_file(mac_file(&self.dir));
        if we_set_file(&self.dir).exists() {
            if self.sys.set_sleep_disabled(false) {
                let _ = std::fs::remove_file(we_set_file(&self.dir));
            } else {
                // Keep the mark: the next launch tries again.
                write_atomic(
                    &note_file(&self.dir),
                    "AFK is off, but the Mac could not be set to sleep again. It will try at the next launch.",
                );
            }
        }
    }
}

// ---- the real Mac -------------------------------------------------------------

pub struct RealSys;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Sys for RealSys {
    fn idle(&mut self) -> Option<u64> {
        parse_idle(&run("/usr/sbin/ioreg", &["-c", "IOHIDSystem"])?)
    }
    fn battery(&mut self) -> Battery {
        run("/usr/bin/pmset", &["-g", "batt"])
            .map(|t| parse_batt(&t))
            .unwrap_or(Battery {
                pct: None,
                ac: true,
            })
    }
    fn set_sleep_disabled(&mut self, on: bool) -> bool {
        Command::new("/usr/bin/sudo")
            .args([
                "-n",
                "/usr/bin/pmset",
                "-a",
                "disablesleep",
                if on { "1" } else { "0" },
            ])
            .status()
            .is_ok_and(|s| s.success())
    }
    fn sleep_disabled(&mut self) -> bool {
        run("/usr/bin/pmset", &["-g"]).is_some_and(|t| {
            t.lines()
                .any(|l| l.contains("SleepDisabled") && l.trim_end().ends_with('1'))
        })
    }
    fn ensure_sudoers(&mut self) -> bool {
        let allowed = || {
            Command::new("/usr/bin/sudo")
                .args(["-n", "-l", "/usr/bin/pmset", "-a", "disablesleep", "1"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if allowed() {
            return true;
        }
        let user = std::env::var("USER").unwrap_or_default();
        if !valid_user(&user) {
            return false;
        }
        let tmp = std::env::temp_dir().join(format!("seance-afk-sudoers-{}", std::process::id()));
        if std::fs::write(&tmp, sudoers_text(&user)).is_err() {
            return false;
        }
        // The one admin prompt. Cancel is an error here, and AFK goes on without it.
        let ok = Command::new("/usr/bin/osascript")
            .args(["-e", &applescript(&admin_shell(&tmp))])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        let _ = std::fs::remove_file(&tmp);
        ok && allowed()
    }
}

#[cfg(test)]
mod tests;
