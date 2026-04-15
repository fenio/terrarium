use std::collections::HashMap;

use anyhow::Result;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, ListParams, LogParams},
    Client,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;

const RUNNER_LABEL: &str = "infra.contrib.fluxcd.io/terraform";

pub async fn poll_runner_pods(
    client: Client,
    tx: UnboundedSender<Action>,
    namespace: Option<String>,
) -> Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));

    loop {
        interval.tick().await;

        let pods = list_runner_pods(&client, namespace.as_deref()).await;
        match pods {
            Ok(pods) => {
                // Fetch logs for Running pods in parallel
                let logs = fetch_active_runner_logs(&client, &pods).await;
                let _ = tx.send(Action::RunnerLogsUpdated(logs));
                let _ = tx.send(Action::RunnerPodsUpdated(pods));
            }
            Err(e) => {
                tracing::warn!("Failed to list runner pods: {}", e);
            }
        }
    }
}

async fn list_runner_pods(client: &Client, namespace: Option<&str>) -> Result<Vec<Pod>> {
    let lp = ListParams::default().labels(RUNNER_LABEL);

    let pods = match namespace {
        Some(ns) => {
            let api: Api<Pod> = Api::namespaced(client.clone(), ns);
            api.list(&lp).await?
        }
        None => {
            let api: Api<Pod> = Api::all(client.clone());
            api.list(&lp).await?
        }
    };

    Ok(pods.items)
}

async fn fetch_active_runner_logs(
    client: &Client,
    pods: &[Pod],
) -> HashMap<(String, String), String> {
    let mut futures = Vec::new();

    for pod in pods {
        let phase = pod
            .status
            .as_ref()
            .and_then(|s| s.phase.as_deref())
            .unwrap_or("");

        // Skip terminal Failed pods
        if phase == "Failed" {
            continue;
        }

        let ns = match pod.metadata.namespace.as_deref() {
            Some(ns) => ns,
            None => continue,
        };
        let pod_name = match pod.metadata.name.as_deref() {
            Some(n) => n,
            None => continue,
        };
        // Runner pod name is "{tf-resource-name}-tf-runner"
        let tf_name = match pod_name.strip_suffix("-tf-runner") {
            Some(n) => n.to_string(),
            None => continue,
        };

        let api: Api<Pod> = Api::namespaced(client.clone(), ns);
        let ns_owned = ns.to_string();
        let pod_name_owned = pod_name.to_string();

        futures.push(async move {
            let result = api
                .logs(
                    &pod_name_owned,
                    &LogParams {
                        tail_lines: Some(100),
                        ..Default::default()
                    },
                )
                .await;

            match result {
                Ok(log_text) if !log_text.is_empty() => {
                    Some(((ns_owned, tf_name), log_text))
                }
                Err(e) => {
                    tracing::debug!("Failed to fetch logs for {}/{}: {}", ns_owned, pod_name_owned, e);
                    None
                }
                _ => None,
            }
        });
    }

    let results = futures::future::join_all(futures).await;
    results.into_iter().flatten().collect()
}
