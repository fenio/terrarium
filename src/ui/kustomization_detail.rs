use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::k8s::kustomization::Kustomization;
use crate::ui::theme;

pub fn render(f: &mut Frame, area: Rect, ks: &Kustomization) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(10),   // Body
            Constraint::Length(6), // Conditions
        ])
        .split(area);

    let ns = ks.metadata.namespace.as_deref().unwrap_or("-");
    let name = ks.metadata.name.as_deref().unwrap_or("-");
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "Kustomization: ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{}/{}", ns, name)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    render_spec(f, body_chunks[0], ks);
    render_status(f, body_chunks[1], ks);
    render_conditions(f, chunks[2], ks);
}

fn render_spec(f: &mut Frame, area: Rect, ks: &Kustomization) {
    let source = format!(
        "{:?}/{}",
        ks.spec.source_ref.kind, ks.spec.source_ref.name
    );
    let path = ks.spec.path.as_deref().unwrap_or(".");
    let interval = &ks.spec.interval;
    let suspended = ks.spec.suspend.unwrap_or(false);
    let prune = ks.spec.prune;
    let target_ns = ks
        .spec
        .target_namespace
        .as_deref()
        .unwrap_or("-");
    let timeout = ks.spec.timeout.as_deref().unwrap_or("-");
    let depends_on = ks
        .spec
        .depends_on
        .as_ref()
        .map(|deps| {
            deps.iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "-".to_string());

    let suspended_text = format!("{}", suspended);
    let prune_text = format!("{}", prune);
    let lines = vec![
        kv_line("Source:       ", &source),
        kv_line("Path:         ", path),
        kv_line("Interval:     ", interval),
        kv_line("Suspended:    ", &suspended_text),
        kv_line("Prune:        ", &prune_text),
        kv_line("Target NS:    ", target_ns),
        kv_line("Timeout:      ", timeout),
        kv_line("Depends On:   ", &depends_on),
    ];

    let block = Block::default().title(" Spec ").borders(Borders::ALL);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_status(f: &mut Frame, area: Rect, ks: &Kustomization) {
    let status = ks.status.as_ref();

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

    let last_applied = status
        .and_then(|s| s.last_applied_revision.as_deref())
        .unwrap_or("-");
    let last_attempted = status
        .and_then(|s| s.last_attempted_revision.as_deref())
        .unwrap_or("-");
    let inventory_count = status
        .and_then(|s| s.inventory.as_ref())
        .map(|i| i.entries.len())
        .unwrap_or(0);

    let inventory_text = format!("{} resources", inventory_count);
    let lines = vec![
        Line::from(vec![
            Span::styled("Ready:         ", Style::default().fg(Color::DarkGray)),
            Span::styled(&ready, ready_style),
        ]),
        kv_line("Last Applied:  ", last_applied),
        kv_line("Last Attempted:", last_attempted),
        kv_line("Inventory:     ", &inventory_text),
    ];

    let block = Block::default().title(" Status ").borders(Borders::ALL);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_conditions(f: &mut Frame, area: Rect, ks: &Kustomization) {
    let conditions = ks.status.as_ref().and_then(|s| s.conditions.as_ref());

    let lines: Vec<Line> = if let Some(conditions) = conditions {
        conditions
            .iter()
            .map(|c| {
                let icon = if c.status == "True" { "✓" } else { "✗" };
                let style = if c.status == "True" {
                    theme::STATUS_READY
                } else {
                    theme::STATUS_NOT_READY
                };
                Line::from(vec![
                    Span::styled(format!(" {} ", icon), style),
                    Span::styled(
                        format!("{:<15}", c.type_),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{:<8}", c.status), style),
                    Span::raw(&c.message),
                ])
            })
            .collect()
    } else {
        vec![Line::from("  No conditions")]
    };

    let block = Block::default()
        .title(" Conditions ")
        .borders(Borders::ALL);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn kv_line<'a>(key: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(key, Style::default().fg(Color::DarkGray)),
        Span::raw(value),
    ])
}
