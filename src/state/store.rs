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

/// How long a row stays "visible" in a filtered list after the user
/// dispatches a K8s action on it. Long enough to watch the controller
/// pick the resource up and transition it, short enough that the list
/// settles back to honoring the active filter promptly.
pub const RECENTLY_ACTED_GRACE_SECS: u64 = 15;

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
    /// Type-to-confirm dialog for destructive actions: the user must type an
    /// expected string (e.g. the resource name) before the action fires.
    ConfirmType,
    Help,
    NamespacePicker,
    ContextPicker,
    ShortcutsPopup,
}

pub struct DialogState {
    pub wrapped_action: Action,
    pub message: String,
    /// When `Some`, this is a type-to-confirm dialog: the wrapped action only
    /// fires once `typed_input` matches this string exactly. `None` for a plain
    /// y/n confirmation.
    pub expected_input: Option<String>,
    /// What the user has typed so far in a type-to-confirm dialog.
    pub typed_input: String,
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
    Revision,
    LastApplied,
    Age,
}

impl SortColumn {
    pub fn next(self) -> Self {
        match self {
            SortColumn::Namespace => SortColumn::Name,
            SortColumn::Name => SortColumn::Ready,
            SortColumn::Ready => SortColumn::Revision,
            SortColumn::Revision => SortColumn::LastApplied,
            SortColumn::LastApplied => SortColumn::Age,
            SortColumn::Age => SortColumn::Namespace,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortColumn::Namespace => "namespace",
            SortColumn::Name => "name",
            SortColumn::Ready => "ready",
            SortColumn::Revision => "revision",
            SortColumn::LastApplied => "applied",
            SortColumn::Age => "age",
        }
    }
}

/// Compiled `when` filter for a shortcut. Built once at config load
/// from `config::When` so each render frame doesn't recompile regexes.
#[derive(Debug, Clone)]
pub struct CompiledWhen {
    pub name: Option<regex::Regex>,
    pub namespace: Option<regex::Regex>,
    pub context: Option<regex::Regex>,
}

impl CompiledWhen {
    /// True when this filter allows the given resource. All specified
    /// fields must match; missing fields impose no constraint.
    pub fn matches(&self, namespace: &str, name: &str, context: &str) -> bool {
        if let Some(re) = &self.name
            && !re.is_match(name)
        {
            return false;
        }
        if let Some(re) = &self.namespace
            && !re.is_match(namespace)
        {
            return false;
        }
        if let Some(re) = &self.context
            && !re.is_match(context)
        {
            return false;
        }
        true
    }
}

/// Sortable columns on the Runners tab. Kept separate from `SortColumn` because
/// the Runners view has no Ready/LastApplied — but does have Terraform and
/// Phase — so a shared enum would expose meaningless cycle entries.
/// PHASE and STATUS render the same underlying pod phase with different
/// styling, so only PHASE is in the cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerSortColumn {
    Namespace,
    Name,
    Terraform,
    Phase,
    Age,
}

impl RunnerSortColumn {
    pub fn next(self) -> Self {
        match self {
            RunnerSortColumn::Namespace => RunnerSortColumn::Name,
            RunnerSortColumn::Name => RunnerSortColumn::Terraform,
            RunnerSortColumn::Terraform => RunnerSortColumn::Phase,
            RunnerSortColumn::Phase => RunnerSortColumn::Age,
            RunnerSortColumn::Age => RunnerSortColumn::Namespace,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RunnerSortColumn::Namespace => "namespace",
            RunnerSortColumn::Name => "name",
            RunnerSortColumn::Terraform => "terraform",
            RunnerSortColumn::Phase => "phase",
            RunnerSortColumn::Age => "age",
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
    pub show_progressing_only: bool,
    pub show_drifting_only: bool,
    pub show_deleting_only: bool,
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
    pub runner_sort_column: RunnerSortColumn,
    pub sort_descending: bool,

    /// Set of (namespace, name) for bulk-selected resources
    pub bulk_selected: std::collections::HashSet<(String, String)>,

    /// Rows the user has recently acted on (Reconcile / Suspend / Resume /
    /// Approve / Replan / etc.). Each entry holds the time of dispatch and
    /// is pruned after `RECENTLY_ACTED_GRACE_SECS`. While present, the
    /// list-view filters treat the row as visible even if it no longer
    /// matches the active filter — so the user sees the resources they
    /// just acted on transition in place instead of vanishing.
    pub recently_acted: HashMap<(String, String), std::time::Instant>,

    pub tf_synced: bool,
    pub ks_synced: bool,
    pub gr_synced: bool,
    pub runners_synced: bool,
    pub tf_crd_missing: bool,
    pub ks_crd_missing: bool,
    pub gr_crd_missing: bool,

    /// Persistent connection error (shown on dashboard until resolved)
    pub connection_error: Option<String>,

    /// Set when the OIDC/exec credential plugin failed and we stopped the
    /// background tasks. Stays until a re-auth (Ctrl-X) reconnects, so we
    /// don't keep re-invoking the plugin and spawning browser login tabs.
    pub needs_reauth: bool,

    /// Non-modal background-poller error (e.g. runner listing failed).
    /// Shown as a status-bar ⚠ indicator; cleared when the poller recovers.
    pub background_error: Option<String>,

    // Namespace picker
    pub ns_picker_items: Vec<String>,
    pub ns_picker_selected: usize,

    // Context picker (Ctrl-X) — switches the whole app to another
    // kube-context from the switcher kubeconfig.
    pub ctx_picker_items: Vec<String>,
    pub ctx_picker_selected: usize,

    // Shortcuts popup (S key)
    /// (namespace, name) of the resource the popup was opened against.
    /// All shortcut URLs render with this resource's placeholders resolved.
    pub shortcuts_popup_resource: Option<(String, String)>,
    pub shortcuts_popup_selected: usize,
    /// Config indices of the shortcuts visible in the popup for the
    /// currently-open resource. Computed when the popup is opened so
    /// `when` regexes don't have to be re-evaluated per render frame.
    /// `shortcuts_popup_selected` indexes into this list, not the raw
    /// config vector.
    pub shortcuts_popup_visible: Vec<usize>,

    /// Compiled `when` filters parallel to `config.shortcuts`. Entry
    /// `i` is `None` when the shortcut has no filter (or its regex
    /// failed to compile — see config load warnings). Populated once
    /// at startup; never reallocated afterwards.
    pub compiled_shortcut_filters: Vec<Option<CompiledWhen>>,

    /// Compiled `[[context_vars]]` table: each entry's `match` regex
    /// paired with its variable map. Looked up by `vars_for_context`
    /// when resolving `{var.KEY}` placeholders in shortcut URLs.
    pub compiled_context_vars: Vec<(regex::Regex, std::collections::BTreeMap<String, String>)>,
    /// `{var.KEY}` lookups that have already been logged as missing for
    /// the current context. Used to throttle the warning to once per
    /// (context, key) so a shortcut activated repeatedly doesn't spam
    /// the tracing log.
    pub var_warned: std::collections::HashSet<(String, String)>,

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

    /// Screen geometry of the main body, used for mouse hit-testing.
    pub body_y: u16,
    pub body_height: u16,
    /// Screen x-ranges of the rendered tab labels, in tab order.
    pub tab_hit_ranges: Vec<(u16, u16)>,
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

/// Compile each shortcut's `when` filter to a runtime-checkable form.
/// On bad regex syntax the entry collapses to `None` (always matches)
/// with a stderr warning — consistent with the config loader's lenient
/// posture toward malformed entries.
fn compile_shortcut_filters(shortcuts: &[crate::config::Shortcut]) -> Vec<Option<CompiledWhen>> {
    shortcuts
        .iter()
        .map(|s| {
            let when = s.when.as_ref()?;
            let name = when
                .name
                .as_deref()
                .map(|r| compile_or_warn(r, "name", s.key, &s.label))
                .unwrap_or(None);
            let namespace = when
                .namespace
                .as_deref()
                .map(|r| compile_or_warn(r, "namespace", s.key, &s.label))
                .unwrap_or(None);
            let context = when
                .context
                .as_deref()
                .map(|r| compile_or_warn(r, "context", s.key, &s.label))
                .unwrap_or(None);
            // If every field is absent (or all failed to compile),
            // there's nothing to enforce — fall back to "always match".
            if name.is_none() && namespace.is_none() && context.is_none() {
                None
            } else {
                Some(CompiledWhen {
                    name,
                    namespace,
                    context,
                })
            }
        })
        .collect()
}

/// Compile each `[[context_vars]]` entry's `match` regex up-front.
/// Entries whose regex fails to compile are dropped with a warning,
/// matching the lenient posture of `compile_shortcut_filters` — the
/// remaining (valid) entries still function.
fn compile_context_vars(
    entries: &[crate::config::ContextVars],
) -> Vec<(regex::Regex, std::collections::BTreeMap<String, String>)> {
    entries
        .iter()
        .filter_map(|e| match regex::Regex::new(&e.match_) {
            Ok(re) => Some((re, e.vars.clone())),
            Err(err) => {
                eprintln!(
                    "Warning: context_vars entry has invalid `match` regex {:?}: {}",
                    e.match_, err
                );
                None
            }
        })
        .collect()
}

fn compile_or_warn(pattern: &str, field: &str, key: char, label: &str) -> Option<regex::Regex> {
    match regex::Regex::new(pattern) {
        Ok(re) => Some(re),
        Err(e) => {
            eprintln!(
                "Warning: shortcut '{label}' ({key}) has invalid `when.{field}` regex {pattern:?}: {e}"
            );
            None
        }
    }
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
        let compiled_shortcut_filters = compile_shortcut_filters(&config.shortcuts);
        let compiled_context_vars = compile_context_vars(&config.context_vars);
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
            show_progressing_only: false,
            show_drifting_only: false,
            show_deleting_only: false,
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
            runner_sort_column: RunnerSortColumn::Namespace,
            sort_descending: false,
            bulk_selected: std::collections::HashSet::new(),
            recently_acted: HashMap::new(),
            tf_synced: false,
            ks_synced: false,
            gr_synced: false,
            runners_synced: false,
            tf_crd_missing: false,
            ks_crd_missing: false,
            gr_crd_missing: false,
            connection_error: None,
            needs_reauth: false,
            background_error: None,
            ns_picker_items: Vec::new(),
            ns_picker_selected: 0,
            ctx_picker_items: Vec::new(),
            ctx_picker_selected: 0,
            shortcuts_popup_resource: None,
            shortcuts_popup_selected: 0,
            shortcuts_popup_visible: Vec::new(),
            compiled_shortcut_filters,
            compiled_context_vars,
            var_warned: std::collections::HashSet::new(),
            log_stream_handle: None,
            log_auto_follow: true,
            pending_dialog: None,
            flash_message: None,
            last_data_update: None,
            stable_tf_failures: (0, 0, std::time::Instant::now()),
            stable_ks_failures: (0, 0, std::time::Instant::now()),
            body_y: 6,
            body_height: 20,
            tab_hit_ranges: Vec::new(),
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

    /// True when the shortcut at `idx` applies to the given resource
    /// under the current kube context. Out-of-range indices and
    /// shortcuts without a `when` always match.
    pub fn shortcut_applies(&self, idx: usize, namespace: &str, name: &str) -> bool {
        match self.compiled_shortcut_filters.get(idx) {
            Some(Some(filter)) => filter.matches(namespace, name, &self.context_name),
            _ => true,
        }
    }

    /// Find the index of the first shortcut bound to `key` whose `when`
    /// matches the given resource. Returns `None` when no shortcut
    /// matches — direct-activation paths flash an error in that case.
    pub fn resolve_shortcut_for(&self, key: char, namespace: &str, name: &str) -> Option<usize> {
        self.config
            .shortcuts
            .iter()
            .enumerate()
            .find(|(i, s)| s.key == key && self.shortcut_applies(*i, namespace, name))
            .map(|(i, _)| i)
    }

    /// Indices into `config.shortcuts` of all entries applicable to the
    /// given resource — used by the popup to hide non-matching entries.
    ///
    /// Deduplicated by `key` with first-match-wins semantics, mirroring
    /// `resolve_shortcut_for`: when several shortcuts share a key (e.g.
    /// a narrow `when`-guarded override plus a no-condition catch-all),
    /// only the entry that would actually fire on key-press is shown.
    /// This lets a user stack any number of `when`-guarded variants on
    /// the same key without writing negative-match rules on the
    /// fallback.
    pub fn visible_shortcut_indices(&self, namespace: &str, name: &str) -> Vec<usize> {
        let mut seen = std::collections::HashSet::new();
        self.config
            .shortcuts
            .iter()
            .enumerate()
            .filter(|(i, s)| self.shortcut_applies(*i, namespace, name) && seen.insert(s.key))
            .map(|(i, _)| i)
            .collect()
    }

    /// Merge all `[[context_vars]]` entries whose `match` regex matches
    /// the active kube context, in declaration order, with first-match-
    /// wins semantics per key. This lets a narrow override entry sit
    /// above a broad `match = ".*"` catch-all that supplies defaults
    /// for keys it doesn't override. Returned by value (small map,
    /// built once per shortcut activation).
    pub fn vars_for_context(&self) -> std::collections::BTreeMap<String, String> {
        let mut merged: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (re, vars) in &self.compiled_context_vars {
            if !re.is_match(&self.context_name) {
                continue;
            }
            for (k, v) in vars {
                merged.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        merged
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
            self.switch_tab_to(tab);
        }
    }

    pub fn go_to_tab(&mut self, idx: usize) {
        if let Some(tab) = tab_from_index(idx, &self.config) {
            self.switch_tab_to(tab);
        }
    }

    pub fn prev_tab(&mut self) {
        let count = self.tab_count();
        let cur = self.active_tab.index(count);
        let prev = if cur == 0 { count - 1 } else { cur - 1 };
        if let Some(tab) = tab_from_index(prev, &self.config) {
            self.switch_tab_to(tab);
        }
    }

    /// Common tail of next_tab/prev_tab/go_to_tab. Clears the bulk
    /// selection on a real switch — the selection is per-tab in spirit
    /// (kinds differ between tabs), and carrying it across would let
    /// the next keypress act on resources the user can no longer see.
    fn switch_tab_to(&mut self, tab: TabKind) {
        if self.active_tab != tab {
            self.bulk_selected.clear();
        }
        self.active_tab = tab;
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

    /// Drop `recently_acted` entries that are past the grace window.
    /// Called on every tick so rows the user acted on stay visible long
    /// enough to see the controller pick them up, then fall back to
    /// honoring the active filter.
    pub fn prune_recently_acted(&mut self) {
        let cutoff = std::time::Duration::from_secs(RECENTLY_ACTED_GRACE_SECS);
        self.recently_acted
            .retain(|_, instant| instant.elapsed() <= cutoff);
    }

    /// Mark a (namespace, name) as recently acted on, restarting its
    /// grace window. Called from every K8s-action dispatch path —
    /// single-resource and bulk — so a reconciled row stays visible
    /// even if the active filter would normally hide it.
    pub fn mark_recently_acted(&mut self, namespace: &str, name: &str) {
        self.recently_acted.insert(
            (namespace.to_string(), name.to_string()),
            std::time::Instant::now(),
        );
    }

    /// True when (namespace, name) is in the grace window — used by the
    /// list filters as an OR-bypass and by the marker column.
    pub fn is_recently_acted(&self, namespace: &str, name: &str) -> bool {
        self.recently_acted
            .contains_key(&(namespace.to_string(), name.to_string()))
    }

    /// Reset all cluster-derived state before (re)connecting to a context.
    ///
    /// Keeps user config, keybindings, compiled shortcut/var tables and the
    /// active namespace filter; drops everything that described the
    /// previously-connected cluster (stores, caches, sync/CRD flags, open
    /// views) and aborts any live per-cluster background tasks. Fresh
    /// reflector stores are swapped in so the watchers spawned for the new
    /// connection write into handles the UI is actually reading.
    pub fn reset_for_reconnect(
        &mut self,
        tf_store: TfStore,
        ks_store: KsStore,
        gr_store: GitRepoStore,
    ) {
        // Tear down per-cluster background work tied to the old client.
        if let Some(h) = self.log_stream_handle.take() {
            h.abort();
        }
        if let Some(h) = self.metrics_task.take() {
            h.abort();
        }
        self.metrics_enabled = false;
        self.metrics_snapshot = None;
        self.metrics_prev = crate::k8s::metrics::PrevCounters::default();
        self.metrics_last_error = None;

        // Swap in the fresh reflector stores for the new connection.
        self.tf_store = tf_store;
        self.ks_store = ks_store;
        self.gr_store = gr_store;

        // Drop cluster-scoped caches.
        self.runner_pods.clear();
        self.runner_logs.clear();
        self.controller_info = ControllerInfo::default();
        self.cached_outputs = None;
        self.cached_secrets.clear();
        self.backlog_namespaces.clear();
        self.bulk_selected.clear();
        self.recently_acted.clear();
        self.var_warned.clear();
        self.last_data_update = None;

        // Reset connection status flags.
        self.tf_synced = false;
        self.ks_synced = false;
        self.gr_synced = false;
        self.runners_synced = false;
        self.tf_crd_missing = false;
        self.ks_crd_missing = false;
        self.gr_crd_missing = false;
        self.connection_error = None;
        self.needs_reauth = false;
        self.background_error = None;

        // Open views point at resources that may not exist on the new
        // cluster — return every tab to its root list.
        let total_tabs = tab_count(&self.config);
        self.view_stacks = (0..total_tabs)
            .map(|i| {
                let tab = tab_from_index(i, &self.config).unwrap_or(TabKind::Controller);
                vec![ViewState::List(tab)]
            })
            .collect();
        self.active_tab = TabKind::Controller;
        self.tf_table_state = TableState::default();
        self.ks_table_state = TableState::default();
        self.runner_table_state = TableState::default();
        self.backlog_table_state = TableState::default();
        for st in &mut self.custom_tab_states {
            *st = TableState::default();
        }
        self.input_mode = InputMode::Normal;
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
    use crate::config::{Shortcut, When};
    use crate::k8s::watcher::{create_gitrepo_store, create_ks_store, create_tf_store};

    fn make_state() -> AppState {
        let (tf, _) = create_tf_store();
        let (ks, _) = create_ks_store();
        let (gr, _) = create_gitrepo_store();
        AppState::new(tf, ks, gr, "test-ctx".to_string(), Config::default())
    }

    fn shortcut(key: char, label: &str, when: Option<When>) -> Shortcut {
        Shortcut {
            key,
            label: label.into(),
            description: None,
            group: None,
            url: Some(format!("https://example.com/{label}")),
            when,
            children: Vec::new(),
        }
    }

    fn state_with_shortcuts(shortcuts: Vec<Shortcut>) -> AppState {
        let config = Config {
            shortcuts,
            ..Config::default()
        };
        let (tf, _) = create_tf_store();
        let (ks, _) = create_ks_store();
        let (gr, _) = create_gitrepo_store();
        AppState::new(tf, ks, gr, "test-ctx".to_string(), config)
    }

    #[test]
    fn resolve_shortcut_picks_first_matching_when() {
        let state = state_with_shortcuts(vec![
            shortcut(
                'g',
                "clusters",
                Some(When {
                    name: Some("^cluster-".into()),
                    namespace: None,
                    context: None,
                }),
            ),
            shortcut(
                'g',
                "gtm",
                Some(When {
                    name: Some("^gtm-automation-".into()),
                    namespace: None,
                    context: None,
                }),
            ),
            // Fallback with no when — should win for anything that
            // doesn't match the patterns above.
            shortcut('g', "fallback", None),
        ]);

        assert_eq!(
            state.resolve_shortcut_for('g', "ns", "cluster-us-ord-tsdb-aclp01-prod"),
            Some(0),
            "cluster-* should resolve to the clusters shortcut"
        );
        assert_eq!(
            state.resolve_shortcut_for('g', "ns", "gtm-automation-acme"),
            Some(1),
            "gtm-* should resolve to the gtm shortcut"
        );
        assert_eq!(
            state.resolve_shortcut_for('g', "ns", "psv2-cfg-cloudlogs01-grafana-xyz"),
            Some(2),
            "everything else should fall through to the fallback shortcut"
        );
    }

    #[test]
    fn visible_shortcut_indices_dedupes_by_key_first_match_wins() {
        // Three shortcuts share key 'g': two narrow `when`-guarded
        // overrides and a catch-all. The popup must show only the
        // entry that would actually fire on key-press, so users don't
        // need negative-match rules on the catch-all to suppress it.
        let state = state_with_shortcuts(vec![
            shortcut(
                'g',
                "clusters",
                Some(When {
                    name: Some("^cluster-".into()),
                    namespace: None,
                    context: None,
                }),
            ),
            shortcut(
                'g',
                "gtm",
                Some(When {
                    name: Some("^gtm-".into()),
                    namespace: None,
                    context: None,
                }),
            ),
            shortcut('g', "fallback", None),
            // A second key not involved in deduping — proves we don't
            // accidentally collapse across keys.
            shortcut('h', "help", None),
        ]);

        // cluster-* → only the clusters entry + the unrelated 'h'.
        assert_eq!(
            state.visible_shortcut_indices("ns", "cluster-foo"),
            vec![0, 3],
            "matching `when` should hide later same-key entries (including catch-all)"
        );
        // gtm-* → only the gtm entry + 'h'.
        assert_eq!(state.visible_shortcut_indices("ns", "gtm-bar"), vec![1, 3],);
        // No narrow match → catch-all wins, narrow entries are filtered
        // out by their own `when`, so the visible set is just [2, 3].
        assert_eq!(
            state.visible_shortcut_indices("ns", "something-else"),
            vec![2, 3],
        );
    }

    #[test]
    fn resolve_shortcut_returns_none_when_nothing_matches() {
        let state = state_with_shortcuts(vec![shortcut(
            'g',
            "clusters",
            Some(When {
                name: Some("^cluster-".into()),
                namespace: None,
                context: None,
            }),
        )]);
        assert_eq!(
            state.resolve_shortcut_for('g', "ns", "gtm-automation-acme"),
            None,
        );
    }

    #[test]
    fn when_namespace_filter_is_anded_with_name() {
        let state = state_with_shortcuts(vec![shortcut(
            'g',
            "prod-clusters",
            Some(When {
                name: Some("^cluster-".into()),
                namespace: Some("^flux-prod-".into()),
                context: None,
            }),
        )]);
        assert_eq!(
            state.resolve_shortcut_for('g', "flux-prod-shared", "cluster-us-ord-foo"),
            Some(0)
        );
        // Wrong namespace — must not match.
        assert_eq!(
            state.resolve_shortcut_for('g', "flux-stag-shared", "cluster-us-ord-foo"),
            None
        );
        // Right namespace, wrong name — must not match either.
        assert_eq!(
            state.resolve_shortcut_for('g', "flux-prod-shared", "gtm-automation-acme"),
            None
        );
    }

    #[test]
    fn invalid_regex_falls_back_to_always_match() {
        // A bad regex should not crash; the entry should just match
        // anything (lenient config behavior consistent with the rest of
        // the loader).
        let state = state_with_shortcuts(vec![shortcut(
            'g',
            "broken",
            Some(When {
                name: Some("[bad-regex".into()),
                namespace: None,
                context: None,
            }),
        )]);
        assert_eq!(state.resolve_shortcut_for('g', "ns", "anything"), Some(0));
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
    fn recently_acted_pruning_drops_old_entries_only() {
        let mut state = make_state();
        state.mark_recently_acted("ns", "fresh");
        // Antedate one entry past the grace window.
        let stale_key = ("ns".to_string(), "stale".to_string());
        state.recently_acted.insert(
            stale_key.clone(),
            std::time::Instant::now()
                - std::time::Duration::from_secs(RECENTLY_ACTED_GRACE_SECS + 5),
        );
        assert_eq!(state.recently_acted.len(), 2);
        state.prune_recently_acted();
        assert!(state.is_recently_acted("ns", "fresh"));
        assert!(!state.is_recently_acted("ns", "stale"));
        assert!(!state.recently_acted.contains_key(&stale_key));
    }

    #[test]
    fn switching_tabs_clears_bulk_selection() {
        let mut state = make_state();
        state.bulk_selected.insert(("ns".into(), "name".into()));
        assert_eq!(state.bulk_selected.len(), 1);

        // Switching to a different tab clears the selection — carrying
        // it across would let the next keypress act on resources the
        // user can no longer see.
        state.next_tab();
        assert!(
            state.bulk_selected.is_empty(),
            "selection must clear on tab switch"
        );

        // No-op "switch" to the same tab must NOT clear (helps keep
        // accidental re-selections from costing the user their work).
        state.bulk_selected.insert(("ns".into(), "name".into()));
        let cur = state.active_tab.clone();
        let idx = cur.index(state.tab_count());
        state.go_to_tab(idx);
        assert_eq!(
            state.bulk_selected.len(),
            1,
            "same-tab go_to_tab must not clear selection"
        );
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

    // ---- context_vars + when.context ----

    fn state_with_context_and_shortcuts(context: &str, shortcuts: Vec<Shortcut>) -> AppState {
        let config = Config {
            shortcuts,
            ..Config::default()
        };
        let (tf, _) = create_tf_store();
        let (ks, _) = create_ks_store();
        let (gr, _) = create_gitrepo_store();
        AppState::new(tf, ks, gr, context.to_string(), config)
    }

    fn state_with_context_vars(context: &str, entries: Vec<(&str, Vec<(&str, &str)>)>) -> AppState {
        let context_vars = entries
            .into_iter()
            .map(|(m, vars)| crate::config::ContextVars {
                match_: m.to_string(),
                vars: vars
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            })
            .collect();
        let config = Config {
            context_vars,
            ..Config::default()
        };
        let (tf, _) = create_tf_store();
        let (ks, _) = create_ks_store();
        let (gr, _) = create_gitrepo_store();
        AppState::new(tf, ks, gr, context.to_string(), config)
    }

    #[test]
    fn compiled_when_matches_by_context() {
        let state = state_with_context_and_shortcuts(
            "pl-labkrk-2-flux-devcloud-01",
            vec![
                shortcut(
                    'b',
                    "grafana-devcloud",
                    Some(When {
                        name: None,
                        namespace: None,
                        context: Some("devcloud".into()),
                    }),
                ),
                shortcut('b', "grafana-prod", None),
            ],
        );
        assert_eq!(
            state.resolve_shortcut_for('b', "ns", "anything"),
            Some(0),
            "devcloud-context should pick the devcloud-gated shortcut first"
        );

        let state_prod = state_with_context_and_shortcuts(
            "prod-lax-01",
            vec![
                shortcut(
                    'b',
                    "grafana-devcloud",
                    Some(When {
                        name: None,
                        namespace: None,
                        context: Some("devcloud".into()),
                    }),
                ),
                shortcut('b', "grafana-prod", None),
            ],
        );
        assert_eq!(
            state_prod.resolve_shortcut_for('b', "ns", "anything"),
            Some(1),
            "non-devcloud context should fall through to the unfiltered shortcut"
        );
    }

    #[test]
    fn vars_for_context_merges_in_declaration_order() {
        // Narrow override sits above a broad catch-all. First match
        // per key wins, but keys the override doesn't define should
        // still come from the catch-all.
        let state = state_with_context_vars(
            "pl-labkrk-2-flux-devcloud-01",
            vec![
                ("devcloud", vec![("grafana_host", "grafana-shared.x.net")]),
                (
                    ".*",
                    vec![
                        ("grafana_host", "grafana-prod.x.net"),
                        ("linode_host", "admin.linode.com"),
                    ],
                ),
            ],
        );
        let merged = state.vars_for_context();
        assert_eq!(
            merged.get("grafana_host").map(String::as_str),
            Some("grafana-shared.x.net"),
            "override entry should win"
        );
        assert_eq!(
            merged.get("linode_host").map(String::as_str),
            Some("admin.linode.com"),
            "catch-all should fill in keys the override omits"
        );
    }

    #[test]
    fn vars_for_context_returns_empty_when_no_match() {
        let state =
            state_with_context_vars("prod-lax-01", vec![("^staging-", vec![("env", "stg")])]);
        assert!(
            state.vars_for_context().is_empty(),
            "no matching [[context_vars]] entry should yield an empty map"
        );
    }
}
