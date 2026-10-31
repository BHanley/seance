//! A circle lead's tasks card: the checklist behind its "☑ N" chip.
//!
//! The data is one daemon call (`FsOp::LeadTasks`) polled for the selected
//! circle's lead; the card is drawn from the last answer, so rendering never
//! blocks on the bridge.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    anchored, deferred, div, point, prelude::*, px, Anchor, AnyElement, Context, Pixels, Point,
    SharedString, Window,
};
use serde::Deserialize;

use crate::events;
use crate::theme::SeancePalette;

use super::util::now_ms;
use super::SeanceApp;

/// How often the selected circle's lead is asked for its tasks.
const POLL_EVERY: Duration = Duration::from_millis(2000);
/// A row's full text stops here, like the pad inspector's task body.
const FULL_TEXT_CAP: usize = 2500;
/// After the daemon fails to answer, ask again this rarely.
const RETRY_OFF: Duration = Duration::from_secs(600);
/// Timeouts in a row before the poll goes quiet.
const TIMEOUTS_BEFORE_OFF: u8 = 3;
/// The popover card's width and its share of the window height.
const CARD_W: f32 = 420.;
const CARD_MAX_H: f32 = 0.6;
/// Phone tasks open with this line (Invoke); the card leaves it out.
const PHONE_HEADER: &str = "📱 From Brian's phone (Invoke):";

/// The open tasks card: whose, and where its chip was clicked.
pub(super) struct TasksPopover {
    pub(super) slug: String,
    pub(super) at: Point<Pixels>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub(super) struct LeadTasks {
    #[serde(default)]
    tasks: Vec<TaskRow>,
    #[serde(default)]
    helpers: Vec<Helper>,
    /// The lead's own deciding screen line while it works.
    #[serde(default)]
    evidence: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct TaskRow {
    id: String,
    status: String,
    #[serde(default)]
    created_ms: u64,
    #[serde(default)]
    finished_ms: Option<u64>,
    #[serde(default)]
    body: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Helper {
    #[serde(default)]
    name: String,
    #[serde(default)]
    evidence: Option<String>,
}

impl LeadTasks {
    /// What the chip counts: open and queued tasks. Parked ones stay out,
    /// since Seance rarely clears them.
    pub(super) fn open_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| matches!(t.status.as_str(), "open" | "queued"))
            .count()
    }

    fn with_status<'a>(&'a self, status: &'a str) -> impl Iterator<Item = &'a TaskRow> {
        self.tasks.iter().filter(move |t| t.status == status)
    }
}

/// A task's text as the lead reads it: the phone header and the
/// "---" footer a phone task carries are cut, the rest is trimmed.
fn clean_body(body: &str) -> String {
    let body = body.split("\n---\n").next().unwrap_or(body).trim();
    let body = body.strip_prefix(PHONE_HEADER).unwrap_or(body);
    body.trim().to_string()
}

fn first_line(body: &str) -> String {
    clean_body(body).lines().next().unwrap_or("").to_string()
}

/// Screen evidence minus its leading spinner glyph ("✻ Thinking…").
fn strip_spinner(line: &str) -> &str {
    let line = line.trim();
    let mut chars = line.chars();
    match chars.next() {
        Some(c) if !c.is_alphanumeric() && chars.as_str().starts_with(' ') => {
            chars.as_str().trim_start()
        }
        _ => line,
    }
}

/// HH:MM, with the date (MM-DD) in front of a time before today.
pub(super) fn clock(ms: u64) -> String {
    let time = &events::fmt_time(ms)[..5];
    if ms < events::local_day_start_ms(now_ms()) {
        format!("{} {time}", &events::local_date(ms)[5..])
    } else {
        time.to_string()
    }
}

fn span(ms: u64) -> String {
    let mins = ms / 60_000;
    match mins {
        0 => "under a min".into(),
        1..=59 => format!("{mins} min"),
        _ => format!("{}h {:02}m", mins / 60, mins % 60),
    }
}

impl SeanceApp {
    /// Poll the daemon for the selected circle's lead tasks.
    pub(super) fn start_lead_tasks_poll(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_EVERY).await;
            let Some(this) = this.upgrade() else { break };
            let ask = this.update(cx, |app: &mut SeanceApp, cx| {
                // The card goes when its lead, pane or layout does.
                if app.tasks_popover.is_some() && !app.tasks_popover_valid() {
                    app.close_tasks_popover(cx);
                }
                if app
                    .lead_tasks_off
                    .is_some_and(|at| at.elapsed() < RETRY_OFF)
                {
                    return None;
                }
                let ws = app.selected_workspace.clone()?;
                let lead = app.stage_lead(&ws)?;
                Some((lead, Arc::clone(&app.client)))
            });
            let Some((lead, client)) = ask else { continue };
            let got = cx
                .background_executor()
                .spawn({
                    let lead = lead.clone();
                    async move { client.lead_tasks(&lead) }
                })
                .await;
            let data = match got {
                Ok(data) => data,
                Err(e) => {
                    // A daemon that never answers costs a 10 s wait per ask.
                    // One tunnel blip is not that: it takes a run of timeouts
                    // to stop asking for a while. (An older daemon also says so
                    // outright, see `GuiEvent::Error` in mod.rs.)
                    if e.to_string().contains("timed out") {
                        this.update(cx, |app: &mut SeanceApp, _| {
                            app.lead_tasks_timeouts = app.lead_tasks_timeouts.saturating_add(1);
                            if app.lead_tasks_timeouts >= TIMEOUTS_BEFORE_OFF {
                                app.lead_tasks_off = Some(Instant::now());
                            }
                        });
                    }
                    continue;
                }
            };
            let Ok(data) = serde_json::from_value::<LeadTasks>(data) else {
                continue;
            };
            this.update(cx, |app: &mut SeanceApp, cx| {
                app.lead_tasks_timeouts = 0;
                app.lead_tasks_off = None;
                if app.lead_tasks.get(&lead) != Some(&data) {
                    app.lead_tasks.insert(lead, data);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Chip count and whether its popover is open. `None` for any pane that
    /// is not its circle's lead.
    pub(super) fn lead_tasks_arg(&self, pane: &super::Pane) -> Option<(usize, bool)> {
        if self.stage_lead(&pane.workspace).as_deref() != Some(pane.slug.as_str()) {
            return None;
        }
        let open = self
            .lead_tasks
            .get(&pane.slug)
            .map_or(0, LeadTasks::open_count);
        Some((open, self.tasks_popover_open(&pane.slug)))
    }

    pub(super) fn tasks_popover_open(&self, slug: &str) -> bool {
        self.tasks_popover.as_ref().is_some_and(|p| p.slug == slug)
    }

    /// Open the lead's tasks card under the chip clicked at `at`, or close it
    /// if it is already open. Focus never moves, so the terminal stays live.
    pub(super) fn toggle_tasks_popover(
        &mut self,
        slug: &str,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.tasks_popover_open(slug) {
            self.close_tasks_popover(cx);
            return;
        }
        // The lead's to-do poll follows the selected circle.
        if let Some(pane) = self.panes.iter().find(|p| p.slug == slug) {
            self.selected_workspace = Some(pane.workspace.clone());
        }
        self.tasks_popover = Some(TasksPopover {
            slug: slug.to_string(),
            at,
        });
        cx.notify();
    }

    /// Close the card. Opening never moved focus, so closing leaves it be: a
    /// composer or notes editor keeps the caret.
    pub(super) fn close_tasks_popover(&mut self, cx: &mut Context<Self>) {
        if self.tasks_popover.take().is_some() {
            cx.notify();
        }
    }

    /// The card only makes sense while its pane is still its circle's lead and
    /// the layout shows the chip (Classic or Chat).
    pub(super) fn tasks_popover_valid(&self) -> bool {
        let Some(pop) = self.tasks_popover.as_ref() else {
            return false;
        };
        matches!(
            self.view.mode,
            super::views::ViewMode::Classic | super::views::ViewMode::Chat
        ) && self
            .panes
            .iter()
            .find(|p| p.slug == pop.slug)
            .is_some_and(|p| self.stage_lead(&p.workspace).as_deref() == Some(pop.slug.as_str()))
    }

    /// The floating card: raised, with its own scroll, over a backdrop that
    /// catches the click-away (the same trick as the ✦ menu).
    pub(super) fn render_tasks_popover(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let pop = self
            .tasks_popover
            .as_ref()
            .filter(|_| self.tasks_popover_valid())?;
        let empty = LeadTasks::default();
        let data = self.lead_tasks.get(&pop.slug).unwrap_or(&empty);
        let card = div()
            .id("tasks-popover")
            .w(px(CARD_W))
            .max_h(window.viewport_size().height * CARD_MAX_H)
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(SeancePalette::border())
            .bg(SeancePalette::bg_elevated())
            .shadow_lg()
            // Clicks inside the card must not reach the backdrop.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_tasks_face(&pop.slug, data, cx)),
            );
        Some(
            deferred(
                div()
                    .id("tasks-popover-backdrop")
                    .absolute()
                    .inset_0()
                    // Swallow mouse events so a click-away does not also land
                    // in the terminal underneath.
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| this.close_tasks_popover(cx)))
                    .child(
                        anchored()
                            .position(point(pop.at.x + px(16.), pop.at.y + px(14.)))
                            .anchor(Anchor::TopRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(card),
                    ),
            )
            .with_priority(10)
            .into_any_element(),
        )
    }

    pub(super) fn render_tasks_face(
        &self,
        slug: &str,
        data: &LeadTasks,
        cx: &Context<Self>,
    ) -> AnyElement {
        let now = now_ms();
        let label = |text: &str| {
            div()
                .mt_1()
                .text_xs()
                .text_color(SeancePalette::violet())
                .child(SharedString::from(text.to_string()))
        };
        let mut face = div()
            .id(SharedString::from(format!("tasks-face-{slug}")))
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .bg(SeancePalette::bg_elevated());

        face = face.child(label("Now"));
        match data.with_status("open").next() {
            Some(t) => {
                let mut sub = Vec::new();
                if let Some(ev) = data.evidence.as_deref().map(strip_spinner) {
                    if !ev.is_empty() {
                        sub.push((ev.to_string(), SeancePalette::flame()));
                    }
                }
                sub.push((
                    format!(
                        "started {} · {} so far",
                        clock(t.created_ms),
                        span(now.saturating_sub(t.created_ms))
                    ),
                    SeancePalette::text_faint(),
                ));
                face = face.child(self.task_row(slug, t, "▶", SeancePalette::flame(), sub, cx));
            }
            None => {
                face = face.child(
                    div()
                        .text_xs()
                        .text_color(SeancePalette::text_faint())
                        .child("nothing open"),
                )
            }
        }

        if !data.helpers.is_empty() {
            face = face.child(label("Helpers working"));
            for h in &data.helpers {
                let ev = h.evidence.as_deref().map(strip_spinner).unwrap_or("");
                face = face.child(
                    div()
                        .text_xs()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .text_color(SeancePalette::text())
                                .child(h.name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_color(SeancePalette::text_dim())
                                .child(ev.to_string()),
                        ),
                );
            }
        }

        for (title, status, glyph) in [("Up next", "queued", "☐"), ("Parked", "parked", "☐")] {
            let rows: Vec<&TaskRow> = data.with_status(status).collect();
            if rows.is_empty() {
                continue;
            }
            face = face.child(label(title));
            for t in rows {
                // A parked task was set aside when it finished its turn.
                let sub = match (status, t.finished_ms) {
                    ("parked", Some(at)) => {
                        vec![(format!("parked {}", clock(at)), SeancePalette::text_faint())]
                    }
                    _ => vec![],
                };
                face =
                    face.child(self.task_row(slug, t, glyph, SeancePalette::text_dim(), sub, cx));
            }
        }

        let done: Vec<&TaskRow> = data
            .tasks
            .iter()
            .filter(|t| matches!(t.status.as_str(), "done" | "failed"))
            .collect();
        if !done.is_empty() {
            face = face.child(label("Done lately"));
            for t in done {
                let end = t.finished_ms.unwrap_or(t.created_ms);
                let (glyph, colour, verb) = if t.status == "done" {
                    ("☑", SeancePalette::success(), "done")
                } else {
                    ("☒", SeancePalette::danger(), "failed")
                };
                let sub = vec![(
                    format!(
                        "{verb} {} · took {}",
                        clock(end),
                        span(end.saturating_sub(t.created_ms))
                    ),
                    SeancePalette::text_faint(),
                )];
                face = face.child(self.task_row(slug, t, glyph, colour, sub, cx));
            }
        }
        face.into_any_element()
    }

    /// One checklist row: glyph, first line, small lines under it. A click
    /// shows the task's full text.
    fn task_row(
        &self,
        slug: &str,
        t: &TaskRow,
        glyph: &str,
        glyph_colour: gpui::Hsla,
        sub: Vec<(String, gpui::Hsla)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let open = self.tasks_row_open.as_deref() == Some(t.id.as_str());
        let id = t.id.clone();
        let mut text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text())
                    .child(first_line(&t.body)),
            )
            .children(
                sub.into_iter()
                    .map(|(line, colour)| div().text_xs().text_color(colour).child(line)),
            );
        if open {
            let mut full = clean_body(&t.body);
            if full.len() > FULL_TEXT_CAP {
                let mut end = FULL_TEXT_CAP;
                while !full.is_char_boundary(end) {
                    end -= 1;
                }
                full.truncate(end);
                full.push('…');
            }
            text = text.child(
                div()
                    .mt_1()
                    .p_2()
                    .rounded_md()
                    .bg(SeancePalette::bg())
                    .border_1()
                    .border_color(SeancePalette::border())
                    .text_xs()
                    .text_color(SeancePalette::text_dim())
                    .child(full),
            );
        }
        div()
            .id(SharedString::from(format!("task-row-{slug}-{}", t.id)))
            .flex()
            .gap_2()
            .px_1()
            .py_0p5()
            .rounded_sm()
            .hover(|s| s.bg(SeancePalette::surface()))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tasks_row_open = if this.tasks_row_open.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id.clone())
                };
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .text_sm()
                    .text_color(glyph_colour)
                    .child(glyph.to_string()),
            )
            .child(text)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_task_loses_header_and_footer() {
        let body = "📱 From Brian's phone (Invoke):\nship the icon\nmore detail\n---\nreply with seance ctl finish";
        assert_eq!(clean_body(body), "ship the icon\nmore detail");
        assert_eq!(first_line(body), "ship the icon");
    }

    #[test]
    fn plain_task_is_kept() {
        assert_eq!(first_line("  build the thing\nsecond"), "build the thing");
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn spinner_glyph_is_stripped_but_words_are_not() {
        assert_eq!(strip_spinner("✻ Thinking… (12s)"), "Thinking… (12s)");
        assert_eq!(strip_spinner("Running tests"), "Running tests");
        assert_eq!(strip_spinner("  · Reading"), "Reading");
    }

    #[test]
    fn spans_read_in_minutes_then_hours() {
        assert_eq!(span(20_000), "under a min");
        assert_eq!(span(5 * 60_000), "5 min");
        assert_eq!(span(135 * 60_000), "2h 15m");
    }

    #[test]
    fn chip_counts_open_and_queued_not_parked() {
        let row = |status: &str| TaskRow {
            id: status.into(),
            status: status.into(),
            created_ms: 0,
            finished_ms: None,
            body: String::new(),
        };
        let data = LeadTasks {
            tasks: vec![row("open"), row("queued"), row("parked"), row("done")],
            ..Default::default()
        };
        assert_eq!(data.open_count(), 2);
    }
}
