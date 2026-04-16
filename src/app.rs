use std::io::Write;
use std::time::Instant;

use crossterm::event::{self, Event, EventStream, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::action::{Action, ResourceKind};
use crate::k8s::actions as k8s_actions;
use crate::k8s::metrics;
use crate::keys::handle_key;
use crate::state::store::{AppState, DialogState, FlashKind, InputMode, TabKind, ViewState};
use crate::ui::kustomization_list::get_filtered_kustomizations;
use crate::ui::layout;
use crate::ui::resource_list::get_filtered_terraforms;
use crate::ui::runner_list::get_filtered_runners;
use crate::ui::custom_tab::get_filtered_entries;

pub struct App {
    pub state: AppState,
    action_tx: mpsc::UnboundedSender<Action>,
    action_rx: mpsc::UnboundedReceiver<Action>,
    client: Option<kube::Client>,
    should_quit: bool,
}

impl App {
    #[allow(dead_code)]
    pub fn new(
        state: AppState,
        action_tx: mpsc::UnboundedSender<Action>,
        action_rx: mpsc::UnboundedReceiver<Action>,
        client: kube::Client,
    ) -> Self {
        Self {
            state,
            action_tx,
            action_rx,
            client: Some(client),
            should_quit: false,
        }
    }

    pub fn new_deferred(
        state: AppState,
        action_tx: mpsc::UnboundedSender<Action>,
        action_rx: mpsc::UnboundedReceiver<Action>,
    ) -> Self {
        Self {
            state,
            action_tx,
            action_rx,
            client: None,
            should_quit: false,
        }
    }

    pub async fn run(&mut self, terminal: &mut crate::tui::Tui) -> anyhow::Result<()> {
        let mut event_stream = EventStream::new();
        let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(250));

        loop {
            terminal.draw(|f| layout::render(f, &mut self.state))?;

            // Wait for the first event (blocking).
            tokio::select! {
                event = event_stream.next() => {
                    if let Some(Ok(evt)) = event
                        && let Some(action) = self.handle_crossterm_event(evt) {
                            if let Action::ExecBreakTheGlass { namespace, name } = &action {
                                self.exec_break_the_glass(terminal, namespace, name).await;
                            } else {
                                self.dispatch(action).await;
                            }
                        }
                }
                action = self.action_rx.recv() => {
                    if let Some(action) = action {
                        self.dispatch(action).await;
                    }
                }
                _ = tick_interval.tick() => {
                    self.state.expire_flash();
                    self.state.tick_count = self.state.tick_count.wrapping_add(1);
                }
            }

            // Drain any remaining queued events before rendering.
            // This collapses rapid input (e.g. paste) into a single frame.
            while event::poll(std::time::Duration::ZERO)? {
                if let Ok(evt) = event::read() {
                    if let Some(action) = self.handle_crossterm_event(evt) {
                        if let Action::ExecBreakTheGlass { namespace, name } = &action {
                            self.exec_break_the_glass(terminal, namespace, name).await;
                        } else {
                            self.dispatch(action).await;
                        }
                    }
                }
            }

            if self.should_quit {
                break;
            }
        }

        Ok(())
    }

    fn handle_crossterm_event(&self, event: Event) -> Option<Action> {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let action = handle_key(key, self.state.current_view(), &self.state.input_mode);

                // Resolve context-dependent actions
                match (&action, self.state.current_view()) {
                    (Action::None, ViewState::List(TabKind::Terraform))
                    | (Action::None, ViewState::List(TabKind::CustomTab(_))) => {
                        self.resolve_tf_action_from_tab(key.code)
                    }
                    (Action::None, ViewState::List(TabKind::Kustomizations)) => {
                        self.resolve_ks_action(key.code, None, None)
                    }
                    (Action::None, ViewState::List(TabKind::Controller)) => {
                        self.resolve_controller_action(key.code)
                    }
                    (Action::None, ViewState::List(TabKind::Runners)) => {
                        self.resolve_runner_action(key.code)
                    }
                    (Action::None, ViewState::TerraformDetail { namespace, name }) => {
                        self.resolve_tf_action(key.code, Some(namespace), Some(name))
                    }
                    (Action::None, ViewState::KustomizationDetail { namespace, name }) => {
                        self.resolve_ks_action(key.code, Some(namespace), Some(name))
                    }
                    _ => Some(action),
                }
            }
            Event::Mouse(mouse) if self.state.mouse_enabled => self.handle_mouse_event(mouse),
            Event::Resize(w, h) => Some(Action::Resize(w, h)),
            _ => None,
        }
    }

    fn handle_mouse_event(&self, mouse: crossterm::event::MouseEvent) -> Option<Action> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Tab bar is row 1 (0-indexed)
                if mouse.row == 1 {
                    // Approximate tab positions — each tab is roughly area.width / 4
                    let tab_idx = (mouse.column as usize * 4) / self.state.body_height.max(1) as usize;
                    // Simpler: just map column to tab quadrants
                    // Tab bar width is the terminal width, tabs are roughly evenly spaced
                    return Some(Action::GoToTab(tab_idx.min(3)));
                }
                // Body area starts at row 3 (header=0, tabs=1, then header margin)
                // Table rows start after the header row + margin
                let body_start = 3_u16; // header(1) + tabs(1) + column_header(1)
                if mouse.row > body_start {
                    let row_idx = (mouse.row - body_start - 1) as usize; // -1 for column header margin
                    return Some(Action::MouseSelect(row_idx));
                }
                None
            }
            MouseEventKind::ScrollDown => Some(Action::SelectNext),
            MouseEventKind::ScrollUp => Some(Action::SelectPrev),
            _ => None,
        }
    }

    /// Resolve TF actions for list views (Terraform or custom tabs).
    fn resolve_tf_action_from_tab(&self, code: KeyCode) -> Option<Action> {
        let selected = match &self.state.active_tab {
            TabKind::CustomTab(i) => self.get_selected_custom_tab(*i),
            _ => self.get_selected_terraform(),
        };
        let (ns, name) = selected?;
        self.resolve_tf_action(code, Some(&ns), Some(&name))
    }

    /// Resolve Terraform context-dependent actions.
    fn resolve_tf_action(
        &self,
        code: KeyCode,
        ns: Option<&String>,
        name: Option<&String>,
    ) -> Option<Action> {
        let (ns, name) = match (ns, name) {
            (Some(n), Some(nm)) => (n.clone(), nm.clone()),
            _ => self.get_selected_terraform()?,
        };

        match code {
            KeyCode::Char('a') => Some(Action::ShowConfirmDialog(
                Box::new(Action::ApprovePlan {
                    namespace: ns.clone(),
                    name: name.clone(),
                }),
                format!("Approve plan for {}/{}?", ns, name),
            )),
            KeyCode::Char('r') => Some(Action::Reconcile {
                kind: ResourceKind::Terraform,
                namespace: ns,
                name,
            }),
            KeyCode::Char('R') => Some(Action::Replan {
                namespace: ns,
                name,
            }),
            KeyCode::Char('s') => Some(Action::ShowConfirmDialog(
                Box::new(Action::Suspend {
                    kind: ResourceKind::Terraform,
                    namespace: ns.clone(),
                    name: name.clone(),
                }),
                format!("Suspend {}/{}?", ns, name),
            )),
            KeyCode::Char('u') => Some(Action::Resume {
                kind: ResourceKind::Terraform,
                namespace: ns,
                name,
            }),
            KeyCode::Char('p') => Some(Action::FetchPlan {
                namespace: ns,
                name,
                workspace: None,
            }),
            KeyCode::Char('F') => Some(Action::ShowConfirmDialog(
                Box::new(Action::ForceUnlock {
                    namespace: ns.clone(),
                    name: name.clone(),
                }),
                format!("Force unlock state for {}/{}?", ns, name),
            )),
            KeyCode::Char('d') => Some(Action::ShowConfirmDialog(
                Box::new(Action::DeleteResource {
                    namespace: ns.clone(),
                    name: name.clone(),
                }),
                format!("DELETE {}/{}? This cannot be undone!", ns, name),
            )),
            KeyCode::Char('y') => Some(Action::FetchJson {
                kind: ResourceKind::Terraform,
                namespace: ns.clone(),
                name: name.clone(),
            }),
            KeyCode::Char('e') => Some(Action::FetchEvents {
                kind: ResourceKind::Terraform,
                namespace: ns,
                name,
            }),
            KeyCode::Char('O') => Some(Action::FetchOutputs {
                namespace: ns,
                name,
            }),
            KeyCode::Char('x') => Some(Action::ExecBreakTheGlass {
                namespace: ns,
                name,
            }),
            KeyCode::Char(c) => {
                self.state.config.shortcuts.iter().position(|s| s.key == c).map(|idx| {
                    Action::OpenShortcut {
                        namespace: ns.clone(),
                        name: name.clone(),
                        shortcut_idx: idx,
                    }
                })
            }
            _ => None,
        }
    }

    /// Resolve Kustomization context-dependent actions.
    fn resolve_ks_action(
        &self,
        code: KeyCode,
        ns: Option<&String>,
        name: Option<&String>,
    ) -> Option<Action> {
        let (ns, name) = match (ns, name) {
            (Some(n), Some(nm)) => (n.clone(), nm.clone()),
            _ => self.get_selected_kustomization()?,
        };

        match code {
            KeyCode::Char('r') => Some(Action::Reconcile {
                kind: ResourceKind::Kustomization,
                namespace: ns,
                name,
            }),
            KeyCode::Char('s') => Some(Action::ShowConfirmDialog(
                Box::new(Action::Suspend {
                    kind: ResourceKind::Kustomization,
                    namespace: ns.clone(),
                    name: name.clone(),
                }),
                format!("Suspend {}/{}?", ns, name),
            )),
            KeyCode::Char('u') => Some(Action::Resume {
                kind: ResourceKind::Kustomization,
                namespace: ns,
                name,
            }),
            KeyCode::Char('y') => Some(Action::FetchJson {
                kind: ResourceKind::Kustomization,
                namespace: ns.clone(),
                name: name.clone(),
            }),
            KeyCode::Char('e') => Some(Action::FetchEvents {
                kind: ResourceKind::Kustomization,
                namespace: ns,
                name,
            }),
            _ => None,
        }
    }

    fn resolve_controller_action(&self, code: KeyCode) -> Option<Action> {
        match code {
            KeyCode::Char('L') => {
                // Stream logs from the first controller pod
                let pod_name = self.state.controller_info.pods.first()?.name.clone();
                let ns = self.state.controller_info.deploy_namespace.clone();
                Some(Action::StreamControllerLogs { namespace: ns, pod_name })
            }
            _ => None,
        }
    }

    fn resolve_runner_action(&self, code: KeyCode) -> Option<Action> {
        let (ns, name) = self.get_selected_runner()?;
        match code {
            KeyCode::Char('d') => Some(Action::ShowConfirmDialog(
                Box::new(Action::KillRunner {
                    namespace: ns.clone(),
                    name: name.clone(),
                }),
                format!("Kill runner pod {}/{}?", ns, name),
            )),
            _ => None,
        }
    }

    fn get_selected_terraform(&self) -> Option<(String, String)> {
        let selected_idx = self.state.tf_table_state.selected()?;
        let items = get_filtered_terraforms(
            &self.state.tf_store,
            &self.state.namespace_filter,
            &self.state.search_query,
            self.state.show_failures_only,
            self.state.show_waiting_only,
            self.state.sort_column,
            self.state.sort_descending,
        );
        let tf = items.get(selected_idx)?;
        Some((
            tf.metadata.namespace.clone().unwrap_or_default(),
            tf.metadata.name.clone().unwrap_or_default(),
        ))
    }

    fn get_selected_kustomization(&self) -> Option<(String, String)> {
        let selected_idx = self.state.ks_table_state.selected()?;
        let items = get_filtered_kustomizations(
            &self.state.ks_store,
            &self.state.namespace_filter,
            &self.state.search_query,
            self.state.show_failures_only,
            self.state.show_waiting_only,
            self.state.sort_column,
            self.state.sort_descending,
        );
        let ks = items.get(selected_idx)?;
        Some((
            ks.metadata.namespace.clone().unwrap_or_default(),
            ks.metadata.name.clone().unwrap_or_default(),
        ))
    }

    fn get_selected_custom_tab(&self, tab_idx: usize) -> Option<(String, String)> {
        let selected_idx = self.state.custom_tab_states.get(tab_idx)?.selected()?;
        let tab_config = self.state.config.custom_tabs.get(tab_idx)?;
        let items = get_filtered_entries(
            &self.state.tf_store,
            &self.state.namespace_filter,
            &self.state.search_query,
            tab_config,
        );
        let entry = items.get(selected_idx)?;
        Some((entry.namespace.clone(), entry.name.clone()))
    }

    fn get_selected_runner(&self) -> Option<(String, String)> {
        let selected_idx = self.state.runner_table_state.selected()?;
        let items = get_filtered_runners(
            &self.state.runner_pods,
            &self.state.namespace_filter,
            &self.state.search_query,
        );
        let pod = items.get(selected_idx)?;
        Some((
            pod.metadata.namespace.clone().unwrap_or_default(),
            pod.metadata.name.clone().unwrap_or_default(),
        ))
    }

    fn current_list_count(&self) -> usize {
        match self.state.active_tab {
            TabKind::Controller => self.state.backlog_namespaces.len(),
            TabKind::Terraform => get_filtered_terraforms(
                &self.state.tf_store,
                &self.state.namespace_filter,
                &self.state.search_query,
                self.state.show_failures_only,
                self.state.show_waiting_only,
                self.state.sort_column,
                self.state.sort_descending,
            )
            .len(),
            TabKind::Kustomizations => get_filtered_kustomizations(
                &self.state.ks_store,
                &self.state.namespace_filter,
                &self.state.search_query,
                self.state.show_failures_only,
                self.state.show_waiting_only,
                self.state.sort_column,
                self.state.sort_descending,
            )
            .len(),
            TabKind::Runners => get_filtered_runners(
                &self.state.runner_pods,
                &self.state.namespace_filter,
                &self.state.search_query,
            )
            .len(),
            TabKind::CustomTab(i) => {
                if let Some(tab_config) = self.state.config.custom_tabs.get(i) {
                    get_filtered_entries(
                        &self.state.tf_store,
                        &self.state.namespace_filter,
                        &self.state.search_query,
                        tab_config,
                    )
                    .len()
                } else {
                    0
                }
            }
        }
    }

    fn viewer_line_count(&self) -> usize {
        match self.state.current_view() {
            ViewState::PlanViewer { content }
            | ViewState::JsonViewer { content }
            | ViewState::EventsViewer { content }
            | ViewState::OutputsViewer { content }
            | ViewState::LogViewer { content, .. } => content.lines().count(),
            _ => 0,
        }
    }

    fn half_page(&self) -> usize {
        (self.state.body_height as usize / 2).max(1)
    }

    fn compute_viewer_search_matches(&mut self) {
        let query = self.state.viewer_search_query.to_lowercase();
        self.state.viewer_search_matches.clear();
        self.state.viewer_search_index = 0;
        if query.is_empty() {
            return;
        }
        // Clone content to avoid borrow conflict
        let content = match self.state.current_view() {
            ViewState::PlanViewer { content } => content.clone(),
            ViewState::JsonViewer { content } => content.clone(),
            ViewState::EventsViewer { content } => content.clone(),
            ViewState::OutputsViewer { content } => content.clone(),
            ViewState::LogViewer { content, .. } => content.clone(),
            _ => return,
        };
        for (i, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(&query) {
                self.state.viewer_search_matches.push(i);
            }
        }
    }

    fn jump_to_viewer_search_match(&mut self) {
        if let Some(&line) = self.state.viewer_search_matches.get(self.state.viewer_search_index) {
            self.state.plan_scroll = line;
        }
    }

    fn cancel_log_stream(&mut self) {
        if let Some(handle) = self.state.log_stream_handle.take() {
            handle.abort();
        }
    }

    /// Toggle the on-demand controller-metrics panel. When enabled, spawns
    /// a background task that fetches /metrics on a fixed interval; when
    /// disabled, aborts the task and clears the snapshot.
    fn toggle_metrics(&mut self) {
        // Only meaningful on the Controller tab.
        if !matches!(self.state.active_tab, TabKind::Controller) {
            return;
        }

        if self.state.metrics_enabled {
            // Disable
            if let Some(h) = self.state.metrics_task.take() {
                h.abort();
            }
            self.state.metrics_enabled = false;
            self.state.metrics_snapshot = None;
            self.state.metrics_prev = crate::k8s::metrics::PrevCounters::default();
            self.state.metrics_last_error = None;
            return;
        }

        // Enable — need a client and a controller pod
        let Some(client) = self.require_client() else {
            self.state.flash_message = Some((
                "K8s client not ready yet".to_string(),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        };
        let namespace = self.state.controller_info.deploy_namespace.clone();
        let pod_name = self
            .state
            .controller_info
            .pods
            .iter()
            .find(|p| p.ready)
            .or_else(|| self.state.controller_info.pods.first())
            .map(|p| p.name.clone());
        if namespace.is_empty() {
            self.state.flash_message = Some((
                "Controller info not loaded yet — try again in a moment".to_string(),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        }

        self.state.metrics_enabled = true;
        let tx = self.action_tx.clone();
        let handle = tokio::spawn(async move {
            // Fetch immediately, then every 5s.
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                match metrics::fetch(
                    &client,
                    &namespace,
                    pod_name.as_deref(),
                    metrics::DEFAULT_METRICS_PORT,
                )
                .await
                {
                    Ok(snap) => {
                        if tx.send(Action::MetricsSnapshotReceived(snap)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        if tx
                            .send(Action::MetricsFetchError(format!("{:#}", e)))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        self.state.metrics_task = Some(handle);
    }

    async fn dispatch(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,

            // Navigation
            Action::NextTab => self.state.next_tab(),
            Action::PrevTab => self.state.prev_tab(),
            Action::GoToTab(idx) => self.state.go_to_tab(idx),
            Action::ToggleHelp => {
                if self.state.input_mode == InputMode::Help {
                    self.state.input_mode = InputMode::Normal;
                } else {
                    self.state.input_mode = InputMode::Help;
                }
            }
            Action::ToggleFailuresOnly => {
                self.state.show_failures_only = !self.state.show_failures_only;
                if self.state.show_failures_only {
                    self.state.show_waiting_only = false;
                }
                self.state.current_table_state().select(Some(0));
            }
            Action::ToggleWaitingOnly => {
                self.state.show_waiting_only = !self.state.show_waiting_only;
                if self.state.show_waiting_only {
                    self.state.show_failures_only = false;
                }
                self.state.current_table_state().select(Some(0));
            }
            Action::ToggleWrap => {
                self.state.viewer_wrap = !self.state.viewer_wrap;
            }
            Action::ToggleMouse => {
                self.state.mouse_enabled = !self.state.mouse_enabled;
                let _ = crate::tui::set_mouse_capture(self.state.mouse_enabled);
                let label = if self.state.mouse_enabled { "Mouse enabled" } else { "Mouse disabled" };
                self.state.flash_message = Some((label.to_string(), std::time::Instant::now(), crate::state::store::FlashKind::Success));
            }
            Action::ToggleMetrics => {
                self.toggle_metrics();
            }
            Action::MetricsSnapshotReceived(mut snap) => {
                let now = Instant::now();
                metrics::fill_rates(&mut snap, &self.state.metrics_prev, now);
                metrics::update_prev(&mut self.state.metrics_prev, &snap, now);
                self.state.metrics_snapshot = Some(snap);
                self.state.metrics_last_error = None;
            }
            Action::MetricsFetchError(msg) => {
                self.state.metrics_last_error = Some(msg);
            }
            Action::CycleSort => {
                self.state.sort_column = self.state.sort_column.next();
                self.state.current_table_state().select(Some(0));
            }
            Action::InvertSort => {
                self.state.sort_descending = !self.state.sort_descending;
                self.state.current_table_state().select(Some(0));
            }
            Action::ScrollTop => {
                if matches!(self.state.current_view(), ViewState::List(_)) {
                    self.state.current_table_state().select(Some(0));
                } else {
                    self.state.plan_scroll = 0;
                    self.state.horizontal_scroll = 0;
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.state.log_auto_follow = false;
                    }
                }
            }
            Action::ScrollBottom => {
                if matches!(self.state.current_view(), ViewState::List(_)) {
                    let count = self.current_list_count();
                    if count > 0 {
                        self.state.current_table_state().select(Some(count - 1));
                    }
                } else {
                    // Clamp to actual content length
                    let line_count = self.viewer_line_count();
                    let visible = self.state.body_height as usize;
                    self.state.plan_scroll = line_count.saturating_sub(visible);
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.state.log_auto_follow = true;
                    }
                }
            }
            Action::ScrollLeft => {
                self.state.horizontal_scroll = self.state.horizontal_scroll.saturating_sub(4);
            }
            Action::ScrollRight => {
                self.state.horizontal_scroll = self.state.horizontal_scroll.saturating_add(4);
            }
            Action::JumpToFirstFailure => {
                self.jump_to_first_failure();
            }
            Action::NextContainer => {
                self.switch_log_container(1).await;
            }
            Action::PrevContainer => {
                self.switch_log_container(-1).await;
            }
            Action::PageDown => {
                let half = self.half_page();
                if matches!(self.state.current_view(), ViewState::PlanViewer { .. } | ViewState::JsonViewer { .. } | ViewState::EventsViewer { .. } | ViewState::OutputsViewer { .. } | ViewState::LogViewer { .. }) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_add(half);
                } else {
                    let current = self.state.current_table_state().selected().unwrap_or(0);
                    let count = self.current_list_count();
                    if count > 0 {
                        self.state
                            .current_table_state()
                            .select(Some((current + half).min(count - 1)));
                    }
                }
            }
            Action::PageUp => {
                let half = self.half_page();
                if matches!(self.state.current_view(), ViewState::PlanViewer { .. } | ViewState::JsonViewer { .. } | ViewState::EventsViewer { .. } | ViewState::OutputsViewer { .. } | ViewState::LogViewer { .. }) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_sub(half);
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.state.log_auto_follow = false;
                    }
                } else {
                    let current = self
                        .state
                        .current_table_state()
                        .selected()
                        .unwrap_or(0);
                    self.state
                        .current_table_state()
                        .select(Some(current.saturating_sub(half)));
                }
            }
            Action::SelectNext => {
                if matches!(self.state.current_view(), ViewState::PlanViewer { .. } | ViewState::JsonViewer { .. } | ViewState::EventsViewer { .. } | ViewState::OutputsViewer { .. } | ViewState::LogViewer { .. }) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_add(1);
                } else {
                    let current = self.state.current_table_state().selected().unwrap_or(0);
                    let count = self.current_list_count();
                    if count > 0 {
                        self.state
                            .current_table_state()
                            .select(Some((current + 1).min(count - 1)));
                    }
                }
            }
            Action::SelectPrev => {
                if matches!(self.state.current_view(), ViewState::PlanViewer { .. } | ViewState::JsonViewer { .. } | ViewState::EventsViewer { .. } | ViewState::OutputsViewer { .. } | ViewState::LogViewer { .. }) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_sub(1);
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.state.log_auto_follow = false;
                    }
                } else {
                    let current = self
                        .state
                        .current_table_state()
                        .selected()
                        .unwrap_or(0);
                    self.state
                        .current_table_state()
                        .select(Some(current.saturating_sub(1)));
                }
            }
            Action::MouseSelect(row_idx) => {
                let count = self.current_list_count();
                if count > 0 {
                    self.state
                        .current_table_state()
                        .select(Some(row_idx.min(count - 1)));
                }
            }
            Action::Enter => match self.state.active_tab {
                TabKind::Controller => {
                    if let Some(idx) = self.state.backlog_table_state.selected() {
                        if let Some((ns, _, _, _)) = self.state.backlog_namespaces.get(idx) {
                            self.state.namespace_filter = Some(ns.clone());
                            self.state.show_failures_only = true;
                            self.state.active_tab = TabKind::Terraform;
                            self.state.view_stack = vec![ViewState::List(TabKind::Terraform)];
                            self.state.tf_table_state.select(None);
                        }
                    }
                }
                TabKind::Terraform => {
                    if let Some((ns, name)) = self.get_selected_terraform() {
                        self.spawn_detail_outputs_fetch(&ns, &name);
                        self.state.view_stack.push(ViewState::TerraformDetail {
                            namespace: ns,
                            name,
                        });
                    }
                }
                TabKind::Kustomizations => {
                    if let Some((ns, name)) = self.get_selected_kustomization() {
                        self.state
                            .view_stack
                            .push(ViewState::KustomizationDetail {
                                namespace: ns,
                                name,
                            });
                    }
                }
                TabKind::Runners => {
                    if let Some((ns, name)) = self.get_selected_runner() {
                        self.start_log_stream(&ns, &name).await;
                    }
                }
                TabKind::CustomTab(i) => {
                    if let Some((ns, name)) = self.get_selected_custom_tab(i) {
                        self.spawn_detail_outputs_fetch(&ns, &name);
                        self.state.view_stack.push(ViewState::TerraformDetail {
                            namespace: ns,
                            name,
                        });
                    }
                }
            },
            Action::Back => {
                if matches!(self.state.current_view(), ViewState::List(_)) {
                    // Peel off filters one at a time: search → failures/waiting → namespace
                    if !self.state.search_query.is_empty() {
                        self.state.search_query.clear();
                    } else if self.state.show_failures_only {
                        self.state.show_failures_only = false;
                    } else if self.state.show_waiting_only {
                        self.state.show_waiting_only = false;
                    } else if self.state.namespace_filter.is_some() {
                        self.state.namespace_filter = None;
                    }
                    // Reset table selection when filters change
                    self.state.current_table_state().select(None);
                } else if self.state.view_stack.len() > 1 {
                    // Cancel log stream if leaving a LogViewer
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.cancel_log_stream();
                    }
                    self.state.view_stack.pop();
                    self.state.viewer_wrap = false;
                    self.state.horizontal_scroll = 0;
                    self.state.viewer_search_query.clear();
                    self.state.viewer_search_matches.clear();
                }
            }

            // Search
            Action::SearchStart => {
                self.state.input_mode = InputMode::Search;
                self.state.search_query.clear();
            }
            Action::SearchPush(c) => {
                self.state.search_query.push(c);
            }
            Action::SearchPop => {
                self.state.search_query.pop();
            }
            Action::SearchConfirm => {
                self.state.input_mode = InputMode::Normal;
                // Auto-select first filtered result so Enter immediately acts on it
                let count = self.current_list_count();
                if count > 0 && self.state.current_table_state().selected().is_none() {
                    self.state.current_table_state().select(Some(0));
                }
            }
            Action::SearchCancel => {
                self.state.input_mode = InputMode::Normal;
                self.state.search_query.clear();
            }

            // Namespace picker
            Action::OpenNamespacePicker => {
                self.state.ns_picker_items = self.state.collect_namespaces();
                // Pre-select current namespace
                self.state.ns_picker_selected = match &self.state.namespace_filter {
                    None => 0,
                    Some(ns) => self
                        .state
                        .ns_picker_items
                        .iter()
                        .position(|n| n == ns)
                        .unwrap_or(0),
                };
                self.state.input_mode = InputMode::NamespacePicker;
            }
            Action::NamespacePickerNext => {
                let len = self.state.ns_picker_items.len();
                if len > 0 {
                    self.state.ns_picker_selected =
                        (self.state.ns_picker_selected + 1).min(len - 1);
                }
            }
            Action::NamespacePickerPrev => {
                self.state.ns_picker_selected =
                    self.state.ns_picker_selected.saturating_sub(1);
            }
            Action::NamespacePickerSelect => {
                self.state.input_mode = InputMode::Normal;
                if self.state.ns_picker_selected == 0 {
                    self.state.namespace_filter = None;
                } else if let Some(ns) = self
                    .state
                    .ns_picker_items
                    .get(self.state.ns_picker_selected)
                {
                    self.state.namespace_filter = Some(ns.clone());
                }
                // Reset table selections
                self.state.tf_table_state.select(None);
                self.state.ks_table_state.select(None);
                self.state.runner_table_state.select(None);
            }
            Action::NamespacePickerCancel => {
                self.state.input_mode = InputMode::Normal;
            }

            // Viewer search
            Action::ViewerSearchStart => {
                self.state.input_mode = InputMode::ViewerSearch;
                self.state.viewer_search_query.clear();
                self.state.viewer_search_matches.clear();
                self.state.viewer_search_index = 0;
            }
            Action::ViewerSearchPush(c) => {
                self.state.viewer_search_query.push(c);
                self.compute_viewer_search_matches();
                self.jump_to_viewer_search_match();
            }
            Action::ViewerSearchPop => {
                self.state.viewer_search_query.pop();
                self.compute_viewer_search_matches();
                self.jump_to_viewer_search_match();
            }
            Action::ViewerSearchConfirm => {
                self.state.input_mode = InputMode::Normal;
            }
            Action::ViewerSearchCancel => {
                self.state.input_mode = InputMode::Normal;
                self.state.viewer_search_query.clear();
                self.state.viewer_search_matches.clear();
                self.state.viewer_search_index = 0;
            }
            Action::ViewerSearchNext => {
                if !self.state.viewer_search_matches.is_empty() {
                    self.state.viewer_search_index =
                        (self.state.viewer_search_index + 1) % self.state.viewer_search_matches.len();
                    self.jump_to_viewer_search_match();
                }
            }
            Action::ViewerSearchPrev => {
                if !self.state.viewer_search_matches.is_empty() {
                    let len = self.state.viewer_search_matches.len();
                    self.state.viewer_search_index =
                        (self.state.viewer_search_index + len - 1) % len;
                    self.jump_to_viewer_search_match();
                }
            }

            // Bulk selection
            Action::ToggleSelect => {
                let selected = match &self.state.active_tab {
                    TabKind::Terraform => self.get_selected_terraform(),
                    TabKind::CustomTab(i) => self.get_selected_custom_tab(*i),
                    TabKind::Kustomizations => self.get_selected_kustomization(),
                    _ => None,
                };
                if let Some(key) = selected
                    && !self.state.bulk_selected.remove(&key) {
                    self.state.bulk_selected.insert(key);
                }
            }
            Action::BulkReconcile => {
                self.execute_bulk_action(|ns, name, kind| Action::Reconcile {
                    kind,
                    namespace: ns,
                    name,
                });
            }
            Action::BulkSuspend => {
                self.execute_bulk_action(|ns, name, kind| Action::Suspend {
                    kind,
                    namespace: ns,
                    name,
                });
            }
            Action::BulkResume => {
                self.execute_bulk_action(|ns, name, kind| Action::Resume {
                    kind,
                    namespace: ns,
                    name,
                });
            }

            // Save viewer content
            Action::SaveViewerContent => {
                self.save_viewer_content();
            }

            // Confirm dialog
            Action::ShowConfirmDialog(wrapped, message) => {
                self.state.pending_dialog = Some(DialogState {
                    wrapped_action: *wrapped,
                    message,
                });
                self.state.input_mode = InputMode::Confirm;
            }
            Action::ConfirmDialog(confirmed) => {
                self.state.input_mode = InputMode::Normal;
                if confirmed {
                    if let Some(dialog) = self.state.pending_dialog.take() {
                        self.spawn_k8s_action(dialog.wrapped_action);
                    }
                } else {
                    self.state.pending_dialog = None;
                }
            }

            // ExecBreakTheGlass is handled directly in the run loop (needs terminal access)
            Action::ExecBreakTheGlass { .. } => {}

            Action::StreamControllerLogs { namespace, pod_name } => {
                self.start_log_stream(&namespace, &pod_name).await;
            }

            Action::OpenShortcut { namespace, name, shortcut_idx } => {
                self.open_shortcut(&namespace, &name, shortcut_idx);
            }

            // Non-destructive actions dispatch directly
            Action::Reconcile { .. } | Action::Resume { .. } => {
                self.spawn_k8s_action(action);
            }

            // Fetch plan (async)
            Action::FetchPlan {
                namespace,
                name,
                workspace,
            } => {
                self.state.flash_message = Some((
                    format!("Fetching plan for {}/{}...", namespace, name),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some(("K8s client not ready yet".to_string(), Instant::now(), FlashKind::Error));
                    return;
                };
                let tx = self.action_tx.clone();
                tokio::spawn(async move {
                    match k8s_actions::fetch_plan(&client, &namespace, &name, workspace.as_deref())
                        .await
                    {
                        Ok(plan_text) => {
                            let _ = tx.send(Action::PlanFetched(plan_text));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::PlanFetchError(format!("{}", e)));
                        }
                    }
                });
            }
            Action::PlanFetched(plan_text) => {
                self.state.flash_message = None;
                self.state
                    .view_stack
                    .push(ViewState::PlanViewer { content: plan_text });
                self.state.plan_scroll = 0;
                self.state.viewer_wrap = false;
            }
            Action::PlanFetchError(e) => {
                self.state.flash_message =
                    Some((format!("Error: {}", e), Instant::now(), FlashKind::Error));
            }

            // YAML view (async)
            Action::FetchJson {
                kind,
                namespace,
                name,
            } => {
                self.state.flash_message = Some((
                    format!("Fetching JSON for {}/{}...", namespace, name),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some(("K8s client not ready yet".to_string(), Instant::now(), FlashKind::Error));
                    return;
                };
                let tx = self.action_tx.clone();
                tokio::spawn(async move {
                    match k8s_actions::fetch_resource_yaml(&client, &kind, &namespace, &name).await
                    {
                        Ok(yaml) => {
                            let _ = tx.send(Action::JsonFetched(yaml));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::JsonFetchError(format!("{}", e)));
                        }
                    }
                });
            }
            Action::JsonFetched(yaml) => {
                self.state.flash_message = None;
                self.state
                    .view_stack
                    .push(ViewState::JsonViewer { content: yaml });
                self.state.plan_scroll = 0;
                self.state.viewer_wrap = false;
            }
            Action::JsonFetchError(e) => {
                self.state.flash_message =
                    Some((format!("Error: {}", e), Instant::now(), FlashKind::Error));
            }

            // Outputs view (async, Terraform only)
            Action::FetchOutputs { namespace, name } => {
                self.state.flash_message = Some((
                    format!("Fetching outputs for {}/{}...", namespace, name),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some(("K8s client not ready yet".to_string(), Instant::now(), FlashKind::Error));
                    return;
                };
                let tx = self.action_tx.clone();
                tokio::spawn(async move {
                    match k8s_actions::fetch_outputs(&client, &namespace, &name).await {
                        Ok(text) => {
                            let _ = tx.send(Action::OutputsFetched(text));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::OutputsFetchError(format!("{}", e)));
                        }
                    }
                });
            }
            Action::OutputsFetched(text) => {
                self.state.flash_message = None;
                self.state
                    .view_stack
                    .push(ViewState::OutputsViewer { content: text });
                self.state.plan_scroll = 0;
                self.state.horizontal_scroll = 0;
                self.state.viewer_wrap = false;
            }
            Action::OutputsFetchError(e) => {
                self.state.flash_message =
                    Some((format!("Error: {}", e), Instant::now(), FlashKind::Error));
            }

            // Events view (async)
            Action::FetchEvents {
                kind,
                namespace,
                name,
            } => {
                self.state.flash_message = Some((
                    format!("Fetching events for {}/{}...", namespace, name),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some(("K8s client not ready yet".to_string(), Instant::now(), FlashKind::Error));
                    return;
                };
                let tx = self.action_tx.clone();
                tokio::spawn(async move {
                    match k8s_actions::fetch_events(&client, &kind, &namespace, &name).await {
                        Ok(events) => {
                            let _ = tx.send(Action::EventsFetched(events));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::EventsFetchError(format!("{}", e)));
                        }
                    }
                });
            }
            Action::EventsFetched(events) => {
                self.state.flash_message = None;
                self.state
                    .view_stack
                    .push(ViewState::EventsViewer { content: events });
                self.state.plan_scroll = 0;
                self.state.horizontal_scroll = 0;
                self.state.viewer_wrap = false;
            }
            Action::DetailOutputsFetched { namespace, name, values } => {
                self.state.cached_outputs = Some(((namespace, name), values));
            }
            Action::EventsFetchError(e) => {
                self.state.flash_message =
                    Some((format!("Error: {}", e), Instant::now(), FlashKind::Error));
            }

            // Log streaming chunks
            Action::LogChunkReceived(chunk) => {
                const MAX_LOG_BYTES: usize = 10 * 1024 * 1024; // 10 MB
                if let Some(ViewState::LogViewer { content, .. }) =
                    self.state.view_stack.last_mut()
                {
                    if content.len() + chunk.len() > MAX_LOG_BYTES {
                        // Trim the front to stay under the cap
                        content.push_str(&chunk);
                        let excess = content.len() - MAX_LOG_BYTES;
                        if let Some(newline_pos) = content[excess..].find('\n') {
                            *content = content[excess + newline_pos + 1..].to_string();
                        } else {
                            *content = content[excess..].to_string();
                        }
                    } else {
                        content.push_str(&chunk);
                    }
                    if self.state.log_auto_follow {
                        let line_count = content.lines().count();
                        let visible = self.state.body_height as usize;
                        self.state.plan_scroll = line_count.saturating_sub(visible);
                    }
                }
            }

            // Runner pods update from poller
            Action::RunnerPodsUpdated(pods) => {
                self.state.runner_pods = pods;
                self.state.last_data_update = Some(Instant::now());
            }
            Action::RunnerLogsUpdated(logs) => {
                self.state.runner_logs = logs;
            }
            Action::ControllerInfoUpdated(info) => {
                self.state.controller_info = info;
                self.state.last_data_update = Some(Instant::now());
            }

            // K8s client initialization
            Action::K8sClientReady { client, context_name } => {
                self.client = Some(client.0);
                self.state.context_name = context_name;
                self.state.connection_error = None;
            }
            Action::ConnectionError(msg) => {
                self.state.connection_error = Some(msg);
            }

            // CRD missing indicators
            Action::TerraformCrdMissing => {
                self.state.tf_crd_missing = true;
            }
            Action::KustomizationCrdMissing => {
                self.state.ks_crd_missing = true;
            }

            // Async K8s action results
            Action::K8sActionSuccess(msg) => {
                self.state.flash_message = Some((msg, Instant::now(), FlashKind::Success));
            }
            Action::K8sActionError(msg) => {
                self.state.flash_message =
                    Some((format!("Error: {}", msg), Instant::now(), FlashKind::Error));
            }

            Action::TerraformStoreUpdated => {
                self.state.tf_synced = true;
                self.state.last_data_update = Some(Instant::now());
            }
            Action::KustomizationStoreUpdated => {
                self.state.ks_synced = true;
                self.state.last_data_update = Some(Instant::now());
            }
            Action::Resize(_, _)
            | Action::None => {}

            _ => {}
        }
    }

    fn require_client(&self) -> Option<kube::Client> {
        self.client.clone()
    }

    fn spawn_detail_outputs_fetch(&self, ns: &str, name: &str) {
        let Some(client) = self.require_client() else { return };
        let tx = self.action_tx.clone();
        let ns = ns.to_string();
        let name = name.to_string();
        tokio::spawn(async move {
            if let Ok(values) = k8s_actions::fetch_output_values(&client, &ns, &name).await {
                let _ = tx.send(Action::DetailOutputsFetched {
                    namespace: ns,
                    name,
                    values,
                });
            }
        });
    }

    fn spawn_k8s_action(&self, action: Action) {
        let client = match self.require_client() {
            Some(c) => c,
            None => {
                let _ = self.action_tx.send(Action::K8sActionError("K8s client not ready yet".to_string()));
                return;
            }
        };
        let tx = self.action_tx.clone();
        let success_msg = format_success_message(&action);

        tokio::spawn(async move {
            let result = execute_k8s_action(&client, &action).await;
            match result {
                Ok(()) => {
                    let _ = tx.send(Action::K8sActionSuccess(success_msg));
                }
                Err(e) => {
                    let _ = tx.send(Action::K8sActionError(format!("{}", e)));
                }
            }
        });
    }

    fn jump_to_first_failure(&mut self) {
        match self.state.active_tab {
            TabKind::Terraform => {
                let items = get_filtered_terraforms(
                    &self.state.tf_store,
                    &self.state.namespace_filter,
                    &self.state.search_query,
                    self.state.show_failures_only,
                    self.state.show_waiting_only,
                    self.state.sort_column,
                    self.state.sort_descending,
                );
                for (i, tf) in items.iter().enumerate() {
                    let is_ready = tf
                        .status
                        .as_ref()
                        .and_then(|s| s.conditions.as_ref())
                        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                        .map(|c| c.status == "True")
                        .unwrap_or(false);
                    if !is_ready {
                        self.state.tf_table_state.select(Some(i));
                        return;
                    }
                }
            }
            TabKind::Kustomizations => {
                let items = get_filtered_kustomizations(
                    &self.state.ks_store,
                    &self.state.namespace_filter,
                    &self.state.search_query,
                    self.state.show_failures_only,
                    self.state.show_waiting_only,
                    self.state.sort_column,
                    self.state.sort_descending,
                );
                for (i, ks) in items.iter().enumerate() {
                    let is_ready = ks
                        .status
                        .as_ref()
                        .and_then(|s| s.conditions.as_ref())
                        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                        .map(|c| c.status == "True")
                        .unwrap_or(false);
                    if !is_ready {
                        self.state.ks_table_state.select(Some(i));
                        return;
                    }
                }
            }
            _ => {}
        }
    }

    fn execute_bulk_action<F>(&self, make_action: F)
    where
        F: Fn(String, String, ResourceKind) -> Action,
    {
        let kind = match self.state.active_tab {
            TabKind::Terraform => ResourceKind::Terraform,
            TabKind::Kustomizations => ResourceKind::Kustomization,
            _ => return,
        };
        for (ns, name) in &self.state.bulk_selected {
            let action = make_action(ns.clone(), name.clone(), kind.clone());
            self.spawn_k8s_action(action);
        }
    }

    fn save_viewer_content(&mut self) {
        let content = match self.state.current_view() {
            ViewState::PlanViewer { content } => Some(("plan", content.clone())),
            ViewState::JsonViewer { content } => Some(("json", content.clone())),
            ViewState::EventsViewer { content } => Some(("events", content.clone())),
            ViewState::OutputsViewer { content } => Some(("outputs", content.clone())),
            ViewState::LogViewer { content, .. } => Some(("logs", content.clone())),
            _ => None,
        };
        if let Some((prefix, text)) = content {
            let timestamp = jiff::Timestamp::now().strftime("%Y%m%d_%H%M%S");
            let filename = format!("terrarium_{}_{}.txt", prefix, timestamp);
            match create_private_file(&filename) {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(text.as_bytes()) {
                        self.state.flash_message = Some((
                            format!("Write error: {}", e),
                            Instant::now(),
                            FlashKind::Error,
                        ));
                    } else {
                        self.state.flash_message = Some((
                            format!("Saved to {}", filename),
                            Instant::now(),
                            FlashKind::Success,
                        ));
                    }
                }
                Err(e) => {
                    self.state.flash_message = Some((
                        format!("Save error: {}", e),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                }
            }
        }
    }

    fn open_shortcut(&mut self, namespace: &str, name: &str, shortcut_idx: usize) {
        let shortcut = match self.state.config.shortcuts.get(shortcut_idx) {
            Some(s) => s.clone(),
            None => return,
        };

        // Resolve template variables
        let mut url = shortcut.url.clone();
        url = url.replace("{context}", &self.state.context_name);
        url = url.replace("{namespace}", namespace);
        url = url.replace("{name}", name);

        // Resolve {output.KEY} from cached outputs
        if url.contains("{output.") {
            if let Some(((cached_ns, cached_name), outputs)) = &self.state.cached_outputs {
                if cached_ns == namespace && cached_name == name {
                    // Replace all {output.KEY} patterns
                    while let Some(start) = url.find("{output.") {
                        if let Some(end) = url[start..].find('}') {
                            let key = &url[start + 8..start + end];
                            let value = outputs.get(key).map(|s| s.as_str()).unwrap_or("");
                            let placeholder = format!("{{output.{}}}", key);
                            url = url.replace(&placeholder, value);
                        } else {
                            break;
                        }
                    }
                } else {
                    self.state.flash_message = Some((
                        "Open detail view first to load outputs".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                    return;
                }
            } else {
                self.state.flash_message = Some((
                    "Open detail view first to load outputs".to_string(),
                    Instant::now(),
                    FlashKind::Error,
                ));
                return;
            }
        }

        // Open in browser
        #[cfg(target_os = "macos")]
        let cmd = "open";
        #[cfg(target_os = "linux")]
        let cmd = "xdg-open";
        #[cfg(target_os = "windows")]
        let cmd = "start";

        match std::process::Command::new(cmd).arg(&url).spawn() {
            Ok(_) => {
                self.state.flash_message = Some((
                    format!("Opened {}", shortcut.label),
                    Instant::now(),
                    FlashKind::Success,
                ));
            }
            Err(e) => {
                self.state.flash_message = Some((
                    format!("Failed to open browser: {}", e),
                    Instant::now(),
                    FlashKind::Error,
                ));
            }
        }
    }

    async fn exec_break_the_glass(
        &mut self,
        terminal: &mut crate::tui::Tui,
        namespace: &str,
        name: &str,
    ) {
        // Just delegate to tfctl — it handles the entire BTG lifecycle correctly.
        // Trying to reimplement its K8s patch logic has proven unreliable.
        self.state.flash_message = Some((
            format!("Launching tfctl break-glass {} -n {}...", name, namespace),
            Instant::now(),
            FlashKind::Success,
        ));
        terminal.draw(|f| layout::render(f, &mut self.state)).ok();

        // Suspend TUI
        if let Err(e) = crate::tui::restore() {
            self.state.flash_message = Some((
                format!("Failed to suspend TUI: {}", e),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        }

        // Run tfctl break-glass — it handles: annotation, wait, exec, cleanup
        let status = std::process::Command::new("tfctl")
            .args(["break-glass", name, "-n", namespace])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status();

        // Restore TUI
        if let Err(e) = crate::tui::init_raw(self.state.mouse_enabled) {
            eprintln!("Failed to restore TUI: {}", e);
            self.should_quit = true;
            return;
        }
        terminal.clear().ok();

        match status {
            Ok(s) if s.success() => {
                self.state.flash_message = Some((
                    format!("BTG session ended for {}/{}", namespace, name),
                    Instant::now(),
                    FlashKind::Success,
                ));
            }
            Ok(s) => {
                self.state.flash_message = Some((
                    format!("tfctl exited with code {}", s.code().unwrap_or(-1)),
                    Instant::now(),
                    FlashKind::Error,
                ));
            }
            Err(e) => {
                self.state.flash_message = Some((
                    format!("Failed to run tfctl: {} — is tfctl installed?", e),
                    Instant::now(),
                    FlashKind::Error,
                ));
            }
        }
    }

    async fn start_log_stream(&mut self, namespace: &str, name: &str) {
        // Cancel any existing stream
        self.cancel_log_stream();

        // Get container list from the pod
        let containers = self
            .state
            .runner_pods
            .iter()
            .find(|p| {
                p.metadata.namespace.as_deref() == Some(namespace)
                    && p.metadata.name.as_deref() == Some(name)
            })
            .map(k8s_actions::get_container_names)
            .unwrap_or_default();

        let first_container = containers.first().cloned();

        // Push the log viewer immediately with empty content
        self.state.view_stack.push(ViewState::LogViewer {
            namespace: namespace.to_string(),
            pod_name: name.to_string(),
            containers: containers.clone(),
            active_container: 0,
            content: String::new(),
        });
        self.state.plan_scroll = 0;
        self.state.viewer_wrap = false;
        self.state.log_auto_follow = true;

        // Start streaming
        let Some(client) = self.require_client() else {
                    self.state.flash_message = Some(("K8s client not ready yet".to_string(), Instant::now(), FlashKind::Error));
                    return;
                };
        let tx = self.action_tx.clone();
        let ns = namespace.to_string();
        let pod_name = name.to_string();
        // Strip "init:" prefix for the API call
        let api_container = first_container.map(|c| {
            c.strip_prefix("init:").unwrap_or(&c).to_string()
        });

        let handle = tokio::spawn(async move {
            if let Err(e) = k8s_actions::stream_pod_logs(
                &client,
                &ns,
                &pod_name,
                api_container.as_deref(),
                tx,
            )
            .await
            {
                tracing::debug!("Log stream ended: {}", e);
            }
        });
        self.state.log_stream_handle = Some(handle);
    }

    async fn switch_log_container(&mut self, direction: isize) {
        let (namespace, pod_name, containers, active) =
            if let ViewState::LogViewer {
                namespace,
                pod_name,
                containers,
                active_container,
                ..
            } = self.state.current_view().clone()
            {
                (namespace, pod_name, containers, active_container)
            } else {
                return;
            };

        if containers.len() <= 1 {
            return;
        }

        let new_idx = if direction > 0 {
            (active + 1) % containers.len()
        } else {
            (active + containers.len() - 1) % containers.len()
        };
        let new_container = containers[new_idx].clone();

        // Cancel existing stream and pop the current log viewer
        self.cancel_log_stream();
        self.state.view_stack.pop();

        // Push new log viewer with empty content
        self.state.view_stack.push(ViewState::LogViewer {
            namespace: namespace.clone(),
            pod_name: pod_name.clone(),
            containers: containers.clone(),
            active_container: new_idx,
            content: String::new(),
        });
        self.state.plan_scroll = 0;
        self.state.log_auto_follow = true;

        // Start new stream
        let Some(client) = self.require_client() else {
                    self.state.flash_message = Some(("K8s client not ready yet".to_string(), Instant::now(), FlashKind::Error));
                    return;
                };
        let tx = self.action_tx.clone();
        let api_container = new_container
            .strip_prefix("init:")
            .unwrap_or(&new_container)
            .to_string();

        let handle = tokio::spawn(async move {
            if let Err(e) = k8s_actions::stream_pod_logs(
                &client,
                &namespace,
                &pod_name,
                Some(&api_container),
                tx,
            )
            .await
            {
                tracing::debug!("Log stream ended: {}", e);
            }
        });
        self.state.log_stream_handle = Some(handle);
    }
}

async fn execute_k8s_action(client: &kube::Client, action: &Action) -> anyhow::Result<()> {
    match action {
        Action::ApprovePlan { namespace, name } => {
            k8s_actions::approve_plan(client, namespace, name).await
        }
        Action::Reconcile {
            kind,
            namespace,
            name,
        } => match kind {
            ResourceKind::Terraform => {
                k8s_actions::force_reconcile(client, namespace, name).await
            }
            ResourceKind::Kustomization => {
                k8s_actions::reconcile_kustomization(client, namespace, name).await
            }
        },
        Action::Replan { namespace, name } => {
            k8s_actions::replan(client, namespace, name).await
        }
        Action::Suspend {
            kind,
            namespace,
            name,
        } => match kind {
            ResourceKind::Terraform => {
                k8s_actions::suspend(client, namespace, name).await
            }
            ResourceKind::Kustomization => {
                k8s_actions::suspend_kustomization(client, namespace, name).await
            }
        },
        Action::Resume {
            kind,
            namespace,
            name,
        } => match kind {
            ResourceKind::Terraform => {
                k8s_actions::resume(client, namespace, name).await
            }
            ResourceKind::Kustomization => {
                k8s_actions::resume_kustomization(client, namespace, name).await
            }
        },
        Action::ForceUnlock { namespace, name } => {
            k8s_actions::force_unlock(client, namespace, name).await
        }
        Action::DeleteResource { namespace, name } => {
            k8s_actions::delete_terraform(client, namespace, name).await
        }
        Action::KillRunner { namespace, name } => {
            k8s_actions::delete_pod(client, namespace, name).await
        }
        _ => Ok(()),
    }
}

fn format_success_message(action: &Action) -> String {
    match action {
        Action::ApprovePlan { namespace, name } => {
            format!("Approved plan for {}/{}", namespace, name)
        }
        Action::Reconcile { namespace, name, .. } => {
            format!("Triggered reconciliation for {}/{}", namespace, name)
        }
        Action::Replan { namespace, name } => {
            format!("Triggered replan for {}/{}", namespace, name)
        }
        Action::Suspend { namespace, name, .. } => format!("Suspended {}/{}", namespace, name),
        Action::Resume { namespace, name, .. } => format!("Resumed {}/{}", namespace, name),
        Action::ForceUnlock { namespace, name } => {
            format!("Force unlocked {}/{}", namespace, name)
        }
        Action::DeleteResource { namespace, name } => {
            format!("Deleted {}/{}", namespace, name)
        }
        Action::KillRunner { namespace, name } => {
            format!("Killed runner {}/{}", namespace, name)
        }
        _ => "Action completed".to_string(),
    }
}

/// Create a file with restrictive permissions (owner-only read/write).
#[cfg(unix)]
fn create_private_file(path: &str) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &str) -> std::io::Result<std::fs::File> {
    std::fs::File::create(path)
}
