//! next-hunk CLI entry.

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use next_hunk::config::{
    CliFlags, Config, DiffRequest, DiffStrategy, ExportOnQuit, LayoutMode, ResolvedConfig,
    VcsPreference,
};
use next_hunk::ir::{parse_unified_diff, FileOrigin, Review};
use next_hunk::source::{
    detect_workspace, produce_diff_request, produce_file_diff, produce_show, resolve_upstream_rev,
    VcsKind, Workspace,
};
use next_hunk::tui::app::ReviewReport;
use next_hunk::tui::{run_review_tui, ReviewOptions};

#[derive(Debug, Parser)]
#[command(
    name = "next-hunk",
    version,
    about = "High-performance terminal review engine for large changesets"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Review working-tree (or staged / full working-set / branch-level) diff.
    Diff {
        /// Review staged changes (`git diff --cached`).
        #[arg(long, short = 's', conflicts_with_all = ["all", "base", "range"])]
        staged: bool,
        /// Review the full working set: staged + unstaged (+ optional untracked).
        ///
        /// One command to see everything `git status` would list as local
        /// changes. File rail marks origins as `S` staged / `M` modified /
        /// `?` untracked when `--include-untracked` is set.
        #[arg(long, short = 'a', conflicts_with_all = ["staged", "base", "range"])]
        all: bool,
        /// Branch-level review against `<rev>` (like `git diff <rev>`): base
        /// tree vs worktree, including uncommitted edits. File rail shows
        /// +/− relative to that base. Use with `--strategy merge-base` for
        /// PR-style `merge-base(<rev>, HEAD)` as the left side.
        #[arg(long, conflicts_with_all = ["staged", "all", "range"])]
        base: Option<String>,
        /// Explicit commit range `A..B` or `A...B` (same as `show <range>`).
        #[arg(long, conflicts_with_all = ["staged", "all", "base"])]
        range: Option<String>,
        /// Diff strategy: `worktree` | `staged` | `working-set` |
        /// `upstream-ahead` (relative to `@{upstream}`, merge-base style) |
        /// `merge-base` (requires `--base <branch>`).
        #[arg(long, value_parser = parse_strategy_arg)]
        strategy: Option<DiffStrategy>,
        /// Re-run on filesystem changes (live reload). Requires the `watch`
        /// feature; otherwise reports that it is unavailable.
        #[arg(long)]
        watch: bool,
        /// Disable syntax highlighting (overrides config/highlight default).
        #[arg(long)]
        no_highlight: bool,
        /// Include untracked files in worktree / working-set / base diff (default: off).
        #[arg(long)]
        include_untracked: bool,
        /// Scroll to this location on startup: `<path>` / `<path>:<line>` /
        /// `<path>:h<n>` (1-based hunk ordinal). Agent-bridge: point the human
        /// at what matters.
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation, repeatable: `<path>:<line>=<text>` /
        /// `<path>:h<n>=<text>` / `banner=<text>`. Shown in the TUI to explain
        /// the change to the human.
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
        /// Selection gate: the human accepts/rejects each hunk (`a`/`r`/`u`),
        /// and on quit the decisions are emitted as JSON on stdout for the
        /// agent to parse. Requires an interactive terminal.
        #[arg(long)]
        select: bool,
        /// On quit, emit an agent-readable review report: `none` (default),
        /// `json`, `markdown`, or `both`. Overrides `export_on_quit` in config.
        /// Works without `--select` (exports notes/comments; all hunks undecided
        /// unless the human used `a`/`r` in select/serve).
        #[arg(long, value_parser = parse_export_on_quit_arg)]
        export_on_quit: Option<ExportOnQuit>,
        /// Diff stream layout: `unified` (default), `stack`, `split`, or `auto` (responsive).
        /// Overrides `layout` from config.toml.
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
        /// Chrome palette preset: `default` | `catppuccin-mocha` |
        /// `catppuccin-latte` | `tokyonight`. Overrides `theme_preset` in config.
        #[arg(long, value_parser = parse_theme_preset_arg)]
        theme_preset: Option<String>,
        /// VCS backend: `auto` (default), `git`, or `jj`. Overrides `vcs` in config.
        #[arg(long, value_parser = parse_vcs_arg)]
        vcs: Option<VcsPreference>,
        /// Disable persisting review decisions across sessions (overrides
        /// `persist_review` in config; default is on).
        #[arg(long)]
        no_persist: bool,
        /// Do not forward into a live `serve` session.
        ///
        /// By default, when a `next-hunk serve` is already running for this
        /// worktree and `--focus` / `--note` is set (without `--select` /
        /// `--watch`), `diff` pushes into that session instead of opening a
        /// second TUI. Pass this flag (or set `auto_forward = false` in
        /// config) to always open a one-shot review.
        #[arg(long)]
        no_forward: bool,
        /// Optional pathspecs to limit the review.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra: Vec<String>,
    },
    /// Review a commit or range (`git show` / `jj diff -r` / range `A..B`).
    Show {
        /// Revision or range (e.g. HEAD, main..HEAD, `@`, `@-`).
        rev: String,
        /// Scroll to this location on startup (same as `diff --focus`).
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation, repeatable (same as `diff --note`).
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
        /// Selection gate (same as `diff --select`). Requires an interactive terminal.
        #[arg(long)]
        select: bool,
        /// On quit, emit an agent-readable review report (see `diff --export-on-quit`).
        #[arg(long, value_parser = parse_export_on_quit_arg)]
        export_on_quit: Option<ExportOnQuit>,
        /// Diff stream layout: `unified`, `stack`, `split`, or `auto` (responsive).
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
        /// Chrome palette preset (see `diff --theme-preset`).
        #[arg(long, value_parser = parse_theme_preset_arg)]
        theme_preset: Option<String>,
        /// VCS backend: `auto` (default), `git`, or `jj`.
        #[arg(long, value_parser = parse_vcs_arg)]
        vcs: Option<VcsPreference>,
    },
    /// Diff two arbitrary files on disk.
    Filediff {
        /// First file (old).
        old: PathBuf,
        /// Second file (new).
        new: PathBuf,
        /// Scroll to this location on startup (same as `diff --focus`).
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation, repeatable (same as `diff --note`).
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
        /// Selection gate (same as `diff --select`). Requires an interactive terminal.
        #[arg(long)]
        select: bool,
        /// On quit, emit an agent-readable review report (see `diff --export-on-quit`).
        #[arg(long, value_parser = parse_export_on_quit_arg)]
        export_on_quit: Option<ExportOnQuit>,
        /// Diff stream layout: `unified`, `stack`, `split`, or `auto` (responsive).
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
        /// Chrome palette preset (see `diff --theme-preset`).
        #[arg(long, value_parser = parse_theme_preset_arg)]
        theme_preset: Option<String>,
        /// VCS backend: `auto` (default), `git`, or `jj` (jj uses system `diff -u`).
        #[arg(long, value_parser = parse_vcs_arg)]
        vcs: Option<VcsPreference>,
    },
    /// Review a unified patch from a file or stdin (`-`).
    Patch {
        /// Path to patch file, or `-` for stdin.
        path: PathBuf,
        /// Scroll to this location on startup (same as `diff --focus`).
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation, repeatable (same as `diff --note`).
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
        /// Selection gate (same as `diff --select`). Requires an interactive terminal.
        #[arg(long)]
        select: bool,
        /// On quit, emit an agent-readable review report (see `diff --export-on-quit`).
        #[arg(long, value_parser = parse_export_on_quit_arg)]
        export_on_quit: Option<ExportOnQuit>,
        /// Diff stream layout: `unified`, `stack`, `split`, or `auto` (responsive).
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
        /// Chrome palette preset (see `diff --theme-preset`).
        #[arg(long, value_parser = parse_theme_preset_arg)]
        theme_preset: Option<String>,
    },
    /// Git pager mode. Reads a unified diff from stdin and opens the TUI.
    ///
    /// Designed for `git config core.pager "next-hunk pager"` so that everyday
    /// `git diff` / `git show` / `git log -p` launch the review TUI directly.
    /// Behaves like `patch -`, but an empty stdin is a clean no-op (exit 0)
    /// rather than an error, because git frequently pipes nothing to its pager
    /// (e.g. `git diff` with no changes).
    Pager,
    /// Print IR summary without opening the TUI (engine smoke / scripting).
    Inspect {
        /// Path to patch file, or `-` for stdin. If omitted, uses worktree diff.
        path: Option<PathBuf>,
        #[arg(long, short = 's', conflicts_with_all = ["all", "base", "range"])]
        staged: bool,
        /// Full working set (staged + unstaged); see `diff --all`.
        #[arg(long, short = 'a', conflicts_with_all = ["staged", "base", "range"])]
        all: bool,
        /// Branch-level base revision (see `diff --base`).
        #[arg(long, conflicts_with_all = ["staged", "all", "range"])]
        base: Option<String>,
        /// Explicit range `A..B` / `A...B` (see `diff --range`).
        #[arg(long, conflicts_with_all = ["staged", "all", "base"])]
        range: Option<String>,
        /// Diff strategy (see `diff --strategy`).
        #[arg(long, value_parser = parse_strategy_arg)]
        strategy: Option<DiffStrategy>,
        /// Include untracked files when reviewing the worktree / working-set / base.
        #[arg(long)]
        include_untracked: bool,
        /// VCS backend: `auto` (default), `git`, or `jj`.
        #[arg(long, value_parser = parse_vcs_arg)]
        vcs: Option<VcsPreference>,
        /// Emit file/hunk structure as JSON (same shape as `next-hunk review`).
        /// Prefer this from agents/skills — no live `serve` required.
        #[arg(long)]
        json: bool,
    },
    /// Open a persistent review TUI that also listens for agent pushes.
    ///
    /// Like `diff`, but the TUI stays open and a separate `next-hunk push` /
    /// `next-hunk decision` process can stream updates into it (focus/notes)
    /// and read the human's accumulated decisions in real time. The TUI runs
    /// with selection mode on (a/r/u per hunk), so `decision` returns real
    /// accept/reject results.
    ///
    /// On quit, serve defaults to `export_on_quit=json` (full report:
    /// decisions + comments + notes + banner) unless config/CLI overrides.
    /// Pager / plain `diff` keep default `none` so `git core.pager` is clean.
    Serve {
        /// Review staged changes (`git diff --cached`).
        #[arg(long, short = 's', conflicts_with_all = ["all", "base", "range"])]
        staged: bool,
        /// Review the full working set: staged + unstaged (+ optional untracked).
        #[arg(long, short = 'a', conflicts_with_all = ["staged", "base", "range"])]
        all: bool,
        /// Branch-level review against `<rev>` (see `diff --base`).
        #[arg(long, conflicts_with_all = ["staged", "all", "range"])]
        base: Option<String>,
        /// Explicit commit range (see `diff --range`).
        #[arg(long, conflicts_with_all = ["staged", "all", "base"])]
        range: Option<String>,
        /// Diff strategy (see `diff --strategy`).
        #[arg(long, value_parser = parse_strategy_arg)]
        strategy: Option<DiffStrategy>,
        /// Re-run on filesystem changes (live reload). Requires the `watch`
        /// feature.
        #[arg(long)]
        watch: bool,
        /// Disable syntax highlighting (overrides config/highlight default).
        #[arg(long)]
        no_highlight: bool,
        /// Include untracked files in worktree / working-set / base diff (default: off).
        #[arg(long)]
        include_untracked: bool,
        /// Scroll to this location on startup: `<path>` / `<path>:<line>` /
        /// `<path>:h<n>` (1-based hunk ordinal).
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation, repeatable: `<path>:<line>=<text>` /
        /// `<path>:h<n>=<text>` / `banner=<text>`.
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
        /// On quit, emit an agent-readable review report (see `diff --export-on-quit`).
        #[arg(long, value_parser = parse_export_on_quit_arg)]
        export_on_quit: Option<ExportOnQuit>,
        /// Diff stream layout: `unified` (default), `stack`, `split`, or `auto` (responsive).
        /// Overrides `layout` from config.toml.
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
        /// Chrome palette preset (see `diff --theme-preset`).
        #[arg(long, value_parser = parse_theme_preset_arg)]
        theme_preset: Option<String>,
        /// VCS backend: `auto` (default), `git`, or `jj`.
        #[arg(long, value_parser = parse_vcs_arg)]
        vcs: Option<VcsPreference>,
        /// Disable persisting review decisions across sessions.
        #[arg(long)]
        no_persist: bool,
        /// Optional pathspecs to limit the review.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra: Vec<String>,
    },
    /// Push a focus/note update into a running `next-hunk serve` in this repo.
    ///
    /// Requires that `next-hunk serve` is already running (the server owns the
    /// TUI). The pushed focus/notes appear live in that TUI; this command
    /// returns immediately with `ok` (or an error message).
    Push {
        /// Scroll the running TUI to this location: `<path>` / `<path>:<line>` /
        /// `<path>:h<n>`.
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation into the running TUI, repeatable.
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
    },
    /// Read the human's accumulated per-hunk decisions from a running `serve`.
    ///
    /// Prints one JSON line on stdout (same shape as `--select` quit output):
    /// `{"accepted":[...],"rejected":[...],"undecided":[...]}`. Returns
    /// immediately — does not wait for the human to quit. Requires a running
    /// `next-hunk serve` in this repo.
    Decision,
    /// Print the last full review export cached on quit.
    ///
    /// When a TUI session quits with `--select` or `export_on_quit` (serve
    /// defaults to json), next-hunk writes the full report under
    /// `.git/next-hunk/last-export.json`. Use this if the agent missed the
    /// human's terminal stdout. Does **not** require a live serve.
    /// Prints one JSON line (same shape as `--export-on-quit json`).
    #[command(name = "last-export")]
    LastExport,
    /// List live next-hunk server sessions.
    ///
    /// Scans well-known socket directories for live servers. Prints one line
    /// per live session: `<hash>  <socket-path>  files=N  repo=<worktree-root>`.
    /// The `repo` field is the absolute worktree root known at `serve` startup
    /// — use it to pick among parallel agent worktrees.
    List {
        /// Only show sessions belonging to worktrees of the **current**
        /// repository (main + linked `git worktree` checkouts). Also lists
        /// known worktree roots that have no live `serve` yet.
        #[arg(long)]
        all_worktrees: bool,
    },
    /// Show info about a running server session.
    ///
    /// Without an argument, checks the current repo's socket. With a hash
    /// argument, looks up a specific session by its repo hash.
    Get {
        /// Optional repo hash to look up (defaults to current repo).
        hash: Option<String>,
    },
    /// Print the current review's file/hunk structure as JSON.
    ///
    /// Connects to a running serve session and dumps the file/hunk summary
    /// (paths, insert/delete counts, hunk ranges) without full patch text.
    /// Without an argument, uses the current repo's socket.
    Review {
        /// Optional repo hash to look up (defaults to current repo).
        hash: Option<String>,
    },
    /// Navigate a running serve session to a file, hunk, or line.
    ///
    /// Uses the same `--focus` syntax: `<path>`, `<path>:<line>`, or
    /// `<path>:h<n>` (1-based hunk ordinal).
    Navigate {
        /// Navigation target: `<path>` / `<path>:<line>` / `<path>:h<n>`.
        target: String,
        /// Optional repo hash to look up (defaults to current repo).
        #[arg(long)]
        hash: Option<String>,
    },
    /// Manage comments on a running serve session.
    Comment {
        #[command(subcommand)]
        action: CommentAction,
    },
    /// Reload the running serve session's diff content.
    ///
    /// Re-fetches the diff from the same source the serve was started with
    /// and refreshes the review, preserving focus/notes/decisions best-effort.
    /// Requires the serve to have been started with `--watch` (or a reloader).
    Reload {
        /// Optional repo hash to look up (defaults to current repo).
        #[arg(long)]
        hash: Option<String>,
    },
    /// Run as an MCP server over stdio (session control plane → tools).
    ///
    /// Speaks Model Context Protocol JSON-RPC on stdin/stdout so Claude Code /
    /// Codex / OpenCode can call `list_sessions`, `navigate`, `add_comment`,
    /// `get_decision`, etc. without multi-step shell. Requires a live
    /// `next-hunk serve` for tool calls. Feature-gated (`mcp`, on by default).
    /// See `docs/MCP.md` for host config snippets.
    Mcp,
    /// Open an in-session review overlay (tmux popup / zellij float / direct TTY).
    ///
    /// Detects `$TMUX` or `$ZELLIJ`, runs `diff --select --export-on-quit json`
    /// in a floating pane that does not leave the agent session, then prints
    /// the full export JSON on this process's stdout when the human quits.
    /// Same report shape as `last-export` / `--export-on-quit json`.
    ///
    /// Without a multiplexer and without a TTY, prints a clear degradation
    /// message (use adjacent pane `serve`, or one-shot `--select`).
    Overlay {
        /// Review staged changes (`git diff --cached`).
        #[arg(long, short = 's', conflicts_with_all = ["all", "base", "range"])]
        staged: bool,
        /// Full working set: staged + unstaged (+ optional untracked).
        #[arg(long, short = 'a', conflicts_with_all = ["staged", "base", "range"])]
        all: bool,
        /// Branch-level review against `<rev>` (see `diff --base`).
        #[arg(long, conflicts_with_all = ["staged", "all", "range"])]
        base: Option<String>,
        /// Explicit commit range (see `diff --range`).
        #[arg(long, conflicts_with_all = ["staged", "all", "base"])]
        range: Option<String>,
        /// Diff strategy (see `diff --strategy`).
        #[arg(long, value_parser = parse_strategy_arg)]
        strategy: Option<DiffStrategy>,
        /// Disable syntax highlighting.
        #[arg(long)]
        no_highlight: bool,
        /// Include untracked files in worktree / working-set / base diff.
        #[arg(long)]
        include_untracked: bool,
        /// Scroll to this location on startup (same as `diff --focus`).
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation, repeatable (same as `diff --note`).
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
        /// Diff stream layout: `unified`, `stack`, `split`, or `auto` (responsive).
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
        /// Chrome palette preset (see `diff --theme-preset`).
        #[arg(long, value_parser = parse_theme_preset_arg)]
        theme_preset: Option<String>,
        /// VCS backend: `auto` (default), `git`, or `jj`.
        #[arg(long, value_parser = parse_vcs_arg)]
        vcs: Option<VcsPreference>,
        /// Disable persisting review decisions across sessions.
        #[arg(long)]
        no_persist: bool,
        /// Export mode on quit (default `json`). Same values as `--export-on-quit`.
        #[arg(long, value_parser = parse_export_on_quit_arg)]
        export_on_quit: Option<ExportOnQuit>,
        /// Optional pathspecs to limit the review.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra: Vec<String>,
    },
}

#[derive(Debug, clap::Subcommand)]
enum CommentAction {
    /// Add a comment to a file.
    Add {
        /// Target file path.
        #[arg(long)]
        file: String,
        /// Optional new-side source line number (range start when `--line-end` is set).
        #[arg(long)]
        line: Option<u32>,
        /// Optional inclusive end of a new-side line range (requires `--line`).
        #[arg(long)]
        line_end: Option<u32>,
        /// Optional hunk ordinal (1-based).
        #[arg(long)]
        hunk: Option<usize>,
        /// Optional repo hash.
        #[arg(long)]
        hash: Option<String>,
        /// Comment text.
        text: String,
    },
    /// List all comments.
    List {
        /// Optional repo hash.
        #[arg(long)]
        hash: Option<String>,
    },
    /// Remove a comment by id.
    Rm {
        /// Comment id to remove.
        id: String,
        /// Optional repo hash.
        #[arg(long)]
        hash: Option<String>,
    },
    /// Apply comments as TUI notes.
    Apply {
        /// Optional repo hash.
        #[arg(long)]
        hash: Option<String>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Diff {
        staged: false,
        all: false,
        base: None,
        range: None,
        strategy: None,
        watch: false,
        no_highlight: false,
        include_untracked: false,
        focus: None,
        note: Vec::new(),
        select: false,
        export_on_quit: None,
        layout: None,
        theme_preset: None,
        vcs: None,
        no_persist: false,
        no_forward: false,
        extra: Vec::new(),
    }) {
        Commands::Diff {
            staged,
            all,
            base,
            range,
            strategy,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            select,
            export_on_quit,
            layout,
            theme_preset,
            vcs,
            no_persist,
            no_forward,
            extra,
        } => {
            // --select never auto-forwards (needs its own interactive gate).
            // Fail the TTY requirement before touching git so agents get a
            // clear signal even outside a repo.
            if select && !std::io::stdout().is_terminal() {
                bail!("--select requires an interactive terminal (stdout is not a tty)");
            }

            // Parse focus/note syntax early (bad specs fail before git/socket).
            // TTY gate for focus/note is deferred until after the auto-forward
            // attempt: a live serve can accept them headless.
            let focus_target = focus
                .as_deref()
                .map(next_hunk::cli_parse::parse_focus)
                .transpose()?;
            let notes = note
                .iter()
                .map(|s| next_hunk::cli_parse::parse_note(s))
                .collect::<Result<Vec<_>>>()?;

            let cwd = std::env::current_dir()?;

            // Layered config: project (.next-hunk/config.toml) > user
            // (~/.config/next-hunk/config.toml). CLI flags override on top.
            let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut resolved = ResolvedConfig::resolve(
                &cfg,
                &CliFlags {
                    staged: if staged { Some(true) } else { None },
                    all: if all { Some(true) } else { None },
                    base,
                    range,
                    strategy,
                    watch: if watch { Some(true) } else { None },
                    highlight: if no_highlight { Some(false) } else { None },
                    include_untracked: if include_untracked { Some(true) } else { None },
                    layout,
                    export_on_quit,
                    vcs,
                    persist_review: if no_persist { Some(false) } else { None },
                    auto_forward: if no_forward { Some(false) } else { None },
                    theme_preset,
                },
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;

            let ws = detect_workspace(&cwd, resolved.vcs)?;
            validate_and_finalize_request(&mut resolved, &ws, strategy)?;

            // Auto-forward: when a live serve exists and the agent is only
            // pointing/annotating (focus/note, no select/watch), push into
            // that TUI instead of opening a second one-shot review. Works
            // headless (no TTY required) so skills can prefer `diff --focus`.
            if try_auto_forward_diff(
                &ws.root,
                resolved.auto_forward,
                select,
                watch,
                &focus_target,
                &notes,
            )? {
                return Ok(());
            }

            // No live serve (or forward disabled): focus needs a TTY for a
            // one-shot TUI. Notes without export-on-quit also need a TTY (they
            // would only render in the TUI); with export they land in the
            // headless report (see open_review_from_text).
            if !std::io::stdout().is_terminal() {
                if focus_target.is_some() {
                    bail!("--focus requires an interactive terminal (stdout is not a tty)");
                }
                if !notes.is_empty() && matches!(resolved.export_on_quit, ExportOnQuit::None) {
                    bail!("--note requires an interactive terminal (stdout is not a tty)");
                }
            }

            if resolved.watch && !next_hunk::tui::watch::Watcher::is_enabled() {
                eprintln!(
                    "note: `--watch` requires the `watch` feature (rebuild with --features watch)"
                );
            }

            let produced =
                produce_diff_request(&ws, &resolved.request, &extra, resolved.include_untracked)?;
            let reloader = if resolved.watch {
                Some(make_diff_reloader(
                    ws.clone(),
                    resolved.request.clone(),
                    extra,
                    resolved.include_untracked,
                ))
            } else {
                None
            };
            open_review_from_produced(
                produced,
                reloader,
                resolved.highlight,
                resolved.line_numbers,
                resolved.wrap,
                resolved.theme,
                resolved.theme_preset,
                resolved.theme_colors,
                resolved.layout,
                Some(ws.root.clone()),
                ReviewOptions {
                    focus: focus_target,
                    notes,
                    select_mode: select,
                    export_on_quit: resolved.export_on_quit,
                    persist_review: resolved.persist_review,
                    persist_scope: resolved.scope.as_str().to_string(),
                },
                None,
            )
        }
        Commands::Show {
            rev,
            focus,
            note,
            select,
            export_on_quit,
            layout,
            theme_preset,
            vcs,
        } => {
            let bridge = parse_agent_bridge_options(focus, note, select, export_on_quit)?;
            let cwd = std::env::current_dir()?;
            let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
            let pref = vcs
                .or_else(|| cfg.vcs.as_deref().map(VcsPreference::parse_str))
                .unwrap_or_default();
            let ws = detect_workspace(&cwd, pref)?;
            let text = produce_show(&ws, &rev)?;
            // `show` is a one-shot snapshot: no watch, highlight default on.
            // Honor the user/project theme/layout config even for `show`.
            let resolved_layout = resolve_layout_opt(layout, &cfg)?;
            let export = resolve_export_opt(bridge.export_on_quit_override, &cfg)?;
            let resolved_preset = resolve_theme_preset_opt(theme_preset, &cfg)?;
            open_review_from_text(
                &text,
                &[],
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                resolved_preset,
                cfg.theme_colors.clone(),
                resolved_layout,
                Some(ws.root),
                ReviewOptions {
                    focus: bridge.focus,
                    notes: bridge.notes,
                    select_mode: bridge.select,
                    export_on_quit: export,
                    persist_review: cfg.persist_review.unwrap_or(true),
                    persist_scope: format!("show-{}", sanitize_persist_scope(&rev)),
                },
                None,
            )
        }
        Commands::Patch {
            path,
            focus,
            note,
            select,
            export_on_quit,
            layout,
            theme_preset,
        } => {
            let bridge = parse_agent_bridge_options(focus, note, select, export_on_quit)?;
            let text = read_patch_input(&path)?;
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
            let resolved_layout = resolve_layout_opt(layout, &cfg)?;
            let export = resolve_export_opt(bridge.export_on_quit_override, &cfg)?;
            let resolved_preset = resolve_theme_preset_opt(theme_preset, &cfg)?;
            open_review_from_text(
                &text,
                &[],
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                resolved_preset,
                cfg.theme_colors.clone(),
                resolved_layout,
                None,
                ReviewOptions {
                    focus: bridge.focus,
                    notes: bridge.notes,
                    select_mode: bridge.select,
                    export_on_quit: export,
                    // Patches have no stable repo identity for multi-session
                    // resume; keep tracking keys available but skip disk store
                    // unless the human is inside a git workdir (path resolves
                    // via workdir=None → no store).
                    persist_review: false,
                    persist_scope: "patch".into(),
                },
                None,
            )
        }
        Commands::Filediff {
            old,
            new,
            focus,
            note,
            select,
            export_on_quit,
            layout,
            theme_preset,
            vcs,
        } => {
            let bridge = parse_agent_bridge_options(focus, note, select, export_on_quit)?;
            let cwd = std::env::current_dir()?;
            let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
            let pref = vcs
                .or_else(|| cfg.vcs.as_deref().map(VcsPreference::parse_str))
                .unwrap_or_default();
            // Prefer a discovered workspace; if neither git nor jj is present,
            // still allow absolute file paths via a synthetic jj-style system diff
            // by treating cwd as the workdir with git forced only when available.
            let (ws, workdir) = match detect_workspace(&cwd, pref) {
                Ok(ws) => {
                    let root = ws.root.clone();
                    (ws, Some(root))
                }
                Err(_) => {
                    // No VCS: fall back to system diff with cwd as label base.
                    let ws = Workspace {
                        root: cwd.clone(),
                        kind: next_hunk::source::VcsKind::Jj, // uses system_file_diff path
                    };
                    (ws, Some(cwd.clone()))
                }
            };
            let text = produce_file_diff(&ws, &old, &new)?;
            if text.trim().is_empty() {
                eprintln!("(files are identical)");
                return Ok(());
            }
            let resolved_layout = resolve_layout_opt(layout, &cfg)?;
            let export = resolve_export_opt(bridge.export_on_quit_override, &cfg)?;
            let resolved_preset = resolve_theme_preset_opt(theme_preset, &cfg)?;
            open_review_from_text(
                &text,
                &[],
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                resolved_preset,
                cfg.theme_colors.clone(),
                resolved_layout,
                workdir,
                ReviewOptions {
                    focus: bridge.focus,
                    notes: bridge.notes,
                    select_mode: bridge.select,
                    export_on_quit: export,
                    persist_review: cfg.persist_review.unwrap_or(true),
                    persist_scope: "filediff".into(),
                },
                None,
            )
        }
        Commands::Inspect {
            path,
            staged,
            all,
            base,
            range,
            strategy,
            include_untracked,
            vcs,
            json,
        } => {
            // Validate layered config even when inspect does not consume layout/
            // theme — illegal enums must fail every subcommand (dogfood P1).
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
            let (text, origins) = if let Some(path) = path {
                (read_patch_input(&path)?, Vec::new())
            } else {
                let mut resolved = ResolvedConfig::resolve(
                    &cfg,
                    &CliFlags {
                        staged: if staged { Some(true) } else { None },
                        all: if all { Some(true) } else { None },
                        base,
                        range,
                        strategy,
                        include_untracked: if include_untracked { Some(true) } else { None },
                        vcs,
                        ..Default::default()
                    },
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                let ws = detect_workspace(&cwd, resolved.vcs)?;
                validate_and_finalize_request(&mut resolved, &ws, strategy)?;
                let produced =
                    produce_diff_request(&ws, &resolved.request, &[], resolved.include_untracked)?;
                (produced.text, produced.origins)
            };
            if text.trim().is_empty() {
                if json {
                    let empty = next_hunk::ir::ReviewSummary {
                        file_count: 0,
                        stream_len: 0,
                        inserts: 0,
                        deletes: 0,
                        files: Vec::new(),
                    };
                    println!("{}", serde_json::to_string_pretty(&empty)?);
                } else {
                    println!("files=0 stream_rows=0 arena_bytes=0");
                }
                return Ok(());
            }
            let mut review = parse_review(&text)?;
            review.apply_file_origins(&origins);
            if json {
                let summary = next_hunk::ir::ReviewSummary::from(&review);
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                print_inspect(&review);
            }
            Ok(())
        }
        Commands::Pager => {
            // Git pipes the diff to our stdin. Honor the user/project theme.
            let cwd = std::env::current_dir()?;
            let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .context("read patch from stdin (pager mode)")?;
            // Empty stdin is a clean no-op in pager mode: git frequently invokes
            // its pager with nothing (e.g. `git diff` with no changes). Exit 0
            // so git sees a successful pager run.
            if buf.trim().is_empty() {
                return Ok(());
            }
            // `o` (open in editor) resolves relative paths against the repo
            // workdir if we're in one, else the cwd.
            let pref = cfg
                .vcs
                .as_deref()
                .map(VcsPreference::parse_str)
                .unwrap_or_default();
            let workdir = detect_workspace(&cwd, pref).ok().map(|ws| ws.root);
            let resolved_layout = resolve_layout_opt(None, &cfg)?;
            let export = resolve_export_opt(None, &cfg)?;
            open_review_from_text(
                &buf,
                &[],
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                cfg.theme_preset.clone(),
                cfg.theme_colors.clone(),
                resolved_layout,
                workdir,
                ReviewOptions {
                    export_on_quit: export,
                    persist_review: cfg.persist_review.unwrap_or(true),
                    persist_scope: "pager".into(),
                    ..Default::default()
                },
                None,
            )
        }
        Commands::Serve {
            staged,
            all,
            base,
            range,
            strategy,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            export_on_quit,
            layout,
            theme_preset,
            vcs,
            no_persist,
            extra,
        } => run_serve(
            staged,
            all,
            base,
            range,
            strategy,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            export_on_quit,
            layout,
            theme_preset,
            vcs,
            no_persist,
            extra,
        ),
        Commands::Push { focus, note } => run_push(focus, note),
        Commands::Decision => run_decision(),
        Commands::LastExport => run_last_export(),
        Commands::List { all_worktrees } => run_list(all_worktrees),
        Commands::Get { hash } => run_get(hash),
        Commands::Review { hash } => run_review(hash),
        Commands::Navigate { target, hash } => run_navigate(target, hash),
        Commands::Comment { action } => run_comment(action),
        Commands::Reload { hash } => run_reload(hash),
        Commands::Mcp => run_mcp(),
        Commands::Overlay {
            staged,
            all,
            base,
            range,
            strategy,
            no_highlight,
            include_untracked,
            focus,
            note,
            layout,
            theme_preset,
            vcs,
            no_persist,
            export_on_quit,
            extra,
        } => run_overlay(
            staged,
            all,
            base,
            range,
            strategy,
            no_highlight,
            include_untracked,
            focus,
            note,
            layout,
            theme_preset,
            vcs,
            no_persist,
            export_on_quit,
            extra,
        ),
    }
}

/// `next-hunk overlay`: tmux/zellij popup review → full export JSON on stdout.
#[allow(clippy::too_many_arguments)]
fn run_overlay(
    staged: bool,
    all: bool,
    base: Option<String>,
    range: Option<String>,
    strategy: Option<DiffStrategy>,
    no_highlight: bool,
    include_untracked: bool,
    focus: Option<String>,
    note: Vec<String>,
    layout: Option<LayoutMode>,
    theme_preset: Option<String>,
    vcs: Option<VcsPreference>,
    no_persist: bool,
    export_on_quit: Option<ExportOnQuit>,
    extra: Vec<String>,
) -> Result<()> {
    // Validate focus/note specs early (same parser as diff) so bad agent args
    // fail before opening a popup.
    if let Some(ref f) = focus {
        let _ = next_hunk::cli_parse::parse_focus(f)?;
    }
    for n in &note {
        let _ = next_hunk::cli_parse::parse_note(n)?;
    }

    let args = next_hunk::overlay::OverlayDiffArgs {
        staged,
        all,
        base,
        range,
        strategy: strategy.map(|s| s.as_str().to_string()),
        include_untracked,
        focus,
        note,
        layout: layout.map(|l| l.as_str().to_string()),
        theme_preset,
        vcs: vcs.map(|v| v.as_str().to_string()),
        no_highlight,
        no_persist,
        extra,
        export_on_quit: export_on_quit.map(|e| e.as_str().to_string()),
        binary: None,
        cwd: None,
        popup_width: None,
        popup_height: None,
    };
    let env = next_hunk::overlay::OverlayEnv::detect();
    next_hunk::overlay::run_overlay(&args, &env)
}

/// `next-hunk mcp`: MCP stdio server (feature `mcp`).
#[cfg(feature = "mcp")]
fn run_mcp() -> Result<()> {
    next_hunk::mcp::run_mcp_server()
}

#[cfg(not(feature = "mcp"))]
fn run_mcp() -> Result<()> {
    bail!("`mcp` requires the `mcp` feature (rebuild with --features mcp)");
}

/// Parsed agent-bridge flags shared by `diff` / `show` / `patch` / `filediff`.
struct AgentBridgeOptions {
    focus: Option<next_hunk::tui::app::FocusTarget>,
    notes: Vec<next_hunk::tui::app::Note>,
    select: bool,
    /// CLI `--export-on-quit` when present (config still layers underneath).
    export_on_quit_override: Option<ExportOnQuit>,
}

/// Parse `--focus` / `--note` / `--select` and refuse interactive flags when
/// stdout is not a tty — never silently drop focus/select.
///
/// `--note` is allowed headless when paired with `--export-on-quit
/// json|markdown|both` (notes land in the quit report). That gate is applied
/// in [`open_review_from_text`] once export mode is fully resolved (CLI + config).
fn parse_agent_bridge_options(
    focus: Option<String>,
    note: Vec<String>,
    select: bool,
    export_on_quit: Option<ExportOnQuit>,
) -> Result<AgentBridgeOptions> {
    let focus = focus
        .map(|s| next_hunk::cli_parse::parse_focus(&s))
        .transpose()?;
    let notes = note
        .iter()
        .map(|s| next_hunk::cli_parse::parse_note(s))
        .collect::<Result<Vec<_>>>()?;

    // Interactive agent-bridge flags cannot run headless. Fail fast so agents
    // never see an exit-0 inspect summary that discarded their annotations.
    // (Exception: `diff` auto-forward into a live serve, handled before this;
    // `--note` + `--export-on-quit` is handled headless in open_review_from_text.)
    if !std::io::stdout().is_terminal() {
        if select {
            bail!("--select requires an interactive terminal (stdout is not a tty)");
        }
        if focus.is_some() {
            bail!("--focus requires an interactive terminal (stdout is not a tty)");
        }
    }

    Ok(AgentBridgeOptions {
        focus,
        notes,
        select,
        export_on_quit_override: export_on_quit,
    })
}

/// When a live `serve` owns this repo, turn `diff --focus`/`--note` into a
/// `push` instead of opening another TUI.
///
/// Returns `Ok(true)` if the command was fully handled (forwarded). Returns
/// `Ok(false)` when the caller should continue with the normal one-shot path
/// (no live serve, disabled, or nothing to push).
///
/// Eligibility (all must hold):
/// - `auto_forward` is enabled (config default true; off via `--no-forward`
///   or `auto_forward = false`)
/// - not `--select` / not `--watch` (those need their own interactive session)
/// - at least one of `--focus` / `--note` is present
/// - a live serve socket exists for `repo`
#[cfg(all(feature = "serve", unix))]
fn try_auto_forward_diff(
    repo: &std::path::Path,
    auto_forward: bool,
    select: bool,
    watch: bool,
    focus: &Option<next_hunk::tui::app::FocusTarget>,
    notes: &[next_hunk::tui::app::Note],
) -> Result<bool> {
    if !auto_forward || select || watch {
        return Ok(false);
    }
    if focus.is_none() && notes.is_empty() {
        return Ok(false);
    }

    let socket = next_hunk::cli_parse::runtime_socket_path(repo);
    let command = next_hunk::tui::server::ServerCommand::Push {
        focus: focus.clone(),
        notes: notes.to_vec(),
    };
    // No separate connect probe: a bare connect without a command makes the
    // serve accept thread log a parse EOF. `send_command` is the single
    // round-trip; connect failure means no live serve → one-shot path.
    match next_hunk::tui::server::send_command(&socket, &command) {
        Ok(next_hunk::tui::server::ServerReply::Ok) => {
            println!("ok: forwarded to running serve");
            Ok(true)
        }
        Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
            bail!("server error while forwarding: {message}")
        }
        Ok(other) => bail!("unexpected server reply while forwarding: {other:?}"),
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("connect to server socket") {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(not(all(feature = "serve", unix)))]
fn try_auto_forward_diff(
    _repo: &std::path::Path,
    _auto_forward: bool,
    _select: bool,
    _watch: bool,
    _focus: &Option<next_hunk::tui::app::FocusTarget>,
    _notes: &[next_hunk::tui::app::Note],
) -> Result<bool> {
    Ok(false)
}

/// Build the live-reload closure for `--watch`: re-runs the same VCS diff.
/// Captures the workspace, request, pathspecs, and untracked flag by value.
fn make_diff_reloader(
    ws: Workspace,
    request: DiffRequest,
    extra: Vec<String>,
    include_untracked: bool,
) -> next_hunk::tui::Reloader {
    Box::new(move || {
        produce_diff_request(&ws, &request, &extra, include_untracked)
            .context("re-run VCS diff for --watch")
    })
}

/// Resolve the current workspace root for serve socket discovery.
/// Honors project/user `vcs` config so git and jj workspaces share the same path.
#[cfg(all(feature = "serve", unix))]
fn workspace_root_for_socket() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
    let pref = cfg
        .vcs
        .as_deref()
        .map(VcsPreference::parse_str)
        .unwrap_or_default();
    Ok(detect_workspace(&cwd, pref)?.root)
}

/// Validate strategy/base combos and resolve `@{upstream}` when needed.
fn validate_and_finalize_request(
    resolved: &mut ResolvedConfig,
    ws: &Workspace,
    strategy_flag: Option<DiffStrategy>,
) -> Result<()> {
    let strategy = strategy_flag.or_else(|| match &resolved.request {
        DiffRequest::AgainstBase {
            base,
            use_merge_base: true,
        } if base == next_hunk::config::UPSTREAM_PLACEHOLDER => Some(DiffStrategy::UpstreamAhead),
        DiffRequest::AgainstBase {
            use_merge_base: true,
            ..
        } => Some(DiffStrategy::MergeBase),
        _ => None,
    });
    if matches!(strategy, Some(DiffStrategy::MergeBase)) {
        match &resolved.request {
            DiffRequest::AgainstBase { .. } => {}
            DiffRequest::Local(_) => {
                bail!(
                    "--strategy merge-base requires --base <branch> \
                     (e.g. next-hunk diff --strategy merge-base --base origin/main)"
                );
            }
            DiffRequest::Range(_) => {}
        }
    }
    let root = ws.root.clone();
    let kind = ws.kind;
    resolved.finalize_request(|| match kind {
        VcsKind::Git => resolve_upstream_rev(&root),
        VcsKind::Jj => bail!(
            "--strategy upstream-ahead is not supported for jj workspaces; \
             use --base <revset> (e.g. main@origin)"
        ),
    })?;
    Ok(())
}

/// `next-hunk serve`: a persistent TUI that also accepts pushes via a Unix
/// socket. Mirrors the `diff` path (config layering, focus/note parsing,
/// optional `--watch`) but unconditionally enables select mode (the whole
/// point of `serve` is to collect decisions via `next-hunk decision`) and
/// binds a server listener on the repo's runtime socket path.
#[cfg(all(feature = "serve", unix))]
#[allow(clippy::too_many_arguments)] // mirrors Commands::Serve field set
fn run_serve(
    staged: bool,
    all: bool,
    base: Option<String>,
    range: Option<String>,
    strategy: Option<DiffStrategy>,
    watch: bool,
    no_highlight: bool,
    include_untracked: bool,
    focus: Option<String>,
    note: Vec<String>,
    export_on_quit: Option<ExportOnQuit>,
    layout: Option<LayoutMode>,
    theme_preset: Option<String>,
    vcs: Option<VcsPreference>,
    no_persist: bool,
    extra: Vec<String>,
) -> Result<()> {
    // serve is interactive (it owns a TUI); require a real terminal up front.
    if !std::io::stdout().is_terminal() {
        bail!("serve requires an interactive terminal (stdout is not a tty)");
    }

    // serve is always select-mode; still parse focus/note the same way as
    // `diff` so bad specs fail before bind. (TTY was already required above.)
    let bridge = parse_agent_bridge_options(focus, note, /*select=*/ true, export_on_quit)?;

    let cwd = std::env::current_dir()?;

    let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut resolved = ResolvedConfig::resolve(
        &cfg,
        &CliFlags {
            staged: if staged { Some(true) } else { None },
            all: if all { Some(true) } else { None },
            base,
            range,
            strategy,
            watch: if watch { Some(true) } else { None },
            highlight: if no_highlight { Some(false) } else { None },
            include_untracked: if include_untracked { Some(true) } else { None },
            layout,
            export_on_quit: bridge.export_on_quit_override,
            vcs,
            persist_review: if no_persist { Some(false) } else { None },
            auto_forward: None,
            theme_preset,
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let ws = detect_workspace(&cwd, resolved.vcs)?;
    validate_and_finalize_request(&mut resolved, &ws, strategy)?;

    if resolved.watch && !next_hunk::tui::watch::Watcher::is_enabled() {
        eprintln!("note: `--watch` requires the `watch` feature (rebuild with --features watch)");
    }

    // Bind the server socket before opening the TUI, so a `push`/`decision`
    // issued the instant the TUI appears finds a live socket. A bind failure
    // (e.g. another serve running) is fatal and leaves no half-open TUI.
    let server = spawn_serve_listener(&ws.root)?;

    let produced =
        produce_diff_request(&ws, &resolved.request, &extra, resolved.include_untracked)?;
    // Reloader is installed only with `--watch` (also drives FS auto-reload).
    // Without it, `next-hunk reload` returns a clear server error rather than EOF.
    let reloader = if resolved.watch {
        Some(make_diff_reloader(
            ws.clone(),
            resolved.request.clone(),
            extra,
            resolved.include_untracked,
        ))
    } else {
        None
    };
    // Serve product default: full agent report on quit. Only apply when neither
    // CLI `--export-on-quit` nor config `export_on_quit` was set — explicit
    // `none` must still win. Pager / plain diff keep ResolvedConfig's None.
    let export_on_quit = resolve_serve_export(
        bridge.export_on_quit_override,
        &cfg,
        resolved.export_on_quit,
    );

    open_review_from_produced(
        produced,
        reloader,
        resolved.highlight,
        resolved.line_numbers,
        resolved.wrap,
        resolved.theme,
        resolved.theme_preset,
        resolved.theme_colors,
        resolved.layout,
        Some(ws.root.clone()),
        ReviewOptions {
            focus: bridge.focus,
            notes: bridge.notes,
            // serve exists to collect decisions, so select mode is always on.
            select_mode: true,
            export_on_quit,
            persist_review: resolved.persist_review,
            persist_scope: resolved.scope.as_str().to_string(),
        },
        Some(server),
    )
}

/// `next-hunk push`: send a focus/note update to the running server in this
/// repo. Returns immediately with a short status line.
#[cfg(all(feature = "serve", unix))]
fn run_push(focus: Option<String>, note: Vec<String>) -> Result<()> {
    let focus_target = focus
        .map(|s| next_hunk::cli_parse::parse_focus(&s))
        .transpose()?;
    let notes = note
        .iter()
        .map(|s| next_hunk::cli_parse::parse_note(s))
        .collect::<Result<Vec<_>>>()?;

    let repo = workspace_root_for_socket()?;
    let socket = next_hunk::cli_parse::runtime_socket_path(&repo);

    let command = next_hunk::tui::server::ServerCommand::Push {
        focus: focus_target,
        notes,
    };
    match next_hunk::tui::server::send_command(&socket, &command) {
        Ok(next_hunk::tui::server::ServerReply::Ok) => {
            println!("ok: pushed to running server");
            Ok(())
        }
        Ok(other) => {
            // The server replied with something other than Ok — surface it.
            bail!("unexpected server reply: {other:?}");
        }
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk decision`: read the human's accumulated decisions from the
/// running server, printed as one JSON line on stdout (same shape as
/// `--select` quit output, so an agent parses it identically).
#[cfg(all(feature = "serve", unix))]
fn run_decision() -> Result<()> {
    let repo = workspace_root_for_socket()?;
    let socket = next_hunk::cli_parse::runtime_socket_path(&repo);

    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Decision,
    ) {
        Ok(next_hunk::tui::server::ServerReply::Decisions(selections)) => {
            println!("{}", serde_json::to_string(&selections)?);
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
            bail!("server error: {message}")
        }
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk list`: discover live server sessions.
///
/// With `--all-worktrees`, restrict to worktrees of the current repository and
/// also surface known worktree roots that do not yet have a live `serve`.
#[cfg(all(feature = "serve", unix))]
fn run_list(all_worktrees: bool) -> Result<()> {
    let sessions = next_hunk::cli_parse::discover_live_sockets();
    let current_hash = workspace_root_for_socket()
        .ok()
        .map(|root| next_hunk::cli_parse::runtime_socket_hash(&root));

    if all_worktrees {
        return run_list_all_worktrees(sessions, current_hash.as_deref());
    }

    if sessions.is_empty() {
        println!("no live sessions found");
        return Ok(());
    }
    for (path, hash) in &sessions {
        print_session_line(path, hash, current_hash.as_deref());
    }
    Ok(())
}

/// Filter live sessions to the current repo's worktrees; list idle worktrees too.
#[cfg(all(feature = "serve", unix))]
fn run_list_all_worktrees(
    sessions: Vec<(PathBuf, String)>,
    current_hash: Option<&str>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    // Same VCS discovery as diff/inspect so missing-repo errors match
    // ("not a git or jj workspace") and pure jj workspaces are accepted.
    let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
    let pref = cfg
        .vcs
        .as_deref()
        .map(VcsPreference::parse_str)
        .unwrap_or_default();
    let ws = detect_workspace(&cwd, pref)?;
    let worktree_roots = match ws.kind {
        VcsKind::Git => next_hunk::source::list_repo_worktree_roots(&ws.root)?,
        VcsKind::Jj => {
            // Pure jj has no linked git worktrees; the workspace root is the
            // only session key (matches serve socket hashing).
            let root = std::fs::canonicalize(&ws.root).unwrap_or_else(|_| ws.root.clone());
            vec![root]
        }
    };

    // Expected socket hash → worktree root for this logical repo.
    let mut expected: Vec<(String, PathBuf)> = worktree_roots
        .into_iter()
        .map(|root| {
            let hash = next_hunk::cli_parse::runtime_socket_hash(&root);
            (hash, root)
        })
        .collect();
    // Stable output order by worktree path.
    expected.sort_by(|a, b| a.1.cmp(&b.1));

    let expected_hashes: std::collections::HashSet<&str> =
        expected.iter().map(|(h, _)| h.as_str()).collect();

    let live_for_repo: Vec<&(PathBuf, String)> = sessions
        .iter()
        .filter(|(_, hash)| expected_hashes.contains(hash.as_str()))
        .collect();

    println!(
        "worktrees of this repo: {} total, {} with live serve",
        expected.len(),
        live_for_repo.len()
    );

    // Print live sessions first (rich Info), keyed by hash so we can mark idle ones.
    let mut live_hashes = std::collections::HashSet::new();
    for (path, hash) in &live_for_repo {
        live_hashes.insert(hash.as_str());
        print_session_line(path, hash, current_hash);
    }

    for (hash, root) in &expected {
        if live_hashes.contains(hash.as_str()) {
            continue;
        }
        let marker = if current_hash == Some(hash.as_str()) {
            "  (current, no serve)"
        } else {
            "  (no serve)"
        };
        println!("{}  —  files=-  repo={}{}", hash, root.display(), marker);
    }

    if live_for_repo.is_empty() && expected.is_empty() {
        println!("no worktrees found");
    }
    Ok(())
}

/// One list line: hash, socket, files, absolute worktree root, optional (current).
#[cfg(all(feature = "serve", unix))]
fn print_session_line(path: &std::path::Path, hash: &str, current_hash: Option<&str>) {
    let current = if current_hash == Some(hash) {
        "  (current)"
    } else {
        ""
    };
    let info =
        next_hunk::tui::server::send_command(path, &next_hunk::tui::server::ServerCommand::Info);
    match info {
        Ok(next_hunk::tui::server::ServerReply::Info {
            repo_path,
            file_count,
        }) => {
            println!(
                "{hash}  {}  files={file_count}  repo={repo_path}{current}",
                path.display()
            );
        }
        _ => {
            println!("{hash}  {}{current}", path.display());
        }
    }
}

/// `next-hunk get [hash]`: show info for a specific session.
#[cfg(all(feature = "serve", unix))]
fn run_get(hash: Option<String>) -> Result<()> {
    let socket = resolve_socket(hash)?;
    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Info,
    ) {
        Ok(next_hunk::tui::server::ServerReply::Info {
            repo_path,
            file_count,
        }) => {
            println!("socket: {}", socket.display());
            println!("repo:   {repo_path}");
            println!("files:  {file_count}");
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
            bail!("server error: {message}")
        }
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk review [hash]`: print the review structure as JSON.
#[cfg(all(feature = "serve", unix))]
fn run_review(hash: Option<String>) -> Result<()> {
    let socket = resolve_socket(hash)?;
    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Review,
    ) {
        Ok(next_hunk::tui::server::ServerReply::Review(summary)) => {
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
            bail!("server error: {message}")
        }
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk comment <action>`: manage session comments.
#[cfg(all(feature = "serve", unix))]
fn run_comment(action: CommentAction) -> Result<()> {
    use next_hunk::tui::server::ServerCommand;
    match action {
        CommentAction::Add {
            file,
            line,
            line_end,
            hunk,
            hash,
            text,
        } => {
            let socket = resolve_socket(hash)?;
            match next_hunk::tui::server::send_command(
                &socket,
                &ServerCommand::CommentAdd {
                    file,
                    text,
                    line,
                    line_end,
                    hunk,
                },
            ) {
                Ok(next_hunk::tui::server::ServerReply::CommentAdded { id }) => {
                    println!("ok: comment added with id {id}");
                    Ok(())
                }
                Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
                    bail!("server error: {message}")
                }
                Ok(other) => bail!("unexpected server reply: {other:?}"),
                Err(e) => bail_on_no_server(e),
            }
        }
        CommentAction::List { hash } => {
            let socket = resolve_socket(hash)?;
            match next_hunk::tui::server::send_command(&socket, &ServerCommand::CommentList) {
                Ok(next_hunk::tui::server::ServerReply::CommentList { comments }) => {
                    if comments.is_empty() {
                        println!("no comments");
                    } else {
                        for c in &comments {
                            let loc = match (c.hunk, c.line, c.line_end) {
                                (Some(h), _, _) => format!(" hunk={h}"),
                                (_, Some(l), Some(e)) if e != l => format!(" line={l}-{e}"),
                                (_, Some(l), _) => format!(" line={l}"),
                                _ => String::new(),
                            };
                            println!("{}  {}{}  {}", c.id, c.file, loc, c.text);
                        }
                    }
                    Ok(())
                }
                Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
                    bail!("server error: {message}")
                }
                Ok(other) => bail!("unexpected server reply: {other:?}"),
                Err(e) => bail_on_no_server(e),
            }
        }
        CommentAction::Rm { id, hash } => {
            let socket = resolve_socket(hash)?;
            match next_hunk::tui::server::send_command(&socket, &ServerCommand::CommentRm { id }) {
                Ok(next_hunk::tui::server::ServerReply::Ok) => {
                    println!("ok: comment removed");
                    Ok(())
                }
                Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
                    bail!("server error: {message}")
                }
                Ok(other) => bail!("unexpected server reply: {other:?}"),
                Err(e) => bail_on_no_server(e),
            }
        }
        CommentAction::Apply { hash } => {
            let socket = resolve_socket(hash)?;
            match next_hunk::tui::server::send_command(&socket, &ServerCommand::CommentApply) {
                Ok(next_hunk::tui::server::ServerReply::Ok) => {
                    println!("ok: comments applied to TUI");
                    Ok(())
                }
                Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
                    bail!("server error: {message}")
                }
                Ok(other) => bail!("unexpected server reply: {other:?}"),
                Err(e) => bail_on_no_server(e),
            }
        }
    }
}

/// Resolve a socket path from an optional hash or the current repo.
#[cfg(all(feature = "serve", unix))]
fn resolve_socket(hash: Option<String>) -> Result<PathBuf> {
    if let Some(h) = &hash {
        let sessions = next_hunk::cli_parse::discover_live_sockets();
        let found = sessions.iter().find(|(_, hh)| hh == h);
        match found {
            Some((path, _)) => Ok(path.clone()),
            None => bail!("no live session with hash {h}"),
        }
    } else {
        let repo = workspace_root_for_socket()?;
        Ok(next_hunk::cli_parse::runtime_socket_path(&repo))
    }
}

/// `next-hunk navigate <target> [--hash <hash>]`: navigate a serve TUI.
#[cfg(all(feature = "serve", unix))]
fn run_navigate(target: String, hash: Option<String>) -> Result<()> {
    let focus_target = next_hunk::cli_parse::parse_focus(&target)?;
    let socket = resolve_socket(hash)?;
    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Navigate {
            target: focus_target,
        },
    ) {
        Ok(next_hunk::tui::server::ServerReply::Ok) => {
            println!("ok: navigated to {}", target);
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
            bail!("server error: {message}")
        }
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk reload [--hash <hash>]`: reload the serve session's diff.
#[cfg(all(feature = "serve", unix))]
fn run_reload(hash: Option<String>) -> Result<()> {
    let socket = resolve_socket(hash)?;
    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Reload,
    ) {
        Ok(next_hunk::tui::server::ServerReply::Ok) => {
            println!("ok: session reloaded");
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error { message }) => {
            bail!("server error: {message}")
        }
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// Turn a socket-connect failure into an actionable "no server" message,
/// while letting unrelated errors (e.g. malformed reply) pass through. Takes
/// the error by value so the unrelated-error path can return it as-is.
#[cfg(all(feature = "serve", unix))]
fn bail_on_no_server(err: anyhow::Error) -> Result<()> {
    // send_command wraps the connect step with "connect to server socket …".
    // A missing socket surfaces as a NotFound/WouldBlock/ConnectionRefused
    // underneath; we match on the textual context to stay decoupled from the
    // exact io::ErrorKind across platforms.
    let msg = format!("{err:#}");
    if msg.contains("connect to server socket") {
        bail!("no next-hunk server running in this repo; start one with `next-hunk serve`");
    }
    Err(err)
}

// --- serve-feature plumbing ------------------------------------------------
// On builds with the `serve` feature (default), bind the repo's runtime socket
// and wire the listener into the TUI. On other builds the subcommands exist in
// the CLI surface but report unavailability at runtime — matching how `watch`
// advertises itself when compiled out.

/// Bind the server socket for `serve`. The path is derived from the worktree
/// root so a `push`/`decision` in the same worktree finds it without a flag.
/// `ServerArg` is a type alias for `ServerListener` under the `serve` feature,
/// so we return the listener directly.
#[cfg(all(feature = "serve", unix))]
fn spawn_serve_listener(repo: &std::path::Path) -> Result<next_hunk::tui::ServerArg> {
    let socket = next_hunk::cli_parse::runtime_socket_path(repo);
    next_hunk::tui::server::ServerListener::spawn(socket)
}

#[cfg(not(all(feature = "serve", unix)))]
#[allow(clippy::too_many_arguments)] // mirrors Commands::Serve field set
fn run_serve(
    _staged: bool,
    _all: bool,
    _base: Option<String>,
    _range: Option<String>,
    _strategy: Option<DiffStrategy>,
    _watch: bool,
    _no_highlight: bool,
    _include_untracked: bool,
    _focus: Option<String>,
    _note: Vec<String>,
    _export_on_quit: Option<ExportOnQuit>,
    _layout: Option<LayoutMode>,
    _theme_preset: Option<String>,
    _vcs: Option<VcsPreference>,
    _no_persist: bool,
    _extra: Vec<String>,
) -> Result<()> {
    bail!("{}", next_hunk::platform::live_session_unavailable("serve"));
}

/// Sanitize a free-form string for use in a persist scope filename.
fn sanitize_persist_scope(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(64));
    for c in s.chars().take(64) {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "rev".into()
    } else {
        out
    }
}

/// clap value_parser for `--layout`. Accepts unified|stack|split (case-insensitive).
fn parse_vcs_arg(s: &str) -> Result<VcsPreference, String> {
    match s.trim().to_lowercase().as_str() {
        "auto" => Ok(VcsPreference::Auto),
        "git" => Ok(VcsPreference::Git),
        "jj" | "jujutsu" => Ok(VcsPreference::Jj),
        other => Err(format!("unknown vcs '{other}', expected auto, git, or jj")),
    }
}

fn parse_layout_arg(s: &str) -> Result<LayoutMode, String> {
    LayoutMode::try_parse(s)
}

/// clap value_parser for `--strategy`.
fn parse_strategy_arg(s: &str) -> Result<DiffStrategy, String> {
    DiffStrategy::try_parse(s)
}

/// clap value_parser for `--export-on-quit`. Accepts none|json|markdown|both.
fn parse_export_on_quit_arg(s: &str) -> Result<ExportOnQuit, String> {
    ExportOnQuit::try_parse(s)
}

/// clap value_parser for `--theme-preset`.
fn parse_theme_preset_arg(s: &str) -> Result<String, String> {
    // Validate via the same allow-list as config; return the canonical name.
    let preset = next_hunk::tui::theme::ThemePreset::try_parse(s)?;
    Ok(preset.name().to_string())
}

/// CLI `--theme-preset` wins; otherwise config `theme_preset`.
fn resolve_theme_preset_opt(cli: Option<String>, cfg: &Config) -> Result<Option<String>> {
    if let Some(s) = cli {
        return Ok(Some(s));
    }
    match cfg.theme_preset.as_deref() {
        Some(s) => {
            let preset = next_hunk::tui::theme::ThemePreset::try_parse(s)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Some(preset.name().to_string()))
        }
        None => Ok(None),
    }
}

/// CLI layout flag (if any) wins; otherwise parse config strictly.
fn resolve_layout_opt(cli: Option<LayoutMode>, cfg: &Config) -> Result<LayoutMode> {
    match cli {
        Some(l) => Ok(l),
        None => match cfg.layout.as_deref() {
            Some(s) => LayoutMode::try_parse(s).map_err(|e| anyhow::anyhow!("{e}")),
            None => Ok(LayoutMode::Unified),
        },
    }
}

/// CLI export flag (if any) wins; otherwise parse config strictly.
fn resolve_export_opt(cli: Option<ExportOnQuit>, cfg: &Config) -> Result<ExportOnQuit> {
    match cli {
        Some(e) => Ok(e),
        None => match cfg.export_on_quit.as_deref() {
            Some(s) => ExportOnQuit::try_parse(s).map_err(|e| anyhow::anyhow!("{e}")),
            None => Ok(ExportOnQuit::None),
        },
    }
}

/// Serve export default: `json` when neither CLI nor config set a value.
/// Explicit `none` (CLI or config) is preserved. Pager/diff do not use this.
/// Gated with the serve+unix call site so no-default-features / non-Unix
/// builds do not emit `dead_code`; kept under `test` so unit tests still run.
#[cfg(any(all(feature = "serve", unix), test))]
fn resolve_serve_export(
    cli_override: Option<ExportOnQuit>,
    cfg: &Config,
    resolved: ExportOnQuit,
) -> ExportOnQuit {
    if cli_override.is_some() || cfg.export_on_quit.is_some() {
        resolved
    } else {
        ExportOnQuit::Json
    }
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_push(_focus: Option<String>, _note: Vec<String>) -> Result<()> {
    bail!("{}", next_hunk::platform::live_session_unavailable("push"));
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_decision() -> Result<()> {
    bail!(
        "{}",
        next_hunk::platform::live_session_unavailable("decision")
    )
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_list(_all_worktrees: bool) -> Result<()> {
    bail!("{}", next_hunk::platform::live_session_unavailable("list"));
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_get(_hash: Option<String>) -> Result<()> {
    bail!("{}", next_hunk::platform::live_session_unavailable("get"));
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_review(_hash: Option<String>) -> Result<()> {
    bail!(
        "{}",
        next_hunk::platform::live_session_unavailable("review")
    );
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_navigate(_target: String, _hash: Option<String>) -> Result<()> {
    bail!(
        "{}",
        next_hunk::platform::live_session_unavailable("navigate")
    );
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_comment(_action: CommentAction) -> Result<()> {
    bail!(
        "{}",
        next_hunk::platform::live_session_unavailable("comment")
    );
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_reload(_hash: Option<String>) -> Result<()> {
    bail!(
        "{}",
        next_hunk::platform::live_session_unavailable("reload")
    );
}

#[allow(clippy::too_many_arguments)]
fn open_review_from_produced(
    produced: next_hunk::source::ProducedDiff,
    reloader: Option<next_hunk::tui::Reloader>,
    highlight_on: bool,
    line_numbers_on: bool,
    wrap_on: bool,
    theme: Option<String>,
    theme_preset: Option<String>,
    theme_colors: Option<next_hunk::config::ThemeColorsConfig>,
    layout: next_hunk::config::LayoutMode,
    workdir: Option<PathBuf>,
    options: ReviewOptions,
    server: Option<next_hunk::tui::ServerArg>,
) -> Result<()> {
    open_review_from_text(
        &produced.text,
        &produced.origins,
        reloader,
        highlight_on,
        line_numbers_on,
        wrap_on,
        theme,
        theme_preset,
        theme_colors,
        layout,
        workdir,
        options,
        server,
    )
}

// Mirrors the layered open path (diff text + origins + TUI knobs + server);
// packing into a struct would only rename the same surface area.
#[allow(clippy::too_many_arguments)]
fn open_review_from_text(
    text: &str,
    origins: &[FileOrigin],
    reloader: Option<next_hunk::tui::Reloader>,
    highlight_on: bool,
    line_numbers_on: bool,
    wrap_on: bool,
    theme: Option<String>,
    theme_preset: Option<String>,
    theme_colors: Option<next_hunk::config::ThemeColorsConfig>,
    layout: next_hunk::config::LayoutMode,
    workdir: Option<PathBuf>,
    options: ReviewOptions,
    server: Option<next_hunk::tui::ServerArg>,
) -> Result<()> {
    if text.trim().is_empty() {
        eprintln!("(empty diff)");
        return Ok(());
    }
    let mut review = parse_review(text)?;
    review.apply_file_origins(origins);
    let select_mode = options.select_mode;
    let export_on_quit = options.export_on_quit;
    // Interactive TUI (Phase 2). If stdout is not a terminal (piped, e.g. when
    // used as git's pager in a pipeline or scripted in CI), or if opening the
    // TUI fails for any other reason, fall back to a short inspect summary so
    // the CLI path stays usable.
    //
    // We check `is_terminal()` explicitly rather than relying on crossterm to
    // error out, because on Windows crossterm's console calls can *block* on a
    // non-console (pipe) stdout instead of returning promptly - which would
    // hang the process (observed: `pager`/`patch -` deadlocked indefinitely in
    // piped integration tests). The upfront check is portable and avoids that.
    //
    // Agent-bridge flags must never be silently discarded on this path: callers
    // that pass --focus/--select are rejected earlier via
    // `parse_agent_bridge_options`. Defend in depth here too.
    //
    // `--export-on-quit` is the intentional headless agent bridge: with no TUI
    // to quit, emit the report immediately (all hunks undecided + notes).
    // Never fall through to the inspect summary when export was requested —
    // that silently substitutes unparseable text for the documented report.
    if !std::io::stdout().is_terminal() {
        if options.select_mode {
            bail!("--select requires an interactive terminal (stdout is not a tty)");
        }
        if let Some(ref focus) = options.focus {
            // Non-TTY cannot open the TUI, so a focus miss would be invisible.
            // Fail with a clear message (path + "not found") rather than the
            // generic "requires tty" only — agents need the miss reason.
            if next_hunk::tui::app::resolve_focus_row(&review, focus).is_none() {
                bail!(
                    "focus not found: {} (and stdout is not a tty)",
                    next_hunk::tui::app::focus_display(focus)
                );
            }
            bail!("--focus requires an interactive terminal (stdout is not a tty)");
        }
        if export_on_quit != ExportOnQuit::None {
            // Notes are part of the export report; allowed headless.
            let report = ReviewReport::from_review_undecided(&review, &options.notes);
            emit_quit_report(&report, select_mode, export_on_quit, workdir.as_deref())?;
            return Ok(());
        }
        if !options.notes.is_empty() {
            bail!("--note requires an interactive terminal (stdout is not a tty)");
        }
        print_inspect(&review);
        return Ok(());
    }
    match run_review_tui(
        review.clone(),
        reloader,
        highlight_on,
        line_numbers_on,
        wrap_on,
        theme,
        theme_preset,
        theme_colors,
        layout,
        workdir.clone(),
        options,
        server,
    ) {
        Ok(report) => {
            emit_quit_report(&report, select_mode, export_on_quit, workdir.as_deref())?;
            Ok(())
        }
        Err(err) => {
            eprintln!("note: {err}");
            print_inspect(&review);
            Ok(())
        }
    }
}

/// Print the quit-time report for agents / humans.
///
/// - `export_on_quit = none` + `--select`: legacy decisions-only JSON (compatible).
/// - `export_on_quit = json|markdown|both`: full report (decisions + comments + notes),
///   even when not in `--select` mode.
///
/// When select or export is active, also caches the **full** report under
/// `.git/next-hunk/last-export.json` for `next-hunk last-export`.
fn emit_quit_report(
    report: &ReviewReport,
    select_mode: bool,
    export: ExportOnQuit,
    workdir: Option<&std::path::Path>,
) -> Result<()> {
    if select_mode || export != ExportOnQuit::None {
        if let Err(e) = next_hunk::tui::persist::save_last_export(workdir, report) {
            eprintln!("warning: cannot cache last export: {e}");
        }
        // Overlay / external launchers set NEXT_HUNK_EXPORT_PATH so the parent
        // process can read the full report after a tmux/zellij popup closes
        // (popup stdout is not the caller's pipe). Write full JSON and skip
        // printing to this process's stdout — the parent emits once.
        if let Ok(path) = std::env::var(next_hunk::overlay::EXPORT_PATH_ENV) {
            if !path.is_empty() {
                match std::fs::write(&path, serde_json::to_string(report)?) {
                    Ok(()) => return Ok(()),
                    Err(e) => eprintln!(
                        "warning: cannot write {}={path}: {e}",
                        next_hunk::overlay::EXPORT_PATH_ENV
                    ),
                }
            }
        }
    }
    match export {
        ExportOnQuit::None => {
            if select_mode {
                // Backward-compatible: only the three decision buckets.
                println!("{}", serde_json::to_string(&report.as_selections())?);
            }
        }
        ExportOnQuit::Json => {
            println!("{}", serde_json::to_string(report)?);
        }
        ExportOnQuit::Markdown => {
            print!("{}", report.to_markdown());
        }
        ExportOnQuit::Both => {
            println!("{}", serde_json::to_string(report)?);
            print!("{}", report.to_markdown());
        }
    }
    Ok(())
}

/// `next-hunk last-export`: print the cached full review report from the last
/// select/export quit in this worktree. Does not require a live serve.
fn run_last_export() -> Result<()> {
    let cwd = std::env::current_dir()?;
    match next_hunk::tui::persist::load_last_export(Some(&cwd)) {
        Some(report) => {
            println!("{}", serde_json::to_string(&report)?);
            Ok(())
        }
        None => bail!(
            "no last-export found for this worktree \
             (quit a `serve` / `--select` / `--export-on-quit` session first)"
        ),
    }
}

fn parse_review(text: &str) -> Result<Review> {
    parse_unified_diff(text).context("failed to parse unified diff")
}

fn print_inspect(review: &Review) {
    println!(
        "files={} stream_rows={} arena_bytes={}",
        review.file_count(),
        review.stream_len,
        review.text_arena.len()
    );
    for (i, file) in review.files.iter().enumerate() {
        let origin = file
            .origin
            .map(|o| format!(" [{}]", o.mark()))
            .unwrap_or_default();
        println!(
            "  [{i}] {}{}  hunks={} stream=[{}+{})",
            file.display_path,
            origin,
            file.hunks.len(),
            file.stream_start,
            file.stream_len
        );
    }
}

fn read_patch_input(path: &std::path::Path) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .context("read patch from stdin")?;
        return Ok(buf);
    }
    if !path.exists() {
        bail!("patch file not found: {}", path.display());
    }
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

#[cfg(test)]
mod serve_export_defaults {
    use super::*;

    #[test]
    fn serve_defaults_to_json_when_unset() {
        let cfg = Config::default();
        assert_eq!(
            resolve_serve_export(None, &cfg, ExportOnQuit::None),
            ExportOnQuit::Json
        );
    }

    #[test]
    fn serve_respects_config_none() {
        let cfg = Config {
            export_on_quit: Some("none".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_serve_export(None, &cfg, ExportOnQuit::None),
            ExportOnQuit::None
        );
    }

    #[test]
    fn serve_respects_cli_override() {
        let cfg = Config::default();
        assert_eq!(
            resolve_serve_export(Some(ExportOnQuit::Both), &cfg, ExportOnQuit::Both),
            ExportOnQuit::Both
        );
        assert_eq!(
            resolve_serve_export(Some(ExportOnQuit::None), &cfg, ExportOnQuit::None),
            ExportOnQuit::None
        );
    }
}
