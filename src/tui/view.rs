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
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
use ratatui::Frame;

use crate::ir::{
    word_diff_regions, DiffLineKind, Review, StreamRow, Viewport, ViewportQuery, WordRegion,
};
use crate::tui::app::{App, Decision, HunkId, InputMode, ToastKind};

/// Rail width (left file list). Capped to a fraction of the area at draw time.
const RAIL_MAX_WIDTH: u16 = 32;

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
    // A one-column scrollbar rides the right edge whenever the review
    // out-scrolls the viewport (the norm on huge diffs): position at a
    // glance. The stream truncates one column earlier to make room, so the
    // bar never paints over diff content.
    let overflows = app.collapse.virtual_len() > area.height as usize;
    let mut scrollbar_area: Option<Rect> = None;
    let content_area = if overflows && area.width > 12 {
        scrollbar_area = Some(Rect {
            x: area.x + area.width - 1,
            width: 1,
            ..area
        });
        Rect {
            width: area.width - 1,
            ..area
        }
    } else {
        area
    };

    if app.show_rail {
        let rail_w = RAIL_MAX_WIDTH.min(content_area.width / 4).max(12);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(rail_w), Constraint::Min(0)])
            .split(content_area);
        draw_rail(app, frame, cols[0]);
        draw_stream(app, frame, cols[1]);
    } else {
        app.rail_rect = None;
        draw_stream(app, frame, content_area);
    }

    if let Some(sb_area) = scrollbar_area {
        draw_scrollbar(app, frame, sb_area);
    }
}

/// Render the stream's vertical scrollbar: a dim track with a theme-accented
/// thumb sized to the visible share of the virtual rows. The travel range is
/// the number of distinct top-row positions, matching how `scroll_y` clamps.
fn draw_scrollbar(app: &App, frame: &mut Frame, area: Rect) {
    // Travel counts distinct top-row positions over the *content* rows — the
    // pane title consumes one row of the main area, so it is not a diff row.
    let visible = area.height.saturating_sub(1) as usize;
    let travel = app.collapse.virtual_len().saturating_sub(visible).max(1);
    let mut state = ScrollbarState::new(travel).position(app.scroll_y.min(travel));
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(Style::default().fg(app.theme.dim))
        .thumb_symbol("█")
        .thumb_style(Style::default().fg(app.theme.hunk_header));
    frame.render_stateful_widget(scrollbar, area, &mut state);
}

fn draw_rail(app: &mut App, frame: &mut Frame, area: Rect) {
    use unicode_width::UnicodeWidthStr;
    let visible = app.visible_files();
    let note_counts = if app.show_notes {
        app.note_counts_by_file()
    } else {
        std::collections::HashMap::new()
    };
    // Inner width available for a rail row, minus the left border and the
    // " N " index prefix. Used to pad the path so the +/- tail right-aligns.
    let rail_inner_w = area.width.saturating_sub(1) as usize;
    let items: Vec<ListItem> = visible
        .iter()
        .map(|&i| {
            let f = &app.review.files[i];
            let head = format!(" {}. ", i + 1);
            let kind = ChangeKind::from_file(f);
            // Compact per-file change tally: `+ins` (green) next to `−del`
            // (red), zero sides omitted (e.g. an add-only file shows `+12`,
            // a pure delete `−3`). Colored so a glance at the rail shows
            // where the change mass sits.
            let (plus, minus) = file_stats_tail(f.inserts, f.deletes);
            // Note badge: how many agent notes/comments target this file
            // (jump between them with `}`/`{`).
            // No leading space: 💬 is double-width and the pad span (when
            // there is room) provides the separation — at tight rail widths
            // the spare column keeps the count from clipping.
            let badge = match note_counts.get(&i) {
                Some(&n) if n > 0 => format!("💬{n}"),
                _ => String::new(),
            };
            // Path budget: whatever the row leaves after the fixed furniture
            // (index, chip, chevron) and the right-aligned tally/badge, so
            // the tally stays on-screen instead of clipping away on narrow
            // rails. Floors at 4 columns (a degenerate 12-column rail can
            // still clip); caps at 22 to keep path lengths uniform.
            let tail_w = plus.width() + minus.width();
            let badge_w = badge.width();
            let need_pad = tail_w > 0 || badge_w > 0;
            let sep = if need_pad { 2 } else { 0 };
            let path_budget = rail_inner_w
                .saturating_sub(head.width() + 4 + sep + tail_w + badge_w)
                .clamp(4, 22);
            let path = short_path(&f.display_path, path_budget);
            let folded = app.folded.contains(&i);
            let mut style = if i == app.selected_file {
                Style::default()
                    .fg(app.theme.selection_fg)
                    .bg(app.theme.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            // Folded files dim and carry a fold chevron, so `zc`'s effect is
            // visible at a glance instead of only as rows vanishing from the
            // stream. The chevron is row chrome — always drawn (▾ open /
            // ▸ folded) so paths stay column-aligned.
            if folded {
                style = style.fg(app.theme.dim).add_modifier(Modifier::DIM);
            }
            // The chip and chevron sit on the selected row's highlight too,
            // or the selection bar shows a hole at their columns.
            let chip_color = match kind {
                ChangeKind::Added => app.theme.add,
                ChangeKind::Deleted => app.theme.delete,
                ChangeKind::Renamed | ChangeKind::Modified => app.theme.dim,
            };
            let chip_style = if i == app.selected_file {
                Style::default().fg(chip_color).bg(app.theme.selection_bg)
            } else {
                Style::default().fg(chip_color)
            };
            let chevron_style = if i == app.selected_file {
                Style::default()
                    .fg(app.theme.dim)
                    .bg(app.theme.selection_bg)
            } else {
                Style::default().fg(app.theme.dim)
            };
            // Pad the path so the tally right-aligns within the row. Widths
            // (not char counts) keep the alignment on double-width CJK
            // paths; the badge is measured the same way (💬 is 2 columns).
            let used = head.width() + 4 + path.width();
            let mut spans: Vec<Span> = vec![
                Span::styled(head, style),
                Span::styled(format!("{} ", kind.letter()), chip_style),
                Span::styled(if folded { "▸ " } else { "▾ " }, chevron_style),
                Span::styled(path, style),
            ];
            if need_pad {
                let pad = rail_inner_w.saturating_sub(used + sep + tail_w + badge_w);
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(plus, Style::default().fg(app.theme.add)));
                spans.push(Span::styled(minus, Style::default().fg(app.theme.delete)));
                if !badge.is_empty() {
                    spans.push(Span::styled(badge, Style::default().fg(app.theme.note)));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let title = if app.path_filter.trim().is_empty() {
        "Files".to_string()
    } else {
        format!("Files ({}/{})", visible.len(), app.review.file_count())
    };
    let block = Block::default().borders(Borders::RIGHT).title(title);
    // Hit-test rect: the list's *content* rows (the block strips the title
    // row and the right border), so a click's `row - rect.y` indexes the
    // rendered items directly.
    app.rail_rect = Some(block.inner(area));

    // Map selected_file to its position in the visible list for the ListState.
    let selected_pos = visible.iter().position(|&i| i == app.selected_file);
    let mut state = ListState::default();
    state.select(selected_pos);
    frame.render_stateful_widget(List::new(items).block(block), area, &mut state);
    // Record where the (possibly scrolled) list starts so mouse clicks can
    // map rows back to items when there are more files than rows.
    app.rail_list_offset = state.offset();
}

fn draw_stream(app: &mut App, frame: &mut Frame, area: Rect) {
    // All three pane renderers draw a one-row title inside this area, so
    // mouse hit-testing must target the content rows below it.
    app.stream_rect = Some(Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    });
    // Keep the virtual index in sync with the layout this width resolves to
    // (auto switches split/stack/unified across thresholds; the index must
    // match what is drawn).
    app.sync_effective_layout(area.width);
    // Narrow terminal: stack/split fall back to unified (no crash).
    match app.effective_layout() {
        crate::config::LayoutMode::Stack if area.width >= 40 => draw_stream_stack(app, frame, area),
        crate::config::LayoutMode::Split if area.width >= 80 => draw_stream_split(app, frame, area),
        _ => draw_stream_unified(app, frame, area),
    }
}

/// Side-by-side split layout: one aligned row per (old, new) pair, two
/// half-width columns separated by a dim divider. Works on the same
/// virtual-row viewport as unified/stack — the index just counts pairs
/// instead of lines, so scrolling, search, hunk jumps and folding all keep
/// working unchanged. Full-width rows (file/hunk headers, collapse markers,
/// notes) span both columns.
fn draw_stream_split(app: &mut App, frame: &mut Frame, area: Rect) {
    // The pane title occupies the area's first row, so only the rows below
    // it are diff rows — materialize exactly what fits. Keeping this in step
    // with the viewport_height the run loop syncs is what lets max_scroll
    // reach the last diff line.
    let height = area.height.saturating_sub(1) as usize;
    let scroll_y = app.scroll_y;
    let viewport = Viewport {
        start: scroll_y,
        height,
    };

    let owned_rows: Vec<OwnedRow> =
        ViewportQuery::rows_virtual(&app.review, viewport, &app.collapse)
            .into_iter()
            .map(|vr| OwnedRow::from_stream_row(&app.review, vr.row, vr.abs_row, app.tab_width))
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
    let notes_by_row = build_notes_by_row(app);

    // Column budget: [gutter|sign|text] │ [gutter|sign|text]
    const GUTTER: usize = 6;
    const DIVIDER: &str = " │ ";
    let per_side = (area.width as usize).saturating_sub(2 * (GUTTER + 1) + 3) / 2;

    let title = app.current_path().to_string();
    let cursor_row = if app.cursor_on {
        Some(app.cursor_stream_row())
    } else {
        None
    };
    let mut lines: Vec<Line> = Vec::with_capacity(owned_rows.len());
    for r in owned_rows {
        let abs_row = owned_row_abs(&r);
        let is_cursor = cursor_row == Some(abs_row);
        let notes = notes_by_row.get(&abs_row);
        match r {
            OwnedRow::Pair { old, new, .. } => {
                let mut spans: Vec<Span> = Vec::with_capacity(8);
                spans.extend(split_side_spans(
                    app,
                    old.as_ref(),
                    per_side,
                    current_match_row,
                    &match_rows,
                ));
                spans.push(Span::styled(DIVIDER, Style::default().fg(app.theme.dim)));
                spans.extend(split_side_spans(
                    app,
                    new.as_ref(),
                    per_side,
                    current_match_row,
                    &match_rows,
                ));
                // Pair columns are width-exact (padded to per_side), so a
                // pair's notes always take the full-width fallback row.
                push_line_with_note_fallback(
                    &mut lines,
                    style_cursor(Line::from(spans), is_cursor, app.theme.cursor_bg),
                    false,
                    notes,
                    app.theme.note,
                    app.theme.dim,
                    None,
                );
            }
            other => {
                // Full-width rows (headers) can host an inline annotation.
                let line = stream_row_to_line(
                    app,
                    other,
                    current_match_row,
                    &match_rows,
                    area.width as usize,
                );
                let line = style_cursor(line, is_cursor, app.theme.cursor_bg);
                let (line, inline_ok) = match notes {
                    Some(notes) => {
                        append_inline_notes(line, notes, area.width as usize, app.theme.note, None)
                    }
                    None => (line, false),
                };
                push_line_with_note_fallback(
                    &mut lines,
                    line,
                    inline_ok,
                    notes,
                    app.theme.note,
                    app.theme.dim,
                    None,
                );
            }
        }
    }

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", title)),
    );
    frame.render_widget(para, area);
}

/// Build the spans for one side of a split row: dim right-aligned line-number
/// gutter, sign, then syntax-highlighted text truncated to the column width.
/// Empty sides pad with blanks so the divider stays aligned.
fn split_side_spans(
    app: &mut App,
    side: Option<&OwnedSide>,
    width: usize,
    current_match_row: Option<usize>,
    match_rows: &std::collections::HashSet<usize>,
) -> Vec<Span<'static>> {
    use unicode_width::UnicodeWidthStr;
    let Some(side) = side else {
        return vec![Span::raw(" ".repeat(width + 6 + 1))];
    };
    let is_current = current_match_row == Some(side.abs_row);
    let is_other_match = !is_current && match_rows.contains(&side.abs_row);

    let tint = row_tint(&app.theme, side.kind);
    let gutter = match side.line_no {
        Some(n) => format!("{n:>5} "),
        None => " ".repeat(6),
    };
    let (sign, kind_style) = match side.kind {
        DiffLineKind::Add => ('+', Style::default().fg(app.theme.add)),
        DiffLineKind::Delete => ('-', Style::default().fg(app.theme.delete)),
        DiffLineKind::Meta => ('\\', Style::default().fg(app.theme.dim)),
        DiffLineKind::Context => (' ', Style::default()),
    };

    let mut spans = vec![
        Span::styled(
            gutter,
            if is_current {
                // The match style is full: fg+bg, so the tint underneath is
                // hidden. Stack it anyway for symmetry with inactive matches.
                tint_style(
                    Style::default()
                        .fg(app.theme.match_active_fg)
                        .bg(app.theme.match_active_bg),
                    tint,
                )
            } else if is_other_match {
                tint_style(Style::default().bg(app.theme.match_inactive_bg), tint)
            } else {
                tint_style(Style::default().fg(app.theme.dim), tint)
            },
        ),
        Span::styled(sign.to_string(), tint_style(kind_style, tint)),
    ];

    // Syntax-highlighted text runs (viewport-only, cached) truncated to the
    // column width. A trailing "…" marks the cut on long lines.
    let file_and_line = ViewportQuery::file_and_line(&app.review, side.abs_row);
    let runs: Vec<(Style, String)> = if app.highlight_on {
        match file_and_line {
            Some((file_idx, li)) => {
                let path = app.review.display_path(file_idx).to_owned();
                if let Some(runs) = app.cache.try_get(file_idx, li) {
                    runs
                } else if let Some(tx) = app.hl_job_tx.as_ref() {
                    let _ = tx.send(crate::highlight::HighlightJob {
                        gen: app.cache.current_gen(),
                        file_idx,
                        line_in_file: li,
                        path,
                        text: side.text.clone(),
                        highlighter: Arc::clone(&app.highlighter),
                    });
                    vec![(Style::default(), side.text.clone())]
                } else {
                    app.cache
                        .get_or_highlight(file_idx, li, &path, &side.text, &app.highlighter)
                }
            }
            None => vec![(Style::default(), side.text.clone())],
        }
    } else {
        vec![(Style::default(), side.text.clone())]
    };

    // Agent attention marks paint the new-side column (Add/Context rows).
    let runs = match (&file_and_line, side.kind, side.line_no) {
        (Some((file_idx, _)), DiffLineKind::Add | DiffLineKind::Context, Some(n)) => {
            apply_highlight_marks(app, runs, *file_idx, Some(n), side.raw.as_deref())
        }
        _ => runs,
    };

    let mut used = 0usize;
    for (style, text) in runs {
        if used >= width {
            break;
        }
        let mut chunk = String::new();
        for ch in text.chars() {
            let w = ch.to_string().width();
            if used + w > width {
                break;
            }
            chunk.push(ch);
            used += w;
        }
        if !chunk.is_empty() {
            spans.push(Span::styled(chunk, tint_style(style, tint)));
        }
    }
    if used < side.text.width() && width > 0 {
        // The line was cut: show an ellipsis in any remaining space (or as
        // the last cell by trimming one char).
        if used < width {
            spans.push(Span::styled(
                "…".to_string(),
                tint_style(Style::default().fg(app.theme.dim), tint),
            ));
            used += 1;
        }
    }
    if used < width {
        let pad = " ".repeat(width - used);
        spans.push(match tint {
            Some(t) => Span::styled(pad, t),
            None => Span::raw(pad),
        });
    }
    spans
}

fn draw_stream_unified(app: &mut App, frame: &mut Frame, area: Rect) {
    // The pane title occupies the area's first row, so only the rows below
    // it are diff rows — materialize exactly what fits. Keeping this in step
    // with the viewport_height the run loop syncs is what lets max_scroll
    // reach the last diff line.
    let height = area.height.saturating_sub(1) as usize;
    let scroll_y = app.scroll_y;
    let viewport = Viewport {
        start: scroll_y,
        height,
    };

    // Collect owned row data so we can release the &app.review borrow before
    // mutating app.cache below for highlighting. Line numbers are resolved here
    // (they need the review) and carried on each OwnedRow.
    let owned_rows: Vec<OwnedRow> =
        ViewportQuery::rows_virtual(&app.review, viewport, &app.collapse)
            .into_iter()
            .map(|vr| OwnedRow::from_stream_row(&app.review, vr.row, vr.abs_row, app.tab_width))
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
    // attach to (line-level and hunk-level). Notes render as a right-aligned
    // inline annotation on the target row when there's room (wrap off), and
    // fall back to a dedicated row below — neither touches `stream_len`, so
    // scroll / search / hunk-jump indices stay stable.
    let notes_by_row = build_notes_by_row(app);

    let title = app.current_path().to_string();
    let cursor_row = if app.cursor_on {
        Some(app.cursor_stream_row())
    } else {
        None
    };
    let mut lines: Vec<Line> = Vec::with_capacity(owned_rows.len());
    for r in owned_rows {
        let abs_row = owned_row_abs(&r);
        let is_marker = matches!(r, OwnedRow::Unchanged { .. });
        let notes = if is_marker {
            None
        } else {
            notes_by_row.get(&abs_row)
        };
        // Only +/- lines carry a diff tint; it fills the row's leftover space
        // so changed rows read as full-width bars rather than isolated glyphs.
        let row_kind = match &r {
            OwnedRow::Line { kind, .. } => *kind,
            _ => DiffLineKind::Context,
        };
        let fill = row_fill(app, row_kind, abs_row, current_match_row);
        let line = stream_row_to_line(app, r, current_match_row, &match_rows, area.width as usize);
        let line = style_cursor(
            line,
            !is_marker && cursor_row == Some(abs_row),
            app.theme.cursor_bg,
        );
        // Markers share an abs_row with the following real row; only the
        // real row carries the note. With wrapping on there is no meaningful
        // "rest of the row", so notes always take the fallback row.
        let (line, inline_ok) = match notes {
            Some(notes) if !app.wrap_on => {
                append_inline_notes(line, notes, area.width as usize, app.theme.note, fill)
            }
            _ => (line, false),
        };
        push_line_with_note_fallback(
            &mut lines,
            line,
            inline_ok,
            notes,
            app.theme.note,
            app.theme.dim,
            fill.map(|t| (area.width as usize, t)),
        );
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
    // The pane title occupies the area's first row, so only the rows below
    // it are diff rows — materialize exactly what fits. Keeping this in step
    // with the viewport_height the run loop syncs is what lets max_scroll
    // reach the last diff line.
    let height = area.height.saturating_sub(1) as usize;
    let scroll_y = app.scroll_y;
    let viewport = Viewport {
        start: scroll_y,
        height,
    };

    let owned_rows: Vec<OwnedRow> =
        ViewportQuery::rows_virtual(&app.review, viewport, &app.collapse)
            .into_iter()
            .map(|vr| OwnedRow::from_stream_row(&app.review, vr.row, vr.abs_row, app.tab_width))
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

    let notes_by_row = build_notes_by_row(app);

    let cursor_row = if app.cursor_on {
        Some(app.cursor_stream_row())
    } else {
        None
    };
    let mut lines: Vec<Line> = Vec::new();
    // Group rows by file to produce old/new blocks per file.
    let mut file_rows: Vec<(usize, Vec<OwnedRow>)> = Vec::new();
    for r in owned_rows {
        let file_idx = match &r {
            OwnedRow::FileHeader { .. } => continue, // handled below
            OwnedRow::HunkHeader { file_idx, .. } => *file_idx,
            OwnedRow::Line { file_idx, .. } => *file_idx,
            OwnedRow::Unchanged { file_idx, .. } => *file_idx,
            OwnedRow::Pair { file_idx, .. } => *file_idx,
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
            if let OwnedRow::Unchanged { count, .. } = &r {
                // Collapsed unchanged runs render once, in the old block
                // (context is shared by both sides).
                lines.push(Line::from(Span::styled(
                    format!("  ··· {count} unchanged lines ···"),
                    Style::default()
                        .fg(app.theme.dim)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
            if let OwnedRow::Line {
                kind,
                text,
                raw,
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
                        raw: raw.clone(),
                        file_idx: *file_idx,
                        abs_row: *abs_row,
                        old_no: *old_no,
                        new_no: *new_no,
                        counterpart: counterpart.clone(),
                    },
                    current_match_row,
                    &match_rows,
                    area.width as usize,
                );
                // Stack columns have no inline margin (blocks are full
                // width); notes take the dedicated row, kept below the line
                // they annotate — same convention as the other layouts.
                let fill = row_fill(app, *kind, *abs_row, current_match_row);
                push_line_with_note_fallback(
                    &mut lines,
                    style_cursor(line, cursor_row == Some(*abs_row), app.theme.cursor_bg),
                    false,
                    notes_by_row.get(abs_row),
                    app.theme.note,
                    app.theme.dim,
                    fill.map(|t| (area.width as usize, t)),
                );
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
                raw,
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
                        raw: raw.clone(),
                        file_idx: *file_idx,
                        abs_row: *abs_row,
                        old_no: *old_no,
                        new_no: *new_no,
                        counterpart: counterpart.clone(),
                    },
                    current_match_row,
                    &match_rows,
                    area.width as usize,
                );
                let fill = row_fill(app, *kind, *abs_row, current_match_row);
                push_line_with_note_fallback(
                    &mut lines,
                    style_cursor(line, cursor_row == Some(*abs_row), app.theme.cursor_bg),
                    false,
                    notes_by_row.get(abs_row),
                    app.theme.note,
                    app.theme.dim,
                    fill.map(|t| (area.width as usize, t)),
                );
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

/// The absolute stream row an [`OwnedRow`] occupies. Used to look up notes.
fn owned_row_abs(row: &OwnedRow) -> usize {
    match row {
        OwnedRow::FileHeader { abs_row, .. } => *abs_row,
        OwnedRow::HunkHeader { abs_row, .. } => *abs_row,
        OwnedRow::Line { abs_row, .. } => *abs_row,
        OwnedRow::Unchanged { abs_row, .. } => *abs_row,
        OwnedRow::Pair { abs_row, .. } => *abs_row,
    }
}

/// Resolve each `--note` target to an absolute stream row and group the note
/// texts by that row. Banner notes are excluded here (they're shown in the
/// status bar, not the stream). Returns an empty map when there are no
/// line/hunk notes, so the fan-out is a no-op.
fn build_notes_by_row(app: &App) -> std::collections::HashMap<usize, Vec<String>> {
    let mut out: std::collections::HashMap<usize, Vec<String>> = std::collections::HashMap::new();
    if !app.show_notes {
        return out;
    }
    for note in &app.notes {
        if let Some(row) = crate::tui::app::note_stream_row(&app.review, &note.target) {
            out.entry(row).or_default().push(note.text.clone());
        }
    }
    out
}

/// Expand tabs to `tab_width`-column stops. Returns the input unchanged
/// (borrowed semantics: same `String`) when the text has no tab. Terminal
/// tab stops are 8-wide and terminal-dependent, which silently breaks the
/// split layout's column alignment — so tabs are expanded here, at render
/// time, under the configured width.
fn expand_tabs(text: &str, tab_width: usize) -> String {
    if !text.contains('\t') || tab_width == 0 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\t' {
            let next = (col / tab_width + 1) * tab_width;
            out.push_str(&" ".repeat(next - col));
            col = next;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

/// Map a char range (1-based, half-open, as used by `highlight add` and
/// consumed by [`overlay_runs_with_range`]) from the raw diff text onto the
/// tab-expanded rendered text. `raw` is the un-expanded line. The returned
/// pair stays 1-based half-open.
fn map_raw_range(raw: &str, tab_width: usize, start: usize, end: usize) -> (usize, usize) {
    if !raw.contains('\t') || tab_width == 0 {
        return (start, end);
    }
    // raw_starts[i] = expanded column of raw char i; the trailing entry is
    // the end column (raw length in expanded columns).
    let mut raw_starts: Vec<usize> = Vec::with_capacity(raw.chars().count() + 1);
    let mut col = 0usize;
    for ch in raw.chars() {
        raw_starts.push(col);
        if ch == '\t' {
            col = (col / tab_width + 1) * tab_width;
        } else {
            col += 1;
        }
    }
    raw_starts.push(col);
    let s = start.saturating_sub(1).min(raw_starts.len() - 1);
    let e = end.saturating_sub(1).min(raw_starts.len() - 1);
    (raw_starts[s] + 1, raw_starts[e] + 1)
}

/// Apply the review-cursor row background to a rendered line. Callers pass
/// `is_cursor` only for real rows (markers alias the following row's
/// `abs_row`). Span-level backgrounds (the diff row tint, the active search
/// match) still win: the cursor fills only cells whose spans carry no bg —
/// plus the empty-space padding span appended per row — so it reads as a
/// frame around the row without drowning the syntax colors.
fn style_cursor(line: Line<'static>, is_cursor: bool, cursor_bg: Color) -> Line<'static> {
    if is_cursor {
        line.style(Style::default().bg(cursor_bg))
    } else {
        line
    }
}

/// Background tint for a changed row, painted span-by-span so a search match
/// or an attention mark can still override it, and so the row reads as the
/// accent fill from the sign column all the way to the frame edge. `None`
/// for context/meta rows — they keep the terminal background.
fn row_tint(theme: &crate::tui::theme::Theme, kind: DiffLineKind) -> Option<Style> {
    match kind {
        DiffLineKind::Add => Some(Style::default().bg(theme.add_bg)),
        DiffLineKind::Delete => Some(Style::default().bg(theme.del_bg)),
        DiffLineKind::Context | DiffLineKind::Meta => None,
    }
}

/// The fill used behind inline notes and the tail stretch: the diff tint on
/// plain rows, but on the *current* search-match row the subdued match bg —
/// that row's restyle repaints every span (gold hit, subdued rest), and the
/// tint must not re-enter through the filler. Context/meta rows keep `None`.
fn row_fill(
    app: &App,
    kind: DiffLineKind,
    abs_row: usize,
    current_match_row: Option<usize>,
) -> Option<Style> {
    if current_match_row == Some(abs_row) {
        return row_tint(&app.theme, kind)
            .map(|_| Style::default().bg(app.theme.match_inactive_bg));
    }
    row_tint(&app.theme, kind)
}

/// Paint `style` under `base` (base wins on any field it sets), so the tint
/// is a backdrop for the syntax/mark style rather than replacing it.
fn tint_style(base: Style, tint: Option<Style>) -> Style {
    match tint {
        Some(t) => t.patch(base),
        None => base,
    }
}

/// Try to place a row's notes as a right-aligned inline annotation
/// (` 💬 text`) on the same rendered row — the note reads as attached to the
/// code it describes, like an editor diagnostic. Returns the (possibly
/// extended) line and whether the notes were placed; when they don't fit
/// (long code line, narrow terminal) the caller falls back to a dedicated
/// note row below via [`note_row`].
fn append_inline_notes(
    mut line: Line<'static>,
    notes: &[String],
    width: usize,
    note_color: Color,
    tint: Option<Style>,
) -> (Line<'static>, bool) {
    use unicode_width::UnicodeWidthStr;
    let note = format!(" 💬 {}", notes.join(" · "));
    let note_w = note.width();
    let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
    let free = width.saturating_sub(used);
    // Need the annotation plus one blank column of separation; cramming it
    // against the code would hurt readability more than the fallback row.
    if note_w + 1 > free {
        return (line, false);
    }
    let pad = free - note_w;
    if pad > 0 {
        line.spans.push(match tint {
            Some(t) => Span::styled(" ".repeat(pad), t),
            None => Span::raw(" ".repeat(pad)),
        });
    }
    line.spans.push(Span::styled(
        note,
        tint_style(
            Style::default()
                .fg(note_color)
                .add_modifier(Modifier::ITALIC),
            tint,
        ),
    ));
    (line, true)
}

/// A dedicated note row — the fallback when the inline annotation doesn't
/// fit. A slim note-colored bar marks it as annotation, never diff content.
fn note_row(text: &str, note_color: Color, dim: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled("  ╰─ ", Style::default().fg(dim)),
        Span::styled(
            format!("💬 {text}"),
            Style::default()
                .fg(note_color)
                .add_modifier(Modifier::ITALIC),
        ),
    ])
}

/// Push `line`, then any note rows for `abs_row` that did not fit inline.
/// `inline_ok` says whether the notes were already placed on the line by
/// [`append_inline_notes`] (callers pass `false` when wrapping is on or the
/// row type never goes inline).
fn push_line_with_note_fallback(
    lines: &mut Vec<Line<'static>>,
    mut line: Line<'static>,
    inline_ok: bool,
    notes: Option<&Vec<String>>,
    note_color: Color,
    dim_color: Color,
    fill_to: Option<(usize, Style)>,
) {
    // When no note went inline, the row stays at its natural length; stretch
    // the diff tint across the rest of the frame so +/- rows read as bars.
    if !inline_ok {
        if let Some((width, tint)) = fill_to {
            use unicode_width::UnicodeWidthStr;
            let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
            if width > used {
                line.spans
                    .push(Span::styled(" ".repeat(width - used), tint));
            }
        }
    }
    lines.push(line);
    if !inline_ok {
        if let Some(notes) = notes {
            for text in notes {
                lines.push(note_row(text, note_color, dim_color));
            }
        }
    }
}

/// Owned snapshot of a stream row's display data, so we can release the
/// `&app.review` borrow before mutating `app` for highlight caching.
enum OwnedRow {
    FileHeader {
        path: String,
        /// Source path for renames (`R` chips), surfaced as `(from old)`.
        old_path: Option<String>,
        kind: ChangeKind,
        inserts: u64,
        deletes: u64,
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
        /// The un-expanded line text when tab expansion changed it (used to
        /// remap attention-mark ranges from raw columns). `None` otherwise.
        raw: Option<String>,
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
    /// A collapsed run of unchanged lines (context run or implied inter-hunk
    /// gap), rendered as a dim marker row.
    Unchanged {
        file_idx: usize,
        count: usize,
        abs_row: usize,
    },
    /// One side-by-side row (split layout): the old-side and/or new-side
    /// code line, each with its resolved source line number.
    Pair {
        file_idx: usize,
        old: Option<OwnedSide>,
        new: Option<OwnedSide>,
        abs_row: usize,
    },
}

/// File-level change classification, from the IR's old/new paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl ChangeKind {
    /// The one-letter rail/header chip.
    fn letter(self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::Renamed => 'R',
        }
    }

    fn from_file(f: &crate::ir::FileDiff) -> ChangeKind {
        // Unified diffs mark absent sides as `/dev/null`; treat those as
        // None so added/deleted files classify correctly.
        let old = f.old_path.as_deref().filter(|p| *p != "/dev/null");
        let new = f.new_path.as_deref().filter(|p| *p != "/dev/null");
        match (old, new) {
            (None, Some(_)) => ChangeKind::Added,
            (Some(_), None) => ChangeKind::Deleted,
            (Some(old), Some(new)) if old != new => ChangeKind::Renamed,
            _ => ChangeKind::Modified,
        }
    }
}

/// One side of a materialized split row.
struct OwnedSide {
    kind: DiffLineKind,
    text: String,
    /// Un-expanded text when tab expansion changed it (see [`OwnedRow::Line`]).
    raw: Option<String>,
    line_no: Option<u32>,
    abs_row: usize,
}

impl OwnedRow {
    fn from_stream_row(review: &Review, row: StreamRow, abs_row: usize, tab_width: usize) -> Self {
        match row {
            StreamRow::FileHeader { file_idx, path } => {
                let f = &review.files[file_idx];
                OwnedRow::FileHeader {
                    path: path.to_string(),
                    old_path: f.old_path.clone().filter(|p| p != "/dev/null"),
                    kind: ChangeKind::from_file(f),
                    inserts: f.inserts,
                    deletes: f.deletes,
                    abs_row,
                }
            }
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
                // Tab expansion: the rendered text replaces tabs with
                // `tab_width`-column stops; the raw text is kept alongside so
                // attention-mark ranges (raw columns) can be remapped.
                let expanded = expand_tabs(text, tab_width);
                let raw = (expanded != text).then(|| text.to_string());
                let counterpart = counterpart.map(|c| expand_tabs(&c, tab_width));
                OwnedRow::Line {
                    kind,
                    text: expanded,
                    file_idx,
                    abs_row,
                    old_no,
                    new_no,
                    counterpart,
                    raw,
                }
            }
            StreamRow::Unchanged { file_idx, count } => OwnedRow::Unchanged {
                file_idx,
                count,
                abs_row,
            },
            StreamRow::Pair { file_idx, old, new } => {
                let side = |s: Option<crate::ir::PairSide<'_>>| {
                    s.map(|p| {
                        let (old_no, new_no) = ViewportQuery::row_line_numbers(review, p.abs_row)
                            .unwrap_or((None, None));
                        let expanded = expand_tabs(p.text, tab_width);
                        let raw = (expanded != p.text).then(|| p.text.to_string());
                        OwnedSide {
                            kind: p.kind,
                            text: expanded,
                            raw,
                            line_no: old_no.or(new_no),
                            abs_row: p.abs_row,
                        }
                    })
                };
                OwnedRow::Pair {
                    file_idx,
                    old: side(old),
                    new: side(new),
                    abs_row,
                }
            }
        }
    }
}

/// Render the file-header rule: change chip, path, and a right-aligned
/// +ins/−del tally with a proportional mini bar (GitHub-style), tied off
/// with a full-bleed `─` rule when the width allows.
fn file_header_line(
    app: &App,
    path: &str,
    old_path: Option<&str>,
    kind: ChangeKind,
    inserts: u64,
    deletes: u64,
    width: usize,
) -> Line<'static> {
    let header_style = Style::default()
        .fg(app.theme.file_header)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(app.theme.dim);
    let chip_color = match kind {
        ChangeKind::Added => app.theme.add,
        ChangeKind::Deleted => app.theme.delete,
        ChangeKind::Renamed | ChangeKind::Modified => app.theme.file_header,
    };

    let mut spans: Vec<Span<'static>> = vec![Span::styled("─── ", dim)];
    spans.push(Span::styled(
        kind.letter().to_string(),
        Style::default().fg(chip_color).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(format!(" {path} "), header_style));
    let origin = old_path
        .filter(|_| kind == ChangeKind::Renamed)
        .map(|old| format!(" (from {old})"));
    let origin_w = origin.as_ref().map_or(0, |o| o.chars().count());
    if let Some(o) = &origin {
        spans.push(Span::styled(o.clone(), dim));
    }

    // Stats + proportional bar (skipped for binary/no-line files). Zero
    // sides are omitted, mirroring the rail's per-file tally.
    if inserts > 0 || deletes > 0 {
        let (ins_cells, del_cells) = change_bar_cells(inserts, deletes, 10);
        let bar_ins = "█".repeat(ins_cells);
        let bar_del = "█".repeat(del_cells);
        let mut stats = String::new();
        if inserts > 0 {
            stats.push_str(&format!("+{inserts} "));
        }
        stats.push_str(&bar_ins);
        if inserts > 0 && deletes > 0 {
            stats.push(' ');
        }
        stats.push_str(&bar_del);
        if deletes > 0 {
            stats.push_str(&format!(" −{deletes}"));
        }
        let stats_w = stats.chars().count() + 2;
        let used = 4 + 1 + 1 + path.chars().count() + origin_w + 1;
        // Right-align the stats inside the rule, padded with ─ on wide panes.
        if width > used + stats_w + 2 {
            let pad = width - used - stats_w - 2;
            spans.push(Span::styled("─".repeat(pad), dim));
        } else {
            spans.push(Span::styled("─".repeat(2), dim));
        }
        spans.push(Span::styled(" ", Style::default()));
        if inserts > 0 {
            spans.push(Span::styled(
                format!("+{inserts} "),
                Style::default()
                    .fg(app.theme.add)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(bar_ins, Style::default().fg(app.theme.add)));
        }
        if inserts > 0 && deletes > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        if deletes > 0 {
            spans.push(Span::styled(bar_del, Style::default().fg(app.theme.delete)));
            spans.push(Span::styled(
                format!(" −{deletes}"),
                Style::default()
                    .fg(app.theme.delete)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        spans.push(Span::styled("───", dim));
    }
    Line::from(spans)
}

/// Split `total` bar cells between insert/delete proportional to the counts,
/// each side getting at least one cell when it is nonzero.
fn change_bar_cells(inserts: u64, deletes: u64, total: usize) -> (usize, usize) {
    let total_n = inserts + deletes;
    if total_n == 0 {
        return (0, 0);
    }
    let ins = ((inserts as f64 / total_n as f64) * total as f64).round() as usize;
    let ins = ins.min(total.saturating_sub(if deletes > 0 { 1 } else { 0 }));
    // A nonzero side always keeps at least one cell.
    let ins = if inserts > 0 { ins.max(1) } else { 0 };
    let del = if deletes > 0 {
        total.saturating_sub(ins).max(1)
    } else {
        0
    };
    (ins, del)
}

fn stream_row_to_line(
    app: &mut App,
    row: OwnedRow,
    current_match_row: Option<usize>,
    match_rows: &std::collections::HashSet<usize>,
    width: usize,
) -> Line<'static> {
    let abs_row = match &row {
        OwnedRow::Line { abs_row, .. } => *abs_row,
        _ => usize::MAX, // headers never match
    };
    let is_current_match = current_match_row == Some(abs_row);
    let is_other_match = !is_current_match && match_rows.contains(&abs_row);

    let line = match row {
        OwnedRow::FileHeader {
            path,
            old_path,
            kind,
            inserts,
            deletes,
            ..
        } => file_header_line(
            app,
            &path,
            old_path.as_deref(),
            kind,
            inserts,
            deletes,
            width,
        ),
        OwnedRow::Unchanged { count, .. } => Line::from(Span::styled(
            format!("  ··· {count} unchanged lines ···"),
            Style::default()
                .fg(app.theme.dim)
                .add_modifier(Modifier::ITALIC),
        )),
        OwnedRow::Pair { new, old, .. } => {
            // Unified/stack renderers never materialize Pair rows; if one
            // slips through, render the new side (fallback: old side).
            let s = new.as_ref().or(old.as_ref());
            Line::from(Span::styled(
                s.map(|x| x.text.clone()).unwrap_or_default(),
                Style::default(),
            ))
        }
        OwnedRow::HunkHeader {
            text,
            file_idx,
            hunk_idx,
            ..
        } => {
            // In --select mode, prefix the header with a decision marker so
            // the human can see at a glance which hunks they've ruled on.
            if app.select_mode {
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
            raw,
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

            // Agent attention marks (`highlight add`) paint their char range
            // on top of whatever styling the line already carries.
            let runs = if matches!(kind, DiffLineKind::Add | DiffLineKind::Context) {
                apply_highlight_marks(app, runs, file_idx, new_no, raw.as_deref())
            } else {
                runs
            };

            // The add/delete row tint goes span-by-span (search-match re-slicing
            // and marks then patch over it). Padding to the frame edge keeps the
            // tint running under inline notes and to the end of short rows.
            let tint = row_tint(&app.theme, kind);
            let mut spans: Vec<Span> = Vec::with_capacity(runs.len() + 4);
            // Optional line-number gutter: " old new " right-aligned in 5 cols.
            if app.line_numbers_on {
                let dim = tint_style(Style::default().fg(app.theme.dim), tint);
                let old_s = old_no
                    .map(|n| format!("{n:>5}"))
                    .unwrap_or_else(|| "     ".into());
                let new_s = new_no
                    .map(|n| format!("{n:>5}"))
                    .unwrap_or_else(|| "     ".into());
                spans.push(Span::styled(format!(" {old_s} {new_s} "), dim));
            }
            spans.push(Span::styled(
                prefix.to_string(),
                tint_style(kind_style, tint),
            ));
            for (style, txt) in runs {
                spans.push(Span::styled(txt, tint_style(style, tint)));
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
    use unicode_width::UnicodeWidthStr;
    let width = area.width as usize;
    let pos = if app.collapse.virtual_len() == 0 {
        "0/0".to_string()
    } else {
        format!("{}/{}", app.scroll_y + 1, app.collapse.virtual_len())
    };
    let hl = if app.highlight_on { " HL" } else { "" };
    // Per-file and total +/- tallies (green inserts, red deletes).
    let file = app.review.files.get(app.selected_file);
    let file_stats = match file {
        Some(f) => format!("+{}/−{}", f.inserts, f.deletes),
        None => String::new(),
    };
    let non_path_suffix = format!(
        "  [{}]  {}  {}{} ",
        app.selected_file + 1,
        pos,
        file_stats,
        hl
    );
    let totals = format!(" Σ +{}/−{} ", app.review.inserts, app.review.deletes);
    let right = format!(" {} ", app.status);
    // A banner note (`--note banner=text`) is surfaced in the status bar so the
    // human sees the agent's high-level summary without scrolling.
    let banner: Option<String> = app
        .notes
        .iter()
        .find(|n| matches!(n.target, crate::tui::app::NoteTarget::Banner))
        .map(|n| format!(" 💬 {} ", n.text));
    // Everything between the totals and the toast: search indicator, toggle
    // badges, run-mode badges, banner.
    let mut mid: Vec<Span> = Vec::new();
    // Persistent search indicator: which match is on screen and how many
    // there are. The one-shot "match 3/17" toast is gone the moment anything
    // else writes a status message; this stays for the whole search session
    // and paints in the active-match colors so it reads as "you are here".
    if app.search.active && !app.search.matches.is_empty() {
        let query = truncate_to_width(app.search.query.as_str(), 12);
        let indicator = format!(
            " /{} {}/{} ",
            query,
            app.search.current + 1,
            app.search.matches.len()
        );
        mid.push(Span::styled(
            indicator,
            Style::default()
                .fg(app.theme.match_active_fg)
                .bg(app.theme.match_active_bg)
                .add_modifier(Modifier::BOLD),
        ));
    }
    // Toggle-state badges: which view transforms are currently active, so a
    // toggle's effect outlives its 4-second toast. Each badge is
    // self-describing — `WS` = ignoring whitespace, `wd−` = word diff off
    // (on is the default), `zx−` = context collapse off, `split`/`stack` =
    // side-by-side/stacked layout; `HL` keeps its long-standing on-badge.
    let badge = Style::default().fg(app.theme.edit_mode_fg);
    if app.ignore_ws {
        mid.push(Span::styled(" WS", badge));
    }
    if !app.word_diff_on {
        mid.push(Span::styled(" wd−", badge));
    }
    if !app.collapse_on {
        mid.push(Span::styled(" zx−", badge));
    }
    if app.wrap_on {
        mid.push(Span::styled(" wrap", badge));
    }
    // A two-key sequence is armed (`]`/`[`/`z`): show which one, persistently
    // — the arming toast expires after 4 seconds but the prefix waits forever.
    if let Some(p) = app.pending_prefix {
        mid.push(Span::styled(format!(" {p}…"), badge));
    }
    match app.effective_layout() {
        crate::config::LayoutMode::Split => mid.push(Span::styled(" split", badge)),
        crate::config::LayoutMode::Stack => mid.push(Span::styled(" stack", badge)),
        crate::config::LayoutMode::Unified | crate::config::LayoutMode::Auto => {}
    }
    // Run-mode badges: make the active mode discoverable. SELECT uses the
    // orange edit color (it's an action mode where a/r/u matter); WATCH/SERVE
    // share the cyan note color. Kept short (≤8 chars) so they don't crowd a
    // narrow status bar.
    if app.select_mode {
        mid.push(Span::styled(
            " SELECT ",
            Style::default()
                .fg(app.theme.edit_mode_fg)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if app.watch_mode {
        mid.push(Span::styled(" WATCH ", Style::default().fg(app.theme.note)));
    }
    if app.serve_mode {
        mid.push(Span::styled(" SERVE ", Style::default().fg(app.theme.note)));
    }
    if let Some(b) = banner {
        mid.push(Span::styled(b, Style::default().fg(app.theme.note)));
    }
    let mid_w: usize = mid.iter().map(|s| s.content.width()).sum();
    // Transient toasts (errors, confirmations — the thing a user most needs
    // to read) own the right edge: the path's budget shrinks to make room,
    // and the left side truncates only as a last resort. The sticky startup
    // hint and an empty status keep the old flow layout instead — a
    // 100-column hint must not push the path and tallies off the line.
    let transient = app.status.set_at.is_some() && !app.status.is_empty();
    let toast_w = if transient { right.width() } else { 0 };
    // The path absorbs squeeze first, down to a 4-column sliver.
    let room = width.saturating_sub(mid_w + toast_w + totals.width() + 1);
    let path_budget = (width / 2)
        .max(12)
        .min(room.saturating_sub(non_path_suffix.width()).max(4));
    let shown_path = status_path(app.current_path(), path_budget);
    let mut left = format!(" {}{}", shown_path, non_path_suffix);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(app.theme.dim);
    let build = |left: &str| -> Vec<Span<'static>> {
        let mut v = Vec::with_capacity(mid.len() + 2);
        v.push(Span::styled(left.to_string(), bold));
        v.push(Span::styled(totals.clone(), dim_style));
        v.extend(mid.iter().cloned());
        v
    };
    let mut spans = build(&left);
    if transient {
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        if used + toast_w > width {
            // Even a 4-column path sliver didn't save the line: shed the
            // whole left side down to what the furniture + toast leave.
            let keep = width
                .saturating_sub(mid_w + totals.width() + toast_w)
                .max(1);
            left = truncate_to_width(&left, keep);
            spans = build(&left);
        }
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        if used + toast_w <= width {
            spans.push(Span::raw(" ".repeat(width - used - toast_w)));
        }
    }
    // Color the status message by severity so errors (red) and confirmations
    // (green) stand out from the dim default.
    let status_style = match app.status.kind {
        ToastKind::Error => Style::default()
            .fg(app.theme.delete)
            .add_modifier(Modifier::BOLD),
        ToastKind::Success => Style::default().fg(app.theme.add),
        ToastKind::Info => Style::default().fg(app.theme.dim),
    };
    spans.push(Span::styled(right, status_style));
    let line = Line::from(spans);
    let para = Paragraph::new(line).style(Style::default().bg(app.theme.status_bg));
    frame.render_widget(para, area);
}

/// Render the help line, or an input prompt when editing search/filter/notes.
fn draw_help_or_prompt(app: &App, frame: &mut Frame, area: Rect) {
    // The ▌ caret sits at the prompt cursor (chars from the start), which
    // Left/Right/Home/End move — split the draft around it.
    let caret = |draft: &str, cursor: usize| -> (String, String) {
        let cursor = cursor.min(draft.chars().count());
        (
            draft.chars().take(cursor).collect(),
            draft.chars().skip(cursor).collect(),
        )
    };
    let content = match app.mode {
        InputMode::Search => {
            // Live feedback from the incremental search: where you are in
            // the match list (or that there's nothing) while still typing.
            let live = if app.search.query.trim().is_empty() {
                String::new()
            } else if app.search.matches.is_empty() {
                " · no match".to_string()
            } else {
                format!(
                    " · match {}/{}",
                    app.search.current + 1,
                    app.search.matches.len()
                )
            };
            let (head, tail) = caret(&app.search.query, app.prompt_cursor);
            format!("/{head}▌{tail}  (Enter confirm · Esc cancel · Ctrl-U/W edit{live})")
        }
        InputMode::Filter => {
            let (head, tail) = caret(&app.path_filter, app.prompt_cursor);
            format!("filter: {head}▌{tail}  (path substring · Enter confirm · Esc cancel · Ctrl-U/W edit)")
        }
        InputMode::Note => {
            let (head, tail) = caret(&app.note_draft, app.prompt_cursor);
            format!("note: {head}▌{tail}  (anchored to the cursor row · Enter save · Esc cancel · Ctrl-U/W edit)")
        }
        InputMode::Normal => {
            // Keymap-driven: the hint shows the *live* first key of each
            // action, so a remapped config never makes this line lie. When an
            // action is unbound entirely, its action name stands in (still
            // discoverable, never blank).
            use crate::tui::keymap::Action;
            let k = |a: Action| {
                app.keymap
                    .keys_for(a)
                    .first()
                    .cloned()
                    .unwrap_or_else(|| a.name().to_string())
            };
            // In `--select` mode the decision keys (a/r/u) are the primary
            // actions, so lead with them — the long base cheatsheet would push
            // them to the tail, where narrow-terminal truncation hides them.
            if app.select_mode {
                format!(
                    " {} accept · {} reject · {} undecided · {}/{} hunk · {}/{} cursor · {} note · {} search · {} help · {} quit ",
                    k(Action::AcceptHunk),
                    k(Action::RejectHunk),
                    k(Action::UndecideHunk),
                    k(Action::NextHunk),
                    k(Action::PrevHunk),
                    k(Action::CursorDown),
                    k(Action::CursorUp),
                    k(Action::ComposeNote),
                    k(Action::Search),
                    k(Action::Help),
                    k(Action::Quit),
                )
            } else {
                format!(
                    " {}/{} cursor · {}/{} half-page · {}/{} top/bottom · {}/{} hunk · {}/{} note · {} note here · {}/{} fold · {} ctx · {} file · {} rail · {} search · {} filter · {} open · {} hl · {} lines · {} word · {} ws · {} wrap · {} theme · {} help · {} quit ",
                    k(Action::CursorDown),
                    k(Action::CursorUp),
                    k(Action::HalfPageDown),
                    k(Action::HalfPageUp),
                    k(Action::GotoTop),
                    k(Action::GotoBottom),
                    k(Action::NextHunk),
                    k(Action::PrevHunk),
                    k(Action::NextNote),
                    k(Action::PrevNote),
                    k(Action::ComposeNote),
                    k(Action::FoldFile),
                    k(Action::UnfoldFile),
                    k(Action::ToggleContextCollapse),
                    k(Action::NextFile),
                    k(Action::ToggleRail),
                    k(Action::Search),
                    k(Action::FilterPaths),
                    k(Action::OpenEditor),
                    k(Action::ToggleHighlight),
                    k(Action::ToggleLineNumbers),
                    k(Action::ToggleWordDiff),
                    k(Action::ToggleIgnoreWhitespace),
                    k(Action::ToggleWrap),
                    k(Action::CycleThemeMode),
                    k(Action::Help),
                    k(Action::Quit),
                )
            }
        }
    };
    // Graceful truncation: a single-row Paragraph silently clips past the area
    // width, hiding the tail keys on narrow terminals. Truncate with an
    // ellipsis instead so the user can tell there's more (and `?` shows all).
    let content = truncate_to_width(&content, area.width as usize);
    let style = match app.mode {
        InputMode::Normal => Style::default().fg(app.theme.dim),
        _ => Style::default()
            .fg(app.theme.edit_mode_fg)
            .add_modifier(Modifier::BOLD),
    };
    let para = Paragraph::new(content).style(style);
    frame.render_widget(para, area);
}

/// Truncate `s` to fit within `width` display columns, appending a single
/// trailing `…` when it would overflow. Widths are measured in display
/// columns (unicode-width, like the rail/status truncation helpers): a CJK
/// hint or search query otherwise paints past its budget. `width == 0`
/// yields empty.
fn truncate_to_width(s: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    // Reserve one column for the ellipsis.
    let budget = width.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Full-screen keybinding reference, drawn on top of the review when the user
/// presses `?`. Rendered as a centered, bordered panel sized to its content
/// (clamped to the terminal); when the terminal is too short the panel
/// scrolls with `j`/`k` (or the wheel) instead of silently clipping the
/// sections near the bottom. Section headers use the hunk-header color so
/// they stand out without fighting the Flexoki chrome.
fn draw_help_overlay(app: &mut App, frame: &mut Frame) {
    let area = frame.area();

    let key = Style::default()
        .fg(app.theme.hunk_header)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(app.theme.dim);
    let head = Style::default()
        .fg(app.theme.file_header)
        .add_modifier(Modifier::BOLD);

    use crate::tui::keymap::Action;
    let rows = |rs: &[HelpRow]| help_rows(app, rs);

    let mut lines: Vec<Line<'static>> = Vec::new();
    push_help_section(
        &mut lines,
        "Navigation",
        &rows(&[
            HelpRow::Pair(
                Action::CursorDown,
                Action::CursorUp,
                "cursor down / up one row",
            ),
            HelpRow::Pair(
                Action::HalfPageDown,
                Action::HalfPageUp,
                "cursor half a page down / up",
            ),
            HelpRow::Pair(
                Action::PageForward,
                Action::PageBackward,
                "cursor a full page down / up",
            ),
            HelpRow::Pair(Action::GotoTop, Action::GotoBottom, "jump to top / bottom"),
            HelpRow::Pair(
                Action::NextHunk,
                Action::PrevHunk,
                "next / previous hunk (wraps files)",
            ),
            HelpRow::Pair(Action::NextFile, Action::PrevFile, "next / previous file"),
            HelpRow::Static(
                "1-9",
                "jump to the Nth file (absolute index, even while filtered)",
            ),
        ]),
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "Notes & files",
        &rows(&[
            HelpRow::Pair(
                Action::NextNote,
                Action::PrevNote,
                "next / previous note (💬 rows)",
            ),
            HelpRow::Action(Action::ComposeNote),
            HelpRow::Action(Action::OpenEditor),
            HelpRow::Pair(
                Action::FoldFile,
                Action::UnfoldFile,
                "fold / unfold current file",
            ),
            HelpRow::Action(Action::ToggleRail),
        ]),
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "View",
        &rows(&[
            HelpRow::Action(Action::ToggleHighlight),
            HelpRow::Action(Action::ToggleLineNumbers),
            HelpRow::Action(Action::ToggleWordDiff),
            HelpRow::Action(Action::ToggleIgnoreWhitespace),
            HelpRow::Action(Action::ToggleWrap),
            HelpRow::Action(Action::ToggleContextCollapse),
            HelpRow::Action(Action::CycleLayout),
            HelpRow::Action(Action::CycleThemeMode),
            HelpRow::Action(Action::CyclePalette),
        ]),
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "Search & filter",
        &rows(&[
            HelpRow::Action(Action::Search),
            HelpRow::Pair(
                Action::NextMatch,
                Action::PrevMatch,
                "next / previous match",
            ),
            HelpRow::Action(Action::FilterPaths),
        ]),
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "Session",
        &rows(&[HelpRow::Action(Action::Help), HelpRow::Action(Action::Quit)]),
        head,
        key,
        dim,
    );
    push_help_section(
        &mut lines,
        "Agent (--select)",
        &rows(&[
            HelpRow::Action(Action::AcceptHunk),
            HelpRow::Action(Action::RejectHunk),
            HelpRow::Action(Action::UndecideHunk),
        ]),
        head,
        key,
        dim,
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " j/k scroll · ? / Esc / q / Enter / Space  dismiss this help",
        Style::default().fg(app.theme.edit_mode_fg),
    )));

    // Size the panel to its content, clamped to the terminal with a 1-row
    // margin. When the content still doesn't fit, the panel scrolls (the
    // scroll is clamped here so a terminal resize can't strand the view
    // past the end).
    let width = 64u16.min(area.width.saturating_sub(2));
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = centered_rect(width, height, area);

    // Clear the underlying cells so the overlay reads as a floating panel.
    frame.render_widget(Clear, popup);

    let inner_h = popup.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_h).min(u16::MAX as usize) as u16;
    app.help_scroll = app.help_scroll.min(max_scroll);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.hunk_header))
        .title(Span::styled(
            " next-hunk — keybindings ",
            Style::default()
                .fg(app.theme.file_header)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.status_bg));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((app.help_scroll, 0))
        .alignment(Alignment::Left);
    frame.render_widget(para, popup);
}

/// Push a titled group of keybinding rows into the help-overlay line list.
/// Uses `&'static str` so the built `Line<'static>` borrows the literal text
/// directly (no allocation, no lifetime knot from a closure).
fn push_help_section(
    lines: &mut Vec<Line<'static>>,
    title: &str,
    rows: &[(String, String)],
    head: Style,
    key: Style,
    dim: Style,
) {
    // Key column width: 16–24. Longer key lists take their own row (below)
    // so the description column never gets squeezed into clipping.
    let key_w = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(16, 24);
    lines.push(Line::from(Span::styled(format!(" {title}"), head)));
    for (k, d) in rows {
        if k.chars().count() > key_w {
            // Absurdly long key lists (deep remaps): keys on their own row,
            // description indented below — still readable.
            lines.push(Line::from(Span::styled(format!("  {k}"), key)));
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(key_w + 3)),
                Span::styled(d.clone(), dim),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(format!("  {k:<key_w$} "), key),
                Span::styled(d.clone(), dim),
            ]));
        }
    }
    lines.push(Line::from(""));
}

/// One help row: either a single action (keys come from the live keymap —
/// unbound actions are omitted), a pair sharing a description, or a static
/// non-remappable entry (e.g. `1-9`).
enum HelpRow {
    Action(crate::tui::keymap::Action),
    Pair(
        crate::tui::keymap::Action,
        crate::tui::keymap::Action,
        &'static str,
    ),
    Static(&'static str, &'static str),
}

/// Materialize help rows against `keymap`: action keys are looked up live so
/// the overlay always reflects the user's `[keybindings]`, never stale
/// defaults.
fn help_rows(app: &App, rows: &[HelpRow]) -> Vec<(String, String)> {
    use crate::tui::keymap::{Action, Keymap};
    let keymap = &app.keymap;
    let keys = |a: Action, km: &Keymap| {
        let k = km.keys_for(a);
        (!k.is_empty()).then(|| k.join(" / "))
    };
    let mut out = Vec::new();
    for row in rows {
        match row {
            HelpRow::Action(a) => {
                if let Some(k) = keys(*a, keymap) {
                    // Toggles carry their live state, e.g.
                    // "toggle syntax highlighting (on)" — the overlay then
                    // answers "what is on right now?" without leaving it.
                    let desc = match toggle_state(app, *a) {
                        Some(state) => format!("{} ({})", a.describe(), state),
                        None => a.describe().to_string(),
                    };
                    out.push((k, desc));
                }
            }
            HelpRow::Pair(a, b, desc) => {
                // Show whichever side is bound; if both are, join them.
                match (keys(*a, keymap), keys(*b, keymap)) {
                    (Some(ka), Some(kb)) => out.push((format!("{ka} / {kb}"), desc.to_string())),
                    (Some(ka), None) | (None, Some(ka)) => out.push((ka, desc.to_string())),
                    (None, None) => {}
                }
            }
            HelpRow::Static(k, d) => out.push((k.to_string(), d.to_string())),
        }
    }
    out
}

/// The live on/off label for toggle actions, shown in the help overlay.
/// `None` for actions without a stateful aspect.
fn toggle_state(app: &App, a: crate::tui::keymap::Action) -> Option<&'static str> {
    use crate::tui::keymap::Action;
    Some(match a {
        Action::ToggleHighlight if app.highlight_on => "on",
        Action::ToggleHighlight => "off",
        Action::ToggleLineNumbers if app.line_numbers_on => "on",
        Action::ToggleLineNumbers => "off",
        Action::ToggleWordDiff if app.word_diff_on => "on",
        Action::ToggleWordDiff => "off",
        Action::ToggleIgnoreWhitespace if app.ignore_ws => "on",
        Action::ToggleIgnoreWhitespace => "off",
        Action::ToggleWrap if app.wrap_on => "on",
        Action::ToggleWrap => "off",
        Action::ToggleContextCollapse if app.collapse_on => "on",
        Action::ToggleContextCollapse => "off",
        Action::ToggleRail if app.show_rail => "on",
        Action::ToggleRail => "off",
        // CycleLayout's description already lists its cycle and the effective
        // layout has its own status badge; a suffix here would clip at the
        // overlay's 64-column width.
        _ => return None,
    })
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

/// Background + foreground color for an attention-mark tone, mapped onto
/// theme slots so every palette (flexoki, catppuccin, …) gets a sensible
/// tone without new per-palette fields. The text is painted in `on_accent`
/// — readable over the solid accent fill in *both* background modes (deep
/// 600-level fills need light text, mid-tone 400-level fills need ink) —
/// except the light-gold warning fill, which pairs with the dark
/// `match_active_fg` ink.
fn tone_overlay(theme: &crate::tui::theme::Theme, tone: &str) -> Style {
    let (bg, fg) = match tone {
        "danger" => (theme.delete, theme.on_accent),
        "info" => (theme.hunk_header, theme.on_accent),
        "accent" => (theme.add, theme.on_accent),
        // "warning" and anything unrecognized: the active-match gold.
        _ => (theme.match_active_bg, theme.match_active_fg),
    };
    Style::default()
        .fg(fg)
        .bg(bg)
        .add_modifier(Modifier::UNDERLINED)
}

/// Split syntax runs so the char range `[start, end)` (1-based, `end`
/// exclusive — the indexing `highlight add --start/--end` uses) carries
/// `overlay`. Chars outside the range keep their style, so syntax
/// highlighting stays visible around the mark. Multiple marks compose by
/// calling this once per mark.
fn overlay_runs_with_range(
    runs: Vec<(Style, String)>,
    start: usize,
    end: usize,
    overlay: Style,
) -> Vec<(Style, String)> {
    // Convert to 0-based half-open [lo, hi) over the line's chars.
    let lo = start.saturating_sub(1);
    let hi = end.saturating_sub(1).max(lo);
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut cursor = 0usize; // char index of the next unprocessed char
    for (style, text) in runs {
        let len = text.chars().count();
        let run_lo = cursor;
        let run_hi = cursor + len;
        if run_hi <= lo || run_lo >= hi || len == 0 {
            // No overlap with the marked range.
            out.push((style, text));
            cursor = run_hi;
            continue;
        }
        // Overlap: split this run into up to three parts.
        let chars: Vec<char> = text.chars().collect();
        let inter_lo = lo.max(run_lo) - run_lo;
        let inter_hi = hi.min(run_hi) - run_lo;
        if inter_lo > 0 {
            out.push((style, chars[..inter_lo].iter().collect()));
        }
        if inter_hi > inter_lo {
            out.push((
                style.patch(overlay),
                chars[inter_lo..inter_hi].iter().collect(),
            ));
        }
        if len > inter_hi {
            out.push((style, chars[inter_hi..].iter().collect()));
        }
        cursor = run_hi;
    }
    out
}

/// Apply every attention mark that targets this row (same file + new-side
/// line). Returns the (possibly re-sliced) runs. No-op when no mark matches.
fn apply_highlight_marks(
    app: &App,
    runs: Vec<(Style, String)>,
    file_idx: usize,
    new_no: Option<u32>,
    raw: Option<&str>,
) -> Vec<(Style, String)> {
    let Some(line) = new_no else {
        return runs;
    };
    if app.highlights.is_empty() {
        return runs;
    }
    let path = app.review.display_path(file_idx);
    let mut runs = runs;
    for mark in &app.highlights {
        if mark.file == path && mark.line == line {
            // Mark ranges are raw-diff columns; when tab expansion shifted
            // the rendered text, translate them onto expanded columns first.
            let (start, end) = match raw {
                Some(raw) => map_raw_range(raw, app.tab_width, mark.start, mark.end),
                None => (mark.start, mark.end),
            };
            runs = overlay_runs_with_range(runs, start, end, tone_overlay(&app.theme, &mark.tone));
        }
    }
    runs
}

/// Truncate a path for the rail display: at most `max_cols` display columns
/// (CJK chars count double), biased toward the trailing filename — that's
/// what identifies the file to the reviewer. Cutting walks char boundaries,
/// so a long non-ASCII path truncates instead of panicking on a byte-index
/// slice.
fn short_path(path: &str, max_cols: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if max_cols == 0 || path.width() <= max_cols {
        return path.to_string();
    }
    if let Some(idx) = path.rfind('/') {
        let last = &path[idx + 1..];
        if last.width() + 2 <= max_cols {
            // "…/" + basename fits: prefer the natural form.
            return format!("…/{}", last);
        }
        return truncate_tail_width(last, max_cols);
    }
    truncate_tail_width(path, max_cols)
}

/// Keep the trailing `max_cols - 1` display columns of `s` behind a leading
/// ellipsis, cutting only at char boundaries. `max_cols <= 1` yields just the
/// ellipsis.
fn truncate_tail_width(s: &str, max_cols: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let budget = max_cols.saturating_sub(1);
    let mut taken = 0usize;
    let mut start = s.len();
    for (i, c) in s.char_indices().rev() {
        let w = c.to_string().width();
        if taken + w > budget {
            break;
        }
        taken += w;
        start = i;
    }
    format!("…{}", &s[start..])
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

/// Truncate a path for the status bar, capped at `max_cols` display columns
/// and biased toward keeping the trailing filename (that's what identifies the
/// file to the reviewer). When the path fits, it's returned unchanged. When it
/// doesn't, we prefer `…/<basename>` (reads naturally) and fall back to a
/// leading ellipsis + the trailing columns. Measured in display width — a CJK
/// path costs two columns per char — and cut at char boundaries, so long
/// non-ASCII paths truncate instead of overflowing the budget or panicking.
fn status_path(path: &str, max_cols: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if max_cols == 0 || path.width() <= max_cols {
        return path.to_string();
    }
    if let Some(slash) = path.rfind('/') {
        let basename = &path[slash + 1..];
        // "…/" + basename
        if basename.width() + 2 <= max_cols {
            return format!("…/{}", basename);
        }
    }
    truncate_tail_width(path, max_cols)
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

    // ---- highlight mark overlays ----

    /// Concatenate a run list into a mask: '#' where the char carries the
    /// given bg, '.' where it doesn't.
    fn runs_to_mask(runs: &[(Style, String)], bg: Color) -> String {
        let mut out = String::new();
        for (style, text) in runs {
            for _ in text.chars() {
                out.push(if style.bg == Some(bg) { '#' } else { '.' });
            }
        }
        out
    }

    #[test]
    fn overlay_marks_exact_char_range() {
        let runs = vec![(Style::default(), "let x = 1;".to_string())];
        let out = overlay_runs_with_range(runs, 5, 10, Style::default().bg(Color::Yellow));
        // chars 5..=9 ("x = 1") marked, the rest plain
        assert_eq!(runs_to_mask(&out, Color::Yellow), "....#####.");
        // text round-trips unchanged
        let text: String = out.iter().map(|(_, t)| t.clone()).collect();
        assert_eq!(text, "let x = 1;");
    }

    #[test]
    fn overlay_splits_across_syntax_runs() {
        // Two syntax runs: "abc" + "def" — mark chars 2..5 ("bc" + "d").
        let runs = vec![
            (Style::default().fg(Color::Red), "abc".to_string()),
            (Style::default().fg(Color::Blue), "def".to_string()),
        ];
        let out = overlay_runs_with_range(runs, 2, 5, Style::default().bg(Color::Yellow));
        assert_eq!(runs_to_mask(&out, Color::Yellow), ".###..");
        // base fg colors survive outside the mark
        assert_eq!(out[0].0.fg, Some(Color::Red));
        assert_eq!(out[out.len() - 1].0.fg, Some(Color::Blue));
    }

    #[test]
    fn overlay_out_of_range_and_empty_marks_are_noops() {
        let runs = vec![(Style::default(), "abc".to_string())];
        let out = overlay_runs_with_range(runs.clone(), 9, 12, Style::default().bg(Color::Yellow));
        assert_eq!(runs_to_mask(&out, Color::Yellow), "...");
        let out = overlay_runs_with_range(runs, 2, 2, Style::default().bg(Color::Yellow));
        assert_eq!(runs_to_mask(&out, Color::Yellow), "...");
    }

    #[test]
    fn overlay_composes_multiple_marks() {
        let runs = vec![(Style::default(), "abcdef".to_string())];
        let once = overlay_runs_with_range(runs, 1, 3, Style::default().bg(Color::Yellow));
        let twice = overlay_runs_with_range(once, 5, 6, Style::default().bg(Color::Red));
        assert_eq!(runs_to_mask(&twice, Color::Yellow), "##....");
        assert_eq!(runs_to_mask(&twice, Color::Red), "....#.");
    }

    // ---- config parity: tab_width / sidebar / agent_notes ----

    #[test]
    fn tabs_expand_to_configured_width() {
        let patch = "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-\tlet x = 1;
+\tlet y = 2;
";
        let review = parse_unified_diff(patch).unwrap();

        // default tab_width = 4 → one leading tab becomes 4 spaces
        let mut app = App::with_highlighter(review.clone(), highlighter());
        app.viewport_height = 20;
        let backend = ratatui::backend::TestBackend::new(60, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains("+    let y = 2;"),
            "tab renders as 4 spaces under default tab_width: {rendered:?}"
        );

        // tab_width = 2 → 2 spaces
        let mut app = App::with_highlighter(review, highlighter());
        app.tab_width = 2;
        app.viewport_height = 20;
        let backend = ratatui::backend::TestBackend::new(60, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains("+  let y = 2;"),
            "tab renders as 2 spaces under tab_width = 2: {rendered:?}"
        );
    }

    #[test]
    fn split_layout_keeps_tabbed_columns_aligned() {
        // Both sides tab-indented: expansion must apply symmetrically so the
        // split divider stays put (the reason tab_width exists).
        let patch = "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-\talpha = 1;
+\tbeta = 2;
";
        let review = parse_unified_diff(patch).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 20;
        app.layout_mode = crate::config::LayoutMode::Split;
        let backend = ratatui::backend::TestBackend::new(140, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains("+    beta = 2;") && rendered.contains("-    alpha = 1;"),
            "both split sides expand tabs to the same stop: {rendered:?}"
        );
    }

    #[test]
    fn attention_mark_range_follows_tab_expansion() {
        // A mark on the raw tab (char 1..2) paints the full 4-column expanded
        // tab on screen — ranges are raw-diff columns, rendering is expanded.
        let patch = "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+\tnew
";
        let review = parse_unified_diff(patch).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 20;
        app.highlights.push(crate::tui::app::HighlightMark {
            id: "hl0".into(),
            file: "a.rs".into(),
            line: 1,
            start: 1,
            end: 2,
            tone: "danger".into(),
        });
        let backend = ratatui::backend::TestBackend::new(40, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        let danger = app.theme.delete;
        // Row 3 is the +new line; the 4 expanded tab cells carry the tone.
        let marked = buf
            .content()
            .iter()
            .skip(3 * buf.area.width as usize)
            .filter(|c| c.bg == danger)
            .count();
        assert_eq!(marked, 4, "raw 1-char tab mark paints all 4 expanded cells");
    }

    // ---- diff row tint (add_bg / del_bg) ----

    /// Rendered text rows and per-cell backgrounds as parallel grids
    /// (`rows[y]` / `bgs[y][x]`), so tests can locate a row by content and
    /// then inspect the paint that landed on it.
    fn render_grid(app: &mut App, w: u16, h: u16) -> (Vec<String>, Vec<Vec<Option<Color>>>) {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(app, f)).unwrap();
        let buf = terminal.backend().buffer();
        let width = buf.area.width as usize;
        let cells: Vec<Vec<_>> = buf.content().chunks(width).map(<[_]>::to_vec).collect();
        let rows = cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .collect()
            })
            .collect();
        let bgs = cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| c.style().bg)
                    .collect::<Vec<Option<Color>>>()
            })
            .collect();
        (rows, bgs)
    }

    /// `-old` / `+new` / ` ctx`: one row of each kind in a single hunk.
    fn tint_sample_app() -> App {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
-old
+new
 ctx
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 20;
        app
    }

    #[test]
    fn add_del_rows_tint_full_width_in_unified() {
        let mut app = tint_sample_app();
        let (add_bg, del_bg) = (app.theme.add_bg, app.theme.del_bg);
        let (rows, bgs) = render_grid(&mut app, 60, 10);
        let y_of = |needle: &str| {
            rows.iter()
                .position(|r| r.contains(needle))
                .unwrap_or_else(|| panic!("{needle} row rendered: {rows:?}"))
        };
        // The tint stretches from the sign to the right frame edge (the pad
        // span), so changed rows read as full-width bars.
        let y_add = y_of("+new");
        assert_eq!(bgs[y_add][59], Some(add_bg), "add tint reaches the edge");
        assert!(
            bgs[y_add].iter().filter(|b| **b == Some(add_bg)).count() > 20,
            "add tint covers the stream width"
        );
        let y_del = y_of("-old");
        assert_eq!(bgs[y_del][59], Some(del_bg), "del tint reaches the edge");
        // Context and meta rows keep the terminal background.
        for needle in ["ctx", "@@"] {
            let y = y_of(needle);
            assert!(
                !bgs[y]
                    .iter()
                    .any(|b| *b == Some(add_bg) || *b == Some(del_bg)),
                "{needle} row must not carry a diff tint"
            );
        }
    }

    #[test]
    fn split_pair_row_carries_tint_on_both_sides() {
        let mut app = tint_sample_app();
        app.layout_mode = crate::config::LayoutMode::Split;
        let (add_bg, del_bg) = (app.theme.add_bg, app.theme.del_bg);
        let (rows, bgs) = render_grid(&mut app, 140, 10);
        // One pair row, two tints: the old side washes red, the new side green.
        let y_pair = rows
            .iter()
            .position(|r| r.contains("-old") && r.contains("+new"))
            .expect("pair row rendered");
        assert!(bgs[y_pair].iter().filter(|b| **b == Some(del_bg)).count() > 20);
        assert!(bgs[y_pair].iter().filter(|b| **b == Some(add_bg)).count() > 20);
        // The trailing context row renders full-width and untinted.
        let y_ctx = rows
            .iter()
            .position(|r| r.contains("ctx"))
            .expect("ctx row rendered");
        assert!(
            bgs[y_ctx]
                .iter()
                .all(|b| *b != Some(add_bg) && *b != Some(del_bg)),
            "ctx row must not carry a diff tint"
        );
    }

    #[test]
    fn stack_rows_tint_full_width() {
        let mut app = tint_sample_app();
        app.layout_mode = crate::config::LayoutMode::Stack;
        let (add_bg, del_bg) = (app.theme.add_bg, app.theme.del_bg);
        let (rows, bgs) = render_grid(&mut app, 80, 10);
        let y_of = |needle: &str| {
            rows.iter()
                .position(|r| r.contains(needle))
                .unwrap_or_else(|| panic!("{needle} row rendered: {rows:?}"))
        };
        assert_eq!(bgs[y_of("+new")][79], Some(add_bg));
        assert_eq!(bgs[y_of("-old")][79], Some(del_bg));
    }

    #[test]
    fn active_search_match_overrides_row_tint() {
        let mut app = tint_sample_app();
        // Stream rows: 0 file header, 1 hunk header, 2 -old, 3 +new, 4 ctx.
        app.search.query = "new".into();
        app.search.matches = vec![3];
        app.search.current = 0;
        app.search.active = true;
        let (match_bg, add_bg) = (app.theme.match_active_bg, app.theme.add_bg);
        let (rows, bgs) = render_grid(&mut app, 60, 10);
        let y = rows
            .iter()
            .position(|r| r.contains("+new"))
            .expect("+new row rendered");
        // The matched substring goes gold, and the match restyle (active bg on
        // the hit, subdued elsewhere) replaces the tint span-by-span — search
        // visibility beats the row decoration.
        assert!(bgs[y].iter().filter(|b| **b == Some(match_bg)).count() >= 3);
        assert!(!bgs[y].contains(&Some(add_bg)));
    }

    #[test]
    fn agent_notes_false_hides_note_rendering() {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs
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
        app.notes.push(crate::tui::app::Note {
            target: crate::tui::app::NoteTarget::Line {
                path: "a.rs".into(),
                line: 1,
            },
            text: "look here".into(),
        });
        app.show_notes = false;
        let backend = ratatui::backend::TestBackend::new(60, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(!rendered.contains('💬'), "notes hidden: no 💬 anywhere");
        assert!(!rendered.contains("look here"));

        // toggle back on → the note row renders again
        app.show_notes = true;
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(rendered.contains('💬') && rendered.contains("look here"));
    }

    #[test]
    fn sidebar_false_starts_without_the_rail() {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs
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
        app.show_rail = false;
        let backend = ratatui::backend::TestBackend::new(60, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        assert!(app.rail_rect.is_none(), "rail hidden when sidebar = false");
        // and the stream spans the full width (file header present)
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(rendered.contains("a.rs"));
    }

    #[test]
    fn highlight_mark_paints_inline_in_the_stream() {
        // End-to-end: a mark on the +new line renders with the tone's bg on
        // exactly the marked chars (drawn through the full view pipeline).
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
        app.highlights.push(crate::tui::app::HighlightMark {
            id: "hl0".into(),
            file: "a.rs".into(),
            line: 1,
            start: 5, // "value"
            end: 10,
            tone: "danger".into(),
        });
        let backend = ratatui::backend::TestBackend::new(40, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let danger = app.theme.delete;
        // Row 3 is the +new line (0 rail/status, 1 file header, 2 hunk
        // header). The marked word "value" must carry the danger bg; the
        // unmarked "new" prefix on the same row must not.
        let mut marked = 0;
        let mut unmarked = 0;
        for cell in buf.content().iter().skip(3 * buf.area.width as usize) {
            if cell.symbol() == " " || cell.symbol().is_empty() {
                continue;
            }
            if cell.bg == danger {
                marked += 1;
            } else {
                unmarked += 1;
            }
        }
        assert_eq!(marked, 5, "exactly the 5 marked chars carry the tone bg");
        assert!(unmarked > 0, "unmarked chars on the row stay plain");
    }

    #[test]
    fn attention_mark_text_is_readable_on_light_theme() {
        // The light palettes fill marks with deep 600-level accents; without
        // an explicit foreground the code text (dark on light) vanished into
        // the fill. Every marked cell must now carry the palette's
        // `on_accent` ink at a WCAG contrast of ≥ 3 against the fill.
        use crate::tui::theme::test_support::contrast;
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
        // The app default is Flexoki dark; pin the light palette explicitly
        // under test.
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 20;
        app.theme = crate::tui::theme::Theme::light();
        app.highlights.push(crate::tui::app::HighlightMark {
            id: "hl0".into(),
            file: "a.rs".into(),
            line: 1,
            start: 5, // "value"
            end: 10,
            tone: "danger".into(),
        });
        let backend = ratatui::backend::TestBackend::new(40, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let (fill, ink) = (app.theme.delete, app.theme.on_accent);
        assert!(
            contrast(ink, fill) >= 3.0,
            "on_accent must contrast the danger fill (got {:.2})",
            contrast(ink, fill)
        );
        let mut marked = 0;
        for cell in buf.content().iter().skip(3 * buf.area.width as usize) {
            if cell.bg == fill {
                marked += 1;
                assert_eq!(
                    cell.fg, ink,
                    "marked cell must paint the on_accent ink, got {cell:?}"
                );
            }
        }
        assert_eq!(marked, 5, "the 5 marked chars carry the danger fill");
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
        let backend = ratatui::backend::TestBackend::new(80, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        // Assert inside the rail area only (x < 19 = rail inner width): the
        // stream pane's own file-header stats would otherwise mask a missing
        // rail tally (this test once passed for exactly that wrong reason).
        // The path budget reserves room for the tally, so it must render.
        let rail_row = |y: u16| -> String {
            (0..19u16)
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect()
        };
        assert!(
            rail_row(1).contains("+1") && rail_row(1).contains("−1"),
            "rail should show a.rs's +1/−1 tally: {}",
            rail_row(1)
        );
        assert!(
            rail_row(2).contains("+1") && rail_row(2).contains("−1"),
            "rail should show b.rs's +1/−1 tally: {}",
            rail_row(2)
        );
    }

    #[test]
    fn rail_marks_folded_files_with_a_chevron() {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs
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
        app.folded.insert(0); // a.rs folded, b.rs open
        let backend = ratatui::backend::TestBackend::new(40, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        // Rail rows start below the pane title (y=0): a.rs at y=1, b.rs at y=2.
        let row = |y: u16| -> String {
            (0..14u16)
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect()
        };
        assert!(row(1).contains('▸'), "folded a.rs marked ▸: {}", row(1));
        assert!(row(2).contains('▾'), "open b.rs marked ▾: {}", row(2));
    }

    #[test]
    fn status_bar_shows_armed_two_key_prefix() {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new value
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.pending_prefix = Some(']');
        let backend = ratatui::backend::TestBackend::new(80, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // The arming toast expires after 4s; the badge persists while the
        // prefix waits for its second key.
        assert!(rendered.contains(" ]…"), "armed-prefix badge: {rendered}");
    }

    #[test]
    fn status_bar_shows_toggle_state_badges() {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new value
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.ignore_ws = true;
        app.word_diff_on = false;
        app.collapse_on = false;
        app.wrap_on = true;
        app.layout_mode = crate::config::LayoutMode::Stack;
        let backend = ratatui::backend::TestBackend::new(80, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // Every non-default view transform leaves a persistent badge in the
        // status bar, outliving the toggle's 4-second toast.
        assert!(rendered.contains(" WS"), "ignore-ws badge: {rendered}");
        assert!(rendered.contains(" wd−"), "word-diff-off badge: {rendered}");
        assert!(rendered.contains(" zx−"), "collapse-off badge: {rendered}");
        assert!(rendered.contains(" wrap"), "wrap-on badge: {rendered}");
        assert!(
            rendered.contains(" stack"),
            "stack layout badge: {rendered}"
        );
    }

    #[test]
    fn help_overlay_shows_toggle_state() {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new value
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.wrap_on = true;
        app.show_help = true;
        let backend = ratatui::backend::TestBackend::new(80, 60);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        // Toggles answer "what is on right now?" in the overlay itself.
        assert!(
            rendered.contains("toggle line wrapping (on)"),
            "wrap toggle should show its live state: {rendered}"
        );
        assert!(
            rendered.contains("toggle ignore-whitespace view (off)"),
            "ws toggle should show its live state: {rendered}"
        );
    }

    #[test]
    fn transient_toast_right_aligns_on_narrow_terminals() {
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new value
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.set_error("save failed — try again later");
        let backend = ratatui::backend::TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer();
        // Status line is the second-to-last row (y = height - 2).
        let row: String = (0..60u16)
            .map(|x| buf[(x, 8u16)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            row.contains("try again later"),
            "transient toast must render in full: {row}"
        );
        assert!(
            row.trim_end().ends_with("later"),
            "toast should sit flush right: {row}"
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

    // ---- truncate_to_width (graceful hint/prompt truncation) ---------------

    #[test]
    fn truncate_to_width_unchanged_when_fits() {
        assert_eq!(truncate_to_width("abc", 10), "abc");
        assert_eq!(truncate_to_width("abc", 3), "abc");
    }

    #[test]
    fn truncate_to_width_appends_ellipsis_on_overflow() {
        assert_eq!(truncate_to_width("abcdef", 4), "abc…");
        assert_eq!(truncate_to_width("abcdefg", 3), "ab…");
    }

    #[test]
    fn truncate_to_width_zero_width_returns_empty() {
        assert_eq!(truncate_to_width("abc", 0), "");
    }

    #[test]
    fn truncate_to_width_measures_display_columns_not_chars() {
        use unicode_width::UnicodeWidthStr;
        // CJK chars are 2 columns each: 4 chars = 8 columns. A width-6
        // budget fits only 2 of them plus the ellipsis.
        let out = truncate_to_width("你好世界", 6);
        assert_eq!(out, "你好…");
        assert!(out.width() <= 6, "truncated text must fit its budget");
        // Exactly-fitting CJK text passes through unchanged.
        assert_eq!(truncate_to_width("你好", 4), "你好");
        // Zero-width chars (combining marks) never overflow the budget.
        let out = truncate_to_width("e\u{301}xample", 3);
        assert!(out.width() <= 3);
    }

    // ---- mode badge + context-aware hint (render) --------------------------

    /// Read a whole terminal row as a String (cells → first char each).
    fn row_text(buf: &ratatui::buffer::Buffer, y: u16, width: u16) -> String {
        (0..width)
            .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
            .collect()
    }

    #[test]
    fn select_mode_shows_status_badge_and_decision_hint() {
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
        app.select_mode = true;
        app.viewport_height = 6;
        let backend = ratatui::backend::TestBackend::new(80, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        // status row = height - 2 = 4; help row = height - 1 = 5
        let status = row_text(buf, 4, 80);
        let help = row_text(buf, 5, 80);
        assert!(
            status.contains("SELECT"),
            "select mode should surface a status-bar badge, got: {status:?}"
        );
        assert!(
            help.contains("a accept"),
            "select mode hint should mention the decision keys, got: {help:?}"
        );
    }

    #[test]
    fn select_mode_off_hides_decision_hint() {
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
        app.viewport_height = 6;
        let backend = ratatui::backend::TestBackend::new(80, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let help = row_text(terminal.backend().buffer(), 5, 80);
        assert!(
            !help.contains("a accept"),
            "decision keys should not show outside select mode, got: {help:?}"
        );
    }

    // ---- UI polish: change chips + stat bars ----

    fn rendered_lines(mut app: App, w: u16, h: u16) -> String {
        app.viewport_height = 30;
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect()
    }

    #[test]
    fn file_headers_show_change_chips_and_bars() {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
-old one
+new one
 context
diff --git a/added.rs b/added.rs
new file mode 100644
--- /dev/null
+++ b/added.rs
@@ -0,0 +1,2 @@
+one
+two
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
--- a/gone.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-a
-b
-c
",
        )
        .unwrap();
        let app = App::with_highlighter(review, highlighter());
        let rendered = rendered_lines(app, 110, 30);

        // Modified: M chip + both-side stats
        assert!(rendered.contains("─── M a.rs "), "M header: {rendered:?}");
        assert!(rendered.contains("+1 █████"), "ins bar: {rendered:?}");
        assert!(rendered.contains("−1"), "del stat: {rendered:?}");
        // Added: A chip, no delete side
        assert!(
            rendered.contains("─── A added.rs "),
            "A header: {rendered:?}"
        );
        assert!(!rendered.contains("A added.rs ─── +2 █████ █ −0"));
        // Deleted: D chip, no insert side, no "+0"
        assert!(
            rendered.contains("─── D gone.rs "),
            "D header: {rendered:?}"
        );
        assert!(!rendered.contains("D gone.rs ─── +0"), "no +0 noise");
        assert!(rendered.contains("−3"), "delete stat present");
        // Rail carries the chips
        assert!(rendered.contains("M a.rs"), "rail M: {rendered:?}");
        assert!(rendered.contains("A added.rs"), "rail A: {rendered:?}");
        assert!(rendered.contains("D gone.rs"), "rail D: {rendered:?}");
    }

    #[test]
    fn change_bar_cells_split_proportionally() {
        assert_eq!(change_bar_cells(1, 4, 10), (2, 8));
        assert_eq!(change_bar_cells(3, 0, 10), (10, 0));
        assert_eq!(change_bar_cells(0, 3, 10), (0, 10));
        assert_eq!(change_bar_cells(0, 0, 10), (0, 0));
        // tiny counts still show one cell per nonzero side
        assert_eq!(change_bar_cells(1, 100, 10), (1, 9));
    }

    #[test]
    fn note_fallback_row_uses_tree_connector() {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-a very long line that leaves no room for an inline annotation in narrow terminals ok
+new
",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.notes.push(crate::tui::app::Note {
            target: crate::tui::app::NoteTarget::Line {
                path: "a.rs".into(),
                line: 1,
            },
            text: "check this".into(),
        });
        let rendered = rendered_lines(app, 40, 12);
        // 💬 is double-width: the flattened buffer inserts its continuation
        // cell as a space, so match the connector and text separately.
        assert!(
            rendered.contains("╰─ 💬") && rendered.contains("check this"),
            "note card connector: {rendered:?}"
        );
    }

    #[test]
    fn help_overlay_reflects_remapped_keys() {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs
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
        app.show_help = true;
        // Remap quit to Q and search to "ctrl-s"
        let mut cfg = std::collections::HashMap::new();
        cfg.insert("quit".to_string(), toml::Value::String("Q".into()));
        cfg.insert("search".to_string(), toml::Value::String("ctrl-s".into()));
        let (km, warns) = crate::tui::keymap::Keymap::with_overrides(&cfg);
        assert!(warns.is_empty(), "{warns:?}");
        app.keymap = km;

        let backend = ratatui::backend::TestBackend::new(80, 45);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains("Q"),
            "remapped quit key listed: {rendered:?}"
        );
        assert!(
            rendered.contains("ctrl-s"),
            "remapped search key listed: {rendered:?}"
        );
        // the replaced default for quit (q / esc) no longer shows as quit's keys
        let quit_row = rendered
            .lines()
            .find(|l| l.contains("quit (clears"))
            .unwrap_or_else(|| panic!("quit row present: {rendered:?}"));
        assert!(!quit_row.trim_start().starts_with("q"), "{quit_row:?}");
    }

    #[test]
    fn help_line_truncates_with_ellipsis_on_narrow_terminal() {
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
        app.viewport_height = 6;
        // 30 cols — far narrower than the ~150-char cheatsheet. The help row
        // must be truncated with an ellipsis rather than silently clipped.
        let backend = ratatui::backend::TestBackend::new(30, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let help = row_text(terminal.backend().buffer(), 5, 30);
        assert!(
            help.trim_end().ends_with('…'),
            "expected the help line to end with an ellipsis on a narrow terminal, got: {help:?}"
        );
    }

    #[test]
    fn error_toast_is_painted_in_the_delete_color() {
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
        app.set_error("ZZBOOMZZ");
        app.viewport_height = 6;
        let backend = ratatui::backend::TestBackend::new(80, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let status_row = 4u16;
        // Find at least one cell whose glyph is part of the error token and
        // whose foreground is the delete (red) color — proving severity drives
        // the status-message styling, not just the dim default. `Color` derives
        // PartialEq, so a direct comparison to the theme's delete slot suffices.
        let red = app.theme.delete;
        let painted_red = (0..80u16).any(|x| {
            let cell = &buf[(x, status_row)];
            cell.symbol() == "Z" && cell.fg == red
        });
        assert!(
            painted_red,
            "expected the error toast text to be painted in the delete color"
        );
    }

    // ---- UI polish round 2: scrollbar, search indicator, width-safe paths --

    #[test]
    fn overflowing_stream_shows_scrollbar() {
        // More virtual rows than viewport rows → a scrollbar rides the right
        // edge of the main area (track + thumb in the reserved column).
        let mut patch =
            String::from("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,4 +1,4 @@\n");
        for i in 0..14 {
            patch.push_str(&format!("+line {i}\n"));
        }
        let review = parse_unified_diff(&patch).unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 6;
        let backend = ratatui::backend::TestBackend::new(40, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        // Main area spans rows 0..6 (8 - status - help). The scrollbar lives
        // in the rightmost column (x = 39).
        let marked = (0..6u16)
            .filter(|&y| {
                let c = buf[(39, y)].symbol();
                c == "│" || c == "█"
            })
            .count();
        assert!(marked > 0, "expected scrollbar cells on the right edge");
    }

    #[test]
    fn no_scrollbar_when_content_fits() {
        // Content shorter than the viewport → no reserved column, no glyphs
        // in the rightmost column of the main area.
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 6;
        let backend = ratatui::backend::TestBackend::new(40, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let buf = terminal.backend().buffer();
        let marked = (0..6u16).filter(|&y| buf[(39, y)].symbol() == "│").count();
        assert_eq!(marked, 0, "no scrollbar when everything fits");
    }

    #[test]
    fn active_search_shows_persistent_match_indicator() {
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
        app.viewport_height = 6;
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in "value".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.search.active && app.search.matches.len() == 2);

        // Overwrite the one-shot toast so the indicator is the only place
        // the match position lives — then render and read the status row.
        app.set_info("something else entirely");
        let backend = ratatui::backend::TestBackend::new(80, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();

        let status = row_text(terminal.backend().buffer(), 4, 80);
        assert!(
            status.contains("/value") && status.contains("1/2"),
            "status bar should carry a persistent n/N search indicator, got: {status:?}"
        );
    }

    #[test]
    fn short_path_prefers_basename_and_stays_in_budget() {
        assert_eq!(short_path("a/b/c/some_file.rs", 16), "…/some_file.rs");
        use unicode_width::UnicodeWidthStr;
        // Basename wider than the budget: tail-truncate, still within budget.
        let out = short_path("a/b/c/功能测试功能测试功能测试.rs", 10);
        assert!(out.width() <= 10, "{out:?}");
        assert!(out.starts_with('…'), "{out:?}");
    }

    #[test]
    fn short_path_is_char_boundary_safe_on_non_ascii() {
        // 30+ bytes of CJK with no separator: the old byte-index cut
        // (`&path[len-22..]`) landed mid-char and panicked. It must truncate
        // to the display budget instead.
        use unicode_width::UnicodeWidthStr;
        let p = "功能测试文件名非常长的中文文件名";
        let out = short_path(p, 24);
        assert!(out.width() <= 24, "{out:?}");
        assert!(out.starts_with('…'), "{out:?}");
    }

    #[test]
    fn status_path_is_width_aware_for_cjk() {
        use unicode_width::UnicodeWidthStr;
        // 12 CJK chars (24 columns) + ".rs": chars-based truncation would
        // overflow a 16-column budget; width-based must fit.
        let p = "功能测试文件名超长超长超长.rs";
        let out = status_path(p, 16);
        assert!(out.width() <= 16, "{out:?}");
    }

    #[test]
    fn help_overlay_scrolls_with_jk_and_clamps_on_resize() {
        let review = parse_unified_diff(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        let mut app = App::with_highlighter(review, highlighter());
        app.viewport_height = 6;
        app.show_help = true;

        // Short terminal: the panel can't show everything; j/k scroll it.
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        for _ in 0..3 {
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.help_scroll, 2, "3 × j then 1 × k → scroll 2");
        terminal.draw(|f| draw(&mut app, f)).unwrap();
        assert_eq!(app.help_scroll, 2, "in-range scroll survives a redraw");

        // Tall terminal: the whole sheet fits, so the draw clamps to 0.
        let mut tall = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 60)).unwrap();
        tall.draw(|f| draw(&mut app, f)).unwrap();
        assert_eq!(app.help_scroll, 0, "everything fits → scroll clamps to 0");

        // Reopening the overlay resets to the top, not to the old offset.
        for _ in 0..2 {
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('?'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(!app.show_help, "? dismisses the overlay");
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('?'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(app.show_help, "? reopens the overlay");
        assert_eq!(app.help_scroll, 0, "reopen starts at the top");
    }
}
