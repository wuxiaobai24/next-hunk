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

**next-hunk** — navigate the review stream by file and hunk. CLI: `next-hunk`,
with a short alias binary `nh` (same program, shorter name).

## Status

`v0.4-dev` — daily driver for reviewing diffs:

- [x] Project scaffold + compact unified-diff IR (runtime model)
- [x] Viewport query with binary search on file spans
- [x] Virtualized multi-file TUI (ratatui)
- [x] gix-backed worktree / staged / show (+ patch stdin)
- [x] `nh` short alias binary — same program, shorter to type
- [x] `diff [target]`: `nh diff main`, `nh diff main...feat -- src/` (rev or range vs worktree, git-style)
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
- [x] Sessions: every review TUI (`diff`/`show`/`serve`) is agent-addressable — `list` / `get` / `review` / `navigate` / `push` / `reload` / `decision` work on a live session
- [x] `line_numbers` config (no silent no-op)
- [x] `tab_width` config + `--tab-width` (render-time tab expansion; keeps split columns aligned)
- [x] `sidebar` config (hunk-style `true`/`false`/`"auto"`) and `agent_notes` toggle
- [x] `[keybindings]` remapping — every action name → key list, `?` overlay and hints list the live bindings
- [x] `nh skill path` + `--agent-context` (embedded agent workflow doc), git-difftool invocation compat, `nh update [--check]`
- [x] File headers: change-type chips (A/M/D/R) + right-aligned `+ins ███ −del` proportional bars; rail carries the chip; note cards use `╰─` tree connectors; `?` help panel rounded with per-section column sizing
- [x] jj & Sapling: workspace auto-detection (`.jj` beats colocated `.git`), revset-native `diff`/`show`/`serve`/`inspect`, jj-titled sessions, agent `reload` re-runs the VCS
- [x] `include_untracked` config + `--include-untracked` flag (off by default)
- [x] `next-hunk filediff <old> <new>` — diff two arbitrary files on disk
- [x] File fold/unfold: `zc` (close) / `zo` (open)
- [x] Stack layout: `layout = "stack"` config (unified default)
- [x] Wrap config: `wrap = true` for line wrapping (default truncate)
- [x] Context collapsing: inter-hunk gaps and long context runs fold to `··· N unchanged lines ···` markers (default on, `zx` toggles)
- [x] Split layout: side-by-side aligned columns (`layout = "split"`); `layout = "auto"` picks split/stack/unified by terminal width
- [x] Inline agent notes: `--note` / serve comments render attached to their code line (inline when there's room, dedicated row otherwise); `}`/`{` jumps between notes; file-rail 💬 counts
- [x] Review cursor: highlighted cursor row (`j`/`k`, clicks); `c` composes a note on it (mirrored into serve comments as `user:N`); `o` opens the cursor line; `cursor_line = "off"` hides the highlight
- [x] Theme presets: catppuccin-mocha/latte, gruvbox-dark, nord, tokyonight + flexoki (`T` cycles family, `t` cycles mode)
- [x] CI perf gate: huge-fixture parse/viewport ceilings run on every PR (`perf-gate` job)
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

### One-click (Linux x86_64)

Downloads the latest static musl binary from Releases, verifies its sha256, and
installs it to `/usr/local/bin` (or `~/.local/bin` if that isn't writable):

```bash
curl -fsSL https://github.com/wuxiaobai24/next-hunk/raw/main/scripts/install.sh | bash
```

Inspect the script first if you prefer; options include `--prefix <dir>`,
`--bin-dir <dir>`, `--version <ver>`, `--as-pager` (also wires it into
`git core.pager`), and `--force`. On platforms without a prebuilt binary
(macOS, aarch64) it falls back to `cargo install --git`.

```bash
# from GitHub (canonical release channel)
cargo install --git https://github.com/wuxiaobai24/next-hunk
# or build from a local clone
cargo install --path .
# or just run it
cargo run --release -- diff
```

### Prebuilt static binary (musl)

Each tagged release publishes a **fully static, all-features** x86_64 musl
binary — a single ~2.6 MB (xz) file with no runtime dependencies, so it runs on
any Linux (Alpine, distroless, old glibc, etc.) without installing Rust or a C
library. Grab it from the [Releases page](https://github.com/wuxiaobai24/next-hunk/releases):

```bash
# example for v0.1.0 (adjust the URL/version as needed):
curl -L https://github.com/wuxiaobai24/next-hunk/releases/latest/download/next-hunk-0.1.0-x86_64-musl.tar.xz \
  | tar -xJ
sudo install -m 0755 next-hunk-0.1.0-x86_64-musl/next-hunk /usr/local/bin/
next-hunk --version
```

#### Build a static binary yourself

next-hunk is pure Rust (gix instead of libgit2, syntect's default-fancy regex,
`zlib-rs`), so **no C cross-toolchain is needed** — just the musl rust-std
target:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --profile dist --all-features --target x86_64-unknown-linux-musl
# → target/x86_64-unknown-linux-musl/dist/next-hunk  (statically linked)
ldd target/x86_64-unknown-linux-musl/dist/next-hunk   # "statically linked"
```

The `dist` profile (fat LTO + strip + `panic=abort`) yields a ~7 MB binary
(~2.6 MB xz). A normal `--release` build stays optimized for speed instead.

### Replace git diff

Two routes, pick either or both:

```bash
# Route 1: a git alias — `git d` behaves like `git diff` but opens the TUI
git config --global alias.d '!nh diff'
git d                 # = git diff
git d -s              # = git diff --cached
git d main...feat     # = git diff main...feat

# Route 2: make next-hunk git's pager — everyday git commands open the TUI
git config --global core.pager "nh pager"
git diff              # → launches the review TUI
git show HEAD         # → launches the review TUI
```

Route 1 keeps plain `git diff` working as-is; route 2 takes over everywhere
(note: in pager mode git controls the input, so untracked files don't appear).

## Usage

```bash
nh                          # working tree diff
nh diff -s                  # staged changes
nh diff main                # main vs worktree (like `git diff main`)
nh diff main...feat -- src/ # range diff, pathspec-limited
nh diff --watch             # live-reload on file changes
nh diff --include-untracked # include untracked files
nh show HEAD~3              # a commit (or range)
nh filediff old.rs new.rs   # diff two arbitrary files on disk
git diff | nh pager         # review whatever git pipes in
nh inspect path/to.patch    # IR summary, no TUI (scripting)
nh skill path               # print the agent skill path (materialized on first use)
nh diff --agent-context     # print the agent workflow doc, exit
nh update [--check]         # compare against the latest GitHub release
```

#### As git's difftool

`nh` recognizes git's difftool invocation shape and reviews the two temp
files under the real path label:

```bash
git config difftool.nh.cmd nh        # or the full path to the binary
git config difftool.prompt false
git difftool --tool nh               # opens the review TUI
git difftool -t nh --staged
```

All forms accept the full binary name too (`next-hunk diff …`).

### Jujutsu & Sapling

`nh` detects the workspace VCS by walking up from the cwd (`.jj` wins over
`.git` in colocated repos — the jj view is the source of truth). In a jj or
Sapling workspace the same commands speak revsets natively:

```bash
nh diff              # working-copy change (@ for jj)
nh diff 'main..@'    # any revset — jj parses it, errors read like jj's own
nh show '@-'         # the change of a revision
nh serve             # select-mode review of the jj working copy (agent bridge)
nh list              # sessions title as "jj working copy (@)" / "sl …"
```

`--staged` / `--include-untracked` are git-only and print a note (ignored) in
jj/sl workspaces — jj always shows the working copy as `@`. Live reload
(`--watch` and agent `reload`) re-runs the same jj/sl command. `filediff`
still requires a git object store (any colocated repo has one).

### Keybindings

| Key | Action |
|-----|--------|
| `j` / `↓` | cursor down one row (viewport follows at the edges) |
| `k` / `↑` | cursor up one row |
| `J` / `PgDn` | scroll half a page |
| `K` / `PgUp` | scroll half a page up |
| `Ctrl-D` / `Ctrl-F` | scroll down (half / full page) |
| `Ctrl-U` / `Ctrl-B` | scroll up (half / full page) |
| `g` / `Home` | jump to top |
| `G` / `End` | jump to bottom |
| `]h` | next hunk (wraps across files) |
| `[h` | previous hunk (wraps across files) |
| `Space` | next hunk (quick `]h` alias) |
| `zc` | fold (collapse) current file |
| `zo` | unfold (expand) current file |
| `Tab` / `l` / `→` | next file |
| `Shift+Tab` / `h` / `←` | previous file |
| `1`–`9` | jump to the Nth file |
| `}` / `{` | next / previous note (💬 rows, wraps) |
| `b` | toggle the file-rail sidebar |
| click file rail | select that file |
| click stream | put the review cursor on that row |
| `H` | toggle syntax highlight |
| `#` | toggle line-number gutter |
| `w` | toggle word-level inline diff |
| `W` | toggle ignore-whitespace (hide whitespace-only changes) |
| `t` | cycle theme mode: dark → light → auto |
| `T` | cycle theme palette: flexoki → catppuccin → gruvbox → nord → tokyonight |
| `/` | search diff content (then `n`/`N` next/prev) |
| `f` | filter file rail by path substring |
| `c` | compose a note on the cursor row (Enter save · Esc cancel) |
| `o` | open the cursor line in `$EDITOR` (at that line) |
| `?` | toggle the full-screen keybinding help |
| `q` / `Esc` / `Ctrl+C` | quit (`Esc` clears active search first) |

Every key above (except `1`–`9` and mouse) is remappable via a `[keybindings]`
table — see [Remapping keys](#remapping-keys). The `?` overlay always lists
the *live* bindings.

### Remapping keys

```toml
# ~/.config/next-hunk/config.toml
[keybindings]
quit = "Q"                       # single key
next_hunk = ["]j", "space"]      # several keys (a list)
search = "ctrl-s"                # ctrl-modified
help = "f1"                      # named keys: f1-f12, esc, enter, tab, …
prev_match = false               # unbind the action entirely
```

Key specs: single chars (`q`, `}` — case-sensitive), named keys (`esc`, `enter`,
`space`, `tab`, `backtab`, `up`, `down`, `left`, `right`, `home`, `end`,
`pageup`, `pagedown`, `backspace`, `delete`, `insert`, `f1`–`f12`),
`ctrl-<char>`, and two-key sequences (`]h`, `zc`, `zx`). An override fully
replaces the action's default keys. Explicit overrides may steal a key from
another action's defaults (warned on stderr); two overrides claiming one key
resolve first-listed-wins (warned). Invalid specs and unknown action names
warn and are ignored — a bad config never bricks the default keys. Run
`next-hunk --help`-style introspection is unnecessary: press `?` in the TUI
to see the effective map.

## Configuration

Persist preferences in a `config.toml` instead of re-typing flags. Two layers,
merged with CLI flags (highest precedence first):

```text
CLI flag  >  .next-hunk/config.toml (project)  >  ~/.config/next-hunk/config.toml (user)  >  defaults
```

Fields:

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `staged` | bool | `false` | review staged changes |
| `highlight` | bool | `true` | syntax highlighting |
| `watch` | bool | `false` | live-reload on file changes |
| `line_numbers` | bool | — | show old/new line-number gutter (`#` toggles at runtime) |
| `include_untracked` | bool | `false` | include untracked files in worktree diff (`--include-untracked`) |
| `layout` | string | `"unified"` | `"unified"` (default, interleaved), `"stack"` (old/new blocks per file), `"split"` (side-by-side columns), or `"auto"` (pick split/stack/unified from the live stream width) |
| `wrap` | bool | `false` | wrap long lines in the diff stream (default truncates) |
| `context_collapse` | int | `8` | collapse unchanged context: runs/gaps of ≥ N lines render as one `··· N unchanged lines ···` marker row (`0` disables; `zx` toggles at runtime) |
| `theme` | string | `"light"` | `"dark"` / `"light"` / `"auto"`, or a preset: `"flexoki"` / `"flexoki-light"`, `"catppuccin-mocha"` / `"catppuccin-latte"`, `"gruvbox-dark"`, `"nord"`, `"tokyonight"`. `t` cycles mode, `T` cycles palette. |
| `tab_width` | int | `4` | tab-stop width (columns) for rendering tabs in diff lines, 1–16 — terminal tab stops (8) break split-column alignment, so tabs are expanded at render time (`--tab-width`) |
| `sidebar` | bool/string | `true` | show the file rail at startup; accepts hunk-style `"auto"` (treated as `true` — the rail already adapts to the terminal width). `b` toggles at runtime |
| `agent_notes` | bool | `true` | render 💬 notes (inline annotations, note rows, rail badges); `false` = plain diff view — `}`/`{` and `c` report "notes hidden" instead |
| `[keybindings]` | table | — | remap any action's keys; see [Remapping keys](#remapping-keys) |

Example `~/.config/next-hunk/config.toml`:

```toml
highlight = true
watch = true
```

CLI overrides: `--staged`, `--watch`, `--no-highlight`, `--include-untracked`, `--tab-width <N>`.

Every interactive mode (`diff`, `show`, `patch`, `pager`, `filediff`, `serve`) honors
the same config layers — `show` and `pager` no longer ignore `highlight`,
`wrap`, or `line_numbers`.

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

### Agent skill

A ready-made skill (`skill/next-hunk/SKILL.md`) teaches a coding agent when
and how to call next-hunk — install it into your agent's skills directory. See
the skill file for the full decision guide and examples.

### Sessions (persistent TUI + live push)

Every interactive review (`diff`, `show`, `serve`) is an **agent-addressable
session**: the TUI binds a per-process Unix socket, and the session CLI
commands below work on it live — including an everyday `nh diff`.

```bash
next-hunk list                  # live sessions: id, mode, repo, focus
next-hunk get                   # session info (mode/pid/repo/files/focus)
next-hunk context               # where is the human looking (file/hunk/line)
next-hunk review                # file/hunk structure as JSON
next-hunk navigate src/a.rs:42  # scroll the human's TUI to a file/line/hunk
next-hunk navigate --next-note  # jump to the next 💬 annotated row
next-hunk push --focus src/a.rs:88 --note banner="please check the token expiry"
next-hunk comment add "drops the legacy column" --file src/db.rs --line 140 --focus
next-hunk comment apply --stdin # JSON batch, validated as a whole
next-hunk comment clear --yes   # remove agent comments (and their notes)
next-hunk highlight add --file src/db.rs --line 140 --start 8 --end 14 --tone danger --focus
next-hunk reload                # re-fetch the diff into the live session
next-hunk decision              # {"accepted":[...],"rejected":[...],"undecided":[...]}
```

Sessions are discovered by repo (no `--socket` flag): a command run anywhere
in the repo finds the session automatically. When several reviews of the same
repo are open, commands list the candidates and accept `--hash <session-id>`
(the `<hash>-<pid>` id printed by `list`).

`serve` is the decision-collecting flavor: a persistent TUI with select mode
always on (`a`/`r`/`u` per hunk), so `decision` returns real accept/reject
results. `decision` on a non-select session reports everything undecided —
the output shape is identical, so an agent parses both the same way. Requires
the `serve` feature (on by default) and a Unix OS; on other builds the
subcommands report that they're unavailable.

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

## License

MIT
