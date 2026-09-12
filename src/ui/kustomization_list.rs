use std::sync::Arc;

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::Style,
    text::Span,
    widgets::{Cell, Row, Table},
};

use crate::ui::resource_list::{bulk_marker_cell, sort_cell};

use crate::k8s::kustomization::Kustomization;
use crate::k8s::watcher::KsStore;
use crate::state::store::{AppState, SelectionIdentity, SortColumn, TabKind};
use crate::ui::theme;
use crate::util;

pub fn render_kustomization_list(f: &mut Frame, area: Rect, state: &mut AppState) {
    let items = get_filtered_kustomizations(
        &state.ks_store,
        &state.namespace_filter,
        state.effective_search_query(),
        state.show_failures_only,
        state.show_waiting_only,
        state.show_progressing_only,
        state.show_deleting_only,
        &state.recently_acted,
        state.sort_column,
        state.sort_descending,
    );
    let identities: Vec<SelectionIdentity> = items
        .iter()
        .map(|ks| {
            SelectionIdentity::new(
                ks.metadata.namespace.clone().unwrap_or_default(),
                ks.metadata.name.clone().unwrap_or_default(),
                ks.metadata.uid.clone(),
            )
        })
        .collect();
    state.reconcile_selection_for(&TabKind::Kustomizations, &identities);
    state.reconcile_bulk_selection_for(&TabKind::Kustomizations, &identities);

    let active = state.sort_column;
    let desc = state.sort_descending;
    let header = Row::new(vec![
        Cell::from(" "),
        sort_cell("NAMESPACE", active == SortColumn::Namespace, desc),
        sort_cell("NAME", active == SortColumn::Name, desc),
        sort_cell("READY", active == SortColumn::Ready, desc),
        Cell::from("S"),
        Cell::from("SOURCE"),
        sort_cell("REVISION", active == SortColumn::Revision, desc),
        sort_cell("LAST APPLIED", active == SortColumn::LastApplied, desc),
        sort_cell("AGE", active == SortColumn::Age, desc),
    ])
    .style(theme::COLUMN_HEADER)
    .bottom_margin(1);

    let rows: Vec<Row> = items
        .iter()
        .map(|ks| {
            let ns = ks.metadata.namespace.as_deref().unwrap_or("-");
            let name = ks.metadata.name.as_deref().unwrap_or("-");
            let (ready_text, ready_style) = get_ready_status(ks);
            let deleting = ks.metadata.deletion_timestamp.is_some();
            let bulk_cell = bulk_marker_cell(state, ns, name, ks.metadata.uid.as_deref(), deleting);
            let suspended_cell = if ks.spec.suspend.unwrap_or(false) {
                Cell::from(Span::styled("S", theme::SUSPENDED))
            } else {
                Cell::from(" ")
            };
            let source = format!("{:?}/{}", ks.spec.source_ref.kind, ks.spec.source_ref.name);
            let revision = ks
                .status
                .as_ref()
                .and_then(|s| s.last_applied_revision.as_deref())
                .map(truncate_revision)
                .unwrap_or_else(|| "-".to_string());
            let last_applied = get_last_applied_time(ks);
            let age = get_age(ks);

            Row::new(vec![
                bulk_cell,
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
        Constraint::Length(2),
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
        .block(crate::ui::detail::block("Kustomizations"));

    f.render_stateful_widget(table, area, &mut state.ks_table_state);
}

#[allow(clippy::too_many_arguments)]
pub fn get_filtered_kustomizations(
    store: &KsStore,
    namespace_filter: &Option<String>,
    search_query: &str,
    failures_only: bool,
    _waiting_only: bool,
    progressing_only: bool,
    deleting_only: bool,
    recently_acted: &std::collections::HashMap<(String, String), std::time::Instant>,
    sort_column: SortColumn,
    descending: bool,
) -> Vec<Arc<Kustomization>> {
    let mut filtered: Vec<Arc<Kustomization>> = store
        .state()
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
            let ns = ks.metadata.namespace.as_deref().unwrap_or("");
            let name = ks.metadata.name.as_deref().unwrap_or("");
            let in_grace = recently_acted.contains_key(&(ns.to_string(), name.to_string()));

            if failures_only {
                return in_grace
                    || util::classify_ready(
                        ks.status.as_ref().and_then(|s| s.conditions.as_ref()),
                    )
                    .is_real_failure();
            }
            if progressing_only {
                return in_grace
                    || matches!(
                        util::classify_ready(
                            ks.status.as_ref().and_then(|s| s.conditions.as_ref()),
                        ),
                        util::ReadyState::Reconciling
                    );
            }
            if deleting_only {
                return in_grace || ks.metadata.deletion_timestamp.is_some();
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
                .cmp(ready_b)
                .then(a.metadata.name.cmp(&b.metadata.name))
        }),
        SortColumn::Revision => filtered.sort_by(|a, b| {
            // Ascending = lexicographic by last applied revision; resources
            // without a revision sort last so they don't pile up at the top.
            let rev_a = revision_str(a);
            let rev_b = revision_str(b);
            match (rev_a, rev_b) {
                (Some(x), Some(y)) => x.cmp(y).then(a.metadata.name.cmp(&b.metadata.name)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.metadata.name.cmp(&b.metadata.name),
            }
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

fn revision_str(ks: &Kustomization) -> Option<&str> {
    ks.status
        .as_ref()
        .and_then(|s| s.last_applied_revision.as_deref())
}

fn get_ready_str(ks: &Kustomization) -> &str {
    ks.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
        .map(|c| c.status.as_str())
        .unwrap_or_default()
}

fn get_ready_status(ks: &Kustomization) -> (String, Style) {
    let conditions = ks.status.as_ref().and_then(|s| s.conditions.as_ref());
    crate::ui::resource_list::ready_label_and_style(util::classify_ready(conditions))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_kustomizations_reuse_store_arcs() {
        let ks: Kustomization = serde_json::from_value(serde_json::json!({
            "apiVersion": "kustomize.toolkit.fluxcd.io/v1",
            "kind": "Kustomization",
            "metadata": {"name": "shared", "namespace": "ns"},
            "spec": {
                "interval": "1m",
                "prune": false,
                "sourceRef": {"kind": "GitRepository", "name": "source"}
            }
        }))
        .expect("minimal Kustomization should deserialize");
        let (store, mut writer) = crate::k8s::watcher::create_ks_store();
        writer.apply_watcher_event(&kube::runtime::watcher::Event::Apply(ks));
        let stored = store
            .get(&kube::runtime::reflector::ObjectRef::new("shared").within("ns"))
            .expect("Kustomization should be present");

        let filtered = get_filtered_kustomizations(
            &store,
            &None,
            "",
            false,
            false,
            false,
            false,
            &std::collections::HashMap::new(),
            SortColumn::Name,
            false,
        );

        assert_eq!(filtered.len(), 1);
        assert!(Arc::ptr_eq(&stored, &filtered[0]));
    }
}
