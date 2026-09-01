# next-hunk extension for the pi coding agent

A native extension for the [pi coding agent](https://github.com/earendil-works/pi)
that turns next-hunk's agent bridge into first-class tools: instead of
remembering CLI flags, the model gets a small, self-describing tool set for
driving a review session — and friendly guidance when no session is running.

## What the agent gets

| Tool | Wraps | Purpose |
|---|---|---|
| `nh_inspect` | `nh inspect` | Quick diff summary (files, rows, bytes); no session needed |
| `nh_sessions` | `nh list` + `nh get` | List live review sessions, marking this repo's |
| `nh_review` | `nh review` | File/hunk structure of the live review as JSON |
| `nh_context` | `nh context --json` | Where the human is currently looking |
| `nh_navigate` | `nh navigate` | Scroll the human's TUI to a file/line/hunk, or hop between comments |
| `nh_comment` | `nh comment add/list/rm/clear` | Leave, read, or clean up review comments |
| `nh_highlight` | `nh highlight add/list/clear` | Paint attention marks over char ranges |
| `nh_push` | `nh push` | Push a focus hint + banner/notes into the TUI |
| `nh_reload` | `nh reload` | Re-read the diff after edits, preserving state |
| `nh_decision` | `nh decision` | Read the human's per-hunk accept/reject verdicts |

Plus a `/nh` slash command for the human (`/nh`, `/nh decision`,
`/nh comments`) and a session-start check that surfaces an already-running
review session while pi boots.

The extension is a single TypeScript file with no dependencies beyond pi's
own packages. It finds the binary as `nh`, falls back to `next-hunk`, and
honors a `NEXT_HUNK_BIN` override. Session-dependent tools never fail
confusingly: with no session running they tell the model to ask the human to
open one with `nh diff` (or `nh serve`).

## Install

Requires pi ≥ 0.84 and a next-hunk binary on PATH
([install](../README.md#install)); the repo you're reviewing needs nothing
added to it.

Pick one:

```bash
# Global — every pi session
cp pi/next-hunk.ts ~/.pi/agent/extensions/next-hunk.ts

# Per-project — only inside a trusted project
mkdir -p .pi/extensions && cp pi/next-hunk.ts .pi/extensions/next-hunk.ts

# Ad hoc
pi -e ./pi/next-hunk.ts
```

## Usage

The typical loop, all from inside pi:

1. You edit code; the agent calls `nh_inspect` to check what changed.
2. You open `nh diff` (or `nh serve` for per-hunk accept/reject) in another
   terminal or tmux pane.
3. The agent guides you: `nh_push` to focus and annotate, `nh_comment` /
   `nh_highlight` for detail, `nh_navigate` to walk hunks, `nh_reload` after
   further edits.
4. The agent calls `nh_decision` to read your verdicts and act on them.

`pi/next-hunk.ts` is standalone — copy it anywhere; the rest of this repo is
not needed at runtime.

## Development

Typecheck the extension against the real pi packages in a scratch directory:

```bash
mkdir -p /tmp/nh-check && cd /tmp/nh-check
npm init -y
npm i @earendil-works/pi-coding-agent @earendil-works/pi-ai typescript @types/node
cp /path/to/next-hunk/pi/next-hunk.ts .
npx tsc --strict --noEmit --module esnext --moduleResolution bundler \
  --skipLibCheck --types node next-hunk.ts
```

The extension is verified end-to-end against a live `nh serve` session
(spawned in a pty) covering every tool, the validation errors, the
no-session guidance, and binary resolution.
