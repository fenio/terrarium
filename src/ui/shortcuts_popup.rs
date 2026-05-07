use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::config::Shortcut;
use crate::state::store::AppState;
use crate::ui::detail;
use crate::ui::theme;

pub fn render(f: &mut Frame, state: &AppState) {
    let Some((ns, name)) = state.shortcuts_popup_resource.as_ref() else {
        return;
    };
    let shortcuts = &state.config.shortcuts;
    if shortcuts.is_empty() {
        return;
    }

    let area = centered_rect(78, shortcuts.len(), f.area());
    f.render_widget(Clear, area);

    let title = format!(" Shortcuts — {ns}/{name} ");
    let block = detail::block(&title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Reserve a footer row for the help hint.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    render_entries(f, chunks[0], state, shortcuts, ns, name);
    render_footer(f, chunks[1]);
}

fn render_entries(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    shortcuts: &[Shortcut],
    ns: &str,
    name: &str,
) {
    let selected = state.shortcuts_popup_selected;
    let key_style = Style::default()
        .fg(Color::Rgb(140, 200, 255))
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default()
        .fg(Color::Rgb(220, 230, 255))
        .add_modifier(Modifier::BOLD);
    let url_style = Style::default().fg(Color::Rgb(140, 145, 165));
    let arrow_style = Style::default().fg(Color::Rgb(70, 80, 100));
    let submenu_style = Style::default()
        .fg(Color::Rgb(240, 200, 60))
        .add_modifier(Modifier::ITALIC);
    let selected_style = Style::default().bg(Color::Rgb(40, 50, 70));

    let label_width = shortcuts.iter().map(|s| s.label.len()).max().unwrap_or(0);

    let lines: Vec<Line> = shortcuts
        .iter()
        .enumerate()
        .map(|(i, sc)| {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(Span::raw(" "));
            spans.push(Span::styled(format!("[{}]", sc.key), key_style));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("{:<width$}", sc.label, width = label_width),
                label_style,
            ));
            spans.push(Span::raw("  "));
            spans.push(Span::styled("→ ", arrow_style));

            match sc.url.as_deref() {
                Some(template) => {
                    spans.push(Span::styled(
                        partial_resolve(template, &state.context_name, ns, name),
                        url_style,
                    ));
                }
                None if !sc.children.is_empty() => {
                    spans.push(Span::styled(
                        format!("(submenu, {} entries)", sc.children.len()),
                        submenu_style,
                    ));
                }
                None => {
                    spans.push(Span::styled("(no url)", submenu_style));
                }
            }

            let mut line = Line::from(spans);
            if i == selected {
                line = line.style(selected_style);
            }
            line
        })
        .collect();

    f.render_widget(Paragraph::new(lines), area);
}

fn render_footer(f: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled("j/k", Style::default().fg(Color::Rgb(140, 200, 255))),
        Span::styled(":nav  ", Style::default().fg(Color::Rgb(140, 145, 165))),
        Span::styled("Enter", Style::default().fg(Color::Rgb(140, 200, 255))),
        Span::styled(":open  ", Style::default().fg(Color::Rgb(140, 145, 165))),
        Span::styled("[key]", Style::default().fg(Color::Rgb(140, 200, 255))),
        Span::styled(":direct  ", Style::default().fg(Color::Rgb(140, 145, 165))),
        Span::styled("Esc", Style::default().fg(Color::Rgb(140, 200, 255))),
        Span::styled(":close", Style::default().fg(Color::Rgb(140, 145, 165))),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::STATUS_BAR_BG)),
        area,
    );
}

/// Substitute the always-known placeholders so the user sees a mostly
/// real URL. `{output.X}` placeholders stay as-is — they're resolved
/// lazily when the shortcut activates.
fn partial_resolve(template: &str, context: &str, namespace: &str, name: &str) -> String {
    template
        .replace("{context}", context)
        .replace("{namespace}", namespace)
        .replace("{name}", name)
}

fn centered_rect(percent_x: u16, n_entries: usize, r: Rect) -> Rect {
    // Height = top border + entries + footer + bottom border
    let height = (n_entries as u16 + 4)
        .min(r.height.saturating_sub(4))
        .max(5);
    let width = r.width * percent_x / 100;
    let x = r.x + (r.width.saturating_sub(width)) / 2;
    let y = r.y + (r.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}
