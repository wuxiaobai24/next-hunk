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
use crate::tui::app::{App, Selections};
use crate::tui::watch::{Watcher, DEBOUNCE};

pub mod app;
pub mod input;
pub mod keymap;
pub mod server;
pub mod theme;
pub mod view;
pub mod watch;

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// A live-reload source: produces a fresh unified-diff string on demand.
/// Carried into the run loop so `--watch` can re-fetch the diff without
/// touching the terminal from a background thread.
pub type Reloader = Box<dyn FnMut() -> Result<String>>;

/// Agent-bridge + run options threaded from the CLI into the run loop. All
/// fields are optional/empty by default, so callers that don't care about the
/// agent features (`--focus` / `--note` / `--select`) or sessions construct
/// `ReviewOptions::default()`.
#[derive(Debug, Default, Clone)]
pub struct ReviewOptions {
    /// `--focus`: scroll here on startup.
    pub focus: Option<app::FocusTarget>,
    /// `--note`: agent annotations to render.
    pub notes: Vec<app::Note>,
    /// `--select`: enable the per-hunk accept/reject gate; emit JSON on quit.
    pub select_mode: bool,
    /// Session metadata for the agent control plane: how this review was
    /// launched (`"diff"` / `"show"` / `"serve"`), reported by `Info`.
    /// Empty means no session was attached (patch/pager/filediff).
    pub session_mode: String,
    /// Short human-readable session title (e.g. `demo working tree`).
    pub session_title: String,
    /// `--watch`: start the filesystem watcher (live reload). Distinct from
    /// the presence of a reloader — every session keeps a reloader so
    /// `next-hunk reload` works, but only `--watch` polls the filesystem.
    pub watch: bool,
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
/// Returns the [`Selections`] (always present; empty buckets when not in
/// `--select` mode) on clean quit. Errors only on fatal terminal I/O. If the
/// process's stdout is not a tty, crossterm will typically still enter raw
/// mode and the caller may choose to fall back to a non-interactive summary.
#[allow(clippy::too_many_arguments)]
pub fn run_review_tui(
    review: Review,
    reloader: Option<Reloader>,
    settings: crate::config::ViewSettings,
    workdir: Option<PathBuf>,
    options: ReviewOptions,
    server: Option<ServerArg>,
) -> Result<Selections> {
    if review.is_empty() {
        anyhow::bail!("nothing to review (empty diff)");
    }

    enable_raw_mode().context("enable raw mode")?;
    let _guard = RawModeGuard; // restore on drop / panic
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)
        .context("enter alternate screen")?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear()?;

    // Honor the config theme: parse preset names ("catppuccin-mocha",
    // "gruvbox-dark", …) or legacy modes ("dark"/"light"/"auto") into a
    // (palette, mode) pair; unknown values fall back to the default inside
    // `parse_theme`.
    let (palette, theme_mode) = settings
        .theme
        .as_deref()
        .map(theme::parse_theme)
        .unwrap_or_default();
    let highlighter = std::sync::Arc::new(
        crate::highlight::Highlighter::load(palette.syntect_theme_name(theme_mode))
            .unwrap_or_else(|_| crate::highlight::Highlighter::load_noop()),
    );
    let mut app = App::with_theme(review, highlighter, theme_mode);
    app.palette = palette;
    app.theme = palette.theme(theme_mode);
    app.highlight_on = settings.highlight;
    app.line_numbers_on = settings.line_numbers;
    app.wrap_on = settings.wrap;
    app.layout_mode = settings.layout;
    app.cursor_on = settings.cursor_line;
    app.tab_width = settings.tab_width as usize;
    app.show_rail = settings.sidebar;
    app.show_notes = settings.agent_notes;
    app.keymap = settings.keymap;
    app.refresh_startup_status();
    app.set_context_collapse(settings.context_collapse);
    // Inject agent-bridge options, then resolve the startup focus before the
    // first draw so the viewport opens at the agent's intended position.
    app.focus_target = options.focus;
    app.notes = options.notes;
    app.select_mode = options.select_mode;
    app.repo_root = workdir.as_ref().map(|p| p.display().to_string());
    app.session_mode = options.session_mode.clone();
    app.session_title = options.session_title.clone();
    app.apply_focus();
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
        options.watch,
    )
}

fn run_loop(
    terminal: &mut Tui,
    app: &mut App,
    mut reloader: Option<Reloader>,
    workdir: Option<PathBuf>,
    #[allow(unused_variables)] server: Option<&ServerArg>,
    hl_worker: Option<crate::highlight::HighlightWorker>,
    watch_enabled: bool,
) -> Result<Selections> {
    // If a reloader was provided, start a filesystem watcher for the current
    // directory. Watcher setup can fail (e.g. feature off, permissions); in
    // that case we keep running without live reload and surface a status note.
    // Every attached session keeps a reloader (so `next-hunk reload` works),
    // but only `--watch` actually polls the filesystem.
    let watcher: Option<Watcher> = if reloader.is_some() && watch_enabled {
        match Watcher::spawn(&std::env::current_dir().unwrap_or_default()) {
            Ok(w) => {
                app.watch_mode = true;
                app.set_info("watching for changes…");
                Some(w)
            }
            Err(e) => {
                app.set_error(format!("watch disabled: {e}"));
                None
            }
        }
    } else {
        None
    };
    // Surface the run mode as a status-bar badge. `serve` keeps its dedicated
    // badge; plain `diff`/`show` sessions listen too but stay visually quiet.
    app.serve_mode = server.is_some() && app.session_mode == "serve";
    let mut last_event: Option<Instant> = None;

    loop {
        // Drop a status toast that has outlived its TTL (idle auto-clear) so a
        // transient message or red error doesn't linger on screen. Sticky
        // toasts (the initial hint) are never cleared here.
        app.expire_status();

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
                let reply = apply_server_command(app, req.command, reloader.as_mut());
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
                // Emit the per-hunk decisions (empty buckets outside --select).
                return Ok(app.selections());
            }
            // `o` requested opening a file in the editor. Suspend the TUI
            // (leave alt screen + raw mode so the editor gets a clean terminal),
            // run the editor as a foreground child, then resume the TUI.
            if let Some(target) = app.open_request.take() {
                suspend_tui()?;
                let result = launch_editor(&target, workdir.as_deref());
                resume_tui(terminal)?;
                match result {
                    Ok(msg) => app.set_success(msg),
                    Err(e) => app.set_error(format!("open failed: {e}")),
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
        Ok(text) => app.reload_review(&text),
        Err(e) => {
            app.set_error(format!("reload error: {e}"));
        }
    }
}

/// Convert a comment entry into the `Note` that renders it in the stream.
/// The renderer adds the 💬 glyph, so the text carries only the id (for
/// `comment rm` correlation) and the body.
#[cfg(all(feature = "serve", unix))]
fn comment_note(entry: &crate::tui::app::CommentEntry) -> crate::tui::app::Note {
    use crate::tui::app::{Note, NoteTarget};
    let target = if let Some(hunk) = entry.hunk {
        NoteTarget::Hunk {
            path: entry.file.clone(),
            hunk,
        }
    } else if let Some(line) = entry.line {
        NoteTarget::Line {
            path: entry.file.clone(),
            line,
        }
    } else {
        NoteTarget::Banner
    };
    Note {
        target,
        text: format!("{}: {}", entry.id, entry.text),
    }
}

/// The `--focus` target for a comment: its file, refined by hunk/line when
/// the entry carries one.
#[cfg(all(feature = "serve", unix))]
fn comment_focus_target(entry: &crate::tui::app::CommentEntry) -> app::FocusTarget {
    if let Some(hunk) = entry.hunk {
        app::FocusTarget::FileHunk(entry.file.clone(), hunk)
    } else if let Some(line) = entry.line {
        app::FocusTarget::FileLine(entry.file.clone(), line)
    } else {
        app::FocusTarget::File(entry.file.clone())
    }
}

/// Sequence for agent comment ids (`c0`, `c1`, …) across every command that
/// mints them (single add and batch apply share it so ids stay unique).
#[cfg(all(feature = "serve", unix))]
static COMMENT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Apply one server-mode command to the app and produce the reply to send back
/// to the CLI client. Lives here (not on `App`) because it bridges the
/// `server::ServerCommand` wire type with the App state — `App` stays free of
/// server-protocol knowledge. Pure w.r.t. I/O; safe to unit-test headlessly.
/// `reloader` is optional — when present, `Reload` commands re-fetch the diff.
#[cfg(all(feature = "serve", unix))]
fn apply_server_command(
    app: &mut App,
    command: server::ServerCommand,
    reloader: Option<&mut Reloader>,
) -> server::ServerReply {
    use crate::tui::app::Note;
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
            // Return session metadata (real repo root, launch mode, focus).
            let (focus_file, focus_hunk, focus_line) = app.current_focus();
            ServerReply::Info {
                repo_root: app.repo_root.clone().unwrap_or_default(),
                file_count: app.review.file_count(),
                pid: std::process::id(),
                mode: if app.session_mode.is_empty() {
                    "review".to_string()
                } else {
                    app.session_mode.clone()
                },
                title: if app.session_title.is_empty() {
                    app.repo_root.clone().unwrap_or_default()
                } else {
                    app.session_title.clone()
                },
                focus_file,
                focus_hunk,
                focus_line,
            }
        }
        ServerCommand::Context => {
            let (focus_file, focus_hunk, focus_line) = app.current_focus();
            ServerReply::Context {
                focus_file,
                focus_hunk,
                focus_line,
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
        ServerCommand::NoteJump { next } => {
            if app.annotated_rows().is_empty() {
                ServerReply::Error {
                    message: "no notes in this diff".into(),
                }
            } else {
                app.jump_note(next);
                ServerReply::Ok
            }
        }
        ServerCommand::CommentAdd {
            file,
            text,
            line,
            hunk,
            focus,
        } => {
            use std::sync::atomic::Ordering;
            let id = format!("c{}", COMMENT_SEQ.fetch_add(1, Ordering::SeqCst));
            let entry = crate::tui::app::CommentEntry {
                id: id.clone(),
                file,
                text,
                line,
                hunk,
            };
            // Render immediately (mirrors hunk's live comment cards): the
            // note lands in the stream the moment the comment is added.
            app.applied_comments.insert(id.clone());
            app.notes.push(comment_note(&entry));
            if focus {
                app.focus_target = Some(comment_focus_target(&entry));
                app.apply_focus();
            }
            app.comments.push(entry);
            app.set_success(format!("comment {id} added by agent"));
            ServerReply::CommentAdded { id }
        }
        ServerCommand::CommentList => ServerReply::CommentList {
            comments: app.comments.clone(),
        },
        ServerCommand::CommentRm { id } => {
            let before = app.comments.len();
            app.comments.retain(|c| c.id != id);
            if app.comments.len() < before {
                // A human note (`user:N`) also renders as a Note — remove the
                // paired note so the row disappears from the stream too.
                if let Some(note) = app.user_notes.remove(&id) {
                    if let Some(pos) = app.notes.iter().position(|n| *n == note) {
                        app.notes.remove(pos);
                    }
                }
                ServerReply::Ok
            } else {
                ServerReply::Error {
                    message: format!("comment {id} not found"),
                }
            }
        }
        ServerCommand::CommentApply => {
            // Merge *not-yet-applied* comments into notes for TUI rendering.
            // `comment add` already applies its comment, so this mostly
            // no-ops today; it stays for re-surfacing after a review swap
            // and for callers that pre-seeded comments another way.
            let mut new_notes: Vec<Note> = Vec::new();
            for c in app.comments.iter() {
                if !app.applied_comments.insert(c.id.clone()) {
                    continue;
                }
                new_notes.push(comment_note(c));
            }
            let n = new_notes.len();
            app.notes.extend(new_notes);
            app.set_success(format!("comments applied ({n} new)"));
            ServerReply::Ok
        }
        ServerCommand::CommentApplyBatch { comments, focus } => {
            // Validate the whole batch before mutating anything: every item
            // must name a known file and, when given, an in-range hunk.
            for (i, c) in comments.iter().enumerate() {
                match crate::ir::ViewportQuery::file_index_for_path(&app.review, &c.file) {
                    None => {
                        return ServerReply::Error {
                            message: format!("comment {} targets unknown file `{}`", i + 1, c.file),
                        }
                    }
                    Some(fi) => {
                        let hunk_count = app.review.files[fi].hunks.len();
                        if let Some(h) = c.hunk {
                            if h == 0 || h > hunk_count {
                                return ServerReply::Error {
                                    message: format!(
                                        "comment {}: `{}` has {} hunk(s); hunk {h} is out of range",
                                        i + 1,
                                        c.file,
                                        hunk_count
                                    ),
                                };
                            }
                        }
                    }
                }
            }
            use std::sync::atomic::Ordering;
            let mut entries = Vec::new();
            for c in comments {
                let id = format!("c{}", COMMENT_SEQ.fetch_add(1, Ordering::SeqCst));
                entries.push(crate::tui::app::CommentEntry {
                    id,
                    file: c.file,
                    text: c.text,
                    line: c.line,
                    hunk: c.hunk,
                });
            }
            let n = entries.len();
            let first_focus = focus
                .then(|| entries.first().map(comment_focus_target))
                .flatten();
            for entry in &entries {
                app.applied_comments.insert(entry.id.clone());
                app.notes.push(comment_note(entry));
                app.comments.push(entry.clone());
            }
            if let Some(t) = first_focus {
                app.focus_target = Some(t);
                app.apply_focus();
            }
            app.set_success(format!("{n} comments applied by agent"));
            ServerReply::Ok
        }
        ServerCommand::CommentClear { file, all } => {
            // Scope: `--file` limits by path; agent comments (c*) always go,
            // human notes (user:*) only with `all`.
            let in_scope = |c: &crate::tui::app::CommentEntry| {
                file.as_deref().is_none_or(|f| c.file == f) && (all || !c.id.starts_with("user:"))
            };
            let removed: Vec<String> = app
                .comments
                .iter()
                .filter(|c| in_scope(c))
                .map(|c| c.id.clone())
                .collect();
            if removed.is_empty() {
                return ServerReply::Error {
                    message: "no comments matched".into(),
                };
            }
            app.comments.retain(|c| !in_scope(c));
            for id in &removed {
                // Drop the paired rendered note: human notes pair through
                // `user_notes` (their text carries no id), agent notes
                // through the "{id}: " text prefix.
                if let Some(note) = app.user_notes.remove(id) {
                    app.notes.retain(|n| *n != note);
                } else {
                    let prefix = format!("{id}: ");
                    app.notes.retain(|n| !n.text.starts_with(&prefix));
                }
            }
            let n = removed.len();
            app.set_success(format!("cleared {n} comment(s)"));
            ServerReply::CommentCleared { removed: n }
        }
        ServerCommand::HighlightAdd {
            file,
            line,
            start,
            end,
            tone,
            focus,
        } => {
            // Validate: known file, sensible range. The line itself is
            // best-effort (a mark on a line not in the diff just never
            // renders), but a garbage range or unknown file is an error.
            if crate::ir::ViewportQuery::file_index_for_path(&app.review, &file).is_none() {
                return ServerReply::Error {
                    message: format!("highlight targets unknown file `{file}`"),
                };
            }
            if start == 0 || end <= start {
                return ServerReply::Error {
                    message: format!(
                        "highlight range must satisfy 1 <= start < end (got {start}..{end})"
                    ),
                };
            }
            let tone = if tone.is_empty() {
                "warning".to_string()
            } else {
                tone
            };
            let mark = crate::tui::app::HighlightMark {
                id: format!("hl{}", app.highlights.len()),
                file,
                line,
                start,
                end,
                tone,
            };
            if focus {
                app.focus_target = Some(app::FocusTarget::FileLine(mark.file.clone(), mark.line));
                app.apply_focus();
            }
            app.set_success("attention mark painted by agent");
            app.highlights.push(mark);
            ServerReply::Ok
        }
        ServerCommand::HighlightList => ServerReply::HighlightList {
            marks: app.highlights.clone(),
        },
        ServerCommand::HighlightClear { file } => {
            let before = app.highlights.len();
            // Retain only marks outside the scope: with no `--file` every
            // mark goes; with one, that file's marks go.
            app.highlights.retain(|m| match file.as_deref() {
                Some(f) => m.file != f,
                None => false,
            });
            let removed = before - app.highlights.len();
            if removed == 0 {
                ServerReply::Error {
                    message: "no marks matched".into(),
                }
            } else {
                app.set_success(format!("cleared {removed} mark(s)"));
                ServerReply::HighlightCleared { removed }
            }
        }
        ServerCommand::Reload => match reloader {
            Some(r) => match (*r)() {
                Ok(text) => {
                    app.reload_review(&text);
                    app.set_success("reloaded by agent");
                    ServerReply::Ok
                }
                Err(e) => ServerReply::Error {
                    message: format!("reload failed: {e}"),
                },
            },
            None => ServerReply::Error {
                message: "no reloader available (this review was opened without one)".into(),
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
            crate::config::ViewSettings::default(),
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
    fn split_layout_renders_aligned_columns() {
        let mut app = sample_app();
        app.layout_mode = crate::config::LayoutMode::Split;
        app.viewport_height = 20;

        let backend = TestBackend::new(120, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // The old and new sides of the first file's change must appear on
        // the same screen, in order, separated by the divider column.
        let old_pos = rendered.find("old").expect("old side visible");
        let new_pos = rendered.find("new").expect("new side visible");
        assert!(old_pos < new_pos, "old column precedes new column");
        assert!(rendered.contains('│'), "divider drawn");
    }

    #[test]
    fn auto_layout_resolves_by_width() {
        let mut app = sample_app();
        app.layout_mode = crate::config::LayoutMode::Auto;
        // Wide stream pane → split.
        app.stream_rect = Some(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 40,
        });
        assert_eq!(app.effective_layout(), crate::config::LayoutMode::Split);
        // Medium → stack.
        app.stream_rect = Some(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 40,
        });
        assert_eq!(app.effective_layout(), crate::config::LayoutMode::Stack);
        // Narrow → unified.
        app.stream_rect = Some(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 40,
        });
        assert_eq!(app.effective_layout(), crate::config::LayoutMode::Unified);
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

    /// Rendered terminal rows, one `String` per row (trailing blanks
    /// trimmed). Used by tests that assert which row a piece of text lands
    /// on (e.g. inline note placement).
    fn rendered_rows(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(app, f)).unwrap();
        let buf = terminal.backend().buffer();
        buf.content()
            .chunks(w as usize)
            .map(|row| {
                row.iter()
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
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
    fn line_note_renders_inline_when_room() {
        use crate::tui::app::{Note, NoteTarget};
        let mut app = select_sample_app();
        // a.rs new line 1 is the +new line. Attach a note to it.
        app.notes.push(Note {
            target: NoteTarget::Line {
                path: "a.rs".into(),
                line: 1,
            },
            text: "agent says hi".into(),
        });
        let rows = rendered_rows(&mut app, 100, 10);
        let code_row = rows
            .iter()
            .find(|r| r.contains("+new"))
            .expect("+new row rendered");
        assert!(
            code_row.contains("💬") && code_row.contains("agent says hi"),
            "note should render inline on the code row when there is room: {code_row}"
        );
    }

    #[test]
    fn line_note_falls_back_to_own_row_when_narrow() {
        use crate::tui::app::{Note, NoteTarget};
        let mut app = select_sample_app();
        app.notes.push(Note {
            target: NoteTarget::Line {
                path: "a.rs".into(),
                line: 1,
            },
            text: "agent says hi".into(),
        });
        // 40 cols: rail takes 12, stream gets 28 — no room next to a gutter
        // line, so the note takes its own row below the code line.
        let rows = rendered_rows(&mut app, 40, 12);
        let code_idx = rows
            .iter()
            .position(|r| r.contains("+new"))
            .expect("+new row rendered");
        let note_idx = rows
            .iter()
            .position(|r| r.contains("agent says hi"))
            .expect("note row rendered");
        assert!(
            note_idx > code_idx,
            "fallback note row should sit below the code row: {rows:?}"
        );
        assert!(
            rows[note_idx].contains("💬"),
            "fallback note row keeps the 💬 glyph: {rows:?}"
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
    fn rail_shows_note_count_badge() {
        use crate::tui::app::{Note, NoteTarget};
        let mut app = select_sample_app();
        app.notes.push(Note {
            target: NoteTarget::Line {
                path: "a.rs".into(),
                line: 1,
            },
            text: "one".into(),
        });
        app.notes.push(Note {
            target: NoteTarget::Hunk {
                path: "a.rs".into(),
                hunk: 1,
            },
            text: "two".into(),
        });
        let rendered = rendered_buffer(&mut app, 80, 10);
        // 💬 is double-width: the buffer flattens its second cell to a
        // space, so the badge reads "💬 2" in the flattened string.
        assert!(
            rendered.contains("💬 2"),
            "rail should show a per-file note count badge: {rendered}"
        );
    }

    #[test]
    fn note_jump_keys_move_between_annotated_rows() {
        use crate::tui::app::{InputMode, Note, NoteTarget};

        // A stream tall enough that jumps actually move the viewport:
        // 24 context rows, then the changed line at new-side line 25.
        let body: String = "ctx\n".repeat(24);
        let patch = format!(
            "diff --git a/big.rs b/big.rs\n--- a/big.rs\n+++ b/big.rs\n@@ -1,25 +1,25 @@\n{body}-old\n+new\n"
        );
        let review = parse_unified_diff(&patch).unwrap();
        let mut app = App::new(review);
        app.set_context_collapse(0); // keep rows 1:1 so the math is direct
        app.viewport_height = 5;
        app.notes.push(Note {
            target: NoteTarget::Line {
                path: "big.rs".into(),
                line: 25,
            },
            text: "look here".into(),
        });
        assert_eq!(app.annotated_rows().len(), 1);
        let note_row = app.annotated_rows()[0];
        let visible = |app: &App| {
            let v = app.collapse.virtual_of_stream(note_row);
            v >= app.scroll_y && v < app.scroll_y + app.viewport_height
        };

        app.handle_key(key(KeyCode::Char('}')));
        assert!(
            visible(&app),
            "`}}` should scroll the annotated row into view (scroll_y={})",
            app.scroll_y
        );
        assert!(
            app.status.message.contains("note 1/1"),
            "jump should report the note ordinal: {}",
            app.status.message
        );

        // Repeated `}` wraps around the (single) note set instead of
        // getting stuck when the viewport clamps at the stream end.
        app.handle_key(key(KeyCode::Char('}')));
        assert!(
            app.status.message.contains("note 1/1"),
            "wrap-around jump should re-report the note: {}",
            app.status.message
        );
        // And `{` walks back.
        app.handle_key(key(KeyCode::Char('{')));
        assert!(
            visible(&app),
            "`{{` should return to the annotated row (scroll_y={})",
            app.scroll_y
        );
        // Mode stays normal (the `}`/`{` keys never open an input mode).
        assert_eq!(app.mode, InputMode::Normal);
    }

    #[test]
    fn note_jump_without_notes_is_a_noop() {
        let mut app = select_sample_app();
        app.handle_key(key(KeyCode::Char('}')));
        assert_eq!(app.scroll_y, 0);
        assert!(
            app.status.message.contains("no notes"),
            "jump without notes should say so: {}",
            app.status.message
        );
    }

    #[test]
    fn catppuccin_palette_paints_the_status_bar_mocha() {
        let mut app = select_sample_app();
        app.palette = theme::Palette::Catppuccin;
        app.theme_mode = theme::ThemeMode::Dark;
        app.apply_theme();
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        // Catppuccin Mocha's status band is the mantle color #181825.
        let mocha_mantle = ratatui::style::Color::Rgb(0x18, 0x18, 0x25);
        assert!(
            buf.content()
                .iter()
                .any(|c| c.style().bg == Some(mocha_mantle)),
            "status bar should be painted with the Mocha mantle color"
        );
    }

    #[test]
    fn cursor_row_gets_highlight_background() {
        let mut app = select_sample_app();
        // +new is stream row 3 (0 file header, 1 hunk header, 2 -old, 3 +new).
        app.set_cursor(3);
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        // rail_w = min(32, 60/4).max(12) = 15, so the stream starts at x=15.
        // The pane title occupies the first row, so stream row 3 (+new)
        // draws at y=4; x=16 is past its gutter columns.
        let cell = &buf[(16u16, 4u16)];
        assert_eq!(
            cell.style().bg,
            Some(app.theme.cursor_bg),
            "cursor row should carry the cursor background"
        );
        // And a non-cursor code row does not.
        let other = &buf[(16u16, 3u16)];
        assert_ne!(other.style().bg, Some(app.theme.cursor_bg));
    }

    #[test]
    fn cursor_line_off_renders_no_highlight() {
        let mut app = select_sample_app();
        app.set_cursor(3);
        app.cursor_on = false;
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        let cell = &buf[(16u16, 4u16)];
        assert_ne!(cell.style().bg, Some(app.theme.cursor_bg));
    }

    #[test]
    fn note_prompt_renders_in_help_line() {
        let mut app = select_sample_app();
        app.set_cursor(3);
        app.handle_key(key(KeyCode::Char('c')));
        for ch in "hi".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        let rendered = rendered_buffer(&mut app, 60, 10);
        assert!(
            rendered.contains("note: hi"),
            "note composer should echo the draft in the prompt line: {rendered}"
        );
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn comment_rm_user_note_also_removes_the_note_row() {
        use crate::tui::server::ServerCommand;
        let mut app = select_sample_app();
        app.set_cursor(3); // +new row
        app.handle_key(key(KeyCode::Char('c')));
        for ch in "stale".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.notes.len(), 1);
        assert_eq!(app.comments[0].id, "user:1");
        apply_server_command(
            &mut app,
            ServerCommand::CommentRm {
                id: "user:1".into(),
            },
            None,
        );
        assert!(
            app.comments.iter().all(|c| c.id != "user:1"),
            "comment removed"
        );
        assert!(app.notes.is_empty(), "paired note row removed too");
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn info_reports_session_meta_and_focus() {
        use crate::tui::server::ServerCommand;
        let mut app = select_sample_app();
        app.repo_root = Some("/tmp/demo".into());
        app.session_mode = "diff".into();
        app.session_title = "demo working tree".into();
        // cursor on the +new row (stream row 3, new-side line 1)
        app.set_cursor(3);
        match apply_server_command(&mut app, ServerCommand::Info, None) {
            crate::tui::server::ServerReply::Info {
                repo_root,
                file_count,
                mode,
                title,
                focus_file,
                focus_hunk,
                focus_line,
                ..
            } => {
                assert_eq!(repo_root, "/tmp/demo");
                assert_eq!(file_count, 1);
                assert_eq!(mode, "diff");
                assert_eq!(title, "demo working tree");
                assert_eq!(focus_file.as_deref(), Some("a.rs"));
                assert_eq!(focus_hunk, Some(1));
                assert_eq!(focus_line, Some(1));
            }
            other => panic!("expected Info, got {other:?}"),
        }
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn context_reports_cursor_focus() {
        use crate::tui::server::ServerCommand;
        let mut app = select_sample_app();
        // hunk header row (stream row 1): file known, hunk known, no line
        app.set_cursor(1);
        match apply_server_command(&mut app, ServerCommand::Context, None) {
            crate::tui::server::ServerReply::Context {
                focus_file,
                focus_hunk,
                focus_line,
            } => {
                assert_eq!(focus_file.as_deref(), Some("a.rs"));
                assert_eq!(focus_hunk, Some(1));
                assert_eq!(focus_line, None, "hunk header carries no line number");
            }
            other => panic!("expected Context, got {other:?}"),
        }
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn comment_add_renders_note_immediately_and_focuses() {
        use crate::tui::server::ServerCommand;
        let mut app = select_sample_app();
        app.cursor_v = 0;
        app.scroll_y = 0;
        match apply_server_command(
            &mut app,
            ServerCommand::CommentAdd {
                file: "a.rs".into(),
                text: "live note".into(),
                line: Some(1),
                hunk: None,
                focus: true,
            },
            None,
        ) {
            crate::tui::server::ServerReply::CommentAdded { id } => {
                assert!(!id.is_empty());
            }
            other => panic!("expected CommentAdded, got {other:?}"),
        }
        // The note is in the stream right away…
        assert_eq!(app.notes.len(), 1, "comment add renders its note live");
        assert!(app.notes[0].text.contains("live note"));
        // …listed by comment list…
        assert_eq!(app.comments.len(), 1);
        // …and `--focus` moved the cursor onto the target row (+new = row 3).
        assert_eq!(app.cursor_v, 3);
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn comment_batch_validates_atomically() {
        use crate::tui::server::{BatchComment, ServerCommand, ServerReply};
        let mut app = select_sample_app();
        // Unknown file → the whole batch is rejected, nothing applied.
        let reply = apply_server_command(
            &mut app,
            ServerCommand::CommentApplyBatch {
                comments: vec![
                    BatchComment {
                        file: "a.rs".into(),
                        text: "ok".into(),
                        line: Some(1),
                        hunk: None,
                    },
                    BatchComment {
                        file: "nope.rs".into(),
                        text: "bad".into(),
                        line: None,
                        hunk: None,
                    },
                ],
                focus: false,
            },
            None,
        );
        match reply {
            ServerReply::Error { message: msg } => {
                assert!(msg.contains("nope.rs"), "error names the file: {msg}")
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(app.comments.is_empty(), "rejected batch must not mutate");
        assert!(app.notes.is_empty(), "rejected batch must not mutate");
        // Out-of-range hunk ordinal is rejected too.
        let reply = apply_server_command(
            &mut app,
            ServerCommand::CommentApplyBatch {
                comments: vec![BatchComment {
                    file: "a.rs".into(),
                    text: "bad hunk".into(),
                    line: None,
                    hunk: Some(7),
                }],
                focus: false,
            },
            None,
        );
        assert!(matches!(reply, ServerReply::Error { .. }));
        // A valid batch applies every comment as a note.
        let reply = apply_server_command(
            &mut app,
            ServerCommand::CommentApplyBatch {
                comments: vec![
                    BatchComment {
                        file: "a.rs".into(),
                        text: "one".into(),
                        line: Some(1),
                        hunk: None,
                    },
                    BatchComment {
                        file: "a.rs".into(),
                        text: "two".into(),
                        line: None,
                        hunk: Some(1),
                    },
                ],
                focus: false,
            },
            None,
        );
        assert!(matches!(reply, ServerReply::Ok));
        assert_eq!(app.comments.len(), 2);
        assert_eq!(app.notes.len(), 2);
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn comment_clear_scopes_and_removes_paired_notes() {
        use crate::tui::server::ServerCommand;
        let mut app = select_sample_app();
        // Seed one agent comment (live-rendered) and one human note.
        apply_server_command(
            &mut app,
            ServerCommand::CommentAdd {
                file: "a.rs".into(),
                text: "agent says".into(),
                line: Some(1),
                hunk: None,
                focus: false,
            },
            None,
        );
        app.set_cursor(3); // +new row
        app.handle_key(key(KeyCode::Char('c')));
        for ch in "human says".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.comments.len(), 2);
        assert_eq!(app.notes.len(), 2);
        // Default clear removes only agent comments (and their notes).
        match apply_server_command(
            &mut app,
            ServerCommand::CommentClear {
                file: None,
                all: false,
            },
            None,
        ) {
            crate::tui::server::ServerReply::CommentCleared { removed } => {
                assert_eq!(removed, 1)
            }
            other => panic!("expected CommentCleared, got {other:?}"),
        }
        assert_eq!(app.comments.len(), 1, "human note survives default clear");
        assert_eq!(app.notes.len(), 1, "its rendered note survives too");
        assert_eq!(app.comments[0].id, "user:1");
        // `--all` takes the human note as well.
        match apply_server_command(
            &mut app,
            ServerCommand::CommentClear {
                file: None,
                all: true,
            },
            None,
        ) {
            crate::tui::server::ServerReply::CommentCleared { removed } => {
                assert_eq!(removed, 1)
            }
            other => panic!("expected CommentCleared, got {other:?}"),
        }
        assert!(app.comments.is_empty());
        assert!(app.notes.is_empty());
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn highlight_add_validates_and_stores_marks() {
        use crate::tui::server::ServerCommand;
        let mut app = select_sample_app();
        app.cursor_v = 0;
        app.scroll_y = 0;
        // Unknown file → error, nothing stored.
        match apply_server_command(
            &mut app,
            ServerCommand::HighlightAdd {
                file: "ghost.rs".into(),
                line: 1,
                start: 1,
                end: 4,
                tone: "warning".into(),
                focus: false,
            },
            None,
        ) {
            crate::tui::server::ServerReply::Error { message } => {
                assert!(message.contains("ghost.rs"), "{message}")
            }
            other => panic!("expected Error, got {other:?}"),
        }
        // Bad range → error.
        match apply_server_command(
            &mut app,
            ServerCommand::HighlightAdd {
                file: "a.rs".into(),
                line: 1,
                start: 4,
                end: 4,
                tone: "warning".into(),
                focus: false,
            },
            None,
        ) {
            crate::tui::server::ServerReply::Error { message } => {
                assert!(message.contains("start"), "{message}")
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(app.highlights.is_empty(), "rejected marks are not stored");
        // Valid mark with focus lands the cursor on the marked line.
        match apply_server_command(
            &mut app,
            ServerCommand::HighlightAdd {
                file: "a.rs".into(),
                line: 1,
                start: 1,
                end: 3,
                tone: String::new(),
                focus: true,
            },
            None,
        ) {
            crate::tui::server::ServerReply::Ok => {}
            other => panic!("expected Ok, got {other:?}"),
        }
        assert_eq!(app.highlights.len(), 1);
        assert_eq!(app.highlights[0].tone, "warning", "empty tone defaults");
        assert_eq!(app.cursor_v, 3, "focus moved to the marked +new row");
        // List + clear.
        match apply_server_command(&mut app, ServerCommand::HighlightList, None) {
            crate::tui::server::ServerReply::HighlightList { marks } => {
                assert_eq!(marks.len(), 1);
            }
            other => panic!("expected HighlightList, got {other:?}"),
        }
        match apply_server_command(&mut app, ServerCommand::HighlightClear { file: None }, None) {
            crate::tui::server::ServerReply::HighlightCleared { removed } => {
                assert_eq!(removed, 1)
            }
            other => panic!("expected HighlightCleared, got {other:?}"),
        }
        assert!(app.highlights.is_empty());
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn note_jump_command_jumps_annotated_rows() {
        use crate::tui::server::ServerCommand;
        let mut app = select_sample_app();
        // No notes yet → error reply for the agent.
        match apply_server_command(&mut app, ServerCommand::NoteJump { next: true }, None) {
            crate::tui::server::ServerReply::Error { message: msg } => {
                assert!(msg.contains("no notes"), "{msg}")
            }
            other => panic!("expected Error, got {other:?}"),
        }
        // One note on the +new row → jump lands the cursor on it.
        apply_server_command(
            &mut app,
            ServerCommand::CommentAdd {
                file: "a.rs".into(),
                text: "look here".into(),
                line: Some(1),
                hunk: None,
                focus: false,
            },
            None,
        );
        app.cursor_v = 0;
        match apply_server_command(&mut app, ServerCommand::NoteJump { next: true }, None) {
            crate::tui::server::ServerReply::Ok => {}
            other => panic!("expected Ok, got {other:?}"),
        }
        assert_eq!(app.cursor_v, 3, "cursor lands on the annotated +new row");
    }

    #[test]
    #[cfg(all(feature = "serve", unix))]
    fn comment_apply_is_idempotent_per_comment() {
        use crate::tui::app::CommentEntry;
        use crate::tui::server::ServerCommand;
        let mut app = select_sample_app();
        app.comments.push(CommentEntry {
            id: "c1".into(),
            file: "a.rs".into(),
            text: "check this".into(),
            line: Some(1),
            hunk: None,
        });
        apply_server_command(&mut app, ServerCommand::CommentApply, None);
        assert_eq!(app.notes.len(), 1, "first apply converts the comment");
        apply_server_command(&mut app, ServerCommand::CommentApply, None);
        assert_eq!(
            app.notes.len(),
            1,
            "re-running apply must not duplicate note rows"
        );
        assert!(
            !app.notes[0].text.contains("💬"),
            "renderer supplies the 💬 glyph; the note text must not carry one"
        );
        assert!(
            app.notes[0].text.contains("c1"),
            "note text keeps the comment id for `comment rm` correlation"
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
}
