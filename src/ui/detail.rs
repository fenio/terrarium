use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::ui::theme;
use crate::util;

/// In-text separator between two key/value cells on the same row.
pub const SEP: &str = "  │  ";

/// Build a panel block with the standard rounded border, bright accent,
/// and a styled title. Used by every detail-view section so the chrome
/// stays consistent across kinds.
pub fn block(title: &str) -> Block<'static> {
    Block::default()
        .title(Span::styled(format!(" {title} "), theme::BLOCK_TITLE))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::BORDER)
}

pub fn kv<'a>(key: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![Span::styled(key, theme::LABEL), Span::raw(value)])
}

pub fn styled_bool(value: bool) -> Span<'static> {
    if value {
        Span::styled(
            "true",
            Style::default()
                .fg(Color::Rgb(240, 200, 60))
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("false", Style::default().fg(Color::Rgb(80, 85, 100)))
    }
}

/// One-line title row: ` Kind: ns/name`.
pub fn render_title(f: &mut Frame, area: Rect, kind: &str, ns: &str, name: &str) {
    let line = Line::from(vec![
        Span::styled(
            format!(" {kind}: "),
            Style::default()
                .fg(Color::Rgb(140, 145, 165))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{ns}/{name}"),
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// Prefix width consumed by the icon + type + status columns in the
/// Conditions panel.
const CONDITIONS_PREFIX_WIDTH: usize = 25;
const CONDITIONS_TYPE_PAD: usize = 14;

/// Render the Conditions panel: icon + type + status + humanized message.
/// Wraps the message body to the panel width, indenting continuation
/// lines under the message column.
pub fn render_conditions(f: &mut Frame, area: Rect, conditions: Option<&Vec<Condition>>) {
    // Custom block (instead of `block("Conditions")`) so the title can
    // carry a dim "(c for full view)" affordance — the inline panel
    // shows the first conditions only, and users routinely forget the
    // dedicated Conditions viewer exists for the full, scrollable text.
    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(" Conditions ", theme::BLOCK_TITLE),
            Span::styled(
                "(c for full view) ",
                Style::default().fg(Color::Rgb(110, 115, 130)),
            ),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::BORDER);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(conditions) = conditions else {
        f.render_widget(Paragraph::new("  No conditions"), inner);
        return;
    };

    let msg_style = Style::default().fg(Color::Rgb(180, 180, 200));
    // Prefix: " ✓ " (3) + type (14) + status (8) = 25 columns
    let type_pad: usize = CONDITIONS_TYPE_PAD;
    let prefix_width: usize = CONDITIONS_PREFIX_WIDTH;
    let msg_width = (inner.width as usize).saturating_sub(prefix_width);

    let mut lines: Vec<Line> = Vec::new();
    for c in conditions {
        let (icon, style) = match c.status.as_str() {
            "True" => ("✓", theme::STATUS_READY),
            "False" => ("✗", theme::STATUS_NOT_READY),
            _ => ("⋯", theme::STATUS_UNKNOWN),
        };

        let humanized = util::humanize_condition_message(&c.message);
        let logical_lines: Vec<&str> = if humanized.is_empty() {
            vec![c.message.as_str()]
        } else {
            humanized.iter().map(String::as_str).collect()
        };

        let mut first_chunk = true;
        for logical in &logical_lines {
            let mut pos = 0;
            let single_pass = msg_width == 0 || logical.len() <= msg_width;
            loop {
                let chunk = if single_pass {
                    let slice = &logical[pos..];
                    pos = logical.len();
                    slice
                } else {
                    let mut end = (pos + msg_width).min(logical.len());
                    while end < logical.len() && !logical.is_char_boundary(end) {
                        end -= 1;
                    }
                    let slice = &logical[pos..end];
                    pos = end;
                    slice
                };
                if first_chunk {
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {icon} "), style),
                        Span::styled(
                            format!("{:<width$}", c.type_, width = type_pad),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!("{:<8}", c.status), style),
                        Span::styled(chunk.to_string(), msg_style),
                    ]));
                    first_chunk = false;
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(prefix_width)),
                        Span::styled(chunk.to_string(), msg_style),
                    ]));
                }
                if pos >= logical.len() {
                    break;
                }
            }
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}
