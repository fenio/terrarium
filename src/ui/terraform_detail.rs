use std::collections::HashMap;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::config::DetailField;
use crate::k8s::source::GitRepository;
use crate::k8s::terraform::Terraform;
use crate::ui::detail::{self, SEP};
use crate::ui::source_summary;
use crate::ui::theme;

pub struct RenderCtx<'a> {
    pub runner_logs: Option<&'a str>,
    pub cached_outputs: Option<&'a HashMap<String, String>>,
    pub detail_fields: &'a [DetailField],
    pub source_gr: Option<&'a GitRepository>,
    pub gr_synced: bool,
}

pub fn render(f: &mut Frame, area: Rect, tf: &Terraform, ctx: &RenderCtx<'_>) {
    let ns = tf.metadata.namespace.as_deref().unwrap_or("-");
    let name = tf.metadata.name.as_deref().unwrap_or("-");

    // Fixed heights — Conditions stays at 5 inner rows; the dedicated
    // viewer (`c`) handles full multi-screen error blobs.
    let spec_status_height = 8_u16;
    let conditions_height = 7_u16;

    if ctx.runner_logs.is_some() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                  // Title
                Constraint::Length(spec_status_height), // Spec + Status
                Constraint::Length(conditions_height),  // Conditions
                Constraint::Min(5),                     // Logs
            ])
            .split(area);

        detail::render_title(f, chunks[0], "Terraform", ns, name);
        render_spec_status(f, chunks[1], tf, ctx);
        let conditions = tf.status.as_ref().and_then(|s| s.conditions.as_ref());
        detail::render_conditions(f, chunks[2], conditions);
        let runner_pod = format!("{name}-tf-runner");
        render_runner_logs(f, chunks[3], ctx.runner_logs.unwrap(), &runner_pod);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                  // Title
                Constraint::Length(spec_status_height), // Spec + Status
                Constraint::Length(conditions_height),  // Conditions
                Constraint::Min(0),                     // Remaining
            ])
            .split(area);

        detail::render_title(f, chunks[0], "Terraform", ns, name);
        render_spec_status(f, chunks[1], tf, ctx);
        let conditions = tf.status.as_ref().and_then(|s| s.conditions.as_ref());
        detail::render_conditions(f, chunks[2], conditions);
    }
}

fn render_spec_status(f: &mut Frame, area: Rect, tf: &Terraform, ctx: &RenderCtx<'_>) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    render_spec(f, cols[0], tf, ctx.source_gr, ctx.gr_synced);
    render_status(f, cols[1], tf, ctx.cached_outputs, ctx.detail_fields);
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
            Span::styled(pod_name, Style::default().fg(Color::Rgb(140, 200, 255))),
            Span::styled(" (live) ", Style::default().fg(Color::Rgb(80, 80, 100))),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::BORDER);

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

fn render_spec(
    f: &mut Frame,
    area: Rect,
    tf: &Terraform,
    source_gr: Option<&GitRepository>,
    gr_synced: bool,
) {
    let source_kind = format!("{:?}", tf.spec.source_ref.kind);
    let path = tf.spec.path.as_deref().unwrap_or(".");
    let interval = &tf.spec.interval;
    let suspended = tf.spec.suspend.unwrap_or(false);
    let workspace = tf.spec.workspace.as_deref().unwrap_or("default");
    let plan_only = tf.spec.plan_only.unwrap_or(false);
    let destroy = tf.spec.destroy.unwrap_or(false);
    let break_the_glass = crate::k8s::actions::break_the_glass_active(tf);
    let approve_plan = tf.spec.approve_plan.as_deref().unwrap_or("-");

    let lines = vec![
        source_summary::source_line(
            "Source:    ",
            theme::LABEL,
            &source_kind,
            &tf.spec.source_ref.name,
            source_gr,
            gr_synced,
        ),
        detail::kv("Path:      ", path),
        Line::from(vec![
            Span::styled("Interval: ", theme::LABEL),
            Span::raw(interval),
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("Workspace: ", theme::LABEL),
            Span::raw(workspace),
        ]),
        Line::from(vec![
            Span::styled("Suspended: ", theme::LABEL),
            detail::styled_bool(suspended),
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("PlanOnly: ", theme::LABEL),
            detail::styled_bool(plan_only),
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("Destroy: ", theme::LABEL),
            detail::styled_bool(destroy),
        ]),
        Line::from(vec![
            Span::styled("BreakGlass: ", theme::LABEL),
            detail::styled_bool(break_the_glass),
        ]),
        detail::kv("Approve:   ", approve_plan),
    ];

    f.render_widget(Paragraph::new(lines).block(detail::block("Spec")), area);
}

fn render_status(
    f: &mut Frame,
    area: Rect,
    tf: &Terraform,
    cached_outputs: Option<&HashMap<String, String>>,
    detail_fields: &[DetailField],
) {
    let status = tf.status.as_ref();

    let (ready, ready_style) = crate::ui::resource_list::ready_label_and_style(
        crate::util::classify_ready(status.and_then(|s| s.conditions.as_ref())),
    );

    let plan_status = status
        .and_then(|s| s.plan.as_ref())
        .map(|p| {
            if let Some(pending) = &p.pending {
                format!("Pending: {pending}")
            } else if let Some(applied) = &p.last_applied {
                format!("Applied: {applied}")
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
    let failures = status.and_then(|s| s.reconciliation_failures).unwrap_or(0);
    let inventory_count = status
        .and_then(|s| s.inventory.as_ref())
        .map(|i| i.entries.len())
        .unwrap_or(0);

    let failures_text = format!("{failures}");
    let inventory_text = format!("{inventory_count}");

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Ready:     ", theme::LABEL),
            Span::styled(&ready, ready_style),
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("Plan: ", theme::LABEL),
            Span::raw(&plan_status),
        ]),
        detail::kv("Applied:   ", last_applied),
        detail::kv("Drift:     ", drift),
        Line::from(vec![
            Span::styled("Failures:  ", theme::LABEL),
            if failures > 0 {
                Span::styled(&failures_text, theme::STATUS_NOT_READY)
            } else {
                Span::raw(&failures_text)
            },
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("Inventory: ", theme::LABEL),
            Span::raw(&inventory_text),
        ]),
    ];

    // Render config-driven detail fields from outputs secret (two per line)
    if !detail_fields.is_empty() {
        for pair in detail_fields.chunks(2) {
            let mut spans = Vec::new();
            for (j, field) in pair.iter().enumerate() {
                if j > 0 {
                    spans.push(Span::styled(SEP, theme::INLINE_SEP));
                }
                let value = cached_outputs
                    .and_then(|o| o.get(&field.source))
                    .map(|s| s.as_str())
                    .unwrap_or("-");
                let padded_label = format!("{}: ", field.label);
                spans.push(Span::styled(padded_label, theme::LABEL));
                let mut style =
                    Style::default().fg(Color::Rgb(field.color[0], field.color[1], field.color[2]));
                if field.bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                spans.push(Span::styled(value.to_string(), style));
            }
            lines.push(Line::from(spans));
        }
    } else if let Some(outputs) = cached_outputs
        && !outputs.is_empty()
    {
        let mut keys: Vec<&str> = outputs.keys().map(|s| s.as_str()).collect();
        keys.sort();
        let keys_text = keys.join(", ");
        lines.push(Line::from(vec![
            Span::styled("Outputs:   ", theme::LABEL),
            Span::styled(keys_text, Style::default().fg(Color::Rgb(100, 105, 120))),
        ]));
    }

    f.render_widget(Paragraph::new(lines).block(detail::block("Status")), area);
}
