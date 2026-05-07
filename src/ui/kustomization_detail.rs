use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::k8s::kustomization::Kustomization;
use crate::k8s::source::GitRepository;
use crate::ui::detail::{self, SEP};
use crate::ui::source_summary;
use crate::ui::theme;

pub fn render(
    f: &mut Frame,
    area: Rect,
    ks: &Kustomization,
    source_gr: Option<&GitRepository>,
    gr_synced: bool,
) {
    let ns = ks.metadata.namespace.as_deref().unwrap_or("-");
    let name = ks.metadata.name.as_deref().unwrap_or("-");

    let spec_status_height = 8_u16;
    let conditions_height = 7_u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                   // Title
            Constraint::Length(spec_status_height),  // Spec + Status
            Constraint::Length(conditions_height),   // Conditions
            Constraint::Min(0),                     // Remaining
        ])
        .split(area);

    detail::render_title(f, chunks[0], "Kustomization", ns, name);
    render_spec_status(f, chunks[1], ks, source_gr, gr_synced);
    let conditions = ks.status.as_ref().and_then(|s| s.conditions.as_ref());
    detail::render_conditions(f, chunks[2], conditions);
}

fn render_spec_status(
    f: &mut Frame,
    area: Rect,
    ks: &Kustomization,
    source_gr: Option<&GitRepository>,
    gr_synced: bool,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    render_spec(f, cols[0], ks, source_gr, gr_synced);
    render_status(f, cols[1], ks);
}

fn render_spec(
    f: &mut Frame,
    area: Rect,
    ks: &Kustomization,
    source_gr: Option<&GitRepository>,
    gr_synced: bool,
) {
    let source_kind = format!("{:?}", ks.spec.source_ref.kind);
    let path = ks.spec.path.as_deref().unwrap_or(".");
    let interval = ks.spec.interval.as_str();
    let suspended = ks.spec.suspend.unwrap_or(false);
    let prune = ks.spec.prune;
    let target_ns = ks.spec.target_namespace.as_deref().unwrap_or("-");
    let timeout = ks.spec.timeout.as_deref().unwrap_or("-");
    let depends_on = ks
        .spec
        .depends_on
        .as_ref()
        .map(|deps| deps.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_else(|| "-".to_string());

    let lines = vec![
        source_summary::source_line(
            "Source:    ",
            theme::LABEL,
            &source_kind,
            &ks.spec.source_ref.name,
            source_gr,
            gr_synced,
        ),
        detail::kv("Path:      ", path),
        Line::from(vec![
            Span::styled("Interval:  ", theme::LABEL),
            Span::raw(interval),
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("Target NS: ", theme::LABEL),
            Span::raw(target_ns),
        ]),
        Line::from(vec![
            Span::styled("Suspended: ", theme::LABEL),
            detail::styled_bool(suspended),
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("Prune: ", theme::LABEL),
            detail::styled_bool(prune),
        ]),
        Line::from(vec![
            Span::styled("Timeout:   ", theme::LABEL),
            Span::raw(timeout),
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("Depends: ", theme::LABEL),
            Span::raw(depends_on),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(detail::block("Spec")), area);
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
    let inventory_text = format!("{inventory_count}");

    let lines = vec![
        Line::from(vec![
            Span::styled("Ready:     ", theme::LABEL),
            Span::styled(&ready, ready_style),
            Span::styled(SEP, theme::INLINE_SEP),
            Span::styled("Inventory: ", theme::LABEL),
            Span::raw(&inventory_text),
        ]),
        detail::kv("Applied:   ", last_applied),
        detail::kv("Attempted: ", last_attempted),
    ];

    f.render_widget(Paragraph::new(lines).block(detail::block("Status")), area);
}
