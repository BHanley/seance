//! Whether a circle is working or waiting: one rule for every Chat.
//!
//! Invoke's circle list (`circle_working`, `circle_waiting` in `seance-chat`)
//! and this app's local Chat must agree to the pixel, so the rule lives here
//! once. Facts come from the daemon's pane rows (`activity`, `task_status`).

/// What the daemon says about one pane of a circle.
pub struct PaneFact<'a> {
    pub slug: &'a str,
    /// `busy`, `idle`, `awaiting-input`, ... (the pane row's `activity`).
    pub activity: &'a str,
    /// The pane's current or latest task: `open`, `done`, ... (`task_status`).
    pub task_status: Option<&'a str>,
}

/// Any pane of the circle is mid-turn.
pub fn working(panes: &[PaneFact]) -> bool {
    panes.iter().any(|p| p.activity == "busy")
}

/// No pane mid-turn, but the lead's task is still open: work it handed off or
/// started in the background is running. A lead stopped on a prompt
/// (`awaiting-input`) is asking for the human, not waiting.
pub fn waiting(lead: Option<&str>, panes: &[PaneFact]) -> bool {
    lead.is_some_and(|lead| {
        panes
            .iter()
            .any(|p| p.slug == lead && p.task_status == Some("open") && p.activity == "idle")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact<'a>(slug: &'a str, activity: &'a str, task: Option<&'a str>) -> PaneFact<'a> {
        PaneFact {
            slug,
            activity,
            task_status: task,
        }
    }

    /// Invoke's cases, row by row: (panes, lead, working, waiting).
    #[test]
    fn matches_invokes_rule() {
        let cases: Vec<(Vec<PaneFact>, Option<&str>, bool, bool, &str)> = vec![
            (
                vec![fact("lead", "idle", Some("open"))],
                Some("lead"),
                false,
                true,
                "open task, idle lead: waiting",
            ),
            (
                vec![fact("lead", "busy", Some("open"))],
                Some("lead"),
                true,
                false,
                "open task, busy lead: working, not waiting",
            ),
            (
                vec![fact("lead", "idle", None)],
                Some("lead"),
                false,
                false,
                "no task: neither",
            ),
            (
                vec![fact("lead", "idle", Some("done"))],
                Some("lead"),
                false,
                false,
                "finished task: neither",
            ),
            (
                vec![fact("lead", "awaiting-input", Some("open"))],
                Some("lead"),
                false,
                false,
                "stopped on a prompt: needs-you, neither",
            ),
            (
                vec![
                    fact("lead", "idle", Some("open")),
                    fact("helper", "busy", None),
                ],
                Some("lead"),
                true,
                true,
                "a helper is busy: working is any pane, waiting is the lead's task",
            ),
            (
                vec![fact("a", "idle", Some("open"))],
                Some("b"),
                false,
                false,
                "another pane's open task is not the lead's",
            ),
            (
                vec![fact("a", "idle", Some("open"))],
                None,
                false,
                false,
                "no lead: not waiting",
            ),
            (vec![], Some("lead"), false, false, "no panes"),
        ];
        for (panes, lead, w, wait, why) in cases {
            assert_eq!(working(&panes), w, "working: {why}");
            assert_eq!(waiting(lead, &panes), wait, "waiting: {why}");
        }
    }
}
