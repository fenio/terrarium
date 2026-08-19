use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::ui::theme;

pub fn render_confirm(f: &mut Frame, message: &str) {
    let area = centered_rect(50, 30, f.area());

    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .style(theme::DIALOG_BORDER);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(2)])
        .margin(1)
        .split(inner);

    let msg = Paragraph::new(message);
    f.render_widget(msg, chunks[0]);

    let buttons = Line::from(vec![
        Span::styled(
            " [y]es ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            " [n]o ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    ]);
    let buttons_para = Paragraph::new(buttons);
    f.render_widget(buttons_para, chunks[1]);
}

/// Type-to-confirm dialog for destructive actions. The user must type
/// `expected` (echoed as `input`) before the wrapped action fires.
pub fn render_typed_confirm(f: &mut Frame, message: &str, expected: &str, input: &str) {
    let area = centered_rect(60, 40, f.area());

    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Confirm destructive action ")
        .borders(Borders::ALL)
        .style(theme::DIALOG_BORDER);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(inner);

    let msg = Paragraph::new(message).wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(msg, chunks[0]);

    let matches = input == expected;
    let input_style = if matches {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let prompt = Line::from(vec![
        Span::styled(format!("Type \"{expected}\": "), theme::LABEL),
        Span::styled(input.to_string(), input_style),
        Span::styled(
            "_",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(prompt), chunks[1]);

    let hint = if matches {
        Line::from(Span::styled(
            " [Enter] confirm   [Esc] cancel ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(Span::styled(
            " Type the name to enable confirm   [Esc] cancel ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
    };
    f.render_widget(Paragraph::new(hint), chunks[2]);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
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
        .split(popup_layout[1])[1]
}
