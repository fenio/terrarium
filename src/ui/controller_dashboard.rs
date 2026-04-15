use std::collections::BTreeMap;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::state::store::AppState;
use crate::ui::theme;
use crate::util;

pub fn render(f: &mut Frame, area: Rect, state: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),  // Controller info
            Constraint::Length(10), // Terraform stats
            Constraint::Length(6),  // Kustomization stats
            Constraint::Min(3),    // Backlog / stale
        ])
        .split(area);

    render_controller_info(f, chunks[0], state);
    render_tf_stats(f, chunks[1], state);
    render_ks_stats(f, chunks[2], state);
    render_backlog(f, chunks[3], state);
}

fn render_controller_info(f: &mut Frame, area: Rect, state: &AppState) {
    let info = &state.controller_info;

    let block = Block::default()
        .title(Span::styled(
            " Controller ",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(50, 55, 70)));

    if let Some(err) = &info.error {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", err),
                Style::default().fg(Color::Rgb(240, 200, 60)),
            )),
        ];
        f.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }

    let health_style = if info.replicas_ready >= info.replicas_desired && info.replicas_desired > 0
    {
        theme::STATUS_READY
    } else if info.replicas_ready > 0 {
        theme::STATUS_PENDING
    } else {
        theme::STATUS_NOT_READY
    };

    let health_text = if info.replicas_ready >= info.replicas_desired && info.replicas_desired > 0 {
        "Healthy"
    } else if info.replicas_ready > 0 {
        "Degraded"
    } else {
        "Unhealthy"
    };

    let running_runners = state
        .runner_pods
        .iter()
        .filter(|p| {
            p.status
                .as_ref()
                .and_then(|s| s.phase.as_deref())
                .map(|ph| ph == "Running" || ph == "Pending")
                .unwrap_or(false)
        })
        .count();

    let runners_line = if let Some(max) = info.max_concurrent {
        let runner_style = if running_runners as i32 >= max {
            theme::STATUS_NOT_READY
        } else if running_runners > 0 {
            theme::STATUS_PENDING
        } else {
            Style::default().fg(Color::Rgb(110, 115, 135))
        };
        Line::from(vec![
            Span::styled("  Runners:     ", theme::LABEL),
            Span::styled(format!("{}/{}", running_runners, max), runner_style),
        ])
    } else {
        Line::from(vec![
            Span::styled("  Runners:     ", theme::LABEL),
            Span::styled(format!("{}", running_runners), Style::default().fg(Color::Rgb(110, 115, 135))),
        ])
    };

    let mut lines = vec![
        kv_line("  Deployment:  ", &info.deploy_name),
        kv_line("  Namespace:   ", &info.deploy_namespace),
        Line::from(vec![
            Span::styled("  Status:      ", theme::LABEL),
            Span::styled(health_text, health_style),
            Span::styled(
                format!("  ({}/{} replicas ready)", info.replicas_ready, info.replicas_desired),
                theme::LABEL,
            ),
        ]),
        kv_line("  Image:       ", &info.image),
        runners_line,
    ];

    if !info.pods.is_empty() {
        lines.push(Line::from(""));
        for pod in &info.pods {
            let ready_icon = if pod.ready { "✓" } else { "✗" };
            let ready_style = if pod.ready {
                theme::STATUS_READY
            } else {
                theme::STATUS_NOT_READY
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", ready_icon), ready_style),
                Span::raw(&pod.name),
                Span::styled(
                    format!("  {}  restarts:{}  age:{}",
                        pod.phase, pod.restarts, pod.age),
                    theme::LABEL,
                ),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_tf_stats(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(Span::styled(
            " Terraform Resources ",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(50, 55, 70)));

    if !state.tf_synced {
        let dots = ".".repeat((state.tick_count % 3) + 1);
        let lines = vec![Line::from(vec![
            Span::styled("  syncing", theme::LABEL),
            Span::styled(dots, theme::LABEL),
        ])];
        f.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }

    let all_tfs: Vec<_> = state.tf_store.state().iter().map(|a| (**a).clone()).collect();
    let total = all_tfs.len();

    let ready_count = all_tfs
        .iter()
        .filter(|tf| is_condition_true(tf.status.as_ref().and_then(|s| s.conditions.as_ref()), "Ready"))
        .count();
    let not_ready = total - ready_count;
    let suspended = all_tfs
        .iter()
        .filter(|tf| tf.spec.suspend.unwrap_or(false))
        .count();
    let pending_plans = all_tfs
        .iter()
        .filter(|tf| {
            tf.status
                .as_ref()
                .and_then(|s| s.plan.as_ref())
                .and_then(|p| p.pending.as_ref())
                .is_some()
        })
        .count();
    let drift_detected = all_tfs
        .iter()
        .filter(|tf| {
            tf.status
                .as_ref()
                .and_then(|s| s.last_drift_detected_at.as_ref())
                .is_some()
        })
        .count();
    let total_failures: i64 = all_tfs
        .iter()
        .filter_map(|tf| tf.status.as_ref().and_then(|s| s.reconciliation_failures))
        .sum();
    let inventory_total: usize = all_tfs
        .iter()
        .filter_map(|tf| {
            tf.status
                .as_ref()
                .and_then(|s| s.inventory.as_ref())
                .map(|i| i.entries.len())
        })
        .sum();

    let lines = vec![
        Line::from(vec![
            Span::styled("  Total:        ", theme::LABEL),
            Span::styled(format!("{}", total), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        stat_line("  Ready:        ", ready_count, theme::STATUS_READY),
        stat_line("  Not Ready:    ", not_ready, theme::STATUS_NOT_READY),
        stat_line("  Suspended:    ", suspended, theme::STATUS_PENDING),
        stat_line("  Pending Plans:", pending_plans, theme::STATUS_PENDING),
        stat_line("  Drift Detect: ", drift_detected, Style::default().fg(Color::Rgb(200, 140, 255))),
        Line::from(vec![
            Span::styled("  Recon Fails:  ", theme::LABEL),
            Span::styled(
                format!("{}", total_failures),
                if total_failures > 0 { theme::STATUS_NOT_READY } else { theme::STATUS_READY },
            ),
        ]),
        Line::from(vec![
            Span::styled("  Managed Res:  ", theme::LABEL),
            Span::raw(format!("{}", inventory_total)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_ks_stats(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(Span::styled(
            " Kustomization Resources ",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(50, 55, 70)));

    if !state.ks_synced {
        let dots = ".".repeat((state.tick_count % 3) + 1);
        let lines = vec![Line::from(vec![
            Span::styled("  syncing", theme::LABEL),
            Span::styled(dots, theme::LABEL),
        ])];
        f.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }

    let all_ks: Vec<_> = state.ks_store.state().iter().map(|a| (**a).clone()).collect();
    let total = all_ks.len();

    let ready_count = all_ks
        .iter()
        .filter(|ks| is_condition_true(ks.status.as_ref().and_then(|s| s.conditions.as_ref()), "Ready"))
        .count();
    let not_ready = total - ready_count;
    let suspended = all_ks
        .iter()
        .filter(|ks| ks.spec.suspend.unwrap_or(false))
        .count();

    let lines = vec![
        Line::from(vec![
            Span::styled("  Total:        ", theme::LABEL),
            Span::styled(format!("{}", total), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        stat_line("  Ready:        ", ready_count, theme::STATUS_READY),
        stat_line("  Not Ready:    ", not_ready, theme::STATUS_NOT_READY),
        stat_line("  Suspended:    ", suspended, theme::STATUS_PENDING),
    ];

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_backlog(f: &mut Frame, area: Rect, state: &mut AppState) {
    let block = Block::default()
        .title(Span::styled(
            " Backlog (past due interval) ",
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(50, 55, 70)));

    let all_tfs: Vec<_> = state.tf_store.state().iter().map(|a| (**a).clone()).collect();

    // For each TF, check if time since last Ready transition exceeds its spec.interval.
    // Classify stale TFs as "waiting" (Ready=True, just backlogged) or "failing" (Ready!=True).
    // ns -> (waiting, failing, total)
    let mut stale_by_ns: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();

    for tf in &all_tfs {
        if tf.spec.suspend.unwrap_or(false) {
            continue;
        }

        let ns = tf.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let entry = stale_by_ns.entry(ns).or_insert((0, 0, 0));
        entry.2 += 1;

        let interval_secs = match util::parse_k8s_duration(&tf.spec.interval) {
            Some(s) => s,
            None => continue,
        };

        let ready_condition = tf
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"));

        let elapsed = ready_condition.map(|c| util::secs_since(c.last_transition_time.0));

        if let Some(elapsed) = elapsed {
            if elapsed > interval_secs + 300 {
                let is_ready = ready_condition
                    .map(|c| c.status == "True")
                    .unwrap_or(false);
                if is_ready {
                    entry.0 += 1; // waiting — healthy but behind schedule
                } else {
                    entry.1 += 1; // failing — broken
                }
            }
        }
    }

    // Update cached backlog list in state (used by Enter handler)
    let mut backlog_entries: Vec<(String, usize, usize, usize)> = stale_by_ns
        .into_iter()
        .filter(|(_, (w, f, _))| *w + *f > 0)
        .map(|(ns, (w, f, t))| (ns, w, f, t))
        .collect();
    backlog_entries.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2))); // most stale first
    state.backlog_namespaces = backlog_entries;

    let total_waiting: usize = state.backlog_namespaces.iter().map(|(_, w, _, _)| *w).sum();
    let total_failing: usize = state.backlog_namespaces.iter().map(|(_, _, f, _)| *f).sum();
    let total_tracked: usize = all_tfs.iter().filter(|tf| !tf.spec.suspend.unwrap_or(false)).count();

    let label = theme::LABEL;
    let selected = state.backlog_table_state.selected();

    // Header area (summary line)
    let inner = block.inner(area);
    let list_area = inner;

    f.render_widget(block, area);

    // Numbers left, namespace right. Legend above as header.
    let w_w = format!("{}", total_waiting).len().max(1);
    let w_f = format!("{}", total_failing).len().max(1);
    let w_t = format!("{}", total_tracked).len().max(1);

    fn num_spans<'a>(
        w: usize, f: usize, t: usize,
        ww: usize, wf: usize, wt: usize,
        label_style: Style,
    ) -> Vec<Span<'a>> {
        vec![
            Span::styled(format!("{:>w$}", w, w = ww), theme::STATUS_PENDING),
            Span::styled("/", label_style),
            Span::styled(format!("{:>w$}", f, w = wf), theme::STATUS_NOT_READY),
            Span::styled("/", label_style),
            Span::styled(format!("{:>w$}", t, w = wt), Style::default().fg(Color::White)),
        ]
    }

    let mut lines: Vec<Line> = Vec::new();

    // Header: totals + legend
    let mut header_spans = vec![Span::styled("  ", label)];
    header_spans.extend(num_spans(total_waiting, total_failing, total_tracked, w_w, w_f, w_t, label));
    header_spans.push(Span::styled("  ", label));
    header_spans.push(Span::styled("waiting", theme::STATUS_PENDING));
    header_spans.push(Span::styled("/", label));
    header_spans.push(Span::styled("failing", theme::STATUS_NOT_READY));
    header_spans.push(Span::styled("/total", label));
    lines.push(Line::from(header_spans));

    // Namespace rows
    for (i, (ns, waiting, failing, total)) in state.backlog_namespaces.iter().enumerate() {
        let is_selected = selected == Some(i);
        let row_bg = if is_selected {
            Style::default().bg(Color::Rgb(40, 50, 80))
        } else {
            Style::default()
        };
        let ns_style = if is_selected {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let mut row_spans = vec![Span::styled("  ", row_bg)];
        row_spans.extend(num_spans(*waiting, *failing, *total, w_w, w_f, w_t, label));
        row_spans.push(Span::styled("  ", row_bg));
        row_spans.push(Span::styled(ns.as_str(), ns_style));
        lines.push(Line::from(row_spans));
    }

    f.render_widget(Paragraph::new(lines), list_area);
}

fn kv_line<'a>(key: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(key, theme::LABEL),
        Span::raw(value),
    ])
}

fn stat_line(label: &str, count: usize, style: Style) -> Line<'_> {
    Line::from(vec![
        Span::styled(label, theme::LABEL),
        Span::styled(format!("{}", count), if count > 0 { style } else { Style::default().fg(Color::Rgb(110, 115, 135)) }),
    ])
}

fn is_condition_true(
    conditions: Option<&Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition>>,
    type_name: &str,
) -> bool {
    conditions
        .and_then(|cs| cs.iter().find(|c| c.type_ == type_name))
        .map(|c| c.status == "True")
        .unwrap_or(false)
}
