//! Sleep / wake: a circle you're done with for now costs no RAM.
//!
//! A sleeping pane has **no process** — the PTY is gone and so is everything
//! the agent was holding (a claude pane and its MCP children run ~350MB; forty
//! of them is most of a workstation). What survives is everything that makes
//! the pane *itself*: slug, name, circle, cwd, command, its claude conversation
//! id, its scratchpad, its task and status — plus the **last frame it rendered**,
//! frozen to disk and served in place of a live snapshot so the circle still
//! reads after the process is gone.
//!
//! Waking is the restore path that already exists: the agent's resume of the
//! pane's own conversation id, in the same cwd (`agent_session.rs`). That is why sleep is only offered for panes that can be put
//! back exactly — see [`Engine::pane_restorable`]. A shell pane can't (its cwd
//! drift, its history, its running children are not reconstructible), so one
//! shell vetoes its whole circle.
//!
//! The deliberate crash. Sleeping does to a pane exactly what the 2026-08-04
//! OOM did to all of them — and the reason it is safe now is the same fix that
//! made that OOM survivable: the pane owns its conversation id.

use std::path::PathBuf;

use anyhow::{bail, Result};

use super::agent_session::{launch_with_session, transcript_exists, Agent};
use super::helpers::{atomic_write_bytes, now_ms};
use super::Engine;
use crate::events;
use crate::runtime::snapshot::{decode_grid_bin, encode_grid_bin, GridSnapshot};

/// Idle before the daemon sleeps a restorable pane, unless terminal.conf's
/// `seance-auto-sleep-idle` says otherwise.
pub const AUTO_SLEEP_IDLE_MS: u64 = 12 * 60 * 60 * 1000;

/// `<state-dir>/frozen/<slug>.scg` — the last frame of a sleeping pane.
fn frozen_dir() -> PathBuf {
    match crate::state::state_dir() {
        Ok(dir) => dir.join("frozen"),
        Err(_) => PathBuf::from(shellexpand::tilde("~/.local/share/seance/frozen").into_owned()),
    }
}

fn frozen_path(slug: &str) -> PathBuf {
    frozen_dir().join(format!("{slug}.scg"))
}

/// `<state-dir>/hosted/<pane_id>.json`: the Claude conversation running in a
/// pane Seance didn't start as an agent (`claude` typed into a shell, or a
/// wrapper like `lead-resume`). A SessionStart hook writes
/// `{"session": "<id>", "cwd": "<dir>"}` on every start, resume and clear.
pub(super) fn hosted_path(pane_id: &str) -> Option<PathBuf> {
    let dir = crate::state::state_dir().ok()?.join("hosted");
    Some(dir.join(format!("{pane_id}.json")))
}

/// That record, when the file holds one.
fn hosted_record(pane_id: &str) -> Option<(String, String)> {
    let text = std::fs::read_to_string(hosted_path(pane_id)?).ok()?;
    parse_hosted(&text)
}

pub(crate) fn parse_hosted(text: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let session = v.get("session")?.as_str()?.trim().to_string();
    let cwd = v.get("cwd")?.as_str()?.trim().to_string();
    (!session.is_empty() && !cwd.is_empty()).then_some((session, cwd))
}

/// What the processes under a pane say about the Claude in it: each claude
/// process's session id and status (from Claude's own
/// `~/.claude/sessions/<pid>.json`), and whether a Bash tool command is
/// running (a `shell-snapshots` shell under it). Empty off Linux.
#[derive(Default)]
pub(crate) struct ClaudeProcs {
    pub sessions: Vec<(String, String)>,
    pub tool_running: bool,
}

pub(crate) fn claude_procs(root: u32) -> ClaudeProcs {
    let mut out = ClaudeProcs::default();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    // pid → parent, for every process, to walk down from the pane's shell.
    let mut parent = std::collections::HashMap::new();
    for e in dir.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        // The command name can hold spaces and parens: fields follow the last ')'.
        if let Some(ppid) = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|p| p.parse::<u32>().ok())
        {
            parent.insert(pid, ppid);
        }
    }
    let under = |mut pid: u32| {
        for _ in 0..64 {
            if pid == root {
                return true;
            }
            match parent.get(&pid) {
                Some(&p) if p > 1 => pid = p,
                _ => return false,
            }
        }
        false
    };
    let sessions_dir = PathBuf::from(shellexpand::tilde("~/.claude/sessions").into_owned());
    for &pid in parent.keys() {
        if !under(pid) {
            continue;
        }
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if comm.trim() == "claude" {
            if let Some((id, status)) =
                std::fs::read_to_string(sessions_dir.join(format!("{pid}.json")))
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .and_then(|v| {
                        Some((
                            v.get("sessionId")?.as_str()?.to_string(),
                            v.get("status")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string(),
                        ))
                    })
            {
                out.sessions.push((id, status));
            }
        } else if std::fs::read(format!("/proc/{pid}/cmdline"))
            .is_ok_and(|c| String::from_utf8_lossy(&c).contains("shell-snapshots"))
        {
            out.tool_running = true;
        }
    }
    out
}

/// A Claude title: its idle ✳ or a working spinner. A pane whose title is
/// back to the shell's (`user@host:~`) left Claude, so its record is stale.
pub(crate) fn title_is_claude(title: Option<&str>) -> bool {
    title.is_some_and(|t| {
        t.trim_start().starts_with('\u{2733}') || seance_core::util::title_looks_busy(t)
    })
}

/// Two folder names for the same place (`~` against the full path).
fn same_dir(a: &str, b: &str) -> bool {
    let full = |s: &str| {
        let p = PathBuf::from(shellexpand::tilde(s).into_owned());
        p.canonicalize().unwrap_or(p)
    };
    full(a) == full(b)
}

/// What a hosted pane wakes into: Claude resumed in its own folder, with a
/// login shell back underneath once it exits. The shell is a fresh one: its
/// old variables, jobs and history went with the sleep.
pub(crate) fn hosted_launch(session: &str, cwd: &str) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    format!(
        "bash -c {} {} {}",
        q("cd -- \"$0\" && claude --resume \"$1\"; exec bash -l"),
        q(cwd),
        q(session)
    )
}

/// Auto-sleep's knobs, from the daemon machine's terminal.conf:
/// `seance-auto-sleep-idle` (default [`AUTO_SLEEP_IDLE_MS`]) and
/// `seance-auto-sleep-skip` (circles never auto-slept).
pub fn auto_sleep_idle_ms() -> u64 {
    crate::term_config::get()
        .seance_auto_sleep_idle_ms
        .unwrap_or(AUTO_SLEEP_IDLE_MS)
}

impl Engine {
    /// Can this pane be put back exactly as it is?
    ///
    /// * file pane — yes, it's a path and a viewer, there is no process.
    /// * claude / grok / codex pane with a session id **and its conversation
    ///   on disk** — yes, a resume lands on the same conversation.
    /// * Claude typed into a shell or started by a wrapper (`lead-resume`) —
    ///   yes while it is still running there and its session record
    ///   (`hosted/<pane_id>.json`) names that live conversation.
    /// * anything else (a bare shell, another agent, an agent pane that was never
    ///   prompted) — no. Sleeping it would throw away state nothing can
    ///   rebuild, so it isn't offered.
    pub fn pane_restorable(&self, slug: &str) -> bool {
        let Some(p) = self.panes.iter().find(|p| p.slug == slug) else {
            return false;
        };
        if p.kind == "file" {
            return true;
        }
        let Some(agent) = Agent::of(&p.command) else {
            // Claude hosted in a shell or wrapper: restorable while it is
            // still the thing running and its conversation is on disk.
            // Cheap: this runs for every pane on every state push. Sleeping
            // checks the live process too (`hosted_live`). A pane already
            // asleep was checked when it went to sleep.
            if p.asleep {
                return hosted_record(&p.pane_id).is_some();
            }
            let title = p.session.as_ref().and_then(|s| s.title());
            return title_is_claude(title.as_deref())
                && hosted_record(&p.pane_id)
                    .is_some_and(|(id, cwd)| transcript_exists(Agent::Claude, &cwd, &id));
        };
        p.agent_session
            .as_ref()
            .is_some_and(|id| transcript_exists(agent, &p.cwd, id))
    }

    /// Every pane in the circle is restorable (and there is at least one).
    /// This is the gate on the manual verb; the automatic sweep checks each
    /// pane on its own with [`Self::pane_restorable`].
    pub fn workspace_restorable(&self, workspace: &str) -> bool {
        let mut any = false;
        for p in self.panes.iter().filter(|p| p.workspace == workspace) {
            any = true;
            if !self.pane_restorable(&p.slug) {
                return false;
            }
        }
        any
    }

    /// The panes that block a circle from sleeping, for a message worth reading.
    pub fn workspace_sleep_blockers(&self, workspace: &str) -> Vec<String> {
        self.panes
            .iter()
            .filter(|p| p.workspace == workspace && !self.pane_restorable(&p.slug))
            .map(|p| p.slug.clone())
            .collect()
    }

    /// Freeze the pane's current frame, kill its process, keep the pane.
    ///
    /// Idempotent: sleeping a sleeping pane is a no-op, not an error, so a
    /// sweep and a right-click can race harmlessly.
    pub fn sleep_pane(&mut self, slug: &str) -> Result<bool> {
        let Some(idx) = self.panes.iter().position(|p| p.slug == slug) else {
            bail!("no pane '{slug}'");
        };
        if self.panes[idx].asleep {
            return Ok(false);
        }
        if !self.pane_restorable(slug) || !self.hosted_live(&self.panes[idx]) {
            bail!("pane '{slug}' can't be restored, so it won't be slept");
        }

        // Freeze BEFORE the process dies — after shutdown the grid is gone.
        if let Some(snap) = self.snapshot_pane(slug) {
            self.freeze_grid(slug, snap);
        }
        let pane = &mut self.panes[idx];
        pane.asleep = true;
        // `asleep` is set first on purpose: shutting the PTY down fires
        // `SessionEvent::Exited`, whose handler auto-closes panes. That guard
        // reads this flag, and once the pane wakes, the session generation.
        if let Some(s) = pane.session.take() {
            s.shutdown();
        }
        self.pane_busy.remove(slug);
        self.broadcast(crate::runtime::protocol::GuiEvent::PaneBusy {
            pane: slug.to_string(),
            busy: false,
        });
        events::log("daemon", None, Some(slug), "pane_slept", "slept".into());
        Ok(true)
    }

    /// Relaunch a sleeping pane onto the conversation it owns.
    pub fn wake_pane(&mut self, slug: &str) -> Result<bool> {
        let Some(idx) = self.panes.iter().position(|p| p.slug == slug) else {
            bail!("no pane '{slug}'");
        };
        if !self.panes[idx].asleep {
            return Ok(false);
        }
        let (kind, cwd, command, workspace, session_id, pane_id) = {
            let p = &self.panes[idx];
            (
                p.kind.clone(),
                p.cwd.clone(),
                p.command.clone(),
                p.workspace.clone(),
                p.agent_session.clone(),
                p.pane_id.clone(),
            )
        };

        // What comes back is a different screen from the frozen one every
        // window is still holding, so nobody may be sent damage against it:
        // drop the daemon's damage base and every connection's claim to one,
        // and the first frame after waking is a whole grid for all of them —
        // not just for the window that happened to click awaken.
        self.last_grid_cells.remove(slug);
        self.invalidate_bases(slug);

        if kind == "file" {
            self.panes[idx].asleep = false;
            self.thaw_grid(slug);
            events::log("daemon", None, Some(slug), "pane_woke", "woke".into());
            return Ok(true);
        }

        // Resume only when the conversation is on disk: a missing one makes
        // the agent exit non-zero, which would close the pane we are bringing back.
        // Claude hosted in a shell or wrapper comes back from its record:
        // nothing can rewrite it while the pane sleeps, so it still names the
        // conversation the pane went to sleep on.
        // A Claude agent pane does the same after a /clear, which moves it to
        // a new conversation its spawn-time session id doesn't know.
        let agent = Agent::of(&command);
        let record = hosted_record(&pane_id)
            .filter(|(id, rec_cwd)| transcript_exists(Agent::Claude, rec_cwd, id));
        let launch = match (agent, record, session_id.as_deref()) {
            (None, Some((id, rec_cwd)), _) => hosted_launch(&id, &rec_cwd),
            (Some(Agent::Claude), Some((id, rec_cwd)), _) if same_dir(&rec_cwd, &cwd) => {
                // Keep it, so a cold restart resumes this one too.
                self.panes[idx].agent_session = Some(id.clone());
                launch_with_session(&command, &cwd, &id)
            }
            (_, _, Some(id)) => launch_with_session(&command, &cwd, id),
            _ => command.clone(),
        };
        let session =
            self.spawn_terminal_session(slug, &launch, &cwd, &workspace, &self.panes[idx].pane_id)?;
        // A fresh session counts frames from 1, but clients drop anything that
        // isn't newer than the last frame they painted — and while this pane
        // slept, the only frame served for it was the frozen one, at the rev
        // the old session died on. Without carrying that count forward the
        // woken pane keeps painting its pre-sleep screen forever while the live
        // one scrolls past underneath, and only local echo (which leaves rev
        // alone) puts a character or two on the glass.
        session.seed_rev(self.frozen_grid(slug).map(|s| s.rev).unwrap_or(0));
        // Waking is activity: the sweep must not put it straight back.
        let now = now_ms();
        self.pane_busy_at.insert(slug.to_string(), now);
        self.pane_woke_at.insert(slug.to_string(), now);
        let pane = &mut self.panes[idx];
        pane.session = Some(session);
        pane.asleep = false;
        self.thaw_grid(slug);
        events::log("daemon", None, Some(slug), "pane_woke", "woke".into());
        Ok(true)
    }

    /// Sleep every pane in the circle. All-or-nothing: a circle holding one
    /// unrestorable pane stays awake, and says which one.
    pub fn sleep_workspace(&mut self, workspace: &str) -> Result<usize> {
        if !self.workspace_restorable(workspace) {
            let blockers = self.workspace_sleep_blockers(workspace);
            if blockers.is_empty() {
                bail!("circle '{workspace}' has no panes to sleep");
            }
            bail!(
                "circle '{workspace}' can't sleep — not restorable: {}",
                blockers.join(", ")
            );
        }
        let slugs: Vec<String> = self
            .panes
            .iter()
            .filter(|p| p.workspace == workspace)
            .map(|p| p.slug.clone())
            .collect();
        let mut n = 0;
        for slug in slugs {
            if self.sleep_pane(&slug)? {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Wake every sleeping pane in the circle.
    pub fn wake_workspace(&mut self, workspace: &str) -> Result<usize> {
        let slugs: Vec<String> = self
            .panes
            .iter()
            .filter(|p| p.workspace == workspace && p.asleep)
            .map(|p| p.slug.clone())
            .collect();
        let mut n = 0;
        for slug in slugs {
            if self.wake_pane(&slug)? {
                n += 1;
            }
        }
        Ok(n)
    }

    /// True while any pane of the circle is asleep.
    #[cfg(test)]
    pub fn workspace_asleep(&self, workspace: &str) -> bool {
        self.panes
            .iter()
            .any(|p| p.workspace == workspace && p.asleep)
    }

    /// Panes idle longer than `idle_ms` that can be put back exactly.
    ///
    /// Per pane, not per circle: a circle's Mac panes and shells can't sleep,
    /// and they shouldn't keep its idle Claude panes awake. Idle is the later
    /// of the circle's last human input and the pane's last busy change (the
    /// daemon's start when it saw none); screen output doesn't count, since a
    /// Claude status line redraws all the time. Never: a busy pane, one
    /// waiting on the human (needs-human, blocked, risky, an open ask), the
    /// focused pane, or a circle in `skip`. Queued tasks don't keep a pane
    /// awake: an idle one gets them within seconds, and the pump wakes a
    /// sleeping one to deliver.
    pub fn auto_sleep_candidates(&self, idle_ms: u64, now: u64, skip: &[String]) -> Vec<String> {
        self.panes
            .iter()
            .filter(|p| {
                if p.asleep || p.kind == "file" || skip.contains(&p.workspace) {
                    return false;
                }
                let slug = p.slug.as_str();
                let waiting =
                    self.statuses.get(slug).is_some_and(|s| {
                        matches!(s.0.as_str(), "needs-human" | "blocked" | "risky")
                    }) || self
                        .asks
                        .iter()
                        .any(|a| a.answer.is_none() && a.from == slug);
                if self.pane_busy.contains(slug)
                    || waiting
                    || self.focused_pane.as_deref() == Some(slug)
                {
                    return false;
                }
                let touch = self
                    .workspace_touch_ms
                    .get(&p.workspace)
                    .copied()
                    .unwrap_or(0);
                let busy = self
                    .pane_busy_at
                    .get(slug)
                    .copied()
                    .unwrap_or(self.started_ms);
                now.saturating_sub(touch.max(busy)) >= idle_ms
                    && self.pane_quiet(p)
                    && self.pane_restorable(slug)
            })
            .map(|p| p.slug.clone())
            .collect()
    }

    /// For Claude hosted in a shell or wrapper: the session record names the
    /// conversation running there now. A pane that left that Claude (exited,
    /// or went on to ssh into another) would otherwise wake into an old one.
    /// True for every other pane. Walks /proc, so only sleeping calls it.
    fn hosted_live(&self, p: &super::EnginePane) -> bool {
        if Agent::of(&p.command).is_some() || p.kind == "file" {
            return true;
        }
        let Some((id, _)) = hosted_record(&p.pane_id) else {
            return false;
        };
        p.session
            .as_ref()
            .and_then(|s| s.child_pid())
            .is_some_and(|pid| claude_procs(pid).sessions.iter().any(|(s, _)| *s == id))
    }

    /// Nothing is happening in the pane right now, beyond its title: the
    /// screen reads idle with nothing typed and waiting, and no Claude under it
    /// is busy or running a Bash command (a background job or a Monitor loop
    /// keeps the title idle). A stub pane with no process (tests) is quiet.
    fn pane_quiet(&self, p: &super::EnginePane) -> bool {
        let Some(s) = p.session.as_ref() else {
            return true;
        };
        let look = seance_core::agent_state::classify(&s.live_screen_text(), s.title().as_deref());
        if look.activity != seance_core::agent_state::Activity::Idle || look.queued_input {
            return false;
        }
        let procs = s.child_pid().map(claude_procs).unwrap_or_default();
        !procs.tool_running && procs.sessions.iter().all(|(_, status)| status == "idle")
    }

    /// One sweep. Returns the panes it put to sleep.
    pub fn auto_sleep_sweep(&mut self, idle_ms: u64, skip: &[String]) -> Vec<String> {
        let mut slept = Vec::new();
        for slug in self.auto_sleep_candidates(idle_ms, now_ms(), skip) {
            if let Ok(true) = self.sleep_pane(&slug) {
                let ws = self
                    .panes
                    .iter()
                    .find(|p| p.slug == slug)
                    .map(|p| p.workspace.clone());
                events::log(
                    "daemon",
                    ws.as_deref(),
                    Some(&slug),
                    "pane_auto_slept",
                    format!("slept after {}h idle", idle_ms / 3_600_000),
                );
                slept.push(slug);
            }
        }
        slept
    }

    /// Keep a pane's last frame, in memory and on disk.
    fn freeze_grid(&mut self, slug: &str, snap: GridSnapshot) {
        if let Ok(bytes) = encode_grid_bin(&snap) {
            let dir = frozen_dir();
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = atomic_write_bytes(&frozen_path(slug), &bytes);
            }
        }
        self.frozen_grids.insert(slug.to_string(), snap);
    }

    /// Drop a frozen frame once the pane is live again.
    fn thaw_grid(&mut self, slug: &str) {
        self.frozen_grids.remove(slug);
        let _ = std::fs::remove_file(frozen_path(slug));
    }

    /// The frozen frame for a sleeping pane, if we have one.
    pub(super) fn frozen_grid(&self, slug: &str) -> Option<GridSnapshot> {
        self.frozen_grids.get(slug).cloned()
    }

    /// Recolor every frozen frame after a theme switch. Its cells hold
    /// resolved colors, so each one that was exactly an old palette color
    /// becomes the new one; anything else (a program's own truecolor) stays.
    /// Re-saved so a restart doesn't bring the old look back. Returns the
    /// panes recolored, to be re-sent.
    pub(super) fn retheme_frozen(
        &mut self,
        old: &seance_core::schemes::TermColors,
        new: &seance_core::schemes::TermColors,
    ) -> Vec<String> {
        let slugs: Vec<String> = self.frozen_grids.keys().cloned().collect();
        for slug in &slugs {
            let Some(mut snap) = self.frozen_grids.remove(slug) else {
                continue;
            };
            for cell in &mut snap.cells {
                cell.fg = retheme_color(cell.fg, (old.fg, new.fg), &old.ansi, &new.ansi);
                cell.bg = retheme_color(cell.bg, (old.bg, new.bg), &old.ansi, &new.ansi);
            }
            // Clients drop a frame that isn't newer than the one they have.
            snap.rev += 1;
            self.freeze_grid(slug, snap);
        }
        slugs
    }

    /// Load frozen frames off disk for panes that came back asleep (daemon
    /// restart or `seance upgrade`). A frame that won't decode is dropped: the
    /// pane still reads as asleep, just blank, which is honest.
    pub(super) fn load_frozen_grids(&mut self) {
        let slugs: Vec<String> = self
            .panes
            .iter()
            .filter(|p| p.asleep)
            .map(|p| p.slug.clone())
            .collect();
        for slug in slugs {
            if let Ok(bytes) = std::fs::read(frozen_path(&slug)) {
                if let Ok(mut snap) = decode_grid_bin(&bytes) {
                    snap.pane = slug.clone();
                    snap.running = false;
                    self.frozen_grids.insert(slug, snap);
                }
            }
        }
    }
}

/// One frozen cell color after a theme switch: an exact old palette color
/// becomes the new one, anything else (a program's own truecolor) stays.
/// Palettes reuse colors (Monokai Pro's black is its background, its white
/// its foreground), so the cell's own default slot is tried before ANSI.
fn retheme_color(c: u32, own: (u32, u32), old: &[u32; 16], new: &[u32; 16]) -> u32 {
    std::iter::once(own)
        .chain(old.iter().copied().zip(new.iter().copied()))
        .find(|(o, _)| *o == c)
        .map_or(c, |(_, n)| n)
}

#[cfg(test)]
mod retheme_tests {
    use super::retheme_color;

    #[test]
    fn own_default_wins_over_a_matching_ansi_slot() {
        // Old: black (0) == bg, white (7) == fg, as in Monokai Pro.
        let mut old = [0u32; 16];
        old[0] = 0x2d2a2e;
        old[7] = 0xfcfcfa;
        let mut new = [0u32; 16];
        new[0] = 0x111111;
        new[7] = 0x777777;
        let (bg, fg) = ((0x2d2a2e, 0x0a1018), (0xfcfcfa, 0xd8e6f0));
        assert_eq!(retheme_color(0x2d2a2e, bg, &old, &new), 0x0a1018);
        assert_eq!(retheme_color(0xfcfcfa, fg, &old, &new), 0xd8e6f0);
        // The same color as a foreground still maps through ANSI black.
        assert_eq!(retheme_color(0x2d2a2e, fg, &old, &new), 0x111111);
        // A program's own color is left alone.
        assert_eq!(retheme_color(0x123456, fg, &old, &new), 0x123456);
    }
}
