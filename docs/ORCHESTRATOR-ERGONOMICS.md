# Orchestrator ergonomics — running log

Requests from the paceline realtime-v2 orchestrator (pane claude-38 driving
~10 codex/claude workers in circle `codex-10`), 2026-09-27. Goal: driving
many agents from seance should need no screen-scraping and no workarounds.
Its dispatcher (`vita/…/2026-09-27-realtime-v2/dispatch_codex.py`) records
every workaround this batch replaces.

**Deploy model.** Two halves:

- **ctl-side** (the `seance ctl` client): works against the *running* daemon
  because it only composes ops the daemon already serves. It goes live the
  moment `target/release/seance` is rebuilt, since `~/.local/bin/seance`
  symlinks to it. No daemon restart is needed, but rebuilding changes `send`
  behaviour for any orchestrator mid-run (see the maintenance window notes).
- **daemon-side**: needs `cargo build --release && seance upgrade`, which
  keeps sessions alive.

Tested against an isolated daemon (`XDG_RUNTIME_DIR` + `SEANCE_STATE_DIR` +
`SEANCE_SOCKET` in a scratch dir, `target/debug/seance daemon`) with real
Claude and Codex panes. The live daemon and its panes were not touched.

## Batch 1 — 2026-09-27 (commit 9f5f444, undeployed)

| # | Request | Change | Side | Test |
|---|---------|--------|------|------|
| 1 | `send` silently not arriving | `send` confirms delivery: busy / echo / Enter re-pressed on an unsent composer / screen change (shells). Otherwise it re-pastes raw (no new task) `--retry N` (default 1) times, then **exit 3** with the open `task_id`. Refuses to paste into a modal. `--no-confirm`, `--confirm-secs S`. | ctl | `ctl::deliver` judge tests; live: Claude `delivered=busy`; a stty -echo pane gives exit 3 after 2 attempts |
| 2 | note to a working agent | `ctl note-agent PANE TEXT` (`nudge` alias): raw bracketed paste + Enter, confirmed like send. No task is opened or cancelled. | ctl | live: note delivered into running task-1, and task-1 stayed open |
| 3 | busy/idle scraping | `seance_core::agent_state::classify(screen, title)` → `activity` = busy · idle · awaiting-input · limited (+ `exited`/`unknown`) with `activity_evidence`, in roster/brief/status. It reads the **live bottom** of the screen, ignoring a human's scrollback. It finds the spinner up to 24 lines above the input box (Claude prints tips under it; a 12-line scrape misread two busy panes at 8:19pm), and the title spinner `◐◑` also counts. | daemon (ctl uses the same fn client-side for send) | 11 frame tests from live panes |
| 4 | usage limits invisible | `quota` {account, five_hour_used_pct, seven_day_used_pct, five_hour_left_pct, weekly_left_pct} parsed from Claude statusline / Codex footer + warnings. `limited` fires on "hit your (usage) limit / limit reached / out of credits" near the prompt, but not on "…% of your weekly limit left". | daemon | frame tests |
| 5 | manual handover | `ctl handoff PANE [--agent claude] [--name N] [--note T] [--keep]`: spawns a successor in the same cwd and circle, waits for boot, kills the original (one writer), then re-sends the open task with a TAKEOVER preamble and confirms. Prints `{old, slug, task_id, from_task, …}`. | ctl | live: w (task-1) → w-claude task-4, delivered=busy |
| 6 | `new` slug + JSON | `new --json` honours `--wait-ready`. `--task-file PATH` / `--task-stdin` sends a first task (and implies wait-ready). One JSON line: `{slug, name, requested_name, workspace, cwd, command, parent, scratchpad, ready, task_id, delivery}`. Human output warns when the name was taken. | ctl (+ daemon adds name/command/cwd/parent to `new`; ctl fills gaps on old daemons) | parse tests; live Claude + Codex |
| 7 | `status --json` has no cwd | `status` returns the full roster row (`cwd`, `activity`, `quota`, `task_id`, `parent`, …). `handoff` falls back to `brief` for cwd on old daemons. | daemon | engine test |
| 8 | completion | `wait PANE --task ID --artifact PATH --either --fresh`: file (≥min-bytes, written after the task was sent) **or** task done. `wait` now fails fast (exit 1) on a missing/exited pane or a superseded `--task` (these used to time out silently), unless the artifact is there. Documented as *the* robust path in `ctl skill`. | ctl | `artifact_ready` test; live: superseded → exit 1 at once, `--either` → 2s |
| 9 | `seance --help` hangs | `--help`/`-h` anywhere (except `ctl`/`web`/`replay`, which own theirs) prints top-level usage. Before this, `seance upgrade --help` **upgraded the daemon** and `restart-gui --help` restarted the GUI. (Bare `seance --help` was already fixed in 00588d5; the installed release binary predates that.) | ctl binary | `wants_top_help` test |
| 10 | helper panes clutter | `parent` = the spawning pane's `$SEANCE_SESSION` at `new`. It's persisted and survives upgrade handoff; dropped on kill. `roster --parent SLUG` / `roster --top` filter. | daemon (filter ctl) | engine test |

Also fixed on the way: `new --wait-ready`'s boot clear used a blind Codex
sequence (`2\r`). On Codex's *trust* dialog, option 2 is "No, quit". It never
actually fired, because `new` didn't return the command. Dialogs are now
answered from the screen (`agents::boot_dialog_answer`: trust gets Enter, the
update menu gets Skip). Permission prompts are never auto-answered.

## Batch 2 — item 11 (commit below, undeployed)

| # | Request | Change | Side | Test |
|---|---------|--------|------|------|
| 11 | untrusted cwd: ready took ~2 min, then the first send's Enter answered Claude's trust prompt and Claude exited (v2-go-touchup, events 58148-58156) | The real Claude 2.1.280 dialog opens with the cursor on **"❯ No, exit"**, so any Enter quits. `--wait-ready` now detects a trust dialog and **fails at once** (0.3s), naming it and leaving the pane at the prompt. `--trust` (on `new` and `handoff`) accepts it one checked step at a time: an arrow toward the "Yes" line, re-read the screen, and Enter only when the cursor is on "Yes". Codex update menus are skipped. `send` / `note-agent` refuse to paste into any modal (from batch 1). This also corrects batch 1, which answered trust with a bare Enter: fine on Codex, fatal on Claude. | ctl | `agents::trust_is_navigated_to_yes_never_blind_enter` + a classifier frame test, both from the live dialog; live: no-trust → exit 1 in 0.3s, send refused, `--trust` → idle in 3.6s |

### Needs a maintenance window

1. **`cargo build --release && seance upgrade`**: daemon half (activity,
   quota, status row, new-response fields, parent). Sessions survive, but
   the rule for this job is no upgrade mid-run.
2. **Rebuilding `target/release/seance` at all** swaps `seance ctl` under
   the running dispatcher. Nothing breaks on the wire (same version), but
   `send` then blocks up to ~30s confirming, and exits **3** when delivery is
   unconfirmed. `dispatch_codex.py`'s `ctl()` raises on any non-zero exit.
   Adopt it deliberately: either pass `--no-confirm` or handle exit 3 (the
   JSON still carries `task_id`).
3. Existing Codex panes on a shared app-server still need relaunching with
   `--no-daemon` (31d49de).

### What the dispatcher can drop once deployed

- `new_pane` regex on "created X" → `new --json` (`data.slug`).
- `confirm_started` + `nudge` → `send` confirms / `note-agent`.
- `idle()` screen scraping → `status --json` `data.activity`.
- `LIMIT_RE` scraping → `activity == "limited"` / `quota`.
- `migrate()` + TAKEOVER rewrite → `handoff PANE --agent claude --json`.
- result-file poll loop → `wait PANE --task T --artifact F --either --fresh`.

### Not done / why

- `note-agent` doesn't append the note to the task record (`ctl task` still
  shows only the original body). That needs a daemon op; the note is in the
  transcript and the event log (`ctl_send_raw`).
- No agent hooks. The classifier reads the screen the daemon already owns,
  which covers every agent without per-agent config. Hooks (Claude
  Stop/SessionStart) would be a sturdier busy signal later.
