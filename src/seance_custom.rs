//! "Seance Custom": the helper circle every install gets once.
//!
//! Its lead is a Claude that knows Seance's settings files (the guide in
//! `assets/seance-custom/CLAUDE.md`, copied to `<config dir>/custom/`). It is
//! created once per install; `custom.json` remembers that, so a circle the
//! user deletes stays deleted. This module holds the pure decisions, the
//! state file, the guide install, and the feature-request URL.
//!
//! Remote daemon: the helper folder lives on the machine running the app, not
//! on the daemon's. So when the GUI is attached to a remote daemon the lead
//! gets no cwd, and `seance-custom-command` in terminal.conf should point at
//! the folder itself, for example
//! `CLAUDE_MAC_DIR=~/.config/seance/custom claude-mac`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

pub const SLUG: &str = "seance-custom";
pub const LABEL: &str = "Seance Custom";
pub const FIRST_MESSAGE: &str = "Help me pick a custom lead reply sound";

/// The helper's instructions, shipped inside the binary. The first line is a
/// version marker, so a changed guide is rewritten on the next launch.
const GUIDE: &str = include_str!("../assets/seance-custom/CLAUDE.md");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Already made once, or nothing to do.
    Nothing,
    /// A circle with the slug exists already: remember it, make nothing.
    MarkCreated,
    /// First time: make the circle.
    Create,
}

/// What the GUI does after the first daemon state.
pub fn decide(created: bool, circle_exists: bool) -> Step {
    match (created, circle_exists) {
        (true, _) => Step::Nothing,
        (false, true) => Step::MarkCreated,
        (false, false) => Step::Create,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRoute {
    OpenHelper,
    FilePicker,
}

/// "Lead Reply Sound > Custom...": the helper when its circle is there, the
/// old file picker when the user deleted it (it is never re-made).
pub fn route(circle_exists: bool) -> MenuRoute {
    if circle_exists {
        MenuRoute::OpenHelper
    } else {
        MenuRoute::FilePicker
    }
}

static OPEN_REQUEST: AtomicBool = AtomicBool::new(false);

/// The menu handler has no app handle: it raises this, the next render acts.
pub fn request_open() {
    OPEN_REQUEST.store(true, Ordering::SeqCst);
}

pub fn take_open_request() -> bool {
    OPEN_REQUEST.swap(false, Ordering::SeqCst)
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    #[serde(default)]
    created: bool,
}

/// `~/.config/seance`, or `$XDG_CONFIG_HOME/seance`.
pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("seance");
        }
    }
    PathBuf::from(shellexpand::tilde("~/.config/seance").as_ref())
}

pub fn helper_dir() -> PathBuf {
    config_dir().join("custom")
}

fn state_path() -> PathBuf {
    config_dir().join("custom.json")
}

pub fn load_created() -> bool {
    load_created_from(&state_path())
}

pub fn save_created() {
    save_created_to(&state_path());
}

fn load_created_from(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<State>(&t).ok())
        .is_some_and(|s| s.created)
}

fn save_created_to(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(json) = serde_json::to_string_pretty(&State { created: true }) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Write the guide into `dir/CLAUDE.md` when it is missing or differs.
fn install_guide_to(dir: &Path, guide: &str) {
    let file = dir.join("CLAUDE.md");
    if std::fs::read_to_string(&file).is_ok_and(|t| t == guide) {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&file, guide);
}

pub fn install_guide() {
    install_guide_to(&helper_dir(), GUIDE);
}

const REQUESTS_URL: &str = "https://github.com/BHanley/seance-requests/issues/new";

/// Percent-encode UTF-8 bytes; only `A-Za-z0-9-_.~` stay as they are.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The prefilled "new issue" page for a feature request.
pub fn request_url(title: &str, body: &str) -> String {
    format!(
        "{REQUESTS_URL}?template=feature-request.md&title={}&body={}",
        encode_component(title),
        encode_component(body)
    )
}

/// `seance feature-request --title T --body B`: print the URL and open it.
pub fn run_feature_request(args: &[String]) -> i32 {
    let mut title = None;
    let mut body = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--title" => title = it.next().cloned(),
            "--body" => body = it.next().cloned(),
            "--help" | "-h" => {
                println!("USAGE: seance feature-request --title TITLE --body BODY");
                return 0;
            }
            _ => {}
        }
    }
    let (Some(title), Some(body)) = (title, body) else {
        eprintln!("usage: seance feature-request --title TITLE --body BODY");
        return 2;
    };
    let url = request_url(&title, &body);
    println!("{url}");
    crate::sysopen::open_blocking(&url);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("seance-custom-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn created_once_and_never_again() {
        assert_eq!(decide(false, false), Step::Create);
        // The user deleted it later: flag set, circle gone.
        assert_eq!(decide(true, false), Step::Nothing);
        assert_eq!(decide(true, true), Step::Nothing);
    }

    #[test]
    fn an_existing_circle_only_marks() {
        assert_eq!(decide(false, true), Step::MarkCreated);
    }

    #[test]
    fn custom_routes_to_the_helper_or_the_picker() {
        assert_eq!(route(true), MenuRoute::OpenHelper);
        assert_eq!(route(false), MenuRoute::FilePicker);
    }

    #[test]
    fn the_open_request_is_taken_once() {
        let _ = take_open_request();
        assert!(!take_open_request());
        request_open();
        assert!(take_open_request());
        assert!(!take_open_request());
    }

    #[test]
    fn state_file_roundtrip() {
        let dir = tmp("state");
        let p = dir.join("custom.json");
        assert!(!load_created_from(&p));
        save_created_to(&p);
        assert!(load_created_from(&p));
        std::fs::write(&p, "junk").unwrap();
        assert!(!load_created_from(&p));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_guide_is_written_and_refreshed() {
        let dir = tmp("guide");
        install_guide_to(&dir, "v1");
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "v1"
        );
        install_guide_to(&dir, "v2");
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "v2"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_shipped_guide_has_a_version_marker_and_no_em_dash() {
        assert!(GUIDE.starts_with("<!-- seance-custom guide v"));
        assert!(!GUIDE.contains('\u{2014}'));
    }

    #[test]
    fn encoding_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(encode_component("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(encode_component("a b"), "a%20b");
        assert_eq!(encode_component("a&b#c?d=e"), "a%26b%23c%3Fd%3De");
        assert_eq!(encode_component("l1\nl2"), "l1%0Al2");
        assert_eq!(encode_component("\u{e9}"), "%C3%A9");
        assert_eq!(encode_component("\u{1F600}"), "%F0%9F%98%80");
    }

    #[test]
    fn the_url_carries_template_title_and_body() {
        let u = request_url("A & B", "why?\nnow");
        assert_eq!(
            u,
            "https://github.com/BHanley/seance-requests/issues/new?template=feature-request.md&title=A%20%26%20B&body=why%3F%0Anow"
        );
    }
}
