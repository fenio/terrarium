use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use ratatui::{
    layout::{Constraint, Rect},
    style::Style,
    text::Span,
    widgets::{Cell, Row, Table},
    Frame,
};

use crate::ui::resource_list::sort_cell;

use crate::k8s::kustomization::Kustomization;
use crate::k8s::watcher::KsStore;
use crate::state::store::{AppState, SortColumn};
use crate::ui::theme;
use crate::util;

pub fn render_kustomization_list(f: &mut Frame, area: Rect, state: &mut AppState) {
    let items = get_filtered_kustomizations(
        &state.ks_store,
        &state.namespace_filter,
        &state.search_query,
        state.show_failures_only,
        state.show_waiting_only,
        state.sort_column,
        state.sort_descending,
    );

    let active = state.sort_column;
    let desc = state.sort_descending;
    let header = Row::new(vec![
        sort_cell(SortColumn::Namespace, "NAMESPACE", active, desc),
        sort_cell(SortColumn::Name, "NAME", active, desc),
        sort_cell(SortColumn::Ready, "READY", active, desc),
        Cell::from("S"),
        Cell::from("SOURCE"),
        Cell::from("REVISION"),
        sort_cell(SortColumn::LastApplied, "LAST APPLIED", active, desc),
        sort_cell(SortColumn::Age, "AGE", active, desc),
    ])
    .style(theme::COLUMN_HEADER)
    .bottom_margin(1);

    let rows: Vec<Row> = items
        .iter()
        .map(|ks| {
            let ns = ks.metadata.namespace.as_deref().unwrap_or("-");
            let name = ks.metadata.name.as_deref().unwrap_or("-");
            let (ready_text, ready_style) = get_ready_status(ks);
            let suspended_cell = if ks.spec.suspend.unwrap_or(false) {
                Cell::from(Span::styled("S", theme::SUSPENDED))
            } else {
                Cell::from(" ")
            };
            let source = format!(
                "{:?}/{}",
                ks.spec.source_ref.kind, ks.spec.source_ref.name
            );
            let revision = ks
                .status
                .as_ref()
                .and_then(|s| s.last_applied_revision.as_deref())
                .map(truncate_revision)
                .unwrap_or_else(|| "-".to_string());
            let last_applied = get_last_applied_time(ks);
            let age = get_age(ks);

            Row::new(vec![
                Cell::from(ns.to_string()),
                Cell::from(name.to_string()),
                Cell::from(Span::styled(ready_text, ready_style)),
                suspended_cell,
                Cell::from(source),
                Cell::from(revision),
                Cell::from(last_applied),
                Cell::from(age),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(12),
        Constraint::Percentage(20),
        Constraint::Percentage(8),
        Constraint::Length(2),
        Constraint::Percentage(15),
        Constraint::Percentage(13),
        Constraint::Percentage(16),
        Constraint::Percentage(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme::SELECTED_ROW)
;

    f.render_stateful_widget(table, area, &mut state.ks_table_state);
}

pub fn get_filtered_kustomizations(
    store: &KsStore,
    namespace_filter: &Option<String>,
    search_query: &str,
    failures_only: bool,
    _waiting_only: bool,
    sort_column: SortColumn,
    descending: bool,
) -> Vec<Kustomization> {
    let all: Vec<Kustomization> = store.state().iter().map(|arc| (**arc).clone()).collect();
    let mut filtered: Vec<Kustomization> = all
        .into_iter()
        .filter(|ks| {
            if let Some(ns) = namespace_filter {
                ks.metadata.namespace.as_deref() == Some(ns.as_str())
            } else {
                true
            }
        })
        .filter(|ks| {
            if search_query.is_empty() {
                true
            } else {
                let name = ks.metadata.name.as_deref().unwrap_or("");
                let ns = ks.metadata.namespace.as_deref().unwrap_or("");
                name.contains(search_query) || ns.contains(search_query)
            }
        })
        .filter(|ks| {
            if !failures_only {
                return true;
            }
            let is_ready = ks
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                .map(|c| c.status == "True")
                .unwrap_or(false);
            !is_ready
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
            // Ascending = oldest first; never-applied resources sort last.
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

fn applied_ts(ks: &Kustomization) -> Option<jiff::Timestamp> {
    ks.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready" && c.status == "True"))
        .map(|c| c.last_transition_time.0)
}

fn get_ready_str(ks: &Kustomization) -> String {
    ks.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
        .map(|c| c.status.clone())
        .unwrap_or_default()
}

fn get_ready_status(ks: &Kustomization) -> (String, Style) {
    let conditions = ks.status.as_ref().and_then(|s| s.conditions.as_ref());

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

fn get_age(ks: &Kustomization) -> String {
    ks.metadata
        .creation_timestamp
        .as_ref()
        .map(|ts| util::format_duration(util::secs_since(ts.0)))
        .unwrap_or_else(|| "-".to_string())
}

fn get_last_applied_time(ks: &Kustomization) -> String {
    ks.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready" && c.status == "True"))
        .map(|c| util::format_duration_ago(util::secs_since(c.last_transition_time.0)))
        .unwrap_or_else(|| "-".to_string())
}

fn find_condition<'a>(conditions: &'a [Condition], type_name: &str) -> Option<&'a Condition> {
    conditions.iter().find(|c| c.type_ == type_name)
}
