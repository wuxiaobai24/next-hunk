//! Pure review-application state — no I/O, fully unit-testable.
//!
//! `App` owns scroll/focus/search/highlight state over an already-parsed
//! [`Review`] and mutates it in response to `KeyEvent`s via [`App::handle_key`].
//! Rendering is a pure function of `&App` (see [`crate::tui::view`]). Nothing
//! here touches the terminal, so the whole interaction model can be exercised
//! headlessly by feeding scripted `KeyEvent`s and asserting on `App` fields or
//! a rendered `TestBackend` buffer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::highlight::{HighlightCache, Highlighter};
use crate::ir::{Review, ViewportQuery};
use crate::tui::theme::{Theme, ThemeMode};

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

/// Stream layout: single unified column or two side-by-side columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// One column, traditional unified diff (default).
    #[default]
    Unified,
    /// Two columns, old on the left / new on the right.
    Split,
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
    /// The unfiltered original review. Kept so the ignore-whitespace toggle can
    /// re-derive the active [`review`](Self::review) from source each time.
    pub base_review: Review,
    /// When true, whitespace-only +/- pairs are collapsed to context in the
    /// active `review`. Toggled with `W`.
    pub ignore_ws: bool,
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

    /// Show a line-number gutter column. ON by default.
    pub line_numbers_on: bool,
    /// Highlight changed words within a line (word-diff). ON by default.
    pub word_diff_on: bool,
    /// Stream layout (unified vs split). Unified by default.
    pub view_mode: ViewMode,
    /// Split left/right column ratio (0..=100 = percent for the LEFT column).
    /// Only meaningful under [`ViewMode::Split`].
    pub split_ratio: u16,

    /// User's theme choice (dark / light / auto). `theme` is the resolved
    /// palette the view reads from; `t` cycles the mode and refreshes it.
    pub theme_mode: ThemeMode,
    pub theme: Theme,

    /// Current input mode (normal / search-edit / filter-edit).
    pub mode: InputMode,
    /// In-stream content search.
    pub search: Search,
    /// File-rail path filter substring (empty = show all).
    pub path_filter: String,
    /// Pending first key of a two-key sequence (`]` / `[`). Cleared on the next
    /// key or after a short no-op. Used to spell `]h` / `[h` (next/prev hunk).
    pub pending_prefix: Option<char>,
    /// A pending "open in editor" request. Set when the user presses `o` on a
    /// code line; the run loop (which owns the terminal) consumes it, suspends
    /// the TUI, spawns `$EDITOR`, and resumes. Keeping it as a field (not an
    /// I/O side effect) keeps `App` pure and headless-testable.
    pub open_request: Option<OpenTarget>,
}

/// A request to open a file in an external editor at a line, produced when the
/// user presses `o` on a code row. The path is relative to the repo workdir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    /// Repo-relative path of the file to open.
    pub path: String,
    /// 1-based line number to jump to (new-side line number when available).
    pub line: u32,
}

impl App {
    pub fn new(review: Review) -> Self {
        Self::with_highlighter(review, Highlighter::load().unwrap_or_else(|_| Highlighter::load_noop()))
    }

    /// Construct with an explicit highlighter (used by `run_review_tui` to
    /// reuse a single loaded `Highlighter` and by tests). Defaults to the dark
    /// theme. Use [`App::with_theme`] to inject a config-driven theme.
    pub fn with_highlighter(review: Review, highlighter: Highlighter) -> Self {
        Self::with_theme(review, highlighter, ThemeMode::Dark)
    }

    /// Construct with an explicit highlighter and theme mode (used by
    /// `run_review_tui` to honor `config.toml`'s `theme`).
    pub fn with_theme(
        review: Review,
        highlighter: Highlighter,
        theme_mode: ThemeMode,
    ) -> Self {
        let status = if review.is_empty() {
            "empty diff".to_string()
        } else {
            format!("{} file(s) — j/k scroll, ]h/[h next/prev hunk, / search, f filter, H highlight, q quit", review.file_count())
        };
        let theme = theme_mode.to_theme();
        Self {
            review: review.clone(),
            base_review: review,
            ignore_ws: false,
            scroll_y: 0,
            selected_file: 0,
            viewport_height: 24,
            should_quit: false,
            status,
            highlight_on: true,
            cache: HighlightCache::new(),
            highlighter,
            line_numbers_on: true,
            word_diff_on: true,
            view_mode: ViewMode::Unified,
            split_ratio: 50,
            theme_mode,
            theme,
            mode: InputMode::Normal,
            search: Search::default(),
            path_filter: String::new(),
            pending_prefix: None,
            open_request: None,
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

    /// Move the scroll position by `delta` rows, clamped to `[0, max_scroll()]`.
    /// Positive scrolls down, negative up. Used by both keys and mouse wheel so
    /// they share one clamp/sync path.
    fn scroll_by(&mut self, delta: i64) {
        let next = if delta >= 0 {
            self.scroll_y
                .saturating_add(delta as usize)
                .min(self.max_scroll())
        } else {
            self.scroll_y
                .saturating_sub((-delta) as usize)
        };
        self.scroll_y = next;
        self.sync_selected_file();
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

    /// Handle a single mouse event. Pure: mutates state only, no I/O.
    ///
    /// Only wheel scroll is handled (one row per notch; Shift widens it to a
    /// half-page, mirroring `j`/`J`). Clicks/drags/moves are ignored. Keeping
    /// this in `App` means mouse behavior is exercisable headlessly, the same
    /// as keys.
    pub fn handle_mouse(&mut self, ev: MouseEvent) {
        let half = (self.viewport_height.max(1) / 2) as i64;
        match ev.kind {
            MouseEventKind::ScrollDown => {
                if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    self.scroll_by(half);
                } else {
                    self.scroll_by(1);
                }
            }
            MouseEventKind::ScrollUp => {
                if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    self.scroll_by(-half);
                } else {
                    self.scroll_by(-1);
                }
            }
            // Clicks, drags, and horizontal scroll are ignored for now.
            _ => {}
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        let half = self.viewport_height.max(1) / 2;

        // Two-key sequence handling for `]h` / `[h` (next/prev hunk).
        // If a prefix is pending, consume it now.
        if let Some(prefix) = self.pending_prefix.take() {
            match (prefix, key.code) {
                (']', KeyCode::Char('h')) => {
                    self.jump_hunk(true);
                    return;
                }
                ('[', KeyCode::Char('h')) => {
                    self.jump_hunk(false);
                    return;
                }
                _ => {
                    // Unrecognized second key: fall through to normal dispatch.
                    // (A lone `]`/`[` that isn't followed by `h` is discarded.)
                }
            }
        }

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
            // hunk navigation prefixes: `]` / `[` await a following `h`
            KeyCode::Char(']') => {
                self.pending_prefix = Some(']');
                self.status = "]".into();
            }
            KeyCode::Char('[') => {
                self.pending_prefix = Some('[');
                self.status = "[".into();
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
            // toggle line-number gutter
            KeyCode::Char('#') => {
                self.line_numbers_on = !self.line_numbers_on;
                self.status = if self.line_numbers_on {
                    "line numbers on".into()
                } else {
                    "line numbers off".into()
                };
            }
            // toggle word-level inline diff
            KeyCode::Char('w') => {
                self.word_diff_on = !self.word_diff_on;
                self.status = if self.word_diff_on {
                    "word diff on".into()
                } else {
                    "word diff off".into()
                };
            }
            // toggle ignore-whitespace view (collapse whitespace-only changes)
            KeyCode::Char('W') => {
                self.ignore_ws = !self.ignore_ws;
                self.apply_ignore_ws();
                self.status = if self.ignore_ws {
                    "ignore-whitespace on".into()
                } else {
                    "ignore-whitespace off".into()
                };
            }
            // toggle unified / split layout
            KeyCode::Char('s') => {
                self.view_mode = match self.view_mode {
                    ViewMode::Unified => ViewMode::Split,
                    ViewMode::Split => ViewMode::Unified,
                };
                self.status = match self.view_mode {
                    ViewMode::Unified => "unified layout".into(),
                    ViewMode::Split => "split layout".into(),
                };
            }
            // cycle theme: dark → light → auto → dark
            KeyCode::Char('t') => {
                self.theme_mode = self.theme_mode.cycle();
                self.theme = self.theme_mode.to_theme();
                self.status = format!("theme: {}", self.theme_mode.name());
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
            // open the focused line's file in $EDITOR
            KeyCode::Char('o') => {
                match self.compute_open_target() {
                    Some(t) => {
                        self.status =
                            format!("opening {}:{}…", t.path, t.line);
                        self.open_request = Some(t);
                    }
                    None => {
                        self.status = "nothing to open here (move to a code line)".into();
                    }
                }
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

    /// Hot-reload the review from freshly produced diff `text`.
    ///
    /// Preserves as much navigation state as possible:
    /// - `selected_file` is re-resolved by matching the old display path;
    ///   if gone, clamps to 0.
    /// - `scroll_y` is kept but clamped to the new `max_scroll`.
    /// - an active search is re-run against the new content.
    /// - the highlight cache is invalidated (line numbers/content may shift).
    ///
    /// On parse failure the old review is kept and an error status is set.
    pub fn reload_review(&mut self, text: &str) {
        if text.trim().is_empty() {
            self.status = "reloaded (empty diff)".into();
            return;
        }
        let new_review = match crate::ir::parse_unified_diff(text) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("reload failed: {e}");
                return;
            }
        };

        // Preserve selected_file by display path.
        let old_path = self.current_path().to_string();
        self.selected_file = new_review
            .files
            .iter()
            .position(|f| f.display_path == old_path)
            .unwrap_or(0);

        // New base; re-apply the ignore-ws view if it's active so the toggled
        // state survives a hot-reload.
        self.base_review = new_review;
        self.review = if self.ignore_ws {
            crate::ir::strip_whitespace_changes(&self.base_review)
        } else {
            self.base_review.clone()
        };
        self.cache.invalidate();

        // Clamp scroll into the new bounds.
        self.scroll_y = self.scroll_y.min(self.max_scroll());
        self.sync_selected_file();

        // Re-run an active search against the new content.
        if self.search.active {
            let query = self.search.query.clone();
            self.search.clear();
            self.search.query = query;
            if self.search.query.trim().is_empty() {
                self.search.active = false;
            } else {
                self.finalize_search_silent();
            }
        }

        self.status = format!("reloaded ({} files)", self.review.file_count());
    }

    /// Run the search without jumping/overwriting a caller-set status prefix.
    /// Mirrors `finalize_search` but is quiet about the status message.
    fn finalize_search_silent(&mut self) {
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
        if !self.search.matches.is_empty() {
            self.scroll_y = self.search.matches[0].min(self.max_scroll());
            self.sync_selected_file();
        }
    }

    /// Jump to the next/previous hunk header, scrolling it to the top of the
    /// viewport and syncing the rail selection. Wraps across file boundaries.
    fn jump_hunk(&mut self, forward: bool) {
        match ViewportQuery::jump_hunk(&self.review, self.scroll_y, forward) {
            Some(row) => {
                self.scroll_y = row.min(self.max_scroll());
                self.sync_selected_file();
                let path = self.current_path().to_string();
                let dir = if forward { "→" } else { "←" };
                self.status = format!("{dir} hunk @ {path}:{}", row);
            }
            None => {
                self.status = "no hunks to jump to".into();
            }
        }
    }

    /// Re-derive the active `review` from `base_review` according to the
    /// `ignore_ws` flag, preserving scroll/selection. Stream layout is stable
    /// (the transform keeps row counts), so positions stay valid; we just clamp
    /// defensively and invalidate the highlight cache since line kinds change.
    fn apply_ignore_ws(&mut self) {
        self.review = if self.ignore_ws {
            crate::ir::strip_whitespace_changes(&self.base_review)
        } else {
            self.base_review.clone()
        };
        self.cache.invalidate();
        self.scroll_y = self.scroll_y.min(self.max_scroll());
        self.sync_selected_file();
    }

    /// Compute the file + line to open for the `o` (open in editor) action.    ///
    /// The TUI is a top-anchored scroll view (no row cursor), so `o` targets
    /// the top visible stream row. If that row is a header (file or hunk),
    /// scan forward within the viewport to the first code line. For a code line
    /// we prefer the new-side line number (so edits land on the live file);
    /// deletes have no new-side, so they fall back to the old-side number.
    /// `None` when no code line is visible or the file has no on-disk path.
    fn compute_open_target(&self) -> Option<OpenTarget> {
        // Search the visible window for the first code line with a line number.
        let start = self.scroll_y;
        let end = self.review.stream_len.min(start + self.viewport_height.max(1));
        for row in start..end {
            // Header rows (file/hunk) have no line number — skip them and keep
            // scanning for the first code line.
            let Some((old_no, new_no)) = ViewportQuery::row_line_numbers(&self.review, row) else {
                continue;
            };
            let (file_idx, _) = ViewportQuery::file_and_line(&self.review, row)?;
            let line = new_no.or(old_no)?;
            let file = self.review.files.get(file_idx)?;
            let path = file
                .new_path
                .clone()
                .filter(|p| p != "/dev/null")
                .or_else(|| file.old_path.clone().filter(|p| p != "/dev/null"))?;
            if path == "unknown" {
                return None;
            }
            return Some(OpenTarget { path, line });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::Highlighter;
    use crate::ir::{parse_unified_diff, Viewport};

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

    #[test]
    fn toggle_line_numbers() {
        let mut app = two_file_app();
        assert!(app.line_numbers_on, "line numbers on by default");
        app.handle_key(char_key('#'));
        assert!(!app.line_numbers_on);
        app.handle_key(char_key('#'));
        assert!(app.line_numbers_on);
    }

    #[test]
    fn toggle_word_diff() {
        let mut app = two_file_app();
        assert!(app.word_diff_on, "word diff on by default");
        app.handle_key(char_key('w'));
        assert!(!app.word_diff_on);
        app.handle_key(char_key('w'));
        assert!(app.word_diff_on);
    }

    #[test]
    fn toggle_view_mode() {
        let mut app = two_file_app();
        assert_eq!(app.view_mode, ViewMode::Unified);
        app.handle_key(char_key('s'));
        assert_eq!(app.view_mode, ViewMode::Split);
        app.handle_key(char_key('s'));
        assert_eq!(app.view_mode, ViewMode::Unified);
    }

    // ---- ignore whitespace (W) ----

    fn ws_change_app() -> App {
        // A line whose only change is indentation: `-  x` → `+    x`.
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
 fn f() {
-  x
+    x
 }
",
        )
        .unwrap();
        // stream: 0=file header, 1=hunk header, 2=ctx, 3=-del, 4=+add, 5=ctx
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 10;
        app
    }

    #[test]
    fn ignore_ws_off_by_default() {
        let app = ws_change_app();
        assert!(!app.ignore_ws);
        // original has 1 insert + 1 delete
        assert_eq!(app.review.inserts, 1);
        assert_eq!(app.review.deletes, 1);
    }

    #[test]
    fn ignore_ws_collapses_whitespace_only_changes() {
        let mut app = ws_change_app();
        app.handle_key(char_key('W'));
        assert!(app.ignore_ws);
        assert_eq!(app.review.inserts, 0, "whitespace-only add collapsed");
        assert_eq!(app.review.deletes, 0, "whitespace-only del collapsed");
        assert!(app.status.contains("ignore-whitespace on"));
    }

    #[test]
    fn ignore_ws_toggle_back_restores_original() {
        let mut app = ws_change_app();
        app.handle_key(char_key('W'));
        assert_eq!(app.review.inserts, 0);
        app.handle_key(char_key('W'));
        assert!(!app.ignore_ws);
        assert_eq!(app.review.inserts, 1);
        assert_eq!(app.review.deletes, 1);
    }

    #[test]
    fn ignore_ws_keeps_real_changes() {
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
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 10;
        app.handle_key(char_key('W'));
        // genuine content change → still counted
        assert_eq!(app.review.inserts, 1);
        assert_eq!(app.review.deletes, 1);
    }

    #[test]
    fn ignore_ws_preserves_scroll_and_layout() {
        let mut app = ws_change_app();
        // small viewport so scroll_y=2 is a valid position within the stream.
        app.viewport_height = 2;
        app.scroll_y = 2;
        let len_before = app.review.stream_len;
        app.handle_key(char_key('W'));
        assert_eq!(app.review.stream_len, len_before, "layout stable");
        assert_eq!(app.scroll_y, 2, "scroll preserved");
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

    // ---- hunk navigation (]h / [h) ----

    fn multi_hunk_app() -> App {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
 ctx
-old1
+new1
@@ -5,2 +5,2 @@
 ctx2
-old2
+new2
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,1 +1,1 @@
-foo
+bar
@@ -3,1 +3,1 @@
-baz
+qux
",
        )
        .unwrap();
        // stream layout:
        //   row 0  file0 header (a.rs)
        //   row 1  hunk0 header  ┐ a.rs
        //   row 2  ctx            │
        //   row 3  -old1          │
        //   row 4  +new1          ┘
        //   row 5  hunk1 header  ┐ a.rs
        //   row 6  ctx2           │
        //   row 7  -old2          │
        //   row 8  +new2          ┘
        //   row 9  file1 header (b.rs)
        //   row 10 hunk0 header  ┐ b.rs
        //   row 11 -foo           │
        //   row 12 +bar           ┘
        //   row 13 hunk1 header  ┐ b.rs
        //   row 14 -baz           │
        //   row 15 +qux           ┘
        // → hunk_starts = [1, 5, 10, 13]
        let mut app = App::with_highlighter(review, highlighter());
        // height 1 so max_scroll = stream_len-1 and every hunk row is reachable.
        app.viewport_height = 1;
        app
    }

    #[test]
    fn prefix_h_jumps_to_next_hunk() {
        let mut app = multi_hunk_app();
        let seq = |app: &mut App| {
            app.handle_key(char_key(']'));
            app.handle_key(char_key('h'));
        };
        // start at row 0 → first hunk (1)
        seq(&mut app);
        assert_eq!(app.scroll_y, 1);
        assert_eq!(app.pending_prefix, None);
        // → second hunk (5)
        seq(&mut app);
        assert_eq!(app.scroll_y, 5);
        // → third hunk (10), now in file b.rs
        seq(&mut app);
        assert_eq!(app.scroll_y, 10);
        assert_eq!(app.selected_file, 1, "rail should sync to file b.rs");
        // → fourth hunk (13)
        seq(&mut app);
        assert_eq!(app.scroll_y, 13);
        // wraps to the first hunk (1)
        seq(&mut app);
        assert_eq!(app.scroll_y, 1);
    }

    #[test]
    fn bracket_h_jumps_to_previous_hunk() {
        let mut app = multi_hunk_app();
        // move forward to the last hunk first (row 13): 4 jumps → 1, 5, 10, 13
        for _ in 0..4 {
            app.handle_key(char_key(']'));
            app.handle_key(char_key('h'));
        }
        assert_eq!(app.scroll_y, 13);
        // [h → previous hunk (10)
        app.handle_key(char_key('['));
        app.handle_key(char_key('h'));
        assert_eq!(app.scroll_y, 10);
        // [h → previous hunk (5)
        app.handle_key(char_key('['));
        app.handle_key(char_key('h'));
        assert_eq!(app.scroll_y, 5);
        // [h → previous hunk (1)
        app.handle_key(char_key('['));
        app.handle_key(char_key('h'));
        assert_eq!(app.scroll_y, 1);
        // [h wraps to the last hunk (13)
        app.handle_key(char_key('['));
        app.handle_key(char_key('h'));
        assert_eq!(app.scroll_y, 13);
    }

    #[test]
    fn lone_prefix_is_discarded_on_unrelated_key() {
        let mut app = multi_hunk_app();
        let start = app.scroll_y;
        app.handle_key(char_key(']')); // pending
        assert_eq!(app.pending_prefix, Some(']'));
        // press an unrelated key (e.g. j) → prefix discarded, j handled
        app.handle_key(char_key('j'));
        assert_eq!(app.pending_prefix, None);
        assert_eq!(app.scroll_y, start + 1);
    }

    #[test]
    fn hunk_jump_status_set() {
        let mut app = multi_hunk_app();
        app.handle_key(char_key(']'));
        app.handle_key(char_key('h'));
        assert!(app.status.contains("hunk"));
    }

    // ---- open in editor (o) ----

    fn openable_app() -> App {
        // Hunk at line 10 → context(old=10,new=10), -old(old=11), +new(new=11).
        let review = parse_unified_diff(
            "\
diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,3 @@
 context line
-old line
+new line
",
        )
        .unwrap();
        // stream layout: 0=file header, 1=hunk header, 2=ctx, 3=-old, 4=+new
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 10;
        app
    }

    #[test]
    fn o_on_top_code_line_requests_open() {
        let mut app = openable_app();
        // scroll to the context line (row 2) so the top visible row is code
        app.scroll_y = 2;
        app.handle_key(char_key('o'));
        let target = app
            .open_request
            .expect("o on a code line should set an open request");
        assert_eq!(target.path, "src/main.rs");
        // context line → new-side number 10
        assert_eq!(target.line, 10);
    }

    #[test]
    fn o_prefers_new_side_line_number() {
        let mut app = openable_app();
        // scroll to the +new line (row 4) so top visible is an add line
        app.scroll_y = 4;
        app.handle_key(char_key('o'));
        let target = app.open_request.expect("open request set");
        assert_eq!(target.line, 11, "add line should use new-side number");
    }

    #[test]
    fn o_falls_back_to_old_side_on_delete_line() {
        let mut app = openable_app();
        // scroll to the -old line (row 3) so top visible is a delete line
        app.scroll_y = 3;
        app.handle_key(char_key('o'));
        let target = app.open_request.expect("open request set");
        assert_eq!(target.line, 11, "delete line falls back to old-side number");
    }

    #[test]
    fn o_on_header_scans_forward_to_first_code_line() {
        let mut app = openable_app();
        // top of the file: scroll_y=0 is the file header. o should scan forward
        // to the first code line (ctx at row 2).
        app.scroll_y = 0;
        app.handle_key(char_key('o'));
        let target = app.open_request.expect("should scan to a code line");
        assert_eq!(target.line, 10);
    }

    #[test]
    fn o_clears_request_each_press() {
        let mut app = openable_app();
        app.scroll_y = 2;
        app.handle_key(char_key('o'));
        assert!(app.open_request.is_some());
        // simulate the run loop consuming it
        let _ = app.open_request.take();
        assert!(app.open_request.is_none());
    }

    #[test]
    fn o_with_no_code_visible_is_noop() {
        let mut app = openable_app();
        // viewport height 0 → no visible code row
        app.viewport_height = 0;
        app.handle_key(char_key('o'));
        assert!(app.open_request.is_none());
        assert!(app.status.contains("nothing"));
    }

    // ---- reload_review (watch hot-reload) ----

    /// Two patches that differ in content but keep the same file paths, so we
    /// can assert selected_file is preserved across a reload.
    fn reload_pair() -> (String, String) {
        let before = "\
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
"
        .to_string();
        // after: same two files, different body text, b.rs grows a line
        let after = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+changed
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,2 +1,2 @@
 ctx
-foo
+bar
"
        .to_string();
        (before, after)
    }

    #[test]
    fn reload_preserves_selected_file_by_path() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        // move to the second file (b.rs)
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.selected_file, 1);
        assert_eq!(app.current_path(), "b.rs");

        app.reload_review(&after);
        assert_eq!(app.current_path(), "b.rs", "selected path preserved");
        assert_eq!(app.selected_file, 1);
        assert!(app.status.contains("reloaded"));
    }

    #[test]
    fn reload_clamps_scroll_to_new_bounds() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        // scroll to the bottom of the old review
        app.handle_key(key(KeyCode::Char('G')));
        let old_scroll = app.scroll_y;
        assert_eq!(old_scroll, app.max_scroll());

        app.reload_review(&after);
        // scroll must not exceed the new max_scroll
        assert!(app.scroll_y <= app.max_scroll(), "scroll {} > max {}", app.scroll_y, app.max_scroll());
    }

    #[test]
    fn reload_invalidates_highlight_cache() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        // populate the cache by drawing a line
        let _ = ViewportQuery::rows(&app.review, Viewport { start: 0, height: 2 });
        app.cache.get_or_highlight(0, 1, "a.rs", "old", &app.highlighter);
        assert!(!app.cache.is_empty());

        app.reload_review(&after);
        assert!(app.cache.is_empty(), "highlight cache should be invalidated");
    }

    #[test]
    fn reload_reruns_active_search() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        // search for a term present in `after` but the matches may differ
        app.handle_key(key(KeyCode::Char('/')));
        for c in "bar".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Enter));
        assert!(app.search.active);

        app.reload_review(&after);
        assert!(app.search.active, "search should stay active after reload");
        // "bar" appears once in `after` (the +bar line)
        assert_eq!(app.search.matches.len(), 1, "search re-run on new content");
    }

    #[test]
    fn reload_keeps_old_review_on_parse_failure() {
        let (before, _after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        let files_before = app.review.file_count();

        // empty text → reload reports empty without dropping the review
        app.reload_review("");
        assert_eq!(app.review.file_count(), files_before, "review kept on empty reload");
        assert!(app.status.contains("empty"));

        // garbage that fails to parse
        app.reload_review("not a diff at all");
        assert_eq!(app.review.file_count(), files_before, "review kept on parse failure");
        assert!(app.status.contains("reload failed"));
    }

    #[test]
    fn reload_falls_back_to_first_file_when_path_gone() {
        let (before, _) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        app.handle_key(key(KeyCode::Tab)); // select b.rs
        assert_eq!(app.current_path(), "b.rs");

        // after reload, only a.rs remains
        let after = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
";
        app.reload_review(after);
        assert_eq!(app.review.file_count(), 1);
        assert_eq!(app.selected_file, 0, "clamped to 0 when old path is gone");
        assert_eq!(app.current_path(), "a.rs");
    }
}
