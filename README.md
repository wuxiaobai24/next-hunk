# next-hunk

**Faster review of huge diffs. Built for agent-era workflows.**

**English** | [中文](./README_zh.md)

High-performance terminal review engine for large changesets.

| Pillar | Commitment |
|--------|------------|
| **Performance** | Viewport-only rendering, compact runtime IR, hard bench gates |
| **Scale** | Multi-file review streams without loading every row into widgets |
| **Experience** | Interactive multi-file navigation, readable layout, scriptable CLI |
| **Agent-era** | Terminal-native, scriptable for humans *and* coding agents (roadmap) |

Binary size is **not** a product goal. We optimize latency and runtime memory on large diffs, not “smallest binary wins”.

## Name

**next-hunk** — navigate the review stream by file and hunk. CLI: `next-hunk`. Short alias later if we want (`nh`).

## Status

`v0.4.0` — daily driver for reviewing diffs:

- [x] Project scaffold + compact unified-diff IR (runtime model)
- [x] Viewport query with binary search on file spans
- [x] Virtualized multi-file TUI (ratatui)
- [x] gix-backed worktree / staged / show (+ patch stdin)
- [x] Jujutsu (jj) first-class: auto-detect, `vcs` config / `--vcs`, `jj diff --git` → same IR (`docs/VCS.md`)
- [x] Robust unified parse (rename, binary placeholder, no-newline, CRLF)
- [x] Benchmarks: parse + viewport materialization
- [x] Syntax highlight (syntect, viewport-only + cached, default on)
- [x] Search: in-stream `/` content search + file-rail `f` path filter
- [x] Hunk navigation: `]h` / `[h` next/prev hunk (binary-searched hunk index)
- [x] Watch mode: `--watch` live-reload (notify, debounce; preserves scroll/selection)
- [x] Pager mode: `next-hunk pager` as git's `core.pager`
- [x] Open in editor: `o` jumps to the focused line in `$EDITOR`
- [x] Diff stats in the status bar (per-file + total `+ins/−del`)
- [x] Ignore-whitespace toggle (`W`, collapses whitespace-only changes)
- [x] Agent bridge: `--focus` startup location, `--note` annotations, `--select` per-hunk approval gate
- [x] Server mode: `next-hunk serve` + `push`/`decision` for live agent→human streaming into a persistent TUI
- [x] Overlay mode: `next-hunk overlay` (tmux popup / zellij float → export JSON on agent stdout)
- [x] `line_numbers` config (no silent no-op)
- [x] `include_untracked` config + `--include-untracked` flag (off by default)
- [x] Working-set review: `diff --all` / `scope = "working-set"` (staged + unstaged; rail `S`/`M`/`?`)
- [x] Branch-level review: `diff --base <rev>` / `--range A..B` / `--strategy upstream-ahead|merge-base`
- [x] `next-hunk filediff <old> <new>` — diff two arbitrary files on disk
- [x] File fold/unfold: `zc` (close) / `zo` (open)
- [x] Stack layout: `layout = "stack"` config (unified default)
- [x] Split layout: `layout = "split"` / `--layout split` side-by-side panes
- [x] Auto layout: `layout = "auto"` / `--layout auto` responsive — picks split (≥120 cols), stack (≥40), or unified on resize without rebuilding the IR
- [x] Wrap config: `wrap = true` for line wrapping (default truncate)
- [x] Async syntax highlight (background worker + gen-id stale rejection; miss renders plain)
- [x] Public perf notes vs common tools (e.g. delta; design claims + bench numbers in `docs/PERF.md`)

## Performance

Parsing and rendering stay in the millisecond / sub-millisecond range even on
megabyte-scale diffs, because the runtime IR is compact and only visible rows
are ever materialized. Measured on the bundled fixtures (release build, one
core; reproduce with `cargo bench`):

| benchmark | input | median |
|---|---|---|
| `parse/huge` | 1.1 MB / 38k-line diff | **~1.3 ms** |
| `parse/medium` | 191 KB / 6.5k-line diff | ~150 µs |
| `parse/small` | 6 KB / 213-line diff | ~5 µs |
| `viewport_huge_h40` | materialize a 40-row window over the huge diff | **~300 µs** |
| `viewport_single_h40` | resolve a file span + clip to viewport | **~190 ns** |

The key idea: navigation (`]h`/`[`h`, file rail) resolves against a
binary-searched index in **nanoseconds**, independent of total diff size, and
the viewport materializes only what's on screen. Full numbers live in
[CHANGELOG.md](./CHANGELOG.md).

## Install

Maintainers: cut tags with [docs/RELEASE.md](./docs/RELEASE.md) (tag-only
workflow; crates.io needs `CARGO_REGISTRY_TOKEN`).

**Recommended paths** (pick one):

| Path | Platforms | Needs Rust? | Command |
|------|-----------|-------------|---------|
| **crates.io** | all | yes (build) | `cargo install next-hunk` |
| **Homebrew** | macOS (Linux brew OK) | yes (build dep) | see below |
| **install.sh** | Linux x86_64/aarch64, macOS arm64/amd64 | no | `curl … \| bash` |
| **GitHub Release** | same as install.sh | no | download `.tar.xz` |
| **from source** | all | yes | `cargo install --git …` |

### crates.io (official Cargo path)

```bash
cargo install next-hunk
# pin a version:
cargo install next-hunk --version 0.4.0
```

### Homebrew

```bash
brew tap wuxiaobai24/next-hunk https://github.com/wuxiaobai24/next-hunk
brew install next-hunk
```

One-shot (no permanent tap):

```bash
brew install --formula \
  https://raw.githubusercontent.com/wuxiaobai24/next-hunk/main/Formula/next-hunk.rb
```

The formula builds from the latest tagged source (`depends_on "rust" => :build`).
Upgrade after a release with `brew update && brew upgrade next-hunk` (tap) or
re-run the one-shot URL.

### One-click installer (prebuilt)

Downloads the latest prebuilt from Releases, verifies sha256, and installs to
`/usr/local/bin` (or `~/.local/bin` if that isn't writable). Covers **Linux
x86_64 / aarch64** (static musl) and **macOS arm64 / x86_64**. Other platforms
fall back to `cargo install` (crates.io, then git).

```bash
curl -fsSL https://github.com/wuxiaobai24/next-hunk/raw/main/scripts/install.sh | bash
```

Options: `--prefix <dir>`, `--bin-dir <dir>`, `--version <ver>`, `--as-pager`
(also wires `git core.pager`), `--force`. Inspect the script first if you prefer.

### Prebuilt Release archives

Each `v*` tag publishes multi-platform **all-features** archives (via the
`release` workflow). Linux musl builds are **fully static** (~2–3 MB xz) and run
on Alpine / distroless / old glibc without a C library. macOS builds use the
same `dist` profile (fat LTO + strip).

| Asset suffix | Target |
|---|---|
| `x86_64-musl` | Linux x86_64, static musl |
| `aarch64-musl` | Linux aarch64, static musl |
| `aarch64-apple-darwin` | macOS Apple Silicon |
| `x86_64-apple-darwin` | macOS Intel |

```bash
# example: Linux x86_64, version 0.4.0 — adjust version + suffix for your platform
VER=0.4.0
TARGET=x86_64-musl   # or aarch64-musl / aarch64-apple-darwin / x86_64-apple-darwin
curl -fsSL "https://github.com/wuxiaobai24/next-hunk/releases/download/v${VER}/next-hunk-${VER}-${TARGET}.tar.xz" \
  | tar -xJ
sudo install -m 0755 "next-hunk-${VER}-${TARGET}/next-hunk" /usr/local/bin/
next-hunk --version
```

See the [Releases page](https://github.com/wuxiaobai24/next-hunk/releases) for
the full asset list and `.sha256` checksums.

### From source / development

```bash
cargo install --git https://github.com/wuxiaobai24/next-hunk --locked   # latest main
cargo install --path .          # local clone
cargo run --release -- diff     # run without installing
```

#### Build a dist binary yourself

next-hunk is pure Rust (gix instead of libgit2, syntect's default-fancy regex,
`zlib-rs`), so **no C cross-toolchain is needed** beyond `musl-tools` for static
Linux targets:

```bash
# Linux static (example: x86_64 musl)
rustup target add x86_64-unknown-linux-musl
cargo build --profile dist --all-features --target x86_64-unknown-linux-musl
# → target/x86_64-unknown-linux-musl/dist/next-hunk  (statically linked)
ldd target/x86_64-unknown-linux-musl/dist/next-hunk   # "statically linked"

# macOS (native)
cargo build --profile dist --all-features
```

The `dist` profile (fat LTO + strip + `panic=abort`) yields a ~7 MB binary
(~2.6 MB xz). A normal `--release` build stays optimized for speed instead.

### Set as git's pager (recommended)

Once installed, make everyday `git diff` / `show` / `log` open the review TUI:

```bash
git config --global core.pager "next-hunk pager"
```

## Usage

```bash
next-hunk                  # working tree diff (unstaged only)
next-hunk diff --staged    # staged only (`git diff --cached`)
next-hunk diff --all       # full working set: staged + unstaged
next-hunk diff --all --include-untracked  # everything `git status` would list
next-hunk diff --base origin/main   # whole branch vs base (+ local worktree edits)
next-hunk diff --strategy merge-base --base origin/main  # PR-style fork point
next-hunk diff --strategy upstream-ahead  # vs @{upstream} (merge-base)
next-hunk diff --range main..HEAD   # explicit commit range (same as show)
next-hunk diff --watch     # live-reload on file changes
next-hunk diff --include-untracked  # include untracked (worktree / --all / --base)
next-hunk filediff old.rs new.rs    # diff two arbitrary files
next-hunk show HEAD
git diff | next-hunk patch -
next-hunk inspect path/to.patch   # IR summary, no TUI (scripting)
next-hunk inspect --json path/to.patch  # same shape as `review` (agent-friendly)
next-hunk inspect --all --include-untracked  # script: list all local buckets
next-hunk inspect --base origin/main --json  # branch-level structure for agents

# Use next-hunk as git's pager for everyday diff/show/log:
git config core.pager "next-hunk pager"
git diff        # → launches the review TUI
git show HEAD   # → launches the review TUI
```

### Keybindings

| Key | Action |
|-----|--------|
| `j` / `↓` | scroll down one row |
| `k` / `↑` | scroll up one row |
| `J` / `PgDn` | scroll half a page |
| `K` / `PgUp` | scroll half a page up |
| `Ctrl-D` / `Ctrl-F` | scroll down (half / full page) |
| `Ctrl-U` / `Ctrl-B` | scroll up (half / full page) |
| `g` / `Home` | jump to top |
| `G` / `End` | jump to bottom |
| `]h` | next hunk (wraps across files) |
| `[h` | previous hunk (wraps across files) |
| `Space` | next hunk (quick `]h` alias) |
| `a` / `r` / `u` | accept / reject / undecided current hunk (review tracking; always on with default persistence) |
| `A` | accept **all** hunks in the current file → next unreviewed file |
| `]u` / `[u` | next / previous unreviewed hunk |
| `]U` / `[U` | next / previous unreviewed file |
| `zc` | fold (collapse) current file |
| `zo` | unfold (expand) current file |
| `Tab` / `l` / `→` | next file |
| `Shift+Tab` / `h` / `←` | previous file |
| `1`–`9` | jump to the Nth file |
| `b` | toggle the file-rail sidebar |
| click file rail | select that file |
| click stream | position the viewport on that row |
| `H` | toggle syntax highlight |
| `#` | toggle line-number gutter |
| `w` | toggle word-level inline diff |
| `W` | toggle ignore-whitespace (hide whitespace-only changes) |
| `t` | cycle theme: light → auto → dark → catppuccin-mocha → catppuccin-latte → tokyonight |
| `/` | search diff content (then `n`/`N` next/prev) |
| `f` | filter file rail by path substring |
| `o` | open the focused line in `$EDITOR` (at that line) |
| `v` | enter **visual range select** at the top code row |
| `j` / `k` (in visual) | extend the selection down / up |
| `c` | comment: current line (normal) or selected range (visual) |
| `C` | comment on the current hunk |
| Enter / Esc (comment draft) | save / cancel the comment |
| `?` | toggle the full-screen keybinding help |
| `a` / `r` / `u` | `--select`: accept / reject / undecided current hunk (auto next) |
| `A` / `R` | `--select`: accept / reject rest of current file |
| `Ctrl-A` / `Ctrl-R` | `--select`: accept / reject all remaining hunks |
| `q` / `Esc` / `Ctrl+C` | quit (`Esc` clears active search / cancels visual first) |

## Configuration

Persist preferences in a `config.toml` instead of re-typing flags. Two layers,
merged with CLI flags (highest precedence first):

```text
CLI flag  >  .next-hunk/config.toml (project)  >  ~/.config/next-hunk/config.toml (user)  >  defaults
```

Fields:

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `scope` | string | `"worktree"` | `"worktree"` (unstaged), `"staged"`, or `"working-set"` (staged+unstaged; CLI `--all`) |
| `strategy` | string | — | `"worktree"` / `"staged"` / `"working-set"` / `"upstream-ahead"` / `"merge-base"` (CLI `--strategy`) |
| `base` | string | — | default base rev for branch reviews (CLI `--base`; pair with `strategy = "merge-base"`) |
| `staged` | bool | `false` | legacy alias for `scope = "staged"` when `scope` is unset |
| `highlight` | bool | `true` | syntax highlighting |
| `watch` | bool | `false` | live-reload on file changes |
| `line_numbers` | bool | — | show old/new line-number gutter (`#` toggles at runtime) |
| `include_untracked` | bool | `false` | include untracked files in worktree / working-set diff (`--include-untracked`) |
| `layout` | string | `"unified"` | `"unified"` (default, interleaved), `"stack"` (old/new blocks per file), `"split"` (side-by-side panes; falls back to stack below 80 cols, unified below 40), or `"auto"` (responsive: split ≥120 cols, stack ≥40, else unified; **recommended for wide terminals**) |
| `wrap` | bool | `false` | wrap long lines in the diff stream (default truncates) |
| `export_on_quit` | string | `"none"` (pager/diff); **serve defaults to `"json"`** when unset | on TUI quit, emit agent report: `"none"` / `"json"` / `"markdown"` / `"both"` (`--export-on-quit`). Serve uses `json` unless config/CLI overrides so agents get comments+notes; pager stays `none` so `core.pager` is clean |
| `vcs` | string | `"auto"` | `"auto"` (prefer jj when `.jj` exists) / `"git"` / `"jj"` — see [`docs/VCS.md`](./docs/VCS.md) |
| `persist_review` | bool | `true` | save accept/reject decisions under `.git/next-hunk/decisions-<scope>.json` and restore on reopen (`--no-persist` disables) |
| `auto_forward` | bool | `true` | when a live `serve` exists, `diff --focus`/`--note` push into it (`--no-forward` disables) |
| `theme` | string | `"light"` | `"dark"` / `"light"` / `"auto"` — Flexoki light/dark mode when `theme_preset` is `"default"`. `t` cycles modes **and** named presets. |
| `theme_preset` | string | `"default"` | Chrome palette: `"default"` (Flexoki) / `"catppuccin-mocha"` / `"catppuccin-latte"` / `"tokyonight"` (CLI `--theme-preset`). |
| `theme_colors` | table | — | Optional hex overrides (`#RRGGBB`): `bg`, `fg`, `add`, `del`, `rail`, `status`. Layered on the active preset. |

**Built-in presets**

| Preset | Character | Syntect |
|--------|-----------|---------|
| `default` | [Flexoki](https://flexoki.com) via `theme` (light/dark/auto) | follows `theme` |
| `catppuccin-mocha` | dark | `base16-ocean.dark` |
| `catppuccin-latte` | light | `base16-ocean.light` |
| `tokyonight` | dark | `base16-ocean.dark` |

Example `~/.config/next-hunk/config.toml`:

```toml
highlight = true
watch = true
theme_preset = "catppuccin-mocha"

# Optional slot overrides (hex):
# [theme_colors]
# add = "#a6e3a1"
# del = "#f38ba8"
# rail = "#45475a"
# status = "#181825"
```

CLI overrides: `--all` / `--staged` / `--base` / `--range` (mutually exclusive modes),
`--strategy <worktree|staged|working-set|upstream-ahead|merge-base>`,
`--watch`, `--no-highlight`, `--include-untracked`, `--layout <unified|stack|split|auto>`,
`--theme-preset <default|catppuccin-mocha|catppuccin-latte|tokyonight>`,
`--export-on-quit <none|json|markdown|both>`, `--vcs <auto|git|jj>`, `--no-persist`, `--no-forward`.

When reviewing a working-set (`--all` or `scope = "working-set"`), the file rail
tags each path with its git bucket: **`S`** staged, **`M`** modified (unstaged),
**`?`** untracked. A path that has both staged and unstaged edits appears twice.

## Agent integration

next-hunk bridges a coding agent's changes to the human reviewer. The agent
calls the CLI; the human gets an interactive TUI pointed at what matters.

**Show changes (no feedback):**

```bash
next-hunk diff \
  --focus src/auth.rs:42 \
  --note src/auth.rs:42="Extracted token validation into its own function" \
  --note banner="Auth refactor — core change is the validation split"
```

- `--focus <path>[:<line>|:h<n>]` — scroll to a file / line / hunk on open.
- `--note <target>=<text>` — agent annotations (repeatable): `<path>:<line>`,
  `<path>:h<n>`, or `banner=<summary>`. Rendered in the TUI.

**Get per-hunk approval (`--select`):**

```bash
next-hunk diff --select --focus src/db/migrate.rs:140 \
  --note src/db/migrate.rs:140="Drops the legacy column — irreversible"
# blocks until the human quits; stdout then gets one JSON line:
# {"accepted":["src/db/migrate.rs:h1"],"rejected":[...],"undecided":[...]}
```

In `--select` mode the human presses `a` (accept) / `r` (reject) / `u`
(undecided) per hunk; on quit the decisions are emitted as JSON for the agent
to parse. `--select` requires an interactive terminal and errors out otherwise.

**Resume review across sessions:** decisions are persisted by default (see
`persist_review`). Quit and reopen the same worktree/staged/working-set diff to
continue; the status bar shows `reviewed N/M files · X/Y hunks`. Use `A` to
accept a whole file, `]u` to jump to the next unreviewed hunk. `--select` still
controls quit-time JSON; persistence is independent.

**Full review report on quit (`export_on_quit`):**

```bash
# One JSON line: decisions + comments + notes (superset of --select shape)
next-hunk diff --export-on-quit json --note banner="please review"

# Markdown for pasting into a coding-agent chat (no --select required)
next-hunk diff --export-on-quit markdown

# Both formats
next-hunk diff --select --export-on-quit both

# Serve: full JSON on quit by default (no flag needed)
next-hunk serve --all --include-untracked

# Recover the last full report if you missed the human's terminal stdout:
next-hunk last-export
```

Pager / plain `diff` default to `none` so `git core.pager` does not pollute
stdout. **`serve` defaults to `json`** when unset so quit always yields a
parseable full report (decisions + comments + notes + banner, with
`schema_version`). Explicit config/CLI `none` still wins. With
`export_on_quit = "json"|"markdown"|"both"`, quit emits the report even without
`--select` (notes/comments included; hunks stay `undecided` until decided in
select/serve). Select/export quits also cache the full report under
`.git/next-hunk/last-export.json` for `next-hunk last-export`.

**Non-TTY / agent contract:** when stdout is not a terminal (piped, CI, agent
tool call), there is no TUI to quit. With `--export-on-quit json|markdown|both`
(or the same value in config), next-hunk **emits the report immediately** and
exits 0 — all hunks `undecided`, plus any `--note` annotations. It never
substitutes the plain inspect summary in that case (so agents can parse stdout
reliably). Without export, non-TTY still falls back to the inspect summary.
`--select` / `--focus` still require an interactive terminal (or a live
`serve` for auto-forward).

### Agent skill

A ready-made skill (`skill/next-hunk/SKILL.md`) teaches a coding agent when
and how to call next-hunk — install it into your agent's skills directory. See
the skill file for the full decision guide and examples.

### Server mode (persistent TUI + live push)

The default is **stateless** (each `next-hunk diff` is a one-shot process).
Optional **server mode** lets an agent stream multiple updates into a single
persistent TUI and read the human's decisions in real time, without
re-launching:

```bash
# Human opens the persistent review TUI (select on; quit → full JSON export):
next-hunk serve

# Agent pushes a new focus/note into the live TUI (returns immediately):
next-hunk push --focus src/auth.rs:88 --note banner="please check the token expiry"

# Agent reads the human's accumulated decisions (one JSON line, returns immediately):
next-hunk decision
# {"accepted":["src/auth.rs:h1"],"rejected":[...],"undecided":[...]}

# After the human quits serve (stdout was on their terminal), recover the full report:
next-hunk last-export
# {"schema_version":1,"accepted":[...],"rejected":[...],"undecided":[...],"comments":[...],...}
```

`serve` binds a Unix socket derived from the **worktree root path** (not the
shared `.git` common dir), so `push`/`decision` run from anywhere in the same
worktree find it automatically — no `--socket` flag. Linked `git worktree`
checkouts each get an independent session and can `serve` in parallel.

```bash
# Discover live sessions (repo= is the absolute worktree root):
next-hunk list
# Only sessions for worktrees of the current repository:
next-hunk list --all-worktrees
```

**Auto-forward:** when a serve is live for the current worktree,
`next-hunk diff --focus … --note …` (without `--select` / `--watch`) pushes
into that TUI instead of opening a second one-shot review. Works without a
TTY so agents can prefer the CLI. Disable with `--no-forward` or
`auto_forward = false` in config.

Requires the `serve` feature (on by default) and a Unix OS; on other builds the
subcommands report that they're unavailable. The `decision` output matches the
`--select` quit shape (three buckets only), so an agent parses both identically.
Full post-quit reports (with comments) use `export_on_quit` / `last-export`.

### MCP control plane (optional host integration)

Agents that speak [MCP](https://modelcontextprotocol.io/) can attach without
shelling out:

```bash
# Human still owns the TUI:
next-hunk serve --all

# MCP host spawns (stdio JSON-RPC):
next-hunk mcp
```

Tools mirror the session CLI (`list_sessions`, `review_structure`, `navigate`,
`add_comment`, `get_decision`, `push_focus_note`, `reload`). Feature `mcp` is
on by default (no extra crates); config snippets for Claude Code and generic
hosts are in **[`docs/MCP.md`](./docs/MCP.md)**.

### Overlay (in-session one-shot review)

When the agent already runs **inside tmux or zellij**, open a floating review
without a separate `serve` pane:

```bash
# Blocks until you quit the popup; prints full export JSON on the caller's stdout:
next-hunk overlay --all --include-untracked \
  --focus src/auth.rs:42 --note banner="please review token expiry"
```

| Host | Behavior |
|------|----------|
| `$TMUX` set | `tmux display-popup` (blocks; size via `NEXT_HUNK_POPUP_WIDTH`/`HEIGHT`, default 90%) |
| `$ZELLIJ` set | floating pane + wait for quit |
| No mux, but TTY | one-shot `diff --select --export-on-quit json` in the current terminal |
| No mux, no TTY | clear error: use adjacent `serve`, or re-run inside tmux/zellij |

Stdout contract matches `--export-on-quit json` / `last-export` (`schema_version`,
decisions, comments, notes, banner). Internally uses `NEXT_HUNK_EXPORT_PATH` so
popup TTY stdout does not need to be the agent's pipe.

## Testing & benchmarks

```bash
# unit + integration + headless TUI tests
cargo test

# generate fixtures (small / medium / huge)
./scripts/gen_fixtures.sh

# benchmarks (PERF.md metrics)
cargo bench --bench parse
cargo bench --bench viewport
```

### Git hooks (for contributors)

A `pre-commit` hook runs `cargo fmt --check` and `cargo clippy -- -D warnings`
locally so formatting/clippy drift never reaches CI. It's installed
**automatically** the first time you run `cargo test` in this repo (via
[cargo-husky](https://github.com/rhysd/cargo-husky)). To bypass it for an
experimental commit, use `git commit --no-verify`.

## Architecture (short)

```
git/patch ──► runtime IR (byte/line spans) ──► viewport query ──► TUI
```

Never build a full widget tree for every diff line. The IR is the source of truth; the UI materializes only what is on screen (+ small overscan).

**Full write-up:**

| Doc | Contents |
|-----|----------|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Position, layers, IR, phases, risks ([中文](docs/ARCHITECTURE_zh.md)) |
| [docs/PERF.md](docs/PERF.md) | Fixtures, metrics, phase gates, anti-patterns ([中文](docs/PERF_zh.md)) |
| [docs/MCP.md](docs/MCP.md) | MCP stdio tools + host config snippets |

## License

MIT
