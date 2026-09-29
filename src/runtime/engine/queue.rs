//! Send queue: `ctl send --queue` — "run this next" without a babysitter.
//!
//! A queued send opens its task as `queued` and touches nothing: the pane's
//! running task stays open. [`Engine::pump_send_queue`] (daemon thread,
//! every [`PUMP_EVERY`]) delivers the oldest queued task once the pane has
//! read `idle` for [`IDLE_SETTLE_MS`], then confirms the paste landed with
//! the same judge `ctl send` uses (`seance_core::agent_state::judge`):
//! Enter re-pressed if it sits in the composer, one re-paste if nothing
//! happened, else the task is marked `failed` and the task it superseded is
//! reopened ([`Engine::fail_task`] — also the `task_fail` op, which `ctl`
//! calls when its own confirmation fails).
//!
//! Queued tasks are durable `TaskRecord`s, so they survive restart and
//! upgrade; the in-flight confirmation is not (after an upgrade a delivered
//! task simply stays `open`).

use std::collections::HashMap;
use std::time::Duration;

use seance_core::agent_state::{judge, Activity, Judgement, Look};

use super::helpers::{now_ms, write_task_sidecar};
use super::Engine;
use crate::events;
use crate::runtime::protocol::*;

pub const PUMP_EVERY: Duration = Duration::from_millis(500);
/// Idle must hold this long before a queued task goes in — an agent between
/// two tool calls reads idle for a frame or two.
pub const IDLE_SETTLE_MS: u64 = 1500;
const CONFIRM_WINDOW_MS: u64 = 15_000;
const STUCK_ENTER_MS: u64 = 1200;

/// What is being delivered: a queued task, or a message (comms.rs).
enum Payload {
    Task(String),
    Message(String),
}

/// A delivery waiting for confirmation.
pub struct Inflight {
    what: Payload,
    /// The pasted text (what the judge looks for on screen).
    text: String,
    before: Look,
    started_ms: u64,
    stuck_since: Option<u64>,
    resubmits: u32,
    attempts: u32,
}

/// Ephemeral pump state (not persisted).
#[derive(Default)]
pub struct SendQueueState {
    idle_since: HashMap<String, u64>,
    inflight: HashMap<String, Inflight>,
}

impl Engine {
    /// Cancel the pane's active open task (a new one is replacing it);
    /// returns its id so the replacement can record what it superseded.
    pub(super) fn supersede_active(&mut self, slug: &str) -> Option<String> {
        let old = self.active_tasks.remove(slug)?;
        let t = self.tasks.get_mut(&old)?;
        if t.status != "open" {
            return None;
        }
        t.status = "cancelled".into();
        t.finished_ms = Some(now_ms());
        Some(old)
    }

    /// Record a queued task; returns (id, 1-based position in the pane's queue).
    pub(super) fn queue_task(&mut self, slug: &str, body: &str, by: &str) -> (String, usize) {
        self.task_counter = self.task_counter.saturating_add(1);
        let id = format!("task-{}", self.task_counter);
        self.tasks.insert(
            id.clone(),
            TaskRecord {
                id: id.clone(),
                pane: slug.to_string(),
                body: body.to_string(),
                status: "queued".into(),
                created_ms: now_ms(),
                queued_by: Some(by.to_string()),
                ..Default::default()
            },
        );
        let position = self.queued_for(slug).len();
        (id, position)
    }

    /// The pane's queued task ids, oldest first.
    pub(super) fn queued_for(&self, slug: &str) -> Vec<String> {
        let mut q: Vec<&TaskRecord> = self
            .tasks
            .values()
            .filter(|t| t.pane == slug && t.status == "queued")
            .collect();
        q.sort_by_key(|t| (t.created_ms, t.id.len(), t.id.clone()));
        q.into_iter().map(|t| t.id.clone()).collect()
    }

    /// Turn queued `tid` into the pane's open task (baseline, supersede).
    pub(super) fn activate_queued(&mut self, slug: &str, tid: &str) -> String {
        let pad_bytes = self
            .panes
            .iter()
            .find(|p| p.slug == slug)
            .and_then(|p| std::fs::metadata(&p.scratch_path).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        let pad_rev = self.pad_revs.get(slug).copied().unwrap_or(0);
        self.inject_baselines
            .insert(slug.to_string(), (pad_rev, pad_bytes));
        let superseded = self.supersede_active(slug);
        if let Some(t) = self.tasks.get_mut(tid) {
            t.status = "open".into();
            t.inject_pad_rev = pad_rev;
            t.inject_pad_bytes = pad_bytes;
            t.supersedes = superseded;
            if let Some(p) = self.panes.iter().find(|p| p.slug == slug) {
                write_task_sidecar(&p.scratch_path, t);
            }
        }
        self.active_tasks.insert(slug.to_string(), tid.to_string());
        tid.to_string()
    }

    /// Mark `tid` failed. If it was the pane's active task and its injection
    /// cancelled another open task, reopen that one (and its pad baseline).
    /// Returns the reopened task id.
    pub(crate) fn fail_task(&mut self, tid: &str, reason: &str) -> Result<Option<String>, String> {
        let t = self
            .tasks
            .get_mut(tid)
            .ok_or_else(|| format!("no task '{tid}'"))?;
        if !matches!(t.status.as_str(), "open" | "queued") {
            return Err(format!(
                "task '{tid}' is {}; only open or queued tasks can fail",
                t.status
            ));
        }
        let was_open = t.status == "open";
        t.status = "failed".into();
        t.finished_ms = Some(now_ms());
        t.note = Some(reason.to_string());
        let slug = t.pane.clone();
        let supersedes = t.supersedes.clone();
        self.send_queue.inflight.remove(&slug);
        if !was_open || self.active_tasks.get(&slug).map(String::as_str) != Some(tid) {
            return Ok(None);
        }
        self.active_tasks.remove(&slug);
        let Some(old) = supersedes else {
            return Ok(None);
        };
        let Some(o) = self.tasks.get_mut(&old).filter(|o| o.status == "cancelled") else {
            return Ok(None);
        };
        o.status = "open".into();
        o.finished_ms = None;
        let baseline = (o.inject_pad_rev, o.inject_pad_bytes);
        let rec = o.clone();
        self.active_tasks.insert(slug.clone(), old.clone());
        self.inject_baselines.insert(slug.clone(), baseline);
        if let Some(p) = self.panes.iter().find(|p| p.slug == slug) {
            write_task_sidecar(&p.scratch_path, &rec);
        }
        Ok(Some(old))
    }

    /// One pump pass: deliver due queued tasks, advance in-flight
    /// confirmations. Returns true when anything changed (persist).
    pub fn pump_send_queue(&mut self, now: u64) -> bool {
        let mut changed = false;
        let panes_with_work: Vec<String> = {
            let mut v: Vec<String> = self
                .tasks
                .values()
                .filter(|t| t.status == "queued")
                .map(|t| t.pane.clone())
                .collect();
            v.extend(self.send_queue.inflight.keys().cloned());
            v.extend(self.panes_with_pending_messages());
            v.sort();
            v.dedup();
            v
        };
        for slug in panes_with_work {
            let Some(idx) = self.panes.iter().position(|p| p.slug == slug) else {
                // Pane is gone: its queue can never run.
                for tid in self.queued_for(&slug) {
                    if let Some(t) = self.tasks.get_mut(&tid) {
                        t.status = "cancelled".into();
                        t.finished_ms = Some(now);
                        t.note = Some("pane closed before delivery".into());
                    }
                }
                self.send_queue.inflight.remove(&slug);
                changed = true;
                continue;
            };
            let Some(look) = self.panes[idx]
                .session
                .as_ref()
                .filter(|s| s.is_running())
                .map(|s| Look::of(s.live_screen_text(), s.title().as_deref()))
            else {
                continue;
            };
            if self.send_queue.inflight.contains_key(&slug) {
                changed |= self.advance_inflight(idx, &slug, look, now);
            } else if !self.queued_for(&slug).is_empty() {
                changed |= self.maybe_deliver(idx, &slug, look, now);
            } else {
                changed |= self.maybe_deliver_message(idx, &slug, look, now);
            }
        }
        changed
    }

    fn maybe_deliver(&mut self, idx: usize, slug: &str, look: Look, now: u64) -> bool {
        let Some(tid) = self.queued_for(slug).into_iter().next() else {
            return false;
        };
        let ready = look.activity == Activity::Idle
            || (look.activity == Activity::Unknown && !self.active_tasks.contains_key(slug));
        if !ready {
            self.send_queue.idle_since.remove(slug);
            return false;
        }
        let since = *self
            .send_queue
            .idle_since
            .entry(slug.to_string())
            .or_insert(now);
        if now.saturating_sub(since) < IDLE_SETTLE_MS {
            return false;
        }
        let (act, body) = match self.tasks.get(&tid) {
            Some(t) => (
                t.queued_by.clone().unwrap_or_else(|| "cli".into()),
                t.body.clone(),
            ),
            None => return false,
        };
        // A human at the keys wins; try again next pass.
        if self.panes[idx].agency.may_inject(&act, false).is_err() {
            return false;
        }
        self.send_queue.idle_since.remove(slug);
        self.inject_task(idx, body.clone(), true, &act, Some(tid.clone()));
        self.send_queue.inflight.insert(
            slug.to_string(),
            Inflight {
                what: Payload::Task(tid),
                text: body,
                before: look,
                started_ms: now,
                stuck_since: None,
                resubmits: 0,
                attempts: 1,
            },
        );
        true
    }

    /// Paste the pane's oldest pending message (ask / tell / reply /
    /// notice). Unlike a queued task it does not wait for idle: a busy agent
    /// takes it behind its running turn. Never opens or cancels a task.
    fn maybe_deliver_message(&mut self, idx: usize, slug: &str, look: Look, now: u64) -> bool {
        let Some(mid) = self.pending_message_for(slug) else {
            return false;
        };
        match self.message_can_land(idx, look.activity) {
            Some(true) => {}
            Some(false) => return false,
            None => {
                if let Some(m) = self.message_mut(&mid) {
                    m.status = "failed".into();
                    m.note = Some(format!(
                        "pane {slug} is not an agent session (nothing there to read it)"
                    ));
                }
                return true;
            }
        }
        let Some(m) = self.message(&mid).cloned() else {
            return false;
        };
        let act = m
            .from_pane
            .as_ref()
            .map(|p| format!("agent:{p}"))
            .unwrap_or_else(|| "cli".into());
        if self.panes[idx].agency.may_inject(&act, false).is_err() {
            return false;
        }
        let text = self.message_paste_text(&m);
        if let Some(session) = self.panes[idx].session.as_ref() {
            session.set_input_origin(&act);
            session.scroll_to_bottom();
            session.inject(text.clone(), true);
        }
        events::log(
            &act,
            Some(&self.panes[idx].workspace.clone()),
            Some(slug),
            "msg_paste",
            mid.clone(),
        );
        self.send_queue.inflight.insert(
            slug.to_string(),
            Inflight {
                what: Payload::Message(mid),
                text,
                before: look,
                started_ms: now,
                stuck_since: None,
                resubmits: 0,
                attempts: 1,
            },
        );
        true
    }

    fn finish_delivery(&mut self, slug: &str, via: &str) {
        let Some(f) = self.send_queue.inflight.remove(slug) else {
            return;
        };
        let id = match f.what {
            Payload::Task(tid) => {
                if let Some(t) = self.tasks.get_mut(&tid) {
                    t.delivery = Some(via.to_string());
                }
                tid
            }
            Payload::Message(mid) => {
                if let Some(m) = self.message_mut(&mid) {
                    if m.status == "pending" {
                        m.status = "delivered".into();
                    }
                    m.delivery = Some(via.to_string());
                }
                mid
            }
        };
        events::log(
            "daemon",
            None,
            Some(slug),
            "delivered",
            format!("{id} via {via}"),
        );
    }

    fn advance_inflight(&mut self, idx: usize, slug: &str, now_look: Look, now: u64) -> bool {
        let Some(f) = self.send_queue.inflight.get_mut(slug) else {
            return false;
        };
        let body = f.text.clone();
        let verdict = match judge(&f.before, &now_look, &body) {
            Judgement::Pending if f.resubmits > 0 => Judgement::Delivered("resubmit"),
            v => v,
        };
        let session = self.panes[idx].session.as_ref();
        match verdict {
            Judgement::Delivered(via) => {
                self.finish_delivery(slug, via);
                return true;
            }
            Judgement::Stuck => {
                let since = *f.stuck_since.get_or_insert(now);
                if now.saturating_sub(since) >= STUCK_ENTER_MS && f.resubmits < 2 {
                    if let Some(s) = session {
                        s.write_bytes(b"\r".to_vec());
                    }
                    f.resubmits += 1;
                    f.stuck_since = None;
                }
                return false;
            }
            Judgement::Pending => f.stuck_since = None,
        }
        if now.saturating_sub(f.started_ms) < CONFIRM_WINDOW_MS {
            return false;
        }
        // A busy agent takes pasted text mid-turn without echoing it;
        // re-pasting there only stacks copies (v2-simp-ios, 2026-09-28).
        if f.before.activity == Activity::Busy
            && !seance_core::agent_state::composer_holds(&now_look.screen, &body)
        {
            self.finish_delivery(slug, "mid-turn");
            return true;
        }
        if f.attempts < 2 {
            f.attempts += 1;
            f.started_ms = now;
            if let Some(s) = session {
                s.inject(body, true);
            }
            return false;
        }
        let Some(f) = self.send_queue.inflight.remove(slug) else {
            return true;
        };
        let id = match f.what {
            Payload::Task(tid) => {
                let _ = self.fail_task(&tid, "queued delivery not confirmed after 2 attempts");
                tid
            }
            Payload::Message(mid) => {
                if let Some(m) = self.message_mut(&mid) {
                    m.status = "failed".into();
                    m.note = Some("delivery not confirmed after 2 attempts".into());
                }
                mid
            }
        };
        events::log("daemon", None, Some(slug), "undelivered", id);
        true
    }
}
