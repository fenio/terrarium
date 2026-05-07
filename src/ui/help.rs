use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::ui::theme;

pub fn render_help(f: &mut Frame) {
    let area = centered_rect(80, 85, f.area());
    f.render_widget(Clear, area);

    let title = format!(" Terrarium v{} — press ? or Esc to close ", env!("CARGO_PKG_VERSION"));
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(Color::Rgb(140, 200, 255)).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(80, 90, 120)))
        .style(Style::default().bg(Color::Rgb(22, 22, 34)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split into two columns
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    let left = build_left_column();
    let right = build_right_column();

    f.render_widget(Paragraph::new(left), columns[0]);
    f.render_widget(Paragraph::new(right), columns[1]);
}

fn build_left_column() -> Vec<Line<'static>> {
    vec![
        section_header("Navigation"),
        help_line("j/k ↑/↓", "Move selection"),
        help_line("Ctrl-d / PgDn", "Half page down"),
        help_line("Ctrl-u / PgUp", "Half page up"),
        help_line("Enter / l", "Open detail / logs"),
        help_line("Esc", "Back / clear filter"),
        help_line("q", "Quit"),
        Line::from(""),
        section_header("Tabs"),
        help_line("1-5", "Jump to tab"),
        help_line("Tab", "Next tab"),
        help_line("Shift+Tab", "Previous tab"),
        Line::from(""),
        section_header("Filtering"),
        help_line("/", "Search / filter list"),
        help_line("\\", "Pause / resume filter"),
        help_line("f", "Toggle failures only"),
        help_line("w", "Toggle waiting only"),
        help_line("n", "Namespace picker"),
        help_line("o", "Cycle sort column"),
        help_line("i", "Invert sort direction"),
        help_line("!", "Jump to first failure"),
        Line::from(""),
        section_header("General"),
        help_line("m", "Toggle mouse support"),
        help_line("?", "Toggle this help"),
        help_line("Ctrl+C", "Quit immediately"),
    ]
}

fn build_right_column() -> Vec<Line<'static>> {
    vec![
        section_header("Terraform Actions"),
        help_line("a", "Approve pending plan"),
        help_line("r", "Reconcile"),
        help_line("R", "Replan"),
        help_line("p", "View plan"),
        help_line("O", "View outputs"),
        help_line("y / Y", "View JSON / YAML"),
        help_line("e", "View events"),
        help_line("s / u", "Suspend / Resume"),
        help_line("F", "Force unlock state"),
        help_line("L", "Stream runner logs"),
        help_line("x", "Break the glass (tfctl)"),
        help_line("d", "Delete resource"),
        Line::from(""),
        section_header("Kustomization Actions"),
        help_line("r", "Reconcile"),
        help_line("y / Y", "View JSON / YAML"),
        help_line("e", "View events"),
        help_line("s / u", "Suspend / Resume"),
        Line::from(""),
        section_header("Runner Actions"),
        help_line("e", "View events"),
        help_line("T", "Jump to Terraform detail"),
        help_line("d", "Kill runner pod"),
        Line::from(""),
        section_header("Plan / Log / JSON Viewer"),
        help_line("g / G", "Top / bottom (G=follow)"),
        help_line("h / l", "Scroll left / right"),
        help_line("/", "Search in content"),
        help_line("n / N", "Next / prev match"),
        help_line("w", "Toggle line wrap"),
        help_line("S", "Save to file"),
        help_line("Tab", "Switch container (logs)"),
    ]
}

fn section_header(title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "─".repeat(30usize.saturating_sub(title.len() + 2)),
            Style::default().fg(Color::Rgb(50, 55, 70)),
        ),
    ])
}

fn help_line(key: &str, desc: &'static str) -> Line<'static> {
    // Pad key to fixed visual width of 16 columns.
    // Unicode arrows (↑↓) are 1 display column each but multi-byte in UTF-8,
    // so we count unicode width for correct alignment.
    let display_width: usize = key.chars().count(); // good enough — all chars here are 1-wide
    let padded = format!("{}{}", key, " ".repeat(16usize.saturating_sub(display_width)));
    Line::from(vec![
        Span::raw("  "),
        Span::styled(padded, theme::STATUS_PENDING),
        Span::styled(desc, Style::default().fg(Color::White)),
    ])
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
