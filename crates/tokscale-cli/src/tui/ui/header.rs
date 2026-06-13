use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use tokscale_core::pulse::weread::format_read_duration;

use super::widgets::{format_cost, format_tokens};
use crate::tui::app::{App, ClickAction, Tab};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_very_narrow = app.is_very_narrow();
    let is_narrow = app.is_narrow();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let row = Rect::new(inner.x, inner.y, inner.width, 1);
    render_brand(frame, app, row, is_very_narrow);
    let right_reserved = render_right_status(frame, app, row, is_narrow);
    render_workspace_tabs(frame, app, row, right_reserved, is_very_narrow);
}

fn render_brand(frame: &mut Frame, app: &App, area: Rect, is_very_narrow: bool) {
    let brand = if is_very_narrow {
        " Tok "
    } else {
        " Tokscale "
    };
    let mut spans = vec![Span::styled(
        brand,
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    if !is_very_narrow {
        spans.push(Span::styled(
            format!(" v{} ", env!("CARGO_PKG_VERSION")),
            app.theme.subtle_text_style(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_right_status(frame: &mut Frame, app: &App, area: Rect, is_narrow: bool) -> u16 {
    if is_narrow || area.width < 40 {
        return 0;
    }

    let active_days = app
        .data
        .daily
        .iter()
        .filter(|day| day.tokens.total() > 0 || day.cost > 0.0)
        .count();
    let (scope, status) = if app.current_tab == Tab::Pulse {
        let week = app
            .pulse
            .weread
            .weekly
            .as_ref()
            .map(|weekly| {
                format!(
                    "{}/7  •  {}",
                    weekly.read_days,
                    format_read_duration(weekly.total_seconds)
                )
            })
            .unwrap_or_else(|| app.pulse.weread.status.label().to_string());
        ("WeRead", week)
    } else if app.current_tab == Tab::Overview
        && app.overview_mode == crate::tui::app::OverviewMode::Today
    {
        let (tokens, cost, models) = app.overview_totals();
        (
            "Today",
            format!(
                "{}  •  {}  •  {} models",
                format_tokens(tokens),
                format_cost(cost),
                models
            ),
        )
    } else {
        (
            "All Time",
            format!("{} days  •  {} models", active_days, app.data.models.len()),
        )
    };
    let status_text = format!("{scope}  •  {status}");
    let width = Line::from(status_text.as_str()).width() as u16;
    let status_area = Rect::new(
        area.right().saturating_sub(width),
        area.y,
        width.min(area.width),
        1,
    );
    let line = Line::from(vec![
        Span::styled(scope, app.theme.subtle_text_style()),
        Span::styled("  •  ", app.theme.subtle_text_style()),
        Span::styled(
            status,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).alignment(Alignment::Right),
        status_area,
    );
    width.saturating_add(2)
}

fn render_workspace_tabs(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    right_reserved: u16,
    is_very_narrow: bool,
) {
    if area.width == 0 {
        return;
    }

    let visible_tabs = header_tabs_for_layout(app, is_very_narrow);
    let tab_count = visible_tabs.len();
    if tab_count == 0 {
        return;
    }

    let item_widths: Vec<u16> = visible_tabs
        .iter()
        .map(|tab| tab_label_width(*tab, is_very_narrow).saturating_add(4))
        .collect();
    let divider_width = 2u16;
    let total_width = item_widths
        .iter()
        .copied()
        .sum::<u16>()
        .saturating_add(divider_width.saturating_mul(tab_count.saturating_sub(1) as u16));
    let left_guard = area.x.saturating_add(if is_very_narrow { 7 } else { 18 });
    let right_guard = area.right().saturating_sub(right_reserved);
    let centered = area.x + area.width.saturating_sub(total_width) / 2;
    let mut x = centered.max(left_guard);
    if x.saturating_add(total_width) > right_guard {
        x = right_guard.saturating_sub(total_width);
        x = x.max(left_guard.min(area.right()));
    }

    for (index, tab) in visible_tabs.into_iter().enumerate() {
        let remaining_width = area.right().saturating_sub(x);
        if remaining_width == 0 {
            break;
        }

        let width = item_widths[index].min(remaining_width);
        let selected = tab == app.current_tab;
        let style = if selected {
            Style::default()
                .fg(app.theme.foreground)
                .bg(app.theme.color(Color::Rgb(30, 64, 175)))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.foreground)
        };
        let label = tab_label(tab, is_very_narrow);
        let text_width = width as usize;
        let text = format!("{:^text_width$}", label);
        let rect = Rect::new(x, area.y, width, 1);
        frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), rect);
        app.add_click_area(rect, ClickAction::Tab(tab));
        x = x.saturating_add(width);

        let remaining_width = area.right().saturating_sub(x);
        if remaining_width == 0 || index + 1 == tab_count {
            break;
        }

        let divider = Rect::new(x, area.y, divider_width.min(remaining_width), 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  ",
                Style::default().fg(app.theme.border),
            ))),
            divider,
        );
        x = x.saturating_add(divider.width);
    }
}

fn tab_label(tab: Tab, is_very_narrow: bool) -> &'static str {
    if is_very_narrow {
        tab.workspace_short_name()
    } else {
        tab.workspace_label()
    }
}

fn tab_label_width(tab: Tab, is_very_narrow: bool) -> u16 {
    Line::from(tab_label(tab, is_very_narrow)).width() as u16
}

fn header_tabs(app: &App) -> Vec<Tab> {
    let mut tabs = app.visible_workspaces();
    if app.is_tab_visible(app.current_tab) && !tabs.contains(&app.current_tab) {
        tabs.push(app.current_tab);
    }
    tabs
}

fn header_tabs_for_layout(app: &App, is_very_narrow: bool) -> Vec<Tab> {
    let tabs = header_tabs(app);
    if !is_very_narrow || tabs.len() <= 1 || !tabs.contains(&app.current_tab) {
        return tabs;
    }
    prioritize_current_tab(tabs, app.current_tab)
}

fn prioritize_current_tab(tabs: Vec<Tab>, current: Tab) -> Vec<Tab> {
    let Some(current_index) = tabs.iter().position(|tab| *tab == current) else {
        return tabs;
    };

    let mut ordered = Vec::with_capacity(tabs.len());
    if current_index > 0 {
        ordered.push(tabs[current_index - 1]);
    }
    ordered.push(current);
    if current_index + 1 < tabs.len() {
        ordered.push(tabs[current_index + 1]);
    }

    for distance in 2..tabs.len() {
        if current_index >= distance {
            ordered.push(tabs[current_index - distance]);
        }
        let right = current_index + distance;
        if right < tabs.len() {
            ordered.push(tabs[right]);
        }
    }

    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{backend::TestBackend, Terminal};

    use crate::tui::app::TuiConfig;

    fn make_app(width: u16) -> App {
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
        let mut app = App::new_with_cached_data(config, None).unwrap();
        app.terminal_width = width;
        app
    }

    fn render_header(app: &mut App, width: u16) -> Vec<Vec<String>> {
        let backend = TestBackend::new(width, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, app, Rect::new(0, 0, width, 3)))
            .unwrap();

        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol().to_string()).collect())
            .collect()
    }

    fn rendered_label_column(rows: &[Vec<String>], label: &str) -> u16 {
        let target: Vec<String> = label.chars().map(|ch| ch.to_string()).collect();

        rows[1]
            .windows(target.len())
            .position(|window| window == target.as_slice())
            .expect("expected label to be rendered") as u16
    }

    fn click_header(app: &mut App, column: u16) {
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
    }

    fn row_text(rows: &[Vec<String>], row: usize) -> String {
        rows[row].iter().map(String::as_str).collect()
    }

    #[test]
    fn timeline_label_click_uses_rendered_normal_tab_position() {
        let mut app = make_app(80);

        let rows = render_header(&mut app, 80);
        click_header(&mut app, rendered_label_column(&rows, "Timeline"));

        assert_eq!(app.current_tab, Tab::Daily);
    }

    #[test]
    fn timeline_short_label_click_uses_rendered_very_narrow_tab_position() {
        let mut app = make_app(59);

        let rows = render_header(&mut app, 59);
        click_header(&mut app, rendered_label_column(&rows, "Time"));

        assert_eq!(app.current_tab, Tab::Daily);
    }

    #[test]
    fn active_non_workspace_tab_gets_temporary_header_item() {
        let mut app = make_app(96);
        app.current_tab = Tab::Hourly;

        let rows = render_header(&mut app, 96);
        let row = row_text(&rows, 1);

        assert!(row.contains("Overview"), "{row}");
        assert!(row.contains("Timeline"), "{row}");
        assert!(row.contains("Usage"), "{row}");
        assert!(row.contains("Hourly"), "{row}");
        assert_eq!(
            header_tabs(&app),
            vec![
                Tab::Overview,
                Tab::Pulse,
                Tab::Models,
                Tab::Daily,
                Tab::Usage,
                Tab::Hourly,
            ]
        );
    }

    #[test]
    fn very_narrow_header_keeps_current_workspace_tab_visible() {
        let mut app = make_app(28);
        app.current_tab = Tab::Usage;

        let rows = render_header(&mut app, 28);
        let row = row_text(&rows, 1);

        assert!(row.contains("Tok"), "{row}");
        assert!(row.contains("Use"), "{row}");
        assert_eq!(
            header_tabs_for_layout(&app, true),
            vec![
                Tab::Daily,
                Tab::Usage,
                Tab::Models,
                Tab::Pulse,
                Tab::Overview
            ]
        );
    }
}
