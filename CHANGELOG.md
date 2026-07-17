# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — Jujutsu (jj) first-class support
- **VCS auto-detect** — walk for `.jj` / `.git`; `vcs = "auto" | "git" | "jj"`
  in config and `--vcs` on `diff` / `show` / `serve` / `inspect` / `filediff`.
  Colocated workspaces default to **jj** under auto (override with `vcs = "git"`).
- **jj adapter** — `diff` / `show` / `serve` / `inspect` use `jj diff --git`
  (and revset ranges) without requiring the gix/git index path. Output re-enters
  the same unified-diff IR (parse + viewport gates unchanged).
- **Scope mapping** — worktree / working-set → `jj diff --git`; `--staged` is
  empty under jj (no index) with a stderr note; `--include-untracked` ignored
  with a note (WC snapshot usually already includes new files).
- **filediff in jj workspaces** — system `diff -u` (no gix object store).
- **Docs** — `docs/VCS.md` (behaviour vs git / hunk); PLAN + ARCHITECTURE + skill
  notes. Integration tests skip when `jj` is not on `PATH`.

### Added — Review state persistence (WXB-6)
- **Persist per-hunk decisions across sessions** — accept/reject (`a`/`r`/`u`)
  state is saved by default under `.git/next-hunk/decisions-<scope>.json`
  (scope: `worktree` / `staged` / `working-set`). Reopening the same worktree
  diff restores decisions; watch/reload still remaps by `path:hN`. Disable with
  `persist_review = false` in config or `--no-persist` on `diff` / `serve`.
  Keys match `--select` / `decision` JSON (`accepted` / `rejected` / `undecided`).
- **Status bar progress** — when review tracking is active:
  `reviewed 12/40 files · 80/210 hunks` (a file is reviewed when every hunk is
  decided).
- **Review hotkeys** — with tracking on (persist or `--select`): `a`/`r`/`u`
  decide the current hunk; outside `--select`, `A` accepts all hunks in the
  current file and jumps to the next unreviewed file; `]u`/`[u` next/previous
  unreviewed hunk; `]U`/`[U` next/previous unreviewed file. `--select` still
  controls quit JSON and its bulk keys (`A`/`R` rest of file, `Ctrl-A`/`Ctrl-R`
  all remaining).

### Added — Multi-worktree session discovery (dogfood P2)
- **`next-hunk list --all-worktrees`** — filter live sessions to worktrees of
  the **current** repository (main + linked `git worktree` checkouts) and list
  known worktree roots that do not yet have a live `serve`. Default `list`
  still scans all live sockets system-wide.
- **`list` marks `(current)`** when a session's worktree matches the cwd, so
  agents can pick among parallel worktree sessions at a glance.
- **Socket hash is per worktree root** (canonical path when available), not the
  shared git common dir — documented as intentional: two linked worktrees can
  `serve` simultaneously without socket collision. Skill + README cover the
  recommended multi-agent layout (one `serve` per worktree, tmux split) and
  how agents should choose a session (`list` / `--all-worktrees` / cwd auto).

### Added — Branch-level base / range review (WXB-8)
- **`diff --base <rev>`** — review the whole branch (and local worktree) against
  a base revision, like `git diff <rev>`. File rail shows +/− relative to that
  base. Same flags on `serve` and `inspect` (`inspect --base origin/main --json`).
  Git uses gix tree-vs-worktree; jj maps to `jj diff --from <base> --to @ --git`
  (merge-base → `heads(::base & ::@)`).
- **`diff --range A..B` / `A...B`** — explicit commit range (same semantics as
  `show`). Prefer this when you want committed-only trees without worktree noise.
- **`--strategy`** — `worktree` | `staged` | `working-set` | `upstream-ahead`
  (vs `@{upstream}`, merge-base style; git only) | `merge-base` (requires
  `--base <branch>` for PR-style fork-point left side). Config: `strategy` +
  optional `base`.
- Large branch diffs still go through the unified-diff IR + viewport path (no
  full-widget fallback). README + skill recommend agents use `--base` after
  finishing a feature branch.

### Fixed — Daily-driver polish (dogfood P1 / WXB-16)
- **Exclude `.next-hunk/` from untracked reviews** — writing project config no
  longer lists `.next-hunk/config.toml` as an undecided untracked file under
  `--include-untracked` / `include_untracked = true`.
- **Illegal config enums fail startup** — typos like `layout = "sidebyside"`
  exit non-zero with the field name and allowed values (`unified|stack|split`,
  etc.) instead of silent fallback + exit 0. Malformed TOML is fatal too.
- **Focus miss warns on stderr** — when `--focus` does not resolve, the TUI
  still shows status `focus not found: …` and now also prints a stderr warning
  before the alternate screen (visible to agents/logs).
- **`--select` bulk decisions** — `a`/`r` already auto-advance to the next hunk;
  new: `A`/`R` accept/reject rest of current file; `Ctrl-A`/`Ctrl-R` accept/
  reject all remaining hunks from the current position. Mid-body `a`/`r` also
  work when the hunk header has scrolled off.
- **Pager parse/fatal errors stay non-zero** — covered by CLI tests (`echo hello
  | next-hunk pager`).

### Fixed — Agent bridge consistency (dogfood P1)
- **`show` / `patch` / `filediff` accept `--focus` / `--note` / `--select`**
  (and `--export-on-quit`), matching `diff`. Agents can point humans at a
  commit range or patch the same way as a worktree review.
- **Non-TTY no longer silently drops agent context** — when stdout is not a
  terminal, `--focus` / `--note` / `--select` exit non-zero with a clear
  message instead of falling back to the inspect summary while discarding
  annotations.
- **`inspect --json`** — headless file/hunk structure (same shape as live
  `next-hunk review`: `file_count`, `stream_len`, `inserts`, `deletes`,
  `files[]` with hunks). Prefer this from skills; no `serve` required.
- **`reload` without `--watch` returns `server error: no reloader…`** —
  `ServerReply::Error` was a newtype `Error(String)` that serde's
  internally-tagged representation cannot serialize, so the accept thread
  failed to write a reply and the client saw `parse reply: EOF`. Error is
  now a struct variant and round-trips correctly.

### Added — Working-set review (dogfood P0)
- **`diff --all` / `scope = "working-set"`** — one command reviews **staged +
  unstaged** local changes (optional untracked still via `--include-untracked`
  / `include_untracked`). Closes the dogfood gap where `diff` missed staged
  files and `diff --staged` missed the worktree. Also on `serve` and `inspect`.
  Default remains worktree-only (`git diff` muscle memory). Config: prefer
  `scope = "worktree" | "staged" | "working-set"`; legacy `staged = true` still
  maps to staged-only. CLI: `--all` / `-a` conflicts with `--staged` / `-s`.
- **File-rail origin marks** — working-set (and single-bucket) reviews tag each
  file as `S` staged / `M` modified / `?` untracked so the human sees which
  git bucket a path came from. Paths with both staged and unstaged edits appear
  twice (once per bucket). `inspect` prints `[S]`/`[M]`/`[?]` next to paths.
  Skill + README point agents at
  `next-hunk diff --all --include-untracked` for a full local review.

### Added — Agent export
- **`export_on_quit` config + `--export-on-quit` CLI** — on TUI quit, optionally
  emit an agent-readable review report: `none` (default) / `json` / `markdown` /
  `both`. JSON is a **superset** of the existing `--select` / `decision` shape
  (`accepted` / `rejected` / `undecided`) plus optional `comments`, `notes`, and
  `banner`. Works **without** `--select` (notes/comments still export; all hunks
  stay undecided until decided). Default `none` keeps `git core.pager` clean.
  Documented in README and `skill/next-hunk/SKILL.md`.

### Fixed — Agent session
- **`list` / `get` `repo` field was the first review file path** — `ServerReply::Info.repo_path`
  incorrectly used `review.files[0].new_path` (e.g. `src/main.rs` or
  `.next-hunk/config.toml`) instead of the worktree root known at `serve`
  startup. Agents relying on `repo=` to select a session among worktrees got
  misleading values. `Info` now reports the absolute repo/worktree root passed
  into the TUI as `workdir`. Skill examples updated to match.

### Added — Configuration
- **`line_numbers` config now actually works** — setting `line_numbers = false` in
  `.next-hunk/config.toml` (or the user-level config) hides the line-number
  gutter at startup. Previously the field was parsed but silently ignored; the
  gutter was always on. `#` still toggles it at runtime.
- **`include_untracked` config & `--include-untracked` CLI flag** — untracked
  files now appear in the worktree diff review when enabled. Off by default
  (safe). Config key: `include_untracked = true` in `.next-hunk/config.toml`.
  CLI flag: `next-hunk diff --include-untracked`. The `serve` subcommand also
  accepts the flag. Untracked files are rendered as new-file additions from
  `/dev/null`.
- **`wrap` config for long-line behavior** — set `wrap = true` in
  `.next-hunk/config.toml` to wrap long lines in the diff stream pane
  (default `false`, truncates). Ratatui's `Paragraph::wrap` is used so no
  viewport or IR changes were needed — wrapping is a presentation-layer
  property applied during rendering.
- **Stack layout mode (`layout = "stack"`)** — alternative diff presentation
  that shows old content (context + deletes) then new content (context + adds)
  in two stacked blocks per file, separated by `▌ old` / `▌ new` labels. Set
  `layout = "stack"` in `.next-hunk/config.toml` (default is `"unified"`).
  Falls back to unified when the terminal is narrower than 40 columns.
- **True side-by-side split layout (`layout = "split"`)** — left pane shows
  old content (context + deletes), right pane shows new content (context +
  adds), with consecutive delete/add runs paired on the same visual row. Set
  `layout = "split"` in config, or pass `--layout split` on `diff` / `show` /
  `patch` / `filediff` / `serve`. Responsive fallback: stream pane `< 80` cols
  → stack, `< 40` cols → unified. Presentation-only — still viewport-only
  materialization via `ViewportQuery::rows`; IR/scroll/search indices unchanged.

### Added — TUI
- **`zc`/`zo` fold/unfold current file** — `zc` (close fold) collapses the
  current file's body so only its header is visible; `zo` (open fold) expands
  it. Follows the vim-style two-key prefix pattern (`z` waits for `c`/`o`),
  same as `]h`/`[h` for hunk jumps. Fold state is preserved across scroll and
  file switches.
- **Syntax highlight follows the UI theme** — light mode now uses the
  `base16-ocean.light` syntect theme instead of being stuck on a dark syntax
  palette. Dark mode keeps `base16-ocean.dark`. Auto mode selects the matching
  theme via `$COLORFGBG`. Config-driven theme switching (`t` key or `theme`
  config) also swaps the syntax theme.
- **`next-hunk filediff <old> <new>`** — diff two arbitrary files on disk using
  gix's diff engine and review them in the TUI. Works both inside and outside
  git repositories (requires a containing repo for the object store). Relative
  paths are resolved against the repo worktree root.

### Added — Agent session (serve protocol v2)
- **`next-hunk list` / `next-hunk get [hash]`** — discover and inspect live
  server sessions. `list` scans `$XDG_RUNTIME_DIR` and `/tmp` for next-hunk
  sockets, probes each for liveness, and prints session info (hash, path,
  file count, repo). `get` shows details for a specific session by hash or
  defaults to the current repo's socket. Requires the `serve` feature on Unix.
- **`next-hunk review [hash]`** — print the current review's file/hunk structure
  as JSON (no full patch text by default). Connects to a running serve session
  and dumps file paths, insert/delete counts, and hunk ranges. Useful for
  agents to understand the review structure before deciding what to focus on.
  Requires the `serve` feature on Unix.
- **`next-hunk navigate <target> [--hash <hash>]`** — navigate a running serve
  TUI to a file, hunk, or line. Target syntax: `<path>`, `<path>:<line>`, or
  `<path>:h<n>` (1-based hunk ordinal), matching the `--focus` convention.
  Uses the same `FocusTarget` → `apply_focus` path as `--focus` and `push`.
- **`next-hunk comment <add|list|rm|apply>`** — manage comments on a running
  serve session. `comment add --file PATH [--line N] [--hunk N] <text>` adds a
  comment (returns an id). `comment list` lists all comments. `comment rm <id>`
  removes one. `comment apply` pushes comments into the TUI as note annotations.
  Uses the existing note-rendering infrastructure. Requires the `serve` feature
  on Unix.
- **`next-hunk reload [--hash <hash>]`** — re-fetch the diff content of a
  running serve session and refresh the review, preserving focus/notes/decisions
  best-effort via the existing `App::reload_review` path. Requires the serve to
  have been started with `--watch` (or a reloader). Requires the `serve` feature
  on Unix.
- **`next-hunk push` / `next-hunk decision`** (existing, extended) — push
  focus/notes and poll decisions. `decision` returns the same JSON shape as
  `--select` quit output, non-blocking.
- **Agent skill updated** (`skill/next-hunk/SKILL.md`) — the agent skill now
  documents the complete session workflow: serve → list → review → navigate →
  comment → apply → decision → reload, matching the shipped CLI commands.

### Added — Documentation
- **PERF.md vs-delta comparison** — a design-level comparison table documenting
  next-hunk's parse latency (~1.4 ms for 1.1 MB), viewport materialization
  (~200 ns single, ~340 µs for 1000 random starts), and architecture differences
  vs `delta`. Real bench numbers from `cargo bench` on AMD Ryzen 7 5700X.
- **README synced** — Status section, config table, keybindings, and usage
  examples updated to cover all 0.4–0.5 features.

### Fixed
- **Reload now preserves decisions, folds, notes, and focus** — `App::reload_review`
  (used by `--watch` and `next-hunk reload`) now re-maps per-hunk decisions and
  file-folds by display path so they survive content refresh. Notes and focus
  targets are preserved unchanged. Decisions for hunks that no longer exist in
  the refreshed diff are silently dropped.
- **Status/hints updated for fold keys** — the startup status line, help
  overlay, and bottom help bar now mention `zc`/`zo` fold/unfold keys.

### Internal
- **Generation-id highlight cache** — `HighlightCache` now tags each entry
  with a generation id. `invalidate()` bumps the gen and clears the map.
  New `try_get()`/`try_insert()`/`apply_result()` methods allow safe
  coexistence with background highlight work: a background fill that
  completes after invalidation can check the gen and skip inserting stale
  results.
- **Async syntax highlight worker** — live TUI spawns a dedicated
  `HighlightWorker` thread. Viewport cache misses render plain text and
  enqueue a job; the main loop drains results each frame and inserts only
  when the snapshot gen still matches. Headless tests keep the sync
  `get_or_highlight` path (no job channel). Theme toggle (`t`) reloads
  the syntect palette into a fresh `Arc<Highlighter>` and invalidates
  the cache so in-flight jobs stay gen-safe.

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


[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.3.0
[0.2.1]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.1
[0.2.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.0
[0.1.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.1.0
