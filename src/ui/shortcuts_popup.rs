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

const KEY_BRACKETS_WIDTH: usize = 5; // "[k]  "
const LABEL_DESC_GAP: usize = 3; //  "   "
const SIDE_PADDING: usize = 4; // border + 1 inner column on each side

pub fn render(f: &mut Frame, state: &AppState) {
    let Some((ns, name)) = state.shortcuts_popup_resource.as_ref() else {
        return;
    };
    let shortcuts = &state.config.shortcuts;
    if shortcuts.is_empty() {
        return;
    }

    let area = popup_rect(f.area(), shortcuts);
    f.render_widget(Clear, area);

    let title = format!(" Shortcuts — {ns}/{name} ");
    let block = detail::block(&title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // top padding
            Constraint::Min(1),    // entries
            Constraint::Length(1), // separator/blank
            Constraint::Length(1), // footer hint
        ])
        .split(inner);

    render_entries(f, chunks[1], state, shortcuts);
    render_footer(f, chunks[3]);
}

fn render_entries(f: &mut Frame, area: Rect, state: &AppState, shortcuts: &[Shortcut]) {
    let selected = state.shortcuts_popup_selected;

    let key_style = Style::default()
        .fg(Color::Rgb(140, 200, 255))
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default()
        .fg(Color::Rgb(220, 230, 255))
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::Rgb(170, 175, 195));
    let dim_style = Style::default().fg(Color::Rgb(110, 115, 135));
    let dash_style = Style::default().fg(Color::Rgb(70, 80, 100));
    let selected_style = Style::default().bg(Color::Rgb(40, 50, 70));

    let label_width = shortcuts.iter().map(|s| s.label.len()).max().unwrap_or(0);
    let max_inner_width = area.width as usize;
    let desc_budget = max_inner_width.saturating_sub(
        KEY_BRACKETS_WIDTH + label_width + LABEL_DESC_GAP + 1, /* leading sp */
    );

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

            let detail_text = description_for(sc);
            if !detail_text.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled("— ", dash_style));
                let trimmed = truncate_visual(&detail_text, desc_budget.saturating_sub(2));
                let style = if sc.url.is_none() && !sc.children.is_empty() {
                    Style::default()
                        .fg(Color::Rgb(240, 200, 60))
                        .add_modifier(Modifier::ITALIC)
                } else if sc.url.is_some() {
                    desc_style
                } else {
                    dim_style
                };
                spans.push(Span::styled(trimmed, style));
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

/// Body text shown after the label. Prefer the user-supplied
/// description; fall back to a hint about submenu/no-url states so
/// the entry isn't bare.
fn description_for(sc: &Shortcut) -> String {
    if let Some(d) = &sc.description {
        return d.clone();
    }
    if !sc.children.is_empty() {
        return format!("submenu ({} entries)", sc.children.len());
    }
    if sc.url.is_none() {
        return "(no url)".to_string();
    }
    String::new()
}

fn truncate_visual(s: &str, budget: usize) -> String {
    if s.chars().count() <= budget {
        return s.to_string();
    }
    if budget <= 1 {
        return "…".to_string();
    }
    let take = budget.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('…');
    out
}

/// Center a popup whose width fits the longest entry (capped at 80%
/// of the terminal) and whose height fits all entries plus chrome.
fn popup_rect(screen: Rect, shortcuts: &[Shortcut]) -> Rect {
    let label_width = shortcuts.iter().map(|s| s.label.len()).max().unwrap_or(0);
    let desc_width = shortcuts
        .iter()
        .map(|s| description_for(s).chars().count())
        .max()
        .unwrap_or(0);

    let footer_width = " j/k:nav  Enter:open  [key]:direct  Esc:close ".len();
    // " [k]  label  — description  " plus side padding
    let entry_width = SIDE_PADDING
        + KEY_BRACKETS_WIDTH
        + label_width
        + LABEL_DESC_GAP
        + if desc_width > 0 { 2 + desc_width } else { 0 };
    let title_width = 30; // leave room for "Shortcuts — ns/name"

    let max = (screen.width as usize) * 80 / 100;
    let min_w = footer_width.max(title_width);
    let width = entry_width.max(min_w).min(max).max(min_w) as u16;

    // chrome: 1 top-pad + 1 separator + 1 footer + 2 borders = 5
    let chrome = 5;
    let height = (shortcuts.len() as u16 + chrome).min(screen.height.saturating_sub(2));

    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}
