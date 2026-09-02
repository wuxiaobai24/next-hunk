---
name: understand
description: Walk the human through freshly generated (usually AI-written) code
  changes in the terminal. Determine the diff range, annotate the key segments
  with the requirement behind each change plus a plain-language explanation,
  and open next-hunk with those annotations rendered inline under the exact
  lines — the terminal replacement for an HTML code-review report. Invoke for
  "/understand", "review 这次改动", "解释一下新生成的代码", "看看这次变更".
---

# understand: explain fresh changes, line by line, in the terminal

Turn "the changes we haven't digested yet" (typically AI-generated) into a
guided terminal walkthrough. next-hunk's file rail is the changed-file tree,
the syntax-highlighted diff is the code pane, and your annotations — the
requirement behind each segment plus a plain-language explanation — render
inline under the exact lines. Adapted for next-hunk from
[smallnest/goal-workflow](https://github.com/smallnest/goal-workflow)'s
`understand` skill (which renders the same review as an HTML page); here the
terminal *is* the report.

**Write every annotation in the human's language** — Chinese when they have
been chatting in Chinese.

## When to use

- The human says `/understand`, "review 这次改动", "解释下新写的代码",
  "看看这次变更做了啥".
- The goal is **understanding + review** of workspace changes that are usually
  fresh and not yet digested. Not a refactor, not a bug hunt.

## When NOT to use

- **You** want approval for **your own** edits — that is the `next-hunk`
  review skill (`--select` / session workflow).
- No changes are detected — say so plainly; never invent annotations.
- The human is not at an interactive terminal — describe the change in chat.

## Workflow

### 1. Determine the range and read the diff

Default range: everything since the merge-base with the main branch —
committed-on-branch, staged, unstaged, **and untracked files** (the HTML
edition always covered untracked files; next-hunk needs the flag explicitly).

```bash
# base: merge-base with the main branch (HEAD when there is none)
base=$(git merge-base HEAD origin/main 2>/dev/null \
  || git merge-base HEAD main 2>/dev/null \
  || echo HEAD)

git diff --stat "$base"                    # which files, how much churn
git ls-files --others --exclude-standard   # untracked (new) files
git diff "$base"                           # the diff to read
```

If the human names a range instead ("just the last commit", "main...feat"),
pass it through: `next-hunk diff HEAD~1`, `next-hunk diff main...feat`.

### 2. Read the changes and plan the annotations

Read every changed file around its hunks (pull in unchanged context when the
delta alone doesn't explain itself), then decide what deserves a note.

**Trace each segment to its requirement.** Real evidence first: commit
messages, `docs/` requirement notes, requirement IDs in code comments, linked
issues. Found one — write it as-is. Only a guess — prefix it 【推测】 (guessed)
so a hunch never disguises itself as a requirement.

**Density:** 1–5 notes per important file. Anchor the segments that carry the
change: new core logic, boundary handling, concurrency/transactions, type or
API pitfalls, SQL/behavior semantics. Never line-by-line narration.

### 3. Open the walkthrough

```bash
next-hunk diff "$base" --include-untracked \
  --focus src/pay/refund.rs:52 \
  --note banner="退款对账:新增按渠道拆分的对账导出" \
  --note src/pay/refund.rs:h1="本文件:退款状态机从轮询改成事件驱动" \
  --note src/pay/refund.rs:52="【需求】expires_at 需按 timestamptz 编码" \
  --note src/pay/refund.rs:52="【解释】Vert.x PG 不支持 Instant,改绑 OffsetDateTime" \
  --note src/pay/refund.rs:h2="【推测】删除旧轮询任务:已被事件订阅取代"
```

Annotation conventions (these replace the HTML edition's structured fields):

- **One fact per note; the prefix is the field.** 【需求】 requirement,
  【解释】 explanation, 【推测】 guessed requirement. Several notes on one
  anchor stack as several `╰─ 💬` rows under the line.
- **Anchor additions and context by new-side line** (`<path>:<line>`, the real
  file line number — the HTML edition's `newNo`). **Pure deletions have no
  new-side line — anchor the hunk** (`<path>:h<n>`).
- **A multi-line segment anchors at its first line**; name the extent in the
  text ("本段 52–58 行").
- **File summary goes on the first hunk** (`<path>:h1`); **whole-change theme
  on `banner=`**; point `--focus` at the single most important line — that is
  where their eye lands first.
- **Each note must fit one terminal row** — notes do not wrap, a long one is
  clipped. Keep it to one clause (roughly ≤ 25 CJK characters after the
  `╰─ 💬` prefix); split into another note instead of writing a paragraph.

A wrong line number fails silently (the note simply does not render), so take
line numbers from the diff you just read, not from memory.

Then summarize in chat, in the human's language: how many files changed, what
the core change is, and the two or three points worth their attention.

## While they read (optional guided tour)

Every open review is a live session, so you can walk the human through the
change instead of firing one shot:

```bash
next-hunk list                            # find the open review
next-hunk navigate src/pay/refund.rs:h2   # scroll them to a hunk
next-hunk comment add --file src/pay/refund.rs --line 140 \
  "这段是本次的核心:幂等键在这里生成" --focus
next-hunk context                         # where are they looking right now?
```

Start the review with `--export json --export-file understand.json` when you
want their questions back: on quit the report carries their `c` comments and
banner text — treat those as follow-up questions, not verdicts.

## The human's keys

- `}` / `{` — next / previous 💬 annotation (this skill's "定位" button)
- `]h` / `[h` — next / previous hunk, wrapping across files
- `Tab` / `h` / `l` — next / previous file; `f` filters the file rail
- `?` — full-screen help; `q` — quit

## Notes

- Read-only: this skill never edits the human's code and writes nothing into
  the repo (unless you opt into `--export-file` — mention the file so it can
  be cleaned up).
- `next-hunk` must be installed (`cargo install --git
  https://github.com/wuxiaobai24/next-hunk`; the binary is also aliased
  `nh`). If it is not on PATH, suggest the install command rather than
  failing silently.
