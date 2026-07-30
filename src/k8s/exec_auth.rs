//! Universal support for `exec` credential plugins (client-go exec auth):
//! OIDC (`kubectl oidc-login`), `aws eks get-token`, GKE's `gke-gcloud-auth-plugin`,
//! Azure, and any other plugin a kubeconfig points at.
//!
//! The problem this solves: kube-rs runs the exec plugin lazily on the
//! first API request, inheriting our stdin/stderr. When that first run is
//! *interactive* (e.g. OIDC opening a browser), its output lands on top of
//! the full-screen TUI and garbles it. We avoid that by running the plugin
//! ourselves, once, on a normal terminal — before entering the TUI at
//! startup, or by briefly suspending it on a context switch. That lets the
//! plugin do any interactive step cleanly and cache its own token, so
//! kube-rs's later invocation returns instantly and silently.
//!
//! Nothing here is provider-specific: we only resolve and run whatever the
//! kubeconfig declares.

use kube::config::{ExecConfig, Kubeconfig};

/// Resolve the exec credential plugin (if any) configured for `context`
/// in `kubeconfig`. `None` when the context uses `--context`'s default,
/// resolves to a non-exec auth method (token, client cert, …), or can't
/// be resolved at all.
pub fn exec_for_context(kubeconfig: &Kubeconfig, context: Option<&str>) -> Option<ExecConfig> {
    let ctx_name = context
        .map(str::to_string)
        .or_else(|| kubeconfig.current_context.clone())?;
    let ctx = kubeconfig
        .contexts
        .iter()
        .find(|c| c.name == ctx_name)?
        .context
        .as_ref()?;
    let user = ctx.user.as_ref()?;
    kubeconfig
        .auth_infos
        .iter()
        .find(|a| a.name == *user)?
        .auth_info
        .as_ref()?
        .exec
        .clone()
}

/// Run the exec credential plugin once on the *current* terminal so any
/// interactive step (browser login, prompt) happens on a normal screen and
/// the plugin can cache its token. stdin/stderr are inherited so the user
/// can see and answer prompts; stdout (the token JSON) is captured and
/// discarded — we only care that the plugin's own cache is warmed.
///
/// The caller MUST have suspended any full-screen TUI first.
pub fn prewarm(exec: &ExecConfig) -> anyhow::Result<()> {
    let command = exec
        .command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("exec auth entry has no command"))?;

    let mut cmd = std::process::Command::new(command);
    if let Some(args) = &exec.args {
        cmd.args(args);
    }
    if let Some(envs) = &exec.env {
        for e in envs {
            if let (Some(name), Some(value)) = (e.get("name"), e.get("value")) {
                cmd.env(name, value);
            }
        }
    }

    // Some plugins require KUBERNETES_EXEC_INFO to be present; provide a
    // minimal, spec-compliant value. Plugins that don't need it ignore it.
    let api_version = exec
        .api_version
        .as_deref()
        .unwrap_or("client.authentication.k8s.io/v1beta1");
    cmd.env(
        "KUBERNETES_EXEC_INFO",
        format!(
            r#"{{"apiVersion":"{api_version}","kind":"ExecCredential","spec":{{"interactive":true}}}}"#
        ),
    );

    cmd.stdin(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    cmd.stdout(std::process::Stdio::piped());

    let out = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run `{command}`: {e}"))?;
    if !out.status.success() {
        anyhow::bail!("`{command}` exited with {}", out.status);
    }
    Ok(())
}
