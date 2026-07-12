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

## How to invoke

### 1. Just show the changes (no feedback needed)

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

### 2. Get approval per hunk (collect decisions)

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

## Decision guide

| Situation | What to do |
|-----------|------------|
| Need approval to proceed | `--select` (you block, get JSON) |
| Just want them informed, you continue | no `--select` (they review, you move on) |
| Change is small / obvious | describe in chat, don't call next-hunk |
| `--select` in a non-interactive context | **errors out** — only use when a human is present at a terminal |

If unsure, prefer **no `--select`** first. The human can always ask you to roll
back; but a blocking `--select` that no one answers will hang.

## Server mode (persistent TUI + live push)

`next-hunk serve` opens a **persistent** review TUI that stays open while you
push updates into it and poll decisions, instead of re-launching a process per
interaction. Use it when the review is **ongoing** — you expect to iterate
(adjust focus, add notes) or poll the human's decisions multiple times.

```bash
# Human opens the persistent TUI (runs with a/r/u enabled):
next-hunk serve

# Agent: push a new focus/note into the live TUI (returns immediately):
next-hunk push --focus src/auth.rs:88 --note banner="re-check token expiry"

# Agent: read the human's accumulated decisions (returns immediately):
next-hunk decision
# {"accepted":["src/auth.rs:h1"],"rejected":["src/util.rs:h1"],"undecided":[]}
```

`push`/`decision` find the server automatically (socket derived from the repo
root) — run them from anywhere in the same repo. `decision` returns the **same
JSON shape** as `--select` quit output, so you parse it identically; it does
**not** wait for the human to quit.

### One-shot `--select` vs server mode

| Situation | What to do |
|-----------|------------|
| Single review, you can block once for the answer | `diff --select` (simpler, no server to manage) |
| Review is ongoing; you'll push updates or poll decisions repeatedly | `serve` + `push` / `decision` |
| Human isn't at a terminal / you can't run `serve` first | `decision` errors: "no server running" — fall back to `--select` |

`serve` requires a Unix OS and the `serve` feature (on by default).

## Writing good `--note` text

- **Explain why, not what.** The diff already shows what changed. Say *why* you
  made the call ("this fixes the OOM by capping the batch size", "renamed to
  match the new domain language").
- **Point `--focus` at the highest-leverage line.** Don't make them scroll to
  find the crux — the focus line is where their eye should land first.
- **One banner note** per invocation works well as a 1-sentence summary; use
  line/hunk notes for the specifics.

## The human's keys (in the TUI)

Once the TUI is open, the human navigates with:

- `j` / `k` — scroll down / up
- `]h` / `[h` — next / previous hunk (wraps across files)
- `Tab` / `h` / `l` — next / previous file
- `a` / `r` / `u` — (**`--select` only**) accept / reject / mark undecided on the current hunk
- `o` — open the focused line in `$EDITOR`
- `q` — quit (in `--select` mode, this emits the JSON)

## Examples

### Refactor across two files, explain the crux

```bash
next-hunk diff \
  --focus src/auth.rs:h1 \
  --note src/auth.rs:h1="Split token validation out of the request handler" \
  --note src/auth.rs:88="New boundary: tokens expire here, not in the middleware" \
  --note banner="Auth refactor — 2 files, core change is extracting token validation"
```

### Ask for approval on a risky change

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
