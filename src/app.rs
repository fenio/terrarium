use std::io::Write;
use std::time::Instant;

use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;

use crate::action::{
    Action, AsyncIdentity, AsyncKind, AsyncScope, AsyncTarget, MutationTarget, ResourceKind,
};
use crate::k8s::actions as k8s_actions;
use crate::k8s::metrics;
use crate::keys::handle_key;
use crate::state::store::{
    AppState, DialogState, FlashKind, InputMode, SelectionIdentity, TabKind, ViewState,
};
use crate::ui::custom_tab::get_filtered_entries;
use crate::ui::kustomization_list::get_filtered_kustomizations;
use crate::ui::layout;
use crate::ui::resource_list::get_filtered_terraforms;
use crate::ui::runner_list::get_filtered_runners;

pub struct App {
    pub state: AppState,
    action_tx: mpsc::UnboundedSender<Action>,
    action_rx: mpsc::UnboundedReceiver<Action>,
    client: Option<kube::Client>,
    should_quit: bool,

    /// In-memory kubeconfig used by the context switcher. When present
    /// (e.g. assembled by a `[switcher] builder` command), clients are
    /// built from it; `None` falls back to on-disk kubeconfig resolution.
    switcher_kubeconfig: Option<kube::config::Kubeconfig>,
    /// On-disk copy of `switcher_kubeconfig`, so subprocesses (tfctl) can
    /// see the same contexts. Passed as `KUBECONFIG` to those children.
    /// Removed on drop. `None` when there's no builder.
    switcher_kubeconfig_path: Option<std::path::PathBuf>,
    /// CLI-derived settings replayed on every (re)connect.
    namespace: Option<String>,
    controller_ns: String,
    tf_debug_log: Option<std::path::PathBuf>,
    /// Handles for the current connection's background tasks (client
    /// init + watchers/pollers). Aborted and replaced on every reconnect
    /// so a context switch never leaves watchers pointed at the old
    /// cluster writing into orphaned stores.
    conn_tasks: std::sync::Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// True while the initial context picker is waiting for a choice. Escape
    /// exits without connecting; the normal in-app picker keeps its usual
    /// cancel behavior.
    initial_context_picker: bool,
}

impl Drop for App {
    fn drop(&mut self) {
        // Best-effort cleanup of the on-disk switcher kubeconfig.
        if let Some(path) = &self.switcher_kubeconfig_path {
            let _ = std::fs::remove_file(path);
        }
    }
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
            switcher_kubeconfig: None,
            switcher_kubeconfig_path: None,
            namespace: None,
            controller_ns: "flux-system".to_string(),
            tf_debug_log: None,
            conn_tasks: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            initial_context_picker: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_deferred(
        state: AppState,
        action_tx: mpsc::UnboundedSender<Action>,
        action_rx: mpsc::UnboundedReceiver<Action>,
        switcher_kubeconfig: Option<kube::config::Kubeconfig>,
        switcher_kubeconfig_path: Option<std::path::PathBuf>,
        namespace: Option<String>,
        controller_ns: String,
        tf_debug_log: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            state,
            action_tx,
            action_rx,
            client: None,
            should_quit: false,
            switcher_kubeconfig,
            switcher_kubeconfig_path,
            namespace,
            controller_ns,
            tf_debug_log,
            conn_tasks: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            initial_context_picker: false,
        }
    }

    /// Show the context picker before the first connection when a kubeconfig
    /// contains several contexts and no explicit `--context` was supplied.
    pub fn begin_initial_context_selection(&mut self, contexts: Vec<String>) {
        if contexts.is_empty() {
            return;
        }
        self.state.ctx_picker_selected = contexts
            .iter()
            .position(|context| context == &self.state.context_name)
            .unwrap_or(0);
        self.state.ctx_picker_items = contexts;
        self.state.input_mode = InputMode::ContextPicker;
        self.initial_context_picker = true;
    }

    /// (Re)connect the app to `context`, or the switcher kubeconfig's
    /// current-context when `None`. Aborts the previous connection's
    /// background tasks, swaps in fresh reflector stores, and spawns a
    /// new client-init + watcher/poller fleet. Rendering continues
    /// immediately; data refills as the new watchers sync.
    pub fn connect(&mut self, context: Option<String>) {
        // Abort the previous connection's tasks (no-op on first connect).
        if let Ok(mut tasks) = self.conn_tasks.lock() {
            for h in tasks.drain(..) {
                h.abort();
            }
        }

        // Fresh stores/writers for this connection generation. A Writer is
        // permanently bound to its Store, so each connection needs its own
        // pair; the read handles are swapped into AppState here and the
        // writers move into the watchers spawned below.
        let (tf_store, tf_writer) = crate::k8s::watcher::create_tf_store();
        let (ks_store, ks_writer) = crate::k8s::watcher::create_ks_store();
        let (gr_store, gr_writer) = crate::k8s::watcher::create_gitrepo_store();
        self.state.reset_for_reconnect(tf_store, ks_store, gr_store);
        self.client = None;

        let tasks_arc = self.conn_tasks.clone();
        let tx = self.action_tx.clone();
        let connection_tx =
            AsyncScope::generation(self.state.connection_generation, AsyncKind::Connection)
                .sender(tx.clone());
        let kubeconfig = self.switcher_kubeconfig.clone();
        let namespace = self.namespace.clone();
        let controller_ns = self.controller_ns.clone();
        let tf_debug_log = self.tf_debug_log.clone();

        let outer = tokio::spawn(async move {
            let client_res = match kubeconfig {
                Some(kc) => {
                    crate::k8s::client::create_client_from_kubeconfig(kc, context.as_deref()).await
                }
                None => crate::k8s::client::create_client(context.as_deref()).await,
            };

            match client_res {
                Ok((client, cluster_info)) => {
                    let _ = connection_tx.send(Action::K8sClientReady {
                        client: crate::action::K8sClient(client.clone()),
                        context_name: cluster_info.context_name,
                    });

                    // Register each spawned task so a later reconnect can
                    // abort it. `push` locks the shared vec per spawn.
                    let push = |h: tokio::task::JoinHandle<()>| {
                        if let Ok(mut t) = tasks_arc.lock() {
                            t.push(h);
                        }
                    };

                    let wtx = connection_tx.clone();
                    let c = client.clone();
                    let dbg = tf_debug_log.clone();
                    push(tokio::spawn(async move {
                        if let Err(e) =
                            crate::k8s::watcher::run_tf_watcher(c, tf_writer, wtx.clone(), dbg)
                                .await
                        {
                            let _ = wtx.send(Action::ConnectionError(format!(
                                "Terraform watcher failed: {e}"
                            )));
                        }
                    }));

                    let wtx = connection_tx.clone();
                    let c = client.clone();
                    push(tokio::spawn(async move {
                        if let Err(e) =
                            crate::k8s::watcher::run_ks_watcher(c, ks_writer, wtx.clone()).await
                        {
                            let _ = wtx.send(Action::ConnectionError(format!(
                                "Kustomization watcher failed: {e}"
                            )));
                        }
                    }));

                    let wtx = connection_tx.clone();
                    let c = client.clone();
                    push(tokio::spawn(async move {
                        if let Err(e) =
                            crate::k8s::watcher::run_gitrepo_watcher(c, gr_writer, wtx.clone())
                                .await
                        {
                            let _ = wtx.send(Action::ConnectionError(format!(
                                "GitRepository watcher failed: {e}"
                            )));
                        }
                    }));

                    let wtx = connection_tx.clone();
                    let c = client.clone();
                    let ns_clone = namespace.clone();
                    push(tokio::spawn(async move {
                        if let Err(e) =
                            crate::k8s::runners::poll_runner_pods(c, wtx.clone(), ns_clone).await
                        {
                            let _ = wtx.send(Action::ConnectionError(format!(
                                "Runner poller failed: {e}"
                            )));
                        }
                    }));

                    let wtx = connection_tx.clone();
                    let c = client.clone();
                    push(tokio::spawn(async move {
                        if let Err(e) = crate::k8s::controller::poll_controller_info(
                            c,
                            wtx.clone(),
                            controller_ns,
                        )
                        .await
                        {
                            let _ = wtx.send(Action::ConnectionError(format!(
                                "Controller poller failed: {e}"
                            )));
                        }
                    }));
                }
                Err(e) => {
                    let _ = connection_tx.send(Action::ConnectionError(format!(
                        "Failed to connect to cluster: {e}"
                    )));
                }
            }
        });

        if let Ok(mut tasks) = self.conn_tasks.lock() {
            tasks.push(outer);
        }
    }

    /// Switch the app to `context`, first suspending the TUI to run an
    /// interactive `exec`/OIDC credential plugin on a clean terminal when
    /// the target context uses one — so a browser login never garbles the
    /// alternate screen. Then reconnect (see [`Self::connect`]). No-op
    /// suspend for token/cert contexts.
    async fn switch_context(&mut self, terminal: &mut crate::tui::Tui, context: String) {
        let exec = self
            .switcher_kubeconfig
            .clone()
            .or_else(|| kube::config::Kubeconfig::read().ok())
            .and_then(|kc| crate::k8s::exec_auth::exec_for_context(&kc, Some(&context)));

        let mut auth_error: Option<String> = None;
        if let Some(exec) = exec {
            self.state.flash_message = Some((
                format!("Authenticating to {context} …"),
                Instant::now(),
                FlashKind::Success,
            ));
            terminal.draw(|f| layout::render(f, &mut self.state)).ok();

            if crate::tui::restore().is_ok() {
                println!("\nAuthenticating to {context} … (a browser may open)\n");
                if let Err(e) = crate::k8s::exec_auth::prewarm(&exec) {
                    // Non-fatal: connect() still tries — kube-rs may already
                    // hold a usable cached token. Surface it back in the TUI.
                    auth_error = Some(format!("Pre-auth failed: {e}"));
                }
                if crate::tui::resume(terminal, self.state.mouse_enabled).is_err() {
                    self.should_quit = true;
                    return;
                }
            }
        }

        self.state.context_name = context.clone();
        self.connect(Some(context.clone()));
        self.state.flash_message = Some(match auth_error {
            Some(e) => (e, Instant::now(), FlashKind::Error),
            None => (
                format!("Switched to {context}"),
                Instant::now(),
                FlashKind::Success,
            ),
        });
    }

    /// Context names offered by the switcher, drawn from the switcher
    /// kubeconfig when present, else the on-disk kubeconfig. Ordered as
    /// they appear in the file.
    fn switcher_contexts(&self) -> Vec<String> {
        let kc = self
            .switcher_kubeconfig
            .clone()
            .or_else(|| kube::config::Kubeconfig::read().ok());
        kc.map(|kc| kc.contexts.into_iter().map(|c| c.name).collect())
            .unwrap_or_default()
    }

    pub async fn run(&mut self, terminal: &mut crate::tui::Tui) -> anyhow::Result<()> {
        let mut event_stream = EventStream::new();
        let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(250));
        let mut needs_redraw = true;

        loop {
            if needs_redraw {
                terminal.draw(|f| layout::render(f, &mut self.state))?;
            }

            // Wait for the first event (blocking).
            needs_redraw = tokio::select! {
                event = event_stream.next() => {
                    match event {
                        Some(Ok(evt)) => self
                            .run_terminal_event(evt, &mut event_stream, terminal)
                            .await?,
                        Some(Err(err)) => return Err(err.into()),
                        None => return Ok(()),
                    }
                }
                action = self.action_rx.recv() => {
                    match action {
                        Some(action) => self.run_action(action, terminal).await,
                        None => return Ok(()),
                    }
                }
                _ = tick_interval.tick() => {
                    self.state.expire_flash();
                    self.state.prune_recently_acted();
                    self.state.tick_count = self.state.tick_count.wrapping_add(1);
                    true
                }
            };

            if self.should_quit {
                break;
            }
        }

        Ok(())
    }

    /// Process terminal input while coalescing a bounded run of list cursor
    /// events. Key repeat can otherwise fill the input queue faster than a
    /// full render completes, leaving the UI visibly behind after key release.
    async fn run_terminal_event(
        &mut self,
        first_event: Event,
        event_stream: &mut EventStream,
        terminal: &mut crate::tui::Tui,
    ) -> anyhow::Result<bool> {
        let refresh_terminal = matches!(&first_event, Event::FocusGained);
        if let Some(first_delta) = self.list_navigation_delta(&first_event) {
            let mut deltas = vec![first_delta];
            let mut deferred_event = None;
            let mut idle_polls = 0;

            for _ in 0..256 {
                let Some(next) = event_stream.next().now_or_never() else {
                    if idle_polls >= 2 {
                        break;
                    }
                    idle_polls += 1;
                    tokio::task::yield_now().await;
                    continue;
                };
                let Some(next) = next else {
                    break;
                };
                let next = next?;
                idle_polls = 0;

                if let Some(delta) = self.list_navigation_delta(&next) {
                    deltas.push(delta);
                } else if matches!(
                    next,
                    Event::FocusGained
                        | Event::FocusLost
                        | Event::Key(crossterm::event::KeyEvent {
                            kind: KeyEventKind::Release,
                            ..
                        })
                ) {
                    // Release/focus notifications do not produce actions in
                    // the normal dispatcher and can be discarded while
                    // draining a repeat burst.
                    continue;
                } else {
                    deferred_event = Some(next);
                    break;
                }
            }

            self.apply_list_navigation(&deltas);
            if let Some(event) = deferred_event
                && let Some(action) = self.handle_crossterm_event(event)
            {
                self.run_action(action, terminal).await;
            }
            Ok(true)
        } else if let Some(action) = self.handle_crossterm_event(first_event) {
            Ok(self.run_action(action, terminal).await)
        } else {
            Ok(refresh_terminal)
        }
    }

    fn list_navigation_delta(&self, event: &Event) -> Option<isize> {
        if self.state.connection_error.is_some()
            || self.state.input_mode != InputMode::Normal
            || !matches!(self.state.current_view(), ViewState::List(_))
        {
            return None;
        }

        let Event::Key(key) = event else {
            return None;
        };
        if matches!(key.kind, KeyEventKind::Release) {
            return None;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => Some(1),
            KeyCode::Char('k') | KeyCode::Up => Some(-1),
            _ => None,
        }
    }

    fn apply_list_navigation(&mut self, deltas: &[isize]) {
        if deltas.is_empty() {
            return;
        }

        self.state.bump_view_revision();
        let tab = self.state.active_tab.clone();
        let identities = self.visible_selection_identities(&tab);
        for delta in deltas {
            self.state.move_selection(*delta, &identities);
        }
    }

    /// Route an action, intercepting the few that must run with access to
    /// the terminal handle (they temporarily suspend the TUI); everything
    /// else goes through the normal `dispatch`.
    async fn run_action(&mut self, action: Action, terminal: &mut crate::tui::Tui) -> bool {
        match action {
            Action::ExecBreakTheGlass { target } => {
                self.exec_break_the_glass(terminal, &target).await;
                true
            }
            Action::SwitchContext(context) => {
                self.switch_context(terminal, context).await;
                true
            }
            other => self.dispatch(other).await,
        }
    }

    fn handle_crossterm_event(&self, event: Event) -> Option<Action> {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                // Dismiss connection error overlay on any key (except Ctrl-C which quits)
                if self.state.connection_error.is_some() {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        return Some(Action::Quit);
                    }
                    return Some(Action::DismissError);
                }

                let action = handle_key(key, self.state.current_view(), &self.state.input_mode);

                // While the Shortcuts popup is open, route any unhandled
                // character key (handle_key already absorbs j/k/Enter/Esc)
                // through the configured shortcut table for direct
                // activation: pressing 'b' in the popup opens Grafana.
                if matches!(action, Action::None)
                    && self.state.input_mode == InputMode::ShortcutsPopup
                    && let KeyCode::Char(c) = key.code
                    && let Some((ns, nm)) = self.state.shortcuts_popup_resource.clone()
                    && let Some(idx) = self.state.resolve_shortcut_for(c, &ns, &nm)
                {
                    return Some(Action::OpenShortcut {
                        namespace: ns,
                        name: nm,
                        shortcut_idx: idx,
                    });
                }

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
            Event::Paste(text) => Some(if self.state.input_mode == InputMode::Search {
                Action::SearchPaste(text)
            } else {
                Action::None
            }),
            Event::Mouse(mouse) if self.state.mouse_enabled => self.handle_mouse_event(mouse),
            Event::Resize(w, h) => Some(Action::Resize(w, h)),
            _ => None,
        }
    }

    fn handle_mouse_event(&self, mouse: crossterm::event::MouseEvent) -> Option<Action> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Tab bar's middle row is row 5 (0-indexed): the version
                // line now occupies row 3.
                if mouse.row == 5
                    && let Some((tab_idx, _)) = self
                        .state
                        .tab_hit_ranges
                        .iter()
                        .enumerate()
                        .find(|(_, (start, end))| mouse.column >= *start && mouse.column < *end)
                {
                    return Some(Action::GoToTab(tab_idx));
                }
                // List tables start after their border, column header, and
                // one-row header bottom margin.
                let first_data_row = self.state.body_y.saturating_add(3);
                if matches!(self.state.current_view(), ViewState::List(_))
                    && mouse.row >= first_data_row
                {
                    let row_idx =
                        self.current_table_offset() + (mouse.row - first_data_row) as usize;
                    return Some(Action::MouseSelect(row_idx));
                }
                None
            }
            MouseEventKind::ScrollDown => Some(Action::SelectNext),
            MouseEventKind::ScrollUp => Some(Action::SelectPrev),
            _ => None,
        }
    }

    fn current_table_offset(&self) -> usize {
        match &self.state.active_tab {
            TabKind::Controller => self.state.backlog_table_state.offset(),
            TabKind::Terraform => self.state.tf_table_state.offset(),
            TabKind::Kustomizations => self.state.ks_table_state.offset(),
            TabKind::Runners => self.state.runner_table_state.offset(),
            TabKind::CustomTab(i) => self
                .state
                .custom_tab_states
                .get(*i)
                .map_or(0, ratatui::widgets::TableState::offset),
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

    /// Map a keypress to the right bulk action for the Terraform tab.
    /// Returns `None` for keys that don't have a bulk equivalent,
    /// letting the per-row resolver handle them as a fallback.
    fn resolve_bulk_tf_action(&self, code: KeyCode) -> Option<Action> {
        let n = self.state.bulk_selected.len();
        let targets = self.bulk_target_names();
        match code {
            KeyCode::Char('r') => Some(Action::ShowConfirmDialog(
                Box::new(Action::BulkReconcile),
                format!("Reconcile {n} selected Terraform resource(s)?\n{targets}"),
            )),
            KeyCode::Char('s') => Some(Action::ShowConfirmDialog(
                Box::new(Action::BulkSuspend),
                format!("Suspend {n} selected Terraform resource(s)?\n{targets}"),
            )),
            KeyCode::Char('u') => Some(Action::ShowConfirmDialog(
                Box::new(Action::BulkResume),
                format!("Resume {n} selected Terraform resource(s)?\n{targets}"),
            )),
            KeyCode::Char('a') => Some(Action::ShowConfirmDialog(
                Box::new(Action::BulkApprovePlan),
                format!("Approve plans for {n} selected Terraform resource(s)?\n{targets}"),
            )),
            _ => None,
        }
    }

    fn bulk_target_names(&self) -> String {
        let mut names: Vec<String> = self
            .state
            .bulk_selected
            .values()
            .map(|target| format!("{}/{}", target.namespace, target.name))
            .collect();
        names.sort();
        names.join(", ")
    }

    fn mutation_target_for_terraform(&self, namespace: &str, name: &str) -> Option<MutationTarget> {
        let tf = self
            .state
            .tf_store
            .get(&kube::runtime::reflector::ObjectRef::new(name).within(namespace))?;
        mutation_target_from_metadata(
            ResourceKind::Terraform,
            namespace,
            name,
            &tf.metadata.uid,
            &tf.metadata.resource_version,
        )
    }

    fn mutation_target_for_kustomization(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<MutationTarget> {
        let ks = self
            .state
            .ks_store
            .get(&kube::runtime::reflector::ObjectRef::new(name).within(namespace))?;
        mutation_target_from_metadata(
            ResourceKind::Kustomization,
            namespace,
            name,
            &ks.metadata.uid,
            &ks.metadata.resource_version,
        )
    }

    fn selected_mutation_target(&self) -> Option<MutationTarget> {
        match self.state.active_tab {
            TabKind::Terraform => {
                let (namespace, name) = self.get_selected_terraform()?;
                self.mutation_target_for_terraform(&namespace, &name)
            }
            TabKind::CustomTab(index) => {
                let (namespace, name) = self.get_selected_custom_tab(index)?;
                self.mutation_target_for_terraform(&namespace, &name)
            }
            TabKind::Kustomizations => {
                let (namespace, name) = self.get_selected_kustomization()?;
                self.mutation_target_for_kustomization(&namespace, &name)
            }
            TabKind::Controller | TabKind::Runners => None,
        }
    }

    /// Map a keypress to the right bulk action for the Kustomization tab.
    fn resolve_bulk_ks_action(&self, code: KeyCode) -> Option<Action> {
        let n = self.state.bulk_selected.len();
        let targets = self.bulk_target_names();
        match code {
            KeyCode::Char('r') => Some(Action::ShowConfirmDialog(
                Box::new(Action::BulkReconcile),
                format!("Reconcile {n} selected Kustomization(s)?\n{targets}"),
            )),
            KeyCode::Char('s') => Some(Action::ShowConfirmDialog(
                Box::new(Action::BulkSuspend),
                format!("Suspend {n} selected Kustomization(s)?\n{targets}"),
            )),
            KeyCode::Char('u') => Some(Action::ShowConfirmDialog(
                Box::new(Action::BulkResume),
                format!("Resume {n} selected Kustomization(s)?\n{targets}"),
            )),
            _ => None,
        }
    }

    /// Resolve Terraform context-dependent actions.
    fn resolve_tf_action(
        &self,
        code: KeyCode,
        ns: Option<&String>,
        name: Option<&String>,
    ) -> Option<Action> {
        // Bulk-aware: when there's a multi-select, r/s/u/a operate on
        // the selection instead of the row under the cursor. The
        // single-resource fall-through below covers the empty-selection
        // case and detail views (where bulk doesn't apply anyway).
        if matches!(self.state.current_view(), ViewState::List(_))
            && !self.state.bulk_selected.is_empty()
            && let Some(action) = self.resolve_bulk_tf_action(code)
        {
            return Some(action);
        }

        let (ns, name) = match (ns, name) {
            (Some(n), Some(nm)) => (n.clone(), nm.clone()),
            _ => self.get_selected_terraform()?,
        };

        let tf = self
            .state
            .tf_store
            .get(&kube::runtime::reflector::ObjectRef::new(&name).within(&ns));
        let mutation_target = tf.as_ref().and_then(|tf| {
            mutation_target_from_metadata(
                ResourceKind::Terraform,
                &ns,
                &name,
                &tf.metadata.uid,
                &tf.metadata.resource_version,
            )
        });
        let break_the_glass = tf
            .as_ref()
            .is_some_and(|tf| k8s_actions::break_the_glass_active(tf));
        // When destroyResourcesOnDeletion is set, deleting the Terraform object
        // makes tofu-controller run a destroy plan against the real managed
        // infrastructure -- the failure mode that wiped customer databases.
        let destroy_on_deletion = tf
            .as_ref()
            .and_then(|tf| tf.spec.destroy_resources_on_deletion)
            .unwrap_or(false);

        match code {
            KeyCode::Char('a') => Some(Action::ShowConfirmDialog(
                Box::new(Action::ApprovePlan {
                    target: mutation_target.clone()?,
                }),
                format!("Approve plan for {ns}/{name}?"),
            )),
            KeyCode::Char('r') => Some(Action::Reconcile {
                target: mutation_target.clone()?,
            }),
            KeyCode::Char('R') if self.state.tfctl_available => Some(Action::Replan {
                target: mutation_target.clone()?,
            }),
            KeyCode::Char('s') => Some(Action::ShowConfirmDialog(
                Box::new(Action::Suspend {
                    target: mutation_target.clone()?,
                }),
                format!("Suspend {ns}/{name}?"),
            )),
            KeyCode::Char('u') => Some(Action::Resume {
                target: mutation_target.clone()?,
            }),
            KeyCode::Char('p') => Some(Action::FetchPlan {
                namespace: ns,
                name,
                workspace: None,
            }),
            KeyCode::Char('F') => Some(Action::ShowConfirmDialog(
                Box::new(Action::ForceUnlock {
                    target: mutation_target.clone()?,
                }),
                format!("Force unlock state for {ns}/{name}?"),
            )),
            KeyCode::Char('C') if break_the_glass => Some(Action::ShowConfirmDialog(
                Box::new(Action::ResetBreakTheGlass {
                    target: mutation_target.clone()?,
                }),
                format!("Disable persistent break-the-glass mode for {ns}/{name}?"),
            )),
            KeyCode::Char('X') => Some(Action::ShowTypedConfirmDialog(
                Box::new(Action::RemoveFinalizers {
                    target: mutation_target.clone()?,
                }),
                format!(
                    "DANGER: remove ALL finalizers from Terraform {ns}/{name}? This bypasses controller cleanup and may orphan managed infrastructure. Type the resource name to confirm:"
                ),
                name.clone(),
            )),
            KeyCode::Char('d') => {
                // Deleting a Terraform object is not like killing a runner pod:
                // tofu-controller reacts to the deletion and (when
                // destroyResourcesOnDeletion is set) destroys the real managed
                // infrastructure. Require the operator to type the resource name
                // so this can't happen by muscle-memory "d, y".
                let message = if destroy_on_deletion {
                    format!(
                        "DANGER: {ns}/{name} has destroyResourcesOnDeletion=true. \
                         Deleting it will DESTROY the managed infrastructure (this cannot be undone). \
                         Type the resource name to confirm:"
                    )
                } else {
                    format!(
                        "DELETE Terraform object {ns}/{name}? tofu-controller may destroy its managed \
                         resources (this cannot be undone). Type the resource name to confirm:"
                    )
                };
                Some(Action::ShowTypedConfirmDialog(
                    Box::new(Action::DeleteResource {
                        target: mutation_target.clone()?,
                    }),
                    message,
                    name.clone(),
                ))
            }
            KeyCode::Char('y') => Some(Action::FetchJson {
                kind: ResourceKind::Terraform,
                namespace: ns.clone(),
                name: name.clone(),
            }),
            KeyCode::Char('Y') => Some(Action::FetchYaml {
                kind: ResourceKind::Terraform,
                namespace: ns.clone(),
                name: name.clone(),
            }),
            KeyCode::Char('e') => Some(Action::FetchEvents {
                kind: ResourceKind::Terraform,
                namespace: ns.clone(),
                name: name.clone(),
            }),
            KeyCode::Char('c') => Some(Action::ViewConditions {
                kind: ResourceKind::Terraform,
                namespace: ns,
                name,
            }),
            KeyCode::Char('O') => Some(Action::FetchOutputs {
                namespace: ns,
                name,
            }),
            KeyCode::Char('x') if self.state.tfctl_available => Some(Action::ExecBreakTheGlass {
                target: mutation_target?,
            }),
            KeyCode::Char('L') => Some(Action::StreamRunnerLogs {
                namespace: ns,
                name,
            }),
            KeyCode::Char('S') => {
                if self.state.config.shortcuts.is_empty() {
                    None
                } else {
                    Some(Action::OpenShortcutsPopup {
                        namespace: ns,
                        name,
                    })
                }
            }
            KeyCode::Char(c) => {
                self.state
                    .resolve_shortcut_for(c, &ns, &name)
                    .map(|idx| Action::OpenShortcut {
                        namespace: ns.clone(),
                        name: name.clone(),
                        shortcut_idx: idx,
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
        // Bulk-aware: see resolve_tf_action for the rationale.
        if matches!(self.state.current_view(), ViewState::List(_))
            && !self.state.bulk_selected.is_empty()
            && let Some(action) = self.resolve_bulk_ks_action(code)
        {
            return Some(action);
        }

        let (ns, name) = match (ns, name) {
            (Some(n), Some(nm)) => (n.clone(), nm.clone()),
            _ => self.get_selected_kustomization()?,
        };
        let ks = self
            .state
            .ks_store
            .get(&kube::runtime::reflector::ObjectRef::new(&name).within(&ns));
        let mutation_target = ks.as_ref().and_then(|ks| {
            mutation_target_from_metadata(
                ResourceKind::Kustomization,
                &ns,
                &name,
                &ks.metadata.uid,
                &ks.metadata.resource_version,
            )
        });
        let prune = ks.as_ref().is_some_and(|ks| ks.spec.prune);

        match code {
            KeyCode::Char('r') => Some(Action::Reconcile {
                target: mutation_target.clone()?,
            }),
            KeyCode::Char('s') => Some(Action::ShowConfirmDialog(
                Box::new(Action::Suspend {
                    target: mutation_target.clone()?,
                }),
                format!("Suspend {ns}/{name}?"),
            )),
            KeyCode::Char('u') => Some(Action::Resume {
                target: mutation_target?,
            }),
            KeyCode::Char('d') => {
                let message = if prune {
                    format!(
                        "DANGER: {ns}/{name} has prune=true. Deleting it may garbage-collect its managed resources. \
                         Type the resource name to confirm:"
                    )
                } else {
                    format!(
                        "DELETE Kustomization object {ns}/{name}? Its managed resources may remain. \
                         Type the resource name to confirm:"
                    )
                };
                Some(Action::ShowTypedConfirmDialog(
                    Box::new(Action::DeleteResource {
                        target: mutation_target.clone()?,
                    }),
                    message,
                    name.clone(),
                ))
            }
            KeyCode::Char('y') => Some(Action::FetchJson {
                kind: ResourceKind::Kustomization,
                namespace: ns.clone(),
                name: name.clone(),
            }),
            KeyCode::Char('Y') => Some(Action::FetchYaml {
                kind: ResourceKind::Kustomization,
                namespace: ns.clone(),
                name: name.clone(),
            }),
            KeyCode::Char('e') => Some(Action::FetchEvents {
                kind: ResourceKind::Kustomization,
                namespace: ns.clone(),
                name: name.clone(),
            }),
            KeyCode::Char('c') => Some(Action::ViewConditions {
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
                Some(Action::StreamControllerLogs {
                    namespace: ns,
                    pod_name,
                })
            }
            _ => None,
        }
    }

    fn resolve_runner_action(&self, code: KeyCode) -> Option<Action> {
        let (ns, name) = self.get_selected_runner()?;
        let pod = self.state.runner_pods.iter().find(|pod| {
            pod.metadata.namespace.as_deref() == Some(ns.as_str())
                && pod.metadata.name.as_deref() == Some(name.as_str())
        })?;
        let mutation_target = mutation_target_from_metadata(
            ResourceKind::Pod,
            &ns,
            &name,
            &pod.metadata.uid,
            &pod.metadata.resource_version,
        );
        match code {
            KeyCode::Char('d') => Some(Action::ShowConfirmDialog(
                Box::new(Action::KillRunner {
                    target: mutation_target?,
                }),
                format!("Kill runner pod {ns}/{name}?"),
            )),
            KeyCode::Char('e') => Some(Action::FetchEvents {
                kind: ResourceKind::Pod,
                namespace: ns,
                name,
            }),
            KeyCode::Char('T') => {
                let tf_name = name.strip_suffix("-tf-runner").unwrap_or(&name).to_string();
                Some(Action::JumpToTerraformDetail {
                    namespace: ns,
                    name: tf_name,
                })
            }
            _ => None,
        }
    }

    fn get_selected_terraform(&self) -> Option<(String, String)> {
        let items = get_filtered_terraforms(
            &self.state.tf_store,
            &self.state.namespace_filter,
            self.state.effective_search_query(),
            self.state.show_failures_only,
            self.state.show_waiting_only,
            self.state.show_progressing_only,
            self.state.show_drifting_only,
            self.state.show_deleting_only,
            &self.state.recently_acted,
            self.state.sort_column,
            self.state.sort_descending,
        );
        let identities: Vec<SelectionIdentity> = items
            .iter()
            .map(|tf| {
                SelectionIdentity::new(
                    tf.metadata.namespace.clone().unwrap_or_default(),
                    tf.metadata.name.clone().unwrap_or_default(),
                    tf.metadata.uid.clone(),
                )
            })
            .collect();
        let selected_idx = self
            .state
            .selected_index_for(&TabKind::Terraform, &identities)?;
        let tf = items.get(selected_idx)?;
        Some((
            tf.metadata.namespace.clone().unwrap_or_default(),
            tf.metadata.name.clone().unwrap_or_default(),
        ))
    }

    fn get_selected_kustomization(&self) -> Option<(String, String)> {
        let items = get_filtered_kustomizations(
            &self.state.ks_store,
            &self.state.namespace_filter,
            self.state.effective_search_query(),
            self.state.show_failures_only,
            self.state.show_waiting_only,
            self.state.show_progressing_only,
            self.state.show_deleting_only,
            &self.state.recently_acted,
            self.state.sort_column,
            self.state.sort_descending,
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
        let selected_idx = self
            .state
            .selected_index_for(&TabKind::Kustomizations, &identities)?;
        let ks = items.get(selected_idx)?;
        Some((
            ks.metadata.namespace.clone().unwrap_or_default(),
            ks.metadata.name.clone().unwrap_or_default(),
        ))
    }

    fn get_selected_custom_tab(&self, tab_idx: usize) -> Option<(String, String)> {
        let tab_config = self.state.config.custom_tabs.get(tab_idx)?;
        let items = get_filtered_entries(
            &self.state.tf_store,
            &self.state.namespace_filter,
            self.state.effective_search_query(),
            tab_config,
        );
        let identities: Vec<SelectionIdentity> = items
            .iter()
            .map(|entry| {
                SelectionIdentity::new(
                    entry.namespace.clone(),
                    entry.name.clone(),
                    entry.uid.clone(),
                )
                .with_optional_row_key(entry.row_key.clone())
            })
            .collect();
        let selected_idx = self
            .state
            .selected_index_for(&TabKind::CustomTab(tab_idx), &identities)?;
        let entry = items.get(selected_idx)?;
        Some((entry.namespace.clone(), entry.name.clone()))
    }

    fn get_selected_runner(&self) -> Option<(String, String)> {
        let items = get_filtered_runners(
            &self.state.runner_pods,
            &self.state.namespace_filter,
            self.state.effective_search_query(),
            self.state.runner_sort_column,
            self.state.sort_descending,
        );
        let identities: Vec<SelectionIdentity> = items
            .iter()
            .map(|pod| {
                SelectionIdentity::new(
                    pod.metadata.namespace.clone().unwrap_or_default(),
                    pod.metadata.name.clone().unwrap_or_default(),
                    pod.metadata.uid.clone(),
                )
            })
            .collect();
        let selected_idx = self
            .state
            .selected_index_for(&TabKind::Runners, &identities)?;
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
                self.state.effective_search_query(),
                self.state.show_failures_only,
                self.state.show_waiting_only,
                self.state.show_progressing_only,
                self.state.show_drifting_only,
                self.state.show_deleting_only,
                &self.state.recently_acted,
                self.state.sort_column,
                self.state.sort_descending,
            )
            .len(),
            TabKind::Kustomizations => get_filtered_kustomizations(
                &self.state.ks_store,
                &self.state.namespace_filter,
                self.state.effective_search_query(),
                self.state.show_failures_only,
                self.state.show_waiting_only,
                self.state.show_progressing_only,
                self.state.show_deleting_only,
                &self.state.recently_acted,
                self.state.sort_column,
                self.state.sort_descending,
            )
            .len(),
            TabKind::Runners => get_filtered_runners(
                &self.state.runner_pods,
                &self.state.namespace_filter,
                self.state.effective_search_query(),
                self.state.runner_sort_column,
                self.state.sort_descending,
            )
            .len(),
            TabKind::CustomTab(i) => {
                if let Some(tab_config) = self.state.config.custom_tabs.get(i) {
                    get_filtered_entries(
                        &self.state.tf_store,
                        &self.state.namespace_filter,
                        self.state.effective_search_query(),
                        tab_config,
                    )
                    .len()
                } else {
                    0
                }
            }
        }
    }

    fn visible_selection_identities(&self, tab: &TabKind) -> Vec<SelectionIdentity> {
        match tab {
            TabKind::Controller => self
                .state
                .backlog_namespaces
                .iter()
                .map(|(namespace, _, _, _)| {
                    SelectionIdentity::new(namespace.clone(), String::new(), None)
                })
                .collect(),
            TabKind::Terraform => get_filtered_terraforms(
                &self.state.tf_store,
                &self.state.namespace_filter,
                self.state.effective_search_query(),
                self.state.show_failures_only,
                self.state.show_waiting_only,
                self.state.show_progressing_only,
                self.state.show_drifting_only,
                self.state.show_deleting_only,
                &self.state.recently_acted,
                self.state.sort_column,
                self.state.sort_descending,
            )
            .into_iter()
            .map(|tf| {
                SelectionIdentity::new(
                    tf.metadata.namespace.unwrap_or_default(),
                    tf.metadata.name.unwrap_or_default(),
                    tf.metadata.uid,
                )
            })
            .collect(),
            TabKind::Kustomizations => get_filtered_kustomizations(
                &self.state.ks_store,
                &self.state.namespace_filter,
                self.state.effective_search_query(),
                self.state.show_failures_only,
                self.state.show_waiting_only,
                self.state.show_progressing_only,
                self.state.show_deleting_only,
                &self.state.recently_acted,
                self.state.sort_column,
                self.state.sort_descending,
            )
            .into_iter()
            .map(|ks| {
                SelectionIdentity::new(
                    ks.metadata.namespace.unwrap_or_default(),
                    ks.metadata.name.unwrap_or_default(),
                    ks.metadata.uid,
                )
            })
            .collect(),
            TabKind::Runners => get_filtered_runners(
                &self.state.runner_pods,
                &self.state.namespace_filter,
                self.state.effective_search_query(),
                self.state.runner_sort_column,
                self.state.sort_descending,
            )
            .into_iter()
            .map(|pod| {
                SelectionIdentity::new(
                    pod.metadata.namespace.unwrap_or_default(),
                    pod.metadata.name.unwrap_or_default(),
                    pod.metadata.uid,
                )
            })
            .collect(),
            TabKind::CustomTab(index) => self
                .state
                .config
                .custom_tabs
                .get(*index)
                .map(|tab| {
                    get_filtered_entries(
                        &self.state.tf_store,
                        &self.state.namespace_filter,
                        self.state.effective_search_query(),
                        tab,
                    )
                    .into_iter()
                    .map(|entry| {
                        SelectionIdentity::new(entry.namespace, entry.name, entry.uid)
                            .with_optional_row_key(entry.row_key)
                    })
                    .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn select_list_index(&mut self, index: Option<usize>) {
        let tab = self.state.active_tab.clone();
        let identities = self.visible_selection_identities(&tab);
        self.state.set_selection_for_index(&tab, index, &identities);
    }

    fn current_list_selection_is_stale(&self) -> bool {
        let tab = self.state.active_tab.clone();
        let identities = self.visible_selection_identities(&tab);
        self.state.selection_is_stale_for(&tab, &identities)
    }

    fn move_list_selection(&mut self, delta: isize) {
        let tab = self.state.active_tab.clone();
        let identities = self.visible_selection_identities(&tab);
        self.state.move_selection(delta, &identities);
    }

    fn current_list_selection_index(&self) -> Option<usize> {
        let tab = self.state.active_tab.clone();
        let identities = self.visible_selection_identities(&tab);
        self.state.selected_index_for(&tab, &identities)
    }

    fn viewer_line_count(&self) -> usize {
        match self.state.current_view() {
            ViewState::PlanViewer { content }
            | ViewState::JsonViewer { content }
            | ViewState::EventsViewer { content }
            | ViewState::OutputsViewer { content }
            | ViewState::ConditionsViewer { content }
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
            ViewState::ConditionsViewer { content } => content.clone(),
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
        if let Some(&line) = self
            .state
            .viewer_search_matches
            .get(self.state.viewer_search_index)
        {
            self.state.plan_scroll = line;
        }
    }

    fn cancel_log_stream(&mut self) {
        if let Some(handle) = self.state.log_stream_handle.take() {
            handle.abort();
        }
        self.state.cancel_log_stream();
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
            self.state.invalidate_async_kind(AsyncKind::Metrics);
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
        let scope = self.state.begin_async(
            AsyncKind::Metrics,
            Some(AsyncTarget::Pod {
                namespace: namespace.clone(),
                name: String::new(),
            }),
            AsyncIdentity::None,
        );
        let tx = scope.sender(self.action_tx.clone());
        let handle = tokio::spawn(async move {
            // Fetch immediately, then every 5s.
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
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
                            .send(Action::MetricsFetchError(format!("{e:#}")))
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

    /// Validate asynchronous envelopes before handing their inner action to
    /// the normal dispatcher. This is the single gate that prevents delayed
    /// work from an old cluster, request, target, view, or log stream from
    /// mutating current state.
    async fn dispatch(&mut self, action: Action) -> bool {
        let action = match action {
            Action::Scoped(scoped) => {
                let list_target_current =
                    if matches!(
                        scoped.scope.kind,
                        AsyncKind::Plan
                            | AsyncKind::Resource
                            | AsyncKind::Outputs
                            | AsyncKind::Events
                            | AsyncKind::DetailOutputs
                            | AsyncKind::ShortcutOutputs
                            | AsyncKind::SecretValues
                    ) && matches!(self.state.current_view(), ViewState::List(_))
                    {
                        self.async_target_matches_selection(&scoped.scope)
                    } else {
                        true
                    };
                if !self.state.is_async_scope_current(&scoped.scope) || !list_target_current {
                    return false;
                }
                self.state.finish_async(&scoped.scope);
                *scoped.action
            }
            action => action,
        };
        if matches!(action, Action::None) {
            return false;
        }
        self.dispatch_inner(action).await;
        true
    }

    async fn dispatch_inner(&mut self, action: Action) {
        if matches!(
            &action,
            Action::SelectNext
                | Action::SelectPrev
                | Action::PageDown
                | Action::PageUp
                | Action::ScreenDown
                | Action::ScreenUp
                | Action::MouseSelect(_)
                | Action::Enter
                | Action::Back
                | Action::ToggleFailuresOnly
                | Action::ToggleWaitingOnly
                | Action::ToggleProgressingOnly
                | Action::ToggleDriftingOnly
                | Action::ToggleDeletingOnly
                | Action::SearchStart
                | Action::SearchPush(_)
                | Action::SearchPaste(_)
                | Action::SearchPop
                | Action::SearchConfirm
                | Action::SearchCancel
                | Action::ToggleSearchSuspend
                | Action::ToggleSelect
        ) {
            self.state.bump_view_revision();
        }

        match action {
            Action::Quit => self.should_quit = true,

            // Navigation. Tab switches preserve each tab's view stack —
            // including any open LogViewer with its live stream — so the
            // user can glance away and return without losing context.
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
                    self.state.show_progressing_only = false;
                    self.state.show_drifting_only = false;
                    self.state.show_deleting_only = false;
                }
                self.select_list_index(Some(0));
            }
            Action::ToggleProgressingOnly => {
                self.state.show_progressing_only = !self.state.show_progressing_only;
                if self.state.show_progressing_only {
                    self.state.show_failures_only = false;
                    self.state.show_waiting_only = false;
                    self.state.show_drifting_only = false;
                    self.state.show_deleting_only = false;
                }
                self.select_list_index(Some(0));
            }
            Action::ToggleDriftingOnly => {
                if self.state.active_tab == TabKind::Terraform {
                    self.state.show_drifting_only = !self.state.show_drifting_only;
                    if self.state.show_drifting_only {
                        self.state.show_failures_only = false;
                        self.state.show_waiting_only = false;
                        self.state.show_progressing_only = false;
                        self.state.show_deleting_only = false;
                    }
                    self.select_list_index(Some(0));
                }
            }
            Action::ToggleWaitingOnly => {
                self.state.show_waiting_only = !self.state.show_waiting_only;
                if self.state.show_waiting_only {
                    self.state.show_failures_only = false;
                    self.state.show_progressing_only = false;
                    self.state.show_drifting_only = false;
                    self.state.show_deleting_only = false;
                }
                self.select_list_index(Some(0));
            }
            Action::ToggleDeletingOnly => {
                self.state.show_deleting_only = !self.state.show_deleting_only;
                if self.state.show_deleting_only {
                    self.state.show_failures_only = false;
                    self.state.show_waiting_only = false;
                    self.state.show_progressing_only = false;
                    self.state.show_drifting_only = false;
                }
                self.select_list_index(Some(0));
            }
            Action::ToggleWrap => {
                self.state.viewer_wrap = !self.state.viewer_wrap;
            }
            Action::ToggleMouse => {
                self.state.mouse_enabled = !self.state.mouse_enabled;
                let _ = crate::tui::set_mouse_capture(self.state.mouse_enabled);
                let label = if self.state.mouse_enabled {
                    "Mouse enabled"
                } else {
                    "Mouse disabled"
                };
                self.state.flash_message = Some((
                    label.to_string(),
                    std::time::Instant::now(),
                    crate::state::store::FlashKind::Success,
                ));
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
                match self.state.active_tab {
                    TabKind::Runners => {
                        self.state.runner_sort_column = self.state.runner_sort_column.next();
                    }
                    _ => {
                        self.state.sort_column = self.state.sort_column.next();
                    }
                }
                self.state.bump_view_revision();
            }
            Action::InvertSort => {
                self.state.sort_descending = !self.state.sort_descending;
                self.state.bump_view_revision();
            }
            Action::ScrollTop => {
                if matches!(self.state.current_view(), ViewState::List(_)) {
                    self.select_list_index(Some(0));
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
                        self.select_list_index(Some(count - 1));
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
                if matches!(
                    self.state.current_view(),
                    ViewState::PlanViewer { .. }
                        | ViewState::JsonViewer { .. }
                        | ViewState::EventsViewer { .. }
                        | ViewState::OutputsViewer { .. }
                        | ViewState::ConditionsViewer { .. }
                        | ViewState::LogViewer { .. }
                ) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_add(half);
                } else {
                    let count = self.current_list_count();
                    if count > 0 {
                        let current = self.current_list_selection_index().unwrap_or(0);
                        self.select_list_index(Some((current + half).min(count - 1)));
                    }
                }
            }
            Action::PageUp => {
                let half = self.half_page();
                if matches!(
                    self.state.current_view(),
                    ViewState::PlanViewer { .. }
                        | ViewState::JsonViewer { .. }
                        | ViewState::EventsViewer { .. }
                        | ViewState::OutputsViewer { .. }
                        | ViewState::ConditionsViewer { .. }
                        | ViewState::LogViewer { .. }
                ) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_sub(half);
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.state.log_auto_follow = false;
                    }
                } else {
                    let current = self.current_list_selection_index().unwrap_or(0);
                    self.select_list_index(Some(current.saturating_sub(half)));
                }
            }
            Action::ScreenDown => {
                let page = (self.state.body_height as usize).max(1);
                if matches!(
                    self.state.current_view(),
                    ViewState::PlanViewer { .. }
                        | ViewState::JsonViewer { .. }
                        | ViewState::EventsViewer { .. }
                        | ViewState::OutputsViewer { .. }
                        | ViewState::ConditionsViewer { .. }
                        | ViewState::LogViewer { .. }
                ) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_add(page);
                } else {
                    let count = self.current_list_count();
                    if count > 0 {
                        let current = self.current_list_selection_index().unwrap_or(0);
                        self.select_list_index(Some((current + page).min(count - 1)));
                    }
                }
            }
            Action::ScreenUp => {
                let page = (self.state.body_height as usize).max(1);
                if matches!(
                    self.state.current_view(),
                    ViewState::PlanViewer { .. }
                        | ViewState::JsonViewer { .. }
                        | ViewState::EventsViewer { .. }
                        | ViewState::OutputsViewer { .. }
                        | ViewState::ConditionsViewer { .. }
                        | ViewState::LogViewer { .. }
                ) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_sub(page);
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.state.log_auto_follow = false;
                    }
                } else {
                    let current = self.current_list_selection_index().unwrap_or(0);
                    self.select_list_index(Some(current.saturating_sub(page)));
                }
            }
            Action::SelectNext => {
                if matches!(
                    self.state.current_view(),
                    ViewState::PlanViewer { .. }
                        | ViewState::JsonViewer { .. }
                        | ViewState::EventsViewer { .. }
                        | ViewState::OutputsViewer { .. }
                        | ViewState::ConditionsViewer { .. }
                        | ViewState::LogViewer { .. }
                ) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_add(1);
                } else {
                    self.move_list_selection(1);
                }
            }
            Action::SelectPrev => {
                if matches!(
                    self.state.current_view(),
                    ViewState::PlanViewer { .. }
                        | ViewState::JsonViewer { .. }
                        | ViewState::EventsViewer { .. }
                        | ViewState::OutputsViewer { .. }
                        | ViewState::ConditionsViewer { .. }
                        | ViewState::LogViewer { .. }
                ) {
                    self.state.plan_scroll = self.state.plan_scroll.saturating_sub(1);
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.state.log_auto_follow = false;
                    }
                } else {
                    self.move_list_selection(-1);
                }
            }
            Action::MouseSelect(row_idx) => {
                let count = self.current_list_count();
                if count > 0 {
                    self.select_list_index(Some(row_idx.min(count - 1)));
                }
            }
            Action::Enter => match self.state.active_tab {
                TabKind::Controller => {
                    if let Some(idx) = self.current_list_selection_index()
                        && let Some((ns, _, _, _)) = self.state.backlog_namespaces.get(idx)
                    {
                        self.state.namespace_filter = Some(ns.clone());
                        self.state.show_failures_only = true;
                        self.state.show_progressing_only = false;
                        self.state.show_waiting_only = false;
                        self.state.show_drifting_only = false;
                        self.state.show_deleting_only = false;
                        self.state.active_tab = TabKind::Terraform;
                        // Reset the TF tab back to its list root so the user
                        // sees the filtered Terraforms first, not whatever
                        // they had open before.
                        *self.state.current_view_stack_mut() =
                            vec![ViewState::List(TabKind::Terraform)];
                        self.state.clear_selection_for(&TabKind::Terraform);
                    }
                }
                TabKind::Terraform => {
                    if let Some((ns, name)) = self.get_selected_terraform() {
                        self.state
                            .current_view_stack_mut()
                            .push(ViewState::TerraformDetail {
                                namespace: ns.clone(),
                                name: name.clone(),
                            });
                        self.state.bump_view_revision();
                        self.spawn_detail_outputs_fetch(&ns, &name);
                    }
                }
                TabKind::Kustomizations => {
                    if let Some((ns, name)) = self.get_selected_kustomization() {
                        self.state
                            .current_view_stack_mut()
                            .push(ViewState::KustomizationDetail {
                                namespace: ns,
                                name,
                            });
                        self.state.bump_view_revision();
                    }
                }
                TabKind::Runners => {
                    if let Some((ns, name)) = self.get_selected_runner() {
                        self.start_log_stream(&ns, &name).await;
                    }
                }
                TabKind::CustomTab(i) => {
                    if let Some((ns, name)) = self.get_selected_custom_tab(i) {
                        self.state
                            .current_view_stack_mut()
                            .push(ViewState::TerraformDetail {
                                namespace: ns.clone(),
                                name: name.clone(),
                            });
                        self.state.bump_view_revision();
                        self.spawn_detail_outputs_fetch(&ns, &name);
                    }
                }
            },
            Action::Back => {
                self.state.bump_view_revision();
                if matches!(self.state.current_view(), ViewState::List(_)) {
                    // Peel off, most recent first: bulk selection → search →
                    // failures/waiting/progressing/drifting → namespace. Clearing
                    // bulk first gives the user a fast escape after a
                    // multi-select they don't want to act on.
                    if !self.state.bulk_selected.is_empty() {
                        self.state.bulk_selected.clear();
                    } else if !self.state.search_query.is_empty() {
                        self.state.search_query.clear();
                        self.state.search_suspended = false;
                    } else if self.state.show_failures_only {
                        self.state.show_failures_only = false;
                    } else if self.state.show_waiting_only {
                        self.state.show_waiting_only = false;
                    } else if self.state.show_progressing_only {
                        self.state.show_progressing_only = false;
                    } else if self.state.show_drifting_only {
                        self.state.show_drifting_only = false;
                    } else if self.state.show_deleting_only {
                        self.state.show_deleting_only = false;
                    } else if self.state.namespace_filter.is_some() {
                        self.state.namespace_filter = None;
                    }
                    // Reset table selection when filters change
                    let tab = self.state.active_tab.clone();
                    self.state.clear_selection_for(&tab);
                    self.state.bulk_selected.clear();
                } else if self.state.current_view_stack().len() > 1 {
                    // Cancel log stream if leaving a LogViewer
                    if matches!(self.state.current_view(), ViewState::LogViewer { .. }) {
                        self.cancel_log_stream();
                    }
                    self.state.current_view_stack_mut().pop();
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
                self.state.search_suspended = false;
            }
            Action::SearchPush(c) => {
                self.state.search_query.push(c);
            }
            Action::SearchPaste(text) => {
                if self.state.input_mode == InputMode::Search {
                    self.state.search_query.push_str(&text);
                }
            }
            Action::SearchPop => {
                self.state.search_query.pop();
            }
            Action::SearchConfirm => {
                self.state.input_mode = InputMode::Normal;
                // Auto-select first filtered result so Enter immediately acts on it
                let count = self.current_list_count();
                if count > 0 && self.current_list_selection_index().is_none() {
                    self.select_list_index(Some(0));
                }
            }
            Action::SearchCancel => {
                self.state.input_mode = InputMode::Normal;
                self.state.search_query.clear();
                self.state.search_suspended = false;
                let tab = self.state.active_tab.clone();
                self.state.clear_selection_for(&tab);
            }
            Action::ToggleSearchSuspend => {
                self.state.bump_view_revision();
                if !self.state.search_query.is_empty() {
                    self.state.search_suspended = !self.state.search_suspended;
                    let tab = self.state.active_tab.clone();
                    self.state.clear_selection_for(&tab);
                }
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
                self.state.ns_picker_selected = self.state.ns_picker_selected.saturating_sub(1);
            }
            Action::NamespacePickerSelect => {
                self.state.bump_view_revision();
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
                self.state.clear_selection_for(&TabKind::Terraform);
                self.state.clear_selection_for(&TabKind::Kustomizations);
                self.state.clear_selection_for(&TabKind::Runners);
                for index in 0..self.state.config.custom_tabs.len() {
                    self.state.clear_selection_for(&TabKind::CustomTab(index));
                }
            }
            Action::NamespacePickerCancel => {
                self.state.input_mode = InputMode::Normal;
            }

            // Context picker (Ctrl-X)
            Action::OpenContextPicker => {
                let contexts = self.switcher_contexts();
                if contexts.is_empty() {
                    self.state.flash_message = Some((
                        "No contexts available to switch to".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                } else {
                    self.state.ctx_picker_selected = contexts
                        .iter()
                        .position(|c| *c == self.state.context_name)
                        .unwrap_or(0);
                    self.state.ctx_picker_items = contexts;
                    self.state.input_mode = InputMode::ContextPicker;
                }
            }
            Action::ContextPickerNext => {
                let len = self.state.ctx_picker_items.len();
                if len > 0 {
                    self.state.ctx_picker_selected =
                        (self.state.ctx_picker_selected + 1).min(len - 1);
                }
            }
            Action::ContextPickerPrev => {
                self.state.ctx_picker_selected = self.state.ctx_picker_selected.saturating_sub(1);
            }
            Action::ContextPickerSelect => {
                self.state.input_mode = InputMode::Normal;
                self.initial_context_picker = false;
                if let Some(ctx) = self
                    .state
                    .ctx_picker_items
                    .get(self.state.ctx_picker_selected)
                    .cloned()
                {
                    // Route through the run loop so the switch can suspend
                    // the TUI for an interactive exec/OIDC login if needed.
                    // Re-selecting the current context is allowed — it acts
                    // as a reconnect / re-authenticate.
                    let _ = self.action_tx.send(Action::SwitchContext(ctx));
                }
            }
            Action::ContextPickerCancel => {
                if self.initial_context_picker {
                    self.initial_context_picker = false;
                    self.state.input_mode = InputMode::Normal;
                    self.should_quit = true;
                } else {
                    self.state.input_mode = InputMode::Normal;
                }
            }
            Action::SwitchContext(ctx) => {
                // Normally intercepted by the run loop (which can suspend
                // the TUI for interactive auth); this is a plain fallback.
                self.state.context_name = ctx.clone();
                self.connect(Some(ctx));
            }

            // Shortcuts popup
            Action::OpenShortcutsPopup { namespace, name } => {
                self.state.bump_view_revision();
                let visible = self.state.visible_shortcut_indices(&namespace, &name);
                self.state.input_mode = InputMode::ShortcutsPopup;
                self.state.shortcuts_popup_resource = Some((namespace, name));
                self.state.shortcuts_popup_selected = 0;
                self.state.shortcuts_popup_visible = visible;
            }
            Action::ShortcutsPopupNext => {
                let max = self.state.shortcuts_popup_visible.len().saturating_sub(1);
                if self.state.shortcuts_popup_selected < max {
                    self.state.shortcuts_popup_selected += 1;
                }
            }
            Action::ShortcutsPopupPrev => {
                self.state.shortcuts_popup_selected =
                    self.state.shortcuts_popup_selected.saturating_sub(1);
            }
            Action::ShortcutsPopupSelect => {
                if let Some((ns, nm)) = self.state.shortcuts_popup_resource.clone() {
                    let sel = self.state.shortcuts_popup_selected;
                    if let Some(&idx) = self.state.shortcuts_popup_visible.get(sel) {
                        self.state.input_mode = InputMode::Normal;
                        self.state.shortcuts_popup_resource = None;
                        self.state.shortcuts_popup_visible.clear();
                        // Reuse the existing helper so output-template
                        // fetching stays consistent with direct activation.
                        self.open_shortcut(&ns, &nm, idx);
                    }
                }
            }
            Action::ShortcutsPopupCancel => {
                self.state.bump_view_revision();
                self.state.input_mode = InputMode::Normal;
                self.state.shortcuts_popup_resource = None;
                self.state.shortcuts_popup_visible.clear();
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
                    self.state.viewer_search_index = (self.state.viewer_search_index + 1)
                        % self.state.viewer_search_matches.len();
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
                if self.current_list_selection_is_stale() {
                    let tab = self.state.active_tab.clone();
                    self.state.clear_selection_for(&tab);
                    return;
                }
                if self.current_list_selection_index().is_none() {
                    // Make the first Space behave like the first j: select
                    // the first visible row before toggling it.
                    self.select_list_index(Some(0));
                } else if self.current_list_selection_is_stale() {
                    return;
                }
                if let Some(target) = self.selected_mutation_target() {
                    let key = (target.namespace.clone(), target.name.clone());
                    let same_target = self.state.bulk_selected.get(&key) == Some(&target);
                    if same_target {
                        self.state.bulk_selected.remove(&key);
                    } else {
                        self.state.bulk_selected.insert(key, target);
                    }
                }
                // After toggling, march the cursor forward so repeated
                // Space presses select consecutive rows (k9s pattern).
                let count = self.current_list_count();
                let cur = self.current_list_selection_index().unwrap_or(0);
                if count > 0 && cur + 1 < count {
                    self.select_list_index(Some(cur + 1));
                }
            }
            Action::BulkReconcile => {
                self.execute_bulk_action(|target| Action::Reconcile { target });
            }
            Action::BulkSuspend => {
                self.execute_bulk_action(|target| Action::Suspend { target });
            }
            Action::BulkResume => {
                self.execute_bulk_action(|target| Action::Resume { target });
            }
            Action::BulkApprovePlan => {
                self.execute_bulk_action(|target| Action::ApprovePlan { target });
            }

            // Save viewer content
            Action::SaveViewerContent => {
                self.save_viewer_content();
            }
            Action::ExportTerraformNames => {
                self.export_terraform_names();
            }

            // Confirm dialog
            Action::ShowConfirmDialog(wrapped, message) => {
                self.state.pending_dialog = Some(DialogState {
                    wrapped_action: *wrapped,
                    message,
                    expected_input: None,
                    typed_input: String::new(),
                });
                self.state.input_mode = InputMode::Confirm;
            }
            Action::ShowTypedConfirmDialog(wrapped, message, expected) => {
                self.state.pending_dialog = Some(DialogState {
                    wrapped_action: *wrapped,
                    message,
                    expected_input: Some(expected),
                    typed_input: String::new(),
                });
                self.state.input_mode = InputMode::ConfirmType;
            }
            Action::ConfirmTypePush(c) => {
                if let Some(dialog) = &mut self.state.pending_dialog {
                    dialog.typed_input.push(c);
                }
            }
            Action::ConfirmTypePop => {
                if let Some(dialog) = &mut self.state.pending_dialog {
                    dialog.typed_input.pop();
                }
            }
            Action::ConfirmTypeCancel => {
                self.state.input_mode = InputMode::Normal;
                self.state.pending_dialog = None;
            }
            Action::ConfirmTypeSubmit => {
                let matched = self
                    .state
                    .pending_dialog
                    .as_ref()
                    .and_then(|d| d.expected_input.as_ref().map(|e| *e == d.typed_input))
                    .unwrap_or(false);
                if matched {
                    self.state.input_mode = InputMode::Normal;
                    if let Some(dialog) = self.state.pending_dialog.take() {
                        self.spawn_k8s_action(dialog.wrapped_action);
                    }
                } else {
                    // Wrong text -- keep the dialog open and nudge the operator.
                    self.state.flash_message = Some((
                        "Typed text does not match the resource name".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                }
            }
            Action::ConfirmDialog(confirmed) => {
                self.state.input_mode = InputMode::Normal;
                if confirmed {
                    if let Some(dialog) = self.state.pending_dialog.take() {
                        match dialog.wrapped_action {
                            // Bulk meta-actions need the main dispatcher's
                            // BulkReconcile/Suspend/Resume/ApprovePlan
                            // handlers, which iterate bulk_selected and
                            // call spawn_k8s_action per row. Re-enqueueing
                            // through action_tx routes them there;
                            // spawn_k8s_action would hit the catch-all in
                            // execute_k8s_action and silently no-op.
                            wrapped @ (Action::BulkReconcile
                            | Action::BulkSuspend
                            | Action::BulkResume
                            | Action::BulkApprovePlan) => {
                                let _ = self.action_tx.send(wrapped);
                            }
                            wrapped => self.spawn_k8s_action(wrapped),
                        }
                    }
                } else {
                    self.state.pending_dialog = None;
                }
            }

            // ExecBreakTheGlass is handled directly in the run loop (needs terminal access)
            Action::ExecBreakTheGlass { .. } => {}

            Action::StreamRunnerLogs { namespace, name } => {
                self.state.bump_view_revision();
                let runner_pod = format!("{name}-tf-runner");
                // Check if the runner pod exists
                if self.state.runner_pods.iter().any(|p| {
                    p.metadata.namespace.as_deref() == Some(&namespace)
                        && p.metadata.name.as_deref() == Some(runner_pod.as_str())
                }) {
                    self.start_log_stream(&namespace, &runner_pod).await;
                } else {
                    self.state.flash_message = Some((
                        format!("Runner pod {runner_pod} not found"),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                }
            }

            Action::JumpToTerraformDetail { namespace, name } => {
                self.state.bump_view_revision();
                // Verify the Terraform resource exists in the store
                let exists = self.state.tf_store.state().iter().any(|tf| {
                    tf.metadata.namespace.as_deref() == Some(&namespace)
                        && tf.metadata.name.as_deref() == Some(&name)
                });
                if exists {
                    // Switch to the TF tab and replace its stack with a fresh
                    // detail view — the cross-tab jump is meant to land on
                    // the TF resource, not on whatever was previously open.
                    self.state.active_tab = TabKind::Terraform;
                    *self.state.current_view_stack_mut() = vec![
                        ViewState::List(TabKind::Terraform),
                        ViewState::TerraformDetail {
                            namespace: namespace.clone(),
                            name: name.clone(),
                        },
                    ];
                    self.spawn_detail_outputs_fetch(&namespace, &name);
                } else {
                    self.state.flash_message = Some((
                        format!("Terraform resource {namespace}/{name} not found"),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                }
            }

            Action::StreamControllerLogs {
                namespace,
                pod_name,
            } => {
                self.state.bump_view_revision();
                self.start_log_stream(&namespace, &pod_name).await;
            }

            Action::OpenShortcut {
                namespace,
                name,
                shortcut_idx,
            } => {
                self.open_shortcut(&namespace, &name, shortcut_idx);
            }

            // K8s mutations dispatch asynchronously with their captured
            // object identity.
            Action::Reconcile { .. }
            | Action::Suspend { .. }
            | Action::Resume { .. }
            | Action::Replan { .. }
            | Action::ApprovePlan { .. }
            | Action::ForceUnlock { .. }
            | Action::ResetBreakTheGlass { .. }
            | Action::RemoveFinalizers { .. }
            | Action::DeleteResource { .. }
            | Action::KillRunner { .. } => {
                self.spawn_k8s_action(action);
            }

            // Fetch plan (async)
            Action::FetchPlan {
                namespace,
                name,
                workspace,
            } => {
                self.state.flash_message = Some((
                    format!("Fetching plan for {namespace}/{name}..."),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some((
                        "K8s client not ready yet".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                    return;
                };
                let scope = self.state.begin_async(
                    AsyncKind::Plan,
                    Some(AsyncTarget::Resource {
                        kind: ResourceKind::Terraform,
                        namespace: namespace.clone(),
                        name: name.clone(),
                    }),
                    AsyncIdentity::View(self.state.view_revision),
                );
                let tx = scope.sender(self.action_tx.clone());
                tokio::spawn(async move {
                    match k8s_actions::fetch_plan(&client, &namespace, &name, workspace.as_deref())
                        .await
                    {
                        Ok(plan_text) => {
                            let _ = tx.send(Action::PlanFetched(plan_text));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::PlanFetchError(format!("{e}")));
                        }
                    }
                });
            }
            Action::PlanFetched(plan_text) => {
                self.state.flash_message = None;
                self.state.bump_view_revision();
                self.state
                    .current_view_stack_mut()
                    .push(ViewState::PlanViewer { content: plan_text });
                self.state.plan_scroll = 0;
                self.state.viewer_wrap = false;
            }
            Action::PlanFetchError(e) => {
                self.state.flash_message =
                    Some((format!("Error: {e}"), Instant::now(), FlashKind::Error));
            }

            // JSON view (async)
            Action::FetchJson {
                kind,
                namespace,
                name,
            } => {
                self.state.flash_message = Some((
                    format!("Fetching JSON for {namespace}/{name}..."),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some((
                        "K8s client not ready yet".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                    return;
                };
                let scope = self.state.begin_async(
                    AsyncKind::Resource,
                    Some(AsyncTarget::Resource {
                        kind,
                        namespace: namespace.clone(),
                        name: name.clone(),
                    }),
                    AsyncIdentity::View(self.state.view_revision),
                );
                let tx = scope.sender(self.action_tx.clone());
                tokio::spawn(async move {
                    match k8s_actions::fetch_resource_json(&client, &kind, &namespace, &name).await
                    {
                        Ok(json) => {
                            let _ = tx.send(Action::JsonFetched(json));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::JsonFetchError(format!("{e}")));
                        }
                    }
                });
            }

            // YAML view (async)
            Action::FetchYaml {
                kind,
                namespace,
                name,
            } => {
                self.state.flash_message = Some((
                    format!("Fetching YAML for {namespace}/{name}..."),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some((
                        "K8s client not ready yet".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                    return;
                };
                let scope = self.state.begin_async(
                    AsyncKind::Resource,
                    Some(AsyncTarget::Resource {
                        kind,
                        namespace: namespace.clone(),
                        name: name.clone(),
                    }),
                    AsyncIdentity::View(self.state.view_revision),
                );
                let tx = scope.sender(self.action_tx.clone());
                tokio::spawn(async move {
                    match k8s_actions::fetch_resource_yaml(&client, &kind, &namespace, &name).await
                    {
                        Ok(yaml) => {
                            let _ = tx.send(Action::JsonFetched(yaml));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::JsonFetchError(format!("{e}")));
                        }
                    }
                });
            }
            Action::JsonFetched(yaml) => {
                self.state.flash_message = None;
                self.state.bump_view_revision();
                self.state
                    .current_view_stack_mut()
                    .push(ViewState::JsonViewer { content: yaml });
                self.state.plan_scroll = 0;
                self.state.viewer_wrap = false;
            }
            Action::JsonFetchError(e) => {
                self.state.flash_message =
                    Some((format!("Error: {e}"), Instant::now(), FlashKind::Error));
            }

            // Outputs view (async, Terraform only)
            Action::FetchOutputs { namespace, name } => {
                self.state.flash_message = Some((
                    format!("Fetching outputs for {namespace}/{name}..."),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some((
                        "K8s client not ready yet".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                    return;
                };
                let scope = self.state.begin_async(
                    AsyncKind::Outputs,
                    Some(AsyncTarget::Resource {
                        kind: ResourceKind::Terraform,
                        namespace: namespace.clone(),
                        name: name.clone(),
                    }),
                    AsyncIdentity::View(self.state.view_revision),
                );
                let tx = scope.sender(self.action_tx.clone());
                tokio::spawn(async move {
                    match k8s_actions::fetch_outputs(&client, &namespace, &name).await {
                        Ok(text) => {
                            let _ = tx.send(Action::OutputsFetched(text));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::OutputsFetchError(format!("{e}")));
                        }
                    }
                });
            }
            Action::OutputsFetched(text) => {
                self.state.flash_message = None;
                self.state.bump_view_revision();
                self.state
                    .current_view_stack_mut()
                    .push(ViewState::OutputsViewer { content: text });
                self.state.plan_scroll = 0;
                self.state.horizontal_scroll = 0;
                self.state.viewer_wrap = false;
            }
            Action::OutputsFetchError(e) => {
                self.state.flash_message =
                    Some((format!("Error: {e}"), Instant::now(), FlashKind::Error));
            }

            // Events view (async)
            Action::FetchEvents {
                kind,
                namespace,
                name,
            } => {
                self.state.flash_message = Some((
                    format!("Fetching events for {namespace}/{name}..."),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some((
                        "K8s client not ready yet".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                    return;
                };
                let scope = self.state.begin_async(
                    AsyncKind::Events,
                    Some(AsyncTarget::Resource {
                        kind,
                        namespace: namespace.clone(),
                        name: name.clone(),
                    }),
                    AsyncIdentity::View(self.state.view_revision),
                );
                let tx = scope.sender(self.action_tx.clone());
                tokio::spawn(async move {
                    match k8s_actions::fetch_events(&client, &kind, &namespace, &name).await {
                        Ok(events) => {
                            let _ = tx.send(Action::EventsFetched(events));
                        }
                        Err(e) => {
                            let _ = tx.send(Action::EventsFetchError(format!("{e}")));
                        }
                    }
                });
            }
            Action::EventsFetched(events) => {
                self.state.flash_message = None;
                self.state.bump_view_revision();
                self.state
                    .current_view_stack_mut()
                    .push(ViewState::EventsViewer { content: events });
                self.state.plan_scroll = 0;
                self.state.horizontal_scroll = 0;
                self.state.viewer_wrap = false;
            }
            Action::DetailOutputsFetched {
                namespace,
                name,
                values,
            } => {
                self.state.cached_outputs = Some(((namespace, name), values));
            }
            Action::DetailOutputsFetchError(e) => {
                tracing::debug!("Detail output fetch failed: {e}");
            }
            Action::ShortcutOutputsFetched {
                namespace,
                name,
                shortcut_idx,
                values,
            } => {
                self.state.cached_outputs = Some(((namespace.clone(), name.clone()), values));
                self.open_shortcut(&namespace, &name, shortcut_idx);
            }
            Action::ShortcutSecretValuesFetched {
                namespace,
                name,
                secret_name,
                shortcut_idx,
                values,
            } => {
                self.state
                    .cached_secrets
                    .insert((namespace.clone(), secret_name), values);
                self.open_shortcut(&namespace, &name, shortcut_idx);
            }
            Action::SecretValuesFetchError(e) => {
                self.state.flash_message = Some((
                    format!("Secret error: {e}"),
                    Instant::now(),
                    FlashKind::Error,
                ));
            }
            Action::EventsFetchError(e) => {
                self.state.flash_message =
                    Some((format!("Error: {e}"), Instant::now(), FlashKind::Error));
            }

            // Conditions viewer (synchronous — built from in-memory store)
            Action::ViewConditions {
                kind,
                namespace,
                name,
            } => {
                let content = match kind {
                    ResourceKind::Terraform => self
                        .state
                        .tf_store
                        .state()
                        .iter()
                        .find(|t| {
                            t.metadata.namespace.as_deref() == Some(namespace.as_str())
                                && t.metadata.name.as_deref() == Some(name.as_str())
                        })
                        .map(|tf| {
                            let conds = tf
                                .status
                                .as_ref()
                                .and_then(|s| s.conditions.as_ref())
                                .map(|v| v.as_slice())
                                .unwrap_or(&[]);
                            crate::util::format_conditions_viewer(
                                "Terraform",
                                &namespace,
                                &name,
                                conds,
                            )
                        }),
                    ResourceKind::Kustomization => self
                        .state
                        .ks_store
                        .state()
                        .iter()
                        .find(|k| {
                            k.metadata.namespace.as_deref() == Some(namespace.as_str())
                                && k.metadata.name.as_deref() == Some(name.as_str())
                        })
                        .map(|ks| {
                            let conds = ks
                                .status
                                .as_ref()
                                .and_then(|s| s.conditions.as_ref())
                                .map(|v| v.as_slice())
                                .unwrap_or(&[]);
                            crate::util::format_conditions_viewer(
                                "Kustomization",
                                &namespace,
                                &name,
                                conds,
                            )
                        }),
                    ResourceKind::Pod => None,
                };
                if let Some(content) = content {
                    self.state
                        .current_view_stack_mut()
                        .push(ViewState::ConditionsViewer { content });
                    self.state.plan_scroll = 0;
                    self.state.horizontal_scroll = 0;
                    self.state.viewer_wrap = false;
                } else {
                    self.state.flash_message = Some((
                        format!("Resource {namespace}/{name} not found"),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                }
            }

            // Log streaming chunks
            Action::LogChunkReceived { stream_id, chunk } => {
                const MAX_LOG_BYTES: usize = 10 * 1024 * 1024; // 10 MB
                // The LogViewer might be on a tab the user has navigated
                // away from — chunks still need to find it. Capture flags
                // we'll need before taking a mutable borrow on the viewer.
                let on_active_tab = matches!(
                    self.state.current_view(),
                    ViewState::LogViewer {
                        stream_id: current,
                        ..
                    } if *current == stream_id
                );
                let auto_follow = self.state.log_auto_follow;
                let body_height = self.state.body_height as usize;
                let mut new_scroll: Option<usize> = None;
                if let Some(ViewState::LogViewer { content, .. }) =
                    self.state.log_viewer_mut_for(stream_id)
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
                    if on_active_tab && auto_follow {
                        let line_count = content.lines().count();
                        new_scroll = Some(line_count.saturating_sub(body_height));
                    }
                }
                if let Some(s) = new_scroll {
                    self.state.plan_scroll = s;
                }
            }

            // Runner pods update from poller
            Action::RunnerPodsUpdated(pods) => {
                self.state.runner_pods = pods;
                self.state.runners_synced = true;
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
            Action::K8sClientReady {
                client,
                context_name,
            } => {
                self.client = Some(client.0);
                self.state.context_name = context_name;
                self.state.connection_error = None;
            }
            Action::ConnectionError(msg) => {
                self.state.connection_error = Some(crate::util::humanize_cluster_error(&msg));
            }
            Action::BackgroundError(err) => {
                self.state.background_error = err.map(|m| crate::util::humanize_cluster_error(&m));
            }
            Action::DismissError => {
                self.state.connection_error = None;
            }
            Action::AuthExpired => {
                // Idempotent: the first failing task tears things down; later
                // AuthExpired sends from sibling tasks are ignored.
                if !self.state.needs_reauth {
                    self.state.needs_reauth = true;
                    self.state.invalidate_connection();
                    if let Some(handle) = self.state.log_stream_handle.take() {
                        handle.abort();
                    }
                    if let Some(handle) = self.state.metrics_task.take() {
                        handle.abort();
                    }
                    self.state.metrics_enabled = false;
                    self.state.metrics_snapshot = None;
                    self.state.metrics_prev = crate::k8s::metrics::PrevCounters::default();
                    self.state.metrics_last_error = None;
                    self.state.cancel_log_stream();
                    // Stop every background task so kube-rs stops re-running
                    // the OIDC plugin (which keeps opening browser tabs).
                    if let Ok(mut tasks) = self.conn_tasks.lock() {
                        for h in tasks.drain(..) {
                            h.abort();
                        }
                    }
                    self.client = None;
                    self.state.connection_error = Some(
                        "Cluster authentication expired — no browser logins will be opened. \
                         Press Ctrl-X to re-authenticate."
                            .to_string(),
                    );
                }
            }

            // CRD missing indicators
            Action::TerraformCrdMissing => {
                self.state.tf_crd_missing = true;
            }
            Action::KustomizationCrdMissing => {
                self.state.ks_crd_missing = true;
            }
            Action::GitRepoCrdMissing => {
                self.state.gr_crd_missing = true;
            }

            // Async K8s action results
            Action::K8sActionSuccess { target, message } => {
                if !self.mutation_target_matches_current_resource(&target) {
                    return;
                }
                self.state.flash_message = Some((message, Instant::now(), FlashKind::Success));
            }
            Action::K8sActionError { target, message } => {
                if !self.mutation_target_matches_current_resource(&target) {
                    return;
                }
                self.state.flash_message = Some((
                    format!("Error: {message}"),
                    Instant::now(),
                    FlashKind::Error,
                ));
            }

            Action::TerraformStoreUpdated => {
                self.state.tf_synced = true;
                self.state.last_data_update = Some(Instant::now());
            }
            Action::KustomizationStoreUpdated => {
                self.state.ks_synced = true;
                self.state.last_data_update = Some(Instant::now());
            }
            Action::GitRepoStoreUpdated => {
                self.state.gr_synced = true;
                self.state.last_data_update = Some(Instant::now());
            }
            Action::Resize(_, _) | Action::None => {}

            _ => {}
        }
    }

    fn require_client(&self) -> Option<kube::Client> {
        self.client.clone()
    }

    /// A mutation result can arrive after an object was deleted and recreated
    /// with the same name. Treat a missing object as compatible with a
    /// completed delete, but never attribute the result to a different UID.
    fn mutation_target_matches_current_resource(&self, target: &MutationTarget) -> bool {
        match target.kind {
            ResourceKind::Terraform => self
                .state
                .tf_store
                .get(
                    &kube::runtime::reflector::ObjectRef::new(&target.name)
                        .within(&target.namespace),
                )
                .is_none_or(|resource| {
                    resource.metadata.uid.as_deref() == Some(target.uid.as_str())
                }),
            ResourceKind::Kustomization => self
                .state
                .ks_store
                .get(
                    &kube::runtime::reflector::ObjectRef::new(&target.name)
                        .within(&target.namespace),
                )
                .is_none_or(|resource| {
                    resource.metadata.uid.as_deref() == Some(target.uid.as_str())
                }),
            ResourceKind::Pod => self
                .state
                .runner_pods
                .iter()
                .find(|pod| {
                    pod.metadata.namespace.as_deref() == Some(target.namespace.as_str())
                        && pod.metadata.name.as_deref() == Some(target.name.as_str())
                })
                .is_none_or(|pod| pod.metadata.uid.as_deref() == Some(target.uid.as_str())),
        }
    }

    fn async_target_matches_selection(&self, scope: &AsyncScope) -> bool {
        let Some(AsyncTarget::Resource {
            kind,
            namespace,
            name,
        }) = &scope.target
        else {
            return true;
        };

        let selected = match (kind, &self.state.active_tab) {
            (ResourceKind::Terraform, TabKind::Terraform) => self.get_selected_terraform(),
            (ResourceKind::Terraform, TabKind::CustomTab(index)) => {
                self.get_selected_custom_tab(*index)
            }
            (ResourceKind::Kustomization, TabKind::Kustomizations) => {
                self.get_selected_kustomization()
            }
            (ResourceKind::Pod, TabKind::Runners) => self.get_selected_runner(),
            _ => return true,
        };
        selected.is_some_and(|(selected_namespace, selected_name)| {
            selected_namespace == *namespace && selected_name == *name
        })
    }

    fn spawn_detail_outputs_fetch(&mut self, ns: &str, name: &str) {
        let Some(client) = self.require_client() else {
            return;
        };
        let scope = self.state.begin_async(
            AsyncKind::DetailOutputs,
            Some(AsyncTarget::Resource {
                kind: ResourceKind::Terraform,
                namespace: ns.to_string(),
                name: name.to_string(),
            }),
            AsyncIdentity::View(self.state.view_revision),
        );
        let tx = scope.sender(self.action_tx.clone());
        let ns = ns.to_string();
        let name = name.to_string();
        tokio::spawn(async move {
            match k8s_actions::fetch_output_values(&client, &ns, &name).await {
                Ok(values) => {
                    let _ = tx.send(Action::DetailOutputsFetched {
                        namespace: ns,
                        name,
                        values,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Action::DetailOutputsFetchError(format!("{e}")));
                }
            }
        });
    }

    fn spawn_k8s_action(&mut self, action: Action) {
        // Mark the target row as recently acted on so the filtered list
        // keeps it visible during the grace window — without this, a
        // reconcile from a failures-only view makes the row vanish the
        // instant the controller starts reconciling. Covers all paths
        // (single-row + bulk-fanout).
        if let Some(target) = action_resource_target(&action) {
            self.state
                .mark_recently_acted(&target.namespace, &target.name);
        }

        let client = match self.require_client() {
            Some(c) => c,
            None => {
                self.state.flash_message = Some((
                    "K8s client not ready yet".to_string(),
                    Instant::now(),
                    FlashKind::Error,
                ));
                return;
            }
        };
        let target = action_resource_target(&action).map(|target| AsyncTarget::Resource {
            kind: target.kind,
            namespace: target.namespace.clone(),
            name: target.name.clone(),
        });
        let scope = self
            .state
            .begin_async(AsyncKind::Mutation, target, AsyncIdentity::None);
        let tx = scope.sender(self.action_tx.clone());
        let mutation_target = action_resource_target(&action)
            .cloned()
            .expect("mutating actions always carry a target");
        let success_msg = format_success_message(&action);
        let context = self.state.context_name.clone();
        // tfctl-backed actions (replan) need to see terrarium's kubeconfig.
        let kubeconfig = self.switcher_kubeconfig_path.clone();

        tokio::spawn(async move {
            let result =
                execute_k8s_action(&client, &action, Some(&context), kubeconfig.as_deref()).await;
            match result {
                Ok(()) => {
                    let _ = tx.send(Action::K8sActionSuccess {
                        target: mutation_target.clone(),
                        message: success_msg,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Action::K8sActionError {
                        target: mutation_target,
                        message: format!("{e}"),
                    });
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
                    self.state.effective_search_query(),
                    self.state.show_failures_only,
                    self.state.show_waiting_only,
                    self.state.show_progressing_only,
                    self.state.show_drifting_only,
                    self.state.show_deleting_only,
                    &self.state.recently_acted,
                    self.state.sort_column,
                    self.state.sort_descending,
                );
                for (i, tf) in items.iter().enumerate() {
                    if crate::util::classify_ready(
                        tf.status.as_ref().and_then(|s| s.conditions.as_ref()),
                    )
                    .is_real_failure()
                    {
                        self.select_list_index(Some(i));
                        return;
                    }
                }
            }
            TabKind::Kustomizations => {
                let items = get_filtered_kustomizations(
                    &self.state.ks_store,
                    &self.state.namespace_filter,
                    self.state.effective_search_query(),
                    self.state.show_failures_only,
                    self.state.show_waiting_only,
                    self.state.show_progressing_only,
                    self.state.show_deleting_only,
                    &self.state.recently_acted,
                    self.state.sort_column,
                    self.state.sort_descending,
                );
                for (i, ks) in items.iter().enumerate() {
                    if crate::util::classify_ready(
                        ks.status.as_ref().and_then(|s| s.conditions.as_ref()),
                    )
                    .is_real_failure()
                    {
                        self.select_list_index(Some(i));
                        return;
                    }
                }
            }
            _ => {}
        }
    }

    fn execute_bulk_action<F>(&mut self, make_action: F)
    where
        F: Fn(MutationTarget) -> Action,
    {
        if !matches!(
            self.state.active_tab,
            TabKind::Terraform | TabKind::CustomTab(_) | TabKind::Kustomizations
        ) {
            return;
        }
        // Take the selection so it auto-clears after dispatch — the
        // user has acted on it, leaving it selected would be a footgun
        // on the next keypress.
        let selected = std::mem::take(&mut self.state.bulk_selected);
        let count = selected.len();
        for (_, target) in selected {
            let action = make_action(target);
            self.spawn_k8s_action(action);
        }
        if count > 0 {
            self.state.flash_message = Some((
                format!("Dispatched {count} bulk action(s)"),
                Instant::now(),
                FlashKind::Success,
            ));
        }
    }

    fn save_viewer_content(&mut self) {
        let content = match self.state.current_view() {
            ViewState::PlanViewer { content } => Some(("plan", content.clone())),
            ViewState::JsonViewer { content } => Some(("json", content.clone())),
            ViewState::EventsViewer { content } => Some(("events", content.clone())),
            ViewState::OutputsViewer { content } => Some(("outputs", content.clone())),
            ViewState::ConditionsViewer { content } => Some(("conditions", content.clone())),
            ViewState::LogViewer { content, .. } => Some(("logs", content.clone())),
            _ => None,
        };
        if let Some((prefix, text)) = content {
            let timestamp = jiff::Timestamp::now().strftime("%Y%m%d_%H%M%S");
            let filename = format!("terrarium_{prefix}_{timestamp}.txt");
            match create_private_file(&filename) {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(text.as_bytes()) {
                        self.state.flash_message = Some((
                            format!("Write error: {e}"),
                            Instant::now(),
                            FlashKind::Error,
                        ));
                    } else {
                        self.state.flash_message = Some((
                            format!("Saved to {filename}"),
                            Instant::now(),
                            FlashKind::Success,
                        ));
                    }
                }
                Err(e) => {
                    self.state.flash_message =
                        Some((format!("Save error: {e}"), Instant::now(), FlashKind::Error));
                }
            }
        }
    }

    fn export_terraform_names(&mut self) {
        if !matches!(self.state.active_tab, TabKind::Terraform) {
            return;
        }

        let items = get_filtered_terraforms(
            &self.state.tf_store,
            &self.state.namespace_filter,
            self.state.effective_search_query(),
            self.state.show_failures_only,
            self.state.show_waiting_only,
            self.state.show_progressing_only,
            self.state.show_drifting_only,
            self.state.show_deleting_only,
            &self.state.recently_acted,
            self.state.sort_column,
            self.state.sort_descending,
        );
        let names = terraform_names_text(&items);
        let timestamp = jiff::Timestamp::now().strftime("%Y%m%d_%H%M%S");
        let filename = format!("terrarium_terraform_names_{timestamp}.txt");

        match create_private_file(&filename).and_then(|mut file| {
            file.write_all(names.as_bytes())?;
            if !names.is_empty() {
                file.write_all(b"\n")?;
            }
            Ok(())
        }) {
            Ok(()) => {
                self.state.flash_message = Some((
                    format!("Exported {} Terraform name(s) to {filename}", items.len()),
                    Instant::now(),
                    FlashKind::Success,
                ));
            }
            Err(error) => {
                self.state.flash_message = Some((
                    format!("Export error: {error}"),
                    Instant::now(),
                    FlashKind::Error,
                ));
            }
        }
    }

    fn open_shortcut(&mut self, namespace: &str, name: &str, shortcut_idx: usize) {
        let shortcut = match self.state.config.shortcuts.get(shortcut_idx) {
            Some(s) => s.clone(),
            None => return,
        };

        // Submenu branches are config-allowed but the popup doesn't drill
        // into them yet — flash and bail rather than panic.
        let Some(url_template) = shortcut.url.clone() else {
            self.state.flash_message = Some((
                format!(
                    "'{}' is a submenu (drill-in not yet supported)",
                    shortcut.label
                ),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        };

        let needs_outputs = url_template.contains("{output.");
        if needs_outputs {
            let cached_match = self
                .state
                .cached_outputs
                .as_ref()
                .map(|((ns, n), _)| ns == namespace && n == name)
                .unwrap_or(false);
            if !cached_match {
                let Some(client) = self.require_client() else {
                    self.state.flash_message = Some((
                        "K8s client not ready yet".to_string(),
                        Instant::now(),
                        FlashKind::Error,
                    ));
                    return;
                };
                self.state.flash_message = Some((
                    format!("Loading outputs for {namespace}/{name}..."),
                    Instant::now(),
                    FlashKind::Success,
                ));
                let scope = self.state.begin_async(
                    AsyncKind::ShortcutOutputs,
                    Some(AsyncTarget::Resource {
                        kind: ResourceKind::Terraform,
                        namespace: namespace.to_string(),
                        name: name.to_string(),
                    }),
                    AsyncIdentity::View(self.state.view_revision),
                );
                let tx = scope.sender(self.action_tx.clone());
                let ns = namespace.to_string();
                let nm = name.to_string();
                tokio::spawn(async move {
                    match k8s_actions::fetch_output_values(&client, &ns, &nm).await {
                        Ok(values) => {
                            let _ = tx.send(Action::ShortcutOutputsFetched {
                                namespace: ns.clone(),
                                name: nm.clone(),
                                shortcut_idx,
                                values,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(Action::OutputsFetchError(format!("{e}")));
                        }
                    }
                });
                return;
            }
        }

        // Same lazy-fetch-and-retry dance for {secret.X.Y} placeholders:
        // pick the first uncached secret name referenced in the URL, fetch
        // it, and re-send OpenShortcut. Loop converges once all are cached.
        if let Some(missing) =
            first_uncached_secret(&url_template, namespace, &self.state.cached_secrets)
        {
            let Some(client) = self.require_client() else {
                self.state.flash_message = Some((
                    "K8s client not ready yet".to_string(),
                    Instant::now(),
                    FlashKind::Error,
                ));
                return;
            };
            self.state.flash_message = Some((
                format!("Loading secret {namespace}/{missing}..."),
                Instant::now(),
                FlashKind::Success,
            ));
            let scope = self.state.begin_async(
                AsyncKind::SecretValues,
                Some(AsyncTarget::Resource {
                    kind: ResourceKind::Terraform,
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                }),
                AsyncIdentity::View(self.state.view_revision),
            );
            let tx = scope.sender(self.action_tx.clone());
            let ns = namespace.to_string();
            let nm = name.to_string();
            let secret_name = missing.clone();
            tokio::spawn(async move {
                match k8s_actions::fetch_secret_values(&client, &ns, &secret_name).await {
                    Ok(values) => {
                        let _ = tx.send(Action::ShortcutSecretValuesFetched {
                            namespace: ns.clone(),
                            name: nm.clone(),
                            secret_name,
                            shortcut_idx,
                            values,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(Action::SecretValuesFetchError(format!("{e}")));
                    }
                }
            });
            return;
        }

        // Resolve template variables
        let mut url = url_template.clone();
        url = url.replace("{context}", &self.state.context_name);
        url = url.replace("{namespace}", namespace);
        url = url.replace("{name}", name);

        // {var.KEY} reads from the merged [[context_vars]] table for the
        // active kube context. Resolved before the resource-scoped
        // placeholders so a context_vars entry can supply, say, a host
        // that the rest of the URL then parameterizes with {name}.
        if url.contains("{var.") {
            let vars = self.state.vars_for_context();
            url = resolve_var_placeholders(
                &url,
                &vars,
                &self.state.context_name,
                &mut self.state.var_warned,
            );
        }

        // {label.KEY} / {annotation.KEY} read directly from the TF
        // resource's metadata — already cached in the store, so no
        // lazy fetch is needed (unlike outputs and secrets). Useful
        // when the cluster identity / environment / module lives on
        // a label rather than in a tofu output.
        if url.contains("{label.") || url.contains("{annotation.") {
            let (labels, annotations) = self
                .state
                .tf_store
                .state()
                .iter()
                .find(|arc| {
                    arc.metadata.namespace.as_deref() == Some(namespace)
                        && arc.metadata.name.as_deref() == Some(name)
                })
                .map(|arc| {
                    (
                        arc.metadata.labels.clone(),
                        arc.metadata.annotations.clone(),
                    )
                })
                .unwrap_or((None, None));
            if url.contains("{label.") {
                url = resolve_map_placeholders(&url, "label", labels.as_ref());
            }
            if url.contains("{annotation.") {
                url = resolve_map_placeholders(&url, "annotation", annotations.as_ref());
            }
        }

        if needs_outputs && let Some((_, outputs)) = &self.state.cached_outputs {
            url = resolve_output_placeholders(&url, outputs);
        }
        if url.contains("{secret.") {
            url = resolve_secret_placeholders(&url, namespace, &self.state.cached_secrets);
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
                    format!("Failed to open browser: {e}"),
                    Instant::now(),
                    FlashKind::Error,
                ));
            }
        }
    }

    async fn exec_break_the_glass(
        &mut self,
        terminal: &mut crate::tui::Tui,
        target: &MutationTarget,
    ) {
        let Some(client) = self.require_client() else {
            self.state.flash_message = Some((
                "K8s client not ready yet".to_string(),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        };
        if let Err(e) = k8s_actions::validate_terraform_target(&client, target).await {
            self.state.flash_message = Some((
                format!("Refusing break-glass: {e}"),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        }
        // Just delegate to tfctl — it handles the entire BTG lifecycle correctly.
        // Trying to reimplement its K8s patch logic has proven unreliable.
        self.state.flash_message = Some((
            format!(
                "Launching tfctl break-glass {} -n {}...",
                target.name, target.namespace
            ),
            Instant::now(),
            FlashKind::Success,
        ));
        terminal.draw(|f| layout::render(f, &mut self.state)).ok();

        // Suspend TUI
        if let Err(e) = crate::tui::restore() {
            self.state.flash_message = Some((
                format!("Failed to suspend TUI: {e}"),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        }

        // The object may have changed while the terminal was being prepared
        // for the interactive command. Re-check immediately before handing
        // control to tfctl; direct API mutations additionally carry the
        // resourceVersion on their request.
        if let Err(e) = k8s_actions::validate_terraform_target(&client, target).await {
            let _ = crate::tui::resume(terminal, self.state.mouse_enabled);
            self.state.flash_message = Some((
                format!("Refusing break-glass: {e}"),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        }

        // Run tfctl break-glass — it handles: annotation, wait, exec, cleanup.
        let mut cmd = std::process::Command::new("tfctl");
        // Point tfctl at terrarium's kubeconfig (in-memory switcher config is
        // materialised to a temp file) and the active context, so BTG targets
        // the cluster the TUI is showing rather than the kubeconfig's default
        // current-context.
        if let Some(kc) = &self.switcher_kubeconfig_path {
            cmd.env("KUBECONFIG", kc);
        }
        let ctx = self.state.context_name.clone();
        if !ctx.is_empty() && ctx != "connecting..." {
            cmd.args(["--context", &ctx]);
        }
        let status = cmd
            .args(["break-glass", &target.name, "-n", &target.namespace])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status();

        // Restore TUI
        if let Err(e) = crate::tui::resume(terminal, self.state.mouse_enabled) {
            eprintln!("Failed to restore TUI: {e}");
            self.should_quit = true;
            return;
        }

        match status {
            Ok(s) if s.success() => {
                self.state.flash_message = Some((
                    format!("BTG session ended for {}/{}", target.namespace, target.name),
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
                    format!("Failed to run tfctl: {e} — is tfctl installed?"),
                    Instant::now(),
                    FlashKind::Error,
                ));
            }
        }
    }

    async fn start_log_stream(&mut self, namespace: &str, name: &str) {
        // Cancel any existing stream
        self.cancel_log_stream();

        let Some(client) = self.require_client() else {
            self.state.flash_message = Some((
                "K8s client not ready yet".to_string(),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        };
        self.state.bump_view_revision();

        let stream_scope = self.state.begin_log_stream(Some(AsyncTarget::Pod {
            namespace: namespace.to_string(),
            name: name.to_string(),
        }));
        let stream_id = stream_scope.request_id;

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
        self.state
            .current_view_stack_mut()
            .push(ViewState::LogViewer {
                namespace: namespace.to_string(),
                pod_name: name.to_string(),
                stream_id,
                containers: containers.clone(),
                active_container: 0,
                content: String::new(),
            });
        self.state.plan_scroll = 0;
        self.state.viewer_wrap = false;
        self.state.log_auto_follow = true;

        // Start streaming
        let tx = stream_scope.sender(self.action_tx.clone());
        let ns = namespace.to_string();
        let pod_name = name.to_string();
        // Strip "init:" prefix for the API call
        let api_container =
            first_container.map(|c| c.strip_prefix("init:").unwrap_or(&c).to_string());

        let handle = tokio::spawn(async move {
            if let Err(e) = k8s_actions::stream_pod_logs(
                &client,
                &ns,
                &pod_name,
                api_container.as_deref(),
                stream_id,
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
        let (namespace, pod_name, containers, active) = if let ViewState::LogViewer {
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
        self.state.current_view_stack_mut().pop();

        let Some(client) = self.require_client() else {
            self.state.bump_view_revision();
            self.state.flash_message = Some((
                "K8s client not ready yet".to_string(),
                Instant::now(),
                FlashKind::Error,
            ));
            return;
        };
        self.state.bump_view_revision();

        let stream_scope = self.state.begin_log_stream(Some(AsyncTarget::Pod {
            namespace: namespace.clone(),
            name: pod_name.clone(),
        }));
        let stream_id = stream_scope.request_id;

        // Push new log viewer with empty content
        self.state
            .current_view_stack_mut()
            .push(ViewState::LogViewer {
                namespace: namespace.clone(),
                pod_name: pod_name.clone(),
                stream_id,
                containers: containers.clone(),
                active_container: new_idx,
                content: String::new(),
            });
        self.state.plan_scroll = 0;
        self.state.log_auto_follow = true;

        // Start new stream
        let tx = stream_scope.sender(self.action_tx.clone());
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
                stream_id,
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

async fn execute_k8s_action(
    client: &kube::Client,
    action: &Action,
    context: Option<&str>,
    kubeconfig: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    match action {
        Action::ApprovePlan { target } => k8s_actions::approve_plan(client, target).await,
        Action::Reconcile { target } => match target.kind {
            ResourceKind::Terraform => k8s_actions::force_reconcile(client, target).await,
            ResourceKind::Kustomization => {
                k8s_actions::reconcile_kustomization(client, target).await
            }
            ResourceKind::Pod => Ok(()),
        },
        Action::Replan { target } => k8s_actions::replan(client, target, context, kubeconfig).await,
        Action::Suspend { target } => match target.kind {
            ResourceKind::Terraform => k8s_actions::suspend(client, target).await,
            ResourceKind::Kustomization => k8s_actions::suspend_kustomization(client, target).await,
            ResourceKind::Pod => Ok(()),
        },
        Action::Resume { target } => match target.kind {
            ResourceKind::Terraform => k8s_actions::resume(client, target).await,
            ResourceKind::Kustomization => k8s_actions::resume_kustomization(client, target).await,
            ResourceKind::Pod => Ok(()),
        },
        Action::ForceUnlock { target } => k8s_actions::force_unlock(client, target).await,
        Action::ResetBreakTheGlass { target } => {
            k8s_actions::reset_break_the_glass(client, target).await
        }
        Action::RemoveFinalizers { target } => {
            k8s_actions::remove_terraform_finalizers(client, target).await
        }
        Action::DeleteResource { target } => match target.kind {
            ResourceKind::Terraform => k8s_actions::delete_terraform(client, target).await,
            ResourceKind::Kustomization => k8s_actions::delete_kustomization(client, target).await,
            ResourceKind::Pod => Ok(()),
        },
        Action::KillRunner { target } => k8s_actions::delete_pod(client, target).await,
        _ => Ok(()),
    }
}

/// Extract the (namespace, name) target of a per-resource K8s action,
/// for marking the row as recently acted on. Returns None for actions
/// that don't target a specific listed resource (Bulk*, sync events,
/// fetch-results, …).
fn action_resource_target(action: &Action) -> Option<&MutationTarget> {
    match action {
        Action::ApprovePlan { target }
        | Action::Replan { target }
        | Action::ForceUnlock { target }
        | Action::ResetBreakTheGlass { target }
        | Action::RemoveFinalizers { target }
        | Action::DeleteResource { target }
        | Action::KillRunner { target }
        | Action::Reconcile { target }
        | Action::Suspend { target }
        | Action::Resume { target } => Some(target),
        _ => None,
    }
}

fn mutation_target_from_metadata(
    kind: ResourceKind,
    namespace: &str,
    name: &str,
    uid: &Option<String>,
    resource_version: &Option<String>,
) -> Option<MutationTarget> {
    Some(MutationTarget {
        kind,
        namespace: namespace.to_string(),
        name: name.to_string(),
        uid: uid.clone()?,
        resource_version: resource_version.clone()?,
    })
}

#[cfg(test)]
mod mutation_target_tests {
    use super::mutation_target_from_metadata;
    use crate::action::ResourceKind;

    #[test]
    fn mutation_target_requires_uid_and_resource_version() {
        assert!(
            mutation_target_from_metadata(
                ResourceKind::Terraform,
                "ns",
                "demo",
                &None,
                &Some("7".into()),
            )
            .is_none()
        );
        assert!(
            mutation_target_from_metadata(
                ResourceKind::Terraform,
                "ns",
                "demo",
                &Some("uid".into()),
                &None,
            )
            .is_none()
        );
    }
}

fn format_success_message(action: &Action) -> String {
    match action {
        Action::ApprovePlan { target } => {
            format!("Approved plan for {}/{}", target.namespace, target.name)
        }
        Action::Reconcile { target } => {
            format!(
                "Triggered reconciliation for {}/{}",
                target.namespace, target.name
            )
        }
        Action::Replan { target } => {
            format!("Triggered replan for {}/{}", target.namespace, target.name)
        }
        Action::Suspend { target } => {
            format!("Suspended {}/{}", target.namespace, target.name)
        }
        Action::Resume { target } => {
            format!("Resumed {}/{}", target.namespace, target.name)
        }
        Action::ForceUnlock { target } => {
            format!("Force unlocked {}/{}", target.namespace, target.name)
        }
        Action::ResetBreakTheGlass { target } => {
            format!(
                "Disabled persistent break-the-glass mode for {}/{}",
                target.namespace, target.name
            )
        }
        Action::RemoveFinalizers { target } => {
            format!(
                "Removed finalizers from Terraform {}/{}",
                target.namespace, target.name
            )
        }
        Action::DeleteResource { target } => {
            let kind = match target.kind {
                ResourceKind::Terraform => "Terraform",
                ResourceKind::Kustomization => "Kustomization",
                ResourceKind::Pod => "runner",
            };
            format!("Deleted {kind} {}/{}", target.namespace, target.name)
        }
        Action::KillRunner { target } => {
            format!("Killed runner {}/{}", target.namespace, target.name)
        }
        _ => "Action completed".to_string(),
    }
}

fn terraform_names_text(items: &[crate::k8s::terraform::Terraform]) -> String {
    let names = items
        .iter()
        .filter_map(|tf| tf.metadata.name.as_deref())
        .collect::<Vec<_>>();
    if names.is_empty() {
        String::new()
    } else {
        format!("{}\n", names.join("\n"))
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

/// Substitute `{output.PATH}` placeholders in a URL.
///
/// `PATH` is dot-separated. The first segment indexes into the outputs secret
/// (a flat key/value map). If more segments follow, the value is parsed as
/// JSON and traversed by object key. Missing keys substitute as empty.
fn resolve_output_placeholders(
    url: &str,
    outputs: &std::collections::HashMap<String, String>,
) -> String {
    let mut result = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(start) = rest.find("{output.") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 8..];
        match after.find('}') {
            Some(end) => {
                let path = &after[..end];
                result.push_str(&lookup_output_path(outputs, path));
                rest = &after[end + 1..];
            }
            None => {
                result.push_str(&rest[start..]);
                return result;
            }
        }
    }
    result.push_str(rest);
    result
}

/// Substitute `{var.KEY}` placeholders against the merged context_vars
/// map for the active kube context. Missing keys substitute as empty
/// — matching the `{output.KEY}` / `{label.KEY}` semantics — and emit
/// a `tracing::warn!` the first time a given `(context, key)` pair
/// goes unresolved so the user can spot a misconfigured [[context_vars]]
/// table without the log filling up on every shortcut activation.
fn resolve_var_placeholders(
    url: &str,
    vars: &std::collections::BTreeMap<String, String>,
    context: &str,
    warned: &mut std::collections::HashSet<(String, String)>,
) -> String {
    let mut result = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(start) = rest.find("{var.") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 5..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(value) => result.push_str(value),
                    None => {
                        let warn_key = (context.to_string(), key.to_string());
                        if warned.insert(warn_key) {
                            tracing::warn!(
                                context = context,
                                key = key,
                                "shortcut URL references {{var.{key}}} but no \
                                 matching [[context_vars]] entry for context \
                                 '{context}' defines it — substituting empty"
                            );
                        }
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                // Malformed (unterminated `{var.`) — leave as-is.
                result.push_str(&rest[start..]);
                return result;
            }
        }
    }
    result.push_str(rest);
    result
}

/// Substitute `{<prefix>.<key>}` placeholders against an optional
/// BTreeMap (used for `{label.KEY}` and `{annotation.KEY}`). Missing
/// keys substitute as empty, matching the `{output.KEY}` semantics.
///
/// Key syntax matches anything between the `{<prefix>.` and the next
/// `}` — so dotted/slashed Kubernetes label keys like
/// `kustomize.toolkit.fluxcd.io/name` work without escaping.
fn resolve_map_placeholders(
    url: &str,
    prefix: &str,
    map: Option<&std::collections::BTreeMap<String, String>>,
) -> String {
    let pattern = format!("{{{prefix}.");
    let mut result = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(start) = rest.find(&pattern) {
        result.push_str(&rest[..start]);
        let after = &rest[start + pattern.len()..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                let value = map.and_then(|m| m.get(key)).cloned().unwrap_or_default();
                result.push_str(&value);
                rest = &after[end + 1..];
            }
            None => {
                result.push_str(&rest[start..]);
                return result;
            }
        }
    }
    result.push_str(rest);
    result
}

/// Substitute `{secret.<name>.<key>}` placeholders in a URL. The secret
/// must already be in the cache (use `first_uncached_secret` first to
/// trigger lazy fetches). Missing secrets/keys substitute as empty.
fn resolve_secret_placeholders(
    url: &str,
    namespace: &str,
    cache: &std::collections::HashMap<(String, String), std::collections::HashMap<String, String>>,
) -> String {
    let mut result = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(start) = rest.find("{secret.") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 8..];
        match after.find('}') {
            Some(end) => {
                let path = &after[..end];
                if let Some((sec_name, key)) = path.split_once('.') {
                    let value = cache
                        .get(&(namespace.to_string(), sec_name.to_string()))
                        .and_then(|m| m.get(key))
                        .cloned()
                        .unwrap_or_default();
                    result.push_str(&value);
                }
                rest = &after[end + 1..];
            }
            None => {
                result.push_str(&rest[start..]);
                return result;
            }
        }
    }
    result.push_str(rest);
    result
}

/// Scan a URL template for `{secret.<name>.<key>}` placeholders and
/// return the first secret name that isn't cached for `namespace` yet.
/// Returns `None` once every referenced secret is in the cache.
fn first_uncached_secret(
    url: &str,
    namespace: &str,
    cache: &std::collections::HashMap<(String, String), std::collections::HashMap<String, String>>,
) -> Option<String> {
    let mut rest = url;
    while let Some(start) = rest.find("{secret.") {
        let after = &rest[start + 8..];
        let end = after.find('}')?;
        let path = &after[..end];
        if let Some((sec_name, _)) = path.split_once('.') {
            let key = (namespace.to_string(), sec_name.to_string());
            if !cache.contains_key(&key) {
                return Some(sec_name.to_string());
            }
        }
        rest = &after[end + 1..];
    }
    None
}

fn lookup_output_path(outputs: &std::collections::HashMap<String, String>, path: &str) -> String {
    let mut parts = path.split('.');
    let head = match parts.next() {
        Some(h) => h,
        None => return String::new(),
    };
    let raw = match outputs.get(head) {
        Some(v) => v,
        None => return String::new(),
    };
    let remaining: Vec<&str> = parts.collect();
    if remaining.is_empty() {
        return raw.clone();
    }
    let json: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let mut cur = &json;
    for seg in &remaining {
        cur = match cur.get(*seg) {
            Some(v) => v,
            None => return String::new(),
        };
    }
    match cur {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        App, first_uncached_secret, resolve_map_placeholders, resolve_output_placeholders,
        resolve_secret_placeholders, resolve_var_placeholders, terraform_names_text,
    };
    use crate::action::{Action, AsyncKind, AsyncScope};
    use crate::config::Config;
    use crate::k8s::watcher::{create_gitrepo_store, create_ks_store, create_tf_store};
    use crate::state::store::{AppState, InputMode};
    use crossterm::event::Event;
    use std::collections::{BTreeMap, HashMap};
    use tokio::sync::mpsc;

    fn test_app() -> App {
        let (tf_store, _) = create_tf_store();
        let (ks_store, _) = create_ks_store();
        let (gr_store, _) = create_gitrepo_store();
        let state = AppState::new(
            tf_store,
            ks_store,
            gr_store,
            "test".into(),
            Config::default(),
        );
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        App::new_deferred(
            state,
            action_tx,
            action_rx,
            None,
            None,
            None,
            "flux-system".into(),
            None,
        )
    }

    #[test]
    fn bracketed_paste_maps_to_one_search_action() {
        let mut app = test_app();
        app.state.input_mode = InputMode::Search;

        let action = app.handle_crossterm_event(Event::Paste("cluster-long-name".into()));

        assert!(matches!(
            action,
            Some(Action::SearchPaste(text)) if text == "cluster-long-name"
        ));
    }

    #[tokio::test]
    async fn no_op_action_does_not_request_redraw() {
        let mut app = test_app();

        assert!(!app.dispatch(Action::None).await);
    }

    #[tokio::test]
    async fn stale_scoped_action_does_not_request_redraw() {
        let mut app = test_app();
        let stale_scope = AsyncScope::generation(1, AsyncKind::Connection);

        assert!(
            !app.dispatch(Action::scoped(stale_scope, Action::Resize(120, 40)))
                .await
        );
    }

    #[tokio::test]
    async fn dispatched_action_requests_redraw() {
        let mut app = test_app();

        assert!(app.dispatch(Action::Resize(120, 40)).await);
    }

    fn outputs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn meta(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn secret_cache(
        ns: &str,
        secret: &str,
        pairs: &[(&str, &str)],
    ) -> HashMap<(String, String), HashMap<String, String>> {
        let mut cache = HashMap::new();
        cache.insert((ns.to_string(), secret.to_string()), outputs(pairs));
        cache
    }

    #[test]
    fn secret_placeholder_substitutes_cached_value() {
        let cache = secret_cache(
            "ns1",
            "app-vars",
            &[("api_host", "https://api.example.com")],
        );
        let out =
            resolve_secret_placeholders("{secret.app-vars.api_host}/apps?search=42", "ns1", &cache);
        assert_eq!(out, "https://api.example.com/apps?search=42");
    }

    #[test]
    fn secret_placeholder_substitutes_empty_when_key_missing() {
        let cache = secret_cache("ns1", "app-vars", &[("other", "x")]);
        let out = resolve_secret_placeholders("{secret.app-vars.absent}/foo", "ns1", &cache);
        assert_eq!(out, "/foo");
    }

    #[test]
    fn first_uncached_secret_returns_missing_name() {
        let cache: HashMap<(String, String), HashMap<String, String>> = HashMap::new();
        assert_eq!(
            first_uncached_secret("{secret.app-vars.k}", "ns1", &cache),
            Some("app-vars".to_string())
        );
    }

    #[test]
    fn first_uncached_secret_skips_cached_entries() {
        let cache = secret_cache("ns1", "app-vars", &[("k", "v")]);
        // Only app-vars referenced and it's cached → None.
        assert_eq!(
            first_uncached_secret("{secret.app-vars.k}", "ns1", &cache),
            None
        );
        // Different secret name not cached → returns it.
        assert_eq!(
            first_uncached_secret("{secret.app-vars.k}/{secret.other.x}", "ns1", &cache),
            Some("other".to_string())
        );
    }

    #[test]
    fn flat_key_substitutes_value() {
        let out = outputs(&[("cluster_id", "12345")]);
        let result = resolve_output_placeholders("https://x/{output.cluster_id}", &out);
        assert_eq!(result, "https://x/12345");
    }

    #[test]
    fn nested_json_path_walks_object() {
        let out = outputs(&[("metadata", r#"{"tenant":"acme","region":"us-east"}"#)]);
        let result = resolve_output_placeholders(
            "https://x/{output.metadata.tenant}/{output.metadata.region}",
            &out,
        );
        assert_eq!(result, "https://x/acme/us-east");
    }

    #[test]
    fn missing_top_level_key_substitutes_empty() {
        let out = outputs(&[("foo", "bar")]);
        let result = resolve_output_placeholders("https://x/{output.absent}", &out);
        assert_eq!(result, "https://x/");
    }

    #[test]
    fn missing_nested_key_substitutes_empty() {
        let out = outputs(&[("metadata", r#"{"tenant":"acme"}"#)]);
        let result = resolve_output_placeholders("https://x/{output.metadata.absent}", &out);
        assert_eq!(result, "https://x/");
    }

    #[test]
    fn nested_path_on_non_json_value_substitutes_empty() {
        let out = outputs(&[("plain", "not-json")]);
        let result = resolve_output_placeholders("https://x/{output.plain.field}", &out);
        assert_eq!(result, "https://x/");
    }

    #[test]
    fn flat_lookup_on_non_json_value_works() {
        let out = outputs(&[("plain", "not-json")]);
        let result = resolve_output_placeholders("https://x/{output.plain}", &out);
        assert_eq!(result, "https://x/not-json");
    }

    #[test]
    fn non_string_json_leaf_is_stringified() {
        let out = outputs(&[("metadata", r#"{"clusterId":12345,"ready":true}"#)]);
        let result = resolve_output_placeholders(
            "https://x/{output.metadata.clusterId}/{output.metadata.ready}",
            &out,
        );
        assert_eq!(result, "https://x/12345/true");
    }

    #[test]
    fn unterminated_placeholder_is_left_as_is() {
        let out = outputs(&[("foo", "bar")]);
        let result = resolve_output_placeholders("https://x/{output.foo", &out);
        assert_eq!(result, "https://x/{output.foo");
    }

    #[test]
    fn url_without_placeholders_is_unchanged() {
        let out = outputs(&[("foo", "bar")]);
        let result = resolve_output_placeholders("https://x/static", &out);
        assert_eq!(result, "https://x/static");
    }

    #[test]
    fn multiple_placeholders_resolve_independently() {
        let out = outputs(&[("a", "1"), ("metadata", r#"{"b":"2"}"#)]);
        let result = resolve_output_placeholders("{output.a}-{output.metadata.b}-{output.a}", &out);
        assert_eq!(result, "1-2-1");
    }

    #[test]
    fn label_placeholder_substitutes_metadata_value() {
        let labels = meta(&[
            ("parent_cluster_name", "app-prod-01"),
            ("kustomize.toolkit.fluxcd.io/name", "external-resources"),
        ]);
        let result = resolve_map_placeholders(
            "https://gitlab.example.com/tree/main/prod/{label.parent_cluster_name}",
            "label",
            Some(&labels),
        );
        assert_eq!(
            result,
            "https://gitlab.example.com/tree/main/prod/app-prod-01"
        );
    }

    #[test]
    fn label_key_with_dots_and_slashes_works() {
        // K8s label keys frequently contain dots and slashes — they
        // must round-trip through the placeholder grammar unscathed.
        let labels = meta(&[("kustomize.toolkit.fluxcd.io/name", "external-resources")]);
        let result = resolve_map_placeholders(
            "https://x/{label.kustomize.toolkit.fluxcd.io/name}/y",
            "label",
            Some(&labels),
        );
        assert_eq!(result, "https://x/external-resources/y");
    }

    #[test]
    fn missing_label_substitutes_empty() {
        let labels = meta(&[("only", "x")]);
        let result = resolve_map_placeholders("a-{label.missing}-b", "label", Some(&labels));
        assert_eq!(result, "a--b");
    }

    #[test]
    fn label_placeholder_with_no_map_substitutes_empty() {
        // Resource doesn't exist / has no labels — placeholder still
        // resolves to empty so the URL is well-formed.
        let result = resolve_map_placeholders("a-{label.foo}-b", "label", None);
        assert_eq!(result, "a--b");
    }

    #[test]
    fn annotation_placeholder_uses_separate_prefix() {
        let annotations = meta(&[("module", "postgres-v2-cfg")]);
        let result = resolve_map_placeholders(
            "https://x/{annotation.module}",
            "annotation",
            Some(&annotations),
        );
        assert_eq!(result, "https://x/postgres-v2-cfg");
    }

    #[test]
    fn unterminated_label_placeholder_is_left_as_is() {
        let labels = meta(&[("foo", "bar")]);
        let result = resolve_map_placeholders("https://x/{label.foo", "label", Some(&labels));
        assert_eq!(result, "https://x/{label.foo");
    }

    // ---- {var.KEY} ----

    fn vars(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn var_placeholder_substitutes_from_vars() {
        let vars = vars(&[("grafana_host", "grafana-shared.example.net")]);
        let mut warned = std::collections::HashSet::new();
        let result = resolve_var_placeholders(
            "https://{var.grafana_host}/d/x?cluster={name}",
            &vars,
            "devcloud",
            &mut warned,
        );
        // Note: {name} isn't substituted by this helper — it's resolved
        // by the surrounding code in activate_shortcut.
        assert_eq!(
            result,
            "https://grafana-shared.example.net/d/x?cluster={name}"
        );
        assert!(
            warned.is_empty(),
            "no warning expected when the key is present"
        );
    }

    #[test]
    fn var_placeholder_substitutes_empty_when_missing() {
        let vars = vars(&[]);
        let mut warned = std::collections::HashSet::new();
        let result = resolve_var_placeholders("a-{var.missing}-b", &vars, "prod-ctx", &mut warned);
        assert_eq!(result, "a--b");
        // Warned exactly once for (context, key).
        assert!(warned.contains(&("prod-ctx".to_string(), "missing".to_string())));
        // Re-running with the same key shouldn't double-warn.
        let len_before = warned.len();
        let _ = resolve_var_placeholders("{var.missing}", &vars, "prod-ctx", &mut warned);
        assert_eq!(warned.len(), len_before, "warning must not duplicate");
    }

    #[test]
    fn var_placeholder_unterminated_is_left_as_is() {
        let vars = vars(&[("k", "v")]);
        let mut warned = std::collections::HashSet::new();
        let result = resolve_var_placeholders("https://x/{var.k", &vars, "ctx", &mut warned);
        assert_eq!(result, "https://x/{var.k");
    }

    #[test]
    fn var_placeholder_substitutes_multiple_occurrences() {
        let vars = vars(&[("host", "h.example"), ("env", "prod")]);
        let mut warned = std::collections::HashSet::new();
        let result =
            resolve_var_placeholders("{var.host}/{var.env}/{var.host}", &vars, "ctx", &mut warned);
        assert_eq!(result, "h.example/prod/h.example");
    }

    #[test]
    fn terraform_names_text_writes_one_name_per_line() {
        let items: Vec<crate::k8s::terraform::Terraform> = vec![
            serde_json::from_value(serde_json::json!({
                "apiVersion": "infra.contrib.fluxcd.io/v1alpha2",
                "kind": "Terraform",
                "metadata": {"name": "cluster-a", "namespace": "ns"},
                "spec": {"interval": "1m", "sourceRef": {"kind": "GitRepository", "name": "source"}}
            }))
            .expect("first Terraform should deserialize"),
            serde_json::from_value(serde_json::json!({
                "apiVersion": "infra.contrib.fluxcd.io/v1alpha2",
                "kind": "Terraform",
                "metadata": {"name": "cluster-b", "namespace": "ns"},
                "spec": {"interval": "1m", "sourceRef": {"kind": "GitRepository", "name": "source"}}
            }))
            .expect("second Terraform should deserialize"),
        ];

        assert_eq!(terraform_names_text(&items), "cluster-a\ncluster-b\n");
    }
}
