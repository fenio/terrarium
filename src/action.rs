use k8s_openapi::api::core::v1::Pod;

/// Wrapper for kube::Client that implements Debug.
#[derive(Clone)]
pub struct K8sClient(pub kube::Client);

impl std::fmt::Debug for K8sClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "K8sClient")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceKind {
    Terraform,
    Kustomization,
    Pod,
}

#[derive(Debug, Clone)]
pub enum Action {
    // Navigation
    Quit,
    NextTab,
    PrevTab,
    GoToTab(usize),
    SelectNext,
    SelectPrev,
    PageDown,
    PageUp,
    ScreenDown,
    ScreenUp,
    ScrollTop,
    ScrollBottom,
    ScrollLeft,
    ScrollRight,
    NextContainer,
    PrevContainer,
    Enter,
    Back,
    ToggleHelp,
    ToggleFailuresOnly,
    ToggleWaitingOnly,
    ToggleProgressingOnly,
    ToggleDeletingOnly,
    ToggleWrap,
    CycleSort,
    InvertSort,
    JumpToFirstFailure,

    // Filtering
    SearchStart,
    SearchPush(char),
    SearchPop,
    SearchConfirm,
    SearchCancel,
    ToggleSearchSuspend,

    // Viewer search
    ViewerSearchStart,
    ViewerSearchPush(char),
    ViewerSearchPop,
    ViewerSearchConfirm,
    ViewerSearchCancel,
    ViewerSearchNext,
    ViewerSearchPrev,

    // Namespace picker
    OpenNamespacePicker,
    NamespacePickerNext,
    NamespacePickerPrev,
    NamespacePickerSelect,
    NamespacePickerCancel,

    // Context picker (Ctrl-X) — switches the active kube-context
    OpenContextPicker,
    ContextPickerNext,
    ContextPickerPrev,
    ContextPickerSelect,
    ContextPickerCancel,
    /// Reconnect the app to the named context. Handled in the run loop
    /// (not plain dispatch) because it may suspend the TUI to run an
    /// interactive exec/OIDC credential plugin on a normal terminal.
    SwitchContext(String),

    // Shortcuts popup (overlays current view, scoped to a resource)
    OpenShortcutsPopup {
        namespace: String,
        name: String,
    },
    ShortcutsPopupNext,
    ShortcutsPopupPrev,
    ShortcutsPopupSelect,
    ShortcutsPopupCancel,

    // Bulk selection
    ToggleSelect,
    BulkReconcile,
    BulkSuspend,
    BulkResume,
    BulkApprovePlan,

    // K8s mutations (Terraform-specific)
    ApprovePlan {
        namespace: String,
        name: String,
    },
    Replan {
        namespace: String,
        name: String,
    },
    ForceUnlock {
        namespace: String,
        name: String,
    },
    ExecBreakTheGlass {
        namespace: String,
        name: String,
    },
    /// Clear `spec.breakTheGlass` after a persistent break-the-glass mode
    /// was left behind by a stuck or interrupted session.
    ResetBreakTheGlass {
        namespace: String,
        name: String,
    },
    DeleteResource {
        namespace: String,
        name: String,
    },
    KillRunner {
        namespace: String,
        name: String,
    },
    StreamRunnerLogs {
        namespace: String,
        name: String,
    },
    JumpToTerraformDetail {
        namespace: String,
        name: String,
    },
    StreamControllerLogs {
        namespace: String,
        pod_name: String,
    },
    OpenShortcut {
        namespace: String,
        name: String,
        shortcut_idx: usize,
    },
    FetchPlan {
        namespace: String,
        name: String,
        workspace: Option<String>,
    },

    // K8s mutations (shared TF + KS)
    Reconcile {
        kind: ResourceKind,
        namespace: String,
        name: String,
    },
    Suspend {
        kind: ResourceKind,
        namespace: String,
        name: String,
    },
    Resume {
        kind: ResourceKind,
        namespace: String,
        name: String,
    },

    // JSON / YAML resource view
    FetchJson {
        kind: ResourceKind,
        namespace: String,
        name: String,
    },
    FetchYaml {
        kind: ResourceKind,
        namespace: String,
        name: String,
    },

    // Outputs view (Terraform only)
    FetchOutputs {
        namespace: String,
        name: String,
    },
    OutputsFetched(String),
    OutputsFetchError(String),

    // Cached output key-values for detail view status panel
    DetailOutputsFetched {
        namespace: String,
        name: String,
        values: std::collections::HashMap<String, String>,
    },
    // Cached values from an arbitrary Secret (for {secret.X.Y} templates).
    SecretValuesFetched {
        namespace: String,
        secret_name: String,
        values: std::collections::HashMap<String, String>,
    },
    SecretValuesFetchError(String),
    JsonFetched(String),
    JsonFetchError(String),

    // Events view
    FetchEvents {
        kind: ResourceKind,
        namespace: String,
        name: String,
    },
    EventsFetched(String),
    EventsFetchError(String),

    // Conditions viewer (no fetch — sourced from in-memory store)
    ViewConditions {
        kind: ResourceKind,
        namespace: String,
        name: String,
    },

    // Save viewer content to file
    SaveViewerContent,

    // K8s data events
    TerraformStoreUpdated,
    KustomizationStoreUpdated,
    GitRepoStoreUpdated,
    RunnerPodsUpdated(Vec<Pod>),
    ControllerInfoUpdated(crate::state::store::ControllerInfo),
    RunnerLogsUpdated(std::collections::HashMap<(String, String), String>),

    // Log streaming
    LogChunkReceived(String),

    // CRD availability
    TerraformCrdMissing,
    KustomizationCrdMissing,
    GitRepoCrdMissing,

    // K8s client initialization
    K8sClientReady {
        client: K8sClient,
        context_name: String,
    },
    ConnectionError(String),
    DismissError,
    /// The `exec`/OIDC credential plugin failed to produce a token. Tears the
    /// connection down and stops the background tasks so kube-rs stops
    /// re-invoking the plugin (which would keep opening browser login tabs);
    /// the user re-authenticates deliberately with Ctrl-X.
    AuthExpired,
    /// A recurring background poller (e.g. runner pods) succeeded (`None`) or
    /// failed (`Some(msg)`). Drives a non-modal status-bar indicator so such
    /// failures are never silent, without stealing focus like the overlay.
    BackgroundError(Option<String>),

    // Async K8s action results
    K8sActionSuccess(String),
    K8sActionError(String),
    PlanFetched(String),
    PlanFetchError(String),

    // Mouse
    ToggleMouse,
    MouseSelect(usize),

    // Controller metrics panel (port-forward to /metrics)
    ToggleMetrics,
    MetricsSnapshotReceived(crate::k8s::metrics::MetricsSnapshot),
    MetricsFetchError(String),

    // UI events
    #[allow(dead_code)]
    Resize(u16, u16),
    ShowConfirmDialog(Box<Action>, String),
    ConfirmDialog(bool),
    /// Type-to-confirm dialog: (wrapped action, message, expected text the user
    /// must type to proceed). Used for destructive actions like deleting a
    /// Terraform object, which makes tofu-controller destroy managed infra.
    ShowTypedConfirmDialog(Box<Action>, String, String),
    ConfirmTypePush(char),
    ConfirmTypePop,
    ConfirmTypeSubmit,
    ConfirmTypeCancel,

    None,
}
