//! Pure review-application state — no I/O, fully unit-testable.
//!
//! `App` owns scroll/focus state over an already-parsed [`Review`] and mutates
//! it in response to `KeyEvent`s via [`App::handle_key`]. Rendering is a pure
//! function of `&App` (see [`crate::tui::view`]). Nothing here touches the
//! terminal, so the whole interaction model can be exercised headlessly by
//! feeding scripted `KeyEvent`s and asserting on `App` fields or a rendered
//! `TestBackend` buffer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ir::{Review, ViewportQuery};

/// Review application state. The single source of truth for scroll/focus.
pub struct App {
    pub review: Review,
    /// Top virtual row of the stream viewport.
    pub scroll_y: usize,
    /// Currently focused file index (rail selection).
    pub selected_file: usize,
    /// Visible height of the stream pane (set from the drawn area).
    pub viewport_height: usize,
    /// Set when the user requests to quit.
    pub should_quit: bool,
    /// Transient status/help message for the status line.
    pub status: String,
}

impl App {
    pub fn new(review: Review) -> Self {
        let status = if review.is_empty() {
            "empty diff".to_string()
        } else {
            format!("{} file(s) — j/k scroll, Tab next file, q quit", review.file_count())
        };
        Self {
            review,
            scroll_y: 0,
            selected_file: 0,
            viewport_height: 24,
            should_quit: false,
            status,
        }
    }

    /// Maximum valid top-row so the last row remains visible.
    fn max_scroll(&self) -> usize {
        self.review
            .stream_len
            .saturating_sub(self.viewport_height)
    }

    /// Sync `selected_file` to whatever file owns the current top scroll row.
    fn sync_selected_file(&mut self) {
        if let Some(idx) = ViewportQuery::file_at_row(&self.review, self.scroll_y) {
            self.selected_file = idx;
        }
    }

    /// Handle a single key event. Pure: mutates state only, no I/O.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ctrl+C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        let half = self.viewport_height.max(1) / 2;

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            // scroll down one
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll_y = self.scroll_y.saturating_add(1).min(self.max_scroll());
                self.sync_selected_file();
            }
            // scroll up one
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_y = self.scroll_y.saturating_sub(1);
                self.sync_selected_file();
            }
            // half-page down
            KeyCode::Char('J') | KeyCode::PageDown => {
                self.scroll_y = self.scroll_y.saturating_add(half).min(self.max_scroll());
                self.sync_selected_file();
            }
            // half-page up
            KeyCode::Char('K') | KeyCode::PageUp => {
                self.scroll_y = self.scroll_y.saturating_sub(half);
            }
            // top / bottom
            KeyCode::Char('g') | KeyCode::Home => {
                self.scroll_y = 0;
                self.sync_selected_file();
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.scroll_y = self.max_scroll();
                self.sync_selected_file();
            }
            // next / prev file
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                if let Some((idx, row)) = ViewportQuery::jump_file(&self.review, self.selected_file, true)
                {
                    self.selected_file = idx;
                    self.scroll_y = row;
                    self.status = format!("→ {}", self.review.display_path(idx));
                }
            }
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
                if let Some((idx, row)) = ViewportQuery::jump_file(&self.review, self.selected_file, false)
                {
                    self.selected_file = idx;
                    self.scroll_y = row;
                    self.status = format!("← {}", self.review.display_path(idx));
                }
            }
            _ => {}
        }
    }

    /// Current focused file's display path (for status / tests).
    pub fn current_path(&self) -> &str {
        self.review.display_path(self.selected_file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;

    fn two_file_app() -> App {
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
        // pretend a small terminal
        app.viewport_height = 4;
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn quit_on_q() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn quit_on_ctrl_c() {
        let mut app = two_file_app();
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn quit_on_esc() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn scroll_down_then_up() {
        let mut app = two_file_app();
        let start = app.scroll_y;
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.scroll_y, start + 1);
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.scroll_y, start);
    }

    #[test]
    fn scroll_clamps_at_bottom() {
        let mut app = two_file_app();
        // stream_len is 10 (2 files × (1 header + 1 hunk header + 2 lines + ...)).
        // Just hammer G and ensure we never exceed max_scroll.
        app.handle_key(key(KeyCode::Char('G')));
        let max = app.max_scroll();
        assert_eq!(app.scroll_y, max);
        // scrolling further does nothing
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.scroll_y, max);
    }

    #[test]
    fn top_and_bottom() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('G')));
        assert!(app.scroll_y > 0);
        app.handle_key(key(KeyCode::Char('g')));
        assert_eq!(app.scroll_y, 0);
    }

    #[test]
    fn tab_cycles_to_next_file() {
        let mut app = two_file_app();
        assert_eq!(app.selected_file, 0);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.selected_file, 1);
        assert_eq!(app.current_path(), "b.rs");
        // wrap to first
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.selected_file, 0);
        assert_eq!(app.current_path(), "a.rs");
    }

    #[test]
    fn backtab_cycles_to_prev_file() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.selected_file, 1);
    }

    #[test]
    fn arrow_keys_scroll() {
        let mut app = two_file_app();
        let start = app.scroll_y;
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.scroll_y, start + 1);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.scroll_y, start);
    }

    #[test]
    fn right_left_navigate_files() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.selected_file, 1);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.selected_file, 0);
    }

    #[test]
    fn scroll_syncs_selected_file() {
        let mut app = two_file_app();
        // scroll down until we cross into file 1's region
        let f1_start = app.review.files[1].stream_start;
        while app.scroll_y < f1_start {
            app.handle_key(key(KeyCode::Char('j')));
        }
        assert_eq!(app.selected_file, 1);
    }
}
