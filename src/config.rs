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
    ///   match = "staging"
    ///   vars  = { grafana_host = "grafana-staging.example.net",
    ///             cloud_admin_host = "cloud-admin-staging.example.net" }
    ///
    ///   [[context_vars]]
    ///   match = ".*"
    ///   vars  = { grafana_host = "grafana-prod.example.net",
    ///             cloud_admin_host = "cloud-admin.example.net" }
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
/// The URL points at a shared `config.toml`. The transport is chosen from
/// the URL scheme:
///   * `https://` / `file://`  — fetched with `curl` (works with any HTTPS
///     artifact store, raw git URL, etc.).
///   * `git+ssh://git@host/owner/repo#path/to/config.toml` — shallow-cloned
///     over SSH and the named file read out, for restricted environments
///     where SSH key auth is easier than HTTPS tokens/SSO.
///
/// Either way `sync-config` validates that the file parses as a Terrarium
/// config, backs up the current file to `config.toml.bak`, then atomically
/// replaces it.
///
/// Because the downloaded config carries its own `[config_sync] url`, a
/// first-time user bootstraps with an explicit URL
/// (`terrarium sync-config <URL>`) and thereafter just runs
/// `terrarium sync-config`.
///
/// SECURITY: a synced config can set `[switcher] builder`, which runs a
/// shell command. Only sync from a URL you trust. `http://` is rejected;
/// use `https://`, a local `file://` path, or `git+ssh://`.
///
/// Example:
///   [config_sync]
///   url = "https://internal.example.com/terrarium/config.toml"
///   # or, over SSH:
///   url = "git+ssh://git@github.example.com/org/config-repo#terrarium-config/config.toml"
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ConfigSync {
    /// URL of the shared `config.toml`. `https://`/`file://` are fetched with
    /// `curl`; `git+ssh://git@host/owner/repo#path` is cloned over SSH. Used
    /// by `terrarium sync-config` when no URL is passed on the command line.
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
    /// URL template with `{context}`, `{namespace}`, `{name}`,
    /// `{var.KEY}`, `{label.KEY}`, `{annotation.KEY}`, `{output.KEY}`,
    /// `{secret.NAME.KEY}`, and `{repo.url|host|path|branch}`
    /// placeholders. The `{repo.*}` set resolves the Flux GitRepository
    /// backing the selected resource, so a link can point at wherever the
    /// tenant's source actually lives (e.g. GitLab vs a self-hosted
    /// GitHub) without hardcoding it. Leaf shortcuts must have a url;
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
    /// (e.g. `staging` to gate a shortcut to QA-style contexts).
    #[serde(default)]
    pub context: Option<String>,
    /// Regex matched against the `spec.url` of the Flux GitRepository
    /// backing the selected resource (resolved via its `sourceRef`).
    /// Lets a shortcut target whichever forge a resource's source lives
    /// on — e.g. match one host for repos still on the old forge and
    /// another for those moved to a new one. A resource with no
    /// resolvable GitRepository never matches a `repo_url` filter.
    #[serde(default)]
    pub repo_url: Option<String>,
    /// Regex matched against any GitRepository `spec.url` in the selected
    /// resource's namespace. This is useful when migration state is signaled
    /// by a namespace-level source rather than the Terraform's own sourceRef.
    #[serde(default)]
    pub namespace_repo_url: Option<String>,
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
/// The transport is picked from the URL scheme (`https://`/`file://` via
/// `curl`, `git+ssh://` via a shallow clone; see [`fetch_shared_config`]),
/// validated by parsing it as a [`Config`], and only then written — the
/// previous file is copied to
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

    // Fetch over a transport we trust. A synced config can run shell commands
    // via [switcher] builder, so plaintext http is refused; https://, a local
    // file://, and authenticated git+ssh:// (SSH key auth, no token/SSO) are
    // allowed.
    eprintln!("Fetching config from {url} …");
    let contents = fetch_shared_config(&url)?;

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

/// Download the shared `config.toml` bytes, picking the transport from the
/// URL scheme so the same `[config_sync] url` works for either style:
///   * `https://` / `file://`  → fetched with `curl` (unchanged behaviour)
///   * `git+ssh://` / `git://`  → shallow-cloned over SSH, file read out
///
/// Plaintext `http://` is rejected: a synced config can run shell commands
/// via `[switcher] builder`, so we only accept transports that are encrypted
/// (https), local (file), or key-authenticated (ssh).
fn fetch_shared_config(url: &str) -> anyhow::Result<String> {
    if let Some(spec) = url
        .strip_prefix("git+ssh://")
        .or_else(|| url.strip_prefix("git://"))
    {
        fetch_via_git(spec)
    } else if url.starts_with("https://") || url.starts_with("file://") {
        fetch_via_curl(url)
    } else {
        anyhow::bail!("refusing to sync from `{url}` — use https://, file://, or git+ssh://")
    }
}

/// Fetch over HTTPS (or a local file) with `curl`. This is the original
/// transport and its behaviour is unchanged.
fn fetch_via_curl(url: &str) -> anyhow::Result<String> {
    use anyhow::Context;

    // -f: fail on HTTP errors; -sS: quiet but still show errors; -L: follow
    // redirects. `--` guards against a URL that looks like a flag.
    let output = std::process::Command::new("curl")
        .args(["-fsSL", "--", url])
        .output()
        .context("failed to run curl (is it installed and on PATH?)")?;
    if !output.status.success() {
        anyhow::bail!(
            "curl failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("downloaded config was not UTF-8")
}

/// Fetch a single file from a git repo over SSH. The URL carries the repo and
/// the in-repo file path separated by `#`, e.g.
///   `git+ssh://git@host/owner/repo#path/to/config.toml`
///
/// `git archive --remote` is disabled on GitHub/GHE, so we shallow-clone the
/// default branch into a temp dir, read the file, and clean up. Auth is left
/// to the user's SSH setup — no token or SSO is involved.
fn fetch_via_git(spec: &str) -> anyhow::Result<String> {
    use anyhow::Context;

    let (repo, file) = spec.split_once('#').context(
        "git+ssh:// URL must name the file after '#', e.g.\n\
         git+ssh://git@host/owner/repo#path/to/config.toml",
    )?;
    if file.is_empty() {
        anyhow::bail!("git+ssh:// URL has an empty file path after '#'");
    }
    // The file path is joined onto a temp dir, so keep it inside the clone.
    if file.starts_with('/') || file.split('/').any(|c| c == "..") {
        anyhow::bail!("file path in git+ssh:// URL must be relative and not contain '..'");
    }
    let clone_url = format!("ssh://{repo}");

    // Unique temp dir; remove any leftover from a previously-crashed run.
    let tmpdir = std::env::temp_dir().join(format!("terrarium-sync-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmpdir);

    eprintln!("Cloning {clone_url} over SSH …");
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", "--quiet", "--", &clone_url])
        .arg(&tmpdir)
        .status()
        .context("failed to run git (is it installed and on PATH?)")?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmpdir);
        anyhow::bail!("git clone failed ({status}) for {clone_url}");
    }

    // Read the file, then always clean up the clone regardless of outcome.
    let result = std::fs::read_to_string(tmpdir.join(file))
        .with_context(|| format!("reading `{file}` from {clone_url}"));
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
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

    #[test]
    fn untrusted_schemes_are_refused() {
        // Plaintext http and anything unrecognised bail before any fetch.
        for url in [
            "http://example.com/config.toml",
            "ftp://example.com/config.toml",
            "example.com/config.toml",
        ] {
            let err = super::fetch_shared_config(url).unwrap_err().to_string();
            assert!(err.contains("refusing to sync"), "{url}: {err}");
        }
    }

    #[test]
    fn git_ssh_url_must_name_a_file() {
        // No '#' fragment at all.
        let err = super::fetch_via_git("git@host/owner/repo")
            .unwrap_err()
            .to_string();
        assert!(err.contains("must name the file after '#'"), "{err}");

        // Empty fragment.
        let err = super::fetch_via_git("git@host/owner/repo#")
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty file path"), "{err}");
    }

    #[test]
    fn git_ssh_file_path_must_stay_in_clone() {
        for file in ["/etc/passwd", "../outside.toml", "a/../../b.toml"] {
            let spec = format!("git@host/owner/repo#{file}");
            let err = super::fetch_via_git(&spec).unwrap_err().to_string();
            assert!(
                err.contains("must be relative and not contain '..'"),
                "{file}: {err}"
            );
        }
    }
}
