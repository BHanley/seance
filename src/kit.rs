//! The setup kit: opt-in starters for a user's own Claude (Seance menu > Set up
//! your Claude…). Each switch writes a short generic starter and records what it
//! wrote in `kit.json`; off removes exactly that and nothing else.
//!
//! The rules, which the tests pin:
//! - A file or settings key that already exists and was not written by the kit
//!   is never touched. The switch reads "already set up" and writes nothing.
//! - The only edits to a user's own file are one appended `@import` line in
//!   `~/.claude/CLAUDE.md`, one key and one hook entry in
//!   `~/.claude/settings.json`. Each is recorded, and off takes out only it.
//! - Off leaves a file alone when the user has edited it since.
//!
//! Everything takes an [`Env`] so tests run against a temp home.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    DesignSheets,
    BrainNotes,
    Memory,
    Rules,
    WeeklyReview,
    ContextWarning,
}

/// Tile and checklist order.
pub const ITEMS: [Item; 6] = [
    Item::DesignSheets,
    Item::BrainNotes,
    Item::Memory,
    Item::Rules,
    Item::WeeklyReview,
    Item::ContextWarning,
];

impl Item {
    fn id(self) -> &'static str {
        match self {
            Item::DesignSheets => "design-sheets",
            Item::BrainNotes => "brain-notes",
            Item::Memory => "memory",
            Item::Rules => "rules",
            Item::WeeklyReview => "weekly-review",
            Item::ContextWarning => "context-warning",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Item::DesignSheets => "Design sheets",
            Item::BrainNotes => "Brain notes",
            Item::Memory => "Memory",
            Item::Rules => "Always-on rules",
            Item::WeeklyReview => "Weekly skill review",
            Item::ContextWarning => "Context-full warning",
        }
    }
    pub fn why(self) -> &'static str {
        match self {
            Item::DesignSheets => "Pick between design options by tapping pictures",
            Item::BrainNotes => "One folder of notes every Claude session reads",
            Item::Memory => "Claude remembers what you taught it last time",
            Item::Rules => "Your standing rules, loaded every session",
            Item::WeeklyReview => "Each week Claude turns your corrections into rules",
            Item::ContextWarning => "A heads-up before a long chat gets too full",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Item::DesignSheets => "▦",
            Item::BrainNotes => "◈",
            Item::Memory => "◷",
            Item::Rules => "☰",
            Item::WeeklyReview => "↺",
            Item::ContextWarning => "◔",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    On,
    Off,
    /// Something of the user's is already there; the kit wrote nothing.
    AlreadySetUp,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::On => "On",
            Status::Off => "Off",
            Status::AlreadySetUp => "Already set up",
        }
    }
}

/// Where things live. `launchctl` is false in tests.
pub struct Env {
    pub home: PathBuf,
    /// `~/.config/seance`
    pub config: PathBuf,
    /// Invoke's config folder (`$INVOKE_CONF`, else `~/.config/seance-chat`).
    pub invoke_conf: PathBuf,
    pub launchctl: bool,
}

impl Env {
    pub fn real() -> Env {
        let home = PathBuf::from(shellexpand::tilde("~").as_ref());
        let invoke_conf = match std::env::var("INVOKE_CONF") {
            Ok(v) if !v.is_empty() => PathBuf::from(v),
            _ => home.join(".config/seance-chat"),
        };
        Env {
            config: crate::seance_custom::config_dir(),
            invoke_conf,
            home,
            launchctl: cfg!(target_os = "macos"),
        }
    }
    fn brain(&self) -> PathBuf {
        self.home.join("Claude/Brain")
    }
    fn claude_md(&self) -> PathBuf {
        self.home.join(".claude/CLAUDE.md")
    }
    fn settings(&self) -> PathBuf {
        self.home.join(".claude/settings.json")
    }
    fn sheets_flag(&self) -> PathBuf {
        self.invoke_conf.join("sheets")
    }
    fn sheets_skill(&self) -> PathBuf {
        self.home.join(".claude/skills/design-sheets/SKILL.md")
    }
    fn hook_script(&self) -> PathBuf {
        self.home.join(".claude/hooks/seance-context-nudge.sh")
    }
    fn review_script(&self) -> PathBuf {
        self.config.join("kit/weekly-review.sh")
    }
    fn plist(&self) -> PathBuf {
        self.home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    }
    fn record_path(&self) -> PathBuf {
        self.config.join("kit.json")
    }
}

const LABEL: &str = "xyz.ham.seance.weekly-review";

/// One thing the kit wrote. Off undoes these in reverse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Wrote {
    /// A file the kit created, with a hash of what it wrote.
    File { path: String, hash: u64 },
    /// A folder the kit created.
    Dir { path: String },
    /// A line appended to a user's file; `created` when the file was new.
    Line {
        path: String,
        text: String,
        created: bool,
    },
    /// A key set in a JSON settings file.
    Key {
        path: String,
        key: String,
        value: Value,
        created: bool,
    },
    /// A command hook added under `hooks.UserPromptSubmit`.
    Hook {
        path: String,
        command: String,
        created: bool,
    },
    /// A launchd agent: its plist.
    Launchd { plist: String },
}

#[derive(Default, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    items: BTreeMap<String, Vec<Wrote>>,
}

fn load_record(env: &Env) -> Record {
    std::fs::read_to_string(env.record_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_record(env: &Env, rec: &Record) -> Result<(), String> {
    let json = serde_json::to_string_pretty(rec).map_err(|e| e.to_string())?;
    write_atomic(&env.record_path(), json.as_bytes())
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("kit-tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The user's own thing that makes this item "already set up", if any.
fn already(item: Item, env: &Env) -> bool {
    match item {
        Item::DesignSheets => env.sheets_flag().exists() || env.sheets_skill().exists(),
        Item::BrainNotes => env.brain().join("INDEX.md").exists(),
        Item::Rules => env.brain().join("rules.md").exists(),
        Item::Memory => {
            env.brain().join("memory/MEMORY.md").exists()
                || read_json(&env.settings())
                    .ok()
                    .and_then(|v| v.get("autoMemoryDirectory").cloned())
                    .is_some()
        }
        Item::ContextWarning => env.hook_script().exists(),
        Item::WeeklyReview => env.plist().exists() || env.review_script().exists(),
    }
}

pub fn status(item: Item, env: &Env) -> Status {
    if load_record(env).items.contains_key(item.id()) {
        Status::On
    } else if already(item, env) {
        Status::AlreadySetUp
    } else {
        Status::Off
    }
}

/// Turn an item on. `AlreadySetUp` means nothing was written. A failure part
/// way rolls back what was written, so a half-set-up item never lingers.
pub fn on(item: Item, env: &Env) -> Result<Status, String> {
    match status(item, env) {
        Status::Off => {}
        other => return Ok(other),
    }
    let mut wrote = Vec::new();
    if let Err(e) = write_item(item, env, &mut wrote) {
        undo(&wrote, env);
        return Err(e);
    }
    let mut rec = load_record(env);
    rec.items.insert(item.id().to_string(), wrote.clone());
    if let Err(e) = save_record(env, &rec) {
        undo(&wrote, env);
        return Err(e);
    }
    Ok(Status::On)
}

/// Turn an item off: remove exactly what `on` recorded.
pub fn off(item: Item, env: &Env) -> Result<Status, String> {
    let mut rec = load_record(env);
    let Some(wrote) = rec.items.remove(item.id()) else {
        return Ok(status(item, env));
    };
    undo(&wrote, env);
    // A folder this item made may still hold another item's file. Hand it on,
    // so whichever switch goes off last takes the folder out.
    let inherited: Vec<Wrote> = wrote
        .into_iter()
        .filter(|w| matches!(w, Wrote::Dir { path } if Path::new(path).exists()))
        .collect();
    if let Some(next) = rec.items.values_mut().next() {
        let mut dirs = inherited;
        dirs.append(next);
        *next = dirs;
    }
    save_record(env, &rec)?;
    Ok(status(item, env))
}

// ── writing ────────────────────────────────────────────────────────────────

fn write_item(item: Item, env: &Env, wrote: &mut Vec<Wrote>) -> Result<(), String> {
    match item {
        Item::DesignSheets => {
            new_file(&env.sheets_flag(), SHEETS_FLAG.as_bytes(), false, wrote)?;
            new_file(&env.sheets_skill(), SHEETS_SKILL.as_bytes(), false, wrote)
        }
        Item::BrainNotes => {
            let index = env.brain().join("INDEX.md");
            new_file(&index, BRAIN_INDEX.as_bytes(), false, wrote)?;
            add_line(&env.claude_md(), "@~/Claude/Brain/INDEX.md", wrote)
        }
        Item::Rules => {
            let rules = env.brain().join("rules.md");
            new_file(&rules, RULES.as_bytes(), false, wrote)?;
            add_line(&env.claude_md(), "@~/Claude/Brain/rules.md", wrote)
        }
        Item::Memory => {
            let dir = env.brain().join("memory");
            new_file(&dir.join("MEMORY.md"), MEMORY.as_bytes(), false, wrote)?;
            set_key(
                &env.settings(),
                "autoMemoryDirectory",
                json!(s(&dir)),
                wrote,
            )
        }
        Item::ContextWarning => {
            let script = env.hook_script();
            new_file(&script, CONTEXT_HOOK.as_bytes(), true, wrote)?;
            add_hook(&env.settings(), &s(&script), wrote)
        }
        Item::WeeklyReview => {
            let script = env.review_script();
            new_file(&script, weekly_script().as_bytes(), true, wrote)?;
            let plist = env.plist();
            new_file(&plist, plist_text(&script).as_bytes(), false, wrote)?;
            if env.launchctl {
                launchctl(&["bootstrap", &gui_domain(), &s(&plist)])?;
            }
            wrote.push(Wrote::Launchd { plist: s(&plist) });
            Ok(())
        }
    }
}

/// Create folders down to `dir`, recording each one that was new.
fn make_dirs(dir: &Path, wrote: &mut Vec<Wrote>) -> Result<(), String> {
    if dir.exists() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        make_dirs(parent, wrote)?;
    }
    std::fs::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    wrote.push(Wrote::Dir { path: s(dir) });
    Ok(())
}

/// Create a file that must not exist yet. Never overwrites.
fn new_file(path: &Path, body: &[u8], exec: bool, wrote: &mut Vec<Wrote>) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        make_dirs(dir, wrote)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(body)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    wrote.push(Wrote::File {
        path: s(path),
        hash: fnv(body),
    });
    #[cfg(unix)]
    if exec {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    let _ = exec;
    Ok(())
}

/// Append `text` as its own line. A line already there (the user's, or
/// another switch's) is left alone and not recorded.
fn add_line(path: &Path, text: &str, wrote: &mut Vec<Wrote>) -> Result<(), String> {
    let existing = std::fs::read_to_string(path).ok();
    if existing
        .as_deref()
        .is_some_and(|t| t.lines().any(|l| l.trim() == text))
    {
        return Ok(());
    }
    let created = existing.is_none();
    if created {
        if let Some(dir) = path.parent() {
            make_dirs(dir, wrote)?;
        }
    }
    let mut body = existing.unwrap_or_default();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(text);
    body.push('\n');
    write_atomic(path, body.as_bytes())?;
    wrote.push(Wrote::Line {
        path: s(path),
        text: text.to_string(),
        created,
    });
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Err(_) => Ok(json!({})),
        Ok(t) if t.trim().is_empty() => Ok(json!({})),
        Ok(t) => match serde_json::from_str::<Value>(&t) {
            Ok(v) if v.is_object() => Ok(v),
            _ => Err(format!(
                "{} is not a JSON object; left alone",
                path.display()
            )),
        },
    }
}

fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    let body = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    write_atomic(path, format!("{body}\n").as_bytes())
}

fn set_key(path: &Path, key: &str, value: Value, wrote: &mut Vec<Wrote>) -> Result<(), String> {
    let created = !path.exists();
    let mut v = read_json(path)?;
    if v.get(key).is_some() {
        return Err(format!("{key} is already set in {}", path.display()));
    }
    if created {
        if let Some(dir) = path.parent() {
            make_dirs(dir, wrote)?;
        }
    }
    v[key] = value.clone();
    write_json(path, &v)?;
    wrote.push(Wrote::Key {
        path: s(path),
        key: key.to_string(),
        value,
        created,
    });
    Ok(())
}

fn add_hook(path: &Path, command: &str, wrote: &mut Vec<Wrote>) -> Result<(), String> {
    let created = !path.exists();
    let mut v = read_json(path)?;
    if created {
        if let Some(dir) = path.parent() {
            make_dirs(dir, wrote)?;
        }
    }
    let list = v
        .as_object_mut()
        .ok_or("settings is not an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("hooks is not an object")?
        .entry("UserPromptSubmit")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("UserPromptSubmit is not a list")?;
    list.push(json!({ "hooks": [{ "type": "command", "command": command }] }));
    write_json(path, &v)?;
    wrote.push(Wrote::Hook {
        path: s(path),
        command: command.to_string(),
        created,
    });
    Ok(())
}

// ── undoing ────────────────────────────────────────────────────────────────

fn undo(wrote: &[Wrote], env: &Env) {
    for w in wrote.iter().rev() {
        match w {
            Wrote::File { path, hash } => {
                // Edited since? Then it is the user's now.
                if std::fs::read(path).is_ok_and(|b| fnv(&b) == *hash) {
                    let _ = std::fs::remove_file(path);
                }
            }
            Wrote::Dir { path } => {
                // remove_dir only removes an empty folder.
                let _ = std::fs::remove_dir(path);
            }
            Wrote::Line {
                path,
                text,
                created,
            } => undo_line(Path::new(path), text, *created),
            Wrote::Key {
                path,
                key,
                value,
                created,
            } => undo_key(Path::new(path), key, value, *created),
            Wrote::Hook {
                path,
                command,
                created,
            } => undo_hook(Path::new(path), command, *created),
            Wrote::Launchd { plist } => {
                if env.launchctl {
                    let _ = launchctl(&["bootout", &gui_domain(), plist]);
                }
            }
        }
    }
}

fn undo_line(path: &Path, text: &str, created: bool) {
    let Ok(body) = std::fs::read_to_string(path) else {
        return;
    };
    let mut removed = false;
    let kept: Vec<&str> = body
        .lines()
        .filter(|l| {
            if !removed && l.trim() == text {
                removed = true;
                return false;
            }
            true
        })
        .collect();
    if kept.iter().all(|l| l.trim().is_empty()) && created {
        let _ = std::fs::remove_file(path);
    } else if removed {
        let mut out = kept.join("\n");
        out.push('\n');
        let _ = write_atomic(path, out.as_bytes());
    }
}

fn undo_key(path: &Path, key: &str, value: &Value, created: bool) {
    let Ok(mut v) = read_json(path) else { return };
    if v.get(key) == Some(value) {
        v.as_object_mut().map(|o| o.remove(key));
    }
    finish_json(path, v, created);
}

fn undo_hook(path: &Path, command: &str, created: bool) {
    let Ok(mut v) = read_json(path) else { return };
    if let Some(list) = v
        .pointer_mut("/hooks/UserPromptSubmit")
        .and_then(Value::as_array_mut)
    {
        list.retain(|entry| {
            !entry["hooks"]
                .as_array()
                .is_some_and(|h| h.iter().any(|x| x["command"] == command))
        });
    }
    // Prune what the kit had to create to hold the hook.
    let empty_list = v
        .pointer("/hooks/UserPromptSubmit")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    if empty_list {
        if let Some(hooks) = v.get_mut("hooks").and_then(Value::as_object_mut) {
            hooks.remove("UserPromptSubmit");
        }
    }
    if v.get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|h| h.is_empty())
    {
        v.as_object_mut().map(|o| o.remove("hooks"));
    }
    finish_json(path, v, created);
}

/// Write a settings file back, or delete it when the kit made it and nothing
/// else is in it now.
fn finish_json(path: &Path, v: Value, created: bool) {
    if created && v.as_object().is_some_and(|o| o.is_empty()) {
        let _ = std::fs::remove_file(path);
    } else {
        let _ = write_json(path, &v);
    }
}

fn gui_domain() -> String {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    format!("gui/{}", uid.trim())
}

fn launchctl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "launchctl {}: {}",
            args[0],
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

// ── first run ──────────────────────────────────────────────────────────────

/// Whether the first-run tiles show. Only on a fresh install: the Seance Custom
/// helper has never been made (`custom_created` is false), and the tiles have
/// not been looked at. Either way the marker is written, so an install that
/// upgrades into this version never gets a popup later.
pub fn first_run(env: &Env, custom_created: bool) -> bool {
    let marker = env.config.join("kit-seen");
    if marker.exists() {
        return false;
    }
    let _ = write_atomic(&marker, b"1");
    !custom_created
}

// ── starter texts (generic; nothing personal ships) ───────────────────────

const BRAIN_INDEX: &str = "# Brain index

This folder is your notes, kept where every Claude session can read them.
Put what Claude should know about you and your work here, one short note per
topic, and list each note below with one line saying what it holds.
`rules.md` is for standing rules, `memory/` is what Claude learns as you
work, and `rule-proposals/` is where a weekly review leaves suggestions.

## Notes

(none yet)
";

/// Its presence turns Invoke's sheet pages on (read when Invoke starts).
const SHEETS_FLAG: &str =
    "# Written by Seance (Set up your Claude > Design sheets). Delete to turn sheets off.\n";

const SHEETS_SKILL: &str = "---
name: design-sheets
description: Show design options as a sheet of pictures the user can check or X, instead of describing them
---

# Design sheets

When you explore a design (a logo, a layout, colors, a screen), do not describe the options. Show them.

1. Make each option its own image. Save them in `~/Claude/Projects/<project>/sheets/<name>/`, one file per option.
2. In that folder write `sheet.json` with a `name`, a short `note`, and `items`: one entry per image with its file name and a one-line caption.
3. Put one line in your answer: `sheet: ~/Claude/Projects/<project>/sheets/<name>`. The user gets a page where they check the ones they like and X the ones they don't.
4. Before you start, read the earlier `sheet.json` files in that project. Ideas the user checked carry over. Ideas they X'd stay out.
";

const MEMORY: &str = "# Memory

Claude adds one line per thing worth remembering. Each links to a note.
";

const RULES: &str = "# My working rules

Standing rules Claude follows in every session. One line each, in plain words.

- Ask before guessing. If a request could mean two things, ask which.
- Show your work before saying it is done: what you ran and what you saw.
- Use plain words. Short sentences, no jargon unless I used it first.
- Keep changes small. Do what I asked, not more, and say what you left alone.
- Say what you did not check. Never imply you tested something you did not.
- Do not delete or overwrite anything of mine without asking first.
- When you are not sure, say so and say how sure you are.

To add a rule, write one line above. To retire one, delete its line.
";

const CONTEXT_HOOK: &str = r#"#!/bin/sh
# Seance setup kit: context-full warning (UserPromptSubmit hook).
# Reads the current context size from the session transcript and adds a note
# once it is long: a heads-up past 300K tokens, a /compact ask from 500K.
in=$(cat)
f=$(printf '%s' "$in" | sed -n 's/.*"transcript_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
[ -n "$f" ] && [ -f "$f" ] || exit 0
last() { tail -c 400000 "$f" | grep -o "\"$1\":[0-9]*" | tail -1 | grep -o '[0-9]*$'; }
a=$(last input_tokens)
b=$(last cache_read_input_tokens)
c=$(last cache_creation_input_tokens)
n=$(( ${a:-0} + ${b:-0} + ${c:-0} ))
k=$(( n / 1000 ))
if [ "$n" -ge 500000 ]; then
  echo "This conversation is about ${k}K tokens long. Ask the user to run /compact now, and say so at the top of your reply."
elif [ "$n" -ge 300000 ]; then
  echo "This conversation is about ${k}K tokens long. Mention it to the user once, and finish the current step before starting something big."
fi
exit 0
"#;

fn weekly_script() -> String {
    r#"#!/bin/sh
# Seance setup kit: weekly corrections review. Run by launchd once a week.
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
week=$(date +%G-W%V)
dir="$HOME/Claude/Brain/rule-proposals"
mkdir -p "$dir"
claude -p "Read the Claude session transcripts under $HOME/.claude/projects that changed in the last 7 days (find them with: find $HOME/.claude/projects -name '*.jsonl' -mtime -7). Find the places where the user corrected you or told you to do something differently. For each one, propose one short rule in plain words that would have avoided it. Write the proposals to $dir/$week.md as a list, each with a one-line quote of the correction. Do not edit any other file. The user will read the file and copy the rules they want into $HOME/Claude/Brain/rules.md." \
  --permission-mode acceptEdits --allowedTools "Read,Glob,Grep,Write,Bash(find:*)" >/dev/null 2>&1
"#
    .to_string()
}

/// Mondays at 9:00.
fn plist_text(script: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>/bin/sh</string><string>{}</string></array>
  <key>StartCalendarInterval</key>
  <dict><key>Weekday</key><integer>1</integer><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
</dict>
</plist>
"#,
        script.display()
    )
}

#[cfg(test)]
mod tests;
