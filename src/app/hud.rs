//! The floating progress HUD (macOS): a tiny always-on-top window listing
//! the panes that are working or waiting on you, so a glance shows what is
//! running without bringing Seance forward.
//!
//! Two looks, picked by `seance-hud` in terminal.conf or the Seance menu:
//!
//! * **Ink**: bottom-right of the display, no container, a line and a 1px
//!   track per pane. Hovering adds a dark backing and each pane's full name.
//! * **Pips**: top-centre under the menu bar, a pill of short bars (a dot for
//!   a pane that needs you). Hovering opens the same row list.
//!
//! The HUD never takes focus and never activates the app: it is a
//! non-activating panel, sized to its content so it covers almost nothing,
//! and hidden while nothing runs. Clicking a row brings the main window
//! forward with that pane on stage. Rows come from the same state the rail
//! reads (busy panes, statuses, open asks); nothing here talks to the daemon.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, AnyWindowHandle, App, Bounds, Context, DisplayId, FontFeatures, Global,
    Hsla, Pixels, SharedString, WeakEntity, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions,
};

use super::wall_model::task_title;
use super::SeanceApp;
use crate::theme::SeancePalette;

/// Which HUD, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudMode {
    Off,
    Ink,
    Pips,
}

impl HudMode {
    /// `seance-hud` value: `ink`, `pips`, else off.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("ink") => Self::Ink,
            Some("pips") => Self::Pips,
            _ => Self::Off,
        }
    }
}

/// A finished row stays this long, fading, before it goes.
const FADE: Duration = Duration::from_secs(3);

// Layout, in points. Every height is fixed so the window can be sized to its
// content without measuring a frame first.
const LINE_H: f32 = 14.;
const TRACK_GAP: f32 = 3.;
const FULL_H: f32 = 13.;
const ROW_GAP: f32 = 7.;
const INK_W: f32 = 260.;
const INK_PAD_X: f32 = 10.;
const INK_PAD_Y: f32 = 8.;
const PIP_W: f32 = 18.;
const PIP_H: f32 = 3.;
const DOT: f32 = 5.;
const PIP_GAP: f32 = 5.;
const PILL_PAD_X: f32 = 8.;
const PILL_H: f32 = 15.;
const LIST_W: f32 = 300.;
const LIST_PAD_X: f32 = 11.;
const LIST_PAD_Y: f32 = 9.;
/// Space between the HUD and the display's edge (Ink) or menu bar (Pips).
const MARGIN: f32 = 16.;
const PIPS_TOP: f32 = 6.;

/// What the app says about one pane right now.
#[derive(Clone, Debug, PartialEq)]
struct RowIn {
    slug: String,
    circle: String,
    /// The circle slug, for opening it.
    workspace: String,
    text: String,
    full: String,
    needs: bool,
    step: Option<(u32, u32)>,
    /// When it went busy, when the app knows.
    busy_since: Option<Instant>,
}

/// A row on the HUD: the latest [`RowIn`], when it first showed, and when it
/// stopped (it fades for [`FADE`] after that).
struct Row {
    now: RowIn,
    since: Instant,
    gone_at: Option<Instant>,
}

/// `N/M` in a status note ("build 3/5"): step N of M.
fn parse_step(note: &str) -> Option<(u32, u32)> {
    static STEP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(\d{1,3})\s*/\s*(\d{1,3})").expect("step pattern")
    });
    STEP.captures_iter(note).find_map(|c| {
        let n: u32 = c[1].parse().ok()?;
        let m: u32 = c[2].parse().ok()?;
        (m > 0 && n <= m).then_some((n, m))
    })
}

/// "45s", "4m", "1h05".
fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h{:02}", s / 3600, (s % 3600) / 60),
    }
}

impl SeanceApp {
    /// Every terminal that is working or needs you ([`merge_rows`] orders
    /// them).
    fn hud_rows(&self, cx: &App) -> Vec<RowIn> {
        let mut out = Vec::new();
        let terminals = self
            .panes
            .iter()
            .filter(|p| matches!(p.body, crate::pane::PaneBody::Remote { .. }));
        for p in terminals {
            let status = self.statuses.get(&p.slug);
            let state = status.map(|s| s.state.as_str()).unwrap_or("");
            let note = status
                .and_then(|s| s.note.as_deref())
                .map(str::trim)
                .filter(|n| !n.is_empty());
            let ask = self
                .asks
                .iter()
                .find(|a| a.answer.is_none() && a.from == p.slug);
            let needs = ask.is_some() || matches!(state, "needs-human" | "blocked" | "risky");
            let busy = self.busy_panes.contains(&p.slug);
            if !needs && !busy && state != "working" {
                continue;
            }
            let circle = self.workspace_label(&p.workspace);
            let task = p.title(cx).and_then(|t| task_title(&t, &p.command));
            let text = match (ask, needs) {
                (Some(a), _) => a.question.clone(),
                (None, true) => note.unwrap_or("needs you").to_string(),
                (None, false) => note
                    .map(str::to_string)
                    .or_else(|| task.clone())
                    .unwrap_or_else(|| p.name.clone()),
            };
            let detail = task.or_else(|| note.map(str::to_string));
            let full = match detail {
                Some(d) => format!("{circle} · {} · {d}", p.name),
                None => format!("{circle} · {}", p.name),
            };
            out.push(RowIn {
                slug: p.slug.clone(),
                circle,
                workspace: p.workspace.clone(),
                text,
                full,
                needs,
                step: if needs {
                    None
                } else {
                    note.and_then(parse_step)
                },
                busy_since: self.busy_since.get(&p.slug).copied(),
            });
        }
        out
    }
}

/// The HUD window's view.
pub struct HudView {
    mode: HudMode,
    app: WeakEntity<SeanceApp>,
    main: AnyWindowHandle,
    rows: Vec<Row>,
    hovered: bool,
    /// The window size last asked of the platform, and whether it is on screen.
    size: (f32, f32),
    shown: bool,
    _tick: gpui::Task<()>,
}

impl HudView {
    fn new(
        mode: HudMode,
        app: WeakEntity<SeanceApp>,
        main: AnyWindowHandle,
        size: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        native::setup(window);
        native::set_visible(window, false);
        let tick = cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| loop {
            let fading = this
                .update_in(cx, |this, window, cx| {
                    this.tick(window, cx);
                    this.rows.iter().any(|r| r.gone_at.is_some())
                })
                .unwrap_or(false);
            let every = if fading { 100 } else { 500 };
            cx.background_executor()
                .timer(Duration::from_millis(every))
                .await;
            if this.upgrade().is_none() {
                return;
            }
        });
        Self {
            mode,
            app,
            main,
            rows: Vec::new(),
            hovered: false,
            size,
            shown: false,
            _tick: tick,
        }
    }

    /// Re-read the app's rows, age out finished ones, and fit the window.
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let fresh = app.read(cx).hud_rows(cx);
        merge_rows(&mut self.rows, fresh, Instant::now());
        self.fit(window);
        cx.notify();
    }

    /// Size the window to the rows (and the hover state), and hide it when
    /// there is nothing to show.
    fn fit(&mut self, window: &mut Window) {
        if self.rows.is_empty() {
            self.hovered = false;
            if self.shown {
                native::set_visible(window, false);
                self.shown = false;
            }
            return;
        }
        let want = content_size(self.mode, self.hovered, &self.rows);
        if want != self.size {
            native::resize(window, want, self.mode);
            self.size = want;
        }
        if !self.shown {
            native::set_visible(window, true);
            self.shown = true;
        }
    }

    /// Bring the main window forward with the pane on stage.
    fn open(&self, workspace: String, slug: String, cx: &mut App) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        cx.activate(true);
        let _ = self.main.update(cx, |_, window, cx| {
            window.activate_window();
            app.update(cx, |app, cx| {
                app.focus_on_stage(&workspace, Some(&slug), window, cx);
            });
        });
    }
}

/// Fold the app's current rows into the HUD's: new ones join, live ones
/// update, ones that stopped start fading and leave after [`FADE`].
fn merge_rows(rows: &mut Vec<Row>, fresh: Vec<RowIn>, now: Instant) {
    let mut fresh: HashMap<String, RowIn> =
        fresh.into_iter().map(|r| (r.slug.clone(), r)).collect();
    for row in rows.iter_mut() {
        match fresh.remove(&row.now.slug) {
            Some(r) => {
                row.now = r;
                row.gone_at = None;
            }
            None => {
                row.gone_at.get_or_insert(now);
            }
        }
    }
    rows.retain(|r| r.gone_at.is_none_or(|t| now.duration_since(t) < FADE));
    for r in fresh.into_values() {
        rows.push(Row {
            since: r.busy_since.unwrap_or(now),
            now: r,
            gone_at: None,
        });
    }
    // Needs-you first, then the longest-running; a stable key for ties.
    rows.sort_by(|a, b| {
        b.now
            .needs
            .cmp(&a.now.needs)
            .then(a.since.cmp(&b.since))
            .then(a.now.slug.cmp(&b.now.slug))
    });
}

/// One row's height in the list (Ink, and Pips when hovered).
fn row_h(row: &Row, hovered: bool) -> f32 {
    let track = if row.now.needs { 0. } else { TRACK_GAP + 1. };
    let full = if hovered { FULL_H } else { 0. };
    LINE_H + track + full
}

/// The window size for `rows` in `mode`.
fn content_size(mode: HudMode, hovered: bool, rows: &[Row]) -> (f32, f32) {
    let list_h = |hovered| {
        rows.iter().map(|r| row_h(r, hovered)).sum::<f32>()
            + ROW_GAP * rows.len().saturating_sub(1) as f32
    };
    match (mode, hovered) {
        (HudMode::Pips, false) => {
            let pips: f32 = rows
                .iter()
                .map(|r| if r.now.needs { DOT } else { PIP_W })
                .sum();
            let gaps = PIP_GAP * rows.len().saturating_sub(1) as f32;
            (pips + gaps + 2. * PILL_PAD_X, PILL_H)
        }
        (HudMode::Pips, true) => (LIST_W + 2. * LIST_PAD_X, list_h(true) + 2. * LIST_PAD_Y),
        _ => (INK_W + 2. * INK_PAD_X, list_h(hovered) + 2. * INK_PAD_Y),
    }
}

/// The HUD's grays: theme text at a few strengths.
fn gray(alpha: f32) -> Hsla {
    SeancePalette::text().opacity(alpha)
}

/// Text with a soft dark copy under it, for reading over any wallpaper
/// (gpui has no text shadow).
fn shadowed(text: SharedString, color: Hsla, shadow: bool) -> gpui::AnyElement {
    let top = div()
        .relative()
        .truncate()
        .text_color(color)
        .child(text.clone());
    if !shadow {
        return top.into_any_element();
    }
    div()
        .relative()
        .child(
            div()
                .absolute()
                .top(px(1.))
                .left_0()
                .right_0()
                .truncate()
                .text_color(gpui::black().opacity(0.55))
                .child(text),
        )
        .child(top)
        .into_any_element()
}

impl HudView {
    fn render_row(&self, i: usize, row: &Row, cx: &mut Context<Self>) -> gpui::AnyElement {
        let now = Instant::now();
        let hi = self.hovered;
        // Ink's resting state floats over the desktop, so it gets the shadow.
        let shadow = self.mode == HudMode::Ink && !hi;
        let alpha = row
            .gone_at
            .map(|t| 1. - now.duration_since(t).as_secs_f32() / FADE.as_secs_f32())
            .unwrap_or(1.)
            .clamp(0., 1.);
        let text_c = if hi || row.now.needs {
            gray(0.92)
        } else {
            gray(0.62)
        };
        let label = format!("{} · {}", row.now.circle, row.now.text);
        let elapsed = fmt_elapsed(now.duration_since(row.since));
        let fill = row.now.step.map(|(n, m)| n as f32 / m as f32).unwrap_or(0.);
        let (workspace, slug) = (row.now.workspace.clone(), row.now.slug.clone());
        let mut el = div()
            .id(("hud-row", i))
            .flex()
            .flex_col()
            .h(px(row_h(row, hi)))
            .opacity(alpha)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open(workspace.clone(), slug.clone(), cx);
            }))
            .child(
                div()
                    .h(px(LINE_H))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(11.))
                    .line_height(px(LINE_H))
                    .whitespace_nowrap()
                    .when(row.now.needs, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .size(px(DOT))
                                .rounded_full()
                                .bg(SeancePalette::violet()),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .truncate()
                            .child(shadowed(label.into(), text_c, shadow)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .font_features(FontFeatures(std::sync::Arc::new(vec![(
                                "tnum".into(),
                                1,
                            )])))
                            .child(shadowed(elapsed.into(), text_c, shadow)),
                    ),
            );
        if !row.now.needs {
            el = el.child(
                div()
                    .mt(px(TRACK_GAP))
                    .h(px(1.))
                    .w_full()
                    .bg(gray(0.10))
                    .child(div().h_full().w(gpui::relative(fill)).bg(gray(0.42))),
            );
        }
        if hi {
            el = el.child(
                div()
                    .h(px(FULL_H))
                    .text_size(px(10.))
                    .line_height(px(FULL_H))
                    .text_color(gray(0.45))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .truncate()
                    .child(row.now.full.clone()),
            );
        }
        el.into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| self.render_row(i, r, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .children(rows)
            .into_any_element()
    }

    fn render_pips(&self) -> gpui::AnyElement {
        let pips = self.rows.iter().map(|r| {
            let alpha = r
                .gone_at
                .map(|t| 1. - t.elapsed().as_secs_f32() / FADE.as_secs_f32())
                .unwrap_or(1.)
                .clamp(0., 1.);
            if r.now.needs {
                return div()
                    .flex_none()
                    .size(px(DOT))
                    .rounded_full()
                    .bg(SeancePalette::violet())
                    .opacity(alpha);
            }
            let fill = r.now.step.map(|(n, m)| n as f32 / m as f32).unwrap_or(0.);
            div()
                .flex_none()
                .w(px(PIP_W))
                .h(px(PIP_H))
                .rounded(px(2.))
                .overflow_hidden()
                .bg(gray(0.10))
                .opacity(alpha)
                .child(div().h_full().w(gpui::relative(fill)).bg(gray(0.55)))
        });
        div()
            .flex()
            .items_center()
            .gap(px(PIP_GAP))
            .children(pips)
            .into_any_element()
    }
}

impl Render for HudView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (w, h) = self.size;
        let backing = SeancePalette::bg().opacity(0.82);
        let root = div()
            .id("hud")
            .w(px(w))
            .h(px(h))
            .overflow_hidden()
            .font_family(".SystemUIFont")
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if this.hovered != *hovered && !this.rows.is_empty() {
                    this.hovered = *hovered;
                    this.fit(window);
                    cx.notify();
                }
            }));
        match (self.mode, self.hovered) {
            (HudMode::Pips, false) => root
                .flex()
                .items_center()
                .px(px(PILL_PAD_X))
                .rounded_full()
                .bg(SeancePalette::bg().opacity(0.38))
                .child(self.render_pips()),
            (HudMode::Pips, true) => root
                .px(px(LIST_PAD_X))
                .py(px(LIST_PAD_Y))
                .rounded(px(12.))
                .bg(backing)
                .child(self.render_list(cx)),
            (_, hovered) => root
                .px(px(INK_PAD_X))
                .py(px(INK_PAD_Y))
                .rounded(px(10.))
                .when(hovered, |d| d.bg(backing))
                .child(self.render_list(cx)),
        }
    }
}

/// The open HUD and what it reads from. One per GUI process.
struct HudCtl {
    app: WeakEntity<SeanceApp>,
    main: AnyWindowHandle,
    display: Option<DisplayId>,
    window: Option<WindowHandle<HudView>>,
}

impl Global for HudCtl {}

/// Called once the main window is up: remember it, and open the HUD
/// `seance-hud` asks for. A blank second window gets none.
pub fn start(
    app: WeakEntity<SeanceApp>,
    main: AnyWindowHandle,
    display: Option<DisplayId>,
    cx: &mut App,
) {
    if app.upgrade().is_none_or(|a| a.read(cx).empty_window) {
        return;
    }
    cx.set_global(HudCtl {
        app,
        main,
        display,
        window: None,
    });
    let mode = HudMode::parse(crate::term_config::get().seance_hud.as_deref());
    set_mode(mode, cx);
}

/// Switch the HUD for this session (the Seance menu's Progress HUD items).
pub fn set_mode(mode: HudMode, cx: &mut App) {
    let Some(ctl) = cx.try_global::<HudCtl>() else {
        return;
    };
    let (app, main, display, old) = (ctl.app.clone(), ctl.main, ctl.display, ctl.window);
    if let Some(old) = old {
        let _ = old.update(cx, |_, window, _| window.remove_window());
    }
    cx.global_mut::<HudCtl>().window = None;
    if mode == HudMode::Off || !cfg!(target_os = "macos") {
        return;
    }
    let display = display
        .and_then(|id| cx.find_display(id))
        .or_else(|| cx.primary_display());
    let Some(display) = display else {
        return;
    };
    let area = display.visible_bounds();
    let size = content_size(mode, false, &[]);
    let origin = match mode {
        HudMode::Pips => gpui::point(
            area.origin.x + (area.size.width - px(size.0)) / 2.,
            area.origin.y + px(PIPS_TOP),
        ),
        _ => gpui::point(
            area.origin.x + area.size.width - px(size.0 + MARGIN),
            area.origin.y + area.size.height - px(size.1 + MARGIN),
        ),
    };
    let bounds: Bounds<Pixels> = Bounds {
        origin,
        size: gpui::size(px(size.0), px(size.1)),
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: Some(display.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some("seance".into()),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| HudView::new(mode, app, main, size, window, cx)),
    );
    match opened {
        Ok(handle) => cx.global_mut::<HudCtl>().window = Some(handle),
        Err(e) => eprintln!("[seance hud] open failed: {e:#}"),
    }
}

/// AppKit calls gpui doesn't expose: hide the title buttons and shadow of a
/// borderless panel, show or hide it without activating the app, and resize
/// it around a fixed corner (gpui's resize keeps the bottom-left).
#[cfg(target_os = "macos")]
mod native {
    use std::ffi::{c_char, c_void, CStr};

    use super::HudMode;

    type Id = *mut c_void;
    type Sel = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_msgSend();
        fn sel_registerName(name: *const c_char) -> Sel;
    }

    fn sel(name: &CStr) -> Sel {
        // SAFETY: a NUL-terminated selector name; the runtime interns it.
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// The panel behind a gpui window.
    fn ns_window(window: &gpui::Window) -> Option<Id> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        // SAFETY: `ns_view` is the window's live content view; `-window`
        // takes no arguments and returns an object.
        let w: Id = unsafe {
            let send: unsafe extern "C" fn(Id, Sel) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            send(appkit.ns_view.as_ptr(), sel(c"window"))
        };
        (!w.is_null()).then_some(w)
    }

    pub fn setup(window: &gpui::Window) {
        let Some(w) = ns_window(window) else {
            return;
        };
        // SAFETY: plain AppKit setters on a live NSWindow, each called with
        // the argument types AppKit declares (NSUInteger, BOOL).
        unsafe {
            let send_id_u: unsafe extern "C" fn(Id, Sel, usize) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_bool: unsafe extern "C" fn(Id, Sel, i8) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            // Close, miniaturize, zoom.
            for kind in 0..3 {
                let button = send_id_u(w, sel(c"standardWindowButton:"), kind);
                if !button.is_null() {
                    send_bool(button, sel(c"setHidden:"), 1);
                }
            }
            send_bool(w, sel(c"setHasShadow:"), 0);
            send_bool(w, sel(c"setExcludedFromWindowsMenu:"), 1);
        }
    }

    pub fn set_visible(window: &gpui::Window, on: bool) {
        let Some(w) = ns_window(window) else {
            return;
        };
        // SAFETY: `-orderFrontRegardless` takes nothing; `-orderOut:` takes
        // a sender, nil here. Neither activates the app.
        unsafe {
            if on {
                let send: unsafe extern "C" fn(Id, Sel) =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                send(w, sel(c"orderFrontRegardless"));
            } else {
                let send: unsafe extern "C" fn(Id, Sel, Id) =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                send(w, sel(c"orderOut:"), std::ptr::null_mut());
            }
        }
    }

    /// Resize to `size`, keeping Ink's bottom-right corner or Pips' top
    /// centre where it is. AppKit frames have their origin at the bottom left.
    pub fn resize(window: &gpui::Window, size: (f32, f32), mode: HudMode) {
        let Some(w) = ns_window(window) else {
            return;
        };
        let (nw, nh) = (size.0 as f64, size.1 as f64);
        // SAFETY: `-frame` returns an NSRect (in registers on arm64, through
        // a hidden pointer on x86_64); `-setFrame:display:` takes one and a BOOL.
        unsafe {
            #[cfg(target_arch = "aarch64")]
            let old = {
                let send: unsafe extern "C" fn(Id, Sel) -> NSRect =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                send(w, sel(c"frame"))
            };
            #[cfg(not(target_arch = "aarch64"))]
            let old = {
                extern "C" {
                    fn objc_msgSend_stret();
                }
                let mut out = NSRect {
                    x: 0.,
                    y: 0.,
                    w: 0.,
                    h: 0.,
                };
                let send: unsafe extern "C" fn(*mut NSRect, Id, Sel) =
                    std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
                send(&mut out, w, sel(c"frame"));
                out
            };
            let new = match mode {
                HudMode::Pips => NSRect {
                    x: old.x + (old.w - nw) / 2.,
                    y: old.y + old.h - nh,
                    w: nw,
                    h: nh,
                },
                _ => NSRect {
                    x: old.x + old.w - nw,
                    y: old.y,
                    w: nw,
                    h: nh,
                },
            };
            let send: unsafe extern "C" fn(Id, Sel, NSRect, i8) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            send(w, sel(c"setFrame:display:"), new, 1);
        }
    }
}

/// Off macOS the HUD never opens; these keep the callers platform-free.
#[cfg(not(target_os = "macos"))]
mod native {
    use super::HudMode;

    pub fn setup(_: &gpui::Window) {}
    pub fn set_visible(_: &gpui::Window, _: bool) {}
    pub fn resize(_: &gpui::Window, _: (f32, f32), _: HudMode) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(slug: &str, needs: bool) -> RowIn {
        RowIn {
            slug: slug.into(),
            circle: "L6".into(),
            workspace: "l6".into(),
            text: "build".into(),
            full: "L6 · builder · build".into(),
            needs,
            step: None,
            busy_since: None,
        }
    }

    #[test]
    fn modes_parse_with_off_as_the_default() {
        assert_eq!(HudMode::parse(Some("ink")), HudMode::Ink);
        assert_eq!(HudMode::parse(Some(" Pips ")), HudMode::Pips);
        assert_eq!(HudMode::parse(Some("off")), HudMode::Off);
        assert_eq!(HudMode::parse(Some("loud")), HudMode::Off);
        assert_eq!(HudMode::parse(None), HudMode::Off);
    }

    #[test]
    fn steps_read_n_of_m_from_a_note() {
        assert_eq!(parse_step("build 3/5"), Some((3, 5)));
        assert_eq!(parse_step("review 1 / 2 of PR"), Some((1, 2)));
        assert_eq!(parse_step("tests"), None);
        assert_eq!(parse_step("ratio 9/3 then 2/4"), Some((2, 4)));
        assert_eq!(parse_step("0/0"), None);
    }

    #[test]
    fn elapsed_reads_short() {
        assert_eq!(fmt_elapsed(Duration::from_secs(45)), "45s");
        assert_eq!(fmt_elapsed(Duration::from_secs(4 * 60 + 59)), "4m");
        assert_eq!(fmt_elapsed(Duration::from_secs(3600 + 5 * 60)), "1h05");
    }

    /// Needs-you rows sort first; a row that stops fades for three seconds,
    /// comes back if it starts again, and is gone after that.
    #[test]
    fn rows_sort_needs_first_and_fade_out() {
        let t0 = Instant::now();
        let mut rows = Vec::new();
        merge_rows(&mut rows, vec![row("a", false), row("b", true)], t0);
        let order: Vec<_> = rows.iter().map(|r| r.now.slug.as_str()).collect();
        assert_eq!(order, ["b", "a"]);

        merge_rows(&mut rows, vec![row("b", true)], t0 + Duration::from_secs(1));
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .any(|r| r.now.slug == "a" && r.gone_at.is_some()));

        merge_rows(
            &mut rows,
            vec![row("a", false), row("b", true)],
            t0 + Duration::from_secs(2),
        );
        assert!(rows.iter().all(|r| r.gone_at.is_none()));

        merge_rows(&mut rows, vec![], t0 + Duration::from_secs(3));
        assert_eq!(rows.len(), 2, "still fading");
        merge_rows(&mut rows, vec![], t0 + Duration::from_secs(7));
        assert!(rows.is_empty());
    }

    #[test]
    fn sizes_follow_rows_and_hover() {
        let rows: Vec<Row> = [row("a", false), row("b", true)]
            .into_iter()
            .map(|r| Row {
                now: r,
                since: Instant::now(),
                gone_at: None,
            })
            .collect();
        let (pw, ph) = content_size(HudMode::Pips, false, &rows);
        assert_eq!((pw, ph), (PIP_W + DOT + PIP_GAP + 2. * PILL_PAD_X, PILL_H));
        let (_, ink) = content_size(HudMode::Ink, false, &rows);
        let (_, ink_hover) = content_size(HudMode::Ink, true, &rows);
        assert_eq!(ink_hover - ink, 2. * FULL_H);
        assert!(content_size(HudMode::Pips, true, &rows).0 > pw);
    }
}
