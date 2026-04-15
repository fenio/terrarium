use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
    Frame,
};

use crate::state::store::AppState;

pub fn render(f: &mut Frame, state: &AppState) {
    let area = centered_rect(40, 60, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(Span::styled(
            " Namespace ",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(100, 160, 220)));

    let items: Vec<ListItem> = state
        .ns_picker_items
        .iter()
        .enumerate()
        .map(|(i, ns)| {
            let is_current = match &state.namespace_filter {
                None => i == 0,    // "(all namespaces)" is index 0
                Some(f) => ns == f,
            };
            let style = if is_current {
                Style::default()
                    .fg(Color::Rgb(100, 220, 140))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let marker = if is_current { " * " } else { "   " };
            ListItem::new(Span::styled(
                format!("{}{}", marker, ns),
                style,
            ))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(45, 50, 70))
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = ListState::default();
    list_state.select(Some(state.ns_picker_selected));

    f.render_stateful_widget(list, area, &mut list_state);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(v[1])[1]
}
