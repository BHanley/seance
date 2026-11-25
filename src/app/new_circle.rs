//! The New circle dialog: Personal or Work, an issue for Work, a name, then
//! Create. Esc anywhere makes the plain circle "+" always made.
//!
//! What decides something is a pure function (link parsing, the name, the
//! space, the first message, reading gh's JSON); the dialog only draws and
//! wires them.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, Focusable as _, SharedString, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};

use crate::theme::SeancePalette;

use super::SeanceApp;

/// How long one gh call gets.
const GH_SECS: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Personal,
    Work,
}

impl Kind {
    fn env(self) -> &'static str {
        match self {
            Kind::Personal => "personal",
            Kind::Work => "work",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Issue {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub labels: Vec<String>,
    pub subs: Vec<String>,
}

// ---- pure rules ----

/// `https://github.com/o/r/issues/12` (scheme, query and fragment optional)
/// or `o/r#12`. Only github.com; anything else is None.
pub(super) fn parse_issue_ref(text: &str) -> Option<(String, String, u64)> {
    let t = text.trim();
    let (owner, repo, num) = match t.split_once('#') {
        Some((slug, n)) if !slug.contains("://") && !slug.contains(' ') => {
            let (o, r) = slug.split_once('/')?;
            (o, r, n)
        }
        _ => {
            let rest = t
                .strip_prefix("https://")
                .or_else(|| t.strip_prefix("http://"))
                .unwrap_or(t);
            let rest = rest.strip_prefix("www.").unwrap_or(rest);
            let rest = rest.strip_prefix("github.com/")?;
            let rest = rest.split(['?', '#']).next()?;
            let mut parts = rest.split('/');
            let (o, r, kind, n) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            if kind != "issues" {
                return None;
            }
            (o, r, n)
        }
    };
    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let n: u64 = num.parse().ok().filter(|n| *n > 0)?;
    (plain(owner) && plain(repo)).then(|| (owner.to_string(), repo.to_string(), n))
}

/// The circle's name from an issue title: the text before the first colon.
pub(super) fn name_from_title(title: &str) -> String {
    let t = title.trim();
    match t.split_once(':') {
        Some((head, _)) if !head.trim().is_empty() => head.trim().to_string(),
        _ => t.to_string(),
    }
}

/// Where the new circle goes. Personal: the space named Personal, else the
/// one on show. Work: the one on show, unless that is Personal, then the space
/// named Projects, else the default.
pub(super) fn target_space(
    kind: Kind,
    shown: &str,
    spaces: &[(String, String)],
    default: &str,
) -> String {
    let named = |want: &str| {
        spaces
            .iter()
            .find(|(_, n)| n.trim().eq_ignore_ascii_case(want))
            .map(|(id, _)| id.clone())
    };
    match kind {
        Kind::Personal => named("Personal").unwrap_or_else(|| shown.to_string()),
        Kind::Work if named("Personal").as_deref() == Some(shown) => {
            named("Projects").unwrap_or_else(|| default.to_string())
        }
        Kind::Work => shown.to_string(),
    }
}

/// What the lead reads first.
pub(super) fn first_message(url: &str) -> String {
    format!(
        "Start from this issue: {url}\nRead the issue and its comments, its sub-issues, the PRs and issues it links (with their live state) and any Figma links. Save the link at the top of this circle's LEAD-LOG.md if it has one. Then tell me where it stands and what's still open, and ask what to start with."
    )
}

/// Invoke only delivers to a pane running an agent: its `takes_tasks` test
/// on the first pane's command.
pub(super) fn command_takes_tasks(command: Option<&str>) -> bool {
    command.is_some_and(|c| {
        ["claude", "codex", "lead-resume"]
            .iter()
            .any(|a| c.contains(a))
    })
}

/// The first pane's command line with the circle's kind set for it.
pub(super) fn with_kind(command: String, kind: Option<Kind>) -> String {
    match kind {
        Some(k) => format!("env SEANCE_CIRCLE_KIND={} {command}", k.env()),
        None => command,
    }
}

fn issue_from(v: &serde_json::Value) -> Option<Issue> {
    let full = v.pointer("/repository/nameWithOwner")?.as_str()?;
    let (owner, repo) = full.split_once('/')?;
    Some(Issue {
        owner: owner.into(),
        repo: repo.into(),
        number: v.get("number")?.as_u64()?,
        title: v.get("title")?.as_str()?.into(),
        url: v.get("url")?.as_str()?.into(),
        ..Default::default()
    })
}

/// `gh search issues --json ...`, read. None when it isn't a list.
pub(super) fn parse_search(json: &str) -> Option<Vec<Issue>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(v.as_array()?.iter().filter_map(issue_from).collect())
}

/// `gh api repos/o/r/issues/N`, read.
pub(super) fn parse_issue(json: &str, owner: &str, repo: &str) -> Option<Issue> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let name = |l: &serde_json::Value| l.get("name")?.as_str().map(str::to_string);
    Some(Issue {
        owner: owner.into(),
        repo: repo.into(),
        number: v.get("number")?.as_u64()?,
        title: v.get("title")?.as_str()?.into(),
        url: v.get("html_url")?.as_str()?.into(),
        state: v.get("state")?.as_str()?.into(),
        labels: v
            .get("labels")?
            .as_array()?
            .iter()
            .filter_map(name)
            .collect(),
        subs: Vec::new(),
    })
}

/// `.../sub_issues`: the titles.
pub(super) fn parse_subs(json: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let title = |s: &serde_json::Value| s.get("title")?.as_str().map(str::to_string);
    v.as_array()
        .map(|a| a.iter().filter_map(title).collect())
        .unwrap_or_default()
}

/// The owners in a list of issues, once each, in order.
pub(super) fn owners(issues: &[Issue]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in issues {
        if !out.contains(&i.owner) {
            out.push(i.owner.clone());
        }
    }
    out
}

// ---- gh, off the draw thread ----

/// `gh` on the login shell's PATH (the app's own is thin), else where
/// Homebrew puts it.
fn find_gh() -> Option<PathBuf> {
    let on_path = crate::claude_signin::login_shell_path()
        .unwrap_or_default()
        .split(':')
        .map(|d| PathBuf::from(d).join("gh"))
        .find(|p| p.is_file());
    on_path.or_else(|| {
        ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    })
}

/// Run gh with these args (never a shell), token variables unset. None when
/// gh is missing, signed out, or slow.
fn gh(args: &[String]) -> Option<String> {
    let mut cmd = std::process::Command::new(find_gh()?);
    cmd.args(args)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN");
    crate::claude_signin::capture(cmd, GH_SECS).filter(|s| !s.trim().is_empty())
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| x.to_string()).collect()
}

const FIELDS: &str = "number,title,url,repository";

/// Your open issues, newest first.
fn fetch_mine() -> Option<Vec<Issue>> {
    let a = args(&[
        "search",
        "issues",
        "--assignee",
        "@me",
        "--state",
        "open",
        "--sort",
        "updated",
        "--order",
        "desc",
        "--limit",
        "30",
        "--json",
        FIELDS,
    ]);
    parse_search(&gh(&a)?)
}

/// A search by words, limited to the owners of your own list when it has any.
fn fetch_search(words: &str, mine: &[Issue]) -> Option<Vec<Issue>> {
    let mut a = args(&["search", "issues", "--state", "open", "--limit", "20"]);
    for o in owners(mine) {
        a.extend(["--owner".to_string(), o]);
    }
    a.extend(args(&["--json", FIELDS, "--"]));
    a.extend(words.split_whitespace().map(str::to_string));
    parse_search(&gh(&a)?)
}

/// One issue in full: its fields and its sub-issues' titles.
fn fetch_issue(owner: &str, repo: &str, n: u64) -> Option<Issue> {
    let path = format!("repos/{owner}/{repo}/issues/{n}");
    let mut issue = parse_issue(&gh(&["api".into(), path.clone()])?, owner, repo)?;
    if let Some(j) = gh(&["api".into(), format!("{path}/sub_issues")]) {
        issue.subs = parse_subs(&j);
    }
    Some(issue)
}

// ---- the dialog ----

/// What the list step shows once gh has answered, or couldn't.
fn mine_from(got: Option<Vec<Issue>>) -> Mine {
    got.map_or(Mine::NoGh, Mine::Got)
}

enum Mine {
    Loading,
    Got(Vec<Issue>),
    /// No gh, or not signed in: the paste box alone, no error.
    NoGh,
}

pub(super) struct Dialog {
    kind: Option<Kind>,
    /// Past the issue step: Personal from the start, Work once an issue is
    /// picked or "No issue" is chosen.
    named: bool,
    paste: Entity<InputState>,
    name: Entity<InputState>,
    /// A pasted link is being looked up.
    resolving: bool,
    mine: Mine,
    hits: Option<Vec<Issue>>,
    picked: Option<Issue>,
}

/// Chords the dialog keeps from the app behind it (new pane, kill pane, next
/// circle...). Plain keys and editing chords go to the inputs.
fn swallows(key: &str, mods: &gpui::Modifiers) -> bool {
    let editing =
        mods.platform && !mods.control && !mods.alt && matches!(key, "a" | "c" | "v" | "x" | "z");
    (mods.control || mods.alt || mods.platform) && !editing
}

/// What the app keeps for the dialog.
#[derive(Default)]
pub(super) struct NewCircleUi {
    dlg: Option<Dialog>,
    /// Counts openings, so an answer that comes back after Esc and a reopen
    /// is dropped.
    gen: u64,
    /// Set by Create for the first pane's command; taken when it spawns.
    spawn_kind: Option<Kind>,
}

impl NewCircleUi {
    pub(super) fn is_open(&self) -> bool {
        self.dlg.is_some()
    }

    /// The command for a new circle's first pane, with its kind if Create set one.
    pub(super) fn first_pane_command(&mut self, command: Option<String>) -> Option<String> {
        let kind = self.spawn_kind.take();
        command.map(|c| with_kind(c, kind))
    }
}

impl SeanceApp {
    pub(super) fn open_new_circle_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paste = cx.new(|cx| InputState::new(window, cx).placeholder("Paste a link or search"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        cx.subscribe_in(
            &paste,
            window,
            |this: &mut SeanceApp, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.nc_submit(window, cx);
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &name,
            window,
            |this: &mut SeanceApp, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.nc_create(window, cx);
                }
            },
        )
        .detach();
        self.new_circle.dlg = Some(Dialog {
            kind: None,
            named: false,
            paste,
            name,
            resolving: false,
            mine: Mine::Loading,
            hits: None,
            picked: None,
        });
        self.new_circle.gen += 1;
        let g = self.new_circle.gen;
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let got = cx.background_executor().spawn(async { fetch_mine() }).await;
            let _ = this.update(cx, |this, cx| {
                if this.new_circle.gen != g {
                    return;
                }
                if let Some(d) = this.new_circle.dlg.as_mut() {
                    d.mine = mine_from(got);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Esc anywhere, P and W on the first step. True when the key was spent.
    pub(super) fn new_circle_key(
        &mut self,
        key: &str,
        mods: gpui::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(d) = self.new_circle.dlg.as_ref() else {
            return false;
        };
        let first = d.kind.is_none();
        if key != "escape" && swallows(key, &mods) {
            return true;
        }
        match key {
            "escape" => {
                // The plain circle, as "+" made it before the dialog.
                self.new_circle.dlg = None;
                self.make_new_circle(None, window, cx);
            }
            "p" if first && !mods.modified() => self.nc_kind(Kind::Personal, window, cx),
            "w" if first && !mods.modified() => self.nc_kind(Kind::Work, window, cx),
            _ => return first,
        }
        true
    }

    /// Focus an input once it is drawn (next frame), not before.
    fn nc_focus(&self, name: bool, window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, move |this, window, cx| {
            if let Some(d) = this.new_circle.dlg.as_ref() {
                let input = if name { &d.name } else { &d.paste };
                let handle = input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
        });
    }

    fn nc_kind(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.new_circle.dlg.as_mut() else {
            return;
        };
        d.kind = Some(kind);
        d.named = kind == Kind::Personal;
        let name = d.named;
        self.nc_focus(name, window, cx);
        cx.notify();
    }

    /// Move to the name step, with the issue (or none) and its title's name.
    fn nc_named(&mut self, issue: Option<Issue>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.new_circle.dlg.as_mut() else {
            return;
        };
        d.named = true;
        let name = issue.as_ref().map(|i| name_from_title(&i.title));
        d.picked = issue;
        if let Some(n) = name {
            d.name.update(cx, |i, cx| i.set_value(n, window, cx));
        }
        self.nc_focus(true, window, cx);
        cx.notify();
    }

    /// Fetch one issue. `pick` moves on when it arrives; otherwise it fills
    /// in the details of the issue already picked.
    fn nc_resolve(
        &mut self,
        (owner, repo, n): (String, String, u64),
        pick: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let g = self.new_circle.gen;
        if pick {
            if let Some(d) = self.new_circle.dlg.as_mut() {
                d.resolving = true;
            }
        }
        cx.spawn_in(window, async move |this: gpui::WeakEntity<Self>, cx| {
            let got = cx
                .background_executor()
                .spawn({
                    let (o, r) = (owner.clone(), repo.clone());
                    async move { fetch_issue(&o, &r, n) }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.new_circle.gen != g {
                    return;
                }
                if let Some(d) = this.new_circle.dlg.as_mut() {
                    d.resolving = false;
                }
                // Without gh a pasted link still works: the link is all the
                // lead needs.
                let found = got.or_else(|| {
                    pick.then(|| Issue {
                        url: format!("https://github.com/{owner}/{repo}/issues/{n}"),
                        owner,
                        repo,
                        number: n,
                        ..Default::default()
                    })
                });
                let Some(i) = found else { return };
                if pick {
                    this.nc_named(Some(i), window, cx);
                } else if let Some(p) = this.new_circle.dlg.as_mut().and_then(|d| d.picked.as_mut())
                {
                    if p.url == i.url {
                        *p = i;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Enter in the paste box: a link picks that issue, words search.
    fn nc_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.new_circle.dlg.as_ref() else {
            return;
        };
        let text = d.paste.read(cx).value().trim().to_string();
        if d.resolving {
            return;
        }
        if let Some(found) = parse_issue_ref(&text) {
            self.nc_resolve(found, true, window, cx);
        } else if !text.is_empty() {
            let mine = match &d.mine {
                Mine::Got(m) => m.clone(),
                _ => Vec::new(),
            };
            let g = self.new_circle.gen;
            cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
                let got = cx
                    .background_executor()
                    .spawn(async move { fetch_search(&text, &mine) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.new_circle.gen != g {
                        return;
                    }
                    if let Some(d) = this.new_circle.dlg.as_mut() {
                        d.hits = Some(got.unwrap_or_default());
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn nc_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.new_circle.dlg.as_mut() else {
            return;
        };
        d.picked = None;
        if d.kind == Some(Kind::Work) {
            d.named = false;
            self.nc_focus(false, window, cx);
        } else {
            d.kind = None;
            d.named = false;
        }
        cx.notify();
    }

    /// Create: the circle in its space, named, with its first pane, and for
    /// an issue the lead's first message.
    fn nc_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.new_circle.dlg.take() else {
            return;
        };
        let (Some(kind), true) = (d.kind, d.named) else {
            self.new_circle.dlg = Some(d);
            return;
        };
        let spaces = &self.subs_pref.spaces;
        let list: Vec<(String, String)> = spaces
            .list
            .iter()
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let space = target_space(kind, &self.shown_space(), &list, spaces.default_id());
        let command = crate::term_config::get().seance_new_circle_command.clone();
        let sends = command_takes_tasks(command.as_deref());
        let Some(ws) = self.make_new_circle(Some(space), window, cx) else {
            return;
        };
        let name = d.name.read(cx).value().trim().to_string();
        self.new_circle.spawn_kind = Some(kind);
        self.commit_rename(&name, cx);
        // The rename box queued a focus of its own; take focus back after it.
        cx.defer_in(window, |this, window, cx| {
            this.restore_keyboard_focus(window, cx)
        });
        if let (Some(issue), true, true) = (d.picked, kind == Kind::Work, sends) {
            let text = first_message(&issue.url);
            // The hub refuses until it lists a pane in the circle: try again
            // while it refuses. Any other failure (no answer, a timeout) may
            // have been delivered, so it is not repeated.
            std::thread::spawn(move || {
                for _ in 0..20 {
                    match crate::chat_api::send_when_ready(&ws, &text) {
                        Err(e) if e.downcast_ref::<crate::chat_api::Refused>().is_some() => {
                            std::thread::sleep(Duration::from_millis(1500));
                        }
                        _ => return,
                    }
                }
            });
        }
    }

    pub(super) fn render_new_circle(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let d = self.new_circle.dlg.as_ref()?;
        let kind_card = |id: &'static str,
                         kind: Kind,
                         title: &'static str,
                         note: &'static str,
                         key: &'static str| {
            let on = d.kind == Some(kind);
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(if on {
                    SeancePalette::flame()
                } else {
                    SeancePalette::border()
                })
                .bg(if on {
                    SeancePalette::surface()
                } else {
                    SeancePalette::bg()
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.nc_kind(kind, window, cx)))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(SeancePalette::text_faint())
                                .child(key),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(SeancePalette::text_dim())
                        .child(note),
                )
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            let b = div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer();
            if primary {
                b.bg(SeancePalette::flame())
                    .text_color(SeancePalette::bg())
                    .child(label)
            } else {
                b.bg(SeancePalette::surface())
                    .text_color(SeancePalette::text())
                    .child(label)
            }
        };
        let faint = |t: SharedString| {
            div()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .child(t)
        };
        let mut card = div()
            .id("new-circle-card")
            .w(px(520.))
            .max_h(gpui::relative(0.9))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(SeancePalette::border())
            .bg(SeancePalette::bg_elevated())
            .shadow_lg()
            .text_color(SeancePalette::text())
            // Clicks inside the card must not reach the backdrop.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("New circle"),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(kind_card(
                        "nc-personal",
                        Kind::Personal,
                        "Personal",
                        "Your own stuff. Starts in your Personal folder.",
                        "P",
                    ))
                    .child(kind_card(
                        "nc-work",
                        Kind::Work,
                        "Work",
                        "Starts from an issue if you have one.",
                        "W",
                    )),
            );
        // The middle scrolls on a short window; the footer (buttons) stays.
        let mut body = div()
            .id("nc-body")
            .flex_initial()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3();
        let mut footer: Option<AnyElement> = None;
        match (d.kind, d.named) {
            (None, _) => footer = Some(faint("Skip with Esc".into()).into_any_element()),
            (Some(_), false) => {
                let list: Option<&[Issue]> = match (&d.hits, &d.mine) {
                    (Some(h), _) => Some(h),
                    (None, Mine::Got(m)) => Some(m),
                    _ => None,
                };
                let caption = match (&d.hits, &d.mine) {
                    (Some(_), _) => "Search results",
                    (None, Mine::Loading) => "Looking up your issues...",
                    (None, Mine::Got(_)) => "Your open issues, newest first",
                    (None, Mine::NoGh) => "Paste an issue link",
                };
                body = body
                    .child(
                        div()
                            .text_sm()
                            .child("Is there an issue you're working on?"),
                    )
                    .child(Input::new(&d.paste))
                    .child(
                        div()
                            .id("nc-issues")
                            .flex_none()
                            .max_h(px(240.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(list.unwrap_or_default().iter().enumerate().map(|(n, i)| {
                                let pick = i.clone();
                                div()
                                    .id(SharedString::from(format!("nc-issue-{n}")))
                                    .flex()
                                    .justify_between()
                                    .gap_3()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_sm()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(SeancePalette::surface()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let (found, issue) = (
                                            (pick.owner.clone(), pick.repo.clone(), pick.number),
                                            pick.clone(),
                                        );
                                        this.nc_named(Some(issue), window, cx);
                                        this.nc_resolve(found, false, window, cx);
                                    }))
                                    .child(
                                        div().flex_1().min_w_0().truncate().child(i.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(SeancePalette::text_faint())
                                            .child(format!("{} {}", i.repo, i.number)),
                                    )
                            })),
                    );
                footer = Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(faint(caption.into()).flex_1().min_w_0())
                        .child(button("nc-none", "No issue", false).flex_none().on_click(
                            cx.listener(|this, _, window, cx| this.nc_named(None, window, cx)),
                        ))
                        .into_any_element(),
                );
            }
            (Some(kind), true) => {
                if let Some(i) = &d.picked {
                    let mut meta = vec![format!("{} {}", i.repo, i.number)];
                    if !i.state.is_empty() {
                        meta.push(i.state.clone());
                    }
                    meta.extend(i.labels.iter().cloned());
                    let subs = match i.subs.len() {
                        0 => "No sub-issues".to_string(),
                        n => format!(
                            "{n} sub-issue{}: {}",
                            if n == 1 { "" } else { "s" },
                            i.subs.join(", ")
                        ),
                    };
                    body = body.child(div().text_sm().child("Issue")).child(
                        div()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .rounded_md()
                            .border_1()
                            .border_color(SeancePalette::border())
                            .child(div().text_sm().font_weight(gpui::FontWeight::BOLD).child(
                                if i.title.is_empty() {
                                    i.url.clone()
                                } else {
                                    i.title.clone()
                                },
                            ))
                            .child(faint(meta.join(" · ").into()))
                            .children(
                                [
                                    "The issue and its comments".to_string(),
                                    subs,
                                    "Linked PRs and issues, with their live state".to_string(),
                                    "Figma links, if it has any".to_string(),
                                ]
                                .into_iter()
                                .map(|t| {
                                    div()
                                        .flex()
                                        .gap_2()
                                        .text_sm()
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_color(SeancePalette::success())
                                                .child("✓"),
                                        )
                                        .child(div().flex_1().min_w_0().child(t))
                                }),
                            ),
                    );
                }
                let spaces = &self.subs_pref.spaces;
                let list: Vec<(String, String)> = spaces
                    .list
                    .iter()
                    .map(|s| (s.id.clone(), s.name.clone()))
                    .collect();
                let into = target_space(kind, &self.shown_space(), &list, spaces.default_id());
                let into = spaces
                    .get(&into)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                body = body
                    .child(div().text_sm().child("Name"))
                    .child(Input::new(&d.name));
                footer = Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(faint(format!("Goes in {into}").into()).flex_1().min_w_0())
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .gap_2()
                                .child(button("nc-back", "Back", false).on_click(
                                    cx.listener(|this, _, window, cx| this.nc_back(window, cx)),
                                ))
                                .child(button("nc-create", "Create", true).on_click(
                                    cx.listener(|this, _, window, cx| this.nc_create(window, cx)),
                                )),
                        )
                        .into_any_element(),
                );
            }
        }
        // Step 1 has nothing in the middle: no empty box, no extra gap.
        if d.kind.is_some() {
            card = card.child(body);
        }
        card = card.children(footer);
        Some(
            gpui::deferred(
                div()
                    .id("new-circle-backdrop")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(SeancePalette::bg().opacity(0.6))
                    .child(card),
            )
            .with_priority(20)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(o: &str, n: &str, i: u64) -> Option<(String, String, u64)> {
        Some((o.into(), n.into(), i))
    }

    #[test]
    fn issue_links_and_names_parse_and_nothing_else_does() {
        for t in [
            "https://github.com/acme/widgets/issues/12",
            "github.com/acme/widgets/issues/12?x=1",
            " http://www.github.com/acme/widgets/issues/12#issuecomment-5 ",
            "acme/widgets#12",
        ] {
            assert_eq!(parse_issue_ref(t), r("acme", "widgets", 12), "{t}");
        }
        for t in [
            "https://github.com/acme/widgets/pull/12",
            "https://example.com/acme/widgets/issues/12",
            "https://github.com.evil.example/acme/widgets/issues/12",
            "https://github.com/acme/widgets/issues/0",
            "https://github.com/acme/widgets/issues/x",
            "https://github.com/acme/wid gets/issues/1",
            "fix the login bug #12",
            "",
        ] {
            assert_eq!(parse_issue_ref(t), None, "{t}");
        }
    }

    #[test]
    fn the_name_is_the_title_before_its_first_colon() {
        assert_eq!(name_from_title("Quick Nav: stops, routes"), "Quick Nav");
        assert_eq!(name_from_title("  Plain title "), "Plain title");
        assert_eq!(name_from_title(": odd start"), ": odd start");
        assert_eq!(name_from_title("A: b: c"), "A");
    }

    #[test]
    fn personal_and_work_pick_their_space() {
        let sp = |l: &[(&str, &str)]| -> Vec<(String, String)> {
            l.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        let all = sp(&[("d", "Default"), ("p", "personal"), ("j", "Projects")]);
        assert_eq!(target_space(Kind::Personal, "j", &all, "d"), "p");
        assert_eq!(target_space(Kind::Work, "j", &all, "d"), "j");
        assert_eq!(target_space(Kind::Work, "d", &all, "d"), "d");
        // Work from the Personal space goes to Projects, else the default.
        assert_eq!(target_space(Kind::Work, "p", &all, "d"), "j");
        let no_projects = sp(&[("d", "Default"), ("p", "Personal")]);
        assert_eq!(target_space(Kind::Work, "p", &no_projects, "d"), "d");
        // No Personal space: personal stays where it is.
        let none = sp(&[("d", "Default"), ("j", "Projects")]);
        assert_eq!(target_space(Kind::Personal, "j", &none, "d"), "j");
    }

    #[test]
    fn the_first_pane_carries_the_kind_only_when_chosen() {
        assert_eq!(
            with_kind("claude-mac".into(), Some(Kind::Work)),
            "env SEANCE_CIRCLE_KIND=work claude-mac"
        );
        assert_eq!(with_kind("claude-mac".into(), None), "claude-mac");
        let mut ui = NewCircleUi::default();
        ui.spawn_kind = Some(Kind::Personal);
        assert_eq!(
            ui.first_pane_command(Some("claude".into())).as_deref(),
            Some("env SEANCE_CIRCLE_KIND=personal claude")
        );
        // Taken once; an Esc circle runs the command as configured.
        assert_eq!(
            ui.first_pane_command(Some("claude".into())).as_deref(),
            Some("claude")
        );
        assert_eq!(ui.first_pane_command(None), None);
    }

    #[test]
    fn only_an_agent_pane_gets_the_held_message() {
        assert!(command_takes_tasks(Some(
            "env SEANCE_CIRCLE_KIND=work claude-mac"
        )));
        assert!(command_takes_tasks(Some("claude")));
        assert!(!command_takes_tasks(Some("zsh")));
        assert!(!command_takes_tasks(None));
        assert!(first_message("https://github.com/acme/widgets/issues/1")
            .starts_with("Start from this issue: https://github.com/acme/widgets/issues/1\n"));
    }

    #[test]
    fn the_dialog_keeps_app_chords_but_not_typing_or_editing() {
        let m = |platform, control, alt| gpui::Modifiers {
            platform,
            control,
            alt,
            ..Default::default()
        };
        assert!(!swallows("p", &m(false, false, false)));
        assert!(!swallows("v", &m(true, false, false)));
        assert!(swallows("w", &m(true, false, false)));
        assert!(swallows("n", &m(true, false, false)));
        assert!(swallows("w", &m(false, true, false)));
        assert!(swallows("pageup", &m(false, true, false)));
        assert!(swallows("n", &m(false, false, true)));
    }

    #[test]
    fn gh_output_reads_and_failure_means_no_list() {
        let list = r#"[{"number":3,"title":"One: a","url":"https://github.com/acme/widgets/issues/3","repository":{"nameWithOwner":"acme/widgets"}},
                       {"number":9,"title":"Two","url":"https://github.com/other/tool/issues/9","repository":{"nameWithOwner":"other/tool"}}]"#;
        let got = parse_search(list).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].repo.as_str(), got[0].number), ("widgets", 3));
        assert_eq!(owners(&got), ["acme", "other"]);
        // Signed out or no gh: nothing parses, and the step shows the paste box.
        assert_eq!(parse_search("gh: not logged in"), None);
        assert!(matches!(mine_from(None), Mine::NoGh));
        assert!(matches!(mine_from(parse_search(list)), Mine::Got(_)));
        let one = r#"{"number":3,"title":"One: a","html_url":"https://github.com/acme/widgets/issues/3","state":"open","labels":[{"name":"mobile"},{"name":"design"}]}"#;
        let i = parse_issue(one, "acme", "widgets").unwrap();
        assert_eq!((i.state.as_str(), i.labels.len()), ("open", 2));
        assert_eq!(
            parse_subs(r#"[{"title":"Alt routes"},{"title":"Button"}]"#),
            ["Alt routes", "Button"]
        );
        assert!(parse_subs("not json").is_empty());
    }
}
