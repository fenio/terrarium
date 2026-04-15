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
#[command(name = "terrarium", about = "TUI for managing tofu-controller resources")]
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

    // Create action channel
    let (action_tx, action_rx) = mpsc::unbounded_channel::<action::Action>();

    // Set up reflectors (stores are immediately available, just empty)
    let (tf_store, tf_writer) = k8s::watcher::create_tf_store();
    let (ks_store, ks_writer) = k8s::watcher::create_ks_store();

    // Build app state immediately with empty stores
    let context_label = cli
        .context
        .clone()
        .or_else(|| {
            kube::config::Kubeconfig::read()
                .ok()
                .and_then(|kc| kc.current_context)
        })
        .unwrap_or_else(|| "connecting...".to_string());

    let config = config::Config::load();
    let mut app_state =
        state::store::AppState::new(tf_store, ks_store, context_label, config);
    if let Some(ns) = cli.namespace.clone() {
        app_state.namespace_filter = Some(ns);
    }
    let mouse_enabled = cli.mouse;
    app_state.mouse_enabled = mouse_enabled;

    // Init terminal and start the app event loop immediately
    let mut terminal = tui::init(mouse_enabled)?;

    // Build app (no K8s client yet — will receive it via action channel)
    let mut app = app::App::new_deferred(app_state, action_tx.clone(), action_rx);

    // Spawn background K8s initialization
    let tx = action_tx.clone();
    let context = cli.context.clone();
    let namespace = cli.namespace.clone();
    let controller_ns = cli.controller_ns.clone();
    tokio::spawn(async move {
        match k8s::client::create_client(context.as_deref()).await {
            Ok((client, cluster_info)) => {
                // Send the client and context name back to the app
                let _ = tx.send(action::Action::K8sClientReady {
                    client: action::K8sClient(client.clone()),
                    context_name: cluster_info.context_name,
                });

                // Now spawn all watchers
                let wtx = tx.clone();
                let c = client.clone();
                tokio::spawn(async move {
                    if let Err(e) = k8s::watcher::run_tf_watcher(c, tf_writer, wtx.clone()).await {
                        let _ = wtx.send(action::Action::ConnectionError(
                            format!("Terraform watcher failed: {}", e),
                        ));
                    }
                });

                let wtx = tx.clone();
                let c = client.clone();
                tokio::spawn(async move {
                    if let Err(e) = k8s::watcher::run_ks_watcher(c, ks_writer, wtx.clone()).await {
                        let _ = wtx.send(action::Action::ConnectionError(
                            format!("Kustomization watcher failed: {}", e),
                        ));
                    }
                });

                let wtx = tx.clone();
                let c = client.clone();
                let ns_clone = namespace.clone();
                tokio::spawn(async move {
                    if let Err(e) = k8s::runners::poll_runner_pods(c, wtx.clone(), ns_clone).await {
                        let _ = wtx.send(action::Action::ConnectionError(
                            format!("Runner poller failed: {}", e),
                        ));
                    }
                });

                let wtx = tx.clone();
                let c = client.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        k8s::controller::poll_controller_info(c, wtx.clone(), controller_ns).await
                    {
                        let _ = wtx.send(action::Action::ConnectionError(
                            format!("Controller poller failed: {}", e),
                        ));
                    }
                });
            }
            Err(e) => {
                let _ = tx.send(action::Action::ConnectionError(format!(
                    "Failed to connect to cluster: {}",
                    e
                )));
            }
        }
    });

    // Run the app (renders immediately, data fills in as watchers connect)
    let result = app.run(&mut terminal).await;

    // Restore terminal
    tui::restore()?;

    result
}
