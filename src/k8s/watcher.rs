use anyhow::Result;
use futures::TryStreamExt;
use kube::{
    api::Api,
    runtime::{
        reflector::{self, store::Writer},
        watcher, WatchStreamExt,
    },
};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::k8s::kustomization::Kustomization;
use crate::k8s::terraform::Terraform;

pub type TfStore = reflector::Store<Terraform>;
pub type KsStore = reflector::Store<Kustomization>;

pub fn create_tf_store() -> (TfStore, Writer<Terraform>) {
    reflector::store()
}

pub fn create_ks_store() -> (KsStore, Writer<Kustomization>) {
    reflector::store()
}

pub async fn run_tf_watcher(
    client: kube::Client,
    writer: Writer<Terraform>,
    tx: UnboundedSender<Action>,
) -> Result<()> {
    let api: Api<Terraform> = Api::all(client);
    let result = watcher(api, watcher::Config::default())
        .default_backoff()
        .reflect(writer)
        .applied_objects()
        .try_for_each(|_obj| {
            let _ = tx.send(Action::TerraformStoreUpdated);
            futures::future::ready(Ok(()))
        })
        .await;

    if let Err(e) = &result {
        let msg = format!("{}", e);
        if msg.contains("404") || msg.contains("not found") || msg.contains("the server could not find the requested resource") {
            let _ = tx.send(Action::TerraformCrdMissing);
            return Ok(());
        }
    }
    result?;
    Ok(())
}

pub async fn run_ks_watcher(
    client: kube::Client,
    writer: Writer<Kustomization>,
    tx: UnboundedSender<Action>,
) -> Result<()> {
    let api: Api<Kustomization> = Api::all(client);
    let result = watcher(api, watcher::Config::default())
        .default_backoff()
        .reflect(writer)
        .applied_objects()
        .try_for_each(|_obj| {
            let _ = tx.send(Action::KustomizationStoreUpdated);
            futures::future::ready(Ok(()))
        })
        .await;

    if let Err(e) = &result {
        let msg = format!("{}", e);
        if msg.contains("404") || msg.contains("not found") || msg.contains("the server could not find the requested resource") {
            let _ = tx.send(Action::KustomizationCrdMissing);
            return Ok(());
        }
    }
    result?;
    Ok(())
}
