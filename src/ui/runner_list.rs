use k8s_openapi::api::core::v1::Pod;
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::Style,
    text::Span,
    widgets::{Cell, Row, Table},
};

use crate::state::store::{AppState, RunnerSortColumn};
use crate::ui::resource_list::sort_cell;
use crate::ui::theme;
use crate::util;

pub fn render_runner_list(f: &mut Frame, area: Rect, state: &mut AppState) {
    let items = get_filtered_runners(
        &state.runner_pods,
        &state.namespace_filter,
        state.effective_search_query(),
        state.runner_sort_column,
        state.sort_descending,
    );

    let active = state.runner_sort_column;
    let desc = state.sort_descending;
    let header = Row::new(vec![
        sort_cell("NAMESPACE", active == RunnerSortColumn::Namespace, desc),
        sort_cell("NAME", active == RunnerSortColumn::Name, desc),
        sort_cell("TERRAFORM", active == RunnerSortColumn::Terraform, desc),
        sort_cell("PHASE", active == RunnerSortColumn::Phase, desc),
        Cell::from("STATUS"),
        sort_cell("AGE", active == RunnerSortColumn::Age, desc),
    ])
    .style(theme::COLUMN_HEADER)
    .bottom_margin(1);

    let rows: Vec<Row> = items
        .iter()
        .map(|pod| {
            let ns = pod.metadata.namespace.as_deref().unwrap_or("-");
            let name = pod.metadata.name.as_deref().unwrap_or("-");
            let tf_name = name.strip_suffix("-tf-runner").unwrap_or("-");
            let phase = pod
                .status
                .as_ref()
                .and_then(|s| s.phase.as_deref())
                .unwrap_or("Unknown");
            let (status_text, status_style) = get_pod_status(pod);
            let age = get_pod_age(pod);

            Row::new(vec![
                Cell::from(ns.to_string()),
                Cell::from(name.to_string()),
                Cell::from(tf_name.to_string()),
                Cell::from(phase.to_string()),
                Cell::from(Span::styled(status_text, status_style)),
                Cell::from(age),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(15),
        Constraint::Percentage(30),
        Constraint::Percentage(20),
        Constraint::Percentage(10),
        Constraint::Percentage(15),
        Constraint::Percentage(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme::SELECTED_ROW)
        .block(crate::ui::detail::block("Runners"));

    f.render_stateful_widget(table, area, &mut state.runner_table_state);
}

pub fn get_filtered_runners<'a>(
    pods: &'a [Pod],
    namespace_filter: &Option<String>,
    search_query: &str,
    sort_column: RunnerSortColumn,
    descending: bool,
) -> Vec<&'a Pod> {
    let mut filtered: Vec<&'a Pod> = pods
        .iter()
        .filter(|pod| {
            if let Some(ns) = namespace_filter {
                pod.metadata.namespace.as_deref() == Some(ns.as_str())
            } else {
                true
            }
        })
        .filter(|pod| {
            if search_query.is_empty() {
                true
            } else {
                let name = pod.metadata.name.as_deref().unwrap_or("");
                let ns = pod.metadata.namespace.as_deref().unwrap_or("");
                let tf = pod
                    .metadata
                    .labels
                    .as_ref()
                    .and_then(|l| l.get("infra.contrib.fluxcd.io/terraform"))
                    .map(|s| s.as_str())
                    .unwrap_or("");
                name.contains(search_query)
                    || ns.contains(search_query)
                    || tf.contains(search_query)
            }
        })
        .collect();

    match sort_column {
        RunnerSortColumn::Namespace => filtered.sort_by(|a, b| {
            a.metadata
                .namespace
                .cmp(&b.metadata.namespace)
                .then(a.metadata.name.cmp(&b.metadata.name))
        }),
        RunnerSortColumn::Name => {
            filtered.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name))
        }
        RunnerSortColumn::Terraform => filtered.sort_by(|a, b| {
            tf_name(a)
                .cmp(&tf_name(b))
                .then(a.metadata.name.cmp(&b.metadata.name))
        }),
        RunnerSortColumn::Phase => filtered.sort_by(|a, b| {
            pod_phase(a)
                .cmp(&pod_phase(b))
                .then(a.metadata.name.cmp(&b.metadata.name))
        }),
        RunnerSortColumn::Age => filtered.sort_by(|a, b| {
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

fn tf_name(pod: &Pod) -> String {
    pod.metadata
        .name
        .as_deref()
        .and_then(|n| n.strip_suffix("-tf-runner"))
        .unwrap_or("")
        .to_string()
}

fn pod_phase(pod: &Pod) -> String {
    pod.status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("")
        .to_string()
}

fn get_pod_status(pod: &Pod) -> (String, Style) {
    let phase = pod
        .status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown");

    match phase {
        "Running" => ("Running".to_string(), theme::STATUS_READY),
        "Succeeded" => ("Succeeded".to_string(), theme::STATUS_READY),
        "Failed" => ("Failed".to_string(), theme::STATUS_NOT_READY),
        "Pending" => ("Pending".to_string(), theme::STATUS_PENDING),
        other => (other.to_string(), theme::STATUS_UNKNOWN),
    }
}

fn get_pod_age(pod: &Pod) -> String {
    pod.metadata
        .creation_timestamp
        .as_ref()
        .map(|ts| util::format_duration(util::secs_since(ts.0)))
        .unwrap_or_else(|| "-".to_string())
}
