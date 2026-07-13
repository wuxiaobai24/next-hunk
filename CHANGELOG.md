# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1] - 2026-07-13

### Added — Reviewer experience
- **Flexoki theme** — both palettes (`light` / `dark`) now use the
  [Flexoki](https://flexoki.com) color system via exact RGB values. The default
  theme is now **light** (Flexoki paper); `t` still cycles light → auto → dark.
  The previous light palette was nearly unreadable on white terminals (it used
  the pale `Light*` ANSI variants meant for dark backgrounds).
- **`?` help overlay** — a full-screen keybinding reference (`?` toggles;
  `Esc`/`q`/`Enter`/`Space` dismiss). Previously `?` did nothing.
- **`Space`** — quick next-hunk jump (single-key alias for `]h`).
- **`b`** — toggle the file-rail sidebar.
- **Mouse clicks** — click a file in the rail to select it; click the stream to
  position the viewport on that row (wheel scroll unchanged).

### Fixed
- **Rail highlight now follows `K`/`PageUp`** — paging up no longer left the
  file-rail selection stuck on the old file. (`sync_selected_file` was missing
  on that one scroll path.)

### Removed
- **`s` (split layout)** — never implemented (the key silently rendered unified
  while claiming "split layout"). Removed the key, the `ViewMode` plumbing, and
  its help/docs entry. Side-by-side split remains on the roadmap.
- **`wrap_lines` config field** — accepted in `config.toml` but never rendered.
  Removed so config no longer advertises a no-op (line wrapping needs a viewport
  model change; tracked separately).

## [0.2.0] - 2026-07-13

### Added — Agent ↔ Human review bridge

next-hunk now bridges a coding agent's changes to the human reviewer. The agent
calls the CLI; the human gets an interactive TUI pointed at what matters.

- **`--focus <path>[:<line>|:h<n>]`** (`diff`) — scroll the TUI to a file, line,
  or hunk on startup, so the human lands where the agent wants their attention.
- **`--note <target>=<text>`** (`diff`, repeatable) — agent annotations rendered
  in the TUI: `<path>:<line>=<text>` shows under that line, `<path>:h<n>=<text>`
  under a hunk header, `banner=<text>` in the status bar. Rendered via a
  viewport fan-out that leaves `stream_len` / `hunk_starts` / search indices
  untouched.
- **`--select`** (`diff`) — per-hunk approval gate. The human presses `a`
  (accept) / `r` (reject) / `u` (undecided); on quit the decisions are emitted
  as JSON on stdout for the agent to parse:
  `{"accepted":[...],"rejected":[...],"undecided":[...]}`. Hunk keys are
  `"<path>:h<n>"` (1-based). Requires an interactive terminal (errors clearly
  otherwise, so an agent scripting it gets an unambiguous signal).
- **Agent skill** (`skill/next-hunk/SKILL.md`) — a ready-made skill that
  teaches a coding agent when and how to call next-hunk.

### Added — Internals

- `ViewportQuery::file_index_for_path` / `hunk_start_row` / `row_for_new_line`
  — forward-resolve helpers (path→file, (file,hunk)→row, (file,line)→row) that
  the agent-bridge features build on.
- `StreamRow::HunkHeader` now carries `hunk_idx`, threaded through to the
  renderer for `--select` markers and hunk-level `--note` targeting.

### Added — Server mode (persistent TUI + live push)

Optional **server mode** lets an agent stream multiple updates into a single
persistent review TUI and read the human's decisions in real time, without
re-launching a process per interaction.

- **`next-hunk serve`** — opens a persistent review TUI (with select mode on)
  that also listens on a Unix socket derived from the repo root. Supports
  `--watch`, `--focus`, `--note`, and pathspecs like `diff`.
- **`next-hunk push --focus … --note …`** — sends a focus/note update into the
  running `serve` in this repo; returns immediately with `ok`.
- **`next-hunk decision`** — reads the human's accumulated per-hunk decisions
  from the running `serve`, printed as one JSON line on stdout (same shape as
  `--select` quit output). Returns immediately; does not wait for the human to
  quit.
- The socket path is deterministic per repo (`runtime_socket_path`), so
  `push`/`decision` find the server automatically — no `--socket` flag.
- Gated behind the `serve` feature (on by default) on Unix; on other builds the
  subcommands report unavailability at runtime, mirroring the `watch` feature.

## Distribution

As of 0.1.0, **next-hunk is distributed via GitHub**:

```bash
cargo install --git https://github.com/wuxiaobai24/next-hunk
```

It is intentionally **not** published to crates.io yet (GitHub-only distribution
chosen for the first release). A crates.io publish may happen in a later release.

### Static binary

Tagged releases also publish a **fully static, all-features** x86_64 musl
binary (single ~2.6 MB xz file, no runtime deps — runs on Alpine/distroless/old
glibc). Built automatically by the `release` workflow on every `v*` tag. See
the README "Prebuilt static binary" section.

## [0.1.0] - 2026-07-12

First usable release — a terminal review engine for large changesets.

### Added
- **Virtualized multi-file TUI** (ratatui): only visible rows are materialized,
  so 1 MB+ diffs scroll in constant time.
- **Compact unified-diff IR** with a binary-searched file/hunk index; parse
  tolerates renames, binary placeholders, no-newline-at-eof, and CRLF.
- **Git backends** via `gix`: working-tree diff, `--staged`, `git show <rev>`,
  plus `patch -` / file input for review sources.
- **Syntax highlight** (syntect, pure-Rust) cached per file, viewport-only.
- **Navigation:** `]h`/`[h` next/prev hunk (wraps files), `Tab`/`h`/`l` file
  rail, `/` content search, `f` path filter, `g`/`G` top/bottom.
- **`o` open in editor** — jump to the focused line in `$EDITOR`.
- **Ignore-whitespace toggle** (`W`) — collapse whitespace-only changes.
- **Watch mode** (`--watch`) — live-reload on file change, preserving
  scroll/selection. (Now a default feature.)
- **Pager mode** (`next-hunk pager`) — drop-in `core.pager` for git so
  everyday `git diff`/`show`/`log -p` launch the TUI.
- **Diff stats** in the status bar (per-file + total `+ins/−del`).
- **Themes** dark/light/auto (`t` cycle; `auto` reads `$COLORFGBG`), via
  `config.toml`.
- **Line-number gutter** (`#`), **word-level inline diff** (`w`), and
  **unified/split layout** (`s`).
- `inspect` subcommand for headless IR summaries (scripting).
- Benchmarks for parse + viewport materialization.
- MIT license, CHANGELOG, and CI (fmt, clippy `-D warnings`, cross-OS tests,
  no-default-features build).

### Performance
Measured on the bundled fixtures (release build):

| bench | input | median |
|---|---|---|
| `parse/huge` | 1.1 MB / 38k-line diff | **~1.3 ms** |
| `parse/medium` | 191 KB / 6.5k-line diff | ~150 µs |
| `parse/small` | 6 KB / 213-line diff | ~5 µs |
| `viewport_huge_h40` | materialize a 40-row window over the huge diff | **~300 µs** |
| `viewport_single_h40` | resolve a file span + clip to viewport | **~190 ns** |

Parsing a megabyte-scale diff in single-digit milliseconds and rendering a
viewport in sub-microsecond to sub-millisecond range is what makes the tool
stay responsive on changesets that stall other viewers.

[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.1.0
