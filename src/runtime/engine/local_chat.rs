//! The daemon half of the Chat layout without a server: where a lead's Claude
//! conversation file is, what its screen says it is doing, and the new bytes
//! of the file since the client last looked. Turning those bytes into bubbles
//! is the client's job (`chat_local.rs`).

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use base64::Engine as _;
use serde_json::json;

use super::agent_session::{claude_transcript, Agent};
use super::sleep::{hosted_record, probe_hosted, resolve_hosted, title_is_claude};
use super::Engine;

/// A first read of a long conversation starts this far from the end.
const TAIL_BYTES: u64 = 256 * 1024;
/// One reply carries at most this much, cut at a whole line.
const CHUNK_BYTES: u64 = 1024 * 1024;

/// What the engine knows about one lead, gathered under its lock.
pub(crate) struct LocalChatLook {
    pub claude: bool,
    pub path: Option<PathBuf>,
    pub activity: String,
    pub evidence: Option<String>,
    pub prompt: Option<seance_core::agent_state::DialogPrompt>,
    /// The daemon's own busy flag for the pane (the title spinner, flipped
    /// only on an edge) and when it last flipped, in ms. Chat reads a
    /// finished turn off this edge, not off polled screen reads.
    pub busy: bool,
    pub busy_at: u64,
}

/// Who Chat talks to in a circle: a lead that was set wins (Brian's own
/// setup, as before); otherwise the Claude in the circle used most recently,
/// the earliest-listed one on a tie. `claudes` is (pane, last used ms) for
/// every pane that is a Claude, agent or hosted.
pub(crate) fn pick_local_lead(set: Option<&str>, claudes: &[(String, u64)]) -> Option<String> {
    if let Some(l) = set {
        return Some(l.to_string());
    }
    claudes
        .iter()
        .enumerate()
        .max_by_key(|(i, (_, used))| (*used, std::cmp::Reverse(*i)))
        .map(|(_, (slug, _))| slug.clone())
}

impl Engine {
    /// One line typed to every circle's lead (AFK turning on). A lead that is
    /// asleep has no session and is skipped. Returns how many were told.
    pub(crate) fn tell_leads(&mut self, text: &str) -> usize {
        let mut circles: Vec<String> = self.panes.iter().map(|p| p.workspace.clone()).collect();
        circles.sort();
        circles.dedup();
        let mut told = 0;
        for circle in circles {
            let Some(lead) = self.local_lead(&circle) else {
                continue;
            };
            let Some(pane) = self.panes.iter().find(|p| p.slug == lead) else {
                continue;
            };
            if let Some(session) = pane.session.as_ref() {
                session.inject(text.to_string(), true);
                told += 1;
            }
        }
        told
    }

    /// Every circle's `working` and `waiting`, by `seance_core::circle_state`
    /// (the rule Invoke's list uses), from the same pane rows `ctl list` shows.
    pub(crate) fn circle_states(&self) -> serde_json::Value {
        use seance_core::circle_state::{self as rule, PaneFact};
        let mut circles: Vec<&str> = self.panes.iter().map(|p| p.workspace.as_str()).collect();
        circles.sort();
        circles.dedup();
        let mut out = serde_json::Map::new();
        for ws in circles {
            // (slug, activity, task_status) of every pane but file panes.
            let rows: Vec<(String, String, Option<String>)> = self
                .panes
                .iter()
                .filter(|p| p.workspace == ws && p.kind != "file")
                .map(|p| {
                    let r = self.pane_summary_json(p);
                    (
                        p.slug.clone(),
                        r["activity"].as_str().unwrap_or("unknown").to_string(),
                        r["task_status"].as_str().map(str::to_string),
                    )
                })
                .collect();
            let facts: Vec<PaneFact> = rows
                .iter()
                .map(|(slug, activity, task)| PaneFact {
                    slug,
                    activity,
                    task_status: task.as_deref(),
                })
                .collect();
            // Who the lead is costs a screen read per pane: only ask when
            // some pane could be waiting.
            let maybe = facts
                .iter()
                .any(|p| p.task_status == Some("open") && p.activity == "idle");
            let lead = maybe.then(|| self.local_lead(ws)).flatten();
            out.insert(
                ws.to_string(),
                json!({
                    "working": rule::working(&facts),
                    "waiting": rule::waiting(lead.as_deref(), &facts),
                }),
            );
        }
        json!({ "circles": out })
    }

    /// The pane Chat uses for `circle` (see [`pick_local_lead`]).
    pub(crate) fn local_lead(&self, circle: &str) -> Option<String> {
        let set = self.comms.leads.get(circle).filter(|l| {
            self.panes
                .iter()
                .any(|p| &p.slug == *l && p.workspace == circle)
        });
        let claudes: Vec<(String, u64)> = self
            .panes
            .iter()
            .filter(|p| p.workspace == circle && p.kind != "file")
            .filter(|p| self.local_chat_look(&p.slug).claude)
            .map(|p| {
                (
                    p.slug.clone(),
                    self.pane_busy_at.get(&p.slug).copied().unwrap_or(0),
                )
            })
            .collect();
        pick_local_lead(set.map(String::as_str), &claudes)
    }

    pub(crate) fn local_chat_look(&self, pane: &str) -> LocalChatLook {
        let none = LocalChatLook {
            claude: false,
            path: None,
            activity: "unknown".into(),
            evidence: None,
            prompt: None,
            busy: self.pane_busy.contains(pane),
            busy_at: self.pane_busy_at.get(pane).copied().unwrap_or(0),
        };
        let Some(p) = self.panes.iter().find(|p| p.slug == pane) else {
            return none;
        };
        if p.kind == "file" {
            return none;
        }
        // A Claude agent pane has its conversation id; one typed into a plain
        // pane is found from its hook record or Claude's own process record.
        let path = match (Agent::of(&p.command), p.agent_session.as_deref()) {
            (Some(Agent::Claude), Some(session)) => Some(claude_transcript(&p.cwd, session)),
            // Awake, it must still look like Claude (a stale record of one
            // that exited doesn't count); asleep, the record is all there is.
            (None, _)
                if p.asleep
                    || p.session
                        .as_ref()
                        .is_some_and(|s| title_is_claude(s.title().as_deref())) =>
            {
                resolve_hosted(hosted_record(&p.pane_id), || probe_hosted(p, false))
                    .map(|(id, cwd)| claude_transcript(&cwd, &id))
            }
            _ => None,
        };
        if path.is_none() {
            return none;
        }
        let Some(s) = p.session.as_ref().filter(|_| !p.asleep) else {
            return LocalChatLook {
                claude: true,
                path,
                activity: "asleep".into(),
                ..none
            };
        };
        let screen = s.live_screen_text();
        let state = seance_core::agent_state::classify(&screen, s.title().as_deref());
        let awaiting = state.activity == seance_core::agent_state::Activity::AwaitingInput;
        LocalChatLook {
            claude: true,
            path,
            activity: state.activity.as_str().into(),
            evidence: state.evidence,
            // A dialog that is not in the transcript: a first-launch picker, the
            // trust question, a permission prompt.
            prompt: seance_core::agent_state::dialog_prompt(&screen, awaiting),
            ..none
        }
    }
}

/// The `LocalChat` answer: the look, plus the file's new whole lines past
/// `offset`. Reads the file, so it runs without the engine lock.
pub(crate) fn local_chat_reply(
    look: LocalChatLook,
    offset: u64,
    session: &str,
) -> serde_json::Value {
    let mut out = json!({
        "claude": look.claude,
        "activity": look.activity,
        "evidence": look.evidence,
        "prompt": look.prompt,
        "busy": look.busy,
        "busy_at": look.busy_at,
        "session": "",
        "size": 0, "from": 0, "to": 0, "reset": false, "b64": "",
    });
    let Some(path) = look.path else {
        return out;
    };
    let id = path.to_string_lossy().to_string();
    out["session"] = json!(id);
    let Ok(mut f) = std::fs::File::open(&path) else {
        // Claude writes the file with the first prompt; until then it is empty.
        out["reset"] = json!(session != id);
        return out;
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let (from, reset) = read_start(size, offset, session == id);
    let Some(chunk) = read_whole_lines(&mut f, from, size, reset) else {
        return out;
    };
    out["size"] = json!(size);
    out["from"] = json!(chunk.from);
    out["to"] = json!(chunk.from + chunk.bytes.len() as u64);
    out["reset"] = json!(reset);
    out["b64"] = json!(base64::engine::general_purpose::STANDARD.encode(&chunk.bytes));
    out
}

/// Where this read starts, and whether the client must start over: a new
/// file, or one that shrank, begins again from the tail.
fn read_start(size: u64, offset: u64, same_file: bool) -> (u64, bool) {
    if same_file && offset <= size {
        (offset, false)
    } else {
        (size.saturating_sub(TAIL_BYTES), true)
    }
}

struct Chunk {
    from: u64,
    bytes: Vec<u8>,
}

/// Bytes `from..size` cut back to the last newline. A start inside a line
/// (the tail of a long file) skips to the next line first.
fn read_whole_lines(
    f: &mut std::fs::File,
    mut from: u64,
    size: u64,
    mid_file_start: bool,
) -> Option<Chunk> {
    let want = (size.saturating_sub(from)).min(CHUNK_BYTES);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = vec![0u8; want as usize];
    f.read_exact(&mut buf).ok()?;
    if mid_file_start && from > 0 {
        let skip = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        buf.drain(..skip);
        from += skip as u64;
    }
    let end = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    buf.truncate(end);
    Some(Chunk { from, bytes: buf })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn claudes(rows: &[(&str, u64)]) -> Vec<(String, u64)> {
        rows.iter().map(|(s, t)| (s.to_string(), *t)).collect()
    }

    #[test]
    fn with_no_lead_chat_uses_the_circles_own_claude() {
        // No Claude in the circle: nobody to talk to.
        assert_eq!(pick_local_lead(None, &[]), None);
        // One Claude: that one.
        assert_eq!(
            pick_local_lead(None, &claudes(&[("cl", 0)])).as_deref(),
            Some("cl")
        );
        // Several: the most recently used; the first listed on a tie.
        let several = claudes(&[("a", 10), ("b", 30), ("c", 20)]);
        assert_eq!(pick_local_lead(None, &several).as_deref(), Some("b"));
        let tied = claudes(&[("a", 5), ("b", 5)]);
        assert_eq!(pick_local_lead(None, &tied).as_deref(), Some("a"));
    }

    #[test]
    fn a_lead_that_was_set_wins() {
        let several = claudes(&[("a", 10), ("b", 30)]);
        assert_eq!(pick_local_lead(Some("a"), &several).as_deref(), Some("a"));
        // Even a lead that is not a Claude, exactly as before: the round then
        // says there is no Claude to read.
        assert_eq!(pick_local_lead(Some("sh"), &[]).as_deref(), Some("sh"));
    }

    fn look(path: PathBuf) -> LocalChatLook {
        LocalChatLook {
            claude: true,
            path: Some(path),
            activity: "idle".into(),
            evidence: None,
            prompt: None,
            busy: false,
            busy_at: 0,
        }
    }

    #[test]
    fn the_reply_carries_the_busy_edge() {
        let mut l = look(PathBuf::from("/nonexistent"));
        l.busy = true;
        l.busy_at = 1234;
        let v = local_chat_reply(l, 0, "");
        assert_eq!(v["busy"], true);
        assert_eq!(v["busy_at"], 1234);
    }

    fn decode(v: &serde_json::Value) -> String {
        let b = base64::engine::general_purpose::STANDARD
            .decode(v["b64"].as_str().unwrap())
            .unwrap();
        String::from_utf8(b).unwrap()
    }

    #[test]
    fn a_read_returns_whole_lines_and_the_next_one_continues() {
        let dir = std::env::temp_dir().join(format!("seance-lc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(&path, "{\"a\":1}\n{\"b\":2}\n{\"c\"").unwrap();
        let id = path.to_string_lossy().to_string();
        let first = local_chat_reply(look(path.clone()), 0, "");
        // Fresh read: a reset, whole lines only (the half line waits).
        assert_eq!(first["reset"], true);
        assert_eq!(decode(&first), "{\"a\":1}\n{\"b\":2}\n");
        let to = first["to"].as_u64().unwrap();
        // Nothing new yet.
        let quiet = local_chat_reply(look(path.clone()), to, &id);
        assert_eq!(
            (quiet["reset"].clone(), decode(&quiet)),
            (false.into(), "".into())
        );
        // The line completes: only it comes back.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b":3}\n").unwrap();
        let next = local_chat_reply(look(path.clone()), to, &id);
        assert_eq!(decode(&next), "{\"c\":3}\n");
        assert_eq!(next["from"].as_u64().unwrap(), to);
        // A shrunken file starts over.
        std::fs::write(&path, "{\"z\":0}\n").unwrap();
        let shrunk = local_chat_reply(look(path), next["to"].as_u64().unwrap(), &id);
        assert_eq!(shrunk["reset"], true);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_pane_with_no_claude_says_so() {
        let (mut eng, _rx) = Engine::bare_for_test(std::env::temp_dir().join("seance-lc-none"));
        let slug = eng.push_stub_pane("shell", "main");
        let l = eng.local_chat_look(&slug);
        assert!(!l.claude);
        assert_eq!(local_chat_reply(l, 0, "")["claude"], false);
    }
}
