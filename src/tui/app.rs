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
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::config::LayoutMode;
use crate::config::DEFAULT_CONTEXT_COLLAPSE;
use crate::highlight::{HighlightCache, HighlightJob, Highlighter};
use crate::ir::{CollapseIndex, Review, Viewport, ViewportQuery};
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
    /// Composing a note anchored to the cursor row (`c`).
    Note,
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

/// Severity of a status-line message, used by the view to color it and by the
/// run loop to pick an auto-expire timeout. Errors pop in red, successes in
/// green, everything else stays in the dim default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastKind {
    /// Neutral navigation/toggle feedback — the dim default (current behavior).
    #[default]
    Info,
    /// A positive confirmation ("highlight on", "reloaded", "opened …").
    Success,
    /// Something went wrong ("no matches", "reload failed", …).
    Error,
}

/// How long (in seconds) a toast of this kind stays on screen before the run
/// loop clears it during idle. Sticky toasts (`set_at == None`) never expire.
impl ToastKind {
    pub fn ttl_secs(self) -> u64 {
        match self {
            ToastKind::Error => 8,
            ToastKind::Info | ToastKind::Success => 4,
        }
    }
}

/// A status-line message with a severity and a timestamp so the run loop can
/// auto-expire it and the view can color it. Replaces a bare `String` so every
/// existing `app.status = "...".into()` keeps working (defaults to `Info`,
/// stamped now); tests that assert `app.status.contains(...)` still work via
/// the inherent [`Toast::contains`] shim.
#[derive(Debug, Clone, Default)]
pub struct Toast {
    /// The message text.
    pub message: String,
    /// Severity — drives color and expire timeout.
    pub kind: ToastKind,
    /// When the toast was set; `None` = sticky (never auto-expire).
    /// Used by [`App::expire_status`] in the run loop.
    pub set_at: Option<Instant>,
}

impl Toast {
    /// A sticky (non-expiring) info toast — used for the initial hint.
    pub fn sticky(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToastKind::Info,
            set_at: None,
        }
    }

    /// True if the message contains `needle`. Delegates to the inner string so
    /// existing `app.status.contains(...)` assertions keep working unchanged.
    pub fn contains(&self, needle: &str) -> bool {
        self.message.contains(needle)
    }

    /// True when the message is empty.
    pub fn is_empty(&self) -> bool {
        self.message.is_empty()
    }

    /// True if this toast has outlived its kind's TTL. Sticky toasts never do.
    pub fn expired(&self) -> bool {
        match self.set_at {
            None => false,
            Some(t) => t.elapsed().as_secs() >= self.kind.ttl_secs(),
        }
    }
}

impl From<&str> for Toast {
    fn from(s: &str) -> Self {
        Self {
            message: s.to_string(),
            kind: ToastKind::Info,
            set_at: Some(Instant::now()),
        }
    }
}

impl From<String> for Toast {
    fn from(s: String) -> Self {
        Self {
            message: s,
            kind: ToastKind::Info,
            set_at: Some(Instant::now()),
        }
    }
}

impl std::fmt::Display for Toast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
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
    /// The review cursor: a virtual row the human moves with `j`/`k` (and
    /// clicks). `c` composes a note on it, `o` opens it in `$EDITOR`. The
    /// viewport follows it (and it clamps into the viewport on pure scrolls).
    pub cursor_v: usize,
    /// Whether the cursor row gets its background highlight
    /// (`cursor_line` config; navigation works either way).
    pub cursor_on: bool,
    /// Currently focused file index (rail selection).
    pub selected_file: usize,
    /// Visible height of the stream pane (set from the drawn area).
    pub viewport_height: usize,
    /// Set when the user requests to quit.
    pub should_quit: bool,
    /// Transient status/help message for the status line. Carries a severity
    /// ([`ToastKind`]) and timestamp so the view can color it and the run loop
    /// can auto-expire it.
    pub status: Toast,

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
    /// The chrome palette family (Flexoki / Catppuccin / …); `T` cycles it.
    pub palette: crate::tui::theme::Palette,
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
    /// `--watch`: live-reload is active. Set by the run loop; surfaced as a
    /// `[WATCH]` badge in the status bar.
    pub watch_mode: bool,
    /// `serve` subcommand: the TUI is driven by an agent socket. Set by the run
    /// loop; surfaced as a `[SERVE]` badge in the status bar.
    pub serve_mode: bool,
    /// `--select`: per-hunk decisions keyed by [`HunkId`].
    pub decisions: std::collections::HashMap<HunkId, Decision>,
    /// Set of file indices whose bodies are folded (collapsed).
    pub folded: HashSet<usize>,
    /// Virtual-row view of the review: folded files, collapsed context runs,
    /// and implied inter-hunk gaps. `scroll_y` is a row in this index, so
    /// every scroll position is exactly one drawn row.
    pub collapse: CollapseIndex,
    /// Whether context collapsing is active (`zx` toggles). When off the
    /// index degrades to a pure 1:1 view (folds still apply).
    pub collapse_on: bool,
    /// Configured threshold (`context_collapse`); runs/gaps shorter than this
    /// never collapse. 0 disables.
    pub context_threshold: usize,
    /// Concrete layout the virtual index was last built for (auto resolves
    /// per draw; a change rebuilds the index).
    pub built_layout: LayoutMode,
    /// Stream-pane width the split index was built for (column-independent,
    /// kept to document the last rebuild).
    pub built_split_width: u16,
    /// Layout mode for the diff stream pane.
    pub layout_mode: LayoutMode,
    /// Agent comments (separate from --note annotations).
    pub comments: Vec<CommentEntry>,
    /// Comment ids already converted into `notes` by `comment apply`.
    /// Prevents duplicate note rows when apply runs more than once.
    pub applied_comments: std::collections::HashSet<String>,
    /// Stream row of the last `}`/`{` note jump, while it is still in view.
    /// Anchors repeated jumps so `}` keeps advancing near the clamped end of
    /// the stream instead of re-finding the same row.
    pub last_note_jump: Option<usize>,
    /// Stream row of the last `]h`/`[h` hunk jump, while it is still in view.
    /// Same purpose as [`App::last_note_jump`]: when the whole stream fits on
    /// one screen the viewport top never moves, so a viewport-top anchor alone
    /// would pin `]h` to the first hunk forever.
    pub last_hunk_jump: Option<usize>,
    /// Draft text while composing a note (`c` … Enter). Kept on `App` like
    /// `search.query` so the prompt line can render it live.
    pub note_draft: String,
    /// Where the composed note will attach — resolved from the cursor row
    /// when `c` was pressed; `None` outside note-composition mode.
    pub note_pending: Option<NoteTarget>,
    /// Counter for human note ids (`user:1`, `user:2`, …) so serve sessions
    /// can list/remove them alongside agent comments.
    pub user_note_seq: usize,
    /// Human notes by their `user:N` id — the bridge back from
    /// `comment rm user:N` to the rendered `Note`.
    pub user_notes: std::collections::HashMap<String, Note>,
    /// Repo root this review was launched from (reported by `Info` so
    /// `list`/`get` can show which checkout a session belongs to).
    pub repo_root: Option<String>,
    /// How this session was launched — `"diff"` / `"show"` / `"serve"`.
    pub session_mode: String,
    /// Short human-readable session title (e.g. `demo working tree`).
    pub session_title: String,
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

/// Resolve a note target to the absolute stream row it attaches to.
/// `None` for banner notes (they live in the status bar) and for targets
/// whose file/line/hunk no longer exists after a reload.
pub fn note_stream_row(review: &Review, target: &NoteTarget) -> Option<usize> {
    match target {
        NoteTarget::Line { path, line } => ViewportQuery::file_index_for_path(review, path)
            .and_then(|idx| ViewportQuery::row_for_new_line(review, idx, *line)),
        NoteTarget::Hunk { path, hunk } => {
            // CLI hunk ordinals are 1-based; storage is 0-based.
            let hunk0 = hunk.saturating_sub(1);
            ViewportQuery::file_index_for_path(review, path)
                .and_then(|idx| ViewportQuery::hunk_start_row(review, idx, hunk0))
        }
        NoteTarget::Banner => None,
    }
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
            Toast::sticky("empty diff")
        } else {
            Toast::sticky(format!("{} file(s) — j/k scroll · ]h/[h hunk · zc/zo fold · zx context · / search · f filter · H highlight · q quit", review.file_count()))
        };
        let theme = theme_mode.to_theme();
        let collapse =
            CollapseIndex::build(&review, DEFAULT_CONTEXT_COLLAPSE, &HashSet::new(), false);
        Self {
            review: review.clone(),
            base_review: review,
            ignore_ws: false,
            scroll_y: 0,
            cursor_v: 0,
            cursor_on: true,
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
            palette: crate::tui::theme::Palette::default(),
            theme,
            mode: InputMode::Normal,
            search: Search::default(),
            path_filter: String::new(),
            pending_prefix: None,
            open_request: None,
            focus_target: None,
            notes: Vec::new(),
            select_mode: false,
            watch_mode: false,
            serve_mode: false,
            decisions: std::collections::HashMap::new(),
            comments: Vec::new(),
            applied_comments: std::collections::HashSet::new(),
            last_note_jump: None,
            last_hunk_jump: None,
            note_draft: String::new(),
            note_pending: None,
            user_note_seq: 0,
            user_notes: std::collections::HashMap::new(),
            repo_root: None,
            session_mode: String::new(),
            session_title: String::new(),
            folded: HashSet::new(),
            collapse,
            collapse_on: true,
            context_threshold: DEFAULT_CONTEXT_COLLAPSE,
            built_layout: LayoutMode::Unified,
            built_split_width: 80,
            layout_mode: LayoutMode::Unified,
        }
    }

    /// Set an info toast (neutral feedback), stamped now.
    pub fn set_info(&mut self, message: impl Into<String>) {
        self.status = Toast {
            message: message.into(),
            kind: ToastKind::Info,
            set_at: Some(Instant::now()),
        };
    }

    /// Set a success toast (green), stamped now.
    pub fn set_success(&mut self, message: impl Into<String>) {
        self.status = Toast {
            message: message.into(),
            kind: ToastKind::Success,
            set_at: Some(Instant::now()),
        };
    }

    /// Set an error toast (red, longer TTL), stamped now.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.status = Toast {
            message: message.into(),
            kind: ToastKind::Error,
            set_at: Some(Instant::now()),
        };
    }

    /// Clear a stale toast if it has outlived its TTL. Called by the run loop
    /// each idle tick so the status line doesn't keep a transient message (or
    /// a red error) on screen after the user stops interacting. Sticky toasts
    /// (the initial hint, banner notes) are never cleared here.
    pub fn expire_status(&mut self) {
        if self.status.expired() {
            self.status = Toast::default();
        }
    }

    /// Maximum valid top-row so the last row remains visible.
    pub fn max_scroll(&self) -> usize {
        self.collapse
            .virtual_len()
            .saturating_sub(self.viewport_height)
    }

    /// Sync `selected_file` to whatever file owns the current top scroll row.
    fn sync_selected_file(&mut self) {
        let stream_row = self.collapse.stream_at_virtual(self.scroll_y);
        if let Some(idx) = ViewportQuery::file_at_row(&self.review, stream_row) {
            self.selected_file = idx;
        }
    }

    /// Jump so the given *stream* row lands at the viewport top, expanding a
    /// collapsed context run that contains it. Every navigation that targets
    /// a real row (`]h`/`[h`, file jumps, search matches, `--focus`, reload
    /// remaps) funnels through here so virtual coordinates stay coherent.
    /// The rail selection syncs from the *target* row, not the clamped top
    /// row — when the viewport is taller than the stream the scroll clamps
    /// to 0 and a position-based sync would clobber the selection. The
    /// cursor lands on the target row: a jump means "go there".
    fn jump_to_stream(&mut self, row: usize) {
        self.collapse.expand_at_stream(row);
        let v = self.collapse.virtual_of_stream(row);
        self.scroll_y = v.min(self.max_scroll());
        self.cursor_v = v.min(self.collapse.virtual_len().saturating_sub(1));
        if let Some(idx) = ViewportQuery::file_at_row(&self.review, row) {
            self.selected_file = idx;
        }
    }

    /// Rebuild the virtual-row index after anything that changes which rows
    /// exist (review swap, fold/unfold, ignore-ws re-derive, `zx`). Keeps
    /// looking at the same stream row so the view does not jump; the cursor
    /// is re-anchored the same way.
    fn rebuild_collapse(&mut self) {
        let anchor = self.collapse.stream_at_virtual(self.scroll_y);
        let cursor_anchor = self.collapse.stream_at_virtual(
            self.cursor_v
                .min(self.collapse.virtual_len().saturating_sub(1)),
        );
        let threshold = if self.collapse_on {
            self.context_threshold
        } else {
            0
        };
        let split = self.effective_layout() == LayoutMode::Split;
        self.collapse = CollapseIndex::build(&self.review, threshold, &self.folded, split);
        let v = self.collapse.virtual_of_stream(anchor);
        self.scroll_y = v.min(self.max_scroll());
        self.cursor_v = self
            .collapse
            .virtual_of_stream(cursor_anchor)
            .min(self.collapse.virtual_len().saturating_sub(1));
        self.clamp_cursor_to_view();
        self.sync_selected_file();
    }

    /// The concrete layout for the current stream-pane width (`auto`
    /// resolves at draw time; concrete modes pass through).
    pub fn effective_layout(&self) -> LayoutMode {
        let width = self.stream_rect.map(|r| r.width).unwrap_or(80);
        let resolved = self.layout_mode.resolve(width);
        // Narrow terminals cannot fit two columns (or a stack): downgrade to
        // unified so the index and the renderer always agree.
        match resolved {
            LayoutMode::Split if width < 80 => LayoutMode::Unified,
            LayoutMode::Stack if width < 40 => LayoutMode::Unified,
            other => other,
        }
    }

    /// Track the layout the index was built for; when a width change (or a
    /// layout config change) moves `auto` across a threshold, rebuild the
    /// virtual index so pair rows / line rows match what is drawn. Called
    /// from the draw path with the live stream-pane width.
    pub fn sync_effective_layout(&mut self, width: u16) {
        let resolved = self.layout_mode.resolve(width);
        let built_for = self.built_layout;
        let width_changed = resolved == LayoutMode::Split
            && built_for == LayoutMode::Split
            && self.built_split_width != width;
        if resolved != built_for || width_changed {
            self.built_layout = resolved;
            self.built_split_width = width;
            self.rebuild_collapse();
        }
    }

    /// Toggle context collapsing (`zx`), keeping the viewed stream row.
    fn toggle_collapse(&mut self) {
        self.collapse_on = !self.collapse_on;
        self.rebuild_collapse();
        if self.collapse_on {
            self.set_success("context collapse on");
        } else {
            self.set_info("context collapse off");
        }
    }

    /// Apply a configured `context_collapse` threshold (from config.toml or
    /// CLI). Called before the first draw; 0 disables markers entirely.
    pub fn set_context_collapse(&mut self, threshold: usize) {
        self.context_threshold = threshold;
        self.collapse_on = threshold > 0;
        self.rebuild_collapse();
    }

    /// Move the scroll position by `delta` rows, clamped to `[0, max_scroll()]`.
    /// Positive scrolls down, negative up. Used by both keys and mouse wheel so
    /// they share one clamp/sync path. A pure scroll does not move the cursor;
    /// the cursor clamps back into the new viewport (sticking to the edge it
    /// left through).
    fn scroll_by(&mut self, delta: i64) {
        let next = if delta >= 0 {
            self.scroll_y
                .saturating_add(delta as usize)
                .min(self.max_scroll())
        } else {
            self.scroll_y.saturating_sub((-delta) as usize)
        };
        self.scroll_y = next;
        self.clamp_cursor_to_view();
        self.sync_selected_file();
    }

    // ── review cursor ────────────────────────────────────────────────────────

    /// The stream row under the review cursor.
    pub fn cursor_stream_row(&self) -> usize {
        self.collapse.stream_at_virtual(
            self.cursor_v
                .min(self.collapse.virtual_len().saturating_sub(1)),
        )
    }

    /// Keep the cursor inside the viewport after a viewport-only scroll
    /// (mouse wheel, PgUp-style keys routed to `scroll_by`).
    fn clamp_cursor_to_view(&mut self) {
        let last_v = self.collapse.virtual_len().saturating_sub(1);
        if self.cursor_v < self.scroll_y {
            self.cursor_v = self.scroll_y.min(last_v);
        }
        let bottom = self
            .scroll_y
            .saturating_add(self.viewport_height.max(1) - 1)
            .min(last_v);
        if self.cursor_v > bottom {
            self.cursor_v = bottom;
        }
    }

    /// Move the cursor by `delta` virtual rows; the viewport follows only
    /// when the cursor would leave it (so the view stays stable around the
    /// cursor, like a scrolloff of 0).
    fn move_cursor(&mut self, delta: i64) {
        let last_v = self.collapse.virtual_len().saturating_sub(1);
        let next = if delta >= 0 {
            self.cursor_v.saturating_add(delta as usize).min(last_v)
        } else {
            self.cursor_v.saturating_sub((-delta) as usize)
        };
        self.set_cursor(next);
    }

    /// Place the cursor on a virtual row, scrolling the viewport minimally
    /// so the row stays visible, and syncing the rail selection to it.
    pub fn set_cursor(&mut self, v: usize) {
        self.cursor_v = v;
        if self.cursor_v < self.scroll_y {
            self.scroll_y = self.cursor_v;
        } else if self.cursor_v >= self.scroll_y + self.viewport_height.max(1) {
            self.scroll_y = self.cursor_v + 1 - self.viewport_height.max(1);
        }
        self.scroll_y = self.scroll_y.min(self.max_scroll());
        let stream_row = self.cursor_stream_row();
        if let Some(idx) = ViewportQuery::file_at_row(&self.review, stream_row) {
            self.selected_file = idx;
        }
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
                self.jump_to_stream(row);
                self.set_info(format!("📍 focus: {}", focus_display(&target)));
            }
            None => {
                self.set_error(format!("focus not found: {}", focus_display(&target)));
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
        ViewportQuery::rows_virtual(&self.review, viewport, &self.collapse)
            .into_iter()
            .find_map(|vr| match vr.row {
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
            self.set_info(format!(
                "{:?} — {}",
                decision,
                self.review
                    .files
                    .get(id.file_idx)
                    .map(|f| f.display_path.as_str())
                    .unwrap_or("?")
            ));
        } else {
            self.set_error("no hunk in view");
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
            InputMode::Note => {
                self.handle_note_input(key);
                return;
            }
            InputMode::Normal => {}
        }

        self.handle_normal_key(key);
    }

    /// Handle a single mouse event. Pure: mutates state only, no I/O.
    ///
    /// Wheel scroll is handled (one row per notch; Shift widens it to a
    /// half-page, mirroring the scroll keys). Left-clicks on the file rail
    /// select the clicked file and scroll to its start; left-clicks on the
    /// stream put the review cursor on the clicked row. Other
    /// clicks/drags/moves are ignored. Keeping this in `App` means mouse
    /// behavior is exercisable headlessly, the same as keys.
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
                                self.set_info(format!("→ {}", self.review.display_path(fidx)));
                            }
                            return;
                        }
                    }
                }
                // Click in the stream → put the review cursor on the clicked
                // row (the viewport stays put; only the cursor moves).
                if let Some(r) = self.stream_rect {
                    if Self::point_in_rect(ev.column, ev.row, r) {
                        let off = (ev.row.saturating_sub(r.y)) as usize;
                        let target = self
                            .scroll_y
                            .saturating_add(off)
                            .min(self.collapse.virtual_len().saturating_sub(1));
                        self.set_cursor(target);
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
                    self.move_cursor(half as i64);
                    return;
                }
                KeyCode::Char('u') => {
                    self.move_cursor(-(half as i64));
                    return;
                }
                KeyCode::Char('f') => {
                    self.move_cursor(full as i64);
                    return;
                }
                KeyCode::Char('b') => {
                    self.move_cursor(-(full as i64));
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
                ('z', KeyCode::Char('x')) => {
                    self.toggle_collapse();
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
                    self.set_success("search cleared");
                } else {
                    self.should_quit = true;
                }
            }
            // cursor down one (viewport follows only at the edges)
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor(1);
            }
            // cursor up one
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor(-1);
            }
            // half-page down
            KeyCode::Char('J') | KeyCode::PageDown => {
                self.move_cursor(half as i64);
            }
            // half-page up
            KeyCode::Char('K') | KeyCode::PageUp => {
                self.move_cursor(-(half as i64));
            }
            // top / bottom
            KeyCode::Char('g') | KeyCode::Home => {
                self.set_cursor(0);
                self.scroll_y = 0;
                self.sync_selected_file();
            }
            KeyCode::Char('G') | KeyCode::End => {
                let last_v = self.collapse.virtual_len().saturating_sub(1);
                self.scroll_y = self.max_scroll();
                self.set_cursor(last_v);
            }
            // next / prev file
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                if let Some((idx, row)) =
                    ViewportQuery::jump_file(&self.review, self.selected_file, true)
                {
                    self.selected_file = idx;
                    self.jump_to_stream(row);
                    self.set_info(format!("→ {}", self.review.display_path(idx)));
                }
            }
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
                if let Some((idx, row)) =
                    ViewportQuery::jump_file(&self.review, self.selected_file, false)
                {
                    self.selected_file = idx;
                    self.jump_to_stream(row);
                    self.set_info(format!("← {}", self.review.display_path(idx)));
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
                    let row = ViewportQuery::file_start_row(&self.review, idx);
                    self.jump_to_stream(row);
                    self.set_info(format!("→ {}", self.review.display_path(idx)));
                }
            }
            // hunk navigation prefixes: `]` / `[` await a following `h`
            KeyCode::Char(']') => {
                self.pending_prefix = Some(']');
                self.set_info("]");
            }
            KeyCode::Char('[') => {
                self.pending_prefix = Some('[');
                self.set_info("[");
            }
            // fold/unfold prefixes: `z` awaits `c` (close) or `o` (open).
            KeyCode::Char('z') => {
                self.pending_prefix = Some('z');
                self.set_info("z");
            }
            // space: jump to the next hunk (wraps across files). A fast
            // single-key alternative to the `]h` two-key sequence.
            KeyCode::Char(' ') => {
                self.jump_hunk(true);
            }
            // next / previous annotated row (a line or hunk carrying a note)
            KeyCode::Char('}') => {
                self.jump_note(true);
            }
            KeyCode::Char('{') => {
                self.jump_note(false);
            }
            // toggle highlight
            KeyCode::Char('H') => {
                self.highlight_on = !self.highlight_on;
                if !self.highlight_on {
                    self.cache.invalidate();
                    self.set_info("highlight off");
                } else {
                    self.set_success("highlight on");
                }
            }
            // toggle line-number gutter
            KeyCode::Char('#') => {
                self.line_numbers_on = !self.line_numbers_on;
                if self.line_numbers_on {
                    self.set_success("line numbers on");
                } else {
                    self.set_info("line numbers off");
                }
            }
            // toggle word-level inline diff
            KeyCode::Char('w') => {
                self.word_diff_on = !self.word_diff_on;
                if self.word_diff_on {
                    self.set_success("word diff on");
                } else {
                    self.set_info("word diff off");
                }
            }
            // toggle ignore-whitespace view (collapse whitespace-only changes)
            KeyCode::Char('W') => {
                self.ignore_ws = !self.ignore_ws;
                self.apply_ignore_ws();
                if self.ignore_ws {
                    self.set_success("ignore-whitespace on");
                } else {
                    self.set_info("ignore-whitespace off");
                }
            }
            // toggle the file-rail sidebar
            KeyCode::Char('b') => {
                self.show_rail = !self.show_rail;
                if self.show_rail {
                    self.set_success("rail shown");
                } else {
                    self.set_info("rail hidden");
                }
            }
            // cycle layout: unified → split → stack → unified. Rebuilds the
            // virtual index (pair rows vs line rows) and keeps the anchor.
            KeyCode::Char('L') => {
                self.layout_mode = match self.layout_mode {
                    LayoutMode::Unified => LayoutMode::Split,
                    LayoutMode::Split => LayoutMode::Stack,
                    LayoutMode::Stack | LayoutMode::Auto => LayoutMode::Unified,
                };
                self.built_layout = self.effective_layout();
                self.rebuild_collapse();
                self.set_success(format!("layout: {}", self.layout_mode.as_str()));
            }
            // cycle theme mode: dark → light → auto → dark (within the
            // current palette family); reloads the syntect palette to match
            KeyCode::Char('t') => {
                self.theme_mode = self.theme_mode.cycle();
                self.apply_theme();
                self.set_info(format!(
                    "theme: {} ({})",
                    self.palette.preset_name(self.theme_mode),
                    self.theme_mode.name()
                ));
            }
            // cycle the palette family: flexoki → catppuccin → gruvbox →
            // nord → tokyonight → flexoki; keeps the current mode
            KeyCode::Char('T') => {
                self.palette = self.palette.cycle();
                self.apply_theme();
                self.set_info(format!(
                    "theme: {} ({})",
                    self.palette.preset_name(self.theme_mode),
                    self.theme_mode.name()
                ));
            }
            // begin in-stream search
            KeyCode::Char('/') => {
                self.mode = InputMode::Search;
                self.search.query.clear();
                self.set_info("search: ");
            }
            // begin path filter
            KeyCode::Char('f') => {
                self.mode = InputMode::Filter;
                self.path_filter.clear();
                self.set_info("filter: ");
            }
            // compose a note anchored to the cursor row
            KeyCode::Char('c') => self.begin_note(),
            // open the focused line's file in $EDITOR
            KeyCode::Char('o') => match self.compute_open_target() {
                Some(t) => {
                    self.set_info(format!("opening {}:{}…", t.path, t.line));
                    self.open_request = Some(t);
                }
                None => {
                    self.set_error("nothing to open here (move to a code line)");
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
                    self.set_info("search: ");
                    return;
                }
                KeyCode::Char('w') => {
                    drop_last_word(&mut self.search.query);
                    self.set_info(format!("search: {}", self.search.query));
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
                self.set_info("search cancelled");
            }
            KeyCode::Backspace => {
                self.search.query.pop();
                self.set_info(format!("search: {}", self.search.query));
            }
            KeyCode::Char(c) => {
                self.search.query.push(c);
                self.set_info(format!("search: {}", self.search.query));
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
                    self.set_info("filter: ");
                    return;
                }
                KeyCode::Char('w') => {
                    drop_last_word(&mut self.path_filter);
                    self.set_info(format!("filter: {}", self.path_filter));
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
                self.set_success("filter cleared");
            }
            KeyCode::Backspace => {
                self.path_filter.pop();
                self.set_info(format!("filter: {}", self.path_filter));
            }
            KeyCode::Char(c) => {
                self.path_filter.push(c);
                self.set_info(format!("filter: {}", self.path_filter));
            }
            _ => {}
        }
    }

    /// Run the search: scan the whole stream for the query (case-insensitive).
    fn finalize_search(&mut self) {
        if self.search.query.trim().is_empty() {
            self.search.clear();
            self.set_error("search empty");
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
            self.set_error(format!("no matches for {:?}", self.search.query));
        } else {
            let row = self.search.matches[0];
            self.jump_to_stream(row);
            self.set_info(format!(
                "match {}/{}: {:?}",
                self.search.current + 1,
                self.search.matches.len(),
                self.search.query
            ));
        }
    }

    /// Move to the next/prev search match (wraps).
    fn advance_match(&mut self, forward: bool) {
        if self.search.matches.is_empty() {
            // Silent no-op previously: the user pressed n/N and saw nothing
            // happen, which reads as a broken keybind. Surface why instead.
            if self.search.active {
                self.set_error(format!("no matches for {:?}", self.search.query));
            } else {
                self.set_error("no search active (press / to search)");
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
        self.jump_to_stream(row);
        self.set_info(format!(
            "match {}/{}: {:?}",
            self.search.current + 1,
            self.search.matches.len(),
            self.search.query
        ));
    }

    /// Apply the path filter: clamp selected_file into the visible set.
    fn apply_filter(&mut self) {
        if self.path_filter.trim().is_empty() {
            self.set_success("filter cleared");
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
                let row = ViewportQuery::file_start_row(&self.review, first);
                self.jump_to_stream(row);
            } else {
                self.set_error(format!("no files match {:?}", self.path_filter));
                return;
            }
        }
        self.set_info(format!(
            "filter: {:?} ({} files)",
            self.path_filter,
            self.visible_files().len()
        ));
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
            self.set_info("reloaded (empty diff)");
            return;
        }
        let new_review = match crate::ir::parse_unified_diff(text) {
            Ok(r) => r,
            Err(e) => {
                self.set_error(format!("reload failed: {e}"));
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

        // Clamp scroll into the new bounds. The rebuilt virtual index
        // re-anchors on the stream row we were looking at.
        self.rebuild_collapse();

        // Jump anchors reference stream rows, which change meaning across a
        // review swap — drop them so `]h`/`}` search from the viewport again.
        self.last_hunk_jump = None;
        self.last_note_jump = None;

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

        self.set_success(format!("reloaded ({} files)", self.review.file_count()));
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
            self.jump_to_stream(self.search.matches[0]);
        }
    }

    /// Jump to the next/previous hunk header, scrolling it to the top of the
    /// viewport and syncing the rail selection. Wraps across file boundaries.
    fn jump_hunk(&mut self, forward: bool) {
        // `]h`/`[h` search from the current view position; translate the
        // virtual top row to a stream row before asking the hunk index.
        // When the whole stream fits on one screen `jump_to_stream` clamps
        // `scroll_y` to 0, so the viewport-top anchor never moves and repeated
        // `]h` would re-find the first hunk forever — anchor on the last jump
        // instead while it is still in view (mirroring `jump_note`).
        let scroll_anchor = self.collapse.stream_at_virtual(self.scroll_y);
        let anchor = match self.last_hunk_jump {
            Some(r)
                if {
                    let v = self.collapse.virtual_of_stream(r);
                    v >= self.scroll_y && v < self.scroll_y + self.viewport_height
                } =>
            {
                r
            }
            _ => scroll_anchor,
        };
        match ViewportQuery::jump_hunk(&self.review, anchor, forward) {
            Some(row) => {
                self.last_hunk_jump = Some(row);
                self.jump_to_stream(row);
                // Label with the human-facing hunk ordinal, not the internal
                // stream row (a bare number reads like a line number).
                let label = match ViewportQuery::locate_hunk(&self.review, row) {
                    Some((file_idx, ordinal)) => {
                        format!("{}:h{}", self.review.display_path(file_idx), ordinal)
                    }
                    None => row.to_string(),
                };
                let dir = if forward { "→" } else { "←" };
                self.set_info(format!("{dir} hunk @ {label}"));
            }
            None => {
                self.set_error("no hunks to jump to");
            }
        }
    }

    /// What the human is currently looking at — the file, 1-based hunk
    /// ordinal, and source line under the review cursor. Best-effort
    /// (rows outside any hunk report file only); used by `Info`/`context`
    /// so an agent can sync with the human's viewport.
    pub fn current_focus(&self) -> (Option<String>, Option<usize>, Option<u32>) {
        let cursor_stream = self.collapse.stream_at_virtual(
            self.cursor_v
                .min(self.collapse.virtual_len().saturating_sub(1)),
        );
        let file_idx = match ViewportQuery::file_at_row(&self.review, cursor_stream) {
            Some(idx) => idx,
            None => return (None, None, None),
        };
        let path = self.review.display_path(file_idx).to_string();
        let hunk =
            ViewportQuery::hunk_containing(&self.review, cursor_stream).map(|(_, ordinal)| ordinal);
        let line = ViewportQuery::row_line_numbers(&self.review, cursor_stream)
            .and_then(|(old, new)| new.or(old));
        (Some(path), hunk, line)
    }

    /// Sorted, deduplicated stream rows that carry notes (`--note`
    /// annotations and applied serve comments). These are the `}`/`{` jump
    /// targets and the source of the rail's per-file 💬 badges.
    pub fn annotated_rows(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = self
            .notes
            .iter()
            .filter_map(|n| note_stream_row(&self.review, &n.target))
            .collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    /// Per-file note counts for the rail's 💬 badge. Banner notes and targets
    /// on unknown files are skipped.
    pub fn note_counts_by_file(&self) -> std::collections::HashMap<usize, usize> {
        let mut out = std::collections::HashMap::new();
        for note in &self.notes {
            let idx = match &note.target {
                NoteTarget::Line { path, .. } | NoteTarget::Hunk { path, .. } => {
                    ViewportQuery::file_index_for_path(&self.review, path)
                }
                NoteTarget::Banner => None,
            };
            if let Some(idx) = idx {
                *out.entry(idx).or_insert(0) += 1;
            }
        }
        out
    }

    /// Jump to the next (`}`) / previous (`{`) annotated row — a code line or
    /// hunk header carrying a note. Wraps around (mirroring `]h`): past the
    /// last note the search continues from the top. The anchor is the last
    /// jumped note while it is still in view, so repeated `}` keeps
    /// advancing even when the viewport clamps at the end of the stream.
    fn jump_note(&mut self, next: bool) {
        let rows = self.annotated_rows();
        if rows.is_empty() {
            self.set_info("no notes in this diff");
            return;
        }
        let scroll_anchor = self.collapse.stream_at_virtual(self.scroll_y);
        let anchor = match self.last_note_jump {
            Some(r)
                if {
                    let v = self.collapse.virtual_of_stream(r);
                    v >= self.scroll_y && v < self.scroll_y + self.viewport_height
                } =>
            {
                r
            }
            _ => scroll_anchor,
        };
        let hit = if next {
            rows.iter().find(|&&r| r > anchor).or_else(|| rows.first())
        } else {
            rows.iter()
                .rev()
                .find(|&&r| r < anchor)
                .or_else(|| rows.last())
        };
        if let Some(&row) = hit {
            let ordinal = rows.iter().position(|&r| r == row).map_or(0, |i| i + 1);
            self.jump_to_stream(row);
            self.last_note_jump = Some(row);
            self.set_info(format!("💬 note {ordinal}/{}", rows.len()));
        }
    }

    /// Re-resolve the chrome theme and the syntect syntax palette from the
    /// current (palette, mode) pair. Shared by `t` (mode cycle), `T`
    /// (palette cycle), and startup. Loads a fresh `Highlighter` Arc so
    /// in-flight worker jobs keep the old theme gen-safe, then bumps the
    /// cache generation so stale runs are discarded.
    pub fn apply_theme(&mut self) {
        self.theme = self.palette.theme(self.theme_mode);
        self.highlighter = Arc::new(
            Highlighter::load(self.palette.syntect_theme_name(self.theme_mode))
                .unwrap_or_else(|_| Highlighter::load_noop()),
        );
        self.cache.invalidate();
    }

    // ── note composition (`c` at the cursor) ─────────────────────────────────

    /// Iterate code lines from `from_row` to the end of its file as
    /// `(old_no, new_no)` pairs. Headers, markers, and other files stop or
    /// are skipped. Single keypress-sized walks (`c`, `o`), never per-frame.
    fn code_lines_from(&self, from_row: usize) -> Vec<(Option<u32>, Option<u32>)> {
        let Some(file_idx) = ViewportQuery::file_at_row(&self.review, from_row) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for row in from_row..self.review.stream_len {
            if ViewportQuery::file_at_row(&self.review, row) != Some(file_idx) {
                break; // crossed into the next file
            }
            if let Some(nums) = ViewportQuery::row_line_numbers(&self.review, row) {
                if nums != (None, None) {
                    out.push(nums); // Meta lines carry no numbers
                }
            }
        }
        out
    }

    /// Where a note composed at the cursor (`c`) attaches: the cursor's code
    /// line if it has a new-side number; otherwise the next line that does
    /// (a delete anchors to the add block that replaces it); a file with no
    /// numbered lines at all falls back to its first hunk, else a banner.
    fn note_target_at_cursor(&self) -> Option<NoteTarget> {
        let cursor_row = self.cursor_stream_row();
        let file_idx = ViewportQuery::file_at_row(&self.review, cursor_row)?;
        let path = self.review.display_path(file_idx).to_string();
        if let Some((_, Some(new_no))) = self
            .code_lines_from(cursor_row)
            .into_iter()
            .find(|(_, new_no)| new_no.is_some())
        {
            return Some(NoteTarget::Line { path, line: new_no });
        }
        let has_hunks = self
            .review
            .files
            .get(file_idx)
            .map(|f| !f.hunks.is_empty())
            .unwrap_or(false);
        if has_hunks {
            Some(NoteTarget::Hunk { path, hunk: 1 })
        } else {
            Some(NoteTarget::Banner)
        }
    }

    /// Start composing a note anchored to the cursor row (`c`).
    fn begin_note(&mut self) {
        match self.note_target_at_cursor() {
            Some(target) => {
                self.note_pending = Some(target);
                self.note_draft.clear();
                self.mode = InputMode::Note;
                self.set_info("note: ");
            }
            None => self.set_error("nothing to annotate here"),
        }
    }

    /// Handle keys while composing a note. Mirrors the search/filter editors
    /// (Ctrl-U / Ctrl-W line shortcuts, Backspace, live echo in the status
    /// line); Enter saves, Esc discards.
    fn handle_note_input(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') => {
                    self.note_draft.clear();
                    self.set_info("note: ");
                    return;
                }
                KeyCode::Char('w') => {
                    drop_last_word(&mut self.note_draft);
                    self.set_info(format!("note: {}", self.note_draft));
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Enter => {
                self.save_note();
                self.mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
                self.note_draft.clear();
                self.note_pending = None;
                self.set_info("note cancelled");
            }
            KeyCode::Backspace => {
                self.note_draft.pop();
                self.set_info(format!("note: {}", self.note_draft));
            }
            KeyCode::Char(c) => {
                self.note_draft.push(c);
                self.set_info(format!("note: {}", self.note_draft));
            }
            _ => {}
        }
    }

    /// Save the composed note: it renders like any `--note` and is mirrored
    /// into the session comments (id `user:N`) so `comment list` / `comment
    /// rm` see it in serve mode.
    fn save_note(&mut self) {
        let Some(target) = self.note_pending.take() else {
            return;
        };
        let text = self.note_draft.trim().to_string();
        self.note_draft.clear();
        if text.is_empty() {
            self.set_info("empty note discarded");
            return;
        }
        self.user_note_seq += 1;
        let id = format!("user:{}", self.user_note_seq);
        let (file, line, hunk) = match &target {
            NoteTarget::Line { path, line } => (path.clone(), Some(*line), None),
            NoteTarget::Hunk { path, hunk } => (path.clone(), None, Some(*hunk)),
            NoteTarget::Banner => (String::new(), None, None),
        };
        self.comments.push(CommentEntry {
            id: id.clone(),
            file,
            text: text.clone(),
            line,
            hunk,
        });
        let note = Note { target, text };
        self.user_notes.insert(id, note.clone());
        self.notes.push(note);
        self.set_success("💬 note added");
    }

    /// Fold (collapse) the currently selected file so only its header is visible.
    fn fold_current(&mut self) {
        if self.folded.insert(self.selected_file) {
            self.set_info(format!("▼ {} (folded)", self.current_path()));
            // Folded files drop their virtual rows; re-anchor the view on
            // the stream row we were looking at.
            self.rebuild_collapse();
        } else {
            self.set_error(format!("already folded: {}", self.current_path()));
        }
    }

    /// Unfold (expand) the currently selected file, revealing its body.
    fn unfold_current(&mut self) {
        if self.folded.remove(&self.selected_file) {
            self.set_success(format!("▶ {} (unfolded)", self.current_path()));
            self.rebuild_collapse();
        } else {
            self.set_error(format!("not folded: {}", self.current_path()));
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
        self.rebuild_collapse();
    }

    /// Compute the file + line to open for the `o` (open in editor) action.
    ///
    /// `o` targets the cursor row. If that row is a header (file or hunk) or
    /// a delete without a new side, scan forward within the file to the first
    /// code line. For a code line we prefer the new-side line number (so
    /// edits land on the live file); deletes have no new-side, so they fall
    /// back to the old-side number. `None` when no code line follows or the
    /// file has no on-disk path.
    fn compute_open_target(&self) -> Option<OpenTarget> {
        let cursor_row = self.cursor_stream_row();
        let file_idx = ViewportQuery::file_at_row(&self.review, cursor_row)?;
        let line = self
            .code_lines_from(cursor_row)
            .into_iter()
            .find_map(|(old_no, new_no)| new_no.or(old_no))?;
        let file = self.review.files.get(file_idx)?;
        let path = file
            .new_path
            .clone()
            .filter(|p| p != "/dev/null")
            .or_else(|| file.old_path.clone().filter(|p| p != "/dev/null"))?;
        if path == "unknown" {
            return None;
        }
        Some(OpenTarget { path, line })
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

    // ---- Toast / feedback system -------------------------------------------

    #[test]
    fn toast_from_str_defaults_to_info_and_is_stamped() {
        let t: Toast = "no matches".into();
        assert_eq!(t.kind, ToastKind::Info);
        assert!(t.set_at.is_some(), "non-sticky toast is stamped now");
        assert_eq!(t.message, "no matches");
    }

    #[test]
    fn toast_contains_delegates_to_message() {
        // Backward-compat shim: existing `app.status.contains(...)` assertions
        // must keep working now that `status` is a Toast, not a String.
        let t: Toast = "reloaded (3 files)".into();
        assert!(t.contains("reloaded"));
        assert!(!t.contains("failed"));
        assert!(!t.is_empty());
        assert!(Toast::default().is_empty());
    }

    #[test]
    fn set_error_marks_error_kind() {
        let mut app = two_file_app();
        app.set_error("boom");
        assert_eq!(app.status.kind, ToastKind::Error);
        assert_eq!(app.status.message, "boom");
        assert!(app.status.contains("boom"));
    }

    #[test]
    fn set_success_marks_success_kind() {
        let mut app = two_file_app();
        app.set_success("highlight on");
        assert_eq!(app.status.kind, ToastKind::Success);
    }

    #[test]
    fn expire_status_clears_a_past_toast_but_not_sticky() {
        let mut app = two_file_app();
        // Initial status is the sticky startup hint.
        assert!(app.status.set_at.is_none(), "startup hint is sticky");
        app.expire_status();
        assert!(!app.status.is_empty(), "sticky toast never expires");

        // A freshly-set info toast hasn't elapsed its TTL, so it survives.
        app.set_info("search: foo");
        app.expire_status();
        assert!(!app.status.is_empty(), "fresh toast not yet expired");
    }

    #[test]
    fn toast_kind_ttl_error_outlasts_info() {
        assert!(ToastKind::Error.ttl_secs() > ToastKind::Info.ttl_secs());
        assert_eq!(ToastKind::Info.ttl_secs(), ToastKind::Success.ttl_secs());
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
        // The cursor moves; the viewport only follows at the edges.
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.cursor_v, 1);
        assert_eq!(
            app.scroll_y, 0,
            "viewport should not move while the cursor is in view"
        );
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.cursor_v, 0);
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
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.cursor_v, 1);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.cursor_v, 0);
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
        // max_scroll = 8 - 4 = 4. Cursor moves in half-page steps; the view
        // follows only when the cursor would leave it.
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.cursor_v, 2);
        assert_eq!(app.scroll_y, 0);
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.cursor_v, 4);
        assert_eq!(
            app.scroll_y, 1,
            "view should follow once the cursor hits the bottom edge"
        );
        assert_eq!(app.selected_file, 1, "cursor on b.rs should sync the rail");
        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.cursor_v, 2);
        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.cursor_v, 0);
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

    #[test]
    fn hunk_jump_advances_when_stream_fits_one_screen() {
        // Regression: with a viewport taller than the whole stream,
        // `max_scroll()` is 0 and `jump_to_stream` cannot move `scroll_y`,
        // so the old viewport-top anchor never advanced — `]h` pinned to the
        // first hunk forever. Repeated `]h` must still walk the hunks (and
        // the cursor must land on each hunk header row).
        let mut app = multi_hunk_app();
        // taller than the 16-row stream → everything fits on screen
        app.viewport_height = 40;
        assert_eq!(app.max_scroll(), 0, "fixture must fit one screen");
        let seq = |app: &mut App| {
            app.handle_key(char_key(']'));
            app.handle_key(char_key('h'));
            app.cursor_v
        };
        assert_eq!(seq(&mut app), 1, "first ]h → a.rs hunk 1");
        assert_eq!(seq(&mut app), 5, "second ]h → a.rs hunk 2");
        assert_eq!(seq(&mut app), 10, "third ]h → b.rs hunk 1");
        assert_eq!(seq(&mut app), 13, "fourth ]h → b.rs hunk 2");
        assert_eq!(seq(&mut app), 1, "fifth ]h wraps to a.rs hunk 1");
    }

    #[test]
    fn hunk_jump_backward_advances_when_stream_fits_one_screen() {
        let mut app = multi_hunk_app();
        app.viewport_height = 40;
        let seq = |app: &mut App| {
            app.handle_key(char_key('['));
            app.handle_key(char_key('h'));
            app.cursor_v
        };
        // from the top: wraps to the last hunk, then walks backwards
        assert_eq!(seq(&mut app), 13);
        assert_eq!(seq(&mut app), 10);
        assert_eq!(seq(&mut app), 5);
        assert_eq!(seq(&mut app), 1);
    }

    #[test]
    fn space_jumps_next_hunk_one_screen() {
        // `Space` shares jump_hunk; same one-screen regression coverage.
        let mut app = multi_hunk_app();
        app.viewport_height = 40;
        app.handle_key(char_key(' '));
        assert_eq!(app.cursor_v, 1);
        app.handle_key(char_key(' '));
        assert_eq!(app.cursor_v, 5);
    }

    #[test]
    fn hunk_jump_anchor_resets_after_manual_scroll() {
        // If the human scrolls away, the stale anchor must not hijack the
        // next `]h` — it falls back to searching from the viewport top.
        let mut app = multi_hunk_app();
        app.viewport_height = 1; // forces scrolling, max_scroll > 0
        app.handle_key(char_key(']'));
        app.handle_key(char_key('h'));
        assert_eq!(app.scroll_y, 1);
        // jump far away (bottom), then back to top: viewport no longer shows row 1
        app.handle_key(char_key('G'));
        app.handle_key(char_key('g'));
        app.handle_key(char_key(']'));
        app.handle_key(char_key('h'));
        assert_eq!(
            app.scroll_y, 1,
            "]h from the top finds the first hunk again"
        );
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
        // put the cursor on the context line (row 2)
        app.set_cursor(2);
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
        // put the cursor on the +new line (row 4)
        app.set_cursor(4);
        app.handle_key(char_key('o'));
        let target = app.open_request.expect("open request set");
        assert_eq!(target.line, 11, "add line should use new-side number");
    }

    #[test]
    fn o_falls_back_to_old_side_on_delete_line() {
        let mut app = openable_app();
        // put the cursor on the -old line (row 3)
        app.set_cursor(3);
        app.handle_key(char_key('o'));
        let target = app.open_request.expect("open request set");
        assert_eq!(target.line, 11, "delete line falls back to old-side number");
    }

    #[test]
    fn o_on_header_scans_forward_to_first_code_line() {
        let mut app = openable_app();
        // top of the file: the cursor sits on the file header. o should scan
        // forward to the first code line (ctx at row 2).
        app.set_cursor(0);
        app.handle_key(char_key('o'));
        let target = app.open_request.expect("should scan to a code line");
        assert_eq!(target.line, 10);
    }

    #[test]
    fn o_clears_request_each_press() {
        let mut app = openable_app();
        app.set_cursor(2);
        app.handle_key(char_key('o'));
        assert!(app.open_request.is_some());
        // simulate the run loop consuming it
        let _ = app.open_request.take();
        assert!(app.open_request.is_none());
    }

    #[test]
    fn o_with_no_code_lines_is_noop() {
        // A binary-file placeholder has no code lines anywhere in the file,
        // so the forward scan from the cursor finds nothing to open.
        let review = parse_unified_diff(
            "diff --git a/bin.dat b/bin.dat\nnew file mode 100644\nBinary files /dev/null and b/bin.dat differ\n",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.handle_key(char_key('o'));
        assert!(app.open_request.is_none());
        assert!(app.status.contains("nothing"));
    }

    // ---- cursor + note composition (`c`) ----

    #[test]
    fn shift_t_cycles_the_palette_and_reloads_the_theme() {
        let mut app = two_file_app();
        assert_eq!(app.palette, crate::tui::theme::Palette::Flexoki);
        app.theme_mode = crate::tui::theme::ThemeMode::Dark;
        app.apply_theme();
        let gen_before = app.cache.current_gen();
        app.handle_key(char_key('T'));
        assert_eq!(app.palette, crate::tui::theme::Palette::Catppuccin);
        assert_eq!(
            app.theme.status_bg,
            crate::tui::theme::Theme::catppuccin_mocha().status_bg,
            "chrome should switch to Mocha"
        );
        assert!(
            app.cache.current_gen() > gen_before,
            "highlight cache generation must bump so stale runs are dropped"
        );
        assert!(
            app.status.contains("catppuccin-mocha"),
            "status should name the preset: {}",
            app.status.message
        );
        // Keep cycling wraps back to Flexoki after the full ring.
        for _ in 0..4 {
            app.handle_key(char_key('T'));
        }
        assert_eq!(app.palette, crate::tui::theme::Palette::Flexoki);
    }

    #[test]
    fn t_cycles_mode_within_the_current_palette() {
        let mut app = two_file_app();
        app.palette = crate::tui::theme::Palette::Catppuccin;
        app.theme_mode = crate::tui::theme::ThemeMode::Dark;
        app.apply_theme();
        app.handle_key(char_key('t'));
        assert_eq!(app.theme_mode, crate::tui::theme::ThemeMode::Light);
        assert_eq!(
            app.theme.status_bg,
            crate::tui::theme::Theme::catppuccin_latte().status_bg,
            "mode switch should resolve to the family's light variant"
        );
        assert!(app.status.contains("catppuccin-latte"));
    }

    #[test]
    fn c_composes_note_at_cursor_line() {
        let mut app = openable_app();
        // Cursor on the +new line (row 4, new-side line 11).
        app.set_cursor(4);
        app.handle_key(char_key('c'));
        assert_eq!(app.mode, InputMode::Note, "c should open the note composer");
        for ch in "check this".chars() {
            app.handle_key(char_key(ch));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, InputMode::Normal);
        assert_eq!(app.notes.len(), 1);
        assert_eq!(
            app.notes[0],
            Note {
                target: NoteTarget::Line {
                    path: "src/main.rs".into(),
                    line: 11
                },
                text: "check this".into()
            }
        );
        // Mirrored into session comments under a user id.
        assert_eq!(app.comments.len(), 1);
        assert_eq!(app.comments[0].id, "user:1");
        assert!(app.status.contains("note added"));
    }

    #[test]
    fn c_on_delete_anchors_to_replacing_add_line() {
        let mut app = openable_app();
        // Cursor on the -old line (row 3): no new side, so the note anchors
        // to the next new-side line (the +new line at 11).
        app.set_cursor(3);
        app.handle_key(char_key('c'));
        app.handle_key(key(KeyCode::Enter)); // empty -> discarded
                                             // Re-compose and save for real.
        app.handle_key(char_key('c'));
        for ch in "hm".chars() {
            app.handle_key(char_key(ch));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.notes[0].target,
            NoteTarget::Line {
                path: "src/main.rs".into(),
                line: 11
            }
        );
    }

    #[test]
    fn note_composer_esc_cancels_and_enter_empty_discards() {
        let mut app = openable_app();
        app.set_cursor(4);
        app.handle_key(char_key('c'));
        for ch in "draft".chars() {
            app.handle_key(char_key(ch));
        }
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.notes.is_empty(), "Esc should discard the draft");
        assert!(app.status.contains("cancelled"));

        app.handle_key(char_key('c'));
        app.handle_key(key(KeyCode::Enter)); // empty draft
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.notes.is_empty(), "empty note should be discarded");
        assert!(app.status.contains("empty"));
    }

    #[test]
    fn note_composer_ctrl_shortcuts_edit_the_draft() {
        let mut app = openable_app();
        app.set_cursor(4);
        app.handle_key(char_key('c'));
        for ch in "one two three".chars() {
            app.handle_key(char_key(ch));
        }
        app.handle_key(ctrl('w')); // drop "three" (and its preceding space)
        assert_eq!(app.note_draft, "one two");
        app.handle_key(ctrl('u')); // clear
        assert_eq!(app.note_draft, "");
        app.handle_key(key(KeyCode::Esc));
    }

    #[test]
    fn jump_keys_land_the_cursor_on_the_target() {
        let mut app = two_file_app();
        app.handle_key(key(KeyCode::Char('G')));
        let bottom = app.cursor_v;
        assert!(bottom > 0);
        app.handle_key(key(KeyCode::Char('g')));
        assert_eq!(app.cursor_v, 0);
        // ]h jumps the viewport AND lands the cursor on the hunk header.
        app.handle_key(key(KeyCode::Char(']')));
        app.handle_key(char_key('h'));
        assert_eq!(app.cursor_v, app.collapse.virtual_of_stream(1));
    }

    #[test]
    fn wheel_scroll_keeps_the_cursor_in_view() {
        let mut app = three_file_app();
        app.viewport_height = 4;
        // Scroll the viewport down past the cursor; the cursor clamps to the
        // viewport's top edge.
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_y, 1);
        assert_eq!(app.cursor_v, 1, "cursor should stick to the top edge");
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
    fn reload_preserves_view_state_layout_collapse_cursor() {
        let (before, after) = reload_pair();
        let review = parse_unified_diff(&before).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 3;
        // Arrange a non-default view state: split layout, collapse off,
        // b.rs folded, cursor on a code line.
        app.layout_mode = crate::config::LayoutMode::Split;
        app.collapse_on = false;
        app.context_threshold = 0;
        app.folded.insert(1);
        app.rebuild_collapse();
        app.set_cursor(2);
        let cursor_row = app.cursor_stream_row();

        app.reload_review(&after);

        assert_eq!(
            app.layout_mode,
            crate::config::LayoutMode::Split,
            "layout choice survives reload"
        );
        assert!(!app.collapse_on, "context-collapse toggle survives reload");
        assert!(
            app.folded.contains(&1),
            "folded files survive reload (path-remapped)"
        );
        // The cursor stays clamped inside the new stream (same file kept
        // its rows in reload_pair's "after").
        assert!(
            app.cursor_v < app.collapse.virtual_len(),
            "cursor re-anchored inside the new stream"
        );
        assert_eq!(
            app.cursor_stream_row(),
            cursor_row,
            "cursor keeps its stream row across reload"
        );
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
        // Click on stream row 3 → the cursor moves there; the viewport stays.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.cursor_v, 3);
        assert_eq!(app.scroll_y, 0, "a click should not scroll the viewport");
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
        app.handle_key(ctrl('d'));
        assert_eq!(app.cursor_v, 3);
        assert_eq!(
            app.scroll_y, 0,
            "cursor stays in view, viewport does not move"
        );
        app.handle_key(ctrl('d'));
        assert_eq!(app.cursor_v, 6);
        assert_eq!(app.scroll_y, 1, "view follows once the cursor leaves it");
    }

    #[test]
    fn ctrl_u_scrolls_half_page_up() {
        let mut app = three_file_app();
        app.viewport_height = 6; // half = 3
        app.set_cursor(5);
        app.handle_key(ctrl('u'));
        assert_eq!(app.cursor_v, 2);
        assert_eq!(
            app.scroll_y, 0,
            "view follows the cursor up to the top edge"
        );
    }

    #[test]
    fn ctrl_f_scrolls_full_page_down() {
        let mut app = three_file_app();
        app.viewport_height = 4; // full = 4
        app.handle_key(ctrl('f'));
        assert_eq!(app.cursor_v, 4);
        assert_eq!(
            app.scroll_y, 1,
            "view follows the cursor to the bottom edge"
        );
    }

    #[test]
    fn ctrl_b_scrolls_full_page_up() {
        let mut app = three_file_app();
        app.viewport_height = 4; // full = 4
        app.set_cursor(9);
        app.handle_key(ctrl('b'));
        assert_eq!(app.cursor_v, 5);
        assert_eq!(app.scroll_y, 5, "view follows the cursor up");
    }

    #[test]
    fn ctrl_d_clamps_at_max_scroll() {
        let mut app = three_file_app();
        app.viewport_height = 6;
        let max = app.max_scroll();
        app.handle_key(key(KeyCode::Char('G'))); // cursor + view to the bottom
        app.handle_key(ctrl('d'));
        assert_eq!(app.scroll_y, max);
        let last_v = app.collapse.virtual_len() - 1;
        assert_eq!(app.cursor_v, last_v, "cursor clamps at the last row");
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

    // ---- context collapse (zx) ----

    /// One file, one hunk: -x, 12 context lines, +y. Stream rows:
    /// 0=file header, 1=hunk header, 2=-x, 3..14=context, 15=+y.
    fn long_context_app() -> App {
        let body: String = (0..12).map(|i| format!(" pad{i}\n")).collect();
        let review = parse_unified_diff(&format!(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,14 +1,14 @@
-x
{body}+y
"
        ))
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 6;
        app
    }

    #[test]
    fn collapse_on_by_default_shrinks_virtual_stream() {
        let app = long_context_app();
        // 16 stream rows; the 12-line context run collapses to one marker.
        assert!(app.collapse_on);
        assert_eq!(app.collapse.virtual_len(), 5);
    }

    #[test]
    fn zx_toggles_collapse_and_keeps_anchor() {
        let mut app = long_context_app();
        // Scroll onto the +y line (stream 15 → virtual 4).
        app.handle_key(char_key('G'));
        let seen_before = app.collapse.stream_at_virtual(app.scroll_y);
        app.handle_key(char_key('z'));
        app.handle_key(char_key('x'));
        assert!(!app.collapse_on);
        assert_eq!(app.collapse.virtual_len(), 16);
        // Still looking at the same stream row.
        assert_eq!(app.collapse.stream_at_virtual(app.scroll_y), seen_before);
        app.handle_key(char_key('z'));
        app.handle_key(char_key('x'));
        assert!(app.collapse_on);
        assert_eq!(app.collapse.virtual_len(), 5);
    }

    #[test]
    fn search_jump_expands_the_collapsed_run_it_lands_in() {
        let mut app = long_context_app();
        // Search for a line inside the collapsed context run.
        app.mode = crate::tui::app::InputMode::Search;
        app.handle_key(char_key('d'));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.search.active);
        assert_eq!(app.search.matches.len(), 12);
        // The jump expanded the run containing the first match (pad0, row 3):
        // stream 3 now maps to virtual 3 (identity restored for that run).
        assert_eq!(app.collapse.virtual_of_stream(3), 3);
        // The viewport shows the match row at the top.
        assert_eq!(app.scroll_y, 3);
    }

    #[test]
    fn hunk_jump_lands_on_header_in_virtual_coords() {
        // Two hunks 8 unchanged lines apart: the gap collapses to a marker,
        // so hunk 2's header moves up one virtual row.
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 ctx
-a
+b
@@ -12,3 +12,3 @@
 ctx2
-c
+d
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 4;
        // Two ]h presses: the first lands on hunk 1's header (virtual 1),
        // the second on hunk 2's header — stream row 5, virtual 6 with the
        // gap marker inserted before it.
        app.handle_key(char_key(']'));
        app.handle_key(char_key('h'));
        app.handle_key(char_key(']'));
        app.handle_key(char_key('h'));
        assert_eq!(app.scroll_y, 6);
        // The row above the header is the gap marker.
        let rows = crate::ir::ViewportQuery::rows_virtual(
            &app.review,
            crate::ir::Viewport {
                start: app.scroll_y - 1,
                height: 2,
            },
            &app.collapse,
        );
        assert!(matches!(
            rows[0].row,
            crate::ir::StreamRow::Unchanged { count: 8, .. }
        ));
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
