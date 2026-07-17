//! Interactive review TUI.
//!
//! Layered so the interaction model is headless-testable:
//! - [`app`] — pure state + key handling (`App::handle_key`), no I/O
//! - [`view`] — pure rendering from `&App`
//! - [`input`] — thin crossterm event reader
//! - [`run_review_tui`] — the only terminal-touching entry point
//!
//! The run loop keeps the input path synchronous and short (architecture §2.3):
//! poll → handle_key → draw. No work happens per frame beyond the viewport
//! materialization in [`view`].

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::ir::Review;
use crate::tui::app::{App, ReviewReport};
use crate::tui::watch::{Watcher, DEBOUNCE};

pub mod app;
pub mod input;
pub mod server;
pub mod theme;
pub mod view;
pub mod watch;

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// A live-reload source: produces a fresh unified-diff string on demand.
/// Carried into the run loop so `--watch` can re-fetch the diff without
/// touching the terminal from a background thread.
/// Live-reload callback. Returns fresh unified-diff text plus optional
/// per-file origins (`S`/`M`/`?`) for working-set reviews.
pub type Reloader = Box<dyn FnMut() -> Result<crate::source::ProducedDiff>>;

/// Agent-bridge options threaded from the CLI into the run loop. All fields are
/// optional/empty by default, so callers that don't care about the agent
/// features (`--focus` / `--note` / `--select`) construct `ReviewOptions::default()`.
#[derive(Debug, Default, Clone)]
pub struct ReviewOptions {
    /// `--focus`: scroll here on startup.
    pub focus: Option<app::FocusTarget>,
    /// `--note`: agent annotations to render.
    pub notes: Vec<app::Note>,
    /// `--select`: enable the per-hunk accept/reject gate; emit JSON on quit.
    pub select_mode: bool,
    /// `export_on_quit` config / `--export-on-quit`: emit a structured report.
    pub export_on_quit: crate::config::ExportOnQuit,
}

/// The server-listener handle threaded into the run loop, or `()` on builds
/// without `serve`. The unit type makes `None::<ServerArg>` a no-op pass-through
/// so the non-serve call sites (Diff/Show/Patch/Pager) compile unchanged.
#[cfg(all(feature = "serve", unix))]
pub type ServerArg = server::ServerListener;
#[cfg(not(all(feature = "serve", unix)))]
pub type ServerArg = ();

/// RAII guard that restores the terminal on drop. crossterm 0.28 ships no
/// built-in guard, so we define our own to guarantee cleanup even on panic.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

/// Leave the alternate screen and disable raw mode so a foreground child (the
/// editor) gets a clean, normal terminal. Paired with [`resume_tui`].
fn suspend_tui() -> Result<()> {
    disable_raw_mode().context("disable raw mode")?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)
        .context("leave alternate screen")?;
    Ok(())
}

/// Re-enter the alternate screen + raw mode and force a full redraw after the
/// editor returns. Paired with [`suspend_tui`].
fn resume_tui(terminal: &mut Tui) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)
        .context("enter alternate screen")?;
    // ratatui diffs against the last buffer, which is now stale; force a full
    // repaint by resizing the backend area to itself (flushes the diff cache).
    let area = terminal.get_frame().area();
    terminal.resize(area)?;
    Ok(())
}

/// Run the interactive review UI over an already-parsed [`Review`].
///
/// `reloader` enables `--watch`: when present (and a [`Watcher`] can be
/// started), the loop hot-reloads the review on filesystem changes, preserving
/// scroll / selection as described in [`App::reload_review`].
///
/// Returns a [`ReviewReport`] (decisions + comments + notes) on clean quit.
/// Outside `--select`, all hunks sit in `undecided` unless the human decided.
/// Errors only on fatal terminal I/O. If the process's stdout is not a tty,
/// crossterm will typically still enter raw mode and the caller may choose to
/// fall back to a non-interactive summary.
#[allow(clippy::too_many_arguments)]
pub fn run_review_tui(
    review: Review,
    reloader: Option<Reloader>,
    start_highlight: bool,
    start_line_numbers: bool,
    wrap_on: bool,
    theme: Option<String>,
    layout: crate::config::LayoutMode,
    workdir: Option<PathBuf>,
    options: ReviewOptions,
    server: Option<ServerArg>,
) -> Result<ReviewReport> {
    if review.is_empty() {
        anyhow::bail!("nothing to review (empty diff)");
    }

    // Surface focus miss on stderr *before* entering the alternate screen so
    // agent logs / non-TTY fallbacks still see a clear warning (TUI status bar
    // alone is easy to miss). Review still opens; the status bar also shows it.
    if let Some(ref focus) = options.focus {
        if app::resolve_focus_row(&review, focus).is_none() {
            eprintln!(
                "warning: focus not found: {}",
                app::focus_display(focus)
            );
        }
    }

    enable_raw_mode().context("enable raw mode")?;
    let _guard = RawModeGuard; // restore on drop / panic
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)
        .context("enter alternate screen")?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear()?;

    // Honor the config theme: parse "dark"/"light"/"auto" into a ThemeMode
    // (unknown/empty falls back to dark inside ThemeMode::parse).
    let theme_mode = theme
        .as_deref()
        .map(theme::ThemeMode::parse)
        .unwrap_or_default();
    let highlighter = std::sync::Arc::new(
        crate::highlight::Highlighter::load(theme_mode.syntect_theme_name())
            .unwrap_or_else(|_| crate::highlight::Highlighter::load_noop()),
    );
    let mut app = App::with_theme(review, highlighter, theme_mode);
    app.highlight_on = start_highlight;
    app.line_numbers_on = start_line_numbers;
    app.wrap_on = wrap_on;
    app.layout_mode = layout;
    // Inject agent-bridge options, then resolve the startup focus before the
    // first draw so the viewport opens at the agent's intended position.
    // Focus miss is already mirrored on stderr below (pre-TTY) so agents /
    // non-interactive logs see it even when the status bar is not visible.
    app.focus_target = options.focus;
    app.notes = options.notes;
    app.select_mode = options.select_mode;
    let _ = app.apply_focus();
    // Background highlight worker: viewport misses enqueue; main loop drains.
    let hl_worker = crate::highlight::HighlightWorker::spawn();
    app.hl_job_tx = Some(hl_worker.job_sender());
    run_loop(
        &mut terminal,
        &mut app,
        reloader,
        workdir,
        server.as_ref(),
        Some(hl_worker),
    )
}

fn run_loop(
    terminal: &mut Tui,
    app: &mut App,
    mut reloader: Option<Reloader>,
    workdir: Option<PathBuf>,
    #[allow(unused_variables)] server: Option<&ServerArg>,
    hl_worker: Option<crate::highlight::HighlightWorker>,
) -> Result<ReviewReport> {
    // If a reloader was provided, start a filesystem watcher for the current
    // directory. Watcher setup can fail (e.g. feature off, permissions); in
    // that case we keep running without live reload and surface a status note.
    let watcher: Option<Watcher> = if reloader.is_some() {
        match Watcher::spawn(&std::env::current_dir().unwrap_or_default()) {
            Ok(w) => {
                app.status = "watching for changes…".into();
                Some(w)
            }
            Err(e) => {
                app.status = format!("watch disabled: {e}");
                None
            }
        }
    } else {
        None
    };
    let mut last_event: Option<Instant> = None;

    loop {
        // Apply finished highlight jobs before draw so the next frame can
        // pick up styles. Stale gens are discarded inside apply_result.
        if let Some(w) = hl_worker.as_ref() {
            for result in w.drain() {
                let _ = app.cache.apply_result(result);
            }
        }

        // Draw, then sync the app's viewport height from the rendered area so
        // clamping on the next key uses the real visible height.
        terminal.draw(|f| {
            view::draw(app, f);
            // Capture the stream-pane height for clamping. We recompute it
            // from the frame area the same way view::draw does.
            let area = f.area();
            let main_height = area.height.saturating_sub(2);
            // main area splits horizontally; stream height == main_height
            app.viewport_height = main_height as usize;
        })?;

        // --- watch: drain events and apply debounce -----------------------
        if let (Some(w), Some(_)) = (watcher.as_ref(), reloader.as_ref()) {
            if w.drain() {
                last_event = Some(Instant::now());
            }
            if let Some(t) = last_event {
                if t.elapsed() >= DEBOUNCE {
                    // quiet period elapsed → reload once
                    last_event = None;
                    reload_once(app, reloader.as_mut().unwrap());
                }
            }
        }

        // --- serve: drain pending push/decision requests from the socket --
        // Mirrors the watch drain: non-blocking try_recv. Each request carries
        // its own reply channel, fulfilled here in the main thread (it owns
        // the App state the replies are derived from).
        #[cfg(all(feature = "serve", unix))]
        if let Some(srv) = server {
            for req in srv.drain() {
                let reply =
                    apply_server_command(app, req.command, reloader.as_mut(), workdir.as_deref());
                // Best-effort reply: a dropped sender means the CLI client
                // hung up (fine — the apply already took effect on the App).
                let _ = req.reply.send(reply);
            }
        }

        let event = input::read_event(250)?;
        let Some(event) = event else {
            continue;
        };

        if let Event::Key(key) = event {
            app.handle_key(key);
            if app.should_quit {
                // Full report for the caller to emit (decisions + comments + notes).
                return Ok(app.report());
            }
            // `o` requested opening a file in the editor. Suspend the TUI
            // (leave alt screen + raw mode so the editor gets a clean terminal),
            // run the editor as a foreground child, then resume the TUI.
            if let Some(target) = app.open_request.take() {
                suspend_tui()?;
                let result = launch_editor(&target, workdir.as_deref());
                resume_tui(terminal)?;
                match result {
                    Ok(msg) => app.status = msg,
                    Err(e) => app.status = format!("open failed: {e}"),
                }
                terminal.clear()?;
            }
        } else if let Event::Mouse(ev) = event {
            app.handle_mouse(ev);
        } else if let Event::Resize(_, _) = event {
            // next draw will pick up the new size
            continue;
        }
        // other events ignored
    }
}

/// Launch `$EDITOR` on `target.path` at `target.line`, resolving the path
/// against `workdir` (falling back to the process cwd). The caller has already
/// left the alternate screen and raw mode, so this runs as a normal foreground
/// child with a real terminal.
///
/// Editor resolution order: `$EDITOR` → `$VISUAL` → `vi`. The line argument
/// uses `+<n>`, the convention understood by vim/nvim/nano/emacs/jed.
fn launch_editor(target: &app::OpenTarget, workdir: Option<&std::path::Path>) -> Result<String> {
    let editor = std::env::var("EDITOR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("VISUAL")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string());

    // Resolve the file path against the repo workdir, else the cwd. This keeps
    // `o` working even when the review was launched from a subdirectory.
    let base = workdir.map(|w| w.to_path_buf()).unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    let file = base.join(&target.path);

    // Split the editor command on whitespace to support `code -w`, `vim -p`,
    // etc. (no shell, so quoting stays simple). Insert `+line` before the path.
    let line_arg = format!("+{}", target.line);
    let mut parts: Vec<String> = editor.split_whitespace().map(|s| s.to_string()).collect();
    let program = parts.remove(0);
    parts.push(line_arg);

    let mut cmd = std::process::Command::new(&program);
    for a in &parts {
        cmd.arg(a);
    }
    cmd.arg(&file);

    let status = cmd
        .status()
        .with_context(|| format!("spawn editor `{editor}`"))?;
    if status.success() {
        Ok(format!("opened {}:{}", target.path, target.line))
    } else {
        Ok(format!("editor exited {status}"))
    }
}

/// Fetch a fresh diff via the reloader and hot-swap it into `app`.
fn reload_once(app: &mut App, reloader: &mut Reloader) {
    match reloader() {
        Ok(produced) => app.reload_review_with_origins(&produced.text, &produced.origins),
        Err(e) => {
            app.status = format!("reload error: {e}");
        }
    }
}

/// Apply one server-mode command to the app and produce the reply to send back
/// to the CLI client. Lives here (not on `App`) because it bridges the
/// `server::ServerCommand` wire type with the App state — `App` stays free of
/// server-protocol knowledge. Pure w.r.t. I/O; safe to unit-test headlessly.
/// `reloader` is optional — when present, `Reload` commands re-fetch the diff.
/// `workdir` is the repo root known at `serve` startup (not a file path from
/// the review); used by `Info` so agents can disambiguate worktrees.
#[cfg(all(feature = "serve", unix))]
fn apply_server_command(
    app: &mut App,
    command: server::ServerCommand,
    reloader: Option<&mut Reloader>,
    workdir: Option<&std::path::Path>,
) -> server::ServerReply {
    use crate::tui::app::{Note, NoteTarget};
    use server::{ServerCommand, ServerReply};
    match command {
        ServerCommand::Push { focus, notes } => {
            // Replace focus (single target) and append notes (accumulating).
            if focus.is_some() {
                app.focus_target = focus;
                app.apply_focus();
            }
            app.notes.extend(notes);
            app.status = "pushed by agent".into();
            ServerReply::Ok
        }
        ServerCommand::Decision => {
            // Snapshot the human's current decisions (empty buckets outside
            // --select). Real-time: returns immediately, doesn't block on quit.
            ServerReply::Decisions(app.selections())
        }
        ServerCommand::Info => {
            // Repo root from serve startup — never the first review file path.
            // Prefer an absolute path so agents can tell worktrees apart.
            let repo_path = workdir
                .map(|p| match p.canonicalize() {
                    Ok(abs) => abs.display().to_string(),
                    Err(_) => p.display().to_string(),
                })
                .unwrap_or_default();
            ServerReply::Info {
                repo_path,
                file_count: app.review.file_count(),
            }
        }
        ServerCommand::Review => {
            // Return file/hunk structure (no full patch text).
            ServerReply::Review(server::ReviewSummary::from(&app.review))
        }
        ServerCommand::Navigate { target } => {
            // Set focus target and apply it (same path as --focus).
            app.focus_target = Some(target);
            app.apply_focus();
            ServerReply::Ok
        }
        ServerCommand::CommentAdd {
            file,
            text,
            line,
            hunk,
        } => {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = format!("c{}", COUNTER.fetch_add(1, Ordering::SeqCst));
            app.comments.push(crate::tui::app::CommentEntry {
                id: id.clone(),
                file,
                text,
                line,
                hunk,
            });
            ServerReply::CommentAdded { id }
        }
        ServerCommand::CommentList => ServerReply::CommentList {
            comments: app.comments.clone(),
        },
        ServerCommand::CommentRm { id } => {
            let before = app.comments.len();
            app.comments.retain(|c| c.id != id);
            if app.comments.len() < before {
                ServerReply::Ok
            } else {
                ServerReply::Error {
                    message: format!("comment {id} not found"),
                }
            }
        }
        ServerCommand::CommentApply => {
            // Merge comments into notes for TUI rendering. Each comment
            // becomes a Note attached to the appropriate target.
            let new_notes: Vec<Note> = app
                .comments
                .iter()
                .map(|c| {
                    let target = if let Some(hunk) = c.hunk {
                        NoteTarget::Hunk {
                            path: c.file.clone(),
                            hunk,
                        }
                    } else if let Some(line) = c.line {
                        NoteTarget::Line {
                            path: c.file.clone(),
                            line,
                        }
                    } else {
                        NoteTarget::Banner
                    };
                    Note {
                        target,
                        text: format!("💬 {}: {}", c.id, c.text),
                    }
                })
                .collect();
            app.notes.extend(new_notes);
            app.status = "comments applied".into();
            ServerReply::Ok
        }
        ServerCommand::Reload => match reloader {
            Some(r) => match (*r)() {
                Ok(produced) => {
                    app.reload_review_with_origins(&produced.text, &produced.origins);
                    app.status = "reloaded by agent".into();
                    ServerReply::Ok
                }
                Err(e) => ServerReply::Error {
                    message: format!("reload failed: {e}"),
                },
            },
            None => ServerReply::Error {
                message: "no reloader available (serve was started without --watch)".into(),
            },
        },
    }
}

// Re-export the most useful bits for tests / external use.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Build an app from a sample and drive it headlessly with TestBackend,
    /// asserting on the rendered buffer.
    fn sample_app() -> App {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-foo
+bar
",
        )
        .unwrap();
        let mut app = App::new(review);
        app.viewport_height = 20;
        app
    }

    #[test]
    fn run_review_tui_errors_on_empty() {
        let empty = Review::default();
        assert!(run_review_tui(
            empty,
            None,
            true,
            true,
            false,
            None,
            crate::config::LayoutMode::Unified,
            None,
            ReviewOptions::default(),
            None
        )
        .is_err());
    }

    #[test]
    fn draw_renders_file_paths_and_diff_lines() {
        let mut app = sample_app();
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // file rail should mention both files
        assert!(rendered.contains("a.rs"), "rail should list a.rs");
        assert!(rendered.contains("b.rs"), "rail should list b.rs");
    }

    #[test]
    fn draw_shows_added_and_deleted_lines() {
        let mut app = sample_app();
        // viewport_height large enough to show the whole stream
        app.viewport_height = 30;
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(rendered.contains("+new"), "should show added line +new");
        assert!(rendered.contains("-old"), "should show deleted line -old");
    }

    #[test]
    fn scripting_keys_navigates_then_quits() {
        let mut app = sample_app();
        // scroll a bit
        for _ in 0..3 {
            app.handle_key(key(KeyCode::Char('j')));
        }
        // move to next file
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.selected_file, 1);
        // go to bottom then top
        app.handle_key(key(KeyCode::Char('G')));
        app.handle_key(key(KeyCode::Char('g')));
        assert_eq!(app.scroll_y, 0);
        // quit
        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn tab_then_draw_shows_second_file_selected() {
        let mut app = sample_app();
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.selected_file, 1);

        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // status line should reference the second file path
        assert!(rendered.contains("b.rs"));
    }

    #[test]
    fn search_renders_prompt_then_status() {
        let mut app = sample_app();
        // enter search mode
        app.handle_key(key(KeyCode::Char('/')));
        assert_eq!(app.mode, crate::tui::app::InputMode::Search);

        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // prompt line should show the "/" search indicator
        assert!(
            rendered.contains('/'),
            "search prompt should render: {rendered}"
        );

        // type + finalize
        app.handle_key(key(KeyCode::Char('n')));
        app.handle_key(key(KeyCode::Char('e')));
        app.handle_key(key(KeyCode::Char('w')));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.search.active);

        terminal.draw(|f| view::draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // status line should show a match count
        assert!(
            rendered.contains("match") || rendered.contains("no matches"),
            "status should show match info: {rendered}"
        );
    }

    #[test]
    fn filter_prompt_renders() {
        let mut app = sample_app();
        app.handle_key(key(KeyCode::Char('f')));
        assert_eq!(app.mode, crate::tui::app::InputMode::Filter);

        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains("filter"),
            "filter prompt should render: {rendered}"
        );
    }

    /// A sample where old/new share a common token so word-diff has something
    /// to emphasize: `-old value` → `+new value`. The changed word ("old" /
    /// "new") should get bold+reversed style when word-diff is on.
    fn word_diff_app() -> App {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old value
+new value
",
        )
        .unwrap();
        let mut app = App::new(review);
        app.viewport_height = 30;
        app
    }

    /// Count cells on a given rendered line that carry a target modifier.
    fn count_modifier_on_line(
        terminal: &Terminal<TestBackend>,
        row: usize,
        modifier: ratatui::style::Modifier,
    ) -> usize {
        let buf = terminal.backend().buffer();
        let width = buf.area.width as usize;
        (0..width)
            .filter(|&x| {
                let cell = &buf[(x as u16, row as u16)];
                cell.style().add_modifier.contains(modifier)
            })
            .count()
    }

    #[test]
    fn word_diff_on_emphasizes_changed_word() {
        use ratatui::style::Modifier;
        let mut app = word_diff_app();
        assert!(app.word_diff_on, "word diff on by default");

        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();

        // Stream layout (scroll_y=0): row0=title, row1=file header, row2=hunk
        // header, row3=-old value, row4=+new value. Both +/- lines have their
        // changed token ("old" / "new") emphasized with BOLD.
        // Find the rows containing "-old" and "+new".
        let buf = terminal.backend().buffer();
        let width = buf.area.width;
        let mut del_row = None;
        let mut add_row = None;
        for y in 0..buf.area.height {
            let row_text: String = (0..width)
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect();
            if row_text.contains("-old") {
                del_row = Some(y as usize);
            }
            if row_text.contains("+new") {
                add_row = Some(y as usize);
            }
        }
        let del_row = del_row.expect("should find -old line");
        let add_row = add_row.expect("should find +new line");

        let del_bold = count_modifier_on_line(&terminal, del_row, Modifier::BOLD);
        let add_bold = count_modifier_on_line(&terminal, add_row, Modifier::BOLD);
        assert!(
            del_bold >= 3,
            "changed word 'old' should be bold (>=3 cells), got {del_bold}"
        );
        assert!(
            add_bold >= 3,
            "changed word 'new' should be bold (>=3 cells), got {add_bold}"
        );
    }

    #[test]
    fn word_diff_off_no_emphasis_on_changed_word() {
        use ratatui::style::Modifier;
        let mut app = word_diff_app();
        // Turn word-diff off.
        app.handle_key(key(KeyCode::Char('w')));
        assert!(!app.word_diff_on);

        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let width = buf.area.width;
        let mut del_row = None;
        let mut add_row = None;
        for y in 0..buf.area.height {
            let row_text: String = (0..width)
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect();
            if row_text.contains("-old") {
                del_row = Some(y as usize);
            }
            if row_text.contains("+new") {
                add_row = Some(y as usize);
            }
        }
        let del_row = del_row.expect("should find -old line");
        let add_row = add_row.expect("should find +new line");

        let del_bold = count_modifier_on_line(&terminal, del_row, Modifier::BOLD);
        let add_bold = count_modifier_on_line(&terminal, add_row, Modifier::BOLD);
        assert_eq!(
            del_bold, 0,
            "no bold emphasis when word-diff off, got {del_bold}"
        );
        assert_eq!(
            add_bold, 0,
            "no bold emphasis when word-diff off, got {add_bold}"
        );
    }

    #[test]
    fn mouse_scroll_moves_one_line_and_half_page() {
        use crossterm::event::{MouseEvent, MouseEventKind};
        // Big review so there's room to scroll down without clamping.
        let mut app = sample_app();
        app.viewport_height = 4;
        let start = app.scroll_y;

        // ScrollDown (no modifier) → advance by exactly 1.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_y, start + 1);

        // Shift+ScrollDown → advance by half viewport (4/2 = 2).
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::SHIFT,
        });
        assert_eq!(app.scroll_y, start + 3); // +1 then +2

        // ScrollUp (no modifier) → back up by 1.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_y, start + 2);

        // Shift+ScrollUp → back up by half viewport (2).
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::SHIFT,
        });
        assert_eq!(app.scroll_y, start);
    }

    #[test]
    fn mouse_scroll_clamps_at_bounds() {
        use crossterm::event::{MouseEvent, MouseEventKind};
        let mut app = sample_app();
        app.viewport_height = 4;
        // At top: ScrollUp must not underflow / move below 0.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_y, 0);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::SHIFT,
        });
        assert_eq!(app.scroll_y, 0);

        // Jump to bottom and confirm ScrollDown clamps there.
        app.scroll_y = app.max_scroll();
        let max = app.max_scroll();
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::SHIFT,
        });
        assert_eq!(app.scroll_y, max);
    }

    #[test]
    fn mouse_click_is_ignored() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut app = sample_app();
        let before = app.scroll_y;
        // A click must not move the scroll position.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_y, before);
    }

    #[test]
    fn t_key_cycles_theme_mode_and_status() {
        use crate::tui::theme::ThemeMode;
        let mut app = sample_app();
        // The default theme is now Light (Flexoki paper).
        assert_eq!(app.theme_mode, ThemeMode::Light);
        let light_add = app.theme.add;

        // Light → Auto
        app.handle_key(key(KeyCode::Char('t')));
        assert_eq!(app.theme_mode, ThemeMode::Auto);
        assert!(app.status.contains("auto"));

        // Auto → Dark
        app.handle_key(key(KeyCode::Char('t')));
        assert_eq!(app.theme_mode, ThemeMode::Dark);
        assert_ne!(app.theme.add, light_add); // palette changed
        assert!(app.status.contains("dark"));

        // Dark → Light
        app.handle_key(key(KeyCode::Char('t')));
        assert_eq!(app.theme_mode, ThemeMode::Light);
        assert!(app.status.contains("light"));
    }

    #[test]
    fn light_theme_paints_status_bar() {
        use crate::tui::theme::ThemeMode;
        let mut app = sample_app();
        // Switch to the light theme explicitly.
        app.theme_mode = ThemeMode::Light;
        app.theme = ThemeMode::Light.to_theme();

        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();

        // The light theme's status bar is painted with Flexoki base-100.
        let status_bg = ratatui::style::Color::Rgb(0xE6, 0xE4, 0xD9);
        let has_bg = buf
            .content()
            .iter()
            .any(|c| c.style().bg == Some(status_bg));
        assert!(
            has_bg,
            "light theme should paint the status bar with Flexoki base-100"
        );
    }

    #[test]
    fn help_overlay_renders_keybindings() {
        let mut app = sample_app();
        // Open the overlay with `?`.
        app.handle_key(key(KeyCode::Char('?')));
        assert!(app.show_help);

        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();
        // Flatten the rendered cells into a string and look for tell-tale text.
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("keybindings"), "overlay title missing");
        assert!(text.contains("Navigation"), "overlay section missing");
        assert!(
            text.contains("ignore-whitespace"),
            "overlay should list the W binding"
        );
    }

    /// Build an app with a known 2-hunk layout for select-mode rendering tests.
    /// Layout: a.rs with hunk0 @ row 2 (after file header @ row 1).
    fn select_sample_app() -> App {
        use crate::ir::parse_unified_diff;
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
",
        )
        .unwrap();
        let mut app = App::new(review);
        app.viewport_height = 20;
        app
    }

    fn rendered_buffer(app: &mut App, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(app, f)).unwrap();
        let buf = terminal.backend().buffer();
        buf.content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect()
    }

    #[test]
    fn select_mode_shows_undecided_marker_by_default() {
        let mut app = select_sample_app();
        app.select_mode = true;
        let rendered = rendered_buffer(&mut app, 60, 10);
        // Hunk header is around row 2; should carry the undecided "?" marker.
        assert!(
            rendered.contains("[?]"),
            "select mode should show [?] on undecided hunk header: {rendered}"
        );
    }

    #[test]
    fn select_mode_shows_accept_marker_after_decision() {
        use crate::tui::app::{Decision, HunkId};
        let mut app = select_sample_app();
        app.select_mode = true;
        app.decisions.insert(
            HunkId {
                file_idx: 0,
                hunk_idx: 0,
            },
            Decision::Accept,
        );
        let rendered = rendered_buffer(&mut app, 60, 10);
        assert!(
            rendered.contains("[✓]"),
            "accepted hunk should show [✓]: {rendered}"
        );
        assert!(
            !rendered.contains("[?]"),
            "decided hunk should not show [?]: {rendered}"
        );
    }

    #[test]
    fn select_mode_off_shows_no_markers() {
        let mut app = select_sample_app();
        // select_mode stays false (default).
        let rendered = rendered_buffer(&mut app, 60, 10);
        assert!(
            !rendered.contains("[?]") && !rendered.contains("[✓]"),
            "no markers outside select mode: {rendered}"
        );
    }

    #[test]
    fn line_note_renders_below_target() {
        use crate::tui::app::{Note, NoteTarget};
        let mut app = select_sample_app();
        // a.rs new line 1 is the +new line at row 4. Attach a note to it.
        app.notes.push(Note {
            target: NoteTarget::Line {
                path: "a.rs".into(),
                line: 1,
            },
            text: "agent says hi".into(),
        });
        let rendered = rendered_buffer(&mut app, 60, 10);
        assert!(
            rendered.contains("💬"),
            "line note should render a 💬 marker: {rendered}"
        );
        assert!(
            rendered.contains("agent says hi"),
            "note text should appear: {rendered}"
        );
    }

    #[test]
    fn hunk_note_renders_below_header() {
        use crate::tui::app::{Note, NoteTarget};
        let mut app = select_sample_app();
        // a.rs hunk 1 (1-based) → header row 2.
        app.notes.push(Note {
            target: NoteTarget::Hunk {
                path: "a.rs".into(),
                hunk: 1,
            },
            text: "review this hunk".into(),
        });
        let rendered = rendered_buffer(&mut app, 60, 10);
        assert!(
            rendered.contains("💬") && rendered.contains("review this hunk"),
            "hunk note should render: {rendered}"
        );
    }

    #[test]
    fn banner_note_renders_in_status_bar() {
        use crate::tui::app::{Note, NoteTarget};
        let mut app = select_sample_app();
        app.notes.push(Note {
            target: NoteTarget::Banner,
            text: "summary here".into(),
        });
        let rendered = rendered_buffer(&mut app, 60, 10);
        assert!(
            rendered.contains("summary here"),
            "banner note should appear in status bar: {rendered}"
        );
    }

    #[test]
    fn note_for_unknown_target_is_dropped() {
        use crate::tui::app::{Note, NoteTarget};
        let mut app = select_sample_app();
        app.notes.push(Note {
            target: NoteTarget::Line {
                path: "missing.rs".into(),
                line: 1,
            },
            text: "ghost".into(),
        });
        let rendered = rendered_buffer(&mut app, 60, 10);
        assert!(
            !rendered.contains("ghost"),
            "note targeting a missing file should not render: {rendered}"
        );
    }

    /// Reload without a reloader must return a clear error (not panic / drop).
    #[cfg(all(feature = "serve", unix))]
    #[test]
    fn reload_without_reloader_returns_clear_error() {
        use server::{ServerCommand, ServerReply};

        let mut app = sample_app();
        let reply = apply_server_command(&mut app, ServerCommand::Reload, None, None);
        match reply {
            ServerReply::Error { message } => {
                assert!(
                    message.contains("no reloader"),
                    "expected no-reloader message, got: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// `Info.repo_path` must be the serve workdir (repo root), never the first
    /// file path in the review — agents use it to pick among worktrees.
    #[cfg(all(feature = "serve", unix))]
    #[test]
    fn info_repo_path_is_workdir_not_first_file() {
        use server::{ServerCommand, ServerReply};
        use std::path::Path;

        let mut app = sample_app();
        // sample_app's first file is a.rs — the bug used that as "repo".
        assert_eq!(
            app.review.files.first().and_then(|f| f.new_path.as_deref()),
            Some("a.rs")
        );

        let workdir = Path::new("/tmp/fake-worktree-root");
        let reply = apply_server_command(&mut app, ServerCommand::Info, None, Some(workdir));
        match reply {
            ServerReply::Info {
                repo_path,
                file_count,
            } => {
                assert_eq!(file_count, 2);
                // Absolute preferred; if canonicalize fails (path missing), still
                // the workdir string — never "a.rs".
                assert!(
                    repo_path.ends_with("fake-worktree-root")
                        || repo_path == "/tmp/fake-worktree-root",
                    "repo_path should be workdir, got {repo_path:?}"
                );
                assert_ne!(repo_path, "a.rs");
                assert!(!repo_path.ends_with("a.rs"));
            }
            other => panic!("expected Info, got {other:?}"),
        }

        // No workdir → empty repo_path (still not a file path).
        let reply = apply_server_command(&mut app, ServerCommand::Info, None, None);
        match reply {
            ServerReply::Info { repo_path, .. } => {
                assert_eq!(repo_path, "");
            }
            other => panic!("expected Info, got {other:?}"),
        }
    }
}
