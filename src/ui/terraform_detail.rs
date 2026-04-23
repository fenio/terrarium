use std::collections::HashMap;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::config::DetailField;
use crate::k8s::terraform::Terraform;
use crate::ui::theme;

const LABEL: Style = Style::new().fg(Color::Rgb(140, 145, 165));
const SEP: &str = "  │  ";

pub fn render(
    f: &mut Frame,
    area: Rect,
    tf: &Terraform,
    runner_logs: Option<&str>,
    cached_outputs: Option<&HashMap<String, String>>,
    detail_fields: &[DetailField],
) {
    let ns = tf.metadata.namespace.as_deref().unwrap_or("-");
    let name = tf.metadata.name.as_deref().unwrap_or("-");

    // Fixed heights
    let spec_status_height = 8_u16;
    let conditions_height = 7_u16;

    if runner_logs.is_some() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                   // Title
                Constraint::Length(spec_status_height),  // Spec + Status
                Constraint::Length(conditions_height),   // Conditions
                Constraint::Min(5),                     // Logs
            ])
            .split(area);

        render_title(f, chunks[0], ns, name);
        render_spec_status(f, chunks[1], tf, cached_outputs, detail_fields);
        render_conditions_compact(f, chunks[2], tf);
        let runner_pod = format!("{}-tf-runner", name);
        render_runner_logs(f, chunks[3], runner_logs.unwrap(), &runner_pod);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                   // Title
                Constraint::Length(spec_status_height),  // Spec + Status
                Constraint::Length(conditions_height),   // Conditions
                Constraint::Min(0),                     // Remaining
            ])
            .split(area);

        render_title(f, chunks[0], ns, name);
        render_spec_status(f, chunks[1], tf, cached_outputs, detail_fields);
        render_conditions_compact(f, chunks[2], tf);
    }
}

fn render_title(f: &mut Frame, area: Rect, ns: &str, name: &str) {
    let title = Line::from(vec![
        Span::styled(
            " Terraform: ",
            Style::default()
                .fg(Color::Rgb(140, 145, 165))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{}/{}", ns, name),
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(title), area);
}

fn render_spec_status(
    f: &mut Frame,
    area: Rect,
    tf: &Terraform,
    cached_outputs: Option<&HashMap<String, String>>,
    detail_fields: &[DetailField],
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    render_spec(f, cols[0], tf);
    render_status(f, cols[1], tf, cached_outputs, detail_fields);
}

fn render_runner_logs(f: &mut Frame, area: Rect, logs: &str, pod_name: &str) {
    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(
                " Runner Logs ",
                Style::default()
                    .fg(Color::Rgb(100, 220, 140))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                pod_name,
                Style::default().fg(Color::Rgb(140, 200, 255)),
            ),
            Span::styled(
                " (live) ",
                Style::default().fg(Color::Rgb(80, 80, 100)),
            ),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(50, 55, 70)));

    let inner_height = area.height.saturating_sub(2) as usize;
    let line_count = logs.lines().count();
    let scroll = if line_count > inner_height {
        (line_count - inner_height) as u16
    } else {
        0
    };

    let para = Paragraph::new(logs)
        .block(block)
        .scroll((scroll, 0))
        .style(Style::default().fg(Color::Rgb(180, 180, 200)));
    f.render_widget(para, area);
}

fn render_spec(f: &mut Frame, area: Rect, tf: &Terraform) {
    let source = format!(
        "{:?}/{}",
        tf.spec.source_ref.kind, tf.spec.source_ref.name
    );
    let path = tf.spec.path.as_deref().unwrap_or(".");
    let interval = &tf.spec.interval;
    let suspended = tf.spec.suspend.unwrap_or(false);
    let workspace = tf.spec.workspace.as_deref().unwrap_or("default");
    let plan_only = tf.spec.plan_only.unwrap_or(false);
    let destroy = tf.spec.destroy.unwrap_or(false);
    let approve_plan = tf.spec.approve_plan.as_deref().unwrap_or("-");

    let sep_style = Style::default().fg(Color::Rgb(50, 55, 70));

    let lines = vec![
        kv("Source:    ", &source),
        kv("Path:      ", path),
        Line::from(vec![
            Span::styled("Interval: ", LABEL),
            Span::raw(interval),
            Span::styled(SEP, sep_style),
            Span::styled("Workspace: ", LABEL),
            Span::raw(workspace),
        ]),
        Line::from(vec![
            Span::styled("Suspended: ", LABEL),
            styled_bool(suspended),
            Span::styled(SEP, sep_style),
            Span::styled("PlanOnly: ", LABEL),
            styled_bool(plan_only),
            Span::styled(SEP, sep_style),
            Span::styled("Destroy: ", LABEL),
            styled_bool(destroy),
        ]),
        kv("Approve:   ", approve_plan),
    ];

    let block = Block::default()
        .title(Span::styled(
            " Spec ",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(50, 55, 70)));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_status(
    f: &mut Frame,
    area: Rect,
    tf: &Terraform,
    cached_outputs: Option<&HashMap<String, String>>,
    detail_fields: &[DetailField],
) {
    let status = tf.status.as_ref();
    let sep_style = Style::default().fg(Color::Rgb(50, 55, 70));

    let ready = status
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
        .map(|c| c.status.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let ready_style = match ready.as_str() {
        "True" => theme::STATUS_READY,
        "False" => theme::STATUS_NOT_READY,
        _ => theme::STATUS_UNKNOWN,
    };

    let plan_status = status
        .and_then(|s| s.plan.as_ref())
        .map(|p| {
            if let Some(pending) = &p.pending {
                format!("Pending: {}", pending)
            } else if let Some(applied) = &p.last_applied {
                format!("Applied: {}", applied)
            } else {
                "-".to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string());

    let last_applied = status
        .and_then(|s| s.last_applied_revision.as_deref())
        .unwrap_or("-");
    let drift = status
        .and_then(|s| s.last_drift_detected_at.as_deref())
        .unwrap_or("-");
    let failures = status
        .and_then(|s| s.reconciliation_failures)
        .unwrap_or(0);
    let inventory_count = status
        .and_then(|s| s.inventory.as_ref())
        .map(|i| i.entries.len())
        .unwrap_or(0);

    let failures_text = format!("{}", failures);
    let inventory_text = format!("{}", inventory_count);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Ready:     ", LABEL),
            Span::styled(&ready, ready_style),
            Span::styled(SEP, sep_style),
            Span::styled("Plan: ", LABEL),
            Span::raw(&plan_status),
        ]),
        kv("Applied:   ", last_applied),
        kv("Drift:     ", drift),
        Line::from(vec![
            Span::styled("Failures:  ", LABEL),
            if failures > 0 {
                Span::styled(&failures_text, theme::STATUS_NOT_READY)
            } else {
                Span::raw(&failures_text)
            },
            Span::styled(SEP, sep_style),
            Span::styled("Inventory: ", LABEL),
            Span::raw(&inventory_text),
        ]),
    ];

    // Render config-driven detail fields from outputs secret (two per line)
    if !detail_fields.is_empty() {
        for pair in detail_fields.chunks(2) {
            let mut spans = Vec::new();
            for (j, field) in pair.iter().enumerate() {
                if j > 0 {
                    spans.push(Span::styled(SEP, sep_style));
                }
                let value = cached_outputs
                    .and_then(|o| o.get(&field.source))
                    .map(|s| s.as_str())
                    .unwrap_or("-");
                let padded_label = format!("{}: ", field.label);
                spans.push(Span::styled(padded_label, LABEL));
                let mut style = Style::default().fg(Color::Rgb(
                    field.color[0],
                    field.color[1],
                    field.color[2],
                ));
                if field.bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                spans.push(Span::styled(value.to_string(), style));
            }
            lines.push(Line::from(spans));
        }
    } else if let Some(outputs) = cached_outputs {
        if !outputs.is_empty() {
            let mut keys: Vec<&str> = outputs.keys().map(|s| s.as_str()).collect();
            keys.sort();
            let keys_text = keys.join(", ");
            lines.push(Line::from(vec![
                Span::styled("Outputs:   ", LABEL),
                Span::styled(
                    keys_text,
                    Style::default().fg(Color::Rgb(100, 105, 120)),
                ),
            ]));
        }
    }

    let block = Block::default()
        .title(Span::styled(
            " Status ",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(50, 55, 70)));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_conditions_compact(f: &mut Frame, area: Rect, tf: &Terraform) {
    let conditions = tf.status.as_ref().and_then(|s| s.conditions.as_ref());

    let block = Block::default()
        .title(Span::styled(
            " Conditions ",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(50, 55, 70)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(conditions) = conditions {
        let msg_style = Style::default().fg(Color::Rgb(140, 140, 160));
        // Prefix: " ✓ " (3) + type (14) + status (8) = 25 columns
        let prefix_width: usize = 25;
        let msg_width = (inner.width as usize).saturating_sub(prefix_width);

        let mut lines: Vec<Line> = Vec::new();
        for c in conditions {
            let icon = if c.status == "True" { "✓" } else { "✗" };
            let style = if c.status == "True" {
                theme::STATUS_READY
            } else {
                theme::STATUS_NOT_READY
            };

            if msg_width == 0 || c.message.len() <= msg_width {
                lines.push(Line::from(vec![
                    Span::styled(format!(" {} ", icon), style),
                    Span::styled(
                        format!("{:<14}", c.type_),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{:<8}", c.status), style),
                    Span::styled(&c.message, msg_style),
                ]));
            } else {
                // Split message into wrapped lines
                let msg = &c.message;
                let mut pos = 0;
                let mut first = true;
                while pos < msg.len() {
                    let mut end = (pos + msg_width).min(msg.len());
                    // Ensure we don't split in the middle of a multi-byte UTF-8 character
                    while end < msg.len() && !msg.is_char_boundary(end) {
                        end -= 1;
                    }
                    let chunk = &msg[pos..end];
                    if first {
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {} ", icon), style),
                            Span::styled(
                                format!("{:<14}", c.type_),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(format!("{:<8}", c.status), style),
                            Span::styled(chunk, msg_style),
                        ]));
                        first = false;
                    } else {
                        lines.push(Line::from(vec![
                            Span::raw(" ".repeat(prefix_width)),
                            Span::styled(chunk, msg_style),
                        ]));
                    }
                    pos = end;
                }
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
    } else {
        f.render_widget(Paragraph::new("  No conditions"), inner);
    }
}

fn kv<'a>(key: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![Span::styled(key, LABEL), Span::raw(value)])
}

fn styled_bool(value: bool) -> Span<'static> {
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
