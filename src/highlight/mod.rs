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
//! `(file_idx, line_in_file)`, and cache misses render plain text synchronously
//! rather than blocking the scroll path.

use std::collections::HashMap;

/// A styled run: a ratatui style and the text it applies to.
pub type StyledRuns = Vec<(ratatui::style::Style, String)>;

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
        pub fn load() -> anyhow::Result<Self> {
            let syntaxes = SyntaxSet::load_defaults_newlines();
            let theme_set = ThemeSet::load_defaults();
            let theme = theme_set.themes["base16-ocean.dark"].clone();
            Ok(Self { syntaxes, theme })
        }

        /// Whether real highlighting is compiled in.
        pub fn is_enabled() -> bool {
            true
        }

        /// Construct a highlighter with no theme/syntaxes (degraded).
        pub fn load_noop() -> Self {
            Self::load().unwrap_or(Self {
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
        let mut style = Style::default().fg(Color::Rgb(
            s.foreground.r,
            s.foreground.g,
            s.foreground.b,
        ));
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
        pub fn load() -> anyhow::Result<Self> {
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
pub struct HighlightCache {
    map: HashMap<(usize, usize), StyledRuns>,
    gen: u64,
}

impl HighlightCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            gen: 0,
        }
    }

    /// Invalidate the entire cache (e.g. on toggle off). Cheap: just bump gen
    /// and clear, since we keep no async in-flight work in the sync MVP.
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

    /// Get a cached highlight, or compute+cache it.
    pub fn get_or_highlight(
        &mut self,
        file_idx: usize,
        line_in_file: usize,
        path: &str,
        text: &str,
        highlighter: &Highlighter,
    ) -> StyledRuns {
        // Fast path: already computed.
        if let Some(runs) = self.map.get(&(file_idx, line_in_file)) {
            return runs.clone();
        }
        // No persistent parser state is carried (diff fragments are highlighted
        // line-independently); pass a throwaway state slot for API symmetry.
        let mut state = None;
        let runs = highlighter.highlight(path, text, &mut state);
        self.map.insert((file_idx, line_in_file), runs.clone());
        runs
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
        c.map.insert((0, 0), vec![]);
        c.invalidate();
        assert!(c.map.is_empty());
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

    // Real syntect path (only compiled with the feature).
    #[cfg(feature = "highlight")]
    #[test]
    fn syntect_highlights_rust_line() {
        let h = Highlighter::load().expect("syntect bundled defaults load");
        let mut state = None;
        let runs = h.highlight("a.rs", "fn main() {}", &mut state);
        // At least one styled run; keyword `fn` should get a non-default fg.
        assert!(!runs.is_empty(), "highlight should produce runs");
        let has_color = runs.iter().any(|(s, _)| s.fg.is_some());
        assert!(has_color, "some run should carry a foreground color");
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn syntect_unknown_ext_falls_back_to_plain() {
        let h = Highlighter::load().unwrap();
        let mut state = None;
        // Unknown extension should not panic; falls back to plain text syntax.
        let runs = h.highlight("file.unknownext", "anything", &mut state);
        assert!(!runs.is_empty());
    }
}
