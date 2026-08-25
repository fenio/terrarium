use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Row, Table},
};

use crate::k8s::terraform::Terraform;
use crate::k8s::watcher::TfStore;
use crate::state::store::{AppState, SelectionIdentity, SortColumn, TabKind};
use crate::ui::theme;
use crate::util;

pub fn render_terraform_list(f: &mut Frame, area: Rect, state: &mut AppState) {
    let items = get_filtered_terraforms(
        &state.tf_store,
        &state.namespace_filter,
        state.effective_search_query(),
        state.show_failures_only,
        state.show_waiting_only,
        state.show_progressing_only,
        state.show_drifting_only,
        state.show_deleting_only,
        &state.recently_acted,
        state.sort_column,
        state.sort_descending,
    );
    let identities: Vec<SelectionIdentity> = items
        .iter()
        .map(|tf| {
            SelectionIdentity::new(
                tf.metadata.namespace.clone().unwrap_or_default(),
                tf.metadata.name.clone().unwrap_or_default(),
                tf.metadata.uid.clone(),
            )
        })
        .collect();
    state.reconcile_selection_for(&TabKind::Terraform, &identities);
    state.reconcile_bulk_selection_for(&TabKind::Terraform, &identities);

    let active = state.sort_column;
    let desc = state.sort_descending;
    let header = Row::new(vec![
        Cell::from(" "),
        sort_cell("NAMESPACE", active == SortColumn::Namespace, desc),
        sort_cell("NAME", active == SortColumn::Name, desc),
        sort_cell("READY", active == SortColumn::Ready, desc),
        Cell::from("S"),
        Cell::from("PLAN"),
        sort_cell("REVISION", active == SortColumn::Revision, desc),
        sort_cell("LAST APPLIED", active == SortColumn::LastApplied, desc),
        sort_cell("AGE", active == SortColumn::Age, desc),
    ])
    .style(theme::COLUMN_HEADER)
    .bottom_margin(1);

    let rows: Vec<Row> = items
        .iter()
        .map(|tf| {
            let ns = tf.metadata.namespace.as_deref().unwrap_or("-");
            let name = tf.metadata.name.as_deref().unwrap_or("-");

            let (ready_text, ready_style) = get_ready_status(tf);
            let deleting = tf.metadata.deletion_timestamp.is_some();
            let bulk_cell = bulk_marker_cell(state, ns, name, tf.metadata.uid.as_deref(), deleting);
            let suspended_cell = if tf.spec.suspend.unwrap_or(false) {
                Cell::from(Span::styled("S", theme::SUSPENDED))
            } else {
                Cell::from(" ")
            };
            let plan_text = get_plan_status(tf);
            let revision = tf
                .status
                .as_ref()
                .and_then(|s| s.last_applied_revision.as_deref())
                .map(truncate_revision)
                .unwrap_or_else(|| "-".to_string());
            let last_applied = get_last_applied_time(tf);
            let age = get_age(tf);

            Row::new(vec![
                bulk_cell,
                Cell::from(ns.to_string()),
                Cell::from(name.to_string()),
                Cell::from(Span::styled(ready_text, ready_style)),
                suspended_cell,
                Cell::from(plan_text),
                Cell::from(revision),
                Cell::from(last_applied),
                Cell::from(age),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Percentage(12),
        Constraint::Percentage(23),
        Constraint::Percentage(8),
        Constraint::Length(2),
        Constraint::Percentage(10),
        Constraint::Percentage(15),
        Constraint::Percentage(16),
        Constraint::Percentage(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme::SELECTED_ROW)
        .block(crate::ui::detail::block("Terraform"));

    f.render_stateful_widget(table, area, &mut state.tf_table_state);
}

#[allow(clippy::too_many_arguments)]
pub fn get_filtered_terraforms(
    store: &TfStore,
    namespace_filter: &Option<String>,
    search_query: &str,
    failures_only: bool,
    waiting_only: bool,
    progressing_only: bool,
    drifting_only: bool,
    deleting_only: bool,
    recently_acted: &std::collections::HashMap<(String, String), std::time::Instant>,
    sort_column: SortColumn,
    descending: bool,
) -> Vec<Terraform> {
    let all: Vec<Terraform> = store.state().iter().map(|arc| (**arc).clone()).collect();
    let mut filtered: Vec<Terraform> = all
        .into_iter()
        .filter(|tf| {
            if let Some(ns) = namespace_filter {
                tf.metadata.namespace.as_deref() == Some(ns.as_str())
            } else {
                true
            }
        })
        .filter(|tf| {
            if search_query.is_empty() {
                true
            } else {
                let name = tf.metadata.name.as_deref().unwrap_or("");
                let ns = tf.metadata.namespace.as_deref().unwrap_or("");
                name.contains(search_query) || ns.contains(search_query)
            }
        })
        .filter(|tf| {
            // Grace-period bypass: rows the user just acted on stay
            // visible regardless of which state filter is active, so
            // they can watch the resource transition instead of having
            // it vanish out of the filtered view.
            let ns = tf.metadata.namespace.as_deref().unwrap_or("");
            let name = tf.metadata.name.as_deref().unwrap_or("");
            let in_grace = recently_acted.contains_key(&(ns.to_string(), name.to_string()));

            if failures_only {
                return in_grace
                    || util::classify_ready(
                        tf.status.as_ref().and_then(|s| s.conditions.as_ref()),
                    )
                    .is_real_failure();
            }
            if progressing_only {
                return in_grace
                    || matches!(
                        util::classify_ready(
                            tf.status.as_ref().and_then(|s| s.conditions.as_ref()),
                        ),
                        util::ReadyState::Reconciling
                    );
            }
            if waiting_only {
                return in_grace || is_waiting(tf);
            }
            if drifting_only {
                return in_grace || is_drifting_now(tf);
            }
            if deleting_only {
                return in_grace || tf.metadata.deletion_timestamp.is_some();
            }
            true
        })
        .collect();

    match sort_column {
        SortColumn::Namespace => filtered.sort_by(|a, b| {
            a.metadata
                .namespace
                .cmp(&b.metadata.namespace)
                .then(a.metadata.name.cmp(&b.metadata.name))
        }),
        SortColumn::Name => filtered.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name)),
        SortColumn::Ready => filtered.sort_by(|a, b| {
            let ready_a = get_ready_str(a);
            let ready_b = get_ready_str(b);
            ready_a
                .cmp(&ready_b)
                .then(a.metadata.name.cmp(&b.metadata.name))
        }),
        SortColumn::Revision => filtered.sort_by(|a, b| {
            // Ascending = lexicographic by last applied revision; resources
            // without a revision sort last so they don't pile up at the top.
            let rev_a = revision_str(a);
            let rev_b = revision_str(b);
            match (rev_a, rev_b) {
                (Some(x), Some(y)) => x.cmp(&y).then(a.metadata.name.cmp(&b.metadata.name)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.metadata.name.cmp(&b.metadata.name),
            }
        }),
        SortColumn::LastApplied => filtered.sort_by(|a, b| {
            // Ascending = oldest applied first; resources that have never applied sort last.
            let ts_a = applied_ts(a);
            let ts_b = applied_ts(b);
            match (ts_a, ts_b) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        }),
        SortColumn::Age => filtered.sort_by(|a, b| {
            // Ascending by creation timestamp = oldest first.
            let age_a = a.metadata.creation_timestamp.as_ref().map(|t| t.0);
            let age_b = b.metadata.creation_timestamp.as_ref().map(|t| t.0);
            age_a.cmp(&age_b)
        }),
    }

    if descending {
        filtered.reverse();
    }
    filtered
}

/// Marker cell for the leftmost list column. Layered priorities:
///   * Bulk-selected → `●` in BULK_SELECTED style.
///   * Recently acted on (within the grace window) → `↻` in
///     RECENTLY_ACTED style — flags rows that the active filter would
///     normally hide but that we keep visible so the user can watch
///     the controller pick them up.
///   * Resource has a `deletionTimestamp` (delete requested, but
///     finalizers haven't drained yet) → `☠` in DELETING style.
///   * Otherwise blank.
pub(crate) fn bulk_marker_cell(
    state: &AppState,
    namespace: &str,
    name: &str,
    uid: Option<&str>,
    deleting: bool,
) -> Cell<'static> {
    let key = (namespace.to_string(), name.to_string());
    if state
        .bulk_selected
        .get(&key)
        .is_some_and(|target| uid == Some(target.uid.as_str()))
    {
        Cell::from(Span::styled("●", theme::BULK_SELECTED))
    } else if state.is_recently_acted(namespace, name) {
        Cell::from(Span::styled("↻", theme::RECENTLY_ACTED))
    } else if deleting {
        Cell::from(Span::styled("☠", theme::DELETING))
    } else {
        Cell::from(" ")
    }
}

/// Build a column header cell that highlights when it's the active sort.
/// Arrow indicates direction: ▲ ascending, ▼ descending.
pub(crate) fn sort_cell(label: &'static str, active: bool, descending: bool) -> Cell<'static> {
    if active {
        let style = Style::default()
            .fg(Color::Rgb(140, 200, 255))
            .add_modifier(Modifier::BOLD);
        let arrow = if descending { " ▼" } else { " ▲" };
        Cell::from(Line::from(vec![
            Span::styled(label, style),
            Span::styled(arrow, style),
        ]))
    } else {
        Cell::from(label)
    }
}

fn applied_ts(tf: &Terraform) -> Option<jiff::Timestamp> {
    tf.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Apply"))
        .map(|c| c.last_transition_time.0)
}

fn revision_str(tf: &Terraform) -> Option<String> {
    tf.status
        .as_ref()
        .and_then(|s| s.last_applied_revision.as_deref())
        .map(|s| s.to_string())
}

fn get_ready_str(tf: &Terraform) -> String {
    tf.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
        .map(|c| c.status.clone())
        .unwrap_or_default()
}

fn get_ready_status(tf: &Terraform) -> (String, Style) {
    let conditions = tf.status.as_ref().and_then(|s| s.conditions.as_ref());
    ready_label_and_style(util::classify_ready(conditions))
}

pub(crate) fn ready_label_and_style(state: util::ReadyState) -> (String, Style) {
    match state {
        util::ReadyState::True => ("True".to_string(), theme::STATUS_READY),
        util::ReadyState::Reconciling => ("…".to_string(), theme::STATUS_RECONCILING),
        util::ReadyState::Failed => ("False".to_string(), theme::STATUS_NOT_READY),
        util::ReadyState::Unknown => ("Unknown".to_string(), theme::STATUS_UNKNOWN),
        util::ReadyState::Missing => ("-".to_string(), theme::STATUS_UNKNOWN),
    }
}

fn get_plan_status(tf: &Terraform) -> String {
    tf.status
        .as_ref()
        .and_then(|s| s.plan.as_ref())
        .map(|plan| {
            if plan.pending.is_some() {
                "Pending".to_string()
            } else if plan.last_applied.is_some() {
                "Applied".to_string()
            } else {
                "-".to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string())
}

fn truncate_revision(rev: &str) -> String {
    if let Some(idx) = rev.rfind('/') {
        let sha = &rev[idx + 1..];
        if sha.len() > 8 {
            format!("{}..{}", &rev[..idx], &sha[..8])
        } else {
            rev.to_string()
        }
    } else if rev.len() > 12 {
        format!("{}...", &rev[..12])
    } else {
        rev.to_string()
    }
}

fn get_age(tf: &Terraform) -> String {
    tf.metadata
        .creation_timestamp
        .as_ref()
        .map(|ts| util::format_duration(util::secs_since(ts.0)))
        .unwrap_or_else(|| "-".to_string())
}

fn get_last_applied_time(tf: &Terraform) -> String {
    tf.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Apply"))
        .map(|c| util::format_duration_ago(util::secs_since(c.last_transition_time.0)))
        .unwrap_or_else(|| "-".to_string())
}

/// Ready=True but past its reconciliation interval + 5min grace period.
fn is_waiting(tf: &Terraform) -> bool {
    if tf.spec.suspend.unwrap_or(false) {
        return false;
    }
    let ready_condition = tf
        .status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"));
    let is_ready = ready_condition.map(|c| c.status == "True").unwrap_or(false);
    if !is_ready {
        return false;
    }
    let interval_secs = match util::parse_k8s_duration(&tf.spec.interval) {
        Some(s) => s,
        None => return false,
    };
    let elapsed = ready_condition.map(|c| util::secs_since(c.last_transition_time.0));
    elapsed.map(|e| e > interval_secs + 300).unwrap_or(false)
}

pub(crate) fn is_drifting_now(tf: &Terraform) -> bool {
    let Some(status) = tf.status.as_ref() else {
        return false;
    };

    let drift_condition = status.conditions.as_ref().is_some_and(|conditions| {
        conditions.iter().any(|condition| {
            condition.type_ == "Ready"
                && condition.status == "False"
                && condition.reason == "DriftDetected"
        })
    });
    let drift_plan_pending = status.plan.as_ref().is_some_and(|plan| {
        plan.is_drift_detection_plan.unwrap_or(false) && plan.pending.is_some()
    });

    drift_condition || drift_plan_pending
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terraform_with_name_and_status(name: &str, status: serde_json::Value) -> Terraform {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "infra.contrib.fluxcd.io/v1alpha2",
            "kind": "Terraform",
            "metadata": {"name": name, "namespace": "ns"},
            "spec": {
                "interval": "1m",
                "sourceRef": {"kind": "GitRepository", "name": "source"}
            },
            "status": status
        }))
        .expect("minimal Terraform should deserialize")
    }

    fn terraform_with_status(status: serde_json::Value) -> Terraform {
        terraform_with_name_and_status("demo", status)
    }

    #[test]
    fn drifting_now_detects_drift_condition() {
        let tf = terraform_with_status(serde_json::json!({
            "conditions": [{
                "type": "Ready",
                "status": "False",
                "reason": "DriftDetected",
                "message": "drift detected",
                "lastTransitionTime": "2026-08-24T12:00:00Z"
            }]
        }));

        assert!(is_drifting_now(&tf));
    }

    #[test]
    fn drifting_now_detects_pending_drift_plan_only() {
        let tf = terraform_with_status(serde_json::json!({
            "plan": {
                "isDriftDetectionPlan": true,
                "pending": "plan-secret"
            }
        }));

        assert!(is_drifting_now(&tf));
    }

    #[test]
    fn ordinary_pending_plan_is_not_current_drift() {
        let tf = terraform_with_status(serde_json::json!({
            "plan": {
                "isDriftDetectionPlan": false,
                "pending": "plan-secret"
            }
        }));

        assert!(!is_drifting_now(&tf));
    }

    #[test]
    fn unrelated_ready_failure_is_not_current_drift() {
        let tf = terraform_with_status(serde_json::json!({
            "conditions": [{
                "type": "Ready",
                "status": "False",
                "reason": "TerraformPlanFailed",
                "message": "plan failed",
                "lastTransitionTime": "2026-08-24T12:00:00Z"
            }]
        }));

        assert!(!is_drifting_now(&tf));
    }

    #[test]
    fn drifting_filter_returns_only_currently_drifting_terraforms() {
        let drifting = terraform_with_name_and_status(
            "drifting",
            serde_json::json!({
                "conditions": [{
                    "type": "Ready",
                    "status": "False",
                    "reason": "DriftDetected",
                    "message": "drift detected",
                    "lastTransitionTime": "2026-08-24T12:00:00Z"
                }]
            }),
        );
        let drift_seen = terraform_with_name_and_status(
            "drift-seen",
            serde_json::json!({
                "lastDriftDetectedAt": "2026-08-24T12:00:00Z"
            }),
        );
        let ordinary = terraform_with_name_and_status(
            "ordinary",
            serde_json::json!({
                "conditions": [{
                    "type": "Ready",
                    "status": "True",
                    "reason": "ReconciliationSucceeded",
                    "message": "ready",
                    "lastTransitionTime": "2026-08-24T12:00:00Z"
                }]
            }),
        );
        let (store, mut writer) = crate::k8s::watcher::create_tf_store();
        for tf in [drifting, drift_seen, ordinary] {
            writer.apply_watcher_event(&kube::runtime::watcher::Event::Apply(tf));
        }

        let filtered = get_filtered_terraforms(
            &store,
            &None,
            "",
            false,
            false,
            false,
            true,
            false,
            &std::collections::HashMap::new(),
            SortColumn::Name,
            false,
        );

        assert_eq!(
            filtered
                .iter()
                .map(|tf| tf.metadata.name.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("drifting")]
        );
    }
}
