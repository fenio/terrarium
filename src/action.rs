use k8s_openapi::api::core::v1::Pod;

/// Wrapper for kube::Client that implements Debug.
#[derive(Clone)]
pub struct K8sClient(pub kube::Client);

impl std::fmt::Debug for K8sClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "K8sClient")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Terraform,
    Kustomization,
    Pod,
}

/// Identity captured when an operator selects a resource for mutation. Names
/// are reusable in Kubernetes, so namespace/name alone is not sufficient to
/// prove that a later action still targets the object the operator saw.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MutationTarget {
    pub kind: ResourceKind,
    pub namespace: String,
    pub name: String,
    pub uid: String,
    pub resource_version: String,
}

/// Logical lanes for asynchronous work. A newer request in the same lane and
/// target supersedes the older one; connection-scoped lanes use request ID 0
/// and are invalidated when the connection generation changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AsyncKind {
    Connection,
    Metrics,
    Plan,
    Resource,
    Outputs,
    Events,
    DetailOutputs,
    ShortcutOutputs,
    SecretValues,
    Mutation,
    LogStream,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AsyncTarget {
    Resource {
        kind: ResourceKind,
        namespace: String,
        name: String,
    },
    Pod {
        namespace: String,
        name: String,
    },
}

/// Identity of the UI object that owns an asynchronous result. View-scoped
/// work is invalidated by navigation or selection changes; stream-scoped work
/// remains valid while its log viewer is preserved on another tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AsyncIdentity {
    None,
    View(u64),
    Stream(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncScope {
    pub generation: u64,
    pub request_id: u64,
    pub kind: AsyncKind,
    pub target: Option<AsyncTarget>,
    pub identity: AsyncIdentity,
}

impl AsyncScope {
    pub fn generation(generation: u64, kind: AsyncKind) -> Self {
        Self {
            generation,
            request_id: 0,
            kind,
            target: None,
            identity: AsyncIdentity::None,
        }
    }

    pub fn request(
        generation: u64,
        request_id: u64,
        kind: AsyncKind,
        target: Option<AsyncTarget>,
        identity: AsyncIdentity,
    ) -> Self {
        Self {
            generation,
            request_id,
            kind,
            target,
            identity,
        }
    }

    pub fn sender(&self, tx: tokio::sync::mpsc::UnboundedSender<Action>) -> ScopedSender {
        ScopedSender {
            tx,
            scope: self.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScopedAction {
    pub scope: AsyncScope,
    pub action: Box<Action>,
}

#[derive(Clone)]
pub struct ScopedSender {
    tx: tokio::sync::mpsc::UnboundedSender<Action>,
    scope: AsyncScope,
}

impl ScopedSender {
    pub fn send(
        &self,
        action: Action,
    ) -> Result<(), Box<tokio::sync::mpsc::error::SendError<Action>>> {
        self.tx
            .send(Action::scoped(self.scope.clone(), action))
            .map_err(Box::new)
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    /// Result or lifecycle event produced by asynchronous work. `dispatch`
    /// validates the scope before unwrapping and handling the inner action.
    Scoped(ScopedAction),

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
    ToggleDriftingOnly,
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
        target: MutationTarget,
    },
    Replan {
        target: MutationTarget,
    },
    ForceUnlock {
        target: MutationTarget,
    },
    ExecBreakTheGlass {
        target: MutationTarget,
    },
    /// Clear `spec.breakTheGlass` after a persistent break-the-glass mode
    /// was left behind by a stuck or interrupted session.
    ResetBreakTheGlass {
        target: MutationTarget,
    },
    /// Remove all finalizers from a Terraform object, bypassing controller
    /// cleanup. This is intentionally separate from normal deletion.
    RemoveFinalizers {
        target: MutationTarget,
    },
    DeleteResource {
        target: MutationTarget,
    },
    KillRunner {
        target: MutationTarget,
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
        target: MutationTarget,
    },
    Suspend {
        target: MutationTarget,
    },
    Resume {
        target: MutationTarget,
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
    DetailOutputsFetchError(String),
    // Output values fetched for a shortcut URL. The continuation is kept in
    // one action so the scoped request can be retired atomically.
    ShortcutOutputsFetched {
        namespace: String,
        name: String,
        shortcut_idx: usize,
        values: std::collections::HashMap<String, String>,
    },
    // Values fetched from an arbitrary Secret for a shortcut URL. The
    // continuation is kept in one action so the scoped request can be retired
    // atomically.
    ShortcutSecretValuesFetched {
        namespace: String,
        name: String,
        secret_name: String,
        shortcut_idx: usize,
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
    /// Export the names of the currently visible Terraform rows.
    ExportTerraformNames,

    // K8s data events
    TerraformStoreUpdated,
    KustomizationStoreUpdated,
    GitRepoStoreUpdated,
    RunnerPodsUpdated(Vec<Pod>),
    ControllerInfoUpdated(crate::state::store::ControllerInfo),
    RunnerLogsUpdated(std::collections::HashMap<(String, String), String>),

    // Log streaming
    LogChunkReceived {
        stream_id: u64,
        chunk: String,
    },

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
    K8sActionSuccess {
        target: MutationTarget,
        message: String,
    },
    K8sActionError {
        target: MutationTarget,
        message: String,
    },
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

impl Action {
    pub fn scoped(scope: AsyncScope, action: Action) -> Self {
        Self::Scoped(ScopedAction {
            scope,
            action: Box::new(action),
        })
    }
}
