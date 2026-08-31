# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Changed — the path filter is live while typing

- The `f` file-rail filter now narrows the rail and re-anchors the
  selection on every keystroke (matching the incremental search) instead
  of waiting for `Enter`; `Enter` just confirms and reports the narrowed
  count, `Esc` still clears.

### Added — `mouse` config (turn off mouse capture)

- **`mouse = false`** (default `true`) stops the TUI from capturing the
  mouse, so the terminal's native click-drag text selection works and the
  TUI is driven purely by the keyboard. Capture is also skipped when
  resuming after the `$EDITOR` round-trip.

### Added — real line editing in the prompts

- The `/` search, `f` filter, and `c` note prompts now support cursor
  motion and editing: `Left`/`Right` move the caret, `Home`/`End` jump to
  the ends, `Delete` removes the char under it, inserts happen at the
  caret, and `Ctrl-U`/`Ctrl-W` operate relative to it (readline
  semantics). Editing used to be append/backspace-only.

### Changed — navigation jumps land centered (vim-style)

- **New `jump_center` config (default `true`)**: `]h`/`[h`, search matches
  (`n`/`N` and live search), file jumps, and `--focus` now land their
  target row mid-viewport so the context above it is visible, instead of
  pinning it to the top edge. Set `jump_center = false` for the old
  top-pinning behavior.

### Added — the help overlay shows toggle state

- Toggle rows in the `?` overlay now carry their live value — e.g.
  "toggle line wrapping (on)", "toggle ignore-whitespace view (off)" — so
  the panel answers "what is on right now?" without leaving it. The layout
  cycle keeps its status badge (a suffix would clip at the overlay's
  64-column width).

### Fixed — rail `+ins/−del` tally was clipped off most terminal widths

- The rail's per-file path was capped at a fixed 22 columns regardless of
  the rail's actual width, so on an 80-column terminal the right-aligned
  tally (and often the 💬 badge) painted past the rail's inner edge and
  was clipped away — visible only on ≳110-column terminals. The path
  budget is now derived from the row: whatever is left after the index,
  chip, chevron, tally, and badge. The tally-visibility test now asserts
  inside the rail area only (it previously passed for the wrong reason:
  the stream pane's own file-header stats matched the assertion).

### Fixed — `--select` keys mark the hunk under the cursor

- With two hunks on screen, `a`/`r`/`u` acted on the first hunk header in
  the viewport, which could be a different hunk than the one the cursor
  was reading. They now act on the hunk containing the review cursor
  (falling back to the viewport's first hunk when the cursor is between
  hunks), then still advance to the next hunk.

### Fixed — the mouse no longer acts while a prompt is open

- Wheel scrolling or clicking while composing a note (or editing a search /
  filter) scrolled the diff under the draft and moved the anchor row. The
  mouse is now inert during text input and works again once the prompt
  closes.

### Fixed — tiny terminals fail fast with a readable message

- Starting a review in a terminal smaller than 20×4 (e.g. a popup pane)
  used to render an empty screen with no explanation. It now fails with a
  readable `terminal too small: WxH (need at least 20 columns x 4 rows)`
  error before the TUI takes over — and the failure is a real non-zero
  exit (an earlier cut degraded it to a `note:` + inspect dump with exit
  0, hiding that no review ran; `--select`/`serve` callers could believe
  the review happened).

### Fixed — a failing `$EDITOR` no longer reports success

- `o` with an editor that exits non-zero (bad args, editor error) rendered
  a green "success" toast reading `editor exited …`. It now surfaces as a
  red error toast like every other open failure.

### Fixed — `zc`/`zo` fold the file the cursor is in, not the rail's

- After a pure scroll (mouse wheel), the rail selection trails the viewport
  *top* while the review cursor clamps to the viewport edge — the two can
  name different files, and `zc` used to fold whichever one the rail named,
  sometimes not the file being read. Fold/unfold now anchor to the file the
  cursor sits in (matching `o` and `c`, which were already cursor-anchored).

### Changed — config typos now warn instead of silently falling back

- Unknown `layout`, `export_on_quit`, `cursor_line`, and `theme` values, an
  invalid `sidebar` value, and an out-of-range `tab_width` now print a
  `warning: …` line at startup naming the valid values — a typo like
  `layout = "spilt"` used to quietly change behavior while keybinding
  typos already warned. `cursor_line` also parses case-insensitively now
  (`Off` used to count as on).

### Added — runtime wrap toggle (`zw`)

- **`zw` toggles line wrapping** at runtime (in the `zc`/`zo`/`zx` fold-key
  family). Wrapping was config-only (`wrap = true`) — hitting one 500-column
  line meant quitting to edit config.toml. A ` wrap` badge appears in the
  status bar while it's on; the toast notes that scroll positions are
  approximate while wrapped (one virtual row can span multiple screen rows).

### Changed — transient toasts sit flush right (no more clipping)

- The status message (errors, confirmations) is rendered **right-aligned**
  with the path's budget shrinking to make room, so a narrow terminal can
  no longer clip away the one thing a user most needs to read. When even a
  4-column path sliver can't save the line, the left side truncates and the
  toast stays whole. The sticky startup hint keeps the old flow layout — a
  100-column hint must not evict the path and tallies.

### Added — persistent toggle badges in the status bar

- View transforms now leave a badge in the status bar, so a toggle's effect
  outlives its 4-second toast: `WS` (ignoring whitespace), `wd−` (word diff
  off), `zx−` (context collapse off), and `split`/`stack` (side-by-side or
  stacked layout; unified stays unbadged). `HL` keeps its existing
  on-badge.

### Added — fold state visible in the file rail

- Folded files (`zc`/`zo`) now show in the rail: a fold chevron per row
  (`▾` open / `▸` folded) and a dimmed row for folded files. Previously
  the only symptom of folding was body rows vanishing from the stream.
- The per-file 💬 note badge drops its leading space (the double-width
  glyph carries the separation), so the chevron column doesn't push the
  count off narrow rails.

### Changed — incremental search (live matches while typing)

- **Search is now incremental.** Typing in the `/` prompt computes matches
  on every keystroke and lands on the first one at/after where you pressed
  `/` (like vim's `incsearch`) — highlights, the status-bar
  ` /query n/N ` indicator, and the prompt's live `match i/N` / `no match`
  feedback all update while typing. `Enter` just confirms and closes the
  prompt; `Esc` still cancels and clears.
- Starting a new search clears the previous one's matches (they used to
  stay highlighted while you typed a different query).
- **`n`/`N` now say when they wrapped** (`search wrapped — match 1/17`)
  instead of showing a plain `match 1/17` that reads like a short list.

### Changed — quitting with unsaved notes asks for confirmation

- **A stray `q`/Ctrl+C no longer silently discards your notes.** In a plain
  review (no `--export` / `export_on_quit`, not `--select`) the notes you
  compose with `c` exist only in that session — quitting dropped them with
  no warning. The first quit attempt now warns (`N notes would be
  discarded — quit again to confirm, or set --export / export_on_quit to
  keep them`) and arms; the next `q` or Ctrl+C quits. Sessions with an
  export target, or no notes, quit immediately as before.

### Changed — Esc cancels instead of quitting

- **`Esc` no longer quits in normal mode.** Esc is the cancel key in every
  other surface of this TUI (search/filter/note prompts, the help overlay),
  so Esc-quit was a muscle-memory trap that could throw away review state
  with one keypress. It now binds a new `cancel` action: clear the active
  search, drop any armed two-key prefix — never quit. Quitting is `q`
  (clears an active search first, then quits) or `Ctrl+C`.
- `cancel` is a regular remappable action (`cancel = "esc"` in
  `[keybindings]`); users who want the old behavior can bind
  `quit = ["q", "esc"]`.

### Fixed — mouse/scroll geometry off-by-ones (pane title row)

- **Mouse clicks landed one row low.** Both panes render a one-row title
  inside their area, but click hit-testing treated the pane's top row as the
  first content row — clicking a file in the rail selected the file *below*
  it, and clicking a diff line put the review cursor one line down. The view
  now stores the *content* rect (below the title), and clicks map against it.
- **Rail clicks missed once the file list scrolled.** With more files than
  rail rows, the `List` scrolls but the click math assumed item *k* sits at
  pane row *k*. The view now records the first rendered item index
  (`rail_list_offset`) and clicks map through it.
- **The last diff row was unreachable.** `viewport_height` was synced to the
  full main-area height, but the pane title consumes one of those rows — so
  `max_scroll` stopped one row short and `G`/`end` could never bring the
  final diff line on screen (the scrollbar travel was one row off too).
  Both now count only the rows that actually fit under the title.

### Fixed — width-aware text truncation (CJK overflow)

- `truncate_to_width` (the help/prompt hint line and the status-bar search
  indicator) measured width in *characters*, not display columns — a CJK
  query or hint painted past its budget and clipped. It now measures with
  unicode-width, matching the rail/status truncation helpers.

### Added — review report export on quit (the human→agent feedback loop)

- **`--export json|markdown|both`** (on `diff` / `serve`) — when the human
  quits, emit a structured review report: the `--select` decision arrays
  (`accepted` / `rejected` / `undecided`, same shape as before) plus the
  session's `comments` (the human's `c` notes as `user:N`, session comments,
  and the agent's `--note` annotations as `note-N`) and the joined `banner`
  text. hunk has no counterpart — this is the structured half of the review
  bridge: the human reviews, the agent reads one artifact and acts on it.
- **`--export-file <path>`** — write the report to file(s) instead of stdout.
  With no explicit format it implies `json`; `both` writes sibling
  `.json` / `.md` files (extensions replace/append sensibly).
- **`export_on_quit`** config (`"none"` default, `"json"` / `"markdown"` /
  `"both"`) — make every review emit the report without the flag; unknown
  values fall back to off.
- **Backward compatible by construction**: without `--export` / config,
  behavior is byte-identical to 0.5.0 — `--select` still prints the legacy
  decisions-only JSON, and `next-hunk decision` output is unchanged.
- A Markdown report (`# next-hunk review report` with Banner / Decisions /
  Comments sections) renders for `markdown` / `both`, suitable for pasting
  into an agent prompt or a PR description.
- Agent skill (`skill/next-hunk/SKILL.md`, also the `--agent-context` body)
  gains a "The review report" section and a decision-guide row.

### Changed — UI polish round 2: scrollbar, search indicator, help scroll, safe paths

- **Stream scrollbar**: when the review out-scrolls the viewport (the norm
  on huge diffs) a one-column scrollbar rides the stream's right edge —
  dim track, theme-accented thumb sized to the visible share of rows. The
  stream truncates one column earlier, so the bar never paints over
  content; it disappears entirely when everything fits.
- **Persistent search indicator**: while a search is active the status bar
  carries ` /query n/N ` painted in the active-match colors, so the match
  position survives any number of status toasts (the old one-shot
  "match 3/17" message was overwritten by the next action).
- **Help overlay scrolls**: the `?` panel is sized to its content (no dead
  space on tall terminals) and, when the terminal is too short, scrolls
  with `j`/`k` or the wheel instead of silently clipping the Session /
  Agent sections near the bottom; reopening the panel resets to the top.
- **Width-aware path truncation**: rail and status-bar path truncation now
  measure display columns (CJK chars cost 2) and cut at char boundaries.
  This fixes a panic on long non-ASCII filenames — the old rail cut sliced
  a raw byte index mid-char — and keeps the rail's right-aligned
  `+ins/−del` tally aligned for CJK paths.

### Changed — the default theme is now Flexoki dark

- With `theme` unset (or an unknown value) in the config, the review TUI
  now starts on **Flexoki dark** — the inky look the bare `"flexoki"`
  preset name has always stood for — instead of flexoki-light, so the
  product default and the preset vocabulary finally agree. The paper
  palette remains one keystroke away (`t` cycles dark/light/auto) and
  `theme = "flexoki-light"` / `"light"` still selects it explicitly.

### Fixed — readable attention marks on light backgrounds

- **`highlight add` marks no longer vanish into their fill on light
  palettes**: a mark paints a solid accent background (the deep 600-level
  red/blue/green of Flexoki-light / Catppuccin-latte) but left the code
  text's foreground untouched — dark syntax ink on a dark fill was
  effectively unreadable. Marks now paint their text in a new `on_accent`
  theme slot (near-white on the light palettes, ink on the dark ones,
  whose accents are mid-tone); the light-gold warning fill keeps its dark
  `match_active_fg` pairing. Locked in by a WCAG-contrast regression gate
  (every palette's `add` / `delete` / `hunk_header` fills and the gold
  match fill pair with their text at ≥ 3:1, light and dark) plus a
  render-level test asserting the marked cells carry the new ink.

### Fixed — difftool review of temp files on Windows

- **`git difftool` (and `next-hunk filediff`) no longer fail when the
  reviewed files live outside the repository.** The blob-diff pipeline was
  handed the files' absolute path as the *resource* path; gix validates
  resource paths by splitting them into components and rejecting
  non-normal ones — a leading `/` slips through on Unix, but a Windows
  drive prefix (`C:`) parses as a path prefix and failed with
  "contains relative or absolute components". The path given to gix now
  falls back to the bare file name for out-of-repo files (attribute
  lookups only ever matched on the name), while the diff header keeps the
  full original label that the difftool relabeling rewrites to the real
  path. This is the failure that had been red on every `test
  (windows-latest)` CI run since before 0.5.0.

## [0.5.0] - 2026-08-31

### Added — measured head-to-head perf vs hunk 0.20

- `docs/PERF.md` / `PERF_zh.md` gain a measured table (same machine, same
  200×50 tmux session, exact-process RSS attribution): process baseline
  **2 ms vs 203 ms**; TUI RSS **25.8 MB vs 115.7 MB** on a 1.1 MB / 38k-line
  diff (4.5× less) and **32.5 MB vs 177.8 MB** on a 7.8k-line real diff
  (5.5× less); viewport materialization ~350 µs per 40-row window
  (~0.35 µs/window amortized). Method notes explain why the smaller diff
  shows the larger absolute gap (runtime baseline vs compact IR).

### Changed — UI polish: change chips, stat bars, note cards

- **File headers carry a change chip and a stat bar**: `─── M src/a.rs
  ─────── +12 ██████ ███ −3` — the A/M/D/R chip derives from the IR's
  old/new paths (`/dev/null` sides classify as add/delete), and a
  proportional 10-cell bar splits insert vs delete mass (zero sides
  omitted, like the rail tally). The rule is full-bleed on wide panes.
- **File rail shows the change kind**: ` 2. D gone.rs  −42`.
- **Note cards use a tree connector**: `  ╰─ 💬 text` — reads as attached
  to the line above it.
- **Help overlay**: rounded, theme-accented border; key column auto-sizes
  (16–24) and long key lists take their own row so descriptions never clip.

### Added — CLI parity: skill path, agent-context, difftool, update

- **`nh skill path`** — prints (and on first use materializes under
  `$XDG_DATA_HOME/next-hunk/skill/`) the bundled agent skill document, so
  agents can load it without a source checkout. Same contract as hunk's
  `hunk skill path`.
- **`--agent-context`** (on `diff` / `serve`) — prints the agent workflow
  document to stdout and exits; the doc is embedded in the binary.
- **git difftool compatibility** — `nh` recognizes difftool's
  `<path> <old> <old-hex> <old-mode> <new> <new-hex> <new-mode>` invocation,
  reviews the two temp files, and relabels them to the real path (headers,
  rail, and agent notes address the file the human changed). Configure with
  `git config difftool.nh.cmd nh`.
- **`nh update [--check]`** — probes the latest GitHub release and reports
  (or prints the install routes). Never self-overwrites: the installer owns
  the binary.
- Restores the Jujutsu & Sapling README section lost in #95's squash.

### Added — `[keybindings]` remapping

- **Every interactive command is a named, remappable action.** A
  `[keybindings]` table in config.toml rebinds any of the ~36 actions:
  `quit = "Q"`, `next_hunk = ["]j", "space"]`, `search = "ctrl-s"`,
  `help = "f1"`, `prev_match = false` (unbind). Specs cover chars (case
  sensitive), named keys (f1–f12, esc, pageup, …), `ctrl-<char>`, and
  two-key sequences (`]h`, `zc`). The default map reproduces the built-in
  keys exactly.
- **Exclusive claims with honest warnings.** An override fully replaces an
  action's keys; stealing a key from another action's defaults warns on
  stderr before the TUI opens; two overrides fighting over a key resolve
  first-listed-wins (warned). Invalid specs / unknown action names warn and
  are ignored — a bad config never bricks the defaults, and a garbage value
  (`quit = [12345]`) keeps the defaults rather than silently unbinding.
- **Help that can't lie.** The `?` overlay, the bottom hint line, and the
  startup status all render from the live keymap, so remapped keys are what
  you see.
- Dispatch refactor: `handle_normal_key`'s 300-line `match` became a keymap
  lookup plus one `run_action` match. Modifier keys no longer leak into
  plain-char bindings (Ctrl+J used to move the cursor down as `j`).

### Added — Jujutsu & Sapling support (revsets)

- **Workspace VCS auto-detection**: walking up from the cwd, a `.jj` directory
  selects Jujutsu (winning over `.git` in colocated repos — the jj view is
  the source of truth), `.sl` selects Sapling, `.git` stays on the in-process
  gix path. Nothing found keeps the existing git error.
- **`diff` / `show` / `serve` / `inspect` speak revsets** in jj/sl workspaces:
  `nh diff` reviews the working copy (`@`), `nh diff 'main..@'` passes the
  revset to `jj diff --git -r`, `nh show '@-'` to `jj show --git -r`. Bad
  revsets surface the VCS's own error text. `--staged`/`--include-untracked`
  are git-only and print a note (ignored).
- **Sessions & reload work the same**: jj/sl reviews bind the agent session
  socket (titles like `jj working copy (@)`), and `reload`/`--watch` re-run
  the same jj/sl command with the same revset and pathspecs.
- Adapter commands run with `--no-pager` (jj) from the workspace root, so
  paths and revsets resolve exactly like the user's own shell.

### Added — config parity: tab_width, sidebar, agent_notes

- **`tab_width`** (1–16, default 4, `--tab-width` flag) — tabs in diff lines
  now expand to configured stops at render time instead of relying on
  terminal tab stops (usually 8), which silently broke the split layout's
  column alignment. Attention-mark ranges (`highlight add`) are raw-diff
  columns and are remapped onto the expanded text, so marks land on the
  right on-screen cells.
- **`sidebar`** config (`true`/`false`, plus hunk-style `"auto"` accepted as
  `true`) — start with the file rail hidden; `b` still toggles at runtime.
- **`agent_notes = false`** — plain-diff mode: no 💬 inline annotations, note
  rows, or rail badges; `}`/`{` and `c` report "notes hidden" instead of
  jumping/composing.
- **`show` / `patch` / `pager` / `filediff` now honor the full config** —
  previously each cherry-picked fields (`show` ignored `highlight`, `pager`
  ignored `sidebar`, …). All review modes resolve the same layered config
  via a new `ViewSettings` struct that also replaces the 9-positional-bool
  plumbing between `cli` and the TUI.

### Added — attention marks (`highlight`)
- **`highlight add`** paints an agent's attention onto exact char ranges of
  a diff line — "look at these columns while I explain". The mark renders
  in the live TUI as a tone background + underline over the range
  (syntax highlighting stays visible around it), in both unified and split
  layouts. `--tone warning|danger|info|accent`, `--focus` scrolls the human
  to the line. `highlight list` and `highlight clear [--file]` manage marks.
  Ranges are 1-based half-open (`--start 8 --end 14` marks chars 8..13).

### Added — comment & context parity with hunk
- **`comment add` renders live.** A comment added from the CLI immediately
  appears as a 💬 note in the running TUI (like hunk's live comment cards);
  the old two-step `comment add` + `comment apply` dance is gone. New
  `--focus` flag also scrolls the human's TUI to the comment.
- **`comment apply --stdin`** — apply a JSON batch
  (`{"comments":[{"file":"a.rs","line":4,"text":"…"},…]}`, per item exactly
  one of `line`/`hunk`) in one round trip. The whole batch is validated
  (known file, in-range hunk) before anything is applied; a bad item errors
  without mutating. Optional `--focus` jumps to the first comment.
- **`comment clear [--file <path>] [--all] --yes`** — remove comments
  (agent ones by default; `--all` also removes human `user:*` notes) and
  their rendered note rows. Guarded by `--yes`.
- **`context`** — report where the human is currently looking
  (`focus: src/a.rs:h2:42`), plain or `--json`. Mirrors hunk's
  `session context`.
- **`navigate --next-note` / `--prev-note`** — jump the TUI between
  annotated rows from the CLI (same as the `}`/`{` keys).

### Fixed — session error replies never reached the client
- **`ServerReply::Error` was unserializable.** As a newtype variant under an
  internally tagged enum it failed serde serialization at runtime, so every
  error reply was silently dropped — CLI clients saw a bare EOF instead of
  the message (and the failed serialize was eprintln'd onto the TUI screen).
  Now a struct variant (`{"reply":"Error","message":"…"}`), with a
  round-trip test over every reply variant.
- **Duplicate comment ids** — `comment add` and batch apply used separate
  id counters, producing colliding `c0` ids; they now share one sequence.

### Added — every review TUI is an agent-addressable session
- **`diff` and `show` now attach the session control plane** (previously
  `serve`-only): an everyday `nh diff` binds a per-process Unix socket and
  `list` / `get` / `review` / `navigate` / `push` / `reload` / `comment` /
  `decision` operate on it live. Bind failure is non-fatal — the review
  still opens without the control plane.
- **Session discovery by repo, multi-session safe.** Socket names carry the
  repo hash + pid (`next-hunk-<hash>-<pid>.sock`), so several reviews of one
  repo coexist. Session commands auto-target the single live session of the
  current repo; with several, they list candidates and accept
  `--hash <session-id>` (as printed by `list`). `push` and `decision` gained
  `--hash` too.
- **`reload` works on any session** — every interactive review keeps a
  reloader (agent-triggered), while `--watch` remains the opt-in filesystem
  poller (the watcher no longer starts merely because a reloader exists).
- **Richer `Info`**: reports the real repo root (previously the first file's
  path), pid, launch mode (`diff`/`show`/`serve`), a session title
  (`demo working tree`, `show HEAD`), and the human's current focus
  (file/hunk/line). `list` and `get` print these.
- **Probe connections no longer corrupt the TUI.** Socket-discovery probes
  (connect, no command) previously triggered an EOF error printed to stderr,
  which lands on the TUI's alternate screen. EOF/no-command connections are
  now silent, and connection-error logging is opt-in via
  `NEXT_HUNK_SERVE_DEBUG=1`.
- **Stale-socket sweep on session start**: TUIs killed by SIGHUP/SIGKILL
  (closed terminal, killed pane) run no cleanup; the next session reclaims
  dead `next-hunk-*.sock` files so the runtime dir doesn't accumulate them.

### Fixed — `]h`/`[h`/`Space` stuck on one-screen diffs
- **Repeated hunk jumps now always advance.** `]h`/`[h`/`Space` anchored
  their search on the viewport-top row; when the whole stream fits on one
  screen (`max_scroll() == 0`) `jump_to_stream` can't move the viewport, so
  the anchor never advanced and every press re-found the first hunk. The
  jump now anchors on the last jumped hunk while it is still in view
  (mirroring the existing `}`/`{` note-jump anchor) and resets on reload.
  Everyday one-screen diffs — the common case — were all affected.
- **Hunk-jump status shows the hunk ordinal** (`→ hunk @ src/a.rs:h2`)
  instead of the internal stream row, which read like a line number.

### Added — CI perf gate
- **`perf-gate` workflow job** — generates the deterministic huge fixture
  (1.1 MB / 38k lines) with `scripts/gen_fixtures.sh` and runs the
  huge-fixture gate with explicit time ceilings: parse ≤ 500 ms, mid-stream
  viewport h40 ≤ 50 ms (debug build, ~30× observed debug timings so slow
  runners never flake; precise numbers stay in `docs/PERF.md` via
  `cargo bench`). This makes the ARCHITECTURE "regressions fail CI"
  promise real — previously the gate test skipped in CI because the
  fixture is gitignored.

### Fixed — reload preserves the whole view state
- **Watch/agent reload now provably keeps layout, context-collapse,
  folds, and the review cursor** (alongside the existing decisions /
  selected-file / search preservation), re-anchored on the same stream
  row — covered by a regression test.

### Added — theme presets (`T`)
- **Curated chrome palettes** beyond the Flexoki default: `catppuccin-mocha`
  / `catppuccin-latte`, `gruvbox-dark`, `nord`, and `tokyonight`, each
  mapped onto the semantic slots from the official source palettes with a
  matching syntect syntax theme. Config: `theme = "catppuccin-mocha"` (or
  `"gruvbox-dark"`, `"nord"`, `"tokyonight"`, `"flexoki"` /
  `"flexoki-light"`); legacy `"dark"` / `"light"` / `"auto"` values keep
  their old meaning. Unknown names fall back to the default — a typo never
  breaks the TUI.
- **`T` cycles the palette family** (flexoki → catppuccin → gruvbox → nord
  → tokyonight, wrap-around), keeping the current mode; `t` still cycles
  dark/light/auto *within* the family. Both reload the syntax palette and
  bump the highlight-cache generation so stale runs are discarded. The
  status line names the live preset (`theme: catppuccin-mocha (dark)`).

### Added — review cursor + `c` note composition
- **A visible review cursor** — `j`/`k` (and arrows, `J`/`K`, Ctrl-D/U/F/B,
  PgUp/PgDn, `g`/`G`) now move a highlighted cursor row; the viewport
  follows only when the cursor would leave it, so the view stays stable
  around what you're reading. Hunk/file/search/note jumps land the cursor
  on their target; mouse clicks in the stream place the cursor instead of
  re-anchoring the viewport; pure viewport scrolls (wheel) clamp the
  cursor back into view at the nearest edge. The highlight can be hidden
  with `cursor_line = "off"` (default `"row"`); navigation works either
  way. The active search match keeps precedence over the cursor highlight.
- **`c` composes a note at the cursor row** — a single-line composer in
  the prompt row (same editing shortcuts as `/` search: Ctrl-U, Ctrl-W,
  Backspace; Enter saves, Esc discards). The note anchors to the cursor's
  code line (new-side number; a delete anchors to the replacing add
  line), renders like any agent note (inline/fallback), and is mirrored
  into the session comments as `user:N` — so `comment list` shows human
  notes in serve sessions and `comment rm user:N` removes both the
  comment entry and the rendered note row.
- **`o` opens the cursor line** in `$EDITOR` (was: the top visible row) —
  scanning forward within the file when the cursor sits on a header.

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


[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.5.0
[0.4.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.4.0
[0.3.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.3.0
[0.2.1]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.1
[0.2.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.0
[0.1.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.1.0
