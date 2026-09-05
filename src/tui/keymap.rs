//! Remappable keybindings (`[keybindings]` in config.toml).
//!
//! Every interactive command is a named [`Action`]. The default keymap
//! reproduces the built-in keys exactly; a `[keybindings]` table overrides
//! the keys of any action:
//!
//! ```toml
//! [keybindings]
//! next_hunk = ["]j", "space"]   # list (or a single string)
//! quit = false                  # unbind
//! ```
//!
//! Key specs:
//! - single chars: `"q"`, `"?"`, `"}"` (case-sensitive)
//! - named keys: `"esc" "enter" "space" "tab" "backtab" "up" "down" "left"
//!   "right" "home" "end" "pageup" "pagedown" "backspace" "delete" "insert"`
//! - ctrl-modified: `"ctrl-d"`
//! - two-key sequences: `"]h"`, `"[h"`, `"zc"`, `"zo"`, `"zx"` — the first
//!   key arms a pending prefix, the second completes it
//!
//! Claims are exclusive: when two actions claim the same key the first
//! claim wins and a warning names the loser. Unknown action names and
//! malformed specs warn and are ignored — a bad config never bricks the
//! default keys.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One named interactive command. The doc comments double as the source for
/// the help overlay's descriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Cancel,
    CursorDown,
    CursorUp,
    HalfPageDown,
    HalfPageUp,
    PageForward,
    PageBackward,
    GotoTop,
    GotoBottom,
    NextFile,
    PrevFile,
    NextHunk,
    PrevHunk,
    NextNote,
    PrevNote,
    FoldFile,
    UnfoldFile,
    ToggleContextCollapse,
    ToggleHighlight,
    ToggleLineNumbers,
    ToggleWordDiff,
    ToggleIgnoreWhitespace,
    ToggleWrap,
    ToggleRail,
    CycleLayout,
    CycleThemeMode,
    CyclePalette,
    Search,
    FilterPaths,
    ComposeNote,
    OpenEditor,
    NextMatch,
    PrevMatch,
    Help,
    AcceptHunk,
    RejectHunk,
    UndecideHunk,
}

impl Action {
    /// Stable snake_case identifier used in config.toml and `keybindings`
    /// error messages. Never rename — it is user-facing config surface.
    pub fn name(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Cancel => "cancel",
            Action::CursorDown => "cursor_down",
            Action::CursorUp => "cursor_up",
            Action::HalfPageDown => "half_page_down",
            Action::HalfPageUp => "half_page_up",
            Action::PageForward => "page_forward",
            Action::PageBackward => "page_backward",
            Action::GotoTop => "goto_top",
            Action::GotoBottom => "goto_bottom",
            Action::NextFile => "next_file",
            Action::PrevFile => "prev_file",
            Action::NextHunk => "next_hunk",
            Action::PrevHunk => "prev_hunk",
            Action::NextNote => "next_note",
            Action::PrevNote => "prev_note",
            Action::FoldFile => "fold_file",
            Action::UnfoldFile => "unfold_file",
            Action::ToggleContextCollapse => "toggle_context_collapse",
            Action::ToggleHighlight => "toggle_highlight",
            Action::ToggleLineNumbers => "toggle_line_numbers",
            Action::ToggleWordDiff => "toggle_word_diff",
            Action::ToggleIgnoreWhitespace => "toggle_ignore_whitespace",
            Action::ToggleWrap => "toggle_wrap",
            Action::ToggleRail => "toggle_rail",
            Action::CycleLayout => "cycle_layout",
            Action::CycleThemeMode => "cycle_theme_mode",
            Action::CyclePalette => "cycle_palette",
            Action::Search => "search",
            Action::FilterPaths => "filter_paths",
            Action::ComposeNote => "compose_note",
            Action::OpenEditor => "open_editor",
            Action::NextMatch => "next_match",
            Action::PrevMatch => "prev_match",
            Action::Help => "help",
            Action::AcceptHunk => "accept_hunk",
            Action::RejectHunk => "reject_hunk",
            Action::UndecideHunk => "undecide_hunk",
        }
    }

    /// One-line description for the help overlay.
    pub fn describe(self) -> &'static str {
        match self {
            Action::Quit => "quit (clears an active search first)",
            Action::Cancel => "clear the active search / pending key sequence",
            Action::CursorDown => "cursor down one row",
            Action::CursorUp => "cursor up one row",
            Action::HalfPageDown => "cursor half a page down",
            Action::HalfPageUp => "cursor half a page up",
            Action::PageForward => "cursor a full page down",
            Action::PageBackward => "cursor a full page up",
            Action::GotoTop => "cursor to the first row",
            Action::GotoBottom => "cursor to the last row",
            Action::NextFile => "next file",
            Action::PrevFile => "previous file",
            Action::NextHunk => "next hunk (wraps across files)",
            Action::PrevHunk => "previous hunk",
            Action::NextNote => "next annotated (💬) row",
            Action::PrevNote => "previous annotated (💬) row",
            Action::FoldFile => "fold the current file",
            Action::UnfoldFile => "unfold the current file",
            Action::ToggleContextCollapse => "toggle context collapsing",
            Action::ToggleHighlight => "toggle syntax highlighting",
            Action::ToggleLineNumbers => "toggle the line-number gutter",
            Action::ToggleWordDiff => "toggle word-level diff emphasis",
            Action::ToggleIgnoreWhitespace => "toggle ignore-whitespace view",
            Action::ToggleWrap => "toggle line wrapping",
            Action::ToggleRail => "toggle the file rail",
            Action::CycleLayout => "cycle layout (unified → split → stack)",
            Action::CycleThemeMode => "cycle theme mode (dark/light/auto)",
            Action::CyclePalette => "cycle theme palette family",
            Action::Search => "search in the diff stream",
            Action::FilterPaths => "filter files by path",
            Action::ComposeNote => "compose a note on the cursor row",
            Action::OpenEditor => "open the focused line in $EDITOR",
            Action::NextMatch => "next search match",
            Action::PrevMatch => "previous search match",
            Action::Help => "toggle this help overlay",
            Action::AcceptHunk => "accept the current hunk, then jump to the next (--select)",
            Action::RejectHunk => "reject the current hunk, then jump to the next (--select)",
            Action::UndecideHunk => {
                "mark the current hunk undecided, then jump to the next (--select)"
            }
        }
    }

    /// The default key specs, exactly the pre-config built-ins.
    fn default_specs(self) -> &'static [&'static str] {
        match self {
            Action::Quit => &["q"],
            Action::Cancel => &["esc"],
            Action::CursorDown => &["j", "down"],
            Action::CursorUp => &["k", "up"],
            Action::HalfPageDown => &["J", "pagedown", "ctrl-d"],
            Action::HalfPageUp => &["K", "pageup", "ctrl-u"],
            Action::PageForward => &["ctrl-f"],
            Action::PageBackward => &["ctrl-b"],
            Action::GotoTop => &["g", "home"],
            Action::GotoBottom => &["G", "end"],
            Action::NextFile => &["tab", "l", "right"],
            Action::PrevFile => &["backtab", "h", "left"],
            Action::NextHunk => &["]h", "space"],
            Action::PrevHunk => &["[h"],
            Action::NextNote => &["}"],
            Action::PrevNote => &["{"],
            Action::FoldFile => &["zc"],
            Action::UnfoldFile => &["zo"],
            Action::ToggleContextCollapse => &["zx"],
            Action::ToggleHighlight => &["H"],
            Action::ToggleLineNumbers => &["#"],
            Action::ToggleWordDiff => &["w"],
            Action::ToggleIgnoreWhitespace => &["W"],
            Action::ToggleWrap => &["zw"],
            Action::ToggleRail => &["b"],
            Action::CycleLayout => &["L"],
            Action::CycleThemeMode => &["t"],
            Action::CyclePalette => &["T"],
            Action::Search => &["/"],
            Action::FilterPaths => &["f"],
            Action::ComposeNote => &["c"],
            Action::OpenEditor => &["o"],
            Action::NextMatch => &["n"],
            Action::PrevMatch => &["N"],
            Action::Help => &["?"],
            Action::AcceptHunk => &["a"],
            Action::RejectHunk => &["r"],
            Action::UndecideHunk => &["u"],
        }
    }
}

/// A resolved key spec: an optional sequence prefix plus the final key.
/// Sequences are exactly two keys (`]h`, `zc`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub prefix: Option<char>,
    pub ctrl: bool,
    pub code: KeyCode,
}

impl KeySpec {
    /// Parse a spec like `"q"`, `"ctrl-d"`, `"pageup"`, or `"]h"` (a
    /// two-key sequence). Resolution order: `ctrl-<char>`, named key or
    /// single char, then two-key sequence.
    pub fn parse(spec: &str) -> Option<KeySpec> {
        let spec = spec.trim();
        if spec.is_empty() {
            return None;
        }
        let lower = spec.to_lowercase();
        if let Some(rest) = lower.strip_prefix("ctrl-") {
            let mut chars = rest.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None; // ctrl-<single char> only
            }
            return Some(KeySpec {
                prefix: None,
                ctrl: true,
                code: KeyCode::Char(c),
            });
        }
        // Named key ("pageup") or a single char ("q", "Q", "⌘"…).
        if let Some(code) = parse_named_key(spec) {
            return Some(KeySpec {
                prefix: None,
                ctrl: false,
                code,
            });
        }
        // Two-key sequence: prefix char + single second key ("]h", "zx").
        let mut chars = spec.chars();
        let first = chars.next()?;
        let rest: String = chars.collect();
        let second = parse_named_key(&rest)?;
        Some(KeySpec {
            prefix: Some(first),
            ctrl: false,
            code: second,
        })
    }

    /// Human-readable form for the help overlay / conflict warnings.
    pub fn display(&self) -> String {
        let key = match self.code {
            KeyCode::Char(' ') if !self.ctrl => "SPC".to_string(),
            KeyCode::Char(c) => {
                if self.ctrl {
                    format!("ctrl-{c}")
                } else {
                    c.to_string()
                }
            }
            KeyCode::Esc => "esc".into(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "backtab".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Insert => "insert".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::F(n) => format!("f{n}"),
            _ => "…".into(),
        };
        match self.prefix {
            Some(p) => format!("{p}{key}"),
            None => key,
        }
    }
}

fn parse_named_key(s: &str) -> Option<KeyCode> {
    // Named keys match case-insensitively; a lone char keeps its case
    // (`q` and `Q` are different keys).
    Some(match s.to_lowercase().as_str() {
        "esc" => KeyCode::Esc,
        "enter" | "return" => KeyCode::Enter,
        "space" | "spacebar" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "backtab" | "shift-tab" | "btab" => KeyCode::BackTab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),
        _ => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(c)
        }
    })
}

/// The resolved keymap: key spec → action, plus the reverse index for the
/// help overlay. Warnings collected during config application are replayed
/// to stderr by the caller (the TUI owns the screen by then).
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    /// Fully-resolved bindings (including sequences).
    bindings: HashMap<KeySpec, Action>,
}

impl Keymap {
    /// The default bindings — identical to the pre-config hardcoded keys.
    pub fn default_map() -> Keymap {
        let mut km = Keymap::default();
        for action in all_actions() {
            for spec in action.default_specs() {
                if let Some(k) = KeySpec::parse(spec) {
                    km.claim(k, *action);
                }
            }
        }
        km
    }

    /// Claim a key for an action (defaults pass). First claim wins; later
    /// conflicting claims return the holder (no warning — defaults are
    /// compile-time and known-good).
    fn claim(&mut self, spec: KeySpec, action: Action) -> Option<Action> {
        if let Some(&held) = self.bindings.get(&spec) {
            if held != action {
                return Some(held);
            }
        }
        self.bindings.insert(spec, action);
        None
    }

    /// Apply `[keybindings]` overrides on top of the defaults. An override
    /// fully replaces the action's default keys (`false` unbinds). Explicit
    /// overrides may steal a key from another action's *defaults* (warned);
    /// two overrides fighting over one key resolve first-wins (warned).
    /// Returns warnings to print (unknown names, bad specs, conflicts).
    pub fn with_overrides(overrides: &HashMap<String, toml::Value>) -> (Keymap, Vec<String>) {
        let mut km = Keymap::default_map();
        let mut warnings = Vec::new();

        // Collect per-action override specs in deterministic (all_actions)
        // order; drop each overridden action's existing bindings first so
        // overrides replace rather than merge.
        let mut applied: Vec<(Action, Vec<KeySpec>)> = Vec::new();
        for action in all_actions() {
            let Some(value) = overrides.get(action.name()) else {
                continue;
            };
            let specs: Vec<String> = match value {
                toml::Value::Boolean(false) => Vec::new(),
                toml::Value::Boolean(true) => action
                    .default_specs()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                toml::Value::String(s) => vec![s.clone()],
                toml::Value::Array(items) => items
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect(),
                other => {
                    warnings.push(format!(
                        "keybindings.{}: expected key list or false, got {other:?} — ignored",
                        action.name()
                    ));
                    continue;
                }
            };
            let mut parsed = Vec::new();
            for spec in specs {
                match KeySpec::parse(&spec) {
                    Some(k) => parsed.push(k),
                    None => warnings.push(format!(
                        "keybindings.{}: cannot parse key `{spec}` — ignored",
                        action.name()
                    )),
                }
            }
            if !matches!(value, toml::Value::Boolean(false)) && parsed.is_empty() {
                // Garbage-in (e.g. `quit = [12345]`): silently unbinding an
                // action would strand the user without the key. Only an
                // explicit `false` unbinds; keep the defaults instead.
                warnings.push(format!(
                    "keybindings.{}: no valid keys — keeping defaults",
                    action.name()
                ));
                continue;
            }
            km.bindings.retain(|_, a| *a != *action);
            applied.push((*action, parsed));
        }

        // Claim keys in order. A key still held by a non-overridden action's
        // default is stolen (with a warning); a key claimed by an earlier
        // override stays with the earlier claim.
        let mut override_claims: HashMap<KeySpec, Action> = HashMap::new();
        for (action, specs) in applied {
            for k in specs {
                if let Some(&earlier) = override_claims.get(&k) {
                    warnings.push(format!(
                        "keybindings: `{}` is claimed by both `{}` and `{}` — keeping `{}`",
                        k.display(),
                        earlier.name(),
                        action.name(),
                        earlier.name()
                    ));
                    continue;
                }
                if let Some(&held) = km.bindings.get(&k) {
                    warnings.push(format!(
                        "keybindings: `{}` takes `{}` from `{}`",
                        action.name(),
                        k.display(),
                        held.name()
                    ));
                }
                km.bindings.insert(k.clone(), action);
                override_claims.insert(k, action);
            }
        }

        for name in overrides.keys() {
            if !all_actions().iter().any(|a| a.name() == name) {
                warnings.push(format!("keybindings.{name}: unknown action — ignored"));
            }
        }
        (km, warnings)
    }

    /// Resolve a single-key event (no pending prefix).
    pub fn lookup(&self, key: &KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        self.bindings
            .get(&KeySpec {
                prefix: None,
                ctrl,
                code: key.code,
            })
            .copied()
    }

    /// Resolve the second key of a sequence armed by `prefix`.
    pub fn lookup_sequence(&self, prefix: char, key: &KeyEvent) -> Option<Action> {
        self.bindings
            .get(&KeySpec {
                prefix: Some(prefix),
                ctrl: false,
                code: key.code,
            })
            .copied()
    }

    /// Does any sequence start with this key (i.e. should pressing it arm a
    /// pending prefix instead of falling through)?
    pub fn arms_prefix(&self, c: char) -> bool {
        self.bindings.keys().any(|k| k.prefix == Some(c) && !k.ctrl)
    }

    /// All keys bound to an action, for the help overlay. Ordered by the
    /// action's default-spec order where possible (HashMap iteration is
    /// otherwise arbitrary and would reshuffle the help on every launch).
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for spec in action.default_specs() {
            if let Some(k) = KeySpec::parse(spec) {
                if self.bindings.get(&k) == Some(&action) {
                    out.push(k.display());
                }
            }
        }
        // Custom keys not in the default list (remapped) go last, sorted.
        let mut extra: Vec<String> = self
            .bindings
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(k, _)| k.display())
            .filter(|d| !out.contains(d))
            .collect();
        extra.sort();
        out.extend(extra);
        out
    }
}

/// Every action, in help-overlay order.
pub fn all_actions() -> &'static [Action] {
    &[
        Action::CursorDown,
        Action::CursorUp,
        Action::HalfPageDown,
        Action::HalfPageUp,
        Action::PageForward,
        Action::PageBackward,
        Action::GotoTop,
        Action::GotoBottom,
        Action::NextFile,
        Action::PrevFile,
        Action::NextHunk,
        Action::PrevHunk,
        Action::NextNote,
        Action::PrevNote,
        Action::FoldFile,
        Action::UnfoldFile,
        Action::ToggleContextCollapse,
        Action::ToggleHighlight,
        Action::ToggleLineNumbers,
        Action::ToggleWordDiff,
        Action::ToggleIgnoreWhitespace,
        Action::ToggleWrap,
        Action::ToggleRail,
        Action::CycleLayout,
        Action::CycleThemeMode,
        Action::CyclePalette,
        Action::Search,
        Action::FilterPaths,
        Action::ComposeNote,
        Action::OpenEditor,
        Action::NextMatch,
        Action::PrevMatch,
        Action::Help,
        Action::Quit,
        Action::Cancel,
        Action::AcceptHunk,
        Action::RejectHunk,
        Action::UndecideHunk,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn defaults_match_the_builtin_keys() {
        let km = Keymap::default_map();
        assert_eq!(
            km.lookup(&key(KeyCode::Char('j'))),
            Some(Action::CursorDown)
        );
        assert_eq!(km.lookup(&key(KeyCode::Down)), Some(Action::CursorDown));
        assert_eq!(km.lookup(&key(KeyCode::Char('q'))), Some(Action::Quit));
        assert_eq!(km.lookup(&key(KeyCode::Esc)), Some(Action::Cancel));
        assert_eq!(km.lookup(&key(KeyCode::Tab)), Some(Action::NextFile));
        assert_eq!(km.lookup(&key(KeyCode::Char(' '))), Some(Action::NextHunk));
        assert_eq!(km.lookup(&ctrl('d')), Some(Action::HalfPageDown));
        assert_eq!(km.lookup(&ctrl('f')), Some(Action::PageForward));
        assert_eq!(km.lookup(&key(KeyCode::Char('?'))), Some(Action::Help));
        // sequences resolve only through lookup_sequence
        assert_eq!(km.lookup(&key(KeyCode::Char(']'))), None);
        assert!(km.arms_prefix(']'));
        assert!(km.arms_prefix('z'));
        assert!(!km.arms_prefix('x'));
        assert_eq!(
            km.lookup_sequence(']', &key(KeyCode::Char('h'))),
            Some(Action::NextHunk)
        );
        assert_eq!(
            km.lookup_sequence('z', &key(KeyCode::Char('c'))),
            Some(Action::FoldFile)
        );
        assert_eq!(
            km.lookup_sequence('z', &key(KeyCode::Char('x'))),
            Some(Action::ToggleContextCollapse)
        );
        assert_eq!(
            km.lookup_sequence('z', &key(KeyCode::Char('w'))),
            Some(Action::ToggleWrap)
        );
    }

    #[test]
    fn half_page_defaults_cover_the_three_builtin_keys() {
        // J, PageDown, ctrl-d were three separate arms with identical bodies
        // — they must stay one action with all three keys.
        let km = Keymap::default_map();
        assert_eq!(
            km.lookup(&key(KeyCode::Char('J'))),
            Some(Action::HalfPageDown)
        );
        assert_eq!(
            km.lookup(&key(KeyCode::PageDown)),
            Some(Action::HalfPageDown)
        );
        assert_eq!(km.lookup(&ctrl('d')), Some(Action::HalfPageDown));
        assert_eq!(
            km.lookup(&key(KeyCode::Char('K'))),
            Some(Action::HalfPageUp)
        );
        assert_eq!(km.lookup(&key(KeyCode::PageUp)), Some(Action::HalfPageUp));
        assert_eq!(km.lookup(&ctrl('u')), Some(Action::HalfPageUp));
    }

    #[test]
    fn spec_parse_round_trip() {
        assert_eq!(
            KeySpec::parse("q"),
            Some(KeySpec {
                prefix: None,
                ctrl: false,
                code: KeyCode::Char('q')
            })
        );
        assert_eq!(
            KeySpec::parse("ctrl-d"),
            Some(KeySpec {
                prefix: None,
                ctrl: true,
                code: KeyCode::Char('d')
            })
        );
        assert_eq!(
            KeySpec::parse("]h"),
            Some(KeySpec {
                prefix: Some(']'),
                ctrl: false,
                code: KeyCode::Char('h')
            })
        );
        assert_eq!(
            KeySpec::parse("pageup"),
            Some(KeySpec {
                prefix: None,
                ctrl: false,
                code: KeyCode::PageUp
            })
        );
        assert_eq!(KeySpec::parse(""), None);
        assert_eq!(KeySpec::parse("nope"), None);
        assert_eq!(KeySpec::parse("ctrl-xy"), None);
    }

    #[test]
    fn overrides_remap_and_unbind() {
        let mut cfg = HashMap::new();
        cfg.insert("quit".to_string(), toml::Value::String("Q".into()));
        cfg.insert("help".to_string(), toml::Value::Boolean(false));
        let (km, warns) = Keymap::with_overrides(&cfg);
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(km.lookup(&key(KeyCode::Char('Q'))), Some(Action::Quit));
        assert_eq!(km.lookup(&key(KeyCode::Char('q'))), None);
        assert_eq!(km.lookup(&key(KeyCode::Char('?'))), None, "help unbound");
        // unbound action's keys_for is empty (help overlay omits it)
        assert!(km.keys_for(Action::Help).is_empty());
    }

    #[test]
    fn override_replaces_not_merges() {
        // Remapping next_file to "N" must drop tab/l/right — otherwise the
        // old keys stay live as zombies.
        let mut cfg = HashMap::new();
        cfg.insert("next_file".to_string(), toml::Value::String("N".into()));
        let (km, _) = Keymap::with_overrides(&cfg);
        assert_eq!(km.lookup(&key(KeyCode::Char('N'))), Some(Action::NextFile));
        assert_eq!(km.lookup(&key(KeyCode::Tab)), None);
        assert_eq!(km.lookup(&key(KeyCode::Char('l'))), None);
    }

    #[test]
    fn conflicting_overrides_warn_and_first_in_order_wins() {
        let mut cfg = HashMap::new();
        cfg.insert(
            "quit".to_string(),
            toml::Value::Array(vec![toml::Value::String("q".into())]),
        );
        cfg.insert(
            "help".to_string(),
            toml::Value::String("q".into()), // fights quit for the key
        );
        let (km, warns) = Keymap::with_overrides(&cfg);
        assert_eq!(warns.len(), 1, "{warns:?}");
        // `help` precedes `quit` in the deterministic all_actions order, so
        // help keeps the key and quit's claim is the warned loser.
        assert_eq!(km.lookup(&key(KeyCode::Char('q'))), Some(Action::Help));
    }

    #[test]
    fn override_steals_a_default_key_with_warning() {
        let mut cfg = HashMap::new();
        // j is cursor_down's default; giving it to help must steal it.
        cfg.insert("help".to_string(), toml::Value::String("j".into()));
        let (km, warns) = Keymap::with_overrides(&cfg);
        assert!(
            warns
                .iter()
                .any(|w| w.contains("takes `j` from `cursor_down`")),
            "{warns:?}"
        );
        assert_eq!(km.lookup(&key(KeyCode::Char('j'))), Some(Action::Help));
        assert_eq!(km.lookup(&key(KeyCode::Down)), Some(Action::CursorDown));
    }

    #[test]
    fn unknown_action_and_bad_spec_warn() {
        let mut cfg = HashMap::new();
        cfg.insert("teleport".to_string(), toml::Value::String("x".into()));
        cfg.insert("search".to_string(), toml::Value::String("nope".into()));
        let (_km, warns) = Keymap::with_overrides(&cfg);
        assert!(warns.iter().any(|w| w.contains("teleport")), "{warns:?}");
        assert!(warns.iter().any(|w| w.contains("search")), "{warns:?}");
    }

    #[test]
    fn remap_sequence_works() {
        let mut cfg = HashMap::new();
        cfg.insert("next_hunk".to_string(), toml::Value::String("]j".into()));
        let (km, _) = Keymap::with_overrides(&cfg);
        assert_eq!(
            km.lookup_sequence(']', &key(KeyCode::Char('j'))),
            Some(Action::NextHunk)
        );
        assert_eq!(km.lookup_sequence(']', &key(KeyCode::Char('h'))), None);
        // space (the other default next_hunk key) was replaced too
        assert_eq!(km.lookup(&key(KeyCode::Char(' '))), None);
    }

    #[test]
    fn keys_for_lists_custom_keys_after_defaults() {
        let mut cfg = HashMap::new();
        cfg.insert(
            "cursor_down".to_string(),
            toml::Value::Array(vec![
                toml::Value::String("j".into()),
                toml::Value::String("ctrl-n".into()),
            ]),
        );
        let (km, _) = Keymap::with_overrides(&cfg);
        let keys = km.keys_for(Action::CursorDown);
        assert_eq!(keys, vec!["j".to_string(), "ctrl-n".to_string()]);
    }
}
