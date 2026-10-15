//! Slug-keyed state vs pane lifetime. Slugs are reused once a pane is gone
//! (`unique_slug` only avoids LIVE panes), so anything keyed by slug — the
//! active task, task/message history, pad revs, the scratchpad file and its
//! sidecars — must die with the pane, or the next pane to get that slug
//! inherits it. The pane's birth (`comms.born`) is the ownership line: a
//! record older than the current pane's birth belongs to a predecessor.

use super::helpers::now_ms;
use super::Engine;
use crate::events;

impl Engine {
    /// Birth of the pane currently holding `slug`; 0 for panes that predate
    /// the birth record (everything under that slug is treated as theirs).
    pub(super) fn born_ms(&self, slug: &str) -> u64 {
        self.comms.born.get(slug).copied().unwrap_or(0)
    }

    /// True when a record stamped `created_ms` under `slug` belongs to the
    /// pane holding that slug now, not a dead one that held it before.
    pub(super) fn owns_record(&self, slug: &str, created_ms: u64) -> bool {
        created_ms >= self.born_ms(slug)
    }

    /// Active task, else the newest task this pane (not a predecessor) got.
    pub(super) fn current_task_id(&self, slug: &str) -> Option<String> {
        self.active_tasks.get(slug).cloned().or_else(|| {
            self.tasks
                .values()
                .filter(|t| t.pane == slug && self.owns_record(slug, t.created_ms))
                .max_by_key(|t| t.created_ms)
                .map(|t| t.id.clone())
        })
    }

    /// Drop every slug-keyed piece of runtime state for a pane that is gone
    /// (`pane_id` known) or for a slug about to be handed to a new pane
    /// (`pane_id` None: leftovers from a death this code never saw). Task and
    /// message records stay in their stores — `owns_record` hides them from
    /// the next owner. Pad files move to `scratch/archive/`.
    pub(super) fn retire_slug_state(&mut self, slug: &str, pane_id: Option<&str>) {
        if let Some(tid) = self.active_tasks.remove(slug) {
            if let Some(t) = self.tasks.get_mut(&tid) {
                if t.status == "open" {
                    t.status = "orphaned".into();
                    t.finished_ms = Some(now_ms());
                    t.note = Some(format!("pane {slug} closed with this task open"));
                }
            }
        }
        // At death the send-queue pump cancels these; at birth it may not
        // have run yet, and would deliver them to the new owner.
        if pane_id.is_none() {
            for tid in self.queued_for(slug) {
                if let Some(t) = self.tasks.get_mut(&tid) {
                    t.status = "cancelled".into();
                    t.finished_ms = Some(now_ms());
                    t.note = Some("pane closed before delivery".into());
                }
            }
            self.send_queue.inflight.remove(slug);
            self.send_queue.idle_since.remove(slug);
        }
        for m in self.comms.messages.iter_mut() {
            if m.status == "pending" && m.to_pane == slug {
                m.status = "failed".into();
                m.note = Some(format!("pane {slug} closed before it was delivered"));
            }
        }
        self.comms.born.remove(slug);
        self.statuses.remove(slug);
        self.pad_revs.remove(slug);
        self.inject_baselines.remove(slug);
        self.asks.retain(|a| a.from != slug || a.answer.is_some());
        let key = match pane_id {
            Some(id) => id.to_string(),
            None => format!("orphan-{slug}-{}", now_ms()),
        };
        let moved = self.store.archive(slug, &key);
        if moved > 0 {
            events::log(
                "daemon",
                None,
                Some(slug),
                "pad_archived",
                format!("{moved} file(s) → scratch/archive/{key}.*"),
            );
        }
    }

    /// Load-time repair for state written before slug state was retired on
    /// close: an active task whose pane is gone, or that predates the live
    /// pane's birth, was inherited through a reused slug. Pad files are NOT
    /// touched here — a live pane's `$SEANCE_SCRATCHPAD` already points at
    /// its file, so only a fresh spawn may move one.
    pub(super) fn prune_inherited_tasks(&mut self) {
        let stale: Vec<(String, String)> = self
            .active_tasks
            .iter()
            .filter(|(slug, tid)| {
                let live = self.panes.iter().any(|p| &p.slug == *slug);
                let owned = self
                    .tasks
                    .get(*tid)
                    .is_some_and(|t| self.owns_record(slug, t.created_ms));
                !live || !owned
            })
            .map(|(s, t)| (s.clone(), t.clone()))
            .collect();
        let mut orphan: Vec<String> = stale
            .iter()
            .map(|(slug, tid)| {
                self.active_tasks.remove(slug);
                tid.clone()
            })
            .collect();
        orphan.extend(
            self.tasks
                .values()
                .filter(|t| {
                    t.status == "queued"
                        && (!self.panes.iter().any(|p| p.slug == t.pane)
                            || !self.owns_record(&t.pane, t.created_ms))
                })
                .map(|t| t.id.clone()),
        );
        for tid in orphan {
            if let Some(t) = self.tasks.get_mut(&tid) {
                t.finished_ms = Some(now_ms());
                match t.status.as_str() {
                    "open" => {
                        t.status = "orphaned".into();
                        t.note = Some(format!(
                            "pane {} that got this task is gone (slug reuse cleanup)",
                            t.pane
                        ));
                    }
                    "queued" => {
                        t.status = "cancelled".into();
                        t.note = Some("pane closed before delivery".into());
                    }
                    _ => {}
                }
            }
        }
    }
}
