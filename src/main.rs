//! next-hunk CLI entry.

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use next_hunk::config::{
    CliFlags, Config, DiffScope, ExportOnQuit, LayoutMode, ResolvedConfig, VcsPreference,
};
use next_hunk::ir::{parse_unified_diff, FileOrigin, Review};
use next_hunk::source::{
    detect_workspace, produce_diff, produce_file_diff, produce_show, Workspace,
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
    /// Review working-tree (or staged / full working-set) diff.
    Diff {
        /// Review staged changes (`git diff --cached`).
        #[arg(long, short = 's', conflicts_with = "all")]
        staged: bool,
        /// Review the full working set: staged + unstaged (+ optional untracked).
        ///
        /// One command to see everything `git status` would list as local
        /// changes. File rail marks origins as `S` staged / `M` modified /
        /// `?` untracked when `--include-untracked` is set.
        #[arg(long, short = 'a', conflicts_with = "staged")]
        all: bool,
        /// Re-run on filesystem changes (live reload). Requires the `watch`
        /// feature; otherwise reports that it is unavailable.
        #[arg(long)]
        watch: bool,
        /// Disable syntax highlighting (overrides config/highlight default).
        #[arg(long)]
        no_highlight: bool,
        /// Include untracked files in worktree / working-set diff (default: off).
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
        /// Diff stream layout: `unified` (default), `stack`, or `split`.
        /// Overrides `layout` from config.toml.
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
        /// VCS backend: `auto` (default), `git`, or `jj`. Overrides `vcs` in config.
        #[arg(long, value_parser = parse_vcs_arg)]
        vcs: Option<VcsPreference>,
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
        /// Diff stream layout: `unified`, `stack`, or `split`.
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
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
        /// Diff stream layout: `unified`, `stack`, or `split`.
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
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
        /// Diff stream layout: `unified`, `stack`, or `split`.
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
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
        #[arg(long, short = 's', conflicts_with = "all")]
        staged: bool,
        /// Full working set (staged + unstaged); see `diff --all`.
        #[arg(long, short = 'a', conflicts_with = "staged")]
        all: bool,
        /// Include untracked files when reviewing the worktree / working-set.
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
    Serve {
        /// Review staged changes (`git diff --cached`).
        #[arg(long, short = 's', conflicts_with = "all")]
        staged: bool,
        /// Review the full working set: staged + unstaged (+ optional untracked).
        #[arg(long, short = 'a', conflicts_with = "staged")]
        all: bool,
        /// Re-run on filesystem changes (live reload). Requires the `watch`
        /// feature.
        #[arg(long)]
        watch: bool,
        /// Disable syntax highlighting (overrides config/highlight default).
        #[arg(long)]
        no_highlight: bool,
        /// Include untracked files in worktree / working-set diff (default: off).
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
        /// Diff stream layout: `unified` (default), `stack`, or `split`.
        /// Overrides `layout` from config.toml.
        #[arg(long, value_parser = parse_layout_arg)]
        layout: Option<LayoutMode>,
        /// VCS backend: `auto` (default), `git`, or `jj`.
        #[arg(long, value_parser = parse_vcs_arg)]
        vcs: Option<VcsPreference>,
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
    /// List live next-hunk server sessions.
    ///
    /// Scans well-known socket directories for live servers. Prints one line
    /// per live session: `<repo-hash>  <socket-path>`.
    List,
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
}

#[derive(Debug, clap::Subcommand)]
enum CommentAction {
    /// Add a comment to a file.
    Add {
        /// Target file path.
        #[arg(long)]
        file: String,
        /// Optional new-side source line number.
        #[arg(long)]
        line: Option<u32>,
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
        watch: false,
        no_highlight: false,
        include_untracked: false,
        focus: None,
        note: Vec::new(),
        select: false,
        export_on_quit: None,
        layout: None,
        vcs: None,
        extra: Vec::new(),
    }) {
        Commands::Diff {
            staged,
            all,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            select,
            export_on_quit,
            layout,
            vcs,
            extra,
        } => {
            // Parse the agent-bridge specs and check the interactive-tty
            // requirement BEFORE touching the repo, so a bad spec or a
            // non-interactive agent-bridge flag fails fast with a clear
            // message (not a git error).
            let options = parse_agent_bridge_options(focus, note, select, export_on_quit)?;

            let cwd = std::env::current_dir()?;

            // Layered config: project (.next-hunk/config.toml) > user
            // (~/.config/next-hunk/config.toml). CLI flags override on top.
            let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
            let resolved = ResolvedConfig::resolve(
                &cfg,
                &CliFlags {
                    staged: if staged { Some(true) } else { None },
                    all: if all { Some(true) } else { None },
                    watch: if watch { Some(true) } else { None },
                    highlight: if no_highlight { Some(false) } else { None },
                    include_untracked: if include_untracked { Some(true) } else { None },
                    layout,
                    export_on_quit: options.export_on_quit_override,
                    vcs,
                },
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;

            let ws = detect_workspace(&cwd, resolved.vcs)?;

            if resolved.watch && !next_hunk::tui::watch::Watcher::is_enabled() {
                eprintln!(
                    "note: `--watch` requires the `watch` feature (rebuild with --features watch)"
                );
            }

            let produced = produce_diff(&ws, resolved.scope, &extra, resolved.include_untracked)?;
            let reloader = if resolved.watch {
                Some(make_diff_reloader(
                    ws.clone(),
                    resolved.scope,
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
                resolved.layout,
                Some(ws.root.clone()),
                ReviewOptions {
                    focus: options.focus,
                    notes: options.notes,
                    select_mode: options.select,
                    export_on_quit: resolved.export_on_quit,
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
            open_review_from_text(
                &text,
                &[],
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                resolved_layout,
                Some(ws.root),
                ReviewOptions {
                    focus: bridge.focus,
                    notes: bridge.notes,
                    select_mode: bridge.select,
                    export_on_quit: export,
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
        } => {
            let bridge = parse_agent_bridge_options(focus, note, select, export_on_quit)?;
            let text = read_patch_input(&path)?;
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
            let resolved_layout = resolve_layout_opt(layout, &cfg)?;
            let export = resolve_export_opt(bridge.export_on_quit_override, &cfg)?;
            open_review_from_text(
                &text,
                &[],
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                resolved_layout,
                None,
                ReviewOptions {
                    focus: bridge.focus,
                    notes: bridge.notes,
                    select_mode: bridge.select,
                    export_on_quit: export,
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
            open_review_from_text(
                &text,
                &[],
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                resolved_layout,
                workdir,
                ReviewOptions {
                    focus: bridge.focus,
                    notes: bridge.notes,
                    select_mode: bridge.select,
                    export_on_quit: export,
                },
                None,
            )
        }
        Commands::Inspect {
            path,
            staged,
            all,
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
                let pref = vcs
                    .or_else(|| cfg.vcs.as_deref().map(VcsPreference::parse_str))
                    .unwrap_or_default();
                let ws = detect_workspace(&cwd, pref)?;
                let scope = if all {
                    DiffScope::WorkingSet
                } else if staged {
                    DiffScope::Staged
                } else {
                    DiffScope::Worktree
                };
                let produced = produce_diff(&ws, scope, &[], include_untracked)?;
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
                resolved_layout,
                workdir,
                ReviewOptions {
                    export_on_quit: export,
                    ..Default::default()
                },
                None,
            )
        }
        Commands::Serve {
            staged,
            all,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            export_on_quit,
            layout,
            vcs,
            extra,
        } => run_serve(
            staged,
            all,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            export_on_quit,
            layout,
            vcs,
            extra,
        ),
        Commands::Push { focus, note } => run_push(focus, note),
        Commands::Decision => run_decision(),
        Commands::List => run_list(),
        Commands::Get { hash } => run_get(hash),
        Commands::Review { hash } => run_review(hash),
        Commands::Navigate { target, hash } => run_navigate(target, hash),
        Commands::Comment { action } => run_comment(action),
        Commands::Reload { hash } => run_reload(hash),
    }
}

/// Parsed agent-bridge flags shared by `diff` / `show` / `patch` / `filediff`.
struct AgentBridgeOptions {
    focus: Option<next_hunk::tui::app::FocusTarget>,
    notes: Vec<next_hunk::tui::app::Note>,
    select: bool,
    /// CLI `--export-on-quit` when present (config still layers underneath).
    export_on_quit_override: Option<ExportOnQuit>,
}

/// Parse `--focus` / `--note` / `--select` and refuse agent-bridge interactive
/// flags when stdout is not a tty — never silently drop focus/notes.
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
    if !std::io::stdout().is_terminal() {
        if select {
            bail!("--select requires an interactive terminal (stdout is not a tty)");
        }
        if focus.is_some() {
            bail!("--focus requires an interactive terminal (stdout is not a tty)");
        }
        if !notes.is_empty() {
            bail!("--note requires an interactive terminal (stdout is not a tty)");
        }
    }

    Ok(AgentBridgeOptions {
        focus,
        notes,
        select,
        export_on_quit_override: export_on_quit,
    })
}

/// Build the live-reload closure for `--watch`: re-runs the same VCS diff.
/// Captures the workspace, scope, pathspecs, and untracked flag by value.
fn make_diff_reloader(
    ws: Workspace,
    scope: DiffScope,
    extra: Vec<String>,
    include_untracked: bool,
) -> next_hunk::tui::Reloader {
    Box::new(move || {
        produce_diff(&ws, scope, &extra, include_untracked).context("re-run VCS diff for --watch")
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
    watch: bool,
    no_highlight: bool,
    include_untracked: bool,
    focus: Option<String>,
    note: Vec<String>,
    export_on_quit: Option<ExportOnQuit>,
    layout: Option<LayoutMode>,
    vcs: Option<VcsPreference>,
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
    let resolved = ResolvedConfig::resolve(
        &cfg,
        &CliFlags {
            staged: if staged { Some(true) } else { None },
            all: if all { Some(true) } else { None },
            watch: if watch { Some(true) } else { None },
            highlight: if no_highlight { Some(false) } else { None },
            include_untracked: if include_untracked { Some(true) } else { None },
            layout,
            export_on_quit: bridge.export_on_quit_override,
            vcs,
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let ws = detect_workspace(&cwd, resolved.vcs)?;

    if resolved.watch && !next_hunk::tui::watch::Watcher::is_enabled() {
        eprintln!("note: `--watch` requires the `watch` feature (rebuild with --features watch)");
    }

    // Bind the server socket before opening the TUI, so a `push`/`decision`
    // issued the instant the TUI appears finds a live socket. A bind failure
    // (e.g. another serve running) is fatal and leaves no half-open TUI.
    let server = spawn_serve_listener(&ws.root)?;

    let produced = produce_diff(&ws, resolved.scope, &extra, resolved.include_untracked)?;
    // Reloader is installed only with `--watch` (also drives FS auto-reload).
    // Without it, `next-hunk reload` returns a clear server error rather than EOF.
    let reloader = if resolved.watch {
        Some(make_diff_reloader(
            ws.clone(),
            resolved.scope,
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
        resolved.layout,
        Some(ws.root.clone()),
        ReviewOptions {
            focus: bridge.focus,
            notes: bridge.notes,
            // serve exists to collect decisions, so select mode is always on.
            select_mode: true,
            export_on_quit: resolved.export_on_quit,
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
#[cfg(all(feature = "serve", unix))]
fn run_list() -> Result<()> {
    let sessions = next_hunk::cli_parse::discover_live_sockets();
    if sessions.is_empty() {
        println!("no live sessions found");
        return Ok(());
    }
    for (path, hash) in &sessions {
        // Try to get session info for richer output.
        let info = next_hunk::tui::server::send_command(
            path,
            &next_hunk::tui::server::ServerCommand::Info,
        );
        match info {
            Ok(next_hunk::tui::server::ServerReply::Info {
                repo_path,
                file_count,
            }) => {
                println!(
                    "{}  {}  files={file_count}  repo={repo_path}",
                    hash,
                    path.display()
                );
            }
            _ => {
                println!("{}  {}", hash, path.display());
            }
        }
    }
    Ok(())
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
                            let loc = match (c.hunk, c.line) {
                                (Some(h), _) => format!(" hunk={h}"),
                                (_, Some(l)) => format!(" line={l}"),
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

/// Bind the server socket for `serve`. The path is derived from the repo root
/// so a `push`/`decision` in the same repo finds it without an explicit flag.
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
    _watch: bool,
    _no_highlight: bool,
    _include_untracked: bool,
    _focus: Option<String>,
    _note: Vec<String>,
    _export_on_quit: Option<ExportOnQuit>,
    _layout: Option<LayoutMode>,
    _vcs: Option<VcsPreference>,
    _extra: Vec<String>,
) -> Result<()> {
    bail!("`serve` requires the `serve` feature on a Unix OS (rebuild with --features serve)");
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

/// clap value_parser for `--export-on-quit`. Accepts none|json|markdown|both.
fn parse_export_on_quit_arg(s: &str) -> Result<ExportOnQuit, String> {
    ExportOnQuit::try_parse(s)
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

#[cfg(not(all(feature = "serve", unix)))]
fn run_push(_focus: Option<String>, _note: Vec<String>) -> Result<()> {
    bail!("`push` requires the `serve` feature on a Unix OS (rebuild with --features serve)");
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_decision() -> Result<()> {
    bail!("`decision` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_list() -> Result<()> {
    bail!("`list` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_get(_hash: Option<String>) -> Result<()> {
    bail!("`get` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_review(_hash: Option<String>) -> Result<()> {
    bail!("`review` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_navigate(_target: String, _hash: Option<String>) -> Result<()> {
    bail!("`navigate` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_comment(_action: CommentAction) -> Result<()> {
    bail!("`comment` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_reload(_hash: Option<String>) -> Result<()> {
    bail!("`reload` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[allow(clippy::too_many_arguments)]
fn open_review_from_produced(
    produced: next_hunk::source::ProducedDiff,
    reloader: Option<next_hunk::tui::Reloader>,
    highlight_on: bool,
    line_numbers_on: bool,
    wrap_on: bool,
    theme: Option<String>,
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
    // that pass --focus/--note/--select are rejected earlier via
    // `parse_agent_bridge_options`. Defend in depth here too.
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
        layout,
        workdir,
        options,
        server,
    ) {
        Ok(report) => {
            emit_quit_report(&report, select_mode, export_on_quit)?;
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
fn emit_quit_report(report: &ReviewReport, select_mode: bool, export: ExportOnQuit) -> Result<()> {
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
