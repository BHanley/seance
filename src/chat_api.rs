//! Invoke's HTTP API, for the Chat layout: the same threads the phone app
//! reads, so a chat started on either continues on the other.
//!
//! Tailscale Serve in front of Invoke adds the caller's identity, so there is
//! no token. Every call blocks, so callers run it off the UI thread. It goes
//! through the system `curl` (TLS and the tailnet's certs included) rather
//! than a new HTTP crate: gpui's own client is a stub until an app installs one.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context as _, Result};
use serde::{de::DeserializeOwned, Deserialize};

const DEFAULT_BASE: &str = "https://xps.tail5c9413.ts.net:8443";
/// Plain requests give up here; the long-poll gets its own, longer limit.
const REQUEST_SECS: u64 = 10;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Circles {
    #[serde(default)]
    pub circles: Vec<CircleRow>,
    #[serde(default)]
    pub v: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct CircleRow {
    pub ws: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub unread: u32,
    #[serde(default)]
    pub working: bool,
    #[serde(default)]
    pub last: Option<Msg>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Msg {
    #[serde(default)]
    pub id: i64,
    /// "me", "lead", or anything else (shown as a system note).
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub ts: u64,
    /// File name under `/api/file/`: the first picture of the note.
    #[serde(default)]
    pub image: Option<String>,
    /// Every picture of a note that has more than one (`image` is the first).
    #[serde(default)]
    pub images: Vec<String>,
}

impl Msg {
    /// The names of all the message's pictures, in order.
    pub fn image_names(&self) -> Vec<&str> {
        if self.images.is_empty() {
            self.image.as_deref().into_iter().collect()
        } else {
            self.images.iter().map(String::as_str).collect()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Thread {
    #[serde(default)]
    pub messages: Vec<Msg>,
    /// Highest message id already read.
    #[serde(default)]
    pub read: i64,
    /// Claude plan usage, which every thread answer carries.
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub five_hour: Option<UsageWindow>,
    #[serde(default)]
    pub seven_day: Option<UsageWindow>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct UsageWindow {
    /// Percent of the window used.
    #[serde(default)]
    pub used: f64,
    /// When the window resets, unix ms.
    #[serde(default)]
    pub resets: Option<u64>,
}

/// `/api/tasks`: only what the thread header shows.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Progress {
    #[serde(default)]
    pub busy: bool,
    /// The lead's progress line while it works.
    #[serde(default)]
    pub doing: Option<String>,
}

/// `/api/tasks`: what the phone's to-do list shows for a circle's lead.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Tasks {
    /// The lead's name; none when the circle has no lead.
    #[serde(default)]
    pub lead: Option<String>,
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub doing: Option<String>,
    #[serde(default)]
    pub now: Vec<TaskItem>,
    #[serde(default)]
    pub waiting: Vec<TaskItem>,
    #[serde(default)]
    pub parked: Vec<TaskItem>,
    #[serde(default)]
    pub done: Vec<TaskItem>,
    /// Other panes in the circle that are working.
    #[serde(default)]
    pub helpers: Vec<HelperItem>,
    /// The lead's background agents (reviews and the like).
    #[serde(default)]
    pub agents: Vec<AgentItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct TaskItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub status: String,
    /// Brian's words.
    #[serde(default)]
    pub text: String,
    /// Sent from the phone (else from a pane).
    #[serde(default)]
    pub phone: bool,
    #[serde(default)]
    pub start: Option<u64>,
    #[serde(default)]
    pub end: Option<u64>,
    /// The lead's own name for the task.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct HelperItem {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub doing: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AgentItem {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub doing: Option<String>,
    /// How long it has been running, as Invoke words it ("5 min").
    #[serde(default, rename = "for")]
    pub running: String,
}

#[derive(Deserialize)]
struct Poll {
    #[serde(default)]
    v: u64,
}

fn base() -> String {
    std::env::var("SEANCE_INVOKE_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
}

/// Percent-encode a query value (circle slugs are plain, but be safe).
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `scheme://host[:port]` of the base URL: what Invoke's POST handler wants
/// in `Origin` (it refuses a request without it, 403).
fn origin(base: &str) -> String {
    match base.split_once("://") {
        Some((scheme, rest)) => {
            format!("{scheme}://{}", rest.split('/').next().unwrap_or(rest))
        }
        None => base.to_string(),
    }
}

fn curl(path: &str, secs: u64, post: Option<&serde_json::Value>) -> Result<Vec<u8>> {
    let mut cmd = Command::new("curl");
    // --fail-with-body: a refusal still hands back Invoke's {"error": ...}.
    cmd.args(["-sS", "--fail-with-body", "-m", &secs.to_string()]);
    if post.is_some() {
        cmd.args([
            "-X",
            "POST",
            "-H",
            "content-type: application/json",
            "-H",
            &format!("Origin: {}", origin(&base())),
            "--data-binary",
            "@-",
        ]);
    }
    cmd.arg(format!("{}{path}", base()))
        .stdin(if post.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().context("run curl")?;
    if let (Some(body), Some(mut stdin)) = (post, child.stdin.take()) {
        stdin.write_all(body.to_string().as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        if let Some(said) = refusal(&out.stdout) {
            return Err(anyhow!("{said}"));
        }
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("{}", err.trim()));
    }
    Ok(out.stdout)
}

/// What Invoke said when it refused a request: its `{"error": ...}` text.
fn refusal(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("error")?.as_str().map(str::to_string)
}

fn get<T: DeserializeOwned>(path: &str, secs: u64) -> Result<T> {
    let bytes = curl(path, secs, None)?;
    serde_json::from_slice(&bytes).with_context(|| format!("read {path}"))
}

pub fn circles() -> Result<Circles> {
    get("/api/circles", REQUEST_SECS)
}

/// The circle's thread; with `after`, only the messages past that id.
pub fn thread(ws: &str, after: Option<i64>) -> Result<Thread> {
    let after = after.map_or(String::new(), |id| format!("&after={id}"));
    get(&format!("/api/thread?ws={}{after}", enc(ws)), REQUEST_SECS)
}

pub fn progress(ws: &str) -> Result<Progress> {
    get(&format!("/api/tasks?ws={}", enc(ws)), REQUEST_SECS)
}

/// The whole to-do list of a circle's lead.
pub fn tasks(ws: &str) -> Result<Tasks> {
    get(&format!("/api/tasks?ws={}", enc(ws)), REQUEST_SECS)
}

/// Blocks until Invoke's change counter moves past `v`, or its own wait of
/// `wait` seconds runs out. Returns the counter either way. curl's limit sits
/// past the server's wait, or every quiet poll would end in a timeout.
pub fn poll(v: u64, wait: u64) -> Result<u64> {
    Ok(get::<Poll>(&format!("/api/poll?v={v}"), wait + 10)?.v)
}

pub fn send(ws: &str, text: &str) -> Result<()> {
    curl(
        "/api/send",
        REQUEST_SECS,
        Some(&serde_json::json!({ "ws": ws, "text": text })),
    )
    .map(|_| ())
}

/// Send a parked task back to the circle's lead as Brian's message and close
/// the parked copy. Invoke refuses with "not a parked task" for anything else.
pub fn resume(ws: &str, id: &str) -> Result<()> {
    curl(
        "/api/resume",
        REQUEST_SECS,
        Some(&serde_json::json!({ "ws": ws, "id": id })),
    )
    .map(|_| ())
}

/// Close a parked task without doing it.
pub fn dismiss(ws: &str, id: &str) -> Result<()> {
    curl(
        "/api/dismiss",
        REQUEST_SECS,
        Some(&serde_json::json!({ "ws": ws, "id": id })),
    )
    .map(|_| ())
}

pub fn mark_read(ws: &str, id: i64) -> Result<()> {
    curl(
        "/api/read",
        REQUEST_SECS,
        Some(&serde_json::json!({ "ws": ws, "id": id })),
    )
    .map(|_| ())
}

/// An attached image, downloaded once into the cache; its local path.
pub fn file(name: &str) -> Result<PathBuf> {
    // The name goes into a path: keep only its last component.
    let leaf = name.rsplit('/').next().unwrap_or(name);
    if leaf.is_empty() || leaf.starts_with('.') {
        return Err(anyhow!("bad file name"));
    }
    let dir = PathBuf::from(shellexpand::tilde("~/.cache/seance/chat").as_ref());
    let path = dir.join(leaf);
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    let bytes = curl(&format!("/api/file/{}", enc(leaf)), 30, None)?;
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Pixel size of a picture, read from its header: PNG, JPEG, GIF, WebP and BMP.
/// None for anything else (HEIC and the like) or a file that cannot be read.
pub fn image_size(path: &std::path::Path) -> Option<(u32, u32)> {
    use std::io::Read as _;
    let mut head = Vec::new();
    // A JPEG's size can sit behind a long EXIF block; a megabyte reaches it.
    std::fs::File::open(path)
        .ok()?
        .take(1 << 20)
        .read_to_end(&mut head)
        .ok()?;
    size_from_header(&head)
}

fn size_from_header(b: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| {
        b.get(at..at + 4)
            .map(|x| u32::from_be_bytes(x.try_into().unwrap()))
    };
    let le16 = |at: usize| {
        b.get(at..at + 2)
            .map(|x| u32::from(u16::from_le_bytes(x.try_into().unwrap())))
    };
    let le24 = |at: usize| {
        b.get(at..at + 3)
            .map(|x| u32::from(x[0]) | u32::from(x[1]) << 8 | u32::from(x[2]) << 16)
    };
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(b"BM") {
        let h = b
            .get(22..26)
            .map(|x| i32::from_le_bytes(x.try_into().unwrap()))?;
        return Some((
            b.get(18..22)
                .map(|x| u32::from_le_bytes(x.try_into().unwrap()))?,
            h.unsigned_abs(),
        ));
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return match b.get(12..16)? {
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            _ => None,
        };
    }
    if b.starts_with(&[0xff, 0xd8]) {
        let mut at = 2;
        while at + 4 <= b.len() {
            if b[at] != 0xff {
                return None;
            }
            let marker = b[at + 1];
            if marker == 0xff {
                at += 1;
                continue;
            }
            let len = usize::from(u16::from_be_bytes([b[at + 2], b[at + 3]]));
            // The start-of-frame markers carry the size (not DHT, JPG or DAC).
            if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                let h = u32::from(u16::from_be_bytes([*b.get(at + 5)?, *b.get(at + 6)?]));
                let w = u32::from(u16::from_be_bytes([*b.get(at + 7)?, *b.get(at + 8)?]));
                return Some((w, h));
            }
            at += 2 + len;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_sizes_come_from_the_header() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend([0, 0, 3, 0x20, 0, 0, 6, 0x40]);
        assert_eq!(size_from_header(&png), Some((800, 1600)));
        assert_eq!(size_from_header(b"GIF89a\x10\x00\x20\x00"), Some((16, 32)));
        // A JPEG: SOI, an APP0 segment to skip, then SOF0 (height 1200, width 900).
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x04,
            0xb0, 0x03, 0x84, 0x01, 0x01, 0x11, 0x00,
        ];
        assert_eq!(size_from_header(&jpeg), Some((900, 1200)));
        // WebP, extended header: 24-bit width-1 and height-1.
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend([0x1f, 0x03, 0x00, 0xff, 0x01, 0x00]);
        assert_eq!(size_from_header(&webp), Some((800, 512)));
        assert_eq!(size_from_header(b"not a picture"), None);
        assert_eq!(size_from_header(b"\x89PNG"), None);
    }

    #[test]
    fn a_note_with_several_pictures_lists_them_all() {
        let one = Msg {
            image: Some("a.png".into()),
            ..Default::default()
        };
        assert_eq!(one.image_names(), ["a.png"]);
        let many: Msg =
            serde_json::from_str(r#"{"image":"a.png","images":["a.png","b.png"]}"#).unwrap();
        assert_eq!(many.image_names(), ["a.png", "b.png"]);
        assert!(Msg::default().image_names().is_empty());
    }

    #[test]
    fn circles_parse_with_a_missing_last_message() {
        let c: Circles = serde_json::from_str(
            r#"{"circles":[{"ws":"main","name":"PM","unread":2,"working":true,
            "last":{"from":"lead","text":"hi","ts":5,"image":null}},
            {"ws":"b","name":"B","last":null}],"v":20,"afk":{"on":true}}"#,
        )
        .unwrap();
        assert_eq!(c.v, 20);
        assert_eq!(c.circles[0].unread, 2);
        assert_eq!(c.circles[0].last.as_ref().unwrap().text, "hi");
        assert!(c.circles[1].last.is_none() && !c.circles[1].working);
    }

    #[test]
    fn thread_ignores_fields_it_does_not_show() {
        let t: Thread = serde_json::from_str(
            r#"{"messages":[{"id":3,"ws":"main","from":"me","ts":9,"status":"answered","mode":"","text":"yes"}],
            "read":3,"working":false,"usage":{}}"#,
        )
        .unwrap();
        assert_eq!(t.read, 3);
        assert_eq!(t.usage, Some(Usage::default()));
        assert_eq!(t.messages[0].from, "me");
        assert_eq!(t.messages[0].image, None);
    }

    #[test]
    fn tasks_parse_names_helpers_and_agents() {
        let t: Tasks = serde_json::from_str(
            r#"{"lead":"L","busy":true,"doing":"✻ Working","now":[{"id":"task-1","status":"open",
            "text":"fix it","phone":true,"start":5,"end":null,"name":"Fix the thing"}],
            "waiting":[],"parked":[{"id":"task-2","status":"parked","text":"x"}],
            "helpers":[{"name":"MacBuilder","doing":null,"task":"build it"}],
            "agents":[{"name":"review","doing":"reading","for":"5 min"}],
            "done":[{"id":"task-3","status":"done","text":"y","phone":false,"start":1,"end":2,"name":null}]}"#,
        )
        .unwrap();
        assert_eq!(t.now[0].name.as_deref(), Some("Fix the thing"));
        assert_eq!(t.agents[0].running, "5 min");
        assert_eq!(t.helpers[0].task.as_deref(), Some("build it"));
        assert_eq!(t.parked[0].start, None);
        assert_eq!(t.done[0].end, Some(2));
    }

    #[test]
    fn a_refusal_reads_back_as_its_error_text() {
        assert_eq!(
            refusal(br#"{"error": "not a parked task"}"#).as_deref(),
            Some("not a parked task")
        );
        assert_eq!(refusal(b"<html>nope</html>"), None);
        assert_eq!(refusal(br#"{"message": "ok"}"#), None);
    }

    #[test]
    fn origin_drops_the_path() {
        assert_eq!(
            origin("https://xps.tail5c9413.ts.net:8443"),
            "https://xps.tail5c9413.ts.net:8443"
        );
        assert_eq!(
            origin("http://localhost:9000/invoke/"),
            "http://localhost:9000"
        );
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(enc("l6-leveling"), "l6-leveling");
        assert_eq!(enc("a b&c"), "a%20b%26c");
    }

    #[test]
    fn file_names_cannot_leave_the_cache() {
        assert!(file("").is_err());
        assert!(file("../").is_err());
        assert!(file("x/.hidden").is_err());
    }
}
