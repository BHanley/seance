//! Color schemes: the whole look of seance (chrome and terminal content) in
//! one switchable table.
//!
//! Pure data, so every client can read it: the native GUI turns `chrome` into
//! its `SeancePalette`, the daemon resolves terminal cells from `term`, and
//! the web client can mirror both later. The daemon owns which scheme is
//! active and tells every attached window (`GuiEvent::Theme`).
//!
//! Colors are packed `0xRRGGBB`.

/// The scheme a fresh install starts on (today's look).
pub const DEFAULT: &str = "candlelit";

/// Chrome tokens, named as `SeancePalette` names them: `flame` is the
/// primary accent and `violet` the second, whatever hue a scheme gives them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chrome {
    pub bg: u32,
    pub bg_elevated: u32,
    pub surface: u32,
    pub border: u32,
    pub text: u32,
    pub text_dim: u32,
    pub text_faint: u32,
    pub flame: u32,
    pub flame_dim: u32,
    pub violet: u32,
    pub violet_dim: u32,
    pub success: u32,
    pub danger: u32,
}

/// Terminal content colors: the 16 ANSI slots plus default fg/bg and cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermColors {
    pub ansi: [u32; 16],
    pub fg: u32,
    pub bg: u32,
    pub cursor: u32,
}

/// The lamp a skin hangs in each pane's title strip and around Chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lamp {
    /// A pixel wall torch (castle).
    Torch,
    /// A flickering neon tube on a metal bracket (neon-district).
    Tube,
}

/// A skin: a tiling image for chrome surfaces (rail, title and tab strips)
/// plus the chrome that goes with it. Never drawn under terminal text.
#[derive(Debug)]
pub struct Texture {
    /// PNG bytes, square, seamless on every edge.
    pub png: &'static [u8],
    /// Width and height in texels. One texel paints as one logical pixel.
    pub size: u32,
    /// Frame color of an unfocused pane. Skinned panes are square.
    pub frame: u32,
    /// A pixel lamp in each pane's title strip, lit by the pane's state;
    /// `None` hangs none.
    pub lamp: Option<Lamp>,
    /// Scales every surface's backdrop strength (`src/skin.rs`), so each
    /// skin can sit at its own weight; capped at fully opaque.
    pub strength: f32,
    /// Carved look: controls and bubbles are square plates with hard pixel
    /// bevels cut from the wall, and Chat's messages sit on dim stone
    /// (`src/carve.rs`). Other skins keep their stock rounded widgets.
    pub carved: bool,
}

/// Brian's own chat bubbles, recolored by a scheme: `bg` fills the bubble
/// and `ink` draws everything in it (text, time, ticks, links, quote).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeBubble {
    pub bg: u32,
    pub ink: u32,
}

#[derive(Debug)]
pub struct Scheme {
    pub id: &'static str,
    pub label: &'static str,
    /// Light ground: widgets switch to their light mode underneath.
    pub light: bool,
    pub chrome: Chrome,
    /// `None` keeps the user's own terminal colors (`terminal.conf`).
    pub term: Option<TermColors>,
    /// Optional chrome texture; `None` paints flat fills.
    pub texture: Option<Texture>,
    /// `None` paints Brian's bubbles in the palette's `violet_dim`.
    pub me_bubble: Option<MeBubble>,
    /// The lead's bubbles recolored the same way; `None` keeps `surface`.
    pub lead_bubble: Option<MeBubble>,
}

/// Terminal source colors in the order the scheme specs list them.
struct TermSpec {
    bg: u32,
    fg: u32,
    green: u32,
    yellow: u32,
    blue: u32,
    magenta: u32,
    red: u32,
    cyan: u32,
    gray: u32,
}

/// Fill the 16 slots from the scheme's named colors. Brights repeat the base
/// hues; black and white come from the chrome so the ends of the ramp sit on
/// the scheme's own grounds. Light schemes flip which end is which.
const fn term(light: bool, c: &Chrome, t: TermSpec) -> TermColors {
    let (black, white, bright_white) = if light {
        (c.text, c.surface, c.bg_elevated)
    } else {
        (c.bg_elevated, c.text_dim, c.text)
    };
    TermColors {
        ansi: [
            black,
            t.red,
            t.green,
            t.yellow,
            t.blue,
            t.magenta,
            t.cyan,
            white,
            t.gray,
            t.red,
            t.green,
            t.yellow,
            t.blue,
            t.magenta,
            t.cyan,
            bright_white,
        ],
        fg: t.fg,
        bg: t.bg,
        cursor: c.flame,
    }
}

const fn chrome(c: [u32; 13]) -> Chrome {
    Chrome {
        bg: c[0],
        bg_elevated: c[1],
        surface: c[2],
        border: c[3],
        text: c[4],
        text_dim: c[5],
        text_faint: c[6],
        flame: c[7],
        flame_dim: c[8],
        violet: c[9],
        violet_dim: c[10],
        success: c[11],
        danger: c[12],
    }
}

const fn spec(t: [u32; 9]) -> TermSpec {
    TermSpec {
        bg: t[0],
        fg: t[1],
        green: t[2],
        yellow: t[3],
        blue: t[4],
        magenta: t[5],
        red: t[6],
        cyan: t[7],
        gray: t[8],
    }
}

// Hex of the HSL in the native `theme.rs`; the native app paints candlelit
// from the HSL itself.
const CANDLELIT: Chrome = chrome([
    0x131111, 0x1C1718, 0x211C1D, 0x352C2E, 0xEBE3DB, 0xA69A91, 0x69605D, 0xE9A03A, 0xA97328,
    0xA790D5, 0x6C5B95, 0x86B67C, 0xD0675D,
]);
const ABYSSAL: Chrome = chrome([
    0x0A1018, 0x0F1722, 0x16233A, 0x22354D, 0xD8E6F0, 0x93AABF, 0x6E86A0, 0x3FE0D0, 0x1F8F86,
    0xFF8A6B, 0xA65544, 0x7BD88F, 0xFF5F6D,
]);
const ECTOPLASM: Chrome = chrome([
    0x0B0F0C, 0x101611, 0x17221A, 0x24332A, 0xD6F5DD, 0x98BFA3, 0x6A8873, 0x7CFF9E, 0x3E9E5A,
    0xD6B8FF, 0x7F6A9E, 0xC6F07A, 0xFF6B6B,
]);
const OUIJA: Chrome = chrome([
    0xF3EBDD, 0xEADFCC, 0xE0D0B5, 0xCDBB9C, 0x2A1E16, 0x5E4A3A, 0x735E4B, 0x8E2A1F, 0xB5574A,
    0x2F3F7A, 0x6B78A8, 0x3D6B2E, 0xC2410C,
]);
const MONOKAI: Chrome = chrome([
    0x19181A, 0x221F22, 0x2D2A2E, 0x403E41, 0xFCFCFA, 0xC1C0C0, 0x8C8A8C, 0xFFD866, 0xB39847,
    0xAB9DF2, 0x6E6599, 0xA9DC76, 0xFF6188,
]);
const TAROT: Chrome = chrome([
    0x120D1C, 0x1A1328, 0x251B3A, 0x362850, 0xEDE6F7, 0xB3A6CC, 0x8A7CA6, 0xE4B94C, 0x9E7F2E,
    0xE0457B, 0x94304F, 0x8FD19E, 0xFF5C5C,
]);
const MOJAVE: Chrome = chrome([
    0x1E1416, 0x281A1C, 0x352225, 0x4A3033, 0xF6E7DC, 0xCDAE9E, 0x9E8073, 0xFF8C42, 0xB05E2A,
    0x3FC1B0, 0x2A7F75, 0xB5D17A, 0xFF5A5F,
]);
const RISO: Chrome = chrome([
    0xF7F4EC, 0xEFEBDF, 0xE4DFCF, 0xCFC8B3, 0x1A1A1A, 0x4A4A48, 0x6B6A64, 0xE6007E, 0xF075B5,
    0x00838A, 0x5FB3B8, 0x2E7D32, 0xD32F2F,
]);
const SYNTH: Chrome = chrome([
    0x0D0B1F, 0x141130, 0x1D1844, 0x2D2660, 0xF2EEFF, 0xB9B0E6, 0x8A80C2, 0xFF3DCB, 0xA62A86,
    0x2DE2FF, 0x1C8EA1, 0x5CFFB0, 0xFF7A3D,
]);
// Fog, Parlor and Herbarium sit between the dark and light ends: a lifted
// slate dark, and two dimmed (not white) light grounds. Fog's terminal sits
// a step darker than its chrome so dim and gray text stay readable; the light
// two keep danger crimson so it doesn't read as their brick-red flame.
const FOG: Chrome = chrome([
    0x3B414D, 0x434A57, 0x4C5462, 0x545B69, 0xECEFF4, 0xC5CBD6, 0x9AA3B2, 0xF2B56B, 0xB0824A,
    0xB7A8F0, 0x7D72AE, 0xA6D38E, 0xFF8A93,
]);
const PARLOR: Chrome = chrome([
    0xCDC3B2, 0xC3B8A6, 0xB8AC98, 0xAEA28D, 0x231C17, 0x4A3E33, 0x5E5145, 0x8A2419, 0xA8564A,
    0x24527A, 0x5E7FA0, 0x2F5E22, 0x9E1449,
]);
const HERBARIUM: Chrome = chrome([
    0xBDC5B3, 0xB2BBA7, 0xA7B09B, 0x9DA690, 0x1C251E, 0x3A463C, 0x4D5A4F, 0x8E3B14, 0xAE6A48,
    0x43407F, 0x7471A6, 0x2E5A1E, 0x9A1040,
]);

// Castle: grey stone blocks with a little moss on the chrome, torchlight for
// the accent. The terminal ground stays flat stone, a step below the tile.
const CASTLE: Chrome = chrome([
    0x1B1B1D, 0x2C2C2E, 0x3A3A3D, 0x4A4A4E, 0xE6E1D6, 0xACA79D, 0x75716A, 0xE39A43, 0x9C672B,
    0x93A6C2, 0x5B6A82, 0x8DAA5B, 0xCF5A42,
]);

/// The castle stone tile, made by `scripts/gen-castle-tile.py`.
pub const CASTLE_STONE: Texture = Texture {
    png: include_bytes!("../../../assets/skins/castle-stone.png"),
    size: 256,
    frame: 0x141415,
    lamp: Some(Lamp::Torch),
    strength: 1.0,
    carved: true,
};

// Neon District: grimy riveted steel in near-black violet, hot neon for the
// accents (magenta first, cyan second). Faint text is lifted past the
// palette's dim so it holds 4.5:1 on the steel (see docs/THEME.md).
const NEON: Chrome = chrome([
    0x04010A, 0x0A0512, 0x1A0F2A, 0x2C1C44, 0xE8E6FF, 0x8B90BB, 0x7C80AA, 0xFF2FB5, 0xA8207A,
    0x12F7FF, 0x0B8C99, 0xA8FF3E, 0xFF3B4E,
]);

/// The Neon District steel tile, made by `scripts/gen-neon-tile.py`.
pub const NEON_STEEL: Texture = Texture {
    png: include_bytes!("../../../assets/skins/neon-steel.png"),
    size: 256,
    frame: 0x1B1A26,
    lamp: Some(Lamp::Tube),
    strength: 1.0,
    carved: false,
};

/// Every scheme, in menu order.
pub static SCHEMES: [Scheme; 14] = [
    Scheme {
        id: "candlelit",
        label: "Candlelit",
        light: false,
        chrome: CANDLELIT,
        term: None,
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "abyssal",
        label: "Abyssal",
        light: false,
        chrome: ABYSSAL,
        term: Some(term(
            false,
            &ABYSSAL,
            spec([
                0x0C131D, 0xC9D7E3, 0x7BD88F, 0xF2C572, 0x6AB0FF, 0xC792EA, 0xFF5F6D, 0x3FE0D0,
                0x6E86A0,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "ectoplasm",
        label: "Ectoplasm",
        light: false,
        chrome: ECTOPLASM,
        term: Some(term(
            false,
            &ECTOPLASM,
            spec([
                0x070A08, 0xB8F2C6, 0x7CFF9E, 0xE6F27A, 0x7FD4FF, 0xD6B8FF, 0xFF6B6B, 0x6FF2E0,
                0x6A8873,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "ouija",
        label: "Ouija",
        light: true,
        chrome: OUIJA,
        term: Some(term(
            true,
            &OUIJA,
            spec([
                0xF8F2E7, 0x2A1E16, 0x3D6B2E, 0x8A6100, 0x2F3F7A, 0x7A2E6E, 0xA3241A, 0x1F6B6B,
                0x735E4B,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "monokai",
        label: "Monokai Seance",
        light: false,
        chrome: MONOKAI,
        term: Some(term(
            false,
            &MONOKAI,
            spec([
                0x2D2A2E, 0xFCFCFA, 0xA9DC76, 0xFFD866, 0x78DCE8, 0xAB9DF2, 0xFF6188, 0x78DCE8,
                0x8C8A8C,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "tarot",
        label: "Tarot",
        light: false,
        chrome: TAROT,
        term: Some(term(
            false,
            &TAROT,
            spec([
                0x150F21, 0xE3DAF2, 0x8FD19E, 0xE4B94C, 0x8AA8FF, 0xC68BFF, 0xFF5C7A, 0x6FD6E0,
                0x8A7CA6,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "mojave",
        label: "Mojave Dusk",
        light: false,
        chrome: MOJAVE,
        term: Some(term(
            false,
            &MOJAVE,
            spec([
                0x22171A, 0xF2DFD3, 0xB5D17A, 0xFFC66B, 0x7FB8E6, 0xE38FB8, 0xFF5A5F, 0x3FC1B0,
                0x9E8073,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "riso",
        label: "Riso",
        light: true,
        chrome: RISO,
        term: Some(term(
            true,
            &RISO,
            spec([
                0xFBF9F3, 0x1A1A1A, 0x2E7D32, 0x8A6A00, 0x2B4FBF, 0xC2006A, 0xC62828, 0x00838A,
                0x6B6A64,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "synth",
        label: "Synth Seance",
        light: false,
        chrome: SYNTH,
        term: Some(term(
            false,
            &SYNTH,
            spec([
                0x0F0C26, 0xE6E0FF, 0x5CFFB0, 0xFFE15C, 0x6E8BFF, 0xFF3DCB, 0xFF4D6D, 0x2DE2FF,
                0x8A80C2,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "fog",
        label: "Fog",
        light: false,
        chrome: FOG,
        term: Some(term(
            false,
            &FOG,
            spec([
                0x30353F, 0xE3E7EE, 0xA6D38E, 0xF2D07A, 0x8CC4FF, 0xD3A8F0, 0xFF8A93, 0x7FDCD3,
                0x9AA3B2,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "parlor",
        label: "Parlor",
        light: true,
        chrome: PARLOR,
        term: Some(term(
            true,
            &PARLOR,
            spec([
                0xD2C9B9, 0x231C17, 0x2F5E22, 0x6E4E00, 0x24427A, 0x6E2462, 0x8A2419, 0x165858,
                0x5E5145,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "herbarium",
        label: "Herbarium",
        light: true,
        chrome: HERBARIUM,
        term: Some(term(
            true,
            &HERBARIUM,
            spec([
                0xC2CAB8, 0x1C251E, 0x2E5A1E, 0x6A5000, 0x2D4A85, 0x6E2E70, 0x8E2418, 0x1A5A5A,
                0x4D5A4F,
            ]),
        )),
        texture: None,
        me_bubble: None,
        lead_bubble: None,
    },
    Scheme {
        id: "castle",
        label: "Castle",
        light: false,
        chrome: CASTLE,
        term: Some(term(
            false,
            &CASTLE,
            spec([
                0x1E1E20, 0xDCD7CC, 0x8DAA5B, 0xD9B25A, 0x7FA2CC, 0xB58BC0, 0xD0654E, 0x7FB5A8,
                0x7A766F,
            ]),
        )),
        texture: Some(CASTLE_STONE),
        // Brian's plate is the palette's `violet_dim` darkened 10% so the
        // chalk ink holds 4.5:1 on it; the lead's is the palette's `surface`.
        me_bubble: Some(MeBubble {
            bg: 0x525F75,
            ink: 0xE6E1D6,
        }),
        lead_bubble: Some(MeBubble {
            bg: 0x3A3A3D,
            ink: 0xE6E1D6,
        }),
    },
    Scheme {
        id: "neon-district",
        label: "Neon District",
        light: false,
        chrome: NEON,
        term: Some(term(
            false,
            &NEON,
            spec([
                0x07030E, 0xE0DEF5, 0xA8FF3E, 0xFFB627, 0x7C8CFF, 0xC47BFF, 0xFF4D6A, 0x12F7FF,
                0x7A7EA8,
            ]),
        )),
        texture: Some(NEON_STEEL),
        me_bubble: Some(MeBubble {
            bg: 0xD6168F,
            ink: 0xFFFFFF,
        }),
        lead_bubble: None,
    },
];

/// Look a scheme up by id. Ids are lowercase; input is trimmed and
/// lowercased so `Ouija` from a config file still matches.
pub fn find(id: &str) -> Option<&'static Scheme> {
    let id = id.trim().to_ascii_lowercase();
    SCHEMES.iter().find(|s| s.id == id)
}

/// Position in [`SCHEMES`], for clients that keep the active scheme as an index.
pub fn index_of(id: &str) -> Option<usize> {
    let s = find(id)?;
    SCHEMES.iter().position(|x| x.id == s.id)
}

/// `candlelit, abyssal, ...` for error messages and help.
pub fn id_list() -> String {
    SCHEMES.iter().map(|s| s.id).collect::<Vec<_>>().join(", ")
}

/// WCAG relative luminance of a packed `0xRRGGBB`.
fn luminance(c: u32) -> f64 {
    let lin = |v: u32| {
        let v = f64::from(v & 0xFF) / 255.;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c >> 16) + 0.7152 * lin(c >> 8) + 0.0722 * lin(c)
}

/// WCAG contrast ratio of two colors, 1.0 to 21.0, either order.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Whether `a` is the lighter of the two colors.
pub fn is_lighter(a: u32, b: u32) -> bool {
    luminance(a) > luminance(b)
}

/// `a` moved `t` (0.0 to 1.0) of the way to `b`, channel by channel.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    [16, 8, 0].iter().fold(0, |out, s| {
        let (x, y) = ((a >> s & 0xFF) as f32, (b >> s & 0xFF) as f32);
        out | ((x + (y - x) * t).round() as u32) << s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_first_and_keeps_the_users_terminal() {
        assert_eq!(SCHEMES[0].id, DEFAULT);
        assert!(find(DEFAULT).unwrap().term.is_none());
    }

    #[test]
    fn lookup_is_case_and_space_tolerant() {
        assert_eq!(find(" Ouija ").map(|s| s.id), Some("ouija"));
        assert_eq!(index_of("synth"), Some(8));
        assert!(find("nope").is_none());
    }

    #[test]
    fn ids_are_unique_lowercase_and_listed() {
        for (i, s) in SCHEMES.iter().enumerate() {
            assert_eq!(s.id, s.id.to_ascii_lowercase());
            assert!(!s.label.is_empty());
            assert!(SCHEMES[i + 1..].iter().all(|o| o.id != s.id), "{}", s.id);
            assert!(id_list().contains(s.id));
        }
        let light: Vec<_> = SCHEMES.iter().filter(|s| s.light).map(|s| s.id).collect();
        assert_eq!(light, ["ouija", "riso", "parlor", "herbarium"]);
    }

    #[test]
    fn castle_and_neon_district_are_the_skins_and_only_castle_burns_torches() {
        let castle = find("castle").expect("castle scheme");
        assert_eq!(castle.texture.as_ref().unwrap().lamp, Some(Lamp::Torch));
        let neon = find("neon-district").expect("neon-district scheme");
        assert_eq!(neon.label, "Neon District");
        assert_eq!(neon.texture.as_ref().unwrap().lamp, Some(Lamp::Tube));
        // Appended last, so no saved index moves.
        assert_eq!(index_of("neon-district"), Some(SCHEMES.len() - 1));
        assert_eq!(index_of("castle"), Some(12));
        let skinned: Vec<_> = SCHEMES
            .iter()
            .filter(|s| s.texture.is_some())
            .map(|s| s.id)
            .collect();
        assert_eq!(skinned, ["castle", "neon-district"]);
        let torches: Vec<_> = SCHEMES
            .iter()
            .filter(|s| {
                s.texture
                    .as_ref()
                    .is_some_and(|t| t.lamp == Some(Lamp::Torch))
            })
            .map(|s| s.id)
            .collect();
        assert_eq!(torches, ["castle"]);
        // A skin's frame is darker than the hairline border it replaces.
        let luma = |c: u32| (c >> 16 & 0xFF) + (c >> 8 & 0xFF) + (c & 0xFF);
        for s in SCHEMES.iter().filter(|s| s.texture.is_some()) {
            let tex = s.texture.as_ref().unwrap();
            assert!(!s.light, "{}", s.id);
            assert_eq!(tex.size, 256, "{}", s.id);
            assert!(luma(tex.frame) < luma(s.chrome.border), "{}", s.id);
        }
    }

    #[test]
    fn only_the_skins_recolor_bubbles_and_castle_alone_recolors_the_leads() {
        let ids = |f: fn(&Scheme) -> bool| -> Vec<_> {
            SCHEMES.iter().filter(|s| f(s)).map(|s| s.id).collect()
        };
        assert_eq!(ids(|s| s.me_bubble.is_some()), ["castle", "neon-district"]);
        assert_eq!(ids(|s| s.lead_bubble.is_some()), ["castle"]);
        assert_eq!(ids(|s| s.texture.is_some_and(|t| t.carved)), ["castle"]);
        let b = find("neon-district").unwrap().me_bubble.unwrap();
        assert_eq!((b.bg, b.ink), (0xD6168F, 0xFFFFFF));
        assert!(contrast(b.ink, b.bg) >= 4.5);
    }

    #[test]
    fn contrast_and_mix_match_the_wcag_ends() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.).abs() < 1e-9);
        assert!((contrast(0xFFFFFF, 0x000000) - 21.).abs() < 1e-9);
        assert!((contrast(0x777777, 0x777777) - 1.).abs() < 1e-9);
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.), 0x000000);
        assert_eq!(mix(0x000000, 0xFFFFFF, 1.), 0xFFFFFF);
        assert_eq!(mix(0x000000, 0xFF8000, 0.5), 0x804000);
    }

    #[test]
    fn every_textured_scheme_has_a_square_seamless_tile() {
        for s in &SCHEMES {
            let Some(t) = &s.texture else { continue };
            let img = image::load_from_memory_with_format(t.png, image::ImageFormat::Png)
                .unwrap_or_else(|e| panic!("{}: {e}", s.id))
                .into_rgb8();
            assert_eq!((img.width(), img.height()), (t.size, t.size), "{}", s.id);
            let n = t.size;
            let px = |x: u32, y: u32| img.get_pixel(x % n, y % n).0;
            let diff = |a: [u8; 3], b: [u8; 3]| -> u32 {
                a.iter().zip(b).map(|(a, b)| a.abs_diff(b) as u32).sum()
            };
            // Mean change across one column or row boundary; `i` is the
            // boundary between i and i + 1, so `n - 1` is the wrap seam.
            let col = |i: u32| (0..n).map(|y| diff(px(i, y), px(i + 1, y))).sum::<u32>() / n;
            let row = |i: u32| (0..n).map(|x| diff(px(x, i), px(x, i + 1))).sum::<u32>() / n;
            // The wrap seam must be no harsher than the harshest boundary
            // inside the tile (mortar lines are the hardest ones there).
            let inner_col = (0..n - 1).map(col).max().unwrap();
            let inner_row = (0..n - 1).map(row).max().unwrap();
            assert!(col(n - 1) <= inner_col, "{} left/right seam", s.id);
            assert!(row(n - 1) <= inner_row, "{} top/bottom seam", s.id);
        }
    }

    #[test]
    fn every_token_is_a_24_bit_color() {
        for s in &SCHEMES {
            let c = s.chrome;
            for v in [
                c.bg,
                c.bg_elevated,
                c.surface,
                c.border,
                c.text,
                c.text_dim,
                c.text_faint,
                c.flame,
                c.flame_dim,
                c.violet,
                c.violet_dim,
                c.success,
                c.danger,
            ] {
                assert!(v <= 0xFF_FFFF, "{}", s.id);
            }
            // Text has to differ from the ground it sits on.
            assert_ne!(c.text, c.bg, "{}", s.id);
            if let Some(t) = s.term {
                assert!(t.ansi.iter().all(|v| *v <= 0xFF_FFFF), "{}", s.id);
                assert_ne!(t.fg, t.bg, "{}", s.id);
                assert_eq!(t.cursor, c.flame, "{}", s.id);
            } else {
                assert_eq!(s.id, DEFAULT);
            }
        }
    }

    #[test]
    fn ansi_ramp_ends_follow_the_ground() {
        let dark = find("abyssal").unwrap();
        let t = dark.term.unwrap();
        assert_eq!(t.ansi[0], dark.chrome.bg_elevated);
        assert_eq!(t.ansi[7], dark.chrome.text_dim);
        assert_eq!(t.ansi[8], 0x6E86A0);
        assert_eq!(t.ansi[9], t.ansi[1]);
        assert_eq!(t.ansi[15], dark.chrome.text);

        let light = find("riso").unwrap();
        let t = light.term.unwrap();
        assert_eq!(t.ansi[0], light.chrome.text);
        assert_eq!(t.ansi[7], light.chrome.surface);
        assert_eq!(t.ansi[15], light.chrome.bg_elevated);
    }
}
