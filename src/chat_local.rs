//! The Chat layout with no server: the circle lead's own Claude conversation
//! shown as bubbles, and what you type in Chat typed into that lead's
//! terminal. Everything comes from the daemon on this Mac (`FsOp::LocalChat`)
//! and goes back through the pane's input, so it needs no setup.
//!
//! It fills the same `chat_api` types the Invoke source does, so the Chat
//! view draws either one. Invoke stays the source when this machine is
//! `seance-personal` and has an Invoke URL (see [`pick_local`]).

use std::collections::{hash_map::DefaultHasher, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Result};
use base64::Engine as _;
use serde_json::Value;

use crate::chat_api::{CircleRow, Circles, Msg, Thread};
use crate::gui_client::GuiClient;
use seance_core::agent_state::{arrow_keys, same_dialog, DialogKind, DialogPrompt};

/// Shown when a circle has no Claude lead to read.
pub const NO_LEAD_MESSAGE: &str = "Start Claude in a pane in this circle to chat.";
/// How often the local source looks again.
pub const POLL_MS: u64 = 1000;
/// Bubbles kept per circle; older ones drop off the top.
const KEEP: usize = 400;
/// A sent note waits this long for its line to reach the transcript.
const IN_FLIGHT_MS: u64 = 20_000;
/// Ids of notes not yet in the transcript and of questions on screen. Both
/// sit above any transcript line's id (a byte offset times 16), and apart
/// from each other so one never reads as the other.
const PENDING_BASE: i64 = i64::MAX / 2;
const PROMPT_BASE: i64 = i64::MAX / 4;

/// Invoke only when this machine is personal and has an Invoke URL.
pub fn pick_local(personal: bool, invoke_url: &str) -> bool {
    !(personal && !invoke_url.trim().is_empty())
}

/// Which source this run uses, decided once like the Invoke URL is read.
pub fn is_local() -> bool {
    static LOCAL: OnceLock<bool> = OnceLock::new();
    *LOCAL.get_or_init(|| pick_local(crate::term_config::personal(), &crate::chat_api::url()))
}

// ---- transcript lines to bubbles -------------------------------------------

/// Text a user line carries that Claude Code wrote, not the person.
const NOISE_PREFIXES: &[&str] = &[
    "<command-",
    "<local-command",
    "<system-reminder",
    "<task-notification",
    "<bash-",
    "Caveat: The messages below",
    "[Request interrupted by user",
];

/// Milliseconds since the epoch for `2026-11-07T19:32:39.346Z`; 0 when the
/// stamp is not in that shape.
pub fn iso_ms(s: &str) -> u64 {
    let num = |a: usize, b: usize| s.get(a..b).and_then(|t| t.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        num(0, 4),
        num(5, 7),
        num(8, 10),
        num(11, 13),
        num(14, 16),
        num(17, 19),
    ) else {
        return 0;
    };
    let ms = s
        .get(19..)
        .and_then(|r| r.strip_prefix('.'))
        .map(|r| {
            r.chars()
                .take_while(char::is_ascii_digit)
                .take(3)
                .collect::<String>()
        })
        .and_then(|f| format!("{f:0<3}").parse::<i64>().ok())
        .unwrap_or(0);
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    ((days * 86400 + h * 3600 + mi * 60 + se) * 1000 + ms).max(0) as u64
}

/// The bubbles one transcript line makes: what you typed, and what Claude
/// said. Tool calls and results, thinking, hook and system lines, side
/// chains and Claude Code's own wrappers make none. `offset` is the line's
/// byte position in the file, which is what keeps ids stable.
pub fn parse_line(offset: u64, line: &str) -> Vec<Msg> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    if v["isMeta"].as_bool() == Some(true) || v["isSidechain"].as_bool() == Some(true) {
        return Vec::new();
    }
    let from = match v["type"].as_str() {
        Some("user") => "me",
        Some("assistant") => "lead",
        _ => return Vec::new(),
    };
    let ts = v["timestamp"].as_str().map_or(0, iso_ms);
    let content = &v["message"]["content"];
    let mut texts: Vec<String> = Vec::new();
    match content {
        Value::String(s) => texts.push(s.clone()),
        Value::Array(blocks) => {
            // A tool result comes back as a user line; it is not you.
            if blocks.iter().any(|b| b["type"] == "tool_result") {
                return Vec::new();
            }
            texts.extend(
                blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str().map(str::to_string)),
            );
        }
        _ => {}
    }
    texts
        .into_iter()
        .enumerate()
        .filter(|(_, t)| {
            let t = t.trim();
            !t.is_empty()
                && t != "No response requested."
                && !(from == "me" && NOISE_PREFIXES.iter().any(|p| t.starts_with(p)))
        })
        .map(|(i, t)| Msg {
            id: (offset as i64) * 16 + (i as i64).min(15) + 1,
            from: from.into(),
            text: t.trim().to_string(),
            ts,
            ..Default::default()
        })
        .collect()
}

/// One circle's conversation, read a chunk at a time.
#[derive(Default)]
struct LocalThread {
    session: String,
    offset: u64,
    msgs: Vec<Msg>,
    /// Lines of yours seen, which is how a sent note is known to have landed.
    users: usize,
}

impl LocalThread {
    fn feed(&mut self, from: u64, text: &str) {
        let mut at = from;
        for line in text.split_inclusive('\n') {
            for m in parse_line(at, line) {
                if m.from == "me" {
                    self.users += 1;
                }
                self.msgs.push(m);
            }
            at += line.len() as u64;
        }
        self.offset = at;
        if self.msgs.len() > KEEP {
            self.msgs.drain(..self.msgs.len() - KEEP);
        }
    }
}

// ---- notes waiting for the lead --------------------------------------------

struct InFlight {
    text: String,
    at_ms: u64,
    users: usize,
}

/// Notes you typed while the lead was busy. One goes in per idle moment, and
/// the next waits until that one has shown up in the transcript (or Claude
/// has visibly started, or it has been too long), so two polls that both read
/// "idle" cannot send twice.
#[derive(Default)]
pub struct SendQueue {
    waiting: VecDeque<String>,
    in_flight: Option<InFlight>,
}

impl SendQueue {
    pub fn push(&mut self, text: String) {
        self.waiting.push_back(text);
    }

    /// The note to type into the pane now, if there is one.
    pub fn tick(&mut self, activity: &str, users: usize, now_ms: u64) -> Option<String> {
        if let Some(f) = &self.in_flight {
            if users > f.users
                || activity == "busy"
                || now_ms.saturating_sub(f.at_ms) > IN_FLIGHT_MS
            {
                self.in_flight = None;
            }
        }
        if self.in_flight.is_some() || activity != "idle" {
            return None;
        }
        let text = self.waiting.pop_front()?;
        self.in_flight = Some(InFlight {
            text: text.clone(),
            at_ms: now_ms,
            users,
        });
        Some(text)
    }

    /// Whether the first of [`Self::pending`] has been typed into the pane
    /// (the lead has it) rather than still waiting for an idle moment.
    pub fn first_is_typed(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Notes not in the transcript yet, in the order they were typed.
    pub fn pending(&self) -> Vec<&str> {
        self.in_flight
            .iter()
            .map(|f| f.text.as_str())
            .chain(self.waiting.iter().map(String::as_str))
            .collect()
    }
}

// ---- one round -------------------------------------------------------------

/// What the UI thread hands a round: the daemon, every circle with its lead,
/// and the open one.
pub struct LocalAsk {
    pub client: Arc<GuiClient>,
    /// (circle slug, label, the lead that was set (none: the circle's own
    /// Claude is used), a Claude there is mid-turn)
    pub circles: Vec<(String, String, Option<String>, bool)>,
    pub ws: Option<String>,
}

pub struct LocalRound {
    pub circles: Circles,
    pub thread: Option<(String, Thread)>,
    /// Nothing moved since the last round: the view need not repaint.
    pub quiet: bool,
}

#[derive(Default)]
struct WsState {
    thread: LocalThread,
    queue: SendQueue,
    prompts_seen: i64,
    prompt_key: Option<String>,
    sig: u64,
    /// The circle has no lead, or its lead is not a Claude.
    no_claude: bool,
    /// The pane the last round talked to (the set lead, else the circle's
    /// own Claude).
    lead: Option<String>,
    /// The dialog the last round showed as a card, so a tap can be checked
    /// against what the screen says now.
    dialog: Option<DialogPrompt>,
    /// Polls in a row the lead has read idle (a turn that just ended needs two
    /// before a note counts as answered, so one stray idle read can't).
    idle_rounds: u8,
}

/// Invoke's ticks for the notes in the thread, from what the pane shows.
/// Invoke: sending until the lead's task opens, got until it finishes,
/// answered after. Here the note is "got" from the moment it is typed into
/// the lead (see [`SendQueue`]) until its turn is over: the lead has replied
/// after it, or has read idle for two polls running. A lead stopped on a
/// question (`awaiting-input`) is mid-task, so the note stays "got".
pub fn mark_notes(msgs: &mut [Msg], activity: &str, idle_rounds: u8) {
    let Some(last) = msgs.iter().rposition(|m| m.from == "me") else {
        return;
    };
    let replied = msgs[last + 1..].iter().any(|m| m.from == "lead");
    let over = replied || (activity == "idle" && idle_rounds >= 2);
    for (i, m) in msgs.iter_mut().enumerate() {
        if m.from == "me" && m.status != "failed" {
            m.status = if i < last || over { "answered" } else { "got" }.into();
        }
    }
}

static STATE: Mutex<Option<HashMap<String, WsState>>> = Mutex::new(None);
static CIRCLES_SIG: Mutex<u64> = Mutex::new(0);
/// The daemon's last answer to `circle_states`.
static LAST_STATES: Mutex<Option<std::collections::HashMap<String, (bool, bool)>>> =
    Mutex::new(None);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn hash_of(parts: &[&dyn Fn(&mut DefaultHasher)]) -> u64 {
    let mut h = DefaultHasher::new();
    for p in parts {
        p(&mut h);
    }
    h.finish()
}

/// Read the open circle's conversation and screen, release any note that can
/// go, and list the circles.
pub fn round(ask: LocalAsk) -> Result<LocalRound> {
    // Working and waiting for every circle come from the daemon, by the rule
    // Invoke's list uses (`seance_core::circle_state`). A round that can't
    // ask keeps the last answer, so the rows don't flicker.
    let states = {
        let mut last = LAST_STATES.lock().unwrap();
        if let Ok(s) = ask.client.circle_states() {
            *last = Some(s);
        }
        last.clone().unwrap_or_default()
    };
    let rows: Vec<CircleRow> = ask
        .circles
        .iter()
        .map(|(ws, name, _, working)| {
            let (w, waiting) = states.get(ws).copied().unwrap_or((*working, false));
            CircleRow {
                ws: ws.clone(),
                name: name.clone(),
                working: w,
                waiting,
                ..Default::default()
            }
        })
        .collect();
    let circles_sig = hash_of(&[&|h| {
        for r in &rows {
            (&r.ws, &r.name, r.working, r.waiting).hash(h);
        }
    }]);
    let circles_moved = {
        let mut g = CIRCLES_SIG.lock().unwrap();
        let moved = *g != circles_sig;
        *g = circles_sig;
        moved
    };
    let mut circles = Circles {
        circles: rows,
        v: circles_sig,
    };
    let Some(ws) = ask.ws.clone() else {
        return Ok(LocalRound {
            circles,
            thread: None,
            quiet: !circles_moved,
        });
    };
    // A lead that was set wins; otherwise the daemon names the circle's own
    // Claude (hosted ones too). Asked before taking the state lock.
    let lead = match ask
        .circles
        .iter()
        .find(|c| c.0 == ws)
        .and_then(|c| c.2.clone())
    {
        Some(l) => Some(l),
        None => ask.client.local_lead(&ws)?,
    };
    let mut guard = STATE.lock().unwrap();
    let st = guard
        .get_or_insert_with(HashMap::new)
        .entry(ws.clone())
        .or_default();
    st.lead = lead.clone();
    let Some(lead) = lead else {
        return Ok(empty_round(circles, ws, st, circles_moved, "no-lead"));
    };
    let v = ask
        .client
        .local_chat(&lead, st.thread.offset, &st.thread.session)?;
    if v["claude"] != true {
        return Ok(empty_round(circles, ws, st, circles_moved, "no-claude"));
    }
    if v["reset"] == true {
        st.thread = LocalThread::default();
    }
    st.no_claude = false;
    st.thread.session = v["session"].as_str().unwrap_or_default().to_string();
    if let Some(b64) = v["b64"].as_str().filter(|b| !b.is_empty()) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| anyhow!("bad transcript chunk: {e}"))?;
        st.thread.feed(
            v["from"].as_u64().unwrap_or(0),
            &String::from_utf8_lossy(&bytes),
        );
    }
    let activity = v["activity"].as_str().unwrap_or("unknown").to_string();
    st.idle_rounds = if activity == "idle" {
        st.idle_rounds.saturating_add(1)
    } else {
        0
    };
    if let Some(text) = st.queue.tick(&activity, st.thread.users, now_ms()) {
        // Same path as typing in the pane: pasted, then Enter.
        if ask.client.inject(&lead, &text, true).is_err() {
            st.queue.waiting_front(text);
        }
    }
    // A dialog waiting on the pane's screen (first-launch pickers, the trust
    // question, permission prompts): not in the transcript, so it comes from
    // the screen. Shown as a card whose buttons answer it.
    let dialog = serde_json::from_value::<DialogPrompt>(v["prompt"].clone()).ok();
    let prompt = dialog.as_ref().map(dialog_card);
    let key = dialog.as_ref().map(|d| {
        format!(
            "{:?}|{}|{}|{}",
            d.kind,
            d.question,
            d.options.join("|"),
            d.screen
        )
    });
    if key.is_some() && key != st.prompt_key {
        st.prompts_seen += 1;
    }
    st.prompt_key = key;
    st.dialog = dialog;
    let mut messages = st.thread.msgs.clone();
    mark_notes(&mut messages, &activity, st.idle_rounds);
    let typed = st.queue.first_is_typed();
    for (i, text) in st.queue.pending().into_iter().enumerate() {
        messages.push(Msg {
            id: PENDING_BASE + i as i64,
            from: "me".into(),
            text: text.to_string(),
            ts: now_ms(),
            // The first one, once typed into the pane, is the lead's to read.
            status: if i == 0 && typed { "got" } else { "sending" }.into(),
            ..Default::default()
        });
    }
    if let Some((question, options)) = prompt {
        messages.push(Msg {
            id: PROMPT_BASE + st.prompts_seen,
            from: "lead".into(),
            text: if question.is_empty() {
                "Claude is waiting for you.".into()
            } else {
                question
            },
            ts: now_ms(),
            choices: options,
            ..Default::default()
        });
    }
    let busy = activity == "busy";
    let newest = messages.iter().map(|m| m.id).max().unwrap_or(0);
    if let Some(row) = circles.circles.iter_mut().find(|r| r.ws == ws) {
        row.working = busy;
        row.last = messages.last().cloned();
    }
    let waiting = states.get(&ws).is_some_and(|s| s.1);
    let ticks: Vec<&str> = messages
        .iter()
        .filter(|m| m.from == "me")
        .map(|m| m.status.as_str())
        .collect();
    let sig = hash_of(&[&|h| {
        (
            messages.len(),
            newest,
            &activity,
            busy,
            &st.prompt_key,
            waiting,
            &ticks,
        )
            .hash(h)
    }]);
    let quiet = sig == st.sig && !circles_moved;
    st.sig = sig;
    Ok(LocalRound {
        circles,
        thread: Some((
            ws,
            Thread {
                messages,
                read: newest,
                lead_busy: busy,
                waiting,
                ..Default::default()
            },
        )),
        quiet,
    })
}

fn empty_round(
    circles: Circles,
    ws: String,
    st: &mut WsState,
    circles_moved: bool,
    why: &str,
) -> LocalRound {
    st.no_claude = true;
    let sig = hash_of(&[&|h| why.hash(h)]);
    let quiet = sig == st.sig && !circles_moved;
    st.sig = sig;
    LocalRound {
        circles,
        thread: Some((ws, Thread::default())),
        quiet,
    }
}

impl SendQueue {
    /// A note the pane refused goes back to the front.
    fn waiting_front(&mut self, text: String) {
        self.in_flight = None;
        self.waiting.push_front(text);
    }
}

/// Queue a note for the circle's lead; the next round types it in when the
/// lead is at its prompt.
pub fn send(ws: &str, text: &str) -> Result<()> {
    let mut guard = STATE.lock().unwrap();
    let st = guard
        .get_or_insert_with(HashMap::new)
        .entry(ws.to_string())
        .or_default();
    if st.no_claude {
        return Err(anyhow!(NO_LEAD_MESSAGE));
    }
    st.queue.push(text.to_string());
    Ok(())
}

/// The button that leaves Chat for the pane itself: always offered with a
/// dialog, the way out when a card can't say it.
pub const SHOW_TERMINAL: &str = "Show terminal";
/// The one key an unreadable dialog gets from Chat. Never a blind Enter: that
/// could approve something the person has not seen.
pub const PRESS_ESC: &str = "Press Esc";

/// The card's stand-in for the sign-in box: Chat draws a masked text box and a
/// Send button where this "choice" would be a pill. Never sent as an answer.
pub const CODE_BOX: &str = "Sign-in code";

/// A dialog as a message card: its text and its buttons.
fn dialog_card(d: &DialogPrompt) -> (String, Vec<String>) {
    match d.kind {
        DialogKind::Numbered | DialogKind::Arrows => {
            let mut choices: Vec<String> = d
                .options
                .iter()
                .enumerate()
                .map(|(i, l)| format!("{}. {l}", i + 1))
                .collect();
            choices.push(SHOW_TERMINAL.into());
            (d.question.clone(), choices)
        }
        DialogKind::LoginCode => (
            "Claude needs you to sign in. Finish in the browser it opened, copy the code \
             it shows, and paste the code here."
                .to_string(),
            vec![CODE_BOX.into(), SHOW_TERMINAL.into()],
        ),
        DialogKind::Unknown => (
            format!("Claude is waiting for you. Its screen:\n{}", d.screen),
            vec![PRESS_ESC.into(), SHOW_TERMINAL.into()],
        ),
    }
}

/// The keys a tap sends, or why it must not send any. `old` is the dialog the
/// card showed, `new` the one on the screen now: a tap only presses keys while
/// it is still the same dialog.
pub fn plan_answer(old: &DialogPrompt, new: &DialogPrompt, choice: &str) -> Result<Vec<u8>> {
    if !same_dialog(old, new) {
        return Err(anyhow!("The prompt changed. Look at the new one first."));
    }
    if choice == PRESS_ESC {
        return Ok(b"\x1b".to_vec());
    }
    if new.kind == DialogKind::LoginCode {
        // Typed text only, through `send_code`: no tap is a key for this one.
        return Err(anyhow!("Type the code in the box."));
    }
    let n = choice
        .split_once(". ")
        .and_then(|(d, _)| d.parse::<usize>().ok())
        .filter(|n| (1..=new.options.len()).contains(n))
        .ok_or_else(|| anyhow!("not an option"))?;
    match new.kind {
        DialogKind::Numbered if n <= 9 => Ok(vec![b'0' + n as u8]),
        DialogKind::Arrows => Ok(arrow_keys(new.selected, n - 1)),
        _ => Err(anyhow!("Chat can't answer this one. Use the terminal.")),
    }
}

/// The pane the circle's last round talked to: what a tap on an option or
/// Stop goes to.
pub fn lead_of(ws: &str) -> Option<String> {
    STATE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(ws))
        .and_then(|st| st.lead.clone())
}

/// The circle's lead is a Claude (true until a round finds otherwise).
pub fn has_claude(ws: &str) -> bool {
    STATE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(ws))
        .is_none_or(|st| !st.no_claude)
}

/// A tapped button on a dialog card: re-read the pane's screen, and only if it
/// still shows that same dialog, press the keys (a digit, or arrows and Enter).
pub fn answer(client: &GuiClient, ws: &str, lead: &str, choice: &str) -> Result<()> {
    let old = STATE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(ws))
        .and_then(|st| st.dialog.clone())
        .ok_or_else(|| anyhow!("Claude is not waiting on that any more."))?;
    let v = client.local_chat(lead, u64::MAX, "")?;
    let fresh = serde_json::from_value::<DialogPrompt>(v["prompt"].clone())
        .map_err(|_| anyhow!("Claude is not waiting on that any more."))?;
    let keys = plan_answer(&old, &fresh, choice)?;
    client.input(lead, &keys)
}

/// The sign-in code, typed into the pane and submitted. Only while the pane
/// still shows the sign-in step. The code goes straight to that pane: it is not
/// kept (no chat message, no state, no log), and the daemon records nothing
/// for it (`GuiRequest::InjectSecret`).
pub fn send_code(client: &GuiClient, ws: &str, lead: &str, code: &str) -> Result<()> {
    let shown = STATE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(ws))
        .and_then(|st| st.dialog.as_ref().map(|d| d.kind));
    let code = check_code(code, shown)?;
    let v = client.local_chat(lead, u64::MAX, "")?;
    serde_json::from_value::<DialogPrompt>(v["prompt"].clone())
        .ok()
        .filter(|d| d.kind == DialogKind::LoginCode)
        .ok_or_else(|| anyhow!("Claude is not waiting for a code any more."))?;
    client.inject_secret(lead, code)
}

/// Is this something to send, and is the sign-in step what the card showed?
fn check_code(code: &str, shown: Option<DialogKind>) -> Result<&str> {
    let code = code.trim();
    if code.is_empty() || code.contains(['\n', '\r']) {
        return Err(anyhow!("Paste the code from the browser, on one line."));
    }
    if shown != Some(DialogKind::LoginCode) {
        return Err(anyhow!("Claude is not waiting for a code any more."));
    }
    Ok(code)
}

/// Esc: Claude stops its turn.
pub fn stop(client: &GuiClient, lead: &str) -> Result<()> {
    client.input(lead, b"\x1b")
}

#[cfg(test)]
mod tests {
    fn dialog(kind: DialogKind, options: &[&str], selected: usize) -> DialogPrompt {
        DialogPrompt {
            kind,
            question: "Do you trust the files in this folder?".into(),
            options: options.iter().map(|s| s.to_string()).collect(),
            selected,
            screen: "bottom of the screen".into(),
        }
    }

    #[test]
    fn a_dialog_becomes_a_card_with_its_options_and_a_way_to_the_terminal() {
        let d = dialog(DialogKind::Numbered, &["Yes, proceed", "No, exit"], 0);
        let (text, choices) = dialog_card(&d);
        assert_eq!(text, "Do you trust the files in this folder?");
        assert_eq!(choices, ["1. Yes, proceed", "2. No, exit", SHOW_TERMINAL]);
        // One it can't read: the screen, and Esc, never a blind Enter.
        let u = dialog(DialogKind::Unknown, &[], 0);
        let (text, choices) = dialog_card(&u);
        assert!(text.contains("bottom of the screen"));
        assert_eq!(choices, [PRESS_ESC, SHOW_TERMINAL]);
        assert!(!choices.iter().any(|c| c.to_lowercase().contains("enter")));
    }

    #[test]
    fn a_tap_presses_a_digit_or_arrows_and_enter_only_for_the_same_dialog() {
        let num = dialog(DialogKind::Numbered, &["Yes, proceed", "No, exit"], 0);
        assert_eq!(plan_answer(&num, &num, "2. No, exit").unwrap(), b"2");
        let arrows = dialog(
            DialogKind::Arrows,
            &["No, exit", "Yes, I trust this folder"],
            0,
        );
        assert_eq!(
            plan_answer(&arrows, &arrows, "2. Yes, I trust this folder").unwrap(),
            b"\x1b[B\r"
        );
        // The selection moved since the card was drawn: counted from where it is now.
        let moved = dialog(
            DialogKind::Arrows,
            &["No, exit", "Yes, I trust this folder"],
            1,
        );
        assert_eq!(
            plan_answer(&arrows, &moved, "1. No, exit").unwrap(),
            b"\x1b[A\r"
        );
        // A different dialog is on screen now: nothing is pressed.
        let other = dialog(DialogKind::Numbered, &["Dark mode", "Light mode"], 0);
        let e = plan_answer(&num, &other, "1. Yes, proceed")
            .unwrap_err()
            .to_string();
        assert!(e.contains("changed"), "{e}");
        // An option that is not there, and Enter-like words, press nothing.
        assert!(plan_answer(&num, &num, "7. Nope").is_err());
        assert!(plan_answer(&num, &num, "Enter").is_err());
    }

    #[test]
    fn an_unreadable_dialog_is_answered_with_esc_only() {
        let u = dialog(DialogKind::Unknown, &[], 0);
        assert_eq!(plan_answer(&u, &u, PRESS_ESC).unwrap(), b"\x1b");
        assert!(plan_answer(&u, &u, "1. Yes").is_err());
    }

    use super::*;

    fn note(from: &str, text: &str) -> Msg {
        Msg {
            from: from.into(),
            text: text.into(),
            ..Default::default()
        }
    }

    fn ticks(msgs: &[Msg]) -> Vec<&str> {
        msgs.iter().map(|m| m.status.as_str()).collect()
    }

    /// Invoke's life of a note: got while the lead is on it, answered after.
    #[test]
    fn a_note_is_got_until_its_turn_is_over() {
        let thread = |extra: Option<Msg>| {
            let mut v = vec![note("me", "old"), note("lead", "ok"), note("me", "new")];
            v.extend(extra);
            v
        };
        // The lead is on it: got; the older note is answered.
        let mut m = thread(None);
        mark_notes(&mut m, "busy", 0);
        assert_eq!(ticks(&m), ["answered", "", "got"]);
        // Stopped on a question: still its task, still got.
        let mut m = thread(None);
        mark_notes(&mut m, "awaiting-input", 0);
        assert_eq!(ticks(&m)[2], "got");
        // One idle read is not the end (a stray read between two spinners).
        let mut m = thread(None);
        mark_notes(&mut m, "idle", 1);
        assert_eq!(ticks(&m)[2], "got");
        // Idle two polls running: the turn is over, answered (even with no text).
        let mut m = thread(None);
        mark_notes(&mut m, "idle", 2);
        assert_eq!(ticks(&m)[2], "answered");
        // A reply after it answers it, whatever the pane reads.
        let mut m = thread(Some(note("lead", "done")));
        mark_notes(&mut m, "busy", 0);
        assert_eq!(ticks(&m)[2], "answered");
        // A failed note stays failed; no notes, nothing to mark.
        let mut m = vec![Msg {
            status: "failed".into(),
            ..note("me", "x")
        }];
        mark_notes(&mut m, "busy", 0);
        assert_eq!(ticks(&m), ["failed"]);
        mark_notes(&mut [], "busy", 0);
    }

    /// Typed into the lead: got. Still waiting for an idle moment: sending.
    #[test]
    fn a_typed_note_is_got_and_a_queued_one_is_sending() {
        let mut q = SendQueue::default();
        q.push("a".into());
        q.push("b".into());
        assert!(!q.first_is_typed());
        assert_eq!(q.tick("idle", 0, 0), Some("a".to_string()));
        assert!(q.first_is_typed());
        assert_eq!(q.pending(), ["a", "b"]);
    }

    fn line(kind: &str, content: &str, extra: &str) -> String {
        format!(
            r#"{{"type":"{kind}","timestamp":"2026-11-07T19:32:39.346Z"{extra},"message":{{"role":"{kind}","content":{content}}}}}"#
        )
    }

    #[test]
    fn typed_prompts_and_answers_become_bubbles() {
        let me = parse_line(0, &line("user", r#""fix the build""#, ""));
        assert_eq!(me.len(), 1);
        assert_eq!(
            (me[0].from.as_str(), me[0].text.as_str()),
            ("me", "fix the build")
        );
        let lead = parse_line(
            100,
            &line("assistant", r#"[{"type":"text","text":"On it."}]"#, ""),
        );
        assert_eq!(
            (lead[0].from.as_str(), lead[0].text.as_str()),
            ("lead", "On it.")
        );
        assert!(lead[0].id > me[0].id, "ids follow file order");
        assert_eq!(me[0].ts, 1_794_079_959_346);
    }

    #[test]
    fn tools_thinking_wrappers_and_side_chains_make_no_bubbles() {
        let none = |l: String| assert!(parse_line(0, &l).is_empty(), "{l}");
        none(line(
            "assistant",
            r#"[{"type":"tool_use","id":"t","name":"Bash","input":{}}]"#,
            "",
        ));
        none(line(
            "assistant",
            r#"[{"type":"thinking","thinking":"hm"}]"#,
            "",
        ));
        none(line(
            "user",
            r#"[{"type":"tool_result","tool_use_id":"t","content":"ok"}]"#,
            "",
        ));
        none(line("user", r#""<command-name>/clear</command-name>""#, ""));
        none(line(
            "user",
            r#""<system-reminder>x</system-reminder>""#,
            "",
        ));
        none(line("user", r#""meta""#, r#","isMeta":true"#));
        none(line(
            "assistant",
            r#"[{"type":"text","text":"side"}]"#,
            r#","isSidechain":true"#,
        ));
        none(r#"{"type":"system","subtype":"turn_duration"}"#.into());
        none("not json".into());
    }

    #[test]
    fn a_half_written_line_waits_for_the_rest() {
        let mut t = LocalThread::default();
        let a = line("user", r#""hi""#, "");
        let b = line("assistant", r#"[{"type":"text","text":"yo"}]"#, "");
        t.feed(0, &format!("{a}\n"));
        assert_eq!(t.msgs.len(), 1);
        assert_eq!(t.users, 1);
        // The daemon only ever hands over whole lines; the offset moves by them.
        assert_eq!(t.offset, a.len() as u64 + 1);
        t.feed(t.offset, &format!("{b}\n"));
        assert_eq!(t.msgs.len(), 2);
        assert_eq!(t.msgs[1].id, (a.len() as i64 + 1) * 16 + 1);
    }

    #[test]
    fn invoke_is_picked_only_for_a_personal_machine_with_a_url() {
        assert!(!pick_local(true, "https://invoke.example"));
        assert!(pick_local(true, ""));
        assert!(pick_local(true, "  \n"));
        assert!(pick_local(false, "https://invoke.example"));
        assert!(pick_local(false, ""));
    }

    #[test]
    fn a_note_goes_in_only_at_an_idle_prompt_and_only_once() {
        let mut q = SendQueue::default();
        q.push("one".into());
        q.push("two".into());
        // Busy and modal panes take nothing.
        assert_eq!(q.tick("busy", 0, 0), None);
        assert_eq!(q.tick("awaiting-input", 0, 0), None);
        assert_eq!(q.pending(), ["one", "two"]);
        // Idle: one goes, and the next poll reading idle again sends nothing.
        assert_eq!(q.tick("idle", 0, 10).as_deref(), Some("one"));
        assert_eq!(q.tick("idle", 0, 20), None);
        assert_eq!(q.pending(), ["one", "two"]);
        // Its line reaches the transcript: the next note may go.
        assert_eq!(q.tick("idle", 1, 30).as_deref(), Some("two"));
        assert_eq!(q.pending(), ["two"]);
    }

    #[test]
    fn a_note_that_never_lands_frees_the_queue_after_a_while() {
        let mut q = SendQueue::default();
        q.push("a".into());
        q.push("b".into());
        assert_eq!(q.tick("idle", 0, 0).as_deref(), Some("a"));
        assert_eq!(q.tick("idle", 0, IN_FLIGHT_MS), None);
        assert_eq!(q.tick("idle", 0, IN_FLIGHT_MS + 1).as_deref(), Some("b"));
    }

    #[test]
    fn claude_starting_to_work_frees_the_latch() {
        let mut q = SendQueue::default();
        q.push("a".into());
        q.push("b".into());
        assert_eq!(q.tick("idle", 0, 0).as_deref(), Some("a"));
        assert_eq!(q.tick("busy", 0, 5), None);
        assert_eq!(q.pending(), ["b"]);
        assert_eq!(q.tick("idle", 1, 9).as_deref(), Some("b"));
    }

    /// With no lead set, against a scratch daemon: the circle's Claude is
    /// found (here typed into a plain pane, no hook), a circle with none is
    /// told so. Needs `SEANCE_SOCKET`, `SEANCE_E2E_WS` (a circle holding the
    /// Claude), `SEANCE_E2E_EXPECT` (its pane), `SEANCE_E2E_EMPTY` (a circle
    /// with only a shell) and `SEANCE_E2E_SESSION` (the Claude's session id).
    #[test]
    #[ignore]
    fn e2e_finds_the_circles_claude_with_no_lead() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
        let (client, _events) = GuiClient::connect_empty().expect("connect");
        let (ws, empty) = (env("SEANCE_E2E_WS"), env("SEANCE_E2E_EMPTY"));
        let ask = |w: &str| LocalAsk {
            client: Arc::clone(&client),
            circles: vec![(w.to_string(), w.to_string(), None, false)],
            ws: Some(w.to_string()),
        };
        let found = client.local_lead(&ws).unwrap();
        eprintln!("local_lead({ws}) = {found:?}");
        assert_eq!(found.as_deref(), Some(env("SEANCE_E2E_EXPECT").as_str()));
        let look = client.local_chat(&found.unwrap(), 0, "").unwrap();
        eprintln!("claude={} session={}", look["claude"], look["session"]);
        assert_eq!(look["claude"], true);
        assert!(look["session"]
            .as_str()
            .unwrap()
            .ends_with(&format!("{}.jsonl", env("SEANCE_E2E_SESSION"))));
        round(ask(&ws)).unwrap();
        assert_eq!(lead_of(&ws), client.local_lead(&ws).unwrap());
        assert!(has_claude(&ws));
        // The circle with only a shell has no one to chat with.
        assert_eq!(client.local_lead(&empty).unwrap(), None);
        round(ask(&empty)).unwrap();
        assert!(!has_claude(&empty));
        assert_eq!(lead_of(&empty), None);
        eprintln!("empty circle says: {NO_LEAD_MESSAGE}");
    }

    /// Claude's first-launch dialog in Chat, against a scratch daemon whose
    /// `claude` fixture prints the trust dialog (no digits) and then reads one
    /// line: the card appears with its options, and a tap answers it with arrows
    /// and Enter. `SEANCE_E2E_WS` names the circle to make.
    #[test]
    #[ignore]
    fn e2e_a_first_launch_dialog_is_answered_from_chat() {
        let (client, _events) = GuiClient::connect_empty().expect("connect");
        let ws = std::env::var("SEANCE_E2E_WS").unwrap();
        client.create_workspace(&ws).unwrap();
        let (pane, _) = client.start_claude(&ws).unwrap();
        let ask = || LocalAsk {
            client: Arc::clone(&client),
            circles: vec![(ws.clone(), ws.clone(), None, false)],
            ws: Some(ws.clone()),
        };
        let mut card = None;
        for _ in 0..40 {
            let r = round(ask()).expect("round");
            let (_, t) = r.thread.expect("thread");
            if let Some(m) = t.messages.iter().find(|m| !m.choices.is_empty()) {
                card = Some(m.clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let card = card.expect("no dialog card appeared");
        eprintln!("card text: {}", card.text.replace('\n', " / "));
        eprintln!("card buttons: {:?}", card.choices);
        let pick = card
            .choices
            .iter()
            .find(|c| c.starts_with("2."))
            .unwrap()
            .clone();
        answer(&client, &ws, &pane, &pick).unwrap();
        eprintln!("answered: {pick}");
        // A second tap meant for the same card, after the dialog is gone, presses nothing.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let _ = round(ask());
        eprintln!(
            "late tap: {:?}",
            answer(&client, &ws, &pane, &pick).map_err(|e| e.to_string())
        );
    }

    /// The sign-in step, against a scratch daemon whose lead is a fixture
    /// Claude that prints Claude's real "Paste code here if prompted >" line
    /// and reads one line back: the card has the code box, the code reaches
    /// the pane, and a second send after the step is gone is refused.
    /// `SEANCE_E2E_WS` names the circle; `SEANCE_E2E_CODE` is the test code.
    #[test]
    #[ignore]
    fn e2e_a_login_code_is_typed_into_the_pane() {
        let (client, _events) = GuiClient::connect_empty().expect("connect");
        let ws = std::env::var("SEANCE_E2E_WS").unwrap();
        let code = std::env::var("SEANCE_E2E_CODE").unwrap();
        client.create_workspace(&ws).unwrap();
        let (pane, _) = client.start_claude(&ws).unwrap();
        let ask = || LocalAsk {
            client: Arc::clone(&client),
            circles: vec![(ws.clone(), ws.clone(), None, false)],
            ws: Some(ws.clone()),
        };
        let mut card = None;
        for _ in 0..40 {
            let r = round(ask()).expect("round");
            let (_, t) = r.thread.expect("thread");
            if let Some(m) = t
                .messages
                .iter()
                .find(|m| m.choices.iter().any(|c| c == CODE_BOX))
            {
                card = Some(m.clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let card = card.expect("no sign-in card appeared");
        eprintln!("card text: {}", card.text.replace('\n', " / "));
        eprintln!("card buttons: {:?}", card.choices);
        eprintln!(
            "sent: {:?}",
            send_code(&client, &ws, &pane, &code).map_err(|e| e.to_string())
        );
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let _ = round(ask());
        eprintln!(
            "second send: {:?}",
            send_code(&client, &ws, &pane, &code).map_err(|e| e.to_string())
        );
    }

    /// "New circles start with Claude", against a scratch daemon: make the
    /// circle `SEANCE_E2E_WS`, start its first pane, and print what the daemon
    /// answered and who Chat would talk to.
    #[test]
    #[ignore]
    fn e2e_new_circle_starts_claude() {
        let (client, _events) = GuiClient::connect_empty().expect("connect");
        let ws = std::env::var("SEANCE_E2E_WS").unwrap();
        client.create_workspace(&ws).unwrap();
        eprintln!("start: {:?}", client.start_claude(&ws).unwrap());
        eprintln!("chat lead: {:?}", client.local_lead(&ws).unwrap());
    }

    /// End to end against a scratch daemon whose lead pane runs a fixture
    /// Claude (see the finish note). Ignored by default: it needs the daemon,
    /// `SEANCE_SOCKET`, and `SEANCE_E2E_LEAD`.
    #[test]
    #[ignore]
    fn e2e_against_a_scratch_daemon() {
        let lead = std::env::var("SEANCE_E2E_LEAD").expect("SEANCE_E2E_LEAD");
        let (client, _events) = GuiClient::connect_empty().expect("connect");
        let ws = "e2e".to_string();
        let ask = || LocalAsk {
            client: Arc::clone(&client),
            circles: vec![(ws.clone(), "e2e".into(), Some(lead.clone()), false)],
            ws: Some(ws.clone()),
        };
        let show = |label: &str, t: &Thread| {
            eprintln!("--- {label} (busy={})", t.lead_busy);
            for m in &t.messages {
                eprintln!("  [{}] {} {:?}", m.from, m.text, m.choices);
            }
        };
        let until = |what: &str, done: &dyn Fn(&Thread) -> bool| -> Thread {
            for _ in 0..40 {
                let r = round(ask()).expect("round");
                let (_, t) = r.thread.expect("thread");
                if done(&t) {
                    show(what, &t);
                    return t;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            panic!("never reached: {what}");
        };
        // Idle fixture, empty conversation.
        let t = until("idle at start", &|t| t.messages.is_empty() && !t.lead_busy);
        assert!(!t.lead_busy);
        // A plain note reaches the pane, lands in the transcript, gets a reply.
        send(&ws, "hello there").unwrap();
        let t = until("first exchange", &|t| {
            t.messages
                .iter()
                .any(|m| m.from == "lead" && m.text == "fixture reply to: hello there")
        });
        assert_eq!(t.messages.iter().filter(|m| m.from == "me").count(), 1);
        // Tool calls and results made no bubbles.
        assert!(t.messages.iter().all(|m| m.text != "ok"));
        // Two notes typed while it works go in one at a time, in order.
        send(&ws, "first").unwrap();
        send(&ws, "second").unwrap();
        let t = until("queued notes", &|t| {
            t.messages.iter().filter(|m| m.from == "lead").count() == 3
        });
        let me: Vec<&str> = t
            .messages
            .iter()
            .filter(|m| m.from == "me")
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(me, ["hello there", "first", "second"]);
        // A permission menu shows as buttons, and a tap presses its digit.
        send(&ws, "ask").unwrap();
        let t = until("question", &|t| {
            t.messages.iter().any(|m| !m.choices.is_empty())
        });
        let q = t.messages.iter().find(|m| !m.choices.is_empty()).unwrap();
        assert_eq!(q.text, "Do you want to proceed?");
        assert_eq!(q.choices.len(), 3);
        assert_eq!(q.choices[0], "1. Yes");
        answer(&client, &ws, &lead, &q.choices[1]).unwrap();
        until("answered", &|t| {
            t.messages.iter().any(|m| m.text == "you pressed 2")
                && t.messages.iter().all(|m| m.choices.is_empty())
        });
    }

    #[test]
    fn the_sign_in_card_has_a_code_box_and_the_terminal_and_no_key_answers() {
        let d = DialogPrompt {
            kind: DialogKind::LoginCode,
            question: "Claude is waiting for the sign-in code.".into(),
            options: vec![],
            selected: 0,
            screen: String::new(),
        };
        let (text, choices) = dialog_card(&d);
        assert!(text.contains("sign in") && text.contains("code"));
        assert_eq!(choices, [CODE_BOX, SHOW_TERMINAL]);
        // No tap on this card presses a key, whatever its label.
        assert!(plan_answer(&d, &d, CODE_BOX).is_err());
        assert!(plan_answer(&d, &d, "1. Yes").is_err());
    }

    #[test]
    fn a_code_is_sent_only_while_the_sign_in_step_is_what_the_card_showed() {
        assert_eq!(
            check_code("  abc#123  ", Some(DialogKind::LoginCode)).unwrap(),
            "abc#123"
        );
        assert!(check_code("", Some(DialogKind::LoginCode)).is_err());
        assert!(check_code("   ", Some(DialogKind::LoginCode)).is_err());
        assert!(check_code("a\nb", Some(DialogKind::LoginCode)).is_err());
        assert!(check_code("abc", None).is_err());
        assert!(check_code("abc", Some(DialogKind::Numbered)).is_err());
    }
}
