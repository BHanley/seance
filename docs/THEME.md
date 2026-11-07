# seance — theme & branding

seance ships fourteen color schemes (see [Schemes](#schemes)); the default, and
the one this page's palette tables describe, is **candlelit**.

Aesthetic: **candlelit**. A dark room lit by one candle — deep warm charcoal
grounds, amber candle-glow accents, muted violet highlights. Elegant and
understated, not gothic kitsch. The séance metaphor (a circle of agents and a
human working in one shared space) is carried by two accents only: the
**flame** (amber — presence / human attention) and the **violet** counterpoint
(agent affordances: ghost propose, notes, history).

All colors are `gpui::Hsla`. Candlelit's source of truth is the HSL in `src/theme.rs`;
hex below is computed from that HSL (sRGB round). Authored with
`gpui_component::hsl(h: 0..360, s: 0..100, l: 0..100)`.

## Core palette

| token            | HSL              | hex       | role |
|------------------|------------------|-----------|------|
| `bg`             | 345°  6%  7%     | `#131111` | the room — window + terminal pane ground |
| `bg_elevated`    | 345°  8% 10%     | `#1C1718` | lifted layer — panels, popovers, sidebar |
| `surface`        | 345°  8% 12%     | `#211C1D` | interactive fills — buttons, inputs, tabs |
| `border`         | 345° 10% 19%     | `#352C2E` | warm hairline separators |
| `text`           |  30° 27% 89%     | `#EBE3DB` | primary text — warm off-white |
| `text_dim`       |  25° 11% 61%     | `#A69A91` | secondary text — muted parchment |
| `text_faint`     |  15°  6% 39%     | `#69605D` | tertiary / disabled |
| `flame`          |  35° 80% 57%     | `#E9A03A` | **primary accent** — amber candle-glow |
| `flame_dim`      |  35° 62% 41%     | `#A97328` | dimmed ember — amber hover/active |
| `violet`         | 260° 45% 70%     | `#A790D5` | **secondary accent** — muted violet |
| `violet_dim`     | 258° 24% 47%     | `#6C5B95` | dimmed violet |
| `success`        | 110° 28% 60%     | `#86B67C` | soft sage — calm affirmation |
| `danger`         |   5° 55% 59%     | `#D0675D` | warm muted red — doesn't scream |

### Session status dots

| token             | maps to     | meaning |
|-------------------|-------------|---------|
| `status_exited`   | `text_faint`| process left the circle |

> `status_running` / `status_idle` were removed 2026-07-22 (dead code); live
> "working" state is drawn from observed TUI title spinners, not theme tokens.

## Schemes

Every scheme defines the same chrome tokens as the table above (`flame` is
its primary accent, `violet` its second) plus, except candlelit, a terminal
palette. The table lives in `crates/seance-core/src/schemes.rs` (packed
`0xRRGGBB`, wasm-safe, so the web client can read it later);
`SeancePalette::*()` returns the active scheme's color, so call sites never
name a scheme.

| id | label | ground | accent | second |
|----|-------|--------|--------|--------|
| `candlelit` | Candlelit (default) | `#131111` | `#E9A03A` | `#A790D5` |
| `abyssal` | Abyssal | `#0A1018` | `#3FE0D0` | `#FF8A6B` |
| `ectoplasm` | Ectoplasm | `#0B0F0C` | `#7CFF9E` | `#D6B8FF` |
| `ouija` | Ouija (light) | `#F3EBDD` | `#8E2A1F` | `#2F3F7A` |
| `monokai` | Monokai Seance | `#19181A` | `#FFD866` | `#AB9DF2` |
| `tarot` | Tarot | `#120D1C` | `#E4B94C` | `#E0457B` |
| `mojave` | Mojave Dusk | `#1E1416` | `#FF8C42` | `#3FC1B0` |
| `riso` | Riso (light) | `#F7F4EC` | `#E6007E` | `#00838A` |
| `synth` | Synth Seance | `#0D0B1F` | `#FF3DCB` | `#2DE2FF` |
| `fog` | Fog | `#3B414D` | `#F2B56B` | `#B7A8F0` |
| `parlor` | Parlor (light) | `#CDC3B2` | `#8A2419` | `#24527A` |
| `herbarium` | Herbarium (light) | `#BDC5B3` | `#8E3B14` | `#43407F` |
| `castle` | Castle (skin) | `#1B1B1D` | `#E39A43` | `#93A6C2` |
| `neon-district` | Neon District (skin) | `#04010A` | `#FF2FB5` | `#12F7FF` |

**Skins.** A scheme may also carry a `texture` (`Texture` in
`seance-core/src/schemes.rs`): a seamless square PNG tile, one texel per
logical pixel, plus the frame color and a `lamp` (`Lamp::Torch`,
`Lamp::Tube` or none). Castle is the
first: a 256 px tile of grey dithered stone with cracks, damp and rust
streaks and a little moss, on the rail, the pane title strips, the minimize
shelf, the dock tab strip and drawer headers. The tile comes from
`scripts/gen-castle-tile.py` and lives in `assets/skins/`. `src/skin.rs`
paints it: gpui has no repeat fill, so a `canvas` lays the tile out with
`paint_image`, anchored to the window origin so every surface shares one
grid. The tile is decoded and upscaled once per scale factor and cached, so
the atlas uploads it once and a frame only adds one sprite per tile position.
Terminal text never sits on a texture; the pane body stays the flat `bg`.
Skinned panes keep the 2px frame but draw it in the skin's dark stone color
with square corners; schemes without a texture paint exactly as before
(rounded frame, `border` color).

Neon District is the second skin: a 256 px tile of riveted steel plates in
near-black violet (22 colors), each plate its own shade, with deep bevelled
seams, rivet caps, patchy dithered shadow, oil stains, bright scratches,
rust streaks running down from rivets and seams, one short strip of worn
hazard tape, faint scanlines and magenta and cyan neon staining that bleeds
well out of some seams, from `scripts/gen-neon-tile.py` (run it with `uv
run --offline --no-project --with numpy --with pillow python
scripts/gen-neon-tile.py`). The grime gets its contrast from deep shadow as
much as from bright detail, so the tile's average stays dark enough for
labels. It covers the same surfaces as Castle, Chat included. Each skin
scales the backdrop strengths by its `Texture::strength`; both skins use
1.0, so Neon District sits at Castle's strengths. Its pane frame is dark
steel `#1B1A26`. Its accent is magenta, its second accent cyan, and
`text_faint` is lifted to `#7C80AA` so faint labels hold 4.5:1 on the steel.

**Brian's bubbles (`Scheme::me_bubble`).** A scheme can recolor Brian's own
chat bubbles with `MeBubble { bg, ink }`; `None` (every scheme but Neon
District) keeps `violet_dim` and the palette's colors. Neon District sets hot
deep magenta `#D6168F` with white ink `#FFFFFF`, and everything in the bubble
takes the ink (`me_ink` in `src/app/chat.rs`): text, links (ink, underlined),
"not delivered", the seen ticks and the reply quote's bar and words at full
ink; the time and a waiting tick at a softer ink; the quote and a video tile
on a darker tint (12% toward black under a light ink, toward the ink under a
dark one, `#BC137E`). The softer ink steps back from 20% toward the fill only
as far as it must to hold 4.5:1: with a light ink on a mid fill that is 4%
(`#FDF6FB`), so it reads as white. WCAG ratios: ink on the fill 4.8, softer
ink on the fill 4.5, ink on the tint 5.9. White on the brighter `#FF2FB5`
was 3.3, which is why the fill is deeper.

On the Chat rail the selected circle row takes the bubble's fill at 22%
opacity and a 3 px bar of it on the left edge (absolute, so nothing shifts);
schemes without `me_bubble` keep `surface`.

Label contrast on the skins (WCAG ratio against the textured ground: the
mean ground, then its 95th-percentile brightest pixel; the tile math and
the rendered shots agree to 0.1 on the mean). The brightest 5% of the neon
ground (rivet caps, scratches, neon stains) dips faint labels to about 4.1
to 4.4; the average holds 4.8 or better:

| surface | scheme | text | text_dim | text_faint |
|---------|--------|------|----------|------------|
| rail (0.6) | neon-district | 15.8 / 13.2 | 6.2 / 5.2 | 5.1 / 4.2 |
| title strip (0.55) | neon-district | 15.8 / 13.5 | 6.3 / 5.3 | 5.1 / 4.3 |
| active title (0.3) | neon-district | 15.1 / 13.8 | 6.0 / 5.4 | 4.8 / 4.4 |
| rail (0.6) | castle | 11.1 / 9.3 | 6.0 / 5.1 | 3.0 / 2.5 |
| title strip (0.55) | castle | 11.0 / 9.4 | 6.0 / 5.1 | 3.0 / 2.5 |
| active title (0.3) | castle | 9.4 / 8.6 | 5.1 / 4.7 | 2.5 / 2.3 |

**Torches.** A skin with `Lamp::Torch` puts a 12x20 pixel wall torch in each
pane's title strip (`src/torch.rs`, painted as quad runs in one canvas). It
burns the way the pane is doing, in Stage's `pane_state` order: stopped (out,
ember and smoke) when the pane exited, sleeps or has no process; needs-you
(blown flat sideways); working (tall, roaring, sparks); idle (a calm flame)
otherwise. Torches have no timer of their own: they ride the sidebar
spinner's 240 ms tick, every state stepping one frame per tick (frame =
ticks seen with a torch on screen), and that tick renders at most once
whether the spinner, a torch or both asked. With no torch on screen
(another scheme, another view, an empty circle) they add nothing.

**Neon tubes.** A skin with `Lamp::Tube` (Neon District) hangs a 12x20
neon tube on a metal bracket in every spot a torch would take (`src/tube.rs`;
`torch::element` and `torch::tall_element` pick tube or torch texels from
the active skin, so no call site changes), with the same states, seeds and
tick. Idle burns steady cyan and flickers once every 24 ticks (dim for a
tick, then a stretch cuts out); working buzzes magenta, swapping hot and lit
every tick with a hot band and sparks off the caps; needs-you blinks amber
every tick; stopped is dead glass that sputters faintly once every 40 ticks.
Each tube's rare events sit at its own phase (`seed % period`), so Chat's
nine decorative tubes never flicker together. The standing torch becomes a
tall cyan tube on a post, 8x40 like the torch.

**Who owns the choice.** The daemon. It saves it in the state dir
(`theme.json`, `{"scheme": ID}`), sends it to each window on attach and
broadcasts `GuiEvent::Theme` on every switch, so the Mac thin client and the
XPS GUI change together. At boot the saved choice wins, then `seance-theme`
in `terminal.conf`, then candlelit; an unknown id falls back to candlelit.

**Switching.** The ✦ menu at the top of the rail lists the fourteen, dark ones then
light ones under their own headings, active one marked; `seance ctl theme` lists them and `seance ctl theme ID` switches (an
unknown ID is refused with the list). A window that hears the switch sets
the active scheme, re-runs `theme::init` and refreshes every window.

**Terminal colors.** Cells are resolved daemon-side, so on a switch the
daemon swaps its palette (`pty_session::set_term_colors`), recolors each live
pane's seeded palette (slots a program set itself via OSC 4/10/11 are kept)
and re-sends every pane as a full frame, idle panes included. OSC color
queries answer from the active palette. The 16 ANSI slots come from each
scheme's named colors: 1 to 6 and 9 to 14 are its red, green, yellow, blue,
magenta, cyan; 8 is its gray; on dark schemes 0 = `bg_elevated`, 7 =
`text_dim`, 15 = `text`, on light schemes 0 = `text`, 7 = `surface`, 15 =
`bg_elevated`. Cursor = accent. Candlelit keeps `terminal.conf` exactly.

**Light schemes** (ouija, riso, parlor, herbarium) put gpui-component in `ThemeMode::Light`
before the palette is applied, use the light code-highlight theme in
markdown panes, and put the ground color on top of filled red (danger). Hovers use
`theme::lift`, which darkens instead of lightening on a light ground.

**Limits.** Sleeping panes are recolored by swapping exact old palette
colors in their frozen frame, so a program's own truecolor stays as it was.
A program that asked for the background once at startup (claude does, via
OSC 11, to pick its own dark or light look) keeps that pick until it
restarts. The web client's chrome stays candlelit for now; its cells follow
the scheme because the daemon resolves them.

## ANSI terminal palette (16-color)

**Not this module** (for candlelit; other schemes bring their own, see
above). Agent CLIs and shells render inside the embedded terminal
against a 16-color palette resolved daemon-side from `src/term_config.rs`.
The default is the ghostty base16 set (`#181818` bg / `#d8d8d8` fg,
`#ab4642 #a1b56c #f7ca88 #7cafc2 #ba8baf #86c1b9` …);
`~/.config/seance/terminal.conf` (Ghostty syntax: `theme`, `palette`,
`background`, `foreground`, `cursor-color`) replaces it, and the program can
still override it via OSC 4/10/11. The seance-tinted ANSI table that used to live here
described `theme.rs::ansi_palette()`, which was removed 2026-07-22 with the
dead local-PTY path — nothing read it. Change terminal colors in
`terminal.conf`; change chrome colors here.

## Public API (`src/theme.rs`)

```rust
pub struct SeancePalette;                 // color accessors, each -> gpui::Hsla
impl SeancePalette {
    pub fn bg() -> Hsla;            pub fn bg_elevated() -> Hsla;
    pub fn surface() -> Hsla;       pub fn border() -> Hsla;
    pub fn text() -> Hsla;          pub fn text_dim() -> Hsla;
    pub fn text_faint() -> Hsla;
    pub fn flame() -> Hsla;         pub fn flame_dim() -> Hsla;
    pub fn violet() -> Hsla;        pub fn violet_dim() -> Hsla;
    pub fn success() -> Hsla;       pub fn danger() -> Hsla;
    pub fn status_exited() -> Hsla;
}

pub fn init(cx: &mut gpui::App);             // apply the active scheme (dark or light) + overrides
pub fn set_scheme(id: &str, cx: &mut gpui::App) -> bool; // follow the daemon; re-inits on change
pub fn active() -> &'static Scheme;          // seance_core::schemes entry in use
pub fn is_light() -> bool;
pub fn term_fg_bg() -> Option<(u32, u32)>;   // scheme terminal fg/bg, None = terminal.conf
```

Colors are associated **functions** (not consts) because they follow the
active scheme. Each scheme's chrome is converted to `Hsla` once per process;
a call is an atomic load and an index.

## How `init()` applies the theme

Call order in `main`: `gpui_component::init(cx)` → **`theme::init(cx)`** →
open window.

`init()` does four steps (see `src/theme.rs::init`); step 4 is font-family
hygiene on non-macOS — pick the first installed sans/mono rather than letting
gpui probe, which is load-bearing for typing latency:

1. **`Theme::change(ThemeMode::Dark, None, cx)`** (`Light` for a light
   scheme): activates the mode.
   `Theme::change` lazily creates the `Theme` global if absent and calls
   `apply_config(dark_theme)`, seeding every `ThemeColor` field from the
   built-in *Default Dark* before we override.
   — `crates/ui/src/theme/mod.rs:163-183`
2. **Mutate `Theme::global_mut(cx).colors`** — `Theme` derefs to `ThemeColor`
   and `colors` is a public field, so we overwrite ~70 relevant fields with the
   candlelit palette (`apply_palette`).
   — `crates/ui/src/theme/mod.rs:97-109` (Deref/DerefMut), field list at
   `crates/ui/src/theme/theme_color.rs:57-345`
3. **`theme.tokens = ThemeTokens::from(&theme.colors)`** — widgets paint from
   the resolved `ThemeTokens` (color + paint background), which does **not**
   auto-recompute when we poke individual `colors` fields. This regeneration is
   the load-bearing step — skip it and the overrides are invisible to
   components.
   — `ThemeTokens` def `theme_color.rs:347-368`; `From<&ThemeColor>`
   `theme_color.rs:364-368`; same pattern used internally at `mod.rs:234` and
   `schema.rs:939`.

Color transforms (`.opacity/.lighten/.darken`) come from `gpui_component`'s
`Colorize` trait (`crates/ui/src/theme/color.rs:19-61`).

### Field-mapping decisions

- **`primary` = flame**; `warning` also = flame (keeps the app to two accents).
- **`info` / `link` = violet**; violet is the only cool accent.
- `primary_foreground` / `success_foreground` / `warning_foreground` = `bg`
  (dark text on the bright amber/sage for contrast).
- `ring` / `caret` = flame — the candle picks out what has focus.
- `selection` = `violet_dim` @ 45% alpha.
- Inputs are border-driven on dark: gpui-component's `Theme::input_background()`
  mixes `input` with transparent, so we set `input = border` and let focus
  recolor via `ring` (`mod.rs:189-196`).

## Uncertainty flags for the integrator

1. **hex is computed, HSL is canonical.** The table hex is a Python
   `colorsys` sRGB round-trip of the HSL in `theme.rs`; gpui's own HSL→sRGB
   conversion may differ by ±1 per channel. Trust the on-screen render / the
   HSL, not the hex.
2. **`apply_palette` covers the fields seance uses.** The `chart_*` swatches and
   the base `red/green/blue/yellow/magenta/cyan(+_light)` `ThemeColor` fields are
   left at the Default-Dark seed — they only feed data-viz widgets seance
   doesn't ship, and terminal color is resolved daemon-side
   (ghostty palette in `runtime/pty_session.rs`), not by the theme module.
   If a component you add reads those, extend `apply_palette`.
   (`ansi_palette()` was removed 2026-07-22 with the dead local-PTY path.)
3. **No `window_border` on non-Linux**, per gpui-component docs
   (`theme_color.rs:314-319`). seance is Linux-only, so this is fine; we set it
   to `border`.
4. **`Theme::change` re-seeds from Default Dark, then we override.** If a future
   gpui-component rev renames/removes a `ThemeColor` field this file sets, that
   line won't compile — the field list is pinned to rev
   `b5eef62336f88bb6c1ee45bf32f73c9895d49f8d`. Verified against the local
   checkout, not training data.
5. **The web client re-declares this palette by hand.** `crates/seance-web`
   has no access to `theme.rs`: the same values appear as GL constants
   (`renderer.rs`) and as hex literals in the chrome CSS (`ui.rs`, `help.rs`,
   `activity.rs`, `menus.rs`, `probe.rs`). Values currently match; there is no
   compile-time link, so a change here must be mirrored there by hand.
```

## Carved stone (`Texture::carved`, castle)

A skin with `carved: true` cuts its widgets from the wall (`src/carve.rs`).
Builders call `carve::plate`, `corners`, `carve` or `worn_plate`; a scheme
that does not carve gets the stock rounded widget back from the same call.

- **Corners and bevels.** Square. A bevel is one canvas of whole-pixel strips:
  a 1 px dark outline, then a lit top and left and a shadowed bottom and
  right (2 px on message plates, 1 px on buttons, tabs and chips; the
  composer and the view switch are slots, lit side below). No blur, gradient
  or transparency. Covered: bubbles, header buttons, "Show earlier", quick
  reply chips, choice pills, attach and Send, the composer, the view switch,
  the popover. Picture and video frames and quote blocks are square.
- **Plates.** `me_bubble` is the palette's `violet_dim` darkened 10%
  (`#525F75`, chalk ink 4.9:1; stock `violet_dim` was 4.2:1); `lead_bubble` is
  the palette's `surface` (`#3A3A3D`). Both take the stone tile at 45% from
  their own corner (so the wear moves with the plate), plus the bevel.
  Worst case over the tile's brightest pixel: ink 5.4 (Brian) and 7.4 (lead),
  time and ticks 4.9 and 5.3, quote text on its tint 5.9 and 9.7.
- **Stone behind messages.** The tile at 55% over the ground, painted by
  `skin::backdrop` anchored to the window, so it stays fixed while the thread
  scrolls. Over its brightest pixel: text 8.8, dim text 4.8. Faint text (the
  palette's own `text_faint`) is 2.4, as it is on the flat ground (3.5).
- **Selected row.** The chain frames it (3 pt texels, rings 5 texels tall,
  hanging 9 pt over the row's top and bottom, deferred so it paints above its
  neighbours). A run's end ring sits in each corner and holds that side's
  bar by its outer end; a side is one whole bar, or two and a slim ring once
  it is 16 texels long. `chain::OPTION` picks the shape: `Frame` (the
  default), `Rails` (top and bottom only, each end ring on a bolt) or
  `Heavy` (2 pt texels, thicker walls, a brighter glint and a black
  outline, in the same 14 to 15 pt band). Row padding is unchanged. Neon's
  wash and bar are for schemes that recolor bubbles and do not carve.
