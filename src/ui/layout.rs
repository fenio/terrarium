use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

use crate::state::store::{AppState, InputMode, TabKind, ViewState};
use crate::ui::{
    controller_dashboard, custom_tab, dialog, help, kustomization_detail, kustomization_list,
    namespace_picker, resource_list, runner_list, status_bar, terraform_detail, theme,
};

pub fn render(f: &mut Frame, state: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // Header (logo + nav + search/info)
            Constraint::Min(5),   // Body
            Constraint::Length(1), // Status bar
        ])
        .split(f.area());

    // Track body height for page scrolling
    state.body_height = chunks[1].height;

    render_header_block(f, chunks[0], state);
    render_body(f, chunks[1], state);
    status_bar::render(f, chunks[2], state);

    // Overlays
    if state.input_mode == InputMode::Confirm
        && let Some(dialog_state) = &state.pending_dialog
    {
        dialog::render_confirm(f, &dialog_state.message);
    }

    if state.input_mode == InputMode::Help {
        help::render_help(f);
    }

    if state.input_mode == InputMode::NamespacePicker {
        namespace_picker::render(f, state);
    }

    // Connection / CRD error overlay
    if let Some(err) = &state.connection_error {
        render_error_overlay(f, err);
    } else if state.tf_crd_missing && state.ks_crd_missing {
        render_error_overlay(
            f,
            "Terraform and Kustomization CRDs not found.\n\n\
             tofu-controller does not appear to be installed on this cluster.\n\n\
             Make sure you are connected to the right cluster and that\n\
             tofu-controller (or tf-controller) is deployed.",
        );
    } else if state.tf_crd_missing {
        render_error_overlay(
            f,
            "Terraform CRD not found.\n\n\
             tofu-controller does not appear to be installed on this cluster.\n\
             Kustomization resources are available but Terraform resources\n\
             cannot be managed.\n\n\
             Check that tofu-controller (or tf-controller) is deployed.",
        );
    }
}

fn render_error_overlay(f: &mut Frame, message: &str) {
    let area = f.area();
    let width = 65u16.min(area.width.saturating_sub(4));
    // Border eats 2 columns on each side
    let inner_width = width.saturating_sub(2) as usize;

    // Count wrapped lines so the popup is tall enough
    let wrapped_lines: u16 = message
        .lines()
        .map(|l| {
            if l.is_empty() || inner_width == 0 {
                1
            } else {
                ((l.len() as u16).div_ceil(inner_width as u16)).max(1)
            }
        })
        .sum();
    let height = (wrapped_lines + 2).min(area.height.saturating_sub(4));

    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    f.render_widget(ratatui::widgets::Clear, popup);

    let block = ratatui::widgets::Block::default()
        .title(Span::styled(
            " Connection Error ",
            Style::default()
                .fg(Color::Rgb(240, 80, 80))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(240, 80, 80)))
        .style(Style::default().bg(Color::Rgb(22, 22, 34)));

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let lines: Vec<Line> = message
        .lines()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(Color::Rgb(200, 200, 220)))))
        .collect();

    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        inner,
    );
}

fn render_header_block(f: &mut Frame, area: Rect, state: &mut AppState) {
    let bg = Block::default().style(Style::default().bg(theme::HEADER_BAR_BG));
    f.render_widget(bg, area);

    let tf_count = state.tf_store.state().len();
    let ks_count = state.ks_store.state().len();
    let runner_count = state.runner_pods.len();
    let tf_failures_raw = count_failures_tf(state);
    let ks_failures_raw = count_failures_ks(state);
    let tf_failures = state.stabilized_tf_failures(tf_failures_raw);
    let ks_failures = state.stabilized_ks_failures(ks_failures_raw);

    let hdr_bg = Style::default().bg(theme::HEADER_BAR_BG);
    let dim = Style::default().fg(Color::Rgb(60, 65, 80)).bg(theme::HEADER_BAR_BG);
    let bright = Style::default().fg(Color::Rgb(200, 210, 230)).bg(theme::HEADER_BAR_BG);
    let fail_style = Style::default()
        .fg(Color::Rgb(240, 80, 80))
        .bg(theme::HEADER_BAR_BG)
        .add_modifier(Modifier::BOLD);
    let info_label = theme::HEADER_CONTEXT_LABEL;

    // -- Rows 0-2: ASCII logo (left) + info pills (right) --
    let logo_lines = [
        "▄▖          ▘     ",
        "▐ █▌▛▘▛▘▀▌▛▘▌▌▌▛▛▌",
        "▐ ▙▖▌ ▌ █▌▌ ▌▙▌▌▌▌",
    ];
    let logo_style = theme::HEADER_LOGO;

    // Logo width (fixed)
    let logo_width: u16 = 21;

    for (i, logo_line) in logo_lines.iter().enumerate() {
        let row_area = Rect {
            y: area.y + i as u16,
            height: 1,
            ..area
        };

        // Render logo on the left
        let logo_area = Rect { width: logo_width.min(area.width), ..row_area };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {}", logo_line), logo_style)),
            logo_area,
        );
    }

    // Info to the right of the logo
    let info_x = area.x + logo_width + 1;
    let info_width = area.width.saturating_sub(logo_width + 1);
    if info_width > 10 {
        let ns_text = match &state.namespace_filter {
            Some(ns) => ns.clone(),
            None => "all".to_string(),
        };
        let (freshness_text, freshness_color) = match state.last_data_update {
            Some(t) => {
                let secs = t.elapsed().as_secs();
                if secs < 5 {
                    ("live".to_string(), Color::Rgb(80, 220, 100))
                } else if secs < 30 {
                    (format!("{}s ago", secs), Color::Rgb(240, 200, 60))
                } else {
                    (format!("{}s ago", secs), Color::Rgb(240, 80, 80))
                }
            }
            None => {
                let dots = ".".repeat((state.tick_count % 3) + 1);
                (format!("connecting{}", dots), Color::Rgb(140, 145, 165))
            }
        };

        // Info row 0: context
        let ctx_area = Rect { x: info_x, y: area.y, width: info_width, height: 1 };
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("ctx: ", info_label),
            Span::styled(format!(" {} ", state.context_name), theme::HEADER_CONTEXT),
        ])), ctx_area);

        // Info row 1: namespace
        let ns_area = Rect { x: info_x, y: area.y + 1, width: info_width, height: 1 };
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled(" ns: ", theme::HEADER_NS_LABEL),
            Span::styled(format!(" {} ", ns_text), theme::HEADER_NS),
        ])), ns_area);

        // Info row 2: freshness
        let fr_area = Rect { x: info_x, y: area.y + 2, width: info_width, height: 1 };
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("  ⟳  ", Style::default().fg(freshness_color).bg(theme::HEADER_BAR_BG)),
            Span::styled(&freshness_text, Style::default().fg(freshness_color).bg(theme::HEADER_BAR_BG)),
        ])), fr_area);
    }

    // -- Row 3: Navigation tabs --
    // (number, label, count_or_none, failures, crd_missing)
    let tf_count_opt = if state.tf_synced { Some(tf_count) } else { None };
    let ks_count_opt = if state.ks_synced { Some(ks_count) } else { None };
    let mut nav_items: Vec<(usize, String, Option<usize>, Option<usize>, bool)> = vec![
        (1, "Controller".to_string(),      None,             None,              false),
        (2, "Terraform".to_string(),       tf_count_opt,     if tf_failures > 0 { Some(tf_failures) } else { None }, state.tf_crd_missing),
        (3, "Kustomizations".to_string(),  ks_count_opt,     if ks_failures > 0 { Some(ks_failures) } else { None }, state.ks_crd_missing),
        (4, "Runners".to_string(),         Some(runner_count), None,            false),
    ];
    for (i, ct) in state.config.custom_tabs.iter().enumerate() {
        let count = if state.tf_synced { Some(custom_tab::count_entries(&state.tf_store, ct)) } else { None };
        nav_items.push((5 + i, ct.name.clone(), count, None, false));
    }

    // Pad nav tabs to start after the logo
    let nav_pad = " ".repeat(logo_width as usize);
    let mut nav_spans: Vec<Span> = vec![Span::styled(&nav_pad, hdr_bg)];
    let total_tabs = state.tab_count();
    for (num, label, count, failures, crd_missing) in &nav_items {
        let is_active = state.active_tab.index(total_tabs) == *num - 1;

        let num_style = if is_active {
            Style::default()
                .fg(Color::Rgb(30, 30, 40))
                .bg(Color::Rgb(100, 180, 255))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Rgb(80, 80, 100))
                .bg(theme::HEADER_BAR_BG)
        };
        nav_spans.push(Span::styled(format!(" {} ", num), num_style));

        let label_style = if is_active {
            Style::default()
                .fg(Color::Rgb(220, 230, 255))
                .bg(theme::HEADER_BAR_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            bright
        };

        if *crd_missing {
            nav_spans.push(Span::styled(
                format!("{:<15}", label),
                Style::default().fg(Color::Rgb(180, 150, 50)).bg(theme::HEADER_BAR_BG),
            ));
            nav_spans.push(Span::styled("no CRD  ", Style::default().fg(Color::Rgb(240, 200, 60)).bg(theme::HEADER_BAR_BG)));
        } else if *num == 1 {
            nav_spans.push(Span::styled(format!("{:<15}        ", label), label_style));
        } else {
            nav_spans.push(Span::styled(format!("{:<15}", label), label_style));
            match count {
                Some(c) => nav_spans.push(Span::styled(format!("{:>3} ", c), dim)),
                None => {
                    let dots = ".".repeat((state.tick_count % 3) + 1);
                    nav_spans.push(Span::styled(format!("{:>4}", dots), dim));
                }
            }
            if let Some(fails) = failures {
                nav_spans.push(Span::styled(format!("{:>3}!", fails), fail_style));
            } else {
                nav_spans.push(Span::styled("    ", hdr_bg));
            }
        }
    }

    let r3_area = Rect { y: area.y + 3, height: 1, ..area };
    f.render_widget(Paragraph::new(Line::from(nav_spans)), r3_area);

    // -- Row 4: filter indicators or container tabs --
    let r4_area = Rect { y: area.y + 4, height: 1, ..area };
    match state.current_view() {
        ViewState::LogViewer {
            namespace,
            pod_name,
            containers,
            active_container,
            ..
        } => {
            let mut spans: Vec<Span> = vec![
                Span::styled(
                    format!(" {}/{} ", namespace, pod_name),
                    Style::default()
                        .fg(Color::Rgb(140, 200, 255))
                        .bg(theme::HEADER_BAR_BG)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " Containers: ",
                    Style::default()
                        .fg(Color::Rgb(140, 145, 165))
                        .bg(theme::HEADER_BAR_BG),
                ),
            ];
            for (i, name) in containers.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" ", hdr_bg));
                }
                if i == *active_container {
                    spans.push(Span::styled(
                        format!(" {} ", name),
                        Style::default()
                            .fg(Color::Rgb(30, 30, 40))
                            .bg(Color::Rgb(100, 220, 140))
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::styled(
                        format!(" {} ", name),
                        Style::default()
                            .fg(Color::Rgb(140, 140, 160))
                            .bg(Color::Rgb(40, 42, 54)),
                    ));
                }
            }
            f.render_widget(Paragraph::new(Line::from(spans)), r4_area);
        }
        _ => {
            let mut info_spans: Vec<Span> = vec![Span::styled(" ", hdr_bg)];
            if state.show_failures_only {
                info_spans.push(Span::styled(
                    " FAILURES ONLY ",
                    Style::default()
                        .fg(Color::Rgb(30, 30, 40))
                        .bg(Color::Rgb(240, 80, 80))
                        .add_modifier(Modifier::BOLD),
                ));
                info_spans.push(Span::styled(" ", hdr_bg));
            }
            if state.show_waiting_only {
                info_spans.push(Span::styled(
                    " WAITING ONLY ",
                    Style::default()
                        .fg(Color::Rgb(30, 30, 40))
                        .bg(Color::Rgb(240, 200, 60))
                        .add_modifier(Modifier::BOLD),
                ));
                info_spans.push(Span::styled(" ", hdr_bg));
            }
            if !state.search_query.is_empty() {
                info_spans.push(Span::styled(
                    format!(" filter: {} ", state.search_query),
                    Style::default()
                        .fg(Color::Rgb(140, 200, 255))
                        .bg(Color::Rgb(30, 40, 60)),
                ));
                info_spans.push(Span::styled(" ", hdr_bg));
            }
            let sort_label = state.sort_column.label();
            info_spans.push(Span::styled(
                format!("sort:{}", sort_label),
                dim,
            ));
            f.render_widget(Paragraph::new(Line::from(info_spans)), r4_area);
        }
    }
}

fn count_failures_tf(state: &AppState) -> usize {
    state
        .tf_store
        .state()
        .iter()
        .filter(|tf| {
            let is_ready = tf
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                .map(|c| c.status == "True")
                .unwrap_or(false);
            !is_ready
        })
        .count()
}

fn count_failures_ks(state: &AppState) -> usize {
    state
        .ks_store
        .state()
        .iter()
        .filter(|ks| {
            let is_ready = ks
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                .map(|c| c.status == "True")
                .unwrap_or(false);
            !is_ready
        })
        .count()
}

// Old render_tabs and render_container_tabs replaced by render_header_block above.

fn render_body(f: &mut Frame, area: Rect, state: &mut AppState) {
    match state.current_view().clone() {
        ViewState::List(tab) => match tab {
            TabKind::Controller => {
                controller_dashboard::render(f, area, state);
            }
            TabKind::Terraform => {
                resource_list::render_terraform_list(f, area, state);
            }
            TabKind::Kustomizations => {
                kustomization_list::render_kustomization_list(f, area, state);
            }
            TabKind::Runners => {
                runner_list::render_runner_list(f, area, state);
            }
            TabKind::CustomTab(i) => {
                custom_tab::render_custom_tab(f, area, state, i);
            }
        },
        ViewState::TerraformDetail {
            ref namespace,
            ref name,
        } => {
            let tf = state
                .tf_store
                .state()
                .iter()
                .find(|t| {
                    t.metadata.namespace.as_deref() == Some(namespace)
                        && t.metadata.name.as_deref() == Some(name)
                })
                .cloned();

            if let Some(tf) = tf {
                let runner_logs = state
                    .runner_logs
                    .get(&(namespace.clone(), name.clone()))
                    .map(|s| s.as_str());
                let cached_outputs = state
                    .cached_outputs
                    .as_ref()
                    .filter(|((ns, n), _)| ns == namespace && n == name)
                    .map(|(_, v)| v);
                terraform_detail::render(f, area, &tf, runner_logs, cached_outputs, &state.config.detail_fields);
            } else {
                let para = Paragraph::new(format!("Resource {}/{} not found", namespace, name))
                    .style(Style::default().fg(Color::Red));
                f.render_widget(para, area);
            }
        }
        ViewState::KustomizationDetail {
            ref namespace,
            ref name,
        } => {
            let ks = state
                .ks_store
                .state()
                .iter()
                .find(|k| {
                    k.metadata.namespace.as_deref() == Some(namespace)
                        && k.metadata.name.as_deref() == Some(name)
                })
                .cloned();

            if let Some(ks) = ks {
                kustomization_detail::render(f, area, &ks);
            } else {
                let para = Paragraph::new(format!("Resource {}/{} not found", namespace, name))
                    .style(Style::default().fg(Color::Red));
                f.render_widget(para, area);
            }
        }
        ViewState::PlanViewer { ref content } => {
            let vp = ViewerParams { scroll: state.plan_scroll, hscroll: state.horizontal_scroll, wrap: state.viewer_wrap, search_query: &state.viewer_search_query };
            render_plan_viewer(f, area, content, &vp);
        }
        ViewState::JsonViewer { ref content } => {
            let vp = ViewerParams { scroll: state.plan_scroll, hscroll: state.horizontal_scroll, wrap: state.viewer_wrap, search_query: &state.viewer_search_query };
            render_json_viewer(f, area, content, &vp);
        }
        ViewState::EventsViewer { ref content } => {
            let vp = ViewerParams { scroll: state.plan_scroll, hscroll: state.horizontal_scroll, wrap: state.viewer_wrap, search_query: &state.viewer_search_query };
            render_viewer(f, area, content, &vp);
        }
        ViewState::OutputsViewer { ref content } => {
            let vp = ViewerParams { scroll: state.plan_scroll, hscroll: state.horizontal_scroll, wrap: state.viewer_wrap, search_query: &state.viewer_search_query };
            render_json_viewer(f, area, content, &vp);
        }
        ViewState::LogViewer { ref content, .. } => {
            let vp = ViewerParams { scroll: state.plan_scroll, hscroll: state.horizontal_scroll, wrap: state.viewer_wrap, search_query: &state.viewer_search_query };
            render_viewer(f, area, content, &vp);
        }
    }
}

struct ViewerParams<'a> {
    scroll: usize,
    hscroll: usize,
    wrap: bool,
    search_query: &'a str,
}

fn highlight_search_in_line<'a>(line: &'a str, query: &str, base_style: Style) -> Line<'a> {
    if query.is_empty() {
        return Line::styled(line, base_style);
    }
    let query_lower = query.to_lowercase();
    let line_lower = line.to_lowercase();
    let mut spans = Vec::new();
    let mut last_end = 0;

    for (start, _) in line_lower.match_indices(&query_lower) {
        if start > last_end {
            spans.push(Span::styled(&line[last_end..start], base_style));
        }
        spans.push(Span::styled(
            &line[start..start + query.len()],
            Style::default()
                .fg(Color::Rgb(30, 30, 40))
                .bg(Color::Rgb(240, 200, 60)),
        ));
        last_end = start + query.len();
    }
    if last_end < line.len() {
        spans.push(Span::styled(&line[last_end..], base_style));
    }
    if spans.is_empty() {
        Line::styled(line, base_style)
    } else {
        Line::from(spans)
    }
}

/// Apply search highlighting on top of an already-styled Line, preserving
/// existing colors for non-matching segments.
fn highlight_search_in_spans<'a>(line: Line<'a>, query: &str) -> Line<'a> {
    if query.is_empty() {
        return line;
    }
    let highlight_style = Style::default()
        .fg(Color::Rgb(30, 30, 40))
        .bg(Color::Rgb(240, 200, 60));
    let query_lower = query.to_lowercase();

    // Flatten all spans into a single string to find match positions
    let full_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let full_lower = full_text.to_lowercase();
    let match_positions: Vec<(usize, usize)> = full_lower
        .match_indices(&query_lower)
        .map(|(start, m)| (start, start + m.len()))
        .collect();

    if match_positions.is_empty() {
        return line;
    }

    // Walk through spans, splitting at match boundaries
    let mut result: Vec<Span<'a>> = Vec::new();
    let mut char_offset: usize = 0;
    let mut match_idx = 0;

    for span in line.spans {
        let span_start = char_offset;
        let span_end = span_start + span.content.len();
        let span_text = span.content;
        let span_style = span.style;

        let mut pos = 0; // position within this span's text
        while pos < span_text.len() && match_idx < match_positions.len() {
            let (m_start, m_end) = match_positions[match_idx];

            if m_start >= span_end {
                // Match is beyond this span
                break;
            }

            // Clamp match to this span's range
            let local_start = m_start.saturating_sub(span_start).max(pos);
            let local_end = m_end.min(span_end) - span_start;

            // Text before the match
            if local_start > pos {
                result.push(Span::styled(
                    span_text[pos..local_start].to_string(),
                    span_style,
                ));
            }

            // The matched portion
            result.push(Span::styled(
                span_text[local_start..local_end].to_string(),
                highlight_style,
            ));

            pos = local_end;
            if m_end <= span_end {
                match_idx += 1;
            } else {
                break; // match continues into next span
            }
        }

        // Remaining text after last match in this span
        if pos < span_text.len() {
            result.push(Span::styled(
                span_text[pos..].to_string(),
                span_style,
            ));
        }

        char_offset = span_end;
    }

    Line::from(result)
}

fn render_viewer(f: &mut Frame, area: Rect, content: &str, vp: &ViewerParams) {
    let scroll_u16 = vp.scroll.min(u16::MAX as usize) as u16;
    let hscroll_u16 = vp.hscroll.min(u16::MAX as usize) as u16;

    if !vp.search_query.is_empty() {
        let base = Style::default().fg(Color::White);
        let lines: Vec<Line> = content.lines().map(|l| highlight_search_in_line(l, vp.search_query, base)).collect();
        let mut para = Paragraph::new(lines).scroll((scroll_u16, hscroll_u16));
        if vp.wrap {
            para = para.wrap(Wrap { trim: false });
        }
        f.render_widget(para, area);
    } else {
        let mut para = Paragraph::new(content)
            .scroll((scroll_u16, hscroll_u16))
            .style(Style::default().fg(Color::White));
        if vp.wrap {
            para = para.wrap(Wrap { trim: false });
        }
        f.render_widget(para, area);
    }
}

fn render_plan_viewer(f: &mut Frame, area: Rect, content: &str, vp: &ViewerParams) {
    let lines: Vec<Line> = content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let style = if trimmed.starts_with("+ ") || trimmed.starts_with("+\t") || trimmed == "+" {
                theme::PLAN_CREATE
            } else if trimmed.starts_with("- ") || trimmed.starts_with("-\t") || trimmed == "-" || trimmed.starts_with("-/") {
                theme::PLAN_DESTROY
            } else if trimmed.starts_with("~ ") || trimmed.starts_with("~\t") || trimmed == "~" {
                theme::PLAN_CHANGE
            } else if trimmed.starts_with("<= ") || trimmed.starts_with("<=\t") {
                theme::PLAN_READ
            } else {
                Style::default().fg(Color::White)
            };
            if !vp.search_query.is_empty() {
                highlight_search_in_line(line, vp.search_query, style)
            } else {
                Line::styled(line, style)
            }
        })
        .collect();

    let scroll_u16 = vp.scroll.min(u16::MAX as usize) as u16;
    let hscroll_u16 = vp.hscroll.min(u16::MAX as usize) as u16;
    let mut para = Paragraph::new(lines).scroll((scroll_u16, hscroll_u16));
    if vp.wrap {
        para = para.wrap(Wrap { trim: false });
    }
    f.render_widget(para, area);
}

fn colorize_json_line<'a>(line: &'a str) -> Line<'a> {
    let trimmed = line.trim();

    // Header lines (non-JSON context lines like "Terraform Outputs for ...")
    if !trimmed.starts_with('"')
        && !trimmed.starts_with('{')
        && !trimmed.starts_with('}')
        && !trimmed.starts_with('[')
        && !trimmed.starts_with(']')
        && !trimmed.starts_with(',')
    {
        // Output key lines like "applications:" or "cluster_id: 12345"
        if let Some(colon_pos) = trimmed.find(':') {
            let key_part = &trimmed[..colon_pos];
            // Only treat as output key if it doesn't start with a quote (not JSON key)
            if !key_part.starts_with('"') {
                let indent = &line[..line.len() - line.trim_start().len()];
                let value_part = &trimmed[colon_pos + 1..];
                return Line::from(vec![
                    Span::raw(indent),
                    Span::styled(key_part, theme::JSON_KEY),
                    Span::styled(":", theme::JSON_BRACE),
                    Span::styled(value_part, colorize_json_value(value_part.trim())),
                ]);
            }
        }
        return Line::styled(line, Style::default().fg(Color::Rgb(160, 170, 200)));
    }

    // Braces and brackets
    if trimmed == "{" || trimmed == "}" || trimmed == "{}" || trimmed == "},"
        || trimmed == "[" || trimmed == "]" || trimmed == "[]" || trimmed == "],"
    {
        return Line::styled(line, theme::JSON_BRACE);
    }

    // JSON key: value lines like `  "key": value`
    let indent = &line[..line.len() - line.trim_start().len()];
    if let Some(colon_pos) = find_json_colon(trimmed) {
        let key_part = &trimmed[..colon_pos];
        let rest = &trimmed[colon_pos + 1..];
        let value = rest.trim();

        let mut spans = vec![
            Span::raw(indent),
            Span::styled(key_part, theme::JSON_KEY),
            Span::styled(": ", theme::JSON_BRACE),
        ];

        let value_style = colorize_json_value(value.trim_end_matches(','));
        let has_comma = value.ends_with(',');
        if has_comma {
            spans.push(Span::styled(&value[..value.len() - 1], value_style));
            spans.push(Span::styled(",", theme::JSON_BRACE));
        } else {
            spans.push(Span::styled(value, value_style));
        }

        return Line::from(spans);
    }

    // Bare values in arrays
    let value_style = colorize_json_value(trimmed.trim_end_matches(','));
    Line::from(vec![
        Span::raw(indent),
        Span::styled(trimmed, value_style),
    ])
}

fn colorize_json_value(value: &str) -> Style {
    if value.starts_with('"') {
        theme::JSON_STRING
    } else if value == "true" || value == "false" {
        theme::JSON_BOOL
    } else if value == "null" {
        theme::JSON_NULL
    } else if value.starts_with('{') || value.starts_with('[') {
        theme::JSON_BRACE
    } else if value.parse::<f64>().is_ok() {
        theme::JSON_NUMBER
    } else {
        Style::default().fg(Color::White)
    }
}

/// Find the colon separating a JSON key from its value, accounting for quotes.
fn find_json_colon(s: &str) -> Option<usize> {
    if !s.starts_with('"') {
        return None;
    }
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            ':' if !in_string => return Some(i),
            _ => {}
        }
    }
    None
}

fn render_json_viewer(f: &mut Frame, area: Rect, content: &str, vp: &ViewerParams) {
    let lines: Vec<Line> = content
        .lines()
        .map(|line| {
            let colorized = colorize_json_line(line);
            if !vp.search_query.is_empty() {
                highlight_search_in_spans(colorized, vp.search_query)
            } else {
                colorized
            }
        })
        .collect();

    let scroll_u16 = vp.scroll.min(u16::MAX as usize) as u16;
    let hscroll_u16 = vp.hscroll.min(u16::MAX as usize) as u16;
    let mut para = Paragraph::new(lines).scroll((scroll_u16, hscroll_u16));
    if vp.wrap {
        para = para.wrap(Wrap { trim: false });
    }
    f.render_widget(para, area);
}
