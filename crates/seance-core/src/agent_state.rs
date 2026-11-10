//! What an agent TUI is doing, read off its rendered screen.
//!
//! Orchestrators kept screen-scraping this per agent and getting it wrong
//! (nudging busy panes, missing a quota wall). Seance owns the PTY, so it
//! answers once, here, for everyone: the daemon folds [`classify`] into
//! `status` / `roster` / `brief`, and `ctl send` uses it to confirm delivery.
//!
//! Pure over `(screen text, OSC title)` — no clock, no IO — so every marker
//! below is pinned by a test built from a real frame. The markers are
//! upstream UI strings, not a contract: **when an agent's TUI changes, a
//! working pane starts reading idle (or the reverse) and this file is where
//! the fix goes.**
//!
//! Precedence: `awaiting-input` (a modal wants an answer) > `busy` >
//! `limited` > `idle` > `unknown`. Busy/limited markers only count near the
//! bottom of the screen — the region above the input box, where both Claude
//! and Codex draw their live status — so an old transcript line quoting
//! "usage limit" or "Working (" can't flip the verdict.

use serde::Serialize;

/// Coarse activity of the agent (or program) in a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Activity {
    /// Mid-turn: spinner, "Working (…)", a running tool.
    Busy,
    /// At its prompt, ready for input.
    Idle,
    /// A modal (permission, trust, picker) is waiting for an answer.
    AwaitingInput,
    /// The agent reported a usage/quota wall and stopped.
    Limited,
    /// Not an agent TUI we recognise (shells, file panes, boot frames).
    Unknown,
}

impl Activity {
    pub fn as_str(self) -> &'static str {
        match self {
            Activity::Busy => "busy",
            Activity::Idle => "idle",
            Activity::AwaitingInput => "awaiting-input",
            Activity::Limited => "limited",
            Activity::Unknown => "unknown",
        }
    }
}

/// Usage-limit readings an agent shows on screen. Every field is optional:
/// only what the TUI actually printed is reported. `*_used_pct` come from a
/// Claude statusline (`[account-2] 5h:19% 7d:95%`); `*_left_pct` from Codex's
/// footer / warnings (`weekly 12% left`, `less than 5% of your weekly limit
/// left`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Quota {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_used_pct: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_used_pct: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_left_pct: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly_left_pct: Option<u32>,
}

impl Quota {
    pub fn is_empty(&self) -> bool {
        *self == Quota::default()
    }
}

/// [`classify`]'s verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentState {
    pub activity: Activity,
    /// The screen line that decided the verdict, trimmed (for humans/logs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<Quota>,
    /// Context window left before the agent compacts: Claude's
    /// "…until auto-compact: N%" / "N% until auto-compact", Codex's
    /// "N% context left".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_left_pct: Option<u32>,
    /// Text is waiting behind a running turn (Claude: "Press up to edit
    /// queued messages"). The agent hasn't read it yet.
    pub queued_input: bool,
}

/// A question on screen with numbered answers (Claude's permission and
/// choice menus): the line that asks, and each option's digit and label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NumberedPrompt {
    pub question: String,
    pub options: Vec<(u8, String)>,
}

/// The numbered options at the bottom of `screen`, when they run 1, 2, 3...
/// as one block. A dialog without digits (the trust dialog) gives none, so a
/// tap can never press a key that does nothing.
pub fn numbered_options(screen: &str) -> Option<NumberedPrompt> {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(TAIL_WINDOW + STATUS_WINDOW + 8);
    let parse = |l: &str| -> Option<(u8, String)> {
        let t = l.trim_start_matches(|c: char| c.is_whitespace() || c == '❯' || c == '>');
        let (d, rest) = t.split_once(". ")?;
        let n: u8 = d.parse().ok().filter(|n| (1..=9).contains(n))?;
        let label = rest.trim();
        (!label.is_empty()).then(|| (n, label.to_string()))
    };
    // The last block whose digits start at 1 and count up by one.
    let mut best: Option<(usize, Vec<(u8, String)>)> = None;
    let mut i = start;
    while i < lines.len() {
        if let Some((1, label)) = parse(lines[i]) {
            let mut opts = vec![(1u8, label)];
            let mut j = i + 1;
            while j < lines.len() {
                match parse(lines[j]) {
                    Some((n, l)) if n as usize == opts.len() + 1 => opts.push((n, l)),
                    _ if lines[j].trim().is_empty() || !lines[j].starts_with(' ') => break,
                    // A wrapped label line belongs to the option above it.
                    _ => {}
                }
                j += 1;
            }
            if opts.len() >= 2 {
                best = Some((i, opts));
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    let (at, options) = best?;
    let question = lines[start..at]
        .iter()
        .rev()
        .map(|l| l.trim())
        .find(|l| l.chars().any(|c| c.is_alphanumeric()))
        .unwrap_or("")
        .to_string();
    Some(NumberedPrompt { question, options })
}

/// How a dialog on the screen is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogKind {
    /// Options with digits: press the digit.
    Numbered,
    /// A list with a moving selection and no digits: arrows, then Enter.
    Arrows,
    /// Something is waiting that this can't read as a list.
    Unknown,
    /// Claude's sign-in step: it opened the browser and waits for the code the
    /// browser shows ("Paste code here if prompted >"). Answered with typed
    /// text, never a key.
    LoginCode,
}

/// An interactive dialog on a Claude pane's screen: the first-launch pickers
/// (theme, login, "Do you trust the files in this folder?") and permission
/// prompts, which are not in the transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct DialogPrompt {
    pub kind: DialogKind,
    pub question: String,
    /// Each option's label, in order (digits and the selection marker removed).
    pub options: Vec<String>,
    /// The option the selection marker is on now (arrows only).
    pub selected: usize,
    /// The bottom of the screen, for a dialog that can't be read as a list.
    pub screen: String,
}

/// The sign-in step is on screen: Claude's last line is its "Paste code here
/// if prompted >" prompt. Only the last non-blank line counts, so the words in
/// a reply, or a step that has moved on, are not it.
fn login_prompt(lines: &[&str]) -> bool {
    lines
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| l.trim_start().starts_with("Paste code here"))
}

/// Footer hints a picker shows under its options.
const DIALOG_FOOTERS: &[&str] = &["Enter to confirm", "Enter to select", "Esc to cancel"];
/// Bottom lines kept for an unreadable dialog.
const UNKNOWN_LINES: usize = 12;

/// The dialog waiting on `screen`, if any. `awaiting` is the classifier's
/// verdict (a known permission or trust marker); a picker footer counts too, so
/// the theme and login pickers show without being in that list.
pub fn dialog_prompt(screen: &str, awaiting: bool) -> Option<DialogPrompt> {
    let lines: Vec<&str> = screen.lines().collect();
    if idle_composer(&lines) {
        return None;
    }
    if login_prompt(&lines) {
        return Some(DialogPrompt {
            kind: DialogKind::LoginCode,
            question: "Claude is waiting for the sign-in code.".into(),
            options: Vec::new(),
            selected: 0,
            screen: String::new(),
        });
    }
    let tail_start = lines.len().saturating_sub(TAIL_WINDOW + STATUS_WINDOW);
    let footer = lines[tail_start..]
        .iter()
        .any(|l| DIALOG_FOOTERS.iter().any(|m| l.contains(m)));
    if !awaiting && !footer {
        return None;
    }
    if let Some(n) = numbered_options(screen) {
        return Some(DialogPrompt {
            kind: DialogKind::Numbered,
            question: n.question,
            options: n.options.into_iter().map(|(_, l)| l).collect(),
            selected: 0,
            screen: String::new(),
        });
    }
    if let Some((question, options, selected)) = arrow_options(&lines, tail_start) {
        return Some(DialogPrompt {
            kind: DialogKind::Arrows,
            question,
            options,
            selected,
            screen: String::new(),
        });
    }
    let mut tail: Vec<&str> = lines
        .iter()
        .rev()
        .skip_while(|l| l.trim().is_empty())
        .take(UNKNOWN_LINES)
        .copied()
        .collect();
    tail.reverse();
    Some(DialogPrompt {
        kind: DialogKind::Unknown,
        question: String::new(),
        options: Vec::new(),
        selected: 0,
        screen: tail.join("\n"),
    })
}

/// A list whose selection is a marker (`❯`) on one line and whose other
/// lines are indented labels with no digit: (question, labels, selected).
fn arrow_options(lines: &[&str], from: usize) -> Option<(String, Vec<String>, usize)> {
    let marked = |l: &str| {
        let t = l.trim_start();
        t.strip_prefix('❯')
            .or_else(|| t.strip_prefix('›'))
            .map(|r| r.trim().to_string())
    };
    let at = (from..lines.len())
        .rev()
        .find(|&i| marked(lines[i]).is_some_and(|l| !l.is_empty()))?;
    let indented = |l: &str| l.starts_with(' ') && !l.trim().is_empty();
    let mut first = at;
    while first > from && indented(lines[first - 1]) && marked(lines[first - 1]).is_none() {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < lines.len() && indented(lines[last + 1]) {
        last += 1;
    }
    // A dialog is the last thing on the screen: only blank lines and its own
    // footer hint may follow the list. Text after it means it was answered.
    let footer = |l: &str| DIALOG_FOOTERS.iter().any(|m| l.contains(m));
    if lines[last + 1..]
        .iter()
        .any(|l| !l.trim().is_empty() && !footer(l))
    {
        return None;
    }
    let label = |l: &str| marked(l).unwrap_or_else(|| l.trim().to_string());
    let options: Vec<String> = lines[first..=last].iter().map(|l| label(l)).collect();
    if options.len() < 2 {
        return None;
    }
    // The paragraph just above the list (a long question wraps over lines).
    let above: Vec<&str> = lines[from..first].iter().map(|l| l.trim()).collect();
    let end = above
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    let begin = above[..end]
        .iter()
        .rposition(|l| l.is_empty())
        .map_or(0, |i| i + 1);
    let question = above[begin..end].join(" ");
    Some((question, options, at - first))
}

/// Keys that move a selection from `selected` to `target` and choose it:
/// Down or Up arrows, then Enter.
pub fn arrow_keys(selected: usize, target: usize) -> Vec<u8> {
    let mut keys = Vec::new();
    for _ in 0..target.abs_diff(selected) {
        keys.extend_from_slice(if target > selected {
            b"\x1b[B"
        } else {
            b"\x1b[A"
        });
    }
    keys.push(b'\r');
    keys
}

/// Is `b` still the dialog `a` was? Same words, so a tap meant for one can
/// never answer another that has taken its place.
pub fn same_dialog(a: &DialogPrompt, b: &DialogPrompt) -> bool {
    a.kind == b.kind && a.question == b.question && a.options == b.options
}

/// Lines above the input box that count as "live status" territory.
const STATUS_WINDOW: usize = 8;
/// Claude prints tips (sometimes many lines) *under* its spinner, so the
/// spinner itself is searched further up than the plain-text markers. Only
/// live spinners match `is_spinner_line`, so the wider reach is safe
/// (a 12-line bottom scrape called two busy panes idle on 2026-09-27).
const SPINNER_WINDOW: usize = 24;
/// With no input box on screen, how many bottom lines count.
const TAIL_WINDOW: usize = 14;

const AWAITING: &[&str] = &[
    "Do you want to proceed",
    "Do you want to make this edit",
    "Do you want to create",
    "Do you want to allow",
    "Would you like to run the following command",
    "Allow command?",
    "Yes, and don't ask again",
    "trust this folder",
    "Do you trust",
    "Yes, I trust",
    "one you trust?",
    "Enter to select",
    "Skip until next version",
    "Paste code here",
];

const BUSY: &[&str] = &[
    "Working (",
    "Waiting for background terminal",
    "esc to interrupt",
    "ctrl+c to interrupt",
];

/// Lowercased. "N% of your weekly limit left" is a warning, not a wall, so
/// nothing here matches "limit left".
const LIMITED: &[&str] = &[
    "hit your usage limit",
    "hit your limit",
    "usage limit reached",
    "limit reached",
    "out of credits",
];

/// Placeholder text Codex paints (dim) in an empty composer. Rendered text
/// carries no colour, so without this list an empty prompt reads as typed.
const PLACEHOLDERS: &[&str] = &[
    "Ask Codex to do anything",
    "Implement {feature}",
    "Find and fix a bug in @filename",
    "Explain this codebase",
    "Summarize recent commits",
    "Write tests for @filename",
    "Improve documentation in @filename",
    "Run /review on my current changes",
    "Use /skills to list available skills",
    "Try \"",
    "Press up to edit queued messages",
];

/// Footer hints an agent shows under its composer only mid-turn (grok's
/// `Shift+Tab:mode  │  Ctrl+c:cancel  │  Ctrl+x:shortcuts`).
const BUSY_FOOTER: &[&str] = &["Ctrl+c:cancel"];
const FOOTER_WINDOW: usize = 3;

/// Claude's marks for input queued behind a running turn.
const QUEUED: &[&str] = &[
    "Press up to edit queued messages",
    "ctrl+x ctrl+s to send now",
];

/// Classify a pane from its rendered screen (as `ctl read` returns it) and
/// its OSC title.
pub fn classify(screen: &str, title: Option<&str>) -> AgentState {
    let lines: Vec<&str> = screen.lines().collect();
    let quota = parse_quota(&lines);
    let quota = (!quota.is_empty()).then_some(quota);
    let context_left_pct = parse_context_left(&lines);
    let bottom = &lines[lines.len().saturating_sub(TAIL_WINDOW + STATUS_WINDOW)..];
    let queued_input = bottom.iter().any(|l| QUEUED.iter().any(|m| l.contains(m)));
    let verdict = |activity: Activity, evidence: Option<&str>| AgentState {
        activity,
        evidence: evidence.map(|l| l.trim().to_string()),
        quota: quota.clone(),
        context_left_pct,
        queued_input,
    };

    let tail_start = lines.len().saturating_sub(TAIL_WINDOW + STATUS_WINDOW);
    // The normal input box first: with it showing, no dialog is, so a reply
    // quoting one above it cannot read as one.
    if !idle_composer(&lines) {
        if let Some(l) = lines[tail_start..]
            .iter()
            .find(|l| AWAITING.iter().any(|m| l.contains(m)))
        {
            return verdict(Activity::AwaitingInput, Some(l));
        }
    }

    let prompt = prompt_line(&lines);
    let window = match prompt {
        Some(i) => &lines[i.saturating_sub(STATUS_WINDOW)..i],
        None => &lines[lines.len().saturating_sub(TAIL_WINDOW)..],
    };
    let spinner_window = match prompt {
        Some(i) => &lines[i.saturating_sub(SPINNER_WINDOW)..i],
        None => &lines[lines.len().saturating_sub(SPINNER_WINDOW)..],
    };
    if let Some(l) = spinner_window.iter().rev().find(|l| is_spinner_line(l)) {
        return verdict(Activity::Busy, Some(l));
    }
    if let Some(l) = window.iter().find(|l| BUSY.iter().any(|m| l.contains(m))) {
        return verdict(Activity::Busy, Some(l));
    }
    if let Some(i) = prompt {
        let footer = &lines[(i + 1).min(lines.len())..(i + 1 + FOOTER_WINDOW).min(lines.len())];
        if let Some(l) = footer
            .iter()
            .find(|l| BUSY_FOOTER.iter().any(|m| l.contains(m)))
        {
            return verdict(Activity::Busy, Some(l));
        }
    }
    if let Some(t) = title.filter(|t| crate::util::title_looks_busy(t)) {
        return verdict(Activity::Busy, Some(t));
    }
    if let Some(l) = window.iter().find(|l| {
        let low = l.to_lowercase();
        LIMITED.iter().any(|m| low.contains(m))
    }) {
        return verdict(Activity::Limited, Some(l));
    }
    match prompt {
        Some(i) => verdict(Activity::Idle, Some(lines[i])),
        None => verdict(Activity::Unknown, None),
    }
}

/// A horizontal rule: Claude draws one above and one below its composer, the
/// top one sometimes carrying a title ("──── Seance · lead ─").
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('─') && t.chars().filter(|c| *c == '─').count() >= 8
}

/// Claude's normal input box is showing: a rule, the `❯` line, a rule, then
/// only status lines. A real dialog never looks like this: its `❯` marks the
/// selected option (a digit and a label, or a label with more options and a
/// footer hint around it), not an empty composer between two rules. When this
/// is true the pane is not waiting on a dialog, whatever the text above it
/// quotes (a finished reply that mentions "Do you trust the files in this
/// folder?" is still just a reply).
fn idle_composer(lines: &[&str]) -> bool {
    let Some(p) = prompt_line(lines) else {
        return false;
    };
    if p == 0 || p + 1 >= lines.len() || !is_rule(lines[p - 1]) || !is_rule(lines[p + 1]) {
        return false;
    }
    let body = unboxed(lines[p]).trim_start_matches(['❯', '›']).trim();
    let numbered = body
        .split_once(". ")
        .is_some_and(|(d, _)| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
    let status = &lines[p + 2..];
    !numbered
        && status.len() <= STATUS_WINDOW
        && !status
            .iter()
            .any(|l| DIALOG_FOOTERS.iter().any(|m| l.contains(m)))
}

/// A line with grok's input-box border (`│ ❯ … │`) peeled off.
fn unboxed(line: &str) -> &str {
    let t = line.trim();
    let t = t.strip_prefix('│').unwrap_or(t);
    t.strip_suffix('│').unwrap_or(t).trim()
}

/// The input-box line: the last line opening with Claude's / grok's `❯` or
/// Codex's `›` glyph. Last matters: grok echoes past prompts in its
/// transcript with the same `❯`, above the composer.
fn prompt_line(lines: &[&str]) -> Option<usize> {
    lines.iter().rposition(|l| {
        let t = unboxed(l);
        t.starts_with('❯') || t.starts_with('›')
    })
}

/// What sits in the composer after the prompt glyph, placeholder excluded.
/// `None` when there is no input box on screen.
pub fn composer_text(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let i = prompt_line(&lines)?;
    let t = unboxed(lines[i]);
    let body = t
        .strip_prefix('❯')
        .or_else(|| t.strip_prefix('›'))
        .unwrap_or(t)
        .trim();
    if PLACEHOLDERS.iter().any(|p| body.starts_with(p)) {
        return Some(String::new());
    }
    Some(body.to_string())
}

/// Did a paste of `text` land in the composer without being submitted? True
/// when the composer holds the start of `text` (TUIs truncate long lines and
/// collapse big pastes to "[Pasted text #1 …]", both accepted).
pub fn composer_holds(screen: &str, text: &str) -> bool {
    let Some(body) = composer_text(screen) else {
        return false;
    };
    if body.is_empty() {
        return false;
    }
    if body.starts_with("[Pasted text") || body.starts_with("[Pasted Content") {
        return true;
    }
    let first = text.lines().map(str::trim).find(|l| !l.is_empty());
    let Some(first) = first else {
        return false;
    };
    // Compare a short prefix: the composer may wrap, truncate with `…`, or
    // show only the head of a multi-line paste.
    let body = body.trim_end_matches('…');
    let n = body.chars().count().min(first.chars().count()).min(24);
    n >= 4 && body.chars().take(n).eq(first.chars().take(n))
}

/// Claude's live spinner line: `✶ Osmosing… (30m 52s · ↓ 48.7k tokens)`, or
/// bare `✽ Gitifying…` for the first beat of a turn. Its idle twin
/// (`✻ Baked for 7m 48s`) has no `…`. A running tool line ending in
/// `… (5m 55s · 3 lines)` also counts: a tool is running.
fn is_spinner_line(line: &str) -> bool {
    let timer = line.match_indices("… (").any(|(i, m)| {
        line[i + m.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    });
    if timer {
        return true;
    }
    let t = line.trim();
    let mut chars = t.chars();
    let glyph = chars
        .next()
        .is_some_and(|g| !g.is_ascii() && !g.is_alphanumeric());
    let rest = chars.as_str().trim_start();
    glyph
        && rest
            .strip_suffix('…')
            .is_some_and(|verb| !verb.is_empty() && verb.chars().all(char::is_alphabetic))
}

/// Lowercased word tokens of the whole screen; `%` stays attached.
fn tokens(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|l| l.split(|c: char| c.is_whitespace() || c == ':' || c == '·' || c == '•'))
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric() && c != '%'))
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn parse_context_left(lines: &[&str]) -> Option<u32> {
    let toks = tokens(lines);
    let mut found = None;
    for (i, tok) in toks.iter().enumerate() {
        let Some(pct) = tok.strip_suffix('%').and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let next = |k: usize| toks.get(i + k).map(String::as_str).unwrap_or("");
        let prev = |k: usize| {
            i.checked_sub(k)
                .and_then(|j| toks.get(j))
                .map(String::as_str)
                .unwrap_or("")
        };
        let codex = next(1) == "context" && next(2) == "left";
        let claude_after = next(1) == "until" && next(2) == "auto-compact";
        let claude_before = prev(1) == "auto-compact" && prev(2) == "until";
        if codex || claude_after || claude_before {
            found = Some(pct);
        }
    }
    found
}

fn parse_quota(lines: &[&str]) -> Quota {
    let mut q = Quota::default();
    // One token stream for the whole screen: warnings wrap ("less than 5% of
    // your weekly" / "limit left."), so a per-line scan would miss them.
    let tokens = tokens(lines);
    for (i, tok) in tokens.iter().enumerate() {
        let Some(pct) = tok.strip_suffix('%').and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let prev = i.checked_sub(1).map(|j| tokens[j].as_str()).unwrap_or("");
        let after = &tokens[i + 1..tokens.len().min(i + 6)];
        let has = |w: &str| after.iter().any(|t| t == w);
        let left = after.first().is_some_and(|t| t == "left") || (has("weekly") && has("left"));
        match (prev, left) {
            ("5h", false) => q.five_hour_used_pct = Some(pct),
            ("7d", false) => q.seven_day_used_pct = Some(pct),
            ("5h", true) => q.five_hour_left_pct = Some(pct),
            ("weekly", true) => q.weekly_left_pct = Some(pct),
            (_, true) if has("weekly") => q.weekly_left_pct = Some(pct),
            _ => {}
        }
    }
    // Claude statuslines lead with the account: `[account-2] 5h:19% 7d:95%`.
    q.account = lines
        .iter()
        .filter(|l| l.contains("5h:") || l.contains("7d:"))
        .find_map(|l| {
            let a = l.find('[')?;
            let b = a + l[a..].find(']')?;
            Some(l[a + 1..b].to_string())
        });
    q
}

/// One look at a pane for delivery confirmation: screen + verdict.
#[derive(Clone, Debug)]
pub struct Look {
    pub activity: Activity,
    pub evidence: Option<String>,
    pub queued_input: bool,
    pub screen: String,
}

impl Look {
    pub fn of(screen: String, title: Option<&str>) -> Self {
        let st = classify(&screen, title);
        Self {
            activity: st.activity,
            evidence: st.evidence,
            queued_input: st.queued_input,
            screen,
        }
    }
}

/// What one look after writing `text` into a pane says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judgement {
    Delivered(&'static str),
    /// The text sits in the composer, unsubmitted — press Enter.
    Stuck,
    Pending,
}

/// Did the write of `text` land, comparing the look `before` it with `now`?
/// Shared by `ctl send` / `note-agent` and the daemon's send queue.
pub fn judge(before: &Look, now: &Look, text: &str) -> Judgement {
    if now.activity == Activity::AwaitingInput && before.activity != Activity::AwaitingInput {
        return Judgement::Delivered("modal");
    }
    if composer_holds(&now.screen, text) {
        return Judgement::Stuck;
    }
    if now.queued_input && !before.queued_input {
        return Judgement::Delivered("queued");
    }
    if now.activity == Activity::Busy && before.activity != Activity::Busy {
        return Judgement::Delivered("busy");
    }
    if echoes(&now.screen, text) > echoes(&before.screen, text) {
        // Echoed into Claude's queue (behind an already-queued note).
        return Judgement::Delivered(if now.queued_input { "queued" } else { "echo" });
    }
    if before.activity == Activity::Unknown && now.screen != before.screen {
        return Judgement::Delivered("changed");
    }
    Judgement::Pending
}

/// How many times the head of `text` appears on `screen`, whitespace
/// ignored — transcripts re-wrap and indent a message at pane width.
fn echoes(screen: &str, text: &str) -> usize {
    let needle: String = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(20)
        .collect();
    if needle.chars().count() < 6 {
        return 0;
    }
    let hay: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
    hay.matches(needle.as_str()).count()
}

#[cfg(test)]
mod tests {
    // Captured screens of Claude's own first-launch and permission dialogs.
    const TRUST_NUMBERED: &str = "\n Do you trust the files in this folder?\n\n /Users/me\n\n Claude Code may read files in this folder. Reading untrusted files may lead\n Claude Code to behave in unexpected ways.\n\n ❯ 1. Yes, proceed\n   2. No, exit\n\n Enter to confirm · Esc to cancel\n";
    const TRUST_ARROWS: &str = "\n Is this a project you created or one you trust? (Like your own code, a well-\n known open source project, or work from your team).\n\n ❯ No, exit\n   Yes, I trust this folder\n";
    const THEME: &str = "\n Welcome to Claude Code\n\n Choose the text style that looks best with your terminal\n To change this later, run /theme\n\n ❯ 1. Dark mode ✔\n   2. Light mode\n   3. Dark mode (colorblind-friendly)\n\n Enter to select · Esc to cancel\n";
    const UNKNOWN: &str = "\n Some tool wants something\n Do you want to proceed?\n Press y to confirm, n to cancel\n";

    #[test]
    fn the_trust_dialog_reads_as_numbered_options() {
        let d = dialog_prompt(TRUST_NUMBERED, true).unwrap();
        assert_eq!(d.kind, DialogKind::Numbered);
        assert_eq!(
            d.question,
            "/Users/me".to_string().replace("/Users/me", &d.question)
        );
        assert_eq!(d.options, ["Yes, proceed", "No, exit"]);
    }

    #[test]
    fn the_trust_dialog_without_digits_is_an_arrow_list_with_its_selection() {
        let d = dialog_prompt(TRUST_ARROWS, true).unwrap();
        assert_eq!(d.kind, DialogKind::Arrows);
        assert_eq!(d.options, ["No, exit", "Yes, I trust this folder"]);
        assert_eq!(d.selected, 0);
        assert!(d.question.contains("trust"), "{}", d.question);
        // Choosing the second: one Down, then Enter. The first: just Enter.
        assert_eq!(arrow_keys(d.selected, 1), b"\x1b[B\r");
        assert_eq!(arrow_keys(0, 0), b"\r");
        assert_eq!(arrow_keys(2, 0), b"\x1b[A\x1b[A\r");
    }

    #[test]
    fn the_theme_picker_shows_without_a_known_marker_through_its_footer() {
        // Not in the awaiting list: the picker footer is what gives it away.
        let d = dialog_prompt(THEME, false).unwrap();
        assert_eq!(d.kind, DialogKind::Numbered);
        assert_eq!(d.options.len(), 3);
        assert!(d.question.contains("To change this later") || d.question.contains("text style"));
    }

    #[test]
    fn an_answered_dialog_left_in_the_scrollback_is_not_a_list_to_tap_again() {
        let answered = format!("{TRUST_ARROWS}\n got keys: ^[[B\n");
        let d = dialog_prompt(&answered, false);
        assert!(d.is_none_or(|d| d.kind != DialogKind::Arrows));
    }

    #[test]
    fn a_dialog_that_is_not_a_list_is_unknown_and_carries_the_screen() {
        let d = dialog_prompt(UNKNOWN, true).unwrap();
        assert_eq!(d.kind, DialogKind::Unknown);
        assert!(d.options.is_empty());
        assert!(d.screen.contains("Press y to confirm"), "{}", d.screen);
    }

    #[test]
    fn an_ordinary_screen_has_no_dialog_and_the_same_dialog_is_recognized() {
        assert_eq!(
            dialog_prompt("\n ✻ Working…\n\n ╭─────╮\n │ >   │\n ╰─────╯\n", false),
            None
        );
        let a = dialog_prompt(TRUST_NUMBERED, true).unwrap();
        let b = dialog_prompt(TRUST_NUMBERED, true).unwrap();
        assert!(same_dialog(&a, &b));
        assert!(!same_dialog(&a, &dialog_prompt(THEME, false).unwrap()));
    }

    use super::*;

    // Frames sampled live 2026-09-27 (narrow panes, so lines are truncated).
    const CLAUDE_BUSY: &str = "     done; sleep 5; … (5m 55s · 3 lines)
     (ctrl+b to run in background)

✶ Osmosing… (30m 52s · ↓ 48.7k tokens)
  ⎿  Tip: Use /clear to start fresh when
     switching topics and free up context



──────────────────────────────────────────
❯
──────────────────────────────────────────
  [account-1] 5h:10% 7d:3%
  ⏵⏵ bypass permissions on · 1 shell · …";

    const CLAUDE_IDLE_UNSENT: &str = "  The report is at
  codex-results/rails-b__fix2.md, and
  the seance ctl finish call
  reported done (task-187).

✻ Baked for 7m 48s · done 7:43 PM

─────────────────────────────────────
❯ run the combined a/b/c/d integrati…
─────────────────────────────────────
  [account-2] 5h:19% 7d:95%
  ⏵⏵ bypass permissions on  · ← fo…";

    const CODEX_IDLE_LOW: &str = "Coordinator note, PRIORITY (…
⚠ Heads up, you have less
  than 5% of your weekly
  limit left. Run /status for
  a breakdown.


› Ask Codex to do anything

  GPT-6-Astra xhigh · ~/work…
  ← for agents · ?  ⚠ 3 · f2";

    const CODEX_BUSY: &str = "• Ran cargo test
  └ test result: ok. 12 passed

• Working (4m 12s • esc to interrupt)


› Ask Codex to do anything

  gpt-5 high · 95% context left · weekly 12% left";

    const CODEX_BG: &str = "• Waiting for background terminal (1m 3s • esc…

› Implement {feature}
";

    const CODEX_LIMITED: &str = "• Explored
  └ Read lib.rs

■ You've hit your usage limit. Upgrade to Pro or try again in 2 days.

› Ask Codex to do anything

  gpt-5 high · weekly 0% left";

    const CLAUDE_LIMITED: &str = "⎿  You've hit your limit · resets 5pm (America/Los_Angeles)

──────────────
❯
──────────────
  [account-2] 5h:100% 7d:95%";

    const CLAUDE_PERMISSION: &str = " Bash command

   rm -rf target

 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don't ask again for rm commands
   3. No, and tell Claude what to do differently (esc)";

    #[test]
    fn claude_spinner_is_busy_and_quota_read() {
        let s = classify(CLAUDE_BUSY, Some("◑ Paceline realtime v2"));
        assert_eq!(s.activity, Activity::Busy);
        assert!(s.evidence.unwrap().contains("Osmosing"));
        let q = s.quota.unwrap();
        assert_eq!(q.account.as_deref(), Some("account-1"));
        assert_eq!(q.five_hour_used_pct, Some(10));
        assert_eq!(q.seven_day_used_pct, Some(3));
    }

    #[test]
    fn claude_done_line_is_idle_and_unsent_text_detected() {
        let s = classify(CLAUDE_IDLE_UNSENT, Some("✳ Lane rails-b"));
        assert_eq!(s.activity, Activity::Idle);
        assert_eq!(s.quota.unwrap().seven_day_used_pct, Some(95));
        assert!(composer_holds(
            CLAUDE_IDLE_UNSENT,
            "run the combined a/b/c/d integration suite\nthen report"
        ));
        assert!(!composer_holds(CLAUDE_IDLE_UNSENT, "something else"));
        assert!(!composer_holds(CLAUDE_BUSY, "anything"));
    }

    #[test]
    fn codex_idle_warning_is_not_a_limit() {
        let s = classify(CODEX_IDLE_LOW, Some("Complete seance task | v2"));
        assert_eq!(s.activity, Activity::Idle);
        assert_eq!(s.quota.unwrap().weekly_left_pct, Some(5));
        assert_eq!(composer_text(CODEX_IDLE_LOW).as_deref(), Some(""));
        assert!(!composer_holds(CODEX_IDLE_LOW, "Ask Codex to do anything"));
    }

    #[test]
    fn codex_working_and_background_wait_are_busy() {
        let s = classify(CODEX_BUSY, None);
        assert_eq!(s.activity, Activity::Busy);
        assert_eq!(s.quota.unwrap().weekly_left_pct, Some(12));
        assert_eq!(classify(CODEX_BG, None).activity, Activity::Busy);
    }

    #[test]
    fn usage_walls_read_limited() {
        let s = classify(CODEX_LIMITED, None);
        assert_eq!(s.activity, Activity::Limited);
        assert_eq!(s.quota.unwrap().weekly_left_pct, Some(0));
        assert_eq!(classify(CLAUDE_LIMITED, None).activity, Activity::Limited);
    }

    #[test]
    fn claude_trust_dialog_is_awaiting_input() {
        // Claude 2.1.280, live: default cursor is "No, exit".
        let s = " Accessing workspace:\n /var/tmp/x\n Quick safety check: Is this a project you \
                 created or one you trust? (Like your own code)\n ❯ No, exit\n   Yes, I trust this \
                 folder\n Enter to confirm · Esc to cancel";
        assert_eq!(classify(s, None).activity, Activity::AwaitingInput);
    }

    // v2-simp-rails, 2026-09-28: a note-agent message queued mid-turn.
    const CLAUDE_QUEUED: &str = "❯ COORDINATOR NOTE: spec gate review 3 lists Rails items;
  'Rails lane'.
  ctrl+x ctrl+s to send now
─────────────────────────────────────────
❯ Press up to edit queued messages
─────────────────────────────────────────
  [account-2] 5h:52% 7d:12%
  ⏵⏵ bypass permissions on · 1 shell · ← for agents";

    #[test]
    fn queued_input_detected_and_not_mistaken_for_typed_text() {
        let s = classify(CLAUDE_QUEUED, Some("◐ Paceline"));
        assert_eq!(s.activity, Activity::Busy);
        assert!(s.queued_input);
        assert_eq!(composer_text(CLAUDE_QUEUED).as_deref(), Some(""));
        assert!(!classify(CLAUDE_BUSY, None).queued_input);
    }

    #[test]
    fn context_left_in_every_known_phrasing() {
        let at = |s: &str| classify(s, None).context_left_pct;
        assert_eq!(
            at("  ⏵⏵ bypass permissions on · Context left until auto-compact: 9%"),
            Some(9)
        );
        assert_eq!(at("  12% until auto-compact · /compact"), Some(12));
        assert_eq!(
            at("› Ask Codex\n  gpt-5 high · 95% context left · weekly 12% left"),
            Some(95)
        );
        assert_eq!(at(CLAUDE_BUSY), None);
        // Codex's weekly figure is not context.
        assert_eq!(
            classify(CODEX_BUSY, None).quota.unwrap().weekly_left_pct,
            Some(12)
        );
    }

    #[test]
    fn judge_reads_queue_busy_echo_and_stuck() {
        let look = |s: &str| Look::of(s.to_string(), None);
        let idle = look("✻ Baked for 7m\n────\n❯\n────");
        let busy = look("✶ Osmosing… (3s)\n────\n❯\n────");
        assert_eq!(judge(&idle, &busy, "do it"), Judgement::Delivered("busy"));
        assert_eq!(judge(&idle, &idle, "do the thing"), Judgement::Pending);
        let stuck = look("✻ Baked\n────\n❯ do the thing now please\n────");
        assert_eq!(
            judge(&idle, &stuck, "do the thing now please"),
            Judgement::Stuck
        );
        let queued = Look::of(CLAUDE_QUEUED.to_string(), Some("◐ x"));
        assert_eq!(
            judge(&busy, &queued, "COORDINATOR NOTE"),
            Judgement::Delivered("queued")
        );
        assert_eq!(judge(&busy, &busy, "a note"), Judgement::Pending);
        // A second note behind an already-queued one still reads "queued".
        let two = Look::of(
            CLAUDE_QUEUED.replace("❯ COORDINATOR", "❯ second note here\n❯ COORDINATOR"),
            Some("◐ x"),
        );
        assert_eq!(
            judge(&queued, &two, "second note here"),
            Judgement::Delivered("queued")
        );
    }

    #[test]
    fn permission_modal_is_awaiting_input() {
        let s = classify(CLAUDE_PERMISSION, None);
        assert_eq!(s.activity, Activity::AwaitingInput);
    }

    #[test]
    fn old_transcript_mentions_do_not_flip_verdict() {
        // A limit / Working quote far above the input box is history.
        let mut screen =
            String::from("• Working (1m • esc to interrupt)\nYou've hit your usage limit\n");
        for _ in 0..20 {
            screen.push_str("transcript line\n");
        }
        screen.push_str("› Ask Codex to do anything\n");
        assert_eq!(classify(&screen, None).activity, Activity::Idle);
    }

    #[test]
    fn spinner_far_above_prompt_behind_tips_is_busy() {
        // The 8:19pm misread: tips pushed the spinner >12 lines up.
        let mut screen = String::from("· Whisking… (45m 39s)\n");
        for i in 0..14 {
            screen.push_str(&format!("  ⎿  Tip line {i}\n"));
        }
        screen.push_str("────\n❯\n────\n  [account-1] 5h:10% 7d:3%\n  ⏵⏵ bypass permissions on");
        assert_eq!(classify(&screen, Some("✳ x")).activity, Activity::Busy);
    }

    #[test]
    fn bare_spinner_verb_is_busy_but_tool_bullets_are_not() {
        let frame = "✽ Gitifying…\n  ⎿  Tip: Use /focus\n────\n❯\n────";
        assert_eq!(classify(frame, Some("✳ x")).activity, Activity::Busy);
        let idle = "● Sleeping for 25 seconds\n✻ Churned for 4s · done 8:19 PM\n────\n❯\n────";
        assert_eq!(classify(idle, Some("✳ x")).activity, Activity::Idle);
    }

    #[test]
    fn busy_title_alone_is_busy_and_plain_shell_unknown() {
        assert_eq!(
            classify("some output", Some("⠋ claude")).activity,
            Activity::Busy
        );
        assert_eq!(
            classify("zack@host:~$ ls\nfoo bar", Some("bash")).activity,
            Activity::Unknown
        );
    }

    // Live grok 4.7 frames, 2026-10-07 (opt-in banner above the composer).
    const GROK_TOP: &str = "  master ~/work/vita                       1.8K / 256K │ [Dashboard]
     ❯ reply with exactly the word banana and nothing else          12:44 PM
";
    const GROK_BOX_TOP: &str = "  Help improve Grok                       [Opt out] [Opt in]
  Read Terms and Privacy Policy.
  ╭──────────────────────────────────────────────╮
";
    const GROK_BOX_BOTTOM: &str = "  ╰──────────────── Grok 4.7 (xhigh) · always-approve ─╯\n";

    fn grok(transcript: &str, composer: &str, footer: &str) -> String {
        format!(
            "{GROK_TOP}{transcript}{GROK_BOX_TOP}  │ ❯ {composer:<44}│\n{GROK_BOX_BOTTOM}  {footer}"
        )
    }

    #[test]
    fn grok_idle_ignores_the_echoed_prompt_above_the_composer() {
        let idle = grok(
            "     ◆ Thought for 3.2s\n     banana   12:44 PM\n     Worked for 6.4s\n",
            "",
            "Shift+Tab:mode  │  Ctrl+x:shortcuts",
        );
        assert_eq!(classify(&idle, None).activity, Activity::Idle);
        assert_eq!(composer_text(&idle).as_deref(), Some(""));
        // The 7:21am double send: the echo was read as an unsent composer.
        assert!(!composer_holds(
            &idle,
            "reply with exactly the word banana and nothing else"
        ));
    }

    #[test]
    fn grok_running_turn_is_busy() {
        let busy = grok(
            "   ⠹ Waiting for response… 2.1s           2.1s ⇣1.82k [stop]\n",
            "",
            "Shift+Tab:mode  │  Ctrl+c:cancel  │  Ctrl+x:shortcuts",
        );
        assert_eq!(classify(&busy, None).activity, Activity::Busy);
    }

    #[test]
    fn grok_unsent_paste_is_stuck_then_delivered_once_running() {
        let text = "reply with exactly the word banana";
        let before = Look::of(grok("", "", "Shift+Tab:mode  │  Ctrl+x:shortcuts"), None);
        let unsent = Look::of(
            grok(
                "",
                text,
                "Enter:send  │  Alt+Enter:newline  │  Ctrl+x:shortcuts",
            ),
            None,
        );
        assert_eq!(judge(&before, &unsent, text), Judgement::Stuck);
        let running = Look::of(
            grok(
                "",
                "",
                "Shift+Tab:mode  │  Ctrl+c:cancel  │  Ctrl+x:shortcuts",
            ),
            None,
        );
        assert_eq!(judge(&before, &running, text), Judgement::Delivered("busy"));
    }

    #[test]
    fn numbered_options_read_a_permission_menu() {
        let s = " Bash command\n   ls -la\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for ls commands\n   3. No, and tell Claude what to do differently (esc)\n";
        let p = numbered_options(s).unwrap();
        assert_eq!(p.question, "Do you want to proceed?");
        assert_eq!(p.options.len(), 3);
        assert_eq!(p.options[0], (1, "Yes".to_string()));
        assert_eq!(p.options[2].0, 3);
    }

    #[test]
    fn numbered_options_skip_menus_without_digits_and_stray_lists() {
        let trust = " Is this a project you trust?\n ❯ No, exit\n   Yes, I trust this folder\n";
        assert_eq!(numbered_options(trust), None);
        // One numbered line is a list item, not a menu.
        assert_eq!(numbered_options("steps:\n 1. do it\n❯ "), None);
        // Counting that skips a number is not a menu.
        assert_eq!(numbered_options("q?\n ❯ 1. a\n   3. b\n"), None);
    }

    // A finished reply that quotes the trust dialog, above Claude's normal box.
    const QUOTE_TOP: &str = "\n⏺ The trust dialog asks:\n\n    Do you trust the files in this folder?\n    ❯ 1. Yes, I trust this folder\n      2. No, exit\n\n  That is the one Seance answers for you.\n\n";
    const RULE: &str = "──────────────────────────────────────────── Seance · Seance lead · XPS ─";
    const RULE2: &str = "─────────────────────────────────────────────────────────────────────────";
    const STATUS: &str = "     /home/me/Claude   main  Opus 5.5  ctx:28%  14:10\n  ⏵⏵ auto mode on (shift+tab to cycle)\n";

    fn quote_screen(activity_line: &str) -> String {
        format!("{QUOTE_TOP}{activity_line}\n\n{RULE}\n❯\n{RULE2}\n{STATUS}")
    }

    #[test]
    fn a_reply_quoting_the_trust_dialog_above_the_normal_box_is_idle() {
        let st = classify(&quote_screen("✻ Cooked for 16s"), None);
        assert_eq!(st.activity, Activity::Idle, "{st:?}");
        // And Chat's dialog card does not show it either.
        assert_eq!(
            dialog_prompt(&quote_screen("✻ Cooked for 16s"), false),
            None
        );
        assert_eq!(dialog_prompt(&quote_screen("✻ Cooked for 16s"), true), None);
    }

    #[test]
    fn the_same_quote_mid_turn_is_busy() {
        let st = classify(&quote_screen("✻ Spinning… (45s · ↓ 3.0k tokens)"), None);
        assert_eq!(st.activity, Activity::Busy, "{st:?}");
    }

    #[test]
    fn the_real_dialogs_still_need_you() {
        for (name, screen) in [
            ("trust numbered", TRUST_NUMBERED),
            ("trust arrows", TRUST_ARROWS),
            ("theme", THEME),
            ("permission", CLAUDE_PERMISSION),
        ] {
            assert_eq!(
                classify(screen, None).activity,
                Activity::AwaitingInput,
                "{name}"
            );
        }
    }

    #[test]
    fn a_boxed_permission_dialog_whose_last_option_is_selected_still_needs_you() {
        let screen = format!(
            "{RULE2}\n Bash command\n\n   rm -rf build\n\n Do you want to proceed?\n   1. Yes\n   2. Yes, and don't ask again\n ❯ 3. No, and tell Claude what to do differently (esc)\n{RULE2}\n"
        );
        assert_eq!(classify(&screen, None).activity, Activity::AwaitingInput);
        assert!(dialog_prompt(&screen, true).is_some());
    }

    #[test]
    fn a_dialog_is_never_mistaken_for_the_box_by_its_footer() {
        let screen = format!("{RULE}\n❯\n{RULE2}\n Enter to select · Esc to cancel\n Do you trust the files in this folder?\n");
        assert_eq!(classify(&screen, None).activity, Activity::AwaitingInput);
    }

    // Claude's own first-run login step, captured from a real `claude 2.1.318`
    // started with a throwaway config (the OAuth challenge and state redacted).
    const LOGIN_PASTE: &str = include_str!("fixtures/login-paste-screen.txt");

    #[test]
    fn the_login_paste_step_is_a_login_code_dialog() {
        let d = dialog_prompt(LOGIN_PASTE, false).expect("a card for the login step");
        assert_eq!(d.kind, DialogKind::LoginCode);
        assert!(d.options.is_empty());
        assert_eq!(
            classify(LOGIN_PASTE, None).activity,
            Activity::AwaitingInput
        );
        // The classifier's own verdict makes no difference.
        assert_eq!(
            dialog_prompt(LOGIN_PASTE, true).unwrap().kind,
            DialogKind::LoginCode
        );
    }

    #[test]
    fn no_other_screen_is_a_login_code_dialog() {
        for (name, screen) in [
            ("trust", TRUST_NUMBERED),
            ("arrows", TRUST_ARROWS),
            ("theme", THEME),
            ("unknown", UNKNOWN),
        ] {
            let kind = dialog_prompt(screen, true).map(|d| d.kind);
            assert_ne!(kind, Some(DialogKind::LoginCode), "{name}");
        }
    }

    #[test]
    fn the_login_text_quoted_in_a_reply_or_scrolled_past_is_not_the_step() {
        // A reply quoting it above the normal input box.
        let quoted = format!(
            "\n⏺ At sign-in it shows:\n\n     Paste code here if prompted >\n\n✻ Cooked for 3s\n\n{RULE}\n❯\n{RULE2}\n{STATUS}"
        );
        assert_eq!(dialog_prompt(&quoted, false), None);
        assert_ne!(classify(&quoted, None).activity, Activity::AwaitingInput);
        // The step has moved on: more output after it.
        let moved = format!("{LOGIN_PASTE}\n Login successful. Press Enter to continue\n");
        assert_eq!(dialog_prompt(&moved, false), None);
    }

    #[test]
    fn two_looks_at_the_login_step_are_the_same_dialog() {
        let a = dialog_prompt(LOGIN_PASTE, false).unwrap();
        let b = dialog_prompt(LOGIN_PASTE, true).unwrap();
        assert!(same_dialog(&a, &b));
        assert!(!same_dialog(
            &a,
            &dialog_prompt(TRUST_ARROWS, true).unwrap()
        ));
    }
}
