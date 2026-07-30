mod action;
mod app;
mod config;
mod error;
mod k8s;
mod keys;
mod state;
mod tui;
mod ui;
mod util;

use clap::Parser;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

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
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,kube_client=off,hyper_util=off,tower=off"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

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

    // Build app state immediately with empty stores. Prefer an explicit
    // --context, then the switcher kubeconfig's current-context, then the
    // on-disk kubeconfig's.
    let context_label = cli
        .context
        .clone()
        .or_else(|| {
            switcher_kubeconfig
                .as_ref()
                .and_then(|kc| kc.current_context.clone())
        })
        .or_else(|| {
            kube::config::Kubeconfig::read()
                .ok()
                .and_then(|kc| kc.current_context)
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

    // Pre-flight `exec`/OIDC credential plugins on the normal terminal,
    // before entering the alternate screen. If the initial context uses an
    // exec plugin (OIDC browser login, aws/gke/azure token, …) this lets it
    // run any interactive step and cache its token cleanly, so the client
    // connect below never garbles the TUI. No-op for token/cert contexts.
    {
        let effective_kc = switcher_kubeconfig
            .clone()
            .or_else(|| kube::config::Kubeconfig::read().ok());
        if let Some(exec) = effective_kc
            .as_ref()
            .and_then(|kc| k8s::exec_auth::exec_for_context(kc, cli.context.as_deref()))
        {
            eprintln!("Authenticating to {context_label} … (a browser may open)");
            if let Err(e) = k8s::exec_auth::prewarm(&exec) {
                eprintln!("Warning: pre-authentication failed: {e}");
            }
        }
    }

    // Init terminal and start the app event loop immediately
    let mut terminal = tui::init(mouse_enabled)?;

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
        cli.namespace.clone(),
        cli.controller_ns.clone(),
        tf_debug_log,
    );

    // Kick off the initial connection (spawns client init + watchers).
    app.connect(cli.context.clone());

    // Run the app (renders immediately, data fills in as watchers connect)
    let result = app.run(&mut terminal).await;

    // Restore terminal
    tui::restore()?;

    result
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
