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
  something — plain `next-hunk diff` / `inspect` still print a summary.
  **Exception:** `diff --focus` / `--note` auto-forwards into a live `serve`
  (no TTY needed). Without a live serve, `--focus` / `--note` / `--select`
  exit non-zero instead of silently dropping annotations. Prefer
  `next-hunk inspect --json` for headless structure.

## Which command shows *all* local changes?

After edits, agents often leave a mix of **staged**, **unstaged**, and
**untracked** files. Plain `next-hunk diff` is worktree-only (like `git diff`)
and **misses staged** files — a half review.

| Goal | Command |
|------|---------|
| **See everything `git status` lists** (recommended after multi-step edits) | `next-hunk diff --all --include-untracked` |
| Unstaged only (default) | `next-hunk diff` |
| Staged only | `next-hunk diff --staged` |
| Staged + unstaged, no untracked | `next-hunk diff --all` |

Config equivalent of the recommended path:

```toml
# .next-hunk/config.toml  or  ~/.config/next-hunk/config.toml
scope = "working-set"
include_untracked = true
```

In the file rail, origins are marked **`S`** staged / **`M`** modified /
**`?`** untracked. A path with both staged and unstaged hunks appears twice.

Default remains worktree-only so everyday `git diff` muscle memory is unchanged.

## Quick start: prefer the CLI (auto-forward when serve is live)

**You do not need to `list` first.** Call `diff --focus` / `--note` directly:

```bash
# Prefer --all when you may have staged some files already:
next-hunk diff --all --include-untracked \
  --focus <path>:<line> --note <path>:<line>="<your explanation>"
```

What happens:

| Situation | Result |
|-----------|--------|
| Human has `next-hunk serve` open for this repo | CLI **auto-forwards** as `push` into that TUI (works **without a TTY**). Prints `ok: forwarded to running serve`. |
| No live serve | Opens a **one-shot** TUI (needs an interactive terminal). |
| You need a second independent TUI | Pass `--no-forward` (or set `auto_forward = false` in config). |

- `--focus <where>`: scroll the TUI to this location.
  - `<path>` — first hunk of that file, e.g. `src/auth.rs`
  - `<path>:<line>` — the code line with this **new-side** line number, e.g. `src/auth.rs:42`
  - `<path>:h<n>` — the `n`-th hunk (1-based) in that file, e.g. `src/auth.rs:h2`
- `--note <target>=<text>`: your annotation, **repeatable**.
  - `<path>:<line>=<text>` — shows under that line
  - `<path>:h<n>=<text>` — shows under that hunk header
  - `banner=<text>` — shows in the status bar (high-level summary)

Without a live serve, the human opens the one-shot TUI, reviews, and quits.
You don't receive any signal back (use `--select` or `serve` + `decision` for
approval).

## Approval: one-shot `--select`

When you need the human to accept/reject your changes hunk-by-hunk:

```bash
next-hunk diff --all --include-untracked --select \
  --focus <path>:<line> --note <path>:<line>="<why this matters>"
```

This **blocks** until the human quits. On quit, stdout gets one JSON line:

```json
{"accepted":["src/auth.rs:h1","src/auth.rs:h3"],"rejected":["src/util.rs:h1"],"undecided":[]}
```

Hunk keys are `"<path>:h<n>"` (1-based ordinal within each file). **Parse stdout
and apply only the `accepted` hunks.** Treat `undecided` as "not approved" — do
not apply them unless the human asked you to.

## Session workflow (persistent TUI + live agent control)

For **ongoing** reviews where you expect to iterate (adjust focus, inspect
structure, add notes, poll decisions) without re-launching a process per
interaction, use server mode.

### Preferred agent path (no list required)

1. Human: `next-hunk serve --all --include-untracked`
2. Agent: `next-hunk diff --focus … --note …` → auto-forwards into that serve
3. Agent: `next-hunk decision` / `comment` / `reload` as needed

Only use `list` / `get` when multiple worktrees are live and you need to
disambiguate, or when debugging why forward did not happen.

### 1. Human opens the persistent TUI

```bash
# Prefer --all so the session includes staged + unstaged (+ untracked if set):
next-hunk serve --all --include-untracked
```

The TUI runs with selection mode on (`a`/`r`/`u` per hunk). It binds a Unix
socket derived from the repo root, so all subsequent commands find it
automatically — no `--socket` flag needed.

### 2. Agent: point / annotate (prefer this over list → navigate)

```bash
next-hunk diff --focus src/auth.rs:42 --note banner="please check token expiry"
# ok: forwarded to running serve
```

Same as `push --focus … --note …`, but you can keep the muscle memory of
`diff`. Explicit push still works:

```bash
next-hunk push --focus src/auth.rs:42 --note banner="please check token expiry"
```

### 3. Agent: discover sessions (only when needed)

```bash
next-hunk list
# c0ffee...  /run/user/1000/next-hunk-c0ffee....sock  files=3  repo=/home/you/project
```

`list` scans `$XDG_RUNTIME_DIR` and `/tmp` for live next-hunk sockets, probes
each, and prints hash/path/files/repo. `repo` is the **absolute worktree root**
known at `serve` startup (not a file path from the diff) — use it to pick the
right session when multiple worktrees are live. `get` shows details for a
specific hash or the current repo.

```bash
next-hunk get
# socket: /run/user/1000/next-hunk-abc123....sock
# repo:   /home/you/project
# files:  3
```

### 4. Agent: inspect the review structure

Prefer **headless** `inspect --json` when you do not already have a live
`serve` (same JSON shape as `review`):

```bash
next-hunk inspect --json
# or a patch / range without opening a TUI:
next-hunk inspect --json path/to.patch
```

With a live session:

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

Both return the file/hunk structure as JSON — file paths, insert/delete
counts, and hunk ranges. No full patch text by default (agents request it
separately if needed). Use this to understand what's in the review before
deciding where to navigate.

One-shot commands also take agent-bridge flags (same as `diff`):

```bash
next-hunk show main..HEAD --focus src/auth.rs:h1 --note banner="please review"
next-hunk patch changes.patch --focus src/a.rs --note src/a.rs:h1="why"
```

### 5. Agent: navigate to what matters

```bash
next-hunk navigate src/auth.rs:42
# ok: navigated to src/auth.rs:42
```

Target syntax: `<path>` (file start), `<path>:<line>` (new-side line number),
or `<path>:h<n>` (1-based hunk ordinal). The TUI scrolls to the target and
syncs the file rail selection. Prefer `diff --focus` / `push` when you also
want notes; use `navigate` for focus-only.

### 6. Agent: add comments

```bash
next-hunk comment add --file src/auth.rs --line 42 "Extracted token validation — fixes the OOM"
# ok: comment added with id c0

next-hunk comment add --file src/auth.rs --hunk 1 "Key change is the boundary shift"
# ok: comment added with id c1
```

Comments are stored on the session. `--line` targets a specific new-side source
line; `--hunk` targets a 1-based hunk ordinal. If neither is given, the comment
becomes a banner note.

```bash
next-hunk comment list
# c0  src/auth.rs line=42  Extracted token validation — fixes the OOM
# c1  src/auth.rs hunk=1   Key change is the boundary shift

next-hunk comment rm c0
# ok: comment removed
```

To show comments in the TUI as note annotations, run:

```bash
next-hunk comment apply
# ok: comments applied to TUI
```

This merges all session comments into the TUI's note renderer, so the human
sees them as `💬 c1: ...` rows below the target line/hunk.

### 7. Agent: poll decisions

```bash
next-hunk decision
# {"accepted":["src/auth.rs:h1"],"rejected":[],"undecided":["src/util.rs:h1"]}
```

Returns immediately — does **not** wait for the human to quit. The JSON shape
matches `--select` quit output exactly, so your parser handles both identically.
`undecided` means "not yet reviewed" — do not apply.

### 8. Agent: reload the diff (optional)

If the diff content changes (e.g. you made more edits), refresh the session:

```bash
next-hunk reload
# ok: session reloaded
```

Re-fetches the diff from the same source the `serve` was started with and
re-parses the review, preserving focus/notes/decisions best-effort (by path
matching). Requires `serve` to have been started with `--watch` (or a reloader).

### 9. Agent: push additional focus/notes

```bash
# Prefer the same command you already use:
next-hunk diff --focus src/util.rs:15 --note banner="Also fixed the batch size"
# ok: forwarded to running serve

# Or the explicit form:
next-hunk push --focus src/util.rs:15 --note banner="Also fixed the batch size"
# ok: pushed to running server
```

`push` / auto-forward replaces the focus target and appends notes to the TUI.

## Complete session workflow summary

```
Human:  next-hunk serve
Agent:  next-hunk diff --focus … --note …       # auto-forwards (no list needed)
        next-hunk decision                      # poll human's decisions
        # ... iterate: diff --focus → comment → decision ...
        next-hunk reload                        # refresh if content changed
# Optional when multi-worktree or debugging:
        next-hunk list / get / review / navigate
```

## One-shot `--select` vs server mode

| Situation | What to do |
|-----------|------------|
| Single review, you can block once for the answer | `diff --select` (simpler, no server to manage) |
| Review is ongoing; you'll navigate, comment, or poll repeatedly | `serve` + `diff --focus` / session commands |
| Human isn't at a terminal / you can't run `serve` first | `decision` errors: "no server running" — fall back to `--select` |

`serve` and all session commands (`list`, `get`, `review`, `navigate`,
`comment`, `reload`, `push`, `decision`) require a Unix OS and the `serve`
feature (on by default). Auto-forward on `diff` uses the same socket.

## Decision guide

| Situation | What to do |
|-----------|------------|
| Need approval to proceed | `--select` (blocks, get JSON) or `serve` + `decision` (non-blocking) |
| Just want them informed, you continue | `diff --focus` / `--note` (auto-forwards if serve is live) |
| Change is small / obvious | describe in chat, don't call next-hunk |
| `--select` in a non-interactive context | **errors out** — only use when a human is present at a terminal |
| Iterating with the human | `serve` + `diff --focus` (prefer) or explicit `push` / `navigate` |

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

## Export a full review report on quit

When you need **comments + notes + decisions** in one shot (not just
`accepted`/`rejected`/`undecided`), ask the human to enable quit-time export:

```bash
# JSON only (one line; superset of decision shape — extra fields are optional)
next-hunk diff --export-on-quit json --note banner="please review"

# Markdown for pasting into chat (no select required)
next-hunk diff --export-on-quit markdown

# Both: JSON line, then Markdown body
next-hunk diff --select --export-on-quit both
```

Or persist in config:

```toml
# ~/.config/next-hunk/config.toml  or  .next-hunk/config.toml
export_on_quit = "json"   # none | json | markdown | both
```

**JSON shape** (fields beyond the three decision buckets are omitted when empty):

```json
{
  "accepted": ["src/auth.rs:h1"],
  "rejected": [],
  "undecided": ["src/util.rs:h1"],
  "comments": [
    {"id": "c0", "file": "src/auth.rs", "text": "…", "hunk": 1}
  ],
  "notes": [
    {"file": "src/auth.rs", "text": "…", "line": 42}
  ],
  "banner": "Auth refactor summary"
}
```

Notes:

- Default is `none` so `git core.pager` use does not pollute stdout.
- Without `--select`, decisions are all `undecided` unless the session used
  `serve` (select always on) and the human pressed `a`/`r`.
- Session comments (`next-hunk comment add`) appear under `comments` when the
  human quits a `serve` TUI with export enabled.
- `--select` alone (export `none`) still emits the legacy three-bucket JSON only.

## Examples

### One-shot refactor with explanation

```bash
next-hunk diff --all --include-untracked \
  --focus src/auth.rs:h1 \
  --note src/auth.rs:h1="Split token validation out of the request handler" \
  --note src/auth.rs:88="New boundary: tokens expire here, not in the middleware" \
  --note banner="Auth refactor — 2 files, core change is extracting token validation"
```

### Session: inspect, navigate, comment, poll

```bash
# Discover the session the human opened:
next-hunk list

# Inspect the review structure:
next-hunk review

# Navigate to the critical hunk and add a comment:
next-hunk navigate src/db/migrate.rs:140
next-hunk comment add --file src/db/migrate.rs --line 140 \
  "This drops the legacy user_email column — irreversible"
next-hunk comment apply

# Later, poll the human's decision:
next-hunk decision
```

### Ask for approval on a risky change (one-shot)

```bash
next-hunk diff --all --include-untracked --select \
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
