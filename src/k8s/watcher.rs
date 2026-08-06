use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use futures::StreamExt;
use kube::{
    api::Api,
    runtime::{
        WatchStreamExt,
        reflector::{self, store::Writer},
        watcher,
    },
};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::k8s::kustomization::Kustomization;
use crate::k8s::source::GitRepository;
use crate::k8s::terraform::Terraform;
use crate::util;

pub type TfStore = reflector::Store<Terraform>;
pub type KsStore = reflector::Store<Kustomization>;
pub type GitRepoStore = reflector::Store<GitRepository>;

pub fn create_tf_store() -> (TfStore, Writer<Terraform>) {
    reflector::store()
}

pub fn create_ks_store() -> (KsStore, Writer<Kustomization>) {
    reflector::store()
}

pub fn create_gitrepo_store() -> (GitRepoStore, Writer<GitRepository>) {
    reflector::store()
}

pub async fn run_tf_watcher(
    client: kube::Client,
    writer: Writer<Terraform>,
    tx: UnboundedSender<Action>,
    debug_log: Option<PathBuf>,
) -> Result<()> {
    let api: Api<Terraform> = Api::all(client);
    let debug_writer: Option<Mutex<std::fs::File>> = debug_log.as_ref().and_then(|p| {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
        {
            Ok(f) => Some(Mutex::new(f)),
            Err(e) => {
                tracing::warn!("could not open debug log {}: {e}", p.display());
                None
            }
        }
    });
    let mut stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .reflect(writer)
        .applied_objects()
        .boxed();

    // kube's watcher is self-healing: when the watch stream drops — an idle
    // or route timeout on a proxy (Envoy's default route timeout is 15s),
    // an apiserver rollout, etc. — it emits an Err and then transparently
    // re-lists and re-watches. Consume it forever, logging transient errors
    // and continuing. Terminating on the first Err (as try_for_each did)
    // turned every routine watch drop into a fatal "connection lost".
    while let Some(item) = stream.next().await {
        match item {
            Ok(obj) => {
                if let Some(w) = debug_writer.as_ref() {
                    log_tf_condition_snapshot(w, &obj);
                }
                let _ = tx.send(Action::TerraformStoreUpdated);
            }
            Err(e) => {
                if crate::util::is_auth_error(&crate::util::error_chain(&e)) {
                    let _ = tx.send(Action::AuthExpired);
                    return Ok(());
                }
                if is_crd_missing(&e) {
                    let _ = tx.send(Action::TerraformCrdMissing);
                    return Ok(());
                }
                tracing::debug!("Terraform watch error (will retry): {e}");
            }
        }
    }
    Ok(())
}

/// True when a watcher error means the CRD itself is absent (vs. a transient
/// stream drop), so the caller shows the "CRD missing" hint instead of
/// retrying forever against a type that doesn't exist on this cluster.
fn is_crd_missing(e: &watcher::Error) -> bool {
    let msg = e.to_string();
    msg.contains("404")
        || msg.contains("not found")
        || msg.contains("the server could not find the requested resource")
}

/// Append one line per Terraform watcher event capturing Ready + Reconciling
/// state and our classification, so transient flickers can be analysed
/// post-hoc instead of trying to press `c` at the right moment.
fn log_tf_condition_snapshot(writer: &Mutex<std::fs::File>, tf: &Terraform) {
    let ns = tf.metadata.namespace.as_deref().unwrap_or("-");
    let name = tf.metadata.name.as_deref().unwrap_or("-");
    let conds = tf.status.as_ref().and_then(|s| s.conditions.as_ref());
    let (ready_status, ready_reason, ready_msg) = conds
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
        .map(|c| (c.status.as_str(), c.reason.as_str(), c.message.as_str()))
        .unwrap_or(("<absent>", "", ""));
    let (recon_status, recon_reason) = conds
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Reconciling"))
        .map(|c| (c.status.as_str(), c.reason.as_str()))
        .unwrap_or(("<absent>", ""));
    let classification = format!("{:?}", util::classify_ready(conds));
    let msg_short: String = ready_msg.chars().take(120).collect();
    let line = format!(
        "{ts} {ns}/{name}\tready={ready_status}/{ready_reason}\treconciling={recon_status}/{recon_reason}\tclass={classification}\tmsg={msg_short}\n",
        ts = jiff::Zoned::now(),
    );
    if let Ok(mut f) = writer.lock() {
        let _ = f.write_all(line.as_bytes());
    }
}

pub async fn run_ks_watcher(
    client: kube::Client,
    writer: Writer<Kustomization>,
    tx: UnboundedSender<Action>,
) -> Result<()> {
    let api: Api<Kustomization> = Api::all(client);
    let mut stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .reflect(writer)
        .applied_objects()
        .boxed();

    while let Some(item) = stream.next().await {
        match item {
            Ok(_obj) => {
                let _ = tx.send(Action::KustomizationStoreUpdated);
            }
            Err(e) => {
                if crate::util::is_auth_error(&crate::util::error_chain(&e)) {
                    let _ = tx.send(Action::AuthExpired);
                    return Ok(());
                }
                if is_crd_missing(&e) {
                    let _ = tx.send(Action::KustomizationCrdMissing);
                    return Ok(());
                }
                tracing::debug!("Kustomization watch error (will retry): {e}");
            }
        }
    }
    Ok(())
}

pub async fn run_gitrepo_watcher(
    client: kube::Client,
    writer: Writer<GitRepository>,
    tx: UnboundedSender<Action>,
) -> Result<()> {
    let api: Api<GitRepository> = Api::all(client);
    let mut stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .reflect(writer)
        .applied_objects()
        .boxed();

    while let Some(item) = stream.next().await {
        match item {
            Ok(_obj) => {
                let _ = tx.send(Action::GitRepoStoreUpdated);
            }
            Err(e) => {
                if crate::util::is_auth_error(&crate::util::error_chain(&e)) {
                    let _ = tx.send(Action::AuthExpired);
                    return Ok(());
                }
                if is_crd_missing(&e) {
                    let _ = tx.send(Action::GitRepoCrdMissing);
                    return Ok(());
                }
                tracing::debug!("GitRepository watch error (will retry): {e}");
            }
        }
    }
    Ok(())
}
