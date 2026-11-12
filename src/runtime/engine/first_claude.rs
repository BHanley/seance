//! A circle's first pane on the standalone Mac build: Claude, as the lead.
//!
//! A fresh Mac has no lead anywhere (only `ctl lead` sets one) and Chat needs
//! someone to talk to. So a new circle opens its first pane running Claude, in
//! the home folder, and that pane is made the circle's lead, the same record
//! `ctl lead` writes. Seance starts it as a Claude agent pane (it mints the
//! session id), so it comes back natively after an update or restart. With no
//! Claude installed the pane is a plain shell and the first-run card does the
//! rest.

use std::path::Path;

use anyhow::Result;

use super::{Engine, SpawnSpec};
use crate::events;

/// What the first pane of a circle should be.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FirstPane {
    /// Claude, launched with this command (the installed binary).
    Claude(String),
    /// A plain shell: Claude isn't installed.
    Shell,
}

/// Claude when it is installed, else a shell. A path with a space in it can't
/// ride in a command word (the agent is recognized by the first word), so
/// that one is left to the pane's own PATH.
pub(crate) fn plan_first_pane(claude: Option<&Path>) -> FirstPane {
    match claude {
        None => FirstPane::Shell,
        Some(p) => {
            let s = p.to_string_lossy();
            if s.contains(char::is_whitespace) {
                FirstPane::Claude("claude".into())
            } else {
                FirstPane::Claude(s.into_owned())
            }
        }
    }
}

/// Which circle to start in when the caller named none: the one on show.
fn target_circle(asked: &str, selected: Option<&str>) -> String {
    if !asked.trim().is_empty() {
        asked.trim().to_string()
    } else {
        selected.unwrap_or("main").to_string()
    }
}

/// What "start Claude here" does in a circle.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StartPlan {
    /// There is already one: return it, spawn nothing. The flag is whether it
    /// is a Claude.
    Reuse(String, bool),
    Spawn,
}

/// A circle that already has its Claude lead is not given a second one, and
/// with no Claude installed a circle that already has a pane is not given
/// another shell: each click on a "Start Claude" button would otherwise open
/// one more terminal. `panes` are the circle's non-file panes, in order.
pub(crate) fn plan_start(lead: Option<&str>, claude_found: bool, panes: &[&str]) -> StartPlan {
    if let Some(l) = lead {
        return StartPlan::Reuse(l.to_string(), true);
    }
    match (claude_found, panes.first()) {
        (false, Some(first)) => StartPlan::Reuse((*first).to_string(), false),
        _ => StartPlan::Spawn,
    }
}

impl Engine {
    /// Open the circle's first pane (Claude when `claude` is found, else a
    /// shell) and, for Claude, record it as the circle's lead. Returns the pane
    /// and whether it is a Claude.
    pub(crate) fn start_first_pane(
        &mut self,
        circle: &str,
        claude: Option<&Path>,
    ) -> Result<(String, bool)> {
        let circle = target_circle(circle, self.selected_workspace.as_deref());
        let in_circle: Vec<String> = self
            .panes
            .iter()
            .filter(|p| p.workspace == circle && p.kind != "file")
            .map(|p| p.slug.clone())
            .collect();
        let refs: Vec<&str> = in_circle.iter().map(String::as_str).collect();
        let lead = self.local_lead(&circle);
        if let StartPlan::Reuse(slug, is_claude) =
            plan_start(lead.as_deref(), claude.is_some(), &refs)
        {
            return Ok((slug, is_claude));
        }
        let (name, command, is_claude) = match plan_first_pane(claude) {
            FirstPane::Claude(c) => ("claude", Some(c), true),
            FirstPane::Shell => ("term", None, false),
        };
        let slug = self.spawn(SpawnSpec {
            name: name.into(),
            cwd: Some("~".into()),
            command,
            workspace: Some(circle),
            tiled: true,
            resume: false,
            file: None,
        })?;
        if is_claude {
            let ws = self
                .panes
                .iter()
                .find(|p| p.slug == slug)
                .map(|p| p.workspace.clone())
                .unwrap_or_default();
            self.comms.leads.insert(ws.clone(), slug.clone());
            events::log("daemon", Some(&ws), Some(&slug), "lead_set", slug.clone());
        }
        self.persist();
        self.push_state_to_all();
        Ok((slug, is_claude))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_claude_here_never_adds_a_second_one() {
        use StartPlan::*;
        // A Claude lead is there: reuse it, however many times you click.
        assert_eq!(
            plan_start(Some("claude"), true, &["claude", "term"]),
            Reuse("claude".into(), true)
        );
        // No Claude installed, a pane already there: no extra shell.
        assert_eq!(
            plan_start(None, false, &["term"]),
            Reuse("term".into(), false)
        );
        // Nothing there yet: spawn (new circle, first click).
        assert_eq!(plan_start(None, true, &[]), Spawn);
        assert_eq!(plan_start(None, false, &[]), Spawn);
        // Claude installed, only a shell in the circle: add Claude beside it.
        assert_eq!(plan_start(None, true, &["term"]), Spawn);
    }

    #[test]
    fn claude_when_installed_else_a_shell() {
        assert_eq!(plan_first_pane(None), FirstPane::Shell);
        assert_eq!(
            plan_first_pane(Some(Path::new("/Users/x/.local/bin/claude"))),
            FirstPane::Claude("/Users/x/.local/bin/claude".into())
        );
        // A space can't ride in the command word the agent is known by.
        assert_eq!(
            plan_first_pane(Some(Path::new("/Users/a b/bin/claude"))),
            FirstPane::Claude("claude".into())
        );
    }

    #[test]
    fn no_circle_named_means_the_one_on_show() {
        assert_eq!(target_circle("", Some("circle-2")), "circle-2");
        assert_eq!(target_circle("  ", None), "main");
        assert_eq!(target_circle("lab", Some("circle-2")), "lab");
    }
}
