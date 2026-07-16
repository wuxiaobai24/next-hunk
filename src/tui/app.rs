//! Pure review-application state — no I/O, fully unit-testable.
//!
//! `App` owns scroll/focus/search/highlight state over an already-parsed
//! [`Review`] and mutates it in response to `KeyEvent`s via [`App::handle_key`].
//! Rendering is a pure function of `&App` (see [`crate::tui::view`]). Nothing
//! here touches the terminal, so the whole interaction model can be exercised
//! headlessly by feeding scripted `KeyEvent`s and asserting on `App` fields or
//! a rendered `TestBackend` buffer.

use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::config::LayoutMode;
use crate::highlight::{HighlightCache, HighlightJob, Highlighter};
use crate::ir::{Review, Viewport, ViewportQuery};
use crate::tui::theme::{Theme, ThemeMode};

/// Format a [`FocusTarget`] for status messages (compact, human-readable).
fn focus_display(target: &FocusTarget) -> String {
    match target {
        FocusTarget::File(p) => p.clone(),
        FocusTarget::FileLine(p, l) => format!("{p}:{l}"),
        FocusTarget::FileHunk(p, h) => format!("{p}:h{h}"),
    }
}

/// Remove the trailing whitespace-separated word from `s`, plus any whitespace
/// immediately before it. Mirrors the readline backward-kill-word gesture
/// (Ctrl-W) used by shells and most TUI inputs. Operates on `char`s so it is
/// correct for non-ASCII paths/queries.
fn drop_last_word(s: &mut String) {
    // Operate on a char vec so slicing is O(1) and UTF-8-safe. Walk back from
    // the end: drop trailing whitespace, drop the word, then drop the
    // separating whitespace before it (readline backward-kill-word semantics).
    let mut chars: Vec<char> = s.chars().collect();
    while chars.last().map(|c| c.is_whitespace()).unwrap_or(false) {
        chars.pop();
    }
    while chars.last().map(|c| !c.is_whitespace()).unwrap_or(false) {
        chars.pop();
    }
    while chars.last().map(|c| c.is_whitespace()).unwrap_or(false) {
        chars.pop();
    }
    *s = chars.iter().collect();
}

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
    /// Lazily-filled highlight cache (main-thread only).
    pub cache: HighlightCache,
    /// Loaded highlighter (syntect or no-op). Shared with the highlight worker.
    pub highlighter: Arc<Highlighter>,
    /// Optional job channel into the background highlight worker. `None` in
    /// headless tests — draw falls back to synchronous `get_or_highlight`.
    pub hl_job_tx: Option<Sender<HighlightJob>>,

    /// Show a line-number gutter column. ON by default.
    pub line_numbers_on: bool,
    /// Wrap long lines in the stream pane. OFF by default (truncate).
    pub wrap_on: bool,
    /// Highlight changed words within a line (word-diff). ON by default.
    pub word_diff_on: bool,

    /// Show the left file-rail sidebar (toggle with `b`).
    pub show_rail: bool,
    /// Last drawn rail area (None when the rail is hidden). Set by draw_main.
    pub rail_rect: Option<ratatui::layout::Rect>,
    /// Last drawn stream area. Set by draw_main.
    pub stream_rect: Option<ratatui::layout::Rect>,

    /// Show the full-screen keybinding help overlay (toggle with `?`).
    pub show_help: bool,

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

    /// `--focus`: where to scroll on startup. Set by the run loop before the
    /// first draw and consumed (cleared) by [`App::apply_focus`].
    pub focus_target: Option<FocusTarget>,
    /// `--note`: agent annotations. Indexed by the view during the viewport
    /// fan-out to render note rows below their target.
    pub notes: Vec<Note>,
    /// `--select`: when true, `a`/`r`/`?` set per-hunk decisions and the run
    /// loop emits [`Selections`] JSON on quit.
    pub select_mode: bool,
    /// `--select`: per-hunk decisions keyed by [`HunkId`].
    pub decisions: std::collections::HashMap<HunkId, Decision>,
    /// Set of file indices whose bodies are folded (collapsed).
    pub folded: HashSet<usize>,
    /// Layout mode for the diff stream pane.
    pub layout_mode: LayoutMode,
    /// Agent comments (separate from --note annotations).
    pub comments: Vec<CommentEntry>,
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

/// `--focus` target: where the TUI should scroll to on startup. Parsed from the
/// CLI spec `path` / `path:line` / `path:h<n>` and resolved to an absolute
/// stream row by [`App::apply_focus`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FocusTarget {
    /// Scroll to the first hunk of this file.
    File(String),
    /// Scroll to the code line with this new-side source line number.
    FileLine(String, u32),
    /// Scroll to the `n`-th hunk (1-based) within this file.
    FileHunk(String, usize),
}

/// `--note` target: where an agent annotation attaches.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NoteTarget {
    /// Show under the code line with this new-side source line number.
    Line { path: String, line: u32 },
    /// Show under the `n`-th (1-based) hunk header of this file.
    Hunk { path: String, hunk: usize },
    /// Show as a transient banner in the status bar.
    Banner,
}

/// One agent annotation (`--note path:line=text`). Kept on `App` so the view
/// can render note rows during the viewport fan-out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Note {
    pub target: NoteTarget,
    pub text: String,
}

/// A comment entry in the session. Defined here (not in server.rs) so it's
/// available without the `serve` feature gate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommentEntry {
    pub id: String,
    pub file: String,
    pub text: String,
    pub line: Option<u32>,
    pub hunk: Option<usize>,
}

/// Stable identity of a hunk: file index + hunk index within that file. Used as
/// the key for `--select` decisions. Serialized as `"{display_path}:h{n}"`
/// (1-based hunk ordinal) in the output JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HunkId {
    pub file_idx: usize,
    /// 0-based index within the file's `hunks` vec.
    pub hunk_idx: usize,
}

/// A per-hunk review decision set by the human in `--select` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decision {
    /// Not yet reviewed (the default).
    #[default]
    Undecided,
    Accept,
    Reject,
}

/// `--select` output: the human's per-hunk decisions, grouped for the agent to
/// consume from stdout. Hunk keys are `"{display_path}:h{n}"` (1-based).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Selections {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
    pub undecided: Vec<String>,
}

/// Full review report emitted on quit when `export_on_quit` is enabled.
///
/// Compatible extension of [`Selections`]: the three decision arrays keep the
/// same names/shape as `--select` quit / `next-hunk decision`. Additional
/// fields:
/// - `comments` — same shape as serve `comment list` ([`CommentEntry`]), plus
///   synthetic `note-*` entries for non-banner `--note` annotations
/// - `banner` — joined banner-note text, if any
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewReport {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
    pub undecided: Vec<String>,
    #[serde(default)]
    pub comments: Vec<CommentEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
}

impl ReviewReport {
    /// Project down to the legacy [`Selections`] shape (decision arrays only).
    pub fn as_selections(&self) -> Selections {
        Selections {
            accepted: self.accepted.clone(),
            rejected: self.rejected.clone(),
            undecided: self.undecided.clone(),
        }
    }
}

impl App {
    pub fn new(review: Review) -> Self {
        Self::with_highlighter(
            review,
            Highlighter::load("base16-ocean.dark").unwrap_or_else(|_| Highlighter::load_noop()),
        )
    }

    /// Construct with an explicit highlighter (used by `run_review_tui` to
    /// reuse a single loaded `Highlighter` and by tests). Defaults to the light
    /// (Flexoki paper) theme. Use [`App::with_theme`] to inject a config-driven
    /// theme.
    pub fn with_highlighter(review: Review, highlighter: Highlighter) -> Self {
        Self::with_theme(review, Arc::new(highlighter), ThemeMode::default())
    }

    /// Construct with an explicit highlighter and theme mode (used by
    /// `run_review_tui` to honor `config.toml`'s `theme`).
    pub fn with_theme(
        review: Review,
        highlighter: Arc<Highlighter>,
        theme_mode: ThemeMode,
    ) -> Self {
        let status = if review.is_empty() {
            "empty diff".to_string()
        } else {
            format!("{} file(s) — j/k scroll · ]h/[h hunk · zc/zo fold · / search · f filter · H highlight · q quit", review.file_count())
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
            hl_job_tx: None,
            line_numbers_on: true,
            wrap_on: false,
            word_diff_on: true,
            show_rail: true,
            rail_rect: None,
            stream_rect: None,
            show_help: false,
            theme_mode,
            theme,
            mode: InputMode::Normal,
            search: Search::default(),
            path_filter: String::new(),
            pending_prefix: None,
            open_request: None,
            focus_target: None,
            notes: Vec::new(),
            select_mode: false,
            decisions: std::collections::HashMap::new(),
            comments: Vec::new(),
            folded: HashSet::new(),
            layout_mode: LayoutMode::Unified,
        }
    }

    /// Maximum valid top-row so the last row remains visible.
    pub fn max_scroll(&self) -> usize {
        self.review.stream_len.saturating_sub(self.viewport_height)
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
            self.scroll_y.saturating_sub((-delta) as usize)
        };
        self.scroll_y = next;
        self.sync_selected_file();
    }

    /// Consume `focus_target`: resolve it to an absolute stream row and move the
    /// viewport there. Called once by the run loop before the first draw. On an
    /// unknown path/line/hunk the focus silently falls back to the top with a
    /// status hint (the review still opens normally).
    pub fn apply_focus(&mut self) {
        let Some(target) = self.focus_target.take() else {
            return;
        };
        let row = match &target {
            FocusTarget::File(path) => {
                match ViewportQuery::file_index_for_path(&self.review, path) {
                    Some(idx) => Some(ViewportQuery::file_start_row(&self.review, idx)),
                    None => None,
                }
            }
            FocusTarget::FileLine(path, line) => {
                ViewportQuery::file_index_for_path(&self.review, path)
                    .and_then(|idx| ViewportQuery::row_for_new_line(&self.review, idx, *line))
            }
            FocusTarget::FileHunk(path, hunk) => {
                // CLI hunk ordinals are 1-based; HunkId/storage is 0-based.
                let hunk0 = hunk.saturating_sub(1);
                ViewportQuery::file_index_for_path(&self.review, path)
                    .and_then(|idx| ViewportQuery::hunk_start_row(&self.review, idx, hunk0))
            }
        };
        match row {
            Some(row) => {
                self.scroll_y = row.min(self.max_scroll());
                self.sync_selected_file();
                self.status = format!("📍 focus: {}", focus_display(&target));
            }
            None => {
                self.status = format!("focus not found: {}", focus_display(&target));
            }
        }
    }

    /// The [`HunkId`] of the first hunk header within the current viewport, if
    /// any. Used by `--select` keys to decide which hunk `a`/`r`/`?` act on.
    fn current_hunk_id(&self) -> Option<HunkId> {
        let viewport = Viewport {
            start: self.scroll_y,
            height: self.viewport_height.max(1),
        };
        ViewportQuery::rows(&self.review, viewport, &self.folded)
            .into_iter()
            .find_map(|row| match row {
                crate::ir::StreamRow::HunkHeader {
                    file_idx, hunk_idx, ..
                } => Some(HunkId { file_idx, hunk_idx }),
                _ => None,
            })
    }

    /// Record a decision for the current viewport's first hunk, then advance to
    /// the next hunk so the human can keep reviewing. No-op (besides status)
    /// when there is no hunk in view.
    fn decide_current(&mut self, decision: Decision) {
        if let Some(id) = self.current_hunk_id() {
            self.decisions.insert(id, decision);
            self.jump_hunk(true);
            self.status = format!(
                "{:?} — {}",
                decision,
                self.review
                    .files
                    .get(id.file_idx)
                    .map(|f| f.display_path.as_str())
                    .unwrap_or("?")
            );
        } else {
            self.status = "no hunk in view".into();
        }
    }

    /// Build the `--select` output from the current decision map. Every hunk in
    /// the review appears in exactly one bucket; unreviewed hunks are
    /// `undecided`. Hunk keys are `"{display_path}:h{n}"` (1-based ordinal).
    /// Pure function — safe to unit-test headlessly.
    pub fn selections(&self) -> Selections {
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        let mut undecided = Vec::new();
        for (file_idx, file) in self.review.files.iter().enumerate() {
            for hunk_idx in 0..file.hunks.len() {
                let key = format!("{}:h{}", file.display_path, hunk_idx + 1);
                match self
                    .decisions
                    .get(&HunkId { file_idx, hunk_idx })
                    .copied()
                    .unwrap_or_default()
                {
                    Decision::Accept => accepted.push(key),
                    Decision::Reject => rejected.push(key),
                    Decision::Undecided => undecided.push(key),
                }
            }
        }
        Selections {
            accepted,
            rejected,
            undecided,
        }
    }

    /// Build the full quit-time [`ReviewReport`]: decisions + session comments
    /// + note-derived comments + banner. Pure — safe to unit-test headlessly.
    pub fn review_report(&self) -> ReviewReport {
        crate::tui::export::build_report(&self.selections(), &self.comments, &self.notes)
    }

    /// Handle a single key event. Pure: mutates state only, no I/O.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ctrl+C always quits, regardless of mode.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // When the help overlay is up, every key is intercepted: `?`, Esc, q,
        // Enter, or Space dismiss it; anything else is swallowed so the user
        // can't navigate behind the overlay.
        if self.show_help {
            match key.code {
                KeyCode::Char('?')
                | KeyCode::Esc
                | KeyCode::Char('q')
                | KeyCode::Enter
                | KeyCode::Char(' ') => {
                    self.show_help = false;
                }
                _ => {}
            }
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
    /// Wheel scroll is handled (one row per notch; Shift widens it to a
    /// half-page, mirroring `j`/`J`). Left-clicks on the file rail select the
    /// clicked file and scroll to its start; left-clicks on the stream position
    /// the viewport so the clicked row is on top. Other clicks/drags/moves are
    /// ignored. Keeping this in `App` means mouse behavior is exercisable
    /// headlessly, the same as keys.
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
            MouseEventKind::Down(MouseButton::Left) => {
                // Click in the file rail → select that file and scroll to its start.
                if self.show_rail {
                    if let Some(r) = self.rail_rect {
                        if Self::point_in_rect(ev.column, ev.row, r) {
                            let visible = self.visible_files();
                            let idx_in_visible = (ev.row.saturating_sub(r.y)) as usize;
                            if let Some(&fidx) = visible.get(idx_in_visible) {
                                let row =
                                    crate::ir::ViewportQuery::file_start_row(&self.review, fidx)
                                        .min(self.max_scroll());
                                self.selected_file = fidx;
                                self.scroll_y = row;
                                self.status = format!("→ {}", self.review.display_path(fidx));
                            }
                            return;
                        }
                    }
                }
                // Click in the stream → position the viewport so the clicked row is on top.
                if let Some(r) = self.stream_rect {
                    if Self::point_in_rect(ev.column, ev.row, r) {
                        let off = (ev.row.saturating_sub(r.y)) as usize;
                        let target = self.scroll_y.saturating_add(off).min(self.max_scroll());
                        self.scroll_y = target;
                        self.sync_selected_file();
                    }
                }
            }
            // Other clicks, drags, and horizontal scroll are ignored for now.
            _ => {}
        }
    }

    /// Returns `true` when the point `(x, y)` lies within the rectangle `r`
    /// (inclusive of the left/top edge, exclusive of the right/bottom edge).
    fn point_in_rect(x: u16, y: u16, r: ratatui::layout::Rect) -> bool {
        x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        let half = self.viewport_height.max(1) / 2;
        let full = self.viewport_height.max(1);

        // Vim/less-style page navigation on Ctrl-modified keys. Checked
        // before the `match key.code` below so the modifier is honored.
        // (Ctrl+C is handled earlier in `handle_key`.)
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => {
                    self.scroll_by(half as i64);
                    return;
                }
                KeyCode::Char('u') => {
                    self.scroll_by(-(half as i64));
                    return;
                }
                KeyCode::Char('f') => {
                    self.scroll_by(full as i64);
                    return;
                }
                KeyCode::Char('b') => {
                    self.scroll_by(-(full as i64));
                    return;
                }
                _ => {}
            }
        }

        // Two-key sequence handling for `]h` / `[h` (next/prev hunk)
        // and `zc` / `zo` (fold/unfold current file).
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
                ('z', KeyCode::Char('c')) => {
                    self.fold_current();
                    return;
                }
                ('z', KeyCode::Char('o')) => {
                    self.unfold_current();
                    return;
                }
                _ => {
                    // Unrecognized second key: fall through to normal dispatch.
                    // (A lone `]`/`[`/`z` that isn't followed by a valid second key is discarded.)
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
                self.sync_selected_file();
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
            // Number keys 1-9 jump directly to the Nth file (1-based). A
            // muscle-memory shortcut for large multi-file diffs, where
            // Tab-cycling to a far-down file is tedious. Falls through
            // (no-op) when the index is out of range.
            KeyCode::Char(c @ ('1'..='9')) => {
                let n = (c as u8 - b'0') as usize;
                if n <= self.review.file_count() {
                    let idx = n - 1;
                    self.selected_file = idx;
                    self.scroll_y = ViewportQuery::file_start_row(&self.review, idx);
                    self.status = format!("→ {}", self.review.display_path(idx));
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
            // fold/unfold prefixes: `z` awaits `c` (close) or `o` (open).
            KeyCode::Char('z') => {
                self.pending_prefix = Some('z');
                self.status = "z".into();
            }
            // space: jump to the next hunk (wraps across files). A fast
            // single-key alternative to the `]h` two-key sequence.
            KeyCode::Char(' ') => {
                self.jump_hunk(true);
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
            // toggle the file-rail sidebar
            KeyCode::Char('b') => {
                self.show_rail = !self.show_rail;
                self.status = if self.show_rail {
                    "rail shown".into()
                } else {
                    "rail hidden".into()
                };
            }
            // cycle theme: dark → light → auto → dark; reload syntect palette
            KeyCode::Char('t') => {
                self.theme_mode = self.theme_mode.cycle();
                self.theme = self.theme_mode.to_theme();
                // New Arc so in-flight worker jobs keep the old theme gen-safe.
                self.highlighter = Arc::new(
                    Highlighter::load(self.theme_mode.syntect_theme_name())
                        .unwrap_or_else(|_| Highlighter::load_noop()),
                );
                self.cache.invalidate();
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
            KeyCode::Char('o') => match self.compute_open_target() {
                Some(t) => {
                    self.status = format!("opening {}:{}…", t.path, t.line);
                    self.open_request = Some(t);
                }
                None => {
                    self.status = "nothing to open here (move to a code line)".into();
                }
            },
            // next / prev search match
            KeyCode::Char('n') => {
                self.advance_match(true);
            }
            KeyCode::Char('N') => {
                self.advance_match(false);
            }
            // toggle the full-screen keybinding help overlay
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
            }
            // --select mode: accept / reject / mark undecided on the current
            // hunk, then jump to the next. These keys are inert outside select
            // mode (a/r/u fall through to the no-op catch-all).
            KeyCode::Char('a') if self.select_mode => {
                self.decide_current(Decision::Accept);
            }
            KeyCode::Char('r') if self.select_mode => {
                self.decide_current(Decision::Reject);
            }
            KeyCode::Char('u') if self.select_mode => {
                self.decide_current(Decision::Undecided);
            }
            _ => {}
        }
    }

    /// Handle keys while editing the search query.
    fn handle_search_input(&mut self, key: KeyEvent) {
        // Line-editing shortcuts honored before the Char catch-all (which
        // would otherwise swallow Ctrl-U/Ctrl-W as literal chars).
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') => {
                    self.search.query.clear();
                    self.status = "search: ".into();
                    return;
                }
                KeyCode::Char('w') => {
                    drop_last_word(&mut self.search.query);
                    self.status = format!("search: {}", self.search.query);
                    return;
                }
                _ => {}
            }
        }
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
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') => {
                    self.path_filter.clear();
                    self.status = "filter: ".into();
                    return;
                }
                KeyCode::Char('w') => {
                    drop_last_word(&mut self.path_filter);
                    self.status = format!("filter: {}", self.path_filter);
                    return;
                }
                _ => {}
            }
        }
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
            // Silent no-op previously: the user pressed n/N and saw nothing
            // happen, which reads as a broken keybind. Surface why instead.
            if self.search.active {
                self.status = format!("no matches for {:?}", self.search.query);
            } else {
                self.status = "no search active (press / to search)".into();
            }
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
        self.status = format!(
            "filter: {:?} ({} files)",
            self.path_filter,
            self.visible_files().len()
        );
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

        // Build path→index maps for the new review so we can re-map indices.
        let new_file_idx: std::collections::HashMap<&str, usize> = new_review
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.display_path.as_str(), i))
            .collect();

        // Preserve selected_file by display path.
        let old_path = self.current_path().to_string();
        self.selected_file = new_review
            .files
            .iter()
            .position(|f| f.display_path == old_path)
            .unwrap_or(0);

        // Preserve decisions by re-mapping (file_path, hunk_idx) pairs.
        let old_decisions = std::mem::take(&mut self.decisions);
        self.decisions = old_decisions
            .into_iter()
            .filter_map(|(id, decision)| {
                let file = self.review.files.get(id.file_idx)?;
                let new_fi = *new_file_idx.get(file.display_path.as_str())?;
                let new_file = new_review.files.get(new_fi)?;
                if id.hunk_idx < new_file.hunks.len() {
                    Some((
                        HunkId {
                            file_idx: new_fi,
                            hunk_idx: id.hunk_idx,
                        },
                        decision,
                    ))
                } else {
                    None // hunk no longer exists
                }
            })
            .collect();

        // Preserve folded by re-mapping file indices.
        let old_folded: Vec<usize> = self.folded.iter().copied().collect();
        self.folded.clear();
        for fi in &old_folded {
            if let Some(file) = self.review.files.get(*fi) {
                if let Some(&new_fi) = new_file_idx.get(file.display_path.as_str()) {
                    self.folded.insert(new_fi);
                }
            }
        }

        // Preserve focus_target if set (re-apply after review swap).
        let had_focus = self.focus_target.is_some();

        // New base; re-apply the ignore-ws view if it's active so the toggled
        // state survives a hot-reload.
        self.base_review = new_review;
        self.review = if self.ignore_ws {
            crate::ir::strip_whitespace_changes(&self.base_review)
        } else {
            self.base_review.clone()
        };
        self.cache.invalidate();

        // Re-apply focus target to the new review.
        if had_focus {
            self.apply_focus();
        }

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

    /// Fold (collapse) the currently selected file so only its header is visible.
    fn fold_current(&mut self) {
        if self.folded.insert(self.selected_file) {
            self.status = format!("▼ {} (folded)", self.current_path());
            // Clamp scroll in case the fold leaves the viewport past the end.
            self.scroll_y = self.scroll_y.min(self.max_scroll());
            self.sync_selected_file();
        } else {
            self.status = format!("already folded: {}", self.current_path());
        }
    }

    /// Unfold (expand) the currently selected file, revealing its body.
    fn unfold_current(&mut self) {
        if self.folded.remove(&self.selected_file) {
            self.status = format!("▶ {} (unfolded)", self.current_path());
        } else {
            self.status = format!("not folded: {}", self.current_path());
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
        let end = self
            .review
            .stream_len
            .min(start + self.viewport_height.max(1));
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

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn drop_last_word_removes_trailing_word_and_ws() {
        let mut s = String::from("foo bar baz");
        drop_last_word(&mut s);
        assert_eq!(s, "foo bar");
    }

    #[test]
    fn drop_last_word_clears_all_when_only_ws_and_word() {
        let mut s = String::from("   onlyword");
        drop_last_word(&mut s);
        assert_eq!(s, "");
    }

    #[test]
    fn drop_last_word_handles_trailing_whitespace() {
        // backward-kill-word: trailing ws + the word + its preceding ws all go.
        let mut s = String::from("foo bar   ");
        drop_last_word(&mut s);
        assert_eq!(s, "foo");
    }

    #[test]
    fn drop_last_word_on_empty_is_noop() {
        let mut s = String::new();
        drop_last_word(&mut s);
        assert!(s.is_empty());
    }

    #[test]
    fn search_ctrl_u_clears_query() {
        let mut app = two_file_app();
        app.handle_key(char_key('/'));
        for c in "hello world".chars() {
            app.handle_key(char_key(c));
        }
        assert_eq!(app.search.query, "hello world");
        app.handle_key(ctrl('u'));
        assert!(app.search.query.is_empty());
    }

    #[test]
    fn search_ctrl_w_drops_last_word() {
        let mut app = two_file_app();
        app.handle_key(char_key('/'));
        for c in "foo bar baz".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(ctrl('w'));
        assert_eq!(app.search.query, "foo bar");
    }

    #[test]
    fn filter_ctrl_u_clears_filter() {
        let mut app = two_file_app();
        app.handle_key(char_key('f'));
        for c in "src/mod".chars() {
            app.handle_key(char_key(c));
        }
        assert_eq!(app.path_filter, "src/mod");
        app.handle_key(ctrl('u'));
        assert!(app.path_filter.is_empty());
    }

    #[test]
    fn filter_ctrl_w_drops_last_word() {
        let mut app = two_file_app();
        app.handle_key(char_key('f'));
        for c in "src lib/parse".chars() {
            app.handle_key(char_key(c));
        }
        // backward-kill-word drops "lib/parse" and the space before it.
        app.handle_key(ctrl('w'));
        assert_eq!(app.path_filter, "src");
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

    #[test]
    fn page_up_down_syncs_selected_file() {
        let mut app = two_file_app();
        // Stream: a.rs (rows 0-3), b.rs (rows 4-7). viewport_height=4, half=2.
        // max_scroll = 8 - 4 = 4.
        // Two PageDowns from 0 → 4 (file 1, b.rs).
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.scroll_y, 2);
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.scroll_y, 4);
        assert_eq!(app.selected_file, 1, "PageDown should sync to file 1");
        // Two PageUps from 4 → 0 (file 0, a.rs).
        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.scroll_y, 2);
        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.scroll_y, 0);
        assert_eq!(app.selected_file, 0, "PageUp should sync back to file 0");
    }

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
    fn question_toggles_help_overlay() {
        let mut app = two_file_app();
        assert!(!app.show_help, "help hidden by default");
        app.handle_key(char_key('?'));
        assert!(app.show_help);
        // While the overlay is up, navigation keys are swallowed (do not move
        // the viewport or quit).
        let scroll_before = app.scroll_y;
        app.handle_key(char_key('j'));
        assert_eq!(app.scroll_y, scroll_before, "j swallowed behind help");
        assert!(!app.should_quit, "no keys quit behind help except Ctrl+C");
        // `?` / Esc / q / Enter / Space all dismiss it.
        app.handle_key(char_key('q'));
        assert!(!app.show_help, "q dismisses help (does not quit)");
        assert!(!app.should_quit);
    }

    #[test]
    fn b_toggles_show_rail() {
        let mut app = two_file_app();
        assert!(app.show_rail, "rail shown by default");
        app.handle_key(char_key('b'));
        assert!(!app.show_rail);
        assert!(app.status.contains("rail hidden"));
        app.handle_key(char_key('b'));
        assert!(app.show_rail);
        assert!(app.status.contains("rail shown"));
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
        assert!(
            app.scroll_y <= app.max_scroll(),
            "scroll {} > max {}",
            app.scroll_y,
            app.max_scroll()
        );
    }

    #[test]
    fn reload_invalidates_highlight_cache() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        // populate the cache by drawing a line
        let _ = ViewportQuery::rows(
            &app.review,
            Viewport {
                start: 0,
                height: 2,
            },
            &app.folded,
        );
        app.cache
            .get_or_highlight(0, 1, "a.rs", "old", &app.highlighter);
        assert!(!app.cache.is_empty());

        app.reload_review(&after);
        assert!(
            app.cache.is_empty(),
            "highlight cache should be invalidated"
        );
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
        assert_eq!(
            app.review.file_count(),
            files_before,
            "review kept on empty reload"
        );
        assert!(app.status.contains("empty"));

        // garbage that fails to parse
        app.reload_review("not a diff at all");
        assert_eq!(
            app.review.file_count(),
            files_before,
            "review kept on parse failure"
        );
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

    #[test]
    fn reload_preserves_decisions_by_path() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        // Set a decision on a.rs hunk0 (file_idx=0, hunk_idx=0).
        app.decisions.insert(
            HunkId {
                file_idx: 0,
                hunk_idx: 0,
            },
            Decision::Accept,
        );
        // Set a decision on b.rs hunk0 (file_idx=1, hunk_idx=0).
        app.decisions.insert(
            HunkId {
                file_idx: 1,
                hunk_idx: 0,
            },
            Decision::Reject,
        );
        assert_eq!(app.decisions.len(), 2);

        app.reload_review(&after);
        // Both files still exist with the same hunk count, so both decisions
        // should be preserved (re-mapped by path).
        assert_eq!(
            app.decisions.len(),
            2,
            "both decisions should survive reload"
        );
        // Verify by path in selections output.
        let s = app.selections();
        assert!(s.accepted.contains(&"a.rs:h1".to_string()));
        assert!(s.rejected.contains(&"b.rs:h1".to_string()));
    }

    #[test]
    fn reload_drops_decisions_for_removed_hunks() {
        let (before, _after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        // Set a decision on a hunk index that won't exist after reload.
        // before has 1 hunk per file; after also has 1 per file, so this test
        // uses a different pair where b.rs shrinks.
        let before2 = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
- old1
+ new1
@@ -5,2 +5,2 @@
- old2
+ new2
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,1 +1,1 @@
- foo
+ bar
@@ -3,1 +3,1 @@
- baz
+ qux
";
        let after2 = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
- old1
+ new1
@@ -5,2 +5,2 @@
- old2
+ new2
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,1 +1,1 @@
- foo
+ bar
";
        let review2 = parse_unified_diff(before2).unwrap();
        let mut app2 = App::with_highlighter(review2, highlighter());
        app2.viewport_height = 1;
        // a.rs hunk0 → accept, a.rs hunk1 → reject, b.rs hunk1 (index 1) → accept
        app2.decisions.insert(
            HunkId {
                file_idx: 0,
                hunk_idx: 0,
            },
            Decision::Accept,
        );
        app2.decisions.insert(
            HunkId {
                file_idx: 0,
                hunk_idx: 1,
            },
            Decision::Reject,
        );
        app2.decisions.insert(
            HunkId {
                file_idx: 1,
                hunk_idx: 1,
            },
            Decision::Accept,
        );

        app2.reload_review(after2);
        // b.rs hunk1 (index 1) is gone in after2 → that decision should be dropped.
        // a.rs still has 2 hunks → both decisions survive.
        let s = app2.selections();
        assert!(s.accepted.contains(&"a.rs:h1".to_string()));
        assert!(s.rejected.contains(&"a.rs:h2".to_string()));
        // b.rs:h2 was removed → should not appear in any bucket
        assert!(!s.accepted.contains(&"b.rs:h2".to_string()));
        assert!(!s.undecided.contains(&"b.rs:h2".to_string()));
    }

    #[test]
    fn reload_preserves_folded_by_path() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        // Fold b.rs (file_idx=1).
        app.folded.insert(1);
        assert!(app.folded.contains(&1));

        app.reload_review(&after);
        // b.rs still exists at file_idx=1, so fold should be preserved.
        assert!(
            app.folded.contains(&1),
            "fold for b.rs should survive reload"
        );
    }

    #[test]
    fn reload_preserves_notes() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 1;
        app.notes.push(Note {
            target: NoteTarget::Line {
                path: "a.rs".into(),
                line: 1,
            },
            text: "check this line".into(),
        });
        assert_eq!(app.notes.len(), 1);

        app.reload_review(&after);
        assert_eq!(app.notes.len(), 1, "notes should survive reload unchanged");
    }

    #[test]
    fn reload_preserves_focus_target() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 4;
        // Set focus_target but don't consume it yet.
        app.focus_target = Some(FocusTarget::File("b.rs".into()));

        app.reload_review(&after);
        // Focus should still be set (re-applied to new review).
        // apply_focus should scroll to b.rs start.
        // after: a.rs rows 0-3, b.rs rows 4-8. viewport_height=4 → max_scroll=4.
        let b_start = ViewportQuery::file_start_row(&app.review, 1);
        assert_eq!(b_start, 4, "b.rs should start at row 4");
        assert_eq!(app.scroll_y, 4, "focus should be re-applied after reload");
    }

    // ---- --focus: apply_focus ----

    #[test]
    fn apply_focus_to_file_line() {
        let mut app = multi_hunk_app();
        // file0 hunk0: @@ -1,2 +1,2 @@ → ctx is new line 1, +new1 is new line 2.
        // ctx @ row 2, +new1 @ row 4. Focus on new line 2 → row 4.
        app.focus_target = Some(FocusTarget::FileLine("a.rs".into(), 2));
        app.apply_focus();
        assert_eq!(app.scroll_y, 4);
        assert_eq!(app.selected_file, 0);
        // focus_target is consumed
        assert!(app.focus_target.is_none());
        assert!(app.status.contains("focus"));
    }

    #[test]
    fn apply_focus_to_hunk_ordinal() {
        let mut app = multi_hunk_app();
        // b.rs hunk1 (1-based = "h2") is at row 13.
        app.focus_target = Some(FocusTarget::FileHunk("b.rs".into(), 2));
        app.apply_focus();
        assert_eq!(app.scroll_y, 13);
        assert_eq!(app.selected_file, 1);
    }

    #[test]
    fn apply_focus_to_file_start() {
        let mut app = multi_hunk_app();
        // Focusing b.rs lands on its header row (9).
        app.focus_target = Some(FocusTarget::File("b.rs".into()));
        app.apply_focus();
        assert_eq!(app.scroll_y, 9);
        assert_eq!(app.selected_file, 1);
    }

    #[test]
    fn apply_focus_unknown_path_falls_back_gracefully() {
        let mut app = multi_hunk_app();
        app.focus_target = Some(FocusTarget::File("missing.rs".into()));
        app.apply_focus();
        // Stays at the top; status explains the miss.
        assert_eq!(app.scroll_y, 0);
        assert!(app.status.contains("not found"));
    }

    #[test]
    fn apply_focus_none_is_noop() {
        let mut app = multi_hunk_app();
        app.apply_focus(); // no target set
        assert_eq!(app.scroll_y, 0);
    }

    // ---- --select: decisions + selections() ----

    #[test]
    fn selections_empty_when_no_decisions() {
        let app = multi_hunk_app();
        let s = app.selections();
        // 2 files × 2 hunks = 4 undecided, none accepted/rejected.
        assert!(s.accepted.is_empty());
        assert!(s.rejected.is_empty());
        assert_eq!(s.undecided.len(), 4);
        // 1-based ordinals in the key format.
        assert!(s.undecided.contains(&"a.rs:h1".to_string()));
        assert!(s.undecided.contains(&"b.rs:h2".to_string()));
    }

    #[test]
    fn selections_buckets_by_decision() {
        let mut app = multi_hunk_app();
        app.decisions.insert(
            HunkId {
                file_idx: 0,
                hunk_idx: 0,
            },
            Decision::Accept,
        );
        app.decisions.insert(
            HunkId {
                file_idx: 1,
                hunk_idx: 1,
            },
            Decision::Reject,
        );
        let s = app.selections();
        assert_eq!(s.accepted, vec!["a.rs:h1".to_string()]);
        assert_eq!(s.rejected, vec!["b.rs:h2".to_string()]);
        // The other 2 remain undecided.
        assert_eq!(s.undecided.len(), 2);
    }

    #[test]
    fn current_hunk_id_finds_first_visible_hunk() {
        let mut app = multi_hunk_app();
        // At row 1 the first hunk header is file0 hunk0.
        app.scroll_y = 1;
        app.viewport_height = 4;
        let id = app.current_hunk_id().unwrap();
        assert_eq!(id.file_idx, 0);
        assert_eq!(id.hunk_idx, 0);
    }

    #[test]
    fn current_hunk_id_advances_with_scroll() {
        let mut app = multi_hunk_app();
        // Scroll to row 13 (b.rs hunk1 header).
        app.scroll_y = 13;
        app.viewport_height = 4;
        let id = app.current_hunk_id().unwrap();
        assert_eq!(id.file_idx, 1);
        assert_eq!(id.hunk_idx, 1);
    }

    // ---- --select: key-driven decisions ----

    #[test]
    fn select_accept_marks_current_hunk() {
        let mut app = multi_hunk_app();
        app.select_mode = true;
        app.scroll_y = 1; // a.rs hunk0 header
        app.handle_key(char_key('a'));
        assert_eq!(
            app.decisions.get(&HunkId {
                file_idx: 0,
                hunk_idx: 0
            }),
            Some(&Decision::Accept)
        );
        // And it advances to the next hunk (a.rs hunk1 @ row 5).
        assert_eq!(app.scroll_y, 5);
    }

    #[test]
    fn select_reject_then_accept_accumulates() {
        let mut app = multi_hunk_app();
        app.select_mode = true;
        app.scroll_y = 1; // a.rs hunk0
        app.handle_key(char_key('r')); // reject hunk0 → advance to hunk1 @ 5
        app.handle_key(char_key('a')); // accept hunk1 → advance (wraps or next file)
        let s = app.selections();
        assert_eq!(s.accepted, vec!["a.rs:h2".to_string()]);
        assert_eq!(s.rejected, vec!["a.rs:h1".to_string()]);
    }

    #[test]
    fn select_keys_inert_outside_select_mode() {
        let mut app = multi_hunk_app();
        app.scroll_y = 1; // a.rs hunk0 header
                          // select_mode is false → 'a'/'r'/'u' are no-ops, no decision recorded.
        let before_scroll = app.scroll_y;
        app.handle_key(char_key('a'));
        app.handle_key(char_key('r'));
        app.handle_key(char_key('u'));
        assert!(app.decisions.is_empty());
        assert_eq!(app.scroll_y, before_scroll, "no jump outside select mode");
    }

    #[test]
    fn select_undecided_resets_prior_decision() {
        let mut app = multi_hunk_app();
        app.select_mode = true;
        app.scroll_y = 1;
        app.handle_key(char_key('a')); // accept hunk0
        assert_eq!(
            app.decisions.get(&HunkId {
                file_idx: 0,
                hunk_idx: 0
            }),
            Some(&Decision::Accept)
        );
        // Jump back and mark it undecided — should overwrite to Undecided.
        app.scroll_y = 1;
        app.handle_key(char_key('u'));
        // Undecided is the default, so it's not stored distinctly — but
        // selections() must now bucket it as undecided again.
        let s = app.selections();
        assert!(s.accepted.is_empty());
        assert!(s.undecided.contains(&"a.rs:h1".to_string()));
    }

    #[test]
    fn review_report_includes_comments_and_banner_without_select() {
        // Non-`--select` sessions still export comments + banner notes.
        let mut app = multi_hunk_app();
        app.select_mode = false;
        app.comments.push(CommentEntry {
            id: "c0".into(),
            file: "a.rs".into(),
            text: "human/session comment".into(),
            line: Some(3),
            hunk: None,
        });
        app.notes.push(Note {
            target: NoteTarget::Banner,
            text: "banner summary".into(),
        });
        app.notes.push(Note {
            target: NoteTarget::Line {
                path: "a.rs".into(),
                line: 7,
            },
            text: "agent line note".into(),
        });
        let report = app.review_report();
        assert!(report.accepted.is_empty());
        assert!(
            !report.undecided.is_empty(),
            "all hunks undecided without select"
        );
        assert_eq!(report.banner.as_deref(), Some("banner summary"));
        assert_eq!(report.comments.len(), 2);
        assert_eq!(report.comments[0].id, "c0");
        assert_eq!(report.comments[0].line, Some(3));
        assert_eq!(report.comments[1].id, "note-0");
        assert_eq!(report.comments[1].text, "agent line note");
    }

    // ---- mouse clicks ----

    #[test]
    fn mouse_click_on_file_rail_selects_file() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;

        let mut app = two_file_app();
        app.scroll_y = 0;
        app.rail_rect = Some(Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 10,
        });
        app.stream_rect = Some(Rect {
            x: 20,
            y: 0,
            width: 60,
            height: 10,
        });
        // Click on the 2nd visible file entry in the rail (row 1, col 0).
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        // The 2nd visible file is b.rs (index 1). It starts at row 4.
        assert_eq!(app.selected_file, 1);
        assert_eq!(app.scroll_y, 4);
        assert!(app.status.contains("b.rs"));
    }

    #[test]
    fn mouse_click_in_stream_positions_viewport() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;

        let mut app = two_file_app();
        app.scroll_y = 0;
        app.stream_rect = Some(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        });
        // Click on stream row 3 → scroll_y should become 3.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_y, 3);
    }

    /// A three-file review, for testing the `1-9` jump-to-Nth-file keys.
    fn three_file_app() -> App {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-a
+A
diff --git a/b.rs b/b.rs
--- b/b.rs
+++ b/b.rs
@@ -1 +1 @@
-b
+B
diff --git a/c.rs b/c.rs
--- c/c.rs
+++ c/c.rs
@@ -1 +1 @@
-c
+C
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 6;
        app
    }

    #[test]
    fn number_key_jumps_to_nth_file() {
        let mut app = three_file_app();
        // Press `3` → should select the 3rd file (index 2) and scroll to its start.
        app.handle_key(char_key('3'));
        assert_eq!(app.selected_file, 2);
        assert_eq!(app.current_path(), "c/c.rs");
    }

    #[test]
    fn number_key_out_of_range_is_noop() {
        let mut app = three_file_app();
        let before = (app.selected_file, app.scroll_y);
        // `9` is past the 3-file count: no-op, no crash.
        app.handle_key(char_key('9'));
        assert_eq!(app.selected_file, before.0);
    }

    #[test]
    fn ctrl_d_scrolls_half_page_down() {
        let mut app = three_file_app();
        app.viewport_height = 6; // half = 3
        app.scroll_y = 0;
        let start = app.scroll_y;
        app.handle_key(ctrl('d'));
        assert_eq!(app.scroll_y, start + 3);
    }

    #[test]
    fn ctrl_u_scrolls_half_page_up() {
        let mut app = three_file_app();
        app.viewport_height = 6; // half = 3
        app.scroll_y = 5;
        let start = app.scroll_y;
        app.handle_key(ctrl('u'));
        assert_eq!(app.scroll_y, start.saturating_sub(3));
    }

    #[test]
    fn ctrl_f_scrolls_full_page_down() {
        let mut app = three_file_app();
        app.viewport_height = 4; // full = 4
        app.scroll_y = 0;
        let start = app.scroll_y;
        app.handle_key(ctrl('f'));
        assert_eq!(app.scroll_y, start + 4);
    }

    #[test]
    fn ctrl_b_scrolls_full_page_up() {
        let mut app = three_file_app();
        app.viewport_height = 4; // full = 4
        app.scroll_y = 5;
        let start = app.scroll_y;
        app.handle_key(ctrl('b'));
        assert_eq!(app.scroll_y, start.saturating_sub(4));
    }

    #[test]
    fn ctrl_d_clamps_at_max_scroll() {
        let mut app = three_file_app();
        app.viewport_height = 6;
        let max = app.max_scroll();
        app.scroll_y = max; // already at the bottom
        app.handle_key(ctrl('d'));
        assert_eq!(app.scroll_y, max);
    }

    #[test]
    fn n_with_no_active_search_gives_hint_status() {
        let mut app = three_file_app();
        app.handle_key(char_key('n'));
        // No search was ever started: the user should learn why nothing moved.
        assert!(
            app.status.contains("no search active"),
            "got: {}",
            app.status
        );
    }

    #[test]
    fn n_with_active_search_no_matches_gives_status() {
        let mut app = three_file_app();
        // Start a search for something that doesn't exist, then confirm.
        app.handle_key(char_key('/'));
        for c in "zzzznotpresent".chars() {
            app.handle_key(char_key(c));
        }
        app.handle_key(key(KeyCode::Enter));
        assert!(app.search.active);
        assert!(app.search.matches.is_empty());
        // Now pressing `n` should surface the "no matches" reason, not be silent.
        app.handle_key(char_key('n'));
        assert!(app.status.contains("no matches"), "got: {}", app.status);
    }

    // ---- fold (zc / zo) ----

    #[test]
    fn zc_folds_current_file() {
        let mut app = multi_hunk_app();
        assert!(app.folded.is_empty());
        app.handle_key(char_key('z'));
        app.handle_key(char_key('c'));
        assert!(app.folded.contains(&0), "file 0 should be folded");
        assert!(app.status.contains("folded"));
    }

    #[test]
    fn zo_unfolds_current_file() {
        let mut app = multi_hunk_app();
        app.folded.insert(0);
        app.handle_key(char_key('z'));
        app.handle_key(char_key('o'));
        assert!(!app.folded.contains(&0), "file 0 should be unfolded");
        assert!(app.status.contains("unfolded"));
    }

    #[test]
    fn fold_reduces_visible_rows() {
        let mut app = multi_hunk_app();
        app.viewport_height = 20;
        // file0 has 1 header + 2 hunks × (1 header + 2 lines) = 1 + 2 + 4 = 7 rows
        // file1 has 1 header + 2 hunks × (1 header + 2 lines) = 1 + 2 + 4 = 7 rows
        // total stream_len = 14
        let total = app.review.stream_len;
        let full_rows = ViewportQuery::rows(
            &app.review,
            Viewport {
                start: 0,
                height: total,
            },
            &HashSet::new(),
        );
        assert_eq!(full_rows.len(), total);

        app.folded.insert(0);
        let folded_rows = ViewportQuery::rows(
            &app.review,
            Viewport {
                start: 0,
                height: total,
            },
            &app.folded,
        );
        // file0 folded: 7 → 1 row (just header). Total: 1 + 7 = 8.
        assert_eq!(folded_rows.len(), 1 + 7);
    }

    #[test]
    fn zc_then_zo_restores_full_view() {
        let mut app = multi_hunk_app();
        app.viewport_height = 20;
        // fold file0
        app.handle_key(char_key('z'));
        app.handle_key(char_key('c'));
        // unfold file0
        app.handle_key(char_key('z'));
        app.handle_key(char_key('o'));
        let total = app.review.stream_len;
        let rows = ViewportQuery::rows(
            &app.review,
            Viewport {
                start: 0,
                height: total,
            },
            &app.folded,
        );
        assert_eq!(rows.len(), total, "unfolded should restore full view");
    }

    #[test]
    fn fold_then_unfold_toggle() {
        let mut app = multi_hunk_app();
        app.handle_key(char_key('z'));
        app.handle_key(char_key('c'));
        assert!(app.folded.contains(&0));
        app.handle_key(char_key('z'));
        app.handle_key(char_key('c'));
        // zc on already-folded file is a no-op (status says already folded)
        assert!(app.folded.contains(&0));
        assert!(app.status.contains("already folded"));
    }

    #[test]
    fn initial_status_mentions_fold_keys() {
        let app = two_file_app();
        assert!(
            app.status.contains("zc/zo"),
            "initial status should mention fold keys: {}",
            app.status
        );
    }
}
