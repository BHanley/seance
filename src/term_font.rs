//! Shared terminal font — match ghostty on this machine.
//!
//! Ghostty config: `font-family = monospace` which resolves (fc-match) to
//! **JetBrainsMono Nerd Font**, size 9, ligatures off. Seance uses the same
//! family so Claude's block-drawing logo aligns the same way.
//! `font-family` / `font-size` in `~/.config/seance/terminal.conf` override
//! both (see `term_config`).

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::OnceLock;

use gpui::{font, App, Font, FontFeatures, SharedString};

/// Family ghostty's `monospace` resolves to on this host.
const DEFAULT_FONT_FAMILY: &str = "JetBrainsMono Nerd Font";

/// Ghostty uses 9; we keep a slightly larger default for multi-pane grids.
/// Still the same face — only the size differs for readability.
const DEFAULT_FONT_SIZE: f32 = 12.0;

/// The family we ship inside the app (JetBrains Mono NL Nerd Font Mono, OFL;
/// license in `assets/fonts/OFL.txt`). A fresh Mac has no Nerd Font, and the
/// system fallback doesn't fit the cell grid (squished, widely spaced text).
pub const BUNDLED_FONT_FAMILY: &str = "JetBrainsMonoNL Nerd Font Mono";

const BUNDLED_FONTS: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/JetBrainsMonoNLNerdFontMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNLNerdFontMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNLNerdFontMono-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNLNerdFontMono-BoldItalic.ttf"),
];

/// Family actually in use, decided once at startup by `init`.
static RESOLVED: OnceLock<String> = OnceLock::new();

fn configured_family() -> &'static str {
    crate::term_config::get()
        .font_family
        .as_deref()
        .unwrap_or(DEFAULT_FONT_FAMILY)
}

/// The configured family if the text system can load it, else the bundled one.
fn pick_family(configured: &str, installed: &HashSet<String>) -> String {
    if installed.contains(configured) {
        configured.to_string()
    } else {
        BUNDLED_FONT_FAMILY.to_string()
    }
}

/// Register the bundled fonts, then settle which family the terminal uses.
/// Call once at startup, before any window draws.
pub fn init(cx: &App) {
    let text_system = cx.text_system();
    let fonts: Vec<Cow<'static, [u8]>> = BUNDLED_FONTS.iter().map(|b| Cow::Borrowed(*b)).collect();
    if let Err(e) = text_system.add_fonts(fonts) {
        eprintln!("[seance] bundled terminal font failed to load: {e}");
    }
    let installed: HashSet<String> = text_system.all_font_names().into_iter().collect();
    let family = pick_family(configured_family(), &installed);
    if family != configured_family() {
        eprintln!(
            "[seance] terminal font '{}' not available, using bundled '{family}'",
            configured_family()
        );
    }
    let _ = RESOLVED.set(family);
}

pub fn font_family() -> &'static str {
    RESOLVED
        .get()
        .map(String::as_str)
        .unwrap_or_else(configured_family)
}

/// Font size in GPUI pixels. The config is in points like Ghostty's: macOS
/// draws a point as one logical pixel, Linux as 96/72 of one.
pub fn font_size() -> f32 {
    match crate::term_config::get().font_size {
        Some(pt) if cfg!(target_os = "macos") => pt,
        Some(pt) => pt * 96.0 / 72.0,
        None => DEFAULT_FONT_SIZE,
    }
}

/// Tight line height so half-blocks stack cleanly (ghostty-ish).
pub const LINE_HEIGHT_FACTOR: f32 = 1.2;

/// Terminal font with ligatures disabled (matches ghostty's -liga/-calt).
pub fn term_font() -> Font {
    Font {
        family: SharedString::from(font_family()),
        features: FontFeatures(std::sync::Arc::new(vec![
            ("calt".into(), 0),
            ("liga".into(), 0),
            ("clig".into(), 0),
            ("dlig".into(), 0),
            ("hlig".into(), 0),
        ])),
        weight: Default::default(),
        style: Default::default(),
        // Sane monospace degradation when the Nerd Font isn't installed
        // (fresh thin-client machines): Menlo covers macOS, the rest Linux.
        fallbacks: Some(gpui::FontFallbacks::from_fonts(vec![
            BUNDLED_FONT_FAMILY.into(),
            "Menlo".into(),
            "JetBrains Mono".into(),
            "DejaVu Sans Mono".into(),
            "monospace".into(),
        ])),
    }
}

pub fn term_font_bold() -> Font {
    term_font().bold()
}

/// Convenience when only family is needed (legacy call sites).
#[allow(dead_code)]
pub fn term_font_plain() -> Font {
    font(font_family())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn installed_family_wins() {
        let installed = set(&["JetBrainsMono Nerd Font", BUNDLED_FONT_FAMILY]);
        assert_eq!(
            pick_family("JetBrainsMono Nerd Font", &installed),
            "JetBrainsMono Nerd Font"
        );
    }

    #[test]
    fn missing_family_falls_back_to_bundled() {
        let installed = set(&[BUNDLED_FONT_FAMILY, "Menlo"]);
        assert_eq!(
            pick_family("JetBrainsMono Nerd Font", &installed),
            BUNDLED_FONT_FAMILY
        );
    }
}
