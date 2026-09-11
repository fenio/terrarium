use clap::Parser;
use terrarium::{action, app, config, k8s, logging, ornament, state, tui};
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(
    name = "terrarium",
    version,
    about = "TUI for managing tofu-controller resources"
)]
struct Cli {
    /// Kubernetes namespace to filter (default: all namespaces)
    #[arg(short, long)]
    namespace: Option<String>,

    /// Kubeconfig context to use
    #[arg(short, long)]
    context: Option<String>,

    /// Namespace where tofu-controller is deployed
    #[arg(long, default_value = "flux-system")]
    controller_ns: String,

    /// Enable mouse support (click, scroll; requires Shift for native copy)
    #[arg(long)]
    mouse: bool,

    #[arg(long, hide = true)]
    gp: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Fetch the shared config.toml from a central URL and replace the
    /// local one. Pass a URL to bootstrap; omit it to refresh from the
    /// [config_sync] url already in your config.
    SyncConfig {
        /// Source URL (https:// or file://). Optional when [config_sync]
        /// url is set in the current config.
        url: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs go to a file, never the terminal — writing to stdout/stderr would
    // corrupt the TUI's alternate screen. Surfaced errors (overlay/status bar)
    // keep the user informed; the file holds the verbose detail.
    logging::init();

    let cli = Cli::parse();

    // Hidden ornament: intentionally absent from README and --help.
    if cli.gp {
        return ornament::run();
    }

    // Subcommands run and exit before any terminal/K8s setup.
    if let Some(Command::SyncConfig { url }) = cli.command {
        return config::sync_config(url);
    }

    // Create action channel
    let (action_tx, action_rx) = mpsc::unbounded_channel::<action::Action>();

    // Seed AppState with empty reflector stores so the UI can render
    // before any client exists. The matching writers are unused here —
    // connect() creates its own store/writer pairs for each connection.
    let (tf_store, _) = k8s::watcher::create_tf_store();
    let (ks_store, _) = k8s::watcher::create_ks_store();
    let (gr_store, _) = k8s::watcher::create_gitrepo_store();

    let config = config::Config::load();

    // Optional [switcher] builder: run once at startup and parse its
    // stdout as the kubeconfig the in-app context switcher works over.
    // A failure here is non-fatal — we fall back to the on-disk
    // kubeconfig and surface a flash so the switcher just uses whatever
    // contexts $KUBECONFIG already provides.
    let mut switcher_warning: Option<String> = None;
    let switcher_kubeconfig = match config.switcher.as_ref().and_then(|s| s.builder.as_deref()) {
        Some(cmd) => match run_switcher_builder(cmd) {
            Ok(kc) => Some(kc),
            Err(e) => {
                switcher_warning = Some(format!("switcher builder failed: {e}"));
                None
            }
        },
        None => None,
    };

    // Materialise the switcher kubeconfig to a private temp file so tfctl
    // subprocesses (replan / break-glass) can resolve the same contexts
    // terrarium built in memory. `None` when there's no builder or the write
    // failed — subprocesses then fall back to the on-disk kubeconfig.
    let switcher_kubeconfig_path = switcher_kubeconfig
        .as_ref()
        .and_then(write_switcher_kubeconfig);

    // The effective kubeconfig is the merged switcher output when configured,
    // otherwise the regular on-disk kubeconfig. Keep it around for startup
    // selection and the exec-auth preflight below.
    let effective_kubeconfig = switcher_kubeconfig
        .as_ref()
        .cloned()
        .or_else(|| kube::config::Kubeconfig::read().ok());
    let initial_contexts: Vec<String> = effective_kubeconfig
        .as_ref()
        .map(|kc| {
            kc.contexts
                .iter()
                .map(|context| context.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let choose_initial_context =
        should_choose_initial_context(cli.context.as_deref(), initial_contexts.len());

    // Build app state immediately with empty stores. Prefer an explicit
    // --context, then the effective kubeconfig's current-context.
    let context_label = cli
        .context
        .clone()
        .or_else(|| {
            effective_kubeconfig
                .as_ref()
                .and_then(|kc| kc.current_context.clone())
        })
        .unwrap_or_else(|| "connecting...".to_string());

    let mut app_state =
        state::store::AppState::new(tf_store, ks_store, gr_store, context_label.clone(), config);
    if let Some(ns) = cli.namespace.clone() {
        app_state.namespace_filter = Some(ns);
    }
    let mouse_enabled = cli.mouse;
    app_state.mouse_enabled = mouse_enabled;

    // Probe $PATH for tfctl once at startup. Replan and Break-the-Glass
    // delegate to tfctl, so flag the absence early rather than silently
    // failing when the user presses 'R' or 'x'.
    //
    // `--help` is the most reliable success-exit probe across CLI styles:
    // tfctl is cobra-based and uses `tfctl version` (no `--version`
    // flag), so probing with a flag would falsely fail. `--help` always
    // exits 0 when the binary exists, regardless of subcommand layout.
    app_state.tfctl_available = std::process::Command::new("tfctl")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !app_state.tfctl_available {
        app_state.flash_message = Some((
            "tfctl not found in PATH — Replan (R) and Break-the-Glass (x) will fail".to_string(),
            std::time::Instant::now(),
            state::store::FlashKind::Error,
        ));
    }
    // A switcher-builder failure is the more actionable startup issue, so
    // let it take the flash slot if both fired.
    if let Some(w) = switcher_warning {
        app_state.flash_message =
            Some((w, std::time::Instant::now(), state::store::FlashKind::Error));
    }

    // For the automatic startup path, pre-flight `exec`/OIDC credential
    // plugins on the normal terminal before entering the alternate screen.
    // Multi-context startup defers this until the user chooses a context, so
    // the selected context is the one that gets prewarmed.
    if !choose_initial_context
        && let Some(exec) = effective_kubeconfig
            .as_ref()
            .and_then(|kc| k8s::exec_auth::exec_for_context(kc, cli.context.as_deref()))
    {
        eprintln!("Authenticating to {context_label} … (a browser may open)");
        if let Err(e) = k8s::exec_auth::prewarm(&exec) {
            eprintln!("Warning: pre-authentication failed: {e}");
        }
    }

    // Init terminal and start the app event loop immediately
    let mut terminal = tui::init(mouse_enabled)?;

    // Now that we're on the alternate screen, send stderr to the log file so
    // a failing kube exec/OIDC credential plugin (run lazily by kube-rs with
    // inherited stderr) can't garble the TUI. Automatic-startup preflight
    // auth ran before this; deferred startup auth suspends the TUI cleanly.
    logging::capture_stderr();

    // Optional per-event TF condition trace, enabled by setting
    // TERRARIUM_DEBUG_LOG=/path/to/file. Used to diagnose transient
    // Ready=False flickers when the user can't press `c` fast enough.
    let tf_debug_log = std::env::var_os("TERRARIUM_DEBUG_LOG").map(std::path::PathBuf::from);

    // Build app (no K8s client yet — established by connect()). The
    // initial stores passed to AppState::new above are immediately
    // replaced by the first connect().
    let mut app = app::App::new_deferred(
        app_state,
        action_tx.clone(),
        action_rx,
        switcher_kubeconfig,
        switcher_kubeconfig_path,
        cli.namespace.clone(),
        cli.controller_ns.clone(),
        tf_debug_log,
    );

    // Let the user choose a kube-context before connecting when a merged
    // kubeconfig offers several contexts. Explicit --context remains
    // authoritative, and single-context kubeconfigs keep the old fast path.
    if choose_initial_context {
        app.begin_initial_context_selection(initial_contexts);
    } else {
        // Kick off the initial connection (spawns client init + watchers).
        app.connect(cli.context.clone());
    }

    // Run the app (renders immediately, data fills in as watchers connect)
    let result = app.run(&mut terminal).await;

    // Restore terminal
    tui::restore()?;

    result
}

fn should_choose_initial_context(explicit_context: Option<&str>, context_count: usize) -> bool {
    explicit_context.is_none() && context_count > 1
}

/// Run the `[switcher] builder` command via `sh -c` and parse its stdout
/// as a kubeconfig YAML. Errors (non-zero exit, non-UTF-8, or unparseable
/// YAML) are returned to the caller, which treats them as non-fatal.
fn run_switcher_builder(cmd: &str) -> anyhow::Result<kube::config::Kubeconfig> {
    use anyhow::Context;
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("spawning `{cmd}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "`{cmd}` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let yaml = String::from_utf8(output.stdout).context("builder stdout was not UTF-8")?;
    kube::config::Kubeconfig::from_yaml(&yaml).context("parsing builder output as kubeconfig")
}

/// Directory for terrarium's transient state (switcher kubeconfig temp files):
/// `$XDG_STATE_HOME/terrarium` or `~/.local/state/terrarium`.
fn state_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state"))
        })
        .map(|d| d.join("terrarium"))
}

/// Serialise the switcher kubeconfig to a private (0600) temp file so tfctl
/// subprocesses can see the same contexts. Returns the path, or `None` if it
/// couldn't be written (callers then leave `KUBECONFIG` alone and tfctl falls
/// back to the on-disk kubeconfig). Named by PID so leftovers can be swept.
fn write_switcher_kubeconfig(kc: &kube::config::Kubeconfig) -> Option<std::path::PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = state_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    sweep_stale_switcher_kubeconfigs(&dir);

    let yaml = serde_yaml::to_string(kc).ok()?;
    let path = dir.join(format!("switcher-kubeconfig-{}.yaml", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .ok()?;
    f.write_all(yaml.as_bytes()).ok()?;
    Some(path)
}

/// Remove `switcher-kubeconfig-<pid>.yaml` files left by terrarium processes
/// that are no longer running (e.g. after a crash, where Drop didn't run).
/// Best-effort; only deletes when the PID is provably gone (ESRCH).
fn sweep_stale_switcher_kubeconfigs(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let self_pid = std::process::id() as i32;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid) = name
            .strip_prefix("switcher-kubeconfig-")
            .and_then(|s| s.strip_suffix(".yaml"))
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        // kill(pid, 0) probes existence without signalling: Ok => alive;
        // EPERM => alive but not ours; ESRCH => gone (safe to remove).
        let gone = unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM);
        if gone {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::should_choose_initial_context;

    #[test]
    fn startup_context_picker_requires_multiple_contexts_without_override() {
        assert!(should_choose_initial_context(None, 2));
        assert!(!should_choose_initial_context(None, 1));
        assert!(!should_choose_initial_context(Some("prod"), 2));
    }

    /// The switcher kubeconfig we write for tfctl must serialize back to YAML
    /// that still parses as a kubeconfig with the same contexts — otherwise
    /// `tfctl --context <name>` can't resolve anything.
    #[test]
    fn switcher_kubeconfig_serializes_back_to_parseable_yaml() {
        let yaml = r#"
apiVersion: v1
kind: Config
current-context: us-ord-flux-internal-02
clusters:
- name: us-ord-flux-internal-02
  cluster:
    server: https://api-us-ord-flux-internal-02.example.net:443
contexts:
- name: us-ord-flux-internal-02
  context:
    cluster: us-ord-flux-internal-02
    user: oidc-admin
users:
- name: oidc-admin
  user: {}
"#;
        let kc = kube::config::Kubeconfig::from_yaml(yaml).expect("parse input");
        let serialized = serde_yaml::to_string(&kc).expect("serialize");
        let reparsed = kube::config::Kubeconfig::from_yaml(&serialized).expect("reparse");
        assert!(
            reparsed
                .contexts
                .iter()
                .any(|c| c.name == "us-ord-flux-internal-02"),
            "context must survive the round-trip"
        );
        assert_eq!(
            reparsed.current_context.as_deref(),
            Some("us-ord-flux-internal-02")
        );
    }
}
