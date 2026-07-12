//! next-hunk CLI entry.

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use next_hunk::config::{CliFlags, Config, ResolvedConfig};
use next_hunk::ir::{parse_unified_diff, Review};
use next_hunk::source::{find_repo, git_diff, git_show};
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
    /// Review working-tree (or staged) diff.
    Diff {
        /// Review staged changes (`git diff --cached`).
        #[arg(long, short = 's')]
        staged: bool,
        /// Re-run on filesystem changes (live reload). Requires the `watch`
        /// feature; otherwise reports that it is unavailable.
        #[arg(long)]
        watch: bool,
        /// Disable syntax highlighting (overrides config/highlight default).
        #[arg(long)]
        no_highlight: bool,
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
        /// Optional pathspecs to limit the review.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra: Vec<String>,
    },
    /// Review a commit or range (`git show` / `git diff A..B`).
    Show {
        /// Revision or range (e.g. HEAD, main..HEAD).
        rev: String,
    },
    /// Review a unified patch from a file or stdin (`-`).
    Patch {
        /// Path to patch file, or `-` for stdin.
        path: PathBuf,
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
        #[arg(long, short = 's')]
        staged: bool,
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
        watch: false,
        no_highlight: false,
        focus: None,
        note: Vec::new(),
        select: false,
        extra: Vec::new(),
    }) {
        Commands::Diff {
            staged,
            watch,
            no_highlight,
            focus,
            note,
            select,
            extra,
        } => {
            // Parse the agent-bridge specs and check the --select tty
            // requirement BEFORE touching the repo, so a bad spec or a
            // non-interactive --select fails fast with a clear message (an
            // agent scripting this gets actionable feedback, not a git error).
            let focus_target = focus
                .map(|s| next_hunk::cli_parse::parse_focus(&s))
                .transpose()?;
            let notes = note
                .iter()
                .map(|s| next_hunk::cli_parse::parse_note(s))
                .collect::<Result<Vec<_>>>()?;

            // `--select` is a blocking interactive gate; it can't run without
            // a real terminal.
            if select && !std::io::stdout().is_terminal() {
                bail!("--select requires an interactive terminal (stdout is not a tty)");
            }

            let cwd = std::env::current_dir()?;
            let repo = find_repo(&cwd)?;

            // Layered config: project (.next-hunk/config.toml) > user
            // (~/.config/next-hunk/config.toml). CLI flags override on top.
            let cfg = Config::load(&cwd);
            let resolved = ResolvedConfig::resolve(
                &cfg,
                &CliFlags {
                    staged: if staged { Some(true) } else { None },
                    watch: if watch { Some(true) } else { None },
                    highlight: if no_highlight { Some(false) } else { None },
                },
            );

            if resolved.watch && !next_hunk::tui::watch::Watcher::is_enabled() {
                eprintln!(
                    "note: `--watch` requires the `watch` feature (rebuild with --features watch)"
                );
            }

            let text = git_diff(&repo, resolved.staged, &extra)?;
            let reloader = if resolved.watch {
                Some(make_diff_reloader(repo.clone(), resolved.staged, extra))
            } else {
                None
            };
            open_review_from_text(
                &text,
                reloader,
                resolved.highlight,
                resolved.theme,
                Some(repo),
                ReviewOptions {
                    focus: focus_target,
                    notes,
                    select_mode: select,
                },
            )
        }
        Commands::Show { rev } => {
            let cwd = std::env::current_dir()?;
            let repo = find_repo(&cwd)?;
            let text = git_show(&repo, &rev)?;
            // `show` is a one-shot snapshot: no watch, highlight default on.
            // Honor the user/project theme config even for `show`.
            let cfg = Config::load(&cwd);
            open_review_from_text(
                &text,
                None,
                true,
                cfg.theme,
                Some(repo),
                ReviewOptions::default(),
            )
        }
        Commands::Patch { path } => {
            let text = read_patch_input(&path)?;
            open_review_from_text(&text, None, true, None, None, ReviewOptions::default())
        }
        Commands::Inspect { path, staged } => {
            let text = if let Some(path) = path {
                read_patch_input(&path)?
            } else {
                let repo = find_repo(&std::env::current_dir()?)?;
                git_diff(&repo, staged, &[])?
            };
            if text.trim().is_empty() {
                println!("files=0 stream_rows=0 arena_bytes=0");
                return Ok(());
            }
            let review = parse_review(&text)?;
            print_inspect(&review);
            Ok(())
        }
        Commands::Pager => {
            // Git pipes the diff to our stdin. Honor the user/project theme.
            let cwd = std::env::current_dir()?;
            let cfg = Config::load(&cwd);
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
            let workdir = find_repo(&cwd).ok();
            open_review_from_text(&buf, None, true, cfg.theme, workdir, ReviewOptions::default())
        }
    }
}

/// Build the live-reload closure for `--watch`: re-runs the same git diff.
/// Captures the repo path, staged flag, and pathspecs by value.
fn make_diff_reloader(repo: PathBuf, staged: bool, extra: Vec<String>) -> next_hunk::tui::Reloader {
    Box::new(move || git_diff(&repo, staged, &extra).context("re-run git diff for --watch"))
}

fn open_review_from_text(
    text: &str,
    reloader: Option<next_hunk::tui::Reloader>,
    highlight_on: bool,
    theme: Option<String>,
    workdir: Option<PathBuf>,
    options: ReviewOptions,
) -> Result<()> {
    if text.trim().is_empty() {
        eprintln!("(empty diff)");
        return Ok(());
    }
    let review = parse_review(text)?;
    let select_mode = options.select_mode;
    // Interactive TUI (Phase 2). If it fails (e.g. stdout is not a tty),
    // fall back to a short inspect summary so the CLI path stays usable.
    match run_review_tui(
        review.clone(),
        reloader,
        highlight_on,
        theme,
        workdir,
        options,
    ) {
        Ok(selections) => {
            // In --select mode the human's per-hunk decisions go to stdout as
            // JSON for the agent to parse. Outside --select, silently drop the
            // (empty) selections.
            if select_mode {
                println!("{}", serde_json::to_string(&selections)?);
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("note: {err}");
            print_inspect(&review);
            Ok(())
        }
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
        println!(
            "  [{i}] {}  hunks={} stream=[{}+{})",
            file.display_path,
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
