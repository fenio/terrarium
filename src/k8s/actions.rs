use crate::action::{Action, MutationTarget, ResourceKind, ScopedSender};
use crate::k8s::kustomization::Kustomization;
use crate::k8s::terraform::Terraform;
use anyhow::{Result, anyhow};
use futures::AsyncReadExt;
use k8s_openapi::api::core::v1::{ConfigMap, Event, Pod};
use kube::api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams, Preconditions};
use serde_json::json;

/// Annotation used by `tfctl break-glass` for a one-time BTG session.
pub const BREAK_THE_GLASS_ANNOTATION: &str = "break-the-glass.tf-controller/requestedAt";

/// Whether the controller will treat this Terraform as being in BTG mode.
/// Both the persistent spec flag and the one-time tfctl annotation activate it.
pub fn break_the_glass_active(terraform: &Terraform) -> bool {
    terraform.spec.break_the_glass.unwrap_or(false)
        || terraform
            .metadata
            .annotations
            .as_ref()
            .is_some_and(|annotations| annotations.contains_key(BREAK_THE_GLASS_ANNOTATION))
}

fn terraform_api(client: &kube::Client, target: &MutationTarget) -> Result<Api<Terraform>> {
    if target.kind != ResourceKind::Terraform {
        return Err(anyhow!(
            "mutation target kind mismatch: expected Terraform, got {:?}",
            target.kind
        ));
    }
    Ok(Api::namespaced(client.clone(), &target.namespace))
}

fn kustomization_api(client: &kube::Client, target: &MutationTarget) -> Result<Api<Kustomization>> {
    if target.kind != ResourceKind::Kustomization {
        return Err(anyhow!(
            "mutation target kind mismatch: expected Kustomization, got {:?}",
            target.kind
        ));
    }
    Ok(Api::namespaced(client.clone(), &target.namespace))
}

fn pod_api(client: &kube::Client, target: &MutationTarget) -> Result<Api<Pod>> {
    if target.kind != ResourceKind::Pod {
        return Err(anyhow!(
            "mutation target kind mismatch: expected Pod, got {:?}",
            target.kind
        ));
    }
    Ok(Api::namespaced(client.clone(), &target.namespace))
}

fn validate_metadata(
    target: &MutationTarget,
    metadata: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
) -> Result<()> {
    let current_uid = metadata
        .uid
        .as_deref()
        .ok_or_else(|| anyhow!("{} has no UID; refusing mutation", target.name))?;
    if current_uid != target.uid {
        return Err(anyhow!(
            "{} has been recreated (UID changed); refusing mutation",
            target.name
        ));
    }

    let current_resource_version = metadata
        .resource_version
        .as_deref()
        .ok_or_else(|| anyhow!("{} has no resourceVersion; refusing mutation", target.name))?;
    if current_resource_version != target.resource_version {
        return Err(anyhow!(
            "{} changed since it was selected; refusing mutation",
            target.name
        ));
    }
    Ok(())
}

async fn validated_terraform(api: &Api<Terraform>, target: &MutationTarget) -> Result<Terraform> {
    let terraform = api.get(&target.name).await?;
    validate_metadata(target, &terraform.metadata)?;
    Ok(terraform)
}

async fn validated_kustomization(
    api: &Api<Kustomization>,
    target: &MutationTarget,
) -> Result<Kustomization> {
    let kustomization = api.get(&target.name).await?;
    validate_metadata(target, &kustomization.metadata)?;
    Ok(kustomization)
}

/// Revalidate a Terraform target immediately before an external mutation such
/// as `tfctl break-glass` runs. The UID check prevents a same-name replacement
/// from receiving the action; the resourceVersion check rejects any object
/// that changed while the action was waiting for confirmation.
pub async fn validate_terraform_target(
    client: &kube::Client,
    target: &MutationTarget,
) -> Result<()> {
    let api = terraform_api(client, target)?;
    validated_terraform(&api, target).await.map(|_| ())
}

fn patch_with_resource_version(
    target: &MutationTarget,
    mut patch: serde_json::Value,
) -> Result<serde_json::Value> {
    let object = patch
        .as_object_mut()
        .ok_or_else(|| anyhow!("mutation patch must be a JSON object"))?;
    let metadata = object
        .entry("metadata")
        .or_insert_with(|| serde_json::json!({}));
    let metadata = metadata
        .as_object_mut()
        .ok_or_else(|| anyhow!("mutation patch metadata must be a JSON object"))?;
    metadata.insert(
        "resourceVersion".to_string(),
        serde_json::Value::String(target.resource_version.clone()),
    );
    Ok(patch)
}

fn delete_params(target: &MutationTarget) -> DeleteParams {
    DeleteParams::default().preconditions(Preconditions {
        uid: Some(target.uid.clone()),
        resource_version: Some(target.resource_version.clone()),
    })
}

// -- Terraform actions --

pub async fn approve_plan(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api: Api<Terraform> = terraform_api(client, target)?;
    let tf = validated_terraform(&api, target).await?;

    let plan_name = tf
        .status
        .as_ref()
        .and_then(|s| s.plan.as_ref())
        .and_then(|p| p.pending.as_ref())
        .ok_or_else(|| anyhow!("No pending plan for {}/{}", target.namespace, target.name))?;

    let patch =
        patch_with_resource_version(target, json!({ "spec": { "approvePlan": plan_name } }))?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

pub async fn force_reconcile(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api: Api<Terraform> = terraform_api(client, target)?;
    validated_terraform(&api, target).await?;
    let patch = patch_with_resource_version(
        target,
        json!({
        "metadata": {
            "annotations": {
                "reconcile.fluxcd.io/requestedAt": jiff::Timestamp::now().to_string()
            }
        }
        }),
    )?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

pub async fn replan(
    client: &kube::Client,
    target: &MutationTarget,
    context: Option<&str>,
    kubeconfig: Option<&std::path::Path>,
) -> Result<()> {
    let api: Api<Terraform> = terraform_api(client, target)?;
    validated_terraform(&api, target).await?;
    // Delegate to tfctl rather than reimplementing the K8s patch logic —
    // both prior attempts (annotation, spec.approvePlan) failed to trigger
    // a replan because the controller's actual mechanism is more involved
    // (clearing pending plan, deleting plan ConfigMap, bumping revision).
    // tfctl gets it right and is the supported tool, so just shell out
    // (same pattern as Break-the-Glass; see exec_break_the_glass in
    // app.rs). This call is non-interactive — no TUI suspend needed.
    //
    // Forward terrarium's active kubeconfig context so the subprocess
    // doesn't fall back to the kubeconfig's default current-context (a
    // different cluster than what the TUI is showing).
    let mut cmd = tokio::process::Command::new("tfctl");
    // Point tfctl at the same kubeconfig terrarium is using. With a
    // `[switcher] builder` the merged kubeconfig lives only in memory + this
    // temp file, so without it tfctl can't resolve `--context` at all.
    if let Some(kc) = kubeconfig {
        cmd.env("KUBECONFIG", kc);
    }
    if let Some(ctx) = context.filter(|c| !c.is_empty() && *c != "connecting...") {
        cmd.args(["--context", ctx]);
    }
    cmd.args(["-n", &target.namespace, "replan", &target.name]);
    // Minimize the validation-to-execution window for the external command.
    // tfctl accepts only namespace/name, so unlike the direct API patches it
    // cannot carry Kubernetes preconditions itself.
    validated_terraform(&api, target).await?;
    let output = cmd
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to run tfctl: {e} — is tfctl installed?"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stderr}{stdout}").trim().to_string();
        if combined.is_empty() {
            return Err(anyhow::anyhow!(
                "tfctl exited with code {}",
                output.status.code().unwrap_or(-1)
            ));
        }
        return Err(anyhow::anyhow!("tfctl: {combined}"));
    }
    Ok(())
}

pub async fn suspend(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api: Api<Terraform> = terraform_api(client, target)?;
    validated_terraform(&api, target).await?;
    let patch = patch_with_resource_version(target, json!({ "spec": { "suspend": true } }))?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

pub async fn resume(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api: Api<Terraform> = terraform_api(client, target)?;
    validated_terraform(&api, target).await?;
    let patch = patch_with_resource_version(target, json!({ "spec": { "suspend": false } }))?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

pub async fn force_unlock(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api: Api<Terraform> = terraform_api(client, target)?;
    validated_terraform(&api, target).await?;
    let patch = patch_with_resource_version(
        target,
        json!({ "spec": { "tfstate": { "forceUnlock": "auto" } } }),
    )?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

/// Disable persistent break-the-glass mode on a Terraform resource.
///
/// This clears both persistent `spec.breakTheGlass` mode and the annotation
/// used by a one-time `tfctl break-glass` session. The latter is normally
/// removed by tfctl's deferred cleanup, but can remain after an interrupted
/// or stuck process. It is intentionally a merge patch so it changes only
/// these BTG markers and leaves the rest of the object untouched.
pub async fn reset_break_the_glass(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api: Api<Terraform> = terraform_api(client, target)?;
    validated_terraform(&api, target).await?;
    let patch = patch_with_resource_version(target, reset_break_the_glass_patch())?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

/// Remove all finalizers from a Terraform object after validating the exact
/// object selected by the operator. This bypasses controller cleanup and can
/// orphan managed infrastructure, so the UI requires explicit confirmation.
pub async fn remove_terraform_finalizers(
    client: &kube::Client,
    target: &MutationTarget,
) -> Result<()> {
    let api = terraform_api(client, target)?;
    validated_terraform(&api, target).await?;
    let patch = patch_with_resource_version(target, remove_finalizers_patch())?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

fn remove_finalizers_patch() -> serde_json::Value {
    json!({ "metadata": { "finalizers": null } })
}

fn reset_break_the_glass_patch() -> serde_json::Value {
    json!({
        "spec": { "breakTheGlass": false },
        "metadata": {
            "annotations": {
                BREAK_THE_GLASS_ANNOTATION: null
            }
        }
    })
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
        .unwrap_or_else(|| format!("{name}-outputs"));

    let secret_api: Api<k8s_openapi::api::core::v1::Secret> = Api::namespaced(client.clone(), ns);
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

/// Fetch all data keys from an arbitrary Secret in `ns`. Generic — used
/// by the shortcut template engine's `{secret.<name>.<key>}` placeholder
/// so users can reference any cluster-side Secret (e.g. tofu-controller's
/// `varsFrom` input secret) without baking platform conventions into the
/// codebase. Values that decode as JSON-quoted strings are unwrapped to
/// match the convention used by `fetch_output_values`.
pub async fn fetch_secret_values(
    client: &kube::Client,
    ns: &str,
    secret_name: &str,
) -> Result<std::collections::HashMap<String, String>> {
    let api: Api<k8s_openapi::api::core::v1::Secret> = Api::namespaced(client.clone(), ns);
    let secret = api.get(secret_name).await?;

    let mut values = std::collections::HashMap::new();
    if let Some(data) = &secret.data {
        for (key, value) in data {
            let decoded = String::from_utf8_lossy(&value.0).to_string();
            if let Ok(serde_json::Value::String(s)) = serde_json::from_str(&decoded) {
                values.insert(key.clone(), s);
            } else {
                values.insert(key.clone(), decoded);
            }
        }
    }
    Ok(values)
}

pub async fn delete_terraform(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api = terraform_api(client, target)?;
    validated_terraform(&api, target).await?;
    api.delete(&target.name, &delete_params(target)).await?;
    Ok(())
}

pub async fn delete_kustomization(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api = kustomization_api(client, target)?;
    validated_kustomization(&api, target).await?;
    api.delete(&target.name, &delete_params(target)).await?;
    Ok(())
}

pub async fn delete_pod(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api = pod_api(client, target)?;
    let pod = api.get(&target.name).await?;
    validate_metadata(target, &pod.metadata)?;
    api.delete(&target.name, &delete_params(target)).await?;
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
        "infra.contrib.fluxcd.io/plan-name={safe_name},infra.contrib.fluxcd.io/plan-workspace={safe_ws}"
    );

    let api: Api<ConfigMap> = Api::namespaced(client.clone(), ns);
    let lp = ListParams::default().labels(&label_selector);
    let cms = api.list(&lp).await?;

    if cms.items.is_empty() {
        return Err(anyhow!(
            "No plan ConfigMap found for {ns}/{name} (workspace: {workspace}). \
             Ensure spec.storeReadablePlan is set to \"human\"."
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
            && let Some(chunk) = data.get("tfplan")
        {
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

pub async fn reconcile_kustomization(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api = kustomization_api(client, target)?;
    validated_kustomization(&api, target).await?;
    let patch = patch_with_resource_version(
        target,
        json!({
        "metadata": {
            "annotations": {
                "reconcile.fluxcd.io/requestedAt": jiff::Timestamp::now().to_string()
            }
        }
        }),
    )?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

pub async fn suspend_kustomization(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api = kustomization_api(client, target)?;
    validated_kustomization(&api, target).await?;
    let patch = patch_with_resource_version(target, json!({ "spec": { "suspend": true } }))?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

pub async fn resume_kustomization(client: &kube::Client, target: &MutationTarget) -> Result<()> {
    let api = kustomization_api(client, target)?;
    validated_kustomization(&api, target).await?;
    let patch = patch_with_resource_version(target, json!({ "spec": { "suspend": false } }))?;
    api.patch(
        &target.name,
        &PatchParams::apply("terrarium"),
        &Patch::Merge(&patch),
    )
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

pub async fn fetch_outputs(client: &kube::Client, ns: &str, name: &str) -> Result<String> {
    // First get the Terraform resource to find the output secret name
    let tf_api: Api<Terraform> = Api::namespaced(client.clone(), ns);
    let tf = tf_api.get(name).await?;

    let secret_name = tf
        .spec
        .write_outputs_to_secret
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("{name}-outputs"));

    let available = tf
        .status
        .as_ref()
        .and_then(|s| s.available_outputs.as_ref())
        .cloned()
        .unwrap_or_default();

    if available.is_empty() {
        return Err(anyhow!("No outputs available for {ns}/{name}"));
    }

    // Fetch the secret
    let secret_api: Api<k8s_openapi::api::core::v1::Secret> = Api::namespaced(client.clone(), ns);

    let secret = secret_api
        .get(&secret_name)
        .await
        .map_err(|e| anyhow!("Could not read output secret '{secret_name}' in {ns}: {e}"))?;

    let mut lines = Vec::new();
    lines.push(format!("Terraform Outputs for {ns}/{name}"));
    lines.push(format!("Secret: {ns}/{secret_name}"));
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
                        lines.push(format!("{key}:"));
                        for l in pretty.lines() {
                            lines.push(format!("  {l}"));
                        }
                    } else {
                        lines.push(format!("{key}: {decoded}"));
                    }
                } else {
                    lines.push(format!("{key}: {decoded}"));
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
    let field_selector = format!("involvedObject.name={name},involvedObject.kind={api_kind}");
    let lp = ListParams::default().fields(&field_selector);
    let events = api.list(&lp).await?;

    if events.items.is_empty() {
        return Ok(format!("No events found for {api_kind} {ns}/{name}"));
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

        lines.push(format!("{time} [{type_}] {reason} (x{count}) — {message}"));
    }

    Ok(lines.join("\n"))
}

// -- Log streaming --

const LOG_BATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
const MAX_LOG_CHUNK_BYTES: usize = 64 * 1024;

fn take_log_chunk(buf: &mut Vec<u8>, stream_done: bool) -> Option<String> {
    if buf.is_empty() {
        return None;
    }
    let mut chunk = String::new();
    let mut consumed = 0;

    while consumed < buf.len() && chunk.len() < MAX_LOG_CHUNK_BYTES {
        let remaining = &buf[consumed..];
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                let mut take = valid.len().min(MAX_LOG_CHUNK_BYTES - chunk.len());
                while !valid.is_char_boundary(take) {
                    take -= 1;
                }
                chunk.push_str(&valid[..take]);
                consumed += take;
                break;
            }
            Err(error) if error.valid_up_to() > 0 => {
                let valid = std::str::from_utf8(&remaining[..error.valid_up_to()])
                    .expect("UTF-8 error prefix is valid");
                let mut take = valid.len().min(MAX_LOG_CHUNK_BYTES - chunk.len());
                while !valid.is_char_boundary(take) {
                    take -= 1;
                }
                chunk.push_str(&valid[..take]);
                consumed += take;
                if take < valid.len() {
                    break;
                }
            }
            Err(error) => match error.error_len() {
                Some(invalid_bytes) if MAX_LOG_CHUNK_BYTES - chunk.len() >= 3 => {
                    chunk.push('\u{fffd}');
                    consumed += invalid_bytes;
                }
                None if stream_done && MAX_LOG_CHUNK_BYTES - chunk.len() >= 3 => {
                    chunk.push('\u{fffd}');
                    consumed = buf.len();
                }
                _ => break,
            },
        }
    }

    if consumed == 0 {
        None
    } else {
        buf.drain(..consumed);
        Some(chunk)
    }
}

pub async fn stream_pod_logs(
    client: &kube::Client,
    ns: &str,
    name: &str,
    container: Option<&str>,
    stream_id: u64,
    tx: ScopedSender,
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
    let mut stream = api.log_stream(name, &params).await?;

    // Batch stream data for up to 50ms before flushing, so the initial
    // history arrives as a few bounded chunks instead of hundreds of
    // tiny actions.
    let mut buf = Vec::with_capacity(MAX_LOG_CHUNK_BYTES);
    let mut read_buf = [0_u8; 8 * 1024];
    let mut stream_done = false;
    loop {
        if !stream_done && buf.len() < MAX_LOG_CHUNK_BYTES {
            let deadline = tokio::time::sleep(LOG_BATCH_INTERVAL);
            tokio::pin!(deadline);

            // Bound batches by both time and size. Check the timer first so
            // an always-ready log stream cannot starve it.
            loop {
                let read_len = (MAX_LOG_CHUNK_BYTES - buf.len()).min(read_buf.len());
                tokio::select! {
                    biased;
                    _ = &mut deadline => break,
                    read = stream.read(&mut read_buf[..read_len]) => {
                        match read {
                            Ok(0) => {
                                stream_done = true;
                                break;
                            }
                            Ok(read) => {
                                buf.extend_from_slice(&read_buf[..read]);
                                if buf.len() == MAX_LOG_CHUNK_BYTES {
                                    break;
                                }
                            }
                            Err(error) => {
                                tracing::debug!("Log stream error: {error}");
                                stream_done = true;
                                break;
                            }
                        }
                    }
                }
            }
        }

        if let Some(chunk) = take_log_chunk(&mut buf, stream_done)
            && tx
                .send(Action::LogChunkReceived { stream_id, chunk })
                .await
                .is_err()
        {
            break;
        }

        if stream_done && buf.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::{
        BREAK_THE_GLASS_ANNOTATION, MAX_LOG_CHUNK_BYTES, break_the_glass_active, delete_params,
        patch_with_resource_version, remove_finalizers_patch, reset_break_the_glass_patch,
        take_log_chunk, validate_metadata,
    };
    use crate::action::{MutationTarget, ResourceKind};
    use crate::k8s::terraform::Terraform;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn target() -> MutationTarget {
        MutationTarget {
            kind: ResourceKind::Terraform,
            namespace: "ns".into(),
            name: "demo".into(),
            uid: "uid-old".into(),
            resource_version: "17".into(),
        }
    }

    fn metadata(uid: &str, resource_version: &str) -> ObjectMeta {
        ObjectMeta {
            uid: Some(uid.into()),
            resource_version: Some(resource_version.into()),
            ..Default::default()
        }
    }

    #[test]
    fn mutation_metadata_accepts_the_selected_object() {
        assert!(validate_metadata(&target(), &metadata("uid-old", "17")).is_ok());
    }

    #[test]
    fn mutation_metadata_rejects_a_recreated_object() {
        let error = validate_metadata(&target(), &metadata("uid-new", "17"))
            .expect_err("a recreated object must not be mutated");
        assert!(error.to_string().contains("recreated"));
    }

    #[test]
    fn mutation_metadata_rejects_a_changed_object() {
        let error = validate_metadata(&target(), &metadata("uid-old", "18"))
            .expect_err("a changed object must not be mutated");
        assert!(error.to_string().contains("changed"));
    }

    #[test]
    fn mutation_patch_carries_resource_version() {
        let patch = patch_with_resource_version(
            &target(),
            serde_json::json!({
                "spec": {"suspend": true}
            }),
        )
        .expect("object patch should be accepted");
        assert_eq!(patch["metadata"]["resourceVersion"], "17");
        assert_eq!(patch["spec"]["suspend"], true);
    }

    #[test]
    fn remove_finalizers_patch_carries_resource_version_and_only_finalizer_removal() {
        let patch = patch_with_resource_version(&target(), remove_finalizers_patch())
            .expect("finalizer patch should be accepted");
        assert_eq!(patch["metadata"]["finalizers"], serde_json::Value::Null);
        assert_eq!(patch["metadata"]["resourceVersion"], "17");
        assert_eq!(patch["metadata"].as_object().map(|m| m.len()), Some(2));
        assert!(patch.get("spec").is_none());
    }

    #[test]
    fn delete_params_carry_uid_and_resource_version_preconditions() {
        let params =
            serde_json::to_value(delete_params(&target())).expect("delete params serialize");
        assert_eq!(
            params["preconditions"],
            serde_json::json!({"uid": "uid-old", "resourceVersion": "17"})
        );
    }

    #[test]
    fn kustomization_delete_params_carry_uid_and_resource_version_preconditions() {
        let target = MutationTarget {
            kind: ResourceKind::Kustomization,
            namespace: "ns".into(),
            name: "demo".into(),
            uid: "uid-ks".into(),
            resource_version: "23".into(),
        };
        let params = serde_json::to_value(delete_params(&target)).expect("delete params serialize");
        assert_eq!(
            params["preconditions"],
            serde_json::json!({"uid": "uid-ks", "resourceVersion": "23"})
        );
    }

    #[test]
    fn break_glass_reset_patch_disables_the_spec_flag() {
        let patch = reset_break_the_glass_patch();
        assert_eq!(patch["spec"]["breakTheGlass"], false);
        assert_eq!(
            patch["metadata"]["annotations"][BREAK_THE_GLASS_ANNOTATION],
            serde_json::Value::Null
        );
    }

    #[test]
    fn annotation_alone_marks_break_glass_active() {
        let mut terraform: Terraform = serde_json::from_value(serde_json::json!({
            "apiVersion": "infra.contrib.fluxcd.io/v1alpha2",
            "kind": "Terraform",
            "metadata": {"name": "demo", "namespace": "ns"},
            "spec": {
                "interval": "1m",
                "sourceRef": {"kind": "GitRepository", "name": "source"}
            }
        }))
        .expect("minimal Terraform should deserialize");
        terraform.metadata.annotations = Some(
            [(BREAK_THE_GLASS_ANNOTATION.to_string(), "now".to_string())]
                .into_iter()
                .collect(),
        );
        assert!(break_the_glass_active(&terraform));
    }

    #[test]
    fn log_chunks_are_size_bounded_and_preserve_utf8() {
        let mut buf = vec![b'a'; MAX_LOG_CHUNK_BYTES - 1];
        buf.push(0xc3);

        let first = take_log_chunk(&mut buf, false).expect("non-empty prefix");
        assert_eq!(first.len(), MAX_LOG_CHUNK_BYTES - 1);
        assert_eq!(buf, [0xc3]);

        buf.push(0xa9);
        assert_eq!(take_log_chunk(&mut buf, false).unwrap(), "é");
    }

    #[test]
    fn log_chunks_replace_invalid_utf8_without_dropping_valid_bytes() {
        let mut buf = b"before\xffafter".to_vec();

        let chunk = take_log_chunk(&mut buf, true).expect("log chunk");

        assert_eq!(chunk, "before\u{fffd}after");
        assert!(chunk.len() <= MAX_LOG_CHUNK_BYTES);
        assert!(buf.is_empty());
    }

    #[test]
    fn log_chunks_replace_truncated_utf8_at_eof() {
        let mut buf = b"before\xc3".to_vec();

        assert_eq!(take_log_chunk(&mut buf, true).unwrap(), "before\u{fffd}");
        assert!(buf.is_empty());
    }
}
