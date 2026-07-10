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

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::ir::{DiffLineKind, StreamRow, Viewport, ViewportQuery};
use crate::tui::app::{App, InputMode};

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
}

fn draw_main(app: &mut App, frame: &mut Frame, area: Rect) {
    let rail_w = RAIL_MAX_WIDTH.min(area.width / 4).max(12);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(rail_w), Constraint::Min(0)])
        .split(area);

    draw_rail(app, frame, cols[0]);
    draw_stream(app, frame, cols[1]);
}

fn draw_rail(app: &App, frame: &mut Frame, area: Rect) {
    let visible = app.visible_files();
    let items: Vec<ListItem> = visible
        .iter()
        .map(|&i| {
            let f = &app.review.files[i];
            let label = format!(" {}. {}", i + 1, short_path(&f.display_path));
            let style = if i == app.selected_file {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(label, style)))
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
    let height = area.height as usize;
    let scroll_y = app.scroll_y;
    let viewport = Viewport {
        start: scroll_y,
        height,
    };

    // Collect owned row data so we release the &app.review borrow before
    // mutating app.cache below for highlighting.
    let owned_rows: Vec<OwnedRow> = ViewportQuery::rows(&app.review, viewport)
        .into_iter()
        .enumerate()
        .map(|(i, row)| OwnedRow::from_stream_row(row, scroll_y + i))
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

    let title = app.current_path().to_string();
    let lines: Vec<Line> = owned_rows
        .into_iter()
        .map(|r| {
            stream_row_to_line(
                app,
                r,
                current_match_row,
                &match_rows,
            )
        })
        .collect();

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", title)),
    );
    frame.render_widget(para, area);
}

/// Owned snapshot of a stream row's display data, so we can release the
/// `&app.review` borrow before mutating `app` for highlight caching.
enum OwnedRow {
    FileHeader { path: String },
    HunkHeader { text: String },
    Line {
        kind: DiffLineKind,
        text: String,
        file_idx: usize,
        abs_row: usize,
    },
}

impl OwnedRow {
    fn from_stream_row(row: StreamRow, abs_row: usize) -> Self {
        match row {
            StreamRow::FileHeader { path, .. } => OwnedRow::FileHeader { path: path.to_string() },
            StreamRow::HunkHeader { text, .. } => OwnedRow::HunkHeader { text: text.to_string() },
            StreamRow::Line {
                kind,
                text,
                file_idx,
                ..
            } => OwnedRow::Line {
                kind,
                text: text.to_string(),
                file_idx,
                abs_row,
            },
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
        OwnedRow::Line { abs_row, .. } => *abs_row,
        _ => usize::MAX, // headers never match
    };
    let is_current_match = current_match_row == Some(abs_row);
    let is_other_match = !is_current_match && match_rows.contains(&abs_row);

    let line = match row {
        OwnedRow::FileHeader { path } => Line::from(Span::styled(
            format!("─── {} ───", path),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        OwnedRow::HunkHeader { text } => Line::from(Span::styled(
            text,
            Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
        )),
        OwnedRow::Line {
            kind,
            text,
            file_idx,
            abs_row,
        } => {
            let (prefix, kind_style) = match kind {
                DiffLineKind::Add => ('+', Style::default().fg(Color::Green)),
                DiffLineKind::Delete => ('-', Style::default().fg(Color::Red)),
                DiffLineKind::Meta => ('\\', Style::default().fg(Color::DarkGray)),
                DiffLineKind::Context => (' ', Style::default()),
            };

            // Compute highlight runs for the code text (viewport-only, cached).
            let line_in_file =
                ViewportQuery::file_and_line(&app.review, abs_row).map(|(_, li)| li);
            let runs = if app.highlight_on {
                if let Some(li) = line_in_file {
                    let path = app.review.display_path(file_idx);
                    app.cache
                        .get_or_highlight(file_idx, li, path, &text, &app.highlighter)
                } else {
                    vec![(Style::default(), text)]
                }
            } else {
                vec![(Style::default(), text)]
            };

            let mut spans: Vec<Span> = Vec::with_capacity(runs.len() + 1);
            spans.push(Span::styled(prefix.to_string(), kind_style));
            for (style, txt) in runs {
                spans.push(Span::styled(txt, style));
            }
            Line::from(spans)
        }
    };

    if is_current_match {
        line.style(Style::default().bg(Color::Yellow).fg(Color::Black))
    } else if is_other_match {
        line.style(Style::default().bg(Color::DarkGray))
    } else {
        line
    }
}

fn draw_status(app: &App, frame: &mut Frame, area: Rect) {
    let pos = if app.review.stream_len == 0 {
        "0/0".to_string()
    } else {
        format!("{}/{}", app.scroll_y + 1, app.review.stream_len)
    };
    let hl = if app.highlight_on { " HL" } else { "" };
    let left = format!(" {}  [{}]  {}{} ", app.current_path(), app.selected_file + 1, pos, hl);
    let right = format!(" {} ", app.status);
    let line = Line::from(vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(""),
        Span::styled(right, Style::default().fg(Color::DarkGray)),
    ]);
    let para = Paragraph::new(line).style(Style::default().bg(Color::Black));
    frame.render_widget(para, area);
}

/// Render the help line, or an input prompt when editing search/filter.
fn draw_help_or_prompt(app: &App, frame: &mut Frame, area: Rect) {
    let content = match app.mode {
        InputMode::Search => {
            format!("/{}▌  (Enter search · Enter confirm · Esc cancel)", app.search.query)
        }
        InputMode::Filter => {
            format!(
                "filter: {}▌  (path substring · Enter confirm · Esc cancel)",
                app.path_filter
            )
        }
        InputMode::Normal => {
            " j/k scroll · J/K half-page · g/G top/bottom · Tab next file · / search · f filter · H highlight · q quit "
                .to_string()
        }
    };
    let style = match app.mode {
        InputMode::Normal => Style::default().fg(Color::DarkGray),
        _ => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    };
    let para = Paragraph::new(content).style(style);
    frame.render_widget(para, area);
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
