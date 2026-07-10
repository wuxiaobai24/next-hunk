//! Pure review-application state — no I/O, fully unit-testable.
//!
//! `App` owns scroll/focus/search/highlight state over an already-parsed
//! [`Review`] and mutates it in response to `KeyEvent`s via [`App::handle_key`].
//! Rendering is a pure function of `&App` (see [`crate::tui::view`]). Nothing
//! here touches the terminal, so the whole interaction model can be exercised
//! headlessly by feeding scripted `KeyEvent`s and asserting on `App` fields or
//! a rendered `TestBackend` buffer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::highlight::{HighlightCache, Highlighter};
use crate::ir::{Review, ViewportQuery};

/// Which input mode the TUI is in. Determines how `handle_key` routes keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Normal navigation.
    #[default]
    Normal,
    /// Editing an in-stream `/` content search query.
    Search,
    /// Editing a file-rail path filter.
    Filter,
}

/// In-stream content search state.
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// Current query string (also while typing).
    pub query: String,
    /// Sorted stream rows that contain a (case-insensitive) match.
    pub matches: Vec<usize>,
    /// Index into `matches` of the current focus.
    pub current: usize,
    /// True once a search has been finalized (Enter) and is active.
    pub active: bool,
}

impl Search {
    pub fn clear(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.current = 0;
        self.active = false;
    }
}

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

    /// Syntax highlight on/off. ON by default.
    pub highlight_on: bool,
    /// Lazily-filled highlight cache.
    pub cache: HighlightCache,
    /// Loaded highlighter (syntect or no-op, depending on feature).
    pub highlighter: Highlighter,

    /// Current input mode (normal / search-edit / filter-edit).
    pub mode: InputMode,
    /// In-stream content search.
    pub search: Search,
    /// File-rail path filter substring (empty = show all).
    pub path_filter: String,
}

impl App {
    pub fn new(review: Review) -> Self {
        Self::with_highlighter(review, Highlighter::load().unwrap_or_else(|_| Highlighter::load_noop()))
    }

    /// Construct with an explicit highlighter (used by `run_review_tui` to
    /// reuse a single loaded `Highlighter` and by tests).
    pub fn with_highlighter(review: Review, highlighter: Highlighter) -> Self {
        let status = if review.is_empty() {
            "empty diff".to_string()
        } else {
            format!("{} file(s) — j/k scroll, / search, f filter, H highlight, q quit", review.file_count())
        };
        Self {
            review,
            scroll_y: 0,
            selected_file: 0,
            viewport_height: 24,
            should_quit: false,
            status,
            highlight_on: true,
            cache: HighlightCache::new(),
            highlighter,
            mode: InputMode::Normal,
            search: Search::default(),
            path_filter: String::new(),
        }
    }

    /// Maximum valid top-row so the last row remains visible.
    pub fn max_scroll(&self) -> usize {
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
        // Ctrl+C always quits, regardless of mode.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // Route by input mode first.
        match self.mode {
            InputMode::Search => {
                self.handle_search_input(key);
                return;
            }
            InputMode::Filter => {
                self.handle_filter_input(key);
                return;
            }
            InputMode::Normal => {}
        }

        self.handle_normal_key(key);
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        let half = self.viewport_height.max(1) / 2;

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                // If a search is active, Esc clears it; otherwise quit.
                if self.search.active {
                    self.search.clear();
                    self.status = "search cleared".into();
                } else {
                    self.should_quit = true;
                }
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
                if let Some((idx, row)) =
                    ViewportQuery::jump_file(&self.review, self.selected_file, true)
                {
                    self.selected_file = idx;
                    self.scroll_y = row;
                    self.status = format!("→ {}", self.review.display_path(idx));
                }
            }
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
                if let Some((idx, row)) =
                    ViewportQuery::jump_file(&self.review, self.selected_file, false)
                {
                    self.selected_file = idx;
                    self.scroll_y = row;
                    self.status = format!("← {}", self.review.display_path(idx));
                }
            }
            // toggle highlight
            KeyCode::Char('H') => {
                self.highlight_on = !self.highlight_on;
                if !self.highlight_on {
                    self.cache.invalidate();
                    self.status = "highlight off".into();
                } else {
                    self.status = "highlight on".into();
                }
            }
            // begin in-stream search
            KeyCode::Char('/') => {
                self.mode = InputMode::Search;
                self.search.query.clear();
                self.status = "search: ".into();
            }
            // begin path filter
            KeyCode::Char('f') => {
                self.mode = InputMode::Filter;
                self.path_filter.clear();
                self.status = "filter: ".into();
            }
            // next / prev search match
            KeyCode::Char('n') => {
                self.advance_match(true);
            }
            KeyCode::Char('N') => {
                self.advance_match(false);
            }
            _ => {}
        }
    }

    /// Handle keys while editing the search query.
    fn handle_search_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.finalize_search();
                self.mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
                self.search.clear();
                self.status = "search cancelled".into();
            }
            KeyCode::Backspace => {
                self.search.query.pop();
                self.status = format!("search: {}", self.search.query);
            }
            KeyCode::Char(c) => {
                self.search.query.push(c);
                self.status = format!("search: {}", self.search.query);
            }
            _ => {}
        }
    }

    /// Handle keys while editing the path filter.
    fn handle_filter_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.apply_filter();
                self.mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
                self.path_filter.clear();
                self.status = "filter cleared".into();
            }
            KeyCode::Backspace => {
                self.path_filter.pop();
                self.status = format!("filter: {}", self.path_filter);
            }
            KeyCode::Char(c) => {
                self.path_filter.push(c);
                self.status = format!("filter: {}", self.path_filter);
            }
            _ => {}
        }
    }

    /// Run the search: scan the whole stream for the query (case-insensitive).
    fn finalize_search(&mut self) {
        if self.search.query.trim().is_empty() {
            self.search.clear();
            self.status = "search empty".into();
            return;
        }
        let needle = self.search.query.to_lowercase();
        let mut matches = Vec::new();
        for row in 0..self.review.stream_len {
            if let Some(text) = ViewportQuery::row_text(&self.review, row) {
                if text.to_lowercase().contains(&needle) {
                    matches.push(row);
                }
            }
        }
        self.search.matches = matches;
        self.search.current = 0;
        self.search.active = true;
        if self.search.matches.is_empty() {
            self.status = format!("no matches for {:?}", self.search.query);
        } else {
            let row = self.search.matches[0];
            self.scroll_y = row.min(self.max_scroll());
            self.sync_selected_file();
            self.status = format!(
                "match {}/{}: {:?}",
                self.search.current + 1,
                self.search.matches.len(),
                self.search.query
            );
        }
    }

    /// Move to the next/prev search match (wraps).
    fn advance_match(&mut self, forward: bool) {
        if self.search.matches.is_empty() {
            return;
        }
        let n = self.search.matches.len();
        self.search.current = if forward {
            (self.search.current + 1) % n
        } else {
            self.search.current.checked_sub(1).unwrap_or(n - 1)
        };
        let row = self.search.matches[self.search.current];
        self.scroll_y = row.min(self.max_scroll());
        self.sync_selected_file();
        self.status = format!(
            "match {}/{}: {:?}",
            self.search.current + 1,
            self.search.matches.len(),
            self.search.query
        );
    }

    /// Apply the path filter: clamp selected_file into the visible set.
    fn apply_filter(&mut self) {
        if self.path_filter.trim().is_empty() {
            self.status = "filter cleared".into();
            return;
        }
        // Keep selected_file valid: if it no longer matches, jump to the first
        // file that does.
        let needle = self.path_filter.to_lowercase();
        let selected_matches = self
            .review
            .display_path(self.selected_file)
            .to_lowercase()
            .contains(&needle);
        if !selected_matches {
            if let Some(first) = self.visible_files().first().copied() {
                self.selected_file = first;
                self.scroll_y = ViewportQuery::file_start_row(&self.review, first);
            } else {
                self.status = format!("no files match {:?}", self.path_filter);
                return;
            }
        }
        self.status = format!("filter: {:?} ({} files)", self.path_filter, self.visible_files().len());
    }

    /// Indices of files matching the current path filter (all if empty).
    pub fn visible_files(&self) -> Vec<usize> {
        if self.path_filter.trim().is_empty() {
            (0..self.review.file_count()).collect()
        } else {
            let needle = self.path_filter.to_lowercase();
            (0..self.review.file_count())
                .filter(|i| {
                    self.review
                        .display_path(*i)
                        .to_lowercase()
                        .contains(&needle)
                })
                .collect()
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
    use crate::highlight::Highlighter;
    use crate::ir::parse_unified_diff;

    fn highlighter() -> Highlighter {
        Highlighter::load_noop()
    }

    fn two_file_app() -> App {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new value
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-foo
+bar value
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 4;
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
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
    fn quit_on_esc_when_no_search() {
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
        app.handle_key(key(KeyCode::Char('G')));
        let max = app.max_scroll();
        assert_eq!(app.scroll_y, max);
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
        let f1_start = app.review.files[1].stream_start;
        while app.scroll_y < f1_start {
            app.handle_key(key(KeyCode::Char('j')));
        }
        assert_eq!(app.selected_file, 1);
    }

    // ---- highlight ----

    #[test]
    fn highlight_on_by_default() {
        let app = two_file_app();
        assert!(app.highlight_on);
    }

    #[test]
    fn toggle_highlight_flips_state() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('H')));
        assert!(!app.highlight_on);
        app.handle_key(key(KeyCode::Char('H')));
        assert!(app.highlight_on);
    }

    // ---- search ----

    #[test]
    fn search_finds_matches_and_jumps() {
        let mut app = two_file_app();
        // `/`, type "value", Enter
        app.handle_key(key(KeyCode::Char('/')));
        assert_eq!(app.mode, InputMode::Search);
        for c in "value".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.search.active);
        // "value" appears in both files (+new value, +bar value)
        assert_eq!(app.search.matches.len(), 2);
        assert!(app.scroll_y > 0);
    }

    #[test]
    fn search_n_cycles_and_wraps() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('/')));
        for c in "value".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.search.matches.len(), 2);
        assert_eq!(app.search.current, 0);
        // next
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(app.search.current, 1);
        // next again wraps to 0
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(app.search.current, 0);
        // N goes backward (wraps to last)
        app.handle_key(key(KeyCode::Char('N')));
        assert_eq!(app.search.current, 1);
    }

    #[test]
    fn search_esc_while_editing_cancels() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(char_key('x'));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, InputMode::Normal);
        assert!(!app.search.active);
    }

    #[test]
    fn search_no_matches() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('/')));
        for c in "zzzzz".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Enter));
        assert!(app.search.active);
        assert!(app.search.matches.is_empty());
    }

    #[test]
    fn esc_clears_active_search() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('/')));
        for c in "value".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Enter));
        assert!(app.search.active);
        // Esc now clears search rather than quitting
        app.handle_key(key(KeyCode::Esc));
        assert!(!app.search.active);
        assert!(!app.should_quit, "Esc should clear search, not quit");
    }

    #[test]
    fn backspace_edits_search_query() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('/')));
        for c in "abc".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.search.query, "ab");
    }

    // ---- path filter ----

    #[test]
    fn path_filter_narrows_files() {
        let mut app = two_file_app();
        assert_eq!(app.visible_files().len(), 2);
        app.handle_key(key(KeyCode::Char('f')));
        assert_eq!(app.mode, InputMode::Filter);
        for c in "b.rs".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, InputMode::Normal);
        let vis = app.visible_files();
        assert_eq!(vis, vec![1]);
        // selected jumped to the matching file
        assert_eq!(app.selected_file, 1);
    }

    #[test]
    fn path_filter_no_match_keeps_filter() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('f')));
        for c in "zzz".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Enter));
        assert!(app.visible_files().is_empty());
    }

    #[test]
    fn filter_esc_clears() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('f')));
        app.handle_key(char_key('a'));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.path_filter.is_empty());
        assert_eq!(app.visible_files().len(), 2);
    }

    #[test]
    fn ctrl_c_quits_even_in_search_mode() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }
}
