use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Row, Table},
    Frame,
};

use crate::k8s::terraform::Terraform;
use crate::k8s::watcher::TfStore;
use crate::state::store::{AppState, SortColumn};
use crate::ui::theme;
use crate::util;

pub fn render_terraform_list(f: &mut Frame, area: Rect, state: &mut AppState) {
    let items = get_filtered_terraforms(&state.tf_store, &state.namespace_filter, state.effective_search_query(), state.show_failures_only, state.show_waiting_only, state.sort_column, state.sort_descending);

    let active = state.sort_column;
    let desc = state.sort_descending;
    let header = Row::new(vec![
        sort_cell(SortColumn::Namespace, "NAMESPACE", active, desc),
        sort_cell(SortColumn::Name, "NAME", active, desc),
        sort_cell(SortColumn::Ready, "READY", active, desc),
        Cell::from("S"),
        Cell::from("PLAN"),
        Cell::from("REVISION"),
        sort_cell(SortColumn::LastApplied, "LAST APPLIED", active, desc),
        sort_cell(SortColumn::Age, "AGE", active, desc),
    ])
    .style(theme::COLUMN_HEADER)
    .bottom_margin(1);

    let rows: Vec<Row> = items
        .iter()
        .map(|tf| {
            let ns = tf
                .metadata
                .namespace
                .as_deref()
                .unwrap_or("-");
            let name = tf
                .metadata
                .name
                .as_deref()
                .unwrap_or("-");

            let (ready_text, ready_style) = get_ready_status(tf);
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
;

    f.render_stateful_widget(table, area, &mut state.tf_table_state);
}

pub fn get_filtered_terraforms(
    store: &TfStore,
    namespace_filter: &Option<String>,
    search_query: &str,
    failures_only: bool,
    waiting_only: bool,
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
            if failures_only {
                let is_ready = tf
                    .status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                    .map(|c| c.status == "True")
                    .unwrap_or(false);
                return !is_ready;
            }
            if waiting_only {
                return is_waiting(tf);
            }
            true
        })
        .collect();

    match sort_column {
        SortColumn::Namespace => filtered.sort_by(|a, b| {
            a.metadata.namespace.cmp(&b.metadata.namespace)
                .then(a.metadata.name.cmp(&b.metadata.name))
        }),
        SortColumn::Name => filtered.sort_by(|a, b| {
            a.metadata.name.cmp(&b.metadata.name)
        }),
        SortColumn::Ready => filtered.sort_by(|a, b| {
            let ready_a = get_ready_str(a);
            let ready_b = get_ready_str(b);
            ready_a.cmp(&ready_b).then(a.metadata.name.cmp(&b.metadata.name))
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

/// Build a column header cell that highlights when it's the active sort.
/// Arrow indicates direction: ▲ ascending, ▼ descending.
pub(crate) fn sort_cell(
    col: SortColumn,
    label: &'static str,
    active: SortColumn,
    descending: bool,
) -> Cell<'static> {
    if col == active {
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

fn get_ready_str(tf: &Terraform) -> String {
    tf.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
        .map(|c| c.status.clone())
        .unwrap_or_default()
}

fn get_ready_status(tf: &Terraform) -> (String, Style) {
    let conditions = tf
        .status
        .as_ref()
        .and_then(|s| s.conditions.as_ref());

    if let Some(conditions) = conditions {
        if let Some(ready) = find_condition(conditions, "Ready") {
            match ready.status.as_str() {
                "True" => ("True".to_string(), theme::STATUS_READY),
                "False" => ("False".to_string(), theme::STATUS_NOT_READY),
                _ => ("Unknown".to_string(), theme::STATUS_UNKNOWN),
            }
        } else {
            ("Unknown".to_string(), theme::STATUS_UNKNOWN)
        }
    } else {
        ("-".to_string(), theme::STATUS_UNKNOWN)
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

fn find_condition<'a>(conditions: &'a [Condition], type_name: &str) -> Option<&'a Condition> {
    conditions.iter().find(|c| c.type_ == type_name)
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
    let is_ready = ready_condition
        .map(|c| c.status == "True")
        .unwrap_or(false);
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
