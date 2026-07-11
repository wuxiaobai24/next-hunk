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
use std::time::Instant;

use anyhow::{Context, Result};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::ir::Review;
use crate::tui::app::App;
use crate::tui::watch::{Watcher, DEBOUNCE};

pub mod app;
pub mod input;
pub mod view;
pub mod watch;

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// A live-reload source: produces a fresh unified-diff string on demand.
/// Carried into the run loop so `--watch` can re-fetch the diff without
/// touching the terminal from a background thread.
pub type Reloader = Box<dyn FnMut() -> Result<String>>;

/// RAII guard that restores the terminal on drop. crossterm 0.28 ships no
/// built-in guard, so we define our own to guarantee cleanup even on panic.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

/// Run the interactive review UI over an already-parsed [`Review`].
///
/// `reloader` enables `--watch`: when present (and a [`Watcher`] can be
/// started), the loop hot-reloads the review on filesystem changes, preserving
/// scroll / selection as described in [`App::reload_review`].
///
/// Returns `Ok(())` on clean quit. Errors only on fatal terminal I/O. If the
/// process's stdout is not a tty, crossterm will typically still enter raw
/// mode and the caller may choose to fall back to a non-interactive summary.
pub fn run_review_tui(
    review: Review,
    reloader: Option<Reloader>,
    start_highlight: bool,
) -> Result<()> {
    if review.is_empty() {
        anyhow::bail!("nothing to review (empty diff)");
    }

    enable_raw_mode().context("enable raw mode")?;
    let _guard = RawModeGuard; // restore on drop / panic
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture
    )
    .context("enter alternate screen")?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear()?;

    let mut app = App::new(review);
    app.highlight_on = start_highlight;
    // prime viewport height from an initial draw
    run_loop(&mut terminal, &mut app, reloader)?;
    Ok(())
}

fn run_loop(
    terminal: &mut Tui,
    app: &mut App,
    mut reloader: Option<Reloader>,
) -> Result<()> {
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

        let event = input::read_event(250)?;
        let Some(event) = event else {
            continue;
        };

        if let Event::Key(key) = event {
            app.handle_key(key);
            if app.should_quit {
                return Ok(());
            }
        } else if let Event::Resize(_, _) = event {
            // next draw will pick up the new size
            continue;
        }
        // other events (mouse) ignored in MVP
    }
}

/// Fetch a fresh diff via the reloader and hot-swap it into `app`.
fn reload_once(app: &mut App, reloader: &mut Reloader) {
    match reloader() {
        Ok(text) => app.reload_review(&text),
        Err(e) => {
            app.status = format!("reload error: {e}");
        }
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
        assert!(run_review_tui(empty, None, true).is_err());
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
        assert!(rendered.contains('/'), "search prompt should render: {rendered}");

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
}
