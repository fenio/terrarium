use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Cell, Row, Table},
    Frame,
};

use crate::config::{CustomColumn, CustomColumnSource, CustomTab};
use crate::k8s::terraform::Terraform;
use crate::k8s::watcher::TfStore;
use crate::state::store::AppState;
use crate::ui::theme;
use crate::util;

/// A single row produced from a Terraform resource + annotation.
pub struct CustomTabEntry {
    pub namespace: String,
    pub name: String,
    pub annotation_key: String,
    pub annotation_value: String,
    pub ready: String,
    pub ready_style: Style,
    pub age: String,
}

pub fn render_custom_tab(f: &mut Frame, area: Rect, state: &mut AppState, tab_idx: usize) {
    let tab_config = &state.config.custom_tabs[tab_idx];
    let items = get_filtered_entries(
        &state.tf_store,
        &state.namespace_filter,
        state.effective_search_query(),
        tab_config,
    );

    let header_cells: Vec<Cell> = tab_config
        .columns
        .iter()
        .map(|col| Cell::from(col.label.as_str()))
        .collect();
    let header = Row::new(header_cells)
        .style(theme::COLUMN_HEADER)
        .bottom_margin(1);

    let rows: Vec<Row> = items
        .iter()
        .map(|entry| {
            let cells: Vec<Cell> = tab_config
                .columns
                .iter()
                .map(|col| render_cell(entry, col))
                .collect();
            Row::new(cells)
        })
        .collect();

    let widths: Vec<Constraint> = tab_config
        .columns
        .iter()
        .map(|col| Constraint::Percentage(col.width))
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme::SELECTED_ROW);

    f.render_stateful_widget(table, area, &mut state.custom_tab_states[tab_idx]);
}

fn render_cell<'a>(entry: &'a CustomTabEntry, col: &CustomColumn) -> Cell<'a> {
    let value = match &col.source {
        CustomColumnSource::Namespace => &entry.namespace,
        CustomColumnSource::Name => &entry.name,
        CustomColumnSource::Ready => &entry.ready,
        CustomColumnSource::Age => &entry.age,
        CustomColumnSource::AnnotationKey => &entry.annotation_key,
        CustomColumnSource::AnnotationValue => &entry.annotation_value,
    };

    if col.date_highlight {
        return Cell::from(Span::styled(value.clone(), classify_date_style(value)));
    }

    if matches!(col.source, CustomColumnSource::Ready) {
        return Cell::from(Span::styled(value.clone(), entry.ready_style));
    }

    let mut style = Style::default();
    if let Some([r, g, b]) = col.color {
        style = style.fg(Color::Rgb(r, g, b));
    }
    if col.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if style == Style::default() {
        Cell::from(value.as_str())
    } else {
        Cell::from(Span::styled(value.clone(), style))
    }
}

pub fn get_filtered_entries(
    store: &TfStore,
    namespace_filter: &Option<String>,
    search_query: &str,
    tab: &CustomTab,
) -> Vec<CustomTabEntry> {
    let all: Vec<Terraform> = store.state().iter().map(|arc| (**arc).clone()).collect();
    let mut entries = Vec::new();

    for tf in all {
        let annotation_value = tf
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(&tab.annotation))
            .cloned();

        let annotation_value = match annotation_value {
            Some(v) if !v.is_empty() && v != "{}" => v,
            _ => continue,
        };

        let ns = tf.metadata.namespace.clone().unwrap_or_default();
        let name = tf.metadata.name.clone().unwrap_or_default();

        if let Some(filter_ns) = namespace_filter
            && ns != *filter_ns
        {
            continue;
        }
        if !search_query.is_empty()
            && !name.contains(search_query)
            && !ns.contains(search_query)
            && !annotation_value.contains(search_query)
        {
            continue;
        }

        let (ready, ready_style) = get_ready_status(&tf);
        let age = tf
            .metadata
            .creation_timestamp
            .as_ref()
            .map(|ts| util::format_duration(util::secs_since(ts.0)))
            .unwrap_or_else(|| "-".to_string());

        if tab.expand_json_map {
            if let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                &annotation_value,
            ) {
                for (key, val) in &map {
                    let val_str = match val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    entries.push(CustomTabEntry {
                        namespace: ns.clone(),
                        name: name.clone(),
                        annotation_key: key.clone(),
                        annotation_value: val_str,
                        ready: ready.clone(),
                        ready_style,
                        age: age.clone(),
                    });
                }
            } else {
                entries.push(CustomTabEntry {
                    namespace: ns.clone(),
                    name: name.clone(),
                    annotation_key: "(raw)".to_string(),
                    annotation_value: annotation_value.clone(),
                    ready: ready.clone(),
                    ready_style,
                    age: age.clone(),
                });
            }
        } else {
            entries.push(CustomTabEntry {
                namespace: ns.clone(),
                name: name.clone(),
                annotation_key: String::new(),
                annotation_value: annotation_value.clone(),
                ready: ready.clone(),
                ready_style,
                age: age.clone(),
            });
        }
    }

    // Sort by the configured sort column
    if let Some(sort_col) = &tab.sort_by {
        let sort_lower = sort_col.to_lowercase();
        if let Some(col) = tab.columns.iter().find(|c| c.label.to_lowercase() == sort_lower) {
            entries.sort_by(|a, b| {
                let va = entry_value(a, &col.source);
                let vb = entry_value(b, &col.source);
                va.cmp(&vb).then(a.name.cmp(&b.name))
            });
        }
    }

    entries
}

fn entry_value<'a>(entry: &'a CustomTabEntry, source: &CustomColumnSource) -> &'a str {
    match source {
        CustomColumnSource::Namespace => &entry.namespace,
        CustomColumnSource::Name => &entry.name,
        CustomColumnSource::Ready => &entry.ready,
        CustomColumnSource::Age => &entry.age,
        CustomColumnSource::AnnotationKey => &entry.annotation_key,
        CustomColumnSource::AnnotationValue => &entry.annotation_value,
    }
}

/// Count how many TF resources match the custom tab annotation filter (for tab badge).
pub fn count_entries(store: &TfStore, tab: &CustomTab) -> usize {
    store
        .state()
        .iter()
        .filter(|tf| {
            tf.metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(&tab.annotation))
                .map(|v| !v.is_empty() && v != "{}")
                .unwrap_or(false)
        })
        .count()
}

fn get_ready_status(tf: &Terraform) -> (String, Style) {
    let conditions = tf.status.as_ref().and_then(|s| s.conditions.as_ref());
    if let Some(conditions) = conditions {
        if let Some(ready) = conditions.iter().find(|c| c.type_ == "Ready") {
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

/// Color the date based on proximity: past = red, soon = yellow, future = green.
fn classify_date_style(date_str: &str) -> Style {
    if let Ok(date) = jiff::civil::Date::strptime("%Y-%m-%d", date_str) {
        let today = jiff::Zoned::now().date();
        if date < today {
            Style::default()
                .fg(Color::Rgb(240, 80, 80))
                .add_modifier(Modifier::BOLD)
        } else if date <= today.checked_add(jiff::Span::new().days(7)).unwrap_or(today) {
            Style::default()
                .fg(Color::Rgb(240, 200, 60))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Rgb(80, 220, 100))
        }
    } else {
        Style::default().fg(Color::White)
    }
}
