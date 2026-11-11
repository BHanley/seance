//! The "Lead Reply Sound" menu: which sound plays when a circle's lead
//! finishes while you are looking elsewhere (`desktop_notify::done_sound`).
//!
//! The choice lives in `~/.config/seance/lead-sound.json`, read at play time.
//! It cannot go through `term_config::get()` (a OnceLock, fixed at launch), and
//! the point of the menu is that a pick takes effect at once. Order: the menu
//! choice if one was made, else `seance-done-sound` from terminal.conf, else
//! the Thump that ships inside the app. A missing file falls back to Thump,
//! then to silence.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the menu can hold. `Custom` carries the picked file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Thump,
    Gloop,
    Glass,
    Pop,
    Custom(PathBuf),
    None,
}

pub const SOUNDS_DIR: &str = "/System/Library/Sounds";

/// File types the Custom… picker accepts.
pub fn is_sound_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "wav" | "aiff" | "aif" | "mp3" | "m4a"))
}

fn config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("seance/lead-sound.json");
        }
    }
    PathBuf::from(shellexpand::tilde("~/.config/seance/lead-sound.json").as_ref())
}

/// The saved choice, None when nothing was ever picked (or the file is junk).
pub fn load() -> Option<Choice> {
    load_from(&config_path())
}

pub fn save(choice: &Choice) {
    save_to(&config_path(), choice);
}

fn load_from(path: &Path) -> Option<Choice> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn save_to(path: &Path, choice: &Choice) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(json) = serde_json::to_string_pretty(choice) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// `Contents/Resources/sounds` of the running app, when running from one.
pub fn bundled_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?.join("Resources/sounds");
    dir.is_dir().then_some(dir)
}

fn file_for(choice: &Choice, bundled: Option<&Path>, system: &Path) -> Option<PathBuf> {
    match choice {
        Choice::Thump => bundled.map(|d| d.join("thump.wav")),
        Choice::Gloop => bundled.map(|d| d.join("gloop.wav")),
        Choice::Glass => Some(system.join("Glass.aiff")),
        Choice::Pop => Some(system.join("Pop.aiff")),
        Choice::Custom(p) => Some(p.clone()),
        Choice::None => None,
    }
}

/// The file to play, None for silence. `exists` is injected so the order can
/// be tested without files.
pub fn resolve(
    choice: Option<&Choice>,
    conf: Option<&Path>,
    bundled: Option<&Path>,
    system: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let thump = || file_for(&Choice::Thump, bundled, system).filter(|p| exists(p));
    let picked = match choice {
        Some(Choice::None) => return None,
        Some(c) => file_for(c, bundled, system),
        None => conf.map(Path::to_path_buf),
    };
    match picked {
        Some(p) if exists(&p) => Some(p),
        // A pick or a configured file that is gone: Thump, then silence.
        Some(_) => thump(),
        None => thump(),
    }
}

/// The file `done_sound` plays right now.
pub fn current(conf: Option<&Path>) -> Option<PathBuf> {
    resolve(
        load().as_ref(),
        conf,
        bundled_dir().as_deref(),
        Path::new(SOUNDS_DIR),
        &|p| p.is_file(),
    )
}

/// The file for one menu pick (the preview), same fallbacks as playing.
pub fn for_choice(choice: &Choice) -> Option<PathBuf> {
    resolve(
        Some(choice),
        None,
        bundled_dir().as_deref(),
        Path::new(SOUNDS_DIR),
        &|p| p.is_file(),
    )
}

/// Menu rows: label and the choice it sets. Custom… (the Seance Custom
/// helper) and Custom file… (a file picker) are handled apart, so their rows
/// carry no choice here.
pub const ROWS: [(&str, Option<Choice>); 7] = [
    ("Thump", Some(Choice::Thump)),
    ("Gloop", Some(Choice::Gloop)),
    ("Glass", Some(Choice::Glass)),
    ("Pop", Some(Choice::Pop)),
    ("Custom…", None),
    ("Custom file…", None),
    ("None", Some(Choice::None)),
];

/// Which row is ticked for the saved choice (no choice = Thump, the default).
pub fn ticked(saved: Option<&Choice>, row: usize) -> bool {
    match saved.unwrap_or(&Choice::Thump) {
        Choice::Thump => row == 0,
        Choice::Gloop => row == 1,
        Choice::Glass => row == 2,
        Choice::Pop => row == 3,
        Choice::Custom(_) => row == 4,
        Choice::None => row == 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have(files: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |p| files.iter().any(|f| p == Path::new(f))
    }
    const B: &str = "/app/sounds";
    const S: &str = "/sys";

    fn go(
        choice: Option<Choice>,
        conf: Option<&str>,
        files: &'static [&'static str],
    ) -> Option<String> {
        resolve(
            choice.as_ref(),
            conf.map(Path::new),
            Some(Path::new(B)),
            Path::new(S),
            &have(files),
        )
        .map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn nothing_set_plays_thump() {
        assert_eq!(
            go(None, None, &["/app/sounds/thump.wav"]).as_deref(),
            Some("/app/sounds/thump.wav")
        );
    }

    #[test]
    fn the_conf_file_beats_thump_but_not_a_menu_pick() {
        let files = &[
            "/app/sounds/thump.wav",
            "/app/sounds/gloop.wav",
            "/c/ding.aiff",
        ];
        assert_eq!(
            go(None, Some("/c/ding.aiff"), files).as_deref(),
            Some("/c/ding.aiff")
        );
        assert_eq!(
            go(Some(Choice::Gloop), Some("/c/ding.aiff"), files).as_deref(),
            Some("/app/sounds/gloop.wav")
        );
    }

    #[test]
    fn a_missing_file_falls_back_to_thump_then_silence() {
        assert_eq!(
            go(
                Some(Choice::Custom("/gone.mp3".into())),
                None,
                &["/app/sounds/thump.wav"]
            )
            .as_deref(),
            Some("/app/sounds/thump.wav")
        );
        assert_eq!(
            go(None, Some("/gone.aiff"), &["/app/sounds/thump.wav"]).as_deref(),
            Some("/app/sounds/thump.wav")
        );
        assert_eq!(go(Some(Choice::Gloop), None, &[]), None);
        assert_eq!(go(None, None, &[]), None);
    }

    #[test]
    fn none_is_silent_even_with_a_conf_file() {
        assert_eq!(
            go(
                Some(Choice::None),
                Some("/c/ding.aiff"),
                &["/c/ding.aiff", "/app/sounds/thump.wav"]
            ),
            None
        );
    }

    #[test]
    fn glass_and_pop_come_from_the_system() {
        assert_eq!(
            go(Some(Choice::Glass), None, &["/sys/Glass.aiff"]).as_deref(),
            Some("/sys/Glass.aiff")
        );
        assert_eq!(
            go(Some(Choice::Pop), None, &["/sys/Pop.aiff"]).as_deref(),
            Some("/sys/Pop.aiff")
        );
    }

    #[test]
    fn no_bundled_folder_means_no_thump() {
        let got = resolve(None, None, None, Path::new(S), &|_| true);
        assert_eq!(got, None);
    }

    #[test]
    fn the_menu_state_persists() {
        let dir = std::env::temp_dir().join(format!("lead-sound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("lead-sound.json");
        assert_eq!(load_from(&path), None);
        for c in [
            Choice::Gloop,
            Choice::Custom("/x/y.m4a".into()),
            Choice::None,
            Choice::Thump,
        ] {
            save_to(&path, &c);
            assert_eq!(load_from(&path), Some(c));
        }
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load_from(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_row_is_ticked_and_thump_is_the_default() {
        for saved in [
            None,
            Some(Choice::Pop),
            Some(Choice::Custom("/a.wav".into())),
            Some(Choice::None),
        ] {
            let n = (0..ROWS.len())
                .filter(|&i| ticked(saved.as_ref(), i))
                .count();
            assert_eq!(n, 1);
        }
        assert!(ticked(None, 0));
        assert!(ticked(Some(&Choice::Custom("/a.wav".into())), 4));
    }

    #[test]
    fn the_picker_takes_sound_files_only() {
        for ok in ["a.wav", "a.AIFF", "a.mp3", "a.m4a", "a.aif"] {
            assert!(is_sound_file(Path::new(ok)), "{ok}");
        }
        for no in ["a.txt", "a", "a.wav.exe"] {
            assert!(!is_sound_file(Path::new(no)), "{no}");
        }
    }
}
