use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use crate::state::store::{AppState, FlashKind, InputMode, TabKind, ViewState};
use crate::ui::theme;

fn is_viewer(view: &ViewState) -> bool {
    matches!(
        view,
        ViewState::PlanViewer { .. }
            | ViewState::JsonViewer { .. }
            | ViewState::EventsViewer { .. }
            | ViewState::OutputsViewer { .. }
            | ViewState::ConditionsViewer { .. }
            | ViewState::LogViewer { .. }
    )
}

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let bg = Block::default().style(Style::default().bg(theme::STATUS_BAR_BG));
    f.render_widget(bg, area);

    // Status bar is two rows. Single-line transient modes (Search, Confirm,
    // flash) render on row 1 and leave row 2 blank.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    match &state.input_mode {
        InputMode::Search => {
            let line = Line::from(vec![
                Span::styled(" / ", theme::STATUS_BAR_KEY),
                Span::styled(
                    state.search_query.clone(),
                    Style::default().fg(Color::White).bg(theme::STATUS_BAR_BG),
                ),
                Span::styled(
                    "_",
                    Style::default()
                        .fg(Color::Rgb(140, 200, 255))
                        .bg(theme::STATUS_BAR_BG),
                ),
            ]);
            f.render_widget(Paragraph::new(line), rows[0]);
        }
        InputMode::Confirm => {
            if let Some(dialog) = &state.pending_dialog {
                let line = Line::from(vec![
                    Span::styled(
                        format!(" {} ", &dialog.message),
                        Style::default()
                            .fg(Color::Rgb(240, 200, 60))
                            .bg(theme::STATUS_BAR_BG),
                    ),
                    Span::styled(" [y]es  [n]o ", theme::STATUS_BAR_KEY),
                ]);
                f.render_widget(Paragraph::new(line), rows[0]);
            }
        }
        InputMode::Help => {
            let line = Line::from(vec![
                Span::styled(" ? ", theme::STATUS_BAR_KEY),
                Span::styled("or ", theme::STATUS_BAR_TEXT),
                Span::styled("Esc", theme::STATUS_BAR_KEY),
                Span::styled(" to close help", theme::STATUS_BAR_TEXT),
            ]);
            f.render_widget(Paragraph::new(line), rows[0]);
        }
        InputMode::ViewerSearch => {
            let match_info = if state.viewer_search_matches.is_empty() {
                if state.viewer_search_query.is_empty() {
                    String::new()
                } else {
                    " (no matches)".to_string()
                }
            } else {
                format!(
                    " ({}/{})",
                    state.viewer_search_index + 1,
                    state.viewer_search_matches.len()
                )
            };
            let line = Line::from(vec![
                Span::styled(" / ", theme::STATUS_BAR_KEY),
                Span::styled(
                    state.viewer_search_query.clone(),
                    Style::default().fg(Color::White).bg(theme::STATUS_BAR_BG),
                ),
                Span::styled(
                    "_",
                    Style::default()
                        .fg(Color::Rgb(140, 200, 255))
                        .bg(theme::STATUS_BAR_BG),
                ),
                Span::styled(
                    match_info,
                    Style::default()
                        .fg(Color::Rgb(140, 145, 165))
                        .bg(theme::STATUS_BAR_BG),
                ),
            ]);
            f.render_widget(Paragraph::new(line), rows[0]);
        }
        InputMode::NamespacePicker | InputMode::ContextPicker | InputMode::ShortcutsPopup => {
            let line = Line::from(vec![
                Span::styled(" j/k", theme::STATUS_BAR_KEY),
                Span::styled(":nav ", theme::STATUS_BAR_TEXT),
                Span::styled("Enter", theme::STATUS_BAR_KEY),
                Span::styled(":select ", theme::STATUS_BAR_TEXT),
                Span::styled("Esc", theme::STATUS_BAR_KEY),
                Span::styled(":cancel", theme::STATUS_BAR_TEXT),
            ]);
            f.render_widget(Paragraph::new(line), rows[0]);
        }
        InputMode::Normal => {
            if let Some((msg, _, kind)) = &state.flash_message {
                let style = match kind {
                    FlashKind::Success => theme::FLASH_SUCCESS,
                    FlashKind::Error => theme::FLASH_ERROR,
                };
                let line = Line::from(vec![Span::styled(
                    format!(" {msg} "),
                    style.bg(theme::STATUS_BAR_BG),
                )]);
                f.render_widget(Paragraph::new(line), rows[0]);
                // Keep a background-error indicator visible even under a flash.
                if let Some(err) = bg_error_line(state) {
                    f.render_widget(Paragraph::new(err), rows[1]);
                }
            } else {
                let (top, mut bottom) = build_help_lines(state);
                // Prepend a ⚠ background-error segment so recurring poller
                // failures (e.g. runner listing) are never silent.
                if let Some(mut err) = bg_error_line(state).map(|l| l.spans) {
                    if !bottom.is_empty() {
                        err.push(Span::styled(" │ ", theme::STATUS_BAR_SEP));
                    }
                    err.extend(bottom);
                    bottom = err;
                }
                f.render_widget(Paragraph::new(Line::from(top)), rows[0]);
                f.render_widget(Paragraph::new(Line::from(bottom)), rows[1]);
            }
        }
    }
}

/// A ⚠ status-bar segment for the current background-poller error, if any.
fn bg_error_line(state: &AppState) -> Option<Line<'static>> {
    let msg = state.background_error.as_ref()?;
    Some(Line::from(vec![
        Span::styled(" ⚠ ", theme::FLASH_ERROR.bg(theme::STATUS_BAR_BG)),
        Span::styled(
            msg.clone(),
            Style::default()
                .fg(Color::Rgb(240, 150, 150))
                .bg(theme::STATUS_BAR_BG),
        ),
    ]))
}

/// A logical group of keybinds. Rendered as `key:label key:label ...`
/// separated from neighboring groups by a dim ` │ `.
struct Group<'a>(&'a [(&'a str, &'a str)]);

fn render_groups(groups: &[Group<'_>], disabled: &[&str]) -> Vec<Span<'static>> {
    let k = theme::STATUS_BAR_KEY;
    let t = theme::STATUS_BAR_TEXT;
    let s = theme::STATUS_BAR_SEP;
    // Disabled keys (e.g. R/x when tfctl is missing) render in the same
    // muted gray as the group separator so they read as inert.
    let disabled_style = s;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut first = true;
    for group in groups {
        if group.0.is_empty() {
            continue;
        }
        if first {
            spans.push(Span::styled(" ", t));
            first = false;
        } else {
            spans.push(Span::styled(" │ ", s));
        }
        for (i, (key, label)) in group.0.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" ", t));
            }
            let is_disabled = disabled.contains(key);
            let key_style = if is_disabled { disabled_style } else { k };
            let label_style = if is_disabled { disabled_style } else { t };
            spans.push(Span::styled(key.to_string(), key_style));
            spans.push(Span::styled(format!(":{label}"), label_style));
        }
    }
    spans
}

/// Build the two-row status bar. Top row is the primary action shortcuts
/// for the current view; bottom row is meta (sort/filter/help/configured
/// shortcuts/mode indicators).
fn build_help_lines(state: &AppState) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    // R (replan) and x (btg) shell out to tfctl; render them disabled
    // when tfctl isn't on PATH so the user sees they're inert.
    let tfctl_disabled: &[&str] = if state.tfctl_available {
        &[]
    } else {
        &["R", "x"]
    };
    let top = match state.current_view() {
        ViewState::List(TabKind::Controller) => {
            let metrics_label: &'static str = if state.metrics_enabled {
                "metrics off"
            } else {
                "metrics"
            };
            render_groups(
                &[
                    Group(&[("j/k", "nav backlog"), ("Enter", "filter ns")]),
                    Group(&[("L", "controller logs"), ("M", metrics_label)]),
                    Group(&[("Tab", "next tab")]),
                ],
                &[],
            )
        }
        ViewState::List(TabKind::Terraform) | ViewState::List(TabKind::CustomTab(_)) => {
            render_groups(
                &[
                    Group(&[("j/k", "nav"), ("Enter", "detail")]),
                    Group(&[("a", "approve"), ("r", "reconcile"), ("R", "replan")]),
                    Group(&[("s/u", "suspend/resume"), ("p", "plan"), ("F", "unlock")]),
                    Group(&[("d", "delete"), ("x", "btg"), ("C", "clear BTG")]),
                ],
                tfctl_disabled,
            )
        }
        ViewState::List(TabKind::Kustomizations) => render_groups(
            &[
                Group(&[("j/k", "nav"), ("Enter", "detail")]),
                Group(&[("r", "reconcile"), ("s/u", "suspend/resume")]),
            ],
            &[],
        ),
        ViewState::List(TabKind::Runners) => render_groups(
            &[
                Group(&[("j/k", "nav"), ("Enter", "logs")]),
                Group(&[("T", "terraform"), ("e", "events")]),
                Group(&[("d", "kill")]),
            ],
            &[],
        ),
        ViewState::TerraformDetail { .. } => render_groups(
            &[
                Group(&[("Esc", "back")]),
                Group(&[
                    ("r", "reconcile"),
                    ("R", "replan"),
                    ("s/u", "suspend/resume"),
                ]),
                Group(&[("a", "approve"), ("p", "plan"), ("F", "unlock")]),
                Group(&[("x", "btg"), ("C", "clear BTG"), ("d", "delete")]),
            ],
            tfctl_disabled,
        ),
        ViewState::KustomizationDetail { .. } => render_groups(
            &[
                Group(&[("Esc", "back")]),
                Group(&[("r", "reconcile"), ("s/u", "suspend/resume")]),
            ],
            &[],
        ),
        ViewState::PlanViewer { .. }
        | ViewState::JsonViewer { .. }
        | ViewState::EventsViewer { .. }
        | ViewState::OutputsViewer { .. }
        | ViewState::ConditionsViewer { .. } => {
            let wrap_label: &'static str = if state.viewer_wrap { "nowrap" } else { "wrap" };
            render_groups(
                &[
                    Group(&[("Esc", "back")]),
                    Group(&[
                        ("j/k", "scroll"),
                        ("Ctrl-f/b", "screen"),
                        ("h/l", "hscroll"),
                        ("g/G", "top/bottom"),
                    ]),
                    Group(&[("/", "search"), ("n/N", "next/prev")]),
                    Group(&[("w", wrap_label), ("S", "save")]),
                ],
                &[],
            )
        }
        ViewState::LogViewer { .. } => {
            let wrap_label: &'static str = if state.viewer_wrap { "nowrap" } else { "wrap" };
            render_groups(
                &[
                    Group(&[("Esc", "back")]),
                    Group(&[("j/k", "scroll"), ("Ctrl-f/b", "screen"), ("G", "follow")]),
                    Group(&[("/", "search"), ("n/N", "next/prev")]),
                    Group(&[("w", wrap_label), ("S", "save"), ("Tab", "container")]),
                ],
                &[],
            )
        }
    };

    let bottom = build_meta_line(state);
    (top, bottom)
}

/// Bottom row: inspect actions for detail views, and meta indicators
/// (filter, sort, help, configured shortcuts, mode flags) everywhere.
fn build_meta_line(state: &AppState) -> Vec<Span<'static>> {
    let k = theme::STATUS_BAR_KEY;
    let t = theme::STATUS_BAR_TEXT;
    let s = theme::STATUS_BAR_SEP;
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Inspect group — actions that open a viewer over the current resource.
    let inspect: Vec<(&'static str, &'static str)> = match state.current_view() {
        ViewState::List(TabKind::Terraform)
        | ViewState::List(TabKind::CustomTab(_))
        | ViewState::TerraformDetail { .. } => vec![
            ("y/Y", "json/yaml"),
            ("e", "events"),
            ("c", "conditions"),
            ("O", "outputs"),
            ("L", "runner logs"),
        ],
        ViewState::List(TabKind::Kustomizations) | ViewState::KustomizationDetail { .. } => {
            vec![("y/Y", "json/yaml"), ("e", "events"), ("c", "conditions")]
        }
        _ => Vec::new(),
    };
    if !inspect.is_empty() {
        spans.push(Span::styled(" ", t));
        for (i, (key, label)) in inspect.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" ", t));
            }
            spans.push(Span::styled(*key, k));
            spans.push(Span::styled(format!(":{label}"), t));
        }
    }

    // Filter / sort / namespace — list views.
    let mut meta: Vec<Span<'static>> = Vec::new();
    let on_sortable_list = matches!(
        state.current_view(),
        ViewState::List(TabKind::Terraform)
            | ViewState::List(TabKind::Kustomizations)
            | ViewState::List(TabKind::CustomTab(_))
    );
    let on_runners_list = matches!(state.current_view(), ViewState::List(TabKind::Runners));
    if on_sortable_list || on_runners_list {
        meta.push(Span::styled(" n", k));
        meta.push(Span::styled(":ns ", t));
        let arrow = if state.sort_descending { "▼" } else { "▲" };
        if on_sortable_list {
            let sort_text = format!("sort:{}{}", state.sort_column.label(), arrow);
            meta.push(Span::styled("o", k));
            meta.push(Span::styled(format!(":{sort_text} "), t));
        } else if on_runners_list {
            let sort_text = format!("sort:{}{}", state.runner_sort_column.label(), arrow);
            meta.push(Span::styled("o", k));
            meta.push(Span::styled(format!(":{sort_text} "), t));
        }
        meta.push(Span::styled("/", k));
        meta.push(Span::styled(":search", t));
    }
    if !meta.is_empty() {
        if !spans.is_empty() {
            spans.push(Span::styled(" │ ", s));
        } else {
            spans.push(Span::styled(" ", t));
        }
        spans.extend(meta);
    }

    // Pause/resume filter when an active query exists.
    if !state.search_query.is_empty() && !is_viewer(state.current_view()) {
        let label = if state.search_suspended {
            ":resume filter"
        } else {
            ":pause filter"
        };
        if spans.is_empty() {
            spans.push(Span::styled(" ", t));
        } else {
            spans.push(Span::styled(" │ ", s));
        }
        spans.push(Span::styled("\\", k));
        spans.push(Span::styled(label, t));
    }

    // Bulk-selection count, when non-empty. Shown only on the lists
    // where selection is meaningful (TF, KS, custom tabs).
    let on_bulk_list = matches!(
        state.current_view(),
        ViewState::List(TabKind::Terraform)
            | ViewState::List(TabKind::Kustomizations)
            | ViewState::List(TabKind::CustomTab(_))
    );
    if on_bulk_list && !state.bulk_selected.is_empty() {
        if spans.is_empty() {
            spans.push(Span::styled(" ", t));
        } else {
            spans.push(Span::styled(" │ ", s));
        }
        let count = state.bulk_selected.len();
        spans.push(Span::styled("●", theme::BULK_SELECTED));
        spans.push(Span::styled(format!(" {count} selected"), t));
    }

    // Configured shortcuts roll up under one S:shortcuts hint — the
    // popup (opened with S) shows the full list with resolved URLs.
    let is_tf_view = matches!(
        state.current_view(),
        ViewState::List(TabKind::Terraform)
            | ViewState::List(TabKind::CustomTab(_))
            | ViewState::TerraformDetail { .. }
    );
    if is_tf_view && !state.config.shortcuts.is_empty() {
        if spans.is_empty() {
            spans.push(Span::styled(" ", t));
        } else {
            spans.push(Span::styled(" │ ", s));
        }
        spans.push(Span::styled("S", k));
        spans.push(Span::styled(":shortcuts", t));
    }

    // Context switcher + help are always available outside viewers.
    if !is_viewer(state.current_view()) {
        if spans.is_empty() {
            spans.push(Span::styled(" ", t));
        } else {
            spans.push(Span::styled(" │ ", s));
        }
        spans.push(Span::styled("Ctrl-x", k));
        spans.push(Span::styled(":ctx ", t));
        spans.push(Span::styled("?", k));
        spans.push(Span::styled(":help", t));
    }

    // Mouse mode indicator.
    if state.mouse_enabled {
        spans.push(Span::styled(
            "  [MOUSE]",
            Style::default()
                .fg(Color::Rgb(80, 200, 120))
                .bg(theme::STATUS_BAR_BG),
        ));
    }

    spans
}
