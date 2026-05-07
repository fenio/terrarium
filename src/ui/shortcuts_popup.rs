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

    let title = format!(" Shortcuts — {ns}/{name} ");
    let area = popup_rect(f.area(), shortcuts, &title);
    f.render_widget(Clear, area);

    let block = detail::block(&title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // top padding
            Constraint::Min(1),    // entries (with blank rows between)
            Constraint::Length(1), // separator
            Constraint::Length(1), // footer hint
            Constraint::Length(1), // bottom padding
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
    let selected_bg = Color::Rgb(60, 90, 140);
    let selected_marker = Style::default()
        .fg(Color::Rgb(140, 200, 255))
        .bg(selected_bg)
        .add_modifier(Modifier::BOLD);

    let label_width = shortcuts.iter().map(|s| s.label.len()).max().unwrap_or(0);
    let max_inner_width = area.width as usize;
    let desc_budget = max_inner_width.saturating_sub(
        KEY_BRACKETS_WIDTH + label_width + LABEL_DESC_GAP + 1, /* leading sp */
    );

    // Render each entry with a blank row separating it from the next,
    // so the list breathes a bit instead of feeling cramped.
    let mut lines: Vec<Line> = Vec::with_capacity(shortcuts.len() * 2);
    for (i, sc) in shortcuts.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        let is_sel = i == selected;
        let mut spans: Vec<Span> = Vec::new();
        // Left marker: a bright cyan ▎ block on the selected row, two
        // spaces of padding otherwise. Reads as a clear "you are here".
        if is_sel {
            spans.push(Span::styled("▎ ", selected_marker));
        } else {
            spans.push(Span::raw("  "));
        }
        // Build the row's body with selected-background applied to each
        // span so the highlight extends behind the styled text.
        let entry_bg = if is_sel { Some(selected_bg) } else { None };
        let with_bg = |s: Style| match entry_bg {
            Some(bg) => s.bg(bg),
            None => s,
        };

        spans.push(Span::styled(format!("[{}]", sc.key), with_bg(key_style)));
        spans.push(Span::styled("  ", with_bg(Style::default())));
        spans.push(Span::styled(
            format!("{:<width$}", sc.label, width = label_width),
            with_bg(label_style),
        ));

        let detail_text = description_for(sc);
        if !detail_text.is_empty() {
            spans.push(Span::styled("  ", with_bg(Style::default())));
            spans.push(Span::styled("— ", with_bg(dash_style)));
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
            spans.push(Span::styled(trimmed, with_bg(style)));
        }

        lines.push(Line::from(spans));
    }

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

/// Center a popup whose default size is generous (~60% screen width,
/// ~50% height with breathing room) and which only grows beyond that
/// to fit content the user actually wrote.
fn popup_rect(screen: Rect, shortcuts: &[Shortcut], title: &str) -> Rect {
    let label_width = shortcuts.iter().map(|s| s.label.len()).max().unwrap_or(0);
    let desc_width = shortcuts
        .iter()
        .map(|s| description_for(s).chars().count())
        .max()
        .unwrap_or(0);

    // Hard minimums (in cells) so the chrome always fits even with very
    // short labels and no descriptions.
    let title_width = title.chars().count() + 4;
    let footer_width = " j/k:nav  Enter:open  [key]:direct  Esc:close ".len() + 2;
    let entry_width = SIDE_PADDING
        + KEY_BRACKETS_WIDTH
        + label_width
        + LABEL_DESC_GAP
        + if desc_width > 0 { 2 + desc_width } else { 0 };

    // Default to ~60% of the screen so a popup with two short entries
    // still feels like a popup, not a tooltip. Cap at 90%.
    let preferred = (screen.width as usize) * 60 / 100;
    let max = (screen.width as usize) * 90 / 100;
    let min_required = title_width.max(footer_width).max(entry_width);
    let width = preferred.max(min_required).min(max) as u16;

    // 1 entry row + 1 blank between = 2 cells per entry beyond the first.
    let entry_rows = if shortcuts.is_empty() {
        0
    } else {
        shortcuts.len() * 2 - 1
    };
    // chrome: 2 top-pad + 1 separator + 1 footer + 1 bottom-pad + 2 borders = 7
    let chrome = 7;
    // Size to fit content. Cap at 80% screen so very long shortcut lists
    // still leave the body visible, but no enforced minimum height —
    // a 2-entry popup shouldn't fill half the screen with whitespace.
    let max_h = (screen.height as usize) * 80 / 100;
    let height = (entry_rows + chrome)
        .min(max_h)
        .min(screen.height.saturating_sub(2) as usize) as u16;

    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}
