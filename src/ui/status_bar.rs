use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::state::store::{AppState, FlashKind, InputMode, TabKind, ViewState};

fn is_viewer(view: &ViewState) -> bool {
    matches!(view, ViewState::PlanViewer { .. } | ViewState::JsonViewer { .. } | ViewState::EventsViewer { .. } | ViewState::OutputsViewer { .. } | ViewState::LogViewer { .. })
}
use crate::ui::theme;

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    // Fill background
    let bg = Block::default().style(Style::default().bg(theme::STATUS_BAR_BG));
    f.render_widget(bg, area);

    let spans = match &state.input_mode {
        InputMode::Search => {
            vec![
                Span::styled(" / ", theme::STATUS_BAR_KEY),
                Span::styled(&state.search_query, Style::default().fg(Color::White).bg(theme::STATUS_BAR_BG)),
                Span::styled("_", Style::default().fg(Color::Rgb(140, 200, 255)).bg(theme::STATUS_BAR_BG)),
            ]
        }
        InputMode::Confirm => {
            if let Some(dialog) = &state.pending_dialog {
                vec![
                    Span::styled(
                        format!(" {} ", &dialog.message),
                        Style::default().fg(Color::Rgb(240, 200, 60)).bg(theme::STATUS_BAR_BG),
                    ),
                    Span::styled(" [y]es  [n]o ", theme::STATUS_BAR_KEY),
                ]
            } else {
                vec![]
            }
        }
        InputMode::Help => {
            vec![
                Span::styled(" ? ", theme::STATUS_BAR_KEY),
                Span::styled("or ", theme::STATUS_BAR_TEXT),
                Span::styled("Esc", theme::STATUS_BAR_KEY),
                Span::styled(" to close help", theme::STATUS_BAR_TEXT),
            ]
        }
        InputMode::ViewerSearch => {
            let match_info = if state.viewer_search_matches.is_empty() {
                if state.viewer_search_query.is_empty() { String::new() } else { " (no matches)".to_string() }
            } else {
                format!(" ({}/{})", state.viewer_search_index + 1, state.viewer_search_matches.len())
            };
            vec![
                Span::styled(" / ", theme::STATUS_BAR_KEY),
                Span::styled(&state.viewer_search_query, Style::default().fg(Color::White).bg(theme::STATUS_BAR_BG)),
                Span::styled("_", Style::default().fg(Color::Rgb(140, 200, 255)).bg(theme::STATUS_BAR_BG)),
                Span::styled(match_info, Style::default().fg(Color::Rgb(140, 145, 165)).bg(theme::STATUS_BAR_BG)),
            ]
        }
        InputMode::NamespacePicker => {
            vec![
                Span::styled(" j/k", theme::STATUS_BAR_KEY),
                Span::styled(":nav ", theme::STATUS_BAR_TEXT),
                Span::styled("Enter", theme::STATUS_BAR_KEY),
                Span::styled(":select ", theme::STATUS_BAR_TEXT),
                Span::styled("Esc", theme::STATUS_BAR_KEY),
                Span::styled(":cancel", theme::STATUS_BAR_TEXT),
            ]
        }
        InputMode::Normal => {
            if let Some((msg, _, kind)) = &state.flash_message {
                let style = match kind {
                    FlashKind::Success => theme::FLASH_SUCCESS,
                    FlashKind::Error => theme::FLASH_ERROR,
                };
                vec![Span::styled(format!(" {} ", msg), style.bg(theme::STATUS_BAR_BG))]
            } else {
                build_help_spans(state)
            }
        }
    };

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn build_help_spans(state: &AppState) -> Vec<Span<'static>> {
    let k = theme::STATUS_BAR_KEY;
    let t = theme::STATUS_BAR_TEXT;
    let sort_label = format!("sort:{} ", state.sort_column.label());
    let mut spans = match state.current_view() {
        ViewState::List(TabKind::Controller) => vec![
            Span::styled(" j/k", k), Span::styled(":nav backlog ", t),
            Span::styled("Enter", k), Span::styled(":filter ns ", t),
            Span::styled("L", k), Span::styled(":controller logs ", t),
            Span::styled("Tab", k), Span::styled(":next tab ", t),
        ],
        ViewState::List(TabKind::Terraform) => vec![
            Span::styled(" j/k", k), Span::styled(":nav ", t),
            Span::styled("Enter", k), Span::styled(":detail ", t),
            Span::styled("a", k), Span::styled(":approve ", t),
            Span::styled("r", k), Span::styled(":reconcile ", t),
            Span::styled("p", k), Span::styled(":plan ", t),
            Span::styled("y", k), Span::styled(":json ", t),
            Span::styled("n", k), Span::styled(":ns ", t),
            Span::styled("o", k), Span::styled(":", t), Span::styled(sort_label, t),
            Span::styled("/", k), Span::styled(":search", t),
        ],
        ViewState::List(TabKind::Kustomizations) => vec![
            Span::styled(" j/k", k), Span::styled(":nav ", t),
            Span::styled("Enter", k), Span::styled(":detail ", t),
            Span::styled("r", k), Span::styled(":reconcile ", t),
            Span::styled("y", k), Span::styled(":json ", t),
            Span::styled("n", k), Span::styled(":ns ", t),
            Span::styled("o", k), Span::styled(":", t), Span::styled(sort_label, t),
            Span::styled("/", k), Span::styled(":search", t),
        ],
        ViewState::List(TabKind::CustomTab(_)) => vec![
            Span::styled(" j/k", k), Span::styled(":nav ", t),
            Span::styled("Enter", k), Span::styled(":detail ", t),
            Span::styled("r", k), Span::styled(":reconcile ", t),
            Span::styled("n", k), Span::styled(":ns ", t),
            Span::styled("/", k), Span::styled(":search", t),
        ],
        ViewState::List(TabKind::Runners) => vec![
            Span::styled(" j/k", k), Span::styled(":nav ", t),
            Span::styled("Enter", k), Span::styled(":logs ", t),
            Span::styled("d", k), Span::styled(":kill ", t),
            Span::styled("/", k), Span::styled(":search", t),
        ],
        ViewState::TerraformDetail { .. } => vec![
            Span::styled(" Esc", k), Span::styled(":back ", t),
            Span::styled("a", k), Span::styled(":approve ", t),
            Span::styled("r", k), Span::styled(":reconcile ", t),
            Span::styled("p", k), Span::styled(":plan ", t),
            Span::styled("O", k), Span::styled(":outputs ", t),
            Span::styled("e", k), Span::styled(":events ", t),
            Span::styled("x", k), Span::styled(":btg ", t),
            Span::styled("s", k), Span::styled(":suspend", t),
        ],
        ViewState::KustomizationDetail { .. } => vec![
            Span::styled(" Esc", k), Span::styled(":back ", t),
            Span::styled("r", k), Span::styled(":reconcile ", t),
            Span::styled("e", k), Span::styled(":events ", t),
            Span::styled("y", k), Span::styled(":json ", t),
            Span::styled("s", k), Span::styled(":suspend ", t),
            Span::styled("u", k), Span::styled(":resume", t),
        ],
        ViewState::PlanViewer { .. } | ViewState::JsonViewer { .. } | ViewState::EventsViewer { .. } | ViewState::OutputsViewer { .. } => {
            let wrap_label = if state.viewer_wrap { "nowrap" } else { "wrap" };
            vec![
                Span::styled(" Esc", k), Span::styled(":back ", t),
                Span::styled("j/k", k), Span::styled(":scroll ", t),
                Span::styled("h/l", k), Span::styled(":hscroll ", t),
                Span::styled("/", k), Span::styled(":search ", t),
                Span::styled("n/N", k), Span::styled(":next/prev ", t),
                Span::styled("w", k), Span::styled(format!(":{} ", wrap_label), t),
                Span::styled("S", k), Span::styled(":save", t),
            ]
        },
        ViewState::LogViewer { containers, .. } => {
            let wrap_label = if state.viewer_wrap { "nowrap" } else { "wrap" };
            let follow_indicator = if state.log_auto_follow { " [FOLLOW]" } else { "" };
            let mut v = vec![
                Span::styled(" Esc", k), Span::styled(":back ", t),
                Span::styled("j/k", k), Span::styled(":scroll ", t),
                Span::styled("G", k), Span::styled(":follow ", t),
                Span::styled("/", k), Span::styled(":search ", t),
                Span::styled("w", k), Span::styled(format!(":{} ", wrap_label), t),
                Span::styled("S", k), Span::styled(":save ", t),
            ];
            if containers.len() > 1 {
                v.push(Span::styled("Tab", k));
                v.push(Span::styled(":container ", t));
            }
            v.push(Span::styled(
                follow_indicator,
                Style::default().fg(Color::Rgb(80, 200, 120)).bg(theme::STATUS_BAR_BG),
            ));
            v
        },
    };
    if !is_viewer(state.current_view()) {
        spans.push(Span::styled(" ?", k));
        spans.push(Span::styled(":help", t));
    }
    // Show configured shortcuts for Terraform views
    let is_tf_view = matches!(
        state.current_view(),
        ViewState::List(TabKind::Terraform)
        | ViewState::List(TabKind::CustomTab(_))
        | ViewState::TerraformDetail { .. }
    );
    if is_tf_view {
        for shortcut in &state.config.shortcuts {
            spans.push(Span::styled(format!(" {}", shortcut.key), k));
            spans.push(Span::styled(format!(":{}", shortcut.label), t));
        }
    }
    if state.mouse_enabled {
        spans.push(Span::styled(
            " [MOUSE]",
            Style::default().fg(Color::Rgb(80, 200, 120)).bg(theme::STATUS_BAR_BG),
        ));
    }
    spans
}
