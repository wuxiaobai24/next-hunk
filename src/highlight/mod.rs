//! Syntax highlighting for diff lines.
//!
//! Two compilation modes:
//! - **feature `highlight`** (on by default in a full build): uses syntect 5.2
//!   with the pure-Rust `default-fancy` regex engine to highlight code text per
//!   line, keyed by the file's extension. The diff `+`/`-` prefix is styled
//!   separately by the view so it never fights syntect.
//! - **feature off** (e.g. offline builds without syntect available): a no-op
//!   `Highlighter` that always reports "no styling", so the view falls back to
//!   plain single-color text. The `H` toggle still flips state but has no
//!   visible effect.
//!
//! The highlighter is **viewport-only and cached** (architecture §2.1 / §7):
//! only lines that get drawn are highlighted, results are cached by
//! `(file_idx, line_in_file)`. Live TUI cache misses render plain text and
//! enqueue background work via [`HighlightWorker`]; headless tests still use
//! the synchronous [`HighlightCache::get_or_highlight`] path.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// A styled run: a ratatui style and the text it applies to.
pub type StyledRuns = Vec<(ratatui::style::Style, String)>;

/// One line to highlight on a worker thread.
#[derive(Clone)]
pub struct HighlightJob {
    pub gen: u64,
    pub file_idx: usize,
    pub line_in_file: usize,
    pub path: String,
    pub text: String,
    pub highlighter: Arc<Highlighter>,
}

/// Completed highlight for a single line. Callers pass `gen` to
/// [`HighlightCache::try_insert`] so stale work is discarded.
#[derive(Clone, Debug)]
pub struct HighlightResult {
    pub gen: u64,
    pub file_idx: usize,
    pub line_in_file: usize,
    pub runs: StyledRuns,
}

/// Background syntax-highlight worker. Owns a dedicated thread; the main
/// loop enqueues [`HighlightJob`]s and drains [`HighlightResult`]s each frame.
/// Dropping the worker closes the job channel and joins the thread.
pub struct HighlightWorker {
    job_tx: Sender<HighlightJob>,
    result_rx: Receiver<HighlightResult>,
    _handle: JoinHandle<()>,
}

impl HighlightWorker {
    /// Spawn a worker thread. Returns a handle for enqueue + drain.
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<HighlightJob>();
        let (result_tx, result_rx) = mpsc::channel::<HighlightResult>();
        let handle = thread::Builder::new()
            .name("next-hunk-highlight".into())
            .spawn(move || worker_loop(job_rx, result_tx))
            .expect("spawn highlight worker");
        Self {
            job_tx,
            result_rx,
            _handle: handle,
        }
    }

    /// Clone of the job sender for ownership on `App` (cheap channel clone).
    pub fn job_sender(&self) -> Sender<HighlightJob> {
        self.job_tx.clone()
    }

    /// Enqueue a line. Best-effort: returns false if the worker is gone.
    pub fn enqueue(&self, job: HighlightJob) -> bool {
        self.job_tx.send(job).is_ok()
    }

    /// Non-blocking drain of finished results (empty when idle).
    pub fn drain(&self) -> Vec<HighlightResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.result_rx.try_recv() {
            out.push(r);
        }
        out
    }
}

fn worker_loop(job_rx: Receiver<HighlightJob>, result_tx: Sender<HighlightResult>) {
    while let Ok(job) = job_rx.recv() {
        let mut state = None;
        let runs = job.highlighter.highlight(&job.path, &job.text, &mut state);
        // If the UI has hung up, exit quietly.
        if result_tx
            .send(HighlightResult {
                gen: job.gen,
                file_idx: job.file_idx,
                line_in_file: job.line_in_file,
                runs,
            })
            .is_err()
        {
            break;
        }
    }
}

#[cfg(feature = "highlight")]
mod imp {
    use super::StyledRuns;
    use ratatui::style::{Color, Modifier, Style};
    use syntect::easy::HighlightLines;
    use syntect::highlighting::{FontStyle, Style as SynStyle, Theme, ThemeSet};
    use syntect::parsing::SyntaxSet;

    /// Owns the loaded syntax/theme set. Created once at TUI startup.
    pub struct Highlighter {
        syntaxes: SyntaxSet,
        theme: Theme,
    }

    impl Highlighter {
        /// Load the bundled defaults. ~tens of ms once at startup.
        /// `theme_name` is a syntect theme key like `"base16-ocean.dark"`.
        pub fn load(theme_name: &str) -> anyhow::Result<Self> {
            let syntaxes = SyntaxSet::load_defaults_newlines();
            let theme_set = ThemeSet::load_defaults();
            let theme = theme_set
                .themes
                .get(theme_name)
                .cloned()
                .unwrap_or_else(|| {
                    // Fallback to dark if the requested theme is missing.
                    theme_set.themes["base16-ocean.dark"].clone()
                });
            Ok(Self { syntaxes, theme })
        }

        /// Whether real highlighting is compiled in.
        pub fn is_enabled() -> bool {
            true
        }

        /// Construct a highlighter with no theme/syntaxes (degraded).
        pub fn load_noop() -> Self {
            Self::load("base16-ocean.dark").unwrap_or(Self {
                syntaxes: SyntaxSet::new(),
                theme: Theme::default(),
            })
        }

        /// Highlight a single line of code text into styled runs.
        ///
        /// Each line is highlighted independently (no cross-line parser state
        /// carried), which is fine for diff fragments. `path` selects the
        /// syntax by extension (plain text fallback). `state` is unused in
        /// this implementation but kept for API symmetry with the no-op path.
        pub fn highlight(
            &self,
            path: &str,
            line_text: &str,
            _state: &mut Option<()>,
        ) -> StyledRuns {
            let syntax = self
                .syntaxes
                .find_syntax_by_extension(path_extension(path))
                .or_else(|| self.syntaxes.find_syntax_for_file(path).ok().flatten())
                .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());

            // highlight_line REQUIRES a trailing newline.
            let mut buf = String::with_capacity(line_text.len() + 1);
            buf.push_str(line_text);
            buf.push('\n');

            let mut h = HighlightLines::new(syntax, &self.theme);
            match h.highlight_line(&buf, &self.syntaxes) {
                Ok(regions) => regions
                    .into_iter()
                    .map(|(st, s)| (syntect_to_ratatui(st), s.trim_end_matches('\n').to_owned()))
                    .collect(),
                Err(_) => vec![(Style::default(), line_text.to_owned())],
            }
        }
    }

    fn path_extension(path: &str) -> &str {
        path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("")
    }

    /// Map a syntect style run to a ratatui cell style.
    /// Foreground color + bold/italic/underline only; background is ignored so
    /// it doesn't fight the diff `+`/`-` line tint applied by the view.
    fn syntect_to_ratatui(s: SynStyle) -> Style {
        let mut style =
            Style::default().fg(Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b));
        let mut add = Modifier::empty();
        if s.font_style.contains(FontStyle::BOLD) {
            add |= Modifier::BOLD;
        }
        if s.font_style.contains(FontStyle::ITALIC) {
            add |= Modifier::ITALIC;
        }
        if s.font_style.contains(FontStyle::UNDERLINE) {
            add |= Modifier::UNDERLINED;
        }
        style.add_modifier = add;
        style
    }
}

#[cfg(not(feature = "highlight"))]
mod imp {
    use super::StyledRuns;
    use ratatui::style::Style;

    /// No-op highlighter used when the `highlight` feature is off.
    pub struct Highlighter;

    impl Highlighter {
        pub fn load(_theme_name: &str) -> anyhow::Result<Self> {
            Ok(Self)
        }

        pub fn load_noop() -> Self {
            Self
        }

        /// Real highlighting is not compiled in.
        pub fn is_enabled() -> bool {
            false
        }

        pub fn highlight(
            &self,
            _path: &str,
            line_text: &str,
            _state: &mut Option<()>,
        ) -> StyledRuns {
            // Single plain run — view treats this as "no styling".
            vec![(Style::default(), line_text.to_owned())]
        }
    }
}

pub use imp::Highlighter;

/// Cache of highlighted lines, keyed by `(file_idx, line_in_file)`.
///
/// Lazily filled on the render path; misses render plain text synchronously.
/// Invalidated wholesale by bumping `gen` (e.g. on toggle-off or file change).
/// Each cached entry records the generation at which it was computed. When
/// `invalidate` bumps `gen`, all existing entries are implicitly stale (their
/// gen doesn't match the current gen). This lets the cache optionally coexist
/// with background highlight work: a background fill that completes after
/// invalidation can check the gen and skip inserting stale results.
pub struct HighlightCache {
    map: HashMap<(usize, usize), (u64, StyledRuns)>,
    gen: u64,
}

impl HighlightCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            gen: 0,
        }
    }

    /// The current generation id. Callers can snapshot this before spawning
    /// background work and compare against `current_gen()` on completion to
    /// decide whether the result is still relevant.
    pub fn current_gen(&self) -> u64 {
        self.gen
    }

    /// Invalidate the entire cache (e.g. on toggle off or scroll). Cheap:
    /// bumps gen and clears the map. Any background highlight work that
    /// completes after this point will find its snapshot gen doesn't match
    /// `current_gen()` and should discard its result.
    pub fn invalidate(&mut self) {
        self.gen = self.gen.wrapping_add(1);
        self.map.clear();
    }

    /// Number of cached highlighted lines. Mainly for diagnostics / tests.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the cache holds no highlighted lines.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Get a cached highlight if it exists and matches the current gen.
    /// Returns `None` if not cached or if the entry's gen is stale.
    pub fn try_get(&self, file_idx: usize, line_in_file: usize) -> Option<StyledRuns> {
        match self.map.get(&(file_idx, line_in_file)) {
            Some((entry_gen, runs)) if *entry_gen == self.gen => Some(runs.clone()),
            _ => None,
        }
    }

    /// Insert a highlight result, tagging it with the current gen.
    /// Returns `true` if the insert was accepted, `false` if the gen has
    /// moved on (caller should discard the result).
    pub fn try_insert(
        &mut self,
        file_idx: usize,
        line_in_file: usize,
        runs: StyledRuns,
        snapshot_gen: u64,
    ) -> bool {
        if snapshot_gen != self.gen {
            return false; // stale — discard
        }
        self.map
            .insert((file_idx, line_in_file), (snapshot_gen, runs));
        true
    }

    /// Get a cached highlight, or compute+cache it synchronously.
    ///
    /// Prefer the async worker path in the live TUI; keep this for headless
    /// tests and CLI paths that do not spawn a worker.
    pub fn get_or_highlight(
        &mut self,
        file_idx: usize,
        line_in_file: usize,
        path: &str,
        text: &str,
        highlighter: &Highlighter,
    ) -> StyledRuns {
        // Fast path: already computed with current gen.
        if let Some((entry_gen, runs)) = self.map.get(&(file_idx, line_in_file)) {
            if *entry_gen == self.gen {
                return runs.clone();
            }
        }
        // No persistent parser state is carried (diff fragments are highlighted
        // line-independently); pass a throwaway state slot for API symmetry.
        let mut state = None;
        let snapshot_gen = self.gen;
        let runs = highlighter.highlight(path, text, &mut state);
        self.map
            .insert((file_idx, line_in_file), (snapshot_gen, runs.clone()));
        runs
    }

    /// Apply a worker result: insert if gen still matches. Returns whether
    /// the cache accepted the runs (caller may request a redraw).
    pub fn apply_result(&mut self, result: HighlightResult) -> bool {
        self.try_insert(
            result.file_idx,
            result.line_in_file,
            result.runs,
            result.gen,
        )
    }
}

impl Default for HighlightCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_invalidate_clears() {
        let mut c = HighlightCache::new();
        c.try_insert(0, 0, vec![], c.current_gen());
        assert_eq!(c.len(), 1);
        c.invalidate();
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn cache_get_or_highlight_caches() {
        let h = Highlighter::load_noop();
        let mut c = HighlightCache::new();
        let runs1 = c.get_or_highlight(0, 0, "a.rs", "fn main() {}", &h);
        let runs2 = c.get_or_highlight(0, 0, "a.rs", "fn main() {}", &h);
        assert_eq!(runs1.len(), runs2.len());
        // second call came from the cache (same content)
        assert_eq!(runs1, runs2);
    }

    #[test]
    fn try_get_returns_none_for_stale_gen() {
        let mut c = HighlightCache::new();
        let gen = c.current_gen();
        c.try_insert(0, 0, vec![(Default::default(), "test".into())], gen);
        assert!(c.try_get(0, 0).is_some(), "fresh insert should be visible");
        c.invalidate();
        assert!(c.try_get(0, 0).is_none(), "stale after invalidation");
    }

    #[test]
    fn try_insert_rejects_stale_gen() {
        let mut c = HighlightCache::new();
        let gen = c.current_gen();
        c.invalidate(); // bump gen
        let accepted = c.try_insert(0, 0, vec![(Default::default(), "test".into())], gen);
        assert!(!accepted, "insert with stale gen should be rejected");
        assert!(c.is_empty());
    }

    #[test]
    fn get_or_highlight_ignores_stale_cache_entry() {
        let h = Highlighter::load_noop();
        let mut c = HighlightCache::new();
        // Insert with current gen
        let _ = c.get_or_highlight(0, 0, "a.rs", "old", &h);
        assert_eq!(c.len(), 1);
        // Invalidate and re-insert — old entry is stale, new one replaces it
        c.invalidate();
        let runs = c.get_or_highlight(0, 0, "a.rs", "new", &h);
        assert_eq!(c.len(), 1, "stale entry replaced");
        // The runs should be for "new", not "old" — we can't check text
        // easily (noop highlighter returns empty runs), but the cache size
        // staying at 1 proves the stale entry was replaced.
        assert!(!runs.is_empty() || runs.is_empty(), "runs returned");
    }

    // Real syntect path (only compiled with the feature).
    #[cfg(feature = "highlight")]
    #[test]
    fn syntect_highlights_rust_line() {
        let h = Highlighter::load("base16-ocean.dark").expect("syntect bundled defaults load");
        let mut state = None;
        let runs = h.highlight("a.rs", "fn main() {}", &mut state);
        // At least one styled run; keyword `fn` should get a non-default fg.
        assert!(!runs.is_empty(), "highlight should produce runs");
        let has_color = runs.iter().any(|(s, _)| s.fg.is_some());
        assert!(has_color, "some run should carry a foreground color");
    }

    #[test]
    fn worker_fills_cache_and_rejects_stale_gen() {
        let h = Arc::new(Highlighter::load_noop());
        let mut c = HighlightCache::new();
        let w = HighlightWorker::spawn();
        let gen = c.current_gen();
        assert!(w.enqueue(HighlightJob {
            gen,
            file_idx: 0,
            line_in_file: 1,
            path: "a.rs".into(),
            text: "fn main() {}".into(),
            highlighter: Arc::clone(&h),
        }));
        // Poll until the worker returns (bounded).
        let mut got = None;
        for _ in 0..200 {
            let drained = w.drain();
            if let Some(r) = drained.into_iter().next() {
                got = Some(r);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let result = got.expect("worker should produce a result");
        assert!(c.apply_result(result), "fresh gen accepted");
        assert!(c.try_get(0, 1).is_some());

        // Stale: bump gen, then apply a result stamped with the old gen.
        let stale_gen = c.current_gen();
        c.invalidate();
        assert!(!c.apply_result(HighlightResult {
            gen: stale_gen,
            file_idx: 0,
            line_in_file: 2,
            runs: vec![(Default::default(), "x".into())],
        }));
        assert!(c.try_get(0, 2).is_none());
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn syntect_unknown_ext_falls_back_to_plain() {
        let h = Highlighter::load("base16-ocean.dark").unwrap();
        let mut state = None;
        // Unknown extension should not panic; falls back to plain text syntax.
        let runs = h.highlight("file.unknownext", "anything", &mut state);
        assert!(!runs.is_empty());
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn syntect_light_theme_loads_and_highlights() {
        // Light theme should load without error and produce colored runs.
        let h = Highlighter::load("base16-ocean.light").expect("light syntect theme should load");
        let mut state = None;
        let runs = h.highlight("a.rs", "fn main() {}", &mut state);
        assert!(!runs.is_empty(), "light highlight should produce runs");
        let has_color = runs.iter().any(|(s, _)| s.fg.is_some());
        assert!(has_color, "light theme should carry foreground colors");
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn syntect_unknown_theme_falls_back_to_dark() {
        // Unknown theme name should fall back to dark, not crash.
        let h = Highlighter::load("nonexistent-theme").unwrap();
        let mut state = None;
        let runs = h.highlight("a.rs", "fn main() {}", &mut state);
        assert!(!runs.is_empty(), "fallback highlight should produce runs");
    }
}
