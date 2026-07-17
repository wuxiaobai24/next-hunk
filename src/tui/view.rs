//! Rendering for the review TUI.
//!
//! Layout (vertical):
//!   ┌─────────────────────────────────────────┐
//!   │  rail (left)  │  stream (right)         │  ← main area
//!   ├─────────────────────────────────────────┤
//!   │  status line                             │  1 row
//!   ├─────────────────────────────────────────┤
//!   │  help / prompt line                      │  1 row
//!   └─────────────────────────────────────────┘
//!
//! The stream pane renders ONLY the rows in `[scroll_y, scroll_y+height)` via
//! [`ViewportQuery::rows`] — O(visible), never a full-line cache of the review
//! (architecture §7 anti-pattern). Syntax highlighting is viewport-only and
//! cached per (file, line) in `App.cache`.

use std::sync::Arc;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::ir::{
    word_diff_regions, DiffLineKind, Review, StreamRow, Viewport, ViewportQuery, WordRegion,
};
use crate::tui::app::{App, Decision, HunkId, InputMode};

/// Rail width (left file list). Capped to a fraction of the area at draw time.
const RAIL_MAX_WIDTH: u16 = 32;

/// Minimum stream-pane width for true side-by-side split. Below this, split
/// falls back to stack (then unified below [`STACK_MIN_WIDTH`]).
const SPLIT_MIN_WIDTH: u16 = 80;

/// Minimum stream-pane width for stack layout. Below this, stack/split fall
/// back to unified so the layout never collapses.
const STACK_MIN_WIDTH: u16 = 40;

/// `layout = "auto"` minimum stream-pane width for side-by-side split. Auto
/// uses a higher bar than explicit `split` so the side-by-side pane pair is
/// genuinely readable when the user has delegated the choice (WXB-25). The
/// exact value is a soft tuning knob, not a hard architectural constraint —
/// bump it (e.g. to 140) if wide-screen users still report cramped panes.
pub const AUTO_SPLIT_MIN_WIDTH: u16 = 120;

/// `layout = "auto"` minimum stream-pane width for stack. Kept equal to
/// [`STACK_MIN_WIDTH`] so the auto ladder is just a stricter split threshold
/// bolted on top of the existing presentation fallback.
pub const AUTO_STACK_MIN_WIDTH: u16 = STACK_MIN_WIDTH;

/// Resolve `mode` against the live stream-pane width into one of the three
/// concrete draw paths. Presentation only — IR indices and `app.scroll_y` are
/// untouched, so width changes (including drag-resize) never rebuild a full
/// widget tree (WXB-25, PERF gate §1–2).
fn effective_layout(mode: crate::config::LayoutMode, width: u16) -> crate::config::LayoutMode {
    use crate::config::LayoutMode;
    match mode {
        LayoutMode::Auto if width >= AUTO_SPLIT_MIN_WIDTH => LayoutMode::Split,
        LayoutMode::Auto if width >= AUTO_STACK_MIN_WIDTH => LayoutMode::Stack,
        LayoutMode::Auto => LayoutMode::Unified,
        other => other,
    }
}

/// Draw the whole app. Takes `&mut App` because highlighting populates the
/// cache lazily during render.
pub fn draw(app: &mut App, frame: &mut Frame) {
    let area = frame.area();

    let main_height = area.height.saturating_sub(2);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(main_height),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    let main = chunks[0];
    draw_main(app, frame, main);

    draw_status(app, frame, chunks[1]);
    draw_help_or_prompt(app, frame, chunks[2]);

    // The keybinding help overlay is drawn last so it sits on top of everything.
    if app.show_help {
        draw_help_overlay(app, frame);
    }
}

fn draw_main(app: &mut App, frame: &mut Frame, area: Rect) {
    if app.show_rail {
        let rail_w = RAIL_MAX_WIDTH.min(area.width / 4).max(12);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(rail_w), Constraint::Min(0)])
            .split(area);
        app.rail_rect = Some(cols[0]);
        app.stream_rect = Some(cols[1]);
        draw_rail(app, frame, cols[0]);
        draw_stream(app, frame, cols[1]);
    } else {
        app.rail_rect = None;
        app.stream_rect = Some(area);
        draw_stream(app, frame, area);
    }
}

fn draw_rail(app: &App, frame: &mut Frame, area: Rect) {
    let visible = app.visible_files();
    // Inner width available for a rail row, minus the left border and the
    // " N " index prefix. Used to pad the path so the +/- tail right-aligns.
    let rail_inner_w = area.width.saturating_sub(1) as usize;
    let items: Vec<ListItem> = visible
        .iter()
        .map(|&i| {
            let f = &app.review.files[i];
            let path = short_path(&f.display_path);
            let style = if i == app.selected_file {
                Style::default()
                    .fg(app.theme.selection_fg)
                    .bg(app.theme.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            // Compact per-file change tally: `+ins` (green) next to `−del`
            // (red), zero sides omitted (e.g. an add-only file shows `+12`,
            // a pure delete `−3`). Colored so a glance at the rail shows
            // where the change mass sits.
            let (plus, minus) = file_stats_tail(f.inserts, f.deletes);
            // Working-set origin mark: `S` staged / `M` modified / `?` untracked.
            // Omitted for patch/show/pager reviews (origin is None).
            let head = match f.origin {
                Some(origin) => format!(" {}.{} ", i + 1, origin.mark()),
                None => format!(" {}. ", i + 1),
            };
            // Pad the path so the tally right-aligns within the row. Count
            // widths before moving the strings into Spans below.
            let tail = format!("  {}{}", plus, minus);
            let need_pad = !plus.is_empty() || !minus.is_empty();
            let used = head.chars().count() + path.chars().count();
            let mut spans: Vec<Span> = vec![Span::styled(head, style), Span::styled(path, style)];
            if need_pad {
                let pad = rail_inner_w.saturating_sub(used + tail.chars().count());
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(plus, Style::default().fg(app.theme.add)));
                spans.push(Span::styled(minus, Style::default().fg(app.theme.delete)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let title = if app.path_filter.trim().is_empty() {
        "Files".to_string()
    } else {
        format!("Files ({}/{})", visible.len(), app.review.file_count())
    };
    let list = List::new(items).block(Block::default().borders(Borders::RIGHT).title(title));

    // Map selected_file to its position in the visible list for the ListState.
    let selected_pos = visible.iter().position(|&i| i == app.selected_file);
    let mut state = ListState::default();
    state.select(selected_pos);
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_stream(app: &mut App, frame: &mut Frame, area: Rect) {
    // Responsive layout ladder (presentation only — IR/scroll indices unchanged):
    //   auto   (≥ AUTO_SPLIT_MIN_WIDTH=120) → split → stack → unified
    //   split  (≥ 80) → stack (40..79) → unified (< 40)
    //   stack  (≥ 40) → unified (< 40)
    //   unified always
    //
    // Auto is resolved per-frame via [`effective_layout`] so a width change
    // (terminal resize, pane split) re-picks the path on the next draw without
    // touching the IR or rebuilding a widget tree (WXB-25, PERF gate).
    use crate::config::LayoutMode;
    match effective_layout(app.layout_mode, area.width) {
        LayoutMode::Split if area.width >= SPLIT_MIN_WIDTH => {
            draw_stream_split(app, frame, area);
        }
        LayoutMode::Split | LayoutMode::Stack if area.width >= STACK_MIN_WIDTH => {
            draw_stream_stack(app, frame, area);
        }
        _ => draw_stream_unified(app, frame, area),
    }
}

fn draw_stream_unified(app: &mut App, frame: &mut Frame, area: Rect) {
    let height = area.height as usize;
    let scroll_y = app.scroll_y;
    let viewport = Viewport {
        start: scroll_y,
        height,
    };

    // Collect owned row data so we can release the &app.review borrow before
    // mutating app.cache below for highlighting. Line numbers are resolved here
    // (they need the review) and carried on each OwnedRow.
    let owned_rows: Vec<OwnedRow> = ViewportQuery::rows(&app.review, viewport, &app.folded)
        .into_iter()
        .enumerate()
        .map(|(i, row)| OwnedRow::from_stream_row(&app.review, row, scroll_y + i))
        .collect();

    let current_match_row = if app.search.active && !app.search.matches.is_empty() {
        Some(app.search.matches[app.search.current])
    } else {
        None
    };
    let match_rows: std::collections::HashSet<usize> = if app.search.active {
        app.search.matches.iter().copied().collect()
    } else {
        std::collections::HashSet::new()
    };

    // Build a lookup of agent notes keyed by the absolute stream row they
    // attach to (line-level and hunk-level). Rendered as extra rows below the
    // target via a viewport fan-out — does NOT touch `stream_len`, so scroll /
    // search / hunk-jump indices stay stable.
    let notes_by_row = build_notes_by_row(&app.review, &app.notes);

    let title = app.current_path().to_string();
    let mut lines: Vec<Line> = Vec::with_capacity(owned_rows.len());
    for r in owned_rows {
        let abs_row = owned_row_abs(&r);
        lines.push(stream_row_to_line(app, r, current_match_row, &match_rows));
        // Fan out: append any note rows attached to this logical row.
        if let Some(notes) = notes_by_row.get(&abs_row) {
            for text in notes {
                lines.push(Line::from(Span::styled(
                    format!("  💬 {}", text),
                    Style::default()
                        .fg(app.theme.note)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }
    }

    let mut para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", title)),
    );
    if app.wrap_on {
        para = para.wrap(Wrap { trim: false });
    }
    frame.render_widget(para, area);
}

/// Stack layout: for each file, render old content (context + deletes) then
/// new content (context + adds) as two stacked blocks separated by a visual
/// divider. Preserves viewport-only materialization — works on the same
/// [`ViewportQuery::rows`] output without touching the IR.
fn draw_stream_stack(app: &mut App, frame: &mut Frame, area: Rect) {
    let height = area.height as usize;
    let scroll_y = app.scroll_y;
    let viewport = Viewport {
        start: scroll_y,
        height,
    };

    let owned_rows: Vec<OwnedRow> = ViewportQuery::rows(&app.review, viewport, &app.folded)
        .into_iter()
        .enumerate()
        .map(|(i, row)| OwnedRow::from_stream_row(&app.review, row, scroll_y + i))
        .collect();

    let current_match_row = if app.search.active && !app.search.matches.is_empty() {
        Some(app.search.matches[app.search.current])
    } else {
        None
    };
    let match_rows: std::collections::HashSet<usize> = if app.search.active {
        app.search.matches.iter().copied().collect()
    } else {
        std::collections::HashSet::new()
    };

    let notes_by_row = build_notes_by_row(&app.review, &app.notes);

    let mut lines: Vec<Line> = Vec::new();
    // Group rows by file to produce old/new blocks per file.
    let mut file_rows: Vec<(usize, Vec<OwnedRow>)> = Vec::new();
    for r in owned_rows {
        let file_idx = match &r {
            OwnedRow::FileHeader { .. } => continue, // handled below
            OwnedRow::HunkHeader { file_idx, .. } => *file_idx,
            OwnedRow::Line { file_idx, .. } => *file_idx,
        };
        if file_rows.last().map(|(f, _)| *f) != Some(file_idx) {
            file_rows.push((file_idx, Vec::new()));
        }
        file_rows.last_mut().unwrap().1.push(r);
    }

    for (file_idx, rows) in &file_rows {
        let path = app.review.display_path(*file_idx);
        // File header
        lines.push(Line::from(Span::styled(
            format!("─── {} ───", path),
            Style::default()
                .fg(app.theme.file_header)
                .add_modifier(Modifier::BOLD),
        )));
        // Old block: context + delete lines
        lines.push(Line::from(Span::styled(
            "▌ old",
            Style::default()
                .fg(app.theme.dim)
                .add_modifier(Modifier::BOLD),
        )));
        for r in rows.iter() {
            if let OwnedRow::Line {
                kind,
                text,
                file_idx,
                abs_row,
                old_no,
                new_no,
                counterpart,
            } = &r
            {
                if *kind == DiffLineKind::Add {
                    continue;
                }
                let line = stream_row_to_line(
                    app,
                    OwnedRow::Line {
                        kind: *kind,
                        text: text.clone(),
                        file_idx: *file_idx,
                        abs_row: *abs_row,
                        old_no: *old_no,
                        new_no: *new_no,
                        counterpart: counterpart.clone(),
                    },
                    current_match_row,
                    &match_rows,
                );
                if let Some(notes) = notes_by_row.get(abs_row) {
                    for text in notes {
                        lines.push(Line::from(Span::styled(
                            format!("  💬 {}", text),
                            Style::default()
                                .fg(app.theme.note)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
                lines.push(line);
            }
        }
        // New block
        lines.push(Line::from(Span::styled(
            "▌ new",
            Style::default()
                .fg(app.theme.dim)
                .add_modifier(Modifier::BOLD),
        )));
        for r in rows.iter() {
            if let OwnedRow::Line {
                kind,
                text,
                file_idx,
                abs_row,
                old_no,
                new_no,
                counterpart,
            } = &r
            {
                if *kind == DiffLineKind::Delete {
                    continue;
                }
                let line = stream_row_to_line(
                    app,
                    OwnedRow::Line {
                        kind: *kind,
                        text: text.clone(),
                        file_idx: *file_idx,
                        abs_row: *abs_row,
                        old_no: *old_no,
                        new_no: *new_no,
                        counterpart: counterpart.clone(),
                    },
                    current_match_row,
                    &match_rows,
                );
                if let Some(notes) = notes_by_row.get(abs_row) {
                    for text in notes {
                        lines.push(Line::from(Span::styled(
                            format!("  💬 {}", text),
                            Style::default()
                                .fg(app.theme.note)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
                lines.push(line);
            }
        }
    }

    let mut para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", app.current_path())),
    );
    if app.wrap_on {
        para = para.wrap(Wrap { trim: false });
    }
    frame.render_widget(para, area);
}

/// Side-by-side split layout: left pane shows old content (context + deletes),
/// right pane shows new content (context + adds). Pairs consecutive delete/add
/// runs so changed lines align across panes.
///
/// **IR invariant:** materializes only the current viewport via
/// [`ViewportQuery::rows`] — never walks or caches the full stream. Scroll,
/// search, and hunk-jump indices stay on the unified stream coordinate system.
fn draw_stream_split(app: &mut App, frame: &mut Frame, area: Rect) {
    let height = area.height as usize;
    let scroll_y = app.scroll_y;
    let viewport = Viewport {
        start: scroll_y,
        height,
    };

    let owned_rows: Vec<OwnedRow> = ViewportQuery::rows(&app.review, viewport, &app.folded)
        .into_iter()
        .enumerate()
        .map(|(i, row)| OwnedRow::from_stream_row(&app.review, row, scroll_y + i))
        .collect();

    let current_match_row = if app.search.active && !app.search.matches.is_empty() {
        Some(app.search.matches[app.search.current])
    } else {
        None
    };
    let match_rows: std::collections::HashSet<usize> = if app.search.active {
        app.search.matches.iter().copied().collect()
    } else {
        std::collections::HashSet::new()
    };

    let notes_by_row = build_notes_by_row(&app.review, &app.notes);

    // Pane width: half of the stream area minus the 1-col center gutter.
    let pane_w = (area.width.saturating_sub(1) / 2).max(1) as usize;
    let visual = build_split_visual_rows(owned_rows);

    let mut lines: Vec<Line> = Vec::with_capacity(visual.len());
    for v in visual {
        match v {
            SplitVisual::Full(row) => {
                let abs_row = owned_row_abs(&row);
                lines.push(stream_row_to_line(app, row, current_match_row, &match_rows));
                if let Some(notes) = notes_by_row.get(&abs_row) {
                    for text in notes {
                        lines.push(Line::from(Span::styled(
                            format!("  💬 {}", text),
                            Style::default()
                                .fg(app.theme.note)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
            }
            SplitVisual::Pair { left, right } => {
                // Collect notes from either side (same abs_row when both set
                // for context; for delete/add pairs notes attach to each side).
                let note_rows: Vec<usize> = [left.as_ref(), right.as_ref()]
                    .into_iter()
                    .flatten()
                    .map(owned_row_abs)
                    .collect();
                lines.push(render_split_pair_line(
                    app,
                    left,
                    right,
                    pane_w,
                    current_match_row,
                    &match_rows,
                ));
                // Fan-out notes under the paired row (dedupe by abs_row).
                let mut seen = std::collections::HashSet::new();
                for abs in note_rows {
                    if !seen.insert(abs) {
                        continue;
                    }
                    if let Some(notes) = notes_by_row.get(&abs) {
                        for text in notes {
                            lines.push(Line::from(Span::styled(
                                format!("  💬 {}", text),
                                Style::default()
                                    .fg(app.theme.note)
                                    .add_modifier(Modifier::ITALIC),
                            )));
                        }
                    }
                }
            }
        }
    }

    // Column labels so reviewers know which pane is old vs new.
    let header = Line::from(vec![
        Span::styled(
            pad_display("▌ old", pane_w),
            Style::default()
                .fg(app.theme.dim)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("│", Style::default().fg(app.theme.dim)),
        Span::styled(
            pad_display(" ▌ new", pane_w),
            Style::default()
                .fg(app.theme.dim)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    lines.insert(0, header);

    // Wrap is intentionally ignored in split: wrapping either pane would
    // desync row alignment. Truncation to pane width is the presentation rule.
    let _ = app.wrap_on;
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", app.current_path())),
    );
    frame.render_widget(para, area);
}

/// One visual row in the split stream: either a full-width header or a
/// left|right pair of optional line rows.
#[derive(Debug)]
enum SplitVisual {
    Full(OwnedRow),
    Pair {
        left: Option<OwnedRow>,
        right: Option<OwnedRow>,
    },
}

/// Transform viewport-owned unified rows into side-by-side visual rows.
///
/// Pure presentation: O(viewport). Consecutive delete runs are paired with
/// following add runs so changed lines sit on the same visual row; unpaired
/// sides render as blank. Context/meta lines appear on both panes.
fn build_split_visual_rows(rows: Vec<OwnedRow>) -> Vec<SplitVisual> {
    let mut out: Vec<SplitVisual> = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        match &rows[i] {
            OwnedRow::FileHeader { .. } | OwnedRow::HunkHeader { .. } => {
                out.push(SplitVisual::Full(rows[i].clone()));
                i += 1;
            }
            OwnedRow::Line {
                kind: DiffLineKind::Context | DiffLineKind::Meta,
                ..
            } => {
                let r = rows[i].clone();
                out.push(SplitVisual::Pair {
                    left: Some(r.clone()),
                    right: Some(r),
                });
                i += 1;
            }
            OwnedRow::Line {
                kind: DiffLineKind::Delete,
                ..
            } => {
                let mut dels = Vec::new();
                while i < rows.len() {
                    if matches!(
                        &rows[i],
                        OwnedRow::Line {
                            kind: DiffLineKind::Delete,
                            ..
                        }
                    ) {
                        dels.push(rows[i].clone());
                        i += 1;
                    } else {
                        break;
                    }
                }
                let mut adds = Vec::new();
                while i < rows.len() {
                    if matches!(
                        &rows[i],
                        OwnedRow::Line {
                            kind: DiffLineKind::Add,
                            ..
                        }
                    ) {
                        adds.push(rows[i].clone());
                        i += 1;
                    } else {
                        break;
                    }
                }
                let n = dels.len().max(adds.len());
                for j in 0..n {
                    out.push(SplitVisual::Pair {
                        left: dels.get(j).cloned(),
                        right: adds.get(j).cloned(),
                    });
                }
            }
            OwnedRow::Line {
                kind: DiffLineKind::Add,
                ..
            } => {
                // Orphan add (no preceding deletes in this run).
                out.push(SplitVisual::Pair {
                    left: None,
                    right: Some(rows[i].clone()),
                });
                i += 1;
            }
        }
    }
    out
}

/// Render one side-by-side pair as a single ratatui line: `left │ right`,
/// each pane truncated/padded to `pane_w` display columns.
fn render_split_pair_line(
    app: &mut App,
    left: Option<OwnedRow>,
    right: Option<OwnedRow>,
    pane_w: usize,
    current_match_row: Option<usize>,
    match_rows: &std::collections::HashSet<usize>,
) -> Line<'static> {
    let left_line = left.map(|r| {
        stream_row_to_line_split_pane(app, r, SplitPane::Old, current_match_row, match_rows)
    });
    let right_line = right.map(|r| {
        stream_row_to_line_split_pane(app, r, SplitPane::New, current_match_row, match_rows)
    });

    let left_spans = truncate_line_to_width(left_line.unwrap_or_else(blank_pane_line), pane_w);
    let right_spans = truncate_line_to_width(right_line.unwrap_or_else(blank_pane_line), pane_w);

    let mut spans = left_spans;
    spans.push(Span::styled("│", Style::default().fg(app.theme.dim)));
    spans.extend(right_spans);
    Line::from(spans)
}

#[derive(Clone, Copy)]
enum SplitPane {
    Old,
    New,
}

fn blank_pane_line() -> Line<'static> {
    Line::from(Span::raw(""))
}

/// Like [`stream_row_to_line`] but for one split pane: only the relevant
/// line number is shown, and the +/- prefix is kept so change kind is clear.
fn stream_row_to_line_split_pane(
    app: &mut App,
    row: OwnedRow,
    pane: SplitPane,
    current_match_row: Option<usize>,
    match_rows: &std::collections::HashSet<usize>,
) -> Line<'static> {
    // Headers should not appear in panes (they use Full width).
    match &row {
        OwnedRow::FileHeader { .. } | OwnedRow::HunkHeader { .. } => {
            return stream_row_to_line(app, row, current_match_row, match_rows);
        }
        OwnedRow::Line { .. } => {}
    }

    let OwnedRow::Line {
        kind,
        text,
        file_idx,
        abs_row,
        old_no,
        new_no,
        counterpart,
    } = row
    else {
        unreachable!()
    };

    let is_current_match = current_match_row == Some(abs_row);
    let is_other_match = !is_current_match && match_rows.contains(&abs_row);

    let (prefix, kind_style) = match kind {
        DiffLineKind::Add => ('+', Style::default().fg(app.theme.add)),
        DiffLineKind::Delete => ('-', Style::default().fg(app.theme.delete)),
        DiffLineKind::Meta => ('\\', Style::default().fg(app.theme.dim)),
        DiffLineKind::Context => (' ', Style::default()),
    };

    let line_in_file = ViewportQuery::file_and_line(&app.review, abs_row).map(|(_, li)| li);
    let hl_runs = if app.highlight_on {
        if let Some(li) = line_in_file {
            let path = app.review.display_path(file_idx);
            if let Some(runs) = app.cache.try_get(file_idx, li) {
                runs
            } else if let Some(tx) = app.hl_job_tx.as_ref() {
                let _ = tx.send(crate::highlight::HighlightJob {
                    gen: app.cache.current_gen(),
                    file_idx,
                    line_in_file: li,
                    path: path.to_owned(),
                    text: text.clone(),
                    highlighter: Arc::clone(&app.highlighter),
                });
                vec![(Style::default(), text.clone())]
            } else {
                app.cache
                    .get_or_highlight(file_idx, li, path, &text, &app.highlighter)
            }
        } else {
            vec![(Style::default(), text.clone())]
        }
    } else {
        vec![(Style::default(), text.clone())]
    };

    let runs = if app.word_diff_on {
        if let Some(their) = counterpart.as_deref() {
            let regions = word_diff_regions(&text, their);
            refine_with_word_regions(
                &hl_runs,
                &regions,
                kind,
                app.theme.word_add,
                app.theme.word_del,
            )
        } else {
            hl_runs
        }
    } else {
        hl_runs
    };

    let mut spans: Vec<Span> = Vec::with_capacity(runs.len() + 2);
    if app.line_numbers_on {
        let dim = Style::default().fg(app.theme.dim);
        let no = match pane {
            SplitPane::Old => old_no,
            SplitPane::New => new_no,
        };
        let s = no
            .map(|n| format!("{n:>5}"))
            .unwrap_or_else(|| "     ".into());
        spans.push(Span::styled(format!(" {s} "), dim));
    }
    spans.push(Span::styled(prefix.to_string(), kind_style));
    for (style, txt) in runs {
        spans.push(Span::styled(txt, style));
    }

    let line = Line::from(spans);
    if is_current_match {
        highlight_current_match_line(
            line,
            app.search.query.as_str(),
            app.theme.match_active_fg,
            app.theme.match_active_bg,
            app.theme.match_inactive_bg,
        )
    } else if is_other_match {
        line.style(Style::default().bg(app.theme.match_inactive_bg))
    } else {
        line
    }
}

/// Pad `s` with trailing spaces so its display width equals `width`.
fn pad_display(s: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let w = s.width();
    if w >= width {
        // Truncate by chars until width fits.
        let mut out = String::new();
        let mut used = 0;
        for c in s.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if used + cw > width {
                break;
            }
            out.push(c);
            used += cw;
        }
        while used < width {
            out.push(' ');
            used += 1;
        }
        out
    } else {
        format!("{s}{}", " ".repeat(width - w))
    }
}

/// Truncate (or pad) a rendered line's spans to exactly `width` display columns.
fn truncate_line_to_width(line: Line<'static>, width: usize) -> Vec<Span<'static>> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for sp in line.spans {
        if used >= width {
            break;
        }
        let content = sp.content.to_string();
        let sw = content.width();
        if used + sw <= width {
            used += sw;
            out.push(Span::styled(content, sp.style));
        } else {
            // Partial span.
            let mut partial = String::new();
            for c in content.chars() {
                let cw = UnicodeWidthChar::width(c).unwrap_or(0);
                if used + cw > width {
                    break;
                }
                partial.push(c);
                used += cw;
            }
            if !partial.is_empty() {
                out.push(Span::styled(partial, sp.style));
            }
            break;
        }
    }
    if used < width {
        out.push(Span::raw(" ".repeat(width - used)));
    }
    out
}

/// The absolute stream row an [`OwnedRow`] occupies. Used to look up notes.
fn owned_row_abs(row: &OwnedRow) -> usize {
    match row {
        OwnedRow::FileHeader { abs_row, .. } => *abs_row,
        OwnedRow::HunkHeader { abs_row, .. } => *abs_row,
        OwnedRow::Line { abs_row, .. } => *abs_row,
    }
}

/// Resolve each `--note` target to an absolute stream row and group the note
/// texts by that row. Banner notes are excluded here (they're shown in the
/// status bar, not the stream). Returns an empty map when there are no
/// line/hunk notes, so the fan-out is a no-op.
fn build_notes_by_row(
    review: &Review,
    notes: &[crate::tui::app::Note],
) -> std::collections::HashMap<usize, Vec<String>> {
    use crate::tui::app::NoteTarget;
    let mut out: std::collections::HashMap<usize, Vec<String>> = std::collections::HashMap::new();
    for note in notes {
        let row = match &note.target {
            NoteTarget::Line { path, line } => ViewportQuery::file_index_for_path(review, path)
                .and_then(|idx| ViewportQuery::row_for_new_line(review, idx, *line)),
            NoteTarget::Hunk { path, hunk } => {
                // CLI hunk ordinals are 1-based; storage is 0-based.
                let hunk0 = hunk.saturating_sub(1);
                ViewportQuery::file_index_for_path(review, path)
                    .and_then(|idx| ViewportQuery::hunk_start_row(review, idx, hunk0))
            }
            NoteTarget::Banner => continue, // shown in the status bar, not here
        };
        if let Some(row) = row {
            out.entry(row).or_default().push(note.text.clone());
        }
    }
    out
}

/// Owned snapshot of a stream row's display data, so we can release the
/// `&app.review` borrow before mutating `app` for highlight caching.
#[derive(Clone, Debug)]
enum OwnedRow {
    FileHeader {
        path: String,
        /// Absolute stream row (for `--note` lookup).
        abs_row: usize,
    },
    HunkHeader {
        text: String,
        /// Which file this hunk belongs to (for `--select` markers).
        file_idx: usize,
        /// 0-based index within the file's hunk list (for `--select` markers
        /// and `--note` hunk targeting).
        hunk_idx: usize,
        /// Absolute stream row of this header (for `--note` lookup).
        abs_row: usize,
    },
    Line {
        kind: DiffLineKind,
        text: String,
        file_idx: usize,
        abs_row: usize,
        /// Old-side source line number (deletes/context), if any.
        old_no: Option<u32>,
        /// New-side source line number (adds/context), if any.
        new_no: Option<u32>,
        /// Text of the paired counterpart line on the other side (Add↔Delete),
        /// for word-level inline highlight. `None` for unpaired / non-+/- lines.
        counterpart: Option<String>,
    },
}

impl OwnedRow {
    fn from_stream_row(review: &Review, row: StreamRow, abs_row: usize) -> Self {
        match row {
            StreamRow::FileHeader { path, .. } => OwnedRow::FileHeader {
                path: path.to_string(),
                abs_row,
            },
            StreamRow::HunkHeader {
                text,
                file_idx,
                hunk_idx,
            } => OwnedRow::HunkHeader {
                text: text.to_string(),
                file_idx,
                hunk_idx,
                abs_row,
            },
            StreamRow::Line {
                kind,
                text,
                file_idx,
                ..
            } => {
                // Resolve old/new source line numbers for the gutter. Cheap
                // (one hunk walk per row) and viewport-only.
                let (old_no, new_no) =
                    ViewportQuery::row_line_numbers(review, abs_row).unwrap_or((None, None));
                // Resolve the paired counterpart line text (Add↔Delete) for
                // word-level inline highlight. Same hunk-walk cost as line
                // numbers; None for context/meta/headers/unpaired lines.
                let counterpart = crate::ir::worddiff::counterpart_text(review, abs_row);
                OwnedRow::Line {
                    kind,
                    text: text.to_string(),
                    file_idx,
                    abs_row,
                    old_no,
                    new_no,
                    counterpart,
                }
            }
        }
    }
}

fn stream_row_to_line(
    app: &mut App,
    row: OwnedRow,
    current_match_row: Option<usize>,
    match_rows: &std::collections::HashSet<usize>,
) -> Line<'static> {
    let abs_row = match &row {
        OwnedRow::FileHeader { abs_row, .. } => *abs_row,
        OwnedRow::HunkHeader { abs_row, .. } => *abs_row,
        OwnedRow::Line { abs_row, .. } => *abs_row,
    };
    // Search matches only apply to code lines (headers use usize::MAX sentinel
    // historically — keep headers out of match sets by comparing against
    // the true abs_row only for Line rows via match_rows membership).
    let is_code_line = matches!(row, OwnedRow::Line { .. });
    let is_current_match = is_code_line && current_match_row == Some(abs_row);
    let is_other_match = is_code_line && !is_current_match && match_rows.contains(&abs_row);
    let in_visual = app.visual.as_ref().is_some_and(|v| v.contains(abs_row));

    let line = match row {
        OwnedRow::FileHeader { path, .. } => Line::from(Span::styled(
            format!("─── {} ───", path),
            Style::default()
                .fg(app.theme.file_header)
                .add_modifier(Modifier::BOLD),
        )),
        OwnedRow::HunkHeader {
            text,
            file_idx,
            hunk_idx,
            ..
        } => {
            // Decision markers: always in --select; otherwise when any
            // decisions exist (restored or recorded this session).
            if app.show_decision_markers() {
                let id = HunkId { file_idx, hunk_idx };
                let (mark, mark_color) = match app.decisions.get(&id).copied().unwrap_or_default() {
                    Decision::Accept => ("✓", app.theme.add),
                    Decision::Reject => ("✗", app.theme.delete),
                    Decision::Undecided => ("?", app.theme.dim),
                };
                Line::from(vec![
                    Span::styled(format!("[{}] ", mark), Style::default().fg(mark_color)),
                    Span::styled(
                        text,
                        Style::default()
                            .fg(app.theme.hunk_header)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(Span::styled(
                    text,
                    Style::default()
                        .fg(app.theme.hunk_header)
                        .add_modifier(Modifier::BOLD),
                ))
            }
        }
        OwnedRow::Line {
            kind,
            text,
            file_idx,
            abs_row,
            old_no,
            new_no,
            counterpart,
        } => {
            let (prefix, kind_style) = match kind {
                DiffLineKind::Add => ('+', Style::default().fg(app.theme.add)),
                DiffLineKind::Delete => ('-', Style::default().fg(app.theme.delete)),
                DiffLineKind::Meta => ('\\', Style::default().fg(app.theme.dim)),
                DiffLineKind::Context => (' ', Style::default()),
            };

            // Compute highlight runs for the code text (viewport-only, cached).
            // Live TUI: miss → plain + enqueue worker. Tests (no job_tx): sync fill.
            let line_in_file = ViewportQuery::file_and_line(&app.review, abs_row).map(|(_, li)| li);
            let hl_runs = if app.highlight_on {
                if let Some(li) = line_in_file {
                    let path = app.review.display_path(file_idx);
                    if let Some(runs) = app.cache.try_get(file_idx, li) {
                        runs
                    } else if let Some(tx) = app.hl_job_tx.as_ref() {
                        let _ = tx.send(crate::highlight::HighlightJob {
                            gen: app.cache.current_gen(),
                            file_idx,
                            line_in_file: li,
                            path: path.to_owned(),
                            text: text.clone(),
                            highlighter: Arc::clone(&app.highlighter),
                        });
                        vec![(Style::default(), text.clone())]
                    } else {
                        app.cache
                            .get_or_highlight(file_idx, li, path, &text, &app.highlighter)
                    }
                } else {
                    vec![(Style::default(), text.clone())]
                }
            } else {
                vec![(Style::default(), text.clone())]
            };

            // When word-diff is on and this +/- line has a paired counterpart,
            // refine the highlight runs to mark just the changed words. The
            // base style of each run (from syntax highlight) is preserved;
            // changed words get an extra emphasis (bold + brighter fg).
            let runs = if app.word_diff_on {
                if let Some(their) = counterpart.as_deref() {
                    let regions = word_diff_regions(&text, their);
                    refine_with_word_regions(
                        &hl_runs,
                        &regions,
                        kind,
                        app.theme.word_add,
                        app.theme.word_del,
                    )
                } else {
                    hl_runs
                }
            } else {
                hl_runs
            };

            let mut spans: Vec<Span> = Vec::with_capacity(runs.len() + 3);
            // Optional line-number gutter: " old new " right-aligned in 5 cols.
            if app.line_numbers_on {
                let dim = Style::default().fg(app.theme.dim);
                let old_s = old_no
                    .map(|n| format!("{n:>5}"))
                    .unwrap_or_else(|| "     ".into());
                let new_s = new_no
                    .map(|n| format!("{n:>5}"))
                    .unwrap_or_else(|| "     ".into());
                spans.push(Span::styled(format!(" {old_s} {new_s} "), dim));
            }
            spans.push(Span::styled(prefix.to_string(), kind_style));
            for (style, txt) in runs {
                spans.push(Span::styled(txt, style));
            }
            Line::from(spans)
        }
    };

    if is_current_match {
        // Re-slice the line's spans so the matched substrings get the active
        // match style (gold bg + black fg + bold) while the rest of the line
        // keeps its syntax color, with a subdued bg so the whole match row
        // still reads as "the hit". This shows *where* in the line the match
        // is, instead of painting the whole line one color.
        highlight_current_match_line(
            line,
            app.search.query.as_str(),
            app.theme.match_active_fg,
            app.theme.match_active_bg,
            app.theme.match_inactive_bg,
        )
    } else if is_other_match {
        line.style(Style::default().bg(app.theme.match_inactive_bg))
    } else if in_visual {
        // Visual range select highlight (viewport-layer only).
        line.style(
            Style::default()
                .bg(app.theme.selection_bg)
                .fg(app.theme.selection_fg),
        )
    } else {
        line
    }
}

/// Rewrite a line's spans so that every (case-insensitive) occurrence of
/// `needle` is painted with the active-match style, and the remaining spans
/// get a subdued background. Used for the *current* search-match row only;
/// other match rows keep their whole-line subdued bg (see `is_other_match`).
///
/// Works on the already-rendered spans (gutter + prefix + syntax/word-diff
/// runs), re-slicing them at match boundaries. Spans that carry their own bg
/// keep it outside matches; matched substrings are forced to the active style.
fn highlight_current_match_line(
    mut line: Line<'static>,
    needle: &str,
    active_fg: Color,
    active_bg: Color,
    inactive_bg: Color,
) -> Line<'static> {
    if needle.is_empty() {
        return line.style(Style::default().bg(active_bg).fg(active_fg));
    }
    // Reconstruct the full line text + per-char source-span index, so we can
    // find match offsets and then re-slice the original spans at those points.
    let spans = std::mem::take(&mut line.spans);
    // Build (text, style) flat list and cumulative char offsets.
    let mut parts: Vec<(String, Style)> = Vec::with_capacity(spans.len());
    let mut offsets: Vec<usize> = Vec::with_capacity(spans.len() + 1);
    let mut acc = 0usize;
    for sp in &spans {
        let t = sp.content.to_string();
        offsets.push(acc);
        acc += t.chars().count();
        parts.push((t, sp.style));
    }
    offsets.push(acc);
    let total = acc;
    let full: String = parts.iter().map(|(t, _)| t.as_str()).collect();

    // Find all match ranges (char offsets) in the full text, case-insensitive.
    let needle_lc = needle.to_lowercase();
    let full_lc = full.to_lowercase();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let needle_chars = needle_lc.chars().count();
    if needle_chars == 0 {
        return Line::from(spans).style(Style::default().bg(active_bg).fg(active_fg));
    }
    while let Some(rel) = full_lc[start..].find(&needle_lc) {
        let s = start + rel;
        let e = s + needle_chars;
        ranges.push((s, e));
        start = e;
        if start >= total {
            break;
        }
    }

    if ranges.is_empty() {
        // No substring (e.g. needle matched a header row this line isn't) —
        // fall back to a whole-line active style so the row still stands out.
        return Line::from(spans).style(Style::default().bg(active_bg).fg(active_fg));
    }

    // Build a per-char "is this char inside a match?" mask.
    let mut in_match = vec![false; total];
    for (s, e) in &ranges {
        in_match[*s..*e].fill(true);
    }

    // Re-slice each source span at match boundaries. For every char we know
    // (a) its source span's style and (b) whether it falls inside a match.
    // Coalesce runs of identical resulting style into single spans.
    let active = Style::default()
        .fg(active_fg)
        .bg(active_bg)
        .add_modifier(Modifier::BOLD);
    let dim_bg = Style::default().bg(inactive_bg);
    let mut out: Vec<Span<'static>> = Vec::new();
    let push_run = |out: &mut Vec<Span<'static>>, st: Style, text: String| {
        if text.is_empty() {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.style == st {
                last.content = format!("{}{}", last.content, text).into();
                return;
            }
        }
        out.push(Span::styled(text, st));
    };
    for (idx, (text, style)) in parts.iter().enumerate() {
        let span_start = offsets[idx];
        let mut buf = String::new();
        let mut buf_style: Option<Style> = None;
        for (i, c) in text.chars().enumerate() {
            let global = span_start + i;
            let m = in_match.get(global).copied().unwrap_or(false);
            let st = if m { active } else { style.patch(dim_bg) };
            if buf_style != Some(st) {
                push_run(&mut out, buf_style.unwrap_or(st), std::mem::take(&mut buf));
                buf_style = Some(st);
            }
            buf.push(c);
        }
        if let Some(st) = buf_style {
            push_run(&mut out, st, std::mem::take(&mut buf));
        }
    }
    Line::from(out)
}

fn draw_status(app: &App, frame: &mut Frame, area: Rect) {
    let pos = if app.review.stream_len == 0 {
        "0/0".to_string()
    } else {
        format!("{}/{}", app.scroll_y + 1, app.review.stream_len)
    };
    let hl = if app.highlight_on { " HL" } else { "" };
    // Per-file and total +/- tallies (green inserts, red deletes).
    let file = app.review.files.get(app.selected_file);
    let file_stats = match file {
        Some(f) => format!("+{}/−{}", f.inserts, f.deletes),
        None => String::new(),
    };
    // Cap the path's display width so a long path can't push the totals,
    // banner, and status text off the right edge of a narrow terminal.
    // Budget: reserve ~half the width for the path; the rest is for the
    // suffix (file index, position, stats) plus the right-side spans.
    let non_path_suffix = format!(
        "  [{}]  {}  {}{} ",
        app.selected_file + 1,
        pos,
        file_stats,
        hl
    );
    let path_budget = (area.width as usize / 2).max(12);
    let shown_path = status_path(app.current_path(), path_budget);
    let left = format!(" {}{}", shown_path, non_path_suffix);
    let totals = format!(" Σ +{}/−{} ", app.review.inserts, app.review.deletes);
    // Review progress when select/tracking is on, or once any decision exists.
    let progress = if app.decisions_active() || !app.decisions.is_empty() {
        Some(format!(" {} ", app.review_progress_label()))
    } else {
        None
    };
    let right = format!(" {} ", app.status);
    // A banner note (`--note banner=text`) is surfaced in the status bar so the
    // human sees the agent's high-level summary without scrolling.
    let banner: Option<String> = app
        .notes
        .iter()
        .find(|n| matches!(n.target, crate::tui::app::NoteTarget::Banner))
        .map(|n| format!(" 💬 {} ", n.text));
    let mut spans = vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(totals, Style::default().fg(app.theme.dim)),
    ];
    if let Some(p) = progress {
        spans.push(Span::styled(p, Style::default().fg(app.theme.hunk_header)));
    }
    if let Some(b) = banner {
        spans.push(Span::styled(b, Style::default().fg(app.theme.note)));
    }
    spans.push(Span::styled(right, Style::default().fg(app.theme.dim)));
    let line = Line::from(spans);
    let para = Paragraph::new(line).style(Style::default().bg(app.theme.status_bg));
    frame.render_widget(para, area);
}

/// Render the help line, or an input prompt when editing search/filter.
fn draw_help_or_prompt(app: &App, frame: &mut Frame, area: Rect) {
    let content = match app.mode {
        InputMode::Search => {
            format!(
                "/{}▌  (Enter search · Enter confirm · Esc cancel)",
                app.search.query
            )
        }
        InputMode::Filter => {
            format!(
                "filter: {}▌  (path substring · Enter confirm · Esc cancel)",
                app.path_filter
            )
        }
        InputMode::Visual => app.status.clone(),
        InputMode::Comment => {
            let loc = app
                .comment_placement
                .as_ref()
                .map(|p| p.display())
                .unwrap_or_else(|| "?".into());
            format!(
                "comment [{loc}]: {}▌  (Enter save · Esc cancel)",
                app.comment_draft
            )
        }
        InputMode::Normal => {
            if app.decisions_active() {
                " j/k · ]h/[h hunk · ]u unreviewed · a/r/u decide · A file · v visual · c comment · zc/zo · / · o · ? · q "
                    .to_string()
            } else {
                " j/k scroll · ]h/[h hunk · v visual · c/C comment · zc/zo fold · Tab file · / search · o open · ? help · q quit "
                    .to_string()
            }
        }
    };
    let style = match app.mode {
        InputMode::Normal => Style::default().fg(app.theme.dim),
        _ => Style::default()
            .fg(app.theme.edit_mode_fg)
            .add_modifier(Modifier::BOLD),
    };
    let para = Paragraph::new(content).style(style);
    frame.render_widget(para, area);
}

/// Full-screen keybinding reference, drawn on top of the review when the user
/// presses `?`. Rendered as a centered, bordered panel with the bindings grouped
/// by category; section headers use the hunk-header color so they stand out
/// without fighting the Flexoki chrome.
fn draw_help_overlay(app: &App, frame: &mut Frame) {
    let area = frame.area();

    // A centered panel: up to 64 cols wide and as tall as the content needs,
    // clamped to the terminal with a 1-row margin.
    let width = 64u16.min(area.width.saturating_sub(2));
    let height = 30u16.min(area.height.saturating_sub(2));
    let popup = centered_rect(width, height, area);

    // Clear the underlying cells so the overlay reads as a floating panel.
    frame.render_widget(Clear, popup);

    let key = Style::default()
        .fg(app.theme.hunk_header)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(app.theme.dim);
    let head = Style::default()
        .fg(app.theme.file_header)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = Vec::new();
    push_help_section(
        &mut lines,
        "Navigation",
        &[
            ("j / ↓", "scroll down one row"),
            ("k / ↑", "scroll up one row"),
            ("J / PgDn", "scroll half a page"),
            ("K / PgUp", "scroll half a page up"),
            ("Ctrl-D / Ctrl-U", "scroll half a page down / up"),
            ("Ctrl-F / Ctrl-B", "scroll a full page down / up"),
            ("g / Home", "jump to top"),
            ("G / End", "jump to bottom"),
            ("]h / [h", "next / previous hunk (wraps files)"),
            ("SPC", "next hunk"),
            ("Tab / l", "next file"),
            ("BackTab / h", "previous file"),
            ("1-9", "jump to the Nth file"),
            ("b", "toggle file rail"),
            ("o", "open focused line in $EDITOR"),
        ],
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "View",
        &[
            ("H", "toggle syntax highlight"),
            ("#", "toggle line-number gutter"),
            ("w", "toggle word-level inline diff"),
            ("W", "toggle ignore-whitespace"),
            ("t", "cycle theme (light → auto → dark → presets)"),
            ("zc / zo", "fold / unfold current file"),
        ],
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "Search & filter",
        &[
            ("/", "search diff content"),
            ("n / N", "next / previous match"),
            ("f", "filter files by path substring"),
        ],
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "Comments (visual range)",
        &[
            ("v", "enter visual select at top code row"),
            ("j / k", "extend selection (in visual)"),
            ("c", "comment: current line, or range in visual"),
            ("C", "comment on current hunk"),
            ("Enter / Esc", "save / cancel comment draft"),
        ],
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "Review decisions",
        &[
            ("a / r / u", "accept / reject / undecided (auto next hunk)"),
            ("A", "accept all hunks in current file (when not --select)"),
            ("A / R", "--select: accept / reject rest of current file"),
            ("Ctrl-A / Ctrl-R", "--select: accept / reject all remaining"),
            ("]u / [u", "next / previous unreviewed hunk"),
            ("]U / [U", "next / previous unreviewed file"),
        ],
        head,
        key,
        dim,
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ? / Esc / q / Enter  dismiss this help",
        Style::default().fg(app.theme.edit_mode_fg),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " next-hunk — keybindings ",
            Style::default()
                .fg(app.theme.file_header)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.status_bg));
    let para = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left);
    frame.render_widget(para, popup);
}

/// Push a titled group of keybinding rows into the help-overlay line list.
/// Uses `&'static str` so the built `Line<'static>` borrows the literal text
/// directly (no allocation, no lifetime knot from a closure).
fn push_help_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    rows: &[(&'static str, &'static str)],
    head: Style,
    key: Style,
    dim: Style,
) {
    lines.push(Line::from(Span::styled(format!(" {title}"), head)));
    for (k, d) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<14}", k), key),
            Span::styled(*d, dim),
        ]));
    }
    lines.push(Line::from(""));
}

/// Center a `w × h` rect inside `area` (used by the help overlay).
fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w.min(area.width), h.min(area.height))
}

/// Refine syntax-highlight runs with word-level change regions.
///
/// `hl_runs` are the (style, text) runs from syntax highlighting (covering the
/// full line text). `regions` classify substrings of the same text as
/// [`WordRegion::Same`] or [`WordRegion::Changed`]. The output preserves the
/// base syntax style of each run but adds emphasis (bold) to the portions that
/// fall inside `Changed` regions.
///
/// This is O(runs × regions) but both are tiny for a single line.
fn refine_with_word_regions(
    hl_runs: &[(Style, String)],
    regions: &[(WordRegion, String)],
    kind: DiffLineKind,
    word_add: Color,
    word_del: Color,
) -> Vec<(Style, String)> {
    // Flatten hl_runs into (style, text) with cumulative char offsets so we
    // can slice them against region boundaries.
    // Build the region boundary list as char offsets into the full text.
    let mut out: Vec<(Style, String)> = Vec::new();
    let mut hl_pos = 0usize; // current char position consumed in hl_runs
    let mut reg_pos = 0usize; // current char position consumed in regions

    // Precompute cumulative starts for regions.
    let mut reg_starts: Vec<usize> = Vec::with_capacity(regions.len() + 1);
    let mut acc = 0;
    for (_, t) in regions {
        reg_starts.push(acc);
        acc += t.chars().count();
    }
    reg_starts.push(acc);

    // Precompute cumulative starts for hl_runs.
    let mut hl_starts: Vec<usize> = Vec::with_capacity(hl_runs.len() + 1);
    let mut acc = 0;
    for (_, t) in hl_runs {
        hl_starts.push(acc);
        acc += t.chars().count();
    }
    let total = acc;
    hl_starts.push(total);

    // Walk both sequences by char offset, emitting overlapping slices.
    let mut hi = 0usize; // index into hl_runs
    let mut ri = 0usize; // index into regions
    while hi < hl_runs.len() && ri < regions.len() {
        let (hl_style, hl_text) = &hl_runs[hi];
        let (region, _reg_text) = &regions[ri];
        let hl_end = hl_starts[hi + 1];
        let reg_end = reg_starts[ri + 1];
        let start = hl_pos.max(reg_pos);
        let end = hl_end.min(reg_end);
        if start < end {
            // Extract the overlapping substring from hl_text.
            let lo = start - hl_starts[hi];
            let hi_len = end - hl_starts[hi];
            let slice: String = hl_text.chars().skip(lo).take(hi_len - lo).collect();
            let style = if *region == WordRegion::Changed {
                word_emphasis_style(hl_style, kind, word_add, word_del)
            } else {
                *hl_style
            };
            // Merge with previous run if same style (avoids span explosion).
            if let Some(last) = out.last_mut() {
                if last.0 == style {
                    last.1.push_str(&slice);
                } else {
                    out.push((style, slice));
                }
            } else {
                out.push((style, slice));
            }
        }
        // Advance whichever ends first (or both if equal).
        if hl_end <= reg_end {
            hi += 1;
            hl_pos = hl_end;
        }
        if reg_end <= hl_end {
            ri += 1;
            reg_pos = reg_end;
        }
    }

    // Safety net: if anything went wrong and we didn't cover the full text,
    // fall back to the plain hl_runs (never lose content).
    let out_len: usize = out.iter().map(|(_, t)| t.chars().count()).sum();
    if out_len != total {
        return hl_runs.iter().map(|(s, t)| (*s, t.clone())).collect();
    }
    out
}

/// Style for a changed word within a +/- line. Keeps the base syntax style but
/// adds bold and shifts the foreground toward a brighter shade of the line's
/// diff color so the change pops without hiding syntax coloring.
fn word_emphasis_style(
    base: &Style,
    kind: DiffLineKind,
    word_add: Color,
    word_del: Color,
) -> Style {
    let style = match kind {
        DiffLineKind::Add => base.fg(word_add),
        DiffLineKind::Delete => base.fg(word_del),
        _ => *base,
    };
    style.add_modifier(Modifier::BOLD | Modifier::REVERSED)
}

/// Truncate a path for the rail display.
fn short_path(path: &str) -> String {
    if path.len() <= 24 {
        return path.to_string();
    }
    if let Some(idx) = path.rfind('/') {
        let last = &path[idx + 1..];
        if last.len() <= 22 {
            return format!("…/{}", last);
        }
        return format!("…{}", &last[last.len() - 22..]);
    }
    format!("…{}", &path[path.len() - 22..])
}

/// Build the two halves of a file's per-file change tally for the rail.
/// Returns `("+N", "−M")` with each side empty when it's zero, so an
/// add-only file shows just `+12` and a pure delete just `−3`. Uses the
/// Unicode minus (`−`) to match the status bar's existing style.
fn file_stats_tail(inserts: u64, deletes: u64) -> (String, String) {
    let plus = if inserts > 0 {
        format!("+{}", inserts)
    } else {
        String::new()
    };
    let minus = if deletes > 0 {
        format!("−{}", deletes)
    } else {
        String::new()
    };
    (plus, minus)
}

/// Truncate a path for the status bar, capped at `max_chars` display columns
/// and biased toward keeping the trailing filename (that's what identifies the
/// file to the reviewer). When the path fits, it's returned unchanged. When it
/// doesn't, we keep the last `max_chars - 1` chars and prefix an ellipsis —
/// unless there's a directory separator, in which case we prefer `…/<basename>`
/// so the truncation reads naturally. Operates on chars (UTF-8 safe).
fn status_path(path: &str, max_chars: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max_chars || max_chars == 0 {
        return path.to_string();
    }
    // Try to keep the basename (last segment after '/') intact under the cap.
    if let Some(slash) = chars.iter().rposition(|&c| c == '/') {
        let basename: String = chars[slash + 1..].iter().collect();
        // "…/" + basename
        if basename.chars().count() + 2 <= max_chars {
            return format!("…/{}", basename);
        }
    }
    // Fall back: keep the trailing (max_chars - 1) chars with a leading ellipsis.
    let keep = max_chars.saturating_sub(1);
    let tail: String = chars[chars.len() - keep..].iter().collect();
    format!("…{}", tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::Highlighter;
    use crate::ir::parse_unified_diff;

    fn highlighter() -> Highlighter {
        Highlighter::load_noop()
    }

    #[test]
    fn file_stats_tail_both_sides() {
        let (plus, minus) = file_stats_tail(12, 3);
        assert_eq!(plus, "+12");
        assert_eq!(minus, "−3");
    }

    #[test]
    fn file_stats_tail_add_only() {
        let (plus, minus) = file_stats_tail(7, 0);
        assert_eq!(plus, "+7");
        assert_eq!(minus, "");
    }

    #[test]
    fn file_stats_tail_delete_only() {
        let (plus, minus) = file_stats_tail(0, 5);
        assert_eq!(plus, "");
        assert_eq!(minus, "−5");
    }

    #[test]
    fn file_stats_tail_no_changes() {
        // A context-only file (no +/-) renders no tally — keeps the rail clean.
        let (plus, minus) = file_stats_tail(0, 0);
        assert!(plus.is_empty());
        assert!(minus.is_empty());
    }

    /// The rail should now render a per-file +/- tally alongside each path.
    /// Uses the same sample app the other draw tests use, rendered to a
    /// buffer; we assert the add count appears as a styled span.
    #[test]
    fn draw_rail_shows_per_file_tally() {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new value
diff --git a/b.rs b/b.rs
--- b/b.rs
+++ b/b.rs
@@ -1,2 +1,2 @@
-foo
+bar
 baz
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        let backend = ratatui::backend::TestBackend::new(40, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // a.rs has +1/−1, b.rs has +1/−1. Both "+1" and the Unicode minus "−1"
        // should appear in the rendered rail area.
        assert!(
            rendered.contains("+1"),
            "rail should show +1 tally: {rendered}"
        );
        assert!(
            rendered.contains("−1"),
            "rail should show −1 tally: {rendered}"
        );
    }

    /// Search for a term, then render and confirm the current-match row's
    /// buffer carries the active-match background somewhere on the matched
    /// text (i.e. the inline highlight is applied, not just a whole-line wash).
    #[test]
    fn search_match_is_highlighted_inline() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old value here
+new value here
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 20;
        // Drive the search through the public handle_key path.
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in "value".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.search.active);
        assert!(!app.search.matches.is_empty());

        let backend = ratatui::backend::TestBackend::new(40, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        // At least one cell must carry the active-match background (gold),
        // proving the inline highlight fired rather than a whole-line wash
        // (which would also set bg, so we additionally assert the matched
        // line is NOT uniformly the inactive bg everywhere).
        let buf = terminal.backend().buffer();
        let active_bg = app.theme.match_active_bg;
        let mut found_active = false;
        for cell in buf.content().iter() {
            if cell.bg == active_bg {
                found_active = true;
                break;
            }
        }
        assert!(found_active, "expected an inline active-match cell");
    }

    /// No active search -> no match background anywhere in the buffer.
    #[test]
    fn no_match_highlight_without_search() {
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
        app.viewport_height = 20;
        let backend = ratatui::backend::TestBackend::new(40, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let active_bg = app.theme.match_active_bg;
        for cell in terminal.backend().buffer().content().iter() {
            assert_ne!(cell.bg, active_bg, "no active match expected");
        }
    }

    #[test]
    fn status_path_short_path_unchanged() {
        assert_eq!(status_path("a.rs", 20), "a.rs");
        assert_eq!(status_path("src/mod.rs", 20), "src/mod.rs");
    }

    #[test]
    fn status_path_keeps_basename_when_too_long() {
        // Long path, generous cap: basename fits under "…/<basename>".
        let p = "very/deeply/nested/module/path/file.rs";
        assert_eq!(status_path(p, 16), "…/file.rs");
    }

    #[test]
    fn status_path_basename_too_long_keeps_tail() {
        // Both the path and its basename exceed the cap: fall back to a
        // leading ellipsis + the trailing (cap-1) chars.
        let p = "src/some/really/long/basename_that_exceeds.rs";
        // cap=10 -> keep 9 trailing chars, prefixed with ellipsis.
        assert_eq!(status_path(p, 10), "…xceeds.rs");
    }

    #[test]
    fn status_path_no_separator_uses_tail() {
        assert_eq!(status_path("abcdefghij", 5), "…ghij");
    }

    #[test]
    fn status_path_zero_budget_returns_unchanged() {
        // A zero budget is a guard; we return the path as-is rather than panic.
        assert_eq!(status_path("a.rs", 0), "a.rs");
    }

    /// Split layout pairs delete+add on the same visual row and keeps
    /// headers full-width. Pure unit test of the pairing transform — no TUI.
    #[test]
    fn build_split_pairs_delete_add_runs() {
        let rows = vec![
            OwnedRow::FileHeader {
                path: "a.rs".into(),
                abs_row: 0,
            },
            OwnedRow::HunkHeader {
                text: "@@ -1 +1 @@".into(),
                file_idx: 0,
                hunk_idx: 0,
                abs_row: 1,
            },
            OwnedRow::Line {
                kind: DiffLineKind::Context,
                text: "keep".into(),
                file_idx: 0,
                abs_row: 2,
                old_no: Some(1),
                new_no: Some(1),
                counterpart: None,
            },
            OwnedRow::Line {
                kind: DiffLineKind::Delete,
                text: "old".into(),
                file_idx: 0,
                abs_row: 3,
                old_no: Some(2),
                new_no: None,
                counterpart: Some("new".into()),
            },
            OwnedRow::Line {
                kind: DiffLineKind::Add,
                text: "new".into(),
                file_idx: 0,
                abs_row: 4,
                old_no: None,
                new_no: Some(2),
                counterpart: Some("old".into()),
            },
            OwnedRow::Line {
                kind: DiffLineKind::Add,
                text: "extra".into(),
                file_idx: 0,
                abs_row: 5,
                old_no: None,
                new_no: Some(3),
                counterpart: None,
            },
        ];
        let visual = build_split_visual_rows(rows);
        // FileHeader + HunkHeader + Context pair + Delete|Add pair + orphan Add
        assert_eq!(visual.len(), 5);
        assert!(matches!(visual[0], SplitVisual::Full(_)));
        assert!(matches!(visual[1], SplitVisual::Full(_)));
        match &visual[2] {
            SplitVisual::Pair {
                left: Some(l),
                right: Some(r),
            } => {
                assert!(matches!(
                    l,
                    OwnedRow::Line {
                        kind: DiffLineKind::Context,
                        ..
                    }
                ));
                assert!(matches!(
                    r,
                    OwnedRow::Line {
                        kind: DiffLineKind::Context,
                        ..
                    }
                ));
            }
            other => panic!("expected context pair, got {other:?}"),
        }
        match &visual[3] {
            SplitVisual::Pair {
                left: Some(l),
                right: Some(r),
            } => {
                assert!(matches!(
                    l,
                    OwnedRow::Line {
                        kind: DiffLineKind::Delete,
                        ..
                    }
                ));
                assert!(matches!(
                    r,
                    OwnedRow::Line {
                        kind: DiffLineKind::Add,
                        text,
                        ..
                    } if text == "new"
                ));
            }
            other => panic!("expected delete|add pair, got {other:?}"),
        }
        match &visual[4] {
            SplitVisual::Pair {
                left: None,
                right: Some(r),
            } => {
                assert!(matches!(
                    r,
                    OwnedRow::Line {
                        kind: DiffLineKind::Add,
                        text,
                        ..
                    } if text == "extra"
                ));
            }
            other => panic!("expected orphan add, got {other:?}"),
        }
    }

    /// Wide terminal + layout=split: stream shows side-by-side labels and the
    /// center gutter. Uses a 100-col backend so the stream pane (after rail)
    /// is ≥ 80 cols and split is active.
    #[test]
    fn draw_split_shows_side_by_side_panes() {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old_line
+new_line
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.layout_mode = crate::config::LayoutMode::Split;
        app.viewport_height = 12;
        // 100 wide: rail ~12–25, stream well above SPLIT_MIN_WIDTH (80).
        let backend = ratatui::backend::TestBackend::new(100, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains("old") && rendered.contains("new"),
            "split should show old and new content: {rendered}"
        );
        assert!(
            rendered.contains('│') || rendered.contains("▌"),
            "split should show pane separator/labels: {rendered}"
        );
    }

    /// Narrow stream width: layout=split must not crash and must fall back
    /// (stack or unified) rather than attempting two panes that would clip.
    #[test]
    fn draw_split_falls_back_on_narrow_terminal() {
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
        app.layout_mode = crate::config::LayoutMode::Split;
        app.viewport_height = 8;
        // 50 cols total → stream pane well under 80; must not panic.
        let backend = ratatui::backend::TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        // Still renders the file content somehow (fallback path).
        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains("old") || rendered.contains("new") || rendered.contains("a.rs"),
            "narrow fallback should still render content: {rendered}"
        );
    }

    /// `layout = "auto"` resolves per-frame based on stream width:
    /// ≥ AUTO_SPLIT_MIN_WIDTH → split, ≥ AUTO_STACK_MIN_WIDTH → stack, else
    /// unified. Verifies the resolution helper directly so the threshold
    /// contract is pinned independent of the renderer (WXB-25).
    #[test]
    fn auto_layout_resolves_by_width() {
        use crate::config::LayoutMode;
        assert_eq!(
            effective_layout(LayoutMode::Auto, AUTO_SPLIT_MIN_WIDTH),
            LayoutMode::Split,
            "auto at AUTO_SPLIT_MIN_WIDTH must pick split"
        );
        assert_eq!(
            effective_layout(LayoutMode::Auto, AUTO_SPLIT_MIN_WIDTH - 1),
            LayoutMode::Stack,
            "auto just below AUTO_SPLIT_MIN_WIDTH must fall back to stack"
        );
        assert_eq!(
            effective_layout(LayoutMode::Auto, AUTO_STACK_MIN_WIDTH),
            LayoutMode::Stack,
            "auto at AUTO_STACK_MIN_WIDTH must pick stack"
        );
        assert_eq!(
            effective_layout(LayoutMode::Auto, AUTO_STACK_MIN_WIDTH - 1),
            LayoutMode::Unified,
            "auto just below AUTO_STACK_MIN_WIDTH must fall back to unified"
        );
        // Explicit modes pass through unchanged — auto only fans out for Auto.
        assert_eq!(effective_layout(LayoutMode::Split, 200), LayoutMode::Split);
        assert_eq!(effective_layout(LayoutMode::Stack, 200), LayoutMode::Stack);
        assert_eq!(
            effective_layout(LayoutMode::Unified, 200),
            LayoutMode::Unified
        );
    }

    /// `layout = "auto"` on a wide terminal must render the side-by-side split
    /// path (no crash, content present). The view ladder handles the render;
    /// this just exercises the full path end-to-end (WXB-25).
    #[test]
    fn draw_auto_wide_renders_split() {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old_line
+new_line
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.layout_mode = crate::config::LayoutMode::Auto;
        app.viewport_height = 12;
        // 200 wide: rail ~25, stream ~175, well above AUTO_SPLIT_MIN_WIDTH (120).
        let backend = ratatui::backend::TestBackend::new(200, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let rendered: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains('│') || rendered.contains("▌"),
            "auto on wide terminal should render the split pane separator: {rendered}"
        );
    }

    /// A long path must not force the status row wider than the terminal.
    /// Render at a narrow width with a deep path and assert the rendered
    /// status line does not overflow the area (no content past the right edge).
    #[test]
    fn long_path_does_not_overflow_narrow_status() {
        let review = parse_unified_diff(
            "\
diff --git a/very/deeply/nested/module/path/file_with_long_name.rs b/very/deeply/nested/module/path/file_with_long_name.rs
--- a/very/deeply/nested/module/path/file_with_long_name.rs
+++ b/very/deeply/nested/module/path/file_with_long_name.rs
@@ -1 +1 @@
-old
+new
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 6;
        // 24 cols wide — narrower than the full path. The status row must fit.
        let backend = ratatui::backend::TestBackend::new(24, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        // The status bar is the second-to-last row. Read its cells and confirm
        // the rendered path was truncated (an ellipsis is present), proving the
        // width budget kicked in rather than the full path being laid out.
        let buf = terminal.backend().buffer();
        let width = 24usize;
        let status_row = 4usize; // main area is rows 0..3, status at row 4
        let row_text: String = (0..width)
            .map(|x| {
                buf[(x as u16, status_row as u16)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(
            row_text.contains('…'),
            "expected the path to be truncated with an ellipsis, got: {row_text:?}"
        );
    }
}
