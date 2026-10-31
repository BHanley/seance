//! The Chat layout: circles on the left, the selected circle's thread on
//! the right, drawn from Invoke's own history (`chat_api`) so the phone and
//! this window share one conversation. No terminal renders here; "show
//! terminal" goes back to Classic on the circle's lead.
//!
//! A background loop refreshes the circle list and the open thread, then
//! long-polls Invoke's change counter. Rendering reads the last answers.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    div, img, prelude::*, px, relative, AnyElement, Context, Entity, Focusable as _, ScrollHandle,
    SharedString, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::text::{TextView, TextViewStyle};
use gpui_component::{StyledExt as _, WindowExt as _};
use regex::Regex;

use crate::chat_api::{self, CircleRow, Circles, Msg, Progress, Thread, Usage, UsageWindow};
use crate::events;
use crate::theme::SeancePalette;

use super::lead_tasks::clock;
use super::util::{now_ms, tip};
use super::views::ViewMode;
use super::SeanceApp;

/// The long-poll waits this long for Invoke to report a change.
const POLL_SECS: u64 = 25;
/// After a failed round, wait before trying again.
const RETRY: Duration = Duration::from_secs(3);
/// While another layout is up the loop only checks back this often.
const IDLE: Duration = Duration::from_secs(1);
const LIST_W: f32 = 300.;
/// Invoke's alerts circle is read-only: nothing to say, no lead to open.
const ALERTS_WS: &str = "alerts";
/// A failed read report waits this long before the next try.
const READ_RETRY: Duration = Duration::from_secs(30);
/// A circle with a message newer than this is Active, as on the phone.
const ACTIVE_MS: u64 = 2 * 3600 * 1000;

#[derive(Default)]
pub(super) struct ChatState {
    circles: Vec<CircleRow>,
    selected: Option<String>,
    threads: HashMap<String, Thread>,
    progress: HashMap<String, Progress>,
    /// Invoke's change counter as of the last round.
    v: u64,
    input: Option<Entity<InputState>>,
    scroll: ScrollHandle,
    /// Frames left to pin the thread to its newest message.
    stick: u8,
    /// Highest id Invoke confirmed read, per circle.
    read_sent: HashMap<String, i64>,
    /// Circles with a read report in flight.
    read_busy: HashSet<String>,
    /// A failed read report is not retried before this.
    read_retry_at: HashMap<String, Instant>,
    /// Whether this window was the active one at the last frame.
    window_active: bool,
    /// Counts down from zero: ids for bubbles shown before Invoke has them.
    next_local: i64,
    /// Text of a send that failed, to put back in the composer.
    restore: Option<String>,
    /// Focus the composer at the next frame (switching into Chat).
    pub(super) focus_input: bool,
    /// Image name to its cached file, once fetched.
    images: HashMap<String, PathBuf>,
    image_asked: HashSet<String>,
    error: Option<String>,
    /// Claude plan usage from the last thread answer; none hides the strip.
    usage: Option<Usage>,
}

impl ChatState {
    /// Newest message Invoke has given us for `ws`: a refresh asks only for
    /// what comes after it.
    fn last_id(&self, ws: &str) -> Option<i64> {
        self.threads
            .get(ws)?
            .messages
            .iter()
            .map(|m| m.id)
            .filter(|id| *id > 0)
            .max()
    }

    pub(super) fn any_working(&self) -> bool {
        self.circles.iter().any(|c| c.working)
    }
}

/// One round's answers; a failed call leaves its part as it was.
struct Round {
    circles: Option<Circles>,
    /// The thread, and whether it holds only the messages past an id.
    thread: Option<(String, Thread, bool)>,
    progress: Option<(String, Progress)>,
    error: Option<String>,
}

fn fetch_round(ws: Option<String>, after: Option<i64>) -> Round {
    let mut round = Round {
        circles: None,
        thread: None,
        progress: None,
        error: None,
    };
    match chat_api::circles() {
        Ok(c) => round.circles = Some(c),
        Err(e) => round.error = Some(e.to_string()),
    }
    if let (Some(ws), None) = (ws, &round.error) {
        round.thread = chat_api::thread(&ws, after)
            .ok()
            .map(|t| (ws.clone(), t, after.is_some()));
        round.progress = chat_api::progress(&ws).ok().map(|p| (ws, p));
    }
    round
}

fn url_re() -> &'static Regex {
    static URL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    URL.get_or_init(|| Regex::new(r"https?://[^\s<>]+").unwrap())
}

/// A link's end: trailing punctuation is the sentence's, not the URL's.
fn trim_link(url: &str) -> &str {
    url.trim_end_matches(['.', ',', ')', ']', ';', ':', '!', '?'])
}

/// A message as markdown that reads back as the same text: every ASCII
/// punctuation mark escaped, line breaks kept, URLs turned into links. The
/// selectable text view draws it, so the bubble keeps its look and gains
/// selection, copy and clickable links.
fn to_markdown(text: &str) -> String {
    let escape = |s: &str| -> String {
        s.chars()
            .flat_map(|c| {
                let esc = c.is_ascii_punctuation().then_some('\\');
                esc.into_iter().chain(std::iter::once(c))
            })
            .collect()
    };
    text.lines()
        .map(|line| {
            if line.is_empty() {
                // A blank line still takes its height.
                return "\u{a0}".to_string();
            }
            let (mut out, mut at) = (String::new(), 0);
            for m in url_re().find_iter(line) {
                let link = trim_link(m.as_str());
                out.push_str(&escape(&line[at..m.start()]));
                out.push_str(&format!("[{}](<{link}>)", escape(link)));
                at = m.start() + link.len();
            }
            out.push_str(&escape(&line[at..]));
            out
        })
        .collect::<Vec<_>>()
        .join("\\\n")
}

/// The paths in `text` (whitespace-separated words starting `/` or `~/`)
/// that contain `word`, the word a double-click just selected.
fn paths_holding<'a>(text: &'a str, word: &str) -> Vec<&'a str> {
    text.split_whitespace()
        .map(|t| t.trim_start_matches(['(', '[', '<', '"', '\'', '`']))
        .map(|t| t.trim_end_matches([')', ']', '>', '"', '\'', '`', '.', ',', ';', ':', '!', '?']))
        .filter(|t| (t.starts_with('/') || t.starts_with("~/")) && t.len() > 1)
        .filter(|t| t.contains(word))
        .collect()
}

/// Double-click in a bubble: the text view has just selected a word. If it sits
/// in a file path of the message that exists here, open it the way the
/// terminal does: images, videos and pages in their app, anything else shown
/// in Finder.
fn open_path_under_double_click(text: &str, window: &mut Window, cx: &mut gpui::App) {
    let word = window.selected_text(cx);
    let word = word.trim();
    if word.is_empty() {
        return;
    }
    let found = paths_holding(text, word)
        .into_iter()
        .find_map(|p| crate::remote_term_view::existing_path(p));
    let Some(path) = found else { return };
    if path.is_file() && crate::remote_term_view::opens_in_app(&path) {
        crate::sysopen::open_detached(&path.to_string_lossy());
    } else {
        crate::sysopen::reveal_detached(&path);
    }
}

/// One-line preview of a message for the circle list.
fn preview(m: &Msg) -> String {
    let text = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = if text.is_empty() && m.image.is_some() {
        "📷 photo".to_string()
    } else {
        text
    };
    if m.from == "me" {
        format!("You: {text}")
    } else {
        text
    }
}

/// Circles Invoke's phone list shows under Active: not alerts, with unread
/// messages or a last message under two hours old, newest first.
fn active_circles(circles: &[CircleRow], now: u64) -> Vec<CircleRow> {
    let mut active: Vec<CircleRow> = circles
        .iter()
        .filter(|c| {
            c.ws != ALERTS_WS
                && (c.unread > 0
                    || c.last
                        .as_ref()
                        .is_some_and(|m| now.saturating_sub(m.ts) < ACTIVE_MS))
        })
        .cloned()
        .collect();
    active.sort_by_key(|c| std::cmp::Reverse(c.last.as_ref().map_or(0, |m| m.ts)));
    active
}

/// 12-hour clock, as the phone shows it: "12:50 PM", or with a weekday and
/// no minutes for the weekly reset: "Thu 2 AM".
fn reset_label(ts_ms: u64, with_day: bool) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let (day, hour, minute) = events::local_clock(ts_ms);
    let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    let ampm = if hour < 12 { "AM" } else { "PM" };
    if with_day {
        format!("{} {h12} {ampm}", DAYS[day as usize % 7])
    } else {
        format!("{h12}:{minute:02} {ampm}")
    }
}

/// The thin usage line under the thread header, like the phone's: "5h 18% ·
/// ⟳ 12:50 PM   7d 35% · ⟳ Thu 2 AM". Numbers go red from 80%. None when
/// neither window is known.
fn usage_strip(u: &Usage) -> Option<AnyElement> {
    let part = |label: &str, w: &UsageWindow, with_day: bool| {
        let hot = w.used >= 80.;
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(label.to_string())
            .child(
                div()
                    .font_semibold()
                    .text_color(if hot {
                        SeancePalette::danger()
                    } else {
                        SeancePalette::text_faint()
                    })
                    .child(format!("{}%", w.used.round() as i64)),
            )
            .children(
                w.resets
                    .map(|at| format!("· ⟳ {}", reset_label(at, with_day))),
            )
    };
    let parts: Vec<_> = [
        u.five_hour.as_ref().map(|w| part("5h", w, false)),
        u.seven_day.as_ref().map(|w| part("7d", w, true)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        return None;
    }
    Some(
        div()
            .flex_none()
            .py_1()
            .flex()
            .justify_center()
            .gap_4()
            .text_xs()
            .text_color(SeancePalette::text_faint())
            .children(parts)
            .into_any_element(),
    )
}

/// The bubble's text keeps the bubble's own look: no gap between lines and the
/// colors and size of the bubble around it.
fn bubble_text_style() -> TextViewStyle {
    TextViewStyle {
        paragraph_gap: gpui::rems(0.),
        is_dark: !crate::theme::is_light(),
        ..Default::default()
    }
}

/// A list heading: a space's name, or Active.
fn list_label(text: String) -> gpui::Div {
    div()
        .px_3()
        .pt_2()
        .pb_1()
        .text_xs()
        .text_color(SeancePalette::text_faint())
        .child(text)
}

impl SeanceApp {
    pub(super) fn start_chat_poll(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            let Some(this) = this.upgrade() else { break };
            let (on, ws, after) = this.update(cx, |app: &mut SeanceApp, _| {
                let ws = app.chat.selected.clone();
                let after = ws.as_deref().and_then(|w| app.chat.last_id(w));
                (app.view.mode == ViewMode::Chat, ws, after)
            });
            if !on {
                cx.background_executor().timer(IDLE).await;
                continue;
            }
            let round = cx
                .background_executor()
                .spawn(async move { fetch_round(ws, after) })
                .await;
            let failed = round.error.is_some();
            let v = this.update(cx, |app: &mut SeanceApp, cx| {
                app.apply_round(round, cx);
                app.chat.v
            });
            if failed {
                cx.background_executor().timer(RETRY).await;
                continue;
            }
            // Returns on a change, or when the wait runs out; either way the
            // next round reads Invoke again.
            let polled = cx
                .background_executor()
                .spawn(async move { chat_api::poll(v, POLL_SECS) })
                .await;
            if polled.is_err() {
                cx.background_executor().timer(RETRY).await;
            }
        })
        .detach();
    }

    fn apply_round(&mut self, round: Round, cx: &mut Context<Self>) {
        let chat = &mut self.chat;
        chat.error = round.error;
        if let Some(c) = round.circles {
            chat.v = c.v;
            chat.circles = c.circles;
        }
        if chat.selected.is_none() {
            chat.selected = self
                .selected_workspace
                .clone()
                .filter(|ws| chat.circles.iter().any(|c| &c.ws == ws))
                .or_else(|| chat.circles.first().map(|c| c.ws.clone()));
        }
        if let Some((ws, p)) = round.progress {
            chat.progress.insert(ws, p);
        }
        if let Some((ws, t, delta)) = round.thread {
            self.apply_thread(ws, t, delta, cx);
        }
        cx.notify();
    }

    /// Take in a thread answer. A `delta` holds only the messages past what
    /// we had; they join it, and bubbles shown before Invoke had them go.
    fn apply_thread(
        &mut self,
        ws: String,
        mut thread: Thread,
        delta: bool,
        cx: &mut Context<Self>,
    ) {
        let chat = &mut self.chat;
        let before = chat.last_id(&ws);
        if delta {
            let mut merged = chat.threads.remove(&ws).unwrap_or_default().messages;
            merged.retain(|m| m.id > 0);
            for m in std::mem::take(&mut thread.messages) {
                match merged.iter_mut().find(|o| o.id == m.id) {
                    Some(o) => *o = m,
                    None => merged.push(m),
                }
            }
            thread.messages = merged;
        }
        let newest = thread.messages.iter().map(|m| m.id).max().unwrap_or(0);
        if before != Some(newest) {
            chat.stick = 3;
        }
        // Images the thread shows: fetch each once.
        for name in thread.messages.iter().filter_map(|m| m.image.clone()) {
            if chat.image_asked.insert(name.clone()) {
                cx.spawn(async move |this, cx| {
                    let leaf = name.clone();
                    let got = cx
                        .background_executor()
                        .spawn(async move { chat_api::file(&leaf) })
                        .await;
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |app: &mut SeanceApp, cx| {
                        match got {
                            Ok(path) => {
                                app.chat.images.insert(name, path);
                            }
                            // Let a later thread refresh try again.
                            Err(_) => {
                                app.chat.image_asked.remove(&name);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        chat.usage = thread.usage.clone();
        chat.threads.insert(ws, thread);
    }

    /// Reading the open thread tells Invoke, so the phone's badge clears: only
    /// while this window is the active one and Chat is showing that thread.
    fn maybe_mark_read(&mut self, cx: &mut Context<Self>) {
        let chat = &mut self.chat;
        let Some(ws) = chat.selected.clone() else {
            return;
        };
        if self.view.mode != ViewMode::Chat || !chat.window_active || chat.read_busy.contains(&ws) {
            return;
        }
        if chat
            .read_retry_at
            .get(&ws)
            .is_some_and(|at| Instant::now() < *at)
        {
            return;
        }
        let Some(thread) = chat.threads.get(&ws) else {
            return;
        };
        let Some(newest) = chat.last_id(&ws) else {
            return;
        };
        let done = thread
            .read
            .max(chat.read_sent.get(&ws).copied().unwrap_or(0));
        if newest <= done {
            return;
        }
        chat.read_busy.insert(ws.clone());
        cx.spawn(async move |this, cx| {
            let at = ws.clone();
            let sent = cx
                .background_executor()
                .spawn(async move { chat_api::mark_read(&at, newest) })
                .await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, _| {
                app.chat.read_busy.remove(&ws);
                // Only a report Invoke accepted counts; a failed one is retried.
                match sent {
                    Ok(()) => {
                        app.chat.read_sent.insert(ws, newest);
                    }
                    // Not every render: a refusal that never clears would
                    // otherwise run curl each frame.
                    Err(_) => {
                        app.chat
                            .read_retry_at
                            .insert(ws, Instant::now() + READ_RETRY);
                    }
                }
            });
        })
        .detach();
    }

    fn chat_select(&mut self, ws: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.selected.as_deref() != Some(ws) {
            self.chat.selected = Some(ws.to_string());
            self.tasks_popover = None;
            self.chat.stick = 3;
        }
        // The lead's to-do poll follows the selected circle.
        if self.workspaces().iter().any(|w| w == ws) {
            self.selected_workspace = Some(ws.to_string());
        }
        if let Some(input) = self.chat.input.as_ref() {
            let focus = input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        let ws = ws.to_string();
        let after = self.chat.last_id(&ws);
        cx.spawn(async move |this, cx| {
            let round = cx
                .background_executor()
                .spawn(async move { fetch_round(Some(ws), after) })
                .await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| app.apply_round(round, cx));
        })
        .detach();
        cx.notify();
    }

    fn chat_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(ws), Some(input)) = (self.chat.selected.clone(), self.chat.input.clone()) else {
            return;
        };
        if ws == ALERTS_WS {
            return;
        }
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.set_value("", window, cx));
        // Shown at once; the next thread answer replaces it with the real one.
        self.chat.next_local -= 1;
        let local_id = self.chat.next_local;
        let after = self.chat.last_id(&ws);
        let thread = self.chat.threads.entry(ws.clone()).or_default();
        thread.messages.push(Msg {
            id: local_id,
            from: "me".into(),
            text: text.clone(),
            ts: now_ms(),
            image: None,
        });
        self.chat.stick = 3;
        let thread_ws = ws.clone();
        cx.spawn(async move |this, cx| {
            let to = ws.clone();
            let body = text.clone();
            let sent = cx
                .background_executor()
                .spawn(async move { chat_api::send(&to, &body) })
                .await;
            let round = cx
                .background_executor()
                .spawn(async move { fetch_round(Some(ws), after) })
                .await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                app.apply_round(round, cx);
                if let Err(e) = sent {
                    // Not sent: the bubble goes and the words go back where
                    // they were typed.
                    if let Some(t) = app.chat.threads.get_mut(&thread_ws) {
                        t.messages.retain(|m| m.id != local_id);
                    }
                    app.chat.error = Some(format!("not sent: {e}"));
                    app.chat.restore = Some(text);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Back to Classic, on this circle's lead.
    fn chat_show_terminal(&mut self, ws: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::Classic, cx);
        self.selected_workspace = Some(ws.to_string());
        if let Some(lead) = self.stage_lead(ws) {
            self.set_active(&lead, window, cx);
        }
        cx.notify();
    }

    /// Is the human typing in the composer? The focus pass must leave it be.
    pub(super) fn chat_input_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.view.mode == ViewMode::Chat
            && self.chat.input.as_ref().is_some_and(|i| {
                window
                    .focused(cx)
                    .is_some_and(|f| f == i.read(cx).focus_handle(cx))
            })
    }

    pub(super) fn render_chat(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.chat.input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .auto_grow(1, 6)
                    .submit_on_enter(true)
                    .placeholder("Message the lead")
            });
            cx.subscribe_in(
                &input,
                window,
                |this: &mut SeanceApp, _input, event: &InputEvent, window, cx| {
                    // Enter sends; Shift+Enter adds a line (the input does that).
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        this.chat_send(window, cx);
                    }
                },
            )
            .detach();
            let focus = input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            self.chat.input = Some(input);
        }
        self.chat.window_active = window.is_window_active();
        if let Some(input) = self.chat.input.clone() {
            if let Some(text) = self.chat.restore.take() {
                // Whatever was typed since stays, after the failed text.
                let typed = input.read(cx).value().trim().to_string();
                let text = if typed.is_empty() {
                    text
                } else {
                    format!("{text}\n{typed}")
                };
                input.update(cx, |i, cx| i.set_value(text, window, cx));
            }
            if std::mem::take(&mut self.chat.focus_input) {
                let focus = input.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
        }
        self.maybe_mark_read(cx);
        if self.chat.stick > 0 {
            self.chat.stick -= 1;
            self.chat.scroll.scroll_to_bottom();
            window.request_animation_frame();
        }
        let list = self.render_chat_list(cx);
        let thread = self.render_chat_thread(cx);
        div()
            .id("chat")
            .flex_1()
            .h_full()
            .min_w_0()
            .flex()
            .bg(SeancePalette::bg())
            .child(list)
            .child(thread)
            .into_any_element()
    }

    /// Circles in the sidebar's order: grouped by space, as Classic lists them.
    fn chat_circles_in_order(&self) -> Vec<CircleRow> {
        let local = self.subs_pref.spaces.all_circles(&self.workspaces());
        let at = |ws: &str| local.iter().position(|l| l == ws).unwrap_or(usize::MAX);
        let mut rows = self.chat.circles.clone();
        rows.sort_by_key(|c| at(&c.ws));
        rows
    }

    fn render_chat_list(&self, cx: &Context<Self>) -> AnyElement {
        let spaces = &self.subs_pref.spaces;
        let mut col = div()
            .id("chat-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        // Active circles sit above the spaces, lifted out of theirs.
        let active = active_circles(&self.chat.circles, now_ms());
        if !active.is_empty() {
            col = col.child(list_label("ACTIVE".to_string()));
            for c in &active {
                col = col.child(self.chat_circle_row(c, cx));
            }
        }
        let mut space = String::new();
        for c in self.chat_circles_in_order() {
            if active.iter().any(|a| a.ws == c.ws) {
                continue;
            }
            let id = spaces.space_of(&c.ws).to_string();
            if id != space {
                if let Some(s) = spaces.list.iter().find(|s| s.id == id) {
                    col = col.child(list_label(s.name.to_uppercase()));
                }
                space = id;
            }
            col = col.child(self.chat_circle_row(&c, cx));
        }
        div()
            .w(px(LIST_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(SeancePalette::bg_elevated())
            .border_r_1()
            .border_color(SeancePalette::border())
            .child(
                div()
                    .flex_none()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.render_view_switch("chat", cx)),
            )
            .child(col)
            .into_any_element()
    }

    fn chat_circle_row(&self, c: &CircleRow, cx: &Context<Self>) -> AnyElement {
        let on = self.chat.selected.as_deref() == Some(c.ws.as_str());
        let ws = c.ws.clone();
        let pulse = 0.45 + 0.55 * (((now_ms() / 300) % 2) as f32);
        div()
            .id(SharedString::from(format!("chat-circle-{}", c.ws)))
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .when(on, |d| d.bg(SeancePalette::surface()))
            .hover(|s| s.bg(SeancePalette::surface()))
            .on_click(cx.listener(move |this, _, window, cx| this.chat_select(&ws, window, cx)))
            .child(div().flex_none().w(px(30.)).flex().justify_center().child(
                super::sprite::render(&super::sprite::for_circle(&c.ws), 2., c.working),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .text_color(SeancePalette::text())
                                    .child(c.name.clone()),
                            )
                            .when(c.working, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(SeancePalette::flame().opacity(pulse)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(SeancePalette::text_faint())
                            .child(c.last.as_ref().map(preview).unwrap_or_default()),
                    ),
            )
            .when(c.unread > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .rounded_full()
                        .bg(SeancePalette::flame())
                        .text_xs()
                        .text_color(SeancePalette::bg())
                        .child(c.unread.to_string()),
                )
            })
            .into_any_element()
    }

    fn render_chat_thread(&self, cx: &Context<Self>) -> AnyElement {
        let Some(ws) = self.chat.selected.clone() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(SeancePalette::text_faint())
                .child(match &self.chat.error {
                    Some(e) => format!("Can't reach Invoke: {e}"),
                    None => "Loading circles…".to_string(),
                })
                .into_any_element();
        };
        let circle = self.chat.circles.iter().find(|c| c.ws == ws);
        let name = circle.map_or(ws.clone(), |c| c.name.clone());
        let working = circle.is_some_and(|c| c.working);
        let line = self
            .chat
            .progress
            .get(&ws)
            .filter(|p| p.busy)
            .and_then(|p| p.doing.clone())
            .unwrap_or_default();
        let lead = self.stage_lead(&ws);
        let todo = lead
            .as_ref()
            .and_then(|l| self.lead_tasks.get(l))
            .map_or(0, |d| d.open_count());

        let mut header = div()
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(SeancePalette::border())
            .child(super::sprite::render(
                &super::sprite::for_circle(&ws),
                2.5,
                working,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .text_color(SeancePalette::text())
                            .child(name),
                    )
                    .when(!line.is_empty(), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(SeancePalette::flame())
                                .child(line),
                        )
                    }),
            );
        let read_only = ws == ALERTS_WS;
        let tasks_open = lead.as_deref().is_some_and(|l| self.tasks_popover_open(l));
        let chip_lead = lead.clone();
        header = header.when(!read_only, |header| {
            header.child(
                div()
                    .id("chat-todo")
                    .flex_none()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_xs()
                    .text_color(if tasks_open {
                        SeancePalette::bg()
                    } else {
                        SeancePalette::text_dim()
                    })
                    .when(tasks_open, |d| d.bg(SeancePalette::violet()))
                    .hover(|s| s.text_color(SeancePalette::flame()))
                    .cursor_pointer()
                    .tooltip(tip("the lead's tasks"))
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                        if let Some(lead) = chip_lead.as_deref() {
                            this.toggle_tasks_popover(lead, ev.position(), cx);
                        }
                    }))
                    .child(format!("☑ {todo}")),
            )
        });
        header = header.when(!read_only, |header| {
            header.child(
                div()
                    .id("chat-terminal")
                    .flex_none()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_xs()
                    .text_color(SeancePalette::text_dim())
                    .bg(SeancePalette::surface())
                    .hover(|s| s.text_color(SeancePalette::flame()))
                    .cursor_pointer()
                    .tooltip(tip("flip to Classic, on this circle's lead"))
                    .on_click({
                        let ws = ws.clone();
                        cx.listener(move |this, _, window, cx| {
                            this.chat_show_terminal(&ws, window, cx)
                        })
                    })
                    .child("show terminal"),
            )
        });

        let mut thread = div()
            .id("chat-thread")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.chat.scroll)
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_2();
        let messages = self.chat.threads.get(&ws).map(|t| t.messages.as_slice());
        for m in messages.unwrap_or_default() {
            thread = thread.child(self.chat_bubble(m, cx));
        }
        if messages.is_some_and(<[Msg]>::is_empty) {
            thread = thread.child(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_faint())
                    .child("No messages yet. Say something to the lead."),
            );
        }

        let mut col = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .children(self.chat.usage.as_ref().and_then(usage_strip));
        if let Some(e) = &self.chat.error {
            col = col.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(SeancePalette::danger())
                    .child(format!("Invoke: {e}")),
            );
        }
        col.child(thread)
            .when(!read_only, |col| col.child(self.render_chat_composer(cx)))
            .into_any_element()
    }

    fn render_chat_composer(&self, cx: &Context<Self>) -> AnyElement {
        let Some(input) = self.chat.input.as_ref() else {
            return div().into_any_element();
        };
        div()
            .flex_none()
            .p_3()
            .flex()
            .items_end()
            .gap_2()
            .border_t_1()
            .border_color(SeancePalette::border())
            .child(div().flex_1().min_w_0().child(Input::new(input)))
            .child(
                div()
                    .id("chat-send")
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .text_sm()
                    .text_color(SeancePalette::bg())
                    .bg(SeancePalette::flame())
                    .hover(|s| s.bg(SeancePalette::flame_dim()))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.chat_send(window, cx)))
                    .child("Send"),
            )
            .into_any_element()
    }

    /// Me on the right, the lead on the left, anything else centered.
    fn chat_bubble(&self, m: &Msg, cx: &Context<Self>) -> AnyElement {
        let me = m.from == "me";
        let system = !me && m.from != "lead";
        let mut body = div().flex().flex_col().gap_0p5();
        if let Some(path) = m.image.as_ref().and_then(|n| self.chat.images.get(n)) {
            // Click opens the cached file in its app (Preview).
            let file = path.clone();
            body = body.child(
                div()
                    .id(SharedString::from(format!("bubble-img-{}", m.id)))
                    .cursor_pointer()
                    .on_click(move |_, _, _| crate::sysopen::open_detached(&file.to_string_lossy()))
                    .child(
                        img(path.clone())
                            .max_w(px(360.))
                            .max_h(px(360.))
                            .rounded_md(),
                    ),
            );
        }
        if !m.text.is_empty() {
            let text = m.text.clone();
            body = body.child(
                div()
                    .id(SharedString::from(format!("bubble-text-{}", m.id)))
                    .min_h(px(14.))
                    // Capture phase, so the selection view cannot swallow it;
                    // the selection itself lands after, hence the defer.
                    .capture_any_mouse_up(cx.listener(
                        move |_, ev: &gpui::MouseUpEvent, window, cx| {
                            if ev.button == gpui::MouseButton::Left && ev.click_count == 2 {
                                let text = text.clone();
                                cx.defer_in(window, move |_, window, cx| {
                                    open_path_under_double_click(&text, window, cx);
                                });
                            }
                        },
                    ))
                    .child(
                        TextView::markdown(
                            SharedString::from(format!("bubble-md-{}", m.id)),
                            to_markdown(&m.text),
                        )
                        .style(bubble_text_style())
                        .selectable(true),
                    ),
            );
        }
        let stamp = if m.ts > 0 { clock(m.ts) } else { String::new() };
        if system {
            return div()
                .flex()
                .justify_center()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .child(body)
                .into_any_element();
        }
        let bubble = div()
            .max_w(relative(0.72))
            .px_3()
            .py_2()
            .rounded_lg()
            .text_sm()
            .text_color(SeancePalette::text())
            .bg(if me {
                SeancePalette::violet_dim()
            } else {
                SeancePalette::surface()
            })
            .child(body)
            .child(
                div()
                    .mt_0p5()
                    .text_xs()
                    .text_color(SeancePalette::text_faint())
                    .child(stamp),
            );
        div()
            .flex()
            .when(me, |d| d.justify_end())
            .child(bubble)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_reads_back_as_the_same_text() {
        assert_eq!(to_markdown("a *b* _c_ #d"), "a \\*b\\* \\_c\\_ \\#d");
        assert_eq!(to_markdown("1. one\n\ntwo"), "1\\. one\\\n\u{a0}\\\ntwo");
    }

    #[test]
    fn urls_become_links_without_their_trailing_punctuation() {
        assert_eq!(
            to_markdown("see https://bike.rodeo/x/. ok"),
            "see [https\\:\\/\\/bike\\.rodeo\\/x\\/](<https://bike.rodeo/x/>)\\. ok"
        );
        assert!(to_markdown("(https://a.b/c)").contains("](<https://a.b/c>)"));
    }

    #[test]
    fn a_double_clicked_word_finds_its_path() {
        let text = "wrote `~/Claude/out/a.png`, and /tmp/b.txt. See https://x.y/z";
        assert_eq!(paths_holding(text, "out"), ["~/Claude/out/a.png"]);
        assert_eq!(paths_holding(text, "b.txt"), ["/tmp/b.txt"]);
        assert!(paths_holding(text, "nope").is_empty());
        // A URL is not a file path.
        assert!(paths_holding(text, "x.y").is_empty());
    }

    #[test]
    fn preview_is_one_line_and_marks_your_messages() {
        let m = Msg {
            from: "me".into(),
            text: "hello\n  there".into(),
            ..Default::default()
        };
        assert_eq!(preview(&m), "You: hello there");
        let photo = Msg {
            from: "lead".into(),
            image: Some("a.png".into()),
            ..Default::default()
        };
        assert_eq!(preview(&photo), "📷 photo");
    }
}
