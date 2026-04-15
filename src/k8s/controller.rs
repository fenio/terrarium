use anyhow::Result;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::state::store::{ControllerInfo, ControllerPodInfo};
use crate::util;

const CONTROLLER_LABEL: &str = "control-plane=tofu-controller";
// Common deployment names to try
const DEPLOY_NAMES: &[&str] = &["tofu-controller", "tf-controller"];

pub async fn poll_controller_info(
    client: kube::Client,
    tx: UnboundedSender<Action>,
    controller_ns: String,
) -> Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));

    loop {
        interval.tick().await;

        let info = fetch_controller_info(&client, &controller_ns).await;
        let _ = tx.send(Action::ControllerInfoUpdated(info));
    }
}

async fn fetch_controller_info(client: &kube::Client, ns: &str) -> ControllerInfo {
    let mut info = ControllerInfo {
        deploy_namespace: ns.to_string(),
        ..Default::default()
    };

    // Try to find the deployment
    let deploy_api: Api<Deployment> = Api::namespaced(client.clone(), ns);
    let mut found_deploy = false;
    for name in DEPLOY_NAMES {
        if let Ok(deploy) = deploy_api.get(name).await {
            info.deploy_name = name.to_string();
            populate_from_deploy(&mut info, &deploy);
            found_deploy = true;
            break;
        }
    }

    if !found_deploy {
        // Try label-based search
        let lp = ListParams::default().labels(CONTROLLER_LABEL);
        if let Ok(deploys) = deploy_api.list(&lp).await
            && let Some(deploy) = deploys.items.first() {
                info.deploy_name = deploy
                    .metadata
                    .name
                    .clone()
                    .unwrap_or_default();
                populate_from_deploy(&mut info, deploy);
                found_deploy = true;
            }
    }

    if !found_deploy {
        info.error = Some(format!(
            "Controller deployment not found in namespace '{}'",
            ns
        ));
        return info;
    }

    // Fetch controller pods — try multiple label selectors
    let pod_api: Api<Pod> = Api::namespaced(client.clone(), ns);
    let label_selectors = [
        CONTROLLER_LABEL,
        "app.kubernetes.io/name=tofu-controller",
        "app.kubernetes.io/name=tf-controller",
    ];
    let mut pod_items = Vec::new();
    for selector in &label_selectors {
        if let Ok(pods) = pod_api.list(&ListParams::default().labels(selector)).await {
            if !pods.items.is_empty() {
                pod_items = pods.items;
                break;
            }
        }
    }
    if !pod_items.is_empty() {
        info.pods = pod_items
            .iter()
            .map(|pod| {
                let name = pod.metadata.name.clone().unwrap_or_default();
                let phase = pod
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.clone())
                    .unwrap_or_else(|| "Unknown".to_string());

                let ready = pod
                    .status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                    .map(|c| c.status == "True")
                    .unwrap_or(false);

                let restarts = pod
                    .status
                    .as_ref()
                    .and_then(|s| s.container_statuses.as_ref())
                    .and_then(|cs| cs.first())
                    .map(|c| c.restart_count)
                    .unwrap_or(0);

                let age = pod
                    .metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|ts| util::format_duration(util::secs_since(ts.0)))
                    .unwrap_or_else(|| "-".to_string());

                ControllerPodInfo {
                    name,
                    phase,
                    ready,
                    restarts,
                    age,
                }
            })
            .collect();
    }

    info
}

fn populate_from_deploy(info: &mut ControllerInfo, deploy: &Deployment) {
    let spec = deploy.spec.as_ref();
    info.replicas_desired = spec.and_then(|s| s.replicas).unwrap_or(1);

    let status = deploy.status.as_ref();
    info.replicas_ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);

    let container = spec
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|ps| ps.containers.first());

    info.image = container
        .and_then(|c| c.image.clone())
        .unwrap_or_default();

    // Extract --concurrent from container args
    info.max_concurrent = container
        .and_then(|c| c.args.as_ref())
        .and_then(|args| {
            args.iter()
                .find_map(|arg| arg.strip_prefix("--concurrent="))
                .and_then(|v| v.parse::<i32>().ok())
                .or_else(|| {
                    // Handle "--concurrent 10" (two separate args)
                    let pos = args.iter().position(|a| a == "--concurrent")?;
                    args.get(pos + 1)?.parse::<i32>().ok()
                })
        });
}
