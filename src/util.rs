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
            _ => "⋯",
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
        let raw = "error running Apply: rpc error: code = Internal desc = exit status 2\nError: boom";
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
    fn humanize_falls_back_when_strip_yields_empty() {
        let raw = "error running Plan: rpc error: code = Internal desc = exit status 1";
        let lines = humanize_condition_message(raw);
        assert_eq!(lines, vec![raw]);
    }
}
