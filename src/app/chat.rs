//! The Chat layout: circles on the left, the selected circle's thread on
//! the right, drawn from Invoke's own history (`chat_api`) so the phone and
//! this window share one conversation. No terminal renders here; "show
//! terminal" goes back to Classic on the circle's lead.
//!
//! A background loop refreshes the circle list and the open thread, then
//! long-polls Invoke's change counter. Rendering reads the last answers.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, img, prelude::*, px, relative, AnyElement, Context, Entity, Focusable as _, Hsla,
    ScrollHandle, SharedString, WeakEntity, Window,
};
use gpui_component::input::{Input, InputEvent, InputState, RopeExt as _};
use gpui_component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_component::text::{TextView, TextViewStyle};
use gpui_component::{StyledExt as _, WindowExt as _};
use regex::Regex;
use seance_core::schemes;

use crate::carve::{self, Cut, Stock};
use crate::chat_api::{
    self, Attachment, CircleRow, Circles, Hit, Msg, PaneRow, Progress, ReplyRef, Thread, Usage,
    UsageWindow,
};
use crate::chat_local;
use crate::events;
use crate::theme::SeancePalette;

use super::header::PlusAction;
use super::lead_tasks::clock;
use super::util::{now_ms, tip, tip_s};
use super::views::ViewMode;
use super::SeanceApp;

/// The long-poll waits this long for Invoke to report a change.
const POLL_SECS: u64 = 25;
/// After a failed round, wait before trying again.
const RETRY: Duration = Duration::from_secs(3);
/// While another layout is up the loop only checks back this often.
const IDLE: Duration = Duration::from_secs(1);
const LIST_W: f32 = 300.;
/// A list row's crew against the header's, as the web Chat scales it.
const LIST_CREW: f32 = 0.62;
/// Invoke's alerts circle is read-only: nothing to say, no lead to open.
const ALERTS_WS: &str = "alerts";
/// A refresh reads this many of the newest messages again, to catch status changes.
const REFRESH_WINDOW: usize = 20;
/// A failed read report waits this long before the next try.
const READ_RETRY: Duration = Duration::from_secs(30);
/// A circle with a message newer than this is Active, as on the phone.
const ACTIVE_MS: u64 = 2 * 3600 * 1000;
/// The pane list for @-mentions is asked again after this.
const PANES_TTL: Duration = Duration::from_secs(30);
/// Within this many points of the bottom, the thread counts as at the bottom.
const NEAR_BOTTOM: f32 = 80.;

/// Follow the bottom as the web app does. `now` is the scroll offset (zero or
/// negative) and the scroll range as the last frame laid them out; `seen` is
/// the same a frame earlier. A change of offset with the range unchanged is
/// Brian scrolling (growth moves the range, not the offset). Returns whether
/// the thread is pinned and whether to scroll it down: only while pinned, and
/// never while `held` (a search result is being shown).
fn follow_step(
    pinned: bool,
    held: bool,
    seen: Option<(f32, f32)>,
    now: (f32, f32),
) -> (bool, bool) {
    let from_bottom = now.1 + now.0;
    let by_hand =
        seen.is_some_and(|(off, max)| (off - now.0).abs() > 0.5 && (max - now.1).abs() < 0.5);
    let grew = seen.is_none_or(|(_, max)| (max - now.1).abs() >= 0.5);
    let mut pinned = if by_hand {
        from_bottom <= NEAR_BOTTOM
    } else {
        pinned
    };
    // A resize or shrink can clamp him onto the bottom without a scroll.
    if !by_hand && from_bottom <= 0.5 {
        pinned = true;
    }
    // Only a change of range scrolls down; a small scroll by hand near the
    // bottom stays where he put it.
    (
        pinned,
        pinned && !held && !by_hand && grew && from_bottom > 0.5,
    )
}

/// A thread draws this many of its newest messages; laying out every bubble
/// of a long thread on each frame cost tens of milliseconds.
const THREAD_WINDOW: usize = 80;
/// "Show earlier" draws this many more.
const THREAD_STEP: usize = 80;

/// The part of a thread that is drawn: its messages from `start` on, under a
/// "Show earlier" row while any come before.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ThreadWindow {
    start: usize,
}

impl ThreadWindow {
    /// The newest `shown` of `len` messages, never fewer than [`THREAD_WINDOW`].
    fn new(len: usize, shown: usize) -> Self {
        Self {
            start: len.saturating_sub(shown.max(THREAD_WINDOW)),
        }
    }

    fn has_earlier(self) -> bool {
        self.start > 0
    }

    /// Message `i`'s place among the scroll's children (after the "Show
    /// earlier" row), or none when it is not drawn.
    fn child(self, i: usize) -> Option<usize> {
        (i >= self.start).then(|| i - self.start + usize::from(self.has_earlier()))
    }
}

/// How many to draw so that message `at` of `len` is among them.
fn widen_to(shown: usize, len: usize, at: usize) -> usize {
    shown.max(THREAD_WINDOW).max(len.saturating_sub(at))
}

/// Where a jump to message `id` lands: how many to draw so it is among them,
/// and its place among the scroll's children. None when the thread does not
/// hold it.
fn jump_plan(shown: usize, ids: &[i64], id: i64) -> Option<(usize, usize)> {
    let at = ids.iter().position(|m| *m == id)?;
    let shown = widen_to(shown, ids.len(), at);
    Some((shown, ThreadWindow::new(ids.len(), shown).child(at)?))
}

/// The message a note in the composer will reply to, and the circle it is in.
#[derive(Clone, Debug, PartialEq)]
struct ReplyDraft {
    ws: String,
    id: i64,
    from: String,
    ts: u64,
    text: String,
}

impl ReplyDraft {
    fn of(ws: &str, m: &Msg) -> Self {
        Self {
            ws: ws.to_string(),
            id: m.id,
            from: m.from.clone(),
            ts: m.ts,
            text: m.text.clone(),
        }
    }

    /// The quote the sent bubble shows until Invoke's own copy arrives.
    fn quote(&self) -> ReplyRef {
        ReplyRef {
            id: self.id,
            from: self.from.clone(),
            ts: self.ts,
            text: self.text.clone(),
        }
    }
}

/// Brian's and the lead's messages can be replied to once Invoke has them;
/// system notes and the read-only alerts circle cannot.
fn can_reply(ws: &str, m: &Msg) -> bool {
    m.id > 0 && ws != ALERTS_WS && (m.from == "me" || m.from == "lead")
}

/// What a Reply sets, from the menu or the arrow alike: nothing for a
/// message that cannot be replied to.
fn reply_draft(ws: &str, m: &Msg) -> Option<ReplyDraft> {
    can_reply(ws, m).then(|| ReplyDraft::of(ws, m))
}

/// Which side of its bubble the reply arrow sits on: the side facing the
/// middle of the thread.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ArrowSide {
    Left,
    Right,
}

fn reply_arrow_side(ws: &str, m: &Msg) -> Option<ArrowSide> {
    can_reply(ws, m).then(|| {
        if m.from == "me" {
            ArrowSide::Left
        } else {
            ArrowSide::Right
        }
    })
}

/// The hover group of a bubble's row, which shows its reply arrow.
const REPLY_GROUP: &str = "chat-bubble-row";

/// The reply arrow beside a bubble. It is laid out all the time, so nothing
/// moves when it shows, and drawn only while the pointer is over its row: a
/// group hover style, which redraws once on enter and once on leave. It sits
/// beside the bubble, never over it; bubbles stop at 72% of the row, so the
/// gutter has room, and in a row too narrow for both it shrinks with them.
pub(crate) fn reply_arrow(id: i64) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("bubble-reply-{id}")))
        .w(px(24.))
        .min_w_0()
        .flex_shrink(1.)
        .overflow_hidden()
        .flex()
        .justify_center()
        .text_sm()
        .text_color(SeancePalette::text_dim())
        .opacity(0.)
        .group_hover(REPLY_GROUP, |s| s.opacity(1.))
        .hover(|s| s.text_color(SeancePalette::flame()))
        .cursor_pointer()
        .child("↩")
}

/// The composer's text with a draft from the sheet page after it: a space
/// between them unless the text is empty or already ends in whitespace.
fn with_draft(typed: &str, draft: &str) -> String {
    if typed.is_empty() || typed.ends_with(char::is_whitespace) {
        format!("{typed}{draft}")
    } else {
        format!("{typed} {draft}")
    }
}

/// Opening a circle drops a reply meant for another one.
fn reply_after_select(cur: Option<ReplyDraft>, ws: &str) -> Option<ReplyDraft> {
    cur.filter(|r| r.ws == ws)
}

/// A send that went through clears the reply it carried; a failed one, or a
/// reply picked since, stays.
fn reply_after_send(cur: Option<ReplyDraft>, sent: Option<i64>, ok: bool) -> Option<ReplyDraft> {
    cur.filter(|r| !(ok && sent == Some(r.id)))
}

/// Who a quote names, as the phone does.
fn quote_label(from: &str) -> &'static str {
    if from == "me" {
        "You"
    } else {
        "Lead"
    }
}

/// A quote's words on one line.
fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        "Photo or file".to_string()
    } else {
        line
    }
}

/// The most the time and a waiting tick in a recolored bubble move from the
/// ink toward the fill: softer, but never under 4.5:1 on the fill.
const SOFT_INK: f32 = 0.2;
/// How far a quote's fill in a recolored bubble darkens: toward a dark ink,
/// toward black under a light one, so the ink holds 4.5:1 on it.
const QUOTE_TINT: f32 = 0.12;
/// How far the selected circle row's wash moves from the rail to the
/// recolored bubble's fill.
const ROW_WASH: f32 = 0.22;
/// Width of the bar on the selected circle row's left edge.
const ROW_BAR: f32 = 3.;

/// Brian's bubble on a scheme that recolors it (`Scheme::me_bubble`), as
/// packed hex: the fill, the ink, the softer ink for the time and a waiting
/// tick, and a quote's fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeInk {
    pub bg: u32,
    pub ink: u32,
    pub soft: u32,
    pub quote_bg: u32,
}

/// The recolored look for a bubble, or `None` for the palette's own: only
/// Brian's bubbles, and only on a scheme that has one.
pub(crate) fn me_ink(me: bool, custom: Option<schemes::MeBubble>) -> Option<MeInk> {
    let b = custom.filter(|_| me)?;
    Some(MeInk {
        bg: b.bg,
        ink: b.ink,
        soft: soft_ink(b.ink, b.bg),
        quote_bg: schemes::mix(
            b.bg,
            if schemes::is_lighter(b.ink, b.bg) {
                0x000000
            } else {
                b.ink
            },
            QUOTE_TINT,
        ),
    })
}

/// The ink moved toward the fill by up to [`SOFT_INK`], less where a full
/// step would drop it under 4.5:1 (a light ink on a mid fill has little room).
fn soft_ink(ink: u32, bg: u32) -> u32 {
    (1..=(SOFT_INK * 100.) as u32)
        .rev()
        .map(|n| schemes::mix(ink, bg, n as f32 / 100.))
        .find(|&c| schemes::contrast(c, bg) >= 4.5)
        .unwrap_or(ink)
}

/// A bubble's time without a recolor. On a skin, `faint` sinks into the
/// bubble (about 1.1:1 on Castle's blue-gray), so Brian's takes the text color
/// and the lead's `dim`.
fn plain_stamp(me: bool, skinned: bool, text: Hsla, dim: Hsla, faint: Hsla) -> Hsla {
    match (skinned, me) {
        (true, true) => text,
        (true, false) => dim,
        (false, _) => faint,
    }
}

/// Whether `s` is carved stone (castle), whatever the shots draw.
fn stone(s: &schemes::Scheme) -> bool {
    s.texture.as_ref().and_then(|t| t.carved) == Some(schemes::Carve::Stone)
}

/// The plate whose wash and bar mark the selected circle row: Brian's
/// recolored one, except on stone, which frames the row with its chain.
pub(crate) fn row_plate(s: &schemes::Scheme) -> Option<schemes::MeBubble> {
    s.me_bubble.filter(|_| !stone(s))
}

/// The selected circle row's fill: the rail's `surface`, or the recolored
/// bubble's fill washed thin where the scheme has one, so the row stands out
/// on a textured rail.
pub(crate) fn selected_row_bg(custom: Option<schemes::MeBubble>) -> Hsla {
    match custom {
        Some(b) => hex(b.bg).opacity(ROW_WASH),
        None => SeancePalette::surface(),
    }
}

/// The bar on a selected circle row's left edge where the scheme recolors
/// the bubble. It is absolute, so the row's contents stay where they were;
/// the row must be `relative`.
pub(crate) fn selected_row_bar(custom: Option<schemes::MeBubble>) -> Option<gpui::Div> {
    let b = custom?;
    Some(
        div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(ROW_BAR))
            .bg(hex(b.bg)),
    )
}

/// [`me_ink`] for the active scheme: Brian's plate, or the lead's.
pub(crate) fn active_me_ink(me: bool) -> Option<MeInk> {
    let s = crate::theme::active();
    if carve::today() && stone(s) {
        return None;
    }
    me_ink(true, if me { s.me_bubble } else { s.lead_bubble })
}

/// A bubble's fill and text color: the recolored ones when `ink` is set,
/// else the palette's (`me_bg` for Brian, `lead_bg` for the lead, `text`).
pub(crate) fn bubble_fill(
    me: bool,
    ink: Option<MeInk>,
    me_bg: Hsla,
    lead_bg: Hsla,
    text: Hsla,
) -> (Hsla, Hsla) {
    match ink {
        Some(i) => (hex(i.bg), hex(i.ink)),
        None => (if me { me_bg } else { lead_bg }, text),
    }
}

pub(crate) fn hex(c: u32) -> Hsla {
    gpui::rgb(c).into()
}

/// Lays out `child` with the theme's link color set to `link`, then puts it
/// back: the text view colors links from the theme alone, and the theme's
/// link color cannot be read on a recolored bubble.
pub(crate) struct LinkInk {
    pub child: AnyElement,
    pub link: Hsla,
}

impl LinkInk {
    fn with<R>(link: Hsla, cx: &mut gpui::App, f: impl FnOnce(&mut gpui::App) -> R) -> R {
        let was = std::mem::replace(&mut gpui_component::Theme::global_mut(cx).link, link);
        let out = f(cx);
        gpui_component::Theme::global_mut(cx).link = was;
        out
    }
}

impl IntoElement for LinkInk {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for LinkInk {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        let child = &mut self.child;
        let id = Self::with(self.link, cx, |cx| child.request_layout(window, cx));
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        let child = &mut self.child;
        Self::with(self.link, cx, |cx| child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        self.child.paint(window, cx);
    }
}

/// The quote on top of a reply's bubble: who wrote the original and up to
/// three lines of it. In a recolored bubble it is a darker tint in the ink.
pub(crate) fn quote_block(from: &str, text: &str, ink: Option<MeInk>) -> gpui::Div {
    let (bar, words, fill) = match ink {
        Some(i) => (hex(i.ink), hex(i.ink), hex(i.quote_bg)),
        None => (
            SeancePalette::violet(),
            SeancePalette::text_dim(),
            SeancePalette::bg().opacity(0.3),
        ),
    };
    div()
        .flex()
        .flex_col()
        .mb_1()
        .pl_2()
        .py_0p5()
        .map(|d| carve::corners(d, Stock::Sm))
        .border_l_2()
        .border_color(bar)
        .bg(fill)
        .text_xs()
        .child(
            div()
                .font_semibold()
                .text_color(words)
                .child(quote_label(from)),
        )
        .child(
            div()
                .text_color(words)
                .line_clamp(3)
                .text_ellipsis()
                .child(text.to_string()),
        )
}

/// The bar above the composer while a reply is set: the quoted words on one
/// line, then `cancel`.
pub(crate) fn reply_bar(text: &str, cancel: impl IntoElement) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1p5()
        .border_t_1()
        .border_color(SeancePalette::border())
        .bg(SeancePalette::bg_elevated())
        .text_xs()
        .text_color(SeancePalette::text_dim())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pl_2()
                .border_l_2()
                .border_color(SeancePalette::violet())
                .truncate()
                .child(one_line(text)),
        )
        .child(cancel)
}

/// How many to draw after a click on "Show earlier".
fn show_earlier(shown: usize, len: usize) -> usize {
    (shown.max(THREAD_WINDOW) + THREAD_STEP).min(len.max(THREAD_WINDOW))
}

/// How many to draw once `added` messages have come in. Pinned to the
/// bottom, the window goes back to the newest [`THREAD_WINDOW`] (he is taken
/// to the bottom anyway); reading older ones (`held`), it grows so nothing
/// above him drops out and moves what he is reading.
fn shown_after_arrival(shown: usize, added: usize, held: bool) -> usize {
    if held {
        shown.max(THREAD_WINDOW) + added
    } else {
        THREAD_WINDOW
    }
}

/// Sits `below` points above the message that was first before "Show
/// earlier", for the scroll to put at the top edge. Takes no room: its
/// margins and the thread's `gap` after it cancel.
fn earlier_marker(below: f32, gap: f32) -> gpui::Div {
    div()
        .flex_none()
        .h(px(0.))
        .mt(px(-below))
        .mb(px(below - gap))
}

/// Crew dances step at about 12 frames a second from a timer: every frame
/// rebuilds the whole window, and they read fine at this rate.
const DANCE_TICK: Duration = Duration::from_millis(83);

/// What keeps the Chat view redrawing on its own.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Frames {
    /// Nothing moves: redraw only when something changes.
    #[default]
    Idle,
    /// A crew dances: the dance timer redraws at [`DANCE_TICK`].
    Dance,
    /// The party runs: every frame, for its few seconds.
    Smooth,
}

/// Frames for this render. Nothing at all in a window nobody is looking at.
fn chat_frames(crew: bool, party: bool, list_dancing: bool, window_active: bool) -> Frames {
    if !window_active {
        Frames::Idle
    } else if party {
        Frames::Smooth
    } else if crew || list_dancing {
        Frames::Dance
    } else {
        Frames::Idle
    }
}

/// Search runs this long after the last keystroke.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);
/// Fewest characters a search is run for (Invoke answers nothing for one).
const SEARCH_MIN: usize = 2;
/// What a click on the header's creature set going: its circle, the move it
/// plays (none for a party) from `at`, and the line in its bubble.
struct Egg {
    ws: String,
    mv: Option<usize>,
    at: u64,
    line: &'static str,
}

/// What the quick-reply chips above an empty composer send.
const QUICK_REPLIES: [&str; 3] = ["Keep going", "Status?", "Ship it"];

/// Chat's decorative torches on a torch scheme: by the mark, on the header
/// wall, by the task count, a standing one at the foot of the list rail and
/// one over Send. All burn calm, each on its own seed, at the spinner tick's
/// frame. Each sits in an absolute layer with no handlers, so it takes no
/// clicks and no cursor; one that would cover text is left out, never moved.
pub(crate) mod decor {
    use gpui::{div, px, Div, ParentElement as _, Styled as _};

    use crate::torch::{self, Torch};

    /// One seed name per torch. An Idle frame depends only on the frame plus
    /// `seed % 12`, so these are picked with twelve distinct phases apart;
    /// two sharing a phase would flicker in step.
    pub(crate) const SEEDS: [&str; 9] = [
        "chat-decor-mark",
        "chat-decor-rail",
        "chat-decor-tasks",
        "chat-decor-send",
        "chat-decor-wall-0",
        "chat-decor-wall-1",
        "chat-decor-wall-2",
        "chat-decor-wall-3",
        "chat-decor-wall-4",
    ];

    /// A small calm torch, 12x20, in an absolute layer.
    fn small(name: &str, frame: u64) -> Div {
        div()
            .absolute()
            .child(torch::element(Torch::Idle, frame, torch::seed(name), 1.0))
    }

    /// Just right of the "seance" mark, in the rail's top row.
    pub(crate) fn by_mark(frame: u64) -> Div {
        small(SEEDS[0], frame).left(px(88.)).top(px(10.))
    }

    /// The standing torch at the foot of the list rail, in the rows' right
    /// padding against the divider, 60 points tall.
    pub(crate) fn rail_post(frame: u64) -> Div {
        div()
            .absolute()
            .right_0()
            .bottom(px(6.))
            .child(torch::tall_element(frame, torch::seed(SEEDS[1]), 1.5))
    }

    /// Just left of the task count chip, in the header's gap before it:
    /// a child of the chip, so it follows the chip wherever it sits.
    pub(crate) fn by_tasks(frame: u64) -> Div {
        small(SEEDS[2], frame).left(px(-12.)).top(px(-1.))
    }

    /// Over the Send button, in the quick-reply row's right end.
    pub(crate) fn over_send(frame: u64) -> Div {
        small(SEEDS[3], frame).right(px(34.)).top(px(12.))
    }

    /// The torch over Send fits while the row above the input shows its
    /// usual chips (quick replies, Stop) and leaves their room free; @
    /// suggestions can fill the row, and with no row the spot is the input.
    pub(crate) fn over_send_fits(composer_w: f32, row_shown: bool, mentioning: bool) -> bool {
        row_shown && !mentioning && composer_w >= 360.
    }

    /// The header wall's five torches around its centre, uneven, as
    /// (left, top) from an anchor at this share of the header's width.
    const WALL_AT: f32 = 0.38;
    const WALL: [(f32, f32); 5] = [(-22., 9.), (-9., 2.), (4., 13.), (16., 4.), (29., 11.)];

    /// Where the wall torches go in a header `header_w` wide whose text ends
    /// at `text_end` and whose chips start at `chips_at`: those that would
    /// reach within 8 points of either are left out. (index, left, top).
    pub(crate) fn wall_spots(
        header_w: f32,
        text_end: f32,
        chips_at: f32,
    ) -> Vec<(usize, f32, f32)> {
        let anchor = header_w * WALL_AT;
        WALL.iter()
            .enumerate()
            .map(|(i, &(dx, top))| (i, anchor + dx, top))
            .filter(|&(_, x, _)| x >= text_end + 8. && x + torch::W as f32 <= chips_at - 8.)
            .collect()
    }

    pub(crate) fn wall(frame: u64, header_w: f32, text_end: f32, chips_at: f32) -> Vec<Div> {
        wall_spots(header_w, text_end, chips_at)
            .into_iter()
            .map(|(i, x, top)| small(SEEDS[4 + i], frame).left(px(x)).top(px(top)))
            .collect()
    }

    /// A generous guess at where the header's text ends: padding, the lead's
    /// torch, the creature (or crew), then the longer of name and doing line
    /// at a wide average glyph.
    pub(crate) fn header_text_end(lead_torch: bool, crew: usize, name: &str, line: &str) -> f32 {
        let torch = if lead_torch { 18. + 12. } else { 0. };
        let creature = 30. * crew.max(1) as f32 + 12.;
        let text = (name.chars().count() as f32 * 8.).max(line.chars().count() as f32 * 7.);
        12. + torch + creature + text
    }

    /// Where the header's chips start: the task count and "show terminal",
    /// with padding and gaps, or just the padding on a read-only circle.
    pub(crate) fn header_chips_at(header_w: f32, chips: bool) -> f32 {
        header_w - if chips { 200. } else { 12. }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum VideoState {
    Downloading,
    Failed(String),
}

/// Pictures a lead sends as a note can also be renders. Those are not pictures:
/// no download until someone asks, and then with time to spare.
fn is_video(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp4" | "webm" | "mov" | "m4v"
    )
}

/// A video's download may take this long.
const VIDEO_SECS: u64 = 900;

#[derive(Default)]
pub(super) struct ChatState {
    circles: Vec<CircleRow>,
    pub(super) selected: Option<String>,
    threads: HashMap<String, Thread>,
    progress: HashMap<String, Progress>,
    /// Invoke's change counter as of the last round.
    v: u64,
    input: Option<Entity<InputState>>,
    scroll: ScrollHandle,
    /// Frames left to pin the thread to its newest message.
    stick: u8,
    /// Brian scrolled up to read: new messages and growth leave the thread
    /// where it is until he is back near the bottom or opens a circle.
    unpinned: bool,
    /// Scroll offset and range as the last frame saw them, to tell his
    /// scrolling from the content growing.
    seen: Option<(f32, f32)>,
    /// Highest id Invoke confirmed read, per circle.
    read_sent: HashMap<String, i64>,
    /// Circles with a read report in flight.
    read_busy: HashSet<String>,
    /// A failed read report is not retried before this.
    read_retry_at: HashMap<String, Instant>,
    /// Whether this window was the active one at the last frame.
    window_active: bool,
    /// The header crew's run of moves, while it shows.
    crew_dance: Option<super::sprite::CrewDance>,
    /// Each list row's crew dance by circle, apart from the header's.
    list_dances: HashMap<String, super::sprite::CrewDance>,
    /// Rows whose crew dances this frame: those scrolled into view.
    list_dancing: HashSet<String>,
    list_scroll: ScrollHandle,
    /// Counts down from zero: ids for bubbles shown before Invoke has them.
    next_local: i64,
    /// Text of a send that failed, to put back in the composer.
    restore: Option<String>,
    /// Focus the composer at the next frame (switching into Chat).
    pub(super) focus_input: bool,
    /// Drafts from the sheet page by circle, added to the composer once that
    /// circle shows.
    drafts: HashMap<String, String>,
    /// Image name to its cached file, once fetched.
    images: HashMap<String, (PathBuf, u32, u32)>,
    /// Videos being fetched on a click, or that failed to be.
    videos: HashMap<String, VideoState>,
    /// Choices tapped here, by the message they answer, until Invoke's own
    /// copy of the thread says so too.
    tapped: HashMap<i64, String>,
    image_asked: HashSet<String>,
    error: Option<String>,
    /// Claude plan usage from the last thread answer; none hides the strip.
    usage: Option<Usage>,
    search: Option<Entity<InputState>>,
    /// The masked box on a sign-in card. Its text is cleared the moment it is
    /// sent and is never kept anywhere else.
    code_input: Option<Entity<InputState>>,
    /// Sign-in cards whose code was sent: they say "Sent", nothing more.
    code_sent: HashSet<i64>,
    /// What the search found; none while the box is empty (the circles show).
    hits: Option<Vec<Hit>>,
    /// Counts keystrokes in the search box, so only the last one searches.
    search_seq: u64,
    /// Under the search box: "No results", or why the search failed.
    search_note: Option<String>,
    /// A message to scroll to once its thread is showing.
    jump: Option<i64>,
    /// The message the composer's next note replies to.
    reply: Option<ReplyDraft>,
    /// A circle scrolled to a search result: new messages do not pin it to
    /// the bottom until Brian sends something or opens another circle.
    no_pin: Option<String>,
    /// A circle marked unread while open: not read again until selected.
    hold_read: Option<String>,
    /// Each circle's panes for @-mentions, and when they were asked for.
    panes: HashMap<String, Vec<PaneRow>>,
    panes_at: HashMap<String, Instant>,
    /// Recent clicks on the header's creature, and what the last one did.
    pokes: super::sprite::Pokes,
    egg: Option<Egg>,
    /// The five-click party, while it runs.
    party: Option<super::sprite::Party>,
    /// Where the header's creature was last painted: a click there during a
    /// party is one more poke, ignored, not the click that ends it.
    creature_at: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// The thread column's width as last laid out (a dock or drawer beside
    /// Chat narrows it), for the decorative torches' room checks.
    thread_w: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    /// When Compact was last clicked, for its cooldown.
    compact_at: Option<(String, Instant)>,
    /// Why the last action Brian took was refused; a click clears it.
    notice: Option<String>,
    /// How many of the open thread's newest messages are drawn (at least
    /// [`THREAD_WINDOW`]); back to that on opening a circle.
    shown: usize,
    /// A "Show earlier" click: the message that was first, and how far below
    /// the thread's top edge it sat, to keep it there once more are drawn.
    earlier: Option<(i64, f32)>,
    /// Frames the follow-the-bottom step sits out after "Show earlier".
    hold_follow: u8,
    /// What the last render asked of the dance timer.
    frames: Frames,
    /// The dance timer is running.
    dance_ticking: bool,
}

impl ChatState {
    /// A circle just made in Chat is the one open, whatever was open before.
    pub(super) fn open_new_circle(&mut self, ws: &str) {
        self.selected = Some(ws.to_string());
        self.arrive();
        self.notice = None;
    }

    /// Newest message Invoke has given us for `ws`: a refresh asks only for
    /// what comes after it.
    fn last_id(&self, ws: &str) -> Option<i64> {
        self.threads
            .get(ws)?
            .messages
            .iter()
            .map(|m| m.id)
            .filter(|id| *id > 0)
            .max()
    }

    /// Where a refresh of `ws` starts: the last `REFRESH_WINDOW` messages again,
    /// not only the new ones, because a note's status moves on after it is sent
    /// (sending, got, answered) and a delta of new messages alone would never
    /// say so. A short thread is read whole.
    fn refresh_after(&self, ws: &str) -> Option<i64> {
        let mut ids: Vec<i64> = self
            .threads
            .get(ws)?
            .messages
            .iter()
            .map(|m| m.id)
            .filter(|id| *id > 0)
            .collect();
        ids.sort_unstable();
        ids.len().checked_sub(REFRESH_WINDOW + 1).map(|at| ids[at])
    }

    /// Coming back to Chat: open at the newest message again.
    pub(super) fn arrive(&mut self) {
        self.stick = 3;
        self.unpinned = false;
        self.no_pin = None;
        self.jump = None;
        self.shown = THREAD_WINDOW;
        self.earlier = None;
        self.hold_follow = 0;
    }

    pub(super) fn any_working(&self) -> bool {
        self.circles.iter().any(|c| c.working)
    }
}

/// One round's answers; a failed call leaves its part as it was.
struct Round {
    circles: Option<Circles>,
    /// The thread, and whether it holds only the messages past an id.
    thread: Option<(String, Thread, bool)>,
    progress: Option<(String, Progress)>,
    error: Option<String>,
    /// The local source found nothing new, so the view need not repaint.
    quiet: bool,
}

fn fetch_round(ws: Option<String>, after: Option<i64>) -> Round {
    let mut round = Round {
        circles: None,
        thread: None,
        progress: None,
        error: None,
        quiet: false,
    };
    match chat_api::circles() {
        Ok(c) => round.circles = Some(c),
        Err(e) => round.error = Some(e.to_string()),
    }
    if let (Some(ws), None) = (ws, &round.error) {
        round.thread = chat_api::thread(&ws, after)
            .ok()
            .map(|t| (ws.clone(), t, after.is_some()));
        round.progress = chat_api::progress(&ws).ok().map(|p| (ws, p));
    }
    round
}

/// One round of the no-server source: the circles, and the open one's
/// conversation as a whole thread (it replaces what was shown).
fn fetch_local(ask: chat_local::LocalAsk) -> Round {
    let mut round = Round {
        circles: None,
        thread: None,
        progress: None,
        error: None,
        quiet: false,
    };
    match chat_local::round(ask) {
        Ok(r) => {
            round.circles = Some(r.circles);
            round.thread = r.thread.map(|(ws, t)| (ws, t, false));
            round.quiet = r.quiet;
        }
        Err(e) => round.error = Some(e.to_string()),
    }
    round
}

fn url_re() -> &'static Regex {
    static URL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    URL.get_or_init(|| Regex::new(r"https?://[^\s<>]+").unwrap())
}

/// A link's end: trailing punctuation is the sentence's, not the URL's.
fn trim_link(url: &str) -> &str {
    url.trim_end_matches(['.', ',', ')', ']', ';', ':', '!', '?'])
}

/// A message as markdown that reads back as the same text: every ASCII
/// punctuation mark escaped, every line its own paragraph (the renderer drops
/// markdown's hard breaks, and the view draws paragraphs with no gap), leading spaces kept (as
/// non-breaking ones, so indentation survives and four of them are not a code
/// block), URLs turned into links. The selectable text view draws it, so the
/// bubble keeps its look and gains selection, copy and clickable links.
fn to_markdown(text: &str) -> String {
    const NBSP: char = '\u{a0}';
    let escape = |s: &str| -> String {
        s.chars()
            .flat_map(|c| {
                let esc = c.is_ascii_punctuation().then_some('\\');
                esc.into_iter().chain(std::iter::once(c))
            })
            .collect()
    };
    let mut lines: Vec<&str> = text.lines().collect();
    // A message does not end in blank space.
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
        .into_iter()
        .map(|line| {
            if line.trim().is_empty() {
                // A blank line still takes its height.
                return NBSP.to_string();
            }
            let body = line.trim_start_matches([' ', '\t']);
            let indent: String = line[..line.len() - body.len()]
                .chars()
                .flat_map(|c| std::iter::repeat_n(NBSP, if c == '\t' { 4 } else { 1 }))
                .collect();
            let (mut out, mut at) = (indent, 0);
            for m in url_re().find_iter(body) {
                let link = trim_link(m.as_str());
                out.push_str(&escape(&body[at..m.start()]));
                out.push_str(&format!("[{}](<{link}>)", escape(link)));
                at = m.start() + link.len();
            }
            out.push_str(&escape(&body[at..]));
            out
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The file paths in `text` (whitespace-separated words starting `/` or
/// `~/`, wrapping quotes and trailing punctuation left out), with where each
/// sits in it.
fn path_spans(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut spans = Vec::new();
    let mut at = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        let start = at;
        at += word.len();
        let lead = word.len()
            - word
                .trim_start_matches(['(', '[', '<', '"', '\'', '`'])
                .len();
        let token = word
            .trim_start_matches(['(', '[', '<', '"', '\'', '`'])
            .trim_end_matches(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        ')' | ']' | '>' | '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?'
                    )
            });
        if (token.starts_with('/') || token.starts_with("~/")) && token.len() > 1 {
            let from = start + lead;
            spans.push((from..from + token.len(), token));
        }
    }
    spans
}

/// The path a double-clicked `word` belongs to, if it can only be that one:
/// the word has a letter or digit in it (gpui picks a lone "/" or "." on
/// punctuation), every place it appears in the message is inside a path, and
/// those places name one path. "png" in "I put the png in ~/out/a.png" is a
/// word of the sentence, not a click on the file.
fn path_of_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    if !word.chars().any(char::is_alphanumeric) {
        return None;
    }
    let spans = path_spans(text);
    let mut found: Option<&str> = None;
    for (at, _) in text.match_indices(word) {
        let end = at + word.len();
        let (_, path) = spans.iter().find(|(r, _)| r.start <= at && end <= r.end)?;
        match found {
            Some(p) if p != *path => return None,
            _ => found = Some(path),
        }
    }
    found
}

/// Double-click in a bubble: the text view has just selected a word. If it sits
/// in a file path of the message that exists here, open it the way the
/// terminal does: images, videos and pages in their app, anything else shown
/// in Finder.
fn open_path_under_double_click(text: &str, window: &mut Window, cx: &mut gpui::App) {
    let word = window.selected_text(cx);
    let Some(printed) = path_of_word(text, word.trim()) else {
        return;
    };
    let Some(path) = crate::remote_term_view::existing_path(printed) else {
        return;
    };
    if path.is_file() && crate::remote_term_view::opens_in_app(&path) {
        crate::sysopen::open_detached(&path.to_string_lossy());
    } else {
        crate::sysopen::reveal_detached(&path);
    }
}

/// One-line preview of a message for the circle list.
fn preview(m: &Msg) -> String {
    let text = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = if text.is_empty() && !m.image_names().is_empty() {
        "📷 photo".to_string()
    } else {
        text
    };
    if m.from == "me" {
        format!("You: {text}")
    } else {
        text
    }
}

/// Circles Invoke's phone list shows under Active: not alerts, with unread
/// messages or a last message under two hours old, newest first.
fn active_circles(circles: &[CircleRow], now: u64) -> Vec<CircleRow> {
    let mut active: Vec<CircleRow> = circles
        .iter()
        .filter(|c| {
            c.ws != ALERTS_WS
                && (c.unread > 0
                    || c.last
                        .as_ref()
                        .is_some_and(|m| now.saturating_sub(m.ts) < ACTIVE_MS))
        })
        .cloned()
        .collect();
    active.sort_by_key(|c| std::cmp::Reverse(c.last.as_ref().map_or(0, |m| m.ts)));
    active
}

/// 12-hour clock, as the phone shows it: "12:50 PM", or with a weekday and
/// no minutes for the weekly reset: "Thu 2 AM".
fn reset_label(ts_ms: u64, with_day: bool) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let (day, hour, minute) = events::local_clock(ts_ms);
    let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    let ampm = if hour < 12 { "AM" } else { "PM" };
    if with_day {
        format!("{} {h12} {ampm}", DAYS[day as usize % 7])
    } else {
        format!("{h12}:{minute:02} {ampm}")
    }
}

/// Context use from which the number is yellow, and from which it is orange
/// and Compact is offered.
const CTX_YELLOW: u32 = 30;
const CTX_ORANGE: u32 = 50;
/// A Compact click is not taken again for this long.
const COMPACT_COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Heat {
    Calm,
    Yellow,
    Orange,
}

impl Heat {
    fn color(self) -> gpui::Hsla {
        match self {
            Self::Calm => SeancePalette::text_faint(),
            Self::Yellow => gpui::rgb(0xe8c34a).into(),
            Self::Orange => gpui::rgb(0xff9f43).into(),
        }
    }
}

fn ctx_heat(percent: u32) -> Heat {
    match percent {
        p if p >= CTX_ORANGE => Heat::Orange,
        p if p >= CTX_YELLOW => Heat::Yellow,
        _ => Heat::Calm,
    }
}

/// The thin usage line under the thread header, like the phone's: "5h 18% ·
/// ⟳ 12:50 PM   7d 35% · ⟳ Thu 2 AM   ctx 28%". Numbers go red from 80%, the
/// last (ctx, the lead's own context and not the plan's) turns yellow from 30%
/// and orange from 50%, never red; from 50% `compact` (a button) follows it.
/// None when nothing is known.
fn usage_strip(
    u: Option<&Usage>,
    context: Option<u32>,
    compact: Option<AnyElement>,
) -> Option<AnyElement> {
    let part = |label: &str, used: f64, color: gpui::Hsla, resets: Option<(u64, bool)>| {
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(label.to_string())
            .child(
                div()
                    .font_semibold()
                    .text_color(color)
                    .child(format!("{}%", used.round() as i64)),
            )
            .children(resets.map(|(at, with_day)| format!("· ⟳ {}", reset_label(at, with_day))))
    };
    let window = |label: &str, w: &UsageWindow, with_day: bool| {
        let color = if w.used >= 80. {
            SeancePalette::danger()
        } else {
            SeancePalette::text_faint()
        };
        part(label, w.used, color, w.resets.map(|at| (at, with_day)))
    };
    let parts: Vec<_> = [
        u.and_then(|u| u.five_hour.as_ref())
            .map(|w| window("5h", w, false)),
        u.and_then(|u| u.seven_day.as_ref())
            .map(|w| window("7d", w, true)),
        context.map(|c| {
            part("ctx", f64::from(c), ctx_heat(c).color(), None)
                .when(c >= CTX_ORANGE, |d| d.children(compact))
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        return None;
    }
    Some(
        div()
            .flex_none()
            .py_1()
            .flex()
            .justify_center()
            .gap_4()
            .text_xs()
            .text_color(SeancePalette::text_faint())
            .children(parts)
            .into_any_element(),
    )
}

/// The bubble's text keeps the bubble's own look: no gap between lines and the
/// colors and size of the bubble around it.
fn bubble_text_style() -> TextViewStyle {
    TextViewStyle {
        paragraph_gap: gpui::rems(0.),
        is_dark: !crate::theme::is_light(),
        ..Default::default()
    }
}

/// A picture in a bubble is at most this wide and this tall, at its own shape;
/// a box this tall stands in until the file has arrived.
const IMG_MAX_W: f32 = 320.;
const IMG_MAX_H: f32 = 420.;
const IMG_LOADING_H: f32 = 240.;

/// The box a `w` by `h` picture takes in a bubble: its own shape, shrunk to fit
/// the limits, never enlarged. An unknown size gets the loading box.
fn fit_image(w: u32, h: u32) -> (f32, f32) {
    if w == 0 || h == 0 {
        return (IMG_MAX_W, IMG_LOADING_H);
    }
    let (w, h) = (w as f32, h as f32);
    let scale = (IMG_MAX_W / w).min(IMG_MAX_H / h).min(1.);
    ((w * scale).round().max(1.), (h * scale).round().max(1.))
}

/// The tick for a message, as (glyph, accent color, tooltip): Brian's notes by
/// status, a lead's messages once their id is at or under what Brian has read.
fn tick_for(m: &Msg, read: i64) -> Option<(&'static str, bool, &'static str)> {
    match (m.from.as_str(), m.status.as_str()) {
        ("me", "sending") => Some(("✓", false, "Waiting for the lead")),
        ("me", "got") => Some(("✓✓", true, "The lead has seen it")),
        ("me", "answered") => Some(("✓✓", true, "Answered")),
        ("lead", _) if m.id > 0 && m.id <= read => Some(("✓✓", true, "Seen")),
        _ => None,
    }
}

/// One child of the circle list.
enum ListItem {
    Label(String),
    /// A space's heading: a circle dropped on it moves there.
    Space {
        id: String,
        name: String,
    },
    Circle(CircleRow),
}

/// A list heading: a space's name, or Active.
/// The spaces that have no circle in the list, in rail order.
fn empty_spaces<'a>(
    spaces: impl Iterator<Item = (&'a str, &'a str)>,
    shown: &std::collections::HashSet<String>,
) -> Vec<(String, String)> {
    spaces
        .filter(|(id, _)| !shown.contains(*id))
        .map(|(id, name)| (id.to_string(), name.to_string()))
        .collect()
}

/// The spaces a circle can move to: every one but where it is.
fn move_targets<'a>(
    spaces: impl Iterator<Item = (&'a str, &'a str)>,
    here: &str,
) -> Vec<(String, String)> {
    spaces
        .filter(|(id, _)| *id != here)
        .map(|(id, name)| (id.to_string(), name.to_string()))
        .collect()
}

fn list_label(text: String) -> gpui::Div {
    div()
        .px_3()
        .pt_2()
        .pb_1()
        .text_xs()
        .text_color(SeancePalette::text_faint())
        .child(text)
}

/// The reminder choices, as the menu words them and as Invoke's `label` does.
const REMIND_CHOICES: [(&str, &str); 4] = [
    ("In 1 hour", "in 1 hour"),
    ("In 3 hours", "in 3 hours"),
    ("Tomorrow at 9 AM", "tomorrow at 9 AM"),
    ("Monday at 9 AM", "Monday at 9 AM"),
];

/// When each of `REMIND_CHOICES` falls (unix ms), by this machine's clock.
/// The 9 AM ones start from a point at midday so a clock change cannot move
/// them to the wrong day.
fn remind_times(now: u64) -> [u64; 4] {
    const HOUR: u64 = 3600 * 1000;
    let today = events::local_day_start_ms(now);
    // Midnight plus 9 hours, moved back onto 9:00 on a 23 or 25 hour day.
    let nine = |probe: u64| {
        let at = events::local_day_start_ms(probe) + 9 * HOUR;
        let hour = u64::from(events::local_clock(at).1);
        (at + 9 * HOUR).saturating_sub(hour * HOUR)
    };
    let monday = (1..=7)
        .map(|d| today + d * 24 * HOUR + 12 * HOUR)
        .find(|probe| events::local_clock(*probe).0 == 1)
        .map_or(0, nine);
    [now + HOUR, now + 3 * HOUR, nine(today + 36 * HOUR), monday]
}

/// A success toast on the window the menu was used in, as the other menus
/// show one.
fn toast(win: gpui::AnyWindowHandle, cx: &mut gpui::App, msg: String) {
    let _ = win.update(cx, |_, window, cx| {
        window.push_notification(gpui_component::notification::Notification::success(msg), cx)
    });
}

/// The words to search for, or why not: nothing typed (no message) and too
/// short (a hint to show).
fn search_query(raw: &str) -> Result<&str, Option<&'static str>> {
    let q = raw.trim();
    match q.chars().count() {
        0 => Err(None),
        n if n < SEARCH_MIN => Err(Some("Type at least 2 characters")),
        _ => Ok(q),
    }
}

/// A spec card's body: the text without its first line, which is the title.
fn spec_body(text: &str) -> &str {
    text.split_once('\n')
        .map_or("", |(_, rest)| rest.trim_start_matches('\n'))
}

/// Change on a spec card sends nothing: it hands the keyboard to the composer.
fn changes_spec(m: &Msg, choice: &str) -> bool {
    m.spec.is_some() && choice.eq_ignore_ascii_case("change")
}

/// A note that is only its document: the server words it "📎 <title>".
fn attachment_only(m: &Msg) -> bool {
    m.attachment
        .as_ref()
        .is_some_and(|a| m.text == format!("📎 {}", a.title))
}

/// A document on a note: its title, as a link that opens it. `ink` colors
/// it in a recolored bubble.
fn attachment_link(id: i64, a: &Attachment, ink: Option<Hsla>) -> AnyElement {
    let url = a.url();
    div()
        .id(SharedString::from(format!("bubble-file-{id}")))
        .underline()
        .text_color(ink.unwrap_or_else(SeancePalette::violet))
        .cursor_pointer()
        .on_click(move |_, _, _| crate::sysopen::open_detached(&url))
        .child(a.title.clone())
        .into_any_element()
}

/// A bubble's right-click menu. It reads the circle list when it opens, so
/// no bubble holds a copy of it.
fn bubble_menu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    app: &WeakEntity<SeanceApp>,
    ws: &str,
    m: &Msg,
) -> PopupMenu {
    let mut menu = menu;
    // Reply, forward, remind and unread all live in Invoke.
    let local = chat_local::is_local();
    if let Some(draft) = reply_draft(ws, m).filter(|_| !local) {
        let weak = app.clone();
        menu = menu.item(PopupMenuItem::new("Reply").on_click(move |_, _, cx| {
            let draft = draft.clone();
            let _ = weak.update(cx, |a, cx| a.chat_reply_to(draft, cx));
        }));
    }
    let text = m.text.clone();
    menu = menu.item(PopupMenuItem::new("Copy text").on_click(move |_, _, cx| {
        let _ = crate::clipboard::copy_text_to_clipboard(&text, cx);
    }));
    if local {
        return menu;
    }
    if m.id > 0 {
        let id = m.id;
        let (from, weak) = (ws.to_string(), app.clone());
        menu = menu.submenu("Forward to", window, cx, move |menu, _, cx| {
            let circles = weak
                .read_with(cx, |a, _| a.chat.circles.clone())
                .unwrap_or_default();
            circles
                .into_iter()
                .filter(|c| c.ws != from && c.ws != ALERTS_WS)
                .fold(menu, |menu, c| {
                    let (weak, to, name) = (weak.clone(), c.ws.clone(), c.name.clone());
                    menu.item(PopupMenuItem::new(c.name).on_click(move |_, window, cx| {
                        let (to, name, win) = (to.clone(), name.clone(), window.window_handle());
                        let _ = weak.update(cx, |a, cx| {
                            a.chat_call(
                                move || chat_api::forward(id, &to),
                                move |_, cx| toast(win, cx, format!("Forwarded to {name}")),
                                cx,
                            )
                        });
                    }))
                })
        });
        for (lead, title) in [
            (false, "Remind me"),
            (true, "Have the lead pick this up later"),
        ] {
            if lead && ws == ALERTS_WS {
                continue;
            }
            let weak = app.clone();
            menu = menu.submenu(title, window, cx, move |menu, _, _| {
                let times = remind_times(now_ms());
                REMIND_CHOICES
                    .iter()
                    .zip(times)
                    .fold(menu, |menu, ((shown, said), at)| {
                        let weak = weak.clone();
                        menu.item(PopupMenuItem::new(*shown).on_click(move |_, window, cx| {
                            let win = window.window_handle();
                            let _ = weak.update(cx, |a, cx| {
                                a.chat_call(
                                    move || chat_api::remind(id, at, lead, said),
                                    move |_, cx| {
                                        let who = if lead {
                                            "The lead will pick it up"
                                        } else {
                                            "Reminder set:"
                                        };
                                        toast(win, cx, format!("{who} {said}"))
                                    },
                                    cx,
                                )
                            });
                        }))
                    })
            });
        }
    }
    let (weak, ws) = (app.clone(), ws.to_string());
    menu.item(PopupMenuItem::new("Mark unread").on_click(move |_, _, cx| {
        let ws = ws.clone();
        let _ = weak.update(cx, |a, cx| a.chat_unread(ws, cx));
    }))
}

impl SeanceApp {
    /// The read of one round, ready to run off the UI thread: Invoke's, or
    /// this Mac's own (every circle with its lead, and the open one's chat).
    fn round_job(
        &self,
        ws: Option<String>,
        after: Option<i64>,
    ) -> Box<dyn FnOnce() -> Round + Send> {
        if !chat_local::is_local() {
            return Box::new(move || fetch_round(ws, after));
        }
        let ask = chat_local::LocalAsk {
            client: Arc::clone(&self.client),
            circles: self
                .workspaces()
                .into_iter()
                .map(|w| {
                    // The daemon names who Chat talks to: the lead that was
                    // set, else the circle's own Claude. (The lead the rest
                    // of the app knows falls back to the earliest pane.)
                    let working = chat_local::lead_of(&w)
                        .or_else(|| self.stage_lead(&w))
                        .is_some_and(|l| self.busy_panes.contains(&l));
                    (w.clone(), self.workspace_label(&w), None, working)
                })
                .collect(),
            ws,
        };
        Box::new(move || fetch_local(ask))
    }

    pub(super) fn start_chat_poll(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            let Some(this) = this.upgrade() else { break };
            let (on, job) = this.update(cx, |app: &mut SeanceApp, _| {
                let ws = app.chat.selected.clone();
                let after = ws.as_deref().and_then(|w| app.chat.refresh_after(w));
                let on = app.view.mode == ViewMode::Chat;
                (on, on.then(|| app.round_job(ws, after)))
            });
            let Some(job) = job.filter(|_| on) else {
                cx.background_executor().timer(IDLE).await;
                continue;
            };
            let round = cx.background_executor().spawn(async move { job() }).await;
            let failed = round.error.is_some();
            let v = this.update(cx, |app: &mut SeanceApp, cx| {
                app.apply_round(round, cx);
                app.chat.v
            });
            if failed {
                cx.background_executor().timer(RETRY).await;
                continue;
            }
            // Returns on a change, or when the wait runs out; either way the
            // next round reads Invoke again. With no server there is nothing
            // to wait on, so the next round simply comes a second later.
            let polled = cx
                .background_executor()
                .spawn(async move {
                    if chat_local::is_local() {
                        std::thread::sleep(std::time::Duration::from_millis(chat_local::POLL_MS));
                        Ok(v)
                    } else {
                        chat_api::poll(v, POLL_SECS)
                    }
                })
                .await;
            if polled.is_err() {
                cx.background_executor().timer(RETRY).await;
            }
        })
        .detach();
    }

    fn apply_round(&mut self, round: Round, cx: &mut Context<Self>) {
        // Nothing moved: no state change and no repaint.
        if round.quiet && round.error.is_none() && self.chat.error.is_none() {
            return;
        }
        let chat = &mut self.chat;
        chat.error = round.error;
        if let Some(c) = round.circles {
            chat.v = c.v;
            chat.circles = c.circles;
        }
        if chat.selected.is_none() {
            chat.stick = 3;
            chat.selected = self
                .selected_workspace
                .clone()
                .filter(|ws| chat.circles.iter().any(|c| &c.ws == ws))
                .or_else(|| chat.circles.first().map(|c| c.ws.clone()));
        }
        if let Some((ws, p)) = round.progress {
            chat.progress.insert(ws, p);
        }
        if let Some((ws, t, delta)) = round.thread {
            self.apply_thread(ws, t, delta, cx);
        }
        cx.notify();
    }

    /// Take in a thread answer. A `delta` holds only the messages past what
    /// we had; they join it, and bubbles shown before Invoke had them go.
    fn apply_thread(
        &mut self,
        ws: String,
        mut thread: Thread,
        delta: bool,
        cx: &mut Context<Self>,
    ) {
        let chat = &mut self.chat;
        let before = chat.last_id(&ws);
        let had = chat.threads.get(&ws).map_or(0, |t| t.messages.len());
        if delta {
            let mut merged = chat.threads.remove(&ws).unwrap_or_default().messages;
            merged.retain(|m| m.id > 0);
            for m in std::mem::take(&mut thread.messages) {
                match merged.iter_mut().find(|o| o.id == m.id) {
                    Some(o) => *o = m,
                    None => merged.push(m),
                }
            }
            thread.messages = merged;
        }
        let newest = thread.messages.iter().map(|m| m.id).max().unwrap_or(0);
        let reading = chat.unpinned || chat.no_pin.as_deref() == Some(ws.as_str());
        // New messages follow the bottom, but never pull him away from older
        // ones he is reading.
        if before != Some(newest) && !reading {
            chat.stick = 3;
        }
        // Nor may they push the oldest drawn ones out from above him.
        if chat.selected.as_deref() == Some(ws.as_str()) {
            let added = thread.messages.len().saturating_sub(had);
            chat.shown = shown_after_arrival(chat.shown, added, reading && had > 0);
        }
        // Images the thread shows: fetch each once.
        let names: Vec<String> = thread
            .messages
            .iter()
            .flat_map(|m| m.image_names().into_iter().map(str::to_string))
            .filter(|n| !is_video(n))
            .collect();
        for name in names {
            if chat.image_asked.insert(name.clone()) {
                cx.spawn(async move |this, cx| {
                    let leaf = name.clone();
                    let got = cx
                        .background_executor()
                        .spawn(async move {
                            // Through Invoke's /api/file, into the cache; its
                            // size reserves the bubble's room before it paints.
                            chat_api::file(&leaf).map(|p| {
                                let (w, h) = chat_api::image_size(&p).unwrap_or((0, 0));
                                (p, w, h)
                            })
                        })
                        .await;
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |app: &mut SeanceApp, cx| {
                        match got {
                            Ok(found) => {
                                app.chat.images.insert(name, found);
                            }
                            // Let a later thread refresh try again.
                            Err(_) => {
                                app.chat.image_asked.remove(&name);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        chat.usage = thread.usage.clone();
        chat.threads.insert(ws, thread);
    }

    /// Reading the open thread tells Invoke, so the phone's badge clears: only
    /// while this window is the active one and Chat is showing that thread.
    fn maybe_mark_read(&mut self, cx: &mut Context<Self>) {
        // Nothing to tell: there is no phone badge without Invoke.
        if chat_local::is_local() {
            return;
        }
        let chat = &mut self.chat;
        let Some(ws) = chat.selected.clone() else {
            return;
        };
        if self.view.mode != ViewMode::Chat
            || !chat.window_active
            || chat.read_busy.contains(&ws)
            || chat.hold_read.as_deref() == Some(ws.as_str())
        {
            return;
        }
        if chat
            .read_retry_at
            .get(&ws)
            .is_some_and(|at| Instant::now() < *at)
        {
            return;
        }
        let Some(thread) = chat.threads.get(&ws) else {
            return;
        };
        let Some(newest) = chat.last_id(&ws) else {
            return;
        };
        let done = thread
            .read
            .max(chat.read_sent.get(&ws).copied().unwrap_or(0));
        if newest <= done {
            return;
        }
        chat.read_busy.insert(ws.clone());
        cx.spawn(async move |this, cx| {
            let at = ws.clone();
            let sent = cx
                .background_executor()
                .spawn(async move { chat_api::mark_read(&at, newest) })
                .await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, _| {
                app.chat.read_busy.remove(&ws);
                // Only a report Invoke accepted counts; a failed one is retried.
                match sent {
                    // Not while the circle is marked unread: that report is stale.
                    Ok(()) if app.chat.hold_read.as_deref() == Some(ws.as_str()) => {}
                    Ok(()) => {
                        app.chat.read_sent.insert(ws, newest);
                    }
                    // Not every render: a refusal that never clears would
                    // otherwise run curl each frame.
                    Err(_) => {
                        app.chat
                            .read_retry_at
                            .insert(ws, Instant::now() + READ_RETRY);
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn chat_select(&mut self, ws: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.selected.as_deref() != Some(ws) {
            self.chat.selected = Some(ws.to_string());
            self.tasks_popover = None;
        }
        self.chat.reply = reply_after_select(self.chat.reply.take(), ws);
        // Opening a circle, or clicking the open one, shows its newest message.
        self.chat.arrive();
        self.chat.jump = None;
        self.chat.notice = None;
        // Selecting a circle again is what reads one marked unread.
        if self.chat.hold_read.as_deref() == Some(ws) {
            self.chat.hold_read = None;
        }
        // The lead's to-do poll follows the selected circle.
        if self.workspaces().iter().any(|w| w == ws) {
            self.selected_workspace = Some(ws.to_string());
        }
        if let Some(input) = self.chat.input.as_ref() {
            let focus = input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        self.chat_refresh(cx);
        self.chat_fetch_draft(cx);
        cx.notify();
    }

    /// Ask Invoke for a draft the sheet page left for the selected circle,
    /// while Chat shows. It waits in `drafts` until that circle is on screen.
    pub(super) fn chat_fetch_draft(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.chat.selected.clone() else {
            return;
        };
        if self.view.mode != ViewMode::Chat || ws == ALERTS_WS || chat_local::is_local() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let asked = ws.clone();
            let found = cx
                .background_executor()
                .spawn(async move { chat_api::draft(&asked) })
                .await;
            // Invoke down or nothing waiting: the composer stays as it is.
            let Ok(text) = found else { return };
            if text.trim().is_empty() {
                return;
            }
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                let held = app.chat.drafts.entry(ws).or_default();
                *held = with_draft(held, &text);
                cx.notify();
            });
        })
        .detach();
    }

    /// Read the selected circle's whole thread again, so no old status is stale.
    fn chat_refresh(&mut self, cx: &mut Context<Self>) {
        let job = self.round_job(self.chat.selected.clone(), None);
        cx.spawn(async move |this, cx| {
            let round = cx.background_executor().spawn(async move { job() }).await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| app.apply_round(round, cx));
        })
        .detach();
    }

    /// Run one Invoke call off the UI thread. `then` runs when it went
    /// through; a refusal shows as the notice, in Invoke's own words.
    fn chat_call(
        &mut self,
        job: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.chat.notice = None;
        cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { job() }).await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                match done {
                    Ok(()) => then(app, cx),
                    Err(e) => {
                        app.chat.hold_read = None;
                        app.chat.notice = Some(e.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "Start Claude here": a new Claude pane in this circle, made its lead
    /// (a plain shell when Claude isn't installed). The same call a new
    /// circle makes.
    fn chat_start_claude(&mut self, ws: String, cx: &mut Context<Self>) {
        let client = Arc::clone(&self.client);
        self.chat_call(
            move || client.start_claude(&ws).map(|_| ()),
            Self::chat_refresh,
            cx,
        );
    }

    /// Stop the open circle's lead mid-turn.
    fn chat_stop(&mut self, ws: String, cx: &mut Context<Self>) {
        if chat_local::is_local() {
            let Some(lead) = chat_local::lead_of(&ws) else {
                return;
            };
            let client = Arc::clone(&self.client);
            self.chat_call(
                move || chat_local::stop(&client, &lead),
                Self::chat_refresh,
                cx,
            );
            return;
        }
        self.chat_call(move || chat_api::stop(&ws), Self::chat_refresh, cx);
    }

    /// Mark a circle unread. An open thread would read itself again at once,
    /// so it is held back until the circle is selected again.
    fn chat_unread(&mut self, ws: String, cx: &mut Context<Self>) {
        if chat_local::is_local() {
            return;
        }
        let at = ws.clone();
        // Held before the call goes out, so no read can slip in behind it; a
        // refusal lifts the hold again (see `chat_call`).
        if self.chat.selected.as_deref() == Some(ws.as_str()) {
            self.chat.hold_read = Some(ws.clone());
        }
        self.chat_call(
            move || chat_api::unread(&at),
            move |app, cx| {
                app.chat.read_sent.remove(&ws);
                app.chat_refresh(cx);
            },
            cx,
        );
    }

    /// A click on the header's creature: a dance move and a line, or at the
    /// fifth click in a row a party. The bubble's frames come from a short
    /// timer, so nothing else has to be re-rendering; the party asks for its
    /// own in `render_chat`.
    fn chat_poke(&mut self, ws: String, cx: &mut Context<Self>) {
        use super::sprite::{Poke, LINE_MS, PARTY_LINE, PARTY_MS};
        let now = now_ms();
        // More clicks while a party runs must not end it by replacing it with
        // a dance and a line.
        if self.chat.party.as_ref().is_some_and(|p| p.on(now)) {
            eprintln!("[seance chat] creature poke ws={ws} at={now} -> ignored, party running");
            return;
        }
        let (mv, line) = match self.chat.pokes.poke(now) {
            Poke::Dance { mv, line } => (Some(mv), line),
            Poke::Party => {
                super::sprite::start_party(now);
                let others = self
                    .chat
                    .circles
                    .iter()
                    .filter(|c| c.ws != ws)
                    .map(|c| super::sprite::for_circle(&c.ws));
                self.chat.party = Some(super::sprite::Party::new(
                    super::sprite::for_circle(&ws),
                    others,
                    now,
                    &mut rand::thread_rng(),
                ));
                // Frames stop while the window is inactive; this redraw
                // takes the scene down on time anyway.
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(PARTY_MS))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
                (None, PARTY_LINE)
            }
        };
        // Kept in the GUI log (~/.local/share/seance/gui.stderr.log) so a
        // report that the egg did nothing can be checked.
        eprintln!(
            "[seance chat] creature poke ws={ws} at={now} {}",
            if mv.is_none() { "-> PARTY" } else { "-> dance" }
        );
        self.chat.egg = Some(Egg {
            ws,
            mv,
            at: now,
            line,
        });
        cx.spawn(async move |this, cx| {
            let end = now_ms() + LINE_MS;
            while now_ms() < end {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
            }
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    /// Draw more of the open thread's older messages, leaving the one that
    /// was first where it is on screen.
    fn chat_show_earlier(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self
            .chat
            .selected
            .as_ref()
            .and_then(|ws| self.chat.threads.get(ws))
        else {
            return;
        };
        let len = t.messages.len();
        let win = ThreadWindow::new(len, self.chat.shown);
        let Some(first) = t.messages.get(win.start) else {
            return;
        };
        let scroll = &self.chat.scroll;
        let below = win
            .child(win.start)
            .and_then(|c| scroll.bounds_for_item(c))
            .map_or(0., |b| {
                f32::from(b.top() + scroll.offset().y - scroll.bounds().top())
            });
        self.chat.earlier = Some((first.id, below));
        self.chat.shown = show_earlier(self.chat.shown, len);
        self.chat.hold_follow = 2;
        self.chat.unpinned = true;
        cx.notify();
    }

    /// Redraw while a crew dances, about 12 times a second, until the last
    /// render says none does (or Chat is gone). One timer at a time.
    fn ensure_dance_timer(&mut self, cx: &mut Context<Self>) {
        if self.chat.dance_ticking {
            return;
        }
        self.chat.dance_ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(super::redraw::dance_gap_ms(
                    super::redraw::draw_ms(),
                )))
                .await;
            let Some(this) = this.upgrade() else { break };
            let go = this.update(cx, |app: &mut SeanceApp, cx| {
                let go = app.view.mode == ViewMode::Chat && app.chat.frames == Frames::Dance;
                if go {
                    cx.notify();
                } else {
                    app.chat.dance_ticking = false;
                }
                go
            });
            if !go {
                break;
            }
        })
        .detach();
    }

    /// Send `/compact` to the open circle's lead, as the composer would.
    fn chat_compact(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.chat.selected.clone() else {
            return;
        };
        if self.compact_locked(&ws) || ws == ALERTS_WS {
            return;
        }
        self.chat.compact_at = Some((ws.clone(), Instant::now()));
        // The button comes back on its own once the wait is over.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COMPACT_COOLDOWN).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        self.chat_post(ws, "/compact".to_string(), None, false, cx);
    }

    fn compact_locked(&self, ws: &str) -> bool {
        self.chat
            .compact_at
            .as_ref()
            .is_some_and(|(at_ws, at)| at_ws == ws && at.elapsed() < COMPACT_COOLDOWN)
    }

    /// The Compact button beside a high ctx; dimmed and dead after a click.
    fn compact_button(&self, ws: &str, cx: &Context<Self>) -> AnyElement {
        let locked = self.compact_locked(ws);
        div()
            .id("chat-compact")
            .underline()
            .text_color(SeancePalette::text_dim())
            .tooltip(tip("send /compact to the lead"))
            .when(locked, |d| d.opacity(0.4))
            .when(!locked, |d| {
                d.hover(|s| s.text_color(SeancePalette::flame()))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.chat_compact(cx)))
            })
            .child("Compact")
            .into_any_element()
    }

    /// Search once typing has paused, and only for two characters or more.
    fn chat_search_soon(&mut self, cx: &mut Context<Self>) {
        let seq = self.chat.search_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                if app.chat.search_seq == seq {
                    app.chat_search(cx);
                }
            });
        })
        .detach();
    }

    /// Search every thread; the results replace the circle list.
    fn chat_search(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.chat.search.clone().filter(|_| !chat_local::is_local()) else {
            return;
        };
        let q = match search_query(&input.read(cx).value()) {
            Ok(q) => q.to_string(),
            Err(hint) => {
                self.chat.hits = None;
                self.chat.search_note = hint.map(str::to_string);
                cx.notify();
                return;
            }
        };
        self.chat.search_note = None;
        cx.spawn(async move |this, cx| {
            let asked = q.clone();
            let found = cx
                .background_executor()
                .spawn(async move { chat_api::search(&asked) })
                .await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                // An answer for words that have since changed is dropped.
                let now = app.chat.search.as_ref().map(|i| i.read(cx).value());
                if now.as_deref().map(str::trim) == Some(q.as_str()) {
                    match found {
                        Ok(hits) => {
                            if hits.is_empty() {
                                app.chat.search_note = Some("No results".into());
                            }
                            app.chat.hits = Some(hits);
                        }
                        // Beside the box: no thread need be open to see it.
                        Err(e) => app.chat.search_note = Some(format!("Search failed: {e}")),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open a search result: its circle, scrolled to the message.
    fn chat_open_hit(&mut self, ws: &str, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.chat_select(ws, window, cx);
        self.chat.jump = Some(id);
    }

    /// The composer changed: an `@` first asks for the circle's panes.
    fn chat_input_changed(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(input)) = (self.chat.selected.clone(), self.chat.input.clone()) else {
            return;
        };
        if input.read(cx).value().starts_with('@')
            && !chat_local::is_local()
            && !self
                .chat
                .panes_at
                .get(&ws)
                .is_some_and(|at| at.elapsed() < PANES_TTL)
        {
            self.chat.panes_at.insert(ws.clone(), Instant::now());
            cx.spawn(async move |this, cx| {
                let asked = ws.clone();
                let found = cx
                    .background_executor()
                    .spawn(async move { chat_api::panes(&asked) })
                    .await;
                let Some(this) = this.upgrade() else { return };
                this.update(cx, |app: &mut SeanceApp, cx| {
                    match found {
                        Ok(panes) => {
                            app.chat.panes.insert(ws, panes);
                        }
                        Err(e) => {
                            app.chat.panes_at.remove(&ws);
                            app.chat.notice = Some(e.to_string());
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Pick a pane from the `@` suggestions.
    fn chat_mention(&mut self, at: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.chat.input.clone() {
            input.update(cx, |i, cx| i.set_value(format!("@{at} "), window, cx));
            let focus = input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
    }

    /// Pick a file to send as a note, with whatever is typed as its caption.
    fn chat_attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |app, window, cx| {
                if let Err(e) = chat_api::check_upload(&path) {
                    app.chat.notice = Some(e.to_string());
                    cx.notify();
                    return;
                }
                let (Some(ws), Some(input)) = (app.chat.selected.clone(), app.chat.input.clone())
                else {
                    return;
                };
                let caption = input.read(cx).value().trim().to_string();
                input.update(cx, |i, cx| i.set_value("", window, cx));
                app.chat_post(ws, caption, Some(path), true, cx);
            });
        })
        .detach();
    }

    fn chat_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(ws), Some(input)) = (self.chat.selected.clone(), self.chat.input.clone()) else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        if ws == ALERTS_WS || text.is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.set_value("", window, cx));
        let reply = self.chat.reply.clone().filter(|r| r.ws == ws);
        self.chat_post_reply(ws, text, None, true, reply, cx);
    }

    /// Reply to a message: the quote bar shows over the composer, which
    /// takes the focus.
    fn chat_reply_to(&mut self, draft: ReplyDraft, cx: &mut Context<Self>) {
        self.chat.reply = Some(draft);
        self.chat.focus_input = true;
        cx.notify();
    }

    /// A click on a quote: scroll to the original, drawing more of the
    /// thread if it is older than what shows. One that is not loaded stays put.
    fn chat_quote_jump(&mut self, id: i64, cx: &mut Context<Self>) {
        let ws = self.chat.selected.clone().unwrap_or_default();
        let ids: Vec<i64> = self
            .chat
            .threads
            .get(&ws)
            .map(|t| t.messages.iter().map(|m| m.id).collect())
            .unwrap_or_default();
        if jump_plan(self.chat.shown, &ids, id).is_some() {
            self.chat.jump = Some(id);
            cx.notify();
        }
    }

    /// Send a note to `ws`'s lead, with a file if there is one. Shown at once;
    /// the next thread answer replaces it with the real one. `typed` says the
    /// words came from the composer, so a failed send puts them back there.
    pub(super) fn chat_post(
        &mut self,
        ws: String,
        text: String,
        file: Option<PathBuf>,
        typed: bool,
        cx: &mut Context<Self>,
    ) {
        self.chat_post_reply(ws, text, file, typed, None, cx);
    }

    /// [`Self::chat_post`], as a reply to `reply` when there is one. The
    /// reply is cleared once Invoke takes the note, kept if it does not.
    fn chat_post_reply(
        &mut self,
        ws: String,
        text: String,
        file: Option<PathBuf>,
        typed: bool,
        reply: Option<ReplyDraft>,
        cx: &mut Context<Self>,
    ) {
        let shown = match (&file, text.is_empty()) {
            (Some(path), true) => format!(
                "📎 {}",
                path.file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            ),
            _ => text.clone(),
        };
        self.chat.next_local -= 1;
        let local_id = self.chat.next_local;
        let after = self.chat.refresh_after(&ws);
        let thread = self.chat.threads.entry(ws.clone()).or_default();
        thread.messages.push(Msg {
            id: local_id,
            from: "me".into(),
            text: shown,
            ts: now_ms(),
            status: "sending".into(),
            reply: reply.as_ref().map(ReplyDraft::quote),
            ..Default::default()
        });
        self.chat.arrive();
        let thread_ws = ws.clone();
        let reply_id = reply.map(|r| r.id);
        // Typed into the lead's terminal on this Mac, or sent through Invoke.
        let job = self.round_job(Some(ws.clone()), after);
        cx.spawn(async move |this, cx| {
            let to = ws.clone();
            let body = text.clone();
            let sent = cx
                .background_executor()
                .spawn(async move {
                    if chat_local::is_local() {
                        return chat_local::send(&to, &body);
                    }
                    match file {
                        Some(path) => chat_api::send_file(&to, &body, &path),
                        None => chat_api::send(&to, &body, reply_id),
                    }
                })
                .await;
            let round = cx.background_executor().spawn(async move { job() }).await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                app.apply_round(round, cx);
                app.chat.reply = reply_after_send(app.chat.reply.take(), reply_id, sent.is_ok());
                if let Err(e) = sent {
                    // Not sent: the bubble goes and the words go back where
                    // they were typed.
                    if let Some(t) = app.chat.threads.get_mut(&thread_ws) {
                        t.messages.retain(|m| m.id != local_id);
                    }
                    app.chat.error = Some(format!("not sent: {e}"));
                    if typed && !text.is_empty() {
                        app.chat.restore = Some(text);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Tap a choice: it goes to the lead as your message, tied to the message
    /// it answers. The pill is filled at once and every pill goes dead; if the
    /// send fails they come back, with the reason on top.
    fn chat_answer(&mut self, answers: i64, choice: String, cx: &mut Context<Self>) {
        let Some(ws) = self.chat.selected.clone() else {
            return;
        };
        if ws == ALERTS_WS || self.chat.tapped.contains_key(&answers) {
            return;
        }
        self.chat.tapped.insert(answers, choice.clone());
        // Shown at once as your message, until the thread answer has the real one.
        self.chat.next_local -= 1;
        let local_id = self.chat.next_local;
        let after = self.chat.refresh_after(&ws);
        let thread = self.chat.threads.entry(ws.clone()).or_default();
        thread.messages.push(Msg {
            id: local_id,
            from: "me".into(),
            text: choice.clone(),
            ts: now_ms(),
            status: "sending".into(),
            ..Default::default()
        });
        self.chat.arrive();
        let thread_ws = ws.clone();
        let job = self.round_job(Some(ws.clone()), after);
        // A tap in local mode presses that option's digit in the lead's pane.
        let local = chat_local::is_local()
            .then(|| chat_local::lead_of(&ws).map(|l| (Arc::clone(&self.client), l)))
            .flatten();
        cx.spawn(async move |this, cx| {
            let (to, text) = (ws.clone(), choice);
            let sent = cx
                .background_executor()
                .spawn(async move {
                    match local {
                        Some((client, lead)) => chat_local::answer(&client, &to, &lead, &text),
                        None if chat_local::is_local() => Err(anyhow::anyhow!("no lead")),
                        None => chat_api::answer(&to, &text, answers),
                    }
                })
                .await;
            let round = cx.background_executor().spawn(async move { job() }).await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                app.apply_round(round, cx);
                if let Err(e) = sent {
                    // Not sent: the pills are live again and the bubble goes.
                    app.chat.tapped.remove(&answers);
                    if let Some(t) = app.chat.threads.get_mut(&thread_ws) {
                        t.messages.retain(|m| m.id != local_id);
                    }
                    app.chat.error = Some(format!("not sent: {e}"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The open circle's lead pane while Chat shows: its torch burns in the
    /// thread header.
    pub(super) fn chat_lead(&self) -> Option<String> {
        if self.view.mode != ViewMode::Chat {
            return None;
        }
        self.chat
            .selected
            .as_deref()
            .and_then(|ws| self.stage_lead(ws))
    }

    /// Back to Classic, on this circle's lead.
    fn chat_show_terminal(&mut self, ws: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewMode::Classic, cx);
        self.selected_workspace = Some(ws.to_string());
        if let Some(lead) = self.stage_lead(ws) {
            self.set_active(&lead, window, cx);
        }
        cx.notify();
    }

    /// Is the human typing in the composer or the search box? The focus pass
    /// must leave them be: it hands focus back to a terminal from any handle
    /// it does not know, which kept the search box from holding a keystroke.
    pub(super) fn chat_input_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.view.mode == ViewMode::Chat
            && [&self.chat.input, &self.chat.search]
                .into_iter()
                .flatten()
                .any(|i| {
                    window
                        .focused(cx)
                        .is_some_and(|f| f == i.read(cx).focus_handle(cx))
                })
    }

    pub(super) fn render_chat(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.chat.input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .auto_grow(1, 6)
                    .submit_on_enter(true)
                    .placeholder("Message the lead")
            });
            cx.subscribe_in(
                &input,
                window,
                |this: &mut SeanceApp, _input, event: &InputEvent, window, cx| {
                    // Enter sends; Shift+Enter adds a line (the input does that).
                    match event {
                        InputEvent::PressEnter { shift: false, .. } => this.chat_send(window, cx),
                        // Quick replies and @ suggestions follow what is typed.
                        InputEvent::Change => this.chat_input_changed(cx),
                        _ => {}
                    }
                },
            )
            .detach();
            let focus = input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            self.chat.input = Some(input);
        }
        if self.chat.code_input.is_none() {
            let code = cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Paste the code")
            });
            cx.subscribe_in(
                &code,
                window,
                |this: &mut SeanceApp, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.chat_send_code(window, cx);
                    }
                },
            )
            .detach();
            self.chat.code_input = Some(code);
        }
        if self.chat.search.is_none() {
            let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search chats"));
            cx.subscribe_in(
                &search,
                window,
                |this: &mut SeanceApp, input, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        // Enter searches now; a pending timer must not repeat it.
                        this.chat.search_seq += 1;
                        this.chat_search(cx);
                    }
                    InputEvent::Change => {
                        this.chat.search_seq += 1;
                        if input.read(cx).value().trim().is_empty() {
                            this.chat.hits = None;
                            this.chat.search_note = None;
                            cx.notify();
                        } else {
                            this.chat_search_soon(cx);
                        }
                    }
                    _ => {}
                },
            )
            .detach();
            self.chat.search = Some(search);
        }
        self.chat.window_active = window.is_window_active();
        // Set again as the header's creature is painted, if it still is.
        self.chat.creature_at.set(None);
        if let Some(input) = self.chat.input.clone() {
            if let Some(text) = self.chat.restore.take() {
                // Whatever was typed since stays, after the failed text.
                let typed = input.read(cx).value().trim().to_string();
                let text = if typed.is_empty() {
                    text
                } else {
                    format!("{text}\n{typed}")
                };
                input.update(cx, |i, cx| i.set_value(text, window, cx));
            }
            let draft = self
                .chat
                .selected
                .as_ref()
                .and_then(|ws| self.chat.drafts.remove(ws));
            if let Some(draft) = draft {
                let text = with_draft(&input.read(cx).value(), &draft);
                input.update(cx, |i, cx| {
                    i.set_value(text, window, cx);
                    // The composer is multi-line, so the caret starts at the top.
                    let end = i.text().offset_to_position(i.text().len());
                    i.set_cursor_position(end, window, cx);
                });
                self.chat.focus_input = true;
            }
            if std::mem::take(&mut self.chat.focus_input) {
                let focus = input.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
        }
        self.maybe_mark_read(cx);
        let ids: Vec<i64> = self
            .chat
            .selected
            .as_ref()
            .and_then(|ws| self.chat.threads.get(ws))
            .map(|t| t.messages.iter().map(|m| m.id).collect())
            .unwrap_or_default();
        // A search result waits here for its thread to arrive; it takes the
        // frame from the pin to the bottom, and widens what is drawn to reach it.
        if let Some(id) = self.chat.jump {
            if let Some((shown, child)) = jump_plan(self.chat.shown, &ids, id) {
                self.chat.jump = None;
                self.chat.stick = 0;
                self.chat.no_pin = self.chat.selected.clone();
                self.chat.shown = shown;
                self.chat.scroll.scroll_to_item(child);
                window.request_animation_frame();
            }
        }
        // "Show earlier": the message that was first keeps its place on
        // screen. A zero-height marker sits that far above it, and this frame
        // scrolls the marker to the top edge.
        let marker = self.chat.earlier.take().and_then(|(id, below)| {
            let at = ids.iter().position(|m| *m == id)?;
            let win = ThreadWindow::new(ids.len(), self.chat.shown);
            self.chat.scroll.scroll_to_top_of_item(win.child(at)?);
            Some((at, below))
        });
        // Follow the bottom while pinned: images, chips and new messages change
        // the thread's size after the pin frames are over.
        let now = (
            f32::from(self.chat.scroll.offset().y),
            f32::from(self.chat.scroll.max_offset().y),
        );
        let held =
            self.chat.no_pin.is_some() || self.chat.jump.is_some() || self.chat.hold_follow > 0;
        let (pinned, down) = follow_step(!self.chat.unpinned, held, self.chat.seen, now);
        // Messages drawn above him are reading, not a slide to the bottom.
        self.chat.unpinned = !pinned || self.chat.hold_follow > 0;
        self.chat.hold_follow = self.chat.hold_follow.saturating_sub(1);
        self.chat.seen = Some(now);
        if self.chat.stick > 0 {
            self.chat.stick -= 1;
            self.chat.scroll.scroll_to_bottom();
            window.request_animation_frame();
        } else if down {
            self.chat.scroll.scroll_to_bottom();
            window.request_animation_frame();
        }
        // Crews dance while one shows in the header or a list row in view; the
        // spinner's 240ms tick is too coarse for them, and every frame (60 a
        // second) rebuilt the whole view for a few pixels of movement, so the
        // dance timer redraws them about 12 times a second. Only the brief
        // party gets every frame. Nothing in a window nobody is looking at.
        let items = self.chat_list_items();
        let now = now_ms();
        // Every circle, not just the rows shown, so a search keeps the dances.
        let crews = self
            .chat
            .circles
            .iter()
            .map(|c| (c.ws.as_str(), &c.crew[..]));
        super::sprite::keep_crew_dances(&mut self.chat.list_dances, crews, now);
        // The list's children in view as of its last layout; all of them
        // before it has one.
        let list = &self.chat.list_scroll;
        let seen = if list.bounds_for_item(0).is_some() {
            list.top_item()..=list.bottom_item()
        } else {
            0..=usize::MAX
        };
        self.chat.list_dancing.clear();
        for (i, item) in items.iter().enumerate() {
            if let ListItem::Circle(c) = item {
                if let Some(d) = self.chat.list_dances.get_mut(&c.ws) {
                    if seen.contains(&i) {
                        d.advance(now, &mut rand::thread_rng());
                        self.chat.list_dancing.insert(c.ws.clone());
                    }
                }
            }
        }
        let crew = self
            .chat
            .selected
            .as_ref()
            .and_then(|ws| self.chat.threads.get(ws))
            .is_some_and(|t| super::sprite::is_crew(&t.crew));
        if crew {
            self.chat
                .crew_dance
                .get_or_insert_with(|| super::sprite::CrewDance::new(now))
                .advance(now, &mut rand::thread_rng());
        } else {
            self.chat.crew_dance = None;
        }
        self.chat.party = self.chat.party.take().filter(|p| p.on(now));
        if let Some(party) = self.chat.party.as_mut() {
            party.advance(now, &mut rand::thread_rng());
        }
        let party = self.chat.party.is_some();
        self.chat.frames = chat_frames(
            crew,
            party,
            !self.chat.list_dancing.is_empty(),
            self.chat.window_active,
        );
        match self.chat.frames {
            Frames::Smooth => window.request_animation_frame(),
            Frames::Dance => self.ensure_dance_timer(cx),
            Frames::Idle => {}
        }
        let list = self.render_chat_list(items, cx);
        // The thread column's width, for the decorative torches' room checks:
        // as last laid out, else the window less the list.
        let thread_w = self
            .chat
            .thread_w
            .get()
            .unwrap_or(f32::from(window.viewport_size().width) - LIST_W);
        // The marker's margins cancel the thread's gap (`gap_2`, half a rem).
        let marker = marker.map(|(at, below)| (at, below, f32::from(window.rem_size()) * 0.5));
        let thread = self.render_chat_thread(thread_w, marker, cx);
        div()
            .id("chat")
            .flex_1()
            .h_full()
            .min_w_0()
            .flex()
            .bg(SeancePalette::bg())
            .child(list)
            .child(thread)
            .children(self.party_overlay(window.viewport_size(), cx))
            .into_any_element()
    }

    /// Circles in the sidebar's order: grouped by space, as Classic lists them.
    fn chat_circles_in_order(&self) -> Vec<CircleRow> {
        let local = self.subs_pref.spaces.all_circles(&self.workspaces());
        let at = |ws: &str| local.iter().position(|l| l == ws).unwrap_or(usize::MAX);
        let mut rows = self.chat.circles.clone();
        rows.sort_by_key(|c| at(&c.ws));
        rows
    }

    /// The circle list's children in order: none while search results stand
    /// in for the circles.
    fn chat_list_items(&self) -> Vec<ListItem> {
        let mut items = Vec::new();
        if self.chat.hits.is_some() {
            return items;
        }
        let spaces = &self.subs_pref.spaces;
        // Active circles sit above the spaces, lifted out of theirs.
        let active = active_circles(&self.chat.circles, now_ms());
        if !active.is_empty() {
            items.push(ListItem::Label("ACTIVE".to_string()));
            items.extend(active.iter().cloned().map(ListItem::Circle));
        }
        let mut space = String::new();
        for c in self.chat_circles_in_order() {
            if active.iter().any(|a| a.ws == c.ws) {
                continue;
            }
            let id = spaces.space_of(&c.ws).to_string();
            if id != space {
                if let Some(s) = spaces.list.iter().find(|s| s.id == id) {
                    items.push(ListItem::Space {
                        id: s.id.clone(),
                        name: s.name.clone(),
                    });
                }
                space = id;
            }
            items.push(ListItem::Circle(c));
        }
        // A new space has no circle yet: its label still shows, so it is there
        // to be named and filled.
        let shown: std::collections::HashSet<String> = self
            .chat_circles_in_order()
            .iter()
            .map(|c| spaces.space_of(&c.ws).to_string())
            .collect();
        let names = spaces.list.iter().map(|s| (s.id.as_str(), s.name.as_str()));
        items.extend(
            empty_spaces(names, &shown)
                .into_iter()
                .map(|(id, name)| ListItem::Space { id, name }),
        );
        items
    }

    fn render_chat_list(&self, items: Vec<ListItem>, cx: &Context<Self>) -> AnyElement {
        let mut col = div()
            .id("chat-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.chat.list_scroll)
            .flex()
            .flex_col();
        if let Some(hits) = &self.chat.hits {
            // Search results stand in for the circles.
            if !hits.is_empty() {
                col = col.child(list_label(format!("{} FOUND", hits.len())));
            }
            for h in hits {
                col = col.child(self.chat_hit_row(h, cx));
            }
        }
        for item in items {
            col = col.child(match item {
                ListItem::Label(text) => list_label(text).into_any_element(),
                ListItem::Space { id, name } => self.chat_space_heading(id, name, cx),
                ListItem::Circle(c) => self.chat_circle_row(&c, cx),
            });
        }
        div()
            .w(px(LIST_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(SeancePalette::bg_elevated())
            .border_r_1()
            .border_color(SeancePalette::border())
            // Castle's stone, as on Classic's rail; flat schemes unchanged.
            .map(|d| crate::skin::under(d, crate::skin::RAIL))
            .child(
                div()
                    .flex_none()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.render_brand_block("chat", "chat", cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div().flex_1().min_w_0().children(
                                    self.chat
                                        .search
                                        .as_ref()
                                        .filter(|_| !chat_local::is_local())
                                        .map(|s| Input::new(s)),
                                ),
                            )
                            .child(self.render_plus_button(
                                "chat-plus",
                                "new circle or space",
                                PlusAction::Menu,
                                cx,
                            )),
                    )
                    .children(self.render_rename_field())
                    .children(self.chat.search_note.as_ref().map(|n| {
                        div()
                            .text_xs()
                            .text_color(SeancePalette::text_faint())
                            .child(n.clone())
                    })),
            )
            .child(col)
            .when(crate::skin::torches(), |d| {
                let frame = self.torch_frame;
                d.child(decor::by_mark(frame))
                    .child(decor::rail_post(frame))
            })
            .into_any_element()
    }

    /// One search result: the circle and who spoke, then the words.
    fn chat_hit_row(&self, h: &Hit, cx: &Context<Self>) -> AnyElement {
        let (ws, id) = (h.ws.clone(), h.id);
        let who = if h.from == "me" {
            "you"
        } else {
            h.from.as_str()
        };
        div()
            .id(SharedString::from(format!("chat-hit-{}-{}", h.ws, h.id)))
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .cursor_pointer()
            .hover(|s| s.bg(SeancePalette::surface()))
            .on_click(
                cx.listener(move |this, _, window, cx| this.chat_open_hit(&ws, id, window, cx)),
            )
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(SeancePalette::text_faint())
                    .child(format!("{} · {who} · {}", h.circle, clock(h.ts))),
            )
            .child(
                div()
                    .line_clamp(2)
                    .text_sm()
                    .text_color(SeancePalette::text())
                    .child(h.text.clone()),
            )
            .into_any_element()
    }

    /// A space's heading. A circle row dropped here moves into the space.
    fn chat_space_heading(&self, id: String, name: String, cx: &Context<Self>) -> AnyElement {
        list_label(name.to_uppercase())
            .id(SharedString::from(format!("chat-space-{id}")))
            .drag_over::<super::util::DraggedCircle>(|style, _, _, _| {
                style.bg(SeancePalette::flame_dim().opacity(0.35))
            })
            .on_drop(
                cx.listener(move |this, drag: &super::util::DraggedCircle, _, cx| {
                    this.move_circle_to_space(&drag.slug, &id);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn chat_circle_row(&self, c: &CircleRow, cx: &Context<Self>) -> AnyElement {
        let targets = move_targets(
            self.subs_pref
                .spaces
                .list
                .iter()
                .map(|s| (s.id.as_str(), s.name.as_str())),
            self.subs_pref.spaces.space_of(&c.ws),
        );
        let drag_slug = c.ws.clone();
        let drag_label = self.workspace_label(&c.ws);
        let on = self.chat.selected.as_deref() == Some(c.ws.as_str());
        let ws = c.ws.clone();
        let (menu_ws, weak) = (c.ws.clone(), cx.weak_entity());
        let pulse = 0.45 + 0.55 * (((now_ms() / 300) % 2) as f32);
        let me = row_plate(crate::theme::active());
        div()
            .id(SharedString::from(format!("chat-circle-{}", c.ws)))
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .relative()
            .when(on, |d| {
                d.bg(selected_row_bg(me))
                    .children(selected_row_bar(me))
                    .children(crate::chain::frame())
            })
            .hover(|s| s.bg(SeancePalette::surface()))
            .on_click(cx.listener(move |this, _, window, cx| this.chat_select(&ws, window, cx)))
            .on_drag(
                super::util::DraggedCircle { slug: drag_slug },
                move |_, _, _, cx| {
                    let label = format!("◈ {drag_label}");
                    cx.new(|_| super::util::DragPill { label })
                },
            )
            .context_menu(move |menu, window, cx| {
                let (ws, weak) = (menu_ws.clone(), weak.clone());
                let (move_ws, move_weak, targets) = (ws.clone(), weak.clone(), targets.clone());
                menu.item(PopupMenuItem::new("Mark unread").on_click(move |_, _, cx| {
                    let ws = ws.clone();
                    let _ = weak.update(cx, |a, cx| a.chat_unread(ws, cx));
                }))
                .submenu("Move to space", window, cx, move |sub, _, _| {
                    targets.iter().fold(sub, |sub, (id, name)| {
                        let (ws, id, weak) = (move_ws.clone(), id.clone(), move_weak.clone());
                        sub.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                            let (ws, id) = (ws.clone(), id.clone());
                            let _ = weak.update(cx, |a, cx| {
                                a.move_circle_to_space(&ws, &id);
                                cx.notify();
                            });
                        }))
                    })
                })
            })
            .child(self.chat_row_creature(c))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .text_color(SeancePalette::text())
                                    .child(c.name.clone()),
                            )
                            .when(c.working, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(SeancePalette::flame().opacity(pulse)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(SeancePalette::text_faint())
                            .child(c.last.as_ref().map(preview).unwrap_or_default()),
                    ),
            )
            .when(c.unread > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .map(|d| carve::corners(d, Stock::Full))
                        .bg(SeancePalette::flame())
                        .text_xs()
                        .text_color(SeancePalette::bg())
                        .child(c.unread.to_string()),
                )
            })
            .into_any_element()
    }

    /// The party over the Chat view: dimmed, with a purple glow, the crew
    /// spinning in the middle and confetti falling. A click on it ends the
    /// party early; it covers the view, so nothing under it takes the click.
    fn party_overlay(
        &self,
        view: gpui::Size<gpui::Pixels>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        use super::sprite::{confetti_at, party_scene, CONFETTI, GENIE};
        let party = self.chat.party.as_ref()?;
        let now = now_ms();
        let t = now.saturating_sub(party.at);
        let scene = party_scene(t)?;
        let (w, h) = (f32::from(view.width), f32::from(view.height));
        let confetti = (0..CONFETTI).filter_map(|i| {
            let (x, y) = confetti_at(party.at, i, t, w, h)?;
            Some(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .size(px(6.))
                    .bg(party.confetti_color(i)),
            )
        });
        // gpui has no radial gradient: nested translucent discs instead.
        let genie = gpui::Hsla::from(gpui::rgb(GENIE)).opacity(0.13);
        let glow = [0.45, 0.7, 1.].into_iter().fold(div(), |inner, k| {
            let d = w.min(h) * k;
            div()
                .flex_none()
                .size(px(d))
                .rounded_full()
                .bg(genie)
                .flex()
                .items_center()
                .justify_center()
                .child(inner)
        });
        let centred = || {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
        };
        Some(
            div()
                .id("chat-party")
                .absolute()
                .inset_0()
                .overflow_hidden()
                .occlude()
                .opacity(scene.fade)
                .bg(gpui::black().opacity(0.6))
                .on_click(cx.listener(|this, ev: &gpui::ClickEvent, _, cx| {
                    this.chat_party_click(ev.position(), cx)
                }))
                .child(centred().child(glow))
                .child(centred().child(party.render(now, w)))
                .children(confetti)
                .into_any_element(),
        )
    }

    /// A click on the party ends it early, unless it is on the header's
    /// creature.
    fn chat_party_click(&mut self, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let now = now_ms();
        let on_creature = self.chat.creature_at.get().is_some_and(|b| b.contains(&at));
        if super::sprite::click_party(&mut self.chat.party, now, on_creature) {
            super::sprite::stop_party();
            eprintln!("[seance chat] party ended early at={now}");
        } else if on_creature {
            eprintln!("[seance chat] creature poke at={now} -> ignored, party running");
        }
        cx.notify();
    }

    /// A list row's creature, or its crew while two or more dance in view.
    fn chat_row_creature(&self, c: &CircleRow) -> AnyElement {
        let sprite = super::sprite::for_circle(&c.ws);
        let slot = div().flex_none().w(px(30.)).flex().justify_center();
        // Needs-you wins: no crew and no dance while a pane asks for the human.
        let needs = self.workspace_needs_human(&c.ws);
        let dance = self
            .chat
            .list_dances
            .get(&c.ws)
            .filter(|_| self.chat.list_dancing.contains(&c.ws) && !needs);
        let Some(dance) = dance else {
            return slot
                .child(super::sprite::render(
                    &sprite,
                    2.,
                    self.circle_dancing(&c.ws, c.working),
                ))
                .into_any_element();
        };
        let cuts = super::sprite::crew_at(&c.crew, dance, now_ms());
        // Wider than the slot: centred over it in a box that holds it, as
        // tall as the lone creature, so the row and its text stay put.
        slot.h(px(20.))
            .relative()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-20.))
                    .right(px(-20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(super::sprite::render_crew(&sprite, &cuts, None, LIST_CREW)),
            )
            .into_any_element()
    }

    /// The thread header's creature. Click it: it dances and says something.
    fn header_creature(&self, ws: &str, working: bool, cx: &Context<Self>) -> AnyElement {
        let now = now_ms();
        let egg = self.chat.egg.as_ref().filter(|e| e.ws == ws);
        let moving = egg.and_then(|e| e.mv.map(|mv| (mv, e.at)));
        let say = egg
            .filter(|e| now.saturating_sub(e.at) < super::sprite::LINE_MS)
            .map(|e| e.line);
        let target = ws.to_string();
        let sprite = super::sprite::for_circle(ws);
        // Two or more panes working: a crew of cutouts instead of one creature.
        // Needs-you wins: a single, still creature while a pane asks for the human.
        let crew = self
            .chat
            .threads
            .get(ws)
            .filter(|_| !self.workspace_needs_human(ws))
            .map_or(&[][..], |t| &t.crew);
        let dance = self
            .chat
            .crew_dance
            .unwrap_or_else(|| super::sprite::CrewDance::new(now));
        let cuts = super::sprite::crew_at(crew, &dance, now);
        div()
            .id("chat-creature")
            .relative()
            .flex_none()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.chat_poke(target.clone(), cx)))
            .child({
                let at = self.chat.creature_at.clone();
                gpui::canvas(move |b, _, _| at.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .size_full()
            })
            .child(if cuts.is_empty() {
                super::sprite::render_move(&sprite, 2.5, working, moving)
            } else {
                super::sprite::render_crew(&sprite, &cuts, moving, 1.)
            })
            .children(say.map(|line| {
                // Deferred, so the thread below cannot paint over it.
                gpui::deferred(
                    carve::plate(div(), SeancePalette::bg_elevated(), Stock::Md, Cut::Control)
                        .absolute()
                        .top(px(34.))
                        .left(px(0.))
                        .px_2()
                        .py_1()
                        .when(!carve::slick(), |d| {
                            d.border_1()
                                .border_color(SeancePalette::border())
                                .shadow_lg()
                        })
                        .map(|d| carve::bg(d, SeancePalette::bg_elevated()))
                        .text_xs()
                        .text_color(SeancePalette::text())
                        .whitespace_nowrap()
                        .child(line),
                )
                .with_priority(5)
            }))
            .into_any_element()
    }

    /// `marker`: after "Show earlier", the index of the message that was
    /// first, how far below the top edge it sat, and the thread's gap.
    fn render_chat_thread(
        &self,
        thread_w: f32,
        marker: Option<(usize, f32, f32)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(ws) = self.chat.selected.clone() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(SeancePalette::text_faint())
                .flex_col()
                .gap_1()
                .child(match &self.chat.error {
                    Some(e) if e == chat_api::NO_URL_MESSAGE => e.clone(),
                    Some(e) if chat_local::is_local() => format!("Can't read this circle: {e}"),
                    Some(e) => format!("Can't reach Invoke: {e}"),
                    None => "Loading circles…".to_string(),
                })
                // A refusal has to show even before a thread is open.
                .children(
                    self.chat
                        .notice
                        .clone()
                        .map(|n| div().text_xs().text_color(SeancePalette::danger()).child(n)),
                )
                .into_any_element();
        };
        let circle = self.chat.circles.iter().find(|c| c.ws == ws);
        let name = circle.map_or(ws.clone(), |c| c.name.clone());
        let working = self.circle_dancing(&ws, circle.is_some_and(|c| c.working));
        let line = self
            .chat
            .progress
            .get(&ws)
            .filter(|p| p.busy)
            .and_then(|p| p.doing.clone())
            .unwrap_or_default();
        let lead = if chat_local::is_local() {
            chat_local::lead_of(&ws).or_else(|| self.stage_lead(&ws))
        } else {
            self.stage_lead(&ws)
        };
        // The lead's wall torch, when the skin has torches.
        let torch = lead
            .as_deref()
            .and_then(|l| self.panes.iter().find(|p| p.slug == l))
            .and_then(|p| self.pane_torch_frame(p, cx))
            .zip(lead.as_deref().map(crate::torch::seed));
        let todo = lead
            .as_ref()
            .and_then(|l| self.lead_tasks.get(l))
            .map_or(0, |d| d.open_count());
        let read_only = ws == ALERTS_WS;
        // The wall's decorative torches, where they clear the header's text.
        let wall = if crate::skin::torches() {
            let crew = self.chat.threads.get(&ws).map_or(0, |t| t.crew.len());
            decor::wall(
                self.torch_frame,
                thread_w,
                decor::header_text_end(torch.is_some(), crew, &name, &line),
                decor::header_chips_at(thread_w, !read_only),
            )
        } else {
            Vec::new()
        };

        let mut header = div()
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(SeancePalette::border())
            .map(|d| crate::skin::under(d, crate::skin::STRIP))
            .children(wall)
            .children(torch.map(|((t, frame), seed)| crate::torch::element(t, frame, seed, 1.5)))
            .child(self.header_creature(&ws, working, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .text_color(SeancePalette::text())
                            .child(name),
                    )
                    .when(!line.is_empty(), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(SeancePalette::flame())
                                .child(line),
                        )
                    }),
            );
        let tasks_open = lead.as_deref().is_some_and(|l| self.tasks_popover_open(l));
        let chip_lead = lead.clone();
        header = header.when(!read_only && !chat_local::is_local(), |header| {
            header.child(
                carve::plate(
                    div().id("chat-todo"),
                    if carve::slick() && tasks_open {
                        SeancePalette::violet()
                    } else {
                        SeancePalette::surface()
                    },
                    Stock::Sm,
                    Cut::Control,
                )
                .flex_none()
                .px_2()
                .py_0p5()
                .when(carve::on() && !tasks_open, |d| {
                    carve::bg(d, SeancePalette::surface())
                })
                .text_xs()
                .text_color(if tasks_open {
                    SeancePalette::bg()
                } else {
                    SeancePalette::text_dim()
                })
                .when(tasks_open, |d| carve::bg(d, SeancePalette::violet()))
                .hover(|s| s.text_color(SeancePalette::flame()))
                .cursor_pointer()
                .tooltip(tip("the lead's tasks"))
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                    if let Some(lead) = chip_lead.as_deref() {
                        this.toggle_tasks_popover(lead, ev.position(), cx);
                    }
                }))
                .child(format!("☑ {todo}"))
                .when(crate::skin::torches(), |d| {
                    d.relative().child(decor::by_tasks(self.torch_frame))
                }),
            )
        });
        let files_chip = self.chat_files_chip(&ws).filter(|_| !read_only);
        let files_open = self.chat_files_open(&ws);
        header = header.children(files_chip.map(|label| {
            carve::plate(
                div().id("chat-files"),
                SeancePalette::surface(),
                Stock::Sm,
                Cut::Control,
            )
            .flex_none()
            .px_2()
            .py_0p5()
            .text_xs()
            .text_color(if files_open {
                SeancePalette::text()
            } else {
                SeancePalette::text_dim()
            })
            .map(|d| carve::bg(d, SeancePalette::surface()))
            .hover(|s| s.text_color(SeancePalette::flame()))
            .cursor_pointer()
            .tooltip(tip("show or hide this circle's files"))
            .on_click({
                let ws = ws.clone();
                cx.listener(move |this, _, _, cx| this.toggle_chat_files(&ws, cx))
            })
            .child(label)
        }));
        // AFK (standalone app): the same switch as the Seance menu's.
        let afk_label = super::afk_ui::chip_label(&self.afk).filter(|_| !read_only);
        let afk_on = self.afk.on;
        let afk_tip = super::afk_ui::chip_tip(&self.afk);
        header = header.children(afk_label.map(|label| {
            carve::plate(
                div().id("chat-afk"),
                SeancePalette::surface(),
                Stock::Sm,
                Cut::Control,
            )
            .flex_none()
            .px_2()
            .py_0p5()
            .text_xs()
            .text_color(if afk_on {
                SeancePalette::flame()
            } else {
                SeancePalette::text_dim()
            })
            .map(|d| carve::bg(d, SeancePalette::surface()))
            .hover(|s| s.text_color(SeancePalette::flame()))
            .cursor_pointer()
            .tooltip(tip_s(afk_tip))
            .on_click(cx.listener(|this, _, _, cx| this.afk_toggle(cx)))
            .child(label)
        }));
        header = header.when(!read_only, |header| {
            header.child(
                carve::plate(
                    div().id("chat-terminal"),
                    SeancePalette::surface(),
                    Stock::Sm,
                    Cut::Control,
                )
                .flex_none()
                .px_2()
                .py_0p5()
                .text_xs()
                .text_color(SeancePalette::text_dim())
                .map(|d| carve::bg(d, SeancePalette::surface()))
                .hover(|s| s.text_color(SeancePalette::flame()))
                .cursor_pointer()
                .tooltip(tip("flip to Classic, on this circle's lead"))
                .on_click({
                    let ws = ws.clone();
                    cx.listener(move |this, _, window, cx| this.chat_show_terminal(&ws, window, cx))
                })
                .child("show terminal"),
            )
        });

        let mut thread = div()
            .id("chat-thread")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.chat.scroll)
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_2();
        let messages = self.chat.threads.get(&ws).map(|t| t.messages.as_slice());
        // The last message Brian has read, on any device: the lead's messages up
        // to it show as seen.
        let read = self.chat.threads.get(&ws).map_or(0, |t| t.read);
        let all = messages.unwrap_or_default();
        let win = ThreadWindow::new(all.len(), self.chat.shown);
        if win.has_earlier() {
            thread = thread.child(
                carve::plate(
                    div().id("chat-earlier"),
                    SeancePalette::surface(),
                    Stock::Md,
                    Cut::Control,
                )
                .flex_none()
                .self_center()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(SeancePalette::text_dim())
                .map(|d| carve::bg(d, SeancePalette::surface()))
                .hover(|s| s.text_color(SeancePalette::flame()))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.chat_show_earlier(cx)))
                .child(format!(
                    "Show earlier ({} more)",
                    win.start.min(THREAD_STEP)
                )),
            );
        }
        for (i, m) in all.iter().enumerate().skip(win.start) {
            if let Some((_, below, gap)) = marker.filter(|(at, ..)| *at == i) {
                thread = thread.child(earlier_marker(below, gap));
            }
            thread = thread.child(self.chat_bubble(m, read, cx));
        }
        if messages.is_some_and(<[Msg]>::is_empty) {
            let no_claude = chat_local::is_local() && !chat_local::has_claude(&ws);
            let mut empty = div().flex().flex_col().gap(px(8.)).child(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_faint())
                    .child(if no_claude {
                        chat_local::NO_LEAD_MESSAGE
                    } else {
                        "No messages yet. Say something to the lead."
                    }),
            );
            if no_claude {
                let at = ws.clone();
                empty = empty.child(
                    super::stage::chip_button("Start Claude here", true)
                        .id("chat-start-claude")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.chat_start_claude(at.clone(), cx)
                        })),
                );
            }
            thread = thread.child(empty);
        }

        let mut col = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .when(crate::skin::torches(), |d| {
                let at = self.chat.thread_w.clone();
                d.relative().child(
                    gpui::canvas(
                        move |b, _, _| at.set(Some(b.size.width.into())),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .when(carve::stone(), |d| {
                d.relative().children(crate::skin::backdrop(carve::STONE))
            })
            .child(header)
            .children(usage_strip(
                self.chat.usage.as_ref(),
                self.chat.threads.get(&ws).and_then(|t| t.context),
                self.chat
                    .threads
                    .get(&ws)
                    .and_then(|t| t.context)
                    .filter(|c| *c >= CTX_ORANGE && !read_only && !chat_local::is_local())
                    .map(|_| self.compact_button(&ws, cx)),
            ));
        if let Some(e) = &self.chat.error {
            col = col.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(SeancePalette::danger())
                    .child(format!("Invoke: {e}")),
            );
        }
        if let Some(n) = &self.chat.notice {
            col = col.child(
                div()
                    .id("chat-notice")
                    .flex_none()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(SeancePalette::danger())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.chat.notice = None;
                        cx.notify();
                    }))
                    .child(n.clone()),
            );
        }
        let reply = self
            .chat
            .reply
            .as_ref()
            .filter(|r| r.ws == ws && !read_only);
        col.child(thread)
            .when_some(reply, |col, r| {
                col.child(reply_bar(
                    &r.text,
                    div()
                        .id("chat-reply-x")
                        .flex_none()
                        .px_1()
                        .cursor_pointer()
                        .hover(|s| s.text_color(SeancePalette::flame()))
                        .tooltip(tip("cancel the reply"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.chat.reply = None;
                            cx.notify();
                        }))
                        .child("✕"),
                ))
            })
            .when(!read_only, |col| {
                col.child(self.render_chat_composer(thread_w, cx))
            })
            .into_any_element()
    }

    fn render_chat_composer(&self, composer_w: f32, cx: &Context<Self>) -> AnyElement {
        let Some(input) = self.chat.input.as_ref() else {
            return div().into_any_element();
        };
        let typed = input.read(cx).value().to_string();
        let ws = self.chat.selected.clone().unwrap_or_default();
        let chip = |id: SharedString| {
            carve::plate(
                div().id(id),
                SeancePalette::surface(),
                Stock::Full,
                Cut::Control,
            )
            .px_2()
            .py_0p5()
            .when(carve::on(), |d| carve::bg(d, SeancePalette::surface()))
            .when(!carve::on(), |d| {
                d.border_1().border_color(SeancePalette::border())
            })
            .text_xs()
            .text_color(SeancePalette::text_dim())
            .hover(|s| s.text_color(SeancePalette::flame()))
            .cursor_pointer()
        };
        // Above the input: quick replies while it is empty, or the panes an
        // `@` at the start can name.
        let mut above = div().flex().flex_wrap().gap_1p5();
        // Stop ends the row while the lead works, whatever is typed, but gives
        // way to the @ suggestions.
        let mentioning = typed
            .strip_prefix('@')
            .is_some_and(|t| !t.contains(char::is_whitespace));
        let lead_busy = self.chat.threads.get(&ws).is_some_and(|t| t.lead_busy);
        if typed.is_empty() {
            for (i, text) in QUICK_REPLIES.into_iter().enumerate() {
                let ws = ws.clone();
                above = above.child(
                    chip(SharedString::from(format!("chat-quick-{i}")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.chat_post(ws.clone(), text.to_string(), None, false, cx)
                        }))
                        .child(text),
                );
            }
        } else if let Some(typed) = typed
            .strip_prefix('@')
            .filter(|t| !t.contains(char::is_whitespace))
        {
            let typed = typed.to_lowercase();
            let panes = self.chat.panes.get(&ws).map_or(&[][..], Vec::as_slice);
            for (i, p) in panes
                .iter()
                .filter(|p| {
                    p.helper
                        && (p.at.to_lowercase().starts_with(&typed)
                            || p.name.to_lowercase().contains(&typed))
                })
                .take(6)
                .enumerate()
            {
                let at = p.at.clone();
                let state = if p.asleep {
                    " · asleep"
                } else if p.busy {
                    " · busy"
                } else {
                    ""
                };
                above = above.child(
                    chip(SharedString::from(format!("chat-mention-{i}")))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.chat_mention(&at, window, cx)
                        }))
                        .child(format!("@{} {}{state}", p.at, p.name)),
                );
            }
        }
        let send_torch = crate::skin::torches()
            && decor::over_send_fits(composer_w, typed.is_empty() || lead_busy, mentioning);
        if lead_busy && !mentioning {
            let stop_ws = ws.clone();
            above = above.child(
                chip(SharedString::from("chat-stop"))
                    .tooltip(tip("stop what the lead is doing"))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.chat_stop(stop_ws.clone(), cx)),
                    )
                    .child("Stop"),
            );
        }
        div()
            .flex_none()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_t_1()
            .border_color(SeancePalette::border())
            .map(|d| crate::skin::under(d, crate::skin::STRIP))
            .when(send_torch, |d| d.child(decor::over_send(self.torch_frame)))
            .child(above)
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_2()
                    .when(!chat_local::is_local(), |d| {
                        d.child(
                            carve::plate(
                                div().id("chat-attach"),
                                SeancePalette::surface(),
                                Stock::Md,
                                Cut::Control,
                            )
                            .flex_none()
                            .px_2()
                            .py_1p5()
                            .text_sm()
                            .text_color(SeancePalette::text_dim())
                            .map(|d| carve::bg(d, SeancePalette::surface()))
                            .hover(|s| s.text_color(SeancePalette::flame()))
                            .cursor_pointer()
                            .tooltip(tip("send a file or picture"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.chat_attach(window, cx)),
                            )
                            .child("📎"),
                        )
                    })
                    .child(
                        carve::carve(div(), SeancePalette::bg(), Cut::Slot)
                            .flex_1()
                            .min_w_0()
                            .when(carve::on(), |d| carve::bg(d, SeancePalette::bg()).px_1())
                            .child(Input::new(input).appearance(!carve::on())),
                    )
                    .child(
                        carve::plate(
                            div().id("chat-send"),
                            SeancePalette::flame(),
                            Stock::Md,
                            Cut::Control,
                        )
                        .flex_none()
                        .px_3()
                        .py_1p5()
                        .text_sm()
                        .text_color(SeancePalette::bg())
                        .map(|d| carve::bg(d, SeancePalette::flame()))
                        // A slick fill is painted, so it dims the whole button.
                        .when(!carve::slick(), |d| {
                            d.hover(|s| s.bg(SeancePalette::flame_dim()))
                        })
                        .when(carve::slick(), |d| d.hover(|s| s.opacity(0.8)))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.chat_send(window, cx)))
                        .child("Send"),
                    ),
            )
            .into_any_element()
    }

    /// A "▶ video" tile in a picture's place: the file name, and a click that
    /// downloads it (no automatic retry) and opens it in its app. In a
    /// recolored bubble it takes the quote's tint and the ink.
    fn video_tile(
        &self,
        id: i64,
        i: usize,
        name: &str,
        ink: Option<MeInk>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let state = self.chat.videos.get(name);
        let status = match state {
            Some(VideoState::Downloading) => "downloading…".to_string(),
            Some(VideoState::Failed(why)) => format!("couldn't download: {why}. Click to retry."),
            None => "click to open".to_string(),
        };
        let busy = state == Some(&VideoState::Downloading);
        let failed = matches!(state, Some(VideoState::Failed(_)));
        let file = name.to_string();
        let (fill, title, label, note) = match ink {
            Some(i) => (hex(i.quote_bg), hex(i.ink), hex(i.ink), hex(i.ink)),
            None => (
                SeancePalette::bg().opacity(0.4),
                SeancePalette::text(),
                SeancePalette::text_dim(),
                if failed {
                    SeancePalette::danger()
                } else {
                    SeancePalette::text_faint()
                },
            ),
        };
        div()
            .id(SharedString::from(format!("bubble-video-{id}-{i}")))
            .flex_none()
            .w(px(IMG_MAX_W))
            .h(px(IMG_MAX_W * 9. / 16.))
            .px_3()
            .map(|d| carve::corners(d, Stock::Md))
            .overflow_hidden()
            .bg(fill)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .child(div().text_color(title).child("▶ video"))
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_xs()
                    .text_color(label)
                    .child(name.to_string()),
            )
            .child(div().text_xs().text_color(note).child(status))
            .when(!busy, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.open_video(&file, cx)))
            })
            .into_any_element()
    }

    /// Fetch a video through Invoke and open it in its app. One try per click.
    fn open_video(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.chat.videos.get(name) == Some(&VideoState::Downloading) {
            return;
        }
        self.chat
            .videos
            .insert(name.to_string(), VideoState::Downloading);
        cx.notify();
        let name = name.to_string();
        cx.spawn(async move |this, cx| {
            let leaf = name.clone();
            let got = cx
                .background_executor()
                .spawn(async move { chat_api::file_within(&leaf, VIDEO_SECS) })
                .await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                match got {
                    Ok(path) => {
                        app.chat.videos.remove(&name);
                        crate::sysopen::open_detached(&path.to_string_lossy());
                    }
                    Err(e) => {
                        app.chat
                            .videos
                            .insert(name, VideoState::Failed(e.to_string()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Me on the right, the lead on the left, anything else centered.
    fn chat_bubble(&self, m: &Msg, read: i64, cx: &Context<Self>) -> AnyElement {
        let me = m.from == "me";
        let system = !me && m.from != "lead";
        let ink = active_me_ink(me);
        let mut body = div().flex().flex_col().gap_0p5();
        if let Some(r) = &m.reply {
            let id = r.id;
            body = body.child(
                quote_block(&r.from, &r.text, ink)
                    .id(SharedString::from(format!("bubble-quote-{}", m.id)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.chat_quote_jump(id, cx))),
            );
        }
        // Every picture, stacked above the text, each in a box whose size is
        // fixed up front: the decoded size fitted to the bubble, or a plain
        // box until the file has arrived. Nothing can paint outside it.
        for (i, name) in m.image_names().into_iter().enumerate() {
            if is_video(name) {
                body = body.child(self.video_tile(m.id, i, name, ink, cx));
                continue;
            }
            let found = self.chat.images.get(name);
            let (bw, bh) = found.map_or((IMG_MAX_W, IMG_LOADING_H), |(_, w, h)| fit_image(*w, *h));
            let frame = div()
                .id(SharedString::from(format!("bubble-img-{}-{i}", m.id)))
                .flex_none()
                .w(px(bw))
                .h(px(bh))
                .map(|d| carve::corners(d, Stock::Md))
                .overflow_hidden()
                .bg(SeancePalette::bg().opacity(0.4));
            body = body.child(match found {
                // Click opens the cached file in its app (Preview).
                Some((path, ..)) => {
                    let file = path.clone();
                    frame
                        .cursor_pointer()
                        .on_click(move |_, _, _| {
                            crate::sysopen::open_detached(&file.to_string_lossy())
                        })
                        .child(
                            img(path.clone())
                                .size_full()
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                }
                None => frame,
            });
        }
        if let Some(a) = &m.attachment {
            body = body.child(attachment_link(m.id, a, ink.map(|i| hex(i.ink))));
        }
        let spec = m.spec.as_deref().filter(|_| !me);
        if let Some(title) = spec {
            body = body
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(SeancePalette::violet())
                        .child("SPEC"),
                )
                .child(div().font_semibold().child(title.to_string()));
        }
        let shown = if spec.is_some() {
            spec_body(&m.text)
        } else {
            m.text.as_str()
        };
        if !shown.is_empty() && !attachment_only(m) {
            let (text, markdown) = (shown.to_string(), to_markdown(shown));
            body = body.child(
                div()
                    .id(SharedString::from(format!("bubble-text-{}", m.id)))
                    .min_h(px(14.))
                    // Capture phase, so the selection view cannot swallow it;
                    // the selection itself lands after, hence the defer.
                    .capture_any_mouse_up(cx.listener(
                        move |_, ev: &gpui::MouseUpEvent, window, cx| {
                            if ev.button == gpui::MouseButton::Left && ev.click_count == 2 {
                                let text = text.clone();
                                cx.defer_in(window, move |_, window, cx| {
                                    open_path_under_double_click(&text, window, cx);
                                });
                            }
                        },
                    ))
                    .child({
                        let view = TextView::markdown(
                            SharedString::from(format!("bubble-md-{}", m.id)),
                            markdown,
                        )
                        .style(bubble_text_style())
                        .selectable(true);
                        match ink {
                            Some(i) => LinkInk {
                                child: view.into_any_element(),
                                link: hex(i.ink),
                            }
                            .into_any_element(),
                            None => view.into_any_element(),
                        }
                    }),
            );
        }
        let stamp = if m.ts > 0 { clock(m.ts) } else { String::new() };
        if system {
            return div()
                .flex()
                .justify_center()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .child(body)
                .into_any_element();
        }
        let (fill, words) = bubble_fill(
            me,
            ink,
            SeancePalette::violet_dim(),
            SeancePalette::surface(),
            SeancePalette::text(),
        );
        let bubble = carve::worn_plate(div(), fill, Stock::Lg)
            .max_w(relative(0.72))
            .px_3()
            .py_2()
            .text_sm()
            .text_color(words)
            .map(|d| carve::bg(d, fill))
            .when(spec.is_some() && !carve::slick(), |d| {
                d.border_1().border_color(SeancePalette::violet())
            })
            .child(body)
            .child(
                div()
                    .mt_0p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(ink.map_or_else(
                        || {
                            plain_stamp(
                                me,
                                crate::theme::active().texture.is_some(),
                                SeancePalette::text(),
                                SeancePalette::text_dim(),
                                SeancePalette::text_faint(),
                            )
                        },
                        |i| hex(i.soft),
                    ))
                    .child(stamp)
                    .children(self.tick_marks(m, read, ink)),
            );
        let (weak, ws, msg) = (cx.weak_entity(), self.chat.selected.clone(), m.clone());
        let bubble = bubble
            .id(SharedString::from(format!("bubble-{}", m.id)))
            .context_menu(move |menu, window, cx| {
                let ws = ws.clone().unwrap_or_default();
                bubble_menu(menu, window, cx, &weak, &ws, &msg)
            });
        // The bubble's row, with the reply arrow on the side facing the
        // middle; the row is the hover group, so the arrow stays while the
        // pointer moves from the bubble onto it.
        let ws = self.chat.selected.as_deref().unwrap_or_default();
        let arrow = reply_arrow_side(ws, m)
            .zip(reply_draft(ws, m))
            .filter(|_| !chat_local::is_local());
        let row = div()
            .group(REPLY_GROUP)
            .w_full()
            .flex()
            .items_center()
            .when(me, |d| d.justify_end());
        let row = match arrow {
            Some((side, draft)) => {
                let arrow = reply_arrow(m.id).on_click(
                    cx.listener(move |this, _, _, cx| this.chat_reply_to(draft.clone(), cx)),
                );
                match side {
                    ArrowSide::Left => row.child(arrow).child(bubble),
                    ArrowSide::Right => row.child(bubble).child(arrow),
                }
            }
            None => row.child(bubble),
        };
        if m.choices.is_empty() || me {
            return row.into_any_element();
        }
        // The lead's choices, as pills under its message.
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .child(row)
            .child(self.choice_pills(m, cx))
            .into_any_element()
    }

    /// Read ticks, as on the phone: one dim ✓ while a note waits for the lead,
    /// ✓✓ in the accent color once the lead has seen it and when answered; on a lead's
    /// message, ✓✓ in the accent once Brian has read it. A failed note says so
    /// in red. Hover for what it means. A recolored bubble draws them in its
    /// ink: full once seen, softer while waiting.
    fn tick_marks(&self, m: &Msg, read: i64, ink: Option<MeInk>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if m.from == "me" && m.status == "failed" {
            out.push(
                div()
                    .text_color(ink.map_or_else(SeancePalette::danger, |i| hex(i.ink)))
                    .child("· not delivered")
                    .into_any_element(),
            );
        }
        if let Some((glyph, accent, tip_text)) = tick_for(m, read) {
            out.push(
                div()
                    .id(SharedString::from(format!("tick-{}", m.id)))
                    .font_semibold()
                    .text_color(match (ink, accent) {
                        (Some(i), true) => hex(i.ink),
                        (Some(i), false) => hex(i.soft),
                        (None, true) => SeancePalette::violet(),
                        (None, false) => SeancePalette::text_dim(),
                    })
                    .tooltip(tip(tip_text))
                    .child(glyph)
                    .into_any_element(),
            );
        }
        out
    }

    /// Tap-to-answer pills in a wrapping row. The first is the lead's pick, so
    /// it is tinted. Once one is tapped (here or elsewhere) all go dead, the
    /// tapped one is filled and the rest are dimmed.
    fn choice_pills(&self, m: &Msg, cx: &Context<Self>) -> AnyElement {
        let picked = m
            .picked
            .clone()
            .or_else(|| self.chat.tapped.get(&m.id).cloned());
        let id = m.id;
        // A sign-in card: a masked box and Send in place of the code "choice".
        let code_row = (chat_local::is_local()
            && m.choices.iter().any(|c| c == chat_local::CODE_BOX))
        .then(|| {
            if self.chat.code_sent.contains(&id) {
                return div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("Sent")
                    .into_any_element();
            }
            div()
                .flex()
                .items_center()
                .gap_2()
                .children(
                    self.chat
                        .code_input
                        .as_ref()
                        .map(|i| div().w(px(260.)).child(Input::new(i))),
                )
                .child(
                    carve::plate(
                        div().id(SharedString::from(format!("code-send-{id}"))),
                        SeancePalette::surface(),
                        Stock::Full,
                        Cut::Control,
                    )
                    .px_3()
                    .py_1()
                    .text_sm()
                    .text_color(SeancePalette::violet())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.chat_send_code(window, cx);
                    }))
                    .child("Send"),
                )
                .into_any_element()
        });
        let pills = div()
            .max_w(relative(0.72))
            .flex()
            .flex_wrap()
            .gap_2()
            .children(
                m.choices
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| *c != chat_local::CODE_BOX)
                    .map(|(i, choice)| {
                        let is_picked = picked.as_deref() == Some(choice.as_str());
                        let lead_pick = i == 0 && picked.is_none();
                        let fill = if is_picked {
                            SeancePalette::violet()
                        } else if lead_pick {
                            SeancePalette::violet().opacity(0.14)
                        } else {
                            SeancePalette::surface()
                        };
                        let pill = carve::plate(
                            div().id(SharedString::from(format!("choice-{id}-{i}"))),
                            // Slick paints the pill's own fill; stone bevels from surface.
                            if carve::slick() {
                                fill
                            } else {
                                SeancePalette::surface()
                            },
                            Stock::Full,
                            Cut::Control,
                        )
                        .px_3()
                        .py_1()
                        .text_sm()
                        .when(!carve::slick(), |d| {
                            d.border_1().border_color(if is_picked || lead_pick {
                                SeancePalette::violet()
                            } else {
                                SeancePalette::border()
                            })
                        })
                        .text_color(if is_picked {
                            SeancePalette::bg()
                        } else if lead_pick {
                            SeancePalette::violet()
                        } else {
                            SeancePalette::text()
                        })
                        .when(is_picked || lead_pick, |d| carve::bg(d, fill))
                        .when(picked.is_some() && !is_picked, |d| d.opacity(0.4))
                        .child(choice.clone());
                        if picked.is_some() {
                            return pill;
                        }
                        let spec_change = changes_spec(m, choice);
                        let choice = choice.clone();
                        pill.cursor_pointer()
                            .when(!carve::slick(), |d| {
                                d.hover(|s| s.bg(SeancePalette::violet().opacity(0.25)))
                            })
                            .when(carve::slick(), |d| d.hover(|s| s.opacity(0.8)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if spec_change {
                                    this.chat.focus_input = true;
                                    cx.notify();
                                } else if chat_local::is_local()
                                    && choice == chat_local::SHOW_TERMINAL
                                {
                                    // The way out of any dialog card: the pane itself.
                                    if let Some(ws) = this.chat.selected.clone() {
                                        this.chat_show_terminal(&ws, window, cx);
                                    }
                                } else {
                                    this.chat_answer(id, choice.clone(), cx);
                                }
                            }))
                    }),
            );
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_2()
            .children(code_row)
            .child(pills)
            .into_any_element()
    }

    /// Send the sign-in code to the pane and clear the box at once. The text
    /// lives only in this call: not in the thread, not echoed, not logged.
    fn chat_send_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(ws), Some(input)) = (self.chat.selected.clone(), self.chat.code_input.clone())
        else {
            return;
        };
        let Some(id) = self.chat.threads.get(&ws).and_then(|t| {
            t.messages
                .iter()
                .rev()
                .find(|m| m.choices.iter().any(|c| c == chat_local::CODE_BOX))
                .map(|m| m.id)
        }) else {
            return;
        };
        if self.chat.code_sent.contains(&id) {
            return;
        }
        let code = input.read(cx).value().to_string();
        if code.trim().is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.set_value("", window, cx));
        let Some(lead) = chat_local::lead_of(&ws) else {
            return;
        };
        self.chat.code_sent.insert(id);
        let client = Arc::clone(&self.client);
        cx.spawn(async move |this, cx| {
            let sent = cx
                .background_executor()
                .spawn(async move { chat_local::send_code(&client, &ws, &lead, &code) })
                .await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |app: &mut SeanceApp, cx| {
                if let Err(e) = sent {
                    app.chat.code_sent.remove(&id);
                    app.chat.error = Some(format!("not sent: {e}"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn move_to_space_lists_every_other_space_and_moves_the_circle() {
        let mut spaces = seance_core::spaces::Spaces::default();
        let circles: Vec<(String, String)> = ["a", "b", "c"]
            .map(|c| (c.to_string(), c.to_string()))
            .into();
        spaces.reconcile(&circles, None);
        let work = spaces.default_id().to_string();
        let home = spaces.create("Home");
        let all: Vec<(String, String)> = spaces
            .list
            .iter()
            .map(|x| (x.id.clone(), x.name.clone()))
            .collect();
        let targets = move_targets(all.iter().map(|(i, n)| (i.as_str(), n.as_str())), &work);
        assert_eq!(targets, [(home.clone(), "Home".to_string())]);
        // The menu item's action: the circle is in Home, the others stay.
        let order: Vec<String> = ["a", "b", "c"].map(String::from).into();
        assert!(spaces.move_circle("b", &home));
        assert_eq!(spaces.space_of("b"), home);
        assert_eq!(spaces.circles_in(&work, &order), ["a", "c"]);
        // Order is the circles' own, so a move keeps each one's place.
        assert!(spaces.move_circle("a", &home));
        assert_eq!(spaces.circles_in(&home, &order), ["a", "b"]);
    }

    #[test]
    fn a_new_circle_opens_its_chat() {
        let mut chat = ChatState::default();
        chat.selected = Some("old".to_string());
        chat.notice = Some("a note".to_string());
        chat.open_new_circle("circle-7");
        assert_eq!(chat.selected.as_deref(), Some("circle-7"));
        assert_eq!(chat.notice, None);
    }

    #[test]
    fn a_space_with_no_circle_still_gets_a_heading() {
        let shown: std::collections::HashSet<String> = ["a".to_string()].into();
        let spaces = [("a", "Work"), ("b", "new space"), ("c", "Home")];
        let names: Vec<_> = empty_spaces(spaces.into_iter(), &shown)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(names, ["new space", "Home"]);
        assert!(empty_spaces(std::iter::empty(), &shown).is_empty());
    }

    use super::*;

    #[test]
    fn wall_torches_leave_out_any_that_would_reach_the_text_or_chips() {
        // A wide header with a short name: all five, at uneven heights.
        let all = decor::wall_spots(1100., 150., 900.);
        assert_eq!(all.iter().map(|s| s.0).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        assert!(all.windows(2).all(|w| w[0].2 != w[1].2));
        // A long doing line reaching into the cluster drops its left end only.
        let left = decor::wall_spots(1100., 400., 900.);
        assert!(!left.is_empty() && left.len() < 5);
        assert!(left.iter().all(|s| s.1 >= 408.));
        assert_eq!(left.last().map(|s| s.0), Some(4));
        // Narrow: the chips crowd in from the right, and the far ones go.
        let right = decor::wall_spots(500., 100., 200.);
        assert!(right.iter().all(|s| s.1 + 12. <= 192.));
        assert!(right.len() < 5);
        // Text right up to the chips: none.
        assert!(decor::wall_spots(700., 500., 520.).is_empty());
    }

    #[test]
    fn no_two_decorative_torches_flicker_in_step() {
        let phases: HashSet<u64> = decor::SEEDS
            .iter()
            .map(|n| crate::torch::seed(n) % 12)
            .collect();
        assert_eq!(phases.len(), decor::SEEDS.len());
    }

    #[test]
    fn the_send_torch_shows_only_over_its_usual_chip_row() {
        assert!(decor::over_send_fits(800., true, false));
        // @ suggestions can fill the row.
        assert!(!decor::over_send_fits(800., true, true));
        // No row above the input: the spot would be the input.
        assert!(!decor::over_send_fits(800., false, false));
        // Too narrow for the quick replies and the torch side by side.
        assert!(!decor::over_send_fits(300., true, false));
    }

    #[test]
    fn a_spec_body_drops_its_title_line() {
        assert_eq!(spec_body("Title\nbody\nmore"), "body\nmore");
        assert_eq!(spec_body("Title\n\nbody"), "body");
        assert_eq!(spec_body("Title only"), "");
    }

    #[test]
    fn change_only_refocuses_on_a_spec_card() {
        let spec = Msg {
            spec: Some("T".into()),
            ..Default::default()
        };
        assert!(changes_spec(&spec, "Change"));
        assert!(!changes_spec(&spec, "Go"));
        assert!(!changes_spec(&Msg::default(), "Change"));
    }

    #[test]
    fn markdown_reads_back_as_the_same_text() {
        assert_eq!(to_markdown("a *b* _c_ #d"), "a \\*b\\* \\_c\\_ \\#d");
        assert_eq!(to_markdown("1. one\n\ntwo"), "1\\. one\n\n\u{a0}\n\ntwo");
    }

    #[test]
    fn every_newline_is_a_paragraph_of_its_own() {
        use markdown::mdast::Node;
        let md = to_markdown("png\nimage: /home/x.png\nimage: /home/y.png");
        let Node::Root(root) = markdown::to_mdast(&md, &markdown::ParseOptions::default()).unwrap()
        else {
            panic!("no root")
        };
        // Three lines, three paragraphs, and no break node the renderer would drop.
        assert_eq!(root.children.len(), 3);
        assert!(root
            .children
            .iter()
            .all(|n| matches!(n, Node::Paragraph(_))));
    }

    #[test]
    fn videos_are_told_apart_by_extension() {
        for v in ["a.mp4", "B.MOV", "x.webm", "y.m4v"] {
            assert!(is_video(v), "{v}");
        }
        for p in ["a.png", "b.jpeg", "mp4", "clip.mp4.png"] {
            assert!(!is_video(p), "{p}");
        }
    }

    #[test]
    fn urls_become_links_without_their_trailing_punctuation() {
        assert_eq!(
            to_markdown("see https://bike.rodeo/x/. ok"),
            "see [https\\:\\/\\/bike\\.rodeo\\/x\\/](<https://bike.rodeo/x/>)\\. ok"
        );
        assert!(to_markdown("(https://a.b/c)").contains("](<https://a.b/c>)"));
    }

    #[test]
    fn a_double_clicked_word_finds_only_its_own_path() {
        let text = "wrote `~/Claude/out/a.png`, and /tmp/b.txt. See https://x.y/z";
        assert_eq!(path_of_word(text, "out"), Some("~/Claude/out/a.png"));
        assert_eq!(path_of_word(text, "b.txt"), Some("/tmp/b.txt"));
        assert_eq!(path_of_word(text, "nope"), None);
        // A URL is not a file path.
        assert_eq!(path_of_word(text, "x.y"), None);
    }

    #[test]
    fn a_word_of_the_sentence_is_not_a_click_on_the_file() {
        let text = "I put the png in ~/Claude/out/a.png";
        assert_eq!(path_of_word(text, "png"), None);
        assert_eq!(path_of_word(text, "a.png"), Some("~/Claude/out/a.png"));
        // Punctuation picked by the word select is not a path word.
        assert_eq!(path_of_word(text, "/"), None);
        assert_eq!(path_of_word(text, "."), None);
        // The same word in two paths cannot say which was clicked.
        assert_eq!(path_of_word("/a/out/x and /b/out/y", "out"), None);
    }

    #[test]
    fn indentation_and_blank_space_survive_as_markdown() {
        let nb = "\u{a0}";
        assert_eq!(to_markdown("    code"), format!("{nb}{nb}{nb}{nb}code"));
        assert_eq!(to_markdown("\tx"), format!("{nb}{nb}{nb}{nb}x"));
        assert_eq!(to_markdown("a\n  - b"), format!("a\n\n{nb}{nb}\\- b"));
        // Whitespace-only lines are blank lines; the tail of a message is not.
        assert_eq!(to_markdown("a\n   \nb"), format!("a\n\n{nb}\n\nb"));
        assert_eq!(to_markdown("a\n   "), "a");
        assert_eq!(to_markdown("a\n\n"), "a");
    }

    #[test]
    fn reset_labels_use_a_12_hour_clock() {
        // Whatever the machine's zone, the shape is fixed.
        let short = reset_label(1_793_000_000_000, false);
        assert!(short.ends_with("AM") || short.ends_with("PM"), "{short}");
        assert!(short.contains(':'), "{short}");
        let long = reset_label(1_793_000_000_000, true);
        assert!(
            !long.contains(':') && long.split(' ').count() == 3,
            "{long}"
        );
    }

    #[test]
    fn usage_strip_hides_without_windows() {
        assert!(usage_strip(Some(&Usage::default()), None, None).is_none());
        assert!(usage_strip(None, None, None).is_none());
        let one = Usage {
            five_hour: Some(UsageWindow {
                used: 18.,
                resets: None,
            }),
            seven_day: None,
        };
        assert!(usage_strip(Some(&one), None, None).is_some());
        // The lead's context alone is enough to show the strip.
        assert!(usage_strip(None, Some(28), None).is_some());
        // Calm under 30, yellow from 30, orange from 50 (Compact's threshold).
        assert_eq!(
            [0, 29, 30, 49, 50, 100].map(ctx_heat),
            [
                Heat::Calm,
                Heat::Calm,
                Heat::Yellow,
                Heat::Yellow,
                Heat::Orange,
                Heat::Orange
            ]
        );
    }

    #[test]
    fn active_is_unread_or_recent_newest_first_and_never_alerts() {
        let now = 10 * ACTIVE_MS;
        let circle = |ws: &str, unread: u32, age: Option<u64>| CircleRow {
            ws: ws.into(),
            unread,
            last: age.map(|a| Msg {
                ts: now - a,
                ..Default::default()
            }),
            ..Default::default()
        };
        let circles = [
            circle("old", 0, Some(ACTIVE_MS + 1)),
            circle("recent", 0, Some(60_000)),
            circle("unread-old", 2, Some(5 * ACTIVE_MS)),
            circle("alerts", 3, Some(1_000)),
            circle("quiet", 0, None),
            circle("newest", 0, Some(1_000)),
            circle("unread-empty", 1, None),
        ];
        let got: Vec<String> = active_circles(&circles, now)
            .into_iter()
            .map(|c| c.ws)
            .collect();
        assert_eq!(got, ["newest", "recent", "unread-old", "unread-empty"]);
    }

    #[test]
    fn ticks_follow_a_note_s_status_and_the_read_mark() {
        let mine = |status: &str| Msg {
            id: 5,
            from: "me".into(),
            status: status.into(),
            ..Default::default()
        };
        assert_eq!(
            tick_for(&mine("sending"), 0),
            Some(("✓", false, "Waiting for the lead"))
        );
        assert_eq!(
            tick_for(&mine("got"), 0),
            Some(("✓✓", true, "The lead has seen it"))
        );
        assert_eq!(
            tick_for(&mine("answered"), 0),
            Some(("✓✓", true, "Answered"))
        );
        assert_eq!(tick_for(&mine("failed"), 0), None);
        let lead = |id| Msg {
            id,
            from: "lead".into(),
            ..Default::default()
        };
        assert_eq!(tick_for(&lead(9), 9), Some(("✓✓", true, "Seen")));
        assert_eq!(tick_for(&lead(10), 9), None);
        assert_eq!(tick_for(&lead(-1), 9), None);
    }

    #[test]
    fn a_refresh_rereads_the_newest_twenty_messages() {
        let mut state = ChatState::default();
        let msg = |id| Msg {
            id,
            ..Default::default()
        };
        // Short: the whole thread.
        state.threads.insert(
            "w".into(),
            Thread {
                messages: (1..=20).map(msg).collect(),
                ..Default::default()
            },
        );
        assert_eq!(state.refresh_after("w"), None);
        // Longer: from just before the 20th from the end; ids are not dense.
        state.threads.insert(
            "w".into(),
            Thread {
                messages: (1..=30).map(|i| msg(i * 3)).collect(),
                ..Default::default()
            },
        );
        assert_eq!(state.refresh_after("w"), Some(30));
        assert_eq!(state.refresh_after("none"), None);
    }

    #[test]
    fn pictures_fit_the_bubble_at_their_own_shape() {
        // A portrait phone photo is held by its height, a wide screenshot by its width.
        assert_eq!(fit_image(3000, 4000), (315., 420.));
        assert_eq!(fit_image(1600, 800), (320., 160.));
        // Small ones are not blown up; unknown sizes get the loading box.
        assert_eq!(fit_image(100, 50), (100., 50.));
        assert_eq!(fit_image(0, 0), (IMG_MAX_W, IMG_LOADING_H));
    }

    #[test]
    fn preview_is_one_line_and_marks_your_messages() {
        let m = Msg {
            from: "me".into(),
            text: "hello\n  there".into(),
            ..Default::default()
        };
        assert_eq!(preview(&m), "You: hello there");
        let photo = Msg {
            from: "lead".into(),
            image: Some("a.png".into()),
            ..Default::default()
        };
        assert_eq!(preview(&photo), "📷 photo");
    }

    #[test]
    fn reminders_land_on_the_hour_the_menu_says() {
        const HOUR: u64 = 3600 * 1000;
        let now = now_ms();
        let [hour, three, tomorrow, monday] = remind_times(now);
        assert_eq!((hour, three), (now + HOUR, now + 3 * HOUR));
        // Local 9:00, tomorrow (a day ahead by the calendar) and a Monday.
        assert_eq!(events::local_clock(tomorrow).1, 9);
        assert_eq!(events::local_clock(tomorrow).2, 0);
        assert!(tomorrow > now && tomorrow - now <= 48 * HOUR);
        assert_eq!(events::local_clock(monday).0, 1);
        assert_eq!(events::local_clock(monday).1, 9);
        assert!(monday > now && monday - now <= 8 * 24 * HOUR);
    }

    #[test]
    fn a_note_that_is_only_its_document_shows_only_the_link() {
        let doc = |text: &str| Msg {
            text: text.into(),
            attachment: Some(Attachment {
                file: "a.pdf".into(),
                title: "plan.pdf".into(),
            }),
            ..Default::default()
        };
        assert!(attachment_only(&doc("📎 plan.pdf")));
        assert!(!attachment_only(&doc("📎 plan.pdf, please read")));
        assert!(!attachment_only(&Msg {
            text: "📎 plan.pdf".into(),
            ..Default::default()
        }));
    }

    #[test]
    fn search_waits_for_two_characters() {
        assert_eq!(search_query("  "), Err(None));
        assert_eq!(search_query(" a "), Err(Some("Type at least 2 characters")));
        assert_eq!(search_query(" ab "), Ok("ab"));
        assert_eq!(search_query("é1"), Ok("é1"));
    }

    #[test]
    fn the_thread_follows_the_bottom_until_he_scrolls_up() {
        // (offset, range): pinned and the content grew, offset unchanged.
        assert_eq!(
            follow_step(true, false, Some((-900., 900.)), (-900., 1000.)),
            (true, true)
        );
        // He scrolled up by hand: range unchanged, offset moved well off the bottom.
        assert_eq!(
            follow_step(true, false, Some((-900., 900.)), (-500., 900.)),
            (false, false)
        );
        // Then new content arrives: unpinned, so nothing pulls him down.
        assert_eq!(
            follow_step(false, false, Some((-500., 900.)), (-500., 1000.)),
            (false, false)
        );
        // Back within 80 points of the bottom: pinned again.
        assert_eq!(
            follow_step(false, false, Some((-500., 900.)), (-850., 900.)),
            (true, false)
        );
        // A little scroll up stays pinned but is not undone.
        assert_eq!(
            follow_step(true, false, Some((-900., 900.)), (-880., 900.)),
            (true, false)
        );
        // Past the bottom (overscroll): pinned, nothing to scroll.
        assert_eq!(
            follow_step(true, false, Some((-900., 900.)), (-930., 900.)),
            (true, false)
        );
        // Clamped onto the bottom by a resize while unpinned: pinned again.
        assert_eq!(
            follow_step(false, false, Some((-500., 900.)), (-1000., 1000.)),
            (true, false)
        );
        // First frame: pinned and off the bottom, scroll down.
        assert_eq!(follow_step(true, false, None, (0., 400.)), (true, true));
        // A search result being shown: never scrolled, whatever grows.
        assert_eq!(
            follow_step(true, true, Some((-100., 900.)), (-100., 1000.)),
            (true, false)
        );
        // A hand scroll while a result is shown still tracks the pin.
        assert_eq!(
            follow_step(true, true, Some((-900., 900.)), (-300., 900.)),
            (false, false)
        );
        // At the bottom already: nothing to do.
        assert_eq!(
            follow_step(true, false, Some((-900., 900.)), (-900., 900.)),
            (true, false)
        );
    }

    #[test]
    fn a_long_thread_draws_only_its_newest_messages() {
        let w = ThreadWindow::new(2000, 0);
        assert_eq!(w.start, 2000 - THREAD_WINDOW);
        assert!(w.has_earlier());
        // A short thread is drawn whole, with no "Show earlier" row.
        let w = ThreadWindow::new(30, 0);
        assert_eq!(w.start, 0);
        assert!(!w.has_earlier());
        // Never fewer than the window, whatever was asked.
        assert_eq!(ThreadWindow::new(500, 10).start, 500 - THREAD_WINDOW);
    }

    #[test]
    fn a_message_s_child_index_counts_the_show_earlier_row() {
        let w = ThreadWindow::new(200, THREAD_WINDOW);
        assert_eq!(w.child(w.start - 1), None, "not drawn");
        assert_eq!(w.child(w.start), Some(1), "under the row");
        assert_eq!(w.child(199), Some(THREAD_WINDOW));
        let whole = ThreadWindow::new(10, 0);
        assert_eq!(whole.child(0), Some(0), "no row, no shift");
    }

    #[test]
    fn show_earlier_adds_a_step_until_the_thread_is_whole() {
        let mut shown = 0;
        shown = show_earlier(shown, 1000);
        assert_eq!(shown, THREAD_WINDOW + THREAD_STEP);
        shown = show_earlier(shown, 1000);
        assert_eq!(shown, THREAD_WINDOW + 2 * THREAD_STEP);
        assert_eq!(ThreadWindow::new(1000, shown).start, 1000 - shown);
        // It stops at the whole thread.
        assert_eq!(show_earlier(990, 1000), 1000);
        assert!(!ThreadWindow::new(1000, show_earlier(990, 1000)).has_earlier());
    }

    #[test]
    fn only_brian_s_and_the_lead_s_saved_messages_take_a_reply() {
        let m = |id: i64, from: &str| Msg {
            id,
            from: from.into(),
            ..Default::default()
        };
        assert!(can_reply("l6", &m(5, "me")));
        assert!(can_reply("l6", &m(5, "lead")));
        assert!(!can_reply("l6", &m(5, "system")));
        assert!(!can_reply("l6", &m(5, "")));
        // A bubble Invoke has not confirmed yet has no id to reply to.
        assert!(!can_reply("l6", &m(-1, "me")));
        assert!(!can_reply(ALERTS_WS, &m(5, "lead")));
    }

    #[test]
    fn the_reply_arrow_faces_the_middle_and_only_where_a_reply_can_go() {
        let m = |id: i64, from: &str| Msg {
            id,
            from: from.into(),
            ..Default::default()
        };
        assert_eq!(reply_arrow_side("l6", &m(5, "me")), Some(ArrowSide::Left));
        assert_eq!(
            reply_arrow_side("l6", &m(5, "lead")),
            Some(ArrowSide::Right)
        );
        assert_eq!(reply_arrow_side("l6", &m(5, "system")), None);
        assert_eq!(reply_arrow_side("l6", &m(-1, "me")), None);
        assert_eq!(reply_arrow_side(ALERTS_WS, &m(5, "lead")), None);
    }

    #[test]
    fn the_menu_and_the_arrow_set_the_same_reply() {
        let m = Msg {
            id: 7,
            from: "lead".into(),
            ts: 42,
            text: "ship it?".into(),
            ..Default::default()
        };
        assert_eq!(
            reply_draft("l6", &m),
            Some(ReplyDraft {
                ws: "l6".into(),
                id: 7,
                from: "lead".into(),
                ts: 42,
                text: "ship it?".into(),
            })
        );
        assert_eq!(reply_draft(ALERTS_WS, &m), None);
        assert_eq!(reply_draft("l6", &Msg { id: 0, ..m }), None);
    }

    #[test]
    fn a_reply_clears_on_a_sent_note_and_stays_on_a_failed_one() {
        let draft = |ws: &str, id| ReplyDraft {
            ws: ws.into(),
            id,
            from: "lead".into(),
            ts: 0,
            text: "go?".into(),
        };
        assert_eq!(reply_after_send(Some(draft("l6", 9)), Some(9), true), None);
        assert_eq!(
            reply_after_send(Some(draft("l6", 9)), Some(9), false),
            Some(draft("l6", 9))
        );
        // Another reply picked while the first was on its way stays.
        assert_eq!(
            reply_after_send(Some(draft("l6", 10)), Some(9), true),
            Some(draft("l6", 10))
        );
        assert_eq!(
            reply_after_send(Some(draft("l6", 9)), None, true),
            Some(draft("l6", 9))
        );
        assert_eq!(reply_after_send(None, Some(9), true), None);
        // Opening another circle drops it; the same circle keeps it.
        assert_eq!(reply_after_select(Some(draft("l6", 9)), "rudle"), None);
        assert_eq!(
            reply_after_select(Some(draft("l6", 9)), "l6"),
            Some(draft("l6", 9))
        );
        // The bubble shown while it sends quotes the same message.
        assert_eq!(draft("l6", 9).quote().id, 9);
    }

    #[test]
    fn a_quote_names_its_writer_and_reads_on_one_line() {
        assert_eq!(quote_label("me"), "You");
        assert_eq!(quote_label("lead"), "Lead");
        assert_eq!(one_line("two\n  lines\there"), "two lines here");
        assert_eq!(one_line(" \n"), "Photo or file");
    }

    #[test]
    fn a_quote_click_widens_to_an_old_original_and_skips_a_missing_one() {
        let ids: Vec<i64> = (1..=500).collect();
        // Message 10 sits well above the newest 80.
        let (shown, child) = jump_plan(THREAD_WINDOW, &ids, 10).unwrap();
        let w = ThreadWindow::new(ids.len(), shown);
        assert!(w.start <= 9);
        assert_eq!(w.child(9), Some(child));
        // One already drawn leaves the window as it is.
        assert_eq!(
            jump_plan(THREAD_WINDOW, &ids, 490).unwrap().0,
            THREAD_WINDOW
        );
        // An original that is not loaded is no jump at all.
        assert_eq!(jump_plan(THREAD_WINDOW, &ids, 9999), None);
    }

    #[test]
    fn a_search_hit_widens_the_window_to_reach_it() {
        let len = 900;
        // Already drawn: nothing changes.
        assert_eq!(widen_to(0, len, 880), THREAD_WINDOW);
        // Far back: drawn from it on, and it is the first message under the row.
        let shown = widen_to(0, len, 100);
        let w = ThreadWindow::new(len, shown);
        assert_eq!(w.start, 100);
        assert_eq!(w.child(100), Some(1));
        // The very first message: the whole thread, no row.
        let w = ThreadWindow::new(len, widen_to(THREAD_WINDOW, len, 0));
        assert_eq!((w.start, w.child(0)), (0, Some(0)));
    }

    #[test]
    fn arrivals_slide_the_window_while_pinned_and_grow_it_while_reading() {
        // Pinned to the bottom: the newest stay drawn, the oldest drop off.
        let shown = shown_after_arrival(THREAD_WINDOW, 3, false);
        assert_eq!(ThreadWindow::new(503, shown).start, 503 - THREAD_WINDOW);
        // Even after "Show earlier": back to the newest window.
        assert_eq!(shown_after_arrival(400, 1, false), THREAD_WINDOW);
        // Scrolled up reading: the first drawn message stays the first.
        let before = ThreadWindow::new(500, THREAD_WINDOW).start;
        let shown = shown_after_arrival(THREAD_WINDOW, 3, true);
        assert_eq!(ThreadWindow::new(503, shown).start, before);
        // From the default (0) too.
        let shown = shown_after_arrival(0, 2, true);
        assert_eq!(ThreadWindow::new(502, shown).start, 500 - THREAD_WINDOW);
    }

    #[test]
    fn opening_a_circle_resets_the_window() {
        let mut chat = ChatState {
            shown: 400,
            earlier: Some((7, 12.)),
            hold_follow: 2,
            ..Default::default()
        };
        chat.arrive();
        assert_eq!(chat.shown, THREAD_WINDOW);
        assert_eq!(chat.earlier, None);
        assert_eq!(chat.hold_follow, 0);
        assert!(!chat.unpinned);
        assert_eq!(
            ThreadWindow::new(2000, chat.shown).start,
            2000 - THREAD_WINDOW
        );
    }

    #[test]
    fn nothing_redraws_on_its_own_while_chat_is_idle() {
        assert_eq!(chat_frames(false, false, false, true), Frames::Idle);
        // Dances go by the timer, never every frame.
        assert_eq!(chat_frames(true, false, false, true), Frames::Dance);
        assert_eq!(chat_frames(false, false, true, true), Frames::Dance);
        // Only the party gets every frame.
        assert_eq!(chat_frames(true, true, true, true), Frames::Smooth);
        // Nobody looking: nothing at all.
        assert_eq!(chat_frames(true, true, true, false), Frames::Idle);
        assert!(DANCE_TICK >= Duration::from_millis(80) && DANCE_TICK <= Duration::from_millis(90));
    }

    #[test]
    fn following_the_bottom_settles_once_nothing_moves() {
        // Pinned, the range grows: one scroll down, then the next frames see
        // the bottom and stop asking for more.
        let mut seen = Some((-900., 900.));
        let mut pinned = true;
        let mut steps = Vec::new();
        for now in [
            (-900., 1000.),
            (-1000., 1000.),
            (-1000., 1000.),
            (-1000., 1000.),
        ] {
            let (p, down) = follow_step(pinned, false, seen, now);
            pinned = p;
            seen = Some(now);
            steps.push(down);
        }
        assert_eq!(steps, [true, false, false, false]);
        assert!(pinned);
    }

    /// Offscreen estimate of one Chat-thread draw: a mock thread of
    /// markdown bubbles shaped like `chat_bubble`'s (the real view needs a
    /// daemon), drawn in gpui's headless window. Compares every message
    /// (what the thread drew before windowing) with [`ThreadWindow`]'s.
    /// `cargo test --release --bin seance thread_draw_cost -- --ignored --nocapture`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "headless Metal benchmark; run with --ignored --nocapture"]
    fn thread_draw_cost() {
        use gpui::AppContext as _;
        use std::sync::Arc;

        struct Mock {
            texts: Vec<String>,
            from: usize,
            scroll: ScrollHandle,
        }
        impl gpui::Render for Mock {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let mut thread = div()
                    .id("t")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px_4()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_2();
                for (i, text) in self.texts.iter().enumerate().skip(self.from) {
                    let body = div().flex().flex_col().gap_0p5().child(
                        div()
                            .id(SharedString::from(format!("bt-{i}")))
                            .min_h(px(14.))
                            .child(
                                TextView::markdown(
                                    SharedString::from(format!("md-{i}")),
                                    to_markdown(text),
                                )
                                .style(bubble_text_style())
                                .selectable(true),
                            ),
                    );
                    let bubble = div()
                        .id(SharedString::from(format!("b-{i}")))
                        .max_w(relative(0.72))
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .text_sm()
                        .bg(gpui::rgb(0x333344))
                        .child(body)
                        .child(
                            div()
                                .mt_0p5()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .child("11:34 AM")
                                .child("✓✓"),
                        );
                    thread = thread.child(
                        div()
                            .flex()
                            .when(i % 2 == 0, |d| d.justify_end())
                            .child(bubble),
                    );
                }
                div().size_full().flex().child(thread)
            }
        }

        // About Invoke's average message (seance-demo: 905 messages, ~280
        // characters each), with a line break and a path now and then.
        let text = |i: usize| {
            let mut t = format!(
                "Message {i}: the lead finished the step and checked the build. \
                 Tests pass on the branch; the review notes are in the pad. "
            );
            t.push_str("Next it reads src/app/chat.rs around the thread render and the timer.\n");
            t.push_str("Then it reports back with the numbers and what is still open.");
            t
        };
        let median = |n: usize, from: usize| -> (f64, f64) {
            let platform = gpui_platform::current_platform(true);
            let mut cx = gpui::HeadlessAppContext::with_platform(
                platform.text_system(),
                Arc::new(()),
                gpui_platform::current_headless_renderer,
            );
            cx.update(gpui_component::init);
            let texts: Vec<String> = (0..n).map(text).collect();
            let win = cx
                .open_window(gpui::size(px(900.), px(800.)), move |_, cx| {
                    cx.new(|_| Mock {
                        texts,
                        from,
                        scroll: ScrollHandle::new(),
                    })
                })
                .expect("headless window");
            cx.run_until_parked();
            let mut ms = Vec::new();
            for _ in 0..22 {
                let d = cx
                    .update_window(win.into(), |_, window, cx| {
                        window.refresh();
                        let t = Instant::now();
                        window.draw(cx).clear();
                        t.elapsed()
                    })
                    .unwrap();
                ms.push(d.as_secs_f64() * 1000.);
            }
            let first = ms[0];
            // The first draws parse the markdown; the rest are what every
            // redraw costs.
            let mut warm = ms[2..].to_vec();
            warm.sort_by(f64::total_cmp);
            (warm[warm.len() / 2], first)
        };
        for n in [905, 2000] {
            let (all, all_first) = median(n, 0);
            let from = ThreadWindow::new(n, 0).start;
            let (win, win_first) = median(n, from);
            eprintln!(
                "[thread_draw_cost] {n} messages: all drawn median {all:.2} ms (first {all_first:.1}); \
                 window of {} median {win:.2} ms (first {win_first:.1}); {:.1}x",
                n - from,
                all / win
            );
        }
    }

    /// "Show earlier" in a headless window: the message that was first is
    /// at the same place on screen after more are drawn above it, and still
    /// there once the marker is gone.
    #[cfg(target_os = "macos")]
    #[test]
    fn show_earlier_keeps_the_first_message_in_place() {
        use gpui::AppContext as _;
        use std::sync::Arc;

        struct Mock {
            n: usize,
            from: usize,
            marker: Option<(usize, f32, f32)>,
            scroll: ScrollHandle,
        }
        impl gpui::Render for Mock {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let mut thread = div()
                    .id("t")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px_4()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_2();
                if self.from > 0 {
                    thread = thread.child(div().flex_none().h(px(28.)));
                }
                for i in self.from..self.n {
                    if let Some((_, below, gap)) = self.marker.filter(|(at, ..)| *at == i) {
                        thread = thread.child(earlier_marker(below, gap));
                    }
                    let h = 40. + (i % 5) as f32 * 13.;
                    thread = thread.child(div().flex_none().h(px(h)));
                }
                div().size_full().flex().child(thread)
            }
        }

        let platform = gpui_platform::current_platform(true);
        let mut cx = gpui::HeadlessAppContext::with_platform(
            platform.text_system(),
            Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        let n = 300;
        let scroll = ScrollHandle::new();
        let first = ThreadWindow::new(n, 0);
        let handle = scroll.clone();
        let win = cx
            .open_window(gpui::size(px(600.), px(500.)), move |_, cx| {
                cx.new(|_| Mock {
                    n,
                    from: first.start,
                    marker: None,
                    scroll: handle,
                })
            })
            .expect("headless window");
        cx.run_until_parked();
        let draw = |cx: &mut gpui::HeadlessAppContext| {
            cx.update_window(win.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
        };
        let on_screen = |child: usize| {
            let b = scroll.bounds_for_item(child).expect("laid out");
            f32::from(b.top() + scroll.offset().y - scroll.bounds().top())
        };
        draw(&mut cx);
        // Scrolled up near the top, the row and the first message in view.
        scroll.set_offset(gpui::point(px(0.), px(-20.)));
        draw(&mut cx);
        let at = first.start;
        let before = on_screen(first.child(at).unwrap());
        assert!(
            before > 0.,
            "the first message sits under the row: {before}"
        );

        let wider = ThreadWindow::new(n, show_earlier(0, n));
        let gap = cx
            .update_window(win.into(), |_, window, _| {
                f32::from(window.rem_size()) * 0.5
            })
            .unwrap();
        win.update(&mut cx, |m, _, _| {
            m.from = wider.start;
            m.marker = Some((at, before, gap));
        })
        .unwrap();
        scroll.scroll_to_top_of_item(wider.child(at).unwrap());
        draw(&mut cx);
        // The marker is the child at the old index; the message is next.
        let after = on_screen(wider.child(at).unwrap() + 1);
        assert!(
            (after - before).abs() < 1.,
            "kept its place: {before} then {after}"
        );
        win.update(&mut cx, |m, _, _| m.marker = None).unwrap();
        draw(&mut cx);
        let settled = on_screen(wider.child(at).unwrap());
        assert!(
            (settled - before).abs() < 1.,
            "the marker took no room: {before} then {settled}"
        );
    }

    #[test]
    fn only_brians_bubble_on_a_recoloring_scheme_gets_ink() {
        let neon = seance_core::schemes::find("neon-district")
            .unwrap()
            .me_bubble;
        let i = me_ink(true, neon).expect("his bubble on neon");
        assert_eq!((i.bg, i.ink), (0xD6168F, 0xFFFFFF));
        // The lead's bubble and every scheme without the field keep the palette.
        assert_eq!(me_ink(false, neon), None);
        for s in &seance_core::schemes::SCHEMES {
            if s.me_bubble.is_none() {
                assert_eq!(me_ink(true, s.me_bubble), None, "{}", s.id);
            }
        }
    }

    #[test]
    fn a_skin_lifts_plain_bubble_times_and_flat_themes_stay_faint() {
        let (text, dim, faint) = (hex(0xE6E1D6), hex(0xACA79D), hex(0x75716A));
        assert_eq!(plain_stamp(true, true, text, dim, faint), text);
        assert_eq!(plain_stamp(false, true, text, dim, faint), dim);
        assert_eq!(plain_stamp(true, false, text, dim, faint), faint);
        assert_eq!(plain_stamp(false, false, text, dim, faint), faint);
    }

    #[test]
    fn bubble_fill_falls_back_to_the_palette_without_ink() {
        let (me_bg, lead_bg, text) = (hex(0x6C5B95), hex(0x211C1D), hex(0xEBE3DB));
        assert_eq!(bubble_fill(true, None, me_bg, lead_bg, text), (me_bg, text));
        assert_eq!(
            bubble_fill(false, None, me_bg, lead_bg, text),
            (lead_bg, text)
        );
        let neon = seance_core::schemes::find("neon-district")
            .unwrap()
            .me_bubble;
        let ink = me_ink(true, neon);
        assert_eq!(
            bubble_fill(true, ink, me_bg, lead_bg, text),
            (hex(0xD6168F), hex(0xFFFFFF))
        );
    }

    #[test]
    fn a_light_ink_darkens_its_quote_and_keeps_a_soft_ink_that_reads() {
        use seance_core::schemes::{contrast, MeBubble};
        let light = me_ink(
            true,
            Some(MeBubble {
                bg: 0xC8128A,
                ink: 0xFFF4E0,
            }),
        )
        .unwrap();
        // Toward black, not toward the ink.
        assert_eq!(
            light.quote_bg,
            seance_core::schemes::mix(0xC8128A, 0, QUOTE_TINT)
        );
        // The full step would read 3.6:1; the stepped-back one holds.
        assert!(
            contrast(
                seance_core::schemes::mix(0xFFF4E0, 0xC8128A, SOFT_INK),
                0xC8128A
            ) < 4.5
        );
        assert!(contrast(light.soft, light.bg) >= 4.5);
        // A dark ink on a light fill keeps the full step.
        let dark = me_ink(
            true,
            Some(MeBubble {
                bg: 0xFFB0E0,
                ink: 0x04010A,
            }),
        )
        .unwrap();
        assert_eq!(
            dark.soft,
            seance_core::schemes::mix(0x04010A, 0xFFB0E0, SOFT_INK)
        );
        assert_eq!(
            dark.quote_bg,
            seance_core::schemes::mix(0xFFB0E0, 0x04010A, QUOTE_TINT)
        );
    }

    #[test]
    fn a_draft_lands_after_whatever_is_typed() {
        assert_eq!(with_draft("", "pick A"), "pick A");
        assert_eq!(with_draft("hi", "pick A"), "hi pick A");
        assert_eq!(with_draft("hi ", "pick A"), "hi pick A");
        assert_eq!(with_draft("hi\n", "pick A"), "hi\npick A");
    }

    #[test]
    fn the_selected_row_is_washed_only_where_the_scheme_recolors_bubbles() {
        let neon = seance_core::schemes::find("neon-district")
            .unwrap()
            .me_bubble;
        assert_eq!(selected_row_bg(neon), hex(0xD6168F).opacity(ROW_WASH));
        assert!(selected_row_bar(neon).is_some());
        assert!(selected_row_bar(None).is_none());
        assert_eq!(selected_row_bg(None), SeancePalette::surface());
        // Neon keeps its wash and bar in every look; stone frames with chain.
        let find = |id| seance_core::schemes::find(id).unwrap();
        assert_eq!(row_plate(find("neon-district")), neon);
        assert_eq!(row_plate(find("castle")), None);
        assert_eq!(row_plate(find("candlelit")), None);
    }

    #[test]
    fn every_ink_in_a_recolored_bubble_holds_4_5_to_1() {
        use seance_core::schemes::contrast;
        for s in &seance_core::schemes::SCHEMES {
            let Some(i) = me_ink(true, s.me_bubble) else {
                continue;
            };
            // Text, links, the seen tick: ink on the fill.
            assert!(contrast(i.ink, i.bg) >= 4.5, "{} ink", s.id);
            // The time and a waiting tick, on the fill.
            assert!(contrast(i.soft, i.bg) >= 4.5, "{} soft", s.id);
            // A quote's words and a video tile's lines, on the tint.
            assert!(contrast(i.ink, i.quote_bg) >= 4.5, "{} quote", s.id);
            // The tint and the softer ink still differ from what they sit on.
            assert_ne!(i.quote_bg, i.bg, "{}", s.id);
            assert_ne!(i.soft, i.ink, "{}", s.id);
        }
    }
}
