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

    // Bulk selection
    ToggleSelect,
    #[allow(dead_code)]
    BulkReconcile,
    #[allow(dead_code)]
    BulkSuspend,
    #[allow(dead_code)]
    BulkResume,

    // K8s mutations (Terraform-specific)
    ApprovePlan { namespace: String, name: String },
    Replan { namespace: String, name: String },
    ForceUnlock { namespace: String, name: String },
    ExecBreakTheGlass { namespace: String, name: String },
    DeleteResource { namespace: String, name: String },
    KillRunner { namespace: String, name: String },
    StreamRunnerLogs { namespace: String, name: String },
    JumpToTerraformDetail { namespace: String, name: String },
    StreamControllerLogs { namespace: String, pod_name: String },
    OpenShortcut { namespace: String, name: String, shortcut_idx: usize },
    FetchPlan { namespace: String, name: String, workspace: Option<String> },

    // K8s mutations (shared TF + KS)
    Reconcile { kind: ResourceKind, namespace: String, name: String },
    Suspend { kind: ResourceKind, namespace: String, name: String },
    Resume { kind: ResourceKind, namespace: String, name: String },

    // JSON / YAML resource view
    FetchJson { kind: ResourceKind, namespace: String, name: String },
    FetchYaml { kind: ResourceKind, namespace: String, name: String },

    // Outputs view (Terraform only)
    FetchOutputs { namespace: String, name: String },
    OutputsFetched(String),
    OutputsFetchError(String),

    // Cached output key-values for detail view status panel
    DetailOutputsFetched {
        namespace: String,
        name: String,
        values: std::collections::HashMap<String, String>,
    },
    JsonFetched(String),
    JsonFetchError(String),

    // Events view
    FetchEvents { kind: ResourceKind, namespace: String, name: String },
    EventsFetched(String),
    EventsFetchError(String),

    // Save viewer content to file
    SaveViewerContent,

    // K8s data events
    TerraformStoreUpdated,
    KustomizationStoreUpdated,
    RunnerPodsUpdated(Vec<Pod>),
    ControllerInfoUpdated(crate::state::store::ControllerInfo),
    RunnerLogsUpdated(std::collections::HashMap<(String, String), String>),

    // Log streaming
    LogChunkReceived(String),

    // CRD availability
    TerraformCrdMissing,
    KustomizationCrdMissing,

    // K8s client initialization
    K8sClientReady { client: K8sClient, context_name: String },
    ConnectionError(String),

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

    None,
}
