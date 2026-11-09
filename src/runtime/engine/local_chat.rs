//! The daemon half of the Chat layout without a server: where a lead's Claude
//! conversation file is, what its screen says it is doing, and the new bytes
//! of the file since the client last looked. Turning those bytes into bubbles
//! is the client's job (`chat_local.rs`).

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use base64::Engine as _;
use serde_json::json;

use super::agent_session::{claude_transcript, Agent};
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
    pub prompt: Option<seance_core::agent_state::NumberedPrompt>,
}

impl Engine {
    pub(crate) fn local_chat_look(&self, pane: &str) -> LocalChatLook {
        let none = LocalChatLook {
            claude: false,
            path: None,
            activity: "unknown".into(),
            evidence: None,
            prompt: None,
        };
        let Some(p) = self.panes.iter().find(|p| p.slug == pane) else {
            return none;
        };
        if p.kind == "file" || Agent::of(&p.command) != Some(Agent::Claude) {
            return none;
        }
        let Some(session) = p.agent_session.as_deref() else {
            return none;
        };
        let path = Some(claude_transcript(&p.cwd, session));
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
            prompt: awaiting
                .then(|| seance_core::agent_state::numbered_options(&screen))
                .flatten(),
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
        "prompt": look.prompt.as_ref().map(|p| json!({
            "question": p.question,
            "options": p.options.iter().map(|(n, l)| json!({ "n": n, "label": l })).collect::<Vec<_>>(),
        })),
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

    fn look(path: PathBuf) -> LocalChatLook {
        LocalChatLook {
            claude: true,
            path: Some(path),
            activity: "idle".into(),
            evidence: None,
            prompt: None,
        }
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
