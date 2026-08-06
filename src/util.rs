/// Format a duration in seconds to a human-readable relative time string.
/// e.g. 90061 -> "1d", 7200 -> "2h", 300 -> "5m", 45 -> "45s"
pub fn format_duration(total_secs: i64) -> String {
    let days = total_secs / 86400;
    let hours = total_secs / 3600;
    let minutes = total_secs / 60;

    if days > 0 {
        format!("{days}d")
    } else if hours > 0 {
        format!("{hours}h")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{total_secs}s")
    }
}

/// Format a duration with "ago" suffix for relative timestamps.
pub fn format_duration_ago(total_secs: i64) -> String {
    format!("{} ago", format_duration(total_secs))
}

/// Calculate seconds elapsed since a jiff Timestamp.
pub fn secs_since(ts: jiff::Timestamp) -> i64 {
    let now = jiff::Timestamp::now();
    now.since(ts).unwrap_or_default().get_seconds()
}

/// Turn a raw cluster/API error (often a verbose `kube` Debug dump like
/// `ApiError: Unauthorized: Failed to parse error data (Status { … })`) into
/// a short, human-readable, actionable message for the UI. The full text is
/// still written to the log file for diagnosis.
pub fn humanize_cluster_error(raw: &str) -> String {
    let lower = raw.to_lowercase();

    // A failing `exec`/OIDC credential plugin (the plugin couldn't mint a
    // token — browser login timed out, network down, etc.). Distinct from a
    // 401 where the API rejected an otherwise-obtained token.
    if is_auth_error(raw) {
        return "Cluster authentication failed (OIDC/exec credential plugin). \
                Re-authenticate — press Ctrl-X to reconnect — and check your network/VPN."
            .to_string();
    }

    if lower.contains("401") || lower.contains("unauthorized") {
        return "Unauthorized (401): your credentials are invalid or expired. \
                Re-authenticate — press Ctrl-X to reconnect the context."
            .to_string();
    }
    if lower.contains("403") || lower.contains("forbidden") {
        return "Forbidden (403): your account lacks permission on this cluster.".to_string();
    }
    if lower.contains("connection refused")
        || lower.contains("dns error")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("failed to lookup")
        || lower.contains("tcp connect error")
        || lower.contains("no route to host")
        || lower.contains("network is unreachable")
    {
        return "Cannot reach the cluster API server. Check your kubeconfig, \
                network/VPN, and that the context points at a running cluster."
            .to_string();
    }

    // Otherwise strip the noisy `Status { … }` Debug framing and collapse
    // whitespace so a single readable line remains.
    strip_status_debug(raw)
}

/// Flatten an error and its `source()` chain into one string, so pattern
/// checks (e.g. [`is_auth_error`]) see the underlying cause and not just the
/// wrapper's top-level message.
pub fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(inner) = src {
        s.push_str(": ");
        s.push_str(&inner.to_string());
        src = inner.source();
    }
    s
}

/// Whether an error comes from a failing `exec`/OIDC credential plugin —
/// i.e. the interactive login couldn't produce a token (browser flow timed
/// out, refresh token expired, network down).
///
/// This is the trigger to *stop* retrying: kube-rs re-runs the plugin on
/// every API call, and an interactive plugin (`oidc-login --grant-type=
/// authcode-browser`) opens a browser tab each time. Left to retry across
/// several watchers/pollers overnight, that piles up hundreds of tabs — so
/// on this error we tear the connection down and wait for a deliberate
/// re-auth (Ctrl-X) instead.
pub fn is_auth_error(raw: &str) -> bool {
    let lower = raw.to_lowercase();
    lower.contains("auth exec command")
        || lower.contains("exec credential")
        || lower.contains("credential plugin")
        || lower.contains("get-token")
        || lower.contains("authcode")
        || lower.contains("oauth2")
        || lower.contains("authentication error")
        || lower.contains("authorization error")
}

/// Drop the `(Status { … })` / `Status { … }` Debug tail that `kube` appends,
/// then collapse internal whitespace and trailing separators.
fn strip_status_debug(raw: &str) -> String {
    let head = raw
        .find("(Status {")
        .or_else(|| raw.find("Status {"))
        .map(|i| &raw[..i])
        .unwrap_or(raw);
    let collapsed = head.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed
        .trim()
        .trim_end_matches([':', '-', ' '])
        .trim()
        .to_string();
    if trimmed.is_empty() {
        "Cluster API error (see log for details).".to_string()
    } else {
        trimmed
    }
}

/// Format the conditions of a resource for the full-message viewer.
/// Each condition is an icon + type + status header followed by its
/// transition/generation metadata and the humanized message body.
pub fn format_conditions_viewer(
    kind: &str,
    namespace: &str,
    name: &str,
    conditions: &[k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition],
) -> String {
    let mut out = format!("{kind}: {namespace}/{name}\n\n");
    if conditions.is_empty() {
        out.push_str("No conditions reported.\n");
        return out;
    }
    for (i, c) in conditions.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let icon = match c.status.as_str() {
            "True" => "✓",
            "False" => "✗",
            _ => "…",
        };
        let reason = if c.reason.is_empty() {
            String::new()
        } else {
            format!("  ({})", c.reason)
        };
        out.push_str(&format!(
            "{icon} {ty:<14} {status}{reason}\n",
            ty = c.type_,
            status = c.status,
        ));
        let transition = c.last_transition_time.0.to_string();
        out.push_str(&format!("   transition: {transition}"));
        if let Some(generation) = c.observed_generation {
            out.push_str(&format!("   gen: {generation}"));
        }
        out.push('\n');
        for line in humanize_condition_message(&c.message) {
            out.push_str("   ");
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Strip tf-controller's RPC framing and split a condition message into
/// logical lines. tf-controller wraps every runner error in
/// "error running <Phase>: rpc error: code = <Code> desc = exit status N\n\n"
/// before the actual terraform output. The framing has no diagnostic value,
/// so peel it off and split on real newlines so the renderer can display
/// the structured terraform output instead of one wall of text.
///
/// Returns the original message split on '\n' if no framing matches.
pub fn humanize_condition_message(msg: &str) -> Vec<String> {
    strip_runner_rpc_framing(msg)
        .split('\n')
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn strip_runner_rpc_framing(msg: &str) -> &str {
    if !msg.starts_with("error running ") {
        return msg;
    }
    let Some(pos) = msg.find("exit status ") else {
        return msg;
    };
    let mut cursor = pos + "exit status ".len();
    let bytes = msg.as_bytes();
    while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
        cursor += 1;
    }
    let rest = msg[cursor..].trim_start_matches(['\n', '\r', ' ']);
    if rest.is_empty() { msg } else { rest }
}

/// Classified state derived from a Ready condition. The split between
/// `Reconciling` and `Failed` exists because tofu-controller (and Flux
/// generally) writes `Ready=False, reason=Progressing` at the start of
/// every reconcile and flips back to `True` (or `False` with a real
/// failure reason) when it finishes. Without this split, the UI paints
/// every routine reconcile as a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyState {
    /// Ready=True
    True,
    /// Ready=False, reason indicates an in-flight reconcile.
    Reconciling,
    /// Ready=False with any other reason — a real failure.
    Failed,
    /// Ready exists but status is neither True nor False.
    Unknown,
    /// No Ready condition (or no conditions at all).
    Missing,
}

impl ReadyState {
    /// True only for real failures — used by failures-only filters and
    /// header failure counts so they don't flicker on every reconcile.
    pub fn is_real_failure(self) -> bool {
        matches!(self, ReadyState::Failed)
    }
}

/// Reasons that indicate a reconcile is in progress (or otherwise not a
/// real failure). `Progressing` is the standard Flux GOTK reason; older
/// or variant code paths use the next two. `Initializing` is what
/// tofu-controller emits with `Ready=Unknown` on a fresh resource.
///
/// `DriftDetected` is tofu-controller-specific: when the controller
/// notices live infra has drifted from desired state it flashes
/// `Ready=False, reason=DriftDetected` for a few hundred milliseconds
/// before the auto-apply puts things back. That's expected behaviour,
/// not a failure — but it produces a visible red flicker on the list
/// view if we don't classify it as in-flight.
const RECONCILING_REASONS: &[&str] = &[
    "Progressing",
    "ReconciliationProgressing",
    "Reconciling",
    "Initializing",
    "DriftDetected",
];

/// Classify the Ready condition for display and filtering.
///
/// The Reconciling state has three signals, in order of reliability:
///   1. A `Reconciling` condition with `status=True` — tofu-controller
///      sets this whenever a reconcile is in flight, regardless of what
///      Ready currently says. Strongest indicator; catches transient
///      Ready=False flashes that use reason strings we don't recognize.
///   2. A reconciling reason on the Ready condition itself (covers
///      tofu-controller events that don't set the Reconciling condition
///      — most flickers fall into this bucket).
///   3. `Ready=Unknown` — in practice tofu-controller only emits this
///      mid-reconcile (fresh resource, between phases), so it's never
///      really "unknown" in the literal sense. Treat as Reconciling.
///      The `ReadyState::Unknown` variant is retained only for a
///      theoretically-possible-but-never-observed status string.
pub fn classify_ready(
    conditions: Option<&Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition>>,
) -> ReadyState {
    let Some(cs) = conditions else {
        return ReadyState::Missing;
    };
    let Some(ready) = cs.iter().find(|c| c.type_ == "Ready") else {
        return ReadyState::Missing;
    };

    // Ready=True wins — a successful reconcile may leave a stale
    // Reconciling=True for a brief moment, but Ready=True means the
    // last reconcile succeeded, so show that.
    if ready.status == "True" {
        return ReadyState::True;
    }

    // Ready=Unknown is effectively a reconcile-in-flight state for
    // tofu-controller — collapse it before the reason check so the
    // user sees "…" instead of an alarming "Unknown" label.
    if ready.status == "Unknown" {
        return ReadyState::Reconciling;
    }

    let reconciling_active = cs
        .iter()
        .any(|c| c.type_ == "Reconciling" && c.status == "True");
    let is_reconciling_reason = RECONCILING_REASONS.iter().any(|r| *r == ready.reason);

    if reconciling_active || is_reconciling_reason {
        return ReadyState::Reconciling;
    }

    match ready.status.as_str() {
        "False" => ReadyState::Failed,
        _ => ReadyState::Unknown,
    }
}

/// Parse a Kubernetes/Go duration string (e.g. "1h", "30m", "10m0s", "1h30m") to seconds.
pub fn parse_k8s_duration(s: &str) -> Option<i64> {
    let mut total: i64 = 0;
    let mut num_buf = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num_buf.push(ch);
        } else {
            let n: i64 = num_buf.parse().ok()?;
            num_buf.clear();
            match ch {
                'h' => total += n * 3600,
                'm' => total += n * 60,
                's' => total += n,
                _ => return None,
            }
        }
    }
    if total > 0 { Some(total) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};

    #[test]
    fn is_auth_error_matches_oidc_plugin_failures_only() {
        assert!(is_auth_error(
            "ServiceError: auth exec command \"kubectl\" \"oidc-login\" failed: oauth2 error"
        ));
        assert!(is_auth_error("get-token: authentication error"));
        // Ordinary failures must NOT be treated as auth (they should retry,
        // not tear the connection down and demand a re-login).
        assert!(!is_auth_error(
            "watch stream failed: connection reset by peer"
        ));
        assert!(!is_auth_error(
            "the server could not find the requested resource"
        ));
    }

    #[test]
    fn error_chain_flattens_sources_for_auth_detection() {
        use std::fmt;
        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "oauth2 error: authorization error")
            }
        }
        impl std::error::Error for Inner {}
        #[derive(Debug)]
        struct Outer(Inner);
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "watch stream failed")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let s = error_chain(&Outer(Inner));
        assert_eq!(s, "watch stream failed: oauth2 error: authorization error");
        // The top-level message alone wouldn't match, but the chain does.
        assert!(!is_auth_error("watch stream failed"));
        assert!(is_auth_error(&s));
    }

    #[test]
    fn humanize_maps_exec_auth_to_reauth_hint() {
        let raw = r#"Runner pods unavailable: ServiceError: auth exec command 'KUBERNETES_EXEC_INFO="{\"kind\":\"ExecCredential\"}"' "kubectl" "oidc-login" failed: error: get-token: oauth2 error: authorization error: context deadline exceeded"#;
        let out = humanize_cluster_error(raw);
        assert!(out.contains("authentication failed"), "got: {out}");
        assert!(out.contains("Ctrl-X"), "got: {out}");
        assert!(!out.contains("KUBERNETES_EXEC_INFO"), "raw leaked: {out}");
    }

    #[test]
    fn humanize_maps_401_to_reauth_hint() {
        let raw = r#"Kustomization watcher failed: failed to perform initial object list: ApiError: Unauthorized: Failed to parse error data (Status { status: Some(Failure), code: 401, message: "Unauthorized\n", reason: "Failed to parse error data", details: None })"#;
        let out = humanize_cluster_error(raw);
        assert!(out.contains("Unauthorized (401)"), "got: {out}");
        assert!(out.contains("Ctrl-X"), "got: {out}");
        assert!(!out.contains("Status {"), "debug framing leaked: {out}");
    }

    #[test]
    fn humanize_maps_403_to_forbidden() {
        let out = humanize_cluster_error("ApiError: Forbidden (code: 403)");
        assert!(out.contains("Forbidden (403)"), "got: {out}");
    }

    #[test]
    fn humanize_maps_connection_errors() {
        for raw in [
            "error trying to connect: tcp connect error: Connection refused (os error 61)",
            "dns error: failed to lookup address information",
        ] {
            let out = humanize_cluster_error(raw);
            assert!(out.contains("Cannot reach the cluster"), "got: {out}");
        }
    }

    #[test]
    fn humanize_strips_status_framing_from_unknown_errors() {
        let raw = "Terraform watcher failed: something odd (Status { code: 500, reason: \"x\" })";
        let out = humanize_cluster_error(raw);
        assert_eq!(out, "Terraform watcher failed: something odd");
    }

    fn make_condition(type_: &str, status: &str, reason: &str, message: &str) -> Condition {
        Condition {
            last_transition_time: Time(jiff::Timestamp::UNIX_EPOCH),
            message: message.into(),
            observed_generation: None,
            reason: reason.into(),
            status: status.into(),
            type_: type_.into(),
        }
    }

    #[test]
    fn format_conditions_viewer_picks_icons_per_status() {
        let conds = vec![
            make_condition("Ready", "True", "OK", "ok"),
            make_condition("Apply", "False", "Failed", "boom"),
            make_condition("Plan", "Unknown", "InProgress", "planning"),
        ];
        let out = format_conditions_viewer("Terraform", "ns", "name", &conds);
        assert!(out.starts_with("Terraform: ns/name\n\n"));
        assert!(out.contains("✓ Ready"), "True should use ✓: {out}");
        assert!(out.contains("✗ Apply"), "False should use ✗: {out}");
        assert!(out.contains("… Plan"), "Unknown should use …: {out}");
    }

    #[test]
    fn format_conditions_viewer_strips_rpc_framing_in_message_body() {
        let conds = vec![make_condition(
            "Ready",
            "False",
            "TFExecPlanFailed",
            "error running Plan: rpc error: code = Internal desc = exit status 1\n\nError: boom",
        )];
        let out = format_conditions_viewer("Terraform", "ns", "name", &conds);
        assert!(
            !out.contains("rpc error"),
            "framing should be stripped: {out}"
        );
        assert!(
            out.contains("Error: boom"),
            "humanized body should appear: {out}"
        );
    }

    #[test]
    fn humanize_strips_plan_rpc_framing() {
        let raw = "error running Plan: rpc error: code = Internal desc = exit status 1\n\nError: Invalid value for variable\n\n  on variables.tf line 1:\n   1: foo\n";
        let lines = humanize_condition_message(raw);
        assert_eq!(lines[0], "Error: Invalid value for variable");
        assert_eq!(lines[1], "  on variables.tf line 1:");
        assert_eq!(lines[2], "   1: foo");
    }

    #[test]
    fn humanize_strips_apply_rpc_framing() {
        let raw =
            "error running Apply: rpc error: code = Internal desc = exit status 2\nError: boom";
        let lines = humanize_condition_message(raw);
        assert_eq!(lines, vec!["Error: boom"]);
    }

    #[test]
    fn humanize_passes_through_unframed_messages() {
        let raw = "GitRepository.source.toolkit.fluxcd.io \"foo\" not found";
        let lines = humanize_condition_message(raw);
        assert_eq!(lines, vec![raw]);
    }

    #[test]
    fn classify_ready_splits_reconciling_from_real_failure() {
        let progressing = vec![make_condition(
            "Ready",
            "False",
            "Progressing",
            "reconciliation in progress",
        )];
        assert_eq!(
            classify_ready(Some(&progressing)),
            ReadyState::Reconciling,
            "Ready=False with reason=Progressing must be Reconciling"
        );
        assert!(
            !classify_ready(Some(&progressing)).is_real_failure(),
            "Progressing must not count as a real failure"
        );

        let failed = vec![make_condition(
            "Ready",
            "False",
            "TerraformPlanFailed",
            "boom",
        )];
        assert_eq!(classify_ready(Some(&failed)), ReadyState::Failed);
        assert!(classify_ready(Some(&failed)).is_real_failure());

        let ok = vec![make_condition(
            "Ready",
            "True",
            "ReconciliationSucceeded",
            "",
        )];
        assert_eq!(classify_ready(Some(&ok)), ReadyState::True);

        // Plain Ready=Unknown (no reason) is treated as Reconciling — in
        // tofu-controller this is always a mid-reconcile state, never a
        // genuine "we don't know" state.
        let unknown = vec![make_condition("Ready", "Unknown", "", "")];
        assert_eq!(classify_ready(Some(&unknown)), ReadyState::Reconciling);

        // Ready=Unknown + reason=Initializing is the same in-flight state
        // with a friendlier reason — classification is identical.
        let initializing = vec![make_condition("Ready", "Unknown", "Initializing", "")];
        assert_eq!(classify_ready(Some(&initializing)), ReadyState::Reconciling);

        let empty: Vec<Condition> = vec![];
        assert_eq!(classify_ready(Some(&empty)), ReadyState::Missing);
        assert_eq!(classify_ready(None), ReadyState::Missing);

        // Ready=False with an unknown reason but Reconciling=True is the
        // in-flight reconcile state — the Reconciling condition catches
        // cases where tofu-controller uses a reason we haven't enumerated.
        let unknown_reason_but_reconciling = vec![
            make_condition("Ready", "False", "SomeReasonWeDontKnow", "..."),
            make_condition("Reconciling", "True", "Progressing", "..."),
        ];
        assert_eq!(
            classify_ready(Some(&unknown_reason_but_reconciling)),
            ReadyState::Reconciling,
            "Reconciling=True must override an unrecognized Ready=False reason"
        );

        // But Ready=True still wins over a stale Reconciling=True so a
        // freshly-finished resource shows green immediately.
        let ready_true_with_stale_reconciling = vec![
            make_condition("Ready", "True", "ReconciliationSucceeded", ""),
            make_condition("Reconciling", "True", "Progressing", ""),
        ];
        assert_eq!(
            classify_ready(Some(&ready_true_with_stale_reconciling)),
            ReadyState::True
        );

        // tofu-controller flashes Ready=False/DriftDetected during an
        // auto-apply cycle. Reconciling condition is typically absent,
        // so the only signal is the reason — it must classify as
        // Reconciling, not Failed.
        let drift_detected = vec![make_condition("Ready", "False", "DriftDetected", "")];
        assert_eq!(
            classify_ready(Some(&drift_detected)),
            ReadyState::Reconciling
        );
        assert!(!classify_ready(Some(&drift_detected)).is_real_failure());
    }

    #[test]
    fn humanize_falls_back_when_strip_yields_empty() {
        let raw = "error running Plan: rpc error: code = Internal desc = exit status 1";
        let lines = humanize_condition_message(raw);
        assert_eq!(lines, vec![raw]);
    }
}
