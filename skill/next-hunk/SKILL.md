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

## Optional structural diff (`difftastic`)

next-hunk's default path is **unified text IR** (gix / `jj diff --git`) — that
is the performance moat. For refactors, JSON/HTML nesting, or rename-heavy
edits where line-oriented unified is noisy, you can opt into an external
structural backend:

```bash
# Requires `difft` (difftastic) on PATH — https://github.com/Wilfred/difftastic
next-hunk diff --structural
next-hunk inspect --structural --json
# or config:
# structural = true
# override binary: NEXT_HUNK_DIFFT=/path/to/difft
```

| When to use | When **not** to use |
|-------------|---------------------|
| Human asked for structural / AST-aware view | Everyday agent reviews (default unified is enough) |
| JSON/HTML/config nesting is hard to read in unified | Huge multi-file diffs where latency matters (one `difft` subprocess **per file**) |
| Spot-checking a single refactor file | CI / default benches — structural is **opt-in**, not under PERF gates |

**Rules for agents:**

- **Default stays off.** Do not pass `--structural` unless the human asked or
  the change is clearly structural-noise heavy.
- **Missing `difft` is a hard error** (install link + `NEXT_HUNK_DIFFT`), not a
  silent fallback. If structural fails mid-file, that file keeps unified and
  stderr warns.
- Still the same File/Hunk IR after rewrite — rail / comments / export work.
- See repository `docs/PERF.md` (structural tradeoffs).

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

## Overlay: in-session popup (preferred when agent is inside tmux/zellij)

When **you** run inside the human's multiplexer session (`$TMUX` or `$ZELLIJ`),
prefer **`next-hunk overlay`** over opening a second pane for `serve` or asking
the human to leave the agent chat:

```bash
# One command: floating TUI → human reviews → full export JSON on *your* stdout
next-hunk overlay --all --include-untracked \
  --focus <path>:<line> \
  --note banner="<1-sentence summary>" \
  --note <path>:<line>="<why this matters>"
```

What happens:

| Environment | Result |
|-------------|--------|
| `$TMUX` set | `tmux display-popup` with `diff --select --export-on-quit json` (blocks) |
| `$ZELLIJ` set | floating pane; same export contract |
| No mux, but interactive TTY | one-shot select in the current terminal |
| No mux, headless | **errors** with a clear fallback (see below) — do **not** hang |

On success, stdout is the **full** export JSON (`schema_version`,
`accepted`/`rejected`/`undecided`, `comments`, `notes`, `banner`) — same shape
as `last-export` / `--export-on-quit json`. Parse it and apply the After review
playbook.

**Long-running:** the command blocks until the human quits. Raise your shell
tool timeout to the maximum the harness allows (e.g. 30+ minutes). Do **not**
background it.

**If overlay errors (no mux):**

```text
1. Ask the human to open an adjacent pane and run:
     next-hunk serve --all --include-untracked
   then use diff --focus / decision / last-export as usual.
2. Or they run one-shot themselves:
     next-hunk diff --all --include-untracked --select --export-on-quit json
   and paste / you recover with last-export.
```

Popup size: `NEXT_HUNK_POPUP_WIDTH` / `NEXT_HUNK_POPUP_HEIGHT` (default `90%`).

### Recommended layouts (Claude / Codex / OpenCode)

**Claude Code (tmux)** — agent and human share one session; overlay stacks on top:

```text
┌─ tmux window ──────────────────────────────────────────────────┐
│  Claude Code (this pane owns $TMUX)                            │
│    …edits…                                                     │
│    next-hunk overlay --all --include-untracked \               │
│      --focus src/a.rs:h1 --note banner="please review"         │
│    → display-popup opens; human q → JSON back on Claude stdout │
└────────────────────────────────────────────────────────────────┘
```

**Codex (zellij or tmux)** — same one-command path; if the harness has no mux,
open a neighbor pane for `serve` once per worktree:

```text
┌─ zellij / tmux ────────────────────────────────────────────────┐
│ pane 0: Codex agent          │ pane 1 (only if no overlay):    │
│   next-hunk overlay …        │   next-hunk serve --all         │
│   (or diff --focus if serve) │                                 │
└──────────────────────────────┴─────────────────────────────────┘
```

**OpenCode / Multica dogfood** — after a multi-file edit, before commit:

```bash
# Prefer (agent inside human's tmux/zellij):
next-hunk overlay --all --include-untracked \
  --focus <main-changed-path> \
  --note banner="dogfood: review before commit"

# Fallback when headless / no mux (human already has serve):
next-hunk diff --all --include-untracked \
  --focus <path> --note banner="…"
# then: next-hunk decision  /  after quit: next-hunk last-export
```

Do **not** invent a custom terminal emulator; do **not** fork next-hunk
business logic in a skill script — call the `next-hunk` binary only.

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

### MCP path (when the host speaks MCP)

If your runtime can attach an MCP server, prefer tools over shell:

1. Human: `next-hunk serve --all --include-untracked`
2. Host config: command `next-hunk`, args `["mcp"]` (see repo `docs/MCP.md`)
3. Agent tools: `list_sessions` → `review_structure` / `navigate` /
   `add_comment` / `push_focus_note` → `get_decision` / `reload`

Same Unix-socket semantics as the CLI (optional `hash` for multi-worktree).
Skill + shell CLI remain valid when MCP is unavailable.

### 1. Human opens the persistent TUI

```bash
# Prefer --all so the session includes staged + unstaged (+ untracked if set):
next-hunk serve --all --include-untracked
# On quit: full JSON report on stdout (export_on_quit defaults to json).
# If you miss stdout: next-hunk last-export
```

The TUI runs with selection mode on (`a`/`r`/`u` per hunk). It binds a Unix
socket derived from the **worktree root path** (not the shared `.git` common
dir), so all subsequent commands find it automatically — no `--socket` flag
needed. **Each `git worktree` checkout gets its own session** — two agents in
two linked worktrees can `serve` in parallel without stealing each other's
socket.

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

next-hunk comment add --file src/auth.rs --line 40 --line-end 55 "Rewrite this whole block"
# ok: comment added with id c1

next-hunk comment add --file src/auth.rs --hunk 1 "Key change is the boundary shift"
# ok: comment added with id c2
```

Comments are stored on the session.

| Flag | Meaning |
|------|---------|
| `--line N` | new-side source line (or **range start** when `--line-end` is set) |
| `--line-end M` | inclusive end of a new-side line range (requires `--line`) |
| `--hunk N` | 1-based hunk ordinal |
| (none) | banner note |

```bash
next-hunk comment list
# c0  src/auth.rs line=42     Extracted token validation — fixes the OOM
# c1  src/auth.rs line=40-55  Rewrite this whole block
# c2  src/auth.rs hunk=1      Key change is the boundary shift

next-hunk comment rm c0
# ok: comment removed
```

To show CLI-added comments in the TUI as note annotations, run:

```bash
next-hunk comment apply
# ok: comments applied to TUI
```

This merges all session comments into the TUI's note renderer, so the human
sees them as `💬 c1:…` rows below the target line/hunk. (Comments the human
authors **inside** the TUI via `v`/`c` already render immediately.)

#### How the human marks ranges (TUI)

Without leaving the viewer:

1. Scroll so the top of the viewport is on the code of interest.
2. Press **`v`** — visual select anchors on the first code row in view.
3. **`j` / `k`** extend the selection (status shows `file:start-end`).
4. Press **`c`** — type the note at the bottom prompt; **Enter** saves.
5. Or **`C`** for a whole-hunk comment; bare **`c`** in normal mode comments
   the current top code line only.
6. Quit with `export_on_quit=json` (or `both`) to dump the structured report.

Selection state is **viewport-only** (two stream-row indices). It never forces
full IR materialization — large fixtures stay under the same PERF gates.

#### How agents parse range comments

`export_on_quit` JSON and `comment list` use the same shape:

```json
{
  "comments": [
    {"id": "c0", "file": "src/auth.rs", "text": "single line", "line": 42},
    {"id": "c1", "file": "src/auth.rs", "text": "rewrite block", "line": 40, "line_end": 55},
    {"id": "c2", "file": "src/auth.rs", "text": "hunk note", "hunk": 1}
  ]
}
```

- **`line`** — start line (treat as `line_start`). Always the lower bound.
- **`line_end`** — inclusive end; **omitted** for single-line comments.
- **`hunk`** — 1-based hunk ordinal (hunk-level; no line fields).
- No line/hunk fields → banner.

When applying human feedback, prefer the tightest placement: range → rewrite
that span; single line → that line; hunk → the whole hunk.

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
        next-hunk decision                      # poll decisions (3 buckets)
        # ... iterate: diff --focus → comment → decision ...
        next-hunk reload                        # refresh if content changed
Human:  q  (quit) → stdout full JSON (default export=json)
Agent:  next-hunk last-export                   # if stdout was missed
        # then After review playbook: rejected + comments only
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
`comment`, `reload`, `push`, `decision`) require a **Unix OS** (Linux/macOS) and
the `serve` feature (on by default). Auto-forward on `diff` uses the same
socket. **Windows:** live serve is deferred to 0.9 — use one-shot
`diff --select` / `overlay` / `last-export` / `inspect --json` instead (matrix:
repository `docs/PLATFORMS.md`).

## Decision guide

| Situation | What to do |
|-----------|------------|
| Inside `$TMUX`/`$ZELLIJ`, need one blocking review | **`overlay`** (popup → full JSON on your stdout) |
| Need approval to proceed (no mux) | `--select` (blocks, get JSON) or `serve` + `decision` (non-blocking) |
| Need comments + decisions after human quits | `overlay` / `serve` quit / `last-export` |
| Just want them informed, you continue | `diff --focus` / `--note` (auto-forwards if serve is live) |
| Change is small / obvious | describe in chat, don't call next-hunk |
| `--select` / `overlay` in a non-interactive context | **errors out** — only when a human can see a TTY/popup |
| Iterating with the human | `serve` + `diff --focus` (prefer) or explicit `push` / `navigate` |
| After review: only fix what they rejected | parse export → rejected/commented files only → `diff --focus` |

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
- `v` — visual range select; then `j`/`k` extend, `c` comment, Esc cancel
- `c` / `C` — comment current line / current hunk (Enter saves)
- `a` / `r` / `u` — (**`--select` / `serve` only**) accept / reject / mark undecided on the current hunk
- `o` — open the focused line in `$EDITOR`
- `#` — toggle line-number gutter
- `w` — toggle word-level inline diff
- `W` — toggle ignore-whitespace
- `t` — cycle theme (light → auto → dark → catppuccin-mocha → catppuccin-latte → tokyonight)
- `/` — search; `n`/`N` next/prev match
- `q` — quit (in `--select`/`serve` mode, emits decisions JSON on quit)

## Export a full review report on quit

**`serve` defaults to full JSON export on quit** (decisions + comments + notes
+ banner). Pager / plain `diff` default to `none` so `git core.pager` does not
pollute stdout. Override anytime:

```bash
# JSON only (one line; superset of decision shape — extra fields are optional)
next-hunk diff --export-on-quit json --note banner="please review"

# Markdown for pasting into chat (no select required)
next-hunk diff --export-on-quit markdown

# Both: JSON line, then Markdown body
next-hunk diff --select --export-on-quit both

# Serve: already json by default; pin explicitly if you prefer both:
next-hunk serve --export-on-quit both
```

Or persist in config (applies to all entry points, including serve):

```toml
# ~/.config/next-hunk/config.toml  or  .next-hunk/config.toml
export_on_quit = "json"   # none | json | markdown | both
```

**JSON shape** (`schema_version` is always present; fields beyond the three
decision buckets are omitted when empty):

```json
{
  "schema_version": 1,
  "accepted": ["src/auth.rs:h1"],
  "rejected": [],
  "undecided": ["src/util.rs:h1"],
  "comments": [
    {"id": "c0", "file": "src/auth.rs", "text": "…", "hunk": 1},
    {"id": "c1", "file": "src/auth.rs", "text": "rewrite this", "line": 40, "line_end": 55}
  ],
  "notes": [
    {"file": "src/auth.rs", "text": "…", "line": 42}
  ],
  "banner": "Auth refactor summary"
}
```

Notes:

- **Pager / no-select `diff`:** default `none` — no stdout pollution.
- **`serve`:** default `json` when unset — quit always yields a parseable full
  report. Explicit config/CLI `none` still wins.
- Without `--select`, decisions are all `undecided` unless the session used
  `serve` (select always on) and the human pressed `a`/`r`.
- Session comments (`next-hunk comment add` / in-TUI `c`/`C`) appear under
  `comments` on serve quit (default export).
- `--select` alone (export `none`) still emits the **legacy three-bucket JSON
  only** (no `schema_version` / comments). Prefer export `json` when you need
  comments. Full report is still cached for `last-export`.
- **`next-hunk last-export`:** prints the cached full report from the last
  select/export quit (stored under `.git/next-hunk/last-export.json`). Use
  this when the human quits `serve` on **their** terminal and you missed
  stdout. Does not require a live serve.
- **`decision` during a live serve** still returns three buckets only
  (backward compatible). For comments mid-session use `comment list`; for the
  full post-quit package use stdout export or `last-export`.
- **Non-TTY (piped / agent tool call):** with `--export-on-quit
  json|markdown|both`, next-hunk emits the report **immediately** (no TUI) and
  exits 0 — all hunks `undecided`, plus any `--note`s. It does **not** print
  the inspect `files=…` summary in that case. Without export, non-TTY still
  falls back to inspect. Use this when you need a parseable headless report
  without a human at a terminal; use `--select` / `serve` when you need
  human decisions.

## After review (agent playbook)

When the human finishes a review, **do not** re-read the whole diff. Consume
the export and fix only what they rejected or annotated.

### 1. Obtain the full report

Prefer, in order:

```bash
# A) You blocked on --select / you own the serve terminal stdout:
#    parse the JSON line from that process's stdout (schema_version may be
#    present; always has accepted/rejected/undecided).

# B) Human ran serve and quit — recover the cache:
next-hunk last-export

# C) Live serve still open — decisions only (no comments):
next-hunk decision
next-hunk comment list   # if you need line/hunk notes before they quit
```

Parse as JSON. Tolerate extra fields. Three-bucket shape from `--select`
(export none) / `decision` is a subset: only `accepted` / `rejected` /
`undecided`.

### 2. Build the work set (rejected + commented only)

```text
work_files = unique paths from:
  - every key in `rejected`          # "path:hN" → path
  - every `comments[].file`
  - (optional) `notes[].file` if you treat agent notes as still open

Skip pure `accepted` hunks with no comments on that file.
Treat `undecided` as not approved — do not ship them unless the human said so.
```

### 3. Fix, then re-present only those files

```bash
# After editing the rejected / commented paths:
next-hunk diff --all --include-untracked \
  --focus <first-rejected-or-commented-path> \
  --note banner="Addressed review feedback on N files"

# Or, with a live serve still open:
next-hunk diff --focus <path>:<line> \
  --note <path>:<line>="Fixed: <summary of their comment>"
```

When you need another approval pass:

```bash
next-hunk diff --all --include-untracked --select --export-on-quit json \
  --focus <path>
# or keep using the human's serve session and poll:
next-hunk decision
# after they quit:
next-hunk last-export
```

### 4. Copy-paste checklist

1. `report = last-export` (or quit stdout / `decision` + `comment list`)
2. For each `rejected` key `path:hN` → open that hunk and fix or revert
3. For each `comments[]` → fix at `line`/`line_end` or `hunk` (tightest wins)
4. Do **not** rewrite accepted-only files
5. Re-show with `diff --focus` on the touched paths
6. Stop when `rejected` is empty and open comments are addressed

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

next-hunk must be installed (`cargo install next-hunk`, or
`cargo install --git https://github.com/wuxiaobai24/next-hunk`)
and the human must be at an interactive terminal. If `next-hunk` isn't on PATH,
suggest the install command rather than failing silently.
