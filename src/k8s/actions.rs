use anyhow::{anyhow, Result};
use futures::{AsyncBufReadExt, StreamExt};
use k8s_openapi::api::core::v1::{ConfigMap, Event, Pod};
use kube::api::{Api, ListParams, LogParams, Patch, PatchParams};
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use crate::action::{Action, ResourceKind};
use crate::k8s::kustomization::Kustomization;
use crate::k8s::terraform::Terraform;

// -- Terraform actions --

pub async fn approve_plan(client: &kube::Client, ns: &str, name: &str) -> Result<()> {
    let api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let tf = api.get(name).await?;

    let plan_name = tf
        .status
        .as_ref()
        .and_then(|s| s.plan.as_ref())
        .and_then(|p| p.pending.as_ref())
        .ok_or_else(|| anyhow!("No pending plan for {}/{}", ns, name))?;

    let patch = json!({ "spec": { "approvePlan": plan_name } });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

pub async fn force_reconcile(client: &kube::Client, ns: &str, name: &str) -> Result<()> {
    let api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let patch = json!({
        "metadata": {
            "annotations": {
                "reconcile.fluxcd.io/requestedAt": jiff::Timestamp::now().to_string()
            }
        }
    });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

pub async fn replan(client: &kube::Client, ns: &str, name: &str) -> Result<()> {
    let api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let patch = json!({
        "metadata": {
            "annotations": {
                "replan.fluxcd.io/requestedAt": jiff::Timestamp::now().to_string()
            }
        }
    });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

pub async fn suspend(client: &kube::Client, ns: &str, name: &str) -> Result<()> {
    let api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let patch = json!({ "spec": { "suspend": true } });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

pub async fn resume(client: &kube::Client, ns: &str, name: &str) -> Result<()> {
    let api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let patch = json!({ "spec": { "suspend": false } });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

pub async fn force_unlock(client: &kube::Client, ns: &str, name: &str) -> Result<()> {
    let api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let patch = json!({ "spec": { "tfstate": { "forceUnlock": "auto" } } });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

// BTG is handled entirely by `tfctl break-glass` — see app.rs exec_break_the_glass

/// Fetch output secret key-value pairs for the detail view status panel.
pub async fn fetch_output_values(
    client: &kube::Client,
    ns: &str,
    name: &str,
) -> Result<std::collections::HashMap<String, String>> {
    let tf_api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let tf = tf_api.get(name).await?;

    let secret_name = tf
        .spec
        .write_outputs_to_secret
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("{}-outputs", name));

    let secret_api: Api<k8s_openapi::api::core::v1::Secret> =
        Api::namespaced(client.clone(), ns);
    let secret = secret_api.get(&secret_name).await?;

    let mut values = std::collections::HashMap::new();
    if let Some(data) = &secret.data {
        for (key, value) in data {
            let decoded = String::from_utf8_lossy(&value.0).to_string();
            // Unwrap JSON strings (remove outer quotes)
            if let Ok(serde_json::Value::String(s)) = serde_json::from_str(&decoded) {
                values.insert(key.clone(), s);
            } else {
                values.insert(key.clone(), decoded);
            }
        }
    }
    Ok(values)
}

pub async fn delete_terraform(client: &kube::Client, ns: &str, name: &str) -> Result<()> {
    let api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    api.delete(name, &Default::default()).await?;
    Ok(())
}

pub async fn delete_pod(client: &kube::Client, ns: &str, name: &str) -> Result<()> {
    let api: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client.clone(), ns);
    api.delete(name, &Default::default()).await?;
    Ok(())
}

/// Fetch the human-readable plan stored as ConfigMap(s) by tofu-controller.
/// Plans are stored with labels:
///   infra.contrib.fluxcd.io/plan-name: <safe-name>
///   infra.contrib.fluxcd.io/plan-workspace: <safe-workspace>
/// Data key: "tfplan"
pub async fn fetch_plan(
    client: &kube::Client,
    ns: &str,
    name: &str,
    workspace: Option<&str>,
) -> Result<String> {
    let workspace = workspace.unwrap_or("default");
    let safe_name = safe_label_value(name);
    let safe_ws = safe_label_value(workspace);

    let label_selector = format!(
        "infra.contrib.fluxcd.io/plan-name={},infra.contrib.fluxcd.io/plan-workspace={}",
        safe_name, safe_ws
    );

    let api: Api<ConfigMap> = Api::namespaced(client.clone(), ns);
    let lp = ListParams::default().labels(&label_selector);
    let cms = api.list(&lp).await?;

    if cms.items.is_empty() {
        return Err(anyhow!(
            "No plan ConfigMap found for {}/{} (workspace: {}). \
             Ensure spec.storeReadablePlan is set to \"human\".",
            ns,
            name,
            workspace
        ));
    }

    // Plans may be chunked across multiple ConfigMaps. Sort by name to get correct order.
    let mut cms = cms.items;
    cms.sort_by(|a, b| {
        let a_name = a.metadata.name.as_deref().unwrap_or("");
        let b_name = b.metadata.name.as_deref().unwrap_or("");
        a_name.cmp(b_name)
    });

    let mut plan_text = String::new();
    for cm in &cms {
        if let Some(data) = &cm.data
            && let Some(chunk) = data.get("tfplan") {
                plan_text.push_str(chunk);
            }
    }

    if plan_text.is_empty() {
        return Err(anyhow!("Plan ConfigMap exists but 'tfplan' key is empty"));
    }

    Ok(plan_text)
}

// -- Pod helpers --

pub fn get_container_names(pod: &Pod) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(spec) = &pod.spec {
        if let Some(init) = &spec.init_containers {
            for c in init {
                names.push(format!("init:{}", c.name));
            }
        }
        for c in &spec.containers {
            names.push(c.name.clone());
        }
    }
    names
}

// -- Kustomization actions --

pub async fn reconcile_kustomization(
    client: &kube::Client,
    ns: &str,
    name: &str,
) -> Result<()> {
    let api: Api<Kustomization> = Api::namespaced(client.clone(), ns);
    let patch = json!({
        "metadata": {
            "annotations": {
                "reconcile.fluxcd.io/requestedAt": jiff::Timestamp::now().to_string()
            }
        }
    });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

pub async fn suspend_kustomization(
    client: &kube::Client,
    ns: &str,
    name: &str,
) -> Result<()> {
    let api: Api<Kustomization> = Api::namespaced(client.clone(), ns);
    let patch = json!({ "spec": { "suspend": true } });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

pub async fn resume_kustomization(
    client: &kube::Client,
    ns: &str,
    name: &str,
) -> Result<()> {
    let api: Api<Kustomization> = Api::namespaced(client.clone(), ns);
    let patch = json!({ "spec": { "suspend": false } });
    api.patch(name, &PatchParams::apply("terrarium"), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

// -- JSON / YAML view --

pub async fn fetch_resource_json(
    client: &kube::Client,
    kind: &ResourceKind,
    ns: &str,
    name: &str,
) -> Result<String> {
    let value = fetch_resource_value(client, kind, ns, name).await?;
    Ok(serde_json::to_string_pretty(&value)?)
}

pub async fn fetch_resource_yaml(
    client: &kube::Client,
    kind: &ResourceKind,
    ns: &str,
    name: &str,
) -> Result<String> {
    let value = fetch_resource_value(client, kind, ns, name).await?;
    Ok(serde_yaml::to_string(&value)?)
}

async fn fetch_resource_value(
    client: &kube::Client,
    kind: &ResourceKind,
    ns: &str,
    name: &str,
) -> Result<serde_json::Value> {
    match kind {
        ResourceKind::Terraform => {
            let api: Api<Terraform> = Api::namespaced(client.clone(), ns);
            let tf = api.get(name).await?;
            Ok(serde_json::to_value(&tf)?)
        }
        ResourceKind::Kustomization => {
            let api: Api<Kustomization> = Api::namespaced(client.clone(), ns);
            let ks = api.get(name).await?;
            Ok(serde_json::to_value(&ks)?)
        }
        ResourceKind::Pod => {
            let api: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client.clone(), ns);
            let pod = api.get(name).await?;
            Ok(serde_json::to_value(&pod)?)
        }
    }
}

// -- Terraform outputs --

pub async fn fetch_outputs(
    client: &kube::Client,
    ns: &str,
    name: &str,
) -> Result<String> {
    // First get the Terraform resource to find the output secret name
    let tf_api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let tf = tf_api.get(name).await?;

    let secret_name = tf
        .spec
        .write_outputs_to_secret
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("{}-outputs", name));

    let available = tf
        .status
        .as_ref()
        .and_then(|s| s.available_outputs.as_ref())
        .cloned()
        .unwrap_or_default();

    if available.is_empty() {
        return Err(anyhow!("No outputs available for {}/{}", ns, name));
    }

    // Fetch the secret
    let secret_api: Api<k8s_openapi::api::core::v1::Secret> =
        Api::namespaced(client.clone(), ns);

    let secret = secret_api.get(&secret_name).await.map_err(|e| {
        anyhow!(
            "Could not read output secret '{}' in {}: {}",
            secret_name,
            ns,
            e
        )
    })?;

    let mut lines = Vec::new();
    lines.push(format!("Terraform Outputs for {}/{}", ns, name));
    lines.push(format!("Secret: {}/{}", ns, secret_name));
    lines.push(String::new());

    if let Some(data) = &secret.data {
        // Sort keys for consistent display
        let mut keys: Vec<&String> = data.keys().collect();
        keys.sort();

        for key in keys {
            if let Some(value) = data.get(key) {
                let decoded = String::from_utf8_lossy(&value.0);
                // Try to pretty-print JSON values
                if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&decoded) {
                    if json_val.is_object() || json_val.is_array() {
                        let pretty = serde_json::to_string_pretty(&json_val)
                            .unwrap_or_else(|_| decoded.to_string());
                        lines.push(format!("{}:", key));
                        for l in pretty.lines() {
                            lines.push(format!("  {}", l));
                        }
                    } else {
                        lines.push(format!("{}: {}", key, decoded));
                    }
                } else {
                    lines.push(format!("{}: {}", key, decoded));
                }
                lines.push(String::new());
            }
        }
    } else {
        lines.push("(secret has no data)".to_string());
    }

    Ok(lines.join("\n"))
}

// -- Events --

pub async fn fetch_events(
    client: &kube::Client,
    kind: &ResourceKind,
    ns: &str,
    name: &str,
) -> Result<String> {
    let api_kind = match kind {
        ResourceKind::Terraform => "Terraform",
        ResourceKind::Kustomization => "Kustomization",
        ResourceKind::Pod => "Pod",
    };

    let api: Api<Event> = Api::namespaced(client.clone(), ns);
    let field_selector = format!(
        "involvedObject.name={},involvedObject.kind={}",
        name, api_kind
    );
    let lp = ListParams::default().fields(&field_selector);
    let events = api.list(&lp).await?;

    if events.items.is_empty() {
        return Ok(format!("No events found for {} {}/{}", api_kind, ns, name));
    }

    let mut lines = Vec::new();
    let mut sorted = events.items;
    sorted.sort_by(|a, b| {
        let a_time = a.last_timestamp.as_ref().map(|t| &t.0);
        let b_time = b.last_timestamp.as_ref().map(|t| &t.0);
        a_time.cmp(&b_time)
    });

    for event in &sorted {
        let time = event
            .last_timestamp
            .as_ref()
            .map(|t| format!("{}", t.0))
            .unwrap_or_else(|| "-".to_string());
        let type_ = event.type_.as_deref().unwrap_or("Unknown");
        let reason = event.reason.as_deref().unwrap_or("-");
        let message = event.message.as_deref().unwrap_or("");
        let count = event.count.unwrap_or(1);

        lines.push(format!(
            "{} [{}] {} (x{}) — {}",
            time, type_, reason, count, message
        ));
    }

    Ok(lines.join("\n"))
}

// -- Log streaming --

pub async fn stream_pod_logs(
    client: &kube::Client,
    ns: &str,
    name: &str,
    container: Option<&str>,
    tx: UnboundedSender<Action>,
) -> Result<()> {
    let api: Api<Pod> = Api::namespaced(client.clone(), ns);
    let mut params = LogParams {
        follow: true,
        tail_lines: Some(1000),
        ..Default::default()
    };
    if let Some(c) = container {
        params.container = Some(c.to_string());
    }
    let stream = api.log_stream(name, &params).await?;
    let mut lines = futures::io::BufReader::new(stream).lines();

    // Batch lines together to avoid 1-action-per-line overhead.
    // Collect lines for up to 50ms before flushing, so the initial
    // burst of historical lines arrives as a few large chunks instead
    // of hundreds of tiny ones.
    let mut buf = String::new();
    loop {
        let deadline = tokio::time::sleep(std::time::Duration::from_millis(50));
        tokio::pin!(deadline);

        // Collect lines until the deadline fires or the stream ends.
        let stream_done = loop {
            tokio::select! {
                biased;
                line = lines.next() => {
                    match line {
                        Some(Ok(text)) => {
                            buf.push_str(&text);
                            buf.push('\n');
                        }
                        Some(Err(e)) => {
                            tracing::debug!("Log stream error: {}", e);
                            break true;
                        }
                        None => break true,
                    }
                }
                _ = &mut deadline => {
                    break false;
                }
            }
        };

        if !buf.is_empty() {
            let chunk = std::mem::take(&mut buf);
            if tx.send(Action::LogChunkReceived(chunk)).is_err() {
                break;
            }
        }

        if stream_done {
            break;
        }
    }

    Ok(())
}

/// Truncate to 63 chars max (Kubernetes label value limit).
/// If longer, take a prefix and append a hash suffix.
fn safe_label_value(value: &str) -> String {
    if value.len() <= 63 {
        return value.to_string();
    }
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    let hash = hasher.finish();
    let prefix = &value[..54]; // 54 + 1 dash + 8 hex = 63
    format!("{}-{:08x}", prefix, hash as u32)
}
