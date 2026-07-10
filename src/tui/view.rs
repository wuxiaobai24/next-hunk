//! Rendering for the review TUI.
//!
//! Layout (vertical):
//!   ┌─────────────────────────────────────────┐
//!   │  rail (left)  │  stream (right)         │  ← main area
//!   ├─────────────────────────────────────────┤
//!   │  status line                             │  1 row
//!   ├─────────────────────────────────────────┤
//!   │  help line                               │  1 row
//!   └─────────────────────────────────────────┘
//!
//! The stream pane renders ONLY the rows in `[scroll_y, scroll_y+height)` via
//! [`ViewportQuery::rows`] — O(visible), never a full-line cache of the review
//! (architecture §7 anti-pattern).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::ir::{DiffLineKind, StreamRow, Viewport, ViewportQuery};
use crate::tui::app::App;

/// Rail width (left file list). Capped to a fraction of the area at draw time.
const RAIL_MAX_WIDTH: u16 = 32;

/// Draw the whole app. Pure function of `&App` + frame area.
pub fn draw(app: &App, frame: &mut Frame) {
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
    draw_help(frame, chunks[2]);
}

fn draw_main(app: &App, frame: &mut Frame, area: Rect) {
    // Rail width: min(RAIL_MAX_WIDTH, quarter of width), but at least 12.
    let rail_w = RAIL_MAX_WIDTH.min(area.width / 4).max(12);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(rail_w), Constraint::Min(0)])
        .split(area);

    draw_rail(app, frame, cols[0]);
    draw_stream(app, frame, cols[1]);
}

fn draw_rail(app: &App, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = app
        .review
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| {
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

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::RIGHT)
            .title("Files"),
    );

    let mut state = ListState::default();
    state.select(Some(app.selected_file));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_stream(app: &App, frame: &mut Frame, area: Rect) {
    // Update the app's notion of viewport height from the actual area.
    // (Safe: we only read it back through `app` for clamping on next key.)
    let height = area.height as usize;
    // We can't mutate app here (draw takes &App), so recompute viewport using
    // the drawn height directly. App.viewport_height is synced after draw by
    // the run loop via `sync_viewport_height`.
    let viewport = Viewport {
        start: app.scroll_y,
        height,
    };
    let rows = ViewportQuery::rows(&app.review, viewport);

    let lines: Vec<Line> = rows
        .iter()
        .map(|row| stream_row_to_line(*row))
        .collect();

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", app.current_path())),
    );
    frame.render_widget(para, area);
}

fn stream_row_to_line(row: StreamRow) -> Line<'static> {
    match row {
        StreamRow::FileHeader { path, .. } => Line::from(Span::styled(
            format!("─── {} ───", path),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        StreamRow::HunkHeader { text, .. } => Line::from(Span::styled(
            text.to_string(),
            Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
        )),
        StreamRow::Line { kind, text, .. } => {
            let (prefix, style) = match kind {
                DiffLineKind::Add => (
                    '+',
                    Style::default().fg(Color::Green),
                ),
                DiffLineKind::Delete => (
                    '-',
                    Style::default().fg(Color::Red),
                ),
                DiffLineKind::Meta => (
                    '\\',
                    Style::default().fg(Color::DarkGray),
                ),
                DiffLineKind::Context => (' ', Style::default()),
            };
            Line::from(Span::styled(format!("{prefix}{text}"), style))
        }
    }
}

fn draw_status(app: &App, frame: &mut Frame, area: Rect) {
    let pos = if app.review.stream_len == 0 {
        "0/0".to_string()
    } else {
        format!(
            "{}/{}",
            app.scroll_y + 1,
            app.review.stream_len
        )
    };
    let left = format!(
        " {}  [{}]  {} ",
        app.current_path(),
        app.selected_file + 1,
        pos,
    );
    let right = format!(" {} ", app.status);
    let line = Line::from(vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(""), // spacer
        Span::styled(right, Style::default().fg(Color::DarkGray)),
    ]);
    let para = Paragraph::new(line).style(Style::default().bg(Color::Black));
    frame.render_widget(para, area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let help = " j/k scroll · J/K half-page · g/G top/bottom · Tab next file · q quit ";
    let para = Paragraph::new(help).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(para, area);
}

/// Truncate a path for the rail display.
fn short_path(path: &str) -> String {
    // Keep the last path component plus a little context, capped.
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
