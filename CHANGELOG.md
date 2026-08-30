# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — inline agent notes (`--note` / serve comments)
- **Notes render attached to their code, not as orphan rows.** When the
  terminal has room, a row's notes appear as a right-aligned inline
  annotation (` 💬 text`) on the same rendered row — code line or hunk
  header, unified/split alike. When the code line is long or the terminal
  is narrow (or `wrap` is on), the note falls back to a dedicated
  `▎ 💬 text` row directly below its target. Multiple notes on one row
  join with ` · ` inline, or stack as rows in the fallback.
- **`}` / `{` jump between annotated rows** (code lines / hunk headers
  carrying notes), mirroring `]h`/`[h` hunk jumps: wrap-around across
  files, status shows the ordinal (`💬 note 3/7`). Listed in the `?` help.
- **Per-file note counts in the file rail** — a `💬N` badge next to the
  `+ins/−del` tally shows where agent attention sits; `}`/`{` walks it.
- **`comment apply` is now idempotent per comment** — re-running it (e.g.
  after adding more comments) only converts comments not yet applied,
  instead of duplicating earlier note rows. Applied note text carries the
  comment id (`c1: …`) for `comment rm` correlation; the 💬 glyph comes
  from the renderer, not the text.

### Added — `nh` short alias binary
- **`nh`** — same program as `next-hunk`, shorter to type (`nh`, `nh diff -s`).
  Usage/error output brands itself after argv[0], so `nh --help` says `nh`.
  The CLI implementation moved into the library (`next_hunk::cli`); both
  binaries are thin shims. `scripts/install.sh` now also creates an `nh`
  symlink next to the installed binary.
- **Replace `git diff`** — documented two routes: a git alias
  (`git config --global alias.d '!nh diff'` → `git d`, `git d -s`,
  `git d main...feat`) or `core.pager "nh pager"`.

### Added — `diff [target]` (git-diff parity)
- **`nh diff main`** — that tree vs the worktree (like `git diff main`);
  with `-s/--staged`, that tree vs the index (like `git diff --cached main`).
- **`nh diff A..B` / `A...B`** — two-tree range diff (merge-base semantics
  for `...`), same engine as `show` ranges.
- **Pathspec disambiguation** — a first positional that doesn't resolve as a
  rev but exists on disk falls back to a pathspec (with a stderr note);
  otherwise a git-style `unknown revision or path not in the working tree`
  error. Pathspecs after `--` (or trailing) still limit any diff form.
- `--watch` reload re-runs the same target diff.

### Changed — feedback & discoverability
- **Severity-colored status messages.** The single dim status string is now a
  typed toast: errors render red (bold), confirmations green, navigation/neutral
  feedback stays dim. Errors and successes no longer look identical.
- **Transient toasts auto-expire.** A status message clears itself after a few
  seconds idle (info/success ~4s, errors ~8s) instead of lingering on screen
  until the next keypress. The startup hint is sticky and never auto-clears.
- **Run-mode badges in the status bar.** `--select`, `serve`, and `--watch`
  now show a `[SELECT]` / `[SERVE]` / `[WATCH]` badge so the active mode is
  visible at a glance, not just inferable from transient status text.
- **Context-aware hint line in `--select` mode.** The bottom cheatsheet leads
  with the decision keys (`a accept · r reject · u undecided`) — previously
  these were only discoverable via the `?` overlay despite being the primary
  keys in select mode.
- **Graceful hint/prompt truncation.** On narrow terminals the help line and
  the `/`/`f` prompts truncate with a trailing `…` instead of being silently
  clipped, so it's clear there is more (and `?` shows the full reference).

### Fixed
- Help overlay listed the theme cycle order as `light → auto → dark`; the
  actual order (`dark → light → auto`, per `ThemeMode::cycle`) is now shown.

### Fixed — CHANGELOG integrity (trust repair)
- **0.4.0 release notes rewritten to match shipped code.** The previously
  published 0.4.0 section described features that do not exist anywhere in
  the tagged `v0.4.0` tree — among them `--structural` (difftastic),
  `docs/PLATFORMS.md`, incremental IR reload, `layout = "auto"`, an `overlay`
  subcommand, an MCP server, catppuccin/tokyonight theme presets,
  `last-export`, headless `--export-on-quit`, Jujutsu support, review-state
  persistence, `diff --base/--range`, and multi-worktree session discovery —
  plus distribution claims (four-platform artifacts, Homebrew formula,
  crates.io publishing) that do not match `.github/workflows/release.yml`.
  These entries were written from a planning document as if already
  implemented and shipped without verification. They are removed; the 0.4.0
  section now lists what `v0.4.0` actually contains, reconstructed from
  `git log v0.3.0..v0.4.0`. Sections 0.3.0 and earlier were verified against
  code and are unchanged. The real Unreleased entries above are untouched.


### Added — Context collapsing (`··· N unchanged lines ···`)
- **Unchanged context folds to marker rows** — long runs of context lines
  *within* a hunk and the unchanged gap *between* consecutive hunks
  (implied by `@@` line numbers, so it works on bare patch input) render as
  one dim `··· N unchanged lines ···` marker. Default on with threshold 8;
  `context_collapse = N` configures (0 disables), `zx` toggles at runtime.
- **Virtual-row scroll model** — `scroll_y` now indexes the collapsed
  (virtual) stream, so every scroll position is exactly one drawn row:
  scrolling never walks through invisible space, and `max_scroll` /
  status-bar totals shrink when content is collapsed or files are folded.
  Materialization stays viewport-only via a binary-searched segment table
  (`ir::collapse`); navigation (`]h`/`[h`, file jumps, search, `--focus`)
  funnels through one stream→virtual mapping, and a jump into a collapsed
  run expands just that run so search hits are always visible.
- **Fold interplay fixed** — folding a file now genuinely compacts the
  stream (previously folded bodies still occupied scroll range and windows
  "pulled through" later files' rows).

### Added — Side-by-side split layout + `layout = "auto"`
- **`layout = "split"`** — old/new content in two aligned half-width columns
  with per-side line-number gutters, colored signs, and true-color syntax
  highlighting on both sides; delete/add runs pair index-wise with blank
  padding on the shorter side. Inter-hunk gap markers still apply; file /
  hunk headers and `--note` rows span both columns.
- **`layout = "auto"`** — responsive layout picked at draw time from the
  live stream-pane width: ≥ 120 cols → split, ≥ 40 → stack, else unified.
  Width changes rebuild the virtual index (pair rows vs line rows) and
  re-anchor the scroll on the same stream row; the IR is untouched.
- Both modes work in `diff` / `show` / `patch` / `pager` / `serve` (config
  key; `patch` now honors config like the repo-backed commands).

### Removed
- **`mcp` cargo feature** — the flag was declared in `Cargo.toml` with a
  comment describing an MCP stdio server (`next-hunk mcp`), but no such
  subcommand or gated code ever existed (`feature = "mcp"` had zero uses in
  `src/`). Removed so the build surface stops advertising a phantom feature.

## [0.4.0] - 2026-07-17

Layout & reading experience, agent session CLI, and async syntax highlight.

### Added — Reading experience
- **Stack layout** — `layout = "stack"` config: old content (context +
  deletes) then new content (context + adds) per hunk; `unified` stays the
  default.
- **File fold/unfold** — `zc` closes the focused file to a single header row,
  `zo` opens it. Folds survive watch/reload.
- **`wrap` config** — `wrap = true` wraps long diff lines instead of the
  default truncation.
- **Highlight follows theme mode** — syntax theme switches with the UI
  light/dark mode (`t`), instead of always using a dark syntax theme.

### Added — Async syntax highlight
- **Background highlight worker** — highlighting runs off the UI thread with
  a generation id; results from a stale generation are rejected on arrival
  and the row renders plain until fresh styles land. Scroll latency no
  longer depends on syntect.
- **Generation-id highlight cache** — cached styles are keyed by generation;
  stale entries cannot be applied to a new review state.

### Added — Agent session CLI (serve v2)
- **`next-hunk list` / `get`** — discover live serve sessions over the
  session socket and dump one session's state.
- **`next-hunk review --json`** — file/hunk structure of the live review
  (paths, hunk ranges, stats; no full patch text).
- **`next-hunk navigate`** — drive the human TUI to a file / hunk / line.
- **`next-hunk comment add|apply|list|rm`** — comment CRUD on the live
  session, rendered as note rows in the TUI.
- **`next-hunk reload`** — swap the reviewed diff/show content in place;
  decisions, folds, notes, and focus are preserved (remapped by `path:hN`).

### Added — Sources & config
- **`include_untracked`** — config key + `--include-untracked` flag to add
  untracked files to the worktree diff (off by default).
- **`next-hunk filediff <old> <new>`** — diff two arbitrary files on disk.
- **`line_numbers` config wired** — the key now reaches the TUI gutter
  (previously parsed but silently ignored); runtime `#` toggle unchanged.

### Testing
- **Huge-fixture gate test** — parse + viewport over the bundled 1.1 MB /
  38k-line fixture keeps the perf contract under `cargo test`.

### Docs
- `docs/PERF.md` gains a rough latency/memory comparison vs delta; the agent
  skill is rewritten around the session v2 workflow (list → review →
  navigate → comment); README status/config/keybinding tables synced.

## [0.3.0] - 2026-07-14

### Changed — Search & filter
- **Inline search-match highlighting** — the active search match now shows
  *where* in the line the hit is, instead of painting the whole line one
  color. Every occurrence of the query on the current match row gets the gold
  active style; the rest of the line keeps its syntax color under a subdued
  background. Other match rows keep their whole-line subdued bg.
- **Ctrl-U / Ctrl-W in the search and filter prompts** — Ctrl-U clears the
  whole input, Ctrl-W deletes the trailing word (readline
  backward-kill-word semantics, including the preceding whitespace). Unix
  users reach for these instinctively; previously they got appended as
  literal characters.

### Fixed — Reviewer experience
- **Status bar no longer overflows on long paths** — the focused file's path
  in the status bar is now capped at ~half the terminal width and truncated
  toward the basename (e.g. `…/file.rs`), so a deeply-nested path can no
  longer push the diff totals, banner note, and status message off the right
  edge of a narrow terminal. Previously the full path was laid out
  unconditionally, which on a long path swallowed the rest of the status row.

### Added — Reviewer experience
- **Per-file change tally in the file rail** — each file in the left rail now
  shows a compact `+ins` (green) / `−del` (red) tally, right-aligned next to the
  path, with zero sides omitted (an add-only file shows `+12`, a pure delete
  `−3`). Lets the reviewer spot where the change mass sits at a glance, instead
  of having to scroll into each file to see its stats. The status bar's
  per-file/total tallies are unchanged.

### Added — Navigation
- **Vim/less-style page keys** — `Ctrl-D`/`Ctrl-U` scroll half a page
  down/up, `Ctrl-F`/`Ctrl-B` scroll a full page. Mirrors the existing
  `J`/`K` (half-page) keys with the muscle-memory Ctrl variants.
- **`1`–`9` jump to the Nth file** — a direct shortcut for large multi-file
  diffs where Tab-cycling to a far-down file is tedious. Out-of-range
  numbers are a no-op.
- **`n`/`N` now report why nothing moved** — pressing them with no active
  search (or with a search that has no matches) sets a status hint instead
  of being silent (which read as a broken keybind).

### Added — Development
- **`pre-commit` git hook (auto-installed)** — running `cargo test` in a fresh
  clone now installs a `pre-commit` hook (via [cargo-husky](https://github.com/rhysd/cargo-husky))
  that runs `cargo fmt --check` and `cargo clippy -- -D warnings` before each
  commit, so formatting/clippy drift is caught locally instead of turning the
  CI `rustfmt`/`clippy` jobs red. Bypass with `git commit --no-verify`.

### Added — Distribution
- **One-click install script** (`scripts/install.sh`) — `curl | bash` installer
  that resolves the latest Release, downloads the static musl binary, verifies
  its sha256, and installs to `/usr/local/bin` (or `~/.local/bin` if not
  writable). Falls back to `cargo install --git` on platforms without a
  prebuilt binary (macOS, aarch64). Flags: `--prefix`, `--bin-dir`,
  `--version`, `--as-pager`, `--force`, `--no-verify-checksum`.

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


[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.4.0
[0.3.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.3.0
[0.2.1]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.1
[0.2.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.0
[0.1.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.1.0
