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
}

static STATE: Mutex<Option<HashMap<String, WsState>>> = Mutex::new(None);
static CIRCLES_SIG: Mutex<u64> = Mutex::new(0);

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
    let rows: Vec<CircleRow> = ask
        .circles
        .iter()
        .map(|(ws, name, _, working)| CircleRow {
            ws: ws.clone(),
            name: name.clone(),
            working: *working,
            ..Default::default()
        })
        .collect();
    let circles_sig = hash_of(&[&|h| {
        for r in &rows {
            (&r.ws, &r.name, r.working).hash(h);
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
    if let Some(text) = st.queue.tick(&activity, st.thread.users, now_ms()) {
        // Same path as typing in the pane: pasted, then Enter.
        if ask.client.inject(&lead, &text, true).is_err() {
            st.queue.waiting_front(text);
        }
    }
    let prompt = v["prompt"].as_object().map(|p| {
        let question = p["question"].as_str().unwrap_or_default().to_string();
        let options: Vec<String> = p["options"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| format!("{}. {}", o["n"], o["label"].as_str().unwrap_or_default()))
            .collect();
        (question, options)
    });
    let key = prompt.as_ref().map(|(q, o)| format!("{q}|{}", o.join("|")));
    if key.is_some() && key != st.prompt_key {
        st.prompts_seen += 1;
    }
    st.prompt_key = key;
    let mut messages = st.thread.msgs.clone();
    for (i, text) in st.queue.pending().into_iter().enumerate() {
        messages.push(Msg {
            id: PENDING_BASE + i as i64,
            from: "me".into(),
            text: text.to_string(),
            ts: now_ms(),
            status: "sending".into(),
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
    let sig = hash_of(&[&|h| (messages.len(), newest, &activity, busy).hash(h)]);
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

/// A tapped option ("2. Yes, and don't ask again"): press its digit.
pub fn answer(client: &GuiClient, lead: &str, choice: &str) -> Result<()> {
    let digit = choice
        .chars()
        .next()
        .filter(char::is_ascii_digit)
        .ok_or_else(|| anyhow!("not an option"))?;
    client.input(lead, &[digit as u8])
}

/// Esc: Claude stops its turn.
pub fn stop(client: &GuiClient, lead: &str) -> Result<()> {
    client.input(lead, b"\x1b")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        answer(&client, &lead, &q.choices[1]).unwrap();
        until("answered", &|t| {
            t.messages.iter().any(|m| m.text == "you pressed 2")
                && t.messages.iter().all(|m| m.choices.is_empty())
        });
    }
}
