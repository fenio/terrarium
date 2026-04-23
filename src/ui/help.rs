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
    let mut lines = Vec::new();

    // Navigation
    lines.push(section_header("Navigation"));
    lines.push(help_line("j/k ↑/↓", "Move selection"));
    lines.push(help_line("Ctrl-d / PgDn", "Half page down"));
    lines.push(help_line("Ctrl-u / PgUp", "Half page up"));
    lines.push(help_line("Enter / l", "Open detail / logs"));
    lines.push(help_line("Esc", "Back / clear filter"));
    lines.push(help_line("q", "Quit"));
    lines.push(Line::from(""));

    // Tabs
    lines.push(section_header("Tabs"));
    lines.push(help_line("1-5", "Jump to tab"));
    lines.push(help_line("Tab", "Next tab"));
    lines.push(help_line("Shift+Tab", "Previous tab"));
    lines.push(Line::from(""));

    // Filtering & Search
    lines.push(section_header("Filtering"));
    lines.push(help_line("/", "Search / filter list"));
    lines.push(help_line("\\", "Pause / resume filter"));
    lines.push(help_line("f", "Toggle failures only"));
    lines.push(help_line("w", "Toggle waiting only"));
    lines.push(help_line("n", "Namespace picker"));
    lines.push(help_line("o", "Cycle sort column"));
    lines.push(help_line("i", "Invert sort direction"));
    lines.push(help_line("!", "Jump to first failure"));
    lines.push(Line::from(""));

    // General
    lines.push(section_header("General"));
    lines.push(help_line("m", "Toggle mouse support"));
    lines.push(help_line("?", "Toggle this help"));
    lines.push(help_line("Ctrl+C", "Quit immediately"));

    lines
}

fn build_right_column() -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Terraform Actions
    lines.push(section_header("Terraform Actions"));
    lines.push(help_line("a", "Approve pending plan"));
    lines.push(help_line("r", "Reconcile"));
    lines.push(help_line("R", "Replan"));
    lines.push(help_line("p", "View plan"));
    lines.push(help_line("O", "View outputs"));
    lines.push(help_line("y / Y", "View JSON / YAML"));
    lines.push(help_line("e", "View events"));
    lines.push(help_line("s / u", "Suspend / Resume"));
    lines.push(help_line("F", "Force unlock state"));
    lines.push(help_line("L", "Stream runner logs"));
    lines.push(help_line("x", "Break the glass (tfctl)"));
    lines.push(help_line("d", "Delete resource"));
    lines.push(Line::from(""));

    // Kustomization Actions
    lines.push(section_header("Kustomization Actions"));
    lines.push(help_line("r", "Reconcile"));
    lines.push(help_line("y / Y", "View JSON / YAML"));
    lines.push(help_line("e", "View events"));
    lines.push(help_line("s / u", "Suspend / Resume"));
    lines.push(Line::from(""));

    // Runner Actions
    lines.push(section_header("Runner Actions"));
    lines.push(help_line("e", "View events"));
    lines.push(help_line("T", "Jump to Terraform detail"));
    lines.push(help_line("d", "Kill runner pod"));
    lines.push(Line::from(""));

    // Viewer
    lines.push(section_header("Plan / Log / JSON Viewer"));
    lines.push(help_line("g / G", "Top / bottom (G=follow)"));
    lines.push(help_line("h / l", "Scroll left / right"));
    lines.push(help_line("/", "Search in content"));
    lines.push(help_line("n / N", "Next / prev match"));
    lines.push(help_line("w", "Toggle line wrap"));
    lines.push(help_line("S", "Save to file"));
    lines.push(help_line("Tab", "Switch container (logs)"));

    lines
}

fn section_header(title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {} ", title),
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
