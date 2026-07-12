# next-hunk Architecture

**English** | [中文](./ARCHITECTURE_zh.md)

## 0. Product position

**next-hunk** is a high-performance terminal **review engine** for large changesets.

The goal is to step past pager-style tools and heavy JS/TS TUI runtimes on four pillars:

| Pillar | Commitment |
|--------|------------|
| **Performance** | Viewport-only rendering, compact runtime IR, hard bench gates |
| **Scale** | Multi-file review streams without materializing every row as a widget |
| **Experience** | Interactive multi-file navigation, readable layout, scriptable CLI |
| **Agent-era** | Terminal-native, scriptable for humans *and* coding agents |

One-liner:

> Review huge diffs faster — built for agent-era workflows.

### Design correction (important)

**Binary size is not a product goal.** Early drafts elevated “small binary / musl static / ban libgit2” to a pillar. That was wrong:

| We actually care about | Not a product goal |
|------------------------|--------------------|
| **Scroll latency, startup latency, RSS** on large diffs | Whether the stripped binary is &lt; 15 MB |
| Compact **runtime IR** (no full widget tree) | Minimizing dependency count for its own sake |
| Correct, extensible source / highlight | Rejecting good libraries to stay “static-friendly” |

Tech choices optimize for **correctness, maintainability, and hot-path performance** — not for looking lean on disk.

---

## 1. Goals and non-goals

### 1.1 Goals

- **Fast**: hard numbers for cold start, scroll, and file switch
- **Large**: tens of thousands of diff lines without OOM or multi-second stalls
- **Clear**: data plane (IR) strictly separated from UI; scriptable and measurable
- **Usable**: multi-file rail + continuous stream, clear keybindings
- **Useful to agents**: terminal-native, scriptable text output that drops into any pipeline

### 1.2 Non-goals (at least for early 0.x)

| Non-goal | Why |
|----------|-----|
| Pixel-perfect clone of another TUI | Pulls us off the review-engine focus |
| Full git client (stage / commit / rebase suite) | That is lazygit / gitui territory |
| Syntax highlight on by default for whole files | Kills scroll budget (async, cancellable, viewport-only is fine) |
| jj / Sapling day-one parity | Adapter later |
| Session daemon / MCP as core | Complexity explosion |
| Binary size as a success criterion | **Not a requirement**; release size is observational only |

---

## 2. System architecture

```text
┌──────────────────────────────────────────────────────────────────┐
│                         CLI (clap)                                │
│   diff | show | patch | bench                                     │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                    Source adapters (lazy, swappable)              │
│   gix (sole git backend)  │  patch file / stdin  │  two-files          │
└────────────────────────────┬─────────────────────────────────────┘
                             │ unified text / blob refs
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                      Diff IR (core asset)                         │
│  text_arena + File[] + Hunk[] + line spans                        │
│  stream_len + per-file stream ranges                              │
│  FORBIDDEN: full Vec<WidgetRow> for entire review                 │
└───────────────┬─────────────────────────────┬────────────────────┘
                │                             │
                ▼                             ▼
┌───────────────────────────┐   ┌──────────────────────────────────┐
│ Viewport query            │   │ Side services (cancellable)      │
│ O(visible ± overscan)     │   │ highlight │ search │ watch      │
│ file_at_row / rows()      │   │ generation id invalidates work   │
└─────────────┬─────────────┘   └──────────────────────────────────┘
              │
              ▼
┌──────────────────────────────────────────────────────────────────┐
│ TUI (ratatui) — immediate / sparse                                │
│  file rail  │  continuous stream  │  status / help                │
│  input path: sync and short; no await on scroll hot path          │
└──────────────────────────────────────────────────────────────────┘
```

### 2.1 Layer responsibilities

| Layer | Responsibility | Must not |
|-------|-----------------|----------|
| **CLI** | Args, subcommands, exit codes | Own business parsing |
| **Source** | Produce unified diff / raw bytes | Know about TUI |
| **IR** | Single source of truth: indices + compact line data | Hold Color / Style / Widget |
| **Viewport** | Materialize `StreamRow` for a window | Full-table scan on every frame (except index build) |
| **TUI** | Draw, keys, scroll state | Own a second full-line cache |
| **Services** | Highlight, search | Block the UI thread |

### 2.2 Diff IR (sketch)

```text
Review {
  text_arena: String,              // shared storage for line/header text
  files: [FileDiff],
  stream_len: usize,               // virtual row count
  hunk_starts: [usize],            // abs stream rows of every hunk header → binary search for ]h/[h
}

FileDiff {
  display_path,
  hunks: [Hunk],
  stream_start, stream_len,        // range in global stream → binary search
}

Hunk { header_span, old/new range, lines: [DiffLine] }
DiffLine { kind: Context | Add | Delete | Meta, text_span }
```

**“Compact IR” means the runtime memory model**, not the release package: one arena + spans, not a styled row structure for the entire review.

**Flattened stream order** (must match TUI):

```text
[FileHeader] [HunkHeader] [Line...] [HunkHeader] [Line...] ... next file ...
```

### 2.3 Performance principles

1. **Index ≠ content** — spans first; bytes on demand  
2. **Viewport is the only hot path** — decorations only for visible ± overscan  
3. **Background work is cancellable** — highlight carries a `generation`; scroll drops stale work  
4. **Sync path stays short** — key → mutate scroll/focus → draw  
5. **Default is fast** — highlight / word-diff opt-in or idle-fill  
6. **Measurement-driven** — fixtures + benches; regressions fail CI  
7. **Git via gix** — in-process repo access and diffs; no `git` CLI subprocess and no fallback

### 2.4 Repository layout

```text
next-hunk/
  Cargo.toml
  README.md
  docs/
    ARCHITECTURE.md      # this file
    PERF.md              # metrics, fixtures, gates
  src/
    main.rs              # CLI entry
    lib.rs               # library root: ir / source / tui …
    ir/                  # model, parse, viewport
    source/              # git, patch, files
    tui/                 # app, rail, stream, keys, watch
    config.rs            # layered config.toml (user + project)
    highlight/           # later: async syntect
  benches/
  fixtures/
  scripts/
```

Single package first (lib + bin); split crates only if boundaries hurt compile times or reuse.

---

## 3. Tech choices (0.x direction)

| Concern | Choice | Notes |
|---------|--------|--------|
| Language | **Rust** | No GC; controllable memory and latency |
| TUI | **ratatui + crossterm** | Stable; revisit only if the framework is the bottleneck |
| Line-level IR | **Custom unified-diff parser** | Full control of layout/perf |
| Word-level diff | **similar** (later, viewport-only) | |
| Git | **gix (gitoxide)** | Sole backend; no CLI fallback |
| Highlight | **syntect** or equivalent (Phase 4, async) | Off or idle by default; dependency size is not a gate |
| CLI | **clap** | |
| Errors | **anyhow** / **thiserror** | |
| Release | Normal release profile; size observational | See PERF.md |

Principle: **capability and hot-path performance first**. Do not treat “few deps / tiny binary / fully static” as architecture constraints.

---

## 4. Competitive position

```text
          lightweight pager                 full git client
                │                                 │
     delta ─────┼────────────────── gitui/lazygit ─┤
                │                                 │
                │      ★ next-hunk                │
                │      review engine              │
                │      large diffs                │
                │      measurable latency         │
```

- **vs delta**: interactive multi-file review on huge diffs
- **vs gitui/lazygit**: refuse the full client surface; stay specialized on review and stay faster on huge diffs  

(**Binary size is not a competitive KPI.**)

---

## 5. Development plan

### Phase 0 — Baseline and skeleton (~0.5 week)

- [x] Name: `next-hunk`
- [x] Repo skeleton + this document
- [x] `docs/PERF.md` + fixture policy
- [x] IR parse + unit tests (library entry wired)
- [ ] Optional one-shot numbers vs delta (or similar) when useful

**Exit:** patch parses; docs exist; bench entry point defined.

### Phase 1 — Engine is falsifiable (~1 week)

- [x] Robust unified parse (rename, binary placeholder, no newline)
- [x] `ViewportQuery` with binary search on file spans (initial version exists)
- [x] Sources: gix worktree / staged / show / range (patch file / stdin via CLI)
- [x] Benches: parse + viewport materialization

**Exit:** huge fixture parses without OOM; random viewport queries meet gates in PERF.md.

### Phase 2 — TUI MVP (~1–1.5 weeks)

- [x] ratatui: left file rail + right continuous stream
- [x] Virtual scroll (`scroll_y` + viewport)
- [x] Keys: j/k, next/prev file, g/G, q, Tab
- [x] Status line: file count / position / mode
- [x] Friendly empty-diff and non-git errors
- [x] `next-hunk` / `next-hunk diff` interactive by default

**Exit:** daily-driver for working-tree review on medium fixtures; huge opens and scrolls (highlight optional).

### Phase 3 — Product completeness (~1–2 weeks)

- [ ] `show` / `patch -` fully wired
- [ ] staged + passthrough git args
- [ ] Optional two-file diff
- [x] Light search (path filter and/or in-stream `/`)
- [ ] Minimal config (colors, rail width)

**Exit:** replaces “delta + manual file hopping” for the main path.

### Phase 4 — Differentiation (~2–3 weeks)

- [ ] Simple local notes (line/file)
- [x] Syntax highlight (syntect, viewport-only + cached, default on)
- [ ] Async syntect (cancellable, default off or idle) — current highlight is sync viewport-only
- [ ] Word-level diff **viewport-only**
- [ ] Public compare notes (vs delta or similar: latency / RSS, not binary size)

**Exit:** story upgrades from “fast” to “agent-era review engine”.

### Phase 5 — Hardening (ongoing)

- [ ] Side-by-side (own perf design; never default hot path without gates)
- [ ] Watch / incremental IR refresh
- [ ] jj adapter
- [ ] Themes, help overlay
- [ ] Fuzz parse; more real-repo regressions

---

## 6. Process rules

```text
Weekly rhythm (suggested):
  early week: engine / correctness / benches
  late week:  TUI / UX
  end of week: run fixture gates; update PERF numbers

PR rules:
  - IR / viewport changes require tests or benches
  - Scroll hot path: no await, no full-file highlight
  - New features default off or stay off the hot path until measured
  - Do not reject reasonable dependencies to shrink the binary
```

---

## 7. Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Feature-chasing every git TUI | Locked non-goals; stay specialized on review |
| UI before virtualization | Phase 1 gates block Phase 2 polish |
| Extreme line length | Truncate + horizontal scroll later; protect vertical scroll first |
| Old terminals | Real-host TERM matrix; success is not defined by musl |
| Parse edge cases | Golden patches + later fuzz |
| Size mistaken for a goal | “Design correction” above + no binary gate in PERF |

---

## 8. Success looks like

| Stage | External one-liner |
|-------|--------------------|
| Phase 2 | “`next-hunk` scrolls huge diffs smoothly” |
| Phase 4 | “Measurable latency, smooth interactive review on huge diffs” |
| Long term | Category: **review engine**, not “yet another git TUI”, and not “smallest diff binary” |

---

## 9. Related docs

- [PERF.md](./PERF.md) — fixtures, metrics, CI gates ([中文](./PERF_zh.md))
- [../README.md](../README.md) — user-facing overview ([中文](../README_zh.md))
