use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use crate::k8s::kustomization::KustomizationSourceRefKind;
use crate::k8s::terraform::TerraformSourceRefKind;
use crate::state::store::{AppState, InputMode, TabKind, ViewState};
use crate::ui::{
    context_picker, controller_dashboard, custom_tab, dialog, help, kustomization_detail,
    kustomization_list, namespace_picker, resource_list, runner_list, shortcuts_popup,
    source_summary, status_bar, terraform_detail, theme,
};

pub fn render(f: &mut Frame, state: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7), // Header: 3 logo/info + version + 3-row tab strip
            Constraint::Min(5),    // Body
            Constraint::Length(2), // Status bar (two rows)
        ])
        .split(f.area());

    // Track body geometry for page scrolling and mouse hit-testing.
    state.body_y = chunks[1].y;
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

    if state.input_mode == InputMode::ConfirmType
        && let Some(dialog_state) = &state.pending_dialog
    {
        dialog::render_typed_confirm(
            f,
            &dialog_state.message,
            dialog_state.expected_input.as_deref().unwrap_or(""),
            &dialog_state.typed_input,
        );
    }

    if state.input_mode == InputMode::Help {
        help::render_help(f, state);
    }

    if state.input_mode == InputMode::NamespacePicker {
        namespace_picker::render(f, state);
    }

    if state.input_mode == InputMode::ContextPicker {
        context_picker::render(f, state);
    }

    if state.input_mode == InputMode::ShortcutsPopup {
        shortcuts_popup::render(f, state);
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
    // Point the user at the log file for the full (verbose) detail.
    let owned;
    let message: &str = match crate::logging::log_path() {
        Some(p) => {
            owned = format!("{message}\n\nFull details logged to: {}", p.display());
            &owned
        }
        None => message,
    };

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
        .map(|l| {
            Line::from(Span::styled(
                l,
                Style::default().fg(Color::Rgb(200, 200, 220)),
            ))
        })
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
    // Same brightness as the bottom status bar text — discoverable
    // at a glance instead of fading into the header background.
    let dim = Style::default()
        .fg(Color::Rgb(140, 145, 165))
        .bg(theme::HEADER_BAR_BG);
    let bright = Style::default()
        .fg(Color::Rgb(200, 210, 230))
        .bg(theme::HEADER_BAR_BG);
    let fail_style = Style::default()
        .fg(Color::Rgb(240, 80, 80))
        .bg(theme::HEADER_BAR_BG)
        .add_modifier(Modifier::BOLD);
    let info_label = theme::HEADER_CONTEXT_LABEL;

    // The gecko occupies the unused right side of the logo/info header on
    // wide terminals. Keep it out of the info area so long context names do
    // not get painted underneath it.
    const GECKO_WIDTH: u16 = 31;
    const GECKO_MIN_HEADER_WIDTH: u16 = 100;
    let gecko_x = if area.width >= GECKO_MIN_HEADER_WIDTH {
        Some(area.x + area.width - GECKO_WIDTH)
    } else {
        None
    };
    let info_right = gecko_x.unwrap_or(area.x + area.width);

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
        let logo_area = Rect {
            width: logo_width.min(area.width),
            ..row_area
        };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {logo_line}"), logo_style)),
            logo_area,
        );
    }

    // Put the version in the open line beneath the wordmark rather than
    // competing with the context information on the right.
    let version_area = Rect {
        x: area.x,
        y: area.y + logo_lines.len() as u16,
        width: logo_width.min(area.width),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" v{}", env!("CARGO_PKG_VERSION")),
            Style::default()
                .fg(Color::Rgb(80, 90, 120))
                .bg(theme::HEADER_BAR_BG),
        )),
        version_area,
    );

    // Info to the right of the logo
    let info_x = area.x + logo_width + 1;
    let info_width = info_right.saturating_sub(info_x + 1);
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
                    (format!("{secs}s ago"), Color::Rgb(240, 200, 60))
                } else {
                    (format!("{secs}s ago"), Color::Rgb(240, 80, 80))
                }
            }
            None => {
                let dots = ".".repeat((state.tick_count % 3) + 1);
                (format!("connecting{dots}"), Color::Rgb(140, 145, 165))
            }
        };

        // Info row 0: context
        let ctx_area = Rect {
            x: info_x,
            y: area.y,
            width: info_width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("ctx: ", info_label),
                Span::styled(format!(" {} ", state.context_name), theme::HEADER_CONTEXT),
            ])),
            ctx_area,
        );

        // Info row 1: namespace
        let ns_area = Rect {
            x: info_x,
            y: area.y + 1,
            width: info_width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ns: ", theme::HEADER_NS_LABEL),
                Span::styled(format!(" {ns_text} "), theme::HEADER_NS),
            ])),
            ns_area,
        );

        // Info row 2: freshness
        let fr_area = Rect {
            x: info_x,
            y: area.y + 2,
            width: info_width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  ⟳  ",
                    Style::default()
                        .fg(freshness_color)
                        .bg(theme::HEADER_BAR_BG),
                ),
                Span::styled(
                    &freshness_text,
                    Style::default()
                        .fg(freshness_color)
                        .bg(theme::HEADER_BAR_BG),
                ),
            ])),
            fr_area,
        );
    }

    // -- Rows 4-6: Browser-tab-style navigation --
    // (number, label, count_or_none, failures, crd_missing)
    type NavItem = (usize, String, Option<usize>, Option<usize>, bool);
    let tf_count_opt = if state.tf_synced {
        Some(tf_count)
    } else {
        None
    };
    let ks_count_opt = if state.ks_synced {
        Some(ks_count)
    } else {
        None
    };
    let runners_count_opt = if state.runners_synced {
        Some(runner_count)
    } else {
        None
    };
    let mut nav_items: Vec<NavItem> = vec![
        (1, "Controller".to_string(), None, None, false),
        (
            2,
            "Terraform".to_string(),
            tf_count_opt,
            if tf_failures > 0 {
                Some(tf_failures)
            } else {
                None
            },
            state.tf_crd_missing,
        ),
        (
            3,
            "Kustomizations".to_string(),
            ks_count_opt,
            if ks_failures > 0 {
                Some(ks_failures)
            } else {
                None
            },
            state.ks_crd_missing,
        ),
        (4, "Runners".to_string(), runners_count_opt, None, false),
    ];
    for (i, ct) in state.config.custom_tabs.iter().enumerate() {
        let count = if state.tf_synced {
            Some(custom_tab::count_entries(&state.tf_store, ct))
        } else {
            None
        };
        nav_items.push((5 + i, ct.name.clone(), count, None, false));
    }

    let nav_pad = " ".repeat(logo_width as usize);
    let mut top_spans: Vec<Span> = vec![Span::styled(nav_pad.clone(), hdr_bg)];
    let mut body_spans: Vec<Span> = vec![Span::styled(nav_pad.clone(), hdr_bg)];
    let mut bot_spans: Vec<Span> = vec![Span::styled(nav_pad.clone(), hdr_bg)];
    state.tab_hit_ranges.clear();
    let mut tab_x = area.x.saturating_add(logo_width);
    let total_tabs = state.tab_count();
    for (i, (num, label, count, failures, crd_missing)) in nav_items.iter().enumerate() {
        let is_active = state.active_tab.index(total_tabs) == *num - 1;

        let border_style = if is_active {
            theme::BORDER.bg(theme::HEADER_BAR_BG)
        } else {
            Style::default()
                .fg(Color::Rgb(60, 70, 90))
                .bg(theme::HEADER_BAR_BG)
        };

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

        let label_style = if is_active {
            Style::default()
                .fg(Color::Rgb(220, 230, 255))
                .bg(theme::HEADER_BAR_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            bright
        };

        // Build inner spans for the body row. Each slot has a fixed width
        // so the tab's box doesn't change size when data loads in (which
        // would otherwise reflow every tab to its right).
        //   ` N ` (3) + `Label ` + count_slot (4) + failures_slot (4)
        // Controller has no count/failures slots — its width is just digit
        // + label.
        let mut inner: Vec<Span> = Vec::new();
        inner.push(Span::styled(format!(" {num} "), num_style));
        if *crd_missing {
            inner.push(Span::styled(format!("{label} "), label_style));
            inner.push(Span::styled(
                "no CRD ",
                Style::default()
                    .fg(Color::Rgb(240, 200, 60))
                    .bg(theme::HEADER_BAR_BG),
            ));
        } else if *num == 1 {
            // Controller: no count or failures — keep its tab tight.
            inner.push(Span::styled(format!("{label} "), label_style));
        } else {
            inner.push(Span::styled(format!("{label} "), label_style));
            // Count slot: 4 chars wide. " 356" / " ..." / "    "
            let count_text = match count {
                Some(c) => format!("{c:>3} "),
                None => {
                    let dots = ".".repeat((state.tick_count % 3) + 1);
                    format!("{dots:>3} ")
                }
            };
            inner.push(Span::styled(count_text, dim));
            // Failures slot: 4 chars wide. " 33!" or "    "
            let fail_text = match failures {
                Some(f) => format!("{f:>3}!"),
                None => "    ".to_string(),
            };
            inner.push(Span::styled(fail_text, fail_style));
        }

        // Total visible width of the inner content (assumes ASCII / single-
        // width chars, which matches every label/digit/glyph used here).
        let inner_width: usize = inner.iter().map(|s| s.content.chars().count()).sum();
        let tab_width = inner_width.saturating_add(2) as u16;
        state
            .tab_hit_ranges
            .push((tab_x, tab_x.saturating_add(tab_width)));

        // Row 4: ╭───╮  Row 5: │ inner │  Row 6: ╰───╯
        top_spans.push(Span::styled("╭", border_style));
        top_spans.push(Span::styled("─".repeat(inner_width), border_style));
        top_spans.push(Span::styled("╮", border_style));

        body_spans.push(Span::styled("│", border_style));
        body_spans.extend(inner);
        body_spans.push(Span::styled("│", border_style));

        bot_spans.push(Span::styled("╰", border_style));
        bot_spans.push(Span::styled("─".repeat(inner_width), border_style));
        bot_spans.push(Span::styled("╯", border_style));

        // Single-cell gap between tabs (skip after last).
        if i < nav_items.len() - 1 {
            top_spans.push(Span::styled(" ", hdr_bg));
            body_spans.push(Span::styled(" ", hdr_bg));
            bot_spans.push(Span::styled(" ", hdr_bg));
            tab_x = tab_x.saturating_add(tab_width).saturating_add(1);
        } else {
            tab_x = tab_x.saturating_add(tab_width);
        }
    }

    // The fourth gecko line shares the otherwise-unused right side of the
    // first tab-strip row, so the artwork stays out of the version line.
    let r3_area = Rect {
        y: area.y + 4,
        height: 1,
        ..area
    };
    f.render_widget(Paragraph::new(Line::from(top_spans)), r3_area);
    let r4_area = Rect {
        y: area.y + 5,
        height: 1,
        ..area
    };
    f.render_widget(Paragraph::new(Line::from(body_spans)), r4_area);
    let r5_area = Rect {
        y: area.y + 6,
        height: 1,
        ..area
    };
    f.render_widget(Paragraph::new(Line::from(bot_spans)), r5_area);

    if let Some(x) = gecko_x {
        let gecko_lines = gecko_frame(state.tick_count);
        // Render each line separately after the tab strip. The fourth line
        // shares the otherwise-unused right side of the strip's top-border
        // row; painting it afterward keeps the complete silhouette visible
        // without adding another global header row.
        for (row, line) in gecko_lines.iter().enumerate() {
            f.render_widget(
                Paragraph::new(Span::styled(*line, theme::HEADER_GECKO)),
                Rect {
                    x,
                    y: area.y + 1 + row as u16,
                    width: GECKO_WIDTH,
                    height: 1,
                },
            );
        }
    }

    // Right-side indicators on the tab body row: state-filter pills (loud,
    // so the user doesn't forget the filter is on) and the active search
    // query (otherwise it's invisible once the search box closes).
    let mut pill_spans: Vec<Span> = Vec::new();
    if state.show_failures_only {
        pill_spans.push(Span::styled(
            " FAILURES ONLY ",
            Style::default()
                .fg(Color::Rgb(30, 30, 40))
                .bg(Color::Rgb(240, 80, 80))
                .add_modifier(Modifier::BOLD),
        ));
    }
    if state.show_waiting_only {
        if !pill_spans.is_empty() {
            pill_spans.push(Span::styled(" ", hdr_bg));
        }
        pill_spans.push(Span::styled(
            " WAITING ONLY ",
            Style::default()
                .fg(Color::Rgb(30, 30, 40))
                .bg(Color::Rgb(240, 200, 60))
                .add_modifier(Modifier::BOLD),
        ));
    }
    if state.show_progressing_only {
        if !pill_spans.is_empty() {
            pill_spans.push(Span::styled(" ", hdr_bg));
        }
        pill_spans.push(Span::styled(
            " PROGRESSING ONLY ",
            Style::default()
                .fg(Color::Rgb(30, 30, 40))
                .bg(Color::Rgb(120, 200, 230))
                .add_modifier(Modifier::BOLD),
        ));
    }
    if state.show_drifting_only && matches!(state.active_tab, TabKind::Terraform) {
        if !pill_spans.is_empty() {
            pill_spans.push(Span::styled(" ", hdr_bg));
        }
        pill_spans.push(Span::styled(
            " DRIFTING ONLY ",
            Style::default()
                .fg(Color::Rgb(30, 30, 40))
                .bg(Color::Rgb(200, 140, 255))
                .add_modifier(Modifier::BOLD),
        ));
    }
    if state.show_deleting_only {
        if !pill_spans.is_empty() {
            pill_spans.push(Span::styled(" ", hdr_bg));
        }
        pill_spans.push(Span::styled(
            " DELETING ONLY ",
            Style::default()
                .fg(Color::Rgb(30, 30, 40))
                .bg(Color::Rgb(240, 100, 100))
                .add_modifier(Modifier::BOLD),
        ));
    }
    if !state.search_query.is_empty() {
        if !pill_spans.is_empty() {
            pill_spans.push(Span::styled(" ", hdr_bg));
        }
        let (text, style) = if state.search_suspended {
            (
                format!(" filter: {} (paused) ", state.search_query),
                Style::default()
                    .fg(Color::Rgb(120, 120, 140))
                    .bg(Color::Rgb(30, 40, 60)),
            )
        } else {
            (
                format!(" filter: {} ", state.search_query),
                Style::default()
                    .fg(Color::Rgb(30, 30, 40))
                    .bg(Color::Rgb(140, 200, 255))
                    .add_modifier(Modifier::BOLD),
            )
        };
        pill_spans.push(Span::styled(text, style));
    }
    if !pill_spans.is_empty() {
        let pill_width: usize = pill_spans.iter().map(|s| s.content.chars().count()).sum();
        let pill_width = pill_width.min(area.width as usize);
        let pill_x = area.x + area.width.saturating_sub(pill_width as u16 + 1);
        let pill_area = Rect {
            x: pill_x,
            y: area.y + 4,
            width: pill_width as u16,
            height: 1,
        };
        f.render_widget(Paragraph::new(Line::from(pill_spans)), pill_area);
    }
}

/// Return the current gecko frame. The app ticks every 250 ms, so changing
/// every 40 ticks gives the lizard a relaxed ten-second pose interval.
///
/// Artwork attribution: `kat/dew`, posted by `Phydeaux` in the
/// `alt.ascii-art` thread "Re: Gecko please" (31 Mar 2002). Keep this credit
/// with the artwork if the frames are copied or adapted:
/// https://www.asciiart.eu/archives/usenet/message/mcb37280b71
fn gecko_frame(ticks: usize) -> [&'static str; 4] {
    const POSE_TICKS: usize = 40;
    const FRAME_A: [&str; 4] = [
        r"        .)/     )/,",
        r"         /`-._,-'`._,@`-,",
        r"  ,  _,-=\,-.__,-.-.__@/",
        r" (_,'    )\`    '(`",
    ];
    const FRAME_B: [&str; 4] = [
        r"         .,     )/_  ,@`-,",
        r"        '\\_____)\_.'__@/",
        r"   (_,-==( ______  .'",
        r"        '/,      7(,",
    ];
    if (ticks / POSE_TICKS) % 2 == 0 {
        FRAME_A
    } else {
        FRAME_B
    }
}

fn count_failures_tf(state: &AppState) -> usize {
    state
        .tf_store
        .state()
        .iter()
        .filter(|tf| {
            crate::util::classify_ready(tf.status.as_ref().and_then(|s| s.conditions.as_ref()))
                .is_real_failure()
        })
        .count()
}

fn count_failures_ks(state: &AppState) -> usize {
    state
        .ks_store
        .state()
        .iter()
        .filter(|ks| {
            crate::util::classify_ready(ks.status.as_ref().and_then(|s| s.conditions.as_ref()))
                .is_real_failure()
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
                let source_gr = if matches!(
                    tf.spec.source_ref.kind,
                    TerraformSourceRefKind::GitRepository
                ) {
                    source_summary::find_gitrepo(
                        &state.gr_store,
                        tf.spec.source_ref.namespace.as_deref(),
                        namespace,
                        &tf.spec.source_ref.name,
                    )
                } else {
                    None
                };
                let ctx = terraform_detail::RenderCtx {
                    runner_logs,
                    cached_outputs,
                    detail_fields: &state.config.detail_fields,
                    source_gr: source_gr.as_deref(),
                    gr_synced: state.gr_synced,
                };
                terraform_detail::render(f, area, &tf, &ctx);
            } else {
                let para = Paragraph::new(format!("Resource {namespace}/{name} not found"))
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
                let source_gr = if matches!(
                    ks.spec.source_ref.kind,
                    KustomizationSourceRefKind::GitRepository
                ) {
                    source_summary::find_gitrepo(
                        &state.gr_store,
                        ks.spec.source_ref.namespace.as_deref(),
                        namespace,
                        &ks.spec.source_ref.name,
                    )
                } else {
                    None
                };
                kustomization_detail::render(f, area, &ks, source_gr.as_deref(), state.gr_synced);
            } else {
                let para = Paragraph::new(format!("Resource {namespace}/{name} not found"))
                    .style(Style::default().fg(Color::Red));
                f.render_widget(para, area);
            }
        }
        ViewState::PlanViewer { ref content } => {
            let vp = ViewerParams {
                scroll: state.plan_scroll,
                hscroll: state.horizontal_scroll,
                wrap: state.viewer_wrap,
                search_query: &state.viewer_search_query,
            };
            render_plan_viewer(f, area, content, &vp);
        }
        ViewState::JsonViewer { ref content } => {
            let vp = ViewerParams {
                scroll: state.plan_scroll,
                hscroll: state.horizontal_scroll,
                wrap: state.viewer_wrap,
                search_query: &state.viewer_search_query,
            };
            render_json_viewer(f, area, content, &vp);
        }
        ViewState::EventsViewer { ref content } => {
            let vp = ViewerParams {
                scroll: state.plan_scroll,
                hscroll: state.horizontal_scroll,
                wrap: state.viewer_wrap,
                search_query: &state.viewer_search_query,
            };
            render_events_viewer(f, area, content, &vp);
        }
        ViewState::OutputsViewer { ref content } => {
            let vp = ViewerParams {
                scroll: state.plan_scroll,
                hscroll: state.horizontal_scroll,
                wrap: state.viewer_wrap,
                search_query: &state.viewer_search_query,
            };
            render_json_viewer(f, area, content, &vp);
        }
        ViewState::ConditionsViewer { ref content } => {
            let vp = ViewerParams {
                scroll: state.plan_scroll,
                hscroll: state.horizontal_scroll,
                wrap: state.viewer_wrap,
                search_query: &state.viewer_search_query,
            };
            render_conditions_viewer(f, area, content, &vp);
        }
        ViewState::LogViewer {
            ref namespace,
            ref pod_name,
            ref containers,
            active_container,
            ref content,
        } => {
            let vp = ViewerParams {
                scroll: state.plan_scroll,
                hscroll: state.horizontal_scroll,
                wrap: state.viewer_wrap,
                search_query: &state.viewer_search_query,
            };
            // Reserve a top row for the container picker so it stays close
            // to the log content it controls.
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(area);
            render_log_picker(
                f,
                chunks[0],
                namespace,
                pod_name,
                containers,
                active_container,
            );
            render_viewer(f, chunks[1], content, &vp);
        }
    }
}

fn render_log_picker(
    f: &mut Frame,
    area: Rect,
    namespace: &str,
    pod_name: &str,
    containers: &[String],
    active_container: usize,
) {
    let mut spans: Vec<Span> = vec![
        Span::styled(
            format!(" {namespace}/{pod_name} "),
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " Containers: ",
            Style::default().fg(Color::Rgb(140, 145, 165)),
        ),
    ];
    for (i, name) in containers.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        if i == active_container {
            spans.push(Span::styled(
                format!(" {name} "),
                Style::default()
                    .fg(Color::Rgb(30, 30, 40))
                    .bg(Color::Rgb(100, 220, 140))
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {name} "),
                Style::default()
                    .fg(Color::Rgb(140, 140, 160))
                    .bg(Color::Rgb(40, 42, 54)),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
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
            result.push(Span::styled(span_text[pos..].to_string(), span_style));
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
        let lines: Vec<Line> = content
            .lines()
            .map(|l| highlight_search_in_line(l, vp.search_query, base))
            .collect();
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
            let style = if trimmed.starts_with("+ ") || trimmed.starts_with("+\t") || trimmed == "+"
            {
                theme::PLAN_CREATE
            } else if trimmed.starts_with("- ")
                || trimmed.starts_with("-\t")
                || trimmed == "-"
                || trimmed.starts_with("-/")
            {
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

/// Colorize a single line of the Conditions viewer. Recognises the
/// scaffolding emitted by `util::format_conditions_viewer` (kind/name
/// header, "icon type status (reason)" rows, transition/gen metadata)
/// and the structured terraform/Helm output that follows:
///   - `Error: …` lines highlighted in red,
///   - `on <file> line N` source-location lines dimmed,
///   - numbered code excerpts (`   28: resource …`) dimmed with the
///     line number tinted so it scans against the body.
///
/// Falls back to plain white for anything that doesn't match a pattern.
fn colorize_condition_line(line: &str) -> Line<'_> {
    let trimmed_start = line.trim_start();
    let indent = &line[..line.len() - trimmed_start.len()];

    // Kind: ns/name header (first line of viewer output)
    if indent.is_empty()
        && !line.starts_with(['✓', '✗', '…'])
        && let Some(colon) = line.find(':')
        && line[colon + 1..].contains('/')
    {
        let (k, rest) = line.split_at(colon);
        return Line::from(vec![
            Span::styled(k.to_string(), theme::BLOCK_TITLE),
            Span::styled(":", theme::JSON_BRACE),
            Span::styled(rest[1..].to_string(), Style::default().fg(Color::White)),
        ]);
    }

    // Condition header: "✓ Type           Status  (Reason)"
    let first_char = trimmed_start.chars().next();
    if matches!(first_char, Some('✓') | Some('✗') | Some('…')) {
        let icon = first_char.unwrap();
        let icon_style = match icon {
            '✓' => theme::STATUS_READY,
            '✗' => theme::STATUS_NOT_READY,
            _ => theme::STATUS_UNKNOWN,
        };
        let (head, reason) = match trimmed_start.find(" (") {
            Some(i) => (&trimmed_start[..i], Some(&trimmed_start[i..])),
            None => (trimmed_start, None),
        };
        let mut parts = head.splitn(2, ' ');
        let _icon_str = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        let (ty, status) = match rest.rfind("  ") {
            Some(i) => (rest[..i].trim_end(), rest[i..].trim_start()),
            None => (rest, ""),
        };
        let pad_after_type = rest.len().saturating_sub(ty.len() + status.len());
        let mut spans = vec![
            Span::raw(indent.to_string()),
            Span::styled(format!("{icon} "), icon_style),
            Span::styled(
                ty.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(pad_after_type)),
            Span::styled(status.to_string(), icon_style),
        ];
        if let Some(r) = reason {
            spans.push(Span::styled(r.to_string(), theme::LABEL));
        }
        return Line::from(spans);
    }

    // Metadata line: "   transition: …   gen: N"
    if trimmed_start.starts_with("transition:") || trimmed_start.starts_with("gen:") {
        return Line::styled(line, theme::LABEL);
    }

    // Error/Warning headings.
    if trimmed_start.starts_with("Error:") || trimmed_start.starts_with("Error running") {
        return Line::from(vec![
            Span::raw(indent.to_string()),
            Span::styled(trimmed_start.to_string(), theme::STATUS_NOT_READY),
        ]);
    }
    if trimmed_start.starts_with("Warning:") {
        return Line::from(vec![
            Span::raw(indent.to_string()),
            Span::styled(trimmed_start.to_string(), theme::PLAN_CHANGE),
        ]);
    }

    // Source-location hints: "on <file> line N[, in …]"
    if trimmed_start.starts_with("on ") && trimmed_start.contains(" line ") {
        return Line::styled(line, theme::LABEL);
    }

    // Numbered code excerpts: "   28: resource …"
    if let Some(colon_pos) = trimmed_start.find(':') {
        let num_part = &trimmed_start[..colon_pos];
        if !num_part.is_empty() && num_part.chars().all(|c| c.is_ascii_digit()) {
            let rest = &trimmed_start[colon_pos..];
            return Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(num_part.to_string(), theme::JSON_NUMBER),
                Span::styled(
                    rest.to_string(),
                    Style::default().fg(Color::Rgb(160, 170, 200)),
                ),
            ]);
        }
    }

    Line::styled(line, Style::default().fg(Color::White))
}

/// Colorize a single line of the Events viewer. The fetch_events
/// helper formats each event as
/// `<RFC3339-timestamp> [<Type>] <Reason> (xN) — <message>`, where the
/// message body may itself contain embedded newlines (tofu-controller's
/// DriftDetected event, for instance, ships the entire plan in the
/// message). Header lines are decomposed and styled per field; lines
/// that don't match the header shape are treated as message
/// continuations and routed through the plan colorizer so embedded
/// terraform output picks up the same +/-/~/<= highlighting.
fn colorize_event_line(line: &str) -> Line<'_> {
    let bytes = line.as_bytes();
    let looks_like_timestamp =
        bytes.len() >= 20 && bytes[4] == b'-' && bytes[7] == b'-' && bytes[10] == b'T';
    if !looks_like_timestamp {
        return colorize_plan_line(line);
    }

    let Some(after_ts) = line.find(" [") else {
        return Line::styled(line, Style::default().fg(Color::White));
    };
    let ts = &line[..after_ts];
    let rest = &line[after_ts + 1..];
    let Some(end_type) = rest.find("] ") else {
        return Line::styled(line, Style::default().fg(Color::White));
    };
    let type_with_brackets = &rest[..=end_type];
    let type_inner = &rest[1..end_type];
    let after_type = &rest[end_type + 2..];

    let type_style = match type_inner {
        "Normal" => theme::STATUS_READY,
        "Warning" => theme::PLAN_CHANGE,
        "Error" => theme::STATUS_NOT_READY,
        _ => theme::STATUS_UNKNOWN,
    };

    let (reason, after_reason) = if let Some(i) = after_type.find(" (x") {
        (&after_type[..i], &after_type[i..])
    } else if let Some(i) = after_type.find(" — ") {
        (&after_type[..i], &after_type[i..])
    } else {
        (after_type, "")
    };

    let mut spans = vec![
        Span::styled(ts.to_string(), theme::LABEL),
        Span::raw(" "),
        Span::styled(type_with_brackets.to_string(), type_style),
        Span::raw(" "),
        Span::styled(
            reason.to_string(),
            Style::default()
                .fg(Color::Rgb(140, 200, 255))
                .add_modifier(Modifier::BOLD),
        ),
    ];

    let after_reason = if let Some(stripped) = after_reason.strip_prefix(' ')
        && stripped.starts_with("(x")
        && let Some(end) = stripped.find(')')
    {
        let count = &stripped[..=end];
        spans.push(Span::raw(" "));
        spans.push(Span::styled(count.to_string(), theme::LABEL));
        &stripped[end + 1..]
    } else {
        after_reason
    };

    if !after_reason.is_empty() {
        spans.push(Span::styled(
            after_reason.to_string(),
            Style::default().fg(Color::White),
        ));
    }

    Line::from(spans)
}

/// Plan-line colorizer used by both the dedicated plan viewer and the
/// events viewer (for continuation lines coming from embedded plan
/// output in event messages).
fn colorize_plan_line(line: &str) -> Line<'_> {
    let trimmed = line.trim_start();
    let style = if trimmed.starts_with("+ ") || trimmed.starts_with("+\t") || trimmed == "+" {
        theme::PLAN_CREATE
    } else if trimmed.starts_with("- ")
        || trimmed.starts_with("-\t")
        || trimmed == "-"
        || trimmed.starts_with("-/")
    {
        theme::PLAN_DESTROY
    } else if trimmed.starts_with("~ ") || trimmed.starts_with("~\t") || trimmed == "~" {
        theme::PLAN_CHANGE
    } else if trimmed.starts_with("<= ") || trimmed.starts_with("<=\t") {
        theme::PLAN_READ
    } else {
        Style::default().fg(Color::White)
    };
    Line::styled(line, style)
}

fn render_events_viewer(f: &mut Frame, area: Rect, content: &str, vp: &ViewerParams) {
    let lines: Vec<Line> = content
        .lines()
        .map(|line| {
            let colorized = colorize_event_line(line);
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

fn render_conditions_viewer(f: &mut Frame, area: Rect, content: &str, vp: &ViewerParams) {
    let lines: Vec<Line> = content
        .lines()
        .map(|line| {
            let colorized = colorize_condition_line(line);
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

fn colorize_json_line(line: &str) -> Line<'_> {
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
    if trimmed == "{"
        || trimmed == "}"
        || trimmed == "{}"
        || trimmed == "},"
        || trimmed == "["
        || trimmed == "]"
        || trimmed == "[]"
        || trimmed == "],"
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
    Line::from(vec![Span::raw(indent), Span::styled(trimmed, value_style)])
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

#[cfg(test)]
mod tests {
    use super::gecko_frame;

    #[test]
    fn gecko_holds_each_pose_for_ten_seconds() {
        assert_eq!(gecko_frame(0), gecko_frame(39));
        assert_ne!(gecko_frame(0), gecko_frame(40));
        assert_eq!(gecko_frame(40), gecko_frame(79));
        assert_eq!(gecko_frame(0), gecko_frame(80));
    }

    #[test]
    fn gecko_frames_have_four_lines() {
        assert_eq!(gecko_frame(0).len(), 4);
        assert_eq!(gecko_frame(40).len(), 4);
    }
}
