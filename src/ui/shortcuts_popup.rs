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
    if state.config.shortcuts.is_empty() {
        return;
    }
    // Only entries whose `when` matches the open resource are shown.
    // Computed once when the popup opens; we just project the indices
    // back to Shortcut references here.
    let shortcuts: Vec<&Shortcut> = state
        .shortcuts_popup_visible
        .iter()
        .filter_map(|&i| state.config.shortcuts.get(i))
        .collect();
    if shortcuts.is_empty() {
        return;
    }

    let title = format!(" Shortcuts — {ns}/{name} ");

    // Build the body lines once. The same vec drives both the popup
    // height calculation and the render — so what you see is exactly
    // what was sized for, no clipping or trailing whitespace.
    let label_width = shortcuts.iter().map(|s| s.label.len()).max().unwrap_or(0);
    let area_for_sizing = popup_rect(f.area(), &shortcuts, &title, label_width);
    let inner_width = area_for_sizing.width.saturating_sub(2) as usize;
    let lines = build_lines(state, &shortcuts, label_width, inner_width);

    f.render_widget(Clear, area_for_sizing);
    let block = detail::block(&title);
    let inner = block.inner(area_for_sizing);
    f.render_widget(block, area_for_sizing);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),                  // top padding
            Constraint::Length(lines.len() as u16), // body
            Constraint::Length(1),                  // separator
            Constraint::Length(1),                  // footer
            Constraint::Length(1),                  // bottom padding
        ])
        .split(inner);

    f.render_widget(Paragraph::new(lines), chunks[1]);
    render_footer(f, chunks[3]);
}

fn build_lines(
    state: &AppState,
    shortcuts: &[&Shortcut],
    label_width: usize,
    inner_width: usize,
) -> Vec<Line<'static>> {
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

    let desc_budget = inner_width.saturating_sub(
        KEY_BRACKETS_WIDTH + label_width + LABEL_DESC_GAP + 1, /* leading sp */
    );

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_group: Option<String> = None;
    let mut first = true;

    for (i, sc) in shortcuts.iter().copied().enumerate() {
        let group_changed = sc.group != current_group;
        if group_changed {
            if !first {
                // Blank row before transitioning to a new group.
                lines.push(Line::from(""));
            }
            if let Some(g) = &sc.group {
                lines.push(section_header(g, inner_width));
            }
            current_group = sc.group.clone();
        }
        first = false;

        lines.push(entry_line(
            sc,
            i == selected,
            label_width,
            desc_budget,
            key_style,
            label_style,
            desc_style,
            dim_style,
            dash_style,
            selected_marker,
            selected_bg,
        ));
    }

    lines
}

#[allow(clippy::too_many_arguments)]
fn entry_line(
    sc: &Shortcut,
    is_sel: bool,
    label_width: usize,
    desc_budget: usize,
    key_style: Style,
    label_style: Style,
    desc_style: Style,
    dim_style: Style,
    dash_style: Style,
    selected_marker: Style,
    selected_bg: Color,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if is_sel {
        spans.push(Span::styled("▎ ", selected_marker));
    } else {
        spans.push(Span::raw("  "));
    }
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

    Line::from(spans)
}

/// Section header: a styled "pill" with the group name on a contrasting
/// background, followed by a faint rule that fills the remaining inner
/// width. Reads as a labeled separator rather than a bare word.
fn section_header(name: &str, inner_width: usize) -> Line<'static> {
    let pill_text = format!(" {name} ");
    let pill_style = Style::default()
        .fg(Color::Rgb(20, 25, 35))
        .bg(Color::Rgb(140, 200, 255))
        .add_modifier(Modifier::BOLD);
    let rule_style = Style::default().fg(Color::Rgb(70, 80, 100));
    let pill_cells = pill_text.chars().count();
    let rule_len =
        inner_width.saturating_sub(pill_cells + 3 /* leading space + " " gap + 1 */);
    Line::from(vec![
        Span::raw(" "),
        Span::styled(pill_text, pill_style),
        Span::raw(" "),
        Span::styled("─".repeat(rule_len), rule_style),
    ])
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

/// Center a popup whose default size is generous (~60% screen width)
/// and which only grows beyond that to fit content. Height is
/// content-driven (entries + section headers + chrome).
fn popup_rect(screen: Rect, shortcuts: &[&Shortcut], title: &str, label_width: usize) -> Rect {
    let desc_width = shortcuts
        .iter()
        .map(|s| description_for(s).chars().count())
        .max()
        .unwrap_or(0);

    let title_width = title.chars().count() + 4;
    let footer_width = " j/k:nav  Enter:open  [key]:direct  Esc:close ".len() + 2;
    let entry_width = SIDE_PADDING
        + KEY_BRACKETS_WIDTH
        + label_width
        + LABEL_DESC_GAP
        + if desc_width > 0 { 2 + desc_width } else { 0 };

    let preferred = (screen.width as usize) * 60 / 100;
    let max = (screen.width as usize) * 90 / 100;
    let min_required = title_width.max(footer_width).max(entry_width);
    let width = preferred.max(min_required).min(max) as u16;

    // Body height: entries + group headers + blanks between groups.
    // Entries within the same group are flush (no blank rows between).
    let mut body_rows: usize = 0;
    let mut prev_group: Option<&str> = None;
    let mut first = true;
    for sc in shortcuts {
        let g = sc.group.as_deref();
        let group_changed = g != prev_group;
        if group_changed {
            if !first {
                body_rows += 1; // blank between groups
            }
            if g.is_some() {
                body_rows += 1; // section header
            }
            prev_group = g;
        }
        first = false;
        body_rows += 1; // the entry itself
    }
    // chrome: 2 top-pad + 1 separator + 1 footer + 1 bottom-pad + 2 borders = 7
    let chrome = 7;
    let max_h = (screen.height as usize) * 80 / 100;
    let height = (body_rows + chrome)
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
