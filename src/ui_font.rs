//! The "Font" menu: which family paints the UI (Chat, sidebar, menus).
//!
//! The pick is per device, saved in `~/.config/seance/font.json` as
//! `{"font":"<id>"}`. It sits beside `theme.json`'s scheme but not in it: the
//! daemon owns that file and a remote daemon would make the font per host, not
//! per device. Terminals keep `mono_font_family`; only `theme.font_family`
//! moves. Four families ship inside the app (SIL OFL, licenses in
//! `assets/fonts`) so the menu works on a Mac that has none of them.
//!
//! Font hygiene (see `theme::init`): a family is only ever set when it
//! resolves; anything else is System, the platform default, untouched.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One menu row.
pub struct Font {
    pub id: &'static str,
    pub label: &'static str,
    /// The family name the font files register under. `None` is System.
    pub family: Option<&'static str>,
}

pub const FONTS: [Font; 5] = [
    Font {
        id: "system",
        label: "System",
        family: None,
    },
    Font {
        id: "inter",
        label: "Inter",
        family: Some("Inter"),
    },
    Font {
        id: "atkinson",
        label: "Atkinson Hyperlegible",
        family: Some("Atkinson Hyperlegible"),
    },
    Font {
        id: "plex",
        label: "IBM Plex Sans",
        family: Some("IBM Plex Sans"),
    },
    Font {
        id: "jetbrains",
        label: "JetBrains Mono",
        family: Some("JetBrains Mono"),
    },
];

macro_rules! face {
    ($f:literal) => {
        Cow::Borrowed(include_bytes!(concat!("../assets/fonts/", $f)) as &'static [u8])
    };
}

/// Regular, bold and italic of each shipped family.
fn bundled() -> Vec<Cow<'static, [u8]>> {
    vec![
        face!("Inter-Regular.ttf"),
        face!("Inter-Bold.ttf"),
        face!("Inter-Italic.ttf"),
        face!("AtkinsonHyperlegible-Regular.ttf"),
        face!("AtkinsonHyperlegible-Bold.ttf"),
        face!("AtkinsonHyperlegible-Italic.ttf"),
        face!("IBMPlexSans-Regular.ttf"),
        face!("IBMPlexSans-Bold.ttf"),
        face!("IBMPlexSans-Italic.ttf"),
        face!("JetBrainsMono-Regular.ttf"),
        face!("JetBrainsMono-Bold.ttf"),
        face!("JetBrainsMono-Italic.ttf"),
    ]
}

/// Hand the bundled faces to gpui's text system. Run once, before the first
/// `theme::init`. A failure only costs the picks that need these files.
pub fn register(cx: &mut gpui::App) {
    if let Err(e) = cx.text_system().add_fonts(bundled()) {
        eprintln!("[seance] bundled fonts: {e}");
    }
}

/// The family to set for a saved pick, or `None` for System: no pick, an id
/// this build doesn't know, or a family that isn't installed or registered.
pub fn resolve(id: Option<&str>, installed: &HashSet<String>) -> Option<&'static str> {
    let family = FONTS.iter().find(|f| Some(f.id) == id)?.family?;
    installed.contains(family).then_some(family)
}

/// `{"font":"<id>"}`, an object so more settings can join it.
fn encode(id: &str) -> String {
    serde_json::json!({ "font": id }).to_string()
}

/// The saved id, or `None` for a missing key, garbage, or an unknown id.
fn decode(body: &str) -> Option<&'static str> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let id = v.get("font")?.as_str()?;
    FONTS.iter().find(|f| f.id == id).map(|f| f.id)
}

fn config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("seance/font.json");
        }
    }
    PathBuf::from(shellexpand::tilde("~/.config/seance/font.json").as_ref())
}

/// The saved pick's id; `None` when nothing was ever picked.
pub fn load() -> Option<&'static str> {
    load_from(&config_path())
}

pub fn save(id: &str) {
    save_to(&config_path(), id);
}

fn load_from(path: &Path) -> Option<&'static str> {
    decode(&std::fs::read_to_string(path).ok()?)
}

fn save_to(path: &Path, id: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, encode(id)).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Whether menu row `id` is the one in force (no pick counts as System).
pub fn ticked(saved: Option<&str>, id: &str) -> bool {
    saved.unwrap_or("system") == id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unknown_missing_or_unresolvable_falls_back_to_system() {
        let have = installed(&["Inter", "IBM Plex Sans"]);
        assert_eq!(resolve(Some("inter"), &have), Some("Inter"));
        assert_eq!(resolve(None, &have), None);
        assert_eq!(resolve(Some("system"), &have), None);
        assert_eq!(resolve(Some("comic"), &have), None);
        // Known pick, but the family didn't register: never set it.
        assert_eq!(resolve(Some("atkinson"), &have), None);
    }

    #[test]
    fn saved_pick_round_trips_and_junk_is_ignored() {
        let dir = std::env::temp_dir().join(format!("seance-font-{}", std::process::id()));
        let file = dir.join("font.json");
        assert_eq!(load_from(&file), None);
        save_to(&file, "atkinson");
        assert_eq!(load_from(&file), Some("atkinson"));
        save_to(&file, "system");
        assert_eq!(load_from(&file), Some("system"));
        std::fs::write(&file, "{\"font\":\"comic\"}").unwrap();
        assert_eq!(load_from(&file), None);
        std::fs::write(&file, "nope").unwrap();
        assert_eq!(load_from(&file), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
