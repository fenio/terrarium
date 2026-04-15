/// Format a duration in seconds to a human-readable relative time string.
/// e.g. 90061 -> "1d", 7200 -> "2h", 300 -> "5m", 45 -> "45s"
pub fn format_duration(total_secs: i64) -> String {
    let days = total_secs / 86400;
    let hours = total_secs / 3600;
    let minutes = total_secs / 60;

    if days > 0 {
        format!("{}d", days)
    } else if hours > 0 {
        format!("{}h", hours)
    } else if minutes > 0 {
        format!("{}m", minutes)
    } else {
        format!("{}s", total_secs)
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
