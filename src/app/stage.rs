//! Stage: one circle's lead up front. A strip of circle pills on top, the
//! needs-you bar, the live lead terminal with its Doing / Next / Waiting on
//! line, the crew as cards, the circle's files, and a health footer.
//!
//! Stage replaces the rail and grid rather than drawing over them, so the
//! pane on stage is the one place its terminal view renders: keys and esc
//! reach it like any focused pane. The pane on stage is the window's active
//! pane when that is a terminal in the circle, else the circle's lead.
//! The lead log and health report are daemon-side files read through the fs
//! bridge on a background task, never from render.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, relative, AnyElement, Context, Hsla, SharedString, Window};
use gpui_component::{input::Input, ActiveTheme as _, StyledExt as _};

use crate::pane::{Pane, PaneBody};
use crate::runtime::snapshot::GridSnapshot;
use crate::theme::SeancePalette;

use super::stage_parse::{
    circle_folder, crew_order, done_chips, ellipsize, fit_pills, is_bare_shell, last_output_lines,
    lead_log_path, pane_host, parse_health, parse_lead_log, pill_width, CrewBand, Health,
    HealthLevel, LeadLine,
};
use super::util::{needs_dot_color, tip, tip_s};
use super::views::ViewMode;
use super::workspaces::{rel_label, WorkspaceAttention};
use super::{PaletteMode, RenameTarget, SeanceApp};

/// The lead log changes a few times an hour; the health check every 2.5h.
const LEAD_LOG_EVERY: Duration = Duration::from_secs(30);
const HEALTH_EVERY: Duration = Duration::from_secs(60);
/// Below this the crew column folds behind a toggle so the terminal keeps
/// its width.
const CREW_FOLD_BELOW: f32 = 1000.;
/// A long space name truncates rather than push the strip's pills out.
const PICKER_MAX_W: f32 = 180.;
/// Bottom rows read for a crew card's last line: enough to clear a TUI's
/// input box without walking the whole screen.
const LAST_LINE_ROWS: usize = 30;

/// Stage's per-window state. Everything here is derived or cached; nothing
/// is saved.
#[derive(Default)]
pub(super) struct StageState {
    /// circle slug → its lead, from the daemon's State.
    pub(super) leads: HashMap<String, String>,
    /// Put the circle's lead up on the next frame: set on coming onto
    /// Stage and by a circle pill. Other moves (a jump, ctrl+page, Open)
    /// keep the pane they land on.
    pub(super) want_lead: bool,
    /// (circle, its newest lead log entry).
    lead_log: Option<(String, Option<LeadLine>)>,
    lead_log_asked: Option<(String, Instant)>,
    lead_log_busy: bool,
    /// The health report: Stage's footer, and the Wall's too.
    pub(super) health: Option<Health>,
    health_asked: Option<Instant>,
    health_busy: bool,
    /// Narrow windows: the crew column is open.
    crew_open: bool,
    /// circle → the file tab Stage shows. Kept apart from the dock's pick
    /// so Stage's tiled files never overwrite Classic's dock tab.
    file_tab: HashMap<String, String>,
    /// Crew cards' last output lines.
    last_lines: LineCache,
}

/// pane slug → (grid rev, its last two output lines), so a card or tile
/// reads its grid only when the grid changed.
#[derive(Default)]
pub(super) struct LineCache(HashMap<String, (u64, Vec<String>)>);

impl LineCache {
    pub(super) fn lines(&mut self, slug: &str, snap: &GridSnapshot) -> &[String] {
        let stale = self.0.get(slug).is_none_or(|(rev, _)| *rev != snap.rev);
        if stale {
            let lines = last_output_lines(&bottom_rows(snap, LAST_LINE_ROWS), 2);
            self.0.insert(slug.to_string(), (snap.rev, lines));
        }
        self.0.get(slug).map_or(&[], |(_, l)| l.as_slice())
    }

    pub(super) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.0.retain(|s, _| keep(s));
    }
}

/// What a pane is doing, for its header or card.
pub(super) struct PaneState {
    pub(super) label: String,
    pub(super) color: Hsla,
    band: CrewBand,
    /// A shell with nothing in it, or a sleeping pane: the dashed card.
    quiet: bool,
}

/// One chip in the needs-you bar.
pub(super) struct Waiting {
    pub(super) circle: String,
    pub(super) pane: Option<String>,
    pub(super) what: String,
    ask: Option<(String, Vec<String>)>,
    /// A pane that finished while you were elsewhere, not one waiting.
    done: bool,
}

impl SeanceApp {
    /// Coming onto Stage shows the selected circle's lead.
    pub(super) fn stage_enter(&mut self) {
        self.stage.want_lead = true;
    }

    /// Repaint now and then on Stage and Wall: elapsed labels move and the
    /// lead logs, health report and Today feed come due.
    pub(super) fn start_stage_ticker(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(15))
                .await;
            let Some(this) = this.upgrade() else { break };
            this.update(cx, |app: &mut SeanceApp, cx| {
                if matches!(app.view.mode, ViewMode::Stage | ViewMode::Wall) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The circle's lead: the daemon's pick while it is a terminal here,
    /// else the circle's first terminal pane.
    pub(super) fn stage_lead(&self, ws: &str) -> Option<String> {
        let ok = |p: &&Pane| p.workspace == ws && stageable(p);
        self.stage
            .leads
            .get(ws)
            .and_then(|l| self.panes.iter().filter(ok).find(|p| &p.slug == l))
            .or_else(|| self.panes.iter().find(ok))
            .map(|p| p.slug.clone())
    }

    /// The pane Stage shows for the selected circle.
    pub(super) fn stage_pane(&self) -> Option<String> {
        let ws = self.selected_workspace.as_deref()?;
        self.active_slug
            .as_deref()
            .and_then(|a| {
                self.panes
                    .iter()
                    .find(|p| p.slug == a && p.workspace == ws && stageable(p))
            })
            .map(|p| p.slug.clone())
            .or_else(|| self.stage_lead(ws))
    }

    /// Keep the active pane on what Stage shows, so keys land there: the
    /// lead when asked for, and whenever the active pane is one Stage can't
    /// show (a file, a popped-out pane).
    pub(super) fn stage_sync(&mut self) {
        let Some(ws) = self.selected_workspace.clone() else {
            return;
        };
        // A jump to one of the circle's files opens its tab here.
        if let Some(a) = self.active_slug.as_deref() {
            if self.panes.iter().any(|p| {
                p.slug == a && p.workspace == ws && matches!(p.body, PaneBody::File { .. })
            }) {
                self.stage.file_tab.insert(ws.clone(), a.to_string());
            }
        }
        let target = if std::mem::take(&mut self.stage.want_lead) {
            self.stage_lead(&ws)
        } else {
            self.stage_pane()
        };
        let Some(target) = target else {
            return;
        };
        if self.active_slug.as_deref() != Some(target.as_str()) {
            self.active_slug = Some(target.clone());
            self.workspace_focus.insert(ws.clone(), target.clone());
            let _ = self.client.set_focus(Some(target.clone()), Some(ws));
            self.pending_focus = Some(target);
        }
    }

    /// Leave the Wall for Stage on `ws`: its lead, or `pane` when given
    /// (a needs-you tile's Open puts the pane that asked up front).
    pub(super) fn focus_on_stage(
        &mut self,
        ws: &str,
        pane: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_workspace.as_deref() != Some(ws) {
            self.select_workspace(ws, window, cx);
        }
        self.set_view(ViewMode::Stage, cx);
        if let Some(slug) = pane {
            self.stage.want_lead = false;
            self.set_active(slug, window, cx);
        }
    }

    /// Put a pane on stage, switching circle first when it lives elsewhere.
    fn stage_show(&mut self, ws: &str, slug: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_workspace.as_deref() != Some(ws) {
            self.select_workspace(ws, window, cx);
        }
        self.set_active(slug, window, cx);
    }

    /// ctrl+shift+page on Stage: step through the panes Stage can show, the
    /// lead and its crew, in pane order. Files and popped panes would only
    /// bounce back to the lead.
    pub(super) fn stage_cycle(
        &mut self,
        ws: &str,
        delta: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list: Vec<String> = self
            .panes
            .iter()
            .filter(|p| p.workspace == ws && stageable(p))
            .map(|p| p.slug.clone())
            .collect();
        if list.len() < 2 {
            return;
        }
        let cur = self
            .stage_pane()
            .and_then(|s| list.iter().position(|x| *x == s))
            .unwrap_or(0);
        let next = (cur as i32 + delta).rem_euclid(list.len() as i32) as usize;
        self.set_active(&list[next], window, cx);
    }

    pub(super) fn open_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = PaletteMode::Jump {
            query: String::new(),
            selected: 0,
        };
        let fh = self.focus_handle.clone();
        window.focus(&fh, cx);
        cx.notify();
    }

    /// Ask the daemon for the lead log and health report when they're due.
    /// Cheap when nothing is: a few comparisons per frame.
    fn stage_feeds(&mut self, cx: &mut Context<Self>) {
        self.health_feed(cx);
        let conf = crate::term_config::get();
        if let (Some(map), Some(ws)) = (
            conf.seance_circle_map.as_ref(),
            self.selected_workspace.as_ref(),
        ) {
            let due = !self.stage.lead_log_busy
                && self
                    .stage
                    .lead_log_asked
                    .as_ref()
                    .is_none_or(|(c, at)| c != ws || at.elapsed() >= LEAD_LOG_EVERY);
            if due {
                let (map, ws) = (map.clone(), ws.clone());
                self.stage.lead_log_busy = true;
                self.stage.lead_log_asked = Some((ws.clone(), Instant::now()));
                let client = Arc::clone(&self.client);
                let circle = ws.clone();
                cx.spawn(async move |this, cx| {
                    let line = cx
                        .background_executor()
                        .spawn(async move {
                            let (text, _) = client.fs_read_string(&map).ok()?;
                            read_lead_line(&client, &map, &text, &circle)
                        })
                        .await;
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |app: &mut SeanceApp, cx| {
                        app.stage.lead_log_busy = false;
                        // The circle changed mid-read: repaint so its own
                        // read starts now, not on the next tick.
                        if app.selected_workspace.as_ref() != Some(&ws) {
                            cx.notify();
                        }
                        let fresh = Some((ws, line));
                        if app.stage.lead_log != fresh {
                            app.stage.lead_log = fresh;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// Re-read the health report when it's due (Stage's footer, the Wall's
    /// feed and footer).
    pub(super) fn health_feed(&mut self, cx: &mut Context<Self>) {
        let conf = crate::term_config::get();
        if let Some(path) = conf.seance_health_file.as_ref() {
            let due = !self.stage.health_busy
                && self
                    .stage
                    .health_asked
                    .is_none_or(|at| at.elapsed() >= HEALTH_EVERY);
            if due {
                let path = path.clone();
                self.stage.health_busy = true;
                self.stage.health_asked = Some(Instant::now());
                let client = Arc::clone(&self.client);
                cx.spawn(async move |this, cx| {
                    let health = cx
                        .background_executor()
                        .spawn(async move {
                            let (text, _) = client.fs_read_string(&path).ok()?;
                            parse_health(&text)
                        })
                        .await;
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |app: &mut SeanceApp, cx| {
                        app.stage.health_busy = false;
                        if app.stage.health != health {
                            app.stage.health = health;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
    }

    pub(super) fn pane_state(&self, p: &Pane, cx: &Context<Self>) -> PaneState {
        let st = |label: &str, color, band, quiet| PaneState {
            label: label.to_string(),
            color,
            band,
            quiet,
        };
        if self.owners.get(&p.slug).is_some_and(|o| o.exited) {
            return st("✕ exited", SeancePalette::danger(), CrewBand::Quiet, false);
        }
        if p.asleep {
            return st(
                "☾ asleep",
                SeancePalette::text_faint(),
                CrewBand::Quiet,
                true,
            );
        }
        match self.pane_attention(&p.slug) {
            Some(WorkspaceAttention::NeedsHuman) => {
                return st(
                    "● needs you",
                    SeancePalette::violet(),
                    CrewBand::Needs,
                    false,
                );
            }
            Some(WorkspaceAttention::Working) => {
                let label = match self.busy_since.get(&p.slug) {
                    Some(t) => format!("◐ working · {}", rel_label(t.elapsed().as_millis() as u64)),
                    None => "◐ working".into(),
                };
                return st(&label, SeancePalette::flame(), CrewBand::Working, false);
            }
            _ => {}
        }
        let done = self.done_unseen.contains(&p.slug)
            || self
                .statuses
                .get(&p.slug)
                .is_some_and(|s| s.state == "done");
        if done {
            return st("✓ done", SeancePalette::success(), CrewBand::Done, false);
        }
        let title = p.title(cx);
        if is_bare_shell(&p.command, title.as_deref(), false) {
            return st("shell", SeancePalette::text_faint(), CrewBand::Quiet, true);
        }
        st("idle", SeancePalette::text_faint(), CrewBand::Idle, false)
    }

    /// Everything waiting on you, in every circle: panes that say they need
    /// you and questions agents asked.
    pub(super) fn stage_waiting(&self) -> Vec<Waiting> {
        let mut out = Vec::new();
        for p in &self.panes {
            let ask = self
                .asks
                .iter()
                .find(|a| a.answer.is_none() && a.from == p.slug);
            let status = self
                .statuses
                .get(&p.slug)
                .filter(|s| matches!(s.state.as_str(), "needs-human" | "blocked" | "risky"));
            if ask.is_none() && status.is_none() {
                continue;
            }
            let what = match (ask, status) {
                (Some(a), _) => format!("{} asks: {}", p.name, a.question),
                (None, Some(s)) => match s.note.as_deref().filter(|n| !n.is_empty()) {
                    Some(n) => format!("{}: {n}", p.name),
                    None if s.state == "blocked" => format!("{} is blocked", p.name),
                    None => format!("{} needs you", p.name),
                },
                (None, None) => continue,
            };
            out.push(Waiting {
                circle: p.workspace.clone(),
                pane: Some(p.slug.clone()),
                what,
                ask: None,
                done: false,
            });
        }
        // Questions from outside any pane (a ctl ask) have no pane to open,
        // so their answers ride on the chip.
        for a in self.asks.iter().filter(|a| a.answer.is_none()) {
            if self.panes.iter().any(|p| p.slug == a.from) {
                continue;
            }
            let choices = if a.choices.is_empty() {
                vec!["ok".to_string(), "no".to_string()]
            } else {
                a.choices.clone()
            };
            out.push(Waiting {
                circle: a.workspace.clone().unwrap_or_default(),
                pane: None,
                what: format!("{} asks: {}", a.from, a.question),
                ask: Some((a.id.clone(), choices)),
                done: false,
            });
        }
        out
    }

    /// Circles with a finished pane you haven't seen (their sticky unread
    /// is done), as chips after the waiting ones. Viewing the circle clears
    /// its unread, and with it the chip.
    fn stage_done(&self) -> Vec<Waiting> {
        // The circle you're on: paths that select without select_workspace
        // can leave its unread behind.
        let selected = self.selected_workspace.as_deref();
        let unread: Vec<&str> = self
            .workspace_unread
            .iter()
            .filter(|(ws, a)| {
                matches!(a, WorkspaceAttention::Done) && Some(ws.as_str()) != selected
            })
            .map(|(ws, _)| ws.as_str())
            .collect();
        if unread.is_empty() {
            return Vec::new();
        }
        let in_order = |keep: &dyn Fn(&Pane) -> bool| -> Vec<(&str, &str)> {
            self.panes
                .iter()
                .filter(|p| keep(p))
                .map(|p| (p.workspace.as_str(), p.slug.as_str()))
                .collect()
        };
        let unseen = in_order(&|p| self.done_unseen.contains(&p.slug));
        let status_done = in_order(&|p| {
            self.statuses
                .get(&p.slug)
                .is_some_and(|s| s.state == "done")
        });
        done_chips(&self.workspaces(), &unread, &unseen, &status_done)
            .into_iter()
            .map(|(circle, pane)| {
                let name = pane
                    .as_deref()
                    .and_then(|s| self.panes.iter().find(|p| p.slug == s))
                    .map(|p| p.name.clone());
                Waiting {
                    // No done pane: Open puts the lead up.
                    pane: pane.or_else(|| self.stage_lead(&circle)),
                    what: name.unwrap_or_else(|| "finished".into()),
                    circle,
                    ask: None,
                    done: true,
                }
            })
            .collect()
    }

    /// The whole Stage layout, in place of the rail and grid.
    pub(super) fn render_stage(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.stage_feeds(cx);
        let width: f32 = window.viewport_size().width.into();
        let ws = self.selected_workspace.clone();
        let on_stage = self.stage_pane();
        let lead = ws.as_deref().and_then(|w| self.stage_lead(w));
        let fold = width < CREW_FOLD_BELOW;
        let crew_w = if width >= 1200. { 330. } else { 260. };
        let show_crew = !fold || self.stage.crew_open;

        let top = self.render_stage_top(width, cx);
        let needs = self.render_stage_needs(cx);
        let lead_el = self.render_stage_lead(on_stage.as_deref(), lead.as_deref(), fold, cx);
        let crew_el = show_crew.then(|| {
            let crew = self.render_stage_crew(on_stage.as_deref(), crew_w, cx);
            if !fold {
                return crew;
            }
            // Folded and opened: the crew floats over the terminal's right
            // edge rather than narrowing it (a resize would reflow the PTY).
            div()
                .id("stage-crew-over")
                .absolute()
                .top(px(10.))
                .right(px(10.))
                .bottom(px(10.))
                .p(px(8.))
                .rounded(px(10.))
                .bg(SeancePalette::bg())
                .border_1()
                .border_color(SeancePalette::border())
                .occlude()
                .child(crew)
                .into_any_element()
        });
        let footer = self.render_stage_footer();

        div()
            .id("stage")
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(SeancePalette::bg())
            .child(self.render_daemon_error_bar(cx))
            .child(top)
            .children(needs)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .gap(px(10.))
                    .p(px(10.))
                    .child(lead_el)
                    .children(crew_el),
            )
            .children(footer)
            .into_any_element()
    }

    /// ✦ mark, circle pills, the layout toggle and the jump box.
    fn render_stage_top(&self, width: f32, cx: &Context<Self>) -> AnyElement {
        let rename = self.render_rename_field();
        let renaming = rename.is_some();
        let selected = self.selected_workspace.clone();
        let space = self.shown_space();
        let circles = self.subs_pref.spaces.circles_in(&space, &self.workspaces());
        let picker = self.render_space_picker(&space, "stage", cx);
        let picker_w = picker.as_ref().map_or(0., |(_, w)| w + 14.);
        let jump_w = if width >= 1200. { 270. } else { 190. };
        let switch_w = 190.;
        // Strip padding, the mark, the space picker, the toggle, the jump box
        // and the gaps.
        let avail = width - 28. - 84. - picker_w - switch_w - jump_w - 14. * 3.;
        let labels: Vec<String> = circles.iter().map(|c| self.workspace_label(c)).collect();
        let widths: Vec<f32> = labels.iter().map(|l| pill_width(l)).collect();
        let sel_idx = selected
            .as_ref()
            .and_then(|s| circles.iter().position(|c| c == s));
        let shown = fit_pills(&widths, avail, 74., 6., sel_idx);
        let hidden: Vec<&str> = (0..circles.len())
            .filter(|i| !shown.contains(i))
            .map(|i| labels[i].as_str())
            .collect();

        let pills = shown.iter().map(|&i| {
            let ws = circles[i].clone();
            let on = sel_idx == Some(i);
            let glyph = self.circle_glyph(&ws);
            div()
                .id(SharedString::from(format!("stage-pill-{ws}")))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(12.))
                .py(px(5.))
                .rounded_full()
                .text_xs()
                .cursor_pointer()
                .when(on, |d| {
                    d.bg(SeancePalette::surface())
                        .text_color(SeancePalette::text())
                        .font_medium()
                })
                .when(!on, |d| {
                    d.text_color(SeancePalette::text_dim())
                        .hover(|s| s.text_color(SeancePalette::text()))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_workspace(&ws, window, cx);
                    this.stage.want_lead = true;
                    cx.notify();
                }))
                .child(glyph)
                .child(labels[i].clone())
        });
        let more = (!hidden.is_empty()).then(|| {
            let names = hidden.join(", ");
            div()
                .id("stage-pill-more")
                .flex_none()
                .px(px(10.))
                .py(px(5.))
                .rounded_full()
                .border_1()
                .border_dashed()
                .border_color(SeancePalette::border())
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .cursor_pointer()
                .hover(|s| s.text_color(SeancePalette::text()))
                .tooltip(tip_s(format!("{names} · opens the jump palette")))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_jump(window, cx);
                }))
                .child(format!("+{} more", hidden.len()))
        });
        let shortcut = if cfg!(target_os = "macos") {
            "⌘⇧J"
        } else {
            "ctrl+shift+j"
        };

        div()
            .flex_none()
            .h(px(50.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(14.))
            .bg(SeancePalette::bg_elevated())
            .border_b_1()
            .border_color(SeancePalette::border())
            .child(
                div()
                    .id("stage-mark")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px_1()
                    .rounded_md()
                    .text_sm()
                    .font_semibold()
                    .cursor_pointer()
                    .hover(|s| s.bg(SeancePalette::surface()))
                    .tooltip(tip("connected guis · theme"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.gui_menu_open = !this.gui_menu_open;
                        cx.notify();
                    }))
                    .child(div().text_color(SeancePalette::flame()).child("✦"))
                    .child(div().text_color(SeancePalette::text()).child("seance")),
            )
            .children(picker.map(|(el, _)| el))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .children(rename)
                    .when(!renaming, |d| d.children(pills).children(more)),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(switch_w))
                    .child(self.render_view_switch("strip", cx)),
            )
            .child(
                div()
                    .id("stage-jump")
                    .flex_none()
                    .w(px(jump_w))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded_lg()
                    .bg(SeancePalette::bg())
                    .border_1()
                    .border_color(SeancePalette::border())
                    .text_xs()
                    .text_color(SeancePalette::text_faint())
                    .cursor_pointer()
                    .hover(|s| s.border_color(SeancePalette::flame_dim()))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_jump(window, cx);
                    }))
                    .child("⌕")
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child("Jump to circle, pane, file"),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(5.))
                            .rounded_sm()
                            .border_1()
                            .border_color(SeancePalette::border())
                            .text_size(px(10.))
                            .child(shortcut),
                    ),
            )
            .into_any_element()
    }

    /// Stage and Wall's space picker: the space on show, with the rail's
    /// needs-you dot when a circle in it waits on you, and its width for the
    /// strip's fitting. None with only one space.
    pub(super) fn render_space_picker(
        &self,
        space: &str,
        key: &str,
        cx: &Context<Self>,
    ) -> Option<(AnyElement, f32)> {
        let spaces = &self.subs_pref.spaces;
        if spaces.list.len() < 2 {
            return None;
        }
        let name = spaces.get(space).map_or(space, |s| s.name.as_str());
        let needs = matches!(
            self.space_attention(space),
            Some(WorkspaceAttention::NeedsHuman)
        );
        let open = self.space_menu_open;
        let el = div()
            .id(SharedString::from(format!("{key}-space-picker")))
            .flex_none()
            .max_w(px(PICKER_MAX_W))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(4.))
            .rounded_full()
            .border_1()
            .border_color(SeancePalette::border())
            .text_xs()
            .text_color(SeancePalette::text())
            .cursor_pointer()
            .when(open, |d| d.bg(SeancePalette::surface()))
            .hover(|s| s.bg(SeancePalette::surface()))
            .tooltip(tip(if cfg!(target_os = "macos") {
                "spaces (⌘⌥↑ / ⌘⌥↓ steps)"
            } else {
                "spaces (ctrl+alt+page up / down steps)"
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.space_menu_open = !this.space_menu_open;
                this.gui_menu_open = false;
                cx.notify();
            }))
            .when(needs, |d| {
                d.child(div().flex_none().text_color(needs_dot_color()).child("●"))
            })
            .child(div().min_w_0().truncate().child(name.to_string()))
            .child(
                div()
                    .flex_none()
                    .text_color(SeancePalette::text_faint())
                    .child("▾"),
            );
        // Padding, border, the caret and the dot, capped like the pill.
        let w = (22. + 14. + if needs { 14. } else { 0. } + name.chars().count() as f32 * 6.9)
            .min(PICKER_MAX_W);
        Some((el.into_any_element(), w))
    }

    /// The space picker's menu: every space with its circle count and
    /// needs-you dot. An empty space has nothing to show, so it can't be
    /// picked. Drawn from the root like the ✦ menu, with the same backdrop.
    pub(super) fn render_space_menu(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let top = match self.view.mode {
            ViewMode::Stage => 48.,
            ViewMode::Wall => 44.,
            ViewMode::Classic => return None,
        };
        if !self.space_menu_open {
            return None;
        }
        let shown = self.shown_space();
        let ordered = self.workspaces();
        let spaces = &self.subs_pref.spaces;
        let rows = spaces.list.iter().map(|sp| {
            let n = spaces.circles_in(&sp.id, &ordered).len();
            let on = sp.id == shown;
            let needs = matches!(
                self.space_attention(&sp.id),
                Some(WorkspaceAttention::NeedsHuman)
            );
            let id = sp.id.clone();
            div()
                .id(SharedString::from(format!("space-menu-{}", sp.id)))
                .px_1()
                .py_0p5()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .when(on, |d| d.bg(SeancePalette::surface()))
                .when(n > 0, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(SeancePalette::surface()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_space(&id, window, cx);
                        }))
                })
                .child(
                    div()
                        .flex_none()
                        .w(px(10.))
                        .text_color(needs_dot_color())
                        .child(if needs { "●" } else { "" }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if n > 0 {
                            SeancePalette::text()
                        } else {
                            SeancePalette::text_faint()
                        })
                        .child(sp.name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(SeancePalette::text_faint())
                        .child(n.to_string()),
                )
        });
        Some(
            div()
                .id("space-menu-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.space_menu_open = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("space-menu-card")
                        .absolute()
                        .left(px(100.))
                        .top(px(top))
                        .w(px(240.))
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .p_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(SeancePalette::flame_dim())
                        .bg(SeancePalette::bg_elevated())
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .px_1()
                                .text_xs()
                                .text_color(SeancePalette::text_faint())
                                .child("spaces"),
                        )
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// The rename field, for layouts with no rail to hold it (Stage, Wall):
    /// a focused field outside the frame would strand the keyboard.
    pub(super) fn render_rename_field(&self) -> Option<AnyElement> {
        let (target, input) = self.renaming.as_ref()?;
        let what = match target {
            RenameTarget::Pane(_) => "rename pane",
            RenameTarget::Workspace(_) => "rename circle",
            RenameTarget::Space(_) => "rename space",
        };
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(SeancePalette::text_faint())
                        .child(what),
                )
                .child(div().w(px(260.)).child(Input::new(input)))
                .into_any_element(),
        )
    }

    /// A pill's status mark: needs you, working, done, exited or idle.
    fn circle_glyph(&self, ws: &str) -> AnyElement {
        let exited = self
            .panes
            .iter()
            .any(|p| p.workspace == ws && self.owners.get(&p.slug).is_some_and(|o| o.exited));
        match self.workspace_attention_cx(ws) {
            Some(WorkspaceAttention::NeedsHuman) => div()
                .flex_none()
                .size(px(14.))
                .rounded_full()
                .bg(SeancePalette::violet().opacity(0.22))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(8.))
                        .rounded_full()
                        .bg(SeancePalette::violet()),
                )
                .into_any_element(),
            Some(WorkspaceAttention::Working) => glyph("◐", SeancePalette::flame()),
            Some(WorkspaceAttention::Done) => glyph("✓", SeancePalette::success()),
            None if exited => glyph("✕", SeancePalette::danger()),
            None => div()
                .flex_none()
                .size(px(6.))
                .rounded_full()
                .bg(SeancePalette::text_faint())
                .into_any_element(),
        }
    }

    /// "Needs you · N" and a chip per waiting pane, every circle, then
    /// "Done · N" and a chip per finished one. Hidden when there are none.
    fn render_stage_needs(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let mut waiting = self.stage_waiting();
        let n = waiting.len();
        let done = self.stage_done();
        let n_done = done.len();
        waiting.extend(done);
        if waiting.is_empty() {
            return None;
        }
        let label = match (n, n_done) {
            (0, d) => format!("Done · {d}"),
            (n, 0) => format!("Needs you · {n}"),
            (n, d) => format!("Needs you · {n} · Done · {d}"),
        };
        let accent = if n > 0 {
            SeancePalette::violet()
        } else {
            SeancePalette::success()
        };
        let chips = waiting.into_iter().enumerate().map(|(i, w)| {
            let circle_label = if w.circle.is_empty() {
                String::new()
            } else {
                self.workspace_label(&w.circle)
            };
            let mut chip = div()
                .id(SharedString::from(format!("stage-wait-{i}")))
                .flex_none()
                .max_w(px(460.))
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(10.))
                .pr(px(6.))
                .py(px(4.))
                .rounded_lg()
                .bg(SeancePalette::bg_elevated())
                .border_1()
                .border_color(SeancePalette::border())
                .when(!circle_label.is_empty(), |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_color(SeancePalette::text_dim())
                            .child(circle_label),
                    )
                })
                .when(w.done, |d| d.child(glyph("✓", SeancePalette::success())))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(SeancePalette::text())
                        .child(ellipsize(&w.what, 70)),
                );
            if let Some(slug) = w.pane {
                let ws = w.circle.clone();
                let done = w.done;
                chip = chip.child(
                    chip_button("Open", true)
                        .id(SharedString::from(format!("stage-wait-open-{i}")))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // Seen now, even when the circle was already selected.
                            if done {
                                this.workspace_unread.remove(&ws);
                            }
                            this.stage_show(&ws, &slug, window, cx);
                        })),
                );
            }
            if let Some((id, choices)) = w.ask {
                for (j, choice) in choices.into_iter().enumerate() {
                    let id = id.clone();
                    chip = chip.child(
                        chip_button(&choice, false)
                            .id(SharedString::from(format!("stage-wait-{i}-{j}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.answer_ask(&id, choice.clone(), cx);
                            })),
                    );
                }
            }
            chip
        });
        Some(
            div()
                .id("stage-needs")
                .flex_none()
                .h(px(42.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(10.))
                .overflow_x_scroll()
                .text_xs()
                .bg(accent.opacity(0.12))
                .border_b_1()
                .border_color(SeancePalette::border())
                .child(
                    div()
                        .flex_none()
                        .font_semibold()
                        .text_color(accent)
                        .child(label),
                )
                .children(chips)
                .into_any_element(),
        )
    }

    /// The pane on stage: its header, the lead log line, the live terminal.
    fn render_stage_lead(
        &self,
        on_stage: Option<&str>,
        lead: Option<&str>,
        fold: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let frame = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .rounded(px(10.))
            .border_1()
            .border_color(SeancePalette::border())
            .overflow_hidden();
        let Some(pane) = on_stage.and_then(|s| self.panes.iter().find(|p| p.slug == s)) else {
            return frame
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(SeancePalette::text_dim())
                        .child("No terminal in this circle yet"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(SeancePalette::text_faint())
                        .child("+ worker starts one"),
                )
                .into_any_element();
        };
        let is_lead = lead == Some(pane.slug.as_str());
        let (name, host) = pane_host(&pane.name, &pane.command);
        let state = self.pane_state(pane, cx);
        let crew_n = self
            .panes
            .iter()
            .filter(|p| p.workspace == pane.workspace && stageable(p) && p.slug != pane.slug)
            .count();
        let ws = pane.workspace.clone();
        let line = self
            .stage
            .lead_log
            .as_ref()
            .filter(|(c, _)| *c == ws)
            .and_then(|(_, l)| l.clone());

        let mut row1 = div()
            .flex()
            .items_center()
            .gap(px(9.))
            .text_sm()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(SeancePalette::text_faint())
                    .child(if is_lead { "LEAD" } else { "ON STAGE" }),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_semibold()
                    .text_color(SeancePalette::text())
                    .child(name.to_string()),
            )
            .children(host.map(host_chip));
        if !is_lead {
            if let Some(lead) = lead.map(str::to_string) {
                row1 = row1.child(
                    chip_button("← back to lead", false)
                        .id("stage-back-to-lead")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_active(&lead, window, cx);
                        })),
                );
            }
        }
        if pane.asleep {
            let ws = ws.clone();
            row1 = row1.child(chip_button("awaken", true).id("stage-awaken").on_click(
                cx.listener(move |this, _, window, cx| {
                    this.wake_workspace_focused(&ws, window, cx);
                }),
            ));
        }
        row1 = row1.child(
            div()
                .ml_auto()
                .flex_none()
                .text_xs()
                .text_color(state.color)
                .child(state.label),
        );
        if fold {
            let open = self.stage.crew_open;
            row1 = row1.child(
                chip_button(
                    &format!("crew · {crew_n} {}", if open { "▸" } else { "◂" }),
                    false,
                )
                .id("stage-crew-toggle")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.stage.crew_open = !this.stage.crew_open;
                    cx.notify();
                })),
            );
        }

        let row2 = line.map(|l| {
            let field = |key: &'static str, value: Option<String>, accent: bool| {
                value.map(|v| {
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .flex()
                        .gap_1()
                        .child(
                            div()
                                .flex_none()
                                .text_color(SeancePalette::text_faint())
                                .child(key),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(if accent {
                                    SeancePalette::violet()
                                } else {
                                    SeancePalette::text_dim()
                                })
                                .child(v),
                        )
                })
            };
            div()
                .flex()
                .gap(px(18.))
                .min_w_0()
                .overflow_hidden()
                .text_xs()
                .children(field("Doing", l.doing, false))
                .children(field("Next", l.next, false))
                .children(field("Waiting on", l.waiting, true))
        });

        let slug = pane.slug.clone();
        let flipped = self
            .flipped
            .as_ref()
            .filter(|(s, _)| *s == pane.slug)
            .map(|(_, d)| d.clone());
        let body = div()
            .id("stage-body")
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.set_active(&slug, window, cx);
                }),
            );
        let body = match flipped {
            Some(notes) => body.bg(SeancePalette::bg_elevated()).child(notes),
            // Asleep: the frozen frame, dimmed like a classic tile.
            None if pane.asleep => body
                .child(
                    div()
                        .size_full()
                        .opacity(0.45)
                        .child(pane.content_element()),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(SeancePalette::bg().opacity(0.35)),
                ),
            None => body.child(pane.content_element()),
        };

        frame
            .child(
                div()
                    .flex_none()
                    .px(px(14.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .bg(SeancePalette::bg_elevated())
                    .border_b_1()
                    .border_color(SeancePalette::border())
                    .child(row1)
                    .children(row2),
            )
            .child(body)
            .into_any_element()
    }

    /// The circle's other terminal panes, in crew order.
    fn stage_crew_slugs(&self, on_stage: Option<&str>, cx: &Context<Self>) -> Vec<String> {
        let Some(ws) = self.selected_workspace.as_deref() else {
            return Vec::new();
        };
        let crew: Vec<&Pane> = self
            .panes
            .iter()
            .filter(|p| p.workspace == ws && stageable(p) && Some(p.slug.as_str()) != on_stage)
            .collect();
        let bands = crew
            .iter()
            .enumerate()
            .map(|(i, p)| (i, self.pane_state(p, cx).band))
            .collect();
        crew_order(bands)
            .into_iter()
            .map(|i| crew[i].slug.clone())
            .collect()
    }

    /// CREW · N with "+ worker", a card per other pane, then FILES.
    fn render_stage_crew(
        &mut self,
        on_stage: Option<&str>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slugs = self.stage_crew_slugs(on_stage, cx);
        self.stage
            .last_lines
            .retain(|s| slugs.iter().any(|x| x == s));
        let mono = cx.theme().mono_font_family.clone();
        let mut cards = Vec::with_capacity(slugs.len());
        for slug in &slugs {
            let Some(idx) = self.panes.iter().position(|p| &p.slug == slug) else {
                continue;
            };
            let pane = &self.panes[idx];
            let state = self.pane_state(pane, cx);
            let (name, host) = pane_host(&pane.name, &pane.command);
            let name = name.to_string();
            let host = host.map(str::to_string);
            let p_slug = pane.slug.clone();
            let snap = pane.remote_terminal().map(|t| t.read(cx).snapshot.clone());
            let last =
                snap.and_then(|snap| self.stage.last_lines.lines(&p_slug, &snap).last().cloned());
            let quiet = state.quiet;
            let click = p_slug.clone();
            cards.push(
                div()
                    .id(SharedString::from(format!("stage-crew-{p_slug}")))
                    .flex_none()
                    .px(px(12.))
                    .py(px(11.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(SeancePalette::border())
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .cursor_pointer()
                    .when(quiet, |d| d.border_dashed())
                    .when(!quiet, |d| d.bg(SeancePalette::bg_elevated()))
                    .hover(|s| s.border_color(SeancePalette::flame_dim()))
                    .tooltip(tip("put this pane on stage"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_active(&click, window, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_sm()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .when(!quiet, |d| {
                                        d.font_medium().text_color(SeancePalette::text())
                                    })
                                    .when(quiet, |d| d.text_color(SeancePalette::text_dim()))
                                    .child(name),
                            )
                            .children(host.map(host_chip_owned))
                            .child(
                                div()
                                    .ml_auto()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(state.color)
                                    .child(state.label),
                            ),
                    )
                    .children(last.map(|l| {
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .text_xs()
                            .text_color(if quiet {
                                SeancePalette::text_faint()
                            } else {
                                SeancePalette::text_dim()
                            })
                            .child(l)
                    }))
                    .into_any_element(),
            );
        }
        let n = cards.len();
        let files = self.render_stage_files(cx);

        div()
            .flex_none()
            .w(px(width))
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(2.))
                    .pt(px(2.))
                    .text_xs()
                    .text_color(SeancePalette::text_faint())
                    .child(format!("CREW · {n}"))
                    .child(
                        chip_button("+ worker", false)
                            .id("stage-add-worker")
                            .tooltip(tip("new pane in this circle (ctrl+shift+n)"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_default_session(cx);
                            })),
                    ),
            )
            .child(
                // Many workers scroll here; the files keep their share below.
                div()
                    .id("stage-crew-cards")
                    .flex_shrink(1.)
                    .min_h_0()
                    .max_h(relative(if files.is_some() { 0.6 } else { 1.0 }))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(cards),
            )
            .children(files)
            .into_any_element()
    }

    /// FILES: the circle's file panes as tabs, the open one below. Shares the
    /// dock's open tab, so both layouts agree on which file is up.
    fn render_stage_files(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let ws = self.selected_workspace.clone()?;
        let files: Vec<&Pane> = self
            .panes
            .iter()
            .filter(|p| {
                p.workspace == ws && p.popped.is_none() && matches!(p.body, PaneBody::File { .. })
            })
            .collect();
        let open = [self.stage.file_tab.get(&ws), self.dock_tab.get(&ws)]
            .into_iter()
            .flatten()
            .find_map(|s| files.iter().find(|p| &p.slug == s))
            .or(files.first())?;
        let open_slug = open.slug.clone();
        let tabs = files.iter().map(|p| {
            let slug = p.slug.clone();
            let on = slug == open_slug;
            let ws = ws.clone();
            div()
                .id(SharedString::from(format!("stage-file-{slug}")))
                .flex_none()
                .pb(px(4.))
                .border_b_2()
                .border_color(if on {
                    SeancePalette::flame()
                } else {
                    gpui::transparent_black()
                })
                .text_color(if on {
                    SeancePalette::text()
                } else {
                    SeancePalette::text_faint()
                })
                .hover(|s| s.text_color(SeancePalette::text()))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.stage.file_tab.insert(ws.clone(), slug.clone());
                    cx.notify();
                }))
                .child(p.name.clone())
        });
        Some(
            div()
                .flex_1()
                .min_h(px(120.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex_none()
                        .mt(px(4.))
                        .px(px(2.))
                        .text_xs()
                        .text_color(SeancePalette::text_faint())
                        .child("FILES"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .rounded(px(10.))
                        .bg(SeancePalette::bg_elevated())
                        .border_1()
                        .border_color(SeancePalette::border())
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("stage-file-tabs")
                                .flex_none()
                                .px(px(12.))
                                .pt(px(9.))
                                .flex()
                                .gap(px(12.))
                                .text_xs()
                                .overflow_x_scroll()
                                .children(tabs),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .child(open.content_element()),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The health report's facts, a dot for those that have one: XPS, Mac,
    /// leads, plan. None when there is no report to show.
    pub(super) fn health_facts(&self) -> Option<Vec<(Option<Hsla>, String)>> {
        crate::term_config::get().seance_health_file.as_ref()?;
        let h = self.stage.health.as_ref()?;
        let level = match h.level {
            HealthLevel::Healthy => SeancePalette::success(),
            HealthLevel::Warning => SeancePalette::flame(),
            HealthLevel::Critical => SeancePalette::danger(),
        };
        let xps = match &h.xps_memory {
            Some(m) => format!("XPS {} · {m}", h.status),
            None => format!("XPS {}", h.status),
        };
        let (mac_dot, mac) = match &h.mac {
            Some(m) => (SeancePalette::success(), m.clone()),
            None => (
                SeancePalette::text_faint(),
                "Mac asleep · panes waiting".to_string(),
            ),
        };
        let mut facts = vec![(Some(level), xps), (Some(mac_dot), mac)];
        facts.extend(h.leads.clone().map(|l| (None, l)));
        facts.extend(h.plan.clone().map(|p| (None, p)));
        Some(facts)
    }

    /// One line from the health report: XPS, Mac, leads, plan, report time.
    fn render_stage_footer(&self) -> Option<AnyElement> {
        let facts = self.health_facts()?;
        let at = self.stage.health.as_ref().and_then(|h| h.at.clone());
        let item = |dot: Option<Hsla>, text: String| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .children(dot.map(|c| div().text_color(c).child("●")))
                .child(text)
        };
        Some(
            div()
                .flex_none()
                .h(px(28.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(18.))
                .overflow_hidden()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .bg(SeancePalette::bg_elevated())
                .border_t_1()
                .border_color(SeancePalette::border())
                .children(facts.into_iter().map(|(dot, text)| item(dot, text)))
                .children(at.map(|at| {
                    div()
                        .ml_auto()
                        .flex_none()
                        .child(format!("health report {at}"))
                }))
                .into_any_element(),
        )
    }
}

/// The newest Doing / Next / Waiting on for `circle`, from the circle map
/// text already read from `map`. Runs off the UI thread.
pub(super) fn read_lead_line(
    client: &crate::gui_client::GuiClient,
    map: &str,
    map_text: &str,
    circle: &str,
) -> Option<LeadLine> {
    let folder = circle_folder(map_text, circle)?;
    let (log, _) = client.fs_read_string(&lead_log_path(map, &folder)).ok()?;
    parse_lead_log(&log)
}

/// Terminals that can take the stage: not files, not popped out.
pub(super) fn stageable(p: &Pane) -> bool {
    matches!(p.body, PaneBody::Remote { .. }) && p.popped.is_none()
}

/// The last `n` screen rows as text.
fn bottom_rows(snap: &GridSnapshot, n: usize) -> Vec<String> {
    let cols = snap.cols as usize;
    let rows = snap.rows as usize;
    if cols == 0 {
        return Vec::new();
    }
    (rows.saturating_sub(n)..rows)
        .filter_map(|r| snap.cells.get(r * cols..(r + 1) * cols))
        .map(|cells| cells.iter().map(|c| c.c).collect())
        .collect()
}

pub(super) fn glyph(text: &'static str, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn host_chip(host: &str) -> gpui::Div {
    host_chip_owned(host.to_string())
}

pub(super) fn host_chip_owned(host: String) -> gpui::Div {
    div()
        .flex_none()
        .px(px(7.))
        .rounded_full()
        .border_1()
        .border_color(SeancePalette::border())
        .text_size(px(10.5))
        .text_color(SeancePalette::text_dim())
        .child(host)
}

/// The small outlined (or, `strong`, violet-filled) buttons in the strip,
/// the needs-you chips and the crew header.
pub(super) fn chip_button(label: &str, strong: bool) -> gpui::Div {
    let d = div()
        .flex_none()
        .px(px(9.))
        .py(px(2.))
        .rounded_md()
        .text_xs()
        .cursor_pointer()
        .child(label.to_string());
    if strong {
        d.bg(SeancePalette::violet())
            .text_color(SeancePalette::bg())
            .font_semibold()
            .hover(|s| s.opacity(0.85))
    } else {
        d.border_1()
            .border_color(SeancePalette::border())
            .text_color(SeancePalette::text_dim())
            .hover(|s| s.text_color(SeancePalette::text()))
    }
}
