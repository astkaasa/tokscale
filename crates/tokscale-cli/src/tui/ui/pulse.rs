use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};

#[cfg(test)]
use super::text_width::display_width;
use super::text_width::truncate_display;
use super::widgets::light_ratio_bar_spans;
use crate::tui::app::{App, ClickAction};
use crate::tui::integrations::weread::model::{WeReadFocusBook, WeReadMonthly};
use crate::tui::integrations::weread::{
    format_compare_ratio, format_read_duration, now_millis, WeReadBookRef, WeReadCategory,
    WeReadState, WeReadStatus, WeReadWeekly,
};

const DAY_LABELS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.background)),
        area,
    );

    if area.width < 44 || area.height < 15 {
        render_narrow(frame, app, area);
    } else {
        render_wide(frame, app, area);
    }
}

fn render_wide(frame: &mut Frame, app: &mut App, area: Rect) {
    let bottom_height = area.height.saturating_sub(8).clamp(7, 15);
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(bottom_height),
            Constraint::Min(0),
        ])
        .split(area);

    render_weread_pulse(frame, app, outer[0]);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(outer[1]);

    render_month_rhythm(frame, app, bottom[0]);
    render_library_signals(frame, app, bottom[1]);
}

fn render_narrow(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .split(area);

    render_weread_pulse(frame, app, chunks[0]);
    render_month_rhythm(frame, app, chunks[1]);
    render_library_signals(frame, app, chunks[2]);
}

fn render_weread_pulse(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = panel_block(app, "WeRead Pulse");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.add_click_area(area, ClickAction::WeReadRefresh);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    match &app.weread.weekly {
        Some(weekly) if inner.width >= 22 && inner.height >= 5 => {
            let focus_height = u16::from(weekly.focus.is_some() && inner.height >= 6);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(focus_height),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .split(inner);

            frame.render_widget(
                Paragraph::new(weekly_summary_line(app, weekly))
                    .style(Style::default().bg(app.theme.background)),
                chunks[0],
            );
            render_week_table(frame, app, weekly, chunks[1]);
            if let Some(focus) = &weekly.focus {
                render_focus_line(frame, app, focus, chunks[2]);
            }
            frame.render_widget(
                Paragraph::new(status_line(app, &app.weread))
                    .style(Style::default().bg(app.theme.background)),
                chunks[3],
            );
        }
        _ => {
            let lines = weread_pulse_lines(app, &app.weread, inner.width);
            frame.render_widget(
                Paragraph::new(lines)
                    .style(
                        Style::default()
                            .fg(app.theme.foreground)
                            .bg(app.theme.background),
                    )
                    .wrap(Wrap { trim: true }),
                inner,
            );
        }
    }
}

fn render_week_table(frame: &mut Frame, app: &App, weekly: &WeReadWeekly, area: Rect) {
    if area.width < 22 || area.height < 3 {
        return;
    }

    let spacing = if area.width >= 54 { 2 } else { 1 };
    let day_width = area
        .width
        .saturating_sub(spacing * 6)
        .checked_div(7)
        .unwrap_or(3)
        .clamp(3, 7);
    let columns = vec![Constraint::Length(day_width); 7];

    let label_cells = DAY_LABELS
        .iter()
        .map(|label| centered_cell(muted_span(app, (*label).to_string())));
    let marker_cells = weekly.days.iter().map(|day| {
        let symbol = if day.checked_in { "✓" } else { "·" };
        let color = if day.checked_in {
            Color::Green
        } else {
            app.theme.muted
        };
        centered_cell(Span::styled(
            symbol,
            Style::default()
                .fg(color)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        ))
    });
    let duration_cells = weekly.days.iter().map(|day| {
        let text = if day.read_seconds == 0 {
            "--".to_string()
        } else {
            format_read_duration(day.read_seconds)
        };
        centered_cell(muted_span(app, truncate_display(&text, day_width as usize)))
    });

    let rows = vec![
        Row::new(label_cells),
        Row::new(marker_cells),
        Row::new(duration_cells),
    ];
    let table = Table::new(rows, columns)
        .column_spacing(spacing)
        .style(Style::default().bg(app.theme.background));
    frame.render_widget(table, area);
}

fn centered_cell(span: Span<'static>) -> Cell<'static> {
    Cell::from(Line::from(span).centered())
}

fn render_focus_line(frame: &mut Frame, app: &App, focus: &WeReadFocusBook, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let label = format!(
        "Focus  {}  {} this week",
        focus.title,
        format_read_duration(focus.read_seconds)
    );
    frame.render_widget(
        Paragraph::new(Line::from(muted_span(
            app,
            truncate_display(&label, area.width as usize),
        )))
        .style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn render_month_rhythm(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(app, "Month Rhythm");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if let Some(monthly) = &app.weread.monthly {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(0)])
            .split(inner);

        render_month_summary(frame, app, monthly, chunks[0]);
        render_category_table(frame, app, monthly.categories.as_slice(), chunks[1]);
    } else {
        frame.render_widget(
            Paragraph::new(empty_state_line(app)).style(Style::default().bg(app.theme.background)),
            inner,
        );
    }
}

fn render_month_summary(frame: &mut Frame, app: &App, monthly: &WeReadMonthly, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut lines = vec![Line::from(vec![
        metric_span(app, format_read_duration(monthly.total_seconds)),
        muted_span(app, format!("  {} days", monthly.read_days)),
        muted_span(
            app,
            format!(
                "  avg {}",
                format_read_duration(monthly.day_average_seconds)
            ),
        ),
    ])];

    if area.height > 1 {
        let preference = monthly.prefer_category_word.as_deref().unwrap_or("");
        lines.push(Line::from(muted_span(
            app,
            truncate_display(preference, area.width as usize),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn render_category_table(frame: &mut Frame, app: &App, categories: &[WeReadCategory], area: Rect) {
    if area.width < 14 || area.height == 0 {
        return;
    }

    let rank_width = 2u16;
    let time_width = 6u16;
    let spacing = 1u16;
    let min_label_width = 4u16;
    let fixed_width = rank_width + time_width + min_label_width + spacing * 3;
    let bar_width = area.width.saturating_sub(fixed_width).clamp(6, 22);
    let label_width = area
        .width
        .saturating_sub(rank_width + bar_width + time_width + spacing * 3)
        .max(min_label_width);

    let rows = categories
        .iter()
        .take(area.height as usize)
        .enumerate()
        .map(|(index, category)| category_row(app, category, index + 1, bar_width as usize));

    let table = Table::new(
        rows,
        [
            Constraint::Length(rank_width),
            Constraint::Length(bar_width),
            Constraint::Length(time_width),
            Constraint::Length(label_width),
        ],
    )
    .column_spacing(spacing)
    .style(Style::default().bg(app.theme.background));

    frame.render_widget(table, area);
}

fn category_row<'a>(
    app: &App,
    category: &WeReadCategory,
    rank: usize,
    bar_width: usize,
) -> Row<'a> {
    let rank = Line::from(Span::styled(
        rank.to_string(),
        Style::default()
            .fg(app.theme.accent)
            .bg(app.theme.background)
            .add_modifier(Modifier::BOLD),
    ))
    .right_aligned();
    let bar = Line::from(light_ratio_bar_spans(
        category.weight,
        bar_width,
        Style::default().fg(Color::Green).bg(app.theme.background),
        app.theme.subtle_text_style().bg(app.theme.background),
    ));
    let time = Line::from(muted_span(
        app,
        format_read_duration(category.reading_seconds),
    ))
    .right_aligned();

    Row::new([
        Cell::from(rank),
        Cell::from(bar),
        Cell::from(time),
        Cell::from(Line::from(muted_span(app, category.title.clone()))),
    ])
    .height(1)
    .style(Style::default().bg(app.theme.background))
}

fn render_library_signals(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(app, "Library Signals");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let has_stats = app.weread.shelf.is_some() || app.weread.notes.is_some();
    let recent = app
        .weread
        .shelf
        .as_ref()
        .map(|shelf| shelf.recent.as_slice())
        .unwrap_or(&[]);

    if !has_stats && recent.is_empty() {
        frame.render_widget(
            Paragraph::new(empty_state_line(app)).style(Style::default().bg(app.theme.background)),
            inner,
        );
        return;
    }

    let stats_height = if has_stats {
        u16::from(app.weread.shelf.is_some()) + u16::from(app.weread.notes.is_some())
    } else {
        0
    };
    let heading_height = if recent.is_empty() { 0 } else { 2 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(stats_height),
            Constraint::Length(heading_height),
            Constraint::Min(0),
        ])
        .split(inner);

    render_library_stats(frame, app, chunks[0]);
    render_recent_heading(frame, app, chunks[1]);
    render_recent_books(frame, app, recent, chunks[2]);
}

fn render_library_stats(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut rows = Vec::new();
    if let Some(shelf) = &app.weread.shelf {
        let public_items = shelf.visible_items.saturating_sub(shelf.private_items);
        let mut pairs = vec![
            ("Items", shelf.visible_items.to_string()),
            ("Private", shelf.private_items.to_string()),
        ];
        if area.width >= 50 {
            pairs.push(("Public", public_items.to_string()));
        }
        rows.push(stat_row(app, &pairs));
    }
    if let Some(notes) = &app.weread.notes {
        rows.push(stat_row(
            app,
            &[
                ("Notes", notes.total_notes.to_string()),
                ("Books", notes.total_books.to_string()),
            ],
        ));
    }

    if rows.is_empty() {
        return;
    }

    let widths = if area.width >= 50 {
        vec![
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(6),
        ]
    } else {
        vec![
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(6),
        ]
    };

    let table = Table::new(rows, widths)
        .column_spacing(2)
        .style(Style::default().bg(app.theme.background));
    frame.render_widget(table, area);
}

fn stat_row<'a>(app: &App, pairs: &[(&str, String)]) -> Row<'a> {
    let mut cells = Vec::with_capacity(pairs.len() * 2);
    for (label, value) in pairs {
        cells.push(Cell::from(Line::from(muted_span(
            app,
            (*label).to_string(),
        ))));
        cells.push(Cell::from(
            Line::from(metric_span(app, value.clone())).right_aligned(),
        ));
    }

    Row::new(cells)
        .height(1)
        .style(Style::default().bg(app.theme.background))
}

fn render_recent_heading(frame: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }

    let lines = if area.height > 1 {
        vec![
            Line::from(""),
            Line::from(muted_span(app, "Recent focus".to_string())),
        ]
    } else {
        vec![Line::from(muted_span(app, "Recent focus".to_string()))]
    };

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.background)),
        area,
    );
}

fn render_recent_books(frame: &mut Frame, app: &App, books: &[WeReadBookRef], area: Rect) {
    if area.width < 8 || area.height == 0 || books.is_empty() {
        return;
    }

    let rank_width = 2u16;
    let spacing = 1u16;
    let title_width = area.width.saturating_sub(rank_width + spacing);
    let rows = books
        .iter()
        .take(area.height as usize)
        .enumerate()
        .map(|(index, book)| recent_book_row(app, book, index + 1, title_width as usize));

    let table = Table::new(
        rows,
        [
            Constraint::Length(rank_width),
            Constraint::Length(title_width),
        ],
    )
    .column_spacing(spacing)
    .style(Style::default().bg(app.theme.background));
    frame.render_widget(table, area);
}

fn recent_book_row<'a>(
    app: &App,
    book: &WeReadBookRef,
    rank: usize,
    title_width: usize,
) -> Row<'a> {
    Row::new([
        Cell::from(
            Line::from(Span::styled(
                rank.to_string(),
                Style::default()
                    .fg(app.theme.accent)
                    .bg(app.theme.background)
                    .add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        ),
        Cell::from(Line::from(muted_span(
            app,
            truncate_display(&book.title, title_width),
        ))),
    ])
    .height(1)
    .style(Style::default().bg(app.theme.background))
}

fn weread_pulse_lines(app: &App, state: &WeReadState, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match &state.weekly {
        Some(weekly) => {
            lines.push(weekly_summary_line(app, weekly));
            lines.push(day_labels_line(app, width));
            lines.push(day_markers_line(app, weekly, width));
            lines.push(day_duration_line(app, weekly, width));
            if let Some(focus) = &weekly.focus {
                let label = format!(
                    "Focus  {}  {} this week",
                    focus.title,
                    format_read_duration(focus.read_seconds)
                );
                lines.push(Line::from(vec![Span::styled(
                    truncate_display(&label, width as usize),
                    app.theme.subtle_text_style().bg(app.theme.background),
                )]));
            }
        }
        None => {
            lines.push(empty_state_line(app));
        }
    }

    lines.push(status_line(app, state));
    lines
}

fn weekly_summary_line(app: &App, weekly: &WeReadWeekly) -> Line<'static> {
    Line::from(vec![
        metric_span(app, format!("{}/7", weekly.read_days)),
        Span::raw("  "),
        metric_span(app, format_read_duration(weekly.total_seconds)),
        muted_span(
            app,
            format!(
                "  avg {}  {}",
                format_read_duration(weekly.day_average_seconds),
                format_compare_ratio(weekly.compare_ratio)
            ),
        ),
    ])
}

fn day_labels_line(app: &App, width: u16) -> Line<'static> {
    if width < 54 {
        return Line::from(muted_span(app, DAY_LABELS.join(" ")));
    }
    Line::from(muted_span(app, DAY_LABELS.join("   ")))
}

fn day_markers_line(app: &App, weekly: &WeReadWeekly, width: u16) -> Line<'static> {
    let gap = if width < 54 { "   " } else { "     " };
    let mut spans = Vec::new();
    for (index, day) in weekly.days.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(gap));
        }
        let symbol = if day.checked_in { "✓" } else { "·" };
        let color = if day.checked_in {
            Color::Green
        } else {
            app.theme.muted
        };
        spans.push(Span::styled(
            symbol,
            Style::default()
                .fg(color)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn day_duration_line(app: &App, weekly: &WeReadWeekly, width: u16) -> Line<'static> {
    let gap = if width < 54 { " " } else { "   " };
    let mut spans = Vec::new();
    for (index, day) in weekly.days.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(gap));
        }
        let text = if day.read_seconds == 0 {
            "--".to_string()
        } else {
            format_read_duration(day.read_seconds)
        };
        spans.push(muted_span(app, format!("{text:>3}")));
    }
    Line::from(spans)
}

fn status_line(app: &App, state: &WeReadState) -> Line<'static> {
    let mut spans = vec![muted_span(app, format!("Sync  {}", state.status.label()))];

    if let Some(last) = state.last_refresh_ms {
        let age_minutes = now_millis().saturating_sub(last) / 60_000;
        spans.push(muted_span(app, format!("  {age_minutes}m ago")));
    }

    if matches!(
        state.status,
        WeReadStatus::AuthMissing | WeReadStatus::Error | WeReadStatus::UpgradeRequired
    ) {
        if let Some(error) = &state.error {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                truncate_display(error, 42),
                Style::default().fg(Color::Yellow).bg(app.theme.background),
            ));
        }
    }

    Line::from(spans)
}

fn panel_block(app: &App, title: &'static str) -> Block<'static> {
    Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(app.theme.foreground)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(app.theme.border)
                .bg(app.theme.background),
        )
        .style(Style::default().bg(app.theme.background))
}

fn metric_span(app: &App, text: String) -> Span<'static> {
    Span::styled(
        text,
        Style::default()
            .fg(app.theme.accent)
            .bg(app.theme.background)
            .add_modifier(Modifier::BOLD),
    )
}

fn muted_span(app: &App, text: String) -> Span<'static> {
    Span::styled(text, app.theme.subtle_text_style().bg(app.theme.background))
}

fn empty_state_line(app: &App) -> Line<'static> {
    Line::from(Span::styled(
        "Set env.WEREAD_API_KEY in settings.json to enable reading pulse",
        app.theme.subtle_text_style().bg(app.theme.background),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::TuiConfig;
    use crate::tui::data::UsageData;
    use crate::tui::integrations::weread::model::{WeReadDay, WeReadFocusBook};
    use crate::tui::integrations::weread::WeReadWeekly;
    use chrono::NaiveDate;
    use ratatui::{backend::TestBackend, Terminal};

    fn make_app() -> App {
        let config = TuiConfig {
            theme: "blue".to_string(),
            refresh: 0,
            sessions_path: None,
            clients: None,
            since: None,
            until: None,
            year: None,
            initial_tab: None,
        };
        App::new_with_cached_data(config, Some(UsageData::default())).unwrap()
    }

    #[test]
    fn renders_weekly_check_marks_without_panic() {
        let mut app = make_app();
        app.weread.weekly = Some(WeReadWeekly {
            period_start: NaiveDate::from_ymd_opt(2026, 6, 8).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2026, 6, 14).unwrap(),
            read_days: 4,
            total_seconds: 17_814,
            day_average_seconds: 3_562,
            compare_ratio: Some(0.35),
            days: [
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 8).unwrap(), 6089),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 9).unwrap(), 8813),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap(), 1281),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 11).unwrap(), 1631),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 12).unwrap(), 0),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 13).unwrap(), 0),
                WeReadDay::new(NaiveDate::from_ymd_opt(2026, 6, 14).unwrap(), 0),
            ],
            focus: Some(WeReadFocusBook {
                id: "1".to_string(),
                title: "Focus".to_string(),
                author: None,
                read_seconds: 3600,
            }),
        });

        let backend = TestBackend::new(92, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, Rect::new(0, 0, 92, 24)))
            .unwrap();

        let output = terminal
            .backend()
            .buffer()
            .content()
            .chunks(92)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("WeRead Pulse"));
        assert!(output.contains("4/7"));
    }

    #[test]
    fn truncate_display_respects_cjk_cell_width() {
        let text = "Focus  万物发明指南  5h9m this week";
        let truncated = truncate_display(text, 18);

        assert!(display_width(&truncated) <= 18);
        assert!(truncated.ends_with('…'));
    }
}
