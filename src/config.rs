use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Extra fields to show in the Terraform detail Status panel,
    /// pulled from the outputs secret.
    #[serde(default)]
    pub detail_fields: Vec<DetailField>,

    /// Custom tabs that filter Terraform resources by annotation
    /// and display configurable columns.
    #[serde(default)]
    pub custom_tabs: Vec<CustomTab>,

    /// Custom keyboard shortcuts that open a URL in the browser.
    /// Each shortcut binds a single key and defines a URL template,
    /// or nests further shortcuts under it as a submenu.
    ///
    /// Available template variables:
    ///   {context}      - kubeconfig context name
    ///   {namespace}    - resource namespace
    ///   {name}         - resource name
    ///   {var.KEY}      - value from a matching [[context_vars]] entry,
    ///                    selected by the active kube context (see below)
    ///   {output.KEY}   - value from the Terraform outputs secret
    ///                    (nested JSON paths supported, e.g. {output.metadata.tenant})
    ///
    /// Example (flat):
    ///   [[shortcuts]]
    ///   key = "b"
    ///   label = "Grafana"
    ///   url = "https://grafana.example.com/explore?cluster={context}&pod={name}"
    ///
    /// Example (submenu — opens the Shortcuts popup at this branch when
    /// the parent's `key` is pressed; child entries are selected with
    /// j/k+Enter or by their own `key`):
    ///   [[shortcuts]]
    ///   key = "g"
    ///   label = "Grafana"
    ///     [[shortcuts.children]]
    ///     key = "o"
    ///     label = "Cluster overview"
    ///     url = "https://..."
    #[serde(default)]
    pub shortcuts: Vec<Shortcut>,

    /// Per-environment variable sets keyed by kube-context regex. Used
    /// to drive `{var.KEY}` placeholders in shortcut URLs, so a single
    /// shortcut can route to a different host depending on which
    /// cluster the user is pointed at.
    ///
    /// Entries are checked top-to-bottom; for each `{var.KEY}` lookup
    /// the first matching entry that defines `KEY` wins. Put narrow
    /// matches first, broad catch-all entries (`match = ".*"`) last.
    ///
    /// Example:
    ///   [[context_vars]]
    ///   match = "devcloud"
    ///   vars  = { grafana_host = "grafana-shared.example.net",
    ///             linode_host  = "admin.devcloud.linode.com" }
    ///
    ///   [[context_vars]]
    ///   match = ".*"
    ///   vars  = { grafana_host = "grafana-prod.example.net",
    ///             linode_host  = "admin.linode.com" }
    #[serde(default)]
    pub context_vars: Vec<ContextVars>,

    /// Optional in-app kube-context switcher. See [`Switcher`].
    #[serde(default)]
    pub switcher: Option<Switcher>,
}

/// Configures the in-app context switcher (opened with Ctrl-X).
///
/// The switcher lists the contexts of an in-memory kubeconfig and
/// reconnects the whole app to the selected one without restarting.
///
/// By default terrarium switches between the contexts found in the
/// kubeconfig it was started with (`$KUBECONFIG` / `~/.kube/config`).
/// Setting `builder` overrides that source: the command is run once at
/// startup via `sh -c`, and its stdout is parsed as a kubeconfig YAML.
/// This lets an external tool assemble the set of switchable contexts —
/// e.g. merging several per-cluster kubeconfigs into one — without
/// terrarium needing to know anything about where they come from.
///
/// Example:
///   [switcher]
///   builder = "my-cluster-tool kubeconfig --merge '*'"
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Switcher {
    /// Shell command whose stdout is a merged kubeconfig YAML. Run once
    /// at startup. If absent, the startup kubeconfig is used as-is.
    #[serde(default)]
    pub builder: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shortcut {
    /// Single character key binding (e.g. "b", "B", "L"). At the top
    /// level it activates the shortcut directly from the list/detail
    /// view; inside the popup it selects the matching entry.
    pub key: char,
    /// Short label (one or two words) shown in the popup and (for
    /// top-level entries) the status bar's `S:shortcuts` summary.
    pub label: String,
    /// Optional human-friendly description shown next to the label in
    /// the popup. Use this to disambiguate shortcuts with the same
    /// label (e.g. "GitLab — Repository" vs "GitLab — GitOps view").
    #[serde(default)]
    pub description: Option<String>,
    /// Optional group label. Consecutive shortcuts sharing the same
    /// group render under one section header in the popup, in the
    /// order they appear in config.toml. Shortcuts without a group
    /// render in an unnamed first section.
    #[serde(default)]
    pub group: Option<String>,
    /// URL template with {context}, {namespace}, {name}, {var.KEY},
    /// {output.KEY} placeholders. Leaf shortcuts must have a url;
    /// submenu shortcuts (with `children`) leave it absent and drill
    /// into their children instead.
    #[serde(default)]
    pub url: Option<String>,
    /// Optional applicability filter. Multiple shortcuts can share a
    /// `key`; on activation terrarium picks the first one whose `when`
    /// matches the selected resource. Entries without a `when` always
    /// match (the historical behaviour). All specified `when` fields
    /// must match (AND).
    #[serde(default)]
    pub when: Option<When>,
    /// Nested submenu entries. When present, activating this shortcut
    /// drills into a submenu in the Shortcuts popup. Each child can
    /// itself have children for deeper menus.
    #[serde(default)]
    pub children: Vec<Shortcut>,
}

/// Resource-applicability filter for a shortcut. Field values are
/// regular expressions matched against the corresponding part of the
/// selected resource (or the active kube context, for `context`).
/// Empty/missing fields impose no constraint.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// Regex matched against the resource name (e.g. `^cluster-`).
    #[serde(default)]
    pub name: Option<String>,
    /// Regex matched against the resource namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Regex matched against the active kubeconfig context name
    /// (e.g. `devcloud` to gate a shortcut to QA-style contexts).
    #[serde(default)]
    pub context: Option<String>,
}

/// A context-scoped set of variables exposed to shortcut URL templates
/// via `{var.KEY}`. `match` is a regex against the active kube context
/// name; the variable map is consulted in declaration order and the
/// first entry that both matches the current context AND defines the
/// requested key wins. This lets a narrow override entry sit above a
/// broad catch-all that supplies defaults for keys it doesn't override.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextVars {
    /// Regex matched against the active kube context name. Use `.*`
    /// for a fallback / default entry.
    #[serde(rename = "match")]
    pub match_: String,
    /// Variable map. Keys appear in shortcut URLs as `{var.KEY}`.
    pub vars: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailField {
    /// Label shown in the status panel (e.g. "LKE ID")
    pub label: String,
    /// Key in the Terraform outputs secret (e.g. "cluster_id")
    pub source: String,
    /// RGB color triple, e.g. [240, 200, 60]
    #[serde(default = "default_detail_color")]
    pub color: [u8; 3],
    /// Whether to render bold
    #[serde(default)]
    pub bold: bool,
}

fn default_detail_color() -> [u8; 3] {
    [200, 210, 230]
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomTab {
    /// Tab label shown in the header (e.g. "Upgrades")
    pub name: String,
    /// Annotation key used to filter Terraform resources.
    /// Only resources carrying this annotation are shown.
    pub annotation: String,
    /// Whether the annotation value is a JSON map that should be
    /// expanded into one row per key-value pair.
    #[serde(default)]
    pub expand_json_map: bool,
    /// Column definitions for this tab.
    #[serde(default)]
    pub columns: Vec<CustomColumn>,
    /// Sort by this column name (must match a column label, case-insensitive).
    /// Falls back to first column if not specified.
    #[serde(default)]
    pub sort_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomColumn {
    /// Column header label
    pub label: String,
    /// Where to get the value
    pub source: CustomColumnSource,
    /// RGB color triple
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// Whether to render bold
    #[serde(default)]
    pub bold: bool,
    /// Column width as percentage (0-100)
    #[serde(default = "default_column_width")]
    pub width: u16,
    /// Treat value as a date and color by proximity
    /// (red=past, yellow=within 7 days, green=future)
    #[serde(default)]
    pub date_highlight: bool,
}

fn default_column_width() -> u16 {
    15
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomColumnSource {
    /// The resource namespace
    Namespace,
    /// The resource name
    Name,
    /// The Ready condition status
    Ready,
    /// The resource age
    Age,
    /// When expand_json_map is true: the JSON key from the annotation map
    AnnotationKey,
    /// When expand_json_map is true: the JSON value from the annotation map.
    /// When expand_json_map is false: the raw annotation value.
    AnnotationValue,
}

impl Config {
    pub fn load() -> Self {
        if let Some(path) = config_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => match toml::from_str(&contents) {
                        Ok(config) => return config,
                        Err(e) => {
                            eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: failed to read {}: {}", path.display(), e);
                    }
                }
            }
        }
        Config::default()
    }
}

fn config_path() -> Option<PathBuf> {
    // Check TERRARIUM_CONFIG env var first
    if let Ok(path) = std::env::var("TERRARIUM_CONFIG") {
        return Some(PathBuf::from(path));
    }
    // Then ~/.config/terrarium/config.toml
    dirs_or_home().map(|d| d.join("terrarium").join("config.toml"))
}

fn dirs_or_home() -> Option<PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
}
