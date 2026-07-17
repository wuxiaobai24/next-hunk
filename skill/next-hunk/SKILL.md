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
  something — plain `next-hunk diff` / `inspect` still print a summary, but
  **do not pass `--focus` / `--note` / `--select` without a TTY** (they exit
  non-zero instead of silently dropping your annotations). Prefer
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
| **Whole feature branch vs main** (recommended after finishing a feature) | `next-hunk diff --base origin/main` |
| PR-style fork point (merge-base) | `next-hunk diff --strategy merge-base --base origin/main` |
| Commits ahead of `@{upstream}` | `next-hunk diff --strategy upstream-ahead` |
| Explicit range (same as `show`) | `next-hunk diff --range main..HEAD` |

### After finishing a feature branch

When the human should review **everything the branch changed relative to
upstream/main** (not just uncommitted worktree noise), prefer a base review:

```bash
# Direct base tree vs worktree (includes local uncommitted edits):
next-hunk diff --base origin/main \
  --focus <path>:<line> --note banner="feature complete — full branch vs main"

# PR-style: left side is merge-base(origin/main, HEAD):
next-hunk diff --strategy merge-base --base origin/main
```

`inspect --base origin/main --json` gives the same structure headless (no TUI).
Large branch diffs still use viewport IR — do not fall back to dumping the full
patch into chat.

Config equivalent of the recommended local path:

```toml
# .next-hunk/config.toml  or  ~/.config/next-hunk/config.toml
scope = "working-set"
include_untracked = true
# Optional default branch base:
# base = "origin/main"
# strategy = "merge-base"
```

In the file rail, origins are marked **`S`** staged / **`M`** modified /
**`?`** untracked for local scopes. Base/range reviews show +/− relative to the
chosen base (rail still lists per-file insert/delete counts).

Default remains worktree-only so everyday `git diff` muscle memory is unchanged.

## Git vs Jujutsu (jj)

next-hunk auto-detects the VCS: if a `.jj` workspace is present (including
colocated git+jj), it uses **jj** (`jj diff --git` → same IR). Pure git repos
still use gix. Override with config or CLI:

```toml
# .next-hunk/config.toml
vcs = "auto"   # auto | git | jj
```

```bash
next-hunk diff --vcs jj
next-hunk show @ --vcs jj
```

**jj differences agents should know:**

- No staging area — plain `next-hunk diff` is enough; `--staged` is empty;
  `--all` / `--include-untracked` are largely git-oriented (untracked is
  ignored under jj with a note).
- Revisions use jj revsets (`@`, `@-`, bookmarks) as well as `A..B` ranges.
- `serve` / `list` / `review` / `navigate` / `comment` / `decision` work the
  same once the workspace is detected.

Details: repository `docs/VCS.md`.

## Quick start: one-shot review (no approval needed)

When you want the human informed but don't need a decision:

```bash
# Prefer --all when you may have staged some files already:
next-hunk diff --all --include-untracked \
  --focus <path>:<line> --note <path>:<line>="<your explanation>"
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
interaction, use server mode:

### 1. Human opens the persistent TUI

```bash
# Prefer --all so the session includes staged + unstaged (+ untracked if set):
next-hunk serve --all --include-untracked
```

The TUI runs with selection mode on (`a`/`r`/`u` per hunk). It binds a Unix
socket derived from the **worktree root path** (not the shared `.git` common
dir), so all subsequent commands find it automatically — no `--socket` flag
needed. **Each `git worktree` checkout gets its own session** — two agents in
two linked worktrees can `serve` in parallel without stealing each other's
socket.

### 2. Agent: discover the session

```bash
next-hunk list
# c0ffee...  /run/user/1000/next-hunk-c0ffee....sock  files=3  repo=/home/you/project  (current)
# ab12cd...  /run/user/1000/next-hunk-ab12cd....sock  files=1  repo=/home/you/project-feature
```

`list` scans `$XDG_RUNTIME_DIR` and `/tmp` for live next-hunk sockets, probes
each, and prints hash/path/files/repo. `repo` is the **absolute worktree root**
known at `serve` startup (not a file path from the diff) — use it to pick the
right session when multiple worktrees are live. The `(current)` marker flags
the session whose worktree matches your cwd.

When several agents work on the **same** repo via linked worktrees:

```bash
# Only sessions (and idle worktree roots) for *this* repository:
next-hunk list --all-worktrees
# worktrees of this repo: 2 total, 1 with live serve
# c0ffee...  ...  files=3  repo=/home/you/project  (current)
# ab12cd...  —  files=-  repo=/home/you/project-feature  (no serve)
```

`get` shows details for a specific hash or the current worktree's socket.

```bash
next-hunk get
# socket: /run/user/1000/next-hunk-abc123....sock
# repo:   /home/you/project
# files:  3
```

### Multi-agent / multi-worktree layout (recommended)

For parallel agents (one task per worktree), give each worktree its own
`serve` and session:

```text
┌─ tmux ─────────────────────────────────────────────────────────┐
│ pane 0: human review          │ pane 1: agent A (feature-a)    │
│   cd ~/project-a &&           │   cd ~/project-a && …edits…    │
│   next-hunk serve --all       │   next-hunk list               │
│                               │   next-hunk navigate …         │
├───────────────────────────────┼────────────────────────────────┤
│ pane 2: human review          │ pane 3: agent B (feature-b)    │
│   cd ~/project-b &&           │   cd ~/project-b && …edits…    │
│   next-hunk serve --all       │   next-hunk list --all-worktrees│
└───────────────────────────────┴────────────────────────────────┘
```

Setup sketch:

```bash
# From the main checkout:
git worktree add ../project-a -b agent/a
git worktree add ../project-b -b agent/b

# Human: one serve per worktree (separate tmux panes / terminals):
cd ../project-a && next-hunk serve --all --include-untracked
cd ../project-b && next-hunk serve --all --include-untracked
```

**How the agent should choose a session:**

| Situation | What to do |
|-----------|------------|
| You are already `cd`'d into the worktree you edit | Prefer bare `next-hunk review` / `navigate` / `decision` (auto-resolves **this** worktree's socket). |
| Multiple `serve` processes may be live | `next-hunk list` — match `repo=` to your worktree path; use the hash with `--hash` on navigate/comment/etc. |
| You only care about worktrees of **this** repo | `next-hunk list --all-worktrees` — ignores unrelated projects' sessions. |
| Wrong worktree / no `(current)` session | `cd` into the correct worktree, or pass the hash from `list`. Do **not** drive another agent's TUI. |

Session commands that accept an optional hash (`get`, `review`, `navigate --hash`,
`comment … --hash`, `reload --hash`) target a non-cwd worktree when needed.

### 3. Agent: inspect the review structure

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

### 4. Agent: navigate to what matters

```bash
next-hunk navigate src/auth.rs:42
# ok: navigated to src/auth.rs:42
```

Target syntax: `<path>` (file start), `<path>:<line>` (new-side line number),
or `<path>:h<n>` (1-based hunk ordinal). The TUI scrolls to the target and
syncs the file rail selection.

### 5. Agent: add comments

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

### 6. Agent: poll decisions

```bash
next-hunk decision
# {"accepted":["src/auth.rs:h1"],"rejected":[],"undecided":["src/util.rs:h1"]}
```

Returns immediately — does **not** wait for the human to quit. The JSON shape
matches `--select` quit output exactly, so your parser handles both identically.
`undecided` means "not yet reviewed" — do not apply.

### 7. Agent: reload the diff (optional)

If the diff content changes (e.g. you made more edits), refresh the session:

```bash
next-hunk reload
# ok: session reloaded
```

Re-fetches the diff from the same source the `serve` was started with and
re-parses the review, preserving focus/notes/decisions best-effort (by path
matching). Requires `serve` to have been started with `--watch` (or a reloader).

### 8. Agent: push additional focus/notes

```bash
next-hunk push --focus src/util.rs:15 --note banner="Also fixed the batch size"
# ok: pushed to running server
```

`push` replaces the focus target and appends notes to the TUI. Useful for
iterating after the initial setup.

## Complete session workflow summary

```
Human:  next-hunk serve
Agent:  next-hunk list                          # discover session
        next-hunk review                        # inspect structure
        next-hunk navigate src/auth.rs:h1       # scroll to key hunk
        next-hunk comment add --file src/auth.rs --hunk 1 "explanation"
        next-hunk comment apply                 # show in TUI
        next-hunk decision                      # poll human's decisions
        # ... iterate: navigate → comment → apply → decision ...
        next-hunk push --focus ...              # adjust focus
        next-hunk reload                        # refresh if content changed
```

## One-shot `--select` vs server mode

| Situation | What to do |
|-----------|------------|
| Single review, you can block once for the answer | `diff --select` (simpler, no server to manage) |
| Review is ongoing; you'll navigate, comment, or poll repeatedly | `serve` + session commands |
| Human isn't at a terminal / you can't run `serve` first | `decision` errors: "no server running" — fall back to `--select` |

`serve` and all session commands (`list`, `get`, `review`, `navigate`,
`comment`, `reload`, `push`, `decision`) require a Unix OS and the `serve`
feature (on by default).

## Decision guide

| Situation | What to do |
|-----------|------------|
| Need approval to proceed | `--select` (blocks, get JSON) or `serve` + `decision` (non-blocking) |
| Just want them informed, you continue | no `--select` (they review, you move on) |
| Change is small / obvious | describe in chat, don't call next-hunk |
| `--select` in a non-interactive context | **errors out** — only use when a human is present at a terminal |
| Iterating with the human | `serve` + session workflow (list → review → navigate → comment → decision) |

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
