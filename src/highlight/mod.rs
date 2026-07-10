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

        /// Construct a no-op highlighter (identical API, no styling).
        /// Available on both cfg branches so callers can always degrade.
        pub fn load_noop() -> Self {
            // Reuses load() in the syntect build; the fallback impl below
            // provides the true no-op. Kept here for API symmetry.
            Self::load().unwrap_or_else(|_| Self {
                syntaxes: SyntaxSet::new(),
                theme: Theme::default(),
            })
        }

        /// Highlight a single line of code text into styled runs.
        ///
        /// `path` selects the syntax by extension (plain text fallback).
        /// `state` carries cross-line parser state and is reset per file —
        /// callers must create a fresh state for each file's first line.
        pub fn highlight(
            &self,
            path: &str,
            line_text: &str,
            state: &mut Option<HighlightLines>,
        ) -> StyledRuns {
            let syntax = self
                .syntaxes
                .find_syntax_by_extension(path_extension(path))
                .or_else(|| self.syntaxes.find_syntax_for_file(path).ok().flatten())
                .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());

            let h = state.get_or_insert_with(|| HighlightLines::new(syntax, &self.theme));
            // highlight_line REQUIRES a trailing newline.
            let mut buf = String::with_capacity(line_text.len() + 1);
            buf.push_str(line_text);
            buf.push('\n');

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

/// Marker for cross-line parser state. Opaque alias so view code is
/// feature-agnostic (the concrete type differs per cfg branch; view never
/// names it).
#[cfg(feature = "highlight")]
pub type LineState = Option<syntect::easy::HighlightLines<'static>>;

#[cfg(not(feature = "highlight"))]
pub type LineState = Option<()>;

pub use imp::Highlighter;

/// Cache of highlighted lines, keyed by `(file_idx, line_in_file)`.
///
/// Lazily filled on the render path; misses render plain text synchronously.
/// Invalidated wholesale by bumping `gen` (e.g. on toggle-off or file change).
pub struct HighlightCache {
    map: HashMap<(usize, usize), StyledRuns>,
    /// Per-file parser state, so multi-line constructs persist across renders.
    states: HashMap<usize, LineState>,
    gen: u64,
}

impl HighlightCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            states: HashMap::new(),
            gen: 0,
        }
    }

    /// Invalidate the entire cache (e.g. on toggle off). Cheap: just bump gen
    /// and clear, since we keep no async in-flight work in the sync MVP.
    pub fn invalidate(&mut self) {
        self.gen = self.gen.wrapping_add(1);
        self.map.clear();
        self.states.clear();
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
        let state = self.states.entry(file_idx).or_insert(None);
        let runs = highlighter.highlight(path, text, state);
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
}
