use std::collections::{BTreeSet, HashMap};

use k8s_openapi::api::core::v1::Pod;
use ratatui::widgets::TableState;

use crate::action::Action;
use crate::config::Config;
use crate::k8s::metrics::{MetricsSnapshot, PrevCounters};
use crate::k8s::watcher::{GitRepoStore, KsStore, TfStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabKind {
    Controller,
    Terraform,
    Kustomizations,
    Runners,
    /// Index into Config::custom_tabs
    CustomTab(usize),
}

impl TabKind {
    pub fn index(&self, _total_tab_count: usize) -> usize {
        match self {
            TabKind::Controller => 0,
            TabKind::Terraform => 1,
            TabKind::Kustomizations => 2,
            TabKind::Runners => 3,
            TabKind::CustomTab(i) => 4 + i,
        }
    }
}

/// Total number of tabs given a config.
pub fn tab_count(config: &Config) -> usize {
    4 + config.custom_tabs.len()
}

/// Map an index to a TabKind.
pub fn tab_from_index(idx: usize, config: &Config) -> Option<TabKind> {
    match idx {
        0 => Some(TabKind::Controller),
        1 => Some(TabKind::Terraform),
        2 => Some(TabKind::Kustomizations),
        3 => Some(TabKind::Runners),
        i if i >= 4 && i < tab_count(config) => Some(TabKind::CustomTab(i - 4)),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub enum ViewState {
    List(TabKind),
    TerraformDetail {
        namespace: String,
        name: String,
    },
    KustomizationDetail {
        namespace: String,
        name: String,
    },
    PlanViewer {
        content: String,
    },
    JsonViewer {
        content: String,
    },
    EventsViewer {
        content: String,
    },
    OutputsViewer {
        content: String,
    },
    ConditionsViewer {
        content: String,
    },
    LogViewer {
        namespace: String,
        pod_name: String,
        containers: Vec<String>,
        active_container: usize,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    ViewerSearch,
    Confirm,
    Help,
    NamespacePicker,
    ShortcutsPopup,
}

pub struct DialogState {
    pub wrapped_action: Action,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct ControllerInfo {
    // Deployment
    pub deploy_name: String,
    pub deploy_namespace: String,
    pub replicas_desired: i32,
    pub replicas_ready: i32,
    pub image: String,
    /// --concurrent flag from controller args (max parallel runners)
    pub max_concurrent: Option<i32>,
    // Pods
    pub pods: Vec<ControllerPodInfo>,
    // Error if we couldn't fetch
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ControllerPodInfo {
    pub name: String,
    pub phase: String,
    pub ready: bool,
    pub restarts: i32,
    pub age: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Namespace,
    Name,
    Ready,
    LastApplied,
    Age,
}

impl SortColumn {
    pub fn next(self) -> Self {
        match self {
            SortColumn::Namespace => SortColumn::Name,
            SortColumn::Name => SortColumn::Ready,
            SortColumn::Ready => SortColumn::LastApplied,
            SortColumn::LastApplied => SortColumn::Age,
            SortColumn::Age => SortColumn::Namespace,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortColumn::Namespace => "namespace",
            SortColumn::Name => "name",
            SortColumn::Ready => "ready",
            SortColumn::LastApplied => "applied",
            SortColumn::Age => "age",
        }
    }
}

pub struct AppState {
    pub config: Config,

    pub tf_store: TfStore,
    pub ks_store: KsStore,
    pub gr_store: GitRepoStore,
    pub runner_pods: Vec<Pod>,
    /// Cached logs for active runners, keyed by (namespace, terraform-resource-name)
    pub runner_logs: HashMap<(String, String), String>,
    pub controller_info: ControllerInfo,

    /// Cached output values for the detail view: (namespace, name) -> key-value pairs
    pub cached_outputs: Option<((String, String), HashMap<String, String>)>,

    /// Cached values from arbitrary Secrets, keyed by (namespace, secret_name).
    /// Populated lazily by `{secret.X.Y}` template placeholders so the next
    /// activation of the same shortcut doesn't refetch.
    pub cached_secrets: HashMap<(String, String), HashMap<String, String>>,

    pub context_name: String,

    pub active_tab: TabKind,
    /// Each tab keeps its own view stack, so jumping away to another tab
    /// and coming back resumes wherever the user was — including any
    /// open detail or viewer. Indexed by `TabKind::index(tab_count)`.
    pub view_stacks: Vec<Vec<ViewState>>,
    pub namespace_filter: Option<String>,
    pub search_query: String,
    pub search_suspended: bool,
    pub show_failures_only: bool,
    pub show_waiting_only: bool,
    pub input_mode: InputMode,

    pub tf_table_state: TableState,
    pub ks_table_state: TableState,
    pub runner_table_state: TableState,
    pub backlog_table_state: TableState,
    /// Table states for custom tabs, indexed by custom tab index.
    pub custom_tab_states: Vec<TableState>,
    /// Cached backlog entries: (namespace, waiting, failing, total), sorted by total stale desc.
    pub backlog_namespaces: Vec<(String, usize, usize, usize)>,

    pub plan_scroll: usize,
    pub horizontal_scroll: usize,
    pub viewer_wrap: bool,

    pub viewer_search_query: String,
    pub viewer_search_matches: Vec<usize>,
    pub viewer_search_index: usize,

    pub sort_column: SortColumn,
    pub sort_descending: bool,

    /// Set of (namespace, name) for bulk-selected resources
    pub bulk_selected: std::collections::HashSet<(String, String)>,

    pub tf_synced: bool,
    pub ks_synced: bool,
    pub gr_synced: bool,
    pub runners_synced: bool,
    pub tf_crd_missing: bool,
    pub ks_crd_missing: bool,
    pub gr_crd_missing: bool,

    /// Persistent connection error (shown on dashboard until resolved)
    pub connection_error: Option<String>,

    // Namespace picker
    pub ns_picker_items: Vec<String>,
    pub ns_picker_selected: usize,

    // Shortcuts popup (S key)
    /// (namespace, name) of the resource the popup was opened against.
    /// All shortcut URLs render with this resource's placeholders resolved.
    pub shortcuts_popup_resource: Option<(String, String)>,
    pub shortcuts_popup_selected: usize,

    // Log streaming
    pub log_stream_handle: Option<tokio::task::JoinHandle<()>>,
    /// When true, log viewer auto-scrolls to bottom on new chunks.
    /// Disabled by scrolling up; re-enabled by G (ScrollBottom).
    pub log_auto_follow: bool,

    pub pending_dialog: Option<DialogState>,
    pub flash_message: Option<(String, std::time::Instant, FlashKind)>,

    /// When K8s data was last received (from any watcher/poller).
    pub last_data_update: Option<std::time::Instant>,

    /// Stabilized failure counts for header display (avoids blinking).
    /// (displayed_count, raw_count, time_raw_count_was_first_seen)
    pub stable_tf_failures: (usize, usize, std::time::Instant),
    pub stable_ks_failures: (usize, usize, std::time::Instant),

    pub body_height: u16,
    pub mouse_enabled: bool,
    pub tick_count: usize,

    /// Whether `tfctl` is on $PATH (probed once at startup).
    /// Replan and Break-the-Glass shell out to tfctl, so we surface a
    /// warning when it's missing rather than failing silently at use.
    pub tfctl_available: bool,

    /// Whether the on-demand controller metrics panel is enabled.
    pub metrics_enabled: bool,
    /// Most recent metrics snapshot (None if never fetched or after disable).
    pub metrics_snapshot: Option<MetricsSnapshot>,
    /// Previous counter values, kept across fetches to compute per-min rates.
    pub metrics_prev: PrevCounters,
    /// Last error from the metrics fetcher (cleared on successful fetch).
    pub metrics_last_error: Option<String>,
    /// Background task that fetches metrics on a timer; aborted on disable.
    pub metrics_task: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlashKind {
    Success,
    Error,
}

impl AppState {
    pub fn new(
        tf_store: TfStore,
        ks_store: KsStore,
        gr_store: GitRepoStore,
        context_name: String,
        config: Config,
    ) -> Self {
        let custom_tab_count = config.custom_tabs.len();
        let total_tabs = tab_count(&config);
        let view_stacks: Vec<Vec<ViewState>> = (0..total_tabs)
            .map(|i| {
                let tab = tab_from_index(i, &config).unwrap_or(TabKind::Controller);
                vec![ViewState::List(tab)]
            })
            .collect();
        Self {
            config,
            tf_store,
            ks_store,
            gr_store,
            runner_pods: Vec::new(),
            runner_logs: HashMap::new(),
            controller_info: ControllerInfo::default(),
            cached_outputs: None,
            cached_secrets: HashMap::new(),
            context_name,
            active_tab: TabKind::Controller,
            view_stacks,
            namespace_filter: None,
            search_query: String::new(),
            search_suspended: false,
            show_failures_only: false,
            show_waiting_only: false,
            input_mode: InputMode::Normal,
            tf_table_state: TableState::default(),
            ks_table_state: TableState::default(),
            runner_table_state: TableState::default(),
            backlog_table_state: TableState::default(),
            custom_tab_states: (0..custom_tab_count)
                .map(|_| TableState::default())
                .collect(),
            backlog_namespaces: Vec::new(),
            plan_scroll: 0,
            horizontal_scroll: 0,
            viewer_wrap: false,
            viewer_search_query: String::new(),
            viewer_search_matches: Vec::new(),
            viewer_search_index: 0,
            sort_column: SortColumn::Name,
            sort_descending: false,
            bulk_selected: std::collections::HashSet::new(),
            tf_synced: false,
            ks_synced: false,
            gr_synced: false,
            runners_synced: false,
            tf_crd_missing: false,
            ks_crd_missing: false,
            gr_crd_missing: false,
            connection_error: None,
            ns_picker_items: Vec::new(),
            ns_picker_selected: 0,
            shortcuts_popup_resource: None,
            shortcuts_popup_selected: 0,
            log_stream_handle: None,
            log_auto_follow: true,
            pending_dialog: None,
            flash_message: None,
            last_data_update: None,
            stable_tf_failures: (0, 0, std::time::Instant::now()),
            stable_ks_failures: (0, 0, std::time::Instant::now()),
            body_height: 20,
            mouse_enabled: false,
            tick_count: 0,
            tfctl_available: false,
            metrics_enabled: false,
            metrics_snapshot: None,
            metrics_prev: PrevCounters::default(),
            metrics_last_error: None,
            metrics_task: None,
        }
    }

    /// Returns the search query used for filtering. Empty when search is suspended.
    pub fn effective_search_query(&self) -> &str {
        if self.search_suspended {
            ""
        } else {
            &self.search_query
        }
    }

    fn active_tab_idx(&self) -> usize {
        let count = self.tab_count();
        self.active_tab.index(count).min(count.saturating_sub(1))
    }

    pub fn current_view_stack(&self) -> &Vec<ViewState> {
        let idx = self.active_tab_idx();
        &self.view_stacks[idx]
    }

    pub fn current_view_stack_mut(&mut self) -> &mut Vec<ViewState> {
        let idx = self.active_tab_idx();
        &mut self.view_stacks[idx]
    }

    pub fn current_view(&self) -> &ViewState {
        self.current_view_stack()
            .last()
            .unwrap_or(&ViewState::List(TabKind::Terraform))
    }

    pub fn tab_count(&self) -> usize {
        tab_count(&self.config)
    }

    /// Find the (only) tab stack that has a LogViewer at its top, if any.
    /// Used to route log chunks to a viewer that may not be on the active
    /// tab when the user has jumped away.
    pub fn log_viewer_mut(&mut self) -> Option<&mut ViewState> {
        for stack in &mut self.view_stacks {
            if matches!(stack.last(), Some(ViewState::LogViewer { .. })) {
                return stack.last_mut();
            }
        }
        None
    }

    pub fn next_tab(&mut self) {
        let count = self.tab_count();
        let cur = self.active_tab.index(count);
        let next = (cur + 1) % count;
        if let Some(tab) = tab_from_index(next, &self.config) {
            self.active_tab = tab;
        }
    }

    pub fn go_to_tab(&mut self, idx: usize) {
        if let Some(tab) = tab_from_index(idx, &self.config) {
            self.active_tab = tab;
        }
    }

    pub fn prev_tab(&mut self) {
        let count = self.tab_count();
        let cur = self.active_tab.index(count);
        let prev = if cur == 0 { count - 1 } else { cur - 1 };
        if let Some(tab) = tab_from_index(prev, &self.config) {
            self.active_tab = tab;
        }
    }

    pub fn current_table_state(&mut self) -> &mut TableState {
        match &self.active_tab {
            TabKind::Controller => &mut self.backlog_table_state,
            TabKind::Terraform => &mut self.tf_table_state,
            TabKind::Kustomizations => &mut self.ks_table_state,
            TabKind::Runners => &mut self.runner_table_state,
            TabKind::CustomTab(i) => &mut self.custom_tab_states[*i],
        }
    }

    /// Returns a stabilized failure count. The displayed value only changes
    /// if the raw count has been stable for at least `hold` duration.
    /// This prevents blinking when resources briefly transition through NotReady.
    fn stabilize(
        stable: &mut (usize, usize, std::time::Instant),
        raw: usize,
        hold: std::time::Duration,
    ) -> usize {
        let (displayed, last_raw, since) = stable;
        if raw == *displayed {
            // Raw matches displayed — keep it, reset tracking
            *last_raw = raw;
            *since = std::time::Instant::now();
            return *displayed;
        }
        if raw != *last_raw {
            // Raw changed to a new value — start tracking
            *last_raw = raw;
            *since = std::time::Instant::now();
        } else if since.elapsed() >= hold {
            // Raw has been stable long enough — adopt it
            *displayed = raw;
        }
        *displayed
    }

    pub fn stabilized_tf_failures(&mut self, raw: usize) -> usize {
        Self::stabilize(
            &mut self.stable_tf_failures,
            raw,
            std::time::Duration::from_secs(5),
        )
    }

    pub fn stabilized_ks_failures(&mut self, raw: usize) -> usize {
        Self::stabilize(
            &mut self.stable_ks_failures,
            raw,
            std::time::Duration::from_secs(5),
        )
    }

    pub fn expire_flash(&mut self) {
        if let Some((_, instant, _)) = &self.flash_message
            && instant.elapsed() > std::time::Duration::from_secs(5)
        {
            self.flash_message = None;
        }
    }

    /// Collect unique namespaces from TF and KS stores for the namespace picker.
    pub fn collect_namespaces(&self) -> Vec<String> {
        let mut namespaces = BTreeSet::new();
        for tf in self.tf_store.state().iter() {
            if let Some(ns) = &tf.metadata.namespace {
                namespaces.insert(ns.clone());
            }
        }
        for ks in self.ks_store.state().iter() {
            if let Some(ns) = &ks.metadata.namespace {
                namespaces.insert(ns.clone());
            }
        }
        let mut result = vec!["(all namespaces)".to_string()];
        result.extend(namespaces);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::k8s::watcher::{create_gitrepo_store, create_ks_store, create_tf_store};

    fn make_state() -> AppState {
        let (tf, _) = create_tf_store();
        let (ks, _) = create_ks_store();
        let (gr, _) = create_gitrepo_store();
        AppState::new(tf, ks, gr, "test-ctx".to_string(), Config::default())
    }

    #[test]
    fn new_creates_one_view_stack_per_tab_each_rooted_in_list() {
        let state = make_state();
        assert_eq!(state.view_stacks.len(), state.tab_count());
        for (i, stack) in state.view_stacks.iter().enumerate() {
            assert_eq!(stack.len(), 1, "tab {i} stack should have only its root");
            let expected = tab_from_index(i, &state.config).unwrap();
            match &stack[0] {
                ViewState::List(t) => assert_eq!(*t, expected, "tab {i} root mismatch"),
                other => panic!("tab {i} root should be List, got {other:?}"),
            }
        }
    }

    #[test]
    fn next_and_prev_tab_change_active_without_touching_stacks() {
        let mut state = make_state();
        state.current_view_stack_mut().push(ViewState::JsonViewer {
            content: "x".into(),
        });
        let depths_before: Vec<usize> = state.view_stacks.iter().map(Vec::len).collect();
        let active_before = state.active_tab.clone();

        state.next_tab();
        assert_ne!(
            state.active_tab, active_before,
            "next_tab must change active"
        );
        let depths_after: Vec<usize> = state.view_stacks.iter().map(Vec::len).collect();
        assert_eq!(
            depths_before, depths_after,
            "tab nav must not mutate stacks"
        );

        state.prev_tab();
        assert_eq!(state.active_tab, active_before, "prev_tab should land back");
    }

    #[test]
    fn pushed_view_survives_round_trip_across_tabs() {
        let mut state = make_state();
        state.current_view_stack_mut().push(ViewState::JsonViewer {
            content: "preserved".into(),
        });
        match state.current_view() {
            ViewState::JsonViewer { content } => assert_eq!(content, "preserved"),
            other => panic!("expected JsonViewer at start, got {other:?}"),
        }

        // Hop to another tab and back.
        state.go_to_tab(1);
        assert!(matches!(
            state.current_view(),
            ViewState::List(TabKind::Terraform)
        ));
        state.go_to_tab(0);
        match state.current_view() {
            ViewState::JsonViewer { content } => assert_eq!(content, "preserved"),
            other => panic!("view should be restored, got {other:?}"),
        }
    }

    #[test]
    fn log_viewer_mut_finds_log_viewer_on_inactive_tab() {
        let mut state = make_state();
        let runners_idx = TabKind::Runners.index(state.tab_count());
        state.view_stacks[runners_idx].push(ViewState::LogViewer {
            namespace: "ns".into(),
            pod_name: "pod".into(),
            containers: vec!["c".into()],
            active_container: 0,
            content: String::new(),
        });
        // Active tab is still Controller (unchanged).
        assert_eq!(state.active_tab, TabKind::Controller);
        assert!(matches!(
            state.current_view(),
            ViewState::List(TabKind::Controller)
        ));
        // …but the streamed-into LogViewer is still findable for chunk routing.
        assert!(matches!(
            state.log_viewer_mut(),
            Some(ViewState::LogViewer { .. })
        ));
    }
}
