use serde::Deserialize;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

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

    /// Optional config-sync source. See [`ConfigSync`].
    #[serde(default)]
    pub config_sync: Option<ConfigSync>,
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

/// Configures `terrarium sync-config`, which refreshes this config file
/// from a central location so a team can distribute one shared config.
///
/// The URL points at a `config.toml` served over HTTPS (an internal
/// artifact store, a raw git URL, etc.). `sync-config` fetches it with
/// `curl`, validates that it parses as a Terrarium config, backs up the
/// current file to `config.toml.bak`, then atomically replaces it.
///
/// Because the downloaded config carries its own `[config_sync] url`, a
/// first-time user bootstraps with an explicit URL
/// (`terrarium sync-config <URL>`) and thereafter just runs
/// `terrarium sync-config`.
///
/// SECURITY: a synced config can set `[switcher] builder`, which runs a
/// shell command. Only sync from a URL you trust. `http://` is rejected;
/// use `https://` (or a local `file://` path).
///
/// Example:
///   [config_sync]
///   url = "https://internal.example.com/terrarium/config.toml"
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ConfigSync {
    /// HTTPS (or `file://`) URL of the shared `config.toml`. Used by
    /// `terrarium sync-config` when no URL is passed on the command line.
    #[serde(default)]
    pub url: Option<String>,
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
        if let Some(path) = config_path()
            && path.exists()
        {
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
        Config::default()
    }
}

/// Fetch a shared `config.toml` from a central URL and replace the local
/// one. Called by `terrarium sync-config`.
///
/// URL resolution: `url_override` (from the command line) wins; otherwise
/// the `[config_sync] url` of the currently-installed config is used.
/// The download is fetched with `curl`, validated by parsing it as a
/// [`Config`], and only then written — the previous file is copied to
/// `config.toml.bak` and the new one is put in place via a temp + rename
/// so a failed or interrupted sync never leaves a half-written config.
pub fn sync_config(url_override: Option<String>) -> anyhow::Result<()> {
    use anyhow::Context;

    let target =
        config_path().context("could not determine config path (set HOME or TERRARIUM_CONFIG)")?;

    // Resolve the source URL: explicit argument first, else the URL baked
    // into the config we already have.
    let url = match url_override {
        Some(u) => u,
        None => Config::load().config_sync.and_then(|c| c.url).context(
            "no URL given and no [config_sync] url in the current config.\n\
             Bootstrap with: terrarium sync-config <URL>",
        )?,
    };

    // Only fetch over a transport we trust; a synced config can run shell
    // commands via [switcher] builder, so plaintext http is refused.
    if !(url.starts_with("https://") || url.starts_with("file://")) {
        anyhow::bail!("refusing to sync from `{url}` — use https:// (or file://)");
    }

    // -f: fail on HTTP errors; -sS: quiet but still show errors; -L: follow
    // redirects. `--` guards against a URL that looks like a flag.
    eprintln!("Fetching config from {url} …");
    let output = std::process::Command::new("curl")
        .args(["-fsSL", "--", &url])
        .output()
        .context("failed to run curl (is it installed and on PATH?)")?;
    if !output.status.success() {
        anyhow::bail!(
            "curl failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let contents = String::from_utf8(output.stdout).context("downloaded config was not UTF-8")?;

    // Validate before touching disk: it must parse as a Terrarium config.
    let parsed: Config =
        toml::from_str(&contents).context("downloaded file is not a valid Terrarium config")?;

    // First-run bootstrap: create ~/.config/terrarium if needed.
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    // Back up any existing config, then write atomically (temp + rename).
    if target.exists() {
        let backup = target.with_extension("toml.bak");
        std::fs::copy(&target, &backup)
            .with_context(|| format!("backing up to {}", backup.display()))?;
        eprintln!("Backed up existing config to {}", backup.display());
    }
    let tmp = target.with_extension("toml.tmp");
    std::fs::write(&tmp, &contents).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &target).with_context(|| format!("replacing {}", target.display()))?;

    eprintln!(
        "Updated {} ({} shortcuts, {} custom tabs, {} context-var sets).",
        target.display(),
        parsed.shortcuts.len(),
        parsed.custom_tabs.len(),
        parsed.context_vars.len(),
    );
    Ok(())
}

fn config_path() -> Option<PathBuf> {
    // Check TERRARIUM_CONFIG env var first
    if let Ok(path) = std::env::var("TERRARIUM_CONFIG") {
        return Some(PathBuf::from(path));
    }
    // Then select the default config by the name used to invoke the binary.
    // This lets a symlink named `terrariumdev` use a separate development
    // config while sharing the same installed binary.
    let invoked = std::env::args_os().next();
    let basename = invoked
        .as_deref()
        .and_then(|arg| Path::new(arg).file_name());
    dirs_or_home().map(|d| d.join("terrarium").join(config_filename(basename)))
}

fn config_filename(invoked_basename: Option<&OsStr>) -> &'static str {
    if invoked_basename == Some(OsStr::new("terrariumdev")) {
        "configdev.toml"
    } else {
        "config.toml"
    }
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

#[cfg(test)]
mod tests {
    use super::config_filename;
    use std::ffi::OsStr;

    #[test]
    fn dev_invocation_uses_development_config() {
        assert_eq!(
            config_filename(Some(OsStr::new("terrariumdev"))),
            "configdev.toml"
        );
    }

    #[test]
    fn normal_invocations_use_default_config() {
        for name in [
            None,
            Some(OsStr::new("terrarium")),
            Some(OsStr::new("other")),
        ] {
            assert_eq!(config_filename(name), "config.toml");
        }
    }
}
