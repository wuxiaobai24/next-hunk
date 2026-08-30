---
name: next-hunk
description: Show your code changes to the human for review in an interactive terminal
  diff viewer. Use when you've made non-trivial edits and want the human to inspect
  them — especially multi-file changes or logic the human should confirm before you
  proceed. Point the human at what matters and explain your reasoning.
---

# next-hunk: Present your changes for human review

`next-hunk` is a high-performance terminal diff viewer. As a coding agent, use it
to bridge your changes to the human: open the diff, point them at what matters,
explain your reasoning, and (when you need approval) collect their per-hunk
decisions.

## When to use

Use this skill **after** you finish a change and **before** you commit, when:

- You made changes across **multiple files** that are hard to convey in chat.
- The change involves **non-obvious logic** the human should sanity-check.
- You need the human's **approval** to proceed (e.g. risky/irreversible edits).
- The human is working in a **terminal** and asked to see what you did.

## When NOT to use

- A **single trivial edit** (typo, one-liner) — just describe it in chat.
- The change is already committed and the human will review it in the **PR UI**.
- The human is clearly **not at a terminal** (then describe in chat instead).
- You're in a **non-interactive** context (piped/scripted) and only want to show
  something — `next-hunk diff` will still print an inspect summary, but the
  interactive review won't run.

## Quick start: one-shot review (no approval needed)

When you want the human informed but don't need a decision:

```bash
next-hunk diff --focus <path>:<line> --note <path>:<line>="<your explanation>"
```

- `--focus <where>`: scroll the TUI to this location on open.
  - `<path>` — first hunk of that file, e.g. `src/auth.rs`
  - `<path>:<line>` — the code line with this **new-side** line number, e.g. `src/auth.rs:42`
  - `<path>:h<n>` — the `n`-th hunk (1-based) in that file, e.g. `src/auth.rs:h2`
- `--note <target>=<text>`: your annotation, **repeatable**.
  - `<path>:<line>=<text>` — shows under that line
  - `<path>:h<n>=<text>` — shows under that hunk header
  - `banner=<text>` — shows in the status bar (high-level summary)

The human opens the TUI, reviews, and quits. You don't receive any signal back.

## Approval: one-shot `--select`

When you need the human to accept/reject your changes hunk-by-hunk:

```bash
next-hunk diff --select --focus <path>:<line> --note <path>:<line>="<why this matters>"
```

This **blocks** until the human quits. On quit, stdout gets one JSON line:

```json
{"accepted":["src/auth.rs:h1","src/auth.rs:h3"],"rejected":["src/util.rs:h1"],"undecided":[]}
```

Hunk keys are `"<path>:h<n>"` (1-based ordinal within each file). **Parse stdout
and apply only the `accepted` hunks.** Treat `undecided` as "not approved" — do
not apply them unless the human asked you to.

## Session workflow (live agent control of an open TUI)

For **ongoing** reviews where you expect to iterate (adjust focus, inspect
structure, add notes, paint attention marks, poll decisions) without
re-launching a process per interaction, use the session commands.

**Every interactive review is a session** — the human's everyday
`next-hunk diff` or `next-hunk show` is already controllable; `next-hunk
serve` additionally keeps selection mode on (`a`/`r`/`u` per hunk) for
decision collection. Each session binds a Unix socket derived from the repo
root, so all subsequent commands run from anywhere in the repo find it
automatically — no `--socket` flag.

### 1. Discover the session

```bash
next-hunk list
# 131e603184455fa7-1837713  mode=diff pid=1837713  files=3  focus=src/auth.rs:h1:42  repo=/home/me/proj  "proj working tree"  /run/user/1000/next-hunk-….sock

next-hunk get
# mode:    diff
# title:   proj working tree
# repo:    /home/me/proj
# files:   3
# focus:   src/auth.rs:h1:42
```

`list` scans `$XDG_RUNTIME_DIR` and `/tmp` for live next-hunk sockets and
prints each session's id, mode, repo, and the human's current focus. When
several reviews of one repo are open, commands list the candidates — pass
`--hash <session-id>` (the `<hash>-<pid>` id from `list`) to pick one.
Sessions auto-resolve when only one is live.

### 2. Inspect the review structure

```bash
next-hunk review
# {
#   "file_count": 2,
#   "stream_len": 24,
#   "inserts": 12,
#   "deletes": 3,
#   "files": [
#     {
#       "display_path": "src/auth.rs",
#       "inserts": 8,
#       "deletes": 1,
#       "hunks": [ { "header": "@@ -10,5 +10,8 @@", ... } ]
#     }
#   ]
# }
```

`review` returns the file/hunk structure as JSON — file paths, insert/delete
counts, and hunk ranges. No full patch text by default (agents request it
separately if needed). Use this to understand what's in the review before
deciding where to navigate.

To see where the human is **currently looking**:

```bash
next-hunk context
# focus: src/auth.rs:h1:42
next-hunk context --json
# {"file":"src/auth.rs","hunk":1,"line":42}
```

### 3. Navigate to what matters

```bash
next-hunk navigate src/auth.rs:42
# ok: navigated to src/auth.rs:42

next-hunk navigate --next-note   # jump to the next 💬 annotated row
```

Target syntax: `<path>` (file start), `<path>:<line>` (new-side line number),
or `<path>:h<n>` (1-based hunk ordinal). The TUI scrolls to the target and
syncs the file rail selection.

### 4. Add comments (they render live)

```bash
next-hunk comment add --file src/auth.rs --line 42 "Extracted token validation — fixes the OOM" --focus
# ok: comment added with id c0
```

The comment becomes a `💬 c0: …` note row in the TUI **immediately** — no
separate apply step. `--line` targets a new-side source line, `--hunk` a
1-based hunk ordinal, neither makes a banner note; `--focus` also scrolls the
human's TUI to it.

```bash
next-hunk comment list
# c0  src/auth.rs line=42  Extracted token validation — fixes the OOM

next-hunk comment rm c0
next-hunk comment clear --yes            # all agent comments
next-hunk comment clear --all --yes      # include human-authored notes
```

For several comments at once, apply a JSON batch (validated as a whole — a
bad item errors without applying anything):

```bash
printf '%s\n' '{"comments":[
  {"file":"src/auth.rs","line":42,"text":"fixes the OOM"},
  {"file":"src/util.rs","hunk":1,"text":"boundary shift"}
]}' | next-hunk comment apply --stdin --focus
```

### 5. Paint attention marks

When the *specific characters* matter, light them up:

```bash
next-hunk highlight add --file src/db.rs --line 140 --start 8 --end 14 --tone danger --focus
next-hunk highlight list
next-hunk highlight clear --file src/db.rs
```

The range is 1-based half-open (`--start 8 --end 14` marks chars 8..13) and
renders as a colored background + underline over exactly those columns.
Tones: `warning` (default), `danger`, `info`, `accent`. Pair with a comment
on the same line for *what* + *exactly where*.

### 6. Poll decisions

```bash
next-hunk decision
# {"accepted":["src/auth.rs:h1"],"rejected":[],"undecided":["src/util.rs:h1"]}
```

Returns immediately — does **not** wait for the human to quit. The JSON shape
matches `--select` quit output exactly, so your parser handles both identically.
`undecided` means "not yet reviewed" — do not apply. Decisions are only
collected in selection mode (`serve`, or `diff --select`); other sessions
report everything undecided.

### 7. Reload the diff (optional)

If the diff content changes (e.g. you made more edits), refresh the session:

```bash
next-hunk reload
# ok: session reloaded
```

Re-fetches the diff from the same source the session was started with and
re-parses the review, preserving focus/notes/decisions best-effort (by path
matching). Works on any session — no `--watch` needed.

### 8. Push additional focus/notes

```bash
next-hunk push --focus src/util.rs:15 --note banner="Also fixed the batch size"
# ok: pushed to running session
```

`push` replaces the focus target and appends notes to the TUI. Useful for
iterating after the initial setup.

## Complete session workflow summary

```
Human:  next-hunk diff        (or `serve` for decision collection)
Agent:  next-hunk list                          # discover sessions
        next-hunk review                        # inspect structure
        next-hunk context                       # where are they looking?
        next-hunk navigate src/auth.rs:h1       # scroll to key hunk
        next-hunk comment add --file src/auth.rs --hunk 1 "explanation" --focus
        next-hunk highlight add --file src/auth.rs --line 42 --start 6 --end 19
        next-hunk decision                      # poll human's decisions
        # ... iterate: navigate → comment → highlight → decision ...
        next-hunk push --focus ...              # adjust focus
        next-hunk reload                        # refresh if content changed
```

## One-shot `--select` vs session commands

| Situation | What to do |
|-----------|------------|
| Single review, you can block once for the answer | `diff --select` (simpler) |
| Review is ongoing; you'll navigate, comment, or poll repeatedly | session commands (work on any open review) |
| Need live decisions without blocking | `serve` + `decision` |
| Human isn't at a terminal / no session open | session commands error out — fall back to `--select` or describe in chat |

All session commands (`list`, `get`, `context`, `review`, `navigate`,
`comment`, `highlight`, `reload`, `push`, `decision`) require a Unix OS and
the `serve` feature (on by default).

## Decision guide

| Situation | What to do |
|-----------|------------|
| Need approval to proceed | `--select` (blocks, get JSON) or `serve` + `decision` (non-blocking) |
| Just want them informed, you continue | no `--select` (they review, you move on) |
| Change is small / obvious | describe in chat, don't call next-hunk |
| `--select` in a non-interactive context | **errors out** — only use when a human is present at a terminal |
| Iterating with the human | session workflow (list → review → context → navigate → comment → highlight → decision) |

If unsure, prefer **no `--select`** first. The human can always ask you to roll
back; but a blocking `--select` that no one answers will hang.

## Writing good annotation text

- **Explain why, not what.** The diff already shows what changed. Say *why* you
  made the call ("this fixes the OOM by capping the batch size", "renamed to
  match the new domain language").
- **Point `--focus` / `navigate` at the highest-leverage line.** Don't make them
  scroll to find the crux — the focus line is where their eye should land first.
- **One banner note** per invocation works well as a 1-sentence summary; use
  line/hunk notes for the specifics.

## The human's keys (in the TUI)

Once the TUI is open, the human navigates with:

- `j` / `k` — scroll down / up
- `]h` / `[h` — next / previous hunk (wraps across files)
- `Tab` / `h` / `l` — next / previous file
- `zc` / `zo` — fold / unfold current file
- `a` / `r` / `u` — (**`--select` / `serve` only**) accept / reject / mark undecided on the current hunk
- `o` — open the focused line in `$EDITOR`
- `#` — toggle line-number gutter
- `w` — toggle word-level inline diff
- `W` — toggle ignore-whitespace
- `t` — cycle theme (light → auto → dark)
- `/` — search; `n`/`N` next/prev match
- `q` — quit (in `--select`/`serve` mode, emits decisions JSON on quit)

## Examples

### One-shot refactor with explanation

```bash
next-hunk diff \
  --focus src/auth.rs:h1 \
  --note src/auth.rs:h1="Split token validation out of the request handler" \
  --note src/auth.rs:88="New boundary: tokens expire here, not in the middleware" \
  --note banner="Auth refactor — 2 files, core change is extracting token validation"
```

### Session: inspect, navigate, comment, highlight, poll

```bash
# Discover the session the human opened:
next-hunk list

# Inspect the review structure:
next-hunk review

# Navigate to the critical hunk and add a comment (renders live):
next-hunk navigate src/db/migrate.rs:140
next-hunk comment add --file src/db/migrate.rs --line 140 \
  "This drops the legacy user_email column — irreversible" --focus
next-hunk highlight add --file src/db/migrate.rs --line 140 \
  --start 11 --end 22 --tone danger

# Later, poll the human's decision:
next-hunk decision
```

### Ask for approval on a risky change (one-shot)

```bash
next-hunk diff --select \
  --focus src/db/migrate.rs:140 \
  --note src/db/migrate.rs:140="This drops the legacy `user_email` column — irreversible" \
  --note banner="Migration: needs your OK before I run it"
# After the human quits, read stdout JSON and only proceed if the migration
# hunk is in `accepted`.
```

## Installation note for the human

next-hunk must be installed (`cargo install --git https://github.com/wuxiaobai24/next-hunk`)
and the human must be at an interactive terminal. If `next-hunk` isn't on PATH,
suggest the install command rather than failing silently.
